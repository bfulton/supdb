#!/bin/sh
# G1-4 r1: each probe at base, r0 (the header words) and the tree, under callgrind,
# the functions' self instructions and calls (callgrind_annotate --tree=caller).
R=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r1
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4
mkdir -p $T
for v in ${VERS:-base r0 tree}; do
  valgrind --tool=callgrind --callgrind-out-file=$R/count.$v.cg $R/count.$v $T/count-$v > $R/count.$v.txt 2>&1 || echo "FAIL count $v"
  rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4/count-$v
  $R/drop.$v build $T/dropbase-$v > $R/drop.$v.build.txt 2>&1 || echo "FAIL dropbuild $v"
  valgrind --tool=callgrind --callgrind-out-file=$R/drop.$v.cg $R/drop.$v merge $T/dropbase-$v $T/droptrial-$v > $R/drop.$v.txt 2>&1 || echo "FAIL drop $v"
  rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4/dropbase-$v /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4/droptrial-$v
  for p in count drop; do
    callgrind_annotate --threshold=100 $R/$p.$v.cg > $R/$p.$v.self 2>&1
    callgrind_annotate --threshold=100 --tree=caller $R/$p.$v.cg > $R/$p.$v.tree 2>&1
  done
done
echo EXIT 0
