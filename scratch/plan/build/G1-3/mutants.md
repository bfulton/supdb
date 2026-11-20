# G1-3 mutants

Runner: `mutants.py` (this directory). Each mutant is one edit to the tree, its killer run, the
verdict appended to `mutants.results` as it came, and the file put back from `keep/` and checked
by its sum (`keep/SHA256SUMS`; `sha256sum -c` clean after each pass). Per-mutant output in
`mutant-<name>.log`. Pass 1 `mutants.run.log` (13:41-14:14), pass 2 `mutants2.run.log`
(14:22-14:27). The killer of a script mutant is `scripts/cost-red.sh` with COST_RED_STEPS naming
the step expected to fail (the step's checks are the same whichever steps run); a source-test
mutant's killer is the named test of tests/source.rs.

## The plan's four

| mutant | edit | killed by |
|---|---|---|
| M1 totals | cost-diff.py: the verdict read from the total (`bad = ["TOTAL"] if (th - tb) / ops > slack`) | A: "a hot function's gain under a level total was not red" |
| M2a hot list empty | scripts/cost-hot.txt emptied to a comment | S: the job exited 1 (cost-diff exits 2 on a list with no line) |
| M2b hot list silently empty | cost-diff.py hot_list returns `[]` | A: a1 not red |
| M3 base's own probe sources | cost.sh: the copy of the head's examples into the base taken out | C: the job exited 1 over a probe that changed with the engine untouched |
| M4 exit swallowed | cost.sh: `|| status=1` -> `|| true` | B: the job exited 0 over a read that pays a load and a branch |

## Mine

| mutant | edit | killed by |
|---|---|---|
| O1 | cost-diff.py: the empty-side check never fires | A: a side that collected nothing was not red |
| O2 | cost.sh: COST_HEAD ignored (the head is always the repository) | B: exited 0 |
| O3 | cost.sh: the toggle never passed (whole program collected) | pass 1: SURVIVED (S and C both green: the base runs the head's probe, so both sides collect the same whole program). Step T added: every commit pays a thousand-step loop, which the point read's toggle must not see. Pass 2: T, exited 1 |
| O4 | cost.sh: the RED verdict never taken | B: exited 0 |
| O5 | tests/source.rs: test code counted as entry points | a_new_entry_point_on_no_line_fails |
| O6 | tests/source.rs: a probe the default run skips accepted | a_line_the_tree_or_the_job_does_not_have_fails |
| O7 | tests/source.rs: a pub fn on no line accepted | a_new_entry_point_on_no_line_fails |
| O8 | cost-red.sh: the same-as-HEAD guard taken out | in a scratch worktree of HEAD with src/lib.rs dirtied, with and without the guard: on, "S: the engine or the suite differs from HEAD", exit 1 (o8_guard.log); off, "S ok", exit 0 (o8_mutant.log) |
| O9 | cost-diff.py: a hot line matched as a substring of the canonical name | pass 1: SURVIVED (canonical names had taken away every case a substring and a path prefix disagree on). Fixture a10 added: `read_all` is not `read_all_untimed`. Pass 2: A |
| O10 | cost-diff.py: raw names compared, no canonical sum | A: "a hot function's closure, named as callgrind names it, was not red" |
| O11 | cost-diff.py: `!` lines ignored | A: "a function a ! line takes out was counted" |
| O12 | cost-diff.py: a probe in which no hot line matched passes | A: "a probe in which no hot line names a function was not red" |

17 mutants, 15 killed in pass 1, the two survivors given a step or a fixture and killed in pass 2.

## Not mutated

- scripts/check.sh's `cost` group (`|| exit 1` on each script): no cheap red input reaches the
  group's own exit; it was run once whole instead (see notes.md).
- .github/workflows/ci.yml's job: never run (no CI run from here).
- cost-red.sh's own checks (the grep of a verdict line, the `0 of them moved` check): they are the
  tests; each was seen to fire on its mutant above where a mutant reaches it.

## Repair round 1

Runners: `mutants2.py` (script and test mutants; files restored from `keep2/`, checked by
`keep2/SHA256SUMS`, sums clean after the pass; results `mutants2.results`, logs `mutant2-*.log`,
16:13-17:00) and `emutants.py` (engine mutants: a scratch copy of the tree with the edit, counted by
`cost.sh HEAD <probes>` with COST_HEAD set to the copy; the tree never edited; results
`emutants.results`, logs `emutant-*.log`, 17:01-17:29). Every earlier mutant was run again against
the scripts as they now are (the comparator was rewritten): all killed.

| mutant | edit | killed by |
|---|---|---|
| M1 M2a M2b M3 M4 O1-O7 O9-O12 | as above, re-aimed at the lines as they now read | the same steps (A, S, C, B, T, the source tests); 16 of 16 |
| E1 (reviewers' 1) | install_ahead begins with a pager load and a branch; scan, scan_long | cost.sh RED, `HOT GAINED: supdb::db::Reader::install_ahead +7.00` in both (the toggle's whole engine is hot). Also standing as cost-red step U |
| E7 (reviewers' 7) | install_ahead begins with an 8-step black_box loop; scan | RED, install_ahead +9.00 |
| E2 (reviewers' 2) | MmapBytes::lend_at's body moved into `#[inline(never)] supdb::bytes::cost_mutant_helper` plus three black_box; scan | RED, cost_mutant_helper +30.02 (and scan_whole +4, Reader::scan +1) |
| E8 (reviewers' 8) | a new `#[inline(never)] supdb::db::cost_read_helper` called at the top of read_all; read | RED, cost_read_helper +7.00, supdb_get +2.00. (Called beside read_all_in, not carved out of its closure: the closure is generic and inlined; the property held is the same, a helper on no line) |
| E4 (reviewers' 4) | commit_direct begins with a 100-step black_box loop; load_durable, load_buffered | RED, commit_direct +0.41 a key in both: the default slack is 0.05 a key now (cost-diff's DEFAULT_SLACK) |
| E6 (reviewers' 6) | the head's supdb_get reads v.len() instead of consume(v), and read_all pays a load and a branch; read | exit 2 before any build: "the suite's `fn supdb_get(` differs between the base and the head". Also standing as cost-red step V |
| X3 (reviewers' 3) | tests/source.rs entry_points reads db.rs and blob.rs only | a_new_entry_point_on_no_line_fails (the impl super::Reader in src/db/zfile.rs case) |
| X5 (reviewers' 5 and 11) | check.sh cost: cost.sh's `|| exit 1` -> `|| true` | cost-red G: "the group exited 0 where cost.sh failed" |
| X9 (reviewers' 9) | DEFAULT_SLACK 0.05 -> 5 (the one definition; cost.sh and cost-red pass none) | A: "a tenth of an instruction an operation was not red" |
| X9b | DEFAULT_SLACK -> 0.0 | A: "a hundredth ... was red" |
| X10 (reviewers' 10) | `--threshold=100` dropped from callgrind_annotate | S: the job exited 1, `PARTIAL: the base side's functions sum to 178,380,323 of its 179,981,283` |
| R1 | cost.sh passes no OWN for toggled probes | U: exited 0 |
| R2 | the sum-to-total check never fires | A a17 |
| R3 | the no-total check never fires | A a18 |
| R4 | the suite guard skipped | V: exited 0 (the job built and counted) |
| R5 | the "(over the engine)" key dropped | A a16 |
| R6 | owned functions not hot | A a13 |
| R7 | check.sh cost: cost-red's `|| exit 1` -> `|| true` | G: "the group ran cost.sh after cost-red.sh failed" |
| R8 | the suite guard ignores an item's attributes | V's second copy (consume's #[inline] -> #[inline(never)]) |

35 mutants this round (16 again, 11 of the reviewers' as 10 edits plus X9b, 8 mine), 35 killed, no
survivor.

## Repair round 2

Runners: `emutants2.py` (engine mutants: a scratch copy with the edit, `cost.sh HEAD <probes>` with
COST_HEAD set to it, the tree never edited; each mutant's listings kept under `em2/<name>/out` and
the round-1 comparator `keep2/scripts__cost-diff.py` run over the same counts, so one count gives the
verdict before the repair and after; results `emutants2.results`, logs `emutant2-*.log`),
`mutants3.py` (script and test mutants on the lines this round wrote; files restored from `keep3/`
and checked by sum after each, all restored; results `mutants3.results`, logs `mutant3-*.log`), and
the two source-test mutants and the bad-ref run by hand in the tree, db.rs restored and checked by
its sum.

| mutant (reviewers' list) | edit | round-1 gate | now |
|---|---|---|---|
| E12 (1, 6) per piece | `std::hint::black_box(0u32)` before each of the three `self.own_row.push(self.own_crc)` (once a 16 KiB key-section piece); seal | green (exit 0) | RED, `AlignedWriter::write_all +1,022 over the run` (0.010 an op) |
| E9 (1) per piece, the reviewers' shape | `if black_box(&self.section).is_some() { black_box(()) }` at the same three sites; seal merge | red already: the branch changed write_hashed's loop, +3.7 an op seal, +5.1 merge | RED, the same |
| E10 (2) libc length | ZFence::fence_word copies `min(16)` bytes into a 16-byte array where it copied `min(8)` into 8; read_z | red (entry_of +0.78) | RED, entry_of +155,337 over the run. memcpy did not move: no tail is past 8 bytes in the probe's keys, and the array's build is the call site's. The engine's raw point read and scan make no memcpy of their own (callgrind's caller tree: the read's one caller is the suite's sink; the scan's are once a scan), so no pure libc mutant was found on them; the libc rule (past 2 an op and 5% of its own count, under a toggle) is held by fixture a21, and a libc gain under both is invisible, said in cost-diff's docs |
| E11 (3) no_mangle | lend_at's body in `#[no_mangle] extern "C" fn cost_mutant_helper` + three black_box; scan | red (callers +4/+5: LTO inlined it) | RED, the same |
| E11b (mine) the same, `#[inline(never)]` | as E11, kept out of line; scan | red (callers +1 each) | RED, `cost_mutant_helper +30.015 an op` (the bare name is hot under a toggle) |
| X11 (4) trait entry point | `pub trait ReadFast` and `impl ReadFast for Reader` appended to src/db.rs | green (inferred: the round-1 scan counted `pub fn` words only, and a trait method has none; not run) | every_entry_point_is_counted...: `Reader::ReadFast::get_fast is on no line` |
| X12 (5) macro entry point | `macro_rules! gen` before `impl Reader`, `gen!(cost_red_entry);` inside it | green (inferred, as X11) | every_entry_point...: `Reader::gen! is on no line` |
| K (7) per-key helper | a new `#[inline(never)] fn cost_red_step(i)` called a key in commit_direct's stream loop; load_durable | green (fixture newfn in before-round2.log; the step itself is new) | cost-red step K, RED, in after.cost3.log |
| E14 (8) per commit | a 13-step black_box loop at the top of Db::sync_run (unlisted, once a due commit); load_durable | green (exit 0) | RED, `Db::sync_run +1,400 over the run` (0.014 an op) |
| E15 (9) merge per block | `if self.r % 64 == 0 { black_box(()) }` in Emitter::leave; merge | red (the test is a key's: +3.98 an op) | RED, the same. A step that costs one instruction a block and nothing a key has no site in the merge's own loop; E12's site is the merge's writer's too, at a 16 KiB piece, which at the merge's 200,000 keys is under the hundredth and is not seen there -- the seal's toggle sees it (the limit is stated in the hot list's header and CLAUDE.md) |
| X13 (10) bad ref | `cost.sh no-such-ref merge seal` | exit 128, git's `fatal: invalid reference: no-such-ref` (mutant3-X13_before.log) | exit 2, `cost: BASE 'no-such-ref' names no commit of this repository; nothing built`; standing in cost-red step G |

Found by the mutants and fixed: E15's run named `Readers::oldest_for` +400,023 and `WriterOp::drop`
+75,000 in the merge probe, which E9's run read -387,780 and -75,003: the sweep's look at the table
and the writer's op ends while it waits, timing on both sides; with Maint::collect and sweep_spares
at a hundredth either way. `~` lines for them (WriterOp::drop for the merge alone, the scoped form
`~merge: path` added for it), and a fixture for the scope (a28).

| mutant (mine, round 2) | edit | killed by |
|---|---|---|
| N1 | TOGGLE_SLACK 64 -> 200 | A: a hundred instructions over the run under a toggle not red |
| N2 | WHOLE_SLACK 0.01 -> 0.02 | A: an instruction every sixty-four operations not red |
| N3 | the bare-name clause of whole mode dropped | A a24 |
| N4 | glibc's `./` files read as source | A a14 (libc's noise red) |
| N5 | the libc rule's `and` -> `or` | A a14 |
| N6 | libc never hot | A a21 |
| N7 | `~` lines applied under a toggle | A a25 |
| N8 | a scoped `~` line applied to every probe | A a28/a25 |
| N9 | an unknown mode accepted | A a27 |
| N10 | impl_trait never found | a_new_entry_point_on_no_line_fails |
| N11 | macro detection off | a_new_entry_point_on_no_line_fails |
| N12 | cost.sh's rev-parse check removed | G (the worktree failure exits 2 with another message) |
| N13 | a function absent at the base not hot | A a26 |

13 of mine, 11 of the reviewers' as 11 runs (E9, E10, E11, E11b, E12, E14, E15, X11, X12, K, X13): all
red where they must be. Of the reviewers' predictions, E12 and E14 were survivors of the round-1 gate
and are red now; E9, E10, E11 and E15 were red under the round-1 gate already, by the call sites
they changed.

## Round 3

Mine, on the lines this round wrote (mutants4.py, results mutants4.results, logs mutant4-*.log; each file put
back from keep4/ and checked by its sum after each):

| mutant | edit | killed by |
|---|---|---|
| P1 | a `~` line's bound ignored (the mode's slack holds) | A: a25 under its bound read red |
| P2 | the smaller of two bounds | A: of two ~ lines the smaller held |
| P3 | a scoped `~` line in every probe | A: a ~ line naming another probe bounded this one |
| P4 | an unscoped `~` line under a toggle too | A: under a toggle a ~ line's function gained, not red |
| P5 | a `~` line with no bound skipped, not refused | A: not refused |
| P6 | a scoped `~` line in whole mode alone | A: a ~ line naming a toggled probe did not bound it there |
| Q1 | aliases ignored | a_new_entry_point_on_no_line_fails |
| Q2/Q2b | blanket impls ignored | a_new_entry_point_on_no_line_fails |
| Q3 | an alias chain resolved one step | a_new_entry_point_on_no_line_fails (R3 = R2 = Db) |
| Q4 | the alias map keeps aliases of any type | SURVIVED: the filter was a second copy of the entry-type check below it; taken out |
| Q5 | the blanket rule for inherent impls too | SURVIVED: Rust has no inherent impl for a type parameter, so `tr.is_some()` was a check no input reaches; taken out |

The reviewers' source mutants, as edits of src/db.rs with the test of the tree (mutant4-X14.log, X15.log):

| mutant | under keep3 (round 2) | now |
|---|---|---|
| X14 (3) blanket `impl<T: Deref<Target = Reader>> ReadFast for T` | green (r3/before-source.log) | RED: `*::ReadFast::get_fast is on no line` |
| X15 (4) `pub type R = Reader; impl R { pub fn cost_red_entry }` | green (same run) | RED: `Reader::cost_red_entry is on no line` |

The rule found two in the tree: `impl<A: Aead + ?Sized> Aead for &A` (src/crypt.rs), listed as
`*::Aead::encrypt`/`decrypt` with the reason.

The reviewers' engine mutants (emutants3.py, results emutants3.results, logs emutant3-*.log, listings em3/;
each a scratch copy counted by cost.sh against HEAD; "round 2" is keep3's comparator and list over the same
listings, for the probes it had):

| mutant | probes | round 2 | now |
|---|---|---|---|
| F1 (1, 6) per-key loop at the top of Maint::land_readable | merge, load_durable, load_buffered, work, land | green in all (`~Maint::`) | RED in merge, both loads (+4 an op, past the hundredth) and land (+400,088); work's round lands nothing it counts |
| F2 (2) an 8-step loop at the top of sweep_retired_states | merge, work, load_durable, land | green | RED in land (+216 over the run, 27 calls under the toggle); the merge's bound of 35 an op lets its 50,000 walks through, which is what land is for |
| F5 (5) a load and a branch in the form walk past 1,024 entries | scan_long_z, scan_z | scan_z red by its layout (+61) | RED, scan_long_z walk_impl +1,157 a scan; scan_z +61 as before (the edit moved the loop's code) |
| F6 (6) a loop a 64-key block of every landed segment in publish_segs_with | merge, load_durable, land | green in merge/loads (`~Maint::`) | RED in all three |
| F7 (7) an inline(always) helper a key from bound_pieces_of | merge, work, land | green | RED in land, in Maint::rank_before_publish (+262,588); merge and work land far from reads and take no bounds |
| F8 (8) black_box(()) a 16 KiB piece in pwritev_all | load_durable, load_buffered, commit_* | red in both loads already (+3,653, 0.037 an op: the sum over the parts a call and the loop, past the hundredth; the reviewers predicted 800) | RED in all four, +3,653, the commit probes' 64 over the run too |
| F8b the same with black_box(i) | same | red in both loads (+5,261) | RED in all four, +5,261 |
| F9 (9) a 300-step loop once a store at the top of Db::open | work, merge | merge red (+2,428), work green | the same: the merge probe opens two stores (2,428 > its 2,000); work opens none through Db::open. A fixed cost once a store is not a key, entry, block or step; CLAUDE.md now gives the slack as numbers |
| F10 (10) 10 steps at `at == 0` in write_buf_with | seal | green | green: never ran (a segment's first write is not at 0 through write_buf_with; +16 over the run, layout) |
| F10b 10 steps at the top of SegmentWriter::finish | seal | green | green: 11 instructions a segment, +44 over four, under the 16 a segment four segments give the toggle's 64 |
| F10c 40 steps at the top of SegmentWriter::finish | seal | (red too over these listings: four segments) | RED, SegmentWriter::finish +164 (41 a segment); with the round-2 seal of one segment it is 41 < 64 |

F10b is the blind spot as a number: under a toggle a function is held to 64 instructions over the run,
and the seal probe's four segments make that 16 a segment.
