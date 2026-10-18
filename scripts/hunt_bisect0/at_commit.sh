#!/bin/sh
# A hunt branch's tool, never for a merge: builds tests/hunt_bisect0.rs (or tests/hunt_cuts.rs) against the sources of any
# commit of the history, without touching the worktree, and prints the path of the test executable.
#
#   sh scripts/hunt_bisect0/at_commit.sh COMMIT KIND [PORT [TEST]]
#
# KIND names the option switches the commit has (the file's header says what each sets):
#   z  none                                                      (109d921 and before)
#   f  commit_forms only                                         (069eecb)
#   s  share_snapshot and commit_forms                           (eeda338, 23115e7)
#   o  every switch, the seal cannot be held                     (39401bb .. 39b8f17)
#   l  as o, and the eager carry (HUNT_EAGER=1)                  (e607e70 only)
#   m  every switch, the seal can be held, no frozen_tables      (b6b259d .. 7b03182)
#   n  every switch, the seal can be held                        (758fcd3 and after)
# PORT is hook (the forcing pause of the snapshot schedule: HUNT_SCAN_AFTER_CHECK_MS between a Snapshot handle's one check
# of the published forms' position and its load of a form), diag (HUNT_CHECK_CUTS=1: the rebase of a rewritten partition
# counts the filed keys it carries and the cuts that are not the new partition's), nolanded (the take-away of that hazard:
# a landing no longer keeps the writer's snapshot and the keys filed into its tables, as before e1bd6fb), or plain (the
# default).
# TEST is hunt_bisect0 (the default) or hunt_cuts (the deterministic stale-cut schedule; KIND o, m or n).
#
# The snapshot schedule, on a binary built with hook: HUNT_SCAN_AFTER_CHECK_MS=300 HUNT_CYCLES=3, with HUNT_UPKEEP=bg1
# where the upkeep thread is to do the publishing, or HUNT_THREADED=1 HUNT_READER_DELAY_MS=0 where the writer's own
# commit is (the first scan on a thread of its own); the cycles that fail are counted on the HUNT-CYCLES line.
#
# Every build has a target directory of its own, removed after the executable is copied out. A shared one is wrong here:
# the root package's artifact name does not depend on the checkout path, `git archive` gives the files their commit
# times, and cargo then takes the library an earlier export built (older commit or newer, same flags) as fresh and links
# the test against it -- two exports that differed in one string constant both linked the first one's.
set -eu
COMMIT="$1"; KIND="$2"; PORT="${3:-plain}"; TEST="${4:-hunt_bisect0}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
OUT="${HUNT_OUT:-${TMPDIR:-/tmp}/hunt_bisect0}"
case "$KIND" in
  z) RF="--cfg hunt_old" ;;
  f) RF="--cfg hunt_forms --cfg hunt_old" ;;
  s) RF="--cfg hunt_share --cfg hunt_forms --cfg hunt_old" ;;
  o) RF="--cfg hunt_opts --cfg hunt_rebase --cfg hunt_carry --cfg hunt_old" ;;
  l) RF="--cfg hunt_opts --cfg hunt_rebase --cfg hunt_carry --cfg hunt_old --cfg hunt_lazy" ;;
  m) RF="--cfg hunt_opts --cfg hunt_rebase --cfg hunt_carry --cfg hunt_nofrozen" ;;
  n) RF="--cfg hunt_opts --cfg hunt_rebase --cfg hunt_carry" ;;
  *) echo "unknown kind $KIND" >&2; exit 2 ;;
esac
SHA="$(git -C "$REPO" rev-parse --short "$COMMIT")"
X="$OUT/src-$SHA-$PORT-$TEST-$$"
rm -rf "$X"; mkdir -p "$X/tests" "$OUT/bin"
git -C "$REPO" archive --format=tar "$COMMIT" Cargo.toml Cargo.lock src | tar -x -C "$X"
case "$PORT" in
  hook) perl "$HERE/hookport.pl" "$X/src/db.rs" >&2 ;;
  diag) perl "$HERE/diagport.pl" "$X/src/db.rs" >&2 ;;
  plain) ;;
  nolanded)
    # The take-away of the stale-cut hazard: a landing no longer keeps the writer's snapshot and the keys filed into
    # its tables (`Reader::rebase_tables`, `keep_snap`), as before e1bd6fb.
    perl -0pi -e 's/let keep_snap = \(same_frozen \|\| landed\)/let keep_snap = (same_frozen)/; s/let keep_snap = same_frozen \|\| landed;/let keep_snap = same_frozen;/' "$X/src/db.rs"
    if grep -q 'landed' "$X/src/db.rs" && ! grep -q 'let keep_snap = (same_frozen)\|let keep_snap = same_frozen;' "$X/src/db.rs"; then
      echo "no keep_snap line to take the landing out of" >&2; exit 2
    fi ;;
  *) echo "unknown port $PORT" >&2; exit 2 ;;
esac
cp "$REPO/tests/$TEST.rs" "$X/tests/$TEST.rs"
export CARGO_TARGET_DIR="$OUT/target-$SHA-$PORT-$TEST-$$"
export RUSTFLAGS="$RF"
LOG="$OUT/build-$SHA-$PORT-$TEST-$$.log"
if ! ( cd "$X" && cargo test --no-run --test "$TEST" > "$LOG" 2>&1 ); then
  rm -rf "$CARGO_TARGET_DIR"
  echo "build failed, see $LOG" >&2
  exit 1
fi
EXE="$(grep -o "Executable tests/$TEST.rs ([^)]*)" "$LOG" | sed 's/.*(\(.*\))/\1/')"
BIN="$OUT/bin/$TEST-$SHA-$PORT-$KIND"
cp "$EXE" "$BIN.$$" && mv -f "$BIN.$$" "$BIN"
rm -rf "$CARGO_TARGET_DIR" "$X"
echo "$BIN"
