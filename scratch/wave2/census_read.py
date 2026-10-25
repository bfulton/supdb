#!/usr/bin/env python3
"""The per-mix census lines of a pairing (`mixcensus`, `SUPDB_MIX_CENSUS=1`)
read by arm and mix: the process's anonymous megabytes, the allocator's in
use and free megabytes, and the owners the engine's census names, each as
the median over the reps, with the owners sorted by size. Usage:
census_read.py A B NAME-ab-1.err [NAME-ab-2.err ...]."""
import re
import statistics
import sys
from collections import defaultdict

HEAD = re.compile(r'mixcensus (\S+) (\S) anon_mb (\d+) inuse_mb (\d+) free_mb (\d+) trims (\d+) looks (\d+) look_ms ([\d.]+) \| (.*)$')


def read(paths):
    rows = defaultdict(list)  # (arm, mix) -> [(anon, inuse, free, {owner: mb})]
    for p in paths:
        for line in open(p, errors='replace'):
            m = HEAD.search(line)
            if not m:
                continue
            arm, mix = m.group(1), m.group(2)
            anon, inuse, free = int(m.group(3)), int(m.group(4)), int(m.group(5))
            owners = {}
            toks = m.group(9).split()
            for k, v in zip(toks[0::2], toks[1::2]):
                try:
                    owners[k] = float(v)
                except ValueError:
                    pass
            rows[(arm, mix)].append((anon, inuse, free, owners))
    return rows


def main():
    a, b, paths = sys.argv[1], sys.argv[2], sys.argv[3:]
    rows = read(paths)
    mixes = sorted({mix for (_, mix) in rows})
    for arm in (a, b):
        print(f"== {arm}")
        for mix in mixes:
            reps = rows.get((arm, mix), [])
            if not reps:
                continue
            med = lambda i: statistics.median(r[i] for r in reps)
            owners = defaultdict(list)
            for r in reps:
                for k, v in r[3].items():
                    owners[k].append(v)
            top = sorted(((statistics.median(v), k) for k, v in owners.items() if not k.endswith('_n')), reverse=True)[:8]
            print(f"  {mix} n {len(reps)} anon_mb {med(0):.0f} inuse_mb {med(1):.0f} free_mb {med(2):.0f} | "
                  + ' '.join(f"{k} {v:.1f}" for v, k in top))


if __name__ == '__main__':
    main()
