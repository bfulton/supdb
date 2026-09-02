#!/bin/sh
# The format gate, minus the modules vendored from the design artifact.
#
# `cargo fmt --all` reformats those modules, which CLAUDE.md forbids:
# `results/baseline/` holds measurements taken against commit 101a4e7 and
# `docs/profiling.md` cites line numbers in `src/store.rs`. So the repository
# cannot hold to `cargo fmt --all -- --check`, and never did -- that gate
# failed on the day the engine was vendored and every day after, which meant
# CI carried a step nobody could make green. This covers everything the
# project writes itself and names the exemptions.
#
#   sh scripts/fmt.sh --check   # what CI runs
#   sh scripts/fmt.sh           # apply
#
# `rustfmt` follows `mod` declarations, so a file list cannot exempt anything:
# formatting `src/lib.rs` formats the whole crate. The check therefore runs
# cargo's own gate and drops the hunks belonging to exempt files, and the
# apply puts those files back byte for byte afterwards.
set -eu
cd "$(dirname "$0")/.."

# Exactly the modules declared with the vendored allow in src/lib.rs.
VENDORED="src/block.rs src/freelist.rs src/index.rs src/flatindex.rs src/keytable.rs src/readers.rs src/store.rs"

if [ "${1:-}" = "--check" ]; then
  # Two different nonzero exits hide behind one status: "formatting differs",
  # which prints `Diff in` lines and is the thing being filtered, and "could
  # not run" -- no toolchain, a parse error, a rustfmt crash -- which prints
  # none. Swallowing both with `|| true` made the second a silent pass, which
  # is a gate that reports green for never having run.
  out=$(cargo fmt --all -- --check 2>&1) && status=0 || status=$?
  if [ "$status" -ne 0 ] && ! printf '%s\n' "$out" | grep -q "^Diff in "; then
    printf '%s\n' "$out"
    echo
    echo "cargo fmt exited $status without reporting a diff: it did not run"
    exit "$status"
  fi
  rest=$(printf '%s\n' "$out" | awk -v vendored="$VENDORED" '
    BEGIN { n = split(vendored, v, " ") }
    /^Diff in / {
      path = $3; sub(/:$/, "", path); sub(/:[0-9]+$/, "", path)
      skip = 0
      for (i = 1; i <= n; i++) if (index(path, v[i]) > 0) skip = 1
    }
    !skip { print }
  ')
  if printf '%s\n' "$rest" | grep -q "^Diff in "; then
    printf '%s\n' "$rest"
    echo
    echo "run 'sh scripts/fmt.sh' to fix; the vendored modules are exempt on purpose"
    exit 1
  fi
  exit 0
fi

# Apply: format everything, then restore the exempt modules exactly as they
# were. Copied aside rather than checked out of git, so an uncommitted edit to
# a vendored module survives running this.
tmp=$(mktemp -d)
for v in $VENDORED; do
  mkdir -p "$tmp/$(dirname "$v")"
  cp "$v" "$tmp/$v"
done
cargo fmt --all
for v in $VENDORED; do
  cp "$tmp/$v" "$v"
done
rm -rf "$tmp"
