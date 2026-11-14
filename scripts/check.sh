#!/bin/sh
# Everything this project checks, in one place.
#
#   sh scripts/check.sh          # all of it, as a contributor runs before pushing
#   sh scripts/check.sh lint     # one group
#   sh scripts/check.sh test wasm
#
# CI calls the same groups. That is the whole point: before this existed the
# checks were written down twice, once in a contributor's habits and once in
# .github/workflows/ci.yml, and the two drifted -- CI never built the wasm
# module at all, so a link break in src/wasmapi.rs survived until a local
# toolchain update happened to surface it. A check defined in one place cannot
# be running in one of them and not the other.
#
# Groups:
#   build    the crate, release
#   test     cargo test (unit and integration, every target), optimized
#            with overflow and invariant checks on
#   lint     clippy at -D warnings, and the format gate
#   wasm     the browser reader and its floor, built for wasm32-unknown-unknown
#            by web/build.sh, and the browser's decrypter held under Node to
#            the envelope the Rust tests hold the writer to
#   bench    the benchmark suite in bench/: its build, tests and lint, through
#            its own scripts/check.sh. Builds the comparators, which is a
#            ten-minute C++ build the first time and cached after
#   quick    one quick-scale measurement of every arm, the gate against
#            bench/runs/, and the figures. Not in the default set: it is a
#            timing run and needs the machine to itself
#   fscrypt  a store under a key, on a filesystem that can encrypt: the
#            ignored half of tests/dirkey.rs, each test named and required
#            to have run. Not in the default set: it needs that filesystem,
#            which scripts/fscrypt.sh makes, with root, and names in
#            SUPDB_FSCRYPT_DIR and SUPDB_FSCRYPT_IMAGE
#
# Not here, deliberately: cross-arm, which needs a cross toolchain and qemu
# and so is CI-only.
set -eu
cd "$(dirname "$0")/.."

ALL="build test lint wasm bench"
# Lowercase because `GROUPS` is a built-in bash variable holding the current
# user's group ids. Assigning to it does not take, so on any host where
# /bin/sh is bash -- macOS ships bash 3.2 as /bin/sh -- this loop would have
# iterated over group ids instead of over check groups. dash, which is
# /bin/sh on the Linux runners, has no such variable and hid it.
groups="${*:-$ALL}"

say() { printf '\n=== %s ===\n' "$1"; }

for g in $groups; do
  case "$g" in
    build)
      say "build"
      cargo build --release
      ;;
    test)
      say "test"
      cargo test --profile checked
      ;;
    lint)
      say "lint"
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      ;;
    wasm)
      say "wasm"
      # Builds the module and the floor and prints their sizes; the check
      # is that the module still links.
      sh web/build.sh
      # The envelope's layout is written down in src/crypt.rs and again in
      # web/crypt.mjs; tests/data/crypt-v1.bin is what holds the two to one
      # answer, and this is the half of that the test group does not run.
      node web/crypt.test.mjs
      ;;
    bench)
      say "bench"
      sh bench/scripts/check.sh build test lint figures
      ;;
    quick)
      say "quick"
      sh bench/scripts/check.sh quick
      ;;
    fscrypt)
      say "fscrypt"
      if [ -z "${SUPDB_FSCRYPT_DIR:-}" ] || [ -z "${SUPDB_FSCRYPT_IMAGE:-}" ]; then
        echo "fscrypt: SUPDB_FSCRYPT_DIR and SUPDB_FSCRYPT_IMAGE are not set. Make them with" >&2
        echo "  sudo sh scripts/fscrypt.sh up /mnt/supdb-fscrypt \"\$(id -u):\$(id -g)\"" >&2
        echo "and export the two lines it prints." >&2
        exit 2
      fi
      out=$(cargo test --profile checked --test dirkey -- --ignored 2>&1) || {
        printf '%s\n' "$out"
        exit 1
      }
      printf '%s\n' "$out"
      # A test renamed or no longer ignored is a test this group stopped
      # running, and a run of none is green: each is asked for by name.
      for t in \
        a_store_under_a_key_is_ciphertext_on_the_device_and_reads_as_itself \
        a_locked_store_is_refused_by_name_and_opens_under_its_key \
        a_directory_under_one_key_refuses_a_store_under_another; do
        printf '%s\n' "$out" | grep -q "^test $t \.\.\. ok" || {
          echo "fscrypt: $t did not run" >&2
          exit 1
        }
      done
      ;;
    *)
      echo "unknown group: $g" >&2
      echo "groups: $ALL" >&2
      exit 2
      ;;
  esac
done

printf '\nall checks passed: %s\n' "$groups"
