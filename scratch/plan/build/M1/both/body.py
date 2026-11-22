# Print, for one function, the diff base->upstream, base->branch, and the merged body's
# diff against each side's body, so the merged body is read once against both.
import subprocess,sys,re,difflib
sys.path.insert(0,sys.path[0]); from both import spans
B,U,H,f,name=sys.argv[1:6]; key=sys.argv[6] if len(sys.argv)>6 else None
def show(rev,f):
    if rev=='WT': return open(f).read().split('\n')
    return subprocess.run(['git','show',f'{rev}:{f}'],capture_output=True,text=True).stdout.split('\n')
def body(rev):
    L=show(rev,f); sp=[s for s in spans(L) if s[2]==name or s[2].endswith('::'+name.split('::')[-1]) and s[2].endswith(name)]
    if key: sp=[s for s in sp if key in L[s[0]] or key in s[2]]
    if len(sp)!=1: return None,[f'<{len(sp)} matches: {[x[2] for x in sp]}>']
    s,e,_=sp[0]; return s,L[s:e+1]
for tag,(a,b) in (('base->upstream',(B,U)),('base->branch',(B,H)),('upstream->merged',(U,'WT')),('branch->merged',(H,'WT'))):
    sa,x=body(a); sb,y=body(b)
    if sa is None or sb is None: print('NO MATCH',tag,x[:1],y[:1])
    d=list(difflib.unified_diff(x,y,a,b,n=1,lineterm=''))
    print(f'---- {tag}: {len(d)} lines'); print('\n'.join(d[:400]))
