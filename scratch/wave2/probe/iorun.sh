#!/bin/sh
# One sitting of two arms at 5M under 640 MiB with the io sampler beside it.
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad
T=$S/tmp-probe; mkdir -p $T; rm -rf $T/*
export TMPDIR=$T
TAG=$3
ERR=$S/logs/io-$TAG.err
BIN=/home/user/supdb/bench/target/release/bench
env -u SUPDB_LAG_COUNTERS SUPDB_LAG_MARK=1 SUPDB_MIX_KINDS=1 "$BIN" ab --arms "$1,$2" --size 5000000 --bound 671088640 --ops-div 10 --reps ${4:-3} \
  --passes read,read-threaded,scan,scan-threaded,mixes > $S/logs/io-$TAG.out 2> $ERR &
PID=$!
python3 -I $S/probe/iosample.py $PID $ERR $S/logs/io-$TAG.samples
wait $PID
echo "exit $?"
rm -rf $T/*
