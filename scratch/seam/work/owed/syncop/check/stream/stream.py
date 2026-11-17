#!/usr/bin/env python3
"""The WAL as a sealed record log: JRP's Stream class (design-free sec. 4),
ADJ's Journal class (design-adjacent sec. 10) and today's LogCipher with the
engine's open-time cut (decision (b): vault.rs:1767-1858, db.rs:27924-27935).

A WAL is resumed: after a crash the store replays it and appends to it
(db.rs:27929-27936), so this model branches: at 'X' (process crash) the page
cache, at 'P' (power loss) every 'any' view, each recovered, judged, and then
written on by a fresh Handle (a new open: a new nonce prefix, or a new
session key) at the recovered end, after the engine's cut if it makes one.

File: units; unit 0 the header; sectors of SU units, atomic.  A record of b
bytes takes 2 + b units (len+nonce, body, tag), unit i holding (img, i).
Replay reads records from unit 1 to the first that is not whole or fails.

fmt:
  stream   key derived per file (K_rec), nonce = (prefix per open, count),
           AAD = (stored offset, len)                      [design-free sec. 4]
  journal  key per open (a session; the reader knows every session's key,
           i.e. E4 F1's header fix assumed), nonce = (seq, attempt),
           AAD = clear part (session idx, seq, len), seq contiguous  [ADJ sec. 10]
cut (what the engine's `if len > valid_len { set_len; sync_data }` does at open):
  none     the layer's len() is the plaintext of the records replay returns,
           so len == valid_len and nothing is cut (the designs as written)
  sync     the stored file is cut after the last record that verifies, and
           synced (today: LogCipher's lengths are stored offsets, db.rs:4573-4581)
  nosync   cut, not synced (MUTANT)
chain:     the AAD also carries the previous record's tag (FIX candidate)
sess_order: journal replay refuses a record of an older session after a newer
           one (E4's first_seq window, FIX candidate)
"""
import itertools, sys, collections
from cand import Dev, World, Orc, Tally, Fail, TORN, judge, short

DEFAULT = dict(SU=3, fmt='stream', cut='none', chain=False, sess_order=False,
               retry_append=False, count_reuse=False, prefix_fixed=False,
               over_last=False,   # MUTANT: a write re-seals the last record with the new bytes, in place
               nocheck=False,     # MUTANT: replay takes a record whose head is there, tag unchecked
               # ---- SYNCOP (work/owed/syncop): syncs in flight together
               wait=False,        # the sequencer: a sync that finds one in flight waits for it
               sync_rule='assign',  # assign: synced := what the sync that just completed covered
                                  # max: synced := the newest sync, by issue, among those completed
               early=True,        # sync() answers at once when synced covers what was written
                                  # (the engine asks the log what it owes: db.rs Wal::sync)
               keep_err=True,     # a failed sync is kept: no completion after it acknowledges
               err_later_only=False,  # MUTANT: the kept error answers syncs issued after it only
               cover_at_completion=False)  # MUTANT: synced := what was written when the sync completed


