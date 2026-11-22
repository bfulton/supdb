# For each function both sides touched: is the merged body exactly the base with both
# sides' changes? Held as: the changed lines (+/-, as a multiset) from upstream's body to
# the merged one equal those from the base's to the branch's, and from the branch's to
# the merged one equal those from the base's to upstream's. UNION where both hold, else
# the four diffs are printed to be read.
import subprocess,sys,collections,difflib
sys.path.insert(0,sys.path[0]); from both import spans
B,U,H,f=sys.argv[1:5]; names=sys.argv[5:]
cache={}
def show(rev):
    if rev not in cache:
        cache[rev]=open(f).read().split('\n') if rev=='WT' else subprocess.run(['git','show',f'{rev}:{f}'],capture_output=True,text=True).stdout.split('\n')
    return cache[rev]
def body(rev,name):
    L=show(rev); sp=[s for s in spans(L) if s[2]==name]
    if len(sp)!=1: return None
    s,e,_=sp[0]; return L[s:e+1]
def ch(a,b):
    return collections.Counter(l[0]+l[1:].strip() for l in difflib.unified_diff(a,b,n=0,lineterm='') if l[:1] in '+-' and not l.startswith(('+++','---')) and l[1:].strip())
for n in names:
    b,u,h,m=(body(r,n) for r in (B,U,H,'WT'))
    if None in (b,u,h,m): print(f'{n}: NO MATCH', [x is None for x in (b,u,h,m)]); continue
    ok=ch(u,m)==ch(b,h) and ch(h,m)==ch(b,u)
    print(f'{n}: {"UNION" if ok else "DIFFERS"} (upstream {sum(ch(b,u).values())} lines, branch {sum(ch(b,h).values())}, merged {len(m)} lines)')
    if not ok:
        for tag,(x,y) in (('upstream->merged',(u,m)),('base->branch',(b,h)),('branch->merged',(h,m)),('base->upstream',(b,u))):
            print('   ----',tag); print('\n'.join('   '+l for l in difflib.unified_diff(x,y,n=1,lineterm='')))
