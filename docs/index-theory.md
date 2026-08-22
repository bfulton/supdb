# The index decision, in theoretical context

`indexlab` measured nine index layouts. This is what the theory says about the
result — including the places the theory predicts something the measurement
does not show, which are the interesting places.

All figures are 10M sixteen-byte decimal keys, `--profile full`, from
`results/f9-index-layout.full.json`.

| layout | hit | miss | scan | B/key | MiB @ 10M |
|---|---|---|---|---|---|
| heap-hash (today) | 369 ns | 246 ns | 2.78 ns/e | 98.8 | 942 |
| hash+flat | 475 ns | 251 ns | 6.80 ns/e | 54.2 | 517 |
| hash+paged | 686 ns | 259 ns | 4.92 ns/e | 43.2 | 412 |
| mph+paged | 801 ns | 667 ns | 4.29 ns/e | 22.2 | 212 |
| mph+bloom+paged | 861 ns | **239 ns** | 4.85 ns/e | 24.9 | 237 |
| packed | 1088 ns | 1121 ns | 8.36 ns/e | **14.1** | 134 |
| btree | 1417 ns | 1476 ns | — | 36.8 | 351 |

## 1. Which cost model applies

The external-memory model (Aggarwal & Vitter, 1988) parameterises by block
size `B` and memory `M`, and gives Ω(log_B N) I/Os as the comparison-based
search lower bound. B-trees match it, which is why they are the reflex choice.

The reflex is wrong here because the parameters are wrong. Supdb's index is
designed to be resident, so the transfer unit is not a 4 KiB disk block but a
64-byte cache line, and `M` is L2/L3 rather than RAM. The same mathematics
applies at a different scale, and at that scale a 4 KiB B-tree node is not a
transfer unit at all — it is 64 cache lines, of which a binary search touches
about six.

That reframing predicts the measured ordering: with B = 64 bytes the B-tree's
fanout advantage largely evaporates, and what remains is its per-node overhead
— headers, a slot directory, separator keys duplicated at every level, and a
fill factor below 100% in any tree that has been mutated. The measurement
agrees: a plain packed array is both **faster and smaller** than the tree.

## 2. Space: how close to the floor?

For a set of `N` keys drawn from a universe of size `U`, the information-
theoretic floor is `log₂ C(U, N)` bits, which for N = 10⁷ sixteen-digit decimal
keys is **3.92 bytes/key**. Extents add perhaps 5–6 bytes varint-packed, so the
floor for this index is roughly **9–10 bytes/key**.

- `packed` at 14.1 B/key is within about **1.5× of the floor**.
- `heap-hash` at 98.8 B/key is about **10× the floor**.

That is the one part of the original architecture review that survives
measurement intact: the current layout is nowhere near any frontier on space,
and the gap is not buying anything structural — it is allocator overhead, `Vec`
headers, and one heap allocation per key.

## 3. Why the B+tree lost, in one sentence

It pays for generality Supdb never uses. A B+tree supports arbitrary
interleaved point updates; Supdb's index is only ever modified in sorted
batches at checkpoint, because `seal_shard` already sorts. Applying a sorted
batch of K by merge is O((K+N)/B) sequential; applying it as K tree inserts is
O(K log_B N) random.

Brodal & Fagerberg (2003) make this precise: there is a proven search/insert
tradeoff curve, and the plain B-tree sits at its most update-hostile endpoint
(ε = 1 in the B^ε family). For a store whose stated purpose is ingest, that is
the wrong corner — and the corner is not compensated by a search advantage the
cache-scale model does not grant it.

## 4. Why the minimal perfect hash disappointed

Fredman, Komlós & Szemerédi established that a minimal perfect hash needs at
least ~1.44 bits/key; RecSplit gets to ~1.56. So MPH is provably near-optimal
**on space**, and the measurement reflects that: 22.2 B/key, the smallest of
any layout retaining near-hash point access.

But the FKS bound says nothing about **evaluation cost**, and that is what bit.
BBHash evaluates by probing successive level bit-arrays, each a random access
into megabytes, plus a rank index — three or more dependent misses where a
hash table has one. 801 ns against heap-hash's 369.

**This is a property of BBHash, not of minimal perfect hashing.** PTHash and
RecSplit evaluate in a single probe plus a small lookup table. The result here
should be read as "BBHash is the wrong MPH for a latency-sensitive path", not
as "MPH cannot work". Re-testing with PTHash is the obvious follow-up and is
not done.

## 5. Why the filter won its category

Bloom's bound: for false-positive rate ε, a Bloom filter needs
`log₂(1/ε)/ln 2` bits/key — 12 bits/key buys about 0.3%. The information-
theoretic floor for an approximate membership structure is `log₂(1/ε)`, i.e.
8.4 bits at that rate, and ribbon filters (Dillinger & Walzer, 2021) come
within a few percent of it.

So a ribbon filter would be ~30% smaller than the blocked Bloom used here, at
the same FPR. It was not used, deliberately: the category being bought is
**miss latency**, and on that axis both are a single cache line. Ribbon's
advantage is space, and space was not the binding constraint for a structure
costing 2.7 B/key.

The filter matters here for a structural reason worth stating: a minimal
perfect hash returns a slot for keys it never saw, so it has **no cheap way to
fail**. Every miss otherwise pays a full record read to discover it was a miss
— 667 ns, which the filter cuts to 239. A hash table needs no such help; it
short-circuits on an empty slot. The filter is a fix for MPH's specific
weakness, not a universal accelerator.

## 6. The objective function was wrong

The sharpest theoretical point is not about data structures at all.

