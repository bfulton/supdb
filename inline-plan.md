# f53: inline runs — registered before the code

The read lead sits at 1.39-1.64x over seven canonical runs (EXT.23) and the
bar is 1.5x reliably. The brief said what is left past the arrangement
ceiling: fewer cache misses per lookup. At a million keys a point read
misses on the hash slot, on the record, on the block table row and on the
block; the last two exist only to reach values that, on the shape this
engine is judged by (one 100-byte value per key), fit in the record.

## The change

A run of values whose bytes fit under a threshold is stored inside the
index record, after its extents, and its extent names `Ext::INLINE`
instead of a block, with `off` the run's offset in the record's tail. A
lookup returns the extents and the tail; a read of an inline run slices
the tail and never consults the block table or a block. Only the segment
writer produces inline runs (a seal, a merge); `Store` never does. A v5
reader given such a file errors -- "extent names a block the table does
not have" -- rather than answering wrongly, and the new reader reads v5
files unchanged, so the format stays v5 and nothing already written is
refused.

One more thing the writer stops doing: the flat index reserves half again
its record bytes so a later checkpoint can add extents in place. An
immutable segment never will, so the slack is tied to `insert_slack` being
non-zero, which the writer never asks for. That is 20 B a key of file today
and it goes with this change on both arms.

## Predictions

- **P53.1 — point reads over a drained store are at least 1.25x faster
  with inline runs than with block-backed runs,** interleaved, the
  EXT.23 shape (1M keys, 100-byte values). Two misses fewer out of four
  or five, each near a DRAM latency.
- **P53.2 — the store on disk is within 1.05x either way.** Values move
  from blocks into records; nothing is duplicated.
- **P53.3 — the ordered scan is no slower with inline runs** (not `Less`),
  because the scan walks records in key order and an inline run is where
  the walk already is.
- **P53.4 — the dictionary count (`scan_counts`) over inline records costs
  at most 2x the block-backed form's per key.** The records are wider and
  the walk touches more bytes per key; this is the price and it is
  registered rather than discovered.
- **P53.5 — ingest-to-routed is within 5% either way.** The writer moves
  the same bytes to a different section.
- **P53.6 — the next canonical run reads at least 1.5x LMDB (EXT.23).**
  The bar that started this.

## Rule

Both arms behind `NextOptions::inline_bytes` (0 disables), one process,
f53-inline. `tests/segwriter.rs` holds a Store-written store and an
inline-written one to the same answers on every read, which is the test
that a second layout cannot answer differently.
