# RocksDB in the external suite — registered before the run

Every matched number the next engine has is against LMDB, and the one it
wins by the most -- 6.6x under shuffled arrival (EXT.27) -- is what any
log-structured engine takes off a B-tree that fsyncs a thousand dirtied
leaf pages per batch. The comparator that separates "the next engine is
fast" from "an LSM is fast" is RocksDB: a WAL, a memtable, sorted
immutable files and compaction, the shape the next engine has. Two arms,
`rocksdb` (WAL synced per batch) and `rocksdb-nosync`, compression off,
defaults otherwise; `Features` matches it to `next` on durability,
atomic batches and checksums.

## Predictions

- **P28 -- durable ordered load: a tie to 1.2x for the next engine.**
  Both pay one fsync per batch; RocksDB's memtable is a skiplist and its
  WAL frames carry a CRC per record, both dearer than the next engine's
  hash table and per-batch CRC; RocksDB's flush and L0 compaction are
  the next engine's seal and promotion. `EXT.28` recorded as holding.
- **P29 -- point reads: the next engine 2x to 3x.** RocksDB's read is a
  memtable probe, a block-cache lookup, a block-index binary search and a
  restart-interval decode; the next engine's is a fence, a hash slot and
  a record. `EXT.29` holds.
- **P30 -- ordered scan: the next engine 0.8x to 1.2x, a tie at the
  gate.** RocksDB's iterator over a merged level set against the next
  engine's rank cursors; neither has a structural edge. `EXT.30` holds
  as "no slower" only if the verdict is not Less.
- **P31 -- shuffled load: 0.9x to 1.3x**, since an LSM should not care
  about arrival order and the next engine's own swing is 1.17x; the 6.6x
  over LMDB does not carry over. `EXT.31` holds narrowly or ties.
- **P32 -- space: RocksDB smaller**, by the segment writer's inline runs
  and the 20-byte extents against RocksDB's prefix-compressed blocks;
  recorded, not claimed.

## What would refute it

RocksDB ahead on the durable ordered load says the next engine's seal
and merge cost more than an LSM's flush and compaction at this size, and
the seal wait is the place to look. RocksDB within 1.5x on reads says
the read lead against LMDB was mostly the B-tree descent, not the
flatindex probe.

## Outcome (full, `results/ext-kv.full.json`, `results/ext-loadshape.full.json`)

- P28 refuted: durable ordered load **0.778x** (503,806 against 647,423,
  p=0.0033). RocksDB writes fewer device bytes (209.9 MB against 299.6)
  and a smaller file (109.8 against 167.8); P32 held.
- P29 refuted upward: reads **7.62x** (1,696,165 against 222,518).
- P30 refuted upward: scan **5.95x** (23.9M against 4.0M entries/s).
- P31 held: shuffled load **1.18x** (265,727 against 224,767, p=0.0049);
  RocksDB's own swing 2.14x against the next engine's 1.37x.

The write side of the niche goes to RocksDB at its defaults; the read
side stays with the next engine by a margin the LMDB pair never showed.
Both halves carry one caveat: RocksDB's defaults (an 8 MB block cache,
no Bloom filter, 64 MB write buffer) are not how it is deployed, and a
tuned arm is the next thing to run before either number is quoted as
"against RocksDB" rather than "against RocksDB as shipped".
