#!/bin/sh
# usage: loop.sh LOAD(0|2|4) N
S=/tmp/claude-0/-home-user-supdb/c073102e-5233-5744-bab5-0a479fff878c/scratchpad
LOAD=$1; N=$2
cd /home/user/supdb
pids=""
i=0; while [ $i -lt $LOAD ]; do sh -c 'while :; do :; done' & pids="$pids $!"; i=$((i+1)); done
trap 'kill $pids 2>/dev/null' EXIT INT TERM
r=1
while [ $r -le $N ] && [ ! -e $S/STOP ]; do
  export TMPDIR=$S/tmp
  rm -rf $TMPDIR/supdb-* 2>/dev/null
  log=$S/logs/load$LOAD-run$r.log
  mkdir -p $S/logs
  start=$(date -u +%FT%TZ)
  sh scripts/check.sh test stride > $log 2>&1; rc=$?
  end=$(date -u +%FT%TZ)
  fails=$(grep -E '^test .* FAILED$|^---- |panicked at|test result: FAILED' $log | grep -c .)
  echo "load=$LOAD run=$r start=$start end=$end rc=$rc fail_lines=$fails" >> $S/logs/summary.txt
  r=$((r+1))
done
