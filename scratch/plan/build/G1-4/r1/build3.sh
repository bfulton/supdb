B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4
cd /home/user/supdb
cargo build --release --example g14_gate --example g14_drop 2>&1 | grep -E "^(error|warning)|Finished" && cp target/release/examples/g14_gate $B/r1/gate.tree && cp target/release/examples/g14_drop $B/r1/drop.tree \
&& git show 869a4fee:src/blob.rs > src/blob.rs && git show 869a4fee:src/flatindex.rs > src/flatindex.rs \
&& cargo build --release --example g14_gate --example g14_drop 2>&1 | grep -E "^(error|warning)|Finished" && cp target/release/examples/g14_gate $B/r1/gate.base && cp target/release/examples/g14_drop $B/r1/drop.base
st=$?
cp $B/keep/src_blob.rs src/blob.rs; cp $B/keep/src_flatindex.rs src/flatindex.rs
(cd $B/keep && sha256sum src_blob.rs src_flatindex.rs) ; sha256sum src/blob.rs src/flatindex.rs
echo "EXIT $st"
