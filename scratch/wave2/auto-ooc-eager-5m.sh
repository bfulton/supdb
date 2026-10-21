#!/bin/sh
# The eager entry priced under a bound (the controller's arm against the same arm buying the form at SUSPECT on the model's prediction, without the rent), at five million keys under 512 MiB, half the ten-million run's I/O a rep, since the container restarted inside three bounded runs at ten million and the floors and the harness do not fit the 256 MiB floor at three million (the cgroup killed that run in its first two minutes): the controller's arm against
# the same arm with the form's threads at the readers' I/O class
# (`supdb-nocacheautozpagesfairio`), at ten million keys under a gibibyte,
# three pairs after the warmup, the reads divided by ten, two sittings with
# the arms swapped; the harness's phase marks on for both and every stderr
# line stamped, so each sitting's trace reads against its passes. Three pairs
# and not six: the container restarted twice inside bounded pairings past
# fifty minutes of saturated I/O and never inside a shorter run, and a run
# that dies writes no table, so a sitting is kept under half an hour and a
# verdict that needs more pairs takes a second process.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-aeager5m
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/auto-ooc-eager5m-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/auto-ooc-eager5m-head.txt"
date -u +%FT%TZ >> "$Z/auto-ooc-eager5m-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=supdb-nocacheautozpages; B=supdb-nocacheautozpageseager
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 536870912 --ops-div 10 --reps 3 \
    --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
    2>&1 >"$Z/auto-ooc-eager5m-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/auto-ooc-eager5m-ab-$sitting.err"
  if grep -q ' quantities: ' "$Z/auto-ooc-eager5m-ab-$sitting.out"; then echo "sitting $sitting: the table is whole"; else echo "sitting $sitting: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
