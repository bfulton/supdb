# E0 items 5 and 6: where write errors and form-page read failures lead (by reading, tree 3df85d22)

I built nothing and timed nothing. Every claim below cites file:line in /home/user/supdb at 3df85d22. "Not verified" marks a claim I did not trace to its end, or one that rests on Linux behaviour I know but did not test on this kernel.

## Item 5: a failed WAL write or sync, a failed direct-run write or sync, a failed helper sync

### 5.1 WAL write (`Wal::write`, src/db.rs:4436-4456)

- `Wal.file` is a `std::fs::File` (src/db.rs:4079), and writes go through its cursor:
  - `Wal::create` uses `File::create` (src/db/vault.rs:1863-1870; the WAL name passes to the directory, vault.rs:562);
  - `Wal::recycle` uses `open_write` plus `seek(0)` (src/db.rs:4282-4283, 4302-4303);
  - `Wal::open_append` uses O_APPEND (vault.rs:1914-1931).

  Nothing writes at an explicit offset.
- The plain arm does `self.file.write_all(&self.pending)?` and only then `written += len` (4442-4443). The cipher arm seals `pending` at `at = self.written` and writes that (4449-4451). On an error, `pending.clear()` (4454) is skipped and `written` does not move. std's `write_all` keeps whatever bytes it wrote before the failing `write`, so the file cursor or EOF has moved by that partial count while `written` has not.
- The retry is made by the next writer call that writes: `Db::commit` (28495-28496), `commit_staged` (28664-28665), `seal` (29107-29108 or 29116-29117) or `log_run` (28872-28873). Each first calls `mark_commit()`, which appends a **second commit frame** because `pending` is not empty (4430-4434). Any new batch's frames are appended in between. The new commit frame's batch CRC is `crc32(&self.pending)` (4391-4395), which covers the first commit frame too.
- At replay, the first commit frame verifies and resets `batch_start` (4680-4683). The second one's batch CRC is checked over the bytes since the first only (4640-4643), so it fails and the walk `break`s. Every batch after it is gone too, because they all lie behind it in the file.
- **Plain finding.** After one failed `Wal::write`, every commit acknowledged later, synced or not, is silently missing at the next crash replay. This holds in the clear, today, whether or not the failed write left any bytes. The open's own truncation comment names this exact hazard ("its records would sit in front of the next batch's commit frame and be adopted by it", 27927-27935) and handles it only at open. A clean `close` or flush hides it: the memtable is sealed and the manifest covers the log. A partial prefix makes it worse: the retried bytes land after it, so replay stops at the cut frame or at C1, and loses the failed batch and everything after it.
- **The design's sentence (§3)** reads "`pending` is resent at the same offset". It is resent at the cursor or EOF, which is the same offset only if the failed attempt wrote 0 bytes. It is resent with an extra commit frame plus any new frames. Under the cipher the AAD carries `at = written` (4449; vault.rs:1791-1797) while the bytes may land at `written + partial`, so the record is refused anyway. The nonce is safe today, because `LogCipher::seal_into` takes a fresh counter nonce on every call (vault.rs:1801-1815, 1804).
- **What the caller sees.** `Db::commit` returns `Err` before `mem().commit()`, so the batch stays staged and invisible under `Latest`. Nothing latches: `pending_err` is only set by `writes_go`, `rotate_wal_by_size` and `leave_direct` (28447-28458, 26985-26992, 28366-28368). The next `commit()` makes both batches visible and acknowledges them, and replay will lose them as above.
- `Txn::commit` (31972-31981) and `put` (28406-28423) add no handling of their own.

### 5.2 WAL fdatasync (`Wal::sync` 4458-4462, `Db::sync_wal` 27055-27081)

