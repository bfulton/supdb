#!/bin/sh
# The format gate. Nothing is exempt.
#
#   sh scripts/fmt.sh --check   # what CI runs
#   sh scripts/fmt.sh           # apply
#
# This is `cargo fmt --all` with one thing added, and that one thing is the
# reason the script exists rather than the command. Two different failures
# hide behind a nonzero status: "formatting differs", which prints `Diff in`
# lines, and "could not run" -- no toolchain, a parse error, a rustfmt crash --
# which prints none. An earlier form swallowed both with `|| true`, making the
# second a gate that reported green for never having run. They are told apart
# here and both fail.
set -eu
cd "$(dirname "$0")/.."

if [ "${1:-}" = "--check" ]; then
  out=$(cargo fmt --all -- --check 2>&1) && exit 0 || status=$?
  printf '%s\n' "$out"
  echo
  if printf '%s\n' "$out" | grep -q "^Diff in "; then
    echo "run 'sh scripts/fmt.sh' to fix"
  else
    echo "cargo fmt exited $status without reporting a diff: it did not run"
  fi
  exit "$status"
fi

cargo fmt --all
