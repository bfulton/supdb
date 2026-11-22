cd /home/user/supdb
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7
echo "START $(date -u) $(git rev-parse HEAD) + the unit's working tree"
cargo test --profile checked --no-fail-fast 2>&1
echo EXIT $?
