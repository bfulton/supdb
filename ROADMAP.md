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
  consistent, so it opens instantly. supdb replays its WAL on open, and
  walks a run of ordered ingest the crash left open to write the index
  its close would have written. Measured by the `recover` workloads
  (`bench/DESIGN.md`).

## Benchmark

The suite has to be fair before any of its numbers are quoted.

- [x] **Tune the buffered RocksDB arm.** `rocksdb-nosync` ran RocksDB on
  defaults (no Bloom filter, the default block cache, default write
  buffers) while the durable arm got the full tuning. The roster's
  buffered RocksDB is `rocksdb-tuned-nosync`, `rocksdb-tuned` with the
  sync alone off, and the rows banked from 2026-10-31 carry it. A buffered
  RocksDB number is quoted from those rows and never from `rocksdb-nosync`.
- [x] **A recovery workload.** Crash at a known point (kill the process
  after N committed batches, with an uncommitted tail), then time the open
  and the first read, for every arm. `recover` and `recover-shuffled` do,
  after the ordered load's batches and after the shuffled load's, in the
  in-core rows (`bench/DESIGN.md`). An accepted loss against LMDB; the
  point is to measure it.
- [x] **Match what a buffered commit survives, or state it.** The
  recovery workloads hold every arm's reopened store to the batches its
  writer was acknowledged, and the buffered supdb arm's ordered load did
  not hold the newest of them: ordered ingest wrote a run's records a
  write piece at a time, so under a policy that does not sync at every
  commit the commits behind the last whole piece were in the writer's
  memory when its process died. Every commit of a run is written to its
  file now, as a commit is written to the WAL, and the rows' count of
  acknowledged batches not recovered (`unrecovered_batches`) is zero for
  every arm.
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
  every arm's full-rewrite point 0.03x -- half of the full rewrite's
  pass was the first scan ranking every piece of the burst whole against
  its partition, and the first scan after the merge's landing ranking
  them again, which nobody does now (`Options::piece_ranks_whole`, off;
  `docs/engine.md`, under "against LMDB with its readahead off"), and
  the rest is the merge streaming the rewritten store twice through a
  cache that cannot hold it, beside scans that found their keys' pages
  in the burst's pieces one fault at a time, which a build asks for
  together now (`Options::pieces_willneed`, the pass 1.5x for it), and
  the partition's own pages a raw scan takes are asked for the same way
  (`Options::blocks_willneed`), the drained point level with LMDB for it
  and the full-rewrite point at 0.12x; the settle's resolves ask for the
  heads they will seek ahead of them (`Options::heads_willneed`: runs
  where the batch is dense in a partition, a stride a key ahead where it
  is sparse, and nothing at a block's build, whose page of heads faults
  beside the records' ask already), the one-percent point's first scan
  0.6x of what it was and its cell 1.46x; a merge's landing plans the next
  merge while reads are around, at the base trigger, and a merge takes its
  due ranges up to a cap of partition bytes (`Options::merge_chains`,
  `Options::merge_bytes_cap`), since the planner ran at the seals alone and
  the full rewrite's pass met the burst's 336 pieces for the whole of it
  with one merge over every range in flight -- the pass 6.9 s against
  10.0-10.7, its cell 1.33-1.39x in two sittings, the burst level, and
  against LMDB at that head the drained pass 1.3x, the one-percent pass
  level, the ten-percent pass 0.35x and the full rewrite 0.17x, with the
  weighted cells 1.4-4.5x ours since LMDB's bursts run at 14k updates a
  second under the bound; out of core a block's build asks for its pieces'
  directory words and a bounds walk for a piece's heads before reading
  them (`Options::pieces_dir_willneed`, `Options::bounds_heads_willneed`,
  the full rewrite's faults 30% fewer, its pass 2-4%), and the splice
  bounds read off the ordered index's top level were refuted, the builder
  they freed costing the reader its installs
  (`Options::boundary_keys_from_top`, off), and the builder ahead starts in
  core alone, since out of core it built every block a burst dirtied from
  cold pages (`Options::ahead_out_of_core`, off; the ten-percent pass's
  faults halved and its time 0.88x in two sittings); the merge's first pass reads its
  keys off the ordered index's heads now (`Options::merge_keys_by_heads`,
  the bursts' background reads an operation 1.6-1.75x fewer, the cells
  within the spread), and what is left there is the merge's second pass
  streaming its inputs through a cache that cannot hold them -- dropping
  those pages behind the merge was tried and read slower at every point,
  since the scans beside it read the same pieces until the landing
  (`Options::merge_drop_behind`, off) -- and the compact form's shorter
  pages once the row's read passes have installed it; the engine's
  trimmer running while the
  store is under a cgroup bound, where free memory is page cache the
  store does not have (`Options::forms_to_fit`'s trim, priced in core
  before it is the default under a bound); and 1 GiB.
- [x] **An arm at FASTER's promise.** `faster` is in the roster with a
  guarantee of its own, in memory until a checkpoint, and had nothing to
  be compared with there: the buffered arms hand every commit to the
  kernel and FASTER hands it to nobody. `supdb-heldnoframes` is the
  matched arm, opened under the one policy of the engine's that promises
  what FASTER does (`SyncPolicy::Explicit`): a commit is in the process
  until a sync. It is an arm for that comparison and not a default. It
  sits in FASTER's group in the roster, `supdb-heldpart` in the bounded
  one, and the figures draw the group (`bench/DESIGN.md`, Arms).
- [x] **The threaded mixes in a row, or not.** `threads-A` and `threads-RMW`
  are the workloads FASTER is picked for, gets and puts from several
  threads at once. A row in core carries them for the arms of FASTER's
  guarantee, a store's worth of operations a thread, on one thread and
  each thread count; for every arm that is hours more a full row, so the
  rest run them only where `--passes mixes-threaded` asks
  (`bench/DESIGN.md`).
- [ ] **A `full` row and a bounded row with the held log's changes.**
  Handles that write, a burst's batches lent under the held log, a
  buffered burst filed by its own commits and the settle's lean shapes
  are banked in quick rows alone, to three hundred thousand keys. No
  `full` row and no bounded row has been taken with them, and under a
  bound the settle was paired once, at three million keys under 1 GiB,
  where nothing resolved. Done when a `full` row and a bounded row at a
  head that has them are banked, and the cells of FASTER's group and the
  lag sweep's in each are read against the rows before.
- [ ] **Match the checksum work, or state it.** RocksDB reads with
  `verify_checksums(false)`, under which it verifies no data block it
  reads, from the file or into its cache. supdb verifies a segment's key
  section against its checksum row when it opens the segment, and a value
  block once per reader. In the suite's shape neither checks a data block
  on the read path: every supdb value is inline, and the `quick` row's
  RocksDB reads a memtable (`bench/DESIGN.md`, Arms). Either RocksDB reads
  with its default, `verify_checksums(true)`, or the methods say what each
  engine verifies.
- [ ] **FASTER's rates between two bounded rows.** At three million keys
  under 1 GiB FASTER's point mixes read 30% to 57% higher in the row at
  `33bc663` than in the row at `001781d` (`bench/runs/killed/ooc`), with
  LMDB level to a quarter higher, the engine's default arm level and its
  held arm a tenth lower, so the held arm's ratio to FASTER there moved
  mostly by FASTER. Done when the two rows' FASTER arms are held to one
  configuration, its sizes under the bound among it
  (`engines::faster_sizes`), or the difference is laid to the host.
- [ ] **Check the 2026-10-17 run's other flags.** The buffered 1% burst at
  1M keys (185K updates/s, below supdb's own durable rate); buffered
  "device bytes written" equalling bytes on disk exactly, which may not be
  measuring the device; the duplicated YCSB tables in the figures.

## Engine

What an engine picker would hit, roughly in order.

- [x] **A stable on-disk format.** A store records its format version in
  every manifest (`STORE_FORMAT`), and a release opens the versions from
  `STORE_FORMAT_OLDEST` to its own and refuses any other at open, naming
  both, before it changes the directory (the README's Status). A test holds
  the version to the files' magics, so a magic cannot move without it.
- [x] **A process lock.** There was none: two processes could open one store
  and write beside each other. A store's `LOCK` is held under an exclusive
  `flock` for the store's life, and a second open, from another process or
  this one, fails at once and touches nothing (`tests/db.rs`). Readers in
  other processes are not supported.
- [x] **A run's temp name behind a directory barrier.** A run of ordered
  ingest was acknowledged durable once its file was synced, and nothing
  synced the directory for the temp name that file is under: the name rode
  the journal commit a new file's first sync forces, which ext4 and XFS
  give and POSIX does not promise. The first sync that acknowledges a
  commit of a run fsyncs the directory once, after the file, as a
  rotation does for its new WAL file (`Db::sync_run`; `tests/runlog.rs`
  counts it on every path that syncs). A store whose commits never sync
  makes none.
- [x] **A synced run that is lost anyway is refused where anything
  follows it.** With its file synced and its name behind a barrier, a run
  goes missing only where the device loses what it was told to keep.
  Frames in the log past a run that is not there are a sequence gap, and
  the open refuses. A run written after the missing one says in its note
  whether everything before it was synced when it began, as a WAL file's
  kind says, and where it was the open refuses too, before it removes
  anything (`tests/runlog.rs`). A run that was the last thing written is
  gone with nothing said, as a log's last synced record would be: nothing
  on disk stands after it to say so.
- [ ] **A scan's limit counts the keys it answers, and a page says where
  the next one starts.** `scan(from, limit, f)` counts every key it
  visits, a deleted key among them for as long as any part of the store
  still records the delete. With eight of a store's first sixteen keys
  deleted and unsealed, a scan of ten answers ten and hands over the
  values of five; with those merged away and four of the keys after them
  deleted, of six, unsealed or sealed in a piece; and of ten once a
  flush's merge has dropped the deletes (sixty keys put, the deletes,
  `seal`, `flush`, with partitions and without, at `e577753`). So
  what a limit buys depends on what the store has merged: a page comes
  back short, or empty, with keys still ahead of it, a caller that reads
  "fewer than the limit" as the end stops early, and a page of exactly the
  limit says nothing of whether another follows. The limit is to count the
  keys that hold a value, a key deleted and not written again passed over
  whichever part of the store records the delete, so a scan answers
  `limit` keys wherever that many stand at or above `from` and fewer only
  at the store's end. And a paged call, `scan_page(from, limit, f)`,
  answers where the next page starts, the first key after the page that
  holds a value, and nothing at the store's end, which a store whose keys
  are a whole number of pages needs as much as one with deletes; it is a
  scan of one key more than the page, whose values are read and not handed
  over. Three things the change has to carry. The merge paths and the
  overlay's walks count a key where a value of it is handed over. The
  block path gives each source a window of keys as long as the limit
  (`overlay_window`, `overlay_mem_window`, and a wide block's under
  `Options::scan_wide_capped`), and once a deleted key counts for nothing a
  walk can spend its whole window under the limit: it stops there and the
  scan goes on from the key after the window's last. And a partition may
  hold a record with no value: a merge that writes the bottom level drops
  the deletes, a promotion links a piece as it stands (`Seg::tombs`;
  `promote_ranges`, `promote_unpartitioned`), and the walks that take a
  partition's records in bulk count records without asking whether one
  holds a value. Done when a scan and a page hold to
  the model in every state of the store, the scan model and the
  differential fuzz counting keys with values and the fuzz paging a store
  from end to end, with the deletes unsealed, in a piece, and in a
  partition a promotion made; when the README shows the paged call; and
  when the scan pass and ycsb-E are priced against the count before in
  both orders, since the count is in the scan's innermost loops. Open for
  the design: whether no partition may hold a record without a value, a
  promotion leaving such a piece to the merge and a store from before
  checked or rewritten once at its first open under a new store format
  version, or whether every record walk says which records hold none and
  the bulk walks stop for them, with no format change.
- [ ] **A buffered burst of one commit pays the filing in that commit.** A
  commit near reads files its batch (`Options::forms_settle_every`), and
  where a burst is one commit the filing is in the burst: the buffered
  arm's one-percent burst at thirty thousand keys runs at two thirds to
  four fifths of the arm that files late, behind in thirty-one pairs of
  thirty-two across both orders (`bench/scripts/pair.sh
  supdb-ingestsettlelate supdb-ingest 30000 --passes load,lag --reps 16`
  at `8eb02b6`). Since that commit stopped building what it filed into
  (`forms_settle_builds_filed` off) the pass after it makes the
  difference back, the point's weighted sum level in both sittings, and
  the quick row at `0526b89` has the burst ahead of both comparators. The
  ten-percent burst at ten thousand keys, behind in the same pairing at
  `5df99d6`, is unresolved by it now and sits level with LMDB's without
  sync in the rows. What the filing costs a burst of one commit is not
  decomposed. The held log's arm has the cell too: its one-percent burst
  at ten thousand keys, one batch of a hundred, read 0.71x of FASTER's in
  the quick row at `9453537`, the one burst cell of twelve behind.
- [ ] **The forms shared between the writer's scans and its upkeep
  thread.** The writer's block tables and scan snapshot are lent whole to
  one thread at a time (`CLAUDE.md`, the writer's upkeep is lent, never
  shared), and a scan through the writer takes them home: it waits out the
  thread's pass in flight and files everything the thread trails by before
  it walks its first block. Filing a write into its block's form costs
  several times the write on whichever thread pays (`docs/engine.md`, what
  filing a write costs), so one thread files at about half the writer's
  rate, and a store has a burst at the writer's rate or the scan pass after
  it at the filed store's, not both. The engine picks by policy today: a
  buffered commit files its burst itself, a syncing one lends it under a
  pace, and only the held log lends it outright
  (`Options::held_bursts_lend`). There the bursts run 1.7x to 3.4x of the
  filed store's and the scan pass straight after a full rewrite at 0.15x
  to 0.35x of it, the first scan's wait and settle 64 ms of a pass of 74
  at three hundred thousand keys (`bench/scripts/pair.sh
  supdb-heldnoframes supdb-heldfiles 300000 --passes
  load,read,mixes,mixes-scrambled,lag --reps 5` at `3f4f23f`). What is
  left in the filing itself is a flat tail, none of it the factor. The
  lever is the scan: going on without the whole trail filed, settling the
  blocks it walks as it reaches them, from the batch sorted once, while
  the thread files the rest beside it. Done when the pass after a lent
  burst reads level with the pass behind a filed one in both orders of
  that pairing with the bursts still at the lent rate; the buffered
  commit's own filing and the syncing commit's pace, which both exist to
  bound what the first scan meets, are each priced against lending beside
  it; the shape before is an arm; and the reader-thread tests and the
  differential fuzz hold it at every upkeep level. Open for the design:
  what the two threads share and in what unit it changes hands, a block
  or a partition's table, where one word hands the whole of it over today
  and nothing in it is `Sync`; what a scan must have filed before it may
  walk a block, the keys created since its snapshot among it; and what a
  pass may publish to reader handles while the writer is walking.
- [ ] **The forms stood down through a stretch of point operations.**
  Under the held log the roster's arm trails FASTER on the point mixes
  one thread runs, 0.6x to 0.85x on A, B and F at a hundred thousand keys
  and 0.7x to 0.8x on A and F at three hundred thousand, timed by kind in
  the row's order (`docs/engine.md`, the point mixes on one thread, by
  what is taken away; `bench/scripts/pair.sh SUPDB_MIX_KINDS=1
  supdb-heldnoframes faster 100000 --passes
  load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled
  --reps 12` at `02bab06`, and beside it `supdb-heldnfnocache` and
  `supdb-heldinplace`). Two of its three causes are one constraint. Once
  a scan has come the forms the scans run on are kept current beside
  every commit, and the writer waits for the lines the upkeep thread
  read: with no block cache A is within a tenth of FASTER and F has two
  fifths of its gap back. And a put appends where FASTER overwrites,
  because the thread reads the values an overwrite would change
  (`Options::put_in_place` refuses the block cache): with both gone A is
  ahead of FASTER at both rungs and F at 0.9x. The forms are what E, the
  scan passes and the lag passes run on, so they cannot be off for a
  store's life; they can be stood down while the workload is point
  operations, as they are while a shared table stands
  (`Reader::sharing`), and brought back when scans come. What that costs
  is the first scans after, which is why it waits on the item above. Done
  when the roster's arm reads A, B and F level with `supdb-heldinplace`
  in the row's order, in both orders at both rungs, with E, the scan
  passes and the lag passes no slower than the roster's arm has them now;
  and the differential fuzz and the handle tests hold an overwrite beside
  the stand-down and the way back. Open for the design: what says a
  stretch is point operations, where two stores' worth of writes since a
  scan says it today (`reads_around`), and what a handle claimed inside
  the stretch finds.
- [ ] **The upkeep thread's alertness beside point writes.** The thread
  stays alert while reads are around, so that a mix's small batches are
  filed before the scans that follow (`Options::upkeep_alert`). Under the
  held log, backed off, A read 1.17x to 1.24x in three sittings of four,
  the scrambled A 1.11x to 1.20x in all four and E, which the alert is
  for, 0.95x to 1.24x, unresolved; F did not move
  (`bench/scripts/pair.sh supdb-heldnoalert supdb-heldnoframes 100000
  --passes
  load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled
  --reps 30`, and `300000 --reps 12`, at `02bab06`). It is the part of the
  item above that needs nothing built. Done when E's price is read by
  count, its walks, its builds and its first scans, at ten, a hundred and
  three hundred thousand keys, and the alert is kept with that price
  stated or keyed on what E needs; and when what the block cache costs
  F's writes beside the alert, 229 ns against 158 without the cache, is
  laid to a cause.
- [ ] **The one writer's get, as short as the shared table's.** With the
  two above gone a get is what is left: on the clean store C reads it
  costs 164 ns against FASTER's 142 at a hundred thousand keys, in every
  arm, and at three hundred thousand, where C is level, F's gets are 250
  ns against 227 and F stays at 0.9x (`docs/engine.md`, the same
  section). The one writer's get is longer than FASTER's by its
  instructions, counted class by class (`docs/engine.md`, what the scans
  leave the writes), and the shared table's lean path, one record address
  a get, came within a sixth of FASTER's count (`CLAUDE.md`, equal
  instructions at two thirds the speed). Done when C at a hundred
  thousand keys reads level with FASTER in both orders of the row's
  order, and F's gets do, timed by kind, in `supdb-heldinplace`.
