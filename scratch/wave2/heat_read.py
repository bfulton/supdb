import sys, re
from collections import OrderedDict
prev = {}
for line in open(sys.argv[1], errors='replace'):
    m = re.search(r'mixheat (\S+) (\S) \| (.*)$', line)
    if not m:
        continue
    arm, mix, body = m.group(1), m.group(2), m.group(3)
    parts = OrderedDict()
    for tok in body.split():
        f = tok.split(':')
        lo, keys, mb, opens, dec, hits = f[0], int(f[1]), f[2], int(f[3]), int(f[4]), int(f[5])
        parts[lo] = (keys, mb, opens, dec, hits)
    p = prev.get(arm, {})
    rows = []
    for lo, (keys, mb, opens, dec, hits) in parts.items():
        o0, d0, h0 = (p[lo][2], p[lo][3], p[lo][4]) if lo in p else (0, 0, 0)
        rows.append((lo, keys, mb, opens - o0, dec - d0, hits - h0, lo in p))
    prev[arm] = parts
    tot_o = sum(r[3] for r in rows) or 1
    tot_d = sum(r[4] for r in rows) or 1
    short = arm.replace('supdb-cache64tofit', '')
    print(f"{short} {mix}: opens {tot_o} decodes {tot_d} partitions {len(rows)}")
    cum = 0
    for i, r in enumerate(rows):
        cum += r[4]
        flag = '' if r[6] else ' (new)'
        if i < 6 or r[3] * 20 > tot_o:
            print(f"   p{i:<2} lo={r[0] or '-':<16} keys {r[1]:>7} {r[2]:>5} opens {r[3]:>7} ({100*r[3]/tot_o:4.1f}%) dec {r[4]:>7} ({100*r[4]/tot_d:4.1f}%, cum {100*cum/tot_d:4.1f}%) hits {r[5]:>7}{flag}")
