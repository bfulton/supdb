#!/usr/bin/env python3
"""E4: crash model of the Pagefile design (design.md sec. 3-4), extended.

Device model (design sec. 5 and the E4 brief):
  * every object the layer writes -- a data page, a tag page, a ring frame
    slot, a header slot, a REC slot -- is several sectors and is written
    whole by one syscall; a sector is old or new, so an object at a crash
    holds its durable image, any image written to it since the last barrier
    that covers it, or a mix, which verifies as nothing (TORN).  A mix of
    two images never authenticates, so collapsing the sector masks of one
    object to TORN loses no state that recovery can tell apart.
  * unsynced writes land in any subset and order ('any');
  * fdatasync(ino) begun at tb and completed: every object dirty at tb holds
    its tb-image or a later one; objects first dirtied after tb are free;
  * a process crash keeps the page cache: every completed write call;
  * directory operations are journaled one by one; without sync_dir any
    subset of them lands, in order ('subset', POSIX); 'prefix' is ext4's
    ordered journal.
The state set grows monotonically between two completed barriers, so the
power-loss states are enumerated just before each sync completes and at
the end of each run: every earlier state is a member of one of those sets
and is judged against a legal set that can only have grown since.

Crypto model: a sealing is an image id; it verifies at a location only if
every sector of the object holds that image, the image was sealed for that
location, and under the key the reader derives (ownership, session table).
A tag or MAC is the image id.  Every (key, nonce) sealed is logged and a
second sealing of one pair is NONCE_REUSE (different plaintext) or
NONCE_REPEAT (identical plaintext, identical ciphertext).
"""
import itertools, sys, time, collections

PB, G = 4, 2          # bytes a page, pages a group
TORN = 'TORN'


class Blocked(Exception):
    """The ring is full of pinned frames (the writer would wait)."""


class Fail(Exception):
    """An injected write error."""


class Refused(Exception):
    pass


# ---------------------------------------------------------------- device
def apply_dir(d, op):
    k = op[0]
    if k == 'create':
        d[op[1]] = op[2]
    elif k == 'rename':
        if op[1] in d:
            d[op[2]] = d.pop(op[1])
    elif k == 'link':
        if op[1] in d:
            d[op[2]] = d[op[1]]
    elif k == 'unlink':
        d.pop(op[1], None)


class Dev:
    def __init__(s):
        s.t = 0
        s.dur, s.pend = {}, {}
        s.ddur, s.dpend = {}, []
        s.infl, s.ntok = {}, 0

    def clone(s):
        d = Dev.__new__(Dev)
        d.t, d.ntok = s.t, s.ntok
        d.dur = dict(s.dur)
        d.pend = {k: list(v) for k, v in s.pend.items()}
        d.ddur, d.dpend = dict(s.ddur), list(s.dpend)
        d.infl = dict(s.infl)
        return d

    def write(s, ino, objs):
        s.t += 1
        for loc, v in objs:
            s.pend.setdefault((ino, loc), []).append((s.t, v))

    def sync_begin(s, ino):
        s.t += 1
        s.ntok += 1
        s.infl[s.ntok] = (ino, s.t)
        return s.ntok

    def sync_end(s, tok):
        ino, tb = s.infl.pop(tok)
        for k in [k for k in s.pend if k[0] == ino]:
            lst = s.pend[k]
            bef = [v for t, v in lst if t <= tb]
            if bef:
                s.dur[k] = bef[-1]
                rest = [(t, v) for t, v in lst if t > tb]
                if rest:
                    s.pend[k] = rest
                else:
                    del s.pend[k]

    def dop(s, op):
        s.t += 1
        s.dpend.append(op)

    def dsync(s):
        for op in s.dpend:
            apply_dir(s.ddur, op)
        s.dpend = []

    def pc(s):
        v = dict(s.dur)
        for k, lst in s.pend.items():
            v[k] = lst[-1][1]
        d = dict(s.ddur)
        for op in s.dpend:
            apply_dir(d, op)
        return v, d

    def any_views(s, dir_mode='subset'):
        keys = list(s.pend)
        ch = []
        for k in keys:
            vals = []
            d0 = s.dur.get(k)
            # a torn object that had no durable image reads as the hole did:
            # neither authenticates, so TORN is kept only over a durable image
            for x in [d0] + [v for _, v in s.pend[k]] + ([TORN] if d0 is not None else []):
                if x not in vals:
                    vals.append(x)
            ch.append(vals)
        m = len(s.dpend)
        if dir_mode == 'prefix':
            dsets = [s.dpend[:i] for i in range(m + 1)]
        else:
            dsets = [[op for op, b in zip(s.dpend, bits) if b]
                     for bits in itertools.product([0, 1], repeat=m)]
        for dops in dsets:
            d = dict(s.ddur)
            for op in dops:
                apply_dir(d, op)
            for pick in itertools.product(*ch):
                v = dict(s.dur)
                for k, x in zip(keys, pick):
                    if x is None:
                        v.pop(k, None)
                    else:
                        v[k] = x
                yield v, d

    def n_any(s):
        n = 1
        for k, lst in s.pend.items():
            d0 = s.dur.get(k)
            n *= len(set([d0] + [v for _, v in lst] + ([TORN] if d0 is not None else [])))
        return n * (2 ** len(s.dpend))

    @staticmethod
    def after_loss(t, v, d):
        n = Dev()
        n.t = t
        n.dur, n.ddur = dict(v), dict(d)
        return n


