# E8 ("once"): plaintext held once under a cipher, out of core. Results

Tree 3df85d22. The scratch patch was applied, tested, built into `bench-once`, saved as `patch.diff` (identical copy `once.patch`), and the tree was restored with `git checkout -- .` before any timing run. `git status --porcelain` was empty before the first timed run and at the end. At the end, with nothing running, `bench/target/release/bench` was rebuilt from the clean tree.

All paths below are under `S=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/once`.

## Verdict in one paragraph

Holding the plaintext once does not close the gap to the clear store.

The best fixed once-arm, `d432`, read the 5M point-read pass at **0.346x and 0.382x of the clear store** (`supdb-nozpages`). That is about **1.29x and 1.24x of `supdb-cipher`**, taken in the same session (R0: 0.281x and 0.283x). The comparison with `supdb-cipher` crosses pairings and is not paired.

`d432` was **OOM-killed in the shuffled load in both sittings** of the load,read run. The dynamic arm `ddyn` survived load,read in both sittings. It read **0.297x-0.332x** of clear across four sittings, which is **1.08x-1.15x of `supdb-cipher`**.

Against the kill line:

- **Strict reading.** The line needs the best surviving arm under 1.25x of `supdb-cipher` **and** under 0.45x of clear in both sittings.
  - With `d432` counted as surviving (it survived the read-only run), it fails the first condition in one sitting (1.29x). It is under 0.45x of clear in both. So the result falls between the lines: **"necessary, not sufficient"**.
  - Counting only arms that survive the load (`ddyn`), both conditions hold in both sittings. That **meets the kill line**.
- **The 0.6x-of-clear line** is reached by no arm in any sitting.

Either way, the faults term is **the plaintext cache's efficiency per MiB**:

- the misses a read follow the uncached fraction of the plaintext: 0.43-0.62 a read at 320-432 MiB, against 0.14 device faults a read for the clear store with about 590 MiB of page cache;
- shmem is 1.22-1.27x of `held` at every budget point.

Removing the double holding did not change the cache's efficiency. What it removed was most of the non-device miss path.

## What was built (the scratch patch, `patch.diff`, 800 lines of diff)

**src/db/pager.rs**

- `Pager::new_with(limit, once, dyn_pct)`.
  - `once` is 0 for today's path, 1 for D (O_DIRECT) and 2 for DN (RWF_NOWAIT first, then O_DIRECT).
  - `dyn_pct` is the evictor recomputing `limit = read_budget().budget × pct/100` every 100 ms, with a floor of 64 MiB.
  - `hard` and `swamp` are now derived from the current limit inside `crowded()` and `swamped()`.
- `fill` is split into `fill_from(c, stored)`.
- **`slow`'s EMPTY arm, when `once != 0`, calls `fill_run`.**
  - It extends a run of EMPTY→BUSY chunks within the unit, before the unit's rescue.
  - The run is bounded by a thread-local hint, by 32 chunks under a `pager::Hold`, or by 1 chunk otherwise. The hint is set by `ensure`'s multi-chunk branch and by `willneed`, which under once no longer advises the mapping.
  - It does one `pread` into a 128 KiB page-aligned thread-local buffer, decrypts each chunk, and puts the chunks back to EMPTY and poisons on failure.
- New counts: `direct_reads`, `direct_bytes`, `direct_us`, `run_chunks`, `nowait_hits`, `limit_lo` and `limit_hi`.
- `SUPDB_PAGER_TRACE=1` prints a `pagertrace` line once a second.
- `SUPDB_ONCE_FORCE=d|dn` is for tests only.
- **One change from the plan, forced by a bug the fuzz found.**
  - The plan had the O_DIRECT file opened lazily by path. A snapshot reading a segment that a merge had unlinked then failed with ENOENT: "a file of the store under its cipher did not read ... No such file or directory", in the first 200-seed fuzz run under D.
  - The fix: once any pager in the process has `once != 0` (`ONCE_ANY`), `Back::open` opens the O_DIRECT descriptor and keeps the buffered one at open time.
  - As a side effect, `supdb-cipher` in the same process as a once-arm holds two descriptors per envelope. This applies only to R9, which is unbounded with both arms in one process. No path a read takes changes.

**src/db/vault.rs**: `StoreCipher::scratch_once` and `scratch_dynamic`, passed to `Pager::new_with`.

**bench/src/engines.rs**

