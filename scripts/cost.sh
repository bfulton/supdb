#!/bin/sh
# What a store in the clear pays an operation, counted by function: the
# bench probes built at BASE and at this tree, each run under callgrind
# with the toggle CLAUDE.md names, and every function's own instructions
# printed side by side (scripts/cost-diff.py). The probes' sources are the
# head's in both builds, so the two differ in the engine alone.
#
#   sh scripts/cost.sh BASE [probe ...]
#
# Exits 1 where a hot function gains past its slack on any probe, or where
# a probe's toggle collected nothing; 2 where BASE names no commit, a build
# or a probe failed, or the suite's code a toggle counts differs between the
# two sides. The verdict line is the last line: `cost: green` or `cost: RED`.
# What is hot and its slack are scripts/cost-diff.py's, the one definition:
# in a probe with a toggle, every function built from source that the
# toggle collected, held to a few instructions over the whole run, and
# libc's and the allocator's to a share of their own count; in a probe with
# none, every function of the engine, the suite and the probe, held to a
# hundredth of an instruction an operation; in both, every line of
# scripts/cost-hot.txt; and a function a `~` line of that list names, to
# the bound the line gives it.
#
# The environment:
#   COST_DIR          the working directory (default $TMPDIR/supdb-cost-PID),
#                     emptied at the start; the outputs stay in $COST_DIR/out
#   COST_HEAD         the tree counted as the head (default this one): a
#                     copy of it with an edit is how scripts/cost-red.sh
#                     makes its red input
#   COST_HEAD_TARGET  the head's cargo target (default $COST_HEAD/bench/target)
#   COST_BASE_TARGET  the base's (default $COST_DIR/tb, removed at the end)
#
# Each side is built and then counted before the other is built, so the two
# may share one target directory: a red copy built where the base was costs
# the engine's and the suite's crates, not a second RocksDB. The base's
# worktree and its target are removed at the end: a third of a gigabyte a
# binary.
set -eu
BASE=${1:?usage: cost.sh BASE [probe ...]}
shift
ROOT=$(git rev-parse --show-toplevel)
HEAD_TREE=${COST_HEAD:-$ROOT}
HT=${COST_HEAD_TARGET:-$HEAD_TREE/bench/target}
W=${COST_DIR:-${TMPDIR:-/tmp}/supdb-cost-$$}
TB=${COST_BASE_TARGET:-$W/tb}
PROBES=${*:-read read_z scan scan_long scan_z scan_long_z load_durable load_buffered commit_durable commit_buffered work merge land seal scan_at}
WHOLE=0
[ $# -ne 0 ] || WHOLE=1
GET='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get'
SCAN='supdb_bench::engines::supdb_range'
WB='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::write_batch'
probe() { # name -> "ops|toggle|binary|args"; the toggle one argument, spaces and all
    case $1 in
    read) echo "200000|$GET|read_only|supdb 100000 1 n-none" ;;
    read_z) echo "200000|$GET|read_only|supdb-nocachezpages 100000 1 n-none" ;;
    scan) echo "1000|$SCAN|lag0_probe|supdb 100000 1" ;;
    scan_long) echo "1000|$SCAN|lag0_probe|supdb 100000 1 2000" ;;
    scan_z) echo "1000|$SCAN|lag0_probe|supdb-nocachezpages 100000 1" ;;
    scan_long_z) echo "1000|$SCAN|lag0_probe|supdb-nocachezpages 100000 1 2000" ;;
    load_durable) echo "100000||load_only|supdb 100000 1" ;;
    load_buffered) echo "100000||load_only|supdb-ingest 100000 1" ;;
    commit_durable) echo "100000|$WB|load_only|supdb 100000 1" ;;
    commit_buffered) echo "100000|$WB|load_only|supdb-ingest 100000 1" ;;
    work) echo "300000||work_probe|100000 whole" ;;
    merge) echo "200000||merge_walk|100000 4 1" ;;
    land) echo "100000|land_probe::rounds|land_probe|50000 8 4" ;;
    seal) echo "100000|seal_probe::seal|seal_probe|25000 4" ;;
    scan_at) echo "1000|scan_at_probe::scans|scan_at_probe|100000 1000 100" ;;
    *) echo "cost: no probe $1" >&2; return 2 ;;
    esac
}
# Every probe named before anything is built, and the binaries they run.
EX=""
for p in $PROBES; do
    spec=$(probe "$p") || exit 2
    bin=$(echo "$spec" | cut -d'|' -f3)
    case " $EX " in *" $bin "*) ;; *) EX="$EX $bin" ;; esac
