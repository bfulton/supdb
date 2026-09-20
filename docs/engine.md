# The engine: a design brief written against the measurements

This brief was written against measurements taken while the engine was
built, and it states what they showed. Every figure in it is a number one of
those measurements produced, rounded, on the host and at the scale it names.
The brief exists because the previous engine's remaining failures were
structural rather than incidental, and because the two load-bearing unknowns
of the obvious replacement shape -- what a commit costs with all engine work
removed, and what segmentation costs a read -- had been measured instead of
assumed. Nothing below was built when it was first written; the promises
were stated first so the build could be checked against them.

## Why start over

Three measured facts no iteration on the previous design could fix:

1. **Index publication is O(key count).** `checkpoint` rewrites the whole key
   index (a 1,000-op durability window costs 25x; checkpoint is 44% of a
   bulk load; YCSB-E loses at 0.43x even unmatched). The value log made
   durability points cheap but any checkpoint that publishes index state
   still pays in proportion to keys, not to change.
2. **There is one appender.** Write throughput barely scales with writer
   threads, and the single appender mutex is why.
3. **The mmap read path degrades 916x out-of-core**, with default readahead
   amplifying a random read 86,977x and no auto-picked threshold that works,
   on either of the two tried.

And one measured fact that says what must survive: the read lead is real,
replicated, and mechanistic — the flat-index probe beats the B-tree descent
per lookup (1.355x on x86, 2.42x on Apple Silicon; two decomposition runs
agree the lead is per-lookup compute, not cache-line or page-size luck).

## What is inherited unchanged

- **The sealed-segment read path.** `flatindex` over a packed section, the
  `block` decoder, `Bytes`/`Blob`. The index layout study already put this
  layout on the frontier -- it beats a bulk-loaded B+tree on speed and size,
  and nothing composite scans faster -- and a sealed segment is byte-for-byte
  the shape `Blob` reads today — the browser reader carries over whole.
- **The schema-property fast paths.** `count_fixed` / `scan_counts_fixed`,
  which the browser reader already depends on.

## The shape

A WAL is the only mutable thing. Sealed segments are immutable. There is no
checkpoint.

- **Commit** = append the batch to the WAL, one fdatasync. That shape with
  all engine work removed measures **1,191,125 ops/s** on this host
  (0.84ms/barrier), 2.08x LMDB's measured durable load, and **1,014,003**
  with the per-op bookkeeping no engine can skip. The previous engine
  committed 5.85x below its own floor on work — arena append, section
  publication — that this design deletes rather than optimizes.
- **Ordered ingest** = a run of keys above the store's greatest, with
  values the record holds inline, goes to an ordered memtable -- appended
  in key order, searched by binary search, never hashed -- and at each
  commit to a segment open for append: the batch's records, a commit
  marker, one fdatasync on that file, and the WAL untouched. No seal and
  no partitioning pass follow, since the keys are already in their final
  order above every partition. The segment closes at the seal threshold,
  or at the first write the run cannot take, on the seal thread, and
  joins as a piece the promotion rule renames. Detection needs no
  interface: the store knows its greatest key. `direct_ingest: false` is
  the WAL path, kept as the comparison arm.
- **Seal** = when the memtable reaches segment size, write one immutable
  segment (data blocks + its own flat index), fsync it, truncate the WAL.
  Sealing is off the commit path; a durability point never publishes index
  structure, which is what removes the checkpoint's mechanism rather than
  its cost.
- **Read** = probe segments, newest first. Both halves of this were
  measured: segmentation itself is free (sixteen perfectly-routed segments
  indistinguishable from one store), and unrouted probes cost 90ns each,
  which kills the read lead already at four segments — the plan said it
  survives k=4 and it does not. **Routing is therefore required, not
  optional** — and every candidate shape was measured. Per-segment blocked
  Blooms keep 82% of the single-segment rate (a fixed probe order queries
  ~8.5 filters per lookup). A generic global map manages 62% of the ceiling
  and a purpose-built one-line fingerprint table 71.5% at 6.7x the blooms'
  memory for a statistical tie with them: at 1M keys any router consulted
  per lookup pays a DRAM miss on a keys-sized structure. The conclusion is
  structural — the only free routing is information the reader already
  holds, so **routing belongs to compaction, not to filters**: compacted
  levels are key-range partitioned and a two-comparison fence routes them
  for nothing (the same fence measured inert on overlapping ranges), while
  the small unpartitioned tail of recent segments carries per-segment
  Blooms. **The ceiling this paragraph was written around did not survive
  contact.** The oracle run had sixteen perfectly-routed segments reading
  20% *faster* than one store (566ns against 522) — but that oracle knew the
  segment by arithmetic. Built, with a fence search and Blooms and a real
  tail, the same shape reads **71.4%** of one store at the same scale. The
  routing conclusion above still stands on its own evidence; what does not
  stand is the assumption that routing recovers everything fan-out spends.
- **Compact** = merge segments under a policy already priced: geometric
  size ladders bought 3.963x on fragmenting writes for a 0.762x read tax.
  The clause that used to follow — "read cost does not force merging" — is
  **wrong as built**: unrouted segments read 864,624/s against ~1,020,000
  routed at 1M keys, so merging is what buys routing and reads do force it.
  What the 1M-key run also shows is that the merge cannot keep up: it
  rewrites the whole live set, so the tail settles where merge duration
  puts it (5–6) no matter what `l0_trigger` says, and compaction costs 42%
  of the durable load. **The incremental merge is the design's largest
  outstanding debt**, named independently by the compaction-policy run and
  the 1M-key run.
- **Delete** = a tombstone. In the memtable it is a chain chunk with a
  marker length and the key's live count resets at it; the seal writes the
  values after the newest tombstone and sets the flag bit format v5
  reserved beside the extent's count; a level-0 piece records at open
  whether any of its extents carries the flag. Reads, counts and scans find
  the newest source holding a tombstone for the key and start there -- a
  pass only a store with tombstones in it pays, and one that costs a second
  probe on the sources that hold the key. Every merge writes the bottom
  level, so a tombstone never survives one: values older than it are
  dropped, a key with nothing live is left out, and its bytes come back at
  the next merge that reaches it. What that costs and what it returns is
  measured under the next point.
- **Commit is a batch, and a batch is atomic.** WAL frames carry a kind --
  put, delete, commit -- and replay applies the frames between commit
  frames whole or not at all; a partial batch used to replay as whole,
  which the first test written against the contract found. `Txn` stages
  puts and deletes and commits them as one batch behind one barrier, reads
  through it see its own staged writes, and drop is abort with nothing to
  undo. The engine is single-writer and a read borrows it, so no read
  observes a batch half-applied. That is the transactions axis of the
  matched comparison, which LMDB held over every Supdb arm until now.
  Measured, all of it costs this: the commit frame is free (a tie on the
  raw shape); a tenth of the keys deleted before the drain leaves 0.913x
  the disk; a deleted key costs a miss, 170 against 194 ns; present-key
  reads after the drain are unaffected because partitions never carry
  tombstones; and the merge is unaffected. Format v5's count field, which
  the tombstone bit rides on, costs 6 B a key -- decomposed to the byte,
  beside the 8 B a key `index_inserts` had already added.
- **A run of one width is written without prefixes (format v6).** The
  segment writer decides at `end`: if every value in the run has the same
  length the values go back to back and the extent carries `Ext::FIXED`
  beside the tombstone bit, the width being `len / records`; a mixed run
  keeps the varint form, and the merge re-encodes from values so the flag
  is a property of the run it describes. A read of a fixed run is a copy
  of its bytes, and `Blob::intersect_fixed` walks two keys' runs in place.
  Priced on the analytics workload: the full-list read from 0.307x of
  LMDB's DUPFIXED to parity or better, the intersection from 0.769x to
  1.15-1.19x, the day index from 5.02 MB to 4.05. The canonical load's
  100-byte values are uniform, so every run there is now fixed as well; its
  numbers were last taken on v5.
- **A segment's blocks can be compressed.** `set_compress` takes the path
  `write_block` always had: chunked above the chunk size, verbatim when it
  does not pay, and a verbatim block carries per-chunk checksums so a run
  read plans chunks. 19.9% of a real day index, against a predicted 25%.
  Postings stored as absolute ordinals compress by nothing at all, which is
  a fact about LZ4 and counters rather than about the writer.
- **A segment opens sparsely in one round trip.** The superblock page's
  spare 3 KiB carries an extension -- header copy and every region's offset
  -- so a sparse open plans itself from the first probe; a head reserve
  (`set_head_reserve`) holds the block table, the checksum row, and copies
  of the fence and directory so a generous probe opens in one wave; data
  reads fetch the chunks a run spans, not the block. Counting the waves, a
  cold search is open, records, postings.
- **A segment's key index is checksummed.** The key section ends in a row
  of CRC32C words, one per 16 KiB object page it touches, named by two
  spare header words; `Blob::open` verifies every piece once -- 26 ms for a
  million-key segment, more than predicted, because with inline runs the
  index is the data -- and no read pays after. The sparse reader rounds its
  plans to the same pages and verifies each on first use. A store's
  in-place-editable index carries no row. `tests/segwriter.rs` flips every
  seventh byte of a segment's key section and requires the open to fail.
- **Write scaling** = one active memtable+WAL per shard or per writer;
  segments make the shared-appender mutex unnecessary rather than cheaper.
- **I/O** = the read path is `Bytes` all the way down; mmap is one backend,
  explicit reads another. One read path instead of the previous two, and
  the out-of-core decision becomes a byte-source choice the caller makes
  instead of a policy the engine mispicks.

## The promises, and what the build measured

Each was measured by the same experiment that convicted the previous
engine, interleaved in one process where the comparison allows:

- **P-A, durable load: met, and the commit path is now at its floor.**
  The canonical load shape reads **955,714 ops/s** against a bar set at
  600,000 — and the lazy-seal arm at 1,029,190 is *past* the raw+index
  floor of 1,014,003 measured above, so the append-and-commit half of the
  engine has no measured headroom left. Phase accounting puts 0.56s of its
  1.05s window in the WAL append and its fdatasync, which is the only work
  a batch waits for.

  What that leaves is not on the commit path at all. Against LMDB the
  durable load read **0.299x** when this was first written and reads
  **0.694x** in the latest run (0.49-0.51x the two runs before). The last
  step is piece promotion: the canonical load's keys ascend, so every
  seal's keys lie above the last partition's and the drain routes by rename
  with no merge -- say so when quoting it, because a uniformly random key
  order does not qualify and sits near 0.42x. A store's first flush now
  writes the partition's name in the seal itself when the flush would
  partition and the piece is tombstone-free and fits one, which is what
  the promotion would have
  linked it as under a second publish: at ten thousand keys, where the
  drain was a third of the load, the probe's drain went from 6.4–8.1 ms
  to 4.5–5.0 ms over four rounds alternated, the promotion's link, second
  open, directory sync, manifest sync and directory sync gone, and the
  seal threads' own directory sync with them, since the publish's covers
  the entries they made. The transactions axis is
  matched, so it is a measurement and not a bound. Leaving partitioning to
  compaction no longer separates the arms at this load (ties both ways).
  The whole of that move is below. The gap on random keys is there because
  the seal and the flush's partitioning land *inside* the timed window.
  That is overhead rather than bytes, and half of it is a policy choice --
  leaving partitioning to background compaction measures **1.985x more
  ingest** and 36% fewer device bytes, for 5% of read throughput, and
  2.007x on the ordered scan is what it costs. The rest was the seal
  writing each segment through `Store`'s general put path -- hash table,
  freelist, arena, per-key bookkeeping, a checkpoint publishing a
  million-key index -- for input that is already sorted, immutable and
  written once.

  **That writer was priced, declined, and then built.** Its floor was
  measured at 2.04-2.06x the general path (replicated), under the 3x that
  had been set as the price of a second writer in the format layer, and it
  was declined. The standing priority then changed -- complexity is spent
  for time -- and `supdb::SegmentWriter` now writes every seal and merge
  output in one forward pass, same format, same `Blob`. Run against the
  general writer interleaved in one process on the canonical load shape
  with the drain inside the window, three times: the seal phase was
  **2.5-3.2x** faster and ingest-to-routed **1.28-1.48x**, p=0.0022 each
  run. Those findings retired with the writer they were measured against;
  the comparison that remains is between the two merge strategies. The
  merge's input side -- collect every key, sort, one hash probe per key per
  input -- then went to a k-way walk over rank cursors, worth 1.305x on the
  merge phase and 1.104x on the window (both *under* the bars set for
  them), for **1.416x** at the shipping configuration in the same run.
  Three predictions fell the other way and each names its mechanism: the
  disk saving is 0.945x rather than the 0.9x promised (the 180 MB is
  records, key index and tables, not slack); reads over bulk segments are
  **1.09-1.13x faster** where a tie was predicted (same layout after the
  drain and the control ties, so it is the writer's block placement, not
  yet isolated); and the merge is **write-bound now** -- its remaining 1.2s
  is 116 MB of output at the writer's own speed plus its fsync, so finding
  keys faster was worth a third, not a half.

  What is left on ingest after that is bytes and barriers, not bookkeeping:
  the routed shape reads and writes the data a second time by design, the
  seal's and the merge's fsyncs sit on the drain, and the commit phase
  rises when a seal runs beside it. The two cheap answers to that last one
  were tried -- idle I/O priority for the seal and merge threads, and
  spreading the segment writer's syncs -- and both are inert on this host
  (every comparison a tie), so the barrier's growth is not a
  queueing-order effect here and both knobs ship off. The partitioning
  pass itself stays optional. The segment-size sweep this brief owed since
  it was written is done: 32 MB seals over 64 MB partitions ingest 1.129x
  at the same device bytes and the same reads, and are the shipping
  default now; smaller seals buy nothing until the merge is incremental.
