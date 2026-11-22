# usage: tmut.sh <name> <runs> <edit>... ; log M1/r2/tmut-<name>.log; tests/db.rs restored by its sum
M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
n=$1; runs=$2; shift 2
L=$M/r2/tmut-$n.log; : > $L
cd /home/user/supdb
python3 $M/r2/tmut.py "$@" >> $L 2>&1 || { echo APPLY-FAILED >> $L; cp $M/keep/tests/db.rs tests/db.rs; exit 9; }
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
if cargo test --profile checked --test db --no-run >> $L 2>&1; then
  red=0; i=0
  while [ $i -lt $runs ]; do i=$((i+1))
    cargo test --profile checked --test db -- --exact a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie >> $L 2>&1 || red=$((red+1))
  done
  echo "VERDICT $n: red $red of $runs" >> $L
else echo "VERDICT $n: BUILD-FAILED" >> $L; fi
cp $M/keep/tests/db.rs tests/db.rs
grep -q "$(sha256sum tests/db.rs | cut -c1-64)" $M/keep/SHA256SUMS && echo RESTORED >> $L
