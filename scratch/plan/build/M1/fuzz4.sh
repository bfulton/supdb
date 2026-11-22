#!/bin/sh
# fuzz4.sh FROM COUNT [ENV...]: the checked fuzz binary built, then four processes of COUNT
# seeds each from FROM, the env given; exits 1 where any range failed.
cd /home/user/supdb || exit 2
from=$1; n=$2; shift 2
bin=$(cargo test --profile checked --test fuzz --no-run 2>&1 | grep -o 'target/checked/deps/fuzz-[0-9a-f]*' | tail -1)
[ -n "$bin" ] || { echo "fuzz4: no binary"; exit 2; }
echo "fuzz4: $bin"
pids=""
for i in 0 1 2 3; do
  f=$((from + i * n)); t=$((f + n))
  env "$@" ZFUZZ_FROM=$f ZFUZZ_TO=$t ./$bin --exact fuzz_range --nocapture > "$TMPDIR/fuzz4-$f.log" 2>&1 &
  pids="$pids $!"
done
rc=0
for p in $pids; do wait $p || rc=1; done
grep -h "FAILURES\|FAIL:\|test result" "$TMPDIR"/fuzz4-*.log | cut -c1-300
rm -f "$TMPDIR"/fuzz4-*.log
exit $rc
