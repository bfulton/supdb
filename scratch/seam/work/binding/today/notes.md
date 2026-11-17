# Cross-file recovery: what binds the files today, and where the design opens holes

Tree `3df85d22`. Everything cited was read unless it says "not verified". Nothing was run on the engine. The model is `xlog.py` in this directory (results in `results.txt`), run under `nice -n 19 taskset -c 3`, each run under 4 s.

Labels: **T-C** today under `Encryption::Cipher`; **T-P** today in the clear; **D** the design with decision 1(a) (run temp a Log-class file, its own log, no binding); **D+L** D plus the lead's binding as stated; **D+F** D plus the corrected binding of section 5.
**NEVER** means the store opens as a state it never held. **ROLLBACK** means it opens as an earlier state it held, possibly losing durably acknowledged commits. **REFUSE** means the open fails.

## 0. Verdict on the lead's theory

The theory is right about the mechanism and its price, and wrong about its scope. One link, "the next WAL record carries the run's synced count", closes one of four directions. As stated it also false-refuses.

1. **It false-refuses on every normal reopen after the run lands**, unless coverage is decided by sequence. A plain run names its segment at the WAL sequence at close (db.rs:28919). The first WAL record after the close therefore has a sequence at or above the manifest's covered value and is replayed at every open. The temp is retired once the manifest names the run (db.rs:37884, 37925-37927). So "run missing, so refuse" fires on an undamaged store. Model: `RC WC LA`, which gives 4,616 false refusals under LIT (Always) and 0 under SEQ.
   - Exempting "the run's id is named by the manifest" breaks once a merge folds the piece into a partition under a new name (not modelled).
   - Fix: the binding takes a WAL sequence below the run's end_seq, so a manifest that covers the run covers the binding. The binding is then its own WAL batch, written at the close. It cannot sit pending: `direct_can_open` requires `wal.pending` empty (db.rs:28327-28329), so a pending binding would block the next run.
2. **It misses run→run.** A run closed by a seal is followed at once by the next run: the live table is empty and `direct_can_open` holds (db.rs:28327-28329, 29100-29104). That is an ordered load's normal shape, one run per seal. Between the two runs there is no WAL record at all, or only the binding batch, which nothing syncs under `Always` because run B's commits sync B's file alone (db.rs:28800-28807). Damaging A then gives B's commits without A's tail. Model: SEQ, `RC CL RC` with A cut or deleted, gives [2].
3. **It misses the predecessor direction.** The WAL's last commits before the run live only in the WAL while the seal of the table they froze into is in flight. A run opens over that empty table without waiting (db.rs:28248-28251, 28327-28329). Damage to those WAL frames, a WAL file cut short or deleted, or an older copy of it, with the run whole, gives the run without its predecessors: NEVER. Model: SEQ, `WC SE RC` plus a WAL cut, gives [2]. The same case comes through slot damage after retirement: `WC SE RC LA RT` plus newest-slot damage gives [2].
4. **Under `SyncPolicy::EveryN` a power loss alone opens a never-held state, on plain today and under D.** No damage is needed, and a synced-count binding cannot see it by construction. Two cases, model "never/no-damage":
   - **(a) Pre-run WAL frames.** A run's due sync syncs only its own file and resets `unsynced` (db.rs:28804-28807). Pre-run WAL frames that are still unsynced are lost while the run's synced commits survive. The seal does not sync the WAL when the policy is not due (db.rs:29111-29128), although `SyncPolicy`'s doc says "always at seal, flush and close" (db.rs:179).
   - **(b) The run's own unsynced tail.** A run's unsynced tail at its close can be lost while a later WAL commit's sync completes. `sync_wal` syncs the WAL files and never a run's file (db.rs:27055-27081). The run's file is fsynced by the seal thread later (db.rs:29004).

   That breaks the doc's promise "an unsynced tail is lost whole and never served in part" (db.rs:166-171). T-C has neither, because the run's frames are WAL frames and `log_run` → `sync_wal` syncs the WAL (db.rs:28862-28884).
