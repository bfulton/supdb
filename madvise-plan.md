# f65-madvise: is the out-of-core cliff readahead, and what does the fix cost?

Written before the first run, so the predictions cannot be fitted to the
numbers. Outcome appended after.

## The question

`F1.2` records the engine's largest standing limitation: once the file
outgrows the memory that can cache it, point reads fall about three orders of
magnitude, and `F1.4` records that the latency distribution goes bimodal with
it. That claim's `because` names a mechanism -- readahead thrashing, with
86,977x read amplification under the kernel's default advice against 141x
under `MADV_RANDOM`, and 25.2x on throughput.

Two things are wrong with leaning on that today.

The experiment it came from, `f23-madvise`, retired with the old engine.
There is no `results/f23-madvise.*` in the tree, so that number is not
evidence here; it is a hypothesis inherited from a run nobody can re-open.

And the remedy it points at is already written and wired to nothing.
`MmapBytes::advise_random` at `src/bytes.rs:190` issues `MADV_RANDOM`; the
trait's default is a no-op for sources with no mapping. Nothing in `src/`,
`tests/`, `bench/` or `web/` calls it. The engine's segment reads go through
`Blob<MmapBytes>` opened in `Db` and take the kernel's default.

So: re-establish the mechanism on this engine, and price the fix on both
access patterns before wiring it in.

## Why both patterns

`MADV_RANDOM` does not make faults cheaper. It turns readahead off. That is
the whole benefit on a random point read -- the kernel stops fetching pages
around one the reader will never touch -- and it is a straightforward cost on
an ordered scan, where every page it would have fetched is a page the scan
was about to want.

The engine does both. A verdict from the random arm alone would recommend a
setting that pays for point reads with scans, which is the axis this project
has already chosen twice (`EXT.24`, `EXT.30`, `EXT.34`). So the experiment
measures four arms: {random point read, ordered scan} x {default, random}.

## Design

Four arms, interleaved in one process through `Trial`, one file built once
and read by all of them, so nothing is compared across runs.

Out-of-core is forced with the v1 memory controller (`env::cap_memory`,
`env::cap_guard` to lift it -- a cap is a property of the process and the
suite has been killed once by one that was not lifted). The page cache is
dropped between repetitions. Both are checked, not assumed:

- the cap must be applied *and* the file must exceed it, or every finding is
  `Finding::not_exercised` (Rule 3). A run that could not make reads cold has
  nothing to say about cold reads.

Rule 4: throughput never travels alone. Each arm reports its latency
distribution, peak RSS, and device read bytes from `/proc/self/io`, from
which read amplification is device bytes over payload bytes asked for.

## Registered predictions

| | outcome | reading |
|---|---|---|
| P1 | `F65.1` holds: random reads at least 2x faster advised | the cliff is readahead, and the fix is one call |
| P2 | `F65.1` no difference | the collapse is fault cost, not readahead. `F1.2`'s cited mechanism does not reproduce on this engine and its `because` must stop asserting it |
| P3 | `F65.2` holds: read amplification falls at least 10x | the direct evidence for P1, and the quantity that does not drift |
| P4 | `F65.3` holds: ordered scan is measurably slower advised | the trade is real, so the advice belongs on a per-store option and not on by default |
| P5 | `F65.3` no difference | readahead was not buying the scan anything either, and the advice can go on unconditionally |

P1 with P4 is the outcome I expect and the least convenient one: it means
neither setting is right for every workload, and the engine needs an
`Options` field rather than a line in `Blob::open`.

## What this does not settle

`F1.2` and `F1.4` stay failing whatever happens here. Readahead is one of the
four costs Crotty et al. name; asynchronous I/O and eviction control are the
others, and `MADV_RANDOM` addresses none of them. A claim flip on those two
would need the out-of-core throughput to come back within 10x, which no
advice call is going to do.
