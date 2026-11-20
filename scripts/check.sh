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
#   stride   the tests again with the stride form forced (an EXPERIMENT,
#            `Options::zpages_stride`): every test that forces the compact
#            form or names no form, under SUPDB_TEST_STRIDE (tests/common),
#            and bounded seed ranges of the differential fuzz and of its
#            readers on threads with the form forced on the store
#            (ZFUZZ_STRIDE), each failing a range no stride read reached,
#            and the same two with the store's partitions asked to change
#            form as they run (ZFUZZ_CONVERT), each failing a range in
#            which none left the stride form.
#            The test group's binaries, so it builds nothing of its own. Its
#            own group because a switch that only a hand ran checked nothing
#            between two hand runs: the adaptations it made of other tests,
#            and the stride form under threads and under a checkpoint, ran
#            in no gate
#   lint     clippy at -D warnings, the format gate, and on Linux the
#            door (scripts/door: the library's file, mapping and advice
#            calls by symbol) with its red input
#   wasm     the browser reader and its floor, built for wasm32-unknown-unknown
#            by web/build.sh, and the browser's decrypter held under Node to
#            the envelope the Rust tests hold the writer to
#   bench    the benchmark suite in bench/: its build, tests and lint, through
#            its own scripts/check.sh. Builds the comparators, which is a
#            ten-minute C++ build the first time and cached after
#   quick    one quick-scale measurement of every arm, the gate against
#            bench/runs/, and the figures. Not in the default set: it is a
#            timing run and needs the machine to itself
#   cost     what a store in the clear pays an operation, by function:
#            scripts/cost-red.sh (the count shown red on a known-red input,
#            green on the tree against itself and on a probe-only change),
#            then scripts/cost.sh against COST_BASE (default origin/main)
#            over the probes COST_PROBES names (default all of them), which
#            fails where a hot function gains past its slack: in a probe
#            with a toggle, more than 64 instructions over the probe's whole
#            run in any function built from source (libc's: more than 2 an
#            operation and 5% of its own count), and in a probe with none,
#            more than a hundredth of an instruction an operation in any
#            function of the engine, and a function a `~` line of
#            scripts/cost-hot.txt names past the bound the line gives it
#            (scripts/cost-diff.py). Not in the
#            default set: it builds the suite's probes twice more and runs
#            them under callgrind, most of an hour, and needs valgrind
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

ALL="build test stride lint wasm bench"
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
    stride)
      say "stride"
      SUPDB_TEST_STRIDE=1 cargo test --profile checked --no-fail-fast
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 \
        cargo test --profile checked --test fuzz concurrent_range -- --exact
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 \
        cargo test --profile checked --test fuzz concurrent_range -- --exact
      ;;
    lint)
      say "lint"
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      # The door, by symbol, and its red input (scripts/door/run.sh).
      say "door"
      sh scripts/door/run.sh || exit 1
      ;;
    wasm)
      say "wasm"
      # Builds the module and the floor under -D warnings and prints their
      # sizes; the check is that the module still links, and builds clean.
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
    cost)
      say "cost"
      # The red input first: a count that has not failed where it must
      # has earned no verdict on the head.
      sh scripts/cost-red.sh || exit 1
      # shellcheck disable=SC2086
      sh scripts/cost.sh "${COST_BASE:-origin/main}" ${COST_PROBES:-} || exit 1
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
        a_directory_under_one_key_refuses_a_store_under_another \
        a_store_kept_one_way_is_refused_the_other; do
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
