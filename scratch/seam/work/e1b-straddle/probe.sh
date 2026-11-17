#!/bin/sh
# probe.sh OPSDIV LABEL: one bench ab sitting (cipherauto then cipher), the two uprobes
# enabled during each arm's read phase only (SUPDB_LAG_MARK's phasemark lines), the
# ftrace ring dumped to LABEL.trace with an ARM marker before each window.
set -u
OPSDIV=$1; LABEL=$2
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e1b-straddle
T=/sys/kernel/tracing
OUT=$W/probe/$LABEL
B=/home/user/supdb/bench/target/release/bench
echo 0 > $T/events/probe_bench/enable
echo 1 > $T/tracing_on
echo 262144 > $T/buffer_size_kb
echo > $T/trace
cat $T/trace_pipe > $OUT.trace &
CAT=$!
SUPDB_LAG_MARK=1 TMPDIR=$W/tmp $B ab --arms supdb-cipherauto,supdb-cipher --size 5000000 \
  --bound 671088640 --passes read --reps 1 --ops-div "$OPSDIV" > $OUT.out 2> $OUT.err &
BENCH=$!
for i in 1 2; do
  until [ "$(grep -c 'phasemark .* read start' $OUT.err)" -ge $i ]; do
    kill -0 $BENCH 2>/dev/null || break 2
    sleep 0.02
  done
  arm=$(grep 'phasemark .* read start' $OUT.err | sed -n "${i}p" | awk '{print $2}')
  echo "ARM $arm" > $T/trace_marker
  echo 1 > $T/events/probe_bench/enable
  echo "enabled $arm $(date +%s.%N)" >> $OUT.log
  until [ "$(grep -c 'phasemark .* read end' $OUT.err)" -ge $i ]; do
    kill -0 $BENCH 2>/dev/null || break 2
    sleep 0.02
  done
  echo 0 > $T/events/probe_bench/enable
  echo "disabled $arm $(date +%s.%N)" >> $OUT.log
  echo "ARM post-$arm" > $T/trace_marker
done
echo 0 > $T/events/probe_bench/enable
wait $BENCH
echo "bench exit $?" >> $OUT.log
sleep 2
kill $CAT
for c in $(ls $T/per_cpu); do
  echo "$c $(grep -E 'overrun|dropped' $T/per_cpu/$c/stats | tr '\n' ' ')" >> $OUT.log
done
