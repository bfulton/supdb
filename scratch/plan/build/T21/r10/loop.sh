#!/bin/bash
# usage: loop.sh BIN OUTDIR ROUNDS PAR
BIN=$1; OUT=$2; R=$3; P=$4
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T21
export ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4
echo "START $(date -u +%T) $BIN"
for r in $(seq 1 $R); do
  for p in $(seq 1 $P); do
    ( "$BIN" concurrent_range --exact > $OUT.r$r.p$p 2>&1; echo "EXIT $?" >> $OUT.r$r.p$p ) &
  done; wait
done
echo "fails: $(grep -l 'EXIT [1-9]' $OUT.r*.p* | wc -l) of $((R*P))"
grep -h 'moved it in and out' $OUT.r*.p* | sed 's/.*moved/moved/' | sort | uniq -c
echo "EXIT 0 END $(date -u +%T)"
