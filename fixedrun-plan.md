# Fixed-width runs — registered before the code

`ext-analytics` reads one term's full posting list at 3.44 ns a posting
against LMDB's DUPFIXED at 1.06 (EXT.18, 0.307x), and intersects two lists
at 0.77x (EXT.17). Both are the value encoding: every value carries a
varint length prefix, a 5-byte stride for 4-byte postings, and each prefix
must be decoded before the next value can be found. The counts and the
dictionary walk, which never touch a value, lead by 3.8x and 7.4x.

## The change

A run whose values all share one width is written back to back with no
prefixes, and its extent carries a flag beside the tombstone bit
(`Ext::FIXED`, bit 30 of the count word). Nothing else is stored: the
extent already has its byte length and its record count, so the width is
`len / records`. `Ext::last` is `(n-1) * width`, as it is for any run.
Mixed-width runs keep the prefixed encoding. The superblock magic moves,
so a reader from before the flag refuses the file rather than parsing a
fixed run as prefixed.

Writers decide at the point they hold the whole run: the segment writer
between `begin` and `end`, the store when it seals a key's pending bytes
into a block and when it consolidates a key's extents into one. Readers
branch on the flag: `read_all`, `values_at`, the ordered scan, the
store's own read paths and its `Reader`. `count_fixed(width)` becomes
exact for a fixed run (the flag says what the caller had to assume);
`count` is unchanged. A new `Blob::intersect_fixed(a, b, width)` merges
two keys' runs in place with a two-pointer walk over undecoded slices,
extent by extent, which is the kernel EXT.17 says is missing.

## Predictions

- **P18 -- reading a full posting list moves from 0.31x to at least
  parity, and past 2x on lists longer than a few hundred postings.** The
  per-posting cost becomes memory bandwidth; LMDB's stays a cursor step
  per page. Short lists are decided by the per-key constant, where a hash
  probe and an extent beat a cursor set by the 7.4x of EXT.16.
- **P17 -- the intersection moves from 0.77x to at least parity** with
  the in-place kernel, since both engines then compare 4-byte words
  across contiguous pages and neither copies.
- **P15, P16 -- ranking and point counts do not move** at the gate;
  neither touches a value.
- **Space -- the day index shrinks by about a fifth**, the prefix's share
  of a 5-byte stride; recorded, not claimed.
- **Every existing agreement holds**: `tests/blob.rs` (Blob against
  Reader on every key), `tests/segwriter.rs`, `tests/dict.rs`, and the
  next engine's oracle, since the encoding is a property of a run and the
  merge re-encodes.

## What would refute it

Parity or worse on the full read says the block read and the checksum
verification, not the decode, were the cost; that would send the next
look at `with_extent`. A loss on the intersection with the kernel in
place says LMDB's page-at-a-time merge has an edge the two-pointer walk
does not, and the walk should batch.

## Outcome (recorded after the run)

Five `ext-analytics` runs at `full` on the v6 code, the last one installed
in `results/ext-analytics.full.json`.

- **P18 held in its first half and not its second.** Reading a full
  posting list went from 0.307x of LMDB's DUPFIXED to 1.201x, 1.192x,
  1.248x (no difference), 1.150x and 1.200x (no difference, p=0.0553):
  parity or better in all five runs, a significant lead in three. EXT.18
  is claimed as parity and flips to `holds`. It did not pass 2x on long
  lists and the reason is in the design, not the run: GET_MULTIPLE is
  also a memcpy-shaped walk over a page of packed 4-byte values, so once
  the prefix is gone the two engines run the same inner loop, and the
  uniform probes over a 255-median dictionary are decided by the per-key
  constant. The 1.0 ns/posting either way is memory bandwidth.
- **P17 held, after its mechanism was refuted once.** The first kernel --
  a byte-offset cursor per key and a bounds-checked slice compare per
  step -- measured 9,534 ns/pair at `full`, 0.842x of LMDB and slower than
  the naive decode-both merge (7,321) in the same process, having been
  1.54x faster than it at `ci`. That is the "walk should batch" branch of
  the refutation paragraph, and the batching that mattered was the
  compiler's: replacing the cursors with `chunks_exact` iterators over
  each key's runs removed the per-step check and gave an exact tie
  (8,083 against 8,084 ns); comparing each 4- or 8-byte value as a
  big-endian integer instead of a slice took it past, to 6,993, 6,700
  and 7,089 ns against 8,325, 7,905 and 8,175 -- 1.191x, 1.180x, 1.153x,
  all significant. EXT.17 flips to `holds`. The naive merge is kept in
  the checksums-on arm so every run prices the kernel against the
  application-side form: 1.06-1.11x.
- **P15 and P16 held**: 3.00x and 6.25x, with the run-to-run spread of the
  LMDB arm (2.92-3.04x, 5.87-6.36x across the five) and no movement
  attributable to the encoding.
- **Space held exactly**: 5.02 MB to 4.05, -19.3%, against the predicted
  fifth. LMDB stays at 7.33.
- **Every agreement held**: `tests/blob.rs`, `tests/segwriter.rs`,
  `tests/dict.rs`, the next-engine oracle and the browser fixtures. One
  test had to move: `tests/known_bugs.rs` flipped every byte of a store
  and expected each flip to be caught, and a flip of the FIXED bit in an
  index record is not damage the block checksum can see -- the run
  re-decodes quietly under the other encoding. The test is bounded to the
  data region and the hole is filed: the key index section needs its own
  checksum.

Not measured here: the day-index roll (`w1`, `f28`) and the canonical
`ext-kv` load, whose 100-byte values are uniform and so now write fixed
runs too. Both should be re-run before their numbers are next quoted.
