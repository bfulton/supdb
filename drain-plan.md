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