- `sync_data()?` failing leaves `synced` where it was, and `sync_wal` returns `Err` before `unsynced = 0` (27077-27079).
- In `Db::commit` the order is write, `mem().commit()` (visible), `unsynced += 1`, then `sync_wal()?` (28495-28511). A failed sync therefore returns `Err` for a batch readers already see.
- The next commit that is due calls `sync_wal` again on the **same descriptor**. Linux reports a writeback error once per open file description (errseq, since 4.13; the engine's own comments at 6109-6113 and 29056-29060 say so). So that sync returns success, and the frames whose writeback failed are believed durable. Not verified on this kernel: whether ext4 drops or keeps those pages. Either way the next fdatasync will not report them.
- Loose closed files are reopened at every `sync_wal` (`vault::open(&p)`, 27066-27068), and a failed one stays at the head of `loose_wals` to be retried (27065-27075). A fresh description samples errseq at open, so the retry on a new description is also told nothing once the error has been seen. This is from kernel knowledge, not verified here.
- `wal_dir_barrier` failing is sticky and retried (27085-27091), but a directory fsync has the same errseq exposure.
- `chain()` writes one byte through a second handle (4498-4507). If it fails, `loose` stays set and `sync_wal` returns before `wal.sync()`.
- **Plain finding.** After a failed WAL fdatasync the engine goes on, and acknowledges later commits on a sync that cannot see the earlier failure. Nothing poisons. There is no failure-injection hook for a WAL write or sync anywhere in the tree. The hooks that exist are `fail_zbuild`, `fail_landing` and `fail_presync` (14621, 14794, 14797).

### 5.3 Direct run commit (`commit_direct`, src/db.rs:28690-28839)

- **Streamed path (the default).**
  - `put_encoded` runs per entry (28750-28754, 6792-6849). It is bookkeeping only (`pos`, `recs_len`, `deferred` count, `last_key`), with no I/O.
  - Then come `stream_out` (28756, 6855-6859), which writes the buffer via `write_buf_with` (5652-5654), and `mark_stream` (28757, 6870-6895). `mark_stream` reserves the marker in the **table's arena** (`src.reserve_contig`, 6887), hashes it, calls `marked()` (6890; `pos` and `recs_len` advance, 6779-6784), and only then `stream_flush(false)?` (6893).
  - `stream_flush` writes with `writev_all` at the cursor (5742) and sets `written = stop` only after success (5743).
  - `write_buf_with` writes before `written += total` (6030) and `buf.clear()` (6034).
  - `impl Write for AlignedWriter` buffers `take` bytes and then returns `Err` from `write_buf()?` (6311-6315). The bytes were taken but `Err` is returned, so a caller's `write_all` retry duplicates them in the buffer.
- **On an I/O error mid-commit**, `commit_direct` returns `Err` before `d.stream_from`, `d.committed` and `mem.commit()` (28758, 28797-28798). The next commit loops from `d.committed` again and calls `put_encoded` on keys already counted. With deferred keys, the `below_last` check (6805-6816) refuses with "keys must arrive in strictly increasing order". **The run is wedged**: every later commit, seal (29100-29104) and flush (28654-28655) of the run returns that error.
- `leave_direct` truncates the table to `d.committed` (28365) and closes the run while the writer's deferred count, `recs_len`, held stream and marker are ahead of the table. What the finish then writes is **not verified**.
- If a partial write preceded the error, the file cursor is `written + partial`. Every later offset the writer computes (`stream_base` 5660-5664, `last_value_at`, `read_section`'s `read_exact_at` 5857, the finish's patches) assumes the file offset equals `written`.
- **Sync.** `d.committed = mem.len()` and `mem.commit()` run **before** `d.w.sync()?` (28797-28806). A failed sync returns `Err` for a batch readers already see.
  - A retried `commit()` with nothing new hits the early return at 28691-28693 and returns `Ok` **without issuing any sync**.
  - `Db::sync()` goes to `commit_staged`, then `commit_direct`, and gets the same early `Ok`. It then skips the WAL branch because the table is ordered (29621-29622) and returns `Ok`.
  - With `adaptive_shape`, `sync()` then hands the run to `close_run` (29639-29645). The trailer's `seg.sync_all()` (29004) is on the same description, so errseq reports success, the run lands durable and the manifest names it.
  - **Plain finding:** a failed run sync is followed by `sync()` returning `Ok` and the store believing the run durable.
- **`SegmentWriter::sync`** (6917-6937) writes held records via `stream_flush(true)` and `flush`, then calls `sync_data`, then `synced_all` (5935-5942).
- **Under today's cipher**, `log_run` runs after the run's memfd "sync" and after `mem.commit()`. It sets `d.logged = mem.len()` before appending the frames (28863-28865), then `wal.write()?` (28873).
  - If the WAL write fails, the batch is visible, `d.logged` has moved, and the frames sit in `wal.pending`. The next `log_run` appends the next batch behind them, and the double-commit-frame loss of 5.1 follows.
  - A retried `commit()` with nothing new returns early (28691). `Db::sync()` returns `Ok` without touching the WAL (29621).
  - **Cipher finding:** a failed log write of a run commit is acknowledged by the next `sync()`, with the batch durable nowhere.

### 5.4 Segment writer's helper (`WriterHelper::run`, src/db.rs:6182-6241)

- The helper's `dup.sync_data()` error is stored as its first code (6218-6226). `s.synced.fetch_max(landed)` advances **anyway** (6227), and the error reaches the writer only at `finish` (6266-6271), which is the run's or the seal's close.
- `dup` is `try_clone`, so it shares the writer's open file description (6156). After the helper consumed the error, the writer's own `sync_data` for the direct run's commits (6934) reports success.
- **Plain finding:** a direct run longer than `sync_ahead` (8 MiB default, 7992) can acknowledge commits whose writeback failed, until the run closes.
- At a seal, the helper error makes the finish fail before the names are signalled. The seal is then not landed, and `seal_wedged` is set (37660-37671), which stops every seal behind it.
- `start_writeback` ignores its result (6071-6088). With `SYNC_FILE_RANGE_WRITE` alone it does not wait, so it is believed not to consume errseq (not verified on 6.18).

### 5.5 Adjacent paths found on the way (same shape, not asked)

- **Presync, non-inline arm** (29061-29071). The flush returns the presync error (29777-29779). But the seal job's trailer `seg.sync_all()` (29004) on the same description succeeds, the landing names the segment and the WAL retires, all before `flush` returns `Err`. So the store believes durable a run whose records' sync failed. The `fail_presync` hook exists (29062) and could hold a test of this.
- **Seal thread failing at its fsyncs after the readable landing** (37657-37672): the segments "stay live with their sequence covered, so the next landing's manifest names them". A manifest then names a segment whose fsync failed, and covers the log frames for it. Read from the code and its comment; the next landing was not traced.
- **Form builder.** `FileSink::put` (3852-3863) propagates a write error, so the build fails. `drop_written_range` uses `WAIT_BEFORE|WRITE|WAIT_AFTER` with its result ignored (3926-3942), which consumes errseq. The build's later `sync_data` is on a new descriptor and its result is ignored too (4378-4381). A form file is regenerable and page-CRC'd, so this is the form's alone, as the design wants.

### 5.6 What the test of a failed write must cover (for the design and for S1/S4/S6)

- **T1.** WAL write fails having written 0 bytes. Then commit and sync more, crash, replay. Every acknowledged commit after the failure must replay. **This is red on plain today** (5.1), so it cannot be a parity test of the layer: the engine needs a fix first. Either latch a failed WAL write as a sticky writer error (a `pending_err`-like latch or poison), or rewind (`set_len(written)`, seek to `written`, and drop the trailing commit frame so `mark_commit` does not add a second one). The latch is the safer of the two, given T4.
- **T2.** The same as T1 with a short write before the error, at each byte count up to a frame. Under `Pg` the Handle owns a Log file's offset (`append`, design §2). The Handle must leave a failed write call with no effect on its logical state (len, tail page, ring head, open group's tags, override table), or poison itself.

  If the Handle did advance len over a failed call, the engine's resend would put the batch's frames and C1 into the logical file **twice**. Replay would verify the second copy, whose batch CRC is over that copy alone (4640-4643, 4680-4683), and then meet seqs below `next_seq`, giving `Err("wal sequence gap: a durable record is missing")` (4661-4667). The store would then **refuse to open**. This is derived by reading, not run.

  A Log write is up to three syscalls: the data `pwritev`, the tag-page `pwrite` and the frame `pwrite` (design §4). Inject at each, at a short count of each, and with the data written and the frame not. ctr must still advance on every attempt (design §3). The retry must not reuse the failed attempt's (p, ctr), and a home page torn by the failed write must not be counted home.
