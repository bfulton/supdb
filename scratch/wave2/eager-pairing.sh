#!/bin/sh
# The pairing that prices `Options::zpages_eager`: the builds started as
# partitions land (`supdb-nocachezpageseager`) against the builds at the
# settle of the form (`supdb-nocachezpages`), in one process, the load pass
# alone at three million keys in core, two sittings with the arms swapped.
# The eager build moves work between the commits and the closing sync, so
# the verdict is the phases' sum (`loadphases` on stderr, one line a load,
# `SUPDB_LOAD_PHASES=1`) beside the rate, read by the arm each line names
# and never by the lines' parity. Needs a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-eager
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/eager-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/eager-head.txt"
date -u +%FT%TZ >> "$Z/eager-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=supdb-nocachezpages,supdb-nocachezpageseager; else arms=supdb-nocachezpageseager,supdb-nocachezpages; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_LOAD_PHASES=1 "$BIN" ab --arms "$arms" --size 3000000 --ops-div 10 --reps 6 --passes load --pairs 'load' \
    > "$Z/eager-ab-$sitting.out" 2> "$Z/eager-ab-$sitting.err"
  rc=$?
  if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/eager-ab-$sitting.out"; then
    echo "sitting $sitting: the table is whole (exit $rc)"
  else
    echo "sitting $sitting: NOT whole (exit $rc)"
  fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
