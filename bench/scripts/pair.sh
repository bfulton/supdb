#!/bin/sh
# pair.sh [VAR=VALUE ...] A B SIZE [bench ab options...]
#
# Two arms priced against each other the way the docs cite a pairing: one
# process, the arms alternating rep by rep, one sitting with A first and a
# second with B first, so that what the order does reads as a disagreement
# between the sittings and not as an effect (`bench ab`, DESIGN.md). Options
# after SIZE go to `bench ab` unchanged: `--bound BYTES` for a bounded
# pairing, `--passes`, `--reps`, `--ops-div`, `--len`, `--pairs`.
#
# Writes OUT/NAME-SIZE-1.{out,err} and OUT/NAME-SIZE-2.{out,err}, the table
# on stdout and the progress on stderr, OUT defaulting to `pairs` and NAME
# to `A-vs-B`; the commit and the tree's state go to OUT/NAME-SIZE-head.txt.
# The suite's timers and counters a pairing can turn on for supdb's arms
# alone are unset, so both arms run alike, whatever the shell exported; a
# VAR=VALUE before the arms sets one for the run on purpose, as a count a
# sentence quotes needs (`SUPDB_MIX_COUNTERS=1`, `SUPDB_ZPAGES_TRACE=1`),
# and its rates are then read knowing supdb timed itself. Run it on an
# otherwise idle machine: two timing runs at once measure each other.
set -u
SETS=""
while [ $# -gt 0 ]; do
  case "$1" in
    *=*) SETS="$SETS $1"; shift ;;
    *) break ;;
  esac
done
[ $# -ge 3 ] || { echo "usage: pair.sh [VAR=VALUE ...] A B SIZE [bench ab options]" >&2; exit 2; }
A=$1; B=$2; SIZE=$3; shift 3
HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN=${BIN:-$HERE/target/release/bench}
OUT=${OUT:-pairs}
NAME=${NAME:-$A-vs-$B}
[ -x "$BIN" ] || { echo "pair.sh: no bench binary at $BIN (cargo build --release)" >&2; exit 2; }
mkdir -p "$OUT"
{ git -C "$HERE" rev-parse --short HEAD; git -C "$HERE" status --short; echo "set:$SETS"; } > "$OUT/$NAME-$SIZE-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  f="$OUT/$NAME-$SIZE-$sitting"
  echo "pair.sh: sitting $sitting, $arms, $(date -u +%T)"
  # shellcheck disable=SC2086
  env $UNSET $SETS "$BIN" ab --arms "$arms" --size "$SIZE" "$@" > "$f.out" 2> "$f.err"
  rc=$?
  if [ $rc -ne 0 ] || ! grep -q ' quantities: ' "$f.out"; then
    echo "pair.sh: sitting $sitting did not finish (exit $rc); see $f.err" >&2
    exit 1
  fi
done
echo "pair.sh: done $(date -u +%T); tables in $OUT/$NAME-$SIZE-{1,2}.out"
