#!/bin/sh
# bound.sh LIMIT_BYTES CMD... : run CMD in a memory cgroup of its own (cgroup v1), a child of the one this shell is in.
# The probe's files should be dropped from the page cache first (drop.py), or pages another group paid for are hits.
P=/sys/fs/cgroup/memory$(sed -n 's/^[0-9]*:memory:\(.*\)$/\1/p' /proc/self/cgroup)
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
