# Pagefile: the experiments owed, what they found, and what they change

Tree `3df85d22`, 2026-11-16. This file collects the experiments that `design.md` §9 owed before building. Each experiment was checked by a skeptic. Where the skeptic was right, its correction is the figure given here; where the concluder disagrees, that is said.

`design.md` is not edited. The changes it needs are listed per experiment and again at the end. Work directories are under `scratchpad/seam/work/<label>/`, and every figure below can be traced to the files named there.

**What the concluder checked against the tree.** These five checks were made by reading, at the lines cited:
- **TRUNC is reachable.** At a loose boundary, `Wal::replay` returns a valid length of `WAL_MAGIC.len()` (src/db.rs:4651-4659). The open cuts the live file to that length (src/db.rs:27876-27880, 27930-27934).
- **The drain's owed slot.** `publish()` writes the next slot with an fdatasync and drops the owed slot without syncing it (src/db.rs:38851-38870, 38885-38914).
- **The coding is sized against the whole budget.** The coding pick prices against `z_budget()`, which is `zfile::read_budget().budget` (src/db.rs:39798-39810, 40574-40580). The cipher's plaintext cache defaults to half of that budget (src/db/vault.rs:637-646).
- **`bench ab` sums the two stores' counters.** It takes the first store's counters, then adds the second store's (bench/src/run.rs:2611, 2653-2655). The skeptic cited 2642-2647; the lines are 2653-2655.
- **The mount is discard, with no journal.** `/proc/fs/ext4/vda/options` lists `dioread_nolock`, `discard` and `delalloc`, and E5 read `journal_task <none>`.

