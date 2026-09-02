# f61: the ordered scan over unrouted sources — registered before the run

EXT.39 found an undrained store -- three level-0 segments and a memtable
in front of the partitions -- scanning at 2.9M entries a second where the
same data routed scans at 24.7M. The routed walk is `Blob::scan` over
partitions in key order, one index lookup an entry. Anything else takes
the k-way merge over rank cursors, and the merge has two costs the code
already names: it resolves `key_at` for every cursor twice per emitted
key (once to find the minimum, once to advance), and it opens a cursor on
*every* segment whose upper fence lies past the scan's start -- every
partition at once for a scan from the beginning, though partitions are
disjoint and only one can hold the next key. And one thing it does not
name: the fast path is all or nothing. A single unsealed key anywhere
past `from` sends the whole scan to the merge.

f61 measures the scan after the canonical ordered load in four shapes,
same data, interleaved: **routed** (a flush), **routed plus a thousand
keys in the memtable**, **four level-0 segments and no memtable** (seal
and join, no partitioning), and **undrained** (three segments and the
memtable, EXT.39's shape).

## Predictions

- **P61.1 -- a thousand memtable keys cost the routed scan at least 3x.**
  Not the keys: the fast path is lost for every entry, and the merge runs
  with one partition cursor per partition plus the unsealed snapshot.
- **P61.2 -- four level-0 segments without a memtable scan within 1.5x
  of the undrained shape.** The level-0 count is the cost, not the
  memtable.
- **P61.3 -- the undrained shape is at least 5x slower than routed**,
  replicating EXT.39's 8.6x inside one process.

## What follows

If P61.1 holds, the first fix is that the partition side of the merge
must keep its fast path -- one cursor that advances through the disjoint
partitions in order -- and the unsealed snapshot and level-0 cursors are
merged against it. With per-cursor key caching that puts the merge at one
lookup per cursor per emitted key. The second run of f61 prices that,
both arms behind `NextOptions::scan_merge` in one process.

## Outcome (full, `results/f61-scanmerge.full.json`)

- P61.1 held: a thousand unsealed keys cost the routed scan **3.41x**
  (32.4M to 9.5M entries/s). The fast path is all or nothing.
- P61.2 refuted, the other way: four level-0 segments without a memtable
  scan at 9.5M, the undrained shape at **1.7M**. The unsealed source is
  the cost, not the segment count -- hundreds of thousands of snapshot
  keys, each paying two hash probes and an allocating chain walk.
- P61.3 held: **19.1x** routed over undrained in one process.

What follows, in order of what the run priced: the unsealed source must
stop allocating and probing per key (the snapshot carries each key's
entry so the emit is a chain walk over a reused scratch buffer); the
merge must resolve each cursor's key once per emitted key, not twice;
the partitions must be one cursor advancing in order, not one per
partition; and the fast path should survive an unsealed key, which the
first three may make unnecessary. Both merges behind
`NextOptions::scan_merge`, f62 prices them on f61's four shapes.

## f62 — registered before the run

Both merges in one process on f61's four shapes. Predictions: the new
merge is **at least 2x** the old on the routed store with a thousand
unsealed keys (P62.1) and **at least 3x** on the undrained store (P62.2),
the unsealed source's two probes and an allocation per key being most of
what f61 measured; the routed scan **does not move** (P62.3), since the
fast path is untouched; and the undrained store comes **within 4x** of
the routed one (P62.4), from 19x, the rest being the level-0 cursors
themselves and the snapshot's sort.
