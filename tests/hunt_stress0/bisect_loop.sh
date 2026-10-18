#!/bin/sh
# The minimal-form binaries of several commits, looped in turn: CYCLES rounds
# of PER processes of each binary, so the host's load is shared by all of them.
#   usage: bisect_loop.sh LISTFILE OUTDIR CYCLES PER PAR
# LISTFILE lines: LABEL BINARY ENV   (ENV is NAME=VALUE or - for none)
LIST=$1
OUT=$2
CYCLES=$3
PER=$4
PAR=$5
HERE=$(dirname "$0")
mkdir -p "$OUT"
: >"$OUT/bisect-summary.txt"
c=0
while [ "$c" -lt "$CYCLES" ]; do
  while read -r label bin envv; do
    case "$label" in
      \#* | "") continue ;;
    esac
    if [ "$envv" = "-" ]; then
      TESTNAME=hunt_scenario sh "$HERE/loop.sh" "$bin" "$OUT" "$PER" "$PAR" "$label-c$c" >>"$OUT/bisect-summary.txt"
    else
      env "$envv" TESTNAME=hunt_scenario sh "$HERE/loop.sh" "$bin" "$OUT" "$PER" "$PAR" "$label-c$c" >>"$OUT/bisect-summary.txt"
    fi
  done <"$LIST"
  c=$((c + 1))
done
echo "BISECT DONE" >>"$OUT/bisect-summary.txt"
