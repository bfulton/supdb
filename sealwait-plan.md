# f60: the seal wait on the commit thread — registered before the run

f57 put the x86 durable load at roughly 45% commit phase, 14% seal phase
and the rest compute. The seal phase is everything `join_seal` does on the
commit thread: waiting for a seal thread that has not finished when the
next seal comes due, the final drain a `flush` performs, and publishing
the manifest with its write and two barriers. Which of those it is decides
whether there is a lever -- a deeper seal pipeline, a cheaper publish --
or a benchmark shape, since a load-then-flush window charges the drain to
the engine and LMDB has no drain to be charged.

## Predictions

- **P60.1 -- under sequential keys, at least 60% of the seal phase is the
  drain.** Four seals of 32 MB in a ~2 s load; the seal thread writes
  32 MB in well under the 0.5 s it takes the memtable to refill, so joins
  mid-load find it finished, and what remains is the last seal, waited
  for in full.
- **P60.2 -- the commit thread blocks on an unfinished seal for under 3%
  of the window under sequential keys.** Same argument.
- **P60.3 -- publishing is under 15% of the seal phase.** A manifest is
  a few hundred bytes, an fsync and a directory fsync, four times.
- **P60.4 -- under uniform keys the blocked share stays under 5%.** The
  merges run on their own thread and are booked to the merge phase; the
  seal thread's work is the same as under ordered keys.

## What would refute it

Blocked joins above a few percent say the seal thread cannot keep up with
the commit thread at 32 MB seals on this device and a second seal in
flight is worth building. A publish share above 15% says the manifest's
barriers belong off the commit thread. A drain share above 60% says the
number to fix is the benchmark's, and EXT.22 should be read knowing that
LMDB's last batch is durable when its commit returns and the next
engine's last seal is charged to the same window.

## Outcome (full, `results/f60-sealwait.full.json`)

All four held, and more sharply than predicted: **zero blocked joins**
under either key order, publish 8 ms (2%), and the drain 74% of the seal
phase -- 0.263 s of a 2.301 s window under sequential keys, 0.306 s under
uniform. There is no engine lever here: the seal thread keeps up, the
manifest is cheap, and the seal phase is the last memtable being written
and partitioned because the adapter's `sync` drains. RocksDB's `sync`
fsyncs its WAL and leaves its memtable and level 0 where they are, so 11%
of the next engine's load window is work its comparator defers. The
decision that follows is about the benchmark's shape, not the engine:
either the next engine's sync stops draining (and the reads after it are
measured against a store with an unsealed tail, as RocksDB's are), or
RocksDB's sync flushes and compacts (charging it the same drain). Both
arms should run; neither has yet.
