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
#   test     cargo test (unit and integration, every target)
#   lint     clippy at -D warnings, and the format gate
#   wasm     the browser reader and its floor, built for wasm32-unknown-unknown
#            by web/build.sh
#
# Not here, deliberately: cross-arm, which needs a cross toolchain and qemu
# and so is CI-only; and the falsification suite, which lives in supdb-bench
# together with the claims it checks and the results they are checked against.
# CI runs that suite against every pull request here, with this checkout
# standing in for the bench repository's submodule -- the `bench` job in
# .github/workflows/ci.yml -- so a claim about the engine still cannot go
# stale without a build going red. The browser test went with it, because it
# is an experiment with a recorded expected state, not a build step.
set -eu
cd "$(dirname "$0")/.."

ALL="build test lint wasm"
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
      cargo test --release
      ;;
    lint)
      say "lint"
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      ;;
    wasm)
      say "wasm"
      # Builds the module and the floor and prints their sizes. Whether a
      # size is acceptable is a claim, and claims are gated in supdb-bench;
      # here the check is that the module still links.
      sh web/build.sh
      ;;
    *)
      echo "unknown group: $g" >&2
      echo "groups: $ALL" >&2
      exit 2
      ;;
  esac
done

printf '\nall checks passed: %s\n' "$groups"
