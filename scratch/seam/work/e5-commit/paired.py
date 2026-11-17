import sys, statistics as st
from collections import defaultdict
# per-rep ratios of one shape's run median to another's in the same rep and size
pairs = [('A','B'),('Aw','B'),('A0','B'),('Ap','B'),('Ap','Cp'),('A','D'),('D','B'),('Cf','B'),('Cp','Cf')]
for fn in sys.argv[1:]:
    g = {}
    for line in open(fn):
        if not line.startswith('label='): continue
        d = dict(kv.split('=',1) for kv in line.split())
        g[(d['label'], int(d['n']), d['shape'])] = d
    sizes = sorted(set(k[1] for k in g)); labels = sorted(set(k[0] for k in g))
    print(f"== {fn}")
    for n in sizes:
        out = []
        for a, b in pairs:
            for q in ['med_us', 'sect_per']:
                rs = [float(g[(l,n,a)][q]) / float(g[(l,n,b)][q]) for l in labels if (l,n,a) in g and (l,n,b) in g]
                if not rs: continue
                if q == 'med_us':
                    wins = sum(1 for r in rs if r > 1)
                    out.append(f"{a}/{b} t {st.median(rs):.2f} [{min(rs):.2f}-{max(rs):.2f}] >1 in {wins}/{len(rs)}")
                else:
                    out[-1] += f", sect {st.median(rs):.2f}"
        print(f"  n={n}: " + "\n         ".join(out))
