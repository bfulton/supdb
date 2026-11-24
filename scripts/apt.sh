#!/bin/sh
# Install Debian packages on a host that has apt, without their recommended
# extras:
#
#   sh scripts/apt.sh PKG...
#
# The install is tried first with the package lists the host has, and the
# lists are fetched afresh only where that fails. A hosted runner's lists are
# as old as its image, a week or two, and still name what a job here asks
# for; fetching them again is thirteen megabytes before the first byte of a
# package, twenty times the three packages the `faster` arm's set-up wants.
# On a day the mirror was slow that step ran three to four minutes in x64
# jobs and held one for half an hour. A host with no lists at all, a fresh
# container, fails the first try at once and takes the second.
#
# One line on stderr says what was installed, by which of the two tries, in
# how many seconds and with apt's own line for the download in it (the
# bytes, the seconds and the rate, which is what a slow mirror shows in);
# and where an install fails, what apt said: the
# scripts that called apt-get themselves sent its output nowhere, and a
# step that sat for minutes or failed had nothing in its log. Nothing goes
# to stdout, which a caller may be reading for something of its own
# (bench/scripts/libclang.sh prints an `export` line there).
set -eu
if [ $# -eq 0 ]; then
  echo "usage: sh scripts/apt.sh PKG..." >&2
  exit 2
fi
if ! command -v apt-get >/dev/null; then
  echo "no apt-get here; install by hand: $*" >&2
  exit 1
fi
# A container runs as root and may have no sudo, and needs none.
if [ "$(id -u)" = 0 ]; then
  root=
elif command -v sudo >/dev/null; then
  root=sudo
else
  echo "not root and no sudo here; install by hand: $*" >&2
  exit 1
fi
log=$(mktemp)
trap 'rm -f "$log"' EXIT
t0=$(date +%s)
# apt's line for a download, `Fetched 664 kB in 1s (512 kB/s)`, from line $1
# of the log on: the last of them, or nothing where nothing was fetched.
fetched() {
  f=$(tail -n +"$1" "$log" | sed -n 's/^Fetched //p' | tail -n 1)
  [ -z "$f" ] || printf ' (%s)' "$f"
}
if $root apt-get install -y -q --no-install-recommends "$@" > "$log" 2>&1; then
  echo "apt: $* installed in $(($(date +%s) - t0)) s$(fetched 1), with the lists the host had" >&2
  exit 0
fi
echo "--- the lists fetched afresh, after:" >> "$log"
if $root apt-get update -q >> "$log" 2>&1; then
  from=$(($(wc -l < "$log") + 1))
  if $root apt-get install -y -q --no-install-recommends "$@" >> "$log" 2>&1; then
    echo "apt: $* installed in $(($(date +%s) - t0)) s$(fetched "$from"), with the lists fetched afresh" >&2
    exit 0
  fi
fi
echo "apt could not install: $* ($(($(date +%s) - t0)) s)" >&2
tail -n 40 "$log" >&2
exit 1
