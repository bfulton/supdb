#!/bin/sh
# The whole browser test, from a clean tree.
#
#   web/test/run.sh [lines] [events]
#
# Builds the wasm reader, writes two real indexes and the answers the native
# reader gives for them, runs the Node unit half (the error paths: sentinel
# normalization, the corrupt-block checksum throw -- web/test/node.mjs), then
# opens the indexes in Chromium: the day index over an OPFS synchronous
# access handle and over an in-memory copy, and the segment index over a
# caching byte source backed by ranged HTTP with a cache smaller than the
# file (R6). Requires every answer to match and the cache to have fetched
# less than the object. Exits non-zero if any assertion fails.
set -eu

cd "$(dirname "$0")/../.."
lines="${1:-20000}"
events="${2:-12000}"

sh web/build.sh ci
cargo build --release --bin logshed
./target/release/logshed fixture --dir web/test/out --lines "$lines"
./target/release/logshed segment --dir web/test/out --events "$events"
node web/test/node.mjs
node web/test/browser.mjs
