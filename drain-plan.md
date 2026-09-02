# The drain, matched both ways — registered before the run

f60 found the next engine's seal phase to be the final drain: the
adapter's `sync` seals the last memtable and partitions what it sealed
inside the load window, 0.263 s of 2.301, while RocksDB's `sync` is an
fsync of its WAL. Every load ratio against RocksDB (EXT.28, EXT.32) and
every read ratio (EXT.29, EXT.33) therefore compares a drained store
against an undrained one. Two arms fix that, one in each direction:
`next-nodrain`, whose `sync` fsyncs and seals nothing, so its reads go
through the memtable and the unrouted tail as RocksDB's do; and
`rocksdb-tuned-drain`, whose `sync` flushes the memtable and compacts
every level into one, so its load carries the drain and its reads run
against a compacted tree.

## Predictions

- **P36 -- both drained, load: 0.85x to 1.15x.** A full compaction of
  110 MB costs RocksDB more than the next engine's partitioning of its
  last seal, and closes most of the third it leads by.
- **P37 -- neither drained, load: 0.72x to 0.85x.** The next engine's
  window loses the 11% the drain was; the ratio moves from 0.688x by
  about that.
- **P38 -- neither drained, reads: 3x to 5x.** The next engine's read
  now probes the memtable and Bloom-checks up to three unrouted segments
  before the partitions; a point read costs more than the 6.45x arm's
  but stays far ahead of an LSM read.
- **P39 -- neither drained, scan: 2x to 4x.** The k-way merge carries the
  memtable's sorted keys as one more cursor.
- **P40 -- both drained, reads: 4x to 7x.** A compacted RocksDB reads
  faster than its post-load shape -- no level-0 files to check -- by less
  than 2x.
- **P41 -- neither drained, shuffled load: 1.1x to 1.4x.** EXT.35's tie
  with the drain removed from one side.

## What would refute it

P36 above 1.15x says the next engine's ingest with its layout work
included is genuinely ahead of an LSM that does the same work, which is
the niche claim; P36 under 0.85x says compaction is cheaper than
partitioning at this size. P38 under 3x says the memtable and the
unrouted tail cost more on the read path than the fence and the record
save, and the drain was buying the read lead rather than reflecting it.

## Outcome (full, `results/ext-kv.full.json`, `results/ext-loadshape.full.json`)

- P36 missed low: both drained, load **0.815x** (483,413 against
  593,130). A full compaction costs RocksDB 10%; the drain costs the next
  engine 19% under ordered keys.
- P37 refuted upward: neither drained, load **0.904x, a tie** (597,044
  against 660,311, p=0.055). The drain was most of the gap on this axis.
- P38 held: neither drained, reads **4.69x** (1,080,539 against 230,490);
  the unsealed tail costs the next engine's read 1.6x.
- P39 refuted by an order of magnitude: neither drained, scan **0.68x, a
  tie** (2.89M against 4.26M) where the drained store scans 24.7M. The
  k-way merge over three unrouted segments and the memtable is 8.6x
  slower than the routed walk. This is the lever the run found.
- P40 held: both drained, reads **7.15x** (1,723,911 against 241,215); a
  compacted RocksDB reads 5% faster than its post-load shape.
- P41 refuted upward: neither drained, shuffled **2.37x** (500,832
  against 211,650), and the next engine's own order swing vanishes
  (1.024x): under shuffled keys the drain was the flush's merges.

What the six say together: the durable load against an LSM is a tie
when neither does layout work in the window and 0.82x when both do; the
point-read lead is 4.7x to 7.1x whichever way it is matched; the ordered
scan's lead exists only over a routed store, and an unrouted one scans
8.6x slower than a routed one -- the unrouted scan path is the next
lever, and it is a read-path change, not a format one.
