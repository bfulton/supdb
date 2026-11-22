M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
B=a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie
R=$M/r3/run.sh
sh $R bt-new-eight tests/db.rs db 3 8 --exact $B --
sh $R bt-narrow-filldiff tests/db.rs db 2 1 --exact $B -- narrow filldiff
sh $R bt-new-filldiff tests/db.rs db 2 1 --exact $B -- filldiff
sh $R bt-r9n tests/db.rs db 3 1 --exact $B -- r9n
export ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4
sh $R mut-fe tests/fuzz.rs fuzz 2 1 concurrent_range --exact --nocapture -- noasks fe
echo ALLDONE
