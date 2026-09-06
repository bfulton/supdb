#!/bin/sh
# The checks. CI calls this script with these names, so a green run here is
# a green run there.
#
#   sh scripts/check.sh              # every group
#   sh scripts/check.sh lint quick   # some groups
#
# build  the crate, release
# test   unit tests
# lint   clippy -D warnings, rustfmt --check, the shell inside the workflows
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

groups="${*:-build test lint quick}"
say() { printf '\n== %s ==\n' "$1"; }

for g in $groups; do
  case "$g" in
    build)
      say build
      cargo build --release
      ;;
    test)
      say test
      cargo test --release
      ;;
    lint)
      say lint
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      sh scripts/workflows.sh
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
      echo "unknown group: $g (build test lint quick)" >&2
      exit 2
      ;;
  esac
done
echo
echo "all checks passed: $groups"
