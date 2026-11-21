R=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4/r1
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4c
mkdir -p $T
st=0
for v in tree x7; do
  valgrind --tool=callgrind --callgrind-out-file=$R/gate4.$v.cg $R/gate.$v $T/g-$v > $R/gate4.$v.txt 2>&1 || { echo "FAIL gate $v"; st=1; }
  valgrind --tool=callgrind --callgrind-out-file=$R/count4.$v.cg $R/count.$v $T/c-$v > $R/count4.$v.txt 2>&1 || { echo "FAIL count $v"; st=1; }
  for p in gate4 count4; do callgrind_annotate --threshold=100 --auto=no $R/$p.$v.cg > $R/$p.$v.self 2>&1; callgrind_annotate --threshold=100 --auto=no --tree=caller $R/$p.$v.cg > $R/$p.$v.tree 2>&1; done
done
$R/drop.tree build $T/db > /dev/null 2>&1 || st=1
valgrind --tool=callgrind --callgrind-out-file=$R/drop4.tree.cg $R/drop.tree merge $T/db $T/tr > $R/drop4.tree.txt 2>&1 || st=1
G14_AHEAD=1048576 valgrind --tool=callgrind --callgrind-out-file=$R/ahead4.tree.cg $R/drop.tree merge $T/db $T/tr > $R/ahead4.tree.txt 2>&1 || st=1
for p in drop4 ahead4; do callgrind_annotate --threshold=100 --auto=no $R/$p.tree.cg > $R/$p.tree.self 2>&1; done
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4c
echo "EXIT $st"