5. **The open's order defeats any check placed in the WAL replay.** The open does the following before replaying the WAL:
   - sweeps the envelopes no valid slot names (db.rs:27635-27637);
   - removes the segments the chosen slot does not name (27688-27692);
   - can rewrite a slot (27698-27710);
   - rebuilds every direct temp into a piece, writes a manifest naming it, and unlinks the temp (27717-27770).

   It replays the WAL only after all of that (27853-27909). A binding refused there leaves the store changed. After the rebuild, a check "by id" passes on the rebuilt short piece. Every link has to be checked before the first deletion.

With the links of section 5, the model finds no NEVER and no false refusal under either policy:
- `Always`: depth 12, 3 runs, 6 commits, 9,305 states.
- `EveryN`: depth 10, 3 runs, 5 commits, 10,237 states.

The price stays "almost free":
- one WAL batch of about 60 bytes at each run close, with no sync;
- about 8-32 bytes in each run's first marker;
- no extra sync under `Always`;
- under `EveryN`, at most three extra fdatasyncs per run, and a run opens at most once per seal.

## 1. Today under `Encryption::Cipher`, file by file

**WAL** (`Wal::replay`, db.rs:4538-4581, cipher branch 4557-4580):
- **Header.** `Wal::resume` unwraps the file's key under `log_aad(id)`, the id taken from the file's name (db.rs:4220-4235, vault.rs:1694-1697, 1760-1765). A changed name byte or wrapped-key byte reads as "not this store's log", and the open is refused. There is one exception: under `recycle_wal`, a sealed-named header whose key fails reads as an empty log (db.rs:4565-4568). That is a silent loss of that file's records, so a ROLLBACK if it is the last file and a gap otherwise.
- **The kind byte (CHAINED/LOOSE, offset 7) is under no tag.** It is not in the wrap's AAD or any record's AAD (vault.rs:1791-1797, 1760-1765), and `chain()` rewrites it in place (db.rs:4498-4507). Flipping 4↔5, which is bit 0, turns a gap's refusal into the log's end, with the later files removed (db.rs:27899-27906), or turns the end into a refusal. The result is REFUSE or a prefix, never NEVER.
- **Records.** Each is `len | nonce | ct | tag`, and the AAD is (file id, offset, len) (vault.rs:1791-1815). `open_records` stops at the first record that is short or fails its tag, wherever it lies (vault.rs:1824-1857). A record failing in the middle is treated exactly like a torn tail: the file is valid to the last good record. If this is the live file, the open then cuts it there and syncs (db.rs:27927-27935), which destroys the records after the damage.
- **Across files.** The next file's first applied record is a sequence gap:
  - REFUSE if that file is CHAINED (db.rs:4661-4673);
  - the log's end if it is LOOSE and not the first file (db.rs:4609, 4651-4660).
- **Result:** REFUSE or a prefix (a held state).

**Manifest slots** (`manifest_read_all`, db.rs:11890-11913; `manifest_open`, 11679-11687; `Vault::open_manifest`, vault.rs:1657-1680):
- The tag binds the clear sequence (`slot_aad(seq)`, vault.rs:1753-1758, 1646).
- A slot that fails is "torn". Where another slot is valid it is ignored and the newest valid slot is chosen. With no valid slot, the open is refused, before anything is touched.
- An empty slot (0 bytes) is skipped (db.rs:11900-11902). With both slots empty, the open takes the pre-manifest scan (db.rs:27656-27684). Under a cipher, that scan runs after a vault sweep with nothing adopted (27635-27637), so every envelope is deleted. A changed byte cannot make a slot empty, but a truncation or replacement can (section 6).
- `adopt_all` refuses a slot whose file list does not match its names (vault.rs:1575-1587). `adopt` does not check that a segment's envelope exists (vault.rs:1541-1551). A missing one fails at `Seg::open` → `back_of` → `Back::open` (vault.rs:907-912, 919-956; NotFound was not followed into `Back::open`).

