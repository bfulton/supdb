#!/bin/bash
# load_census for one arm inside a 640 MiB memory cgroup: census-cg.sh <arm> <out> [size] [every] [order]
# S is the session scratchpad and CG a child of the tool cgroup of the container
# it ran in (the parent's path is per container: read it from /proc/self/cgroup).
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad
CG=/sys/fs/cgroup/memory/process_api/01a18777-6cd5-708b-a866-7aded7120da4/claude-code-bash/census-probe
BIN=/home/user/supdb/bench/target/release/examples/load_census
mkdir -p $S/tmp-census
export TMPDIR=$S/tmp-census
mkdir -p $CG && echo 671088640 > $CG/memory.limit_in_bytes || exit 2
( echo $BASHPID > $CG/cgroup.procs; exec "$BIN" "$1" "${3:-5000000}" "${4:-100}" "${5:-shuffled}" "${6:-1}" ) > "$2" 2>&1
rc=$?
echo "rc $rc max_mb $(( $(cat $CG/memory.max_usage_in_bytes) >> 20 )) failcnt $(cat $CG/memory.failcnt)" >> "$2"
rmdir $CG
rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-census
