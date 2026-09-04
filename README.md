# Supdb

A read-optimized embedded key-multivalue store in Rust, and the evidence for
and against it. It spends space to buy read and scan speed, and the trade is
recorded rather than implied.

This repository holds two things that are meant to stay together: the engine,
and a benchmark suite whose job is to **try to falsify** the claims made about
it.

```
src/db.rs          the engine -- WAL, memtable, sealed segments, compaction, deletes, Txn
src/blob.rs        the read path over any byte source; compiles for wasm
src/flatindex.rs   the flat key index the format is built on
src/bench/         the measurement substrate  -- repetition, significance, latency, I/O accounting
src/bin/internal   the falsification suite    -- Supdb against itself, as it scales
src/bin/correctness  the correctness suite    -- damaged files, crash injection
src/bin/logshed    the browser-reader suite   -- day-index shape, round trips, size budget
bench/external/    the comparison suite       -- Supdb inside other projects' evaluations
web/               the browser reader, its size budget and its browser test
results/           committed measurements     -- the source of truth for figures and claims
figures/           publication-quality SVG    -- generated from results/, never by hand
claims.json        every statement, with the state it is expected to be in
docs/              the architecture review that produced all of the above
```

## Quick start

```sh
sh scripts/check.sh          # everything CI runs: build, test, lint, browser, claims, suites
sh scripts/check.sh lint     # or one group at a time
```

CI calls the same script with the same group names, so what passes here is what
passes there.

```sh
cargo run --release --bin internal -- all --profile dev      # falsification suite
cargo run --release --bin external -- all --profile dev      # against redb, LMDB, sled, RocksDB
cargo run --release --bin correctness -- all --profile dev   # damage, oracle, crashes
cargo run --release --bin verify                             # claims vs measurements
cargo run --release --bin figures                            # results/ -> figures/*.svg
```

## What the measurements say

The figures are in `claims.json` and `results/`, not here: they move with every
canonical run, and only `--profile full` is citable. What is stable is their
shape. Every comparison below is **matched** — an engine is not ranked against
another until the two promise the same thing about durability, transactions and
checksums.

**Reads and scans lead.** Point reads beat LMDB and RocksDB tuned as it would
be deployed (`EXT.23`, `EXT.33`). Ordered scans tie LMDB and lead tuned RocksDB
(`EXT.24`, `EXT.34`). YCSB A, C, E and F all lead tuned RocksDB
(`EXT.42`–`EXT.45`).

**Space paid for them.** Blocks are stored uncompressed by default, which is
what a point read that decompresses nothing costs; RocksDB keeps the smaller
file. Compression is per segment rather than global -- `SegmentWriter` takes
it -- and on a real day's index it saves about a fifth of the file (`W6.8`).

**Ingest depends on arrival order.** The durable ordered load trails LMDB and
tuned RocksDB (`EXT.22`, `EXT.32`); under shuffled arrival it leads LMDB
(`EXT.27`), because a durable commit of scattered keys dirties about as many
B-tree leaf pages as it has keys. Quote the two together or neither.

**Correctness is where the suite has earned most.** The engine agrees with a
model of itself over randomized appends, deletes and crash-reopens. Damaged
files error rather than panic or serve wrong bytes (`C1.1`, `C1.2`). A
segment's key index is checksummed per piece, and every recovered state after
crash injection is an exact prefix of the commit order (`C4.1`–`C4.5`).

**What is open**, and recorded as failing on purpose: the durable ordered load
above; reads degrade sharply once the dataset outgrows memory (`F1.2`, `F1.4`);
and the index-layout study found points on the frontier that are both smaller
and faster than the shipping layout (`F9.7`). Each is a claim with an expected
state, so none can improve or decay unnoticed.

## The rules

Four, enforced in code rather than remembered:

1. **Nothing is measured once.** Configurations run interleaved; results are a
   median with an interquartile range and a bootstrap interval.
2. **A difference is not a difference until it clears the gate** — Mann-Whitney
   U at p < 0.05 *and* a minimum effect size.
3. **A finding whose precondition was not met reports `not_exercised`**, never
   `holds`. An untested hazard must not read as a green build.
4. **Throughput never travels alone** — latency distribution, peak RSS, and
   bytes actually written to the device come with it.

And one about method: to measure the cost of a change, run both arms
interleaved in one process. Running the suite before and after and subtracting
does not work here; between two such runs the *unchanged* comparators have
moved by tens of percent.

## How this stays honest

`claims.json` records the expected state of every finding, *including the ones
that currently fail*. `verify` checks it against `results/` and CI runs it, so:

- a limitation that gets worse turns the build red;
- a limitation that gets **fixed** also turns the build red, because either the
  engine improved and the claim is stale, or the experiment stopped testing
  anything. Both need a person.

That symmetry is the point. A known problem written down is a problem that
cannot be quietly forgotten.

## Where the reasoning lives

This file states what is true now. The history — why a decision was made, what
it cost, what was tried and refuted — is kept out of it on purpose, and lives
in:

| | |
|---|---|
| `claims.json` | every finding, its expected state, and the evidence for it |
| `CLAUDE.md` | the working notes: what broke, what was fixed, what not to repeat |
| `*-plan.md` | one per experiment: predictions registered before the run, outcome appended after |
| `docs/` | the architecture review that started it, and the engine's own design notes |
| `tests/` | the model oracle, the read paths held to each other, the format's damage cases |

## Status

A prototype. The open gaps are the ones listed above, each carried as a claim.

## License

MIT. See `LICENSE`.
