# f38-fanout: does the read lead survive segmentation?

Registered before the first full run, like `read-decomposition-plan.md`. The
next engine's write side wants immutable sealed segments with a WAL in front
(the value-log arc and f37 both point there), which turns every point lookup
into a probe across k segments instead of one. The read lead is the one axis
this engine has won on two architectures (EXT.11: 1.355x on x86, 2.42x
replicated on Apple Silicon), and ext-readdecomp established its mechanism is
per-lookup compute — the probe is cheaper than the descent. Fan-out spends
extra probes, which attacks the lead at exactly its mechanism. This experiment
prices that before the design hardens.

## Shape

One process, five arms interleaved under `Trial`, same total data in every
arm: N keys, one 100-byte value each (EXT.11's read shape), uniform probes of
present keys.

- **k1** — all keys in one store; the baseline, today's engine.
- **fan4 / fan16** — keys split round-robin over k stores; the reader does not
  know which segment holds a key and probes them in fixed order until a
  lookup hits. Hit position is uniform, so a probe costs (k+1)/2 segment
  lookups on average. This is the unfiltered LSM read.
- **oracle4 / oracle16** — same k stores, but the reader consults the right
  segment directly. This is the upper bound of what a perfect per-segment
  existence filter buys; a real filter sits between fan and oracle.

Segment stores are built grouped (the roll's shape), `defer_merge` at its
default, one value per key so consolidation never fires; the arms differ only
in segment count and probe policy.

## Predictions, from numbers already on the books

A failed probe is a hash-slot miss: no extent decode, no block touch —
`f28-count` prices resolve-and-stop at 77 ns and a miss is at most that. The
k1 read (resolve + one block read of 100 bytes) should land near EXT.11's
~850 ns/op on this host.

- **P1 — fan cost is linear in probes, 40–120 ns per extra probe.**
  fan4 pays ~1.5 extra probes: predicted 0.85–0.95x of k1.
  fan16 pays ~7.5 extra probes: predicted 0.55–0.75x of k1.
  Refuted low if fan16 ≥ 0.85x (fan-out nearly free — filters unnecessary,
  segment count a non-issue); refuted high if fan16 ≤ 0.40x (superlinear
  per-probe cost — TLB/mapping spread — and segment counts must stay tiny or
  merge aggressively).
- **P2 — segmentation itself is free; only the probing costs.**
  oracle4 and oracle16 within noise of k1 (≥ 0.95x). Refuted if oracle16
  ≤ 0.90x: then splitting the data across mappings taxes reads even with a
  perfect filter, and the design needs fewer/larger segments, not better
  filters.
- **P3 — the x86 read lead (1.355x) survives fan4 and dies by fan16.**
  1.355 × 0.90 ≈ 1.22x at fan4 (survives); 1.355 × 0.65 ≈ 0.88x at fan16
  (gone). So an unfiltered design is viable only if compaction holds live
  segment counts near 4; with filters the bound is P2's instead.

## What this decides

If P1/P2 hold: segments + per-segment existence filters are the design, and
the filter's false-positive rate budget follows from the measured per-probe
cost. If P1 refutes high or P2 refutes, the segment fan is the wrong shape
for this read path and the next engine needs a global index over segments
instead — a different design with its own costs. Either way the decision is
made by measurement rather than by analogy to LSM folklore.

Absent-key lookups (logshed's R4.3 axis — absence prunes with certainty) cost
k probes unfiltered and ~0 filtered, so they widen whatever gap this measures;
they are the follow-up, not this experiment.
