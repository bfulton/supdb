#!/usr/bin/env python3
# One row per sitting of each NAME: the cipher arm's read cell beside the clear arm's, with
# the sampler joined over the cipher arm's read window (second half for held/shmem/cache).
# usage: summary.py NAME [NAME...]   (pairs/NAME-*-k.{out,err,rc}, samples/NAME-k.txt)
import glob, os, re, sys, statistics as st
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
_argv = sys.argv; sys.argv = [sys.argv[0]]
from report import table, samples, marks, med, S
sys.argv = _argv

READS = 5_000_000


def rowsof(path):
    arms, rows = None, {}
    for line in open(path):
        if line.startswith("quantity"):
            arms = line.split()[1:3]
            continue
        f = line.split()
        if len(f) >= 4 and f[0] in ("read", "count", "load", "load-shuffled", "scan"):
            try:
                rows[f[0] + " " + f[1]] = (float(f[2]), float(f[3]))
            except ValueError:
                pass
    return arms, rows


print("name k | arm | reads/s | clear r/s | xclear | misses/rd | majflt/rd | clear majflt | held med2 | shmem med2/max | shmem/held | cache-shmem med2 | usage med/max | RssAnon | limit med (lo..hi) | peak | breaths | failcnt+ | oom")
for name in sys.argv[1:]:
    for out in sorted(glob.glob(f"{S}/pairs/{name}-*-[12].out")):
        k = out[-5]
        err = out[:-4] + ".err"
        rc = open(out[:-4] + ".rc").read().strip()
        arms, rows = rowsof(out)
        if not arms:
            print(f"{name} {k} | rc {rc} no table"); continue
        ci = 0 if "cipher" in arms[0] else 1
        if "cipher" not in arms[ci]:
            ci = 0
        cl = 1 - ci
        g = lambda r, i: rows.get(r, (float("nan"),) * 2)[i]
        sm = samples(f"{S}/samples/{name}-{k}.txt")
        mk, tr = marks(err)
        arm = arms[ci]
        w = mk.get((arm, "read"), {})
        extra = ""
        if "start" in w and "end" in w:
            a, b = w["start"], w["end"]
            inw = [s for s in sm if a <= s["t"] <= b and s["arm"].startswith(arm + "-")]
            half = [s for s in inw if s["t"] >= (a + b) / 2]
            trh = [x for x in tr if (a + b) / 2 <= x[0] <= b]
            trw = [x for x in tr if a <= x[0] <= b]
            ratio = []
            for (t, held, lim, unb) in trh:
                if held > 0 and half:
                    near = min(half, key=lambda s: abs(s["t"] - t))
                    if abs(near["t"] - t) < 1.0:
                        ratio.append(near.get("shmem_mb", 0) * 1048576 / held)
            gv = lambda key, xs: [s.get(key, 0) for s in xs]
            cs = [s.get("cache_mb", 0) - s.get("shmem_mb", 0) for s in half]
            fc = (inw[-1].get("failcnt", 0) - inw[0].get("failcnt", 0)) if inw else float("nan")
            ok = inw[-1].get("oom_kill", 0) if inw else float("nan")
            extra = (f"{med([x[1] for x in trh])/1048576:.0f} | "
                     f"{med(gv('shmem_mb', half)):.0f}/{max(gv('shmem_mb', inw) or [0]):.0f} | "
                     f"{med(ratio):.3f} | {med(cs):.0f} | "
                     f"{med(gv('usage_mb', inw)):.0f}/{max(gv('usage_mb', inw) or [0]):.0f} | "
                     f"{med(gv('RssAnon_mb', inw)):.0f} | "
                     f"{med([x[2] for x in trw])/1048576:.0f} ({g('count cipher_limit_lo', ci)/1048576:.0f}..{g('count cipher_limit_hi', ci)/1048576:.0f}) | "
                     f"{g('count cipher_peak', ci)/1048576:.0f} | {g('count cipher_breaths', ci):.0f} | {fc:.0f} | {ok:.0f}")
        else:
            extra = f"(no read window for {arm}; rc {rc})"
        rr, rc_ = g("read reads_per_s", ci), g("read reads_per_s", cl)
        print(f"{name} {k} rc{rc} | {arm} | {rr:,.0f} | {rc_:,.0f} | {rr/rc_:.3f} | "
              f"{g('count cipher_fills', ci)/READS:.3f} | {g('read majfaults_per_op', ci):.3f} | "
              f"{g('read majfaults_per_op', cl):.3f} | " + extra)
