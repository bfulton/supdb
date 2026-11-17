# sync-run (sync-ordered-buffered; run-tail-kill's second test): notes

Status: DONE, uncommitted. Base HEAD cdb5296b (delete-max, log-errors, run-errors, log-at-open committed).
Uncommitted: src/db.rs, tests/durable.rs, CLAUDE.md, ROADMAP.md. Also wrote stage0/fix/deferred.md.

## Fixed copy
keep/{db.rs,durable.rs,CLAUDE.md,ROADMAP.md}, sums in keep/SHA256SUMS, keep/worktree.patch = git diff vs HEAD.
The working tree matched keep/ byte for byte at the end (cmp). db.rs sha256 b23a1ac1...

## What was wrong
Db::sync: (1) `if !self.mem().ordered && (log owes)` -- the log was never synced while the live table was a
direct run's ordered table, though a run opens over a log that may hold unsynced frames of tables frozen before
it (their seal not yet durable), and under a cipher a run's commits are the log's frames (Db::log_run);
(2) nothing synced the open run: commit_staged -> commit_direct returns at "nothing new", and a run's commit
that is not due under EveryN writes only whole 1 MiB pieces (the tail held in the table's arena on the stream
path, the AlignedWriter buffer on the copy paths), so a sync() answered Ok lost to a process kill every key of a
run under a piece, and to a power cut everything since the last due sync; (3) `unsynced = 0` forgot the debt.
Under adaptive_shape the sync then handed the run to a seal (hand_tail) whose finish writes the tail on the
seal thread, so a kill after the Ok lost it too.

## What changed (src/db.rs)
- Db::sync: `run_owed = self.unsynced != 0 && self.direct.is_some()` read first (sync_wal zeroes the count);
  the log synced whenever it owes (frames, loose files, the directory) whatever the live table; then, if
  run_owed, `Db::sync_run` (cold, inline(never)): `d.w.sync()` -- the call a due commit makes, which writes
  batch_buf, the held stream tail and the buffer, fdatasyncs, and reads the helper's error -- with errors
  through run_failed (a lost sync kept, Db::lost; a write failure answered and owed, run_again next time).
  All before hand_tail, so the handed run's commits are on the file before the hand-off.