- **P-B, the read lead survives: met, with its condition stated.** The
  test was "the canonical read shape with live segment counts under the
  compaction policy stays ≥ 1.2x on x86". At the shipping configuration it
  reads **2.2-2.5x** across the three full runs with inline runs and
  1.4-1.6x across the seven before (ten consecutive runs, each p=0.0022);
  the tenth, at 2.208x, is on a recovered host state, which is the
  measurement the two before it owed. The alternative to routing was then
  re-priced under inline runs and lost: four Bloom-routed pieces read at
  0.79x of four fence-routed partitions, seven at 0.69x, and the ordered
  scan at a quarter. Routing at rest stays.

  Getting here took two corrections and one reversal worth keeping. The
  reversal: at 8+ segments the same data reads **0.846x** and **0.850x**
  (replicated), and the 1M-key run has it at 0.77x. Segment count is the
  variable that decides this axis — one segment 1.19x, eight 0.77x — so
  the promise is conditional by construction and the condition is part of
  it.

  The corrections were both mine. Three early readings of 1.4–1.7x had
  the level structure idle *and* served a large share of their keys from
  a resident hash memtable, which is not the engine LMDB was being
  measured against; the adapter now drains before reading, so every key
  is sealed on both sides. And a flush now leaves the store **routed** —
  it partitions what it sealed — so a read touches exactly one segment
  rather than paying a Bloom check on each of several overlapping ones.

  The engine reads at or above what the 1M-key run measures for the same
  data in a single segment, so segmentation costs nothing at this
  operating point and the read path itself was the ceiling. Past that
  ceiling needed fewer cache misses per lookup, and that is what inline
  runs are: a run of values up to 256 bytes lives in its index record, and
  a read of it touches the hash slot and the record and never the block
  table or a block. Measured interleaved against block-backed runs three
  times: point reads **1.36-1.72x faster**, disk within 1.7%, and -- once
  the writer streamed the section records-first instead of building it at
  the end -- ingest **1.15-1.16x faster** too (the first run's 0.807x is
  why the layout changed). The prices are on the sequential walks, where a
  record that carries its values is wider: the ordered scan 0.86-0.90x and
  the dictionary count 2.3-2.9x per key, both counted as the trade rather
  than netted against the gain.
- **P-C, the durability curve flattens:** the durability-window sweep
  shows window cost independent of key count — the 25x at a 1,000-op
  window becomes a bounded, window-size-only cost. That finding flips or
  the design failed at its main job.
- **P-D, writes scale: ruled out at the floor, before the build.** Raw WAL
  streams run N-wide with no engine work: four independent streams commit
  **1.61x** one stream, eight add nothing over four, and a group commit
  over one file *loses* to independence at 0.784x because the mutex costs
  more than the shared barrier saves. This device serves ~2,700 barriers a
  second however they are issued, so durable-per-batch ingest cannot scale
  past ~1.6x one writer here by any arrangement of writers. Sharding is
  still worth building for 1.6x under a spend-complexity-for-time
  priority, and its bar is now **1.6x, not 2.5x**. Where ingest headroom
  actually lives on a barrier-bound device is fewer barriers per record:
  larger batches, or a bounded-loss sync policy. **Built and measured:**
  `SyncPolicy::EveryN` syncs every Nth commit and writes the WAL on every
  one. Every-16 ingests **1.634x** every-batch (p=0.0022, commit phase
  0.84s to 0.28s, device bytes unchanged), every-64 adds only 1.087x over
  that, and a torn unsynced tail is lost whole and never in part. That is
  the same 1.6x sharding would buy, for a policy bit instead of N writers;
  the two attack different terms (barriers per record, barriers per
  second), so whether they compose is the next thing to measure rather
  than assume.
- **P-E, crash semantics: met, and sharpened.** A store killed before any
  seal opens from the WAL alone, history survives reopen (segments do not
  forget), and -- since the commit frame -- a batch is lost whole or kept
  whole: `tests/db.rs` cuts the WAL inside a batch's commit frame, at
  it, and inside its last record, and the batch is gone in every case and
  stays gone after the next commit, because `open` truncates the WAL to
  its last commit frame before appending behind it.

### Apple Silicon, replicated

The canonical pair taken twice on Apple Silicon: durable load a tie
(0.989x and 0.963x, both no difference, both engines at 160,000-175,000
ops/s because one F_FULLFSYNC per batch is the floor for either), point
reads **3.302x** and **3.177x**, ordered scan **1.203x** and **1.196x**,
every comparison at p=0.0022 with arms agreeing across the pair to within
1.5% on reads. The read lead is larger there than on x86 and the scan
axis, a coin toss on x86, separates cleanly, which is the shape the second
reader's campaigns had already found.

### Against RocksDB

The comparator that separates "the engine is fast" from "an LSM is
fast", matched on durability, atomic batches and checksums, at its
defaults with compression off. Durable ordered load **0.778x** (p=0.0033)
and RocksDB writes fewer device bytes and a smaller file, so the write
side goes to it; point reads **7.62x** and ordered scan **5.95x** stay
with the engine by margins the LMDB pair never showed; shuffled durable
load **1.18x**, the number the 6x over LMDB on shuffled arrival needed
beside it. The prediction was a tie on the load and 2-3x on reads, wrong
both ways. RocksDB's 8 MB block cache and absent filter are its shipped
defaults; tuned as deployed (a 256 MB block cache, a Bloom filter, four
background threads) its read moved 1.19x and its scan 1.09x at 1M keys,
so the pair reads **6.45x** and scans **4.70x** either way; the load stays
at 0.688x, the shuffled load a tie.

### The seal wait

`Db::seal_waits` splits the seal phase the commit thread pays. Under
either key order zero joins found the seal thread still running,
publishing the manifest is 2% of the phase, and 74% is the final drain:
the adapter's `sync` seals the last memtable and partitions it inside the
load window, 0.263 s of 2.301, where RocksDB's `sync` is an fsync of its
WAL. No engine lever there; both benchmark shapes now run. Neither
draining, the durable ordered load against tuned RocksDB is a tie
(0.904x) and the shuffled load 2.37x, the next engine's own arrival-order
swing gone; both draining, 0.815x. Point reads lead 4.7x undrained and
7.1x drained. The ordered scan is where not draining costs: 2.9M
entries/s over three unrouted segments and a memtable against 24.7M
routed (0.68x of RocksDB, a tie). Decomposed, the k-way merge is the
smallest piece of that gap -- 1.7x of routed for scans that start in a
segment, 2.3x for entries served from the memtable's range; the rest was
the sorted snapshot of the unsealed keys that the first scan after a
commit builds, at 300 ns a key over a memtable that still had a frozen
twin behind `sync`. The build is 5.8-9.8x cheaper now (the keys in one
arena, the slots radix-ordered by key offset so the copy is sequential, a
24-byte prefix sort), and the undrained scan moves 2.28x on that alone.
What remains for an undrained scan is the memtable's own 2.3x, which
sealing sooner would remove and a faster walk would not.

### Where a scan's time goes

Measured rather than guessed, and the guesses were wrong twice. `bench ab
--len` swept over scan lengths 1, 10, 100 and 1000, four threads at a
hundred thousand keys, seven pairs a point, fits microseconds per scan as
a fixed cost plus a per-entry one:

| | fixed per scan | per entry |
|---|---|---|
| `scan`, drained | 232 ns vs LMDB 126 (1.84x) | 3.99 ns vs 4.81 (**0.83x**) |
| `scan-mixed`, unsealed keys | 301 ns vs LMDB 134 (2.24x) | 6.91 ns vs 5.84 (1.18x) |

The walk is faster than LMDB's. What is lost is a fixed cost of about
230-300 ns a scan against its 130 -- the whole of why a scan of one entry
reads 1.69x behind and of ten 1.98x -- and an unsealed key at 6.91 ns
against this engine's own 3.99 on a drained store. At a length of a
hundred those are 167 ns and 290 ns of a 993 ns scan.

Callgrind over scans of one entry, which is almost all fixed cost, puts
about 1,285 instructions in a scan and attributes them:

| | instructions a scan | share |
|---|---|---|
| `OrdIndex::seek_exact` and `lower_bound` | 418 | 33% |
| `Reader::scan`'s own body | ~255 | 20% |
| `Blob::scan_at`, the one entry | 121 | 9% |
| the preamble: `enter`, `sync_log`, `build_ctx`, `refresh_snapshot`, `install_ahead`, `start_ahead` | ~136 | 11% |
| `first_reaching` | 36 | 3% |

So a third of a short scan is the seek, and the preamble -- where the
cost was assumed to be before this was run -- is a ninth of it. The
binary search is 139 of those instructions over about fifteen probes,
nine instructions a probe, which is a branchless search already; what
would cut it is fewer probes, not a cheaper one, and that is an index
layout question rather than a loop to tighten. `read_advice`'s prefetch
plan does not appear in the profile at all.

Count what the probe collects and not what it measures: the loop that
warms the handle before the timed one is inside the toggle too, so the
first reading of this was every figure doubled. The shares were right
and the totals were not.

Under the cache model a scan of one entry takes about 10.7 first-level
data misses and 0.3 that reach memory, on a partition of thirty thousand
keys that is entirely resident.

Then the same scan sampled for wall time rather than counted for
instructions, and it does not agree. Two million scans, `cpu-clock` at
5 kHz, keys formatted before the loop rather than in it:

| | wall time, len 1 | instructions, len 1 | wall time, len 100 |
|---|---|---|---|
| `Blob::scan_at` | **33.1%** | 9% | **77.6%** |
| `seek_exact` + `lower_bound` | 33.7% | 33% | 11.3% |
| `Reader::scan` | 13.6% | 20% | 4.2% |

`scan_at` takes three to four times the share of the time that it takes
of the instructions, which is what a stall looks like and what counting
instructions cannot see. So "a third of a short scan is the seek" was
half the story: in time the seek and the walk of a single entry cost
about the same, and by a hundred entries the walk is three quarters of
everything. The per-entry cost is the target, not the index's layout,
and `scan_at` is where the 6.91 ns an unsealed key costs against this
engine's own 3.99 on a drained store has to be.

The lesson is about the instrument. Callgrind is exact and deterministic
and says what a processor was asked to do; it says nothing about waiting,
and this read path waits. Both belong, and where they disagree the clock
is the one that pays.

Two things it told us not to do. `prefetch_lines` sizes its hint to the
block rather than to the scan, which looks like waste on a scan of one
entry: with it off, three runs of twenty thousand scans read 673, 673 and
669 ns against 654, 642 and 599 with it on, so it earns its place even
there. A single run had said the opposite by 57 ns, which is what a
single run is worth at this scale.

#### What the overlay costs, and where

The table above prices an unsealed key at 6.91 ns against 3.99 on a
drained store, which reads as a walk that got slower. It is not. A probe
of one handle over a hundred thousand keys, sixteen-byte keys and
forty-byte values, scanning at random start keys, drained against a store
where every key has been written again and left unsealed:

| entries a scan | drained | overlaid |
|---|---|---|
| 1 | 393 ns | 785 ns |
| 16 | 609 | 1,040 |
| 64 | 1,229 | 1,510 |
| 256 | 3,636 | 3,715 |

Fitted over 64 to 256 the overlaid walk costs **11.5 ns an entry against
the drained walk's 12.5**, and at 256 entries the two are two percent
apart. The whole of the overlay's cost is about 390 ns paid before the
first entry, and a scan of one entry is where all of it shows.

Shrinking how many distinct keys the loop asks for holds the whole access
chain in cache and separates what the path computes from what it waits
for:

| distinct start keys | drained | overlaid | gap |
|---|---|---|---|
| 1 | 162 ns | 245 ns | 83 |
| 64 | 265 | 365 | 100 |
| 512 | 279 | 504 | 225 |
| 8,192 | 356 | 718 | **362** |

So about 83 ns of the overlay is instructions and the rest is waiting,
and the overlaid path is two and a half times as sensitive to the working
set as the drained one (+473 ns against +194). That is the shape of a
deeper chain of dependent loads, not of more work: a walk reaches its
block through the slot, the `Arc`, and the copy's three buffers, where
the drained walk reaches the partition's records through a mapping the
seek has just touched. Asking in key order rather than at random is worth
about 130 ns to **both** paths and closes none of the gap, so it is the
depth of the chain and not the order the blocks are met in.

The cost arrives with the density of the overlay, and it arrives as a
cliff. Scans of one entry against the same partition with the unsealed
keys spread through it: none 394 ns, a thousand 414, five thousand 417,
twenty-five thousand 610, a hundred thousand 711. Up to a few percent
unsealed the block path costs what the bulk walk costs; past that the
blocks stop being clean, every one of them is materialised, and the
copies are a second heap the size of the partition they shadow --
1,552 blocks of three separately allocated buffers, about 7.8 MB beside
a 6 MB mapping.

Four things this says not to do, each measured and each refuted:

- **Scope the block prefetch to the scan's length.** It looks like pure
  waste to pull a block's entries, keys and a kilobyte of values to read
  one entry, and `prefetch_block` is 14.6% of an overlaid scan of one.
  With it off that scan costs 802 ns against 611; entries and keys
  without the values, 624. It is absorbing the walk's latency, which is
  what a prefetch high in a sampled profile usually means.
- **Stop taking `Arc::make_mut` per block to ask whether a form is
  wide.** Four alternating pairs: 651 ns against 661 at one entry, 975
  against 943 at sixteen, 1,851 against 1,853 at a hundred. The slot is
  uniquely held, so the call is one uncontended compare-exchange.
- **Prefetch the slot early.** The slot's address is known a few hundred
  instructions before the walk reads it, but one level of a five-deep
  chain is not worth hiding: three wins in four pairs at one entry, two
  in four at sixty-four, about one percent.
- **Walk the partition against the snapshot instead of copying.** The
  merge has no copy to chase, so it should win at short scans. It loses
  everywhere -- 1,391 ns against 686 at one entry, 38,068 against 3,701
  at 256 -- because it pays a binary search over every unsealed key on
  every scan, which is what the block path was changed to stop paying.

