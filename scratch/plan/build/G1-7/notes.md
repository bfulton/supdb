# G1-7 `sealed`: notes

Began at HEAD e9a6abff (the base of every count and of the lists' check).
`git status --porcelain` empty at the start.

## Step: where I am (repair round 3)

Round 3 (r3/): the seal's source test refuses `lent` and `lend` in any
position inside a macro's tokens (the one blocking item), the round's
extra mutants. Done: every step. Round 3's copy of every changed file is
`keep/r3/` (sums `keep/r3.sha256`, equal to the working tree), taken
before the mutants; the round began from `keep/r3start/` (= `keep/r2/`,
sums `keep/r3start.sha256`). It differs from `keep/r3start/` in
`tests/source.rs` and `CLAUDE.md` alone: no file under src/ changed.
Runner `r3/mutants.py`, verdicts `r3/results.txt`, logs `r3/mut/`.
Section "Repair round 3" below. Uncommitted.

## Round 2's state (kept)

Round 2 (r2/): the seal's source test reads names as identifiers (the one
blocking item), the round's extra mutants. Round 2's copy of every changed
file is `keep/r2/` (sums `keep/r2.sha256`), taken before the mutants; the
round began from `keep/r2start/` (= `keep/r1final/`). Runner
`r2/mutants.py`, verdicts `r2/results.txt`, logs `r2/mut/`.

## Round 1's state (kept)

