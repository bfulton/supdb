#!/bin/sh
# The memo's size by count: one rep of the combined arm against the same
# arm with the memo of decoded groups at 16 and then 32 MiB
# (`Options::zpages_memo_bytes`, `supdb-cache64tofitautozpagesmemo16`,
# `...memo32`) at five million keys under 640 MiB, with the per-mix
# counters on. The counts are deterministic -- the groups decoded and the
# memo's hits and misses per mix -- so one rep says what a larger memo
# saves in decodes; what it costs in page cache under the bound is a
# pairing's to price, after.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-zmemo
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/zmemo-count-head.txt"
git -C /home/user/supdb status --short >> "$Z/zmemo-count-head.txt"
date -u +%FT%TZ >> "$Z/zmemo-count-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for mib in 16 32; do
  name=zmemo${mib}count5m
  arms=supdb-cache64tofitautozpages,supdb-cache64tofitautozpagesmemo$mib
  echo "##### $name: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 1 \
    --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
    2>&1 >"$Z/$name-ab.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/$name-ab.err"
  if grep -q ' quantities: ' "$Z/$name-ab.out"; then echo "$name: the table is whole"; else echo "$name: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### zmemo count done $(date -u +%T)"
