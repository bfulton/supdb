import sys, statistics as st, re, glob, os
W = sys.argv[1]
def parse(p):
    L = [l for l in open(p).read().splitlines() if l.strip()]
    a, b = L[0].split()[1:3]
    d = {}
    for l in L[1:]:
        m = re.match(r'^(.*?)\s{2,}(-?[0-9.NaN]+)\s+(-?[0-9.NaN]+)\s', l)
        if m: d[m.group(1).strip()] = (float(m.group(2)), float(m.group(3)))
    return a, b, d
def col(d, a, b, arm, k):
    return d[k][0] if arm == a else d[k][1]
for label, files, F, R in [
    ("640MiB forced/raw", sorted(glob.glob(W+"/pairs/supdb-nocachezpages-vs-supdb-nozpages-5000000-[12].out"))+sorted(glob.glob(W+"/pairs/r[23]/*-[12].out")), "supdb-nocachezpages", "supdb-nozpages"),
    ("384MiB forced/raw", sorted(glob.glob(W+"/pairs/b384/*-[12].out")), "supdb-nocachezpages", "supdb-nozpages"),
    ("448MiB forced/raw", sorted(glob.glob(W+"/pairs/b448/*-[12].out")), "supdb-nocachezpages", "supdb-nozpages"),
    ("640MiB forced/auto", sorted(glob.glob(W+"/pairs/p2/*-[12].out")), "supdb-nocachezpages", "supdb-nocache"),
]:
    rows = []
    print("==", label)
    for f in files:
        a, b, d = parse(f)
        rf = col(d,a,b,F,"read reads_per_s"); rr = col(d,a,b,R,"read reads_per_s")
        ff = col(d,a,b,F,"read majfaults_per_op"); fr = col(d,a,b,R,"read majfaults_per_op")
        pf = col(d,a,b,F,"read p99_us"); pr = col(d,a,b,R,"read p99_us")
        zr = col(d,a,b,R,"count zpage_reads")
        rows.append((rf/rr, rf, rr, ff, fr, pf, pr))
        print(f"  {os.path.relpath(f, W):70s} first={a:22s} ratio {rf/rr:6.3f}  F {rf:9.0f}/s {ff:.4f} flt p99 {pf:6.1f}  R {rr:9.0f}/s {fr:.4f} flt p99 {pr:6.1f}  R.zpage_reads {zr:.0f}")
    for i, name in enumerate(["ratio","F rate","R rate","F flt","R flt","F p99","R p99"]):
        v = [r[i] for r in rows]
        print(f"   {name:7s} median {st.median(v):10.4f}  min {min(v):10.4f}  max {max(v):10.4f}  n {len(v)}")
