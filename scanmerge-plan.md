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
