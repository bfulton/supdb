# The next engine: a design brief written against the measurements

Every assertion here cites a recorded result in `results/` or a claim in
`claims.json`. The brief exists because the current engine's remaining
failures are structural rather than incidental, and because the two
load-bearing unknowns of the obvious replacement shape have now been measured
(f38, f39) instead of assumed. Nothing below is built; the promises are
registered so the build can be falsified.

## Why start over

Three measured facts no iteration on the current design can fix:

1. **Index publication is O(key count).** `checkpoint` rewrites the whole key
   index (F4.1: a 1,000-op durability window costs 25x; F31.1: checkpoint is
   44% of a bulk load; YCSB-E loses at 0.43x even unmatched). The value log
   made durability points cheap but any checkpoint that publishes index state
   still pays in proportion to keys, not to change.
2. **There is one appender.** F6.1: write throughput barely scales with
   writer threads, and the claim names the single appender mutex.
3. **The mmap read path degrades 916x out-of-core** (F1.2), with default
   readahead amplifying a random read 86,977x (F23.1) and no auto-picked
   threshold that works (F24.1, F24.2).

And one measured fact that says what must survive: the read lead is real,
replicated, and mechanistic — the flat-index probe beats the B-tree descent
per lookup (EXT.11 1.355x x86 / 2.42x Apple Silicon; ext-readdecomp run 1
and 2 agree the lead is per-lookup compute, not cache-line or page-size
luck).

## What is inherited unchanged

- **The falsification harness.** `claims.json`, `verify`, `stats::compare`,
  interleaved arms, the profiles, the not_exercised discipline. The new
  engine is built under the same gates and its claims live in the same file.
- **The sealed-segment read path.** `flatindex` over a packed section, the
  `block` decoder, `Bytes`/`Blob`. f9-index-layout already put this layout on
  the frontier (F9.3: beats a bulk-loaded B+tree on speed and size; F9.5:
  nothing composite scans faster), and a sealed segment is byte-for-byte the
  shape `Blob` reads today — the browser reader carries over whole.
- **The schema-property fast paths.** `count_fixed` / `scan_counts_fixed`
  (W2.2-W2.4), which logshed already depends on.

## The shape

A WAL is the only mutable thing. Sealed segments are immutable. There is no
checkpoint.

- **Commit** = append the batch to the WAL, one fdatasync. f39 measured that
  shape with all engine work removed at **1,191,125 ops/s** on this host
  (0.84ms/barrier), 2.08x LMDB's recorded durable load, and at **1,014,003**
  with the per-op bookkeeping no engine can skip (F39.1, F39.2). Today's
  engine commits 5.85x below its own floor (F39.3) on work — arena append,
  section publication — that this design deletes rather than optimizes.
- **Seal** = when the memtable reaches segment size, write one immutable
  segment (data blocks + its own flat index), fsync it, truncate the WAL.
  Sealing is off the commit path; a durability point never publishes index
  structure, which is what removes F4.1's mechanism rather than its cost.
- **Read** = probe segments, newest first. f38 measured the two halves of
  this: segmentation itself is free (F38.2 — sixteen perfectly-routed
  segments indistinguishable from one store), and unrouted probes cost
  90ns each (F38.1), which kills the read lead already at four segments
  (F38.3, a registered prediction refuted — the plan said the lead survives
  k=4 and it does not). **Routing is therefore required, not optional** —
  and f40/f41 measured every candidate shape. Per-segment blocked Blooms
  keep 82% of k1 (F40.1: a fixed probe order queries ~8.5 filters per
  lookup). A generic global map manages 62% of the ceiling (F40.2, refuted)
  and a purpose-built one-line fingerprint table 71.5% at 6.7x the blooms'
  memory for a statistical tie with them (F41.1, F41.2, both refuted): at
  1M keys any router consulted per lookup pays a DRAM miss on a keys-sized
  structure. The conclusion is structural — the only free routing is
  information the reader already holds, so **routing belongs to compaction,
  not to filters**: compacted levels are key-range partitioned and a
  two-comparison fence routes them for nothing (the same fence F40.3 shows
  inert on overlapping ranges), while the small unpartitioned tail of
  recent segments carries per-segment Blooms. The perfectly-routed ceiling
  is worth reaching: sixteen quarter-size indexes read 20% faster than one
  store (566ns against 522 — oracle16 vs k1, f41).
