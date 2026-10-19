#!/bin/sh
# The forms lead's counts: the default at 3M with the lag counters on, one rep, whole scan counts,
# to read blocks built and the forms' bytes per lag point beside the memory. Run after the rung.
S=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad
cd /home/user/supdb/bench
rm -rf "$S"/tmp-price/supdb-next-*
env -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_LAG_MARK -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u SUPDB_SCAN_OPS SUPDB_LAG_COUNTERS=1 TMPDIR=$S/tmp-price \
  ./target/release/bench run --scale quick --top 3000000 --bottom 3000000 --reps 1 --arms supdb --out $S/ooc/formscount/row 2> $S/ooc/formscount/stderr.log
echo "rc=$? $(date -u +%T)"; echo "all done"
