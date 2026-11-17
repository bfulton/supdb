# E7: the cipher's cost to a store that does not fit its memory bound

Tree 3df85d22, clean. Bench binary `bench/target/release/bench`; `cargo build --release` said it was fresh (0.77 s, no compile). Machine: 4 vCPU, 16 GB, no swap, cgroup v1. Nothing else was running at any timed start (checked with `ps`). Each sitting is one pair (`--reps 1`, fresh process per pass), so every figure below has two readings, one from each sitting order, and the spread is those two.

## What was run

Every run was 5,000,000 keys, `--bound 671088640` (640 MiB), `--ops-div 1` (5M point reads), `TMPDIR=work/e7-ooc/tmp`, with `SUPDB_LAG_MARK=1 SUPDB_LOAD_PHASES=1 SUPDB_ZPAGES_TRACE=1` set for both arms. These are harness phase stamps, a load-phase line and the controller's trace. None of them is in a timed loop.

1. `pair.sh supdb-cipher supdb ... --passes load,read`. The task's command. `supdb`'s controller bought the form in both sittings, so:
2. `pair.sh supdb-cipher supdb-nozpages ... --passes load,read`. This prices the cipher alone: both arms run the raw form with `ZPages::Off`. The cipher arm gets Off through `cipher_controller`, src/db.rs:11529-11533, and the plain arm through engines.rs:7068.
3. `NAME=ro-... pair.sh supdb-cipher supdb-nozpages ... --passes read`. Read only, so the cipher counters cover only the ordered load and the read pass: no shuffled load, whose store's counters ab sums in (bench/src/run.rs:2610, 2642-2647).
4. `NAME=ro-... pair.sh supdb-cipher64 supdb-cipher ... --passes read`. A 64 MiB plaintext cache against the default under the same bound. This is the budget split.
5. A profiling run, not for rates: `bench ab --arms supdb-cipher,supdb-nozpages ... --passes read`, with `perf record -e cpu-clock -g -p <pass pid>` for 20 s, taken 10 s into each arm's read (`profile.sh`). perf 6.8's `report` segfaults on call graphs, so the stacks were bucketed from `perf script` by `stacks.py`.

A sampler (`sampler.py`, every 2 s) read the bench group's `memory.stat` and the pass child's `/proc/<pid>/status` from outside the process. `phasemem.py` joins those samples to the phase stamps.

## The cipher's plaintext budget under this bound

`supdb-cipher` sets no cache. `cipher_cache: 0` gives the engine's own choice (engines.rs:7225-7228), which is `default_cache()` (vault.rs:637-646, called at vault.rs:776). That is half of `read_budget().budget`, and the budget is limit − anon − margin, where margin = max(limit/20, 64 MiB) (zselect.rs:2232-2246, zfile.rs:5209-5268). Here that is (640 − ~40 − 64)/2, about 270-280 MiB.

What was measured:
- `cipher_held_at_end` was 268-281 MiB.
- During the read pass the group's `shmem` (the memfd plaintext) was 334-340 MiB median, and 349-354 MiB at most.

The pager allows slack above its limit: `hard` = limit + max(limit/16, 4 units), and `swamp` = limit + limit/2 (pager.rs:317-321). Why shmem stays about 55 MiB above `held` (about 1.2x) is not verified. The candidate cause is that a 4080-byte chunk touches two memfd pages.

## Results (A = supdb-cipher)

Pairing 2, cipher alone against the clear raw store. The two values in each cell are sitting 1 (plain ran first) and sitting 2 (cipher ran first).

