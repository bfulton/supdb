#!/bin/bash
# The timed runs of E5: every shape at every size, REPS times, each run a
# process of its own on fresh files, the shapes' order rotated each rep.
# Usage: run.sh OUT REPS [sizes...]
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit
OUT=$1; REPS=$2; shift 2
SIZES=${@:-100 4096 13300 133000}
T=$W/tmp
SHAPES=(A Aw A0 Ap B Cf Cp D)
mkdir -p $W/raw/lat
for rep in $(seq 1 $REPS); do
  for n in $SIZES; do
    k=$(( (rep - 1) % ${#SHAPES[@]} ))
    for j in $(seq 0 $(( ${#SHAPES[@]} - 1 ))); do
      s=${SHAPES[$(( (j + k) % ${#SHAPES[@]} ))]}
      c=1000
      E5_DUMP=$W/raw/lat/$s-$n-r$rep.lat $W/e5 $s $n $c $T r$rep >> $OUT || echo "FAIL $s $n r$rep" >> $OUT
    done
  done
done
