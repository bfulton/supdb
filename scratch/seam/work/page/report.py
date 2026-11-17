#!/usr/bin/env python3
# E8's tables: for each pairing NAME and sitting k, the ab rows that matter
# per arm, and the sampler's memory joined to the arm's phase windows
# (phasemark lines, CLOCK_MONOTONIC) and to the pager's trace.
# usage: report.py NAME [NAME...]   (reads pairs/NAME-*-k.{out,err}, samples/NAME-k.txt)
import glob, os, re, sys, statistics as st

S = os.path.dirname(os.path.abspath(__file__))
ROWS = ["read reads_per_s", "read p99_us", "read majfaults_per_op", "read read_bytes_per_op",
        "read read_us_per_io", "read refaults_per_op", "read bg_read_bytes_per_op", "load device_bytes_per_byte", "scan entries_per_s", "scan majfaults_per_op", "scan read_bytes_per_op",
        "load ops_per_s", "load-shuffled ops_per_s",
        "count cipher_fills", "count cipher_direct_reads", "count cipher_direct_bytes",
        "count cipher_direct_us", "count cipher_run_chunks", "count cipher_nowait_hits",
        "count cipher_evicted_units", "count cipher_held_at_end", "count cipher_peak",
        "count cipher_limit_lo", "count cipher_limit_hi", "count cipher_waits",
        "count cipher_rescues", "count cipher_breaths", "count cipher_evictor_longest_wait_us"]


def table(path):
    arms, rows = None, {}
    for line in open(path):
        if line.startswith("quantity"):
            arms = line.split()[1:3]
            continue
        for r in ROWS:
            if line.startswith(r + " ") or line.startswith(r + "\t"):
                f = line[len(r):].split()
                try:
                    rows[r] = (float(f[0]), float(f[1]))
                except (ValueError, IndexError):
                    pass
    return arms, rows


def samples(path):
    out = []
    if not os.path.exists(path):
        return out
    for line in open(path):
        f = line.split()
        if len(f) < 10:
            continue
        d = {"t": float(f[0]), "arm": f[1]}
        for i in range(2, len(f) - 1, 2):
            try:
                d[f[i]] = float(f[i + 1])
            except ValueError:
                pass
        out.append(d)
    return out


def marks(err):
    m = {}
    tr = []
    for line in open(err):
        g = re.match(r"phasemark (\S+) (\S+) (\S+) ([0-9.]+)", line)
        if g:
            m.setdefault((g.group(1), g.group(2)), {})[g.group(3)] = float(g.group(4))
        g = re.match(r"pagertrace ([0-9.]+) held (\d+) limit (\d+) unbacked (\d+)", line)
        if g:
            tr.append((float(g.group(1)), int(g.group(2)), int(g.group(3)), int(g.group(4))))
    return m, tr


def med(xs):
    return st.median(xs) if xs else float("nan")


for name in sys.argv[1:]:
    for out in sorted(glob.glob(f"{S}/pairs/{name}-*-[12].out")):
        k = out[-5]
        err = out[:-4] + ".err"
        rc = open(out[:-4] + ".rc").read().strip()
        arms, rows = table(out)
        print(f"### {name} sitting {k} (rc {rc}) arms {arms}")
        for r in ROWS:
            if r in rows:
                a, b = rows[r]
                print(f"  {r:42s} {a:16.3f} {b:16.3f}")
        sm = samples(f"{S}/samples/{name}-{k}.txt")
        mk, tr = marks(err)
        for (arm, phase), w in sorted(mk.items(), key=lambda x: x[1].get("start", 0)):
            if "start" not in w or "end" not in w:
                continue
            a, b = w["start"], w["end"]
            inw = [s for s in sm if a <= s["t"] <= b]
            half = [s for s in inw if s["t"] >= (a + b) / 2]
            if not inw:
                continue
            g = lambda key, xs=inw: [s.get(key, 0) for s in xs]
            cs = [s.get("cache_mb", 0) - s.get("shmem_mb", 0) for s in half]
            dev = (inw[-1]["vda_rsect"] - inw[0]["vda_rsect"]) * 512 / 1048576
            fc = inw[-1].get("failcnt", 0) - inw[0].get("failcnt", 0)
            ok = inw[-1].get("oom_kill", 0)
            ratio = []
            for (t, held, lim, unb) in tr:
                if a <= t <= b and held > 0:
                    near = min(inw, key=lambda s: abs(s["t"] - t))
                    if abs(near["t"] - t) < 1.0:
                        ratio.append(near.get("shmem_mb", 0) * 1048576 / held)
            trw = [x for x in tr if a <= x[0] <= b]
            print(f"  mem {arm:18s} {phase:14s} {b-a:7.1f}s usage med {med(g('usage_mb')):.0f} max {max(g('usage_mb')):.0f}"
                  f" maxusage {max(g('maxusage_mb')):.0f} rss {med(g('rss_mb')):.0f} RssAnon {med(g('RssAnon_mb')):.0f}"
                  f" shmem med {med(g('shmem_mb')):.0f} max {max(g('shmem_mb')):.0f}"
                  f" cache-shmem(2nd half) med {med(cs):.0f} max {max(cs) if cs else float('nan'):.0f}"
                  f" devMB {dev:.0f} failcnt+ {fc:.0f} oom_kill {ok:.0f}"
                  + (f" held med {med([x[1] for x in trw])/1048576:.0f}MiB limit {med([x[2] for x in trw])/1048576:.0f}MiB"
                     f" unbacked max {max(x[3] for x in trw)/1048576:.0f}MiB" if trw else "")
                  + (f" shmem/held med {med(ratio):.3f}" if ratio else ""))
        print()
