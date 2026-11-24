#!/bin/sh
# Install the toolchain rust-toolchain.toml names, with its components and
# targets, and any further targets given as arguments. The file is the one
# definition of the version; scripts/deps.sh and every workflow call this,
# so none of them names a version or a channel of its own.
#
#   sh scripts/toolchain.sh                              # what the file names
#   sh scripts/toolchain.sh aarch64-unknown-linux-gnu    # and a target for one job
#
# It ends by holding the active rustc to the file's channel, since "rustup
# ran" is not "the pinned toolchain is the one cargo will use": a
# RUSTUP_TOOLCHAIN in the environment or a directory override outranks the
# file, and says nothing.
set -eu
cd "$(dirname "$0")/.."

if ! command -v rustup >/dev/null; then
  echo "no rustup; install it from https://rustup.rs" >&2
  exit 1
fi
want=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
if [ -z "$want" ]; then
  echo "rust-toolchain.toml names no channel" >&2
  exit 1
fi
# With no argument rustup installs the active toolchain, the file's, with
# the components and targets the file lists. A rustup from before that form
# installs it on first use instead, which the second command is.
rustup toolchain install || rustup show active-toolchain
[ $# -eq 0 ] || rustup target add "$@"

have=$(rustc --version)
case "$have" in
  "rustc $want "*) ;;
  *)
    echo "rust-toolchain.toml names $want and the active rustc is: $have" >&2
    echo "(RUSTUP_TOOLCHAIN or a rustup override outranks the file)" >&2
    exit 1
    ;;
esac
echo "$have"
cargo clippy --version
cargo fmt --version
