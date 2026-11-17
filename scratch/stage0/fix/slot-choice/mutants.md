# slot-choice: mutants

Runner: mut.py (each mutant written into src/db.rs from keep/db.rs.fixed, the
durable binary run whole and tests/db.rs's two slot tests run; the fixed copy
put back and checked by sha256 after each). Verdicts: mut-results.jsonl, logs
m-<name>.log, diffs m-<name>.diff. Second run (S5, O2, O6, O7) after two tests
were added: mut-run2.log.

| mutant | what | verdict | killed by |
|---|---|---|---|
| S1 parity-in-publish | Maint::publish writes slot seq%2 | killed | a_publish_after_a_drains_owed_slot_... (+cipher), a_second_seal_landed_..., every_arm_... |
| S2 parity-in-publish-unsynced | publish_unsynced writes slot seq%2 | killed | a_publish_after_..., a_second_seal_..., an_open_that_takes_an_unsynced_slot_..._under_a_cipher, every_arm_... |
| S3a no-open-sync | open's fdatasync of the slot it read deleted (record kept) | killed (root only; see round 1, R1-S3a) | an_open_makes_the_slot_it_takes_durable (+cipher) (kernel's page flags, readable only by root) |
| S3b open-durable-by-parity | open marks slot seq%2 durable | killed | db::a_manifest_publish_alternates_two_slots_... |
| S3c open-block-gone | the open's sync and its durable choice both deleted | killed | an_open_makes_the_slot_it_takes_durable (+cipher) |
| S4 durable-at-issue | publish_unsynced marks its slot durable at the write | killed | a_publish_after_..., a_second_seal_..., an_open_that_takes_... (+cipher), every_arm_... |
| S5 owed-sync-not-landed | sync_owed_slot does not mark its slot durable | SURVIVED run 1; killed run 2 | a_drain_after_an_owed_slot_was_synced_keeps_that_slot (+cipher), added for it |
| O1 next-is-durable | SlotChoice::next returns the durable slot (flipped) | killed | the four watch tests, a_seal_whose_fsync_failed_is_not_named_durable_by_the_close, both db.rs slot tests |
| O2 pending-not-landed | the pending slot not marked durable after the helper's sync | SURVIVED run 1; killed run 2 | every_arm_that_writes_a_slot_keeps_the_last_durable_one (pending arms at full rounds, which rotate and retire the log) |
| O3 publish-not-landed | Maint::publish does not mark its slot durable | killed | a_publish_after_..., a_second_seal_..., every_arm_... |
| O4 upgrade-over-newest | the open's upgrade rewrite goes over the newest slot | killed | db::a_manifest_publish_alternates_..., db::a_store_from_before_the_pending_flags_... |
| O5 open-write-not-landed | the open's own publishes do not move the durable slot | killed | db::a_manifest_publish_alternates_..., db::a_store_from_before_the_pending_flags_... |
| O6 open-sync-after-sweep | the open's slot sync moved after the orphan sweep (swapped order) | SURVIVED in round 0; killed in round 1 (R1-O6) | an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+cipher) |
| O7 none-is-slot-zero | with no manifest read, the first write takes slot 1 | SURVIVED | none: equivalent, see below |
| O8 pending-whole-scan | pending_whole back to the whole-section scan | killed | every_arm_... (pending-cipher) |

Survivors (round 0):
- O6. WRONG in round 0, corrected in round 1. Round 0 called the order
  unobservable, on the claim that every slot the engine leaves unsynced names a
  superset of the durable slot before it. A process kill between a merge's or a
  promotion's slot write and its fdatasync makes that false: the slot in the
  page cache names the outputs, and the inputs are still on disk, because they
  are removed only after the publish. An open that sweeps before its sync, then
  meets a power cut or a failed sync, leaves the device's other slot naming
  removed files. The order matters, and round 1's seam test holds it
  (R1-O6, killed).
- O7. With no manifest read, neither slot holds a valid manifest, so either
  slot is a correct first write. This is an equivalent mutant, not a check.

## Round 1 (repair)

Runner: r1/mut1.py, which works from the fixed copies in r1/keep (sha256 in
r1/keep/sha256.txt). Each verdict is appended to r1/mut-results.jsonl as soon
as it is known. Logs and diffs are in r1/m-<name>.{log,diff}. There were three
runs:
- Run 1 was stopped early. Its partial results are in
  r1/mut-results-run1-partial.jsonl.
