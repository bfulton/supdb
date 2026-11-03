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

- [x] **Tune the buffered RocksDB arm.** `rocksdb-nosync` ran RocksDB on
  defaults (no Bloom filter, the default block cache, default write
  buffers) while the durable arm got the full tuning. The roster's
  buffered RocksDB is `rocksdb-tuned-nosync`, `rocksdb-tuned` with the
  sync alone off, and the rows banked from 2026-10-31 carry it. A buffered
  RocksDB number is quoted from those rows and never from `rocksdb-nosync`.
- [ ] **A recovery workload.** Crash at a known point (kill the process
  after N committed batches, with an uncommitted tail), then time the open
  and the first read, for every arm. Done when it runs in the suite and
  banks a row. An accepted loss against LMDB; the point is to measure it.
- [ ] **Rows past memory.** The `full` ladder to at least 1.5x the machine's
  memory, as `bench/DESIGN.md` specifies, is banked to 3M keys. The bounded
  scale (`--scale ooc`) is banked through 10M keys under 1.5 GiB, where
  LMDB's store fits the bound and supdb's raw form does not, and supdb
  trails every read cell there while leading the writes. Its roster runs
  the defaults, and the compact form the controller builds out of core
  (`ZPages::Auto`) was off by default until its in-core cost was named,
  so the rows banked before then measure the raw form under `supdb` and
  the controller under `supdb-autozpages`.
  The lag points' first scan there paid for the burst the upkeep thread
  had been lent and not filed; the thread files a burst's batches now
  (`Options::upkeep_files_bursts`), and what the divided pass still
  weights is the thread's phase at the burst's end and the full
  rewrite's pieces (`docs/engine.md`, the pass after a burst). The
  controller arm is in the roster now, and under a bound both engines
  leave core at -- 1 GiB at ten million keys -- it reads and scans
  several times LMDB's rate (`docs/engine.md`, the shift to the compact
  form at ten million keys); a row under that bound is the one to bank
  next, and `ZPages::Auto` as the engine's default is the decision after
  it, once the in-core pairing of the two reads level. The bound such a
  row can run under is set by the process's anonymous memory at the full
  rewrite and not by the engine's live bytes. The memtables' blocks the
  allocator kept are mapped from the kernel now (`Options::blocks_mapped`)
  and the contents-counted cache arm is out of the roster
  (`bench/DESIGN.md`); what still kills the roster under 1.25 GiB is the
  one process its arms share, which keeps 20-30 MB of allocator
  fragments past each store's trim, 0.3 GB by the fifth arm of a rep,
  under the controller arm's own 0.9 GB at the full rewrite, a third of
  it forms shed into free lists that nothing returns while the store
  runs. The runner gives each arm's rep a process of its own now
  (`Plan::fresh_process`, the bounded scale's default), and the roster's
  row under 1.25 GiB is banked (`bench/runs/ooc/`, at `5bf1f88`, by
  `sh bench/scripts/ooc.sh 1342177280`). Where both engines leave core
  at ten million keys the controller arm reads level with LMDB's
  readahead-off configuration on one thread and 1.7x on three, scans
  1.3x, the read-heavy mixes 1.3-2x and the update mixes 4x, at a tenth
  of its major faults a read, while the raw form reads 0.2-0.5x. What
  the row leaves: `ZPages::Auto` as the engine's default once its
  in-core cost on the point-read mixes is named; the lag points under
  the bound, where the controller arm's ten-percent point reads 0.3x and
  every arm's full-rewrite point 0.03x, the latter the merge's streaming
  of a store rewritten whole; the engine's trimmer running while the
  store is under a cgroup bound, where free memory is page cache the
  store does not have (`Options::forms_to_fit`'s trim, priced in core
  before it is the default under a bound); and 1 GiB.
- [ ] **Match the checksum work, or state it.** RocksDB reads with
  `verify_checksums(false)`, under which it verifies no data block it
  reads, from the file or into its cache. supdb verifies a segment's key
  section against its checksum row when it opens the segment, and a value
  block once per reader. In the suite's shape neither checks a data block
  on the read path: every supdb value is inline, and the `quick` row's
  RocksDB reads a memtable (`bench/DESIGN.md`, Arms). Either RocksDB reads
  with its default, `verify_checksums(true)`, or the methods say what each
  engine verifies.
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
- [x] **Whether `Auto` ships on.** It does: `ZPages::Auto` is the default
  and the raw form alone is the arm (`supdb-nozpages`), in both rosters
  in the controller arm's place. What the controller cost a read in
  memory was named by count and not by the pairings that had read it as
  5-28% on the point-read mixes in three sittings of four: the census
  hooks' flag said the controller was on where their work ran only
  under the census gate, so every read paid the call and the window's
  copy out of its cell to be sent back, and the form dispatch its
  out-of-line gate check beside them, 124 instructions of a read's 830
  by callgrind. The hooks test the state they work on now and the
  dispatch asks the gate inline, 12 instructions a read, and the
  pairings against the raw form read level in both orders at a hundred
  thousand, three hundred thousand and three million keys
  (`docs/engine.md`, the controller; `CLAUDE.md`, a hook priced behind
  its flag). The in-core rows banked after it carry the controller
  under `supdb`, as the bounded rows do, and the first of each is a
  definition step banked on its controls (`bench/CLAUDE.md`).

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
