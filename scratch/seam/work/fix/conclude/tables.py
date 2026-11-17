# tables.py RAW... : the conclusion's markdown tables, paired per rep (same sitting, rep, size)
import sys, statistics as st
rows={}
for fn in sys.argv[1:]:
    for line in open(fn):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split())
        rows[(d['label'],int(d['n']),d['shape'])]=float(d['med_us'])
sizes=sorted(set(k[1] for k in rows)); labels=sorted(set(k[0] for k in rows))
def cell(a,b,n):
    v=[rows[(l,n,a)]/rows[(l,n,b)] for l in labels if (l,n,a) in rows and (l,n,b) in rows]
    k=sum(1 for x in v if x>1)
    return f"{st.median(v):.2f} [{min(v):.2f}-{max(v):.2f}] {k}/{len(v)}"
def absu(s,n):
    v=[rows[(l,n,s)] for l in labels if (l,n,s) in rows]
    return f"{st.median(v):.0f}"
def table(title,cands,refs):
    print(f"\n#### {title}\n")
    print("| size | shape | µs | " + " | ".join(f"/{r if not callable(r) else 'A'}" for r in refs) + " |")
    print("|---|---|---|" + "---|"*len(refs))
    for n in sizes:
        for c in cands:
            cells=[]
            for r in refs:
                rr = r(c) if callable(r) else r
                cells.append(cell(c,rr,n) if rr!=c else "-")
            print(f"| {n} | {c} | {absu(c,n)} | " + " | ".join(cells) + " |")
same_wt=lambda c: ('Aw' if c.endswith('w') else 'A0')
table("The run temp: candidates against the plain run, today's cipher run and the design (A0 for no write-through, Aw for pwrite write-through)",
      ['R0','Rw','T0','Tw','O0','Ow','A0','Aw'], ['B','D',same_wt])
table("The run temp's candidates against the WAL shapes", ['R0','Rw','T0','O0','A0'], ['Cf','Cp','Ds','Dsp'])
table("The WAL: record classes and the paged Log WAL", ['Ds','S','J','A0','Dsp','Sp','Jp','Ap0'], ['Cf','Cp','Ds','Dsp','B'])
