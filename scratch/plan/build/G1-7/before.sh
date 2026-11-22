cd /home/user/supdb
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7
echo "START $(date -u)"
echo "== HEAD $(git rev-parse HEAD); the tree: the change in place"
echo "== doctests"
cargo test --profile checked --doc > $1.doc 2>&1; s=$?
tail -60 $1.doc; echo "doctest status $s"
echo "== seal-red"
sh scripts/seal-red.sh > $1.seal 2>&1; s=$?
tail -40 $1.seal; echo "seal-red status $s"
echo EXIT 0
