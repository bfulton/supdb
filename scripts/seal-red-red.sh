#!/bin/sh
# The red input of scripts/seal-red.sh, run by the lint group after it:
# the script is run once a case with a line of the case's own added to its
# list (SEAL_RED_EXTRA), and each run has to fail, by its exit status, with
# the words of the check the case is for. A check of seal-red.sh that a
# change took out reads green here and fails this script.
#
#   sh scripts/seal-red-red.sh    # prints `seal red red: ok`
set -u
root="$(cd "$(dirname "$0")/.." && pwd -P)"
fail() {
  echo "seal red red: $*" >&2
  exit 1
}
mkdir -p "$root/target/seal-red" || fail "no target/seal-red"
dir="$(mktemp -d "$root/target/seal-red/red.XXXXXX")" || fail "no temporary directory"
trap 'rm -rf "$dir"' EXIT
trap 'exit 130' INT TERM

# case NAME WORDS LINE: seal-red.sh with LINE added must exit 1 and say WORDS.
case_() {
  printf '%s\n' "$3" > "$dir/$1"
  out=$(SEAL_RED_EXTRA="$dir/$1" sh "$root/scripts/seal-red.sh" 2>&1)
  status=$?
  if [ "$status" != 1 ]; then
    printf '%s\n' "$out" | tail -20
    fail "$1: seal-red.sh answered $status, wanted 1"
  fi
  printf '%s\n' "$out" | grep -q "$2" || {
    printf '%s\n' "$out" | tail -20
    fail "$1: seal-red.sh failed without saying \"$2\""
  }
  echo "seal red red: $1 red ($2)"
}

T="$(printf '\t')"
# A line that does not compile where its call is allowed: the home phase.
case_ home "does not compile in the module allowed" \
  "src/blob.rs${T}src/db/zfile.rs${T}#[allow(dead_code)] fn seal_red_typo() -> u8 { crate::blob::lend::Tokne::new() }"
# A line that compiles abroad: the abroad phase's error at the line.
case_ abroad "no error at src/db/zfile.rs" \
  "src/blob.rs${T}src/db/zfile.rs${T}#[allow(dead_code)] fn seal_red_asks(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::slice_at(m, 0, 1) }"
# A line with no module abroad: the count of the lines placed there.
case_ placed "of the list's" \
  "src/blob.rs${T}-${T}#[allow(dead_code)] fn seal_red_nowhere() {}"
# The words check of case_ itself: a run that fails for its own reason
# and not the one named is not a red case. The home typo again, named with
# words seal-red.sh never prints, must fail case_.
if (case_ words "words seal-red.sh never prints" \
  "src/blob.rs${T}src/db/zfile.rs${T}#[allow(dead_code)] fn seal_red_typo() -> u8 { crate::blob::lend::Tokne::new() }") >/dev/null 2>&1; then
  fail "words: a case that failed without its words read as red"
fi
echo "seal red red: words held"
echo "seal red red: ok"
