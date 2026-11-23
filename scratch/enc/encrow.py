#!/usr/bin/env python3
"""Read an encrypted-roster row: each encrypted comparator against the engine
under a cipher, cell by cell. usage: encrow.py ROW.json [--all]"""
import json, sys, statistics as st
row = json.load(open(sys.argv[1]))
show_all = '--all' in sys.argv
LOWER = ('p99_us', 'bytes_on_disk_per_byte', 'device_bytes_per_byte', 'open_ms',
         'first_read_us', 'unrecovered_batches')
def lower_better(q): return q in LOWER or q.startswith('weighted_ms')
cells = {}
for m in row['measurements']:
    if m.get('size') is None or not m.get('arm'):
        continue
    cells[(m['size'], m['workload'], m['quantity'], m['arm'])] = m['samples']
pairs = [('supdb-cipher', ['lmdb3-enc', 'rocksdb-tuned-enc', 'supdb']),
         ('supdb-ingestcipher', ['lmdb3-nosync-enc', 'rocksdb-tuned-nosync-enc', 'supdb-ingest'])]
sizes = sorted({k[0] for k in cells})
skip = ('mem-floor', 'page-floor', 'scan-floor', 'wal-floor')
print(f"row {row['utc']} {row['sha'][:7]} scale {row['scale']} sizes {sizes}")
for ours, theirs in pairs:
    for them in theirs:
        won = lost = level = 0
        lines = []
        for size in sizes:
            for (sz, w, q, arm) in sorted(cells):
                if sz != size or arm != ours or w in skip: continue
                a = cells[(sz, w, q, ours)]; b = cells.get((sz, w, q, them))
                if not b or not a: continue
                ma, mb = st.median(a), st.median(b)
                if ma == 0 and mb == 0:
                    level += 1; continue
                if lower_better(q):
                    ratio = (mb / ma) if ma else float('inf')
                    ahead = max(a) < min(b); behind = min(a) > max(b)
                else:
                    ratio = (ma / mb) if mb else float('inf')
                    ahead = min(a) > max(b); behind = max(a) < min(b)
                verdict = 'AHEAD' if ahead else 'BEHIND' if behind else 'level'
                won += ahead; lost += behind; level += (not ahead and not behind)
                if show_all or behind or (not ahead and ratio < 1.0):
                    lines.append(f"  {size:>8} {w:>16} {q:<26} {ratio:6.2f}x {verdict}")
        print(f"\n{ours} against {them}: ahead in {won} cells, behind in {lost}, unresolved {level} "
              f"(a cell is resolved when every rep lies on one side of every rep of the other)")
        for l in lines: print(l)
