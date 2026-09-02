# YCSB with the next engine and RocksDB — registered before the run

`ext-ycsb` ran every engine once per workload, on the old engine set, and
carried one claim that compares unmatched arms. It now repeats and
interleaves like every other suite here (fresh load per rep, medians,
`stats::compare`), and gains the matched pair the niche question wants:
`next-nodrain` -- durable per batch, undrained after its load, as RocksDB
is -- against `rocksdb-tuned`. One hundred-record batches, each committed
durably, one million records and operations at `full`.

## Predictions

- **P42 -- YCSB-A (50/50 update-heavy, Zipfian): 0.8x to 1.2x, a tie.**
  Half the operations are 100-record durable batches, one fsync each on
  either side; the reads favour the next engine, the write buffer favours
  RocksDB.
- **P43 -- YCSB-C (read-only, Zipfian): 3x to 6x.** The 4.7x point-read
  lead of EXT.38, under a skew that keeps RocksDB's block cache hot.
- **P44 -- YCSB-E (short scans, 5% inserts): 0.5x to 1x, RocksDB or a
  tie.** Fifty-entry scans from random starts over an undrained store
  take the k-way merge (EXT.39), and the inserts keep the memtable
  populated.
- **P45 -- YCSB-F (read-modify-write): 0.9x to 1.3x.** Half reads at the
  next engine's advantage, half durable batches at parity.
- **EXT.3 stays holding**: Supdb's own A-to-C ratio under 10x.

## What would refute it

P44 above 1x says the merge is not what a fifty-entry scan pays; P43
under 3x says the read lead needs the drained shape after all, which
EXT.38 said it does not.
