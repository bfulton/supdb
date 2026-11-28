#!/bin/sh
# A round's merged-tree run on the working branch: every gate group on its
# own (so one red group does not hide the verdicts after it), the cost count
# against the head before the round, and the fuzz past the gate's ranges
# under the cipher and in one file. Nothing else may run on the machine.
#
#   git worktree add DIR HEAD && (cd DIR && git switch -C roundN HEAD)
#   sh release/tools/round.sh DIR BASE LOGDIR
#
# Each step's exit and time go to LOGDIR/steps; each group's output to
# LOGDIR/gate-GROUP.log, the count's to LOGDIR/cost.log, the fuzz's to
# LOGDIR/fuzz-*.log. FASTER's sources are fetched, not tracked, so a new
# worktree takes them from the checkout that fetched them (ROOT).
wt=$1 base=$2 out=$3
ROOT=${ROOT:-$(git -C "$(dirname "$0")" rev-parse --show-toplevel)}
mkdir -p "$out" "$out/tmp"
export TMPDIR=$(cd "$out/tmp" && pwd -P)
cd "$wt" || exit 2
[ -f bench/vendor/faster/COMMIT ] ||
  cp -a "$ROOT/bench/vendor/faster/src" "$ROOT/bench/vendor/faster/COMMIT" \
    "$ROOT/bench/vendor/faster/LICENSE" bench/vendor/faster/
stamp() { date -u +%FT%TZ; }
clean() { rm -rf "$TMPDIR"/supdb-next-* "$TMPDIR"/supdb-ab-*; }
echo "gate start $(stamp)" > "$out/steps"
for grp in build test stride onefile lint wasm bench lists; do
  sh scripts/check.sh $grp > "$out/gate-$grp.log" 2>&1
  echo "gate $grp exit $? $(stamp)" >> "$out/steps"
  clean
done
echo "cost start $(stamp)" >> "$out/steps"
sh scripts/cost.sh "$base" > "$out/cost.log" 2>&1
echo "cost exit $? $(stamp)" >> "$out/steps"
clean
for v in ZFUZZ_CIPHER ZFUZZ_ONEFILE; do
  echo "fuzz $v start $(stamp)" >> "$out/steps"
  env $v=1 ZFUZZ_FROM=24 ZFUZZ_TO=524 cargo test --profile checked --test fuzz fuzz_range -- --exact --nocapture > "$out/fuzz-$v.log" 2>&1
  echo "fuzz $v exit $? $(stamp)" >> "$out/steps"
  clean
done
echo "done $(stamp)" >> "$out/steps"
