# Binding the run into the WAL: verdict on the lead's theory

Tree `3df85d22`, read only. Nothing was built, run, or tested on the engine. Sources:
- the readers: `today/notes.md`, `lifecycle/notes.md`, `model/notes.md`;
- the models: `today/xlog.py`, `lifecycle/model.py`, `model/model.py`, `scope/seqmodel.py`, `skeptic/engine_buf.py`, with their result files;
- two skeptics' findings.

All of these are under `/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/binding/`.

**What I re-read for this verdict, and confirmed:**
- src/db.rs:166-181, 4480-4507, 6293-6324, 6735-6775, 6860-7004, 27050-27081, 27626-27770, 28196-28255, 28320-28375, 28404-28441, 28470-28529, 28640-28684, 28686-28939 and 28940-29073 (`close_run` through the presync), 29606-29647, 31020-31031;
- src/db.rs:3835, 3851 and 3933 (the defaults: `sync: SyncPolicy::Always`, `direct_ingest: true`, `adaptive_shape: false`);
- bench/src/engines.rs:7189-7197;
- tests/cipher.rs:875-945.

Every other line cited here is cited from a reader and was not re-read ("by the readers").

## 1. Verdict

**The theory holds in its core and fails as stated.** It holds with changes, and the changes are larger than the theory.

**What holds:**
- Recording the run's last *synced* commit count once per run costs nothing per commit, because the count is a store after a sync that already happens.
- Checking at the open that the run came back with at least that many commits cannot fire after a clean power loss under the default policy. Every model agrees, and mutant M1 shows that counting the *committed* count instead does fire.

**What fails, in five ways:**
1. **Placement.** "The next WAL record" has a sequence at or past the run's `end_seq` (src/db.rs:28919), so no manifest ever covers it. Once the temp is retired and a merge folds the run's piece away, every later open refuses a healthy store as "run missing". The models found:
   - lifecycle: 846 + 888 refusals;
   - model reader: 109 at depth 6 and 4,758 at depth 8;
   - today reader: 4,616.
2. **The recovering open** rebuilds a run into a piece named at the old covered sequence (src/db.rs:27747-27768) and so leaves the binding live. The open then refuses after a later merge (176), or, with a void "settle", opens a never-held state through an older slot.
3. **Coverage.** It binds one of the three boundaries a run has. It misses:
   - run→run, the normal shape of an ordered load: a seal closes one run and the next opens at once (src/db.rs:28248-28251, 28327-28329);
   - WAL→run: the pre-run frames whose seal is still in flight.

   Both open states the store never held under damage.
4. **Under the buffered policy it does nothing.** Every buffered bench arm, `supdb-ingestcipher` among them, runs `EveryN(u32::MAX)` (bench/src/engines.rs:7193-7197). There, no run commit is ever due, so the synced count is 0 and the check never fires. The question's own case, commit 2 of the run damaged with WAL commit 3 after it, opens as {c1, c3}.
5. **The engine does not make "synced" mean what the theory assumes.** Three defects lie underneath, all confirmed by reading:
   - A commit that is not due leaves its batch in the writer's user-space buffer (src/db.rs:6743-6764, 6870-6895, 6293-6318). A process kill alone then loses run commits while a later WAL commit survives. That is a never-held state with no power loss and no damage, under `EveryN`, in a plain store today.
   - A failed run fdatasync is retried as Ok without syncing. `committed` advances before the sync (src/db.rs:28797-28807), and the retry returns early (28691-28693). This produces a never-held state under the default `Always`.
   - `Db::sync` with a run open syncs nothing of the run (src/db.rs:29616-29626).

**What keeps the guarantee.** The same idea extended to three links, a re-binding at the recovering open, an end rule for the unsynced band, and four engine fixes keeps the guarantee, and the price stays almost free:
- under `Always`: nothing per commit, no extra fdatasync, and about 100 bytes per run;
- under `EveryN`: at most one or two fdatasyncs per run, issued inside syncs the policy already asked for.

