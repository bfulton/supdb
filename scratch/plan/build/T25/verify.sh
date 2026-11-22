#!/bin/sh
cd /home/user/supdb
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T25
mkdir -p $T; export TMPDIR=$T
fail=0
run() { echo "== $*"; "$@"; st=$?; echo "-- exit $st"; [ $st -ne 0 ] && fail=1; }
run cargo test --profile checked --test cipher
run env SUPDB_TEST_STRIDE=1 cargo test --profile checked --test cipher
i=0; while [ $i -lt 6 ]; do i=$((i+1)); run cargo test --profile checked --test cipher a_form_build_holds_its_cache_and_not_its_partition -- --exact; done
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T25
mkdir -p $T
run sh scripts/check.sh lint
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T25
echo "FAIL=$fail"
echo EXIT $fail
