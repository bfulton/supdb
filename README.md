# Supdb

An ingest-optimized embedded key-multivalue store in Rust, and the evidence for
and against it.

This repository holds two things that are meant to stay together: the engine,
and a benchmark suite whose job is to **try to falsify** the claims made about
it. The engine came from a design document; the suite came from reviewing that
document and asking what measurement would prove it wrong.

```
src/               the original engine, vendored from the design artifact
src/next.rs        the next engine -- WAL, memtable, sealed segments, compaction, deletes, Txn
src/blob.rs        the read path over any byte source; compiles for wasm
src/bench/         the measurement substrate  -- repetition, significance, latency, I/O accounting
src/bin/internal   the falsification suite    -- Supdb against itself, as it scales
src/bin/correctness  the correctness suite    -- damaged files, a model oracle, crash injection
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

CI calls the same script with the same group names, so what passes here is
what passes there. That is deliberate: the checks used to be written down
twice, once in a contributor's habits and once in the workflow, and the two
had drifted far enough that CI never built the wasm module at all.

```sh
cargo build --release --workspace
cargo run --release --bin internal -- all --profile dev      # falsification suite
cargo run --release --bin external -- all --profile dev      # against redb, LMDB, sled
cargo run --release --bin correctness -- all --profile dev   # damage, oracle, crashes
cargo run --release --bin verify                             # claims vs measurements
cargo run --release --bin figures                            # results/ -> figures/*.svg
```

## What the measurements currently say

Every number below is `--profile full`, which is the only profile this project
treats as citable, and every one of them is read out of `results/` rather than
remembered. There are two engines here: the original `Store`, and `src/next.rs`,
which is where the recent work has gone. Comparisons are **matched** — an engine
is not ranked against another until they promise the same thing about durability,
transactions and checksums, and `Features::unmatched` refuses the ranking when
they do not.

**What holds.** The next engine reads **2.14×** faster than LMDB with both
committing durably per batch and both transactional (1,913,379 against 895,249
reads/s, `EXT.23`), and **6.97×** faster than RocksDB tuned as it would be
deployed (`EXT.33`). Its ordered scan ties LMDB (0.992×, no difference,
`EXT.24`) and beats tuned RocksDB by **5.10×** (`EXT.34`). On YCSB, matched, it
leads tuned RocksDB on update-heavy A by 1.74×, read-only C by 2.45×, short-scan
E by 1.28× and read-modify-write F by 2.20× (`EXT.42`–`EXT.45`).

Arrival order decides the load, and both halves are recorded: under **shuffled**
arrival the next engine loads at **5.93×** LMDB (`EXT.27`), because a durable
commit of a thousand random keys dirties about as many B-tree leaf pages and the
fsync writes them all. Quote that only alongside the ordered load below.

The original `Store` reads **1.52×** LMDB and scans **1.25×** with checksums
equalized (`EXT.11`, `EXT.12`). Durability has a usable point on its curve since
block compression went off by default: a 20,000-op window sustains **199,308
ops/s** with about 2 MB at risk (`F4.2`). Reader open is no longer proportional
to key count (`F2.2`), and the key index is 89 bytes per key in a mapped section
every reader process shares rather than 131 bytes duplicated per process
(`F7.2`).

Correctness is where the suite has earned the most. The store agrees with a
`BTreeMap` model across randomized appends, replaces and deletes (`C2.1`);
damaged files error rather than panic or serve wrong bytes (`C1.3`); a segment's
key index is checksummed per 16 KiB piece, and `tests/segwriter.rs` flips every
seventh byte of one and requires each flip to fail the open. Crash injection
against the next engine shows every recovered state is an exact prefix of the
commit order, and recovery invents nothing (`C4.1`–`C4.5`).

**What does not.** These are recorded as failing on purpose; each is
load-bearing evidence, and `claims.json` fails the build in both directions so
none can be quietly forgotten.

| finding | measured |
|---|---|
| the next engine loads faster than LMDB, both durable | **0.755×** — 463,695 against 613,821 ops/s (`EXT.22`) |
| the next engine loads faster than tuned RocksDB | **0.611×** (`EXT.32`); RocksDB also keeps the smaller file |
| reader open is independent of key count | not independent: 100× the keys costs **20×** the open (`F2.1`) |
| write throughput scales with threads | 4 threads is **0.93×** of one — the appender mutex (`F6.1`) |
| a 1,000-op durability window is affordable | **25×** throughput; `checkpoint` rewrites the whole key index (`F4.1`) |
| the mean summarises append cost | p99.9/mean = **19.2×**; inline `merge_key` holds two locks (`F5.1`) |
| Supdb stores the same data in less space than LMDB | **187.6 MB against 126.9** (0.68×, `EXT.6`) — traded knowingly for scans |
| reads survive the dataset outgrowing memory | **916×** degradation, 338,681 reads/s resident against 370 (`F1.2`) |
| a store killed before its first checkpoint is readable | it is not; nothing reaches disk until a checkpoint (`C3.4`) |

The size result is a deliberate trade, not a regression: turning block
compression off cost the space axis and bought the scan axis, and both entries
say so. The ordered-load deficit is the one that is genuinely open — what
remains is the per-batch append, fsync and section work against LMDB's single
page-chain commit, and the macOS `F_FULLFSYNC` pair says the floor there is the
fsync count itself.

`CLAUDE.md` carries the full scorecard, including which numbers have replicated
and on which host; `docs/next-engine.md` is the next engine's own account.

## Defects found and fixed

The three below were the first, and none is reachable by any benchmark in the
original design, because none of those compares the store against an independent
model or feeds it damaged bytes. A dozen more have been found since — a delete
that was never marked dirty, a logged durability point that could be lost whole,
replay applying records over newer state, a freelist class that underflowed for
every block of 4 KiB or less. `CLAUDE.md` lists them with the reasoning; the
reproducers live in `tests/known_bugs.rs`.

**A deleted key came back.** `append` calls `seal_shard` inline once a shard's
buffer fills, staging the extent in the block builder — but the block has no id
yet, so `entry.extents` is not updated until `flush_builder` runs. `delete`
cleared `entry.extents` and knew nothing about the staged member, so
`flush_builder` pushed it back and the key returned with every value it had.
*Fixed*: `delete` and `put` drop matching entries from `Shard::members`.

**A freed slot was handed out while still referenced.** Two causes.
`Appender::release` used a saturating guard — decrement if positive, then free
if zero — which did not prevent a double release, it *guaranteed* one: a second
call on an already-freed block skipped the decrement and fell straight into the
free-list push. And `seal_shard` released a replaced key's superseded blocks
without clearing `entry.extents`, so the index kept naming blocks whose
refcount had reached zero. Instrumentation showed three block ids describing
the identical range `[300643..319075)`, one still holding 71 live references.
*Fixed*: an unbalanced release is refused and asserts in debug builds; the
index reference is dropped in the same breath as the release.

**Damaged bytes were served as data, or killed the process.** `get_uvarint`
read past the end of its buffer and shifted without bound; `emit` sliced on a
length it had just read; extent block ids indexed `self.blocks` unchecked.
*Fixed*: every length is validated against the bytes remaining, block ids are
validated once at index-build time, and the file now carries checksums —
CRC-32C per chunk in the chunk directory so a point read verifies only what it
decodes, plus a whole-block CRC for blocks stored verbatim.

Reproducers live in `tests/known_bugs.rs` and stay there after the fix: a test
written from the failure is worth more than one written from the patch.

## What the fixes cost

Checksums are not free in principle, so the cost is measured — **interleaved in
one process**, both arms round-robin, because the obvious approach of running
the suite before and after and subtracting does not work. When that was tried
here, the three *unchanged* comparators in the external suite moved by +20% to
+43% between the two runs. Most of the apparent improvement was the machine.

| axis | cost | significant? |
|---|---|---|
| write throughput | **+8.5%** | yes (p = 0.0022) |
| read throughput | −0.9% | no (p = 0.37) — free |
| stored size | +0.166% | yes — and drift-immune |

Reads are free because verification happens at chunk granularity: a point read
hashes only the chunk it decodes, which is the reason the chunking exists.
Writes pay 8.5% because every block is hashed once on the way out. That is a
real cost on the axis the design is built to win, and it is the price of not
returning silently wrong data — a trade worth making, but worth stating.

Note the `dev` profile put the write cost at +3.0% and called it insignificant
(p = 0.21). It was underpowered, not free; only the `full` profile has variance
tight enough (rel IQR 2.3%) to resolve the effect. That is why `full` is the
only profile this project treats as citable.

A first attempt used a byte-at-a-time CRC-32 (IEEE) table and cost **13–35% of
write throughput**, which is not a reasonable price on the axis this design is
built to win. Replacing it with hardware CRC-32C on x86-64 — with a portable
slice-by-8 fallback, and a test asserting the two agree — brought it into the
noise. `results/baseline/` keeps the pre-fix measurements so this is checkable
rather than asserted.

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

Still a prototype, and the gaps are named rather than implied. The two this
section used to name are closed: `Store::open` exists, and every block carries a
checksum — CRC-32C per chunk in the chunk directory, plus a whole-block CRC for
blocks stored verbatim, which is what the section above measures the cost of.

What is open, in the order it matters: the durable ordered load trails LMDB and
RocksDB (`EXT.22`, `EXT.32`); `checkpoint` is O(key count) rather than O(what
changed), which is why the 1,000-op durability window still costs 25×; write
throughput does not scale with writer threads; and a reopened store declares
history before the reopen broken, because `Store::open` does not carry the reuse
log across. Each is a claim in `claims.json` with the state it is expected to be
in, so none of them can improve or decay without turning the build red.

`docs/architecture-review.md` is the full account, and `docs/next-engine.md` the
next engine's.
