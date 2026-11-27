#!/bin/sh
# The checks. CI calls this script with these names, so a green run here is
# a green run there.
#
#   sh scripts/check.sh              # every group
#   sh scripts/check.sh lint quick   # some groups
#
# build  the crate, release
# test   unit tests, under the release profile with the arithmetic and the
#        invariants checked -- see [profile.checked] in Cargo.toml
# lint   clippy -D warnings, rustfmt --check, the shell inside the workflows
# figures the committed figures under docs/figures/ are what `bench figures`
#        draws from runs/ (scripts/figures.sh --check)
# quick  a quick-scale run of every arm, written under runs-ci/ (ignored),
#        then the gate against runs/ and the figures; proves the runner,
#        the gate and the renderer work end to end on this host. A timing
#        run: nothing else may be running on the machine. ARMS=a,b runs
#        those arms alone, and KEYS=ids the row over random ids
# enc    the encrypted comparators (Cargo.toml, feature `enc`): the binary
#        built with LMDB's 1.0 line and RocksDB's encrypted Env in the
#        released LMDB's place, linted, the arms' own tests run and each
#        required by name, and every arm of the encrypted roster opened and
#        run once at the smallest rung. Not in the default set: it is a
#        second build of RocksDB, in a target directory of its own
#        (target-enc) so that the default binary is not replaced under a
#        measurement, and it needs OpenSSL's headers
#
# Every gate this repository has broken has broken the same way: a check
# that was not running, or one reporting a verdict it had not earned. This
# script is the definition of the suite's checks; the engine's scripts/check.sh
# calls it by group name and adds nothing.
set -eu
cd "$(dirname "$0")/.."

groups="${*:-build test lint figures quick}"
say() { printf '\n== %s ==\n' "$1"; }

for g in $groups; do
  case "$g" in
    build)
      say build
      cargo build --release
      ;;
    test)
      say test
      # The library, the binary and the test targets: the examples hold no
      # tests, and each links RocksDB into a binary of a third of a
      # gigabyte, twenty of them, which filled this host's disk allowance;
      # `lint` compiles every one of them under clippy (`--all-targets`).
      cargo test --profile checked --lib --bins --tests
      ;;
    lint)
      say lint
      cargo clippy --release --all-targets -- -D warnings
      sh scripts/fmt.sh --check
      sh scripts/workflows.sh
      python3 scripts/policy_defaults.py
      ;;
    figures)
      say figures
      cargo build --release --bin bench
      sh scripts/figures.sh --check
      ;;
    enc)
      say enc
      # A target directory of its own: the two builds differ in their
      # features, and one written over the other is `lmdb` refused, or the
      # encrypted arms refused, in the middle of somebody's pairing.
      cargo build --release --no-default-features --features enc --target-dir target-enc --bin bench
      cargo clippy --release --no-default-features --features enc --target-dir target-enc --lib --bins -- -D warnings
      out=$(cargo test --profile checked --no-default-features --features enc --target-dir target-enc --lib enc:: 2>&1) || {
        printf '%s\n' "$out"
        exit 1
      }
      printf '%s\n' "$out"
      # A test renamed is a test this group stopped running, and a run of
      # none is green: each is asked for by name.
      for t in \
        the_encrypted_comparators_keep_their_keys_from_the_device \
        the_encrypted_rocksdb_reads_its_table_files \
        the_streams_counter_is_the_iv_plus_the_block \
        the_lmdb_hook_is_aes_256_gcm_written_from_the_input \
        reader_threads_over_an_encrypted_lmdb_read_what_was_written; do
        printf '%s\n' "$out" | grep -q "^test engines::enc::tests::$t \.\.\. ok" || {
          echo "enc: $t did not run" >&2
          exit 1
        }
      done
      # Every arm of the encrypted roster opened and run through every pass
      # once, at the smallest rung: a check that each runs, not a timing --
      # no rate of it is read or kept.
      smoke=$(mktemp -d)
      ./target-enc/release/bench run --scale quick --top 10000 --bottom 10000 --reps 1 --arms enc --out "$smoke" || {
        rm -rf "$smoke"
        exit 1
      }
      rm -rf "$smoke"
      ;;
    quick)
      say quick
      cargo build --release --bin bench
      rm -rf runs-ci
      # `ARMS`, where set, names the arms to run (`ci`'s `arms` input): a
      # reader reproducing a row runs two arms in a fraction of the time.
      # `KEYS`, where set, names the keys' shape (`ci`'s `keys` input,
      # `bench run --keys`): `ids` takes the row of random ids, a series
      # of its own beside the numbered keys'. The loop above runs over
      # `$groups`, so the positional parameters are free to hold the
      # run's options.
      arms="${ARMS:-}"
      keys="${KEYS:-}"
      set -- --scale quick --out runs-ci
      if [ -n "$arms" ]; then
        set -- "$@" --arms "$arms"
      fi
      if [ -n "$keys" ]; then
        set -- "$@" --keys "$keys"
      fi
      ./target/release/bench run "$@"
      # Then the gate against the committed series. With no rows yet for
      # this class it says so and passes; the first regression it catches
      # is the day the series earns its keep.
      ./target/release/bench gate runs-ci/quick/*.json --runs runs
      # And the figures, from the row just written, so the renderer is
      # exercised on every host the checks run on.
      ./target/release/bench figures --runs runs-ci --out runs-ci/figures --scale quick
      ;;
    *)
      echo "unknown group: $g (build test lint figures quick)" >&2
      exit 2
      ;;
  esac
done
echo
echo "all checks passed: $groups"
