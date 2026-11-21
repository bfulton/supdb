#!/bin/sh
# G1-4 round 2, mutant 8: #[cold] #[inline(never)] dropped from
# willneed_dir_by_piece and record_offset_by_piece; the scratch drop and gate
# probes counted by function under callgrind, the tree against the mutant.
B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4
R=$B/r2
T=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4c
cd /home/user/supdb
(cd $B/keep && sha256sum -c SHA256SUMS) >/dev/null || { echo "keep sums bad"; echo "EXIT 2"; exit 2; }
cmp -s src/blob.rs $B/keep/src_blob.rs || { echo "tree is not keep"; echo "EXIT 2"; exit 2; }
cp $B/g14_gate.rs $B/g14_drop.rs examples/
st=0
cargo build --release --example g14_gate --example g14_drop > $R/b8.tree.log 2>&1 && cp target/release/examples/g14_gate $R/gate.tree && cp target/release/examples/g14_drop $R/drop.tree || st=1
python3 - <<'PY' || st=1
p='/home/user/supdb/src/blob.rs'; s=open(p).read()
for f in ['fn willneed_dir_by_piece', 'fn record_offset_by_piece']:
    o='    #[cold]\n    #[inline(never)]\n    ' + f
    assert s.count(o)==1, f
    s=s.replace(o, '    ' + f)
open(p,'w').write(s)
PY
cargo build --release --example g14_gate --example g14_drop > $R/b8.mut.log 2>&1 && cp target/release/examples/g14_gate $R/gate.mut && cp target/release/examples/g14_drop $R/drop.mut || st=1
cp $B/keep/src_blob.rs src/blob.rs
rm -f examples/g14_gate.rs examples/g14_drop.rs
cmp -s src/blob.rs $B/keep/src_blob.rs && echo restored || st=2
mkdir -p $T
for v in tree mut; do
  valgrind --tool=callgrind --callgrind-out-file=$R/gate8.$v.cg $R/gate.$v $T/g-$v > $R/gate8.$v.txt 2>&1 || st=1
  $R/drop.$v build $T/db-$v > /dev/null 2>&1 || st=1
  valgrind --tool=callgrind --callgrind-out-file=$R/drop8.$v.cg $R/drop.$v merge $T/db-$v $T/tr-$v > $R/drop8.$v.txt 2>&1 || st=1
  for p in gate8 drop8; do callgrind_annotate --threshold=100 --auto=no $R/$p.$v.cg > $R/$p.$v.self 2>&1; callgrind_annotate --threshold=100 --auto=no --tree=caller $R/$p.$v.cg > $R/$p.$v.tree 2>&1; done
done
rm -f $R/gate.tree $R/gate.mut $R/drop.tree $R/drop.mut
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4c
echo "EXIT $st"
