# Milestone 4: range-partitioned compaction — registered before the code

Written while the seven-engine ext-kv full run holds the machine (nothing
may compile beside a timing run), registered before any compaction code
exists, in the same discipline as fanout-plan.md through segroute-plan.md.

## What it is

The routing verdict (f40/f41, recorded in F40.1-F41.2) was structural: any
global router consulted per lookup pays a DRAM miss on a keys-sized
structure, so the only free routing is information the reader already
holds. Compaction is how the reader comes to hold it:

- **Partitioning merge.** When the overlapping tail reaches T segments,
  merge them into P disjoint key-range segments (P chosen so each lands
  near the segment-size target). The merge is a k-way ordered walk of the
  tail's indexes — the same walk `Db::scan` does today — written out
  through the existing `Store` writer, one output segment per range.
- **Fences.** Each partitioned segment's min/max key rides its file name
  (the same trick the covered end-sequence uses), so `open` learns the
  ranges for free and a point read binary-searches the fence list — two
  comparisons, no side structure, the F40.3 fence made useful by making
  ranges disjoint.
- **Tail blooms.** The unpartitioned tail (segments newer than the last
  partitioning merge) carries the per-segment blocked Blooms f40 built,
  at 1.25 B/key, worth 82% of k1 when the tail is all there is (F40.1) —
  and the tail is bounded at T, so the bloom walk is bounded with it.
- **Scan.** A scan touches every tail segment (bounded by T) plus exactly
  the partitioned segments whose ranges intersect the scan — for most
  scans, one. This is what EXT.24 is waiting for.

Deletes stay out of scope until this milestone lands: a tombstone's
semantics (mask everything older, drop at partitioning merge) depend on
merge order, so building them before the merge exists would be building
them twice.

## Predictions

- **P4.1 — EXT.24 recovers from ~0.001x to at least 0.5x of LMDB.** A
  post-compaction scan is one fence search plus one segment's index walk,
  which is the shape EXT.5/EXT.12 measured at parity. Refuted low means the
  merge left more overlap than the design assumes, or the tail dominates.
- **P4.2 — the read path over a compacted store holds P-B (≥ 1.2x on
  EXT.23's shape)** with a tail of T=4: one fence search + at most 4 bloom
  checks + one segment read. From f38/f40 arithmetic: fences ~free, 4
  blooms ≤ ~60ns, one probe ~90ns budget — against the 566ns oracle read,
  predicted ≥ 85% of oracle. Refuted means the tail bound or the bloom
  cost was mispriced and T shrinks.
- **P4.3 — the partitioning merge costs less device traffic than today's
  checkpoint regime per byte ingested.** The merge writes each byte O(1)
  times per level crossed (F37's geometric argument); the prediction is
  total device bytes for load+compact stays under 2x the value-log
  engine's 297.8 MB on the f42 shape. Refuted means the merge schedule is
  too eager.
- **P4.4 — durable load throughput does not regress**: f42's 800k ops/s
  within noise with compaction enabled, because the merge runs on the seal
  thread's schedule, never the commit path's. A regression convicts the
  backpressure, not the merge.

## The experiments

f43-compact: arms [no-compact (today's M2), compact-T4, compact-T8]
interleaved on the f42 load shape, measuring load ops/s, device bytes,
disk bytes, then read and scan over the loaded store — one experiment, all
four predictions. EXT.22-24 re-run at full after it lands, same canonical
engine set.

## What this decides

T (the tail bound) and P4.1's verdict close the brief's "Partitioned
compaction policy" question. If P4.2 refutes, the fence+bloom composition
is wrong and the read path needs re-decomposition before more building.
