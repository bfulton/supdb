# rename-arm mutants

Runner: mut.py (mutants.json, made by mkmut.py and edited after); each mutant applied to src/db.rs, the
named tests run with `cargo test --profile checked --test durable -- <names> --exact`, the verdict
written to mut-results.txt, src/db.rs put back from keep/src_db.rs.fixed and checked by sha256. Logs
mut-<id>.log. report.md and deferred.md list no mutants for these defects (they were found since stage 0);
all below are this unit's own.

| id | mutant | verdict | killed by |
|---|---|---|---|
| M1-no-stop | land_front: the stop on a failed rename publish deleted | KILLED | a_rename_publish_whose_directory_fsync_fails_names_nothing_durable_after (+_under_a_cipher) |
| M2-stop-gate-flipped | the stop gated on `manifest_slots` instead of its negation | KILLED | same two |
| M3-stop-after-count | the stop moved after the seal is counted out | SURVIVED | -- see below |
| M4-no-open-dir-sync | the rename open's directory fsync deleted | KILLED | a_kill_between_a_rename_and_its_directory_fsync_survives_a_power_cut_after_the_reopen (+_under_a_cipher) |
| M5-open-dir-sync-gate-flipped | the rename open's block gated on the slots | KILLED | the (c) pair, the dir-sync-fails pair, a_pending_store_reopened_under_the_rename_shape_survives_a_power_cut |
| M6-open-dir-sync-after-removals | the rename open's directory fsync (and its seam) moved after the removals | KILLED | an_open_under_the_rename_shape_whose_directory_fsync_fails_removes_nothing (+_under_a_cipher) |
| M7-no-pending-sync | the rename open's pending fsyncs deleted | KILLED | a_pending_store_reopened_under_the_rename_shape_survives_a_power_cut |
| M8-pending-sync-cipher-gate-flipped | the pending fsyncs gated on a cipher (`held.is_some()`) | KILLED | both (d) tests |
| M9-seam-ignored | the publish's dir-fsync seam never fails (the cold path never taken) | KILLED | the (b) pair (premise: the flush answers the failure) |
| M10-retire-before-publish | guard (a): a seal's log retired before the publish that names its segments | KILLED | a_rename_shape_publish_leaves_a_whole_manifest_at_every_step (+_under_a_cipher) |
| M11-slot-index-zero | guard (f): every manifest read as slot 0 | KILLED | an_open_under_the_rename_shape_refuses_by_the_manifest_file_it_read |
| M12-make-slots-no-dir-sync | guard (e) rename->slots: manifest_make_slots's directory fsync deleted | KILLED | a_rename_shape_store_killed_before_its_directory_fsync_reopens_under_the_slots (+_cipher) |
| M13-open-slot-sync-only-under-slots | guard (e) slots->rename: the open's slot fdatasyncs skipped under the rename shape | SURVIVED (first run) | -- a missing test: the slots->rename kill state never had the open sweep what the slot stopped naming |
| M13b (same, after the test was added) | | KILLED | an_open_under_the_rename_shape_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+_under_a_cipher) |
| M14-publish-no-dir-sync | guard (a): manifest_write's directory fsync deleted | SURVIVED (first run) | -- a missing test: every (a) state is built at the watch, before the fsync |
| M14b (same, after the test was added) | | KILLED | a_rename_publish_fsyncs_the_directory_before_its_flush_is_answered (+_under_a_cipher) |

M3, the survivor: the stop's place before the count-out mirrors land_front_readable's, whose comment says
why there (a table no landing will retire beside a count that reads "no seal in flight"). On this path the
failed seal's table was retired at its readable landing, which came before land_durable, so a reader of the
count between the two steps finds no frozen table waiting on that seal and the order is not observable by
any state a test can build; it is an order kept for symmetry with the sibling, not a check, and nothing was
taken out. Recorded as a survivor with that argument, not as a kill.

M14 under the prefix model: the (a) states assume a power cut keeps a prefix of the directory's operations,
and under that model a publish without its directory fsync loses no acknowledged commit (the log's unlinks
come after the rename), so M14 is caught by the record of the calls (`Db::trace_dir_io`), not by a crash
state.

## Repair round 1

Runner mut-r1.py (keep/ now holds src_db.rs, src_db_vault.rs, tests_durable.rs; restored and checked by sha256
after each); lists mutants-r1.json, mutants-r1b.json and the round-0 mutants.json re-run against the round-1 tree;
verdicts in mut-results-r1.txt, logs mut-r1-<id>.log. Killing tests are the FAILED lines of each log.

