#!/bin/sh
# The noise floor queue (run sheet 4.4): supdb against itself, one pairing at a
# time, RESUME=1 so a whole table is kept. Stops at the first failure.
set -u
cd /home/user/c0/bench
export OUT=/home/user/base/pairs RESUME=1
log=/home/user/base/logs/floor.log
run() { # NAME SIZE args...
  name=$1; size=$2; shift 2
  if grep -q ' quantities: ' "$OUT/$name-$size-1.out" 2>/dev/null && grep -q ' quantities: ' "$OUT/$name-$size-2.out" 2>/dev/null; then
    echo "$(date -u +%FT%TZ) skip $name-$size (whole)" >> $log; return 0; fi
  T=/home/user/base/tmp/$name-$size; mkdir -p "$T"; export TMPDIR=$(cd "$T" && pwd -P)
  echo "$(date -u +%FT%TZ) start $name-$size df $(df -B1 --output=avail "$TMPDIR" | tail -1)" >> $log
  NAME=$name sh scripts/pair.sh supdb supdb "$size" "$@" >> $log 2>&1; rc=$?
  echo "$(date -u +%FT%TZ) end $name-$size exit $rc" >> $log
  rm -rf "$TMPDIR"/supdb-ab-* "$TMPDIR"/supdb-next-*
  [ $rc -eq 0 ] || { echo "$(date -u +%FT%TZ) QUEUE STOPPED at $name-$size" >> $log; exit $rc; }
}
P1=load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag,recover
P2=load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag
for K in numbered ids; do for SIZE in 10000 30000 100000 300000 1000000 3000000; do
  run "floor-supdb-core-$K" $SIZE --reps 7 --keys "$K" --passes $P1 --pairs ' '
done; done
for B in 1073741824 1342177280; do for K in numbered ids; do for SIZE in 3000000 10000000; do
  run "floor-supdb-b$((B >> 20))m-$K" $SIZE --bound "$B" --ops-div 10 --reps 3 --keys "$K" --passes $P2 --pairs ' '
done; done; done
echo "$(date -u +%FT%TZ) QUEUE DONE" >> $log
