#!/bin/bash
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/page
P="--bound 671088640 --passes read --ops-div 1"
cd /home/user/supdb; [ -z "$(git status --porcelain)" ] || { echo DIRTY; exit 1; }
BIN=$W/bench-p4080 sh $W/sit.sh r0 supdb-cipher supdb-nozpages 5000000 1 $P
for a in xdyn94 x520 x480; do BIN=$W/bench-page sh $W/sit.sh $a supdb-cipher$a supdb-nozpages 5000000 1 $P; done
BIN=$W/bench-page sh $W/sit.sh pdef supdb-cipher supdb-nozpages 5000000 1 $P
for a in x550 xdyn85 x432; do BIN=$W/bench-page sh $W/sit.sh $a supdb-cipher$a supdb-nozpages 5000000 1 $P; done
echo CHAIN1 DONE
