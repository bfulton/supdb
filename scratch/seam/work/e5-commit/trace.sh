#!/bin/bash
# block_rq_issue/complete over one short run: trace.sh OUT e5-args...
TR=/sys/kernel/tracing
OUT=$1; shift
echo 0 > $TR/tracing_on
echo > $TR/trace
echo 8192 > $TR/buffer_size_kb
echo 1 > $TR/events/block/block_rq_issue/enable
echo 1 > $TR/events/block/block_rq_complete/enable
echo 1 > $TR/tracing_on
"$@"
echo 0 > $TR/tracing_on
echo 0 > $TR/events/block/block_rq_issue/enable
echo 0 > $TR/events/block/block_rq_complete/enable
cat $TR/trace > $OUT