In the engine's read path, the index lookup is roughly a fifth of a point read
(≈356 ns of ≈1.67 µs at 10M keys). So the 1.29× index regression that
`hash+flat` costs is about **+5% end to end**.

Meanwhile the same structure is **100% of reader open cost** (1.45 s at 10M
keys) and **100% of reader memory** (942 MiB, duplicated per process rather
than shared).

By Amdahl's argument the term worth optimising is not the one being measured
in the lookup column. The whole comparison above ranks layouts by a quantity
that contributes a fifth of one operation, while the decision actually turns on
two costs where the index is the entire term. That is why `hash+flat` losing
1.29× on lookups is a good trade and `packed` losing 2.9× probably is not: the
question is how much of a fifth you are willing to spend to delete a 1.45-
second open and 7.9 GiB of duplication across eight readers.

## 7. What is provable, and what is chosen

Provable:

- `packed` is within ~1.5× of the information-theoretic space floor.
- A plain B-tree is a non-dominating endpoint of the Brodal–Fagerberg curve.
- MPH space cannot go below ~1.44 bits/key (FKS).
- Static predecessor search is **not** O(1) at linear space (Pătrașcu–Thorup),
  so no ordered structure gets constant-time lookup in the worst case.

Not provable, and not provable in principle:

- Which point to pick. The table above has six non-dominated entries. This is
  the RUM tradeoff (Athanassoulis et al., EDBT'16) made concrete: read,
  update, memory — optimise two. There is no dominating answer to find, only a
  workload to state.

## 8. Where the theory fails to predict, and the next measurement

Two gaps, both worth naming rather than papering over.

**RESOLVED — and the model was right.** This section previously recorded an
unexplained result: `heap-hash` costs three dependent misses, `hash+flat` two,
and yet `hash+flat` lost 475 ns to 369.

Hardware counters were unavailable — this is a Firecracker guest, and `perf`
reports every PMU event as `<not supported>` — so the model's *inputs* were
instrumented instead. `indexlab trace` records the byte ranges each lookup
reads and counts distinct 64-byte lines and 4 KiB pages:

| layout | reads | distinct lines | distinct pages |
|---|---|---|---|
| heap-hash | 3.20 | 3.53 | 3.01 |
| hash+flat | 2.20 | **2.67** | **2.01** |
| hash+paged | 5.20 | 5.69 | 3.30 |

So the model's input was correct: `hash+flat` really does touch fewer lines and
fewer pages. Subtracting the miss path, which is identical for both (240 ns
against 253), sharpened it further — `heap-hash`'s *two* extra memory accesses
cost 126 ns while `hash+flat`'s *single* access cost 241 ns. One access costing
twice what two cost is not a memory effect.

The remaining candidate on that path was varint decoding: four varints, a
serial branchy loop with data-dependent exits. `hash+flatfixed` tests exactly
that and nothing else, storing the extent as four fixed u32s:

| layout @ 10M | hit | miss | scan | B/key |
|---|---|---|---|---|
| heap-hash | 366 ns | 240 ns | 2.69 ns/e | 98.8 |
| hash+flat (varint) | 494 ns | 253 ns | 7.33 ns/e | 54.2 |
| **hash+flatfixed** | **314 ns** | 248 ns | 2.95 ns/e | 61.1 |

Varint decoding was the entire anomaly, and it was an artifact of how the
records were encoded rather than anything about the layout. With it removed the
mmap-able layout is **faster than the current heap index on the dominant
operation**, level on misses and scans, at 1.6× less memory — and it is
shareable and opens in O(1), which the heap index can never be.

The lesson generalises past this benchmark: **variable-length encoding is a
space optimisation that must not sit on a latency-critical path.** Sixteen
fixed bytes cost about 7 B/key more than varints and bought back 180 ns per
lookup. The same encoding choice is still on the hot path in `hash+paged` and
`mph+paged`, which is the obvious next thing to fix.

**Every layout is far beyond TLB reach, and none of them was measured with
huge pages.** A typical L2 TLB of ~1536 entries covers 6 MiB with 4 KiB pages.
The structures here are 134–942 MiB, so every random access risks a page-table
walk on top of the data miss. With 2 MiB pages the same TLB covers ~3 GiB —
enough for all of them.

That predicts a substantial, possibly ordering-changing win for every
mmap-backed layout from `MADV_HUGEPAGE`, and it is a deployment knob rather
than a structural change. The engine currently calls `madvise` nowhere at all,
which the architecture review already flagged for a different reason. This is
the highest-leverage untested hypothesis the theory hands us, and it is cheap
to test.

## References

- Aggarwal & Vitter, *The Input/Output Complexity of Sorting and Related
  Problems*, CACM 1988.
- Brodal & Fagerberg, *Lower Bounds for External Memory Dictionaries*, SODA 2003.
- Athanassoulis et al., *Designing Access Methods: The RUM Conjecture*, EDBT 2016.
- Pătrașcu & Thorup, *Time-Space Trade-Offs for Predecessor Search*, STOC 2006.
- Fredman, Komlós & Szemerédi, *Storing a Sparse Table with O(1) Worst Case
  Access Time*, JACM 1984.
- Esposito, Graf & Vigna, *RecSplit: Minimal Perfect Hashing via Recursive
  Splitting*, ALENEX 2020.
- Dillinger & Walzer, *Ribbon Filter: Practically Smaller Than Bloom and Xor*,
  2021.
- Crotty, Leis & Pavlo, *Are You Sure You Want to Use MMAP in Your Database
  Management System?*, CIDR 2022.
