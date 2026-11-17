# Stage 0: the plain store's durability defects, reproduced (tree 3df85d22, nothing fixed, nothing committed)

I couldn't save report.md: the harness refused the Write. The report is here instead. Nothing was fixed and nothing was committed; the tree is clean at 3df85d22, and stage0/tmp has been removed.

The combined patch is 5,364 lines: 15 new test files under tests/s0_*.rs plus seams in src/db.rs and src/db/vault.rs. It applies cleanly to 3df85d22 (checked against HEAD's two engine files).

**Build and run**
- `cargo test --profile checked --no-run` built the 15 new test binaries, `--lib` and `--test db` with no errors and no warnings.
- Each test was listed with `--list` and run alone with `--exact` under its own TMPDIR.
- 46 tests fail at the assertion their reproducer named, each with a "test result: FAILED" line. None was a watchdog exit.
- 2 controls pass as intended: `an_open_with_both_slots_zeroed_refuses_and_leaves_every_file` and `a_put_after_a_delete_survives_a_crash_before_the_deletes_seal_writes`.
- 11 existing tests/db.rs tests next to the seams pass, and 2 lib tests pass: `a_long_segment_is_synced_behind_its_writer` and `the_engine_names_no_file_but_through_the_door`.
- `scripts/fmt.sh`: I ran it once in apply mode by mistake. It reformatted only this patch's files (`git status` showed nothing else), and `--check` then exited 0. The saved patch is the formatted one.

**Merging the seams by hand**
- seal-after-failed-sync and manifest-pending-rewrite both added `Shared::fail_seal_sync` / `Db::fail_next_seal_sync` at the same site. One copy is kept, seal-after's, which also covers the direct run's close. manifest-pending's `open_synced` was added by hand.
- sync-completion-order's local `open_synced: u64` shadowed manifest-pending's `Vec`, so it is renamed `wal_open_synced`.
- Fuzzy application had put `fail_helper` inside a doc comment and after a struct's closing brace, `owed_slot_hold` inside `seal_wedged`'s doc, and the seal-seam initialisers outside the Shared literals. I moved all of them back. The `RUN_FAULT_*` constants now sit above AlignedWriter's doc, as the skeptic asked.
- All seams are `#[doc(hidden)]` with no cfg, like the existing ones.

**Verdicts I changed or decided**
- both-slots-unreadable is not a durability defect. The skeptic is right: create syncs a slot before any segment exists, and no engine path empties a slot. The test builds the state by deleting synced bytes. The tests stay as hardening: the open deletes 13 par-, 13 pcs- and 26 ord- files and answers Ok, where it should refuse.
- sync-completion-order: the named mechanism (a mark trusting a sync that completed out of order) is refuted by reading. What was reproduced is a different defect, "replayed frames marked synced": Wal::open_append sets synced = written for frames read from the page cache after a kill. File it under that name. Its fix also has to put replayed older log files on loose_wals; otherwise a chained live header beside an unsynced predecessor makes a power cut refuse the open.
- slot-damaged-rollback test 2 tests "the open sweeps, then refuses". It does not test the silent WAL rollback its doc names. That half is real, so the test stays.

**Corrections to the tests**
- run-write-fail's fdatasync test now ends by asserting that sync() returns Err. The seam fails before the fdatasync is issued, so a retry fix would have passed; on Linux a retry answers Ok over pages already marked clean. Today the test still fails first at the k2 read-back.
- wal-sync-fail's doc now says the copy stands for a power cut after the second sync, not a process kill.

**Each defect, and the mutants its fix's tests must fail under**

wal-write-fail. A failed log write leaves the batch in `pending`. The next commit frame's CRC then covers two batches, replay rejects it, and a commit acknowledged after the failure is lost. A partial write shifts the file. A rotation makes the open refuse with "wal sequence gap".
- Fix: take the CRC from `batch_from`, cut back to `written` on failure, and latch if the cut fails.
- Mutants: CRC over all of pending; no cut back; `batch_from` not reset; no latch on a failed cut; the failed batch dropped (add an assertion that k1 appears exactly once).

wal-sync-fail. A failed fdatasync is forgotten, and the next commit or sync answers Ok over lost frames.
- Fix: a sticky writer error, set by every sync error in sync_wal and cleared only by a reopen or a covering durable landing.
- Mutants: latch not set; commit or Db::sync not checking it; cleared by the next fdatasync; take()n; not set by a loose-file or directory error (needs sibling seams).

wal-fresh-header. A log no sync ever covers sits beside acknowledged state held elsewhere. If its header comes back as zeros, every open refuses. This is rare on ext4/xfs defaults.
- Fix: replay_frames reads 8 zero bytes as an empty log.
- Mutants: any foreign bytes accepted; file not rewritten; zeros accepted only at length 8; only the first file; replay continued past the zero header.

run-write-fail. A failed tail write or fdatasync leaves the batch counted, so the next commit() and sync() answer Ok with no I/O. A failed piece write wedges the run, which then refuses with "keys must arrive in strictly increasing order". A partial write shifts the cursor and a later Ok commit is lost.
- Fix: pwritev at `written`; an owed flag at commit_direct's early return; a sticky run error on a failed fdatasync; a failed piece stays held.
- Mutants: cursor writev restored; owed flag cleared early; the latch replaced by a retry; mark_stream still returning the piece error.

helper-presync-error.
- H1: SegmentWriter::sync never reads the helper's stored EIO.
- P1 (EveryN): the close's presync error never reaches the seal job, so the next flush names and acknowledges the segment.
- P2 (close_finish_inline): the presync is joined after seal_readable.
- Fix: load the helper's error in sync; the seal job waits on the presync's result before seal_readable; the inline join moves above the link.
- Mutants: the error load deleted; no wait; the join after seal_readable; the poison cleared after the first Err.
- A Db-level H1 test under Always is still owed.

sync-ordered-buffered. Under EveryN(n>1), Db::sync never writes or syncs an open run's tail, skips sync_wal under an ordered table, and zeroes the unsynced count.
- Fix: Db::sync calls d.w.sync() on an open run and drops `!mem().ordered` from the gate.
- Mutants: the tail written but not synced (the power-cut test catches it, the kill test does not); the gate kept; the sync placed after hand_tail; gates that ignore the held stream or use the unsynced counter.
- By reading, seal() under EveryN leaves the sealed table's frames unsynced. That has no test yet and should be filed separately.

run-tail-kill. close_run leaves the held tail for the seal thread, while a later log commit is written at once. A kill in between keeps the later commit and loses earlier ones, which breaks the prefix rule.
- Fix P1: write the tail on the writer's thread before hand_seal, on every exit (leave_direct, Db::seal, hand_tail).
- Mutants: the tail written only on the seal thread; one path of stream/copies only; the write in close_direct only; whole pieces only; the write moved into the job.
- Its second test needs P3, which is sync-ordered-buffered's fix.

delete-above-max. Db::delete does not raise max_key. An append of a key deleted above the greatest opens a run, and after a crash the replayed tombstone masks the acknowledged append.
- Fix: raise max_key in delete as put does. The reproducer applied this temporarily and all five tests passed.
- Mutants: the raise removed; the raise on one path only; `<=` changed to `<` in goes_direct; max_key_of skipping memtable tombstones (needs a reopen-then-seal test).

owed-slot. A drain's unsynced slot is followed by a publish that overwrites the other slot by parity, which is the last durable one, and drops the owed sync. A torn write then refuses the open. This rests on a slot write being torn.
- Fix: choose the slot by durability, and at open fdatasync the slot that was read.
- Mutants: parity restored in publish; parity restored in publish_unsynced; no open sync; durable_slot set at issue rather than after the sync; sync_owed_slot not updating it.
- a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid needs its expectation restated.

seal-after-failed-sync. A data fsync that fails after the readable landing does not wedge (`if !landed`), so the next manifest names the segment and covers its sequence. The directory-sync case is a breach of the engine's own comment, not a likely loss on ext4/xfs.
- Fix: wedge always; every publish path refuses while wedged.
- Mutants: `!landed` restored; the check missing from publish, publish_unsynced or the pending path; a retried fsync (the fix's test needs a sticky seam or a seals_wedged assertion); a seal landing that retires a rotated log; the direct-run close (seam present, untested).

manifest-pending-rewrite (arm). The open rewrites verified-in-cache pending names as durable with no fsync, and Maint::stop names a failed seal durable.
- Fix: fsync the pending segments, their ord files and the directory before the durable slot; stop publishes only over seals that returned Ok.
- Mutants: no directory sync; no segment sync; first name only; ord file omitted; open_synced recorded without a syscall (needs a counting seam); stop discarding the Err.

run-temp-dir-barrier (hardening). The run's temp file is acknowledged by its own fdatasync with no directory fsync. No loss is expected on ext4, xfs or btrfs; I read that and did not observe it.
- Fix: sync_dir right after the create.
- Mutants: deleted; moved before the create; first run only; in a lazy form, the flag cleared on failure or not paid in Db::sync.

slot-damaged-rollback. The open falls back past a damaged slot without checking it, and sweeps before deciding. It answered Ok with 2000/2000 flushed keys gone. The state is media damage; the crash route is through owed-slot.
- Fix: no unlink until a slot is chosen; refuse a fallback with an absent name, or an unnamed segment beyond its coverage (end_seq, or id >= next id).
- Mutants: sweep moved first; absent-name check deleted; later-segment check deleted; `>` for `>=`. The refusal's wording needs a specific test, because "contains manifest" is satisfied by an accidental NotFound today.

both-slots-unreadable (hardening). Fix: refuse before any removal when no valid slot stands beside par-/pcs- files, or an empty slot stands beside segments.
- Mutants: check deleted; check placed after the sweep or after make_slots; par- only; refusing whenever no slot stands (a_store_from_before_manifests_opens_from_its_segment_names catches that one).

replayed-marked-synced (filed as sync-completion-order). Fix: synced = head for replayed frames; replayed older files go on loose_wals.
- Mutants: synced = written restored; older files not made loose (needs a two-file test); frames_unsynced ignoring head; the deferral ignoring wal_unsynced; a CHAINED rotation after a reopen. Also a control that a clean reopen issues no fdatasync.

**The owner's decision that the log and the runs share one sequence**
- run-tail-kill: P1 alone fixes the kill case. The power-cut sibling (the run's file unsynced while a later log commit is synced) needs either a sync order or the shared sequence: commit numbers in run markers and log frames, plus the run's opening number in its head, so recovery cuts both logs at the first gap. That keeps a prefix only by dropping the later commit, so P1 is still needed.
- delete-above-max: the shared sequence is the structural fix. Pieces are ordered against replayed frames by sequence, so the older tombstone cannot mask the append, and stores already on disk are repaired. The max_key line costs nothing and should go first anyway.

**Not reproduced, and what it would take**
- run-tail-kill's power-cut sibling: possible now, with direct_run_synced from sync-ordered-buffered's seam.
- The crash route of slot-damaged-rollback: owed-slot's watch seam plus a tear of the other slot.
- The silent WAL rollback at rest: three attempts did not reach it; it is likely reachable only mid-life.
- Sibling sync sites: wal-sync-fail's loose files and directory barrier; helper H2/H3; the direct-run close's sync_all.
- The cipher arms.
- A real errseq: no test shows one; it needs a dm-error or dm-flakey device under the store, with root and a loop device, as scripts/fscrypt.sh sets up its filesystem.
- seal() under EveryN leaving frames unsynced.
- Rotation under Always with the new header zeroed.

**What I did not verify**
- I did not re-derive the mechanisms from source. They are the reproducers' and skeptics' reading at 3df85d22; I re-read their problems and the logs, and re-ran every test from the combined patch.
- The filesystem likelihoods (torn slot, zero header, lost directory entry, errseq) are reasoned, not observed.
- wal-write-fail's 5- and 20-byte cuts ran only by hand edit in stage 0; this run stops at the 1-byte cut.
- wal-write-fail's claim that k1 was absent was never printed; by reading, k1 should be recovered.
- I did not build bench/ or experiments/. Their SegmentOptions literals end in ..Default::default().
- The full tests/db.rs binary was built, not run to completion; 11 of its tests next to the seams were run and pass.

## Table

Policies: Always is the default; EveryN(n>1) is the shipped buffered policy. "Default" below means Options::default().

| defect | reproduced | config | severity | fix | write-path cost |
|---|---|---|---|---|---|
| wal-write-fail | yes (4 tests fail as stated) | default (Always); EveryN sync() too | acknowledged data loss, or a refused open, after a device write error | latch; better: batch CRC from batch_from, cut back to written on failure, latch if the cut fails | one usize per commit; ftruncate+lseek on failure only |
| wal-sync-fail | yes (2), power-cut model | default; EveryN sync() | acknowledged data loss after an fsync EIO | sticky writer error, never cleared by a retried fdatasync | one load+branch per commit, folded into the pending_err check; no syscall |
| wal-fresh-header | yes (3), under the stated model; rare on ext4/xfs defaults | default | refused open at every open | replay_frames reads an all-zero 8-byte header as an empty log | none (one compare at open) |
| run-write-fail | yes (5) | default (Always) | acknowledged data loss; wedged run with the wrong error | pwritev at written; owed flag; sticky run error on a failed fdatasync; failed piece stays held | none on success |
| helper-presync-error | yes (4) | H1 default writer (tested at EveryN(2)); P1 EveryN; P2 close_finish_inline arm | acknowledged data loss after an fsync EIO | sync reads the helper's error; the seal job waits for the presync before its readable landing | one atomic load per synced run commit; a wait in the seal job (needs pricing) |
| sync-ordered-buffered | yes (4) | EveryN(n>1) with a run open; test D needs adaptive_shape | acknowledged loss after an Ok sync(), deterministic | Db::sync syncs the open run; drop !mem().ordered from the sync_wal gate | under EveryN, one fdatasync per sync() with a run open |
| run-tail-kill | yes (2) | EveryN | prefix violation under a process kill; Ok sync() lost | P1: close_run writes the held tail on the writer's thread; P3 for test 2 | under EveryN, one write of under 1 MiB per run close, moved onto the writer (needs pricing) |
| delete-above-max | yes (4 fail, put control passes) | default | acknowledged append lost (Always); a state the store never held (EveryN) | delete raises max_key as put does | one compare per delete |
| owed-slot | yes (3), given a torn in-place slot write | default | refused open after a power cut, acknowledged commits unreachable | choose the slot by durability; fdatasync the slot read at open | none per commit or drain; 1-2 small fdatasyncs per open |
| seal-after-failed-sync | yes (2); case 2 is the real one | default | acknowledged data loss after a data fsync EIO | wedge whether or not the seal landed; every publish refuses while wedged | one atomic load per publish |
| manifest-pending-rewrite | yes (2) | manifest_pending arm (off) | refused open after kill, reopen, then power cut | open fsyncs pending segments and the directory; stop publishes durable only over seals that returned Ok | recovery only |
| run-temp-dir-barrier | yes (3); the barrier is missing | default | hardening: no loss on ext4/xfs/btrfs | sync_dir after the run temp is created | one directory fsync per run (needs pricing) |
| slot-damaged-rollback | yes (2) by media damage; crash route via owed-slot | default | silent rollback, or the data deleted and then the open refuses | no unlink before the slot is checked; refuse an inconsistent fallback | none |
| both-slots-unreadable | not a durability defect: hardening (2 fail, zeroed control passes) | any | destructive handling of outside damage | refuse before removal when no valid slot stands beside par-/pcs- files | none |
| sync-completion-order | partly: named defect refuted; "replayed frames marked synced" reproduced (4) | default; EveryN | loss after kill, reopen, Ok sync/flush, then power cut | synced = head for replayed frames; replayed older files go on loose_wals | none in steady state; one fdatasync on the first sync after an open that replayed frames |

## Fix order

- 1. delete-above-max: one line in Db::delete that raises max_key. It is independent of everything else, costs nothing, and the shared sequence later makes it structural.
- 2. One sticky writer latch shared by the log and the run, checked where commit already checks pending_err and never take()n: wal-sync-fail, wal-write-fail, run-write-fail's fdatasync half, and helper-presync-error H1 (SegmentWriter::sync reads the helper's error). Then wal-write-fail's batch_from CRC and cut-back, and run-write-fail's pwritev, owed flag and held failed piece.
- 3. The log at open, in Wal::open_append and replay_frames: replayed frames marked synced (synced = head, older replayed files go on loose_wals), and wal-fresh-header's zero-header rule.
- 4. Db::sync, close_run and commit_direct: sync-ordered-buffered's P3 first (it also fixes run-tail-kill test 2), then run-tail-kill's P1, then run-temp-dir-barrier.
- 5. Seal landing after a failed sync, in land_front, Maint::publish*, close_run's two arms and Maint::stop: one wedge rule for seal-after-failed-sync, helper-presync-error P1/P2, and manifest-pending-rewrite's close half.
- 6. Manifest slots and the open's order, in publish, publish_unsynced, sync_owed_slot and Db::open's slot section: owed-slot (slot chosen by durability, sync at open), then move every unlink in the open after the slot is chosen and checked, which covers slot-damaged-rollback, both-slots-unreadable and manifest-pending-rewrite's open half.
- 7. Later, with the owner's shared log/run sequence: run-tail-kill's power-cut sibling, and replay that orders run pieces against log frames by sequence (which also repairs delete-above-max stores already on disk).

## Needs the owner

- The latch (wal-write-fail, wal-sync-fail, run-write-fail's fdatasync half, helper H1) is a contract change. After a device error the store refuses every commit, sync and flush until it is reopened, so a transient ENOSPC stops the writer. A sync error must latch in any case because of errseq; a write error could instead be retried with the batch_from CRC and the cut-back. No new syscall, but the commit-path check should be counted under callgrind against origin/main (bench/examples/load_only.rs, work_probe.rs) by function.
- seal-after-failed-sync's wedge extends to every manifest publish, merges included: after a failed seal fsync the store lands nothing until it is reopened. This is a contract change, with no write-path cost.
- owed-slot changes the slot choice from parity to durability, which changes the existing test a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid. The preferred fix adds 1-2 small fdatasyncs at open only. The alternative (sync the owed slot before the next publish) adds an fdatasync inside a drain and would need pair.sh beside supdb-waitmanifest with SUPDB_DRAIN_STEPS=1 at 10000 and 300000.
- slot-damaged-rollback and both-slots-unreadable turn opens that today answer Ok into refusals. That is a contract change for stores already damaged on disk; no write-path cost.
- manifest-pending-rewrite is the arm supdb-manifestpending / supdb-ingestmanifestpending. Fix it (recovery-only fsyncs, plus a check at close) or remove the arm. Removing it does not fix the default store's analogue, which is seal-after-failed-sync.
- run-temp-dir-barrier adds one directory fsync per run on the writer's commit path. Price it with bench/scripts/pair.sh, base against fix, --passes load at 10000 and 300000, durable and buffered ordered loads, with SUPDB_LOAD_PHASES=1 for the run's first commit.
- sync-ordered-buffered (P3) makes Db::sync issue the run's fdatasync under EveryN, which the doc already promises. First check whether the buffered arms' adapter in bench/src/engines.rs calls Db::sync (not verified), then price the buffered ordered load's closing sync with pair.sh and SUPDB_LOAD_PHASES=1.
- run-tail-kill (P1) moves the tail write, under 1 MiB, onto the writer at each run close, and splits the last piece into two writes, which changes its folio composition. Price it with pair.sh on the buffered ingest arms at 10000 and 300000, --passes load and the read passes.
- helper-presync-error P1 makes the seal job wait for the presync before the readable landing, which can move the landing later in a drain. Price it with pair.sh on the ordered load, durable and buffered, at 10000 and 300000, with SUPDB_DRAIN_STEPS=1.
- The shared log/run sequence, already decided, is a format and recovery change. run-tail-kill's power-cut sibling and the structural form of delete-above-max wait on it. It adds about 8 B per run marker and needs its own crash tests.

## Skeptics, per defect

### wal-write-fail (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes. I checked the syscall order by reading 3df85d22; I compiled and ran nothing. Under Always, Db::commit runs mark_commit, then Wal::write (write_all), then mem().commit(), then sync_wal (fdatasync), then rotate_wal_by_size (db.rs:4430-4456, 28495-28513).

When write_all fails, `?` returns before `written` moves and before pending.clear(). The failed batch B1 and its commit frame C1 stay in `pending`. The next commit's mark_commit takes the batch CRC over all of pending, B1+C1+B2 (db.rs:4391-4392). That write and its fdatasync succeed, so the file holds B0 C0 B1 C1 B2 C2, all covered by an fdatasync before k2's commit returned Ok.

The test copies the live store after k3's fdatasync. A power cut must keep every byte of that copy and a kill keeps it too, so the test drops nothing a sync covered.

The failure itself is real. write(2) on ext4 or xfs returns ENOSPC, EDQUOT or EFBIG, or rarely EIO. A short count followed by an error is exactly what write_all leaves: the prefix on the file and the append cursor past it. The seam reproduces that file state by writing the prefix and then returning an error. ENOSPC is also transient (the engine itself unlinks files), so a later commit that succeeds is plausible.

Rotation case: wal-00000001 exists in the copy. A kill keeps its name whether or not the directory was fsynced, so the refusal holds at least for a process kill. I did not check whether a directory fsync follows the rotation, so I make no power-cut claim about that name.

The open's own comment at db.rs:27925-27927 ("left in place, its records would sit in front of the next batch's commit frame and be adopted by it") shows the engine knows this shape at open and does not guard it on the live path.

**contract_promised.** - SyncPolicy::Always (the default, db.rs:3835): "One fdatasync per commit. Durable per batch" (db.rs:177-178).
- Db::commit: "The durability point: WAL append + fdatasync" (db.rs:28470).
- Db::sync: "The promise is this call's, whatever the policy left unsynced" (~29619, per the mapper; I did not re-read it).
- CLAUDE.md, the crash-discipline paragraph: a commit's batch "is durable or its tail frame fails its CRC and replay stops before it". The same paragraph says a sequence gap at a chained boundary means a lost durable record. Here the open reports that when the engine itself wrote the gap, after the device lost nothing it had acknowledged.

The tests assert what these promise. Commits that returned Ok after the failed one must survive. The failed batch itself may be present once or absent. The store must open.

**fails_for_stated_reason.** Yes, for k2 and for the refusal. In run.log, three tests fail at the k2 assertion (`left: [] right: [v2]`) and the rotation test fails at Db::open with "wal sequence gap: a durable record is missing". That matches the mechanism: replay at db.rs:4641 compares C2's stored CRC (over B1+C1+B2) with crc32(B2) and breaks; the open then cuts the file at valid_len.

One reported observation is not supported. notes.md says k1 read back absent in the zero-byte case. The test never prints k1: failed_batch_is_whole_or_absent passes either way. By my reading k1 should be recovered, because C1's CRC was taken when pending held only B1. So "k1 absent" is unverified and probably wrong. It does not affect the verdict.

The cuts after 5 and 20 bytes were run only by hand edits, and their output is not in run.log. Only the cut after 1 byte is logged; the loop stops at its first failure.

**fix.** Smallest fix I would accept: latch the failure. When Wal::write returns an error, set a sticky error so that no later commit, sync or flush returns Ok until the store is reopened. It costs nothing on the success path. On reopen the open's existing cut to valid_len removes any stray prefix. The price is that a transient ENOSPC stops the writer until a reopen.

Better fix:
(1) Take the batch CRC over the current batch only. Record batch_from in mark_commit, reset it when write clears pending, and compute crc32(&pending[batch_from..]). The failed batch is then retried as a valid batch of its own. It must be retried, not dropped: it is already in the hashed memtable and becomes visible at the next commit's watermark.
(2) On a failed write, call set_len(written) and seek to written. The fd is append-mode after open_append, so the truncate is what counts.
(3) If the truncate fails, fall back to the latch.

The fix's tests must fail under each of these mutants:
(a) CRC over all of pending, not from batch_from: the zero-byte test fails.
(b) No truncate or seek back to `written`: the partial tests at 1, 5 and 20 bytes fail. The 20-byte cut leaves a whole record frame that the retry's commit frame would adopt.
(c) batch_from not reset in write: later commits are lost.
(d) No latch when the truncate fails: this needs a second seam that fails the set_len.
(e) The failed batch dropped instead of retried: assert k1 is present exactly once after the next commit and after reopen. Today's tests allow absent, so they would not catch this mutant.
(f) The rotation case: the open succeeds with every acknowledged key.
Repeat the tests under a cipher, where the same early return exists in the sealed arm.

Neither fix adds a sync or a byte to a commit or a drain, so neither needs pricing in the benchmark suite.

**fix_cost.** On the success path: one usize stored per commit (batch_from), or one flag tested per commit for the latch. No syscall, fsync or byte is added. The failure path alone pays one ftruncate and one lseek. The test seam costs one load and branch per Wal::write, beside a write(2).

**problems.**
- notes.md's claim that k1 read back absent in the zero-byte case is not observed: no assertion or print shows k1. By reading, replay accepts C1 and recovers k1. Either the claim is wrong or something unexplained is happening; it should be printed before it is written down.
- The partial-write test loops over cuts of 1, 5 and 20 bytes and stops at the first failure. Only the 1-byte cut is in run.log; the 5- and 20-byte cuts were run by editing the array and were not logged.
- The rotation test uses seal_bytes = 64 KiB and seal_max_pct = 0 to hold open the window between rotation and the seal's retirement of the closed file. Under defaults that window exists only if the log rotates before a seal's durable landing, so the refusal is reachable under defaults but its window was widened for the test.
- The power-cut claim for the rotation case was not checked against a directory fsync after the rotation; only the process-kill case is shown.
- The cipher arm of Wal::write has the same early return, and the retry reseals all of pending as one record. That arm is not exercised; by reading it fails the same way, with the batch CRC breaking inside the decrypted record.
- The seam's Option::take does a store on every Wal::write even when unused. `if self.fail_next.is_some()` would avoid it. The cost is negligible either way.

### wal-sync-fail (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes for a power cut; not as a plain process kill, which the reproducer also says. I checked the syscall order at 3df85d22. Test 1, SyncPolicy::Always, for commit A: Wal::write does write(2), then sync_wal does wal_dir_barrier (nothing owed), chain (no-op) and fdatasync, which succeeds, so synced = end of A. For commit B: write(2), then mem().commit() makes B visible, then sync_wal fails between chain() and sync(). The seam returns Err at the same point a failed fdatasync would, and synced does not move. For commit C: write(2) at the file's end, then fdatasync on the same open file description, which the seam lets succeed, so commit returns Ok.
On Linux, a writeback error is reported once per open file description (errseq). The failed pages are left clean, so C's successful fdatasync does not write B's range again. After a power cut that follows C's sync, the device holds A, then whatever was in B's blocks before (a hole or unwritten extent reads as zeros; recycle_wal is off by default, so the file is fresh), then C. That is the state the seam builds by zeroing [max(synced, head), written) and copying the files.
Nothing the test drops was covered by a successful sync: B's bytes were covered only by the failed one. The WAL name, the segments and the manifest were made durable by create and flush before the window, so the copy keeps no name a journal commit could not have kept.
Under a process kill alone, a real kernel usually keeps B's pages in the page cache, so the loss needs a power cut, or the pages evicted and read back from the device before a crash. Test 2 is the same with EveryN(u32::MAX): 20 write(2) calls, a failed sync() (synced does not move), then a second sync() whose fdatasync succeeds.

**contract_promised.** Db::commit's doc, src/db.rs:28470: "The durability point: WAL append + fdatasync". Under the default SyncPolicy::Always, an Ok commit is a durable commit.
Db::sync, about line 29611: "Make everything written durable ... The promise is this call's, whatever the policy left unsynced".
CLAUDE.md, under "Crash discipline": "Commit is a WAL append and one fdatasync". Its presync rule: "an fsync's error is consumed by the call that sees it, so the result of every sync on a path is checked by that path and never left to a later sync of the same file".
Test 1 asserts the Always promise and test 2 the sync() promise, which holds under any policy. Neither test asserts anything about B, which returned Err. Both assertions are conditional on Ok, so a latch that answers Err passes them.

**fails_for_stated_reason.** Yes. run.log shows --list naming both tests, and both fail at the stated assertions.
Test 1 fails at line 107, "a commit answered Ok after a failed sync of the log is durable", left [] right [[67]]. The guard that B returned Err passed, and A and the 100 flushed keys read back.
Test 2 fails at line 160, left [] right [[0,0,0,0]], after a guarded Err from the first sync().
I read the engine path that produces this. Wal::sync (4458-4462) advances synced only on success, and nothing records the failure: pending_err is take()n and is not set here. So frames_unsynced() stays true and the next sync_wal re-issues fdatasync. commit (28495-28510) has no check that would stop C.
The reproducer's controls point the same way. With the seam's zeroing removed, both tests go green. With a throwaway latch, both go green. So the red comes from lost frames followed by a later Ok, and not from the copy helper or the reopen: the damaged log reopens, and replay stops at the zeros.
I did not run anything; this rests on the log and on reading.

**fix.** Smallest acceptable fix: a sticky writer latch.
- Set it whenever sync_wal sees an error from wal.sync(), from a loose file's sync_data, or from wal_dir_barrier. Optionally also from Wal::write and the direct run's sync.
- While it is set, commit, commit_staged, sync and close's commit return Err. Fold the check into the pending_err test commit already makes, so it is not take()n.
- Clear it only on reopen, or at a seal's durable landing whose manifest covers every sequence up to the log's written end. That works because the segment is written from the memtable and not from the log. A flush answering Ok after the failure is then truthful.
- A retry of fdatasync must never clear it.

Mutants the fix's tests must fail under:
- (a) the latch not set on the live file's sync error;
- (b) set, but commit does not check it (test 1);
- (c) set, but Db::sync does not check it (test 2);
- (d) cleared by the next successful fdatasync;
- (e) take()n on first report, like pending_err, so the commit after the Err commit is Ok again;
- (f) cleared by a flush whose manifest does not cover the failed frames, if clearing at a landing is implemented;
- (g) not set on the loose-file sync or the directory-barrier error. This needs sibling seams and tests.

Also add a test that the store reopens and replays to A, and does not refuse the open.

**fix_cost.** The latch adds no syscall, fsync or byte. Setting it is on the error path only. On the commit path it is one load and branch, which can share the existing pending_err check (for example, a combined flag read only when set). It needs the callgrind count CLAUDE.md requires, commit path by function against origin/main (bench/examples/load_only.rs and work_probe.rs), to show it costs nothing per operation. It adds nothing to a drain. The rewrite-the-frames alternative copies every unsynced frame into a retained buffer. Under buffered policies that is up to a log file of memory and a memcpy per commit, which would need pricing in the suite; I would not accept it as the smallest fix.

**problems.**
- The seam zeroes B's frames in the page cache, so the copy is taken as a kill copy but stands for a power-cut state. On a real kernel a kill alone usually keeps B's pages in the cache. The test's doc comment says the copy is 'what a process kill leaves', and that should say 'a power cut after the second, successful sync'.
- Errseq is modelled, not observed: no test or probe here shows a real second fdatasync succeeding after a real EIO. The seam's behaviour matches documented Linux semantics, but it is the seam's.
- The seam is not behind a cfg; it follows fail_presync's pattern (#[doc(hidden)] pub, AtomicBool in Shared). When unused it costs one atomic swap per sync_wal call, beside an fdatasync. It is not on the commit's hot path when no sync is due, and it does not change behaviour.
- lose_unsynced writes through vault::open_write. That is sound for a plain store only. Under a cipher, zeroing sealed records would model a different loss. The tests do not use a cipher.
- Only the live file's sync is covered. The loose files' sync_data in sync_wal (a fresh open, where errseq is consumed by whichever description saw it first), the directory barrier and the direct run's fdatasync are sibling sites with no test.
- The window closes at the next seal's durable landing. The segment is written from the memtable, which holds B and C, and the manifest covers the sequence. So exposure lasts from the failed sync to the next flush or seal landing, and needs a power cut inside it.
- B is visible to readers before its sync (mem().commit() at 28497) and stays visible after its Err. That is a related visible-but-not-durable question, and neither test asserts it.

### wal-fresh-header (stands: True; severity: refused_open_default)

**state_reachable.** Yes under the workflow's crash model, but only through a filesystem path that ext4's and xfs's defaults make rare. Syscall order I checked by reading. Wal::create (db.rs:4180-4183) does create plus write_all of the 8-byte header and syncs nothing. Db::create (27311) and next_wal's non-recycle branch (27175) do the same. The only fdatasync of a live log is Wal::sync (4458-4462). It is a whole-file sync_data, so a header can never be lost while synced frames of the same file survive. The mapper's correction holds.

Default config: a durable ordered load's commits fdatasync only the direct run's file, and frames_unsynced (4489-4491) is false for a log that holds only its header, so wal-00000000 is never synced. Under EveryN with drain_syncs_wal off, a flush acknowledges by segment fsync and manifest slot, so the log's header and frames are never synced either. The reproducer put a temporary trace in Wal::sync and Wal::prefill and confirmed neither fired before the copy. So the zeroed bytes are bytes no sync covered, and the test drops nothing an earlier sync had covered.

Which states a real power cut can leave:
(a) wal-0 at length 8, all zero. The inode size must be durable while the data block is not. On ext4 data=ordered with delalloc, i_disksize moves only at writeback, and ordered mode writes the data before the commit that allocates it. On xfs, di_size is updated at I/O completion. Neither default leaves this state. ext4 data=writeback can, though it exposes stale blocks rather than zeros, which the open refuses all the same. So (a) is admissible under the stated model ("in part") and rare on real defaults.
(b) Page 0 zero with later pages present (the flush test). This needs page 0's unwritten extent left unconverted while a later range's conversion and size committed. That is possible with ext4 dioread_nolock when page 0's first writeback and a later range's writeback fall in separate io_ends and the cut lands between them. The window is milliseconds and needs writeback ordering, so it is rare.
(c) A size of 0, or a file shorter than 8 bytes: the open already reads these as an empty log. On ext4/xfs this is the likely outcome.

The directory entries are not at issue: the log's name exists in the copy, and the fsync of another file commits the journal.

**contract_promised.** CLAUDE.md, "Crash discipline is an order, and every window in it is survivable": a crash leaves a state the open reads. The same paragraph makes a flush an acknowledgement once its seal's fsyncs and slot are done. Under SyncPolicy::Always a commit that returned Ok is acknowledged. The engine's own doc on Wal::frames_unsynced (db.rs:4487-4488) states that "the header needs no sync of its own: an open takes a file short of it, or torn inside it, as a log with nothing in it". That covers only a prefix tear. replay_frames (4590-4607) refuses 8 bytes that are not a header. So the engine promises survival and rests that promise on a filesystem assumption (no full-length zero header) that the crash model does not grant. Nothing acknowledged is read wrong. The data becomes unreachable because the open refuses, at every open, since the WAL is re-read each time.

The buffered test asserts only what a flush promises: the 400 keys were flushed with Ok before the cut, not merely committed. The durable tests assert acknowledged commits. The first test asserts that an empty store opens.

**fails_for_stated_reason.** Yes. run.log shows each test panicking at its open with "not a supdb WAL: the header is missing or from an older format", the replay_frames refusal at db.rs:4600-4607, propagated by Db::open.

In the direct-run test, both opens refuse. After the first open the directory holds seg-00000000, its .oidx and the manifest, so the run was recovered and published before replay refused.

In the flush test the page case fails first, and the whole-file case is never reached because the panic ends the loop. It would fail the same way, since a zero header is refused whatever its length. The preconditions are asserted: a direct-*.tmp exists, the log is 8 bytes, and the buffered log is longer than 8 KiB. No seam is involved. Copying after drop is fine, because drop issues no sync of the log (the trace confirmed it).

**fix.** Smallest acceptable fix: in Wal::replay_frames (db.rs:4596-4608), read a header whose 8 bytes are all zero the way a torn prefix is already read, as a log with nothing in it (return (from, 0, false)), so the open truncates the file and rewrites the header. This is sound because no engine write puts zeros there, and any acknowledged frame in a file was fdatasynced together with its header (sync_data covers the whole file). Frames after a zero header were therefore never acknowledged in that file, or are covered by the manifest. Garbage that is not zeros should stay refused: it is damage, or data=writeback's stale blocks, and accepting it would mask foreign files.

The fix's tests must fail under these mutants:
(1) Accept any non-header 8 bytes as empty. A test that a foreign header (e.g. "SUPDBWX" plus a kind, or random bytes) is still refused must fail this.
(2) Accept the zero header but skip the truncate/rewrite. A reopen after a post-recovery commit must read the commit back; the first test does this.
(3) Accept zeros only when the file length is exactly 8. The page-zeroed case of the flush test must fail this.
(4) Accept zeros only for the first or only file. A rotation test with wal-N+1 zeroed under Always must still read every commit from wal-N.
(5) Treat the zero header as empty but keep replaying the bytes behind it. The page case with surviving frames must not apply unacknowledged frames past the manifest's sequence, or refuse on them.

The stricter alternative is to write each new log's header to a temp name, fdatasync it and rename it into place. That costs a sync and a rename at create, at each size rotation (on the rotating commit) and at a header rewrite, and would have to be priced in the suite. I would not take it over the replay rule.

**fix_cost.** Replay rule: one 8-byte compare at open, nothing on the write path, no commit or drain cost. The stricter header-sync alternative adds one fdatasync plus a rename per new log file, about 0.3 ms by CLAUDE.md's device figures. It lands on the commit that triggers a rotation, so it would need a `bench ab` pairing on the load passes before shipping. It costs nothing extra under recycle_wal.

**problems.**
- Likelihood outside a test is low on the default filesystems. ext4 data=ordered with delalloc, and xfs, publish a size only after the data is written, so the full-length zero header (state (a)) is essentially unreachable there. data=writeback and other filesystems reach it, as garbage rather than zeros, which is refused the same way. The page-0-lost case (b) needs dioread_nolock unwritten-extent conversion to complete out of order across io_ends: a window of milliseconds. The tests are valid under the workflow's stated model (unsynced data may survive in part); they are not a demonstration of an ext4/xfs-default trace.
- The whole-file-zero case of the flush test was never executed, because the page case panics first. Its outcome is inferred from the code, not observed.
- recycle_wal=on was cleared by reading and a trace of prefill+sync_all; no crash state was built for it. That reading looks right to me: the header is synced before any use, and a recycled header is rewritten in place inside one sector over a previously synced header.
- A sibling window is untested: rotation under Always without a direct run. The new wal-N+1 header is unsynced between the rotating commit and the next commit's fdatasync, and a zero header there refuses an open whose acknowledged data sits in wal-N. Same mechanism, same fix.
- The direct-run test's emphasis that the open 'publishes the run before refusing, so the refusal is permanent' is not what makes it permanent. Every open re-reads the zeroed log and refuses whether or not the run was published.

### run-write-fail (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes, for all three sub-cases. I re-read the engine at 3df85d22; the tree is clean.

**The syscall order I checked under `Always`:**
1. `commit_direct` (28690) does no I/O until `mark_stream`.
2. `mark_stream` (6870) calls `stream_flush(false)`, which writes whole pieces only. For a small batch its loop does not run, so the armed fault stays armed.
3. `d.committed = mem.len(); mem.commit()` (28797-28798).
4. `SegmentWriter::sync` (6917): `stream_flush(true)` writes the held tail with `writev_all` at the file cursor (5742), and `written`/`held_from` move only on success. Then `sync_data`, then `synced_all`.
5. The next `commit()` returns Ok at 28691 (`committed == len`), with no I/O.
6. `Db::sync` reaches the same early Ok through `commit_staged` (28646). It issues no `sync_wal` for an ordered table (29622). `adaptive_shape` is off by default (3933), so `hand_tail` does not run either.

**(a), the write.** The seam fails a `writev` having written nothing, which is what EIO or ENOSPC from `write(2)` gives. The test then models a kill with `mem::forget`, so the page cache survives. No byte of k2 was ever on the file, so nothing a sync covered is dropped. The state is valid.

**(a), the fdatasync.** k2's bytes are written to the page cache and the fdatasync fails. The test cuts the temp file to `out.synced` (4328, k1's end). No later fdatasync of that file succeeded, so losing everything past 4328 is one admissible power-cut outcome. No name is kept or dropped. The state is valid.

**(c).** A short `writev` followed by an error is the classic buffered ENOSPC/EDQUOT shape: some pages are copied, the next call fails, and space comes back later. `writev_all` loops at the cursor, so the cursor ends at written+k. The next commit writes the held range again at the shifted offset and fdatasyncs it. A kill alone then loses k3. No power-cut emulation is needed, and none is used.

**(b).** The state is only the in-memory wedge. On disk the last marker is intact, and reopen recovers a0, which the test checks.

**contract_promised.** **`Db::sync` doc (29611):** "Make everything written durable". k2 was committed to the memtable (`mem.commit()`) and is visible to readers, so it is "written". A `sync()` that returns Ok promises it.

**`commit` doc (28469-28474):** under ordered ingest, the durability point is "the batch streamed to the direct segment and that synced".

**CLAUDE.md, "Crash discipline is an order":** a commit is "one fdatasync", and in ordered ingest "then one fdatasync on that file". k3's commit in (c) returned Ok under `Always`, so it is promised durable against a kill. A buffered policy promises less per commit, but `sync()` and `flush()` promise the same under any policy, and the tests assert only what Ok answers promise.

**(b) breaks no durability promise.** It breaks the rule in CLAUDE.md's cipher section that "a refusal is tested by what it says". Every later commit and sync answers "keys must arrive in strictly increasing order", never the device's error, and no retry is possible. That is a refusal for the wrong reason, not a loss.

**The `commit()` with nothing new, in (a).** Its Ok is weaker evidence than `sync()`'s, because no batch is attached to it. The test ORs the two, and `sync()` alone carries the assertion.

**fails_for_stated_reason.** Yes. `run.log` shows `--list` naming the four tests, and then the five after (c) was split. Each run fails with a "test result: FAILED" line, so none of these exits is a watchdog's.

- **(a) write:** commit() and sync() return Ok(()); after reopen k2 reads []. That fits a sync-arm failure: a failure in the piece arm would have made the retry fail on key order, not return Ok.
- **(a) fdatasync:** commit() and sync() return Ok; the run is synced to 4328; k2 reads [].
- **(b):** the next commit is Err(InvalidData "segment writer: keys must arrive in strictly increasing order"), and the assertion that the error names the injected failure fires.
- **(c):** k3's commit and sync() are Ok; k3 reads [] after the kill.
- **(c), flushed:** flush is Err("segment par-...: the block table is not readable"). It leaves `par-` and `ord-` files that no manifest names; open sweeps those. k3 reads [].

The seam's faults fire where the mapper predicted. The engine paths are unmodified apart from the `writev_armed` and `sync_data` wrappers.

**fix.** **The smallest fix I would accept**
1. **Write at the counted offset.** `stream_flush` and the held branch of `write_buf_with` write with `pwritev` at `self.written` (`FileExt::write_vectored_at` in a loop) instead of `writev` at the cursor. A short or failed write then never shifts the file, and a retry overwrites the stray bytes. This fixes (c).
2. **An owed flag on the Direct.** Set it before the sync arm and clear it only after `synced_all`. The early return at 28691 is taken only when the flag is clear. `Db::sync` with the flag set writes the held tail and syncs again. This fixes the write half of (a), and also the EveryN sync gap noted in problems, if the flag is also set by a commit that is not due.
3. **A sticky run error for a failed `sync_data`.** It holds the original io::Error. Every later commit, sync, flush and close of the run returns it, and the recovery is a reopen. This fixes the fdatasync half of (a); do not retry fdatasync, because of errseq.
4. **For (b), a failed piece write in `mark_stream` is no longer the commit's answer.** Leave the piece held, with `written` and `held_from` unmoved, and let the commit go on to the sync arm, which writes it again positionally and fails there with the device's error. The alternative is to latch the device's error as in 3. Do not try to roll back `put_encoded`'s counters.

**Changes to the tests**
- The fdatasync test must require Err from sync() after the injected sync failure, or the seam must model errseq: a fault that also makes the next `sync_data` 'succeed' while `synced` must not advance.

**Mutants the fix's tests must fail under**
- `pwritev` replaced by cursor `writev`. Test (c) fails.
- The owed flag cleared before `synced_all`, or never consulted at 28691. Test (a) write fails.
- The latch not set, or `take()`n rather than sticky. A second sync after the failed fdatasync returns Ok, and the strengthened fdatasync test must fail.
- A fix that retries `sync_data` instead of latching. It must fail the strengthened fdatasync test.
- `mark_stream` still returns the piece error early. Test (b) fails with the key-order message.
- An EveryN test: commit, then sync(), then a power cut truncated to `synced`. It must fail if the owed flag ignores commits that were not due.

**Cost**
- On the success path: none. `pwritev` replaces `writev` one for one, the owed flag is a store and a test per commit, and the latch is one branch at the commit's start. No new fsync is added on a commit or a drain, so there is nothing new to price in the suite.
- The EveryN case: `sync()` would now issue the run's fdatasync where it currently issues none. That cost is the contract's own price, and it should be noted in the PR.
- On the failure path: the held tail is written again, or the store is refused until it is reopened.

**fix_cost.** On the success path it adds nothing. `pwritev` replaces `writev` one for one, the owed flag is one store and one test per commit, and a latched run adds one branch at the commit's start. There is no new syscall, fsync or byte. The one exception is the EveryN sync gap found beside this defect: a `sync()` after commits that were not due would now issue the run's fdatasync it promises and currently skips. That is the contract's own price, not a regression, and the benchmark rows for ordered EveryN loads that call `sync` would show it. The test seam adds one relaxed load per run commit and one byte test per run write and per run sync.

**problems.**
- The fdatasync test cannot reject an unsound fix. RUN_FAULT_SYNC returns an error before calling sync_data, so the device never saw a failed flush, and a later real fdatasync really succeeds. On Linux, after a real EIO from writeback, the next fdatasync on the same description returns Ok (errseq) while the pages may already be dropped or marked clean. A fix that only keeps an `owed` flag and re-issues fdatasync would make `synced` advance, so the test truncates to the new extent and passes. The real kernel would still lose k2. For the fdatasync case the test should require that sync(), and every later run operation, return Err (a latch), not accept 'Ok and reads back'. As written, its pass condition admits the fix the mapper itself calls unsafe.
- The 'Ok and reads back' branch of the write test (a) is sound: a failed write left nothing in the page cache, so rewriting the held tail from the arena is a valid recovery. Keep it.
- Cosmetic seam defect: the patch puts the RUN_FAULT_* consts between AlignedWriter's existing `///` doc block and the struct. That doc now attaches to RUN_FAULT_WRITE, and AlignedWriter loses its documentation. Move the consts above the doc comment.
- In (a), `promised = again.is_ok() || synced.is_ok()`: commit() with an empty batch returning Ok is a weak promise, but sync() alone carries the case, so the failure stands.
- (b) is not data loss. Nothing acknowledged is lost and reopen recovers. It is a wedged run refused by an unrelated error, which belongs in the wrong_answer or hardening class, not with (a) and (c).
- The (b) follow-on through leave_direct, and close_run after a failed commit, stay unverified. Neither test reaches them.
- Out of scope, but found by reading the same code (not run): under SyncPolicy::EveryN(n>1) with an ordered run, a commit that is not due leaves the batch unsynced. A later Db::sync() reaches commit_direct, which returns Ok at 28691 because committed == len. It issues no fdatasync of the run, and the `!ordered` sync_wal branch is skipped, so sync() answers Ok with the run unsynced and no device error at all. This should be routed as its own defect: an arm, but no fault needed. It shares a fix with (a): an owed flag checked at the early return.

### helper-presync-error (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes, within the device-error model. I read the engine at 3df85d22 and did not run anything; the tree is clean. The kernel behaviour the tests rely on is standard Linux errseq: each struct file samples f_wb_err when it is opened. try_clone (WriterHelper::spawn 6156) and w.file_clone() (close_run) both dup the fd, so the handles share one struct file. The first fdatasync that sees a writeback EIO advances f_wb_err, and later fdatasyncs on that description return 0. ext4 and xfs mark the failed pages clean or drop them, so a later sync writes nothing and the bytes are gone after a power cut.

Syscall order I checked:

(H1, T1/T2) The helper's dup.sync_data fails and stores the error in HelperShared::error (6219-6226), then advances synced anyway (6227). The writer's own handle then calls sync_data at 6934, which returns Ok, and SegmentWriter::sync returns Ok. The only reader of the stored error is finish (join_helper). The seam skips the sync and stores EIO, which is what a consumed error leaves behind, so the test is a faithful model.

(P1, T3) In the non-inline arm the order is hand_seal(job) first, then the writer's presync f.sync_data, whose error goes only to presync_failed. On the seal thread: finish, write_ord_unsynced, hard_link, seal_readable, vault::persist, then seg.sync_all() on the same description, which returns Ok. land_durable then names the segment in a manifest slot. The next flush answers Ok. Under EveryN(1_000_000) no commit synced the run. sync_ahead is 8 MiB and upto is 409600, so the helper never synced either. The presync was therefore the only sync over [8192, upto). The finish rewrites only the prefix [0, SUPER + HEADER rounded up) = [0, 8192) (layout(): SUPER is 4096, flatindex::HEADER is 192, and a direct run has no head reserve), plus offsets at or past upto. So zeroing [8192, upto) drops no byte that a successful sync covered. The real-world case needs the presync to see the EIO before the seal's sync_all does. That is the usual order, because the job is still composing the trailer while the writer presyncs.

(T4, inline arm) after() links the file, calls seal_readable (which is where SEAL_HOLD_DURABLE is waited), and only then joins the presync with ??. land_front's Err branch with landed=true leaves the segments live with their sequence covered, so the next manifest names them (37660-37672). The hold only forces an order the reproducer saw 9 times in 30 unheld runs. Seam D only lifts the hold from another thread.

**contract_promised.** SyncPolicy::Always is "one fdatasync per commit. Durable per batch" (src/db.rs:176-177). EveryN makes "one fdatasync per n commits, and always at seal, flush and close", with loss bounded at n batches (166-180). CLAUDE.md, under "Crash discipline is an order", says the manifest names only durable segments and a flush's drain is answered once its seal's fsyncs are done. Under "A sync whose error the next sync could not see" it says the flush answers with the presync's error, and that a sync's error is checked by the path that saw it, never left to a later sync of the same file. The WriterHelper doc (6110-6112) says the helper keeps its error because the writer's own fsync "would then report nothing". The commit-time sync at 6934 is the case it does not cover. The workflow's definition adds that a flush returning Ok made everything before it durable.

T1, T3 and T4 assert exactly that. T2 asserts more than the EveryN doc strictly promises at that instant: losing batches 1 and 2 is within a bound of n=2. The bound breaks only once later batches are acked behind the zeroed records, because the recovery walk stops at the first bad marker. The natural place to refuse is still the due sync, so I accept T2 as a proxy.

**fails_for_stated_reason.** Yes, by the log, with one unexplained difference. --list shows all four tests.

- T1 fails at the w.sync().is_err() assert, after waiting on the seam counter, which is bumped only after the error was stored.
- T2 fails at the due commit's assert.
- T3 prints "presync failed over 409600 bytes ... next flush acked: true", hit=1 (the par- link to the run's inode). Db::open of the cut copy then fails with "key index checksum mismatch in piece 0" on par-00000000-...sup. That is the expected result of zeroed records inside the key section that the manifest names. It failed in 20 of 20 runs.
- T4 prints acked: true and hit=2 (the tmp name and the par link both survive, because land_front's Err branch retires nothing). It then fails because key 0 reads [], not because the open refuses.

Why T4 opens, while T3 refuses at the checksum, is not explained in the notes. One possibility: the second flush's manifest named the segment differently, or the open took the tmp recovery path. Either way, an acked flush followed by lost keys after the cut is the stated contract violation. The notes say a diagnostic on an intact copy read every key, so the loss comes from the zeroed pages and not from the test.

The existing tests near the seams still pass with the seams unused.

**fix.** Smallest fix I would accept, in three parts:

(a) SegmentWriter::sync, after its own sync_data at 6934, loads HelperShared::error and returns Err if it is set. The error is sticky for that writer, and commit_direct turns it into a failed run: the store's pending error, with the run's table never named durable.

(b) In both close_run arms, the presync's result reaches the seal job before seal_readable, not just before sync_all. In the non-inline arm the job waits on a one-shot slot the writer fills after its presync. In the inline arm the presync join moves above the hard_link and seal_readable. A failed presync then fails the job with nothing published.

(c) land_front's landed=true Err branch must not leave the segments for the next manifest to name. Either poison the store, so every later flush, sync and commit is Err (fsyncgate semantics, the simplest), or reseal the table into a new file under a new inode. It must never re-sync the same inode and trust it.

Mutants the fix's tests must fail under:
- delete the error load in sync (T1, T2);
- the job not waiting on the presync slot in the non-inline arm (T3);
- the inline arm joining the presync after seal_readable (T4);
- land_front's stay-live branch restored (T4, and a new default-arm test that fails seg.sync_all after the readable landing);
- the poison or sticky error cleared after the first Err (T3, T4's second flush returning Ok);
- the helper storing the error but the sync reading a different field.

Also add a Db-level test of H1 under Always: one commit of at least sync_ahead, with the helper's turn held before the commit's sync.

**fix_cost.** (a) One atomic load per synced direct-run commit, with no syscall and no bytes. (b) On the success path, a wait on the seal thread for a slot the writer's presync usually fills while the trailer is being composed. Where it does not, the wait replaces part of sync_all's own device wait. No new fsync or syscall is added. The readable landing may move later by the presync's remaining latency in the non-inline arm, so the drain stamps (SUPDB_DRAIN_STEPS=1) should be read before and after, on the ordered load's drain at 10k and 300k keys, buffered and durable arms. (c) A flag check on the error path only. No sync is added to a commit or a drain, so no new fsync needs pricing, but (b)'s reordering of the readable landing against the presync does need a pairing of the ordered load (pair.sh supdb supdb 300000 --passes load, base against fix) before it ships.

**problems.**
- T1 and T2 test the error contract, not lost bytes. The injection skips the helper's sync, and the writer's later sync_data really syncs. The model is right for a consumed EIO, but no test shows the H1 data loss.
- H1 under the default policy (Always, one commit of 8 MiB or more racing the helper) is not tested at the Db level, only at the writer level (T1). T2 uses EveryN(2).
- T2 asserts Err at the due commit, which is stricter than what the EveryN doc literally bounds at that instant (n batches). The bound is only violated once later commits or a flush are acked.
- P1 (T3) loses data only under EveryN. Under the default Always the run's commits already synced the records. The existing test a_failed_presync_is_the_flushs_answer runs under Always and asserts that the seal lands, so a poisoning fix must reconcile with it.
- T4 runs the close_finish_inline arm (off by default) and needs a new seam D plus a hold. Its failure (key 0 absent) differs from T3's (open refused) and the reason is not established.
- T4 really exposes a broader defect, not specific to the presync: land_front's Err branch for a seal that failed after its readable landing leaves the segments live, and the next manifest names them. Any phase-D failure (seg.sync_all, ordf.sync_all, vault::persist) in the default arm, for ordinary seals too, is named durable by the next flush. This should be filed as its own item.
- The mapper's H2/H3 claim (a helper failure at a seal's or merge's finish is caught by join_helper before any rename) was not tested; it is correct by reading only.
- The power-cut emulation zeroes the pages. A real failed write on a fresh ext4 extent reads as zeros (unwritten extent), so that is fine, but other filesystems may leave stale data. Not material.

### sync-ordered-buffered (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes, for all four tests. I read the code at 3df85d22 and compiled and ran nothing. The order I checked:

- commit_direct (src/db.rs:28743-28807). Under direct_stream, mark_stream calls stream_flush(false), which writes only whole 1 MiB pieces (5707-5713). The rest of the batch and its marker stay in the table's arena. d.w.sync() (6917-6936: batch_buf, then stream_flush(true), flush, sync_data, synced_all) runs only when the policy is due (28800-28807).
- The writer's helper fdatasyncs only once `ahead` bytes are unsynced (6217). It raises shared.synced with fetch_max only after sync_data returns, and it records the error.
- Db::sync (29616-29626) calls commit_staged, which returns early when nothing is staged (28691). Then `if !self.mem().ordered && ...` skips sync_wal, and `unsynced = 0` forgets the debt.

Test by test:

- **A (process kill).** The copy taken after sync lacks the bytes still in process memory. A kill loses exactly those, so the state is real. Under EveryN(5) the last due sync was at commit 10, so commits 11 and 12 were never written (2000 keys). Under the buffered policy everything past the first whole piece is gone (4000 keys).
- **B (power cut).** The run file is cut to synced_to. That is max(the writer's synced, the helper's synced), and the helper's figure is ignored if its error is set. Both figures are raised only after a completed sync_data (5935-5941, 6218-6227), so the cut never drops bytes an fdatasync covered. Buffered: 1 MiB written, nothing synced, below the 8 MiB the helper syncs behind, so a cut to 0 is a legal power-cut state. The test keeps the run's directory entry even though nothing fsynced the directory. That keeps more than a power cut must, so it does not weaken the result.
- **C (power cut, log beside a run).** seal() under EveryN does not sync the log (29111-29124), and wal_durable reported synced=0, written=56629. hold_seal_durable stops the frozen table's seal before its fsyncs and manifest, so open sweeps its segment. The WAL is cut to max(synced, 8). Keeping the unsynced 8-byte header is again more than a power cut must keep.
- **D (process kill, adaptive_shape arm).** close_run's non-draining job waits at SEAL_HOLD_LANDING before finish writes the held tail (29047-29051). A kill in that window loses what was only in memory. In the real engine that window is the seal thread's wake-up plus the start of finish: milliseconds, widened here by the hold.

**contract_promised.** - Db::sync's doc, src/db.rs:29611-29613: "Make everything written durable ... the WAL's pending frames written and fsynced".
- Its own comment at 29619-29620: "The promise is this call's, whatever the policy left unsynced".
- SyncPolicy::EveryN's doc, 179-180: "always at seal, flush and close".

A buffered policy promises less per commit. It does not promise less for an explicit sync() that returned Ok. Every key the tests require was committed before that Ok sync, so the assertions are within what the engine promises.

**fails_for_stated_reason.** Yes, as far as the log and the code show:

- **A:** EveryN(5) loses 2000 keys from key-010000, which is exactly commits 11-12, past the due sync at commit 10. Buffered loses 4000 keys from key-008000, which is past the first whole 1 MiB piece (about 8000 records of ~130 bytes).
- **B:** EveryN(5) reports synced = file length = 1204528, so the loss is the never-written tail. Buffered: 1048576 written, 0 synced, all 12000 lost.
- **C:** the reproducer's trace showed ordered=true, frames_unsynced=true, wal synced=0. That is the 29621 gate.
- **D:** 4000 missing from key-008000, the same whole-piece boundary, with the hold placed before finish.

The control tests in tests/db.rs pass with the seam in place, so the seam broke nothing. I do not see a test mistake that produces these numbers: the policy loop reports both policies, and C and D assert their preconditions (a run open; the run handed).

**fix.** Smallest fix I would accept: in Db::sync, after commit_staged and before hand_tail:
- If a run is open (self.direct is Some), call d.w.sync() under self.r.unlend(), the same call a due commit makes. Do it unconditionally, or gated on the run having anything held or unsynced: stream_held, buf or batch_buf non-empty, or file_written > synced_to. A fix that gates on that condition must get it right.
- Before a run's first sync, fsync the store directory once for the run's temp name.
- Drop `!self.mem().ordered &&` from the sync_wal gate, so frames of a table frozen before the run (and, under a cipher, the run's logged frames) are synced.
- Keep unsynced = 0 after both.

Because the run is synced before hand_tail, D is covered without touching close_run.

Mutants the tests must fail under:
1. The run's tail written (stream_flush(true) or flush) but not fdatasynced: B fails, A passes. This is why B must stay.
2. The ordered gate on sync_wal kept: C fails.
3. The run synced only when !adaptive_shape, or after hand_tail: D fails.
4. A gate on file_written > synced_to that ignores the held stream or batch_buf: A fails.
5. A gate on the self.unsynced counter: it reads 0 right after a due commit, so the next sync with a held tail skips. Add a case of sync() directly after a due commit followed by one more commit.
6. The directory fsync left out: no current test catches it. Add a check of the run name's directory sync (a counter seam), or say plainly that it is held by reading only.

Run beside them: every_n_loses_the_unsynced_tail_whole_and_never_in_part, a_direct_segment_recovers_to_its_last_commit_marker, a_direct_run_streamed_from_its_table_is_the_run_the_copies_write, and the fuzz with EveryN.

**fix_cost.** - **Commit path:** nothing added; the change is confined to sync().
- **Always:** one comparison per sync, since every commit already synced the run.
- **EveryN(n>1), per sync() with a run open:** one write of the held tail (under 1 MiB) and one fdatasync of the run's dirty bytes, at most about 8 MiB since the helper syncs behind past that. By CLAUDE.md's device figure (0.33 ms + 0.85 ms/MiB) that is roughly 0.3-7 ms.
- **The log:** one WAL fdatasync per sync where unsynced frames exist beside an open run. sync() already pays that over a hashed table.
- **The run's first sync:** one directory fsync, once per run.
- **After a sync:** the next piece write is off the 1 MiB grid, as after any due EveryN sync. That may matter for folio composition, per CLAUDE.md's write-pattern note.
- **adaptive_shape:** the device time moves from the seal's fsync to the sync rather than growing.

It should be priced with bench/scripts/pair.sh on the buffered ordered load (supdb-ingest arms) before shipping, after checking whether the buffered arm's adapter calls Db::sync at all; I did not verify that. It is not an arm, because the old shape is a wrong answer.

**problems.**
- Not reachable under the default SyncPolicy::Always, which syncs the run at every commit. It is reachable under any EveryN(n>1), which is a public policy rather than a bench arm, and the loss there is deterministic, not a race. A process kill after an Ok sync loses up to a piece's worth of commits that were never written. A power cut loses up to 8 MiB of the run, and under EveryN with no due sync, the WAL frames of a table whose seal is in flight. D needs the adaptive_shape arm (default false).
- The reproducer also observed that seal() under EveryN leaves the sealed table's WAL frames unsynced, which disagrees with EveryN's doc ('always at seal'). C depends on it as setup but does not test it. It is a separate item and should get its own test.
- The run's temp file never gets a directory fsync (28695-28699). B keeps the directory entry in its copy, so a fix that fdatasyncs the run but never fsyncs the directory would pass B, yet a real power cut could lose the run file's name. A fix's tests should also cover the name (at least by reading the code, or with a dir-fsync counter).
- C keeps the WAL's unsynced 8-byte header. That is a legal power-cut state, but it hides the related fresh-header-unsynced item (conclusion.md). A fix that syncs the WAL covers the header anyway.
- The seam is read-only: synced_to and direct_run_synced are called by nothing in the engine, so it changes no behaviour when unused. It is pub #[doc(hidden)] and not cfg-gated, which matches wal_durable's pattern.
- The file name is tests/s0_sync_ordered_buffered.rs, not the planned tests/durability.rs. That is cosmetic.

### run-tail-kill (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes, for a process kill. I read the syscall order at 3df85d22 and did not run anything.
- Under EveryN(u32::MAX), the run's commits issue write syscalls only for whole 1 MiB pieces (stream_flush(false) on the stream path, AlignedWriter's boundary write on the copies path). The sync in commit_direct (28800-28807) is never due, so no fdatasync runs.
- close_run (28917) writes nothing on the writer's thread. close_finish_inline is off by default and applies only when draining, so the else branch at about 29043 runs: the job parks at seal_hold_wait(SEAL_HOLD_LANDING), and finish, which writes the held tail together with the trailer, runs after it on the seal thread.
- The later key "a-later" sorts below the run, so it goes to the WAL. Its commit is a write_all with no fdatasync.
- The copy is taken with the seal thread parked before its finish. That is the page cache's state at that moment, and a process kill keeps the page cache and every directory entry, so no fsync or directory fsync matters here.
- The hold only widens a window that exists without it. With no hold, the seal job runs finish on another thread at some time after the writer has already returned from the later WAL commit. The window is the thread hand-off (15-200 µs here) plus the trailer's composition, longer behind other seals or under load.
- Test 2: Db::sync skips ordered tables (`!self.mem().ordered`, 29621). adaptive_shape is off by default (3933), so hand_tail does not run. The test's own premise (`!in_flight().0`, temp present) confirms that. After sync() returns, nothing writes the run, so the copy is the state a kill leaves at any instant afterwards.
- Neither test drops bytes an fsync had covered, and neither keeps a name that would need a journal commit.

**contract_promised.** Under a buffered policy, a kill may lose a tail of the commit order, never a commit before one it keeps. Where this is written:
- SyncPolicy's doc (src/db.rs:166-170): "The WAL is WRITTEN on every commit under every policy", and "an unsynced tail is lost whole and never served in part".
- Db::wal_durable's doc (31336-31340): a process kill "cannot tell EveryN from Always".
- docs/engine.md "Crash injection" (6982-7005): "which prefix of the commit order", and "every recovered state was an exact prefix".
- CLAUDE.md's crash-discipline paragraph.

Test 1 asks only for the prefix. That is weaker than wal_durable's doc, which would forbid any acknowledged loss under a kill. The run's tail being lost with nothing after it is pinned as intended by tests/db.rs:6610-6660 and is not asserted here.

Test 2 rests on Db::sync's doc (29611-29615): "Make everything written durable". Its own code then excludes an ordered table, so this is a doc-versus-code mismatch (P3) more than a breach of the commit-order contract. It is still a real acknowledged loss: sync() returned Ok and the keys did not survive a kill.

Db::seal does not promise durability on return (its doc at 29077-29091 says it is two-phase), so the BySeal case leans on the later WAL commit alone, which is correct.

**fails_for_stated_reason.** Yes. In run.log every premise held and both tests fail at their contract assertions, not at a premise or a panic in the engine:
- Test 1: the temp file existed, the WAL was unchanged by the run, the seal was in flight, and the WAL grew with the later commit. recover_direct on the copied temp file reported 0 of 300 records (small runs) and 8,000 of 12,000 (large runs, the last whole 1 MiB piece, the same figure tests/db.rs:6610 pins). The reopened copy read the later WAL key while 300 of 300, or 4,000 of 12,000, run keys were absent. All 8 combinations failed the same way.
- Test 2: 300 of 300, and 400 of 400 when staged, lost after sync() returned Ok, in all 4 combinations.

The values that were read back were checked as written. The only failure is absence, at the assertion at line 232 (test 1) and line 291 (test 2). The log shows one test run per invocation with "test result: FAILED" lines, not a watchdog exit. The log does not show the --list output, but notes.md says both tests were listed.

**fix.** Smallest acceptable fix for test 1 (P1): in close_run, on the writer's thread and before hand_seal and before any later commit can run, write what the PieceWriter holds with no sync. On the stream path that is stream_flush(true); on the copies path the AlignedWriter buffer, along with any batch_buf hashed as sync() does at 6921-6929. Every exit that closes a run must reach it: leave_direct through append_pinned, put and delete; Db::seal through close_direct; and hand_tail. The fix also makes the drain's presync of a clone cover the tail.

Mutants test 1 must fail under:
- the tail written only on the seal thread (today's code);
- the tail written on the stream path but not the copies path (or the reverse), which is why both direct_stream values stay in the test;
- the write placed in close_direct only, which misses Db::seal or hand_tail; keep the BySeal case, and add a hand_tail case under adaptive_shape;
- whole pieces written but not the partial last piece, which is why the small (under 1 MiB) and large cases both stay;
- the write moved after hand_seal into the job.

Test 2 needs P3: Db::sync writes and fdatasyncs an open run's file. Its mutants:
- write without fdatasync, which passes the kill test, so a power-cut emulation via a run_durable seam must also be required;
- sync of the WAL only (today's code).

For power cuts, a sync order is needed: a closed run's file is fdatasynced before the WAL's next sync by policy. A single shared commit sequence would let recovery cut both logs at the first missing commit number. It has to record:
- a commit number taken from one counter by every commit, carried in each run marker (about 8 B added) and in each WAL frame;
- the number each run opens at, in the run file's head, so a temp file with no marker still claims its range;
- the manifest's covered point and segment end_seq in the same space;
- recovery that gathers the commits above the covered point from the WAL chain and every unnamed temp, cuts at the first gap, and truncates both logs there before it sweeps or replays.

The sequence alone keeps a prefix only by discarding the later WAL commit. Keeping both commits under a kill still needs P1.

**fix_cost.** P1:
- Under Always it costs nothing: the last commit's sync has already written the tail.
- Under EveryN it costs one write per run close, under 1 MiB, moved from the seal thread onto the writer's path, with no fsync and no extra bytes.
- The last piece becomes two writes (tail, then index and trailer) instead of one write_buf_with. Per CLAUDE.md's folio rule, that changes the folio composition, so the reads over the segment have to be priced.
- Pair the buffered ingest arms (supdb-ingest against an arm without the write) with bench/scripts/pair.sh at 10k and 300k, over the load and the read passes, before it ships.

P3 adds one fdatasync of the run file per Db::sync with a run open. That sync is already promised by the doc, but it still has to be priced on the buffered load's closing sync.

The sync order adds up to one run-file fdatasync per policy sync after a run closes with unsynced commits. The shared sequence adds about 8 B per marker. Neither is priced.

**problems.**
- Not the default: it needs SyncPolicy::EveryN (the default is Always, where every run commit's d.w.sync() writes the held tail). It also needs direct_ingest, which is on by default. EveryN is the shipped buffered arm, so the reach is an arm, not a rarity.
- Test 2 reproduces P3 (Db::sync skips an open run), not P1. The two share a mechanism, the held tail, but have different fixes. P1 alone leaves test 2 red, and the coordinator should file it under P3's owner.
- The power-cut sibling (a later due WAL fdatasync while the run file was never synced) is not tested, so a P1-only fix would leave a prefix violation under power loss with no red test. It needs a run_durable-style seam.
- The test's copy_store tolerates NotFound and reads files while the parked seal thread and the upkeep thread are alive. I found nothing that writes store files in that window: seal_max_pct is 0, no merge is in flight, and the upkeep thread writes no files. This is read, not observed.
- The tests hard-code the temp name direct-00000000.tmp. That holds for a fresh store whose first segment id is 0, but it breaks if id allocation changes. That would make a premise fail, not produce a false green.

### delete-above-max (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes, for a process kill. I checked the syscall order by reading the source at 3df85d22.
1. The load's flush is landed durably, and settle() runs. The WAL then holds the delete+put batch, written and fdatasynced at its commit under Always.
2. Db::seal hands the frozen table to a seal held at seal_hold_wait. On the hash-table branch (29046-29050) that wait comes before finish(), so nothing is written in BeforeWrite. In BetweenPhases the segment is renamed and published but neither synced nor named by a manifest, and open sweeps it either way.
3. append(K) opens a direct run. commit_direct (28694-28699) creates direct-<id>.tmp with PieceWriter::create and writes the record and its mark(). Under Always `due` is true, so d.w.sync() fdatasyncs the file (28800-28806).
4. copy_store takes every completed write as it stands. That is exactly the page-cache state a kill leaves, and nothing in the copy goes past what was written.
Power cut: every byte the test relies on was covered by an fdatasync. One gap: commit_direct never fsyncs the directory after creating direct-<id>.tmp. Under a power cut the run file's name may therefore be lost entirely, which is a separate loss of an acknowledged write. When the name does survive, the state is this one. So the test file's header claim that "a power cut leaves the same" is slightly overstated, but the kill case is enough on its own.
The buffered test is a process kill under EveryN, and its premise is observed green: key(500999) recovers, so the first batch's piece and marker are on file. That is a reachable kill state.

**contract_promised.** Under SyncPolicy::Always (the default), a commit that returned Ok is durable. CLAUDE.md says "Commit is a WAL append and one fdatasync; the batch is durable". For ordered ingest it says a commit is "a commit marker ... then one fdatasync on that file", and recovery rewrites the records up to the last marker as a piece. The doc comment on Db::delete (28425-28427) says "later appends start fresh". Here a delete and then an append, both acknowledged in that order, reopen as the delete alone, so the acknowledged append is lost permanently.
Under EveryN only the weaker promise holds: CLAUDE.md's "every window in it is survivable", and the state-it-was-in property that tests/cipher.rs states for a store under a cipher. The recovered store has key(500000) deleted while key(500001..500999) from the same later batch are present. The store was never in that state. This is a wrong answer, not the loss of an acknowledged write.

**fails_for_stated_reason.** Yes. The log shows all three Always tests failing at tests/s0_delete_above_max.rs:192, the final assertion ("a write acknowledged after the key's delete reads back after the crash", left []). That assertion comes after both premises: key(500002) recovered from the rebuilt run piece, and key(10) reads "rewritten" from the replayed WAL batch. It also comes after both live-read controls passed.
The buffered test fails at line 299, after its premise at line 294 passed.
The put control passes, and it asserts that no direct run opened, which matches put's code (28406-28417): it leaves any run and always writes the WAL.
The mechanism matches the code. Db::delete (28428-28440) never raises max_key, while put (28414-28417) and the WAL branch of append (28239-28242) do. goes_direct (28337) therefore takes K as above the greatest, and append_pinned opens an ordered table (28202-28226). Open rebuilds the temp file as a piece named at the manifest's `sealed` (27745-27748), then replays the WAL past `sealed` into the memtable, which is newer than every segment (27869-27910). The tombstone masks the piece.
The reproducer also reports, though I did not observe it myself, a trace: with max_key raised in delete, all five tests pass.

**fix.** The smallest fix: in Db::delete, after leave_direct and before the WAL write, add `if key > self.max_key.as_slice() { self.max_key.clear(); self.max_key.extend_from_slice(key); }`, as put does. The invariant this restores: a run opens only above every key that a WAL frame past the manifest's covered sequence names.
I would also accept the alternative that orders replay by sequence: record the run's start sequence in its header and name the rebuilt piece above the frames it follows. That also repairs stores already on disk, but it is a recovery change and needs its own crash tests.
Mutants the fix's tests must fail under:
(a) the raise in delete removed;
(b) the raise placed only on the non-ordered path (before leave_direct) or under a wrong guard;
(c) goes_direct's `key <= max_key` changed to `key < max_key`;
(d) max_key_of (28379) skipping memtable entries that are tombstones. This needs one more test: delete K, commit, reopen so the tombstone is replayed into the memtable, seal with the hold, append K, crash, reopen, and read K. As it stands, an empty memtable blocks the run until the seal, so only a reopen-then-seal test reaches max_key_of.
Keep the three Always windows (BeforeWrite append, BetweenPhases append, append_batch), the put control and the buffered prefix test as they are.

**fix_cost.** The max_key fix adds one byte compare per delete, plus a copy of the key when it is a new greatest. It adds no syscall, no fsync and no byte on disk, and nothing to commits or drains, so no sync has to be priced. One behavioural effect: after a delete above the greatest, later ordered appends between the old greatest and the deleted key go through the WAL instead of a direct run. No suite workload deletes above the greatest, so I would expect no change in the suite's rows, but I have not checked that by a pairing.

**problems.**
- No seam was added, and the holds used (hold_seal_landing, hold_seal_durable) only park the seal thread. They cannot create the ordering; they only keep the window open.
- The test file's header says the copied state is also what a power cut leaves. That is not quite true: the direct run's temp file is created without a directory fsync (commit_direct 28696-28699), so a power cut may lose its name. That is a separate defect, worth its own item. The kill state alone is enough for this test.
- Reachability is rare. It needs a delete of a key above the store's greatest (usually a key that does not exist), then that table frozen and handed to a seal by Db::seal, by a size-triggered freeze, or by sync's hand-off, then an append or append_batch of that key into the emptied table with nothing staged, then a crash before the seal's manifest. The window is one seal's duration, from milliseconds up to the hundreds of milliseconds a large seal takes. The defect is reachable under the default Options. put is not affected.
- The buffered test's premise depends on the 1 MiB write-piece arithmetic (1,000 then 9,500 hundred-byte values). It is green on this tree, but it would turn into a premise failure, not a false pass, if the piece size or the seal bound changed.
- copy_store copies a live store without an atomic snapshot. With the seal held and the writer idle, nothing writes concurrently, so this does not matter here.

### owed-slot (stands: True; severity: data_loss_default)

**state_reachable.** Yes, if one premise holds: a power cut can tear an in-place write of a slot file. The engine's own design assumes it can (manifest_read_all's doc, and CLAUDE.md's "a torn publish is the slot an open ignores").

Syscall order, read at 3df85d22:
1. Publish N-1 is written by manifest_write_slot with its fdatasync. retire_covered then unlinks the closed WAL files and the merge inputs.
2. The next drain's seal runs land_readable, which fsyncs the directory. That journal commit makes the unlinks from step 1 durable.
3. publish_unsynced (db.rs:38851) does a write_all_at of slot N into MANIFEST_SLOTS[N%2] at offset 0 (11709). There is no fdatasync, and the File goes to slot_sync_due.
4. One of two things follows in the same drain:
   - publish (38886) write_all_at's N+1 with an fdatasync into MANIFEST_SLOTS[(N+1)%2], the file that holds N-1. Then `slot_sync_due.take()` drops N's File unsynced (38909).
   - Or a second publish_unsynced writes N+1 there without a sync and overwrites slot_sync_due (38869).
5. N is never fdatasynced. An fdatasync of one slot file does not write back the other inode's overwritten data pages, on ext4 data=ordered or on xfs.

So at a power cut during N+1's write, all of the following are allowed:
- N's dirty page is lost, or partly written back.
- N+1's write tears.
- The unlinks are durable.

The open (27631-27709) reads the slots and syncs none of them, so sub-case D (kill, then reopen) has the same shape.

How the test builds each state:
- The slot being written: its device bytes with the header's sequence restamped, so the CRC refuses it. That is a tear.
- The other slot: the bytes that file's last fdatasync covered, or those bytes torn.
- Every other file: as the page cache holds it.

Keeping the other files is the lenient direction, and both failures are about a missing file or a refused checksum, so leniency cannot have produced them. The test's own fsync of both slots at its start makes the device hold more than the engine guarantees. That is also lenient: without it, slot 1 on the device would hold seq 1, not 3.

No state the test builds drops bytes a sync covered, or keeps a name a journal commit could not have kept. The one premise is the tear. Without a tear the outcomes are only "N+1 whole" or "N+1 absent, N-1 intact", and both are survivable.

**contract_promised.** The test asserts the durable-store contract. Under SyncPolicy::Always (the default), every commit that returned Ok must survive a power cut, and a store must open.

Where the engine promises the slot behaviour the test relies on:
- CLAUDE.md, "Crash discipline is an order": the manifest is two slot files "written in place and fdatasynced, the newest valid slot by its publish sequence being the store at open, so a torn publish is the slot an open ignores". The same paragraph justifies the deferred slot sync as safe: "a crash inside it is a torn newest slot, the case the slots were made for".
- docs/engine.md, "The manifest in two slots".
- manifest_read_all's doc (db.rs:11884): "the torn one is the publish the crash interrupted and the other is the store".

All of these assume the other slot is durable while one slot is written. The deferral plus the parity choice of slot breaks that assumption.

The model only contains commits acknowledged before the armed flush, and those are all under Always. The test asks no more than the engine promises.

**fails_for_stated_reason.** Yes. In run.log every failure is the contract assertion and none is a guard. A, B and D each failed in 3 of 3 runs.

Sub-case A: the watched writes are [(5, slot1, deferred), (6, slot0, synced, slot0 last synced at 4)]. That is exactly "N deferred, then N+1 over the durable N-1". Two states fail:
- "6 torn, manifest.alt holding 3": the open falls back to seq 3, which "names segment seg-...17017.sup, which is not in the store". That segment was unlinked behind durable publish 4.
- "6 torn and 5 torn": the open refuses with "manifest failed its checksum".

These are the two outcomes the mechanism predicts. B fails with the same two messages at 6, 7 and 8, the three-seals-in-one-drain chain. D fails with the E4 F6 shape after `manifest_seq == 5` was asserted and passed.

I found no mistake in the test's slot parsing (seq_of matches tests/db.rs) or in how it tears a slot. The engine code at 38851-38920 is as the mapper read it.

**fix.** Smallest fix I would accept: choose the slot by durability, not by parity. Maint keeps durable_slot, the file whose last fdatasync returned. Every publish, synced or deferred, writes the other file. A successful sync of a slot sets durable_slot to it. At open, fdatasync the newest valid slot (or both) before the first publish, and seed durable_slot from it; that closes D.

The alternative also works: before any slot write, if slot_sync_due is Some, fdatasync it and retire behind it, then write. This also closes A and B, and D still needs the open-time sync.

Mutants the fix's tests must fail under:
1. The parity choice restored in publish only. A must go red.
2. The parity choice restored in publish_unsynced only. B must go red.
3. The open's sync of the slot it read removed, or durable_slot initialised by parity at open. D must go red.
4. durable_slot updated when the write is issued rather than after sync_data returns. A must go red.
5. sync_owed_slot no longer updating durable_slot. A second drain must then overwrite the durable slot; this needs a test with two consecutive drains.

The seam must record syncs at the open and at the manifest_pending write too, and SlotWrite::slot must carry the slot the fix chose, not seq%2. Keep the existing tests a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid and a_buffered_flush_is_answered_by_its_slot_and_needs_nothing_of_its_log green; the first asserts alternation by parity and will need its expectation restated.

**fix_cost.** Preferred fix (slot chosen by durability): nothing is added to a commit or a drain. There is no new syscall or fsync on the write path. The open gains one or two fdatasyncs of a slot file of a few KB: about 0.03 ms when clean and about 0.3 ms when a kill left it dirty. Those figures are CLAUDE.md's and were not measured here.

Alternative fix (sync the owed slot before the next write): one more fdatasync of about 0.3 ms on the segment work's thread, inside every drain where another publish follows the seal's before the answer. That is the common case under partition_on_flush, against a 10k-key drain of about 8.5 ms. It would have to be priced before shipping: pair.sh against the current default with SUPDB_DRAIN_STEPS=1 and SUPDB_LOAD_PHASES=1, beside supdb-waitmanifest.

**problems.**
- Reachability rests entirely on a torn in-place write of a slot file. The image is probably one or two KB, so it sits inside one 4 KiB page. A device that writes a page atomically never tears it, and then the defect cannot fire. The engine's design states that tears happen, so the test holds it to its own premise, but no tear was observed on any device. Without a tear, every outcome is survivable.
- The window is narrow. In A it is the in-flight write and fdatasync of N+1, about a millisecond per drain that has a later publish, though that is the common case under partition_on_flush. In B, N+1's own sync waits until after the flush answers, so the window runs past flush() returning Ok. Outside a test this needs a power cut, not a process kill, inside that window. Severity is data_loss_default because the configuration is the default and the contract is broken, but the event is rare.
- The watcher copies the whole store directory on the segment work's thread at every armed slot write. This slows the drain and may change which interleaving occurs; in A, B is the shape it met first. It cannot invent a write order: the recorded (seq, slot, deferred, synced) tuples are what the engine did.
- The `synced` bookkeeping only covers the three traced sites. Not traced: the open's own slot writes (27706-27708) and the manifest_pending arm's write. A test that starts after a reopen relies on `device` passed in by hand, which D does correctly.
- The kill copy in D is taken mid-drain, so files still being written are copied part-written. The open succeeded and asserted seq 5, so this did not affect the result. A file copied short could only make a state fail for another reason, and the failure messages rule that out here.
- Side observation, not a problem with the test: the seam shows publish 3 of the set-up drain was never synced by the engine. The condition, a durable slot overwritten while the newer one is unsynced, occurs in ordinary use, not only in the armed rounds.

### seal-after-failed-sync (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Case 2 (a failed data fsync after the readable landing) is reachable, and is the stronger of the two. The syscall order I read at 3df85d22: the seal writes the segment, renames it into place (~29511) and signals its names. land_readable publishes (37767), moves covered_seq (37769-37771) and fsyncs the directory (37798). The seal thread then calls seg.sync_all (29527), which fails. land_front's Err branch wedges only `if !landed` (37669), and its comment (37658-37664) says on purpose that "the next landing's manifest names them". The merge's landing then calls Maint::publish (38885), which pwrites a slot in place and fdatasyncs it, naming the segment and covering its sequence. On Linux a writeback EIO marks the pages clean and is reported once, so that segment's bytes are on the device in no known state: unwritten extents read back as zeros, and on ext4 the size may not be committed, because a failed fsync skips the journal commit. Truncating the file to zero, or leaving it at full length full of zeros, are both states a power cut can leave. Even a process kill can lose the bytes, once the clean pages are evicted and read back from the device. Errors where the file system keeps running after a data EIO do happen: dm-thin out of space, bad sectors, ext4 with data_err=ignore. The WAL frames survive, but replay skips them because the slot covers their sequence. Case 1 (a failed directory fsync followed by a later successful manifest) is close to unreachable on ext4 and xfs. A failed directory fsync means a jbd2 commit failure, which aborts the journal, or a log I/O error, which shuts xfs down. After either, the merge's slot pwrite or fdatasync fails too: ext4_da_write_begin or writepages returns EROFS or EIO. Only a transient failure could produce case 1. The test's removal of the link is also realistic only if no journal commit happened afterwards. No later sync_dir runs in the test: the merge's 12867 ran before the rename, and the slot is written in place with fdatasync and no size change. But ext4's periodic 5 s commit would make the entry durable in practice. Case 1 shows the engine breaking its own comment (37794-37797), more than a likely loss.

**contract_promised.** CLAUDE.md, "Crash discipline is an order": "no manifest is written while a seal's segments are published and unsynced -- it would name a segment that may be torn and cover a sequence the WAL still has to hold". The Maint::publish doc says the same, as does the comment at 37794-37797 ("the state a crash before the publish leaves: the WAL holds the writes"). The test runs under SyncPolicy::Always, the default, where a commit that returned Ok is durable. The test asserts exactly that contract: no slot names the piece or covers its end sequence, and every acknowledged key reads back after the cut. A buffered store would promise only what a sync() acknowledged. The same mechanism applies there after an Ok sync().

**fails_for_stated_reason.** Yes. run.log shows both tests panicking at the first manifest assertion (lines 231 and 300). Each names seg-00000011-...0063.sup inside the newest slot's names. Each test asserts first that its fault was consumed (seal_sync_fault_pending false), and test 1 also asserts seals_wedged, so neither passes or fails because a seam did not fire. The notes say the traces confirm land_front's Err branch ran with landed=true in case 2, and a merge landing after the wedge in case 1. With the asserts skipped, the open refused: "not in the store" in case 1, "file too short to hold a superblock" in case 2. That is the consequence the tests claim. In case 1 I cannot tell from the log alone whether the merge's landing or the drop wrote the offending slot. Either is a manifest naming the segment, so the claim holds.

**fix.** Smallest fix I would accept, three parts:
(a) In land_front's Err branch, set seal_wedged whether or not the seal has landed. A seal thread that fails after its readable landing is wedged, not retried.
(b) Maint::publish, publish_unsynced, the pending-slot path in land_readable, and so promote_ranges and every merge landing through collect/join_merge, refuse with the wedge error while seal_wedged is set. No slot is written and retire_covered is never reached.
(c) Preferably, move the covered_seq update in land_readable below the sync_dir/helper.wait, so a failed directory sync leaves covered_seq where it was.
Then fix the comments at 37658-37664 and 37794-37797.

Mutants the fix's tests must fail under:
1. The `if !landed` guard restored. Test 2 must fail.
2. The wedge check removed from publish, merge path only. Tests 1 and 2 must fail.
3. The wedge check present in publish but missing in publish_unsynced or the pending path. This needs a test under manifest_pending or a drain-deferred landing.
4. A fix that retries sync_all or sync_dir once and goes on. This needs a sticky seam or a seals_wedged assertion in test 2.
5. A next seal's landing (not a merge's) naming the piece and retiring a rotated WAL. Add a third test with a small wal rotation size, requiring that the WAL file holding the failed seal's frames stays on disk.
6. The direct run close's failed sync_all (29004).
The reopen must replay the WAL and return every acknowledged key, by point read and scan, through kv/vf.

**fix_cost.** Nothing on the write path. No syscall, fsync, write or byte is added to a commit, drain or landing. The cost is one atomic load of seal_wedged per manifest publish (per landing, never per commit) plus a store on a failure path. No benchmark pricing is needed. If the covered_seq move is taken, it changes only the order of a field update relative to an fsync that already exists.

**problems.**
- Case 1's trigger is a failed directory fsync followed by a successful manifest write and fdatasync. ext4 and xfs do not produce that: a failed journal or log commit aborts or shuts down the file system, and every later write or fsync fails. Treat case 1 as a breach of the engine's own comment and contract, not a likely loss. Case 2 carries the defect.
- The seams fail once, then pass. A 'fix' that retries seg.sync_all, or sync_dir, after the error would pass both tests, yet on Linux a retried fsync proves nothing: the error was reported once and the pages are already marked clean. The fix's test should keep the fault sticky (fail every later sync of that file or that seal), or assert seals_wedged after case 2.
- Case 1 also needs a merge in flight across the seal, one that dir-fsynced before the seal's rename. A merge or promotion started after the failure would dir-fsync again and cover the entry. Under the model that case is only a publish while wedged, not a loss.
- The WAL-retirement consequence (a later seal's land_durable -> publish -> retire_covered removing a rotated WAL file) is not exercised: wal-00000000 stayed on disk. The loss shown is the covered sequence plus the refused open, not a removed WAL.
- The power cut in case 2 is modelled as truncation to zero. A full-length file of zeros is equally possible and would fail the open the same way. This is not a defect of the test, but a fix should not rely on the length check alone.
- The second test's name says the seal's 'data fsync failed after its publish'. The direct run's close site (29004) also has a seam but is untested, so a fix must cover that path too (an ordered-ingest run close whose sync_all fails after the landing).

### manifest-pending-rewrite (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes for both tests. I checked the syscall order at 3df85d22 by reading the code. I compiled and ran nothing; the failures quoted below are the reproducer's run.log.

Test 1 (kill, reopen, power cut):
- The single commit of 3000 keys under SyncPolicy::Always is fdatasynced in the WAL (84 KB). That is the acknowledgement.
- The seal writes the piece through finish_unsynced. sync_ahead defaults to 8 MiB, and this piece is far under it, so the writer's helper thread never fdatasyncs it.
- write_ord_unsynced, then rename tmp to seg-...3001.sup.
- seal_readable signals the names, then parks under SEAL_HOLD_DURABLE before `seg.sync_all()` (29524-29531).
- The segment work writes the pending slot, and the SyncHelper does slot.sync_data() then dir.sync_all() (36684).
- mem::forget stands in for a kill: the parked seal thread never fsyncs, just as a dead process would not.
- The reopen's pending_whole (28309-28325) only reads. Then manifest_write_slot (27701-27706, 11695-11704) writes and fdatasyncs a slot naming the piece durable, covering the seal's sequence. No fsync of the segment, the ord file or the directory.
- So after the reopen: the slot and the WAL are durable, and the piece's data is covered by no fsync. Its directory entry is covered by the helper's dir fsync at most.
- ext4 data=ordered with delalloc forces no data on a new-name rename (auto_da_alloc only fires on rename-over-existing), and xfs is similar. A power cut can therefore leave the piece whole, with a run of pages unwritten, empty, or (if the entry was lost) missing. Every one of those except whole fails Seg::open or a block checksum.
- The test's two tears (4 KiB of zeroes at 8 KiB, and set_len(0)) are therefore legal states. It drops nothing an fsync covered.

Test 2 (failed fsync, then close):
- The seam returns Err at the point the seal's first sync_all would run.
- Under Linux errseq semantics, after an fsync EIO the pages may be marked clean and never reach the device, and a retried fsync reports success. So "the piece is torn after a power cut" is legal whether the real error came from the first file's fsync or a later one.
- The close's slot is fdatasynced by publish (38893-38903) after Maint::stop discards the join result (37386-37387) or land_front returns the Err with landed=true (37660-37673). So the durable slot naming the piece durable, beside a torn piece, is a real end state.

Caveat: the emulation runs the third open in the same process, beside the forgotten Dbs' threads. Those threads stay parked or idle, and none syncs the piece.

**contract_promised.** Under SyncPolicy::Always a commit that returned Ok must survive a power cut.

CLAUDE.md, "Crash discipline is an order, and every window in it is survivable", says of this arm: "a pending name counts only once the segment reads back whole, else the slot before is the store -- and retires the WAL only behind a slot that names the segments durable". It also says a manifest never names "a segment that may be torn and cover a sequence the WAL still has to hold".

The manifest_image doc (db.rs ~11640) says "a manifest names only what is durable".

Maint::stop's own comment ("The seals joined above are durable") is false on the error path.

Reading back whole through the page cache is not durability. The open turns a verified-in-cache name into a durable one, and the slot's covered sequence makes later replays skip the WAL frames that are the commits' only durable copy.

**fails_for_stated_reason.** Yes.

run.log shows `--list` naming both tests. Each then fails at the third or reopen Db::open, with the message the test builds:
- Test 1: "the open named seg-...3001.sup durable: true; it fsynced: []" then "key index checksum mismatch in piece 0" for Tear::Run. For Tear::Empty, run alone by a temporary loop edit, "file too short to hold a superblock".
- Test 2: "the close's slot names seg-...3001.sup durable: true", the same checksum refusal. A trace (since removed) showed the injected failure firing and then Maint::stop publishing the name durable.

wal-00000000 (84,029 bytes) is still present, and replay skips it because of the covered sequence. So the failure comes from the engine's covered sequence and not from a test mistake.

In both runs the failure is a refused open, not wrong values. Test 2's final assertion (no durable name) is never reached, because the open panics first.

The nearest existing tests pass with the seams in: a_pending_manifest_is_the_store_only_once_its_segment_reads_back_whole, a_clean_close_leaves_no_pending_name and a_failed_presync_is_the_flushs_answer.

**fix.** Smallest fix I would accept:

(A) At the open, only when the newest slot has pending names that pass pending_whole: fsync each pending segment, and its ord file under ord_durable, then fsync the directory once, before manifest_write_slot writes the all-durable slot. The alternative is to rewrite nothing and leave the names pending until a seal-equivalent sync has run.

(B) In Maint::stop, publish the all-durable slot only if every seal joined since the last publish returned Ok(names). Record a 'failed after readable' flag where land_front gets an Err with landed=true. On any Err or panic, leave the pending slot as the store's truth so the next open verifies it and the WAL replays. At runtime under pending, a seal that fails after its readable landing must keep its names pending, or wedge the store, rather than be named durable by the next publish. Do this together with the default store's failed-seal-fsync fix.

Removing the arm is defensible, since its doc says it bought nothing here. Removal deletes case 1 outright, but not case 2's default analogue, and it removes the experiment from bench/src/engines.rs. I prefer fixing it, because (B) is owed anyway.

Mutants the tests must fail under:
- open fsyncs the segments but not the directory. Needs a test variant that removes the piece when the directory was not synced.
- open fsyncs the directory but not the segments.
- open fsyncs only the first pending name. Needs a seal of two pieces, so a partitioned store.
- open omits the ord file under ord_durable.
- open records names in open_synced without syncing them. Needs syscall-level evidence or an fsync-counting seam that counts actual calls.
- stop discards the seal's Err again.
- stop re-fsyncs and then publishes durable. The !named_durable assertion catches this.
- a runtime landing after the failed seal names the piece durable. Needs a test that commits and seals again after the injected failure, before dropping the store.

Write path: nothing. The open's cost is one fsync per pending segment, one per ord file, and one directory fsync, only during recovery from a crash inside a pending window. The close's cost is a Result check per joined seal. No commit, seal or drain gains a sync, so no benchmark pricing is required.

**fix_cost.** None on any commit, seal or drain path. Open: only after a crash that left a pending slot, it adds N segment fsyncs, plus N ord fsyncs under ord_durable, plus 1 directory fsync, once per recovery, after pending_whole has already read those segments. Close: a branch per joined seal, with no syscalls on success. Runtime: an error path only.

**problems.**
- Reachable only under Options::manifest_pending, which is off by default (arms supdb-manifestpending, supdb-ingestmanifestpending). Case 1 needs a process kill inside a seal's window between its pending slot and its fsync (milliseconds a seal), then a reopen, then a power cut before writeback and a device flush (seconds). Case 2 needs an fsync EIO.
- What the tests observe is a refused open. The acknowledged frames are still in the un-retired WAL, so the data is recoverable by hand but the store will not open. By reading only (not run): if the reopened store had kept running, a later durable landing would retire wal-00000000 behind a manifest that covers it, and that turns the refusal into real loss.
- The open_synced seam records intentions, not syscalls. A fix that pushes a name into open_synced without actually fsyncing it makes test 1 pass. Test 1 also never emulates losing the directory entry, so a fix that fsyncs the segment but not the directory also passes. The test should take what the open synced from the real calls, or add a variant that deletes the piece when the open did not fsync the directory.
- Test 1 stops at Tear::Run's panic, so Tear::Empty is exercised only by a hand edit. After a fix, both variants run, which is fine.
- Case 2's mechanism is not specific to the arm. land_front's comment (37663-37668) says a seal that failed at its fsyncs after its readable landing stays live, "so the next landing's manifest names them". That is the default store's failed-seal-fsync defect, a separate S0 item. Here the arm only adds the stop() publish as a second route.
- The fail_seal_sync seam adds one atomic swap per seal when unused and changes nothing else. It is not behind a cfg; the existing seams are not either. It injects the error before any fsync is issued, so the test does not exercise Linux's clean-after-EIO behaviour. The test's !named_durable assertion is what rules out a 'retry the fsync' fix that would look correct here and be wrong on Linux.

### run-temp-dir-barrier (stands: True; severity: hardening)

**state_reachable.** The tests construct no on-disk state. They assert that a barrier was issued, which is the right form for this defect, because the state it protects against can only be made by a power cut and a test cannot cause one.

Syscall order, read at 3df85d22 (db.rs:28689-28807) and agreeing with the log:
1. `commit_direct` calls `vault::remove_file(tmp)`.
2. `PieceWriter::create` creates `direct-NNNNNNNN.tmp` with open(O_CREAT).
3. The batch and its marker are written with pwrite.
4. When the commit is due, `d.w.sync()` fdatasyncs that one file.
5. The commit returns Ok.

No `sync_dir` falls between steps 2 and 5. The first directory fsync that lists the name is the landing's, after the close's `hard_link`. The seam observed this, and its control passed: it did record the landing's fsync with a `.sup` name.

Under POSIX, a power cut after step 5 may leave the file's data with no directory entry naming it. Open finds a run only by listing `direct-*.tmp`, so it would return Ok without the acknowledged batches. The reproducer's demo confirmed that open answer by deleting the tmp by hand; that demo was not kept.

Whether a real power cut can produce the state depends on the filesystem, and I only read this, I did not observe it:
- ext4 with a journal: fdatasync of a new inode waits on the jbd2 transaction that holds the create, so the entry is committed with it.
- XFS: forcing the log to the inode's LSN covers the create.
- ext4 without a journal, which is this mount: `ext4_sync_file` calls `ext4_sync_parent` for an inode in EXT4_STATE_NEWENTRY, so the entry is written as well.
- A process kill never loses the name.

So the window is real against the contract the engine states for itself, and against POSIX. In practice it is closed on ext4, XFS and btrfs, and stays open on filesystems without those properties, such as ext2, vfat and some FUSE or network filesystems.

**contract_promised.** The engine states the rule itself:
- docs/engine.md, around line 7012: "commits into it are acknowledged from then on and an fdatasync of a file does not promise the entry that names it", which is why the seal fsyncs the directory once a new WAL exists.
- db.rs:27035-27040 (`rotate_wal`): "The directory barrier the new name needs before anything is acknowledged into it", paid through `wal_dir_unsynced` and `wal_dir_barrier`.
- CLAUDE.md, "Ordered ingest has its own log": the run's commit is acknowledged by "one fdatasync on that file", and recovery walks "the temp file's record stream".

Under `Always` every commit that returns Ok is durable. Under `EveryN(n)` the commit that issues the policy's sync is the acknowledgement. Together these require the tmp's name to be durable before the run's first acknowledgement.

Test 3 relies on `Db::sync` promising durability of everything before it. Under a policy that never syncs at a commit, that is a weaker, separate promise, and today it is not kept for the run's data either (P3). Its barrier assertion is still a requirement, but that test mixes in a second defect's precondition.

**fails_for_stated_reason.** Yes. run.log shows all three tests failing on their barrier assertions, alone and together:
- Test 1 at line 91 (the first run, `Always`).
- Test 2 at line 134 (`EveryN(2)`).
- Test 3 at line 162 (`EveryN(u32::MAX)` with `sync()`).

Test 1's control assertion, that the landing's directory fsync was recorded with a `.sup` name, passed before its failing assertion, so the seam records. The three existing WAL-barrier tests pass with the seam in place.

`barrier_covers` accepts any recorded fsync that listed the name. That is correct because the check is made before the flush in test 1, and right after the acknowledging call in tests 2 and 3, so no later fsync can satisfy it.

The reproducer also reports that a temporary `sync_dir` right after the create turned all three green. That is evidence the failure is exactly this missing call and not an artefact of the test.

**fix.** Smallest fix I would accept: a `vault::sync_dir(&self.dir)?` in `commit_direct`'s `if self.direct.is_none()` branch, right after `PieceWriter::create` and before the batch is written. That is one barrier per run, independent of the policy, and it also turns test 3 green without coupling to P3. An error from it must fail the commit.

If the fix is made lazy instead (a `run_dir_unsynced` flag paid in the `if due` branch, cleared only on success), it must also be paid in the run sync that P3 adds to `Db::sync`.

Mutants the fix's tests must fail under:
- (a) the `sync_dir` deleted;
- (b) the `sync_dir` moved before the create;
- (c) the barrier paid only for the store's first run (test 1's second run catches this);
- (d) in the lazy form, the flag cleared before the fsync or on its failure. This needs a failure injection at `sync_dir`, and the commit must return Err, not Ok;
- (e) in the lazy form, `Db::sync` not paying the barrier (test 3).

Also keep a test that the barrier lands on the run's own tmp name, not just on any directory fsync, which the seam's name listing already gives.

**fix_cost.** One directory fsync per run on the writer's thread, at the run's first commit, and nothing per commit after that. A run lasts up to the seal threshold, so a 10k-key ordered load pays one.

docs/engine.md prices a directory fsync of a new name at about 0.3 ms here. Because nojournal ext4's fdatasync already syncs the parent, the cost may be close to the 0.03 ms of a no-op sync if it is issued after the first fdatasync. That is an estimate, not a measurement.

It sits on the load's commit path, and at 10k keys it is a visible share of a drain of about 8 ms. It must be priced with `bench/scripts/pair.sh` with `--passes load` at the smallest rung (10000), under both the durable and the buffered arm, before it ships. The pre-created-spare alternative takes it off the commit path at the cost of an open-time sweep.

**problems.**
- The loss is not reachable on the filesystems this engine runs on. Journaled ext4 and XFS commit the create with the file's fdatasync, and nojournal ext4 (this mount) syncs the parent of a new inode. I only read this; I did not observe it. The defect is a breach of the engine's stated rule and of POSIX, not an observed data loss. That is why I chose severity hardening rather than data_loss_default.
- Test 3 depends on P3 (Db::sync does not sync the run's data). Its assertion checks only the barrier, so it can pass while P3 stays broken. P3 needs its own test under sync-ordered-buffered.
- The seam lists the directory before the fsync, so a name created by another thread between the listing and the fsync is missed. That can only produce a false negative, which is conservative for this test. The pending-manifest helper's dir.sync_all(), reached through vault::open_dir, is not recorded. It cannot fall in these tests' windows.
- The seam is a process-wide static that is not behind a cfg. That matches the existing seams. Unused, it costs one relaxed load beside each directory fsync. The change from `File::open(..)?.sync_all()` to `sync_all()?; Ok(())` returns the same result, so behaviour is unchanged.

### slot-damaged-rollback (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** The exact state the tests build is media damage, not a crash outcome. In test 1 the newest slot (seq 1) was fdatasynced by the close's sync_owed_slot before the run's temp was retired, so a flipped byte in it is a device that lost a synced sector. A process kill or a power cut cannot leave that slot torn. The order is manifest_write_slot_unsynced, then sync_owed_slot's sync_data, then retire_covered; checked at src/db.rs:11702-11719, 38851-38883 and 37852-37888.

The open's view of that state is reachable by a power cut, though, through the owed-slot superseding (e4-model F6, "W U W"):
1. publish seq N writes slot A and fdatasyncs it, and retire_covered runs.
2. A drain's land_durable calls publish_unsynced for seq N+1 into slot B. That is pwrite only, with slot_sync_due set.
3. The drain's promotion or merge calls publish() for seq N+2 into slot A, overwriting N in place. publish() drops the owed sync (`slot_sync_due.take()` at 38909) without issuing it.
4. A power cut while N+2's pwrite or fdatasync is in flight can leave slot A torn and slot B still holding seq N-1 from disk. N+1 was never synced.
The WAL and temps that N covered are already gone, so the open falls back to N-1 past retired evidence. Test 2's own log shows the drain-then-merge pair at a close (older seq 3 was publish_unsynced and was superseded by the merge's seq 4), so steps 2-3 are the ordinary drain shape. The window is one slot pwrite plus fdatasync (sub-ms to ~1 ms) right after a drain, and it needs a torn write of a slot of a few hundred bytes. Rare.

Test 2's refusal-after-sweep is also reachable by that route: the fallback slot names inputs a merge unlinked after its own synced publish.

A process kill cannot reach any of it: every write completed, and the page cache holds the newest slot whole.

**contract_promised.** Under SyncPolicy::Always (the default) an Ok commit is durable. Under any policy an Ok flush is durable. CLAUDE.md's "Crash discipline" paragraph says a torn publish is "the slot an open ignores" because the WAL and temps retire only behind a durable slot. manifest_read_all's doc (src/db.rs:11884-11889) ignores a failing slot "since the torn one is the publish the crash interrupted". The open never checks that premise against the retired WAL or temps.

For damage, the plain store's stance elsewhere (tests/segwriter.rs, "a checksum that cannot see a corruption") is that a detected corruption fails the open. It is not silently read as another state. CLAUDE.md also holds a refusal to leave "a directory left as it was".

So test 1's assertion is supported: refuse, or answer every flushed key. So is test 2's: a refusal deletes nothing. The promise is not explicit for media damage of a synced slot. It is explicit for the crash-reachable F6 equivalent, where an acknowledged flush or commit is lost.

**fails_for_stated_reason.** Yes for test 1. In 3 of 3 runs, Db::open returned Ok from the birth slot (seq 0, names nothing). The orphan sweep (src/db.rs ~27685-27692) unlinked par-00000000 before any check, and all 2000 flushed keys read absent. That is the stated silent rollback, at the stated assertion (line 128).

Test 2 fails partly for a different reason than its doc comment states. Both slots cover 7217, so this is not a rollback past a retired WAL. The older slot (seq 3, the drain's unsynced publish superseded by the merge's seq 4) names par-00000001 and seg-00000009, which the merge unlinked. The open sweeps par-00000010 and its ord file as orphans first, then refuses with NotFound. It fails on "a refused open leaves the directory's names as they were" (line 144). That is a real defect, destruction before refusal, but it is the sweep-ordering half of this defect, not the silent-rollback half. Its "names the manifest" check passes only because the NotFound text contains "manifest". No seam is involved; no engine file was changed.

**fix.** Open side, smallest acceptable fix:

(1) manifest_read_all reports that a slot failed its CRC.

(2) No unlink in Db::open, neither the orphan sweep nor others, runs before the fallback is accepted.

(3) When the open falls back past a failing slot, refuse (words naming the damaged manifest slot, directory untouched) if either holds:
- a name the fallback lists is absent;
- an unnamed par-/seg-/pcs- file exists whose end_seq exceeds the fallback's covered, or whose id is at or above the fallback's next id. The id check is what catches a run, whose segment carries no WAL evidence. Check it against drain_names_partition's par- naming.

Write side (the F6 companion; this is what makes the crash route unreachable): publish() issues the owed slot's fdatasync before it writes the other slot, instead of dropping it.

Mutants a fix's tests must fail under:
- the sweep moved back before the checks (test 2 and a refusal-branch test);
- the absent-name check deleted (test 2);
- the unnamed-later-segment check deleted (test 1);
- the id comparison off by one, '>' for '>=' (test 1, whose par id is 0 against the birth slot's next id 0);
- publish() dropping the owed sync. Needs a test that builds the W U W state: copy slot B's pre-unsynced bytes and tear slot A.

Control that must stay green (no false refusal): tests/db.rs a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid (~5534), whose forged torn next slot has its WAL still present.

**fix_cost.** Open-side checks: nothing on the write path. They reuse the directory listing and the names the open already parses, and move the unlinks later. F6 companion: no new fsync in the steady state, since the owed fdatasync is issued before the next publish instead of being dropped. A drain whose promotion or merge publishes right after the seal's unsynced slot gains one fdatasync of a small slot, about 0.03-0.3 ms here, on the segment-work thread before the drain's second publish. That changes the drain's latency and must be priced with bench/scripts/pair.sh at 10k and 300k keys (load passes, both sync policies, SUPDB_DRAIN_STEPS=1) before it ships. Commits are unchanged.

**problems.**
- The tests build the state by flipping a byte in a slot that was fdatasynced. That is media damage, not a crash outcome. The crash-reachable form needs the F6 owed-slot superseding plus a torn slot write during the next publish, and no test here builds it.
- Test 2's doc comment says the older slot covers less of the log and the open would answer the first values. The log shows both slots at covered 7217 and a NotFound refusal. Its precondition was loosened to 'covers less OR names files gone', so it tests the sweep-before-validation half, not the silent WAL rollback. The silent WAL rollback at rest was not reached.
- Test 2's older slot (seq 3) was never fsynced: publish() superseded it without syncing. Its bytes are the page cache's, so the 'older durable state' the fallback reads is not one a power cut guarantees.
- The refusal-wording assertion (contains 'manifest') is too weak. The accidental NotFound message satisfies it. A fix's test should require specific words for the damaged-slot refusal.
- Both cases also depend on the orphan sweep running before validation. A fix that refuses by name but keeps the sweep first would still destroy data. The directory-unchanged assertion catches that only in the refusal branch.

### both-slots-unreadable (stands: False; severity: hardening)

**state_reachable.** No kill, power cut or device error the engine survives can leave this state. I checked the syscall order at 3df85d22.

1. Db::create (db.rs:27330-27332) writes the `manifest` slot with vault::open_slot, which opens O_CREAT without truncate (vault.rs:1935-1943). It writes at offset 0 and calls sync_data, and only then does manifest_make_slots create `manifest.alt` and fsync the directory (11726-11740).
2. Create returns Ok only after all of that, and no par-/pcs- file can exist before create has returned.
3. Every later publish (manifest_write_slot, 11695-11722) writes in place at offset 0, never truncates, and fdatasyncs.
4. A torn in-place write leaves non-empty bytes that fail the magic or the CRC. manifest_read_all counts that slot as torn and refuses when no other slot is valid. The control test observed exactly this: "manifest magic is wrong", no file changed.
5. Publishes alternate slots and run one at a time, so a crash interrupts at most one slot, and the other was fdatasynced earlier.
6. The rename shape (manifest_slots=false) calls sync_all on the temp file, then rename, then a directory fsync (11777-11796), so it cannot leave a zero-length manifest either.

To get two zero-length or missing slot files beside par-/pcs- files, something outside the engine has to truncate or remove two files whose bytes and directory entries were already fdatasynced and journalled: an operator, a copy tool that skips them, or e2fsck clearing two corrupt inodes. The test makes that damage with set_len(0) or remove_file on slots an fdatasync had covered. By the workflow's own rule, that is dropping bytes an earlier sync covered. It is not a crash window.

**contract_promised.** The engine promises less than the test asserts, and the two promises it does make contradict each other.

- manifest_read_all's doc (db.rs:11884-11889) explicitly defines a missing slot or an empty slot ("made and never written") as "a store that has never sealed, or one written before manifests". It refuses only torn slots.
- Db::open's comment (27618-27622) and CLAUDE.md ("Every store has a manifest from birth ... that scan is kept for stores from before manifests alone") say a store from after manifests never takes the scan and must not.

Under the engine's own write order that promise holds, because the engine never empties a slot. Neither text promises to refuse, or to leave files alone, when an outside agent empties both slots.

The acknowledgement contract given for this workflow (durable commits, and Ok from sync/flush/settle surviving a kill or power cut) is not engaged, because the state is not a crash state. What the test asserts is a robustness property against external damage. The property is reasonable, and the code's own comment supports it, but it is not a durability promise.

**fails_for_stated_reason.** Yes. The log shows all three tests listed by --list (run.log lines 4-6).

- Emptied slots (fails at line 202) and removed slot files (fails at line 242): the open returned Ok. It removed 13 par- .sup, 13 pcs- .sup and 26 ord- .oidx files, and wrote manifest.alt (emptied case) or added both slot files (removed case). Afterwards key-000001 read [] and key-000007 read only v1, because the WAL replayed from sequence 0. The failure is at the unchanged-files assertion, as the reproducer stated.
- Zeroed control: refused with "manifest magic is wrong" and no file changed.

The mechanism matches the code as I read it:
1. manifest_read_all returns Ok(vec![]) (11898-11904).
2. With no manifest, the None arm takes only `seg-` names as live (27670-27682).
3. The orphan loop removes every on_disk par-/pcs- file (27686-27692).
4. manifest_write_shape publishes a manifest that names nothing.

I found no mistake in the test. Two notes:
- It drops the store without closing it, which only leaves a piece standing.
- It uses common::vf and the checked profile.

**fix.** The smallest fix I would accept is in Db::open, before the cipher sweep, manifest_make_slots and any removal. When manifest_read_all returns no valid slot, list the directory once (move the existing listing up). Then refuse with a named error, touching nothing, if either of these holds:

- (a) any par- or pcs- .sup file stands. Those names are written only by a store that has a manifest.
- (b) a slot file exists (empty) beside any .sup file. Create writes and syncs the first slot before any segment can exist, so this is also damage.

The scan stays for its one legitimate case: no slot file at all and only seg- names. A store with empty slots and no segment, which an interrupted create can leave, still opens.

The fix's tests must fail under these mutants:
- the check deleted;
- the check placed after the orphan loop, or after manifest_make_slots (the file-unchanged assertion catches it, because the removed-files case would gain slot files);
- the check matching only par- and not pcs-, which needs a fixture with pieces and no par- (or one assertion per prefix);
- the check refusing whenever there is no valid slot. a_store_from_before_manifests_opens_from_its_segment_names in tests/db.rs must stay green and catches this one.

Keep the zeroed control, so that reordering the open cannot regress the torn-slot refusal. Each test should also require that the store reads every acknowledged key once its slots are put back.

**fix_cost.** Nothing on the write path: no syscall, fsync or byte is added to a commit, seal, drain or publish, so the benchmark suite needs no pricing. At open, the existing directory listing moves ahead of the sweep and the slot creation.

**problems.**
- The constructed state (both slot files zero-length or missing beside par-/pcs- files) cannot come from a process kill, a power cut or a device error under the engine's syscall order. Both slots' bytes were fdatasynced and their directory entries fsynced before any segment existed, and no engine path truncates a slot. The test removes bytes an earlier sync covered.
- Not a durability defect under the stated acknowledgement contract. It is destructive handling of damage made outside the engine: the open deletes data instead of refusing, which makes a repairable store unrepairable. That is real and worth fixing, but as hardening.
- The engine's own doc for manifest_read_all defines an empty or missing slot as 'never written / before manifests', so the behaviour is as documented at that function. It contradicts only Db::open's comment and CLAUDE.md, which are claims about what the engine itself produces, and those claims hold.
- The mapper's original framing ('both slots empty or unreadable') was wrong for unreadable (torn) slots. The control test confirms those are already refused with no file changed.
- No seam was added, so there is no seam behaviour to question.
- Reachable under every Options value (both sync policies, manifest_slots on and off) once the damage exists. Likelihood outside a test: rare. It needs an operator or tool removing or truncating exactly the two small manifest files, or fs-corruption repair clearing both inodes. Not tied to any crash window.

### sync-completion-order (stands: True; severity: data_loss_arm_or_rare)

**state_reachable.** Yes, for the defect that was reproduced. It is not the defect the id names. I checked the syscall order by reading db.rs at 3df85d22.

Life 1:
- Db::create writes the log and syncs it.
- Batch a: write_all, then Wal::sync (fdatasync), so synced = 5219.
- Batch b: write_all with no fdatasync.
- A process kill (mem::forget). Batch b is in the page cache only.

Life 2:
- Db::open replays the log. There is no torn tail, so the truncation's set_len + sync_data at 27931-27934 does not run.
- Wal::open_append (4355-4358) sets synced = written.
- mem.commit() makes b visible.
- Db::sync (29621-29625) asks frames_unsynced() (4489-4491), which is false. The loose list is empty, because open sets loose_wals to Vec::new() at 28138. So no fdatasync is issued, and sync() returns Ok.
- Flush case: wal_unsynced is false (31030), so the drain defers (37684). The segment is fsynced and the directory synced. The slot is written in place, and its fdatasync is owed after the answer. The test holds that fdatasync with a new seam.

The power cut the test builds:
- The log is cut to the longest length any fdatasync of either life covered: life 1's 5219, and 0 issued in life 2.
- In the flush case, the slot just written is restored to its pre-flush bytes.

Both are states a real power cut may leave: unsynced data, and an unsynced in-place overwrite. No byte a sync covered is dropped. The log's name was made durable at create. The new segment is kept and is legitimately swept as unnamed.

Under Always, life 1 runs EveryN. That produces the same bytes as a kill between Wal::write (4442) and its fdatasync, because a commit is one write_all of the pending frames.

The trigger needs two events: a process kill, then a power cut after the reopen and before the kernel's own writeback, or the next fdatasync of the file, puts b on the device.

Not reachable: the defect as named. No mark is advanced by a sync that completed out of order:
- Wal::sync runs under &mut, with no write between issue and return.
- sync_wal syncs the loose files serially, then the live file.
- WriterHelper::run loads `landed` (6187) before sync_data (6218) and merges with fetch_max (6227). That mark is only a throttle; finish reads only the error.
- synced_all (5935-5942) uses fetch_max after the writer's own sync.
- The presync and the seal's sync_all set no mark.
- The owed slot is synced serially, or superseded by a synced newer slot.

**contract_promised.** The promises:
- Db::sync's doc (29610): "Make everything written durable."
- The Wal field doc: synced is what was behind a barrier at the last sync, and "the difference is exactly what a power loss may take". After a reopen the difference reads zero while a power loss can take all of batch b.
- The task's definition: a sync() or flush() that returned Ok made everything before it durable, under any policy.
- CLAUDE.md "Crash discipline": a buffered drain is answered behind the slot's fdatasync, because the log's durability covered that window. Here the log does not cover it.
- CLAUDE.md "A replay that builds the thing without the last step": the open commits what it replayed, so b is visible to readers before the acknowledgement.

A buffered store promises less for its commits, but not for an explicit sync() or flush(), and those are what the tests assert.

Under Always, b itself was never acknowledged in life 1. That makes the Always case weaker: it rests on sync() and flush() covering everything visible, not on a commit's ack. A later commit in life 2 under Always would fdatasync the file and cover b anyway. So the Always exposure is only a sync() or flush() with no commit after the reopen.

**fails_for_stated_reason.** Yes.

What run.log shows:
- `--list` shows all four tests.
- Each test fails at assert_every_key (line 113) on key b0000.
- After sync(), the trace reads wal_fsynced = 0 (no fdatasync issued in life 2), wal_durable = (10430, 10430), and life 1's synced = 5219. That is exactly the mark claiming coverage with no sync behind it.
- The flush tests read owed_slots 1 and owed_synced 0, so the deferred drain path ran as described.

What I only read, not ran: the reproducer reports that changing the replay mark to 0 (equivalent to head, through frames_unsynced's max) made all four tests pass. That localises the failure to db.rs 4358.

The power-cut construction does not drop bytes that were covered. The cut length is max(life 1's synced, life 2's fsynced), and life 2's fsynced is set only from Wal::sync and from the open's truncation sync. The only other fdatasync of the live log in the engine is sync_wal at 27077, through Wal::sync. Loose files are synced through another handle, but none exist here.

**fix.** Smallest acceptable fix:
- In Wal::open_append, set synced = head (not written) for a file whose frames replay kept from an unsynced source.
- Correct the comment: replay read the page cache, not the device.
- In Db::open, push onto loose_wals every replayed older log file that is not known to be synced. Simplest: every file in `retiring` whose successor's header is LOOSE. Simpler still: all of them, since a redundant fdatasync is about 0.03 ms.
- Alternative: one fdatasync of each replayed file at the open whenever replay kept frames, then synced = written is true. This costs one sync per such open.

Mutants a fix's tests must fail under:
- synced = written restored at 4358.
- The open not pushing the older files onto loose_wals, or not syncing them. This needs a new test: a kill with two log files where the live header is LOOSE, a reopen, sync(), a power cut that cuts the predecessor to its synced length, then an open that succeeds with every key.
- frames_unsynced reverted to ignore `head`, which would make every clean reopen pay.
- The deferral at 37684 not consulting wal_unsynced (the flush test).
- rotate_wal closing CHAINED after a reopen. This needs a test of its own that fills the log past the rotation size after a reopen.
- A control: a reopen after a clean close must issue no fdatasync, holding the steady-state cost at zero.

**fix_cost.** Nothing per commit or per drain in steady state. The cost falls only after an open that replayed frames:
- The first sync() pays one fdatasync of the live file, plus one for each older replayed file: about 0.3 ms plus 0.85 ms per MB dirty by CLAUDE.md's figure, and about 0.03 ms if the pages were already written back.
- The first flush after such an open is answered behind its slot's fdatasync once, about 0.4-0.9 ms.
- Under the sync-at-open alternative, the open itself pays those fdatasyncs instead.

A reopen after a clean close replays nothing and pays nothing. No suite pairing is owed for the write path, though a reopen's first sync or flush could be stamped if anyone wants the figure.

**problems.**
- The id names out-of-order sync completion. That mechanism is refuted by reading, and the mapper and reproducer agree, at the lines listed in state_reachable. What was reproduced is a different defect: a synced mark set at the open with no sync behind it (Wal::open_append, db.rs 4355-4358). Record it under its own name.
- The mapper's fix (synced = head) is incomplete when replay kept more than one log file. The open keeps the older files in `retiring` and starts with loose_wals empty (28138). If the live file's header is LOOSE, its predecessor was never synced. With the fix, the next sync_wal would chain() the live header and sync only the live file. A power cut could then cut the unsynced predecessor while the live file says CHAINED, and replay would refuse the open as a lost record instead of ending at the loose boundary. The fix must also put every replayed older file whose successor is loose onto loose_wals, or sync it at the open. No test covers this case.
- The rotation consequence is by reading only and untested: rotate_wal closes a replayed, unsynced file CHAINED and off loose_wals.
- The Always tests build life 1 under EveryN and argue the bytes are equal. That is sound for a single write_all, but the Always case is the weaker contract: b was never acknowledged in life 1, and any commit in life 2 under Always fdatasyncs the file anyway.
- The seams are added with no cfg gate, matching fail_presync and hold_seal_landing. When unused, behaviour is unchanged: `fsynced` is never read by the engine, the owed-slot hold is one atomic load behind slot_sync_due.is_some() on the drain path, and the counters are two relaxed adds per deferred drain. Even so, Wal::sync now takes an extra field store on every commit's sync path under Always. That is negligible but is on the commit path.
- Likelihood is low. It needs a process kill, a reopen, and a power cut within the window before the kernel's writeback (dirty_expire, about 30 s by default) or the next fdatasync of the log puts b on the device. The flush variant additionally needs the cut inside the sub-millisecond gap between the answer and the owed slot's fdatasync. A supervisor's crash-restart loop followed by a power loss is the realistic route.
