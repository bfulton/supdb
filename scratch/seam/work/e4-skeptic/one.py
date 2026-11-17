"""Run one op sequence with options; print every violating state (first few)."""
import sys
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-model')
import pfm, logrun, dbg, ast
from logrun import Runner
def run(o, ops, show=2):
    r = Runner(o, 'one', branch_cap=10**9)
    worlds = []
    orig = r.any_check
    def any_check(W, orc, fid, ops_, trace):
        out = orig(W, orc, fid, ops_, trace)
        for v, d, vd, st in out:
            if vd and len(worlds) < show:
                worlds.append((W, v, vd, st, list(trace), orc.legal()))
        return out
    r.any_check = any_check
    r.run(tuple(ops))
    print('states', dict(r.T.states), 'bad', dict(r.T.bad))
    for W, v, vd, st, trace, legal in worlds:
        print('--- crash after', trace, 'verdict', vd)
        dbg.show(W, v)
        print('recovered', None if st is None else st.get('content'), 'legal', sorted(len(c) for c in legal))
    return r
if __name__ == '__main__':
    o = ast.literal_eval(sys.argv[1]); ops = ast.literal_eval(sys.argv[2])
    run(o, ops, int(sys.argv[3]) if len(sys.argv) > 3 else 2)
