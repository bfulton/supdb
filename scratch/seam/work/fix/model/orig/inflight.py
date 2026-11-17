#!/usr/bin/env python3
"""A sync in flight while the writer keeps writing (db.rs:29030-29045: the
presync's f.sync_data() beside finish(w); db.rs:6218: the helper's
dup.sync_data() beside the writer).  The sync acknowledges the state at its
start.  The design pins the *last completed* sync's span and the newest
CKPT's span; the in-flight sync's span is neither once a CKPT is written
during it.  At the sync's end every location dirty at its start holds its
start value or any later value (a page re-dirtied before writeback goes
down with the later content); locations first dirtied after the start may
or may not have landed.  Enumerate all of that and check the state the sync
acknowledged survives."""
import itertools, sys
import model as M


def scenario(pin_inflight, R=8, K=2, pre=3, mid=2, during=9):
    r = M.Run(K=K, R=R, ckpt_at_write=True, fix=True)
    if pin_inflight:
        orig = r._frame
    for _ in range(pre):
        r.write(1)
    r.sync()
    for _ in range(mid):
        r.write(1)
    # sync begins: CKPT if due, then the barrier's snapshot
    if r.since >= r.K:
        r._ckpt()
    begin = len(r.events)
    acked = {'len': r.n, 'base': r.newest_ckpt, 'last': r.seq, 'home': r.home}
    if pin_inflight:
        # the fix: the in-flight sync's span is pinned from its issue
        prev = r.synced
        r.synced = acked   # what the allocator protects (design: last completed)
        keep = prev
    try:
        for _ in range(during):
            r.write(1)
    except M.LapBlocked:
        return None
    end_events = r.events
    return r, begin, acked


def states(r, begin):
    dur, pend = {}, {}
    for e in r.events[:begin]:
        if e[0] == 'b':
            for l, v in pend.items():
                dur[l] = v[-1]
            pend = {}
        elif e[0] == 'w':
            for l, v in e[1]:
                pend.setdefault(l, []).append(v)
    snap = {l: v[-1] for l, v in pend.items()}
    later = {}
    for e in r.events[begin:]:
        if e[0] == 'w':
            for l, v in e[1]:
                later.setdefault(l, []).append(v)
    locs = sorted(set(snap) | set(later))
    choices = []
    for l in locs:
        if l in snap:
            choices.append([snap[l]] + later.get(l, [])[-1:])
        else:
            choices.append([dur.get(l)] + later.get(l, [])[-1:])
    for pick in itertools.product(*choices):
        d = dict(dur)
        for l, v in zip(locs, pick):
            if v is None:
                d.pop(l, None)
            else:
                d[l] = v
        yield d


for pin in (False, True):
    out = scenario(pin)
    if out is None:
        print('pin_inflight', pin, 'writer blocked (ring full of pinned frames)')
        continue
    r, begin, acked = out
    n = bad = 0
    first = None
    for d in states(r, begin):
        n += 1
        b = M.check(r, d, acked['len'])
        if b and b[0] in ('LOSS', 'UNREADABLE', 'NOCKPT', 'WRONG'):
            bad += 1
            first = first or b
    print('pin_inflight', pin, 'states', n, 'violations', bad, 'first', first)