class StreamH:
    def __init__(s, W, o, orc, fid, start=None):
        s.W, s.o, s.orc, s.fid = W, o, orc, fid
        s.kf = ('krec', fid)
        s.sidx = 0
        if start is None:
            hdr = W.seal(('kf', fid), W.uniq('rnd'), ('hdr', fid), kind='hdr')
            W.dev.write(s.write_units(0, [hdr]))
            # the chain starts at the header's own tag: a header written again
            # (an empty log created over a lost one) starts a new chain
            s.end, s.content, s.prev, s.seq, s.sessions = 1, [], hdr, -1, [W.uniq('S')]
        else:
            s.end, s.content, s.prev, s.seq, s.sessions = start
            s.sessions = list(s.sessions) + [W.uniq('S')]
        s.sidx = len(s.sessions) - 1
        s.prefix = ('pfx', fid) if o['prefix_fixed'] else W.uniq('pfx')
        s.count = 0
        s.att = {}
        s.failed = []
        s.infl = {}
        s.synced, s.synced_tok = s.orc.at(), 0   # SYNCOP: a fresh open's replayed content counts as
        s.doomed = set()
        s.queued, s.lost = 0, None               # synced only after a sync (engine: db.rs open marks)
        if start is not None:
            s.synced = -1

    def write_units(s, pos, units, extra_mid=None):
        SU = s.o['SU']
        out = {}
        for i, uval in enumerate(units):
            u = pos + i
            sec = u // SU
            if sec not in out:
                cur = s.W.dev.cur(('ss', sec))
                out[sec] = list(cur) if cur is not None else [None] * SU
            out[sec][u % SU] = uval
        objs = [(('ss', sec), tuple(v)) for sec, v in sorted(out.items())]
        return objs

    def seal_rec(s, at, bts):
        o, W = s.o, s.W
        n = 2 + len(bts)
        if o['fmt'] == 'stream':
            key = s.kf
            nonce = (s.prefix, s.count)
            aad = (at, len(bts), s.prev if o['chain'] else None)
            seq = None
        else:
            key = s.sessions[s.sidx]
            q = s.seq + 1
            nonce = ('j', q, s.att.get(q, 0))
            aad = (s.sidx, q, len(bts), s.prev if o['chain'] else None)
            seq = q
        img = W.seal(key, nonce, (aad, tuple(bts)), kind='rec', key_=key, aad=aad, b=tuple(bts),
                     n=n, sidx=s.sidx, seq=seq)
        return img, n

    def write(s, nb, fail=None, mid=None):
        W, o = s.W, s.o
        new = [W.uniq('b') for _ in range(nb)]
        bts = s.failed + new
        content2 = s.content + bts
        at = s.end
        if o['over_last'] and getattr(s, 'last', None):
            at, lb, pv = s.last
            bts = list(lb) + bts           # MUTANT: the newest record written again, longer
            s.prev = pv
        img, n = s.seal_rec(at, bts)
        objs = s.write_units(at, [(img, i) for i in range(n)])
        if fail is not None:
            W.dev.write(s.write_units(s.end, [('G', W.uniq('g'))] * n))
            s.failed = bts
            if o['fmt'] == 'stream':
                if not o['count_reuse']:
                    s.count += 1
            else:
                q = s.seq + 1
                if not o['count_reuse']:
                    s.att[q] = s.att.get(q, 0) + 1
            if o['retry_append']:
                s.end += n           # MUTANT: the retry lands after the failed bytes
            raise Fail()
        if mid:
            for j in range(len(objs)):
                mid(W.dev.pc(objs[:j]), content2)
        W.dev.write(objs)
        if o['fmt'] == 'stream':
            s.count += 1
        else:
            s.seq += 1
        s.last = (at, tuple(bts), s.prev)
        s.end = at + n
        s.prev = img
        s.failed = []
        s.content = content2
        s.orc.done(s.content)

    def sync_begin(s):
        tok = s.W.dev.sync_begin()
        s.infl[tok] = s.orc.at()
        return tok

    def sync_end(s, tok, dev=True):
        if tok in s.doomed:                     # CHECK: the error errseq handed to this sync
            s.doomed.discard(tok)
            return s.sync_fail(tok)
        if dev:
            s.W.dev.sync_end(tok)
        idx = s.infl.pop(tok)
        failed_before = s.lost is not None and (not s.o['err_later_only'] or tok > s.lost)
        if s.o['keep_err'] and failed_before:
            s.after_completion()
            return
        if s.o['cover_at_completion']:
            idx = s.orc.at()                      # MUTANT: covers what is written now
        if s.o['sync_rule'] == 'assign' or tok > s.synced_tok:
            s.synced, s.synced_tok = idx, tok
        s.orc.acked(idx)
        s.after_completion()

    def steal(s, which):
        # CHECK 'Y'/'V': errseq's one report on a shared description: `which`
        # returns Ok over a failed writeback; the other reports the failure
        if len(s.infl) < 2:
            return
        tok = max(s.infl) if which == 'newest' else min(s.infl)
        other = min(s.infl) if which == 'newest' else max(s.infl)
        s.W.dev.sync_fail(tok)
        s.doomed.add(other)
        s.sync_end(tok, dev=False)

    def sync_fail(s, tok):
        if tok in s.W.dev.infl:
            s.W.dev.sync_fail(tok)
        s.infl.pop(tok)
        if s.lost is None:
            s.lost = tok
        s.after_completion()

    def after_completion(s):
        if s.o['wait'] and s.queued and not s.infl:
            s.queued -= 1
            s.sync_begin()

    def request_sync(s):
        if s.o['wait'] and (s.infl or s.queued):
            s.queued += 1
            return None
        return s.sync_begin()

    def complete(s, which, fail=False):
        if not s.infl:
            return
        tok = min(s.infl) if which == 'oldest' else max(s.infl)
        if fail:
            s.sync_fail(tok)
        else:
            s.sync_end(tok)

    def sync(s):
        if s.o['wait']:
            while s.infl:
                s.sync_end(min(s.infl))
            while s.queued:
                s.queued -= 1
                s.sync_end(s.sync_begin())
        if (s.o['early'] and (s.lost is None or s.o.get('early_ignores_err')) and s.synced >= s.orc.at()):
            s.orc.acked(s.orc.at())               # nothing owed: answered without a sync
            return
        s.sync_end(s.sync_begin())


