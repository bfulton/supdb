# supdb's guarantees against its tests, at C0

What this covers: C0 is `b3a043dc` (#37). The working tree was read at
`dd5dbbbc`, and `git diff --stat b3a043dc HEAD` shows changes under
`release/` alone, so `src/`, `tests/`, `docs/`, `README.md` and `ROADMAP.md`
are C0's. This was a read-only audit: no build, no test run, and no file
written in the repository.

The documents audited: `README.md`; `src/lib.rs`; the rustdoc of `SyncPolicy`,
`Encryption`, `Options` (fields without `doc(hidden)`), `Db::{open, commit,
sync, flush, seal, settle, checkpoint, close}`, `Txn`, `Isolation`,
`Reader`, `SegmentWriter` and `SegmentOptions`; and the guarantee sections of
`docs/engine.md` ("The promises", "Crash injection", "What this does not
promise").

How each test holds its promise, in the table below:
- **built**: the crash state is built by hand, either by copying the page
  cache (`copy_live`/`copy_closed`) or by cutting files with
  `trace_wal_syncs`/`watch_slot_writes`.
- **kill**: a child process is killed with SIGKILL (`tests/kill.rs`, the
  `tests/runlog.rs` oracle).
- **seam**: an engine seam fails a syscall (`fail_next_*`, `tear_*`).
- **fuzz**: the differential fuzz (`tests/fuzz.rs`). Its "crash" is a
  `drop`, which joins the store's threads and writes the held frames, so it
  holds visibility and consistency, not loss on a power cut.
- **unit**: a `#[cfg(test)]` module in `src/`.
- **none**: no test holds it.

## 1. Promise-to-test table

### Commit, sync, flush and close durability

| Promise (where) | Test(s) | How |
|---|---|---|
| A commit is one log append plus one fdatasync, and batches are atomic (README; `Db::commit`; `db` module doc) | `tests/db.rs::a_torn_tail_loses_its_batch_whole_and_earlier_batches_survive`, `a_flipped_byte_anywhere_in_a_batch_loses_that_batch_and_the_ones_after`; `tests/kill.rs::a_killed_writers_acknowledged_batches_are_all_there` (Always, puts, 10 rounds) | built + kill |
| Under `Always`/`EveryN` a commit is *written* when it is made, so a process kill takes nothing acknowledged (`SyncPolicy`) | `tests/kill.rs::a_killed_buffered_writer_loses_nothing_it_acknowledged` (EveryN(u32::MAX), ordered run, seals held); `tests/runlog.rs::a_torn_store_is_a_prefix_of_what_was_committed`, rounds with nothing torn | kill |
| `EveryN(n)` bounds the loss at n batches, and an unsynced tail is lost whole, never in part (`SyncPolicy` head) | `tests/db.rs::every_n_loses_the_unsynced_tail_whole_and_never_in_part`: one tear of 5 bytes, WAL arm only; the runlog oracle at EveryN(u32::MAX) tears every log to any length and asserts only that the result is a prefix | built (weak; see M4) |
| Under `Explicit` nothing is promised until `sync`/`flush`/close, and the loss is whole, never in part | `tests/runlog.rs::a_held_commit_is_in_no_file_until_a_sync`, `a_log_with_no_frames_writes_nothing_and_its_sync_drains`, `frames_a_durable_seal_covers_are_never_written`, `held_frames_wait_for_the_manifest_that_covers_them`, `a_log_over_its_cap_writes_what_it_holds`, `frames_held_before_a_run_go_to_their_own_file`, `a_dropped_store_writes_what_it_held_and_a_closed_one_nothing`; the oracle's explicit rounds (killed, never torn) | `Db::crash` + kill |
| A held batch is dropped unwritten only below a sequence that a manifest on the device covers (`SyncPolicy::Explicit`; CLAUDE invariant) | `frames_a_durable_seal_covers_are_never_written`, `held_frames_wait_for_the_manifest_that_covers_them` (pending and not) | built |
| `sync` Ok means every commit before it is durable, replayed frames included, and an open run's commits too (`Db::sync`) | `tests/durable.rs::a_sync_after_a_reopen_makes_the_replayed_writes_durable_{buffered,always,under_a_cipher}`, `a_sync_makes_an_open_runs_commits_survive_a_power_cut`, `a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut`, `a_sync_puts_an_open_runs_commits_where_a_process_kill_keeps_them` (7 RunHeld shapes × EveryN(5) and buffered), `tests/runlog.rs::a_sync_covers_the_runs_as_well_as_the_log` | built |
| When the log owes frames, `flush` answers only once its slot is synced (`Db::flush`) | `a_flush_after_a_reopen_is_answered_only_once_its_writes_are_durable_{buffered,always,under_a_cipher}`, `a_drain_answered_before_its_slots_sync_keeps_the_log_file_the_slot_covers`, `a_drain_names_its_runs_segment_durable_before_it_answers{,_inline}` | built + seam (`hold_owed_slot`) |
| `close` makes everything durable (it is a flush, and a promise point under `Explicit`) | `a_dropped_store_writes_what_it_held_and_a_closed_one_nothing`; fuzz clean closes | built + fuzz; **the rustdoc contradicts it** (M2) |
| A replayed log is not taken as durable, and the first sync covers it (`Db::open`, `SyncPolicy`) | `a_sync_after_a_reopen_*`, `a_rotation_after_a_reopen_leaves_the_replayed_file_loose`, `a_reopen_with_nothing_replayed_owes_its_log_no_sync` (read off the engine's own trace; ROADMAP lists a syscall-level test as open) | built |
| A first sector of zeroes is an empty log; any other foreign header is refused (`Db::open`) | `a_store_just_created_opens_after_a_power_cut_loses_its_logs_unsynced_header{,_under_a_cipher}`, `a_foreign_log_header_is_refused`, `a_zeroed_header_over_frames_in_its_first_sector_is_refused_as_damage`, `a_zeroed_older_log_file_before_a_chained_one_is_refused_as_damage{,_under_a_cipher}`, `tests/db.rs::a_wal_header_torn_by_power_loss_opens_and_is_rewritten` | built |
| A `Txn` is all or nothing, reads its own writes, and is aborted on drop (`Txn`; engine.md "rollback, read-your-writes") | `tests/db.rs::a_transaction_is_all_or_nothing_and_sees_its_own_writes`; crash atomicity rests on the commit-frame tests; `tests/durable.rs::an_append_after_a_transactions_delete_survives_a_crash_before_its_seal_writes` | built |
| A run's batch that ends in the log is atomic | the runlog oracle (draw 8: run, then logged ops, in one commit) | kill + tear |

### Errors that are kept and answered

| Promise (where) | Test(s) | How |
|---|---|---|
| A failed log write is put back and written again by the next commit, and no acknowledged commit is lost | `a_failed_log_write_loses_no_commit_acknowledged_after_it{,_under_a_cipher}`, `a_log_write_that_failed_part_way_*` (including a reopened log opened `O_APPEND`), `a_failed_log_write_in_a_recycled_log_*`, `a_store_whose_log_rotated_after_a_failed_write_survives_a_power_cut` | seam (`fail_next_wal_write`, Always only: the helper asserts it) |
| A write that cannot be cut back is kept | `a_failed_write_that_cannot_be_put_back_is_kept_until_the_store_is_reopened`, `..._kept_by_a_{sync,flush,seal}`, `..._by_a_runs_commit_under_a_cipher` | seam (`fail_next_wal_cut`) |
| A failed log fdatasync is never retried; every later commit/sync/flush/Txn/seal/close answers it until reopen, and the open finds a prefix | `a_failed_log_sync_is_kept_until_the_store_is_reopened` (Always), `a_commit_answered_ok_after_a_failed_log_sync_survives_a_crash{,_under_a_cipher}`, `a_sync_answered_ok_after_a_failed_one_makes_every_commit_before_it_durable` (EveryN(u32::MAX)), `a_failed_sync_of_a_loose_log_file_is_kept` | seam (`fail_next_wal_sync`, `fail_next_loose_wal_sync`) |
| A failed directory barrier is made good by a `.guard` link, and kept where the guard fails | `a_failed_sync_of_the_log_directory_*` (plain, cipher, rotation, guard fails); `tests/runlog.rs::a_barrier_that_fails_is_made_by_a_guard`, `a_barrier_whose_guard_fails_is_kept`; `tests/db.rs` guard tests | seam |
| A run's write/fdatasync failure: the batch is owed, its sync kept, and the run is never closed over a failed sync | ~30 `tests/durable.rs` tests: `a_run_write_that_failed_*`, `a_failed_run_fdatasync_is_kept_{until_the_store_is_reopened,under_a_cipher}`, `a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment`, the helper tests (`a_due_commit_is_not_acknowledged_over_a_failed_helper_sync`, `a_helpers_description_is_its_own_at_every_width_of_the_writers_descriptor`, ...) | seam (`fail_next_run_write{,_partial,s_twice}`, `fail_next_run_sync`, `HelperFault`) |
| A seal that fails stops the queue; no manifest names it; flush and settle answer it; sync does not (`SyncPolicy`) | `a_seal_whose_directory_fsync_failed_is_named_by_no_manifest`, `a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest`, `a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log`, `a_seal_whose_envelope_failed_under_a_cipher_*`, `a_stopped_queue_*`, `a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look`, the presync family, `tests/db.rs::a_failed_landing_stops_the_seals_behind_it_and_a_reopen_replays_them` | seam (default policy and cipher) |
| A slot's fdatasync or write that fails, or a torn slot, never lets the last durable slot be overwritten | `a_publish_after_a_drains_owed_slot_never_overwrites_the_last_durable_slot{,_under_a_cipher}`, `a_slot_write_torn_over_an_owed_slot_*`, `a_slot_whose_{,owed_}fdatasync_failed_*`, `every_arm_that_writes_a_slot_keeps_the_last_durable_one` | seam + built |

### Open, recovery and damage

| Promise (where) | Test(s) | How |
|---|---|---|
| The open removes and writes nothing until its slot is checked, and a refusal leaves every file as it found it (`Db::open`) | `an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file`, `an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file`, `an_open_with_both_slots_zeroed_*`, `an_open_with_no_readable_slot_beside_envelopes_refuses_under_a_cipher`, `a_fallback_onto_a_slot_naming_files_gone_is_refused_*`, `an_open_that_refuses_an_unnamed_segment_replays_no_log_first`, `a_refused_open_leaves_the_log_files_its_replay_ended_before` | built |
| The open fdatasyncs both slots before it removes anything | `an_open_makes_the_slot_it_takes_durable{,_under_a_cipher}`, `an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it*`, `an_open_opens_each_slot_file_once*` | seam + engine trace (`trace_dir_io`, `vault::sync_slot`) |
| Pending and rename manifest shapes, and moves between shapes | ~30 tests (`a_rename_shape_*`, `a_pending_store_*`, `a_store_moved_*`) | built + seam |
| A run's recovery is finished where it lies, and each crash window of that finish | `tests/runlog.rs::a_run_an_open_takes_is_finished_where_it_lies`, `an_open_cut_short_in_a_runs_finish_is_made_again`, `a_run_begun_behind_a_sync_refuses_a_gap_before_it` | built |
| A damaged key index fails the open (README) | `tests/segwriter.rs::every_flip_in_the_key_section_fails_the_open` (at the Blob level); `src/ordindex.rs::a_flipped_byte_anywhere_fails_the_open` | built at the Blob level; no Db-level test (L) |
| A damaged block fails the read rather than answering wrongly (README) | `tests/blob.rs::a_corrupted_block_byte_fails_the_read_rather_than_under_returning`, `src/block.rs::a_flipped_bit_in_a_chunk_is_detected`; form files: `tests/zpages.rs::a_flipped_byte_in_a_form_file_is_never_an_answer`, `src/db/zfile.rs` tests | built; **broken by a process-wide switch** (H1) |
| A damaged or missing ordered index is rebuilt | `tests/db.rs::a_segment_whose_ordered_index_is_missing_or_damaged_rebuilds_it_at_open` | built |
| The format version is refused outside [OLDEST, STORE_FORMAT], naming both versions, before anything changes (README Status) | `src/db.rs::store_format::{the_refusal_names_both_versions, a_store_of_a_newer_format_is_refused_before_its_directory_changes, a_newer_format_in_the_older_slot_is_refused, store_format_names_the_files_magics, ...}` | unit |
| A store from before manifests opens from its `seg-` names (`Db::open`) | `tests/db.rs::a_store_from_before_manifests_opens_from_its_segment_names` | built |

### Isolation, lock and checkpoint

| Promise (where) | Test(s) | How |
|---|---|---|
| `Latest`/`Snapshot`/`Dirty` (`Isolation`, README) | `tests/db.rs::a_reader_handle_sees_what_its_isolation_says`, the reader-thread tests, `a_handle_adopts_no_snapshot_past_its_commit`; fuzz handles at Latest and Snapshot; `tests/cipher.rs::a_snapshot_under_a_cipher_holds_its_state_and_none_of_the_cache` | built + fuzz |
| One open at a time, in the same process or another; the lock goes with the process; a close frees it whatever a forked child holds (README Status) | `tests/db.rs::a_store_open_in_one_process_is_refused_to_another` (the WouldBlock test at about line 23292); `tests/lock.rs::a_store_closed_while_a_forked_child_holds_its_lock_opens_again`; `src/db.rs::store_lock_tests::a_forked_childs_drop_leaves_its_parents_lock`; the kill tests reopen after the kill (implicit) | built + kill |
| A checkpoint is the store as of the call, its own store; `dest` is absent or whole at every instant; the next checkpoint removes the temp directory; links within a filesystem, copies across (README, `Db::checkpoint`) | `tests/checkpoint.rs` (7 tests), `tests/onefile.rs::a_checkpoint_of_a_store_in_one_file_is_a_store_of_files`, `tests/zpages.rs::a_checkpoint_of_a_stride_store_shares_its_stride_files` | built; the crash state is made by hand; durability after return is untested (L); refused under encryption, which is **undocumented and untested** (M6) |

### Encryption

| Promise (where) | Test(s) | How |
|---|---|---|
| Kernel key: create where the directory can't be encrypted writes nothing; a key given to a plain store is refused | `tests/dirkey.rs::a_store_given_a_key_where_none_can_be_set_writes_nothing`, `a_key_given_to_a_store_in_the_clear_is_refused_and_changes_nothing` | built |
| Kernel key: ciphertext on the device, another key refused, a locked key refused by name, a store kept one way refused the other | `tests/dirkey.rs` 4 tests, `#[ignore]`, run only by `scripts/check.sh fscrypt` (in the `ci.yml` fscrypt job, not the default gate) | built on loop fscrypt |
| Cipher: create wants an empty directory; the wrong cipher or no cipher is refused; a plain store is refused under a cipher; each refusal leaves the directory as found | `tests/cipher.rs::a_store_under_a_cipher_opens_under_no_other`, `an_unpinned_writer_is_refused_under_a_cipher` | built |
| Cipher: a changed byte is refused at open, ends the log as a torn write does, or fails the read and every operation after it; a rollback of the directory is not detected | `tests/cipher.rs::a_changed_byte_is_refused_or_is_where_the_log_ends` (all four outcomes required), `a_log_cut_anywhere_opens_as_the_commits_before_the_cut`, `a_changed_byte_in_a_{form,stride}_file_takes_the_form_out_of_service_and_not_the_store` | built; "every operation after" is held for reads only (L) |
| Cipher: the cache is held to `cache_bytes` | `a_store_larger_than_its_cache_reads_as_itself_within_it`, the builders' tests, the fuzz at a 16 KiB cache (gate) | built + fuzz |
| Cipher: rewrap and key_id | `a_store_handed_to_another_key_opens_under_that_one`, `a_refused_second_open_leaves_the_markers_temp_name_and_the_open_after_clears_it` | built |
| Cipher is Linux only and refused elsewhere before anything is written (ROADMAP) | none | none (L) |
| Envelope: a changed bit, a moved or borrowed chunk, an object cut or grown, or the wrong key is refused; the layout is on record | `src/crypt.rs` tests (`a_changed_bit_anywhere_is_refused`, `a_chunk_moved_or_borrowed_is_refused`, `an_object_cut_short_or_grown_is_refused_at_open`, `the_wrong_key_is_refused_at_open`), `tests/crypt.rs` (8), `web/crypt.test.mjs` | unit + built |

## 2. Fault injection by mode

Each cell names the seam that exercises the fault in that mode, or none.

| Fault | Default store (Always) | EveryN | Held log (`Explicit`) | Store in one file | Cipher |
|---|---|---|---|---|---|
| EIO or partial write of the log | `fail_next_wal_write` (fresh, reopened `O_APPEND`, recycled) | via cipher runs only | **none** | **none** (durable.rs never sets `one_file`) | yes |
| Cut-back of a failed write fails | `fail_next_wal_cut` | — | **none** | none | run path |
| Log fdatasync fails (models ENOSPC) | `fail_next_wal_sync` | `sync()` path | **none** | none | yes |
| Loose-file sync, rotation dir-sync, barrier, guard | yes | yes (runlog) | **none** | none | yes |
| Run file write or sync fails, helper sync | yes | yes (EveryN(5), buffered) | n/a (runs written unsynced) and untested | none | yes |
| Seal data fsync, dir fsync, envelope, presync, landing | yes | — | **none** (frames are dropped at a durable seal) | **none** (the data-file sync through its own handle is the durability point) | yes (envelope) |
| Slot fdatasync fails or slot write torn | yes | — | none | none | yes |
| Open-time syncs fail | yes | — | none | none | yes |
| Torn tail or lost page (built) | yes | yes (runlog oracle tears at any length) | **never torn** (oracle kills explicit rounds without a tear) | data file cut: `a_data_file_cut_under_a_place_its_manifest_names_is_refused`, `a_seal_cut_between_its_landing_and_its_sync_*` | yes (tamper, cut) |
| Real ENOSPC or EIO from a device | none (ROADMAP: dm-flakey items) | none | none | none (`fallocate` growth failure untested) | none |
| Lost directory entry (name taken by a power cut) | guard tests, `a_synced_run_that_is_gone_is_a_record_lost` | runlog | none | n/a | `an_envelope_nobody_names_is_swept_and_one_missing_is_refused` |
| `Encryption::Kernel` | only the refusals, and only on fscrypt | | | | |

How the gate's groups reach these:
- The `onefile` group runs only the tests that build on
  `common::options()`.
- `tests/durable.rs`, the 283-test fault binary, builds on
  `Options::default()`, so none of its seams ever run with `one_file`.
- `tests/kill.rs`, the runlog oracle and `tests/cipher.rs` do run under
  `one_file`.

## 3. Findings

### High

**H1. One store's `SegmentOptions::checksums = false` can switch block checksums off for every store in the process.**
- `SegmentWriter::create` and `resume_run` store the option into
  `block::CHECKSUMS`, a process-wide static (`src/db.rs` lines 13765 and
  14424, `src/block.rs` line 407).
- So a `Db` or a user's `SegmentWriter` that sets `checksums: false` makes
  any other store's seal or merge in flight write zero chunk checksums.
- That store reads with its own `verify_checksums` (true). It then refuses
  its own blocks ("block checksum mismatch", and a merge failure stops its
  segment work). Under the opposite interleaving, blocks are silently stored
  with no checksum.
- This contradicts README's "every data block … checksummed" and the field's
  own doc ("Compute and verify block checksums", given as a per-file setting
  beside `create_with`).
- `tests/checksums_off.rs` says it was moved into its own process for this
  reason, two failures in about twenty gate runs. The hazard was isolated
  away, not fixed or tested.
- The field is public and not `doc(hidden)`.
- **Fix:** carry the flag per writer (in `AlignedWriter`/`BlockBuilder`) and
  delete the static. Or, before the tag, hide the field and refuse
  `checksums: false` beside any other writer.
- **Test:** two stores in one process, one with checksums off, sealing and
  merging concurrently. The checksummed store's blocks must verify, and a
  flipped block byte in it must still fail the read.

**H2. The held log (`SyncPolicy::Explicit`) has no fault-injection test, and its failure semantics are not documented.**
- `SyncPolicy`'s failure paragraphs say "under either policy", meaning
  Always/EveryN. Under `Explicit` the log is written at a `sync`, a rotation,
  the cap (`spill`) or a drop, and no seam test fails any of those writes or
  fsyncs. `tests/durable.rs` mentions `Explicit` only in an `unreachable!`.
- What a commit answers when its cap spill fails is unspecified, and so is
  whether its batch is then visible. The code (`Wal::write`, about line 9787)
  pushes the batch's mark before `spill()`, so the batch looks held and
  visible while the commit returns `Err`. Under Always, a failed commit's
  batch is invisible.
- Untested as well:
  - whether a `sync` after a failed held-frame sync answers the kept error;
  - whether a seal that fails keeps every held frame its manifest would have
    covered (the drop rule).
- The runlog oracle's explicit rounds are killed but never torn, so spilled
  frames that a power cut cuts are untested too.
- **Fix:** state `Explicit`'s failure contract in `SyncPolicy`. Run the
  existing seams under `Explicit` (`fail_next_wal_write` and
  `fail_next_wal_sync` at a `sync`, at the cap and at a rotation;
  `fail_next_seal_sync` with frames held), plus a torn explicit round in the
  oracle.

### Medium

**M1. README's description of `ZPages::Auto` under a cipher is out of date.**
- README.md lines 325-328 say `ZPages::Auto` leaves a store under a cipher
  raw "unless `Options::zpages_under_cipher` is set".
- That option ships `true` (`src/db.rs` line 8705). Its own doc (line 6993
  on) and `Encryption::Cipher`'s doc already say the controller runs there.
- README also says "every option a store in the clear runs with, a store
  under a cipher runs with", while `checkpoint` is refused (M6).
- **Fix:** rewrite the paragraph. The controller runs under a cipher by
  default, the `zpages_under_cipher: false` arms are the shape before, and
  the heads' copy and checkpoints are the exceptions.

**M2. `Db::close`'s doc contradicts `SyncPolicy`.**
- The doc (`src/db.rs` line 54016) says "Close is a convenience, not a
  durability point -- the WAL already made everything durable".
- That is false under `EveryN` and `Explicit`. `SyncPolicy::Explicit`
  promises nothing "until a `sync`, a `flush` or the close", and `close` is
  `flush` + `settle`.
- **Fix:** say that close is a flush and so a durability point under every
  policy, and that a dropped store is not (it writes held frames unsynced).
- **Test:** a store under `Explicit` closed and then crash-copied reads every
  commit. This is close to `a_dropped_store_writes_what_it_held_and_a_closed_one_nothing`.

**M3. `Db::commit`'s doc describes a tail-holding behaviour that the code and tests no longer have.**
- The doc (about line 48602) says "under `EveryN` a run's commit that is not
  due leaves the batch's tail short of a whole write piece with the writer".
- `commit_direct` calls `write_out()` on every commit that is not due
  (about line 49108, "Written, under every policy").
- `SyncPolicy` says a run's commit writes its batch whatever the policy, and
  `tests/kill.rs::a_killed_buffered_writer_loses_nothing_it_acknowledged`
  holds that.
- The two public docs disagree on what a kill takes.
- **Fix:** delete the stale clause.

**M4. The `EveryN(n)` loss bound is held by one weak test, and `docs/engine.md` describes a crash-injection test that is not in the tree.**
- "`EveryN(n)` bounds loss at n batches" (SyncPolicy) is held by
  `tests/db.rs::every_n_loses_the_unsynced_tail_whole_and_never_in_part`
  alone. That test:
  - makes one 5-byte tear of the live WAL, on the WAL arm;
  - takes the 16 synced frames from arithmetic, not from `trace_wal_syncs`.
- `docs/engine.md` "Crash injection" (lines 9390-9411) describes a child
  that aborts with seals and merges in flight, a tear of the unsynced tail,
  120 crashes, "under EveryN(8) the most lost was six batches against a
  bound of seven", and a self-check mode that tears below the synced mark.
- No such test exists. `tests/kill.rs` and the runlog oracle replaced it,
  and neither runs a finite n.
- **Fix:**
  - Add an oracle round (in `tests/runlog.rs`) under `EveryN(8)`, killed,
    with each log file cut to a random length at or above what
    `trace_wal_syncs` says was synced. Assert a prefix that is short of the
    last acknowledged commit by at most 8, plus the self-check that tears
    below the mark and must see a loss.
  - Rewrite the engine.md paragraph to name the tests that exist.

**M5. The store in one file has no fault-injection coverage.**
- `one_file` is a `doc(hidden)` experiment (it is on the roadmap for a
  default), but in it a segment's durability is a sync of the data file
  through the segment's own handle (`vault::File::ride`, `One::sync`).
- No test fails that sync, or a growth `fallocate`, or a short write into an
  extent.
- `tests/durable.rs` builds on `Options::default()`, so the gate's `onefile`
  group runs none of its 283 tests with `one_file`. The 33 `tests/onefile.rs`
  tests build crash states, not syscall failures.
- **Fix:** build `tests/durable.rs`'s seal and slot sections on a helper
  that honours `common::one_file_switch()` (or add `_in_one_file` twins), and
  give `onefile.rs` a failed data-file sync and a refused growth.

**M6. `Db::checkpoint` is refused for every encrypted store, and neither the doc nor a test says so.**
- The code (`src/db.rs` lines 50880-50886) returns `Unsupported` under a
  cipher or a kernel key.
- README's checkpoint section and the `Db::checkpoint` rustdoc promise a
  checkpoint with no caveat, and no test asserts the refusal or that it
  leaves the store unchanged.
- **Fix:** document it in both places, and add a test under a cipher (and,
  in the fscrypt set, under a kernel key) that the call is refused before
  the store is flushed and that `dest` is not created.

**M7. The meaning of `scan`'s `limit` is undocumented, and the roadmap's top item says it is wrong.**
- `Reader::scan` and `read_all` have no rustdoc. README shows
  `db.scan(b"user:", 100, …)`.
- ROADMAP High says `limit` counts deleted keys, so a page comes back short.
- **Fix:** document what `limit` counts until `scan_page` lands, or land the
  fix before the tag, with the differential fuzz paging end to end over
  deletes (that item's "done when").

### Low

- **The `supdb::db` module doc's crash discipline is out of date**
  (`src/db.rs` lines 21-27). It has the seal fsyncing before the rename and
  "then reset the WAL". Seals now publish before their fsyncs, write a
  manifest slot, and keep the WAL, which rotates by size. Line 9 says "There
  is no checkpoint", but `Db::checkpoint` exists.
- **`Db::seal`'s doc** says "rotate the WAL", but seals keep it by default
  (`seal_rotates_wal` is the arm that rotates).
- **`Txn`'s rustdoc** starts with an orphaned paragraph about dropping a
  `Db` (`src/db.rs` lines 54678-54681). Move it onto `impl Drop for Db`. It
  also says a drop "emulates a crash", which is false under `Explicit`.
- **`SyncPolicy::Explicit` overstates the loss.** "A process's death …
  takes the commits since the last of those" is not quite true: commits
  spilled at the cap or a rotation survive it
  (`a_log_over_its_cap_writes_what_it_holds`). Say "may take any suffix of
  them".
- **`docs/engine.md` "What this does not promise"** lists multi-reader
  snapshots as backlog, but `Reader::snapshot` ships.
- **A damaged key index failing `Db::open`** is held only at the Blob level.
  Add a Db-level test: flip a byte of a named segment's key section and
  expect the open refused with the directory untouched.
- **Checkpoint durability after it returns** (the parent directory's fsync,
  `checkpoint_into`) is untested; `trace_dir_io` could hold it.
  `a_checkpoint_on_another_filesystem_is_a_copy` passes silently on a host
  with no second filesystem, a verdict it did not earn: make it fail or
  `#[ignore]` there.
- **A store under a cipher's poisoning** is held for reads only.
  README says "every operation after it", but no commit or flush after the
  poison is asserted.
- **The cipher's refusal off Linux** ("refused before it writes anything")
  has no test. The macOS CI leg compiles the cipher tests out.
- **`Encryption::Kernel`** has no crash or durability test on fscrypt; there
  are only the 4 ignored refusal tests in the fscrypt job.
- **README** says a cipher cache size you set "is held whatever the process
  does beside it". The roadmap's "a read of one key larger than the cache"
  is an exception that README does not state.
- **`Options::sync`**, the most consequential field, has no rustdoc.
- **`SegmentWriter::finish`** fsyncs the file but not its directory. README's
  write-once example implies a finished file; say that the caller syncs the
  directory.
- **Power-cut states are reasoned, not observed.** Under `Always` with a
  kill, neither oracle tears anything, so power-cut states (unnamed seals,
  unsynced ordered indexes, run temp names before their barrier) are held
  only by the hand-built tests in `tests/durable.rs`. This is already on
  ROADMAP as the dm-flakey and dm-log-writes items; it stays known, not new.
- **Two places rely on the engine's own trace, not the kernel.** The
  checks that the open syncs a slot, and that a reopen with nothing replayed
  makes no fdatasync, read the engine's trace (ROADMAP: "A reopen with
  nothing replayed held to no fdatasync at the syscall").
- **The doc comment of `tests/runlog.rs::torn`** says "`Db::crash` leaves a
  store's threads running". That is no longer true: `Life` ends them.

## 4. Recovery: open time and recovering a crashed store

**What is measured.**
- `bench` workloads `recover` and `recover-shuffled` (`bench/DESIGN.md`
  lines 26-27 and 196-236): the writer is killed with SIGKILL in a process
  of its own (`bench crash-load`), with one batch staged.
  - The row records `open ms`, `first read µs` and, for buffered arms,
    `unrecovered_batches`.
  - The reopened store is held to the acknowledged prefix outside the clock;
    a durable arm that misses a batch fails the run.
  - These run only at the in-core scales. Bounded rows and pairings run them
    only with `--passes recover`, so out-of-core recovery is not in the
    series.
- The probe is `bench/examples/reopen.rs`. Its figures are in
  `docs/engine.md`:
  - 754M → 180M instructions for an open after an ordered load at 300k keys,
    62-75 ms;
  - 90-115 ms after a shuffled load.

**What is tested.**
- The crash windows of the recovery itself:
  `a_run_an_open_takes_is_finished_where_it_lies` and
  `an_open_cut_short_in_a_runs_finish_is_made_again` (four windows, each
  opened to the same store and held byte for byte to the segment a clean
  close makes).
- What the open reads:
  - `an_open_opens_each_slot_file_once*`
  - `a_plain_open_finds_its_named_segments_in_its_listing`
  - `an_open_under_a_cipher_looks_for_each_named_envelope`
  - `an_open_after_a_crash_reads_only_the_segments_over_an_unnamed_segments_keys`
  - the cipher open "held to a handful of chunks a file", in
    `a_store_larger_than_its_cache_reads_as_itself_within_it`
- **No test bounds open time or its I/O volume.** Regressions are caught
  only by the gate's window over `recover`'s `open ms` in quick rows.

**What is open on the roadmap.**
- "A cheaper verification at open, and a way to skip it" (High):
  - Every open except a cipher's (`Bytes::authenticated`) verifies each
    segment's whole key section against its checksum row. With inline
    records that is about the whole segment: four fifths of an open's
    instructions, about 90 ms at 3M keys, so open time is linear in store
    size.
  - There is no option to skip it yet. When one lands, README's "a damaged
    key index fails the open" becomes conditional and must say so.
  - The roadmap's done-when needs a test of the option.
- "An open after a crash, priced by its parts": an open that finds a merge
  held at its landing costs over half again. The unnamed-segment check
  opens each candidate whole for one key.
- "The open after a crash, under a cipher": 1.2x-2.6x the plain store's in
  the encrypted quick row. Runs are replayed from the log; phases unstamped.
- "A reopen with nothing replayed held to no fdatasync at the syscall".

The open's verification is fail-closed for availability: one damaged key
section refuses the whole store. Nothing documents a recovery path for that
case, such as an option to open a store with one segment set aside.

## Memory

The user memory was empty, so nothing above comes from it.
