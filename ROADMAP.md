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

- [ ] **Tune the buffered RocksDB arm.** `rocksdb-nosync` is
  `Rocks::create(path, false)`, which runs RocksDB on defaults: no Bloom
  filter, the default block cache, default write buffers. The durable arm
  gets the full tuning. Done when the buffered arm uses `rocksdb-tuned`'s
  options with `sync = false`, and a row is banked with it. Until then, no
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

## Housekeeping

- [ ] **Verify the jump of 2026-10-19.** supdb's engine grew 4.5K lines in
  one day (23.5K to 28K). Confirm it is real code before quoting 28K.

## Proposed out of scope

Complexity RocksDB carries for its own institution's needs. Not decided:
confirm or move any of these up.

- Concurrent writers with conflict detection (RocksDB's `TransactionDB`)
- Column families
- Wide columns, user-defined timestamps
- TTL, compaction filters
- Encryption at rest, tracing and replay