- [ ] **The threaded mixes at ten thousand keys.** In the quick row at
  `9453537` the roster's held arm is ahead of FASTER in 39 of the 48
  cells of the threaded mixes. Six of the nine behind are at ten thousand
  keys, 0.81x to 0.97x: `threads-A-uniform` on one, two and three
  threads, `threads-A` on two, and both read-modify-write mixes on one
  thread. The other three are within a hundredth of level. Nothing of it
  is decomposed; a cell there is a few milliseconds, and each thread's
  time and faults are printed (`SUPDB_THREADED_COUNTERS`). Done when
  those six read level or ahead in both orders of a pairing in the row's
  pass order, or what they pay is counted and stated.
- [ ] **A shared table left when its handles stop writing.** The table
  every handle writes is left for the one writer's only at one of the
  writer's own commits or seals (`Options::share_on_write`,
  `Db::share_look`). A store whose handles stop writing and whose writer
  then only reads keeps the shared form, and its scans merge their
  sources, a sixth of the block cache's rate on ycsb-E, until the writer
  commits; and a scan beside handles that write always merges
  (`CLAUDE.md`, Standing limitations). The levers named there are the
  segment work's thread leaving the table for an idle writer, which needs
  the freeze made safe from a second thread, and a form kept current from
  the shared table's own writes, which needs them logged. Done when a
  store that has stopped writing through handles scans at the block
  cache's rate without a commit of the writer's.
