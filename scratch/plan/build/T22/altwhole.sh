#!/bin/bash
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
o=$D/runs/altwhole; mkdir -p $o; s=$o/summary.log; : > $s
t=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T22/altwhole
echo "start $(date -u +%FT%TZ)" >> $s
for r in $(seq 1 $1); do for m in 1 0; do
  rm -rf $t; mkdir -p $t
  TMPDIR=$t RUST_BACKTRACE=$m timeout 1500 qemu-aarch64-static -L /usr/aarch64-linux-gnu $D/supdb-base.bin > $o/bt$m-$r.log 2>&1
  echo "$r bt$m exit $? $(date -u +%T) $(grep '^test result' $o/bt$m-$r.log)" >> $s
done; done
rm -rf $t; echo "EXIT done $(date -u +%FT%TZ)" >> $s
