# Z-pages: a compact second form for out-of-core point reads and scans

Pre-registered design for the first instance of the principle in `CLAUDE.md:526-543` ("a cell a
comparator wins is a workload the engine has no strategy for"). Written against HEAD `4ea80c4` and revised
against three adversarial reviews at HEAD `5dbfa23`; every change the revision made is listed, finding by
finding, in the last section ("Review record") and marked in the text as `(rev r.n)`, review `r`, finding `n`.
`git diff 4ea80c4 5dbfa23 --stat` lists `bench/src/engines.rs` (5 lines added at 4263) and `bench/src/run.rs` (26
added at 491, 10 at 2835) and nothing else, so **every `src/`, `tests/`, `docs/` and `.github/` file:line in this
document is valid at both commits**; the `bench/src/` lines are `4ea80c4`'s (`git show 4ea80c4:<path>`) and sit 5
(engines.rs, past 4263) or 26-36 (run.rs, past 491 and 2835) lines later at HEAD. Nothing here was built, run or
timed by this work: the machine was held by timing runs, so it read files and recomputed arithmetic in a few
lines of Python, and did nothing else.

Order of registration, because a pre-registration is worth its order. Section 7's ranges were written before the
standalone probe's 10M result (M17) arrived; 7.2a scores them against it and changes none of them. What the
revision added or changed is tagged [P2]: written after M17 and after the reviews, before any engine code, with
its derivation shown. A [P2] range is held to the rule a [P] one is -- a value outside it is a refutation -- but it
is not blind to M17, and the verdict reads the two apart.

| tag | meaning |
|---|---|
| **[M]** | measured by someone, in a file that is cited |
| **[D]** | derived here by arithmetic from [M] and the code; the formula is shown |
| **[P]** | predicted by this move before M17: a claim the verdict in section 7 can refute |
| **[P2]** | predicted in the revision, after M17 and the reviews; the same status, not blind to M17 |
| **[U]** | not measured, or not found; said plainly where it matters, and gathered in the appendix |

Naming. The option, the arms, the counters, the files and the tests say `zpages` and `zpg`.
`compact` is taken twice already (`Options::compact`, `src/db.rs:424`; `SegmentOptions::compact_records`,
`src/db.rs:5078`, and the compact record of format 0007, `src/flatindex.rs:685-705`), and `regime` is
taken by the maintenance regime of the upkeep (`src/db.rs:10104`, `10130`, `16984`), so this document
says *form* for what is chosen (the raw form, the ordered-index form L1, the z form), *rung* for the controller's
states, and *selector* for the store-level word and the controller that sets it (`ZSelect`, module
`src/db/zselect.rs`; the first version called both `ZRegime`, the name the upkeep already holds: rev 3.12). It
uses "regime" only where the task text does. A quantity with a name is used under that name
(`CLAUDE.md`, "A constant with two meanings"): the z-page size is `ZPAGE`, not `format::SUPER`
(`src/format.rs:43`), not `block::CHUNK`, not `flatindex::PIECE_SHIFT` (`src/flatindex.rs:255`), not
`SegmentOptions::write_piece` (`src/db.rs:5091`).

## Summary (rev 3.8)

* **Thesis.** At 10M keys under a 1 GiB bound the raw form takes 0.41 major faults a point read
  (`lmdb-nordahead`: 0.27) because the hash (about 190 MB) and the records (about 1.3 GB) compete for a 0.94 GB
  cache. Records packed into LZ4 groups that each fill one 4 KiB page are about 0.72 GB, fit that cache, and a read
  then takes one fault at most, and none once the file is resident.
* **Evidence in hand.** One standalone probe at that cell (M17): 0.000 faults a read from the second pass at
  262-280k reads/s against the raw layout's 0.42 at 54-58k, with the z file at 79% of the cache. Nothing is
  measured inside the engine, with more than one thread, with skewed reads, for scans, for key shapes other than
  sixteen sequential digits, above 79% occupancy, or for the controller (7.2a, point 4). The probe's source and logs
  exist in a session scratchpad only; banking them is the first act of stage 0a (8).
* **What the reviews changed.** The form is a window against LMDB, not a law (D9, 7.2b): it beats `lmdb-nordahead`
  by 1.5x or more up to about 2.6 times the cache, ties it near 7 times and trails it by a tenth past that (both
  take one fault a read and the decode adds three microseconds), while it stays 1.75x ahead of the raw form
  everywhere past 3 times, because the raw hash stops fitting too. The controller therefore builds on a predicted
  saving and leaves when the realized one falls under break-even (4.4, 4.5). Every read, count and scan of a
  covered partition must leave the raw hash, directory and records alone, and three scan paths did not
  (3.8-3.10). The census needs refaults, scan windows and windows that tolerate commits, or `Auto` cannot enter for
  a mix or a scan (4.3). `Auto` degrades where it cannot run instead of refusing the open (6.1). The default is
  flipped only behind a gate that the forms-memory lead has closed (7.8). The plan runs the cheapest experiments
  that can fail first (8).
* **The first experiment.** Stage 1a, the margin sweep on the probe's existing files: no code, about 30 minutes (8,
  "The first experiment"). Banking the evidence (0a) is a repository write, which this revision did not make.
* **Stop rule.** Records a page under 36 on the suite's shape, any 7.2 refutation, any `raw_touches` on a covered
  partition, or an in-memory mark that keeps its sign in two sittings (7.7).
* **Deferred, and said so.** A compressing RocksDB arm (R11), L2 (9.3), the default policy under the bound (R17),
  merge-time builds (stage 5), `Auto` off Linux (2.9).
* **Open.** Eleven questions are listed at the end of the Review record. The first is the one this document cannot answer: past
  about seven times the cache, what would let a form that decodes match LMDB's reads.

## 0. Premise: what is being decided, and the constraint it answers

The request this document answers (relayed with the task, user's words): *every loss should be
reversible*; the sentence "past the memory line trading space for speed is no longer free" is "maybe,
but probably not"; store both the larger and the more compact form and pick the advantageous one; the
project does whatever the workload calls for.

The repository already holds both halves of the answer, and they pull against each other:

* `CLAUDE.md:526-543` says the file may carry more than one form and the reader choose, because the page
  cache is charged for the pages a path touches and not for the file's size, so a form the chosen path
  never reads costs disk and the time to write it. Two rules keep it honest: a second form may only be one
  the other paths never touch, and the choice is made from what the store measures, built only when that
  measure calls for it, and taken back when the measure turns. Its last sentence names the first version
  of the note as the one that priced the file by its size.
* `src/ordindex.rs:67-77` and `91-107` record what carrying two shapes and routing *each operation* to the better
  one does: the cliff of each shape sits at its own footprint, so two shapes move the cliff to their sum;
  a pair collapsed 400x at a cap where either alone held, and "one point read in two hundred costs 7x".
  Its conclusion: a second shape is selected per *workload* and never per operation, and "anything that
  beats this has to REPLACE the record rather than accompany it".

So "opportunistic" is read here, and defended in sections 4 and 5, as follows. The choice is per workload
and, for coverage, per partition: when the store is in the compact form, *every* read, count and scan of
a covered partition is served from the z form and none from the raw form, except at the sites 3.9 lists as
raw on purpose, each of them bounded, counted, and visible to the "z is not paying" exit of 4.5 (the
instrument that proves it is `raw_touches`, section 6; three scan paths the first version of this document
left raw by omission are in the table now: rev 1.1, rev 2.5). The only per-operation choice is for operations
whose pages are not kept. The build reads the raw segment once, through a mapping of its own and a descriptor
it drops behind itself (5.8), and so adds nothing to anyone's working set. The merge and the open do not drop
what they read -- `src/` has no `fadvise`, `DONTNEED` or `MADV_PAGEOUT` anywhere (grep) -- and leave their raw
pages to the LRU, which is today's behaviour, priced in 5.8 and R19 and not claimed away (rev 1.13). Opportunism
in *time* is the rest of it: the build runs on idle CPU and idle I/O priority, the form is kept after the workload
turns for as long as a re-entry is likelier than not to be cheaper than a rebuild, and `warm` fills headroom the
cache really has, on the build's own thread and never on the segment work's (5.4).

What "reversible" is made to mean (the ledger is section 5.9, and every row has a test):

1. The raw segment, its ordered index, the format, its magic and the browser reader are never touched.
   Deleting the sidecar files and setting `ZPages::Off` is the whole of the undo.
2. Switching form is a store-level word and a per-partition pointer; both forms answer every read
   identically, so a switch at any instant cannot change an answer, only its price.
3. Disk is returned: the sidecar's mapping is dropped through the epoch (`RetireList`, swept wherever the
   other retire lists are, 4.8), the file is renamed to a unique dead name when its cell is retired and
   unlinked on an idle tick (5.6), so `bytes_on_disk_per_byte` after a take-back equals a store that never
   had it.
4. What is *not* reversible is stated, not hidden: the CPU and device bytes a build already spent, and the
   page-cache displacement a build causes, which a cancelled build wastes (5.9). The design bounds them (one
   build per partition per entry, cancellable, niced, retained across a flap, and a budget for builds a merge
   supersedes before they install: 5.2) and section 7 prices them.

Where the sentence the request quotes stands, at one probe's framing. "Extra space costs only disk"
is true of the sidecar: the 0.68 GB it adds is a file no read touches until the form is chosen, and in the form
it replaces the 1.37 GB the probe's raw reads touched (hash and records; 1.49 GB in the engine, 9.1) with 0.68 GB. "Past the memory line it becomes the
miss rate" is true of a form that *accompanies* the one the reads use, and false of one that *replaces* it for the
reads: at 10M keys under 1 GiB the probe read the raw layout at 0.42 faults a read and the compact one at none from
the second pass (M17). That is evidence from a standalone probe with fixed records, one thread and uniform reads,
and 7.2a point 4 lists what it did not touch; it is the only measurement here that bears on the request, and it
bears the way the request suspected. It bears inside a window (rev 3.4): a form that replaces the records by a
0.55 fraction of their bytes removes the miss rate only while it fits, and D9 and 7.2b give where it wins over
LMDB (a store of about one to 2.6 times the cache in records, for the 1.5x of C4), where it ties LMDB (about seven
times) and what it is past that: a tenth behind LMDB and 1.75x ahead of the raw form, whose hash stops fitting too.
Every design with one fault a read ties on faults there, and a decode is pure cost.

## 1. CELL and DECOMPOSITION

### 1.1 The cell

