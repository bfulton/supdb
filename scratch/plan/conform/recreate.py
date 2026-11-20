import sys, re, os
ns = {}
src = open('/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/conform/classify.py').read().replace('\nmain()\n', '\n')
exec(compile(src, 'classify', 'exec'), ns)
root = sys.argv[1]
for rel in sys.argv[2:]:
    s, nc, code, fl, _ = ns['analyse'](os.path.join(root, rel))
    hits = []
    for f in fl:
        if not f[4]: continue
        body = nc[f[2]:f[3]]
        args = re.findall(r'\bDb::create\(\s*&?([A-Za-z_][A-Za-z0-9_]*)', body)
        dup = [a for a in set(args) if args.count(a) > 1]
        if dup: hits.append((f[5], f[0], dup))
    print(rel, 'tests with Db::create twice on one name:', len(hits))
    for h in hits[:40]: print('  ', rel + ':' + str(h[0]), h[1], h[2])
