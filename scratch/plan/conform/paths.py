import sys, re, os
ns = {}
src = open('/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/conform/classify.py').read().replace('\nmain()\n', '\n')
exec(compile(src, 'classify', 'exec'), ns)
P = {
 'point': re.compile(r'\.(read_all|get|count|read_vec|values_at)\s*\(|\bread_vec\('),
 'scan': re.compile(r'\.scan\s*\(|\bscan_all\b|\bscan_vec\b'),
 'handle': re.compile(r'\.reader\s*\(|\bReader\b'),
 'snapshot': re.compile(r'Isolation::Snapshot|\bsnapshot\s*\('),
 'merge': re.compile(r'\.(flush|settle|compact)\s*\('),
 'reopen': re.compile(r'\bDb::open\s*\('),
 'zalways': re.compile(r'ZPages::Always'),
 'zauto_inject': re.compile(r'zpages_inject'),
 'threads': re.compile(r'thread::spawn|thread::scope|\.spawn\('),
 'direct': re.compile(r'direct_ingest|ordered|append_sorted|\bload_ordered'),
}
root = sys.argv[1]
cpath = os.path.join(root, 'tests/common/mod.rs')
_, cnc, ccode, cfl, _ = ns['analyse'](cpath)
cnames = {f[0] for f in cfl}
cfeat, _ = ns['transitive'](cfl, cnc, {}, cnames, set())
common_feat = {f[0]: cfeat[(f[0], f[1])] for f in cfl}
grand = {k: 0 for k in P}; gn = 0
for rel in sys.argv[2:]:
    s, nc, code, fl, _ = ns['analyse'](os.path.join(root, rel))
    local = {f[0] for f in fl if not f[4]}
    feat, refs = ns['transitive'](fl, nc, common_feat, local, cnames)
    # path features transitively through local helpers
    pf = {}
    for f in fl:
        body = nc[f[2]:f[3]]
        pf[(f[0], f[1])] = {k: bool(r.search(body)) for k, r in P.items()}
    byname = {}
    for f in fl:
        if not f[4]: byname.setdefault(f[0], []).append((f[0], f[1]))
    ch = True
    while ch:
        ch = False
        for key, d in pf.items():
            for h in refs[key]:
                for k2 in byname.get(h, []):
                    for k in d:
                        if pf[k2][k] and not d[k]: d[k] = True; ch = True
    cnt = {k: 0 for k in P}; n = 0
    for f in fl:
        if not f[4]: continue
        g = feat[(f[0], f[1])]
        if not g['opens'] or g['enc'] or g['fs'] or g['names'] or g['mapfile']: continue
        n += 1
        for k in P:
            if pf[(f[0], f[1])][k]: cnt[k] += 1
    print(rel, n, cnt)
    gn += n
    for k in P: grand[k] += cnt[k]
print('TOTAL', gn, grand)
