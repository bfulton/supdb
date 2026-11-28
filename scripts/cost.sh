#!/bin/sh
# What a store in the clear pays an operation, counted by function: the
# bench probes built at BASE and at this tree, each run under callgrind
# with the toggle CLAUDE.md names, and every function's own instructions
# printed side by side (scripts/cost-diff.py). The probes' sources and the
# suite's build definition (bench/.cargo/config.toml) are the head's in both
# builds, so the two differ in the engine alone.
#
# Both sides are built in one directory, /tmp/supdb-cost/src, into one
# target, /tmp/supdb-cost/t, one after the other: a worktree of BASE there,
# built, its probes copied out to /tmp/supdb-cost/bin/base; then the head's
# tree synced over it -- each file that differs written, each file of
# BASE's the head lacks removed, .git and the targets left alone -- built
# again, its probes copied out to /tmp/supdb-cost/bin/head; and each side
# counted from its copies, with its stores under /tmp/supdb-cost too. Built
# at two paths, the two sides differed in more than the engine: the path is
# in every crate's metadata hash and in the paths the binaries carry, so
# their layout moved with it. Counted against itself from two checkouts,
# b3a043dc read libc's compare in scan_s at 19 instructions a scan on one
# side and 23 on the other, and at one checkout's path valgrind read no
# symbols of scan_at_probe at all -- of sixteen probe binaries, the one
# whose RELRO segment ended on a page boundary -- so the toggle never fired
# and the probe read empty on every count. Built at one path under
# COST_DIR, the layout still followed COST_DIR: of one tree at two of them,
# valgrind read scan_at_probe's symbols at one and at the other none, and
# with the path padded to one length the two builds' segments were still
# sixteen bytes apart, since the crate's hash, and with it how its code is
# cut and laid out, follows what the path says and not only its length; the
# target's path is in the binaries as well (bench/src/env.rs includes a
# file the build writes). So the paths are fixed: one tree builds to one
# layout on any machine and for anyone who counts it, whose binaries
# valgrind reads or does not every time. One count at a time holds them,
# by a lock (/tmp/supdb-cost/lock) a second count is refused at; the target
# is kept from one count to the next, so RocksDB is built once, and
# `rm -rf /tmp/supdb-cost` takes it back.
#
#   sh scripts/cost.sh BASE [probe ...]
#
# Exits 1 where a hot function gains past its slack on any probe, or where
# a probe's toggle collected nothing, or valgrind read no symbols of a
# probe's binary (said so by name: it can name none of the binary's
# functions, so a toggle collects nothing and no hot line matches); 2 where
# BASE names no commit, a build or a probe failed, or the suite's code a
# toggle counts differs between the two sides. The verdict line is the last
# line: `cost: green` or `cost: RED`.
# What is hot and its slack are scripts/cost-diff.py's, the one definition:
# in a probe with a toggle, every function built from source that the
# toggle collected, held to a few instructions over the whole run, and
# libc's and the allocator's to a share of their own count; in a probe with
# none, every function of the engine, the suite and the probe, held to a
# hundredth of an instruction an operation; in both, every line of
# scripts/cost-hot.txt; and a function a `~` line of that list names, to
# the bound the line gives it.
#
# The environment, all of it that the script reads:
#   COST_DIR     the working directory (default $TMPDIR/supdb-cost-PID),
#                emptied at the start, where the outputs stay
#                ($COST_DIR/out)
#   COST_HEAD    the tree counted as the head (default this one), read and
#                never written: its files as git lists them, tracked and
#                untracked but not ignored, or every file but .git and the
#                targets where it is no git work tree. A copy of this tree
#                with an edit is how scripts/cost-red.sh makes its red input
# COST_TARGET, COST_BASE_TARGET and COST_HEAD_TARGET, the targets of the
# layouts before this one, are refused: the target's path is in the
# binaries, so it is fixed with the build directory's.
#
# The count is otherwise sensitive to the memory group's state: the compact
# form's controller reads the group's room and refaults on a clock, so on a
# box whose other work fills the group's page cache near its limit one side
# can be counted through the census and the asks out of core (in a count of
# b3a043dc against itself in #38's review, scan_long's base ran
# `zcensus_begin_slow`, `Blob::willneed_ranks` and `mincore` and its head
# did not, -390 an op on the same binaries). Each probe's run starts with
# the page cache dropped where /proc/sys/vm/drop_caches is writable, as
# bench/scripts/room.sh drops it; where it is not, the count says so and
# each side meets the group as the box leaves it. The drop is the whole
# machine's page cache, so nothing else may run beside a count -- no other
# check.sh, cargo test or test binary: a zpages test that reads what the
# cache holds failed under a count's drops. The counts of the store's own
# clock-driven work move with the machine's load as well
# (scripts/cost-hot.txt).
set -eu
BASE=${1:?usage: cost.sh BASE [probe ...]}
shift
ROOT=$(git rev-parse --show-toplevel)
HEAD_TREE=${COST_HEAD:-$ROOT}
W=${COST_DIR:-${TMPDIR:-/tmp}/supdb-cost-$$}
# The build's paths, fixed (the header says why).
BUILD=/tmp/supdb-cost
SRC=$BUILD/src
T=$BUILD/t
BIN=$BUILD/bin
if [ -n "${COST_TARGET:-}${COST_BASE_TARGET:-}${COST_HEAD_TARGET:-}" ]; then
    echo "cost: COST_TARGET, COST_BASE_TARGET and COST_HEAD_TARGET are gone: both sides build in $T, at a fixed path, since a target's path is in the binaries; nothing built"
    exit 2
