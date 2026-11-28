#!/bin/sh
# The concurrency tests under ThreadSanitizer, nightly only (outside
# scripts/check.sh), with the standard library rebuilt under the sanitizer
# and a target directory of its own.
#
#   sh release/findings/unsafe/tsan.sh build
#   sh release/findings/unsafe/tsan.sh run TARGET [FILTER]
#
# TARGET: lib, or an integration test's name (share, shared, settle,
# jobs_mapped, jobs_pooled, fuzz). Logs to release/findings/unsafe/out/.
set -u
cd "$(dirname "$0")/../../.."
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/supdb-tsan-target}
export RUSTFLAGS="-Zsanitizer=thread -Cforce-frame-pointers=yes"
export RUSTDOCFLAGS="-Zsanitizer=thread"
export TSAN_OPTIONS=${TSAN_OPTIONS:-"halt_on_error=0 report_signal_unsafe=0 second_deadlock_stack=1"}
host=x86_64-unknown-linux-gnu
out=release/findings/unsafe/out
mkdir -p "$out"
cmd=$1; shift
case $cmd in
  build)
    cargo +nightly test -Zbuild-std --target $host --profile checked --no-run \
      --lib --test share --test shared --test settle --test jobs_mapped \
      --test jobs_pooled --test fuzz 2>&1 | tail -3
    ;;
  run)
    t=$1; f=${2:-}
    sel="--test $t"; [ "$t" = lib ] && sel=--lib
    log=$out/tsan-$t$(echo "$f" | tr -c 'a-zA-Z0-9_\n' '_').log
    timeout ${TSAN_SECS:-3600} cargo +nightly test -Zbuild-std --target $host \
      --profile checked $sel --no-fail-fast -- $f --test-threads=2 > "$log" 2>&1
    r=$?
    echo "$t ${f:-all}: exit $r, $(grep -c '^WARNING: ThreadSanitizer' "$log") reports, $(grep -m1 '^test result' "$log")"
    ;;
esac
