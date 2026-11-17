import sys,re
sys.path.insert(0,'.')
import third
src=sys.argv[1]; names=sys.argv[2].split(',')
F,_,_,rel=third.run(src)
pat=re.compile(r'\.z\.get\(\)|z_spare\.get\(\)|\bzc\(|ZCursor::|PWalk|PartReader')
for f in F:
    if rel(f)!='db.rs' or f['name'] not in names: continue
    L=f['body'].split('\n')
    for i,l in enumerate(L):
        if pat.search(l):
            ctx=' | '.join(x.strip() for x in L[i:i+4])
            print(f['name'],f['line']+i,ctx[:230])