def units_of(v, o, upto=64):
    SU = o['SU']
    out = []
    for sec in range(upto):
        x = v.get(('ss', sec))
        out += list(x) if x is not None else [None] * SU
    return out


def recover(W, v, o, fid, sessions):
    """returns (content, unreadable, end, prev, last seq)"""
    u = units_of(v, o)
    h = u[0]
    if h is None or not isinstance(h, int) or W.imgs[h]['kind'] != 'hdr':
        return (), [], None, None, -1
    p, content, prev, seq, sidx_max = 1, [], h, -1, -1
    while p < len(u):
        x = u[p]
        if not (isinstance(x, tuple) and len(x) == 2 and isinstance(x[0], int) and x[1] == 0):
            break
        m = W.imgs.get(x[0])
        if m is None or m['kind'] != 'rec':
            break
        n = m['n']
        if p + n > len(u) or any(u[p + i] != (x[0], i) for i in range(n)):
            if o['nocheck'] and p + n <= len(u):
                content += [None] * len(m['b'])     # MUTANT: the torn record's bytes taken
                p += n
                continue
            break
        if o['fmt'] == 'stream':
            if m['key'] != ('krec', fid):
                break
            at, ln, pv = m['aad']
            if at != p or ln != len(m['b']):
                break
            if o['chain'] and pv != prev:
                break
        else:
            if m['sidx'] >= len(sessions) or m['key'] != sessions[m['sidx']]:
                break
            si, q, ln, pv = m['aad']
            if q != seq + 1:
                break
            if o['sess_order'] and si < sidx_max:
                break
            if o['chain'] and pv != prev:
                break
            seq = q
            sidx_max = max(sidx_max, si)
        content += list(m['b'])
        prev = x[0]
        p += n
    return tuple(content), [], p, prev, seq


class ResumeRunner:
    def __init__(s, o, name, maxbranch=None):
        s.o = dict(DEFAULT, **o)
        s.T = Tally(name)
        s.maxbranch = maxbranch

    def run(s, ops):
        W = World()
        fid = W.uniq('fid')
        orc = Orc()
        h = StreamH(W, s.o, orc, fid)
        s.go(W, h, orc, fid, ops, 0)
        s.T.runs += 1

    def check(s, W, orc, fid, h, ops, v, legal, ackhist=None, model='any'):
        res = recover(W, v, s.o, fid, h.sessions)
        vd = judge(res[:2], legal, orc.hist, orc.ack if model == 'any' else len(orc.hist) - 1)
        s.T.add(model, vd, lambda: {'ops': ops, 'view': short(v), 'rec': res[0], 'legal': sorted(legal, key=len)})
        return vd, res

    def resume(s, W2, orc2, fid, h, res, ops, i):
        content, _, end, prev, seq = res
        orc2.cut_to(content if end is not None else ())
        # the engine's cut at open (db.rs:27929-27934): today's lengths are
        # stored offsets, so a file longer than its last whole record is cut
        # there and synced; a file with no valid header is cut to nothing
        cut = s.o['cut']
        if cut in ('sync', 'nosync'):
            e = end if end is not None else 0
            u = units_of(W2.dev.pc(), s.o)
            if any(x is not None for x in u[e:]):
                W2.dev.write(h_cut_objs(W2, s.o, e, len(u)))
                if cut == 'sync':
                    W2.dev.sync_end(W2.dev.sync_begin())
        if end is None:
            h2 = StreamH(W2, s.o, orc2, fid)      # a restart: a table of one session
        else:
            h2 = StreamH(W2, s.o, orc2, fid, start=(end, list(content), prev, seq, h.sessions))
        s.go(W2, h2, orc2, fid, ops, i)

    def go(s, W, h, orc, fid, ops, i):
        while i < len(ops):
            op = ops[i]
            i += 1
            last = i == len(ops)
            if isinstance(op, int):
                def mid(view, new, W=W, orc=orc, h=h):
                    s.check(W, orc, fid, h, ops, view, {orc.hist[-1], tuple(new)}, model='proc')
                h.write(op, mid=mid if last else None)
            elif op == 'S':
                h.sync()
            elif op == 'B':
                h.request_sync()
            elif op == 'E':
                h.complete('oldest')
            elif op == 'Y':
                h.steal('newest')
            elif op == 'V':
                h.steal('oldest')
            elif op == 'N':
                h.complete('newest')
            elif op == 'F':
                h.complete('oldest', fail=True)
            elif op == 'G':
                h.complete('newest', fail=True)
            elif isinstance(op, tuple) and op[0] == 'f':
                try:
                    h.write(op[1], fail=True)
                except Fail:
                    pass
            elif op == 'X':
                v = W.dev.pc()
                vd, res = s.check(W, orc, fid, h, ops, v, {orc.hist[-1]}, model='proc')
                if vd:
                    return
                s.resume(W.clone(), orc.copy(), fid, h, res, ops, i)
                return
            elif op == 'P':
                legal = orc.legal()
                seen = set()
                branches = []
                for v in W.dev.any_views():
                    vd, res = s.check(W, orc, fid, h, ops, v, legal)
                    if vd:
                        continue
                    key = frozenset(v.items())
                    if key in seen:
                        continue
                    seen.add(key)
                    branches.append((v, res))
                for v, res in branches:
                    s.resume(W.clone_after_loss(v), orc.copy(), fid, h, res, ops, i)
                return
        v = W.dev.pc()
        s.check(W, orc, fid, h, ops, v, {orc.hist[-1]}, model='proc')
        legal = orc.legal()
        for v in W.dev.any_views():
            s.check(W, orc, fid, h, ops, v, legal)
        for b in W.bad:
            s.T.add('seal', (b[0], b[1]), {'ops': ops})
            break


