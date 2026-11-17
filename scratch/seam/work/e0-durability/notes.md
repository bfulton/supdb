# E0 items 2 and 4: an audit by reading (tree 3df85d22, no build, no timing)

The store directory here is ext4 (`df -T`: /dev/vda ext4). Citations are file:line in /home/user/supdb.
Defaults that matter (src/db.rs:4002-4026): manifest_slots true, manifest_pending false,
flush_waits_manifest false, drain_syncs_wal false, ord_durable false, close_presync true,
direct_ingest true (3851), zpages_build_sync true (3910), recycle_wal false (3938).

## Item 2: a directory fsync before the slot that names a file is durable

### What a manifest names
Only segments: `seg-`, `pcs-`, `par-*.sup` (manifest_encode db.rs:11626-11647; open's on_disk filter 27638-27645).
The ordered index (`ord-*.oidx`) and the form file (`zpg-*.zpg`) are named by no manifest. The open
derives the index from the segment's name and recomposes it when missing (open_ord 8503-8527). The form
file is found by name and a missing one is `Reject::Missing` (zfile.rs:115-117).

### Per path, in order: create, rename/link, data fsync, dir fsync, manifest durable

**A. Seal output** (seg-, pcs-, or the first partition par-, all on the seal thread's job, db.rs:29246-29538)
- create `seal-{id}.tmp` 29406-29414 (PieceWriter::create 8021 -> SegmentWriter::create 6633 `vault::create_rw`)
- ord index: tmp 127-128, renamed unsynced 119 (call at 29509)
- rename to the final name 29511, readable signal 29520, data fsync 29527 (and ord 29530 only under ord_durable)
- slots, not pending (DEFAULT): the dir fsync is `land_readable` 37798, on the segment-work thread after the
  signal. The slot is written in `land_durable` -> `publish` 38897-38903 -> `manifest_write_slot`
  11702/11719, after the seal thread is joined (37643-37646, 37685). Order: rename < dir fsync < slot
  fdatasync, and data fsync < slot. **Safe.**
- slots, drain deferred (flush_waits_manifest off, the default; `defer` 37683-37684): the same dir fsync at 37798.
  The slot is written unsynced (`publish_unsynced` 38851-38871) and fdatasynced later (`sync_owed_slot`
  38876-38883). **Safe.**
- slots + `manifest_pending` (an arm): the slot naming the seal's segments PENDING is written first
  (37728-37737), *before* the segments are published. The helper does `slot.sync_data()` and THEN
  `dir.sync_all()` (36684). A power loss between the two can leave a durable slot that names segments whose
  entries are not durable. On ext4 that is reachable: an in-place fdatasync of a slot that keeps its
  size forces no journal commit. It is covered by the whole-check: `pending_whole` 28309-28325 runs
  `Seg::open`, which fails on a missing file, so the slot is skipped (27650-27655). The WAL is kept,
  because it retires only behind a durable slot that names the segments durable (37854-37886). **Safe by the
  whole-check, not by the order.** See defect 1 for the open that follows a process crash.
- rename shape (`manifest_slots: false`, an arm): no dir fsync at the landing (37787 is gated on manifest_slots).
  `publish` -> `manifest_write` 11765-11798: `manifest.tmp` create/write/fsync 11775-11777, rename over
  `manifest` 11795, `sync_dir` 11796. The segment's rename and the manifest's rename are made durable by
  ONE directory fsync, issued after both. By POSIX, a power loss between 11795 and 11796 may keep the
  manifest's rename and not the segment's. On ext4 (and XFS) both renames are journaled in time order,
  and a commit that carries the later one carries the earlier, so it is not reachable there. Not verified
  on other filesystems. A POSIX gap in an arm, not a bug on this box.
- Both policies: identical. The policy only picks `defer` (37684 `!wal_unsynced`) and whether the WAL is synced.

**B. Ordered run output** (close_run 28917-29061; same landing as A through hand_seal 29045/29052)
- create `direct-{id}.tmp` 28696-28699 (at the run's first commit)
- ord 28987, `hard_link(tmp, name)` 28989, signal 28994, data fsync 29004 (records presync 29029-29039 or 29053+)
- per shape: exactly as A (37798 / 36684 / 11795-11796). **Safe** in the default. The same caveats apply to the arms.

**C. Partition merge piece** (compact_run 12619-12877)
- create `compact-{id}.tmp` 12715. The create is 12549 (Emitter::enter) or 12721 (an empty fence), each after a
  `remove_file` (12547/12720)
- data fsync in `PieceWriter::finish` 8146-8149 -> `SegmentWriter::finish` 7506 (`sync_all`), at 12563/12724
- ord `write_ord` 12566/12726, rename 12567/12727
- **dir fsync 12867**, at the end of the job and before it returns its names
- manifest: `join_merge` 41662-41720 -> `publish` 41720. `collect` holds a merge landing while a seal is
  between its phases (37159-37167).
- Every shape and both policies: the dir fsync is inside the job. **Safe.**

**D. Piece merge** (tier_walk 12949-13037): `tier-{id}.tmp` 12983, the same Emitter, **dir fsync 13035**, then
returns. **Safe.**

**E. Promotion** (`apply_promotion` 38790-38838; its only callers are 38690 and 38784):
`hard_link` 38812, **sync_dir 38830**, publish 38833. **Safe** in every shape.

**F. The open's recovery of a direct run** (27717-27770): `seal-{id}.tmp` 27751-27761 (the finish fsyncs it),
ord durable 27763, rename 27764, **sync_dir 27765**, `manifest_write_shape` 27768, unlink of the temp 27769. **Safe.**

**G. The open's own slot writes.**
- A pre-manifest store gets its manifest at 27700. Before that, `manifest_make_slots` 27670 makes the slots
  and syncs the directory (11726-11739). **Safe.**
- The rewrite at 27701-27709 has a problem: see defect 1.

**H. Ordered index.** It is not manifest-named. `write_ord`/`write_ord_unsynced` (99-121) never sync the
directory and fsync the data only under `ord_durable`. A crash leaves it missing or torn, and the open
recomposes it (8503-8527). Correct by design.

**I. Form file.** It is not manifest-named.
- temp `zpg-..zpg.tmp` zfile.rs:4251-4255, create zfile.rs:4607, rename zfile.rs:4360
- data fdatasync zfile.rs:4378-4381 when `plan.sync`; never the directory
- A lost name means a raw read and a rebuild. Correct by design.

**Under a cipher today**:
- `persist_with` syncs the real directory after any envelope it made (vault.rs:1392-1394).
- seal 29525 and run close 28999 make their envelopes before the data "fsync".
- merge: `persist_one` 12574 (no dir sync), then 12866, which made nothing, then `sync_dir` 12867, which goes to the real directory (vault.rs:2086).
- `manifest_image` 11672: late envelopes, plus a dir sync if it made any, before the slot is written.
- A promotion's link is table-only (vault.rs:1998).
- Form envelopes: temp, rename, never a dir sync (vault.rs:1415, by design).

### Defects found by reading (plain store; none verified by a test; the tests' "crash" is a drop, which keeps the page cache)

1. **`manifest_pending` arm: the open after a process crash names doubtful segments durable.**
   - A process crash inside the pending window leaves the segments' data and names in the page cache, and possibly unsynced.
   - `pending_whole` (28309-28325) reads them back whole through that cache.
   - The open then writes a slot naming them durable (27701-27709 -> 11702). It does so with no fsync of their data and no `sync_dir`. Nothing between 27631 and 27706 syncs either.
   - The slot's covered sequence also makes replay skip their WAL frames (27874 `from = sealed`).
   - A power loss before writeback then leaves a durable slot naming torn segments, with the frames skipped, and the frames are retired at the next landing.
   - The arm's doc (3021-3022) says "The open and the close write a slot with every name durable".
   - The fix: fsync each pending segment and the directory before 27706, or keep the names flagged pending.
   - The close has the same shape (37386-37402): it joins the seal threads with `let _ = job.handle.join()` and then publishes the pending names durable, even if a seal's fsync failed (an error path).
2. **A failed dir fsync at a seal's readable landing leaves the seal's segments live and its sequence covered.**
   - `land_readable` publishes the segments (37767) and advances `covered_seq` (37769-37771) *before* `sync_dir` (37798).
   - On its error, the Err branch (37269-37283) says "its segments are unnamed and its sequence uncovered". Here that is false.
   - The branch pops the seal (37277), so `seal_between_phases` (37290-37292) reads false.
   - `collect` (37159-37168) then lands a finished merge, whose `publish` (41720) names `live_names()` with `covered_seq` past the seal's WAL frames.
   - It is reachable only after a failed directory fsync (EIO). By reading, not verified.
3. **A direct run is acknowledged into a name no directory fsync has covered** (both policies; not a manifest path).
   - `direct-{id}.tmp` is created at 28699, and each commit is made durable by the file's fdatasync alone (28800-28807 -> 6917-6936).
   - The WAL has the rule this lacks: "The directory barrier the new name needs before anything is acknowledged into it" (27035-27046; `wal_dir_barrier` 27085-27090; `sync_wal` 27064).
   - By POSIX a power loss can lose the name, and with it every acknowledged batch of the run, since the WAL never saw them.
   - On ext4 with a journal, the first fdatasync that allocates blocks commits the transaction carrying the create, so it is not reachable on this box. Not verified elsewhere.
4. (Incidental, outside E0) **`Db::sync()` with an ordered table under `SyncPolicy::EveryN(n>1)`.**
   - `commit_staged` -> `commit_direct` syncs only when the policy's count is due (28800-28807).
   - `sync()` skips its WAL branch for an ordered table (29621) and resets `unsynced` (29626).
   - `hand_tail` hands the run to a seal it does not wait for (29639-29645). So `sync()` can return with the run's records unsynced.
   - Its doc says "Make everything written durable" (29611). The policy's doc (179) lists seal, flush and close.
   - Not verified.

### Effect on the design
- **The E0 item 2 statement holds for the default shape, and for merges, promotions and open-recovery in every shape.**
  The rename shape (one dir fsync after both renames) and the pending shape (the slot synced before the dir) do not follow it literally.
- **§4 Open, step 4 ("a manifest names a file whose entry is missing: refuse the open") conflicts with `manifest_pending`.**
  Under that arm, a durable slot naming a pending segment whose entry was lost is a legal crash state, and today it falls back to the slot before.
  Under Pg the step must treat a missing entry for a *pending* name as "not whole", the slot before is then the store. The alternatives are to refuse `manifest_pending` under Pg, or to swap the helper's order (dir fsync, then slot).
- The rename shape is refused under Pg (§2), so its POSIX gap does not carry over.

## Item 4: names ending in .tmp

| name (format string) | made by | renamed / removed by | read after a crash? | the plain open today |
|---|---|---|---|---|
| `direct-{id:08}.tmp` (db.rs:28696) | commit_direct 28699 | linked 28989, kept until the manifest names the segment, unlinked by `retire_covered` 37925-37927 | **yes**: recovery 27717-27770 (`recover_direct` 6948) | if its id is live, removed (27728-27730); otherwise recovered and removed (27769); if empty, removed (27734) |
| `seal-{id:08}.tmp` (29406; 27751) | seal job 29413; the open's recovery 27755 | rename 29511 / 27764, same call | no | **left**. Removed only when the same id goes to a seal again (29407), or to a recovering direct run (27752) |
| `compact-{id:08}.tmp` (12715) | Emitter 12549, empty 12721 | rename 12567/12727, same job | no | **left**. Removed only when a later partition merge takes the same id (12547/12720) |
| `tier-{id:08}.tmp` (12983) | the same Emitter | rename 12567, same job | no | **left**, as compact |
| `ord-{id:08}-{seq:016}.oidx.tmp` (127) | write_ord_tmp 128 (callers 12566, 12726, 27763, 28987, 29509, 8520) | rename 105/119, same call | no | **removed** by the `ord-` orphan sweep 27779-27785, since it is never in `live_ord` |
| `zpg-{id:08}-{seq:016}.zpg.tmp` (zfile.rs:4251-4255) | write_pages zfile.rs:4607 | rename zfile.rs:4360; removed on every failure path 4356/4361/4387 | no | **removed** by `z_sweep` 27794-27798 / 31630-31643 (prefix `zpg-`, not claimed); held by tests/zpages.rs:3885-3906, 4529-4536 |
| `manifest.tmp` (11773) | manifest_write 11775 (the rename shape: create 27335, open 27700/27768, publish 38918) | rename 11795, same call | no (`manifest_read_all` reads only `manifest` and `manifest.alt`, 11893-11907) | **left**. Truncated by the next rename-shape publish; never touched under slots |
| `supdb.cipher.tmp` (vault.rs:441) | write_marker vault.rs:443 | rename vault.rs:447 | no | not a logical name. The cipher open removes it (vault.rs:850); a plain open ignores it |
| `x-<token>.z.tmp` (vault.rs:1456) | Vault::persist_form | rename vault.rs:1463 | no | cipher-internal; removed by `Vault::sweep` vault.rs:1598-1604 |

No `.tmp` other than `direct-*.tmp` is read at open, by recovery, or by a later rename. Each of the others is renamed in the same call or job that created it. After a crash, nothing picks one up, and a later job of the same kind and id removes it before creating it again. **The kill does not fire.**

### What the proposed sweep must respect
- **Match the engine's exact patterns, not the suffix.** The plain open leaves names it does not own (tests/zpages.rs:3891-3906 keeps `notes.txt`). A rule of "every logical `*.tmp`" would delete a caller's `foo.tmp` in the store's directory.
  - The patterns: `seal-`, `compact-` and `tier-` each followed by 8 digits and `.tmp`, and `manifest.tmp`.
  - The `ord-*.oidx.tmp` and `zpg-*.zpg.tmp` temps are swept already.
  - `direct-*.tmp` is never swept.
  - Except `supdb.cipher.tmp` in plain mode.
- **Run it at open, before the segment work, upkeep, builder and seal threads exist.** The open's own temps (27700/27751/27763, Seg::open's 8520) are created and renamed inside one call, so any position as a separate step is safe.
- **No lock excludes a second process or `Db` on the same directory** (no flock in src). The open already deletes unnamed segments, `ord-` orphans and `zpg-` files, so the temp sweep adds no new hazard of that kind.

### Corrections to the design's text
- §4 "Temps a crash leaks" says `compact-`, `tier-`, `seal-` and the index's `.tmp` are not swept by the plain open. The index's `.tmp` *is* swept (27779-27785), and so is the form's (27794-27798). The plain leak is `seal-`, `compact-`, `tier-` and `manifest.tmp`.
