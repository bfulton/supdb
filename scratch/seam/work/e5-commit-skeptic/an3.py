import statistics as st
R='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit/raw/'
rows={}
for s,fn in enumerate(['main.txt','main2.txt','main3.txt','main4.txt'],1):
    for line in open(R+fn):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split())
        rows[(s,d['label'],int(d['n']),d['shape'])]=d
def pr(a,b,n,sit,q='med_us'):
    rat=[float(rows[k][q])/float(rows[(k[0],k[1],k[2],b)][q]) for k in rows if k[0]==sit and k[2]==n and k[3]==a and (k[0],k[1],k[2],b) in rows]
    if not rat: return '      -       '
    return f"{st.median(rat):.2f} {sum(x>1 for x in rat)}/{len(rat)}"
for n in [100,4096,13300,133000]:
    print('n=',n)
    for a,b in [('A','B'),('A0','B'),('Aw','B'),('A','D'),('Aw','D'),('A0','D'),('D','B'),('A','Cf'),('A0','Cf'),('Ap','Cp')]:
        print(f"  {a:2}/{b:2}", '  '.join(f"s{s}: {pr(a,b,n,s)}" for s in (1,2,3,4)))
