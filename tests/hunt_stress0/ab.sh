#!/bin/sh
# Alternate two binaries over the same knobs, process by process, so that the
# host's load falls on both alike.
#   usage: ab.sh BIN_A BIN_B OUTDIR RUNS PARALLEL LABEL
# Each worker runs A then B for even numbers and B then A for odd ones. The
# ids carry the arm: LABEL-A-n and LABEL-B-n. Prints one summary line per arm.
BIN_A=$1
BIN_B=$2
OUT=$3
RUNS=$4
PAR=$5
LABEL=${6:-ab}
HERE=$(dirname "$0")
mkdir -p "$OUT/log" "$OUT/tmp"
RES="$OUT/results-$LABEL.txt"
: >"$RES"
seq 0 $((RUNS - 1)) | LABEL="$LABEL" HERE="$HERE" BIN_A="$BIN_A" BIN_B="$BIN_B" OUT="$OUT" xargs -P "$PAR" -I{} sh -c '
  if [ "${SEEDS:-0}" = 1 ]; then export HUNT_SEED=$(( ${SEEDBASE:-1000} + {} )); fi
  if [ $(( {} % 2 )) = 0 ]; then
    sh "$HERE/run_one.sh" "$BIN_A" "$OUT" "$LABEL-A-{}"
    sh "$HERE/run_one.sh" "$BIN_B" "$OUT" "$LABEL-B-{}"
  else
    sh "$HERE/run_one.sh" "$BIN_B" "$OUT" "$LABEL-B-{}"
    sh "$HERE/run_one.sh" "$BIN_A" "$OUT" "$LABEL-A-{}"
  fi
' >>"$RES"
for arm in A B; do
  n=$(grep -c " $LABEL-$arm-" "$RES")
  ok=$(grep "^ok " "$RES" | grep -c " $LABEL-$arm-")
  fail=$(grep -v "^ok " "$RES" | grep -c " $LABEL-$arm-")
  echo "AB $LABEL arm=$arm runs=$n ok=$ok failed=$fail"
done
