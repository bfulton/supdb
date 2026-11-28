#!/bin/sh
# Everything this project checks, in one place.
#
#   sh scripts/check.sh          # all of it, as a contributor runs before pushing
#   sh scripts/check.sh lint     # one group
#   sh scripts/check.sh test wasm
#
# CI calls the same groups, and nothing else: every check a workflow that
# runs on a change's event makes (.github/workflows/ci.yml, and
# apple-silicon.yml for the machine group) is `sh scripts/check.sh <group>`,
# its other steps install or prepare, and tests/source.rs holds each such
# workflow to that and all of them to running every group below or saying
# why not; every other workflow under .github/workflows is one it lists as
# checking no change, held to triggers that are no change's, and a file it
# does not list is red. It reads every key of such a workflow by the whole
# path to it, not the commands alone (a job named
# `inputs` is a job): the triggers are the ones it lists, by value, so no
# change goes unchecked for want of an event; an environment entry is one it
# lists, name and value (the event's values, a person's input); an action's
# input is one it lists; no step or job carries a condition but the
# publishing step's `if: always()`, and none a `continue-on-error`; a key
# it does not know, an anchor or a flow mapping is a step it cannot read,
# and red. That is the whole
# point: before this existed the checks were written down twice, once in a
# contributor's habits and once in .github/workflows/ci.yml, and the two
# drifted -- CI never built the wasm module at all, so a link break in
# src/wasmapi.rs survived until a local toolchain update happened to surface
# it. A check defined in one place cannot be running in one of them and not
# the other.
#
# Groups:
#   build    the crate, release
#   test     cargo test (unit and integration, every target), optimized
#            with overflow and invariant checks on, and a bounded seed
#            range of the differential fuzz with handles that write
#            (ZFUZZ_SHARE), failing a range whose stores never went to the
#            shared table and back
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
#   onefile  the tests again with their stores kept in one file (an
#            EXPERIMENT, `Options::one_file`): every test that takes its
#            options from tests/common, under SUPDB_TEST_ONEFILE, so that
#            its seals, merges, form builds and opens go through the
#            store's data file, but the tests of a directory's own files,
#            which say so (`common::files`); and bounded seed ranges of
#            the differential fuzz and of its readers on threads with the
#            `z` store in one file beside `r` in files (ZFUZZ_ONEFILE),
#            in the clear, with the block cache's paths, and with the
#            compact form forced, each failing a range in which no store
#            kept a file in its data file. The test group's binaries. Its
#            own group for the stride group's reason: a switch only a
#            hand ran holds nothing between two hand runs
#   lint     clippy at -D warnings, for this host and for the macOS target
#            (the library alone from a host that is not a Mac), the format
#            gate, on Linux the door (scripts/door: the
#            library's file, mapping and advice calls by symbol) with its
#            red input, and the seal's red input (scripts/seal-red.sh: the
#            whole borrows of the read path refused by the compiler outside
#            their readers)
#   wasm     the browser reader and its floor, built for wasm32-unknown-unknown
#            by web/build.sh, and the browser's decrypter held under Node to
#            the envelope the Rust tests hold the writer to
#   bench    the benchmark suite in bench/: its build, tests and lint, through
#            its own scripts/check.sh. Builds the comparators, which is a
#            ten-minute C++ build the first time and cached after
#   quick    one quick-scale measurement of every arm, the gate against
#            bench/runs/, and the figures. Not in the default set: it is a
#            timing run and needs the machine to itself
#   lists    the lists that only go down (tests/data's, registered in
#            scripts/ratchet.py) held against the commit the change is made
#            on: scripts/ratchet-red.sh (the check shown red and green on
#            scratch repositories), then scripts/ratchet.sh, which fails a
#            list grown past what scripts/raises.txt declares. Needs the
#            history back to the base (no shallow clone) and python3
#   cost     what a store in the clear pays an operation, by function:
#            scripts/cost-red.sh (the count shown red on a known-red input,
#            green on the tree against itself and on a probe-only change),
#            then scripts/cost.sh against the base scripts/base.sh decides,
#            COST_BASE the explicit override (default the merge-base of HEAD
#            and origin/main), over the probes COST_PROBES names (default all of them), which
#            fails where a hot function gains past its slack: in a probe
#            with a toggle, more than 64 instructions over the probe's whole
#            run in any function built from source (libc's: more than 2 an
#            operation and 5% of its own count, or its `~` line's bound),
#            and in a probe with none,
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
#   cross-arm  the library's unit tests built for aarch64 Linux and run
#            under qemu-user, with a linker, a runner and `--cfg
#            supdb_emulated` (qemu-user answers `madvise` advice with
#            success and passes it to no kernel, so the unit tests that read
#            the kernel's answer are compiled out; the native ARM leg runs
#            them), and with backtraces off: qemu-user 8.2 fakes
#            /proc/self/maps from its own record of the guest's mappings
#            and dies reading it ("QEMU internal SIGSEGV") while another
#            thread maps or unmaps, and a panic's backtrace is symbolized
#            by reading that file, so under RUST_BACKTRACE every panic --
#            a test's on purpose or a real failure's -- beside the other
#            tests' mappings killed the run (tests/source.rs holds the
#            library's own code to reading no such file and making no
#            backtrace); the run is held to having filtered no test out,
#            since a word after `--lib` is a filter and a run of none is
#            green. Not in the default set: it needs the aarch64 target,
#            aarch64-linux-gnu-gcc and qemu-aarch64-static, which
#            `rustup target add aarch64-unknown-linux-gnu` and `apt-get
#            install gcc-aarch64-linux-gnu qemu-user-static` give
#   enc      the encrypted comparators (bench/Cargo.toml, feature `enc`):
#            the suite built with LMDB's 1.0 line behind its page hook and
#            RocksDB on its encrypted Env, which stand beside `supdb-cipher`
#            as what a user who needs a store encrypted would otherwise
#            pick; linted, their tests each required by name, and every arm
#            of the encrypted roster run once at the smallest rung, through
#            bench/scripts/check.sh. Not in the default set: it is a second
#            build of the comparators, ten minutes of C++ the first time,
#            and needs OpenSSL's headers (`sh scripts/deps.sh openssl`)
#   machine  the suite's machine capture on an Apple Silicon host: `bench
#            machine` read its line size and read 128. Not in the default
#            set: it is that host's alone, and refuses any other.
#            .github/workflows/apple-silicon.yml runs it on a macOS runner
#            where a pull request changes the capture
#
# A check that compares the tree with the commit the change is made on (the
# lists, the cost) takes that commit from scripts/base.sh alone, which
# decides it from an override and the event's values CI passes, the same
# way on a contributor's clone.
#
# Every group runs under the toolchain rust-toolchain.toml names, which
# scripts/deps.sh installs here and scripts/toolchain.sh in CI.
set -eu
cd "$(dirname "$0")/.."

