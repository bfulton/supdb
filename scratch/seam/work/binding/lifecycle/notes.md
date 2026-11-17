# The direct run's lifecycle, and a binding of the run into the WAL

Tree: /home/user/supdb at 3df85d22, read only. Line numbers are src/db.rs unless named.
Model: `work/binding/lifecycle/model.py` (bounded enumeration; results in `run2.txt`).

## 0. Verdict on the lead's theory

The theory's central claim holds. The binding should record the run's last *synced* commit,
written once per run and nothing per commit. With that count, a clean power loss cannot make the
check fire, because the run's file already holds that count durably before any copy of the
binding exists. As stated, though, the theory refuses healthy stores in two ways, and one variant
of it loses detection:

1. **"The next WAL record after the close carries it."** That record's sequence is at or past
   the run's segment name `end_seq` (`close_run`, 28919). So R's own durable landing
   (covered := e_R, 37770) does not put the binding below the replay's `from`. The binding stays
   live after the temp is retired (37925-37927), and once a merge folds R's piece away, the open
   finds no temp and no segment with id R, and refuses. Model variant A under `always`: 846
   refusals of undamaged stores at the first open, plus 888 at a second open.
   **Fix:** frame the binding inside `close_run` before `end_seq` is read, so e_R = s_b + 2. R's
   durable landing then moves covered past it, and the existing filter (`s >= from`, 4661-4677)
   skips it.
2. **"Missing → refuse"** also fires after an open that recovered R's temp into a piece. That
   open names the piece at `sealed` and leaves covered where it was (27747-27768). The binding
   stays live until some later seal lands; if a merge folds the recovered piece away first, the
   next open refuses. Model B0 (B without the frame that fixes this): 2,232 refusals of
   undamaged stores at the second open.
   **Fix:** the recovering open appends a *settle* frame for R and syncs it before it returns.
3. **Carrying e_R without consuming a sequence** (variant Ae) makes e_R == s_open. The landing
   of the table sealed just before the run then covers it, and the binding goes moot while R is
   still undurable. The model shows 2,186 damaged states accepted as never-held under `always`.

There is also a gap that the theory as stated does not remove. Under `EveryN`, a plain store
already reaches a state it never held after a clean power loss (§1.8). The lead's binding (B)
leaves that gap in place. Variant C closes it: the binding also carries the committed count, a
shortfall inside the unsynced band ends the log at the binding, and the first sync by policy
after the close chains the run file.

Model, every policy (`always`, `every2`, `every3`, `never`), k ≤ 3 run commits, m ≤ 2 WAL
commits:

