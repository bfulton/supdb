# Supdb roadmap

Open work, one item each: what it is, when it is done, and the design note
where there is one. Figures name the row they are read from. High priority
is a wrong answer, an API or format that is harder to change later, a
number not yet fair to publish, an arm the bound kills, or a cell more than
a tenth behind its comparator or behind a shape the engine already reaches.

## High priority

- [ ] **A conditional write.** A put or delete that applies only if the
  key is as its caller read it, on the writer and in a `Txn`.
  `Reader::rmw` exists for handles under the held log, and is atomic only
  where the live table holds the key's whole read. Done when a refused
  write leaves the store and its batch as they were, threads
  read-modify-writing a few hot keys lose no update, and `threads-RMW` runs
  every arm through it. Design: `docs/engine.md`, "A live table every
  writer writes".
- [ ] **The held arm through a full rewrite at ten million keys.**
  `supdb-heldpart` was killed once under 1.25 GiB, and the kill's owner was
  never read. Done when a run sampled from outside the process names it,
  which needs a count of the engine's mapped blocks by length.
- [ ] **What each engine's checksums verify.** RocksDB reads with
  `verify_checksums(false)`, in memory, from its files out of core and in
  `rocksdb-tuned-enc`, whose counter-mode Env has no tag, while a store
  under a cipher checks a tag on every chunk it reads. Done when RocksDB
  reads with its default, or the methods say what each engine verifies, the
  encrypted arms included.
- [ ] **A kill in a rep taken again.** `Ledger::forget_rep` deletes a cut
  rep's kills with its passes, so a row can lose a kill. Done when the row
  keeps them and the gate reads them.
- [ ] **Device bytes measured at the device.** "Device bytes written" is
  the process's `/proc/self/io` count, which equals bytes on disk for
  buffered arms, while `bench/DESIGN.md` calls it the device's counter.
  Done when it reads the device, or is renamed.
- [ ] **The scan pass after a lent burst.** Under the held log a burst is
  lent to the upkeep thread, and the first scan waits for the thread's pass
  and settles its trail: the pass after a full rewrite reads 0.15x to 0.35x
  of the filed store's. Let the scan settle only the blocks it walks while
  the thread files the rest. Done when that pass reads level with the
  filed store's in both orders, with the bursts at the lent rate. Design:
  `docs/engine.md`, "What a burst costs under the held log" and "What
  filing a write costs".
- [ ] **The forms stood down through point operations.** Under the held log
  the point mixes on one thread trail FASTER (A, B, F and the scrambled
  mixes, in the quick row at `33bc663`), because the scans' forms are kept
  current beside every commit. Stand them down while the workload is point
  operations and bring them back when scans come. Done when A, B and F read
  level with `supdb-heldinplace` in both orders at a hundred and three
  hundred thousand keys, with E and the scan passes no slower. Design:
  `docs/engine.md`, "The point mixes on one thread, by what is taken away".
- [ ] **The upkeep thread's alertness beside point writes.** Backed off,
  the thread costs A a sixth less and E an unresolved amount. Done when E's
  price is counted at ten, a hundred and three hundred thousand keys and
  the alert is keyed on what E needs. Design: as above.
- [ ] **The one writer's get beside FASTER's.** C on a clean store reads
  ahead in the quick row at `33bc663`; F's gets still trail. Done when C
  and F's gets read level or ahead in both orders of a pairing at a hundred
  thousand keys. Design: `docs/engine.md`, "Past FASTER: what an operation
  costs".
- [ ] **The threaded mixes at three hundred thousand keys.** `threads-A` on
  one thread reads 0.85x of FASTER in the quick row at `33bc663`; the cells
  at ten thousand keys that trailed before read ahead there. Done when a
  pairing in the row's order reads every threaded cell level or ahead.
- [ ] **The held log's small bursts.** The one-percent burst at ten
  thousand keys reads 0.85x of FASTER and the full rewrite at a hundred
  thousand 0.96x in the quick row at `33bc663`. Done when both read level
  or ahead in both orders.
- [ ] **A shared table left when its handles stop writing.** It is left
  only at the writer's own commit or seal, so a store whose handles stop
  writing scans by merging until the writer commits. Done when such a
  store scans at the block cache's rate with no commit. Design:
  `docs/engine.md`, "A live table that follows who writes it".
- [ ] **The buffered arm against LMDB without sync.** The quick row at
  `33bc663` reads its scan pass at 0.93x to 0.96x, its scrambled E at
  0.89x and the pass after a full rewrite at 0.88x at three hundred
  thousand keys. Done when each is paired in the row's order and won or
  decomposed.
