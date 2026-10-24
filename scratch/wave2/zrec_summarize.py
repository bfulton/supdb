import re, sys, statistics
rows = {}
order = []
for line in open(sys.argv[1]):
    m = re.match(r'round (\d+) (\S+)\s+load\s+([\d.]+)s\s+([\d.]+) MB \|(.*)', line)
    if not m: continue
    arm = m.group(2)
    if arm not in rows: rows[arm] = {'mb': float(m.group(4)), 'mix': {}}; order.append(arm)
    for mm in re.finditer(r'(\S+)\s+([\d.]+) us/op\s+([\d.]+) majflt/op\s+([\d.]+) MB (\d+) miss', m.group(5)):
        name, us, mj, mb, miss = mm.groups()
        d = rows[arm]['mix'].setdefault(name, {'us': [], 'mj': [], 'mb': set(), 'miss': 0})
        d['us'].append(float(us)); d['mj'].append(float(mj)); d['mb'].add(mb); d['miss'] += int(miss)
mixes = list(rows[order[0]]['mix'])
print(f"{'arm':22} {'files':>9} | " + " | ".join(f"{n:>26}" for n in mixes))
for arm in order:
    r = rows[arm]
    cells = []
    for n in mixes:
        d = r['mix'].get(n)
        if not d: cells.append(f"{'-':>26}"); continue
        us = statistics.median(d['us']); mj = statistics.median(d['mj'])
        cells.append(f"{us:7.2f} us {mj:6.3f} flt n={len(d['us'])}")
    print(f"{arm:22} {r['mb']:7.1f}MB | " + " | ".join(cells))
# ratios to lmdb-nordahead (median of per-round ratios)
if 'lmdb-nordahead' in rows:
    print("\nus/op relative to lmdb-nordahead, median of per-round ratios (under 1 = faster than LMDB):")
    for arm in order:
        if arm == 'lmdb-nordahead': continue
        cells = []
        for n in mixes:
            a = rows[arm]['mix'].get(n); l = rows['lmdb-nordahead']['mix'].get(n)
            if not a or not l: cells.append('-'); continue
            rs = [x/y for x, y in zip(a['us'], l['us'])]
            cells.append(f"{n} {statistics.median(rs):5.2f} ({min(rs):.2f}-{max(rs):.2f})")
        print(f"  {arm:22} " + "   ".join(cells))
