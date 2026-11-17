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
- [ ] **A helper sync in flight when the run's commit syncs.** The run's
  commit reads the helper thread's kept fdatasync error after its own
  fdatasync, and a helper whose failed sync has consumed the device's
  error but not yet stored it is read as clean: the commit is
  acknowledged, and the next sync answers the failure. The window is the
  instant between the helper's syscall returning and its store. Done when
  the commit waits for a helper sync in flight before reading (a count
  the helper raises before its fdatasync and lowers after its store,
  asked beside the commit's own fdatasync), priced on the ordered load
  under `Always` at a size past `sync_ahead`, with a test that holds the
  helper inside its sync. The cheaper shape to price first: the helper
  syncs through an open file description of its own (the file opened
  again by path, or through `/proc/self/fd` for a memory file) in place
  of a duplicate of the writer's, so that the kernel reports a writeback
  error to the writer's own fdatasync as well, whichever sync met it
  first -- one open where the helper is spawned, nothing a commit; it
  needs a device that fails writeback (dm-flakey) to be tested at all.
- [ ] **A seal's failed sync held on a device that makes it, and a
  store that goes on past it.** A seal that fails at any step stops the
  seal queue for the life of the open: no manifest is written, every
  `flush` answers the failure, and the open replays the log
  (`Shared::seal_wedged`). A `sync` answers nothing of it, since what it
  promises is in the log; a caller that syncs and never flushes learns of
  it from the one commit that answers the segment work's error. Every `sync` answering it, with commits
  still `Ok`, is a writer-side kept answer set where the writer first
  learns of the failure and returned by the check `sync` makes of a kept
  error already: a contract to choose before it is built. The tests inject each failure through seams
  that skip the step they fail, and none has met a real writeback error
  under a seal, nor a directory fsync that fails while the filesystem goes
  on. Done when a dm-flakey or dm-error target fails a seal's fsync after
  its readable landing and the store names nothing of it and reopens to
  every acknowledged write. Beyond that, a store that stops sealing holds
  every later table in memory until the frozen list is full and is of use
  only to be reopened; the shape that goes on writes the failed seal's
  table again into a new file under a new name, never fsyncing the old
  inode again, and lands it as any seal -- to be built with its own crash
  tests if a store that must not be reopened ever asks for it. Merges and
  promotions still start while the queue is stopped, and their outputs are
  files no slot names, swept at the open.
- [ ] **A run's due commit beside log frames no sync covered.** Under
  `EveryN` a run's commit that is due syncs the run's file and not the
  log, and a run opens over a log that may hold the frames of a table
  frozen before it, unsynced, its seal not yet durable: a power cut then
  keeps the run's commits and loses the earlier ones in the log, which is
  not a tail of the commit order. A `sync` covers both. Done when the
  run's open notes, once a run, whether the log owes a sync, and the due
  branch asks that note beside the run's fdatasync and syncs the log
  first, priced on the buffered loads after a shuffled one, with a crash
  test that holds the frozen table's seal before its fsyncs.
- [ ] **A sync over a run closed into a seal in flight.** Under
  `EveryN` a run in the clear that closes -- at any write it cannot take
  (an `append` of a key at or below its greatest, of an empty or overlong
  key, or of a value that does not inline; a `put`; a `delete`), or at
  any seal (`Db::seal`, or the one a commit makes at the threshold) --
  hands its writer to a seal with the commits the
  policy left unsynced, the tail short of a whole piece still held: a
  `sync` before that seal lands answers `Ok` over them, and a kill or a
  power cut then loses them while a later commit to the log survives. A
  `flush` covers them; the docs say a `sync` does not. Two shapes, each
  priced first: the run's close notes, once a run, that it closed owing
  commits, and a `sync` that finds the note waits for that seal's
  durable landing -- a wait on another thread inside `sync`; or the close
  writes the held tail on the writer's thread (run-tail-kill's P1, one
  write under a megabyte a close) and keeps the run's file handle, and a
  `sync` that finds the note fdatasyncs it, a syscall only where
  something is owed. Done with a test that closes a run every way --
  a key below its greatest, a `delete` and a `put` above it, a value that
  does not inline, `Db::seal`, and a commit that crosses the seal
  threshold -- under `EveryN(u32::MAX)`, holds the seal's landing, syncs,
  and reads every commit back after a kill and after a power cut, on
  every run arm.
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
- [ ] **A copy's entry found by its rank, wrong.** One seed of the
  differential fuzz, outside the gate's range, trips the block cache's
  check that a copy's entry taken by rank is the entry a search finds
  (`ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1 ZFUZZ_SEEDS=1179 cargo test --profile
  checked --test fuzz fuzz_range -- --exact`), in a plain store with the
  upkeep inline. A release build has no check there and patches the entry
  the rank names.
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
