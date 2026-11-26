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
- [ ] **An arm at FASTER's promise.** `faster` is in the roster with a
  guarantee of its own, in memory until a checkpoint, and nothing to be
  compared with there: the buffered arms hand every commit to the kernel
  and FASTER hands it to nobody. A supdb arm whose log stays in the
  process's memory between checkpoints would be the matched one, and is
  an arm for that comparison and not a default. Done when the group has a
  supdb arm in it and the figures draw it.
- [ ] **The threaded mixes in a row, or not.** `threads-A` and `threads-RMW`
  are the workloads FASTER is picked for, gets and puts from several
  threads at once, and they run only where `--passes mixes-threaded` asks:
  a store's worth of operations a thread, on one thread and each thread
  count, for every arm, is hours more a full row. Done when a row carries
  them for the arms and at the operation count somebody chose, or the
  suite says why it does not.
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
  makes none, and a run on a spare whose name a directory fsync the store
  made anyway has promised makes none either (`Options::run_spare`): a
  durable ordered load at ten thousand keys makes one directory fsync
  fewer, its first commit a tenth of a millisecond shorter.
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
  decomposed.
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
- [ ] **A write's first touch of the memtable's index, priced across
  every core.** A profile of the writer through the lag sweep's full
  rewrite at ten million keys under 1.25 GiB (`perf record -a -g`, cut to
  the burst by `SUPDB_LAG_MARK`, at `b7a1177`) put 12.5% of the plain
  store's writer in `smp_call_function_many_cond` under
  `MemTable::index_insert`, by way of `ptep_clear_flush`: a page of the
  hash index read by the probe before the insert writes it maps the
  shared zero page, and the write copies it and flushes the page table
  entry from every CPU the process runs on, which on this guest is an
  interrupt to each. A store under a cipher pays the same, 15%. Not
  priced against a shape that writes the index's pages before it reads
  them; the probe of a slot that has never been written is the
  question.
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
- [x] **One sequence for the log and the runs.** A recovery orders the
  log's replayed frames above every piece, the one it rebuilds from a
  direct run's file included, so a run is sound only above every key the
  log names; the writer keeps that by its greatest key, which every write
  raises, a delete's included. A store written before deletes raised it
  may hold a run of a key whose tombstone is in its log past the
  manifest. Runs take sequence numbers since store format 2, and the
  open now orders a delete against a run it takes by them: a delete older
  than the run, of a key at or above the run's first, masks nothing a
  put had written -- the run opened above every such key -- and is
  dropped from the replay, which a store written since never holds
  (`SealWaits::stale_deletes`). The range note an open leaves for a run
  it took carries the run's first key, so every open until the manifest
  covers the run orders it the same way
  (`an_append_after_an_unraised_delete_is_ordered_by_its_sequence_at_the_open`,
  `tests/durable.rs`, through `Db::emulate_unraised_deletes`). A
  version-1 store's runs carry no numbers and are taken wherever they
  stand, as before.
- [ ] **A log's device errors held on a device that makes them.** A
  failed write of the log is put back and written again, and a failed
  sync of it is kept until the store is reopened (`SyncPolicy`). The
  tests inject both through seams in the engine, and the seam for a
  failed sync models Linux's: the error reported once, the pages it
  covered lost, and a later fdatasync of the same file succeeding. No
  test has run the store over a device that fails a write or a writeback
  for real. Done when a dm-flakey or dm-error target under the store's
  filesystem (root and a loop device, as `scripts/fscrypt.sh` sets one
  up) fails the log's writeback in the middle of a commit stream, and the
  store answers the error to every commit after it and reopens at a
  prefix of what it acknowledged; and when the same target returns
  ENOSPC for a write, and the store commits again once space is freed.
  The same for a direct run's file: its writes at the offset the writer
  counts, a failed one held and written again at the next sync, and a
  failed fdatasync kept, its own or the helper's behind the writer.
- [ ] **A log's power cuts held on a device that loses pages.** An open
  takes what it replays from the log as unsynced, and reads a log file
  whose first sector is zeroes as a log with nothing in it. The tests
  build each state a power cut can leave by hand, from a copy of the
  page cache and a trace of the fdatasyncs the store issued
  (`Db::trace_wal_syncs`); none has cut a real device's power, and which
  of those states ext4 and xfs reach under their defaults is reasoned and
  not observed. Done when a dm-flakey target that drops unflushed writes
  (`drop_writes`, or a dm-log-writes replay to each flush) runs a kill, a
  reopen, a `sync` and a cut, and a fresh store, a rotation and a flush
  with the log never synced, and every open after reads what was
  acknowledged. The reading takes a power cut to keep or lose a sector
  whole: a device that tears inside one would undo the first-sector rule,
  and the in-place rewrite of a recycled file's header
  (`Options::recycle_wal`) with it, since there the new header and this
  life's first frames share the sector behind which the last life's
  header would otherwise survive.
- [ ] **A reopen with nothing replayed held to no fdatasync at the
  syscall.** The tests read the log's syncs through the engine's own
  trace (`Db::trace_wal_syncs`), which records the syncs the engine's
  sync paths make and the open's cut, and a sync written straight into
  the open's path records nothing there: such a sync passes every test.
  `strace -f -y -e trace=fdatasync,fsync` over
  `a_reopen_with_nothing_replayed_owes_its_log_no_sync` reads none of a
  reopened log's files synced but where the open cut a torn tail or a
  lost header, by hand. Done when a test counts the process's
  fdatasyncs of the store's files across an open, without a seam the
  path has to remember to call.
- [x] **A zeroed first sector over frames that verify behind it.** An
  open reads a log file whose first sector is zeroes as a log with
  nothing in it and cuts it, whatever lies behind the sector. A power
  cut leaves that state only over frames no sync covered; media damage
  to a synced sector leaves it over frames that were acknowledged, and
  the open cannot tell the two apart from the file. A file whose bytes
  past the sector hold a commit frame that verifies under its id's seed
  is copied whole to `wal-<id>.aside`, synced, before the replay's cut,
  and counted (`SealWaits::wal_set_aside`); the open goes on as before,
  since the power cut's store must open. Not under a cipher, where a
  name the vault does not class is held in memory and lost at the close
  (`a_lost_first_sector_over_frames_that_verify_is_set_aside_and_not_cut`,
  `tests/durable.rs`).
- [x] **A helper sync in flight when the run's commit syncs.** The
  helper thread behind a segment's writer syncs the file through an open
  file description of its own (the file opened again through
  `/proc/self/fd`, a memory file as well), so a writeback error its
  fdatasync meets is reported to the writer's own fdatasync too,
  whichever runs first, and a commit that returns before the helper has
  stored its error answers the error itself. A test stands in for the
  device's error by description (`HelperFault`) and holds the helper
  between its sync and its store; on a real device the two-description
  behaviour was read with a probe (ext4 on a loop device over a full
  tmpfs), not by the store's tests, and the store over such a device is
  the item above. Where the file cannot be opened again the writer has
  no helper and syncs alone.
- [ ] **A seal's failed sync held on a device that makes it, and a
  store that goes on past it.** A seal that fails at any step stops the
  seal queue for the life of the open: the segment work writes no
  manifest from its first look after the failure, every `flush` and
  `settle` answers it, and the open replays the log
  (`Shared::seal_wedged`). A `sync` answers nothing of it, since what it
  promises is in the log, until a write is dropped for want of room in
  the frozen tables -- a write that ends an ordered run, or the first
  write after a `sync` handed the live table to a seal -- after which
  every commit, `sync` and `flush` answers it; a caller that syncs and
  never flushes otherwise learns of it from the one commit that answers
  the segment work's error. Every `sync` answering it, with commits
  still `Ok`, is a writer-side kept answer set where the writer first
  learns of the failure and returned by the check `sync` makes of a kept
  error already: a contract to choose before it is built. The tests inject each failure through seams
  that skip the step they fail, and none has met a real writeback error
  under a seal, nor a directory fsync that fails while the filesystem goes
  on. Done when a dm-flakey or dm-error target fails a seal's fsync after
  its readable landing and the store names nothing of it and reopens to
  every acknowledged write. Beyond that, a store that stops sealing holds
  every later table in memory -- until the frozen list is full where the
  segment work has a thread of its own, and in one live table that every
  commit at the threshold answers the failure over where it is inline --
  and is of use
  only to be reopened; the shape that goes on writes the failed seal's
  table again into a new file under a new name, never fsyncing the old
  inode again, and lands it as any seal -- to be built with its own crash
  tests if a store that must not be reopened ever asks for it. A merge in
  flight at the failure finishes and is held, and its outputs are files no
  slot names, swept at the open.
