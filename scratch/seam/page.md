# E9 ("page"): whole-page chunks under a cipher, out of core: conclusion

5M keys under 640 MiB, uniform point reads, tree 3df85d22, nothing committed. Arms from a scratch patch (`work/page/page.patch`) and its one-constant control (`page-4080.patch`); commands, raw tables and deviations are in `work/page/notes.md`.

Where two reviewers corrected the notes, I re-read the raw outputs and use the corrected figure: the notes' H1 per-reading pairing is swapped (in a self-pair process 1 is column B), ×clear is against the pooled clear median of 149k (each sitting's clear arm moved 135-155k), and the fits were recomputed.

## 1. Is the out-of-core miss rate set by the plaintext held, and nothing else? (H1)

**Yes, to within about 10%. The pager is slightly better per MiB than the kernel, not worse.** The "uncounted factor of two" in once.md does not exist.

The store's size is measured: 682.7 MiB of partitions. The 38.1 MiB of ordered indexes are not resident in any read pass.

| store (source) | C, MiB cached | per read |
|---|---|---|
| clear, 328 MiB bound (1 sitting, C by `mincore`) | 271 / 278 | 0.763 / 0.748 faults |
| clear, 473 MiB (2 sittings) | 413-417 | 0.468-0.476 faults |
| clear, 575 MiB (2 sittings) | 517-527 | 0.260-0.277 faults |
| clear, 640 MiB (every H2 sitting) | ~580, from the line | 0.138-0.158 faults |
| cipher, both layouts (C is `held`) | 273-274 | 0.683-0.685 misses |
| cipher, whole pages (x432, xdyn85, x480, xdyn94, x520, x550) | 423 / 440 / 470 / 487 / 509 / 541 | 0.430 / 0.398 / 0.343 / 0.314 / 0.274 / 0.221 misses |

- **Two straight lines.** Least squares of m = a(1 − C/P): clear 1.29(1 − C/658), residuals ±1.4%; cipher 1.16(1 − C/669), ±2%. The 4080 and whole-page points share the cipher line: misses are a function of plaintext held, whatever the layout.
- **The lines are distinct.** A joint fit leaves ±4-6% systematic residuals. At equal C the clear store takes about 10% more faults at C 273 and 5-8% more at 425-510. H1's registered band (15% or 0.04) is met, on the pager's side.
- The notes' "convex" came from fixing P at the file size; with P free each family is linear, and the derived 660 matched the intercepts, not the file.

On uniform reads, page-granular eviction has nothing to win over the 1 MiB clock. Skew was not resolved.

## 2. Do whole pages let it hold what the clear store holds, and how close did the best survivor get? (H2)

**The mechanism holds; holding what the clear store holds does not survive a load.** Shmem/held is 0.988-1.009 in every whole-page arm against 1.228-1.229 for the 4080 control, one constant apart. Every fill reads its tag from the tag region; a unit test flips a tag byte and its chunk fails.

Read pass, two sittings each; ×clear against the pooled 149k.