Out-of-core point reads and scans of the durable arm, 10,000,000 keys of sixteen decimal digits with
100-byte values of which half is a repeated phrase (`bench/src/workload.rs:200-231`), under a 1 GiB
cgroup v1 bound (`bench/src/bound.rs`), arm `supdb-nocache` (the default policy and `supdb-cache256` are
killed under the bound at 3M keys by the forms' memory, `docs/engine.md:5920-5928`, and are not this
move's problem; section 5.8). Quantities: for `read`, `read_2t`, `read_3t`: `reads_per_s`,
`majfaults_per_op`, `read_bytes_per_op`, `refaults_per_op`; for `scan` and its threaded cells:
`entries_per_s` and the same counts; the six YCSB mixes; the load's `ops_per_s`,
`bytes_on_disk_per_byte`, `device_bytes_per_byte`. Controls: the same arms at 3M keys under the same bound,
which is in core (every read and scan cell prints 0 B / 0 mf / 0 rf), and the quick row, which is in memory.

### 1.2 Measured [M]

| id | quantity | value | source |
|---|---|---|---|
| M1 | store at 10M, `supdb-nocache` | 1.3036 bytes on disk per byte stored (116 B/key stored, so about 1.51 GB); device bytes 1.3393 | `scratchpad/ooc/rung1-10m/ooc/20261018T010652Z-ca738ec.json`, banked rep |
| M2 | hash / records / directory | about 190 MB (8-byte slots, power of two, 19 B/key at this partition size) / about 1.3 GB (126 B/key) / 4 B/key | task brief; `docs/engine.md:5974-5988`; `src/flatindex.rs:90-137` |
| M3 | cache the bound leaves | about 937 MB; the process holds `rss 0.10 GB` | task brief; `scratchpad/ooc/rung1.log` pass lines |
| M4 | `supdb-nocache` point read, 1t / 2t / 3t | 53,778/s, faults per read 0.413 / 0.370 / 0.361, 1,694 B per read (warmup rep); banked rep 53,642/s, 0.463 / 0.421 / 0.426; 3t 147k/s (banked 132k/s) | `rung1.log` lines "442s" and "4465s": 1,000,000 reads, `413482mf`, `740657mf`/2M, `1082479mf`/3M |
| M5 | `lmdb-nordahead` point read | 82,005/s (banked 78,897/s), 0.274 / 0.263 / 0.267 faults per read, 1,122 B per read; 3t 195k/s (banked 192k/s) | `rung1.log` lines "3968s" and "7983s" |
| M6 | `lmdb` as a user gets it | 33,246/s, 0.267 faults per read, 19,809 B per read (74 KB a fault) | `rung1.log` line "2680s" |
| M7 | scan of 100 entries, `supdb-nocache` | 797k entries/s (banked 763k), 2.87 (3.04) faults per scan on one thread, 4.08M (3.56M) entries/s on three | `rung1.log` |
| M8 | scan, `lmdb-nordahead` | 2.03M (2.06M) entries/s, 1.11 faults per scan, 3t 4.82M (4.68M) | `rung1.log`, JSON |
| M9 | YCSB mixes, banked rep, `supdb-nocache` against `lmdb-nordahead` (A, B, C, E, F Zipfian with theta 0.99, D uniform: `bench/src/run.rs:297-306`; run order C B A F D E, `:308`) | ops/s: A 224k / 64k, B 174k / 193k, C 107k / 239k, D 42k / 87k, E 49k / 138k, F 146k / 67k; faults per operation in C 0.215 / 0.075, in E 0.285 / 0.121 | `docs/engine.md:6004-6032` (the banked-rung table), JSON, `readrow.py` |
| M10 | the load | `supdb-nocache` 1.35-1.51M ops/s; `lmdb-nordahead` 0.92-0.94M | `rung1.log` |
| M11 | in-core control, 3M under the same bound | `supdb-nocache` read 1.37M/s (0.73 us), scan 30.3M entries/s, ycsb-E 169k ops/s; `lmdb-nordahead` 0.79M/s, 20.8M entries/s, 489k | `rung1-3m/ooc/20261018T005338Z-ca738ec.json` |
| M12 | seek floor, one random page of a cold file | p50 36-38 us, p99 76-88 us | `rung1.log` floors lines |
| M13 | offline, Python modules, 200,000 records | the suite's records compress to 0.57 under LZ4 in blocks of 4-8 KiB, 0.53-0.55 under zlib level 1, 1.0 one value at a time; LZ4 compresses about 0.6 GB/s and decompresses a block in 2-3 us | `docs/engine.md:5989-5999` (which itself calls what follows from it "arithmetic, not a measurement") |
| M14 | standalone probe `zprobe`, 200k keys, no bound, niced, fixed 116-byte records, 8-byte page header | raw pages 117.0 B/key; z pages 68.3 B/key at 60.0 records per page; in-memory read raw 0.307 us, z 3.112 us (whole-page decompress every read) | `scratchpad/ooc/zprobe-notes.md`; source `scratchpad/zprobe/src/main.rs` (read, not run) |
| M15 | `rocksdb-tuned`, uncompressed (`bench/src/engines.rs:4415`, 256 MB block cache `:4418`) | 3M, in core: bytes on disk 1.0103 per byte, device bytes 1.678, reads 239-252k/s. **10M under 1 GiB, banked rep:** reads 37.3k / 62.5k / 75.4k per s (1t / 2t / 3t), 0 major faults (it reads with `pread`) and 0.705 refaults, 2,889 B per read; scans 0.85M / 1.52M / 2.03M entries/s; mixes A 133k, B 110k, C 88.7k, D 26.6k, E 8.3k, F 78.2k ops/s; load 954k ops/s, shuffled 290k; bytes on disk 1.0022, device bytes 1.986 | `scratchpad/ooc/rung2.log`, `rung2-3m/ooc/20261018T032126Z-ca738ec.json`, `rung2-10m/ooc/20261018T032333Z-ca738ec.json` |
| M16 | the standalone probe's 10M build (116-byte fixed records, 8-byte page header) | `raw.bin` 1,170,288,640 B (117.03 B/key), `z.bin` 679,784,448 B (165,963 pages, 60.25 records a page, 68.0 B/key), `hash.bin` 199,229,440 B, fences 5.7 MB (raw) and 3.3 MB (z); its first passes, raw layout with the hash file, one thread, 1M reads: pass 0 46.1k/s, 0.532 faults; pass 1 54.1k/s, 0.423; pass 2 56.1k/s, 0.422 | file sizes in `scratchpad/tmp-price/zp10` at 03:56; pass lines as read from the run's task output at 03:57 (that output file is no longer on disk [U]) |
| M17 | **the same probe's result at 10M under the bound, which arrived after section 7 was written** (its author's pre-registration is `scratchpad/zprobe/prereg.md`, with the result appended and dated 04:00 UTC): a 1 GiB cgroup, 100 MB of anonymous ballast, 1M uniform reads a pass, four passes, one thread, the probe's files dropped from the cache first | **raw pages and the 199 MB hash file** (the engine's shape): pass 0 0.532 faults/read at 46.1k/s (M16); passes 1-3 0.423 / 0.422 / 0.414 at 54-58k/s. **raw pages, no hash file**, a 5.7 MB page fence (L2's footprint): passes 1-3 0.182 / 0.182 / 0.182 at 113-117k/s. **z pages, no hash file**, a 3.3 MB fence: pass 0 0.166 faults/read at 96.7k/s; passes 1-3 0.000 at 262k / 280k / 264k reads/s, 3.6-3.8 us a read. The probe's own caveats: fixed 116-byte records, a binary search in the page by position (no rank), one thread, no overlay, tombstones or multi-value keys, half-compressible values | `scratchpad/zprobe/prereg.md`, block "RESULT (04:00 UTC)"; the per-pass output is not on disk, so the no-hash raw layout's pass 0 and any line beyond those the block lists are not known [U]; the probe, its logs and the 10M rows exist in a session scratchpad only, and banking them is stage 0a (rev 3.8) |

### 1.3 Derived [D]

| id | quantity | formula and value |
|---|---|---|
| D1 | cost of a major fault | `t_f = (1/rate - t_hit) / faults`: supdb `(18.6 - 0.73)/0.413 = 43 us`; LMDB-nordahead `(12.2 - 1.27)/0.274 = 40 us`. Use 41.5 us (40-43). Consistent with M12 plus the fault path |
| D2 | the footprint model | faults per read `phi = 1 - (C - H)/R` for a path that touches the hash (H, always resident) and one record page (R): `1 - (937-190)/1300 = 0.425` against M4 0.413-0.463. LMDB `1 - (937-10)/1330 = 0.30` against M5 0.267-0.274. [P2] The hash stays resident only while it fits: Che's approximation to LRU over the two populations (4.2) reproduces the cell, 0.428 against 0.425, and gives the hash its own misses from about twice the cache on, where the one-population form saturates at 1 (D9; rev 2.2) |
| D3 | where the 6.4 us gap to LMDB sits | `(0.413 - 0.274) * 41.5 = 5.8 us` of it is miss rate (90%); the rest is CPU. The cell is a *footprint* loss, and a CPU lever cannot close it |
| D4 | how much of the footprint is the hash | `H/R = 190/1300 = 0.146` faults per read at most. `docs/engine.md:5998-6000` calls this "the smaller lever"; it is. **[After the result, 7.2a: M17 measured the hash worth 0.241 faults a read at the probe's R, against 0.170 by this formula, so "at most" was wrong.]** The 0.07 residual is unexplained; eviction of hash pages is the likeliest cause [U], and stage 1a measures the hash file's residency and the faults by region (rev 3.2). |
| D5 | break-even **saving** for a form that costs more CPU | `saving* = (t_z - t_hit)/t_f`: the fall in faults per read that the extra CPU must be repaid by. From M14 alone `(3.112 - 0.307)/43 = 0.065`. In the engine the z read is the raw read's fixed cost plus the probe's difference plus the fence, the filter and the record walk: `t_z = 0.73 + 2.8 + 0.5 = 4.0 us` [P], so `saving* = (4.0 - 0.73)/41.5 = 0.079` (0.065-0.09). [P2] The engine's `t_z` is 4.0-4.7 us, central 4.4, and `saving*` 0.079-0.096: the probe's 3.6-3.8 us ran the engine's own codec (the unsafe decoder: 7.2a, point 3), so it is no upper bound on decode; the engine walks a group's records where the probe searched fixed records by position (+0.2-0.5 us); and the raw read's fixed cost is 0.42 us above the probe's (rev 3.14). The first version called this a fault *rate* and assumed the form removes every fault (`phi_z = 0`), which holds at the suite's cell and not past D9's window, so the entry test is on the predicted saving `phi_raw - phi_z` (4.4; rev 2.2) |
| D6 | the z form's size | M14's 68.3 B/key at 60.0 records per 4,088-byte payload; the engine's records add eight framing bytes each (`klen`, the compact word, the run length and count: `src/flatindex.rs:685-705`), which repeat verbatim from record to record and cost a byte or two compressed, and the page header is 24 bytes where the probe's was 8 (16 bytes a page, 0.3 B/key): about 71 B/key, 57 records per 4,072-byte payload [P]. 10M keys: 175k pages, 0.72 GB |
| D7 | cold-file arithmetic | reads `r` against a file of `P` pages touch `P(1 - e^(-r/P))` pages once each, a major fault each. With `P = 175k`: `r = 1M` gives 0.174 faults/read (0.1656 at the probe's 165,963 pages, where M17 measured 0.166; the first version's 0.1725 was a 173k-page figure); `r = 333k` (the default `--ops-div 30` cell) gives 0.447. A cold z file is as bad as the raw file in the default divided cell, which is why `warm` (section 5.4) is part of the move and not a tuning |
| D8 | the ladder the model predicts at 10M, 1 GiB | raw with hash 0.425; point reads through the ordered index (L1) `1 - (937-120)/1300 = 0.372`; page fence over the raw records, no hash (L2) `1 - (937-6)/1300 = 0.284`; z pages (L3) `W = 0.72 + 0.02 GB < 0.92 GB`, steady 0. LMDB-nordahead is at 0.274, so L2 would tie it and only L3 can beat it |
| D9 | the fit window (rev 3.4, rev 2.2) | the z form beats LMDB and the raw form only while the records are not much larger than the cache, and past that it trails LMDB by a tenth and still leads the raw form: the model and the table follow the rows |
| D10 | the stop thresholds (rev 1.12, rev 2.13, rev 3.14) | records a page at which one thread ties `lmdb-nordahead`: 35.7; at which C4's 1.5x fails: 40.1; at which the scan claim's 2x fails: 39.3, or 43.1 with the heads resident; derivation after the table |

**D9 in numbers [D].** For `N` keys: records and directory `R = 130 N` bytes, hash `H = 19 N`, z pages
`W_z = 71.9 N` (57 records a page), resident part `f = 1.74 N` (the Bloom filter is 1.25 B a key, so it grows with the
store), LMDB's file `133 N`, cache `B = 937 MB`. The z form is one population read uniformly:
`phi_z = clamp(1 - (B - f)/W_z)`, LMDB's `phi_L = clamp(1 - (B - 10)/133 N)`. The raw form reads one hash page and
one record page per read, uniformly, so its misses are Che's approximation to LRU: `T` solves
`n_h (1 - e^(-T/n_h)) + n_r (1 - e^(-T/n_r)) = B/4096` with `n_h = H/4096`, `n_r = R/4096`, and
`phi_raw = e^(-T/n_h) + e^(-T/n_r)`, which exceeds 1 once the hash stops fitting (the first version saturated it at
1, and its x_max of 14 to 21 times the cache followed from that cap). Read times are `0.73 + 41.5 phi_raw`,
`4.0 + 41.5 phi_z` and `1.27 + 40 phi_L`.

| keys | records / cache | `phi` raw / z / LMDB | z over raw | z over LMDB | saving `phi_raw - phi_z` |
|---|---|---|---|---|---|
| 10M (the cell) | 1.4 | 0.43 / 0.00 / 0.30 | 4.6x | 3.3x | 0.43 |
| 12.7M (z just fits) | 1.8 | 0.60 / 0.00 / 0.45 | 6.4x | 4.8x | 0.60 |
| 15M | 2.1 | 0.71 / 0.16 / 0.54 | 2.9x | 2.2x | 0.56 |
| 18.7M | 2.6 | 0.87 / 0.33 / 0.63 | 2.1x | **1.5x** | 0.54 |
| 30M | 4.2 | 1.20 / 0.59 / 0.77 | 1.8x | 1.1x | 0.61 |
| 50M | 6.9 | 1.48 / 0.76 / 0.86 | 1.75x | **1.0x** | 0.72 |
| 90M | 12.5 | 1.70 / 0.88 / 0.92 | 1.76x | 0.94x | 0.82 |
| 200M | 27.7 | 1.86 / 0.96 / 0.97 | 1.8x | 0.91x | 0.90 |

The model's LMDB at the cell is 0.30 faults against the 0.274 measured (M5), so its z-over-LMDB column reads
about 10% high there; the registered cell ratios of 7.3 use the measured LMDB. Three consequences. (1) C4's 1.5x
over LMDB holds up to about 18.7M keys (2.6 times the cache) at 57 records a page, 15.3M at 50 and 12.9M at 45, and
`t_z` of 4.4 and 4.7 us move the edge to 18.3M and 18.0M keys. Past about 50M keys one thread is a tenth behind LMDB,
whose read is one fault and 1.3 us where this one is one fault and 4.4: the goal "everywhere but disk" is not reached
past the window by any single-fault form that decodes (open question 1). (2) The z form stays well ahead of the raw
form at any size, because the raw form takes up to two faults a read once the hash does not fit, so the saving the
controller tests (4.4) does not fall under its threshold as the store grows. The first version of this section
tested the raw model alone and would have built nothing past one fault a read; the reviews' x_max does not survive
the hash's own misses (Review record 2.2). (3) Where the saving does fall under `1.5 saving*` is when the form does
not compress (`W_z` close to `R`), when the hot set fits, or when the cache grows: the cases the gate and the exits
are for.

**D10 [D].** One thread beats `lmdb-nordahead`'s 12.2 us a read while `4.0 + 41.5 phi_z <= 12.2`, i.e.
`phi_z <= 0.198`, i.e. `W_z <= (B - f)/(1 - phi_z) = 0.92/0.802 = 1.147 GB`, i.e. 279.9k pages and **35.7 records a
page** at 10M keys. The first version wrote `W_z <= 1.15 B_z`, which is 38.6; `1/(1 - 0.2)` is 1.25, and 38.6 is
the figure for a `B - f` of 0.84 GB (heads resident). C4's 1.5x (8.1 us) needs `phi_z <= 0.0995`, 40.1 records a
page. For scans, 2x LMDB's 49 us per hundred entries (2.03M entries/s) is 24.7 us, 11.3 of it the resident scan,
so at most 0.32 faults a scan over 2.74 pages, `phi_z <= 0.117`: 39.3 records a page, or 43.1 with the 80 MB of
heads resident, which is the first version's design (3.4, rev 2.5).

### 1.4 What this move claims [P] (numbers in section 7)

C1 footprint: the z form's working set is half of the raw form's (0.74 GB against 1.49 GB) and fits the
cache with 0.18-0.21 GB over, at 57 records a page and the suite's sixteen sequential digits for a key; at 50
records a page (sixteen random bytes of key, 7.1) it leaves 0.1 GB over and at 45 none (rev 2.7). C2 faults: after
the file is resident, a point read takes at most 0.03 faults and a scan at most 0.3; the first cell, which carries
the cold fill unless `warm` has run, at most 0.10. The verdict's refutation line is 0.05 at three threads, a margin
over the prediction and not a second prediction (rev 3.14). C3 CPU: a z point read costs 4.0-4.7 us where a raw hit
costs 0.73 us [P2: D5], a scan of 100 about 11 us. C4 rates, at this cell, where the records are 1.4 times the
cache: from the second read cell on, point reads beat `lmdb-nordahead`'s by 1.5x or more on one thread and 2x on
three, scans by 2x; D9 and 7.2b give the window around the cell and the claim stops there. C5 price: +0.63
bytes on disk per byte stored, 6-14 s of build CPU and 0.72 GB written at 10M, all of it reversible. C6 no harm:
nothing moves in memory (a branch of three instructions, no thread, no file, no sample), and the 3M control, which
is in core, is not routed to the z form. The claims that are *not* made: that point reads in the Zipfian YCSB
mixes beat LMDB without a decoded-group memo (section 3.7); that the move reaches "everywhere but disk" past
D9's window; and anything for key shapes with less redundancy than the suite's, which 7.1 prices before the
engine is touched.

### 1.5 The change, in one paragraph

A derived sidecar `zpg-<id>-<end_seq>.zpg` per partition holds the partition's records, verbatim, in
groups that each compress into one 4 KiB page, with a resident fence of first keys and ranks and a
resident Bloom filter bound to the code that builds it (sections 2-3). It is built on a niced thread when the
selector's measure calls for it (4) and read through a mapping of its own; the raw segment is untouched (sections 4-5). Both forms answer
identically, which is what lets the choice be per workload, reversible and wrong without being unsafe.

## 2. FORMAT, BUILDER, INTEGRITY

### 2.0 Choices, and what each is for

1. **A sidecar file per partition, not a region of the segment.** The segment's format, its magic
   (`src/format.rs:34`, 0007) and the browser reader stay as they are; the rule is the ordered index's
   (`src/ordindex.rs:163-165`, "a companion file rather than a region of the segment because that leaves
   the segment format, and the browser reader over it, untouched"). It is also what makes deletion a
   complete undo.
2. **Verbatim record images, not a re-encoding.** A group is the concatenation of the bytes
   `parse_record` consumes for each record, in rank order, each image 4-aligned (`src/flatindex.rs:502-541`
   returns the image's length, padding included, as its fourth element). One decoder then serves every
   read, count and scan -- `parse_record` -- and no second definition of the layout exists (`CLAUDE.md`,
   "A size chosen before the thing is measured": a second copy of that arithmetic is a second definition
   of the format). A re-encoding through `stream_record_as` (`src/flatindex.rs:807-815`) could be exact for
   writer-made extents only, and could turn a full-form record into a compact one.
3. **A page is the unit of residency.** One page, 4,096 bytes at a 4,096-aligned file offset, is one fault
   and one cache page. The fence names a page; the page names its own ranks.
4. **The resident part is at the tail and the trailer is the last page**, so the builder streams pages from
   offset zero without knowing how many there will be and never patches an earlier byte (a patch re-dirties
   its whole folio and `/proc/self/io` counts it again in full, `src/db.rs:2667-2676`, `CLAUDE.md` "device
   bytes").
5. **Identity is content, not a name** (2.5): the engine hands segment ids out again after a crash
   (`src/db.rs:18997`) and the ordered index binds to its segment by key count alone
   (`src/ordindex.rs:417-419`), which is enough for heads and not for a file that carries values.
6. **One owner for each piece of arithmetic a second reader would restate** (rev 1.2, rev 1.15). The compact
   record's length is `flatindex::record_len_compact` (`src/flatindex.rs:703`), which `record_key_next` calls and does
   not restate; the Bloom filter's bit layout is `BlockedBloom`'s alone, and the sidecar stores a filter only with a
   signature that changes whenever that layout does (2.3, 2.5).

### 2.1 File layout

All integers little-endian. The file name is `zpg-{id:08}-{end_seq:016}.zpg`, the temp name the same with
`.tmp`, built from the two name fields a promotion keeps (`Db::name_field`, `src/db.rs:18513-18518`; the
ordered index's `Db::ord_name`, `src/db.rs:18525-18531`, is the model), so a promotion's hard link names
no new file. The file length is a multiple of 4,096.

| region | offset | length | contents |
|---|---|---|---|
| A, pages | `4096 * p` | `4096 * P` | `P` z-pages (2.2); page `p` at file offset `4096 * p` |
| B, resident part | `res_off = 4096 * P` | `res_len`, padded with zeros to a multiple of 4096 | B1 `first_rank[E]` as u32; B2 `page_no[E]` as u32 (`0xFFFF_FFFF` = `ZNOPAGE`, a record the form does not hold); B3 `key_off[E+1]` as u32, `key_off[0] = 0`, the last the length of B4; B4 the key blob (entry `i`'s key is `B4[key_off[i]..key_off[i+1]]`); pad to 8; B5 the filter, `filter_blocks * 64` bytes, each block eight u64 words: the layout of `BlockedBloom::blocks` (`src/db.rs:2367-2370`), valid only under the trailer's `bloom_sig` (2.3, 2.5) |
| C, trailer | `file_len - 4096` | 4096 | the trailer (2.3) |

`E` entries cover the partition's ranks: entry `i` covers `[first_rank[i], first_rank[i+1])`, the last
entry up to `nkeys`. An entry with a page names the page that holds exactly those records; an entry with
`ZNOPAGE` names one or more consecutive records the form does not hold (a record that cannot fit a page
once compressed, 2.6), and every read of them is answered from the raw segment and counted. In the suite
`E = P` and no entry is `ZNOPAGE`.

Resident bytes at 10M keys [D]: B1+B2 `175k * 8 = 1.4 MB`, B3 0.7 MB, B4 `175k * 16 = 2.8 MB`, B5
`10M * 10 bits / 8 = 12.5 MB` (`BlockedBloom::with_capacity`, `src/db.rs:2429-2433`): 17.4 MB, 1.74 B/key.

### 2.2 The page

| offset | size | field |
|---|---|---|
| 0 | 4 | `crc`: CRC32C (`block::crc32`, `src/block.rs:116`; hardware where the CPU has it) over bytes `[4, 4096)` of the page, padding included, **XOR the trailer's `bind32`** (2.3): a page copied in from another sidecar, or from the same partition's sidecar of another generation, fails its own check at no cost (rev 1.15) |
| 4 | 4 | `tag` = `"ZPG1"` (`0x3147_505A`), never zero: a zeroed page cannot pass |
| 8 | 4 | `first_rank` (u32) |
| 12 | 2 | `count` (u16, 1..=256 = `ZMAX_COUNT`) |
| 14 | 1 | `flags`: bit 0 `STORED` (the payload is the group verbatim, `clen == ulen`); the rest zero |
| 15 | 1 | zero |
| 16 | 4 | `ulen` (u32): bytes of the group, a multiple of 4, at most 65,536 = `ZMAX_ULEN` |
| 20 | 2 | `clen` (u16): payload bytes, at most `ZCAP = 4096 - 24 = 4072` |
| 22 | 2 | zero |
| 24 | `clen` | the payload: an LZ4 block (`lz4_flex::block::compress` output, no size prefix, no frame), or the group verbatim if `STORED` |
| 24 + `clen` | rest | zeros |

The payload starts at offset 24, which is 8-aligned in a 4,096-aligned page of an aligned mapping, so a
`STORED` page is parsed in place with the 4-alignment `parse_record` demands for a full-form record's
extent array (`src/flatindex.rs:520-523`; a misaligned buffer makes it return `None`, which a probe loop
reads as "absent": the silent-miss hazard, risk R6). A decoded group goes to a buffer whose storage is
`u64` words, the shape `SparseBlob::dictionary_walk` already uses for a copied span
(`src/blob.rs:2504-2515`).

LZ4 is used exactly as `src/block.rs:330-347` uses it, with two differences the crate's source forces.
The decoder stops only when its input is exhausted after a literal run, and trailing bytes are parsed as
further sequences (`lz4_flex-0.11.6/src/block/decompress.rs:364-368`, `161-173`), so the page carries
`clen` and the reader hands the decoder exactly `clen` bytes; and `block::decompress_into` discards the
decoded length (`src/block.rs:339-347`) while the thread's scratch is reused without zeroing, so the z
reader calls `lz4_flex::block::decompress_into` itself and requires the returned length to equal `ulen`.
The engine builds the crate with `default-features = false, features = ["std", "frame"]` (`Cargo.toml:15`), so the unsafe
codec is the one in use (`safe-decode` and `safe-encode` are defaults of `lz4_flex-0.11.6`'s `[features]` and are off), as it was in the
standalone probe (7.2a, point 3); the CRC is checked before any decode because LZ4 can decode corrupt input into plausible
bytes (`src/block.rs:10-13`), and the unchecked decoder is never handed a page whose CRC failed.

### 2.3 The trailer (the last 4,096 bytes)

| offset | size | field |
|---|---|---|
| 0 | 8 | magic `"SUPDZPG1"`; the digit moves whenever an older reader would misread rather than refuse |
| 8 | 4 | `version` = 1 |
| 12 | 4 | `page_size` = 4096; a reader refuses any other |
| 16 | 8 | `n_pages` (`P`) |
| 24 | 8 | `n_entries` (`E`) |
| 32 | 8 | `nkeys`: the segment's key count |
| 40 | 8 | `res_off` (must equal `4096 * P`) |
| 48 | 8 | `res_len` |
| 56 | 8 | `keys_len` (the length of B4) |
| 64 | 8 | `filter_off` (within region B) |
| 72 | 8 | `filter_blocks` |
| 80 | 4 | `res_crc`: CRC32C over region B `[0, res_len)` |
| 84 | 4 | `flags`: bit 0 filter present; bit 1 reserved for a fence-only kind (section 9, L2); a version-1 reader refuses a file with any other bit set |
| 88 | 8 | `total_ulen`: the sum of the groups' bytes, for the ratio |
| 96 | 8 | `seg_recs_len` |
| 104 | 8 | `seg_crc_off` |
| 112 | 8 | `seg_key_off` |
| 120 | 8 | `seg_hash_cap` |
| 128 | 8 | `seg_file_len` |
| 136 | 8 | `seg_timestamp` (the superblock's, `Blob::version().1`) |
| 144 | 8 | `seg_generation` (`Blob::version().0`) |
| 152 | 4 | `seg_row_crc`: CRC32C over the segment's key-section checksum row (2.5) |
| 156 | 4 | `rec_version`: `flatindex::VERSION` (3, `src/flatindex.rs:43`) |
| 160 | 8 | `rec_magic`: `format::MAGIC` (`src/format.rs:34`) |
| 168 | 4 | `page_cap` (4072) |
| 172 | 4 | `max_count` (256) |
| 176 | 8 | `seg_id`: the name's id field (`Db::name_id`, `src/db.rs:18533`) |
| 184 | 8 | `seg_end_seq`: the name's covered end-sequence field (`Db::name_end_seq`, `src/db.rs:18537`) |
| 192 | 4 | `bind32`: CRC32C over `seg_id`, `seg_end_seq`, `nkeys` and `seg_row_crc`, 28 bytes little-endian; every page's `crc` is XORed with it (2.2) |
| 196 | 4 | `bloom_probes` (4) |
| 200 | 8 | `bloom_sig`: `BLOOM_SIG`, the constant beside `BlockedBloom` that changes with its hash, finalizer, block choice or probe walk (2.5) |
| 208 | 4 | `bloom_bits_per_key` (10) |
| 212 | 3880 | zero |
| 4092 | 4 | `trailer_crc`: CRC32C over `[0, 4092)` |

### 2.4 In memory

```rust
// src/db/zpages.rs (a child module of db.rs: it reaches BlockedBloom, err, start_writeback,
// idle_io_priority, idle_cpu_priority and Seg without widening any visibility; db is already
// cfg(not(wasm)), src/lib.rs:68-69, so the module is excluded from the wasm build by construction;
// every item that touches the OS is cfg(target_os = "linux") with a no-op elsewhere: 2.9)
pub(super) struct ZSide {
    map: MmapBytes,                 // the sidecar, mapped read-only; its own advice
    advice_random: AtomicBool,      // set at attach from the store's current advice and flipped with it (rev 1.15)
    fence: ZFence,                  // heap copies of B1..B4: anonymous, never evicted (swap is off here)
    filter: BlockedBloom,           // heap copy of B5, valid only under the trailer's bloom_sig (2.5)
    nkeys: usize, n_pages: usize,
    verified: Box<[AtomicU64]>,     // one bit per page: CRC and structural walk passed in this process
    bad: Box<[AtomicU64]>,          // one bit per page: failed
    damaged: AtomicBool,            // any failure: the whole sidecar leaves service
    id: u64,                        // minted at attach from a global counter, as Blob::id (src/blob.rs:758-760)
    ident: ZIdent,                  // the trailer's binding fields, bind32 among them
}
struct ZFence { first_rank: Vec<u32>, page: Vec<u32>, key_off: Vec<u32>, keys: Vec<u8> }
pub(super) struct ZCell { p: AtomicPtr<ZSide> }   // Seg::z; an Arc<ZSide> through into_raw/from_raw
// The per-read counters are not here: they live in ZStat, one 128-byte line per reader slot (6.1, 6.4).
```

`ZSide` holds no raw pointer and no cell a reader follows; the compile-time test that asks the compiler
`Send + Sync` of `State`, `Shared` and `Snapshot` (`src/db.rs:28116-28131`) gets `ZSide` added, because
an `AtomicPtr<ZSide>` in `Seg` would otherwise let a non-`Sync` type in unasked (`CLAUDE.md` "Invariants":
"a raw pointer behind an atomic would let one in unasked").

### 2.5 Binding a sidecar to its segment

No engine segment has a strong identity [D from code]: every one is finished with generation 1
(`src/db.rs:5312-5342`) and a timestamp in whole seconds (`src/db.rs:4958-4961`), the ordered
index checks a key count (`src/ordindex.rs:417-419`), and `next_seg` restarts at the largest live id plus
one at every open (`src/db.rs:18997`), so a swept orphan's id can name a different segment next time. A
sidecar holds *values*, so the binding is to the content: at attach the reader recomputes from the live
segment, and the trailer must equal, all of

`nkeys`, `recs_len`, `crc_off`, `key_off`, `hash_cap`, `file_len` (the superblock's `high_water`),
`timestamp`, `generation`, `rec_version`, `rec_magic`, `row_crc` (the CRC32C of the segment's key-section
checksum row), the name's `id` and `end_seq`, and `bind32` recomputed from them; and, for the filter,
`bloom_sig`, `bloom_probes` and `bloom_bits_per_key` equal to the running engine's.

The row is the strong part. Every segment carries one, unconditionally (`src/db.rs:4801`, `4871`;
`SegmentOptions::checksums` toggles the *block* checksums, `block::CHECKSUMS`, not this row: `src/db.rs:3791`,
`4551-4556`), one CRC32C per 16 KiB piece of the whole key section (header, records, fence, directory,
hash: `src/flatindex.rs:243-300`), so four bytes per 16 KiB are a fingerprint of every byte the records
live in, and reading it costs a few hundred kilobytes per gigabyte, not a pass over the file.

**The filter is a function of engine code, and the row cannot see that** (rev 1.2). `BlockedBloom`'s bits are a function of
`flatindex::key_hash` (the index's FNV, a format quantity: its change moves the magic), of `finalize` (two
multiplier constants and two shifts, `src/db.rs:2441-2444`), of the block choice `(h >> 32) % blocks.len()` and the
four-probe walk of `slots_of` (`src/db.rs:2481-2492`), and of `with_capacity`'s `n * 10 / 512` blocks
(`src/db.rs:2429-2433`). Only the first is format; an edit to any other moves no magic, no record version, no row CRC
and no trailer field, so an engine that changed `slots_of` (five probes, a new finalizer) would attach an old sidecar
whose trailer CRC, `res_crc`, binding and sampled page CRCs all pass, and every present key whose bits no longer line up
would test negative and be answered "absent" with no fence search and no counted fallback (3.2, step 1). That is a derived
thing whose function changed under it, and nothing raises. The comment at `src/db.rs:2438-2440` ("the filter is built at
open and never stored, so its hash is free to be the index's") is false once a filter is stored, and is corrected in the
same commit. Three defences, each cheap: (1) `BLOOM_SIG`, a constant beside `BlockedBloom` bumped with any of the
above, stored in the trailer and compared at attach; (2) a unit test that builds a filter over 4,096 fixed keys and
compares a CRC32C of its words with a literal, so an edit to `BlockedBloom` fails a test whose message says to bump
`BLOOM_SIG` and the sidecar `version`; (3) at attach every fence key (a real key of the partition: 7.6k a partition at
10M, under a millisecond) must test positive in the filter, which a mismatched layout fails with certainty (a present
key tests positive under a wrong layout at the false-positive rate, 1.26%, so 7.6k keys all passing is `0.0126^7600`).

Two accessors are new, both `#[cfg(not(target_family = "wasm"))]` in the place the existing native-only
ones sit (`src/blob.rs:882-951`, `prefetch_ranks` and its kin, which call `crate::db::prefetch_lines`):
`Blob::record_regions(&self) -> Option<(&[u8], &[u8])>` (records, directory; `Blob::flat` is private,
`src/blob.rs:723`, though `FlatIndex::regions` is public, `src/flatindex.rs:1908`) and
`Blob::z_source_facts(&self) -> Option<ZSource>` returning the fields above. `Blob` has no field added and
no function the wasm ABI reaches changes (`src/wasmapi.rs:333-348`, `445-801` reach `lookup`, `count`,
`stored_bytes`, `read_concat`, `scan_counts` and the dictionary walks).

### 2.6 The builder

`ZBuild::run(seg_path, out_dir, ident, cancel) -> Result<ZOutcome>`, on its own thread (5.2).

1. **Open the input as a merge does** (`src/db.rs:8774-8786`), **on the segment-work thread, and hand the open `File`
   to the build thread at spawn** (rev 1.8). A promotion links the segment under its new name, publishes, and then
   unlinks the old name inline (`apply_promotion`, `src/db.rs:27409-27455`; `retire_seg`, `27130-27142`), so a build
   that opened by name after the unlink found `ENOENT` and counted a failed build; an open file survives the
   unlink. The thread maps the descriptor itself: its own `MmapBytes` and `Blob::open_with` with `verify_index:
   false` (the open that made the segment live already verified the row, and a promotion's re-open skips it for the
   same reason, `src/db.rs:27428-27438`), advice `Normal` so the kernel reads ahead on the sequential walk. The live
   mapping is `MADV_RANDOM` after the first point read (`Reader::advise`, `src/db.rs:13322-13338`), which would make
   this a page-at-a-time walk. Take `(recs, dir) = blob.record_regions()` and the facts.
2. **Sample and decide** (2.6.1) before committing the pass.
3. **Stream.** For each rank in order, `off = dir[rank]`, `(key, exts, tail, len) = parse_record(recs, off)`;
   the image is `recs[off .. off + len]`. The directory is the authority for where a record lies
   (`src/flatindex.rs:1964-1967`) and *never* the difference of two directory words, because a segment
   closed from a direct run keeps its 24-byte commit markers inside the record region, counted in
   `recs_len` and in every later directory word (`src/db.rs:1925-1943`, `3909-3949`, `4036-4060`); read as
   a record a marker is `klen = 0, n = 0`, a 4-byte "record" followed by garbage. [U: whether the suite's
   10M partitions are such segments; the builder handles both and a test builds both.] After each 8 MiB window of the record
   region and the directory the thread drops it behind itself (5.8).
4. **Fill a group** (`fill_group`, below), compress, keep it if it fits, emit the page, advance `rank` by
   the group's count, and put `(first_rank, page_no, key)` in the fence.
5. **Hash the keys into the filter** as they pass: `flatindex::key_hash` (FNV, the engine's one hash,
   `src/flatindex.rs:1528`) into `BlockedBloom::add_fnv` through the lookahead ring `BloomFill`
   (`src/db.rs:2378-2420`), 30 ns a key [D: 65 instructions, `CLAUDE.md` "A per-key cost written down from a
   guess"], 0.3 s at 10M.
6. **Verify every page before it is written**: decode the payload (or take the stored bytes), compare with
   the image bytes, require `ulen` and `count` to agree, **and run on the page as written the same
   `verify_page(page, fence_entry)` that a reader's first decode runs** (2.7: tag, `first_rank` and `count`
   against the fence entry, the structural walk to exactly `ulen`, the first key equal to the fence key; rev 1.7).
   A builder bug then fails the build, with a counter, instead of shipping a page with a good CRC and wrong
   bytes -- which no checksum downstream can see -- and a defect that would fail every first decode cannot pass
   the build and loop the rebuild. Cost: one decode and one compare a page, 0.6 s at 10M [D: 175k pages *
   (3 us + 0.3 us)].
7. **Write** through a plain `File` in `SegmentOptions::write_piece` (1 MiB) pieces at their own
   alignment, `start_writeback` on each piece (`src/db.rs:3228-3248`, a `sync_file_range` hint, no
   fsync), the resident part, the trailer last, then `rename` from the temp name. Unsynced, like the
   ordered index (`src/db.rs:54-86`, `Options::ord_durable` default off, `src/db.rs:1512`). The folio
   composition this makes in the page cache is [U]: the repository measured that a file written in 1 MiB
   pieces and read in 4 KiB pages composes differently from one written otherwise (`CLAUDE.md` "A write
   pattern priced on the bytes it wrote"); a `/proc/self/pagemap` + `/proc/kpageflags` walk of the built
   file at 1 MiB, 64 KiB and 4 KiB pieces is stage-2a work (the proof slice builds a file) and the piece is a named constant until then.

**`fill_group`.** `lz4_flex::block::compress_into` cannot test whether a group fits a page: it returns
`OutputTooSmall` whenever the output buffer is shorter than `get_maximum_output_size(input) = 20 +
input * 110/100`, whatever the data would compress to
(`lz4_flex-0.11.6/src/block/compress.rs:369-371`, `628-630`), so about 7 KB of input needs an 8 KB buffer
and a 4 KiB one cannot be used as the probe. The group is compressed into a right-sized `Vec` and its
length compared with `ZCAP`, as `block::compress` does (`src/block.rs:330-337`). The loop predicts, then
verifies:

```
m = clamp(previous page's count, 1, min(ZMAX_COUNT, n - rank))        // start where the last page ended
grown = false; grow_rounds = 0; best = none
loop at most 12 turns (i = 1, 2, ...):                      // m is 1 by the twelfth turn at the latest
    image = images rank..rank+m (appended incrementally; a shrink truncates at a record boundary)
    if len(image) > ZMAX_ULEN:
        if m == 1: return NOPAGE                           // one record over the decoded-size cap
        m = max(1, m / 2); continue
    c = lz4::compress(image)
    if len(c) >= len(image) and len(image) <= ZCAP:
        body = STORED(image)                               // LZ4 did not help and the image fits as it is:
                                                           // LZ4's own expansion is never a reason to shrink
    elif len(c) > ZCAP:
        if grown: return best                              // the growth overshot: keep the page that fit
        if m == 1: return NOPAGE                           // this one record cannot fit a page
        if i <= 4: m = max(1, m - max(1, ceil((len(c) - ZCAP) * m / len(c))))   // proportional shrink, never to zero
        else:      m = max(1, m / 2)                       // from the fifth turn on halve, so the loop ends
        continue
    else:
        body = LZ4(c)
    best = (m, body)
    slack = ZCAP - len(body);  per = max(1, len(body) / m)
    if grow_rounds < 1 and m < ZMAX_COUNT and rank + m < n and slack >= per:
        m += slack / per;  grow_rounds += 1;  grown = true;  continue   // one growth call: fill to the edge
    return best
```

Expected compress calls a page: one when the previous page's count fits again, two when it needs a
trim or a growth; the zprobe's grow-by-one loop (`scratchpad/zprobe/src/main.rs`, build) costs one call
per record added and is not the algorithm. The fill (payload bytes over `ZCAP`) is expected at 0.97 or
better [P] and `grow_rounds` is the knob stage 0b sets from a measurement of fill against calls.

**The function is total** (rev 1.6). The first version's shrink, `m -= max(1, ceil((len(c) - ZCAP) * m / len(c)))`, is
`m = floor(m * ZCAP / len(c))`, which is zero whenever `m * ZCAP < len(c)`: two records of 4,500 compressed bytes each
(`m = 2`, `len(c) = 9,000`), or `m = 7` and `len(c) = 35,700`. It then formed an empty image and divided by zero at
`per = max(1, len(body) / m)`, so the record-larger-than-a-page case this section says it handles panicked the build
thread (counted, retried forever) and the sampler, which runs the same function before the decline decision. It also
returned `NOPAGE` for a lone incompressible record of 4,056-4,072 bytes, whose LZ4 output is 4,073-4,089 bytes although
the record itself fits `ZCAP`. `m = 0` is unrepresentable now, the loop ends in at most twelve turns, and both cases
are fixtures of stage 0b (7.1) and of `tests/zpages.rs` (6.3).

**A record that does not compress, and one larger than a page.** A group whose LZ4 output is not smaller
than its image is stored verbatim if the *image* fits `ZCAP` (`STORED`: for 124-byte images, 32 records and 3,968
bytes a page, parsed in place with no decode and no copy); LZ4's own expansion is never a reason to shrink a group or
to give a record up. A single record whose image exceeds `ZCAP` and does not LZ4-code under it, or whose image exceeds
`ZMAX_ULEN` (a compact record's run is under 16,384 bytes, a key under 65,536, `src/flatindex.rs:690-699`), becomes a
`ZNOPAGE` entry and is read from the raw segment. A record with a block-backed extent (a run over
`Options::inline_bytes` = 256 goes to a 64 KiB block, `src/db.rs:4358-4378`, `1827`) is copied verbatim, extent and
all, and stays exact; its *value* still lives in the raw block region and its read faults there, so the form shrinks
only the records.

#### 2.6.1 The skip rule: the form is not built for data that does not compress

**The size floor.** A partition whose `recs_len + block_backed_bytes` is under `ZMIN_RECS = 2 MiB` (512 raw pages) is
declined before anything is sampled, with `declined_small` and `declined_small_bytes` counted and no file written. Two
reasons, both arithmetic [D]: the sample below needs 32 groups, about 1,900 records and 235 KB, and the attach's sampled CRCs
need 16 pages, so a file of a few dozen pages is mostly its own checks; and the most the form can take from the cache is
`(1 - 0.59) * 512` = 210 pages, 0.86 MB, which a partition that small repays only if every page of it were cold, and a
partition that small has few pages to be cold. A partition is at most `Options::partition_bytes` (default 64 MiB, `src/db.rs:1828`; the field's doc at
`:603-608`; `partition_limit` at `:18503-18508`), so a 10M store of about 1.5 GB is at least 23 files and the floor is expected to
decline none of them but possibly the last [P: `declined_small` 0-2 at 10M; the count 23 is `1.5 GB / 64 MiB` and not a
directory listing of the banked store, which was not kept, U]. The constant is a size chosen before the thing is measured,
which this repository's notes call a guess; a wrong value costs a file or the saving of a few hundred pages and never a
fault, and `declined_small_bytes` says how much of the store it left out.

Before the pass, 32 groups are formed at evenly spaced ranks with the real `fill_group` (about 130 KB of
output, 3-4 ms, 64 raw pages that may fault once each, every touch timed: the first measured `t_f` the selector has,
4.3). Let `pages_est = n / mean(count)` and `W_z = 4096 * pages_est + fence + filter`. **The partition is declined, with
a counter and no file, unless the model predicts a saving the extra CPU repays with margin** (rev 2.10, rev 2.2):

`phi_hat_raw - phi_hat_z  >=  1.5 * saving*`, with `phi_hat_z = clamp(1 - (B - f)/W_z)` over the partitions decided
together, `phi_hat_raw` the two-population form of 4.2, and `saving* = (t_z - t_hit)/t_f` of D5, `t_z` being the
sampler's measured decode and walk of its 32 groups (no decode for `STORED` pages) times 1.25 for a cold page, plus the
fixed cost.

This is the test the selector makes at entry (4.5), run by the sampler per partition set so that a partition that does
not compress is declined on its own. The first version declined on `4096 * pages_est + filter + fence > ZMAX_SHRINK *
(recs_len + block_backed_bytes)` with `ZMAX_SHRINK = 0.85`: a comparison against L2's footprint, a form this move does
not build, so it declined data that compresses to 0.85-1.05 of the records although the form still beats the raw form
that is actually served. At 0.85, `W_z = 1.07 GB`, `phi_z = 0.14` and a read costs 9.9 us against the raw form's 18.4,
1.9x; at 0.89, the 100-random-byte case, `phi_z = 0.18`, 11.4 us, 1.6x; both were declined by the fixed ratio and served
by nothing. The constant survives as `ZMAX_SHRINK_L2 = 0.85`, the right comparison on the day L2 is built (9.3). At the
suite's shape the ratio is `(175k * 4096 + 17 MB) / 1.24 GB` = 0.59 [D]. What the gate declines is the case with no
saving: a cache that already holds the raw form, a partition whose bytes are mostly block-backed, a store small enough
that the hash is the only footprint and the CPU is not repaid. Incompressible values are not declined at the cell; they
are stored whole and still save the hash's footprint (7.3, the incompressible control).

During the stream the gate is run again after an eighth of the partition on the pages written so far, and the build is
abandoned, the temp file removed and the partition marked declined for the life of the `Seg` (`Seg::z_declined`, 6.1)
if the realized `W_z` fails it by more than 10%.

A data shape the form does not serve at all is also declined: a partition whose block-backed bytes dominate
(`block_backed_bytes` is the sum of the block table's stored sizes, walked through `MappedBlocks::get`,
`src/flatindex.rs:2920-2965`, and returned by `Blob::z_source_facts` with the other facts; zero for the suite)
makes `W_z` and `W_raw` nearly equal and fails the same gate.

### 2.7 Integrity: what is checked, when, and what happens

| failure | seen by | when | action |
|---|---|---|---|
| file absent, empty, length not a multiple of 4096 | attach | open of the form (5.5) | none; raw serves; counted `rejected_damage` if a file was there |
| trailer torn | `trailer_crc`, magic, `version`, `page_size` | attach | file deleted, counted, raw serves, rebuilt when the form is wanted |
| resident part damaged | `res_crc`; the fence's tiling (below) | attach | as above |
| another segment's sidecar (id reused after a crash, a merge's orphan, a record-format change, an engine whose Bloom layout changed) | the binding fields of 2.5, `bloom_sig` and the fence-key membership check among them | attach | as above, counted `rejected_identity` |
| truncated tail, zeroed pages, a page lost to a torn unsynced write | length; the trailer; CRC of the first, last and 14 evenly spaced pages (16 faults, about 0.7 ms a file, once) | attach; and page CRC at first decode | as above |
| bit flip or zeroing of one page | per-page `crc`, `tag`, `first_rank` and `count` against the fence, `ulen` against the decoder's returned length, then a full structural walk (below) | the page's first decode in this process; the bit in `verified` is set only after all of it passes | that read is answered from the raw segment (fallback, counted), the page's `bad` bit set, `damaged` set; Maint retires the sidecar (renamed to a dead name, 5.6) and rebuilds it if the form is still wanted, at most three times a `Seg` (below) |
| a wrong group with a valid CRC (a builder bug) | the builder's decode-and-compare (2.6 step 6); the structural walk; the tests | build | the build fails, temp removed, `build_failed` counted, nothing attached |

The fence's tiling is checked at attach from the resident part alone, with no page read: `first_rank[0] =
0`; `first_rank` strictly increasing and `< nkeys`; the keys strictly increasing; every page number below
`P`, strictly increasing over the entries that have one, and each page used once; and `P` equals the
number of entries with a page. The structural walk of a page's first decode: the records parse
(`parse_record`) back to back to exactly `ulen`, there are exactly `count` of them, their keys ascend, the
first equals the fence key of the entry, and the page's `first_rank` and `count` equal the fence's.

A failed check must not answer "absent" and must not hide: the raw form is complete, so unlike the ordered
index (`src/ordindex.rs:163-171`, "no fallback") a fallback is allowed here, *and counted*, because a path
taken silently is the shape of every gate this repository broke (`src/db.rs:2566-2570`, `CLAUDE.md`
"Running the checks"). The rebuilt bytes need not equal the damaged file's (the grouping is a pure function
of the segment bytes, the constants and the `lz4_flex` version, and the test for rebuild compares decoded
groups and the fence, not bytes, so a dependency update does not turn it red).

**Rebuild after damage is bounded** (rev 1.7). `Maint` retires a damaged sidecar (renamed to a dead name, 5.6) and may
rebuild it while the form is still wanted, at most three times a `Seg`, each after the 60 s doubling backoff of a failed
build (5.7); after the third the partition is declined for the life of the `Seg` and counted `declined_damage`. A
deterministic builder defect that slips past step 6, or a media fault that persists, is thereby a counted decline and
not a loop of 0.35 CPU-s and 64 MiB written per partition per cycle. Step 6 runs the first-decode check on every page
it writes, so the defects that reach a reader are the ones neither the build nor the check can see.

Not checked, by design: a whole-file CRC at open. `OrdIndex::open` CRCs its entire file at every open
(`src/ordindex.rs:406-410`); for 0.72 GB that is a pass that re-streams the file through the cache the
form exists to keep resident. The per-page CRC at first decode is the same guarantee spread over the
reads that need it.

### 2.8 The wasm reader and the three-readers invariant

* `src/db/zpages.rs` is a child of `db`, which is `cfg(not(target_family = "wasm"))` (`src/lib.rs:68-69`);
  nothing in it can reach the browser's module, and `web/build.sh`'s `wasm_bytes` and `wasm_gzip` must read
  the same before and after (stage-2a check; the only edits in wasm-compiled files are two gated methods in
  `src/blob.rs` and one small function in `src/flatindex.rs`, which compiles on every target and is covered
  by its existing `dead_code` allowance for the wasm target, `src/lib.rs:51`, `70`).
* No method is added to `Bytes` (`src/bytes.rs:46-122`): `HostBytes` and the tests' copying sources
  implement `len` and `read_at` only (`src/wasmapi.rs:72-76`). No `async` anywhere under `blob`,
  `flatindex` or `bytes`.
* `Blob::zero_copy()` stays true on the native path (`src/blob.rs:824-832`, pinned by `tests/blob.rs:155`,
  `190-193`, `216-219` and `tests/segwriter.rs:150-153`): the z reader is a different reader on a different
  file and `Blob`'s own lending is not touched.
* `tests/blob.rs`, `tests/dict.rs`, `tests/ranges.rs` and `tests/segwriter.rs` open `Blob` and `SparseBlob`
  over a `SegmentWriter` file and never see a sidecar. They are **not parameterized and not edited**: that
  they pass unchanged is the evidence that the three readers (a `Blob` over a mapping, over a copying
  source, and `SparseBlob` over ranges) did not move. The z reader is a *fourth* reader, native only, and it
  is held to the first three in a new `tests/zpages.rs` (6.3) over the very partition file the `Db`
  wrote: every key, count and scan equal to `Blob<MmapBytes>` (lending), to a `Blob` over a copying source,
  and to `SparseBlob`'s dictionary walk through the helpers of `tests/dict.rs:23-82`.
* `flatindex::record_key_next(buf, off) -> Option<(&[u8], usize)>` (the key and the offset of the record
  after) is added **in `src/flatindex.rs` beside `record_key`** (`src/flatindex.rs:492`), because the walk
  that searches a group needs a record's length without rebuilding its compact extent -- no function gives
  that today; `parse_record` is the only one (`src/flatindex.rs:502-541`) -- and the layout must have one
  owner. Its compact case reads the four-byte header and returns `off + record_len_compact(klen, run_len)` (`src/flatindex.rs:703`,
  `pub(crate)`: the one owner of that arithmetic, which `parse_record`'s fourth element equals, rev 1.15); every other
  shape calls `parse_record`. A test compares it with `parse_record`'s fourth element on every record of
  every fixture and on a hundred thousand random ones.

### 2.9 Platforms (rev 3.7)

CI builds, tests and lints the engine and the suite on `ubuntu-latest`, `ubuntu-24.04-arm` and `macos-latest` (arm64)
(`.github/workflows/ci.yml:29-38`, and `scripts/check.sh build test lint bench` at `:55-58`), runs the library tests under
qemu for `aarch64-unknown-linux-gnu` (`:87-106`) and builds the wasm module (`:108`). Its own comment says the macOS leg
is there to compile the other side of the `cfg(target_os = "linux")` guards (`:29-33`; the engine's are `src/db.rs:153`,
`3233`, `6034`, `6633`, `6648`, `22567`, `22578`). The first version of this design added Linux-only calls with no other
side, and a green local run would not have been a green CI run (`CLAUDE.md`, "Running the checks"). The rules:

* `getrusage(RUSAGE_THREAD)`, the cgroup and `/proc` readers, `posix_fadvise`, `madvise(DONTNEED)` over a file
  mapping, `ioprio_set`, `sched_setscheduler` and every `mincore` or `/proc/self/maps` use are
  `#[cfg(target_os = "linux")]` with a no-op or `None` elsewhere. `libc` 0.2.191 defines `RUSAGE_THREAD` for Linux and
  some other targets and for none of the Apple ones (`src/unix/linux_like/linux_l4re_shared.rs:1019`; grep of the
  vendored crate), so an ungated use fails the macOS leg at compile time.
* Off Linux, `ZPages::Auto` behaves as `Off`, counted (`auto_inert`, 6.1), and `Always` works with the portable calls only
  (`MADV_WILLNEED` through `memmap2` is portable; the `DontNeed` drop-behind of 5.8 is gated to Linux and skipped
  elsewhere, where the build's read pages are the LRU's).
* Tests and examples that read residency (`tests/zpages.rs`'s residency check, `bench/examples/zresident.rs`, the
  `/proc/self/maps` assertion) are `#[cfg(target_os = "linux")]`; the rest of `tests/zpages.rs` runs on the three
  native legs (`cargo test --profile checked` through `scripts/check.sh test`), so the layout, the cursor and the oracle are held
  on three kernels' mmap; the qemu job runs `--lib` only (`.github/workflows/ci.yml:106`), so what it holds is the unit tests
  inside `src/db/zpages.rs` (the layout, `fill_group`, the encoders). The sidecar copies record images verbatim and so inherits
  the segment's native-endian rule (`src/flatindex.rs:25-31`).
* The page is 4 KiB by the format's choice (`page_size`, 2.3), not the host's. Where the host's page is larger (16 KiB on
  Apple silicon) one z page is not one fault and the model's "one fault a read" is false; `Always` still answers
  correctly, and the selector reads `sysconf(_SC_PAGESIZE)` and builds only where it equals `ZPAGE` (declined and
  counted `declined_page_size` otherwise). Stage 2a's DONE WHEN names all three legs (8).

## 3. READ PATHS IN THE COMPACT FORM

The rule every path below keeps: **a covered partition's read, count and scan touch the sidecar and nothing of the raw
segment's record, directory or hash regions, except at the sites 3.9 marks raw on purpose.** Every exception is in the
table, bounded, counted, and visible to the selector's "z is not paying" exit (4.5, E3), because a path that touches the
raw form beside the z form enlarges the working set it was meant to shrink (the union leak: `src/ordindex.rs:91-107`).
The first version of this section claimed no exceptions, and three scan paths broke the claim (rev 1.1, rev 2.5):
`scan_partitions` calls `BuildCtx::cut_at`, which reads two raw records whenever an unsealed key falls in the walk window
(`src/db.rs:17841`, `25702`, `25706`); `scan_merged` reads the partition's key through `blob.key_at` after every emitted
key and holds it across iterations (`18058-18059`, `18074`, `18145`, `18149`); and `OrdIndex::exact_at` binary-searches
`key_at` over a tied run unless every key has one length within a head (`src/ordindex.rs:864-905`). The suite's drained
scans reach none of the three, so a `raw_touches == 0` check over the suite's cells alone would have passed and a mixed
store would have failed it; 6.3 tests the shapes. Everything else in `Reader::read_all`, `Reader::count` and the scans is
untouched: the epoch pin, the partition routing by fence (`src/db.rs:13385-13388`; partitions carry no Bloom and
`Seg::may_hold` is the fence alone, `src/db.rs:5625-5632`, `5514`), the level-0 pieces over the range, the frozen tables
and the live table, the tombstone pre-pass, and the order sources are read in (partition, pieces, frozen oldest first,
live: `src/db.rs:13453-13484`). The sidecar replaces the one source that is a partition, nothing more.

### 3.1 Where the branch sits, and why both forms may be read at any instant

`Reader::read_all` loads the store's gate once, beside `self.advise(true)` (`src/db.rs:13374`), and holds
it for the operation as it holds the state (`src/db.rs:13381`: "a read takes the state once, at its
start"). With the gate at COMPACT the partition source is read by `Seg::read_all_z`, else by the closure
`seg_read` as it is today (`src/db.rs:13445-13457`); `Reader::count` the same at `src/db.rs:18246-18251`.
`Seg::read_all_z` loads the segment's `ZCell` (an acquire load; the read is pinned, so the pointer it
returns stays valid to the end of the operation, `src/db.rs:12703-12748`, `10587-10628`) and, if it is null
or the sidecar is damaged, answers from the raw form and counts a `rawserve`. Because the two forms answer
identically (the tests of section 6 hold them to it), a gate store, a cell install or a retirement at any
instant between two loads changes the price of a read and never its answer; that is the property that makes
publication a store instead of a protocol.

### 3.2 Point read, `ZSide::read_all(raw, key, f)`

1. `h = flatindex::key_hash(key)` (FNV, `src/flatindex.rs:1528`). If `!filter.maybe_contains_fnv(h)`: return
   0 values, **no fence search, no page**. The raw form answers an absent key from its resident hash
   without a record read (`src/flatindex.rs:1757-1788`) and partitions have no Bloom (`src/db.rs:5514`), so
   without this step the compact form would fault and decode a group to say "not here" -- a loss; with it an
   absent key costs one cache line of a 12.5 MB filter, and about 1.3% of absent keys pay a page [D: a blocked filter at 10
   bits a key, four probes in a 512-bit block, averaged over Poisson block loads: 1.26%, where the first version said
   2%: rev 2.13]. The filter answers only under a `bloom_sig` that matched at attach (2.5; rev 1.2).
2. `e = fence.entry_of(key)`: the greatest entry whose key is `<= key` (`partition_point` over `key_off`
   and `keys`, thirteen `key_cmp` compares of a 16-byte key (`src/db.rs:9443-9464`: two big-endian words, what `memcmp`
   answers without the call) at about 7.6k entries a partition, about 100 ns, the fences being 3 MB of heap). `None` (the key sorts below the first key) returns 0 values: a free negative for the first
   partition, whose fence is open (`Seg::below_lo`, `src/db.rs:5606`).
3. If `page[e] == ZNOPAGE`: return `Fallback`, counted `nopage_reads`; the caller reads the raw form.
4. `g = group(e)`: the memo (3.7), else the page: lend 4,096 bytes from the sidecar's mapping
   (`MmapBytes::slice_at`, `src/bytes.rs:206-212`) -- **this is the one page fault of a read**, at most --
   check the `verified` bit, and on its absence do the first-decode checks of 2.7 (CRC, tag, `first_rank`
   and `count` against the entry, decode length against `ulen`, the structural walk); then decode into the
   thread's aligned scratch, or for a `STORED` page parse in place.
5. Walk the group: from offset 0, `(k, next) = record_key_next(buf, off)`; `k < key`: `off = next`;
   `k == key`: found; `k > key` or the end: absent (a filter false positive). About 28 records are passed
   on average at 57 a page, at a few nanoseconds each with the compact-record shortcut [P: 0.2-0.5 us]. [P2] The keys are
   compared with `key_cmp`, never `<[u8]>::cmp`, which the repository measured at about a hundred instructions for
   dozen-byte keys (`src/ordindex.rs:903-906`); 28 compares at 8-30 ns are 0.2-0.85 us, so stage 0b times the walk apart from
   the decode (7.1) and the memo's hit read is priced with it (3.7; rev 2.9).
6. Found: `(_, exts, tail, _) = parse_record(buf, off)` and `raw.read_exts(&exts, tail, f)`
   (`src/blob.rs:1338-1346`, `pub`), so inline runs come out of the decoded group's tail and a block-backed
   extent comes out of the raw blocks exactly as in the raw path (`with_run`, `src/blob.rs:1223-1233`).
   `Blob::read_exts` ignores the tombstone flag and emits every value of every extent, which is what the
   raw `read_all` does, so a partition record that carries the flag (a partition can: `src/db.rs:2577-2586`,
   `13191-13239`, `27370-27372`) reads the same. The values are slices of the decoded group, lent for the
   duration of `f`; the scratch is taken out of its thread-local `Cell` and put back after, so a callback
   that re-enters the store gets a fresh buffer instead of a panic (`src/blob.rs:635-666`, `1249-1306`).

`lookup`-shaped access that returns `Exts<'_>` borrowed from the mapping (`Blob::lookup`,
`src/blob.rs:843`) has no z counterpart: extents that borrow from a thread's scratch cannot be returned, so
every z accessor is callback-shaped, like `with_lookup` and `with_record_at`
(`src/flatindex.rs:1757`, `1859`), which the engine already moved to for the compact record's sake.

**Pages touched by a present key with an inline run:** one z page; zero raw pages; the fence and the filter
are heap. With a block-backed run: one z page and the raw block's 4 KiB chunk (`chunk_span`,
`src/blob.rs:2075-2086`), as the raw form would touch it. **Absent key:** none (98.7%) or one (1.3%).

### 3.3 Count

`Blob::count` is `lookup` then the sum of `Ext::records()` (`src/blob.rs:1491-1497`). `ZSide::count` is steps
1-5 and then the sum over `exts` of the same function; the same pages. The Db's count adds the
other sources as before (`src/db.rs:18253-18275`).

### 3.4 Seek and `key_at`

`ZSide::seek(key) -> usize` returns the rank of the first key at or after `key` as `Blob::seek` does
(`src/blob.rs:870-876`): steps 2-5 with "first record whose key is `>= key`"; the rank is
`first_rank[e] + index`; if every record of the group is smaller, `first_rank[e] + count`, which is the next
entry's first rank or `nkeys`, with no second page; a key below the first key is rank 0. One page.
`ZSide::key_at(rank, f)` finds the entry by `partition_point` over `first_rank` (a `u32` array: thirteen
compares of integers), decodes it, skips `rank - first_rank[e]` records and hands the key to `f`. Callback
shaped, for the lifetime reason above, and a loop that keeps a position uses the cursor of 3.10 instead.

**Seeks of a covered partition go by the z fence and not by the ordered index** (rev 2.5; the first version made this
stage 3b, conditional). `ZSide::seek` is steps 2-5 above and costs the page the scan reads next anyway, since the walk
begins there, so it adds the in-group walk (0.2-0.85 us) and takes the seek's own page fault out of the ordered index's
heads. The ordered index (`OrdIndex::seek_exact_hint`, `src/db.rs:17058`) answers a seek from the heads, 8 B a key (80 MB
at 10M), when every key has one length up to its prefix plus eight bytes -- the suite's (`src/ordindex.rs:28-35`,
`864-887`) -- and otherwise binary-searches `key_at` over the tied run (`exact_at`, `src/ordindex.rs:864-905`): in the
compact form that search is a raw record read per probe, the union leak, and the suite's keys would never have shown it.
Seeking by the fence drops the heads from the compact form's working set (0.82 to 0.74 GB at 57 records a page, which is the
difference between D10's 43.1 and 39.3 records a page for the scan claim), makes the seek independent of key shape, and
leaves the ordered index serving the partitions that have no sidecar and the pieces. The heads path stays as the arm that
prices the choice (`Options::zpages_seek_ord`, `supdb-nocachezpagesheads`, 6.2).

### 3.5 Scan, `ZSide::scan_at(raw, rank, limit, f)`

The contract is `Blob::scan_at`'s (`src/blob.rs:1695-1901`): `limit` counts **keys**, the return value is the
keys visited, a key whose values are all gone (a tombstone-only key) counts and emits nothing, and the Db
errors if a partition walk returns fewer than it was asked for (`src/db.rs:17104-17111`: "a partition's
walk stopped short of its key count"). Algorithm:

```
seen = 0; r = rank
while seen < limit and r < nkeys:
    e = entry holding r                                   // binary search over first_rank, once; then e += 1
    if page[e] == ZNOPAGE:                                // the form does not hold these records
        n = raw.scan_at(r, min(limit - seen, count(e) - (r - first_rank[e])), f)?   // counted: nopage_scans
        seen += n; r += n; continue
    g = group(e)
    advance to record index (r - first_rank[e]) by record_key_next                    // skip, no parse
    for each remaining record of the group while seen < limit:
        (k, exts, tail, len) = parse_record(g, off)
        for ext in exts: with_run / each_value -> f(k, v)       // the general loop of Blob::scan_at, src/blob.rs:1838+
        seen += 1; r += 1
    // the group is spent: the next entry is e + 1, its first_rank == r by the tiling invariant
return seen
```

All arithmetic on `rank` and `limit` saturates: `limit` is `usize::MAX` for an unbounded scan and a span
that adds a rank to it wrapped once and panicked in the first debug build that reached it
(`CLAUDE.md` "Running the checks"; the `checked` profile and a test for `usize::MAX` guard it). **Pages
touched:** `1 + floor((j + L - 1)/57)` for a start at offset `j` in its page and `L` keys, which averages
`1 + 99/57 = 2.74` at `L = 100` [D]; the seek is by the fence (3.4) and adds no page. **Boundaries:** a
group is decoded once per scan call and the next entry's group when the walk crosses; the memo (3.7)
keeps the last decoded page of the thread, so the Db's many short `scan_at` calls from one rank range (the
block walks call it per run: `src/db.rs:25456`, `25480`, `25645`) do not decode the same page again.
**Prefetch** (rev 2.12): when the thread's last closed census window faulted at 0.05 or more a scan, and not otherwise, a scan
of `L` keys asks before its first decode for the pages it will walk after the first, `k = min(ceil((L - 1)/mean_count), 16)`,
with one `MADV_WILLNEED` over `k * 4096` bytes (`MmapBytes::advise_willneed`, `src/bytes.rs:219-240`; it works under
`MADV_RANDOM`, `docs/engine.md:5858-5866`), counted `prefetches`. A resident store issues none: the raw scan plans no hint for
inline runs at all (`Blob::prefetch_scan`, `src/blob.rs:1657-1658`), and a `madvise` over pages already in the cache is a
syscall and a page-cache lookup a page, about 0.5-1 us of a scan of 11 [U: not timed here], 5-9%. The raw form's equivalents (`Blob::prefetch_ranks`, `prefetch_dir`,
`prefetch_scan`; `src/blob.rs:883-951`, `1658`) hint raw lines; in the compact form they are not issued.

The fast loop of `Blob::scan_at` (compact-FIXED and one-extent full-FIXED records parsed in place at 12.7 ns
a record, `src/blob.rs:1747-1836`) is not reused: its input is a directory and a record region, and a
group has neither. A group's records are walked in order by `parse_record`, 20-30 ns a record [D from
`src/blob.rs:1717-1725`'s own measurements: "the record layout parses at about 12 ns an entry and the
general loop walked it at 29"], which at 100 records is 2-3 us against 8 us of decode: the fast loop is
the second lever, after the decode, and is not taken in this move.

### 3.6 Values, extents, multi-value keys, tombstones

* `ZSide::values_at(raw, rank, f)` is `Blob::values_at` (`src/blob.rs:973-989`): the record at `rank` by the
  entry and the skip, then `read_exts` for each extent.
* `ZSide::with_exts_at(rank, f)` hands `(key, &[Ext], tail)` to `f`; the four Db sites that call
  `Blob::exts_at` to look at flags or counts (`src/db.rs:13915`, `16790`, `18126`, `24822`) take this form.
* **Multi-value keys.** A key's values are one run in one extent of one record (`SegmentWriter` makes
  exactly one extent a key, `src/db.rs:4348-4378`): FIXED (equal widths, no prefixes) or prefixed, a
  compact record while the run is under 16,384 bytes and fits `inline_bytes` = 256, else a full-form
  record with an extent into a 64 KiB block. The group holds the record image either way, so `each_value`
  (`src/index.rs:206-260`) is the decoder of both forms. A compact *prefixed* run rebuilds its `last` by
  walking `n - 1` prefixes on every parse (`compact_ext`, `src/flatindex.rs:770-778`): a per-parse cost
  proportional to the record count, which the in-group walk pays on every record it passes; the walk uses
  `record_key_next`, which does not rebuild the extent, and parses fully only the record it stops at.
* **Tombstones.** A seal writes the values after the newest tombstone plus the flag
  (`src/db.rs:20300-20365`), so a record can carry the flag and live values, and a pure delete is an
  empty run with the flag. A partition can hold both (the places cited in 3.2). Records are copied verbatim,
  so flags are exact, and a tombstone-only key is a key that counts and emits nothing in both forms.

### 3.7 The decoded-group memo (a hot-key tier that is not a second form)

The Zipfian mixes (B, C, E, F; `bench/src/run.rs:297-306`) read a hot set that sits in a few contiguous
pages: the generator is unscrambled -- `KeyGen::next` returns rank 0 for the hottest (`bench/src/workload.rs:121-155`) and the mix
driver maps the draw straight to a key (`db_key_into(keys.next(), ..)`, `bench/src/run.rs:2285-2315`) -- so the most popular
keys are the lowest numbered, the first ranks of the first partition. In the raw form those pages stay
resident and a hot read costs 0.73 us; in the z form every read of them would decode, 3 us for a record
that was read a thousand times. The footprint does not change (the page is the same page) but the CPU does,
and mix C is where it shows: `supdb-nocache` is at 107k ops/s against 239k, and a z read at 4 us would
bring it to about 215k, 0.9x -- level, not a win [D: 1/(4.0 + 0.4 + 0.3) us].

The remedy is a per-thread, direct-mapped memo of decoded groups: **1,024 entries, capped at 8 MiB of decoded bytes a
thread** (rev 2.9: the first version's 256 entries and "about 2 MB a thread" assumed 7 KB groups, and `ZMAX_ULEN` is 64
KiB, so 256 entries of highly compressible data would be 16 MB; the byte cap bounds it), each an `Rc` to the group's
aligned buffer, indexed by `(side id, page)` hashed to a slot, filled on a miss, allocated lazily per entry and reusing the
buffer of the entry it replaces when its count is one. A hit costs the index, a compare and an `Rc` clone (3 ns) instead of
the decode, and **still pays the in-group walk** (28 `key_cmp` compares, 3.2 step 5): a hit read is 1.2-1.9 us, not 0.9
[P2]. The `Rc` keeps the buffer alive across a re-entrant callback without a `Cell` take-and-put; a retired sidecar's
entries are never hit (the id is never reused) and are overwritten in time. Memory: at most 8 MiB a thread, allocated only
in the compact form; at three reader threads that is 24 MB of the cache budget, which the model reads as `anon` (4.2).

The hit rate is the first version's error. For Zipf(0.99) over 10M keys the 256 most popular pages hold the lowest 14,600
keys, which carry 0.589 of the reads [D: the generalized harmonic partial sums; the top 256 keys carry 0.348, the top 3,600
0.504, the top 100,000 0.707; recomputed here over the 175k page masses of 57 keys: 0.5889]. That is the *static optimum* for
256 entries, which no online policy reaches, because 41% of the reads go to 175k tail pages and every miss evicts a slot.
Over the same page masses a direct-mapped 256-entry memo hits 0.45 (sd 0.01 across hash seeds), a 4-way 0.48, a fully
associative LRU (Che's approximation) 0.49; 1,024 entries hit 0.55 direct-mapped and 0.58 fully associative, and 4,096 pages
hold 0.76 as a static optimum [D, recomputed in a few lines of Python; rev 2.9]. Mix C's average operation is
`h * t_hit + (1 - h) * t_miss + o` with `h = 0.55`, `t_hit` 1.2-1.9 us, `t_miss` 4.0-4.7 us (D5) and the mix's own cost `o`
0.4-0.7 us (the first version's 0.4 + 0.3): 2.9-3.9 us, **260-350k ops/s, central 300k, 1.1-1.5x LMDB's 239k** [P2], where
the first version said 430k and 1.8x. A frequency-aware admission (a page admitted on its second touch within a window, a
few hundred bytes of count-min) is the arm if the direct-mapped hit rate lands under 0.45, and is not built in this move.
It is a priced lever, not an assumption: `Options::zpages_memo` (default on) and the arm `supdb-nocachezpagesnomemo` (6.2);
if the memo does not move mix C by what the arithmetic says, the arithmetic is wrong and the row says so.

### 3.8 The overlay of unsealed keys

The z form replaces the partition's source and no other. A key written since the partition was written is
in a level-0 piece, a frozen table or the live table; those sources are read as they are, in the order
above, and a partition's value for the same key is read first and the overlay's after, so the order of
values is the raw form's. In the scans, the machinery that merges the partition's walk with the unsealed
keys' snapshot (`scan_blocks`, `scan_partitions`, `scan_merged`, `walk_deltas`, `copy_from_deltas`:
`src/db.rs:16924`, `17791`, `18007`, `25456`, `25645`) calls the partition through the `scan_at`, `values_at`, `exts_at`,
seek and cut sites of 3.9, and goes through the z wrappers, or the cursor of 3.10 where it keeps a position, at each; the
snapshot's overlay is never read from a sidecar. `Cached::Block` copies own their values and read no partition after they are
built (`src/db.rs:17382-17419`); building them reads through the same wrappers. A direct run in progress above the store's
greatest key is an ordered memtable, not a partition, and is read as today.

### 3.9 The sites, and the mechanism that lists them

The uses of `seg.blob.` in `src/db.rs` (about 40 `key_at`, 40 `keys`, 14 `id`, the rest the read methods; a one-line grep
counts 40 `blob.key_at` and a multi-line-aware count 45, and the first version's 38 and 35 were counts that moved) are the whole
surface. `Seg::blob` changes from `Blob<MmapBytes>` to `SegBlob`, a newtype around it with **no `Deref`**. The
mechanism by which the compiler lists the sites is not "a method for each name": a forwarder compiles at every site that
calls it, so the decision would be made once per method name and not once per site (rev 1.1: the first version said "a
site that was not decided does not compile", and with a `key_at` forwarder none of the 39 would have failed).
`SegBlob` therefore has **no forwarder for any name that has a z counterpart or a raw footprint** -- `key_at`, `lookup`,
`read_all`, `read_all_by_value`, `count`, `scan`, `scan_at`, `values_at`, `exts_at`, `prefetch_scan`, `prefetch_dir`,
`prefetch_ranks(_est)` -- and forwards only the names that read no record: `keys`, `id`, `index_bytes`,
`advise_random`, `advise_normal`. A site that calls one of the first group does not build until its author writes a
z-aware call (which takes the covered or uncovered branch) or `.raw()`, the raw form on purpose. `raw()` is the one door
to the raw `Blob`: under `debug_assertions` it increments `raw_touches` when the segment has an installed sidecar and the gate
is at COMPACT (6.4), so `grep -n '\.raw()' src/db.rs` is the list of every deliberate raw read and the counter is the list
of the ones that run. `prefetch_block` (`src/db.rs:9553-9565`) takes a `&Blob` and is given `seg.blob.raw()`. The test of the
mechanism is `the_compact_form_touches_no_raw_page` over the shapes of the table below, each of whose rows is a fixture; the
first version's table missed `cut_at`, the partition cursor of `scan_merged` and `exact_at`, and that test would have failed
on exactly those.

| site | operation | disposition |
|---|---|---|
| `src/db.rs:13445-13457` | point read of the partition | **z** `read_all` |
| `src/db.rs:18246-18251` | count of the partition | **z** `count` |
| `src/db.rs:13432`, `18235` | `lookup` in the tombstone pre-pass | raw: level-0 pieces only |
| `src/db.rs:17104`, `17374`, `17847`, `24929`, `25284`, `25456`, `25480`, `25645` | `scan_at` over a partition | **z** `scan_at` |
| `src/db.rs:13929`, `16814`, `17967`, `18141`, `24857` | `values_at` over a partition | **z** `values_at` |
| `src/db.rs:16824`, `18157`, `24864` | `values_at` over a piece | raw |
| `src/db.rs:13915`, `16790`, `18126`, `24822` | `exts_at` for flags / counts | **z** `with_exts_at` |
| `src/db.rs:17832`, `18058-18059` | `scan_partitions`' and `scan_merged`'s seek of a partition, and its first key | **z** `ZSide::seek` / `cursor_seek`, by the fence (3.4, 3.10) |
| `src/db.rs:17841` calling `25702`, `25706` (`BuildCtx::cut_at`) | where the next unsealed key cuts the walk window | **z** `ZSide::cut_in` (3.10); two raw records otherwise (rev 1.1, rev 2.5) |
| `src/db.rs:18074`, `18141`, `18145`, `18149` | `scan_merged`'s partition cursor: first key of the next partition, its values, the key after each emitted key | **z** `ZCursor::{key, values, advance}` (3.10) |
| `src/db.rs:18042`, `18046`, `18161` | `scan_merged`'s level-0 cursors | raw: a piece has no sidecar (3.10) |
| `src/ordindex.rs:864-905` (`exact_at`) | binary search of `key_at` over a tied run when keys are not one length within a head | not reached for a covered partition: its seeks go by the fence |
| `src/db.rs:17058`, `17067`, `16501`, `22216-22220`, `24130-24131`, `24221-24222`, `24265-24266`, `24306-24358`, `24760` | seek, cut and rank closures of the block-cache machinery (`scan_blocks`, `BoundsById`, `owner_of`) and of the pieces | the default policy's: raw `key_at` behind the ordered index on pieces; on a covered partition through the z wrappers in stage 3, counted by `raw_touches` until then (R17) |
| `src/db.rs:13865`, `13874`, `13914`, `13926` | the k-way arm (`scan_merge: false`) | raw and counted; a comparison arm never paired with `zpages` |
| `src/db.rs:9553-9565`, `17063`, `13719-13723` | prefetch hints | not issued on a covered partition; z span hint instead |
| `src/db.rs:13322-13338` | `advise` loop | adds `ZSide::advise` |
| `src/db.rs:8774-8786`, `9056-9068` | merges' own mapping of their inputs | raw by design: one sequential pass, nothing kept |
| `src/db.rs:18917-18924`, `19289-19302` | open's checksum-row verify, `pending_whole` | raw by design: a pass over the whole key section at every open, dropped by the cache as z pages are touched |
| `src/db.rs:5438-5440` | `learn_prefix` first key at open | raw: one page a segment |
| `src/db.rs:21359-21368` (`same_blocks`) | key at every 64th rank of a rewritten partition and its predecessor, at a publish | skipped when no forms are kept, sampled from the heads otherwise (rev 1.14): the layout it feeds belongs to the block cache, so `layout_of`'s `rebase` argument (`src/db.rs:26910`; `forms_rebase`, default true, `src/db.rs:1918`) is `false` for a store with `scan_block_cache` off; with forms, the keys come from the ordered index's heads (`whole_key_at`, `src/ordindex.rs:488`, as `block_bounds_of` does, `src/db.rs:9993`), 8x fewer pages and resident after any scan. Raw it would read `n/64` record pages of the old partition, about 6.8k at 435k keys, cold by construction in COMPACT: 0.28 s of major faults a landing, on the thread everything lands through |
| `src/db.rs:16895-16900` (`make_table`) | the partition's last key, once a table | raw while forms are built (the default policy; the nocache arms build none); stage 5 |

### 3.10 The partition cursor, and what is raw until it exists (rev 1.1, rev 2.5)

Three paths keep a *position* in a partition and ask for the key there again and again: `scan_merged` (`src/db.rs:18007`)
holds `pkey = blob.key_at(prank)` across iterations and compares it with the pieces' keys; `scan_partitions` asks
`BuildCtx::cut_at` where the next unsealed key falls in its window; and the default policy's `walk_deltas` and
`copy_from_deltas` (`25456`, `25645`) do the same over a block's ranks. The callback-shaped `ZSide::key_at(rank, f)` of 3.4
serves none of them: it re-skips from the group's start on every call (O(57), ten times the raw per-entry cost) and cannot lend
a key across an iteration. One type serves all three:

* `ZSide::cursor_seek(key) -> ZCursor` and `ZSide::cursor_at(rank) -> ZCursor`: the entry by `partition_point` over the fence
  or over `first_rank` (once), the group through the memo (an `Rc`), the record by `record_key_next`;
* `ZCursor::key() -> Option<&[u8]>`, `ZCursor::values(f)`, `ZCursor::rank()` and `ZCursor::advance()`, which is one
  `record_key_next` and, at the end of a group, the next entry's group through the memo; an entry that is `ZNOPAGE` hands the
  cursor to a raw sub-cursor for exactly those records (counted `nopage_scans`);
* `ZSide::cut_in(rank, end, uk) -> (usize, Ordering)`, `cut_at`'s answer from a cursor: the fence picks the one group of the
  window that can hold `uk` and a walk of at most 57 `key_cmp` compares finds the rank, 0.25 us on average, where the heads
  and one record cost a cache line and a page.

`scan_merged` keeps a key across the advance of the cursor that lent it. On the raw side the key borrows from the mapping and
nothing changes; on the z side the key lives in the cursor's group, which `advance` may release, so the loop copies the
partition's key into a 32-byte owned buffer (a copy a key, 2-4 ns, priced in stage 3 by callgrind on both forms; the raw side
keeps its borrow behind an enum dispatched once per partition, not once per key). The alternative is a ring of two retained
groups and one lifetime-extending `unsafe` with its SAFETY argument (the buffer is immutable while an `Rc` to it lives); it is
the arm if the copy shows in the scan cells. Neither touches the raw path of an `Off` store.

**Until the cursor exists** (stages 2a and 2b ship point reads and counts only) a store under `Always` serves the scans of a
covered partition raw, counted `raw_scans`, and the verdict reads the footprint of those cells as raw plus z; stage 3 is the
stage that makes them z, and `raw_touches == 0` is its gate, not stage 2's. What the controller does while raw scans run beside
z reads is E3 (4.5): the realized saving falls, and the exit fires when it stays under `saving*` for a dwell. The level-0 pieces
over a range are raw by construction (a piece has no sidecar and its life is the merge), and a read or scan over a range that has
one touches the piece's pages beside the partition's z pages; that union is bounded by the pieces' bytes (at most `l0_trigger` a
range) and is the write-heavy mixes' price (R8).

## 4. FORM SELECTION

The form is chosen by the selector (`ZSelect`), a controller with three rungs, run by the segment work's thread, fed
by two signals and a sketch, and published as one store-level word. It builds nothing, samples nothing and reads no
cgroup while the store fits; every constant is named with the quantity it stands for, because "nothing typed: the
moment a threshold appears in code, ask what measured quantity it is standing in for" (`bench/CLAUDE.md`, "Rules
that stay"), and none of them is claimed measured until stage 4 measures it. The word stores the *form wanted* (0 raw,
2 z; 1 is the raw form with the census running), and a third value is reserved so that L1 and L2 (9.2, 9.3) plug into
this selector if they are ever built, instead of growing a second one (rev 3.12).

### 4.1 Rungs and options

| rung | `Shared.zgate` | what it means | cost in the read path |
|---|---|---|---|
| FITS | 0 | the model says the raw form's footprint fits the cache budget; the initial rung | the gate load, a test and a not-taken branch: three instructions, one extra hot line |
| SUSPECT | 1 | the model says it may not; handles run the census; reads are served raw | the census countdown, about five instructions |
| COMPACT | 2 | covered partitions are read from the z form | a z read, and the footprint sketch's one store |

`ZPages::Off` pins the gate to 0, samples nothing, builds nothing and sweeps sidecars at open.
`ZPages::Always` pins it to 2 and builds at every landing (the forced arm: it prices the form without the
controller). `ZPages::Auto` is the controller below, and **it degrades where it cannot run instead of refusing the
open** (rev 1.3, rev 2.18): `Auto` needs the segment work's thread, which exists unless `publish_in_background` is off
(`src/db.rs:18733`, `19160`, `21583`), a pinned writer (`writer_pins`: the clearable cell is freed through the epoch, and
an unpinned writer reads it unpinned, `src/db.rs:8178-8183` is the precedent) and Linux (2.9); without any of them it
behaves as `Off`, counted (`auto_inert`, a gauge of `zpage_counts`). Only an explicit `Always` beside an unpinned
writer is refused by `check_options` (`src/db.rs:8173`): a request that cannot be honoured is an error, a default that
cannot be honoured is not. The first version refused `Auto` in those cases and made `Auto` the default at stage 6, which
turned every `Options::default()` store with `publish_in_background: false` or `writer_pins: false` into a failed
`Db::create`: `tests/db.rs:607`, `616` (`oracle-nopin-inline`, the supported inline-and-unpinned shape), `8355`
(`snapshot_keeper`), `9327` (`seal_snapshot_store_with`, the base of many tests), `11712` (both arms of
`a_flush_publishes_its_seal_before_the_seal_is_durable`) and the suite's `supdb-inlinemaint` and `supdb-nopin` arms
(`bench/src/engines.rs:3480`, `3486`) -- the shape "an arm sound only beside the default it was priced with" turned around
(`CLAUDE.md`): a new default sound only beside the old defaults of other options, enforced as an open error.

### 4.2 Signal S1: the model, out of band, free

On the segment work's tick (`Maint::run`, `MAINT_POLL` = 10 ms, `src/db.rs:25822`, `25991-26015`), **once a second in every
rung** (every hundredth turn; rev 2.15: the first version ran S1 at every 10 ms turn outside FITS, which re-resolves the
group from `/proc/self/cgroup` and `/proc/self/mountinfo` and reads `memory.stat` at 100 Hz, about 100-300 us a tick [U: not
timed] on the thread that lands seals and merges). The census is summed and the transitions of 4.5 are evaluated every
tenth turn, a 100 ms interval; the 10 ms turn itself only polls the build and the warm:

* **The cache budget `B`** = `L_eff - anon - margin`. `L_eff`: the cgroup's limit through the chain --
  v1 `hierarchical_memory_limit` from the process's group `memory.stat` (the group is found from
  `/proc/self/cgroup` and `/proc/self/mountinfo`, re-resolved every tick because the process can be moved
  and limits change: `bench/src/bound.rs:169-205`, `284-298`), or `memory.limit_in_bytes` where `use_hierarchy` is
  off, `9223372036854771712` meaning none; v2 the minimum of `memory.max` and `memory.high` over the path to
  the mount root; with neither, `MemTotal`. This host is a hybrid: the memory controller is v1
  (`4:memory:/process_api/.../claude-code-bash`) and the unified v2 mount lists `hugetlb` alone
  (`/proc/self/cgroup`, `/sys/fs/cgroup/unified/cgroup.controllers`, read now [M]), so a v2-first reader
  finds no `memory.max`: a v1 `N:memory:` line wins when present. `anon`: `rss + rss_huge` (v1) / `anon`
  (v2) of the same group's `memory.stat`, else `RssAnon` of `/proc/self/status`. `margin = max(5% of L_eff,
  64 MiB)`. The worked numbers of this document use the measured cache, 937 MB, which is the limit less the process's `anon`; S1's `B` takes the margin off as well, about 870 MB at the cell, where `phi_hat_raw` reads 0.48 and `phi_hat_z` 0, and the thresholds are tested on S1's `B` (D10 says which cell each stop is for; rev 2.13). The parsers exist in `bench/src/bound.rs` (`parse_cgroup_path` 33-44, `parse_mount` 51-62,
  `parse_stat` 89-112) and are ported, not shared, since the engine cannot depend on the suite
  (`src/db/zselect.rs`, v1 and v2). Nothing in `src/` reads a cgroup today [U: grep of `src/` finds none].
* **The footprint** of what a raw point read touches: `H = sum of hash-region bytes`, `R = sum of
  record-region + block-backed bytes` over the live partitions, from `Blob::z_source_facts`, kept on
  the `Seg` at open (two words).
* **The model's fault rate**, per form. The raw form reads one hash page and one record page a read, uniformly: Che's
  approximation to LRU over the two populations, `T` solving `n_h (1 - e^(-T/n_h)) + n_r (1 - e^(-T/n_r)) = B/4096` for
  `n_h = H/4096` and `n_r = R/4096` (a dozen bisection steps a second), and `phi_hat_raw = e^(-T/n_h) + e^(-T/n_r)`. It
  reproduces D2 at the cell, 0.428 against 0.425, and exceeds 1 once the hash stops fitting (D9); the first version's
  `clamp(1 - (B - H)/R, 0, 1)` saturated at one fault a read, which hid that and produced an x_max (rev 2.2). The z form is one
  population: `phi_hat_z = clamp(1 - (B - f)/W_z)` with `W_z` from the sampler, `max(0, 1 - (937 - 17)/720) = 0` at the cell.
  The *saving* is `phi_hat_raw - phi_hat_z`.
* **The pressure term**, for a store the model cannot see the neighbours of. The model prices one store
  against the whole budget; two stores in a process, or a process beside others in a group, each say "fits"
  and together do not, and a rung that waits for a model's permission would never sample. So S1 is the model
  *or* the group's own pressure: the rate of `workingset_refault_file` (the file pages reclaimed and read
  back; `bench/src/bound.rs:89-112` already reads it from the v1 `memory.stat`, where this host has it, and the v2 file has the same
  key) above 256 pages a second on a tick. The pressure term opens the census **and the build clause of 4.5** (rev 2.16: the
  first version required `phi_hat >= theta_sus` there as well, which blocked exactly the neighbour case the term and R21 exist
  for, since a neighbour's page cache is not subtracted from `B`); the census, not the pressure, decides whether to switch.

### 4.3 Signal S2: the census, in band, only in SUSPECT and COMPACT

What the engine cannot see is that a lookup blocked: the first touch of lent bytes is a page fault inside
`seg_read`, behind `slice_at` (`src/bytes.rs:206-212`; the mapping is `MAP_SHARED`, no populate). It is
inferred from outside, from the thread's own counters, and **not by timing lookups**.

* **What is read.** `getrusage(RUSAGE_THREAD).ru_majflt` (cumulative per thread; `libc` 0.2.191 exports `getrusage` and,
  for Linux and some other targets and not for Apple's, `RUSAGE_THREAD`: 2.9; the engine already calls `libc` for
  `clock_gettime(CLOCK_THREAD_CPUTIME_ID)`, `membarrier`, `madvise`, `sched_setscheduler`, `ioprio_set`: `src/db.rs:22577-22590`,
  `6643-6653`, `152-165`) and the thread's CPU and wall clocks. Two readings bracket any number of operations: the faults the
  thread took as *major*, per read or per scan, with no latency threshold. This is the suite's own measure
  (`bench/src/run.rs:1122-1124`, `1247-1274`), so the engine and the row agree on what a fault is. It counts the faults that
  missed the page cache, which are not quite the faults that blocked: a fault that finds its page in the cache but still under
  another reader's I/O, or under a `WILLNEED` or readahead in flight, waits without being counted as major [U: kernel
  behaviour, not checkable in this tree; stage 1a tests it with a `WILLNEED` in flight], so `phi_meas` under-counts blocking
  whenever `warm` or a scan's prefetch is running. The first version's "exactly, no false positive" is true of the count and
  not of the inference (rev 1.10, rev 2.1).
* **Why not the others.** An `Instant` pair is 50 ns (`CLAUDE.md` profiling: "a probe of the clock in a
  tight loop read 50 ns a pair"), 25% of a 212 ns read at 10k keys [M: `bench/runs/full/20260929T025048Z-5eabd8d.json`, 4.72M reads/s; a clock read is 25-40 ns, `docs/engine.md:2570-2572`] and it
  needs a latency threshold; `rdtsc` is x86 only and also needs one; `mincore` reports residency, not
  blocking, answers for page-cache pages only of files the caller owns or may write on kernels since 5.0
  [U: the exact rule is kernel-dependent and was not tried on this guest], and is a syscall of a microsecond; it is
  used by the *tests* (the residency probe of 6.3) and never on the read path.
* **Cost.** A countdown in the handle's `Slot` (`src/db.rs:6665-6686`, a line the read already stores to
  at its pin and unpin): `down -= 1; if down == 0 { close_window() }`, a load, a subtract, a store and a
  branch on a line owned exclusively. A window is opened with `N` operations (reads, or scans) to go and closed when they
  have gone or when 250 ms of wall time have, so a sparse or scan-only handle still closes: one `getrusage` (0.3-0.7 us, [U: not timed here]), the two clocks, a `gettid` to see that the
  handle did not move threads (handles migrate: `bench/src/engines.rs:4335-4341`). `N` is chosen so a window
  lasts about 100 ms of wall time: `N' = clamp(N * 100 ms / wall, 64, 2^20)`. At 100 ms the cost is
  under 0.001% of a thread; at the smallest window, 64 reads of 44 us, 0.05%. This stays inside the 0.3%
  budget the in-memory rows allow at any read cost (0.64 ns of a 212 ns read), because **in memory it
  is not run at all**: the countdown exists only in SUSPECT and COMPACT.
* **Validity** (rev 2.4, rev 1.10). A window is classed when it closes by what it contained: a *read window* (point reads
  and counts, no scans), a *scan window* (scans, no point reads), or *mixed*, which is dropped and counted
  (`mixed_windows`). It counts only if it ran on one thread (the `gettid` token), so a window opened by one thread and closed by
  another is dropped, which is why handles on a thread pool yield little evidence (said, not fixed). **A commit does not drop
  a window.** What a commit costs is an fsync wait and the WAL's `write`, neither a major fault, and under the nocache policy a
  commit reads no partition record (it appends to the memtable), so `phi_meas` and the scan rate count faults over windows with
  commits in them. What a commit corrupts is the wall clock: `t_f = (d wall - d cpu)/d flt` is taken from commit-free windows
  only, and where none has been seen -- the suite's A, B, D, E and F commit every hundred updates (`bench/src/run.rs:309`),
  which is in every 100 ms window, and at the banked rates a commit every millisecond in A and F -- from the sampler's timed
  cold touches (2.6.1), refreshed whenever a commit-free window appears. The first version dropped every window that held a
  commit or a scan, which is every window of those five mixes, and E (95% scans, the `ycsb` driver, `bench/src/run.rs:2275-2334`) and any scan-only
  workload have no read windows at all and no scan census to replace them, so `Auto` could not have entered COMPACT for a mix
  or a scan; the suite hid it by running the read cells first and letting the rung hold (`YCSB_ORDER`, `:308`). A policy that
  files forms at commit (the default policy) can fault inside a commit; `Auto` is restricted to policies that build none until
  that lead closes (7.8), and the windows of any policy with commits are dropped as the first version dropped them.
* **The scan census** (rev 1.10, rev 2.4). A scan window counts scans and the faults taken in them: `phi_scan = d sflt / d
  scans`, against its own break-even `saving*_scan = (t_z_scan - t_hit_scan)/t_f`, with `t_z_scan` the sampler's measured
  decode-plus-walk of `pages_per_scan` pages (2.74 at 100 keys, 1.86 at the mixes' 50) plus the fixed cost, and `t_hit_scan`
  the minimum over windows of cpu per scan. At the suite's cell that is `(11.3 - 3.3)/41.5 = 0.19` faults a scan against 2.9
  measured raw (M7). Either class can enter COMPACT; scans are no longer evidence-free.
* **Evidence, and the cold start** (rev 2.1). A cold cache reads as a footprint loss: after a restart or a cache drop on a store
  that fits, every first touch is a major fault, `phi_meas` is about 0.9 until the hot set is resident, and 0.3 s holds only
  `0.3 s / 41.5 us` = 7.2k faults (29 MB), so the first version's three-interval streak passed for any hot set over 30 MB however
  skewed. An interval is therefore *evidence* only when (a) it holds at least 64 valid operations, and (b) most of the store's
  faults are pages the cache once held: S1 reads the group's `workingset_refault_file` (the pressure term's counter; this host's
  v1 `memory.stat` has it, `bench/src/bound.rs:89-112`) and the interval counts when `d refault >= 0.5 * d majflt` over the
  last 5 s, `d majflt` being the store's own (`Readers::stat` of the handles' `zflt`). A first touch is not a refault; a store
  larger than its cache refaults almost every fault (the banked cell has `refaults_per_op` equal to major faults per op,
  M4, M7). Where the counter is absent the fallback is cumulative faults since SUSPECT of at least `B/4096`, the cache's size
  in pages (229k at the cell). [U: the kernel's shadow-entry accounting bounds how many refaults it can attribute to a very large
  thrashing set; the stage-4 replay carries a cold-start trace and a burst trace, and stage 1a one cold start of the probe.]
* **What the tick takes from it.** The handles' cumulative `zreads`, `zscans`, `zflt`, `zsflt`, `zwall`, `zcpu` over valid
  windows (single writer each, on the Slot and the `ZStat` line; summed by `Readers::stat`, `src/db.rs:6807`); the tick
  differences them: `phi_meas = d flt / d reads`, `phi_scan = d sflt / d scans`; `t_f = (d wall - d cpu)/d flt` over commit-free
  windows (the cost of a fault as this machine charges it: D1 says 41.5 us here, and the number is measured, never typed);
  `t_hit = min over windows of cpu/read` (the all-hit cost).

### 4.4 The thresholds, each with the quantity it stands for

| name | value [P2, set in stage 4] | the quantity it stands for |
|---|---|---|
| `saving*` | `(t_z - t_hit)/t_f`: 0.079-0.096 at `t_z` 4.0-4.7 us, `t_hit = 0.73`, `t_f = 41.5`; one per class, so `saving*_scan` for scans | the fall in faults per operation at which the compact form's extra CPU is exactly repaid by the faults it removes (D5; the first version's `phi*`, which assumed the form removes every fault). `t_z` is the build sampler's measured decode-plus-walk of its 32 groups, times 1.25 for a cold page, plus the engine's fixed read cost `t_hit`, until the census has measured the real one |
| `theta_enter` | `1.5 * saving*` = 0.12-0.14, tested on the **predicted saving** `phi_hat_raw - phi_hat_z` and on the measured `phi_meas - phi_hat_z` | enter only when the saving is half again the extra CPU; the margin covers the model's error and the build's one-time cost. The first version tested the raw rate alone, which cannot tell a form that removes every fault from one that removes few (rev 2.2) |
| `theta_sus` | `saving*/2` = 0.04-0.05, on `phi_hat_raw` | the model rate at which sampling is worth the countdown's five instructions |
| `C_build` | `build_cpu_est + displacement_est`: 0.35 s of CPU per 64 MiB partition from the sampler, 8 s at 10M, plus a displacement term stage 4c measures (0.5 x the CPU until then) | the ski-rental price: the form is bought when the rent paid since SUSPECT, `excess` (4.5), equals it, so the regret is at most twice the price whenever the workload ends (rev 2.1). It replaces the first version's streak of three 100 ms intervals, far shorter than the 8 s of CPU, 1.3 GB read and 0.72 GB written that it started, and under which a 0.3 s uniform burst on a skewed store began a 15-40 s build |
| `refault_share` | 0.5 over 5 s | a refault is a capacity miss and a first touch is not: cold start against a store larger than its cache (4.3) |
| S1 period, census interval | 1 s, 100 ms | the model moves with the limit and the anon, which move in seconds; the census adds in-band samples (rev 2.15) |
| `dwell` | `max(60 s, 10 * last build wall)` | a switch must pay back what it cost. A thread that reads continuously at any fault rate above `theta_enter` spends 0.87 or more of its time stalled (`phi * t_f / (t_hit + phi * t_f)` = 0.87 at 0.12 faults a read, 0.96 at 0.41 [D]) and the form removes nearly all of it, so a build of 20 s of idle-priority CPU is repaid inside the dwell; the dwell also keeps a transient from costing two builds; a cancelled build counts as a switch for the dwell and doubles the back-off of the trial (rev 2.1) |
| `retain` | `max(600 s, 20 * build wall)` | how long a sidecar is kept after the form is left: while a re-entry is likelier to be cheaper than a rebuild |
| trial backoff | 10 min, doubling to 4 h | the cost of being wrong about a skewed workload, bounded to under 1% of the time |

### 4.5 The transitions, with hysteresis

| from | to | condition |
|---|---|---|
| FITS | SUSPECT | `phi_hat_raw >= theta_sus` or the pressure term, on 2 consecutive S1 ticks (2 s) |
| SUSPECT | FITS | `phi_hat_raw < theta_sus/2` and no pressure, on 5 consecutive S1 ticks |
| SUSPECT | building (gate stays 1) | all of: (a) `excess >= C_build` (below), accumulated over evidence intervals only (4.3); (b) the predicted saving `phi_hat_raw - phi_hat_z >= theta_enter` from the sampler's `W_z` (2.6.1) and the measured `phi_meas - phi_hat_z >= theta_enter`; (c) the model not contradicting: `phi_hat_raw >= theta_sus` **or** the pressure term (rev 2.16); (d) `dwell` since the last switch, cancelled builds included; (e) the sampler not declining the partitions. Sidecars already on disk with a matching identity are attached instead of rebuilt, and `C_build` is then the attach's cost |
| building | SUSPECT | `phi_meas - phi_hat_z < theta_enter/2` on 3 evidence intervals (the evidence vanished): the build is cancelled, **counts as a switch** and doubles the back-off |
| building | COMPACT | sidecars attached for at least 90% of the covered footprint bytes (declined partitions excluded); the gate is stored **last**, after every cell |
| COMPACT | SUSPECT | **E1** the limit was raised or the store shrank: the maximum of `phi_hat_raw` over the last `dwell`, on S1 values smoothed over 30 s, is under `theta_sus/2` (rev 2.15: the first version exited on `phi_hat < theta_sus` for 5 ticks, 50 ms at the 10 ms tick, with no dwell, and `phi_hat` moves 0.3 for 400 MB of `anon`; E1 is unreachable at the 1 GiB cell, where it needs `B >= 1.4 GB`); or **E2** the trial of 4.7 says raw fits; or **E3** the form is not paying: the realized saving `phi_hat_raw - phi_meas_z` stays under `saving*` for a dwell, `phi_meas_z` being the census in COMPACT (rev 2.2) |

`excess` accumulates over every evidence interval since the rung entered SUSPECT, over all handles: `excess += (phi_meas -
phi_hat_z - saving*) * t_f * ops` (reads, and scans through their own rates), the thread-seconds the form is predicted to
have saved net of its extra CPU (rev 2.1). It is reset when the rung returns to FITS and halves for each `dwell` in which no
evidence interval was added, so a stale rent is not a credit. One thread stalled 96% of the time accumulates about 0.9 s a
second, so at the cell the build starts about 9 s in on one thread and about 3 s in on three, where the first version started
it at 0.3 s. Two consequences are accepted on purpose: the suite's `Auto` cells show a delayed entry (the three read cells are
a staircase, the entry falling in the first, the build and warm in the first or second and COMPACT from the third), and the
verdict reads `Auto` against the forced arm by the cells after the entry (7.4). What the rule buys is the regret bound: if
the workload stops the moment the build starts, the loss is the build, which is at most the rent already paid.

### 4.6 What the in-memory read pays

* In FITS, on `read_all`, `count` and the scan entry: one relaxed load of `zgate`, one compare, one branch
  that is never taken: three instructions [P; counted by callgrind over the exact demangled name of the
  monomorphised `Reader::read_all` with `--collect-atstart=no --toggle-collect=`, since a glob flips collection
  off at nested names, `CLAUDE.md` "Profiling"; expect +3 per call]. A cycle is 0.22% of the 212 ns read at
  10k keys [D: 445 cycles at 2.1 GHz], so the branch is at most that and sits off the hash probe's
  dependency chain. The word is a store-level atomic on a line of its own (`Shared` holds counters that
  scans write, `src/db.rs:10360-10370`; the slot-table lesson is that a word one thread writes on every
  operation lives on a line no other writes, `src/db.rs:6655-6663`; this one is written only by the tick).
* No allocation, no thread, no file, no syscall, no sample. The tick in FITS reads three small cgroup files
  a second (20-40 us [U]) on a thread that already wakes every 10 ms, and S1 runs at that rate in every rung. No `zpg-`
  listing happens after open.
* No read writes a line another thread writes. The per-read and per-scan counters of 6.4 are on the reader's own `ZStat`
  line, written by that handle alone and summed on read; the segment work's events (builds, attaches, retires, switches) are
  on `Shared`. The first version put every counter on `Shared`, about four increments a read, which is the failure
  `CLAUDE.md` records in "A slot table whose slots share a line": every scan through a handle bumped seven to thirteen shared
  statistics, and four handles read the threaded scan mix at 0.46x of LMDB, 0.91x with the counters off (rev 1.4, rev 2.11,
  rev 3.10). The pairing that prices it is `supdb-nocachezpages` against `supdb-nocachezpagessharedcounts`, the first version's
  shape kept as an arm, at three threads (7.5), because callgrind cannot see a contended line.
* The quick row must show it: deterministic quantities identical to three decimals (they are, in the nine most
  recent banked quick rows, 20261012T112705Z to 20261013T130539Z: 1.314 / 1.341 / 1.367 / 1.304 bytes on disk
  per byte, device bytes 1.349 / 1.376 / 1.402 / 1.339 at 10k / 30k / 100k / 300k for `supdb`; older rows differ
  because the write path then differed), rates level against the arm run against itself,
  and no extra thread in `SUPDB_THREAD_SPANS`.

### 4.7 Not flapping, and not fooled by a workload that fits

* **A skewed workload that fits the cache in the raw form** has a low `phi_meas` however large the store
  is, so it never builds: **S1 only decides whether to count, S2 decides whether to switch.** The model
  alone says "the store is bigger than the cache" and a Zipfian reader on it may never miss; the census says
  so, and the rung stays SUSPECT at five instructions a read.
* **A uniform workload entering and leaving.** The ski-rental price and `dwell` bound switches to one a minute, and a build
  cancelled before it installs counts as a switch (rev 2.1); a synthetic square wave between uniform and a 1% hot set at
  periods of 10 s, 60 s and 300 s is the test (`a_square_wave_switches_at_most_once_per_dwell`, 6.3), and the assertion is at
  most one switch per `dwell` and at most one build per partition per entry. In the first version a build started at 0.3 s of
  the 10 s wave and cancelled at about 5 s was not a switch, so builds churned while the assertion held; here it is one.
* **A skewed workload arriving while in COMPACT** costs CPU, not faults: 4 us where 0.73 would do, less
  with the memo (3.7). E1 cannot see it (the model is about the limit and the store). **E2** can: while
  COMPACT, each handle sets one bit per z page it touches in a bitmap of its own (`n_pages` bits, 22 KB;
  one store a read, 1 ns on a 4 us read); at window close it posts the popcount; the tick sums the
  handles' counts, an upper bound on the union (up to `T` times loose for `T` threads on one hot set). The raw footprint of
  those pages is at most `min(H, distinct * 57 * 4096) + distinct * 1.7 * 4096`: a page's 57 keys hash to up to 57 distinct
  hash pages, bounded by the hash file, and 1.7 record pages of 124-byte records (rev 2.13; the first version counted one hash
  page and left the 190 MB hash out). When `z faults per read < theta_sus` and that bound is under `B/2` for 3 consecutive
  10 s measures, the raw form would fit with room to spare.
* **The trial** then confirms rather than assumes, **on the refilled state and not on the fill** (rev 2.3). The gate goes back
  to 1 (sidecars stay attached) and the census runs on the raw form, whose hot pages are cold: they were not touched while the
  rung was COMPACT and the z pages displaced them. The trial is judged only once the raw form has been refilled: when the trial's
  faults reach `1.2 * predicted_raw_pages` (E2's bound over 4096), or the per-interval fault rate has fallen for five
  intervals running, or `3 * predicted_raw_pages * t_f / threads` of wall time (capped at 30 s) has passed, whichever is
  first; then three evidence intervals decide. `phi_meas >= theta_enter` returns the gate to 2 and doubles the backoff, while a
  low reading leaves the rung at SUSPECT and starts `retain`. The first version judged after three 100 ms intervals, when a
  cold fill runs at about one fault a read and had refilled only 7.2k pages (29 MB): the trial could succeed for hot sets
  under 30 MB, 6% of the 468 MB E2 admits, and reverse the other 94% back to the form that cost 4 us a read for 0.73. A wrong
  trial costs the hot set's refault: the bound above makes it at most `B/2` of cold pages, 114k faults, 4.7 s of one thread's
  stall in the worst case, once per backoff.
* **A form that is not paying (E3).** Whatever the cause -- the cache grew, the keys compress worse than the sampler's 32
  groups said, a raw path beside the z reads enlarged the union (3.10), the hot set shrank -- the census in COMPACT measures
  `phi_meas_z` and the exit fires when `phi_hat_raw - phi_meas_z` stays under `saving*` for a dwell. It is the one exit that
  needs no model of why, and the only one that covers the union leak the gate cannot see.
* **Never per operation:** a read goes to one form for the whole of its epoch pin, a rung holds for `dwell`,
  and the only per-operation choice is the streaming maintenance reads of section 0.

### 4.8 The initial state, publication and reversal

* **Initial:** gate 0 at every open and create, whatever is on disk. `Auto`'s first tick is within 10 ms of
  the segment work's start; it reads S1 at once. Sidecars found at open are attached lazily, by the tick that
  first wants the compact form (an attach reads a partition's resident part, 0.76 MB at the suite's shape, in one
  sequential read, about 10 ms cold and 0.2 s for the store [D: 4,250 pages at a fault each is the bound];
  the tick does it, not the open and not a reader), so a restart in a
  bound environment pays only the attach, which is what `C_build` is for a sidecar already on disk (0.2 s), and reaches COMPACT in
  about a second of evidence plus the attach, where a rebuild takes 15-40 s.
* **Publish:** cells first (`ZCell::install`, a compare-and-swap from null; each partition's `warm` ran on the build thread before its
  install, 5.4), then `zgate.store(2, Release)`. A reader that sees the gate sees a complete cell or none (acquire load), and a
  partition without one is served raw and counted a `rawserve`, so partial coverage is a measured state and
  not a hazard.
* **Reverse:** `zgate.store(1)`; in-flight reads finish on their pinned pointers; after `retain`, each cell
  is swapped to null and its `Arc` pushed on `Shared::retired_z` under a fresh epoch
  (`retire_state`'s pattern: `bump`, `push`, `sweep`, `src/db.rs:25981-25986`), the mapping dropped once no
  reader is pinned at or before that epoch. **`retired_z` is swept wherever the other retire lists are** (rev 1.9): that
  pattern sweeps once, at the push, and a reader pinned at that moment keeps the item with nothing to sweep it later, since the
  writer's `leave` sweeps only `shared.retired` (`src/db.rs:12740-12755`, `14906-14909`) and the forms and snapshots lists are swept
  by `sweep_retired_forms` (`:14916`, called at `19538`, `20732`, `20743`, `23453`, `27000`). `sweep_retired_forms` sweeps
  `retired_z` too, the writer's `leave` sweeps it when it is non-empty, and the segment work's tick sweeps it, so the munmap
  and the unlinked file's blocks wait for the last pinned reader and not for the next retirement, possibly hours away. A
  `Snapshot`-isolation handle pins at `snapshot()` and holds until `release` (`src/db.rs:12798-12811`), so a held snapshot defers the
  munmap and the return of the disk for as long as it lives; the take-back test keeps `Snapshot` handles out of it and a
  second test holds one on purpose and asserts the deferral. Then one file a tick is unlinked on the idle sweep
  (`Maint::sweep_spares`' pattern, `src/db.rs:27559`: a freed inode is a device round trip here, 20 ms for
  forty megabytes, `CLAUDE.md` profiling). The cell is clearable, unlike a `OnceLock`, on purpose: an
  unlinked file stays allocated while it is mapped, and "disk is returned" would be false until the segment
  died.

## 5. LIFECYCLE

### 5.1 Build trigger

`Maint::z_step()` runs once a turn of the segment work's loop, after `shape_if_read` and before
`sweep_spares` (`src/db.rs:26012-26014`), and also from `Maint::collect`'s polling shape for the inline home
(`MaintHome::Here`, `src/db.rs:25714-25717`; `Always` works there, `Auto` is inert there: 4.1).
It reads one atomic, the published generation, and does nothing unless the segment set changed, a build
finished, or the controller's rung asks. When the rung is COMPACT-wanted (always for `Always`; building or
COMPACT for `Auto`) it picks the first live partition without a cell, not declined and not in flight, and
starts **one** build; the next starts when that one is collected. A partition lands under: a seal that
names it directly (`src/db.rs:20372-20414`), a drain's last seal (`drain_names_partition`), a promotion
(`apply_promotion`), a partition merge (`join_compact`, `src/db.rs:27682`), and an open. A seal's level-0
piece never gets one: pieces turn over at `l0_trigger` = 4 (`src/db.rs:1820`) and their life has the merge.
Not at a landing's critical path: the ordered index is composed there because the writer already holds
every key (2.2 ns a key against 20 ns for a later pass, `src/db.rs:5227-5229`), but a z build compresses
every record (0.25 us a record for one compress call at 0.5 GB/s, 0.55 us at the two calls a page costs) on every seal and merge of every store including the in-memory ones,
which breaks "the in-memory rows must not move"; only the *wanted* form is built, after the landing.

### 5.2 Thread, priority, cancellation

A plain thread named `supdb-zbuild` (the shape of `start_compact`, `src/db.rs:27654-27674`) that runs one *job* at a
time, a build or a warm (5.4), and first calls `idle_io_priority()` and `idle_cpu_priority()` unconditionally
(`src/db.rs:152-165`, `22566-22572`; today a merge calls the first only when `Options::background_io == Idle`, default
Normal, `src/db.rs:1825`): this is derived data, and the form is built when the store is already short of memory, which
is when taking cores from the readers costs most. It checks `cancel: Arc<AtomicBool>` once a page and once more
immediately before the rename. It unparks the segment work's thread when it ends (`src/db.rs:27670-27672`). Collected by
polling `is_finished()` in `z_step`, **never joined there**: a join on the thread everything lands through blocked every
landing for a seal's whole run (`CLAUDE.md` "A join that blocked the thread everything lands through").
`Maint::stop` (`src/db.rs:26285`) cancels and joins it; `settle` does not wait for it (a build takes seconds; tests hold it
with `hold_zpage_build`, 6.1); a panic is a failed build. It is not registered in `in_flight_input`
(`src/db.rs:26406-26412`), or `promote_ranges` would skip the range and a build would block promotions
(`src/db.rs:27254-27261`).

Cancelled by: `join_compact` before its retire loop (`src/db.rs:27715`) for a build whose target is among
`inputs`; `join_tier` (`src/db.rs:27108`); `stop`; a drop of the rung below building; the end of `retain`.
A promotion is not a cancel: it replaces the `Arc<Seg>` over the same bytes (`src/db.rs:27437`), so the
install resolves its target by `(id, end_seq)` plus the binding of 2.5, and the build continues.

**The windows between a build and a merge or a promotion** (rev 1.8). Cancel is a flag and `join_compact` sets it without
waiting, so three windows follow, and each is closed here. (1) A build past its last check still writes the resident part
and the trailer and renames its temp over the final name after `retire_seg` has run; the install's liveness check (5.3) then
fails and the finished file would be an orphan until the next open, and the take-back test's "the file set equals a store
that never built" would flake on it. So the collect step of a finished job whose target is no longer live (its `Arc` and its
`(id, end_seq)` both gone) renames the file to a dead name and queues it (5.6), and counts `build_orphaned`. (2) A
promotion unlinks the old name inline after its publish (`apply_promotion`, `src/db.rs:27409-27455`; `retire_seg`,
`27130-27142`), so a build that opens the segment by name between its spawn and its open finds `ENOENT` and counts a failed
build: the segment-work thread opens the input and hands the `File` over at spawn (2.6, step 1). (3) The take-back and the
damage unlinks are by name on an idle tick while the segment is still live, so `retire_seg`'s claim test cannot guard them,
and a rebuild of the same `(id, end_seq)` renamed into place before the queued unlink runs would be deleted by it. The
manifest spares avoid exactly this by linking the old manifest to a unique name before the rename (`manifest_write`,
`src/db.rs:8355-8385`; `Maint::sweep_spares`, `27543-27563`); a sidecar is retired the same way (5.6).

**A wasted-build budget** (rev 2.17). Each partition merge mints a new identity, so a rebuild follows every rewrite, and
5.6 derives that a store written across its key space rewrites every partition each cycle, at a rate that can exceed one serial
builder. The per-partition-per-entry cap bounds a partition's builds and not the total, so `Auto` keeps the outcome of its last
eight builds: when more than four were superseded (their target retired by a merge before they installed), building is
suspended for a `dwell`, counted `build_suspended`, and the rung stays on the raw form, which is today's behaviour. `Always`
does not suspend; it reports coverage as a covariate of every cell that writes (7.3).

### 5.3 Publish to readers, and why it is not a State publish

Install happens on the segment work's thread, in `z_step`, never on a reader's: (1) the target is still
live (`Arc::ptr_eq` against `self.segs()`, or the same `(id, end_seq)` and binding under a promotion);
(2) `ZSide::attach(path, &seg, random)` opens the final name, runs the checks of 2.7 (no page body but
sixteen sampled), maps it **with the store's current advice** (`Reader::advice_random`, `src/db.rs:13345`, as `Seg::open_full`
passes it to the segment and to the ordered index, `src/db.rs:5435-5437`, `5448-5455`: a sidecar attached in a scan phase
would otherwise sit on default readahead for its life; `zpages_advised` holds it, 6.1; rev 1.15), copies the resident part to
the heap, and requires the binding of 2.5, `bloom_sig` and the fence keys in the filter; (3) `seg.z.install(Arc<ZSide>)`, a
compare-and-swap from null on a pointer only this thread writes. It is **not** a `publish_segs`: a publish
starts the next state with a fresh forms table and a null scan snapshot, bumps the epoch and wakes the keeper
(`src/db.rs:26949-26975`, `25981-25986`, `27001-27003`), all of which an attach would reset for nothing.
"Nothing a reader follows moves" holds: the cell goes from null to a complete object once and from that
object to null once, each through the epoch.

### 5.4 Warm

The cold-file arithmetic (D7) is the reason: a divided read cell of 333k reads against a cold 175k-page file takes 0.45
faults a read, as bad as the raw form. After the cells are installed, when `Options::zpages_warm` (default on; the arm
`supdb-nocachezpagesnowarm` is the same engine with it off) and `headroom = B - resident parts - other sidecars' resident >
file bytes` (`B` is already net of `anon` and the margin: the first version subtracted `anon` twice, and so would have skipped
warm at about 50 records a page or fewer with room to spare, rev 2.13), the job issues `MADV_WILLNEED` over the mapping in
8 MiB chunks, skipping chunks `mincore` reports resident, stopping at the headroom. It reads at most the file once,
sequentially (0.72 GB; a few seconds at the device's sequential rate here [U: not measured; 7.3 s of 175k faults at 42 us is
the cost it replaces, D7]), and what it evicts is, by the controller's own decision, the raw form's pages.

