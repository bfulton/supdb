# Fewer dependent round trips on a cold open -- registered before the code

logshed measured a first page of search results over a cold cache at seven
dependent round trips on a real day (NASA-HTTP, 13 July 1995: 134k
requests, a 15.4 MiB store), five of them the store's before a posting
byte moves: superblock; key header and block table; fence; directory
slice; records; then the postings. Their R7 asks for three things, each
removing a wave. What the layout gives today, read out of the code rather
than remembered:

- The superblock page is 4 KiB with two 144-byte slots at 0 and 512; the
  probe reads 656 bytes and 3.4 KiB of the page is spare.
- A block read fetches the whole block (`with_extent` takes
  `BlockLoc::stored` bytes) and verifies it in 4 KiB chunks, so the plan
  for a two-posting run is the block it shares with its neighbours: the
  920 KiB logshed measured for "Shuttle".
- `SegmentWriter` already stores a run up to 256 bytes inside its index
  record (`inline_max`, since the inline extension of v5), so a rare word
  written by it costs no postings wave. `Store` never inlines, and
  logshed's roll -- and `logshed build` here -- write through `Store`.

## The changes

**R7.1 -- the open.** A write-once segment writes an extension into the
spare part of the superblock page: a copy of the key header and the
offset and length of the fence, the directory, the hash region and the
checksum row. The sparse open's first plan then names everything the
open needs -- fence, block table, row -- and its second plan is empty:
two waves, from three. With `SegmentWriter::set_head_reserve(bytes)` the
writer also leaves a reserve after the superblock page and, at finish,
places the block table and a copy of the fence there when they fit,
pointed to from the extension; a host whose first probe is that generous
(`openSparse(wasm, cache, {probe})`) then has everything after one wave.
The reserve is off by default and costs its own size in the file when on;
`Store` writes no extension and opens as it does now.

**R7.2 -- the directory.** `SparseBlob::open_with` gains
`BlobOptions::resident_directory`: the open wave fetches the directory
whole (page-rounded), `dir_slice` answers from memory, phase one of every
dictionary plan is empty, and a point lookup plans its records with no
dependent read. A search is then open, records, postings: three waves
cold, and with the directory and fence warm, two.

**R7.3 -- the postings.** For a plain block carrying per-chunk checksums,
the plan for an extent is the 4 KiB chunks it spans, not the block, and
the read fetches and verifies exactly those. A two-posting run reads one
chunk. Compressed and unchunked blocks keep reading whole. Inline runs
are already the answer to the rare word for a segment; `logshed build`
gains a segment-writer arm so the case is measured both ways, and the
recommendation to the roll is recorded rather than implemented on the
store's in-place path.

**w6-waves** measures all of it on the day fixture through a source that
models the host: an `ensure` of bytes not yet resident is one wave, and
the bytes it adds are counted, so a claim here is a count and a byte
total, not a timing.

## Predictions

- **P7.1 -- cold open: 2 waves with the extension, 1 with a reserve that
  fits and a generous probe**, from 3 today; open bytes unchanged in the
  first case, and in the second the probe plus nothing.
- **P7.2 -- lookup after open: 1 wave with the directory resident**, from
  2; the open grows by the directory, which on the fixture is under half
  a megabyte and on logshed's day 0.37 MiB. Search cold: 3 waves with
  R7.3, from 7.
- **P7.3 -- a rare key's postings wave reads at most 8 KiB** (a run
  inside two chunks) where it read the block, and the plan stays exact
  (W4.1 holds on the chunk plan as it did on the block plan).
- **P7.4 -- a segment-written day answers a two-posting key at the
  dictionary**: zero postings waves and zero postings bytes; the
  store-written day does not.
- **P7.5 -- nothing native moves**: the lending reader slices the same
  bytes; `tests/blob.rs`, `tests/dict.rs` and the browser suite hold.
- **P7.6 -- the reserve costs under 2% of a fixture-sized segment** when
  on, and the fence copy is the only duplicated structure.

## What would refute it

An open that still needs a dependent read says something the open needs
was left out of the extension -- the block table's row of chunk CRCs is
the likely one -- and the extension gains it. A lookup that still costs
two waves with the directory resident says the fence or the hash is
consulted through the source, which the plan must then name.

## Outcome (w6-waves, full; `results/w6-waves.full.json`)

All six predictions held, one after a correction to the design, and the
harness taught one lesson about page rounding.

- **P7.1 held**: two waves for a segment from a page probe (W6.1), one
  with a 128 KiB reserve and a probe that covers it (W6.2). The design
  changed once on the way: the checksum row sits at the end of the
  section, so a reserve holding only the table and the fence still
  needed a second wave for it; the row is copied into the reserve too.
- **P7.2 held, after the same correction for the directory**: with it
  resident a lookup is at most one wave (W6.3), and the cold search is
  two waves on the best shape and three by construction (W6.4) -- but
  only once the reserve also carries a copy of the directory. Without
  that the directory's own wave stays, since it lives in the section 11
  MiB from the probe; with it, a directory-resident open is one wave when
  the reserve is sized for it, which on logshed's day means about half a
  megabyte.
- **P7.3 held**: 32 KiB page-rounded for the rare key's chunks (W6.5).
- **P7.4 held**: zero postings waves on the segment (W6.6).
- **P7.5 held**: `tests/blob.rs`, `tests/dict.rs`, `tests/ranges.rs`,
  the Node and browser suites, all green; one test moved -- the damaged-
  block test picked its "undamaged neighbour" by whole-block plan, and a
  neighbour sharing the damaged chunk now rightly fails too.
- **P7.6 held**: 1.63% at full (W6.7); 12.7% at ci on a 1 MB file, which
  is why the reserve is the writer's choice.

Page rounding is what the first W6.3 tripped on: with 16 KiB pages a
lookup can cost zero waves because its records' page arrived with the
open, and the baseline lookup can cost one because its directory slice
shared a page with a fence. Counts are at most, not exactly, and the
finding says so.
