import statistics as st
R='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit/raw/'
rows=[]
for s,fn in enumerate(['main.txt','main2.txt','main3.txt','main4.txt'],1):
    prev=None
    for idx,line in enumerate(open(R+fn)):
        if not line.startswith('label='): continue
        d=dict(kv.split('=',1) for kv in line.split()); d['sit']=s; d['pos']=idx; d['prev']=prev
        prev=d['shape']+'/'+d['n']
        rows.append(d)
for n in ['4096','13300','133000']:
  for sh in ['B','Cf','D','A','A0']:
    print(f'n={n} {sh}: sit label prev sect_per ios_per med_us late_sect st_blocks')
    for r in rows:
        if r['n']==n and r['shape']==sh:
            print('   ',r['sit'],r['label'],r['prev'],r['sect_per'],r['ios_per'],r['med_us'],r['late_sect'],r['st_blocks'])
