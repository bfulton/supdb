#!/bin/sh
# E taken apart under the forced form: the forced arm against LMDB with
# its readahead off at five million keys under 640 MiB, the load and the
# mixes alone, two pairs, with the mixes' timer by kind on (`mixkinds`, the
# scans and the commits apart) and each mix's form events and counts
# (`mix-events`, printed under the mix counters, which the first run of
# this script had unset and so printed none), every stderr line stamped. A diagnostic: its cells are
# not read as a verdict; its lines say where E's twelve microseconds an
# operation go -- the decode where the memo misses, the merge of the
# unsealed head, or the walk.
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-edecomp
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/e-decomp-head.txt"
date -u +%FT%TZ >> "$Z/e-decomp-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
echo "##### e-decomp start $(date -u +%T)"
env $UNSET SUPDB_MIX_COUNTERS=1 SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms lmdb-nordahead,supdb-nocachezpages --size 5000000 --bound 671088640 --ops-div 10 --reps 2 \
  --passes load,mixes --pairs 'ycsb' \
  2>&1 >"$Z/e-decomp-ab.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/e-decomp-ab.err"
echo "##### e-decomp done $(date -u +%T), mixkinds lines: $(grep -c mixkinds "$Z/e-decomp-ab.err"), mix-events lines: $(grep -c "mix-events" "$Z/e-decomp-ab.err")"
rm -rf "$T"/*
