#!/bin/sh
# The same probe under the 640 MiB bound: unbounded, both arms peak in the
# shuffled load at 320-380 MB of anonymous memory, forty apart, and the
# bound killed the combined arm there at 651 MB, so the growth is the
# bound's own -- the loads and the merges slower under it, and what they
# hold held longer. The combined arm dies in its warmup rep's shuffled
# load, which ends the run; the controller's arm, first in the order, has
# run its own shuffled load by then, so both curves are read
# (`anon-probe640.err`, `anon-probe640-rss.txt`).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-anonprobe
mkdir -p "$T"; export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms supdb-nocacheautozpages,supdb-cache64tofitautozpages --size 5000000 --bound 671088640 --ops-div 10 --reps 1 \
  --passes load,mixes --pairs 'ycsb' \
  2>&1 >"$Z/anon-probe640.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/anon-probe640.err" &
sleep 1
pid=$(pgrep -f "^$BIN ab --arms supdb-nocacheautozpages,supdb-cache64tofitautozpages" | head -1)
echo "sampling pid $pid"
: > "$Z/anon-probe640-rss.txt"
while kill -0 "$pid" 2>/dev/null; do
  printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$(grep -E 'RssAnon|RssFile' /proc/$pid/status 2>/dev/null | awk '{printf "%s %s ", $1, $2}')" >> "$Z/anon-probe640-rss.txt"
  sleep 0.25
done
wait
echo "##### anon-probe640 done $(date -u +%T), samples $(wc -l < "$Z/anon-probe640-rss.txt")"
rm -rf "$T"/*