What did pay was removing work rather than adding it. `touched` is read
only by the shed, which returns at once with no budget, and `dense` is
written only by a promotion; both default off, and both were a cold line
apiece on every block walked. Skipping them took a scan of one entry from
678 ns to 650, three wins in four pairs, and nothing measurable at
sixteen entries or sixty-four. That is the size of a bookkeeping array
that stays resident: the 280 ns of waiting is the copies themselves.

#### What the forms at commit cost, and who pays

Two options decide this and `supdb-settle` moves both, which is how the
first reading of it went wrong. The arm is `forms_from_reader_scans: 0`
*and* `commit_forms_build: false`: it maintains at every commit whoever
is reading, and builds no form when it does. `supdb-forms` moves only
the first. Against `supdb`, fifteen pairs at ten thousand keys:

| | settle over supdb | forms over supdb |
|---|---|---|
| scan-lag, 10% unmerged | **4.157x** (15/15) | **3.186x** (15/15) |
| scan-lag, 1% unmerged | 1.222x (13/15) | 1.314x (13/15) |
| ycsb-E | **1.231x** (15/15) | 0.968x (6/15, ns) |
| scan-mixed 4t | 0.675x (0/15) | 1.036x (11/15, ns) |

So the two numbers have two different causes. ycsb-E's 1.231x is the
build: the regime alone does nothing for it. The lag sweep's 4.157x is
mostly the regime, 3.186x of it, and only the rest is the build.

The regime is the larger finding and it is the opposite of what this
file assumed. `maintain_forms` waits for a handle the *caller* made to
have scanned, on the reasoning that the forms "lose where the writer
reads its own store". The suite's lag store is exactly that store --
loaded shuffled, swept by `e.range` on the writer's own handle, and
never touched by a handle from `Db::reader` -- so under the default the
maintenance never runs there at all, and the writer's own reads pay at
scan time for the settling no commit did. Maintaining regardless reads
3.19x at ten thousand keys with a tenth of the store unmerged.

It is not a default, because it does not hold at size. Eleven pairs at a
hundred thousand: scan-lag at 1% reads 1.340x (10/11, p=0.012), at 10%
1.055x (ns), and at 100% **0.869x** (1/11, p=0.012) -- a real loss --
while the forms held go 1,537 to 3,099 and their bytes 1.30 MB to 2.82
MB, 2.18x the memory. A win of 3x at one rung and a loss at another
rung's deepest lag is a policy question, not a constant.

What does not work is letting the writer read what it maintains.
`supdb-wforms` admits the writer's handle when nothing has been written
past the commit the forms were settled at, which is the only window in
which a form is what a read honouring no watermark must see. It takes
them -- at a hundred thousand keys `form_takes` 24,225 against 51,113,
`canon_hit` 6,000 against 19,862, blocks built by the engine 2,801
against 2,653 -- and ycsb-E reads **0.981x**, one win in eleven,
p=0.012. At ten thousand it takes nothing the counters can see:
`canon_hit` stays at 600 while `canon_tried` goes 1,200 to 3,285, so
every writer scan there checked and missed. A probe of the lag shape
shows the other end of it -- the writer hitting 20,000 of 20,000 and
reading 2,026 ns against 1,938 -- so the admission is sound and the
reading is simply not worth what the check costs.

Two warnings from doing this. A quick row's small rungs are 3 ms
measurements: ycsb-E's 1.23x at ten thousand keys is real only because
the five samples of the two arms do not overlap (500-594k against
594-760k), and the same row's 1.06x at three hundred thousand is noise
(438-473k against 435-494k). And a probe that reproduces neither number
is a probe missing an ingredient, not a refutation: three of them here
ran the lag shape at about 51M entries/s, which is the *settle* arm's
44M and not the shipping arm's 10.6M, because none of them loaded the
keys shuffled.

#### Where the engine actually loses to LMDB: unmerged writes

The lag sweep is the whole story and it was being read one point at a
time. Scan throughput against LMDB's, over the depth of unmerged writes,
with `supdb-forms` -- the arm that maintains the forms whoever is
reading -- beside it:

| unmerged | 10k | 30k | 100k | 300k |
|---|---|---|---|---|
| none | 1.35 / 1.40 | 1.49 / 1.47 | 1.48 / 1.50 | 1.64 / 1.53 |
| 1% | 0.67 / 0.91 | 0.83 / 1.16 | 1.22 / 1.46 | 1.10 / 1.16 |
| 10% | 0.24 / 0.81 | 0.35 / 0.38 | 0.32 / 0.29 | 0.28 / 0.25 |
| all | 0.04 | 0.04 | 0.03 | 0.12 |

Drained the engine reads 1.35x-1.64x of LMDB at every rung. A tenth of
the store unmerged and it reads a quarter to a third of it; all of it
unmerged and a thirtieth. A B-tree has no unmerged state -- every write
goes into the tree and is paid for there -- so LMDB's scan is flat in
this variable and this engine's is not. Every workload that reads a
store with unsealed keys is a point on this curve, ycsb-E and the
threaded scan mix among them, and every other axis in the suite sits
between 0.7x and 1.6x. This one is the gap.

The regime moves the shallow end of it. Maintaining the forms whoever is
reading, rather than waiting for a handle the caller made, takes 1%
unmerged from 0.67x to 0.91x at ten thousand keys, 0.83x to 1.16x at
thirty, and 1.22x to 1.46x at a hundred -- paired, 1.314x (15/15,
p=0.000) and 1.358x (11/11, p=0.001) -- and 10% unmerged from 0.24x to
0.81x at ten thousand alone. It does nothing at 10% above that rung and
costs 0.869x-0.880x where the store is entirely unmerged, for 2.0x the
forms held and 2.18x their bytes.

`forms_max_unsealed_pct` was added to buy that back by stopping the
maintenance where the store is mostly unsealed, and it does not work.
At 25 it never fires usefully: the sweep's depth rises through the
write phase, so the forms are built before the bound trips and holding
them is what costs. Measured at the same rung, entirely unmerged reads
0.963x unbounded, 0.936x bounded at 25 and 0.954x bounded at 1 -- the
loss barely moves with how much maintenance happened, so it is not the
maintenance. The option and `supdb-regime` stay as the vehicle for the
next attempt, and at 1 they do bound: forms held 157 against 103 and
their bytes 0.185x, with the 10% win given up for it.

#### Capping the seal by a share of the store

`seal_bytes` and `seal_grows` are both floors, so on a store smaller
than the floor the memtable can hold the whole of it and never seal. A
hundred thousand keys are six megabytes against a 32 MiB seal, which is
why every point of the lag sweep sat unsealed and why the engine read
0.03x-0.35x of LMDB there. `seal_max_pct` caps the threshold at a share
of the store instead, ten percent, floored at a megabyte.

Paired against the arm that keeps the old shape, over the whole ladder
(15, 11, 11 and 7 pairs):

| capped over uncapped | 10k | 30k | 100k | 300k |
|---|---|---|---|---|
| scan-lag, all of it unmerged | **1.76x** | **2.51x** | **5.68x** | **2.23x** |
| ycsb-E | 1.02x (ns) | 1.02x (ns) | **1.31x** | **1.28x** |
| ycsb-F | 1.00x (ns) | 1.04x (ns) | 0.81x | 0.92x |
| load ops/s | ns | ns | ns | ns |
| bytes on disk, device bytes | identical | identical | identical | identical |

Every starred figure is 0/n or n/n on the sign test at p<=0.016. The lag
point moves at every rung; ycsb-E moves at the two where the mixes write
more than the floor, since below that the cap never binds during them --
thirty thousand keys write about 150 KB through A and F against a floor
of a megabyte.

The trade is one-sided on the ladder, which is the whole reason to take
it. ycsb-E is the only mix this engine loses to LMDB -- 0.76x, 0.74x,
0.78x, 0.87x across the rungs, against 1.46x-6.39x on A, B, C, D and F
-- so this puts E at about 1.02x at a hundred thousand and 1.11x at
three hundred, and leaves F, which the cap costs, at about 3.3x and
5.8x instead of 4.08x and 6.27x.

The cap engages only once something has been sealed, because a store of
no bytes has no share to take. That is why the load axis does not move
and `device_bytes_per_byte` is identical with the cap and without: a
first load runs uncapped, and the cap is about the updates that follow.

The share cannot be pushed further, and the point that would want it is
scan-lag at a tenth unmerged, which the cap does not reach: the row puts
it at 0.23x-0.28x of LMDB at every rung, unmoved. At a hundred thousand
keys a tenth of the store is about 900 KB against the 600 KB that point
writes, so it never trips. Nine pairs there, with the floor at 512 KiB
so it is the share that varies: a twentieth reaches it, 1.711x (9/9,
p=0.004), and takes ycsb-E to 1.372x -- but ycsb-D falls to 0.749x,
ycsb-A to 0.879x, ycsb-F to 0.874x and the drained scan to 0.924x (1/9
each, p=0.039). A thirtieth reaches the same lag point and reverses
ycsb-E outright, 0.873x, with ycsb-D at 0.636x (0/9, p=0.004). Since
ycsb-D reads 1.41x of LMDB at that rung, a quarter off it is most of its
margin, and what it buys is about six percent more on ycsb-E. A tenth is
the point. The floor makes no difference there -- 512 KiB and a megabyte
measure the same at a tenth -- so it stays where the small rungs put it.

The floor, not the share, is what the sweep turned on. At ten thousand
keys the store is about 600 KB, so a floor below it binds and the
memtable seals on nearly every commit: at 64 KiB ycsb-F reads 0.571x
(0/15, p=0.000), at 256 KiB ycsb-E reads 0.792x and the threaded scan
mix 0.673x (0/9 and 1/9). At a megabyte the small rung is clean and the
hundred-thousand rung keeps the whole win, because a store too small to
have a lag problem is one the cap should not touch. Sweeping the share
instead -- a tenth, a quarter, a half -- moved the lag point around and
never recovered ycsb-E at the rungs where the floor was binding.

#### The small rungs, where ycsb-E still loses

The seal cap closed ycsb-E at a hundred thousand keys and above. At ten
and thirty thousand it does not bind -- the store is 600 KB and 1.8 MB,
and the mixes write about 150 KB against a megabyte floor -- and E stays
at 0.76x and 0.74x of LMDB. What is there instead, measured:

- **The commit-time fill costs E 19%.** `supdb-forms` against
  `supdb-settle` differ in `commit_forms_build` alone: fifteen pairs at
  ten thousand keys give ycsb-E 1.190x (15/15, p=0.000) without the
  fill, scan-lag at 10% unmerged 1.302x (13/15), against the fully
  unmerged point at 0.184x -- the fill is worth 5.44x there -- and the
  threaded scan mix at two threads 0.837x (3/15). A trade, and at this
  rung it is one the fill wins.
- **Moving the fill to the builder thread buys nothing.** Below
  `scan_cache_ahead_min_blocks` the builder declines and the writer fills
  inline at the commit, and that threshold was measured as the builder
  against *no* builder rather than against the inline fill. Measured
  against the inline fill, fifteen pairs at ten thousand: ycsb-E 1.010x
  (7/15, ns), the drained scan **0.700x** and scan-lag at 0% **0.705x**
  (1/15, p=0.001 both), ycsb-A 0.927x. The threshold stands.
- **A perfect `memcmp` would not close it.** `__memcmp_evex_movbe` is
  9.7% of a scan at this rung, all of it 16-byte key compares through a
  libc call, so inlining them is worth at most that -- 0.76x to about
  0.83x.

Which leaves the arithmetic. The best lever here, dropping the fill,
takes E from 0.76x to about 0.90x and costs the deep lag point 5.44x. So
the small rungs do not lose E to any one thing the block cache does;
they lose it to what a scan costs before it reads anything, 232 ns
against LMDB's 126 on a drained store, and that is spread -- roughly a
quarter in the seek, a fifth in the walk of the first entry, an eighth
in the preamble, an eighth in the prefetch. Closing it is a fixed-cost
problem, not a policy one.

#### The lag sweep measured a deferred settle

Three probes, four value sizes and a shuffled load later, the sweep's
figure at a tenth unmerged came from none of those. It came from a
warmup. The probe that finally matched it ran the suite's thousand cold
scans with no warmup before them, and read 17 µs a scan against 4 with
two hundred warm scans first: about 13 ms that the warmup had been
absorbing, charged to the first scans after the writes.

That 13 ms is the writes themselves. `maintain_forms` settles a commit's
batch into the forms only when a scan has happened since the last
commit, on the reasoning that a run of writes with no read between them
should pay nothing for a structure nobody is reading. So a burst settles
its first batch and defers the rest, and the first read after it files
every batch since -- nine thousand writes on the sweep at a hundred
thousand keys, on the first of a thousand scans. LMDB pays that at the
write. This engine was charging it to the reader, and the sweep was
measuring the bill.

`forms_settle_backlog_pct` bounds it: past that share of the store's
keys in unfiled writes, a commit settles whether or not a scan preceded
it. The trade against the arm that never does, eleven pairs at a hundred
thousand keys, the bound given as the count it was at that rung:

| backlog | scan-lag, a tenth | ycsb-A | ycsb-F |
|---|---|---|---|
| every commit | 3.56x | 0.68x | 0.67x |
| 2,000 | 3.51x | 0.81x (2/11) | 0.79x (1/11) |
| 5,000 | **1.90x** (11/11) | 1.07x (ns) | 0.88x (ns) |

Every commit rebuilds the sorted snapshot in a write-heavy mix
(`snapshot_builds` 9 against 24) and costs A and F a third; five
thousand costs nothing the sign test can see. It is a share and not a
count, for the reason the seal cap's floor is: the same five thousand
at three hundred thousand keys is a sixtieth of the store, the mixes
there write five times over it, and it reads the lag point at 3.59x for
ycsb-A at 0.66x and F at 0.80x (0/7). Five percent is the default, and
as a share it holds at that rung: seven pairs at three hundred thousand
read the point at 1.86x (7/7, p=0.016) with A at 0.95x and F at 0.89x,
neither significant, and nothing else moved.

