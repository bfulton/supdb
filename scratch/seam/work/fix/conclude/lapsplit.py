import sys, os, statistics as st
D=sys.argv[1]
def load(s,shape,n,r):
    p=os.path.join(D,f"s{s}-{shape}-{n}-r{r}.lat")
    if not os.path.exists(p): return None
    return [int(l.split()[0]) for l in open(p) if l.strip()]
def med(x): return st.median(x)
for n,cut in ((4096,240),(13300,77),(133000,500)):
    for a,b in (("R0","B"),("R0","Ds"),("R0","Dsp"),("R0","Cp"),("R0","A0"),("Rw","B"),("A0","B"),("Ds","B"),("Dsp","Cp"),("O0","A0"),("O0","B")):
        early=[];late=[]
        for s in (1,2,3):
            for r in (1,2):
                x=load(s,a,n,r); y=load(s,b,n,r)
                if not x or not y: continue
                early.append(med(x[:cut])/med(y[:cut])); late.append(med(x[cut:])/med(y[cut:]))
        if early:
            print(f"n={n:6d} {a}/{b:4s} commits<{cut}: {med(early):.2f} [{min(early):.2f}-{max(early):.2f}]  after: {med(late):.2f} [{min(late):.2f}-{max(late):.2f}]  pairs={len(early)}")
    for sh in ("R0","B","Ds","Dsp","Cp","A0"):
        e=[];l=[]
        for s in (1,2,3):
            for r in (1,2):
                x=load(s,sh,n,r)
                if x: e.append(med(x[:cut])/1000); l.append(med(x[cut:])/1000)
        if e: print(f"   {sh:4s} abs us: before {med(e):.0f}  after {med(l):.0f}")