| variant | undamaged refused | undamaged never-held | damaged accepted as never-held |
|---|---|---|---|
| none (today's plain, Pagefile as designed) | 0 | 584 / 990 / 2,392 under EveryN | 2,744 (always) … 4,256 |
| A (lead, literal) | 846 (first open) + 888 (second open) under always; 0 under never, where synced is always 0 | as none | 0 under always; 1,032-4,256 under EveryN |
| B (fixes 1 and 2) | 0 | as none under EveryN | 0 under always; 1,032-4,256 under EveryN |
| C (B plus prefix) | 0 | 0 | 0 |

No variant drops a WAL commit that a completed sync had covered.

## 1. The run's lifecycle, read end to end

### 1.1 Entry and the run's open

- `append_pinned` (28198-28246) goes direct when `mem().ordered || direct_can_open()` and
  `goes_direct(key, value)` is true.
  - `direct_can_open` (28327-28329): `direct_ingest`, an empty memtable, and **an empty
    `wal.pending`**.
  - `goes_direct` (28337-28345): the key is non-empty, at most u16::MAX long and above
    `max_key`, and the value is inline.
- The first such write swaps in an ordered table (28204-28226). Under a cipher today it is
  `.logged()`, which sets `RUN_LOGGED`. The write then sets `max_key` (28227-28228) and appends
  to the table only. Nothing goes to the WAL.
- `put` and `delete` (28406-28441) and a non-direct append (28234-28236) all leave order first
  through `leave_direct`.
- **`delete` does not raise `max_key`** (28428-28440, compare 28414-28417 and 28239-28242).
  `MemTable::delete` makes a tombstone entry for a key never seen (10630-10640). See S1.
- The file is created at the run's first commit, in `commit_direct` (28694-28720):
  - id = `next_seg.fetch_add` (28695);
  - name `direct-{id:08}.tmp`;
  - `PieceWriter::create(.., sync_every 0, ..)`, with `head_reserve` 0 and `set_marks(true)`
    (28711);
  - `Direct { committed: 0, logged: 0, .. }` (28712-28720).
- No directory fsync is made at the temp's creation. The first acknowledged commit rests on the
  file's own fdatasync covering its new directory entry. That holds on journaled ext4. It is
  not verified on this filesystem.

### 1.2 A run commit

`commit` (28475) → `commit_direct` (28690-28839):

- If `committed == mem.len()`, it returns at once (28691-28693). So every marker covers at
  least one record, and the record count is unique per marker.
- Otherwise it lays out the batch's records:
  - stream path: `put_encoded`, `stream_out`, then `mark_stream` (6870-6895) puts the marker in
    the stream and writes the whole pieces;
  - copy path: `put_one` or `begin`/`value`/`end`, then `mark` (6743-6764).
- The marker (`marker_bytes`, 6768-6775) is 24 bytes: two zero u16s, `SUPD`, the CRC of the
  batch's records, the batch length, and the **cumulative record count** (`key_count()`).
- Then `d.committed = mem.len()` and `mem.commit()` (28797-28798). Readers see the batch from
  here.
- Then `unsynced += 1` (28799). The counter is **shared with WAL commits** (16251) and is reset
  by any barrier.
- If the policy says a sync is due (28800-28807), `d.w.sync()` runs: `SegmentWriter::sync`
  (6917-6937) does `stream_flush(true)`, `flush` and `sync_data`, then `unsynced = 0`.
- Under a cipher today, `log_run` (28862-28884) also sends the batch to the WAL as PUT frames
  plus a commit frame, `sync_wal` if due, and `rotate_wal_by_size`. That is the double write.
- Background syncs:
  - The segment writer's helper thread fdatasyncs the run's file whenever `sync_ahead` (default
    8 MiB, 7992) has landed unsynced (6215-6227). The writer never learns which markers those
    syncs covered.
  - At a drain the presync covers the rest (29029-29039 and 29061-29071).

### 1.3 Leaving the run

`leave_direct` (28354-28375):

1. It copies the uncommitted tail, the entries from `committed` to `len`.
2. It truncates the table to `committed` (28365).
3. It calls `close_direct`; an error is kept in `pending_err`.
4. It re-logs the tail into **WAL pending** with a hashed table (28369-28374).

So the first WAL batch after the close holds the run's last uncommitted writes, then the write
that ended the run. That batch is "commit 3" in the question.

### 1.4 The close

`close_direct` (28897-28911) calls `land_seal`, which waits for room in the frozen list
(30949-30987), then `freeze`, then `close_run`. `hand_tail` (29661-29685) reaches `close_run`
from `sync` under `adaptive_shape`, which is off by default (3933).

`close_run` (28917-29073):

- `end_seq = self.wal.seq` (28919).
  - Plain: nothing reaches the WAL while a run is open (`direct_can_open` requires empty
    pending, and every other write leaves order first). So `end_seq` equals the WAL sequence at
    the run's open, and that equals the `end_seq` of the table sealed just before the run
    (29186).
  - Cipher today: it covers the run's logged frames.
- Name: `seg-{id}-{end_seq}` before the first partitioning, else `pcs-{id}-{end_seq}-{lo}-`
  (28922-28933). Or `par-{id}-{seq}--` (28982-28986).
- `wals = take(closed_wals)` (29021): the files a size rotation closed. They retire with this
  seal.
- Presync clone only when `draining && close_presync` (28937-28939).
- The seal thread's job (28975-29015), after `seal_hold_wait`, runs in this order:
  1. the finish: trailer, Bloom and ordered index (28951-28971);
  2. `write_ord_unsynced`;
  3. `hard_link(tmp → name)` (28989);
  4. `seal_readable` (28994). The segment work publishes R readable and makes the directory
     entries durable at that landing, according to the comments at 29009-29013 and 38894-38896;
     I did not trace the code;
  5. `vault::persist` (the envelope, under a cipher today);
  6. the presync join;
  7. **`seg.sync_all()` (29004)**, which makes every committed record and the trailer durable;
  8. `ordf.sync_all()` if `ord_durable`.
- `hand_seal` (30997-31046) computes `wal_unsynced = frames_unsynced() || loose_wals non-empty`
  (31030). A buffered drain then waits for its slot's fdatasync.

Durable landing (`land_durable`, 37847-37893):

- `publish` (38885-38940) writes the manifest slot and syncs it (`manifest_write_slot`,
  11698-11702), with `covered_seq` (37770: the max over landed seal names).
- At a drain, `publish_unsynced` runs and the slot's sync is owed (37858-37867).
- Then `retire_covered` (37897-37928) removes the closed WAL files (or offers one as the spare)
  and **removes the temp name** (37925-37927). It does this only behind a durable slot: under
  `manifest_pending`, after the next publish (37876-37887); with a slot sync owed, after it
  (37885).

Order: seals land in the order handed. Only the oldest lands, and no manifest is written while a
seal is between its phases (38887-38890). So a durable manifest with covered ≥ e_R implies that
R's segment is durable.

### 1.5 sync, flush, close

- `Db::sync` (29616-29647) calls `commit_staged`, which for an ordered table is `commit_direct`
  and so syncs **only if due** (28646-28684). The WAL branch is skipped for an ordered table
  (29621). Then `unsynced = 0` (29626). See S2.
- `flush` (29759-29781) runs `commit_staged`, `unsynced = 0`, `draining`, and `seal()`. For an
  ordered table, `seal()` is `commit_direct` then `close_direct` (29100-29105). Then
  `maint_request(Drain)`. The run's records are therefore durable before `flush` returns:
  presync and `seg.sync_all`. The slot is durable before the return under a buffered policy
  with unsynced frames; otherwise its fdatasync follows.
- `close` (31353-31370) is `flush` followed by `settle`.

### 1.6 Recovery at open (27580-27937), in order

1. Manifest slots (27631). Under a cipher, the vault sweep (27635).
2. On-disk segments that the manifest does not name are removed (27688-27692).
3. Run temps (27717-27770):
   - A temp whose id is named live is removed.
   - Otherwise `recover_direct` (6948-7004) walks the records to the last marker whose CRC,
     length and count agree.
   - An empty result removes the temp.
   - Anything else is rewritten as a piece **named at `sealed`**, the manifest's covered:
     `write_ord(.., true)`, rename, `sync_dir`, `manifest_write_shape` with covered unchanged.
     Then the temp is removed.
4. Orphan ordered indexes and form files are swept, and segments are opened.
5. The WAL is replayed from `sealed`, file by file in id order (27882-27909), into one hashed
   memtable. A loose boundary ends the replay and **removes the later files inside the loop**
   (27899-27906). The memtable reads newer than every segment.
6. The live file is truncated to `valid_len` and synced (27930-27936). Then `open_append`
   (4324-4373) counts the replayed bytes as synced (4355-4358).

### 1.7 How run records are ordered against WAL records

Plain store:

- A run's records carry no WAL sequence. The run's segment is named at `end_seq` = s_open.
- After a crash the recovered piece is named at `sealed` ≤ s_open, and every replayed WAL frame
  reads newer than it.
- Frames written after the close are newer than the run, which is right.
- Frames written before the run's open, still unsealed at the crash, also read newer. That is
  right only while the run's keys are disjoint from them. Appends and puts raise `max_key`.
  Deletes do not (S1).

Cipher today:

- The run's batches are WAL frames with sequences in [s_open, s_close). The segment is named at
  s_close, so a durable landing covers them.
- The temp is a memory file, absent from the directory after a crash, so `recover_direct` never
  runs. The WAL replays the run in order with everything else: one log, prefix semantics.

### 1.8 Durability by policy

| | `Always` | `EveryN(n)` |
|---|---|---|
| run commit | at its `d.w.sync()` before `commit` returns (28800-28807) | at the n-th commit since the last barrier, counted across run and WAL commits; or the helper's background fdatasync (6215-6227); or writeback; or the presync at a drain's close; or `seg.sync_all` at the close (29004). **Not by `Db::sync`** (S2). |
| run close | records already durable. Segment durable at `seg.sync_all` (29004); its naming at the durable landing's slot sync; temp removed after that (37925) | the same. `seg.sync_all` is the first sync that covers the unsynced tail; a drain's presync comes earlier |
| WAL record after the close | at its own `sync_wal` (28510-28512) | at the next sync by policy (`sync_wal` 27055-27081: directory barrier, loose WAL files, chain, live file). **`sync_wal` never syncs the run's file.** Or by writeback at any time |

Can a WAL record written after the close become durable before the run's last commits?

- Under `Always`, no: every run commit was synced before the close.
- Under `EveryN`, yes, in two ways:
  - **by an explicit fdatasync**: the shared counter can make commit 3 the n-th, and
    `sync_wal` covers the WAL alone while the seal thread has not yet run `seg.sync_all`;
  - **by writeback**: the kernel may write the WAL's pages back before the run's.

A clean power loss then leaves the run cut short and commit 3 present, which is not a prefix of
the history. That holds for a plain store today (model `none` every2: 584 states) and for
Pagefile as designed. The cipher store today does not reach it.

### 1.9 Side findings (by reading; none reproduced)

- **S1, the recovery order.**
  1. `delete(k)` with k above `max_key`, then a commit, a seal (freeze) and `append(k, v)`. The
     append goes direct, because `max_key` was not raised.
  2. A crash before the frozen table's seal lands durably.
  3. At open the run is recovered as a piece at `sealed`, and the WAL replays the tombstone into
     the memtable, which reads newer. k reads absent; the store acknowledged `[v]`.

  This holds for a plain store today. Pagefile inherits it; the cipher store today does not.
  Fix: `delete` raises `max_key` (one compare). The run's keys are then strictly above every key
  that any earlier WAL record names.
- **S2.** Under `EveryN`, `Db::sync` with a run open makes neither the run's file nor (under a
  cipher today) its logged frames durable. That contradicts the doc at 29611-29615. The doc says
  the call promises what "the policy left unsynced"; here it resets `unsynced` and returns. Fix:
  sync the run's writer there. I found no test of `EveryN` with a run: the `EveryN` tests use
  `wal_arm()`, which sets `direct_ingest: false` (tests/db.rs:2289-2294, 2651, 2705).