This is checked by models, one boundary set at a time; no single model covers all of it (§7). Decision 1 of the design should be neither (a), giving the guarantee up, nor (b), a write per run commit. It should be this (c).

## 2. The mechanism

### 2.1 Engine prerequisites (needed by any binding, and fixes in their own right)

| | what | where, thread | why |
|---|---|---|---|
| P1 | **Write the run's held tail at its close.** `stream_flush(true)` and `flush`, no sync. | `close_run`, on the writer's thread, before `end_seq` is read (src/db.rs:28917-28919) and before `hand_seal`. | Today the tail reaches the file only in the seal thread's finish, after the trailer is composed (src/db.rs:7584-7624, by the readers). A kill before that loses acknowledged run commits while later WAL commits survive. It costs nothing under `Always`, where the last commit's sync flushed already, and one write per close under `EveryN`. |
| P2 | **A failed run sync is sticky.** A failed `d.w.sync()` sets a poison (`pending_err`, or a `Direct::sync_failed` that every later commit returns). Alternatively the early return at 28691 is taken only when `synced == committed`. | `commit_direct`, writer, error path only. | Without it the retry returns Ok with nothing synced. The skeptic's model: LEAD with a failure gives a never-held state after a power loss; LEAD_RETRY is clean. The poison costs nothing per commit; the compare costs one. |
| P3 | **`Db::sync` syncs an open run** (`d.w.sync()`) and sets `synced`. | src/db.rs:29618-29626, writer. | Its own doc promises "everything written durable" (29611-29615). Under `EveryN(u32::MAX)` this is the only sync a run ever gets before its close. |
| P4 | **`frames_unsynced` ignores binding frames.** Compare a high-water of *user* frames, not `written`. | `Wal` (src/db.rs:4489-4491). It is read at `hand_seal` (31030), `commit_staged` (28675-28680) and `Db::sync` (29621-29622). | Without it an unsynced binding batch makes the next buffered drain wait for its slot's fdatasync (the 0.4-0.9 ms CLAUDE.md names) and gives `Db::sync` over a hashed table a WAL fdatasync for the binding alone. |

### 2.2 What is recorded

`Direct` gains `synced: u64`, set to `d.committed` after a completed `d.w.sync()` (src/db.rs:28804-28806) or P3's sync. Background syncs (the helper's at 6215-6227, the presync, `seg.sync_all`) do not count, so the field is a lower bound.

**L1, run → what follows (the lead's link, corrected).**
- **What:** its own WAL batch, one new record kind `WAL_RUN` followed by a commit frame. The fields are run id R, synced count s and committed count k. The run's end sequence is implicit: the frame's sequence plus 2.
- **Size:** about 54-62 B in today's plain frame format (model reader); 62/70 B if the end is carried explicitly (lifecycle). Under today's `LogCipher` add 32 B (lifecycle; vault.rs:1768 and crypt.rs:107-109, by the readers).
- **When:**
  1. In `close_run`, *before* `let end_seq = self.wal.seq` (28919), so that e_R = s_b + 2 and the run's own durable landing (covered = max end, 37770) covers the binding.
  2. Written with `wal.write()` *after* `hand_seal`, with no sync and no rotation at that write.
- **Thread:** the writer.

**L2, run → the next run.**
- **What:** the next run's binding block, about 33 B: the previous run's id, its s and k, and its end. Present only when no WAL user batch came between the two runs.
- **Where and when:** written once, where the next run's temp is created (`commit_direct`'s `direct.is_none()` branch, src/db.rs:28694-28720), ahead of the first record and inside the record stream, never in the per-commit marker path.
- **Durable with:** the run's first fdatasync.
- **Thread:** the writer.

**L3, the WAL before the run → the run.**
- **What:** the same block carries the WAL's last *user* commit position P at the run's open and the WAL's synced position s3 then, 16 B. Binding batches do not move P.
- **Why:** this is what makes the WAL's pre-run frames, whose seal may be in flight, a predecessor the open must reach.

