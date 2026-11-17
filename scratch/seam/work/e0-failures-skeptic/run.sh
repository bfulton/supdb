#!/bin/bash
# run.sh MODE NAME [c]  -- write phase under the shim, then read phase plain
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e0-failures-skeptic
BIN=$W/target/debug/e0probe
MODE=$1; NAME=$2; C=$3; PATHSUB=$4
D=$W/tmp/$NAME
rm -rf "$D"
export TMPDIR=$W/tmp
echo "== $MODE $NAME $C (shim on '$PATHSUB')"
SHIM_PATH="$PATHSUB" SHIM_WRITE_ARM=$W/tmp/arm.w SHIM_PARTIAL_ARM=$W/tmp/arm.p SHIM_SYNC_ARM=$W/tmp/arm.s LD_PRELOAD=$W/shim/shim.so $BIN $MODE $D $C 2>&1 | grep -v "^$"
rm -f $W/tmp/arm.w $W/tmp/arm.p $W/tmp/arm.s
ls -la $D | grep -v "^total" | awk '{print "   file", $5, $9}'