- [x] **Order ordered ingest's runs across a machine's crash.** Under a
  policy that does not sync at every commit (`SyncPolicy::EveryN`), a
  machine's crash could keep a later run, or frames in the log, and take
  the end of an earlier run no sync had covered, and the open served the
  later commits without the earlier ones, where the policy promises a tail
  lost whole. A run's records take sequence numbers now, the open walks
  the frames and the runs as one history and ends it at the first loss,
  and a sync by policy covers every file written before it (`CLAUDE.md`,
  a history in several files). Held by tests that build the state a lost
  page leaves (`tests/runlog.rs`); a rig that cuts the power is still the
  one thing that would show a device lying about its sync.
- [x] **Backup or checkpoint.** A consistent copy of a live store, like
  LMDB's `mdb_env_copy` or RocksDB's `Checkpoint`: `Db::checkpoint` makes
  the store as of the call a store of its own at a new path, its segments
  hard-linked where the path is on the store's filesystem and copied
  where it is not (`tests/checkpoint.rs`). It flushes first, as RocksDB's
  does by default; a checkpoint that leaves the memtable where it is and
  carries the log would be the cheaper one for a store that writes
  between them.
- [ ] **The first scan after a burst.** The scan pass after a burst pays
  its first scan for the burst's end: the wait for the upkeep thread's pass
  in flight, the log read, the settle of what the thread trailed by and the
  snapshot's extension, 0.4 to 14 ms at three million keys where the rest
  of a divided pass is 12 to 15, with a merge the idle plan starts at that
  scan beside it in a rep in four (`CLAUDE.md`, open targets). It is what
  the pass after a full rewrite at three million keys under 1 GiB has left
  against LMDB's, level to a tenth ahead where the same pass in memory is
  1.8x. Two levers are priced and neither chosen: the share the syncing
  pace lets the thread trail by (`Options::upkeep_pace_syncs_bp`), which
  gives the first scan back and takes an eighth to a third of the small
  bursts, and the idle plan's merge held off or run at the idle I/O class
  while the writer's forms cover the store and no point read is near, which
  is not built. Done when the first scan of the pass after each burst costs
  what the scans behind it do, in both orders of the pairing at three
  million keys in memory and under both bounds, with no burst slower for it.
