#!/bin/sh
# r10-flake.sh <tag> <rounds> <par>: build the cipher test binary from the tree as it is, then run it whole
# <rounds> times, <par> processes at once, each with a TMPDIR of its own; one line per run with its verdict.
TAG=$1; R=$2; PAR=$3
P=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/open-cost
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/tmp/open-cost
LOG=$P/r10-flake-$TAG.log
cd /home/user/supdb
: > $LOG
cargo test --profile checked --test cipher --no-run > $P/r10-flake-$TAG-build.log 2>&1; b=$?
echo "build $b" >> $LOG
[ $b = 0 ] || { echo "exit build-failed" >> $LOG; exit 1; }
BIN=$(grep -o 'target/checked/deps/cipher-[0-9a-f]*' $P/r10-flake-$TAG-build.log | tail -1)
echo "bin $BIN $(sha256sum $BIN | cut -c1-16)" >> $LOG
i=0
while [ $i -lt $R ]; do
  j=0
  while [ $j -lt $PAR ]; do
    d=$T/f-$TAG-$i-$j; mkdir -p $d
    ( TMPDIR=$d ./$BIN > $P/r10-flake-$TAG-$i-$j.out 2>&1; s=$?
      f=$(grep -c 'the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit \.\.\. FAILED' $P/r10-flake-$TAG-$i-$j.out)
      echo "run $i $j status $s target_failed $f $(grep 'test result:' $P/r10-flake-$TAG-$i-$j.out)" >> $LOG
      rm -rf $d ) &
    j=$((j+1))
  done
  wait
  i=$((i+1))
done
echo "exit done" >> $LOG
