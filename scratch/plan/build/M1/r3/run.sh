# usage: run.sh <name> <file> <test binary name> <rounds> <copies> <test filter + args> -- <edit>...
# Applies the edits to keep/<file>, builds the test binary, runs <copies> at once <rounds> times,
# counts red runs, restores <file> from keep/ by its sum. Log M1/r3/<name>.log ends in VERDICT and RESTORED.
M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
n=$1; f=$2; bin=$3; rounds=$4; copies=$5; shift 5
args=""; while [ "$1" != "--" ]; do args="$args $1"; shift; done; shift
L=$M/r3/$n.log; : > $L
cd /home/user/supdb
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1
mkdir -p $TMPDIR
python3 $M/r3/tmut.py $f "$@" >> $L 2>&1 || { echo APPLY-FAILED >> $L; cp $M/keep/$f $f; exit 9; }
if cargo test --profile checked --test $bin --no-run >> $L 2>&1; then
  exe=$(grep -o "target/checked/deps/$bin-[0-9a-f]*" $L | tail -1)
  echo "BINARY $exe" >> $L
  red=0; tot=0; r=0
  while [ $r -lt $rounds ]; do r=$((r+1)); c=0
    while [ $c -lt $copies ]; do c=$((c+1))
      ( ./$exe $args > $M/r3/$n.r$r.c$c.out 2>&1; echo "RC $?" >> $M/r3/$n.r$r.c$c.out ) &
    done
    wait
    c=0; while [ $c -lt $copies ]; do c=$((c+1)); tot=$((tot+1))
      grep -q '^RC 0$' $M/r3/$n.r$r.c$c.out || red=$((red+1))
      grep -h 'ASKS\|panicked\|FAILED\|test result\|Err' $M/r3/$n.r$r.c$c.out | head -8 | sed "s/^/r$r.c$c: /" >> $L
    done
  done
  echo "VERDICT $n: red $red of $tot" >> $L
else echo "VERDICT $n: BUILD-FAILED" >> $L; fi
cp $M/keep/$f $f
grep -q "$(sha256sum $f | cut -c1-64)" $M/keep/SHA256SUMS && echo RESTORED >> $L
