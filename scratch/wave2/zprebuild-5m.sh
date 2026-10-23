#!/bin/sh
# The form files built ahead priced: the combined arm with the builds ahead
# on (`supdb-cache64tofitautozpagesprebuild`, `Options::zpages_prebuild`)
# against the combined arm as shipped, at five million keys under 640 MiB,
# three pairs after the warmup, both orders, with the controller's trace
# for the install's time against the load's end. Then the same pair in
# memory, unbounded at three hundred thousand keys, one sitting, whose
# point is the counter: a store that fits builds nothing ahead
# (`zpage_prebuilt` zero in both arms), so its rows cannot move.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-zprebuild
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/zprebuild-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/zprebuild-head.txt"
date -u +%FT%TZ >> "$Z/zprebuild-head.txt"
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
pairing zprebuild5m supdb-cache64tofitautozpages supdb-cache64tofitautozpagesprebuild
echo "##### in memory: start $(date -u +%T)"
env $UNSET SUPDB_MIX_COUNTERS=1 "$BIN" ab --arms supdb-cache64tofitautozpages,supdb-cache64tofitautozpagesprebuild --size 300000 --reps 1 \
  --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' > "$Z/zprebuild300k-ab.out" 2> "$Z/zprebuild300k-ab.err"
if grep -q ' quantities: ' "$Z/zprebuild300k-ab.out"; then echo "in memory: the table is whole"; else echo "in memory: NOT whole"; fi
rm -rf "$T"/*
echo "##### zprebuild done $(date -u +%T)"
