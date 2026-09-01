# f49: the bulk segment writer, priced in one process — registered before the run

Written after the writer exists and its agreement test passes, before any
timing of it has been taken. The user's standing priority reopened what f46
declined: "this project is about sacrificing space and complexity for time."

## What it is

`next::SegmentWriter` writes a sealed segment in one forward pass for input
that arrives sorted with each key's values together: values packed into
blocks in arrival order, one extent per key, then the block table, the key
section and the superblock. Same format, same `Blob`, `store::Reader`
opens it too (`tests/segwriter.rs`). It replaces `Store::create` + `append`
+ `checkpoint` + `close` in both places the next engine writes a segment:
the seal and the partitioning merge.

f46 measured the FLOOR of this idea at 2.04x the general path with the block
table, checksums and superblock omitted (F46.1), and the index build at 19%
of it (F46.2). This is the built writer against the general one, with
everything included, on the load the engine is judged by.

## The rule

Never compare two separate runs. So the general writer stays behind
`NextOptions::bulk_writer` (default on), and f49 runs both arms interleaved
in one process on f42's shape: 1M keys, 1,000-record batches, 100-byte
values, durable per batch, partitioning on. The timed window is the load
**plus the drain** (`flush`: seal, join, partition), which is the shape the
external suite times -- on the loop alone the seal overlaps the commits and
F42.3 put its visible cost near 7%, so a loop-only window would refute
P49.1 by construction rather than by measurement. Space is the exception
and file size may be compared across runs, but it is taken here beside the
rest.

## Predictions

- **P49.1 — durable load throughput with the bulk writer is at least 1.25x
  the general writer's.** f42's phase split put roughly half the window
  outside the commit path (seal and merge); halving that half is ~1.3x.
  Refuted low means the seal was not where the time was, or the writer
  did not halve it.
- **P49.2 — the seal phase itself is at least 1.8x faster.** f46's floor
  said 2.04x with the table, checksums and superblock left out; the real
  writer pays them, and the memtable sort and the chain walk are the same
  in both arms.
- **P49.3 — the segments on disk are no larger, and at most 0.9x.** A
  bulk segment has no freelist rounding, no reuse log, no redo-log arena
  and no index slack. Space is what the priority says to spend; this
  checks it was not spent here.
- **P49.4 — reads over the loaded store do not differ.** Same format,
  same `Blob`, same routing; `stats::compare` at a 5% minimum effect
  should return `no_difference`. A difference either way means the two
  writers pack blocks differently enough to matter, which would be worth
  knowing and is not the claim.

## After

If P49.1 holds, re-run the canonical `ext-kv` at `full` (next and
next-ingest against LMDB), record EXT.22/EXT.25 from `results/`, and only
then rewrite the brief's P-A paragraph that currently says the writer was
priced and declined.
