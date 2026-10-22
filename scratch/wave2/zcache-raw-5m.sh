#!/bin/sh
# The third pairing of `zcache-all.sh` on its own -- the combined arm with
# the block cache's reads raw against the same arm with them through the
# form file, five million keys under 640 MiB, three pairs after the
# warmup, both orders, without the shuffled load -- run again with the
# binary `zcache-all.sh` built (`zcache-head.txt` names its head), since
# the container restarted inside its first sitting. Writes
# `zcache-raw5m-ab-<sitting>.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-zcache
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=supdb-cache64tofitautozpagesraw; B=supdb-cache64tofitautozpages
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### zcache-raw5m sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
    --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
    2>&1 >"$Z/zcache-raw5m-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/zcache-raw5m-ab-$sitting.err"
  if grep -q ' quantities: ' "$Z/zcache-raw5m-ab-$sitting.out"; then echo "sitting $sitting: the table is whole"; else echo "sitting $sitting: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### zcache-raw5m done $(date -u +%T)"
