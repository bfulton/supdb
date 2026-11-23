# G1 as built: the completeness critic (read-only, tree at 31b33339, clean)

Most important first. File:line is of this tree.

## Headline
- The default gate is green and the known cipher flake did not fire.
- Three gates cannot give a verdict on a pull request into main:
  - cost reads red by function against any base;
  - lists holds nothing against main;
  - cross-arm is red in 2 of 5 parallel runs (QEMU SIGSEGV, not a test).
- The seal's source test does hold what it says it holds. But it trips on ordinary code (`lent` inside any macro, and lines keyed by their exact text).
  - Recommendation: rename the seal's words to unique names (option a).
- `OrdIndex::whole_key_at` +10.9 an op in the merge probe is probably a real per-key cost. It is a cold arm that returns into a hot path. Not verified by a listing.

## Holds (rule -> gate -> red input -> what it cannot see)

### R0, the wall
- Gate: tests/source.rs `the_forms_and_the_cipher_do_not_name_each_other` against tests/data/couplings.txt. Held across a change by the lists group.
- Red inputs: the sibling tests (`a_coupling_added_to_a_form_fails_as_growth`, `a_vault_call_spelled_as_a_path_fails`, ...).
- Cannot see:
  - a cipher branch written in words the token list lacks. Example: zfile's `SegSrc::Kept` / `StrideNo::Kept` decline is a mode branch, and the wall cannot see it as one.
  - one coupling swapped for another at the same count.
  - anything on a PR into main, where couplings.txt is "new since the base".

### R1, the door
- Gate: scripts/door/run.sh (clippy disallowed_*) in lint, plus `the_door_holds_what_clippy_cannot` and the door-libc test.
- Red input: scripts/door/red.sh (11 families in a copy of zfile.rs).
- Cannot see:
  - threads, parks, joins, waits and sleeps, which are only counted by file (decision 11, owed to S1);
  - tests, examples and bench;
  - non-Linux code, and config paths that were resolved on x86_64 only;
  - a barred call added inside a function that already carries an item allow, e.g. `ask_file`'s allow covers a `File::open` added beside it (G1-6 open item).
- The libc test reads the registry source. A vendored or offline build fails it outright.

### R2, whole borrows
- Gate, by the compiler:
  - `Bytes::lend_at(.., blob::lend::Token)` (src/bytes.rs:112);
  - `MmapBytes::lent<T: whole::Reader>` (src/bytes.rs:468);
  - `Paging::base(Base)`;
  - `Blob::records()` (src/blob.rs:1666);
  - `flat` and `pieces` are private (blob.rs:1166, 1196).
- Red inputs:
  - the compile-fail doctests in src/lib.rs;
  - scripts/seal-red.sh (9 home lines compile, 20 fail abroad, 14 s);
  - seal-red-red.sh (43 s, about 100 MB at peak, all given back);
  - tests/source.rs `seal_check` with `a_seals_token_made_another_way_fails` and `..._spelled_another_way_fails`.
- Cannot see:
  1. A whole-range `slice_at(0, len)`, which compiles. G2's conform pass is meant to catch it at run time.
  2. A new walk inside blob.rs over `self.flat()`, `Sec::get` or `flat_lent`. Only callers of `lend_at` are listed (tests/source.rs:6200); callers of the wrappers are not. Such a walk passes every gate and silently answers None or absent under a cipher. That is A6 exactly.
     - The same hole exists in zfile.rs: a new caller of `ZFile::stride_bytes`.
     - forms.md's "As built" lists "a new caller inside a reader's own module" as unseen. The stage's running account said the source test holds it. Both are half right: callers of lend_at and lent are held by scope; callers of the wrappers are not.
  3. The ordered index's CRC skip keys on `piecewise`, not `authenticated`.
  4. The stride form's four readers lend nothing under a cipher, so the form is silently declined.

### R3, advice is a method of the source
- Gate: G1-6 moved every site to `Bytes::resident_share`, `resident_at`, `advise_willneed` and `advise_dontneed_range`, and to the crate-private `Paging::{willneed,forget,resident}`. No advice site takes an address.
- Held by:
  - tests/cipher.rs `a_merge_under_a_cipher_asks_ahead_for_its_cold_inputs`;
  - tests/zpages.rs `a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing` and `builds_ahead_at_landing_read_the_records_residency_and_not_the_directorys`;
  - tests/blob.rs `the_advice_by_rank_answers_the_same_for_a_source_by_the_piece` (G1-4).