- [ ] **A torn manifest slot on a device that tears one.** A publish
  writes the slot that does not hold the newest manifest known durable,
  and an open fdatasyncs both slots as it found them (`SlotChoice`), so
  a power cut tearing a slot write leaves the other slot whole on the
  device. The
  tests build the tear by hand -- the slot's header rewritten over what
  the device held, the other slot as its last fdatasync left it, read
  from the engine's own record of which syncs returned
  (`Db::watch_slot_writes`) -- and no device here has been seen to tear
  a write of a slot, which fits in a page; nor has anything shown that an
  fdatasync of one slot file leaves the other's dirty page unwritten, on
  which the defect the rule closes rests. Done when a dm-flakey target
  drops or tears the writes after a chosen point inside a drain with its
  slot owed and a merge publishing behind it, and the store opens to
  every acknowledged write.
- [x] **A fall back past a damaged slot under a cipher.** An open holds
  a segment the slot it takes does not name to what holds its writes;
  under a cipher such a segment is an envelope whose key was in a slot
  that failed its check, so it cannot be read, and the open sweeps it.
  What makes the sweep safe now is that every way a slot passed over can
  have taken the only other copy of its writes leaves a trace the slot
  taken can see: a seal's landing retires the log files that held them,
  which the slot taken needs (`Manifest::needs_log`, `Db::log_refusal`);
  a run's commits under a cipher are frames of that log
  (`Db::log_run`); and a merge's or a promotion's landing removes the
  inputs the slot taken names, which the open refuses as missing
  (`a_fall_back_under_a_cipher_past_a_slot_whose_log_retired_is_refused`,
  `tests/cipher.rs`).
- [ ] **An open after a crash, priced by its parts.** An open that
  finds segments the slot taken does not name, newer than every segment
  it names and within what it covers -- a merge held at its landing by
  the crash -- costs over half again an open of the same store with no
  slot synced and nothing checked, and the fold of the check to the
  segments whose fences take each key in did not move it, since a
  merge's outputs span most partitions. What is left is not attributed:
  the check opens each such segment whole (`Blob::open`, its key
  section's checksum row read) for one key, and each holder whole. Done
  when the open's phases are stamped over that state and the share of
  each read is known, and, if the whole opens are it, a last key read
  through a check that covers only what is read is priced against them.
- [x] **A fall back past a damaged slot whose log retired.** A slot
  that names a seal's segment durable retires the log files that held
  its writes; with that slot damaged, the open fell back to the slot
  before, swept the segment, replayed a log that no longer held them,
  and answered the writes' older values. Every slot now names the oldest
  log file it needs, one past every log file a publish up to it has
  retired or is about to (`Manifest::needs_log`, behind the format word,
  which a reader from before it never reads), and an open refuses a slot
  whose log file the directory lacks (`Db::log_refusal`): a log file
  retires only behind a durable slot that needs a later one, so the file
  is gone only where a newer slot was passed over. A buffered store's
  torn publish leaves the log file in place and opens as before
  (`a_fallback_past_a_slot_whose_log_retired_is_refused`,
  `tests/durable.rs`).
- [ ] **A fall back held by a segment's last key.** In the
  clear an open holds an unnamed segment newer than every segment the
  slot taken names, and within what it covers, to its last key, and
  sweeps it where that key is held by that slot's segments or a run's
  temp name, as a merge's outputs are by their inputs. Two of the ways
  past it are closed: a seal's whose log retired behind a newer slot is
  refused by the log file the slot taken needs (`Manifest::needs_log`),
  and a run's piece past what the slot covers with its temp name gone is
  refused whatever its id, which a spare drawn early can put below the
  slot's (`a_run_below_the_slots_ids_is_not_lost_when_the_newest_slot_is_damaged`,
  `tests/durable.rs`). One is open: a merge output past what the slot
  covers that folded in a run's piece, whose temp name the newer slot
  retired, reads by its name as a seal's in the crash window and is
  swept. Media damage alone. Done when the segment says in its own
  bytes that it is a merge's output and an open can show that no merge
  in the crash window stands past what the slot it takes covers.
- [ ] **What a guard does not cover.** A failed directory barrier is made
  by guards (`vault::guard`) and the writes go on; where the guard fails
  too, the failure is kept until the store is reopened, and the store
  leaves a mark (`dir-lost`) for the next open, which guards every log
  file and run temp name it lists and syncs the directory before it goes
  on: a reopen in the same boot reads those names out of the kernel's
  cache, where the failed fsync may have dropped them, and an fsync
  through a fresh descriptor says nothing of that. Where the directory
  takes no new entry at all the mark is not made, and nothing more is
  promised. The order a guard and its file go in, the guard first, is
  held by a test that stops the removal between the two
  (`vault::tests::a_guard_goes_before_its_file`). What is left: the
  directory fsyncs of the segment work -- a seal's landing, the rename
  shape's publish -- stop the seal queue where they fail rather than
  guard the names, which keeps every write's durability and gives up the
  store's writes until it is reopened; guarded, they would go on.
- [ ] **The open's directory fsync held on a device.** An open that
  adopts pending segments (`Options::manifest_pending`) fsyncs each, its
  ordered index under `ord_durable`, and the directory before the slot
  that names them durable. The tests hold the order through the open's
  own record of what it fsynced (`Db::open_synced_files`) and the calls
  it made and saw return (`Db::trace_dir_io`: each file's fsync and the
  directory's before the slot's open), and where the process is root,
  each file's through the kernel's page flags; nothing reads whether the
  directory's entries or a file's pages reached the device. Done when a
  dm-flakey target that drops unflushed writes cuts the power after
  such an open, and the store reopens with every acknowledged write,
  with the directory fsync removed as its control.
- [x] **A run's commits on the device before its first due sync.** A
  run's records take sequence numbers as the log's frames do and its file
  says where it began and whether everything before it was synced, so a
  power cut that keeps run commits the helper thread, the kernel's
  writeback or the close's presync put on the device, and loses older
  frames of the log, keeps none of them: the open removes a run that
  begins past where the history it finds ends.
  `a_runs_commits_its_helper_synced_before_a_due_one_outlive_older_log_frames_until_it`
  and `a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames`
  hold the prefix.
- [x] **A sync over a run closed into a seal in flight.** A run that
  closes with commits no sync covered is noted loose, its file held open,
  and the next sync -- the policy's, a `sync` -- syncs its
  file before the log (`a_sync_covers_the_runs_as_well_as_the_log`); a
  later run's due commit does the same, so a power cut after it keeps
  both runs
  (`a_runs_due_commit_after_a_run_closed_into_a_held_seal_outlives_that_runs_commits`).
- [x] **Encryption at rest for a live store.** A store opens under a key
  (`Encryption::Kernel`) and the kernel encrypts its directory: every file
  it writes is ciphertext on the device and plaintext in the page cache,
  so its reads lend from the mapping as they did (`supdb::dirkey`). It is
  Linux's file encryption, on a filesystem that has it, and it is refused
  anywhere else rather than written in the clear.
- [x] **A live store under a cipher the caller supplies.** A store opens
  under a cipher of the caller's (`Encryption::Cipher`) and seals everything
  it writes to its directory under it: the segments and their indexes as
  envelopes, the log record by record, the manifest slot by slot, and the
  files' names, which stay in the manifest. It reads its files through a
  cache of plaintext of its own, a chunk decrypted where it is first read
  and lent from in place. The caller's cipher wraps the store's own key and
  nothing else, each file is under a key drawn for it, and
  `supdb::cipher_rewrap` hands a store from one of the caller's keys to
  another without touching a file (`supdb::db::vault`). On any filesystem,
  Linux only. What it does not do yet is the open items below.
- [ ] **An envelope written beside its segment.** A plain segment's bytes
  go to the device while the segment is built, and its fsync finds little
  left to write; a segment under a cipher is encrypted and written after
  it is finished, since the envelope's check covers the file's length and
  nothing can be sealed before the length is known. That is the whole of
  what the pairings show against the same store in the clear: the loads
  slower, and a merge's landing later. Done when the segment writer hands
  finished pieces to the cipher as it writes them, which wants an
  envelope whose chunks do not bind the length, and a last chunk that says
  it is the last.
- [x] **A store under a cipher larger than memory.** The store's cache of
  plaintext has a budget (`StoreCipher::cache_bytes`), gives back what has
  not been read lately once it is past it, and decrypts a chunk again
  where it is read again, so a store of any size runs in the cache it is
  given; an open decrypts what it reads of each file and holds no
  descriptor for one. The readers ask their source for bytes before they
  read them (`Bytes::piecewise`), which a reader over a mapped file does
  not, and a file changed on the device is refused at the read that first
  wants the changed bytes (`supdb::db::pager`).
- [ ] **A read of one key larger than the cache.** What a read is lent it
  holds until it ends or rests, and the cache gives none of it back
  meanwhile: a scan rests every thousand keys and a merge every few
  hundred, but one key's values are read in one piece, so a key whose
  values outrun the cache holds the cache past its budget for the length
  of that read. Done when a walk of one key's runs rests between blocks,
  asking again for the extents it walks by.
