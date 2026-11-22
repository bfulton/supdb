M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
R=$M/r3/run.sh
export ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4
sh $R fix-alone tests/fuzz.rs fuzz 6 1 concurrent_range --exact --nocapture --
sh $R fix-noasks tests/fuzz.rs fuzz 3 1 concurrent_range --exact --nocapture -- noasks
sh $R fix-load tests/fuzz.rs fuzz 3 6 concurrent_range --exact --nocapture --
sh $R mut-fa tests/fuzz.rs fuzz 2 1 concurrent_range --exact --nocapture -- fa
sh $R mut-fb tests/fuzz.rs fuzz 2 1 concurrent_range --exact --nocapture -- fb
sh $R mut-fc tests/fuzz.rs fuzz 2 1 concurrent_range --exact --nocapture -- fc
sh $R mut-fd src/db.rs fuzz 2 1 concurrent_range --exact --nocapture -- fd
echo ALLDONE
