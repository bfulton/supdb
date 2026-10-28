import json, glob, statistics, sys
new = sys.argv[1]
def load(p):
    r = json.load(open(p))
    m = r['machine']
    cls = (m['cpu_model'], m['cpus'], m['mem_total_kb'] // (1 << 20))
    d = {}
    for x in r['measurements']:
        d[(x['workload'], x['arm'], x.get('size', 0), x['quantity'])] = statistics.median(x['samples'])
    return r['sha'], r['utc'], cls, d
sha, utc, cls, cur = load(new)
priors = []
for p in sorted(glob.glob('runs/quick/*.json')):
    s, u, c, d = load(p)
    if c == cls:
        priors.append((u, s, d))
priors = priors[-10:]
print('class', cls, 'priors', [s for _, s, _ in priors])
pairs = [('supdb', 'lmdb'), ('supdb-ingest', 'lmdb-nosync')]
lower = {'p99_us', 'p50_us', 'bytes_on_disk_per_byte', 'device_bytes_per_byte'}
rows = []
for (w, a, n, q), v in cur.items():
    for arm, comp in pairs:
        if a != arm: continue
        c = cur.get((w, comp, n, q))
        if not c or not v: continue
        if q in lower: r = c / v
        else: r = v / c
        pr = []
        for _, _, d in priors:
            x, y = d.get((w, arm, n, q)), d.get((w, comp, n, q))
            if x and y:
                pr.append(y / x if q in lower else x / y)
        if len(pr) < 3: continue
        rows.append((w, q, n, arm, r, min(pr), statistics.median(pr), max(pr), pr[-1]))
rows.sort()
below = [x for x in rows if x[4] < x[5]]
print(f'{len(rows)} ratio cells (>1 = supdb ahead); this row below every prior ratio: {len(below)}')
for w, q, n, arm, r, lo, med, hi, last in rows:
    mark = ' <<' if r < lo else (' >>' if r > hi else '')
    print(f'{w:14} {q:22} {n:>7} {arm:13} now {r:6.2f}  priors {lo:5.2f}..{hi:5.2f} med {med:5.2f} last {last:5.2f}{mark}')
