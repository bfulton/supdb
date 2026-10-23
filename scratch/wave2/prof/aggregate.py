#!/usr/bin/env python3
"""Aggregate `perf script` output (with callchains) by symbol, cut to time
windows read from the harness's `phasemark` lines and to one thread.

usage: aggregate.py --script perf.txt --marks run.err --arm ARM --phase ycsb-E
                    [--tid TID | --comm bench] [--top 40] [--self-only]

A `phasemark <arm> <phase> start|end <sec.nsec>` pair bounds each window
(CLOCK_MONOTONIC, the clock `perf record -k CLOCK_MONOTONIC` stamps with).
Samples whose time falls in any window of the named arm and phase, and
whose tid is the chosen one (default: the tid with the most samples in the
windows), are aggregated: self cost (the leaf frame) and inclusive cost (a
symbol counted once per sample wherever it appears in the chain).
"""
import argparse, re, sys
from collections import Counter, defaultdict

ap = argparse.ArgumentParser()
ap.add_argument('--script', required=True, nargs='+')
ap.add_argument('--marks', required=True)
ap.add_argument('--arm', required=True)
ap.add_argument('--phase', required=True)
ap.add_argument('--tid', type=int)
ap.add_argument('--top', type=int, default=40)
ap.add_argument('--self-only', action='store_true')
ap.add_argument('--callers', help='print the callers (next frame up) of this symbol')
a = ap.parse_args()

windows = []
start = None
for line in open(a.marks, errors='replace'):
    m = re.search(r'phasemark (\S+) (\S+) (start|end) (\d+\.\d+)', line)
    if not m or m.group(1) != a.arm or m.group(2) != a.phase:
        continue
    t = float(m.group(4))
    if m.group(3) == 'start':
        start = t
    elif start is not None:
        windows.append((start, t)); start = None
if not windows:
    sys.exit(f'no {a.phase} windows for {a.arm} in {a.marks}')
total_win = sum(e - s for s, e in windows)
print(f'{len(windows)} windows of {a.arm} {a.phase}, {total_win*1e3:.0f} ms in all')

def in_window(t):
    for s, e in windows:
        if s <= t <= e:
            return True
    return False

# perf script format (default): "comm tid [cpu] time: period event: \n\t ip sym+off (dso)\n..."
hdr = re.compile(r'^(\S.*?)\s+(\d+)(?:/(\d+))?\s+(?:\[\d+\]\s+)?(\d+\.\d+):\s')
frame = re.compile(r'^\s+([0-9a-f]+)\s+(.*?)(?:\+0x[0-9a-f]+)?\s+\((.*)\)\s*$')

samples = []  # (tid, [frames])
cur = None
for path in a.script:
  with open(path, errors='replace') as f:
      for line in f:
          if not line.strip():
              if cur is not None:
                  samples.append(cur); cur = None
              continue
          m = hdr.match(line)
          if m:
              if cur is not None:
                  samples.append(cur)
              tid = int(m.group(3) or m.group(2))
              t = float(m.group(4))
              cur = (tid, []) if in_window(t) else None
              continue
          if cur is not None:
              m = frame.match(line)
              if m:
                  sym = m.group(2).strip()
                  if sym == '[unknown]':
                      sym = f'[unknown {m.group(3)}]'
                  cur[1].append(sym)
if cur is not None:
    samples.append(cur)

by_tid = Counter(t for t, _ in samples)
print('samples by tid in the windows:', dict(by_tid.most_common(8)))
tid = a.tid if a.tid is not None else (by_tid.most_common(1)[0][0] if by_tid else None)
chosen = [fr for t, fr in samples if t == tid]
n = len(chosen)
print(f'tid {tid}: {n} samples ({n/total_win:.0f}/s)')
if n == 0:
    sys.exit(0)

def short(s):
    s = re.sub(r'^<(.*?) as .*?>::', r'\1::', s)
    s = re.sub(r'::h[0-9a-f]{16}$', '', s)
    return s

selfc = Counter(); incl = Counter(); callers = Counter()
for fr in chosen:
    if fr:
        selfc[short(fr[0])] += 1
    seen = set()
    for i, s in enumerate(fr):
        ss = short(s)
        if ss not in seen:
            seen.add(ss); incl[ss] += 1
        if a.callers and a.callers in ss and i + 1 < len(fr):
            callers[short(fr[i+1])] += 1
print(f'\n== self, top {a.top} ==')
for s, c in selfc.most_common(a.top):
    print(f'{100*c/n:6.2f}%  {c:6d}  {s}')
if not a.self_only:
    print(f'\n== inclusive, top {a.top} ==')
    for s, c in incl.most_common(a.top):
        print(f'{100*c/n:6.2f}%  {c:6d}  {s}')
if a.callers:
    print(f'\n== callers of {a.callers} ==')
    for s, c in callers.most_common(a.top):
        print(f'{100*c/n:6.2f}%  {c:6d}  {s}')
