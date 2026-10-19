#!/bin/sh
# One process per seed, each in its own TMPDIR and under a timeout; one
# verdict line per seed into $OUT.
FROM=$1; TO=$2; OUT=$3; CACHE=${4:-all}
BIN=${BIN:-$(ls -t target/debug/deps/fuzz-* | grep -v "\.d$" | head -1)}
: > "$OUT"
seed=$FROM
while [ "$seed" -lt "$TO" ]; do
  T=${SWEEP_TMP:-/tmp/supdb-fuzz-sweep}/s$seed
  rm -rf "$T"; mkdir -p "$T"
  log="$T/log"
  TMPDIR="$T" ZFUZZ_SEEDS=$seed ZFUZZ_CACHE=$CACHE timeout 300 "$BIN" fuzz_range --nocapture > "$log" 2>&1
  rc=$?
  if [ $rc -eq 0 ]; then
    echo "seed $seed: PASS" >> "$OUT"
    rm -rf "$T"
  elif [ $rc -eq 124 ]; then
    echo "seed $seed: HANG" >> "$OUT"
  else
    first=$(grep -m1 -E 'CHECK FAILED|panicked at' "$log" | cut -c1-200)
    echo "seed $seed: FAIL rc=$rc $first" >> "$OUT"
  fi
  seed=$((seed + 1))
done
echo "done" >> "$OUT"
