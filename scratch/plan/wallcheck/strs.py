import sys,re
sys.path.insert(0,'/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/wall')
from lex import regions
T='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree/'
for f in sys.argv[1:]:
  for ln,(c,m,t,e) in enumerate(regions(open(T+f).read()),1):
    if t: continue
    for s in re.findall(r'b?"(?:[^"\\]|\\.)*"',c):
        if len(s)<14: print(f,ln,e,s,sep='\t')
