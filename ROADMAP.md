# Supdb roadmap

Open work, one item each: what it is, when it is done, and the design note
where there is one. Figures name the row they are read from.

## Engine: API and correctness

- [ ] **A scan's limit counts the keys it answers.** `scan(from, limit)`
  counts deleted keys the store still records, so a page comes back
  short: with eight of the first sixteen keys deleted, a scan of ten hands
  over five values. Count only keys that hold a value, and add
  `scan_page`, which answers where the next page starts. Open: whether a
  partition may hold a record with no value, a format change. Done when the
  differential fuzz pages a store end to end with deletes unsealed, in a
  piece and in a partition, and the scan pass and ycsb-E are priced.
- [ ] **Open in time independent of the store's size.** Every open
  verifies each segment's whole key section against its checksum row, and
  with inline records that is the whole segment: four fifths of the open's
  instructions, about 10 ms at 300 thousand keys and 90 ms at three
  million, so recovery trails LMDB by more as the store grows. Verify each
  16 KB piece at first use, as the sparse reader does, or after the open.
  Done when `open_ms` does not grow with the store and a damaged piece is
  still refused before any read of it answers.
- [ ] **A conditional write.** A put or delete that applies only if the
  key is as its caller read it, on the writer and in a `Txn`.
  `Reader::rmw` exists for handles under the held log, and is atomic only
  where the live table holds the key's whole read. Done when a refused
  write leaves the store and its batch as they were, threads
  read-modify-writing a few hot keys lose no update, and `threads-RMW` runs
  every arm through it. Design: `docs/engine.md`, "A live table every
  writer writes".
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

## Engine: performance

- [ ] **The scan pass after a lent burst.** Under the held log a burst is
  lent to the upkeep thread, and the first scan waits for the thread's pass
  and settles its trail: the pass after a full rewrite reads 0.15x to 0.35x
  of the filed store's. Let the scan settle only the blocks it walks while
  the thread files the rest. Done when that pass reads level with the
  filed store's in both orders, with the bursts at the lent rate. Design: `docs/engine.md`, "What
  a burst costs under the held log" and "What filing a write costs".
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
- [ ] **The index layouts the study found.** Done when one is built behind
  an option and priced in the suite's shape, or refuted there. Design:
  `docs/index-theory.md`.

## Out of core

- [ ] **The ordered load beside the builds ahead.** It reads 0.85x to
  0.88x of `supdb-nozpages` at ten million keys under both bounds. Done when
  it reads level, or the trade is priced against `supdb-noland` in the
  roster. Design: `docs/engine.md`, "Out of core: the second form".
- [ ] **The writer's reads in the census.** No arm differs from the
  default in `zpages_writer_census` alone. Done when one does and the A and
  F mixes are priced with it.
- [ ] **The pass after a full rewrite under a bound.** Over random ids at
  ten million keys under 1 GiB it reads 0.6x to 0.9x of LMDB: the merge
  debt written beside it, and a working set larger than the bound. Done
  when it is ahead in both orders with the burst no slower. Design:
  `docs/engine.md`, "Out of core: keys that are no run of numbers".
- [ ] **A store past its budget that is only scanned.** The controller
  buys on point reads' faults, and a sweep of bursts and scans has none.
  The banked rows show no sweep starting raw; the pairings did. Done when
  the controller's trace says what decides the purchase and every rep
  starts on the form the budget calls for.
- [ ] **The builds ahead at the half-budget line.** They start where the
  store passes half a budget that moves with the process, so a store on
  the line is bet on by chance. Done when the bet is called off under the
  selector's line, or the line ignores the engine's transient memory.
- [ ] **The crossing off the landings' thread.** The crossing's asks run
  on the thread every landing goes through, beside the first reads. Done
  when they run off it and the read pass after the load reads one shape.
- [ ] **The full rewrite's burst at three million keys under 1 GiB.** The
  upkeep thread builds copies from raw records the cache let go, and the
  sampler samples for a purchase no read is near. Done when the burst takes
  the faults of its fast reps in every rep, with the pass after it no
  slower.
- [ ] **The held arm through a full rewrite at ten million keys.**
  `supdb-heldpart` was killed once under 1.25 GiB, and the kill's owner was
  never read. Done when a run sampled from outside the process names it,
  which needs a count of the engine's mapped blocks by length.
- [ ] **The memos of decoded groups in slabs.** `zpages_memo_slab`
  returns the memos' memory and is off, since the hot-raw policy's room is
  drawn from memory the slab moves. Done when the room is drawn from
  something else and the slab is priced again at both ten-million bounds.
- [ ] **Keys that are no run of numbers.** The sparse stride form is not
  the default anywhere, the block form's threaded scans trail LMDB over
  ids, and `supdb-nocache` was killed once. Every ids row predates four
  pull requests. Done when an ids row is taken again and each has its
  pairing. Design: `docs/engine.md`, "Out of core: keys that are no run of
  numbers".
- [ ] **A budget read where the load ends.** The selector's buy reads the
  budget with the process at its smallest. Done when it counts what the
  engine's structures are set to hold, as the stride choices do.
- [ ] **The trim under a bound.** `forms_to_fit`'s trim returns freed
  memory while the store runs. Done when it is priced under a bound and
  made the default there or refuted.

## Benchmark

- [ ] **The full ladder past memory.** `bench/DESIGN.md` promises `full`
  to 1.5x the machine's memory; its rows stop at three million keys. Done
  when a `full` row reaches it.
- [ ] **A full row with the held log's changes.** No `full` row since
  `a728c4c`, and none carries the held arm or FASTER. Done when one is
  banked and FASTER's group is read against the quick rows.
- [ ] **What each engine's checksums verify.** RocksDB reads with
  `verify_checksums(false)`, in memory and from its files out of core.
  Done when RocksDB reads with its default, or the methods say what each
  engine verifies.
- [ ] **A kill in a rep taken again.** `Ledger::forget_rep` deletes a cut
  rep's kills with its passes, so a row can lose a kill. Done when the row
  keeps them and the gate reads them.
- [ ] **Device bytes measured at the device.** "Device bytes written" is
  the process's `/proc/self/io` count, which equals bytes on disk for
  buffered arms, while `bench/DESIGN.md` calls it the device's counter.
  Done when it reads the device, or is renamed.
- [ ] **FASTER's rates between two bounded rows.** FASTER's point mixes
  read a third to a half higher at `33bc663` than at `001781d` with its
  configuration unchanged. Done when the move is laid to the host, and the
  held arm's to the engine changes between the rows.

## Housekeeping

- [ ] **The crash seam's threads with no hold.** A piece merge, the
  builder ahead, the sampler and a form file's warm have no hold to put
  them in flight at a crash, and the wake of the threads that park between
  turns survives its mutant. Done when each mutant fails.
- [ ] **Two states of the settle no test builds.** A frozen batch cut by a
  budget, and the cache's bind at the fill's check. Done when a test builds
  each and its mutant fails, or the path is shown unreachable and removed.
