#!/bin/sh
# One process of tests/hunt_stress0.rs under a TMPDIR of its own; prints one
# classified line.   usage: run_one.sh BIN OUTDIR ID
# The scenario's knobs are the HUNT_* variables of the caller.
BIN=$1
OUT=$2
ID=$3
T="$OUT/tmp/$ID"
mkdir -p "$T" "$OUT/log"
TMPDIR="$T" timeout 600 "$BIN" hunt_scenario --exact --nocapture --test-threads=1 >"$OUT/log/$ID.txt" 2>&1
RC=$?
rm -rf "$T"
LOG="$OUT/log/$ID.txt"
if [ "$RC" = 0 ] && grep -q 'HUNT-OK' "$LOG"; then
  echo "ok $ID"
  rm -f "$LOG"
  exit 0
fi
if grep -q 'HUNT-FAIL snapshot-later-write' "$LOG"; then
  echo "F1-later-write $ID"
elif grep -q 'HUNT-FAIL snapshot-mismatch' "$LOG"; then
  echo "F1-mismatch $ID"
elif grep -q 'HUNT-FAIL windows' "$LOG"; then
  echo "F0-windows $ID"
elif grep -q 'HUNT-FAIL writer-after' "$LOG"; then
  echo "F0-writer-after $ID"
elif grep -q "an overlay key's cut is behind the walk" "$LOG"; then
  echo "F2-cut-known rc=$RC $ID"
elif grep -q "the writer's upkeep is lent" "$LOG"; then
  echo "F2b-lent rc=$RC $ID"
else
  echo "other rc=$RC $ID"
fi
