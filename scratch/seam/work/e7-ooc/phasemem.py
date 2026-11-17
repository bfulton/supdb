#!/usr/bin/env python3
# Joins the sampler's lines (samples-*.txt) to the harness's phasemark lines
# (pairs/*.err, CLOCK_MONOTONIC) and prints, per sitting, arm and phase, the
# median and max of the group's rss/cache/shmem, the page cache that is not
# shmem (cache - shmem), the child's RssAnon/RssShmem, and the group's major
# faults and device read sectors over the phase.
# usage: phasemem.py SAMPLES ERR1 [ERR2 ...]
import re, sys, statistics as st

samples = []
for line in open(sys.argv[1]):
    f = line.split()
    if len(f) < 10:
        continue
    d = {"t": float(f[0]), "arm": f[1]}
    for i in range(2, len(f) - 1, 2):
        try:
            d[f[i]] = float(f[i + 1])
        except ValueError:
            pass
    samples.append(d)

for err in sys.argv[2:]:
    marks = []
    for line in open(err):
        m = re.match(r"phasemark (\S+) (\S+) (\S+) ([0-9.]+)", line)
        if m:
            marks.append((m.group(1), m.group(2), m.group(3), float(m.group(4))))
    # phases per arm in order: load(ordered), read, load(shuffled)
    phases = []
    open_ = {}
    count = {}
    for arm, ph, edge, t in marks:
        if edge == "start":
            open_[(arm, ph)] = t
        elif edge == "end" and (arm, ph) in open_:
            n = count.get((arm, ph), 0)
            count[(arm, ph)] = n + 1
            name = ph if not (ph == "load" and n == 1) else "load-shuffled"
            phases.append((arm, name, open_.pop((arm, ph)), t))
    print(f"== {err}")
    for arm, name, t0, t1 in phases:
        ss = [s for s in samples if t0 <= s["t"] <= t1 and s["arm"].rsplit("-", 2)[0] == arm]
        if not ss:
            print(f"{arm:16s} {name:14s} {t1-t0:7.1f}s  no samples")
            continue
        def med(k):
            return st.median(s.get(k, 0) for s in ss)
        def mx(k):
            return max(s.get(k, 0) for s in ss)
        pc = [s.get("cache_mb", 0) - s.get("shmem_mb", 0) for s in ss]
        mf = ss[-1].get("pgmajfault", 0) - ss[0].get("pgmajfault", 0)
        rs = ss[-1].get("vda_rsect", 0) - ss[0].get("vda_rsect", 0)
        span = ss[-1]["t"] - ss[0]["t"]
        print(
            f"{arm:16s} {name:14s} {t1-t0:7.1f}s n={len(ss):3d} "
            f"usage med {med('usage_mb'):4.0f} max {mx('usage_mb'):4.0f} | "
            f"grp rss med {med('rss_mb'):4.0f} max {mx('rss_mb'):4.0f} | "
            f"shmem med {med('shmem_mb'):4.0f} max {mx('shmem_mb'):4.0f} | "
            f"pagecache(non-shmem) med {st.median(pc):4.0f} max {max(pc):4.0f} | "
            f"RssAnon med {med('RssAnon_mb'):4.0f} max {mx('RssAnon_mb'):4.0f} | "
            f"majflt/s {mf/span if span>0 else 0:8.0f} rMB/s {rs*512/1048576/span if span>0 else 0:6.1f}"
        )