Two percent became the default once a settle stopped publishing for
readers that are not there (see "Publishing for readers that exist"),
which took the copy out of every patch. Priced against five with that
in place: eleven pairs at a hundred thousand keys read the point at
2.31x (11/11, p=0.001) for ycsb-D at 0.86x (1/11, p=0.012), A, E and F
within noise and the two-thread scan mix at 0.97x (1/11, p=0.012);
seven pairs at three hundred thousand, 2.11x (7/7, p=0.016) for D at
0.82x (0/7). The probe with every one of D's commits and read windows
timed says what D pays: not the reads, which take 440-500 ns before and
after, but the one commit of twenty-five where the backlog crosses the
bound, 5.2-5.4 ms against 0.3-0.6 for every other, filing six thousand
writes that are mostly F's tail, and one window of reads at 550 ns
while the cache refills. The bound decides which mix files a burst's
tail, and D is the one committing when it crosses; under five percent
the same tail waited for E's first scan.

What a settled write costs was then taken apart with both instruments,
and they disagreed the way the profiling notes say they will. Callgrind
put a write at about 3,000 instructions: two fifths in `malloc`,
`realloc` and `free` -- a `run` buffer built from empty per write, and a
one-element `Overlay` -- a quarter in the sort that groups the batch's
duplicates through a tuple-and-closure key, an eighth in the seek, a
tenth in three hash probes for a slot the log had already named. A
reusable `run` buffer, the slot carried in `pending`, and a single
integer sort key took that to about 2,300 instructions, a quarter
fewer, and a write settled in the same 665 ns: the path waits, and what
it waits on is `Blob::key_at`, the partition's record at the write's
position, one cold line per write because `pending` is ordered by arena
offset and consecutive writes land nowhere near each other in the
partition. Resolving every write's position first, sorting by it, and
applying in that order -- a B-tree's bulk update -- reads 600 ns a
write: 10%, and not the half the profile's share suggested, since the
seek that finds the position is itself a random read and stays one.
Two thousand three hundred instructions in 600 ns at 2.1 GHz is an IPC
near two, so neither instrument's half is the whole cost now; the rest
is the work itself, a seek, a record read and a splice per write.
Splicing a block's writes together, now that they arrive adjacent, is
the next cut and a larger one.

#### What the bound moves, and where the loss actually is

A bound of two percent reads the tenth-unmerged point at 2.04x the
five-percent default at a hundred thousand keys and ycsb-E at 1.16x,
for ycsb-A and D at 0.84x. Two probes in the suite's exact shape --
its generators, seeds, batches and pass order, one for the mixes and
one for the sweep -- reproduce those ratios and say what each one is.

Filing a write into a block form costs the same whoever does it. On
the commit path the settle phase ran at 0.64-0.82 µs a write over
batches of six to nine thousand; on the first scan after a deferred
burst it ran at 0.68-0.93 µs over three to eight thousand. Same code,
same price, so the bound only moves the work. At the tenth-unmerged
point the five-percent default files at the first commit after the
previous point's scans and again at the bound, and leaves three
thousand writes for the first scan: 2.1-2.5 ms, charged to a window
of a thousand scans that takes 2 ms. Two percent leaves none, and the
first scan takes 6 µs. That is the whole of the 2.04x: a relocation,
not a speedup. At three hundred thousand keys five percent leaves nine
thousand writes and two percent leaves two thousand, the same shape.

What makes the move a loss elsewhere is that the forms belong to one
memtable generation and a seal discards them. Under two percent, A's
four settles build about a thousand forms in 4 ms, and F's two seals
throw every one away before any scan; the forms table holds zero at
the end of F under either bound. D's one settle under two percent
files F's tail and its own inserts, 1.3 ms in a 6.5 ms mix, which E
then reads: D pays and E gains less than D paid. Smaller batches also
fold fewer of a zipfian mix's duplicate keys, 210 ns a write in one
batch of five thousand against 385 ns over four of two thousand. And
the fully-unmerged point, at a tenth to a fifth of LMDB, is the same
discard at scale: ninety commits with two or three seals leave the
table empty at the scan under every bound, and the thousand scans
build nine hundred blocks at 17-22 µs each while the maintenance done
during the writes -- 25 ms of settle and install at a hundred thousand
keys under two percent, 45 ms at three hundred thousand -- was thrown
away.

`forms_settle_recent_pct` was the adaptive bet: settle by the backlog
only within that share of the store written since the last scan, on
the reasoning that a recent scan says the reads are near and a distant
one says the seal is nearer. It cannot tell the two bursts apart. A's
writes and the sweep's are each nine percent of the store after a scan
pass, so at the commit that decides they look the same, and A settles
four times under the window as without it. What the window changes is
F, whose tail it leaves unfiled, and D; F reads about 1.1x and D
unchanged, and E, the first reader after them, inherits eight thousand
writes to file instead of two and a half and reads about 0.9x. The
sweep reads as the bound alone does. The window is off, and
`supdb-recency` prices it. The loss the bound trades against is not
the timing of the filing but its discard at the seal, and a form that
survived a seal -- its content is unchanged by one, only the slots it
was resolved against move -- would make A's filing E's to read and
give the fully-unmerged point something to walk.

#### A form dropped by its writer stayed published

Reading for the carry below found a bug on the canonical path that has
been there since the forms went into the state. A reader takes a
published form as the block, and with the table complete takes an
empty slot as clean. The writer drops its own form for a block whose
overlay outgrows every form but the wide one -- the last block of the
last partition, which collects every key inserted past the end -- and
builds it wide, a form it never publishes. So the block's slot kept
whatever was published before it went wide, or nothing, and a handle
taking the forms read the last block short of every key inserted past
the end since: three hundred of five hundred in the test that found
it, and the suite's threaded scans after the mixes, which read through
handles over a store D and E have inserted into, were reading it that
way. The slot now carries a mark -- an empty wide form, which a reader
already treats as "build your own" -- whenever the writer drops a
block's form or holds it wide, and a debug assertion at the settle
holds that a block the writer has no form for has none published as
the block.

#### Forms across a seal: correct, and not worth it

The section above ends by pointing at the seal's discard, and this one
is what came of building the thing it pointed at. A form's content is
unchanged by a seal, so the writer can file its backlog at the freeze,
move the state's pointers into the state the freeze publishes and
again into the two the piece's join publishes, keep its own tables'
forms and remake only what named the old memtable: the pieces' bounds,
the snapshot's, the lists of keys filed since, the wide forms. It is
built, behind `forms_carry`, and held to the model through two seals,
a block gone wide and the merge that finally drops it.

It does not pay. `bench ab` over eleven pairs at a hundred thousand
keys reads ycsb-F at 0.83x (0/11, p=0.001) and ycsb-E at 0.89x (1/11,
p=0.012), A and D at 0.90x and 0.89x within noise, the fully-unmerged
lag point at 1.39x (9/11, p=0.065), and eighteen snapshot builds
against eleven; nothing else moved. The two probes say why. ycsb-E
inherits F's forms and does not read faster for them: the builder
ahead fills E's table on a spare core within E's first millisecond
whether or not the seal emptied it, and E's own settles now patch
carried forms -- a copy per touched block, since each was published --
where over an emptied table they patched nothing. The fully-unmerged
lag point scans with an empty
table under both arms, and the probe's partition and piece counts say
what emptied it: three to seven pieces still standing at the scans,
out of the dozen the burst sealed. The store merges during the burst,
a merge rewrites the partition, and no old form maps onto the blocks
of a rewritten partition. So the discard there is the merge's, the
forms are legitimately absent, and the scans build nine hundred blocks
from three to seven pieces at 13-18 µs each; carrying forms into
merges that drop them made that burst's writes 2.3x slower for
nothing. F pays the same way, at each of its two seals and every
settle after them: a settle over a table the seal emptied patches no
block, and that is what the carry takes away.

That last clause is a cost the carry did not create and made visible.
The writer patches its own copy of a form and publishes it by an `Arc`
clone, so every patch after a publish copies the block first, and a
zipfian batch of five thousand writes touches about a thousand blocks:
with publishing switched off through a temporary flag, a settle in the
mixes ran 25-30% cheaper and F read 1.10x-1.20x, while E read
0.91x-0.95x, which has no mechanism yet. Publishing for readers that
hold no handle is the lead this leaves.

#### The builder from the publish, at the fully-unmerged point

With the carry refuted, the same point was tried from the other side:
the builder ahead, which already makes exactly the forms those scans
build, started at the first commit after every publish instead of at
the first scan, and its forms installed at the commits they arrive at
rather than at that first scan, which had been paying 6-8 ms to splice
in eight commits' writes. The burst outruns it. The point seals a
dozen times and merges between, every publish restarts the builder
over the whole overlay, and a commit the builder has posted to files
its batch: 63,000 of the burst's 100,000 writes were filed at the
commits, the writes ran 2.7x slower, and the scans still met an empty
table, the last seal having restarted the builder just before them.
The point read 1.16x at a hundred thousand keys and 0.92x at three
hundred thousand, and in the mixes the builder restarted at each of
F's seals read ycsb-E at 0.84x-0.87x and F at 0.80x-0.97x. It is off,
`supdb-aheadpub` prices it, and a test holds the mechanism.

What the point measures is now clear enough to state as a limitation
rather than a bug. A burst that rewrites the store seals and merges as
it goes and ends with several pieces standing; the scans that follow
immediately walk blocks with four to eight sources each, and either
they build the forms, at 13-20 µs a block, or something built them
during the burst and the next publish dropped them. Only a form that
survives a merge -- one keyed to content rather than to a partition's
block -- or a cheaper build changes it, and the first is not the shape
of this cache.

#### The threaded scan mix at ten thousand keys

Starting on ycsb-E at the small rungs found the target had moved. In
one process against LMDB at ten thousand keys, eleven pairs: E at
0.96x within noise, the single-thread scan at 1.10x (p=0.012), the lag
points at 1.19x-1.27x, and the one loss the threaded scans after the
mixes, 0.54x on two threads and 0.46x on four (11/11). Those go
through handles the caller made, a hundred scans of a hundred entries
each per thread, over the store the mixes leave. A probe in that shape
with every scan timed said where: a handle's first scan cost 120-150
µs against 1.7 µs for every scan after, and every scan bumped shared
counters that four cores contended for.

The first scan was three things. The handle read the whole write log,
two thousand entries, into its list of keys created since a snapshot
it did not yet have: 20 µs. It then built its own snapshot of the
unsealed keys, 70 µs, because the published one was the empty
snapshot the drained scan pass had built: the writer keeps a snapshot
until the keys since outnumber it and files them by block meanwhile,
which is right for the writer, whose tables carry those keys, and
useless to a handle. And it made its block table and walked two cold
blocks, 25 µs. Now a claim brings the writer's snapshot current and
publishes it, so a handle adopts, and a handle with no snapshot files
nothing from the log. The first scan reads 15-40 µs.

The counters were the slot table's bug a second time. A scan through
a handle bumped seven to thirteen shared words -- the scan counts, the
regime's signal, the canonical tries and hits, two per form taken --
and four handles on four cores bumping the same lines read the mix at
0.46x; with the counters switched off through a temporary flag, 0.91x.
Each handle's statistics now live on its slot's own line, summed by the
accessors, and a handle signals the regime once per commit rather than
once per scan, since the writer compares the count and never reads it.
Against LMDB after both, the mix reads 0.89x on two threads (8/11, ns)
and 0.78x on four (10/11, p=0.012), from 0.54x and 0.46x, with E, the
single-thread scans and the lag points where they were. What remains
is the first scan's cold blocks and the table it makes, some 20 µs
against a steady scan of 1.6.

#### Publishing for readers that exist

The copy the carry made visible is the publish's: a canonical form is
an `Arc` clone of the writer's own, so the next patch of that block
copies it first, and the suite's mixes and its lag sweep hold no
handle, so every publish there was for nobody. The forms are now
published only while a handle the caller made is live, and all at once
when one is claimed: the claim files the backlog and publishes every
block left dirty, at the log's length when nothing is staged and at
the next commit otherwise. The table's position and completeness move
only when it publishes, which is what keeps a handle at an older
commit finding the forms of that commit; `forms_to_writer` publishes
regardless, since the writer is then a taker too. The builder ahead
now skips the blocks the writer holds, whether published or not, since
it used to learn that only from the published table. Against the arm
that publishes at every maintained commit, `supdb-pubalways`, eleven
pairs at a hundred thousand keys read the tenth-unmerged lag point at
1.11x (10/11, p=0.012) and every mix within noise: the probe's F gain
of a tenth to a fifth with publishing switched off did not survive
pairing, and what remains is the first scan after a burst filing
three thousand writes without copying a thousand blocks first.

Writing the test for it found a fault older than any of this. The
writer's first block table over a state was made by its own scan until
the forms were maintained at commit; made by the commit's fill
instead, because a handle's scan and not the writer's had asked for
the maintenance, the flag that files writes into the tables stayed
off. `maintain_forms` sets it before reading the log, and the log
read resets it when the generation has moved, and the fill made the
tables without setting it again, so from then to the next generation
every write went unfiled: the writer's own scan answered a key short
of the value it had just written while the point read beside it
answered it, and the forms it published carried the same hole. The
flag is set where a table is made now, and a test holds a handle's
scan, the commit's fill, a staged write and the scans after it to the
model.

#### The first scan over a store just flushed

