#!/usr/bin/env python3
"""What a bounded pass's memory group is charged for, by kind, through a run.

    python3 scripts/groupmem.py sample OUT          # beside the run, until killed
    python3 scripts/groupmem.py table OUT ERR [PHASE,...]

`sample` reads, every quarter second, the `memory.stat` of every group the
suite has made for a pass (`supdb-bench-<pid>/pass-<n>` under the cgroup v1
memory hierarchy, `bound::Bound::pass`), its `memory.usage_in_bytes` and its
`memory.kmem.usage_in_bytes`, and the kernel's count of a few slab caches
from `/proc/slabinfo`, which is the machine's and not the group's: one line
a group a sample, stamped on the monotonic clock. The page cache and the
anonymous memory are megabytes, the kernel memory and the usage kilobytes,
the slab caches objects.

`table` lays the samples to the phase marks a run prints under
`SUPDB_LAG_MARK=1` (`phasemark ARM PHASE start|end MONO` on stderr, kept in
ERR): the sample nearest each pass's start, and the end of a load and of
the last scrambled mix.

The kernel memory is what the cache's and the process's bytes leave out of
a group's usage, and a group is bounded on the sum. A page the cache holds
because it was written carries the filesystem's buffer heads, 104 bytes a
block, where a page read back from the device carries none, so two stores
with the same files in the cache can stand megabytes apart under one limit
(`docs/engine.md`, the group's kernel memory).
"""
import glob
import os
import re
import sys
import time

STAT = (
    "cache",
    "rss",
    "mapped_file",
    "inactive_file",
    "active_file",
    "pgmajfault",
    "dirty",
    "writeback",
)
COUNTS = ("pgmajfault",)
SLABS = ("buffer_head", "radix_tree_node", "ext4_inode_cache", "dentry")


def groups():
    found = []
    for root, dirs, _ in os.walk("/sys/fs/cgroup/memory"):
        for d in dirs:
            if d.startswith("supdb-bench-"):
                base = os.path.join(root, d)
                found.append(base)
                found.extend(glob.glob(base + "/*/"))
        # The suite's groups are leaves of the group the run started in.
        dirs[:] = [d for d in dirs if not d.startswith("supdb-bench-")]
    return found


def sample(out_path):
    out = open(out_path, "w")
    known, looked = [], 0.0
    while True:
        t = time.clock_gettime(time.CLOCK_MONOTONIC)
        if t - looked > 1.0:
            known, looked = groups(), t
        slabs = ""
        try:
            for line in open("/proc/slabinfo"):
                w = line.split()
                if w and w[0] in SLABS:
                    slabs += " %s=%s" % (w[0], w[1])
        except OSError:
            pass
        for g in known:
            try:
                st = dict(
                    l.split()
                    for l in open(os.path.join(g, "memory.stat"))
                    if len(l.split()) == 2
                )
                procs = open(os.path.join(g, "cgroup.procs")).read().split()
            except OSError:
                continue
            if not procs:
                continue
            extra = ""
            for name, f in (
                ("usage_kb", "memory.usage_in_bytes"),
                ("kmem_kb", "memory.kmem.usage_in_bytes"),
            ):
                try:
                    extra += " %s=%d" % (name, int(open(os.path.join(g, f)).read()) >> 10)
                except (OSError, ValueError):
                    pass
            fields = " ".join(
                "%s=%s" % (k, st.get(k, 0))
                if k in COUNTS
                else "%s=%d" % (k, int(st.get(k, 0)) >> 20)
                for k in STAT
            )
            out.write(
                "mem %.3f %s %s %s%s%s\n"
                % (t, os.path.basename(g.rstrip("/")), procs[0], fields, extra, slabs)
            )
        out.flush()
        time.sleep(0.25)


def table(out_path, err_path, phases):
    marks = []
    for line in open(err_path, errors="replace"):
        m = re.match(r"phasemark (\S+) (\S+) (start|end) ([\d.]+)", line)
        if m:
            marks.append((float(m.group(4)), m.group(1), m.group(2), m.group(3)))
    marks.sort()
    mem = []
    for line in open(out_path):
        p = line.split()
        if len(p) < 5 or p[0] != "mem":
            continue
        mem.append((float(p[1]), {k: int(v) for k, v in (x.split("=") for x in p[4:])}))
    if not marks or not mem:
        sys.exit("groupmem: no phase marks in %s or no samples in %s" % (err_path, out_path))
    print(
        "%-34s %-14s %5s | %6s %6s %8s %8s %10s %9s"
        % ("arm", "phase", "edge", "cache", "rss", "usage", "kernel", "buf heads", "maj flt")
    )
    last = marks[-1][2]
    for t, arm, phase, edge in marks:
        if phases and phase not in phases:
            continue
        if edge == "end" and phase not in ("load", "ycsbs-E", last):
            continue
        _, d = min(mem, key=lambda s: abs(s[0] - t))
        print(
            "%-34s %-14s %5s | %6d %6d %8.0f %8.1f %10d %9d"
            % (
                arm,
                phase,
                edge,
                d.get("cache", 0),
                d.get("rss", 0),
                d.get("usage_kb", 0) / 1024,
                d.get("kmem_kb", 0) / 1024,
                d.get("buffer_head", 0),
                d.get("pgmajfault", 0),
            )
        )


def main():
    a = sys.argv[1:]
    if len(a) == 2 and a[0] == "sample":
        sample(a[1])
    elif len(a) in (3, 4) and a[0] == "table":
        table(a[1], a[2], a[3].split(",") if len(a) == 4 else [])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
