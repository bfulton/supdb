#!/bin/sh
# The whole browser test, from a clean tree.
#
#   web/test/run.sh [lines]
#
# Builds the wasm reader, writes a real day index and the answers the native
# reader gives for it, then opens both in Chromium -- once over an OPFS
# synchronous access handle and once over an in-memory copy -- and requires
# every answer to match. Exits non-zero if any assertion fails.
set -eu

cd "$(dirname "$0")/../.."
lines="${1:-20000}"

sh web/build.sh ci
cargo build --release --bin logshed
./target/release/logshed fixture --dir web/test/out --lines "$lines"
node web/test/browser.mjs
