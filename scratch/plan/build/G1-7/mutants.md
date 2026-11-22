# G1-7 mutants

Runner: `mut/mutants.py` (each mutant applied alone to the working tree, its
killer run, the verdict appended to `mut/results.txt`, the file put back from
`keep/final/` and checked by its sha256; `mut/<id>.log` is each killer's
output). All eleven applied, all killed, every file put back with its sum,
the tree compared with `keep/final` after (no difference). Each kill read in
its log for its reason: the seal's by an error missing at the lines abroad
(not a build that failed elsewhere), the tests' by named test failures in a
binary that compiled.

The plan's three:

| mutant | killed by |
|---|---|
| M1 `blob::lend::Token::new` widened from `pub(in crate::blob)` to `pub(crate)` | `sh scripts/seal-red.sh`: "no error at" the three lines abroad that call `lend_at` (zfile.rs, ordindex.rs, bytes.rs) |
| (plan: "`lent` without its token") -- `lent` has no token in this tree (notes, departure 2); its analogue, M2 `bytes::Base`'s field widened to `pub(crate)` | `sh scripts/seal-red.sh`: no error at the two lines abroad (pager.rs, vault.rs) |
| M3 `sample`: a wildcard arm (`_ => Err`) ahead of the arm by the piece | tests/zpages.rs: `the_controller_buys_builds_and_installs_the_form_under_a_cipher`, `form_files_an_earlier_open_left_under_a_cipher_are_dormant_and_then_attached` |
| M3b the kept build: a wildcard arm ahead of `Records::Pieces` | tests/zpages.rs: four cipher tests (the warm, the builds ahead, the controller, the dormant files) |

Mine, on the lines I wrote:

| mutant | killed by |
|---|---|
| M4 the warm's touch dropped (`sum = sum.wrapping_add(map.touch(..))` deleted) | `db::zfile::tests::a_warm_reads_the_cold_file_once_and_skips_a_resident_one` |
| M5 `MmapBytes::touch` bounded by the file's length and not the lendable length (a paged mapping touched) | `db::vault::tests::a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope`, by the assertion added for it (a paged mapping is not touched with its pages present, under a hold, after they were read); the test's first assertion alone, made before any page is present, was not run against this mutant, and was expected to read it green since unpresent pages of the memory file read as zeroes (not verified) |
| M6 `Blob::records`: the arm by the piece dropped (`None`) | tests/zpages.rs: four cipher tests |
| M7 `Seg::records_by_descriptor`: a place answered for a reader by the piece | tests/zpages.rs `form_files_an_earlier_open_left_under_a_cipher_are_dormant_and_then_attached` |
| M8 `Paging` made `pub` | the doctest naming `supdb::bytes::Paging` (src/lib.rs) compiles: FAILED |
| M9 `MmapBytes::lent` made `pub` | the doctest calling `MmapBytes::open(p)?.lent()` compiles: FAILED |
| M10 `MmapBytes::lend_at` bounded by the file's length (a paged mapping lent whole unasked) | tests/cipher.rs `a_store_larger_than_its_cache_reads_as_itself_within_it` |

Survivors: none.

## Repair round 1

Runner: `r1/mutants.py` (each mutant's edits applied together, its killer
run, the verdict appended to `r1/results.txt` at once, every file put back
from `keep/r1/` and checked by its sha256; `r1/mut/<id>.log` each killer's
output, read for its reason). Twenty-eight, all killed, every file put back
with its sum, the tree compared with `keep/r1` after (no difference).
Round 0's M2 row ("`lent` without its token", swapped for `Base`'s field)
is superseded by R1: `lent` has its tokens now.

