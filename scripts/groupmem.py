#!/usr/bin/env python3
"""What a bounded pass's memory group is charged for, by kind, through a run.

    python3 scripts/groupmem.py sample OUT          # beside the run, until killed
    python3 scripts/groupmem.py table OUT ERR [PHASE,...]
    python3 scripts/groupmem.py end OUT ERR [PID]

`sample` reads, every quarter second (`GROUPMEM_INTERVAL_MS` for another
period: a peak a load makes lasts a tenth of a second), the `memory.stat`
of every group the
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

With `GROUPMEM_SMAPS_MS` set, `sample` also reads, that often, the
`/proc/PID/smaps` of each group's process and writes a `maps` line: its
resident bytes by class of mapping, the main heap (`[heap]`), the stacks
(`[stack]` and a mapping behind an unreadable guard), every other
anonymous mapping and the files' mappings, in megabytes, then the other
anonymous mappings' sizes with their resident megabytes, the most
resident first. The kernel merges two anonymous mappings that lie side by
side, an engine's block beside an arena's heap among them, so neither the
sizes nor a class drawn from them say whose the bytes are; that is read
from the engine's own count and the allocator's, `blockwatch` on the
run's stderr (`SUPDB_BLOCKS_WATCH`). A read of smaps takes the process's
mapping lock and walks its page tables, a millisecond or so, so the
period is longer than the group's: a tenth of a second, against the
group's ten milliseconds up to a kill.

`end` reads a pass's last moments, for every pass's process in the samples
or the one PID names: the group's last sample and its largest charge, the
last `maps` line and the last `blockwatch` line before the last sample,
and from them who held the group's charge: the page cache, the kernel's
memory, and of the anonymous bytes the engine's mapped blocks resident and
the rest, beside what the allocator has mapped, which is mapped and not
resident -- an arena's top and a chunk of its own are pages nobody touched
until somebody does.

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
    "shmem",
    "mapped_file",
    "inactive_file",
    "active_file",
    "pgmajfault",
    "dirty",
    "writeback",
)
COUNTS = ("pgmajfault",)
INTERVAL = int(os.environ.get("GROUPMEM_INTERVAL_MS", "250")) / 1000.0
SMAPS = int(os.environ.get("GROUPMEM_SMAPS_MS", "0")) / 1000.0
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


def mappings(pid):
    """The process's mappings from smaps: (start, end, perms, anonymous,
    path, resident bytes)."""
    maps, cur = [], None
    with open("/proc/%s/smaps" % pid) as f:
        for line in f:
            head = line.split(None, 5)
            if head and "-" in head[0] and not head[0].endswith(":"):
                if cur:
                    maps.append(cur)
                lo, hi = (int(x, 16) for x in head[0].split("-"))
                path = head[5].strip() if len(head) > 5 else ""
                anon = head[4] == "0" and (path == "" or path.startswith("["))
                cur = [lo, hi, head[1], anon, path, 0]
            elif cur and line.startswith("Rss:"):
                cur[5] = int(line.split()[1]) << 10
    if cur:
        maps.append(cur)
    return maps


def classify(maps):
    """Resident bytes by class (heap, stack, anon, file), and the other
    anonymous mappings as (size, count, resident)."""
    by = {"heap": 0, "stack": 0, "anon": 0, "file": 0}
    sizes = {}
    for i, (lo, hi, perms, anon, path, rss) in enumerate(maps):
        if not anon:
            by["file"] += rss
            continue
        if path == "[heap]":
            by["heap"] += rss
            continue
        if path.startswith("[stack"):
            by["stack"] += rss
            continue
        if not perms.startswith("rw"):
            continue
        prev = maps[i - 1] if i > 0 else None
        if prev and prev[1] == lo and prev[3] and prev[2].startswith("---") and prev[1] - prev[0] <= 64 << 10:
            by["stack"] += rss
            continue
        by["anon"] += rss
        g = sizes.setdefault(hi - lo, [0, 0])
        g[0] += 1
        g[1] += rss
    return by, sorted(((k, v[0], v[1]) for k, v in sizes.items()), key=lambda x: -x[2])


def maps_line(t, group, pid):
    by, sizes = classify(mappings(pid))
    mb = lambda b: b / (1 << 20)
    return "maps %.3f %s %s %s | %s\n" % (
        t,
        group,
        pid,
        " ".join("%s=%.1f" % (k, mb(v)) for k, v in by.items()),
        " ".join("%.1fmbx%d=%.1f" % (mb(z), n, mb(r)) for z, n, r in sizes[:10]),
    )


def sample(out_path):
    out = open(out_path, "w")
    known, looked, smapped = [], 0.0, 0.0
    while True:
        t = time.clock_gettime(time.CLOCK_MONOTONIC)
        if t - looked > 1.0:
            known, looked = groups(), t
        read_maps = SMAPS > 0 and t - smapped >= SMAPS
        if read_maps:
            smapped = t
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
            if read_maps:
                try:
                    out.write(maps_line(t, os.path.basename(g.rstrip("/")), procs[0]))
                except (OSError, ValueError, IndexError):
                    pass
        out.flush()
        time.sleep(INTERVAL)


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


def end(out_path, err_path, only):
    mem, maps, watch = {}, {}, {}
    for line in open(out_path):
        p = line.split()
        if len(p) >= 5 and p[0] == "mem":
            d = {k: int(v) for k, v in (x.split("=") for x in p[4:])}
            mem.setdefault(p[3], []).append((float(p[1]), p[2], d))
        elif len(p) >= 4 and p[0] == "maps":
            maps.setdefault(p[3], []).append((float(p[1]), line.split(" ", 4)[4].strip()))
    for line in open(err_path, errors="replace"):
        m = re.match(r"blockwatch (\d+) (\S+) (\d+) ([\d.]+) (\S+) (.*)", line)
        if m:
            watch.setdefault(m.group(1), []).append((float(m.group(4)), m.group(2), m.group(3), m.group(5), m.group(6)))
    pids = [only] if only else sorted(mem, key=lambda q: mem[q][-1][0])
    if not pids or not any(q in mem for q in pids):
        sys.exit("groupmem: no samples of %s in %s" % (only or "any process", out_path))
    for pid in pids:
        if pid not in mem:
            print("pid %s: no samples" % pid)
            continue
        ms = mem[pid]
        t_end, group, last = ms[-1]
        top = max(ms, key=lambda s: s[2].get("usage_kb", 0))
        print("pid %s %s: %d samples, %.3f s to %.3f" % (pid, group, len(ms), ms[0][0], t_end))
        for name, (t, _, d) in (("last", ms[-1]), ("largest", top)):
            print(
                "  %-7s %.3f usage %d MB = cache %d + rss %d + kernel %.0f; shmem %d mapped_file %d"
                % (
                    name,
                    t,
                    d.get("usage_kb", 0) >> 10,
                    d.get("cache", 0),
                    d.get("rss", 0),
                    d.get("kmem_kb", 0) / 1024,
                    d.get("shmem", 0),
                    d.get("mapped_file", 0),
                )
            )
        mp = [x for x in maps.get(pid, []) if x[0] <= t_end + 0.001]
        if mp:
            t, text = mp[-1]
            print("  maps    %.3f %s" % (t, text))
        wl = [x for x in watch.get(pid, []) if x[0] <= t_end + 0.001]
        if not wl:
            continue
        t, arm, rep, why, rest = wl[-1]
        print("  engine  %.3f %s rep %s %s %s" % (t, arm, rep, why, rest))
        f = dict(re.findall(r"(\w+_mb) ([\d.]+)", rest.split(" | ")[0]))
        f = {k: float(v) for k, v in f.items()}
        anon = f.get("anon_mb", 0)
        engine = f.get("resident_mb", 0)
        alloc = f.get("heap_mb", 0) + f.get("mmapped_mb", 0)
        print(
            "  owners  of %.0f MB anonymous at %.3f: the engine's blocks %.1f resident, the rest %.1f, beside the allocator's %.1f mapped (arenas %.1f, %.1f of it free; its own mappings %.1f); of the group's %d MB at %.3f: page cache %d, kernel %.0f"
            % (
                anon,
                t,
                engine,
                anon - engine,
                alloc,
                f.get("heap_mb", 0),
                f.get("free_mb", 0),
                f.get("mmapped_mb", 0),
                last.get("usage_kb", 0) >> 10,
                t_end,
                last.get("cache", 0),
                last.get("kmem_kb", 0) / 1024,
            )
        )


def main():
    a = sys.argv[1:]
    if len(a) == 2 and a[0] == "sample":
        sample(a[1])
    elif len(a) in (3, 4) and a[0] == "table":
        table(a[1], a[2], a[3].split(",") if len(a) == 4 else [])
    elif len(a) in (3, 4) and a[0] == "end":
        end(a[1], a[2], a[3] if len(a) == 4 else None)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
