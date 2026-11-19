#!/bin/sh
# What a store in the clear pays an operation, counted by function: the
# bench probes built at BASE and at this tree, each run under callgrind
# with the toggle CLAUDE.md names, and every function's own instructions
# printed side by side. The probes' sources are this tree's in both builds,
# so the two differ in the engine alone.
#
#   sh scripts/cost.sh BASE [probe ...]
#
# Exits non-zero where a function listed in scripts/cost-hot.txt -- the
# per-key, per-entry, per-block and per-step functions -- gains an
# instruction an operation past COST_SLACK (default 0.5) on any probe. The
# worktree and its build are removed at the end: a third of a gigabyte each.
set -eu
BASE=${1:?usage: cost.sh BASE [probe ...]}
shift
ROOT=$(git rev-parse --show-toplevel)
W=${COST_DIR:-${TMPDIR:-/tmp}/supdb-cost-$$}
SLACK=${COST_SLACK:-0.5}
PROBES=${*:-read scan load_durable load_buffered work merge}
rm -rf "$W"
mkdir -p "$W/out"
cleanup() {
    git -C "$ROOT" worktree remove --force "$W/base" 2>/dev/null || true
    rm -rf "$W/tb" "$W/base"
}
trap cleanup EXIT
git -C "$ROOT" worktree add --detach "$W/base" "$BASE" >/dev/null
EX="load_only work_probe read_only merge_walk lag0_probe"
for e in $EX; do
    cp "$ROOT/bench/examples/$e.rs" "$W/base/bench/examples/$e.rs"
done
build() {
    args=""
    for e in $EX; do args="$args --example $e"; done
    # shellcheck disable=SC2086
    (cd "$1/bench" && CARGO_TARGET_DIR="$2" cargo build --release $args) >"$W/out/build-$3.log" 2>&1 ||
        { echo "cost: the $3 build failed; see $W/out/build-$3.log"; exit 2; }
}
build "$W/base" "$W/tb" base
build "$ROOT" "$ROOT/bench/target" head
GET='--toggle-collect=<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get'
SCAN='--toggle-collect=supdb_bench::engines::supdb_range'
probe() { # name -> "ops toggle binary args"
    case $1 in
    read) echo "200000|--collect-atstart=no $GET|read_only|supdb 100000 1 n-none" ;;
    scan) echo "1000|--collect-atstart=no $SCAN|lag0_probe|supdb 100000 1" ;;
    load_durable) echo "100000||load_only|supdb 100000 1" ;;
    load_buffered) echo "100000||load_only|supdb-ingest 100000 1" ;;
    work) echo "300000||work_probe|100000 whole" ;;
    merge) echo "200000||merge_walk|100000 4 1" ;;
    *) echo "cost: no probe $1" >&2; exit 2 ;;
    esac
}
run() { # side target-dir name
    spec=$(probe "$3")
    tog=$(echo "$spec" | cut -d'|' -f2)
    bin=$(echo "$spec" | cut -d'|' -f3)
    args=$(echo "$spec" | cut -d'|' -f4)
    t="$W/tmp-$1-$3"
    mkdir -p "$t"
    # shellcheck disable=SC2086
    TMPDIR="$t" valgrind --tool=callgrind --callgrind-out-file="$W/out/$1.$3.out" $tog \
        "$2/release/examples/$bin" $args >"$W/out/$1.$3.stdout" 2>"$W/out/$1.$3.vg" ||
        { echo "cost: $1 $3 exited non-zero; see $W/out/$1.$3.vg"; exit 2; }
    rm -rf "$t"
    callgrind_annotate --auto=no --inclusive=no --threshold=100 "$W/out/$1.$3.out" >"$W/out/$1.$3.self"
}
status=0
for p in $PROBES; do
    run base "$W/tb" "$p"
    run head "$ROOT/bench/target" "$p"
    ops=$(probe "$p" | cut -d'|' -f1)
    python3 - "$W/out/base.$p.self" "$W/out/head.$p.self" "$ops" "$SLACK" "$ROOT/scripts/cost-hot.txt" "$p" <<'PY' || status=1
import re, sys
base, head, ops, slack, hot, name = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4]), sys.argv[5], sys.argv[6]
def read(path):
    out = {}
    for line in open(path):
        m = re.match(r"\s*([\d,]+)\s+\(\s*[\d.]+%\)\s+(.*)$", line) or re.match(r"\s*([\d,]+)\s+(\S.*)$", line)
        if not m or ":" not in m.group(2):
            continue
        fn = m.group(2).split(":", 1)[1].strip()
        fn = re.sub(r"\s+\[.*\]$", "", fn)
        out[fn] = out.get(fn, 0) + int(m.group(1).replace(",", ""))
    return out
b, h = read(base), read(head)
tb, th = sum(b.values()), sum(h.values())
hotfns = [l.strip() for l in open(hot) if l.strip() and not l.startswith("#")]
print(f"== {name}: total {tb:,} -> {th:,} ({(th - tb) / ops:+.2f} an op over {ops:,.0f})")
moved = sorted(((h.get(f, 0) - b.get(f, 0), f) for f in set(b) | set(h)), key=lambda x: -abs(x[0]))
for d, f in moved[:25]:
    if d:
        print(f"  {d / ops:+9.2f} an op  {b.get(f, 0):>14,} -> {h.get(f, 0):>14,}  {f}")
bad = []
for f in set(b) | set(h):
    if any(hf in f for hf in hotfns) and (h.get(f, 0) - b.get(f, 0)) / ops > slack:
        bad.append(f)
for f in sorted(bad):
    print(f"  HOT GAINED: {f} {(h.get(f, 0) - b.get(f, 0)) / ops:+.2f} an op")
sys.exit(1 if bad else 0)
PY
done
exit $status
