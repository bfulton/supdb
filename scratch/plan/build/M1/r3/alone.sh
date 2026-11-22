M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
R=$M/r3/run.sh
export ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4
sh $R alone-count tests/fuzz.rs fuzz 4 1 concurrent_range --exact --nocapture -- count
sh $R alone-bound1 tests/fuzz.rs fuzz 6 1 concurrent_range --exact --nocapture -- bound1
sh $R alone-ask0 tests/fuzz.rs fuzz 6 1 concurrent_range --exact --nocapture -- ask0 count
echo ALLDONE
