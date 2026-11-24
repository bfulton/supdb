#!/bin/sh
# Make this host ready for scripts/check.sh. A fresh container is the case it
# is for: one came with Rust, rustfmt and clippy, no wasm32 target, and
# libclang only as versioned files, so the wasm group and every build of the
# comparators failed before compiling a line of this repository. Each group
# checks first and installs only what is missing, so running it again, or at
# every session start, costs seconds.
#
#   sh scripts/deps.sh              # what every group of scripts/check.sh needs
#   sh scripts/deps.sh profile      # the profilers CLAUDE.md's recipes use
#
# rust      the toolchain rust-toolchain.toml names, with rustfmt, clippy
#           and the wasm32 and macOS targets (every group); the one
#           definition is scripts/toolchain.sh, which CI calls too
# libclang  bindgen's libclang, for the comparators (bench, quick): a check
#           that the host has one with clang's own headers, and an install
#           only where it has not; the one definition is
#           bench/scripts/libclang.sh, which CI calls too
# faster    FASTER's sources at the pinned commit and the libraries they
#           link, for the suite's `faster` arm (bench, quick; Linux x86-64
#           alone); the one definition is bench/scripts/faster.sh
# profile   perf and valgrind; not in the default set
#
# It ends with one line, `all deps ready: <groups>`, which is the verdict a
# caller gates on.
set -eu
cd "$(dirname "$0")/.."

groups="${*:-rust libclang faster}"
say() { printf '\n== %s ==\n' "$1"; }
# A container runs as root and may have no sudo; anywhere else, ask for it.
if [ "$(id -u)" = 0 ]; then root=; else root=sudo; fi
apt_install() {
  if ! command -v apt-get >/dev/null; then
    echo "no apt-get here; install by hand: $*" >&2
    exit 1
  fi
  $root apt-get update -qq >/dev/null
  $root apt-get install -y -qq --no-install-recommends "$@" >/dev/null
}

for g in $groups; do
  case "$g" in
    rust)
      say rust
      # The toolchain rust-toolchain.toml names, with its components and
      # targets, and not whatever toolchain the host came with: a container
      # kept the stable it was made with, CI installed the stable of the
      # day, and a lint green under the first was red under the second.
      sh scripts/toolchain.sh
      ;;
    libclang)
      say libclang
      # bindgen loads libclang when RocksDB's build script runs
      # (`bindgen-runtime`, bench/Cargo.toml) and finds it by itself, so a
      # build needs no variable on either system and none is kept; the
      # script says whether the host has what that load needs and installs
      # it only where it has not. While the build script linked libclang
      # instead, clang-sys kept the directory it had first found one in for
      # as long as its output was cached, and a build whose directory had
      # since gone failed at RocksDB's build script, "unable to find library
      # -lclang"; this group cleaned such an output. A library opened by
      # its path at run time leaves no directory in any output.
      sh bench/scripts/libclang.sh
      ;;
    faster)
      say faster
      # FASTER's sources at the pinned commit and the libraries they link,
      # for the suite's `faster` arm; the one definition is
      # bench/scripts/faster.sh, which CI calls too. Linux x86-64 alone.
      sh bench/scripts/faster.sh
      ;;
    profile)
      say profile
      # This guest's kernel has no linux-tools package of its own, and the
      # 6.8 binary samples fine against it (CLAUDE.md, Profiling).
      perf=/usr/lib/linux-tools-6.8.0-31/perf
      [ -x "$perf" ] || apt_install linux-tools-6.8.0-31
      command -v valgrind >/dev/null || apt_install valgrind
      "$perf" --version
      valgrind --version
      ;;
    *)
      echo "unknown group: $g" >&2
      echo "groups: rust libclang faster profile" >&2
      exit 2
      ;;
  esac
done

printf '\nall deps ready: %s\n' "$groups"
