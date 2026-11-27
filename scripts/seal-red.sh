#!/bin/sh
# The seal's red input (forms.md R2, plan G1-7), run by the lint group of
# scripts/check.sh after the door. The read path's lend of a section on the
# bounds check alone, which a source that lends by the piece refuses
# (`Bytes::lend_at`), takes a token only `blob` can make
# (`blob::lend_seal`, whose constructor is `pub(in crate::blob)`); the
# address behind a paged mapping (`Paging::base_address`) takes a token
# only `bytes` can make (`bytes::BaseAddressToken`, whose field is
# private); a mapping as it lies (`MmapBytes::lend_whole_map`) takes a
# token only its readers make, the ordered index (`ordindex::lend_seal`)
# and the stride form (`db::zfile::lend_seal`); a mapping's whole range,
# whose pages its reader asks for before it reads them
# (`MmapBytes::lend_asked_map`), takes the stride form's token and no
# other; no token is had by
# `Default::default()`; and `MmapBytes::as_slice` is gone.
# A token made by any other route, and a new caller inside a reader's own
# module, are held by tests/source.rs
# (`the_seals_tokens_are_made_only_where_they_are_spent`).
# The compiler is the gate; this is what shows it is: each line below is a
# function that makes one such call, appended once to the module allowed to
# make it and once to a module that is not, in a copy of the crate.
#
# At home every line has to compile, so the call is shown to be one the
# compiler takes and not a typo; abroad every line has to fail to compile
# with an error that points at it. A line whose home is `-` has none (the
# method is gone). The copy stands inside the repository, under
# target/seal-red, so cargo reads the configuration the gate reads, and is
# removed at the end, pass or fail, with its builds. Outside the crate the
# same borrows are held by the doctests of src/lib.rs, each beside a twin
# that compiles.
#
#   sh scripts/seal-red.sh    # prints `seal red: ok`
set -u
root="$(cd "$(dirname "$0")/.." && pwd -P)"
fail() {
  echo "seal red: $*" >&2
  exit 1
}
mkdir -p "$root/target/seal-red" || fail "no target/seal-red"
copy="$(mktemp -d "$root/target/seal-red/copy.XXXXXX")" || fail "no temporary directory"
trap 'rm -rf "$copy"' EXIT
trap 'exit 130' INT TERM

# The lines: the home module, a tab, the module abroad, a tab, the line.
cat > "$copy/lines" <<'EOF'
src/blob.rs	src/db/zfile.rs	#[allow(dead_code)] fn seal_red_lend(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, crate::blob::lend_seal::Token::new()) }
src/blob.rs	src/ordindex.rs	#[allow(dead_code)] fn seal_red_lend_ord(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, crate::blob::lend_seal::Token::new()) }
src/blob.rs	src/bytes.rs	#[allow(dead_code)] fn seal_red_lend_seam(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, crate::blob::lend_seal::Token::new()) }
src/bytes.rs	src/db/pager.rs	#[allow(dead_code)] fn seal_red_base(p: &dyn crate::bytes::Paging) -> *const u8 { p.base_address(crate::bytes::BaseAddressToken(())) }
src/bytes.rs	src/db/vault.rs	#[allow(dead_code)] fn seal_red_base_vault(p: &dyn crate::bytes::Paging) -> *const u8 { p.base_address(crate::bytes::BaseAddressToken(())) }
-	src/db/zfile.rs	#[allow(dead_code)] fn seal_red_as_slice(m: &crate::bytes::MmapBytes) -> &[u8] { m.as_slice() }
src/ordindex.rs	src/db.rs	#[allow(dead_code)] fn seal_red_whole_ord(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(crate::ordindex::lend_seal::Token::new()) }
src/ordindex.rs	src/blob.rs	#[allow(dead_code)] fn seal_red_whole_ord_blob(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(crate::ordindex::lend_seal::Token::new()) }
src/db/zfile.rs	src/db.rs	#[allow(dead_code)] fn seal_red_whole_stride(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(crate::db::StrideLendToken::new()) }
src/db/zfile.rs	src/ordindex.rs	#[allow(dead_code)] fn seal_red_whole_stride_ord(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(crate::db::StrideLendToken::new()) }
src/db/zfile.rs	src/db.rs	#[allow(dead_code)] fn seal_red_asked_stride(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_asked_map(crate::db::StrideLendToken::new()) }
src/db/zfile.rs	src/ordindex.rs	#[allow(dead_code)] fn seal_red_asked_stride_ord(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_asked_map(crate::db::StrideLendToken::new()) }
-	src/ordindex.rs	#[allow(dead_code)] fn seal_red_asked_ord(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_asked_map(lend_seal::Token::new()) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_asked_default(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_asked_map(Default::default()) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_asked_literal(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_asked_map(crate::db::StrideLendToken(())) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_whole_bare(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map() }
-	src/db/zfile.rs	#[allow(dead_code)] fn seal_red_lend_default(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, Default::default()) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_whole_ord_default(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map::<crate::ordindex::OrdLendToken>(Default::default()) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_whole_stride_default(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map::<crate::db::StrideLendToken>(Default::default()) }
-	src/db/pager.rs	#[allow(dead_code)] fn seal_red_base_default(p: &dyn crate::bytes::Paging) -> *const u8 { p.base_address(Default::default()) }
-	src/db.rs	#[allow(dead_code)] struct SealRedMine; impl crate::bytes::whole_map_readers::Reader for SealRedMine {}
-	src/db.rs	#[allow(dead_code)] fn seal_red_whole_unit(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(()) }
-	src/db/zfile.rs	#[allow(dead_code)] fn seal_red_lend_literal(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, crate::blob::lend_seal::Token(())) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_whole_ord_literal(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(crate::ordindex::OrdLendToken(())) }
-	src/db.rs	#[allow(dead_code)] fn seal_red_whole_stride_literal(m: &crate::bytes::MmapBytes) -> &[u8] { m.lend_whole_map(crate::db::StrideLendToken(())) }
EOF
# The script's own red input (scripts/seal-red-red.sh) adds lines of its
# own, in the same form, to show that each check below fails.
if [ -n "${SEAL_RED_EXTRA:-}" ]; then
  cat "$SEAL_RED_EXTRA" >> "$copy/lines" || fail "no $SEAL_RED_EXTRA"
