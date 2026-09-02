# Compressed segment blocks -- registered before the code

logshed moved its index writing to `SegmentWriter` for the one-round-trip
open and inline runs (R7.1-R7.3) and paid 30% in size: 19.9 MiB against
15.4 on the NASA day, 64% of the raw archive against 49%. The cause is
plain in `flush_block`, which writes every block verbatim and sets
`stored == uncompressed`. `Store::write_block` has taken a `compress`
flag since the beginning and LZ4s posting deltas about 2x.

What the two writers actually do today:

- `Store`, compressing: `block::write_chunked_sz` produces a block whose
  own directory carries per-chunk starts **and per-chunk CRCs**, so
  `read_chunked_range` decodes and verifies only the chunk an extent
  lands in. `BlockLoc::chunked` is set; `chunk_crc` is not, because that
  flag names the *other* mechanism.
- `Store`, verbatim: `block::chunk_crcs` fills a row in the block table
  and `chunk_crc` is set. That is the row `blob::chunk_span` reads when
  it plans a range rather than a whole block (R7.3).
- `SegmentWriter`: verbatim, no row, `chunk_crc: false`. So a segment's
  blocks are read whole, which is what logshed's 16 KiB block size exists
  to bound.

## The changes

**`SegmentWriter::set_compress(bool)`**, beside `set_inline_max`, taking
the same path `Store` takes: chunked when the payload exceeds the chunk
size, verbatim when compression does not pay, `uncompressed` recording the
payload length either way. Inline runs live in the key section and are
untouched, so the two features compose rather than trade.

**Per-chunk checksums on a segment's verbatim blocks.** `finish` already
writes a row per block into the block table and fills it with zeros;
filling it with `block::chunk_crcs` costs nothing new in the format and
makes `chunk_span` plan by chunk for uncompressed segments.

**A compressed block read by range.** `chunk_span` returns the whole block
for anything not plain, because `with_extent` hands `read_chunked_range`
the whole stored buffer. A chunked block is self-describing at its head,
so a ranged reader can fetch the directory, then the byte span of the
chunks the extent covers. This is the piece that lets logshed raise its
block size back up, and it is the one with a real chance of being wrong,
so it is measured separately from the other two.

## Predictions

- **P4.1 -- the day index shrinks by at least 25%** with compression on,
  putting the segment at or below the store's 15.4 MiB on the same day.
  Recorded as a size, which needs no repetition to be believed.
- **P4.2 -- inline runs are unaffected**: the same count of keys answer
  at the dictionary with no postings wave, compressed or not (W6.6's
  measurement, re-run in both arms).
- **P4.3 -- a point read of a compressed segment costs no more than 1.3x
  an uncompressed one**, warm, interleaved in one process: one chunk
  decompressed against one memcpy.
- **P4.4 -- the ordered scan pays more**: between 1.0x and 2.0x, because
  a scan decompresses every chunk it crosses. Recorded rather than
  claimed as a win.
- **P4.5 -- with per-chunk checksums a verbatim segment's rare-key
  postings wave falls to at most two chunks**, as it did for the store in
  W6.5, and W4.1's exactness holds on the chunk plan.
- **P4.6 -- a compressed block read by range fetches its directory plus
  the chunks the extent spans**, and never the whole block, for a block
  above 32 KiB; below that the directory is most of the saving and the
  whole block may be cheaper.

## What would refute it

A size saving under 25% says the postings are already dense enough that
LZ4 has little to find, and the 30% logshed measured came from something
else in the store's layout. A point read past 1.3x says the chunk
directory lookup, not the decompression, is the cost, and the chunk size
wants raising. A ranged compressed read that fetches more than the whole
block says the directory is too large at that block size, which is a
measurement that sets the block size rather than a reason not to do it.

## Outcome (w6-waves, full; `results/w6-waves.full.json`)

**P4.1 refuted, and the feature kept.** `set_compress` saves 19.9% of the
day index (6,092,168 bytes against 7,608,372), not the 25% predicted;
8.8% at ci, where fixed costs are a larger share of a smaller file. Two
findings under the finding, neither predicted:

- **The encoding decides, not the flag.** The same day stored as
  absolute line ordinals compresses by **0.0%** -- byte for byte
  identical -- because LZ4 matches repeated byte sequences and a rising
  counter has none. logshed's 2x is a property of their deltas. Both
  arms of the recorded comparison store deltas so that compression is
  the only difference between them; the first version of this experiment
  compared ordinals and measured nothing, twice, before that was
  understood.
- **Inline runs and compression pull against each other.** 19.9% is far
  under the 2x LZ4 gets on the block bytes, because every run under 256
  bytes lives in the key section, which is not compressed, and on a Zipf
  dictionary that is most of the terms. So the 30% logshed attributed to
  moving off `Store` is not all compression, and the rest of it is worth
  finding before more is spent here.

**P4.2 held**: the open is one wave either way and the rare key is still
answered at the dictionary with no postings wave.

**P4.5 held for verbatim segments**: their blocks now carry per-chunk
checksums, so `chunk_span` plans by chunk. `tests/segwriter.rs` holds a
compressed segment to an uncompressed one on every key, asserts the key
section is the same size, and asserts the chunk plan is no larger than
the block plan.

Not measured yet, and the reason the block size is still what it is:

- **P4.3, P4.4** -- the point-read and scan cost of a compressed segment,
  interleaved in one process.
- **P4.6** -- a compressed block fetched by range. `chunk_span` still
  returns the whole block for anything not plain, because `with_extent`
  hands `read_chunked_range` the whole stored buffer. A chunked block is
  self-describing at its head, so the reader could fetch the directory
  and then the chunks the extent spans. That is the piece that would let
  logshed raise its 16 KiB block size, and it is untouched here.