- The seven arms: `supdb-cipher{b384,b432,d320,d384,d432,ddyn,dndyn}`, with `ddyn`/`dndyn` at 75%.
- The `Policy` fields, the names, the open lines and the guarantee lines.
- The seven new `cipher_*` rows.

## Tests run on the patched tree (checked profile, TMPDIR=S/tmp, each run's test names checked with `--list`)

| run | result |
|---|---|
| `cargo test --lib pager`, plain | 10/10 pass |
| the same with `SUPDB_ONCE_FORCE=d` | **7 pass, 3 fail**: `a_range_asked_for_is_the_plaintext_and_nothing_else_is_decrypted` (6 fills expected), `a_view_says_what_is_in_memory...` (`willneed` returns 0 under D by design), `a_changed_chunk_fails_its_read_and_poisons_the_store` |
| `--test cipher`, plain (once=0) | 24/24 in three runs. **One run of four failed** `the_builders_rest_at_every_block_of_a_store_of_wide_values`. Whether that flake predates the patch was not checked on the clean tree (**not verified**) |
| `--test cipher`, `SUPDB_ONCE_FORCE=d` and `=dn` | 20/24 each. The four failures are bounds on fills and peaks: `an_attach_decrypts...` (fills), `a_snapshot_under_a_cipher...` (peak), `a_store_larger_than_its_cache...` (peak 14.0 MB against a 2 MiB budget), `the_compact_form_is_built...` (fills) |
| fuzz `fuzz_range`, `ZFUZZ_CIPHER=1 ZFUZZ_CIPHERCACHE=16384`, seeds 0-199, `SUPDB_ONCE_FORCE=d` | first run **failed** (the ENOENT above). After the fix: pass, 108 s |
| the same, seeds 200-299, `SUPDB_ONCE_FORCE=dn` | pass |
| `fuzz_range_under_a_cipher_that_does_not_fit_its_cache` (4 seeds) under d | pass |

What the D failures mean:

- None of them is a wrong byte read. The cipher authenticates every chunk, and the fuzz compares answers against the model.
- They are the 32-chunk lookahead under a `Hold`, which:
  - decrypts what was not asked for;
  - overshoots a tiny budget by runs per thread;
  - in the poison test, poisons the store from chunk 2 when chunk 0 was asked for (a read-ahead of a damaged chunk nobody asked for).
- A real design must not poison on a read-ahead.

## Sanity before timing (S/sanity)

- **Each scratch arm, 100k keys, `read,scan`, unbounded, and 300k keys under 256 MiB.** All completed with rc 0. Every arm showed `cipher_fills = 0`: written files are adopted whole, and nothing is evicted when the store fits.
- **`ddyn` against `dndyn` at 1M keys under 256 MiB (`--ops-div 10`).** rc 0. ddyn had 24,424 direct reads; dndyn had 14,173 direct reads and 10,216 nowait hits. No poison, no error. This is the check that the D and DN paths ran inside the bench binary.

## How the timed runs were made

`S/sit.sh NAME A B SIZE PERIOD OPTS`. For each sitting it:

- checks `ps` (no bench, cargo, rustc or perf running) and that `S/tmp` is empty;
- starts `S/sampler.py` on the memory group's parent;
- runs `env $UNSET TMPDIR=S/tmp SUPDB_LAG_MARK=1 SUPDB_LOAD_PHASES=1 SUPDB_ZPAGES_TRACE=1 SUPDB_PAGER_TRACE=1 S/bench-once ab --arms A,B --size SIZE --reps 1 OPTS`;
- records rc and the new dmesg lines, and removes `S/tmp/*` by name.

Sitting 2 runs B,A. `UNSET` is the plan's scrub of `GLIBC_TUNABLES`, `MALLOC_ARENA_MAX` and every exported `SUPDB_*`.

Outputs are in `S/pairs/NAME-SIZE-k.{out,err,rc,dmesg,df}` and `S/samples/NAME-k.txt`. `S/report.py` joins them, and its full output is `S/report-all.txt`.

- **Primary OPTS:** `--bound 671088640 --passes read --ops-div 1`, with 5M point reads in the read pass.
- **Sampler fix mid-run.** The sampler's `max_usage`, `failcnt` and `oom_kill` columns were missing in R0 and R1 sitting 1, because of an edit bug in the copy. They were fixed from R1 sitting 2 onward.
- **failcnt is the group's running count.** The tables give its rise over the phase.

## The primary cell: 5M keys under 640 MiB, read pass

