# The door's command and its config check, sourced by run.sh and red.sh so
# that the run and its red input are one definition: a change to either
# function changes what red.sh proves as well as what run.sh runs.

# door_lint CRATE CONFDIR TARGETDIR: the door over the library of the crate
# at CRATE, under the checked profile (the code under
# `cfg(debug_assertions)` is linted too), with the config in CONFDIR. The
# output, stdout and stderr, is the caller's; the status is clippy's. The
# output is read by pattern (door_config_diag, red.sh's `missing`), so it is
# in no colour whatever the environment asks: CI sets CARGO_TERM_COLOR to
# always, a diagnostic's first line then starts with an escape and not with
# `error`, and red.sh read every line it had placed as missing.
door_lint() {
  (cd "$1" && CLIPPY_CONF_DIR="$2" cargo clippy --color never --profile checked --lib \
    --target-dir "$3" -- -D warnings 2>&1)
}

# door_config_diag: whether clippy's output on stdin holds a diagnostic
# located in the config file. Clippy only warns of an entry that resolves to
# nothing or to a thing of the wrong kind, `-D warnings` does not make that
# an error, and such an entry bars nothing; whatever its wording, the
# diagnostic points into clippy.toml.
door_config_diag() {
  grep -q -e '--> .*clippy\.toml:[0-9]'
}

DOOR_NAMES_NOTHING="door: an entry of the config names nothing, or the wrong kind of thing"

# door_run CRATE CONFDIR TARGETDIR: door_lint, its output printed, and a
# failure (status 2, with DOOR_NAMES_NOTHING) where the config holds an entry
# that bars nothing, else clippy's status. red.sh runs it over a config with
# one such entry and requires the line, so the check is held where it is.
door_run() {
  out=$(door_lint "$1" "$2" "$3")
  status=$?
  printf '%s\n' "$out"
  if printf '%s\n' "$out" | door_config_diag; then
    echo "$DOOR_NAMES_NOTHING"
    return 2
  fi
  return "$status"
}