The suite's scan pass is a thousand scans at a hundred thousand keys
and three thousand at three hundred thousand, over the store the load
left: partitioned, nothing unsealed, every read before it a point read.
Timed one scan at a time, the pass's first scan cost 200 µs at a
hundred thousand keys and 700 at three hundred thousand against 2 µs
for every scan after, a tenth of the pass at both rungs, and a fresh
open of the same store paid the same, so it was the store's and not
the process's. Page faults were not it: the first scan took two. It
was three things, each once per state.

The largest was a walk over the partition's block boundaries for a
source with no keys. A block table maps where each source's positions
fall against the partition's blocks -- every level-0 piece meeting the
range, and the snapshot of unsealed keys -- by walking the source and
reading each block's first key once. The read came before the check
that the source had any position left, so a snapshot of no keys, which
is what every scan over a flushed store starts from, cost a cold line
per block: 4,500 of them at three hundred thousand keys, 500 µs, and
the builder ahead read the same lines again on its own thread to find
every block clean. The walk reads a boundary only while the source has
positions below it, which is the same answer for every source and no
read for an empty one.

The ordered index's top level, every sixty-fourth head, was built at
the first seek: one line in eight of the index file, 60 µs at three
hundred thousand keys. It is built at open, where the seal or the merge
that made the segment pays it off every read's path.

And the builder ahead started at the first scan whatever there was to
build: a thread spawned to find every block clean and exit, 60 µs on
this machine. With no piece and no unsealed key the scan records the
state's one run as spent and spawns nothing. Spent, and not skipped:
the first version left the record empty for a later scan to start the
builder, and the lag sweep's first point with unsealed keys, which had
never had a builder beside its scans, got one and read 0.71x of what it
had read.

The first scan reads 40-65 µs at a hundred thousand keys and 100 at
three hundred thousand now, most of it the table's arrays, one per
block, and the pass is two to three percent of that scan rather than a
tenth. Against LMDB in one process, six pairs a run, the binary before
read 0.88x of LMDB's single-thread scan pass at a hundred thousand keys
and 0.65x at three hundred thousand, the binary after 1.08x and 0.94x;
two runs of the same binary on this machine differ by a fifth, so the
timed first scan is the measurement and the pairs are what it predicts.

#### A handle's first scan after the mixes

The threaded scan mix claims a handle per thread over the store the
mixes leave -- a partition or two, a piece or two, and the unsealed
keys -- and each thread scans a hundred entries `size / 100` times.
Timed one scan at a time through the probe in that shape, a handle's
first scan cost 30 µs at ten thousand keys, 270-310 at a hundred
thousand and 1.2 ms at three hundred thousand, against 1.6-2.3 µs for
every scan after: an eighth of each thread's pass at every rung. It
was the block table's two walks, and the table made them for every
handle and for the writer at every state a seal or a merge published.

A table maps where each source's positions fall against the
partition's block boundaries: every level-0 piece meeting the range,
and the snapshot's main run. Each walk read the boundary key from the
partition's record, a cold line per block, and the piece's keys from
the piece's records, a record read per key. At three hundred thousand
keys the piece walk was 640 µs and the snapshot walk 290. Three things
changed. The boundary key comes from the ordered index's top level
where a head is a whole key, which for keys of one length no longer
than the common prefix plus eight bytes it is, so a boundary costs no
record; a piece is walked along its own index's heads, one sequential
word per key, with the prefix's verdict and the boundary's head taken
once per boundary; and the bounds are cached where they belong. A
piece's bounds against a partition are a function of two sealed files,
so they live in the piece, keyed by the partition's blob id as its
ranks are, and a snapshot's bounds against a partition are a function
of its main run, which a filing never changes, so they live in the
snapshot and follow the copy a filing makes of a shared one. The
writer's table takes them at its first scan over a state; every handle
that adopts the snapshot finds them, and a new piece costs the walk
once and not once per table.

A handle's first scan reads 11-17 µs at ten thousand keys, 17-19 at a
hundred thousand and 60 at three hundred thousand when the bounds are
there, and the thread that finds them missing computes the snapshot's
over a fresh publish: 34, 100 and 320 µs. What is left is the table's
arrays, one per block, and the faults on a fresh thread's heap. Each
thread's pass in the probe: 165-186 µs at ten thousand keys from
193-203, 1.8-1.9 ms at a hundred thousand from 2.3-2.5, and 6.6-7.1 ms
at three hundred thousand from 7.3-8.8.

#### Address translation over a 46 MB partition

What is left of the scan at three hundred thousand keys grows with the
store: the fixed cost per scan from about 240 ns at ten thousand keys
to 450 at three hundred thousand, the cost per entry from 6 ns to 10,
and LMDB's grows too. A partition there is 46 MB mapped in 4 KB pages,
far past what the TLB reaches, so every distinct page a scan touches is
a page walk, two-dimensional under a hypervisor. This machine has no
hardware counters, so a probe took translation away instead: the same
bytes as the seal wrote them, as one write to a new file, as an
anonymous copy with huge pages, and as an anonymous copy without, each
behind the same slice source, scanned with the same hundred start keys
window by window so drift lands on all alike.

The kernel's side is settled. The page cache sizes a folio by the write
that creates it, at that write's alignment, and a mapping takes a
PMD-sized folio with one page-table entry: the file written in one
write shows 45 MB of `FilePmdMapped` in `smaps`, the seal-written one 4
MB. `MADV_COLLAPSE` on a file mapping is `EINVAL` here (the kernel is
built without collapse for files), and a cold file read back by one
readahead gets small folios, so a file has its huge folios from the
write that made it or not at all. The segment writer writes 2 MB
pieces at 2 MB offsets now, where a `BufWriter` of a megabyte flushed
wherever it filled; every segment a seal, a partition or a merge
rewrites is cached in PMD folios. The one that is not is the direct
segment the ordered load appends to, whose pieces are its commits, each
flushed and synced: a folio larger than the commits into it is the WAL
recycler's write amplification, so that segment stays as it is, and it
is the one the suite's scan pass reads.

The time is not settled, and cannot be here. Three runs read the huge
folios 12% faster than the seal-written file and both anonymous copies
20% faster, the copy without huge pages as fast as the one with; the
fourth run read every copy 20% slower than the file, and the engine's
own scan over the same store at the blob's speed. The one thing that
changes between runs is where the guest's pages land, and how the host
backs them is not visible from inside. A question of ten percent about
translation or folio size is not answerable on this machine, and a
figure for the writer's change waits for one where it is.

#### Which half of the lag gap, by rung

The build and the walk split by size, and the arms say which is which
without a probe. `supdb` against `supdb-lazyforms` -- the same engine
with the forms built at commit or left to the read -- reads the tenth-
unmerged point at **3.79x** at ten thousand keys and **0.97x** at a
hundred thousand. So prebuilding is the whole story at the small rung
and nothing at all above it: at ten thousand the lag gap is form
construction, and at a hundred thousand and above it is the walk over
the forms, 119 ns an entry against LMDB's 29.

That is the split the next attempt starts from, and the two halves want
opposite things: the small rung wants the build cheaper or skipped, the
large rungs want the sparse walk faster. A change aimed at one should be
priced at both rungs, because today's measurements have the effect
inverting between them three separate times.

#### The lag gap is a build cost, and the two defaults compound

What the lag sweep measures is mostly not scanning. The same store and
the same scans, varying only how many of them, a tenth of a hundred
thousand keys unmerged and a hundred entries a scan:

| scans | ns a scan |
|---|---|
| 1,000 | 4,372 / 4,317 |
| 4,000 | 2,010 / 2,329 |
| 20,000 | 1,913 / 1,892 |
| 80,000 | 1,413 / 1,713 |

The sweep does `size / scan_len`, which is a thousand scans over 1,563
blocks, so most of them meet a block for the first time and build its
form: about 2.9 µs of the 4.3 is construction. That is why the arm that
builds the forms at commit wins the sweep, why capping the seal wins it
(fewer blocks need a form at all), and why three probes written against
this failed to reproduce any of it -- each reused a few thousand start
keys over a hundred thousand scans and amortised every build away.

Which makes the two defaults complementary rather than alternatives.
Before the seal cap, maintaining the forms whoever is reading lost
0.869x at the fully unmerged point and cost 2.18x the forms' bytes at a
hundred thousand keys, and that is why it was left off. With the cap in,
less stays unsealed, the fill is smaller, and the same flip measures
against the arm that keeps the old shape, eleven pairs each:

| | 10k | 100k |
|---|---|---|
| scan-lag, all of it unmerged | **6.03x** | 0.96x (ns) |
| scan-lag, a tenth | **3.79x** | 0.97x (ns) |
| scan-lag, a hundredth | **1.43x** | **1.34x** |
| every mix, the load, the reads | ns | ns |
| the forms' bytes | 9.35x | unchanged |

Starred figures are 11/11 at p=0.001. What is left to pay is memory on a
small store, 162 KB of forms against 1.52 MB at ten thousand keys, and
`scan_cache_bytes` bounds it for a caller who minds. Nothing else on the
ladder moved either way.

### Arrival order

Every durable-load number above comes from a load whose keys ascend, and
piece promotion made that shape special. Loading the same million keys
both ways, interleaved, with LMDB beside each arm: ordered, the pair reads
0.653x, in line with the canonical load. Shuffled, it reads **5.931x**
(284,938 against 48,041 ops/s, p=0.0022): LMDB's durable ingest falls
13.7x when the keys stop arriving in order, because each per-batch fsync
then writes about a thousand dirtied leaf pages, while the engine's falls
1.51x, the cost of merging what promotion cannot route. The prediction was
the opposite ordering -- it priced the engine's merge and not the
B-tree's page writeback. The two numbers are one finding: which engine
wins the matched durable load depends on the arrival order, by a factor
of nine.

### Crash injection

The promises above were held by one-shot tests that tore a file by hand.
The crash-injection test kills the process instead: a child commits
batches of self-describing puts, deletes and transactions under 48 KB
seals, so that seals, promotions, merges and manifest swaps are all in
flight, and aborts -- at a fixed operation, or at the first one that finds
a seal or a merge running, so the windows are reached on purpose rather
than by thread timing. Then the parent does the one thing a process kill
cannot: it tears the live WAL's unsynced tail to a random length. A kill
alone leaves the page cache intact, and `EveryN` would have looked
exactly like `Always`. The parent regenerates the child's stream from
its seed and asks which prefix of the commit order the reopened store
equals.

At full scale, 120 crashes: 82 with a seal in flight, 72 with a merge, 72
with partitions, 76 with bytes torn. Every directory opened; under
`Always` no acknowledged batch was lost (the statement the durable-load
figure rests on); every recovered state was an exact prefix, with `count`
and `scan` agreeing; nothing was invented; under `EveryN(8)` the most
lost was six batches against a bound of seven. The test's check on itself
is a mode that lets the tear reach below the synced mark: an acknowledged
batch is then lost in three trials of four, which is how the parent is
known to be able to see a lost batch.

It found one thing before it held. A seal rotates to a fresh WAL whose
eight-byte header is written and not synced until the first commit into
it, and replay refused a WAL shorter than its magic -- so a power loss in
that window left a store that would not open. A prefix of the magic is
now an empty WAL, `open` truncates and rewrites it, and the seal fsyncs
the directory as soon as the new WAL exists, since commits into it are
acknowledged from then on and an fdatasync of a file does not promise
the entry that names it. One directory barrier per seal, off the
per-commit path.

## Open, and deliberately so

- ~~Filter choice~~ — **answered by measurement**: fences via
  range-partitioned compaction for sealed levels, per-segment Blooms for
  the overlapping tail; global routing structures rejected by measurement
  twice.
- **The incremental merge** — measured before it was built, and the
  measurement changed what it is. The range merge already rewrites only
  ranges holding pieces; the flush now does too (`flush_ranges`; it is
  safe, and reads do not notice). Neither buys bytes: with uniform keys
  every range holds pieces, and with ordered keys every seal lands in the
  last partition, which is rewritten and re-split each round -- ordered
  keys wrote *more* device bytes than random ones at 16 MB seals. What
  makes ordered ingest incremental is promotion, not selection, and it is
  built: a piece whose keys all lie above a partition's last key becomes a
  partition by rename -- hard links, one manifest write, the old names
  unlinked -- with nothing rewritten. On a log's key order at 16 MB seals,
  device bytes **0.453x**, ingest-to-routed **1.688x** (561,195 against
  332,397 ops/s) with the merge phase at zero, reads unchanged; on uniform
  keys nothing qualifies and nothing changes. The canonical run's shape is
  uniform, so its figure does not move; a log's does.
- ~~Seal size at scale~~ — **answered by measurement at thirty million
  keys**. A seal of a fixed size drops a slice into every range, and the
  slice shrinks as ranges multiply: 3 MB a range at three million keys,
  0.3 MB at thirty million, where `l0_trigger` of them made 1.2 MB and
  merging them rewrote a 64 MB partition, fifty times the bytes, while
  twenty more seals landed on the ranges the job covered. Level-0 reached
  24 pieces a range, every read walked 2,496 pieces' fences and probed 24
  Blooms, and every scan merged 24 pieces: on the suite's mixes D fell to
  35k ops/s and E to 14k
  against LMDB's 489k and 354k. `Options::seal_grows` sizes the seal at a
  sixteenth of the partitions' bytes past the `seal_bytes` floor -- the
  divisor is four times the trigger, so a merge takes in at least a
  quarter of what it rewrites -- and at thirty million keys level-0 stayed
  under three pieces a range: D 533k, E 334k, A 352k against LMDB's 50k,
  F 282k against 51k, the load at 0.96x. At and below three million the
  floor is the larger number and nothing changes.
