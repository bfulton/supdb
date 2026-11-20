import re
T='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree/'
src={}
bad=0;n=0
for hf in ['wc.hits.tsv','wf.hits.tsv']:
  for l in open(hf):
    f,ln,reg,sc,tok,c,txt=l.rstrip('\n').split('\t',6)
    if f not in src: src[f]=open(T+f).read().split('\n')
    ln=int(ln); L=src[f]
    if '<item>' in sc or sc.startswith(('struct','const','mod','enum')): continue
    want=sc.split('::')[-1].split('>')[-1]
    # nearest fn header at or above
    for i in range(ln-1,-1,-1):
        m=re.search(r'\bfn\s+(\w+)',L[i].split('//')[0])
        if m: got=m.group(1); break
    n+=1
    if got!=want:
        bad+=1; print(f,ln,sc,'nearest fn:',got,'|',txt[:70])
print(n,'checked',bad,'differ')
