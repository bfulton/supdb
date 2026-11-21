B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-5
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5
cd /home/user/supdb
(date -u +"START %FT%TZ"; sh scripts/check.sh lint lists; echo "EXIT $?") > $B/lint2.log 2>&1
(date -u +"START %FT%TZ"; sh scripts/check.sh wasm; echo "EXIT $?") > $B/wasm2.log 2>&1
(date -u +"START %FT%TZ"; cargo test --profile checked --no-fail-fast; echo "EXIT $?") > $B/after2.log 2>&1
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5
mkdir -p /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5
echo GATES-DONE > $B/gates2.done
