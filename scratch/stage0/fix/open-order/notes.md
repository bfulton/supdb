# open-order (slot-damaged-rollback, both-slots-unreadable, manifest-pending-rewrite's open half)

STATUS: repair round 10 DONE: the unit gate (sh scripts/check.sh build test lint) exit 0 on the tree as left (gate-r10.log, gate-r10.status); uncommitted in the working tree, which is round 3's keep/*.fixed byte for byte.
FIXED COPIES (the round-3 repair, the ones that count): keep/src_db.rs.fixed,
keep/src_db_vault.rs.fixed, keep/tests_durable.rs.fixed, keep/CLAUDE.md.fixed,
keep/ROADMAP.md.fixed (sha256 in keep/sha256.txt). keep/r2/ holds the round-2 copies,
keep/r1/ the round-1 repair's, keep/r3/ the original round-3 copies before that. If
mut-current.txt exists, a mutant is in src/: copy keep/*.fixed back.
keep/src_db.rs.seamonly + keep/src_db_vault.rs.head are the BEFORE engine (HEAD + the
open_synced seam), not the fixed one. Step log: notes-progress.md.

## What was wrong
- The open listed the segments and removed every one its chosen slot did not name
  (and, under a cipher, every envelope neither slot named) before deciding whether
  that slot could be the store. With the newest slot damaged it fell back to the
  slot before, swept what only the damaged slot named -- a flushed run whose temp
  name had retired, its only copy -- and answered Ok with every flushed key absent;
  with the older slot naming a merge's unlinked inputs it swept the merge's output
  and then refused with a NotFound further in.
- With no readable slot (both empty or missing) beside par-/pcs- files, the open took
  the pre-manifest scan (seg- names only) and removed every partition and piece; under
  a cipher the vault's sweep removed every envelope.
- Under Options::manifest_pending the open took a pending segment that read back whole
  from the page cache after a kill and wrote a slot naming it durable, covering the
  log frames that held its writes, with no fsync of the segment, its ordered index or
  the directory: a power cut after that reopen lost the segment and the log was
  skipped past it.

## What changed
src/db.rs, Db::open: the order is now read slots -> pending skip -> choose -> the
slot's fdatasync (slot-choice's) -> list -> CHECKS -> the log replayed (moved up; its
removals of spare logs and of log files after a loose end deferred) -> the unnamed
check -> then every mutation: the vault's sweep, manifest_make_slots, spares and later
log files removed, the orphan sweep, the pending/upgrade slot write (now preceded,
for a store in the clear, by the fsync of every pending segment, its ord file under
ord_durable, and the directory), run recovery, ord/z sweeps, the log cut.
- Db::open_refusals: no slot read and a par-/pcs- file (or under a cipher any segment
  envelope, vault::holds_envelopes) -> refuse; a name the slot lists not on the device
  (vault::on_device: the file, or under a cipher the envelope the slot named) -> refuse.
- Db::unnamed_refusals (plain only): for each segment the slot does not name with
  id >= the slot's next id and end_seq <= the slot's covered sequence (a merge's output
  or a run's segment; a seal's segment is always past covered), that opens, its LAST
  key must be held by a named segment or a segment beside its run temp name (a run's
  own among them); otherwise refuse. (Round 1 repair: the replayed log is no longer a
  holder, and the first key is no longer read -- see below.)
- manifest_read_all doc; vault.rs: Vault::holds_segments/on_device, vault::on_device,
  vault::holds_envelopes. Seam: Shared::open_synced / Db::open_synced_files (a Vec set
  at construction; nothing on any path but the open's pending branch pushes to it).
- Words: "no manifest slot can be read", "is missing from the directory", "holds keys
  nothing the slot names", each ending "nothing was removed".

## Why that shape (and what was tried and taken back)
- First shape also refused an unnamed segment with writes past the slot's covered
  sequence that the replay did not reach (the "log retired behind the damaged slot"
  case). Mutant round 1 (gate on at every open) showed tests/durable.rs
  a_sync_with_a_run_open_syncs_the_log_before_the_run building a whole seal segment
  beside a log a power cut emptied: a buffered store whose landing publish is torn
  leaves those files, and must open. Nothing in the files tells that from damage past a
  retired log, so the check was taken out (ROADMAP item; the reproduction is
  log-retired-test.rs, verified failing on the final tree: log-retired-now.log) and the
  key check restricted to end_seq <= covered.
- The checks first ran only when a slot failed its check (a fall back); with the
  restriction the gate survived the whole durable and db binaries as a mutant, so it
  was taken out: the check runs at every open (cost: only where an unnamed segment
  newer than the slot's names and within its covered sequence stands, i.e. after a
  crash inside a merge's window, or damage).
- The pending fsync covers the ord file only under ord_durable: an ord file is
  regenerated at the open when torn, so syncing it otherwise is unobservable; the test
  asserts the open's record for it, which the kernel check holds to the device.
- Under a cipher the pending segments' envelopes are written and synced (file and
  directory) by manifest_image/persist before the pending slot is written, so the open
  syncs nothing there (read, not tested).

## Tests (tests/durable.rs, section "open-order"), each held to a mutant
- a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired
  (3 variants: plain, the run's first key and its last key written again into the log):
  refusal by the keys words, directory unchanged, store whole after the slot is put back.
- a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed (+cipher):
  the report's test 2, refusal by the "missing from the directory" words (the NotFound
  further in carries other words), directory unchanged, repaired store whole.
- a_refused_open_leaves_the_log_files_its_replay_ended_before: buffered, three log files,
  the first cut, damaged slot over a flushed run: the third log file is still there.
- the_windows_a_torn_publish_leaves_open_through_the_slot_before (guard, passes before):
  seal / run / merge / merge inputs left beside durable outputs / buffered seal with the
  log cut, each copied with a job held and the next publish torn: opens with every write.
- an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file
  (emptied, removed, pieces alone), an_open_with_both_slots_zeroed_... (control, passes
  before), an_open_with_no_readable_slot_beside_envelopes_refuses_under_a_cipher.
- acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment,
  an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable,
  an_open_that_adopts_several_pending_pieces_makes_each_durable: kill between a seal's
  pending slot and its fsync, open, power cut (unsynced files torn, names removed where
  no directory fsync came after), reopen reads every key; the kernel's page flags
  (/proc/kpageflags, root only) hold each file the open says it fsynced to the device.
Logs: before.log (final tests on the seam-only engine: 9 fail at their assertions, the
2 guards pass), after-mine4.log, after.log (whole suite --no-fail-fast, exit 0),
neigh-*.log (durable, db, cipher, fuzz, segwriter, lib, on the round-2 tree), clippy.log,
fmt.log, mutants.md, mut-results.jsonl (+ round1/round2).

## Repair round 1 (two reviewers, one blocking finding: the docs promised a check of
every key where the code read the first and last)
- Docs: Db::open's doc, unnamed_refusals' doc, CLAUDE.md's Crash discipline sentence and
  its Shapes entry ("A sweep made before the choice it was for"), and ROADMAP's item
  (now "A fall back held by a segment's last key") say the one key that is read.
- Extra mutants (mutants.md): X1 (the log as a holder ignored) survived every test and
  the whole durable binary: the log arm was taken out (nothing a crash leaves puts a
  run's or a merge's keys in the log alone; a run's commits never go to it in the
  clear), with `mem` no longer passed. With it gone O6/O7 (one end unread) both survived:
  in every state one damaged slot leaves the two ends are held alike, so the first-key
  read was taken out and the last kept (Y1, the first instead of the last, survives as
  an equivalent mutant; reasoned, not tested). Y2 (any holder opened = held) survived:
  the run test got a variant with a segment the slot before names (below=true).
  X4 -> new test an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file
  (fails on the old engine at its words, before-r1.log). X3 -> new guard
  an_open_under_a_cipher_adopts_a_pending_segment_and_opens_again (passes on the old
  engine). X5 -> seam Options::fail_open_pending_sync (#[doc(hidden)] pub, read once per
  pending file beside the open's fsync, recovery only) + test
  an_open_whose_pending_fsync_fails_names_nothing_durable (cannot be run on the old
  engine: the seam does not exist there). X6 -> guard
  an_open_that_recycles_no_log_removes_the_spare_it_finds; X7 -> guard
  an_open_removes_the_log_files_after_the_end_it_replayed_to (both pass on the old
  engine, which removed those files as well, earlier). X9b -> window case "inputs-old".
- Every earlier mutant rerun on the repaired tree: all killed (mut-results-r1b/r1c).
- Logs: before-r1.log (2 new tests on the seam-only engine), after-r1b-mine.log (16 of 16),
  r1-below.log, after-r1.log + clippy-r1.log + whole-r1.status (whole suite), fmt-r1.log.

## Repair round 2 (two reviewers; one blocking: nothing held the order of the pending
adoption's directory fsync against the slot write)
- src/db.rs: the open pushes the slot it writes into its record (`open_synced`, after
  `write_shape`, which fdatasyncs it); the record's docs say it is in the order the
  fsyncs returned. Recovery only (the pending/upgrade branch of the open).
- src/db/vault.rs: `trace_io` (and `Db::trace_dir_io`, #[doc(hidden)] pub): a
  process-wide record of every directory fsync that returned (`sync_dir`) and every
  manifest slot opened for writing (`open_slot`), each behind one relaxed load of a
  static beside the syscall the call makes; off unless a test turns it on.
- tests/durable.rs: a_pending_segment_adopted_by_an_open asserts the adoption (premise),
  every pending segment, its ord file under ord_durable and "." before the slot in the
  open's record, and a directory fsync of the store's directory before the open of the
  slot written, read off the call trace. the_windows_..._slot_before gains run-torn (a
  run's piece name and temp name with the superblock page zeroed: the power cut before
  the close's fsync) and empty (a merge's output of no record).
- Extra mutants Z1-Z10 (mutants.md): all killed but Z7 (seam placement, equivalent for
  the property) and Z8 (next_id over par- only, equivalent in every crash state;
  argued, not shown). Z10 and Z6 class needed the call trace (Z6 still only as root).
- Docs: CLAUDE.md, the pending-arm passage under "A replay that builds the thing
  without the last step of the live path" amended (a record of each end apart holds
  that each happened and not the order; a record the code under test keeps is its
  word). ROADMAP: "The open's directory fsync held on a device".
- Seen, not this unit's: an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes
  (delete-max) failed once in a whole-binary run under mutant Z4 with "manifest.alt
  names seg-... missing from the directory": it copies a live store while an unheld
  seal lands, and a copy taking the newest slot mid-write is no crash state; 20 of 20
  alone on the tree as left. Reasoned to fail the same way on the old engine (NotFound
  further in), not run there.
- Logs: r2-build.log, r2-pend.log, after-r2-mine.log (16 of 16), mut-results-r2*.jsonl,
  fmt-r2.log, fmtsh-r2.log, after-r2.log + clippy-r2.log + whole-r2.status (whole suite).

## Repair round 3 (two reviewers; blocking: the docs claimed the unnamed-segment
refusal under a cipher too; a pending file's fsync held only by root-only page flags)
- Docs: Db::open's doc and CLAUDE.md's Crash discipline sentence say the third refusal
  is made in the clear, and that under a cipher such an envelope has its key in no slot
  the open can read, is swept, and a fall back that is not refused answers without the
  writes only the damaged slot named (ROADMAP items cited). The CLAUDE.md pending-arm
  passage (under "A replay that builds the thing without the last step") amended: the
  page flags are root-only, so each file's fsync goes through a call that records it.
  ROADMAP "The open's directory fsync held on a device" restated to match.
- src/db/vault.rs: `vault::sync_file(path)`: open, fsync, then record `sync_file` in the
  call trace (one relaxed load beside the syscall). src/db.rs: the pending loop uses it
  (seam check before it, as before); trace docs updated. Open's recovery branch only:
  nothing on any success path.
- tests/durable.rs: a_pending_segment_adopted_by_an_open requires a `sync_file` of each
  pending segment and each ord under ord_durable before the slot's `open_slot`;
  `os_page_unwritten` answers None under SUPDB_TEST_UNPRIVILEGED; the run test gained a
  one-key run variant (W1); new guard
  an_open_adopts_a_pending_segment_whose_durable_index_a_power_cut_took (W3; passes
  without the hardening, which was not adopted: pending_whole composes a missing ord,
  synced, before the loop). That guard first raced the forgotten Db's threads (4/30
  premise failures) and now works on a copy (0/40).
- Extra mutants W1-W9, Z6u, Z2u (mutants.md, mut-results-r3.jsonl): all killed but W3
  (equivalent, see above). W5 is the same line as W4.
- Logs: r3-build*.log, r3-pend.log (pending tests root and unprivileged, before the new
  guard), r3-before.log (the two new/extended tests on the tree before W3's decision:
  both pass, i.e. neither is a before-the-fix failure; they are guards), r3-ordloop*.log,
  fmt-r3.log, after-r3.log + clippy-r3.log + whole-r3.status (whole suite),
  after-r3-mine.log (the unit's 17 tests, unprivileged).

## Repair round 10 (the commit step's gate never reached a verdict)
- The commit step's gate was interrupted in its build group; nothing in the tree was wrong
  that it had found. The tree was checked equal to keep/*.fixed (sha256) and not changed.
- The reviewer's round-3 items (the cipher exception in Db::open's doc and CLAUDE.md; the
  pending file's fsync through vault::sync_file in the call trace) are in the tree, and the
  mutants that hold them were run again on this exact tree, unprivileged: Z6u, Z2u, W2, W6
  all KILLED (mut-results-r10.jsonl, mutants.md "Repair round 10").
- The history the task names (f8d130a8, 0aa41df7, 38378d1d as the latest) is older than the
  branch: HEAD 682b2c09 carries the later units (log-at-open a424903d, sync-run 10898a94,
  seal-sync 02b7a440, slot-choice 2e8b173d) and the design-notes commit. This unit is built
  on that HEAD; nothing to change.
- Gate: sh scripts/check.sh build test lint with TMPDIR=stage0/tmp/open-order, as root:
  exit 0, "all checks passed: build test lint" (fmt.sh --check inside lint); every one of
  the unit's 17 tests and db's a_store_from_before_manifests_opens_from_its_segment_names
  ok in its log. The TMPDIR (and the commit step's empty tmp/open-order-gate) removed.

## Restated tests: none. No existing test changed (the unit's own tests were extended).

## Not built / left out
- The silent rollback past a damaged durable slot whose log retired (ROADMAP item);
  under a cipher, a fall back past a damaged slot sweeps envelopes whose key was in it
  (ROADMAP item); the last-key check misses a segment whose last key the slot's
  segments hold and whose other keys they do not, a run whose id predates a merge the
  older slot names, or one whose end_seq is past the older slot's covered sequence
  (ROADMAP item). All media damage only.
- manifest-pending-rewrite's close half (Maint::stop) was seal-sync's and is not touched.

## Not verified
- No real power cut or device: every crash state is built by hand (copies, cuts, tears).
- The kernel check of what the open fsynced runs only as root (it ran here); without
  root the call trace holds each file's fsync (Z6 killed unprivileged, round 3), but
  nothing reads that the pages reached the device.
- Round 3's new tests are guards, not before-the-fix failures: the one-key run variant
  was not run on the old engine, and the lost-index guard passes with or without the
  W3 hardening. The guard's earlier premise failure is attributed to the forgotten
  Db's threads by reasoning, not by a trace.
- The directory fsync's effect is emulated (names removed where it did not happen); the
  call trace says the fsync was made and returned before the slot, not that the device
  kept the entries.
- The windows cases run-torn and empty were not run on the old engine (guards).
- Mutants of earlier rounds were not rerun on the round-2 tree (the engine change is the
  record push and the trace; the tests only gained assertions and cases).
- The before-state of the run test's later variants ("written again" twice, and the
  segment below): the test fails before at its first variant, so those were not shown
  failing on the old engine separately.
- Y1 (the first key read instead of the last) is argued equivalent, not shown: no state
  one damaged slot leaves was found where the two ends of such a segment are held
  differently.
- The pending fsync's failure test cannot be run on the old engine (its seam is new);
  the guards for spares and later log files pass on the old engine by design.
- No timing: the open's added per-named-segment stat and the moved replay were not timed.
step: commit-step gate running, log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/fix/open-order/gate.log
step: gate interrupted before a verdict (harness demanded the answer while build was compiling); NOT committed; commit message drafted at commit-msg.txt; tree still == keep/*.fixed
step: commit-step gate r11 running, log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/fix/open-order/gate-r11.log
step: committed 561ce7ef69c67817d27685a4fcad444832b22cc7; gate r11 EXIT 0
