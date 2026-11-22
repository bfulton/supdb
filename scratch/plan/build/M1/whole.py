# For each file both or either side changed: every line a side added is in the merged file
# as many times as the side's file has it more than the base (multiset), and every line a
# side removed is in the merged file no more times than that side's file has it.
import subprocess,sys,collections
B,U,H=sys.argv[1:4]; files=sys.argv[4:]
def show(rev,f):
    r=subprocess.run(['git','show',f'{rev}:{f}'],capture_output=True,text=True)
    return r.stdout.split('\n') if r.returncode==0 else []
for f in files:
    base=collections.Counter(l.strip() for l in show(B,f) if l.strip())
    m=collections.Counter(l.strip() for l in open(f).read().split('\n') if l.strip())
    for side,rev in (('upstream',U),('branch',H)):
        s=collections.Counter(l.strip() for l in show(rev,f) if l.strip())
        lost=[];kept=[]
        for l,c in s.items():
            add=c-base.get(l,0)
            if add>0 and m.get(l,0)-base.get(l,0) < add and m.get(l,0)<c: lost.append(l)
        for l,c in base.items():
            if s.get(l,0)<c and m.get(l,0)>c-(c-s.get(l,0)) and m.get(l,0)>s.get(l,0): kept.append(l)
        print(f"{f} {side}: added-and-missing {len(lost)}, removed-but-present {len(kept)}")
        for l in lost[:40]: print("   MISSING:",l[:150])
        for l in kept[:40]: print("   STILL:",l[:150])
