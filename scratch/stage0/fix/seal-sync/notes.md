# seal-sync (seal-after-failed-sync, helper-presync-error P1/P2, manifest-pending-rewrite close half)

Base HEAD d4ef84ef. Uncommitted in the working tree. Fixed copy: keep/ (sha256 in keep/SHA256SUMS,
whole diff keep/worktree.patch). Progress log of the session: notes-progress.md.
STATE: all steps done; whole suite green (after.log "exit 0"); TMPDIR removed. Uncommitted.

## What was wrong
- land_front stopped the seal queue only for a seal that failed before its readable landing. A seal that
  failed after it (data fsync, envelope under a cipher, a run's presync joined in the inline arm) left its
  segments live "so the next landing's manifest names them": a merge's landing, a later seal's, or the
  pending arm's close named them durable and covered their sequence. The directory-fsync failure at the
  readable landing wedged, but a merge held across it still wrote its slot (publish never asked).
- P1: the non-inline presync runs on the writer after the job is handed; the segment work could land the
  job durable before the writer had its error (deterministic with Reader::hold_presync).
- Maint::stop discarded a joined seal's error and published the pending names durable.
- close_direct took the run, then waited for room; a refusal (queue stopped, frozen full) dropped the
  run's writer with the ordered table live, and the write that ended the run panicked in the table's
  order check ("an ordered memtable takes each key above its last"); Db::seal of the run dropped it.

## What changed (src/db.rs)
- Shared::wedge(e) (cold): keeps the first cause, sets seal_wedged. Every wedge site uses it; land_front
  wedges on every Err. Shared::wedged_err: "a seal failed to make its segments durable, and no seal lands
  and no manifest is written until the store is reopened: <cause>", kind of the cause.
- Maint::publish refuses while wedged (one load beside the slot's pwrite+fdatasync). publish_unsynced and
  the pending slot need no check: no seal landing runs once wedged (seal_phases_due/join_seal); checks
  there were unreachable and were taken out (comment at publish says why).
- Maint::stop wedges on a joined seal's Err/panic, so its pending publish is refused.
- P1: SealJob::awaits_writer (hand_seal's new arg, true only for the non-inline close with a presync);
  collect_seal lands such a job readable but durable only from join_seal (the writer's Drain, which comes
  after the presync); the writer wedges on a failed presync. No wait before the readable landing, no syscall.
- P2: nothing moved; the wedge-always closes it.
- Writer answers: join_seal (flush/settle), land_seal and take_fresh_mem answer wedged_err; Db::sync answers
  it after syncing what it would (one load + branch per sync); the first commit after a Maint-side
  failure answers the seal's error once (existing maint_err).
- close_direct waits for room before taking the run; leave_direct asks first and, with the frozen list
  full, keeps the refusal (keep_lost) and drops the write (returns false; append/put/delete return).
  A maint error met with room is the next commit's (as before) and the close goes on.
- Seams (doc(hidden), no cfg, load-then-swap): fail_next_seal_sync (before vault::persist, so it models the
  envelope step under a cipher), fail_next_seal_dir_sync, seal_sync_fault_pending, failed_presync_extent,
  Reader::hold_presync, Reader::hold_seal_durable_from_handle, merge_waits_at_landing (MERGE_HOLD_WAITING);
  the inline presync thread honours fail_presync.

## Tests (tests/durable.rs, section "seal-sync") -- before.log / after-mine.log
| test | before (fails at) |
|---|---|
| a_seal_whose_directory_fsync_failed_is_named_by_no_manifest | manifest names seg-...0063 |
| a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest | manifest covers 63 |
| a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log | manifest covers 7003 |
| a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest | CipherCounts::late 1 |
| a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records (writer arm, held) | slot names par-...|
| a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records | second flush answered Ok |
| a_runs_close_whose_fsync_failed_is_named_by_no_manifest (seal, flush, flush inline) | slot names both par- |
| a_seal_whose_fsync_failed_is_not_named_durable_by_the_close (thread; inline added after before.log) | close named durable |
| a_run_closed_after_its_kept_sync_failure_answers_the_device_and_reads_on | PASSES before: not evidence (guard) |
| a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing | panic in memtable |
| a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open | words (old "did not land") |
Each asserts the disk state first (manifest names / coverage / power-cut reopen), then the answers' words.
The inline variant of the pending-close test was added after before.log; MP1 (stop discards Err) kills it.

Restated: tests/db.rs a_failed_presync_is_the_flushs_answer -- asserted the next flush Ok; now Err with
the presync's words, commit still Ok, reopen reads every key.

## Mutants: mutants.md (17 run incl. control, all killed).

## Not built / left
- The helper-sync race (run unit): unchanged; ROADMAP already has it.
- Recovering a wedged store without a reopen (reseal into a new inode): ROADMAP.
- Merges/promotions still start while wedged; their outputs are unnamed and swept at the open.
- Db::sync's wedge check is a load+branch per sync not folded into an existing check.

## Not verified
- No real device error (dm-flakey): ROADMAP.
- No pricing / callgrind count (rule 10: no timing runs).
- The cipher test proves no late envelope and reopen; it does not cut a cipher store's power.
- The P1 writer-arm test waits up to 2 s for a landing that would happen if the fix were gone; a mutant
  landing slower than that under heavy load would read as killed late or survive.

## Repair round 1 (in progress)
STATE: round 1 started. Tree matched keep/SHA256SUMS at start (round-0 fixed copy = keep/). Round-1 fixed copy
will be keep1/ once made; until then keep/ is the last fixed copy.
step r1.1 (done): src edits -- leave_direct waits again unless wedged/no seal (loop), Db::sync check removed (sync answers
  nothing of a wedge; contract restated), Maint::publish check removed and folded into seal_between_phases (Maint field
  seals_stopped, set by Maint::stop_seals at every Maint-side wedge), Maint::stop asks seal_wedged in its existing
  condition; seams put_segment_work_err, release_seal_landing_once_taken; docs SyncPolicy/commit/flush/sync.
step r1.2 (done): tests -- two new (roomerr, wedgeroom), sync assertions restated via ss_sync_answers_nothing.
step r1.3: running section tests -> r1-mine.log
step r1.3 done: r1-mine.log green (13 + db's restated). Three tests restated for held merges (ss_merge_landed_or_held).
step r1.4 (done): also writes_go keeps lost / retries, take_fresh_mem freezes over a stopped queue with room, stop's
  join one arm, Shared::wedge stores the flag under the cause's lock; new tests hand_off_over_a_stopped_queue x2 (pass).
step r1.5: fmt, keep1/ copy, then before-round1.log (round-0 code as mutants) and mutants
FIXED COPY (round 1): keep1/ (src_db.rs, tests_durable.rs, tests_db.rs, CLAUDE.md, ROADMAP.md, SHA256SUMS,
  worktree.patch). keep/ is round 0 and is NOT the fixed copy any more. The tree must match keep1/SHA256SUMS
  before and after every mutant (mut1.py restores src/db.rs from keep1/src_db.rs and checks the sum).
step r1.5: fmt --check green, clippy -D warnings green; keep1 made. next: mut1.py -> mutants-r1.jsonl (verdict per mutant)
step r1.6: mut1.py running (results mutants-r1.jsonl, one line per verdict; the mutant in the tree is named in mut1-current.txt; logs m1-<name>.log). After a restart: copy keep1/src_db.rs over src/db.rs, check sha, rerun mut1.py (skips done ones).

## Round 1: what changed and why (the reviewers' items)
- leave_direct: drops the write and keeps the error only when room will not come (frozen full and the queue
  stopped, or no seal in flight; count read before the state). Any other error (a merge's maint_err met while
  the list is full of seals that will land) is the next commit's and the wait for room goes on (loop).
- Db::sync: the wedge load+branch removed (rule 1). A sync answers nothing of a stopped queue: what it
  promises is in the log, and a run's commits closed into the seal are outside its promise (existing doc).
  Every flush answers it. The every-sync form is in ROADMAP and not_built.
- Maint::publish: the wedge load removed (rule 1). Folded into seal_between_phases (a Maint-local field
  seals_stopped, set by Maint::stop_seals at every Maint-side wedge), which every manifest path except the
  seal's own landings and the close already asks; writer-side presync wedge: the awaits_writer job stays
  at the front, landed. Maint::stop asks seal_wedged in the condition it already evaluates.
  Consequence: a finished merge is held for good behind a stopped queue (tests restated: ss_merge_landed_or_held).
- Found by the extra mutant 11: writes_go (HEAD code) dropped a write after a hand-off when take_fresh_mem
  failed and answered it once, so later commits answered Ok beside a dropped write. Now it keeps lost only
  where the table cannot be replaced (wedged and no room, or no seal), retries otherwise; take_fresh_mem
  freezes the handed table over a stopped queue where there is room.
- Shared::wedge stores the flag under the cause's lock (mutant 8 becomes equivalent).
- Maint::stop's join: the panic and the error are one arm.
- Docs: SyncPolicy/commit/flush/sync restated (who answers in which words); CLAUDE.md entries restated.
X10b hung (100% CPU in a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken > 6 min); binary killed by hand -> recorded killed (hang)
X11b hung (parks forever in the without_room test > 60 s); binary killed by hand -> killed (hang)
W2 survived first; test strengthened (dropped write unread by the writer); keep1 tests_durable.rs updated; rerunning W2
step r1.7: full suite cargo test --profile checked --no-fail-fast -> after-r1.log
after-r1.log run 1 red: 1 my test bug (key 7 was in the model; fixed), leak/zpages red from a TMPDIR with '..' (mapping paths; green with canonical TMPDIR). W2 rerun with fixed test.
step r1.8 DONE: after-r1.log (full suite, canonical TMPDIR) exit 0, 20 binaries ok, durable 134. fmt --check and
  clippy -D warnings green. keep1/ is the final fixed copy (SHA256SUMS current). mutants.md has the round-1 table.
  before-round1.log: B1-B3 (round-0/HEAD code) fail the three new tests that are evidence. The wedgeroom test
  (a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken) passes on round 0: a guard, not evidence.
  TMPDIR removed. Uncommitted.
Not built (round 1): sync answering a stopped queue every time (writer-side kept answer; contract for the owner;
  ROADMAP); the helper-sync race (run unit); a panic seam for the seal thread (X6 survives, unreachable).

## Repair round 2 (in progress)
STATE: round 2 started. Tree matched keep1/SHA256SUMS at start (keep1/ = round-1 fixed copy, the fixed copy until keep2/ exists).
step r2.0: reading the blocking items.
step r2.1: decided: (a) Maint observes the shared wedge where it already loads it -- seal_phases_due's and join_seal's
  wedged branches set seals_stopped (cold store; seal_phases_due becomes &mut, Db::seal_due &mut); (b) docs restated:
  inline arm answers every seal / threshold commit, settle and zpages_settle answer, Shared::seal_wedged doc, test docs;
  (c) tests: merge held across a writer-side presync wedge before readable (blocking 1), inline-arm + settle words,
  ss_failed_presync variants "both" (new seam SEAL_HOLD_READABLE in collect_seal's readable branch, for extra mutant 1)
  and "pending" (manifest_pending, for extra mutant 6). Writing tests now (src: seam only), then before-round2.log.
step r2.2: tests written (durable.rs: 3 new tests + variants; src: SEAL_HOLD_READABLE seam). running before-round2.log (run_r2.sh) on round-1 code
step r2.2b: first before run: new merge test failed at in_flight (race in the test's wait); test's wait fixed; rerun -> before-round2b.log (before-round2-first.log is the first)
step r2.3: before-round2b.log: merge test fails at 'a manifest was written after the seal queue stopped' (5,42) vs (4,42). fix2.py applied (seals_stopped set in seal_phases_due/join_seal wedged branches; docs at seals_stopped, seal_between_phases, publish). next: crate docs + CLAUDE.md + ROADMAP
step r2.4: crate docs done (SyncPolicy, Shared::seal_wedged, Db::settle). next CLAUDE.md, ROADMAP, then build+tests
step r2.5: CLAUDE.md, ROADMAP done; fmt applied. running section tests after fix -> after-r2-mine.log (run_r2.sh)
step r2.6: after-r2-mine.log green (11 section tests incl. 3 new); fmt --check and clippy -D warnings (r2-clippy.log) green.
FIXED COPY (round 2): keep2/ (src_db.rs, tests_durable.rs, tests_db.rs, CLAUDE.md, ROADMAP.md, SHA256SUMS, worktree.patch).
  keep1/ and keep/ are older rounds and NOT the fixed copy. The tree must match keep2/SHA256SUMS before and after every mutant.
step r2.7: mutants via mut2.py -> mutants-r2.jsonl (one line per verdict as it lands; current mutant named in mut2-current.txt).
  After a restart: copy keep2/src_db.rs over src/db.rs, check sha, rerun mut2.py (skips done ones).
step r2.6b: keep2 made then superseded: removed join_seal's seals_stopped set (redundant: every manifest path looks via seal_phases_due first), swapped stop_seals above in_seal.fetch_sub (3 sites, closes the words race), new test a_stopped_queue_answers_the_failure_that_stopped_it (for extra 3/9). running -> after-r2b-mine.log; keep2 to be remade after
step r2.7: after-r2b-mine.log green; clippy+fmt green; keep2/ REMADE (this is the fixed copy, SHA256SUMS current). mut2.py started -> mutants-r2.jsonl, mut2-run.log
step r2.8: mutants pass 1 done (mutants-r2-pass1.jsonl). R5 'killed' by a race in my new test (commit read before put_maint_err): fixing tests' first-answer wait; moving the readable seam into collect (holds the whole poll) for an R10 test; then keep2 remade and R1,R2,R5,R10,R12,R1+R12 rerun (pass 2)
step r2.9: seam moved into collect; ss_commit_answer helper (a_later_seal + first-cause test); new test a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look. running -> after-r2c-mine.log
step r2.10: keep2/ REMADE again (fixed copy; SHA256SUMS current). pass 2 of mutants: mut2.py -> mutants-r2b.jsonl, mut2b-run.log

## Round 2: what changed and why (the reviewers' items)
- Blocking 1 (writer-side presync wedge before the job's readable landing let merges/promotions write manifests):
  Maint::seal_phases_due (now &mut; Db::seal_due &mut) sets seals_stopped in the branch that already finds
  seal_wedged -- a store in the cold branch only, no new load. Every Maint path to a manifest looks there first:
  collect (and the loop's idle_plan/shape_if_read after it), inline the commit's seal_due before
  flush_merge_finished; the writer's asks end at join_seal's top. A join_seal set was tried and taken out
  (redundant: nothing between it and the next collect writes a manifest). Test:
  a_presync_that_fails_before_its_seals_publish_stops_every_manifest (before-round2b.log: fails at "a manifest
  was written after the seal queue stopped", slot (5,42) vs (4,42)).
- Blocking 2 (docs): SyncPolicy restated (who answers: flush and settle every time; with the segment work
  inline every seal and every commit at the threshold, batch in the log first; Away arm a seal/commit needing
  room; sync nothing); Shared::seal_wedged doc ("every sync and flush" -> "every flush and settle"; manifests
  stop at the segment work's first look); Db::settle doc; Maint::publish comment; seal_between_phases and
  seals_stopped docs; test doc of a_later_seal...; CLAUDE.md crash-discipline + "A failure after the publish"
  amended; ROADMAP item restated. Tests of the words: a_stopped_queue_is_answered_by_every_seal_inline (inline
  seal, threshold commits x2, settle, flush, sync Ok, reopen) -- passes on round-1 code (a test of the docs,
  not evidence of a fix); settle answers added to a_seal_whose_data_fsync_failed..., ss_failed_presync, the
  new merge test.
- Extra 7 taken as a fix: stop_seals moved above in_seal.fetch_sub at the three failure sites in
  land_front_readable/land_front, so a writer that reads the count and then the flag finds the stop wherever
  the count is gone (closes the words race "the frozen table's seal did not land"). Failure path only.
- New seam Db::hold_readable_landing / Reader::hold_readable_landing_from_handle (SEAL_HOLD_READABLE, a load in
  collect only when a readable phase is due, before the landings' syscalls). Used by
  ss_failed_presync "both" (extra 1) and a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look (extra 10).
- ss_failed_presync gained variants "both" and "pending" (manifest_pending), test
  a_flush_after_a_failed_presync_is_not_ok_whichever_way_its_seal_lands.
- a_stopped_queue_answers_the_failure_that_stopped_it (extra 3/9: first cause kept).
- Test race fixed: a commit right after seals_wedged() reads true can answer Ok (the work stops the queue before
  it leaves its error): ss_commit_answer helper, used in a_later_seal... and the first-cause test.
step r2.11: pass 2 (mutants-r2b.jsonl): R1 survived (lifter released both holds at once, the presync's wedge raced the look) -> lifter fixed; R6/R7a/R7b/R8 'killed' by a flaky first answer in ss_run_close_fails (seal arm) -> ss_commit_answer there; R10 survived: equivalent by done.is_ok() (a seal failure in collect_seal is Err, so the merge is not landed that pass). Rerunning tests -> after-r2d-mine.log then pass 3
step r2.12: keep2/ REMADE (tests changed; src same). pass 3: mut2.py N0 R1 R1R12 R6 R7a R7b R8 R10 R10b -> mutants-r2c.jsonl, mut2c-run.log
step r2.13: pass 3 done (mutants-r2c.jsonl). keep2 CLAUDE.md refreshed (test-race note). rerunning round-1 X10c -> mutants-r2c.jsonl (same file)
step r2.14: X10c survives now (its round-1 kill was the racing test). R8 and X10c: the seals==0 arms in writes_go and leave_direct taken out (unreachable once stop precedes count-out), loads removed, docs+CLAUDE.md. next: clippy, keep2 remake, full suite -> after-r2.log
step r2.15: keep2/ REMADE (final candidate). full suite running -> after-r2.log (exit line at end)
step r2.16 DONE: after-r2.log (full suite, canonical TMPDIR) exit 0, durable 139; durable binary 5 more runs green
  (r2-repeat.log); fmt --check and clippy -D warnings green. keep2/ is the final fixed copy (SHA256SUMS current,
  tree matches). mutants.md has the round-2 table. TMPDIR removed. Uncommitted.
- Also round 2: the seals==0 arms of writes_go and leave_direct taken out (R8/X10c survived: unreachable once the
  stop precedes the count-out), with their loads.
Not verified (round 2): R6/R7a/R7b are held by reading only (timing); the "both" variant's lifter sleeps 500 ms
  for the seal thread to return -- on a slower host the look may meet only the readable phase, which weakens the
  variant (R1 then survives) and does not make it fail.

## Repair round 3 (in progress)
STATE: round 3 started. Tree matched keep2/SHA256SUMS at start (keep2/ = round-2 fixed copy, the fixed copy until keep3/ exists).
step r3.0: blocking item = docs (sync answers a stopped queue after a dropped write: run-ending write or a write after a
  sync's hand-off, with the frozen list full). Then 10 extra mutants via mut3.py -> mutants-r3.jsonl.
step r3.1 (done): docs3.py applied (SyncPolicy, Db::sync, CLAUDE.md crash discipline, ROADMAP item). next: check the test asserts sync words in the full case; then keep3 + mut3.py
FIXED COPY (round 3): keep3/ (src_db.rs, tests_durable.rs, tests_db.rs, CLAUDE.md, ROADMAP.md, SHA256SUMS, worktree.patch).
  keep2/ and older are NOT the fixed copy. Tree must match keep3/SHA256SUMS before/after every mutant (mut3.py restores src/db.rs
  from keep3/src_db.rs and checks the sum).
step r3.2: new test a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline added (extra 1). next: run it on fixed code, then under R2 (before)
step r3.3: inline test passes on fixed code (r3-inline.log). keep3/ REMADE (fixed copy). mut3.py running -> mutants-r3.jsonl (verdict per line), current mutant in mut3-current.txt, logs m3-<name>.log, run log mut3-run.log. After restart: copy keep3/src_db.rs over src/db.rs, check sha vs keep3/SHA256SUMS, rerun mut3.py (skips done).
step r3.4: E1 (=before of the inline test) killed by both presync tests; before-round3.log. E2,E3 killed. waiting on the rest
step r3.5: E4 killed but the two stops_every_manifest tests HUNG under it (flush waits on a held seal / joins a held merge). Prepared watchdog3.py (seam Reader::lift_landing_holds_from_handle + SsWatchdog in both tests) -- apply ONLY after mut3.py ends (it rewrites src/db.rs), then remake keep3 and rerun E4 (+N0,E1).
step r3.6: verdicts so far: E1-E5 killed (E5 only by the publish debug assertion in the inline close test; the thread variant's maint panic is swallowed at close), E6/E7/E9 survived. Plan: take out land_seal's and take_fresh_mem's seals==0 checks and loads (unreachable), E9 equivalent (flush always asks Drain after close_run). Waiting for O1-O3.
O2 hung (writer loops in writes_go over a stopped queue without room, >9 min); binary killed by hand -> killed (hang)
O3 killed by 8 failing tests; the two stops_every_manifest tests hung (no watchdog yet); binary killed by hand
step r3.7: applied watchdog3.py (seam + SsWatchdog), checks3.py (E6/E7 checks and their count loads taken out), comments3.py. next: CLAUDE.md amendment, build+section tests, keep3 remake, rerun N0/E1/E4/E6/E7 equivalents
step r3.8: after-r3-mine.log green (21 section + db). keep3/ REMADE (fixed copy, SHA256SUMS current). pass 2: mut3b.py N0 E1 E4 E5 E10 O3 -> mutants-r3b.jsonl (mut3b-run.log)
step r3.9: clippy green (r3-clippy.log). full suite running -> after-r3.log (exit line at end)
step r3.10: mutants.md round-3 table written. waiting for after-r3.log
step r3.11 DONE: after-r3.log (full suite, canonical TMPDIR, --no-fail-fast) exit 0, 20 binaries ok, durable 140. fmt --check
  and clippy -D warnings green. keep3/ is the final fixed copy (SHA256SUMS current, tree matches). mutants.md has the round-3
  table. TMPDIR removed. Uncommitted.

## Round 3: what changed and why (the reviewers' items)
- Blocking (docs): a sync does answer a stopped queue once a write was dropped for want of room -- the run-ending write or the
  first write after a sync's hand-off (writes_go/take_fresh_mem). SyncPolicy, Db::sync, CLAUDE.md crash discipline and the
  ROADMAP item name both drops and say every commit, sync and flush answers from the dropped write on. Already asserted by
  ss_no_room_for_the_run (sync answers the cause) and ss_handed_over_a_stopped_queue("full"); no code change for it.
- Extra 1: new test a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline (inline segment work,
  partition_on_flush off, flush_schedules: a flush leaves a merge held; a run's presync fails in the next flush; the merge
  finishes; the next commit must not land it). Fails under E1 (= round-1 code): before-round3.log, slot (4,21) vs (3,21).
- Extras 6/7: land_seal's and take_fresh_mem's `seals == 0` arms survived (unreachable); taken out with their in_seal loads;
  comments at the stop-before-count-out sites and land_front's doc restated (the writer reads no count now).
- E4/O3 hung two tests; SsWatchdog + Reader::lift_landing_holds_from_handle (doc(hidden) seam, test-only, no hot path).
- E9 equivalent by reading (flush is the only draining caller and always asks Drain after close_run).
- E5/E10 are killed only through Maint::publish's debug assertion (inline close variant); in release the clause holds by reading.
- CLAUDE.md: "A refusal that came after the take" amended (the arms taken out of the waits; the doc that named one drop).

## commit step (Tue Nov 17 11:21:45 UTC 2026)
- tree sha256 matches keep3/. Reviewer item (sync docs) already addressed in tree.
- step 2: gate running, log at /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/fix/seal-sync/gate-commit.log, status gate-commit.status
- committed 2744a21a8b01bfd613cc80348b6a8dada1d07b99; tmp removed