Brief's extra mutants:
| id | mutant | verdict | killed by |
|---|---|---|---|
| X1 | rename open block moved after the cipher sweep, before the orphan removals | KILLED | an_open_under_the_rename_shape_whose_directory_fsync_fails_removes_nothing_under_a_cipher |
| X2 | land_front stop unconditional (slots too) | SURVIVED first run (no slots test failed a seal's own publish); KILLED after the test was added | a_seals_publish_whose_slot_sync_fails_is_written_again_by_the_next_under_the_slots (+_and_a_cipher) |
| X3 | `&& had_manifest` dropped | SURVIVED (twice) -- harmless: with no manifest read the store is one from before manifests, the block's only effects are one extra directory fsync (alt_found is false with no manifest); the gate is kept as the brief's allowance names it ("an open with a manifest read") |
| X4 | `held.is_none()` dropped from the rename open's sync_pending | KILLED | a_pending_store_reopened_under_the_rename_shape_survives_a_power_cut_under_a_cipher |
| X5 | stop only when the error is the dir-fsync seam's | KILLED (by the new test) | a_rename_publish_that_fails_before_its_rename_stops_the_seal_queue (+_under_a_cipher) |
| X6 | dir fsync only where a segment is removed | KILLED | (c) cipher, srs plain pair, dir-fsync-fails cipher. Answer to the brief's question: the fsync is not needed for the wal_later or spare_extra removals (a log file past a loose end, a spare: neither manifest needs them); in the clear what needs it beyond the orphan sweep is the alt removal and a direct run's temp name the manifest names (not built); under a cipher on_disk lists no envelope, so the gate skipped the fsync before the vault sweep |
| X7 | block after the spare and log removals, before the orphan sweep | KILLED | ..._directory_fsync_fails_removes_nothing_under_a_cipher (the vault sweep is before both). In the clear the log/spare removals before the fsync are harmless (as X6) |
| X8 | the hook clears the fail seam without failing | KILLED | the (b) pair |
| X9 | sync_pending gated on the reopening `opts.manifest_pending` | SURVIVED first run ((d) reopened with manifest_pending on); KILLED after (d) gained reopens without it | a_pending_store_reopened_under_the_rename_shape_without_the_option_survives_a_power_cut |

Own (round 1):
| id | mutant | verdict | killed by |
|---|---|---|---|
| R1 | the rename open's manifest.alt handling deleted | KILLED | the four srs tests |
| R2 | alt removed only where it was taken | KILLED | srs (manifest-newest) pair |
| R3 | taken alt removed without writing `manifest` first | SURVIVED first run (no state between the rename open and its first publish); KILLED after | a_store_moved_from_the_alt_slot_killed_at_its_rename_open_is_whole (+_under_a_cipher) |
| R4 | alt_found set by slot 0 | KILLED | 17 rename-section tests |
| R5 | alt removed before the open's dir fsync | SURVIVED -- harmless: both steps precede every removal and every publish of the life, and the dir fsync after covers the removal |
| R6 | the hook (watch, fail) run after the fsync and its trace record | SURVIVED (twice) -- the seam's own place: the states are built from file contents, which the fsync does not change, and a failure answered after the fsync leaves the same files and answers; the trace-based test sees other directory fsyncs (the log's rotation) after each mark. Not a check of the engine; recorded, not killed |
| R7 | hook never asked | KILLED | 18 tests |
| R8 | publish's fsync marked Other | KILLED | 18 tests |
| R9 | manifest_write's dir fsync deleted (M14b again) | KILLED | 18 tests incl. a_rename_publish_fsyncs_the_directory_before_its_flush_is_answered |
| R10 | hooks dropped at every seamed fsync | KILLED | 18 tests |
| M4b | rename open's dir fsync deleted | KILLED | the (c) pair |
| S1 | slots open: make_slots after the cipher sweep (the order before this round) | KILLED | a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher (this is the test's before-state) |
| S2 | make_slots's fail seam ignored | KILLED | the crossing-fail pair |
| S3 | the open passes no seam to make_slots | KILLED | the crossing-fail pair |

Round-0 mutants re-run on the round-1 tree: M1, M2, M5, M7, M8, M10, M11, M12, M13b KILLED (same tests as before);
M3 SURVIVED (as before, argued above); M4, M6, M9, M14b did not apply (their text moved) and stand as M4b, X1/X7,
X8/R7, R9.

## Repair round 2

Runner mut-r1.py with MUTS=mutants-r2.json (A..H on the round-1 tree), mutants-r2b.json and mutants-r2c.json (after
the removal seam and its test went in); verdicts in mut-results-r1.txt (lines r2-*), logs mut-r1-r2-<id>.log. src/db.rs
and src/db/vault.rs restored from keep/ and checked by sha256 after each.

| id | mutant | verdict | killed by / why not |
|---|---|---|---|
| r2-A | land_front's Err: stop_seals after `in_seal.fetch_sub` | SURVIVED (round-1 tree, and again with the new tests as A-b) | unobservable, argued: the only readers of `in_seal` are `seals_first_partition` (src/db.rs ~28337; the failed seal's segments are published by then, so `segs()` is non-empty and the answer is false either way) and the test accessor `in_flight`; `seals_stopped` is the segment work's own field, read by this thread after `land_front` returns; and the writer's flush waits on this thread's answer, which follows the stop. The place before the count is kept to mirror `land_front_readable`. Not a check; nothing taken out. |
| r2-B | the rename open writes `manifest` at alt's sequence (bumped in memory) | SURVIVED (and B-b) | unobservable, argued: the two files then hold one sequence and the same names; `manifest_read_all` sorts stably by sequence, so slot 0, `manifest`, is taken at a tie under either shape, and `manifest` is renamed and its directory fsynced before the removal, so no device state holds an older `manifest` beside alt at that sequence. The expected killer (a_store_moved_from_the_alt_slot_killed_at_its_rename_open_is_whole) and the new failed-removal test, which leaves both files at a tie, open whole. |
| r2-C | the alt removal's error ignored (`let _ = removed`), with a failure injected (`Options::fail_open_alt_removal`, new) | KILLED | a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it (+_under_a_cipher): "the open answered no failure" |
| r2-D | hook_publish_dir_sync fails a directory fsync of any kind | KILLED | named FAILED before the binary aborted: a_rename_publish_fsyncs_the_directory_before_its_flush_is_answered (+cipher), a_rename_publish_whose_directory_fsync_fails_names_nothing_durable_after (+cipher), a_store_moved_slots_rename_slots_killed_before_a_directory_fsync_survives_a_power_cut, and in the first run the open's dir-fsync-fails pair and the slot-sync-fails test. Both runs then aborted the binary ("failed to allocate an alternative stack: Cannot allocate memory", a thread start), under the mutant only; the section is green on the fixed tree. Read as a kill: the runner's BUILD-FAIL/OTHER is its label for exit 101 without a summary line. |
| r2-E | sync_dir_seamed's hook found for any directory | KILLED | 18 tests (round-1 tree), e.g. a_rename_shape_publish_leaves_a_whole_manifest_at_every_step, a_kill_between_a_rename_and_its_directory_fsync_survives_a_power_cut_under_a_cipher |
| r2-F | the alt removal moved after the sweep and the log removals | SURVIVED on the round-1 tree (no state had anything else to remove beside a failing removal); restated on the seam's text as Fb: KILLED | a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it (+cipher): "changed the directory before its failure: [spare-..., x-....z.tmp]" |
| r2-G | the rename open's `mseq += 1` dropped, `manifest` written at alt's sequence | SURVIVED (and G-b) | as B: a tie of identical names resolved to `manifest`; nothing in the next life's sequence choice reads the difference (its first publish writes one past the newest). A value, not a check; kept so the store's sequences stay strictly increasing per publish. |
| r2-H | stop_seals only for an error whose words say "directory fsync" | KILLED | a_rename_publish_that_fails_before_its_rename_stops_the_seal_queue (+_under_a_cipher; the cipher's temp is a vault create) |
| r2-S4 (own) | the removal seam never fails | KILLED | the failed-removal pair: "the open answered no failure" |

## Repair round 3

Runner mut-r3.py over mutants-r3.json; verdicts in mut-results-r3.txt, logs mut-r3-<id>.log; src/db.rs restored from
keep/ by sha256 after each. T1 ran before a container restart; T2..O5 re-run after it (mut-r3-runner2.log, EXIT 0).
T6 and T7 ran the whole durable binary (226 tests); the others the 36 rename-section tests.

| id | mutant | verdict | killed by / why not |
|---|---|---|---|
| r3-T1 | no directory fsync before the slot file is made | KILLED | the crossing-fails pair, a_slots_open_after_a_crossing_open_that_failed_survives_a_power_cut (+_under_a_cipher) |
| r3-T2 | the pre-make fsync skipped where the seam is set | KILLED | the same four |
| r3-T3 | the rename open's record of "." dropped | KILLED | the (d) tests (record held to the traced calls) |
| r3-T4 | the rename open's directory fsync before the pending fsyncs | SURVIVED | an order of two fsyncs that both precede every removal and the open's first write; no state tells them apart. Harmless, as expected |
| r3-T5 | make_slots' seam fails after the fsync | KILLED | the crossing-fails pair (fsync count) |
| r3-T6 | a rename publish's error from manifest_write stops the seal queue (every caller) | SURVIVED (226 tests) | check-the-tests: no test fails a merge's or a promotion's rename publish; open item, not built |
| r3-T7 | the publish hook never pruned when its owner is gone | SURVIVED (226 tests) | check-the-tests: a seam's bookkeeping; a dead owner's hook is never asked by a live store in any test |
| r3-O1 | the open passes settle=false | KILLED | the four as T1 |
| r3-O2 | the pre-make fsync moved after the make | KILLED | the crossing-fails pair (order in the trace) |
| r3-O3 | settle flipped | KILLED | the four as T1 |
| r3-O4 | the pre-make fsync only when a file was made already | KILLED | the four as T1 |
| r3-O5 | the pre-make fsync's error ignored | SURVIVED | no seam fails that fsync (the seam fails the one after the make); a device error there is not built. Open item |
