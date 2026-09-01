# f51: the barrier and the background writers — registered before the code

Written while f50, the index reruns and the canonical run hold the machine.

## What f49 left on the table

The bulk writer made the seal 2.5-3.2x faster and the whole
ingest-to-routed window 1.28-1.48x, and in every run the commit phase --
the WAL append and its fdatasync, the only work a batch waits for -- got
SLOWER when the seal got faster: 0.672s to 0.802s in run 1, 0.917s to
1.047s in run 3 (F49.1). The seal thread now pushes 64 MB at the device
while the commit path is issuing a barrier per batch on the same device,
and the barrier waits behind the seal's dirty pages. The same contention
sits under the merge, which writes 116 MB more during the drain.

Two levers, both in the seal and merge threads and neither on the commit
path:

- **I/O priority.** `ioprio_set(IOPRIO_CLASS_IDLE)` on the seal and merge
  threads, so the block layer serves the commit path's barrier before the
  background writers' pages. One syscall at thread start (`libc` is
  already a dependency off wasm). Whether the host's I/O scheduler honours
  the class is exactly what is not known and exactly what the run says.
- **Write-behind spreading.** The segment writer calls `sync_data` every N
  MB as it streams blocks, so its dirty pages leave in slices instead of in
  one 64 MB flush at `finish`. Standard library only. It can also go the
  other way -- more barriers from the seal contending with the commit
  path's -- which is why it is measured and not assumed.

Both go behind `NextOptions` knobs (`background_io`, `seal_sync_every`)
so f51 runs them interleaved against the shipping configuration in one
process, on f49's shape (1M keys, 1,000-record durable batches, drain
inside the window), with the phase accounting f42 added.

## Predictions

- **P51.1 — idle I/O priority takes the commit phase to at most 0.9x the
  baseline's** in the same run, with the seal and merge phases within
  1.15x of the baseline's (the background work is deferred, not
  multiplied). Refuted with the phases unchanged means the scheduler
  ignores the class on this host, which is a fact about the host worth
  recording once.
- **P51.2 — idle I/O priority lifts ingest-to-routed by at least 1.05x**
  (`stats::compare` Greater at the 5% floor). The commit phase is roughly
  a third of the window; taking a tenth off it is 1.03x, so this needs
  the seal and merge not to slow down in exchange.
- **P51.3 — spreading the seal's syncs every 4 MB takes the commit phase
  to at most 0.9x the baseline's** without lifting the seal phase past
  1.15x. Refuted the other way -- commit phase up -- means the extra
  barriers cost the commit path more than the smoother flush saves, and
  the knob ships off.
- **P51.4 — the two compose:** both together reach at least the better of
  the two on the commit phase. Not additive; at least not worse.

## What this does not touch

The memtable append path, which F48.2 puts at the floor once the barrier
is amortised, and `SyncPolicy::EveryN`, already priced at 1.63x (F48.1).
Both compose with anything here in principle; whether they do is the run
after this one.
