#!/bin/sh
# The eager arm's cancelled builds, traced: the first sitting's two cancels
# were the vanish rule's (`BuildCancelled`), two seconds into the threaded
# read pass, not a merge's supersession, and the trace of that sitting
# carried no census. The same two arms at five million keys under 640 MiB,
# the passes cut to the load, the read pass and the threaded read pass,
# four pairs, with the trace extended to print the census, the evidence,
# the generation and the segment work in flight at every interval of a
# build. A diagnostic, not a pairing: its cells are not read.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-aeagerdiag
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/auto-ooc-eager-diag-head.txt"
date -u +%FT%TZ >> "$Z/auto-ooc-eager-diag-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=supdb-nocacheautozpages; B=supdb-nocacheautozpageseager
echo "##### diag: $A,$B, start $(date -u +%T)"
env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$A,$B" --size 5000000 --bound 671088640 --ops-div 10 --reps 4 \
  --passes load,read,read-threaded --pairs 'read' \
  2>&1 >"$Z/auto-ooc-eager-diag.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/auto-ooc-eager-diag.err"
echo "##### done $(date -u +%T), cancels: $(grep -c BuildCancelled "$Z/auto-ooc-eager-diag.err")"
rm -rf "$T"/*
