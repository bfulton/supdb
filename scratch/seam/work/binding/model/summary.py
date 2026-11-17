#!/usr/bin/env python3
"""results.txt -> one line a run: P1 failures by verdict, P2 never-held count,
P2 rollbacks below what was promised durable (allowed by P2, listed)."""
import re, sys
runs = []
cur = None
for line in open(sys.argv[1] if len(sys.argv) > 1 else 'results.txt'):
    if line.startswith('variant='):
        kv = dict(x.split('=', 1) for x in line.split())
        cur = dict(head=kv, c={}, shape={}, ex=[])
        runs.append(cur)
    elif line.startswith('  NEVER-SHAPE'):
        f = line.split()
        cur['shape'][(f[1], f[2], f[3], f[4])] = int(f[5])
    elif line.startswith('  EX'):
        cur['ex'].append(line.strip())
    elif re.match(r'^  P[12] ', line):
        f = line.split()
        cur['c'][(f[0], f[1], f[2])] = int(f[3])
print('%-6s %-6s %-6s %-2s %7s  %-44s %7s %7s  %s' % ('var', 'policy', 'runsem', 'd', 'nodes', 'P1 failures', 'P2never', 'P2lostd', 'never shapes'))
for r in runs:
    h, c = r['head'], r['c']
    p1 = []
    for (pr, mode, v), n in sorted(c.items()):
        if pr != 'P1':
            continue
        bad = (v != 'WHOLE') if mode != 'pl' else v in ('NEVER', 'LOSTDUR', 'REFUSE', 'REFUSE_READ')
        if bad:
            p1.append('%s:%s=%d' % (mode, v, n))
    never = sum(n for (pr, mode, v), n in c.items() if pr == 'P2' and v == 'NEVER')
    lostd = sum(n for (pr, mode, v), n in c.items() if pr == 'P2' and v == 'LOSTDUR')
    shapes = {}
    for (pr, mode, b, d), n in r['shape'].items():
        shapes[(pr, b)] = shapes.get((pr, b), 0) + n
    sh = ' '.join('%s:%s=%d' % (pr, b, n) for (pr, b), n in sorted(shapes.items()))
    print('%-6s %-6s %-6s %-2s %7s  %-44s %7d %7d  %s' % (h['variant'], h['policy'], h['run_sem'], h['depth'], h['nodes'],
          ','.join(p1) or '-', never, lostd, sh))
