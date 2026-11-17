"""Re-run one op sequence and print the first violating crash state, object by object."""
import sys, pfm, logrun, dbg
from logrun import Runner

def explain(o, ops, want=None):
    r = Runner(o, 'explain', branch_cap=10**9)
    worlds = []
    orig = r.any_check
    def any_check(W, orc, fid, ops_, trace):
        out = orig(W, orc, fid, ops_, trace)
        for v, d, vd, st in out:
            if vd and (want is None or vd[0] == want) and not worlds:
                worlds.append((W, v, vd, st, list(trace), orc.legal()))
        return out
    r.any_check = any_check
    r.run(tuple(ops))
    if not worlds:
        print('no violation'); return
    W, v, vd, st, trace, legal = worlds[0]
    print('ops', ops, 'crash after', trace)
    print('verdict', vd)
    print('crash state (object: image):'); dbg.show(W, v)
    print('recovered', None if st is None else st.get('content'))
    print('legal lengths', sorted(len(c) for c in legal))

cases = {
 'header': (dict(), [1, 'S', 'X', 1, 'X', 1]),
 'retry_R0': (dict(K=8), [('f', 1, 'frame'), 1, 'S']),
 'rehome_sh': (dict(resume_sync=True), [5, 5, 'P', 'S', 1]),
 'rehome_sh_proc': (dict(K=3), [5, 5, 'X', 'S', 1]),
 'finish_ovr': (dict(retry='R2'), [5, 'S', 'B', ('O', 'P0'), 'E', 'Z', 'S']),
 'finish_scope': (dict(retry='R2'), [1, 'S', 'B', 5, 'E', 'Z', 'S']),
 'trunc_owner': (dict(resume_sync=True, sh_rehome=True, retry='R2'), [5, 'S', 'X', ('T', 3), 'S', 1, 'S', 1, 'S']),
 'trunc_hdr': (dict(resume_sync=True, sh_rehome=True, retry='R2', trunc_base=True), [5, 'S', 'X', ('T', 3), 'S', 1]),
 'stale': (dict(resume_sync=True, sh_rehome=True, accept_stale=True), [1, 1, 'P', 1]),
}
for name in sys.argv[1:]:
    o, ops = cases[name]
    print('=====', name, o)
    explain(o, ops)
