#!/bin/sh
# The shipping arms against LMDB out of core again, after the engine's own
# LZ4 decoder became the default: LMDB with its readahead off against the
# forced form with the drop (`supdb-cache64tofitzpagesdrop`) and against the
# combined arm (`supdb-cache64tofitautozpages`, the passes before the
# install raw), five million keys under 640 MiB, three pairs after the
# warmup, the reads divided by ten, each in both orders. Every variable that
# turns on a supdb-only timer is unset, since a comparator's cell is read
# with nothing in one arm's path the other lacks. Writes
# `ooc-cmp2-<name>-ab-{1,2}.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-ooccmp
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/ooc-cmp2-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/ooc-cmp2-head.txt"
git -C /home/user/supdb status --short >> "$Z/ooc-cmp2-head.txt"
date -u +%FT%TZ >> "$Z/ooc-cmp2-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
pairing() {
  name=$1; A=$2; B=$3
  # ONLY="name:sitting ..." runs those sittings alone, to finish a run a
  # restart stopped; unset, every sitting.
  for sitting in 1 2; do
    if [ -n "$ONLY" ] && ! echo " $ONLY " | grep -q " $name:$sitting "; then continue; fi
    if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
    echo "##### $name sitting $sitting: $arms, start $(date -u +%T)"
    env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
      --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
      2>&1 >"$Z/ooc-cmp2-$name-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/ooc-cmp2-$name-ab-$sitting.err"
    if grep -q ' quantities: ' "$Z/ooc-cmp2-$name-ab-$sitting.out"; then echo "$name sitting $sitting: the table is whole"; else echo "$name sitting $sitting: NOT whole"; fi
    rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-ooccmp/supdb-* /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-ooccmp/lmdb-*
  done
  echo "##### $name done $(date -u +%T)"
}
pairing lmdbforced lmdb-nordahead supdb-cache64tofitzpagesdrop
pairing lmdbauto lmdb-nordahead supdb-cache64tofitautozpages
