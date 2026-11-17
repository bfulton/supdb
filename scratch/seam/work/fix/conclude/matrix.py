# matrix.py RAW... : paired median [min-max] k/n of run medians, every candidate against every reference
import sys, statistics as st
rows={}
for fn in sys.argv[1:]:
    for line in open(fn):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split())
        rows[(d['label'],int(d['n']),d['shape'])]=float(d['med_us'])
sizes=sorted(set(k[1] for k in rows))
labels=sorted(set(k[0] for k in rows))
CAND=['R0','Rw','T0','Tw','O0','Ow','Ds','Dsp','J','Jp','S','Sp']
REF=['B','D','A0','Aw','A','Cf','Cp','Ds','Dsp']
for c in CAND:
    print(f"\n{c}")
    for n in sizes:
        cells=[]
        for r in REF:
            if r==c: continue
            v=[rows[(l,n,c)]/rows[(l,n,r)] for l in labels if (l,n,c) in rows and (l,n,r) in rows]
            if not v: continue
            k=sum(1 for x in v if x>1)
            cells.append(f"{c}/{r} {st.median(v):.2f} [{min(v):.2f}-{max(v):.2f}] {k}/{len(v)}")
        print(f"  n={n:6d}: " + " | ".join(cells))
