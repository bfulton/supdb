#!/bin/sh
# G1-4 round 2, mutant 9 as a gate red input: a copy of this tree with
# `#[inline(never)]` taken off Blob::record_offset, counted by cost.sh's
# merge probe against the unit's base. Must exit 1 with `cost: RED` and
# the gain in compact_run. Then the same copy without the edit: exit 0,
# `cost: green` (the control, the same build path).
ROOT=/home/user/supdb
R=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r2/red9
BASE=869a4fee
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r2/red9/h
mkdir -p "$R/h"
(cd "$ROOT" && git ls-files -co --exclude-standard -- . ':!scratch' ':!bench/runs' ':!docs/figures' | tar -cf - -T -) | (cd "$R/h" && tar -xf -)
python3 - "$R/h/src/blob.rs" <<'PY' || { echo "edit failed"; exit 3; }
import sys
p = sys.argv[1]; s = open(p).read()
old = "    #[inline(never)]\n    pub fn record_offset(&self, rank: usize) -> Option<u64> {"
if s.count(old) != 1: sys.exit("count %d" % s.count(old))
open(p, "w").write(s.replace(old, "    pub fn record_offset(&self, rank: usize) -> Option<u64> {"))
PY
st=0
COST_DIR="$R/w" COST_HEAD="$R/h" COST_HEAD_TARGET="$R/t" COST_BASE_TARGET="$R/t" \
    sh "$ROOT/scripts/cost.sh" $BASE merge > "$R/mutant.log" 2>&1 || st=$?
last=$(tail -n 1 "$R/mutant.log")
echo "mutant: exit $st, last line: $last"
ok=1
[ "$st" = 1 ] && [ "$last" = "cost: RED" ] || ok=0
grep -n 'compact_run' "$R/mutant.log" | grep -i 'gain\|RED\|HOT' | head -5
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r2/red9/h
mkdir -p "$R/h"
(cd "$ROOT" && git ls-files -co --exclude-standard -- . ':!scratch' ':!bench/runs' ':!docs/figures' | tar -cf - -T -) | (cd "$R/h" && tar -xf -)
st2=0
COST_DIR="$R/w" COST_HEAD="$R/h" COST_HEAD_TARGET="$R/t" COST_BASE_TARGET="$R/t" \
    sh "$ROOT/scripts/cost.sh" $BASE merge > "$R/control.log" 2>&1 || st2=$?
last2=$(tail -n 1 "$R/control.log")
echo "control: exit $st2, last line: $last2"
[ "$st2" = 0 ] && [ "$last2" = "cost: green" ] || ok=0
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r2/red9/h
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r2/red9/t
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r2/red9/w/tb
if [ $ok = 1 ]; then echo "red9: ok"; echo "EXIT 0"; exit 0; fi
echo "red9: FAILED"; echo "EXIT 1"; exit 1
