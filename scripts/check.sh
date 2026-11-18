#!/bin/sh
# Everything this project checks, in one place.
#
#   sh scripts/check.sh          # all of it, as a contributor runs before pushing
#   sh scripts/check.sh lint     # one group
#   sh scripts/check.sh test wasm
#
# CI calls the same groups. That is the whole point: before this existed the
# checks were written down twice, once in a contributor's habits and once in
# .github/workflows/ci.yml, and the two drifted -- CI never built the wasm
# module at all, so a link break in src/wasmapi.rs survived until a local
# toolchain update happened to surface it. A check defined in one place cannot
# be running in one of them and not the other.
#
# Groups:
#   build    the crate, release
#   test     cargo test (unit and integration, every target), optimized
#            with overflow and invariant checks on
#   stride   the tests again with the stride form forced (an EXPERIMENT,
#            `Options::zpages_stride`): every test that forces the compact
#            form or names no form, under SUPDB_TEST_STRIDE (tests/common),
#            and bounded seed ranges of the differential fuzz and of its
#            readers on threads with the form forced on the store
#            (ZFUZZ_STRIDE), each failing a range no stride read reached,
#            and the same two with the store's partitions asked to change
#            form as they run (ZFUZZ_CONVERT), each failing a range in
#            which none left the stride form.
#            The test group's binaries, so it builds nothing of its own. Its
#            own group because a switch that only a hand ran checked nothing
#            between two hand runs: the adaptations it made of other tests,
#            and the stride form under threads and under a checkpoint, ran
#            in no gate
#   lint     clippy at -D warnings, and the format gate
#   wasm     the browser reader and its floor, built for wasm32-unknown-unknown
#            by web/build.sh
#   bench    the benchmark suite in bench/: its build, tests and lint, through
#            its own scripts/check.sh. Builds the comparators, which is a
#            ten-minute C++ build the first time and cached after
#   quick    one quick-scale measurement of every arm, the gate against
#            bench/runs/, and the figures. Not in the default set: it is a
#            timing run and needs the machine to itself
#
# Not here, deliberately: cross-arm, which needs a cross toolchain and qemu
# and so is CI-only.
set -eu
cd "$(dirname "$0")/.."

ALL="build test stride lint wasm bench"
# Lowercase because `GROUPS` is a built-in bash variable holding the current
# user's group ids. Assigning to it does not take, so on any host where
# /bin/sh is bash -- macOS ships bash 3.2 as /bin/sh -- this loop would have
# iterated over group ids instead of over check groups. dash, which is
# /bin/sh on the Linux runners, has no such variable and hid it.
groups="${*:-$ALL}"

say() { printf '\n=== %s ===\n' "$1"; }

for g in $groups; do
  case "$g" in
    build)
      say "build"
      cargo build --release
      ;;
    test)
      say "test"
      cargo test --profile checked
      ;;
    stride)
      say "stride"
      SUPDB_TEST_STRIDE=1 cargo test --profile checked --no-fail-fast
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 \
        cargo test --profile checked --test fuzz concurrent_range -- --exact
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 \
        cargo test --profile checked --test fuzz concurrent_range -- --exact
      ;;
    lint)
      say "lint"
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      ;;
    wasm)
      say "wasm"
      # Builds the module and the floor under -D warnings and prints their
      # sizes; the check is that the module still links, and builds clean.
      sh web/build.sh
      ;;
    bench)
      say "bench"
      sh bench/scripts/check.sh build test lint figures
      ;;
    quick)
      say "quick"
      sh bench/scripts/check.sh quick
      ;;
    *)
      echo "unknown group: $g" >&2
      echo "groups: $ALL" >&2
      exit 2
      ;;
  esac
done

printf '\nall checks passed: %s\n' "$groups"
