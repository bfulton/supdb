import re,sys,os,collections
root=sys.argv[1]
def code_of(text):
    # blank out comments and string literals, preserving newlines
    out=[];i=0;n=len(text)
    while i<n:
        c=text[i]
        if text.startswith('//',i):
            j=text.find('\n',i); j=n if j<0 else j
            out.append(' '*(j-i)); i=j; continue
        if text.startswith('/*',i):
            j=text.find('*/',i)+2; seg=text[i:j]; out.append(re.sub(r'[^\n]',' ',seg)); i=j; continue
        if c=='"' :
            j=i+1
            while j<n and text[j]!='"':
                j+= 2 if text[j]=='\\' else 1
            seg=text[i:j+1]; out.append('"'+re.sub(r'[^\n]',' ',seg[1:-1])+'"'); i=j+1; continue
        if c=="'" and i+2<n and (text[i+2]=="'" or (text[i+1]=='\\')):
            j=text.find("'",i+2 if text[i+1]!='\\' else i+3); out.append(' '*(j-i+1)); i=j+1; continue
        out.append(c); i+=1
    return ''.join(out)
pat=sys.argv[2]
rx=re.compile(pat)
res=collections.Counter(); rows=[]
for d,_,fs in os.walk(os.path.join(root,'src')):
  for f in fs:
    if not f.endswith('.rs'): continue
    p=os.path.join(d,f); rel=os.path.relpath(p,root)
    t=open(p).read(); c=code_of(t)
    lines=c.split('\n')
    test=[False]*len(lines)
    off=[0]
    for l in lines: off.append(off[-1]+len(l)+1)
    for m in re.finditer(r'#\[cfg\(test\)\]',c):
        # find next '{' or ';' after attribute (skip other attrs)
        k=m.end()
        depth=0; start=None
        while k<len(c):
            ch=c[k]
            if ch==';' and depth==0 and start is None: break
            if ch=='{':
                depth+=1; start=start or k
            elif ch=='}':
                depth-=1
                if depth==0: break
            k+=1
        a=c.count('\n',0,m.start()); b=c.count('\n',0,k)
        for q in range(a,b+1): test[q]=True
    for i,l in enumerate(lines):
        for mm in rx.finditer(l):
            rows.append((rel,i+1,'T' if test[i] else '-',mm.group(0)))
for r in rows: print(*r,sep='\t')
