import sys, shutil, os
sys.path.insert(0,'.')
import sites_skeptic as S, third
src=sys.argv[1]
want={6:('db/zfile.rs','file_bytes'),7:('db/zfile.rs','drop'),8:('db/blocks.rs','form_peek'),9:('db/zfile.rs','sample')}
for k,(f,n) in want.items():
    r=S.mutate(src,k)
    F,reads,why,rel=third.run(r)
    hit=[x for x in reads if x[0]==f and x[2]==n]
    print('M%d'%k, 'third.py', 'CAUGHT' if hit else 'missed', hit, [why[h] for h in hit])
    shutil.rmtree(os.path.dirname(r))
