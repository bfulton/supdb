#!/bin/sh
# The second sitting of stage 2a's pairing (scratch/wave2/zproof-commands.sh, section 2), the arms swapped:
# column A = supdb-nocachezpages, column B = supdb-nocache, so read B/A here as A/B of the first sitting.
Z=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/wave2
BIN=/home/user/supdb/bench/target/release/bench
mkdir -p /tmp/wave2-zproof-tmp
export TMPDIR=/tmp/wave2-zproof-tmp
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/sitting2-head.txt"
date -u +%FT%TZ >> "$Z/sitting2-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
env $UNSET "$BIN" ab \
    --arms supdb-nocachezpages,supdb-nocache \
    --size 10000000 --bound 1073741824 --ops-div 10 --reps 12 \
    --passes load,read,read-threaded --pairs 'read ' \
  > "$Z/zproof-ab-2.out" 2> "$Z/zproof-ab-2.err"
rc=$?
if [ $rc -eq 0 ] && grep -q ' quantities: ' "$Z/zproof-ab-2.out"; then
  echo "sitting 2: the table is whole (exit $rc)" > "$Z/sitting2-verdict.txt"
else
  echo "sitting 2: NOT whole (exit $rc)" > "$Z/sitting2-verdict.txt"
fi
date -u +%FT%TZ >> "$Z/sitting2-verdict.txt"
