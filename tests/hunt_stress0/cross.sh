#!/bin/sh
# The 2x2 of the two scratch fixes against the two forced schedules, N runs of
# each cell.   usage: BINDIR=dir cross.sh N
# BINDIR holds x<arm>-hunt_stress0 and x<arm>-hunt_stress0_oracle for the arms
# none, b, c and bc: the test binaries built from the hunt branch's src/db.rs
# with apply_fix_b.py, apply_fix_c.py, both or neither applied to a copy of it.
N=${1:-10}
D=${BINDIR:?BINDIR names the directory of the binaries of the four arms}
export TMPDIR=${TMPDIR:-/tmp}/hunt-cross
mkdir -p "$TMPDIR"
for arm in none b c bc; do
  # the forms schedule: the publisher parked between its swaps and its position
  parked=0
  wrong=0
  i=0
  while [ "$i" -lt "$N" ]; do
    out=$("$D/x$arm-hunt_stress0" hunt_forced --exact --nocapture --test-threads=1 2>&1)
    if echo "$out" | grep -q 'publisher parked true'; then
      parked=$((parked + 1))
    fi
    if echo "$out" | grep -q 'wrong-while-parked Some'; then
      wrong=$((wrong + 1))
    fi
    i=$((i + 1))
  done
  echo "arm=$arm forms-schedule runs=$N parked=$parked wrong-answer=$wrong"
  for v in assert silent; do
    right=0
    assertion=0
    wronga=0
    other=0
    i=0
    while [ "$i" -lt "$N" ]; do
      out=$(HUNT_CUT_VARIANT=$v "$D/x$arm-hunt_stress0_oracle" hunt_cut_forced --exact --nocapture --test-threads=1 2>&1)
      if echo "$out" | grep -q 'scan after the rewrite is right'; then
        right=$((right + 1))
      elif echo "$out" | grep -q 'engine panicked'; then
        assertion=$((assertion + 1))
      elif echo "$out" | grep -q 'scan after the rewrite is wrong'; then
        wronga=$((wronga + 1))
      else
        other=$((other + 1))
      fi
      i=$((i + 1))
    done
    echo "arm=$arm cut-schedule variant=$v runs=$N right=$right assertion=$assertion wrong-answer=$wronga other=$other"
  done
done
rm -rf "$TMPDIR"