ALL="build test stride onefile lint wasm bench lists"
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
      # Every binary, whichever fails: a red one stops a plain `cargo
      # test` there, and the macOS leg's first run of this branch's tests
      # showed one binary's failures and nothing of the binaries after it.
      cargo test --profile checked --no-fail-fast
      # The differential fuzz with handles that write, every seed's log
      # held with no frames: the live table goes to the shared one and
      # back many times a seed, the block cache on in every seed, and a
      # range that reached no flip fails.
      ZFUZZ_SHARE=all ZFUZZ_CACHE=all ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
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
    onefile)
      say "onefile"
      # The group's stores in a temp directory of its own, removed where
      # the group passes. A store in one file reserves its data file's
      # growth from the device, so a test's store of a few hundred
      # kilobytes holds a megabyte or three where a store of files holds
      # what it wrote: the group's stores were three gigabytes beside the
      # test group's one, and the first run of them left a test binary
      # dead of a full disk. Spelled as the kernel resolves it, since the
      # tests that find a store's mappings match /proc/self/maps by the
      # store's directory. A group that fails leaves its stores to be read.
      one_tmp=$(mktemp -d "${TMPDIR:-/tmp}/supdb-onefile.XXXXXX")
      one_tmp=$(cd "$one_tmp" && pwd -P)
      TMPDIR="$one_tmp" SUPDB_TEST_ONEFILE=1 \
        cargo test --profile checked --no-fail-fast
      TMPDIR="$one_tmp" ZFUZZ_ONEFILE=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      TMPDIR="$one_tmp" ZFUZZ_ONEFILE=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 \
        cargo test --profile checked --test fuzz concurrent_range -- --exact
      TMPDIR="$one_tmp" ZFUZZ_ONEFILE=1 ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1 \
        ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      TMPDIR="$one_tmp" ZFUZZ_ONEFILE=1 ZFUZZ_ZPAGES=1 ZFUZZ_FROM=0 ZFUZZ_TO=24 \
        cargo test --profile checked --test fuzz fuzz_range -- --exact
      rm -rf "$one_tmp"
      ;;
    lint)
      say "lint"
      cargo clippy --release --all-targets -- -D warnings
      # The macOS side of the tree's cfg guards, type-checked: every target
      # -- the library, its binaries, its unit and integration tests, its
      # examples -- as the macOS leg of CI lints them, which no Linux lint
      # can stand in for. Clippy links nothing, so the target's rust-std,
      # which rust-toolchain.toml lists, is what Rust needs; the tests' and
      # the examples' cipher (`ring`, a dev-dependency) compiles C in its
      # build script, which clang builds for the target here against
      # stand-ins for the four headers it reads of Apple's and the C
      # library's (scripts/macos-headers), freestanding, its objects never
      # linked. The library and its binaries alone were checked here for a
      # while, and the pull request's first macOS legs failed on what that
      # left out: two test seams in `vault` under `cfg(test)` alone that
      # call a module only Linux compiles, then an example's and two test
      # files' Linux calls (`posix_fadvise`, `syncfs`, `kcmp`, a thread's
      # rusage) with no guard. Skipped where the host is that target and
      # the line above was this one.
      mac=aarch64-apple-darwin
      if [ "$(rustc -vV | sed -n 's/^host: //p')" != "$mac" ]; then
        cc_mac=$(command -v clang || ls -d /usr/lib/llvm-*/bin/clang 2>/dev/null | sort -V | tail -1)
        [ -n "$cc_mac" ] || {
          echo "lint: no clang to build the macOS target's C with (apt: clang)" >&2
          exit 1
        }
        CC_aarch64_apple_darwin="$cc_mac" \
          CFLAGS_aarch64_apple_darwin="-ffreestanding -I$PWD/scripts/macos-headers" \
          cargo clippy --release --all-targets --target "$mac" -- -D warnings
      fi
      sh scripts/fmt.sh --check
      # The door, by symbol, and its red input (scripts/door/run.sh).
      say "door"
      sh scripts/door/run.sh || exit 1
      # The seal: the read path's whole borrows take tokens only their
      # readers make, and the compiler refuses them elsewhere
      # (scripts/seal-red.sh, the red input that shows it does).
      say "seal"
      sh scripts/seal-red.sh || exit 1
      # ... and the red input of that red input: each of its checks made to
      # fail by a line of its own (scripts/seal-red-red.sh).
      sh scripts/seal-red-red.sh || exit 1
      # The cost group's red input edits the tree at named text; that each
      # text is still there is read here, in a second, since the group
      # itself runs where CI runs it and nowhere a change is gated before
      # it is pushed (scripts/cost-red.sh, COST_RED_ANCHORS).
      say "cost-red anchors"
      COST_RED_ANCHORS=1 sh scripts/cost-red.sh || exit 1
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
    enc)
      say "enc"
      sh bench/scripts/check.sh enc
      ;;
    cost)
      say "cost"
      # The red input first: a count that has not failed where it must
      # has earned no verdict on the head.
      sh scripts/cost-red.sh || exit 1
      base=$(SUPDB_BASE="${COST_BASE:-${SUPDB_BASE:-}}" sh scripts/base.sh) || exit 1
      echo "cost: the base is $base"
      # shellcheck disable=SC2086
      sh scripts/cost.sh "$base" ${COST_PROBES:-} || exit 1
      ;;
    lists)
      say "lists"
      # The red input first, as the cost group's: a check that has not
      # failed where it must has earned no verdict on the tree.
      sh scripts/ratchet-red.sh || exit 1
      sh scripts/ratchet.sh || exit 1
      # Every commit after the same base carries a signature
      # (scripts/signed.sh), its red input first.
      sh scripts/signed.sh --red || exit 1
      sh scripts/signed.sh || exit 1
      ;;
    cross-arm)
      say "cross-arm"
      for t in aarch64-linux-gnu-gcc qemu-aarch64-static; do
        command -v "$t" >/dev/null 2>&1 || {
          echo "cross-arm: no $t: apt-get install gcc-aarch64-linux-gnu qemu-user-static" >&2
          exit 2
        }
      done
      out=$(CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64-static -L /usr/aarch64-linux-gnu" \
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="--cfg supdb_emulated" \
        RUST_BACKTRACE=0 \
        cargo test --profile checked --target aarch64-unknown-linux-gnu --lib 2>&1) || {
        printf '%s\n' "$out"
        exit 1
      }
      printf '%s\n' "$out"
      # A word after `--lib` is a filter, and a run that filtered every
      # test out is green by its exit status: the run has to have run some
      # and filtered none.
      printf '%s\n' "$out" | grep -q '^test result: ok\. [1-9][0-9]* passed; 0 failed; [0-9]* ignored; 0 measured; 0 filtered out' || {
        echo "cross-arm: the run did not run every test it compiled" >&2
        exit 1
      }
      ;;
    machine)
      say "machine"
      # The suite's machine capture, read on an Apple Silicon host: the
      # detector once read a /sys path macOS does not have and defaulted to
      # a 64-byte line where the lines are 128. `bench machine` exits
      # non-zero where it defaulted the line size, and the capture has to
      # say it read 128.
      [ "$(uname -s) $(uname -m)" = "Darwin arm64" ] || {
        echo "machine: an Apple Silicon host's check, and this is $(uname -s) $(uname -m)" >&2
        exit 2
      }
      (cd bench && cargo build --release --bin bench) || exit 1
      m=$(./bench/target/release/bench machine) || {
        printf '%s\n' "$m"
        exit 1
      }
      printf '%s\n' "$m"
      for want in '"cache_line_detected": true' '"cache_line": 128'; do
        printf '%s\n' "$m" | grep -qF "$want" || {
          echo "machine: the capture does not say $want" >&2
          exit 1
        }
      done
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
      echo "groups: $ALL (the default), quick cost fscrypt cross-arm machine enc" >&2
      exit 2
      ;;
  esac
done

printf '\nall checks passed: %s\n' "$groups"