- **T3.** Under the cipher today: `log_run`'s WAL write fails, then `sync()`. It must not be `Ok`; today it is (5.3).
- **T4.** fdatasync fails on a Log Handle, then any later sync of it, from any clone or thread, must fail.

  The design's one Handle per file_id is shared by `chain`'s handle (4502), the helper's dup (6156), the presync clone (28937-28939) and the WAL opened twice at open (27932, 27937). So the Handle is where to latch. Its sequencer must not count a failed sync as completed, and must keep that sync's ring span pinned, since what reached the device is unknown.

  At engine level: after an injected sync failure, `commit()` and `sync()` must not acknowledge. Today they do, for the WAL (27077, then the next commit's sync) and for the run (28691, 29621).
- **T5.** Direct run: inject at `stream_flush` (5742), at `write_buf_with` (6018, 6026) and at the run's `sync_data` (6934). Assert either a clean retry or a refused store, and never a run closed with the writer's count, held stream or marker ahead of the table (5.3).
- **T6.** Helper sync failure (6218), then the run's next commit sync. Under `Pg` the latch of T4 makes that commit fail; today it is `Ok` (5.4).
- **T7.** Presync failure, non-inline arm (`fail_presync`, 29062): the manifest must not name the run's segment, or the store must be refused (5.5).
- **T8.** The seal's fsync fails after the readable landing (37660-37667). Under `Pg` this is a Once file a manifest names without a durable `FINAL`. E4 should model it.
- **E4 mutants to add:**
  - "a sync after a failed sync reports success";
  - "a failed write call advances the Handle's len";
  - "a retried batch carries a second commit frame".

## Item 6: every site that reads a compact-form page, and what a failed read does

### 6.1 The primitive, and what "a failed read" is

- `ZFile::page_bytes(p)` is `self.map.slice_at(p*ZPAGE, ZPAGE)` (src/db/zfile.rs:2141-2148). It never fails itself.
  - **Plain:** `None` only out of bounds. Damaged bytes are lent, and the checks (`first_decode`, `first_checks`, `page_view`, `check_entry`, `check_group`) refuse them.
  - **Under a cipher today:** `slice_at` goes to `slice_slow` and `Paged::ensure` (pager.rs:1312-1339), then to `slow` and `fill`. A fill error (failed tag, missing envelope, short envelope; pager.rs:966-986) sets the chunk back to EMPTY and calls **`self.pager.poison(..)`**, which is store-wide (pager.rs:1100-1103, 426-432), and returns false. `slice_at` answers `None`.
- `ZFile::fail(p)` (zfile.rs:2151-2157) sets the page's bad bit and `damaged`. From then on every entry point answers Out or Fallback, gated at its start: 2422, 2464, 2677, 2925, 2975, 3101, 463, 674 and 1036.
- The advice paths `Paging::resident`, `forget` and `willneed` (pager.rs:1380-1500) decrypt nothing and have no failure path.

### 6.2 Read sites in src/db/zfile.rs (each handles None and check failures as noted)

**Attach** (`ZFile::attach`, 1982-2100). It runs under `vault::hold` (2003) and reads three things:
- the trailer, `map.slice_at` (2028-2032): `None` becomes `Reject::Damage`;
- the resident part (2051-2053): `None` becomes `Reject::Damage`;
- `check_sample`, 16 pages (2098, 2109-2139): `page_bytes` `None` becomes `ZError::Length`, then `Reject::Damage`.

The callers act on the `Reject` (db.rs):
- `z_attach` (31482-31489) counts it and sets `z_declined = DECLINED_REFUSED`, so a rebuild follows.
- `z_give` after a build (31586-31591) removes the file and records a failed build.
- The collect sites:
  - a spare (41212) is marked `spare_failed`;
  - a recode (41274-41280) removes the new file and keeps the old one;
  - a plain build (41325-41330) removes the file and records a failed build.
- **Plain:** the form is the form's alone. **Cipher today:** the fill already poisoned the store. At open, under `ZPages::Always` (27821-27847), the open's `p.check()?` (28171) **refuses the open**, and tests/cipher.rs:1972-1976 accepts that outcome.

**The other read sites:**

| function | lines | page reads | on `None` or a check failure |
|---|---|---|---|
| `open_group` | 2178-2338 | `page_bytes` (2188) | `fail` + `None` (2188-2191). A bad index, a view error or no group: `fail` (2193-2196, 2218-2221, 2224-2227). `first_checks` or `first_decode` failing a check: `fail` (2239-2241, 2293-2296, 2333-2336). `Fail::Misaligned` returns `None` **without** `fail` (2238, 2292, 2332): a source that cannot lend aligned, which is no damage and never happens with a mapping or the memfd. |
| `view_of` | 2362-2371 | `page_bytes` | `fail` |
| `group_of_index` | 2376-2385 | `page_bytes` | `fail` |
| `group_of_key` | 2392-2401 | `page_bytes` | `fail` |
| `locate_rank` | 2421-2459 | via `view_of` and `open_group` | `Fallback`; a walk to the record failing: `fail` (2447-2449) |
| `locate` | 2463-2578 | `view_of` (2497), `page_bytes` for `first_checks` (2510-2513), payload, `record_group_seek`, `decode_record`, `group_seek` | each `fail` + `Fallback` (2510-2553, 2562-2567) |
| `read_all`, `count` | 2583-2615, 2618-2643 | via `locate` | a parse failure: `fail` + `Fallback` (2605-2610, 2635-2638) |
| `scan_at` / `scan_in` | 2661-2847 | `view_of` (2743), `open_group` (2753), coded payload (2773-2781), `coded_record_to` (2787-2791), `record_key_next` and `parse_record` (2825-2837) | each `fail` + `ScanEnd::Fallback`, having emitted exactly `out.seen` records; raw-form errors propagate (`?`, 2730, 2787, 2838) |
| `values_at`, `key_at_into` | 2877-2909, 2924-2964 | via `locate_rank` | `fail` + `Fallback`; the no-page single-record key comes from the heap (2925-2935) |
| `seek` | 2974-3077 | `view_of` (3007). The coded arm's `page_bytes` (3021-3024) falls through to `open_group` on `None`, which asks the page again and then fails. | `Fallback`; check failures `fail` (3030-3033, 3047-3050, 3068-3071) |
| `owner_span_from` | 3100-3126 | `page_bytes` (3114) | `fail` + `None`. `entry(..)?` and `group(j)?` (3108, 3119) return `None` without `fail`; they are heap and view lookups, and the caller then resolves exactly. |
| `cut_in` | 3133-3158 | via `seek` | as `seek` |
| `ZCursor::seek` | 457-517 | `group_of_key` (483), `open_at` (486), the seek over the group (496-507) | `Err(Fallback)` or `fail` |
| `ZCursor::open_at` | 551-617 | `page_bytes` (571-574) with no `fail`; `open_group` asks again and fails | `fall()` (583, 610-613) |
| `place`, `load`, `values`, `walk_impl`, `skip_to`, `advance_until` | 637-1219 | the held group's bytes | every parse failure: `fail` + `fall()`, and the rest of the walk is the raw form's from the failing rank (no repeat, no skip; 647-650, 655-658, 704-707, 765-766, 776-777, 924-927, 973-979, 1161-1164, 1201-1204). `coded_record_to` (305-337) calls `f` only after a whole parse, so `Ok(false)` has emitted nothing. |

**Advice and housekeeping, with no failure path:** `ask_ahead` (2857-2872, `advise_willneed`), `drop_cache` (3255-3260), `prefetch_all` (3266-3268), `swap_ahead` (3957-3989; its `File::open(form)` fails under a cipher and returns 0) and `warm_file` (3778-3812).

`warm_file` touches `map.as_slice()[at]` on plain. Under a cipher `as_slice().len()` is 0, so it reads nothing. Under the design's S7 WARM (`lend = len`) it would read the memfd directly, without `ensure`: not verified, and worth a look at S7.

**The memo** (1542-1830) holds decoded groups in the heap, keyed by the attach id and page. A hit reads no page (2198-2209, 2262-2272). **Verify** is `first_decode`, `first_checks` and `verified_group_in` over bytes `page_bytes` lent; there is no separate read.

**Not form readers:**
- the sampler (`zfile::sample`, 5052-5087, on supdb-zsample, db.rs:39661-39699), which reads the raw segment only;
- the build (`build_file_seen`, 4242-4391, on supdb-zbuild, db.rs:41036), which reads raw and writes the form file;
- the compaction merges (db.rs:11960-13100 has no zfile or `.z` use).

### 6.3 Dispatch sites in src/db.rs

Every one takes `Fallback`, `Out`, `NoPage` or `None` to the raw form and never answers from a failed form:
- `part_read_z` 19661-19700, `part_count_z` 19704-19740;
- `part_scan_z` 19750-19794, which continues raw from `rank + seen` (19787, 19806-19815);
- `part_seek_z` 19822-19845, `part_cut_z` 19850-19872, `part_values_z` 19879-19912;
- `zc` 19920-19925, which filters `damaged`;
- `cache_scan`, `cache_values` and `cache_cut` 19933-19979;
- `cache_seek_exact` 20001-20021, `scan_seek` 20028-20046, `lazy_block` 20064-20078, `cache_owner_of` 20080-20084;
- `block_bounds` and `cache_key_at` 20094-20145;
- `pwalk_at` and `pwalk_seek` 20149-20187;
- `PartReader` 36181-36340, which closes and turns off on `fell_back`;
- the overlay walk 26364-26379.

**By reading, no site swallows a failed form read and answers wrong.**

### 6.4 Threads that read form pages on behalf of another operation

These go through `Reader::zc` with `Options::zpages_block_cache`, or through `PartReader` and `ZCursor`.

- **The upkeep thread** (supdb-upkeep, 33576; `keep_upkeep` 33311-33414; `upkeep_pass` 33419 under an `Op` guard, which makes no pager check). It files the writer's commits and builds forms for later scans:
  - `settle_each` 24119 (`lazy_block` 24258, `cache_owner_of` 24266);
  - `fold_piece_block` 24857 (`cache_cut` 24898, `cache_values` 24927);
  - `make_table` 24969 (24995, 25008);
  - `PartReader` 24595 and 35358-36068.

  A pass's error goes to the writer's next commit once (`put_back`, 33406; `take_err`, 28537).
- **The builder ahead** (unnamed thread, 30821-30831), which builds forms for the next scan to install. Its result is **discarded** (`let _ = r.build_ahead_job(..)`, 30828).
- **The segment-work thread** (supdb-maint): the attaches at 40856, 41181, 41228 and 41299, and the controller's buy.
- **The writer itself:** its inline fill or settle (`maintain_forms`, `Hand::File`/`Settle`), `install_ahead` (21914; 22018, 22023), `rebase_tables` (21575), and `zpages_settle`'s `z_give` (31506-31612) inline.
- **The open:** the attach under `Always` (27821-27847).

What a failure does on those threads:
- **Plain:** `ZFile::fail` sets a flag shared by every thread (the cell's `ZFile`), and the thread answers from raw. It is reported to nobody but the per-handle counter (`zfallbacks`). The segment work then retires and rebuilds the form, up to `ZREBUILDS` (31731-31743; `z_start` 40876-40881).
- **Cipher today:** the fill poisons the store, and the poison surfaces where the pager is next checked:
  - the next **user read on any handle**, at its end (`Entered::finish` 16351-16361, `end_slow` 18134-18143, `p.check().and(r)`), even though that read's answer came from raw and was right;
  - the next **merge**, whose `vault::check` (12865, 13033; vault.rs:2217-2222) refuses its outputs, so that error reaches the writer's next commit;
  - **at open**, as the open's check (28171).
- The writer's commits never check the pager.

### 6.5 What has to change for "a form file's failure is the form's alone" (design §4 Trust, decision on Regen-lazy)

- **(a) pager.rs:1100-1103.** A fill error on a Handle of class Regen-lazy must return false without calling `Pager::poison`. It may record the failure on the file, so that repeated asks of the bad page do not redo the miss. Reads gate on `damaged` only at their start (6.1), and `seek` and `open_at` ask a failed page twice (3021-3024 then 2188; 571-574 then 2188). Key that record by the Handle or file_id, not by name: a rebuild renames a new file over the old name.
- **(b) pager.rs:426-441 (`poison`, `check`, `failed`).** The store-level check reads only failures of trusted files. With that in place, these callers need no change, and they still catch raw-segment failures:
  - db.rs:18140 (`end_slow`);
  - db.rs:12865 and 13033 (merges, through vault.rs:2217-2222);
  - db.rs:27563 and 28171 (open and create).
- **(c) zfile.rs read sites: no change needed.** Every `None` and every check failure already becomes `fail` plus raw (6.2). Two optional changes:
  - make `seek`'s coded arm (3021) and `ZCursor::open_at` (571-574) call `fail` on a `None` page directly, rather than asking again through `open_group`;
  - `Fail::Misaligned` stays a refusal without `fail`, and is unreachable under `Pg`.
- **(d) The attach at open (db.rs:27821-27847).** With (a) and (b) the open no longer refuses: the `Reject::Damage` path (31482-31489) marks the partition `DECLINED_REFUSED` for a rebuild. tests/cipher.rs:1941-2000 accepts the refusal today, and must be rewritten to require the store to open with the form out of service (design §10 already says this).
- **(e) Device errors.** A miss copies from a mapping today (pager.rs:1003-1004, `copy_from_slice` from `back.map`), and does so in the design's chosen shape (§4, read and miss). An EIO there is SIGBUS for a form page as for any page, so "the form's alone" covers tag failures only. E2's `pread` arm (iii) would make an EIO an `Err`, then `None`, then `fail`.
- **(f) Tests.** The `None`-page arm is held only for `count`: zfile.rs:8566-8571 uses `skewed = Some((Vec::new(), 0))`, and only `count` is called under it. No unit test drives a `None` page through any of the following, and S3 should add one per site:
  - `read_all`, `values_at`, `key_at_into`;
  - `scan_at`, `seek`, `cut_in`, `owner_span_from`;
  - `ZCursor::{at, seek, skip_to, walk_until, walk_run, values}`;
  - the attach's trailer and resident reads.

  I checked the zfile.rs unit tests only, and did not search the integration tests.
- **(g) No new site was found that would need the design to change the dispatch code.** That answers the open item "whether helper threads read Regen-lazy pages on behalf of another operation and drop the failure" (design §12):
  - They do read: the upkeep thread, the builder ahead, the segment work and the writer's fill.
  - They drop the failure only in the sense that nobody is told. The shared `damaged` flag carries it to every thread, and the segment work rebuilds.
  - The supdb-helper, presync, sync-helper (36681), trim, zsample and zbuild threads read no form page.
  - The run keeper (supdb-keeper, 34151) is not verified; it builds snapshots of the memtable.
