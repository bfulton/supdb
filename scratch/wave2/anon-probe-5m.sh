#!/bin/sh
# The process's anonymous memory through the load, the mixes and the
# shuffled load, for the combined arm and the controller's, unbounded, one
# rep after the warmup: the cgroup killed the combined arm at 651 MB in the
# shuffled load's first seconds under 640 MiB whatever the cache's bound,
# 256 MB or 64, so the growth is not the cache's and is read here against
# the passes, `RssAnon` sampled four times a second beside the harness's
# stamped phase marks (`anon-probe.err`, `anon-probe-rss.txt`).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-anonprobe
mkdir -p "$T"; export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms supdb-nocacheautozpages,supdb-cache64tofitautozpages --size 5000000 --ops-div 10 --reps 1 \
  --passes load,mixes --pairs 'ycsb' \
  2>&1 >"$Z/anon-probe.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/anon-probe.err" &
sleep 1
pid=$(pgrep -f "^$BIN ab --arms supdb-nocacheautozpages,supdb-cache64tofitautozpages" | head -1)
echo "sampling pid $pid"
: > "$Z/anon-probe-rss.txt"
while kill -0 "$pid" 2>/dev/null; do
  printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$(grep -E 'RssAnon|RssFile' /proc/$pid/status 2>/dev/null | awk '{printf "%s %s ", $1, $2}')" >> "$Z/anon-probe-rss.txt"
  sleep 0.25
done
wait
echo "##### anon-probe done $(date -u +%T), samples $(wc -l < "$Z/anon-probe-rss.txt")"
rm -rf "$T"/*
