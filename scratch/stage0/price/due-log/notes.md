# due-log (pricing unit, followup 5)

Base HEAD 639cb0de (0c6748bb + other-slot, rename-arm). Tree clean at start.
run-close did NOT land (price/run-close/not-landed.patch): there is no run-close flag on this tree to unify with.

## Defect
commit_direct's due branch syncs the run's file only; log frames of a table frozen before the run
(EveryN), loose files, or frames an open replayed (Always after a reopen) stay unsynced: a power cut keeps
the run's commits, loses the older log frames.

## Shape
Direct.log_owed: bool, noted in commit_direct's run-open branch (cold) as
wal.frames_unsynced() || !loose_wals.is_empty() || wal_dir_unsynced, only when the run is not logged
(RUN_LOGGED: under a cipher log_run's due sync_wal covers it). In the due branch: `if d.log_owed` ->
cold fn that asks the log again, sync_wal()s if it still owes, clears the bool, then syncs the run and
does the commit's tail.

## Step
1 reading: done.
2 tests: done. before.log (fix reverted with git apply -R fix-engine.patch, then reapplied): 4 of 6 fail
  - frames: "the due commit kept 20 of the run's 20 keys and lost 150 of the 150 keys committed to the log before the run" (first arm default)
  - loose: "kept 30 of 30 ... lost 379 of 379"
  - reopen (Always after reopen): "kept 10 of 10 ... lost 100 of the 200" (batch b)
  - fail-order: "the due commit answered Ok over a failed log sync"
  - clean (owes nothing, no log sync) and cipher (log synced once behind its frames): pass before -- holding tests, not evidence.
3 fix: done (src/db.rs). after-unit.log: 6/6 pass. keep/ = fixed tree incl. price_due_log_old field (default false).
4 neighbours: neigh.sh -> n-<t>.log, neigh.status (durable 232/232 ok so far).
5 measuring: bench arms supdb-duelogold, supdb-ingestduelogold (Policy.price -> Options::price_due_log_old). Measuring-only eprintln in the cold path under SUPDB_DUELOG_TRACE (to see if a cell reaches it).
4 after.log: whole suite --no-fail-fast exit 0, 20 test results ok.
5a bench built (bench-build.log exit 0, release, examples). strace -f -c (strace/<ex>-<arm>.c) load_only and lag_only 300000 1 for all four arms:
   load_only supdb: fdatasync 303 pwritev 335 pwrite64 17 fsync 6 sfr 42 = duelogold identical; ingest fdatasync 7 = ingestduelogold identical.
   lag_only (shuffled load + lag): supdb fdatasync 627 pwrite64 265 / old 628 266; ingest 29/265 both. "duelog: due" lines: 0 in all eight -> cold path never reached.
5b pairings: pairs.sh list1.txt -> pairs/, status in pairs/status.txt
5c pairings m (list1), r2 (list2), r3 (list3: ingest 10k and 300k again, for ycsb-B/C at 10k and lag10 weighted at 300k marked in one sitting of r2). Tables pairs/*-{m,r2,r3,st}-SIZE-{1,2}.out. "duelog: due" lines: 0 in every .err (cold path never reached by any cell).
   stamped (st, SUPDB_LOAD_PHASES+DRAIN_STEPS): phases.py medians, ordered load commits phase: durable 300k 206.35 new vs 224.22 old (s1), 209.53 vs 196.15 (s2) -> split; 10k 7.08/6.71, 8.27/8.19 -> +0.08..0.37 in both, sittings differ ~1.2 -> not resolved. buffered 300k 38.23/37.99, 38.34/38.86; 10k 2.02/1.88, 1.92/1.94.
5 verdict: no cost resolved (see pricing table at end). 13g done: measuring.patch saved (whole measuring diff); bench-arms.patch reverted, price_due_log_old + SUPDB_DUELOG_TRACE eprintln removed.
6 landing tree: wal_dir_unsynced clause dropped from the open note and cold recheck (redundant: dir owed => loose file listed, as Db::sync argues); clean test got an "everyn-synced-after" case. after-landing.log 6/6. keep/ updated = LANDING tree (src_db.rs, tests_durable.rs, CLAUDE.md, ROADMAP.md).
7 mutants: mut.py -> mut-results.txt, mut-<id>.log

## Final (step 9)
What was wrong: commit_direct's due branch synced the run's file only. Log frames that a sync by policy
promises (a hashed table frozen before the run under EveryN, a loose file from a rotation, frames an open
replayed under Always) stayed unsynced, so a power cut kept the run's commits and lost older log commits.

What changed (src/db.rs): Direct.log_owed, noted in the run-open branch (cold, once a run) as
`after & RUN_LOGGED == 0 && (wal.frames_unsynced() || !loose_wals.is_empty())` (the directory clause left
out as Db::sync leaves it: a dir barrier owed implies a loose file listed). Due branch: `if d.log_owed
{ return self.run_due_log_first(); }` (a load and a branch inside the due branch). Cold path asks the log
again (a sync since the open may have paid it), sync_wal()s if owed, clears the note, syncs the run, zeroes
unsynced. The refs conversion of direct_table_refs is left to the run's next commit on that one commit
(mutant round 1 showed nothing observable; kept the hot path's text unchanged).
run-close did not land, so there is no second flag to unify. They would be one state ("what is older than
the open run and unsynced": the log's frames, or a closed run's file), noted at the open and paid at the
first due sync; if run-close lands, its closed-run owed list folds into the same note.

Tests (tests/durable.rs, section due-log): frames (all run arms), loose, reopen-Always (all run arms),
fail-order, clean (incl. synced-after), cipher (holding). See before.log / after-landing.log.

Pricing (A=new, B=old, B/A as printed; sittings 1/2):
- strace -c load_only 300000 1: supdb fdatasync 303 pwritev 335 = old; ingest fdatasync 7 = old.
  lag_only 300000 1: supdb 627/628 fdatasync (old one more), ingest 29 = 29. Cold path lines: 0 everywhere.
- pair.sh supdb supdb-duelogold 10000 --passes load,lag,mixes: s1 5 marks all lag (new faster: weighted lagall
  1.153 old/new), s2 1 mark lag10 entries (new slower 0.573); rerun r2: s1 load 0.855 (old slower), lag0 0.886
  (old slower); s2 none. Mixes: m s1 old faster 8-25% unmarked, s2 new slower 3-17% unmarked; r2 both
  sittings new faster 5-28% unmarked. -> no cost resolved (split across sittings and pairings).
- pair.sh supdb supdb-duelogold 300000: no rate marked in either sitting; level.
- pair.sh supdb-ingest supdb-ingestduelogold 10000: m s2 marks lag0 entries 0.889, lag1 updates 0.963 (new
  slower), s1 none; r2 s2 ycsb-B 0.764 C 0.876 (new slower), s1 B 1.011 C 0.925 unmarked; r3 none in
  either sitting -> unresolved.
- 300000: m s1 ycsb-B 1.040 (new slower), s2 0.989 unmarked; r2 s2 lag10 weighted 1.065 / updates 0.901 (new
  slower), s1 0.982 unmarked; r3 s1 lag1 weighted 1.009 (new faster), s2 none -> unresolved.
- stamped load phases (st, SUPDB_LOAD_PHASES/DRAIN_STEPS): ordered commits phase 300k durable 206.35 new
  vs 224.22 / 209.53 vs 196.15 (split); 10k 7.08/6.71 and 8.27/8.19 (new +0.08-0.37 ms both, sittings
  differ ~1.2 ms -> not resolved); buffered 300k 38.23/37.99, 38.34/38.86; 10k 2.02/1.88, 1.92/1.94.
Verdict: no cell of the suite reaches the cold path (0 trace lines in every sitting and probe); every cell
level or unresolved -> lands.

Not verified: callgrind count against the base; the priced tree (measuring.patch) had the refs tail in a
helper (inline(always)) and the dir clause in the note, both removed afterwards (hot path's added work
is the same load and branch); no power-cut test on a real device; Kernel encryption mode not tested
(runs under it are the plain path on an encrypting filesystem).

## Repair round 1 (reviewers: helper fdatasync / writeback reach the same non-prefix state)
Choice: (a) -- name the residue truthfully (docs, ROADMAP item narrowed back), no success-path code change,
so no re-pricing. (b) only narrows (writeback still breaks it) and moves the helper's flushes onto the close;
(c) is a syscall at run open the brief did not price. Test: holds the state with a small sync_ahead and a
test accessor for what the helper's fdatasyncs covered (Db::run_device_synced_bytes, doc(hidden), free).
Step: R1 writing accessor + test.
R1 step: accessor + test written (r1-helper.log 17/17 a_runs_*, 20/20 reps). keep/ updated (round-0 keep in keep-r0/).
Mutants round 2: mut2.py -> mut-results.r2.txt (restores src/db.rs and tests/durable.rs by sum after each).
R1: docs narrowed (SyncPolicy headline + ordered-run paragraph, Direct::log_owed, CLAUDE.md ordered-ingest + Shapes amendment, ROADMAP item back, narrowed). New test 2 (unopened loose file) kills E6. fmt + clippy ok. keep/ = R1 tree.
R1 step: neighbours running: neigh.sh -> n-r1-<t>.log, neigh-r1.status. No re-pricing: nothing on a success path changed (a test accessor and docs).
R1 done: neighbours durable 234/234, db 226, cipher 24, fuzz 5, lib 404 (neigh-r1.status all 0); after-r1.log; final-r1.diff = tree.
Mutants r2: E1 E2 E3(window probe, fails as meant) E5 E6(killed after unopened test) E7 E8 E9 E10 killed; M10 argued as before. See mutants.md.

## Repair round 2 (reviewers: run_again syncs the run ahead of an owed log; close's presync/job fsync not named)
Start: tree == keep/ (sums checked). keep-r1/ = copy of keep at round-2 start.
R2 step: run_again tests appended to tests/durable.rs (a_runs_owed_batch_written_again_*); running before-r2.log
R2: run_again fix in src/db.rs (log first when log_owed); keep/ updated (fixed src+tests, docs not yet). after-r2-unit.log 13/13
R2: close holding test added (a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames, r2-close.log passes = window open); docs updated (SyncPolicy, log_owed, run_again, Db::sync comment, CLAUDE.md x2, ROADMAP); keep/ = R2 tree. Next: mutants (mut3.py -> mut-results.r3.txt)
R2: holding tests for mutants added (again_over_a_log_a_sync_paid, cipher loose); keep/ updated; mut3.py running
R2: mutants r3 done (mut-results.r3.txt), O2 survives (argued as M10). Next: fmt, clippy, neighbours (neigh.sh r2)
R2 blocking items: (1)+(3) run_again: fixed (cold, no success path changed: run_again runs only where a run write failed) -> no re-pricing.
  (2) close's presync / seal job fsync: named in docs (SyncPolicy headline + ordered-run para, Direct::log_owed, CLAUDE.md x2,
  ROADMAP item), held by the close test (window open). Not ordered behind the log: that adds a log sync on the close path (unpriced).
R2 neighbours: neigh-r2.sh -> n-r2-<t>.log, neigh-r2.status.
R2 done: neighbours all 0 (durable 240, db 226, cipher 24, fuzz 5, lib 404), fmt+clippy ok, close/again tests 20/20 reps; final-r2.diff = tree; TMPDIR removed

## Repair round 3 (reviewers: non-kept sync_wal error in run_due_log_first leaves batch not owed; prefix sentence vs a run closed into a seal in flight)
Start: tree == keep/ checked; keep-r2/ = copy of keep at round-3 start.
R3 step: tests X1 (always unopened loose -> owes batch), run_again unopened loose, closed-run window (holding) added; before-r3.log: X1 fails 'a commit of nothing answered Ok under Always ... Some(0)', other two pass before (holding). Fix: run_due_log_first marks the writer owed (SegmentWriter::owe_unsynced) on a sync_wal error. after-r3-unit.log 17/17. keep/ = R3 src+tests (docs pending). Next: docs, then mutants (mut4.py -> mut-results.r4.txt).
R3 step: docs done (SyncPolicy headline/ordered-run para/closed-run sentence, Direct::log_owed, run_due_log_first, run_again, CLAUDE.md x2, ROADMAP item); keep/ = R3 tree incl docs. mut4.py running -> mut-results.r4.txt
R3 step: mutants done (mut-results.r4/r4b/r4c), fmt+clippy ok (fmt-r3.log lint-r3.log), keep/ = R3 tree formatted. neighbours: neigh-r3.sh -> n-r3-<t>.log, neigh-r3.status
R3 blocking items:
 (1) non-kept sync_wal error in run_due_log_first (loose open / chain): fixed -- the batch is marked owed by the run's writer
     (SegmentWriter::owe_unsynced, cold, inside `if let Err(e) = self.sync_wal()`), so a call with nothing new goes
     run_attended -> run_again, which syncs the log first (note still set) then the run. run_again needed nothing: its batch
     is owed by the write failure, its note survives a failed sync_wal (`?` before the clear). Docs of run_due_log_first and
     run_again say which errors are kept. X1 test fails before (before-r3.log), passes after.
 (2) prefix sentence vs a run closed into a seal in flight: docs qualified ("every commit of the log before it", headline and
     closed-run sentence "a later commit, to the log or of a later run, survives"), Direct::log_owed says it is of the log
     alone and that the two would be one state; CLAUDE.md x2; ROADMAP item widened. Holding test added (window open).
 No re-pricing: the only code change is on a sync_wal error path inside a #[cold] fn (no success path changed), and no
 reviewer said a pairing misses the mechanism (the earlier rounds' finding stands: no cell reaches the cold path).
R3 done: neighbours durable 243, db 226, cipher 24, fuzz 5, lib 404 all ok (neigh-r3.status); fmt+clippy ok; after-r3.log;
 final-r3.diff = tree; keep/ = tree (sums match); TMPDIR removed.
step: commit gate running -> price/due-log/gate.log
COMMITTED 6f2e8dd3f5077aba8338c074855cdc29798d605a
