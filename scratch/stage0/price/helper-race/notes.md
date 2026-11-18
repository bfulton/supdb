# helper-race (pricing unit)

Base: HEAD 6f2e8dd3 (run base 0c6748bb + other-slot, rename-arm, due-log commits). Tree clean at start.

## Step
- [x] 1 read (ROADMAP item, run-errors notes, seam/work/owed/syncop notes+check)
- [ ] 2 tests + seams, before.log
- [ ] 3 fix (shape 1: helper opens its own description via /proc/self/fd/N)
- [ ] 4 neighbours, after.log
- [ ] 5 pricing
- [ ] 6 measuring field out
- [ ] 7 mutants
- [ ] 8 docs, fmt
- [ ] 9 finish

## Plan
Seam: SegmentOptions::helper_fault: Option<Arc<HelperFault>> (doc hidden). Armed: helper's fdatasync
stands in failed; records a dup of the helper's description (the one the error was reported to);
hold: helper waits between its syscall and its store. Writer side: a fault bit (0x10) set at create
when the seam is set routes every sync to sync_attended, which asks the seam whether the error
reaches the writer's description (kcmp KCMP_FILE against the recorded dup: a different description
gets the error once, as errseq does). sync_at_stop: armed helper takes its due sync even after stop
(so seal/merge tests are not a bet on timing). refuse_reopen: the helper's reopen fails -> no helper.

## Progress log
- Step 2 done: tests in tests/durable.rs (section helper-race), seam HelperFault in src/db.rs. before.log: part A the
  untouched tree (writer-level race + no-/proc fallback fail; store-level failed for a seam reason: Db's run open
  assigned the fault byte, overwriting RUN_FAULT_WB -> now `|=`), part B the old shape (measuring field forced on):
  both store-level tests fail at "the policy's sync ... is not answered Ok". Seal/merge tests pass before (held by mutants).
