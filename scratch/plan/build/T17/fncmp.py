import sys,collections
def ld(f):
    d=collections.Counter()
    for l in open(f):
        v,loc,t=l.rstrip('\n').split('\t',2); d[(loc.split(':')[0],t)]+=int(v)
    return d
a,b=ld(sys.argv[1]),ld(sys.argv[2])
ks=sorted(set(a)|set(b),key=lambda k:-abs(b[k]-a[k]))
print("total",sum(a.values()),sum(b.values()),sum(b.values())-sum(a.values()))
for k in ks[:int(sys.argv[3])]: print(f"{b[k]-a[k]:>+11,} {a[k]:>11,} {b[k]:>11,} {k[0]} {k[1][:100]}")
