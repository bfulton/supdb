#!/bin/sh
# The red input of scripts/cost.sh: the count is trusted only once it has
# failed where it must, by its exit status, and passed where it must.
#
#   sh scripts/cost-red.sh
#
# Ten steps, each a fact about the job and not about the engine:
#   A. the comparator (scripts/cost-diff.py) over made-up counts: a hot
#      function that gains while the total stands still is red; a total that
#      gains in no hot function is green; a side whose toggle collected
#      nothing is red; identical counts are green; and over names as
#      callgrind prints them, a hot function's closure that gains is red, a
#      function that names a hot one only in its generic arguments is not
#      it, and a probe in which no hot line names a function is red; a line
#      ending in `::` takes in its module, and a `!` line takes one back out;
#      a line is no prefix of a name, so `read_all` is not `read_all_untimed`.
#      The slacks: with no toggle, an instruction every sixty-four operations
#      is red and one every two hundred green; under a toggle, a hundred
#      instructions over the run are red and thirty-two green, whatever the
#      operations. Under a toggle every function built from source is hot --
#      the engine's on no line, a library's over the suite's types or its
#      own, one with a bare name -- and so is an unscoped `~` line's; libc's is red at
#      half again its count and ten an operation and green at its measured
#      noise, and a `!` line's is never hot. With no toggle, every function
#      of the engine by name is hot, a new one included, and one with a bare
#      name in a file of the workspace; libc's and a library's are not. A `~`
#      line holds its function to its own bound, red past it, the larger of
#      two lines' where both name it; one with no bound is refused; one that
#      names its probes holds in those alone, a toggled one included. A listing whose functions do not sum to its program total,
#      or that has none, is red, and a mode the comparator does not know is
#      refused.
#   G. the group: `check.sh cost` exits non-zero where cost-red.sh fails, and
#      stops there, and where cost.sh fails; and cost.sh over a BASE that
#      names no commit exits 2 and names it, before anything is made.
#   V. the job over a scratch copy whose suite's `supdb_get` reads a value's
#      length and not the value, and over one whose sink is `#[inline(never)]`:
#      exit 2 before anything is built, the suite's per-operation code
#      differing between the two sides.
#   S. the job over a scratch copy of this tree as it stands, against HEAD:
#      exit 0, `cost: green`, and no hot function of the point read's
#      toggle moved by one instruction -- the tree against itself reads
#      zero, so a count that moves is the engine's and not the build's
#      (another directory, another target). libc's counts may move: they
#      follow the addresses they are handed, and the point read's memcmp
#      moved by most of an instruction a read between two runs of one build.
#   B. the job over a scratch copy of this tree whose `Reader::read_all`
#      begins with a load of the store's pager and a branch on it -- the
#      shape CLAUDE.md records a plain store once paid -- against HEAD: exit
#      1, `cost: RED`, and the gain reported in a hot function.
#   U. the job's scan probe over a scratch copy whose
#      `Reader::install_ahead` -- out of line, run once a scan, and on no
#      line of the hot list -- begins with the load and the branch of B,
#      against HEAD: exit 1, `cost: RED`, the gain reported in
#      install_ahead. A toggle's whole engine is hot, not the list's.
#   K. the job's durable ordered load over a scratch copy whose run's
#      append calls a new helper of its own, out of line, at every key, against
#      HEAD: exit 1, `cost: RED`, the gain reported in the helper. A probe
#      with no toggle holds the engine's every function, not the list's.
#   T. the job over a scratch copy whose every commit runs a thousand-step
#      loop first, against HEAD: exit 0, `cost: green` -- the point read's
#      probe commits its load before the toggle starts, so a probe counts
#      its operation and not the setup around it.
#   C. the job over a scratch copy whose probe, and nothing else, reads
#      every key twice, against HEAD: exit 0, `cost: green` -- the probe is
#      the head's on both sides, so a probe that changes is no cost.
# S, B, T and C count the point read alone, U the scan and K the ordered
# load, and all six share one target directory, so
# RocksDB is built once. Everything is made under COST_RED_DIR (default
# $TMPDIR/supdb-cost-red-PID) and removed at the end, pass or fail; the
# shared target is COST_RED_TARGET where that is set, and is then kept, so
# that runs one after another build RocksDB once between them.
# COST_RED_STEPS (default `A G V S B U K T C`) runs the steps it names, in the order
# named, and each step's checks are the ones above whichever runs. The last
# line is `cost-red: ok` or names the step that went wrong; exit 0 or 1.
#
# COST_RED_ANCHORS=1 makes every step's edit of the tree as it stands and
# builds nothing: each edit's text found once, the job not run, in the time
# a copy of the tree takes. The `lint` group runs it, since the whole job
# runs only where CI runs it, on a pull request: two edits of db.rs named
# text a change and a merge had replaced, and the group read red at a pull
# request's first run, two days after the first of them, where the gate had
# passed every day.
set -eu
ROOT=$(git rev-parse --show-toplevel)
R=${COST_RED_DIR:-${TMPDIR:-/tmp}/supdb-cost-red-$$}
rm -rf "$R"
mkdir -p "$R/fx"
trap 'rm -rf "$R"' EXIT
fail() {
    echo "cost-red: $1"
    exit 1
}

