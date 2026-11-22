#!/bin/sh
# The whole db binary of upstream's head and of the merged tree, by turns, five each, the
# test TMPDIR emptied between runs; a line a run.
M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
UP=$M/upwt-target/checked/deps/db-7162ab4a5ae0d618
ME=/home/user/supdb/$(cd /home/user/supdb && ls -t target/checked/deps/db-* | grep -v '\.d$' | head -1)
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
for i in 1 2 3 4 5; do
  for side in up me; do
    if [ $side = up ]; then b=$UP; d=$M/upwt; else b=$ME; d=/home/user/supdb; fi
    (cd $d && $b > $M/alt-$side-$i.log 2>&1); rc=$?
    echo "$side $i rc $rc $(grep -h 'test result' $M/alt-$side-$i.log | cut -c1-50) $(grep -h '^test .* FAILED' $M/alt-$side-$i.log | tr '\n' ' ')"
    rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
    mkdir -p /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
  done
done
echo EXIT done
