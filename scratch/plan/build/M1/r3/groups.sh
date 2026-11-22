cd /home/user/supdb
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
(cargo test --profile checked --no-fail-fast --test fuzz --test db -- --exact a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie > $M/r3-unit.log 2>&1; echo EXIT $? >> $M/r3-unit.log)
(cargo test --profile checked --no-fail-fast --test fuzz > $M/r3-fuzz.log 2>&1; echo EXIT $? >> $M/r3-fuzz.log)
(sh scripts/check.sh stride > $M/r3-stride.log 2>&1; echo EXIT $? >> $M/r3-stride.log)
(sh scripts/check.sh lint > $M/r3-lint.log 2>&1; echo EXIT $? >> $M/r3-lint.log)
(sh scripts/check.sh bench > $M/r3-bench.log 2>&1; echo EXIT $? >> $M/r3-bench.log)
(SUPDB_BASE=HEAD sh scripts/check.sh lists > $M/r3-lists.log 2>&1; echo EXIT $? >> $M/r3-lists.log)
echo ALLDONE