- **S3.** The prefix gap of §1.8.
- **S4, outside the binding's scope.** `loose_wals` is empty at open (28138), while the live
  file's header may say LOOSE (4344, 4353). After a process crash, the first sync by policy
  rewrites the header as chained (4498-4506) without syncing the predecessor, whose tail may
  still be only in the page cache. A power loss after that makes the next open refuse with a
  sequence gap.
- **S5.** The tamper test's history uses `put` alone (tests/cipher.rs:332-339), and `put` never
  goes direct (28411-28421). So `a_changed_byte_is_refused_or_is_where_the_log_ends` holds no
  run today, and it cannot catch the regression that decision 1(a) accepts. A run-bearing
  history is needed: ascending appends, then a `put` of one of their keys, then more commits.

## 2. The binding

### 2.1 The quantity

The **cumulative record count at the run's last completed sync** is the `count` field of the
marker that the sync covered. `recover_direct` already returns that count as `recs.len()` (6970,
7002), and it is unique per marker (28691). The byte length at the marker would serve equally,
but the count needs no change to recovery.

- Writer side: a new field `Direct::synced`, set to `d.committed` right after `d.w.sync()?`
  returns Ok (28804-28806). It is a store on the sync path only.
- Under Pagefile it is set after the Log class's sync returns.
- It is a lower bound. The helper's syncs and the presync are not counted.
  - Optional: raise it from the helper's `s.synced` byte offset. That needs a queue of
    (marker end, count) since the last known sync. Not proposed.

