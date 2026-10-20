#!/bin/sh
# Piece 8's in-core pairing: the forced compact form (`supdb-nocachezpages`)
# against the raw form on the same paths (`supdb-nocache`), in one process at
# three million keys, over the load, the point reads, the scan passes and the
# mixes with the operations divided by ten, two sittings with the arms
# swapped. The form is expected to lose the scans in core (every group
# decoded that the raw walk reads in place) and the mixes B, D and E to
# straddle; the cells are read against the out-of-core pairing's
# (`ooc-scan-pairing.sh`), where the faults are. Three million keys for the
# reason `memo-pairing.sh` gives. Needs a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-scan
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/scan-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/scan-head.txt"
date -u +%FT%TZ >> "$Z/scan-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=supdb-nocache,supdb-nocachezpages; else arms=supdb-nocachezpages,supdb-nocache; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET "$BIN" ab --arms "$arms" --size 3000000 --ops-div 10 --reps 6 \
    --passes load,read,scan,scan-threaded,mixes --pairs 'scan' \
    > "$Z/scan-ab-$sitting.out" 2> "$Z/scan-ab-$sitting.err"
  rc=$?
  if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/scan-ab-$sitting.out"; then
    echo "sitting $sitting: the table is whole (exit $rc)"
  else
    echo "sitting $sitting: NOT whole (exit $rc)"
  fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
