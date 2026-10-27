# Per arm, rep and phase: the device's write MB, write ios and flushes over
# the phase's window, the dirty bytes (global, cgroup) at its start, and the
# commits' time from the mixkinds line.
import sys, bisect, collections
err, samples = sys.argv[1], sys.argv[2]
S = [list(map(float, l.split())) for l in open(samples)]
T = [s[0] for s in S]
def at(t):
    i = min(max(bisect.bisect_left(T, t), 0), len(S) - 1)
    return S[i]
marks = []; kinds = collections.defaultdict(list)
for ln in open(err):
    p = ln.split()
    if p and p[0] in ('phasemark', 'mixkinds'):
        p = ['-'] + p
    if len(p) > 5 and p[1] == 'phasemark':
        marks.append((p[2], p[3], p[4], float(p[5])))
    if len(p) > 3 and p[1] == 'mixkinds':
        i = p.index('commits'); g = p.index('gets')
        kinds[(p[2], p[3])].append((float(p[i + 2]), float(p[g + 2])))
open_ = {}; rep = collections.Counter(); rows = collections.defaultdict(list)
for arm, ph, ev, t in marks:
    if ev == 'start':
        open_[(arm, ph)] = t
    elif ev == 'end' and (arm, ph) in open_:
        t0 = open_.pop((arm, ph)); a, b = at(t0), at(t)
        rows[(ph, arm)].append(dict(ms=(t - t0) * 1e3, wmb=(b[4] - a[4]) * 512 / 1e6,
            wios=b[3] - a[3], fl=b[5] - a[5], dirty=a[1] / 1024, wb=a[2] / 1024,
            cgd=a[6] / 1e6, cgc=a[8] / 1e6))
phs = ['load', 'read', 'scan', 'ycsb-C', 'ycsb-B', 'ycsb-A', 'ycsb-D', 'ycsb-E', 'ycsb-F']
for ph in phs:
    for (p2, arm), rs in sorted(rows.items()):
        if p2 != ph: continue
        ks = kinds.get((arm, ph.replace('ycsb-', '')), [])
        cells = []
        for i, r in enumerate(rs):
            c = ' c%.0f' % ks[i][0] if i < len(ks) else ''
            cells.append('%.0fms w%.1fMB/%dio f%d d%.0f/%.0f cg%.0f/%.0f%s' % (r['ms'], r['wmb'], r['wios'], r['fl'], r['dirty'], r['wb'], r['cgd'], r['cgc'], c))
        print('%-7s %-14s %s' % (ph, arm[-14:], ' | '.join(cells)))
