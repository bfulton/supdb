M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
R=$M/r3/run.sh
export ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4
sh $R new-alone-count tests/fuzz.rs fuzz 6 1 concurrent_range --exact --nocapture -- count
sh $R new-noasks-count tests/fuzz.rs fuzz 3 1 concurrent_range --exact --nocapture -- noasks count
sh $R new-load-count tests/fuzz.rs fuzz 2 6 concurrent_range --exact --nocapture -- count
echo ALLDONE