**R, the recovering open.**
- **What:** for each temp it rebuilds into a piece, the open writes a WAL binding that is moot once the chosen slot's publish number is at least the number of the slot that named the piece (L1P).
- **Also:** the slot it writes (src/db.rs:27768) gains a WAL floor, 8 B, a new slot version. The open syncs the replayed WAL before writing that slot, because `open_append` counts replayed bytes as synced without syncing them (4355-4358, by the readers).
- **Costs:** one frame per rebuilt run and one WAL fdatasync per open that rebuilds.
- **Thread:** the opening thread.

**Sync order, `EveryN` only.**
- A run's due sync (or P3's) first syncs the WAL when the WAL holds unsynced pre-run frames.
- The first sync by policy after a close where s < k does three things in order:
  1. it fdatasyncs the closed run's file, which P1 has already made whole on the file;
  2. it appends `WAL_RUN(R, k, k)`;
  3. it syncs the WAL.

  This is the existing `loose_wals` loop (src/db.rs:27065-27077) with closed runs added.
- **Without P1** the sync is of a clone missing the tail. That is the skeptic's C_CLONE: a false refusal after a crash.
- **Without the order** (C_NOCHAIN), the end rule drops a WAL commit whose fdatasync had completed.

### 2.3 The recovery check, and the order of the open

1. **The order comes first, whatever check is used.** Today the open does all of the following before it replays the WAL:
   - sweeps envelopes;
   - removes unnamed segments;
   - rewrites slots;
   - rebuilds temps into pieces and unlinks the temps.

   That covers src/db.rs:27631-27770, re-read here; the replay at 27853-27909 is cited from the readers. A check placed in the replay is therefore destructive when it refuses, and a check "by id" passes on the short piece the rebuild just made.

   The open must read the chosen slot, the WAL chain and every temp, collect the bindings, decide, and only then sweep, rebuild, write slots and cut the live WAL file (27927-27935). It must also defer the replay's removal of later files at a loose end (27899-27906, by the readers). No new I/O is needed: the open reads all of these today.
2. **Collect.** The replay collects `WAL_RUN` frames into `bindings[R]`, the last one per run winning. The temps give each run's first block (L2, L3) and its recovered count r (`recover_direct(..).len()`, src/db.rs:6948-7004).
3. **Decide moot.** A link is moot if covered ≥ its end (decided by sequence, never by id), or if the chosen slot's publish number is at least the recorded one (R). Otherwise:

| state | condition | outcome |
|---|---|---|
| whole | r ≥ k | recover as today |
| damaged | r < s | **REFUSE**: "the run's file holds fewer commits than were synced" |
| lost unsynced tail | s ≤ r < k | **END**: the history ends after the run's r-th commit. The WAL replay stops before the binding batch, and later files are removed as the loose rule removes them. |
| missing, synced | no temp, no segment with id R, s > 0 | **REFUSE**: "a run the log names is missing" |
| missing, never synced | s = 0, k > 0 | **END** at the binding |
| L3 short | the replay's reach is below s3 | **REFUSE** |
| L3 lost unsynced | s3 ≤ reach < P | **END** at the first missing WAL commit |
| L2 | as the first five rows, for the previous run, read from the next run's block | the same outcomes |
| reopen slot | the chosen slot's WAL floor is not reached by the replay | **REFUSE** |

4. **Refusal text.** Each refusal is worded by what it found, not "damaged". The skeptic's device histories reach r < s with no attacker:
   - an EIO in the helper's background sync followed by a later Ok (6218-6226, by the readers);
   - a partial writev retried at the cursor (6046-6063, by the readers).

### 2.4 Format

- **Plain store.** An older reader meets the new kind at `_ => break` (src/db.rs:4706, by the readers), ends the log there, and truncates the live file to that point (27929-27935). That is a misread that destroys data. The WAL's kind bytes must move: new values for chained and loose that an older `wal_kind` refuses as "not a supdb WAL". Every header writer writes them, and an old-kind live file is re-kinded before its first `WAL_RUN`. The run file's first block is also new to an older `recover_direct`. It reads it as a record that does not parse and returns an empty run, which is silent. So it moves with the same version.
- **Cipher under Pagefile.** The marker moves at S4 whatever binding is chosen (vault.rs:364, 387, by the readers). Land the binding in that version.

