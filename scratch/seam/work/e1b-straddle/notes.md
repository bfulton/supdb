# E1b: is the 0.73x of the compact form under a cipher the straddle?

Label: e1b-straddle. Tree 3df85d22 (bench binary built 2026-11-16 06:27, newer than the
last commit touching src/ or bench/src, 2026-11-15 22:51).

## 1. What the existing outputs carry (pairs3/, run at b4d555bc)

Files: scratchpad/pairs3/supdb-cipher-vs-supdb-ciphernozpages-5000000-{1,2}.{out,err}
(two sittings, one in each order, `--reps 2 --ops-div 10`, 500k reads, medians of 2) and
long-1.{out,err} (1 sitting, `--reps 1 --ops-div 1`, 5M reads). At b4d555bc `supdb-cipher`
had the controller on (today `supdb-cipherauto`), `supdb-ciphernozpages` was raw (today
`supdb-cipher`).

Counters present:
- `count cipher_fills` = `PagerCounts::fills`, chunks decrypted (pager.rs:252-253, bumped at
  pager.rs:1094 on EMPTY->BUSY->fill). Read once at the end of the pass's process
  (bench/src/run.rs:2611, engines.rs:8542): it is the PROCESS total, load + read pass,
  not the read pass's own. No line prints it around the read phase (read_phase,
  run.rs:2131-2156, has no counters line; the `cipher` stderr line at run.rs:574-588 is
  printed only for the lag sweep's and mixes' windows).
- `count zpage_reads` (form-page reads; every one asks the pager for one whole 4096-byte
  page: `open_group` -> `page_bytes` -> `slice_at(p*4096, 4096)`, zfile.rs:2141-2148,
  2184-2190, before any memo look).
- `read majfaults_per_op`: the reading thread's own major faults over the read phase
  (run.rs:1882-1925).
- No counter of page-level pager misses (an ask that found a chunk not HOT). PagerCounts
  (pager.rs:246-277) counts chunks only. `Paged::slow` is entered per non-HOT chunk, COLD
  included (pager.rs:1027-1105), so even its entries are not fills.

So the outputs do not settle the question as posed: page-level misses are not counted,
and the fills are not the read pass's alone. What they do give, by differencing the
500k-read and 5M-read runs at b4d555bc: auto (4.628M - ~0.596M)/4.5M = 0.896 fills a read,
raw (3.414M - 0.355M)/4.5M = 0.680; faults per fill 0.67 and 0.63. Section 3 re-runs the
pairing at today's head plus a load-only baseline to take the read pass's fills by difference;
section 4 counts the page-level misses with uprobes.

## 2. The arithmetic of the premise

Geometry (pager.rs:108-125): a chunk is 4080 bytes of plaintext at 4080c, stored as one
4096-byte page of the envelope (16-byte tag after it); a unit is 256 chunks = 255 pages
of 4096 = 1,044,480 bytes. A form page is 4096 bytes at plaintext 4096k (ZPAGE, zfile.rs:2147).

- Fraction of form pages across two chunks: 100%. Page j of a unit (j = 0..254) covers
  plaintext [4096j, 4096j+4096) of the unit and the chunk boundary 4080(j+1) = 4096j + (4080-16j)
  lies strictly inside it for every j < 255; three chunks would need two boundaries in 4096 bytes,
  which needs a start within 15 bytes before a boundary, and 4080c - 4096k = 16(255c - 256k) is a
  multiple of 16, so only 0 (page starts on the boundary, and then covers chunks c and c+1).
  So page j touches chunks j and j+1 of its unit, neighbours share a chunk, and no page crosses a
  unit (255 pages per 256 chunks): a page asked cold costs 2 chunks = 2 stored pages unless a
  neighbour page's read filled one of them first.
- Raw point read: one hash slot (8 B, 8-aligned; 4080 is a multiple of 8, so a slot never
  crosses a chunk) and one record. Record = 620,000,000 / 5,000,000 = 124 B (zpage_group_bytes),
  4-aligned (REC_ALIGN, flatindex.rs:63): it crosses a chunk boundary with probability
  ~ (124-4)/4080 = 2.9%. Slots: power of two >= 4/3 keys, 8 B each (flatindex.rs:57-62), about
  4 MiB a partition of ~263k keys, ~76 MiB over 19 partitions = ~19.5k chunks.
