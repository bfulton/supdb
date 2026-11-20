#!/bin/sh
# The door, by symbol (forms.md R1, gate test G-door), as the lint group of
# scripts/check.sh runs it: no file, mapping, advice, sync or process read in
# the library outside the modules that carry its allow, with
# tests/source.rs holding where an allow may stand.
#
# The config is in its own directory and the run is over the library alone,
# so tests, examples and bench, which read stored files by path on purpose,
# see none of it; under the checked profile, so the code under
# `cfg(debug_assertions)` is linted too; into its own target directory, so a
# run under another config never answers for it. A config missing or
# misread lints nothing and passes the run; its red input (red.sh) fails it,
# run by the same command (scripts/door/lint.sh) over the same config
# directory, in a directory of its own that has to be empty after it.
#
# Linux only: the libc names are Linux's, and the CI Linux legs (x86_64 and
# arm64) are the gate. What this build compiles out -- code under a `cfg`
# that does not hold for the library under `checked` on Linux x86_64, the
# other targets' twins, `not(debug_assertions)` -- the lint never sees;
# tests/source.rs counts each such `cfg` (tests/data/door.txt, `cfgout`),
# bars every path the config names as text inside it, and fails a `cfg`
# that names `clippy`, which this run alone sets.
set -u
cd "$(dirname "$0")/../.."
if [ "$(uname -s)" != Linux ]; then
  echo "door: skipped, not Linux"
  exit 0
fi
. scripts/door/lint.sh
# The library is linted at every run: clippy names its config in the
# build's dependencies only where it found one, so a result cached from a
# run that found none would answer for the config forever after.
cargo clean --profile checked --target-dir target/door -p supdb -q || exit 1
# A config entry that resolves to nothing, or to the wrong kind of thing,
# is a warning clippy prints and `-D warnings` does not make an error: it
# would bar nothing and pass. door_run fails on it.
# The config's directory, named once: the run and its red input take the
# same one, so a run pointed at a directory with no clippy.toml (where
# clippy would look above it, find none and lint with nothing barred)
# fails the red input's copy of it.
conf="$PWD/scripts/door"
door_run "$PWD" "$conf" "$PWD/target/door" || exit 1
# The red input's copy stands inside the repository, so cargo reads the same
# configuration files above it as the run did (a `.cargo/config.toml` that
# silenced the lint would silence the red input too) and the same
# environment; it has to be empty after.
mkdir -p target/door || exit 1
red="$(mktemp -d "$PWD/target/door/red.XXXXXX")" || exit 1
TMPDIR="$red" sh scripts/door/red.sh "$conf"
status=$?
left="$(ls -A "$red")"
rm -rf "$red"
[ "$status" -eq 0 ] || exit 1
[ -z "$left" ] || {
  echo "door: the red input left $left behind" >&2
  exit 1
}
echo "door: ok"
