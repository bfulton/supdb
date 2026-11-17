#!/usr/bin/env python3
# E8's memory sampler (E7's plus max_usage, failcnt, oom_kill): every PERIOD seconds, for each bench cgroup
# (supdb-bench-*) under the bash group, one line of the group's memory.stat
# fields that separate anonymous memory, the cipher's plaintext (shmem: the
# pager's memfds) and the kernel's page cache of the store's files, plus the
# pass child's own RssAnon/RssFile/RssShmem from /proc/<pid>/status, the arm
# taken from the child's cmdline (`bench one-pass <root>/<arm>-<rep>-<i>.spec.json`),
# and the block device's read sectors. Time is CLOCK_MONOTONIC (time.monotonic),
# the clock the harness's `phasemark` lines use (SUPDB_LAG_MARK=1).
# It reads /proc and /sys files only; it touches nothing of the engine.
import glob, os, sys, time

PARENT = sys.argv[1]
OUT = sys.argv[2]
PERIOD = float(sys.argv[3]) if len(sys.argv) > 3 else 2.0
FIELDS = ["rss", "rss_huge", "cache", "shmem", "mapped_file", "dirty", "writeback",
          "pgmajfault", "workingset_refault_file", "inactive_file", "active_file",
          "inactive_anon", "active_anon"]


def read(p):
    try:
        with open(p) as f:
            return f.read()
    except OSError:
        return None


def stat(g):
    t = read(os.path.join(g, "memory.stat")) or ""
    d = {}
    for line in t.splitlines():
        k, _, v = line.partition(" ")
        if k in FIELDS:
            d[k] = int(v)
    return d


def status(pid):
    t = read(f"/proc/{pid}/status") or ""
    d = {}
    for line in t.splitlines():
        for k in ("RssAnon", "RssFile", "RssShmem", "VmHWM"):
            if line.startswith(k + ":"):
                d[k] = int(line.split()[1])  # kB
    return d


with open(OUT, "a", buffering=1) as out:
    while True:
        now = time.monotonic()
        dev = read("/sys/block/vda/stat")
        rsect = int(dev.split()[2]) if dev else -1
        for g in glob.glob(os.path.join(PARENT, "supdb-bench-*")):
            procs = (read(os.path.join(g, "cgroup.procs")) or "").split()
            st = stat(g)
            usage = (read(os.path.join(g, "memory.usage_in_bytes")) or "0").strip()
            maxu = (read(os.path.join(g, "memory.max_usage_in_bytes")) or "0").strip()
            failcnt = (read(os.path.join(g, "memory.failcnt")) or "0").strip()
            oomc = read(os.path.join(g, "memory.oom_control")) or ""
            oomk = next((l.split()[1] for l in oomc.splitlines() if l.startswith("oom_kill ")), "0")
            for pid in procs:
                cmd = (read(f"/proc/{pid}/cmdline") or "").split("\0")
                if "one-pass" not in cmd:
                    continue
                spec = cmd[cmd.index("one-pass") + 1] if len(cmd) > cmd.index("one-pass") + 1 else ""
                arm = os.path.basename(spec).rsplit(".spec.json", 1)[0]
                ps = status(pid)
                mb = lambda b: b / 1048576.0
                out.write(
                    f"{now:.3f} {arm} pid {pid} usage_mb {mb(int(usage)):.0f} "
                    + " ".join(f"{k}_mb {mb(st.get(k, 0)):.0f}" for k in ("rss", "cache", "shmem", "mapped_file", "inactive_file", "active_file", "dirty"))
                    + f" pgmajfault {st.get('pgmajfault', 0)} refault {st.get('workingset_refault_file', 0)}"
                    + f" RssAnon_mb {ps.get('RssAnon', 0)/1024:.0f} RssFile_mb {ps.get('RssFile', 0)/1024:.0f} RssShmem_mb {ps.get('RssShmem', 0)/1024:.0f} VmHWM_mb {ps.get('VmHWM', 0)/1024:.0f}"
                    + f" vda_rsect {rsect} maxusage_mb {mb(int(maxu)):.0f} failcnt {failcnt} oom_kill {oomk}\n"
                )
        time.sleep(PERIOD)
