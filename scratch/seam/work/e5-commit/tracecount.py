import sys, re, collections
# counts block requests issued by the probe's own thread, per kind, per commit
f, commits = sys.argv[1], int(sys.argv[2])
c = collections.Counter(); secs = collections.Counter(); metab = collections.Counter()
for l in open(f):
    if 'block_rq_issue' not in l: continue
    m = re.search(r'block_rq_issue: (\S+) (\S+) (\d+) \(\) (\d+) \+ (\d+)', l)
    if not m: continue
    dev, rwbs, nbytes, sec, n = m.groups()
    who = 'probe' if (' e5-' in l or 'iocost' in l) else 'kworker'
    kind = ('flush' if 'F' in rwbs and int(n) == 0 else 'meta' if 'M' in rwbs else 'discard' if 'D' in rwbs else 'data')
    c[(who, kind)] += 1; secs[(who, kind)] += int(n)
    if kind == 'meta': metab[sec] += 1
print(' '.join(f"{w}/{k}={v/commits:.2f}io,{secs[(w,k)]/commits:.1f}sect" for (w,k),v in sorted(c.items())), '| meta blocks:', dict(metab.most_common(4)))
