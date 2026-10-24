#!/bin/sh
# A pairing of two arms at five million keys under 640 MiB: five pairs a
# sitting after the warmup, both orders, the per-mix counters on. Usage:
# pair-5m.sh NAME A B; it writes NAME-ab-{1,2}.out (the table) and .err
# (the per-rep lines, each stamped with the uptime) beside this script, and
# NAME-head.txt with the tree it ran on. Read the .err with
# pairing_read.py A B NAME-ab-1.err NAME-ab-2.err.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-pair
name=${1:?NAME}; A=${2:?arm A}; B=${3:?arm B}
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/$name-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/$name-head.txt"
git -C /home/user/supdb status --short >> "$Z/$name-head.txt"
date -u +%FT%TZ >> "$Z/$name-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### $name sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 5 \
    --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
    2>&1 >"$Z/$name-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/$name-ab-$sitting.err"
  if grep -q ' quantities: ' "$Z/$name-ab-$sitting.out"; then echo "$name sitting $sitting: the table is whole"; else echo "$name sitting $sitting: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### $name done $(date -u +%T)"
