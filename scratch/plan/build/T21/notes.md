# T21 seal-names: notes

Start: HEAD 2a4c8b32, base 6092187041964435429a40e314e841eaa37ca568; `git diff --stat base HEAD` names only scratch/plan/build/T21/{brief.txt,launch-args.json}. Tree clean.

## What the tree has (against the brief)
- Seal words today: `Bytes::lend_at` (bytes.rs trait default + MmapBytes impl), `MmapBytes::lent` (bytes.rs), token modules `mod lend` in src/blob.rs, src/ordindex.rs, src/db/zfile.rs, re-exports `OrdLendToken` (ordindex.rs), `StrideLendToken` (db.rs), the seam's import alias `LendToken` (bytes.rs), the readers' trait `bytes::whole::Reader`, `Paging::base` and `bytes::Base`.
- Implementors of `Paging::base`: src/db/pager.rs (Paged), src/ordindex.rs test `InMemory`.
- Callers: Sec::get, Sec::lend (blob); OrdIndex::over, OrdIndex::mapped_heads (ordindex); ZFile::attach_stride, ZFile::stride_bytes, check_stride, file_facts (zfile); MmapBytes::paged (bytes).
- `lend` is also MmapBytes's field (`self.lend`, the lendable length) and Sec's method `Sec::lend`: both ordinary, stay.
- `whole` (the readers' trait's module) is a common word too (locals `let whole = ...`): renamed as well, beyond the brief's list (the brief's rule: the seal's words unique).
- tests/source.rs seal section: lines 6164..7133 before (970 lines incl. the two red tests).

## Names chosen
- MmapBytes::lent -> MmapBytes::lend_whole_map
- mod lend (x3) -> mod lend_seal
- bytes::whole (trait Reader) -> bytes::whole_map_readers
- Paging::base -> Paging::base_address
- bytes::Base -> bytes::BaseAddressToken
- kept: Bytes::lend_at, LendToken, OrdLendToken, StrideLendToken
Grep before (git grep -nw over src tests bench/src bench/examples examples experiments, comments included):
lend_whole_map 0, lend_seal 0, whole_map_readers 0, base_address 0, BaseAddressToken 0; LendToken 8, OrdLendToken 11, StrideLendToken 12, lend_at 36 (all the seal's or comments/strings about it).

## Steps
1. red-first test added (P1/P2 green cases) -> before.log
2. rename
3. check rebuilt, red cases rewritten
4. scripts, doctests, docs, lists
5. gates; 6 cost; 7 mutants

## Step now
(see bottom)

## Progress (12:30 UTC)
- before.log: the_seal_leaves_ordinary_code_alone red on today's check (EXIT 101), all six cases refused (signature split, declaration grown, call rewrapped, engine's own words in debug_assert, another type's base, a test's local).
- rename done (src, lib.rs doctests, scripts/seal-red*.sh, couplings.txt reason, CLAUDE.md paragraph); check rebuilt; the four seal tests green.
- finished copies: keep/final/ (sha256 in keep/final.sha256); originals keep/orig.*.
- after.log: whole test group --no-fail-fast (running).

## Finished (13:00 UTC)
Finished copies: keep/final/ (keep/final.sha256); the tree equals them (tests/source.rs 2a9b3a4d...).