### 2.2 The record

A new WAL frame kind, `WAL_RUN = 3`, is a record frame like PUT and DEL:

- crc word 0;
- covered by its batch's commit frame;
- fixed body: seq u64, kind u8, payload.

The payload:

| field | type | meaning |
|---|---|---|
| `run` | u64 | R's segment id, which names `direct-{R}.tmp` and every segment name R takes (`name_id`, 27269) |
| `end` | u64 | e_R, the sequence R's segment is named at |
| `synced` | u64 | §2.1 |
| `committed` | u64 | variant C only: `d.committed` at the close (== `table.len()`, asserted at 28918) |

Size, as its own batch: B's frame is 8 + 33 = 41 bytes (header, then seq, kind and three u64s), C's
8 + 41 = 49, and the commit frame behind either is 21 (FRAME_HEADER 8, 4065; a commit body of 13,
4637): 62 bytes a run for B, 70 for C. B could drop `end`, since the first frame's position gates
it (§2.3), for 54. Replay accepts the frame only if `crc == 0` and the body is exactly 33 or 41
bytes long, and hands the payload to `apply` with the kind.

The frame stands in its own batch because the record must not sit in `pending` past the close:

- `direct_can_open` requires an empty `pending` (28328), so a binding left there would stop the
  next ordered run from opening. A drain followed by an ordered load would fall back to the hash
  table.