| id | mutant | killed by (the reason, from its log) |
|---|---|---|
| R1 | the plan's: `lent` without its token, the signature and all six callers | seal-red.sh: the home lines for `lent` no longer compile (and the abroad line `m.lent()` would compile) |
| R2 | ordindex's `Token::new` widened to `pub(crate)` | seal-red.sh: no error at src/db.rs and src/blob.rs (the ordered index's token abroad) |
| R2s | the same | tests/source.rs: "the token's module is `..pub(crate) fn new()..`" |
| R3 | zfile's `Token::new` widened to `pub(crate)` | seal-red.sh: no error at src/db.rs and src/ordindex.rs |
| R4 | the readers' trait's module made `pub(crate)` | seal-red.sh: no error at src/db.rs (an impl of `whole::Reader` abroad) |
| X1 | zfile: `m.lend_at(0, 1, unsafe { std::mem::zeroed() })` (the reviewers' first extra: expected to survive) | tests/source.rs: "calls `lend_at(` outside src/blob.rs" |
| X1b | the same with `core::mem::transmute(())` | the same |
| X2 | blob: `pub(crate) fn token() -> lend::Token`, zfile calls `lend_at` with it (the reviewers' second extra: expected to survive) | tests/source.rs: "makes a seal's token that no call of its own takes" (blob) and "calls `lend_at(` outside src/blob.rs" (zfile) |
| X2b | blob: `impl Default for lend::Token`, zfile `lend_at(.., Default::default())` | tests/source.rs: "gives a seal's token an impl", and the two above |
| X2c | blob: `impl Default for lend::Token` alone | seal-red.sh: no error at the `Default::default()` line abroad in zfile |
| X3 | zfile: a new reader `let all = m.lent(lend::Token::new()); if all.is_empty() { return None; }` (the reviewers' third extra) | tests/source.rs: "a caller of `.lent(` that is not listed (SEAL_CALLS)" |
| X4 | M5 (touch bounded by the file's length) with the vault test's second assertion taken out, so its first alone | `db::vault::tests::a_kept_file_..._as_an_envelope` at vault.rs:2954, the first assertion (left 373, right 0): the memory file's pages hold the body before any ask, so the touch reads them. Round 0's "not verified" is verified |
| X5 | `#[derive(Clone, Copy, Default)]` on blob's Token | seal-red.sh: no error at the `Default::default()` line abroad |
| X5d | the same | the doctests: src/lib.rs's `b.lend_at(0, 1, Default::default())` compile_fail compiled |
| X5s | the same | tests/source.rs: "the token's module is `..#[derive(Clone, Copy, Default)]..`" |
| X6 | `#[derive(Default)]` on `bytes::Base` | seal-red.sh: no error at the `p.base(Default::default())` line in pager.rs |
| X6b | `impl Base { pub(crate) fn new() -> Base }` | tests/source.rs: "gives a seal's token an impl" and "makes a seal's token that no call of its own takes" |
| X7 | `Seg::records_by_descriptor`: the `Records::Whole` arm answering `None` (a plain store) | tests/zpages.rs `a_build_in_flight_at_the_crossing_has_the_file_it_dropped_asked_back` (the dropped file not asked back) |
| X8a | `MmapBytes::touch` stepping from `off + page` | bytes.rs `a_touch_reads_each_page_of_its_range_once_and_nothing_past_the_file` (21, wanted 22); the warm test alone was not read for it |
| X8b | touch clamped by `self.lend.saturating_sub(page)` | the same test (15, wanted 22) |
| X9a | seal-red.sh: the home phase's status check taken out | seal-red-red.sh: "home: seal-red.sh answered 0, wanted 1" |
| X9b | seal-red.sh: `\|\| [ "$abroad" != "$listed" ]` taken out | seal-red-red.sh: "placed: seal-red.sh answered 0, wanted 1" |
| O1 | bytes: `impl whole::Reader for LendToken {}` (blob's token admitted as a reader of a mapping whole) | tests/source.rs: "gives a seal's token an impl" |
| O2 | bytes: `impl whole::Reader for () {}` | seal-red.sh: no error at the `m.lent(())` line abroad |
| O2s | bytes: `impl<T> whole::Reader for (T,) {}` | tests/source.rs: "gives a seal's token an impl" (the first draft of the test read only impls naming a token, which a blanket impl does not; seen before the run, and `whole::Reader` is read too now, with a red case) |
| O3 | zfile's token with a public field | seal-red.sh: no error at the `crate::db::StrideLendToken(())` line |
| O4 | tests/source.rs: the check of the calls' scopes taken out | its red input `a_seals_token_made_another_way_fails` ("a caller of `.lent(` that is not listed") |
| O5 | tests/source.rs: the check of a token made apart from its call taken out | its red input ("makes a seal's token that no call of its own takes") |

Survivors: none. O2s was found as a hole before it was run (a blanket impl
named no token), and the check and its red case went in first.

## Repair round 2

Runner: `r2/mutants.py` (each mutant's edits applied together, every
command of its row run in order and each status kept, the verdict the last
command's status, the verdict appended to `r2/results.txt` at once, every
file put back from the copy taken before the edit and checked by its
sha256; the tree compared with `keep/r2/` after: no difference, and
tests/blob.rs clean against HEAD). Logs `r2/mut/<id>.log`, read for each
reason. For E1-E4 the first command is clippy `-D warnings` over the
mutant (status 0 in each: every one is a route that compiles clean with
no lint), the second the source test.

| id | mutant | killed by (the reason, from its log) |
|---|---|---|
| E1 | ordindex `pub(crate) fn hand() -> lend::Token { <lend::Token>::new() }`; db.rs `MmapBytes::lent(m, crate::ordindex::hand())` (reviewers: expected to survive) | clippy 0; tests/source.rs: "src/db.rs (sneak): ... calls `lent` outside src/ordindex.rs, src/db/zfile.rs", "src/ordindex.rs (hand): `pub(crate) fn hand() -> lend::Token {` names a seal's token outside the call it is made for", and the same for `<lend::Token>::new()` |
| E2 | zfile: an unlisted `MmapBytes::lent(m, <lend::Token>::new())` (expected to survive) | clippy 0; "src/db/zfile.rs (another): ... a caller of `lent` that is not listed (SEAL_CALLS)" |
| E3 | blob: `use lend::Token as Tk;` and `let f = <B as Bytes>::lend_at; f(src, 0, 1, Tk::new())` (expected to survive) | clippy 0; "src/blob.rs (<item>): `use lend::Token as Tk;` names a seal's token outside the call", "src/blob.rs (lend_any): `let f = <B as Bytes>::lend_at;`: a caller of `lend_at` that is not listed" |
| E4 | tests/blob.rs: `b.lend_at(0, 1, unsafe { std::mem::zeroed() })` (expected: nothing reads it) | clippy over the blob test 0 (it compiles, and reaches the method); tests/source.rs, which reads tests/, examples/ and bench/ now: "tests/blob.rs (conjured): ... calls `lend_at` outside the crate" |
| E5 | the vault fixture zeroed at every page start, and `MmapBytes::paged` with `lend: pager.size()` | `db::vault::tests::a_kept_file_..._as_an_envelope`: "a paged mapping lends nothing whole" (14100, wanted 0): the assertion added this round reads the lendable length, whatever the bytes are |
| E5b | `paged` with `lend: pager.size()`, the fixture as it is | the same assertion |
| E5c | the fixture zeroed at every page start alone | the same test: "the body has a byte that is not zero at every page's start" (the premise the touch's zero sum rests on, held by the test now) |
| T1 | `MmapBytes::touch`: `at += page / 2` (reviewers: survives the zero mid-pages) | bytes.rs `a_touch_reads_each_page_of_its_range_once_and_nothing_past_the_file`: 522, wanted 22 (the fixture has bytes of 100 at 1, page/4, page/2 and page-1 of every whole page now) |
| W1 | `warm_file`: `map.touch(off, span.min(page))` | `db::zfile::tests::a_warm_reads_the_cold_file_once_and_skips_a_resident_one`, the partial-read half (zfile.rs:14005, "resident after the warm", 42 of 57): the first, fully evicted half passes under the mutant (the ask and the fault's readahead bring the file), the half after a read under `POSIX_FADV_RANDOM` does not. Run three times, killed three times at the same count |
| D1 | `Seg::records_by_descriptor`: the `Records::Pieces` arm answers `record_region_place()` | tests/zpages.rs `form_files_an_earlier_open_left_under_a_cipher_are_dormant_and_then_attached` (zpages.rs:8683), the test that killed round 0's M7 |
| B1 | `Blob::records`: `pieces()` asked before `flat_lent()` | SURVIVED, equivalent: `Sec::pieces` answers only where `src.piecewise()`, and `lend_at` refuses every source that says so -- the default by its first line, `MmapBytes`'s because a paged mapping's lendable length is zero (a key section is never empty at offset zero, the superblock being before it), and no source outside the crate can override it. So at most one of the two is `Some` for any source and the order is unobservable; the lib's zfile tests and the zpages cipher, drop and warm tests read green under it. Recorded so; no check is added |
| S1 | seal-red-red.sh: `case_`'s words check deleted (reviewers: no red input) | seal-red-red.sh itself: "words: a case that failed without its words read as red" -- the script now runs one case (the home typo) named with words seal-red.sh never prints, in a subshell, and requires `case_` to fail |

Mine, on the lines this round wrote (tests/source.rs):

| id | mutant | killed by |
|---|---|---|
| O1 | `seal_names`: a `use` item's `lend` not counted (`in_use` dropped) | `a_seals_token_spelled_another_way_fails`: "passed the check: src/blob.rs (<item>): `use self::lend as mk;`" |
| O2 | a listed call's token names not counted against its calls (`toks != calls.len()` dropped) | the same test: the listed `lend_at` line that makes a second token (`.filter(\|_\| keep(<lend::Token>::new()))`) passed |
| O3 | `outside_sources` not reading bench/ | `the_seals_tokens_are_made_only_where_they_are_spent`: "the tests and the suite are read" |
| O4 | `lent` reached by a path (`::lent`) not counted | the spelled test: E1's and E2's `MmapBytes::lent(..)` passed |
| O5 | `OrdLendToken` not counted | first run SURVIVED (the one red case naming it was red by its unlisted call anyway); a red case added, `pub(crate) fn hand() -> OrdLendToken { OrdLendToken::new() }` in ordindex, and the rerun killed: "passed the check: src/ordindex.rs (hand): `OrdLendToken::new()` ..." |
| O6 | `StrideLendToken` not counted | the red case added beside O5's (`crate::db::StrideLendToken::new()` handed out in zfile) |

Survivors: B1, equivalent, said above. O5's first survival was a missing
red case, added; the keep copy of tests/source.rs was taken again after it
(`keep/r2.sha256`) and O5, O6 and W1 run against it.

## Repair round 3

Runner `r3/mutants.py` (as round 2's: each mutant's edits applied
together, every command run in order and each status kept, the verdict
the last command's status, appended to `r3/results.txt` at once, every
file put back from the copy taken before the edit and checked by sha256;
the tree checked against `keep/r3.sha256` after: no difference). Logs
`r3/mut/<id>.log`. For the X mutants the first command is clippy `--lib
-D warnings` (status 0 in each: every one compiles clean), the second
`cargo test --profile checked --test source -- seal`.

| id | mutant | killed by (from its log) |
|---|---|---|
| X-macro-hand | ordindex `ty!`/`mk!` and `pub(crate) fn hand() -> ty!(lend) { mk!(lend) }`; db.rs `call!(m, lent, crate::ordindex::hand())` (reviewer: survives today) | clippy 0; the tree test: "src/db.rs (sneak): `call!(m, lent, crate::ordindex::hand()).len()` names `lent` or `lend` inside a macro", and both ordindex lines (`-> ty!(lend) {`, `mk!(lend)`) |
| X-macro-home | zfile `ZFile::sneak`: `call!(self.map, lent, mk!(lend))` (reviewer: survives today) | clippy 0; "src/db/zfile.rs (ZFile::sneak): `call!(self.map, lent, mk!(lend))` names `lent` or `lend` inside a macro" |
| X-macro-blob | control: blob `hand() -> ty!(lend)`; zfile `call!(src, lend_at, 0, 1, crate::blob::hand())` (reviewer: red already) | clippy 0; "calls `lend_at` outside src/blob.rs" (the `lend_at` arm, as before) and the two blob lines inside a macro (new) |
| X-macro-use | mine: ordindex `macro_rules! u { () => { use self::lend as mk; }; } u!();` and `hand() -> mk::Token` | clippy 0; "src/ordindex.rs (u!): `() => { use self::lend as mk; };` names `lent` or `lend` inside a macro" |
| V4 | `outside_sources` without examples/ (reviewer: likely survives) | the tree test: "the tests, the examples and the suite are read" (it asserts `examples/crypt.rs` now) |
| V5 | `base` arm `(path && post != "()")` -> `path` (reviewer: likely equivalent) | the green case: "src/db/pager.rs (others): `let b = Others::base();` calls `base` outside src/bytes.rs" (a `base()` of no argument by type must read green, and does without the mutant) |
| V6 | `opens_use` without its `pub(..) use` disjunct | the red case added: "passed the check: src/blob.rs (<item>): `pub(crate) use self::lend as mk;`" |
| V7 | `dot` without the `..` exclusion (reviewer: probably survives) | the green case: "`(0..lent(n)).count() + b` calls `lent` outside" (a range to a local closure must read green) |
| V8 | step 3 without the `whole::Reader` disjunct (reviewer: likely survives) | killed by a case that was there: `a_seals_token_made_another_way_fails`, "The readers' trait given to every type" (`impl<T> whole::Reader for (T,) {}` in bytes.rs), "gives a seal's token an impl" not raised; no other path reads that line |

Mine, on the lines this round wrote (tests/source.rs):

| id | mutant | killed by |
|---|---|---|
| R1 | `SEAL_MACRO_WORDS` matched by file and text, not scope | the `lent,`-on-its-own-line case: "wanted `src/db.rs (sneak): `lent,` names ...`" (db.rs's listed `lent,` let it through) |
| R2 | `seal_in_macro`: `macro_rules!`'s name not skipped, so a definition is not read | "passed the check: `() => { use self::lend as mk; };` ..." |
| R3 | only `(` opens a macro | the same case (the definition's `{`) |
| R4 | a macro never closes (`open.pop()` dropped) | the tree: every later `lend` of blob.rs read as inside a macro |
| R5 | keywords read as macro names | the green case: "`if !(lent(n) > 1) {` names `lent` or `lend` inside a macro" |
| R6 | inside a macro only `lent` refused, not `lend` | "passed the check: src/ordindex.rs (hand): `mk!(lend)` ...", the use-body case, and the blob control's `mk!(lend)` |

Survivors: none.