- Why the counter as the gate: by reading it is exact for "the open run has commits no sync covered" (raised by
  every run commit, lowered only by syncs that covered the run: commit_direct's due branch, run_again,
  sync_wal on log_run's due path after the run's sync, Db::sync, flush before its seal closes the run); it is
  one load (zero under Always after every commit, zero after a due commit), where the writer's own predicate is
  four loads. Caveat, not tested: under EveryN(u32::MAX) the u32 count wraps after 2^32 commits in release
  (pre-existing; the policy's own due test has the same wrap).
- Order: log first, run second -- the log's frames predate every commit of the open run, so a crash inside a
  sync (or a failed log sync) leaves a prefix.
- Docs: SyncPolicy (the run under EveryN: whole pieces written, the rest held; what a sync does), EveryN's
  variant doc (was "always at seal, flush and close"; seal() does not sync the log under the default
  seal_rotates_wal=false -- its records are durable once its segment lands), Db::commit (the fdatasync where the
  policy says; the held tail), Db::sync (log whatever the table; the run written and synced).
  CLAUDE.md: amendment to "A fast path gated on the case it was written for" (came back as a skip of a promise),
  and the "Ordered ingest has its own log" paragraph of the crash discipline (the fdatasync where due; sync).
  ROADMAP: "A run's due commit beside log frames no sync covered" (not built, below). README: nothing false.

## Success-path cost (rule 1), by reading the diff (not counted under callgrind, no timing run)
- commit, flush, drain, seal, rotation, manifest publish, reads: untouched.
- Db::sync: one load of `unsynced` and a branch (plus `direct.is_some()` only when nonzero); for an ordered live
  table the log gate's three loads that a hashed table's sync already paid (the `ordered` load removed).
  Syscalls only when something is unsynced: the run's write + fdatasync when the run owes, the log's
  fdatasync when the log owes -- Db::sync's own contract. Under Always with a run open: no new syscall
  (held by a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it).
- Price owed (no benchmark allowed here): the bench's nodrain arms call Db::sync at the end of a load
  (bench/src/engines.rs ~8924); under a buffered policy that sync now writes and fdatasyncs the run.

## Tests (tests/durable.rs, section "sync-run")
before.log: the first five, each failing at its contract assertion (before-summary.log). The later ones'
before state is mutant M0 (the fix reverted), m-M0-fix-reverted.log.
| test | before | holds |
|---|---|---|
| a_sync_puts_an_open_runs_commits_where_a_process_kill_keeps_them | every arm x EveryN(5)/EveryN(MAX) x shape: all keys of small runs lost, 2000/12000 (every5) and 4000/12000 (buffered) of large | R4, O1, O3 |
| a_sync_makes_an_open_runs_commits_survive_a_power_cut | same matrix; buffered large 12000/12000 | R1, R4, O1, O3 |
| a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut | 200/200 log keys and 300/300 run keys | R1, R2, O1, O2 |
| a_sync_that_hands_an_open_run_to_a_seal_has_put_its_commits_on_the_file | 300/300, 4000/12000 (default, copyvals) | R3, R5 |
| a_sync_makes_an_open_runs_logged_commits_survive_a_power_cut_under_a_cipher | every shape, both policies | R2 |
| a_sync_whose_run_fdatasync_fails_answers_it_and_keeps_it | via M0 | O4, O5 |
| a_sync_whose_write_of_the_runs_tail_fails_is_written_by_the_next_sync | via M0 | O4, R1 |
| a_sync_with_a_run_open_syncs_the_log_before_the_run | via M0 | O6, R2 |
| a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it | passes before: not evidence | O9 |
| a_sync_starts_the_policys_count_over_for_an_open_run | passes before: not evidence | O8 |
Shapes (RunHeld): One (one commit), Small (3x100, nothing on the file), Large (12x1000, pieces on the file,
tail held), Staged (100 staged, committed by the sync), Twice (sync, commits, sync). Arms: run_arms() (default,
copyvals, threecalls, refvals, norecycle, keysarena, keycopies, fatentries).
Skeptic's problems: B (power cut) kept beside A (R1 passes A); C asserts its premises (run open, seal in
flight); D asserts the hand-off; the log cut is to its synced length (power_cut), not kept at 8 bytes;
direct_run_synced (helper's max) not added -- run_synced_bytes (the writer's own) is the conservative cut;
temp names not hard-coded (run_tmp_name/run_ino). The directory barrier of the run's temp name is not held
(deferred, run-temp-dir-barrier).

## Mutants: mutants.md (16 run, all killed; report's 5 is the fix's gate, explained; report's 6 deferred).

## Restated tests: none. Nothing existing went red: whole suite --no-fail-fast green (suite.log: db 226, durable
111, lib 404, cipher 24, fuzz 5, zpages 136, ...), clippy --release --all-targets -D warnings exit 0, fmt --check 0.
tests/db.rs's crash windows are in its 226.

## Not built (reported)
- The due commit of a run beside unsynced log frames (deferred.md; reproduced on this tree, due-commit.log, test
  in due-commit-test.diff): EveryN(5), three log commits, Db::seal held before its fsyncs, two run commits -> the
  fifth is due and syncs the run only; a power cut keeps 4-5 and loses 1-3. A different site (commit_direct's due
  branch) and a commit-path syscall (rule 1; the brief: a commit under EveryN is untouched). Smallest form: a bool
  noted at the run's open (cold, once a run) when the log owes, asked in the due branch beside the run's
  fdatasync, its cold path syncing the log first. ROADMAP item.
- Db::seal under EveryN: by reading, a different contract, not this gate: with seal_rotates_wal off (default) the
  seal commits what is staged under the policy and does not sync the log; Db::seal's doc promises durability only
  at the durable landing (segment fsync + manifest), and the log keeps the frames until then. EveryN's variant doc
  said "always at seal"; restated to what is true. The prefix it can break is the due-commit item above.

