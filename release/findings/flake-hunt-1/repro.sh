#!/bin/sh
# usage: repro.sh NAME BIN TEST ROUNDS N [ENV=VAL...] -- runs N copies of the test at once, ROUNDS times, beside a gate run
SP=/tmp/claude-0/-home-user-supdb/c073102e-5233-5744-bab5-0a479fff878c/scratchpad
NAME=$1; BIN=$2; T=$3; ROUNDS=$4; N=$5; shift 5
mkdir -p $SP/repro/$NAME
( cd /home/user/supdb && export TMPDIR=$SP/tmp && rm -rf $TMPDIR/supdb-*; sh scripts/check.sh test stride > $SP/repro/$NAME/gate.log 2>&1; echo "gate rc=$?" >> $SP/repro/$NAME/gate.log ) &
gate=$!
r=1
while [ $r -le $ROUNDS ]; do
  i=1
  while [ $i -le $N ]; do
    d=$SP/repro/$NAME/t$i; mkdir -p $d; rm -rf $d/supdb-*
    env TMPDIR=$d "$@" $SP/stalldump.sh $SP/repro/$NAME/r$r-$i 45 -- $BIN --exact $T &
    i=$((i+1))
  done
  wait_pids=$(jobs -p | grep -v "^$gate$")
  for p in $(jobs -p); do [ $p != $gate ] && wait $p; done
  for i in $(seq 1 $N); do printf "round $r inst $i: "; grep -E 'test result|watchdog|rc=' $SP/repro/$NAME/r$r-$i.log | tr '\n' ' '; echo; done >> $SP/repro/$NAME/summary.txt
  r=$((r+1))
done
echo "rounds done $(date -u +%T)" >> $SP/repro/$NAME/summary.txt
wait $gate
echo "gate done $(date -u +%T)" >> $SP/repro/$NAME/summary.txt
