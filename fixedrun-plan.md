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
