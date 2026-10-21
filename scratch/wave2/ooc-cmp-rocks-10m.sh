#!/bin/sh
# RocksDB as tuned against the forced form, out of core. Under 640 MiB at
# five million keys the cgroup killed the harness in both orders at 653 MB
# of anonymous memory in RocksDB's load (`ooc-cmp-rocksforced-ab-*.err`,
# the kernel's record in the stage note): the tuned shape configures a
# 256 MB block cache and four write buffers of 128 MB, 768 MB before the
# harness, which no bound under a gibibyte holds. So ten million keys
# under a gibibyte, the suite's own bounded rung, two pairs after the
# warmup, one sitting a run (`sitting` the argument), since the container
# has restarted inside bounded runs past fifty minutes; each sitting
# writes `ooc-cmp-rocks10m-ab-<sitting>.*`.
sitting=${1:-1}
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=/tmp/claude-0/-home-user-supdb/7a8bc057-f313-5ed1-917a-533bb60079da/scratchpad/tmp-rocks10m
mkdir -p "$T"
export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$Z/ooc-cmp-rocks10m-head-$sitting.txt"
date -u +%FT%TZ >> "$Z/ooc-cmp-rocks10m-head-$sitting.txt"
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
A=rocksdb-tuned; B=supdb-nocachezpages
if [ "$sitting" = 1 ]; then arms=$A,$B; else arms=$B,$A; fi
echo "##### rocks10m sitting $sitting: $arms, start $(date -u +%T)"
env $UNSET SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 "$BIN" ab --arms "$arms" --size 10000000 --bound 1073741824 --ops-div 10 --reps 2 \
  --passes load,read,read-threaded,scan,scan-threaded,mixes --pairs 'scan' \
  2>&1 >"$Z/ooc-cmp-rocks10m-ab-$sitting.out" | while IFS= read -r line; do printf '%s %s\n' "$(cut -d' ' -f1 /proc/uptime)" "$line"; done > "$Z/ooc-cmp-rocks10m-ab-$sitting.err"
if grep -q ' quantities: ' "$Z/ooc-cmp-rocks10m-ab-$sitting.out"; then echo "sitting $sitting: the table is whole"; else echo "sitting $sitting: NOT whole"; fi
rm -rf "$T"/*
echo "##### rocks10m sitting $sitting done $(date -u +%T)"
