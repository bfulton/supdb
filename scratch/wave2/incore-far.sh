#!/bin/sh
# In core: the two far-from-reads gates against the arms that restore each,
# on the loads and the passes after them, twelve pairs a sitting, the arms
# named in the order given by $1 (ab = default first, ba = arm first).
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad
Z=/home/user/supdb/scratch/wave2
BIN=/home/user/supdb/bench/target/release/bench
T=$S/tmp-pair; mkdir -p "$T"; export TMPDIR="$T"
cd /home/user/supdb/bench || exit 2
UNSET="-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
ord=$1
run() { # base arm size passes
  if [ "$ord" = ab ]; then arms="$1,$2"; else arms="$2,$1"; fi
  o=$Z/far-$2-$3-$ord
  echo "##### $arms $3 $4 start $(date -u +%T)"
  env $UNSET "$BIN" ab --arms "$arms" --size $3 --reps 12 --passes $4 > $o.out 2> $o.err
  echo "rc $? $(tail -1 $o.out)"
  rm -rf "$T"/*
}
run supdb supdb-sealcopyfar 30000 load,read,scan,lag
run supdb supdb-sealcopyfar 300000 load,read,scan,lag
run supdb supdb-sealcopyfar 1000000 load,scan
run supdb supdb-sealcopyfar 3000000 load,scan
run supdb supdb-ranksfar 300000 load,scan,lag
run supdb supdb-ranksfar 1000000 load,scan
run supdb supdb-ranksfar 3000000 load,scan
run supdb-ingest supdb-ingestsealcopyfar 30000 load,read,scan,lag
run supdb-ingest supdb-ingestsealcopyfar 300000 load,read,scan,lag
run supdb-ingest supdb-ingestsealcopyfar 1000000 load,scan
echo "##### done $(date -u +%T)"
