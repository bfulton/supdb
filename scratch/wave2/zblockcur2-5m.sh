#!/bin/sh
# The second pairing of the block read through one cursor, with the walk leaving a group spent at its limit to the next read (`ZCursor::resume`): the combined arm as it now ships
# (`Options::zpages_block_cursor` on: a build's or a walk's reads of a
# covered partition through one cursor held across the block, the cuts
# where it stops) against `supdb-cache64tofitautozpagesperrun`, a cursor
# a run, a seek a cut and a read by rank a replaced record, at five
# million keys under 640 MiB, five reps a sitting, both orders, with the
# per-mix counters on. The counts -- groups decoded, memo asks, scans
# served -- are the mechanism's evidence; the rates are read as a whole
# table in both orders.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-zblockcur
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/zblockcur2-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/zblockcur2-head.txt"
git -C /home/user/supdb status --short >> "$Z/zblockcur2-head.txt"
date -u +%FT%TZ >> "$Z/zblockcur2-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
pairing() {
  name=$1; A=$2; B=$3
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
}
pairing zblockcur5mb supdb-cache64tofitautozpagesperrun supdb-cache64tofitautozpages
echo "##### zblockcur2 done $(date -u +%T)"
