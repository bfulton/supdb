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
# rust      rustfmt, clippy and the wasm32-unknown-unknown target (lint, wasm)
# libclang  bindgen's libclang, for the comparators (bench, quick); the one
#           definition is bench/scripts/libclang.sh, which CI calls too
# profile   perf and valgrind; not in the default set
#
# It ends with one line, `all deps ready: <groups>`, which is the verdict a
# caller gates on.
set -eu
cd "$(dirname "$0")/.."

groups="${*:-rust libclang}"
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
      if ! command -v rustup >/dev/null; then
        echo "no rustup; install Rust stable from https://rustup.rs" >&2
        exit 1
      fi
      rustup component add rustfmt clippy
      rustup target add wasm32-unknown-unknown
      ;;
    libclang)
      say libclang
      # The script installs libclang-dev where nothing unversioned exists.
      # On Linux that puts libclang.so where clang-sys searches by itself,
      # so a build needs no variable afterwards and none is kept. macOS
      # needs its variables at every build (an rpath in RUSTFLAGS), so
      # there the line to eval is printed.
      found=$(sh bench/scripts/libclang.sh)
      echo "$found"
      if [ "$(uname -s)" = Darwin ]; then
        echo 'before building: eval "$(sh bench/scripts/libclang.sh)"'
      fi
      # clang-sys's build script declares nothing to rerun on, so the
      # directory it found libclang in at the first build stays in its
      # cached output for good; once that directory is gone, every later
      # build fails at the link of RocksDB's build script ("unable to find
      # library -lclang"). Such an output is cleaned, and the next build
      # finds libclang afresh and compiles RocksDB again.
      for out in bench/target/*/build/clang-sys-*/output; do
        [ -f "$out" ] || continue
        for d in $(sed -n 's/^cargo:rustc-link-search=\(native=\)\{0,1\}//p' "$out"); do
          if ! ls "$d"/libclang*.so* >/dev/null 2>&1 \
            && ! ls "$d"/libclang*.dylib >/dev/null 2>&1; then
            echo "stale: $out names $d, which holds no libclang; cleaning clang-sys"
            (cd bench && cargo clean -p clang-sys --release)
          fi
        done
      done
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
      echo "groups: rust libclang profile" >&2
      exit 2
      ;;
  esac
done

printf '\nall deps ready: %s\n' "$groups"
