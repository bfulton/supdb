#!/usr/bin/env python3
"""land_read.py ERR...: per rep of a land-pair sitting, for each arm, the
time from the load's end to the controller's buy and install, the form files
on disk at the first sample, the builds ahead and the controller's builds,
and the partitions marked cold (`zahead` lines, where the trace has them)."""
import re
import sys

for path in sys.argv[1:]:
    print(f"== {path}")
    rep = None
    reps = []
    for line in open(path):
        parts = line.split()
        if len(parts) < 2:
            continue
        kind = parts[1]
        if kind == "phasemark" and " load start " in line:
            rep = {"arm": parts[2], "le": None, "buy": None, "inst": None,
                   "disk": None, "ahead": 0, "ctl": 0, "cold": []}
            reps.append(rep)
        elif rep is None:
            continue
        elif kind == "phasemark" and " load end " in line:
            rep["le"] = float(parts[-1])
        elif kind == "zsample" and rep["disk"] is None:
            m = re.search(r"on_disk (\d+)", line)
            rep["disk"] = int(m.group(1)) if m else None
        elif kind == "zselect" and rep["le"] is not None:
            t = float(parts[0])
            if "BuildStart" in line and rep["buy"] is None:
                rep["buy"] = t - rep["le"]
            if "Some(Install)" in line and rep["inst"] is None:
                rep["inst"] = t - rep["le"]
        elif kind == "zbuild":
            if " ahead " in line:
                rep["ahead"] += 1
            elif "wall_ms" in line:
                rep["ctl"] += 1
        elif kind == "zahead" and " cold " in line:
            m = re.search(r"par-0*(\d+)-.* cold resident ([0-9.]+)", line)
            if m:
                rep["cold"].append(f"{m.group(1)}@{float(m.group(2)):.2f}")

    def f(x):
        return "-" if x is None else f"{x:.1f}"

    for r in reps:
        print(f"{r['arm'][-24:]:>24} buy {f(r['buy']):>5} install {f(r['inst']):>5}"
              f" on_disk {r['disk']} ahead {r['ahead']} ctl {r['ctl']}"
              f" cold {' '.join(r['cold'])}")