- ~~Level-0 pieces on the read path~~ — **answered by measurement at thirty
  million keys**. With the seal grown, D still read at 0.8x of LMDB, and
  the same store loaded under a hand-set 320 MiB seal, 11 partitions
  instead of 42, read D a third faster; the partition count looked like
  the cost. It was not. A read found its partition by a fence search and
  then walked every level-0 piece in the store, two fence compares each,
  to find the ones over its range: 84 pieces over 42 ranges, 22 over 11,
  2,496 with the seal fixed. The pieces sort by lower fence and then by
  name, so a range's pieces are one consecutive run, oldest first, and
  two binary searches bound it (`Db::pieces_over`) whenever every piece
  is aligned to a partition, which the segment sort records; a piece that
  spans ranges, sealed during the first partitioning, puts the walk back.
  Measured on one machine, one binary with the routing switched by
  environment, two rounds interleaved and one reversed: D **295k-303k to
  415k-480k ops/s** against LMDB's 344k-354k, F 216k-220k to 233k-238k,
  the other mixes and the load inside their round-to-round spread. A
  flag for whether any segment holds a tombstone, in place of the read's
  walk over every segment's, was measured beside it and moved nothing.
- ~~The scan snapshot on the merge path~~ — **answered by measurement at
  thirty million keys**. Without the block cache, the scan's snapshot of
  the unsealed keys was rebuilt at the first scan after any write, and
  after the mixes before it had left a million keys unsealed, ycsb-E's
  2,500 write batches made 2,500 rebuilds: 336 s of a 427 s pass, a scan
  at 89 µs. The snapshot now outlives writes on that path as it did with
  the cache: the keys created since the build are filed into sorted side
  runs at each scan, a cursor merges the runs and folds a key held by the
  main run and a side one, a rehash renumbers the runs' slots with the
  rest, and the snapshot is rebuilt once the filed keys reach an eighth
  of it. Same machine, one binary switched by environment, two rounds
  interleaved: E **11.7k to 59k-61k ops/s**, the scan at 17 µs. What is
  left in those 17 µs is the merge over the partition, the pieces and the
  memtables, which is what the block cache replaces at 4 µs a scan; the
  merge path's scan opens a cursor into each piece over its range now
  rather than every piece in the store, measured beside this at a fifth
  of the gain.
- ~~Readahead out-of-core~~ — **answered**. Once the file outgrows the
  page cache, the kernel's default readahead is the whole cliff: cold
  point reads run 75.8x and 78.9x faster under `MADV_RANDOM`, at 1.0x read
  amplification against 1800x, the default having fetched 157 GB off the
  device to serve 89 MB anybody asked for. It is a trade rather than a
  win, because the ordered scan wants exactly the pages a point read does
  not and pays 2.3x to 2.5x for losing them. `Options::read_advice`
  carries that trade: `ReadAdvice::Random` takes one side of it for the
  life of the store, `ReadAdvice::Normal` the other.
  What removes the choice is that a store knows which of the two it is
  doing: `read_all` and `scan` are different calls, so the advice can
  follow the workload rather than be picked once. Doing that beats a fixed
  `MADV_RANDOM` by 7.650x and 7.941x on a phased workload and ties an
  oracle switching at true phase boundaries — and costs nothing on a
  workload that never scans. The threshold is one scan, which is no
  counter at all, because hysteresis measured as the cost rather than the
  protection: two consecutive scans as the trigger falls to 33.2% and
  30.8% of the better fixed advice on a workload with no phases, where one
  scan is 1.5x it. A `madvise` is microseconds and a cold scan in the
  wrong mode is milliseconds, so the policy is right to act on the first
  call rather than wait for a second. It is `ReadAdvice::Adaptive`, it
  takes no threshold, because a knob whose only good value is known is a
  knob nobody should have, and it is the default. What lets it be the
  default is the measurement over a `Db` of several segments, where a
  switch costs one `madvise` each: it is 4.3-4.4x the kernel's default and
  6.5-6.6x a fixed `MADV_RANDOM`, and on a store that fits in memory --
  where it can win nothing and can only cost -- it is a tie. The canonical
  comparison agrees, which is what a default has to be measured against
  rather than predicted from.

- **Partitioned compaction policy** — built and measured. The tail bound
  is a real dial: T8 sends 0.898x of T4's device bytes and scans 0.910x as
  fast. What the same run also convicted is the merge itself — it rewrites
  the whole live set every time, so it costs 21.6% of durable load
  throughput and its device ratio grows with the store. **An incremental
  merge — rewriting only the partitions the tail overlaps — is the open
  work**, and compaction's share of the durable load is where it gets
  measured. The 1M-key run raises its priority twice over: compaction now
  costs 42% of the durable load at 1M keys against 21.6% at 300k (the
  whole-live-set rewrite growing with the store, as the 300k run warned),
  and because the merge cannot keep up with seals, the tail bound does not
  control the tail. An incremental merge is what would make the policy
  knob real.
- **What the ordered axis actually costs.** The prediction was that
  partitioning recovers scans 12x; measured, it is 1.367x. The axis was
  losing to the scan implementation, not to the fan: candidate enumeration
  through the posting-counting walk, and a hash probe per key per source.
  Both are fixed and both arms gained. The ordered scan needs re-measuring
  against LMDB before anyone knows where the ordered axis stands.
- **The block cache** (`Options::scan_block_cache`, on; `scan_cache_bytes`,
  none) — built and measured, an arm of the suite, not shipped. The
  ordered scan over unsealed keys merged the memtable's chained values
  with the partition at about 80 ns a key, and ycsb-E scans the keys the
  mixes before it updated: four times behind LMDB at 300k keys, where the
  same scans over the compacted store tie it. The cache keeps, for each
  partition block a scan crosses, the block as it would read compacted
  -- clean; the partition's walk with a few resolved keys slipped in at
  their cuts; a dense copy from sixteen keys up, since copying every
  touched block held more and ran slower and a walk cut every few keys
  costs more than the copy it saves; or, past four blocks' worth of keys
  above it, no copy at all but a merge seeked into each source -- built
  on first touch, dropped by a write to a key it owns and rebuilt at the
  next scan, with the level-0 pieces aligned to a partition ranked
  against it, keyed to that partition, so their keys cut the walk
  without a compare: a piece sealed while a merge of its range ran is
  kept across the merge's publish under a new partition, and its ranks
  against the old one were behind by every key the merge folded in
  below, which the checked profile's tests reached as an assertion in
  about one run in twenty until the ranks carried the partition's
  identity and were taken again at the merge's publish. On the
  suite's mixes E runs at three to four times the arm without it. Memory
  is what the pass touched: E's starts reach nearly every block, so
  unbounded the cache holds about a tenth of the store at three million
  keys and at thirty, and a budget below that sheds blocks E touches
  again -- at thirty million, a sixteenth of the store cost E a quarter
  of its rate and a sixty-fourth three fifths, one machine, rounds
  interleaved. So the budget's default is none: the bound is the memory
  the caller has, which only the caller knows. Where E's time goes at
  thirty million, timed apart by form on the store A, F and D leave --
  82 pieces over 41 partitions, 90k blocks clean, 275k sparse, 42k
  copies -- against the same store flushed before E: a scan of a hundred
  is 4.4 µs on its first pass and 3.4 on its second there, 2.3 and 2.2
  flushed, where it beats LMDB by a fifth. The seek is 0.6 either way.
  The dirty store's first pass builds for 0.8 µs a scan, three quarters
  of it copies at 68 µs each, and walks for 2.1 against the clean walk's
  1.5, most of that in copies because the blocks E scans most are the
  ones A and F updated most; the copy walks at a clean walk's price, and
  the sparse walk cost 2.5x that, 2.9 gap walks at 0.44 µs and 2.4
  emissions at 0.5, each a read into a piece's cold page, which is why
  the sparse form now keeps its values: five rounds interleaved, the
  first pass unchanged at 253k a median either way, since the read
  moves from the first walk to the build, and the second 300k-318k
  against 285k-305k, at 290 MB of cache against 202. A copy's build at
  the sixteen-key threshold is 42 µs, 13 of them the three buffers'
  allocation and 29 the merged walk, and eighteen thousand of them in a
  first pass are a twentieth of it; the sparse builds about as much
  again, so the builds together are a tenth of the dirty store's first
  pass and the rest of its excess over the second is first touch. Where
  that leaves E at thirty million on the store A, F and D leave, two
  rounds interleaved with LMDB: the first pass 261k–266k against
  321k–331k, 0.79x to 0.83x; the second 316k–319k against 324k–329k,
  0.96x to 0.98x. With the write path settling in place and the cache on
  by default, the same pair again: the first pass 360k–380k against
  398k–441k, 0.82x to 0.95x, the second 430k–443k against 401k–421k,
  1.02x to 1.11x, and the ordered load ahead at 704k–732k against
  599k–617k. The same store flushed reads a fifth ahead of LMDB on
  both passes, so what remains is the overlay, which LMDB never has
  because it patches its pages at write time. Pre-faulting the mappings at
  open gained 2-3% on the first pass and nothing on the second: the
  build's cold reads are not page faults. A second level over the
  seek's samples, every sixty-fourth of them, was a tie the same way:
  two rounds at thirty million timed by step, the seek 0.38–0.49 µs a
  scan without it and 0.42–0.45 with, inside the spread the unchanged
  walks showed between runs, so the first level's 90 KB a partition
  stays cached through a pass and the seek's rest is the heads' cold
  lines. Rebuilding a sparse block as a copy once eight scans had walked
  it lost at every size, two rounds each: at thirty million 44k of the
  298k sparse blocks were promoted for a fifth fewer sparse walks and
  two and a half times the copy builds, E's first pass 318k–334k
  against 331k–336k, and the cache 625 MB against 290; at three million
  357k–370k against 382k–416k. The blocks E walks most are copies from
  their first build, F having updated them densest, and a sparse block
  walked eight times is walked seven more on average, under the
  eighteen a copy's build costs in the walks it saves. What a copy's
  walk paid was not the entries it emitted but the lines it waited for:
  after a pass over hundreds of megabytes of blocks a copy's three
  buffers are cold, and the lower bound over its entries, a binary
  search whose every compare reads a key, was six dependent misses into
  two of them before a byte was emitted. The walk now issues a fetch
  for the entries, the keys and the first kilobyte of the values before
  the search, and for the next block's while this one walks when the
  scan will cross into it, so the misses overlap and cost one. Two
  rounds interleaved, timed by step: at thirty million the copy walk
  1.06–1.12 µs a scan to 0.37–0.45, E's first pass 264k–280k to
  306k–350k and its second 295k–313k to 385k–406k; at three million
  the walk 0.90 to 0.32–0.40, E 297k–300k to 355k–422k and 333k–335k
  to 410k–481k; at three hundred thousand, where the buffers are warm,
  the walk 0.29–0.34 to 0.22–0.24, the first pass level at 457k–528k
  against 502k–509k and the second 638k–655k to 684k–696k. The same
  for the rest of what a scan waits on, timed the same way: a sparse
  block's buffers before the lower bound over its deltas; the
  partition's records a sparse or clean walk streams, from the
  directory's word for the first rank to its word for the last, capped
  at 4 KB, so the stream starts with its lines in flight instead of
  ramping the hardware prefetcher from a miss; and the seek's stride of
  heads, eight lines the search probed three of one after another. Two
  rounds: at thirty million the sparse walk 0.42–0.51 µs a scan to
  0.16–0.17, the seek 0.41–0.48 to 0.35–0.38, E's first pass 357k–381k
  to 392k–415k and its second 382k–414k to 461k–470k; at three million
  the sparse walk 0.45 to 0.18, the seek 0.25–0.31 to 0.21–0.23, E
  427k–432k to 484k–496k and 480k–512k to 561k–574k. Where that
  leaves E against LMDB with both, two rounds interleaved on the store
  A, F and D leave: at thirty million the first pass 454k–460k against
  404k–415k, 1.09x to 1.14x, the second 539k–554k against 418k–423k,
  1.27x to 1.33x; at three million 565k against 498k–500k and
  645k–675k against 500k–514k. The overlay LMDB never has now costs
  less than the walk it saves, so the standing above is the one before
  this and not the one to quote. At a hundred thousand keys and three
  hundred, where the suite's rows still trail LMDB, the scan timed by
  step is a third builds on its first pass, the pass a row measures,
  and the sparse build's cost was the cut search: a gallop and a binary
  search of parsed records for each overlay key. It runs over the
  ordered index's heads for the block now, eight lines hot after the
  first build, with one key read to say whether the cut is the key: the
  sparse build 0.22–0.25 µs a scan to 0.15–0.18 by the timers, and on
  plain probes over four rounds E's first pass 620k–692k to 659k–700k
  at a hundred thousand and 452k–653k to 630k–649k at three hundred,
  the second pass level. What was left of a build at those sizes was
  the emit: with nothing sealed since the mixes every overlay value is
  in the memtable, and a copy's build spent 15 of its 23 µs emitting 33
  keys, two misses a key into a 5 MB arena, the entry and then the
  chunk at its head. The build now fetches them in two sweeps before
  it emits, every key's entry and then every head chunk with the line
  before it, where the tombstone a put leaves sits, so the misses
  overlap: the emit 5 µs a copy build by the timers, the sparse build
  0.20 to 0.16 µs a scan at three hundred thousand. On plain probes the
  pass moves less than the machine does: 664k–705k to 682k–704k at
  three hundred thousand over three rounds, 552k to 577k–590k and then
  541k–570k to 543k–550k at three million, 425k–498k either way at
  thirty million over five rounds. The write
  path settles in
  place: a write is queued and, at the next scan, spliced into the built
  block it landed in -- the key's run resolved as a build resolves it,
  the partition's values for an equal key, then the pieces' and the
  memtables', a tombstone masking older -- a clean block becoming sparse,
  a sparse or copied block taking the key at its place or its run
  replaced, a deleted key an empty run that counts as the model counts it
  until a merge reclaims it. Before this the block was dropped and
  rebuilt at the next scan that crossed it. A mix that updates and scans
  one hot range of ten thousand keys at three million, rounds of a
  hundred updates then a hundred scans of fifty on the cache an E pass
  built, three rounds interleaved: the scans 5.8–6.7 µs against 2.5–2.9
  patched, the updates level, 4.7–10.1 against 5.0–6.1. Two rules kept
  the patch equal to a rebuild: only a build chooses the wide form, so a
  block whose overlay crosses the wide bound is dropped for the next
  scan to build wide, which the wide-block test found missing; and a
  replaced run's bytes stay until they outweigh the live ones, when the
  block is dropped instead. The builder ahead of the reader
  (`scan_cache_ahead`, on) is a reader handle on a thread of its own,
  started by the writer's first scan over a published state: it pins the
  state as of the last commit -- the watermark, the write log's length
  and the entry count the memtable records at each commit -- builds its
  own snapshot of the unsealed keys and every block a piece or one of
  them overlays, sparse forms included, and sends each form back as it
  is built; the writer installs them at its scans while the state is the
  one they were built over, and splices in the keys logged after the
  builder's commit, which it lists as it reads the log, seeded with what
  it had read past the commit already -- a staged batch its own scans
  see. Once per state: an installed form is kept current by the settle
  of every write after, so a second builder over the same state has
  nothing to add.