Repair round 1 (r1/): `lent` takes a token per reader; the seal's source
test; seal-red-red.sh; the touch's unit test. The round's copy of every
changed file is `keep/r1/` (sums `keep/r1.sha256`); the mutant runner is
`r1/mutants.py`, its verdicts `r1/results.txt`, its logs `r1/mut/`.
`keep/final/` is round 0's finished copy, kept for the record. Done: every
step of the round. The finished copy is `keep/r1final/` (sums
`keep/r1final.sha256`), equal to the working tree; it differs from
`keep/r1/` (the mutants' baseline) in CLAUDE.md alone, edited after the
mutants. Uncommitted. Removed after use: `r1/before-tree` (git archive of
HEAD with `keep/final/` over it, to make again), `r1/cost-tb`, the test
TMPDIR.

## What the tree has that the plan read differently (plan at 682b2c09)

- `OrdIndex::head` no longer reads the mapping: since upstream's heads copy
  (`Options::heads_owned`) it reads `self.heads`, an `AtomicPtr<u8>` set at
  `over` to the mapping's heads or to a copy (`set_heads`), with the bound
  `lim` (zero for a paged mapping, where `head_absent` asks by the piece).
  So the plan's "`head` keeps its one range check through `lent`" is moot:
  `head` names no `MmapBytes` method and is not touched. The whole borrows of
  an ordered index are at `OrdIndex::over` (the CRC, the eager top, the
  heads pointer) and `OrdIndex::mapped_heads` (what a copy is made from and
  compared with). Those take `lent`.
- `MmapBytes::as_slice` callers outside tests (by receiver, each read):
  ordindex.rs `over`, `mapped_heads`; zfile.rs `ZFile::attach_stride`,
  `ZFile::stride_bytes`, `check_stride`, `file_facts` -- the four zfile ones
  are upstream's stride form, which did not exist at 682b2c09. Test uses:
  zfile.rs `a_drop_behind_unmaps_the_window_and_no_page_past_it` (the
  mapping's address for /proc/self/pagemap) and `hold_a_page_mapped` (a
  touch of one page). Every other `.as_slice()` in src and tests is a Vec's
  or a slice's.
- `Bytes::lend_at` callers outside src/blob.rs: one, `warm_file`
  (zfile.rs), G1-6's touch of a mapped file's pages.
- `Blob::record_regions` callers: zfile.rs `build` (the mapped arm),
  `sample`, db.rs `Seg::records_by_descriptor` (a test of the kind alone),
  and nine zfile tests. `record_pieces`: zfile.rs `build` (the kept arm),
  `sample`.
- `Blob::record_region_place` is used by tests/blob.rs (G1-4's
  `holds_to_the_file` traces), so it stays public: it is an offset, not a
  borrow, and answers for both kinds of reader (departure 2).
- Implementors of `Paging`: `pager::Paged` and `ordindex`'s test
  `InMemory`. `.base()` has one caller, `MmapBytes::paged`.
- `Blob::open_with` dispatches `match key.pieces(&src)` twice (G1-5).

## Step 2: the token pattern (plan "Not verified", first item)

Built in a scratch crate (`token/lib.rs`, `lib2.rs`, rustc 1.97.0, under
`#![deny(warnings)]`):
- a `pub struct Token(())` in `pub(crate) mod lend` inside `blob`, with
  `pub(in crate::blob) fn new()`, named in a public trait `Bytes`'s method
  signature defined in `bytes`: compiles, no `private_interfaces` warning;
- an outside crate cannot override the method (E0603, private module) and
  an outside impl that keeps the default compiles;
- an outside crate *can* write a call whose argument diverges
  (`m.lend_at(0, 1, todo!())`): it compiles, with an `unreachable call`
  warning, and the call is never reached, since the argument panics first.
  The same in-crate is the `unreachable_code` warning, an error under
  `-D warnings`. `unsafe { std::mem::zeroed() }` would make one too, in
  unsafe code. So "cannot call" holds for every safe call that reaches the
  callee, which is what the rule needs; said here and in the doctest's prose.
- a token defined in a private module (`db::zfile::lend`) and re-exported
  `pub(crate) use` from `db` is nameable from `bytes`, its constructor
  `pub(in crate::db::zfile)` refused elsewhere (E0624); a method generic
  over a trait in a private module of `bytes` (`by::Whole`), implemented
  there for two such tokens, raises no `private_bounds` warning.
So the pattern holds; the unit is built as the plan says, with the
departures below.

## Step 3: red first (before.log, before-1.log, before-2.log)

- `src/lib.rs`'s crate docs gained compile_fail doctests, each beside a twin
  that compiles (the first doctests the crate has that are not `ignore`).
  Before the change: before-2.log (before-1.log's first record_pieces
  doctest failed for the wrong reason, `Pieces::size` not being public, and
  was rewritten before the change): six compile_fail doctests FAILED (they
  compiled: `as_slice`, `lend_at(0, 1)`, an outside `lend_at` override,
  naming `bytes::Paging`, `record_regions`, `record_pieces`), status 101.
  The two first drafted with the plan's token (`supdb::blob::lend::Token`)
  passed before for the wrong reason (no such path); they went with the
  token (departure 1).
- `scripts/seal-red.sh` (new, in the lint group after the door): before the
  change, red (status 1) at its first phase, the lines not compiling at home
  (no token, no `Base` existed): red before, but for the reason that the
  things it names did not exist -- not evidence of the seal, only that the
  script fails on a tree without it. Its evidence is after: every line
  compiles at home and fails abroad at its own line (E0603 private function
  / private constructor, E0599 no method).

## The change, and the departures from the plan

1. **`Bytes::lend_at` takes the plan's token, and the wall's count of the
   seam's paths out rises by one, declared.** The plan's shape (`fn
   lend_at(&self, off, len, _: crate::blob::lend::Token)`, the token in
   `pub(crate) mod blob::lend` with `pub(in crate::blob) fn new()`) compiled
   clean and the wall (tests/source.rs, G1-1) refused it: `src/bytes.rs` is
   the seam and its paths out into the forms are counted, at 0 at the base;
   the signature names the token twice (the default and `MmapBytes`'s
   override): `src/bytes.rs path out of the encryption modules code: 2 > 0`.
   Shapes tried before the raise:
   - **A shape that raises no count, counted and refused.**
     `Bytes::lend_at` taken off the trait; `Bytes::lendable(&self) -> u64`
     in its place (`len()` by default, 0 for a source that lends by the
     piece, `self.lend` for `MmapBytes`) and the lend a private function of
     `blob` (`end > src.lendable()` refuses, else `src.slice_at(off,
     len)`), confined to blob by plain privacy with no loophole. The second
     compare did not fold: `sh scripts/cost.sh e9a6abff ...` read RED
     (cost1-lendable.log): `Blob::scan_whole` +6 a scan in scan and
     scan_long, the public `Blob::scan_at` probe +127 a scan of a thousand
     entries, `supdb_get` +1 a read in read_z, and in land `Blob::key_at`
     +0.88, `scan_whole` +0.76, `BuildCtx::copy_block` +0.81, `exts_at`
     +0.13 an op. Rule 1 refuses it.
   - A token defined in `bytes` cannot have its constructor confined to
     `blob` (`pub(in path)` takes an ancestor of the item's own module, and
     a private field confines it to `bytes`); a trait bound defined in
     `bytes` that `blob` implements for its token is one any module of the
     crate can implement for a type of its own, so it seals nothing
     in-crate.
   So the token is named once in `bytes`, by `use crate::blob::lend::Token
   as LendToken;` (the two signatures say `LendToken`); with `lent`'s two
   tokens (departure 2, round 1) the wall's line is `src/bytes.rs <item>
   path out of the encryption modules code 3 stays`, and
   `scripts/raises.txt` declares the raise (0 to 3) with the reasons.
   The seal: no code outside `blob` can call `lend_at` (E0624 on
   `Token::new`), and no source outside the crate can override it (E0603,
   the module is private) or call it with a token. The loophole the token
   pattern leaves (step 2): a call whose argument diverges compiles and
   never reaches the method; in-crate it is the `unreachable_code` warning,
   an error under the gate's `-D warnings`.
   tests/source.rs's `a_seam_re_export_of_the_engine_fails` asserted the
   seam's paths out at `1 > 0`, a count it assumed was zero; it reads the
   listed count now and asserts one over it (`2 > 1` today).
2. **`MmapBytes::as_slice` is deleted; `MmapBytes::lent` is its
   crate-private successor, sealed by a token of each reader (repair round
   1).** Round 0 gave `lent` no token, to keep the wall's count of the
   seam's paths out at 1; both reviewers refused it: any module of the crate
   could write `map.lent()`, the whole borrow that answers the empty slice
   for a paged mapping (R2's hazard), and "prefer the shape that raises no
   count" chooses among shapes that hold the rule. Now `pub(crate) fn
   lent<T: whole::Reader>(&self, _: T) -> &[u8]`, `whole` a private module
   of `bytes` whose trait `bytes` alone implements, for two tokens: the
   ordered index's (`ordindex::lend::Token`, constructor
   `pub(in crate::ordindex)`, re-exported `ordindex::OrdLendToken`) -- the
   plan's -- and the stride form's (`db::zfile::lend::Token`, constructor
   `pub(in crate::db::zfile)`, re-exported `db::StrideLendToken` since
   `zfile` is a private module of `db`) -- a departure: the plan predates
   upstream's four stride readers. The step-2 shape, as the reviewers
   named. Wall: `src/bytes.rs <item> path out of the encryption modules
   code` 1 -> 3 (one `use` a token), declared in scripts/raises.txt as 0 ->
   3 against e9a6abff with the reason. A grouped `use crate::{..}` would
   have read 1 by the wall's rule that a group is one path; not taken, since
   the count would then hide two couplings. The ordered index's token is
   named in `bytes` by a re-export of its own name because the wall's
   alias reading took `use ...::Token as OrdLendToken` for vault's `Token`
   (an `impl` of a seam type, a false count). blob's token lost its
   `#[derive(Clone, Copy)]` (no use). The doctest calling `.lent()` from
   outside stands (crate-private). Callers (6): `OrdIndex::over`,
   `OrdIndex::mapped_heads`, `ZFile::attach_stride`, `ZFile::stride_bytes`,
   `check_stride`, `file_facts`; a seventh is a line of tests/source.rs's
   `SEAL_CALLS`, read in review.
3. **`OrdIndex::head` is not touched.** The plan's `lent` was for `head`'s
   range check; since upstream's heads copy `head` reads `self.heads`, a
   pointer, and names no `MmapBytes` method.
4. **`Blob::record_region_place` stays public.** tests/blob.rs (G1-4's
   held-to-the-file trace) calls it; it is an offset and answers for both
   kinds of reader, so hiding it serves no rule.
5. `Paging` is `pub(crate)` with `base(&self, _: Base)`; `bytes::Base(())`
   is `pub(crate)` with a private field, so only `bytes` makes one: the
   plan's shape, and no wall count (the token is the seam's own).
