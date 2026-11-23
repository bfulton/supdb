# G1-1 wall: notes

Base ea635218 (HEAD == base, no earlier unit of this run). Tree clean at start.

## Step now
1. reading done; step 2/3 (generator with the check's corrections, then the Rust test).

## What the tree has that the plan read at 682b2c09 did not
- Same 20 files under src/ (lib.rs included). No new module.
- lex.py's depth self-check closes at 0 in all 20 files of today's tree.
- gen.py (old tokens, old door list) over today's tree: form code db.rs 297 (was 232),
  zfile.rs 15 (12), ordindex.rs 3 (2), blob.rs 7 (7); enc vault.rs 111 (110),
  pager.rs 2 (0). Keys new since the old tree: see gen_today.tsv vs gen_old.tsv
  (k_old / k_today in this directory).
- vault.rs gained plain-store door functions since: copy, dir_exists, open_rw,
  open_lock, sync_file, sync_slot, sync_dir_as/DirSync, the guards (guard, guarded,
  adopt_guard, forget_guards, GUARD_SUFFIX), dir_ticket/dir_synced_after, and the
  I/O trace seams (note, note_file, trace_io, traced_descriptors, watch_dir,
  unwatch_dir, hook_dir_sync/DirHook). Also on_device, holds_envelopes.

## Restart at 07:05 (second builder)
- Step 0: `git status --porcelain` showed ` M scratch/plan/build/g1-build-v2.js` once
  (mtime 07:05:09, after the restart); content identical to the index blob
  (hash-object == ls-files), a second status was empty. Stat-only; tree clean.
- py/gen2.py re-run over today's tree: today2.tsv == today.tsv (byte-identical). Reused.
- py/staged.tsv reviewed: 238 carried, 78 by rule, none unstaged. One change:
  `OrdIndex::head_absent` was carried as G1 from the old file; the plan's
  "Changes to the contract" 2 leaves the ordered index's skip, and the
  function is the lazy top's assertion, so it is `stays` like `OrdIndex::over`.
- Design: tests/source.rs (the wall) + tests/source/lex.rs (the lexer module
  and its unit tests). Token matchers hand-written (no regex dev-dependency;
  the patterns use lookaround, which the regex crate lacks). File lists
  explicit in the test (form list enumerated, so a new file fails).
  couplings.txt produced from the Rust print (SUPDB_WALL_PRINT=1) joined
  with staged.tsv by py/stage.py; never by the test.
  SUPDB_WALL_ROOT=<dir> points the gate at another tree, for the red input
  as a script step (red.sh).
- Step 3 done: tests/source.rs (the wall, 9 tests) + tests/lex/mod.rs (the lexer
  module, 15 unit tests; `mod lex;` resolves to tests/lex/mod.rs as `mod common;` does).
  Rust measurement == py/gen2.py over today's tree, all 316 (file,scope,token,region)
  keys and counts identical (rust.tsv vs py.tsv; diff empty).
  tests/data/couplings.txt written by py/write_couplings.py from rust.tsv + py/staged.tsv
  (+ the head_absent fix). red.sh = the red inputs as script steps; before.log has them.
- Next: fmt, clippy (lint group), whole suite --no-fail-fast -> after.log, mutants.
- keep/ holds the finished copies (source.rs, lex_mod.rs = tests/lex/mod.rs, couplings.txt) with SHA256SUMS. Whole suite running -> after.log.
- .github/CODEOWNERS added (the plan's Leaves: a CODEOWNERS line on couplings.txt). Owner handle @bfulton read off the origin remote's path: NOT VERIFIED that it is the owner's login.

## What the gate holds (draft; finished at step 8)
- tests/source.rs `the_forms_and_the_cipher_do_not_name_each_other`: totals by
  (file, token, region) of the cipher tokens in the 14 form files and bytes.rs, and of
  the form tokens in the 4 encryption files, against tests/data/couplings.txt; growth
  and shrinkage both fail, the scopes and hit lines printed. Any src/*.rs on no list
  fails, and so does a listed file that is gone. Stage column validated against STAGES;
  a key listed twice fails.
- `every_file_closes_at_depth_zero`: the lexer's self-check over every src file.
- In-binary red tests: growth in warm_file and open_seg, a comment green, a move between
  scopes green, a removal red until lowered, a test-region coupling counted apart, an
  unclassified file and a missing one, the encryption side naming a form, token patterns.

## Today's counts (gen2 == Rust)
form code 290 (db.rs 268, zfile.rs 12, blob.rs 7, ordindex.rs 3), form test 13,
seam (bytes.rs) code 27, enc code 138 (vault.rs 136, pager.rs 2), enc test 272;
`passes(` in vault.rs 25 code + 1 test; `use` of an encryption module 0;
`vault::real` 6. Plan's 259/238/23/0/6 were 682b2c09's.
- Whole suite run 1 (after.dotdot-tmpdir.log): EXIT=101, five failures (zpages: 4 mapping
  tests; leak: a_closed_store_gives_back_what_it_allocated). Cause: my TMPDIR was spelled
  `.../build/G1-1/../tmp/G1-1`; tests/common's zpg_mappings matches /proc/self/maps (which
  names canonical paths) by the prefix dir.join("zpg-"), so a `..` in TMPDIR maps nothing.
  Each alone 5x with the same TMPDIR: 25/25 red (rerun.log); with the canonical spelling:
  25/25 green (rerun2.log). Not the diff's. Whole suite re-run with the canonical TMPDIR
  -> after.log.
- Whole suite run 2 with the canonical TMPDIR: after.log, EXIT=0, 27 binaries ok.
- Clippy (lint group's command, --release --all-targets -D warnings): EXIT=0 (clippy.log);
  fmt.sh --check: 0 (fmt.log). red.sh after the last edits: red_after.log, all as expected.
- Mutants: mutants.md (18 + one re-form; 3 survivors got tests, 1 equivalent).
- CLAUDE.md: one sentence in the TMPDIR paragraph (the `..` failure), the paragraph's tail
  rewrapped. keep/CLAUDE.md is the finished copy.
- TMPDIR directory removed by its literal path. No scratch tree or target dir left
  (red.sh removes its redroot).

## STATUS: unit done, uncommitted. Finished copies: keep/ (sha256 in keep/SHA256SUMS:
## source.rs = tests/source.rs, lex_mod.rs = tests/lex/mod.rs, couplings.txt, CODEOWNERS).

## Departures from the plan
1. Lexer at tests/lex/mod.rs (`mod lex;` from tests/source.rs, as tests/common is), not
   inside tests/source.rs: one module with its own unit tests, as asked; tests/source/lex.rs
   is not where a crate-root test file's `mod` looks.
2. Tokens compared against py/gen2.py (gen.py + the check's corrections: vault::NAME keyed by
   name, vault::real counted, `passes(`, bytes.rs as seam, wasmapi.rs and blocks.rs on the
   form side, PartWalk::Walked excluded from "held build"), not raw gen.py: the plan asks for
   the corrected tokens, and gen.py lacks them. Result identical key for key (316 keys).
3. Door list widened beyond the check's ten by the plain-store door functions added since
   682b2c09 (copy, dir_exists, open_rw, open_lock, sync_file, sync_slot, sync_dir_as/DirSync,
   guard, guarded, adopt_guard, forget_guards, GUARD_SUFFIX, dir_ticket, dir_synced_after,
   note, note_file, trace_io, traced_descriptors, watch_dir, unwatch_dir, hook_dir_sync/DirHook):
   each is a plain store's file operation or its trace through the door, names no cipher.
   `on_device`, `holds_envelopes` stay counted (cipher questions). FOR THE REVIEWERS.
4. The red input in warm_file reads "vault::kept code: 2 > 1" not "growth from 0": by
   decision 12 the gate is per (file, token, region) and open_seg already holds one; the
   scope line printed under it says "warm_file: 1 (listed 0)".
5. `cfg(any(test, x))` is read as code (compiled where x holds), `any` all-test as test;
   lex.py read neither `any` as test. No such cfg in src/ today, so no count moves.
6. Line scope semantics kept lex.py's (scope open at line end, else at line start), so a fn
   on one line is keyed by its parent; a unit test says so. Kept for parity with the old
   file's keys the stages were carried by; scopes are informational under decision 12.
7. head_absent restaged G1 -> stays (the plan's change 2 leaves the ordered index's skip).
8. Added .github/CODEOWNERS (the plan's "Leaves" says the line is built here). The handle
   @bfulton is the origin remote's owner path: NOT VERIFIED as the owner's login.
9. SUPDB_WALL_ROOT (another tree for the gate), so the red input is a script step (red.sh)
   whose verdict is the gate's exit status; the same inputs are also in-binary tests.

## Lines staged by rule (no old line): py/staged.tsv rows with "rule" (78), listed by
`awk -F'\t' '$8=="rule"' py/staged.tsv`. In short: bytes.rs 21 lines `stays` (the seam);
vault.rs `passes(` 24 lines S6; vault::real 6 sites S6; Paged::make (regenerable, ".z" and
"zpg") S1 per decision 19; new db.rs scopes (wal_set_aside, Wal::write_sealed, guard_names,
run spares) S6; map_resident_share G1 (G1-6); Db::open_refusals, Db::checkpoint,
Db::restart_cipher_peak, Entered::finish, Reader::leave_by stays; Db::close_run S4
(conditional); zfile name_built S6; ordindex over/mapped_heads/test stays;
Blob::scan_at_in stays; vault Vault::holds_segments and a_guard_goes_before_its_file S6.

## What it cannot see
- A cipher branch spelled in words not on the token lists (token completeness, forms.md 6);
  a neutrally named predicate in bytes.rs.
- Anything outside src/ (tests, bench, examples, web).
- Cross-commit growth: the test sees one tree; CODEOWNERS is the only hold.
- Comments (by design).
- A move of a coupling between scopes of one file (by decision 12); between files is growth
  in one and shrinkage in the other, both red.

## Callgrind
None: the diff is tests/source.rs, tests/lex/mod.rs, tests/data/couplings.txt,
.github/CODEOWNERS and CLAUDE.md; `git diff --stat -- src` empty. No compiled engine code.

## Not verified
- The CODEOWNERS handle (8 above).
- That the lexer reads every future shape right: held only on today's 20 files (the depth
  self-check and gen2 parity) and the unit tests.
- Stages are predictions of later units (R4, S1, S4, S6 ...); G1 lines (records_resident,
  map_resident_share: G1-6) assume G1-6 removes `paged_resident`.
- Whether apple-silicon CI runs tests (irrelevant here: the test needs no Linux).

## Repair round 1 (three blocking findings)
Step: red tests in (before_r1.log: 5 reviewer inputs red as in-binary tests, EXIT=101), gate
changed, couplings.txt raised by 9 lines, green (33 tests); red.sh extended with the five
reviewer inputs as script steps (red_after_r1.log: all as expected, EXIT=0); fmt ok
(fmt_r1.log). Mutants: mutants_r1.py -> mutants_r1.results / mutants_r1.log. keep/ refreshed
(SHA256SUMS). Then clippy, the neighbours, notes, mutants.md.
1. lib.rs measured (EXEMPT renamed ROOT, counted with the cipher tokens, every line `stays`).
   New tokens, all sides that take the cipher's tokens: "item used from an encryption module"
   (file-level: each item a `use` takes from vault/pager/crypt/dirkey, renamed items by their
   new name, `self` as the module, a bare module as one) -- catches a name added inside an
   existing re-export's braces, which the per-line `use` count does not; "crate-root alias of an
   encryption item" (any word the root re-exports from an encryption module under a name no
   other cipher token catches: today only DirState, once in db.rs Db::checkpoint, staged `stays`
   with that scope's dirkey lines). DEPARTURE from the plan (lib.rs exempt): reason = the
   reviewer's input (`pub(crate) use db::vault::kept as keep_named;` + `crate::keep_named(path)`
   in warm_file passed the gate).
2. Every file under src/ is read; a non-.rs file fails ("is not a .rs file the wall reads").
   "code from outside the file" (`include!`, `#[path]`/`#![path]`, whitespace allowed) counted on
   both sides; zero today (include_str! is not include!, and the two in src/ are tests reading text).
3. "macro" (`macro_export`, `macro_rules`) counted on the encryption side; zero today.
4. `vault` called by its path (`Held::vault(h)`) or as a method with args/spaces counts under
   "vault/pager method" (replaces the `.vault()` literal; today's count unchanged, 6 in db.rs).
New couplings.txt lines (for the reviewers): db.rs Db::checkpoint crate-root alias 1 stays;
lib.rs <item> cipher 4, crate-root alias 1, crypt 1, dirkey 2, item used 8, pager 1, use of an
encryption module 2; lib.rs `mod crypt` crypt 1 -- all `stays`.
Engine: nothing under src/ changed (git diff --stat -- src empty); no callgrind rows.
Mutants r1: 7 reviewer + 10 own; 2 survivors got tests (a_use_names_its_items_and_a_bare_module_is_one,
code_from_outside_is_every_spelling_of_the_two); all killed (mutants.md). Clippy (lint group's
command) EXIT=0 after a `type Edit` alias (clippy_r1.log); fmt.sh --check EXIT=0 (fmt_r1.log);
source binary 35/35 (after_r1.log); red.sh all as expected (red_after_r1.log). Whole suite
--no-fail-fast -> after_r1_suite.log (no other test binary includes tests/lex or reads couplings.txt).
What the wall still cannot see, added: a glob re-export (`pub use db::vault::*`) brings names the alias
set does not know -- the root's use count and item count grow (red at lib.rs), so it cannot land
silently, but a form's later use of a globbed name is then counted by nothing but the names' own tokens;
a macro defined on the form side that names no cipher token; a `use` hidden in a string literal is read
as a use (overcount, red, never silent).
Finished copies: keep/ (SHA256SUMS: source.rs, lex_mod.rs, couplings.txt, CODEOWNERS).
Whole suite r1: after_r1_suite.log EXIT=0, 27 binaries ok. TMPDIR directory removed by its literal path.
## STATUS (round 1): repairs done, uncommitted; finished copies in keep/.
## Departure 10: src/lib.rs measured (the plan had it exempt), every line `stays` -- reason: the
## renaming re-export the reviewers built passed an exempt root.

## Repair round 2 (two blocking findings)
Finished copies: keep/r2/ (SHA256SUMS there: source.rs, lex_mod.rs, couplings.txt, CODEOWNERS);
keep/ itself still holds round 1's.
1. The encryption side naming the engine (reviewer inputs (a) impl super::Db in the vault, (b) use
   super::{Options, Shared}, (c) impl crate::bytes::MmapBytes in the pager). New file-level tokens,
   each over the file's flat text (literals blanked; new Line.flat in tests/lex, one unit test):
   - encryption side "path out of the encryption modules": every path from `super`/`crate`/`$crate`
     (and `self::super`) resolved against the module it is written in (file path + inline `mod x {}`),
     followed through `::` segments, `super` climbs, groups (each element), globs; a hit where it
     leaves crate::{db::{vault,pager,dirkey}, crypt, bytes} or ends above them (`use crate::db;`,
     `super::*` from the vault); `use crate as x`, `extern crate` too. Today: vault.rs 2 (default_cache
     super::zfile::read_budget, G3; persist_one crate::db::Db::ord_name_for, S6, each the stage of that
     scope's existing line). dirkey's `super::refused` and the tests' `use super::*` resolve inside.
   - encryption side "impl of a type the file does not define" (self type = the header's last path
     word at depth 0, generics excluded, item position only): 0 today.
   - form side (and seam, root) "impl of a seam or encryption type" (a type defined in bytes.rs or an
     encryption file, not in this file): 1 today, db.rs Hold::<item> `impl MergeRest for &pager::Hold`,
     R4 like that scope's MergeRest and pager lines. Added because the mirror of (c) -- a form giving
     MmapBytes a method the pager calls -- passed the same way.
   - encryption side "macro from outside the file": a call `name!(..)` to a macro neither std (list)
     nor the file defines; a macro_rules in db.rs before `mod vault;` is in the vault's textual scope by
     its bare name. 0 today (Rust keywords before `!` excluded: crypt.rs `if !(..)` read as one first).
   - Bare `Db`/`Options`/`Shared`/`Reader` not counted: none can be named without a path or a `use`,
     both counted now (globs included).
2. `#[cfg_attr(.., path = ..)]`: "code from outside the file" is now file-level over the flat text, so
   an attribute across lines is read whole; `path` counts as an argument of any cfg_attr (nested too)
   when followed by `=`; every `include` word counts (a renamed `use std::include as inc` too).
3. vault::NAME is file-level over the code text with whitespace and line breaks skipped around `::`
   (reviewer's spaced and split inputs); the field-word test skips whitespace before `::` too.
Counts today: unchanged for every old token (round 1's lines all hold); 3 new lines in couplings.txt
(reasons end "new token (round 2)"), all FOR THE REVIEWERS.
Red first: before_r2.log = red.sh run with keep/'s round-1 source.rs/lex/couplings (the working
copies set aside and restored by sum): the 11 new inputs exit 0 (WRONG). red_after_r2.log: all 23 as
expected (24 with item 4), EXIT=0. In-binary: the_encryption_side_naming_the_engine_fails, a_path_out_is_resolved_against
_its_module, an_impl_names_its_self_type, a_macro_from_outside_is_a_call_the_file_does_not_define,
a_vault_name_may_be_spaced_or_broken_across_lines, a_crate_root_alias_is_a_whole_word,
a_use_of_a_nested_group_reads_to_its_semicolon, a_code_coupling_moved_into_a_test_fails_by_region, and
code_from_outside_is_every_spelling_of_the_two extended. Source binary 45/45 (after_r2.log).
Mutants: mutants.md "Round 2" (32 + one re-run; one survivor got a test).
fmt.sh --check 0 (fmt_r2.log); clippy lint command 0 (clippy_r2.log; --test source alone also 0).
Engine: nothing under src/ changed (git diff --stat -- src empty): no callgrind rows.
CLAUDE.md: unchanged this round; the rule it taught (a source gate reads names as the language
resolves them -- paths from their module, impls, macro scope -- not a word list of the other side) is
left to the plan's last unit, which writes the wall's notes; it is in the test's module doc now.
4. Found while writing these notes: `extern crate self as x` in lib.rs would let the vault write
   `x::Options` with no path from `super`/`crate`. New token "extern crate" on the form side, seam and
   root (the encryption side counts it under paths out); 0 today; test the_crate_named_by_another_name_fails
   and a red.sh step; its red-before is the mutant that drops the token (killed).
What it still cannot see, added: trait methods brought by a form trait
implemented in a form file on a form type are invisible and harmless; `impl` of a type defined by a
macro.
Whole suite: after_r2_suite.log EXIT=0 (27 binaries ok; run before item 4, which touched tests/source.rs only; the source binary after it: after_r2.log 45/45). red_after_r2.log: 24 inputs as expected. TMPDIR directory removed by its literal path.
## STATUS (round 2): repairs done, uncommitted; finished copies keep/r2/.

## Repair round 3 (three blocking findings) -- in progress
Start 08:27: working copies == keep/r2 (sums checked). Step: red tests and red.sh steps in, run red
(before_r3.log), then the fixes. rustc check: `use super as v;`, `use self::super as v;` and
`use super::{self as v}` all compile (edition 2021), so paths_out's `as` guard is reachable.
Red first: before_r3.log (08:29) -- the three new check-level tests FAILED (45 passed, 3 failed,
EXIT=101) and red.sh's four new steps read WRONG (exit 0), RED EXIT=1; every older step as before.
Fixes (tests/source.rs only; src/ untouched):
1. Form side (and seam, root): an impl's self type is resolved through the file's renames and aliases
   before it is checked (`enc_aliases`): a `use` leaf renamed `as X` whose path passes through `bytes`
   or an encryption module, or whose last segment is a seam/encryption type; a `type X = ..` whose right
   side names such a type, such a module followed by `::`, or another such alias, to a fixed point. The
   `own` exclusion no longer hides an alias (`type WallM = crate::bytes::MmapBytes` is defined in the
   file and counted). New `use_leaves` reads a use tree to its leaves (groups, `self`, globs, `$crate`).
2. The seam is measured with paths_out too ("path out of the encryption modules" at bytes.rs): its
   `use super::*` in tests resolves inside bytes, so 0 today; `pub(crate) use crate::db::Options as
   Engine` there is 1 > 0.
3. A std macro name is exempt only where no file under src/ defines a macro_rules of that name
   (`macros_defined`, `shadowed`). The first draft took the encryption files out of the set; a
   macro_rules on the encryption side is counted as "macro" already, so that exclusion changed no
   verdict any input could reach, and was taken out (rule: a check nothing reaches).
4. Found by the new unit test for the reviewers' path_group mutant: a `super` that begins a group
   element was resolved twice -- by the group walk from the path before it, and again by paths_out's
   outer loop as a fresh path from the file's module -- so `use super::{super::bytes::B}` in the vault
   read as a path out (an overcount, red, never silent; and it hid the climb's mutant, since the two
   hits were at one position and deduplicated). The group walk marks its elements `seen` and the outer
   loop skips them.
Counts today: no couplings.txt line changed (the gate over today's tree passes unedited; the seam's
paths out, today's renames/aliases in form files -- `Ordering as ..`, `Bytes as _`, `Write as _` -- and
std-named macro_rules are all 0).
New tests: a_seam_type_renamed_or_aliased_by_a_form_fails (4 spellings red; a generic shadowing an
encryption type's name and a rename of a form's own type green), a_seam_re_export_of_the_engine_fails,
a_form_macro_named_like_a_std_one_fails, a_use_tree_is_read_to_its_leaves,
every_std_macro_is_exempt_until_a_form_defines_one_by_its_name (calls every STD_MACROS entry, so an
entry removed is red whether or not an encryption file uses it today),
a_super_inside_a_group_climbs_and_a_super_alias_inside_is_inside (rustc accepts `use super as v;`,
`use self::super as v;`; from a module inside the vault both name the vault). red.sh: 4 new steps.
fmt.sh applied (rewrapped three test lines), --check 0. Finished copies: keep/r3/ (SHA256SUMS: source.rs,
lex_mod.rs, couplings.txt, CODEOWNERS, red.sh). Mutants: mutants_r3.py -> mutants_r3.results / .log.
Mutants r3: 22 + 1 own added in the rerun. Two survived the first pass (STD_MACROS without debug_assert_eq,
and without todo): my test had built its calls from the list. The test now writes them out and holds
the length; rerun killed all five list mutants (mutants_r3_rerun.log). Survivors: none. mutants.md "Round 3".
red.sh under the two named mechanism mutants (red_under_mutant_r3.log): each makes exactly its own
step WRONG. The wrapper printed grep's status there, so the MISMATCH line is the verdict.
red_after_r3.log: 28 inputs as expected, RED EXIT=0. after_r3.log: source binary 51/51 EXIT=0 (after the
module-doc edit). after_r3_suite.log: whole suite --no-fail-fast EXIT=0, 27 binaries ok (run before the
module-doc edit, which touched a comment in tests/source.rs only). clippy (lint group's command) EXIT=0 and
--test source after the doc edit EXIT=0 (clippy_r3.log); fmt.sh --check 0 (fmt_r3.log).
Engine: git diff --stat -- src empty, so there are no callgrind rows. CLAUDE.md unchanged this round.
What it still cannot see, added: an `impl` on the encryption side of a `type` alias the file defines over
an engine type is counted only through the alias's path (paths out), not as an impl; a trait's blanket
impl on the form side (`impl<T> FormTrait for T`) gives seam types methods (known, harmless kind).
TMPDIR directory removed by its literal path; no redroot, scratch rustc dir removed.
## STATUS (round 3): repairs done, uncommitted; finished copies keep/r3/ (SHA256SUMS there).

## Commit step (round 3)
Gate `sh scripts/check.sh build test lint` -> gate_commit.log, EXIT 101: tests/cipher.rs
a_recycled_log_under_a_cipher_holds_what_was_committed_and_no_more FAILED ("no rotation took the
spare"), a test not in the known-flake list and not touched by the diff (src/ and tests/cipher.rs
unchanged). Alone five times: pass 5/5 (cipher_recycle_alone.log). Not committed per step 3; the
gate stopped at the cipher binary, so the later test binaries and lint did not run in it.

## Repair round 10 (the commit step's gate red at tests/cipher.rs recycled log)
Start 08:51. Working copies == keep/r3 (sums checked). Step 1: the red test reproduced under load
with the gate's own binary (recycle_load.sh -> recycle_load.log). Step 2: whole test group
--no-fail-fast -> after_r10_suite.log; lint -> clippy_r10.log, fmt_r10.log.
Findings:
1. Red outside the unit, tests/cipher.rs a_recycled_log_under_a_cipher_holds_what_was_committed_and_no_more.
   Not the unit's, shown three ways. (a) The binary: target/checked/deps/cipher-b004b6a09b51b303.d
   lists only tests/cipher.rs and tests/common/mod.rs, and libsupdb is built from src/, which the
   unit leaves alone (git diff --stat -- src is empty). No file of the diff is compiled into it, and
   the binary the gate ran is the one this round ran (same hash, cipher-b004b6a09b51b303).
   (b) Reproduced under load with that binary: recycle_load.sh runs the test alone 8 copies at a
   time x 6 rounds, each copy with its own TMPDIR. recycle_load.log: pass 45 fail 3, every failure
   "no rotation took the spare". Two listings match the gate's exactly (spare-00000003 beside
   wal-00000004/5). One shows wal-0..5 all there, no log ever retired to a spare.
   (c) Mechanism, from the code: Db::next_wal takes a spare only if Maint::retire_covered has
   already offered one, and retire_covered runs on the segment work's thread after a seal's durable
   landing and its slot sync. When that thread lags the writer's rotations, every rotation finds no
   spare and creates its file, and the test's 16 rounds make about 5 rotations. That is a timing bet
   in the test (CLAUDE.md, "A test that bet a seal would still be running"); the engine does nothing
   wrong. Its detection was already repaired once for another race (840fd254).
   Not fixed here: tests/cipher.rs is outside the unit's diff. The fix belongs to whoever owns the
   test: the test should hold the state it needs, e.g. settle the segment work (or hold the
   rotation) until a spare has been offered before the commit that rotates, instead of betting on
   the thread. It should be added to the known-flake list until then.
2. The gate's later binaries and lint, run whole this round: after_r10_suite.log is
   `cargo test --profile checked --no-fail-fast`. EXIT 0, 27 binaries ok; the cipher binary passed
   this time, recycled-log test included; source binary 51/51. clippy_r10.log (lint group's
   command) EXIT 0; fmt_r10.log (fmt.sh --check) EXIT 0.
3. The working tree still holds the unit's work uncommitted, as the step requires (M CLAUDE.md,
   ?? .github/CODEOWNERS, tests/data/couplings.txt, tests/lex/, tests/source.rs). It matches
   keep/r3/SHA256SUMS. Nothing changed this round, under src/ or anywhere else, so there are no
   callgrind rows and no new mutants.
TMPDIR directory removed by its literal path.
## STATUS (round 10): nothing to repair in the unit; finished copies keep/r3/. The gate's only red is
## the recycled-log test's timing bet, reproduced at 3/48 under load with the gate's own binary.
## Commit step (round 11): gate running -> commit_gate.log; message commit_msg.txt
## Committed e7d3fe716d218c8e580c17c57c7861cdd445eae6; gate EXIT 0 (all checks passed: build test lint); TMPDIR removed