### What changed
- Names (each unique as an identifier; grep counts after: lend_whole_map 37, lend_seal 86, whole_map_readers 16, base_address 16, BaseAddressToken 30 across src tests bench/src bench/examples examples experiments, comments included -- outside src only tests/source.rs, in string literals, and couplings.txt's reason text): MmapBytes::lent -> lend_whole_map; mod lend (blob, ordindex, zfile) -> lend_seal; bytes::whole -> whole_map_readers (beyond the brief's list: `whole` is a common local); Paging::base -> base_address; bytes::Base -> BaseAddressToken. Kept: Bytes::lend_at, LendToken, OrdLendToken, StrideLendToken (unique already). Doctests of src/lib.rs follow (their local fns renamed `section`, a caller's function and not the seal's word).
- tests/source.rs seal section rebuilt: SEAL_WORDS (token/trait names), SEAL_CALLS / SEAL_DECLS / SEAL_IMPLS as `|`-separated row tables keyed by file and scope (decls), call by file and scopes, impls by header whitespace aside; seal_names, seal_in_macro, SEAL_MACRO_WORDS and the exact-text SEAL_DECLS gone. Rules: every word of src is a call at a listed (file, scope), non-test code, followed by `(`, with exactly one seal word inside its parens equal (whitespace aside, trailing comma aside) to the call's token; or a token/trait word inside such parens; or a word at a decl site making no constructor (`(` after the word unless after `struct`, or `::new` after optional `::Token`/`>`); a definition (`fn` before a call word) only at a decl site; a `use` naming a seal word has no `{`/`*` and binds LendToken/OrdLendToken/StrideLendToken; an `impl` header naming one is listed. Module shapes (three token modules, whole_map_readers) by squashed text; BaseAddressToken struct and its attributes.
- Failure message (SEAL_HELP) appended once: what the author is to do; test holds its words.
- New green test the_seal_leaves_ordinary_code_alone (P1, P2), red before (before.log, EXIT 101, all six cases refused).
- Red tests rewritten; every old red case kept under the new names (none dropped: the macro cases now refused by the word's scope, not a macro rule); added: token module public field, a second fn in the module, zeroed and transmuted tokens at a listed call, a token bound to a local at a listed scope, a function value at a listed scope, the readers' trait given an item, BaseAddressToken with a public field, a renamed import beside the re-export at ordindex <item>, groups in a use (both orders), a second token inside the call's parentheses, a listed scope's name taken by test code, a const hand-out at db.rs <item>, an associated const in trait Paging, the help's words.
- P5 (line counts, from the seal header to the file's end; measured on HEAD's file and the tree's): the check and its helpers (header to the first test, outside_sources and seal_outside_check included) 552 -> 426 (-126); the two red tests 418 -> 525 (new cases, and rustfmt splitting long literals); the new green test 85; whole section 970 -> 1036. The critic's ~200 fewer is not reached in the check (-126): what the new check adds back is the call-span parse, the use and impl rules, and the decl table, which replace the exact text the old one compared.
- scripts/seal-red.sh and seal-red-red.sh follow the names (functions seal_red_lent_* -> seal_red_whole_*); green with each red input red (lint.log).
- CLAUDE.md: the seal paragraph made true of the code, with the rule the critic's finding taught. couplings.txt: the reason text of src/bytes.rs <item> names MmapBytes::lend_whole_map (key and count unchanged). raises.txt not touched: its key's text did not change (its reason text still names the old words `blob::lend`, `MmapBytes::lent`; left as the declaration's record).

### Gates (logs in this directory)
- before.log: red (EXIT 101). after.log: whole test group --no-fail-fast EXIT 0, 1570 passed 0 failed, 27 binaries.
- stride.log: `sh scripts/check.sh stride` EXIT 0 (1574 passed). lint.log: EXIT 0 (clippy -D warnings, fmt, door, seal red: 9 home, 20 abroad; seal red red ok). wasm.log: EXIT 0, wasm_bytes 130094 = base's 130094 (built in a worktree at 6092187, removed). lists.log: SUPDB_BASE=6092187 EXIT 0, all four lists held.
- cost.log: `sh scripts/cost.sh 6092187 read scan read_z scan_z land seal` "cost: green", EXIT 0; no engine function moved in any probe (only libc and the allocator, and join_seal/Timespec within their bounds in land); no renamed function appears as gone or new (lend_at, lend_whole_map are inlined; Paged::base_address is reached once a paged mapping).
- mutants.md: 13 mutants, all killed by named cases.

### What it cannot see
- A name a macro builds from pieces; code from outside a listed file.
- `Paging::<item>` is a decl site for base_address and BaseAddressToken (the lexer reads a one-line bodiless signature as the trait's items), so a further bodiless signature in Paging that names the token passes (mutants.md, extra).
- Scopes are names: a non-test fn elsewhere in a home file with a listed scope's name (two `check_stride`s) would read as listed; test code is excluded.
- The module shapes and the impl headers are compared as text with whitespace collapsed (an item's text, not a line's): rustfmt cannot change them, a comment inside the module is stripped by the lexer.

### Departures
- `bytes::whole` renamed too (brief named Paging::base and bytes::Base and the lend/lent words; `whole` is a common word and is the readers' trait's module).
- P5 not met as the critic expected (see above).
- No rule for macros (as the brief says); the macro red cases are held by scope.

### Not verified
- That every route the old check refused is refused by the new one beyond the cases written (the extra mutant names the two known relaxations, both answered by the compiler).

## Repair round 1 (started 12:50 UTC, 22 Nov)
Reviewers: <item> rows admit any top-level line (macro invocations, a zeroed one-line hand-out); a type alias hand-out; extra mutants X1-X5 and five on the check's lines.
Step: red cases first (r1-before.log), then the check: at a declaration site a word must stand in a `use` head, a listed impl head, a call word's fn signature, or a module/struct held whole; each listed call scope is one definition in its file.
Start copy of tests/source.rs: keep/r1-start/ (sha in keep/r1-start.sha256).

### Round 1 finished (13:10 UTC)
- Changed: tests/source.rs (check step 3: item spans at declaration sites, one definition a listed scope, constructor-at-declaration rule removed; 11 red cases added; SEAL_DECLS doc and the header comment), CLAUDE.md (the seal paragraph: the item rule and the rule the round taught). No src change this round (engine_changed false; the count, wasm and stride runs of round 0 stand, since nothing they build changed).
- Finished copies: keep/final/ (keep/final.sha256; tree = keep checked). Round start: keep/r1-start/.
- Logs: r1-before.log (EXIT 101, six cases passed today's check), r1-after.log (seal tests EXIT 0), r1-source.log (whole source binary, 79 passed, EXIT 0), r1-lint.log (sh scripts/check.sh lint EXIT 0, seal red ok, seal red red ok), r1-lists.log (SUPDB_BASE=6092187 EXIT 0), mut-results-r1.txt (20 mutants run, all killed; two of the review's N/A, rule removed).
- Reviewers' alias finding: not a hole (top-level `type` lines get scope `type Tk`); held by two red cases anyway.
- P5 now: the check and its helpers (header to the first test) 552 -> 446 (round 0: 426; +20 for the item spans and the one-definition count); the seal section 970 -> 1167.

## Repair round 2 (started 13:10 UTC, 22 Nov)
Start copy: keep/r2-start/ (r2-start.sha256). Intermediate (red cases + home-row token): keep/r2-mid/. Finished tests/source.rs: keep/r2-final/ (r2-final.sha256 ff2d5f35...). CLAUDE.md edits after.
Step: mutants running, runner mut3.py set "final", results mut-results-r2.txt, logs mut-r2-<id>.log; source.rs restored from keep/r2-final by sha after each.
r2-before.log: the home-row case red on the round's start check (EXIT 101; first row's token used). r2-after.log: seal tests EXIT 0.
X7, X8 run on keep/r2-mid: both SURVIVED -> `>{next}` arm removed, `made` read as the first seal word to the close.

### Round 2 finished (13:55 UTC)
- Changed: tests/source.rs (seal_check_with; the token from the call's home row; `made` the first seal word to the close; the `>{next}` arm out; seal_until counts parentheses only; the closure limit in the header; red cases: nested fn in a listed scope, a word after a bodiless `-> impl Sized;`, a word on a line with a `use` after it, two attributes on BaseAddressToken, a call at a scope listed for another call, a second token in an earlier argument, a word's rows of other tokens before and after; green cases: a signature grown, a trait signature returning `impl Sized` before a listed impl), CLAUDE.md (the seal paragraph: the rule this round taught and the closure limit). No src change (engine_changed false; the count, wasm and stride runs of round 0 stand).
- Finished copies: keep/r2-final/ (r2-final.sha256: source.rs 18239a70..., CLAUDE.md bce28ff7...); the tree equals them.
- Logs: r2-before.log (EXIT 101, the home-row case), r2-after.log (seal tests EXIT 0), r2-source.log (79 passed, EXIT 0), r2-lint.log (EXIT 0; seal red ok, seal red red ok), r2-lists.log (SUPDB_BASE=6092187 EXIT 0), mut-results-r2.txt.
- P5: the check and its helpers (header to the first test) 552 at HEAD -> 457 (round 1: 445 by this count); the seal section 970 -> 1277.
- Nothing left running; private TMPDIR removed.

## Commit step (14:30 UTC, 22 Nov): not committed
- gate1.log (build test stride lint): EXIT 101. build, test green (source 79 passed); stride group red at its last run, `ZFUZZ_ZPAGES=1 ZFUZZ_STRIDE=1 ZFUZZ_CONVERT=1 ZFUZZ_FROM=0 ZFUZZ_TO=4 concurrent_range`: seed 0 "three settled asks of the first partition moved it in and out (1, 3) times" (tests/fuzz.rs:3025, wants in+out == 3 alternating). Not in the brief's known-flaky list, so not committed.
- fuzz-rerun.log: the same command alone five times, all green. gate2.log lists (SUPDB_BASE=6092187) EXIT 0; gate3.log wasm EXIT 0; gate4.log lint run alone EXIT 0 (seal red ok, seal red red ok).
- Tree unchanged, equals keep/r2-final; private TMPDIR removed; nothing running.

## Repair round 10 (14:05-14:20 UTC, 22 Nov): the stride group's red read as not this change's
- No file of the tree changed this round (tests/source.rs and CLAUDE.md still equal keep/r2-final by sha).
- r10/rename-proof.log: each changed src file, with the five renames undone and comments and whitespace stripped, is HEAD's code byte for byte; tests/fuzz.rs is not in the diff. So the fuzz binary runs the engine HEAD runs, under other names.
- The mechanism, read off the code: the concurrent range asks conversions through its loop (tests/fuzz.rs, `zpages_convert_ask(step / 53)`), ends with `db.settle()`, whose `MaintAsk::Settle` joins seals, merges, finishes, rewarms and asks but not a form build in flight (`zbuild`; only `ZSettle` waits for `z_pending`), and then reads `before`. A conversion started from the loop's last ask that lands inside the first of the three settled asks is a fourth move: (1, 3) is partition 0 out, in, out plus the loop's move out. A test race, present at HEAD, filed already as task 24 ("Fix the concurrent fuzz's convert-steer flake"); not fixed here (out of the brief, and a fix of a race wants the race made wide first).
- r10/loop-tree.log: the gate's command, 48 runs 8 at a time under load: 0 failed (with fuzz-rerun.log, 0 of 53 since the one red). Not run at HEAD's build: the code is the same, and a red that comes once in 50+ runs would need hundreds there to say anything.
- Nothing running; private TMPDIR removed.
