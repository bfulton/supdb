#!/bin/bash
# Block requests per commit by kind (block_rq_issue through tracefs) over short
# runs of every shape at every size, then strace -c over the same. tracefs is
# mounted for the run and unmounted after, as E5 did.
# Usage: tracerun.sh COMMITS
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/fix/probe
C=${1:-300}
TR=/sys/kernel/tracing
T=$W/tmp
mkdir -p $W/raw/trace $W/raw/strace $T
mounted=0
if [ ! -e $TR/trace ]; then mount -t tracefs nodev $TR && mounted=1; fi
SHAPES="A Aw A0 Ap0 B D Ow O0 Tw T0 Rw R0 Cf Cp Ds Dsp J Jp S Sp"
for n in 100 4096 13300 133000; do
  for s in $SHAPES; do
    echo 0 > $TR/tracing_on
    echo > $TR/trace
    echo 16384 > $TR/buffer_size_kb
    echo 1 > $TR/events/block/block_rq_issue/enable
    echo 1 > $TR/events/block/block_rq_complete/enable
    echo 1 > $TR/tracing_on
    E5_KEEP=1 $W/e5 $s $n $C $T tr >> $W/raw/trace/runs.txt
    echo 0 > $TR/tracing_on
    rm -f $T/x* $T/e5-*
    echo 0 > $TR/events/block/block_rq_issue/enable
    echo 0 > $TR/events/block/block_rq_complete/enable
    cat $TR/trace > $W/raw/trace/trace-$s-$n.txt
    echo "$s $n: $(python3 -I $W/tracecount.py $W/raw/trace/trace-$s-$n.txt $C)" >> $W/raw/trace/tracecount.txt
  done
done
echo 0 > $TR/tracing_on
[ $mounted = 1 ] && umount $TR
for n in 100 4096 13300 133000; do
  for s in $SHAPES; do
    strace -c -f -o $W/raw/strace/c-$s-$n.txt $W/e5 $s $n 200 $T st > /dev/null
  done
done
