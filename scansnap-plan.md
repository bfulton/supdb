# The unrouted scan's snapshot -- registered before the code

f62 left the undrained shape (three level-0 segments and a memtable) at
2.06M entries/s against 33.2M routed, 16.1x apart (F62.4), and the new
merge only took it from 19.1x to that. A probe on the undrained store,
bucketing 6,400 scans by where their start key falls, decomposes it:

- a scan that starts inside a sealed segment costs **55-65 ns/entry** under
  the merge, against about 30 routed -- the k-way merge over unrouted
  sources is a 2x, not a 16x;
- a scan that starts in the memtable's key range costs **240 ns/entry**,
  with a 428k-key memtable: hash-order entries, a chain walk and a value
  fetch per key against a segment's contiguous records;
- the **first scan of the process took 130 ms** for a 428k-key memtable,
  and averaged into whichever bucket it landed in as 1.4-1.6 us/entry. That
  is `Db::scan` building the sorted snapshot of the unsealed keys -- one
  `Vec<u8>` per key, sorted by dereferencing two heap pointers per compare
  -- about 300 ns a key. f62 times 400 scans of 1,000 entries with no
  warm-up scan, so that build is inside every undrained measurement, and
  every ext-kv scan phase pays it once per repetition.

## The change

The snapshot keeps the keys in one arena and sorts 24-byte records -- the
first sixteen bytes of the key as two big-endian words, and the index of
the arena slice -- so a compare touches the arena only on a shared 16-byte
prefix. Same `SnapKey` contract to the merge: a key and its live and frozen
entry indices, dedup across the two tables. Behind
`NextOptions::scan_snapshot_arena` (default on) so the two builds can be
interleaved in one process, as f8 does for checksums.

## Predictions

- **P63.1 -- the build is at least 3x faster** at both 143k and 428k
  unsealed keys (the two shapes f62 and the probe used), measured as the
  first `scan` after a commit minus the second.
- **P63.2 -- f62's undrained shape end-to-end moves at least 1.2x** with the
  new build alone, and with the build reported beside the steady-state rate
  the remaining gap to routed is under 5x for scans that start in a
  segment. If it does not move, the time is elsewhere -- the third seal
  still landing during the scan phase, since f62 never calls `settle` --
  and the harness, not the engine, is the next look.
- **P63.3 -- a memtable-range entry costs 3-5x a segment-range entry** with
  a warm snapshot, and the build is not what separates them; recorded as
  the price of scanning a hash table in key order, to be moved by a
  different change (the frozen table could be sealed to a piece sooner).
- **P63.4 -- the merge itself is within 2.5x of routed** for scans that
  start inside a segment: 55-65 ns against about 30, which is `key_at` per
  cursor per entry and one `values_at`, and that is the whole cost of
  scanning unrouted sources once the snapshot is paid.

## What would refute it

A build under 3x faster says the sort was not the cost and the hash-table
walk was; then the snapshot should be maintained across commits instead of
rebuilt. An end-to-end that does not move with a 3x faster build says the
measured 194 ms was never mostly the build, and EXT.39's 8.6x is the
in-flight seal or the frozen table -- either way something f62's harness
should hold still before it times.

## Outcome (f63-scansnap, full, `results/f63-scansnap.full.json`)

All four held, and the refutation clause fired once on the way.

- **P63.1 held, after its first build was refuted.** The arena and the
  24-byte sort took the 428k-key build from about 140 ms to 89 ms on the
  probe -- 1.6x, under the predicted 3x, which the plan said would mean the
  hash-table walk and not the sort was the cost. It was: a hash table
  walked in slot order visits its key bytes in random order, one cache
  miss a key, and an intermediate version that ordered the slots by key
  offset first reintroduced the same miss on the slot side. The build now
  records (key offset, length, slot) without touching a key, radix-sorts
  the triples in two 16-bit passes, and copies the arena sequentially.
  Measured interleaved against the old build: **10.0 ms against 58.3** at
  142,000 unsealed keys (5.81x) and **32.1 against 314.5** at 428,571
  (9.81x), both p=0.0009.
- **P63.2 held at 2.28x**: 10.5M entries/s against 4.6M for f62's
  measurement -- the build plus 400 uniform scans of 1,000 entries -- with
  the build 58 ms of the old arm's 87. The remaining 29 ms is the scans.
- **P63.3 held, better than predicted**: 124 ns an entry in the memtable's
  range against 53 inside a segment, 2.33x rather than 3-5x; the probe's
  240 was measured against a 428k-key table with a frozen twin beside it.
- **P63.4 held at 1.69x**: 53.2 ns an entry under the merge against 31.4
  routed, for scans that start inside a segment.

Two things the run corrected about f62. Its undrained arm was never the
shape it named: `sync` seals nothing and joins nothing, so at the moment
the scans started the third seal was still in flight -- a 286,000-key
frozen table beside the 142,000-key live one, and a 49 MB segment being
written on another core. `settle` is what f63's arms call, and
`Db::unsealed_keys()` now exists to check. And f62 had no warm-up scan, so
its 194 ms per undrained arm was mostly one snapshot build over 428,000
keys, amortized over 400 scans -- which is why F62.2's rel_iqr was 20%
while every other arm's was 5%. The 16.1x of F62.4 decomposes as: the
build, the frozen table, and then a merge that is 1.7x routed and a
memtable range that is 2.3x. The k-way merge was never the lever; the
snapshot was, and it is paid once per commit rather than per scan.

Not measured here: EXT.39, whose adapter (`next-nodrain`) also scans
behind `sync`, and so pays the build once per repetition over whatever
the memtable holds; the next canonical drain run will say what remains.
