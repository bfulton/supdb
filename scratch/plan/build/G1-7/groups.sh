cd /home/user/supdb
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7
B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-7
run() { name=$1; shift; echo "START $name $(date -u)" > $B/$name.log; "$@" >> $B/$name.log 2>&1; echo "EXIT $?" >> $B/$name.log; echo "$name: $(tail -1 $B/$name.log)"; }
run libdoc cargo test --profile checked --lib --doc --no-fail-fast
run lint sh scripts/check.sh lint
run wasm sh scripts/check.sh wasm
run lists-base env SUPDB_BASE=e9a6abff sh scripts/check.sh lists
run lists sh scripts/check.sh lists
rm -rf /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7/supdb-next-*
run stride sh scripts/check.sh stride
echo ALLDONE