- **Compact** = merge segments under the policy f37 already priced: geometric
  size ladders bought 3.963x on fragmenting writes for a 0.762x read tax
  (F37.1, F37.3), and F38.2 says read cost does not force merging — only
  filter false-positive accumulation and space do.
- **Write scaling** = one active memtable+WAL per shard or per writer;
  segments make the shared-appender mutex (F6.1) unnecessary rather than
  cheaper.
- **I/O** = the read path is `Bytes` all the way down; mmap is one backend,
  explicit reads another. One read path instead of the current two, and the
  out-of-core decision (F1.2, F23, F24) becomes a byte-source choice the
  caller makes instead of a policy the engine mispicks.

## Registered promises (the build's falsifiers)

To be measured by the same experiments that convicted the current engine,
interleaved where the harness allows:

- **P-A, durable load:** EXT.9's shape lands **≥ 600,000 ops/s** on this
  host — within 1.7x of the raw+index floor and past LMDB's recorded rate.
  Below 600k the design has a leak that must be named; below 572k the axis
  is conceded and the brief's premise was wrong.
- **P-B, the read lead survives:** EXT.11's shape with live segment counts
  under the compaction policy stays **≥ 1.2x** on x86. This is F38's arithmetic
  obligation: routing must recover what fan-out spends. Standing at 1.447x
  and 1.405x against LMDB in two full runs (EXT.23), and f43 shows routing
  paying rather than costing at 23 live segments (F43.2, 1.117x).
- **P-C, the durability curve flattens:** the F4-durability sweep shows
  window cost independent of key count — the 25x at a 1,000-op window
  (F4.1) becomes a bounded, window-size-only cost. F4.1 flips or the design
  failed at its main job.
- **P-D, writes scale:** F6.1's sweep shows ≥ 2.5x at 4 writers. Refuted
  means the sharding is cosmetic.
- **P-E, crash semantics:** a store killed before any seal opens from the
  WAL alone (C3.4 flips), and history survives reopen (segments do not
  forget).

## Open, and deliberately so

- ~~Filter choice~~ — **answered by f40/f41**: fences via range-partitioned
  compaction for sealed levels, per-segment Blooms for the overlapping
  tail; global routing structures rejected by measurement twice.
- **Partitioned compaction policy** — built and measured (f43). The tail
  bound is a real dial: T8 sends 0.898x of T4's device bytes and scans
  0.910x as fast. What f43 also convicted is the merge itself — it
  rewrites the whole live set every time, so it costs 21.6% of durable
  load throughput (F43.4, a refuted P4.4) and its device ratio grows with
  the store. **An incremental merge — rewriting only the partitions the
  tail overlaps — is the open work**, and F43.4 is where it gets measured.
- **What the ordered axis actually costs.** P4.1 predicted partitioning
  recovers scans 12x; measured, it is 1.367x (F43.1, refuted). The axis
  was losing to the scan implementation, not to the fan: candidate
  enumeration through the posting-counting walk, and a hash probe per key
  per source. Both are fixed and both arms gained. EXT.24 needs
  re-measuring against LMDB before anyone knows where the ordered axis
  stands.
- **Segment size** — trades WAL replay length against segment count; needs
  its own sweep.
- **Group commit** — whether concurrent writers share a barrier; matters
  only after P-D.
- **What EXT.6 becomes** — segments plus a WAL will not beat LMDB on disk;
  the space claim stays failing and gets re-priced honestly.

## What this does not promise

Transactions, MVCC beyond segment-set snapshots, or beating LMDB out-of-core.
The guarantee set stays what `Features` can equalize, so every comparison the
external suite makes remains matched — the durable-commit axis finally
becomes equalizable in both directions, which is worth more to the
comparisons than any single number in this file.
