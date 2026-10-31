#!/bin/sh
# land-pair.sh: NAME A B SIZE -- two arms paired under 640 MiB, the reads
# divided by ten, three pairs after the warmup, one sitting in each order
# (ONLY="name:1 name:2" picks sittings), the controller's trace on, the
# supdb-only timers unset. Writes `lp-<name>-<size>-{1,2}.{out,err}`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-landpair
NAME=$1; A=$2; B=$3; SIZE=$4
mkdir -p "$T"
export TMPDIR="$T"
git -C /home/user/supdb rev-parse --short HEAD > "$Z/lp-$NAME-$SIZE-head.txt"
git -C /home/user/supdb status --short >> "$Z/lp-$NAME-$SIZE-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
for sitting in 1 2; do
  if [ -n "$ONLY" ] && ! echo " $ONLY " | grep -q " $NAME:$sitting "; then continue; fi
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  out="$Z/lp-$NAME-$SIZE-$sitting"
  echo "##### $NAME $SIZE sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size "$SIZE" --bound 671088640 --ops-div 10 --reps 3 \
    --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
    2>&1 >"$out.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$out.err"
  if grep -q ' quantities: ' "$out.out"; then echo "$NAME $SIZE sitting $sitting: the table is whole"; else echo "$NAME $SIZE sitting $sitting: NOT whole"; fi
  rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-landpair/supdb-*
done
echo "##### $NAME $SIZE done $(date -u +%T)"
