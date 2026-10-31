#!/bin/sh
# snapstress.sh N OUT: N runs of the windows test's four block-cache
# variants (`scans_with_the_block...`, tests/zpages.rs), whose snapshot half
# runs with the cache on now, each run a process of its own in a TMPDIR of
# its own and under a timeout; one verdict line a run into OUT. Hunt I read
# a Snapshot handle answering a post-snapshot write in about one run in two
# hundred with the cache on (scratch/README.md).
N=$1; OUT=$2
BIN=${BIN:-$(ls -t target/debug/deps/zpages-* | grep -v '\.d$' | head -1)}
ROOT=${STRESS_TMP:-/tmp/supdb-snapstress}
FILTER=${FILTER:-scans_with_the_block}
EXPECT=${EXPECT:-4}
: > "$OUT"
i=0
while [ "$i" -lt "$N" ]; do
  T=$ROOT/r$i
  rm -rf "$T"; mkdir -p "$T"
  TMPDIR="$T" timeout 900 "$BIN" "$FILTER" > "$T/log" 2>&1
  rc=$?
  if [ $rc -eq 0 ] && grep -q "test result: ok. $EXPECT passed" "$T/log"; then
    echo "run $i: PASS" >> "$OUT"
    rm -rf "$T"
  elif [ $rc -eq 124 ]; then
    echo "run $i: HANG" >> "$OUT"
  else
    first=$(grep -m1 -A2 -E 'panicked at|test result' "$T/log" | tr '\n' ' ' | cut -c1-300)
    echo "run $i: FAIL rc=$rc $first" >> "$OUT"
  fi
  i=$((i + 1))
done
echo "done" >> "$OUT"
