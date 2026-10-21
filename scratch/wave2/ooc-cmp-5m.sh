#!/bin/sh
# The compact form against the comparators out of core, which no sitting
# had measured: every bounded pairing so far paired supdb's arms with each
# other, and the one comparator table (`docs/engine.md`, ten million keys
# under a gibibyte) priced the raw form alone. Three pairings at five
# million keys under 640 MiB, three pairs after the warmup, the reads
# divided by ten, each in both orders: LMDB with its readahead off (the
# LMDB a user configures for the regime) against the forced form
# (`supdb-nocachezpages`, the form from the first read: the ceiling), the
# same LMDB against the controller's arm (`supdb-nocacheautozpages`, what
# ships: the passes before the install raw), and RocksDB as tuned against
# the forced form. The harness's phase marks on and every stderr line
# stamped; each pairing writes `ooc-cmp-<name>-ab-{1,2}.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-ooccmp
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/ooc-cmp-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/ooc-cmp-head.txt"
date -u +%FT%TZ >> "$Z/ooc-cmp-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
pairing() {
  name=$1; A=$2; B=$3
  for sitting in 1 2; do
    if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
    echo "##### $name sitting $sitting: $arms, start $(date -u +%T)"
    env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
      --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
      2>&1 >"$Z/ooc-cmp-$name-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/ooc-cmp-$name-ab-$sitting.err"
    if grep -q ' quantities: ' "$Z/ooc-cmp-$name-ab-$sitting.out"; then echo "$name sitting $sitting: the table is whole"; else echo "$name sitting $sitting: NOT whole"; fi
    rm -rf "$T"/*
  done
  echo "##### $name done $(date -u +%T)"
}
pairing lmdbforced lmdb-nordahead supdb-nocachezpages
pairing lmdbauto lmdb-nordahead supdb-nocacheautozpages
pairing rocksforced rocksdb-tuned supdb-nocachezpages
echo "##### all done $(date -u +%T)"