6. `Blob::record_regions` and `record_pieces` are one `pub(crate) fn
   records(&self) -> Option<Records<'_, B>>`, `Records::{Whole, Pieces}`;
   the callers (zfile's build, both arms; `sample`; db.rs
   `Seg::records_by_descriptor`) name both arms, no wildcard; zfile's tests
   go through one helper `whole_records`. The two lines of
   tests/data/cost-entries.txt for the removed public methods are gone (the
   list goes down).
7. `warm_file`'s touch is `MmapBytes::touch(off, len)` (crate-private, in
   the seam): the first byte of each page of a mapped file's range, nothing
   of a paged one (lendable length zero), the wrapping sum returned. The
   pagemap test reads the address through `#[cfg(test)] MmapBytes::addr`.
   vault's test that a kept file "lends nothing that was not asked for"
   asserted `as_slice().is_empty()`; it asserts `touch(0, len) == 0` now
   (the body's bytes are not zero).
8. The sentences G1-5 owed at `Own` and at `MmapBytes::map()`: a third kind
   answering `None` at `map()` would read as authenticated.
9. tests/blob.rs's `Advised` forwards `lendable`. The other outside impls of
   `Bytes` (tests 14, examples 1) take the default, which is the old
   default `lend_at`'s rule; none overrode `lend_at`.
10. Not edited: `ask_file`'s item allow (this unit does not edit that
    function).