# ---------------------------------------------------------------- world
class World:
    def __init__(s):
        s.dev = Dev()
        s.imgs = {}
        s.nid = 0
        s.nonce = {}
        s.bad = []
        s.cnt = [0]       # shared counter for keys, fids, random nonces, bytes
        s.hooks = []      # called just before every sync completes

    def clone_after_loss(s, v, d):
        w = World.__new__(World)
        w.dev = Dev.after_loss(s.dev.t, v, d)
        w.imgs, w.nid, w.cnt = s.imgs, s.nid, s.cnt
        w.nonce, w.bad = dict(s.nonce), list(s.bad)
        w.hooks = []
        return w

    def clone(s):
        w = World.__new__(World)
        w.dev = s.dev.clone()
        w.imgs, w.nid, w.cnt = s.imgs, s.nid, s.cnt
        w.nonce, w.bad = dict(s.nonce), list(s.bad)
        w.hooks = []
        return w

    def uniq(s, tag):
        s.cnt[0] += 1
        return (tag, s.cnt[0])

    def seal(s, key, nonce, content, **meta):
        k = (key, nonce)
        old = s.nonce.get(k)
        if old is not None:
            s.bad.append(('NONCE_REUSE' if old != content else 'NONCE_REPEAT', k))
        else:
            s.nonce[k] = content
        s.cnt[0] += 1
        i = s.cnt[0]
        meta.update(key=key, nonce=nonce, content=content)
        s.imgs[i] = meta
        return i

    def sync(s, ino, before_end=None):
        tok = s.dev.sync_begin(ino)
        return tok

    def sync_end(s, tok):
        for h in s.hooks:
            h()
        s.dev.sync_end(tok)


def fz(d):
    """a hashable, ordered repr of a frame body for the nonce log"""
    return repr(sorted(d.items()))


# ---------------------------------------------------------------- the Log class
DEFAULT = dict(
    K=2,                # frames between CKPTs (written at write time)
    R=8,                # ring slots
    CAP=48,             # session table capacity
    pin_inflight=True,  # the in-flight sync's span is pinned
    round=True,         # verify from synced_home rounded down to its group
    rehome=True,        # re-home the tag pages of groups closed in the replayed run
    accept_stale=False,  # MUTANT: frames of a session beyond the next session's first_seq,
                        # or of sessions before the newest restart, accepted
    lazy=True,          # a resumed Handle creates its session at its first mutation
    resume_sync=False,  # FIX F1: fdatasync before a resumed Handle overwrites a header slot
    resume_trust=False,  # MUTANT: a resumed Handle takes the recovered state as synced
    retry='R0',         # R0: a failed call's frame seq is spent (design: seq never repeats)
                        # R1: the retry reuses the seq (nonce 0x11||seq||0 again)
                        # R2: FIX: the frame nonce carries an attempt counter
    ctr_rollback=False,  # MUTANT: a failed call gives back its ctr and seq
    owner_check=True,   # a page decrypts under owner(p) alone
    owner_by_run=False,  # FIX for TRUNC: only sessions present in the chosen run own pages
    trunc_ckpt=False,   # FIX for TRUNC: TRUNC is a CKPT carrying the cut page's image
    final='design',     # design: FINAL verifies the pages finish wrote
                        # verify_sh: FINAL also verifies from round_down(synced_home)
                        # deferred: FIX: finish's writes and FINAL go after the final sync
    final_trust=False,  # MUTANT: FINAL trusted without verifying its pages
    ovr_home=False,     # MUTANT: a patch below the tail written home in place while Log
    final_ovr_rec=False,  # FIX: FINAL carries the override images, overridden pages never go home
    hdr_same_slot=False,  # MUTANT: a new header overwrites the slot of the newest valid one
    restart_keeps=False,  # VARIANT: a restart appends to the table instead of starting a new one
    trunc_base=False,   # FIX for TRUNC: the session is created at the cut, base = the cut page
    rehome_invalid_only=False,  # VARIANT: re-home only tag pages that do not verify at recovery
    lazy_home_check=False,  # FIX: HOME pages a later OVR in the run overrides are not verified
    sh_rehome=False,    # FIX: synced_home stops at the first group whose re-home is due
    eager_rehome=False,  # FIX (alt): a resumed Handle re-homes at its open
)


