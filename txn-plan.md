# Format v5, deletes and transactions — registered before the code

Written while f49's third run and the canonical external run hold the
machine. Three asks arrived together: build transactions and deletes,
salvage as much load as possible, and fix the value-count design. They are
one piece of work, because deletes need a bit in the extent record, counts
belong in the same record, and transactions are the WAL contract that both
ride on.

## The count correction

Variable-width value counts were put in a companion file per segment. That
was wrong twice: the right home was always the extent record beside
`Ext::last`, which exists for the same O(1) reason; and "sidecar" names a
process, not a file. The companion file avoided a format change, and a
format change is what this priority spends.

**`Ext` gains a fifth `u32`: the record count of the run, with the top bit
reserved as the tombstone flag.** 20 bytes per extent, `repr(C)`, still
4-aligned, still borrowed straight out of the mapping. The format magic
moves from `...0004` to `...0005`, so a file written before is refused by
name rather than misread. Every writer sets it: the store's seal, its
consolidation, its redo log and varint index, `flatindex::encode`, and the
segment writer. `Blob::count` and `Reader::count` become a sum over
extents; `count_fixed` keeps its contract and stops being special. The
companion file, its readers and its name are removed.

## Deletes

`Db::delete(key)` ends every value of `key` written before it; later
appends start fresh.

- **WAL:** frames gain a kind byte after the sequence: put, delete, commit.
- **Memtable:** a tombstone is a chunk in the key's chain with a marker
  length; the entry's live count resets to zero at it. A read walks the
  chain newest-first and stops at the first tombstone.
- **Segments:** a seal writes the values after the newest tombstone and
  sets the tombstone flag on the extent if one was seen, meaning "this
  extent supersedes everything older for this key".
- **Reads:** `read_all`, `count` and `scan` gather sources newest-first,
  stop at the first flagged extent or memtable tombstone, and emit what
  they gathered in append order. A key with no tombstone costs one flag
  test per source it touches.
- **Merges:** every merge here writes the bottom level, so a tombstone
  never survives one: values older than the newest flagged extent are
  dropped, a key with nothing live is omitted, and its bytes are gone.

## Transactions

The external suite's `transactions` axis means an atomic multi-record
commit with rollback and consistent reads, which is the axis LMDB holds
over every Supdb arm today and the reason the matched comparisons carry a
residual. Three pieces:

- **Atomic batches.** Today a batch is the WAL frames written by one
  `write_all` and replay applies every intact frame, so a crash that
  persists a prefix of a batch replays a partial batch. Replay now applies
  a batch only when its commit frame follows it intact; frames after the
  last commit frame are discarded whole. One 17-byte frame per commit.
- **`Txn`.** `begin` stages puts and deletes in a side buffer; `commit`
  appends them to the WAL and memtable behind one commit frame and one
  barrier; `abort` (or drop) discards the buffer. Reads through the
  transaction see its own staged writes after the store's, so
  read-your-writes holds inside it. Staging costs one copy of each value,
  and the plain `append`/`commit` path stays for callers who do not need
  rollback -- it is atomic too, by the commit frame.
- **Consistent reads.** The engine is single-writer and a read borrows
  `&Db`, so no write can interleave with a read in this thread; a
  `Snapshot` beyond that waits for the multi-reader work.

`Features::transactions` flips to true for `next` in the external suite,
and every matched comparison against LMDB loses that residual.

## Predictions

- **P50.1 — the commit frame costs nothing measurable.** f50 runs the f39
  raw WAL shape with and without a commit frame per 1,000-record batch,
  interleaved: `no_difference` at the 5% floor. Refuted means the extra
  frame moved the fdatasync's cost, which it should not.
- **P50.2 — `Blob::count` on variable-width values comes within 1.3x of
  resolving the key.** f28-count rerun at `full`: W2.1 flips to `holds`
  (counting IS cheaper than reading once the count is stored), and W2.2's
  27x becomes a statistical tie, so it flips to `fails` with its prose
  saying why.
- **P50.3 — the index grows by at most 8% per key** (57 B/key to ≤ 61.6
  on f2-open's shape; one extent per key, four more bytes each) **and the
  read lead survives it: EXT.23 ≥ 1.4x LMDB** in the next canonical run
  (the last three read 1.42-1.64x; the bar allows the larger index its
  share of misses). Refuted below 1.4x means the twenty-byte record
  crossed a cache-line boundary that the sixteen-byte one did not.
- **P50.4 — deletes reclaim space through the merge.** f50: the f42 load
  with 10% of keys deleted before the drain leaves at most 0.92x the disk
  of the same load without deletes, and reads of a deleted key answer in
  under 1.2x the time of a missing key. Refuted means tombstones survived
  the merge or the read path walks past them.
- **P50.5 — every contract survives the model oracle** extended with
  deletes, aborted transactions and crashes at every step: uncommitted
  transactions vanish whole, committed ones survive whole, a deleted key
  stays deleted through seals, merges and reopens, and a key re-appended
  after its delete carries only the new values. Not a number; a test that
  must pass under all three writer/merge configurations.

## Load, after this

f49 replicated the writer at 1.42-1.48x on ingest-to-routed. What its
phase split says is left: the commit phase itself rose 0.67s to 0.80s when
the seal got faster, consistent with a 64 MB segment write contending with
the commit path's fdatasyncs for the device. The next levers, in order of
cost: I/O priority for the seal and merge threads (`ioprio_set`, idle
class) so the barrier wins the device; `SyncPolicy::EveryN` for callers
who accept bounded loss (F48.1, 1.63x); then the memtable append path,
which is the floor once the barrier is amortised (F48.2).

## Amendment, registered before f50 runs

Built and under test: the commit frame, deletes, `Txn`, and the adapter's
transactions axis. One cost the design above did not price: once any
source in a store holds a tombstone, every read pays a newest-first pass
over the sources that hold its key (a second probe on hits) to find where
live values start, and a store nothing was deleted from skips it. So f50
carries two load arms, interleaved, on the f42 shape with the drain inside
the window: `no-deletes`, and `deletes-10pct` (a tenth of the keys deleted
before the drain). Reads run after the drain over three key sets: keys
present in both arms, keys deleted in the second arm, and keys never
written.

- **P50.5 — present-key reads in a store with tombstones are within 1.15x
  of reads in a store without.** The pass costs a flag test per source and
  a second probe only on sources that hold the key; after a drain the
  store is partitions only, and partitions never carry tombstones, so the
  extra pass should be near free there. Refuted means the tombstone check
  reaches into the read path even when nothing it guards is present, and
  it needs a cheaper gate.
- **P50.6 — a delete costs the merge nothing measurable:** the
  `deletes-10pct` arm's merge phase is within 1.1x of the `no-deletes`
  arm's. It reads the same inputs and writes a tenth less.
