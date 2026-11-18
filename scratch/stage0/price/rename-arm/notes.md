# rename-arm -- notes

Base 0c6748bb; units before: other-slot (4a7efafc). Tree clean at start.

## Step
- step 1 (read) done. step 2: tests + seams going in (tests/durable.rs, section "rename-arm" at the end).

## Plan
- seam: Maint::publish's rename branch calls watch_slot before manifest_write (a test's watch of a rename publish);
  fail_next_seal_dir_sync, under the rename shape, fails the publish's directory fsync after its rename
  (manifest_write gains a `fail_dir` arg; the seal's dir fsync IS that publish's under the rename shape).
- (b) fix candidate: land_front stops the seal queue when land_durable fails under the rename shape.
- (c) fix candidate: open, rename shape, a dir fsync before it removes anything.
- (d) fix candidate: open, rename shape, the pending fsyncs (segments, ords under ord_durable, dir).
- device model: directory ops since the last dir fsync that returned reach the device as a prefix in
  program order (what the rename shape rests on: a seal's segment renamed before the manifest naming it,
  no fsync between) -- except (c), which holds the open to not resting on that order for the
  previous life's rename: its removals kept, the rename lost.
- step 2: tests compiled; running before-fix: /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/rename-arm/run-new.sh /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/rename-arm/before-run1.log
- step 2 done: before.log (run3; runs 1-2 had test-model bugs: per-round model, copy names)
- step 3: fix in (land_front stop under rename; open: rename shape fsyncs pending + dir before removals; Db::sync_pending factored). running /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/rename-arm/after-run1.log
- keep/ current (fix + tests). step 4: neighbours via /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/rename-arm/neigh.sh -> n-*.log, neigh.status
- step 4: neighbours running (neigh.sh). n-durable: 203/204, the one failure a test-model race in
  a_slots_store_..._under_a_cipher (tracker read the manifest before the watch was set; a publish between
  them made `before` stale -> "wal sequence gap"). Fixed in the test: holds around the tracker's start in
  ra_rounds and in the slots->rename test. tests/durable.rs edited after keep/ -> keep/ to be refreshed.
- docs: CLAUDE.md edited (crash discipline + amendment to "A path only one arm exercises").
  src/db.rs doc edits pending in docs_edit.py (applied after neighbours, then keep/ refreshed).
- mutants: mutants.json (mkmut.py + edits), runner mut.py -> mut-results.txt
- step 4 done (n-*.log + n-durable2.log green 204; 5 reps of the section green). fmt applied. keep/ refreshed. step 7 mutants: python3 /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/rename-arm/mut.py -> mut-results.txt
- mutants done (mut-results.txt; M13b, M14b with added tests). keep/ refreshed. whole suite -> after.log

## Finished (step 9)
What was wrong (under `manifest_slots` off, the rename shape; each shown failing in before.log):
- (b) followup 18: land_front counted out a seal whose land_durable failed with the queue going. Under the
  rename shape that publish's directory fsync is the seal's names' only one; the next landing's fsync named
  the segments durable over entries the failed one may have lost and retired the log. Power cut -> "the
  manifest slot manifest names seg-...sup, which is missing", plain and cipher.
- (c) followup 32: an open of a manifest a kill left renamed and unsynced swept the old partitions without
  a directory fsync; a power cut that keeps the removals and loses the rename -> refused. Plain and cipher.
- (d) followup 55: a store written under manifest_pending reopened in the rename shape took the pending
  names without fsyncing them; the next flush named them durable and retired the log; a power cut tearing
  the unsynced segment -> "key index checksum mismatch". Plain only; under a cipher the envelope was synced
  before the pending slot (test holds that).
What changed (src/db.rs):
- land_front: on land_durable's Err, under the rename shape, stop_seals before the count-out (cold path;
  the success path's one branch replaces the `out?`).
- Db::open: under the rename shape with a manifest read, before the cipher sweep and every removal:
  Db::sync_pending (pending segments + ords under ord_durable, in the clear) then one vault::sync_dir.
  Db::sync_pending factored out of the slots branch (same calls there).
- Seams: Options::fail_open_dir_sync; fail_next_seal_dir_sync fails the rename publish's dir fsync after
  its rename; watch_slot_writes called between a rename publish's rename and dir fsync (manifest_write
  gained `fail_dir` and `watch` args; Maint::publish's rename branch pays two relaxed loads for them).
