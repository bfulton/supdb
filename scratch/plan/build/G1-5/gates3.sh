B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-5
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5
cd /home/user/supdb
(date -u +"START %FT%TZ"; cargo test --profile checked --no-fail-fast --test blob --test cipher --test segwriter --test source; echo "EXIT $?") > $B/neighbours2.log 2>&1
(date -u +"START %FT%TZ"; sh scripts/check.sh lint lists; echo "EXIT $?") > $B/lint3.log 2>&1
(cd $B && date -u +"START %FT%TZ" && python3 mutants5.py E8; echo "EXIT $?") > $B/mutants6.log 2>&1
rm -rf -- /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5
mkdir -p /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5
echo done > $B/gates3.done
