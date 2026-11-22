#!/bin/sh
# The fuzz binary under every environment the gate runs it in, then twelve more runs of the
# concurrent range with conversions on, then the lint group. One thing at a time.
cd /home/user/supdb
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T23
mkdir -p $T; export TMPDIR=$T
fail=0
run() { echo "== $*"; "$@"; st=$?; echo "-- exit $st"; [ $st -ne 0 ] && fail=1; }
run cargo test --profile checked --test fuzz
run env SUPDB_TEST_STRIDE=1 cargo test --profile checked --test fuzz
run env ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 cargo test --profile checked --test fuzz fuzz_range -- --exact
run env ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 cargo test --profile checked --test fuzz concurrent_range -- --exact
run env ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 cargo test --profile checked --test fuzz fuzz_range -- --exact
i=0; while [ $i -lt 13 ]; do i=$((i+1)); run env ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 cargo test --profile checked --test fuzz concurrent_range -- --exact; done
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T23
mkdir -p $T
run sh scripts/check.sh lint
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T23
echo "FAIL=$fail"
echo EXIT $fail
