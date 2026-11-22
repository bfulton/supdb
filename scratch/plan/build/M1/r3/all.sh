M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
B=a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie
R=$M/r3/run.sh
sh $R r1-eight tests/db.rs db 3 8 --exact $B -- r1
sh $R r9-alone tests/db.rs db 3 1 --exact $B -- r9
export ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4
sh $R noasks-count tests/fuzz.rs fuzz 3 1 concurrent_range --exact --nocapture -- noasks count
sh $R noasks-ask0 tests/fuzz.rs fuzz 3 1 concurrent_range --exact --nocapture -- noasks ask0 count
sh $R noasks-bound1 tests/fuzz.rs fuzz 3 1 concurrent_range --exact --nocapture -- noasks bound1
sh $R load-bound1 tests/fuzz.rs fuzz 2 6 concurrent_range --exact --nocapture -- bound1
sh $R load-count tests/fuzz.rs fuzz 2 6 concurrent_range --exact --nocapture -- count
sh $R load-ask0 tests/fuzz.rs fuzz 2 6 concurrent_range --exact --nocapture -- ask0 count
echo ALLDONE
