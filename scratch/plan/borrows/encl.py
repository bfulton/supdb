import re,sys
# usage: encl.py root file:line ...  -> prints file:line  fn
root=sys.argv[1]
pat=re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"C"\s+)?fn\s+([A-Za-z0-9_]+)|^\s*macro_rules!\s+([A-Za-z0-9_]+)')
impl=re.compile(r'^\s*(?:unsafe\s+)?impl\b(.*?)\{?\s*$')
cache={}
for a in sys.argv[2:]:
    f,l=a.rsplit(':',1); l=int(l)
    if f not in cache: cache[f]=open(root+'/'+f).read().split('\n')
    L=cache[f]; fn='?'; im=''
    for i in range(l-1,-1,-1):
        m=pat.match(L[i])
        if m and fn=='?': fn=m.group(1) or (m.group(2)+'!'); continue
        if fn!='?':
            m2=impl.match(L[i])
            if m2 and not L[i].startswith('        '): im=m2.group(1).strip(); break
    print(f"{f}:{l}\t{fn}")
