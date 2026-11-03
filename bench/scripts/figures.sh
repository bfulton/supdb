#!/bin/sh
# The committed figures: `docs/figures/<scale>/` is what `bench figures`
# draws from `runs/` for every scale with rows, class by class, with
# `latest/` the newest row's class for the README's paths. A figure is a
# function of the rows and of the renderer, so the committed set is held to
# both by `--check`, which draws the set again into a scratch directory and
# fails where the committed one differs; a row's banking commit redraws it.
#
#   sh scripts/figures.sh            # draw into ../docs/figures, replacing what is there
#   sh scripts/figures.sh --check    # fail where ../docs/figures is not what runs/ draws
#
# Gated on explicit tests and never on `set -e`, which this environment's
# shell walks past (CLAUDE.md).
cd "$(dirname "$0")/.." || exit 2
bin=./target/release/bench
if [ ! -x "$bin" ]; then
  cargo build --release --bin bench || exit 2
fi
out=../docs/figures
tmp=$(mktemp -d) || exit 2
trap 'rm -rf "$tmp"' EXIT
for scale in quick full ooc; do
  [ -d "runs/$scale" ] || continue
  "$bin" figures --runs runs --out "$tmp/$scale" --scale "$scale" >/dev/null || {
    echo "figures: drawing the $scale figures failed" >&2
    exit 1
  }
done
if [ "${1:-}" = "--check" ]; then
  if [ ! -d "$out" ]; then
    echo "figures: $out is missing; run sh bench/scripts/figures.sh" >&2
    exit 1
  fi
  if diff -r "$tmp" "$out" >"$tmp.diff" 2>&1; then
    rm -f "$tmp.diff"
    echo "figures: docs/figures is what runs/ draws"
    exit 0
  fi
  echo "figures: docs/figures is not what runs/ draws; run sh bench/scripts/figures.sh and commit" >&2
  head -n 20 "$tmp.diff" >&2
  rm -f "$tmp.diff"
  exit 1
fi
rm -rf "$out" && mkdir -p "$out" && cp -R "$tmp"/. "$out"/ || exit 2
echo "figures: drawn into $out ($(find "$out" -name '*.svg' | wc -l | tr -d ' ') figures)"
