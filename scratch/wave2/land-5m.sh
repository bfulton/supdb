#!/bin/sh
# The combined arm with the drop (`supdb-cache64tofitautozpagesdrop`)
# against itself with builds ahead at landing from a percent of the budget
# (`...dropland<pct>`), five million keys under 640 MiB, the reads divided
# by ten, the controller's trace on. ARMS and REPS and TAG from the
# environment; writes `land-<tag>.{out,err}`, the err lines stamped with
# the uptime.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-land5m
mkdir -p "$T"
export TMPDIR="$T"
git -C /home/user/supdb rev-parse --short HEAD > "$Z/land-$TAG-head.txt"
git -C /home/user/supdb status --short >> "$Z/land-$TAG-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
echo "##### $TAG: $ARMS reps $REPS, start $(date -u +%T)"
env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$ARMS" --size 5000000 --bound 671088640 --ops-div 10 --reps "$REPS" \
  --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
  2>&1 >"$Z/land-$TAG.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/land-$TAG.err"
if grep -q ' quantities: ' "$Z/land-$TAG.out"; then echo "$TAG: the table is whole"; else echo "$TAG: NOT whole"; fi
rm -rf /tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad/tmp-land5m/supdb-*
echo "##### $TAG done $(date -u +%T)"
