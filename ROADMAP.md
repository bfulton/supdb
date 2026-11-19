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
- [ ] **Range delete.** Deleting a key range without a tombstone per key.
- [ ] **Reverse and bounded scans.** `scan` is forward-only from a start key
  with a count; add an end bound and reverse order.
- [ ] **Batched point reads (`MultiGet`).** Cheap to add, and a natural new
  benchmark workload.
- [ ] **One sequence for the log and the runs.** A recovery orders the
  log's replayed frames above every piece, the one it rebuilds from a
  direct run's file included, so a run is sound only above every key the
  log names; the writer keeps that by its greatest key, which every write
  raises, a delete's included. A store written before deletes raised it
  may hold a run of a key whose tombstone is in its log past the
  manifest, and opens with the run's values masked. Done when run
  markers and log frames carry one commit sequence and a recovery orders
  a rebuilt run against the frames by it, which repairs such a store at
  its open, with crash tests of each order.
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
- [ ] **A zeroed first sector over frames that verify behind it.** An
  open reads a log file whose first sector is zeroes as a log with
  nothing in it and cuts it, the newest file included, whatever lies
  behind the sector. A power cut leaves that state only over frames no
  sync covered; media damage to a synced sector leaves it over frames
  that were acknowledged, and the open cannot tell the two apart from
  the file, since the policy that wrote it is the last life's. Done when
  such a file whose bytes past the sector hold a commit frame that
  verifies is set aside under a name of its own and named in the open's
  refusal or its counters, not cut.
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
- [ ] **A fall back past a damaged slot under a cipher.** An open holds
  a segment the slot it takes does not name to what holds its writes,
  and refuses where nothing does (`Db::unnamed_refusals`); under a
  cipher such a segment is an envelope whose key was in a slot that
  failed its check, so it cannot be read, and the open sweeps it as the
  crash window's. A crash passes over only a slot that never became
  durable, so this is media damage alone,
  and the open answers `Ok` with the writes only the damaged slot named
  missing. Done when a slot under a cipher carries what an open needs to
  judge such an envelope without its key, or the open keeps the
  envelopes no slot it read names until a slot is written over the
  damaged one.
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
- [ ] **A fall back past a damaged slot whose log retired.** A slot
  that names a seal's segment durable retires the log files that held
  its writes; with that slot damaged, the open falls back to the slot
  before, sweeps the segment, replays a log that no longer holds them,
  and answers the writes' older values. A buffered store whose publish
  of that landing was torn by a power cut leaves the same files -- the
  segment synced, the log's frames for it never synced and cut -- and
  must open, so the files cannot tell the two apart and the open sweeps
  in both (`Db::unnamed_refusals`). Media damage alone. Done when a slot
  names the log file its covered sequence lies in, so that an open can
  see a log file the slot before needs and the directory lacks; the
  reproduction is `a_fallback_past_a_slot_whose_log_retired_is_refused`
  in `tests/durable.rs`, ignored while it fails; the words it holds the
  refusal to are a placeholder for the open's own.
- [ ] **A fall back held by a segment's last key.** In the
  clear the same check reads an unnamed segment newer than every segment
  the slot taken names, and within what it covers, by its last key, and
  sweeps it where that key is held by that slot's segments or a run's
  temp name, as a merge's outputs are by their inputs; no other key of
  it is read. A segment whose last key the slot's segments hold and whose
  other keys they do not, or a run whose id was drawn
  before a merge the older slot names, or that closed after a commit the
  log holds past what the older slot covers, passes. Damage
  only, as above. Done when the segment says in its own bytes whether it is a
  merge's output, or the check reads every key where the last passes.
- [ ] **What a guard does not cover.** A failed directory barrier is made
  by guards (`vault::guard`) and the writes go on; where the guard fails
  too, the failure is kept until the store is reopened. Two ways in stay
  open. A reopen in the same boot after a kept failure lists the names in
  the kernel's cache and fsyncs the directory through a fresh descriptor,
  whose `Ok` says nothing of an entry a failed sync dropped, so it takes
  for durable a name the failure was for; a guard made at the open for
  every name no slot covers would close it, at a link a name an open. And
  the directory fsyncs of the segment work -- a seal's landing, the rename
  shape's publish -- stop the seal queue where they fail rather than
  guard the names. Beside them, the order a guard and its file are
  removed in, the guard first, is held by reading alone: no test builds
  the state between the two unlinks.
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
- [ ] **A form file's damage taken as the form's alone.** A chunk of a
  form file that fails its tag is a read of the store that failed, and
  the store refuses every read after it, as it does for a segment's. A
  plain store takes a damaged form file out of service, reads the
  partition raw and builds the file again, and a form file is as
  regenerable under a cipher. Done when a failed chunk of a form file's
  envelope is the form's failure, at every site that reads a form page,
  with the byte-damage test saying which outcome each flip had.
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
- [ ] **A writer that waits for segment work that is gone.** A panic on
  the segment work's thread leaves the writer waiting in `land_seal` for
  a landing nobody will make, with no end: found under a mutant whose
  debug assertion fired on that thread, where the test hung and did not
  fail. Done when the writer answers with the thread's failure.
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
  two calls. `bench/scripts/pair.sh supdb-cipher supdb 300000`
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
