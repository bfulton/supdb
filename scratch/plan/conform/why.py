import sys, re, importlib.util
spec = importlib.util.spec_from_file_location('c', '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/conform/classify.py')
src = open(spec.origin).read().replace('\nmain()\n', '\n')
ns = {}
exec(compile(src, 'classify', 'exec'), ns)
root, rel = sys.argv[1], sys.argv[2]
import os
cpath = os.path.join(root, 'tests/common/mod.rs')
_, cnc, ccode, cfl, _ = ns['analyse'](cpath)
cnames = {f[0] for f in cfl}
cfeat, _ = ns['transitive'](cfl, cnc, {}, cnames, set())
common_feat = {}
for (name, st, b, e, t, line) in cfl:
    common_feat[name] = cfeat[(name, st)]
src, nc, code, fl, _ = ns['analyse'](os.path.join(root, rel))
local = {f[0] for f in fl if not f[4]}
feat, refs = ns['transitive'](fl, nc, common_feat, local, cnames)
FEAT = ns['FEAT']
for f in fl:
    if not f[4]: continue
    g = feat[(f[0], f[1])]
    if not (g['opens'] and not g['enc'] and (g['fs'] or g['names'] or g['mapfile'])): continue
    body = nc[f[2]:f[3]]
    hits = []
    for ln in body.split('\n'):
        if any(FEAT[k].search(ln) for k in ('fs','names','mapfile')):
            hits.append(ln.strip()[:90])
    hs = []
    for h in sorted(refs[(f[0], f[1])]):
        srcs = [feat[(n, s)] for (n, s, *_ ) in fl if n == h and not _[2]] if False else []
        hf = None
        for (n, s, b, e, t, l) in fl:
            if n == h and not t: hf = feat[(n, s)]
        if hf is None and h in common_feat: hf = common_feat[h]
        if hf and (hf['fs'] or hf['names'] or hf['mapfile']): hs.append(h)
    print(f'{rel}:{f[5]} {f[0]}\n   helpers={hs}\n   ' + '\n   '.join(hits[:4]))