11. **`Blob::open_with`'s row dispatch: G1-5's reviewer's shape tried and
    refused by the count.** The old `if crc && verify && key.pieces(&src)
    .is_none() { the whole arm }` with `else if crc && verify { if let
    Some(p) = key.pieces(&src) { verify_row_by_piece(..)? } }`, counted
    against e9a6abff (cost3.log, probes load_durable load_buffered seal
    merge land): RED, `Blob::open_with` +296 over the land probe's run
    (2,806 to 3,102) and +793 in the merge's (9,553 to 10,346), where G1-5's
    `match key.pieces(&src)` reads level (cost2.log: no move). The match
    stays; the three instructions G1-5 put on an open stay, nothing a key.

12. **scripts/ratchet-red.sh's fixture no longer copies the tree's raises.**
    With the couplings raise declared, the lists group went RED at its red
    input (`ratchet-red: l_deregistered: exit 2, wanted 1`): the fixture
    copied the tree's `scripts/raises.txt`, and the case that takes
    couplings.txt out of the tree's registry then failed on the raise ("no
    list of the registry") and not on the deregistration it is for. The
    fixture takes the file's header alone now; every case declares its own
    raises (`decl`). Any unit that declared a couplings raise would have met
    it. After: `SUPDB_BASE=e9a6abff sh scripts/check.sh lists` green (66
    cases; couplings held, 74 keys at the base, 75 in the tree, 1 raised,
    the raise let through; cost-entries 332 to 330), and the default base
    green ("new since the base").
13. CLAUDE.md gains one paragraph, the rule this unit's count taught (a
    bound restated in a generic caller ahead of the override does not fold;
    counted, not assumed), after the `emit_unsealed` paragraph.

## Evidence (logs in this directory)

