#!/usr/bin/env python3
"""RocksDB on glibc malloc against RocksDB on the prefixed jemalloc the
crate links: two rows of the `rocksdb-tuned` arm over the quick ladder, one
from each build (`rocksdb-alloc.yml`), each quantity's median beside the
other's with the reps' spread, and a mark where one build's median lies
outside the other's spread. A table for a person to read; no verdict.

    python3 scripts/alloc_compare.py GLIBC_RUNS JEMALLOC_RUNS
"""
import glob
import json
import statistics
import sys


def cells(runs):
    rows = glob.glob(f"{runs}/quick/*.json")
    if len(rows) != 1:
        sys.exit(f"alloc_compare: expected one row under {runs}/quick, found {len(rows)}")
    out = {}
    for m in json.load(open(rows[0]))["measurements"]:
        if "size" in m and m["arm"] == "rocksdb-tuned":
            out[(m["workload"], m["quantity"], m["size"])] = m["samples"]
    return out


def main():
    a, b = cells(sys.argv[1]), cells(sys.argv[2])
    print("| workload | quantity | rung | glibc median (min-max) | jemalloc median (min-max) | jemalloc/glibc |")
    print("|---|---|---|---|---|---|")
    for key in sorted(set(a) & set(b)):
        sa, sb = a[key], b[key]
        ma, mb = statistics.median(sa), statistics.median(sb)
        ratio = mb / ma if ma else float("nan")
        apart = mb > max(sa) or mb < min(sa)
        mark = " **apart**" if apart else ""
        w, q, n = key
        print(f"| {w} | {q} | {n} | {ma:,.3g} ({min(sa):,.3g}-{max(sa):,.3g}) | {mb:,.3g} ({min(sb):,.3g}-{max(sb):,.3g}) | {ratio:.2f}x{mark} |")
    print("\nA ratio above one is jemalloc's build ahead on a rate and behind on a cost; **apart** marks a median outside the other build's reps.")


if __name__ == "__main__":
    main()
