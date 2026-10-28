import json, glob, statistics, sys, math, re
sit = sys.argv[1]
procs = []
for d in sorted(glob.glob(f'{sit}/p*-*/'), key=lambda p: int(re.search(r'/p(\d+)-', p).group(1))):
    i, rev = re.search(r'/p(\d+)-(\w+)/', d).groups()
    f = glob.glob(d + 'quick/*.json')
    if not f: continue
    r = json.load(open(f[0]))
    procs.append((int(i), rev, {(x['workload'], x['arm'], x.get('size', 0), x['quantity']): statistics.median(x['samples']) for x in r['measurements']}))
rounds = {}
for i, rev, d in procs:
    rounds.setdefault((i + 1) // 2, {})[rev] = d
rounds = [v for k, v in sorted(rounds.items()) if len(v) == 2]
print(f'{sit}: {len(rounds)} complete rounds')
lower = {'p99_us', 'p50_us', 'bytes_on_disk_per_byte', 'device_bytes_per_byte'}
pairs = [('supdb', 'lmdb'), ('supdb-ingest', 'lmdb-nosync')]
keys = sorted(k for k in rounds[0]['44389dc'] if k[1] in ('supdb', 'supdb-ingest'))
want = sys.argv[2].split(',') if len(sys.argv) > 2 else None
print('cell                                         arm           ratio-to-LMDB old->new (gmean)   new/old of ratio: gmean [per round]   supdb new/old  lmdb new/old')
for k in keys:
    w, a, n, q = k
    if want and w not in want: continue
    comp = dict(pairs)[a]
    ck = (w, comp, n, q)
    rr, ro, rn, sa, sc = [], [], [], [], []
    for R in rounds:
        o, nw = R['5df834c'], R['44389dc']
        if not all(x in o and x in nw and o[x] and nw[x] for x in (k, ck)): break
        f = (lambda v, c: c / v) if q in lower else (lambda v, c: v / c)
        ro.append(f(o[k], o[ck])); rn.append(f(nw[k], nw[ck]))
        rr.append(rn[-1] / ro[-1])
        sa.append((o[k] / nw[k]) if q in lower else (nw[k] / o[k]))
        sc.append((o[ck] / nw[ck]) if q in lower else (nw[ck] / o[ck]))
    else:
        g = lambda xs: math.exp(sum(map(math.log, xs)) / len(xs))
        mark = ' <' if all(x < 1 for x in rr) else (' >' if all(x > 1 for x in rr) else '')
        print(f'{w:13} {q:24} {n:>7} {a:13} {g(ro):6.2f} -> {g(rn):6.2f}   {g(rr):5.2f} [{" ".join(f"{x:.2f}" for x in rr)}]{mark}   {g(sa):5.2f}  {g(sc):5.2f}')
