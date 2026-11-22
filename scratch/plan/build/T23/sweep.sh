#!/bin/sh
# sweep.sh TAG RUNS WIDTHS...: the stride group's concurrent fuzz with conversions on,
# run RUNS times at each scratch width (ms the segment work parks between its own turns; the engine parks 10), one at a time.
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T23
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T23
BIN=$(ls -t /home/user/supdb/target/checked/deps/fuzz-* | grep -v '\.d$' | head -1)
tag=$1; runs=$2; shift 2
out=$D/sweep-$tag.log
: > $out
echo "binary $BIN" >> $out
for w in "$@"; do
  red=0; i=0
  while [ $i -lt $runs ]; do
    i=$((i+1))
    mkdir -p $T
    log=$D/sweep-$tag-w$w-r$i.log
    TMPDIR=$T ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 SUPDB_SCRATCH_MAINT_POLL_MS=$w \
      $BIN concurrent_range --exact > $log 2>&1
    st=$?
    if [ $st -ne 0 ]; then red=$((red+1)); grep -h -o 'FAIL: seed [0-9]*\|moved it in and out ([0-9, ]*) times\|panicked at [^:]*:[0-9]*' $log | tr '\n' ' ' >> $out; echo "  <- w=$w run $i exit $st" >> $out; else rm -f $log; fi
    rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T23
  done
  echo "width ${w}ms: $red red of $runs" >> $out
done
echo EXIT 0 >> $out
