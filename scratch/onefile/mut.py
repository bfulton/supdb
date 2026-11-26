#!/usr/bin/env python3
"""Run the mutants of a list against the one-file tests.

  mut.py LIST [ID ...]

A line of LIST:  id | file | text | replacement | what it breaks
with \\n for a newline and \\| for a bar in the two texts, and an optional
'#k' after the id for the k-th occurrence (1-based) where the text stands
more than once. A mutant is killed where a test fails or the run passes
seven minutes (a hang is a death the test did not name: it is said so);
it survives where every test passes. The source is put back as a change,
at a time after the mutant's build, so the build after it is of the tree.
"""
import os, subprocess, sys, time, glob, shutil
ROOT='/home/user/supdb'
S='/tmp/claude-0/-home-user-supdb/28b9c9ae-2263-5e29-a6fc-74dd8e38e2a1/scratchpad'
env=dict(os.environ); env['TMPDIR']=S+'/tmp'
for k in [k for k in env if k.startswith('SUPDB_')]: env.pop(k,None)
def un(t): return t.replace('\\n','\n').replace('\\|','|')
def split(l):
    out=[];cur='';i=0
    while i<len(l):
        if l.startswith(' | ',i): out.append(cur);cur='';i+=3
        else: cur+=l[i];i+=1
    out.append(cur); return out
want=set(sys.argv[2:])
res=[]
TESTS=[['cargo','test','--profile','checked','--lib','onefile'],
       ['cargo','test','--profile','checked','--test','onefile']]
for l in open(sys.argv[1]):
    if not l.strip() or l.startswith('#'): continue
    p=split(l.rstrip('\n').replace(' | | ',' | <EMPTY> | '))
    mid,f,old,new,what=p[0].strip(),p[1].strip(),un(p[2]),un(p[3]).replace('<EMPTY>',''),p[4].strip() if len(p)>4 else ''
    k=1
    if '#' in mid: mid,k=mid.split('#'); k=int(k)
    if want and mid not in want: continue
    path=os.path.join(ROOT,f); src=open(path).read()
    n=src.count(old)
    if n<k:
        res.append((mid,'NOTEXT',what)); print(mid,'NOTEXT',n,flush=True); continue
    at=-1
    for _ in range(k): at=src.index(old,at+1)
    open(path,'w').write(src[:at]+new+src[at+len(old):])
    verdict='survived'; by=''
    try:
        for t in TESTS:
            try:
                r=subprocess.run(t,cwd=ROOT,env=env,capture_output=True,text=True,timeout=420)
            except subprocess.TimeoutExpired:
                subprocess.run(['pkill','-f','target/checked/build/supdb'])
                verdict='killed'; by='a hang (seven minutes)'; break
            out=r.stdout+r.stderr
            if 'error[' in out or 'error: could not compile' in out:
                verdict='NOCOMPILE'; by=[x for x in out.split('\n') if x.startswith('error')][:1]; break
            if r.returncode!=0:
                verdict='killed'
                by=', '.join(x.split()[1] for x in out.split('\n') if x.startswith('test ') and x.rstrip().endswith('FAILED'))[:200]
                break
    finally:
        open(path,'w').write(src)
        now=time.time()+1; os.utime(path,(now,now))
        for d in glob.glob(S+'/tmp/supdb-*'): shutil.rmtree(d,ignore_errors=True)
    res.append((mid,verdict,what)); print(mid,verdict,'|',what,'|',by,flush=True)
print('---'); print('survived:',[m for m,v,_ in res if v=='survived']); print('other:',[(m,v) for m,v,_ in res if v not in('survived','killed')])
