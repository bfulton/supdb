# usage: fmut.sh <name> <runs> <source> <edit>... ; restores tests/fuzz.rs from keep/ by its sum
M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
n=$1; runs=$2; src=$3; shift 3
L=$M/r2/fmut-$n.log; : > $L
cd /home/user/supdb
python3 $M/r2/fmut.py $src "$@" >> $L 2>&1 || { echo APPLY-FAILED >> $L; cp $M/keep/tests/fuzz.rs tests/fuzz.rs; exit 9; }
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
if cargo test --profile checked --test fuzz --no-run >> $L 2>&1; then
  red=0; i=0
  while [ $i -lt $runs ]; do i=$((i+1))
    ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 cargo test --profile checked --test fuzz concurrent_range -- --exact --nocapture >> $L 2>&1 || red=$((red+1))
  done
  echo "VERDICT $n: red $red of $runs" >> $L
else echo "VERDICT $n: BUILD-FAILED" >> $L; fi
cp $M/keep/tests/fuzz.rs tests/fuzz.rs
grep -q "$(sha256sum tests/fuzz.rs | cut -c1-64)" $M/keep/SHA256SUMS && echo RESTORED >> $L
