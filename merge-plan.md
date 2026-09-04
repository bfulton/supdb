# f54: the incremental merge — registered before the code

f52 priced smaller seals at 1.5x the device bytes (F52.2) and named the
incremental merge as what stands between the engine and them. Reading the
merge before building it corrects the premise: the range merge is already
incremental. `maybe_compact` merges only the ranges that hold enough
pieces, and a piece is cut at the live fences when it is sealed, so a merge
round touches exactly the partitions the new data overlaps. For uniformly
random keys -- this suite's shape -- every seal touches every range, so
every round rewrites the live set and no selection of ranges can change
that. What is structural stays structural, and is recorded as such.

Two things are not structural:

- **The flush re-partitions everything.** `flush` calls the merge with no
  fences whenever level 0 is non-empty, which re-derives the boundaries
  from every key and rewrites every partition, whether or not it holds
  new pieces. With partitions present the flush should merge the ranges
  that hold pieces under the live fences, like the background trigger
  does, and leave the others untouched.
- **Key locality is not exploited by the benchmark, but exists.** A store
  fed time-ordered keys -- a log, which is what this engine was started
  for -- writes each seal into a few ranges. There the range merge pays,
  and the flush's full rewrite is the whole cost.

## The experiment

f54 runs, interleaved, on the f42 durable load with the drain inside the
window at 16 MB seals over 64 MB partitions (the shape where f52 found the
amplification): `flush-full` (today's flush) against `flush-ranges` (the
flush merges only ranges with pieces), each under two key orders, uniform
random and sequential. Device bytes, disk bytes, ingest-to-routed, phases,
partition count, and point reads after the drain.

## Predictions

- **P54.1 — with uniform keys, the range flush changes nothing:** device
  bytes within 1.05x and ingest a tie. Every range holds pieces; the
  selection selects everything. Refuted either way means the flush was
  doing something other than the merge it looked like.
- **P54.2 — with sequential keys, the range flush cuts device bytes to at
  most 0.6x the full flush's** at 16 MB seals, because a seal's pieces
  fall into one or two ranges and only those are rewritten.
- **P54.3 — with sequential keys, the range flush lifts ingest-to-routed by
  at least 1.2x over the full flush.** The drain's merge shrinks with the
  bytes it rewrites.
- **P54.4 — reads after the drain do not differ between the two flushes**
  under either key order: both leave a fully routed store, and the range
  flush keeps the boundaries where they were.

## What this decides

Whether the flush becomes a range merge (P54.1 says it is safe, P54.2 and
P54.3 say it is worth it), and the honest statement for the brief: on
random keys the merge's amplification is the two-level design's, and the
seal size sweep already found its optimum; on ordered keys the engine is
incremental.
