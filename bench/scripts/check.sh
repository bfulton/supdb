#!/bin/sh
# The checks. CI calls this script with these names, so a green run here is
# a green run there.
#
#   sh scripts/check.sh              # every group
#   sh scripts/check.sh lint quick   # some groups
#
# build  the crate, release
# test   unit tests, under the release profile with the arithmetic and the
#        invariants checked -- see [profile.checked] in Cargo.toml
# lint   clippy -D warnings, rustfmt --check, the shell inside the workflows
# figures the committed figures under docs/figures/ are what `bench figures`
#        draws from runs/ (scripts/figures.sh --check)
# quick  a quick-scale run of every arm, written under runs-ci/ (ignored),
#        then the gate against runs/ and the figures; proves the runner,
#        the gate and the renderer work end to end on this host. A timing
#        run: nothing else may be running on the machine
#
# Every gate this repository has broken has broken the same way: a check
# that was not running, or one reporting a verdict it had not earned. This
# script is the definition of the suite's checks; the engine's scripts/check.sh
# calls it by group name and adds nothing.
set -eu
cd "$(dirname "$0")/.."

groups="${*:-build test lint figures quick}"
say() { printf '\n== %s ==\n' "$1"; }

for g in $groups; do
  case "$g" in
    build)
      say build
      cargo build --release
      ;;
    test)
      say test
      # The library, the binary and the test targets: the examples hold no
      # tests, and each links RocksDB into a binary of a third of a
      # gigabyte, twenty of them, which filled this host's disk allowance;
      # `lint` compiles every one of them under clippy (`--all-targets`).
      cargo test --profile checked --lib --bins --tests
      ;;
    lint)
      say lint
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      sh scripts/workflows.sh
      python3 scripts/policy_defaults.py
      ;;
    figures)
      say figures
      cargo build --release --bin bench
      sh scripts/figures.sh --check
      ;;
    quick)
      say quick
      cargo build --release --bin bench
      rm -rf runs-ci
      ./target/release/bench run --scale quick --out runs-ci
      # Then the gate against the committed series. With no rows yet for
      # this class it says so and passes; the first regression it catches
      # is the day the series earns its keep.
      ./target/release/bench gate runs-ci/quick/*.json --runs runs
      # And the figures, from the row just written, so the renderer is
      # exercised on every host the checks run on.
      ./target/release/bench figures --runs runs-ci --out runs-ci/figures --scale quick
      ;;
    *)
      echo "unknown group: $g (build test lint figures quick)" >&2
      exit 2
      ;;
  esac
done
echo
echo "all checks passed: $groups"
