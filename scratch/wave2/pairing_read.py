#!/usr/bin/env python3
"""A pairing's stderr read by the arm each line names: per mix, the two arms'
own times (`mixkinds`: the gets', the scans' and the commits' milliseconds)
paired rep by rep -- the k-th rep of one arm with the k-th of the other,
never by the lines' parity, since the arms' order swaps every other rep --
with the geometric mean of the ratio B/A over the pairs and how many pairs
B was slower in; and the form's per-mix counters (`mixcounters`: groups
decoded, memo hits and misses, walks served, blocks built) as each arm's
median over its reps, which say what the arms did and whether they did the
same work. Usage: pairing_read.py A B file.err [file.err ...], A and B the
arm names as the lines carry them."""
import re
import statistics
import sys


def geomean(xs):
    return statistics.geometric_mean(xs) if xs else float('nan')


def read(paths):
    kinds = {}      # (arm, mix) -> [ {gets, scans, commits, total} per rep ]
    counts = {}     # (arm, mix) -> [ {dec, scans, groups, hits, misses, built, flt} per rep ]
    kre = re.compile(r'mixkinds (\S+) (\S) size \d+ ops (\d+) gets (\d+) ([\d.]+) scans (\d+) ([\d.]+) commits (\d+) ([\d.]+) total ([\d.]+)')
    cre = re.compile(r'mixcounters (\S+) (\S) flt up (\d+) main (\d+) .*?\| built up (\d+) eng (\d+) rd (\d+) .*?\| z reads (\d+) dec (\d+) scans (\d+) groups (\d+) memo (\d+)/(\d+) pf (\d+)')
    for p in paths:
        for line in open(p, errors='replace'):
            m = kre.search(line)
            if m:
                arm, mix = m.group(1), m.group(2)
                kinds.setdefault((arm, mix), []).append(dict(
                    ops=int(m.group(3)), gets=float(m.group(5)), scans=float(m.group(7)),
                    commits=float(m.group(9)), total=float(m.group(10))))
                continue
            m = cre.search(line)
            if m:
                arm, mix = m.group(1), m.group(2)
                counts.setdefault((arm, mix), []).append(dict(
                    flt=int(m.group(4)), built=int(m.group(6)) + int(m.group(7)),
                    reads=int(m.group(8)), dec=int(m.group(9)), scans=int(m.group(10)),
                    groups=int(m.group(11)), hits=int(m.group(12)), misses=int(m.group(13))))
    return kinds, counts


def main():
    a, b, paths = sys.argv[1], sys.argv[2], sys.argv[3:]
    kinds, counts = read(paths)
    mixes = [m for m in 'CBAFDE' if (a, m) in kinds and (b, m) in kinds]
    print(f'A = {a}\nB = {b}\npairs: the k-th rep of each arm, over {len(paths)} file(s)')
    print(f'{"mix":>3} {"pairs":>5} {"what":>8} {"B/A gmean":>9} {"B slower":>8}   {"A median ms":>11} {"B median ms":>11}')
    for mix in mixes:
        ka, kb = kinds[(a, mix)], kinds[(b, mix)]
        n = min(len(ka), len(kb))
        for what in ('gets', 'scans', 'commits', 'total'):
            xa = [r[what] for r in ka[:n]]
            xb = [r[what] for r in kb[:n]]
            pairs = [(x, y) for x, y in zip(xa, xb) if x > 0 and y > 0]
            if not pairs:
                continue
            ratios = [y / x for x, y in pairs]
            slower = sum(1 for r in ratios if r > 1)
            print(f'{mix:>3} {len(pairs):>5} {what:>8} {geomean(ratios):>9.3f} {slower:>3}/{len(pairs):<4}   {statistics.median(xa):>11.1f} {statistics.median(xb):>11.1f}')
    print()
    print('counters, each arm\'s median over its reps (the mechanism\'s evidence):')
    print(f'{"mix":>3} {"arm":<44} {"built":>7} {"groups":>9} {"hits":>9} {"misses":>9} {"walks":>8} {"reads":>7} {"dec":>7} {"flt":>6}')
    for mix in mixes:
        for arm in (a, b):
            cs = counts.get((arm, mix), [])
            if not cs:
                continue
            med = {k: statistics.median(c[k] for c in cs) for k in cs[0]}
            print(f'{mix:>3} {arm:<44} {med["built"]:>7.0f} {med["groups"]:>9.0f} {med["hits"]:>9.0f} {med["misses"]:>9.0f} {med["scans"]:>8.0f} {med["reads"]:>7.0f} {med["dec"]:>7.0f} {med["flt"]:>6.0f}')
    # Whether the counts differ across reps of one arm: a mechanism's count is
    # deterministic, and a spread says a thread's timing is in it.
    spread = []
    for (arm, mix), cs in counts.items():
        for k in ('groups', 'hits', 'misses', 'built'):
            vals = [c[k] for c in cs]
            if len(vals) > 1 and max(vals) != min(vals):
                spread.append(f'{arm} {mix} {k}: {min(vals)}..{max(vals)}')
    if spread:
        print('\ncounts that vary across reps of one arm:')
        for s in spread:
            print('  ' + s)


if __name__ == '__main__':
    main()
