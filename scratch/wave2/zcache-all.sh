#!/bin/sh
# The three pairings that price the block cache's reads through the form
# file (`Options::zpages_block_cache`) at five million keys under 640 MiB,
# three pairs after the warmup, both orders, without the shuffled load
# (`cache-e-noshuf-5m.sh` says why), the reads divided by ten, with the
# mixes' timer by kind and the form's counters per mix: the combined arm
# against the controller's nocache arm (`zcache-e5m-ab-*`, the question
# the change was made for: E), against LMDB with its readahead off
# (`zcache-lmdb5m-ab-*`, whether E is won), and against the same arm with
# the cache's reads raw (`zcache-raw5m-ab-*`, what the change itself
# moved). The bench binary is built first, release, from the tree as it
# stands; the head and the time are written beside the tables.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-zcache
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/zcache-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/zcache-head.txt"
date -u +%FT%TZ >> "$Z/zcache-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
pairing() {
  name=$1; A=$2; B=$3
  for sitting in 1 2; do
    if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
    echo "##### $name sitting $sitting: $arms, start $(date -u +%T)"
    env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
      --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
      2>&1 >"$Z/$name-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/$name-ab-$sitting.err"
    if grep -q ' quantities: ' "$Z/$name-ab-$sitting.out"; then echo "$name sitting $sitting: the table is whole"; else echo "$name sitting $sitting: NOT whole"; fi
    rm -rf "$T"/*
  done
  echo "##### $name done $(date -u +%T)"
}
pairing zcache-e5m supdb-nocacheautozpages supdb-cache64tofitautozpages
pairing zcache-lmdb5m lmdb-nordahead supdb-cache64tofitautozpages
pairing zcache-raw5m supdb-cache64tofitautozpagesraw supdb-cache64tofitautozpages
echo "##### zcache-all done $(date -u +%T)"
