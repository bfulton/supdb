#!/bin/sh
cd /home/user/supdb
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
mkdir -p $T; export TMPDIR=$T
i=0; while [ $i -lt 6 ]; do i=$((i+1)); cargo test --profile checked --lib a_view_says_what_is_in_memory_and_drops_and_asks_for_what_stands_behind_it 2>&1 | grep 'test result\|FAILED\|left:\|right:'; echo "ALONE $i exit $?"; done
sh scripts/check.sh test stride lint; echo "EXIT1 $?"
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T26g
echo "EXIT done"
