#!/bin/sh
# Loop tests/hunt_stress0.rs over processes and read a rate.
#   usage: loop.sh BIN OUTDIR RUNS PARALLEL [LABEL]
# The scenario's knobs are the HUNT_* variables of the caller. With SEEDS=1
# each process gets HUNT_SEED=$SEEDBASE+its number (default: the original
# scenario's seeds, 0). Prints one summary line.
BIN=$1
OUT=$2
RUNS=$3
PAR=$4
LABEL=${5:-run}
HERE=$(dirname "$0")
mkdir -p "$OUT/log" "$OUT/tmp"
RES="$OUT/results-$LABEL.txt"
: >"$RES"
seq 0 $((RUNS - 1)) | LABEL="$LABEL" HERE="$HERE" BIN="$BIN" OUT="$OUT" xargs -P "$PAR" -I{} sh -c '
  if [ "${SEEDS:-0}" = 1 ]; then export HUNT_SEED=$(( ${SEEDBASE:-1000} + {} )); fi
  sh "$HERE/run_one.sh" "$BIN" "$OUT" "$LABEL-{}"
' >>"$RES"
ok=$(grep -c '^ok ' "$RES")
f1=$(grep -c '^F1' "$RES")
f0=$(grep -c '^F0' "$RES")
f2=$(grep -c '^F2' "$RES")
ot=$(grep -c '^other' "$RES")
echo "SUMMARY $LABEL runs=$RUNS ok=$ok F1=$f1 F0=$f0 F2=$f2 other=$ot"