- Other placements carry the record but fail the gating of §2.3: a field of a commit frame (whose
  body length is fixed at 13, 4637), or the file header at a rotation (a create and a directory
  barrier per run).

### 2.3 Composition: thread, counter, moment

On the writer's thread, inside `close_run`, which serves both `close_direct` and `hand_tail`:

1. Before `let end_seq = self.wal.seq` (28919): `wal.frame(WAL_RUN, ..)` with
   `run = d.id, synced = d.synced, committed = d.committed`, then `wal.mark_commit()`. These
   consume s_b and s_b + 1, so **e_R = s_b + 2**. Write `end = s_b + 2` into the payload. It is
   known before framing, since `wal.seq` is the next sequence.
2. Then name the segment, take `closed_wals`, and call `hand_seal` as today.
3. **After `hand_seal`**, call `wal.write()`, with no sync and no rotation. Any error goes to
   `pending_err`, as `rotate_wal_by_size` does.

Writing after `hand_seal` keeps `wal_unsynced` (31030) false where no user frame is unsynced.
Otherwise every buffered drain that closes a run would wait for its slot's fdatasync: the 0.4-0.9
ms regression that CLAUDE.md's "two counters that start apart" names. The drain does not need
the binding durable. If the binding is lost, no later WAL batch can be applied (§2.5).

Why the order is the gate:

- Every seal landing before R has `end_seq` ≤ s_open ≤ s_b (29186; the run blocks freezes).
- R's landing sets covered := e_R = s_b + 2.
- Seals after R land after R.
- So a durable manifest's covered > s_b holds exactly when R's segment is named durably. The
  replay filter (`s >= from`) then skips the binding.
- `manifest_pending`: a pending slot's covered includes R (37733-37737). The open either verifies
  R whole or takes the slot before (27650-27655). Both readings are consistent.

### 2.4 The check at open

Reorder the open as follows:

1. Replay the WAL **before** the run temps are touched. The replay callback collects
   `WAL_RUN` → `bindings[run] = last frame seen`: later frames for R replace earlier ones; the
   settle frame and C's second frame replace the first.
2. Defer the replay's removal of later files at a loose end (27902-27904) until the checks pass.
3. Check, then recover the temps.
4. Then truncate the live file (27930).

For each binding (R, e_R, s, c):

