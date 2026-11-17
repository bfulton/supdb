#!/usr/bin/env python3
"""Skeptic check: ADJ's ring slots against the records cases 0 and 4 put there.

design-adjacent sec. 3.3 sizes the ring as 8 slots of 2 pages and says a
record is at most 6 pages (with overrides), so a free run always exists.
Cases 0 and 4 put a record carrying Q's page images in the ring, and Q is
bounded only by q_max = 2M+2 (18 at M = 8).  The fix model runs M = 2,
q_max = 6, 8 slots: a ring of 16 pages for records of at most 7, so it can
never block.  This counts the largest ring record asked for, and the runs
Blocked, at the model's ring and at rings scaled down to the real ratio.

usage: python3 -I ringcap.py <model dir> FAMILY NS [k=v ...]
"""
import sys, itertools
sys.path.insert(0, sys.argv[1])
import cand, adj
from cand import NoResumeRunner
import run as R_

fam, NS = sys.argv[2], int(sys.argv[3])
o = dict(case1_checkP=1, NS=NS)
for a in sys.argv[4:]:
    k, v = a.split('=', 1)
    o[k] = int(v)
cap = o.pop('cap', None)
n = o.pop('n', 5)
maxlen = o.pop('maxlen', 5)

big = [0]
orig = adj.ADJ.ring_alloc


def ring_alloc(s, k, P):
    big[0] = max(big[0], k)
    return orig(s, k, P)


adj.ADJ.ring_alloc = ring_alloc
T = NoResumeRunner(adj.ADJ, adj.recover, o, 'adj %s NS=%d %s' % (fam, NS, o), cap=cap)
if fam == 'dur':
    gen = R_.fam_dur(n, [1, 2, 3, 5])
elif fam == 'infl':
    gen = R_.fam_infl(n, [1, 2, 3, 5])
elif fam == 'buf':
    gen = R_.fam_any(maxlen, [4, 1, 3, 'S', 'B', 'E'])
elif fam == 'any':
    gen = R_.fam_any(maxlen, [1, 2, 3, 5, 4, 'S', 'B', 'E'])
for s_ in gen:
    T.run(s_)
T.T.report()
print('largest ring record asked for: %d pages; ring: %d pages; q_max: %d'
      % (big[0], 2 * NS, 2 * adj.DEFAULT['M'] + 2))