- [ ] **The durable loads at the small rungs.** The ordered load reads
  0.89x to 0.92x of RocksDB at ten and a hundred thousand keys, and the
  shuffled load 0.94x at ten thousand, where the seal hands the writer its
  records one at a time. Done when both are ahead in both orders of a
  pairing in the row's shape.
- [ ] **Buffered bursts at a million keys and up.** The full row at
  `a728c4c` reads the buffered arm's lag bursts at 0.62x to 0.84x of
  RocksDB's at one and three million keys, and its one-percent burst below
  the durable arm's. Done when they read ahead.
- [ ] **Scans on a store with unsealed keys spread through it.** A few
  unsealed keys in a partition take every scan of it off the bulk walk, a
  fifth to a quarter slower than the same store folded; over random ids
  the threaded scans after the mixes trail LMDB. Done when those scans
  read level with the scan pass before them, over ids and numbered keys.
- [ ] **The ordered load beside the builds ahead.** It reads 0.85x to
  0.88x of `supdb-nozpages` at ten million keys under both bounds. Done when
  it reads level, or the trade is priced against `supdb-noland` in the
  roster. Design: `docs/engine.md`, "Out of core: the second form".
- [ ] **The pass after a full rewrite under a bound.** Over random ids at
  ten million keys under 1 GiB it reads 0.6x to 0.9x of LMDB: the merge
  debt written beside it, and a working set larger than the bound. Done
  when it is ahead in both orders with the burst no slower. Design:
  `docs/engine.md`, "Out of core: keys that are no run of numbers".
- [ ] **The crossing off the landings' thread.** The crossing's asks run
  on the thread every landing goes through, beside the first reads. Done
  when they run off it and the read pass after the load reads one shape.
- [ ] **The memos of decoded groups in slabs.** `zpages_memo_slab`
  returns the memos' memory and is off, since the hot-raw policy's room is
  drawn from memory the slab moves. Done when the room is drawn from
  something else and the slab is priced again at both ten-million bounds.
- [ ] **A fall back held by a segment's last key.** In the clear an open
  sweeps an unnamed segment newer than every segment its slot names where
  that slot's segments or a run's temp name hold its last key. A merge
  output past what the slot covers that folded in a run's piece, whose temp
  name the newer slot retired, reads as a seal's and is swept: under media
  damage, its writes are lost. Done when a segment says in its bytes that it
  is a merge's output and an open shows no merge in the crash window past
  its slot.
- [ ] **What `sync` answers after a seal fails.** A seal that fails at any
  step stops the seal queue for the open's life (`Shared::seal_wedged`).
  `flush` and `settle` answer it; `sync`, whose promise is in the log,
  answers nothing of it until a write is dropped for want of room in the
  frozen tables, so a caller that syncs and never flushes learns of it from
  one commit. Every `sync` answering it, with commits still `Ok`, is the
  other contract. Done when one is chosen, stated on `Db::sync`, and tested
  at each step a seal can fail.
- [ ] **A store under a cipher under a bound.** Its close and a read's
  settle, which killed it at five million keys under 640 MiB, hold nothing
  of the cache: twelve lag sweeps of twelve passed at `471ec44`, at worst
  25-58 MiB from the limit. A seal's output and a run's file reach the
  cache's room unannounced; `supdb-cipherauto` was lost once at ten million
  keys under 1.25 GiB (`26315ea`). Done when ten sittings of the lag sweep
  and the mixes, over numbered keys and ids, kill nothing at three and ten
  million keys under 1 GiB and 1.25 GiB.
- [ ] **The page layer.** The cipher runs through the engine as an envelope
  a file, a memory file while it is written and a sealed record in the log,
  so a run's double write, an envelope written after its segment, a form
  page across two chunks and an unclassed file lost at the close are each a
  special case. Wanted: one module behind the `vault::` door that knows
  pages, lengths, sync points, sealed names and durability classes and no
  format, storing a page of plaintext as one page, its tag apart. Done when
  `supdb-cipher` is that shape. Design: `scratch/onefile/design.md` at
  `5b81772`.
- [ ] **An envelope written beside its segment.** A segment under a cipher
  is encrypted and written once finished, since the envelope's check binds
  the file's length; a plain one reaches the device as it is built. Wanted:
  a second layout for a store's own files, the published `crypt` envelope
  staying version 1, whose chunks do not bind the length and whose last
  chunk says it is last. Done when files the cache has no room for are
  written with no memory file, priced on the pass after the full rewrite
  against `rocksdb-tuned-enc`, 0.55-0.82x at five million keys under 640 MiB
  (`bfd3818`).
