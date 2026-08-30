#!/bin/sh
# Build the browser reader and measure what it costs.
#
# Two artifacts: the module itself, and the *floor* -- an empty cdylib with the
# same standard-library surface, built the same way. The floor is the control.
# A wasm module measured alone cannot say whether it is large because supdb is
# large or because a Rust cdylib starts out large, and those want different
# responses. See `web/floor/Cargo.toml`.
#
# Writes results/w3-bundle.<profile>.json through `logshed bundle`, so the size
# record carries its machine and goes through the same gate as everything else.
#
#   web/build.sh [profile]     profile defaults to ci
set -eu

cd "$(dirname "$0")/.."
profile="${1:-ci}"

echo "# building the reader for wasm32-unknown-unknown"
cargo build --profile wasm --lib --target wasm32-unknown-unknown
wasm=target/wasm32-unknown-unknown/wasm/supdb.wasm

echo "# building the floor"
cargo build --release --target wasm32-unknown-unknown --manifest-path web/floor/Cargo.toml
floor=web/floor/target/wasm32-unknown-unknown/release/supdb_wasm_floor.wasm

# gzip -9, because that is what a CDN serves and the budget is about what the
# user waits for rather than what is on disk.
gz() { gzip -9 -c "$1" | wc -c | tr -d ' '; }
sz() { wc -c < "$1" | tr -d ' '; }

cp "$wasm" web/supdb.wasm
echo "# wrote web/supdb.wasm"

cargo build --release --bin logshed
./target/release/logshed bundle \
  --profile "$profile" \
  --wasm-bytes "$(sz "$wasm")" --wasm-gzip "$(gz "$wasm")" \
  --floor-bytes "$(sz "$floor")" --floor-gzip "$(gz "$floor")"