- [x] **The block cache's builders under a cache the store does not fit.**
  The upkeep thread's pass, the builder ahead and the fill a commit makes
  itself rest between two blocks, holding nothing of a file, so the cache
  gives back what the blocks before were read from; they give their forms
  up only where the cache is past its budget after a rest and a wait.
- [ ] **A miss at less than a fault.** A chunk read into the cache is a
  fresh page of a memory file: the kernel's fault on it is over half of a
  miss, the copy of the ciphertext a fifth, and the cipher a tenth. A run
  of chunks read in order could be populated in one call and decrypted as
  one, which wants the cache to know a run when it sees one.
- [x] **A copy's entry found by its rank, wrong.** A copy's claim that
  its entries are its block's records was carried across a merge that
  rewrote the partition; the claim names the partition now
  (`CachedBlock::identity`), and `tests/settle.rs` builds the rewrite.
- [x] **Ordered ingest under a cipher.** An ordered run keeps its table,
  its record stream and its close; its commits go to the log as well,
  sealed there, and its segment gets its envelope where the run closes.
- [ ] **An ordered run under a cipher as its own log.** A run's records
  are written twice, to the log and to the segment's envelope, and the
  envelope is written whole at the close, where a plain run's bytes
  reach the device commit by commit. It wants the streamed envelope
  above, with a commit's chunks sealed as the commit is made and a
  recovery that reads the run's envelope to its last whole commit. Done
  when a run under a cipher writes no frame to the log.
- [ ] **A log file retired off the drain.** A drain whose promotion or
  merge publishes a manifest retires the closed log files behind it
  before it answers, and a file retired is an inode freed, a device
  round trip the size of the file. An ordered run under a cipher fills
  the log, so its drain pays it; a plain store's does wherever a closed
  log file waits at such a publish. The manifest's spare is unlinked at
  an idle tick for the same reason.
- [x] **The compact form under a cipher.** A form file is built from its
  segment's records read by the piece through the store's cache, sealed
  beside the segment's envelope under the segment's key, and read by the
  page through the cache, under `ZPages::Always`; the controller samples,
  builds ahead, buys and installs there as it does for a plain store
  where it is asked to (`Options::zpages_under_cipher`).
- [ ] **An envelope whose chunks are whole pages.** A chunk of an
  envelope is a stored page less its tag, so a page of plaintext lies
  across two chunks, and a form page, which is a page, is two pages of
  ciphertext to a cold read: under a bound the form the controller buys
  read at 0.73x of the raw form it replaced, and the controller is off
  under a cipher for it. With the tags kept apart, at the envelope's end
  or in a file beside it, a chunk is a page, a form page is one page of
  ciphertext and a tag's page serves a megabyte of them; the store's
  cache then gives back by the page and not by the megabyte a run of
  chunks comes to. It is a second layout of an envelope, for a store's
  own files, with the first on record for a published one. Done when
  `pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640
  --passes read` reads the form ahead in both sittings, and the
  controller is on under a cipher.
- [x] **A form file's damage taken as the form's alone.** A chunk of a
  form file that fails its tag was a read of the store that failed, and
  the store refused every read after it. The pager keeps such a failure
  the file's (`pager::Paged::regenerable`), the chunk a range the file
  does not lend, which the form's readers take as a page out of service,
  and the store reads the partition raw as a plain store does
  (`a_changed_byte_in_a_form_file_takes_the_form_out_of_service_and_not_the_store`,
  `tests/cipher.rs`, at the attach and in service).
