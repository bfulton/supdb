# A checksummed key index -- registered before the code

Every block is checksummed and verified once per reader (f8, `Options::
checksums`); the key index section is not. Format v6 made that a
correctness hole rather than a theoretical one: `tests/known_bugs.rs`
flips every byte of a store and expects each flip to be caught, and a flip
of the `Ext::FIXED` bit in an index record is not damage the block
checksum can see -- the run re-decodes quietly under the other encoding
and the read returns different values without an error. The test was
bounded to the data region and the hole filed (fixedrun-plan.md). A
flipped record offset, block id or count had the same property all along.

## The change

The key section gets a row of CRC32C words, one per 16 KiB piece of its
content, written after the hash region and named by two header words
(the row's offset and the piece shift; the 192-byte header has the room).
Piece 0 covers the header itself, so a damaged header word is caught by
the same mechanism rather than by the region checks alone. The superblock
magic moves to v7: a reader from before the row refuses the file rather
than parsing the row as slack, and a section without a row -- one the
store may edit in place -- says so in a header flag and is read unverified,
as today.

Verification is once per reader per piece, on first touch, through the
same bitmap `Blob` keeps for block chunks: `key_at`, `lookup`, `seek` and
`exts_at` name the offsets they are about to read, and the piece those
offsets fall in is checked before the bytes are interpreted. Open verifies
piece 0 and nothing else, so open stays what it is (F2.2). `SparseBlob`
already fetches by range and caches in 16 KiB pages; its plans round out
to piece boundaries and a fetched piece is verified when it is first used,
so the browser reader gets the same guarantee over a partial fetch.

The writers: `SegmentWriter` (the next engine's immutable segments) and the
store's full rewrite compute the row in one pass over the finished section
on the sealing thread. The store's in-place checkpoint publishes a record
with one aligned 8-byte store into a section readers are mapping, and a
piece CRC cannot be kept consistent with that lock-free -- a reader would
verify a piece between the slot write and the row update and report
damage that is not there. So a section the store may edit in place is
written with the flag and no row; that path stays unprotected and the
claim says so.

## Predictions

- **P64.1 -- every single-byte flip in the key section is caught.** The
  reproducer in `tests/known_bugs.rs` goes back to the whole file for a
  segment written by `SegmentWriter`: each flip either errors on the read
  that touches it, or -- for a byte no read touches -- changes no answer.
  The FIXED-bit flip, the offset flip and the count flip are named cases.
- **P64.2 -- the point read does not move at the gate** (`EXT.23`'s shape,
  measured in-process against the unverified arm behind
  `BlobOptions::verify_index`): a piece is verified once and a read after
  that costs one bit test, against a hash probe and a record that already
  cost two misses. Under 2%, not resolvable.
- **P64.3 -- the ordered scan does not move at the gate** either: pieces
  are verified in order, once, 16 KiB of CRC32C per 16 KiB of records.
- **P64.4 -- the seal costs under 2% more**: one CRC pass over a 16 MB
  section on the sealing thread, off the commit path (f60's ledger says
  where the seal's time goes and this is a rounding error against it).
- **P64.5 -- the section grows by 0.03%**: four bytes per 16 KiB.
- **P64.6 -- the sparse reader's bytes per range move by page geometry
  and its answers do not**: `tests/dict.rs` holds every range to the whole
  reader's answer, and W5's byte counts are re-recorded.

## What would refute it

A read cost that moves at the gate says the bitmap test landed on the
hot path in a way the block one did not -- then verify at open for
sections under a size and keep the bitmap for larger ones. A flip that
survives says a read path reaches the section without naming its
offsets, which is a code path to find, not a parameter to tune.
