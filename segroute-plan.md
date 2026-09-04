# f41-segroute: routing in one cache miss, or not at all

Registered before the first full run. f40 left the routing question in a
shape its own plan predicted it might: per-segment blooms cap at 82.1% of k1
(F40.1 — ~8.5 filter queries per lookup is the fan tax at a smaller
constant), and the generic global map refuted its prediction at 61.7% of the
oracle (F40.2 — ~290ns of hashing and DRAM walk per query). The refutation
clause said what to re-examine: a routing structure has to answer in one
cache miss or it is not a routing structure. Meanwhile the oracle itself
reads 20% faster than the single store (472ns against 568), so perfect
routing does not merely preserve the read lead, it extends it.

## The candidate

A flat, bucketized fingerprint table built at seal time: 64-byte buckets of
sixteen u32 entries, each entry a 28-bit key fingerprint plus a 4-bit
segment id; bucket chosen by one cheap hash, occupancy kept at load 0.5 so
a query is one line load and a 16-way compare, with at most one spill
bucket. ~8 bytes per key against the blooms' 1.25 — the memory trade is
6.4x and is recorded, not hidden. A false fingerprint match (odds ~2^-23
per query) routes to a segment whose read answers empty and falls back to
the fan, so correctness never rests on the filter.

## Shape

Four arms interleaved over the f40 builds, same-run: **k1**, **bloom16**
(the per-segment structure to beat), **table16** (the candidate), and
**oracle16** (the ceiling).

## Predictions

- **P1 — table16 lands at 85-100% of oracle16.** One predicted cache miss
  (~60-90ns) over a ~472ns routed read. Refuted low means even one global
  miss is too dear on this host: the design keeps per-segment blooms,
  accepts ~82% of k1, and the read-lead promise P-B gets re-derived from
  that number. Refuted high (>100%) is suspect and gets scrutinized, not
  celebrated.
- **P2 — table16 beats bloom16 outright** (a gated Greater, not a lean).
  This is the decision gate: if the one-miss table cannot clearly beat the
  zero-global-state blooms, global routing is not worth its mutability
  concession at any price measured so far.

## What this decides

The brief's "Filter choice" open question closes either way: table16
clearing both gates makes the routing table the design's one new structure,
sized ~8B/key, rebuilt at seal/compact from segment indexes, with blooms
unnecessary; P2 failing keeps routing entirely inside immutable per-segment
state and the design pays the 18-point gap to the ceiling knowingly. The
absent-key axis (blooms win those categorically; a table must still probe
once on a true miss unless paired with a bloom) is measured after the
structure is chosen, with logshed's R4.3 shape.
