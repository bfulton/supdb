# open-cost mutants

Runner: mut.py over mutants.json (the first 22) and mutants-rest.json (the last 4, after the first runner
was stopped by the harness's background limit mid-O8; src/db.rs was put back from keep/src_db.rs.fixed,
sha256 checked, before the second runner). Each mutant written into src/db.rs from keep/src_db.rs.fixed,
the whole durable binary run under the checked profile (TMPDIR of the unit's own), the verdict appended as
it came, the fixed file put back and its sha256 checked. Logs mut-<name>.log. The tree as left is
byte for byte keep/*.fixed.

| mutant | what | verdict (killing tests, first few) |
|---|---|---|
| N1-listing-says-present | open_refusals in the clear: every named name read as in the listing | KILLED by a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, an_open_makes_no_slot_file_it_does_not_find, an_open_under_the_rename_shape_refuses_by_the_manifest_file_it_read, an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file |
| N2-plain-stats-each | in the clear, each named segment stat-ed again (vault::on_device) | KILLED by a_plain_open_finds_its_named_segments_in_its_listing |
| N3-cipher-uses-listing | under a cipher, the listing (the vault's names) asked instead of each envelope | KILLED by a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, an_open_makes_no_slot_file_it_does_not_find_under_a_cipher, an_open_under_a_cipher_looks_for_each_named_envelope |
| N4-name-takes-all | name_takes answers true for every key: every candidate mapped | KILLED by an_open_after_a_crash_reads_only_the_segments_over_an_unnamed_segments_keys |
| N5-lo-flipped | name_takes: key < lo flipped to key > lo | KILLED by a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look, a_presync_that_fails_before_its_seals_publish_stops_every_manifest, a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline, a_rename_shape_publish_leaves_a_whole_manifest_at_every_step (+6 more) |
| N6-hi-flipped | name_takes: key < hi flipped to key > hi | KILLED by a_rename_shape_publish_leaves_a_whole_manifest_at_every_step, a_slots_store_killed_over_an_owed_slot_reopens_under_the_rename_shape, an_injected_presync_failure_is_taken_once_inline, an_open_after_a_crash_reads_only_the_segments_over_an_unnamed_segments_keys (+1 more) |
| N7-listing-unsorted | the listing not sorted before binary_search | KILLED by a_commit_answered_ok_after_a_failed_log_sync_survives_a_crash, a_buffered_crash_never_recovers_a_batch_without_its_first_key, a_delete_below_the_greatest_leaves_it_where_it_was, a_drain_after_an_owed_slot_was_synced_keeps_that_slot (+76 more) |
| N8-unnamed-after-replay | unnamed_refusals moved back after the replay | KILLED by an_open_that_refuses_an_unnamed_segment_replays_no_log_first |
| N9-routing-inverted | the fence filter inverted (only segments that do not take the key in) | KILLED by a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look, a_presync_that_fails_before_its_seals_publish_stops_every_manifest, a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline, a_rename_shape_publish_leaves_a_whole_manifest_at_every_step (+6 more) |
| N10-slot-reopened-for-sync | each slot file opened again by path for its fdatasync (the landed shape's extra open, with no trace note) | SURVIVED |
| N11-read-keeps-no-file | manifest_read_all keeps no descriptor: no slot synced | KILLED by a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing, a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it_under_a_cipher (+19 more) |
| S3a-no-open-sync | slot-choice S3a: the open's slot fdatasync deleted | KILLED by acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen, an_open_makes_the_slot_it_takes_durable, an_open_makes_the_slot_it_takes_durable_under_a_cipher (+9 more) |
| M1-other-slot-not-synced | other-slot M1: the other slot's entry out of the loop | KILLED by a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing, a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it_under_a_cipher (+13 more) |
| M5-taken-synced-twice | other-slot M5/R7: 1 - m.slot -> m.slot | KILLED by a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing, a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it_under_a_cipher (+13 more) |
| M6-other-error-swallowed | other-slot M6: the other slot's sync error dropped | KILLED by a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen, a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen_under_a_cipher |
| M7-other-skipped-under-cipher | other-slot M7: the other slot skipped under a cipher | KILLED by a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher, a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it_under_a_cipher, a_slots_open_after_a_crossing_open_that_failed_survives_a_power_cut_under_a_cipher, a_store_moved_slots_rename_slots_killed_before_a_directory_fsync_survives_under_a_cipher (+3 more) |
| R6-seams-swapped | other-slot R6: the two seams swapped | KILLED by a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen, a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen_under_a_cipher, an_open_under_the_rename_shape_whose_slot_sync_fails_leaves_the_directory_as_it_found_it, an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+2 more) |
| M9-absent-slot-an-error | other-slot M9 (moved): an absent slot file an error (manifest_read_all's NotFound arm deleted) | KILLED by a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing, a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher, a_kill_between_a_rename_and_its_directory_fsync_survives_a_power_cut_after_the_reopen, a_kill_between_a_rename_and_its_directory_fsync_survives_a_power_cut_under_a_cipher (+32 more) |
| M2-named-check-deleted | open-order M2: the named-segment check deleted | KILLED by a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, an_open_makes_no_slot_file_it_does_not_find, an_open_makes_no_slot_file_it_does_not_find_under_a_cipher (+3 more) |
| Z5-named-check-pcs-only | open-order Z5: the check on pcs- names only | KILLED by a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed, a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, an_open_makes_no_slot_file_it_does_not_find, an_open_makes_no_slot_file_it_does_not_find_under_a_cipher (+2 more) |
| Z9-named-check-first-only | open-order Z9: the check on the first name only | KILLED by an_open_makes_no_slot_file_it_does_not_find, an_open_makes_no_slot_file_it_does_not_find_under_a_cipher, an_open_under_a_cipher_looks_for_each_named_envelope, an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file |
| W6-cipher-exists-not-device | open-order W6: under a cipher vault::exists for vault::on_device | KILLED by a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, an_open_makes_no_slot_file_it_does_not_find_under_a_cipher, an_open_under_a_cipher_looks_for_each_named_envelope |
| O8-cipher-on-device-always | open-order O8: under a cipher every envelope read as there | KILLED by a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher, an_open_makes_no_slot_file_it_does_not_find_under_a_cipher |
| W8-holders-runs-only | open-order W8: holders from the runs' segments only | KILLED by a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look, a_presync_that_fails_before_its_seals_publish_stops_every_manifest, a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline, a_rename_shape_publish_leaves_a_whole_manifest_at_every_step (+6 more) |
| O5-runs-hold-nothing | open-order O5: the runs' segments hold nothing | KILLED by a_presync_that_fails_before_its_seals_publish_stops_every_manifest, a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline, the_windows_a_torn_publish_leaves_open_through_the_slot_before |
| X2-every-unnamed-held | open-order X2: the unnamed refusal never fires | KILLED by a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired, a_refused_open_leaves_the_log_files_its_replay_ended_before, an_open_that_refuses_an_unnamed_segment_replays_no_log_first |

N10 survived, as expected of a cost mutant: an open of the slot file again by path for its sync changes no
state and no answer. The unit's test counts the open's own trace notes (`read_slot`), which a reopen through
plain `File::open` does not make; it held the fold only against the landed shape, whose second open went
through `vault::open_slot_found` and noted it. Held by reading (the loop takes `slot_files[s]`, and the only
other opener of a slot for reading, `vault::open_slot_found`, is gone) and by the syscall count
(strace/summary.txt: 20 openat an open at 10000 keys against the landed shape's 24). Same class as other-slot's
R5 (sync_all for sync_data). Not equivalent; not reachable by a behaviour test without counting the
process's opens, which no test here does.

Equivalent shapes not run: the `break` after the first holder removed (maps more candidates, same answer);
`drop(slot_files)` removed (the descriptors close at the open's end instead).

## Repair round 1

The test the review named (`an_open_opens_each_slot_file_once`, + cipher) counted the read's own trace
record and so held the fold only against the measuring tree's shape. It now holds the descriptor: the
slot read (`read_slot`, recorded once the bytes are read) and the slot's fdatasync (`sync_slot`) each
record, through `vault::note_file`, the descriptor's number and its open file description's offset
(`Db::trace_slot_descriptors`); the test asserts both slot files are read whole through one descriptor
and synced through the same (number, offset). A second open of the file starts at offset 0, so a reopen
fails it whether or not the read's descriptor is still alive (fd number reused or not). Runner: mut.py
over r1-mutants.json / r1-mutants-rest.json / r1-mutants-e8.json, results r1-mut-results.txt, logs
mut-R1-*.log, each against keep/*.fixed of the round with the fixed copy put back and its sum checked.

| mutant | what | verdict |
|---|---|---|
| R1-N10-reopened-by-path | sync_slot given `File::open(vault::real(path))`, the read's File taken and dropped | KILLED by an_open_opens_each_slot_file_once, an_open_opens_each_slot_file_once_under_a_cipher ((fd, 0) against (fd', len)) |
| R1-N10b-vault-open | the same through `vault::open(&path)` | KILLED by the same two |
| R1-N10c-reopen-read-kept-alive | `slot_files[s].as_ref()` then `vault::open(&path)`: read's descriptor still open | KILLED by the same two |
| R1-E1-seg-name-takes-nothing | name_takes answers false for a name with no fences (`seg-`) | SURVIVED the tests there were; KILLED by the new an_open_after_a_crash_holds_an_unnamed_segments_keys_to_pieces_without_fences (refused: "par-...--.sup holds keys nothing the slot names holds") |
| R1-E2-file-kept-only-if-valid | manifest_read_all keeps a slot's descriptor only when the slot parses (so a torn/empty slot is never synced) | KILLED by a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen (+_under_a_cipher). This is also the brief's sixth extra mutant (the same edit, listed twice) |
| R1-E4a-listing-contains | open_refusals in the clear: `!listed.contains(n)` on the sorted listing | SURVIVED: equivalent answer, cost probe only (linear for binary search) |
| R1-E4b-on-disk-for-listing | `!on_disk.contains(n)` in place of the listing | SURVIVED: equivalent for segment names (on_disk is the listing's segment names), cost probe only |
| R1-E5-candidates-no-runs-chain | unnamed_refusals' candidates: the runs' segments' chain dropped, routing kept | KILLED by a_presync_that_fails_before_its_seals_publish_stops_every_manifest (+_inline), an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes, the_windows_a_torn_publish_leaves_open_through_the_slot_before |
| R1-E7-no-break-after-held | the `break` after a holder is found removed | SURVIVED: cost only (maps every routed candidate after the holder; same answer). Not killed by a cost count; noted |
| R1-E8-pcs-takes-all | name_takes reads only `par-` fences; every `pcs-` name takes every key | SURVIVED the tests there were (the route test's merge folded its pieces); KILLED by the new an_open_after_a_crash_reads_no_piece_whose_fences_leave_an_unnamed_segments_key_out (the piece over the first range read for the middle range's key) |
| R1-E9-unnamed-after-listing-loop | unnamed_refusals moved after the wal/spare listing loop, still before the replay | SURVIVED: equivalent but for which refusal answers first against "wal file name is malformed"; no test pins that order, and none is wanted |
| R1-E10-unsorted-linear | the listing's sort removed, `!listed.contains(n)` | SURVIVED: equivalent answer; the sort is there for the binary search alone (the wal ids are sorted on their own, spares are interchangeable) |

The brief's first and sixth extra mutants are one edit (a slot's descriptor kept only for a slot that
parses); run once as R1-E2. Its second ("files[i] after the parse") is the same. N10 is held now.

## Repair round 2

The reviewers' nine extra mutants, deduplicated to eight edits (their first and fifth are one edit, as are
their fourth and eighth; "delete `hi.is_empty() ||`" and "`!hi.is_empty() &&`" run apart). Runner: mut.py
over r2-mutants.json, then r2-mutants-x1.json; results r2-mut-results.txt, logs mut-R2-*.log; each against
keep/*.fixed with the fixed copy put back and its sum checked. No src change this round.

| mutant | what | verdict |
|---|---|---|
| R2-X1-lo-le | name_takes: `key < lo` -> `key <= lo` | SURVIVED the tests there were: no test built an output whose last key is a holder's lower fence. Test added, an_open_after_a_crash_holds_an_unnamed_segments_key_at_its_ranges_lower_fence (partitions a-/m/z-, the middle one the key `m` alone, so its fence is the key; a piece writing `m` again merged and held at its landing; every named segment that takes `m` in has `m` for its lower fence). Rerun as R2-X1b: KILLED by that test (and an_injected_presync_failure_is_taken_once_inline) |
| R2-X2-hi-unbounded-excludes | `hi.is_empty() \|\|` -> `!hi.is_empty() &&` | KILLED by a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look, a_presync_that_fails_before_its_seals_publish_stops_every_manifest (+_inline), the_windows_a_torn_publish_leaves_open_through_the_slot_before, every_arm_that_writes_a_slot_keeps_the_last_durable_one (+4 more) |
| R2-X3-hi-empty-deleted | `hi.is_empty() \|\|` deleted (unhex("") is empty, no key below it) | KILLED by the same nine |
| R2-X4-skip3 | `.skip(2)` -> `.skip(3)`: the fields read as no two fences, every name takes every key | KILLED (not cost-only: the trace tests hold which segments are read) by an_open_after_a_crash_reads_only_the_segments_over_an_unnamed_segments_keys, an_open_after_a_crash_reads_no_piece_whose_fences_leave_an_unnamed_segments_key_out |
| R2-X5-skip1 | `.skip(2)` -> `.skip(1)` | KILLED by the same two |
| R2-X6-other-slots-descriptor | `slot_files[s].take()` -> `slot_files[1 - s].take()` | KILLED by an_open_opens_each_slot_file_once (+_under_a_cipher), an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it (+cipher; the inode-naming seam), a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen, the rename-shape crossing tests (alt_found then set from the wrong file) (+24 more) |
| R2-X7-descriptors-dropped-first | `drop(std::mem::take(&mut slot_files))` ahead of the sync loop (as written in the brief, `drop(slot_files)` there does not compile: the loop uses it) | KILLED by an_open_makes_the_slot_it_takes_durable (+cipher), acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment, an_open_opens_each_slot_file_once (+cipher) (+19 more) -- S3a's killers among them |
| R2-X8-note-before-read | `vault::note_file("read_slot", ..)` before `read_to_end` | KILLED by an_open_opens_each_slot_file_once, an_open_opens_each_slot_file_once_under_a_cipher (the read's offset 0 against the sync's len: the offset, not the fd number, holds the descriptor) |

## Repair round 10

No extra mutants were given and no source changed; the mutant tables above stand for the tree, which is
byte for byte keep/*.fixed.
