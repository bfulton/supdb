import os,re,sys
sys.path.insert(0,'/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/wall')
from lex import regions
T='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree'
enc=['src/db/vault.rs','src/db/pager.rs','src/crypt.rs','src/db/dirkey.rs']
form=['src/db/zfile.rs','src/db/zpages.rs','src/db/zselect.rs','src/db/zlz4.rs','src/db/blocks.rs','src/ordindex.rs','src/flatindex.rs','src/blob.rs','src/block.rs','src/index.rs','src/format.rs','src/reserve.rs','src/db.rs','src/bytes.rs']
fnre=re.compile(r'\bfn\s+(\w+)')
def names(files):
    s=set()
    for f in files:
        for code,c,t,e in regions(open(os.path.join(T,f)).read()):
            if not t: s.update(fnre.findall(code))
    return s
E=names(enc); F=names(form)
only=E-F
print('enc-only method names:',len(only),sorted(only))
hits={}
for line in open(sys.argv[1]):
    p=line.split('\t'); hits.setdefault((p[0],int(p[1])),[]).append(p[4])
out=[]
for f in form:
    for ln,(code,c,t,e) in enumerate(regions(open(os.path.join(T,f)).read()),1):
        if t: continue
        for m in re.findall(r'\.\s*(\w+)\s*(?:::<[^>]*>)?\(',code):
            if m in only and (f,ln) not in hits:
                out.append((f,ln,e,m,code.strip()[:120]))
for o in out: print('\t'.join(map(str,o)))
