# f48-syncpolicy: fewer barriers per record

Registered before the run. f47 established that this device serves about
2,700 fdatasyncs a second however they are issued, so durable-per-batch
ingest cannot scale past ~1.6x one writer by adding writers. The lever
that remains is fewer barriers per record: the WAL is written every commit
and synced every Nth, with loss bounded at N batches on a crash. The old
engine offers this as `Sync::EveryN`; the new one has committed every batch
since milestone 1.

## Shape

Four arms of the same engine on the f42 load shape (1M keys, 100-byte
values, 1,000-record batches), interleaved: sync every batch (today), every
4th, every 16th, every 64th. Load throughput, phase split, device bytes.
The WAL is written on every commit in every arm -- the policy moves only
the barrier -- and recovery is unchanged: replay stops at the first frame
that is torn or missing, and the sequence-gap check refuses anything past a
hole, so an unsynced tail is lost whole and never served in part.

## Predictions

- **P48.1 — every-16 reaches at least 1.6x every-batch.** f42 puts the
  synced commit at 0.56s of a 1.05s window; removing fifteen of sixteen
  barriers should take most of that. Refuted low means the append and
  memtable, not the barrier, are what remain -- and f42's lazy-seal arm
  already sits past f39's raw+index floor, so that would say the floor
  itself is the wall.
- **P48.2 — every-64 gains little over every-16** (under 1.15x). Once the
  barrier is amortised over sixteen batches its share is small; past that
  the memtable is the cost. Refuted means barriers were an even larger
  share than f42 measured.
- **P48.3 — the unsynced window is lost whole and only whole.** A crash
  test, not a throughput one: kill after K unsynced commits, reopen, and
  every committed-and-synced record is present, no record past the first
  missing one is served, and nothing is duplicated. This is the contract
  the policy sells, so it is measured with the speed.

## What this decides

Whether the new engine offers bounded-loss durability and at what N the
gain saturates -- and, against f47, whether a single writer with EveryN
beats four sharded writers with Always, which decides which of the two to
build first.
