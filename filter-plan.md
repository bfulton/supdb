# f40-filter: routing against the 90ns budget

Registered before the first full run. F38 settled the shape — segmentation
is free (F38.2), unrouted probes cost 90ns each (F38.1), and the read lead
does not survive an unfiltered four-segment fan (F38.3) — so the next
engine's read path stands on one new data structure: something that answers
"does segment S hold key K", or better "which segment holds K", for well
under 90ns. This experiment prices the candidates instead of choosing from
literature.

## Shape

Six arms interleaved under `Trial`, over the f38 builds (one store; sixteen
segments, keys dealt round-robin so segment key-ranges fully overlap): 1M
keys, one 100B value, uniform present-key probes.

- **k1** — the single store; the anchor.
- **fan16** — unfiltered probe-until-hit; f38's arm, re-run in-process so
  every comparison here is same-run.
- **fence16** — per-segment min/max key fences consulted before probing.
- **bloom16** — a per-segment blocked Bloom filter (one 64-byte block per
  query, ~10 bits/key) consulted before probing.
- **route16** — a global map from key to segment id (hash map, built at
  open), one query then a direct probe.
- **oracle16** — f38's perfect router; the ceiling.

## Predictions

- **P1 — per-segment filters only halve the tax at k=16.** A fixed probe
  order queries ~8.5 filters per lookup, so even a 10-20ns filter pays
  85-170ns before its first data probe. bloom16 lands *between* fan16 and
  oracle16 — predicted 60-85% of k1 (fan16 sits at 44%) — and does not
  approach the ceiling. If bloom16 lands within 5% of oracle16 instead, the
  per-query cost is far below 10ns and per-segment filters suffice at this k.
- **P2 — the global route recovers at least 95% of oracle16.** One hash
  lookup (~20-40ns) against oracle's free routing, amortized over a ~500ns
  read. Refuted means a routing map's real cost is not its lookup and the
  bet on it needs re-examination.
- **P3 — fences prune nothing here.** With round-robin keys every segment's
  range covers every key: fence16 within noise of fan16. This is recorded so
  the design cannot assume fences work without key-partitioned sealing;
  fences are a compaction-policy benefit, not a general router.

## What this decides

P1 and P2 together pick the structure: if per-segment blooms cannot reach
the ceiling at k=16 and the global route can, the brief's routing structure
is a key→segment map maintained at seal/compact time — small (about a byte
per key), rebuildable from segment indexes, and the one concession to
global mutable state the design makes, priced here before it is made. If P1
refutes high (blooms near the ceiling), the design keeps routing entirely
inside immutable per-segment state, which is strictly simpler, and the map
is dropped. Absent-key probes — where blooms win categorically and a route
map must still answer — are the follow-up, measured with logshed's R4.3
shape once the structure is chosen.
