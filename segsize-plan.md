# f52: segment size — registered before the run

The brief has listed "segment size: trades WAL replay length against
segment count; needs its own sweep" as open since it was written. It is
now the ingest lever with the most in it, for a reason f49 and f51 made
plain: at the shipping 64 MB seal, a 116 MB load seals once inside the
window and does everything else -- the second seal, and the partitioning
merge that rewrites the live set -- inside the drain, on the committing
thread's clock. Smaller seals move that work onto the machine's other
cores while the load is still running. The price is write amplification:
a seal's memtable holds keys from the whole key space, so its pieces touch
every partition, and every merge round rewrites the live set again.

## The experiment

f52 runs `seal_bytes` = 64 MB (shipping), 32, 16 and 8, interleaved, on
f49's shape (1M keys, 1,000-record durable batches, 100-byte values,
partitioning on, the drain inside the window), with the phase accounting,
device bytes and disk bytes, and a point-read sample after the drain. No
engine code changes: the arms are one option apart.

## Predictions

- **P52.1 — 16 MB seals lift ingest-to-routed by at least 1.2x over 64 MB**
  (`stats::compare` Greater). Merges overlap the load on idle cores; the
  drain shrinks to the last piece and the ranges it touches.
- **P52.2 — at 16 MB, device bytes are at most 2.0x the 64 MB arm's.** Each
  merge round rewrites the live set; at 16 MB there are about seven rounds
  over a live set that grows from 16 to 116 MB, roughly 460 MB of merge
  output against the single ~116 MB round at 64 MB. Refuted high means the
  per-range merge is rewriting more than the ranges the new pieces touch.
- **P52.3 — reads after the drain do not differ across the arms.** After
  the drain every arm is partitions only, and the partition count is set by
  `max_keys`, not by the seal size.
- **P52.4 — the sweep has an interior optimum: 8 MB ingests no faster than
  16 MB.** Below some size the merge amplification and the per-seal fixed
  costs (index build, fsync, publish) take back what the overlap gave.

## What this decides

The shipping `seal_bytes`, and whether the incremental merge is the next
build (if P52.2 refutes high, it is) or the sweep alone buys the ingest
back (if P52.1 holds at a tolerable P52.2).

## Amendment, registered before the second run

The first run refuted P52.1 and P52.3 together and for one reason: the
first partitioning sized its partitions from the seal size (3, 6, 12 and
24 partitions for 64, 32, 16 and 8 MB), and more partitions read slower
after the drain (7% at 16 MB, 12% at 8). What held instead was an interior
optimum at 32 MB: 1.142x ingest at identical device bytes, because three
seals overlap the load and no extra merge round is triggered. So the
partition size is decoupled from the seal size (`NextOptions::partition_bytes`,
`None` keeps today's coupling) and f52 gains a fifth arm, 32 MB seals with
64 MB partitions.

- **P52.5 — 32 MB seals with 64 MB partitions ingest at least 1.10x the
  64 MB arm** (Greater at the 5% floor), keeping what the first run found.
- **P52.6 — and read no slower than it after the drain** (`no_difference`),
  because they leave the same three partitions behind. If both hold, the
  shipping configuration becomes 32 MB seals over 64 MB partitions and the
  canonical run is taken again under it.
