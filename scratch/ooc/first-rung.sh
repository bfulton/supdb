#!/bin/sh
# Part B: the first bounded rung. 3M is the in-core control, 10M under 1 GiB is out of core for the
# arms whose heap fits (the default supdb's unbounded forms are refused at 10M by the design; it runs at 3M only).
S=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad
cd /home/user/supdb/bench
rm -rf "$S"/tmp-price/supdb-next-*
E="env -u SUPDB_LAG_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_LAG_MARK -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u SUPDB_SCAN_OPS"
echo "== prove red"; $E TMPDIR=$S/tmp-price sh scripts/ooc.sh --prove-red; echo "prove-red rc=$?"
echo "== row A: 3M control with every arm, then 10M for the arms that fit, 1 GiB, reads divided by 10"
$E TMPDIR=$S/tmp-price sh scripts/ooc.sh 1073741824 --top 3000000 --bottom 3000000 --reps 1 --ops-div 10 --arms supdb-nocache,supdb-cache256,supdb-noadvice,lmdb,lmdb-nordahead --out $S/ooc/rowA3m; rc3=$?; echo "3M rc=$rc3"
# The default alone at 3M, in a row of its own: its forms over the lag sweep reach 960 MB at this rung,
# so under the gibibyte it reads with its page cache reclaimed under them, or is killed; either is the finding.
$E TMPDIR=$S/tmp-price sh scripts/ooc.sh 1073741824 --top 3000000 --bottom 3000000 --reps 1 --ops-div 10 --arms supdb --out $S/ooc/rowA3m-default; echo "3M default rc=$?"
# The 10M rung only behind a banked 3M: a rung killed under the bound says the bound, not the engine.
if [ "$rc3" -eq 0 ]; then
  $E TMPDIR=$S/tmp-price sh scripts/ooc.sh 1073741824 --top 10000000 --bottom 10000000 --reps 1 --ops-div 10 --arms supdb-nocache,supdb-cache256,supdb-noadvice,lmdb,lmdb-nordahead --out $S/ooc/rowA10m; echo "10M rc=$?"
else
  echo "10M skipped: the 3M control did not bank"
fi
echo "done $(date -u +%T)"