- [ ] **An envelope whose chunks are whole pages.** A chunk is a stored page
  less its tag, so a form page is two pages of ciphertext to a cold read;
  under a bound the bought block form read 0.73x of the raw form. With tags
  kept apart a chunk is a page and the cache gives back by the page: the
  same layout as an envelope written beside its segment. Done when the block
  form under a cipher (`ZPages::Always`, stride form off) reads ahead of the
  raw form there at five million keys under 640 MiB in both sittings.
- [ ] **An ordered run under a cipher as its own log.** Its records are
  written twice, as log frames and into the segment's envelope at the close;
  the ordered load reads 0.89-0.97x of `rocksdb-tuned-enc` at five million
  keys under 640 MiB (`bfd3818`). Seal a commit's whole chunks as it is made
  and its partial chunk as a record behind the run's frontier, and let the
  open read the envelope to its last whole commit. Done when such a run
  writes no log frame and `tests/cipher.rs` builds its torn states as
  `tests/runlog.rs` does a plain run's.
- [ ] **The store in one file.** Segments, ordered indexes and form files
  can be extents of one data file (`Options::one_file`, off), in the clear
  and under a cipher. Left: the log, the manifest's slots and an ordered run
  in the file; a full-copy checkpoint; mechanical mutants over
  `src/db/onefile.rs`; and what leans explained, among it the three-thread
  read pass under 1 GiB at 0.94x and the pass after the full rewrite under a
  cipher at 0.32-0.43x. Done when the default is set where the suite reads
  it better and the gate passes, or the layout is refuted. Design:
  `scratch/onefile/design.md` at `5b81772`.
- [ ] **The ordered load under a cipher.** In the encrypted quick row at
  `cc1af6a` (`bench run --scale quick --arms enc`) the durable load trails
  both encrypted comparators at ten thousand keys (0.66x) and reads
  0.83-0.94x, unresolved, above; the buffered one trails encrypted LMDB at
  every rung (0.65-0.75x) and encrypted RocksDB at ten thousand (0.75x).
  Done when it leads both from thirty thousand keys in both groups, and at
  ten thousand stands to them as the plain store to the plain ones. Levers:
  a run as its own log, an envelope beside its segment, a log file retired
  off the drain.
- [ ] **A log file retired off the drain.** A drain whose promotion or merge
  publishes a manifest retires the closed log files behind it before it
  answers, and each is an inode freed, a device round trip the size of the
  file. An ordered run under a cipher fills the log, so its drain pays it; a
  plain store's pays wherever a closed log file waits at such a publish.
  Done when the encrypted ordered load is priced with no retire ahead of the
  drain's answer, by the sum of the phases that inherit the free.
- [ ] **The loads with the controller on under a cipher.** Shipped on, its
  builds during a load bring the loads to 0.79-0.92x of the store with it
  off at every bound in both orders, the shuffled load the lower
  (`bench/scripts/pair.sh supdb-cipherauto supdb-cipher SIZE --bound B` at
  `9b5c4f5`). The ordered load's writer blocks as often, each block some 26
  µs longer, on a wait not yet read: the log's sync beside the builds'
  envelopes, or a hand-off. Done when the loads read level with the store
  with it off at every bound in both orders.
- [ ] **The lag sweep with the controller on under a cipher.** Against the
  same store with it off at `9b5c4f5`, in both orders: the full rewrite's
  burst reads 0.49-0.62x, the pass after the one-percent burst 0.40-0.47x,
  and under 640 MiB and 1.25 GiB the clean store's pass 0.82-0.85x and the
  ten-percent pass 0.20-0.72x. With the controller on the store reads as out
  of core, the writer gives up its block forms, and the sweep's scans take
  the merge path through block-coded form files. Done when each reads level
  with the store with it off, at every bound in both orders.
- [ ] **The pass after a full rewrite under a cipher, under a bound.** At
  ten million keys under 1.25 GiB it reads 0.87-0.96x of `rocksdb-tuned-enc`
  at `2027778`, its scans behind the burst's merges. With the merges' asks
  laid to their inputs' regions it read about twice RocksDB's rate in four
  reps of six, unpaired; the other two left the burst's debt to the pass. A
  merge under a cipher costs 342 ms of CPU to the plain store's 196. Done
  when it is ahead in every rep in both orders, there and at five million
  keys under 640 MiB.

## Low priority

- [ ] **The first scan after a burst.** It waits for the upkeep thread's
  pass in flight and settles what the thread trailed by, and a merge the
  idle plan starts can run beside it. Done when that scan costs what the
  scans behind it do, at three million keys in memory and under both
  bounds, with no burst slower. Design: `docs/engine.md`, "The first scan
  after the buffered burst".
