#!/bin/sh
# Ready a host for the `faster` arm: Microsoft's FASTER, its C++ key-value
# store (github.com/microsoft/FASTER, MIT). It is a comparator like LMDB and
# RocksDB, but it is not a crate and its sources are not this repository's,
# so a host is readied for it as `libclang.sh` readies one for RocksDB's
# bindgen: every workflow that builds the suite calls this, and
# `scripts/deps.sh` does.
#
#   sh scripts/faster.sh
#
# The arm is built on Linux x86-64 alone, the one platform the store
# supports that the suite runs on; anywhere else there is no arm and this
# does nothing. There it installs the libraries the store links where their
# headers are missing (libaio, TBB, uuid), and fetches the store's sources
# at the commit `vendor/faster/PIN` names into `vendor/faster/src`, which
# git ignores, recording the commit beside them in `vendor/faster/COMMIT`.
# `build.rs` holds the sources to the pin and fails the build without them.
# Upstream is no longer maintained, so the pin is its last commit and does
# not move. Needs git and the network, once a host.
set -eu
bench=$(cd "$(dirname "$0")/.." && pwd)
if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
  echo "faster: no arm on $(uname -s) $(uname -m), nothing to ready"
  exit 0
fi

missing=
[ -f /usr/include/libaio.h ] || missing="$missing libaio-dev"
[ -f /usr/include/tbb/concurrent_queue.h ] || missing="$missing libtbb-dev"
[ -f /usr/include/uuid/uuid.h ] || missing="$missing uuid-dev"
if [ -n "$missing" ]; then
  # With the package lists the host has, and fresh ones only where those
  # fail: the engine's scripts/apt.sh, which says why.
  # shellcheck disable=SC2086 # a list of package names
  sh "$bench/../scripts/apt.sh" $missing
fi

dest=$bench/vendor/faster
pin=$(tr -d '[:space:]' < "$dest/PIN")
if [ "$(cat "$dest/COMMIT" 2>/dev/null || true)" = "$pin" ] && [ -f "$dest/src/core/faster.h" ]; then
  echo "faster: sources at $pin"
  exit 0
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git init -q "$tmp"
git -C "$tmp" fetch -q --depth 1 https://github.com/microsoft/FASTER.git "$pin"
git -C "$tmp" checkout -q FETCH_HEAD
got=$(git -C "$tmp" rev-parse HEAD)
if [ "$got" != "$pin" ]; then
  echo "faster: asked for $pin and got $got" >&2
  exit 1
fi
rm -rf "$dest/src" "$dest/COMMIT"
cp -R "$tmp/cc/src" "$dest/src"
cp "$tmp/LICENSE" "$dest/LICENSE"
echo "$pin" > "$dest/COMMIT"
echo "faster: sources at $pin, fetched"
