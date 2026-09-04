# f56: the tail bound under inline runs — registered before the run

The durable load sits at 0.5x of LMDB on random keys and the whole of the
gap is one decision: the store is routed at rest, so the drain ends by
reading and rewriting the live set. The decision was priced by f38 and f44
-- an unrouted probe cost ~90 ns and eight overlapping segments read at
0.77x of one -- when a probe was four cache misses ending in a block. With
inline runs a probe the Bloom lets through is two misses and a probe it
rejects is none. So the price of not routing is re-measured before anything
else is built.

## The experiment

The canonical shape (1M keys, 100-byte values, 1,000-record durable
batches), interleaved, the drain inside the window:

- `routed`: today's defaults -- 32 MB seals, trigger 4, the flush
  partitions what it sealed.
- `tail-4`: 32 MB seals, trigger 8, no partitioning at flush: about four
  live pieces after the drain.
- `tail-8`: 16 MB seals, trigger 16: about eight.
- `tail-15`: 8 MB seals, trigger 32: about fifteen.

Ingest-to-drain, phases, device and disk bytes, live segments after the
drain, then point reads over the drained store and one ordered scan.

## Predictions

- **P56.1 — at about eight pieces, point reads are at least 0.85x the
  routed arm's.** f44 had 0.77x before inline runs; two misses fewer per
  probe and none per Bloom rejection should recover a good part of it.
- **P56.2 — at about eight pieces, ingest-to-drain is at least 1.3x the
  routed arm's.** The drain's merge is gone and the seals overlap the load.
- **P56.3 — at about four pieces, reads are within 5% of routed** (a tie,
  or a ratio at or above 0.95).
- **P56.4 — the ordered scan is where it costs: at eight pieces it is at
  most half the routed arm's rate,** because the single-partition walk
  becomes a k-way merge over pieces. Registered so the trade is stated
  with the gain.

## What this decides

If P56.1 and P56.2 hold, routing moves off the drain: the flush publishes
and returns, compaction runs on the idle cores, and the load axis is
re-measured under that policy. If P56.1 refutes, the load gap on random
keys is the design's, honestly, and the remaining ingest lever is bounded
loss (`SyncPolicy::EveryN`, F48.1).
