#!/bin/sh
# E only: perf attached from ycsb-E's start to its end, the scan timers off
# (no SUPDB_MIX_COUNTERS), the form events on.
P=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/prof-e2
BIN=/home/user/supdb/bench/target/release/bench
PERF=/usr/lib/linux-tools-6.8.0-31/perf
T=$P/tmp
mkdir -p "$T"
export TMPDIR="$T"
A=${1:?arm a}
B=${2:?arm b}
REPS=${3:-2}
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$P/head.txt"
env -u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_CENSUS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX \
  SUPDB_MIX_EVENTS=1 SUPDB_LAG_MARK=1 \
  "$BIN" ab --arms "$A,$B" --size 5000000 --bound 671088640 --ops-div 10 --reps "$REPS" \
  --passes read,read-threaded,scan,scan-threaded,mixes --pairs ycsb > "$P/run.out" 2> "$P/run.err" &
PID=$!
echo "bench pid $PID"
i=0
PERFPID=
tail -n +1 -F "$P/run.err" --pid=$PID 2>/dev/null | while IFS= read -r line; do
  case "$line" in
    *"phasemark "*" ycsb-D start "*)
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
