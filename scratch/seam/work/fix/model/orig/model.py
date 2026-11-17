#!/usr/bin/env python3
"""Crash model of the pagefile design's Log class (design-draft.md sec. 3-4):
an append-and-sync file whose full pages go home once, whose partial tail
lives in ring frames (TAIL), whose page tags live in HOME frames, a CKPT
(len, tail page, open-group tags), and a tag page per group written once
when the group completes.

Device: sectors.  A page is SPP sectors, a frame slot 2 sectors, a tag page
G sectors (one entry each).  A sector is atomic (old or new); anything
larger can tear at every sector boundary.

Crash models over the device writes issued since the last completed barrier:
  prefix : in issue order, a prefix lands, the next write torn sector by sector
  offset : page-cache writeback in stored-offset order (ring < tags < data),
           a prefix of the dirty locations lands with their latest value
  any    : every sector independently holds its durable value or any value
           written to it since (the page cache writes back in any order)
  proc   : process crash, the page cache survives: every latest value
Checks on every crash state:
  LOSS       recovered length < the length a completed sync acknowledged
  UNREADABLE a page below the recovered length fails its tag (the design
             poisons the store there: "a failed tag ... below the length
             its own recovery established poisons the store")
  WRONG      recovered bytes differ from the bytes written / len > written
  LATENT     a page in a closed group whose on-disk tag page does not
             hold its tag (readable now only through ring frames that a
             later lap overwrites)
"""
import itertools, sys

PB, SPP, G = 4, 2, 2      # bytes a page, sectors a page, pages a group


class Run:
    def __init__(s, K=2, R=64, ckpt_at_write=False, fix=False, pin=True,
                 pick='newest'):
        s.K, s.R, s.caw, s.fix, s.pin, s.pick = K, R, ckpt_at_write, fix, pin, pick
        s.imgs, s.frames = {}, {}
        s.events = []          # ('w', [(loc,val),...]) | ('b',) | ('ack', len)
        s.n = 0                # bytes written (byte i has value i)
        s.home, s.tail, s.open = 0, [], {}
        s.seq, s.newest_ckpt, s.since = -1, None, 0
        s.synced = None
        s.slot_of = {}         # seq -> slot
        s.lap = 0
        s.synced_home = 0
        s._frame({'k': 'C', 'len': 0, 'home': 0, 'tail': [], 'open': {},
                  'sh': 0})
        s.events.append(('b',))
        s.synced = {'len': 0, 'base': 0, 'last': 0, 'home': 0}
        s.events.append(('ack', 0))

    # ---- writer
    def _frame(s, f):
        s.seq += 1
        f['seq'] = s.seq
        fid = len(s.frames)
        s.frames[fid] = f
        spans = []
        if s.synced:
            spans.append((s.synced['base'], s.synced['last']))
        if s.newest_ckpt is not None:
            spans.append((s.newest_ckpt, s.seq))
        if not hasattr(s, 'occ'):
            s.occ, s.last_slot = {}, -1
        slot = None
        for k in range(1, s.R + 1):
            c = (s.last_slot + k) % s.R
            old = s.occ.get(c)
            if old is None or not s.pin or not any(a <= old <= b for a, b in spans):
                slot = c
                break
        if slot is None:
            s.lap += 1
            raise LapBlocked()
        s.occ[slot] = s.seq
        s.last_slot = slot
        s.events.append(('w', [(('r', slot, i), (fid, i)) for i in range(2)]))
        if f['k'] == 'C':
            s.newest_ckpt = s.seq
            s.since = 0
        else:
            s.since += 1

    def write(s, nbytes):
        new = list(range(s.n, s.n + nbytes))
        s.n += nbytes
        buf = s.tail + new
        start_home = s.home
        home_list, groups = [], []
        p = s.home
        while len(buf) >= PB:
            content = tuple(buf[:PB]); buf = buf[PB:]
            img = len(s.imgs); s.imgs[img] = (p, content)
            s.events.append(('w', [(('d', p, i), (img, i)) for i in range(SPP)]))
            s.open[p] = img
            home_list.append((p, img))
            if p % G == G - 1:
                g = p // G
                ent = [s.open.pop(g * G + i) for i in range(G)]
                s.events.append(('w', [(('t', g, i), ent[i]) for i in range(G)]))
                groups.append(g)
            p += 1
        s.home = p
        s.tail = buf
        tail_new = buf if s.home > start_home else new
        s._frame({'k': 'W', 'len': s.n, 'home': s.home, 'tailb': list(tail_new),
                  'homes': home_list, 'groups': groups})
        if s.caw and s.since >= s.K:
            s._ckpt()

    def _ckpt(s):
        s._frame({'k': 'C', 'len': s.n, 'home': s.home, 'tail': list(s.tail),
                  'open': dict(s.open), 'sh': s.synced['home']})

    def sync(s):
        if s.since >= s.K:
            s._ckpt()
        s.events.append(('b',))
        s.synced = {'len': s.n, 'base': s.newest_ckpt, 'last': s.seq,
                    'home': s.home}
        s.events.append(('ack', s.n))


