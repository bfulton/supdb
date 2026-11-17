"""Ring lapping after a resume: the exhaustive families stop at 5-6 ops, so a
resumed Handle never wraps its ring.  Prefixes that leave a synced base and
unsynced CKPTs, a crash (X or P), then long post sequences."""
import sys, time, itertools, ast
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-model')
import pfm
from logrun import Runner
o = ast.literal_eval(sys.argv[1]); cap = int(sys.argv[2]); postlen = int(sys.argv[3])
r = Runner(o, 'lap %s cap=%d postlen=%d' % (o, cap, postlen), branch_cap=cap)
pres = [(5, 'S', 1, 1, 1), (1, 'S', 5, 1, 1), (5, 5, 'S', 1, 1, 1), (3, 'S', 3, 3)]
t0 = time.time(); n = 0
for pre in pres:
    for x in ('X', 'P'):
        for post in itertools.product([1, 5, 'S', 'B', 'E'], repeat=postlen):
            if sum(1 for p in post if isinstance(p, int)) < postlen - 2:
                continue
            r.run(pre + (x,) + post); n += 1
            if time.time() - t0 > 500: break
print('sequences', n, 'secs %.0f' % (time.time() - t0))
if r.sampled: print('sampled at', cap, 'in', r.sampled, 'points')
r.T.report()
