#!/bin/sh
# The second order of zprebuild-5m.sh, on the binary the first sitting ran (b5ea3ae plus the prebuild change), after a container restart cut it.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-zprebuild
mkdir -p "$T"; rm -rf "$T"/*
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
echo "##### zprebuild5m sitting 2 start $(date -u +%T)"
env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms supdb-cache64tofitautozpagesprebuild,supdb-cache64tofitautozpages --size 5000000 --bound 671088640 --ops-div 10 --reps 3 \
  --passes read,read-threaded,scan,scan-threaded,mixes --pairs 'ycsb' \
  2>&1 >"$Z/zprebuild5m-ab-2.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/zprebuild5m-ab-2.err"
if grep -q ' quantities: ' "$Z/zprebuild5m-ab-2.out"; then echo "sitting 2: the table is whole"; else echo "sitting 2: NOT whole"; fi
rm -rf "$T"/*
echo "##### zprebuild s2 done $(date -u +%T)"
