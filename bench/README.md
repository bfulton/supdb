# Benchmarks

Two suites, with different jobs.

## Internal — `internal`

Supdb measured against itself as it scales. Six experiments, chosen because
they are the ones most likely to **falsify** the design rather than confirm it.
Several are expected to fail; a suite that only contains tests the engine
passes is a marketing document.

```
cargo run --release --bin internal -- all --profile dev
```

| id | asks |
|----|------|
| `f1-outofcore`  | does read throughput survive the dataset outgrowing memory? |
| `f2-open`       | is reader open cost independent of key count, and when does a short-lived process break even? |
| `f3-multiproc`  | do many reader processes against a live writer see consistent state? |
| `f4-durability` | what does a bounded data-loss window cost in throughput? |
| `f5-latency`    | what is the distribution behind the throughput means? |
| `f6-threads`    | does write throughput scale with writer threads? |
| `f7-index`      | how much memory does a reader's index cost, and where is the ceiling? |

## External — `external`

Supdb entered into **other projects' evaluations**, on their workload
definitions rather than ours. Comparators are redb, LMDB (via `heed`) and sled
— all native Rust bindings, so no measurement crosses a language boundary.

```
cargo run --release --bin external -- all --profile dev
```

| suite | shape |
|-------|-------|
| `kv`   | redb's own benchmark: bulk load, random reads, range scans |
| `ycsb` | YCSB core workloads A–F (Cooper et al., SoCC'10), Zipfian θ=0.99 |

Every external result carries each engine's **feature score** — durable commit,
transactions, checksums, reopen-for-write, read-your-writes, ordered scan.
Supdb provides one of six; the others provide five or six. A throughput number
that does not say so is comparing promises as much as implementations.

## Correctness — `correctness`

A fast wrong answer is not a result, so these produce `Finding`s in the same
format and are governed by the same claims file.

```
cargo run --release --bin correctness -- all --profile dev
```

| id | asks |
|----|------|
| `c1-decoders` | does a damaged file produce an error, or take the host process down? |
| `c2-oracle`   | does the store agree with a `BTreeMap` model over random operation sequences? |
| `c3-crash`    | what survives a writer killed at an arbitrary point? |

`c1` aims damage at the key index deliberately. Uniform random corruption
almost always lands in a value payload, where a flipped byte is structurally
harmless and silently served — which is itself a finding, but a different one.

## Index layout laboratory — `indexlab`

Not a benchmark of the engine: a benchmark of a *proposed replacement* for its
weakest part, run before anyone writes code against it.

```
cargo run --release --bin indexlab -- --profile full
```

Six layouts × three key shapes × three scales, with correctness assertions
before any timing and resident size measured in a child process. It exists
because the architecture argument for replacing the reader index turned on an
assumption about constant factors, and that is not the sort of thing to settle
by reasoning.

It has already overturned two recommendations, including mine. See
`results/f9-index-layout.full.json` for the measurements and
`docs/index-theory.md` for where they sit against the known bounds — including
the two places the theory predicts something the measurement does not show.

## Profiles

`--profile ci` runs in seconds and is **never citable**; it proves the
experiments run. `dev` is minutes. `full` is the only profile a published claim
may cite, and results record which they were taken at.

## The rules every number obeys

1. Nothing is measured once. Configurations run **interleaved**, reported as a
   median with an interquartile range and a bootstrap interval.
2. A difference is not a difference until it clears the gate: a Mann-Whitney U
   test at p < 0.05 **and** a minimum effect size. The design document's own
   rule was "nothing under ~15% means anything without repetition"; it then
   reported a 13.9% difference as a win. `stats.rs` carries that case as a
   regression test.
3. Throughput is never reported alone — latency distribution, peak RSS and
   **bytes actually written to the device** travel with it.
4. A finding whose precondition was not met reports `not_exercised`, never
   `holds`. An untested hazard must not read as a green build.
5. Every record carries the machine that produced it.

## Verification

`claims.json` records the expected state of every finding, including the
known-failing ones. `verify` checks it against `results/` and fails in **both**
directions — a finding that starts passing is as loud as one that starts
failing, because either the engine improved and the claim is stale, or the
experiment stopped testing anything.

```
cargo run --release --bin verify -- --profile ci
cargo run --release --bin figures -- --profile ci   # -> figures/*.svg
```

## Not yet built

Named so their absence cannot be mistaken for a passing result:

- RocksDB and Pebble as comparators (both need a non-Rust toolchain in CI).
- `db_bench --benchmarks=mixgraph`, the FAST'20 realistic workload.
- Real production traces (Twitter OSDI'20).
- `loom` on the reader-table claim protocol, and `miri` over the five `unsafe`
  blocks.
- Exhaustive crash-point enumeration in the ALICE sense. `c3-crash` samples
  crash points at random rather than enumerating the write sequence.
- Damage aimed at the block chunk directories, as `c1` aims at the key index.
