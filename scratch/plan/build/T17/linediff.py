import sys,re,collections
def load(f):
    d=collections.Counter(); on=False
    for l in open(f):
        if l.startswith('-- User-annotated source'): on=True; continue
        if not on: continue
        m=re.match(r'^\s*([0-9,]+) \([ 0-9.]+%\)\s(.*)$',l.rstrip('\n'))
        if m and not m.group(2).strip().startswith(("=>","<counts")): d[m.group(2).strip()]+=int(m.group(1).replace(',',''))
    return d
a,b=load(sys.argv[1]),load(sys.argv[2])
keys=set(a)|set(b)
diffs=sorted(keys,key=lambda k:-abs(b[k]-a[k]))
for k in diffs[:int(sys.argv[3])]:
    print(f"{b[k]-a[k]:>+12,} {a[k]:>12,} {b[k]:>12,}  {k[:110]}")
