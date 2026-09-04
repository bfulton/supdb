# f39-walfloor: can a log-only commit reach the one-barrier floor?

Registered before the first full run, like `fanout-plan.md`. The next
engine's durability story is a WAL as the only mutable thing: a durable batch
is append + one fsync and *nothing else*. EXT.9 says today's engine loads at
0.348x of LMDB when both commit per batch (199,485 against 572,416 ops/s on
this host), and its prose attributes the residual to per-point arena append +
fsync + section work against LMDB's single page-chain commit. The Mac ladder
bracketed the barrier's share. What no record yet states is the *floor on
this host*: what does an ideal log-only commit sustain, with all engine work
removed? If that floor is below LMDB's recorded rate, the redesign's durable-
load promise is dead on arrival and should die here, before the design brief
is written.

## Shape

Three arms interleaved under `Trial`, fresh file per rep, the EXT.9 load
shape exactly: every key new, 100-byte values, a durability point every
1,000 ops.

- **raw-wal** — a plain file; per batch, frame the 1,000 records
  (length-prefixed key and value), one `write_all`, one `fdatasync`. No
  index, no engine. This is the syscall + device floor for a log-only
  commit.
- **raw-wal+index** — the same, plus a hash-map insert per op recording
  (offset, len) as a memtable would. The floor plus the bookkeeping no
  engine can skip.
- **supdb** — today's engine: `put` + `checkpoint` per batch with the
  value-carrying log (the shipped default), f36's log-values arm.

LMDB is not re-run; EXT.9's recorded 572,416 ops/s is cited as context and
nothing gates on a cross-run comparison.

## Predictions

- **P1 — the raw floor lands between 600k and 2.5M ops/s.** Basis: f13's
  2.4ms publish fsync is a large dirty mapping, not a 120 KB append; a small
  append + fdatasync on this host should cost 0.4–1.7ms per batch. Refuted
  low (< 600k, and especially < LMDB's recorded 572k) means a one-barrier
  log-only commit cannot beat LMDB's commit on this hardware and the
  redesign must find a different durability story (group commit across
  batches, or concede the axis). Refuted high (> 2.5M) means the fsync is
  lying (device write cache) and the arm needs `O_DSYNC`/barrier scrutiny
  before anything is believed.
- **P2 — the bookkeeping tax is under 20%.** A hash insert is tens of ns
  against a multi-µs batch commit share per op. Refuted means the memtable,
  not the log, is the next engine's write-path problem.
- **P3 — today's engine sits 3–8x below the raw floor.** That gap is
  exactly what the redesign claims it can recover by making the WAL the only
  write-path work. Below 3x means today's engine is already near the floor
  and the rewrite buys little on this axis; above 8x means the per-point
  work is even worse than EXT.9's decomposition suggests.

## What this decides

P1 holding gives the design brief a registered, measured promise: a WAL-only
engine's durable load on this host should land near the raw+index arm, and
missing it by more than the usual gate is a design defect, not noise. P1
refuting low kills the "beat LMDB durably" goal honestly and early, and the
brief gets written around bounded-loss durability instead.