## 3. Cost

| | under a cipher (Pagefile, S4) | plain store, if applied |
|---|---|---|
| per run commit, `Always` | nothing written. One u64 store after a sync that already happened. P2 costs nothing as a poison and one compare as the early-return form. | the same. It must be counted by callgrind against `origin/main` by function (`bench/examples/load_only.rs`, `work_probe.rs`); nothing has been counted. |
| per run commit, `EveryN` | nothing | nothing |
| per run, `Always` | L1: about 54-62 B and one `write(2)`, unsynced, on the writer. Plus 32 B per record while the WAL is `LogCipher` (until S6). L2 and L3: about 49 B in the run's first block, synced by its first commit's fdatasync. P1: nothing. | the same, without the 32 B |
| per run, `EveryN` | the above, plus P1's one write at the close. At most one fdatasync of the closed run's file and one 70 B binding at the first sync by policy after the close. At a run's first due sync, at most one WAL fdatasync if pre-run frames are unsynced. Under `EveryN(u32::MAX)` these fall at `Db::sync`, `flush` and `close`, which already sync. | the same. This is a behaviour change for a plain buffered ordered load and must be priced: `bench/scripts/pair.sh` with two arms that differ in it, under `EveryN`. |
| per open, clean | one match arm in the replay. The temps are read by `recover_direct` anyway. | the same |
| per open that rebuilds a run | one binding frame per rebuilt run, one WAL fdatasync before the slot, and 8 B in that slot | the same |

The runs are bounded: a run opens only over an empty table (src/db.rs:28327-28329), so there is at most one run per seal.

The lead said "about 8 bytes, once per run". It is about 100 B per run across two files, plus the open-time costs above. That is still almost free.

## 4. Scenarios checked

The models' bounds:
- **FULL** (model reader): every operation to depth 9, with power loss and damage combined to depth 6-8. 375,207 nodes under `always`; 1,076,819 under each idealised buffered order. Merges are included; promotion is not.
- **LOOSE** (today reader): `Always` to depth 12 (9,305 states); `EveryN` to depth 10 (10,237).
- **C_TAIL** (skeptic): the engine's buffered writer, the run→WAL boundary only, K=3, M=2.
- **M2** (scope): `Always` to depth 13 (94,351 states); buffered to depth 11.

