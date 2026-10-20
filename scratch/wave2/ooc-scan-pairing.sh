#!/bin/sh
# Piece 8's out-of-core pairing: `supdb-nocache` against `supdb-nocachezpages`
# at ten million keys under a bound of one gibibyte, the load, the point
# reads on one thread and the class's threads, the scan passes and the mixes
# with the operations divided by ten, six pairs after the warmup, two
# sittings with the arms swapped. The read cells repeat the proof sitting's
# (`zproof-commands.sh`, section 2); the scan and mix cells are new, and are
# read with the faults a scan takes beside its rate. A third argument pair
# prices one of the scan's arms against the forced arm instead:
#   sh ooc-scan-pairing.sh heads     -> supdb-nocachezpages against supdb-nocachezpagesheads
#   sh ooc-scan-pairing.sh prefetch  -> supdb-nocachezpages against supdb-nocachezpagesprefetch
# Needs the memory controller writable and a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-ooc
WHAT=${1:-raw}
case "$WHAT" in
  raw) A=supdb-nocache; B=supdb-nocachezpages; PASSES=load,read,read-threaded,scan,scan-threaded,mixes ;;
  heads) A=supdb-nocachezpages; B=supdb-nocachezpagesheads; PASSES=load,scan,scan-threaded,mixes ;;
  prefetch) A=supdb-nocachezpages; B=supdb-nocachezpagesprefetch; PASSES=load,scan,scan-threaded,mixes ;;
  *) echo "unknown pairing $WHAT"; exit 2 ;;
esac
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/ooc-$WHAT-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/ooc-$WHAT-head.txt"
date -u +%FT%TZ >> "$Z/ooc-$WHAT-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET "$BIN" ab --arms "$arms" --size 10000000 --bound 1073741824 --ops-div 10 --reps 6 \
    --passes "$PASSES" --pairs 'scan' \
    > "$Z/ooc-$WHAT-ab-$sitting.out" 2> "$Z/ooc-$WHAT-ab-$sitting.err"
  rc=$?
  if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/ooc-$WHAT-ab-$sitting.out"; then
    echo "sitting $sitting: the table is whole (exit $rc)"
  else
    echo "sitting $sitting: NOT whole (exit $rc)"
  fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