Each row is one sitting. In sitting 1 the once or cipher arm ran first. "misses" is `cipher_fills` / 5M. "dev/read" is `read majfaults_per_op`, which is 0 under D because an O_DIRECT read is not a fault. "B/read" is `read read_bytes_per_op`; it counts O_DIRECT bytes, about 4096 a miss, which **verifies that the kernel accounts O_DIRECT reads in the task's read_bytes**. "µs/dio" is `cipher_direct_us / cipher_direct_reads`.

| run | arm | reads/s | clear reads/s | ×clear | µs/read | misses/read | dev faults/read | B/read | µs/dio | held at end MiB | p99 µs (arm / clear) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| R0-1 | supdb-cipher | 40,723 | 144,763 | **0.281** | 24.6 | 0.684 | 0.438 | 1792 | – | 278 | 106 / 65 |
| R0-2 | supdb-cipher | 42,254 | 149,260 | **0.283** | 23.7 | 0.684 | 0.432 | 1771 | – | 275 | 103 / 65 |
| R2-1 | d320 | 36,238 | 118,624 | 0.305 | 27.6 | 0.622 | 0 | 2549 | 36.5 | 314 | 94 / 75 |
| R2-2 | d320 | 37,088 | 154,375 | 0.240 | 27.0 | 0.619 | 0 | 2537 | 35.6 | 316 | 92 / 61 |
| R1-1 | d384 | 43,307 | 137,628 | 0.315 | 23.1 | 0.511 | 0 | 2094 | 36.9 | 373 | 92 / 73 |
| R1-2 | d384 | 44,446 | 134,353 | 0.331 | 22.5 | 0.511 | 0 | 2092 | 36.1 | 382 | 90 / 64 |
| R3-1 | d432 | 52,509 | 151,746 | **0.346** | 19.0 | 0.427 | 0 | 1749 | 36.5 | 419 | 86 / 61 |
| R3-2 | d432 | 52,241 | 136,775 | **0.382** | 19.1 | 0.428 | 0 | 1751 | 36.4 | 417 | 87 / 73 |
| R4-1 | b384 | 42,268 | 142,816 | 0.296 | 23.7 | 0.513 | 0.490 | 2005 | – | 379 | 100 / 67 |
| R4-2 | b384 | 39,293 | 141,755 | 0.277 | 25.5 | 0.514 | 0.492 | 2014 | – | 365 | 110 / 68 |
| R5-1 | b432 | 49,788 | 145,217 | 0.343 | 20.1 | 0.428 | 0.422 | 1727 | – | 431 | 99 / 67 |
| R5-2 | b432 | 48,251 | 145,873 | 0.331 | 20.7 | 0.428 | 0.423 | 1733 | – | 429 | 103 / 65 |
| R6-1 | ddyn | 46,657 | 153,304 | 0.304 | 21.4 | 0.490 | 0 | 2006 | 35.8 | 380 | 89 / 63 |
| R6-2 | ddyn | 45,656 | 137,496 | 0.332 | 21.9 | 0.490 | 0 | 2008 | 36.7 | 382 | 91 / 68 |

The clear store read 118.6k-158.8k reads/s across sittings, a spread of ±15%, at 0.137-0.170 faults a read. So every ×clear figure carries that spread.

Arm against `supdb-cipher`, by rate. These are across pairings, R0 sitting k against the same k, and not paired:

| arm | sitting 1 | sitting 2 |
|---|---|---|
| d320 | 0.89 | 0.88 |
| d384 | 1.06 | 1.05 |
| d432 | 1.29 | 1.24 |
| b384 | 1.04 | 0.93 |
| b432 | 1.22 | 1.14 |
| ddyn | 1.15 | 1.08 |

### Memory in the read pass (sampler medians, 1 s; the second half of the pass for cache−shmem)

| arm | usage med (max) | RssAnon | shmem med (max) | cache−shmem, second half | shmem/held (pagertrace joined to sampler) | failcnt rise | oom_kill |
|---|---|---|---|---|---|---|---|
| supdb-cipher (R0) | 632-634 (640) | 39-47 | 335 (348-350) | 235-242 | 1.221-1.225 | not recorded | 0 |
| d320 | 614-620 (640) | 39-47 | 397-398 (413) | 153-166 | 1.266-1.274 | 816-976 | 0 |
| d384 | 615-617 (640) | 38-47 | 471-472 (491) | 82-91 | 1.252-1.257 | 784 (s2) | 0 |
| d432 | 615 (636-640) | 39-46 | 516 (532-535) | 39-47 | 1.222-1.223 | 800-854 | 0 |
| b384 | 638-639 (640) | 43-47 | 474 (489) | 96-102 | 1.259 | 2.49-2.50M | 0 |
| b432 | 638-640 (640) | 38-47 | 518-520 (533) | 56-64 | 1.225 | 2.12-2.14M | 0 |
| ddyn | 617-621 (640) | 38-39 | 485-486 (502) | 78-82 | 1.255 | 653-685 | 0 |
| supdb-nozpages | 640 | 35-53 | 0 | 575-594 | – | 650k-822k | 0 |

