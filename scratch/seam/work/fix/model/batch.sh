#!/bin/sh
# Runs each configuration in turn, one at a time, under nice, after the
# machine is free of bench runs.  Usage: sh batch.sh LISTFILE OUTFILE
# LISTFILE: one command per line (relative to this directory).
cd "$(dirname "$0")"
list=$1
out=$2
: > "$out.tmp"
while IFS= read -r cmd; do
    case "$cmd" in ''|'#'*) continue;; esac
    while pgrep -f 'bench ab|pair.sh' >/dev/null; do sleep 60; done
    echo "## $cmd" >> "$out.tmp"
    timeout 660 nice -n 19 $cmd >> "$out.tmp" 2>&1
    echo "## exit $?" >> "$out.tmp"
done < "$list"
mv "$out.tmp" "$out"