| quantity | supdb-cipher | supdb-nozpages | A/B |
|---|---|---|---|
| read reads/s | 44,206 / 40,999 | 148,194 / 123,475 | **0.298x / 0.332x** |
| read majfaults/op | 0.433 / 0.447 | 0.139 / 0.157 | 3.11x / 2.85x |
| read device bytes/op (thread's own) | 1774 / 1830 | 570 / 642 | about 1 page a fault in both |
| read refaults/op | 0.433 / 0.447 | 0.139 / 0.157 | |
| read µs per device I/O | 32.3 / 34.1 | 33.0 / 34.7 | equal |
| read p99 µs | 95.7 / 103.9 | 63.0 / 75.3 | |
| ordered load ops/s | 1,021,857 / 946,086 | 1,387,404 / 1,539,782 | **0.737x / 0.614x** |
| load device bytes per byte stored | 2.500 / 2.500 | 1.339 / 1.339 | |
| bytes on disk per byte | 1.362 / 1.362 | 1.303 / 1.303 | |
| shuffled load ops/s | 244,910 / 223,275 | 426,499 / 409,853 | **0.574x / 0.545x** |
| shuffled load's closing sync (loadphases) | 13.76 / 14.85 s | 4.17 / 4.16 s | |
| shuffled load commits | 5.64 / 6.35 s | 6.55 / 6.92 s | |
| shuffled bg device bytes/op | 190 / 196 | 464 / 398 | |

Pairing 3, read only: 0.298x and 0.283x. Faults per read: cipher 0.445 / 0.435, plain 0.136 / 0.140. Fills (plaintext misses) 3,422,662 and 3,417,404 per 5M reads, which is **0.684 misses a read**. 65% of misses went to the device; the rest were served from the kernel's cache of ciphertext. Evicted units 157k (about 22 fills a unit), evictor waits 416-418, rescues 37-39k, peak 329 MB.

Pairing 1, against the shipped `supdb`. Its controller bought the form in both sittings: zpage_builds 39 and 40 (all prebuilt ahead), sel_installs 1 and 1, and zpage_reads 4,979,424 and 4,985,159 of 5M. Its form fits the bound (usage 460-469 MiB, page cache 379-385 MiB, anon 68-71 MiB).
- read: 0.0963x and 0.1217x (40,688 against 422,508; 42,024 against 345,214).
- faults per read: 0.441 / 0.445 against 0.0057 / 0.0043.
- ordered load: 0.721x and 0.878x.
- shuffled load: 0.637x and 0.654x.

Pairing 4, `supdb-cipher64` against `supdb-cipher`. The 64 MiB arm reads **1.072x and 1.074x** of the default, which is still about 0.32x of the clear store; that figure is across runs and not paired.

| quantity | 64 MiB | default |
|---|---|---|
| faults per read | 0.222 / 0.236 | 0.442 / 0.433 |
| misses per read | 1.138 (5.69M fills) | 0.684 |
| evicted units | 2.47M / 2.48M (2.3 fills a unit) | 159k / 161k |
| µs per I/O | 38.5 / 37.5 | 32.9 / 33.4 |
| shmem | 68 MiB | 338 MiB |
| page cache of ciphertext | 510-518 MiB | 234-242 MiB |

### Cipher cache counts, pairings 1 and 2 (load and read, both stores)

| count | value |
|---|---|
| fills | 3.659-3.666M |
| evicted_units | 155-162k |
| held_at_end | 284-295 MB |
| peak | **632-642 MB** (329 MB in the read-only runs) |
| waits | 355-419 |
| breaths | 6-7 |
| rescues | 32-39k |
| evictor longest wait | 202-207 ms (26-29 ms read-only) |

So the pager's held count came to about 95% of the bound in the shuffled load, against about 280 MiB of budget. Yet the 2 s sampler's group shmem never went above 311 MiB in that phase. Which of the two is right, and when the peak happens, is not verified.

### Memory each arm held (sampler medians in the read pass; max in brackets)

All values are MiB.

| arm | group usage | RssAnon | shmem (plaintext) | page cache not shmem |
|---|---|---|---|---|
| supdb-cipher | 631-636 (640) | 38-47 | 334-340 (354) | 233-243 (279) |
| supdb-nozpages | 640 | 34-46 | 0 | 583-594 |
| supdb (form bought) | 460-469 | 68-71 | 0 | 379-385 |
| supdb-cipher64 | 639 | 38-46 | 68 (76) | 510-518 |

In the shuffled load, RssAnon was: cipher 154-183, nozpages 140-144, supdb 126-140.

## Where the cipher's read time goes (profile run, per read)

The profile run read 40.9k against 121.6k reads/s; perf was attached. The reader thread was on-CPU 11.9 s and 11.3 s of the 20 s window.

| | cipher µs/read | plain µs/read |
|---|---|---|
| wall | 24.4 | 8.2 |
| device: doorbell exit (iowrite16), wake/switch, off-CPU wait | 16.9 (38.3 a fault) | 6.1 (39.6 a fault) |
| memfd fresh-page fault (shmem_fault ...) | 1.88 | 0 |
| other page-fault entry and exit | 1.38 | 0.17 |
| envelope mapping fault, map and lookup | 1.10 | 0.51 |
| memmove (chunk copy plus the value consume) | 0.89 | 0.15 |
| read path, user | 0.66 | 0.88 |
| memcg direct reclaim | 0.59 (1.35 a fault) | 0.21 (1.38 a fault) |
| AES-GCM (ring) | **0.42** (0.61 a miss) | 0 |
| pager, user | 0.28 | 0 |

On another core, the pager (evictor) thread was 6.7% of a core: 49% madvise/fallocate punching memfd units, 37% TLB-shootdown IPIs.

Gap: +16.2 µs a read.
- **Device: +10.8 (about 2/3).** This is the same ~38 µs a fault in both arms, on 3x the faults.
- **Non-device: +5.4 (about 1/3)**, about 7.9 µs a plaintext miss. Of this, the cipher itself (AES) is 0.42 µs a read, **2.6% of the gap**.

The same split from the paired rates and counts (pairing 3) gives +10 µs device and +6.5 µs non-device, about 9.5 µs a miss. Pairing 4 gives about 9.2-9.5 µs a miss at 1.14 misses a read.

## What it means

**The price is not the decrypt-on-miss.**
- About two thirds of it is the plaintext held beside the page cache's ciphertext: 3x the device faults per read.
- About one third is the miss path's kernel work around the decrypt: the fresh memfd page fault, extra fault entries, the envelope mapping's minor faults and the copy, and on the other core the evictor's punches and IPIs.

Why the faults triple: under a bound, the plaintext cache (memfd shmem, unreclaimable without swap, about 337 MiB resident) and the kernel's cache of ciphertext (about 240 MiB) split the 640 MiB that the clear store gives wholly to the page cache (about 590 MiB). The two caches overlap: a miss puts the page in both. A plaintext hit never touches the kernel's copy, so the kernel's LRU sees only the filtered miss stream.

Shrinking the plaintext cache to 64 MiB halves the faults (page cache about 515 MiB). It also raises misses to 1.14 a read at about 9.5 µs each, and the evictor punches a 1 MiB unit for every 2.3 chunks. The net is 1.07x. So no point on the budget split reaches the clear store: even with page cache close to plain's, the faults are 1.5-1.7x plain's. Why is not verified; the filtered LRU and the extra 4.5% stored size are candidates.

**Section 8 has no row for this cell.**
- Its out-of-cache row (0.17x, predicted 0.19-0.21x) is 1M keys under a 64 MiB cache on an unbounded 15 GB box, with the ciphertext resident.
- The "past 8B" row prices only tag-page reads.
- "Memory under a bound" is about outputs held whole.

The design keeps the memfd backing (design.md §4, "The plaintext backing stays a memfd", and "copy from the ciphertext mapping"). It keeps the budget at the caller's `cache_bytes`, which is today's half of the budget (decision 3a). It adds write-through of written pages into the memfd. So the design as written leaves the two-thirds device term where it is.

Its whole-page chunk acts only on the non-device third:
- one memfd page a miss, where it is two today;
- no straddle;
- possibly the about 1.2x of shmem over `held`.

That is a share of 1.9 µs memfd fault plus 1.4 µs fault entry a read. At best it is a few µs of 24.

The row to add: a store larger than its bound, cipher against the clear raw store, at **0.28-0.33x** today. It should list three terms:
- the faults term, 3x;
- the miss term, about 8-10 µs a miss at about 0.68 a read;
- the AES term, 0.4 µs.

A design lever on the faults term is owed and not designed. Options, all unpriced:
- not caching the ciphertext at all for files the plaintext cache covers, and giving that memory to the plaintext cache;
- dropping a page's ciphertext behind its fill;
- page-granular eviction in place of 1 MiB units.

Each of these trades against what §4 keeps the ciphertext mapping for: `ru_majflt`, the census, and the controller's model.

**Writes.** The ordered load is 0.61-0.74x of the clear store, in line with the in-core 0.68x row. The shuffled load is **0.55-0.57x** out of core against 0.80-0.84x in core. All of its loss is the closing sync: the drain's merges, 13.8-14.9 s against 4.2 s. Its commits are not slower. The drain under the bound is not decomposed here (not verified). Candidates are the merge outputs held whole until their envelope (the pager's held count reached 632-642 MB) and the waits on a crowded cache (evictor longest wait 202-207 ms, against 26-29 ms without the shuffled load). The design's write-through and "givable once home" bear on it, and §8's "memory under a bound" row should be read against this drain.

## Files

All under `work/e7-ooc/`:
- tables: `pairs/*.out`;
- stderr with phase stamps and the controller trace: `pairs/*.err`;
- tree and settings: `pairs/*head.txt`;
- memory samples: `samples-pair{1..4}.txt`, with `phasemem-all.txt` the per-phase join;
- profiles: `perf-supdb-{cipher,nozpages}.data`, with `rep-sym.txt` and `stacks-buckets.txt`;
- tools: `profile.sh`, `sampler.py`, `phasemem.py`, `stacks.py`.

`TMPDIR` was removed (empty at the end), and no `supdb-bench-*` cgroup remains.
