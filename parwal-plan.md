# f47-parwal: does the one-barrier commit scale across writers?

Registered before the run. Priority restated by the owner: this project
sacrifices space and complexity for time. That reopens f45 and f46, which
were declined against bars that priced complexity, and it makes the
brief's P-D -- writes scale with writers -- the largest unexplored axis,
because everything on the single-writer commit path is now at its floor
(F42.3: the lazy-seal arm runs past f39's raw+index floor).

## The question

One WAL stream commits a 1,000-record batch in ~0.84ms on this host,
almost all of it the fdatasync. Sharded writers would give each shard its
own WAL and its own memtable, so N shards issue N concurrent barriers. If
the device serves them concurrently, durable ingest scales with N until
something else saturates. If it serialises them, sharding buys CPU
overlap on the append and nothing on the barrier -- and the barrier is
most of the cost.

This is the same shape as f39: measure the floor with all engine work
removed, and let the number decide whether to build toward it.

## Shape

Arms at 1, 2, 4 and 8 threads, each thread owning one WAL file and
committing its own 1,000-record batches (framed append + fdatasync), the
f39 raw-wal arm run N-wide. Aggregate durable records per second is the
metric. A fifth arm runs 4 threads that share ONE file under a group
commit -- appends interleave, one fdatasync per round covers everyone --
because that is the other way to spend N writers and it stresses the
device differently.

## Predictions

- **P47.1 — 4 independent streams reach at least 2.5x one stream.** This
  is P-D's bar, applied to the floor. Refuted means the barrier
  serialises at the device and sharded WALs cannot deliver P-D here;
  group commit becomes the only route.
- **P47.2 — scaling is sublinear past 4.** 8 streams under 1.6x of 4.
  Refuted (near-linear to 8) means the device has more concurrency than
  the design assumed and shard count should follow core count.
- **P47.3 — group commit over one file beats 4 independent streams.** One
  barrier amortised over four writers' batches should cost less than four
  barriers, if the device is the bottleneck. Refuted means barriers are
  cheap in parallel and independence wins on lock-free appends.

## What this decides

Whether P-D is built as sharded WALs, as a group-committed single WAL, or
not at all -- and the ceiling to register for it before the build.
