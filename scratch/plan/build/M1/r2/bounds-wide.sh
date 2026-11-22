# usage: bounds-wide.sh <test source> <log> ; restores tests/db.rs from keep/ by its sum
M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
cd /home/user/supdb
python3 $M/r2/bounds-wide.py "$1" tests/db.rs || exit 9
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
cargo test --profile checked --test db --no-run > "$2" 2>&1 || { echo BUILD-FAILED >> "$2"; }
red=0
for i in 1 2 3 4 5; do
  cargo test --profile checked --test db -- --exact a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie >> "$2" 2>&1 || red=$((red+1))
done
echo "RED RUNS $red of 5" >> "$2"
cp $M/keep/tests/db.rs tests/db.rs
grep -q "$(sha256sum tests/db.rs | cut -c1-64)" $M/keep/SHA256SUMS && echo RESTORED >> "$2"
