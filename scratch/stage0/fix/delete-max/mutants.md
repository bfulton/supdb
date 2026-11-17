# delete-max: mutants

The fixed files were kept in keep/ (db.rs, durable.rs, with sha256 sums in
keep/sums) before the first mutant. Each mutant was applied to src/db.rs,
`cargo test --profile checked --test durable` was run (log: m-<name>.log,
the mutation itself: m-<name>.diff), and src/db.rs was copied back from
keep/ and checked against the sums after each one (all OK).

| mutant | source | verdict | killed by |
|---|---|---|---|
| a. the raise in `Db::delete` removed (the engine at 3df85d22) | report | killed | an_append_after_a_delete_survives_a_crash_before_the_deletes_seal_writes, ..._between_the_deletes_seal_phases, an_append_batch_..., an_append_after_a_transactions_delete_..., an_append_after_a_delete_that_ends_a_run_..., a_buffered_crash_never_recovers_a_batch_without_its_first_key (6 of 9) |
| b. the raise only on the non-ordered path (`if ordered { leave_direct() } else if key > max { raise }`) | report | killed | an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes (alone; the test was added because no other test deletes while a run is open) |
| c. `goes_direct`'s `key <= max_key` changed to `key < max_key` | report | killed | 7 of 9: every append test above, plus an_append_after_a_replayed_delete_... |
| d. `max_key_of` skipping memtable entries whose live count is zero (a tombstone at the head) | report | killed | an_append_after_a_replayed_delete_survives_a_crash_before_its_seal_writes (alone) |
| e. `>` flipped to `<` in delete's raise | own | killed | the same 6 as (a) |
| f. delete's `extend_from_slice` deleted (the greatest key cleared to nothing) | own | killed | the same 6 as (a) |
| g. `max_key_of`'s memtable loop removed | own | killed | an_append_after_a_replayed_delete_survives_a_crash_before_its_seal_writes |
| h. the raise moved above `writes_go` and `leave_direct` (swapped order) | own | survives: equivalent | none; `leave_direct` never reads or writes `max_key`, and raising it for a write that `writes_go` then drops only makes the guard stricter (a later append below that key goes to the log, which is always sound). No test can tell the two orders apart by an outcome, so none was added. |

Tests that kill nothing on their own: the put control and the cipher test
(both pass on the unfixed engine; see notes.md).

## Repair round 1 (the reviewers' extra mutants)

`keep/db.rs` was refreshed to the tree's db.rs first (it predated the
`Db::delete` doc comment, which mut.sh would have reverted; nothing else
differed), with `keep/dbsum`. Driver: `apply.py <m>` makes the mutation,
`mut2.sh NAME TESTS...` runs the named binaries under `--profile checked`
and copies db.rs back, checked against the sum after each (all OK);
`chain.sh` ran them in order. Two tests were added to durable.rs first
(`an_append_after_a_delete_second_in_its_batch_...`,
`an_append_after_a_lone_delete_...`); both fail on the engine with the
raise removed (m-a2-raise-removed-new-tests.log, appended to before.log),
which is mutant (a) again: killed by 8 of 11.

| mutant | source | verdict | killed by |
|---|---|---|---|
| i. `max_key_of`'s segment loop removed | reviewer | survives durable.rs (11/11); killed in tests/db.rs | db: a_crash_before_the_manifest_lands_keeps_the_pre_merge_store ("merge slices leave a key unassigned": a run opened below the reopened store's greatest, its piece named against the last partition's fence). Held by db.rs, so no durable.rs test was added. |
| m. in that loop `k > best` flipped to `k < best` | reviewer | survives durable.rs; killed in tests/db.rs | db: a_crash_before_the_manifest_lands_keeps_the_pre_merge_store, the_oracle_holds_under_the_shape_with_seals_that_keep_the_wal |
| j. delete's raise guarded by `!self.mem().is_empty()` | reviewer | killed | 7 of 11: every test whose delete is first or alone in a table emptied by the flush (Delete, Alone, Txn, AppendBatch, BetweenPhases, ends-a-run, buffered) |
| k. delete's raise guarded by `self.wal.pending.is_empty()` | reviewer | killed | an_append_after_a_delete_second_in_its_batch_survives_a_crash_before_its_seal_writes (alone; added this round, since every earlier test deleted the key first in its batch) |
| l. the raise moved after `self.wal.delete(key)` | reviewer | survives: equivalent | none. `prefetch` and `Wal::delete` are calls on the memtable and the log, which hold no reference to `Db::max_key`; nothing between the two positions reads it. |
| n. append_pinned's direct branch: `self.max_key.clear(); self.max_key.extend_from_slice(key);` deleted | reviewer | survives durable.rs; killed in tests/db.rs | db: about thirty tests (the oracle family, the_forms_survive_a_direct_runs_start, the_extension_orders_a_batch_alike_through_sixteen_bytes_on_the_merge_path, ...); one of them hung and the binary was stopped by hand after 37 minutes, the mutant already dead |
| o. delete's raise as `key >= self.max_key` | reviewer | survives: equivalent | none. On an equal key the clear and copy write the same bytes back. |
| p. the raise moved after `self.mem().delete(..)` | reviewer | survives: equivalent | none. `MemTable::delete` holds no reference to `Db::max_key`, and nothing after it in `delete` reads it. |

