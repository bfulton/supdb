# tests-tidy: mutants

Runner: scripts/mut.py (definitions in scripts/mutants.json), one at a time, each edit applied to the
fixed file, the named tests run under `cargo test --profile checked --no-fail-fast`, the verdict written to
mut-results.txt as it came (log mut-<name>.log), and the file put back from keep/ and checked against
keep/sha256.txt after each. The stress mutant runs a prebuilt binary (scripts/stress.sh).

| mutant | what | verdict | killed by |
|---|---|---|---|
| S0 no-pause (item 1/56) | `copy_live` copies with no pause (the old `copy_store` at every live copy); bin/durable-nopause, four processes at a time, 12 runs each | KILLED (2 of 48 runs) | an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes, twice: "refused: the manifest slot manifest.alt names seg-00000000-0000000000000000.sup, which is missing from the directory" (stress-nopause/w1-r2.out, w1-r10.out). The pause is what protects the copy. Same run: an_injected_presync_failure_is_taken_once_inline failed 3 times (no copy in it: the test's own read race, fixed in this unit by a wait), and one run died of SIGBUS with no test named (not attributed). |
| T1 pause-no-ack | `pause_segment_work` returns without waiting for the thread to say it is waiting | SURVIVED the first test | a_paused_segment_work_lands_nothing_until_it_goes_on cannot tell: the thread takes the job before the merge is let go. Test added. |
| T1b (same) | as T1, against the added test | KILLED | a_pause_asked_inside_a_landing_returns_after_it ("the pause returned while the landing it was asked inside was still writing its slot") |
| T2 pause-no-wait | the segment work's thread acknowledges the pause and goes on without waiting | KILLED | a_paused_segment_work_lands_nothing_until_it_goes_on |
| T3 trace-no-root-filter | `io_trace` records every call in every record, whatever directory | KILLED | the_call_record_holds_the_directory_asked_for_until_it_is_let_go |
| T4 trace-off-keeps | asked off, the record is handed back and kept | KILLED | the_call_record_holds_the_directory_asked_for_until_it_is_let_go |
| T5 = run-errors O3 (concern 10) | `defer` leaves the writer unstuck (`self.piece = STUCK_PIECE` deleted) | KILLED, by an assertion | a_marked_segments_failed_sync_is_every_later_syncs_answer: "the writer made no progress for a minute: a write after its held failure loops" (before this unit: a hang, killed only by the runner's timeout) |
| T6 = other-slot M10/E5 (item 28), SUPDB_TEST_UNPRIVILEGED=1 | `vault::sync_slot` records a sync it never made (the fdatasync deleted, the record kept) | KILLED unprivileged | an_open_makes_the_slot_it_takes_durable{,_under_a_cipher}: "both slots ... are on the device once the open has returned" through the page cache's drop of clean pages (before: SURVIVED unprivileged, other-slot's E5 as uid 65534) |
| T7 = slot-choice X3 (item 31) | `Maint::publish` lands its slot only where the segment work has a thread of its own | KILLED | every_arm_that_writes_a_slot_keeps_the_last_durable_one, by its inline arm: "inline: 8 of 8 crash states lost acknowledged commits" |
| T8 (concern 8) | a publish writes the slot of its sequence's parity | KILLED | db::a_manifest_publish_spares_the_newest_durable_slot_and_the_open_takes_the_newest_valid: "a publish wrote over the slot holding the newest manifest known durable: SlotWrite { seq: 4, slot: 0, deferred: false, synced: [2, 0] }" |
| T9 (item 34) | the pending landing's assertion inverted (`slot_sync_due.is_some()`) | KILLED | twelve pending tests (the pending landing is reached in each; the assertion as written fired in none of the binaries run: durable, db, cipher, fuzz -- after.log) |
| T10 (16b) | a write to a run whose fdatasync failed is taken (the refusal in `run_write_fault` deleted) | KILLED | a_write_that_leaves_a_run_whose_sync_failed_leaves_the_store_reading_and_refusing ("the run's seal stops the queue: the case was not reached", after its two-minute wait) |
| T11 (item 36) | the rename open removes `manifest.alt` without first writing the manifest it read from it as `manifest` | SURVIVED twice | the kills after each flush are taken after a publish, which writes `manifest` anyway; the test now also kills right after each crossing's open, and varies which slot is newest |
| T11c (same) | as T11 | KILLED | a_store_moved_between_the_manifest_shapes_keeps_its_writes: "spare false, extra false, round 2: the kill after the crossing: refused: the manifest slot manifest names par-... which is missing from the directory" |

## Stage 0's mutants credited to a test whose copy changed (followup 1), run again

Re-run: delete-max's mutants credited to an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes,
the test whose copy the race reached (followup 56), against the whole durable binary under the pause.

| mutant | stage 0 | now |
|---|---|---|
| DM-b raise on the non-ordered path only | killed by the ends-a-run test alone | KILLED by the ends-a-run test alone: the kill was the test's |
| DM-a the raise removed | killed by 6 of 9 (later 9 of 14) | KILLED by 9 |
| DM-j the raise guarded by a non-empty table | killed by 7 of 11, the ends-a-run test among them | KILLED by 7, the ends-a-run test not among them: its stage-0 credit there was the race; the verdict stands on the other six |

Already done by stage 0 itself: log-at-open's mut10 (every mutant credited to the rotation test, after its hold;
none changed verdict), open-order's Z4 (its "kill" by the same delete test was this race, recorded there, and
killed by its own test later). Not re-run: the other units' mutants credited to durable tests whose copy moved from
copy_store to copy_live (see notes.md, not verified).
