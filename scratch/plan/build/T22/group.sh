#!/bin/bash
# usage: group.sh TAG N [stop-at-first-red]; runs `sh scripts/check.sh cross-arm` N times
# with RUST_BACKTRACE=1 in the caller's environment, as this shell has it.
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
tag=$1; n=$2; stop=${3:-0}; mkdir -p $D/runs/$tag; sum=$D/runs/$tag/summary.log; : > $sum
echo "start $(date -u +%FT%TZ) check.sh sha=$(sha256sum /home/user/supdb/scripts/check.sh|cut -c1-12)" >> $sum
cd /home/user/supdb
for i in $(seq 1 $n); do
  t=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T22/$tag-$i; rm -rf "$t"; mkdir -p "$t"
  TMPDIR=$t RUST_BACKTRACE=1 sh scripts/check.sh cross-arm > $D/runs/$tag/run$i.log 2>&1
  rc=$?
  echo "run $i exit $rc segv $(grep -c 'QEMU internal SIGSEGV' $D/runs/$tag/run$i.log) $(grep '^test result' $D/runs/$tag/run$i.log | tail -1) $(grep -c 'all checks passed: cross-arm' $D/runs/$tag/run$i.log)" >> $sum
  rm -rf "$t"
  [ "$stop" = 1 ] && [ $rc -ne 0 ] && break
done
echo "EXIT done $(date -u +%FT%TZ)" >> $sum
