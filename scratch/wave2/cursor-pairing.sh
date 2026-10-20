#!/bin/sh
# Piece 9's pairings: the overlay walk through the cursor
# (`supdb-nocachezpages`, the default) against the walk cut by rank
# (`supdb-nocachezpagesrankwalk`), in one process, six pairs after the
# warmup, two sittings with the arms swapped. `sh cursor-pairing.sh core`
# runs in core at three million keys over the load, the scan pass and the
# mixes with the operations divided by ten; `sh cursor-pairing.sh ooc` under
# a bound of a gibibyte at ten million keys with the threaded scans too. E
# is the cell the counts named (`e-probe.sh`), the scan passes the cells
# that must not move, since a drained store's walk meets no unsealed key.
# Needs a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-cursor
WHAT=${1:-core}
case "$WHAT" in
  core) SIZE="--size 3000000"; PASSES=load,scan,mixes ;;
  ooc) SIZE="--size 10000000 --bound 1073741824"; PASSES=load,scan,scan-threaded,mixes ;;
  *) echo "core or ooc"; exit 2 ;;
esac
A=supdb-nocachezpages; B=supdb-nocachezpagesrankwalk
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > /dev/null 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/cursor-$WHAT-head.txt"
date -u +%FT%TZ >> "$Z/cursor-$WHAT-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET "$BIN" ab --arms "$arms" $SIZE --ops-div 10 --reps 6 --passes "$PASSES" --pairs 'ycsb-E' \
    > "$Z/cursor-$WHAT-ab-$sitting.out" 2> "$Z/cursor-$WHAT-ab-$sitting.err"
  rc=$?
  if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/cursor-$WHAT-ab-$sitting.out"; then
    echo "sitting $sitting: the table is whole (exit $rc)"
  else
    echo "sitting $sitting: NOT whole (exit $rc)"
  fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