- **The snapshot is the state's, not the handle's** (`share_snapshot`,
  off until the suite prices it). The snapshot is a sort of every
  unsealed key, and it was each handle's own: at a hundred thousand keys
  each updated once, four fresh handles scanning the same store sorted
  the same 63,242 keys four times, 7.1-10.5 ms each on the scan that
  wanted it. With the organiser running it was worse, not better -- the
  organiser starts a builder every few thousand commits and each one
  built the snapshot from scratch on its own thread, fifteen builds and
  37 ms of a core over one burst of writes, and the reading thread then
  built a sixteenth for itself. A handle now offers the snapshot it
  built to the state, behind one pointer that only moves forward, and
  the handles after it clone it rather than sort again: four handles,
  one build, and the first scan of every handle after the first goes
  from 7.1-10.5 ms to 1.09-1.37 ms, what is left being its own block
  tables. The builder ahead publishes too, which is the point of it --
  that build is on a core the reads are not using. What a state gives up
  when it is replaced is freed past every pinned reader, as the
  canonical forms beside it are, since a handle can be between loading
  the pointer and cloning it; the sweep runs wherever one is retired
  rather than only at the writer's publish, because a store between
  seals never reaches that and each retired snapshot holds the key bytes
  of everything unsealed.
- **A snapshot is carried forward, not sorted again.** A committed batch
  cannot change, so its order is settled once: the keys written since a
  run was built are sorted on their own and merged into it, which is
  linear in the run where a build is a radix pass and a sort of every
  unsealed key with a random touch on each tie. Over one burst of writes
  at a hundred thousand keys, the organiser went from fifteen builds to
  three builds and fifteen merges, and the merges cost 0.13 ms at 1,976
  keys rising to 2.3 ms at 61,377 against a build's 9.5 ms at 63,242.
  The reader's own first scan is the same decision: the state's run
  stopped 1,480 keys short of current, and taking that as it stands
  would leave those in the added list while sorting afresh throws away a
  run that is nearly all of the answer -- it merges instead, 3.6 ms
  against 9.5. A handle carries forward whichever run is longer, its own
  or the state's, and publishes what it ends with. First contact over a
  fully unmerged store goes from 21.3 to 16.1 us a scan, six rounds
  interleaved, and the round after it from 1.76 to 1.64. What a merge
  still pays that it need not is the arena: the run's key bytes are
  copied because the published run is immutable, so a merge is linear in
  the run rather than in the batch. Runs kept side by side and folded
  rarely -- what `side` and `fresh` already are on the merge path --
  would drop that, and are what to do next.
- **A published snapshot stops where its builder stopped, and the
  adopting handle must key its added list to that** and not to the
  memtable's end. The slots past it stay in the handle's list, where
  each is carried into the block it overlays; keying them to the end
  instead loses exactly the newest keys, which no scan of a store
  written and read in one go would show, and which a handle's own
  snapshot could never suffer because it is built at the end by
  construction. How far behind a published snapshot may be and still be
  worth adopting is its own quantity (`snapshot_adopt_behind`, 0): at
  zero a handle takes only a snapshot that is current, which is the
  whole of the saving above and buys no list at all, and the looser
  settings are the arm for a store written between the reads. Its first version built from the segments alone with
  an empty memtable, because the memtable was one thread's; that version
  was a tie at thirty million, where E starts on a million and a half
  unsealed keys and splicing those into a form costs about what building
  it costs. This one builds from the memtable, and what it buys is
  bounded by what a build costs the scan that would have done it, less
  what a thread beside the scans costs them: at three million keys the
  builder sends 37k forms in 140–160 ms, the scans have 30–32k of them
  installed by the sixth percentile of the pass, and E prices 558k–590k
  against 537k–572k without it, two rounds interleaved, the scan 1.37–
  1.47 µs against 1.44–1.55; at 300k, five rounds, 598k–665k against
  622k–670k, a tie, since the first pass there costs its cold cache and
  not its builds: 3.9k forms in 14 ms, 3.1k installed by the eighth
  percentile, and the pass moved by nothing; at thirty million, two
  rounds, 438k–463k against 401k–434k, the scan 1.84–1.89 µs against
  1.90–1.95, with the write mixes before it level. Below a hundred
  thousand keys the builder is a loss, and a large one: its run beside a
  pass of two milliseconds costs the scans more than the builds it
  saves, 310k–351k against 598k–699k at ten thousand keys and 588k–635k
  against 529k–772k at thirty thousand, six rounds interleaved, where at
  a hundred thousand it reads 702k–773k against 682k–758k; so no builder
  starts on partitions of fewer than `scan_cache_ahead_min_blocks`, a
  thousand blocks, and with that gate the small rungs read level with
  the arm without it. Two versions between were measured and thrown
  out. One restarted the builder at a commit once
  the memtable had grown by an eighth, and once a run's forms were all
  installed restarted it at every commit: at three million it ran three
  times beside E's scans, every form of the second and third run dropped
  for a slot already filled, and E priced 397k–427k against 498k–517k;
  at 300k, restarting from zero, 180k–305k against 490k–655k. A thread
  sweeping the store beside the scan thread costs the scans a quarter
  while it runs, so it runs once. The other read each piece's ranks
  through a read-write lock at every block build, twenty-two pieces a
  block at three million, so the builder and the scans bounced the lock
  words between their cores; the lock is taken once per table now, which
  clones the shared vector, and a build reads it lock-free. The arm
  without the builder priced 498k–517k on E at three million in the run
  with the lock and 537k–572k in the run after it, nothing else in that
  arm changed, and that is not a comparison the suite would accept: the
  lock did not stay on the count alone. With the builder on, a scan at
  300k keys was decomposed with cycle counters, each pair of which cost
  about thirty nanoseconds on this hypervisor, so the counters resolved
  the hundreds and not the tens: of about 1.2 µs, the walk of fifty keys
  took some 400, the block prefetch 180 that paid for itself twice over
  in the walk, the seek 180, the install of the builder's forms 110, the
  preamble 120, and the builds the builder had not reached 70. Four
  things came off. The install compared each form's block bounds
  against the keys written since through the memtable's arena, twenty-
  two cold lines a form, and read two index records for the bounds of
  every block though every key a mix inserts past the end falls in the
  last one; the keys' bytes are copied as they are filed, a block learns
  by one compare against the partition's last key that none fall in it,
  and the builder sends its forms sixty-four to a message with their
  sizes, so the install touches nothing another core wrote, and raises a
  flag the scan reads before it asks the channel: 110 ns a scan to 55. A
  scan settled writes and checked its snapshot's staleness at every
  call, four cell borrows and a length, when the log had not moved since
  the last; it asks the log first. And the seek read the partition's
  record at the rank to learn whether the key there was the query, which
  the ordered index's heads already know when every key is one length,
  and searched its top level with a branch a step, half of them
  mispredicted; the index answers the tie, and the top search selects
  instead of branching. Six rounds interleaved against the head before,
  every mix: E 627k–732k against 642k–716k at 300k, level within the
  spread at 100k, and 565k–636k against 541k–546k at three million in
  two rounds. One more cost sat outside the counters, on the first scan
  of a store: the snapshot build's radix sort zeroed two histograms of
  65,536 words for every call, over a memtable the load had left empty,
  and the faults on those fresh pages were 400 µs at ten thousand keys
  and at a hundred thousand, three quarters of the suite's hundred-scan
  pass at the smallest rung. The counters are 256 words on the stack
  now, a byte a pass over the bytes the largest key offset has, the
  first scan at ten thousand keys costs 24–40 µs, the mapping's first
  touches, and the build over 428k unsealed keys is unchanged at 10–11
  ms. The walk itself then: callgrind put the suite's scan pass at 145
  instructions an entry, 78 of them in `parse_record`, which resolves a
  record's regions one checked step at a time for any shape. The common
  shape, one inline fixed extent, is read in place now, a bounds check
  per region and no `Option` chain, at 69 an entry; the probe's warm
  scan pass went from 78–84M entries a second to 99–116M at ten
  thousand keys, 62–68M to 70–83M at a hundred thousand and 37–44M to
  45–50M at three hundred thousand, and E through the probe rose by a
  sixth at a hundred thousand. At a hundred thousand keys the pass the
  suite times is colder than any of those: the store is sixteen
  megabytes, past this machine's private caches, the read passes before
  it leave the walk slower than a fresh mapping's (5,400 cycles a scan
  against 4,200–4,600, and 3,400 warm), and a software prefetch of the
  span, of 4 KB or of a line per page, measured neutral to worse; that
  pass is memory-bound, and bytes an entry are its lever. The cache is
  on by default: its write path settles in place, and in every quick row
  the arm with it prices the same as the arm without on every workload
  but E, where it runs three times the merge -- what it costs is the
  memory, which `scan_cache_bytes` bounds when the caller says so. The
  merge on every scan is the comparison arm, `supdb-nocache` in the
  suite, and the suite matches caches as it matches guarantees: the
  unbounded default against LMDB, whose page cache has no bound but the
  machine's, and `supdb-cache256` against `rocksdb-tuned`'s 256 MB block
  cache. What the unbounded default still lacks to be the page cache's
  equal is giving memory back under pressure, which the page cache does
  page by page and a heap cache cannot on its own: either the forms live
  in `MADV_FREE` regions with a validity word the kernel zeroes when it
  reclaims one, so a form that comes back zeroed is rebuilt, or a
  watcher on memory pressure sheds to a target. Neither moves a row,
  since no run here is under pressure; both are on the backlog, with
  the shape to build them in: two targets in place of `scan_cache_bytes`,
  each a byte count or a percentage of the memory available at open
  (`MemAvailable`, or what a cgroup limit set under it leaves, because a
  container's `/proc/meminfo` is the host's), a hard target the cache
  keeps whatever the pressure and a soft one it grows to and gives back
  from under pressure, oldest-touched first, the way the page cache
  reclaims its inactive list rather than all at once. A build sheds to
  the soft target as it sheds to the bound now. The tables are the
  reader thread's (`Cell`s, unshared), so a watcher thread polling
  `/proc/pressure/memory` can only set the request, and the next scan
  sheds a slice of what lies above the hard target while the pressure
  holds -- which is release only in a process that scans, and the
  `MADV_FREE` form, every cached form flat in pages of its own with a
  word in each that the kernel's zeroing clears, is what gives memory
  back from an idle one. The defaults would be a hard target of a tenth
  of available memory and a soft one of half, which on the machine class
  here is inert: the cache after a pass at thirty million is under
  300 MB against fifteen thousand available. The arms would match as
  they match now, RocksDB's block cache gives nothing back under
  pressure so `supdb-cache256` sets both targets to 256 MB, and the page
  cache LMDB reads through has no bound but the machine's and keeps
  nothing under pressure, hard 0 and soft unbounded. The two are built
  together: a soft target ahead of the release is a label on a bound
  that does not release, the ingest arm's mistake in another place.
- **Ordered ingest straight into segments** — built, and the default.
  The load's keys arrive in order and went through a memtable, a WAL
  frame, a seal that sorts the sorted, and a partitioning pass; the
  segment writer for sorted input takes them as they come, with the
  growing segment's own tail as the log (the shape above; the crash
  windows in `CLAUDE.md`). The ceiling was measured first, the same
  bytes through the writer alone, and the path was built twice against
  it. The first form kept the hash memtable for reads and closed the
  segment on the commit thread, and measured *below* the WAL path it
  bypassed at three million keys and at thirty: timed apart, the
  memtable insert was a quarter of the load and the close a tenth, and
  what the path had removed -- the WAL write and the sort -- was less
  than either. The second form fills an ordered memtable, appended in
  key order with no hashing and searched by binary search, and closes
  on the seal thread. Interleaved on one machine, durable and buffered,
  the WAL path (`direct_ingest: false`) beside it and the writer alone
  as the ceiling; LMDB from the same container:

  | keys | direct, durable | WAL path, durable | writer, durable | LMDB, durable | direct, buffered | WAL path, buffered | writer, buffered |
  |---|---|---|---|---|---|---|---|
  | 300k | 540k–673k | 336k–365k | 787k–877k | 643k | 0.98M–1.14M | 452k–510k | 1.30M–1.71M |
  | 3M | 635k–706k | 491k–499k | 762k–852k | 581k | 1.64M–1.65M | 683k–725k | 1.49M–1.62M |
  | 30M | 498k–531k | 403k–431k | 632k–685k | see below | 1.24M–1.27M | 575k–601k | 1.15M–1.23M |

  Durable, 1.2x to 2.0x the WAL path, level with LMDB at 300k and ahead
  of it at three million keys; at thirty million the pair was measured
  in later runs through the suite's own load loop, interleaved: 460k–467k
  against LMDB's 526k–536k in one, 637k–660k against 605k–610k in the
  next, so level, the machine moving both by a fifth between runs --
  the first draft of this entry put an LMDB figure from an earlier run
  in that cell and read it as a lead, which is the cross-run comparison
  this repository's notes warn against. Buffered, 2.0x to 2.5x, and level
  with the writer alone at three million and thirty, whose close runs
  on the thread that writes. What stands between the durable path
  and its ceiling is the fdatasync on a file that grows, which the writer
  alone pays too, and the ordered memtable's copy of every value, which
  the reads want. Detection needs no interface: the store knows its
  greatest key, and a key above it with a value the record holds inline
  goes to the run, while any other write ends the run and goes through
  the WAL. The partition shape is the seal's: the run closes at the seal
  threshold and joins as a piece the promotion rule renames, so a load
  leaves the same partitions either way. The suite does not price the
  arm yet; a `direct_ingest: false` arm beside the default is the row
  that would.
