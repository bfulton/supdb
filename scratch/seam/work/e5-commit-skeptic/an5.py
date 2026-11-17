import statistics as st
R='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit/raw/'
rows={}
for s,fn in enumerate(['main.txt','main2.txt','main3.txt','main4.txt'],1):
    for line in open(R+fn):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split())
        rows[(s,d['label'],int(d['n']),d['shape'])]=d
disc=lambda r: int(r['dev_discard_sect'])>=1000
for n in [4096,13300,133000]:
    for a,b in [('A0','B'),('A','B'),('Aw','B'),('A0','D'),('Aw','D'),('A0','Cf')]:
        same=[];diff=[]
        for k,r in rows.items():
            if k[2]!=n or k[3]!=a: continue
            kb=(k[0],k[1],n,b)
            if kb not in rows: continue
            x=float(r['med_us'])/float(rows[kb]['med_us'])
            (same if disc(r)==disc(rows[kb]) else diff).append(x)
        f=lambda v: f"{st.median(v):.2f} [{min(v):.2f}-{max(v):.2f}] {sum(t>1 for t in v)}/{len(v)}" if v else '-'
        print(f"n={n} {a}/{b}: same FS mode {f(same)} | {a} in mode, {b} not {f(diff)}")