- [x] **The stride form under a cipher.** A stride file is written from
  a kept segment's records read by the piece (`StrideWalked for Held`),
  read back and attached through the store's cache, and served by the
  readers a plain store's is: they index the file's range at its
  offsets, each page after the page's check, and for a file whose
  mapping lends nothing whole that check is where the page is asked for,
  at every read (`ZFile::asks`, `MmapBytes::lend_asked_map`). A store in
  the clear runs the code it ran (`sh scripts/cost.sh 83e63ed read_s
  scan_s scan_long_s`). Held by the stride oracle under a cipher in a
  small cache (`tests/zpages.rs`), a stride file's damage and its
  build's rests (`tests/cipher.rs`), and the differential fuzz with the
  form forced there under `SUPDB_PAGER_GUARD`. Priced in "Reads and
  scans out of core", below, by `supdb-cipherzstride`. Two things it
  leaves: a read through it asks the cache for a page a value, an
  out-of-line call a record of a walk, which in core is the scans at
  nine tenths of the raw form's under a cipher (`bench ab --arms
  supdb-cipherzstride,supdb-cipher --size 100000 --reps 3 --passes
  load,read,scan` at `3ea2aa2`, one sitting) and wants a walk that asks
  its run once; and
  the borrow its readers index spans the cache's whole range, pages
  other threads fill among them, so it holds by what the readers read,
  each page asked for first, and not by the borrow's type
  (`lend_asked_map`'s safety note), where readers generic over a source
  that hands each range, as `flatindex`'s are over `Sect`, would hold it
  by type at a count of the plain store's eight readers.
- [ ] **The plaintext a store under a cipher holds follows the room.**
  Built: where the cipher names no size the cache's budget is what the
  store holds and how much more the process may hold in memory the
  kernel cannot take back, read from the platform every twenty
  milliseconds, with what the store's large buffers have mapped coming
  off at once and what the cache lately overshot kept back
  (`pager::Room`, `zfile::unreclaimable_slack`; `CLAUDE.md` has the
  kills that shaped it). Held by the cache's unit tests under a room a
  test moves, a store whose room falls and rises under it
  (`tests/cipher.rs`), and six mutants. Under 640 MiB at five million
  keys it survives the row's passes in five reps of five and holds
  about 530 MiB where the half held 290. What is left: two stores in one
  process share the platform's room in equal parts, which no test holds;
  the bounds and rungs of the bounded roster beyond that one, a load and
  a lag sweep among them, ten sittings each; a caller whose own memory
  grows beside the store; and the controller's model reading the same
  room (`zpages_under_cipher`).
- [ ] **A store under a cipher under a bound, past the close that killed
  it.** At five million keys under 640 MiB the lag sweep killed it in
  four sittings of ten, and this item had that down as the writer piling
  up behind landings a cipher makes slower. It was the store's close: a
  drop joins the merges in flight with the writer's operation pinned, the
  cache gave nothing back for as long as they ran, and they read on into
  it a breath at a time (`CLAUDE.md`, the close's pin). The drop's joins
  say they hold nothing now, held by
  `a_close_that_joins_a_merge_holds_nothing_of_the_cache` and its two
  mutants. With it, at `b8b7e28`: the lag sweep in twelve sittings of
  twelve (`bench ab --arms supdb-cipherauto,supdb-cipher --size 5000000
  --bound 671088640 --passes lag --reps 6 --ops-div 10`), the most the
  kernel could not take back 582-615 MiB of the 640 where the killed
  passes sat at 629-630 for six to twelve seconds; and the row's other
  passes with both mix families in ten of ten (`... --passes
  load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled
  --reps 5`). Over ids at the same store and bound (`--keys ids`) it was
  killed twice more, and neither was the writer either: a read's settle
  ahead of its walk, one pin as long as the backlog the write mixes had
  left, which killed the store as shipped in a scan mix in four reps of
  five; and, with the controller on, a merge's output written into a
  cache steered to its margin before a reading of the room had seen it,
  one rep in three. The settle's look rests in a handle's outermost read
  and a merge says what each output is to hold where it opens it
  (`CLAUDE.md`, both, with their tests in `cipher_looks` and the
  vault's). With them, at `39ecdd7` (`bench ab --arms
  supdb-cipherauto,supdb-cipher --size 5000000 --bound 671088640 --keys
  ids --passes
  load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled
  --reps 5 --ops-div 10`): neither arm killed in five reps of five, the
  most the kernel could not take back 581-595 MiB in every pass where
  the run before the announcement had 621-636 in four of five. What is
  left:
  - The other bounds of the bounded roster, three and ten million keys
    under 1 GiB and 1.25 GiB, ten sittings each of the lag sweep and of
    the mixes, over numbered keys and over ids, with
    `scripts/stallwatch.py` beside them. Done when none is killed. Begun
    at `5112f9a` over numbered keys, both cipher arms, every pass, two
    pairs a bound (`bench ab --arms supdb-cipherauto,supdb-cipher --size
    3000000 --bound 1073741824 --reps 2 --ops-div 10`, and `--size
    10000000 --bound 1342177280`): none killed and the watcher with
    nothing to dump; at ten million keys each pass's most the kernel
    could not take back was 1,228-1,251 MiB of the 1,280, so that bound
    has some forty mebibytes in hand.
  - What is written unannounced: a seal's output, whose size is its
    table's, and an ordered run's file, which grows a commit at a time;
    each reaches the room a reading late, as a merge's output did. A
    form file's build says what its file is to hold where it makes it
    (`vault::expect` in `zfile::write_pages` and `write_stride`, at
    `153a623`): at ten million keys under 1.25 GiB two form files built
    ahead at the ordered load's end had grown shared memory by 99 MiB in
    86 ms beside a cache at its budget, and the same three loads a side
    (`SUPDB_PAGER_TRACE=1 SUPDB_LAG_MARK=1 bench ab --arms
    supdb-cipherauto,supdb-cipherauto --size 10000000 --bound 1342177280
    --passes load --reps 3 --ops-div 10` beside `GROUPMEM_INTERVAL_MS=20
    scripts/groupmem.py sample`) read the most the kernel could not take
    back at 1,214-1,253 MiB of the 1,280 at `781e2b8` and at 1,195-1,226
    with the announcement, none killed either way. A merge's own arrays,
    and a seal's sort and its writer's, are the engine's mappings
    (`Options::jobs_mapped`), which the room counts as they are mapped:
    with the cache full the process's own memory rose by 58 MiB at most
    within a tenth of a second, the rest of each fast rise being memory
    files, and the rises of a hundred megabytes and more are each pass's
    start, the harness priming fresh pages with the cache empty. What the
    peaks are now: five loads of six at 1,195-1,201 MiB in the ordered
    load, the cache at its budget beside the run's file of 23-41 MB and
    the room read at 7-36 MiB of slack; and one at 1,226, the shuffled
    load's end, the process at 106 MiB of its own and a form file built at
    a merge's finish announced. A lever for the run's file or a seal's
    piece waits for a peak that names it, read on the group's memory by
    kind through the pass, kill or none.
  - The evictor's waits, of which the long ones were a merge's. A merge
    wrote each output's envelope under its hold of the cache, and writes
    it unheld now (`CLAUDE.md`, the trace that named the close;
    `a_merge_writing_an_envelope_holds_nothing_of_the_cache`). The same
    sweep at `2a23cc4`, twelve sittings: 60 waits of sixty milliseconds
    or more where it read 246, five and a half seconds of them for
    thirty, and the longest of a sitting 107 ms at the median for 220.
    What is left of them is no wait on a file: a thread's own cold reads
    between two rests (a merge's `MERGE_REST` steps through the device,
    the ordered index's top level built by the piece at a first seek),
    and the upkeep thread freeing its forms under its pin.
  - What that did not move. Over numbered keys the most the kernel
    could not take back is where it was, 581-612 MiB a sitting, each
    peak a tenth of a second in a load: an ordered run's file in flight
    beside the cache with the process at 50 MiB, or a shuffled load's
    tables at 400 beside 200 of plaintext. The budget follows the room
    at twenty milliseconds, and what the writer puts in flight between
    two readings and does not announce is the excursion.
  - The margin. At its worst instants the group had 25-58 MiB between
    what the kernel cannot take back and its limit, and what puts it
    there is the process's own memory beside the cache: 330-405 MiB at
    the full rewrite where the plain store holds 199 and the same store
    with its controller on 265-300 (`SUPDB_LAG_COUNTERS=1`, the
    `lagcensus` line at the last point). A store under a cipher has its
    controller off (`cipher_controller`), so it never reads as out of
    core and keeps the writer's block forms through the sweep, 65 MB of
    copies in every sitting, which the plain store and the controller's
    arm have given up there; the items on the controller under a cipher,
    below, are this one's as well. The allocator's free and resident
    bytes, 145-250 MiB of the process in either arm, are the readers' and
    the jobs' arenas (`CLAUDE.md`, a lever built for the largest peak
    known) and no trim returns them.
  - What the shape still is, and what did not kill it: the frozen tables
    held to a count and not to bytes (`FROZEN_CAP`, a standing limitation
    in `CLAUDE.md`), a file in flight held whole until its envelope, and
    the envelopes written one at a time (`Vault::persisting`). The levers
    for those, the writer's wait drawn from the room by bytes and an
    envelope sealed as its file is written, wait for a kill that names
    them.
- [ ] **The heads' copy under a cipher.** The copy of a partition's
  ordered-index heads in the process's own memory (`Options::heads_owned`)
  is made from the index's mapping as it lies, which a store under a
  cipher does not lend, so none is made there and every seek reads its
  heads through the store's cache. Done when the copy is made by asking
  the index for its heads by the piece, and priced under a bound against
  the store without it; until then, the look that would make it still
  asks the kernel for the heads' ciphertext at each look before it finds
  it has nothing to copy, where the compact form is on under a cipher.
- [ ] **The controller's model held to a measurement under a cipher.**
  Its budget there is of two caches in series, the store's own and the
  kernel's of the envelopes, and at five million keys under 640 MiB it
  predicted no fault through the form where the census read 0.6 a read,
  and did not leave the form in five million reads. Done when the
  model's prediction for each form is printed beside the census there
  and the two agree, or the selector's exits act on the difference.
- [ ] **The controller under a cipher prices the kernel's cache alone.**
  It reads the group's budget, its refaults and the thread's major
  faults, as for a plain store, which under a cipher are the kernel's
  cache of the envelopes: right where the store does not fit memory. A
  store that fits memory and not its own cache of plaintext
  (`StoreCipher::cache_bytes`) takes two chunks decrypted for a raw point
  read and one and a decode through the form, and the controller sees
  none of it. Whether the form pays there is not measured.
- [x] **A writer that waits for segment work that is gone.** A panic on
  the segment work's thread left the writer waiting in `land_seal` for
  a landing nobody would make, with no end: found under a mutant whose
  debug assertion fired on that thread, where the test hung and did not
  fail. The thread's panic stops the seal queue with the panic as its
  cause, which is what it is -- nothing lands and no manifest is written
  for the rest of the open -- so every wait that answers a stopped queue
  answers it (`tests/durable.rs`, the writer beside segment work that
  panicked, on a thread with an end).
- [ ] **A store under a cipher on another platform.** The memory files a
  file is written in and the cache gives pages back from are Linux's
  (`memfd_create`, `MADV_REMOVE`); elsewhere the store is refused before it
  writes anything.
- [ ] **A store under a cipher, in its cache, at the plain store's rate.**
  A reader of a file under a cipher asks its source for every range
  before it reads it, where a reader of a mapped file reads. With the
  whole store in its cache that is all that is left between the two, and
  it shows: point reads a little behind the same store in the clear, the
  scan passes further, and furthest where a walk reads by the rank, which
  is the scans after a burst and the scan mix. The scan over a partition
  asks for a window of records at a time already. Done when the walks by
  rank do too, a point read asks once for each piece it touches, and what
  a read does at its start and its end under a cipher is a look and not
  two calls. One cost there was taken on for the plain store's count: the
  ordered index's two cold arms went out of line (`plan_seek_late`,
  `seek_planned_by_piece`), and the settle's planned seek reads
  sixty-seven instructions a key more under a cipher for it; two shapes that
  took it back each cost the plain store and were refused. `bench/scripts/pair.sh supdb-cipher supdb 300000`
  is the pairing, and `bench/examples/read_only.rs supdb-cipher 100000 1
  n-none` under callgrind beside `supdb` the count; a shape
  that takes an instruction from the read under a cipher is counted on
  the plain read before it is kept.
- [ ] **A store under a key, priced on a kernel with hardware AES.** The
  pairings of `supdb-dirkey` against `supdb` ran where the kernel has no
  AES instructions built in (`/proc/crypto` names `aes-fixed-time`): reads,
  scans and the read mixes were level, and every write that syncs ran at
  about half, as a bare write and `fdatasync` with no engine did in the
  same two directories. Done when the pairing has run on a kernel that
  names `xts-aes-aesni`.
- [x] **The encrypted reader in the suite.** `supdb-segment`,
  `supdb-segment-enc` and `supdb-segment-encbounded`, and the same three
  with every value in a block (`supdb-segmentblocks...`): a write-once
  segment read plain and through an envelope with either reader, paired by
  `bench ab`. They are arms for a pairing and no row holds them.
- [ ] **An envelope written in one pass.** Publishing a segment encrypted
  writes it plain, in memory, and encrypts that into the file: the load of
  `supdb-segment-enc` runs at about six tenths of `supdb-segment`'s. Done
  when the segment writer writes the envelope itself.
- [x] **An envelope's first touch.** Asking for a long range's pages in one
  call took a quarter off the open and is the default
  (`ReadOptions::populate`); huge pages added four tenths to it and are off
  (`ReadOptions::huge_pages`). `examples/crypt_cost.rs` has both arms.

## Encryption: what is left before it merges

The encryption work (`Encryption::Cipher`, `Encryption::Kernel`,
`supdb::crypt`) merges whole, and its bar is the owner's: no impact for a
store with no encryption, and ahead of the stores a user would otherwise
pick on the encrypted roster (`CLAUDE.md`, the encryption work on this
branch). The engine's open items under a cipher are under "Engine" above.
This section is what the roster says, the order those items are worked
in, and what stands between the branch and a pull request.

### What the encrypted roster says

The roster is `bench run --scale quick --arms enc --out DIR` in the `enc`
build (`bench/DESIGN.md`), read cell by cell by `python3
bench/scripts/encrow.py ROW.json`; what follows is that row at `b12bd36`.
From ten to three hundred thousand keys the store under a cipher is ahead
of each encrypted comparator, every rep of it on the better side of every
rep of theirs, in about five cells of six of the durable group and four
of five of the buffered: every read, scan, shuffled load, lag point and
mix. Bytes on disk is no cell of
the bar (the owner's word; the plain format's is an accepted loss above,
and under a cipher an ordered load's records are on the device twice
until the run is its own log). What it loses:

- [ ] **Reads and scans out of core.** `BIN=$PWD/bench/target-enc/release/bench
  bench/scripts/pair.sh supdb-cipher rocksdb-tuned-enc 5000000 --bound
  671088640 --passes read,scan --reps 2 --ops-div 10` at `b12bd36`:
  encrypted RocksDB reads at 1.3-1.4x the store under a cipher and scans
  at 3.5x; encrypted LMDB (the same pairing against `lmdb3-enc`) scans at
  1.3x and reads at a quarter. The plain store is not out of core at that
  bound at all: its numbered keys take the stride form, a hundred bytes a
  key, which fits, and it reads at seventeen times the store under a
  cipher (the same pairing against `supdb`). What loses the cells is that
  a store under a cipher is kept out of the compact forms: the stride
  form was declined there when the pairing ran, and the controller is off
  because a form page lies across two chunks. With the stride form built
  there and forced (`supdb-cipherzstride` in that pairing's place, three
  reps, at `01b6f0b`), the ciphertext of an ask's chunks asked for
  together and a walk's pages asked as it begins, encrypted RocksDB
  reads at 1.31-1.32x and scans at 1.9-2.2x: the form reads 1.07-1.11x
  of the raw form under a cipher and scans 1.35-1.37x (the same pairing
  against `supdb-cipher`), and the together ask is 1.28-1.33x of the
  reads' rate by itself (against `supdb-cipherzstrideapart`). What the
  cell turns on is the plaintext held. A scratch build of `3ea2aa2` with
  the together ask and a cache whose size came from the environment, at
  500 MiB of the 640 where the store takes half of what is free at its
  open, read the forced stride
  form at 182-183 thousand reads a second against encrypted RocksDB's
  56, 3.2x and 3.3x in the two sittings, and scanned at 0.62-0.68x of
  it; the stride form is 477 MiB at that store where the raw form is
  669, so it is the form that can be held. The items, in the order they
  are worked: "The plaintext a store under a cipher holds follows the
  room", then the scans, which at 500 MiB still take two major faults a
  scan of a hundred where RocksDB reads the same bytes from the device
  with none and are not yet taken apart, then the controller on under a
  cipher, all under
  "Engine". With the cache following the room (`a166b4c`, the same
  command, three reps): the forced stride form reads at 2.7x and 3.1x of
  encrypted RocksDB and scans at 0.59-0.75x of it; the store as shipped,
  in the raw form, which the room cannot hold, still reads at 0.75-0.78x
  and scans at 0.35-0.39x, though ahead of itself at the half in both
  orders (`pair.sh supdb-cipher supdb-cipherhalf 5000000 --bound 671088640
  --passes read,read-threaded,scan,scan-threaded,mixes --reps 2 --ops-div
  10`: reads 1.11-1.25x, the threaded scans 1.6-1.7x, the six mixes
  1.2-1.9x, the one-thread scan level). So what the cell wants now is
  the store under a cipher taking the stride form of itself, which is
  the controller on there, and the scans. With the controller on
  (`supdb-cipherauto`) the one-thread scan pass was two major faults a
  scan, and they were nineteen reads of an ordered index's top level, a
  head an ask through a cache that had given the heads back, at each
  partition's first seek (`CLAUDE.md`, a level made late for the open);
  the level is read a run of heads an ask and by the landing that
  publishes the index now. After it the cell read in two shapes, by how
  many partitions had their form file when the load ended, and the
  builds ahead start past a quarter of the store's own cache now as well
  (`CLAUDE.md`, a line drawn on the page cache). At `6f04f9e`, with
  every pair printed (`BIN=$PWD/target-enc/release/bench
  scripts/pair.sh supdb-cipherauto rocksdb-tuned-enc 5000000 --bound
  671088640 --passes read,scan --reps 4 --ops-div 10 --pairs _per_s`):
  reads at 1.9-2.1x of encrypted RocksDB in the four pairs of one sitting
  and 1.07-2.34x in the other's; scans at 7-19x in the first and at
  1.02x, 2.6x, 17x and 18x in the second, the two low pairs the slow
  shape that is left. Done when `supdb-cipher` reads and scans ahead of
  `rocksdb-tuned-enc` in both sittings: the controller on as shipped
  under a cipher, which wants its pairings in core and at the other
  bounds first, and the shape that is left.
- [ ] **The loads with the controller on under a cipher.** At five
  million keys under 640 MiB the controller's builds during a load -- the
  builds ahead at landing in the ordered load, the builds at a merge's
  finish in the shuffled load's drain, twenty-two of them there for
  nineteen partitions, most superseded by the next merge -- cost the
  loads about a twentieth against the same store with the controller off
  (`scripts/pair.sh SUPDB_LOAD_PHASES=1 supdb-cipherauto supdb-cipher
  5000000 --bound 671088640 --passes load --reps 4 --pairs _per_s` at
  `acbc5ae`): the ordered load's commits half a second longer at the same
  CPU, the shuffled load's drain a second longer. Against encrypted
  RocksDB the ordered load is still ahead with it (1.08-1.21x in eight
  pairs of eight) and the shuffled load is not (0.82-1.04x, behind in
  seven of eight, where the store with the controller off is level, 0.91-1.07x;
  `BIN=$PWD/target-enc/release/bench scripts/pair.sh supdb-cipherauto
  rocksdb-tuned-enc ... --passes load`). What the writer waits on is not
  known: the writer's fdatasyncs are no slower beside the builds (`strace
  -f -e trace=fdatasync,fsync -T` over a pass of each, medians 0.43-0.44
  ms against 0.47), and the cache's waits at its crowding line are not the
  writer's and are fewer with the controller on; the cache fills twice the
  chunks and evicts 1.85x the units over the two loads. The writer's
  scheduler counters around the commits, four pairs (a scratch reading of
  `/proc/thread-self/schedstat` and the thread's switches beside
  `loadphases`): in the ordered load its time on a core is the same in
  both arms (2.77-2.99 s), its wait for a core a few milliseconds in
  both, and it blocks as often (18,600-19,600 voluntary switches), each
  block some 26 µs longer on the mean with the controller on -- the half
  second; in the shuffled load it does a twentieth more work on its core
  (4.75-5.44 s against 4.63-4.79) and waits longer for one (33-115 ms
  against 20-32). The next reading is what the ordered load's blocks are
  waiting on, the log's sync beside the builds' envelopes going to the
  device or a hand-off to another thread. The plain store builds at a
  merge's finish in a shuffled load as well.
- [ ] **The lag sweep with the controller on under a cipher.** At ten
  million keys under 1.25 GiB the store with the controller on reads,
  scans and runs every mix at 2.5x to 40x the store with it off, and its
  lag sweep's scan passes over the clean store and after the one- and
  ten-percent bursts run at 0.4-0.7x of it, the full rewrite's pass ahead
  (the same command as the other bounds', above, two pairs). The three
  are the plain store's out-of-core regime: with the controller on the
  store reads as out of core, the writer gives its block forms up for
  scans whose start blocks do not recur (`zformsoff`), and the sweep's
  scans take the merge path through the block-coded form files, eight
  groups a scan, at 11.6 and 22 µs a scan after the two smaller bursts;
  with it off the cache never reads as out of core and the upkeep
  thread's copies, 109,000 over the ten-percent burst, serve them at 6-7.
  Against the comparator it is not a cell lost: paired with encrypted
  RocksDB at the same bound (`BIN=$PWD/target-enc/release/bench
  scripts/pair.sh supdb-cipherauto rocksdb-tuned-enc 10000000 --bound
  1342177280 --passes load,read,scan,lag --reps 2 --ops-div 10 --pairs
  _per_s` at `27e5c5e`) the store with the controller on is ahead on the
  reads (2-14x) and the scans (2.7-6x) in four pairs of four, on every
  burst (1.9-3.7x), and on the three lighter lag passes and the ordered
  load in three of four; it is behind on the shuffled load (0.85-0.93x)
  and on the pass after the full rewrite (0.21-0.27x), four of four. That
  pass is the item below.
- [ ] **The pass after a full rewrite under a cipher, under a bound.**
  At ten million keys under 1.25 GiB encrypted RocksDB scans the pass
  after the lag sweep's full rewrite at 400-460 thousand entries a
  second and the store under a cipher at 90-117 thousand with the
  controller on, 78-99 with it off. In the pass 138-146 level-0 pieces
  stand, the scans take some 27 major faults a scan, and the device reads
  1.1-1.8 GB and writes 0.7-1.0 GB inside it: the burst's merge debt,
  paid beside the scans, which is the plain store's standing limitation
  for this pass (`CLAUDE.md`) with merges that decrypt their inputs and
  seal their outputs. The plain store at the same bound reads the pass
  at 0.9-2.2 million entries a second (the bounded rows at `a728c4c`),
  so under a cipher it is ten to twenty times slower, and the plain
  store's limitation is not the account of it. Beside the plain store in
  one sitting (`bench ab --arms supdb,supdb-cipherauto --size 10000000
  --bound 1342177280 --passes load,lag --reps 1 --ops-div 10`, with
  `SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1`, at `27e5c5e`): the plain store
  meets the pass with 41 pieces standing and nothing merging, 26 major
  faults and 138 MB read in it, 508 ms; under a cipher 144 pieces and its
  merges inside the pass, 259,000 major faults, 1.1 GB read and 1.0 GB
  written, 10.3 s, having landed 65 merges over the sweep to the plain
  store's 98. Each merge's own account (`SUPDB_MERGE_TRACE=1`) says why:
  under a cipher a merge moves 50 MB of input a second of its wall and
  109 of its CPU, against the plain store's 126 and 227, and takes 825,000
  major faults over the sweep to the plain store's 2,500 -- about half the
  pages of ciphertext it reads, each a device round trip. The merge's ask
  ahead (`Options::merge_read_ahead`, four mebibytes, by a scratch switch
  on the arm) fired, 382 asks of 1.3 GB, and moved neither the faults
  (661,000) nor the rate (49 MB/s): it asks a window only where the
  window's first page is not in memory, and covered a fifth of the input.
  Why the rest faults -- plaintext the cache gives back between the ask
  and the walk, or ciphertext the kernel drops under the bound -- is not
  established; the next reading is the merge's faults by file and offset
  against the cache's evictions of the same units. The CPU half is the
  cipher's and the cache's fills, and is counted next. The faults were
  the ask's own test: a window was asked only where its first page was
  not in memory, which says nothing of a source read by the piece, and a
  merge under a cipher asks a window ahead, a window whole, as shipped
  now (`Options::merge_read_ahead_cipher`, `CLAUDE.md`, a first page
  standing for a window). Paired against the shape before at `a19ffc6`:
  the pass 2.1-2.5x, the shuffled load 1.09-1.34x, the full rewrite's
  burst 0.74-0.77x, every pair of four; 39-44 pieces at the pass.
  Against encrypted RocksDB again at `7675d82` (the command in the item
  above, `--reps 2`, four pairs): the pass at 0.45-0.62x where it was
  0.21-0.27x, and with it the shuffled load ahead, 1.04-1.15x in four
  pairs of four where it was behind in four, the ordered load 1.01-1.15x,
  reads 2.1-21x, scans 2.9-7x, the clean store's pass 1.08-1.41x and every
  burst 1.5-4x; the passes after the one- and ten-percent bursts ahead in
  two pairs and three of four. Read beside the plain store's at `b7a1177`
  (`SUPDB_LAG_COUNTERS=1 SUPDB_MERGE_TRACE=1 SUPDB_LAG_MARK=1`, one sweep
  of each, and `perf record -a -g` cut to the burst and to the pass by
  the marks): both meet the pass with 34 pieces, and the plain store's
  merges are done before it (0.5 s, 184 major faults) where six merges
  and seven form builds of the store under a cipher run inside it (4.7
  s, 29,000 major faults, 600 MB written). In the burst the plain store's
  merges move 347 MB of input a CPU-second and 117 a wall-second, under
  a cipher 161 and 59, and the profiles by function put two thirds of
  what the cipher adds to a merge in the cache's fills of its inputs,
  seven tenths of a fill being faults on a fresh page of a memory file
  and on the envelope's page; the form builds at a merge's finish are
  the same CPU in both stores. A merge reading its inputs beside the
  cache (`StoreCipher::merges_beside_cache`, `pager::Stream`: each chunk
  decrypted into a buffer of the merge's own that its rests write again,
  a chunk the cache holds copied from it) moves 181-187 MB of input a
  CPU-second in the burst, and paired against the shape before at
  `b7a1177` with it in the tree (`bench ab --arms
  supdb-cipherautomergestream,supdb-cipherauto --size 10000000 --bound
  1342177280 --passes load,lag --reps 2 --ops-div 10 --pairs _per_s`):
  the pass after the full rewrite 1.24x in both pairs, and the clean
  store's pass 0.72x and the one-percent burst 0.70x, both pairs, the
  loads level. The two losses are the cache's state after the load:
  with the merges no longer filling it, it holds their outputs whole,
  1,017-1,022 MB against 603-704, and the kernel has less room for the
  ciphertext the clean pass reads. Off as shipped; what is left of a
  merge under it is spread over the envelope's copies and faults, the
  cipher, the output's memory file and the reads by the piece, none of
  them a lever that finishes the merges before the pass. The first rep
  of that pairing lost `supdb-cipherauto` to the bound in the shuffled
  load, 1,053 MB of shared memory charged where the process mapped 850:
  the writes no reading of the room sees ("A store under a cipher under
  a bound"). With a drop giving the plaintext it covers back
  (`StoreCipher::drops_keep_plaintext` off, at `95c4f1b`, `CLAUDE.md`)
  the cache meets the pass holding the pieces and the form files and
  almost none of the partitions' records, and the pass still reads
  near three seconds: the scanning thread's major faults are 8,300 on
  form files the burst's merges built in the ten seconds before the
  pass and 900 on pieces, with the cache 130-440 MB under its budget
  through the pass, and the merges in flight read 960 MB and write 640
  inside it. What took those form files out of the cache is not
  counted: by the code, the cache's clock passes once over a unit a
  merge read and takes a unit nothing has read since the cache took the
  file whole at its envelope, which a form file built at a merge's
  finish is. Counted by kind (`SUPDB_PAGER_TRACE=1`'s `given` and
  `pagerback`, `CLAUDE.md`): the burst gives back 1.4-1.6 GB of form
  files and 550-690 MB of pieces against 13-76 MB of partitions'
  records, and the pass's faults are 4,900-6,100 on form files and
  4,200-7,200 on the segments, pieces most of them; leaving a merge's
  reads cold for the clock moved the gives and not the pass, and was
  taken out. Inside the pass the merges are still running, 787 MB read
  and 632 MB written at the device in a pass of 2.7 s, and the rewarm of
  the form files waits for them to land: in one sweep it fired 1.9 s
  into the pass, as the last merge landed, and the pass's tenths after
  the two ran at a fifth of the ones before. In the burst the cache's budget falls to 450 MB where the
  merges' outputs are memory files while they are written (`writing`
  and `coming`, 100-280 MB). The levers left are those three: the merge
  debt paid before the pass, a merge under a cipher being twice the
  plain store's wall; the rewarm under a cipher, whose asks are
  ciphertext the kernel may take back, beside the merges; and a merge's
  output sealed as it is written ("An envelope written beside its
  segment"), so that it is not memory the cache gives way to.
  `StoreCipher::merges_beside_cache`, priced again against a cache that
  gives a served partition's records back (`SUPDB_LAG_COUNTERS=1
  SUPDB_SCAN_OPS=1 bench/scripts/pair.sh supdb-cipherautomergestream
  supdb-cipherauto 10000000 --bound 1342177280 --passes load,lag --reps
  2 --ops-div 10` at `068aca1`), still loses: the full rewrite's pass
  0.84-0.93x of the shipped store and the ten-percent pass 0.80-0.82x
  in both sittings, the loads level.
  Against encrypted RocksDB at `9e7ae54` (the command in the item above,
  `--passes load,read,scan,lag --reps 2`, four pairs) the pass read
  0.87-0.96x, every other cell of the pairing ahead or level: the reads
  4-4.9x, the scans 13-14x, the three lighter lag passes 1.9-2.8x, every
  burst 1.3-1.4x. Its scans waited 2.3-2.5 ms a read against RocksDB's
  62-66 µs, behind the burst's merges still writing at the device. The
  rewarm beside those merges (`supdb-cipherautorewarmbeside`) read the pass
  level or behind in four pairs, asking for the form files of partitions
  the merges then replaced and nothing for their outputs: refuted. The
  merges' asks laid to their inputs' regions (`CLAUDE.md`, a merge's asks)
  took the faults from 36,000 a burst to 530 and the pass to twice
  RocksDB's in four reps of six; in the other two the burst landed fewer
  merges and left the debt to the pass. Two things hold the merges now,
  by their threads' kernel stacks sampled every 5 ms through the burst: a
  merge's thread runs three fifths of its wall and waits for the device a
  fifth, on the reads its asks have in flight; and the segment work's
  thread spends 3.9 s of a 25 s burst in the filesystem's discard of the
  inputs a landing retires (`ext4_free_blocks` under `evict`), landing and
  planning nothing meanwhile. The retired inputs' unlink taken off that
  thread was built three ways and refuted (`CLAUDE.md`, a cost moved off
  the threads that paid it): the frees' discards land in the passes after
  the load whoever issues them, so the lever is not to free, which is the
  store in one file ("The page layer", below). Left beside it: a merge's
  CPU under a cipher, 342 ms against the plain store's 196 for the same
  input; and the pass paired against encrypted RocksDB again with the
  merges' asks, which has been read only against the shape before them.
- [ ] **The builds ahead's last cold partition under a cipher.** At five
  million keys under 640 MiB with the builds ahead started at a quarter
  of the store's cache, one or two partitions are still found cold in
  most reps, and in about one rep in eight the load ends with several
  unbuilt and the reads run beside their builds from the device. Past
  the selector's line a build ahead takes the oldest partition due first,
  a cold one among them, and the drain's last partitions, warm, wait
  behind it until the reads have evicted them too. Taking the warm ones
  first past the line changes the plain store's order as well, so it is
  priced on the bounded roster's rungs before it is built.
- [ ] **The ordered load.** In the row the durable ordered load is behind
  both encrypted comparators at ten thousand keys (0.66x, resolved) and
  lower and unresolved at the three rungs above (0.83-0.94x); the
  buffered is behind encrypted LMDB's at every rung (0.65-0.75x) and
  encrypted RocksDB's at ten thousand (0.75x). Against the plain store it
  is 0.72-0.75x durable and 0.53-0.66x buffered, which is the records
  written twice and the envelope sealed whole at the close ("An ordered
  run under a cipher as its own log", "An envelope written beside its
  segment", "A log file retired off the drain"); at about 0.9x of the
  plain load durable and 0.72-0.88x buffered it is ahead of both from
  thirty thousand keys. At ten thousand keys the plain store's durable
  ordered load trails both plain comparators too, and its buffered one
  LMDB's, so no encryption work wins that rung. Done when the ordered load is ahead of both encrypted comparators
  from thirty thousand keys in both groups; at ten thousand, where the
  owner has not said, the reading taken is that it stands to them as the
  plain store stands to the plain ones.
- [ ] **The open after a crash, under a cipher.** The `recover` workloads
  of the same row: after an ordered load the open under a cipher is 1.2x
  the plain store's at ten thousand keys and 2.6x at three hundred
  thousand (2x buffered), and 1.4-1.7x after a shuffled load. It is ahead
  of encrypted RocksDB's at every rung (1.3-5.8x) and behind LMDB's, which
  recovers nothing (an accepted loss above, the plain store's too). The
  difference from the plain store is not attributed: a run under a cipher
  has no file in the directory to finish where it lies, so its commits
  are replayed from the log into a table, and what else the open
  decrypts has not been stamped. Done when the open's phases are stamped
  over both stores and the store under a cipher opens within what its
  decryption costs of the plain one.
- [ ] **The roster past the quick scale, and the kernel's side.** The row
  is the quick scale's and the bounded cells are two pairs an order at
  one bound. Encrypted LMDB stops at three hundred thousand keys
  (`bench/DESIGN.md`), so past it the comparator is RocksDB's. Neither
  comparator has been run on a directory the kernel encrypts, beside
  `supdb-dirkey`, and this guest's kernel has no AES instructions built
  in ("A store under a key, priced on a kernel with hardware AES").
  Done when a `full` row of the roster is taken with encrypted RocksDB,
  the bounded scale's bounds are paired against it, and the three stores
  are paired on an encrypted directory under a kernel that names
  `xts-aes-aesni`.
- [ ] **The threaded mixes in the encrypted roster.** Gets and puts from
  several threads at once (`threads-A`, `threads-RMW` and their uniform
  twins, `bench run --passes mixes-threaded`) are no pass of a row unless
  asked for, and no encrypted row has carried them. `supdb-cipher` runs
  them, its threads a reader each and a turn at the one writer, and so
  does encrypted RocksDB through its shared handle; `lmdb3` and
  `lmdb3-enc` stop with "takes no writes from threads here"
  (`Engine::thread_writers`), the arm being on LMDB's C API with a
  decrypting context a thread and no writer a thread yet. Done when the
  two arms have one, a test of three threads reads back every value they
  wrote, and the encrypted roster carries the cells in both groups.

### The order of the work

First the "Engine" items the bounded cells above are lost by, by what
the pairings against the encrypted comparators put first: "The plaintext
a store under a cipher holds follows the room", built and to be held at
the roster's other bounds, "A store under a cipher is killed by its own
writes under a bound", the scans, the controller on, and "The heads'
copy under a cipher". The pass below
cannot be green while a mechanism does nothing under a cipher. Then that
pass, then the layer, whose stages are read again against those
pairings before one is built: it was designed for the ordered load's
double write and the two chunks a form page lies across, and the reads
turned on neither.

- [ ] **The log's sealing under the door.** Under a cipher the log
  seals its own writes (`Wal::enc`, `Wal::write_sealed`, a header of
  its own length), so every change main makes to the log's write path is
  a conflict here, and a mechanism main adds to it -- the log held in
  the process (`SyncPolicy::Explicit`) was the last -- has to be taught
  the sealing by hand. The reads are below the engine already: sources
  read by the piece, every file call through the door. Done when the log
  writes to an append file the vault hands it, which seals what it is
  given and cuts back what a failed write left, the log's own code being
  main's, and a store under a cipher passes the log's tests and the
  crash oracle without a branch on the cipher in `Wal`. The cache's
  give-back is the other place the cipher reaches into the engine's
  code (holds, rests, `unlent`), and is narrowed after: a long reader
  that says where it holds nothing is the price of reading in place
  from a cache that decrypts.
- [ ] **Every mechanism held to acting under a cipher.** The seal bars
  a whole borrow at the sites it lists, and a new walk over a section
  borrowed whole inside a reader's own module, or a new caller of a
  form's whole-file read, compiles, passes every gate and answers nothing
  under a cipher. Three mechanisms were found doing nothing there after
  they shipped: the stride form, built there since, the heads' copy, and
  the third merge of a
  stretch nobody reads (`Options::merge_parallel_quiet`), which waits for
  the compact form's gate, where the controller is off. A fourth was
  unsound and not idle: an unpinned writer (`writer_pins` off) was
  accepted under a cipher, where the store's cache does not wait for its
  reads, and is refused at the open now. Done when a group of
  `scripts/check.sh` runs every test
  that opens a plain store as a store under an identity cipher, with
  eviction forced at every rest and the guard on (`SUPDB_PAGER_GUARD`);
  the differential fuzz runs so at a small cache; every field of
  `Options` is drawn there or exempt with a reason; the asks a read, a
  scan entry and a merge step make are a list that only goes down.
- [ ] **The page layer.** The cipher's code is threaded through the
  engine as an envelope a file, a memory file while a file is written
  and a sealed record in the log, which is why a merge of the line this
  is built on costs what it does and why the items under "Engine" are
  each a special case: the double write of a run, the envelope written
  after its segment, a form page across two chunks, an unclassed file
  lost at the close. The layer is one module behind the `vault::` door
  that knows pages, lengths, versions, sync points, sealed names and a
  durability class a file, and no format, with a page of plaintext
  stored as one page and its tag kept apart. Its stages: the door's file
  type over the plain store alone, counted equal; the module standing
  alone under a stand-in cipher, its torn states built by hand; segments,
  merge pieces, ordered indexes and form files wired behind an option and
  an arm of its own, the envelope staying the default; a run's temp file
  as its own log; the controller on under a cipher; the log, its spare
  and the manifest's slots under the layer, and the default flipped; and
  files that fit the budget lent whole, which is where the design
  predicts a read under a cipher within tens of instructions of the plain
  read's. Each stage ends on the gate, the count and the pairings its
  predictions name. Done when `supdb-cipher` is that shape and the
  envelope is left to `supdb::crypt`, a published file's.
  The layer's lower half is a store in one file, by the owner's word
  (`CLAUDE.md`, the encryption work on this branch): the manifest's slots
  at the file's head, the log and every segment extents of it, space
  reused and never freed, a checkpoint a full copy. It is built first and
  for the plain store, as an arm, since what it takes back is the plain
  store's too -- every free, 8 s of the full rewrite's traced 26.6 at ten
  million keys under 1.25 GiB, every directory sync, open and close, and
  by a probe 44% of a segment's write and sync -- and the sealing then
  sits on extents where the design had a stored file a logical file, with
  no sealed names. `scratch/plan/onefile.md` has the stages and what each
  is held to; `scratch/onefile/design.md` the decisions.

### Before a pull request

- [ ] **Timing rows against the line this is built on.** "No impact for
  a store with no encryption" is held by instruction count, change by
  change, and a count sees no stall. Done when the quick, full and
  bounded rows at the branch's head stand beside the same rows at the
  commit of that line last merged, on one host, and no cell of the plain
  arms is on the worse side.
- [ ] **A verdict from the count against a line it did not grow from.**
  `sh scripts/cost.sh` against that line's head reads every probe under
  it by its total and exits red by function: a function the other build
  inlines and ours calls reads as a gain beside the caller that lost
  more, so a new regression reads like the list already reconciled, and
  against the main line the count takes in everything merged from the
  other line. Done when `scripts/cost-diff.py` reads a declared group of
  functions held to its net, valid against one named base and shaped as
  `scripts/raises.txt` is, and the job's verdict against a pull request's
  base is green for a tree that pays nothing. Beside it, what the count
  does not reach: no probe runs with the form's gate away from raw
  (`Db::zgate_set`) or merges under `merge_drop_behind` and
  `merge_read_ahead`, which is where `Blob::willneed_ranks` and its kin
  are called a key or a block at a time; `Txn`, `RunWalk` and the
  `SparseBlob` readers are outside the entry-point list; the bounds on
  the `~` lines rest on two to four self-checks a probe; and valgrind's
  noise on a hosted runner is unmeasured. And the count takes a loop
  head's alignment padding for an instruction: a `nop` the fall into a
  loop runs, which comes and goes with the length of whatever was
  inlined ahead of the loop. The writer's round has read red by it since
  the stride form went under a cipher, `work_probe::main` one
  instruction a key over the commit before with no instruction of its
  hot code changed (`CLAUDE.md`, "A second way for a check to say yes").
  The merge probe reads red by it since the merge's asks covered their
  inputs' directories (`b63c2de`): `compact_run` ten instructions a key
  over `9e7ae54` in a plain store, which asks nothing ahead, the sum by
  mnemonic +2.0M of `xchg %ax,%ax`, `nopl` and `nopw` against -0.4M of
  `data16` and the rest the same to within a few instructions over the
  run, with the ask's look out of line and the plain walk's source
  unchanged (`COST_DIR` and `COST_BASE_TARGET` kept, each binary under
  `--dump-instr=yes --dump-line=no`, summed by mnemonic).
  Done for that when the count is taken by instruction and a function's
  sum leaves its padding out, with a red input in which a gain of
  padding alone reads green beside a gain of one instruction that reads
  red. And the count is a function of the compiler: against one
  line's head it moved four probes from under to over with a toolchain
  and no commit (`CLAUDE.md`, "A count held under one compiler"), and
  what stands over upstream by function since is the block form's open
  path -- `zpages::view_with` five instructions a page view, with
  `parse_page`, `ZCursor::open_at`, `ask_group_ahead`,
  `ZFile::open_group_with` and `first_checks` about seventy a scan of a
  hundred records between them (`sh scripts/cost.sh 94356bc scan_z`) --
  from a source file identical on both sides, in probes that are under
  by total. Which of the program's differences the link-time optimizer
  answers with that is not known; it is to be found, by building
  upstream's tree with this branch's files added one at a time, or shown
  to be the unit's and held as a declared group. And the walk through a
  form of records that are not the common shape -- several values of
  different widths, a value in a block -- is on no probe of either
  line's: `walk_fixed` leaves that path as it was by construction, and
  nothing counts it.
- [ ] **The gates that have run on no hosted runner.** The `cost`,
  `lists`, `cross-arm`, `machine`, `enc` and `fscrypt` jobs first run
  where the first pull request is opened, and the `machine` group has not
  run on a Mac at all. Two are known to give no verdict there as they
  stand: the count, above, and the lists, which hold nothing against a
  base that predates them, as the main line does until this lands. The
  door's libc test reads the libc `Cargo.lock` resolves out of cargo's
  registry and fails a vendored or offline layout outright, and of
  `tests/durable.rs` one test holds about a thousand descriptors open
  and two ask `kcmp`, none tried against a hosted runner's limits. The
  owner's to choose: the lists landed on the main line ahead of the rest,
  and the libc test kept or its class list kept in the tree.
- [ ] **The `zpages` tests' waits under load.** Four copies of the
  checked `zpages` binary at once fail most rounds, on this branch and on
  the other line's own build alike, in the same tests: wall-clock waits
  and controller states never reached
  (`scans_after_a_quiet_stretch_have_the_cold_form_files_asked_for` most
  often, `the_trial_judges_the_refilled_state`, the hot-raw policy's,
  `readers_read_through_a_partitions_conversions_both_ways`,
  `handles_on_threads_read_stride_files_while_merges_retire_them`). The
  gate's one copy passes. What fixed the ones already done is a wait on
  the state and not on a count of rounds (`drive_to` to `drive_to_built`,
  `tick_until` to `tick_until_built`), test by test: a bulk replacement
  broke the tests that are exact to a tick. Done when four copies at once
  pass three rounds.
- [ ] **Defaults inherited unpriced under a cipher.** `bounds_by_heads`
  and `land_bounds_read` were made defaults on pairings of plain stores,
  and the cipher arms run them as inherited with no arm that has them
  off. Done when each is paired under a cipher against the same arm
  without it, at three hundred thousand keys and under a bound at ten
  million.
- [ ] **What no second reader has read.** The last repair of several
  units was read by whoever launched it and by no reviewer: the wall, the
  door, the count, the lists' check, the count against the other line,
  the seal's rebuild, the `cross-arm` group's repair, and five of the
  durability pricing units before them; `594fe3ca`, the merge probe's
  functions, was read by one reader. Done when the branch's whole diff of
  `src`, `tests/source.rs` and `scripts` has had a review that did not
  write it.
- [ ] **Small things left where they were found.** `key_eq`'s arm for a
  key under sixteen bytes compares two empty slices by a call of no bytes
  (the empty key in a carried copy, one key a store). `compact_run` takes
  `record_offset(0)` of every input of every merge, dropping or not,
  which under a cipher is a directory word read through the cache and
  thrown away. Three tests of `tests/blob.rs` hand closures straight to
  `Blob::scan_at`, a copy of the read path each. `zfile::ask_file`'s item
  allowance covers any barred call added to that function. The ordered
  index skips its checksum over any source that lends by the piece, where
  `Bytes::authenticated` now says which sources checked their bytes. The
  store's cache keeps no count of the advice it is given
  (`Paging::willneed`, `forget`), so what a build and the crossing ask of
  it under a cipher is held by outcomes alone. And three red inputs live
  outside the tree and run nowhere: a strace gate
  over the plain build's advice calls, a check of doc sentences against
  the code, and two probes of the readers' offsets.

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

## Security

What a review of the store as something that parses bytes it did not
write asked for, before encryption at rest was built. One is begun.

- [ ] **A threat model, written down.** A `SECURITY.md`: how to report,
  who is trusted (the embedding process and the live store's directory)
  and who is not (keys and values through the API, any file handed to the
  read-only readers or the wasm module), what counts as a vulnerability
  and what does not, what an embedder has to do, and what the format
  promises a reader that meets a file it does not understand. Two
  decisions come first: the reporting channel, and whether `Db::open` on a
  store copied from elsewhere is in scope.
- [ ] **The read-only parsers fuzzed behind a valid checksum.** `Blob`,
  `SparseBlob` and the wasm ABI exist to parse objects fetched from
  elsewhere, and a CRC stops an accident, not an author. The damage tests
  flip bytes, which the checksum refuses before any deeper check runs.
  The key section is done (`tests/forged.rs`): its header, hash slots,
  directory and records changed and its row computed again, every reader
  of `Blob` through a source that lends and one that copies, and
  `SparseBlob`'s plans and walk, held to no panic and no read without
  end; thirty thousand forgeries by hand, eighteen thousand of which
  opened under their rows, broke none, and an unchecked slice put into
  the record parser on purpose broke one in seventeen. Still to forge
  under their own checksums: the block table, a block's chunks, and the
  superblock's two slots; and the wasm ABI is not driven at all. Done
  when each of those is, with a known-red run for each.
- [ ] **The safe LZ4 decoder on the path an untrusted blob reaches.**
  `lz4_flex` is built without its checked decoder, and `src/block.rs`
  decodes blocks of a file the reader did not write. Priced with the suite
  first, since it costs decode speed.
- [ ] **No key or value can panic or stall the store, as a contract.** The
  differential fuzz found the empty key and the long common prefix; the
  guarantee is not stated, and the fuzz is what would hold it. Done when
  it is stated and the fuzz draws from the whole of the key and value
  space under it.
- [ ] **One writer a directory.** A process lock and a store format
  version, written and waiting in pull request #21.
- [ ] **Said, not fixed:** the index hash is unkeyed, so a caller's own
  users can choose keys that collide; a published blob is replaced by
  rename and never rewritten in place, since a reader's mapping of a file
  truncated under it is a fault and not an error; and the browser's
  credentials for object storage are short-lived and read-only.

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
- Tracing and replay
