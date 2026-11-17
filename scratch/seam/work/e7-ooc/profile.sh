#!/bin/sh
# E7's profiling run (not for rates): one bounded ab pass of each arm, and a
# 20-second `perf record -e cpu-clock -g` of each arm's pass process taken
# 10 s into its read phase, found by the harness's phasemark line.
set -u
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e7-ooc
PERF=/usr/lib/linux-tools-6.8.0-31/perf
BIN=/home/user/supdb/bench/target/release/bench
ERR=$W/profile.err
: > "$ERR"
env -u GLIBC_TUNABLES SUPDB_LAG_MARK=1 TMPDIR=$W/tmp "$BIN" ab --arms supdb-cipher,supdb-nozpages \
  --size 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1 > $W/profile.out 2>> "$ERR" &
AB=$!
for arm in supdb-nozpages supdb-cipher; do
  until grep -q "phasemark $arm read start" "$ERR"; do
    kill -0 $AB 2>/dev/null || { echo "ab exited before $arm's read"; exit 1; }
    sleep 1
  done
  sleep 10
  PID=$(pgrep -f "one-pass .*/$arm-1-" | head -1)
  echo "$arm pid $PID at $(date -u +%T)"
  timeout -s INT 20 $PERF record -e cpu-clock -g -p "$PID" -o $W/perf-$arm.data > /dev/null 2>> $W/perf-$arm.log
done
wait $AB
echo "ab rc=$?"