- ~~Segment size~~ — **swept.** 16 and 8 MB seals are ties on ingest at
  1.5x the device bytes; 32 MB seals are an interior optimum, 1.129x at
  identical device bytes -- once the partition size was set apart from the
  seal size, because the first partitioning had been cutting as many
  partitions as the live set held seals, and more partitions read slower
  (the sweep's first run). 32 MB seals over 64 MB partitions is the
  shipping default and reads no differently from 64 MB seals. What the
  sweep priced beyond that is the incremental merge: below 32 MB every
  extra merge round rewrites the live set, and that is what stands between
  this engine and smaller seals.
- **Recycling WAL files** — built and measured, not the default. An
  fdatasync into blocks already allocated and written carries no inode
  change through the journal, and the commit phase falls 19% on ordered
  keys and 6% on uniform with retired WALs renamed back into place; but
  the pool's one-time pre-write pays that back inside a 1M-key load and
  the ingest reads a tie both ways, so a tie it stays until a longer-lived
  shape is measured. The run also found a measurement trap worth more than
  the flag: pre-writing in 1 MB pieces left 1 MB page-cache folios, and
  every 100 KB commit wrote a megabyte back — 2.2x the device bytes, 11.2x
  in isolation. A folio is sized by the write that creates it; pre-write
  in pages.
- **Group commit** — whether concurrent writers share a barrier; matters
  only after P-D.
- **Read and write concurrency** — being built, readers first, in the
  order the structures depend on each other; writers beside each other
  stay on the backlog as the appender question (2 above): a WAL and
  memtable per writer, or a shared barrier (group commit, above). The
  model is LMDB's: one writer, any number of readers, no lock between
  them, and a reader table of slots where each reader handle pins the
  epoch it is reading in, so the writer can free what it replaced once
  no slot holds an older epoch and never waits for a reader. Each slot
  has a cache line of its own, since every read stores to its handle's
  slot: with eight slots to a line, four threads read a partitioned
  store of ten thousand keys at one thread's rate, and at 3.8x it once
  the slots were spaced. The isolation is the reader's to choose, and it falls out of one
  mechanism: a committed watermark on the memtable's value arena. Since
  chunks are appended in time order, the arena's tail at the last commit
  divides every key's chain into an uncommitted prefix and a committed
  rest; a reader that honours the latest watermark reads committed, one
  that pins a watermark and a state reads a snapshot, and one that
  honours none reads dirty, which is what the store's own reads have
  always done and keep doing. The write log a handle settles its cached
  blocks from is read to the same mark: the length it had at the
  watermark's commit, taken before the watermark. Read to its end, a
  handle settled a staged write under the committed watermark and kept
  the old run after the commit, since the log had not moved.

  *The memtable* is the first part, built: nothing in it moves once
  published. Keys and values live in arenas of blocks that are never
  reallocated (a value larger than a block gets a block of its own, and a
  reservation never straddles two) and never zeroed: a byte is read only
  past a write that covered it, and the first version zeroed a fresh
  table's first blocks, eleven megabytes with the slabs, so its first
  write cost 3.7 ms and ycsb-B at ten thousand keys, whose whole pass is
  under a millisecond, priced 3x below its rows; the quick row found it
  where five rounds at 300k could not, and the first write costs 19 µs
  now; the entries are a slab of the same
  kind, numbered in the order they were made, so a number handed to the
  scan snapshot or a block table stays good for the table's life; the
  hash index is a table of entry numbers under the hash's high half, and
  when it fills it is rebuilt whole and published by one pointer store,
  the table it replaces retired at the epoch the publish bumps and freed
  past every pinned reader. A chunk is `[prev: u64][len: u32][value]`,
  the length fixed so the header is one read of twelve bytes all written
  before the head that names them; a head, an index slot and the slab's
  length are each published with a release store after everything they
  name, and read with an acquire. Before this the entries lived in the
  hash table itself and a rehash moved every one, which is why every
  structure that named a slot -- the snapshot, the block tables' lists,
  the wide blocks -- had to be patched from a map the rehash returned;
  that machinery is gone with the move. What the split costs is a line:
  at thirty million keys the slot and the entry behind it are both
  misses where the entry in the slot was one, so the index carries the
  hash's high half beside the number to skip the entries a probe will
  not match, a put probes once for its tombstone and its value where a
  delete then an append probed twice, and every write and point read
  fetches its slot line first and does the store's bookkeeping -- the
  WAL frame, the fences -- while it comes in. Measured single-threaded
  against the head before it, every mix, two rounds interleaved, in
  thousands of ops/s: at thirty million keys the load 729–734 to
  766–780, A 359–387 to 393–409, B 1387–1404 to 1475–1478, C 2580–2796
  to 2672–2732, D 770–835 to 891–932, E 469–490 to 494–501 and its
  second pass 547–556 to 559–575, F 315–329 to 337–338; at three
  million every mix level or ahead by the same margins, D 908–976 to
  1105–1118 the widest. Before the slot prefetch and the one-probe put,
  A at thirty million read 5–8% behind the head, the extra miss on
  every hit; with them it reads ahead.

  *The published state and the reader handle* are the second part,
  built. The segment set, the live memtable and the frozen one are one
  `State` behind one pointer; a seal, a join, a merge, a promotion or a
  freeze builds the next one and swaps it, and the one before is
  retired at the epoch the swap bumps and freed past every pinned
  reader. A segment is immutable once open -- its block table left it
  for the handle, its piece ranks a lock a table takes once, and the
  blob's checksum memo became atomic words and its
  decompression buffers the thread's -- so a `State` is `Send + Sync`,
  which a test asks the compiler. `Db` is now the writer over a
  `Reader`, the handle that carries the read API and the caches of its
  own: the scan snapshot, the block tables, and where it has read to in
  the memtable's write log, which is how a handle learns the keys
  written since its snapshot without the writer keeping a list for it.
  The writer's own handle is one of them, so there is one read path.
  `Db::reader` makes another with a slot in the reader table, `Send`
  and not `Sync`; a read pins the epoch through the slot, takes the
  state once and holds it to the end -- a read that took it twice
  crossed a freeze and walked one memtable's entry down another's
  chains -- and takes its watermark: the memtable's last commit under
  `Latest`, the snapshot's under `Snapshot`, which also holds its state
  through every publish after it, none under `Dirty`. One caveat the
  block cache already had: a key created past the watermark, like a
  deleted key before the merge reclaims it, can appear in a scan with
  no values. The tests hold each level to its word across a staged
  batch, a commit, a seal and a merge, run three reader threads through
  a writer that seals and merges every few hundred puts, and fill the
  reader table to its last slot; the threaded one found an order that
  had held only by name (`CLAUDE.md`, the shapes). Single-threaded
  against the head with the memtable alone, every mix, two rounds
  interleaved, thousands of ops/s: at thirty million the load 742–746
  to 736–760, A 388–402 to 376–391, C 2534–2624 to 2345–2432, E
  464–474 to 462–474 and its second pass 543–550 to 524–527, the rest
  level; at three million level throughout; at three hundred thousand,
  where a first pair read the load and the write mixes a tenth behind,
  five rounds put every mix at 0.98x to 1.07x of the head. Those rounds
  were too few and too short for a point read: ten rounds of the probe's
  C alone, each pass ten times the suite's, put the two commits together
  at 83% of the commit before them at 300k keys and 94% at 100k, about
  thirty-five nanoseconds on a read of two hundred. Two of the three
  costs were taken back. The read hashed the key and fetched the
  memtable's slot line before knowing the table was empty, which every
  read over a store just flushed is; the hash and the prefetch wait for a
  table with entries now. It also took the state through an accessor at
  every step, two loads and a branch each; it takes the state once. And
  a read asked every segment whether it held a tombstone, forty-one
  pointer chases at thirty million keys, in this engine and the one
  before; the state records the answer at publish. Ten rounds after:
  427k–480k against 464k–501k at 300k, 94%, and level at 100k in seven
  rounds of ten with three a third slower that the base did not show.
  The range-read structure written at ingest, as `commit_forms`: the
  writer keeps a canonical form of every overlaid block current at each
  commit, in a table the state carries, a slot per block that the writer
  installs into with one atomic swap; a reader whose watermark is the
  commit the table was maintained at walks those forms and builds
  nothing, and once the table is complete a block with no form is clean.
  The writer patches its own copy and swaps that in, so a reader walking
  a form is never overtaken, and a replaced form is retired against the
  reader epoch and freed past every reader that could hold it. The
  regime is a runtime property and not a build-time one: the forms are
  maintained for a store that is being range-read, which the scan path
  records in the state, so a write-only stretch pays nothing and a
  scanned store pays at its commits. Measured with the probe over the
  store the mixes leave, three rounds alternated, the first pass and the
  steady pass of a thousand scans of a hundred: at a hundred thousand
  keys one thread 6.9 ms and 2.6 to 4.0 and 2.0, four threads 6.5 and
  2.8 to 4.0 and 2.4; at three hundred thousand, one thread 21.1 and 9.7
  to 12.6 and 7.4, four threads 21.0 and 10.8 to 12.9 and 8.2. The
  writer pays for it: E through its own handle, which holds the forms
  and gains nothing, 689k to 662k ops/s at a hundred thousand and 706k
  to 608k at three hundred thousand, and A 456k to 449k and 496k to
  475k. The dense threshold stays the one the cache measured: a copy of
  every overlaid block instead cost E 628k and 501k and held 3.1 MB and
  22.4 MB against 1.3 MB and 3.7 MB, which is what the handles' own
  cache holds. One property shapes what the forms can buy: a form is
  current to a commit, so a reader between commits walks it and a reader
  behind the writer builds its own, and under a writer committing every
  fifty puts the threaded test's readers took none at all. Maintenance
  pays where scans outnumber commits, which is the policy the arm exists
  to price.

  A block held two ways at once, with the read choosing, as
  `promote_entries`: a block keeps a merged copy beside its cheap form
  once reads have taken enough entries from it, every read after that
  walks the copy, and a write drops the copy and halves the count. The
  threshold is the measured crossover between the two walks, about
  twenty-two cycles an entry against a build of three to five thousand,
  so a few hundred entries. It switches as designed and it loses. Over
  the probe's six mixes at a hundred thousand keys, at three hundred
  entries: 144 of 1,537 blocks promoted, 935 block walks over a copy
  against 23,993 over the cheap form, 1.25 MB of copies, E 705k to 649k
  ops/s and the scan pass 42M and 52M to 30M and 26M entries a second,
  with 259 and 525 minor faults where there had been two -- the copies'
  fresh pages. At a thousand entries three blocks promote; at four
  thousand none, and the arm is the default. The lesson is the one
  `CACHE_DENSE` already carries, now from the read side: a second form
  costs the page cache and the fresh pages it faults, not just the
  build, and a threshold set by hand is wrong in both directions at
  once. What the mechanism is for is a policy that sets it from
  feedback, and what it leaves behind is the machinery to do that: the
  choice is per read, the counters say what the reads chose, and the
  writes' drops are the other half of the signal.

  A block cache shared between handles was built and measured, and not
  kept. Three versions: a table under a mutex, one under a read-write
  lock, and one of atomic slots in the state, a slot per block, a form
  installed by compare-and-swap at the log position its handle had
  settled to and taken by a handle at the same position, replaced forms
  retired against the reader epoch and freed by the writer. All three
  answered the model. Timed per block with cycle counters, four reader
  threads over the store A leaves at a hundred thousand keys: a
  handle's own first touch is a build of 3,300–4,600 cycles and a walk
  of 1,800; a shared one is a take of 500–1,300 and a walk of
  3,500–5,100, since the form's buffers were written by another core
  and its refcount is contended, and a build that installs costs 1,400
  more than one that does not. The passes came out even: the build a
  take saves reads the memtable across cores, and the take reads the
  form across cores instead. At three million keys, four threads,
  three passes each, shared 185–203, 125–142 and 111–114 ms against
  176–197, 117–122 and 106–110 for the handles' own. What sharing buys
  is one copy of the forms in memory rather than one per handle, and
  the builder ahead's forms reaching reader handles; neither moved a
  figure, so the handles keep their own. The suite's `scan-mixed`
  workload, threaded scans on the store the mixes leave, is where a
  later attempt would show.
  The six percent left is the pointer to the state and the `Arc` each
  segment sits behind, and it is the price of a state a reader thread
  can hold. The block cache's slots as atomic pointers a
  miss fills by compare-and-swap and a settle patches by copy, so the
  handles share one cache, and a thread count in the suite's matrix
  with LMDB alongside (`bench/DESIGN.md`), follow.
- **What the on-disk size ordering becomes** — segments plus a WAL will
  not beat LMDB on disk; that loss stands and gets re-priced honestly.

## What this does not promise

Multi-reader snapshots (MVCC beyond the single-writer borrow) -- on the
backlog above, promised once built and measured -- or beating LMDB
out-of-core. Transactions it does promise now -- atomic batches,
rollback, read-your-writes -- and deletes that reclaim their bytes. The
guarantee set stays what `Features` can equalize, so every comparison
against another engine remains matched: the durable-commit axis is
equalizable in both directions, and the transactions axis no longer leaves
a residual on the engine's side.