done
if ! git -C "$ROOT" rev-parse --verify --quiet "$BASE^{commit}" >/dev/null; then
    echo "cost: BASE '$BASE' names no commit of this repository; nothing built"
    exit 2
fi
rm -rf "$W"
mkdir -p "$W/out" || { echo "cost: cannot make $W/out"; exit 2; }
cleanup() {
    git -C "$ROOT" worktree remove --force "$W/base" 2>/dev/null || true
    rm -rf "$W/tb" "$W/base"
}
trap cleanup EXIT
if ! git -C "$ROOT" worktree add --detach "$W/base" "$BASE" >"$W/out/worktree.log" 2>&1; then
    echo "cost: the worktree of BASE '$BASE' could not be made; see $W/out/worktree.log"
    exit 2
fi
# The suite's code a toggle counts through, the same text on both sides: a
# probe's toggle names the suite's function (`supdb_get`, `supdb_range`), the
# engine's read is inlined into it, and the suite's sources are each side's
# own -- only the probes' are copied, since the base's engine cannot build
# the head's suite -- so a step the head's suite took out would net a cost
# the head's engine put in. Compared, not copied, and checked only where a
# probe that counts through them runs.
SUITE=""
for p in $PROBES; do
    case $p in read | read_z | scan | scan_long | scan_z | scan_long_z | commit_durable | commit_buffered) SUITE=1 ;; esac
done
if [ -n "$SUITE" ] && ! python3 - "$W/base/bench/src/engines.rs" "$HEAD_TREE/bench/src/engines.rs" <<'PY'
import sys
# The suite's per-operation items: a toggle starts in one of them, or they
# are inlined into one. Each from its first line to its closing brace.
ITEMS = ["static SINK:", "fn consume(", "fn supdb_get(", "fn supdb_range(",
         "impl Engine for Supdb {/fn get(", "impl Engine for Supdb {/fn range(",
         "impl Engine for Supdb {/fn write_batch("]
def item(text, spec):
    lines = text.split("\n")
    start = 0
    *outer, name = spec.split("/")
    for o in outer:
        start = next((i for i in range(start, len(lines)) if o in lines[i]), None)
        if start is None:
            return None
    at = next((i for i in range(start, len(lines)) if name in lines[i]), None)
    if at is None:
        return None
    while at > 0 and lines[at - 1].strip().startswith("#["):
        at -= 1
    depth, out = 0, []
    for line in lines[at:]:
        out.append(line)
        depth += line.count("{") - line.count("}")
        if depth <= 0 and not line.strip().startswith("#[") and ("{" in "".join(out) or line.rstrip().endswith(";")):
            return "\n".join(out)
    return None
base, head = (open(p).read() for p in sys.argv[1:3])
bad = 0
for spec in ITEMS:
    b, h = item(base, spec), item(head, spec)
    if b is None or h is None:
        print(f"cost: the suite's `{spec}` is not in bench/src/engines.rs on the {'base' if b is None else 'head'} side")
        bad = 1
    elif b != h:
        print(f"cost: the suite's `{spec}` differs between the base and the head: a count through it would be the suite's and the engine's together")
        bad = 1
sys.exit(bad)
PY
then
    echo "cost: the two sides do not run one suite; nothing counted"
    exit 2