**Warm runs on the build thread, not on the segment work's** (rev 1.5). The segment work's loop is `try_recv` a job,
`collect`, `shape_if_read`, `sweep_spares`, park (`src/db.rs:25995-26015`), and it is the thread every seal's landing, merge
collection, promotion and manifest publish goes through and the thread a flush's `Drain` ask is answered on (`handle` and
`answer`, `src/db.rs:26063-26083`). While it warmed -- 90 chunks of `WILLNEED` and `mincore` over 0.72 GB, a few seconds -- none
of those would run: a finished seal would wait, the frozen list (`FROZEN_CAP` = 3, `src/db.rs:10015`) would fill and the
writer would block on the oldest seal's readable landing, and a `flush` would wait out the warm. The first version said "on the
segment work's thread, at idle priority"; priority does not help a thread that is not turning its loop, and `ioprio_set`
applies to the calling thread (`src/db.rs:152-165`), so lowered there and not restored it would have demoted the manifest's
`fdatasync`s too. Warm is a job of the idle-class build thread (5.2): after its rename it maps the finished file itself and
warms it in chunks, cancellable at every chunk, counted in `warm_us`; a restart with sidecars on disk runs a warm job per
partition with nothing to build; and the segment-work thread then only attaches, installs and stores the gate. The warm
holds no epoch pin across its work (a pin held across a long job keeps every state retired meanwhile alive: the comment at
`src/db.rs:26052`, and `op()` at `26036`, `26097`, `26237`). It is a cost of the form the controller has already chosen, paid off
every read and every commit.