E3 (callgrind of the plain cost, and WARM's headroom) and E6 (tag residency) were **not run**. Nothing below rests on them.

## Summary

| exp | question | kill | verdict |
|---|---|---|---|
| E0 items 2, 4 | dir fsync before naming; may a `.tmp` survive an open | a `.tmp` that must survive drops the sweep for its prefix | **kill not fired**: only `direct-*.tmp` is read at open. Two plain durability windows found (one shared with E4) |
| E0 items 5, 6 | failed WAL/run/helper writes and syncs; every form-page read site | none defined | **mixed**: item 6 supports §4's Regen-lazy row. Item 5 found the plain engine losing acknowledged commits after one failed WAL write, observed by fault injection |
| E1 | is the form ahead of raw out of core, in the clear, forced | form not ahead | **kill not fired**: 2.57-3.19x (640 MiB), 6.6-7.9x (448), 2.6-2.7x (384) |
| E1b | is today's form loss under a cipher the straddle | faults not ~2x form-page misses | **kill as worded fired** (1.1-1.25 faults per miss). The mechanism is confirmed (two chunks per page). "The straddle is the whole gap" is **withdrawn**: the coding's file size is a co-equal cause |
| E2 | the cost of one miss | (ii) or (iv) above 3.4 µs | **(iv) killed** everywhere. **(ii) at or above the line** by mean (4 of 5 cells), straddling it by median. The relative gain depends on a regime the engine has not been counted in |
| E4 | do §4's unmodelled protocols survive the crash model | an acknowledged loss the model cannot close halts S4 | **kill not fired**, but six protocols failed as written; each closed by a change re-run green. TRUNC is reachable, so it must work rather than be refused |
| E5 | the device cost of a Log commit | A well above B at small commits | **kill not fired** (A/B 0.69x at 100 B). §8's saving of 20-50 ms on commits **does not exist**: at the load's commit the design's commit is level with today's |
| E7 | the cipher's cost to a store larger than its bound | none defined | **mixed**: 0.28-0.33x of the clear store. About 69% of the gap is device faults from a bound split three ways. The design leaves that term where it is |

---

## E0, items 2 and 4: directory fsync before naming, and the `.tmp` sweep

Work directory: `work/e0-durability/`. Skeptic: stands.

**Question.**
- Item 2: does every create that a manifest later names get a `sync_dir` before the slot that names it is durable? On every path, both manifest shapes, both policies.
- Item 4: does any `*.tmp` other than `direct-*.tmp` have to survive an open?

**What was run.** An audit by reading at `3df85d22`: src/db.rs, src/db/zfile.rs, src/db/vault.rs, and the tests that hold temp names. Plus `df -T`, and greps for `.tmp`, `sync_dir`, `open_dir`, `hard_link` and `rename`. Nothing was built or run.

**Found (item 2).** A manifest names only segments. The ordered index and the form file are named by no manifest and are regenerated. In the default shape, every named create gets its directory fsync before the slot that names it:
- seal and run output: `land_readable`'s `sync_dir` at db.rs:37798;
- merges: db.rs:12867 and 13035, inside the job;
- promotion: db.rs:38830, before 38833;
- the open's direct recovery: db.rs:27765, before 27768.

The two non-default shapes differ:
- `manifest_pending` syncs the slot *before* the directory (db.rs:36684). Only `pending_whole` covers it (db.rs:28309-28325).
- The rename shape issues one `sync_dir` after both renames (db.rs:11795-11796). It is refused under `Pg`.

**Found (item 4).** Only `direct-*.tmp` is read at open (db.rs:27717-27770).
- `seal-`, `compact-` and `tier-*.tmp` and `manifest.tmp` are left by the plain open. Nothing reads them.
- `ord-*.oidx.tmp` and `zpg-*.zpg.tmp` are **already swept** by the plain open (db.rs:27779-27785, 27794-27798; zfile.rs `z_sweep`).
- Their ids are formatted `{id:08}`, which is at least eight digits, not exactly eight.

**Plain defects found by reading.** The tests' crash is a drop, so they cannot see any of these.
1. Under `manifest_pending`, the open rewrites pending names as durable with no fsync of the segments or the directory (db.rs:27701-27709). The close does the same and discards the seal threads' results (db.rs:37386-37402).
2. A failed `sync_dir` at db.rs:37798 comes after the publish and after `covered_seq` has moved (37767-37771). A later merge landing can then name the seal's segments.
3. `direct-*.tmp` is acknowledged by its own fdatasync with no directory barrier (db.rs:28699, 28800-28807). The WAL has one (db.rs:27035-27046).
4. `Db::sync()` with an ordered table under `EveryN(n>1)` can return with the run unsynced (db.rs:28800-28807, 29615-29626).

The skeptic added two. The concluder confirmed (A) by reading.
- **(A) The drain's owed slot.** In the default shape under a durable policy, a drain's seal landing writes slot N unsynced (db.rs:37684, 38851-38870). A later publish in the same drain overwrites slot N-1 and drops the owed N unsynced (38909). This happens on merge landings (38000, 41720), promotions (38017-38024, 38488), and a second deferred seal (38869). If N+1 then tears, the newest valid slot is N-2, after N-1 may have retired WAL files and unlinked names.
- **(B)** A failed data fsync after the readable landing (29527, 29004) does not wedge the queue (37669). The next manifest names the segments (comment at 37661-37664).

E4 found (A) independently, as its F6.

**Verdict against the kill.** The kill does not fire. The temp sweep can go ahead, by exact pattern.

**Changes to the design.**
- **§4 "Temps a crash leaks".** The index's and the form's temps are already swept by the plain open. The plain leak is `seal-`, `compact-` and `tier-*.tmp` plus `manifest.tmp`. In plain mode the sweep matches the engine's exact patterns (prefix, at least eight digits, `.tmp`; and `manifest.tmp`), never a bare suffix, since a plain directory may hold a caller's `foo.tmp`. Under `Pg`, a logical-name suffix rule touches only this store's own files, so it is safe. Sweep before any engine thread starts. Except `supdb.cipher.tmp` in plain mode.
- **§4 Open, step 4.** Under `manifest_pending`, a missing or failed entry for a *pending* name reads as "not whole", which is today's fallback to the slot before. It must not refuse the open. Alternatives: refuse `manifest_pending` under `Pg`, or reverse the helper's order to `sync_dir`, then slot.
- **§5, manifest row.** It inherits defect (A). "The other slot is the store" holds only once the owed slot is synced before the next publish (see E4 F6).
- **§4 Log, `direct-*.tmp`.** The run temp's first sync carries the directory barrier the WAL already has, or the design says why not (defect 3). The same holds for any Log file created under `Pg`.
- **§12 open.** Close "the `.tmp` sweep pending E0 item 4". Add defects 1-4, (A) and (B) as plain-engine fixes (see the stage S0 proposed below).

---

## E0, items 5 and 6: failed writes and syncs; every form-page read site

Work directories: `work/e0-failures/` (reading), and the skeptic's LD_PRELOAD shim at `work/e0-failures-skeptic/shim/shim.c` with the probe crate beside it. Skeptic: stands, with observed corrections.

**Question.**
- Item 5: after a failed WAL write or fdatasync, a failed direct-run flush or sync, or a failed helper sync, does the engine retry, poison, or carry on? What must a test of a failed write cover?
- Item 6: does every site that reads a form page treat `page_bytes → None` as `ZFile::fail`, and which threads read form pages on another operation's behalf?

**What was run.**
- Reading: db.rs's WAL (4078-4712), its callers, `AlignedWriter` and `SegmentWriter` (5471-6370), `WriterHelper`, `close_run`, the seal landings, and every `page_bytes`, `view_of`, `open_group`, `group_of_*` and mapping use in zfile.rs, together with its db.rs dispatch. Pager failure paths: pager.rs:420-445, 960-1110, 1300-1510.
- The skeptic then injected `write`/`writev`/`fdatasync` failures with an LD_PRELOAD shim, at zero bytes and as a short count, into a plain and a toy-cipher store, and crashed with the page cache kept.

**Found (item 5): observed by injection, plain unless stated.**

| failure | what happens | first line cited |
|---|---|---|
| WAL write, 0 bytes | `pending` is kept. The next commit adds a **second commit frame**, whose batch CRC also covers the first frame (db.rs:4391-4395, 4430-4434). Replay breaks there (4641). After a crash, the failed batch k2 is recovered, and **every later acknowledged commit in that file is lost** (k3, k4, and the `sync()` after them). After the file rotates and before a seal retires it, the open **refuses**: "wal sequence gap" (N = 250, 265, 280). After retirement nothing is lost (N = 1500). | db.rs:4436-4456 |
| WAL write, partial | The cursor is past `written`. The failed batch and everything after it are lost. | db.rs:4436-4456 |
| WAL write, under the cipher | Replay applies the recovered batch, but `Wal::replay` maps the valid length back to the end of the record before it (db.rs:4573-4579, vault.rs:1824-1857). Reopen 1 is OK. One more commit, then reopen 2 refuses: "wal sequence gap". | db.rs:27927-27935 |
| WAL fdatasync | The batch is visible. The next `commit()` or `sync()` re-issues fdatasync and returns Ok. Whether data is lost is the kernel's errseq behaviour, which was not verified. | db.rs:28497, 4458-4461 |
| run, small batch, in the sync arm | No wedge. A next commit with new keys rewrites the held bytes. But a `commit()` or `sync()` with nothing new returns Ok without writing (28691), and a crash then loses the batch. | db.rs:28797-28806 |
| run, piece flush of ≥1 MiB | The run is wedged. Every later commit gets "keys must arrive in strictly increasing order". | db.rs:6805-6816 |
| run, partial write | The file is shifted. Later acknowledged commits are lost. `flush` fails with "the block table is not readable" and leaves a `par-` file. | db.rs:5741-5743, 6026-6031 |
| helper fdatasync | `synced` advances after a failure (6218-6227). The dup shares the description, so the writer's next sync reports Ok (6934). By reading. | db.rs:6156 |
| presync | The non-inline arm keeps the error while the trailer's `sync_all` succeeds (29004, 29055-29071). By reading. | |
| seal-thread error after the readable landing | Not latched; `seal_wedged` is set only if `!landed` (37669). | |
| `log_run` under the cipher | The frames stay in `wal.pending` and are resent with the doubled frame. They are retried, not dropped; the original result was wrong here. | db.rs:28867-28873 |

**Found (item 6): by reading, the skeptic concurring.**
- Every zfile read site routes a `None` page, and every failed check, to `ZFile::fail` and then the raw form: attach and its sample, `open_group`, `view_of`, `group_of_*`, `locate*`, `read_all`, `count`, `values_at`, `key_at_into`, `scan_in` (which stops at a record boundary; `part_scan_z` continues raw from `rank+seen`, db.rs:19785-19791), `seek`, `owner_span_from`, `cut_in`, and every `ZCursor` method.
- No site that answers wrong was found.
- The chain `Paged::slow → false` → `ensure` false → `slice_slow` None → `page_bytes` None holds (pager.rs:1312-1339, bytes.rs:395-403, zfile.rs:2141-2148). So the read side needs **no zfile change**.
- Under the cipher today, a fill error poisons the store (pager.rs:1100-1103) and `end_slow` replaces every answer after it (db.rs:18140).
- Threads that read form pages for another operation: the upkeep thread, the builder ahead (its result is discarded, db.rs:30828), the segment-work thread (its attaches), the writer's inline fill, `install_ahead`, `rebase_tables`, `z_give`, and the open.
- On plain, the `supdb-zwarm` thread reads every form page through the mapping unchecked (zfile.rs:3784, 3802). A device EIO there is SIGBUS, by kernel knowledge.
- Only `count` has a None-page test (zfile.rs:8566-8571).

**Verdict.** No kill was defined.
- Item 6 supports §4's Regen-lazy row. The change lives in `Paged::slow`, which the design's per-file failure state already implies.
- Item 5 contradicts §3's "ctr makes it safe either way; this only bounds the test". The engine itself loses or refuses acknowledged data after one failed write, whatever the layer does.

**Changes to the design.**
- **§3, sessions.** Replace "A write that fails and is retried reseals at a new ctr. That answers the retry finding" with this. The engine latches a failed WAL or run write or sync as a sticky writer error (preferred), or rewinds to `written` and drops the trailing commit frame. The Handle **latches** a failed write and a failed fdatasync: every later write or sync on it returns the error, and the sequencer neither counts a failed sync as completed nor unpins its span. With a latch there is no retry under a key, which also closes E4's F3 by construction (below).
- **§4 Trust.** The change is `Paged::slow` (pager.rs:1100-1103). A fill error on a Regen-lazy file returns false without `Pager::poison`, and the failure is recorded per file so a bad page is not decrypted on every ask. db.rs:18140, 12865, 13033 and 28171 need no change. "The form's alone" covers tag failures only: a device EIO on the ciphertext mapping is SIGBUS under `Pg`, as it is on any plain mapping.
- **§10 S2, S4, S6 tests.** Inject at each syscall of a write call, at 0 bytes and at a short count. Under the cipher: reopen, write, reopen again. Cover the window after rotation and before retirement, and the run's sync arm and its piece-flush arm, each at 0 bytes and partial. Add mutants: "a sync after a failed sync reports success", "a failed write advances the Handle's len", and "a retried batch carries a second commit frame". These tests are red on today's engine, so they need the engine fixes first.
- **§10 S3.** A None-page test for each form read site, not only `count`. `tests/cipher.rs:1941` is rewritten to require the form out of service, as §10 says already.
- **§12.** Close open item 3 (the list of form read sites; it is the list above). Close "whether helper threads read Regen-lazy pages": they do. Their failure is carried by the shared `damaged` flag and a rebuild, recorded only in counters. Add: S7's WARM `warm_file` would read the memfd without `ensure`.

---

## E1: the compact form's upper bound in the clear, forced

Work directory: `work/e1-forced/`. Skeptic: stands, with two caveats.

**Question.** Out of core, in the clear, with both forms standing from the start, is the compact form ahead of raw on point reads?

**What was run.** All runs are `cd bench && TMPDIR=$W/tmp OUT=$W/pairs sh scripts/pair.sh SUPDB_ZPAGES_TRACE=1 supdb-nocachezpages supdb-nozpages 5000000 --bound <B> --passes read --reps 1 --ops-div 1`:
- B = 671088640, three times (6 sittings);
- B = 469762048, and B = 402653184 (2 sittings each);
- `supdb-nocachezpages supdb-nocache` at 671088640 (2 sittings).

The arms:
- `supdb-nocachezpages` is `ZPages::Always`, block cache off (engines.rs:5577-5586).
- `supdb-nozpages` is `ZPages::Off`, block cache on (engines.rs:6505-6513).
- `supdb-nocache` is `ZPages::Auto` (db.rs:3875), **not a raw arm**.

**Found.**

| bound | forced/raw, by sitting | forced faults/read | raw faults/read |
|---|---|---|---|
| 640 MiB | 2.57, 2.87, 3.01, 3.09, 3.19, 2.61; median 2.94 | 0.0000 | 0.136-0.156 |
| 448 MiB | 7.90 (raw first), 6.57 (forced first) | 0.017 | 0.52 |
| 384 MiB | 2.58 (raw first), 2.69 (forced first) | 0.169-0.174 | 0.646-0.647 |

- Forced arm: 19/19 form files built before the reads, 391,266,304 B, all block-coded. 5M reads through the form, 0 raw serves, 0 fallbacks, 98.8% of reads decoded a group.
- Raw arm: no form reads, and its block cache held nothing (`cache_bytes_at_end` 0).
- Fits, not registered predictions:
  - raw t = 0.99 µs + 41.4 µs × faults;
  - form t = 2.41 + 46.4 µs × faults;
  - break-even at 0.36 form faults when raw takes 0.44.
- Against `Auto` in the clear: 1.06x and 1.09x. The controller prebuilt all 19 block-coded files and served 4.98M of 5M reads through them.

**Skeptic's corrections, adopted.**
- **The order labels are swapped.** Under a bound rep 1 runs the second-named arm first (run.rs:878-880, 3258-3268). The "no order effect" reading survives the swap.
- **The ratio is not monotone in the bound** (2.94, 7.23, 2.64 by median), and a cipher store's caches match no single clear bound. So "the clear figure is an upper bound for the cipher" is **not established**.
- **The arms differ in more than the form.** The raw arm has the block cache on, and the forced arm's `sync` runs `zpages_settle`. Neither arm's anonymous memory was read. The magnitudes carry this; the direction does not.
- At 640 MiB the result measures the form wholly resident against raw mildly out of core. Only 384 MiB has both forms faulting.

**Verdict against the kill.** It does not fire. The form is ahead in all 10 sittings at all three bounds, so S5's controller step stays.

**Changes to the design.**
- **§8, form row.** Strike "the ratio the clear store shows in a forced pairing, within a tenth". It contradicts the row's own "about 1.4x", and the clear ratio ranges 2.6-7.9x with the bound. The row's point prediction is E1b's model (below), and the decisive pairing is a forced cipher arm.
- **§9/§10.** S3's "a forced pair as E1" under the cipher needs an arm that does not exist: no `create_cipher*` sets `zpages: true`. Add one, with the coding fixable to block (see E1b).
- **Arm hygiene.** `supdb-nocache` must not be used as a raw arm. Its doc comment (engines.rs:5563-5576) is stale.

---

## E1b: is the form's loss under a cipher the straddle?

Work directory: `work/e1b-straddle/`. Skeptic: does not stand as headlined.

**Question.** Today a 4096-byte form page lies across two 4080-byte chunks. Is that what makes the form read at 0.73x of raw under the cipher at 5M keys under 640 MiB?

**What was run.**
- `cd bench && TMPDIR=$W/tmp OUT=$W/runA sh scripts/pair.sh SUPDB_ZPAGES_TRACE=1 supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1` (2 sittings).
- The same with `--reps 3 --ops-div 5000` (`runB`), for each arm's fills outside the read pass.
- ftrace uprobes on the shipped binary:
  - `zask` at `<Paged as Paging>::ensure` (vaddr 0x306780);
  - `zpiece` at `piece_from` (0x3068c0);
  - `zfill` inside `Paged::slow` after a good decrypt (0x3076ef).
  These were limited to the reader thread and the read phase (`SUPDB_LAG_MARK=1`), in one sitting at `--ops-div 2`, reduced by `asks.py` and `form.awk`.
- Arithmetic from pager.rs:108-125 and flatindex.rs:57-63.

**Found (measured).**
- Every form-page ask is two chunks: 3,397,868 chunk asks for 1,698,934 page asks. The per-chunk marginal miss rates are equal (0.4355, 0.4364).
- Form fills a read: 0.795 and 0.807 (runA, unprobed); 0.872 probed. Raw: 0.684-0.701.
- A cold form page costs 1.65 fills and about 1.1-1.25 faults. Raw records take 1.029 chunks, and the hash slots are 97% resident.
- Faults per fill are equal across the two arms within a sitting: 0.64-0.67 unprobed, 0.75-0.76 probed. The kernel's cache of the ciphertext serves about a third of the fills in both.
- Rates at today's head: form against raw 0.845x and 0.791x. Faults 1.20-1.21x.
- A one-parameter model, fills a read = Σ P(chunk absent) with about 72-74k chunks held (≈ the pager's `held_at_end`), reproduces both arms' fills across runA, the probe and b4d555bc within about 2%.

**Skeptic's corrections, adopted.**
- **"The straddle is the whole gap" does not stand.** The model is fills = 2(1 − held/form chunks). The form file's size is a second, co-equal cause:
  - The controller built 10-15 of 19 partitions raw-tailed, giving files of 500-558 MB against the all-block 391 MB.
  - It does so because `choose_coding` prices against the whole `read_budget` (db.rs:39798-39810, 40574-40580), while under a cipher the pager holds half of it (vault.rs:637-646). Confirmed by the concluder's reading.
  - At the fitted cache, an **all-block 391 MB form with today's straddle** takes 0.43-0.47 fills a read against raw's 0.685. That is about 0.28-0.31 faults against 0.44, and **the form leads**.
  - Either fix alone flips the sign. Across runs, with the straddle fixed, the cell moved 0.60x / 0.73x / 0.75x / 0.79-0.85x as the raw-tail count went 15 / 13 / 11 / 10-11.
- **The kill as worded fired** (faults per page miss of 1.1-1.25, not about 2). The original result proposed replacing it with "fills = 2(1−c)". The skeptic notes that is an identity once c is read off the fills. The concluder adds one point: c was fitted *once* and then predicted fills across eight arm-runs and three file sizes, and it equals the pager's own held count, so the model is a test. Replacing the kill is still the owner's call.
- The whole-page counterfactual, (1−c) at the same c, follows from uniform reads (run.rs:2139). That makes it a model, not a measurement. Its inputs (the kernel's third, time per fault) moved between probed and unprobed runs.

**Verdict against the kill.**
- The kill as written fires, but it was mis-specified: it ignored the kernel's cache and neighbouring pages sharing a chunk.
- The mechanism it was meant to test is confirmed: two chunks per page, fills twice the per-chunk miss rate, and a model that holds across runs.
- The design's causal claim, that the 0.73x is the straddle, is **half right**.

**Changes to the design.**
- **§8, form row.**
  - Today's figure is 0.79-0.85x at `3df85d22` (2 one-rep sittings), and 0.60-0.85x across runs. Quote it with its raw-tail count.
  - "A cold form page today costs two stored pages and will cost one" becomes "each read asks two chunks, and fills a read are 2(1−c) today and (1−c) under whole-page chunks".
  - Name both causes.
  - Prediction (model): form 0.40-0.45 fills a read at today's coding; about 0.23 a read with an all-block form, i.e. about 0.15 faults against raw's 0.44. Time from E1's fits gives about 1.6-2.0x of raw. The design's "about 1.4x" is at or below the model.
- **New owed change, before or at S5.** Under a cipher, the coding pick prices against the pager's budget, not the group's. This is a fix to today's tree that can be priced on the envelope shape alone.
- **§9: new E1c**, owed before S3's out-of-core pairing and S5. A forced form under the cipher with the coding fixed to block, against `supdb-cipher`, 5M under 640 MiB, `SUPDB_ZPAGES_TRACE=1`. The model predicts the form beats raw today. If it still loses, the straddle is the larger cause. E1b(b), the `page+1` touch in the clear, remains the causal test of the straddle alone.

---

## E2: the cost of a miss, as a C probe

Work directory: `work/e2-miss/` (`e2.c`, `run.sh`, `summarize.py`, `raw/SUMMARY.txt`). Skeptic: does not stand as headlined.

**Question.** Under the Pagefile geometry, is a miss under 3.4 µs, and how much does it beat today's 4080-byte chunk? Which miss shape is cheapest, and what does a tag-tier miss add?

**What was run.**
- `gcc -O2 -o e2 e2.c -lcrypto` (OpenSSL 3.0.13 AES-256-GCM, tags verified, bad=0); `./e2 setup data`.
- `./run.sh main; ./run.sh main2`: arms i-vi at 1 and 3 threads, 11 rounds, rotated order.
- `./run.sh sparse` (STRIDE=3), `steady` (25% and 50% budgets), `split`, `drop`, `tag` (t1-t3), `gbbusy`, `cold`, `mems`, `base`, `gb`.
- perf 6.8 `cpu-clock` over `./e2 zloop`.

All 400 MiB ciphertext files were populated in the page cache. Medians of run medians, with the range.

**Found (1 thread, ns a page).**

| quantity | (ii) whole page | (i) today |
|---|---|---|
| dense, median | 2828 / 2876 | 2862 / 3038 (0.998 faults/chunk) |
| isolated (STRIDE=3), median | 3093 | 4441 / 4226 (2.0 faults/chunk) |
| steady 25% / 50% | 2812 / 2559 | 4265 / 3887 (1.60 / 1.33 faults per miss) |
| + give-back, by **mean** | dense 3.73 / 3.78; steady 3.62 / 3.27; isolated 3.99 µs | dense 3.69-3.89; steady 5.28 / 4.57; isolated 5.48 / 5.34 µs |

- **Give-back, amortised.** 0.45-0.8 µs a page, not the 0.22-0.34 that §8 took from probe.md: 464-562 idle, 619 with busy readers, 777 at 3 threads. probe.md's figure was a sequential fill freed on the same thread (uffd.c:243-262).
- **Split of (ii).** Fault about 2.0 µs, copy 0.7, cipher 0.47, tier lookup 0.2. Stamping inflates the total by 0.4-0.6.
- **(iii) pread.** Never cheaper than (ii): +7-11% dense, +4% isolated, +13-19% at 3 threads. This holds only with the ciphertext mapping's PTEs populated; the `SRCZAP=1` state was never run.
- **(iv) decrypt + pwrite.** 4.0-4.7 µs before give-back, above 3.4 in every condition. A full-page shmem write does skip the zeroing (`clear_page_erms` 0.00%), but zeroing is about 0.1 µs of a fault.
- **(v) 64-page range.** 2120 / 2247 ns a page, 2.6-2.8 µs with give-back. Populate is 0.65-0.75 of a fault, not half.
- **Tags.**
  - t2, a tier miss with the tag page in the kernel's cache: +0.8-0.95 µs. In the same set the unrotated arm order showed about 0.5 µs of arm-to-arm noise.
  - t3, the tag page from the device: about 45 µs at 1 thread, about 57 at 3.
- **3 threads on one memfd.** +23-38% per op; +7% with a memfd per thread. Not profiled, so VMA and inode contention are not told apart.

**Skeptic's corrections, adopted.**
- **The 4 µs calibration is not established.** By median without give-back, isolated and steady (i) match 4 µs. By mean with give-back, dense (i) matches it, and dense is where the gain vanishes. What the engine's 4 µs is was never defined by a count.
- **Read the kill by mean.** The cell is a throughput, so its cost per miss is a mean. By mean, (ii) plus give-back is above 3.4 µs in 4 of 5 single-thread cells, and the dense median reaches 3.43 with busy readers. Verdict: "at or above the line on this machine", with ±10-20% noise.
- **Ratio (ii)/(i).** 0.66-0.74 where today's chunk takes 1.3-2 faults; 0.88-1.02 in the dense regime, including the third sitting's 0.88.
- **pread against the copy** is settled only with the source PTEs populated.

**Concluder's cross-reading with E7, by inference and not counted.**
- **The engine's bounded misses look like E2's steady regime.** E7's profile puts the memfd fresh-page fault at 1.88 µs a read at 0.684 misses a read, about 2.75 µs a miss. At E2's about 2.0 µs a fault, that is about 1.4 faults a fill.
- **The 64 MiB arm looks isolated.** E7's 64 MiB arm evicts at 2.3 fills a unit, so neighbouring chunks are almost never present.
- **E2's populated ciphertext PTEs are not the engine's bounded state.** E7 measured envelope-mapping map/lookup at 1.10 µs a read against plain's 0.51, which is the zapped state E2 did not run. So E2's (ii)-against-(iii) verdict is untested exactly where the engine runs out of core.

**Verdict against the kill.**
- (iv) is killed.
- (ii) is at or above 3.4 µs by mean, so the kill fires marginally by the statistic the cell sees, and straddles by median.
- The out-of-cache row (0.19-0.21x) moves from "bounded by E2" to **unresolved**: 0.21-0.23x if the engine's fills take 1.3-2 faults, 0.17-0.19x (within noise of today) if they take one. One count decides it.

**Changes to the design.**
- **§8, single-page miss row.** Inputs: give-back 0.45-0.8 µs, not 0.22-0.34; populate 0.65-0.75 of a fault. Prediction: (ii) 3.3-4.0 µs by mean at 1 thread, unbounded. The relative claim is 0.66-0.74x of today where today takes 1.3-2 faults a fill, and about 1.0x on dense fills. "The real reason is half the faults" holds only for isolated and steady misses.
- **§8, range-fill row.** 2.1-2.25 µs a page before give-back, 2.6-2.8 after (was 1.5-2.0).
- **§8, past-8B row.** A tier miss is +0.8-0.95 µs (was 0.75), with about 0.5 µs of noise; a tag page from the device is about 45 µs.
- **§8, out-of-cache row.** Withdrawn from "bounded by E2" until the engine's minflt per fill is counted (below).
- **§4, Read and miss.**
  - Keep the copy from the mapping and `ru_majflt` *provisionally*. The pread decision has to be re-taken in the zapped-PTE state.
  - Say that one memfd per file serialises concurrent misses on a hot file (+23-38% at 3 threads).
  - Drop (iv) from the candidates.
- **§9.** E2's remaining work: `SRCZAP=1` for (ii) against (iii); the tag, sparse and steady sets in rotated order; a 3-thread run with one memfd mapped three times.

---

## E4: the crash model of the Log and Replace classes, extended

Work directories: `work/e4-model/` (`pfm.py`, `logrun.py`, `e4.py`, `replace.py`, `cmp_replace.py`, `dirmodels.py`, `explain.py`) and `work/e4-skeptic/` (`idealhdr.py`, `lap.py`, `recycle_lazy.py`, `recycle_twice.py`). Skeptic: does not stand as written.

**Question.** Do the protocols §4 designed but never checked survive the assumed device, and does each named mutant fail? The protocols: session bases with resume, TRUNC, finish and its fallback, the header lagging the first sync, the recycle header lagging the rename, Replace's REC at write with its sync on another thread, and table compaction. The device: sectors old or new, any write can tear, unsynced writes land in any subset, and the page cache survives a process crash.

**What was run.** A new Python model in the old one's shape, which itself re-ran green: `fix_caw 3` gave 0 violations, and `inflight` gave 0 with the pin and 244,032 losses without it. Families were enumerated over op alphabets, mostly unsampled, at PB=4, G=2 (G=4 once), K=2-8, R=5-8. Commands:
- `python3 e4.py base|inflight|header|resume|retry|trunc|finish|mix ...`
- `python3 replace.py plain|pg ...`, `python3 cmp_replace.py`
- `python3 dirmodels.py recycle|compact ...`

The skeptic re-ran the headline figures and added `idealhdr.py`, `lap.py`, `recycle_lazy.py` and `recycle_twice.py`.

**Found.**
- **The design as written, where already modelled, passes.** base maxlen 5 gave 0 of 11,779,603 'any' states; inflight R=8 gave 0 of 1,312,998.
- **F1, header lag on resume (LOSS).** header maxlen 6: 480 LOSS of 6,147,778; overwriting the newest slot instead gives 4,864. Ops `1 S X 1 X 1`: a second resume overwrites the only durable header with an unsynced one. Fixed by one fdatasync before a resumed Handle's first header write: 0 of 501,334. The skeptic showed this sync also carries the resumed Handle's **ring pins and re-home set**: with headers idealised and no sync, 237,072 LOSS of 758,264. So any alternative must make the recovered state durable before the resumed Handle writes.
- **F2, a sync covering an owed re-home (UNREADABLE).** Ops `5 5 P S 1`. Fixed by stopping `synced_home` at the first group whose re-home is pending, or by re-homing at the open: resume maxlen 5 gave 0 of 2,603,214; maxlen 6, time-limited, 0 of 39,449,074. The default re-home (every group closed in the run) plus this fix passes. "Re-home only failing tag pages" is not needed (skeptic). §4 step 6's "were never durable" is false.
- **F3, the frame nonce has no ctr.**
  - Spending the seq on a retry: 1,584 LOSS at K=8, 1,460 of them after a process crash alone.
  - Reusing the seq: 782 NONCE_REUSE.
  - Fixed by nonce `0x11‖seq‖attempt u24`, with recovery taking the highest attempt that verifies: 0 at K=2 and K=8.
  - Not modelled: a failed attempt whose frame landed whole (skeptic).
- **F4, TRUNC as written.** It fails three ways:
  - (a) the base page is not at the cut: `5 S X T3 S 1 S 1 S` recovers 3 of 4 acknowledged bytes;
  - (b) a session created at the cut: `5 S X T3 S` recovers 0 of 5;
  - (c) TRUNC as a CKPT: 936 UNREADABLE.
  - **The skeptic reversed reachability, and the concluder confirmed it by reading.** At a loose rotation boundary the open cuts the live WAL to its header (db.rs:4651-4659, 27876-27880, 27930-27934). That is a `set_len` below the recovered length, in a legal crash state under a buffered policy. "Refuse or debug-assert" would refuse or panic that open.
- **F5, finish.**
  - (a) Overridden pages written home in place destroy the image the ring fallback needs: LOSS, `5 S B O(P0) E Z S`.
  - (b) FINAL verifies only what finish wrote, so it trusts a torn trailer page: `1 S B 5 E Z S`.
  - Two fix sets pass all 1,404 sequences:
    - **verify_sh + ovr_rec**: FINAL carries the ≤4 override images, and FINAL verifies from `round_down(synced_home)`. 0 of 16,302,381.
    - **deferred + ovr_rec**: finish's writes and FINAL go after the final sync, unsynced. 0 of 5,510,928.
  - Their costs differ (skeptic). verify_sh reads the trailer and key section at every open of a finished run, against S4's test of "a handful of pages". deferred's FINAL is never synced by anyone, so the ring punch must wait for a sync nobody issues.
- **F6, Replace.**
  - Plain and `Pg` fail the same 924 of 2,539 sequences. The cause is the engine's: a publish overwrites the slot holding the newest durable publish while the other slot's write is owed, in flight, or only in the page cache. This is E0's defect (A).
  - The fix is to sync an owed slot before the next publish, and sync both slots at open: plain 0 of 109,468, `Pg` 0 of 1,513,393. In the default configuration (no `manifest_pending`), sync-owed-first plus open-sync suffices.
  - The loss needs a *torn* write of the overwritten slot (skeptic).
  - Under `Pg`, eager sessions fail with ops `X` alone (314,520 NO_MANIFEST). Lazy sessions fix it, or REC-key ownership with eager sessions (0 of 993,066, skeptic).
- **F7, recycle.**
  - The original counterexample is on the spare's own entry, which neither a lazy-header open nor the engine reads (db.rs:27858-27865). With the spare never opened, the design passes: 0 of 205,108.
  - But a **spare recycled twice** (the first recycle's header landed and its rename did not) refuses the WAL's open: 128 of 402. `rslot=other` does not fix it. Two fixes pass: read any foreign header on a recycled-flagged Log entry as an empty log, or take `prev` from the inode's newest header.
- **F8, compaction** passes: 0 of 186,512. Every mutant fails (no copy fsync: 3,268,580; no `sync_dir`: 249,588; and the rest).
- **Combined run with every Log fix.** mix maxlen 5, unsampled: 0 of 19,467,707 'any' and 760,337 process states (skeptic reproduced it). The design as written in the same family: 19,307 violations, sampled.
- **Named mutants.** All fail except two:
  - "Verify from synced_home without rounding" does not fail. With sealed tag pages the rounding is not load-bearing.
  - The restart half of "old-session frames accepted" fires only if a restart keeps pre-restart entries, which the design's restarts never do.

**Verdict against the kill.** It does not fire. Every acknowledged loss found in the Log path, finish and recycle closes in the model with a protocol change, re-run green. The manifest loss is the engine's and closes with an engine fix.

**Changes to the design (§3, §4, §5).**
1. **§4 Sync / header.** Replace "the header is never synced on its own" with: a resumed Log or Replace Handle issues one fdatasync before its first write, header included, unless it has seen a completed sync since. Once per resumed file per store open. Its cost is not measured; CLAUDE.md gives 0.03 ms for an fdatasync with nothing dirty. §4 must also say what a resumed Handle pins: the recovered state, made durable by that sync.
2. **§4 Recovery, step 6.** Re-home the tag page of every group the replayed run closed, without claiming they were never durable. `synced_home` stops at the first group whose re-home is pending, or re-home at the open. **Step 4.** The rounding is harmless and not load-bearing; strike "the model's variant without it failed".
3. **§3 frame nonce and the retry.** Either latch the Handle on a failed write (E0 items 5/6, preferred: there is no retry), or nonce `0x11‖seq‖attempt u24` with the highest verifying attempt taken. "seq never repeat" as written either loses data or reuses a nonce.
4. **§4 finish.** FINAL carries the ≤4 override images, and overridden pages never go home. In addition, *either* FINAL verifies from `round_down(synced_home)`, which drops S4's "handful of pages" test, *or* finish's writes and FINAL go after the final sync, unsynced, with the ring punch gated on a later sync of the file. This is an owner choice.
5. **§4 `set_len`/TRUNC.** TRUNC as a frame is withdrawn. A cut below the recovered length (reachable: WAL replay at a loose boundary) is done as a **restart**: F8's compaction shape, the recovered prefix written under one `restart` session to `.pfcopy`, synced, then renamed over. The model passes that shape. For the WAL's cut to its header the copy is one page.
6. **§4 Recycle.** A recycled-flagged Log entry reads any file_id mismatch as an empty log, or `prev` is taken from the inode's newest header. "A file_id mismatch in any other case is refused" stays for unflagged entries.
7. **§4 Replace.** Sessions are created at the slot's first write, or eager sessions are owned by the REC's key. The engine fix is stated in §5.
8. **§5, manifest row.** "Equal" holds only after the engine syncs an owed slot before the next publish and syncs both slots at open.
9. **§12.** Close "E4 has not yet modelled...". Add: Once from birth not modelled (by analogy with F5(b), a FINAL landing before its barrier is trusted, which matters for `pending_whole` and `recover_direct`); failures of the CKPT, header, re-home and finish writes not injected; a failed attempt with a whole frame not modelled; the ring punch not modelled.

---

## E5: the device cost of a Log commit

Work directories: `work/e5-commit/` (`e5.c`, `iocost.c`, `run.sh`, `pooled.py`, `trace.sh`, `tracecount.py`, `raw/`) and `work/e5-commit-skeptic/` (`an1.py`-`an5.py`, `tr.py`). Skeptic: does not stand as headlined.

**Question.** What does one Log commit cost at the device, against the plain run's commit and the plain WAL's, at 100 B, 4 KiB and 133 KB (plus 13.3 KB, the mixes' batch)? Kill: A well above B at small commits. Also: zero fsyncs at create.

**What was run.**
- `gcc -O2 -o e5 e5.c -lcrypto`, and `./run.sh` in 4 sittings, 5-10 reps × 1,000 commits per shape and size.
- Shapes:
  - A: the design;
  - Aw: write-through by pwrite;
  - A0: no write-through;
  - Ap: prefilled;
  - B: the plain run, from db.rs:6870-6893, 5707-5754, 6917-6936;
  - Cf and Cp: the plain WAL, fresh and prefilled;
  - D: today's cipher run commit.
- Also: `/sys/block/vda/stat` around each window, `strace -c`, block-request traces through tracefs, `iocost`, an `strace -f -y` of the real engine's load (`bench ab --arms supdb,supdb --size 30000 --passes load --reps 1`), and a journaled ext4 on a direct-io loop device.

**Found, with the skeptic's corrections.**
- **100 B.** A/B 0.69 [0.56-0.97], with A slower in 0 of 20 pairs, every sitting 0.66-0.76. Sectors 0.58x. The mechanism: A TAIL-only commit rewrites a ring page inside i_size, at 0.10 inode writes a commit against B's 1.01. On journaled ext4 A/B is 0.37 (counts reliable).
- **4 KiB and 13.3 KB.** The reported 1.29x and 1.39x are mostly an **ext4 discard mode** on this mount (`dioread_nolock,discard`, no journal). A block is allocated, freed and synchronously discarded each commit, at 55-65 µs of device time.
  - A's sparse layout is in that mode in 20/20 runs at 4 KiB and 18/20 at 13.3 KB; B in 6/20 and 4/20.
  - With both arms in the same mode: 4 KiB A0/B 1.08 (4/6), A/B 0.95; 13.3 KB A0/B 0.99 (2/4), A/B 1.08 (3/4).
  - **On this mount, the design's layout would pay 1.2-1.5x at these sizes.** The cause is extent and discard behaviour, mechanism not verified with `filefrag`, not the separate ring request.
- **133 KB, the load's batch.**
  - Same-mode A0/B 1.17 (8/8), Aw/B 1.31 (8/8), A/B 1.45 (5/5).
  - **Against today's cipher commit:** A0/D 1.00 [0.75-1.24] (10/20) and Aw/D 1.04 [0.83-1.42] (11/15), level within noise. Only memcpy write-through, A/D 1.16, is worse.
- **Write-through.** memcpy into the MAP_SHARED memfd: +65 µs a 133 KB commit. pwrite: +30-46 µs (paired Aw−A0 +46 [−29, +103]).
- **TAIL.** At ≥4 KiB a TAIL carries about 2 KB a commit, the unhomed partial page, which is written twice. The 40 B is the frame's own overhead only.
- **Fsyncs.** Exactly one fdatasync a commit in every shape. Zero at create **by construction**, since the probe issues none. The unsynced header riding the first sync costs nothing resolvable at 10 reps.
- **The real engine's strace** confirms B and Cf's shapes: one 124,024 B writev plus fdatasync a run commit; one 134,021 B write plus fdatasync a WAL commit.
- **Missing comparisons.** The WAL-cell figures (Ap/Cp 1.09 at 13.3 KB, 1.36 at 133 KB) carry memcpy write-through and the run's call shape. No prefilled arm without write-through exists, and no WAL-shaped arm.

**Verdict against the kill.** It does not fire at small commits. §8's "the log double write (20-50 ms of commits over 300)" saving does not exist: at the load's commit, the design's commit costs what today's double-written commit costs.

**Changes to the design.**
- **§8, ordered-load row (durable).** Remove the commits saving. The commits stay about +25 ms (today's D) to +38 ms (Aw) over plain across 300 commits. What is left of the prediction is the drain: the envelope (43-55 ms) and the unlink (14 ms), from CLAUDE.md's stamps, not re-measured here.
  - Arithmetic: if the drain gap closes and the commits stay where they are, the durable ordered load is about **0.80-0.90x**, with pwrite or no write-through, and about 0.76-0.83x with memcpy write-through. The plain load length is inferred at about 150-250 ms, not measured.
  - The buffered row is not re-priced by E5.
  - B/B: per-commit sectors are 1.04x plain at 133 KB, so 1.35-1.45 rests on a measurement at the commit level.
- **§4 Write.**
  - Write-through by `pwrite` into the memfd, not by memcpy through the shared mapping.
  - A TAIL frame carries the unhomed tail, about 2 KB a commit at ≥4 KiB, not about 40 B.
  - The separate ring request and the sparse layout's extent behaviour are the levers on ≥4 KiB commits. Two candidates, neither designed nor measured: keep the extent count stable from create (header B and ring/tag placeholders at create), and place the frame contiguous with the data.
- **§12, decision 1 (store-level integrity).** Its recommendation leaned on "(b) brings back part of the double write". E5 shows that keeping `log_run` costs about what the Log class's commit costs at 133 KB (A0/D 1.00). Keeping today's shape, where the run's commits are WAL frames, is therefore not dearer on commits and keeps today's store-level property. Add it as an option (c).
- **§10 S4 and S6.** Register E5's figures, not §8's, as the predictions. Before S6, an arm that is WAL-shaped (one write a commit plus `fadvise DONTNEED` per MiB), prefilled without write-through, to price the A and F cells at 13.3 KB. Read discard IOs (diskstats fields 12 and 14) on every run on this mount.
- **§12.** Add: the 1.2-1.5x at 4-13 KB commits on a discard mount, with its unverified extent mechanism; NVMe or a journaled fs may differ (journaled loop counts: A/B 0.94-0.96 at ≥4 KiB).

---

## E7: the cipher's cost to a store larger than its bound

Work directory: `work/e7-ooc/` (`pairs/`, `sampler.py`, `phasemem.py`, `stacks.py`, `profile.sh`, `perf-*.data`). Skeptic: stands, with corrections.

**Question.** What does the cipher cost a store that does not fit memory, against the same store in the clear? Is it the decrypt, the plaintext held beside the ciphertext's page cache, or something else? Does §8 have a row for it?

**What was run.** All at 5,000,000 keys, `--bound 671088640 --reps 1 --ops-div 1`, with `SUPDB_LAG_MARK=1 SUPDB_LOAD_PHASES=1 SUPDB_ZPAGES_TRACE=1`:
- `pair.sh supdb-cipher supdb ... --passes load,read`;
- `pair.sh supdb-cipher supdb-nozpages ... --passes load,read`;
- the same with `--passes read` only;
- `pair.sh supdb-cipher64 supdb-cipher ... --passes read`.

Also a profiled `bench ab --arms supdb-cipher,supdb-nozpages ... --passes read` with `perf record -e cpu-clock -g` for 20 s in each read pass, bucketed by `stacks.py`, and a 2 s cgroup and `/proc` sampler joined to the phase marks.

**Found, with the skeptic's corrections.**
- **Point reads, cipher against the clear raw store (both `ZPages::Off`).** 0.283-0.332x over four readings in two pairings.
  - Faults: 0.433-0.447 against 0.136-0.157 a read, about 3x.
  - Device cost per fault is equal: 33-34 µs an I/O, about 38-40 µs a fault with the wake.
  - Device bytes: one page a fault in both arms.
- **Memory over the read pass.**
  - Cipher: about 337 MiB of memfd plaintext (shmem, unreclaimable, no swap), about 240 MiB of ciphertext page cache, 38-47 MiB anonymous.
  - Clear: about 590 MiB of page cache.
  - The shmem runs 55-66 MiB above the pager's held count (268-278 MiB).
- **Profile, µs a read: cipher 24.4, clear 8.2.**
  - About **69%** of the +16.2 µs is the extra device faults, counting fault-scaled memcg reclaim.
  - About 31% is the miss path: about **7.3 µs a miss** at 0.684 misses a read. That splits into memfd fault about 2.7, extra fault entries about 1.8, envelope-mapping fault about 0.9, copy about 1.1, AES 0.6 and pager 0.4.
  - AES is 0.42 µs a read, 2.6% of the gap.
  - The evictor takes 6.7% of a core: half madvise/punch, 37% TLB-shootdown IPIs.
- **Budget split.** `supdb-cipher64` (64 MiB plaintext) against the default: 1.072x and 1.074x, which is below what a pairing resolves. Faults halve (0.222-0.236), misses rise to 1.138 a read, and 2.3 fills are made per unit evicted.
- **Against the shipped `supdb`** (whose controller buys the form and fits): 0.096-0.122x. That prices the missing form as well as the cipher.
- **Loads.**
  - Ordered: 0.61-0.74x against nozpages, 0.72-0.88x against supdb.
  - Shuffled: 0.545-0.574x. All of the loss is the closing sync, 13.8-14.9 s against 4.2 s, not decomposed.
  - **The 632-642 MB pager peak and the 202-292 ms evictor wait are sums over two stores** (run.rs:2611, 2653-2655, confirmed). The shuffled store's own peak is about 290-298 MiB, near the `hard` line of about 285 MiB. The "95% of the bound" reading is withdrawn.
- **Mechanism not measured.** "The two caches overlap" was not read; no `mincore` was taken. A rough model fits two independently evicting caches about as well (0.41 predicted, 0.433-0.447 observed).
- **The shmem excess.** If the 55-66 MiB is the 4080-byte chunk straddling memfd pages (256 chunks in 255 pages), the whole-page chunk returns it to the page cache. That acts on the **faults** term too, by roughly 2 µs a read, about 12% of the gap. That is an estimate.

**Verdict.** No kill was defined.
- The design's main premise for out-of-core reads, that the miss's own cost is the price, is **contradicted for a store larger than its bound**. Two thirds of the price is the bound split between plaintext and ciphertext, and the design keeps that split: the memfd backing and the copy from the ciphertext mapping (§4), and the half budget (decision 3a).
- The whole-page chunk acts on the miss third and perhaps on 12% of the faults term.

**Changes to the design.**
- **§8, new row.** Store larger than its bound, cipher against clear raw, 5M under 640 MiB: today 0.28-0.33x. Faults about 3x the clear store's; misses 0.68 a read at about 7-10 µs; AES 0.4 µs a read. Predicted under Pagefile: the miss term falls by roughly the E2 ratio, and the faults term by the shmem excess at most. Roughly 0.31-0.37x, a guess.
- **§8, single-page miss row.** Its "today 4 µs" is the unbounded cell's figure. Under a bound the non-device cost of a miss is about 7-10 µs (memcg charge, extra fault entries, the envelope mapping's minor faults), about twice as much.
- **§8, new row.** Shuffled load under a bound: 0.545-0.574x, all in the drain, with the pager at its `hard` line. "Memory under a bound: givable once home" is priced against this drain at S3.
- **§12, decision 3.** Decide it on this measurement, not on the in-core one.
- **§12, new open item.** A lever on the faults term is owed before S5 and S7, not designed and unpriced. Candidates: not caching ciphertext the plaintext covers (drop a page's ciphertext behind its fill), page-granular eviction in place of 1 MiB units, and a budget point between 64 MiB and about 268 MiB. Each must keep what §4 keeps the ciphertext mapping for: `ru_majflt`, the census, and the controller's model.
- **§12.** The controller's "two caches in series" model now has a measurement to be held to: faults 0.433-0.447 a read, with about 337 MiB plaintext and about 240 MiB ciphertext cached.

---

## What rests on a measurement now, and what is still a guess

| §8 row | status after the experiments |
|---|---|
| miss, single page | **Measured in a probe** (E2), unbounded: (ii) 3.3-4.0 µs by mean, 0.66-0.74x of today where today takes 1.3-2 faults a fill, about 1.0x on dense fills. **Not counted in the engine**: which regime it is in (minflt per fill), and what the 4 µs is. Under a bound the miss is about 7-10 µs (E7) and the probe does not model that. |
| 64-page range fill | **Measured in a probe**: 2.6-2.8 µs a page with give-back. |
| out-of-cache point read, 64 MiB / 1M | **Still a model**: 0.21-0.23x or 0.17-0.19x depending on the uncounted regime. |
| past 8B | **Partly measured**: a tier miss +0.8-0.95 µs (noise about 0.5), a device tag read about 45 µs. The share of misses affected is arithmetic. E6 is not run. |
| ordered load, durable | **Commit side measured** (E5: level with today's commit at 133 KB). **Drain side still from CLAUDE.md's stamps**, not re-measured. The 0.80-0.90x is arithmetic on both. |
| ordered load, buffered | **A guess**; E5 measured durable commits only. |
| device bytes per byte stored | **Measured at the commit**: 1.04x plain per commit at 133 KB. The drain's envelope removal is assumed. |
| form under a bound against raw | **Mechanism measured** (E1b), **clear ratio measured** (E1), **point prediction a model** with a coding confound. Decided by E1c, not run. |
| in-core point read (+320-335; WARM +20-60) | **A guess.** E3 not run. |
| in-core first pass after a load | **A guess.** |
| memory under a bound | **A guess**, with one fact: today's shuffled drain sits at the pager's `hard` line (E7). |
| new: store larger than its bound (0.28-0.33x) | **Today measured** (E7). The Pagefile effect is a guess. |
| new: shuffled load under a bound (0.545-0.574x) | **Today measured** (E7). Not decomposed. |
| crash protocols (§4) | **Model-checked** at small scale (E4) with the nine changes above. Once from birth, failed CKPT/header/re-home/finish writes and the ring punch are not modelled. |
| temp sweep, form read sites, durability order | **Settled by reading** (E0). |

## Stages: drop, reorder, add

- **Add S0: plain engine durability fixes**, before S1's gate or beside it, each with a failing test first. The tests construct the exact on-disk state, as CLAUDE.md requires, or use the skeptic's LD_PRELOAD shim (`work/e0-failures-skeptic/shim/`). These are plain correctness bugs, independent of the design, and the S2/S4/S6 failure tests are red on today's engine until they land:
  - latch a failed WAL or run write or sync (the double commit frame loses acknowledged commits, observed);
  - the run's accounting before I/O, and its partial write;
  - `commit()`/`sync()` returning Ok with a batch unsynced;
  - the helper's and the presync's error swallowed;
  - the drain's owed manifest slot overwritten unsynced (E0 (A) = E4 F6): sync it before the next publish, and sync both slots at open;
  - `manifest_pending`'s open and close rewrites;
  - a merge naming a seal after a failed `sync_dir`, or after a failed data fsync past the readable landing;
  - `direct-*.tmp`'s missing directory barrier;
  - `Db::sync()` under `EveryN` with an ordered table.
- **Add E1c and the coding fix ahead of S3's out-of-core pairing and S5.** This can run on today's tree (envelope shape). Size the coding pick against the pager's budget under a cipher, and pair a forced block-coded cipher form against `supdb-cipher`. If the form wins today, part of S5's decision is reachable without the layer, and S3's pairing then prices the straddle alone.
- **S2** takes E4's nine changes and E0's latch, and drops TRUNC-as-a-frame for TRUNC-as-restart.
- **S3: two gaps found by reading the design against the engine, not verified.** The run temp becomes an on-disk `Pg` Once file while `log_run` still carries durability, and two things follow:
  - (a) `commit_direct` calls `d.w.sync()` on every due commit (db.rs:28800-28807). Under the envelope shape that sync is a memfd's and free. Under S3 it would be a real REC plus fdatasync on top of the WAL's: two device syncs a commit.
  - (b) The open runs `recover_direct` on any `direct-*.tmp` it lists (db.rs:27717-27770). Under S3 the WAL also replays the logged run's commits, so the recovered piece and the replayed memtable would hold the same values twice.

  S3 must skip the run temp's per-commit sync while the run is logged, and must not recover a logged run's temp (remove it, as today's cipher store never has one). Both need a test.
- **S4 is gated, not dropped.** E5 measured its commit-side gain at about zero at 133 KB (A0/D 1.00). S3 already removes the envelope at close. What S4 alone adds is the 14 ms unlink and the small-commit device savings, against E4's six protocol changes and decision 1's weakening. Make S4 conditional on S3's load pairing, with `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1`, showing a remaining gap that `log_run` causes. Decision 1 gains option (c): keep `log_run`.
- **S5** stays: E1's kill did not fire. Its sign under a cipher depends on E1c and the coding fix, and E7 says the faults term, not the form, dominates out of core.
- **S6**: TRUNC is reachable and must work (as a restart). The manifest engine fix lands in S0. Before S6, price the WAL-shaped Log commit at 13.3 KB on this mount, with discard IOs read.
- **S7**: still gated on E3, which is not run. Its WARM `warm_file` must not read the memfd without `ensure`.
- **New stage or experiment before S5 and S7: the out-of-core faults lever** (E7). Decision 3 waits for it.

## Next experiments for what is unresolved

1. **E2's regime, in the engine, a count with no timing.** Over the bounded and the 64 MiB-cache point-read passes: the reader threads' minflt ÷ `cipher_counts().cache.fills`, and time in `Paged::slow` per fill. This defines the 4 µs and decides the out-of-cache row (0.21-0.23x or 0.17-0.19x).
2. **E2 reruns:** `SRCZAP=1` for (ii) against (iii), which is the engine's bounded state; the tag, sparse and steady sets in rotated order; 3 threads with one memfd mapped three times.
3. **E1c:** a forced, block-coded form under the cipher against `supdb-cipher`, 5M under 640 MiB (a scratch arm: `zpages: true`, `zpages_under_cipher`, coding pinned to block). The model predicts form fills 0.43-0.47 a read against raw's 0.685. Then E1b(b), the `page+1` touch in the clear.
4. **E7 follow-ups:**
   - `mincore` over the envelope mappings beside the pager's present chunks (overlap), and over the memfd against `held` (the 1.2x);
   - `supdb-nozpages` under a bound reduced by the cipher's plaintext footprint (does the page cache's size alone explain the faults?);
   - a budget point between 64 and about 268 MiB;
   - the shuffled load with `SUPDB_DRAIN_STEPS=1`, and an unbounded 5M shuffled pairing.
5. **E5 follow-ups:**
   - discard IOs and sectors per run, and `filefrag -v` of A's file at commits 1, 128 and 500;
   - extent-stable arms (header B and placeholders at create);
   - A0/D and Aw/D in two interleaved sittings, with D's per-key frame build and CRC (db.rs:28866-28872);
   - a WAL-shaped arm, and a prefilled arm without write-through;
   - a non-discard or journaled mount as a control.
6. **E4 follow-ups:** model Once from birth; inject failures of the CKPT, header, re-home and finish writes; a failed attempt with its whole frame landed; the ring punch after either finish fix set.
7. **E3** (callgrind of the plain cost at S1, and WARM's headroom) and **E6** (tag residency after S3): not run, still owed as §9 says.
8. **S3 gaps (a) and (b):** confirm by reading S3's code when it exists, and hold each with a test (a logged run's temp at open; fdatasyncs per commit counted under S3).
