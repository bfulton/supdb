# open-order mutants

## Repair round 10 (the commit step's gate was interrupted; the tree unchanged from round 3)

The commit step could not say the reviewer's round-3 items were held by a mutant on the tree it
gated, so the mutants that hold them were run again on that tree (byte for byte keep/*.fixed,
sha256 checked before and after), unprivileged (SUPDB_TEST_UNPRIVILEGED=1, so the kernel's
page flags are not read and only the call trace can hold the fsyncs): mut8.py ->
mut-results-r10.jsonl, logs m-r10-*.log.

| mutant | what | verdict |
|---|---|---|
| Z6u-sync-deleted-push-kept | the pending file's `vault::sync_file` deleted, its name still pushed into the open's record | KILLED: acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| Z2u-only-first-file-before-slot | only the first pending file synced before the slot write, the rest after it | KILLED: an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| W2-listing-not-device | the missing-name check reads the listing instead of `vault::on_device` | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher |
| W6-exists-not-device | the missing-name check reads `vault::exists` instead of `vault::on_device` | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher |

## Repair round 3 (the tree as left)

Runner: mut7.py, from keep/*.fixed (round 3), every test run with
SUPDB_TEST_UNPRIVILEGED=1 (the kernel's page flags not read, as in a gate that is not
root). Each mutant written into src/db.rs, durable rebuilt, the unit's 17 tests run by
exact name, and for W1/W4/W8 the whole durable and db binaries too; the tree restored
byte for byte (sha256 checked) after each. Verdicts in mut-results-r3.jsonl (written
per mutant as it came), logs m-r3-*.log, diffs m-r3-*.diff.

| mutant | what | verdict |
|---|---|---|
| W1-one-key-skipped | unnamed_refusals: `b.keys() == 0` to `<= 1` | KILLED: a_flushed_run_is_not_lost_..._after_its_temp_retired (new variant: a run of one key), durable(all). (The new index test also failed in that run, at its premise: a race of the test's own, fixed below, not W1.) |
| W2-listing-not-device | open_refusals: `!vault::on_device(..)` to `!on_disk.contains(n)` | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher |
| W3-absent-ord-skipped | pending loop: an ord file not there skipped | SURVIVED: equivalent; the hardening was not adopted (below) |
| W4-covered-bound-ge | unnamed_refusals: `end > sealed` to `end >= sealed` (also the fifth extra mutant: the same line) | KILLED: a_flushed_run_is_not_lost_..., a_refused_open_leaves_the_log_files_its_replay_ended_before, durable(all): the run's segment there is named at end == the older slot's covered sequence |
| W6-exists-not-device | open_refusals: `vault::on_device` to `vault::exists` | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher |
| W7a-segment-synced-as-ord | pending loop: the segment's fsync made of its ord file, the record unchanged | KILLED: acknowledged_commits_survive_..., ..._makes_its_durable_index_durable, ..._several_pending_pieces_makes_each_durable (the call trace: no `sync_file` of the segment before the slot's open) |
| W7b-ord-synced-as-segment | pending loop: the ord's fsync made of the segment, the record unchanged | KILLED: ..._makes_its_durable_index_durable, ..._several_pending_pieces_makes_each_durable |
| W8-holders-runs-only | unnamed_refusals: holders from the runs' segments only (named segments dropped) | KILLED: the_windows_a_torn_publish_leaves_open_through_the_slot_before (merge case), durable(all), db(all) |
| W9-unnamed-after-later-wals | unnamed_refusals moved after the removal of log files after the replay's end (and after the cipher sweep and make_slots), before the orphan sweep | KILLED: a_refused_open_leaves_the_log_files_its_replay_ended_before |
| Z6u-sync-deleted-push-kept | the pending file's fsync deleted, its record kept; unprivileged | KILLED: acknowledged_commits_survive_..., ..._makes_its_durable_index_durable, ..._several_pending_pieces_makes_each_durable (the call trace) |
| Z2u-only-first-file-before-slot | only the first pending file fsynced before the slot, the rest after; unprivileged | KILLED: ..._makes_its_durable_index_durable, ..._several_pending_pieces_makes_each_durable |

What was added or decided for them:
- W1: a_flushed_run_is_not_lost_..._after_its_temp_retired gained a variant with a run of
  one key; it passes on the fixed tree (it is the same refusal as the first variant,
  which failed on the old engine; this variant was not run on the old engine).
- Z6/Z2 and W7a/b (the blocking item): the open's fsync of a pending segment and its ord
  file goes through `vault::sync_file`, which records `("sync_file", path)` in the
  process's call trace once the fsync returns (one relaxed load beside the syscall, the
  open's pending branch only). The test requires, for each pending segment and each ord
  under ord_durable, a `sync_file` of it before the `open_slot` of the slot written.
  `SUPDB_TEST_UNPRIVILEGED` makes the tests' page-flag reader answer None, so a root run
  shows what the gate without root holds: Z6 and Z2 are killed there (before this round
  Z6 survived without root).
- W3 (the suggested hardening: skip an ord file that is absent): not adopted, since no
  state reaches it. The open's check that a pending segment reads back whole
  (`pending_whole`, `Seg::open` -> `Seg::open_ord`) composes a missing or torn ordered
  index and writes it synced before the pending loop runs, so the loop always finds the
  file. The guard an_open_adopts_a_pending_segment_whose_durable_index_a_power_cut_took
  (ord_durable, the ord files removed after the kill) passes on the tree without the
  hardening; with it, W3 is the same code in every reachable state. The ord_durable
  tests still require each present ord in the open's record and in the call trace.
- The guard first removed the ord files in the store the forgotten Db's threads still
  ran in, and failed at its premise 4 runs in 30 (the open had taken the slot before,
  both slots naming nothing, the segment swept and the log replayed: every key read).
  Read as the test's: the live process's landing goes on opening the segment it
  published and may compose the index the test removed while the open reads it; not
  confirmed by a trace. The test copies the store as it stands after the kill and works
  on the copy: 0 in 40 (r3-ordloop2.log).

## Repair round 2 (the tree as left)

Runner: mut5.py (round-1 repair tree + the slot in the open's record), then mut6.py
(the tree as left: + the vault's call trace and the windows cases run-torn/empty).
Each mutant written into src/db.rs from keep/src_db.rs.fixed, durable rebuilt, the
unit's 16 tests run by exact name, and for Z3/Z4/Z8 the whole durable binary and the
whole db binary too; the tree restored byte for byte (sha256 checked) after each.
Verdicts in mut-results-r2.jsonl (mut5) and mut-results-r2b.jsonl (mut6, the
verdicts that count), logs m-r2-*.log / m-r2b-*.log, diffs *.diff.

| mutant | what | mut5 (before this round's tests) | mut6 (the tree as left) |
|---|---|---|---|
| Z1-dir-sync-after-slot | the directory's fsync and its record moved after the slot write | KILLED by the slot's place in open_synced (new assertion) | KILLED: acknowledged_commits_survive_..., an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| Z2-only-first-file-before-slot | only the first pending file fsynced before the slot, the rest (ords, later pieces) after it | KILLED (same) | KILLED: ..._makes_its_durable_index_durable, ..._several_pending_pieces_makes_each_durable |
| Z3-unopened-unnamed-refused | refuse where an unnamed segment does not open | KILLED by durable(all) only (other units' power-cut tests: a run's segment torn under its piece name) | KILLED: the_windows_..._slot_before (new case run-torn: "run-torn: the window refused: ... does not open"), durable(all) |
| Z4-empty-unnamed-not-skipped | `if b.keys() == 0 { continue; }` deleted | "KILLED" by durable(all) through an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes, which failed with "names seg-... missing from the directory": not Z4's line (see below); no test built the state | KILLED: the_windows_..._slot_before (new case empty: "attempt to subtract with overflow" at the last-key read), durable(all) |
| Z5-missing-check-pcs-only | the missing-name check for pcs- names only | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, ..._under_a_cipher | same |
| Z6-sync-deleted-push-kept | `file.sync_all()?` deleted, the record kept | KILLED (the kernel's page flags; root only: survives without root) | same |
| Z7-seam-after-sync | fail_open_pending_sync checked after the real fsync | SURVIVED | SURVIVED: equivalent for the property the seam is for (below) |
| Z8-next-id-par-only | next_id from par- names only | SURVIVED (durable+db whole) | SURVIVED: equivalent in every state a crash leaves (below) |
| Z9-missing-check-first-only | the missing-name check on live.first() only | KILLED: an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file | same |
| Z10-dir-sync-skipped-for-one | the directory fsync skipped for a single pending segment, its record kept | SURVIVED | KILLED: acknowledged_commits_survive_..., ..._makes_its_durable_index_durable (the call trace: no sync_dir of the store's directory before the slot's open) |

What was added for them:
- Z1/Z2 (the blocking item): the open pushes the slot it writes into its record
  (`open_synced`, after `write_shape` returns, which fdatasyncs it) and the test holds
  every pending segment, its ord file under ord_durable, and "." to a place before it;
  it also asserts the open adopted the segments (a premise, no longer an `if`).
- Z10/Z6 class (the record kept, the call skipped): `vault::trace_io` /
  `Db::trace_dir_io`, a process-wide record of every directory fsync that returned and
  every manifest slot opened for writing, each behind one relaxed load beside the
  syscall (off unless a test turns it on); the test reads it for its own directory and
  requires a directory fsync before the open of the slot it writes. A file's fsync is
  still held only by the kernel's page flags (root).
- Z3: windows case run-torn (the run's piece name and its temp name both with the
  superblock page zeroed: a power cut before the close's fsync; the temp's records,
  synced at each commit, recover the run).
- Z4: windows case empty (every key of the partition deleted, the merge held at its
  landing: an unnamed output of no record beside the slot before).

Z7 argued: the seam stands for "the open's fsync of a pending file answers an error";
the property its test holds is that no slot names the segment durable and the slots
are as found. Whether the injected error comes before or after a real fsync, the open
answers it before the slot write, so both placements hold the property; a failure
after the data reached the device is still a failure the open must not name durable.
Kept before the fsync (no fsync issued for a failure the test pretends).

Z8 argued (not shown): the id gate skips unnamed segments older than a named
segment. Under Z8 those older than every named partition but newer than a named piece
are checked by their last key instead. In a crash state such a segment is a job's
input: a partition merge's inputs are older than the partitions it wrote (so skipped
under both), and a piece merge carries every key and tombstone of its inputs into its
output (db.rs, "A piece merge carries it"), so the last key is held and nothing is
refused under either; a promotion's old name is older than its new partition name.
No state was built where the two differ.

Z4's first "kill" (mut5): an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes
(delete-max's test) failed once in the whole binary under the mutant, with the open
refusing "manifest.alt names seg-00000000-...sup, which is missing from the
directory". Z4's line was not reached (that refusal is open_refusals'). The test copies
the live store with only the second seal held while the run's own seal lands; a copy
that catches the newest slot mid-write and the older slot's segment already promoted
away is no crash state. The old engine fails the same copy (it opens the older slot's
missing segment and errors). It passed 20 of 20 runs alone on the tree as left. Not
fixed here (another unit's test); reported.


## Repair round 1


Runners: mut2.py (the reviewers' extra mutants on the round-3 tree, results mut-results-r1.jsonl, logs m-X*.log), mut3.py (every mutant again on the tree after the first repairs, mut-results-r1b.jsonl, logs m-r1b-*.log), mut4.py (the key-check mutants on the last-key shape, mut-results-r1c.jsonl, logs m-r1c-*.log). Each writes its mutant into src/ from keep/*.fixed, runs the unit's 16 tests by exact name (names.txt + names-r1-new.txt), and for suites marked so the whole durable binary (and the db binary), then puts the fixed file back and checks its sha256; each verdict is appended as it comes.

### The reviewers' extra mutants

| mutant | first run (round-3 tree) | what was done | final |
|---|---|---|---|
| X1 the replayed log ignored as a holder (both of the reviewers' items 1 and 8) | SURVIVED every unit test and the whole durable binary | the log arm taken out of Db::unnamed_refusals (no crash state puts a run's or a merge's keys in the log alone; a run's commits never go to it in the clear), and dropped from every doc | n/a (arm gone); the Some(1999) variant of a_flushed_run_... now holds the log's not being a holder: with a last-key check, a log arm would sweep it (reasoned; the arm put back was not run as a mutant) |
| X2 every unnamed segment a holder (runs filter dropped) | KILLED: a_flushed_run_..., a_refused_open_leaves_the_log_files_... | none | KILLED (r1c) |
| X3 the pending fsync under a cipher too | KILLED by the new an_open_under_a_cipher_adopts_a_pending_segment_and_opens_again (guard: passes on the old engine, before-r1.log) | test added | KILLED (r1b) |
| X4 the on-device check gated on a fall back (run only where fewer than two slots read) | KILLED by the new an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file (fails on the old engine at its words: before-r1.log) | test added | KILLED (r1b) |
| X5 the pending fsyncs moved after the slot write | SURVIVED | seam Options::fail_open_pending_sync (checked once per pending file, beside the open's fsync, recovery only) and test an_open_whose_pending_fsync_fails_names_nothing_durable | KILLED (r1b); X10 (the seam ignored) KILLED by the same test |
| X6 spares beyond the one kept never removed | SURVIVED unit + durable + db | guard test an_open_that_recycles_no_log_removes_the_spare_it_finds | KILLED (r1b) |
| X7 log files after a loose end kept | SURVIVED unit + durable | guard test an_open_removes_the_log_files_after_the_end_it_replayed_to | KILLED (r1b) |
| X9a next_id over on_disk | KILLED: a_flushed_run_..., a_refused_open_..., durable(all) | none | KILLED (r1c) |
| X9b next_id from .min() | SURVIVED | window case "inputs-old" in the_windows_a_torn_publish_leaves_open_through_the_slot_before: a merge's inputs left beside its durable outputs, with an older partition the merge left alone, so the inputs' ids are above the lowest named id | KILLED (r1c): the_windows_..., durable(all) |

### What the repairs set off

| mutant | verdict | what was done |
|---|---|---|
| O6 last key unchecked / O7 first key unchecked | both SURVIVED once the log arm was gone (r1b) | in every state one damaged slot leaves, a run's two ends are both unheld and a merge's outputs' both held, so reading both was a check nothing could reach: the first-key read taken out, the last key kept (a run's keys sit above every other, so a merge that folds a run in shows it at its last key) |
| Y1 the first key read instead of the last | SURVIVED (r1c) | equivalent by the argument above, which is reasoned and not tested: no test, and no state found, tells the two ends apart |
| Y2 the key check skipped wherever any holder opened (`holders.is_empty()`) | SURVIVED the first r1c run: every refusal test had nothing named beside the run | variant below=true of a_flushed_run_is_not_lost_...: a segment flushed before the run, named by the slot before (partition_on_flush, flush_schedules, drain_names_partition off, so nothing is renamed under it) | KILLED (r1c) |

### Every earlier mutant on the repaired tree (r1b; key-check ones again in r1c)

M1-M13, O1-O3, O5, O8-O13: all KILLED (r1b; M1, M3, M4, O1, O2, O3, O5 again in r1c on the last-key shape, all KILLED). M10/M13's patterns were moved to the open's new fsync lines (`file.sync_all()?`). M2 is now killed by the new newest-slot test as well.


## Rounds before the repair

Runner: mut.py (each mutant written into src/db.rs or src/db/vault.rs from keep/*.fixed, durable rebuilt, the unit's 11 tests run by exact name (M8 also tests/db.rs a_store_from_before_manifests_opens_from_its_segment_names), tree restored byte for byte and its sha256 checked after each). Logs m-<id>.log, diffs m-<id>.diff.

## Final round (round 3, the tree as left): 25 of 25 killed

| mutant | what | killed by |
|---|---|---|
| M1-sweep-before-checks | report: the orphan sweep run before the slot's checks | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired, a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file, a_refused_open_leaves_the_log_files_its_replay_ended_before |
| M2-absent-name-check-deleted | report: the check that every name the slot lists is in the directory deleted | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher |
| M3-keys-check-deleted | report: the unnamed-later-segment check deleted (every unnamed segment skipped) | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired, a_refused_open_leaves_the_log_files_its_replay_ended_before |
| M4-id-off-by-one | report: id compare '<' to '<=' (the '>' for '>=' of the report) | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired, a_refused_open_leaves_the_log_files_its_replay_ended_before |
| M5-no-slot-check-deleted | report (both-slots): the no-readable-slot check deleted | KILLED: an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file |
| M6-checks-after-sweep-and-make-slots | report (both-slots): the checks placed after the cipher sweep, make_slots and the orphan sweep | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file, an_open_with_no_readable_slot_beside_envelopes_refuses_under_a_cipher |
| M7-par-only | report (both-slots): par- only | KILLED: an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file |
| M8-refuse-whenever-no-slot | report (both-slots): refuse whenever no slot stands beside any segment | KILLED: db:a_store_from_before_manifests_opens_from_its_segment_names |
| M9-no-dir-sync | report (pending): the open fsyncs the segments but not the directory | KILLED: acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| M10-no-segment-sync | report (pending): the open fsyncs the directory but not the segments (nor records them) | KILLED: acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| M11-first-pending-only | report (pending): only the first pending name fsynced | KILLED: an_open_that_adopts_several_pending_pieces_makes_each_durable |
| M12-ord-omitted | report (pending): the ordered index omitted under ord_durable | KILLED: an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| M13-recorded-not-synced | report (pending): names recorded in open_synced without the fsync | KILLED: acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| O1-seal-segments-held-too | own: the end<=covered restriction dropped (seal segments held to their keys too) | KILLED: the_windows_a_torn_publish_leaves_open_through_the_slot_before |
| O2-covered-bound-off-by-one | own: end > sealed to end >= sealed | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired, a_refused_open_leaves_the_log_files_its_replay_ended_before |
| O3-unnamed-check-off | own: the unnamed-segment check never called | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired, a_refused_open_leaves_the_log_files_its_replay_ended_before |
| O5-runs-hold-nothing | own: a run's segment beside its temp name holds no key | KILLED: the_windows_a_torn_publish_leaves_open_through_the_slot_before |
| O6-last-key-unchecked | own: only the first key held to account | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired |
| O7-first-key-unchecked | own: only the last key held to account | KILLED: a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired |
| O8-cipher-on-device-always | own (cipher): vault::on_device answers true for every adopted name | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher |
| O9-cipher-holds-no-envelopes | own (cipher): holds_envelopes answers false | KILLED: an_open_with_no_readable_slot_beside_envelopes_refuses_under_a_cipher |
| O10-cipher-sweep-before-checks | own (cipher): the vault's sweep moved back before the checks | KILLED: a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, an_open_with_no_readable_slot_beside_envelopes_refuses_under_a_cipher |
| O11-pending-sync-off | own (pending): the pending block gated off | KILLED: acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable, an_open_that_adopts_several_pending_pieces_makes_each_durable |
| O12-later-wals-removed-in-replay | own: log files after a loose end removed inside the replay, before the checks | KILLED: a_refused_open_leaves_the_log_files_its_replay_ended_before |
| O13-make-slots-before-checks | own: manifest_make_slots before the checks | KILLED: an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file |

## Earlier rounds, and what they changed

- Round 1 (mut-results-round1.jsonl, stopped after O12) and round 2 (mut-results-round2.jsonl) ran on the shapes before.
- Round 1's O3 (the fall-back gate on at every open) was killed by tests/durable.rs a_sync_with_a_run_open_syncs_the_log_before_the_run and tests/db.rs a_crash_before_the_manifest_lands_keeps_the_pre_merge_store: the first version's log check (an unnamed segment with writes past the slot's covered sequence must be replayed by the log) refused a whole seal segment beside a log a power cut had cut, which a buffered store's torn landing publish leaves too. That check was taken out, the key check restricted to segments within what the slot covers, and a buffered window case added to the_windows_a_torn_publish_leaves_open_through_the_slot_before (killing O1 above).
- Round 2: O3 (gate always on) SURVIVED the whole durable and db binaries once the restriction was in: the gate was a condition nothing could reach, and was taken out (the check runs at every open). O12 SURVIVED: test a_refused_open_leaves_the_log_files_its_replay_ended_before added (kills it above). O14 (the reading of an unnamed segment without its key-section check) SURVIVED: within the check's domain no crash state leaves a torn segment (a merge's outputs are synced before their rename; a run's segment stands beside its temp name, which holds it itself), so the explicit options were replaced by Blob::open's defaults and the mutant dropped as equivalent for every crash state; damage to such a segment as well as to the slot is not tested.
- Round 2 earlier rows: M1-sweep-before-checks=KILLED, M2-absent-name-check-deleted=KILLED, M3-keys-check-deleted=KILLED, M4-id-off-by-one=KILLED, M5-no-slot-check-deleted=KILLED, M6-checks-after-sweep-and-make-slots=KILLED, M7-par-only=KILLED, M8-refuse-whenever-no-slot=KILLED, M9-no-dir-sync=KILLED, M10-no-segment-sync=KILLED, M11-first-pending-only=KILLED, M12-ord-omitted=KILLED, M13-recorded-not-synced=KILLED, O1-seal-segments-held-too=KILLED, O2-covered-bound-off-by-one=KILLED, O3-fallback-gate-always=SURVIVED, O4-fallback-gate-never=KILLED, O5-runs-hold-nothing=KILLED, O6-last-key-unchecked=KILLED, O7-first-key-unchecked=KILLED, O8-cipher-on-device-always=KILLED, O9-cipher-holds-no-envelopes=KILLED, O10-cipher-sweep-before-checks=KILLED, O11-pending-sync-off=KILLED, O12-later-wals-removed-in-replay=SURVIVED, O13-make-slots-before-checks=KILLED, O14-fallback-reads-whole-without-index-check=SURVIVED
