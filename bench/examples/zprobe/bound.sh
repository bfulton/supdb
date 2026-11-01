#!/bin/sh
# bound.sh LIMIT_BYTES CMD... : run CMD in a memory cgroup of its own (cgroup v1), a child of the one this shell is in.
# The probe's files should be dropped from the page cache first (drop.py), or pages another group paid for are hits.
# The memory controller's mount and this shell's path under it, as bench/src/bound.rs resolves them: the controller
# may share a line with others (`3:cpu,memory:/a/b`), and a host without it is refused, never placed at the root.
M=$(awk '$3 == "cgroup" && $4 ~ /(^|,)memory(,|$)/ { print $2; exit }' /proc/self/mounts)
C=$(sed -n 's/^[0-9]*:\([^:]*,\)\{0,1\}memory\(,[^:]*\)\{0,1\}:\(.*\)$/\3/p' /proc/self/cgroup)
[ -n "$M" ] && [ -n "$C" ] || { echo "bound.sh: no cgroup v1 memory controller here" >&2; exit 1; }
P=$M$C
L=$1; shift
G=$P/zprobe-$$
mkdir "$G" || exit 1
echo "$L" > "$G/memory.limit_in_bytes" || { rmdir "$G"; exit 1; }
echo $$ > "$G/cgroup.procs" || { rmdir "$G"; exit 1; }
"$@"
rc=$?
echo $$ > "$P/cgroup.procs"
rmdir "$G"
exit $rc
