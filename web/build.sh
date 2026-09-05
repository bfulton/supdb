#!/bin/sh
# Build the browser reader and measure what it costs.
#
# Two artifacts: the module itself, and the *floor* -- an empty cdylib with the
# same standard-library surface, built the same way. The floor is the control.
# A wasm module measured alone cannot say whether it is large because supdb is
# large or because a Rust cdylib starts out large, and those want different
# responses. See `web/floor/Cargo.toml`.
#
# Prints the four sizes it measures, one per line, as `name bytes`. Recording
# them against a budget is the falsification suite's job, in supdb-bench: this
# script builds the artifact and measures it, and nothing here decides whether
# a number is good.
#
#   web/build.sh
set -eu

cd "$(dirname "$0")/.."

echo "# building the reader for wasm32-unknown-unknown"
# Ask cargo where its target directory is rather than assuming `target/`.
# This repository is a submodule of supdb-bench, and there the workspace root
# -- and so the target directory -- is a level up. A hard-coded path built
# fine and then failed to find its own artifact.
target=$(cargo metadata --format-version 1 --no-deps \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
# An empty parse is not a missing directory, it is this script no longer
# knowing where it is looking -- and it would surface later as a confusing
# "no such file" for the module itself, blaming the build for a failure in
# the question that preceded it.
if [ -z "$target" ]; then
  echo "could not read target_directory out of \`cargo metadata\`; its output" >&2
  echo "format may have changed. Not guessing where the module will be." >&2
  exit 2
fi

# The size settings are given here rather than as a named profile in
# Cargo.toml. Cargo takes profiles from the workspace root, and this
# repository is a submodule of supdb-bench, where the root is a level up --
# so a `[profile.wasm]` here was ignored there, and keeping a second copy in
# the other manifest meant two definitions of one thing with nothing holding
# them equal. As overrides they travel with the only script that wants them.
cargo build --release --lib --target wasm32-unknown-unknown \
  --config 'profile.release.opt-level="z"' \
  --config 'profile.release.lto="fat"' \
  --config 'profile.release.codegen-units=1' \
  --config 'profile.release.panic="abort"' \
  --config 'profile.release.strip=true' \
  --config 'profile.release.debug=false' 
wasm="$target/wasm32-unknown-unknown/release/supdb.wasm"

echo "# building the floor"
cargo build --release --target wasm32-unknown-unknown --manifest-path web/floor/Cargo.toml
floor=web/floor/target/wasm32-unknown-unknown/release/supdb_wasm_floor.wasm

# gzip -9, because that is what a CDN serves and the budget is about what the
# user waits for rather than what is on disk.
gz() { gzip -9 -c "$1" | wc -c | tr -d ' '; }
sz() { wc -c < "$1" | tr -d ' '; }

cp "$wasm" web/supdb.wasm
echo "# wrote web/supdb.wasm"

echo "wasm_bytes $(sz "$wasm")"
echo "wasm_gzip $(gz "$wasm")"
echo "floor_bytes $(sz "$floor")"
echo "floor_gzip $(gz "$floor")"
