#!/bin/sh
# Room above a bounded run. A pass's bound is a group of its own, made
# under the group the run was started in, and a limit on a group above it
# is a second bound on the pass: at that limit the kernel takes pages from
# every group under it at each charge, the pass's among them, however far
# the pass is under its own. On this host the run starts in the tool's own
# group, which a day's builds fill with page cache to its limit, and the
# first pass of a bounded sitting then started with tens of megabytes of
# its store pushed out that no bound of its own had asked for.
#
#   sh scripts/room.sh          # make room, then say what the group holds
#
# Making room is dropping the page cache (clean pages only, nothing a
# process holds is lost), which wants root and a writable sysctl; where
# that is not this shell, nothing is dropped and the line says so. Each
# pass records the room it began with either way (`bound-start
# room_above_bytes`), and the run reports a pass that began short of its
# bound (`run::room_note`). Never beside a timing run: it empties the
# cache the run is reading from.
set -u
if [ -w /proc/sys/vm/drop_caches ] && sync && echo 1 > /proc/sys/vm/drop_caches 2>/dev/null; then
  did="page cache dropped"
else
  did="page cache NOT dropped (not root, or the sysctl is read-only)"
fi
g=$(awk -F: '$2 ~ /(^|,)memory(,|$)/ {print $3}' /proc/self/cgroup 2>/dev/null | head -n 1)
d=/sys/fs/cgroup/memory$g
if [ -n "$g" ] && [ -r "$d/memory.limit_in_bytes" ]; then
  lim=$(cat "$d/memory.limit_in_bytes"); use=$(cat "$d/memory.usage_in_bytes")
  echo "room.sh: $did; the group this shell is in holds $((use >> 20)) MB of a limit of $((lim >> 20)) MB ($d)"
else
  echo "room.sh: $did; no cgroup v1 memory group to read here"
fi
