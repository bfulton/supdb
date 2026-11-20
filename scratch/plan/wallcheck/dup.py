import sys,re,collections
sys.path.insert(0,'/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/wall')
from lex import regions
T='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree/'
keys=set(l.split('\t')[2] for l in open('/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/wall/couplings.draft.tsv') if not l.startswith('#'))
for f in sys.argv[1:]:
    starts=collections.defaultdict(list); prev=None
    for ln,(c,m,t,e) in enumerate(regions(open(T+f).read()),1):
        if re.search(r'\bfn\s+\w+',c) and e!=prev: starts[(e,t)].append(ln)
        prev=e
    for (e,t),l in starts.items():
        if len(l)>1 and e in keys: print(f,e,'test' if t else 'code',l)
