#!/bin/sh
# Stage 4's in-memory pairing (the design's 7.5): the controller's arm
# (`supdb-autozpages`) against its base (`supdb`) in one process at a
# hundred thousand and three hundred thousand keys, every pass, twelve
# pairs a sitting, two sittings with the arms swapped. In memory the
# controller must build nothing and the pair must read level: nothing
# marked under Holm, no mark keeping its sign in the second sitting, every
# `zpage_*` counter of the arm zero but the controller's own ticks. Needs a
# free machine (bench/CLAUDE.md), and the environment clean of the
# suite's instruments, which it sets for both arms.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-auto
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/auto-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/auto-head.txt"
date -u +%FT%TZ >> "$Z/auto-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u SUPDB_ZPAGES_TRACE -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for size in 100000 300000; do
  for sitting in 1 2; do
    if [ "$sitting" = 1 ]; then arms=supdb,supdb-autozpages; else arms=supdb-autozpages,supdb; fi
    echo "##### size $size sitting $sitting: $arms, start $(date -u +%T)"
    env $UNSET "$BIN" ab --arms "$arms" --size "$size" --reps 12 \
      > "$Z/auto-ab-$size-$sitting.out" 2> "$Z/auto-ab-$size-$sitting.err"
    rc=$?
    if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/auto-ab-$size-$sitting.out"; then
      echo "size $size sitting $sitting: the table is whole (exit $rc)"
    else
      echo "size $size sitting $sitting: NOT whole (exit $rc)"
    fi
    rm -rf "$T"/*
  done
done
echo "##### done $(date -u +%T)"
