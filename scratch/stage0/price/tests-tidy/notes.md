# tests-tidy (followups 1, 22, 25, 28, 31, 34, 36, 46, 52, 56; fixes.md concerns 6, 8, 10, 16b)

Base: HEAD 1ea68aae (run base 0c6748bb + other-slot .. open-cost). Tree clean at start.
TMPDIR for every cargo: /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/tmp/tests-tidy

## Plan
- Item 1/56: seam `Db::pause_segment_work()` (a MaintJob the segment work's thread waits in until the
  guard drops; nothing on a success path: a match arm on the job channel). tests/durable.rs: `copy_live(&db, from, to)`
  pauses across the copy; every copy of a live store moves to it, or keeps copy_store with a comment.
- 22/25: seal-sync sleeps -> holds where one exists.
- 28: slot-choice kpageflags test.
- 52: vault io trace bounded.
- 31, 34, 36, concerns 6, 8, 10, 16b.

## Step log
- step 1 (reading): in progress
- step 2 (14:55): engine seams in (src/db.rs: Db::pause_segment_work + MaintJob::Pause, debug_assert at the
  pending landing; Db::trace_dir_io(dir,on)/trace_slot_descriptors(dir); src/db/vault.rs per-dir records).
  Tests: durable.rs copy_live/copy_closed/copy_held, trace sites per dir, O3 deadline wrapper, new tidy section
  at the end (leave_direct live test, ignored log-retired test), shape-move kill states, unprivileged page probe;
  db.rs concern 8 restated + v2 fixture comment; fuzz.rs ZFUZZ_SYNCVARY.
  Fixed copies: keep/*.fixed (sha256.txt). Engine edits as script: scripts/engine_edits.py (applied to HEAD's db.rs).
- stress of the no-pause mutant (copy_live without the pause): bin/durable-nopause, scripts/stress.sh,
  results stress-nopause.log (one line a run), failing outputs in stress-nopause/.
- mutants: scripts/mutants.json, runner scripts/mut.py -> mut-results.txt (verdict per line), mut-<name>.log; running T1..T11 (started 15:04)
- first mutant batch done (mut-results.txt). T1 (pause returns without waiting for the thread) SURVIVED the first test;
  added a_pause_asked_inside_a_landing_returns_after_it (a landing held at its slot write by the watch callback),
  T1b reruns it. Second batch (T1b, DM-b, DM-a, DM-j: delete-max's mutants credited to the racing test) chained after.
- stress-nopause: item 56's refusal reproduced (w1 r2); an_injected_presync_failure_is_taken_once_inline flaked
  (a read race of the test's own; the fixed tree waits for the extent).
- NEXT: build the fixed durable binary -> bin/durable-fixed, stress fixed 4x40 (stress-fixed.log); fuzz ranges with
  ZFUZZ_SYNCVARY=1 (0..4, 4..8, 8..12, 12..16) -> fuzz-syncvary-*.log; full suite --no-fail-fast -> after.log;
  fmt; mutants.md; finish notes.

## Items 22/25: the seal-sync sleeps and waits (read, kept, why)
- tests/durable.rs ss presync "both" variant: 500 ms sleep for the seal thread's readable signal, its fsyncs and its
  return with the poll held. The thread's return is visible only to the segment work (its JoinHandle); no hold stops
  the seal thread after its fsyncs, and a mark the thread set there would be a store on every seal's path (rule 1).
  Kept. Shorter than the thread on a slow host, the landing meets the readable phase alone: the test weakens, never
  fails red.
- the 2 s waits (the writer arm and "both"): waits for a slot naming the segment that must NOT come (an absence),
  bounded; kept for the same reason (an absence has no hold).
- a_merge_finished... / presync-before-publish tests: 500 ms and 300 ms sleeps after a merge's landing hold is lifted,
  for the merge thread to return; the merge thread's return is visible only to the segment work. Kept. (The hold bit
  could be cleared inside the held branch only, but "left the hold" is not "returned"; the gap is a bet either way.)
- SsWatchdog (60 s) and ps_watchdog: ends for tests whose writer waits on a hold, not bets; kept.
- an_injected_presync_failure_is_taken_once{,_inline}: read the presync's extent at the flush's answer -- a bet the
  stress lost (2 of the first 32 runs); now waits for it (wait_for).
- stress-nopause DONE: 48 runs; 42 green; item 56's test refused twice (w1 r2, w1 r10: "names seg-... missing from the
  directory"); an_injected_presync_failure_is_taken_once_inline failed three times (the test's own read race, fixed);
  one run (w1 r3) ended by SIGBUS (exit 135) after 212 tests with no test named and nothing in dmesg: not attributed.
- mutants so far: T1 survived -> T1b killed by the new inside-a-landing test; T2-T10 killed; T11 survived twice ->
  the shape test now also kills right after each crossing's open, T11c killed; DM-b killed by the ends-a-run test
  alone under the pause (its stage-0 kill stands); DM-a killed by 9; DM-j killed by 7 and no longer by the ends-a-run
  test (its stage-0 credit there was the race).
- fmt applied; keep refreshed (all files). Fixed stress started: bin/durable-fixed (source sum in
  bin/durable-fixed.source-sum), stress-fixed.log, 4 workers x 40 runs.
- fuzz with ZFUZZ_SYNCVARY=1, all five fuzz tests, seeds 0..4, 4..8, 8..12, 12..16 (fuzz-syncvary-*.log): every
  store answer matched the model in every range (FAILURES 0). One red: fuzz_range_under_a_cipher_with_the_compact_form
  at 8..12, "no read or scan went through a form file" -- the range's own premise, not an answer. Attributed: the
  same range without the variable passes (fuzz-nosyncvary-8.log); per seed under the variable (fuzz-verbose-*),
  seed 10 draws Always (no EveryN, no synced crash: the variable's only effects there are the shifted draws) and
  reads nothing through a form, where without the variable the same seed reads 695 -- the premise depends on the
  seeds' draws, which the extra draw moves. Not a defect.

## Summary (step 9)

What was wrong (tests, not the engine):
- item 1/56: ~70 copies of a live store in tests/durable.rs relied on each test remembering a hold; a landing inside
  a copy gives a slot naming files the copy lacks (no crash state). Reproduced: the delete test's refusal, 2 of 48
  busy runs with the pause taken out.
- item 52: the vault's call record was process-wide, on for good once any test asked, growing with every test.
- item 28: an_open_makes_the_slot_it_takes_durable read only kpageflags (root) for the slots' pages; unprivileged it
  printed and passed; a sync_slot that records a sync it never made survived unprivileged.
- concern 10: run-errors O3 killed only by a hang.
- concern 8: the db test's name said "alternates"; its assertion said only "older".
- concern 6: ROADMAP pointed at a reproduction in session scratch.
- item 36: the shape-crossing test read values after closes only.
- item 34: the pending landing's discard of write_slot's owed answer, unasserted.
- 16b: the fuzz never drew EveryN(1)/EveryN(u32::MAX) nor synced before a crash; the leave_direct-after-kept-error
  state was tested only by a reopen.
- found on the way: an_injected_presync_failure_is_taken_once_inline read the presync's extent at the flush's
  answer while the inline arm's presync thread had not recorded it (3 of 48 busy runs).

What changed:
- src/db.rs: Db::pause_segment_work (+ MaintJob::Pause, SegmentWorkPaused): a job the segment work's thread waits in,
  taken by its loop before any pin; nothing on any success path (a match arm on the job channel, a variant).
  debug_assert at the pending landing that no slot is owed. Db::trace_dir_io(dir, on), Db::trace_slot_descriptors(dir).
- src/db/vault.rs: the call record kept per directory asked for (IoRecord list); the recorders' one load is unchanged
  (IO_SEAMS & SEAM_TRACE), the filter by root runs only when tracing.
- tests/durable.rs: copy_files/copy_live/copy_closed/copy_held (copy-sites.txt: 70 live, 14 closed, 3 held); every
  trace reader asks by its directory and lets go after its last read; O3's test on a thread with a 60 s end;
  os_page_kept_unwritten (fadvise DONTNEED + mincore) as the unprivileged instrument; the shape test kills after each
  life's flush and after each crossing's open, both slot parities; presync-extent wait; new tidy section: leave_direct
  live test, the ignored log-retired reproduction, pause tests (lands nothing; returns after a landing in flight),
  the record test.
- tests/db.rs: the slots test renamed a_manifest_publish_spares_the_newest_durable_slot_and_the_open_takes_the_newest_valid,
  holds every publish of its second round by the slot watch (never over the newest known durable; the slot not
  written holds exactly the last synced sequence of its file); the v2 fixture says why it is placed by parity.
- tests/fuzz.rs: ZFUZZ_SYNCVARY.
- docs: ROADMAP (log-retired item points at the test), CLAUDE.md (copy entry amended, trace corollary, page-cache
  instrument, O3's end).

Pricing: not priced (tidy unit). Nothing added to a success path: the pause is a job on the channel the segment work
already reads; the trace's recorders keep their one load; the debug_assert is debug-only.
- after.log done (beside the stress; an OOM kill voided stress-fixed w4 r6). lib pager residency test red under that
  pressure, ok alone x3; db a_flush_publishes_its_seal_before_the_seal_is_durable red (timer bet) -> restated to a
  hold lifted by the thread that sees the publish, ok. clippy (release, all targets, -j2) clean after two fixes; fmt
  clean. keep refreshed. NOW: waiting on stress-fixed (4x40); then whole db and lib binaries once quiet, the durable
  binary with SUPDB_TEST_UNPRIVILEGED=1, remove TMPDIR, finish.