- **D's usage stays below the limit.** Under D the group's usage sits 20-25 MiB under the limit (medians 615-620), and the failcnt rise is under a thousand per pass. B and the clear store sit at the limit and reclaim continuously: the failcnt rise is in the millions for B and in the hundreds of thousands for the clear store.
- **The kill line's "no rise in failcnt" condition cannot be applied as written.** The clear store's own read pass raises failcnt by about 0.7M. D raised it least of all.
- **No once-arm's reader took a major fault in the read pass** (`read majfaults_per_op` = 0.000). So there is no sign of text thrash.

### Evidence that the mechanism fired, and that the arms differed in it

- **D arms.** `cipher_direct_reads` equals `cipher_fills` exactly in every read-only sitting, for example 2,556,708 = 2,556,708. Read faults are 0. `run_chunks` is 0, so point reads are one chunk a pread, as planned.
- **B arms and the baseline.** `direct_reads` is 0, and device faults are 0.42-0.49 a read, which is 96-99% of misses for B.
- **The page cache that is not shmem fell** from 235-242 MiB (baseline) to 39-166 MiB under D.
- **What that remaining page cache is was not identified.** D's reads put nothing there, and usage is under the limit, so the memcg had no pressure to reclaim what the load left. That cache is therefore expected to be the ordered load's written envelopes and other files, but this is **not verified**.
- **The budgets read as intended.** `cipher_limit_lo`/`hi` read 320, 384 and 432 MiB for the fixed arms. For ddyn in the read pass they read 280-419 MiB, with a `pagertrace` median limit of 397 MiB.

## Predictions against measurements

| prediction (plan §8) | measured | verdict |
|---|---|---|
| misses/read: 0.60-0.62 at 320, 0.47-0.53 at 384, 0.36-0.45 at 432 | 0.619-0.622, 0.511, 0.427-0.428 | inside every band. The proportional model, 1.12(1−X/721) = 0.623, 0.523, 0.449, fits best; the linear fit undershoots at 432 |
| µs/read under D: 25.5-31.6 (320), 20.4-27.3 (384), 16.1-23.5 (432) | 27.0-27.6, 22.5-23.1, 19.0-19.1 | inside |
| D against supdb-cipher: 0.72-0.96x (320), 0.83-1.2x (384), 0.96-1.5x (432) | 0.88-0.89, 1.05-1.06, 1.24-1.29 | inside |
| D against clear: 0.21-0.32x, 0.25-0.40x, 0.29-0.50x | 0.240-0.305, 0.315-0.331, 0.346-0.382 | inside |
| ddyn between d384 and d432 | 0.304-0.332x of clear, 21.4-21.9 µs/read, X ≈ 397 | yes |
| device reads/read under D = misses, about a tenth from today's 0.44 | 0.43 (d432) to 0.62 (d320) against 0.43-0.44. d432 moves it 2% down; d384 and d320 move it 17% and 42% **up** | the device term did not shrink. The D gain is elsewhere (see the next section) |
| µs per O_DIRECT read against 32-38 µs a buffered fault | 35.6-36.9 µs, against 31.6-34.9 µs for the clear store's fault in the same sittings | O_DIRECT costs about 2-4 µs more an I/O here |
| shmem/held falls toward 1.1 at 432 if the straddle is the cause | 1.27 (320), 1.25 (384), 1.22 (432) | **not borne out**: it moves by 0.05 over the range. In the scan pass it read 1.01-1.10, and in the load 1.02-1.12. Under uniform point reads the fill density per unit does not rise much with X. This is consistent with the straddle (isolated chunks), but the run does not prove it |
| cache−shmem under 50 MiB in the second half of the D read pass | 39-47 (432), 82-91 (384), 153-166 (320) | met only at 432; see the evidence section above |
| B within ±10% of D at the same X | D/B: 1.02-1.13 at 384, 1.02-1.15 at 432 (across pairings) | D ahead in all four comparisons, by 2-15%; within ±10% in two of four |
| R7: ordered load 0.61-0.74x (ddyn) | 0.743 and 0.635 | inside |
| R7: shuffled load survival under ddyn, not predicted | survived both sittings, 0.739x and 0.875x of clear (E7 cipher: 0.55-0.57x) | see R7 |
| R8: scan within ±15% of the baseline cipher scan | d432 2.27x and 2.54x of supdb-cipher | **far outside** (see R8) |
| R9: D 0.4-0.7x of supdb-cipher in core | no fill at all in either arm; level within noise | the premise did not hold (see R9) |

