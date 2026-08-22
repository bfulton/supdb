#!/bin/bash
# Simulated cache behaviour for the index layouts.
#
# Deterministic: the same binary and input give identical counts every run,
# which is what makes this gateable in CI where wall-clock numbers are not.
# The cache model is pinned rather than detected so the numbers mean the same
# thing on any host. See docs/profiling.md.
set -u
CG="valgrind --tool=cachegrind --cache-sim=yes --D1=32768,8,64 --LL=8388608,16,64 --cachegrind-out-file=/dev/null"
KEYS=${KEYS:-300000}
LOOKUPS=${LOOKUPS:-30000}
LAYOUTS=${LAYOUTS:-"heap-hash hash+flat hash+flatfixed hash+paged"}
BIN=./target/release/indexlab

command -v valgrind >/dev/null || { echo "valgrind not installed"; exit 1; }
[ -x "$BIN" ] || { echo "build first: cargo build --release --bin indexlab"; exit 1; }

rd() { sed -n "s/.*$1  *misses: *[0-9,]* *( *\([0-9,]*\) rd.*/\1/p" | tr -d ,; }
printf "%-16s %12s %12s\n" layout D1rd/lookup LLdrd/lookup
for L in $LAYOUTS; do
  # Subtract a build-only run: cachegrind attributes to the whole process.
  b=$($CG $BIN probe --layout "$L" --keys "$KEYS" --lookups 0 2>&1)
  f=$($CG $BIN probe --layout "$L" --keys "$KEYS" --lookups "$LOOKUPS" 2>&1)
  python3 -c "
d=$(echo "$f" | rd D1)-$(echo "$b" | rd D1)
l=$(echo "$f" | rd LLd)-$(echo "$b" | rd LLd)
print(f'{\"$L\":<16} {d/$LOOKUPS:12.2f} {l/$LOOKUPS:12.2f}')"
done
