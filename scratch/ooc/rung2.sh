#!/bin/sh
# RocksDB under the same gibibyte at 3M and 10M keys: the comparator that compresses, so its
# footprint is the one the compact-form thesis is measured against. Reads divided by 10, one rep.
S=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad
cd /home/user/supdb/bench
rm -rf "$S"/tmp-price/supdb-next-*
E="env -u SUPDB_LAG_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_LAG_MARK -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u SUPDB_SCAN_OPS"
$E TMPDIR=$S/tmp-price sh scripts/ooc.sh 1073741824 --top 3000000 --bottom 3000000 --reps 1 --ops-div 10 --arms rocksdb-tuned --out $S/ooc/rung2-3m; rc3=$?; echo "3M rc=$rc3 $(date -u +%T)"
if [ "$rc3" -eq 0 ]; then
  rm -rf "$S"/tmp-price/supdb-next-*
  $E TMPDIR=$S/tmp-price sh scripts/ooc.sh 1073741824 --top 10000000 --bottom 10000000 --reps 1 --ops-div 10 --arms rocksdb-tuned --out $S/ooc/rung2-10m; echo "10M rc=$? $(date -u +%T)"
else
  echo "10M skipped: the 3M control did not bank"
fi
echo "done $(date -u +%T)"