Equivalents (h, l, o, p) are positions or forms of the same raise inside
one `&mut self` call with no reader of `max_key` between them; no test can
tell them apart and none was added.

## Repair round 2 (the reviewers' extra mutants)

Driver: `apply2.py <m>` makes the mutation, `mut3.sh NAME TESTS...` runs the
named binaries under `--profile checked --no-fail-fast` and copies db.rs
back, checked against `keep/dbsum` after each (all OK); `chain2.sh` ran the
first nine in order. Three tests were added to durable.rs this round
(below); the a3 run is mutant (a) again on the tests as they stood then:
9 of 14 killed, the new `..._its_seal_commits_...` among them.

| mutant | verdict | killed by |
|---|---|---|
| q. delete's raise made unconditional (`clear; extend`), lowering the greatest at a delete below it | killed | an_append_after_a_transactions_delete_... (the Txn's second delete, of key(10), lowers it before key(500000) is appended), and a_delete_below_the_greatest_leaves_it_where_it_was (added: "merge slices leave a key unassigned" at the flush, a run opened at key(6) across partitions). tests/db.rs does not kill it (m-q-unconditional-raise-db.log, 226/226). |
| r. the raise guarded by `!self.mem().ordered` and placed above the `leave_direct` block | killed | an_append_after_a_delete_that_ends_a_run_... (alone) |
| s. `max_key_of`'s memtable loop takes the last entry only | killed | an_append_after_a_replayed_delete_... (alone): the replayed table holds the tombstone of key(500000) and then key(10)'s put, so the last entry is key(10) |
| t. the raise skipped under a cipher (`self.vault.is_none() && ...`) | survives: equivalent | none. Under a cipher a run's commits are logged too (`RUN_LOGGED`, `log_run`), behind the tombstone, and the run's file is a memory file a crash loses, so recovery replays both from the log in order; the cipher test passes on 3df85d22's engine and under this mutant. Kernel stores have no vault and take the raise. No cipher window was found where it matters. |
| u. the raise moved to `Db::commit` (the batch's greatest deleted key kept in a field and raised there) | killed | an_append_after_a_delete_its_seal_commits_survives_a_crash_before_the_seal_writes (added: the delete is staged and the seal commits it with no `commit`; `seal`, `sync` and `flush` commit through `commit_staged`/their own path, not `commit`). The reviewers' suggested killer, a delete and an append of the same key in one batch, cannot kill it: `direct_can_open` requires `self.wal.pending.is_empty()`, and a staged delete leaves a pending frame, so no append in the delete's batch opens a run; nothing between a delete and the call that commits its batch reads `max_key`. A variant raising in every committing path is equivalent for that reason. |
| v. the raise guarded by `!self.mem().ordered` after `leave_direct` | survives: equivalent on every reached path | none. `leave_direct` ends in `close_direct`, which installs a hashed table (`set_mem(MemTable::new)` with nothing committed, `freeze` -> `freeze_table` otherwise), so the guard is true wherever `close_direct` succeeds. It differs only when `land_seal` fails inside `close_direct` (segment work stopped): the delete then stays in the ordered table, `pending_err` is set and the next commit returns it; no seam fails `land_seal` and no test reaches it. |
| w. `max_key_of` takes each segment's first key | survives durable.rs and tests/db.rs (m-w-segs-first-key-db.log, 226/226): equivalent in outcome | none. The maximum of the segments' first keys is the last partition's lower fence or above (partitions tile; a piece's first key only raises it), and the memtable, every frame a recovery replayed, is still counted, so the run it lets open lies inside the last partition's range and above every frame of the log. A probe (a run opened at key(90) in a partition [key(80), ...), flushed, merged, killed, reopened) read every key back; the run is a piece over the last range, ordered above the partition. The bound the order needs is the last partition's fence and the log's keys; the segments' last keys are stricter. Recorded as a survivor that no outcome can tell apart. |
| x. the raise moved above the `writes_go` check | survives: equivalent | none, as (h): it differs only for a delete `writes_go` drops (segment work stopped), where a raised greatest only sends later appends through the log, and the next commit returns the error. |
| i. (round 1) `max_key_of`'s segment loop removed, run again | killed in durable.rs now | an_open_keeps_the_greatest_its_segments_hold (added: the reopened store's greatest falls to nothing, key(6) opens a run across partitions, the flush's merge refuses it) |
| m. (round 1) that loop's `>` flipped, run again | killed in durable.rs now | an_open_keeps_the_greatest_its_segments_hold |

