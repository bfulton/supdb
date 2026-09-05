#!/bin/sh
# Everything this project checks, in one place.
#
#   sh scripts/check.sh          # all of it, as a contributor runs before pushing
#   sh scripts/check.sh lint     # one group
#   sh scripts/check.sh test browser
#
# CI calls the same groups. That is the whole point: before this existed the
# checks were written down twice, once in a contributor's habits and once in
# .github/workflows/ci.yml, and the two drifted -- CI never built the wasm
# module at all, so a link break in src/wasmapi.rs survived until a local
# toolchain update happened to surface it. A check defined in one place cannot
# be running in one of them and not the other.
#
# Groups:
#   build    the workspace, release
#   test     cargo test --workspace (unit and integration, every target)
#   lint     clippy at -D warnings, and the format gate
#   browser  the wasm module, the Node error paths and the Chromium suite
#   claims   verify the committed results at both profiles, and redraw figures
#   suites   the falsification, comparison and correctness suites at `ci`,
#            then verify the claims against those fresh results
#
# Not here, deliberately: cross-arm, which needs a cross toolchain and qemu
# and so is CI-only, and `--profile full`, which takes hours and is run by
# hand when a number is going to be cited.
set -eu
cd "$(dirname "$0")/.."

ALL="build test lint browser claims suites"
# Lowercase because `GROUPS` is a built-in bash variable holding the current
# user's group ids. Assigning to it does not take, so on any host where
# /bin/sh is bash -- macOS ships bash 3.2 as /bin/sh -- this loop would have
# iterated over group ids instead of over check groups. dash, which is
# /bin/sh on the Linux runners, has no such variable and hid it.
groups="${*:-$ALL}"

# Where `suites` writes. CI overrides these so it can upload them.
#
# Spelled out rather than `${CHECK_RESULTS:-results-ci}`, because macOS ships
# bash 3.2 as /bin/sh and it mishandles a default expansion of an *unset*
# variable under `set -u`: the script died there having printed nothing, on a
# checkout that passed on both Linux runners. `${VAR+x}` is the portable test
# for "is it set", and the `if` is not an `&&` because a false `&&` would trip
# `set -e`.
RESULTS=results-ci
FIGURES=figures-ci
if [ -n "${CHECK_RESULTS+x}" ]; then RESULTS=$CHECK_RESULTS; fi
if [ -n "${CHECK_FIGURES+x}" ]; then FIGURES=$CHECK_FIGURES; fi

say() { printf '\n=== %s ===\n' "$1"; }

for g in $groups; do
  case "$g" in
    build)
      say "build"
      cargo build --release --workspace
      ;;
    test)
      say "test"
      cargo test --release --workspace
      ;;
    lint)
      say "lint"
      cargo clippy --release --workspace --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      ;;
    browser)
      say "browser"
      sh web/test/run.sh
      ;;
    claims)
      say "claims"
      cargo build --release --bin verify --bin figures
      ./target/release/verify --profile ci
      ./target/release/verify --profile full
      # `dev` too, not because claims are pinned there -- almost none are, so
      # nearly everything skips -- but because verify's results-to-claims
      # direction only sees the profile it is given. Two committed dev records
      # described the retired engine and nothing could see them.
      ./target/release/verify --profile dev
      sh scripts/claimrefs.sh
      # Figures must regenerate from the committed results, so a result whose
      # schema drifted is caught here rather than when someone redraws.
      ./target/release/figures --profile ci --out "$FIGURES-committed"
      test -s "$FIGURES-committed/index.html"
      ;;
    suites)
      say "suites"
      cargo build --release --workspace
      ./target/release/internal all --profile ci --out "$RESULTS"
      ./target/release/external kv   --profile ci --out "$RESULTS"
      ./target/release/external ycsb --profile ci --keys 10000 --ops 10000 --out "$RESULTS"
      ./target/release/correctness all --profile ci --out "$RESULTS"
      # The browser suite's experiments run here too. They used to be run by
      # hand, and the committed records drifted from the engine without
      # anything noticing: a fixture whose index measured 686,506 bytes when
      # it was recorded measures 1,112,132 today, and `verify` was comparing
      # claims against the old recording and reporting green. An experiment
      # that is not in this script is an experiment nobody runs.
      ./target/release/browser budget --profile ci --out "$RESULTS"
      ./target/release/browser ranges --profile ci --out "$RESULTS"
      ./target/release/browser dict   --profile ci --out "$RESULTS"
      ./target/release/browser waves  --profile ci --out "$RESULTS"
      ./target/release/verify --profile ci --results "$RESULTS"
      ./target/release/figures --profile ci --results "$RESULTS" --out "$FIGURES"
      ;;
    *)
      echo "unknown group: $g" >&2
      echo "groups: $ALL" >&2
      exit 2
      ;;
  esac
done

printf '\nall checks passed: %s\n' "$groups"