- [ ] **The settle's cost a key.** Filing a write into its block's form
  is what E's commit and a burst's first scan pay. The buffered scrambled E
  at ten thousand keys reads level with LMDB and E's commit costs twice
  LMDB's. Done when both are ahead under both policies. Design:
  `docs/engine.md`, "What filing a write costs".
- [ ] **A light form's conversion with a trigger of its own.** A form under
  the dense count is converted only on a walk something else started, so a
  store of light forms converts none. Done when the conversion is started
  by scans reading the form, priced on the scrambled E at every rung.
  Design: `docs/engine.md`, "A settle's conversion leaves the fill its
  walk".
- [ ] **The index layouts the study found.** Done when one is built behind
  an option and priced in the suite's shape, or refuted there. Design:
  `docs/index-theory.md`.
- [ ] **The writer's reads in the census.** No arm differs from the
  default in `zpages_writer_census` alone. Done when one does and the A and
  F mixes are priced with it.
- [ ] **A store past its budget that is only scanned.** The controller
  buys on point reads' faults, and a sweep of bursts and scans has none.
  The banked rows show no sweep starting raw; the pairings did. Done when
  the controller's trace says what decides the purchase and every rep
  starts on the form the budget calls for.
- [ ] **The builds ahead at the half-budget line.** They start where the
  store passes half a budget that moves with the process, so a store on
  the line is bet on by chance. Done when the bet is called off under the
  selector's line, or the line ignores the engine's transient memory.
- [ ] **The full rewrite's burst at three million keys under 1 GiB.** The
  upkeep thread builds copies from raw records the cache let go, and the
  sampler samples for a purchase no read is near. Done when the burst takes
  the faults of its fast reps in every rep, with the pass after it no
  slower.
- [ ] **Keys that are no run of numbers.** The sparse stride form is not the
  default anywhere, the block form's threaded scans trail LMDB over ids, and
  `supdb-nocache` was killed once. Every ids row is at `a728c4c`, before #28
  to #35. Done when an ids row is taken again and each has its pairing.
  Design: `docs/engine.md`, "Out of core: keys that are no run of numbers".
- [ ] **A budget read where the load ends.** The selector's buy reads the
  budget with the process at its smallest. Done when it counts what the
  engine's structures are set to hold, as the stride choices do.
- [ ] **The trim under a bound.** `forms_to_fit`'s trim returns freed
  memory while the store runs. Done when it is priced under a bound and
  made the default there or refuted.
- [ ] **The full ladder past memory.** `bench/DESIGN.md` promises `full`
  to 1.5x the machine's memory; its rows stop at three million keys. Done
  when a `full` row reaches it.
- [ ] **A full row with the held log's changes.** No `full` row since
  `a728c4c`, and none carries the held arm or FASTER. Done when one is
  banked and FASTER's group is read against the quick rows.
- [ ] **FASTER's rates between two bounded rows.** FASTER's point mixes
  read a third to a half higher at `33bc663` than at `001781d` with its
  configuration unchanged. Done when the move is laid to the host, and the
  held arm's to the engine changes between the rows.
- [ ] **The crash seam's threads with no hold.** A piece merge, the
  builder ahead, the sampler and a form file's warm have no hold to put
  them in flight at a crash, and the wake of the threads that park between
  turns survives its mutant. Done when each mutant fails.
- [ ] **Two states of the settle no test builds.** A frozen batch cut by a
  budget, and the cache's bind at the fill's check. Done when a test builds
  each and its mutant fails, or the path is shown unreachable and removed.
- [ ] **Concurrent writers with conflict detection.** Transactions from
  several threads that commit or are refused on a conflict, as RocksDB's
  `TransactionDB` does, where today one writer applies every batch.
  Builds on the conditional write.
- [ ] **Reverse and bounded scans.** `scan` is forward from a start key
  with a count. Add an end bound and reverse order, beside `scan_page`.
- [ ] **Range delete.** Deleting a key range without a tombstone per key.
- [ ] **Batched point reads (`MultiGet`).** Several keys in one call, and a
  workload for it.
- [ ] **Column families.** Independent key spaces in one store, sharing
  its log and committed together.
- [ ] **Wide columns.** Named columns within a key's value, read and
  written apart.
- [ ] **User-defined timestamps.** A version a caller supplies, readable as
  of a timestamp.
- [ ] **TTL and compaction filters.** Values that expire, and a hook that
  drops or rewrites records as merges pass over them.
