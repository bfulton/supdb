import sys,re,collections
# sum self Ir by function (all files) from callgrind_annotate output
def load(p):
    d=collections.Counter()
    for l in open(p):
        m=re.match(r'\s*([\d,]+) \([ \d.]+%\)\s+(\S+?):(.*?)(?: \[.*\])?$',l)
        if m:
            d[m.group(3).strip()]+=int(m.group(1).replace(',',''))
    return d
a=load(sys.argv[1]); b=load(sys.argv[2])
rows=[(b[k]-a[k],k) for k in set(a)|set(b) if b[k]!=a[k]]
rows.sort()
for dlt,k in rows:
    if abs(dlt)>=int(sys.argv[3] if len(sys.argv)>3 else 1000): print(f"{dlt:+12d} {a[k]:12d} {b[k]:12d} {k[:150]}")