## Where D's gain came from (per read, from the counts)

The non-device time per read is µs/read − misses × µs/dio:

| arm | non-device µs/read |
|---|---|
| d320 | 27.3 − 0.62 × 36.0 = **5.0** |
| d384 | 22.8 − 0.511 × 36.5 = **4.1** |
| d432 | 19.05 − 0.428 × 36.45 = **3.5** |
| ddyn | 21.65 − 0.49 × 36.25 = **3.9** |

- **For the baseline,** 24.2 − 0.435 × about 33 = **about 9.8**, which is E7's 7.9-9.5 µs a miss at 0.684 misses.
- **For the clear store,** 6.3-8.4 µs a read, of which about 4.6 is device. That leaves about 2-4.
- **So D roughly halved the non-device miss path.** It removed the envelope mapping's fault, the copy out of the page cache and the memcg's direct reclaim, which B still pays: B is 0.4-1.8 µs a read slower than D at the same X.
- **D left the device term where it was** for the same misses.

The remaining gap to clear, at d432:

- **About 11 µs a read of device time:** 0.43 device reads a read against 0.14.
- **1-2 µs of non-device time.**

That device term is the cache's efficiency. At 432 MiB of budget the store holds 417-419 MiB of plaintext (`held`) in 516 MiB of shmem, and misses 43% of reads. The clear store holds its file in about 590 MiB of page cache and faults on 14%.

R11 (the perf profile under D) was not run, so these splits are from counts and rates, not from a profile.

## R7: load,read, 5M under 640 MiB (sampler 0.5 s)

**R7, `d432` (the best fixed arm; ddyn read 0.87-0.89x of d432, so not within a tenth): OOM-killed in both sittings, rc 1.**

- `bench ab: no pair was measured: 1 of 1 reps were killed under the bound`.
- The kill came in the shuffled load, 3.7 s and 4.5 s after its start. That was after the ordered load and the 5M read pass, which took 97 s and 94 s.
- dmesg, sitting 1: `Memory cgroup out of memory: Killed process 1041 (bench-once) ... anon-rss:159864kB, file-rss:13104kB, shmem-rss:436572kB`.
- dmesg, sitting 2: `... Killed process 1197 ... anon-rss:183512kB, file-rss:13240kB, shmem-rss:367884kB`.
- The last samples had shmem 381-434 MiB and RssAnon 143-157 MiB, with the group at 640.
- This is the plan's §4 answer: **a fixed budget sized for the read pass does not survive the shuffled load.**

