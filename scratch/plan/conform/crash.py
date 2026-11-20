import sys, re, os
ns = {}
src = open('/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/conform/classify.py').read().replace('\nmain()\n', '\n')
exec(compile(src, 'classify', 'exec'), ns)
root, rel = sys.argv[1], sys.argv[2]
targets = set(sys.argv[3].split(','))
cpath = os.path.join(root, 'tests/common/mod.rs')
_, cnc, ccode, cfl, _ = ns['analyse'](cpath)
cnames = {f[0] for f in cfl}
cfeat, _ = ns['transitive'](cfl, cnc, {}, cnames, set())
common_feat = {f[0]: cfeat[(f[0], f[1])] for f in cfl}
s, nc, code, fl, _ = ns['analyse'](os.path.join(root, rel))
local = {f[0] for f in fl if not f[4]}
feat, refs = ns['transitive'](fl, nc, common_feat, local, cnames)
# transitive reach of target helper names
byname = {}
for f in fl:
    if not f[4]: byname.setdefault(f[0], []).append((f[0], f[1]))
reach = {k: set(v) & targets for k, v in refs.items()}
ch = True
while ch:
    ch = False
    for k, r in reach.items():
        for h in list(refs[k]):
            for k2 in byname.get(h, []):
                add = reach[k2] - r
                if add: r |= add; ch = True
n = 0; nenc = 0
for f in fl:
    if f[4] and reach[(f[0], f[1])]:
        n += 1
        if feat[(f[0], f[1])]['enc']: nenc += 1
print(rel, 'tests reaching', sorted(targets), n, 'of which set encryption', nenc)