Guards (pass before; mutant that shows each can fail in mutants.md): (a) every-step states, publish's dir
fsync by the calls; (e) both directions; (f) refusal words name the file read (followup 58: m.slot is the
file read, "manifest" or "manifest.alt").
Followups closed: 18, 32, 36, 55, 58.
Success-path cost: slots store -- none (land_front: the Err branch only). Rename shape -- open: one
directory fsync, plus the pending fsyncs only where pending names were taken; publish: two relaxed loads
and branches (test seams). Not priced (brief).
Not built / not verified: a store slots->rename->slots (manifest.alt left from a slots life) killed in its
rename life and opened under the slots: the slots open has no directory fsync before its removals when both
slot files exist, so under the (c) model that state would be refused; not built, not fixed (slots path is
out of this unit's allowance). A power cut during the open itself (between a removal and the dir fsync) is
not built; the dir-fsync-fails seam stands in for it. A merge's or promotion's rename publish failing its
dir fsync does not stop the queue (only the seal's land_durable path is changed); the merge job's own dir
fsync made its outputs durable and its inputs are unlinked only after a publish that returned, so no state
was found that needs it; not tested. No real device; states built by hand under the stated prefix model.
- after.log green (20 binaries, durable 208), lint green, fmt green. TMPDIR removed. final.diff saved.

## Repair round 1 (reviewers: 36 not closed -- slots->rename->slots crossing; seam loads on the rename publish path)
- keep/ at start of round 1 = round-0 fixed tree (sha OK). Round-0 copies saved as keep/*.round0.
- R1 step 1: test for slots->rename->slots kill (rename life killed between rename and dir fsync, opened under slots,
  power cut) -> before-r1.log. Fix plan: the rename open, where the other slot file was found (a crossing),
  after its dir fsync and before removals: if it took manifest.alt, writes `manifest` (rename shape, durable)
  first; then removes manifest.alt. Cold: only where manifest.alt exists.
- R1 step 2: seams moved into vault::sync_dir's existing IO_TRACE_ON word (a hook bit), the publish passes a
  constant kind; Maint::publish's rename branch back to its round-0-base shape.
- R1: before-r1.log: the 4 new srs tests FAILED (4 of 8 states each: "the manifest slot manifest names par-..., which is missing").
  Fix in (Db::open rename block: alt_found from the slot-sync loop; after the dir fsync, if alt taken write `manifest`
  via slot.write_shape, then remove manifest.alt). Seams moved to vault::sync_dir_as (IO_SEAMS word, SEAM_HOOK bit,
  hook_dir_sync; Db::hook_publish_dir_sync registers from fail_next_seal_dir_sync / watch_slot_writes under rename).
  Maint::publish rename branch and manifest_write's args back to base. after-r1.log: section 24/24 green.
  keep/ refreshed (now 4 files incl. src_db_vault.rs.fixed). Next: mutants (mut.py reads mutants.json; round-1 set in
  mutants-r1.json -> mut-results-r1.txt), then neighbours, docs, fmt.
- R1 mutants running: mut-r1.py (mutants-r1.json then MUTS=mutants.json) -> mut-results-r1.txt, logs mut-r1-<id>.log; runner restores from keep/ by sum
- R1 survivors so far: X2 (no slots test of a seal publish failing), X3 (harmless), X9 ((d) reopens with pending on), R3 (no kill at the rename open), R5 (harmless), R6 (seam order). Test edits drafted in r1_tests_edit.py, applied after the runner. Plan also: slots open manifest_make_slots before the cipher sweep (crossing only), seam fail_open_dir_sync extended to it.
- R1b: test edits applied (r1_tests_edit.py + crossing fail test), make_slots before cipher sweep + seam; keep/ refreshed; mutants-r1b.json running -> mut-results-r1.txt (ids *-b, M4b, S1-3)
- R1 final: whole suite -> after-r1-all.log, then lint -> lint-r1.log, fmt -> fmt-r1.log

## Round 1 finished
- Blockers: 36 closed for the states built (slots->rename->slots, both slot positions, plain and cipher; and the
  slots open's mid-open window under a cipher, by a seam). Seams moved off the publish path into vault::sync_dir_as's
  existing word; Maint::publish's rename branch and manifest_write are back to the base shape but for the constant
  kind passed to the directory fsync.
- Success-path cost now: slots store -- none (land_front's Err branch; make_slots moved before the cipher sweep is
  the same calls). Rename shape: publish -- none added (sync_dir_as loads the word sync_dir loaded, before the fsync
  instead of after); open -- one directory fsync, the pending fsyncs where pending names were taken, and on a
  crossing only (manifest.alt there) an unlink, plus a manifest write where manifest.alt held the store.
- Followups closed: 18, 32, 36 (crash states across the shapes; its sub-item about a v2 fixture comment in
  a_store_from_before_the_pending_flags_ is not touched), 55, 58.
- Not built: a power cut during the rename open (seam stands in); a direct run's temp name removed by a rename open
  after a kill (X6's question); a merge's or promotion's rename publish failing its dir fsync.
- Tree == keep/ (4 files). final-r1.diff saved. TMPDIR removed.

## Repair round 2 (no blockers; 8 extra mutants)
- keep/ at start of round 2 = round-1 fixed tree (sha OK).
- step 1: mutants-r2.json (A..H, the brief's 8) run on the round-1 tree first: python3 mut-r1.py with MUTS=mutants-r2.json
  -> mut-results-r1.txt lines r2-*, logs mut-r1-r2-*.log. C needs a removal seam: added after the first pass.
- (resumed after the 00:19 restart) tree == keep/ by sha256 (round-1 tree). Results in before the restart: A, B, F SURVIVED;
  E KILLED; D: 7 tests FAILED by name before the binary aborted (ENOMEM spawning a thread: "failed to allocate an
  alternative stack") -- a kill; rerun for a clean log. G was mid-build (dead), H not run, C waits for a seam.
- step R2.2: runner for D (rerun), G, H: MUTS=mutants-r2.json python3 mut-r1.py r2-D-hook-any-kind r2-G-no-seq-bump r2-H-stop-only-dir-fsync
  -> mut-r2-runner2.log (exit line at the end), verdicts appended to mut-results-r1.txt.
- R2.3: seam Options::fail_open_alt_removal + test a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it(+cipher) in tree (NOT yet in keep/); running -> r2-c-test.log
- R2.3 done: new pair green on the tree (r2-c-test.log). keep/ refreshed (sha256.txt). D rerun: killed again by name
  (5 FAILED) and aborted again (thread stack alloc ENOMEM, under the mutant only); G SURVIVED; H KILLED.
- R2.4: mutants-r2b.json (C, Fb = F restated on the seam's text, S4 seam ignored, A-b B-b G-b with the new tests)
  running: MUTS=mutants-r2b.json python3 mut-r1.py -> mut-r2b-runner.log (EXIT line), verdicts in mut-results-r1.txt
- R2.4 done: C, Fb, S4 KILLED by the new pair; A-b SURVIVED; B-b/G-b not applied (text) -> mutants-r2c.json running -> mut-r2c-runner.log
- R2.5: B-b, G-b SURVIVED (argued). Comment on the alt removal + CLAUDE.md clause added, fmt applied, keep/ refreshed. Whole suite -> after-r2-all.log (EXIT line), then lint -> lint-r2.log, fmt -> fmt-r2.log
- R2 finished: after-r2-all.log green (20 binaries, durable 226), lint-r2.log, fmt-r2.log green. Tree == keep/ (4 files).
  final-r2.diff saved. TMPDIR removed. mutants.md has the round-2 table. Not committed (brief).

## Repair round 3 (blocking: a slots open that made manifest.alt and never reached its dir fsync)
- keep/ at start of round 3 = round-2 tree (sha OK; tree == keep/ checked).
- R3.1: tests in (not in keep/): ra_kill_and_reopen(prefail) + a_slots_open_after_a_crossing_open_that_failed_survives_a_power_cut(+_under_a_cipher);
  a_crossing_slots_open_whose_dir_sync_fails now counts the failed open's directory fsyncs (== 1 after the fix: the pre-create one).
  before: run-r3.sh -> before-r3.log
- Fix plan: manifest_make_slots(dir, fail, settle): when a file is missing and `settle` (the open's call; Db::create passes false),
  vault::sync_dir before the first create, the post-create fsync as before (the seam fails that one).
- R3.2: fix in, keep/ refreshed (sha256.txt). mutants: python3 mut-r3.py (mutants-r3.json) -> mut-results-r3.txt, logs mut-r3-<id>.log, runner log mut-r3-runner.log (EXIT line)
- NOTE: a doc edit to Options::fail_open_dir_sync's comment (src/db.rs ~1231) was made while the runner ran; the runner's restore drops it -- reapply after the runner, then refresh keep/.
- (restart) tree had r3-T2 applied; restored from keep/ by sha. runner dead after T1 (T2 log partial).
- doc edit to fail_open_dir_sync reapplied; keep/ refreshed. Rerunning r3 mutants T2..O5 -> mut-results-r3.txt, mut-r3-runner2.log
- r3 mutants done (12): 8 killed; T4,T6,T7,O5 survived (mutants.md). Next: commit step: gate (check.sh build test lint) -> gate.log
- COMMITTED 2e214887 (gate.log EXIT 0: build test lint). Not pushed.