- [ ] **Tracing and replay.** Record a store's operations and replay them
  against another store or engine.
- [ ] **A log's device errors held on a device that makes them.** A failed
  log write is written again and a failed log sync kept until reopen
  (`SyncPolicy`), held only through seams that model Linux's. Done when a
  dm-flakey or dm-error target under the store's filesystem (a loop device,
  as `scripts/fscrypt.sh` sets one up) fails the log's writeback mid-stream
  and the store answers every later commit with it and reopens at a prefix
  of what it acknowledged; when after ENOSPC it commits once space is freed;
  and the same for a direct run's file and its helper's sync.
- [ ] **A log's power cuts held on a device that loses pages.** The tests
  build each state a power cut leaves by hand, from a copy of the page cache
  and the engine's trace of its syncs (`Db::trace_wal_syncs`); which states
  ext4 and xfs reach is reasoned, and the first-sector rule takes a sector
  to be kept or lost whole. Done when a dm-flakey target that drops
  unflushed writes, or a dm-log-writes replay, cuts power after a kill, a
  reopen, a `sync`, a fresh store, a rotation and an unsynced flush, and
  every open reads what was acknowledged. Design: `docs/engine.md`, "Crash
  injection".
- [ ] **A reopen with nothing replayed held to no fdatasync at the
  syscall.** The tests read the log's syncs through the engine's own trace
  (`Db::trace_wal_syncs`), so a sync written straight into the open's path
  records nothing and passes every test; `strace` over
  `a_reopen_with_nothing_replayed_owes_its_log_no_sync` was read by hand.
  Done when a test counts the process's fdatasyncs of the store's files
  across an open, with no seam the path must remember to call.
- [ ] **A seal's failed sync held on a device.** The tests inject each
  failure of a seal through seams that skip the step they fail; none has met
  a real writeback error under a seal, or a directory fsync that fails while
  the filesystem goes on. Done when a dm-flakey or dm-error target fails a
  seal's fsync after its readable landing and the store, no manifest naming
  the seal, reopens to every acknowledged write. A store that writes the
  failed table again under a new name and goes on is built only if one asks
  for it.
- [ ] **A torn manifest slot on a device that tears one.** A publish writes
  the slot that does not hold the newest manifest known durable, and an open
  fdatasyncs both as it found them (`SlotChoice`); the tests build the tear
  by hand (`Db::watch_slot_writes`), and no device here has been seen to
  tear a slot. Done when a dm-flakey target drops or tears the writes after
  a chosen point inside a drain with its slot owed and a merge publishing
  behind it, and the store opens to every acknowledged write. Design:
  `docs/engine.md`, "The manifest in two slots".
- [ ] **The open's directory fsync held on a device.** An open that adopts
  pending segments (`Options::manifest_pending`) fsyncs each, its ordered
  index under `ord_durable`, and the directory before the slot that names
  them durable; the tests hold the order through the open's own records
  (`Db::open_synced_files`, `Db::trace_dir_io`) and, as root, the kernel's
  page flags, and nothing reads what reached the device. Done when a
  dm-flakey target that drops unflushed writes cuts the power after such an
  open and the store reopens with every acknowledged write, the directory
  fsync removed as the control.
- [ ] **What a guard does not cover.** A failed directory barrier on the
  writer's path is made good by guards (`vault::guard`) and the writes go
  on, or the store leaves `dir-lost` for the next open to guard and sync.
  The segment work's own directory fsyncs, a seal's landing and the rename
  shape's publish, stop the seal queue where they fail instead: every write
  stays durable and the store takes none until it is reopened. Done when
  those are guarded and the store goes on writing, with a test at each crash
  window the guard adds.
- [ ] **An open after a crash, priced by its parts.** An open that finds a
  merge held at its landing by the crash costs over half again an open of
  the same store with no slot synced and nothing checked; folding the check
  to segments whose fences take each key did not move it. The check opens
  each unnamed segment whole (`Blob::open`, its checksum row) for one key,
  and each holder whole. Done when the open's phases are stamped and, if the
  whole opens are the cost, a last-key read checked only where it reads is
  priced against them.