**R7b, `ddyn` (added beyond the plan, since R7 died and the dynamic arm is the plan's answer to §4): survived both sittings, rc 0.**

| quantity | sitting 1 (ddyn first) | sitting 2 |
|---|---|---|
| read ×clear | 0.331 (47,317 / 143,160) | 0.297 (47,141 / 158,821) |
| ordered load ×clear | 0.743 | 0.635 |
| shuffled load ×clear | 0.739 (313,720 / 424,451) | 0.875 (333,400 / 381,116) |
| shuffled load's closing sync (loadphases) | 8.52 s, against clear's 4.13-4.33 | 7.97 s |
| pagertrace limit, median | 314 MiB (shuffled load), 397 (read) | 299, 397 |
| RssAnon in the shuffled load | 156 | 174 |
| shmem in the shuffled load, med (max) | 313 (367) | 284 (339) |
| `run_chunks` | 119,832 (merges reading runs under a Hold) | 121,047 |
| evictor longest wait | 103 ms | 225 ms |
| `cipher_peak` | 801 MB | 795 MB |

- **Against E7.** The closing sync is 13.8-14.9 s in E7's cipher store, and the shuffled load read 0.55-0.57x there. These are across sessions and not paired.
- **What the improvement is attributable to is not separated.** It is within the shuffled load and could come from the run reads under a Hold, the different budget, or D.
- **`cipher_limit_lo`/`hi` and `cipher_peak` are sums over the pass's two stores**, so the per-store peak is not separable.

## R8: the scan pass, 5M under 640 MiB (one scan pass per sitting, 2-6 s long)

| | sitting 1 | sitting 2 |
|---|---|---|
| R8a supdb-cipher ×clear | 0.472 (903,796 / 1,914,483 entries/s) | 0.402 |
| R8a fills, faults/op | 139k fills, 2.19 faults/op | 139.6k, 2.12 |
| R8b d432 ×clear | **1.020** (2,052,135 / 2,011,534) | **0.860** |
| R8b fills, direct reads | 87.4k fills, 48.7k direct reads | 88.5k, 50.3k |
| R8b run_chunks | 60.7k (about 69% of fills read as part of a run) | 60.0k |

- **d432 scanned 2.27x and 2.54x as fast as supdb-cipher**, against a prediction of ±15%.
- **Two things moved at once, and they are not separated:**
  - `willneed`'s exact ranges turning into one pread per run, where the baseline faults page by page;
  - a 432 MiB budget against 280. Fills are 37% fewer.
- **The comparison crosses pairings**, and each scan pass is a few seconds.

## R9: in core, 1M keys, no bound, `--passes load,read,scan --reps 4` (`r9b-*`)

The first attempt (`r9-*`) ran 1 rep, because `sit.sh` passes `--reps 1` before OPTS and ab kept the first. It was rerun directly as `r9b` with `--reps 4`, without the sampler, since an unbounded run has no group.

**Neither arm filled a single chunk** (`cipher_fills` = 0). In an unbounded `ab` the store is written and read in one process, so every file is adopted whole and never evicted. The plan's premise, that first touches go to the device after a reopen, never arose, and the D path was not exercised in core.

d432 against supdb-cipher:

| pass | sitting 1 | sitting 2 |
|---|---|---|
| read | 0.966 | 1.075 |
| scan | 1.033 | 1.190 |
| ordered load | 1.081 | 0.974 |
| shuffled load | 0.974 | 0.999 |

The sign tests are 0.125-1.0, so nothing is resolved. This is noise over identical work.

**R10 (DN) was skipped**, since D did not lose in core. The first-touch cost of D after a reopen in core is **not measured**.

## What was not run, and failures

- **R11 (the perf profile under D) was not run.** The split of the non-device time above comes from counts, not a profile.
- **R10 was skipped** (see R9).
- **R7b (ddyn under load,read) was added** after R7's kills. It is not in the plan's list.
- **The test failures listed above:**
  - 3 pager unit tests and 4 cipher tests under D/DN, all on decrypt counts and peaks, from the 32-chunk lookahead under a Hold. That lookahead also poisons the store on a damaged chunk nobody asked for.
  - One plain-mode cipher test flaked once in four runs. Its cause was not checked against the clean tree.
  - The ENOENT bug the fuzz found, fixed before the build that was timed.
- **The sampler lacked `max_usage`/`failcnt`/`oom_kill` for R0 and R1 sitting 1.**
- **Every bounded sitting ended with rc 0 except R7's two (rc 1, OOM).** No idle-reclaim `drop_caches` line fell inside any sitting's window (the `.dmesg` files are empty except R7's).
- `S/tmp` was emptied after every sitting and is empty now. No `supdb-bench-*` group remains. `df -h /` shows 6.1 GB free.

## What the prototype cannot show

This is unchanged from the plan's §9:

- whole-page chunks;
- page-granular eviction;
- the census and controller under D, which is blind to the misses, as `read majfaults_per_op` = 0 shows;
- who owns the budget (a fixed budget dies in the shuffled load);
- asynchronous misses;
- first touch after a reopen in core;
- writes;
- two sittings per figure, so differences under a tenth are unresolved.

## Files

- `S/patch.diff` (and `once.patch`), `S/bench-once`, `S/edit1.py`, `S/edit2.py`, `S/edit3.py` (the scripted edits that made the patch).
- `S/sit.sh`, `S/sampler.py`, `S/report.py`, `S/phasemem.py`.
- `S/report-all.txt` (every table and memory join).
- `S/pairs/` (`.out`, `.err`, `.rc`, `.dmesg`, `.df`, `head.txt`), `S/samples/`, `S/sanity/`.