class Orc:
    """The history of one logical file's contents and its acknowledgements."""

    def __init__(s):
        s.hist = [()]
        s.ack = 0

    def copy(s):
        o = Orc()
        o.hist, o.ack = list(s.hist), s.ack
        return o

    def done(s, content):
        s.hist.append(tuple(content))

    def at(s):
        return len(s.hist) - 1

    def acked(s, i):
        s.ack = max(s.ack, i)

    def legal(s):
        return set(s.hist[s.ack:])

    def cut_to(s, content):
        c = tuple(content)
        for i in range(len(s.hist) - 1, s.ack - 1, -1):
            if s.hist[i] == c:
                s.hist = s.hist[:i + 1]
                return
        raise AssertionError('cut_to an illegal state')


class LogH:
    """One Handle of a Log file, as section 4 describes it."""

    def __init__(s, W, ino, fid, o, orc):
        s.W, s.ino, s.fid, s.o, s.orc = W, ino, fid, o, orc
        s.kf = ('kf', fid)
        s.sessions, s.cur, s.ctr = [], None, 0
        s.hdr_seq, s.hdr_slot, s.hdr_safe = 0, None, True
        s.n, s.content, s.home, s.tail = 0, [], 0, []
        s.tags, s.open, s.gmac, s.ovr = {}, {}, {}, {}
        s.seq, s.occ, s.last_slot = -1, {}, -1
        s.att = {}
        s.newest_ckpt, s.since = None, 0
        s.synced, s.infl, s.synced_home = None, {}, 0
        s.rehome_due, s.resumed = [], False
        s.held = set()
        s.failed = []
        s.finishing = False
        s.finished = False
        s.trunc_seq = None

    # -- creation and resume
    @classmethod
    def create(cls, W, ino, fid, o, orc, hdr=None):
        """A new file, or one recovered empty: hdr = (hdr_seq, slot) of a
        header that still authenticates there, which the new one outranks."""
        h = cls(W, ino, fid, o, orc)
        h.hs_t = -1
        if hdr is not None:
            h.hdr_seq, h.hdr_slot, h.hdr_safe = hdr[0], hdr[1], False
            if len(hdr) > 2 and o['restart_keeps']:
                h.sessions = list(hdr[2])
        h.new_session('restart')
        h.frame(h.ckpt_body())       # the base every recovery starts from
        return h

    @classmethod
    def resume(cls, W, ino, fid, o, orc, st):
        h = cls(W, ino, fid, o, orc)
        h.resumed = True
        h.sessions = list(st['table'])
        h.hdr_seq, h.hdr_slot, h.hdr_safe = st['hdr_seq'], st['hdr_slot'], False
        h.content = list(st['content'])
        h.n, h.home, h.tail = len(h.content), st['home'], list(st['tail'])
        h.tags = dict(st['tags'])
        g = h.home // G
        h.open = {p: t for p, t in h.tags.items() if p // G == g and p < h.home}
        h.gmac = dict(st['gmac'])
        h.ovr = dict(st['ovr'])
        h.seq = st['last']
        h.occ = dict(st['slots'])
        h.last_slot = st['slot_of_last']
        h.newest_ckpt = st['base']
        h.since = st['last'] - st['base']
        if o['resume_trust']:
            h.synced = {'base': st['base'], 'last': st['last'], 'home': h.home}
            h.synced_home = h.home
        else:
            h.synced = {'base': st['base'], 'last': st['last'], 'home': st['sh']}
            h.synced_home = st['sh']
        h.rehome_due = (list(st['rehome'] if o['rehome_invalid_only'] else st['closed_in_run'])
                        if o['rehome'] else [])
        h.att = {}
        if o['eager_rehome'] and h.rehome_due:
            h.ensure_session()          # FIX (alt): re-home at the open, before any sync
        return h

    # -- sessions and headers
    def skey(s):
        return s.sessions[s.cur][0]

    def nctr(s):
        c = s.ctr
        s.ctr += 1
        return c

    def new_session(s, kind):
        base = 0 if kind == 'restart' else s.n // PB
        table = [] if kind == 'restart' and not s.o['restart_keeps'] else list(s.sessions)
        s.sessions = table + [(s.W.uniq('s'), kind, base, s.seq + 1)]
        s.cur = len(s.sessions) - 1
        s.ctr = 0
        s.write_header()

    def write_header(s):
        if s.o['resume_sync'] and not s.hdr_safe:
            tok = s.W.dev.sync_begin(s.ino)          # the layer's own fdatasync
            s.W.sync_end(tok)
            s.hdr_safe = True
        if s.hdr_slot is None:
            slot = 0
        elif s.o['hdr_same_slot']:
            slot = s.hdr_slot
        else:
            slot = s.hdr_slot ^ 1
        s.hdr_seq += 1
        body = (s.fid, s.hdr_seq, tuple(s.sessions))
        img = s.W.seal(s.kf, s.W.uniq('rnd'), body, kind='hdr', fid=s.fid,
                       hdr_seq=s.hdr_seq, table=tuple(s.sessions))
        s.W.dev.write(s.ino, [(('h', slot), img)])
        s.hdr_slot = slot
        s.hdr_safe = False
        s.hdr_t = s.W.dev.t

    def ensure_session(s):
        if s.cur is None:
            s.new_session('continue')
            if s.rehome_due:
                s.do_rehome()

    def do_rehome(s):
        groups = []
        for g in s.rehome_due:
            tg = tuple(s.tags[g * G + i] for i in range(G))
            timg = s.W.seal(s.skey(), ('t', g, s.nctr()), tg, kind='tag', g=g)
            s.gmac[g] = timg
            groups.append((g, timg))
        s.rehome_due = []
        s.W.dev.write(s.ino, [(('t', g), t) for g, t in groups])
        s.frame({'k': 'W', 'off': s.n, 'b': (), 'homes': (), 'groups': tuple(groups),
                 'len': s.n, 'home': s.home})

    # -- the ring
    def spans(s):
        sp = []
        if s.synced:
            sp.append((s.synced['base'], s.synced['last']))
        if s.o['pin_inflight']:
            for b, l, _ in s.infl.values():
                sp.append((b, l))
        if s.newest_ckpt is not None:
            sp.append((s.newest_ckpt, s.seq))
        return sp

    def alloc(s):
        sp = s.spans()
        R = s.o['R']
        for k in range(1, R + 1):
            c = (s.last_slot + k) % R
            old = s.occ.get(c)
            if old is None or not any(a <= old <= b for a, b in sp):
                return c
        raise Blocked()

    def frame(s, body, torn=False):
        s.seq += 1
        a = s.att.get(s.seq, 0) if s.o['retry'] == 'R2' else 0
        body = dict(body)
        img = s.W.seal(s.skey(), ('f', s.seq, a), fz(body), kind='frame',
                       sidx=s.cur, seq=s.seq, body=body)
        slot = s.alloc()
        s.occ[slot] = s.seq
        s.last_slot = slot
        s.W.dev.write(s.ino, [(('r', slot), TORN if torn else img)])
        if body['k'] == 'C':
            s.newest_ckpt = s.seq
            s.since = 0
        else:
            s.since += 1

    def ckpt_body(s):
        return {'k': 'C', 'len': s.n, 'home': s.home, 'tail': tuple(s.tail),
                'open': tuple(sorted(s.open.items())), 'sh': s.synced_home,
                'gmac': tuple(sorted(s.gmac.items())), 'ovr': tuple(sorted(s.ovr.items()))}

    def ckpt(s):
        s.frame(s.ckpt_body())

    # -- writes
    def save(s):
        return (s.n, list(s.content), s.home, list(s.tail), dict(s.tags), dict(s.open),
                dict(s.gmac), dict(s.ovr), set(s.held))

    def restore(s, sv):
        (s.n, s.content, s.home, s.tail, s.tags, s.open, s.gmac, s.ovr, s.held) = sv

    def write(s, nb, fail=None, proc=None, vals=None):
        """One write call of nb new bytes (behind any bytes of a failed call,
        which the engine resends: db.rs:4436-4456 keeps `pending`).
        fail = index of the syscall that fails, its objects written torn."""
        s.ensure_session()
        new = list(vals) if vals is not None else [s.W.uniq('b') for _ in range(nb)]
        bytes_ = s.failed + new
        sv = s.save()
        seq0, ctr0 = s.seq, s.ctr
        off = s.n
        s.content += bytes_
        s.n += len(bytes_)
        buf = s.tail + bytes_
        datas, groups, homes, ovrs = [], [], [], []
        p = s.home
        while len(buf) >= PB:
            page = tuple(buf[:PB])
            buf = buf[PB:]
            img = s.W.seal(s.skey(), ('d', p, s.nctr()), page, kind='data', p=p)
            if p in s.held:
                # a page past a TRUNC's cut that was home: not home again until a
                # synced CKPT no longer needs its old image (sec. 4 set_len)
                s.ovr[p] = img
                ovrs.append((p, img))
                s.tags.pop(p, None)
            else:
                datas.append((('d', p), img))
                homes.append((p, img))
                s.tags[p] = img
            s.open[p] = img
            if p % G == G - 1:
                g = p // G
                tg = tuple(s.open.pop(g * G + i) for i in range(G))
                if any(g * G + i in s.held for i in range(G)):
                    # a re-opened group's tag page is a page a synced CKPT names
                    for i in range(G):
                        s.open[g * G + i] = tg[i]
                else:
                    timg = s.W.seal(s.skey(), ('t', g, s.nctr()), tg, kind='tag', g=g)
                    s.gmac[g] = timg
                    groups.append((g, timg))
            p += 1
        s.home = p
        s.tail = buf
        body = {'k': 'W', 'off': off, 'b': tuple(bytes_), 'homes': tuple(homes),
                'groups': tuple(groups), 'len': s.n, 'home': s.home,
                'ovr': tuple(ovrs)}
        calls = []
        if datas:
            calls.append(('data', datas))
        if groups:
            calls.append(('tag', [(('t', g), t) for g, t in groups]))
        calls.append(('frame', body))
        for i, (kind, objs) in enumerate(calls):
            if fail is not None and i == fail:
                if kind == 'frame':
                    s.frame(objs, torn=True)
                else:
                    s.W.dev.write(s.ino, [(l, TORN) for l, _ in objs])
                s.restore(sv)
                s.failed = bytes_
                if s.o['ctr_rollback']:
                    s.seq, s.ctr = seq0, ctr0
                elif s.o['retry'] in ('R1', 'R2'):
                    if kind == 'frame':
                        s.att[s.seq] = s.att.get(s.seq, 0) + 1
                        s.seq -= 1          # the retry reseals this seq
                    # a failed data or tag syscall sealed no frame: seq unspent
                raise Fail()
            if kind == 'frame':
                s.frame(objs)
            else:
                s.W.dev.write(s.ino, objs)
            if proc and i < len(calls) - 1:
                proc(mid=tuple(s.content))
        s.failed = []
        s.orc.done(s.content)
        if s.since >= s.o['K']:
            s.ckpt()

    def patch(s, pos):
        """write_all_at below what was written: one byte at pos changes."""
        s.ensure_session()
        v = s.W.uniq('b')
        s.content[pos] = v
        p = pos // PB
        if p >= s.home:
            s.tail[pos - s.home * PB] = v
            s.frame({'k': 'W', 'off': pos, 'b': (v,), 'homes': (), 'groups': (),
                     'len': s.n, 'home': s.home, 'ovr': ()})
        else:
            page = tuple(s.content[p * PB:(p + 1) * PB])
            img = s.W.seal(s.skey(), ('d', p, s.nctr()), page, kind='data', p=p)
            if s.o['ovr_home']:
                s.W.dev.write(s.ino, [(('d', p), img)])
                s.tags[p] = img
                g = p // G
                if g in s.gmac:
                    tg = tuple(s.tags[g * G + i] for i in range(G))
                    timg = s.W.seal(s.skey(), ('t', g, s.nctr()), tg, kind='tag', g=g)
                    s.gmac[g] = timg
                    s.W.dev.write(s.ino, [(('t', g), timg)])
                    s.frame({'k': 'W', 'off': s.n, 'b': (), 'homes': ((p, img),),
                             'groups': ((g, timg),), 'len': s.n, 'home': s.home, 'ovr': ()})
                else:
                    s.open[p] = img
                    s.frame({'k': 'W', 'off': s.n, 'b': (), 'homes': ((p, img),),
                             'groups': (), 'len': s.n, 'home': s.home, 'ovr': ()})
            else:
                s.ovr[p] = img
                s.frame({'k': 'O', 'p': p, 'img': img})
        s.orc.done(s.content)
        if s.since >= s.o['K']:
            s.ckpt()

    def trunc(s, L):
        """set_len(L): sec. 4's TRUNC{len} frame; the page containing len
        becomes the Handle's tail; pages at or past it that were home are
        re-sealed only after a synced CKPT no longer needs their old images."""
        if s.o['trunc_base'] and s.cur is None:
            n0, s.n = s.n, L                # FIX: the session the cut opens owns from the cut page
            s.ensure_session()
            s.n = n0
        s.ensure_session()
        s.content = s.content[:L]
        s.n = L
        nh = L // PB
        if nh < s.home:
            for p in range(nh, s.home):
                s.tags.pop(p, None)
                s.held.add(p)
            for g in [g for g in s.gmac if (g + 1) * G > nh]:
                del s.gmac[g]
            s.open = {p: s.tags[p] for p in range((nh // G) * G, nh)}
            s.home = nh
            s.tail = s.content[nh * PB:L]
        else:
            s.tail = s.content[s.home * PB:L]
        for p in [p for p in s.ovr if p * PB >= L]:
            del s.ovr[p]
        if s.o['trunc_ckpt']:
            s.frame(dict(s.ckpt_body(), trunc=True))
        else:
            s.frame({'k': 'T', 'len': L})
        s.trunc_seq = s.seq
        s.orc.done(s.content)

    # -- syncs
    def sync_begin(s):
        idx = s.orc.at()
        tok = s.W.dev.sync_begin(s.ino)
        home = s.home
        if s.o['sh_rehome'] and s.rehome_due:
            # FIX: a sync covers no group whose tag page is still to be re-homed
            home = min(home, min(s.rehome_due) * G)
        s.infl[tok] = (s.newest_ckpt, s.seq, (home, idx, getattr(s, 'hdr_t', None)))
        return tok

    def sync_end(s, tok):
        s.W.sync_end(tok)
        b, l, (home, idx, ht) = s.infl.pop(tok)
        s.synced = {'base': b, 'last': l, 'home': home}
        s.synced_home = home
        if ht is not None and ht == getattr(s, 'hdr_t', None):
            s.hdr_safe = True
        if s.trunc_seq is not None and b is not None and b >= s.trunc_seq:
            s.held = set()
            s.trunc_seq = None
        s.orc.acked(idx)
        if s.finishing and s.o['final'] == 'deferred' and not s.infl:
            s.finishing = False
            s.finish_writes()

    def sync(s):
        s.sync_end(s.sync_begin())

    # -- finish: Log -> Once
    def finish(s):
        s.ensure_session()
        if s.o['final'] == 'deferred':
            s.finishing = True
            return
        s.finish_writes()

    def finish_writes(s):
        written = []
        fin_ovr = {}
        for p, img in sorted(s.ovr.items()):
            if s.o['final_ovr_rec']:
                fin_ovr[p] = img                          # the image rides in FINAL
            else:
                page = s.W.imgs[img]['content']
                nimg = s.W.seal(s.skey(), ('d', p, s.nctr()), page, kind='data', p=p)
                written.append((('d', p), nimg))
                fin_ovr[p] = nimg
        np_ = s.home
        last = {}
        if s.tail:
            timg = s.W.seal(s.skey(), ('d', s.home, s.nctr()), tuple(s.tail), kind='data',
                            p=s.home)
            written.append((('d', s.home), timg))
            last[s.home] = timg
            np_ = s.home + 1
        gl = (np_ - 1) // G if np_ else None
        gmac = dict(s.gmac)
        if np_ and gl not in gmac:
            tg = tuple((s.open.get(gl * G + i) or last.get(gl * G + i))
                       for i in range(G) if gl * G + i < np_)
            limg = s.W.seal(s.skey(), ('t', gl, s.nctr()), tg, kind='tag', g=gl)
            written.append((('t', gl), limg))
            gmac[gl] = limg
        if written:
            s.W.dev.write(s.ino, [w for w in written if w[0][0] == 'd'])
            tw = [w for w in written if w[0][0] == 't']
            if tw:
                s.W.dev.write(s.ino, tw)
        body = {'len': s.n, 'np': np_, 'gmac': tuple(sorted(gmac.items())),
                'ovr': tuple(sorted(fin_ovr.items())), 'written': tuple(written),
                'sh': s.synced_home}
        rimg = s.W.seal(s.skey(), ('rec', s.nctr()), fz(body), kind='rec', sidx=s.cur,
                        body=body)
        s.W.dev.write(s.ino, [(('rec', 0), rimg)])
        s.finished = True


# ---------------------------------------------------------------- recovery
def hdr_at(W, v, ino, slot, fid):
    img = v.get((ino, ('h', slot)))
    if img is None or img == TORN:
        return None
    m = W.imgs[img]
    if m['kind'] != 'hdr' or m['key'] != ('kf', fid):
        return None
    return m


def headers(W, v, ino, fid):
    out = []
    for slot in (0, 1):
        m = hdr_at(W, v, ino, slot, fid)
        if m:
            out.append((m['hdr_seq'], slot, m))
    return out


def recover_log(W, v, ino, fid, o, R=None, prev=None):
    """Returns a state dict, or raises Refused.  Implements sec. 4's
    recovery steps 1-6 and the FINAL choice of `finish`."""
    R = R or o['R']
    hs = headers(W, v, ino, fid)
    if not hs:
        if prev is not None and headers(W, v, ino, prev):
            return {'empty': True, 'why': 'recycled, header lags'}
        return {'empty': True, 'why': 'no header'}
    hdr_seq, hslot, hm = max(hs, key=lambda x: x[0])
    hinfo = (hdr_seq, hslot)
    table = list(hm['table'])
    rs = max(i for i, t in enumerate(table) if t[1] == 'restart')
    live = list(range(0 if o['accept_stale'] else rs, len(table)))

    def owner(p, upto=None):
        cand = [i for i in live if table[i][2] <= p and (upto is None or i <= upto)]
        return table[max(cand)][0] if cand else None

    # ring scan
    fr = {}
    slots = {}
    for slot in range(R):
        img = v.get((ino, ('r', slot)))
        if img is None or img == TORN:
            continue
        m = W.imgs[img]
        if m['kind'] != 'frame':
            continue
        si = m['sidx']
        if si not in live or table[si][0] != m['key']:
            continue
        q = m['seq']
        if not o['accept_stale']:
            if q < table[si][3]:
                continue
            nxt = [j for j in live if j > si]
            if nxt and q >= table[nxt[0]][3]:
                continue
        prevf = fr.get(q)
        if prevf is None or (m['nonce'][2], m['sidx']) > (prevf['nonce'][2], prevf['sidx']):
            fr[q] = m
            slots[q] = slot
    iters = 0

    # FINAL
    rimg = v.get((ino, ('rec', 0)))
    if rimg not in (None, TORN):
        rm = W.imgs[rimg]
        if rm['kind'] == 'rec' and rm['sidx'] < len(table) and table[rm['sidx']][0] == rm['key'] \
                and rm['sidx'] in live:
            b = rm['body']
            ok = o['final_trust'] or all(v.get((ino, loc)) == img for loc, img in b['written'])
            if ok and o['final'] == 'verify_sh' and not o['final_trust']:
                gm = dict(b['gmac'])
                fo = dict(b['ovr'])
                for p in range((b['sh'] // G) * G, b['np']):
                    t = fo.get(p) or tag_from_page(W, v, ino, p, gm)
                    if t is None or not page_ok(W, v, ino, p, t, None, o, ovr=p in fo and o['final_ovr_rec']):
                        ok = False
                        break
            if ok:
                gm = dict(b['gmac'])
                fo = dict(b['ovr'])
                content, bad = [], []
                for p in range(b['np']):
                    t = fo.get(p) or tag_from_page(W, v, ino, p, gm)
                    inrec = p in fo and o['final_ovr_rec']
                    if t is None or not page_ok(W, v, ino, p, t, None, o, ovr=inrec):
                        bad.append(p)
                        content += [None] * PB
                    else:
                        content += list(W.imgs[t]['content'])
                return {'empty': False, 'trusted': True, 'content': content[:b['len']],
                        'unreadable': bad, 'latent': [], 'hdr_seq': hdr_seq, 'hdr_slot': hslot,
                        'table': table}

    ckpts = sorted((q for q, m in fr.items() if m['body']['k'] == 'C'), reverse=True)
    for c in ckpts:
        iters += 1
        f = fr[c]['body']
        st = {'len': f['len'], 'home': f['home'], 'tail': list(f['tail']),
              'tags': dict(f['open']), 'gmac': dict(f['gmac']), 'ovr': dict(f['ovr']),
              'closed_in_run': [], 'base': c, 'last': c, 'sh': f['sh']}
        upto = fr[c]['sidx'] if o['owner_by_run'] else None
        lo = (f['sh'] // G) * G if o['round'] else f['sh']
        ok = True
        for p in range(lo, f['home']):
            t = st['tags'].get(p) or tag_from_page(W, v, ino, p, st['gmac'])
            if p in st['ovr']:
                continue
            if t is None or not page_ok(W, v, ino, p, t, owner(p, upto), o):
                ok = False
                break
        if not ok:
            continue
        later_ovr = set()
        if o['lazy_home_check']:
            # FIX: a page an OVR later in the run overrides is not held to its
            # HOME image, which finish may have overwritten
            q = c + 1
            while q in fr and fr[q]['body']['k'] != 'C':
                b = fr[q]['body']
                if b['k'] == 'O':
                    later_ovr.add(b['p'])
                for p, _ in b.get('ovr', ()):
                    later_ovr.add(p)
                q += 1
        q = c + 1
        while q in fr:
            iters += 1
            if iters > 10000:
                raise AssertionError('recovery does not terminate')
            w = fr[q]['body']
            if o['owner_by_run']:
                upto = max(upto, fr[q]['sidx'])
            k = w['k']
            if k == 'C':
                break
            if k == 'W':
                if not all(p in later_ovr or page_ok(W, v, ino, p, t, owner(p, upto), o)
                           for p, t in w['homes']):
                    break
                for p, t in w['homes']:
                    st['tags'][p] = t
                for p, t in w.get('ovr', ()):
                    st['ovr'][p] = t
                for g, t in w['groups']:
                    st['gmac'][g] = t
                    st['closed_in_run'].append(g)
                if w['off'] < st['len']:            # a TAIL at an older offset
                    if w['off'] < st['home'] * PB or w['off'] + len(w['b']) > st['len']:
                        break                       # not a frame of this history
                    for i, b in enumerate(w['b']):
                        st['tail'][w['off'] + i - st['home'] * PB] = b
                elif w['off'] > st['len'] or w['home'] < st['home']:
                    break
                else:
                    buf = st['tail'] + list(w['b'])
                    st['tail'] = buf[(w['home'] - st['home']) * PB:]
                    st['home'], st['len'] = w['home'], w['len']
            elif k == 'O':
                st['ovr'][w['p']] = w['img']
            elif k == 'T':
                L = w['len']
                nh = L // PB
                if nh < st['home']:
                    t = st['ovr'].get(nh) or st['tags'].get(nh) or tag_from_page(W, v, ino, nh, st['gmac'])
                    if t is None or not page_ok(W, v, ino, nh, t, owner(nh, upto), o, ovr=nh in st['ovr']):
                        break                       # the cut page does not read
                    st['tail'] = list(W.imgs[t]['content'][:L % PB])
                    for p in range(nh, st['home']):
                        st['tags'].pop(p, None)
                    for p in range((nh // G) * G, nh):      # the re-opened group's pages below the cut
                        if p not in st['tags']:
                            tp = tag_from_page(W, v, ino, p, st['gmac'])
                            if tp is not None:
                                st['tags'][p] = tp
                    for g in [g for g in st['gmac'] if (g + 1) * G > nh]:
                        del st['gmac'][g]
                    st['home'] = nh
                else:
                    st['tail'] = st['tail'][:L - st['home'] * PB]
                for p in [p for p in st['ovr'] if p * PB >= L]:
                    del st['ovr'][p]
                st['len'] = L
            st['last'] = q
            q += 1
        # the content and every page below home
        content, bad, tags = [], [], {}
        for p in range(st['home']):
            if p in st['ovr']:
                t = st['ovr'][p]
                content += list(W.imgs[t]['content'])
                continue
            t = st['tags'].get(p) or tag_from_page(W, v, ino, p, st['gmac'])
            if t is None or not page_ok(W, v, ino, p, t, owner(p, upto), o):
                bad.append(p)
                content += [None] * PB
            else:
                tags[p] = t
                content += list(W.imgs[t]['content'])
        content += st['tail']
        latent = []
        for g in range(st['home'] // G):
            m = st['gmac'].get(g)
            onpage = v.get((ino, ('t', g)))
            if m is None or onpage != m:
                if o['rehome'] and g in st['closed_in_run']:
                    continue
                if all(g * G + i in st['ovr'] for i in range(G)):
                    continue
                latent.append(g)
        st['rehome'] = [g for g in st['closed_in_run'] if v.get((ino, ('t', g))) != st['gmac'].get(g)]
        st.update(empty=False, trusted=False, content=content[:st['len']], unreadable=bad,
                  latent=latent, hdr_seq=hdr_seq, hdr_slot=hslot, table=table,
                  slots={slots[q]: q for q in range(st['base'], st['last'] + 1)},
                  slot_of_last=slots[st['last']], tags={**st['tags'], **tags})
        return st
    return {'empty': True, 'why': 'no CKPT verifies', 'table': table, 'hdr': (hdr_seq, hslot, table)}


def tag_from_page(W, v, ino, p, gmac):
    g = p // G
    m = gmac.get(g)
    if m is None:
        return None
    if v.get((ino, ('t', g))) != m:
        return None
    tg = W.imgs[m]['content']
    i = p % G
    return tg[i] if i < len(tg) else None


def page_ok(W, v, ino, p, t, okey, o, ovr=False):
    if t is None:
        return False
    m = W.imgs.get(t)
    if m is None or m.get('p') != p:
        return False
    if ovr:
        return True                 # the image is inside a verified frame or REC
    if v.get((ino, ('d', p))) != t:
        return False
    if o['owner_check'] and okey is not None and m['key'] != okey:
        return False
    return True


# ---------------------------------------------------------------- reporting
class Tally:
    def __init__(s, name):
        s.name = name
        s.states = collections.Counter()
        s.bad = collections.Counter()
        s.first = {}
        s.runs = 0
        s.blocked = 0
        s.t0 = time.time()

    def add(s, model, verdict, info):
        s.states[model] += 1
        if verdict:
            s.bad[(model, verdict[0])] += 1
            key = (model, verdict[0])
            if key not in s.first:
                s.first[key] = (verdict, info)

    def report(s, out=sys.stdout):
        tot = sum(s.states.values())
        nb = sum(s.bad.values())
        print('==', s.name, 'runs', s.runs, 'blocked', s.blocked, 'states', dict(s.states),
              'violations', nb, 'secs %.1f' % (time.time() - s.t0), file=out)
        for k in sorted(s.bad):
            print('   ', k, s.bad[k], file=out)
        for k in sorted(s.first):
            verdict, info = s.first[k]
            print('  first', k, verdict, file=out)
            print('     ops', info.get('ops'), file=out)
            if info.get('trace'):
                print('     trace', info['trace'], file=out)
            if info.get('view'):
                print('     view', info['view'], file=out)
            if info.get('rec'):
                print('     recovered', info['rec'], file=out)
        out.flush()
        return nb
