# per (file,line) self cost of one function in a callgrind out (no --dump-instr: positions = line)
import sys,re,collections
out,fn,src_root=sys.argv[1],sys.argv[2],sys.argv[3]
names={};files={};cur=None;curfile=None;fl=None;last=0;skip=False
c=collections.Counter()
def nm(tag,line,table):
    m=re.match(r'^%s=\((\d+)\)(?: (.*))?$'%tag,line)
    if m.group(2): table[m.group(1)]=m.group(2)
    return table[m.group(1)]
for line in open(out):
    line=line.rstrip('\n')
    if line.startswith('fl='): fl=nm('fl',line,files); curfile=fl; continue
    if line.startswith(('fi=','fe=')): curfile=nm(line[:2],line,files); continue
    if line.startswith('fn='): cur=nm('fn',line,names); curfile=fl; continue
    if line.startswith('cfn='): nm('cfn',line,names); continue
    if line.startswith(('cfi=','cfl=')): nm(line[:3],line,files); continue
    if line.startswith('calls='): skip=True; continue
    if line and (line[0] in '+-*' or line[0].isdigit()):
        t=line.split()
        p=t[0]
        if p.startswith('+'): l=last+int(p[1:])
        elif p.startswith('-'): l=last-int(p[1:])
        elif p=='*': l=last
        else: l=int(p)
        last=l
        if skip: skip=False; continue
        if cur==fn and len(t)>1: c[(curfile,l)]+=int(t[1])
srcs={}
def text(f,l):
    p=f if f.startswith('/') else src_root+'/'+f
    if p not in srcs:
        try: srcs[p]=open(p).read().split('\n')
        except: srcs[p]=[]
    s=srcs[p]
    return s[l-1].strip() if 0<l<=len(s) else '?'
for (f,l),v in c.items(): print(f"{v}\t{f.split('/')[-1]}:{l}\t{text(f,l)}")
