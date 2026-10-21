#!/bin/sh
# The block cache beside the compact form under a bound: the combined arm
# (`supdb-cache64tofitautozpages`, the cache's merged forms over the hot
# blocks kept to fit 64 MB, the compact form the controller's; at 256 MB
# the cgroup killed the harness in the shuffled load, `cache-e5m256-ab-*`) against
# the controller's arm without it, at five million keys under 640 MiB,
# three pairs after the warmup, the reads divided by ten, both orders.
# What E, A, B and F gain from the merged forms of the Zipfian head
# against what the point reads and the controller's install lose to the
# page cache the cache displaces. Each sitting writes
# `cache-e5m-ab-<sitting>.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-cachee
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/cache-e5m-head.txt"
date -u +%FT%TZ >> "$Z/cache-e5m-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=supdb-nocacheautozpages; B=supdb-cache64tofitautozpages
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### cache-e sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
    --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
    2>&1 >"$Z/cache-e5m-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/cache-e5m-ab-$sitting.err"
  if grep -q ' quantities: ' "$Z/cache-e5m-ab-$sitting.out"; then echo "sitting $sitting: the table is whole"; else echo "sitting $sitting: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### cache-e done $(date -u +%T)"