fi
# lines FILE: its count of lines, as a number alone: BSD's `wc` pads the
# count with spaces, and on the macOS leg every `file:line` below read
# `file:   19884`, which no error's location matched.
lines() { wc -l < "$1" | tr -d ' '; }
listed=$(lines "$copy/lines")

# check: `cargo check` of the library of the copy, under the profile the
# tests build with (the code under `cfg(debug_assertions)` included), its
# output on stdout and its status returned, in no colour whatever the
# environment asks, since each error is found by the start of its line
# (CI's CARGO_TERM_COLOR=always put an escape there).
check() {
  (cd "$copy" && cargo check --color never --profile checked --lib --target-dir "$copy/target" \
    --message-format short 2>&1)
}

# place COLUMN: the copy's sources fresh from the tree, then each line
# appended to the file in COLUMN (1 home, 2 abroad), and `file:line` of each
# printed, in the list's order; a line with no file there is skipped.
place() {
  rm -rf "$copy/src" && cp -R "$root/src" "$copy/src" || fail "copy"
  : > "$copy/placed"
  while IFS='	' read -r home abroad line; do
    if [ "$1" = 1 ]; then f="$home"; else f="$abroad"; fi
    [ "$f" = - ] && continue
    [ -f "$copy/$f" ] || fail "no $f"
    printf '\n%s\n' "$line" >> "$copy/$f"
    echo "$f:$(lines "$copy/$f")" >> "$copy/placed"
  done < "$copy/lines"
}

cp "$root/Cargo.toml" "$root/Cargo.lock" "$copy/" || fail "copy"

# 1. At home: every line compiles.
place 1
homes=$(lines "$copy/placed")
[ "$homes" -gt 0 ] || fail "no line has a home"
out=$(check)
status=$?
if [ "$status" != 0 ]; then
  printf '%s\n' "$out"
  fail "a line does not compile in the module allowed to make its call (cargo check answered $status)"
fi

# 2. Abroad: every line fails, by an error located at it.
place 2
out=$(check)
status=$?
[ "$status" != 0 ] || fail "every line compiled abroad"
missing=""
while read -r at; do
  printf '%s\n' "$out" | grep -q "^$at:[0-9]*: error" || missing="$missing $at"
done < "$copy/placed"
abroad=$(lines "$copy/placed")
if [ -n "$missing" ] || [ "$abroad" != "$listed" ]; then
  printf '%s\n' "$out"
  fail "no error at$missing ($abroad of the list's $listed placed)"
fi
printf '%s\n' "$out" | grep ': error'
echo "seal red: ok (the $homes lines with a home compile there; all $listed fail abroad, each at its line)"
