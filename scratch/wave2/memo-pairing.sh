#!/bin/sh
# The pairing that prices `Options::zpages_memo`: the forced arm with the
# memo of decoded groups (`supdb-nocachezpages`) against the arm without it
# (`supdb-nocachezpagesnomemo`), in one process, in core at three hundred
# thousand keys over the load, the point-read pass and the mixes, two
# sittings with the arms swapped. The memo saves a decode on every read of
# a page the thread decoded before, so the point reads and the C mix are
# the cells it is read on, with `zpage_memo_hits` and `zpage_decodes` beside
# them to say how often it answered. Needs a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-memo
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/memo-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/memo-head.txt"
date -u +%FT%TZ >> "$Z/memo-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=supdb-nocachezpages,supdb-nocachezpagesnomemo; else arms=supdb-nocachezpagesnomemo,supdb-nocachezpages; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET "$BIN" ab --arms "$arms" --size 300000 --reps 6 --passes load,read,mixes --pairs 'ycsb-C' \
    > "$Z/memo-ab-$sitting.out" 2> "$Z/memo-ab-$sitting.err"
  rc=$?
  if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/memo-ab-$sitting.out"; then
    echo "sitting $sitting: the table is whole (exit $rc)"
  else
    echo "sitting $sitting: NOT whole (exit $rc)"
  fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
