import sys,re
sys.path.insert(0,'.')
import third
src=sys.argv[1]; want=sys.argv[2].split(',')
F,reads,why,rel=third.run(src)
for f in F:
    if f['name'] in want and rel(f)=='db.rs':
        lines=f['body'].split('\n')
        hits=[(f['line']+i,l.strip()) for i,l in enumerate(lines) if re.search(r'\bz\b|\bzc\(|ZCursor|PWalk|PartReader|_z\(|pwalk|zfile|\.z\.|z_spare|cache_(seek_exact|key_at|values|scan|cut)|lazy_block|zpages',l)]
        print('==',f['owner'],f['name'],f['line'],len(hits))
        for h in hits[:12]: print('   ',h[0],h[1][:110])
