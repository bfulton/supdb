import re, sys, os, json
root = sys.argv[1]
pats = json.load(open(sys.argv[2]))  # name -> regex
files = []
for d,_,fs in os.walk(os.path.join(root,'src')):
    for f in fs:
        if f.endswith('.rs'): files.append(os.path.join(d,f))
files.sort()
fnre = re.compile(r'^\s*(pub(\([^)]*\))?\s+)?(const\s+)?(async\s+)?(unsafe\s+)?(extern\s+"C"\s+)?fn\s+(\w+)')
modre = re.compile(r'^\s*(pub(\([^)]*\))?\s+)?mod\s+(\w+)')
cfgre = re.compile(r'^\s*#\[cfg\((.*)\)\]\s*$')
def testonly(c):
    c=c.strip()
    return c=='test' or c.startswith('all(test') or re.match(r'all\(.*\btest\b',c) is not None and not c.startswith('any')
def strip_strings(l):
    return re.sub(r'"(\\.|[^"\\])*"','""',l)
out=[]
for p in files:
    rel=os.path.relpath(p,root)
    L=open(p).read().split('\n')
    # test regions
    tr=[False]*len(L)
    i=0
    while i<len(L):
        m=cfgre.match(L[i])
        if m and testonly(m.group(1)):
            # find item start: next non-attr non-comment line
            j=i+1
            while j<len(L) and (L[j].strip().startswith('#[') or L[j].strip().startswith('//')): j+=1
            if L[j].rstrip().endswith(';'):
                for k in range(i,j+1): tr[k]=True
                i=j+1; continue
            depth=0; started=False; k=j
            while k<len(L):
                s=strip_strings(L[k].split('//')[0]) if '//' in L[k] and not L[k].strip().startswith('//') else (strip_strings(L[k]) if not L[k].strip().startswith('//') else '')
                depth+=s.count('{')-s.count('}')
                if '{' in s: started=True
                if started and depth<=0: break
                if not started and s.rstrip().endswith(';'): break
                k+=1
            for q in range(i,k+1): tr[q]=True
            i=k+1; continue
        i+=1
    # enclosing fn: nearest preceding fn decl
    cur=None; curmod=None
    fnat=[None]*len(L)
    for i,l in enumerate(L):
        m=fnre.match(l)
        if m: cur=m.group(7)+':'+str(i+1)
        fnat[i]=cur
    for i,l in enumerate(L):
        code=l.split('//')[0] if not l.strip().startswith('//') else ''
        # keep strings for /proc detection
        for name,rx in pats.items():
            target = l if name in ('procsys',) else code
            if re.search(rx,target):
                if l.strip().startswith('//') and name!='procsys': continue
                if l.strip().startswith('//'): continue
                out.append((rel,i+1,fnat[i],'T' if tr[i] else '-',name,l.strip()[:150]))
for o in out: print('\t'.join(str(x) for x in o))
