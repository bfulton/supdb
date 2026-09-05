# The index decision, in theoretical context

`indexlab` measures twelve index layouts. This is what the theory says about
the result — including the places the theory predicts something the
measurement does not show, which are the interesting places.

The experiment, its claims and the results it wrote are in
[supdb-bench](https://github.com/bfulton/supdb-bench); paths below are that
repository's. All figures are 10M sixteen-byte decimal keys, `--profile full`,
from `results/f9-index-layout.full.json`. The two fixed-extent paged arms are also
measured pairwise against their varint originals, interleaved in one process,
in `results/f10-pair-*.full.json`; the probe's own cross-layout figures come
from sequential `Trial` blocks and are a table rather than a claim.

| layout | hit | miss | scan | B/key | MiB @ 10M |
|---|---|---|---|---|---|
| heap-hash (today) | 370 ns | 243 ns | **2.52 ns/e** | 98.8 | 942 |
| **hash+flatfixed** | **307 ns** | 244 ns | 2.94 ns/e | 61.1 | 583 |
| hash+flat | 462 ns | 239 ns | 7.19 ns/e | 54.2 | 517 |
| hash+pagedfixed | 481 ns | 255 ns | 3.52 ns/e | 49.8 | 475 |
| hash+paged | 673 ns | 249 ns | 5.98 ns/e | 42.9 | 409 |
| mph+pagedfixed | 676 ns | 656 ns | 3.51 ns/e | 28.8 | 275 |
| mph+paged | 767 ns | 623 ns | 5.74 ns/e | 21.8 | 208 |
| mph+bloom+paged | 855 ns | **245 ns** | 5.66 ns/e | 24.6 | 235 |
| hash+packed | 1102 ns | 251 ns | 8.33 ns/e | 41.0 | 391 |
| packed | 1113 ns | 1111 ns | 7.25 ns/e | **14.1** | 134 |
| packed+radix | 1104 ns | 1092 ns | 7.59 ns/e | 14.2 | 135 |
| btree | 1390 ns | 1419 ns | — | 36.8 | 351 |

Read the pairs together rather than the column: `hash+pagedfixed` and
`mph+pagedfixed` each buy roughly 1.2–1.4× on hits and 1.6× on scans from
their varint originals, for 16% and 34% more space respectively. Only
`hash+flatfixed` is faster than the heap index outright, and it is the largest
of the mmap-able layouts.

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

**DONE — and the generalisation held, at about half the strength predicted.**
`hash+pagedfixed` and `mph+pagedfixed` apply the same change to the paged
blob. They are arms rather than replacements, and `indexlab pair` measures
each against its varint original **interleaved in one process**, because the
probe's cross-layout comparisons come from sequential `Trial` blocks and that
is not a basis for a claim about a change. At 10M keys, `--profile full`,
decimal16:

| pair | hit | scan | B/key | verdict |
|---|---|---|---|---|
| `hash+paged` → `hash+pagedfixed` | 694 → 508 ns (**1.37×**) | 5.10 → 3.22 ns/e (**1.59×**) | 42.9 → 49.8 | p=0.0022 / 0.0122 |
| `mph+paged` → `mph+pagedfixed` | 788 → 662 ns (**1.19×**) | 5.36 → 3.32 ns/e (**1.61×**) | 20.5 → 27.4 | p=0.0022 / 0.0122 |

Predicted ~350 ns and ~450 ns; measured 508 and 662. The prediction assumed
the varint was the same share of the path it was in `hash+flat`, where it
*was* the path. A paged hit also pays a page-directory lookup, a slot-directory
read and a prefix comparison, and an MPH hit pays several level bit-array
probes and a rank before it reaches the record at all — which is exactly why
the MPH arm gains least. The denominator matters, and reasoning about the
numerator alone overstated both.

Absent-key lookups are **unchanged** in both pairs (p=0.25, p=0.70), which is
the result that makes the rest credible: a miss fails at the key comparison and
never reaches the extent, so an encoding change behind that comparison must not
move it. That is recorded as finding P3 in each pair rather than left as an
observation, so a future run that *does* move it fails the build.

The scan column is the more consequential half. F9.5 fails because no composite
scans as fast as the heap index; at 3.22 against heap-hash's 2.71 ns/entry the
gap is 1.19× rather than 1.88×. Still failing, and F9.5 stays `fails` in
`claims.json` — but it now fails by a margin that a page-layout change could
plausibly close, rather than by one that says the approach is wrong.

The space cost is the thing to watch, and it is not symmetric: +16% for the
hash arm, +34% for the MPH arm, because the MPH arm has less other space to
absorb the same twelve bytes. Both are pinned as `max` metrics in
`claims.json` so speed cannot be bought with space again without a person
deciding to.

**TESTED — and the prediction was too strong.** The argument was that an L2
TLB of ~1536 entries covers 6 MiB at 4 KiB pages while these structures are
134–942 MiB, so every random access risks a page-table walk; 2 MiB pages raise
reach to ~3 GiB. That predicted a "substantial, possibly ordering-changing"
win.

Measured by running the whole sweep twice with the system THP setting toggled
(verified in effect: `AnonHugePages` went from 0 to 956 MiB), at 10M keys:

| layout | hit, 4 KiB | hit, 2 MiB | delta |
|---|---|---|---|
| heap-hash | 366.2 ns | 354.1 ns | −3.3% |
| hash+flat | 493.6 ns | 442.8 ns | −10.3% |
| hash+flatfixed | 314.0 ns | 299.1 ns | −4.7% |
| hash+paged | 682.2 ns | 647.8 ns | −5.0% |
| mph+bloom+paged | 890.8 ns | 859.7 ns | −3.5% |
| packed | 1078.1 ns | 1075.6 ns | −0.2% |
| btree | 1367.6 ns | 1394.4 ns | +2.0% |

Consistent single-digit gains for most layouts, nothing above 10%, and **the
ordering is unchanged**. The hypothesis is not supported at the strength it was
stated.

Two caveats keep this from being a firm negative. The arms are separate runs —
THP is a global kernel setting and cannot be interleaved within one process —
so by this project's own rule the comparison cannot clear the significance
gate, and drift of a few percent between runs is plausible. And the deltas
being small is itself consistent with the tracer: at 2–3 distinct pages per
lookup, the page-walk caches likely absorb most of the cost, so there was less
TLB pressure to relieve than the reach calculation implied.

The practical reading: huge pages are worth having and cost nothing to enable,
but they are a few percent, not a redesign.

## Is one implementation enough for every machine?

The tuning constants are the only part of the design that is plausibly
machine-dependent. The structural findings are not: flat records beat
pointer-chasing because they cost one fewer dependent load and no per-key
allocation, and fixed-width beats varint on a hot path because a branchy serial
decode is compute rather than memory. Neither mechanism refers to a cache line,
and the varint one should if anything be *stronger* on a machine with a weaker
branch predictor.

That leaves the granularity constants — records per page, restart group size,
compression chunk size — which are sized against a cache line or a memory page,
and those differ by 2× and 4× between x86-64, Graviton and Apple Silicon.
`Machine::detect` reads them at runtime and derives the constants, so one
binary adapts rather than being tuned for whichever machine it was benchmarked
on.

`indexlab sweep` tests whether the derivation is good enough. On x86-64
(64-byte lines, 4 KiB pages, derived value 32) at 2M keys, interleaved with the
significance gate:

| records/page | hit | scan | B/key | rel IQR |
|---|---|---|---|---|
| 8 | 553.4 ns | 6.22 ns/e | 33.9 | 3.6% |
| 16 | 559.4 ns | 5.82 ns/e | 33.0 | 4.6% |
| **32 (derived)** | 550.7 ns | 5.84 ns/e | 32.5 | 4.5% |
| 64 | 540.7 ns | 5.97 ns/e | 32.5 | 4.9% |
| 128 | 528.8 ns | 6.12 ns/e | 32.8 | 3.2% |
| 256 | 518.9 ns | 5.70 ns/e | 32.8 | 4.8% |

The derivation is 6% off the best setting, and the gate calls that a real
difference (p = 0.0215). So the mechanism behind it — "size the slot directory
to one cache line" — is not the right formula; larger pages keep winning, which
points at page-directory size and prefix amortisation rather than slot
locality.

**But the more useful number is the spread: 7.7% across a 32× range of the
parameter.** The constant barely matters. That is the strongest available
evidence for one unified implementation: if a 32× error in the tuning costs
under 8%, then a mediocre derivation is fine and no per-architecture table is
being bought. The question is whether that insensitivity survives a machine
with 128-byte lines, where the same parameter spans a different number of
lines.

The formula is deliberately *not* being refit to this one result. Fitting a
derivation to a single machine is how a constant becomes machine-specific
while looking principled. It should be fit once there are at least two cache
geometries to fit against.

### The 2M x86 baseline, held for the Apple Silicon comparison

The Apple Silicon sweep cannot run at 10M keys: localmost caps a job at 600
seconds, measured twice to the second, and a 10M sweep does not fit. It will
run at 2M instead — which answers the sweep's own question, since that is a
comparison among candidates on one machine, but says nothing across
architectures unless x86 is measured at the same scale. So it was, on the same
idle Linux box as everything else here:

| records/page | hit | scan | B/key | rel IQR |
|---|---|---|---|---|
| 8 | 580.8 ns | 7.57 ns/e | 33.9 | 5.9% |
| 16 | 568.3 ns | 6.06 ns/e | 33.0 | 7.8% |
| **32 (derived)** | 572.2 ns | 5.83 ns/e | 32.5 | 4.9% |
| 64 | 569.0 ns | 6.86 ns/e | 32.5 | 5.2% |
| 128 | 571.6 ns | 6.52 ns/e | 32.8 | 5.7% |
| 256 | 579.4 ns | 6.69 ns/e | 32.8 | 5.4% |

`per_page=16 vs derived=32: NO DIFFERENCE (ratio 0.993, p=1.0000)`.

At 2M the derivation is not merely close to the best setting, it is
indistinguishable from it, and the spread across the whole 32× range is
**2.2%** rather than 7.7%. That is a stronger result than the 10M one and it
should be read with suspicion for exactly that reason: at 2M these structures
are 30–70 MiB and sit far closer to last-level cache than the 10M versions do,
so the parameter has less opportunity to matter. It is the right control for
the Apple Silicon run and the wrong number to quote as the headline.

Two caveats on the sweep. Interleaving means all six layouts are resident and
accessed round-robin, so every measurement runs against a cache the others have
polluted; absolute latencies here are higher than the single-layout figures
elsewhere in this document and are not comparable to them. And the earlier,
blocked version of this sweep gave an unstable answer — two runs disagreed
about which setting was best — which is what interleaving fixed and why the
project's own rule exists.

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