- Cannot see:
  - advice parity, which is G2-9;
  - descriptor advice (S1);
  - the `willneed_ranks`, `willneed_dir` and `record_offset` arms called a key or a block at a time: no cost probe reaches them;
  - the plain build's drop. It is held by scratch/plan/build/G1-6/strace/gate.sh, which is not in the tree and runs nowhere.

### Cost job (section 5, R14)
- Gate: `sh scripts/check.sh cost` (outside the default set; a CI job at .github/workflows/ci.yml:115-135). It runs cost-red.sh, then cost.sh over 15 probes with cost-diff.py and cost-hot.txt.
- Entry points: `every_entry_point_is_counted_by_a_probe_or_says_why_not`, red by `a_new_entry_point_on_no_line_fails`.
- Cannot see:
  - a move between functions: the gate reads it red and it is paired by hand;
  - a fixed cost under the slack;
  - a row missing inside a probe;
  - the G1-4 arms above;
  - `Txn`, `RunWalk` and `SparseBlob`.
- Against main (CI's PR base) it counts weeks of upstream work, so its exit status is meaningless there.

## Acceptance (forms.md section 1)
- **Z, not yet.**
  - The wall stops forms from naming the encryption modules.
  - A2 still breaks (4080-byte chunk against ZPAGE), and so do A3 (vault `Kind`) and A7 (the controller is off: `cipher_controller`, db.rs:17349).
  - The stride form and the heads' copy need work behind the pager.
  - Who holds it: S2-S3 (pages and classes), S5 and G3 (controller and Memory), S6 (log and manifest).
- **N, not yet.**
  - Nothing runs a plain test under the cipher's paths. That is G2a (conform tier 1 and the fuzz).
  - Until then, a change is shown right under a cipher only by tests/cipher.rs and the cipher fuzz.
- **K, partly.**
  - The wall, the door by symbol and the seal take away the need to know "which call is safe".
  - Rests, holds, `unlent` and `check` (A5) are still the author's to know. That is S1/R4-R6.
- **F, partly.**
  - Held: A6 for `lend_at`/`lent` callers by the compiler; A9 by the methods.
  - Not held: whole-range `slice_at`; a new caller of `flat`, `Sec::get` or `stride_bytes`; A5; A10 (options not drawn).
  - Who holds it: G2 conform on the paths tests reach, R13 (G2-10), S1.

## Concerns
1. **Cost gate.** scripts/check.sh:178-190 exits red by function before and after any change. A new regression reads like the reconciled list. In CI, `SUPDB_PR_BASE`, which is main, adds upstream's whole engine delta. The cost group cannot fail usefully on the first PR (task 20).
2. **Lists hold nothing on a pull request.**
   - scripts/ratchet.py: "new since the base", and since 2a0d750a a declared raise for a new list is "not held yet". Against main, which is the PR's base, all four lists hold nothing.
   - The rule "only goes down" is held only on pushes.
   - 2a0d750a does what its message says, but it widens that hole: a declaration's from/to are checked against nothing until a base has the list.
3. **cross-arm is red in 2 of 5 parallel runs.** It is QEMU 8.2.2 SIGSEGV, not a test failure. It was green 5 of 5 at 19d078d1 with 519 tests and has 530 now. CI will flake. The group was not changed.
4. **Seal source test fragility** (tests/source.rs:6338 `SEAL_MACRO_WORDS`, 6241 `SEAL_DECLS`, 6580 check):
   - any `lent` or `lend` word inside any macro's tokens is red. The engine has an upkeep field `to.lent` (db.rs:46080, 46574) and a local `lend` (db.rs:46604), so `debug_assert!(to.lent <= n)` in new engine code is red;
   - exceptions match the squashed line text and scope, so rustfmt rewrapping or renaming a test function makes them red again;
   - a `SEAL_DECLS` line wrapped by rustfmt (e.g. bytes.rs's `fn lend_at(&self, off: u64, len: usize, _: LendToken) -> Option<&[u8]> {`) splits, and its `LendToken` line reads as "names a seal's token outside the call";
   - any `.base(x)` or `::base(x)` call with arguments anywhere in src is read as the seal's `base` (tests/source.rs `seal_names`). Today only `codec.base()` exists.
   - The error message names the list to add to. It does not say the key is exact text that must be re-made after formatting.
5. **`OrdIndex::whole_key_at`** (src/ordindex.rs:650-653) reads `self.top()`, an inline check with a cold `top_late`, and `self.head(rank)`, whose cold `head_absent` returns. Upstream read the field `self.top` and asserted.
   - This is the "cold path that returns into a hot path" shape, per key: +10.9 an op in the merge probe.
   - It predates M1 (it is in T17's table at the same figure). It is probably a real plain cost.
   - `Db::append` +4, `seal_table::{closure#4}` +5 and `Emitter::enter` +3 are undecomposed. T17's notes say "their sum is under the merge's other losses", which is not a pairing.
6. **Seal probe** `std::fs::OpenOptions::_open` 944 against M1's 804 (+140, over the 64 slack). Unexplained. The longer COST_DIR path is a guess and not verified. `Maint::join_seal` +0.029 against a bound of 0.010 in land, past its own `~` bound in both M1's count and this one.
7. **README.md:320-329**, "Every option a store in the clear runs with, a store under a cipher runs with", is true of acceptance and false of effect:
   - `heads_owned` and the stride options act on nothing;
   - `merge_parallel_quiet` only rises at GATE_Z (db.rs:53045-53057), which the controller never reaches under a cipher by default. The quiet third merge is not mentioned in README.md, ROADMAP.md or the crate docs.
8. **No test of today's behaviour** under a cipher for the stride decline (`zfile::StrideNo::Kept`, zfile.rs:6972) or for the missing heads' copy. G1-8 said so, and it is owed to S5. The owner's rule (nothing switched off as a design property) is kept only in prose, in ROADMAP.md:559-575.
9. **Red inputs kept out of the tree**, so nothing runs them again:
   - G1-6's strace gate;
   - G1-8's docs_red.py (16 doc-to-code checks);
   - G1-4's g14_gate.rs and g14_drop.rs (three cost-only mutants held by no gate).
10. **`compact_run` takes `record_offset(0)` of every input of every merge** (db.rs). Under a cipher that is a directory word read through the pager per input, then thrown away. G1-4 left it open.
11. **The ordered index's CRC skip keys on `piecewise`**, not on `Bytes::authenticated`. It is right only because its one piece source is `vault::map`'s paged mapping.
12. **Departures from the plan.**
    - Tokens instead of `pub(in ..)` on trait methods: a strong reason, since a trait method's visibility cannot be restricted.
    - The thread half of the door deferred: decision 11.
    - The strace gate left in scratch: a weak reason. It is a plain-store behaviour gate with an exit status, and it could have been an ignored test plus a check.sh group.
13. **Not found wrong** where read:
    - M1's `seek_in_exact` unsafe (ordindex.rs:1053-1065): `a <= m < b <= hi <= held.len()` holds. `heads_held` gives `lim` heads, where `lim` is 0 or n. Its SAFETY comment says "as long as the caller's pin stands", but the slice's lifetime is `&self`. It is used only inside two searches, so this is fine.
    - `ModeNote` as Cells: Reader is Send and !Sync, so this is sound.
    - 067d9fa8 (`ss_commit_answer`, tests/durable.rs:6058): it loops on empty commits with a 120 s bound. It is stronger than before (every later commit must be Ok), and a failure answered nowhere fails at the bound.
    - The M1 bench test of the cipher arms.
    - Not read line by line: the last repairs of G1-1, G1-2, G1-3, T18, T17 and G1-5.

## For G2 (g2.md must change)
- **Names that moved.**
  - The advice methods are `Bytes::resident_share`, `resident_at`, `advise_willneed` and `advise_dontneed_range`, plus the crate-private `Paging::{willneed,forget,resident}`. G2-9's `track_caller` targets these.
  - `MmapBytes::as_slice` is gone; use `touch` or `lent` with a token.
  - `record_regions` and `record_pieces` are now `Blob::records()`.
  - `flat` and `pieces` are private.
  - Line numbers: `check_options` db.rs:17362, `cipher_controller` 17349, `page_pin` 15768, `end_slow` 25963, `leave_slow` 25982, `Paged::ensure` pager.rs:1333, `nudge` 486, `map_shared` 1550, `guard_on` 1591/1597, `slice_slow` bytes.rs:523, `piece_at` 164/623, `vault::key_id` vault.rs:400.
- **G2-1 is still needed.** `check_options` does not refuse `writer_pins` off under `Cipher`.
- **G2-9's red input** becomes reverting G1-4's `*_by_piece` arms. The plan's "return 0" is the old shape.
- **G2-10:** Options has 357 pub fields, not 220. Recount the draws and the never-named list.
- **New lists:** conform-skips.txt, fuzz-exempt.txt and asks.txt must each be registered in scripts/ratchet.py. An unregistered text file under tests/data is red.
- **New public items** need lines in cost-entries:
  - `Db::partition_fences` and `Db::form_fence_keys`, which do not exist yet. tests/common/mod.rs:131's `partition_fences` reads the directory.
  - Each new reason is a declared raise in scripts/raises.txt, as M1's two were.
- **The conform group's CI job** is only `sh scripts/check.sh conform`, with the event's values. The workflow tests also need it listed (a job or `CI_NOT_RUN`).
- **Wall:** `vault::resolve` called from db.rs, and a `conform` API naming the cipher, raise db.rs's couplings. Prefer a shape inside vault's open, or declare the raise with a reason.
- **Under conform these do nothing,** so their tests fail or pass vacuously:
  - the stride form declines (`StrideNo::Kept`);
  - `heads_owned` does nothing.
  - G2-3 must either list them as mode seams, which breaks the owner's rule, or schedule the ROADMAP's stride-by-piece and heads-by-piece fixes before G2-4's first green.
- **`Bytes::authenticated` is true for a paged mapping,** so under conform the key-section row is never verified. `verify_pieces_in` stays reached only by tests/blob.rs's sources.
- **Upstream's new mechanisms enter the conform run unpriced under a cipher:** bounds by heads, `land_bounds_read`, the quiet merge (skipped by `cipher_controller` unless conform skips it), and the fence-stepping cursor. The cipher flake (task 14) will show there too.
- **The seal test** turns red on any new `assert!` naming `lent` or `lend` inside a macro. G2's tests in src (pager, vault) will meet it.
- **Every count of tests** (639, 581, 267 opt-outs) is from 682b2c09 and must be redone.
- **cross-arm** gets heavier with each test G2 adds.

## Owner items
1. **The seal's source test.**
   - (a) Rename `lend_at`, `lent` and `mod lend` to words nothing else uses (e.g. `lend_whole` / `seal_lend`), drop `SEAL_MACRO_WORDS` and read the word anywhere. Cost: one rename and about 200 lines fewer. Ordinary code stops tripping it.
   - (b) Cut the text half back to the construction sites and the callers' list. Cost: it no longer holds a deliberate hand-out of a token, which the compiler and seal-red.sh still mostly bar.
   - Status quo: it costs every author who writes `to.lent` in an assert, or reformats a listed line.
   - Recommend (a).
2. **The cost gate.**
   - (i) A baseline file of accepted moves that cost-diff.py reads, grouped by probe and held to each group's net, shaped like raises.txt and valid against one named base. Cost: a file to maintain each merge, and moves stay visible.
   - (ii) A CI base of the last integrated upstream rather than main. Cost: one more definition of "the base".
   - Either way, decompose `whole_key_at` and the three small gainers first, rather than baselining them.
3. **The lists on a PR into main hold nothing.** Accept that until integration, or land the lists on main first. A list landed on main then holds every later change.
4. **cross-arm flake.** Choose: `--test-threads=1` (639 s against 181 s), a newer QEMU, or compiling the heaviest tests out under `supdb_emulated`.
5. **Stride form, heads' copy, quiet merge under a cipher.** Choose: fix by the piece before G2's conform run (the owner's rule, and the conform run needs it), or accept them as listed mode seams until S5. The second is a switch-off in effect.
6. **The door's libc test reads cargo's registry.** Keep it (fails offline or vendored) or snapshot the class list in tree.
7. **Bring the strace gate and docs_red.py into the tree** as groups outside the default set, or accept that they hold nothing after G1.
