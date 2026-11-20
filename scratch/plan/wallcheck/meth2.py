import os,re,sys
sys.path.insert(0,'/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/wall')
from lex import regions
T='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree'
hits=set()
for line in open('wc.hits.tsv'):
    p=line.split('\t'); hits.add((p[0],int(p[1])))
rx=re.compile(sys.argv[1])
for f in sys.argv[2:]:
    for ln,(code,c,t,e) in enumerate(regions(open(os.path.join(T,f)).read()),1):
        if t: continue
        if rx.search(code):
            print(f,ln,e,'COUNTED' if (f,ln) in hits else 'UNCOUNTED',code.strip()[:110],sep='\t')