- Run 2 covered every mutant, after the slot-sync seam was added.
- Run 3 re-ran the survivors after the owed_only variant, the tear control and
  the second-seal tear test were added.

| mutant | what | verdict | killed by |
|---|---|---|---|
| R1-O6 open-sync-after-sweep | the open's slot fdatasync (and its seam) moved after the orphan sweep | killed | an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+cipher): the failed open had removed the files the older slot names |
| R1-S3a no-open-sync | the open's open_slot, seam check and fdatasync deleted, the durable choice kept; run as root, with no root-dependent test needed | killed | an_open_whose_slot_sync_fails_... (+cipher): Ok where the injected failure was due (also an_open_makes_the_slot_it_takes_durable (+cipher), but only as root) |
| R1-open-sync-only-if-pending | the open syncs its slot only when the slot had pending names | killed | an_open_whose_slot_sync_fails_... (+cipher), an_open_makes_... (+cipher) |
| R1-owed-landed-before-sync | sync_owed_slot records durable (and the seam's record) before its fdatasync | SURVIVED run 2; killed run 3 | a_slot_whose_owed_fdatasync_failed_is_not_the_one_a_publish_keeps_off (+cipher), added for it |
| R1-owed-landed-before-sync-seam-kept | sync_owed_slot calls landed before its fdatasync, the seam's record after | SURVIVED run 2; killed run 3 | same |
| R1-owed-taken-after-write | the round-0 shape: Maint::publish takes the owed entry only after its write and sync; write_slot takes nothing | killed | a_slot_write_torn_over_an_owed_slot_... (+cipher), a_second_seals_slot_torn_... (+cipher), a_slot_whose_fdatasync_failed_... (+cipher), a_drain_answered_before_its_slots_sync_keeps_the_log_file_the_slot_covers. This is also before.log |
| R1-unsynced-owed-not-taken | publish_unsynced puts the owed entry back when its write fails | SURVIVED run 2; killed run 3 | a_second_seals_slot_torn_over_an_owed_slot_... (+cipher), added for it |
| R1-pending-owed-not-taken | the pending landing keeps the owed entry across its write | SURVIVED | none. See below |
| R1-publish-landed-before-write | Maint::publish records its slot durable before writing it | killed | a_slot_write_torn_... (+cipher), a_slot_whose_fdatasync_failed_... (+cipher) |
| R1-publish-landed-before-sync | Maint::publish records its slot durable between its write and its fdatasync | killed | a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off (+cipher) |
| R1-unsynced-not-deferred | publish_unsynced tells the watch its write is synced | killed | a_publish_after_a_drains_owed_slot_... (+cipher), a_second_seal_landed_..., a_drain_after_an_owed_slot_was_synced_... (+cipher), an_open_that_takes_an_unsynced_slot_... The watch's crash model reads the deferred flag (the kill states, `reached`), not only the synced[] record |
| R1-upgrade-seed-deleted | the open's record of the upgrade's second write in slot_synced deleted | SURVIVED run 2 | No test reads it. It was the test seam's bookkeeping, so the line was taken out of the engine |
| R1-upgrade-landed-older | the open's upgrade marks the older slot, written second, as the durable one | SURVIVED run 2 (pattern gone in run 3 with the line above) | Equivalent. After an upgrade both slots are fdatasynced and hold the same names and coverage, so either is a correct slot to keep. A publish over the newer slot that then tears leaves the older one, which is the same store |
| R1-tear-write-whole | the tear seam writes the whole image before it fails | SURVIVED run 2; killed run 3 | a_slot_write_torn_... (+cipher), through a control added for it: the open after the tear skips a slot as torn |
| R1-sync-slot-ignores-seam | the slot-sync seam does not fail | killed | a_slot_whose_fdatasync_failed_... (+cipher): the case is not reached |
| R1-open-seed-deleted | the open's seed of slot_synced for the slot it synced deleted | killed | an_open_that_takes_an_unsynced_slot_never_overwrites_the_durable_one (+cipher) |

Survivor (round 1):
- R1-pending-owed-not-taken. Under manifest_pending, a drain's slot is written
  deferred (publish_unsynced) only when land_durable finds the pending list
  empty. land_readable sets that list whenever its seal has segments, and only
  a synced publish clears it. So a slot can be owed at a pending landing only
  if a seal with no segments was landed in between. No test reaches that state
  (the tear seam, which fires only over an owed slot, never fired under the
  pending arm). I did not prove that no such state exists. The pending path
  makes no check of its own here: it calls the same Maint::write_slot as the
  other two writers, and that call takes the owed entry before the write.

## Round 2 (repair)

Runner: r2/mut2.py, from the fixed copies in r2/keep (sha256 in
r2/keep/sha256.txt). Each verdict is appended to r2/mut-results.jsonl as it
is got; logs and diffs are r2/m-<name>.{log,diff}. Pass 1 ran on the tree
after the blocking repair (slot_sync_due: Option<File>); pass 2 after the
seams and tests below were added. E1 was run with the root-only kpageflags
test skipped (MUT_SKIP), which is what a non-root gate sees.

| mutant | what | verdict | killed by |
|---|---|---|---|
| E1 open-syncs-other-slot | the open fdatasyncs `MANIFEST_SLOTS[1 - m.slot]`, keeps `slot.landed(m.slot)` | pass 1: killed only as root (an_open_makes_the_slot_it_takes_durable (+cipher)); with that test skipped SURVIVED. Pass 2: killed without root | an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+cipher): the seam's words name the inode it failed for, and the test holds it to the slot taken |
| E2 read-all-slot-zero | manifest_read_all leaves `slot` 0 for every manifest | killed (both passes, root test skipped in pass 2) | db::a_manifest_publish_alternates_two_slots_..., an_upgrade_never_writes_first_over_the_slot_the_store_was_read_from. an_open_that_takes_an_unsynced_slot_... does not kill it (run), and why is reasoned, not traced: in its states the slot overwritten under E2 is the unsynced one the open took, the log it covers is not retired (the owed sync never ran before the kill), so a tear there leaves the older slot plus the log, every write. The case that loses (the sweep before a power cut) is the open-sync-fails test's, which the inode check holds |
| E3 tear-after-take | write_slot takes the owed entry before reading it for the tear | killed | a_slot_write_torn_... (+cipher), a_second_seals_slot_torn_... (+cipher): "case not reached" |
| E4 publish-landed-only-if-not-owed | Maint::publish lands its slot only when no slot was owed | killed | a_publish_after_a_drains_owed_slot_... (+cipher), a_second_seal_landed_..., a_slot_whose_fdatasync_failed_... (+cipher), a_slot_write_torn_... (+cipher), an_open_whose_slot_sync_fails_... (+cipher), every_arm_... |
| E5 pending-landed-before-wait | the pending landing names its slot durable before the helper's wait | SURVIVED (equivalent) | See below |
| E6 upgrade-older-first | the open's upgrade writes the older slot (the one read) first | pass 1 SURVIVED; pass 2 killed | an_upgrade_never_writes_first_over_the_slot_the_store_was_read_from, through Options::tear_open_slot_write (added for it) |
| E7 publish-retires-only-on-pending | a synced publish over an owed slot does not retire what it held | killed | a_drain_answered_before_its_slots_sync_keeps_the_log_file_the_slot_covers, db::a_manifest_publish_alternates_... |
| E8 owed-retire-before-sync | sync_owed_slot retires before its fdatasync | killed | a_slot_whose_owed_fdatasync_failed_... (+cipher) |
| E9 owed-kept-when-torn | write_slot keeps the owed entry when it tears | killed | a_slot_write_torn_over_an_owed_slot_... (+cipher). The pending-path-only variant (take after the write returns Ok on the pending path) is R1-pending-owed-not-taken, which survives for the reason given in round 1: no state reaches an owed slot at a pending landing |
| R2-owed-slot-is-durable | sync_owed_slot lands `1 - next()` | killed | a_drain_after_an_owed_slot_was_synced_... (+cipher), a_slot_whose_owed_fdatasync_failed_... (+cipher) |
| R2-owed-seq-stale | sync_owed_slot records `manifest_seq - 1` for the seam | killed | a_drain_after_an_owed_slot_was_synced_... (+cipher) |
| R2-owed-not-landed | sync_owed_slot computes the slot and lands nothing | killed | a_drain_after_..., a_slot_whose_owed_fdatasync_failed_... (+cipher) |
| R2-helper-fail-ignored | the pending landing hands the helper `fail = false` | killed | a_pending_slot_whose_fdatasync_failed_stops_the_slots_until_a_reopen (+cipher): the flush answered Ok |
| R2-open-tear-ignored | the open's tear seam never set | killed | an_upgrade_never_writes_first_... |
| R2-helper-fail-no-stop | a seal landing's failed land_readable (the inline path) does not stop the queue | killed | a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look (not the new pending test: its drain lands through land_front_readable) |