class LapBlocked(Exception):
    pass


LAPS = [0]


ORDER = {'r': 0, 't': 1, 'd': 2}


def disks(run, model):
    """Yield (disk, acked_len, written_len) for every crash state."""
    dur = {}
    pend = {}          # loc -> list of values since last barrier
    issued = []        # (loc,val) since last barrier, in order
    ack = 0
    written = 0
    ev = run.events
    # map event index -> bytes written so far (for the WRONG check)
    def states_at(cut_writes_partial):
        pass
    nbytes_after = []  # not needed: we bound by run.n
    for i, e in enumerate(ev):
        if e[0] == 'ack':
            ack = e[1]
            continue
        if e[0] == 'b':
            for loc, vals in pend.items():
                dur[loc] = vals[-1]
            pend.clear(); issued.clear()
            continue
        # crash before this write lands, and torn inside it
        sectors = e[1]
        for mask in itertools.product([0, 1], repeat=len(sectors)):
            part = [sv for sv, m in zip(sectors, mask) if m]
            if model == 'prefix':
                d = dict(dur)
                for loc, v in issued + part:
                    d[loc] = v
                yield d, ack
            elif model == 'any':
                p2 = {k: list(v) for k, v in pend.items()}
                for loc, v in part:
                    p2.setdefault(loc, []).append(v)
                locs = list(p2)
                choices = [[dur.get(l)] + p2[l] for l in locs]
                for pick in itertools.product(*choices):
                    d = dict(dur)
                    for l, v in zip(locs, pick):
                        if v is None:
                            d.pop(l, None)
                        else:
                            d[l] = v
                    yield d, ack
        # this write issued whole
        for loc, v in sectors:
            pend.setdefault(loc, []).append(v)
            issued.append((loc, v))
        if model == 'offset':
            locs = sorted(pend, key=lambda l: (ORDER[l[0]],) + l[1:])
            for k in range(len(locs) + 1):
                d = dict(dur)
                for l in locs[:k]:
                    d[l] = pend[l][-1]
                yield d, ack
        if model == 'proc':
            d = dict(dur)
            for loc, vals in pend.items():
                d[loc] = vals[-1]
            yield d, ack


def frame_at(run, d, slot):
    a, b = d.get(('r', slot, 0)), d.get(('r', slot, 1))
    if a is None or b is None or a[0] != b[0] or a[1] != 0 or b[1] != 1:
        return None
    return run.frames[a[0]]


def page_ok(run, d, p, tag):
    if tag is None:
        return False
    for i in range(SPP):
        if d.get(('d', p, i)) != (tag, i):
            return False
    return run.imgs[tag][0] == p


