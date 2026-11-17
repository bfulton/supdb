import sys
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-model')
import pfm, logrun, dbg
from logrun import Runner
import ast; o = ast.literal_eval(sys.argv[1]); OPS = ast.literal_eval(sys.argv[2])
r = Runner(o, 'one', branch_cap=10**9)
orig_go = r.go
def go(W, h, orc, fid, ops, i, trace):
    print('GO at', i, 'resumed', h.resumed, 'rehome_due', h.rehome_due, 'synced_home', h.synced_home, 'gmac', h.gmac, 'seq', h.seq, 'tags', h.tags)
    return orig_go(W, h, orc, fid, ops, i, trace)
r.go = go
orig_any = r.any_check
def any_check(W, orc, fid, ops_, trace):
    print(' any_check after', trace, 'pending keys', sorted(k[1] for k in W.dev.pend), 'dur', sorted(k[1] for k in W.dev.dur))
    for k in sorted(W.dev.pend, key=repr):
        print('   pend', k[1], [ (t, W.imgs[x]['kind'] if x!='TORN' else x, W.imgs[x].get('seq') if x!='TORN' else None) for t,x in W.dev.pend[k]])
    for k in sorted(W.dev.dur, key=repr):
        x = W.dev.dur[k]
        print('   dur', k[1], x, W.imgs[x]['kind'] if x!='TORN' else x, W.imgs[x].get('seq') if x!='TORN' else None, W.imgs[x].get('body',{}).get('groups') if x!='TORN' else None, W.imgs[x].get('body',{}).get('sh') if x!='TORN' else None)
    return orig_any(W, orc, fid, ops_, trace)
r.any_check = any_check
r.run(tuple(OPS))
print(dict(r.T.states), dict(r.T.bad))
