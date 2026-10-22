#!/bin/sh
# The promotion arm priced: the combined arm with a block read a block's
# worth of entries held as a copy in its slot under the form
# (`Options::zpages_promote_copies`, `supdb-cache64tofitautozpagespromote`)
# against the combined arm, and against LMDB with its readahead off, at
# five million keys under 640 MiB, three pairs after the warmup, both
# orders, without the shuffled load, with the mixes' timer by kind and the
# form's counters per mix. The bench binary is built first, release, from
# the tree as it stands; the head and the time are written beside the
# tables. Each pairing writes `zcache-<name>-ab-<sitting>.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-zpromote
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/zcache-promote-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/zcache-promote-head.txt"
date -u +%FT%TZ >> "$Z/zcache-promote-head.txt"
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
pairing zcache-promote5m supdb-cache64tofitautozpages supdb-cache64tofitautozpagespromote
pairing zcache-promotelmdb5m lmdb-nordahead supdb-cache64tofitautozpagespromote
echo "##### zcache-promote done $(date -u +%T)"