- [ ] **The settle's cost a key.** Filing a write into its block's form is
  what the upkeep thread trails by, what the first scan after a burst
  settles and what a commit near reads pays: in the scrambled E a commit of
  a hundred inserts at twice LMDB's, 2,500 instructions an insert, and at
  ten thousand keys a first scan of 100 to 140 µs in a mix of 1.1 ms
  (`docs/engine.md`, timed by kind). The loop wants making again and not
  trimming: its sort for duplicates, the key read four times a write, a
  patch's allocations. Beside it, a scan that takes its upkeep back from a
  pass in its fill by a short spin where it parks, fifteen microseconds a
  hand-off. Done when E's commit is level with LMDB's and the scrambled E
  at ten and thirty thousand keys holds ahead in the rows by more than the
  twentieth it has, under both policies.
- [ ] **A light form's conversion with a trigger of its own.** The fill
  converts a form under the dense count only on a walk of the tables, and
  a walk starts where something else leaves them incomplete: a write into
  a block with no form, a form left at the dense count, a settle's
  conversion (`Options::settle_convert_fills`; `CLAUDE.md`, a step made
  earlier). A store whose every block holds a light form, with no batch
  taking one to the dense count, converts none, and its scans walk deltas.
  Done when the conversion is started by what it is about, a form past the
  line that the scans have read, priced against today's on the scrambled E
  at every rung and on the write mixes that pay for the walks, with the
  shape before as an arm.
