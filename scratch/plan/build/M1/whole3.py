# For each file: every non-blank line (stripped) is in the merged file as many times as the
# base has it plus what each side added or removed (m == b + (u-b) + (h-b), floored at 0).
# A line off that count is printed with the four counts: each is a line the resolution
# wrote or dropped by hand (or a line both sides added alike, counted twice by the sum).
import subprocess,sys,collections
B,U,H=sys.argv[1:4]; files=sys.argv[4:]
def show(rev,f):
    r=subprocess.run(['git','show',f'{rev}:{f}'],capture_output=True,text=True,errors='replace')
    return r.stdout.split('\n') if r.returncode==0 else []
C=lambda L:collections.Counter(l.strip() for l in L if l.strip())
for f in files:
    b,u,h=(C(show(r,f)) for r in (B,U,H)); m=C(open(f,errors='replace').read().split('\n'))
    off=[]
    for l in set(b)|set(u)|set(h)|set(m):
        want=max(0,b[l]+(u[l]-b[l])+(h[l]-b[l]))
        if m[l]!=want: off.append((l,b[l],u[l],h[l],m[l],want))
    print(f"{f}: {len(off)} lines off the sum")
    for l,*n in sorted(off,key=lambda x:x[0]): print(f"   base {n[0]} up {n[1]} branch {n[2]} merged {n[3]} (sum {n[4]}): {l[:140]}")
