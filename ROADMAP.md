# Supdb roadmap

What supdb needs, from a feature comparison against RocksDB and LMDB and a
review of the benchmark suite (2026-10-17 to 10-20).

Each item says why it matters and when it is done.

## Accepted losses

Rows supdb is expected to lose, measured and published anyway. Losing them
is the cost of the design, and showing them is what makes the wins credible.

- **Storage size.** 1.31 bytes on disk per byte stored, against LMDB's 1.15
  and RocksDB's 1.03 (2026-10-17 run). Measured already.
- **Recovery after a crash.** LMDB never recovers: its file is always
  consistent, so it opens instantly. supdb replays its WAL on open. Not
  measured yet; see the recovery workload below.

## Benchmark

The suite has to be fair before any of its numbers are quoted.

- [ ] **Tune the buffered RocksDB arm.** `rocksdb-nosync` ran RocksDB on
  defaults (no Bloom filter, the default block cache, default write
  buffers) while the durable arm got the full tuning. The roster's
  buffered RocksDB is `rocksdb-tuned-nosync` now, `rocksdb-tuned` with the
  sync alone off. Done when a row is banked with it. Until then, no
  buffered RocksDB number is quoted.
- [ ] **A recovery workload.** Crash at a known point (kill the process
  after N committed batches, with an uncommitted tail), then time the open
  and the first read, for every arm. Done when it runs in the suite and
  banks a row. An accepted loss against LMDB; the point is to measure it.
- [ ] **Rows past memory.** The `full` ladder to at least 1.5x the machine's
  memory, as `bench/DESIGN.md` specifies; nothing is banked past 3M keys.
  In progress. The supdb README already says reads "fall off a cliff" there.
- [ ] **State the checksum asymmetry in the methods.** RocksDB reads with
  `verify_checksums(false)` (its cached blocks were checked when loaded);
  supdb verifies every piece once per reader. Roughly matched; say so.
- [ ] **Check the 2026-10-17 run's other flags.** The buffered 1% burst at
  1M keys (185K updates/s, below supdb's own durable rate); buffered
  "device bytes written" equalling bytes on disk exactly, which may not be
  measuring the device; the duplicated YCSB tables in the figures.

## Engine

What an engine picker would hit, roughly in order.

- [ ] **A stable on-disk format.** The README calls supdb a prototype whose
  format is not yet stable. Done when there is a format version and a
  stated compatibility promise (what a newer release will still open).
- [ ] **A process lock.** No lock against a second process opening the same
  store was found. Confirm first; if so, two writers in two processes could
  corrupt it. Done when a second open fails cleanly (or readers in other
  processes are supported deliberately).
- [ ] **Backup or checkpoint.** A consistent copy of a live store, like
  LMDB's `mdb_env_copy` or RocksDB's `Checkpoint`. Table stakes for
  operating a store.
- [ ] **Range delete.** Deleting a key range without a tombstone per key.
- [ ] **Reverse and bounded scans.** `scan` is forward-only from a start key
  with a count; add an end bound and reverse order.
- [ ] **Batched point reads (`MultiGet`).** Cheap to add, and a natural new
  benchmark workload.

## Out of core

Where the compact form under `Auto` stands against LMDB with its readahead
off (`docs/engine.md`, "Out of core: the second form").

- [ ] **The read pass's first seconds.** At five million keys under 640 MiB
  the single-threaded read pass reads level with LMDB and its p99 trails:
  the pass starts at the load's end, and for its first seconds the reads go
  to the raw form, before the compact form installs. Done when the pass
  reads ahead in both orders of the pairing.
- [ ] **The ordered load beside the builds ahead.** Building form files
  during the load costs the ordered load about a tenth at three and five
  million keys under 640 MiB. Done when it reads level with the controller
  without the builds ahead, or the trade is priced and stated.
- [ ] **The writer's reads in the census.** Counting them while the
  controller decides read the A and F mixes slower in two sittings,
  unresolved; a store that stays undecided pays it. Done when resolved.
- [ ] **What is not priced yet.** Ten million keys under a gibibyte with
  the builds ahead, the lag sweep under a bound, and a shuffled load
  followed by reads. Done when each is paired against LMDB with its
  readahead off.
- [ ] **Whether `Auto` ships on.** The compact form is off by default
  (`ZPages::Off`) and `Auto` is an arm. Done when `Auto` is priced in
  memory against the default with no cell given up, and the default is
  decided on that.

## Housekeeping

- [x] **Verify the jump of 2026-10-19.** It is real code: the compact
  form's merges of 2026-10-18 and -19, its layout (`src/db/zpages.rs`),
  its selector (`src/db/zselect.rs`) and the forced form (`src/db/zside.rs`,
  since replaced by `src/db/zfile.rs`), each with its tests. A line count
  is quoted from the tree it was taken at.

## Proposed out of scope

Complexity RocksDB carries for its own institution's needs. Not decided:
confirm or move any of these up.

- Concurrent writers with conflict detection (RocksDB's `TransactionDB`)
- Column families
- Wide columns, user-defined timestamps
- TTL, compaction filters
- Encryption at rest, tracing and replay
