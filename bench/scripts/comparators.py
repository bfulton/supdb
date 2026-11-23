#!/usr/bin/env python3
"""The suite's comparator cells against the comparators' own tools.

Reads the row `bench/scripts/comparators.sh` wrote (`OUT/quick/*.json`, the
arms `rocksdb-tuned` and `lmdb`) and the tools' outputs (`OUT/tools/
rocksdb-N.txt` from db_bench, `OUT/tools/lmdb-N.txt` from lmdb_bench), and
prints one table: at every rung of the quick ladder, the suite's load, point
read and scan cell beside the tool's, and their ratio. The rule, registered
in docs/engine.md before the first run: the suite's cell is at least MARGIN
of the tool's on every quantity the tool measures, or this exits 1 naming
each quantity under it. A suite cell far below the tool's is a comparator
the suite has configured or driven badly, which is the finding; one far
above it is the tool driven badly, and is printed, not failed.

    python3 scripts/comparators.py OUT
"""
import glob
import json
import re
import statistics
import sys

MARGIN = 0.7
RUNGS = (10_000, 30_000, 100_000, 300_000)
# (suite workload, suite quantity, the row's name for the cell) and how the
# tool's figure maps to the suite's unit.
CELLS = [
    ("load", "ops_per_s", "load keys/s"),
    ("read", "reads_per_s", "point reads/s"),
    ("scan", "entries_per_s", "scan entries/s"),
]


def suite_cells(out):
    rows = glob.glob(f"{out}/quick/*.json")
    if len(rows) != 1:
        sys.exit(f"comparators: expected one row under {out}/quick, found {len(rows)}")
    ms = json.load(open(rows[0]))["measurements"]
    med = {}
    for m in ms:
        if "size" not in m:
            continue
        med[(m["arm"], m["workload"], m["quantity"], m["size"])] = statistics.median(m["samples"])
    return med


def db_bench(path):
    """db_bench's rates: fillseq keys/s, readrandom reads/s, seekrandom
    seeks/s, each from its `N ops/sec` line."""
    got = {}
    for line in open(path):
        m = re.match(r"^(fillseq|readrandom|seekrandom)\s*:\s*[\d.]+ micros/op (\d+) ops/sec", line)
        if m:
            got[m.group(1)] = float(m.group(2))
    missing = {"fillseq", "readrandom", "seekrandom"} - set(got)
    if missing:
        sys.exit(f"comparators: {path} lacks {sorted(missing)}")
    # A seek and 99 nexts is a scan of a hundred entries.
    return {"load": got["fillseq"], "read": got["readrandom"], "scan": got["seekrandom"] * 100.0}


def lmdb_bench(path):
    text = open(path).read()
    m = re.search(r"load_ops_per_s (\S+) read_ops_per_s (\S+) scan_entries_per_s (\S+)", text)
    if not m:
        sys.exit(f"comparators: {path} has no rates: {text.strip()[:200]}")
    return {"load": float(m.group(1)), "read": float(m.group(2)), "scan": float(m.group(3))}


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "comparators-ci"
    med = suite_cells(out)
    lines = ["| tool | rung | quantity | suite | tool | suite/tool |", "|---|---|---|---|---|---|"]
    under = []
    for arm, tool, parse in (("rocksdb-tuned", "db_bench", db_bench), ("lmdb", "lmdb_bench", lmdb_bench)):
        for n in RUNGS:
            tool_cells = parse(f"{out}/tools/{'rocksdb' if arm == 'rocksdb-tuned' else 'lmdb'}-{n}.txt")
            for workload, quantity, label in CELLS:
                s = med.get((arm, workload, quantity, n))
                t = tool_cells[workload]
                if s is None:
                    sys.exit(f"comparators: the row has no {arm} {workload} {quantity} at {n}")
                ratio = s / t if t > 0 else float("inf")
                mark = "" if ratio >= MARGIN else " **under**"
                lines.append(f"| {tool} ({arm}) | {n} | {label} | {s:,.0f} | {t:,.0f} | {ratio:.2f}x{mark} |")
                if ratio < MARGIN:
                    under.append(f"{arm} {label} at {n}: {ratio:.2f}x")
    print(f"The suite's comparator cells against the tools' (the rule: at least {MARGIN:.1f}x on every quantity).\n")
    print("\n".join(lines))
    if under:
        print(f"\nUNDER the margin: {'; '.join(under)}")
        sys.exit(1)
    print(f"\nEvery cell holds the margin of {MARGIN:.1f}x.")


if __name__ == "__main__":
    main()