def tagpage(d, p):
    return d.get(('t', p // G, p % G))


def recover(run, d):
    fr = {}
    for slot in range(run.R):
        f = frame_at(run, d, slot)
        if f is not None:
            fr[f['seq']] = f
    ckpts = sorted((q for q, f in fr.items() if f['k'] == 'C'), reverse=True)
    if run.pick == 'oldest':
        ckpts = ckpts[::-1]
    best = None
    for c in ckpts:
        f = fr[c]
        st = {'len': f['len'], 'home': f['home'], 'tail': list(f['tail']),
              'tags': dict(f['open']), 'closed_in_run': []}
        if run.fix:
            ok = True
            for p in range((f['sh'] // G) * G, f['home']):
                t = st['tags'].get(p, tagpage(d, p))
                if not page_ok(run, d, p, t):
                    ok = False; break
            if not ok:
                continue
        q = c + 1
        while q in fr and fr[q]['k'] == 'W':
            w = fr[q]
            if not all(page_ok(run, d, p, t) for p, t in w['homes']):
                break
            for p, t in w['homes']:
                st['tags'][p] = t
            if w['home'] > st['home']:
                st['tail'] = list(w['tailb'])
            else:
                st['tail'] = st['tail'] + list(w['tailb'])
            st['home'], st['len'] = w['home'], w['len']
            st['closed_in_run'] += w['groups']
            q += 1
        best = st
        break   # the design takes the newest CKPT that verifies
    return best


def check(run, d, ack):
    st = recover(run, d)
    if st is None:
        return ('NOCKPT',) if ack > 0 else None
    if st['len'] < ack:
        return ('LOSS', st['len'], ack)
    if st['len'] > run.n:
        return ('WRONG', 'len past written')
    data = []
    for p in range(st['home']):
        t = st['tags'].get(p)
        if t is None:
            t = tagpage(d, p)
        if not page_ok(run, d, p, t):
            return ('UNREADABLE', p, st['len'], ack)
        data += list(run.imgs[t][1])
    data += st['tail']
    if data != list(range(st['len'])):
        return ('WRONG', data, st['len'])
    for p in range(st['home'] - st['home'] % G):
        if tagpage(d, p) != st['tags'].get(p, tagpage(d, p)) or tagpage(d, p) is None:
            if run.fix and p // G in st['closed_in_run']:
                continue          # fix: recovery re-homes these tag pages
            return ('LATENT', p, st['len'])
    return None


def ops_sequences(maxlen, sizes):
    for n in range(1, maxlen + 1):
        for seq in itertools.product(sizes + ['S'], repeat=n):
            yield seq


def drive(cfg, maxlen, sizes, models):
    counts = {m: 0 for m in models}
    first = {}
    runs = 0
    for ops in ops_sequences(maxlen, sizes):
        try:
            r = Run(**cfg)
            for o in ops:
                r.sync() if o == 'S' else r.write(o)
            r.sync()          # a final sync so the last ops are acknowledged
        except LapBlocked:
            LAPS[0] += 1
            continue
        runs += 1
        for m in models:
            for d, ack in disks(r, m):
                counts[m] += 1
                bad = check(r, d, ack)
                if bad and (m, bad[0]) not in first:
                    first[(m, bad[0])] = (ops, bad)
    return runs, counts, first


if __name__ == '__main__':
    which = sys.argv[1] if len(sys.argv) > 1 else 'design'
    sizes = [1, 3, 5]
    cfgs = {
        'design': dict(K=2),
        'design_caw': dict(K=2, ckpt_at_write=True),
        'fix': dict(K=2, fix=True),
        'fix_caw': dict(K=2, ckpt_at_write=True, fix=True),
        'mut_oldest': dict(K=2, pick='oldest', fix=True),
        'ring_pinned': dict(K=2, R=6, ckpt_at_write=True, fix=True),
        'ring8_pinned': dict(K=2, R=8, ckpt_at_write=True, fix=True),
        'ring8_synconly': dict(K=2, R=8, ckpt_at_write=False, fix=True),
        'ring_nopin': dict(K=2, R=6, ckpt_at_write=True, fix=True, pin=False),
    }
    cfg = cfgs[which]
    maxlen = int(sys.argv[2]) if len(sys.argv) > 2 else 4
    models = sys.argv[3].split(',') if len(sys.argv) > 3 else ['prefix', 'offset', 'proc', 'any']
    runs, counts, first = drive(cfg, maxlen, sizes, models)
    print(which, cfg, 'maxlen', maxlen, 'op sequences', runs, 'states', counts, 'laps blocked', LAPS[0])
    for k in sorted(first):
        print(' ', k, first[k])
