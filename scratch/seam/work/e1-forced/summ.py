#!/usr/bin/env python3
# summ.py OUT... : one line per arm per sitting from `bench ab` tables.
import sys, re

KEYS = [
    "read reads_per_s", "read majfaults_per_op", "read refaults_per_op",
    "read read_bytes_per_op", "read p99_us", "read read_us_per_io",
    "count zpage_reads", "count zpage_rawserves", "count zpage_fallbacks",
    "count zpage_builds", "count zpage_build_starts", "count zpage_build_cancelled",
    "count zpage_sel_installs", "count zpage_sel_builds", "count zpage_attached",
    "count zpage_form files_at_end", "count zpage_file_bytes_at_end",
    "count zpage_damaged_at_end", "count zpage_decodes", "count zpage_memo_hits",
    "count zpage_raw_dropped", "count zpage_prebuilt", "count zpage_auto",
    "count zpage_gate_at_end", "count zpage_phase_at_end",
    "count cache_bytes_at_end", "count forms_held_at_end",
    "count partitions", "count pieces", "count merges_landed",
]

for path in sys.argv[1:]:
    lines = [l for l in open(path).read().splitlines() if l.strip()]
    hdr = lines[0].split()
    a, b = hdr[1], hdr[2]
    vals = {}
    for ln in lines[1:]:
        for k in KEYS:
            if ln.startswith(k + " ") or ln.startswith(k + "\t"):
                rest = ln[len(k):].split()
                vals[k] = (float(rest[0]), float(rest[1]))
    print("==", path.rsplit("/", 1)[-1], "A =", a, "B =", b)
    for k in KEYS:
        if k in vals:
            va, vb = vals[k]
            r = (va / vb) if vb else float("nan")
            print(f"  {k:36s} {va:16.4f} {vb:16.4f}  A/B {r:8.3f}")
