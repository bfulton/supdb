#!/bin/sh
# rows.sh OUT: the two rows the charts are drawn from, at the head, one after
# the other and nothing else running: the quick row (the pull-request gate's
# scale: 300 000 keys, five reps) and the full-scale row to three million
# keys (seven reps). The six headline arms: the durable and buffered supdb,
# and one stated configuration of each comparator (bench/DESIGN.md, Arms).
# The supdb-only timers and counters are unset, so every arm runs alike.
OUT=$1
# The stores are removed under $OUT, so $OUT is held to the scratchpad.
case "$OUT" in
  /tmp/claude-0/*/scratchpad/?*) ;;
  *) echo "rows.sh: OUT must be a directory under the session's scratchpad"; exit 2 ;;
esac
BIN=/home/user/supdb/bench/target/release/bench
ARMS=supdb,lmdb,rocksdb-tuned,supdb-ingest,lmdb-nosync,rocksdb-tuned-nosync
UNSET="-u SUPDB_LAG_COUNTERS -u SUPDB_MIX_COUNTERS -u SUPDB_MIX_KINDS -u SUPDB_MIX_EVENTS -u SUPDB_MIX_CENSUS \
-u SUPDB_SCAN_OPS -u SUPDB_THREAD_SPANS -u SUPDB_LOAD_PHASES -u SUPDB_DRAIN_STEPS -u SUPDB_ZPAGES_TRACE \
-u SUPDB_LAG_MARK -u GLIBC_TUNABLES -u MALLOC_ARENA_MAX"
mkdir -p "$OUT/tmp"
cd /home/user/supdb/bench || exit 2
git -C /home/user/supdb rev-parse --short HEAD > "$OUT/head.txt"
git -C /home/user/supdb status --short >> "$OUT/head.txt"
echo "quick start $(date -u +%T)"
env $UNSET TMPDIR="$OUT/tmp" "$BIN" run --scale quick --arms "$ARMS" --out "$OUT" > "$OUT/quick.log" 2>&1
echo "quick exit $? $(date -u +%T)"
rm -rf "$OUT"/tmp/supdb-*
echo "full start $(date -u +%T)"
env $UNSET TMPDIR="$OUT/tmp" "$BIN" run --scale full --top 3000000 --arms "$ARMS" --out "$OUT" > "$OUT/full.log" 2>&1
echo "full exit $? $(date -u +%T)"
rm -rf "$OUT"/tmp/supdb-*
echo "rows done $(date -u +%T)"
