#!/bin/sh
# The E pairing without the shuffled load: the combined arm against the
# controller's at five million keys under 640 MiB, three pairs after the
# warmup, both orders, the passes the question is about -- the reads, the
# scans and the mixes -- and not the shuffled load, the one pass that
# kills the block-cache arm under this bound (`anon-probe640-rss.txt`:
# its second rep's shuffled load climbs from 143 MB of anonymous memory
# to 635 in eleven seconds while the page cache is squeezed to nothing,
# where the controller's arm peaks at 333). The ordered load still runs,
# since the passes need the store; `load` left out of `--passes` leaves
# out the shuffled load and the load's own rows. Each sitting writes
# `cache-e5m-noshuf-ab-<sitting>.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-cachee
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/cache-e5m-noshuf-head.txt"
date -u +%FT%TZ >> "$Z/cache-e5m-noshuf-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=supdb-nocacheautozpages; B=supdb-cache64tofitautozpages
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### cache-e-noshuf sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
    --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
    2>&1 >"$Z/cache-e5m-noshuf-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/cache-e5m-noshuf-ab-$sitting.err"
  if grep -q ' quantities: ' "$Z/cache-e5m-noshuf-ab-$sitting.out"; then echo "sitting $sitting: the table is whole"; else echo "sitting $sitting: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### cache-e-noshuf done $(date -u +%T)"