The two tests added for q and i/m (`a_delete_below_the_greatest_...`,
`an_open_keeps_...`) pass on 3df85d22's engine, which neither lowers nor
recomputes the greatest: they are guards against those mutants, not evidence
of the defect. A first version of them over one partition (keys 0..1000 by
one run) killed nothing: a run below the greatest inside one partition's
range reads right, and only one that spans partitions is refused; they load
several partitions now (small seals, `partition_bytes: None`).

## Repair round 3 (the reviewers' extra mutants)

Driver: `apply3.py <m>` makes the mutation, `mut3.sh NAME durable` runs the
binary under `--profile checked --no-fail-fast` and copies db.rs back,
checked against `keep/dbsum` after each (all OK); `chain3r.sh` ran y1-y7 in
order (chain3r.log). src/db.rs did not change this round; durable.rs did
(the ends-a-run test's tail, below), and keep/durable.rs and keep/dursum
were refreshed to it before y2b, y5b and a4.

| mutant | verdict | killed by |
|---|---|---|
| y1. delete's raise only when `self.opts.direct_ingest` | survives (14/14): equivalent, as the reviewers expected | none. With `direct_ingest` off no write opens a run (`goes_direct` asks it first), so `max_key` guards nothing; a reopen under `direct_ingest: true` recomputes the greatest from the segments and the replayed log (`max_key_of`), tombstones included, not from the field. |
| y2. `leave_direct` drops the tail's `self.wal.append` | survived the tests as they stood (14/14, m-y2-...log): the ends-a-run test committed the load before its delete, so the run had no tail. Killed after the test was given one (m-y2b-...log) | an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes, at durable.rs:458, "ends-a-run, the first open: the run's tail, committed with the delete that ended the run, was recovered", left [] (alone). The test now appends key(1000)..key(1009) to the open run uncommitted before the delete, and reads them live and at two opens of the crash copy. |
| y3. delete's raise guarded by `self.mem().ordered` | killed | 9 of 14: every append-after-delete test but the replayed one (durable.rs:239) and the buffered one (:535). After `leave_direct` the table is always hashed, so the raise never runs. |
| y4. compare by length (`key.len() > max_key.len()`) | killed | the same 9 as y3: `key()` makes fixed-width keys, so the raise never fires, as the raise removed |
| y5. `Txn::commit` applies a staged delete by `wal.delete` + `mem().delete` | killed, but by the writer's debug assertion "the writer reads the state only inside an operation" (src/db.rs:17959), not by an outcome | an_append_after_a_transactions_delete_... ; so it was run again as y5b |
| y5b. the same, inside `let _op = db.op()` | killed | an_append_after_a_transactions_delete_survives_a_crash_before_its_seal_writes (alone), at durable.rs:239, "plain-BeforeWrite-Txn-Append, the first open: a write acknowledged after the key's delete reads back after the crash", left [] |
| y6. `max_key_of`'s memtable loop with `>=` | survives (14/14): equivalent | none. On an equal key `best` takes the same bytes. |
| y7. delete's raise as `extend_from_slice` then `clear` (the greatest emptied) | killed | the same 9 as y3 (an emptied greatest lets every later append open a run) |

a4: mutant (a), the raise removed, on the tests as they stand after this
round: 9 of 14 fail, the ends-a-run test still at durable.rs:239 (the
crash's final assertion, before its tail assertions).
