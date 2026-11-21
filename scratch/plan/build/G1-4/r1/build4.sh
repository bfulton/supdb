B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4
cd /home/user/supdb
(cd $B/keep && sha256sum -c SHA256SUMS) >/dev/null || { echo "keep sums bad"; exit 2; }
cmp -s src/blob.rs $B/keep/src_blob.rs || { echo "tree is not keep"; exit 2; }
cargo build --release --example g14_gate --example g14_drop --example g14_count 2>&1 | grep -E "^(error|warning)|Finished" && cp target/release/examples/g14_gate $B/r1/gate.tree && cp target/release/examples/g14_drop $B/r1/drop.tree && cp target/release/examples/g14_count $B/r1/count.tree || { echo "tree build failed"; exit 2; }
python3 - <<'PY'
p='/home/user/supdb/src/blob.rs'; s=open(p).read()
o='    #[cfg(not(target_family = "wasm"))]\n    #[cold]\n    #[inline(never)]\n    fn willneed_ranks_by_piece'
assert s.count(o)==1; s=s.replace(o,'    #[cfg(not(target_family = "wasm"))]\n    fn willneed_ranks_by_piece'); open(p,'w').write(s)
PY
cargo build --release --example g14_gate --example g14_count 2>&1 | grep -E "^(error|warning)|Finished" && cp target/release/examples/g14_gate $B/r1/gate.x7 && cp target/release/examples/g14_count $B/r1/count.x7
st=$?
cp $B/keep/src_blob.rs src/blob.rs
cmp -s src/blob.rs $B/keep/src_blob.rs && echo restored
echo "EXIT $st"