E5 is equivalent. The helper's wait failing makes land_readable answer the
error, and both callers stop the seal queue at once (`land_front_readable`'s
Err arm and the inline path's, each `self.stop_seals(&e)`). From then no
slot is written until the store is reopened (publish asserts the window
shut; the close asks the queue), so the `durable` the mutant set early is
never read by a `next()`; a reopen reads the slots afresh. The new test
a_pending_slot_whose_fdatasync_failed_stops_the_slots_until_a_reopen
(+cipher) holds that premise: after the failed helper sync, the next flush
answers the failure and the slot watch sees no write, and a power cut
there, with the pending slot whole or torn, opens to every acknowledged
commit. It passes under E5 too, which is the point of an equivalent mutant.

## Round 3 (r3/mut3.py, verdicts r3/mut-results.jsonl, X7/X4 rerun r3/mut-run-x7.log)

Fixed engine: r3/keep/db.rs.fixed (sha256 in r3/keep/sha256.txt). Each run: whole durable binary plus the two db slot tests.

| mutant | what | verdict | killed by |
|---|---|---|---|
| X1 recovery-not-landed | the open's run recovery writes its slot with manifest_write_shape into slot.next() and lands nothing | killed (expected to survive; new test) | an_open_that_recovers_a_run_keeps_the_slot_that_names_it: the reopened store's first publish tears the recovery slot and the run is lost |
| X2 nomanifest-not-landed | the manifest-less open's write is not landed | killed (new test) | a_store_opened_without_a_manifest_keeps_the_slot_its_open_wrote: "publishes [2, 3] were written over the only manifest the device held" (OsTracker.bare) |
| X3 publish-not-landed-inline | Maint::publish lands only when `self.wake.is_some()` | killed (new arm in the test) | every_arm_that_writes_a_slot_keeps_the_last_durable_one, its "inline" arm (publish_in_background off) |
| X4 open-landed-before-sync | the open's `slot.landed(m.slot)` before `f.sync_data()?` | SURVIVED, equivalent | `slot` is a local of Db::open; the `?` on sync_data returns from the open before Maint is built, and nothing reads `slot` between the two lines |
| X5 create-none | Db::create seeds Maint with SlotChoice::NONE | killed (new test) | the_first_publish_after_create_keeps_off_the_slot_create_wrote (+cipher), db::a_manifest_publish_alternates_..., a_seal_whose_fsync_failed_is_not_named_durable_by_the_close |
| X6 publish-recorded-before-sync | Maint::publish records slot_synced before sync_slot | killed (assertion added) | a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off (+cipher): the engine's record at the first write after the failure is held to what the test saw synced; the watch is on from the store's creation so the record is kept |
| X7 tear-not-only-over-owed | the tear seam fires without `self.slot_sync_due.is_some()` | pass 1 SURVIVED; killed after the test asserts which write it tore | a_slot_write_torn_over_an_owed_slot_... (+cipher): the torn write (the newest slot's seq) must follow a deferred write whose sync it did not see |
| X8 open-sync-unrecorded | the open's `synced[m.slot] = m.seq` deleted | killed | an_open_that_takes_an_unsynced_slot_... (+cipher) |
| X9 upgrade-same-slot | the upgrade's older copy written into `to` | killed | an_upgrade_never_writes_first_..., db::a_store_from_before_the_pending_flags_opens_with_both_slots_rewritten, db::a_manifest_publish_alternates_... |
| R3 sweep-before-sync | the cipher sweep moved back before the open's slot fdatasync | killed | an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it_under_a_cipher |
| R3 sweep-deleted | the cipher sweep deleted | killed | the same test's control (the successful open sweeps an envelope neither slot names) |
| R3 recovery-sync-unrecorded | the recovery write's new `synced[to] = mseq` deleted | killed | an_open_that_recovers_a_run_keeps_the_slot_that_names_it (the record the crash builder reads) |