- Step 3 done: fix = AlignedWriter::helper_description -> reopen() via /proc/self/fd/N (stack path, O_RDONLY|O_CLOEXEC);
  open failure -> no helper (helper_done). Measuring field Options::price_helper_race_old -> SegmentOptions -> try_clone.
  keep/*.measuring = the measuring tree (with field). .fixed copies made after the field is out.
- Kernel check (errseq/probe2.c, probe2.out): ext4 on a loop over a full tmpfs, the space freed between the syncs:
  helper dup fails ENOSPC -> writer's fdatasync Ok (hole); helper own description fails -> writer's fdatasync ENOSPC.
  Twice each. Kernel 6.18.44. Torn down.
- Step 4: neigh.sh running -> n-<t>.log, neigh.status.
- Step 4 done: neighbours all 0; after.log whole suite --no-fail-fast exit 0 (20 binaries ok). Step 5: pairs.sh list1.txt -> pairs/, status pairs/status.txt

## Pricing (step 5) -- verdict: no cost resolved, lands
Arms: supdb-helperraceold / supdb-ingesthelperraceold = supdb / supdb-ingest with Options::price_helper_race_old
(bench-arms.patch; Policy differs in `price` only, read). Bench build: bench-build.log exit 0.
Ratios below are new/old for rates (>1 new faster); sitting 2 tables have the old arm as column A.
- pair.sh supdb supdb-helperraceold 10000 --passes load,lag (m): 0 marked either sitting. load s1 0.99 s2 0.97;
  shuffled 1.02 / 0.92; lag weighted_ms lagall s1 old/new 1.011, s2 new/old 1.047; all unmarked, split.
- pair.sh supdb-ingest supdb-ingesthelperraceold 10000 (m): 0 marked. load 1.04 / 0.99; shuffled 0.98 / 1.03.
- pair.sh supdb supdb-helperraceold 300000 (m): s1 marks count upkeep_ms (new 370 vs old 336 ms, p .039), s2 the
  lag100 updates rate new faster 1.019 (p .039). Rerun (r2): 0 marked either sitting; upkeep_ms new +2.6% both,
  unmarked -> unresolved. load s1 1.04 s2 0.92 (m), 1.02/0.96 (r2); shuffled 1.02/0.98, 1.03/0.98.
- pair.sh supdb-ingest supdb-ingesthelperraceold 300000 (m): s2 marks load-shuffled new faster 1.034 (p .039);
  s1 none. load 1.00 / 1.03; shuffled 1.04 / 0.97(s2 marked new faster).
- Stamped (st; SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1, --passes load), medians of 13 (phases.py, drain.py):
  300k supdb ordered closing sync new 30.91 vs old 29.22 (s1), 28.39 vs 28.80 (s2): split; drain answer 29.55 vs
  27.75 (s1), 27.19 vs 27.39 (s2): split; seal fsync after readable 4.57/4.64, 4.35/4.68.
  300k ingest ordered sync 32.56/31.85, 30.45/31.85: split; drain 31.59/30.69, 29.53/31.08: split.
  shuffled drains: supdb 227.1/237.2, 243.0/238.5; ingest 101.9/106.0, 99.8/102.0. finishes' helper join
  (finishsteps helper-written) 29/40, 28/21 us ordered; 272/322, 301/305 shuffled: level.
  10k: no helper exists below sync_ahead (8 MiB): the change cannot act; phases level (drain 1854/1814, 1733/1817).
- strace -c / -tt -T -y over load_only 300000 1 (strace/): the helper's F_DUPFD_CLOEXEC is replaced by one
  openat("/proc/self/fd/N", O_RDONLY|O_CLOEXEC) = 29 us under strace (old dup 20-29 us); ingest's helper made
  4 fdatasyncs on its own fd5 (writer fd4) vs 4 on the dup fd4 (writer fd5), 6.6-7.9 ms each in both; the
  sync_file_range hints are the writer's own (34 + 6 on the writer's fd) in both arms, unchanged -- the helper
  issues no hints. supdb (durable) spawns one helper that makes no sync (commits sync every batch).
- Page cache (folios/, folio_probe example + folios.py: pagemap + kpageflags over every file of the store):
  ordered and shuffled 300k loads on all four arms: every file's folio composition identical new vs old.
- Step 6 done: measuring field + arms out (bench-arms.patch reversed, folio_probe example deleted); after-landing.log 11/11 helper tests. keep/*.fixed = landing tree (pre-docs). Step 7 mutants next.
- Step 7: mut.py running -> mut-results.txt, mut-<name>.log; restores src/db.rs from keep/src_db.rs.fixed by sum
- RESTART 06:05; resumed. Sums of src/db.rs, src/lib.rs, tests/durable.rs checked = keep/sha256.txt. M3's earlier run was cut (log has no verdict).
  Running mut.py M3..M9 -> mut-results.txt (appended), log mut-runner2.log. Docs (CLAUDE.md, ROADMAP.md) not yet kept: next after mutants.
- Mutants M1-M9 all KILLED (mutants.md). Docs edited (src/db.rs comments, engine.md); fmt applied. Running final.sh -> final.log
- keep/ refreshed with final tree incl. docs (spawn takes HelperSeams for clippy's too_many_arguments). Running whole suite -> final-suite.log

## Finish (step 9)
What was wrong: WriterHelper synced through try_clone() of the writer's File -- one open file description. A
writeback error is reported once per description, so a helper fdatasync that met it consumed it, and a writer
fdatasync returning in the instant before the helper stored its word in HelperShared::error read synced_all clean:
a run commit acknowledged over the hole.
Changed: AlignedWriter::helper_description -> reopen(): open("/proc/self/fd/N", O_RDONLY|O_CLOEXEC) with the path
on the stack, once where the helper is spawned; a failed open leaves the writer with no helper (helper_done), the
writer syncs and hashes alone. Seam HelperFault (SegmentOptions::helper_fault, #[doc(hidden)] pub, free when unset):
fails the helper's sync by description (records a dup of the description it failed on), can hold the helper between
its syscall and its store, answers the writer's sync the error where kcmp says its description is another; refuses
the reopen. WriterHelper::spawn takes Option<HelperSeams> (clippy too_many_arguments). Db's run open ORs its fault
byte into the writer's (it assigned it, which dropped the seam's bit).
Tests (tests/durable.rs, section helper-race): run commit trace "1 B B Y" plain and under a cipher; the writer-level
marked sync; no-/proc fallback; a seal's output's helper sync failed -> no live segment, no manifest names/covers,
flush+settle answer EIO, queue wedged, reopen replays; a merge's piece's helper sync failed -> nothing lands, commit
answers EIO, partition and pieces stand, reopen reads the model. The seal/merge tests passed before (join_helper was
already right) and are held by mutants M2/M3, not counted as failing-first evidence.
Pricing: see "Pricing" above -- no cost resolved; the one marked cell (upkeep_ms, 300k durable, s1) rerun and split.
Mutants: mutants.md, 9 of 9 killed.
Left out: a real device error under the store (dm-flakey, ROADMAP item); shape (2) not built (shape (1) did not cost).
Not verified: behaviour on a kernel other than 6.18.44 / filesystems other than ext4 for the two-description errseq
report (probe2 on ext4 only); non-Linux reopen falls back to try_clone (the old shape) and is not tested; final
whole-suite run -> final-suite.log.
- final-suite.log: exit 0, whole suite green on the landing tree.

## Repair round 1 (no BLOCKING; ten extra mutants, eight distinct)
Start: tree sums = keep/sha256.txt (checked). Plan: E1 O_WRONLY, E2 no CLOEXEC, E3 fd widths (M7 rerun), E4
same_description inverted, E5 sync_at_stop false (several runs), E6 told_other dropped, E7 fallback after own_on.
New tests first (CLOEXEC/description/readable at fd widths 2,3,4; no-/proc unmarked variant), then round-1 runner
mut2.py -> mut2-results.txt, mut2-<name>.log, restores src/db.rs from keep/src_db.rs.fixed by sum.
- round1: new tests pass on fixed tree (r1-new.log, 12/12). mut2.py running E1-E7 (E5 x3) -> mut2-results.txt, mut2-runner.log
- round1 b: E5 (x3), E6, E7 survived. E6: told_other taken out (unreachable: lose() sets RUN_ATTN_LOST, sync_attended answers held before reaches). E5: seam start_after_stop + test a_finish_answers_a_helper_sync_taken_at_the_stop. E7: lib test a_writer_with_no_helper_keeps_its_own_row. keep/ refreshed. Rerun E4,E5,E7 -> mut3-results.txt
- 2nd edit (spawn::<SEAMED>) collided with the running mut3 runner (E5 baked in); runner killed, E5 hunk reverted by hand, diff vs keep checked = only the spawn edit. mut3 E4 result valid (KILLED); E5/E7 to rerun.
- keep refreshed after spawn::<SEAMED> (r1-new3.log 6+13 ok). mut4: E5 x3, E7, E8, E9 -> mut4-results.txt
- mut4: E5 x3 KILLED (rep0 also the seal test: without sync_at_stop the seal test bets on timing), E7 E8 E9 KILLED. Next: re-pricing (spawn restructured on the writer's path): m-apply.py + bench-arms.patch -> bench build -> pairs-r1/ via list-r1.txt
- pricing r1: pairs-r1.sh list-r1.txt -> pairs-r1/ (status.txt)
- r1 tables read: no cell worse both sittings; stamped: durable ordered fsync-after-readable new +0.27/+0.21 ms both sittings (round 0 had new shorter both) -> rerun st2
- 07:2x resumed after restart 3. st2 read (phases.py, drain.py): durable ordered fsync-after-readable new 4335 vs old 4546 us (s1, new shorter), 4276 vs 4153 (s2, new longer): split. st1+st2 together: not longer in both sittings of both stamped pairings -> unresolved, not a resolved cost. drained (answer) ordered 28223/28213, 28659/27726: split. That phase is the seal thread's fsync of the segment on the writer's own description after the readable landing; the change (one openat at the helper's spawn, durable arm's helper makes no sync) reaches it by nothing on that path. m1 tables: marked cells (ingest 300k merge_plans s1, prime us_per_page s1, merge_head_inputs s2; supdb 300k bytes_on_disk s1) none in both sittings. Verdict r1: no cost resolved; lands.
- Next: 13g take field+arms out (keep/src_db.rs.fixed over src/db.rs, git checkout HEAD -- bench/src/engines.rs), save measuring-r1.patch first.
- 13g done: measuring field+arms out (measuring-r1.patch saved), src sums = keep. CLAUDE.md differs from keep (round-1 paragraph): finishing it next.
- CLAUDE.md round-1 paragraph finished; keep/CLAUDE.md.fixed + sums refreshed. Next: unit tests + neighbours on the fixed tree -> r1-final-*.log
- r1final.sh running (fmt --check, whole suite --no-fail-fast, clippy) -> r1final.status, r1final-*.log
- mutants.md round-1 section written.
- r1final: fmt --check differs (src/db.rs start(), tests/durable.rs asserts) -- apply after the suite finishes, refresh keep, rerun helper tests
- fmt applied + clippy fix (is_none_or in width test); keep refreshed. Running r1final2.sh (clippy, durable, lib) -> r1final2.status
- r1final2: clippy 0, durable+segwriter+lib 0 (405/251/19); whole suite earlier (r1final-suite.log) exit 0 before fmt-only/clippy-only edits. Unit TMPDIR removed. Round 1 done.

## Repair round 2 (no BLOCKING; eight extra mutants R1-R8)
Start: tree sums = keep/sha256.txt (checked, 6 files). Step: read the code for each mutant, then runner mut5.py -> mut5-results.txt.
- mut5.py (R1-R7; R8 = R3) first pass on the round-1 tree -> mut5-results.txt, mut5-runner.log. Do not edit tests/ or src/ while it runs.
- mut5 first pass: R3 R5 R7 KILLED; R1 R2 R4 R6 SURVIVED (expected). Adding tests (noseam, O_RDONLY, arm after create, lib unarmed).
- r2 tests pass on fixed tree (r2-new.log 7+14). keep/ refreshed (src_db.rs = lib test added; tests_durable.rs). mut6 = mut5.py RES=mut6-results.txt all R1-R7 on the final tree
- mut6 (final tree): R1-R7 all KILLED. Running r2final.sh (fmt --check, whole suite, clippy) -> r2final.status, r2final-*.log
- r2final: fmt --check 0, whole suite --no-fail-fast 0 (20 binaries ok), clippy -D warnings 0. Tree sums = keep/sha256.txt. mutants.md round-2 section written. Unit TMPDIR removed. Round 2 done.
  Round 2 changed tests only (tests/durable.rs: new a_helper_with_no_seam_syncs_on_a_description_of_its_own, width test's
  accmode assertion == O_RDONLY, writer-level race test armed before and after create; src/db.rs streaming_writes:
  an_unarmed_seams_helper_counts_no_failure). No engine code changed and no reviewer item said a pairing missed the
  mechanism, so no pairing rerun; measuring field and arms stay out (measuring.patch, measuring-r1.patch kept).

## Repair round 3 (no BLOCKING; seven extra mutants -> N1..N7b)
Start: tree sums = keep/sha256.txt (checked, 6 files). First pass mut7.py on the round-2 tree -> mut7-results.txt,
mut7-<name>.log, log mut7-runner.log; restores src/db.rs from keep/src_db.rs.fixed by sum. Do not edit src/ while it runs.
- mut7 first pass: N1 N5a N5b KILLED; N2 N3 N4a N4b N7a N7b SURVIVED; N6 compile error (mutant fixed), rerun first pass TMO=600 -> mut7b-runner.log. Planned test edits in r3-tests.py (not applied yet).
- r3 test edits applied (r3-tests.py; fmt), r3-new.log 7+16 ok on fixed tree; keep/tests_durable.rs.fixed refreshed. Final pass: RES=mut8-results.txt mut7.py all (logs mut7-<name>.log overwritten)
- mut8 final pass: all KILLED but N7a N7b (equivalent, argued). CLAUDE.md amended (fallback stride), keep refreshed. Running r3final.sh -> r3final.status
- r3final: fmt 0, whole suite --no-fail-fast 0, clippy -D warnings 0. Sums = keep. Unit TMPDIR removed. mutants.md round-3 written. Round 3 done.
