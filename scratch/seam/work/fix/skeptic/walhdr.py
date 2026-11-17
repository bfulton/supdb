#!/usr/bin/env python3
"""Skeptic check: the fix model's decision-(b) WAL against today's open.

stream.py's recover answers a file whose header unit is missing with "no
log" (end None), and the runner restarts the file: cut to nothing, a new
header.  Today's open does not: Wal::resume (db.rs:4220-4235) refuses any
sealed log file of WAL_SEALED_LEN bytes or more whose header is not the
sealed magic and kind ("not this store's log"), replay_frames
(db.rs:4596-4607) the same for a plain one, and the open propagates it
(db.rs:27882-27894, `?`).  This counts the crash views judged by the model
in which the header unit is gone and a later unit of the file holds bytes:
views today's open would refuse rather than restart.

usage: python3 -I walhdr.py <model dir> FAMILY [k=v ...]   (as stream.py)
"""
import sys
sys.path.insert(0, sys.argv[1])
import stream as S

cnt = {'views': 0, 'refused_today': 0}
first = []
orig = S.recover


def recover(W, v, o, fid, sessions):
    cnt['views'] += 1
    u = S.units_of(v, o)
    h = u[0]
    hdr_ok = h is not None and isinstance(h, int) and W.imgs[h]['kind'] == 'hdr'
    if not hdr_ok and any(x is not None for x in u[1:]):
        cnt['refused_today'] += 1
        if not first:
            first.append({k: x for k, x in sorted(v.items(), key=repr)})
    return orig(W, v, o, fid, sessions)


S.recover = recover
sys.argv = ['stream.py'] + sys.argv[2:]
try:
    S.main()
finally:
    print('recoveries', cnt['views'], 'with the header gone and later bytes present (today: refused open):',
          cnt['refused_today'])
    if first:
        print('first such view', first[0])
