import statistics as st
R='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit/raw/'
rows=[]
for s,fn in enumerate(['main.txt','main2.txt','main3.txt','main4.txt'],1):
    for line in open(R+fn):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split()); d['sit']=s; rows.append(d)
for n in ['4096','13300','133000']:
    for sh in ['A','Aw','A0','B','Cf','D']:
        hi=[float(r['med_us']) for r in rows if r['n']==n and r['shape']==sh and int(r['dev_discard_sect'])>=1000]
        lo=[float(r['med_us']) for r in rows if r['n']==n and r['shape']==sh and int(r['dev_discard_sect'])<1000]
        sp_hi=[float(r['spart_med_us']) for r in rows if r['n']==n and r['shape']==sh and int(r['dev_discard_sect'])>=1000]
        sp_lo=[float(r['spart_med_us']) for r in rows if r['n']==n and r['shape']==sh and int(r['dev_discard_sect'])<1000]
        f=lambda v: f"{st.median(v):6.1f} (n={len(v)})" if v else '   -   '
        print(f"n={n} {sh:3} discard>=1000: med {f(hi)} sync {f(sp_hi)} | <1000: med {f(lo)} sync {f(sp_lo)}")
# B-runs relative to A0 low-discard