**Segment envelopes:**
- Bound to the slot by token and wrapped key.
- A changed byte is refused in one of two places: where the open reads it (`Pager::check` at the open's end, db.rs:27562-27564), or at the first read that wants it, after which every read fails.
- An older copy is impossible, since an envelope's token is drawn once.

**Ordered index** (`x-<token>.o`, under the segment's key, vault.rs:1552-1567): the CRC is skipped for paged sources (ordindex.rs:403-429). A failing chunk poisons the store: `open_ord` recomposes, but the pager remembers the failure and the open is refused at 27562. Result: REFUSE or unread.

**Form file:** a failing chunk poisons the store, so the store is unread (CLAUDE.md, "What is not the same as a plain store").

**Ordered run under a cipher:**
- No temp on disk. `direct-*` does not pass the door (vault.rs:556-564) and is a memfd, and the open lists no such name (vault.rs:1131-1140). So `recover_direct` (db.rs:6948-7004) is never reached.
- Each run commit's batch is a WAL record (`log_run`, db.rs:28862-28884, entered from 28834-28837 because the table is `logged()`, 28217-28223).
- The run's segment is named at the WAL sequence after its frames (db.rs:28919), and its envelope is written where it closes (28999).
- One log, so a damaged run is a cut of that log.

**The fixture** (`with_a_history`, tests/cipher.rs:323-352; `tiny`, 313-321): sixty commits of 30 `put`s, flush, settle, then forty commits of 2 `put`s, then dropped.
- **Ordered run: no.** `put` never goes direct, and leaves an ordered table if one is open (db.rs:28406-28423). The only direct path is `append` (db.rs:28198-28246).
- **WAL commit after a run: no.**
- **Rotation: yes, by reading.** `seal_bytes` is 16 KiB and `rotate_wal_by_size` runs after every commit (db.rs:28513, 26985-26992). The count was not verified.
- **Retired WAL files: yes, by reading.** Closed files retire at each durable seal landing (db.rs:37883-37887, 37897-37928). They are gone before the snapshot, so the test never tampers with them. Its flips of the newest slot do exercise "older slot after retirement", where the outcome is REFUSE or whole.
- **Not exercised:** recycle and spare (`recycle_wal` defaults to false, db.rs:3938), pending slots (`manifest_pending` defaults to false, db.rs:4005), and any kind-byte flip (the stride is `len/24`, so offset 7 is hit only if the stride divides 7).
- **Destructive refusals are not seen.** The test restores the whole directory before every flip (tests/cipher.rs:896) and counts a refused open without looking at what it left.

## 2. Today in the clear

- **WAL.** Same walk (`replay_frames`, db.rs:4584-4700). The batch CRC is seeded by the file id (db.rs:4165-4172, 4388-4418), so a changed byte ends the walk: a prefix, then a gap (REFUSE) or the end at a LOOSE file. The 8-byte header has no checksum, so a changed kind byte is the same flip as under the cipher. Nothing is forgery-proof.
- **Slots.** CRC over the sequence and the body (db.rs:11832-11844): the same choice as the cipher's.
- **Segments.** Key-section CRC row at open, block checksums at read.
- **Ordered index.** CRC at open (ordindex.rs:424-429), then `n == keys` (438-440). A failure recomposes (db.rs:8503-8526).
- **Form file.** `ZFile::fail`, then the raw form.
- **Direct run.** The temp is its own log: one fdatasync of it per due commit, and the WAL never written (db.rs:28690-28812). `recover_direct` walks to the last marker whose CRC, length and count agree (db.rs:6948-7004). A changed byte anywhere ends the walk there, so a damaged middle commit is a cut. The open rebuilds the temp into a piece named at the manifest's covered sequence (db.rs:27747-27768). The WAL replay then puts every record at or above that sequence into the memtable, newer than every piece.

## 3. Pairs

| pair | change / delete / older copy | T-C | T-P | D | D+L (as stated) | D+F |
|---|---|---|---|---|---|---|
| run temp → later WAL commits (the lead's case) | temp's commit k changed, temp deleted, or an older copy | n/a, one log: a cut → prefix | **NEVER** (`RC WC`, run cut or deleted) | **NEVER** (damage in the verified span or ring; damage below `synced_home` poisons → REFUSE, design.md:276, 313) | REFUSE; false REFUSE on normal reopens | REFUSE |
| run A → run B, no WAL commit between | A damaged or deleted | n/a | **NEVER** (`RC CL RC`) | **NEVER** | **NEVER** (binding unsynced, lost) | REFUSE (link in B's first marker) |
| WAL tail before a run ← run | pre-run frames changed, file cut or deleted, or an older copy, while the pre-run seal is unlanded | prefix | **NEVER** (`WC SE RC`, WAL cut) | **NEVER** | **NEVER** | REFUSE (run binds the WAL's commit position at its open) |
| manifest slot → WAL it covers → retirement (the asked case, §4) | newest slot changed after retirement → older slot | REFUSE (destructive), whole, or ROLLBACK | as T-C without runs; **NEVER** with a run (`RC WC LA RT`) | **NEVER** with a run | REFUSE for the run's own case; **NEVER** for the predecessor variant (`WC SE RC LA RT`) | REFUSE or ROLLBACK (ROLLBACK remains, §4) |
| manifest → segments it names | changed | REFUSE or unread | REFUSE or read error | REFUSE (step 4) or poison | = D | = D |
| | deleted | REFUSE, after sweeping the other slot's outputs | REFUSE | REFUSE | = D | = D |
| | older copy | impossible (token) | n/a | an authentic file that once held the same logical name opens as the store; reuse not verified (§6) | = D | = D unless slots list file_ids |
| pending segment (`manifest_pending`) | changed or deleted | envelope synced before the slot (db.rs:11661-11674); a changed chunk poisons → REFUSE | not whole → slot before, WAL and temp kept (db.rs:28309-28325, 37876-37887) → whole | no `FINAL` → slot before (design.md:317); with `FINAL` → poison | = D | = D |
| ordered index → its segment | changed | poison → REFUSE or unread | CRC → recompose | verify at open → recompose; later failure → poison | = D | = D |
| | deleted | recompose | recompose | recompose | = D | = D |
| | older copy | same bytes | same bytes | same bytes; bound by logical name and `n == keys` only | = D | = D |
| form file → its segment | changed | poison → unread | raw form | raw form, if every reader of a form page treats None as `ZFile::fail`; that list is owed (design.md:319, 597). A site that reads None as "absent" answers **NEVER** | = D | = D |
| | deleted or older copy | rebuilt or equal | rebuilt or equal (content identity) | rebuilt or equal | = D | = D |
| spare → recycled WAL | live WAL header changed | under `recycle_wal`: an empty log → ROLLBACK or gap; otherwise REFUSE | header lag reads empty | "no valid header → empty" (design.md:239-241, 273): ROLLBACK or gap; a pre-run file read empty with a run whole → **NEVER** | **NEVER** for that | REFUSE |
| WAL file → WAL file | middle file deleted or its kind byte flipped | REFUSE (CHAINED) or end (LOOSE), later files removed | same | same, the kind byte now authenticated | = D | = D |
| two slots, both unreadable | both changed | REFUSE | REFUSE | Replace "no valid header → empty" → both skipped → pre-manifest scan → **NEVER** or destructive REFUSE (§6), with two damaged files | = D | = D unless a created store with no valid slot is refused |
| any run ↔ WAL, `EveryN` power loss, no damage | — | prefix | **NEVER** | **NEVER** | **NEVER** | needs §5's sync order |

## 4. The asked case: the newest slot damaged after the WAL files it covered were retired

**Today, plain and cipher alike:** the WAL path is identical, and only the sweep's mechanism differs.

1. `manifest_read_all` drops the newest slot as torn, and the older slot N-1, with covered value c_old, is chosen (db.rs:11903-11912).
2. **Destruction comes first.**
   - Cipher: only N-1's segments were adopted, so `Vault::sweep` deletes the envelopes of every segment only slot N named: seal outputs, merge outputs, a run's segment (db.rs:27635-27637, vault.rs:1593-1624).
   - Plain: the orphan loop deletes the same names (db.rs:27688-27692).
   - Repairing the slot afterwards cannot restore the store.
3. If N's landing removed files N-1 names (merge inputs, a promoted piece's old name), `Seg::open` fails and the open is refused (db.rs:27801-27815).
4. Otherwise the WAL replays from c_old.
   - The first remaining file is `first`, so the LOOSE exception does not apply (db.rs:4609).
   - If its first record at or above c_old is not c_old (the files holding [c_old, c_N) were retired), the open is refused: "wal sequence gap: a durable record is missing" (db.rs:4661-4673).
   - If it is c_old (those records are still in a live file), the store is whole.
5. **The hole that is not refused: ROLLBACK.** If no remaining record is at or above c_old, nothing is checked and the store opens as N-1's state. The [c_old, c_N) commits, which were durable, are lost silently.
   - This happens when the live file was begun by a rotation with nothing committed since.
   - It also happens under `seal_rotates_wal`, which rotates at every seal (db.rs:29156-29159).
   - Under `EveryN` it happens without retirement: the covered frames may never have been synced.
   - Model: `WC SE LA RT` + newest-slot damage → [] in every variant, including CIPHER and D+F.

   To close it, a later file must carry what the slot said. One way: each WAL file's header carries the sequence it begins at (a `first_seq`), which closes the retired case. The `EveryN` unsynced case needs the newest publish recorded in a later file. This is not designed here.
6. **Plain with a run makes it NEVER.**
   - A plain run writes no WAL, and a run opens only over an empty table. The pre-run seal and the run therefore end at the same WAL sequence (seal: db.rs:29186; run: 28919, 29017-29020). Slot N names run A's segment; c_old equals A's end_seq.
   - After N is durable, A's temp is retired (db.rs:37884, 37925-37927).
   - With N damaged, A's segment is swept and A's temp is gone. The WAL commits after A replay contiguously from c_old: the store holds them without A.
   - Model: PLAIN, `RC WC LA RT` + newest-slot damage → [2].
   - T-C cannot do this: A's frames are WAL records in [c_old, A.end). It either still has them (whole) or meets a gap (REFUSE).

**Under the design:**
- **D (a):** inherits plain's behaviour, step 6 included. The design's own sweep rule, never deleting a name that fails to authenticate (design.md:15, 332), does not cover step 2. Those names authenticate; the chosen slot simply does not name them.
- **D+L:** A's binding replays with A neither recovered nor covered, so REFUSE. But the predecessor variant still opens NEVER: a pre-run commit's seal named only by slot N, its WAL retired, the run whole (`WC SE RC LA RT`).
- **D+F:** REFUSE in both. The ROLLBACK of step 5 remains.

## 5. What the binding has to be (the theory, corrected) and its cost

A link at every boundary between logs, each checked before the open deletes or rewrites anything:

- **L1, run → whatever follows (the lead's link).** At a run's close, the WAL gets its own batch (run id, committed count k) before the run's `end_seq` is taken, so `end_seq` = the batch's sequence + its frames.
  - At replay, a binding at or above covered means the run is covered by no manifest. Its temp must then recover at least k commits, or the open is refused. A missing temp is refused too.
  - Coverage is decided by sequence (covered ≥ end_seq), never by id.
- **L2, run → the run after it.** The next run's first marker carries (previous run's id, k, end_seq) when no WAL commit came between.
  - Free: the run's first fdatasync covers its first marker.
  - Without it, B's commits depend on L1's batch, which nothing syncs under `Always` before B's commits are acknowledged. The alternative, B's first sync syncing the WAL first, costs one fdatasync per run.
  - The marker, not page 0, because the finish patches page 0 (design.md:218).
- **L3, the WAL before the run → run.** The run's first marker carries the WAL's last user-commit position at its open. Binding batches do not advance that position, so a lost, unsynced L1 batch of the previous run cannot false-refuse. Recovery requires max(covered, WAL replay end) to reach it.
- **The check runs before the sweep.** The open has to:
  - read the chosen slot, the WAL chain and every temp;
  - check L1-L3;
  - only then sweep, rebuild temps and write slots.

  This is a reordering of db.rs:27631-27909, not new I/O: the open reads all of these already.
- **`EveryN` only, as sync order.** Model variant LOOSE.
  - A run's due sync first syncs the WAL when the WAL holds unsynced frames. While a run is open those frames are all pre-run.
  - A run closed with unsynced commits syncs the WAL, if it is unsynced, and then the run, before L1 is written. With whole-file fdatasync there is no other order: once post-run frames are in the WAL, syncing the WAL first makes them durable before the run's tail, and syncing the run first makes the run durable before the pre-run frames. The model found exactly that false refusal (`WC SE RC WC SY`) before this rule was added.
  - Recovery: a run whose recovered commits include no completed sync, and whose L3 is short, ends the history before the run instead of refusing. This needs sync evidence in the run's log. The design's Log class has its sync frames and CKPTs (design.md:233-251); plain's markers do not.
  - A synced-count link alone, the lead's choice, cannot close the `EveryN` power-loss holes. It is the sync order that does.

**Cost:**
- `Always` (the default, db.rs:3835): one WAL write of about 60 bytes per run close, unsynced (about 90 bytes as a sealed record under `LogCipher`), and 8-32 bytes in each run's first marker. No extra fdatasync, and nothing per commit.
- `EveryN`: at most one WAL fdatasync at a run's first sync, plus at most two fdatasyncs at a run's close with unsynced commits.
- A run opens only over an empty table (db.rs:28327-28329), so at most one run per seal.
- Applied to the plain store too, the same links fix §2's never-held windows. Under `EveryN` that is a plain behaviour change and needs pricing: a buffered ordered load, `pair.sh` with the arms differing in it.
- A refinement that avoids the WAL write in pure run→run loads: reserve the sequence at the close and write L1 lazily at that sequence with the next WAL write. Each run must reserve its own sequence, or a manifest naming A covers B. Not modelled.

## 6. Other holes found on the way (not about the run)

- **Every refusal after step 2 of §4 is destructive.** That covers a named segment missing, a WAL gap, and `Pager::check` at the open's end (db.rs:27562). All of them come after:
  - the sweeps (27635-27637, 27688-27692, 27779-27785, 27797);
  - slot rewrites (27698-27710);
  - temp rebuilds (27717-27770);
  - the live WAL cut at a mid-file damage (27927-27935).

  CLAUDE.md asks of other refusals "a directory left as it was". These do not leave one, and no test can see it (tests/cipher.rs:896).
- **Both slots empty or unreadable-as-empty → the pre-manifest path.** Every created store writes slot 0 at birth (db.rs:27324-27336). An open that finds no valid slot and no torn one is damage, but it takes the legacy scan:
  - only `seg-` names are live (db.rs:27672-27684);
  - every `par-`/`pcs-` is removed as an orphan;
  - under a cipher, every envelope was already swept.

  Today this needs both slots truncated to zero. Under the design, Replace's "no valid header → empty" (design.md:239-241) makes one damaged header in each slot enough. The result is NEVER, or REFUSE after the destruction. Fix: a created store (marker present, or any `wal-` with records) with no valid slot is refused; a Replace file with a header that is present but fails is damaged, not empty.
- **Design: "no valid header → empty" for Log files** (design.md:239-241, 273). Today a changed WAL header under a cipher is a refusal, except under recycle (db.rs:4220-4235, 4565-4568). Under the design it is an empty log: a silent ROLLBACK of that file, or NEVER for a pre-run WAL file whose run survives, without L3. "Empty" should be kept for a header that is absent or a prefix of what create writes, as today's `resume` does (db.rs:4223-4229), not for one that fails.
- **Design: the manifest binds segments by logical name only.** The layer "knows nothing of segments" (design.md:35), and the open resolves duplicate names by `name_seq` (design.md:323, 333). Today's sealed slot binds each name to its envelope's token and wrapped key (vault.rs:1633-1680, 1575-1587). Under the design, an authentic file that once held the same logical name opens as the store when put back in place of the current one, with the current entry deleted. Ids are reissued after a crash (next_seg = max live + 1, db.rs:27938).
  - Whether a reissued name can carry different content was not verified.
  - Fix: the slot lists each named file's file_id (16 bytes a name). The same applies to the ordered index, whose only tie to its segment is the name plus `n == keys` (ordindex.rs:438-440). The form file is bound by content (zfile attach), so it is safe.
- **Crash only, plain today and D, not T-C: a delete above the store's greatest key, then a seal, then a run that writes that key.**
  - `delete` does not raise `max_key` (db.rs:28428-28440), so the key goes direct (db.rs:28337-28345).
  - A crash before the pre-run seal lands replays the tombstone into the memtable, and the memtable is newer than the run's rebuilt piece. The key reads absent with the run's other keys present: NEVER, without damage.
  - The live store orders correctly: same sequence, then name by id (db.rs:5451-5457).
  - Not verified by a test. A fix is `delete` raising `max_key`, or the open applying a temp's records into the replay at its L3 position.
- **Stale doc.** `SyncPolicy::EveryN` says "always at seal, flush and close" (db.rs:179). `seal()` syncs only when the policy is due (db.rs:29111-29128), and a buffered flush commits under its policy (CLAUDE.md, crash discipline).

## 7. The model, and what it does not cover

**What it models:**
- Commits are distinct numbers 1..n. A held state is a prefix.
- Events:
  - WAL commit (closing an open run first, as `leave_direct` does);
  - run commit (a run opening over an empty table);
  - run close by seal;
  - seal of the WAL table;
  - landing of the oldest seal (its manifest, a run's file synced);
  - retirement (WAL records below covered, temps of named runs);
  - `SY` under `EveryN`.
- A crash keeps each log's synced prefix plus any prefix of the rest.
- Damage is at most one per outcome: the WAL cut, one temp cut or deleted, or the newest slot unreadable.

**Results** (`results.txt`):

| variant | Always NEVER | Always false REFUSE | EveryN NEVER | EveryN false REFUSE |
|---|---|---|---|---|
| CIPHER (T-C) | 0 | 0 | 0 | 0 |
| PLAIN (T-P, D) | 20,381 | 0 | 171,690 (23,023 with no damage) | 0 |
| LIT (lead as stated) | 13,989 | 4,616 | 313,440 | 33,755 |
| SEQ (L1 with sequence coverage) | 16,426 | 0 | 358,179 | 0 |
| FULL (L1-L3, no sync order) | 0 | 0 | 84,467 | 48,468 |
| LOOSE (L1-L3 + sync order + end rule) | 0 | 0 | 0 | 0 |

ROLLBACK losing durable commits through newest-slot damage occurs in every variant, CIPHER included (§4 step 5).

**Not modelled:**
- the WAL as several files (CHAINED/LOOSE kinds, rotation), with retirement modelled as dropping every record below covered;
- merges and promotions, hence the coverage-by-id failure, which is argued in §0;
- pending slots, recycle and the spare;
- damage to segments, indexes and form files;
- key semantics (§6's delete case);
- the open's destructiveness;
- two damages at once;
- a crash between the sub-syncs of the `EveryN` close rule (modelled as atomic; each intermediate state was argued to be a prefix, not checked).

The model checks the protocol's logic, not the engine.

## 8. Not verified

- Every engine claim above is from reading. Nothing was run, and no test was written or executed.
- How many rotations and retirements the fixture makes.
- `Back::open` on a missing envelope returning NotFound.
- Whether a reissued logical name can carry different content.
- The delete-above-max case.
- That T-C has no other cross-file NEVER: no case was found by reading, and the model's CIPHER variant finds none within its scope.
