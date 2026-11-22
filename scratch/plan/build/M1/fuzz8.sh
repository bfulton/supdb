#!/bin/sh
# fuzz8.sh FROM COUNT [ENV...]: the checked fuzz binary built, then four processes of COUNT
# seeds each from FROM, the env given; exits 1 where any range failed.
cd /home/user/supdb || exit 2
from=$1; n=$2; shift 2
bin=$(cargo test --profile checked --test fuzz --no-run 2>&1 | grep -o 'target/checked/deps/fuzz-[0-9a-f]*' | tail -1)
[ -n "$bin" ] || { echo "fuzz8: no binary"; exit 2; }
echo "fuzz8: $bin"
pids=""
for i in 0 1 2 3 4 5 6 7; do
  f=$((from + i * n)); t=$((f + n))
  env "$@" ZFUZZ_FROM=$f ZFUZZ_TO=$t ./$bin --exact fuzz_range --nocapture > "$TMPDIR/fuzz8-$f.log" 2>&1 &
  pids="$pids $!"
done
rc=0
for p in $pids; do wait $p || rc=1; done
grep -h "FAILURES\|FAIL:\|test result" "$TMPDIR"/fuzz8-*.log | cut -c1-300
cp "$TMPDIR"/fuzz8-*.log "$OUT"/ 2>/dev/null; rm -f "$TMPDIR"/fuzz8-*.log
exit $rc
