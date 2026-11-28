#!/bin/sh
# The library's pure modules' unit tests under Miri, one test per process
# under a timeout, JOBS at a time, nightly only (outside scripts/check.sh),
# in a target directory of its own.
#
#   sh release/findings/unsafe/miri.sh [MODULE_PREFIX...]
#
# MIRIFLAGS is taken from the environment (default: isolation off, so the
# tests may read the clock and the environment). MIRI_SECS (default 600)
# bounds each test. Each test's log is release/findings/unsafe/out/miri/
# <test>.log; release/findings/unsafe/out/miri-summary.txt has one line a
# test: pass, fail(exit), timeout.
set -u
cd "$(dirname "$0")/../../.."
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/supdb-miri-target}
export MIRIFLAGS=${MIRIFLAGS:--Zmiri-disable-isolation}
out=$PWD/release/findings/unsafe/out
mkdir -p "$out/miri"
mods=${*:-db::zlz4:: flatindex:: block:: blob:: bytes:: index:: reserve:: db::radix:: db::lockfree:: db::snap_arena:: db::put_index:: crypt::}
cargo +nightly miri test --lib --no-run 2>&1 | tail -1
list=$(cargo +nightly miri test --lib -- --list 2>/dev/null | sed -n 's/: test$//p')
: > "$out/miri-summary.txt"
for m in $mods; do echo "$list" | grep "^$m"; done | xargs -P "${JOBS:-3}" -I{} sh -c '
  t={}; f='"$out"'/miri/$(echo "$t" | tr ":" "_").log
  timeout '"${MIRI_SECS:-600}"' cargo +nightly miri test --lib -- --exact "$t" > "$f" 2>&1
  r=$?
  case $r in 0) v=pass ;; 124) v=timeout ;; *) v="fail($r)" ;; esac
  grep -q "1 passed" "$f" || [ $v != pass ] || v="pass-but-no-test-ran"
  echo "$v $t" >> '"$out"'/miri-summary.txt
'
sort "$out/miri-summary.txt" | awk "{print \$1}" | uniq -c