- [ ] **The buffered arm's cells under LMDB without sync in the quick
  row.** The row at `33bc663` reads the buffered arm's scan pass at 0.93x
  to 0.96x at ten, thirty and three hundred thousand keys, its scrambled E
  at 0.89x at three hundred thousand and the pass after the full rewrite
  there at 0.88x. None of these is paired; the pairings of that binary at
  ten and a hundred thousand keys read the scan pass and E ahead in both
  orders. Done when each is paired in the row's order in both orders, and
  won or decomposed.
- [ ] **The seal's record path.** A seal hands the segment writer its
  records one at a time, about 830 instructions a key, and the shuffled
  load's drain at ten thousand keys is the seal's CPU: the durable
  shuffled load reads 0.94x of RocksDB's there in the row at `33bc663`. A
  direct run's commit was given its batch at once
  (`Options::direct_lean_append`); the seal's path was not. Done when the
  shuffled load at ten and thirty thousand keys is ahead of both
  comparators in both orders, and the durable ordered load's cells at ten
  and a hundred thousand keys, which the pairings read ahead and the row
  0.89x to 0.92x, are reproduced in the row's shape or laid to it.
- [ ] **Scans on a store with unsealed keys spread through it.** A few
  unsealed keys in a partition take every scan of it off the bulk walk and
  through the block loop and the form file's cursor, a fifth to a quarter
  slower than the same scans on the store folded, whatever form serves
  (`SUPDB_SCAN_MIXED_FOLD`). Over the suite's numbered keys the inserts
  land above every key and the stride form's lead covers the rest; over
  keys that fall anywhere it is the cell lost to LMDB wherever LMDB's file
  is in memory, the threaded scans after the mixes at ten million random
  ids, 0.86x to 0.91x under 1.25 GiB. The levers: the block loop's three
  loads a block, each behind the last; what the values' copies wait on in
  the cursor's walk, which is not known; and folding the keys in once a
  store is only read. Done when the scans after the mixes read level with
  the scan pass before them over ids and over numbered keys.
