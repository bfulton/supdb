#!/bin/sh
# The design's 7.4 again: the forced form's arm against the controller's
# (`supdb-nocachezpages` against `supdb-nocacheautozpages`), at five
# million keys under 640 MiB -- the first two sittings ran under 512 MiB
# with the model's hash population the whole key section, records
# included, and are a measurement of that controller; the bound is 640
# MiB now, under which the harness itself fits at this rung -- three pairs
# after the warmup, the reads divided by ten, two sittings with the arms
# swapped; the harness's phase marks on for both and every stderr line
# stamped, so each sitting's trace reads against its passes. Three pairs
# and not six: the container restarted twice inside bounded pairings past
# fifty minutes of saturated I/O and never inside a shorter run, and a run
# that dies writes no table, so a sitting is kept under half an hour and a
# verdict that needs more pairs takes a second process. The tables of the
# 512 MiB sittings are `auto-ooc-forced5m-ab-{1,2}.*`; this run writes
# `auto-ooc-forced5m640s-ab-{1,2}.*`.
# `auto-ooc-forced5m640-ab-{1,2}.*` are the 640 MiB sittings with the
# populations right and the sampler's estimate 1.7x the file; this run,
# with the sample's fills chained, writes `auto-ooc-forced5m640s-ab-{1,2}.*`.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-aforced5m
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/auto-ooc-forced5m640s-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/auto-ooc-forced5m640s-head.txt"
date -u +%FT%TZ >> "$Z/auto-ooc-forced5m640s-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=supdb-nocachezpages; B=supdb-nocacheautozpages
for sitting in 1 2; do
  if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
  echo "##### sitting $sitting: $arms, start $(date -u +%T)"
  env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
    --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
    2>&1 >"$Z/auto-ooc-forced5m640s-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/auto-ooc-forced5m640s-ab-$sitting.err"
  if grep -q ' quantities: ' "$Z/auto-ooc-forced5m640s-ab-$sitting.out"; then echo "sitting $sitting: the table is whole"; else echo "sitting $sitting: NOT whole"; fi
  rm -rf "$T"/*
done
echo "##### done $(date -u +%T)"