step_A() {
# A. The comparator, over a hundred thousand made-up operations.
hot="$R/fx/hot.txt"
printf '%s\n' '# a comment' 'supdb::db::Reader::read_all' >"$hot"
LIBC=./string/../sysdeps/x86_64/multiarch/memmove-vec-unaligned-erms.S
STD=/rustc/0000/library/alloc/src/raw_vec/mod.rs
row() { # instructions function [file: the workspace's by default]
    printf ' %s ( 1.00%%)  %s:%s [/bin/x]\n' "$1" "${3:-src/db.rs}" "$2"
}
# A listing ends in its program total, as callgrind_annotate's does: the
# sum of its rows where a fixture has written none.
totalled() { # fixture -> the fixture with its total
    if grep -q 'PROGRAM TOTALS' "$1"; then
        cat "$1"
    else
        cat "$1"
        awk '{ gsub(",", "", $1); s += $1 } END { printf " %d (100.0%%)  PROGRAM TOTALS\n", s }' "$1"
    fi
}
LIST='nothing::'
OWN='supdb::,supdb_bench::,read_only::'
diffs() { # head-fixture mode own -> the comparator's exit status
    totalled "$R/fx/base" >"$R/fx/base.t"
    totalled "$R/fx/$1" >"$R/fx/$1.t"
    st=0
    python3 "$ROOT/scripts/cost-diff.py" "$R/fx/base.t" "$R/fx/$1.t" 100000 "$hot" "$1" "$2" "$3" >"$R/fx/$1.out" || st=$?
    echo "$st"
}
# The list alone (a probe with no toggle, whose own names are none here).
{ row 4,000,000 'supdb::db::Reader::read_all'; row 2,000,000 'supdb::db::Other::f'; } >"$R/fx/base"
{ row 4,800,000 'supdb::db::Reader::read_all'; row 1,200,000 'supdb::db::Other::f'; } >"$R/fx/a1"
{ row 4,000,000 'supdb::db::Reader::read_all'; row 3,000,000 'supdb::db::Other::f'; } >"$R/fx/a2"
: >"$R/fx/a3"
cp "$R/fx/base" "$R/fx/a4"
[ "$(diffs a1 whole "$LIST")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::read_all' "$R/fx/a1.out" ||
    fail "A: a hot function's gain under a level total was not red"
[ "$(diffs a2 whole "$LIST")" = 0 ] || fail "A: a total's gain in no hot function was not green"
[ "$(diffs a3 whole "$LIST")" = 1 ] && grep -q 'EMPTY: the head side' "$R/fx/a3.out" ||
    fail "A: a side that collected nothing was not red"
[ "$(diffs a4 whole "$LIST")" = 0 ] || fail "A: identical counts were not green"
# Names as callgrind prints them: a method as `<T>::f`, a monomorph with its
# arguments. A closure of a hot function that gains is red; a function that
# only names a hot function in its generic arguments is not that function;
# a probe in which no hot line names a function is red.
printf '%s\n' 'supdb::db::Reader::read_all_in' 'supdb_bench::engines::supdb_range' >"$hot"
g='<supdb::db::Reader>::read_all_in::<supdb_bench::engines::supdb_get::{closure#0}>::{closure#5}'
o='<supdb::blob::Blob<supdb::bytes::MmapBytes>>::read_exts_inline::<supdb_bench::engines::supdb_range::{closure#0}>'
{ row 4,000,000 "$g"; row 2,000,000 "$o"; } >"$R/fx/base"
{ row 4,800,000 "$g"; row 1,200,000 "$o"; } >"$R/fx/a5"
{ row 4,000,000 "$g"; row 3,000,000 "$o"; } >"$R/fx/a6"
{ row 4,000,000 'supdb::db::Other::f'; } >"$R/fx/a7"
[ "$(diffs a5 whole "$LIST")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::read_all_in::{closure#5}' "$R/fx/a5.out" ||
    fail "A: a hot function's closure, named as callgrind names it, was not red"
[ "$(diffs a6 whole "$LIST")" = 0 ] || fail "A: a function named in another's generic arguments was taken for it"
cp "$R/fx/a7" "$R/fx/base"
[ "$(diffs a7 whole "$LIST")" = 1 ] && grep -q 'NO HOT' "$R/fx/a7.out" ||
    fail "A: a probe in which no hot line names a function was not red"
# A line ending in `::` takes in its module, and a `!` line takes a
# function back out of it.
printf '%s\n' 'supdb::db::zfile::' '!supdb::db::zfile::read_budget' >"$hot"
{ row 1,000,000 'supdb::db::zfile::read_budget'; row 1,000,000 '<supdb::db::zfile::ZCursor>::open_at'; } >"$R/fx/base"
{ row 1,900,000 'supdb::db::zfile::read_budget'; row 1,000,000 '<supdb::db::zfile::ZCursor>::open_at'; } >"$R/fx/a8"
{ row 1,000,000 'supdb::db::zfile::read_budget'; row 1,900,000 '<supdb::db::zfile::ZCursor>::open_at'; } >"$R/fx/a9"
[ "$(diffs a8 whole "$LIST")" = 0 ] || fail "A: a function a ! line takes out was counted"
[ "$(diffs a8 toggle "$OWN")" = 0 ] || fail "A: a function a ! line takes out was counted under a toggle"
[ "$(diffs a9 whole "$LIST")" = 1 ] && grep -q 'HOT GAINED: supdb::db::zfile::ZCursor::open_at' "$R/fx/a9.out" ||
    fail "A: a function of a hot module was not red"
# A line names a function and what is below it, never a function whose
# name only begins with it.
printf '%s\n' 'supdb::db::Reader::read_all' >"$hot"
{ row 1,000,000 '<supdb::db::Reader>::read_all::<G>'; row 1,000,000 '<supdb::db::Reader>::read_all_untimed'; } >"$R/fx/base"
{ row 1,000,000 '<supdb::db::Reader>::read_all::<G>'; row 1,900,000 '<supdb::db::Reader>::read_all_untimed'; } >"$R/fx/a10"
[ "$(diffs a10 whole "$LIST")" = 0 ] || fail "A: a function whose name begins with a hot line's was taken for it"
# The slacks. With no toggle, a hundredth of an instruction an operation:
# one instruction a block of sixty-four keys is red, a two-hundredth green.
{ row 4,000,000 'supdb::db::Reader::read_all'; } >"$R/fx/base"
{ row 4,001,563 'supdb::db::Reader::read_all'; } >"$R/fx/a11"
{ row 4,000,500 'supdb::db::Reader::read_all'; } >"$R/fx/a12"
[ "$(diffs a11 whole "$LIST")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::read_all ' "$R/fx/a11.out" ||
    fail "A: an instruction every sixty-four operations was not red without a toggle"
[ "$(diffs a12 whole "$LIST")" = 0 ] || fail "A: a two-hundredth of an instruction an operation was red without a toggle"
# Under a toggle, a few instructions over the run, whatever the operations:
# the same sixty-fourth an operation is red, and so is a hundred over the
# run; thirty-two over the run is green.
{ row 4,000,000 'supdb::db::Reader::read_all'; row 1,000,000 '<supdb::db::Reader>::install_ahead'; } >"$R/fx/base"
{ row 4,000,000 'supdb::db::Reader::read_all'; row 1,001,563 '<supdb::db::Reader>::install_ahead'; } >"$R/fx/a19"
{ row 4,000,000 'supdb::db::Reader::read_all'; row 1,000,100 '<supdb::db::Reader>::install_ahead'; } >"$R/fx/a20"
{ row 4,000,000 'supdb::db::Reader::read_all'; row 1,000,032 '<supdb::db::Reader>::install_ahead'; } >"$R/fx/a12t"
[ "$(diffs a19 toggle "$OWN")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::install_ahead ' "$R/fx/a19.out" ||
    fail "A: under a toggle, an instruction every sixty-four operations was not red"
[ "$(diffs a20 toggle "$OWN")" = 1 ] || fail "A: under a toggle, a hundred instructions over the run were not red"
[ "$(diffs a12t toggle "$OWN")" = 0 ] || fail "A: under a toggle, thirty-two instructions over the run were red"
# What a toggle holds: every function built from source, the engine's on
# no line, a library's instantiated over the suite's and a library's own;
# libc's only past its own noise; a `!` line's never.
w='<std::thread::local::LocalKey<core::cell::RefCell<alloc::vec::Vec<u8>>>>::with::<supdb_bench::engines::consume::{closure#0}, ()>'
lib='<alloc::raw_vec::RawVec<u8>>::grow_one'
printf '%s\n' 'supdb::db::Reader::read_all' '!supdb::db::zfile::read_budget' '~supdb::db::Reader::sweep_retired_states 5' >"$hot"
base_rows() { # memcpy memcmp malloc install_ahead with grow_one read_budget sweep helper
    row 4,000,000 'supdb::db::Reader::read_all'
    row "$1" '__memcpy_avx_unaligned_erms' "$LIBC"
    row "$2" '__memcmp_avx2_movbe' ./string/../sysdeps/x86_64/multiarch/memcmp-avx2-movbe.S
    row "$3" '_int_malloc' ./malloc/./malloc/malloc.c
    row "$4" '<supdb::db::Reader>::install_ahead'
    row "$5" "$w" "$STD"
    row "$6" "$lib" "$STD"
    row "$7" 'supdb::db::zfile::read_budget'
    row "$8" '<supdb::db::Reader>::sweep_retired_states'
    [ "$9" = 0 ] || row "$9" 'cost_mutant_helper' src/bytes.rs
}
base_rows 2,000,000 2,000,000 48,000,000 1,000,000 1,000,000 1,000,000 1,000,000 1,000,000 0 >"$R/fx/base"
base_rows 2,000,000 2,000,000 48,000,000 1,300,000 1,000,000 1,000,000 1,000,000 1,000,000 0 >"$R/fx/a13"
base_rows 2,000,000 2,083,000 48,240,000 1,000,000 1,000,000 1,000,000 1,900,000 1,000,000 0 >"$R/fx/a14"
base_rows 2,000,000 2,000,000 48,000,000 1,000,000 1,300,000 1,000,000 1,000,000 1,000,000 0 >"$R/fx/a16"
base_rows 3,000,000 2,000,000 48,000,000 1,000,000 1,000,000 1,000,000 1,000,000 1,000,000 0 >"$R/fx/a21"
base_rows 2,000,000 2,000,000 48,000,000 1,000,000 1,000,000 1,900,000 1,000,000 1,000,000 0 >"$R/fx/a22"
base_rows 2,000,000 2,000,000 48,000,000 1,000,000 1,000,000 1,000,000 1,000,000 1,400,000 0 >"$R/fx/a25"
base_rows 2,000,000 2,000,000 48,000,000 1,000,000 1,000,000 1,000,000 1,000,000 1,000,000 300,000 >"$R/fx/a26"
base_rows 2,000,000 2,000,000 48,000,000 1,000,000 1,000,000 1,000,000 1,000,000 1,000,000 2,000 >"$R/fx/a24"
[ "$(diffs a13 toggle "$OWN")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::install_ahead ' "$R/fx/a13.out" ||
    fail "A: under a toggle, a function of the engine on no line gained and was not red"
[ "$(diffs a14 toggle "$OWN")" = 0 ] ||
    fail "A: under a toggle, libc's noise (memcmp four percent, the allocator half a percent) or a ! line's function was red"
[ "$(diffs a16 toggle "$OWN")" = 1 ] && grep -q 'HOT GAINED: std::thread::local::LocalKey::with (over the engine) ' "$R/fx/a16.out" ||
    fail "A: under a toggle, a library function instantiated over the suite's gained and was not red"
[ "$(diffs a21 toggle "$OWN")" = 1 ] && grep -q 'LIBRARY GAINED: __memcpy_avx_unaligned_erms ' "$R/fx/a21.out" ||
    fail "A: under a toggle, libc's copy at half again its count, ten an operation, was not red"
[ "$(diffs a22 toggle "$OWN")" = 1 ] && grep -q 'HOT GAINED: alloc::raw_vec::RawVec::grow_one ' "$R/fx/a22.out" ||
    fail "A: under a toggle, a library function built from source gained and was not red"
[ "$(diffs a25 toggle "$OWN")" = 1 ] || fail "A: under a toggle, a ~ line's function gained and was not red"
[ "$(diffs a26 toggle "$OWN")" = 1 ] && grep -q 'HOT GAINED: cost_mutant_helper ' "$R/fx/a26.out" ||
    fail "A: under a toggle, a new function with a bare name was not red"
# What a probe with no toggle holds: the engine's every function by name,
# a new one included, and one with a bare name in a file of the workspace;
# not libc's, not a library's; and a `~` line's to its own bound.
[ "$(diffs a13 whole "$OWN")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::install_ahead ' "$R/fx/a13.out" ||
    fail "A: without a toggle, a function of the engine on no line gained and was not red"
[ "$(diffs a24 whole "$OWN")" = 1 ] && grep -q 'HOT GAINED: cost_mutant_helper ' "$R/fx/a24.out" ||
    fail "A: without a toggle, a new function in a file of the workspace was not red"
[ "$(diffs a26 whole "$OWN")" = 1 ] || fail "A: without a toggle, a bare-named function of the workspace was not red"
for f in a14 a16 a21 a22 a25; do
    [ "$(diffs $f whole "$OWN")" = 0 ] || fail "A: without a toggle, $f (libc, a library or a ~ line under its bound) was red"
done
# A `~` line's bound is a bound: four an operation past a bound of three is
# red, and named with its bound.
printf '%s\n' 'supdb::db::Reader::read_all' '!supdb::db::zfile::read_budget' '~supdb::db::Reader::sweep_retired_states 3' >"$hot"
[ "$(diffs a25 whole "$OWN")" = 1 ] && grep -q 'HOT GAINED: supdb::db::Reader::sweep_retired_states .*its bound 3.000' "$R/fx/a25.out" ||
    fail "A: without a toggle, a ~ line's function past its bound was not red"
# Of two lines that name one function, the larger bound holds.
printf '%s\n' 'supdb::db::Reader::read_all' '~supdb::db::Reader::sweep_retired_states 3' '~supdb::db::Reader:: 5' >"$hot"
[ "$(diffs a25 whole "$OWN")" = 0 ] || fail "A: of two ~ lines naming one function, the smaller bound held"
# A `~` line with no bound is refused.
printf '%s\n' 'supdb::db::Reader::read_all' '~supdb::db::Reader::sweep_retired_states' >"$hot"
[ "$(diffs a25 whole "$OWN")" = 2 ] && grep -q 'names no bound' "$R/fx/a25.out" ||
    fail "A: a ~ line with no bound was not refused"
# A `~` line that names its probes holds in those alone, and in a probe with
# a toggle too.
printf '%s\n' 'supdb::db::Reader::read_all' '~a28: supdb::db::Reader::sweep_retired_states 5' >"$hot"
cp "$R/fx/a25" "$R/fx/a28"
[ "$(diffs a28 whole "$OWN")" = 0 ] || fail "A: a ~ line naming its probe did not bound its function there"
[ "$(diffs a28 toggle "$OWN")" = 0 ] || fail "A: a ~ line naming a probe with a toggle did not bound its function there"
[ "$(diffs a25 whole "$OWN")" = 1 ] || fail "A: a ~ line naming another probe bounded its function in this one"
# A listing cut short of its program total: callgrind_annotate's default
# threshold leaves out the functions under it, and a cost added to one of
# them would read as nothing on both sides.
{ cat "$R/fx/base"; printf ' %s (100.0%%)  PROGRAM TOTALS\n' 10,000,001; } >"$R/fx/a17"
[ "$(diffs a17 toggle "$OWN")" = 1 ] && grep -q 'PARTIAL: the head side' "$R/fx/a17.out" ||
    fail "A: a listing whose functions fall short of its total was not red"
totalled "$R/fx/base" >"$R/fx/base.t"
st=0
python3 "$ROOT/scripts/cost-diff.py" "$R/fx/base.t" "$R/fx/base" 100000 "$hot" a18 toggle "$OWN" >"$R/fx/a18.out" || st=$?
[ "$st" = 1 ] && grep -q 'PARTIAL: the head side.*no PROGRAM TOTALS' "$R/fx/a18.out" ||
    fail "A: a listing with no program total was not red"
st=0
python3 "$ROOT/scripts/cost-diff.py" "$R/fx/base.t" "$R/fx/base.t" 100000 "$hot" a27 timed "$OWN" >"$R/fx/a27.out" || st=$?
[ "$st" = 2 ] || fail "A: a mode the comparator does not know was not refused"
echo "cost-red: A ok"
}

# The scratch copy: this tree's files as they stand, committed or not.
copy() { # dest
    mkdir -p "$1"
    (cd "$ROOT" && git ls-files -co --exclude-standard -- . ':!scratch' ':!bench/runs' ':!docs/figures' |
        tar -cf - -T -) | (cd "$1" && tar -xf -)
}
edit() { # file old new: replace the one occurrence or fail
    python3 - "$1" "$2" "$3" <<'PY' || fail "the edit of $1 did not apply"
import sys
path, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path).read()
if s.count(old) != 1:
    sys.exit(f"{path}: {s.count(old)} occurrences of the text to replace")
open(path, "w").write(s.replace(old, new))
PY
}
RT=${COST_RED_TARGET:-$R/t}
# Every step's copy is made at one path, $R/h, and removed when the step
# ends: cargo keys a path package's artifacts by its path, so a copy at a
# path of its own would leave another build of the engine, the suite and the
# probe in the shared target for every step, a gigabyte and more each.
# S, B, T and C count copies of this tree against HEAD, so the engine and the
# suite in the tree have to be HEAD's: an uncommitted change to either would
# read as the job's fault in S and hide behind it in B.
anchored() { [ -n "${COST_RED_ANCHORS:-}" ]; }
same_as_head() {
    anchored && return 0
    git -C "$ROOT" diff --quiet HEAD -- src Cargo.toml Cargo.lock bench/src bench/Cargo.toml bench/Cargo.lock ||
        fail "$1: the engine or the suite differs from HEAD in the tree; commit it, or count it with cost.sh"
}
job() { # copy step [probe] -> the job's exit status; its output in $R/<step>.log
    if anchored; then
        echo anchored
        return 0
    fi
    st=0
    COST_DIR="$R/w" COST_HEAD="$1" COST_HEAD_TARGET="$RT" COST_BASE_TARGET="$RT" \
        sh "$ROOT/scripts/cost.sh" HEAD "${3:-read}" >"$R/$2.log" 2>&1 || st=$?
    echo "$st"
}
shown() { # step: the job's output, kept past the cleanup
    sed "s/^/  $1| /" "$R/$1.log"
}

step_S() {
same_as_head S
# S. The tree against itself.
copy "$R/h"
st=$(job "$R/h" S)
shown S
[ "$st" = 0 ] || fail "S: the job exited $st over the tree against itself"
tail -1 "$R/S.log" | grep -qx 'cost: green' || fail "S: the verdict line is not cost: green"
grep -q '^== read (toggle)' "$R/S.log" && grep -q 'on no line), 0 of them moved, ' "$R/S.log" ||
    fail "S: the tree against itself moved a hot function under the toggle"
echo "cost-red: S ok"
rm -rf "$R/h"
}

step_B() {
same_as_head B
# B. A load and a branch at the top of the point read.
copy "$R/h"
edit "$R/h/src/db.rs" \
    '        let _entered = self.enter();
        self.read_all_in::<F, false>(key, f)' \
    '        if std::hint::black_box(&self.shared.pager).is_some() {
            std::hint::black_box(());
        }
        let _entered = self.enter();
        self.read_all_in::<F, false>(key, f)'
st=$(job "$R/h" B)
if anchored; then echo "cost-red: B's edits apply"; rm -rf "$R/h"; return 0; fi
shown B
[ "$st" = 1 ] || fail "B: the job exited $st over a read that pays a load and a branch"
tail -1 "$R/B.log" | grep -qx 'cost: RED' || fail "B: the verdict line is not cost: RED"
grep -q 'HOT GAINED: ' "$R/B.log" || fail "B: no hot function was reported"
echo "cost-red: B ok"
rm -rf "$R/h"
}

step_T() {
same_as_head T
# T. A cost in the load the point-read probe makes before its toggle.
copy "$R/h"
edit "$R/h/src/db.rs" \
    '    pub fn commit(&mut self) -> Result<()> {
        let _op = self.op();' \
    '    pub fn commit(&mut self) -> Result<()> {
        for i in 0..1000u32 {
            std::hint::black_box(i);
        }
        let _op = self.op();'
st=$(job "$R/h" T)
if anchored; then echo "cost-red: T's edits apply"; rm -rf "$R/h"; return 0; fi
shown T
[ "$st" = 0 ] || fail "T: the job exited $st over a cost the point read's probe pays outside its toggle"
tail -1 "$R/T.log" | grep -qx 'cost: green' || fail "T: the verdict line is not cost: green"
echo "cost-red: T ok"
rm -rf "$R/h"
}

step_G() {
# G. The group passes on each script's failure, and stops at cost-red's.
# The inner runs make their own directories: one named by the caller here
# would be emptied under this run.
st=0
env -u COST_RED_DIR -u COST_DIR COST_RED_STEPS=no_such_step COST_PROBES=no_such_probe \
    sh "$ROOT/scripts/check.sh" cost >"$R/G1.log" 2>&1 || st=$?
shown G1
[ "$st" != 0 ] && grep -q 'cost-red: no step no_such_step' "$R/G1.log" ||
    fail "G: the group exited $st where cost-red.sh failed"
grep -q 'cost: no probe' "$R/G1.log" && fail "G: the group ran cost.sh after cost-red.sh failed"
st=0
env -u COST_RED_DIR -u COST_DIR COST_RED_STEPS=A COST_PROBES=no_such_probe \
    sh "$ROOT/scripts/check.sh" cost >"$R/G2.log" 2>&1 || st=$?
shown G2
[ "$st" != 0 ] && grep -q 'cost: no probe no_such_probe' "$R/G2.log" ||
    fail "G: the group exited $st where cost.sh failed"
# A BASE that names no commit is refused by name before anything is made.
st=0
env -u COST_DIR sh "$ROOT/scripts/cost.sh" cost-red-no-such-ref seal >"$R/G3.log" 2>&1 || st=$?
shown G3
[ "$st" = 2 ] && grep -q "BASE 'cost-red-no-such-ref' names no commit" "$R/G3.log" ||
    fail "G: cost.sh exited $st over a BASE that names no commit"
echo "cost-red: G ok"
}

step_V() {
same_as_head V
# V. The suite's code a toggle counts through, changed on one side.
copy "$R/h"
edit "$R/h/bench/src/engines.rs" \
    '    r.read_all(key, |v| n += consume(v))' \
    '    r.read_all(key, |v| n += v.len())'
st=$(job "$R/h" V)
if ! anchored; then
shown V
[ "$st" = 2 ] || fail "V: the job exited $st over a suite whose per-read code differs from the base's"
grep -q "the suite's \`fn supdb_get(\` differs" "$R/V.log" || fail "V: the suite's change was not named"
grep -q 'Compiling' "$R/w/out/build-base.log" 2>/dev/null && fail "V: the job built before it refused"
fi
rm -rf "$R/h"
# An attribute is the item's: the sink taken out of line is a step of every
# read added in the suite.
copy "$R/h"
edit "$R/h/bench/src/engines.rs" '#[inline]
fn consume(' '#[inline(never)]
fn consume('
st=$(job "$R/h" V2)
if anchored; then echo "cost-red: V's edits apply"; rm -rf "$R/h"; return 0; fi
shown V2
[ "$st" = 2 ] && grep -q "the suite's \`fn consume(\` differs" "$R/V2.log" ||
    fail "V: the job exited $st over a suite whose sink changed its attribute"
echo "cost-red: V ok"
rm -rf "$R/h"
}

step_U() {
same_as_head U
# U. A load and a branch at the top of a function out of line, once a
# scan, on no line of the list.
copy "$R/h"
edit "$R/h/src/db.rs" \
    '    fn install_ahead(&self, unsealed: &Snapshot) -> Result<()> {
' \
    '    fn install_ahead(&self, unsealed: &Snapshot) -> Result<()> {
        if std::hint::black_box(&self.shared.pager).is_some() {
            std::hint::black_box(());
        }
'
grep -q '^supdb::db::Reader::install_ahead$' "$ROOT/scripts/cost-hot.txt" && hotted=1 || hotted=0
st=$(job "$R/h" U scan)
if anchored; then echo "cost-red: U's edits apply"; rm -rf "$R/h"; return 0; fi
shown U
[ "$st" = 1 ] || fail "U: the job exited $st over a scan that pays a load and a branch"
tail -1 "$R/U.log" | grep -qx 'cost: RED' || fail "U: the verdict line is not cost: RED"
grep -q 'HOT GAINED: supdb::db::Reader::install_ahead ' "$R/U.log" || fail "U: the gain was not reported in install_ahead"
[ "$hotted" = 0 ] || fail "U: install_ahead is on a line of the list, so U holds the list and not the toggle"
echo "cost-red: U ok"
rm -rf "$R/h"
}

step_K() {
same_as_head K
# K. A step of every key in a helper of its own, on no line, in the
# ordered load's append to its run, the one step of every key a run's
# lean commit leaves (`Options::direct_lean_append`): a probe with no
# toggle holds every function of the engine, and a new one most of all.
copy "$R/h"
edit "$R/h/src/db.rs" \
    '                if self.opts.direct_lean_append {
                    mem.append_ordered(key, value);' \
    '                if self.opts.direct_lean_append {
                    cost_red_step(key.len());
                    mem.append_ordered(key, value);'
printf '\n/// cost-red step K: a step of every key, out of line.\n#[inline(never)]\nfn cost_red_step(i: usize) {\n    std::hint::black_box(i);\n}\n' >>"$R/h/src/db.rs"
st=$(job "$R/h" K load_durable)
if anchored; then echo "cost-red: K's edits apply"; rm -rf "$R/h"; return 0; fi
shown K
[ "$st" = 1 ] || fail "K: the job exited $st over an ordered load that pays a helper a key"
tail -1 "$R/K.log" | grep -qx 'cost: RED' || fail "K: the verdict line is not cost: RED"
grep -q 'HOT GAINED: supdb::db::cost_red_step ' "$R/K.log" || fail "K: the gain was not reported in the new helper"
echo "cost-red: K ok"
rm -rf "$R/h"
}

step_C() {
same_as_head C
# C. The probe changed and the engine not.
copy "$R/h"
edit "$R/h/bench/examples/read_only.rs" \
    'fn main() {' \
    'fn main() {
    // cost-red step C: the whole probe twice, the engine untouched.
    real_main();
    real_main();
}

fn real_main() {'
st=$(job "$R/h" C)
if anchored; then echo "cost-red: C's edits apply"; rm -rf "$R/h"; return 0; fi
shown C
[ "$st" = 0 ] || fail "C: the job exited $st over a probe that changed with the engine untouched"
tail -1 "$R/C.log" | grep -qx 'cost: green' || fail "C: the verdict line is not cost: green"
echo "cost-red: C ok"
rm -rf "$R/h"
}

# The steps with no edit of the tree have nothing to anchor.
anchored && default="V B U K T C" || default="A G V S B U K T C"
for step in ${COST_RED_STEPS:-$default}; do
    case $step in
    A | G | V | S | B | U | K | T | C) "step_$step" ;;
    *) fail "no step $step" ;;
    esac
done
echo "cost-red: ok"