## Encryption
- None/Kernel: the plain path. Cipher: the run's commits are log frames; the log gate change syncs them (tested,
  both policies, every shape). Db::sync_run also writes and syncs the run's memory file (harmless; its durability
  is the log's). Cipher-only analogues (vault::persist, memory files, pager), by reading: nothing in Db::sync
  depends on them; a seal's or a run's memory file is covered by the log until its envelope and manifest.

## Not verified
- No timing or benchmark (rule 10): the closing sync of the nodrain/buffered arms now pays the run's fdatasync.
- Not counted under callgrind.
- The fuzz does not vary the sync policy (no ZFUZZ knob for it); it ran green but does not exercise EveryN.
- A real power cut / dm-flakey: the power cuts are cuts of copies to the synced lengths the engine reports.
- The directory entry of the run's temp file (deferred unit).

## Repair round 1 (started)
Step: reading. Fixed copy of round 0 = keep/ (sha256 matches working tree at start of round 1).
Round-1 copy will be keep1/ once built. Blocking: sync after a run closed (lower key / Db::seal) into a seal in flight.
Step: closed-run test appended TEMPORARILY to tests/durable.rs (restore from keep/durable.rs if found); running -> round1/closed-before.log
Step: closed-run test confirmed failing (round1/closed-before.log, 16/16 cases); durable.rs restored from keep. Decision: report (b), not build a wait in sync (rule 1).
Step: docs restated in src/db.rs (SyncPolicy, EveryN, Db::sync); two characterization tests appended to durable.rs (flush after closed run; cipher sync after closed run). Running -> round1/char.log
Step: added RunHeld::SyncedThenStaged, helper-sync test, count-across-close test; running new tests -> round1/new.log
Fixed copy for round-1 mutants: keep1/db.rs (docs only changed vs keep/), keep1/durable.rs. Runner: round1/mut1.py, verdicts appended to round1/mutants1.jsonl as each comes.
Step: round-1 mutants running in background (round1/mut1.py -> round1/mutants1.jsonl, round1/mut1-run.log). If restarted: check src/db.rs sha against keep1/SHA256SUMS, cp keep1/db.rs back if not, rerun mut1.py (it skips done ones).
Step: round-1 mutants done (mutants1.jsonl; X3, X9b survived, equivalent by reading). Next: fmt, suite -> round1/suite.log
Step: suite running (round1/suite.sh -> round1/suite.log, round1/suite.status). keep1/ is the fixed copy (post-fmt sums in keep1/SHA256SUMS).

