# f44-tail: is the L0 tail what costs the read lead?

Registered before the run, as f38 through f43 were.

## Why

EXT.23 refuted P-B twice (0.846x, 0.850x): with the level structure live,
the new engine reads slower than LMDB where the old engine's single store
read faster. The diagnostic that prompted this experiment, at ext-kv's own
scale (1M keys, 116 MB, 8 MB seals):

| arrangement | segments | L0 tail | reads/s |
|---|---|---|---|
| one store (`supdb-buffered`, same record) | 1 | -- | 1,276,920 |
| next, no compaction | 14 | 14 | 733,801 |
| next, T=8 | 21 | 6 | 800,286 |
| next, T=4 | 21 | 5 | 835,766 |
| lmdb (same record) | -- | -- | ~950,000 |

Routing is working -- fewer unrouted segments reads faster, monotonically --
and the tail bound is being enforced (5-6 against a trigger of 4, the extra
from a merge deferring rather than blocking). But 21 routed segments still
cost 35% against the same data in one store, and that 35% is the whole
distance from EXT.23 holding to failing.

F38.2 measured segmentation as free at this key count -- sixteen segments
indistinguishable from one store -- but its oracle knew which segment held
the key. It paid no fence search, no Bloom, and had no L0 tail. The
difference between that and this is what the tail costs, so that is what
this experiment measures.

## Shape

Five arms interleaved, ext-kv's scale and shape (1M keys, 8 MB seals, 100 B
values, durable batches of 1,000), each loading and then answering the same
uniform point reads: `no-compact` (every segment unrouted), and compaction
at `l0_trigger` of 8, 4, 2 and 1. Reported per arm: read rate, the live
level split, load rate, device bytes.

## Predictions

- **P44.1 — the read rate falls monotonically with the tail.** T=1 reads
  at least 1.15x of T=8. Refuted if the curve is flat: then the tail is not
  the cost and the fence search or the mapping count is, which is a
  different fix.
- **P44.2 — a minimal tail nearly recovers the single store.** At T=1 the
  read rate is at least 90% of the same data in one store (1,276,920/s in
  the cited record, re-measured here as the no-compact arm is not that
  baseline). Refuted means segmentation costs the read path even when
  perfectly routed at this scale, contradicting F38.2 and putting the
  design's whole read premise in question.
- **P44.3 — and it is bought with write.** T=1 loads at most 0.77x of T=8,
  because every seal triggers a merge that rewrites the live set. The trade
  curve, not a free lunch: F43.4 already priced compaction at 0.784x of the
  uncompacted load.

## What this decides

If P44.1 and P44.2 hold, the tail bound is the read path's dial and EXT.23
is recoverable by turning it down -- at a write cost P44.3 quantifies, which
is then the policy question the brief's "partitioned compaction policy"
entry has been waiting for. If P44.2 refutes, routing is not the answer to
fan-out at all and the design needs the decomposition F38 deferred:
per-segment index residency, mapping count, and whether one store's
contiguous index is simply better than k of them.
