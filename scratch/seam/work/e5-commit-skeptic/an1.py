import sys, statistics as st
R='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit/raw/'
rows=[]
for s,fn in enumerate(['main.txt','main2.txt','main3.txt','main4.txt'],1):
    for idx,line in enumerate(open(R+fn)):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split()); d['sit']=s; d['pos']=idx
        rows.append(d)
def med(v): return st.median(v) if v else float('nan')
sizes=[100,4096,13300,133000]
shapes=['A','Aw','A0','Ap','B','Cf','Cp','D']
print("per-sitting median of run med_us")
for n in sizes:
    print('n=',n)
    for s in shapes:
        out=[]
        for sit in (1,2,3,4):
            v=[float(r['med_us']) for r in rows if int(r['n'])==n and r['shape']==s and r['sit']==sit]
            out.append(f"{med(v):7.1f}({len(v)})" if v else '   -   ')
        print(f"  {s:3}", ' '.join(out))
# paired Aw/D, A0/D within sittings 2-4
def paired(a,b,n,sits=(1,2,3,4),q='med_us'):
    g={(r['sit'],r['label'],int(r['n']),r['shape']):r for r in rows}
    rat=[];dif=[]
    for (sit,l,nn,s),r in g.items():
        if nn==n and s==a and sit in sits and (sit,l,nn,b) in g:
            x=float(r[q]);y=float(g[(sit,l,nn,b)][q]); rat.append(x/y); dif.append(x-y)
    if not rat: return '-'
    return f"{med(rat):.2f} [{min(rat):.2f}-{max(rat):.2f}] >1 {sum(x>1 for x in rat)}/{len(rat)} diff {med(dif):+.0f} [{min(dif):+.0f},{max(dif):+.0f}]"
print()
for n in sizes:
    print('n=',n)
    for a,b in [('A','B'),('Aw','B'),('A0','B'),('A','D'),('Aw','D'),('A0','D'),('D','B'),('A','Cf'),('Aw','Cf'),('A0','Cf'),('Ap','Cp'),('A','A0'),('Aw','A0')]:
        print(f"  {a}/{b} all: {paired(a,b,n)} | sit2-4: {paired(a,b,n,(2,3,4))} | sit1: {paired(a,b,n,(1,))}")
