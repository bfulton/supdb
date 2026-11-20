import re, sys, os
src = open('../classify.py').read().replace('\nmain()\n', '\n')
ns = {}; exec(src, ns)
ROOT = sys.argv[1]; pat = re.compile(sys.argv[2])
runs = set()
for ln in open('../classes.tsv'):
    c, loc, name = ln.rstrip('\n').split('\t')
    if c == 'runs': runs.add((loc.split(':')[0], name))
files = sorted({f for f,_ in runs})
cpath = os.path.join(ROOT,'tests/common/mod.rs')
_, cnc, ccode, cfl, _ = ns['analyse'](cpath)
cnames = {f[0] for f in cfl}
cbody = {f[0]: cnc[f[2]:f[3]] for f in cfl}
for rel in files:
    _, nc, code, fl, _ = ns['analyse'](os.path.join(ROOT, rel))
    bodies = {}
    for f in fl:
        if not f[4]: bodies.setdefault(f[0], []).append(nc[f[2]:f[3]])
    allb = dict(bodies); 
    for k,v in cbody.items(): allb.setdefault(k, []).append(v)
    for f in fl:
        if not f[4] or (rel, f[0]) not in runs: continue
        seen, stack, hits = set(), [nc[f[2]:f[3]]], []
        while stack:
            b = stack.pop()
            for m in pat.finditer(b): hits.append(m.group(0))
            for h, bl in allb.items():
                if h in seen: continue
                if re.search(r'(?<![A-Za-z0-9_])'+re.escape(h)+r'(?![A-Za-z0-9_])', b):
                    seen.add(h); stack.extend(bl)
        if hits: print(f'{rel}:{f[5]}\t{f[0]}\t{sorted(set(hits))}')
