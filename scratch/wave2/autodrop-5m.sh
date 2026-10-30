#!/bin/sh
# The combined arm against itself with the forced arm's raw drop, the drop
# held for the install (`supdb-cache64tofitautozpagesdrop`), five million
# keys under 640 MiB, three pairs after the warmup, the reads divided by
# ten, one sitting in each order; the supdb-only timers unset. ONLY as in
# ooc-cmp2-5m.sh. Writes `autodrop-<name>-ab-{1,2}.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-autodrop
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/autodrop-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/autodrop-head.txt"
git -C /home/user/supdb status --short >> "$Z/autodrop-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
pairing() {
  name=$1; A=$2; B=$3
  for sitting in 1 2; do
    if [ -n "$ONLY" ] && ! echo " $ONLY " | grep -q " $name:$sitting "; then continue; fi
    if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
    echo "##### $name sitting $sitting: $arms, start $(date -u +%T)"
    env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
      --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
      2>&1 >"$Z/autodrop-$name-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/autodrop-$name-ab-$sitting.err"
    if grep -q ' quantities: ' "$Z/autodrop-$name-ab-$sitting.out"; then echo "$name sitting $sitting: the table is whole"; else echo "$name sitting $sitting: NOT whole"; fi
    rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-autodrop/supdb-*
  done
  echo "##### $name done $(date -u +%T)"
}
pairing auto supdb-cache64tofitautozpages supdb-cache64tofitautozpagesdrop
