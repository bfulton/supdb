# f58: where the durable load's instructions go — registered before the run

f57's decomposition left the x86 durable load at roughly 45% commit phase
(WAL append and its fdatasync), 14% waiting on seals, and the rest on the
caller's thread building frames and memtable entries. The barrier is
one fdatasync per batch on either side of `EXT.22` (0.694x), so what
remains is compute, and compute is what cachegrind counts exactly. The
method is `docs/profiling.md`'s: `external loadprof --engines next`
under cachegrind with the pinned cache model, once with `--keys 0` and
once with the load, subtracted, divided by keys; `cg_annotate` for the
functions.

## Predictions

- **P58.1 -- under 1,500 instructions per appended record** on the
  append-and-commit path below the first seal (100,000 keys, 100-byte
  values, 1,000-record batches), against the 1,543 `Store::put` was
  brought to. The path is a hash probe, a frame encode and two copies.
- **P58.2 -- the memtable insert is the largest single cost**, over a
  third of the instructions: the open-addressed probe, the key compare and
  the arena append.
- **P58.3 -- the driver's own payload generation is under 15%** of the
  total, so the number is about the engine and not the harness.
- **P58.4 -- fewer than 30 D1 misses per record**: the memtable's chunk
  and slot, the WAL buffer, the key copy.

## What decides the next lever

If P58.2 holds, the lever is the memtable: a batch-local staging buffer
that inserts sorted runs, or a cheaper probe. If the frame encode or a
copy dominates instead, the lever is the WAL path: write frames straight
from the caller's buffers. If the driver dominates, the measurement is
wrong and gets fixed first.

## Outcome (cachegrind and callgrind, 100,000 records, subtracted)

Per appended record: **1,359 instructions, 22.5 D1 misses, 7.9 LL
misses** (283.9M - 148.0M instructions over 100,000). P58.1 and P58.4
held. The other two were refuted, and the refutation is the finding:

- **The harness is 47% of it.** `loadprof` -- and `ext-kv`, whose
  adapter takes `&[(Vec<u8>, Vec<u8>)]` -- allocates and frees two
  vectors per record: malloc 15.3M, free 21.2M with a memset of every
  freed chunk (16.7M, 200,055 calls), the copies into them 6.3M, and the
  payload generator 20.5M. About 640 instructions and most of the write
  misses per record, paid identically by every engine, so every load
  ratio the external suite reports is compressed toward 1.0 by a term
  that belongs to neither engine. P58.3 (under 15%) refuted.
- **Inside the engine, 677 per record**: `Wal::frame` 227 (the CRC 92,
  copies into the pending buffer 47, the rest the encode), the memtable
  probe and entry 180 or so with 3.6 D1 and 1.5 LL read misses -- half
  of all the run's last-level read misses land there --
  `MemTable::push_chunk` 74, the adapter's own loop the rest. The WAL
  frame is the largest single cost, not the memtable; P58.2 refuted
  narrowly. Table growth is not visible: no memset comes from the
  engine.

What decides the next lever, then: not compute first. Put beside f57's
decomposition, the durable load on x86 is 0.78 microseconds a record
waiting on the barrier (of which the journal commit was 0.18 and is now
priced), 0.24 waiting on seals (the drain at the end, mostly), and about
0.4 of compute of which the engine's share is perhaps 0.2. The two
cheapest moves are the harness -- borrow the batch instead of owning it,
which moves every engine's number and none of the comparisons' honesty
-- and a per-batch CRC in place of the per-frame one, 92 instructions a
record for the same torn-batch semantics, since replay drops a batch at
the first frame that fails either way.

## After the borrowed batch

The harness change measured the same way: **1,037 instructions and 19.0
D1 misses a record**, from 1,359 and 22.5, with the engine's 677 exactly
where it was. What remains of the harness is the payload generator and
one copy of each key and value into the batch arenas.
