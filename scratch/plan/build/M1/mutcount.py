# A mutant held by the count: the mutation applied, the bench's release probe built, the
# probe run under callgrind with cost.sh's toggle, the file put back (checked by its sum)
# and the probe rebuilt clean and run again; the functions named, printed both ways.
import subprocess,hashlib,sys,os
ROOT='/home/user/supdb'; D=os.path.dirname(os.path.abspath(__file__)); K=D+'/keep'
tag,f,a,b,probe=sys.argv[1:6]; fns=sys.argv[6:]
a=a.encode().decode('unicode_escape'); b=b.encode().decode('unicode_escape')
P={'scan':('supdb_bench::engines::supdb_range','lag0_probe','supdb 100000 1'),
   'land':('land_probe::rounds','land_probe','50000 8 4')}
tog,binn,args=P[probe]
env=dict(os.environ, TMPDIR=D.replace('/M1','/tmp/M1'))
def sha(p): return hashlib.sha256(open(p,'rb').read()).hexdigest()
def count(side):
    r=subprocess.run(f'cargo build --release --example {binn}',shell=True,cwd=ROOT+'/bench',env=env,capture_output=True,text=True)
    assert r.returncode==0,r.stderr[-2000:]
    out=f'{D}/ins2/{tag}.{side}.out'
    subprocess.run(['valgrind','--tool=callgrind','--collect-atstart=no',f'--toggle-collect={tog}',f'--callgrind-out-file={out}',f'{ROOT}/bench/target/release/examples/{binn}']+args.split(),env=env,capture_output=True,check=True)
    ann=subprocess.run(['callgrind_annotate','--auto=no','--inclusive=no','--threshold=100',out],capture_output=True,text=True).stdout
    os.remove(out)
    tot=[l for l in ann.split('\n') if 'PROGRAM TOTALS' in l][0].split()[0]
    got={}
    for fn in fns:
        got[fn]=sum(int(l.split()[0].replace(',','')) for l in ann.split('\n') if fn in l and l.strip()[:1].isdigit())
    return tot,got
p=os.path.join(ROOT,f); orig=open(p).read(); assert orig.count(a)==1,(tag,a)
open(p,'w').write(orig.replace(a,b))
try: mut=count('mutant')
finally:
    open(p,'w').write(orig); assert sha(p)==sha(os.path.join(K,f)),'restore'
clean=count('clean')
line=f"{tag}\tprobe {probe}\ttotal {clean[0]} -> {mut[0]} (clean -> mutant)\t"+'; '.join(f"{fn} {clean[1][fn]:,} -> {mut[1][fn]:,}" for fn in fns)
print(line)
open(D+'/mutants-r1.counts','a').write(line+'\n')
