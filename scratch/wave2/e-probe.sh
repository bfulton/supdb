#!/bin/sh
# The E mix decomposed, after the out-of-core pairing read it at 1.1x of the
# raw form with its faults down 17x: the same arms with the mixes' timers by
# kind (`SUPDB_MIX_KINDS`: the gets', the scans' and the commits' time apart)
# and the store's counters around each mix (`SUPDB_MIX_COUNTERS`: the
# form's walks, groups and decodes per mix, among the rest), set for both
# arms alike, so nothing of this sitting is a rate to quote -- it says where
# E's time goes and how many groups one of its scans of fifty opens over a
# store the mixes before it have written into. In core at three million
# keys first, then under the bound at ten million, two pairs each after the
# warmup. Needs a free machine (bench/CLAUDE.md).
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-eprobe
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
cargo build --release > /dev/null 2>&1 || { echo "the bench binary did not build"; exit 2; }
git -C /home/user/supdb rev-parse --short HEAD > "$Z/e-probe-head.txt"
date -u +%FT%TZ >> "$Z/e-probe-head.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_LAG_MARK -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
echo "##### in core, 3M: $(date -u +%T)"
env $UNSET SUPDB_MIX_KINDS=1 SUPDB_MIX_COUNTERS=1 "$BIN" ab --arms supdb-nocache,supdb-nocachezpages \
  --size 3000000 --ops-div 10 --reps 2 --passes load,mixes \
  > "$Z/e-probe-3m.out" 2> "$Z/e-probe-3m.err"; echo "exit $?"
rm -rf "$T"/*
echo "##### under the bound, 10M: $(date -u +%T)"
env $UNSET SUPDB_MIX_KINDS=1 SUPDB_MIX_COUNTERS=1 "$BIN" ab --arms supdb-nocache,supdb-nocachezpages \
  --size 10000000 --bound 1073741824 --ops-div 10 --reps 2 --passes load,mixes \
  > "$Z/e-probe-10m.out" 2> "$Z/e-probe-10m.err"; echo "exit $?"
rm -rf "$T"/*
echo "##### done $(date -u +%T)"
