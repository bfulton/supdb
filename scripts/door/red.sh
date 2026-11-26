#!/bin/sh
# The door's red input (forms.md G-door), run by `sh scripts/check.sh lint`
# beside the door itself. A lint that is not running reports green, so the
# door is trusted only where it fails on calls past it: this copies the
# crate to a temporary directory (under $TMPDIR, which run.sh points inside
# the repository so that cargo reads the configuration the run read),
# writes the calls of the list below past the door into a function of
# src/db/zfile.rs that carries no allow -- a path call, one through an
# alias, a method on a value, memmap2, libc through an alias, one inside a
# closure, a libc call by path, one compiled only under
# `debug_assertions`, libc's remove (the stdio half of the path calls), a
# program run, and libc's utime (a member of the libc class the source test
# holds) -- and runs the door's own command (scripts/door/lint.sh) over the
# copy twice, under the config the run hands it (CONFDIR, its clippy.toml).
#
# First under that config with one entry that names nothing: the run has to
# fail with a `disallowed` diagnostic on each line, and the run's check for
# such an entry (door_run) has to fail on it, so a clippy that words or
# places that warning differently fails here and not silently in the run. A
# family whose config path no longer resolves, a config the run did not
# read, or a profile that compiles a line out leaves its line without one.
# Then under that config less the entries each line is listed with: the run
# has to pass with no diagnostic on any of the lines, so each line is barred
# by its own entries, and every line is one the check above reads (a check
# that skipped one would find it missing here as well and not count it).
# The copy and its builds are removed at the end, pass or fail.
#
# The function needs no check that it carries no allow: one there, in the
# source or in tests/data/door.txt, silences a line or fails the source
# test, and either fails here or there.
#
#   sh scripts/door/red.sh CONFDIR    # prints `door red: ok`
set -u
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
. "$root/scripts/door/lint.sh"
fail() {
  echo "door red: $*" >&2
  exit 1
}
[ $# = 1 ] || fail "usage: red.sh CONFDIR"
conf="$1"

copy="$(mktemp -d "${TMPDIR:-/tmp}/supdb-door-red.XXXXXX")" || fail "no temporary directory"
trap 'rm -rf "$copy"' EXIT
trap 'exit 130' INT TERM
cp "$root/Cargo.toml" "$root/Cargo.lock" "$copy/" || fail "copy"
cp -R "$root/src" "$copy/src" || fail "copy"
mkdir "$copy/conf" "$copy/less" || fail "config"
cp "$conf/clippy.toml" "$copy/conf/door.toml" || fail "no clippy.toml in $conf"

# The lines, each its own family: the config entries that bar it, a tab,
# the line. Each goes in after the anchor's line, in order.
cat > "$copy/lines" <<'EOF'
std::fs::File::open	    let _ = std::fs::File::open("/");
std::fs::read	    { use std::fs as red; let _ = red::read("/"); }
std::path::Path::exists	    let _ = std::path::Path::new("/").exists();
memmap2::MmapOptions::new memmap2::MmapOptions	    let _ = memmap2::MmapOptions::new();
libc::madvise	    unsafe { use libc as c; c::madvise(std::ptr::null_mut(), 0, c::MADV_NORMAL); }
std::fs::read_to_string	    let _red: fn(&str) -> std::io::Result<String> = |p| std::fs::read_to_string(p);
libc::unlink	    unsafe { libc::unlink(c"/".as_ptr()) };
std::fs::metadata	    #[cfg(debug_assertions)] let _ = std::fs::metadata("/");
libc::remove	    unsafe { libc::remove(c"/".as_ptr()) };
std::process::Command::new std::process::Command	    let _ = std::process::Command::new("true");
libc::utime	    unsafe { libc::utime(c"/".as_ptr(), std::ptr::null()) };
EOF

# The anchor: a function compiled on every target.
file="$copy/src/db/zfile.rs"
anchor='fn drop_written(file: &super::vault::File) -> Result<()> {'
n=$(grep -cF "$anchor" "$file")
[ "$n" = 1 ] || fail "the anchor \`$anchor\` is in src/db/zfile.rs $n times, not once"
at=$(grep -nF "$anchor" "$file" | cut -d: -f1)
before=$(wc -l < "$file")
awk -v at="$at" -v lines="$copy/lines" '
  { print }
  NR == at { while ((getline l < lines) > 0) { sub(/^[^\t]*\t/, "", l); print l } }
' "$file" > "$file.red" && mv "$file.red" "$file" || fail "insert"
# How many went in, counted off the file and not off the list.
added=$(( $(wc -l < "$file") - before ))
listed=$(wc -l < "$copy/lines")

# The lines of src/db/zfile.rs past the anchor that `disallowed` errors
# point at (an error under -D warnings, so a line found is a run that
# exited non-zero), and of those that went in, the ones missing.
missing() {
  hit=$(printf '%s\n' "$1" | awk '
    /^error: use of a disallowed/ { want = 1; next }
    want && /-->/ { sub(/.*src\/db\/zfile.rs:/, "", $0); sub(/:.*/, "", $0); print; want = 0 }
  ')
  k=1
  while [ "$k" -le "$added" ]; do
    printf '%s\n' "$hit" | grep -qx "$((at + k))" || printf ' %s' "$((at + k))"
    k=$((k + 1))
  done
}

# 1. The config with one entry that names nothing.
awk '
  { print }
  /^disallowed-methods = \[/ { print "  \"libc::no_such_fn_door_red\","; n++ }
  END { if (n != 1) exit 1 }
' "$copy/conf/door.toml" > "$copy/conf/clippy.toml" ||
  fail "the config has no single \`disallowed-methods = [\` line"
out=$(door_run "$copy" "$copy/conf" "$copy/target")
status=$?
gone=$(missing "$out")
if [ -n "$gone" ]; then
  printf '%s\n' "$out"
  fail "no disallowed diagnostic on src/db/zfile.rs line(s)$gone"
fi
if ! printf '%s\n' "$out" | grep -qxF "$DOOR_NAMES_NOTHING"; then
  printf '%s\n' "$out"
  fail "door_run did not fail on an entry that names nothing"
fi
[ "$status" = 2 ] || fail "door_run answered $status, not 2, on an entry that names nothing"

# 2. The config less every line's entries: no line barred, the run clean.
cut -f1 "$copy/lines" | tr ' ' '\n' | sort -u > "$copy/entries"
awk -v entries="$copy/entries" '
  BEGIN { while ((getline e < entries) > 0) drop["\"" e "\""] = 1 }
  {
    # An entry is letters, digits, `_` and `::`: no character a regular
    # expression reads otherwise.
    for (e in drop) {
      if (gsub("[{] path = " e ", reason = \"[^\"]*\" [}],? ?", "")) found[e]++
      if (gsub(e ",? ?", "")) found[e]++
    }
    print
  }
  END { for (e in drop) if (!found[e]) { print "unlisted " e > "/dev/stderr"; bad = 1 }; exit bad }
' "$copy/conf/door.toml" > "$copy/less/clippy.toml" ||
  fail "an entry the lines are listed with is not in the config"
cargo clean --profile checked --target-dir "$copy/target" -p supdb -q --manifest-path "$copy/Cargo.toml" ||
  fail "clean"
out=$(door_run "$copy" "$copy/less" "$copy/target")
status=$?
gone=$(missing "$out")
want=$(k=1; while [ "$k" -le "$added" ]; do printf ' %s' "$((at + k))"; k=$((k + 1)); done)
# All of them, by place and by count against the list itself.
if [ "$status" != 0 ] || [ "$gone" != "$want" ] || [ "$(echo $gone | wc -w)" != "$listed" ]; then
  printf '%s\n' "$out"
  fail "without their entries the lines read as missing:$gone, not$want (the list's $listed), and door_run answered $status"
fi
echo "door red: ok (a disallowed diagnostic on each of the $added lines and door_run failing on an entry that names nothing; without the lines' entries none and a clean run)"
