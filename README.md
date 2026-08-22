# Supdb

An ingest-optimized embedded key-multivalue store in Rust, and the evidence for
and against it.

This repository holds two things that are meant to stay together: the engine,
and a benchmark suite whose job is to **try to falsify** the claims made about
it. The engine came from a design document; the suite came from reviewing that
document and asking what measurement would prove it wrong.

```
src/               engine  (~1,400 lines, two dependencies: memmap2, lz4_flex)
src/bench/         the measurement substrate  -- repetition, significance, latency, I/O accounting
src/bin/internal   the falsification suite    -- Supdb against itself, as it scales
bench/external/    the comparison suite       -- Supdb inside other projects' evaluations
results/           committed measurements     -- the source of truth for figures and claims
figures/           publication-quality SVG    -- generated from results/, never by hand
claims.json        every statement, with the state it is expected to be in
docs/              the architecture review that produced all of the above
```

## Quick start

```sh
cargo build --release --workspace
cargo run --release --bin internal -- all --profile dev      # falsification suite
cargo run --release --bin external -- all --profile dev      # against redb, LMDB, sled
cargo run --release --bin verify                             # claims vs measurements
cargo run --release --bin figures                            # results/ -> figures/*.svg
```

## What the measurements currently say

Taken on 4 cores / 15 GB at the `ci` and `dev` profiles. **Neither is citable
evidence** — only `--profile full` is, and these are the honest small-scale
numbers, not headline ones.

**What holds.** Supdb has the fastest bulk load in the field, and the smallest
file — 1.55× smaller than LMDB on the same data. Every reader process sees a
complete, self-consistent state under a live writer. Chunk-granular
decompression inside a packed block is a genuinely good idea and it works.

**What does not.**

| finding | measured |
|---|---|
| reader open is independent of key count | **super-linear**: 20× the keys costs 34.7× the open |
| a short-lived reader process is viable | 100 reads against 200k keys costs 433 µs/read against a 1.4 µs steady state; break-even is 16,384 reads |
| write throughput scales with threads | 4 threads is **0.86×** of one thread — negative scaling |
| durability is affordable | a 1,000-op loss window costs **25×** throughput; no usable point on the curve |
| the mean summarises append cost | p99.9/mean = 61×; one checkpoint stalled **32.8 seconds** |
| many reader processes are safe | 80 readers past the 64-slot table, held 35 s past the 30 s stale window: **2 read errors** |
| Supdb reads faster than LMDB | measured natively, LMDB is **2.4× faster** on reads and 4.7× on scans |
| Supdb sustains a mixed read/write workload | YCSB-A runs **13.5× slower** than read-only YCSB-C |

The last two matter most, because they contradict the design document rather
than merely extending it. The LMDB comparison reverses once the Java harness is
removed. The mixed-workload result is structural: `Store` exposes no read
method, so a read after a write needs a checkpoint and a fresh `Reader`, both
`O(key count)` — and no benchmark in the original suite mixes reads with
writes.

## The rules

Four, enforced in code rather than remembered:

1. **Nothing is measured once.** Configurations run interleaved; results are a
   median with an interquartile range and a bootstrap interval.
2. **A difference is not a difference until it clears the gate** — Mann-Whitney
   U at p < 0.05 *and* a minimum effect size. The design document's own rule
   was "nothing under ~15% means anything without repetition", and it then
   reported a 13.9% difference as a win. That case is a regression test.
3. **A finding whose precondition was not met reports `not_exercised`**, never
   `holds`. An untested hazard must not read as a green build.
4. **Throughput never travels alone** — latency distribution, peak RSS, and
   bytes actually written to the device come with it.

## How this stays honest

`claims.json` records the expected state of every finding, *including the ones
that currently fail*. `verify` checks it against `results/` and CI runs it, so:

- a limitation that gets worse turns the build red;
- a limitation that gets **fixed** also turns the build red, because either the
  engine improved and the claim is stale, or the experiment stopped testing
  anything. Both need a person.

That symmetry is the point. A known problem written down is a problem that
cannot be quietly forgotten.

## Status

The engine is a prototype and several gaps are expected of one. Two are large
enough to name here: a store cannot be reopened for writing (`Store::create`
always truncates, and there is no `Store::open`), and there are no checksums on
any data block. `docs/architecture-review.md` is the full account.