fi
PROBES=${*:-read read_z read_s scan scan_long scan_z scan_long_z scan_s scan_long_s load_durable load_buffered commit_durable commit_buffered work merge land seal scan_at}
WHOLE=0
[ $# -ne 0 ] || WHOLE=1
GET='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get'
SCAN='supdb_bench::engines::supdb_range'
WB='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::write_batch'
probe() { # name -> "ops|toggle|binary|args"; the toggle one argument, spaces and all
    case $1 in
    read) echo "200000|$GET|read_only|supdb 100000 1 n-none" ;;
    read_z) echo "200000|$GET|read_only|supdb-nocachezpages 100000 1 n-none" ;;
    read_s) echo "200000|$GET|read_only|supdb-nocachezstride 100000 1 n-none" ;;
    scan) echo "1000|$SCAN|lag0_probe|supdb 100000 1" ;;
    scan_long) echo "1000|$SCAN|lag0_probe|supdb 100000 1 2000" ;;
    scan_z) echo "1000|$SCAN|lag0_probe|supdb-nocachezpages 100000 1" ;;
    scan_long_z) echo "1000|$SCAN|lag0_probe|supdb-nocachezpages 100000 1 2000" ;;
    scan_s) echo "1000|$SCAN|lag0_probe|supdb-nocachezstride 100000 1" ;;
    scan_long_s) echo "1000|$SCAN|lag0_probe|supdb-nocachezstride 100000 1 2000" ;;
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
mkdir -p "$T" || { echo "cost: cannot make $T"; exit 2; }
# One count at a time on the build's fixed paths: a second is refused, and
# the lock goes with this process.
exec 9>"$BUILD/lock"
if ! flock -n 9; then
    echo "cost: another count holds $BUILD/lock; nothing built"
    exit 2
fi
rm -rf "$W"
mkdir -p "$W/out" || { echo "cost: cannot make $W/out"; exit 2; }
# Absolute, since the builds and the listings run from the build directory.
W=$(cd "$W" && pwd -P)
cleanup() {
    git -C "$ROOT" worktree remove --force "$SRC" 2>/dev/null || true
    rm -rf "$SRC" "$BIN" "$BUILD"/tmp-*
}
# What a count killed before its end left on the fixed paths, its worktree
# among them.
cleanup
trap cleanup EXIT
if ! git -C "$ROOT" worktree add -f --detach "$SRC" "$BASE" >"$W/out/worktree.log" 2>&1; then
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
    case $p in read | read_z | read_s | scan | scan_long | scan_z | scan_long_z | scan_s | scan_long_s | commit_durable | commit_buffered) SUITE=1 ;; esac
done
if [ -n "$SUITE" ] && ! python3 - "$SRC/bench/src/engines.rs" "$HEAD_TREE/bench/src/engines.rs" <<'PY'
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
    cp "$HEAD_TREE/bench/examples/$e.rs" "$SRC/bench/examples/$e.rs" ||
        { echo "cost: the head's probe $e could not be copied into the base"; exit 2; }