- Red before: `before-2.log` (six compile_fail doctests FAILED, status
  101); `before-1.log` (the seal's red input red, for the reason above).
- After: `after-red.log` (every doctest ok; seal red ok), the source binary
  75/75 with the wall's raise and the cost entries lowered.
- Whole suite: `after.log` (`cargo test --profile checked --no-fail-fast`,
  EXIT 101): every binary green but tests/durable.rs
  `a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open`, which
  panicked at durable.rs:6952 (`db.commit().unwrap()` on "a test failed this
  landing": the segment work's left failure answered by the second commit,
  a test that reads the failure's answer at a commit; nothing of this
  unit's diff is on that path -- seal landings, the segment work). Run alone
  five times, five green (`durable-alone.log`); green again in the stride
  group's whole run. Not a known flake of the launch's list; reported here
  with both outputs. After that run the vault test gained one assertion and
  src/lib.rs one doctest; `libdoc.log` reruns the lib's unit tests (534
  green) and the doctests (15 green, 1 ignored) on the finished tree.
- `stride.log`: `sh scripts/check.sh stride`, EXIT 0, on the finished tree
  (31 test results, none failed).
- `lint.log`: `sh scripts/check.sh lint` EXIT 0 (clippy `-D warnings`, the
  format gate, the door and its red input, the seal's red input).
- `wasm.log`: EXIT 0; wasm_bytes 130094, wasm_gzip 53427, floor 29292 /
  13032; the base e9a6abff built from `git archive` in a scratch directory
  (77 MB, removed): 130094 / 53426 / 29292 / 13032 (`wasm-base.txt`). The
  module's size is the same; its gzip is one byte more. No budget number is
  checked by the group (web/build.sh prints the sizes).
- `bench.log`: `sh scripts/check.sh bench` EXIT 0.
- `lists-base.log`, `lists.log`: green (item 12).
- Count: `cost2.log`, `sh scripts/cost.sh e9a6abff read scan scan_long
  read_z scan_z scan_long_z load_durable load_buffered commit_durable
  commit_buffered scan_at land` -> `cost: green`, EXIT 0: rows 1, 2, 3, 6, 8
  and the land probe, every hot function level (the toggled probes "0 of
  them moved" but commit_durable's and commit_buffered's one bounded `~`
  function, inside its bound; the whole probes' moves inside their slack).
  `Sec::get` and `Sec::lend` are inlined (no row of their own); `Blob::
  scan_whole`, `key_at`, `exts_at`, the read path through `supdb_get` and
  the public `scan_at` probe are the functions they are inlined into, all
  level. OrdIndex::head, seek_in_exact, plan_seek and Reader::cut_raw: not
  moved in land (OrdIndex's edits are at `over` and `mapped_heads`, where
  `lent()` is `as_slice()`'s body). `cost1-lendable.log` is the refused
  shape's RED; `cost3.log` the refused open_with shape's RED.
  `cost1-nospace.log`: the first attempt, which died at the head's link for
  the disk (bench/target/release/examples and bench/target/checked/
  incremental were removed by their literal paths to make room).
- Mutants: `mutants.md`, eleven, all killed.
- strace: not taken. The unit names no plain-store syscall, and its diff
  adds none: the warm's touch is the same loads through the seam
  (`MmapBytes::touch`), the rest is visibility and a match.

## Repair round 1: what changed and its evidence (logs under r1/)

The two blocking items (one finding, from both reviewers: `lent` with no
token) are fixed by giving `lent` the tokens (departure 2, rewritten); the
nine extra mutants are run, each killed (mutants.md, "Repair round 1"),
three of the nine expected to survive by the reviewers and killed by what
this round added:
- `tests/source.rs`: `seal_check`, `the_seals_tokens_are_made_only_where_they_are_spent`
  and its red input `a_seals_token_made_another_way_fails` (eleven routes
  round the seal, each a compiling edit of the tree's sources, each red
  with its own words). Red before: `r1/before-source.log`, the test run
  (by `SUPDB_WALL_ROOT`) against round 0's tree (`r1/before-tree`, HEAD
  with `keep/final/` over it): eight errors, the six tokenless `lent`
  calls, the two token modules missing, blob's derive. After: green
  (`r1/after.log`, the source binary 77/77).
- `scripts/seal-red.sh`: fifteen lines more (the `lent` tokens at home and
  abroad both ways; `lent()` bare; `lent(())`; `Default::default()` for
  every token; each token by its field literal; an impl of the readers'
  trait abroad): `r1/seal.log`, "the 9 lines with a home compile there;
  all 20 fail abroad, each at its line" (the red-of-red cases add a line
  of their own, 10 and 21). Against round 0's tree
  (`r1/before-seal.log`) it is red at its home phase, because the tokens it
  names do not exist there: red, but not evidence of anything but that.
- `scripts/seal-red-red.sh` (new, in the lint group after seal-red.sh):
  seal-red.sh run with a line of each case's own (`SEAL_RED_EXTRA`), a line
  that does not compile at home, one that compiles abroad, one with no
  module abroad; each must exit 1 with its check's words
  (`r1/seal-red-red.log`). Mutants X9a and X9b (a check of seal-red.sh
  taken out) read red there.
- `src/bytes.rs`: `a_touch_reads_each_page_of_its_range_once_and_nothing_past_the_file`
  (X8a, X8b: the warm test was not shown to see either).
- `CLAUDE.md`: one paragraph after round 0's, the rule this round taught
  (a count is chosen among the shapes that hold a rule; what a seal cannot
  see is held by the source).

Gates, on the finished tree:
- `r1/after.log`: `cargo test --profile checked --no-fail-fast`, EXIT 0, 27
  test results, none failed (no flake this run).
- `r1/lint.log`: `sh scripts/check.sh lint` green (clippy `-D warnings`,
  the format gate, the door and its red input, the seal and its red
  input). `r1/clippy.log`: clippy `--profile checked --all-targets -D
  warnings` clean; `r1/fmt.log` the format gate.
- `r1/lists-base.log`: `SUPDB_BASE=e9a6abff sh scripts/check.sh lists`
  green, the raise 0 -> 3 let through by its declaration; `r1/lists.log`
  the default base, green.
- `r1/wasm.log`: green; 130094 bytes, gzip 53425 (base 130094 / 53426);
  `lent` and its tokens are not built for wasm (the db and ordindex modules
  are not), and blob's token lost a derive.
- `r1/bench.log`: `sh scripts/check.sh bench`.
- `r1/cost.log`: `sh scripts/cost.sh e9a6abff` over all fifteen probes
  (rows 1-8, work, merge, land, seal) -> `cost: green`, EXIT 0: every
  toggled probe "0 of them moved" among the hot functions but land's
  bounded `Maint::join_seal` (+0.002 an op, bound 0.010) and
  commit_buffered's bounded one; the whole probes' moves inside their
  slack. `OrdIndex::over`, `mapped_heads`, the four stride readers: no row
  of their own moved (`lent` inlines, its token a zero-sized value);
  OrdIndex::head, seek_in_exact, plan_seek and Reader::cut_raw not moved in
  land. The base's target (`r1/cost-tb`, 5.1 GB) removed after.
- strace: not taken; the round adds no syscall (a zero-sized argument and a
  test).

## What the seal holds, and what it cannot see

Holds, by the compiler, shown by `scripts/seal-red.sh` (lint group; its
own red input `scripts/seal-red-red.sh` right after it) and the doctests
(test group):
- `Bytes::lend_at`: called only in `blob` (E0624 abroad), not overridable
  or callable from outside (doctests; E0603); no token by
  `Default::default()` (E0277 abroad; outside, a compile_fail doctest) nor
  by its field (E0603).
- `MmapBytes::lent`: called only in `ordindex` and `db::zfile`, each with
  its own token (E0624 abroad, both ways), not without a token (E0061),
  not with `()` (E0277), not with a token by `Default` (E0277) or by its
  field (E0423); the readers' trait not implementable abroad (E0603);
  outside the crate not at all (doctest).
- `Paging::base`: its token made only in `bytes` (E0603 abroad), not by
  `Default` (E0277); `Paging` crate-private (doctest).
- `MmapBytes::as_slice` gone; `Blob::record_regions` / `record_pieces`
  gone from the public surface (doctests); `Blob::records` makes every
  in-crate caller name both arms -- a wildcard is still writable, and its
  mutants are killed by tests, not by the compiler.
Holds, by tests/source.rs (`the_seals_tokens_are_made_only_where_they_are_spent`,
its red inputs in `a_seals_token_made_another_way_fails`), what the
compiler cannot see: each token module is exactly its one text (no
derive, no field, no second function); `Base` is `pub(crate) struct
Base(());` with no attribute but its cfg; no `impl` gives a token (or the
readers' trait) anything but the seal's own; every token is made as the
last argument of the call it is for, in a scope `SEAL_CALLS` lists, so a
token by a function that hands one out, by a binding, or by unsafe code
(`zeroed`, `transmute`) is red, and so is a call of `lend_at`, `lent` or
`base(..)` outside its home, whatever its argument; and a new caller of
`lent` inside its own home module is red until `SEAL_CALLS` lists it.
Cannot see:
- A whole-range `slice_at` (R2 allows it; G2's conform pass is to hold it).
- An in-crate reader that writes `if src.piecewise() { nothing }`.
- (Round 2: the source test reads names as identifiers, not text; see
  "Repair round 2". A call split so its token is on another line is red, the
  safe side. Round 3: inside a macro's tokens -- an invocation's delimiters
  and what they hold, a `macro_rules!` body -- `lent` and `lend` are refused
  in every position (`seal_in_macro`), but four test assertions that name a
  local `lent`, listed by file, scope and text (`SEAL_MACRO_WORDS`); so a
  name handed to a macro as an identifier and put where a call or a path
  goes is red, which round 2's reading by position was not (see "Repair
  round 3"). What it cannot read is a name a macro builds from pieces --
  `concat_idents!` is unstable, `paste` is no dependency -- and code
  included from outside a file, which the wall refuses everywhere, "code
  from outside the file", at a count of zero. `base` is not refused inside
  a macro: a call of it needs a `Base`, whose name is counted in any
  position in the seam and as `bytes::Base` elsewhere, and whose private
  field confines its making to the seam.)
- The doctests check that a snippet fails to compile, not why (rustdoc on
  stable checks no error code); each has a twin that compiles, and the
  seal's red input does print the error codes in-crate.

## Not verified

- That rustdoc on this toolchain ignores error codes on compile_fail (not
  tried: the doctests carry none).
- The wasm module's content beyond its sizes (the same length, gzip one
  byte more).
- (round 0's "M5's kill by the vault test's first assertion alone" is
  verified in round 1: mutant X4.)
- macOS and aarch64 builds; `cross-arm` not run.
- The durable test's red in the whole run: not diagnosed beyond "passes
  alone five times and in the stride run".
- No timing of anything (rule 8). Nothing starts to work under a cipher on
  a default's path because of this unit: the warm under a cipher touches
  nothing as before (G1-6's behaviour), the records' arms are the ones the
  code took before, behind `ZPages::Always`/the controller as before.

## Owed to G1-8 (docs)

- docs/zpages-design.md names `Blob::record_regions` (a design record;
  left).
- The prose of `Bytes::lend_at`, `MmapBytes::lent`, `Paging`, `touch` and
  the crate docs' section "What the read path does not hand out" are this
  unit's; G1-8 reads them against the code.

## Last edit, after the groups

`MmapBytes::addr` is `#[cfg(all(test, target_os = "linux",
not(supdb_emulated)))]`, the cfg of its one caller (the pagemap test), so
no build of the tests leaves it dead. Checked: the pagemap test green;
clippy `--release --all-targets -D warnings` clean; the format gate; and
`RUSTFLAGS="--cfg supdb_emulated" cargo check --profile checked --lib
--tests` with no warning, into a target of its own (237 MB, removed after).
No other file changed after the groups; the count (cost2.log) is of the
same code (the attribute is a test build's).

## Repair round 2: what changed and its evidence (logs under r2/)

The one blocking item: the seal's source test read a token made and a call
made as substrings (`Token::new`, `Base(())`; `lend_at(`, `.lent(`,
`.base(`), and four spellings rustfmt keeps read as neither -- `<lend::Token>::new()`,
a renamed import, a call by type `MmapBytes::lent(&m, t)`, a call through a
function value -- which together made the route CLAUDE.md said was held.
Fixed in the check (tests/source.rs), not by moving the borrow, so no
compiled engine code changes:
- `seal_names` reads each line's identifiers off the lexer's `flat` text
  (literals blanked): a call by its method's name, `lend_at` anywhere but
  after `fn`, `lent` after `::` or after `.` with a call, `base` after `.`
  with an argument or after `::` not followed by `()` (`.base()` and the
  fields `self.base`, `..base` are other things'); a token by every name it
  can be reached by -- `lend` as a path (`lend::`, `mod lend`, any `use`
  item, multi-line ones followed to their `;`), the names `LendToken`,
  `OrdLendToken`, `StrideLendToken`, and `Base` (any in the seam,
  `bytes::Base` elsewhere; `FlatIsa::Base` in zlz4 is not it).
- Step 4: a line that names a token and makes no call is one of
  `SEAL_DECLS` (the three `mod lend {`, the imports and re-exports, the
  signatures taking a token, the seam's two impls, `Base`'s definition),
  exact by file and text, or red ("names a seal's token outside the call it
  is made for"); a line with a call is in a listed (file, scope) of
  `SEAL_CALLS`, and makes exactly one token per call, as written at the
  call (`lend::Token::new())`, `Base(()))`), with no other token name on
  the line ("with a token not made at the call").
- `seal_outside_check` over tests/, examples/ and bench/ (not `target`):
  `lend_at` named nowhere outside the crate, where only unsafe code can
  make its token (E4). The tree test asserts tests/blob.rs and
  bench/src/engines.rs were read.
- Bases outside the seam are not all counted (an import group `use
  crate::bytes::{Base, ..}` is not): a `Base` is made only in the seam
  (private field, compiler), every `Base` named there is a listed line, and
  `base` with an argument is red outside `MmapBytes::paged`, so a Base held
  elsewhere cannot be spent. Said in the notes, not in a red case.
- Red before (`r2/before.log`): the new test
  `a_seals_token_spelled_another_way_fails` against round 1's check, six of
  its seven cases "passed the check" (E1's two halves, E2, E3's import,
  the glob `use lend::*;`, the address by a function value); the seventh,
  OrdLendToken by name in an unlisted fn, was red by the old words. E4's
  case and the five added after (the macro, `use self::lend as mk;`, a
  second token on a listed line, the two re-export names) went in with the
  new check, not before it: not red-first evidence; their evidence is the
  mutants O1-O6.
- CLAUDE.md: the round-1 sentence ("holds ... every token to the call it is
  made for ... with a red input for each route") replaced by what this
  round found and the rule (a check over source holds a name by every
  spelling the language gives it, its red inputs the spellings), and what
  it cannot read (a name a macro builds from pieces). tests/source.rs's
  comment above SEAL_TOKENS likewise.

The other extra mutants (mutants.md, "Repair round 2"): E5's question, T1
and the seal-red-red words check each needed a test:
- vault: `assert_eq!(m.lends(), 0)` (`MmapBytes::lends`, `#[cfg(test)]`,
  the lendable length) holds a paged mapping to lending nothing whatever its
  bytes; and the fixture's premise (a byte not zero at every page start)
  asserted, since the touch's zero sum says nothing without it.
- the touch test's fixture: bytes of 100 at 1, page/4, page/2 and page-1 of
  every whole page, which a touch reads none of.
- seal-red-red.sh: `case_` run once in a subshell with words seal-red.sh
  never prints, required to fail ("words held").
- W1 is killed by the warm test's partial-read half, three runs alike; D1
  by the dormant-files cipher test; B1 equivalent (argued in mutants.md).

Gates on the finished tree (`keep/r2/`, sums `keep/r2.sha256`, equal to
the working tree):
- `r2/after.log`: `cargo test --profile checked --no-fail-fast --lib --test
  source --test blob --test zpages --test cipher` (535, 21, 26, 78, 233, all
  green) and `--doc` (16, 1 ignored), EXIT 0.
- `r2/lint.log`: `sh scripts/check.sh lint` EXIT 0 (clippy `--release
  --all-targets -D warnings`, the format gate, the door, seal-red.sh,
  seal-red-red.sh with its words case).
- `r2/lists-base.log`: `SUPDB_BASE=e9a6abff sh scripts/check.sh lists`
  EXIT 0; `r2/lists.log` the default base, EXIT 0.
- Not rerun this round, and why: the whole suite, wasm, bench, stride and
  the count. This round's change under src/ is test code alone
  (`r2/src-diff.txt`: a `#[cfg(test)]` accessor and two test bodies), so
  the release build, the wasm module and the suite's binaries are the
  round-1 build's, whose logs (`r1/`) stand; the count's rows are of
  release code and did not move. Round 1's whole-suite run (`r1/after.log`)
  is the last whole run.
- strace: not taken (no syscall of any build changed).

## Repair round 3: what changed and its evidence (logs under r3/)

The one blocking item: `seal_names` counted `lent` only after `.` with a
call or after `::`, and `lend` only before `::`, in a `use` or after
`mod`, so the names handed to a macro as identifiers were read by nobody:
`ty!(lend)` / `mk!(lend)` (expanding to `$m::Token`, `$m::Token::new()`)
handing the ordered index's token out, and `call!(m, lent, hand())`
(expanding to `$b.$m($t)`) spending it in db.rs. Mutant X-macro-hand shows
it compiles clean (clippy `-D warnings`, status 0). Fixed in the check
(tests/source.rs), with no file under src/ changed:
- `seal_in_macro` marks the characters inside a macro's tokens, carrying
  the bracket depth and the open macros across lines: a delimiter after
  `name!` (any whitespace between, the name not a keyword, so `if !(..)`
  is no macro) or after `macro_rules! name`. In step 4 a line with the word
  `lent` or `lend` at a marked character is red ("names `lent` or `lend`
  inside a macro, where the macro chooses its place"), unless it is one of
  `SEAL_MACRO_WORDS`: four test assertions naming a local `lent`
  (crypt.rs, db.rs, pager.rs, flatindex.rs), keyed by file, scope and text,
  so the same text in another function's macro (`lent,` on a line of its
  own) is red. Of the reviewer's two repairs, this is the second widened:
  the first (the words counted everywhere with the non-seal uses listed)
  would have listed some fifty lines of db.rs's upkeep (`lend`, `lent` as
  locals and fields), every edit of which would go red; the second as
  written (refuse macros using a fragment as a method name or path head)
  would refuse wasmapi.rs's `on!`/`any!` (`x.$m(..)`) and leaves the
  one-line `macro_rules!` body `() => { use self::lend as mk; }` (an
  import the invocation `u!()` does not name; X-macro-use) to a
  fragment-free shape. Refusing the two words inside every macro holds
  both: what a macro forwards has to be written in an invocation, and what
  it emits is written in its definition.
- Red cases added to `a_seals_token_spelled_another_way_fails`: the
  reviewer's X-macro-hand (both halves), X-macro-home, the X-macro-blob
  control (red by `lend_at` as before and now by `mk!(lend)` as well), the
  `lent,`-on-its-own-line call (holds the scope in the key), the one-line
  macro body that imports the module as `mk`, and `pub(crate) use
  self::lend as mk;` (extra mutant 6). A green case beside them, which
  must pass: `Others::base()` (extra mutant 5), `0..lent(n)` (extra mutant
  7) and `if !(lent(n) > 1)` (the keyword list). The tree test asserts
  `examples/crypt.rs` was read (extra mutant 4).
- Red before (`r3/before.log`, the new cases against round 2's check):
  three "passed the check" (the reviewer's two edits and the zfile one) and
  the blob control red by `lend_at` only, status 101. The `pub(crate) use`
  case and the green case passed already (the first held by the existing
  third disjunct of `opens_use`, which extra mutant 6 shows is the line
  that holds it). The `lent,` case and the one-line body case went in with
  the new check, not before it: their evidence is mutants R1 and R2.
- CLAUDE.md: round 2's sentence "a call by its method's name however it is
  reached ... a name made inside a macro from pieces is the one it cannot
  read, and the crate has no macro that makes one" was false (the forwarded
  ident is not built from pieces, and blob.rs has three macro_rules); it
  says now what is read (a call where a call puts it, outside a macro; the
  two words in any position inside one) and what the second review taught.
  The comments above SEAL_TOKENS and SEAL_CALLS likewise.

Gates on the finished tree (`keep/r3/`):
- `r3/after-source.log`: `cargo test --profile checked --test source`,
  78 passed, EXIT 0 (the lexer's unit tests are in this binary).
- `r3/lint.log`: `sh scripts/check.sh lint` EXIT 0 (clippy `--release
  --all-targets -D warnings`, the format gate, the door and its red input,
  seal-red.sh, seal-red-red.sh).
- `r3/lists-base.log`: `SUPDB_BASE=e9a6abff sh scripts/check.sh lists`
  EXIT 0; `r3/lists.log` the default base, EXIT 0.
- Mutants: `r3/results.txt`, fifteen, all killed (mutants.md, "Repair
  round 3").
- Not rerun this round, and why: the whole suite, wasm, bench, stride, the
  count and strace. No file under src/ changed this round (`keep/r3/` and
  `keep/r3start/` compared file by file: tests/source.rs and CLAUDE.md
  only), so every build of the crate, the wasm module and the suite's
  binaries are round 1's and round 2's, whose logs stand (`r1/after.log`
  the last whole-suite run, `r1/cost.log` the last count, both at the same
  src/ as now).