## Repair round 1: what was done
- Blocking (both reviewers, one item): a sync after a run closed (lower key / Db::seal) into a seal in flight
  answers Ok over the run's unsynced commits. Reproduced: round1/closed-run-test.rs, failing 16/16 (closed-before.log).
  NOT BUILT (rule 1: the fix needs a wait on the seal thread inside sync, or P1's write on the close path). Reported:
  deferred.md (new item, the test and its state), ROADMAP.md (new item with both smallest forms), docs restated
  (SyncPolicy paragraph, EveryN variant, Db::sync's Ok) so none promises it; CLAUDE.md: the Ordered-ingest crash
  paragraph says an open run's only and a flush covers a closed one; the shape entry amended.
- New tests in durable.rs: RunHeld::SyncedThenStaged (kills X1); a_sync_over_a_run_whose_helper_sync_failed_answers_it_and_keeps_it
  (fails under M0 = before the fix; kills X2); a_runs_commits_count_toward_the_policys_sync_after_it_closes (characterization, kills
  X6/X6b); a_flush_after_a_run_closed_into_a_seal_makes_the_runs_commits_durable and
  a_sync_after_a_run_closed_into_a_seal_covers_its_logged_commits_under_a_cipher (characterization of the restated docs; pass before,
  not evidence).
- Code: unchanged from round 0 (src/db.rs differs from keep/db.rs by doc comments only).
- Mutants: mutants.md "Repair round 1". X3 and X9b survive, equivalent (argued there).
- Round-1 suite (round1/suite.log, copied to after-round1.log): clippy -D warnings 0, cargo test --profile checked
  --no-fail-fast exit 0 (db 226, durable 115, lib 404, cipher 24, fuzz 5, zpages 136, ...). fmt --check 0, scripts/fmt.sh 0.
  Working tree == keep1/ (cmp). TMPDIR removed. Status: round 1 DONE, uncommitted.

## Repair round 2 (started)
Step: reading. Working tree == keep1/ at start (sha256 matched). Round-2 work dir: round2/. Fixed copy for
round 2 will be keep2/ once built.
Step: extended closed-run test written to round2/closed-run-test.rs; appending TEMPORARILY to tests/durable.rs (restore from keep1/durable.rs); run -> round2/closed-before.log
Step: closed-run extended test FAILS on tree, 24/24 cases (all six closes x 2 shapes x 2 arms), closing writes survive; round2/closed-before.log. durable.rs restored from keep1 (sum checked). Next: docs.
Step: docs done (src/db.rs SyncPolicy + Db::sync, CLAUDE.md, ROADMAP.md, deferred.md). Next: fmt check, build keep2, mutants (round2/mut2.py -> round2/mutants2.jsonl).
Step: added a_sync_with_only_the_run_owing_syncs_nothing_of_the_log (for extra mutant E4); running -> round2/new.log
Fixed copy for round 2: keep2/ (db.rs = keep1 + doc comments; durable.rs = keep1 + a_sync_with_only_the_run_owing_syncs_nothing_of_the_log). Sums keep2/SHA256SUMS.
Step: round-2 mutants running (round2/mut2.py -> round2/mutants2.jsonl per verdict, round2/mut2-run.log). If restarted: check src/db.rs against keep2/SHA256SUMS, cp keep2/db.rs back if not, rerun mut2.py (skips done).
Step: E8 survived; added a_run_commit_after_a_failed_due_write_is_due; running it on fixed tree -> round2/new2.log, then E8 again (mut2.py E8-due-zeroed-before-sync) after keep2 refresh
Step: fmt ok; suite running (round2/suite.sh -> round2/suite.log, round2/suite.status). keep2/ is the fixed copy.

## Repair round 2: what was done (DONE, uncommitted; working tree == keep2/, cmp)
- Blocking 1 (closed-run exception named two closes): every close is now named in SyncPolicy's ordered-run
  paragraph and Db::sync's doc (any write the run cannot take: append of a key at/below its greatest, an empty
  or overlong key, a value that does not inline; any put; any delete; any seal: Db::seal or the commit's at the
  threshold -- seal_while_idle never seals an ordered table), in CLAUDE.md's ordered-ingest paragraph, in
  ROADMAP's item and its "Done when" test, and in deferred.md. The extended test (round2/closed-run-test.rs:
  six closes x Small/Large x default/copyvals) FAILS on this tree in all 24 cases (round2/closed-before.log),
  the closing write's own commit surviving in each; it is not in durable.rs (the gap is not built).