### 5.5 Attach at open, at promotion, and a dormant sidecar

`Db::open` lists the directory already (`src/db.rs:18905-18912`, `read_dir` for the `ord-` sweep); the same
listing yields the `zpg-` names. A name equal to a live segment's `(id, end_seq)` is **kept dormant**: no
mapping, no read. Every other `zpg-` name, `.tmp` and `zpg-dead-*` included, is removed (errors ignored, as the other sweeps
ignore them: `src/db.rs:18815`, `18910`; the binding is the second defence against an id handed out again,
`src/db.rs:18997`). `ZPages::Off` removes the live ones too: the user-visible undo. At promotion, `apply_promotion`
(`src/db.rs:27409-27455`) opens the new `Seg` over the same bytes and, **before** `publish_segs` (`:27449`),
attaches the sidecar again by `(id, end_seq)` if the old `Seg`'s cell was set: the resident part and a
sixteen-page sample, about a millisecond, rather than carrying a shared `Arc` across two `Seg`s. The file
carries no fence, level or name of the segment, so a closed fence invalidates nothing (the engine
re-opens an ordered tail's last partition at every promotion, `src/db.rs:27296-27304`).

### 5.6 Retirement on merge

`Maint::retire_seg` (`src/db.rs:27130-27142`) unlinks the `.sup`, then the ordered index unless a live
segment claims its name. It gains the sidecar by the same claim test: a promotion keeps `(id, end_seq)`, so
the retired and the live names address one file, and unlinking by name once took the ordered index of an
open segment and failed seven tests (`src/db.rs:27125-27129`). **The sidecar is renamed, not unlinked** (rev 1.8): the cell's
retirement (4.8) renames the file to a unique `zpg-dead-N` (a directory operation that frees no inode), the dead name goes on
the list the idle tick unlinks like the manifest spares (`src/db.rs:27543-27563`), not inline as the `.sup` is -- the second
large file per partition would double the merge landing's device round trips -- and `zpg-dead-*` is swept at open (5.5). A
rebuild of the same `(id, end_seq)` can then rename into the free final name without a queued by-name unlink able to delete it.
A merge's *outputs* get no sidecar from the merge: they are new ids, and `z_step` builds them when the rung wants. The
exposure window, a partition served raw between its landing and its install, is the build's **wall**, 0.65-1.7 s a 64 MiB
partition (rev 2.17: the first version wrote 0.3-0.6 s, which is the CPU, 8 s over 23 partitions; the 15-40 s of 7.1 over 23 is
the wall, one build at a time on four shared vCPUs), counted as `rawserves`; building inside the merge's `Emitter` from the
records it is already writing is stage 5 and is priced by what the exposure costs mix A.

**Rewrite rate against rebuild rate** (rev 1.11, rev 2.17). `merge_due` selects every range that holds `l0_trigger` = 4 aligned
pieces (`src/db.rs:12892-12931`), a seal cuts its table at every fence, and "every range that is over its bound merges in one
job" (`src/db.rs:13240-13248`), so a store written across its key space rewrites every partition in one job, and Zipfian
writes reach every range too (a far range draws 0.3% of the writes). A seal fires at `max(seal_bytes, store/(4 *
l0_trigger))` capped at a tenth of the store (`threshold_for`, `src/db.rs:13086-13096`): 32 MiB at small stores and about
101 MB at 10M keys, i.e. about 700k writes of mix A [D: ~140 B a write in the memtable; U]. Four seals are then 2.9M writes, 26 s
of mix A at its 112k writes/s, and each cycle rewrites about 1.5 GB (reads 1.3 GB, writes 1.3 GB, displacing z pages) and retires
all 22-23 sidecars. A serial builder at 0.65-1.7 s a partition needs 15-40 s to cover them again, the same order as the
cadence: at that write rate coverage hovers near half, and a covered partition is rebuilt almost as soon as it installs,
the 0.35 s of CPU a partition spent for a few seconds of life. What that costs `Always` is `rawserves` (the share of reads served
raw, a covariate of every A, F and lag cell) and 0.72 GB of device writes a cycle beside the merge's 1.3 GB; what it costs
`Auto` is bounded by the budget of 5.2, which stops building and leaves the raw form. The registered mix cells never reach a seal
(7.3); the lag sweep's burst does.

### 5.7 Crash windows

Derived, unsynced and unnamed by the manifest (`src/db.rs:8249-8270`; sidecars are not in `live_names`,
`:27516`, nor in `store_bytes_of`, `:21370-21376`, so the seal cadence does not move).

| window | what a crash leaves | what the open does |
|---|---|---|
| mid-build | `zpg-<id>-<end_seq>.zpg.tmp` | swept: not a live name |
| renamed, not yet attached | a complete final file | dormant; the first attach validates it |
| torn final file (unsynced; a name durable over torn data is a normal outcome here, as it is for the ordered index) | short, or a zero trailer, or zero pages | length, trailer CRC, resident CRC, identity or the sampled page CRCs refuse it: deleted, counted `rejected_damage`, raw serves |
| valid trailer, one torn page in the middle | everything checks at attach | the page's CRC at its first decode: fallback for that read, `damaged`, the segment work retires and rebuilds, at most three times a `Seg` (2.7) |
| a finished build whose target a merge retired first | a final-named file no live segment claims | renamed to a dead name by the collect step (5.2), swept at the next open if the crash came first |
| a dead name queued, crash before the idle unlink | `zpg-dead-N` | swept at open |
| segment retired by a merge, crash between the manifest (`src/db.rs:27714`) and the unlink (`:27715`) | an unnamed `.sup` and its sidecar | both swept (`src/db.rs:18813-18817`; the `zpg-` prefix rule) |
| merge outputs written, manifest not | unnamed outputs | swept; no sidecar can exist: builds target *landed live* partitions |
| promotion: link (`:27429`), manifest (`:27450`), unlink (`:27451`) | two names, one inode | the claim by `(id, end_seq)` keeps the file while either name is live |
| id reused after a crash | a swept orphan's sidecar under a re-handed id | the sweep removes it at open before ids are handed out; if the unlink failed, the binding refuses it |
| disk full, write error | a temp file | removed, `build_failed` counted, serving stays raw, retried with a backoff of 60 s doubling to an hour; not an error for the store (contrast the ordered index, whose write failure fails the seal) |
| power loss, data not written back, file the right length | zero or stale pages | per-page CRC and tag |

Failure injection is a hook, not a bet: `fail_next_zpage_build`, beside `fail_next_presync`
(`src/db.rs:21912`); and a build the test needs in flight is held (`hold_zpage_build`), not raced
(`CLAUDE.md` "A test that bet a seal would still be running").

### 5.8 Disk and device-byte cost, and the forms cache

* **Disk** [P]: +0.72 GB of pages and 17 MB of resident part and trailer at 10M: +0.634 bytes on disk
  per byte stored (`0.737 GB / 1.16 GB`), 1.304 -> 1.94 (1.88-2.01); device bytes per byte stored
  +0.634 for the pages written (`/proc/self/io` `write_bytes`, counted at dirtying time per folio:
  `CLAUDE.md` profiling; the page cache's folio composition for a file written in 1 MiB pieces is [U]):
  1.339 -> 1.97. `dir_size` counts allocated blocks of every file at any depth, each inode once
  (`bench/src/engines.rs:270-295`), so the suite sees the sidecar with no edit. In `Auto` the load writes
  none of it; the build's writes are in the pass that carried the regime change.
* **The build's I/O** (rev 2.6, rev 1.13). It reads the record region (1.24 GB) and the directory (40 MB) once,
  sequentially, through a mapping of its own on the descriptor it was handed, and drops each 8 MiB window behind itself:
  `MADV_DONTNEED` over the window of its own mapping, then `posix_fadvise(DONTNEED)` over the same range of its descriptor.
  Both, because `posix_fadvise(DONTNEED)` skips a page that is still mapped (the kernel's `invalidate_mapping_pages` leaves a
  folio whose reference count shows a mapping [U: from knowledge of the kernel, not run in this tree; `zresident` measures it]),
  and the pages the build has just consumed are mapped by its own mapping. Without the zap the build streams 1.3 GB through
  the LRU of a cgroup already at its limit, which is the opposite of the first version's "a few megabytes". `memmap2` has the
  call (`unchecked_advise_range` with `UncheckedAdvice::DontNeed`, `memmap2-0.9.11/src/lib.rs:866`; `unsafe` because it destroys
  data in a writable private mapping, and does nothing of the kind to a read-only shared file mapping, which the SAFETY comment
  says). Nothing in `src/` calls `fadvise`, `DONTNEED` or `PAGEOUT` today (grep), so the drop-behind is new code with a test of
  its own (6.3). Pages another mapping holds, the readers' hot raw pages, survive the `fadvise` by design. **The write side is not
  dropped**: the 0.72 GB the build writes arrives as new page-cache pages while the gate is still on the raw form, displaces raw
  pages for the build's whole wall (15-40 s at 10M), and is wasted if the build is then cancelled. It is kept on purpose, since
  those pages are the form's working set and the warm then needs less; its price is the readers' faults per read during the
  build, which the verdict reads (7.6, item 6) and the ski-rental price carries (`displacement_est`, 4.4). The merge's and the
  open's reads of raw input are not dropped either (the engine has no drop-behind anywhere); a COMPACT-only drop-behind in
  `compact_run`, so that merges stop evicting z pages, is a separate gated change, priced as an arm if the verdict shows merges
  displacing the sidecar (R19).
* **The forms cache.** The anonymous consumers sit in the same cgroup as the sidecar: `FormsState`
  (block tables, scan keys; per handle), the published copies in `State.forms`, the scan snapshot, and
  under writes the memtable chain (the live table plus `FROZEN_CAP` = 3 frozen, `src/db.rs:10015`). This
  move's scope is the nocache policy, whose rows are the bounded rows (the default and `supdb-cache256`
  are killed at 3M under the bound by the forms alone, `docs/engine.md:5920-5928`, and
  `scratchpad/ooc/forms-lead.md` is that lead), and for it there is no interaction: no forms are built.
  For the default policy: form builds read the partition through the wrappers of 3.9 and so are served
  from the sidecar; the budget is not coupled in this move. The arithmetic for the follow-up is on the
  page: a dense `Cached::Block` costs `keys + vals + 16 * entries` = 132-136 B a key
  (`src/db.rs:9737-9744`) against 71 B a key in a z page, so in the compact form a dense copy costs twice
  the page it was built from and re-deriving a block from its page (2-3 us) is cheaper than holding it; a
  `headroom` number published by the tick for `scan_cache_bytes` to read (it is read from each handle's
  cloned `Options` at four sites, `src/db.rs:16034`, `17033`, `22428`, `15775`, and a non-zero budget stops
  the commit's fill and leaves tables incomplete, `:15762-15776`, so it cannot be made dynamic without a
  test that flips it with complete tables) is the follow-up, not this move.

### 5.9 The reversibility ledger

| loss | size [P] | reversed by | what it costs to reverse | what shows it |
|---|---|---|---|---|
| disk bytes | +0.72 GB, +0.63 per byte stored | cell retired through the epoch (and swept: 4.8), mapping dropped, file renamed to a dead name and unlinked on the idle sweep after `retain` | one device round trip a file, 20 ms, off every path; a held `Snapshot` handle defers the munmap and the disk (4.8) | `a_take_back_returns_the_disk_and_the_file_set` (6.3): after a take-back the directory's file set and `bytes_on_disk_per_byte` equal a store that never built (to 0.001), `Snapshot` handles kept out; `Off` at open removes them |
| build CPU and I/O | 6-14 s CPU, 1.3 GB read, 0.72 GB written | cancel flag; niced; not repeated while retained; the wasted-build budget (5.2) | none for time already spent -- stated, not hidden | `zpage_builds` per partition; `build_cancelled`, `build_orphaned`, `build_suspended`; the flap test: at most one build per partition per entry |
| page-cache displacement by the build | the 1.3 GB it reads (dropped behind: 5.8) and the 0.72 GB it writes (kept), against a cache of 0.94 GB for the build's wall, 15-40 s [U] | the drop-behind of the reads; the writes are the form's own pages; cancel stops it | the readers' extra faults during the build, priced in the ski-rental price and wasted if the build is cancelled | the residency probe and the readers' faults per read through a build (6.3, 7.6 item 6) |
| in-memory read cost | +3 instructions | none needed | -- | callgrind; quick row |
| wrong-form CPU (4 us where 0.73 would do) | up to 5x a read | E1, E2, E3 and the trial; the memo | a trial: at most 5 s of one thread's stall, once per backoff, judged on the refilled state (4.7) | `a_square_wave_switches_at_most_once_per_dwell` (6.3) |
| absent-key reads | 0 (filter) | -- | -- | `the_z_reader_agrees_with_the_blob_on_every_key`, absent keys (6.3) |
| resident memory | 17 MB at 10M | cell retire | -- | `zpage_sidecars_at_end` gauge |
| rebuild on every merge | 0.35-0.5 CPU-s per 64 MiB partition rewritten, up to the whole store a cycle (5.6) | gate off stops builds; the budget of 5.2 suspends them; retired inputs take theirs | the exposure window of 5.6, 0.65-1.7 s a partition | `rawserves`, `build_suspended`, the lag cells in the verdict |
| the format, the magic, the browser reader, the raw segment | none | -- | -- | `tests/blob.rs`, `dict.rs`, `ranges.rs`, `segwriter.rs` unedited and green; `web/build.sh` bytes equal |

What is not reversible is the CPU and the device writes of a build already spent, and the cache warmth of
whichever form was left: after a take-back the raw pages must be refaulted. Both are bounded above and priced
in section 7 rather than claimed free.

## 6. TESTS, ARMS AND INSTRUMENTATION

### 6.1 The option, and every site it touches in the engine

| site | change |
|---|---|
| `src/db.rs:224`, beside `ReadAdvice` | `pub enum ZPages { Off, Auto, Always }`, `Clone, Copy, PartialEq, Eq, Debug, Default`; `#[default]` is `Off` through stage 5 and moves to `Auto` at stage 6 only under the gate of 7.8(b) (rev 3.6; `CLAUDE.md` "A path only one arm exercises is a path nothing tests": flipping the default is the test an arm never gets). Re-exported in the `cfg(not(wasm))` list of `src/lib.rs:99-103` |
| `src/db.rs:288` `Options`, fields end near `:1798` | `zpages: ZPages`; `zpages_warm: bool` (default true); `zpages_memo: bool` (default true); `zpages_seek_ord: bool` (default false: the arm that seeks by the heads, 3.4); `zpages_shared_counters: bool` (default false: the first version's counters, 6.4); `point_via_ord: bool` (default false: L1, 9.2); `#[doc(hidden)] zpages_inject: Option<Arc<ZInject>>` (default `None`; a test's seam for the measures of 4.2-4.3 and for the shrink limit, per store, never a process-wide static: a test binary runs many stores as threads of one process). `Options` is `#[derive(Clone)]` only, no `Debug` (`src/db.rs:287`), so nothing else is needed |
| `src/db.rs:1801-1926` `Default` | the seven defaults |
| `src/db.rs:8173` `check_options` | refuse `Always` with `writer_pins` off (the clearable cell is freed through the epoch, and an unpinned writer reads it unpinned, `src/db.rs:8178-8183` is the precedent). **`Auto` is never refused**: with `publish_in_background` off, with `writer_pins` off, or off Linux it behaves as `Off` and is counted (`auto_inert`) (rev 1.3, rev 2.18; 4.1). Tests: a refusal test for `Always` shaped like `tests/db.rs:587-620`, and `options_default_plus_each_inline_and_unpinned_set_opens`, which opens `Options::default()` over each of `{publish_in_background: false}`, `{writer_pins: false, publish_in_background: false}` and `{snapshot_keeper: true}`, so that the flip of stage 6 cannot turn the cases at `tests/db.rs:607`, `616`, `8355`, `9327`, `11712` or the suite's `supdb-inlinemaint` and `supdb-nopin` arms red by construction |
| `src/db.rs:2522` `Seg`; literals at `:5472`, `:5505`, `:5530` | `blob: SegBlob`; `z: ZCell`; `z_declined: AtomicU8` (the reason a partition is declined for the life of the `Seg`, 0 for none: 2.6.1, 2.7; rev 1.15) and `z_rebuilds: AtomicU8` (damage rebuilds so far, at most three); `foot: (u64, u64)` (H and R of 4.2) |
| `src/db.rs:10281` `Shared` | `zgate`, a one-word `#[repr(align(128))]` wrapper of an `AtomicU8` as `Slot` is aligned (`src/db.rs:6664`), `retired_z: RetireList<Arc<ZSide>>`, `zevents: ZEvents` (the segment work's and the build thread's events only: builds, attaches, retires, switches, declines; the per-read counters are on `ZStat`, 6.4) |
| `src/db.rs:6616` `Readers` | `zstats: Box<[ZStat]>`, one `#[repr(align(128))]` line a reader slot with the slot's index, written by the slot's handle alone with relaxed stores and summed by the accessors as `Readers::stat` sums the slots; it holds the per-read and per-scan counters of 6.4 and the census's `zdistinct` (rev 1.4, rev 2.11, rev 3.10) |
| `src/db.rs:6665` `Slot` | `zdown`, `zreads`, `zscans`, `zflt`, `zsflt`, `zwall`, `zcpu` (single-writer atomics, 56 bytes: the Slot's nine words are 72 and the line is 128, so seven fit exactly; the assertion at `src/db.rs:6704` becomes `size_of::<Slot>() == 128`) |
| `src/db.rs:13322`, `13372`, `18192` | `advise` also flips the sidecars' advice; `read_all` and `count` take the gate and call `read_all_z` / `count_z` for the partition |
| the sites of 3.9 and 3.10 | the `scan_at`, `values_at`, `exts_at`, seek, cut and cursor sites; `SegBlob` has no forwarder for any name that reads a record, so each site is written down (3.9) |
| `src/db.rs:25892` `Maint` | `zbuild: Option<ZBuild>`, `zrung: ZRung` (the selector's state, `src/db/zselect.rs`); `z_step()` at `:26012-26014`; cancel and join in `stop` (`:26285`) |
| `src/db.rs:27130`, `27409`, `18744`, `18905-18912` | `retire_seg`, `apply_promotion`, `Db::open`'s directory listing and sweep, as 5.5-5.6 |
| `src/db.rs:28116` `mod threading` | `shared::<ZSide>()` |
| `src/db.rs`, new hooks (`pub`, `#[doc(hidden)]`, beside `ord_rebuilt` `:23915`, `ords_advised` `:13364`, `hold_seal_landing` `:21933`, `fail_next_presync` `:21912`) | `zpage_counts()`, `zpages_advised() -> (usize, usize)`, `zpages_settle()` (waits for builds and warm, never for the controller), `hold_zpage_build(bool)` and `release_zpage_build_after(n)`, `fail_next_zpage_build()`, `zpage_fence_keys(partition)` |
| `src/db/zpages.rs` (new, about 1,300 lines without its tests), `src/db/zselect.rs` (new, about 600) | children of `db`, declared `mod zpages; mod zselect;` below the `use` lines at `src/db.rs:30-40`. The selector's core is a pure function, `ZSelect::step(state, inputs) -> (state, action)`, so the replay of stage 1d is a unit test of the shipping code and the transitions have one definition |
| `src/flatindex.rs:492` | `record_key_next`; and `pub(crate) const REC_VERSION: u32 = VERSION` |
| `src/blob.rs:882` | `Blob::record_regions`, `Blob::z_source_facts`, both `#[cfg(not(target_family = "wasm"))]` |
| `lib.rs`, `bytes.rs`, `format.rs`, `block.rs`, `index.rs`, `ordindex.rs`, `wasmapi.rs`, `web/` | unchanged |

### 6.2 The arms, and every site of the suite they touch

Arms (each an `Options` value over its row's default and nothing else: "an arm that keeps an old shape keeps
that shape's other settings with it", `CLAUDE.md`):

| arm | options | role |
|---|---|---|
| `supdb-nocachezpages` | the nocache policy, `ZPages::Always` | **the ooc row's arm**: the policy that survives the bound |
| `supdb-nocachezpagesnowarm`, `supdb-nocachezpagesnomemo` | as the first, `zpages_warm` / `zpages_memo` off | price the two levers inside one process |
| `supdb-nocachezpagesheads` | as the first, `zpages_seek_ord` on | prices the seek by the z fence against the ordered index's heads (3.4; rev 2.5) |
| `supdb-nocachezpagessharedcounts` | as the first, `zpages_shared_counters` on | the first version's counters on `Shared`, kept to price the per-slot line at three threads (6.4; rev 1.4) |
| `supdb-nocacheordread` | the nocache policy, `point_via_ord` on | stage 1b: L1, the point read through the ordered index (9.2; rev 3.3); off by default, chosen by hand |
| `supdb-nocachepart<N>` | the nocache policy, `partition_bytes` set where the hash's power-of-two step lands at 0.62-0.75 load | stage 1c, optional: prices the hash sizing of 9.1 (L0'; rev 3.2); `Policy` gains the field, which the suite hard-codes today (`bench/src/engines.rs:3330`) |
| `supdb-autozpages`, `supdb-nocacheautozpages` | `Auto` | stage 4, until the default flips |
| `supdb-nozpages`, `supdb-ingestnozpages`, `supdb-nocachenozpages` | `Off` | introduced when the default flips to `Auto`, so the old shape stays an arm and the quick row's "Auto in memory never fires" has a same-process pair |

Arms are introduced with the stage that needs them. Two of the first version's are dropped until a verdict needs them (rev
3.12): `supdb-ingestzpages`, an inert control whose job `an_auto_store_that_fits_builds_nothing` and a buffered-arm twin of it
do in a test without the fifteen suite sites below, and `supdb-zpages`, `Always` on the default policy, which is killed under
the bound at 3M (R17) and in core is predicted at 0.15-0.24x of raw (7.3), so it prices a wrong choice. `bench ab` at the sizes
the default policy survives pairs `supdb-nocachezpages` with `supdb-nocache` instead. `supdb-inlinemaint` and `supdb-nopin`
(`bench/src/engines.rs:3480`, `3486`) inherit `Auto` as inert and are held by `options_default_plus_each_inline_and_unpinned_set_opens`
(6.1).

`Policy`'s new field is `Option<supdb::ZPages>`; `None` follows the engine's default, so every arm that
pins nothing inherits the flip by `..Default::default()` (`bench/src/engines.rs:3513`), which is also why
changing the default changes what `supdb-nocache`, `-cache256` and `-noadvice` measure at ooc: a definition
step for those series, announced in the commit and in `docs/engine.md`, and `supdb-nocachenozpages` is
the continuity arm.

Suite sites, all in `bench/src/engines.rs` unless named. The compiler enforces **four** (s1, s2, s3, s5);
the rest are enforced by nothing but the tests named in 6.3, and `supdb-rankrecords` is filed under the
buffered guarantee at `:4987` though `create_rankrecords` is durable (`:3021-3029`) -- the silent kind of
miss these sites invite.

| # | site | change |
|---|---|---|
| s1 | `struct Supdb`, after `bglag` (`:652`) | `zpages: Option<supdb::ZPages>`, `zwarm`, `zmemo`, `zseek_ord`, `zsharedcounts`, `ordread` flags, documented with the arm names |
| s2 | `struct Policy` (`:657`, `:977`) | the same fields |
| s3 | `impl Default for Policy` (`:990`, `:1065`) | `None` / defaults |
| s4 | constructors after `:1488` | `create_nocache_zpages`, `create_nocache_zpages_nowarm`, `create_nocache_zpages_nomemo`, `create_nocache_zpages_heads`, `create_nocache_ordread`, and the `auto` and `nozpages` set as their stages need them; models `create_noadvice` (`:3155`), `create_nocache` (`:3168`), `create_ingest_bglag` (`:1478`) |
| s5 | `with_policy` (`:3191`) | the destructure (`:3285`), the `Options` literal (`:3294-3514`, `zpages.unwrap_or(supdb::Options::default().zpages)` as `:3422` and `:3479` do), the `Supdb` literal (`:3618`) |
| s6 | `name()` (`:3681-4196`) | a clause before the final `match (partition, drain, advice.is_some(), block_cache)` at `:4184-4195`, which would otherwise name the nocache + `Always` policy `supdb-nocache` |
| s7 | `counters()` (`:4234-4334`) | the counters of 6.4, gauges `*_at_end`, read from `Shared` atomics only |
| s8 | `guarantee()` (`:4833`) | durable names in the durable group (`:4835-4925`), `ingest` names in the buffered group (`:4926-4993`) |
| s9 | `open()` (`:5000`) | one dispatch line a name (models `:5088-5089`) |
| s10 | `OOC_ARMS` (`:4823-4831`, `[&str; 7]`) | the durable ooc names; the buffered group is not in it (`:4819-4821`). `ARMS` (`:4799`, 12) gains **nothing** until the flip: CI's quick job runs every arm of `ARMS` (`.github/workflows/ci.yml:62-79`) |
| s11 | `mod tests` (`:5212-5424`) | B1, B2 of 6.3 |
| s12 | `bench/src/run.rs:3003` | the forced arm in the literal arm loop of `threaded_readers_read_the_loaded_store` |
| s13 | `bench/src/figures.rs:45-79` | `style()` and `pretty()` entries (an unlisted arm draws as a grey comparator labelled `?`), and a test over `every_arm()` |
| s14 | `bench/DESIGN.md:208-211`, `243-249`; `bench/README.md:48-49`; `bench/src/main.rs:8-49` | the arms table, the ooc roster sentence, the usage text |
| s15 | `Supdb::sync` (`:4343-4360`) | under `Always`, `sync` calls `zpages_settle()` after `flush()`; otherwise a flush returns before the segment work's later steps and the forced arm silently measures the raw form |

Runner prerequisites, which are stage 0a deliverables and not stage 2's (rev 3.3: the cheapest engine experiment, stage 1b,
waited behind the most expensive stage for them): `bench ab` takes `--ops-div` (it builds its plan at `Scale::Quick` and never
reads the flag, `bench/src/main.rs:249-256`, whose `Plan::new` gives a divisor of 1, `bench/src/run.rs:264-267`, so `ab
--bound` runs the whole counts, 2.7 h against about 15 min for the same 10M pairing [D from the pass times of M-table row
`rung1.log`]); `load_bpb` and `disk_bpb` stop being NaN unless `passes.load` (`bench/src/run.rs:1674-1683`; a pairing of the
sidecar's disk and device bytes needs them without building the shuffled store); and a `--compress` flag on `ab` **only**,
never on `run` (it would redefine every banked quantity: "a bisect across a change in the measurement", `bench/CLAUDE.md`), so
the incompressible control is a pairing and not a row. `one_pass` refuses to measure a forced arm whose sidecars are not whole,
in the shape of its byte checks (`bench/src/run.rs:1430-1436`, `1517-1522`, `1607-1612`): after the load
`zpage_sidecars_at_end == partitions`, after the read phase `zpage_reads > 0` for `Always` and `== 0` for `Off`.

### 6.3 The tests

All under `profile.checked` (`scripts/check.sh`; overflow checks and debug assertions on), sizes in
thousands of keys. The existing tests are twinned where they hold the contract a second form must keep, and
**not parameterized** where they hold the format: `tests/blob.rs`, `tests/dict.rs`, `tests/ranges.rs`,
`tests/segwriter.rs` stay as they are. Every twin ends by asserting the path was taken (`zpage_counts().reads
> 0`) and, for the `Off` twin, that it was not -- "a path only one arm exercises is a path nothing tests".

`tests/zpages.rs` (new file; its own process, so a checksums-off case cannot race the others,
`tests/checksums_off.rs:1-13`; fixtures copied, as each test file copies its helpers: `varlen`
`tests/segwriter.rs:47-75`, `fixed` `:78-95`, `postings` `:826`, `compressible` `:1321`, the suite's record
shape):

| name | asserts |
|---|---|
| `the_z_reader_agrees_with_the_blob_on_every_key` | through a `Db` forced to partitions, every key's `read_all`, `count`, `stored_bytes` equal to `Blob<MmapBytes>`, to a `Blob` over a copying source and to `SparseBlob`, on the three fixtures; absent keys below the first, above the last, between two records of a page, one byte either side of every fence key, a proper prefix of a fence key, a key longer than a fence key it extends |
| `the_z_reader_agrees_with_the_blob_on_every_scan_window` | start in {each fence key, its predecessor and successor, key + `+`, empty, `a`, `zzz`, `0xff` x 8} x limit in {1, 2, 3, 5, 17, 56, 57, 58, 114, `usize::MAX`}: the stream, the returned count and `scan_counts`, through the writer, a `Latest` handle and a `Snapshot` handle, against the model and against the raw `Blob` |
| `windows_that_cross_partitions_and_covered_with_uncovered_agree` | at least three partitions; a store where partition `i` has a sidecar and `i+1` does not (`hold_zpage_build` after `k`) |
| `a_stored_page_parses_in_place_and_a_misaligned_buffer_is_refused_not_missed` | incompressible fixture; full-form records (`compact_records` off) and block-backed runs; `STORED` pages at a pointer that is 8-aligned; a deliberately misaligned buffer makes the reader refuse and count, never answer absent (risk R6) |
| `records_larger_than_a_page_are_read_from_the_raw_segment` | keys of 200, 1,000, 5,000 bytes, a 9,000-byte value; `ZNOPAGE` entries; scans across them; `nopage_reads` and `nopage_scans` |
| `commit_markers_in_a_direct_segment_are_not_records` | a partition promoted from a direct run (markers inside the record region) and the same keys rewritten by a merge: equal decoded groups; `record_key_next` against `parse_record` on both |
| `a_run_of_every_shape_reads_the_same` | runs of 1, 2, 17, 200, 1,500 values; FIXED and prefixed together; zero-length values; an empty run; a run over `inline_bytes` straddling the block; tombstone-only keys; a key whose tombstone masks a 1,500-value run (`tests/blob.rs:70-78`, `tests/segwriter.rs:496-519`) |
| `the_gate_declines_what_it_should_and_stores_what_does_not_compress` | random values at the cell's shape: built as `STORED` pages (32 records a page), read in place, faults and rate as 7.3 registers; a budget large enough to hold the raw form (the injected `B`): declined, counter, no file, bytes on disk equal to `Off`'s; the suite shape builds; fill and calls a page are asserted to be in the range the doc predicts (rev 2.10) |
| `fill_group_is_total` | records of 200, 1,000, 4,056-4,072 (incompressible), 4,500 and 5,000 bytes, alone and among compressible neighbours: `m = 0` is never formed, the loop ends in at most twelve turns, `NOPAGE` only where the record cannot fit, a lone incompressible record that fits `ZCAP` is `STORED`; the pair `m = 2, len(c) = 9,000` is a fixture (rev 1.6) |
| `a_bloom_layout_change_cannot_attach_an_old_sidecar` | `BLOOM_SIG` is pinned by a literal CRC32C over the words of a filter built from 4,096 fixed keys (the failure message says to bump `BLOOM_SIG` and the sidecar `version`); a sidecar whose `bloom_sig`, `bloom_probes` or `bloom_bits_per_key` is altered, or whose filter words are permuted under a good CRC, is refused at attach (`rejected_identity`) and never answers "absent" (rev 1.2) |
| `a_finished_build_whose_target_died_leaves_no_file` | a build held with `hold_zpage_build` is released after its target merged away: the finished file is renamed to a dead name and gone after the idle sweep, `build_orphaned == 1`; a promotion between spawn and open does not fail the build (the `File` was handed over); a rebuild renamed into the final name is not deleted by a queued dead-name unlink (rev 1.8) |
| `rebuild_after_damage_is_bounded` | an injected builder that always writes one bad page: `verify_page` fails the build at step 6; a sidecar damaged after attach is rebuilt three times with the doubling backoff, then `declined_damage` and `Seg::z_declined`, with no loop (rev 1.7) |
| `a_flipped_byte_anywhere_in_a_sidecar_is_never_an_answer` | every seventh byte (the template `tests/segwriter.rs:618-686`), each flip followed by a read of every key: never a wrong value; every flip in a covered byte counted as a rejection or a fallback; truncation at every 997th length never panics (`src/flatindex.rs:2716-2750`); a zeroed page; a file of zeros of the right length; with block checksums on, and in `tests/zpages_checksums_off.rs` with them off (the page CRC must not depend on `block::CHECKSUMS`, which is process-wide and which every bench arm sets off) |
| `a_sidecar_of_another_segment_is_refused` | the hard case: build `S1`, set its sidecar aside, apply updates to the same keys (different values) and merge, so the new partition has the same key count; plant `S1`'s sidecar under the new partition's name and under a re-handed id with the same end-sequence: reads return `S2`'s values, `rejected_identity == 1`, `rebuilt == 1` (`OrdIndex::open` would accept it: it checks key count alone, `src/ordindex.rs:417-419`) |
| `a_retired_sidecar_outlives_its_last_reader_and_no_longer` | a pinned handle reads the old sidecar's values after a merge lands, a new handle reads the new, and the mapping disappears from `/proc/self/maps` at the pinned handle's release, not at the next retirement (`retired_z` is swept at `leave`: 4.8); a `Snapshot` handle held across the landing defers it, which the test asserts (rev 1.9) |
| `a_take_back_returns_the_disk_and_the_file_set` | after a take-back and `retain`, the directory's file set (no `zpg-dead-*` left) and `bytes_on_disk_per_byte` equal a store that never built (to 0.001), with no `Snapshot` handle held; `Off` at open removes the live sidecars |

`tests/db.rs`:

| name (new) | template | asserts |
|---|---|---|
| `the_oracle_holds_with_zpages_forced_on` and `..._with_a_sidecar_damaged_before_each_reopen` | `oracle_in` `:389-477` | the model oracle (97 short keys, deletes, crashes at 13%) with `Options { zpages: Always, seal_bytes: 2 << 10, .. }` |
| twins of the plain, cache, upkeep-on-a-thread, pieces-only and budget scan-model tests, and of `fold_model_with` `:2733`, `extend_order_model_with` `:6546`, `carry_model_with` `:6745` | `overlay_model_with` `:4373-4511` gets a trailing `zpages: bool`; callers `:2593-2647` | each `m.check` on the writer and on a handle; the nocache twin sets `scan_block_cache: false` so the merge and bulk-walk paths carry the form |
| `scan_path_arms()` gains `("zpages", ..)` and `("nocachezpages", ..)`; its type goes from 7 to 9 | `:9605-9652`, consumers `:9523`, `:10135` | a live write over the frozen table is one key on every scan path |
| `readers_on_threads_keep_answering_through_seals_and_merges_with_zpages` | `:4681-4780` | 300 rounds over 3,000 keys with partitions of 64 KB: a stale sidecar after a merge is "a version that went backwards" (`:4734`) |
| `reader_threads_over_blocks_on` wrapper with a trailing `Options` argument (nine wrappers `:5511-5642` already) | `:5654-5857` | |
| `a_reader_handle_keeps_the_retired_sidecar_until_released` | `:4564` | |
| lifecycle twins | `:784-880` damaged / missing and counted; `:1145-1179` survives promotion and reopen; `:2251-2287` a merge takes the sidecar with the segment (count `zpg` against `.sup`, no orphan, without reopening); `:2290-2348` advice reaches the mapping (`zpages_advised`); `:8751-8816` an unnamed segment is swept with its sidecar; `:1279` a crash before the manifest keeps the pre-merge store; `:12538-12609` `.sup` and `.oidx` byte-identical with the option on and off; `:12620-12700` a load nobody reads builds nothing; `:12311-12339` a failed sidecar build **does not** fail the flush (the opposite of a failed presync, whose error propagates) and is counted | |
| `an_auto_store_that_fits_builds_nothing` | | in memory: no build, no read through the form, no `zpg` file, directory identical to `Off`'s, `zgate == 0`, no thread beyond the segment work (`/proc/self/task`) |
| the selector set | `ZInject`, replay | `auto_builds_when_the_measure_says_blocked_and_the_limit_is_below_the_footprint`; `auto_serves_raw_until_the_sidecar_is_whole`; `auto_returns_to_raw_when_the_limit_is_lifted_and_gives_the_disk_back`; `a_skewed_workload_that_fits_never_builds`; `a_square_wave_switches_at_most_once_per_dwell`; `the_trial_judges_the_refilled_state` (a 100 MB hot set after COMPACT returns to raw, a 600 MB one stays); `two_stores_in_one_process_do_not_share_a_measure`; `a_cold_start_is_not_a_capacity_miss` (first-touch faults with no refaults never build; refault share at or over 0.5 does); `a_mix_with_commits_reaches_evidence` and `a_scan_only_handle_reaches_evidence` (windows with commits count for `phi_meas` and not for `t_f`; scan windows count; `mixed_windows` is counted); `the_ski_rental_price_is_paid_before_a_build` (excess against `C_build`); `a_cancelled_build_counts_as_a_switch`; `a_form_that_stops_paying_is_left` (E3); `a_store_far_past_the_cache_still_builds` (a model-only synthetic 50x store: the two-population raw model keeps the predicted saving over `theta_enter`, D9); `a_neighbours_pressure_opens_the_build_clause` (rev 2.16); `builds_suspend_after_four_of_eight_are_superseded` (rev 2.17). Each is a trace fed to `ZSelect::step` first (stage 1d) and to the engine second (stage 4) |
| `auto_degrades_where_it_cannot_run_and_always_is_refused_beside_an_unpinned_writer` | `:587-620` | `Options::default()` with `Auto` over each of `publish_in_background: false` and `writer_pins: false` with `publish_in_background: false` opens, `zgate == 0`, `auto_inert` set; `Always` beside `writer_pins: false` is refused (rev 1.3) |
| `options_default_plus_each_inline_and_unpinned_set_opens` | `:607`, `616`, `8355`, `9327`, `11712` | the option sets those tests use, opened over `Options::default()` once the default is `Auto`; the test that makes the flip safe by construction (6.1) |
| `the_compact_form_touches_no_raw_page` | debug-assertions build | `raw_touches == 0` per operation kind **and per shape of 3.9's table** on a covered partition -- a scan over a range with a level-0 piece (`scan_merged`), a scan with an unsealed key inside the walk window (`cut_at`), variable-length keys and random sixteen-byte keys whose heads do not name a key (`exact_at`), a key straddling a `ZNOPAGE` entry, a merge landing's `same_blocks` -- on the writer and on a `Latest` handle, against a raw model; then the residency check below. The first version's test would have passed on the suite's drained cells and failed on a mixed store (rev 1.1, rev 2.5) |
| `a_stray_raw_read_in_two_hundred_does_not_collapse_the_compact_form` | `src/ordindex.rs:100-107` | one raw-path operation (a `Dirty` read through a level-0 piece's key, or a count on an uncovered partition) per 200 form reads under a bound: faults per read do not rise by more than the stray reads' own pages. This is the footprint-sum rule asserted, not assumed |

`tests/leak.rs:206-228` gains `a_zpages_life` in the loop of lives (a store with the form, readers with their
scratch and memo, a merge replacing a sidecar, closed with a retired sidecar unswept), and a check of
`/proc/self/maps` that no mapping of the store's directory survives the drop, because the counting allocator
does not see a mapping. `src/db/zpages.rs` carries unit tests: `record_key_next` against `parse_record` on
every fixture record and 100,000 random ones; `fill_group` fits `ZCAP` for every group of every fixture;
build-then-decode equals the images for every page; the trailer and page encoders against a hand-written
decoder in the test (a second definition of the layout *in a test*, on purpose); `fill_group` is total; the `BLOOM_SIG` pin;
`size_of::<ZStat>() == 128` with the slot's index, and three threads reading with the summed counters equal to the reads.

Residency, the instrument the fault counts cannot be: `bench/examples/zresident.rs` writes through
`Db::create` and `flush` (a landed segment skips the index verify, `src/db.rs:5389-5398`; a reopen verifies
the whole key section, `:18917-18924`, and would make it resident), drops both files with
`posix_fadvise(DONTNEED)`, runs N compact-form reads, then `mincore`s a fresh mapping of each: the raw
segment's resident pages stay at the open's few and the sidecar's grow. The count of faults tells that
something was read; this tells which file. It also samples the raw segment's resident pages and a reader's faults per read
through a build and its drop-behind (5.8; rev 2.6), and the hash file's residency after a pass (rev 3.2). It is
`#[cfg(target_os = "linux")]` with the other residency readers (2.9).

Bench side: **B1** `zpages_arm_pairs_differ_by_the_mode_alone` (models `bench/src/engines.rs:5306-5358`,
`5360-5423`), which also asserts `guarantee(name)` -- neither neighbour does; **B2**
`a_forced_arm_reads_through_the_form` (opens the arm, writes enough for partitions, `sync`, reads, asserts
`zpage_reads > 0` and `zpage_builds == partitions`; the reopen test `:5273-5304` writes one key and proves
nothing about a path); **B3** s12; **B4** s13.

### 6.4 Instrumentation

Counters, `Db::zpage_counts() -> ZCounts` (rev 1.4, rev 2.11, rev 3.10). **Per-operation counters** -- `reads`, `counts`,
`scans`, `filter_negatives`, `nopage_reads`, `nopage_scans`, `fallbacks`, `rawserves` (a covered partition served raw: the
exposure), `raw_scans` (3.10), `prefetches`, `memo_hits`, `memo_misses`, `decodes`, `verifies` -- live in `ZStat`, one
128-byte line per reader slot (`Slot`'s own discipline, `src/db.rs:6655-6685`): written by the slot's handle alone with a
relaxed `store(load + 1)`, no locked instruction, and summed over the slots by the accessor as `Readers::stat` sums the
slot statistics (`src/db.rs:6807`). Three reader threads at 100-300k reads a second bumping four shared words a read is the
failure `CLAUDE.md` records in "A slot table whose slots share a line" (every scan through a handle bumped seven to thirteen
shared statistics and four handles read the threaded scan mix at 0.46x of LMDB, 0.91x with the counters off), and it
would have sat inside the arm being measured and hidden itself behind the counters. **Events** -- `builds`, `build_bytes`,
`build_failed`, `build_cancelled`, `build_orphaned`, `build_suspended`, `warm_us`, `declined_gate`, `declined_small`,
`declined_small_bytes`, `declined_damage`, `declined_page_size`, `attached`, `rejected_damage`, `rejected_identity`, `rebuilt`,
`retired`, `unlinked`, `switches`, `trials`, `trials_back`, `mixed_windows`, `auto_inert` -- are `AtomicU64` on `Shared`,
written by the segment work and the build thread only, **where the action happens** (the lesson of `ord_rebuilt`, summed only
in `Db::open`, `src/db.rs:18927`, `19053`, so a rebuild inside a running store is invisible to it). Gauges are named
`*_at_end` because the runner sums a counter over a pass's two stores (`bench/src/run.rs:1657-1662`): `sidecars_at_end`,
`pages_at_end`, `resident_bytes_at_end`, `rung_at_end`. Read from `Shared` and the slots alone: a counter accessor that calls
`take_back_upkeep` changes the next phase (`src/db.rs:21153`, `21161`; `bench/src/run.rs:1743-1745`). `raw_touches` is
compiled under `debug_assertions` only (3.9): an instrument in the measured path is priced by taking it away, and this one is
the test's, not the suite's. The first version's shape is kept as the arm that prices the choice (`Options::zpages_shared_counters`,
`supdb-nocachezpagessharedcounts`, which bumps the same counters on `Shared`): its pairing against `supdb-nocachezpages` at three
threads is where the contention shows, because callgrind cannot see a contended line, and the pairing is checked to differ in that
option alone before a verdict is read (`CLAUDE.md`, "An instrument in the measured path is priced by taking it away").

Environment: `SUPDB_ZPAGES_TRACE=1` (a stderr line for each controller decision: rung, `phi_hat`,
`phi_meas`, `t_f`, `t_hit`, `B`, `H`, `R`, the action) and `SUPDB_ZPAGES_BUILD_STEPS=1` (a `zbuildsteps`
line a build: keys, pages, records a page, fill, compress calls a page, and microseconds in read, parse,
compress, decode-verify, CRC, write, rename: the hand-off-by-hand-off stamping `SUPDB_DRAIN_STEPS` made the
rule, `CLAUDE.md` profiling). The suite already turns timers on for every arm under `SUPDB_LAG_COUNTERS`; a
pairing of two arms is checked to differ in the option before a verdict is read, and the pairing script
exports neither (`CLAUDE.md` "An instrument in the measured path is priced by taking it away").

The branch's exact price: `valgrind --tool=callgrind --collect-atstart=no --toggle-collect=<the exact
demangled monomorphised name of Reader::read_all from the function list>` over 100,000 uniform reads of a
partitioned store of 10,000 keys, the binary before and after; the difference is the three instructions, and
the rate pairing (two binaries alternated in one sitting, ten and thirty thousand and three hundred thousand
keys, and mix C) can only say it is not more.

## 7. PREDICTIONS, VERDICT, STOP RULE, DONE WHEN

All numbers below are [P] (written before the engine's code, and before M17 where they sit in 7.1-7.2) or [P2] (written in the
revision, after M17 and the reviews) unless a bracket says otherwise. The offline Python figures (M13) and the 200k smoke
probe (M14) preceded both, and 7.1 stands on them; the first version's "before any code or probe run exists" was wrong about
that (rev 3.14). Ranges are the claim; a value outside one is a refutation, not a surprise to be explained afterwards.

### 7.1 Sizing (stage 0b)

| quantity | prediction | derivation |
|---|---|---|
| records per 4,072-byte payload, 124-byte images of the suite's shape | 57 (52-62) | M14: 60.0 on 116-byte records and a 4,088-byte payload; plus eight framing bytes a record that repeat verbatim (a byte or two compressed each) |
| page bytes per key | 71.9 (66-79) | `4096/57` |
| the sidecar at 10M keys | 0.72 GB of pages (0.66-0.79) plus 17.4 MB resident part and trailer | 175k pages |
| fill (payload bytes / 4,072) | 0.97 or better | one growth round |
| compress calls a page | 1.6-2.6 | start at the last page's count; trim or grow once |
| decode of one page, cache-hot | 2.5-3.5 us with the engine's unsafe codec. The standalone probe ran that same codec (7.2a, point 3), so its 3.6-3.8 us is no upper bound on decode; the planned `bench/examples` binary would link the *safe* codec through the dev-dependency (`lz4_flex = "0.11"` with default features, `bench/Cargo.toml:41-42`, unified by cargo for targets that build dev-dependencies), so stage 0b pins the features, prints which codec ran, and builds the cross-check both ways [rev 2.14, rev 3.14] | M13, M14 |
| build CPU at 10M | 8 s (6-14) | 2.7 GB of LZ4 input at 0.5 GB/s (5.4 s), parse and copy 0.4 s, decode-verify 0.6 s, CRC 0.3 s, filter 0.3 s, write 0.3-1.7 s by the host's page price |
| build wall at 10M on 4 vCPUs shared | 15-40 s | |
| bytes written / read by a full build | 0.74 GB / at most 1.3 GB | |
| records per page by key shape [P2] | sequential decimal (the suite's) 57 (52-62); sixteen random decimal digits 48-52; sixteen random bytes 48-52; 36-character UUID text 40-46 | byte accounting: a sequential key costs about 5 compressed bytes (a match against the previous record's key, 124 bytes back, and two literal digits), a random one 16, so +11 a record: `4072/(70.7 + 11) = 49.8`; UUID text is 32 hex characters of literals and dashes, about +31: 40. To be measured by stage 0b, not trusted |
| cache occupancy at 10M and `B` = 937 MB [P2] | 79% (57 a page), 89% (50), 99% (45), 111% (40) | `(10M / rpp * 4096 + 17 MB) / 937 MB` (D10); the scan claim needs the heads out of the working set (3.4) to hold at 50 |

**Stage 0b stops** (rev 1.12, rev 2.13, rev 3.14) if records per page are under 36 on the suite's shape: the break-even for
beating `lmdb-nordahead`'s single-thread reads is `phi_z <= 0.198`, i.e. `W_z <= (B - f)/(1 - phi_z) = 1.147 GB`, i.e. 35.7
records a page (D10). The first version wrote `1.15 * B_z = 1.06 GB`, 38.6 records, which is the figure for `B - f` of
0.84 GB, not 0.92, and so stopped about three records early for the point-read claim. Between 50 and 36 the claims fall away
in this order as records per page falls: C1 (fits) at 45, where occupancy reaches 99%; the scan 2x at 43.1 with the heads
resident and 39.3 without; C4's 1.5x at 40.1; parity with LMDB at 35.7 (D10). **For the other key shapes the stage reports and never stops**:
the gate of 2.6.1 decides per partition, and the grid says what the form is worth where keys are less redundant -- at 40
records a page one thread is still 1.6x LMDB at 10M (D9's sensitivity), at 36 it ties.

### 7.2 The physics under the bound (as registered before M17, for the standalone probe: 10M, 1 GiB, 1M uniform reads a pass, three passes after a `DONTNEED` reset)

| layout | faults per read | reads/s on one thread |
|---|---|---|
| raw pages and a 190 MB file touched once a read (the hash) | 0.36-0.45 every pass (engine: 0.413-0.463 [M]) | 52-58k |
| raw pages, no hash file (L2's footprint) | 0.25-0.32 every pass | 70-85k |
| z pages, no hash | pass 0 0.15-0.20 (derived 0.1725, D7), pass 1 at most 0.03, pass 2 at most 0.02 | pass 0 75-100k, pass 2 250-330k at the probe's CPU of 3.1 us [M14] |

**Refuted** if z pass 2 faults exceed 0.10 or its reads/s fall under 100k, or the raw-without-hash layout
reads 0.38 or more (the hash is then not what crowds the records, and the reasoning of D4 and D8 is
wrong), or the raw-with-hash layout falls outside 0.33-0.48 (the probe does not reproduce the engine's
cell and nothing it says about z can be read until it does).

#### 7.2a After the result (added after M17; the table and the refutation paragraph above are as registered)

M17 is the same three layouts at 10M under the bound, run as `zprobe` v1 (its author's pre-registration and the result are in `scratchpad/zprobe/prereg.md`): fixed 116-byte records, one
thread, uniform reads, no overlay, no tombstones, no multi-value keys, no first-decode CRC. It is not the cell these ranges
were written for -- its records are 117 B a key where the engine's are about 124-126 -- so each line says
whether it tests a range or only bears on one.

| registered above (before M17) | observed (M17) | reading |
|---|---|---|
| raw + hash: 0.36-0.45 every pass, 52-58k/s | pass 0 0.532 at 46.1k/s; passes 1-3 0.423 / 0.422 / 0.414 at 54-58k/s | the warm passes are inside; **pass 0 is outside, and "every pass" was the error**: a pass over a cold cache fills the cache. After the fact [D]: with a 957 MB cache the hash file's 48,640 pages and 185,000 record pages fill it; the fill takes about 298k reads and costs one fault for each of its 233,640 pages; the other 702k reads fault at the steady 0.423; `(233.6k + 297.0k) / 1M = 0.531`, measured 0.532 |
| raw, no hash: 0.25-0.32, 70-85k/s | 0.182 / 0.182 / 0.182 at 113-117k/s | **missed low, and not a test of the range**, which was written for R of about 1.3 GB and observed at 1.17 GB. D2's formula at the probe's R is `1 - (937 - 5.7)/1170 = 0.204`, 0.02 above the measurement, so the cache the probe had was about 957 MB and not 937. Re-derived at the engine's R: `1 - 957/1300 = 0.264` [D]. The refutation line for this layout (0.38 or more) did not fire |
| z, no hash: pass 0 0.15-0.20 (D7: 0.1725), pass 1 at most 0.03, pass 2 at most 0.02; pass 0 75-100k/s, pass 2 250-330k/s | pass 0 0.166 at 96.7k/s; passes 1-3 0.000 at 262k / 280k / 264k/s | inside every range. D7 at the probe's file, `165,963 (1 - e^(-6.026)) / 1M = 0.1656`, measured 0.166: the cold-file arithmetic is right to three places |
| refuted if z pass 2 is over 0.10 or under 100k/s; raw without hash 0.38 or more; raw with hash outside 0.33-0.48 | 0.000 and 280k/s; 0.182; 0.414-0.423 | none of the three fired: **7.2 is not refuted at the probe's framing**, and the probe reproduces the engine's cell (0.41-0.46, 54k) |

What the result changes in the reasoning, and what it does not:

1. **D4's "at most" is wrong.** Removing the hash lowered the faults from 0.423 to 0.182, 0.241 a read, where the hash's
   footprint share is `199/1170 = 0.170`. The other 0.07 is not explained by the footprint model; the likeliest cause is
   hash pages the cache did not keep between touches [U: not measured; `mincore` over the hash file after a pass would
   say]. Two consequences, both [D] and neither a new prediction: L1 (9.2), which swaps the hash for the heads, may gain
   more than the registered 0.425 to 0.372 (about 0.33-0.37 if the heads lose pages as the hash does; stage 1b says); and
   L2 at the engine's R lands near LMDB's 0.267-0.274 and not clearly below it. The ladder's order stands: only a form
   that shrinks the records goes clearly under LMDB.
2. **`warm` (5.4) is part of the move, as D7 said.** A z file read cold faults its whole size once, 0.166 of a read over
   the first million; the cell the suite's divided pass runs, a third of that many reads, would pay 0.44.
3. **`t_z` (D5) is consistent with it, not confirmed by it.** The probe's 3.6-3.8 us ran the engine's own codec:
   `lz4_flex` with `default-features = false, features = ["std"]` (`scratchpad/zprobe/Cargo.toml`; the built fingerprint lists
   `["std"]`), which drops `safe-decode` and selects the unsafe decoder the engine ships (`Cargo.toml:15`;
   `lz4_flex-0.11.6/src/block/mod.rs:21-28`). It is therefore no upper bound on decode and there is no codec headroom to
   expect; the first version of this point said the probe had a safe decoder (rev 2.14, rev 3.14). It has no engine fixed cost,
   no filter, no first-decode CRC and a position search where the engine walks: the engine adds its raw read's 0.42 us over the
   probe's (0.73 against 0.31), about 0.05 for the filter and 0.2-0.5 for the walk, 4.4-4.7 us against D5's first 4.0 (D5
   carries the range now). If the engine's steady-state faults are nearer the probe's
   0.000 than the registered 0.04, one thread reads about 230-245k/s, the top of 7.3's range. **A result above 230k/s is
   read as the suite reads a cell that is better than predicted -- a broken measurement until someone looks (the gate
   flags a row entirely on the better side, `CLAUDE.md`) -- and is not banked as a win.**
4. **Not shown, and what stages 0b to 2a are for:** the engine's own record images and the CRC at first decode;
   Zipfian reads (the memo, 3.7); more than one thread (the decode is CPU, so three threads on four vCPUs scale by the
   CPU and not by the faults); scans (the page walk); the incompressible control; the overlay of unsealed keys; and the
   controller. 7.2b registers what stage 1 measures of it; the engine-level items are 7.3's.

#### 7.2b The margin, the window and the bound axis [P2] (rev 3.1, rev 3.4, rev 2.7, rev 3.2)

M17 passed the thesis at 79% occupancy of the cache with the suite's best-case keys. What no row has measured is how far
from the edge that is, what the window around the cell looks like, and what happens beside the form. Stage 1 measures it
with the probe's existing files, 0 lines of engine code, and these ranges are written before it runs. The z file of M16
is 679.8 MB with a 3.3 MB fence and the probe's cache at ballast 100 MB was about 957 MB (7.2a), so the cache for the z
pages at ballast `b` is `957 - (b - 100)` MB less the resident part and, in the second row, the 80 MB stand-in for the
ordered index's heads.

| experiment (stage) | prediction [P2] | refuted if |
|---|---|---|
| **Knee sweep** (1a): z layout, 10M, 1 GiB, ballast 100-400 MB, no heads; passes 1-3 | faults per read 0.00-0.01 up to ballast 350, 0.03-0.05 at 400; reads/s 255-280k up to 350 | over 0.02 at ballast 250 |
| **Knee with the heads' 80 MB** (1a): the same, with an 80 MB file touched once a read | 0.00-0.01 up to 250, about 0.01 at 300, 0.08 (+-0.03) at 350, 0.16 at 400 | over 0.05 at ballast 200, where the z pages and the heads are 89% of the cache: the engine's scan figure at 50 records a page |
| **Hash residency** (1a): `mincore` over `hash.bin` after a raw-with-hash run | 97-100% of its pages resident, in which case the 0.07 residual of 7.2a point 1 is not eviction of the hash | under 90% resident: holding the hash resident is then a lever (9.5) |
| **Faults by region** (1a, optional, after the residency answer; rev 3.2): the same raw-with-hash run under `perf record -e major-faults -c 1 -d`, the data addresses bucketed by file (the probe maps the hash and the records as two files); for the engine, one untimed `perf record` of the baseline arm's read cell bucketed by the segment mapping's regions (1b) [U: untried on this guest] | hash file 0.00-0.02 faults a read (the two-population model puts the hash at 0.003 and the records with the directory at 0.425, D9), the records the rest, 0.40-0.45 | hash file over 0.04 a read: the hash is being evicted, M17's residual is real, and holding it resident (9.5) is worth up to that |
| **Stray raw reads** (1c): z reads with one read in 200 through the raw layout (hash page and record page), ballast 100 and 250 | faults per read at most the strays' own pages plus 0.01: 0.02 at ballast 100 | over 0.05 at ballast 100: the collapse of `src/ordindex.rs:100-107` is then not an effect of the knee and R4 is understated |
| **Three threads** (1c): z reads, 1 GiB, ballast 100 | 600-750k reads/s with 0.00 faults (the decode is CPU: three threads on four vCPUs, 0.85 scaling of 262-280k) | under 480k |
| **Raw to z switch** (1c): two raw passes with the raw pages cached, then z reads on a z file that is cold, and again on one `WILLNEED`-warmed first | cold: pass 0 0.15-0.20 faults and zero from pass 1; warmed: pass 0 at most 0.03 | cold pass 0 over 0.25 |
| **Bound axis** (1c): z, raw and (by the model) LMDB at 768, 640 and 512 MiB, `B` 668, 534, 400 MB | z faults 0.09 / 0.28 / 0.47 at 126k / 64k / 43k reads/s; raw 0.63 / 0.74 / 0.84 at 37k / 32k / 28k; z over `lmdb-nordahead` 2.7x / 1.6x / 1.3x (D9; one `bench run` row at 512 MiB checks the LMDB model) | z over LMDB under 1.5x at 640 MiB: the window is narrower than D9 says |
| **Hash sizing** (1c, optional): `partition_bytes` set where a partition holds 0.62-0.75 of a hash power of two | hash 11-13 B a key (65-80 MB less), faults per read down 0.04-0.07 | down under 0.02 |
| **Key shapes** (0b): records per page for random decimal, random bytes and UUID text keys | 7.1's grid | 7.1's stops |

The bound-axis row is the answer to "the cell is z's best case on the store/cache axis": the same 10M store under a cache
that is smaller, which is a bigger store under the same cache with the sizes in the same ratio (D9). C4 stays a claim about the
cell and the table above is where its edge is measured. The stray-read row prices the union leak (R4) at a coverage the
suite's cells cannot reach: it is the experiment whose result could say the engine's worry is wrong, and it is registered
that way.

### 7.3 The engine's forced arm against the banked baseline (stages 2a-3), ooc 10M, 1 GiB, `--ops-div 10` as the baseline's rung ran

| cell | `supdb-nocache` [M] | `lmdb-nordahead` [M] | `rocksdb-tuned` [M] | `supdb-nocachezpages` [P] / [P2] |
|---|---|---|---|---|
| point read, faults per read, cells 1t / 2t / 3t | 0.41-0.46 / 0.37-0.42 / 0.36-0.43 | 0.27 / 0.26 / 0.27 | 0 major (it uses `pread`); refaults 0.71 / 0.94 / 1.07 | at most 0.10 (central 0.04, warm on) / 0.03 / 0.03 |
| point read, bytes per read | 1,694-1,895 | 1,122 | 2,889 / 3,848 / 4,398 | at most 120 from the second cell on (0.03 faults of one 4 KiB page each), at most 410 in the first (rev 3.14) |
| refaults per op | equal to faults | equal | the refaults of the line above | equal to faults within 5% |
| point reads/s, 1t / 3t | 53.6-53.8k / 132-147k | 79-82k / 192-195k | 37.3k / 75.4k | 120-230k (central 180k) / 330-620k (central 470k) |
| scan of 100, faults per scan 1t / 3t | 2.9-3.0 / 1.3-1.5 | 1.1 / 1.1 | 0 major; refaults 3.88 / 4.11 | at most 0.3 / 0.3 |
| scan entries/s, 1t / 3t | 0.76-0.80M / 3.56-4.08M | 2.03-2.06M / 4.68-4.82M | 0.85M / 2.03M | 3.5-8M (central 5.5M) / 9-22M (central 14M) |
| mix C, ops/s | 107k | 239k | 88.7k | without the memo 185-230k; with it 260-350k, central 300k [P2: 3.7] |
| mix B / D / E | 174k / 42k / 49k | 193k / 87k / 138k | 110k / 26.6k / 8.3k | 220-380k / 120-210k / 90-180k |
| mix A / F | 224k / 146k | 64k / 67k | 133k / 78.2k | 1.15-1.6x / 1.2-1.6x of `supdb-nocache` [P2], refuted below 0.9x: the banked faults an operation are 0.052 and 0.099, 2.2 of 4.46 and 4.1 of 6.85 us; the registered cells never seal (derivation below) |
| ordered load, ops/s | 1.35-1.51M | 0.92-0.94M | 954k | `Always`: 0.55-0.85x (the builds are in `sync`); `Auto`: 1.00x |
| `bytes_on_disk_per_byte` | 1.304 | 1.147 | 1.0022 | 1.94 (1.88-2.01) |
| `device_bytes_per_byte` | 1.339 | 2.54-2.61 | 1.986 | 1.97 (1.90-2.05) |
| the 3M control (in core), point reads/s | 1.37M | 0.79M | 239-252k | `Always`: 0.15-0.24x of raw, 200-330k; `Auto` (stage 4): raw's, within the arm-against-itself spread |
| `load-shuffled`, ops/s | 318k | 24k | 290k | `Always`: 0.4-0.8x of `supdb-nocache` [P2]: about 50-70 builds [U: 23 partitions rewritten two to three times] at 0.35 s of idle-class CPU each beside seals and merges on four vCPUs, and `sync` waits for the last ones |
| `lag-burst`, updates/s at 1 / 10 / 100% | 1.10M / 0.82M / 0.78M | 16k | [U: not read here] | `Always`: 0.85-1.05x at each [P2]: the builder is idle-class and a burst writes no sidecar bytes; the 100% point's merge cycles compete for the same vCPUs |
| `scan-lag`, entries/s at 0 / 1 / 10 / 100% | 1.0M / 9.9M / 1.5M / 0.05M | 0.85M / 2.6M / 0.83M / 0.80M | 1.646M at 10%, 0.399M at 100% | `Always`: 3.5-8M / 3-6M / 1.5-4M / 0.04-0.06M [P2]. The 1% point is a loss by design: its raw pass takes 0.07 faults a scan (666 faults over the pass), so the raw scan is CPU-light and the z scan, 11 us, is 0.3-0.6x of it -- the cell that says why the controller exists (`Auto` would not enter: 0.07 is under `saving*_scan` of 0.19). At 100% the burst rewrites every partition (about fourteen seals, three merge cycles), the sidecars are superseded, the pass reads raw and `rawserves` is near 1: level, 0.9-1.1x |
| incompressible control (`ab --compress 0.0`, 10M) | 54k reads/s, 0.43 faults (the records are the same size) | -- | -- | built as `STORED` pages, 32 a page: faults 0.24-0.32, 65-90k reads/s (1.2-1.6x of `supdb-nocache`), `bytes_on_disk_per_byte` +1.0-1.2, `declined_gate` 0 [P2: 2.6.1]. The same control at 3M in core: declined (no saving), bytes on disk equal to `Off`'s to three decimals, rates level 0.97-1.03 |

Per-cell derivations: a point read is `t_z + phi * t_f` with `t_z = 4.0 us` (D5) and `t_f = 41.5 us`:
`phi = 0.04` gives 5.7 us, 175k/s; `0.02` gives 4.8 us, 207k/s; three threads at 0.85 scaling on four
vCPUs. A scan of 100 is `0.4 (seek) + 2.74 * 3.0 (decode) + 100 * 0.02 (walk) + 0.7 (engine) = 11.3 us`,
8.8M entries/s resident, [P2] and with the seek by the fence the 80 MB of heads leave the working set (3.4), so the 6M that
the first version priced for the heads' faults is gone [D]. Mix C without the memo is `1/(4.0..4.7 + 0.4..0.7) us` = 185-230k,
with it 260-350k (3.7).

**Mixes A and F** (rev 1.11, rev 3.9; the first version said "level, 0.85-1.15x, no gain claimed" and derived nothing). The
banked rep has `supdb-nocache` at 224k and 146k ops/s, i.e. 4.46 and 6.85 us an operation, with 8,703 and 16,460 major
faults over 166,667 operations (`rung1.log`, line `4465s`), 0.052 and 0.099 an operation, which at 41.5 us is 2.2 us (49%) and
4.1 us (60%) of the operation. The form removes those faults and adds the decode on the reads that reach a partition: A has
0.25-0.5 such reads an operation (half its operations are reads and some are served by the memtable), F about 0.9, each
costing 1.9-2.2 us more than a raw hit with the memo at 0.55: A `4.46 - 2.2 + 0.5..1.1 = 2.8..3.4 us`, 295-360k, 1.3-1.6x; F
`6.85 - 4.1 + 1.5..2.2 = 4.3..5.0 us`, 200-235k, 1.4-1.6x. The registered ranges are wider and lower, for the reads of keys
the mix itself just wrote, which are memtable hits either way, and the refutation line (0.9x) is the verdict's reason to
keep these mixes: a loss, not a gain. **The cells never seal.** `ycsb_ops` is a sixth of the keys, divided by 10 here:
166,667 operations (`bench/src/run.rs:315`, `Plan::divided` `:277`), so mix A writes 83k updates, about 12 MB, and a seal at
10M keys fires at `max(32 MiB, store/16)`, about 101 MB (`threshold_for`, `src/db.rs:13086-13096`). No seal, merge or rebuild
lands inside them, so they price the read-through-memtable path only; the first version said the verdict was there to catch
the exposure window and the rebuild cost in them, and they cannot. A run at `--ops-div 1` makes about one seal and no merge
(four pieces are needed), so the exposure window is priced by the lag sweep's bursts, which seal about fourteen times at 100%
(the rows above), and the mix soak that would price it directly needs a runner knob that does not exist [U]. **Mix E**
(50-key scans, `YCSB_SCAN`, `bench/src/run.rs:310`): a scan touches 1.86 pages, `0.4 + 1.86 * 3.0 + 50 * 0.02 + 0.7 =
7.7 us` plus the mix's insert share, 90-180k against LMDB's 138k: a straddle, decided by the decode.

**What the banked rows already say against `rocksdb-tuned`, and what this move does not claim.** `supdb-nocache` wins
every rate and byte cell of the table above but two -- the one-thread scan (0.763M against 0.849M entries/s, 0.90x) and
bytes on disk (1.304 against 1.0022) -- and, outside the table, the shuffled load (318k against 290k ops/s) [M:
`rung1-10m` and `rung2-10m`, medians of the banked reps]. It loses the lag sweep's 100% point to both comparators and its
10% point to RocksDB: `scan-lag` entries/s with 10% and 100% of the keys rewritten read 1.454M and 52.5k against
RocksDB's 1.646M and 399.4k (0.88x and 0.13x) and `lmdb-nordahead`'s 0.826M and 0.797M (1.76x, then 0.066x). This move is
not claimed to touch those two points: a burst's merges rewrite the partitions and a rewritten partition is raw until its
sidecar installs (the exposure window, 5.6), so the pass reads the form it reads today, and what costs the 100% point is not
a footprint on its face [U: not examined here; `forms-lead.md` is the nearest lead]. So the goal "every row but disk" is not
reached by this move (rev 3.9: the first version said it closed "the point-read, scan and mix cells against LMDB", which its
own ranges do not support): the rows it closes are the point-read and scan cells against LMDB, mixes A, B, D and F, and the
one-thread scan against RocksDB; mix C is decided by the memo (260-350k against 239k, a straddle at its low end) and mix E
by the decode (90-180k against 138k, a straddle); the lag points at 10M under the bound stay open for a follow-up; and past
D9's window no single-fault form reaches LMDB's reads (open question 1).

### 7.4 The controller (stage 4)

At 3M under the bound: the rung stays FITS, zero samples, zero builds, zero files, `zgate` 0 for the pass.
At 10M: SUSPECT within two S1 ticks of the first read (4.5: two consecutive 1 s ticks; the first version said "within one tick",
which 4.5 contradicted: rev 2.15), the ski-rental price paid in about 9 s on one thread and 3 s on three, the build and the warm
(15-45 s) at raw rates, then COMPACT; a restart with valid sidecars reaches COMPACT in about a second. A limit lifted to 4
GiB: FITS within 5 S1 ticks, and after `retain` the directory equals a store that never built. A square wave of uniform and a
1% hot set at 10, 60 and 300 s: at most one switch per `dwell`, and a build per partition per entry, never more. A skewed
workload that fits: no build, ever. The default policy (forms on) under the bound is expected to be killed at 3M by its
forms (R17) or to stay out of COMPACT because `anon` leaves `B` too little for `W_z` to fit and the gate declines; its absence
from the rows is not a pass (7.8, (b)).

The replay of stage 1d adds the traces the first version never had (rev 2.1-2.4, rev 2.8, rev 2.16), each asserting 4.5's
table and the one-switch-per-dwell rule: a cold start of a store that fits (no build: refaults are zero); a 0.3 s uniform burst
on a skewed store (no build: `excess` stays under `C_build`); a 100 MB hot set after COMPACT (the trial returns to raw) and a
600 MB one (it stays); a scan-only trace; an A/F-shaped trace with a commit in every window; a B-shaped one; windows recorded
from `rung1.log`; a store fifty times the cache (it builds: D9, the raw hash misses too); an `anon` swing of 400 MB with `B` near
`R` (no flap); a neighbour's page cache (the pressure term opens the build clause); and builds superseded four times in eight
(building suspends).

### 7.5 In memory (stage 4 onward; the quick row)

* The deterministic quantities identical to three decimals for every supdb arm: `bytes_on_disk_per_byte`
  1.314 / 1.341 / 1.367 / 1.304 and `device_bytes_per_byte` 1.349 / 1.376 / 1.402 / 1.339 at 10k / 30k / 100k
  / 300k for `supdb`, `supdb-ingest` 1.304 / 1.304 at 300k (identical in the nine most recent banked quick rows
  [M: `bench/runs/quick`]; `supdb-ingest`'s device bytes at 300k read 1.304 in the one I checked). A difference is looked for in an unrelated engine change by building the commit
  before, before the form is blamed.
* `bench ab --arms supdb,supdb-autozpages` at 100k and 300k, all passes, twelve pairs a sitting, two sittings
  with the order swapped, and the arm against itself beside: nothing marked under Holm, no single mark
  keeping its sign in the second sitting, in particular `read`, `read_2t`, `read_3t`, `ycsb-B`, `ycsb-C`,
  `ycsb-E`, `scan`; every `zpage_*` counter zero; no extra thread in `SUPDB_THREAD_SPANS`; the pass line's
  `rss` unchanged.
* The per-slot counters, priced by taking the alternative away (rev 1.4): `bench ab --arms supdb-nocachezpages,
  supdb-nocachezpagessharedcounts --passes read-threaded` at three threads, twelve pairs a sitting, two sittings with the order
  swapped, the arms checked to differ in `zpages_shared_counters` alone. [P2] The per-slot arm reads level or better at one
  thread and 1.0-1.3x at three; the shared-counters arm is expected to lose at three. If the two read level, the per-slot line
  bought nothing and the simpler shape stands.
* Off Linux `Auto` is inert (2.9): the macOS and ARM legs of CI run `tests/zpages.rs` minus its residency checks and assert
  `auto_inert`.
* Callgrind: `Reader::read_all` +3 instructions (at most +6).
* Two binaries (the commit before and after) alternated in one sitting, several rounds, and again in a
  second: comparators' ratio between the binaries is the machine, the `supdb` arms' is the change
  (`bench/CLAUDE.md`, "An A/B of two engine revisions is two runs").

### 7.6 The verdict: quantities, order, rule

Counts first, rates second, the sum of the phases last (`CLAUDE.md` "A read measured fast because the write
paid for it"). On `bench ab` pairs in two sittings with the order swapped, and one `bench run --scale ooc`
row for the series. At least six pairs are needed for p under 0.05 (all one way is 0.031), twelve reach
0.0005; a single mark is a place to look (`bench/DESIGN.md:326-338`).

1. **Did the reads leave the raw file:** forced `read_3t majfaults_per_op <= 0.05` (the refutation line; the
   prediction is 0.03, C2: rev 3.14) and `refaults_per_op` within 5% of it; `read_bytes_per_op / majfaults_per_op` one
   page; the residency probe's raw segment pages flat; `raw_touches == 0` (for points and counts at stage 2a, for every scan
   shape of 3.9 at stage 3); `rawserves` under 1% after the settle; and the *staircase* read, not one cell: the three read
   cells run in order on one store, so a cold file falls across them while a capacity-bound raw arm stays flat (0.413,
   0.370, 0.361 [M]).
2. **Did it win where the comparator wins:** `reads_per_s_3t >= 1.5x` and `scan entries_per_s_3t >= 1.5x`
   `lmdb-nordahead`'s in the same row, and the one-thread cells reported with their staircase; mixes B, C, D,
   E reported per cell with the memo on and off.
3. **Did it cost what it was predicted to, and no more:** the load's `ops_per_s` by the sum of phases
   (`SUPDB_LOAD_PHASES=1`, commits against the closing sync), `bytes_on_disk_per_byte`, `device_bytes_per_byte`,
   `load-shuffled`, `lag-burst`, `scan-lag` and the incompressible control at the ranges of 7.3, mixes A and F; for
   every cell that writes beside a merge, `rawserves` as a share of the reads, the merge cycles that landed and the builds
   superseded, with the forced arm's coverage reported as a covariate of the cell (rev 1.11).
4. **Did it not wreck what it should not touch:** the 3M control, the quick row (7.5), the `check.sh` groups,
   `web/build.sh` bytes, `tests/blob.rs` and the others unedited and green.
5. **Was it reversible:** the take-back test and the row's disk cell after a take-back.
6. **What the build cost the readers meanwhile** (rev 2.6): the readers' faults per read and reads/s through a build and its
   warm, from the `zresident` samples and the pass's window columns, against the same arm without a build; charged to the
   ski-rental price (`displacement_est`, 4.4).
7. **Where the window's edge is** (rev 3.4): the rows of 7.2b, the bound axis in particular, are read with the verdict. C4 is
   a claim about the cell, and the table says how far from it the claim holds.

### 7.7 Stop rule

Stage gates (8 has the stages): 0b stops under 36 records a page on the suite's shape (7.1); 1a stops if z faults exceed 0.05
at ballast 200 with the heads' 80 MB or 0.02 at 250 without (7.2b); 1b stops if its pairs read level on faults in both
sittings; 1c stops on any 7.2b refutation; 1d on a trace that violates 4.5's table or the one-switch-per-dwell rule; 2a on a
refutation of 7.3's counts in two sittings, or on any `raw_touches` for a point or a count; 3 on a `raw_touches` for any scan
shape of 3.9 or a scan cell outside 7.3; 4a-4c on 7.4 and 7.5. **Operational stops:** a rung is
run behind a banked 3M (`scratchpad/ooc/rung1.sh:1-4`): a 10M rung killed under the bound says the bound,
not the engine, and exit 137 means raise the bound (`bench/scripts/ooc.sh`); a forced arm killed, or a
counts line `odd` by more than a few percent (the banked `read_3t` is odd by two refaults of 1.08M, so
the raw triple is read before the word), stops the pairing for a decomposition, not a tuning; a move of a
quick cell outside its band in both sittings stops the stage; **no second timing run while one is in
flight**, and none started in a `TMPDIR` shared with another run, because `bench run` deletes every
`supdb-bench-*` in it at start (`bench/src/run.rs:624-640`, and the engine tests share the prefix).

### 7.8 Done when

Every stage has its own DONE WHEN (8) and leaves a mergeable state with `ZPages::Off` the default. The program's is
two-tiered (rev 3.6): the first version required the flip of the default as part of the program's done while 5.8 and R17 put
the default policy out of scope, which would have validated the flip on a configuration, `nocache`, that the default is not.

**(a) The form and the selector are in, behind the option.** The verdict items 1-7 of 7.6 hold in two sittings with the order
swapped, on the nocache policy; the selector's behaviours of 7.4 hold under their tests; `sh scripts/check.sh` is green in every
group on all three CI legs (`deps.sh` first on a fresh box) with `Off` the default; the 3M control and the quick row are clean;
`docs/engine.md` carries the section with the checked numbers; `CLAUDE.md` carries what the form taught -- the invariant that
a covered partition is read from one form or from a listed, counted exception, the identity rule (a derived file binds to the
identity of what it derives from, the segment's key-section row and name fields, and a derivation that stores a function of
engine code, the filter, binds to a signature of that code: 2.5; the first version's "tombstone-free identity rule" was
defined nowhere and is dropped, rev 3.14) and the failure shapes found on the way; the `README.md` says, factually and without
a standing figure, that a store out of memory may carry a derived sidecar and what removing it does.

**(b) The default moves to `Auto`** only when the forms-memory lead (`scratchpad/ooc/forms-lead.md`; `docs/engine.md:5895-5930`:
the default policy was killed in its first rep at 3M under the bound, `anon-rss` at the limit; commit `5dbfa23`: its anonymous
memory reaches 760-820 MB at the full-rewrite lag point against 225-255 for no cache) is closed, **or** `Auto` is restricted to
policies that build no forms and its option doc says so; and a bounded run of the default policy at a rung where it survives
has been read with `Auto` on, and shows either that it never leaves FITS or a stated, expected kill. Until then the absence of
a default-policy row under the bound is not a pass: 7.4 lists it as an expected "never leaves FITS, or is killed".

## 8. STAGED PLAN

Each stage is the cheapest thing that can refute the one before it, runs only when the machine is free of any other timing
run (`bench/CLAUDE.md`: "Never run two timing benchmarks at once. This includes your own agents"), and ends in a banked
artifact or a stop. Lines are new or changed, tests included, [D] estimates from the sizes of the nearest existing code
(`src/ordindex.rs`, a comparable derived companion, is 1,494 lines with its tests). Machine times are [D] from the measured
pass times (`rung1.log`: a `supdb-nocache` 10M pass 441-497 s at `--ops-div 10`, `lmdb-nordahead` 1,288 s, `lmdb` 2,172-2,238 s)
and are estimates because none of these stages has been run. The first version started with physics the standalone probe had
already passed (M17) and spent 2,400 lines before anything tested the margin or the controller; this order starts with the
experiments that can still fail (rev 2.8, rev 3.1, rev 3.5, rev 3.11). Every stage has a DONE WHEN, and the `Off`-default state
it leaves is mergeable.

| stage | what it is | lines | machine time | decides | stops on | DONE WHEN |
|---|---|---|---|---|---|---|
| **0a** | bank the evidence and fix the runner (rev 3.8, rev 3.3): copy the probe's `src/main.rs`, `Cargo.toml`, `prereg.md` with its RESULT block, `bound.sh` and `drop.py`, the rung logs and the 10M rows into the repository (`bench/examples/zprobe/` and a short note; the rows under `bench/runs/ooc/b<MiB>m/` only if the person decides to bank them: committing a row is a person's decision, `.github/workflows/ci.yml:63-68`) and re-run v1 once with its output captured to a file; `bench ab --ops-div`; `load_bpb` and `disk_bpb` finite without the load pass; the `--compress` flag on `ab` | 60 | none for the copy; 10 min for the captured v1 re-run | nothing: it makes the evidence re-readable and unblocks 1b | -- | the files are in the repository and the note names the commit; `ab --ops-div 10` runs a 10M pairing in about 15 minutes; `sh scripts/check.sh bench` green |
| **0b** | the layout half of `src/db/zpages.rs`: `fill_group`, page and trailer encode and decode, `record_key_next`, `BLOOM_SIG` and its pin, unit tests, wired to no read path (so no row can move); and `bench/examples/zsize.rs`: the real `fill_group` over the suite's record images (built with `flatindex::stream_record` from a loaded partition's `exts_at_full`, byte-equal to the stored compact records), at payload compressibility 0.5, 0.0 and 1.0 and for the four key shapes of 7.1: records and bytes a page, fill, compress calls a page, decode, walk and CRC us a page, occupancy at 10M; `lz4_flex` features pinned, the codec printed | 570 (layout 400; example 170) | 5 min CPU | records per page and the decode and walk cost on the engine's own bytes, per key shape; the one layout every later stage shares (the probe's v2 as a separate program is retired: its page header, CRC and walk would have been a second definition of the layout, rev 3.1) | records per page under 36 on the suite's shape | layout merged `Off`, wired to nothing, `check.sh` green on the three CI legs, every row unchanged; the grid banked |
| **1a** | the margin sweep on the probe's existing files, no code (7.2b): the knee sweep with and without the 80 MB heads stand-in, the hash file's residency by `mincore`, the faults by file under `perf record` (optional), one cold start. **The first experiment**; its command lines follow the table | 0 | 30 min | the thesis margin (79% occupancy to 100%) and whether the hash is evicted | faults over 0.05 at ballast 200 with the heads, over 0.02 at 250 without | the results, the command lines and the files' hashes banked |
| **1b** | `Options::point_via_ord` and the arm `supdb-nocacheordread`: a point read through `OrdIndex::seek_exact` then `Blob::values_at` (L1, 9.2), off by default; and one untimed `perf record -e major-faults -c 1 -d` of the baseline arm's read cell, bucketed by the segment's regions (7.2b; rev 3.2). It needs 0a's `ab --ops-div` | 70 | 40 min a sitting, two sittings, 12 pairs; 15 min for the perf run | the model's coefficient for page-cache auxiliary structures (which also re-prices z's scan working set, R5), and whether L1 is the incompressible fallback | the pair reads level on faults (under 0.02 apart) in both sittings | the arm in place, off by default, with 9.2's ranges met or refuted |
| **1c** | the probe grown by flags (v1.1, about 150 lines): three threads, the 1-in-200 stray raw reads, the raw-to-z switch with and without a warm, the bound axis at 768, 640 and 512 MiB, and optionally the hash-sizing arm | 150 | 45 min | R4's stray-read collapse, three-thread scaling, the window's edge (7.2b) | any 7.2b refutation | the rows of 7.2b filled in |
| **1d** | the selector core `ZSelect::step` and its replay tests (7.4): the traces, no machine | 500 | none | the transitions (4.5) before any census or I/O exists | a trace that violates 4.5's table or the one-switch-per-dwell rule | tests green under `checked`; the constants are the replay's, read off the traces |
| **2a** | the proof slice: the forced arm for point reads and counts only. The builder (no `NOPAGE`, no `STORED`: a partition with a block-backed, oversize or incompressible record is declined), attach with the trailer CRC, the tiling check and sampled pages, the cell, `read_all` and `count` behind one gate load, the counters of reads, rawserves, builds and declines, the build called from `Supdb::sync` under `Always`; the oracle test and one scan-model twin; no `SegBlob`, filter, memo, warm or controller | 900 | tests 15 min a run under `checked`, about four runs; `ab` at 3M 15 min; at 10M with `--ops-div 10`, 12 pairs a sitting 40 min, two sittings | C2 and C4 for point reads on the engine's own path, the in-situ `t_z` and the three-thread number; R3 and R4 for points | a refutation of 7.3's counts in two sittings; any `raw_touches` on a point read or count | the forced arm measured in two sittings, `check.sh` green on all three CI legs, `Off` the default, rows unchanged |
| **2b** | lifecycle: retire through the epoch with the sweeps, promotion attach, sweep at open and the dead names, the orphan collect, the crash windows, the 20-field identity trailer, damage injection and the bounded rebuild, the Bloom filter and `bloom_sig`, `ZNOPAGE` and `STORED`, the `ZStat` counters, the warm job; arms s1-s15 as the stage needs them; the tests of 6.3 for lifecycle, damage and identity (about thirty, and twins of eight existing tests) | 1,500 | tests 15 min a run, about six runs | R6, R7, R15 | any lifecycle test red under load (run beside a build, four at a time) | the store survives every crash window of 5.7; `a_zpages_life` leaks nothing |
| **3** | scans: `SegBlob` without forwarders, `ZCursor`, the seek by the fence, `cut_in`, `ZSide::scan_at`, `values_at` and `with_exts_at` at the sites of 3.9, the prefetch gate, the memo and its arm; the scan-model tests; the Zipfian mixes. (The seek by the fence is no longer a conditional 3b: rev 2.5) | 1,400 (engine 900; tests 500) | `ab` with the scan passes 90 min; mixes 60 min; the three-arm row (`supdb-nocache`, `supdb-nocachezpages`, `lmdb-nordahead`) at `--reps 3 --ops-div 10`, 3-3.5 h, twice | C2 for scans, C4 for scans and mixes B, C, D, E, the memo's worth, R4 and R5 | 7.3's scan cells; a mix C the memo does not move; any `raw_touches` on a shape of 3.9 | verdict items 1, 2 and 4 in two sittings for the scan cells and the mixes |
| **4a** | `Auto`, the thin part: `zselect.rs` wired to the engine (cgroup v1 and v2 readers, the model, the tick at 1 Hz, build-when-wanted), `ZInject`; no hot-path code | 500 | injected tests 20 min; the 10M `Auto` pass 60 min | S1 and the build decision, on the real thing | an in-memory mark that keeps its sign; a build in memory | in memory: no thread, no file, no sample, callgrind +3 instructions; `Auto` enters under `ZInject` |
| **4b** | the census: the `Slot` fields and `ZStat`, the windows and their validity (commits, scans), the refault evidence, the scan census; the probe takes each workload (scan-only, B-shaped, A-shaped) to a fresh store, so that no earlier pass has taught the rung (rev 2.4) | 450 | 15 min of tests, a 30 min probe | the census's `t_f`, `t_hit` and `saving*` on the real thing | a window class that never produces evidence in a mix | the replay's traces pass with the engine's census |
| **4c** | E2 (the bitmaps), E3, the trial on the refilled state, the back-off, the ski-rental price, the wasted-build budget; the constants measured against the square wave and a sweep | 400 | tests 20 min; sweeps 60 min; the in-memory pairs `supdb` against `supdb-autozpages` at 100k and 300k, all passes, 12 pairs, two sittings, 2 h; callgrind 5 min | C6, 7.4, 7.5 | an in-memory mark that keeps its sign; a skewed workload that builds; more than one switch per `dwell` | 7.4's behaviours and 7.5 hold |
| **5** | polish: build inside the merge from the records it is writing (the `Emitter`, `src/db.rs:8701-8715`), the promotion attach, the exposure counter's target | 500 | mixes A and F pairs 40 min, two sittings | the exposure window's price | `rawserves` above 5% in a mix | the lag cells and `rawserves` at 7.3's ranges |
| **6** | the flip, only under 7.8(b): `ZPages::Auto` the default, the `*nozpages` arms, the rosters, the quick row, docs (`docs/engine.md`, `CLAUDE.md`, `README.md`) | 150 plus docs | the quick row twice, 20 min each; the ooc row 3 h, two sittings | 7.5 across the whole suite | any gate group red | 7.8 (b) |

Total: about 7,150 lines, a bit under half of them tests, and 30 hours of machine time with the second sittings 7.8
requires (23 with one sitting of the three-arm and ooc rows), 3 hours of it before the sidecar's read path exists (0a to 1c); none of it
while another timing run holds the machine. The first version said 14-16 hours; its own cells summed to 17.1 and, with the
conditional stage 3b, 22.9, before the second sittings (rev 3.13).

**Why this order** (rev 2.8, rev 3.1, rev 3.5). The thesis-level gate was passed before the engine's first line: M17 ran
exactly the three layouts and the 10M cell that 7.2 registered, and 7.2a records that none of the refutation lines fired, so
the first version's stage 1 (a v2 of the probe with threads, Zipfian reads, engine-framed images and a variable-key fence)
re-measured a passed gate and had no registered range for what it added. The cheapest experiments that can still fail are the
margin (how far from the cache's edge 79% occupancy is, and whether the hash is evicted: 1a, no code), the selector's transitions against traces nobody has run (1d, no machine), and the first
in-situ numbers (2a: 900 lines and not 2,400, because the proof slice leaves out what the cell does not contain -- `ZNOPAGE`,
`STORED`, the filter, the lifecycle, `SegBlob`, `warm` -- all of which the first version bundled with the proof, so that a
lifecycle bug could not have been told from a failed thesis). L1 (1b) comes before the sidecar because it needs no file, no
format change, no identity check and, as an off-by-default flag, no controller. Stage 0b is not on that list as a gate: its stop
on the suite's shape (36 records a page, against a prediction of 57 and the probe's 60.0) is not expected to fire and is kept because
it is free. What 0b can still refute are the ranges of 7.1 (fill, compress calls a page, decode, records a page for each key shape,
where UUID text at 40-46 straddles C4's 40.1 and the scan claim's 43.1), and its main product is the one layout every later stage
shares (rev 3.1).

**The first experiment (stage 1a)** needs a free machine, 80 MB of new disk (the probe's 2.05 GB of files are on disk already, M16) and 30 minutes. `$Z` is the scratchpad
directory (or wherever 0a put the probe); `$D` holds the probe's 10M files (`raw.bin` 1,170,288,640 B, `z.bin` 679,784,448 B,
`hash.bin` 199,229,440 B, `raw.fence`, `z.fence`: M16) and the built binary is `$Z/zprobe/target/release/zprobe`. The command shape
is the probe's own, read from its source and scripts and not run here: `zprobe read DIR N LAYOUT HASH_MB READS PASSES` with
`BALLAST_MB` in the environment (`HASH_MB` above zero maps `hash.bin`, zero omits it), inside `bound.sh LIMIT_BYTES CMD...`
(which hard-codes in `P=` the cgroup parent of the host it was written on; another host changes that one line), after `drop.py` has dropped the
files' pages. Each run prints `pass i: R reads/s  U us/read  F major faults/read`.

```sh
Z=/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad     # holds zprobe/ and tmp-price/
D=$Z/tmp-price/zp10; H=$Z/tmp-price/zp10h80
# bound.sh names the cgroup it was written under in P=; point it at the one this shell is in (a no-op in the shell it was written in)
sed -i "s|^P=.*|P=/sys/fs/cgroup/memory$(sed -n 's/^[0-9]*:memory:\(.*\)$/\1/p' /proc/self/cgroup)|" $Z/zprobe/bound.sh
# the 80 MB stand-in for the ordered index's heads: the same z and raw files, a smaller file touched once a read
mkdir -p $H && for f in raw.bin z.bin raw.fence z.fence; do ln -sf $D/$f $H/$f; done
head -c 83886080 $D/hash.bin > $H/hash.bin && sync      # written back before drop.py runs: fadvise(DONTNEED) cannot drop a dirty page
# the knee sweep: z layout, 10M keys, a 1 GiB cgroup, 1M uniform reads a pass, four passes, the files dropped first
for dir in $D $H; do
  hm=0; [ "$dir" = "$H" ] && hm=1
  for b in 100 150 200 250 300 350 400; do
    python3 -I $Z/zprobe/drop.py $dir/raw.bin $dir/z.bin $dir/hash.bin
    BALLAST_MB=$b sh $Z/zprobe/bound.sh 1073741824 $Z/zprobe/target/release/zprobe read $dir 10000000 z $hm 1000000 4
  done
done 2>&1 | tee $Z/1a-knee.log
# hash residency: the engine's shape (raw pages and the 199 MB file), then mincore over the hash file once the run is over
python3 -I $Z/zprobe/drop.py $D/raw.bin $D/z.bin $D/hash.bin
BALLAST_MB=100 sh $Z/zprobe/bound.sh 1073741824 $Z/zprobe/target/release/zprobe read $D 10000000 raw 1 1000000 4 2>&1 | tee $Z/1a-hash.log
python3 -I - $D/hash.bin <<'EOF' 2>&1 | tee -a $Z/1a-hash.log
import ctypes, os, sys
libc = ctypes.CDLL(None, use_errno=True); libc.mmap.restype = ctypes.c_void_p
fd = os.open(sys.argv[1], os.O_RDONLY); n = os.fstat(fd).st_size
addr = libc.mmap(None, n, 1, 1, fd, 0)                       # PROT_READ, MAP_SHARED: mincore reads residency, not the pages
pages = (n + 4095) // 4096; vec = (ctypes.c_ubyte * pages)()
assert libc.mincore(ctypes.c_void_p(addr), ctypes.c_size_t(n), vec) == 0
print(sys.argv[1], sum(v & 1 for v in vec), "of", pages, "pages resident")
EOF
# faults by file (optional, after the two runs above; perf is outside the cgroup, the probe inside) [U: untried on this guest]
python3 -I $Z/zprobe/drop.py $D/raw.bin $D/z.bin $D/hash.bin
BALLAST_MB=100 perf record -e major-faults -c 1 -d -o $Z/1a-faults.data -- sh $Z/zprobe/bound.sh 1073741824 $Z/zprobe/target/release/zprobe read $D 10000000 raw 1 1000000 4 2>&1 | tee $Z/1a-faults.log
perf report -i $Z/1a-faults.data --stdio --sort dso_daddr 2>&1 | head -30 | tee -a $Z/1a-faults.log
```

Read against 7.2b's first four rows. Run the loop twice, the second time with the ballast values in reverse, so a position
effect shows as a difference between the sittings and not as a slope (`bench/CLAUDE.md`, "Interleave the arms"). `mincore`
after the cgroup is gone reports the page cache's residency at the end of the run, which is the state to read; the helper
is untried on this guest [U].


## 9. ALTERNATIVES CONSIDERED, AND WHAT IS CUT

### 9.1 The ladder of forms, at 10M keys under 1 GiB

| form | new on disk | resident, always | working set `W` | `phi_hat` | CPU a read | beats `lmdb-nordahead` (0.274)? |
|---|---|---|---|---|---|---|
| raw (today) | 0 | hash 190 MB | 1.49 GB | 0.425 [D2]; 0.41-0.46 [M] | 0.73 us | no |
| **L0'** the hash at 0.62-0.75 load (9.5) | 0 | hash 110-125 MB | 1.41 GB | 0.36-0.37 [D, U] | 0.73 us | no |
| **L1** point read through the ordered index | 0 | heads 80 MB, directory 40 MB | 1.42 GB | 0.372 | +0.39 us seek | no |
| **L2** page fence over the raw records, no hash | a 6 MB fence, a 12.5 MB filter | 19 MB | 1.31 GB | 0.284-0.294 | +0.4 us walk | ties |
| **L3** z-pages (this move) | 0.72 GB | 17 MB | 0.74 GB | 0, steady | 4.0-4.7 us | yes, if it fits, and inside D9's window |
| L4 a verbatim copy in fence-addressed 4 KiB pages | 1.28 GB | 19 MB | 1.3 GB | 0.28 | +0.4 us | no, and twice the disk |

`phi_hat = 1 - (B - resident)/W_records` with `B = 937 MB`, from D2 (the one-population form, adequate where the hash
fits, as it does in this table). The column to read first is the last: the raw form's loss is a *footprint* loss (D3), the
hash is a seventh of the footprint (D4), and only a form that shrinks the *records* can go below LMDB's 0.274.

**After the result (after M17; the table is as registered).** The probe measured three rungs at 10M under the bound (M17):
the engine's shape, raw pages and a 199 MB hash file, 0.414-0.423; the L2 footprint, raw pages and a page fence, 0.182 at the
probe's 1.17 GB (about 0.26 at the engine's 1.3 GB, by the formula that number calibrates [D]); and L3, z pages and a
fence, 0.166 in the first pass and 0.000 from the second. L1 was not measured, and 7.2a says why its registered 0.372 may
be pessimistic. The table's order holds, and L2 is a tie with LMDB and not a clear win.

**After the reviews [P2].** The ladder is a ladder at one point of D9: the cell, where the records are 1.4 times the cache.
Away from it the order of L3 and LMDB changes (L3 is 1.5x LMDB to 2.6 times the cache, ties it near 7 and trails it by a tenth
beyond), while L3 stays 1.75x ahead of the raw form at any size because the raw form's hash stops fitting too (D9, with the
two-population model of 4.2). Two levers the first version missed sit beside the ladder and are priced by stage 1 before
anything is built: the hash's power-of-two step and holding the hash resident (9.5).

### 9.2 The ordered-index point read (L1), the task's cheaper alternative

A hook in `Seg::read_all`: `seg.ord.seek_exact(key, |r| blob.key_at(r))` returns `(rank, Some(exact))`; for
keys of one length up to the prefix plus eight bytes -- the suite's -- the heads *are* the keys and the
answer needs no record read (`src/ordindex.rs:710-716`, `864-887`); then `Blob::values_at(rank)` reads one
directory word and one record (`src/blob.rs:973-989`, `src/flatindex.rs:1859-1876`). It touches the heads
(8 B/key, one 512-byte window of 64 heads, eight lines of one page), the directory (4 B/key) and the record
page, and none of the hash. The working set is the heads and the directory, 120 MB, in place of the hash's
190: 70 MB, `70/1300 = 0.054` faults a read, 0.425 to 0.372 [D], which in time is 18.6 us to 16.4, 54k
reads/s to about 61k [D] -- a third of the gap to `lmdb-nordahead`. It needs no file, no build and no identity
(the ordered index exists and is checked at open), is reversed by the gate, and is the cheapest *engine* change that moves
the cell (about 70 lines). It cannot be the move: 0.33-0.38 against LMDB's 0.274 leaves the cell a loss. Nor does it wait
behind the sidecar (rev 3.3): it is stage 1b, before the first sidecar line of the engine, because it is the one lever that
needs no file, no format change, no identity check and, as an off-by-default `Options` flag chosen by hand, no controller.
"Needs the same controller" holds only for a default-on L1: in memory a lookup of 94 ns (`src/db.rs:18194-18195`) becomes a
seek of 390 ns, 170 of them the tie search at 300k keys (`src/ordindex.rs:34-36`), which is why it is never the default.
Registered [P2]: faults 0.33-0.38 at 10M and 1 GiB (the model's 0.372 and, if the heads lose pages as the hash may, M17's
residual, 0.33), 58-66k reads/s on one thread against 54k; a pair level with `supdb-nocache` on faults in both sittings
means the model's coefficient for page-cache auxiliary structures is wrong, which also re-prices z's scan working set (R5).
**As the fallback for data that does not compress**: under the benefit gate of 2.6.1 the incompressible control is built as
`STORED` pages and still saves the hash's footprint (7.3), so what the gate declines is the case with no saving; L1's place
there is a manual flag now, and it joins the selector as a form the gate word can store (4.1's reserved value) if its pairs resolve a gain.
The first version wrote "dominated by L2", an L2 it does not build.

### 9.3 L2: the raw records read through a page fence (cut from this move, reserved in the format)

For every 4 KiB page of the record region, the first record that starts in it and its key: derived from the
directory and the ordered index's heads without reading a record (for the suite's keys the heads are the
keys), 317k entries and 6 MB at 10M. A point read: fence search in heap, one raw record page, a walk of the
records that start in it (the record that straddles the page's end is the last of its window and costs the
next page 3% of the time, as the hash route's record straddles 2.9% [D: 30 of 1,024 four-byte phases]).
The footprint is the records alone, 1.31 GB, so `phi_hat` is 0.284 and the read rate about 76k/s [D]: 0.93x
of `lmdb-nordahead` -- a tie that cannot become a win. It also needs the negative filter, which needs the
keys' hashes, which needs a pass over the record region at build (the same pass as z's), or the hash resident
for negatives (190 MB, which gives the gain back). So it has most of z's lifecycle -- a derived file,
identity, a controller -- and none of z's win. Its role in this design is one, and smaller than the first
version gave it: it is **what `ZMAX_SHRINK_L2` stands for** on the day L2 is built (2.6.1: do not copy what does not shrink by
15%). The first version also named it "the answer to what to do when data does not compress"; L2 is not built, and until it
is the answer is the gate of 2.6.1 and, by hand, L1. The trailer's `flags` bit 1 is reserved for it so
adding the kind later is not a format change.

### 9.4 Cut, with the reason

| alternative | why it is cut |
|---|---|
| per-value compression | the records compress to about 1.0 one value at a time (M13): there is no redundancy inside a value |
| zlib or another codec for the groups | zlib level 1 gives 0.53-0.55 against LZ4's 0.57 (M13): 2.4 more records a page, for a decoder 5-8x slower on a read that is already CPU-bound at 4 us; revisit only if stage 0b reads fewer than 50 records a page and the decode has room |
| 8 KiB groups | two pages, two faults: the point of the form is one fault a read |
| compress the segment itself (chunked blocks, `src/block.rs:330-361`) or re-lay-out the raw record (drop the eight framing bytes, delta the keys, 4-byte hash slots) | a format change and a new magic (`CLAUDE.md` "The magic moves when a reader from before the change would misread"), a loss in memory (every read decodes), and no second form: the raw form could no longer be the in-memory form |
| route each key to the form where it is cheaper (hot keys raw, cold keys z) | exactly what `src/ordindex.rs:91-107` measured to collapse; the memo (3.7) is the hot tier's CPU saving without a second footprint |
| `pread` or `O_DIRECT` with an engine-owned buffer pool in place of a mapping | the faults stop being faults, so the suite's refault leg fails by construction (`bench/DESIGN.md:100-103`), lending is lost, and every hit pays a syscall |
| build at the landing, as the ordered index is | a z build is 0.25-0.55 us a record on every seal and merge of every store including the in-memory ones: it breaks "the in-memory rows must not move". Allowed in stage 5 for merges, when the form is already wanted |
| build lazily on the first reader | a build of seconds on a reader's thread holds every replaced state (`RetireList::sweep`, `src/db.rs:10587-10628`) and runs un-niced; "the first reader to find the cache empty pays" is a named defect (`CLAUDE.md` "A publish that left its derived work to whoever looked first") |
| a `OnceLock` cell (the in-memory cells' precedent, `IdCache`, `src/db.rs:22093-22165`, is append-only too) | it cannot be cleared, so a take-back could not drop the mapping, and an unlinked file stays allocated while mapped: "disk is returned" would be false until the segment died |
| identity by name and key count, as the ordered index | it holds heads, not values; a stale sidecar would return another segment's values silently |
| one sidecar for the store | merges rewrite partitions; the grain of identity, retirement and promotion is the partition |
| a time-based signal (`Instant`, `rdtsc`) or `mincore` on the read path | 50 ns a pair against a 212 ns read, a latency threshold to type, and a residency answer that is not a blocking answer (4.3); `mincore` is the tests' instrument |
| the PSI file `/proc/pressure/memory` as the trigger | present on this host; a group-wide figure with no attribution to the store's reads. Used by nothing; the refault rate in `memory.stat` is the pressure term |
| a per-handle form | the build is store-level work and the footprint is shared; two handles in different forms is the sum-of-footprints failure |
| a decline test against L2's footprint (`ZMAX_SHRINK` = 0.85) | it compares with a form that is not built and declines data that compresses to 0.85-1.05 of the records, which the form still serves better than the raw form does (2.6.1); replaced by the benefit gate |
| a compressing RocksDB arm in this move | not cut, unscheduled: the suite builds its comparators "without compression libraries" (`bench/Cargo.toml:31-33`), so it is its own piece of work, and R11 stands until it exists: no claim against RocksDB, in 7.8 or anywhere, before it |

### 9.5 Levers the reviews added, priced before anything is built (rev 3.2)

* **The hash's power-of-two step.** `hash_cap_for` is the smallest power of two that keeps the table at or under 3/4 load
  (`src/flatindex.rs:55-56`, `90-96`), so the bytes a key run from 10.7 to 21.3 as a partition's key count moves through an
  octave. The banked 10M store's partitions, cut at the 64 MiB limit (`src/db.rs:1828`, `18503-18508`) with about 450k keys,
  sit at 1,048,576 slots, 41% load, 18.6-19 B a key (M2). A limit where the key count lands at 0.62-0.75 of a power of two
  (about 393k keys, a file of 52-53 MiB) would carry 10.7 B a key: 80 MB less hash for the ordered load, 0.06 faults a read by D2,
  the size of L1's modelled gain, with no read-path change, no format change and no in-memory cost. Partitions that a merge
  splits land anywhere on the sawtooth (a split of a just-over-limit partition lands at 197k keys and 21 B a key), so the mean
  saving depends on the distribution and is 0 to 80 MB. It is an `Options` field already (`partition_bytes`, `src/db.rs:608`),
  hard-coded by the suite (`bench/src/engines.rs:3330`); the arm is `supdb-nocachepart<N>` (6.2). [U: the banked store's
  partition key counts were not kept.] Registered in 7.2b.
* **Holding the hash resident.** M17's 0.07 residual (7.2a, point 1) is a hypothesis of eviction. If stage 1a's `mincore` reads
  under 90% of the hash file resident, then an `mlock` or a populate of the 190 MB region every read touches is a ten-line,
  file-free, reversible lever worth up to 0.07 faults a read, and its coefficient is the one L1's 120 MB of heads and
  directory and the first version's 80 MB of heads for z would obey. If it reads 97-100%, D4's attribution is wrong the other
  way and the residual is something else; the next instrument is a per-region attribution of the baseline cell's faults with
  `perf record -e major-faults -c 1 -d`, bucketing data addresses by the segment mapping's regions [U: untried on this guest].

## 10. RISKS

| # | risk | why it is a risk here | what answers it | what stays open |
|---|---|---|---|---|
| R1 | records per page under 36 on the engine's own images (point reads), or under about 45 for the cache to hold the form with room | the offline 0.57 was Python on 116-byte records; the engine's records carry eight framing bytes; the probe's 60.0 had an 8-byte header and fixed records; keys with less redundancy than 16 sequential digits cost 11-31 bytes a record more (7.1) | stage 0b before anything; the stop; the key-shape grid | M13, M14 and M16 are not the engine's bytes [U] |
| R2 | decode costs more than 3 us on a cache-cold page | M14's 3.1 us is hot and the page's 64 lines come from DRAM through a mapping (a TLB walk under nested paging is dearer here); the decoder is `lz4_flex`'s unsafe build, not the C one the offline number came from | stage 0b times it; the memo removes it for hot pages; `t_z` is measured by the census, not typed | the standalone probe ran the engine's own (unsafe) codec, so its 3.6-3.8 us is no upper bound; the example would link the safe codec unless 0b pins the features [U] |
| R3 | the cold-start staircase, and a divided pass | a read cell of 333k reads against a cold 175k-page file faults 0.44 a read (D7); a divided pass is a different pass where its first operation builds what the rest reuse (`docs/engine.md` on divided passes) | `warm`, on the build thread (5.4); the staircase read as the evidence, not one cell; stage 1 measures pass 0 apart (v1 did: 0.166 against D7's 0.1656, M17) | warm's cost in this guest [U] |
| R4 | **the union leak**: any path still reading the raw hash, record or directory pages of a covered partition | a second form that accompanies the records only adds to the working set (`src/ordindex.rs:67-77`); "one point read in two hundred costs 7x"; the cache is charged for what a path touches | `SegBlob` with no `Deref` and no forwarder for any name that reads a record (3.9), `raw()` and `raw_touches`, the cursor and the seek by the fence (3.10, 3.4), the table's rows as fixtures, the residency probe on a landed store, the stray-read test, E3 as the net (4.5), `bg_read_bytes_per_op` in the row | the maintenance touches of 3.9 are bounded, not zero; a range with a level-0 piece touches the piece's pages beside the z pages (3.10) |
| R5 | the scan's working set at less redundant keys: 0.72 + 0.017 GB of 0.92 at 57 records a page (80%), 0.84 + 0.017 at 50 (91%); the first version's 80 MB of heads on top made 89% and 100% | 91% of the budget leaves no room for the memtable chain under writes (up to about 400 MB by arithmetic, `src/db.rs:13085-13127`) | the seek by the fence takes the heads out of the working set (3.4); the model reads `anon` each tick; the key-shape grid | scans under heavy writes in the bound [U] |
| R6 | the silent miss: a misaligned decode buffer makes `parse_record` return `None` for a full-form record, which a probe loop reads as "absent" | `src/flatindex.rs:520-523`, `1776-1783`; a compact-only fixture cannot catch it | `u64`-backed scratch, a refuse-and-count path, a test with full-form records and a deliberately misaligned buffer | |
| R7 | a stale sidecar serves another segment's values | ids are re-handed after a crash (`src/db.rs:18997`); the ordered index's binding is a key count | content binding by the checksum row's CRC, the name fields and the segment's sizes, and `bloom_sig` for the stored filter (2.5); the planted-sidecar test | |
| R8 | the exposure window after a merge or a promotion: a new partition is raw until its sidecar installs, 0.65-1.7 s a partition | the form's footprint win is only as good as its coverage; a store written across its key space rewrites every partition each cycle, at a rate near one serial builder's (5.6) | `rawserves`; promotion attaches before the publish; the wasted-build budget (5.2); merge-time build in stage 5; the registered mix cells never seal (7.3), so the lag sweep prices it | the cost on mix A beyond the registered cells needs a runner knob [U] |
| R9 | the census lies: a handle migrates threads, a window holds a scan, a callback faults, a write-heavy mix yields little evidence, a first touch looks like a capacity miss | `RUSAGE_THREAD` is the calling thread's; the suite's mixes commit in every window; a cold cache faults at about one a read | thread token, window classes (commits tolerated, scans their own census: 4.3), the refault evidence and the ski-rental price (4.4, 4.5), and **holding the rung when there is no evidence**; the model gates counting only | the cost of `getrusage` and the clocks on this guest [U]; handles on a thread pool are dropped; the kernel's accounting of faults that wait on I/O in flight [U] |
| R10 | flapping, and the trial's cost | a wrong switch costs a build (15-40 s) or a hot set's refault (5 s) | the ski-rental price, `dwell`, `retain`, the trial judged on the refilled state, a cancelled build counted as a switch, the square-wave test and the replay (7.4) | the constants are provisional until stage 4 [U] |
| R11 | the comparison is not like for like | `rocksdb-tuned` is uncompressed (`bench/src/engines.rs:4415`; the suite builds its comparators "without compression libraries", `bench/Cargo.toml:31-33`). Its 10M row is banked (M15) and `supdb-nocache` already beats it on every rate cell of 7.3 but the one-thread scan, so against this RocksDB the z form is needed for one cell. An LZ4-compressing RocksDB would hold the same data in about 0.66 GB [D: 1.16 GB at the offline 0.57, M13], inside the cache, and read at no more than the in-core rate this machine measured for it, 239-252k/s at 3M keys [M15], an upper bound for a read that decompresses and one that 7.3's predicted 120-230k/s does not clear. `lmdb-nordahead` is LMDB configured for the regime. A win over an uncompressed RocksDB is not a win over RocksDB as a user runs it out of core | a compressing RocksDB arm (LZ4 on) before any claim against RocksDB; the claim in 7.6 is against LMDB | the compressing arm's 10M row [U] |
| R12 | the payload and the keys are the form's best case | half a phrase, half entropy (`bench/src/workload.rs:194-231`), and sixteen sequential decimal digits (`db_key_into`, `bench/src/workload.rs:171`) that LZ4 codes as a match and two digits | the incompressible control on `ab` only (`--compress`), never on `run`; the key-shape grid of 7.1 (random decimal, random bytes, UUID text) | what real data compresses to |
| R13 | the host: balloon page prices move load and build phases 2.5-3x between sittings (`CLAUDE.md` profiling, "A fresh page's price here is the host's"); a three-thread cell is the scheduler's on four vCPUs (`bench/CLAUDE.md`) | the build writes 0.72 GB of fresh page-cache pages | counts first, two sittings with the order swapped, the floors read first | |
| R14 | the in-memory branch moves a read by code layout rather than instructions | a two-instruction change in a monomorphised generic can; the pin's fence once read as 25% and was nothing | callgrind for the instruction count; two binaries alternated in one sitting and again in a second; the arm-against-itself coin rate | |
| R15 | a lifecycle bug of a class this repository has met | attach is not a publish (`src/db.rs:26949-26975`); a join blocking the thread everything lands through; a flag set before the call that resets it; a sweep that reads pins before the list; a derived thing keyed by position; a test that bets a seal is still running; a counter read that takes the upkeep home; an absence whose meaning a publish changed | the cell and the gate are written in the order that makes a partial state served raw, never wrong; the build is polled, never joined; the pins and the retire list are the forms' own; cells are keyed by identity; tests hold states (`hold_zpage_build`); counters read from `Shared` alone | |
| R16 | disk: about 10 GB free on this host when read (`df`); a forced 10M pass needs 4-5 GB; `bench run`'s guard knows 3x the records (`bench/src/run.rs:803-814`) and not the sidecar; `ab` has none | the load's measured 3.1x was without the form | re-measure the guard's factor with the form; give every pairing a `TMPDIR` of its own | |
| R17 | the default policy is killed under the bound at 3M by the forms' memory | `docs/engine.md:5920-5928`; commit `5dbfa23`: 760-820 MB of anonymous memory at the full-rewrite lag point against 225-255 for no cache | not this move: the nocache policy carries the rows; `forms-lead.md` is the lead; 5.8 states the coupling for the follow-up; **7.8(b) gates the flip of the default on it** | the row for the default arm stays absent, and its absence is not a pass |
| R18 | the browser half of "the three readers agree" is not in this tree | `tests/blob.rs:485` names `web/test/node.mjs`, which does not exist [U] | nothing compiled for wasm; `web/build.sh` bytes before and after | |
| R19 | the reopen's verify pass makes the raw key section resident at every open, and merges read 1.3-1.5 GB of raw input a cycle | `src/db.rs:18917-18924` reads every byte of every segment's key section; the engine drops nothing behind a merge (5.8) | attach is lazy; raw pages age out as z pages are touched; a restart in a bound environment pays one raw pass first; a COMPACT-only drop-behind in `compact_run` is a separate gated change, priced as an arm if the verdict shows merges displacing the sidecar | |
| R20 | the sidecar's folio composition under a 1 MiB write piece is unmeasured | a folio is sized by the write that makes it, and a file written one way reads another (`CLAUDE.md` "A write pattern priced on the bytes it wrote") | the piece is a named constant; a `pagemap` + `kpageflags` walk at three piece sizes in stage 2a | [U] |
| R21 | two controllers in one cgroup (two stores, or a store beside another process) | each store's model says "fits" | the pressure term (4.2); `two_stores_in_one_process_do_not_share_a_measure` | cross-process interplay [U] |
| R22 | the form is wrong for a data shape: values over `inline_bytes`, multi-value keys of three or more 100-byte values (a run over 256 bytes goes to a block), tombstone-heavy partitions | z holds the records; the blocks stay raw and fault in both forms | the skip rule counts block-backed bytes in `W`; mixed partitions are copied verbatim and stay exact | the gain on such stores is small and declined |
| R23 | the counters bump shared lines | per-read counters on `Shared` are a line every handle writes: `CLAUDE.md` "A slot table whose slots share a line" (four handles at 0.46x of LMDB with thirteen shared statistics) | `ZStat`, one line a slot, summed on read (6.4); the shared shape kept as the arm `supdb-nocachezpagessharedcounts` and paired at three threads (7.5) | the line costs 264 slots x 128 B = 34 KB [D] |
| R24 | portability: the engine builds and tests on Linux x86, Linux ARM and macOS arm64, and `RUSAGE_THREAD`, cgroups, `mincore` and `fadvise` are Linux's | `libc` 0.2.191 defines `RUSAGE_THREAD` for no Apple target (2.9) | every Linux-only item gated with a no-op elsewhere, `Auto` inert off Linux, hosts whose page is not 4 KiB decline | `Always` on a 16 KiB-page host is correct and not one fault a read [U] |
| R25 | the evidence lives in a session scratchpad | the probe, M17's block and the 10M rows are not in the repository, and `CLAUDE.md`'s rule is that a number lives in a banked row | stage 0a | M17's per-pass output is gone [U] |
| R26 | the window: no single-fault form that decodes reaches LMDB's reads past about 7 times the cache | D9: both take one fault a read and the decode adds three microseconds | stated in 1.4, 7.3 and 9.1; "everywhere but disk" is not claimed past the window | open question 1 |
| R27 | the stored filter's bits are a function of engine code | an edit to `BlockedBloom` that changes no format quantity would make an old sidecar answer "absent" for present keys, with nothing raised (2.5) | `BLOOM_SIG`, its pin test and the fence-key membership check at attach | the first sidecars of a new engine version are rebuilt and not attached whenever the signature moves |
| R28 | the controller's evidence in the suite's own mixes | A, B, D, E and F commit in every 100 ms window (`bench/src/run.rs:309`); E and a scan-only workload have scans in every window | commits tolerated, a scan census (4.3) | handles on a thread pool are dropped; the kernel's accounting of faults under I/O in flight [U] |

## Appendix A. Not found, not measured, not verified

Not found in the repository or on this host, said plainly:

* Any z-page code or text: no `zpage`, `z-page` or `zpg` in any `*.rs`, `*.md`, `*.toml` or `*.sh`.
* A file defining the pre-registration list (CELL, DECOMPOSITION, CHANGE, PREDICTIONS, VERDICT, STOP RULE,
  DONE WHEN): its form is in `scratchpad/ooc/prereg-rung1.md` and `forms-lead.md` only.
* Engine code that reads a cgroup, `/proc/meminfo`, `/proc/pressure` or `ru_majflt`: `src/` has none; the
  only cgroup code is `bench/src/bound.rs`, v1 only.
* An `ooc` row under `bench/runs/` (only `full` and `quick`); the 10M rows are in
  `scratchpad/ooc/rung1-10m` and are not committed.
* `web/test/node.mjs`, named at `tests/blob.rs:485`.
* Cancellation of a seal or a merge; a sweep at open of `seal-N.tmp`, `compact-N.tmp`, `tier-N.tmp`
  (`src/db.rs:20288`, `8923`, `9082`), which is why the sidecar's temp name must sit under a swept prefix;
  a directory lock between processes.
* A public way to read a segment's record bytes or its checksum row from `Blob` (`Blob::flat` and the index are
  private, `src/blob.rs:588-591`, `723`); a function that returns a record's length without rebuilding a
  compact extent.
* `fadvise`, `DONTNEED`, `MADV_PAGEOUT` or `MADV_COLD` anywhere in `src/` (grep): the engine drops nothing behind any read,
  merge or open, which section 0 of the first version said it did (rev 1.13).
* A definition of "the tombstone-free identity rule", which the first version of 7.8 told `CLAUDE.md` to carry; it is dropped
  and 7.8(a) names the identity rule 2.5 actually states.
* The 10M `rocksdb-tuned` row is banked and is M15; the standalone probe's 10M result is M17. The probe's per-pass output file
  is no longer on disk, so M17 rests on the result block of `prereg.md` and on the raw layout's first three passes (M16); the
  probe was not re-run for this document.

Not measured (everything in [P] and [P2] is also unmeasured; these are the inputs it stands on): the ratio of the engine's
124-byte record images, per key shape (stage 0b); `lz4_flex`'s unsafe decode of 7 KB groups on this guest, and its compress
rate; a cache-cold page's decode; the cost of `getrusage(RUSAGE_THREAD)`, `mincore` and the two clocks on this guest; the three
instructions of the branch; the folio composition of a sidecar; whether the suite's 10M partitions carry commit markers (the
builder handles both); the 2.9% and 3% record-straddle fractions (arithmetic, 30 of 1,024 phases); the residency a forced
build leaves; `warm`'s cost; every machine time of 8 beyond the derived; why removing the hash is worth 0.07 faults a read more
than its footprint (M17, 7.2a); the hash file's residency (stage 1a); the partition sizes and key counts of the banked 10M
store; the cause of the two lag points 7.3 says this move does not claim; what the build costs the readers meanwhile; whether
the kernel counts a fault that waits on another reader's I/O as major; whether `posix_fadvise(DONTNEED)` skips pages the
caller's own mapping holds (4.3, 5.8: both from knowledge of the kernel, not run in this tree); the stray-read result at 80%
occupancy; whether `perf record -e major-faults -d` and `perf report --sort dso_daddr` name the mapping a software event's data address fell in (7.2b); the 7.2b rows.

Verified by reading the code at HEAD for this document and for the revision (and corrected where the first notes were off):
the record, compact-record and page-parse layouts; the hash probe and `hash_cap_for`; the directory and the fence; the key
section's header, checksum row and its unconditional presence; the superblock and its extension; commit markers; `Blob`'s read,
scan, count, seek and `read_exts` signatures; `block::compress` and `decompress_into`; the `lz4_flex` 0.11.6 `compress_into`
size check, decoder termination and feature gates, and the features the engine, the probe (by its build fingerprint) and the
bench dev-dependency select; `Reader::read_all`, `count`, `advise`; the `scan_at`, `values_at`, `exts_at`, seek and cut sites
of 3.9, including `scan_partitions`, `scan_merged`, `BuildCtx::cut_at` and `OrdIndex::exact_at`; `Seg`, `Seg::open_full`,
`open_ord`, `retire_seg`, `apply_promotion`, the open's sweeps, `Maint`'s loop and `stop`; `RetireList`, `Readers`, `Slot` (nine
words, 72 B), `Shared`; `check_options` and the tests and arms that set `publish_in_background` or `writer_pins` off; the sweeps
of the retire lists; `merge_due`, `threshold_for` and the seal threshold; the ordered index's open, binding and `whole_key_at`;
the `ycsb` driver, `YCSB_BATCH` and `ycsb_ops`; the bench arm sites; `cmd_ab` (it builds a `Quick` plan and never reads
`--ops-div`); the CI matrix; `libc` 0.2.191's `RUSAGE_THREAD` definitions; `memmap2` 0.9.11's `unchecked_advise_range`;
`bench/src/bound.rs`' `memory.stat` keys and this host's v1 `memory.stat` (it has `workingset_refault_file`); the cgroup layout
of this host (a v1 memory controller beside a v2 mount that lists `hugetlb` alone). The first notes' line numbers were off by a
few lines in about a dozen places (`format.rs:46` is `:43`; `ord_name` at `:18525`; `BlockedBloom` at `:2367`); the corrected
ones are the ones in this document.

Taken from other sessions' notes and files without re-deriving them, and so to be re-checked when the numbers are used:
the reading of `bench/src/run.rs` pass counts (`:277-283`), the disk figures for this host, the Python offline compression
figures (M13), the zprobe smoke test's numbers (M14) and the probe's 10M result block (M17). The RocksDB 10M numbers of M15 and
7.3 were re-read from the banked JSON (`rung1-10m` and `rung2-10m`).

## Review record

Three adversarial reviews of the first version gave 15, 18 and 14 findings, 47 in all. `r.n` below is review `r`, finding `n`, in the
order the review listed them, and the text carries the same tag as `(rev r.n)` wherever it changed. Each finding was checked against
the code it cites, read at the pinned commits, or against the arithmetic, recomputed in a few lines of Python; nothing was built,
tested, run or timed, because the machine was held by timing runs. Where a finding rests on kernel behaviour or on a figure that
cannot be read here, the disposition says so and Appendix A lists it. **No finding was rejected whole.** 32 are accepted in full
and 15 in part; the parts that were not taken are listed first, because that is where a reviewer and this revision differ, and
each has its reason. The tables after it say, finding by finding, what changed and where.

### Parts not taken

| finding | the part not taken | why |
|---|---|---|
| 1.1 | the reviewer's remedy for the partition cursor of `scan_merged` and for `cut_at`: read the keys from the ordered index's heads (`whole_key_at`, `src/ordindex.rs:488`) | a head names a whole key only when every key has one length within a head (`src/ordindex.rs:28-35`, `864-887`), which is the suite's shape and no other; the heads are 80 MB that the seek by the fence takes out of the compact form's working set (D10: the scan claim's 2x needs 43.1 records a page with them resident and 39.3 without); and a key borrowed across loop iterations needs a cursor that owns its group for the shapes where the heads do not apply. The z cursor of 3.10 serves every key shape. The heads stay for `same_blocks` and as the arm `supdb-nocachezpagesheads` |
| 1.11 | the premise that a write-heavy mix seals every few seconds at 32 MiB | `seal_grows` is on by default (`src/db.rs:1817`) and `threshold_for` (`src/db.rs:13086-13100`) takes `max(seal_bytes, store/16)`, about 101 MB at 10M keys: a seal about every 6 s of mix A and a merge cycle (four pieces) about every 26 s, and the registered mix cells (83k updates, 12 MB; `Plan::divided`, `bench/src/run.rs:277`) never seal. The rate race the finding describes is real at that cadence and is derived in 5.6 |
| 1.13 | a COMPACT-only drop-behind in `compact_run`, as part of this move | it edits the merge every store runs, for a displacement nobody has measured; it is a separate gated change, priced as an arm if the verdict shows merges displacing the sidecar (R19) |
| 2.2 | the x_max of 14-21 times the cache and the net loss of the z form at 60 times (44 us against 42 us) | both come from a raw model capped at one fault a read, `1.15 - 1/x`. Once the 190 MB hash stops fitting, a raw read faults on the hash page as well: Che's approximation over the two populations (4.2) reproduces the cell, 0.428 against 0.425, and gives 1.94 faults a read at 60 times the cache, so a raw read is `0.73 + 41.5 * 1.94 = 81 us` against the z form's `4.0 + 41.5 * 0.99 = 45 us`, and the saving never falls under 0.43 at any size in D9's table (0.54 or more from 15M keys on, out to 200M). The gate and the exit the finding asked for are in; what they are for is no compression, a hot set that fits, and a cache that grows |
| 2.4 | one fresh store per workload for stage 4's ooc rows | a 10M load and a build are minutes before the first cell. Taken for 4b's probe, which takes each workload (scan-only, B-shaped, A-shaped) to a fresh store, and for the replay's cold traces (7.4) |
| 2.6 | `pread` in 1 MiB chunks, each followed by `fadvise`, as the stream | the builder parses records out of a mapped `Blob` (`Blob::record_regions`, 2.6 step 1), so it keeps the mapping and zaps the consumed window with `MADV_DONTNEED` before `posix_fadvise(DONTNEED)`; `pread` is the fallback if `zresident` shows the zap does not drop the pages |
| 2.8 | a 300-line engine spike (`Blob::record_regions`, a stage-0 sidecar, a hidden-option `read_all`, no lifecycle) as its own step before stage 2 | it is stage 2a, at 900 lines and not 300: an in-situ `t_z` and a three-thread number are admissible only from a file that passed attach (trailer CRC, tiling check, identity), else they are read off a file nothing checked |
| 2.18 | stage 6 pins `ZPages::Off` in the inline and unpinned arms and tests | with `Auto` degrading to `Off` where it cannot run (1.3) those arms need no pin; the test that `Options::default()` opens over each of those option sets runs before any flip |
| 3.1 | "neither stage is a gate", for stage 0b | conceded for 0b's stop on the suite's shape (36 records a page against a prediction of 57 and the probe's 60.0): it is not expected to fire and is kept because it is free. Not conceded for the stage: 7.1's ranges for fill, compress calls, decode and records a page by key shape can refute, and its product is the one layout the engine and the probes share |
| 3.2 | `mlock` or a populate of the hash as a step before the residency answer; the fault attribution as an unconditional stage | the lever is ten lines and moot if the hash reads 97-100% resident, so it waits for 1a (9.5); the attribution is an optional run of 1a (by file, on the probe) and one untimed run in 1b (by region, on the engine), both [U] on this guest |
| 3.5 | `warm` moved to stage 4; `ZNOPAGE` and `STORED` conditional on a census of real stores | warm is in 2b and not in 2a, and it skips chunks `mincore` reports resident (5.4), so a forced build that leaves its file resident (the reviewer's guess, [U]) costs it one pass of `mincore`; 7.3 registers the first read cell with warm on. `ZNOPAGE` and `STORED` stay in 2b: the incompressible control of 7.3 is `STORED` pages and a mixed partition (R22) needs `ZNOPAGE` |
| 3.8 | symbols in place of line numbers; a shorter document | the task's rule is file:line for every claim about the code and `CLAUDE.md` cites lines; the revision states once that every `src/`, `tests/`, `docs/` and `.github/` line is valid at both pinned commits. The front page is the one-page summary asked for; the document is long because it is the pre-registration |
| 3.9 | run A and F at `--ops-div 1`, or add a write burst before them | at `--ops-div 1` mix A writes about 117 MB (833k updates at the document's 140 B a write [U]) against a seal at 101 MB: one seal and no merge (four pieces are needed), so neither the merge nor the rebuild race is reached. The lag sweep's bursts reach them and are registered (7.3); a mix soak needs a runner knob that does not exist [U] |
| 3.11 | a rate term in the controller (device bytes of builds a second against reads served) | the wasted-build budget (5.2) is the signal of rebuild churn, `build_bytes` is read beside the reads in verdict item 3, and a rate against a rate adds two constants to a controller whose constants stage 4c has yet to measure |
| 3.12 | rung names `Raw`, `Ord`, `Z` | the rungs name the evidence state (`FITS`, `SUSPECT`, `COMPACT`) and the gate word stores the form wanted, with a value reserved so that L1 and L2 plug into the one selector (4) |

### Review 1

| finding | what it said | where | disposition and what was checked |
|---|---|---|---|
| 1.1 | major: the nocache scan paths read the raw hash, directory and records of a covered partition (`cut_at`, `scan_merged`'s cursor), and "the compiler lists every site" is not a mechanism | 0, 3.8-3.10, 6.3 | **Accepted in part** (remedy above). Both shapes are real: `cut_at` is reached from `scan_partitions` (`src/db.rs:17841` into `25702`, `25706`), and `scan_merged` reads the partition's key after every emitted key and keeps it across iterations (`18058-18059`, `18074`, `18145`, `18149`). 3.9's table has them, and the level-0 cursors (`18042`, `18046`, `18161`) as raw by construction; 3.10 designs the cursor (`ZCursor`, `cut_in`) and says what is raw until stage 3 (`raw_scans`). `SegBlob` has no forwarder for any name that reads a record, which is what makes the compiler list each site, and `raw_touches` with a test per shape of the table is the check |
| 1.2 | major: the persisted Bloom filter is a function of engine code that nothing in the trailer fingerprints | 2.0, 2.5, 3.2, 6.3 | **Accepted.** The trailer carries `bloom_probes`, `bloom_bits_per_key` and `bloom_sig`; `BLOOM_SIG` is pinned by a literal CRC32C over a filter of 4,096 fixed keys (the failing test says to bump it and the sidecar version); attach requires every fence key to test positive; the stale comment at `src/db.rs:2438-2440` is on the edit list. An edit to `slots_of` (`src/db.rs:2476-2495`) would have turned present keys into "absent" with nothing raised |
| 1.3 | major: `check_options` refuses `Auto` beside `publish_in_background: false` or `writer_pins: false`, and stage 6 makes `Auto` the default | 4.1, 6.1, 6.3 | **Accepted.** `Auto` is never refused: it behaves as `Off` and is counted (`auto_inert`); only `Always` beside an unpinned writer is refused. `options_default_plus_each_inline_and_unpinned_set_opens` opens `Options::default()` over the option sets of `tests/db.rs:607`, `616`, `8355`, `9327`, `11712` and the arms `supdb-inlinemaint` and `supdb-nopin` (`bench/src/engines.rs:3480-3486`), and runs before any flip |
| 1.4 | major: per-read counters on `Shared` | 4.6, 6.1, 6.2, 6.4, 7.5 | **Accepted** (the same defect as 2.11 and 3.10). `ZStat`, a 128-byte line per reader slot written by its handle alone and summed on read as `Readers::stat` sums the slots; `Shared` keeps events (builds, attaches, retires, switches). The `Slot` has nine words, 72 B (`src/db.rs:6665-6686`), so the census's seven words fit exactly (72 + 56 = 128 B) and five counters more would not. The shared shape stays as the arm `supdb-nocachezpagessharedcounts`, priced at three threads |
| 1.5 | major: `warm` runs on the segment-work thread | 5.4 | **Accepted.** Warm is a job of the idle-class build thread, chunked, cancellable, holding no pin; the segment-work thread attaches, installs and stores the gate and nothing else. The loop of `Maint::run` (`src/db.rs:25995-26015`) and `FROZEN_CAP` = 3 (`10015`) make the stall the finding describes real |
| 1.6 | minor: `fill_group`'s shrink reaches `m = 0` and divides by zero | 2.6, 6.3 | **Accepted.** Checked: `m = 2, len(c) = 9000` gives 0, 8144 gives 1, 8145 gives 0, and `m = 7, 35700` gives 0. The function is total: `m` never reaches 0, the loop ends in at most twelve turns, an image that fits `ZCAP` is `STORED` before any shrink, and `NOPAGE` is returned only where a lone record cannot fit. 0b's example has the 5,000-byte-key and 4,056-4,072-byte incompressible fixtures (`fill_group_is_total`) |
| 1.7 | minor: rebuild after damage is unbounded, and step 6 does not check header against fence | 2.6, 2.7, 6.3 | **Accepted.** Step 6 runs `verify_page(page, fence_entry)` for each page it writes; rebuild after damage runs at most three times with the doubling backoff, then `declined_damage` (`Seg::z_declined`, `z_rebuilds`) |
| 1.8 | minor: three windows between a build and a merge or promotion | 2.6, 5.2, 5.6, 6.3 | **Accepted.** (1) The collect step renames a finished build's file whose target is gone to a dead name and counts `build_orphaned`; (2) the segment-work thread opens the input and hands the `File` to the build thread at spawn, so a promotion's unlink of the old name (`src/db.rs:27130-27142`, `27449-27455`) cannot fail the open; (3) a retired sidecar is renamed to a unique `zpg-dead-N`, queued for an idle unlink as the manifest spares are (`src/db.rs:8355-8385`, `27543-27563`) and swept at open |
| 1.9 | minor: `retired_z` is swept once, at the push | 4.8, 6.3 | **Accepted.** `retired_z` is swept wherever the other retire lists are (the writer's `leave`, the `Maint` tick; `src/db.rs:14906-14909`, `12740-12755`). A `Snapshot` handle held across a landing defers the munmap and the disk, which 5.9 states and the test asserts |
| 1.10 | minor: scan-only workloads yield no evidence; "exactly, no false positive" is too strong | 4.3 | **Accepted.** Scan windows have their own census and windows with commits count. "Exactly" now says "faults that missed the page cache"; the kernel's accounting of a fault that waits on another reader's I/O is [U], and stage 1a tests it with a `WILLNEED` in flight. Handles on a thread pool stay dropped, said and not fixed |
| 1.11 | minor: the exposure window is a rate race priced only as build wall | 5.6, 7.3, 7.6 | **Accepted in part** (premise above). 5.6 derives the rewrite rate against the rebuild rate; 7.3 re-derives mixes A and F with a refutation line at 0.9x; `Always` cells report coverage as a covariate; `rawserves`, merge cycles and builds superseded are in verdict item 3; `Auto` is bounded by the budget of 5.2; building inside the merge stays stage 5 |
| 1.12 | minor: the stage-0 stop is three records early | 1.3 (D10), 7.1 | **Accepted.** Recomputed: `(B - f)/(1 - 0.198) = 1.147 GB`, 280k pages, 35.7 records a page; 38.6 was the figure for `B - f` = 0.84 GB. The stop is "under 36", and D10 lists the thresholds (35.7 parity, 40.1 for 1.5x, 39.3 and 43.1 for the scans). D7 at 175k pages is 0.1744 |
| 1.13 | minor: the engine drops nothing it reads; `fadvise` skips mapped pages; the build's write side displaces | 0, 5.8, Appendix A | **Accepted in part** (above). Section 0 no longer says the engine drops what it reads (no `fadvise`, `DONTNEED` or `PAGEOUT` in `src/`, by grep); the build drops behind itself; the write side (0.72 GB of new page-cache pages for the build's wall) is in the ledger and in the readers' cost during the build (7.6 item 6, `displacement_est`). The kernel claim about mapped pages is from knowledge and not run |
| 1.14 | minor: `same_blocks` reads cold pages in COMPACT | 3.9 | **Accepted.** Skipped when no forms are kept, taken from the heads otherwise; priced at 0.28 s of major faults a landing, on the thread everything lands through (`src/db.rs:21359-21368`) |
| 1.15 | minor: six loose ends | 2.0, 2.2, 2.4, 2.8, 5.3, 6.1 | **Accepted in full.** (1) `record_key_next` calls `record_len_compact`, the one owner of that arithmetic; (2) "the tombstone-free identity rule" is dropped, defined nowhere; (3) the census and cgroup reads are `cfg(target_os = "linux")` and `Auto` is inert elsewhere (2.9); (4) attach takes the store's advice (`src/db.rs:5435-5437`; `zpages_advised`); (5) `Seg::z_declined` is listed in 6.1; (6) the page CRC is XORed with the trailer's `bind32` |

### Review 2

| finding | what it said | where | disposition and what was checked |
|---|---|---|---|
| 2.1 | major: the census cannot tell a first touch from a refault; a streak of three 100 ms intervals starts a 15-40 s build; no regret accounting | 4.3-4.5, 4.7, 7.4 | **Accepted.** Evidence is faults of pages the cache once held (`workingset_refault_file` at half the store's faults over the last 5 s; a first touch is not a refault); the entry is a ski-rental price (`C_build`, `excess`), which bounds the regret to twice the price; a cancelled build counts as a switch and doubles the back-off; the cold-start and burst traces are in the replay of 1d. "No false positive" is true of the count, not of the inference, and 4.3 says so |
| 2.2 | major: nothing asks whether the form would fit or help; no "not paying" exit | 1.3, 2.6.1, 4.2, 4.4, 4.5 | **Accepted in part** (x_max above). The entry tests the predicted saving `phi_hat_raw - phi_hat_z` against `1.5 saving*` (`theta_enter`); E3 leaves when the realized saving stays under `saving*` for a dwell; the sampler's `W_z` feeds `phi_hat_z`; `a_store_far_past_the_cache_still_builds` is a model-only synthetic fifty-times store |
| 2.3 | major: the trial decides on the fill, not the refilled state | 4.7 | **Accepted.** The trial concludes after its faults reach the sketch's predicted raw pages, or the rate has stopped falling, and is budgeted at the predicted refill; a 100 MB hot set returns to raw and a 600 MB one stays (a test and a replay trace) |
| 2.4 | major: windows with a commit or a scan are dropped, so `Auto` cannot enter for a mix or a scan | 4.3, 7.4 | **Accepted in part** (the fresh-store rule, above). Commits no longer drop a window: they corrupt only `t_f`, which is taken from commit-free windows or the sampler's cold touches. The scan census has its own break-even, `(11.3 - 3.3)/41.5 = 0.19` faults a scan against 2.9 measured (M7). The suite hid the problem by pass order (`YCSB_ORDER`, `bench/src/run.rs:308`) and 4.3 says so. 7.3's predictions are for `Always`, and the shipped `Auto` is not claimed from them |
| 2.5 | major: the no-raw-touch rule is false for `cut_at`, `scan_merged` and `exact_at` | 0, 1.3, 3, 3.4, 3.9, 3.10, 6.2, 6.3, 8 | **Accepted** (the same defect as 1.1). The seek by the fence is the default in stage 3 (the first version made it a conditional 3b), the cursor is O(1) a key, `cut_in` is z-aware, `exact_at` is unreachable for a covered partition (`src/ordindex.rs:864-905`), and the table lists `18074`, `18145`, `18149` and `18161`. The heads path stays as the arm `supdb-nocachezpagesheads` |
| 2.6 | major: the build streams 1.24 GB through the LRU; `fadvise(DONTNEED)` skips mapped pages | 5.8, 6.3, 7.6 | **Accepted in part** (`pread` above). `MADV_DONTNEED` over the consumed window of the build's own mapping, then `posix_fadvise(DONTNEED)`; the kernel claim is from knowledge and `zresident` measures it. The readers' faults and rate through a build are verdict item 6 and are charged to the ski-rental price |
| 2.7 | major: the fit has been run once, at 79% occupancy, with the best-case keys | 1.4, 7.1, 7.2b | **Accepted.** 7.1 has the key-shape grid with its byte accounting (sequential decimal 57, random decimal or bytes 48-52, UUID text 40-46) and the occupancy at 10M (79, 89, 99 and 111%); 7.2b registers the knee sweep (ballast 100-400 MB, with and without an 80 MB heads stand-in) as the first experiment; C1 says how much room each density leaves |
| 2.8 | major: the plan does not start with the cheapest decisive experiments | 7.4, 8 | **Accepted in part** (the spike above). The order is 0a, 0b, 1a (the knee sweep, no code), 1b, 1c, 1d (the selector replay, no machine), 2a; the codec is pinned and printed in every new example |
| 2.9 | minor: the memo's hit rate, the walk's cost, `ZMAX_ULEN` | 3.2, 3.7 | **Accepted.** Recomputed: the static optimum is 0.589 and a 256-entry direct-mapped memo hits 0.45. The memo is 1,024 entries capped at 8 MiB a thread, hit rate 0.55; a hit pays the walk (28 `key_cmp` compares, `src/db.rs:9443-9464`, 0.85 us); mix C is 260-350k, central 300k. Count-min admission is the arm if the direct-mapped rate lands under 0.45 |
| 2.10 | minor: the decline test compares with L2, which is not built | 2.6.1, 6.3 | **Accepted.** The fixed ratio is replaced by the benefit gate; `ZMAX_SHRINK` is kept as `ZMAX_SHRINK_L2` for the day L2 exists; L1, chosen by hand in stage 1b, is the declined partitions' fallback |
| 2.11 | minor: per-read counters on shared words | 4.6, 6.1, 6.4 | **Accepted** (see 1.4) |
| 2.12 | minor: a `WILLNEED` syscall on every z scan | 3.5 | **Accepted.** The hint is issued only when the thread's last closed census window faulted at 0.05 or more a scan |
| 2.13 | minor: six arithmetic slips (a)-(f) | 1.3, 3.2, 4.2, 4.7, 5.4, 7.1 | **Accepted in full**, each recomputed. (a) 35.7 for `B - f` = 0.92 GB (point reads), 38.6 for 0.84; (b) D7 0.1744; (c) 5.4's headroom no longer subtracts `anon` twice; (d) the worked numbers use the measured cache, 937 MB, and S1's `B` also takes the margin off, about 870 MB at the cell where `phi_hat_raw` reads 0.48 (4.2 says so); (e) the Bloom false-positive rate at 10 bits a key and four probes per 512-bit block is 1.3%; (f) E2's bound counts the hash pages a hot z page's keys hash to (up to 57) and sums per-handle sketches |
| 2.14 | minor: the probe's decoder is the engine's, not a safe one | 7.1, 7.2a | **Accepted.** Checked in `scratchpad/zprobe/Cargo.toml` and the build fingerprint: `["std"]` without `safe-decode`, the unsafe decoder the engine ships (`Cargo.toml:15`; `lz4_flex-0.11.6/src/block/mod.rs:21-28`). The probe's 3.6-3.8 us is no upper bound, `t_z` is 4.0-4.7 (central 4.4), and 0b pins the features and prints the codec |
| 2.15 | minor: E1 flaps; S1 runs at 100 Hz; 7.4 contradicts 4.5 | 4.2, 4.4, 4.5, 7.4 | **Accepted.** E1 needs the maximum `phi_hat_raw` over a `dwell`, smoothed over 30 s, under `theta_sus/2`; S1 runs once a second in every rung and the 10 ms turn only sums and polls; 7.4 says two S1 ticks |
| 2.16 | minor: the pressure term cannot open the build | 4.2, 4.5, 6.3, 7.4 | **Accepted.** Build clause (c) is `phi_hat_raw >= theta_sus` or the pressure term; the neighbour's page cache is a replay trace and a test |
| 2.17 | minor: exposure priced at CPU, not wall; no budget for superseded builds | 5.2, 5.6, 6.3 | **Accepted.** Priced at the wall, 0.65-1.7 s a partition; `Auto` suspends building for a `dwell` when more than four of its last eight builds were superseded (`build_suspended`); `Always` reports coverage |
| 2.18 | minor: the default flip makes the inline and unpinned arms and tests refuse to open | 4.1, 6.1 | **Accepted in part** (above): solved by degrading, which is 1.3 |

### Review 3

| finding | what it said | where | disposition and what was checked |
|---|---|---|---|
| 3.1 | major: the thesis gate was passed by M17; v2 has no registered range; the stage-0 stop cannot fire | 7.2b, 8 | **Accepted in part** (above). v2 is retired as a separate program: its page header, CRC and walk would have been a second definition of the layout, so the layout is 0b in the real module and the example runs the real `fill_group`. The new axes have ranges registered before they run (7.2b: the knee with and without heads, hash residency, faults by region, stray raw reads, three threads, the raw-to-z switch, the bound axis, hash sizing, key shapes); Zipf is priced on the engine in stage 3, where the memo is |
| 3.2 | major: the 0.07 residual, the hash's power-of-two step and the fault attribution are in no stage | 1.3, 6.2, 6.3, 7.2b, 9.5 | **Accepted in part** (above). (a) `mincore` over the hash file is stage 1a, with the range 97-100% resident and the refutation under 90%; (b) the hash's step (`src/flatindex.rs:55-56`, `90-96`; `partition_bytes`, `src/db.rs:608`; hard-coded at `bench/src/engines.rs:3330`) is 9.5 and the optional arm `supdb-nocachepart<N>` in 1c, [U] because the banked store's partition sizes were not kept; (c) faults by file on the probe (1a, optional) and by region on the engine's baseline cell (1b), registered at hash 0.00-0.02 a read, the two-population model's 0.003 |
| 3.3 | major: L1 and the incompressible control are sequenced and registered so that they cannot do their job | 6.2, 8, 9.2 | **Accepted.** `ab --ops-div`, finite `load_bpb` and `disk_bpb` without the load pass, and `--compress` on `ab` are stage 0a (`ab` builds a Quick plan, whose divisor is 1: `bench/src/main.rs:249-256`, `bench/src/run.rs:264-267`). 1b runs before any sidecar line, with 9.2's range (0.33-0.38 faults, about 61k reads/s) and a number in its stop (level on faults, under 0.02 apart, in both sittings). L1 is the declined case's hand-chosen fallback until L2 exists. The incompressible control is a row of 7.3 |
| 3.4 | major: nothing brackets the regime where the form does not fit | 0, 1.3, 7.2b, 7.6 | **Accepted.** D9 models the window (z over `lmdb-nordahead` 3.3x at the cell, 1.5x at 18.7M keys, 1.0x at 50M, 0.94x at 90M; over the raw form 1.75x or more past 3 times the cache); 7.2b registers the bound axis (768, 640 and 512 MiB) with predicted ratios; entry is on the predicted saving, E3 leaves, 7.4 has the fifty-times store, and verdict item 7 reads the window's edge |
| 3.5 | major: stage 2 is one 2,400-line stage bundling the proof with shapes the cell lacks | 8 | **Accepted in part** (above). 2a (900 lines): the format with `E = P`, no `NOPAGE` and no `STORED`, a partition with an irregular record declined, attach with the trailer CRC, the tiling check and sampled pages, `read_all` and `count` behind one gate load, no `SegBlob`, filter, memo, warm or controller. 2b: lifecycle, damage, identity, the Bloom filter, `ZNOPAGE`, `STORED`, `ZStat`, warm. 3: scans. Each stage has a DONE WHEN and an `Off`-default state that merges |
| 3.6 | major: DONE WHEN requires the default flip that the scope excludes | 6.1, 7.8, R17 | **Accepted.** Two tiers: (a) the default `Off`, the forced arm and `Auto` behind the option, merged on nocache evidence with the 3M control and the quick row clean; (b) the flip only when the forms-memory lead (`docs/engine.md:5920-5928`) is closed. Until then `Auto` serves no policy that builds forms, and the default policy under the bound is an expected "killed or never leaves FITS" (7.4) whose absence is not a pass |
| 3.7 | major: no portability plan | 2.9 | **Accepted.** 2.9 gates the census, the cgroup readers, `Auto`, `zresident` and the `/proc` and `mincore` tests on Linux, with `Auto` inert elsewhere; `libc` 0.2.191 defines `RUSAGE_THREAD` for no Apple target; a host whose page is not 4 KiB declines (R24); "compiles on all three CI legs" is in the DONE WHEN of 0b and 2a |
| 3.8 | major: the evidence cannot be re-read; the page is a session log; it is long | header, Summary, 1.2, 8, Appendix A | **Accepted in part** (above). Done: a front summary after the header; the session-log voice and the timestamps taken out of 7.2 and 7.2a (M16 and M17 keep theirs, as the provenance of a measurement); stage 0a banks the probe's source, `prereg.md` with its RESULT block, the scripts, the logs and, if the person decides, the 10M rows, and re-runs v1 once with its output captured. Banking is a repository write, which this revision did not make |
| 3.9 | major: registered cost checks cannot fire (the mixes never seal; ranges missing) | 7.3 | **Accepted in part** (above). The cells never seal, and the first version's claim that A and F catch the exposure window is withdrawn; ranges are registered for `load-shuffled`, `lag-burst`, `scan-lag` and the incompressible pairing; the closing paragraph says that reads, scans and mixes A, B, D and F are closed, mixes C and E are decided by the memo and the decode, and past D9's window nothing is claimed |
| 3.10 | minor: per-read counters on `Shared` | 4.6, 6.1, 6.4 | **Accepted** (see 1.4) |
| 3.11 | minor: stage 4 is 1,200 lines on provisional constants; no write-cost input | 8 | **Accepted in part** (above). Stage 4 is split: 4a (S1, the tick and build-when-wanted, no hot-path code), 4b (the census), 4c (E2, E3, the trial, the back-off, the price, the budget), each with a DONE WHEN. A mixed read-write workload has its verdict lines in 7.3 (A and F) and 7.6 item 3 |
| 3.12 | minor: ten arms for fifteen suite sites each, two with no verdict item; `ZRegime` collides with the upkeep's regime | 6.1, 6.2, naming | **Accepted in part** (above). `supdb-ingestzpages` and `supdb-zpages` are dropped (their jobs are tests); the `*nozpages` arms arrive at the flip; the module and type are `src/db/zselect.rs` and `ZSelect` |
| 3.13 | minor: the machine-time budget does not follow from the cells; no DONE WHEN per stage | 8 | **Accepted.** Restated from the cells: about 30 hours with the second sittings 7.8 asks for (23 with one sitting of the three-arm and ooc rows), 3 hours of it before the sidecar's read path exists; every stage has a DONE WHEN |
| 3.14 | minor: six factual slips | 1.3, 1.4, 7, 7.1, 7.2a, 7.6, 7.8 | **Accepted in full.** (1) the probe's codec is the engine's, `t_z` 4.0-4.7; (2) 7.1's "before any code or probe run" is corrected, since M13 and M14 preceded it; (3) 35.7, stop under 36; (4) one threshold a quantity: 0.03 after the file is resident, 0.10 for the first cell, 0.05 the refutation line at three threads, bytes a read 120 and 410; (5) the tombstone rule is dropped; (6) the passage is `CLAUDE.md:526-543` |

### What the reviews said was missing

| review | what was named | where it is now |
|---|---|---|
| 1 | the nocache merge scans over a covered partition; per-handle counters; a Bloom fingerprint; `Auto` beside the inline and unpinned options; `retired_z`'s sweeps, the orphaned finished build, the declined flag; bounded rebuild; the rewrite rate against the rebuild rate; a scan evidence path and `Auto` off Linux; the tombstone rule | 3.9-3.10; 6.4; 2.5; 4.1 and 6.1; 4.8, 5.3, 6.1; 2.7; 5.6; 4.3 and 2.9; dropped |
| 2 | a scan census; a predicted-benefit gate and a "z is not helping" exit; a trial that waits for the refill; a ski-rental entry; the readers' rate during a build; a memo admission policy or byte cap; a named key compare; a wasted-build budget; pinning `Off` for the inline and unpinned arms. Not found: an experiment above 79% occupancy, a key shape other than sequential decimal, a controller trace, a variable-length key through the z scan path | 4.3; 2.6.1 and 4.5; 4.7; 4.3-4.4; 7.6 item 6; 3.7; 3.2; 5.2; not needed (1.3). Not found: 7.2b; 7.1; 1d and 7.4; 6.3 |
| 3 | a front summary; ranges for v2's axes, 1b, the lag points, the incompressible pairing, the switch transient; a fit-window experiment and a "not paying" exit; fault attribution and `mincore`; a platform plan; a DONE WHEN per stage; durable copies of the evidence; a write-cost input; a compressing RocksDB arm; the definition of the pre-registration form | Summary; 7.2b, 9.2, 7.3; 7.2b and 4.5; 7.2b; 2.9; 8; 0a; 5.2 and 7.3 (no rate term: above); deferred (R11); not in the repository (Appendix A) |

### Open questions

These are the ones this document cannot answer and does not pretend to. The first is the one the request is most about.

1. **Past about seven times the cache, what would let a form that decodes match LMDB's reads?** (D9, 7.3's closing paragraph, R26.)
   Both take one fault a read, so the gap is the decode: about three microseconds on a read of forty-five. The z form ties LMDB near
   50M keys and trails by a tenth at 90M, while it stays 1.75x ahead of the raw form. Candidates, none priced: a decode that touches
   a fraction of the page (restart points inside a group, a format change that gives up some ratio); a codec with cheaper random
   access; a memo that holds more of a skewed head (3.7), which saves the decode and not the fault. A smaller fault is not on the
   list: a fault is a page. The document claims "everywhere but disk" inside the window only.
2. **How far from the cache's edge is 79% occupancy?** (7.2b, stage 1a.) The probe has run one point.
3. **What is the 0.07 faults a read that removing the hash saves beyond its footprint?** (7.2a point 1; 1a's `mincore` and faults by
   file.) If it is eviction, holding the hash resident is a lever worth up to that (9.5).
4. **Does the engine's own record image pack 57 to a page on the suite's shape, and 40-46 on UUID text?** (7.1; 0b.) The probe packed
   60.0 fixed 116-byte records.
5. **Do a fault that waits on another reader's I/O and a `posix_fadvise(DONTNEED)` over pages another mapping holds behave as 4.3 and
   5.8 assume?** Both are from knowledge of the kernel, not run in this tree; 1a tests the first with a `WILLNEED` in flight and
   `zresident` the second.
6. **Can the default policy, with its forms, run under the bound at all?** (R17, 7.8(b), `forms-lead.md`.) Until it can, `Auto` serves
   no policy that builds forms and the flip of the default stays gated.
7. **What does a write-heavy store pay for the exposure window and for rebuild churn?** (5.6, R8.) The registered mix cells never
   seal; the lag sweep's bursts price it; a mix soak needs a runner knob that does not exist; stage 5 builds inside the merge.
8. **What is the z form on a host whose page is not 4 KiB?** (2.9, R24.) `Always` is correct there and is not one fault a read; a
   16 KiB `ZPAGE` is not designed.
9. **Is an LZ4-compressing RocksDB beaten out of core?** (R11.) The claim in 7.6 is against LMDB; the compressing arm is unscheduled.
10. **Do scans under heavy writes fit in the bound?** (R5.) The memtable chain can take about 400 MB.
11. **Do handles on a thread pool ever produce evidence?** (4.3.) A window opened by one thread and closed by another is dropped.

### What was recomputed in this pass

In a few lines of Python each, no timing: D9's table from 10M to 200M keys (Che's approximation over the hash and the record
populations, `phi_z = 1 - (B - f)/W_z`, LMDB's single population), D10's thresholds (35.74 records a page for parity, 40.10 for 1.5x,
39.3 and 43.1 for the scan claim), the split of the cell's faults between the two populations (the hash 0.003 and the records with
the directory 0.425 at 937 MB; at S1's `B` of 870 MB the raw form reads 0.482 and the z form 0), the sixty-times point of 2.2 (1.94 and 0.99 faults a read,
81 and 45 us), and the shrink arithmetic of 1.6. Nothing under `/home/user/supdb` was modified, and no build, test or timing run was
started.
