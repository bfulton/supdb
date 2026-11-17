# Holding memory once under a cipher, out of core: conclusion

## The answer

No, holding memory once does not close the gap. In a 5M-key store under 640 MiB, the only once-arm that survived both a load and a read was the dynamic one. It read point reads at 0.297x-0.332x of the clear store across four sittings, which is 1.08x-1.16x of today's cipher store (0.281x and 0.283x). That meets the plan's kill line in every sitting. No arm reached 0.6x.

Most of the gain is the larger plaintext budget, not the mechanism. Today's code given the same budget gets about the same rate.

The risk is real. The plaintext lives in shmem (the pager's memory files), which cannot be reclaimed without swap. A fixed budget sized for the read pass was OOM-killed in the shuffled load in both sittings.

## The arms

- **supdb-cipher:** today's store. Its plaintext budget is half of what the group has free at open, about 280 MiB.
- **bX (B):** today's code with `cache_bytes(X MiB)`. The ciphertext stays mapped, and the kernel reclaims it as shmem grows.
- **dX (D):** a miss is one `O_DIRECT` pread of the stored page into an aligned buffer, then a decrypt into the memfd. The ciphertext is never in the page cache.
- **ddyn:** D, with the budget recomputed every 100 ms as 75% of the group's limit, less anon and a margin. It came to about 397 MiB in the read pass and 300-314 MiB in the shuffled load.

The read pass is 5M uniform point reads. A "miss" is a plaintext chunk filled (`cipher_fills` / reads). The device column counts major faults for B and supdb-cipher, and O_DIRECT reads for D. Each sitting is one rep. The arm-against-cipher comparisons cross pairings.

**Sitting 1 (the cipher arm ran first)**

| arm | reads/s | clear reads/s | ×clear | misses a read | device reads a read |
|---|---|---|---|---|---|
| supdb-cipher | 40,723 | 144,763 | 0.281 | 0.684 | 0.438 |
| b384 | 42,268 | 142,816 | 0.296 | 0.513 | 0.490 |
| b432 | 49,788 | 145,217 | 0.343 | 0.428 | 0.422 |
| d320 | 36,238 | 118,624 | 0.305 | 0.622 | 0.622 |
| d384 | 43,307 | 137,628 | 0.315 | 0.511 | 0.511 |
| d432 (killed in load,read) | 52,509 | 151,746 | 0.346 | 0.427 | 0.427 |
| ddyn, read only | 46,657 | 153,304 | 0.304 | 0.490 | 0.490 |
| ddyn, load,read | 47,317 | 143,160 | 0.331 | – | – |

**Sitting 2 (the clear arm ran first)**

| arm | reads/s | clear reads/s | ×clear | misses a read | device reads a read |
|---|---|---|---|---|---|
| supdb-cipher | 42,254 | 149,260 | 0.283 | 0.684 | 0.432 |
| b384 | 39,293 | 141,755 | 0.277 | 0.514 | 0.492 |
| b432 | 48,251 | 145,873 | 0.331 | 0.428 | 0.423 |
| d320 | 37,088 | 154,375 | 0.240 | 0.619 | 0.619 |
| d384 | 44,446 | 134,353 | 0.331 | 0.511 | 0.511 |
| d432 (killed in load,read) | 52,241 | 136,775 | 0.382 | 0.428 | 0.428 |
| ddyn, read only | 45,656 | 137,496 | 0.332 | 0.490 | 0.490 |
| ddyn, load,read | 47,141 | 158,821 | 0.297 | – | – |

The clear store took 0.137-0.170 faults a read. Its own rate moved ±15% between sittings, so compare the cipher arms by absolute rate: those were stable to a few percent.

**What D adds over B at the same budget:** 1.055x and 1.083x at 432 MiB, and 1.025x and 1.131x at 384. That is 1.0-1.6 µs a read at 432, which one rep a sitting does not resolve. The mechanism did fire:
- D's direct reads equal its fills exactly;
- the reader took no major faults;
- the page cache that is not shmem fell to 39-47 MiB at 432;
- a probe on the TMPDIR filesystem refused an unaligned O_DIRECT read.

## What is left of the gap (d432, read pass)

1. **Device reads.** 0.43 a read against the clear store's 0.14-0.17. At about 36 µs a pread, that is roughly 10 µs a read, most of the gap. Holding once did not move it: misses follow the uncached fraction of the plaintext, about 1.12 × (1 − X/721 MiB). That is a three-point fit, and the 721 MiB is derived, not measured.
2. **The straddle.** A 4080-byte chunk starts 16·c bytes short of a page, so an isolated chunk takes two shmem pages. Shmem was 1.22-1.27x of `held` (the plaintext the pager counts) at every budget. At 432, 516 MiB of shmem held 417-422 MiB of plaintext, so 97-116 MiB bought nothing. The ratio did not fall as the budget grew, so the straddle is consistent with the counts but not proven to be the cause.
3. **Headroom and leftover page cache, 60-70 MiB.** Under D the group sat 20-25 MiB under its limit, and 39-47 MiB of non-shmem page cache remained. What that cache holds is not identified.
4. **Cache efficiency beyond size.** Even if all 516 MiB of shmem were plaintext, the miss model gives about 0.32 misses a read, roughly twice the clear store's faults. This factor is not counted. The candidates are FIFO eviction by 1 MiB unit against the kernel's LRU, which keeps the hash-slot pages, and records split across chunks.
5. **Non-device time.** On the harness's common clock it is 9.4-9.8 µs a read for supdb-cipher, 6.8-7.2 for b432, 4.3-5.4 for d432, and about 2-4 for clear. So the budget took off about 2.6 µs and D another 1.4-2.9. This is derived from rates, not from a profile.

## What it means for the design

- **Who owns the budget.** The page layer should own it, behind `layer::Memory`, and recompute it as the process runs, never read once at open. A fixed budget died as soon as the shuffled load's anon reached 143-184 MiB. The dynamic arm survived, but:
  - Its margin was an accident: 75% of `held` is about 94% in shmem pages. The budget has to be counted in resident pages, not chunk bytes.
  - It is per store. Two cipher stores in one process would each take 75%.
  - It does not subtract files being written, which cannot be given back.
  - It was never run against a caller whose own memory grows.
- **How the ciphertext is read.** O_DIRECT pread works and is a little cheaper. But it makes misses invisible to `ru_majflt`, the census, the sampler's price of a fault and the compact form's controller. Design §4 chose to copy from a kept mapping to keep those counters, with pread allowed if it beats the mapping "by more than its noise". Here it did not: 5-8% at one rep a sitting. Keep the mapping copy. If pread is ever chosen, the layer must:
  - count the pager's own fills for the census;
  - record whether the O_DIRECT open fell back to a buffered one, which the prototype did silently.
- **At the limit.** Nothing under shmem is reclaimable, so the layer needs a line below the group limit where readers wait. Today a caller's read waits only at 1.5x the budget, which at these budgets is past the OOM point. Read-ahead must stay inside the budget: the prototype's 32-chunk lookahead under a `Hold` overshot a 2 MiB budget to a 14 MB peak. It also must not poison the store from a damaged chunk nobody asked for, which the prototype did.
- **A store that fits.** The cost is not measured. In core no chunk was filled in either arm, because `ab` adopts the files it wrote. Under D, a first touch after a reopen goes to the device even when the ciphertext is cached. The RWF_NOWAIT variant was built but not timed. The design's write-through covers new segments, but not a reopened store.
- **Sequential reads.** d432 scanned at 0.86x-1.02x of clear and 2.27x-2.54x of supdb-cipher. That confounds one pread per run with the larger budget, since no B432 scan was run. With merges reading 32-chunk runs, ddyn's shuffled load closed in 8.0-8.5 s, against E7's 13.8-14.9 s; that comparison crosses sessions and is not attributed.

## Decision 3: the plaintext budget

The options:
- **(a)** The caller's `cache_bytes`, with today's default of half the free budget read at open.
- **(b)** A cgroup-derived budget, recomputed continuously, owned by the layer through `Memory`, counted in resident pages, with the caller's value as a cap.
- **(c)** As (b), plus holding once (no ciphertext cache, via O_DIRECT).

**Recommendation: (b), not (c).**
- Keep today's conservative default until the layer exists. It survived. A larger fixed value, from a caller or as a default, is what got killed.
- At the layer stage, make the budget the layer's and recompute it.
- Spend the cipher's out-of-core effort on cache efficiency per MiB: whole-page chunks, which the design's 4096-byte chunk already gives, and page-granular eviction. Holding once is not where the effort should go.

**What would change it:**
- A paired D-against-B run in one process with four or more reps showing D at 1.1x or better would make (c) worth its loss of the fault counters.
- A prototype with whole-page chunks and page eviction reaching 0.6x of clear would show the remaining gap is the cache's, and settle the order of work.
- If a reopened store that fits loses under D, that rules out O_DIRECT without the RWF_NOWAIT probe.

## Not verified

- Whether D beats B: 2-13% at one rep a sitting, across pairings.
- What the 39-47 MiB of non-shmem page cache under D holds.
- That the straddle causes the 1.22-1.27x shmem ratio.
- The cause of the uncounted factor-of-two miss excess (eviction policy or chunk layout).
- The 721 MiB plaintext size and the miss model; both are derived.
- The split of non-device time (R11, the profile, was not run). What `read_us_per_io` measures for D, where it read 31.9-34.7 with no faults.
- D's cost to a reopened store that fits (R9 filled nothing, R10 skipped).
- The scan gain's split between budget and run preads.
- The cause of ddyn's faster shuffled load.
- Safety of the dynamic budget with two stores, or with a caller whose memory grows. Its `swamp` line lies past the group limit.
- A plain-mode cipher test (`the_builders_rest_at_every_block_of_a_store_of_wide_values`) failed once in four runs on the patched tree. Whether it predates the patch is not checked, and the patch did change once=0 code.
- The "usage under 625 MiB" selection criterion: the medians were 614-621 MiB, but every D arm's maximum was 636-640 MiB.
