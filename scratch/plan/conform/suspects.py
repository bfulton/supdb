import sys, re, os
ns = {}
src = open('/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/conform/classify.py').read().replace('\nmain()\n', '\n')
exec(compile(src, 'classify', 'exec'), ns)
root = sys.argv[1]
SUS = re.compile(r'\b(advis\w*|resident\w*|madvise|mincore|fadvise|willneed|dontneed|zpages_inject|ZInject|anon\w*|rss\w*|drop_raw|page_cache|cached_pages|budget)\b', re.I)
cpath = os.path.join(root, 'tests/common/mod.rs')
_, cnc, ccode, cfl, _ = ns['analyse'](cpath)
cnames = {f[0] for f in cfl}
cfeat, _ = ns['transitive'](cfl, cnc, {}, cnames, set())
common_feat = {f[0]: cfeat[(f[0], f[1])] for f in cfl}
tot = 0
for rel in sys.argv[2:]:
    s, nc, code, fl, _ = ns['analyse'](os.path.join(root, rel))
    local = {f[0] for f in fl if not f[4]}
    feat, refs = ns['transitive'](fl, nc, common_feat, local, cnames)
    n = 0
    for f in fl:
        if not f[4]: continue
        g = feat[(f[0], f[1])]
        if not g['opens'] or g['enc'] or g['fs'] or g['names'] or g['mapfile']: continue
        body = nc[f[2]:f[3]]
        words = sorted(set(m.group(0).lower() for m in SUS.finditer(body)))
        if words:
            n += 1
            print(f'{rel}:{f[5]}\t{f[0]}\t{",".join(words)}')
    print(f'# {rel} suspects {n}')
