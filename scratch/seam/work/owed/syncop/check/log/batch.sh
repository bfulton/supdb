#!/bin/sh
# one at a time, niced, each bounded by timeout 600 and the driver's secs
cd "$(dirname "$0")"
while read -r line; do
  case "$line" in ''|\#*) continue;; esac
  echo "### $line"
  nice -n 10 timeout 600 python3 $line 2>&1 | grep -A4 "^sequences\|^==\|Traceback\|Error"
done < "$1"
