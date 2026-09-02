# f59: one CRC per batch — registered before the code

f58 put the next engine's commit path at 677 instructions an appended
record, and the WAL frame at 227 of them, of which the per-frame CRC is
92. A batch is the frames between commit frames and replay applies it
whole or not at all: the first frame that fails its CRC ends the walk,
and everything from the batch's first frame on is dropped. A CRC per
frame therefore buys nothing a CRC over the batch does not -- either way
a damaged byte anywhere in the batch loses the batch -- and it costs a
CRC setup and finish per record instead of per thousand.

## The change

Put and delete frames carry `len | seq | kind | payload` with the CRC
word zero; the commit frame's CRC covers every byte of the batch from
its first frame through the commit frame's own header. Replay
accumulates the CRC as it parses and checks it at the commit frame; a
mismatch, or a missing commit frame, drops the batch. The WAL magic
moves to `\x04`, so an older WAL is refused by name. The frame layout
does not otherwise change, so the accounting `c4-crash` tears against
is the same.

## Predictions

- **P59.1 -- the commit path loses at least 80 instructions a record**
  under cachegrind (677 to under 600), the CRC's setup and finish per
  frame becoming one per batch; the bytes hashed are the same.
- **P59.2 -- durable ordered ingest does not fall, and rises by less
  than 1.05x** at full, interleaved: 90 instructions is about 0.03
  microseconds of a 1.9 microsecond record, below the gate.
- **P59.3 -- c4-crash holds unchanged**: 120/120 open, no acknowledged
  batch lost under Always, every state a prefix, EveryN within seven.
- **P59.4 -- a byte flipped inside any frame of a batch loses exactly
  that batch and the ones after it**, as before; a unit test flips one
  byte at every offset of a two-batch WAL and checks the first batch
  survives and the second does not.

## What would refute it

An ingest change either way at the gate says the CRC was not where the
time went, which f58's instruction count already suggests; the
instruction saving is the claim, and the wall-clock is expected to be a
tie recorded as such.
