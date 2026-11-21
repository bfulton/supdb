# From a cost.sh log: each probe's moved functions, gainers and losers listed, over 0.05 an op.
import sys,re
probe=None
rows={}
for l in open(sys.argv[1]):
    m=re.match(r'^== (\S+) \((\w+)\): total ([\d,]+) -> ([\d,]+) \(([-+\d.]+) an op',l)
    if m: probe=m.group(1); rows[probe]={'total':m.group(5),'f':[]}; continue
    m=re.match(r'^\s+([-+][\d.]+) an op\s+([-+][\d,]+)\s+([\d,]+) ->\s+([\d,]+)\s+(.*)$',l)
    if m and probe: rows[probe]['f'].append((float(m.group(1)),m.group(5).strip()))
for p,r in rows.items():
    g=[(v,n) for v,n in r['f'] if v>=0.05]; lo=[(v,n) for v,n in r['f'] if v<=-0.05]
    print(f"## {p}: total {r['total']} an op")
    print("  gained: "+"; ".join(f"{n} {v:+.2f}" for v,n in g))
    print("  lost:   "+"; ".join(f"{n} {v:+.2f}" for v,n in lo))