def h_cut_objs(W, o, end, nunits):
    SU = o['SU']
    out = {}
    for u in range(end, nunits):
        sec = u // SU
        if sec not in out:
            cur = W.dev.cur(('ss', sec))
            if cur is None:
                continue
            out[sec] = list(cur)
        out[sec][u % SU] = None
    return [(('ss', sec), tuple(x) if any(y is not None for y in x) else None)
            for sec, x in sorted(out.items())]


def overlaps(t):
    k = 0
    for x in t:
        if x == 'B':
            if k >= 1:
                return True
            k += 1
        elif x in ('E', 'F', 'N', 'G'):
            k -= 1
        elif x == 'S' and k >= 1:
            return True
    return False


def seqs(alpha, maxlen):
    for n in range(1, maxlen + 1):
        for t in itertools.product(alpha, repeat=n):
            yield t


def main():
    o = {}
    for a in sys.argv[2:]:
        k, v = a.split('=', 1)
        if v in ('True', 'False'):          # SYNCOP: 'False' was a truthy string
            v = v == 'True'
        else:
            try:
                v = int(v)
            except ValueError:
                pass
        o[k] = v
    fam = sys.argv[1]
    maxlen = o.pop('maxlen', 6)
    secs = float(o.pop('secs', 540))    # SYNCOP: a bound inside timeout 600
    import time as _t
    t0, nseq, stopped = _t.time(), 0, False
    name = 'stream %s %s maxlen=%d' % (fam, ' '.join('%s=%s' % kv for kv in sorted(o.items())), maxlen)
    R = ResumeRunner(o, name)
    if fam == 'base':
        alpha = [1, 2, 'S', 'B', 'E', 'X', 'P']
        ok = lambda t: t.count('P') <= 1 and t.count('X') <= 2
    elif fam == 'fail':
        alpha = [1, 2, 'S', 'X', 'P', ('f', 1), ('f', 2)]
        ok = lambda t: t.count('P') <= 1 and any(isinstance(x, tuple) for x in t)
    elif fam == 'ooo':
        alpha = [1, 2, 'S', 'B', 'E', 'N']
        ok = lambda t: overlaps(t)
    elif fam == 'ooox':
        alpha = [1, 2, 'S', 'B', 'E', 'N', 'X', 'P']
        ok = lambda t: overlaps(t) and t.count('P') <= 1 and t.count('X') <= 1
    elif fam == 'steal':
        alpha = [1, 2, 'S', 'B', 'E', 'N', 'Y', 'V']
        ok = lambda t: any(x in ('Y', 'V') for x in t)
    elif fam == 'sfail':
        alpha = [1, 2, 'S', 'B', 'E', 'N', 'F', 'G']
        ok = lambda t: any(x in ('F', 'G') for x in t)
    elif fam == 'twoP':
        alpha = [1, 2, 'S', 'P']
        ok = lambda t: t.count('P') <= 2
    else:
        raise SystemExit('family?')
    for t in seqs(alpha, maxlen):
        if not ok(t):
            continue
        k, good = 0, True
        for x in t:
            if x == 'B':
                k += 1
                good &= k <= 2
            elif x in ('E', 'F'):
                good &= k > 0
                k -= 1
            elif x in ('N', 'G', 'Y', 'V'):
                good &= k > 1          # with one outstanding, newest == oldest
                k -= 1
            elif x in ('X', 'P'):
                good &= k == 0          # a crash ends the syncs in flight with the process
        if not good:
            continue
        R.run(t)
        nseq += 1
        if _t.time() - t0 > secs:
            stopped = True
            break
    print('sequences', nseq, 'STOPPED AT TIME LIMIT' if stopped else 'complete')
    R.T.report()


if __name__ == '__main__':
    main()