- [ ] **A write's first touch of the memtable's index.** Through the lag
  sweep's full rewrite at ten million keys under 1.25 GiB (`perf record -a
  -g` cut by `SUPDB_LAG_MARK`, at `26315ea`), 12.5% of the plain writer, 15%
  under a cipher, is in cross-CPU page-table flushes under
  `MemTable::index_insert`: the probe reads a never-written page of the hash
  index, which maps the shared zero page, and the insert's write copies it
  and flushes the entry from every CPU. Done when a shape that writes the
  index's pages before it reads them is priced against it in the suite's
  shape.
- [ ] **A read of one key larger than the cache.** What a read is lent it
  holds until it ends or rests, and one key's values are read in one piece,
  so under a cipher a key whose values outrun the store's cache holds the
  cache past its budget for that read. Done when a walk of one key's runs
  rests between blocks, asking again for the extents it walks by, and a test
  reads such a key with the cache held to its budget.
- [ ] **A miss at less than a fault.** A chunk read into the store's cache
  under a cipher is a fresh page of a memory file: the kernel's fault on it
  is over half of a miss, the copy of the ciphertext a fifth and the cipher
  a tenth. Done when a run of chunks read in order is populated in one call
  and decrypted as one, which wants the cache to know a run when it sees
  one, and the reads out of core are priced with it.
- [ ] **The plaintext a store under a cipher holds follows the room.** Where
  the cipher names no size, the store's cache follows the room: what the
  store holds plus what the process may still hold that the kernel cannot
  take back, read every twenty milliseconds (`pager::Room`). Two stores in
  one process share the room in equal parts, which no test holds, and a
  caller whose own memory grows beside the store is unpriced. Done when a
  test holds two stores to their shares and a caller that grows beside a
  store under a bound kills nothing.
- [ ] **The heads' copy under a cipher.** The copy of a partition's
  ordered-index heads in the process's own memory (`Options::heads_owned`)
  is made from the index's mapping, which a store under a cipher does not
  lend, so none is made there: every seek reads its heads through the
  store's cache, and the look still asks the kernel for the heads'
  ciphertext before finding nothing to copy. Done when the copy is made by
  asking the index for its heads by the piece, and priced under a bound
  against the store without it.
- [ ] **The controller's model held to a measurement under a cipher.** Under
  a cipher its budget is two caches in series, the store's own and the
  kernel's of the envelopes; at five million keys under 640 MiB it predicted
  no fault through the form where the census read 0.6 a read, and did not
  leave the form in five million reads. Done when the model's prediction for
  each form is printed beside the census there and the two agree, or the
  selector's exits act on the difference.
- [ ] **The controller under a cipher prices the kernel's cache alone.** It
  reads the group's budget, refaults and the thread's major faults, which
  under a cipher are the kernel's cache of the envelopes. A store that fits
  memory but not its own cache of plaintext (`StoreCipher::cache_bytes`)
  takes two chunks decrypted for a raw point read and one and a decode
  through the form, and the controller sees none of it. Done when the form
  is priced against the raw form for such a store, and the controller reads
  the store's cache where the form pays.
- [ ] **A store under a cipher on another platform.** The memory files a
  file is written in and the cache gives pages back from are Linux's
  (`memfd_create`, `MADV_REMOVE`), so elsewhere `Encryption::Cipher` is
  refused before it writes anything. Done when a store under a cipher opens
  and passes `tests/cipher.rs` on macOS.
- [ ] **A store under a cipher, in its cache, at the plain store's rate.**
  With the whole store in its cache a reader under a cipher still asks for
  every range before it reads it: point reads trail the store in the clear a
  little, scans further, furthest where a walk reads by rank. Done when
  walks by rank and through the stride form ask a window at a time, a point
  read asks once a piece, and a read's start and end are a look, each shape
  counted on the plain read first (`bench/scripts/pair.sh supdb-cipher supdb
  300000`).
- [ ] **A store under a key, priced on a kernel with hardware AES.** The
  pairings of `supdb-dirkey` against `supdb` ran where the kernel has no AES
  instructions (`/proc/crypto` names `aes-fixed-time`): reads, scans and
  read mixes level, and every synced write at about half, as a bare write
  and `fdatasync` were in the same two directories. Done when the pairing
  has run on a kernel that names `xts-aes-aesni`.
- [ ] **An envelope written in one pass.** Publishing a segment encrypted
  writes it plain in memory and encrypts that into the file, so
  `supdb-segment-enc` loads at about six tenths of `supdb-segment`. Done
  when the segment writer writes the envelope itself.
- [ ] **The builds ahead's last cold partition under a cipher.** At five
  million keys under 640 MiB one or two partitions are cold at the load's
  end in most reps, and in one in eight several are unbuilt and the reads
  run beside their builds: past the selector's line a build ahead takes the
  oldest partition due, a cold one among them, and the warm ones wait behind
  it. Taking warm ones first moves the plain store's order too. Done when
  that order is priced on the bounded roster's rungs and every rep's load
  ends with its partitions built.
- [ ] **The open after a crash, under a cipher.** In the encrypted quick row
  at `cc1af6a` the `recover` open under a cipher takes 1.2x the plain
  store's at ten thousand keys and 2.6x at three hundred thousand after an
  ordered load, 1.4-1.7x after a shuffled one, and leads encrypted RocksDB
  at every rung. A run under a cipher is replayed from the log, having no
  file to finish, and what else the open decrypts is unstamped. Done when
  the open's phases are stamped over both stores and it opens within its
  decryption's cost of the plain one.
- [ ] **The encrypted roster past the quick scale, and the kernel's side.**
  The encrypted row is the quick scale's, its bounded cells two pairs an
  order at one bound, and encrypted LMDB stops at three hundred thousand
  keys. No comparator has run on a directory the kernel encrypts, beside
  `supdb-dirkey`, and this guest's kernel has no AES instructions. Done when
  a `full` row of the roster is taken with encrypted RocksDB, the bounded
  scale's bounds are paired against it, and the three stores are paired on
  an encrypted directory under a kernel that names `xts-aes-aesni`.
- [ ] **The threaded mixes in the encrypted roster.** `threads-A`,
  `threads-RMW` and their uniform twins (`bench run --passes
  mixes-threaded`) are in no encrypted row. `supdb-cipher` and encrypted
  RocksDB run them; `lmdb3` and `lmdb3-enc` refuse writes from threads
  (`Engine::thread_writers`), the arm being LMDB's C API with a decrypting
  context a thread. Done when the two LMDB arms take them, a test of three
  threads reads back every value they wrote, and the encrypted roster
  carries the cells in both groups.
- [ ] **The log's sealing under the door.** Under a cipher the log seals its
  own writes (`Wal::enc`, `Wal::write_sealed`, a header of its own length),
  so each mechanism added to the log's write path must be taught the sealing
  by hand. Done when the log writes to an append file the vault hands it,
  which seals what it is given and cuts back what a failed write left, and a
  store under a cipher passes the log's tests and the crash oracle with no
  branch on the cipher in `Wal`. The cache's give-back (holds, rests,
  `unlent`) is narrowed after.
- [ ] **Every mechanism held to acting under a cipher.** A new whole borrow
  of a section, or a new caller of a form's whole-file read, passes every
  gate and does nothing under a cipher, as the stride form, the heads' copy
  and `Options::merge_parallel_quiet` did. Done when a `scripts/check.sh`
  group runs every plain store's test under an identity cipher, eviction at
  every rest and `SUPDB_PAGER_GUARD` on; the differential fuzz does so at a
  small cache, drawing or exempting every `Options` field; and the asks of a
  read, a scan entry and a merge step are a list that only goes down.
- [ ] **The stride readers' borrow held by its type.** Under a cipher the
  stride form's readers index a borrow that spans the store cache's whole
  range, among pages other threads fill, so it is sound because each page is
  asked for before it is read (`MmapBytes::lend_asked_map`'s safety note),
  not because of the borrow's type. Done when the readers are generic over a
  source that hands each range, as `flatindex`'s are over `Sect`, with the
  plain store's eight readers counted beside it.
- [ ] **A one-file build's reads bounded to its own extent.** In the one
  data file a build's reads through a mapping are read around and ahead by
  the kernel past its own file into the next, so a second build ahead reads
  the first's dropped file back as fast as it drops it, and that form file
  is attached checked page by page instead of trusted. The tests that hold
  every word take one build at a time there (`one_build_ahead_in_one_file`).
  Done when every read a build makes in the data file is bounded to its
  extent, the mapping advised random and asked ahead in pieces, and the
  one-file arm's builds are priced with it.
- [ ] **The hosted gates green on main.** At #35's merge (`09cadec0`) the
  x64 `test` leg failed `tests/settle.rs`'s
  `a_copy_of_fixed_size_records_is_one_stream_until_a_write_it_cannot_take`,
  its copies dropped across a merge that kept their keys; here it failed
  once in about 150 whole runs of the binary. At #35's head the ARM leg had
  failed a `tests/durable.rs` case not reached. The door's libc test fails
  a vendored or offline registry. Done when every job of `ci.yml` passes at
  main's head and the settle state is held through a seam.
- [ ] **The count's verdict by function.** `sh scripts/cost.sh` exits red by
  function, so a function the other build inlines reads as a gain beside the
  caller that lost more; loop-head padding counts as instructions
  (`work_probe::main`, `compact_run` red with no hot instruction changed); a
  monomorph over a renamed type reads as two functions; and the block form's
  open path stands over upstream from an unchanged source. Done when
  `scripts/cost-diff.py` holds declared groups of functions to their net
  against one named base, sums leave padding out, each with a red input, and
  a tree that pays nothing reads green.
- [ ] **Main's #30 to #33 held under a cipher.** Handles that write a shared
  live table (`Options::shared_writers`), drains answered at the seal's
  readable landing (`drain_answers_readable`), copies claimed by key
  sequence (`identity_by_keys`), packed copies, frozen tables held to bytes
  and a run's file made ahead came from main unread under a cipher. Done
  when each is read for which bytes it reads and through what, and held
  under a cipher by a test where it reads a file, first a shared table's
  seal and a drain answered early beside a crash in `tests/cipher.rs`, and
  the differential fuzz runs `ZFUZZ_SHARE` under a cipher.
- [ ] **Timing rows against the store before the encryption merge.** "No
  impact for a store with no encryption" was held by instruction count,
  change by change, and a count sees no stall. Done when the quick, full and
  bounded rows at `09cadec` stand beside the same rows at `99d44af` on one
  host, and no cell of the plain arms is on the worse side.
- [ ] **The `zpages` tests' waits under load.** Four copies of the checked
  `zpages` binary at once fail most rounds, before and after the encryption
  work alike, in tests that wait on the wall clock or a count of rounds for
  a controller state
  (`scans_after_a_quiet_stretch_have_the_cold_form_files_asked_for` most
  often); one copy passes. Each moves to a wait on the state
  (`drive_to_built`, `tick_until_built`) test by test, since a bulk change
  broke tests exact to a tick. Done when four copies at once pass three
  rounds.
- [ ] **Defaults inherited unpriced under a cipher.** `bounds_by_heads` and
  `land_bounds_read` were made defaults on pairings of plain stores, and the
  cipher arms run them with no arm that has them off. Done when each is
  paired under a cipher against the same arm without it, at three hundred
  thousand keys and under a bound at ten million.
- [ ] **What no second reader has read.** The last repair of several units
  of the encryption work was read only by whoever launched it: the wall, the
  door, the count, the lists' check, the count against the other line, the
  seal's rebuild, the `cross-arm` group's repair and five durability pricing
  units; `a39aaaa5` had one reader. Done when `git diff 99d44af 09cadec --
  src tests/source.rs scripts` has had a review by someone who did not write
  it.
- [ ] **Small things left where they were found.** `key_eq`'s arm under
  sixteen bytes compares two empty slices by a call of no bytes.
  `compact_run` reads `record_offset(0)` of every input of every merge,
  through the cache under a cipher, and drops it. Three tests in
  `tests/blob.rs` hand closures straight to `Blob::scan_at`.
  `zfile::ask_file`'s allowance covers any barred call added to it. The
  ordered index skips its checksum over any source that lends by the piece
  without asking `Bytes::authenticated`. The store's cache counts no advice
  (`Paging::willneed`, `forget`). Three red inputs outside the tree run
  nowhere. Done when each is fixed or run.
- [ ] **A threat model, written down.** A `SECURITY.md`: how to report; who
  is trusted (the embedding process, the live store's directory) and who is
  not (keys and values through the API, any file handed to the read-only
  readers or the wasm module); what each `Encryption` mode protects; what
  counts as a vulnerability; what an embedder must do; and what is said and
  not fixed: the unkeyed index hash, a blob replaced only by rename, the
  browser's short-lived read-only credentials. Done when it is written,
  after the reporting channel and whether `Db::open` of a copied store is in
  scope are decided.
- [ ] **The read-only parsers fuzzed behind a valid checksum.** `Blob`,
  `SparseBlob` and the wasm ABI parse objects fetched from elsewhere, and a
  CRC stops an accident, not an author. The key section is forged under
  recomputed rows (`tests/forged.rs`), every reader held to no panic and no
  endless read. Still to forge under their own checksums: the block table, a
  block's chunks and the superblock's two slots; and the wasm ABI is not
  driven at all. Done when each is, with a known-red run for each.
- [ ] **The checked LZ4 decoder on the path an untrusted blob reaches.**
  `lz4_flex` is built without `safe-decode`, so `src/block.rs` decodes
  blocks of a file the reader did not write with the crate's unsafe decoder,
  which bounds-checks its input but is unsafe code fed an author's bytes.
  Done when the checked decoder is priced in the suite's shape and taken on
  that path, or the forged blocks above hold the unchecked one.
- [ ] **No key or value can panic or stall the store, as a contract.** The
  differential fuzz found the empty key and the long common prefix; the
  guarantee is stated nowhere. Done when the crate docs state it and the
  fuzz draws from the whole of the key and value space under it.