- Blocking 2 (SyncPolicy still promised a tail and a policy sync over replayed frames): the first paragraph
  names an open run as the exception to "a tail lost whole"; the replay paragraph says a policy sync of a run's
  file leaves the log; the ordered-run paragraph names the window (frames of tables frozen before the run, or
  replayed by the open, that no sync covered are durable only at a sync, a flush or their seal's landing).
  Docs only. CLAUDE.md shape entry and ordered-ingest paragraph amended to match.
- Extra mutants: mutants.md "Repair round 2". Two new tests for survivors:
  a_sync_with_only_the_run_owing_syncs_nothing_of_the_log (E4, N1) and a_run_commit_after_a_failed_due_write_is_due
  (E8); both pass before the fix (counted through their mutants only). E5, E7 equivalent (argued there).
- Code under src/: doc comments only this round (db.rs differs from keep1/db.rs in comments only).
- Round-2 suite (round2/suite.log, copied to after-round2.log): clippy --release --all-targets -D warnings 0,
  cargo test --profile checked --no-fail-fast exit 0 (lib 404, db 226, durable 117, cipher 24, zpages 136, fuzz 5,
  ...). cargo fmt --check 0, scripts/fmt.sh 0. TMPDIR stage0/tmp/sync-run removed.

## Repair round 3 (started)
Step: reading. Working tree == keep2/ at start (sha256 matched all four). HEAD a424903d. Round-3 work dir: round3/.
Fixed copy for round-3 mutants: keep2/ unless a test is added (then keep3/, said here).
Step: plan. New tests: RunHeld::AfterDue (mutant 5: a count of one after a due sync, every arm); hand-off test
gains a second sync in a row (mutant 4); a run whose commits end on a write piece's boundary under a power cut
(mutant 7; needs a test accessor Db::run_held_bytes); the log directory's barrier paid by a sync with a run open
(mutant 9's question; own mutant Q1 gates the loose/dir clauses on !ordered). Mutant 9 expected equivalent
(wal_dir_unsynced implies loose_wals non-empty outside the kept-error state, which commit_staged answers first).
Step: new tests written (src/db.rs gains Db::run_written_and_held, test accessor); running on fixed tree -> round3/new.log
Fixed copy for round-3 mutants: keep3/ (db.rs = keep2 + Db::run_written_and_held; durable.rs = keep2 + round-3 tests), sums keep3/SHA256SUMS.
Step: round-3 mutants running (round3/mut3.py -> round3/mutants3.jsonl per verdict, round3/mut3-run.log). If restarted: check src/db.rs against keep3/SHA256SUMS, cp keep3/db.rs back if not, rerun mut3.py (skips done).
Step: round-3 mutants done (round3/mutants3.jsonl): F3, F4, F6 survive (equivalent), F9 survives because the
clause is unreachable -> taken out of Db::sync's log gate (comment says why, as run_again's). New fixed copy:
keep4/ (db.rs = keep3 minus `|| self.wal_dir_unsynced` in Db::sync + comment). Next: G1 (loose clause deleted)
via round3/mut4.py -> round3/mutants4.jsonl, then suite round3/suite.sh -> round3/suite.log.
Step: G mutants running (round3/mut4.py -> round3/mutants4.jsonl). Fixed copy keep4/.
Step: suite running (round3/suite.sh -> round3/suite.log, round3/suite.status).
Step: clippy found an unread initial value in the piece test; restructured (loop yields last). Rerunning clippy + durable.

## Repair round 3: what was done (DONE, uncommitted; working tree == keep4/, cmp)
- Nine extra mutants run (mutants.md "Repair round 3"): F1, F2, F5, F7, F8 killed; F3, F4, F6 equivalent
  (argued there); F9 survived because the clause was unreachable, so `|| self.wal_dir_unsynced` is taken out of
  Db::sync's log gate with a comment saying why (the only code change of the round besides a test accessor).
- New tests (all fail under M0, round3/before-new-tests.log): RunHeld::AfterDue; the hand-off test's second sync;
  a_sync_syncs_a_run_whose_commits_end_on_a_piece_boundary (kills F7); a_sync_with_a_run_open_pays_the_log_directorys_barrier
  (kills Q1, G1). New test accessor Db::run_written_and_held (#[doc(hidden)] pub, read-only).
- Own mutants: Q1, Q3, G1, G2, M0, all killed.
- Suite: round3/suite.log (after-round3.log): cargo test --profile checked --no-fail-fast exit 0 (db 226, durable
  119, zpages 136, cipher 24, ...). clippy -D warnings failed once on the new test (an unread initial value),
  fixed by restructuring the loop; then clippy 0, fmt --check 0, scripts/fmt.sh 0, durable 119 green again
  (round3/durable-after.log). The whole suite was not rerun after that test-only restructure.
- TMPDIR stage0/tmp/sync-run removed.
Step: commit-stage gate running (sh scripts/check.sh build test lint -> fix/sync-run/gate.log, gate.status)
Step: COMMITTED d4ef84ef; TMPDIR removed

## On the branch

The unit is `10898a94` on the branch, the commit `d4ef84ef` made again with the same tree, message, author and dates on top of `beecdc25`, which takes back the two snapshots (`340ee98e`, `e07a4571`) pushed while the unit was in review. Landed 2026-11-17 07:29 UTC by the orchestrator; the unit gate (`build test lint`) was green on `d4ef84ef` (gate.log, gate.status).
