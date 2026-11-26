"""Where a run's threads spend their time in file-system calls: the strace
of a load and a lag sweep laid to the phase, the thread and the call."""
import re, sys, collections
O = sys.argv[1]
off = float(open(O + '/inv.off').read())
marks = []
for l in open(O + '/inv.err'):
    m = re.match(r'(lagmark|phasemark) (\S+) (\S+) (\S+) ([\d.]+)', l)
    if m: marks.append((float(m.group(5)), f"{m.group(3)} {m.group(4)}"))
marks.sort()
# phases: ordered load, its sync, shuffled load, its sync, settle, then the sweep's bursts and passes
names = []
loads = 0
for t, n in marks:
    if n == 'load start': loads += 1; names.append((t, f'load{loads}'))
    elif n == 'load sync': names.append((t, f'load{loads} drain'))
    elif n == 'load end': names.append((t, f'after load{loads}'))
    elif n.endswith('start'): names.append((t, f'pass {n.split()[0]}'))
    elif n.endswith('end'): names.append((t, f'burst after {n.split()[0]}'))
def phase(t):
    p = 'before'
    for mt, n in names:
        if t >= mt: p = n
    return p
line = re.compile(r'^(\d+)\s+([\d.]+) (\w+)\((.*)\)\s+= (-?\d+|\?).*<([\d.]+)>$')
resumed = re.compile(r'^(\d+)\s+([\d.]+) <\.\.\. (\w+) resumed>(.*)= (-?\d+|\?).*<([\d.]+)>$')
unfinished = re.compile(r'^(\d+)\s+([\d.]+) (\w+)\((.*) <unfinished \.\.\.>$')
comm = {}
pend = {}
tot = collections.defaultdict(float); cnt = collections.Counter(); byts = collections.Counter()
span = collections.defaultdict(lambda: [1e18, 0])
def kind(args):
    m = re.search(r'<([^>]*)>', args)
    path = m.group(1) if m else (re.search(r'"([^"]*)"', args).group(1) if '"' in args else '')
    b = path.rsplit('/', 1)[-1]
    for k in ('wal-', 'spare-', 'manifest', 'par-', 'pcs-', 'seg-', 'ord-', 'zpg-', 'direct-', 'cov-', 'LOCK'):
        if b.startswith(k): return k.rstrip('-')
    if b.endswith('.oidx'): return 'ord'
    if b.endswith('.zpg') or '.zpg' in b: return 'zpg'
    if b.endswith('.tmp'): return 'tmp'
    if '/tmp/ab-rw' in path and '.' not in b: return 'dir'
    return 'other'
def add(tid, t, call, args, ret, dur):
    ph = phase(t - off)
    k = kind(args) if call not in ('munmap', 'mmap') else ''
    key = (ph, tid, call, k)
    tot[key] += dur; cnt[key] += 1
    if call in ('pwrite64', 'write', 'pwritev', 'writev') and ret.lstrip('-').isdigit(): byts[key] += max(0, int(ret))
    s = span[ph]; s[0] = min(s[0], t); s[1] = max(s[1], t + dur)
for l in open(O + '/inv.strace'):
    m = line.match(l)
    if m:
        tid, t, call, args, ret, dur = m.groups(); add(tid, float(t), call, args, ret, float(dur)); continue
    m = unfinished.match(l)
    if m:
        tid, t, call, args = m.groups(); pend[tid] = (float(t), call, args); continue
    m = resumed.match(l)
    if m:
        tid, t, call, rest, ret, dur = m.groups()
        if tid in pend:
            t0, c0, a0 = pend.pop(tid); add(tid, t0, c0, a0 + rest, ret, float(dur))
# thread names: the tid that does the commits' fdatasync of wal is the writer; others by what they touch
role = collections.defaultdict(collections.Counter)
for (ph, tid, call, k), v in cnt.items(): role[tid][(call, k)] += v
def who(tid):
    r = role[tid]
    if r[('fdatasync', 'wal')] + r[('pwrite64', 'wal')] + r[('write', 'wal')] > 50: return 'writer'
    if r[('rename', 'manifest')] + r[('pwrite64', 'manifest')] + r[('fdatasync', 'manifest')] + r[('unlink', 'par')] + r[('unlink', 'pcs')] + r[('link', 'pcs')] > 3: return 'segwork'
    return 'other'
agg = collections.defaultdict(float); aggn = collections.Counter(); aggb = collections.Counter()
for (ph, tid, call, k), v in tot.items():
    key = (ph, who(tid), call, k); agg[key] += v; aggn[key] += cnt[(ph, tid, call, k)]; aggb[key] += byts[(ph, tid, call, k)]
order = []
for _, n in names:
    if n not in order: order.append(n)
for ph in ['before'] + order:
    rows = [(v, k) for k, v in agg.items() if k[0] == ph and v >= 0.02]
    if not rows: continue
    s = span[ph]
    print(f'== {ph}: {s[1]-s[0]:.1f} s')
    by = collections.defaultdict(float)
    for v, k in rows: by[k[1]] += v
    for w in ('writer', 'segwork', 'other'):
        rs = sorted([(v, k) for v, k in rows if k[1] == w], reverse=True)
        if not rs: continue
        print(f'  {w}: {by[w]:.2f} s in these calls')
        for v, k in rs[:9]:
            mb = aggb[k] / 1e6
            print(f'    {k[2]:16s} {k[3]:9s} {v:7.2f} s  x{aggn[k]:<7d}' + (f' {mb:8.0f} MB' if mb else ''))

print()
print('== by what the call is for (seconds; writer / segment work / seal, merge and build threads)')
def cat(call, k):
    if call in ('munmap', 'unlink', 'unlinkat', 'ftruncate', 'fallocate'): return 'frees'
    if call in ('fsync', 'fdatasync') and k in ('wal',): return 'commit flush'
    if call in ('fsync', 'fdatasync') and k == 'direct': return 'commit flush'
    if call in ('fsync',) and k == 'dir': return 'names'
    if call in ('rename', 'renameat', 'renameat2', 'link', 'linkat', 'openat', 'close'): return 'names'
    if call in ('fsync', 'fdatasync'): return 'segment sync'
    if call in ('pwrite64', 'pwritev', 'write', 'writev', 'sync_file_range'): return 'log write' if k in ('wal', 'direct') else 'segment write'
    return 'rest'
tab = collections.defaultdict(float)
for (ph, w, call, k), v in agg.items():
    if w == 'writer' and call == 'unlinkat': continue   # the harness removing a store
    tab[(ph, cat(call, k), w)] += v
cats = ['commit flush', 'log write', 'segment sync', 'segment write', 'frees', 'names', 'rest']
for ph in order:
    s = span[ph]
    if s[1] - s[0] < 1.0: continue
    print(f'{ph} ({s[1]-s[0]:.1f} s)')
    for c in cats:
        row = [tab[(ph, c, w)] for w in ('writer', 'segwork', 'other')]
        if sum(row) >= 0.05: print(f'   {c:14s} {row[0]:6.2f} {row[1]:6.2f} {row[2]:6.2f}')