| arm | budget (MiB) | held | shmem/held | misses/read | major faults/read | reads/s | ×clear | load,read |
|---|---|---|---|---|---|---|---|---|
| clear (supdb-nozpages) | page cache | ~580 | – | – | 0.138-0.158 | 135-155k | 1 | survives |
| 4080 control, default | 280 | 274 | 1.23 | 0.684 | 0.43-0.44 | 43.2-44.2k | 0.29-0.30 | (today's code) |
| whole pages, default | 280 | 273 | 1.00 | 0.685 | 0.36 | 53.2-53.3k | 0.36 | not run |
| x432 (fixed) | 432 | 423-426 | 1.00 | 0.430 | 0.40 | 52.3-52.8k | 0.35 | not run |
| **xdyn85** (85% dynamic) | 450 | 440-441 | 1.00 | 0.398 | 0.38 | 54.9-55.5k | **0.37** | **2 of 2** |
| x480 (fixed) | 480 | 469-470 | 1.00 | 0.343 | 0.335 | 61.9-63.9k | 0.42-0.43 | not run |
| xdyn94 (94%) | 498 | 485-490 | 1.00 | 0.314 | 0.31 | 62.5-66.2k | 0.42-0.44 | **killed 1 of 2** |
| x520 (fixed) | 520 | 508-510 | 1.00 | 0.274 | 0.27 | 72.9-74.9k | 0.49-0.50 | not run |
| x550 (fixed) | 550 | 540-542 | 1.00 | 0.221 | 0.22 | 82.7-87.6k | 0.56-0.59 | not run |

- **Layout alone, same budget and misses: 1.21-1.23x** (about 1.15-1.29x allowing for drift across pairings). It is fewer device faults, not fewer misses: the 60-75 MiB the straddle wasted went to the kernel's ciphertext cache (309 MiB against 233-244), which served more misses (0.36 major faults against 0.43-0.44).
- **The best survivor of load and read is xdyn85, at 0.37-0.38x** (today 0.28-0.30x). No arm with a shown margin under load reaches the plan's 0.55x: the verdict is "between". The fixed arms are read-pass results only; 520-550 MiB of unreclaimable shmem cannot sit beside the shuffled load's 150-240 MiB of anon in 640.
- **xdyn85's other passes:** ordered load 0.57-0.77x, shuffled 0.67-0.69x (closing sync 11.2-13.4 s against 4.9-5.4), device bytes per byte 2.50 against 1.34 as today, scan 0.58-0.63x (1.53-1.57 major faults a scan against 0.92-0.98).

**What the remaining gap is made of (xdyn85 against clear: about 18.1 against 6.7 µs a read).**
- **Device faults, about 8 of the 11.4 µs:** 0.23 extra faults a read at about 36 µs, from a regression over the whole-page sittings (µs/read ≈ 2.6 + 36 × major faults + 4.5 × misses, within 4%). Derived, not profiled. They come from holding 440 MiB of plaintext where the clear store caches about 580.
- **The rest of the bound in the read pass** (sampler): 141 MiB of ciphertext page cache, 38 of anon, about 10 of headroom. The ciphertext saves 0.02 faults a read (0.40 misses, 0.38 faults); the same MiB as plaintext would save about 0.24 by the line (a guess). But it is the reclaimable room the shuffled load's anon takes back, as it takes the clear store's cache. Shmem does not give way, so the gap is structural: the cipher store sizes unreclaimable plaintext for the worst phase, or shrinks faster than anon grows.
- **Non-device time, about 1.8 µs a read** (4.5 µs a miss): derived, and a ceiling, since misses served from cached ciphertext are mixed in. **About 0.7 µs** of base difference is unaccounted (a guess).

## 3. What it means for decision 3 and for the design

**Decision 3: (b)**, the budget the page layer owns, recomputed, counted in resident pages, with the caller's value as a cap. (a) is today's 0.29x and leaves 140-300 MiB of the bound to ciphertext that saves little. (c) was not tested; its case is the 141 MiB above, but that room is what keeps the store alive in a load, so (c) pays only if the budget can take it safely, which is the open margin question.

**What the budget has to subtract.** The prototype subtracted anon only (`read_budget`, every 100 ms). It must also subtract:
- files being written, which cannot be given back (`could_give` subtracts them from the wait lines, not from the budget);
- the other cipher stores in the process: each prototype store took its share of the whole group, so two at 94% is a kill; the share belongs in `layer::Memory`;
- a margin sized by how fast anon grows within one recompute interval.

It must not subtract the ciphertext page cache, which is reclaimable and is where the room comes from.

**Where the wait lines go.** In the prototype a caller's read waits at 1.5 × the limit and the store's own work at the limit + max(limit/16, 4 MiB). From a limit of 427 MiB up in a 640 MiB group, the caller's line is past the OOM point, so "breaths 0" at the large budgets proves nothing. Both lines have to be drawn against the group's limit less anon and less what cannot be given back, with a reader waiting below the group limit.

**The margin.** 85% of `read_budget` survived load and read in 2 of 2 sittings; 94% was killed in 1 of 2 (shmem 363 + anon 195 MiB, 10 s into the shuffled load, the limit following anon down to 336-422 and not fast enough). Peaks do not separate them: shmem + anon reached 603 MiB in the 94% sitting that survived, 579 in the one killed, 559-562 at 85%. **No safe margin is established, at n = 2**; survival is set by anon's growth rate against the evictor's cadence, which was not measured. Reaching 0.55x needs about 540 MiB held in reads (x550), so a budget that drops to about 360-440 MiB in a shuffled load and comes back; that has not been shown safe.

**Design.md, confirmed:**
- `CHUNK` = 4096 with the tags apart; held is then resident pages, which removes once.md's "75% of held is 94% of shmem" accident.
- The copy from a kept mapping: major faults ≤ misses + 0.01 in every arm, so the fault counters keep meaning device reads.
- The pager's protocol and 1 MiB unit as they are.

**Changed:**
- §12 decision 3 gains option (c) and the recommendation (b) with the subtractions and lines above; "(a) until S7" stands only as today's default until the layer exists.
- Once.md's next effort, page-granular eviction, is dropped for uniform reads.
- §8's out-of-core prediction is restated from the cipher line 1.16(1 − held/669 MiB), about 36 µs a device fault and about 4.5 µs a miss.
- Residency and advice must cover tag pages: the prototype's `resident`, `willneed` and `forget` cover data only (moot at 5M, where tags are 2.7 MiB and hot), and the design's tag tier (pinned to B/16, charged to the budget) has to sit inside them.

**Still a guess:** the margin, the recompute cadence, the line positions, whether (c) pays, and the design's sealed per-group tag pages and 1 MiB-aligned layout (the prototype put flat tags after the data).

## 4. Not verified

- A safe margin for the dynamic budget (n = 2; peaks do not separate 85% from 94%).
- x480, x520 and x550 under any load.
- A second sitting at the 328 MiB bound (my tool's timeout killed it).
- The clear store's C at 640 MiB by `mincore` (about 580 is from the line).
- The gap's split into device, miss and base terms (a regression, not a profile).
- Layout-alone 1.21-1.23x across two non-interleaved pairings; binaries not rebuilt to confirm the one-constant difference.
- Skew (ycsb-C twice): 1-3 s windows, budget and layout confounded; no conclusion stands.
- Option (c), holding no ciphertext.
- Two cipher stores in one process, or a caller whose own memory grows.
- The design's tag tier, sealed tag pages and aligned layout.
- Tag pages in the residency and advice paths (scans, the controller, stores far larger than memory).
- That the tree was clean during each pairing (`*-head.txt` records the end).
- Threaded reads, a reopened store that fits, stores past 5M keys.