fi
# The head's probe sources in the base, so the two builds run one probe.
for e in $EX; do
    cp "$HEAD_TREE/bench/examples/$e.rs" "$W/base/bench/examples/$e.rs" ||
        { echo "cost: the head's probe $e could not be copied into the base"; exit 2; }
done
build() { # tree target side
    args=""
    for e in $EX; do args="$args --example $e"; done
    # shellcheck disable=SC2086
    if ! (cd "$1/bench" && CARGO_TARGET_DIR="$2" cargo build --release $args) >"$W/out/build-$3.log" 2>&1; then
        echo "cost: the $3 build failed; see $W/out/build-$3.log"
        exit 2
    fi
}
run() { # side target-dir name
    side=$1 dir=$2 name=$3
    set --
    spec=$(probe "$name")
    tog=$(echo "$spec" | cut -d'|' -f2)
    bin=$(echo "$spec" | cut -d'|' -f3)
    args=$(echo "$spec" | cut -d'|' -f4)
    t="$W/tmp-$side-$name"
    mkdir -p "$t"
    if [ -n "$tog" ]; then
        set -- "$@" --collect-atstart=no "--toggle-collect=$tog"
    fi
    # shellcheck disable=SC2086
    if ! TMPDIR="$t" valgrind --tool=callgrind --callgrind-out-file="$W/out/$side.$name.out" "$@" \
        "$dir/release/examples/$bin" $args >"$W/out/$side.$name.stdout" 2>"$W/out/$side.$name.vg"; then
        echo "cost: $side $name exited non-zero; see $W/out/$side.$name.vg"
        exit 2
    fi
    rm -rf "$t"
    callgrind_annotate --auto=no --inclusive=no --threshold=100 "$W/out/$side.$name.out" >"$W/out/$side.$name.self"
}
side() { # tree target side: build it, then count every probe on it
    build "$1" "$2" "$3"
    for p in $PROBES; do
        run "$3" "$2" "$p"
    done
}
side "$W/base" "$TB" base
side "$HEAD_TREE" "$HT" head
status=0
: >"$W/out/seen"
for p in $PROBES; do
    spec=$(probe "$p")
    ops=$(echo "$spec" | cut -d'|' -f1)
    # A probe with a toggle is counted as the operation alone, one without
    # as the whole program (scripts/cost-diff.py); the engine's, the suite's
    # and the probe's own crate's names are each one's.
    mode=whole
    [ -z "$(echo "$spec" | cut -d'|' -f2)" ] || mode=toggle
    own="supdb::,supdb_bench::,$(echo "$spec" | cut -d'|' -f3)::"
    SEEN="$W/out/seen" python3 "$ROOT/scripts/cost-diff.py" "$W/out/base.$p.self" "$W/out/head.$p.self" \
        "$ops" "$ROOT/scripts/cost-hot.txt" "$p" "$mode" "$own" || status=1
done
# The hot lines no probe of a whole run reached: inlined into their callers
# on both sides today, or gone. Named, not failed: a probe that matched no
# line at all has failed above. A run of named probes reaches what those
# probes run, so it names nothing.
[ "$WHOLE" = 0 ] || python3 - "$ROOT/scripts/cost-hot.txt" "$W/out/seen" <<'PY'
import sys
sys.dont_write_bytecode = True
sys.path.insert(0, sys.argv[1].rsplit("/", 1)[0])
canon = __import__("importlib").import_module("cost-diff").canon
lines = [l.strip() for l in open(sys.argv[1]) if l.strip() and l[0] not in "#!~"]
seen = {l.strip() for l in open(sys.argv[2])}
for l in lines:
    if (l if l.endswith("::") else canon(l)) not in seen:
        print(f"== hot line reached by no probe of this run: {l}")
PY
if [ "$status" -ne 0 ]; then
    echo "cost: RED"
    exit 1
fi
echo "cost: green"