| # | scenario | outcome under §2 | checked by |
|---|---|---|---|
| 1 | No damage, `Always`, a process crash or power loss at every point, with merges and a second open | never refused, never a never-held state | model (FULL d9, LOOSE d12) |
| 2 | No damage, ordinary lifecycle: close, landing, temp retired, merge, reopen | never refused (the lead's literal form refuses here) | model (model reader, lifecycle, today reader) |
| 3 | No damage, a recovering open, then a merge, then a reopen | never refused (L1 alone refuses 176) | model (model reader, lifecycle B0) |
| 4 | No damage, `EveryN`, power loss: the run's tail unsynced and a later WAL commit synced, by `sync_wal` or by writeback | END, giving a held state; no synced commit dropped | model (LOOSE d10, idealised writes). Engine buffering at run→WAL only: C_TAIL |
| 5 | No damage, `EveryN`, **process crash** with the tail in the writer's buffer | a held state, given P1 | model (skeptic C_TAIL, CLOSESYNC); by reading (6293-6318, 6743-6764, 6870-6895, 6917-6937, re-read) |
| 6 | No damage, `Always`, a failed run fdatasync, a retry, then a power loss | REFUSE or a held state, given P2 | model (skeptic LEAD_RETRY); by reading (28691-28693, 28797-28807, re-read) |
| 7 | The question's case: a byte in run commit 2, WAL commit 3 after it, `Always` | REFUSE (r < s) | model (all) |
| 8 | The same under `EveryN(u32::MAX)` | END: {c1}, a held state. Nothing in it had been promised durable unless `Db::sync` ran; with P3, s = k there and the outcome is REFUSE | model (FULL buffered, LOOSE); P3 by reading |
| 9 | Run temp deleted, or an older copy at each shorter count | REFUSE (s > 0) or END (s = 0) | model |
| 10 | run→run: the first run damaged or deleted, the second whole | REFUSE or END by L2 (lead: never-held) | model |
| 11 | WAL→run: pre-run frames cut, deleted or read empty, with the pre-run seal unlanded | REFUSE or END by L3 (lead: never-held) | model |
| 12 | Through a reopen that deleted the temp carrying L3, then the WAL cut | REFUSE by the slot's WAL floor | model (FULL against FULLN) |
| 13 | Newest slot damaged after retirement, **with** a run named only there | REFUSE (plain and D: never-held) | model (today reader D+F, model reader) |
| 14 | Newest slot damaged after retirement, **without** a run, no record ≥ the old covered left | ROLLBACK silently losing durable commits, in every variant including today's cipher | model (today reader). Not closed here; see §5 |
| 15 | A binding in a rotated, closed WAL file | used, not ignored (mutant M4 shows ignoring it opens never-held states) | model |
| 16 | Damage to the binding record itself | a cut; WAL commits after it go with it, giving a prefix | by reading (skeptic) |
| 17 | WAL kind byte flipped | REFUSE or a prefix today. Under Pagefile it is authenticated | by reading; not modelled |
| 18 | Two damages at once; both slots unreadable | not checked | none |
| 19 | `manifest_pending`, `recycle_wal`, `seal_rotates_wal`, drains with presync, promotion and `pcs-` naming, partitioning | not modelled. The binding is written after `hand_seal` so a drain does not wait (lifecycle, by reading) | partly by reading |
| 20 | Delete above `max_key`, a seal unlanded, then the run appends that key, then a crash | **never-held with no damage, which no count binding sees** (S1, §5) | by reading (28428-28440, re-read; 27747-27749, re-read) |

## 5. Other cross-file cases found, and what closes them

| case | today | closed by this mechanism? | what closes it |
|---|---|---|---|
| S1: `delete` does not raise `max_key` (src/db.rs:28428-28440, compare `put` at 28414-28417). A run can append a key that a pre-run WAL tombstone names; after a crash the tombstone replays newer than the rebuilt piece. | plain: never-held, no damage; cipher today: correct (one log) | no | `delete` raises `max_key` (one compare, already paid by `put` and `append`; counted by callgrind), or a replay ordered by sequence (M2, §6) |
| Newest slot damaged after retirement with no run (§4 row 14) | ROLLBACK in every variant | no | each WAL file's header carries its first sequence (8 B per rotation); an older slot whose covered value is below it is a gap. The unsynced `EveryN` case remains. Not modelled. |
| Every refusal after the open's first deletion is destructive (src/db.rs:27635-27770, re-read). The tamper test restores the directory before each flip (tests/cipher.rs:896) and never sees it. | both modes | the reorder in §2.3 fixes it for the binding's checks and the WAL gap | the same reorder, extended to `Seg::open` and `Pager::check` failures. A test that a refused open leaves the directory byte for byte as it was. |
| Both slots empty or unreadable leads to the pre-manifest scan (27656-27684, re-read), which removes every `par-`/`pcs-` and, under a cipher, every envelope. Pagefile's "no valid header means empty" for Replace makes one damaged header per slot enough. | needs both slots truncated | no | refuse a created store with no valid slot. A Replace or Log header that is present and fails is damage, not empty; keep "empty" for an absent or prefix header, as `Wal::resume` does (4223-4229, by the readers) |
| Pagefile's "a Log file with no valid header reads as empty" (design.md:239-241, 273) for the WAL | today's cipher refuses a changed header except under recycle | L3 turns a pre-run file read as empty into a REFUSE | the same header rule as the row above |
| Pagefile's manifest binds segments by logical name only (design.md:35, 323-333); the ordered index is tied to its segment only by name and `n == keys` (ordindex.rs:438-440, by the readers). An older authentic file put back may open. | today's sealed slot binds name to token and key | no | the slot lists each named file's `file_id` (16 B per name); the index records its segment's identity. Whether a reissued name can carry different content is not verified. |
| Pagefile: a manifest-named, non-pending Once file whose `FINAL` fails reads as untrusted, and its failures can surface as absent keys (design.md:311-317) | today's cipher poisons | no | refuse at open a named, non-pending Once file without a verified `FINAL` |
| S4: `loose_wals` is empty at open while the live header may say LOOSE; the first sync chains the header without syncing the predecessor (by the readers) | plain and cipher | no | populate `loose_wals` from the headers at open, or sync the chain there |
| A partial write retried at the cursor (WAL 4442-4443, writer 6046-6063, by the readers) | both | turns some silent losses into the REFUSE of row "damaged" | positional writes, or poison on a write error |
| The tamper test's doc says "the last or one before it"; its assertion is `history.contains` (tests/cipher.rs:876-877 against 922-925, re-read) | | | decide which is meant. Neither today's cipher, which cuts the live file at a mid-file failure, nor this mechanism keeps "one before it" for every damage. |

## 6. The cheapest working alternative, and the recommendation

**Sequence accounting (M2)** is the scope skeptic's design. It carries the lead's 8 bytes for free:
- the temp is named `direct-{id}-{s_open}.tmp`;
- `close_run` sets `wal.seq = s_open + d.committed` before reading `end_seq`, so the next WAL record's sequence *is* the run's count;
- the open replays one chain by sequence, WAL files and uncovered temps together, consulting the temps only in the gap branch, and keeps temps instead of rebuilding them.

**What it gives:**
- 0 never-held states and 0 false refusals in its model (`Always` depth 13, buffered depth 11, with two crashes and two merges);
- it binds all three boundaries and the slot-damage-with-run case;
- it fixes S1 by ordering.

**What it does not give:** a REFUSE for damage to a synced run commit. It answers that with a silent rollback below what was promised: LOSTDUR, 466,878 run-cut states under `Always`. That passes the tamper test's assertion but is weaker than today's cipher, which refuses damage in a closed WAL file under `Always`.

**What it costs:**
- recovery changes from rebuilding temps to replaying them into the memtable: memory at unclean opens, and the first seal after reopen goes through the WAL;
- `next_seg` must count kept temps;
- an older plain reader ignores the new temp name silently (the parse at src/db.rs:27719-27722, re-read), so the format moves too;
- the longer name counts against Pagefile's sealed-name budget, which is two characters under NAME_MAX (design.md:596).

**Recommendation.** Build §2: the lead's synced count, corrected and extended. It is the shape that keeps REFUSE for damage under the default policy, which is what the lead wanted the count for. Its per-commit price is nil and its per-run price is about 100 B. If the owner accepts "a held state" in place of "refused" for run damage, M2 is cheaper in bytes and simpler at the boundaries, but costlier in recovery code. M2 plus the lead's s, as a loose/chained kind at the run boundary, was proposed by both skeptics and is the likely best of both. It has not been modelled.

Under either choice, P1-P4, the open's reorder and S1's fix are owed first. Three of them are wrong answers in a plain store today (P1, P2, P3), so by CLAUDE.md they are fixes and not arms.

## 7. Still open

- **No single model covers the whole mechanism.**
  - FULL idealises the buffered policy as "a sync of every file", whereas the engine's syncs are per file, its counter is shared, and `Db::sync` skips the run.
  - LOOSE has the engine's sync order but writes every commit at the commit.
  - C_TAIL has the engine's buffering, but only at the run→WAL boundary.

  Owed: one model with the engine's `EveryN`, the buffered writer, L1-L3, R, rotation, merges and a promotion.
- Pagefile's Log recovery returning at least the last synced state after a power loss is on paper. E4 has not modelled `finish`, `TRUNC` or session bases (design.md:595).
- L1P assumes slot publish numbers never repeat after a damaged newer slot, and that the slot rewrite at an upgrade (src/db.rs:27698-27710) keeps that true. Neither is traced.
- The temp's directory entry is not synced at its creation (src/db.rs:28694-28711). That it is durable after the file's first fdatasync is assumed by the engine and not verified on this filesystem. The binding turns a failure of that assumption from a silent loss into a refusal.
- Where L2/L3's block goes in the streamed record path (`set_stream_source`, `mark_stream`): the stream must stay the file's record section byte for byte. Not designed.
- Whether the cipher fuzz runs `EveryN`. Not verified.
- Every cost above is arithmetic. Nothing was counted with callgrind or paired.
- Not modelled: `manifest_pending`, `recycle_wal`, `seal_rotates_wal`, drains with presync, promotion, two damages at once, and keys apart from S1.

## 8. Tests and mutants the mechanism needs

**Tests.** Each refusal is asserted by its words, and each holds a directory snapshot to "unchanged" after a refused open.
1. A tamper test with run-bearing histories, under `Always` and under `EveryN(u32::MAX)`. The histories are:
   - ascending appends, then a `put` of one of their keys, then more commits (the question's case);
   - an ordered load of several runs with no WAL commit between them (run→run);
   - a run opened with the pre-run seal held in flight (`hold_seal_landing`), for WAL→run.

   Today's history uses `put` alone (tests/cipher.rs:323-352, by the readers) and holds no run.
2. False-refusal tests: close, durable landing, temp retired, merge (`hold_merge_landing`), reopen; and a recovering open, a merge, a reopen. Each opens whole.
3. Power-loss images built by hand, as `tests/db.rs` builds crash windows. Each must open as a held state that includes everything synced:
   - the run's tail unsynced with a later WAL commit synced;
   - pre-run frames unsynced with the run synced;
   - a binding batch unsynced between two runs;
   - each of these with and without the chain binding.
4. A process-crash test that does not join the seals, which `Db::drop` does (src/db.rs:41745-41759, by the readers). This needs a seam or a child process killed. Under `EveryN`, a run commit not due, a `put`, and a WAL commit: the reopen holds a prefix.
5. A failed run sync injected (as `fail_presync` is, src/db.rs:29062), then a retried commit: an error, not Ok.
6. Format: the new WAL kinds are written; kinds 4 and 5 are still read; an old-kind live file is re-kinded before its first `WAL_RUN`.
7. The drain's slot wait counted after a binding batch, which with P4 adds none.
8. Callgrind rows for the plain store, against `origin/main` by function: `load_only.rs`, `work_probe.rs`, `lag0_probe.rs` and `read_only.rs`.

**Mutants.** Each must fail a test above. Mutants that need a buffered policy to show are marked "buf".
- M1 (buf): s counts committed rather than synced commits.
- M2: the binding is framed after `end_seq`, or coverage is decided by id.
- M3: a missing run with s > 0 passes.
- M4: a binding in a rotated closed WAL file is ignored.
- M5: no reopen binding, or a void settle in its place.
- M6: no L2.
- M7: no L3.
- M8: no slot WAL floor, or no WAL sync before the reopen's slot.
- M9 (buf): END where REFUSE belongs, and REFUSE where END belongs.
- M10 (buf): no P1, the tail left to the seal thread.
- M11: no P2, the retry returns Ok.
- M12 (buf): no P3.
- M13 (buf): the sync order removed, or the chain syncing a clone without P1.
- M14: the checks run after the sweep and rebuild, not before.
- M15: P4 removed, a cost mutant caught by the slot-wait counter.

The model mutants M1-M4 were already caught in the model reader's runs. M1 and the other buffered mutants equal the correct design under `Always`, where s = k, so they must be tested under a buffered policy.
