#!/usr/bin/env python3
"""faults.py PERF_TXT ERR PHASE: every major fault of each arm's PHASE window
laid to the file its address is a mapping of: by the envelope's name for a
store of files, by its place for a store in one file (pagerback's at)."""
import re, sys, collections, bisect
txt, err, phase = sys.argv[1], sys.argv[2], sys.argv[3]
marks = {}; backs_by_name = {}; backs_one = collections.defaultdict(list)
for l in open(err, errors='replace'):
    m = re.match(r'phasemark (\S+) (\S+) (start|end|sync) ([0-9.]+)', l)
    if m: marks.setdefault((m.group(1), m.group(2), m.group(3)), []).append(float(m.group(4))); continue
    m = re.match(r'pagerback (\S+) (\S+) at (\d+) len (\d+)', l)
    if m:
        label, name, at, ln = m.group(1), m.group(2), int(m.group(3)), int(m.group(4))
        if name == 'store.one': backs_one[(at, (ln + 4095) // 4096 * 4096)].append(label)
        else: backs_by_name[name] = label
maps = collections.defaultdict(list)   # pid -> [(time, start, len, pgoff, file)]
faults = []
mm = re.compile(r'\s*(\S+)\s+(\d+)/(\d+)\s+([0-9.]+): PERF_RECORD_MMAP2 (\d+)/\d+: \[0x([0-9a-f]+)\(0x([0-9a-f]+)\) @ (0x[0-9a-f]+|0) \S+ \S+ \S+\]: \S+ (.*)$')
ff = re.compile(r'\s*(\S+)\s+(\d+)/(\d+)\s+([0-9.]+):\s+([0-9a-f]+)\s*$')
used = collections.Counter()
for l in open(txt, errors='replace'):
    m = mm.match(l)
    if m:
        pid = int(m.group(5)); f = m.group(9).strip(); pg = int(m.group(8), 16); ln = int(m.group(7), 16)
        base = f.rsplit('/', 1)[-1]
        if base == 'store.one':
            ls = backs_one.get((pg, ln))
            if ls:
                i = used[(pg, ln)]; label = ls[min(i, len(ls) - 1)]; used[(pg, ln)] += 1
            else: label = 'store.one?'
        elif base.startswith('x-'): label = backs_by_name.get(base, 'x-?')
        else: label = base
        maps[pid].append((float(m.group(4)), int(m.group(6), 16), ln, pg, label))
        continue
    m = ff.match(l)
    if m: faults.append((float(m.group(4)), int(m.group(2)), int(m.group(3)), m.group(1), int(m.group(5), 16)))
def kind(label):
    k = label.split('-', 1)[0]
    if label.endswith('.zpg') or k == 'zpg': return 'form'
    if k == 'ord' or label.endswith('.oidx'): return 'ord'
    if k in ('par', 'pcs', 'seg'): return k
    return 'other:' + label[:24]
for (arm, ph, edge), ts in sorted(marks.items()):
    if ph != phase or edge != 'start': continue
    for i, t0 in enumerate(ts):
        t1 = marks[(arm, ph, 'end')][i]
        by = collections.Counter(); bycomm = collections.Counter(); files = collections.Counter()
        for (t, pid, tid, comm, addr) in faults:
            if not (t0 <= t <= t1): continue
            best = None
            for (mt, st, ln, pg, label) in maps.get(pid, ()):
                if mt <= t and st <= addr < st + ln: best = (label, (addr - st) // 4096, ln // 4096)
            k = kind(best[0]) if best else 'unmapped'
            by[k] += 1; bycomm[(comm, k)] += 1
            if best: files[(k, best[0])] += 1
        print(f'{arm} {ph} {t1 - t0:.2f}s faults {sum(by.values())}: ' + ' '.join(f'{k} {v}' for k, v in by.most_common()))
        print('   by thread: ' + ' '.join(f'{c}/{k} {v}' for (c, k), v in bycomm.most_common(8)))
        for k in ('form', 'par', 'ord', 'seg', 'pcs'):
            fs = sorted((v for (kk, _), v in files.items() if kk == k), reverse=True)
            if fs: print(f'   {k}: {len(fs)} files, top {fs[:8]}')
