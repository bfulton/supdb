cd /home/user/supdb
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T22c
L=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22/commit
date > $L/start
sh scripts/check.sh build test lint > $L/btl.log 2>&1; echo "EXIT $?" >> $L/btl.log
SUPDB_BASE=bc9cdd8472591d6343a74e467dc79827724d404e sh scripts/check.sh lists > $L/lists.log 2>&1; echo "EXIT $?" >> $L/lists.log
for i in 1 2 3; do sh scripts/check.sh cross-arm > $L/arm$i.log 2>&1; echo "EXIT $?" >> $L/arm$i.log; done
echo "EXIT done" > $L/gate.done
