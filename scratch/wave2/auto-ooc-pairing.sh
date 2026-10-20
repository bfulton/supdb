#!/bin/sh
# Stage 4's bounded measurement (the design's 7.4): the controller's arm
# (`supdb-nocacheautozpages`) against the forced arm
# (`supdb-nocachezpages`) at ten million keys under a bound of one
# gibibyte, the load, the point reads on one thread and the class's
# threads, the scan passes and the mixes with the operations divided by
# ten, six pairs after the warmup, two sittings with the arms swapped.
# The controller is expected to suspect within two ticks of the first
# read, pay the price in seconds, build and warm at raw rates and reach
# COMPACT, so the read cells are a staircase and the cells after the
# entry read against the forced arm's; the `zpage_*` rows carry its
# phases and the trace its decisions. A second shape prices the
# controller against the raw form at three million keys under the same
# bound, where the model must say FITS and nothing may be built:
#   sh auto-ooc-pairing.sh fits
# Needs the memory controller writable and a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-aooc
WHAT=${1:-forced}
case "$WHAT" in
  forced) A=supdb-nocachezpages; B=supdb-nocacheautozpages; SIZE=10000000 ;;
  fits) A=supdb-nocache; B=supdb-nocacheautozpages; SIZE=3000000 ;;
  *) echo "unknown pairing $WHAT"; exit 2 ;;
esac
PASSES=load,read,read-threaded,scan,scan-threaded,mixes
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/auto-ooc-$WHAT-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/auto-ooc-$WHAT-head.txt"
date -u +%FT%TZ >> "$Z/auto-ooc-$WHAT-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_ZPAGES_TRACE=1 "$BIN" ab --arms "$arms" --size "$SIZE" --bound 1073741824 --ops-div 10 --reps 6 \
    --passes "$PASSES" --pairs 'scan' \
    > "$Z/auto-ooc-$WHAT-ab-$sitting.out" 2> "$Z/auto-ooc-$WHAT-ab-$sitting.err"
  rc=$?
  if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/auto-ooc-$WHAT-ab-$sitting.out"; then
    echo "sitting $sitting: the table is whole (exit $rc)"
  else
    echo "sitting $sitting: NOT whole (exit $rc)"
  fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
