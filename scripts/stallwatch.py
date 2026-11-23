#!/usr/bin/env python3
"""Every thread's stack of a bounded pass that sits at its memory limit.

A pass of `bench run --bound` or `bench ab --bound` runs in a memory group
of its own (`supdb-bench-<pid>/pass-<n>` under the cgroup v1 memory
hierarchy, as `scripts/groupmem.py` reads it). A process the group kills
has usually sat at the limit first, and whether it was growing there or
waiting is the first thing to know: a store whose every thread is asleep
at its limit is a ring of waits, and no count of its memory by owner says
so. A group whose page cache has filled stands at its limit for as long
as it runs, with nothing wrong, so the limit is held against what the
kernel cannot take back: the group's anonymous and shared memory and its
kernel memory (`rss`, `shmem`, `memory.kmem.usage_in_bytes`). This
watches every such group, and where that sum has stood within `--within`
mebibytes of the limit for `--after` seconds it writes, for the group's
process, each thread's name, state and kernel stack and then the user
stacks by `gdb -batch`, up to `--dumps` times a stretch there. The first
version watched the group's usage, and dumped a store scanning in a
steady state with eighty mebibytes of file cache to spare.

Run it beside the bench, as root, before the passes start:

    python3 scripts/stallwatch.py OUTDIR &

Reads `/proc/<pid>/task/*/stack`, which wants root, and attaches gdb,
which stops the process for the length of the backtrace: the rates of a
run watched this way are not a row's.
"""

import argparse
import glob
import os
import subprocess
import time

GROUPS = "/sys/fs/cgroup/memory/**/supdb-bench-*/pass-*/"


def dump(out, group, pid, since, now):
    out.write("t %.3f group %s pid %s at its limit for %.2fs\n" % (now, group, pid, now - since))
    for task in sorted(glob.glob("/proc/%s/task/*" % pid)):
        try:
            comm = open(task + "/comm").read().strip()
            stat = open(task + "/stat").read()
            state = stat[stat.rindex(")") + 2 :].split()[0]
        except OSError:
            continue
        try:
            stack = open(task + "/stack").read()
        except OSError as e:
            stack = "(no kernel stack: %s)\n" % e
        out.write("--- tid %s %s state %s\n%s" % (os.path.basename(task), comm, state, stack))
    out.write("=== gdb\n")
    out.flush()
    try:
        r = subprocess.run(
            ["gdb", "-p", pid, "-batch", "-ex", "set pagination off", "-ex", "thread apply all bt 28"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=60,
        )
        out.write(r.stdout.decode("utf-8", "replace"))
    except (OSError, subprocess.SubprocessError) as e:
        out.write("gdb failed: %r\n" % (e,))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("outdir")
    ap.add_argument("--after", type=float, default=2.0, help="seconds at the limit before a dump")
    ap.add_argument("--within", type=int, default=40, help="mebibytes under the limit that count as at it")
    ap.add_argument("--dumps", type=int, default=3, help="dumps a stretch at the limit")
    a = ap.parse_args()
    os.makedirs(a.outdir, exist_ok=True)
    at = {}  # group -> (since, dumps, last)
    n = 0
    while True:
        now = time.clock_gettime(time.CLOCK_MONOTONIC)
        for g in glob.glob(GROUPS, recursive=True):
            try:
                stat = dict(l.split() for l in open(g + "memory.stat") if len(l.split()) == 2)
                kept = int(stat["rss"]) + int(stat["shmem"])
                kept += int(open(g + "memory.kmem.usage_in_bytes").read())
                lim = int(open(g + "memory.limit_in_bytes").read())
                procs = open(g + "cgroup.procs").read().split()
            except (OSError, ValueError, KeyError):
                continue
            if not procs or kept < lim - (a.within << 20):
                at.pop(g, None)
                continue
            since, dumps, last = at.get(g, (now, 0, 0.0))
            at[g] = (since, dumps, last)
            if now - since < a.after or dumps >= a.dumps or now - last < a.after * 0.75:
                continue
            at[g] = (since, dumps + 1, now)
            n += 1
            with open(os.path.join(a.outdir, "dump-%d.txt" % n), "w") as out:
                dump(out, g, procs[0], since, now)
        time.sleep(0.05)


if __name__ == "__main__":
    main()
