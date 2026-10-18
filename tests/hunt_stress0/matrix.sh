#!/bin/sh
# Run a list of variants one after another and append each summary to a file.
#   usage: matrix.sh BIN OUTDIR LISTFILE
# Each line of LISTFILE:  label runs parallel [VAR=value ...]
# (a '#' line is a comment). The variables are exported for that variant only.
BIN=$1
OUT=$2
LIST=$3
HERE=$(dirname "$0")
while read -r label runs par vars; do
  case "$label" in
    '' | '#'*) continue ;;
  esac
  (
    for kv in $vars; do
      export "$kv"
    done
    sh "$HERE/loop.sh" "$BIN" "$OUT" "$runs" "$par" "$label"
  ) | tee -a "$OUT/matrix-summary.txt"
done <"$LIST"
echo "MATRIX DONE" | tee -a "$OUT/matrix-summary.txt"
