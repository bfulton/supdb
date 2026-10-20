#!/bin/sh
# The bounded pairing's second sitting (the design's 7.4, the arms swapped),
# run again after the container restarted inside its warmup: the same
# invocation as `auto-ooc-pairing.sh forced`'s second sitting, with the
# harness's phase marks on for both arms (`SUPDB_LAG_MARK`, a line at each
# pass's edges) and every stderr line stamped with the monotonic clock as it
# arrives, so the controller's phases read against the passes.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-aooc
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > "$Z/auto-ooc-forced-build.log" 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD >> "$Z/auto-ooc-forced-head.txt"
date -u +%FT%TZ >> "$Z/auto-ooc-forced-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
echo "##### sitting 2: supdb-nocacheautozpages,supdb-nocachezpages, start $(date -u +%T)"
env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms supdb-nocacheautozpages,supdb-nocachezpages --size 10000000 --bound 1073741824 --ops-div 10 --reps 6 \
  --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
  2>&1 >"$Z/auto-ooc-forced-ab-2.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/auto-ooc-forced-ab-2.err"
if grep -q ' quantities: ' "$Z/auto-ooc-forced-ab-2.out"; then echo "sitting 2: the table is whole"; else echo "sitting 2: NOT whole"; fi
rm -rf "$T"/*
echo "##### done $(date -u +%T)"