| condition | outcome |
|---|---|
| covered ≥ e_R | moot (only C's second frame and the settle frame reach this row; the first frame is filtered by `from`) |
| a live segment has id R | satisfied. This is the piece an earlier recovering open made, before its settle frame was durable |
| `direct-R.tmp` exists, with `rec = recover_direct(..).len()` | refuse if rec < s ("the run's file holds fewer commits than the log says were synced"). Under C, if s ≤ rec < c, the log **ends at R's first binding**: re-run the replay up to that batch, cut the file there, and remove later files as the loose rule does. Otherwise recover as today |
| no temp, no segment, s > 0 | refuse ("a run the log names is missing"). Under C with s = 0 and c > 0: end at the first binding, since an absent temp reads as empty |
| s = 0 (B) | nothing to check |

The check runs before the temp is removed for an empty result (27733-27735).

Specific cases:

- **The temp is absent because the manifest named R's segment.** Then covered ≥ e_R and the
  binding is moot. A crash between the slot and the temp's removal leaves the temp present and
  id R named, which the open removes as today (27728-27730).
- **Several runs.** Each run has its own id and is checked independently. Runs close in id
  order, and seals land in queue order.
- **The WAL file that carries the binding was rotated.** Closed files stay in the replay chain
  (27882) until a seal handed after the rotation retires them. That seal lands after R, so the
  binding is moot by then. The binding is written into the live file after `closed_wals` has
  been taken, with no rotation at that write. A binding inside a file that R's own landing
  retires would be moot at that retirement anyway.
- **The file was retired, or under `recycle_wal`.** Retirement comes only behind a durable slot
  that covers it (37883-37887). A recycled file is a new life with a new seed or key (4280-4319),
  and the old life's frames fail their checks.
- **A loose boundary.** If the binding is in a lost tail, the gap ends the log before the next
  file, so no later batch is applied (4651-4659).
- **A drain closes the run.** The answer comes after `seg.sync_all`, and `retire_covered` waits
  for the slot's sync (37885). So a torn newest slot leaves the binding live with the temp
  present and every committed record durable.
- **A crash between the close and the binding's write.** No binding exists, and the writer has
  made no later WAL batch, so the state is the pre-run state plus the run's prefix, a held state.
  If R landed durably first, covered = e_R, and the replay's `from` ignores the missing s_b and
  s_b + 1 (continuity is checked only from `from`, 4661).
- **A recovering open.**
  1. It recovers R into a piece (27747-27768) while R's binding is live.
  2. After the manifest that names the piece is durable, and after `open_append` (27937), it
     appends a settle frame `WAL_RUN(R, e_R, 0, 0)` with its commit frame.
  3. It calls `wal.sync()` before it returns.

  Without the settle frame, a merge of the piece before the next seal lands makes the next open
  refuse a healthy store (B0).

### 2.5 What makes the check never fire without damage

These five conditions together are sufficient. The model checked them over every interleaving
and crash point within its bounds.

- **s comes from a completed sync of the run's file.** In program order the binding's bytes
  reach the WAL file after that sync returned. So no durable copy of the binding exists before
  the run's file holds s records durably.
  - After a power loss, recovery reads at least the synced prefix. For Pagefile, §4's Log
    recovery says this; it is a property on paper, not verified.
  - After a process crash, recovery reads everything committed.
  - c = committed would fire on a clean power loss under `EveryN`, because writeback can land
    the binding before the run's tail.
- **The binding goes moot when R is named durably** (§2.3: framed before `end_seq`). Otherwise
  the retirement plus a merge produces "missing".
- **e_R is unique to R** (a sequence consumed at the close). Otherwise the previous seal's landing
  makes the binding moot early. That loses detection rather than refusing.
- **The settle frame at a recovering open.**
- **The run's file's directory entry survives a power loss once the file was fdatasynced.** The
  engine already assumes this when it acknowledges run commits (1.1). The binding turns a wrong
  assumption from a silent loss into a refusal. Not verified on this filesystem.

### 2.6 Variant C: the prefix under `EveryN`

C adds `committed` to the payload and the end-at-the-binding rule (§2.4). It also adds a chain,
modelled on `loose_wals`:

- `close_run` pushes `(R, e_R, presync-style file clone (28937), committed)` onto `loose_runs`
  when `synced < committed`.
- The first `sync_wal` after that (27055-27081) does three things in order: fdatasync R's file
  (0.03 ms if the seal thread already synced it); append `WAL_RUN(R, e_R, committed, committed)`;
  and only then the WAL's own fdatasync.

The cost of the rule: a loss within the unsynced band drops the WAL commits after the binding.
At most n - 1 commits are lost in all, since the shared counter fires within n commits. None of
those had been synced. The model found no synced WAL commit dropped.

## 3. Cost

| | cipher store (Pagefile) | plain store, if applied |
|---|---|---|
| per run commit | none written; one u64 store after a completed sync | the same |
| per run | one frame and commit, 62 B (C: 70 B), one write(2) on the writer's thread, no fsync. Under today's LogCipher, plus 32 B (record head 16, a length of 4 and a nonce of 12, plus a tag of 16; vault.rs:1768, crypt.rs:107-109). Under the layer's WAL (S6), one `TAIL` frame. C under `EveryN`: at most one fdatasync of the run's file, plus 70 B, at the first sync by policy after the close; none under `EveryN(u32::MAX)` | the same |
| per open | collecting bindings is one match arm in the existing replay; the run's temp is read by `recover_direct` anyway. A recovering open: one frame and one WAL fdatasync per open. C's end rule: a second replay pass, only where a run lost its unsynced tail | the same |
| format | no WAL magic move needed: Pagefile's marker version refuses the store in an older build (vault.rs:364, 387-389) | **the WAL magic must move** (below) |

Why the plain WAL's magic moves:

- An older reader meets kind 3 at `_ => break` (4706). It ends the log there and then truncates
  the live file to `valid_len` (27930-27935), silently dropping every commit after the binding.
  That is a misread, so the magic moves.
- Means: new kind bytes, for example 6 for chained and 7 for loose, so that `wal_kind` returns
  `None` in an older reader and it refuses with "not a supdb WAL" (4604-4608).
- Every writer of a header writes them: `begin` (4203), `recycle` (4280), `chain` (4504) and the
  spare's `prefill` path.
- An old-kind live file found at open is re-kinded or rotated before its first `WAL_RUN`.
- The new reader accepts 4, 5, 6 and 7.

What the store already records, and whether it could serve:

- **The value**: yes. It is the marker's `count` (6773), already returned by `recover_direct`.
- **The gate** ("is this binding still live"): yes, with no new field. The sequences, the
  manifest's covered and the name's `end_seq` do it, once the binding consumes sequences before
  `end_seq`.
- **The record itself**: no. Frames are PUT, DEL and COMMIT of fixed shapes. A PUT under a
  reserved key would be applied as a write by every reader, which is a misread.
- **Encoding the count as a deliberate sequence gap** (advance `wal.seq` by s at the close) needs
  no field. It fails:
  - it refuses clean `EveryN` power losses (or needs C's machinery anyway);
  - it collides with real gaps;
  - at a loose file start an older reader reads it as the log's end, silently;
  - it cannot express the settle.

  Rejected.
- **Today's cipher**: `log_run` is the binding at full price, the double write.

## 4. Not verified / open

- The model (`model.py`) abstracts:
  - one WAL file and one run;
  - no later seal covering the W commits;
  - no rotation, recycle, `manifest_pending`, drain or second run;
  - power loss as a per-file prefix between synced and written;
  - directory-entry loss only as damage.
- Pagefile's Log recovery returning at least the last synced state on a clean power loss is the
  design's §4 claim. Its E4 has not yet modelled `finish`, `TRUNC` or session bases.
- That the directory entries are made durable at the readable landing comes from comments
  (29009-29013, 38894-38896), not traced code.
- S1, S2 and S4 come from reading only. No test was run, and none written.
- The helper's background syncs could raise `synced`. Not designed.
- The binding's frame layout and the replay changes are a design, not code; nothing was built or
  run against the engine.