- Expected chunks a cold read touches: form 2.00; raw record 1.03 (slot ~0, resident).
- Expected chunk FILLS per read, by linearity, whatever the correlation between neighbouring
  chunks' residency: sum over the chunks the read asks of P(chunk absent).
  form today: 2 (1 - c_z), c_z = form chunks held / form chunks;
  form with whole-page chunks: 1 (1 - c_z') with c_z' = c_z (same plaintext bytes, same budget);
  raw: 1.03 (1 - c_r) + slot misses, c_r = record chunks held / record chunks.
  So the straddle doubles the form's fills a read exactly, and the design's layout halves them,
  independent of how residency is correlated. What the correlation decides is only how many
  asks those fills fall in (page-level misses, between fills/2 and fills).

## 3. The pairing again at today's head (runA) and a load-only baseline (runB)

Machine idle before each (ps). Bench binary fresh (see top).

runA: `cd bench && TMPDIR=$W/tmp OUT=$W/runA sh scripts/pair.sh SUPDB_ZPAGES_TRACE=1 supdb-cipherauto
supdb-cipher 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1` (07:02-07:11 UTC).
Outputs: runA/supdb-cipherauto-vs-supdb-cipher-5000000-{1,2}.{out,err}.

| quantity | auto s1 | raw s1 | raw s2 | auto s2 |
|---|---|---|---|---|
| reads_per_s | 35,151 | 41,602 | 42,342 | 33,477 |
| auto/raw | 0.845 | | | 0.791 |
| majfaults_per_op (reader thread) | 0.5223 | 0.4364 | 0.4471 | 0.5426 |
| cipher_fills (process) | 4,084,540 | 3,425,274 | 3,423,150 | 4,145,063 |
| zpage_reads / rawserves | 4,974,186 / 22,059 | | | 4,867,632 / 53,639 |
| form file bytes (pages) | 500,199,424 (119,743) | | | 506,834,944 (121,352) |
| zpage_builds / prebuilt | 19 / 13 | | | 19 / 14 |
| cipher_held_at_end, peak (MB) | 285.0, 334.3 | 292.1, 328.9 | 288.6, 328.9 | 287.9, 340.4 |
| cipher_evicted_units | 52,197 | 155,629 | 156,065 | 64,253 |
| p99_us | 135.2 | 102.4 | 99.8 | 138.2 |
| read_us_per_io (device) | 39.2 | 34.7 | 33.4 | 49.0 |
| bg_read_bytes_per_op | 37.4 | 0 | 0 | 105.2 |

Today the cell reads 0.79-0.85x (not 0.73x), faults 1.20-1.21x raw's (not 1.40x). The form files
are 500-507 MB here against 512-544 MB at b4d555bc (coding picks differ by run).

runB: same with `--reps 3 --ops-div 5000` (1000 reads, the floor of `Plan::divided`,
run.rs:312-318): the process's fills with the read pass removed. Median of 3 a sitting:
auto 105,120 and 113,211; raw 1,203 and 1,203. (The auto arm's load fills are the builds
ahead reading raw records; the raw arm's load leaves the seals' plaintext adopted, not
decrypted: cipher_held_at_end 281-292 MB after 1000 reads with 1.2k fills.)

Read pass's fills, total less load (load taken as 109k +- 4k for auto):
- auto: 3.975M and 4.036M over 5M reads -> 0.795 and 0.807 fills a read (this still holds the
  5-6 builds that ran in the read pass and the sampler's reads, on threads of their own).
- raw: 3.424M and 3.422M -> 0.685 and 0.684 fills a read.
- faults per fill (reader's majflt / pass fills): auto 0.657, 0.672; raw 0.637, 0.653.
  The kernel's cache of the envelopes serves about a third of the pager's fills in both arms.
- fills ratio auto/raw 1.16, 1.18; faults ratio 1.20, 1.21; time ratio 1.18, 1.26.

## 4. Page-level misses, counted with uprobes (no code change)

No counter says how many asks the fills fall in, so the pager's two ask entries and its fill
were counted with ftrace uprobes on the shipped bench binary (built from 3df85d22):
- `zask`: entry of `<Paged as Paging>::ensure` (vaddr 0x306780, file offset 0x305780),
  self=%di off=%si len=%dx;
- `zpiece`: entry of `<Paged as Paging>::piece_from` (vaddr 0x3068c0, offset 0x3058c0),
  self=%di off=%si (the by-the-piece path the raw record read takes, pager.rs:1345-1359);
- `zfill`: vaddr 0x3076ef inside `Paged::slow`, after a successful decrypt and before the chunk
  is stored COLD (`lock incq 0xf8(%rbp)` = `present.fetch_add` precedes it; pager.rs:1091-1095),
  self=%bp c=%bx.
`slow` is called only from `ensure` (0x306824 call, 0x306864 jmp) and `piece_from` (0x3068fd)
(objdump over the whole text), so every fill follows one of the two asks on its thread.
Probes filtered to threads named "bench" (the pass's reader), enabled only between each arm's
`phasemark ... read start/end` (SUPDB_LAG_MARK=1), ring 256 MiB a CPU, 0 overruns/drops.
Driver: probe.sh; reductions: asks.py (raw arm, reads grouped by their slot asks) and form.awk
(form arm, consecutive asks of one page merged into one read: 1.34 page asks a read, the second
ask of a page within a read never fills). tracefs was mounted for this and unmounted after;
probes removed, ring reset to 1410 KB.

Run: `probe.sh 2 main` = `SUPDB_LAG_MARK=1 bench ab --arms supdb-cipherauto,supdb-cipher --size
5000000 --bound 671088640 --passes read --reps 1 --ops-div 2` (2.5M reads an arm), one sitting.
The probes slow the reads to 9-11k/s (from 33-42k), so the probe run's rates are not read; its
form file was 534 MB (127,912 pages). Its own table: fills auto 2,307,422 / raw 1,778,709
(the raw arm's reader-attributed fills 1,778,218 match the counter), majfaults/read 0.6616 /
0.5403, zpage_reads 2,400,652 (= the 1,096,137 + 1,304,499 merged form reads below).

Form arm (reader thread, form-page reads), by half of the read window:

| half | reads | fills/read | misses/read | fills/miss | 0 filled | 1 filled | 2 filled |
|---|---|---|---|---|---|---|---|
| 1 | 1,096,137 | 0.903 | 0.550 | 1.643 | 493,753 | 215,278 | 387,106 |
| 2 (steady) | 1,304,499 | 0.872 | 0.527 | 1.653 | 616,465 | 238,729 | 449,305 |

One-chunk misses split evenly between the page's first and second chunk (118,780 / 119,949):
the other chunk was filled by the neighbouring page's read (the shared chunk).

Raw arm (reader thread), by half:

| half | reads | slot chunks/read | slot fills/read | record chunks/read | record fills/read | fills/read |
|---|---|---|---|---|---|---|
| 1 | 1,237,418 | 1.0013 | 0.041 | 1.0292 | 0.675 | 0.715 |
| 2 | 1,275,087 | 1.0013 | 0.030 | 1.0292 | 0.671 | 0.701 |

Record reads with any fill (half 2): 836,145 = 0.656 a read; fills per record miss 1.023.
The record straddle measured, 1.0292 chunks a record, is the 2.9% of section 2.

Faults per fill under the probes (reader majflt x 2.5M / reader fills): auto 0.750, raw 0.760;
unprobed (runA) 0.657-0.672 and 0.637-0.653. Equal between arms either way.

## 5. What this says

- Every form read asks two chunks (100% of pages straddle), a hit needs both, and the fills a
  read are 2(1-c): c = 0.564 from the steady half (0.872 fills), i.e. 73.8k of the form's
  130.9k chunks held, ~301 MB. The same held figure (73.6k chunks, ~300 MB, between the
  end-of-pass held 278-292 MB and the peaks 329-340 MB) reproduces runA's 0.80 fills a read for
  500-507 MB form files, and b4d555bc's 0.896 for 544 MB (2(1 - 73.6/133.4) = 0.896), and the
  raw arm's 0.68-0.70 (1.029 (1 - c_r) + 0.03 slot fills, c_r = 0.348). One model for all.
- Faults follow fills with one constant for both arms (a third of fills, a quarter under the
  probes, come from the kernel's cache of the ciphertext). So the fault ratio is the fill ratio:
  runA fills 1.16-1.18x, faults 1.20-1.21x; b4d555bc 1.32x and 1.41x.
- Time follows faults closely: wall per fault 55.1, 52.8 (raw) and 54.5, 55.0 us (auto) in runA,
  so read time is ~54 us x faults/read in all four arm-sittings.
- The straddle is the whole gap, but not as "2 stored pages per cold page": a cold form page
  costs 1.65 fills (a third of misses find the shared chunk already filled), and a page is a hit
  less often than a whole-page chunk would be (47.3% against c = 56.4%). The two effects
  multiply to exactly 2x in fills a read (0.527 x 1.653 = 0.871 against 1 - c = 0.436).
- Literal kill criterion ("faults about twice the form-page misses"): faults per form-page miss
  = 1.653 x 0.66 = 1.09 (unprobed constant), not 2, and per raw record miss 1.023 x 0.65 = 0.66.
  Form/raw per miss = 1.62. The criterion as worded fails, but for two reasons that leave the
  prediction standing: the kernel's ciphertext cache serves a third of fills in both arms
  equally, and a third of form misses share a chunk with a neighbour. The relevant quantity,
  fills a read, is exactly 2(1-c) today and (1-c) under whole-page chunks, independent of the
  correlation between neighbouring chunks (linearity of expectation over the chunks a read asks).

Prediction for the design's layout, same budget and bound, raw unchanged (a 124-B record
crosses a 4096-B page as often as a 4080-B chunk, 2.9%; slots 97% resident):
- form fills a read 1 - 73.3k/pages: 0.40 (500 MB form file), 0.41 (507), 0.44 (534), 0.45 (544);
- form faults a read x 0.64-0.67: 0.26-0.30, against raw 0.44-0.45; for runA's own files
  (500-507 MB) 0.263-0.276, 0.59-0.63 of raw's;
- if time keeps following faults at ~54 us each (it did in runA within 4%): form 1.6-1.7x raw
  for runA's files. With an intercept of 2-3 us a read (decode, fill CPU) and the rest fitted
  to raw, 1.5-1.6x; over the whole 500-544 MB range of file sizes, 1.4-1.7x.
  The design's "about 1.4x" is at or under this; E1 (forced, in the clear) is still owed.

Against the design (section 8):
- "today 0.73x": at 3df85d22 the same pairing reads 0.79x and 0.85x (faults 1.21x, 1.20x). The
  ratio moves with the form file's size, which moves with the coding the sampler picks (500-544 MB
  across five runs), because c = held/form chunks.
- "A cold form page today costs two stored pages": measured 1.65 on average, and hits are rarer
  than with whole pages; net 2.0x in fills a read, which is what the prediction needs.
- "time follows faults (1/0.73 = 1.37 against 1.40)": holds; runA 1.18/1.26 time against 1.20/1.21
  faults.
- "the hash slots stay resident": 0.030-0.041 slot fills a read (97% resident).

Caveats: the page-level counts come from one probed sitting at 2.5M reads (half 2 as steady
state); probes change timing (faults per fill rose from ~0.66 to ~0.75 in both arms) but the
counts depend on the order of reads and the cache's content. The load baseline is a median of
3 in separate processes (spread not printed by bench ab; 105k and 113k between sittings). The
auto arm's builds that ran during the read pass (5-6 of 19) and the sampler add fills on other
threads to cipher_fills; the probe counts exclude them (reader thread only). Not verified: the
controller's buy timing in the probed run (65k rawserves); whether the kernel cache's share of
fills stays a third under whole-page chunks (assumed, since it was equal across arms with
different ask shapes).
