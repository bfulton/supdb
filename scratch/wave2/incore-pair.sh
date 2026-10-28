#!/bin/sh
# A pairing of two arms in core: every pass, at SIZES (default 100000 and
# 300000), five pairs a sitting after the warmup, both orders, the per-mix
# times by kind on for both. Usage: incore-pair.sh NAME A B; writes
# NAME-<size>-ab-{1,2}.{out,err} beside this script.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-incore
name=${1:?NAME}; A=${2:?arm A}; B=${3:?arm B}; SIZES=${SIZES:-100000 300000}
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/$name-head.txt"
git -C /home/user/supdb status --short >> "$Z/$name-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for size in $SIZES; do
  for sitting in 1 2; do
    if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
    echo "##### $name $size sitting $sitting: $arms, start $(date -u +%T)"
    env $UNSET SUPDB_MIX_KINDS=1 "$BIN" ab --arms "$arms" --size "$size" --reps 5 \
      > "$Z/$name-$size-ab-$sitting.out" 2> "$Z/$name-$size-ab-$sitting.err"
    if grep -q ' quantities: ' "$Z/$name-$size-ab-$sitting.out"; then echo "whole"; else echo "NOT whole"; fi
    rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-incore/supdb-*
  done
done
echo "##### $name done $(date -u +%T)"