- [ ] **The index layouts the study found.** `docs/index-theory.md` has
  smaller and faster points on the frontier than the layout that ships.
  Done when one is built behind an option and priced in the suite's shape,
  or the study's point is refuted there.
- [ ] **Range delete.** Deleting a key range without a tombstone per key.
- [ ] **Reverse and bounded scans.** `scan` is forward-only from a start key
  with a count; add an end bound and reverse order.
- [ ] **Batched point reads (`MultiGet`).** Cheap to add, and a natural new
  benchmark workload.
- [ ] **A conditional write.** A put or delete that applies only if the key
  is still as its caller read it, and says whether it applied:
  compare-and-swap. A read-modify-write is atomic today only through the
  writer (a `Txn` borrows it, reads through it and commits as one batch),
  so threads that share a store either queue their reads behind the writer
  or read through a `Reader` and write later, where another thread's write
  to the key can land in between and be lost. It matters to an engine
  picker who has several threads and one counter, and to the suite: the
  `threads-RMW` workloads run FASTER's read-modify-write, which is atomic,
  beside a read and a later write for supdb, which is not. It buys
  correctness and costs speed: `put` looks nothing up in the store, so
  there is no second lookup to save, and the check is work inside the one
  writer. It is not concurrent writers with conflict detection, which
  stays under proposed out of scope: the writer is still one, and it
  applies or refuses. Done when the writer and a `Txn` take a conditional
  put and delete, a refused one leaves the store and its batch as they
  were, a test holds read-modify-writes from several threads over a few
  hot keys to no lost update, and `threads-RMW` runs supdb through it.
  Open for the design: whether the condition is the value or the position
  it was read at.

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
- [x] **What was not priced.** Ten million keys under a gibibyte with the
  builds ahead, the lag sweep under a bound, and a shuffled load followed
  by reads: the bounded rosters run three and ten million keys under 1 GiB
  and 1.25 GiB with the lag sweep and the shuffled load among their
  passes, against LMDB with its readahead off and RocksDB tuned
  (`bench/runs/ooc`, `bench/DESIGN.md`).
