import re,sys
def gains(p):
    out={};probe=None
    for l in open(p):
        m=re.match(r'^== ([a-z_]+) \(',l)
        if m: probe=m.group(1)
        m=re.match(r'\s*HOT GAINED: (.*?) ([+-][\d.]+) an op, ([+-][\d,]+) over the run',l)
        if m: out[(probe,m.group(1))]=(float(m.group(2)),int(m.group(3).replace(',','')))
    return out
a=gains(sys.argv[1]); b=gains(sys.argv[2])
print("in the merge's count and not in T17's against 9bfc4362, or larger by >64 over the run:")
for k,v in sorted(b.items()):
    if k not in a: print("  NEW ",k,v)
    elif v[1]-a[k][1]>64: print("  MORE",k,a[k],'->',v)
print("in T17's and gone now:",len([k for k in a if k not in b]))
