import sys,re
sys.path.insert(0,'.')
import third
src=sys.argv[1]
F,_,_,rel=third.run(src)
pat=re.compile(r'\.z\.get\(\)|z_spare\.get\(\)|\bzc\(|ZCursor::|PWalk::Z|PartReader|zfile::ZFile|ZFile::attach|warm_file|\.z\.(take|replace|install|keep|flip)\(')
for f in F:
    if rel(f).startswith('db/zfile'): continue
    hits=sorted(set(pat.findall(f['body'])))
    if hits: print('%-10s %-12s %-28s %6d %s'%(rel(f),f['owner'],f['name'],f['line'],hits))
