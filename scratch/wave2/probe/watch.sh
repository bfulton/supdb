#!/bin/sh
# One sitting of two arms at 5M under 640 MiB; at each arm's read-pass end and
# E end, the page-cache residency of the store's files by kind.
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad
T=$S/tmp-probe; mkdir -p $T; rm -rf $T/*
export TMPDIR=$T
OUT=$S/logs/resident.err
BIN=/home/user/supdb/bench/target/release/bench
env -u SUPDB_LAG_COUNTERS SUPDB_LAG_MARK=1 SUPDB_MIX_HEAT=1 "$BIN" ab --arms "$1,$2" --size 5000000 --bound 671088640 --ops-div 10 --reps 1 \
  --passes read,read-threaded,scan,scan-threaded,mixes > $S/logs/resident.out 2> $OUT &
PID=$!
tail -n +1 -F $OUT --pid=$PID 2>/dev/null | while IFS= read -r line; do
  case "$line" in
    *"phasemark "*" read end "*|*"phasemark "*" ycsb-E end "*|*"phasemark "*" read start "*)
      tag=$(echo "$line" | awk '{print $2"/"$3"/"$4}')
      python3 -I $S/probe/resident.py $T "$tag" >> $S/logs/resident.txt 2>&1 ;;
  esac
done
wait $PID
echo "exit $?"
rm -rf $T/*
