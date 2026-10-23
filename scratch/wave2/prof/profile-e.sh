#!/bin/sh
# One sitting of two arms at five million keys under 640 MiB, the pairings'
# instruments on, with perf attached to the harness during each mixes pass
# (from ycsb-C's start to ycsb-E's end) and detached after it, so the loads
# and the read passes are not recorded. perf-<n>.data per mixes pass.
S=${SCRATCH:?the scratchpad this session writes to}
P=$S/prof
BIN=/home/user/supdb/bench/target/release/bench
PERF=/usr/lib/linux-tools-6.8.0-31/perf
T=$S/tmp-prof
mkdir -p "$T" "$P"
export TMPDIR="$T"
A=${1:-supdb-cache64tofitautozpages}
B=${2:-supdb-nocacheautozpages}
REPS=${3:-2}
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$P/head.txt"
date -u +%FT%TZ >> "$P/head.txt"
env -u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX \
  SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 \
  "$BIN" ab --arms "$A,$B" --size 5000000 --bound 671088640 --ops-div 10 --reps "$REPS" \
  --passes read,read-threaded,scan,scan-threaded,mixes --pairs ycsb > "$P/run.out" 2> "$P/run.err" &
PID=$!
echo "bench pid $PID"
i=0
PERFPID=
tail -n +1 -F "$P/run.err" --pid=$PID 2>/dev/null | while IFS= read -r line; do
  case "$line" in
    *"phasemark "*" ycsb-C start "*)
      i=$((i+1))
      $PERF record -p $PID -e cpu-clock -F 3000 -k CLOCK_MONOTONIC --call-graph dwarf,12288 -o "$P/perf-$i.data" > "$P/perf-$i.log" 2>&1 &
      PERFPID=$!
      echo "perf $i started for: $line" ;;
    *"phasemark "*" ycsb-E end "*)
      if [ -n "$PERFPID" ]; then
        sleep 0.2
        kill -INT $PERFPID
        wait $PERFPID
        echo "perf $i stopped at: $line"
        PERFPID=
      fi ;;
  esac
done
wait $PID
echo "bench exit $?"
rm -rf "$T"/*
echo "profile done $(date -u +%T)"