- [ ] **The pass after a full rewrite under a bound.** It pays for the
  burst's merge debt twice: as a merge writing beside it, in the reps
  where the chain has not drained, and as the working set the burst
  leaves, which under 1 GiB at ten million keys is about 1.2 GB against
  the bound: form files that do not fit, the pieces, and the memtables'
  unsealed keys, whose sort is the pass's first scan. There the pass over
  random ids reads 0.6x to 0.9x of LMDB and over numbered keys 1.1x to
  1.3x, a third of it user time, a quarter the kernel's fault handling and
  the rest device waits (`CLAUDE.md`, open targets). The idle I/O class
  for the merges and their inputs asked ahead were priced on it and
  refuted. Done when the pass over ids is ahead of LMDB in both orders
  under 1 GiB with the burst no slower, by the bytes of that working set
  or the merge path's user time over them.
- [ ] **A store past its budget that is only scanned.** The controller
  buys the compact form on its point reads' faults, and a sweep of bursts
  and scans has none. At ten million keys the held arm's lag sweep began
  with the compact form serving in seventeen sweeps of eighteen under 1 GiB
  and in three of six under 1.25 GiB, and where it began raw the one- and
  ten-percent bursts ran ten times longer, the writer held behind an
  upkeep thread that resolves its keys through raw records the bound has
  pushed out (`docs/engine.md`, the frozen tables' bytes). What decides the
  purchase before the sweep, the load's builds ahead or the budget as the
  load ends, has not been read off the controller's trace
  (`SUPDB_ZPAGES_TRACE=1` over the pairing). Done when it is, and the sweep
  begins on the form the budget calls for in every rep.
- [ ] **The builds ahead at the half-budget line.** The builds ahead at
  landing start where the store's raw bytes pass half its budget
  (`Options::zpages_land_pct`), and the budget is the limit less the
  process, so a store that ends its load on that line is bet on or not by
  what the process happens to hold: at three million keys under 1 GiB one
  or two builds ran through the first burst, twice its length, in a third
  of the reps before the jobs' arrays were mapped, which holds that store
  eighteen megabytes under. A store a tenth larger is on the line again.
  Done when the bet is called off where scans or commits come while the
  store is under the selector's line, or the line is drawn against a
  budget the engine's own transient memory does not move, and which
  syscall of a build stalls a commit beside it has been traced.
- [ ] **The crossing off the thread every landing goes through.** The
  one-thread read pass straight after a load under a bound reads in two
  shapes by when the load's last partitions land: at ten million keys
  under 1.25 GiB the last five land in one burst at the load's end in half
  the reps, and the crossing's third of a second on the segment work's
  thread runs with the reads already going, 1.4 to 1.7 s for a million
  reads against 0.9 to 1.1. Twice LMDB's rate in the long shape. Done when
  the crossing's asks are made off that thread and the pass reads one
  shape.
- [ ] **The full rewrite's burst at three million keys under 1 GiB.** The
  upkeep thread takes 8,000 to 46,000 major faults a burst building copies
  from raw records the cache had let go, the burst 6.9 s at the first
  count and 9.3 at the second, and the controller's sampler 2,800 to 4,300
  more, sampling at every landing for a purchase no read is near to make
  (`CLAUDE.md`, a reservation in a budget's arithmetic). Done when the
  burst takes the faults of its fast count in every rep, with the pass
  after it no slower.
- [ ] **The memos of decoded groups in slabs.** The memos keep their
  memory after their readers end, in arenas the allocator does not return,
  and where the block form misses its fit by that much the store faults
  for it. In slabs (`Options::zpages_memo_slab`) they return it, the
  scrambled mixes 1.19x to 1.85x at ten million random ids under 1 GiB
  before the heads placed the bounds, and the option is off: over numbered
  keys at that bound the slab has nothing to return, and the hot-raw
  policy's one release, asked into memory that stands unused at a look, is
  never asked under it. Done when that policy's room is drawn from
  something the slab does not move, and the slab is priced again beside
  the heads, six reps a sitting over numbered keys at both ten-million
  bounds, with the quick row and the gate.
- [ ] **Keys that are no run of numbers.** Three things are owed over
  random ids. The sparse stride form is priced where it fits, 1.5 GiB at
  ten million keys, and is not the default anywhere: its pricing over ids
  and numbered keys at the roster's bounds, and the decision. The block
  form serves ids wherever that form does not fit, and its three-thread
  scan pass at ten million ids under 1.25 GiB is 0.94x of LMDB's. And
  `supdb-nocache` was killed at ten million ids under 1 GiB in the lag
  sweep after its ten-percent point, the cause not read. Done when each
  has its pairing or its count, and the scan pass is ahead.
- [ ] **A budget read where the load ends.** The controller reads its
  budget as the load ends, the process at its smallest. A store whose raw
  form fits that budget by less than the process then grows, ten million
  random ids under 1.5 GiB by a hundredth, is served raw until its reads'
  faults have paid for the form, four cells of a divided row, and a form
  built ahead for it is dropped as it is written and read back under the
  first reads. Done when the budget counts what the engine's own
  structures are set to hold.
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
- [ ] **The crash seam's count, for the threads no hold reaches.** The
  tests' crash (`Db::crash`) returns once every thread of the store that
  touches its files has ended (`Life`), and the tests put each kind of
  thread a hold can reach in flight at a crash. Four kinds have no hold:
  a piece merge, the builder ahead, the sampler and a form file's warm.
  A mutant without the count of any one of them passes every test. Done
  when each has a hold or a seam that puts it in flight at a crash and
  its mutant fails.
- [ ] **Two states of the settle no test builds.** Of the thirty-nine
  mutants of the settle's lean shapes two pass: a frozen batch cut by a
  budget, which no test builds, and the cache's bind at the fill's
  check, which a staged patch's shed reaches first in every test. Done
  when a test builds each state and its mutant fails, or the path is
  shown to be unreachable and taken out.

## Proposed out of scope

Complexity RocksDB carries for its own institution's needs. Not decided:
confirm or move any of these up.

- Concurrent writers with conflict detection (RocksDB's `TransactionDB`)
- Column families
- Wide columns, user-defined timestamps
- TTL, compaction filters
- Encryption at rest, tracing and replay
