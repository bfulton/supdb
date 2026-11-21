R=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r1
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4
mkdir -p $T
st=0
for v in base tree; do
  valgrind --tool=callgrind --callgrind-out-file=$R/gate.$v.cg $R/gate.$v $T/gate-$v > $R/gate.$v.txt 2>&1 || { echo "FAIL gate $v"; st=1; }
  $R/drop.$v build $T/dropbase-$v > $R/drop.$v.build.txt 2>&1 || { echo "FAIL dropbuild $v"; st=1; }
  valgrind --tool=callgrind --callgrind-out-file=$R/drop.$v.cg $R/drop.$v merge $T/dropbase-$v $T/droptrial-$v > $R/drop.$v.txt 2>&1 || { echo "FAIL drop $v"; st=1; }
  G14_AHEAD=1048576 valgrind --tool=callgrind --callgrind-out-file=$R/ahead.$v.cg $R/drop.$v merge $T/dropbase-$v $T/droptrial-$v > $R/ahead.$v.txt 2>&1 || { echo "FAIL ahead $v"; st=1; }
  for p in gate drop ahead; do
    callgrind_annotate --threshold=100 --auto=no $R/$p.$v.cg > $R/$p.$v.self 2>&1
    callgrind_annotate --threshold=100 --auto=no --tree=caller $R/$p.$v.cg > $R/$p.$v.tree 2>&1
  done
done
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4
echo "EXIT $st"
