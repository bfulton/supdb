#!/bin/bash
# The timed runs: every shape at every size, REPS times, each run a process of
# its own on fresh files, the shapes' order rotated each rep (and the rotation
# offset by the sitting, so three sittings of ten cover all twenty orders).
# Usage: run.sh OUT REPS SITTING [sizes...]
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/fix/probe
OUT=$1; REPS=$2; SIT=$3; shift 3
SIZES=${@:-100 4096 13300 133000}
T=$W/tmp
SHAPES=(A Aw A0 Ap0 B D Ow O0 Tw T0 Rw R0 Cf Cp Ds Dsp J Jp S Sp)
NS=${#SHAPES[@]}
mkdir -p $W/raw/lat $T
for rep in $(seq 1 $REPS); do
  for n in $SIZES; do
    k=$(( (rep - 1 + (SIT - 1) * REPS) % NS ))
    for j in $(seq 0 $(( NS - 1 ))); do
      s=${SHAPES[$(( (j + k) % NS ))]}
      if [ $rep -le 2 ]; then export E5_DUMP=$W/raw/lat/s$SIT-$s-$n-r$rep.lat; else unset E5_DUMP; fi
      $W/e5 $s $n 1000 $T s${SIT}r$rep >> $OUT || echo "FAIL $s $n s${SIT}r$rep" >> $OUT
    done
  done
done