done
# The head's build in the base, so the two sides are compiled alike:
# bench/.cargo/config.toml is the machine level and the code-generation
# policy every engine of the suite is built with, read from the directory
# cargo runs in, and a base from before it -- or from before a change to
# it -- would be counted at another level than the head, every function's
# instructions the flags' as well as the engine's. A head without the file
# builds the base without it too.
if [ -f "$HEAD_TREE/bench/.cargo/config.toml" ]; then
    mkdir -p "$SRC/bench/.cargo"
    cp "$HEAD_TREE/bench/.cargo/config.toml" "$SRC/bench/.cargo/config.toml" ||
        { echo "cost: the head's build definition could not be copied into the base"; exit 2; }
else
    rm -f "$SRC/bench/.cargo/config.toml"
fi
# FASTER's sources, for a suite that builds the `faster` arm
# (bench/build.rs): they are fetched and never the repository's
# (bench/scripts/faster.sh), so a worktree of the base comes without them,
# and the suite's build stops there. Handed over from this tree where it
# holds the commit the build directory pins, before each side's build; git
# ignores them, so the head's sync leaves them where its pin is the base's.
# A pin they do not fit fails its own build, which says what to run.
faster_for() { # tree
    v="$1/bench/vendor/faster"
    [ -f "$v/PIN" ] || return 0
    pin=$(tr -d '[:space:]' <"$v/PIN")
    if [ "$(cat "$v/COMMIT" 2>/dev/null || true)" = "$pin" ] && [ -f "$v/src/core/faster.h" ]; then return 0; fi
    have="$ROOT/bench/vendor/faster"
    if [ "$(cat "$have/COMMIT" 2>/dev/null || true)" != "$pin" ]; then return 0; fi
    rm -rf "$v/src" "$v/COMMIT"
    cp -R "$have/src" "$v/src" && cp "$have/COMMIT" "$v/COMMIT"
}
# The head's tree over the build directory: each file the head lists that
# differs from the build directory's, or that it lacks, written there, which
# gives it a time after the base's build and so has cargo build it again;
# each file BASE tracks that the head lists not, removed; a file the two
# hold alike left as it is, so what only BASE built stays built. The head's
# files are the ones git lists in it, tracked and untracked but not ignored,
# or, where it is no git work tree, every file but .git and the targets.
sync_head() { # head-tree build-directory
    python3 - "$1" "$2" >"$W/out/sync.log" <<'PY' || { echo "cost: the head's tree could not be synced into $2; see $W/out/sync.log"; exit 2; }
import filecmp, os, shutil, subprocess, sys

head, dest = sys.argv[1:3]


def git_files(tree):
    """The files git lists in `tree` where it is the top of a work tree."""
    top = subprocess.run(["git", "-C", tree, "rev-parse", "--show-toplevel"],
                         capture_output=True, text=True)
    if top.returncode != 0 or os.path.realpath(top.stdout.strip()) != os.path.realpath(tree):
        return None
    out = subprocess.run(["git", "-C", tree, "ls-files", "-z", "-co", "--exclude-standard"],
                         capture_output=True, check=True).stdout.decode()
    return [p for p in out.split("\0") if p]


def walked(tree):
    out = []
    for root, dirs, names in os.walk(tree):
        rel = os.path.relpath(root, tree)
        dirs[:] = [d for d in dirs
                   if d != ".git" and not (d == "target" and rel in (".", "bench"))]
        out += [os.path.normpath(os.path.join(rel, n)) for n in names]
    return out


files = git_files(head)
files = walked(head) if files is None else files
files = {f for f in files if os.path.lexists(os.path.join(head, f))}
written = removed = 0
for f in sorted(files):
    s, d = os.path.join(head, f), os.path.join(dest, f)
    if os.path.islink(s):
        if os.path.islink(d) and os.readlink(d) == os.readlink(s):
            continue
        if os.path.lexists(d):
            os.remove(d)
        os.makedirs(os.path.dirname(d), exist_ok=True)
        os.symlink(os.readlink(s), d)
    else:
        if (os.path.isfile(d) and not os.path.islink(d) and filecmp.cmp(s, d, shallow=False)
                and os.stat(s).st_mode & 0o111 == os.stat(d).st_mode & 0o111):
            continue
        if os.path.lexists(d):
            os.remove(d)
        os.makedirs(os.path.dirname(d), exist_ok=True)
        shutil.copyfile(s, d)
        shutil.copymode(s, d)
    written += 1
    print(f"written {f}")
tracked = subprocess.run(["git", "-C", dest, "ls-files", "-z"],
                         capture_output=True, check=True).stdout.decode()
for f in (p for p in tracked.split("\0") if p):
    if f not in files and os.path.lexists(os.path.join(dest, f)):
        os.remove(os.path.join(dest, f))
        removed += 1
        print(f"removed {f}")
print(f"the head's {len(files)} files: {written} written, {removed} of BASE's removed")
PY
}
build() { # side: the build directory built into the one target, its probes copied out
    args=""
    for e in $EX; do args="$args --example $e"; done
    # shellcheck disable=SC2086
    if ! (cd "$SRC/bench" && CARGO_TARGET_DIR="$T" cargo build --release $args) >"$W/out/build-$1.log" 2>&1; then
        echo "cost: the $1 build failed; see $W/out/build-$1.log"
        exit 2
    fi
    mkdir -p "$BIN/$1"
    for e in $EX; do
        cp "$T/release/examples/$e" "$BIN/$1/$e" ||
            { echo "cost: the $1 build's $e could not be copied out of $T"; exit 2; }
    done
    (cd "$BIN/$1" && sha256sum $EX) >"$W/out/bin-$1.sha256"
}
run() { # side name
    side=$1 name=$2
    set --
    spec=$(probe "$name")
    tog=$(echo "$spec" | cut -d'|' -f2)
    bin=$(echo "$spec" | cut -d'|' -f3)
    args=$(echo "$spec" | cut -d'|' -f4)
    t="$BUILD/tmp-$side-$name"
    mkdir -p "$t"
    if [ -n "$tog" ]; then
        set -- "$@" --collect-atstart=no "--toggle-collect=$tog"
    fi
    # Both sides' runs meet the memory group alike: the page cache dropped
    # first where that can be done (the header says why).
    if [ -n "$DROP" ]; then
        { sync && echo 1 >/proc/sys/vm/drop_caches; } 2>/dev/null ||
            { echo "cost: the page cache could not be dropped before $side $name"; exit 2; }
    fi
    # `-v` for valgrind's line that it read the binary's symbols, and
    # nothing of the program's: valgrind's own messages go to the .vg file.
    # shellcheck disable=SC2086
    if ! TMPDIR="$t" valgrind -v --tool=callgrind --callgrind-out-file="$W/out/$side.$name.out" "$@" \
        "$BIN/$side/$bin" $args >"$W/out/$side.$name.stdout" 2>"$W/out/$side.$name.vg"; then
        echo "cost: $side $name exited non-zero; see $W/out/$side.$name.vg"
        exit 2
    fi
    rm -rf "$t"
    # A binary valgrind read no symbols of has no function it can name: a
    # toggle never fires and a listing names nothing of it. Said so, and red.
    if ! grep -q "Reading syms from .*/bin/$side/$bin\$" "$W/out/$side.$name.vg"; then
        echo "cost: valgrind read no symbols from $BIN/$side/$bin ($side $name), so it can name none of its functions; see $W/out/$side.$name.vg"
        echo "$side" >>"$W/out/$name.nosyms"
    fi
    # From the build directory: callgrind_annotate names a source under the
    # directory it runs in by its path from there, so both sides' listings
    # name the engine's, the suite's and the probes' sources by their place
    # in the tree (`src/db.rs`), which is how scripts/cost-diff.py tells the
    # workspace's from a library's.
    (cd "$SRC" && callgrind_annotate --auto=no --inclusive=no --threshold=100 "$W/out/$side.$name.out") \
        >"$W/out/$side.$name.self"
}
side() { # side: build the build directory, then count every probe on its copies
    faster_for "$SRC"
    build "$1"
    for p in $PROBES; do
        run "$1" "$p"
    done
}
DROP=""
if [ -w /proc/sys/vm/drop_caches ]; then
    DROP=1
    echo "cost: the page cache is dropped before each probe's run"
else
    echo "cost: the page cache cannot be dropped here (not root, or the sysctl is read-only): each side meets the memory group as the box leaves it"
fi
side base
sync_head "$HEAD_TREE" "$SRC"
side head
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
    if [ -f "$W/out/$p.nosyms" ]; then
        echo "  NO SYMBOLS: valgrind read none from the probe's binary, side $(tr '\n' ' ' <"$W/out/$p.nosyms")"
        status=1
    fi
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
