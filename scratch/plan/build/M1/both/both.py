# Functions both sides touched: each side's diff from the merge base (-U0), each hunk's
# changed base lines mapped to the base function that encloses them (a `fn` line and its
# body by brace depth, strings and comments skipped), and the hunk header git's rust
# driver names besides. Printed: file, function, the base line it starts at.
import subprocess,re,sys
B=U=H=None; files=[]
if __name__=='__main__': B,U,H=sys.argv[1:4]; files=sys.argv[4:]
FN=re.compile(r'^\s*(pub(\([^)]*\))?\s+)?(default\s+)?(const\s+)?(async\s+)?(unsafe\s+)?(extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)')
IMPL=re.compile(r'^\s*(unsafe\s+)?impl\b(.*?)\{?\s*$')
def show(rev,f):
    r=subprocess.run(['git','show',f'{rev}:{f}'],capture_output=True,text=True); return r.stdout.split('\n')
def spans(lines):
    # (start,end,name) of every fn, with the impl it is in.
    out=[]; stack=[]; depth=0; impl=None; impl_depth=None
    i=0; n=len(lines); in_block=0
    for i,l in enumerate(lines):
        m=FN.match(l)
        if m and not l.strip().startswith('//'):
            stack.append([i,None,(impl or '')+m.group(8),depth])
        mi=IMPL.match(l)
        if mi and depth<=1: impl=re.sub(r'\s+',' ',mi.group(2)).strip()+'::'; impl_depth=depth
        # brace count, skipping strings, chars and comments crudely
        s=l; j=0; ins=False
        while j<len(s):
            c=s[j]
            if in_block:
                if s.startswith('*/',j): in_block-=1; j+=2; continue
                j+=1; continue
            if ins:
                if c=='\\': j+=2; continue
                if c=='"': ins=False
                j+=1; continue
            if s.startswith('//',j): break
            if s.startswith('/*',j): in_block+=1; j+=2; continue
            if c=='"': ins=True
            elif c=="'" and j+2<len(s) and (s[j+2]=="'" or (s[j+1]=='\\')): j+= (3 if s[j+2]=="'" else 4); continue
            elif c=='{': depth+=1
            elif c=='}':
                depth-=1
                while stack and depth<=stack[-1][3]:
                    st=stack.pop(); st[1]=i; out.append(tuple(st[:3]))
                if impl_depth is not None and depth<=impl_depth: impl=None; impl_depth=None
            elif c==';':
                if stack and depth==stack[-1][3] and stack[-1][1] is None: stack.pop()  # a declaration
            j+=1
    return out
def touched(rev,f,sp):
    d=subprocess.run(['git','diff','-U0',B,rev,'--',f],capture_output=True,text=True).stdout
    names=set()
    for m in re.finditer(r'^@@ -(\d+)(?:,(\d+))? \+\d+(?:,\d+)? @@',d,re.M):
        a=int(m.group(1)); c=int(m.group(2)) if m.group(2) is not None else 1
        rng=range(a-1,a-1+max(c,1))  # c==0: an insertion after line a
        for (s,e,nm) in sp:
            if any(s<=x<=e for x in rng): names.add((s,nm))
    return names
if __name__=="__main__":
 for f in files:
     sp=spans(show(B,f))
     u=touched(U,f,sp); h=touched(H,f,sp)
     # innermost only: drop a function that encloses another touched one on both sides
     both=sorted(u&h)
     print(f"== {f}: {len(u)} upstream, {len(h)} branch, {len(both)} both")
     for s,nm in both: print(f"   {nm}  (base line {s+1})")
