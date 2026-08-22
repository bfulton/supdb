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
- Crash injection (ALICE/CrashMonkey), fuzzing, `loom` on the reader protocol.
