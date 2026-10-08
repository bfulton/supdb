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
  segment (data blocks + its own flat index) and fsync it. The WAL is not
  touched: the seal ends at the live file's sequence, replay skips what a
  manifest covers, and the log rotates by size, a closed file retiring at
  the next seal's landing. Sealing is off the commit path; a durability point never publishes index
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

LMDB's side of this table, and of every lag and shuffled-load figure
in the rows before 260c09d, is not the store the engine's side read:
the arm never closed its environment, so a pass's second open handed
it the first store back, and its lag sweep scanned the ordered load's
tree after an overwrite of every key rather than a tree its shuffled
load built (`bench/CLAUDE.md`). Rows of one engine on either side of
the fix -- f87b394 and 260c09d, a quick row and a full row each -- put
LMDB's lag scans at 0.6-0.8x of what the old store gave them, at every
depth and every rung from 30k up, while its plain scan moved 0.87-1.01x
and the engine's lag scans held. So every lag ratio against LMDB here
and in the rows before understates the engine's by about half again;
LMDB's shuffled load, an insert into an empty tree now rather than an
overwrite, is 1.2-1.4x faster at 10k and 30k and level from 300k up.

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

The store's size, for the cap and for `seal_grows` both, is its
partitions' key and value bytes -- which every segment records in its
superblock, counted as the memtable counts them -- taken in file bytes at
seven fifths. The rules above were measured against the partitions' file
bytes, which the suite's full records put at 1.37-1.43 times the data
(the hash capacity steps with the key count), so the full record's
thresholds sit within a few percent of where those measurements had them,
and a denser record format no longer seals sooner. `seal_on_file` keeps
the file rule; `supdb-sealfile` prices it.

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

The store's keys are the partitions'. The bound first summed every live
segment, and a level-0 piece sealed from a burst of updates holds keys
a partition holds already, so each seal of the lag sweep's burst grew
the bound: at thirty thousand keys, three pieces in, 2% was 1,040
writes against the 600 meant, a commit of a thousand stopped crossing
it, and the commits after the seal's publish settled every other time.
Whether the burst's last commit was one that settled came down to where
the seal published, which is the seal thread's timing, and when it was
not, the first scan filed the thousand writes: 0.5-0.8 ms of a pass of
0.45. The pass read two shapes, 22-33M entries a second in six reps of
sixteen and 55-85M in the rest, and every count the probe kept was the
same in both until the settle was timed on its own. Taken on the
partitions (`forms_settle_keys_all` is the old sum), sixteen pairs at
thirty thousand keys read the point at 1.17x (14/16, p=0.004) and never
took the slow shape; at ten thousand no piece stands when the bound is
asked and the arms are the same store; at a hundred and three hundred
thousand nothing the sign test sees moved, mixes included.

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

As first built it did not pay, and the loss was the arm's own: "The
carry, priced with its own costs taken out" below has the four costs
and the pricing without them, and it is the default now. What follows
is the first measurement, kept for what it read into a loss that was
not the seal's. `bench ab` over eleven pairs at a hundred thousand
keys read ycsb-F at 0.83x (0/11, p=0.001) and ycsb-E at 0.89x (1/11,
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

#### The steady scan through a handle at the small rungs

With the first scan's setup gone, the threaded scan mix at ten and
thirty thousand keys still read 0.86x-0.93x of LMDB on two threads,
and ycsb-E at ten thousand 0.97x. There a pass is a hundred scans, so
the first scan and the per-scan cost are shares of the same size, and
the per-scan cost is the whole story: 1.6-2.1 µs a scan through a
handle on the store the mixes leave, against 1.28 on the store as
loaded, with LMDB at 1.83 on the same store. Phase timers in the
probe, in the suite's shape, said the pass builds nothing at all -- a
handle walks the forms the writer published at its commits -- and
that the whole surcharge is the walk of a sparse block: 620-890 ns for
about forty entries, 3.4 deltas and 3.5 separate record walks, where
the clean walk of the same entries is 480. The seek is 200 ns a scan
on either store. The record prefetch a block walk issues is worth a
quarter of the pass at three hundred thousand keys (3.2-3.4 µs a scan
with it against 4.2 without, three rounds) and is not measurable at
ten thousand.

Callgrind over one handle's pass, which this machine's noise cannot
resolve by timing, put about twelve thousand instructions in a scan of
a hundred entries: 62% in the record walk at 73 an entry, 13% in the
prefetch bursts, 7% in the scan's own body, 4% in the seek, and the
table's make, once per pass, 6%. The record walk runs at about three
instructions a cycle, so it is instruction-bound, and it is what every
scan quantity walks.

Holding every overlaid block as a merged copy was the obvious lever
and is not one: E, whose Zipfian scans keep their blocks in the first
cache level, read 20% faster with copies, and the handle's uniform pass
did not move -- a copy walked at 515-635 ns a block against the sparse
form's 620-700, since a block is a stream to the prefetcher either
way, and a copy is three of them.

Three changes, each held by pairing the probe's binaries over fifteen
passes with fresh handles and taking the minimum, three rounds:

- The walk divided twice an entry, `elen / records` for the stride and
  once more inside `chunks_exact` for the remainder, a dependent chain
  of two 32-bit divides in a loop of twenty-five cycles an entry, for a
  quantity that is one in the common record. One value is emitted
  without a divide now: 73 instructions an entry to 57, the clean scan
  through a handle 1.18 µs to 0.96, the mixed 1.85 to 1.65, E 5%.
- The snapshot's block bounds are taken at the first build that needs
  them and not when the table is made. A handle over complete
  canonical forms builds nothing, so it never needs them, and the walk
  of the run against every boundary was 16 µs of the first scan at ten
  thousand keys and 47 at thirty, a tenth of the pass, on every fresh
  snapshot; what `clean_throughout` asks of the bounds is their two
  ends, which are two searches.
- The next block's prefetch was issued twice: once as the next block
  when the scan would cross into it, and again as the block when the
  scan did, 147 of the 394 prefetch calls in a hundred scans.

Together the mixed scan on two threads at ten thousand keys reads 1.85
µs to 1.57 in the probe and the pass 189 µs to 160. The suite's figures
are in the pull request.

A second round over the same profile, at 9,240 instructions a scan of
a hundred entries after the first. The record walk's loop compiled to
fifty-six instructions an entry, and the assembly said where nine of
them went: the `count` word read as four bytes and reassembled, and the
directory word's bounds check with the loop's two tests. Read as arrays
of exact size every word is one load; walked through `chunks_exact`
over the exact span the scan may take, the iterator's end is the
loop's one test -- and through `take` on an open span it was not, the
adapter costing what the checks it replaced had cost, which is worth
knowing before reaching for one. Four bounds checks an entry remain,
one of them on the key slice that the extent's check already implies,
and they stay: the check that makes it unnecessary is on arithmetic
that wraps on a 32-bit target with a directory word an untrusted file
supplies. The seek compared the query's first dozen bytes against the
common prefix through `memcmp`, a hundred instructions of call and
dispatch for two word compares, and compares them in place now. The
sparse walk searched its deltas for the scan's cursor on every block,
when every block after the first is walked from its start. A handle
bumped its slot's take counter once per block, an atomic add on a scan
of two or three; it counts once at the scan's end. The prefetch loop's
compare and add per line were twice the prefetch itself, and it steps
four lines.

Together 8,300 instructions a scan, ten percent fewer, and the probe
paired against the first round's head, fifteen passes with fresh
handles and the minimum, three rounds: at ten thousand keys the mixed
scan through a handle 1.04 µs to 0.85 and the clean 0.65 to 0.57, E 12%
faster; at thirty thousand the mixed 1.28 to 1.18, the clean 0.81 to
0.75, E 6%. (The machine had changed under the session between the
rounds, and every figure here is faster than the round before for that
reason; the pairs are what hold.)

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
write that made it or not at all. The segment writer wrote 2 MB
pieces at 2 MB offsets from here (until the write side was priced,
in the section after this one), where a `BufWriter` of a megabyte flushed
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

#### The write piece, priced on the write side

The 2 MB pieces were the reader's: a 2 MB write at a 2 MB offset is
cached in a PMD-sized folio that a mapping takes with one page-table
entry, and what that is worth to a scan was not settled here (the
section before). What it costs the write was never priced, and on this
machine it is the larger number. The guest's balloon reports free
memory to its host: a free 2 MB block is handed back about two seconds
after it frees, and the guest's next touch of it faults on the host for
every 4 KB page, which nothing inside the guest can see. A 2 MB folio
takes a whole free 2 MB block, so a 2 MB write lands in memory the host
has taken back; a smaller folio comes from memory the guest freed
recently and has not reported, while there is some. Written to a new
file three seconds after the last write, 36 MB went at 2.8-4.7 GB/s in
pieces of 64 KB to 1 MB and at 150-250 MB/s in pieces of 2 and 4 MB; the
same 2 MB pieces written again within a second went at 4.5 GB/s, and
slowly again after three. 300 MB went at 140-240 MB/s in every piece,
4 KB included: what the guest has freed and not reported is tens of
megabytes, and past it every page the cache adds is a fault on the host.

The buffered ordered load writes its segment on the commit path with no
sync between pieces, so its pieces were whole 2 MB ones: 17 writes at
three hundred thousand keys took 228 ms of the load, and the kernel's
copy into them was 45% of the load's samples in a fresh process. LMDB
writes the same bytes in writes of about 130 KB. The suite returns a
dropped store's memory between passes, so every pass's load met
reported memory. With the piece a megabyte, the suite at a hundred and
three hundred thousand keys, two runs of each alternated:
`supdb-ingest`'s ordered load went from 0.33-0.44x of `lmdb-nosync`'s to
0.69-0.73x, its batches from 103-123 ms to 49-52 at a hundred thousand
keys and from 355-381 to 148-152 at three hundred thousand, and
`supdb`'s shuffled load from 3.8-4.4x of `lmdb`'s to 4.6-5.6x; nothing
read moved by more than the runs moved.

In one process, each arm's pass beside the other's, eight pairs a size,
2 MB read 0.89x and 0.91x of 1 MB on the buffered ordered load at a
hundred and three hundred thousand keys (1 MB faster in 7 and 6 of 8),
0.97x and 0.68x on the durable shuffled load (7 of 8 each), and 1.07x
the other way on the buffered shuffled load (2 MB faster in 7 and 6 of
8); no read, scan or mix resolved. Passes back to back reuse the memory
the pass before freed, within the two seconds the balloon waits: with
three seconds before each load, the buffered ordered load read 0.72x and
0.54x (1 MB faster in 8 of 8 at both) and the buffered shuffled load
0.65x and 0.68x (7 and 8 of 8). What a 2 MB piece costs is how long the
memory under it has been free, which the suite's separate passes and a
process's first load make long.

A megabyte is the default (`SegmentOptions::write_piece`), the write
path's own size, and `supdb-pmd` and `supdb-ingestpmd` keep 2 MB to
price it.

#### The fully-unmerged point is the build, and the build is the pieces

The sweep's last point reads 0.10x-0.16x of LMDB at every rung, and
the probe in its shape says where. At a hundred thousand keys the point
holds one partition, seven pieces and the last commits unsealed; the
pass's scans read 22-25 µs each and build 850 blocks. The same scans
run again over the forms the pass built read 1.6-2.0 µs, and a third
time 1.5: the walk over a built form costs what a drained scan costs,
and every microsecond above it is the build, about 24 µs a block at
seven or eight pieces, at a hundred thousand keys and at three hundred
thousand alike. Callgrind puts a build at 74,000 instructions for
sixty-four partition records and forty overlay keys held across fifty
piece entries; the same shape with two pieces is 18,000. What grows
with the pieces is everything per overlay key: the piece keys read and
sorted, the record resolved once for the tombstone rule and once for
the values, the memtable's chunk chased, the copy's pushes, and thirty
allocations.

Three tweaks were tried against that and each was a wash by instruction
count, which is the one measure this machine reports exactly. A cursor
over the partition's records in place of a walk re-entered per overlay
gap costs the same per record once it has the walk's own fast path and
more without it; sorting the piece keys through two words of the key
rather than `memcmp` saves the compare and spends it on the order it
builds; and prefetching the pieces' runs and the block's records ahead
of the build changes nothing, because the build is instruction-bound
and not waiting. A rewrite that merges the sources' cursors straight
into the copy, resolving each record once, is the remaining lever on
the build itself, worth two to three times at seven pieces by its
instruction budget and nothing at two, where the current build is
already near that shape.

The pieces are the merge scheduler's. A merge starts when a range holds
`l0_trigger` pieces and absorbs the pieces present when it starts, one
merge at a time; at the point's write rate four seal while one runs,
so the reads meet seven or eight. Joined after the pass, the merge in
flight finishes in 60 ms at a hundred thousand keys and 100 at three
hundred thousand and leaves four or five pieces, over which the same
scans read 12-13 µs. A workload that scans after a burst meets the
merges catching up within a tenth of a second; the sweep's point, by
its definition, does not wait. Bounding the pieces a read meets while
a partition merge runs -- merging pieces into a piece, which is cheap
where a partition rewrite is not -- is the structural answer this point
asks for, and it is a compaction policy and not a read path.

#### Piece merges beside the partition merge: correct, and not enough

The structural answer above was built and measured: `tier_pieces`,
off by default, and the `supdb-tier` arm at three. When a partition's
range holds that many aligned pieces that no partition merge holds as
inputs, a thread of its own merges them into one piece over the same
range, named by a fresh id and the newest input's covered sequence so it
sorts where that input did; for each key the inputs older than its
newest flagged extent are dropped as the partition merge drops them,
and the flag is carried, since the partition below still holds what it
masks. The forms and the writer's tables are carried across its
publish whatever `forms_carry` says: no key or value moved, only the
file some of them sit in, so only the pieces' bounds are walked again.
A test drives a store that merges and one that does not through the
same appends, deletes and seals and holds them to the same answers,
before and after a reopen.

The crash oracle found the ordering hazard in the first version. Level
0 is newer than the level below it, every piece of it, so a partition
merge may take only a prefix of a range's pieces by age. One started
beside a piece merge excluded that merge's inputs and took the piece
sealed after them, folding newer values under older ones: a key's
values came back out of order, and a key whose older values a dropped
tombstone had masked lost them. No partition merge starts while a piece
merge runs now; the other way round is safe, since a partition merge's
inputs are the range's oldest pieces.

What it buys: the sweep's fully-unmerged point holds one or two pieces
instead of seven or eight, the probe's scans there read 21 µs against
22-25, and paired against the arm without it, six pairs each way, the
point reads 1.11x at a hundred thousand keys and 1.13x at three hundred
thousand (0/6, p=0.031); nothing else on the ladder moves. That is a
tenth of the point's cost where the piece count said three or four
times, because the pass's cost is not the pieces alone: the merge's
completion races the write phase, so the same configuration read 13.5
µs a scan in one run and 24 in the next, and the build of a block over
one piece and the memtable is still ten microseconds.

What it costs, and why it is off: a merged piece counts one toward
`l0_trigger`, so a range that keeps folding three pieces into one never
reaches four, the partition merge waits for a flush, and the merged
piece is rewritten every few seals -- the probe's settle after the
point left one or two pieces where the arm without it left none, and
the write phase ran 15-35% longer. A version worth turning on would
count what a merged piece absorbed, or trigger the partition merge by
the pieces' bytes against the partition's, and would take only pieces
newer than a running partition merge's inputs so the two could run
beside each other. The mechanism, the test and the arm are here for
that version.

#### The fully-unmerged point, a third look: memory-bound, and the LSM's own

With the steady scan through a handle ahead of LMDB at every rung, the
one place the ladder is behind by a wide margin is this point, at a
seventh to a ninth of LMDB at ten and thirty thousand keys. Reopened
with the lag sweep's probe in the suite's shape, from the instruction
count up.

Callgrind over the point's pass alone at ten thousand keys, zeroed at a
marker before the scans, put 7,250 instructions in a scan of a hundred
entries: the same as a scan over the drained store, though the pass
builds a copy of every block it touches. Natively the same scan takes
13 µs against one. So the point is not doing work; it is waiting, and
the instrument that counts cannot see on what. Phase timers said the
wait is the build: 5-8 µs a copy over sixty-four partition records and
as many overlay keys, with the walk over a built copy 0.3 µs, and
`cpu-clock` samples over three hundred handle passes said what the
build waits on: a seek into the partition's index for every snapshot
key of the block, to find where the key cuts the walk (a third of the
samples, with the key reads and compares it makes); the copy's own
`memmove` (a fifth); the chase into the memtable for each key -- the
entry, the chain's tombstone check, the value (a third); and thirty
allocations.

Two of those were tried and neither moves the pass.

The seeks were removed. A snapshot's key cuts the partition where
`owner_of` would put it, a function of the snapshot's run and the
partition, so the cuts were walked once along the partition's index
heads when the snapshot's block bounds are taken and kept beside them;
the pieces already carry theirs. The seeks left the profile entirely.
Paired against the head that seeks, the same probe alternated three
rounds at ten, thirty and a hundred thousand keys, the minimum of each
did not move: 9.05 against 8.94 µs a scan, 11.3 against 11.7, 11.7
against 14.1, inside a sitting whose noise on one binary was 11.7 to
21.4. Instructions that overlap a memory stall cost nothing to remove.
It is not landed: it costs a walk over the run at every fresh snapshot,
which the first scan after a burst would pay, for a gain no pairing
could see.

The copies were skipped. A dense block on a read's first touch was
walked through its sources, as a wide block is, and copied only once
its reads had repaid a copy, the promotion `promote_entries` already
makes for a sparse block. In isolation, on repeated handle passes at
ten thousand keys, the walk reads 4.5 µs a scan against 9-15 for the
copy; at thirty thousand, with three pieces standing, the two are
level, since a walk reads and sorts each piece's keys over the window
at every scan where a copy reads them once; and ycsb-E loses 25-50% at
every threshold tried, because one wide walk of a hot block costs what
the copy it defers costs, and E touches its blocks fifteen times. The
wide walk's own profile: a quarter of it is the tombstone check on
each key's chain, which the suite's updates put there -- an update is
a delete and an append -- and which is the chain's first touch, a miss
the copy's build prefetches ahead in two sweeps and the walk does not.
Not landed either; the mechanism is a line in `materialize` and a
match arm at the promotion, and the numbers are here.

What the point costs, then, is the chase into the memtable for every
overlaid key and, above thirty thousand keys, the pieces per block:
memory latency paid at read time, in place of the write-time sort a
B-tree pays. LMDB's updates run at a third of this engine's, ycsb-A
0.29x-0.37x and the shuffled load 0.22x-0.55x in this sitting, and its
scans after them at full speed; a burst of N updates followed by N
entries scanned costs this engine less in total at every rung, and the
sweep's point, by its definition, times the second half alone. Against
the other engine that defers the sort, RocksDB tuned, in one process
at ten thousand and a hundred thousand keys: this engine reads the
point at 1.5x and 1.8x, every other scan quantity at 5x-20x, and E at
13x-23x; RocksDB loads 1.03x-1.39x faster.

The ground left at this point is write-time structure that survives
the burst's merges -- forms the merge itself would make over the frozen
snapshot, or an unsealed run kept in key order -- and each earlier
attempt at it, the carry and the builder from the publish, is priced
above. Neither is a read path, and neither is small.

#### The unsealed run: the snapshot carries the values

The write-time structure the point asks for, built as the read half
first. The scan snapshot already keeps the unsealed keys sorted in one
arena; `snapshot_runs` has it copy each key's whole chain beside them
when it is built or extended -- oldest chunk first, each with its
memtable offset and length, a tombstone by its mark -- so a read of an
overlaid key streams the copy under its own watermark instead of
chasing the entry, the chain and the value. A key written again after
the copy is found through a per-handle stale set fed from the write
log past the snapshot's log position and read from its chain; a key
filed since the snapshot carries no run. The cuts of the snapshot's
keys against the partition are walked once along the index heads and
kept with its block bounds, so a build seeks nothing. And a block the
runs cover, three quarters of its keys counting every source's run
over it, is walked on a read's first touch and not copied: every
partition record under such a run is masked by its update's
tombstone, and the copy would have been a copy of the run; a block met
a second time is copied then, since a block a mix reads over and over
wants the copy (the keeper's section below has that finding). A test
holds the runs to the chains on both scan paths, through the writer
and a handle under `Latest`, across a seal left in flight and a
reopen.

What the simulator says the copy costs, and why no one cut moved it:
callgrind with its cache and branch models over three handle passes at
ten thousand keys puts, per block built, about 440 mispredicted
branches (a quarter of them the per-key seek, a tenth the run parse),
a hundred cold-line writes for the copy's own destination and several
hundred first-level read misses. Three costs of the same size that
overlap, so removing any one of them stayed inside this machine's
noise, which on one binary ran 11.7 to 21.4 µs a scan across a
sitting.

What it measures, paired binaries alternated three rounds. On the
handle passes over the point's store, which build without the snapshot
copy in the timed region: at ten thousand keys 8.0 µs a scan against
9.7, every round; at thirty thousand level, 13.3-13.9 against
12.9-13.4, since the walk reads and sorts each piece's keys over the
window at every scan where a copy read them once; at a hundred
thousand level. On the writer's own pass, which is the suite's shape,
slower: 41-49 µs a scan against 28-31 at ten thousand keys, the copy
of ten thousand chains landing in the pass's first scan, and 19-25
against 18-20 at thirty thousand. E and the threaded scan mix do not
move. The walk over the runs at ten thousand keys, all its keys read
from runs and no copy made, still reads 8 µs a scan: the wide walk
assembles the window's overlay per scan and dispatches per key, and
that, not the chase, is now most of it.

So the option is off and `supdb-runs` prices it. Two things would make
it the structure it is meant to be: the copy paid at the commit, which
is write-time proper and takes the first scan's copy off the pass, and
is the keeper below; and a walk that streams the runs and the pieces'
records through cursors without assembling a window, at which point a
block the runs cover reads at the run's speed, which is the next work.

#### The run keeper: the copy paid at the commit, on a thread of its own

The write half of the unsealed run. `snapshot_keeper` gives the store
a thread at idle priority (`SCHED_IDLE` on Linux) that keeps the
published scan snapshot current to the commits, so the first scan after
a burst adopts a snapshot that has the burst, runs and all, and sorts
and copies nothing. Three things make that cheap enough to do at every
commit:

- The snapshot's key bytes and runs live in an arena shared by every
  version of one snapshot -- append-only, its blocks doubling from a
  base sized at the build, a reservation one fetch-add on the tail so
  no thread waits on the keeper for it -- and a version is a set of
  entries over it. An extension appends the batch's keys and runs and
  merges the entry run by a search and a block copy per batch key; the
  version before it copied every key and every run of the base into
  fresh vectors first, 1.4 MB at ten thousand keys, for every batch.
  The runs written again since the base's copy are the log's business
  and not a handle's: the extension reads the write log from the base's
  mark to its own and copies those again, so any handle can carry any
  published version forward.
- The keeper polls: it sleeps between looks at the commits, a hundred
  microseconds after a tick that did something and doubling to twenty
  milliseconds while nothing is due, and extends once the commits
  since its version amount to a sixteenth of what it holds, so a burst
  that outruns it is covered by fewer and larger extensions and never
  by a queue, and a mix of small batches does not buy a copy of the
  entry run per hundred writes. Nothing on the commit path or the read
  path touches it: the version before this one was unparked by every
  commit, and the wake is a futex call and an interrupt to an idle
  core, on this guest a VM exit paid by the thread that woke it, which
  read as 12-23% on ycsb-A's commits at a hundred thousand keys and as
  a tenth of the drained scan pass at ten thousand when a scan did the
  waking; polling, A's commits read level, 540-670 µs against 570-680.
  A publish and a settle wake it at once. It publishes each
  version into the state as a scan would, and stops publishing while a
  reader pinned across the burst holds the retired versions alive.
- It carries the snapshot across a publish: unchanged across a merge,
  whose memtables are the same; at a freeze the live entries become
  frozen ones, the run's order and so its block bounds intact; when the
  seal lands the frozen entries leave and the live keys and runs move
  to an arena of their own, so the old arena and the values it holds go
  with the frozen table.

The regime is the forms' own: nothing before the first scan over the
store, and past `snapshot_keeper_recent_pct` of the sealed keys written
since the last scan, nothing until the next, since the copy is a copy
of every value written and a load with no read in sight is not worth
doubling in memory; a dormant keeper looks for a scan at the long poll.
A writer whose own snapshot the published one has left
behind by an eighth of what it holds, or a thousand writes, takes the
published one; below that it keeps its own and files as before. A test
drives a burst against the keeper with a seal left in flight and a
handle claimed before it, holds every scan to the model through the
writer and the handle, and asserts that the writer sorted nothing after
the burst.

What it measures, paired binaries alternated three rounds at ten
thousand keys. With the forms maintained at commit, the default regime,
the burst's write time comes back to the shape without runs: the
writer's nine commits read 26-29 ms without runs, 33-48 with them and
the copy paid inline at each commit, 28-36 with the keeper paying it
beside; and the pass after reads 2.2-2.4 µs a scan against 21-23, the
snapshot current and the blocks walked over their runs. With the forms
off, which isolates the snapshot: the first scan after the burst falls
from 4.4-5.2 ms with the runs copied there to 1.4-2.9, the writer
builds no snapshot at all, and the pass reads 45-62 µs a scan against
44-53 without runs and 72-79 with them. The copy is gone from the read
path, and what is left of the pass is the walk, the other half.

Two things it found. The covered rule as first landed walked a covered
block on every touch, and ycsb-E at a hundred thousand keys read 0.58x
with the keeper beside the runs: its fresher snapshots made more blocks
covered, and E touches a block two hundred times a pass. A covered
block is now walked on its first touch and copied on its second, which
puts E level and costs the pass after a burst the copies of the blocks
it meets twice. And a pass that read bimodal across rounds, 2 µs a scan
in one and 15-36 in the next, was two shapes and not noise: at this
rung the burst seals once, at its seventh commit of nine, and whether
the seal has landed by the eighth commit's check decides whether the
ninth refills the writer's tables the landing dropped. The probe's
counters split the rounds -- the tables at the pass's start, 157
blocks or none -- before any average could hide them, and they say
what the 8x this point read for `supdb-runs` was: the runs' copy paid
inline made the eighth commit 8 ms instead of 4, long enough for the
landing to be caught there, so the ninth refilled the tables and the
pass walked them. The default arm's quick commits catch the landing at
the ninth and start the pass with no tables every round, and the
keeper gives the writer its quick commit back and that shape with it.
The pass that decides this point is a walk over a store whose tables a
publish just emptied, and what it costs is the walk, the other half.

The suite, paired rep by rep. At ten thousand keys over twelve pairs,
the fully-unmerged lag point reads 1.21x against the default arm,
twelve of twelve -- the walk and the second-touch copy over empty
tables against a copy per block -- and the four-thread scan mix 0.87x
on two pairs of twelve; nothing else separates. At a hundred thousand
over six pairs, against the default arm: A 0.84x, none of six the
other way; B, C and D 1.06-1.10x on four of six and the four-thread
scan pass 1.09x on five; the rest inside the noise. Against
`supdb-runs`, the read half alone: D 1.26x six of six, F 1.12x on
five, the two-thread scan pass 0.94x on none of six, A 0.92x on two.
The run before this one, with the keeper woken by every commit, read
the hundred-thousand scan passes 1.15x and 1.24x, the lag point 1.44x
and the four-thread mix 1.66x, six of six each, and this one reads
them level: on this machine the sign test inside a run is not the
spread between runs, and a figure a run gives six of six is a figure
until the next run. What the keeper costs where it costs is the
extension itself, and mostly the runs' copy rather than the entry run's:
every value written is copied once more, on a core beside the writer's,
which A's shape at a hundred thousand meets a hundred and sixty times
over five hundred commits.

#### What the pass over emptied tables costs, timed a call at a time

The walk that was to be the other half was measured before it was
built, with timers on the wide walk, the second-touch copy and the
first-touch build, over the handles' passes on the lag point's store at
ten thousand keys with the forms off. Callgrind first: the wide walk is
6,500 instructions a scan, 2 µs at this clock, where the pass measured
16-30 a scan, so whatever it costs is not instructions. Then the
timers: over two passes of a hundred scans the wide walk ran eight
times, at 2.7 µs a call. The covered rule barely fires on this store,
because the burst's seal has moved most of it into a piece and the
snapshot holds a tenth of a block's keys; the blocks are copied on
their first touch in every arm, 264 builds a pass, and those builds are
the pass -- 8-9 µs each from the chains, 11-12 from the runs. The
runs' three microseconds were their parse: every word read went
through the arena's block lookup and its bounds checks, and a run was
parsed twice a key, once for the tombstone and once for the values. A
run carries its records' length now and is sliced once, and a copy
from the runs reads 7.8-8.4 µs a block against 8.3-9.2 from the chains.
Prefetching the runs and the pieces' records ahead of the walk was
tried in the same sitting and moved nothing, so it is not here.

So the pass over emptied tables is a block build per block touched,
and the build is what stands between this point and LMDB's leaf walk.

#### The commit joined its seal after filling the tables the join empties

Why the tables were empty at the pass in every round of the default
arm was in the commit's order. A commit maintained the forms first --
the batch settled into the writer's tables, a form built for every
block it touched -- and joined a finished seal after, and the join's
publish drops the tables, so it dropped what the maintenance had just
filled; the next commit's maintenance was due only past the settle
bound, two batches at a hundred thousand keys, and a burst's last
commits are where the seals land. The commit joins first now, so the
commit that lands a seal fills the new tables from its own batch when
that batch reaches the settle bound, under the maintenance's own
regime; a publish makes nothing due that a scan has not, which is what
the regime's tests hold, and a version that marked every publish due
failed four of them and cost the hundred-thousand burst's writes 2.5x
in rebuilds. With the order alone, at ten thousand keys the lag point's
pass reads 0.95-1.46 µs a scan against 14, three of three, the tables
in place every round and the burst's writes unchanged; at a hundred
thousand nothing moves either way, since a batch of a thousand is half
the settle bound there and the landing's commit fills only when it is
a due one, and a freeze at the last commit leaves the pass building
whatever the order -- a freeze's batch is in the frozen table, the
pending list goes with the generation, and nothing rebuilds those
blocks until a write touches them. Filing the frozen batch after the
freeze was tried for that and does nothing.

Two arms would spare the pass the builds a freeze leaves it, and both
were priced beside the keeper with the forms on, three rounds. The
forms carried across the publish (`supdb-carry`) read 2.1-2.3 µs a
scan at a hundred thousand keys, every block in place, for a burst that
wrote in 400 ms instead of 130 -- and 21-41 µs a scan at ten thousand,
worse than not carrying, with the carried tables short of six blocks
and the pass building through them. A builder started at the publish
(`supdb-aheadpub`) read 8.6-13.7 at a hundred thousand for the same
write cost, its forms installed at the commits behind it, and level at
ten thousand. Neither was made the default on that: the pass's gain
was bought with the burst's writes both times, and the join-first
commit took most of what the builder gets for nothing. The carry's
price was then read commit by commit, and most of it was its own.

#### The carry, priced with its own costs taken out

Four costs in the carry arm were the arm's and not the carry's, each
found in the lag probe's commit-by-commit profile with the forms on.
The carry published the forms it moved, whether or not a handle was
there to read them, so every patch after it cloned its block before
writing: the hundred-thousand burst's 400 ms against 130. It reset the
log position across a landing, where the memtable is the one it was,
so the landing's commit settled the whole log a second time. A form the
patches alone had made -- clean at its build, a delta a write -- stayed
deltas for the store's life, and with the tables carried across the
seals every block of the lag point's store was sixty-four deltas walked
per scan, 2-4 µs against 1 over the copies a rebuild makes; a sparse
form grown past the dense bound is rebuilt as a copy by the next fill
now. And the carried tables took the new piece's bounds before its
ranks -- the carry runs inside the publish, and the ranks were taken
after it -- so a build through them sought the partition once per
piece key, where a build through a fresh table cuts at the rank.

The copies themselves grew under the burst. A patch appended the key's
new run and repointed the entry, so a burst that rewrote every key left
half of every copy's bytes pointed at by nothing -- at ten thousand keys
the carried copies held 2.0 MB where a fill makes 1.36, 660 KB dead and
seven of ten runs out of key order; at a hundred thousand, 18 MB from
13 -- and the blocks past twice their live bytes were unlisted and
built again in the pass that followed, half of them in one round. The
pass after the burst read 1.5-1.7 µs a scan over those copies against
1.0-1.15 over refilled ones; the same pass again, warm, read the same
over either (0.78-0.87 against 0.73-0.75), and again over the copies
refilled in place, so the layout costs the pass whose caches the burst
just left and not the walk. A run no longer than the one it replaces is
written over it now, in the copy and in the sparse form alike, and the
carried copies are a fill's size to the byte: the pass reads 1.12-1.15
against 0.99-1.05, the last tenth unexplained. `tests/db.rs` holds the
rule with a store rewritten at the same length, shorter, and longer.

With those out, the price that is left is the maintenance the carry
keeps alive. The profile of the default arm at a hundred thousand keys
reads the burst's ninety commits at 0.4-0.6 ms each from the fourteenth
on, with no tables at all: a freeze drops them, the builder ahead
cannot rebuild fifteen hundred blocks in the half-millisecond commits
between one landing and the next freeze, and the regime files nothing
without a scan, so the burst writes as if the forms did not exist and
the pass builds through three to seven pieces at 12-30 µs a scan. The
carry arm's tables are whole throughout, its maintenance files four
batches at every fourth commit for 5-8 ms, and the burst writes in
275 ms against 110 for a pass at 2.2-2.6 µs a scan -- or 5.8 when the
burst's last merge rewrote the partition and the refill was still
landing a few hundred blocks a commit when the pass began. That is the
trade, and the suite takes it: `bench ab` over eight pairs at a hundred
thousand keys reads the fully-unmerged lag point at 7.8x and ycsb-E at
1.07x, eight of eight both, with nothing lost (F 0.96x, three of eight);
five pairs at three hundred thousand read the point at 3.3x, the
drained scan pass at 1.15x, A, D, E and F at 1.06-1.18x and the point
reads at 1.07x, five of five each; eight at ten thousand read level on
every mix and lag point, the four-thread scan the one quantity leaning
the other way (one of eight, p=0.07). The carry is the default and
`supdb-nocarry` is the shape before it.

Three leads it leaves. The carry resets the tables' snapshot bounds
and the first scan after a landing rebuilds the snapshot, 170 µs at ten
thousand keys and 400 at a hundred thousand, which is a fifth of a
hundred-scan pass and could be built at the carry, or by the keeper.
A replacing write's patch seeks every piece for the key and then masks
what it finds, so a patch under seven pieces pays seven seeks for a
run it could have written from the memtable alone. And the refill
after a merge's rewrite lands a few hundred blocks a commit, so a pass
that begins within three commits of one builds the rest itself.

#### What the filing costs, asked of the settle rather than of a policy

The carry leaves one price: the writer files every commit's batch into
the forms, and the fully-unmerged point's burst pays about 250 ms of its
500 at a hundred thousand keys for filing nobody has yet read. Two
policies were tried against it and both are refuted; the price turned
out to be stalls rather than a decision.

The first was to spend per block instead of per key. A patch resolves
one key's run and splices it; a build resolves a whole block. So the
backlog's density -- keys per block it touches -- says which is cheaper,
and the settle now counts it (`settle_density`, and
`Options::forms_settle_rebuild_from` is a bound over it). The density
is 1.8-1.9 keys a block at a hundred thousand and three hundred
thousand, and 6.1 at ten thousand, where a batch is a tenth of the
store: a bound of six is reached by about one block in a hundred at the
larger rungs and by three in five at the smallest. Where it is reached
it loses. Six pairs at ten thousand keys read the threaded scan mix at
0.77x (0/6, p=0.031) and 0.58x on four threads, ycsb-D at 0.88x and A
at 0.92x, for 2.7x the blocks built by the engine (6/6): the rebuilds
replace patches the reads had already paid for and land on the reads
themselves. At a hundred thousand, where the bound is rarely reached,
the burst's writes and the pass read as they do without it. So the
choice is not between a cost per key and a cost per block -- the block
the backlog rewrites is the block the reads want -- and
`supdb-rebuild` prices it with the default at zero.

The second was to defer the filing while no scan is near, which is what
`forms_settle_recent_pct` does. At ten percent of the store the burst's
writes fell only to 366-432 ms from 467-518 and the pass read 22-27 µs
a scan against 3.5: the window gates the whole maintenance, the fill
included, so the pass builds every block like the arm that carries
nothing, while the seals file the backlog anyway -- a carry must leave
the forms current to the whole log, and the burst seals a dozen times.
Deferral and the rebuild bound together wrote in 270-311 ms, near the
226-240 of carrying nothing, and read 22-57 µs a scan, which is that
arm's trade back again.

So the filing was timed instead, by phase, over the burst at a hundred
thousand keys: resolving the batch's keys to their blocks 44-53 ms,
applying them 116-133, of which the run's emission was 15 and the
splice into the form 67-91. Callgrind put the patch at about 2,200
instructions, and three suspects in it were refuted by measurement. The
piece seeks are not the cost: a `put` is a delete and an append, so a
tombstone in the key's chain masks every older source and the pieces
hold nothing the run emits -- 88,742 of the point's 88,744 keys -- and
skipping the seeks entirely moved nothing. The live-byte sum the bloat
test takes over every entry is not the cost: a counter the patch
maintains instead moved nothing. The emission's machinery is a tenth of
it: an `Over` allocated and freed per key, `emit_over`'s walk over the
sources and `oldest_live`'s second walk of the same chain, all of which
a masked key can skip by writing its run straight from the chain, which
took the emission from 15-18 ms to 12-14.

The splice is the lower bound's misses. It is a binary search over the
form's entries and its keys, six dependent loads into two cold
allocations, about 0.9 µs a key -- and the settle's keys are grouped by
block, so the next group's form can be fetched while this group is
patched. With that one prefetch the splice reads 21-25 ms at a hundred
thousand against 67-91, and 65-85 at three hundred thousand against
292-300. The apply loop reads its keys in key order where the log wrote
them in arrival order, so the keys two ahead are fetched too, which
took the apply from 337-346 ms to 290-318 at three hundred thousand.
Asking the wide bound once a block rather than once a key was tried
beside them and moved nothing, its words being warm from the blocks
before.

End to end, three rounds at a hundred thousand and two at the others:
the burst's writes read 401-412 ms against 464-568, 1245-1389 against
1572-1755 at three hundred thousand, and 22.1-22.2 against 24.0-25.0 at
ten thousand. A second sitting on another host, which is what this suite
asks of a margin before it is a result, read 342-407 ms against 444-531
at a hundred thousand, three of three, and 1205-1335 against 1766-1795
at three hundred thousand. The rule the two prefetches are an instance of: a batch
applied to a structure keyed by position knows every position before it
applies the first, so a stall it pays per key is a stall it need not
pay at all.

#### The snapshot dropped at a publish, and what carrying it moves

The writer drops its scan snapshot at every publish, so the next commit
due sorts every unsealed key again. Timed at the burst, that is fifteen
builds at about 2 ms over the hundred-thousand lag point and eight at
4-7 ms over the three-hundred-thousand one -- 30 and 50 ms of bursts of
400 ms and 1.4 s -- while the snapshot's bounds walk, cached by the
partition's blob id, costs nothing there at all.

The keeper already carries its own snapshot by three cases
(`carry_snapshot`): whole across a merge, its live entries made frozen
ones across a freeze, which keeps the bounds since the run's order does
not change, and at a landing its frozen entries dropped and its runs
moved to an arena of their own, which does not. Wiring the writer into
the first two (`snapshot_carry`, `supdb-snapcarry`) does what it says:
at a hundred thousand keys the builds fall from eighteen to sixteen over
a pair, six of six, and the extends rise from three to twelve, six of
six; at three hundred thousand, twenty to eighteen, five of five. The
landing's case is the keeper's alone -- for the writer it dropped the
bounds and the burst then walked them, 20 ms against the 17 ms of builds
it had saved.

No timing quantity moves. Eleven pairs over the two rungs read every
mix, the point reads, the scans and the four lag points within noise,
the largest leaning being a load at 1.15-1.16x on three and five of six.
The probe read the three-hundred-thousand lag pass at 6.0-9.1 µs a scan
against 10-26 over four rounds each, which looked like the mechanism and
was not: two binaries alternated across processes is the shape
`bench/CLAUDE.md` warns about, and the in-process pair refuted it. What
the carry moves is where the work lands rather than how much there is --
a snapshot in hand at a commit is one the fill walks its bounds against
there instead of at the first scan after -- and at these rungs the two
places cost the same. It is off, and the arm prices it.

#### The snapshot built where a block needs it

A scan that finds no snapshot of the state in hand -- dropped at the
publish before it, not yet rebuilt by a commit -- built one before it
walked anything, and the first scan after the lag burst at three hundred
thousand keys spent 4.4-6.5 ms of its 7-10 on it over a pass that built
no block. `scan_lazy_snapshot` walks without one and stops at the first
block that reads it -- a block to build, a wide form, a table to make,
the builder's forms to install -- builds it there and goes on from that
block's first key, which is exact: everything emitted is below that key
and everything after is at or above it.

It needed two fixes before it could be priced at all, and each was found
by a count rather than a time. With nothing unsealed the lazy walk ran
every scan of the drained pass at three hundred thousand keys, block by
block, where the eager one takes a clean partition's one walk: 0.92x,
0 of 6. An empty snapshot costs nothing to build, so the walk is lazy
only while something is unsealed. And the walk started the builder
ahead before it held a snapshot, so the builder sorted every unsealed
key itself where it would have adopted the writer's: a histogram of the
build sites over four passes read 79 builds for the eager arm and 82 for
the lazy one, which saved two builds on the scan path and added four on
the builder's core. The builder starts once the walk holds a snapshot
now, and the same histogram reads 73.

Then it is flat. Ten pairs at three hundred thousand keys mark nothing
that holds for the table, the builds over a pass level at twenty, and at
a hundred thousand the lazy walk almost never runs. The scan path makes
a tenth of a pass's builds; the commits' maintenance makes the rest
whichever way the scan goes. It is off, and `supdb-lazysnap` prices it.

The first pricing, before either fix, read wins at a hundred thousand
keys -- the drained scan 1.11x and ycsb-A 1.10x, each six of six at
p=0.031 -- on a rung where the fixed path later ran no lazy walk at all,
and ycsb-A runs no scan. Six of six is the sign test's floor for six
pairs, and a table of three dozen quantities marks one or two at that
floor by chance: an arm set against itself did so twice
(`bench/CLAUDE.md`, the ab control). `bench ab` marks a quantity twice
now only where it holds for the table as a whole.

#### The snapshot's build, a sort and two gathers

The first scan after the hundred-percent lag burst at three hundred
thousand keys spent 8.6 ms before it walked anything: 3.7 ms settling the
burst's backlog into the forms and 4.8 ms building the scan snapshot over
34,203 unsealed keys, 31,211 of them in the frozen table of a seal still in
flight. The same build ran about sixteen times inside the burst, at the
commits that maintain the forms. Timed by phase in the probe, a build over
34-42 thousand keys was a third walking the tables, 2.1-4.3 ms in the
comparison sort of the prefix records, and 0.9-2.9 ms laying the sorted
entries down.

Nothing in it needed a comparison sort. The records are two big-endian
prefix words and an index, so an LSD radix over only the bytes that vary
among them orders them exactly as the comparator did, stably, and the rare
run of equal prefixes -- keys sharing sixteen bytes -- is sorted by its
keys afterwards; the suite's keys vary in six of the sixteen bytes and take
six passes. Alone, on the suite's key shape, it is 0.69 ms against 1.98 at
34,000 records and 2.9-4.2x faster from three thousand to three hundred
thousand. The fold that laid the entries down compared each key with the
last through the arena, two scattered reads a record, where the prefixes it
already held say whether two keys of sixteen bytes or less are the same.
And the walk took an atomic reservation in the arena for every key, where
the keys are the only bytes it appends and can take one between them.

Over three runs of the probe the first scan's build went from 4.4-4.7 ms to
2.3-3.4, the sort to 1.0-1.3 ms, the fold to 0.4-0.8 and the frozen table's
walk from 1.3-1.5 to 0.8-1.2. That is a phase measured inside one binary,
not a pass priced against another, and the pass it sits in is bimodal, so
no figure for the lag point rests on it. The settle beside it is the larger
cost over the whole run -- twelve percent of the probe's samples against the
build's two, over a microsecond a key filed, spread across locating each
key's block through the ordered index (a fifth of it), parsing the block's
records, the memtable's tombstone check and the patch copies -- and is the
next thing to take.

The record parse was the settle's largest leaf, and one of its sources
needed no record at all. Finding a write's block asked the ordered index
for the rank and then read the partition's record at that rank to learn
whether the key was there, a cold line in the record region for every key
filed; the heads say so themselves wherever the keys have one length, and
a scan's start had already been moved onto `seek_exact` for the same read.
`owner_of` takes it too. The probe built before and after, alternated over
six rounds in one sitting, put the first scan after the one-percent burst
at 1.0-1.5 ms against 1.4-1.8 and after the ten-percent one at 1.7-2.5
against 2.2-3.7, lower in six pairs of six each, and the ten-percent
burst's commits at 0.91x, five of six. Two binaries is a probe and not a
result; the hundred-percent point sat under its own bimodality in both.

#### The fully-unmerged point's two shapes, and the merge between them

The hundred-percent lag point has been bimodal at every rung that seals
during its burst, and the shapes are named now. Counted across the pass
at a hundred thousand keys: the fast pass starts with every block's form
held -- 1,563 of 1,563 -- builds nothing and reads 25-45M entries a
second; the slow one starts with none published and 0-1,088 in the
writer's tables, builds 850-1,500 copies at about 14 us each, and reads
6-12M. A carry refused for a replaced partition separated them exactly,
one in every slow run and none in any fast one: the partition merge that
folds the burst's pieces had published during the burst, and the carry,
which keeps forms only over the same partition objects, dropped them all.
Whether that merge lands before the pass is timing, which is why the
same binary took either shape run to run, and why a change to the
store's size alone could move a rung from one to the other.

A merge that folds updates rewrites a partition over the same keys, and a
block's copy owns every byte of its range. `forms_rebase` carries the
copies across a merge whose partition has the same fences, key count and
key at every block's first rank, and drops the sparse forms, whose deltas
splice at ranks in the old partition's records. In the probe the slow
shape went away, twelve runs of twelve at 3.8-4.6 us a scan whether the
merge had landed or not.

`bench ab` against the arm without it, first under the file-sized seal:
at a hundred thousand keys the point read 2.02x (10/12); at three
hundred thousand the arm without it collapsed in four reps of twelve,
and in the reps where both took the fast shape the rebase read
0.61-0.97x, which kept it off. With the seal sized by the data and the
compact record on, sixteen pairs a rung:

| rebase over none | 10k | 30k | 100k | 300k |
|---|---|---|---|---|
| scan-lag, all of it unmerged | never fires | never fires | **2.62x** | **2.55x** |
| every other quantity | ns | ns | ns | ns |
| forms held at the end | same | same | 2.0x | 2.0x |

Both starred figures are 15/16 at p=0.001 and hold under Holm. The arm
without it collapsed in fourteen reps of sixteen at three hundred
thousand, to 4.8-13.7M against 18.6-25.0M, and in the two where it did
not the rebase read 0.76x and 1.13x: the cost seen in the first pricing
did not come back. Nor did the probe find one. The second pass over
carried copies, which builds nothing, read 1.65-2.49 us a scan against
1.71-2.54 over copies built fresh, one batch of rounds each way, which
is the machine's placement of the bytes rather than the copies. Below a
hundred thousand keys no partition merge rewrites a partition over the
same keys during a pass, so the blocks built and the forms held are
identical with it and without. The forms it holds at the end are the
copies of the burst's blocks, which the arm without it holds too until
the merge's publish drops them. It is on; `supdb-norebase` prices it.

#### The compact record, and the seal it moved

A record whose one run is inline carried a twenty-byte extent that said
nothing its position did not: the run is inline, starts at the tail and
ends where the tail does. `compact_records` writes a four-byte header in
its place -- the run's length, and its count with the fixed and tombstone
flags above it -- from which the reader rebuilds the extent (format
0007). The suite's record goes from 140 bytes to 124, the store from
1.504 bytes a stored byte to 1.366 at a hundred thousand keys, and the
device bytes of the ten-thousand-key load from 1.592 to below LMDB's
1.494.

Priced first at the Blob, the same keys at both record sizes and rounds
alternated: a segment's scan read 0.86-0.98x the time from a hundred
thousand keys to three million, 29 rounds of 36 faster, with point reads
level. In the engine it first priced as a loss: at a hundred thousand
keys the point read 0.71x (0/12), ycsb-A 0.74x (1/12) and ycsb-B 0.86x
(2/12) -- and ycsb-A never scans, so no read of a record explains it.
The seal cap did: it was a share of the store's bytes on disk, and the
same data in a denser file sealed a tenth sooner, 1.50 MB against 1.66.
Over the suite's mixes, run back to back on one store, that was one more
seal -- twenty snapshot builds against eighteen, twelve pairs of twelve
-- landing inside the mixes.

With the seal sized by the data (the seal cap's section), both records
seal at one threshold, and against the full record in one process
(`supdb-fullrec`, twelve pairs a rung) the compact record is a record
size and nothing else:

| compact over full | 10k | 100k | 300k |
|---|---|---|---|
| bytes on disk | **0.90x** | **0.91x** | **0.90x** |
| drained scan | 1.02x (10/12) | 1.01x (ns) | 1.06x (8/12) |
| scan, a hundredth unmerged | 0.96x (ns) | 0.96x (ns) | 1.09x (10/12) |
| ycsb-A, ycsb-B | level | level | level |
| snapshot builds | 3 and 3 | 20 and 20 | 19 and 20 |

Every quantity marked on the sign test at any rung favours the compact
record, the drained scan is faster in 25 pairs of 36 across the three
rungs, and nothing reads slower. It is on.

Sizing the seal by the data moved the full record's cadence where the
calibration is not exact. Against `supdb-sealfile` at a hundred thousand
keys, whose partitions weigh 1.434 times their data where the rule takes
1.4, the threshold is 1.62 MB against 1.66 and the mixes take one more
seal, twenty snapshot builds against eighteen (11/12): ycsb-B read 1.12x
(10/12) and the fully-unmerged lag point took its slow shape in more reps
(ns). At three hundred thousand no quantity of 36 moved, and below a
hundred thousand the floor binds under both rules. A rung is on a knife
edge wherever the mixes' writes land near a multiple of the threshold,
and a few percent either way moves one seal and whichever pass it lands
in; the slow shape of the lag point is what `forms_rebase` removes.

"Nothing reads slower" held for every path `supdb-fullrec` prices, and
it prices the partition path. The buffered arm, `supdb-ingest`, keeps its
level-0 piece a piece, so its scans walk the merge path, which asks each
source for a key and then for that key's values, and `Blob::key_at`
decoded the whole record for the key: every key a merge compared rebuilt
the compact extent, and the values read after decoded it again. The next
quick row found the arm's drained scan at half its rate; bisected in one
sitting against `lmdb-nosync`, the drop came in two steps, at the commit
that gave the read path its `Exts` type (compact records still off) and
at the one that turned them on. Callgrind put the pass at 80.4M
instructions against 63.9M before either. The key is read alone now
(`flatindex::record_key`, which refuses what the full decode refuses in
the words it reads), by `key_at` and by the index's own seek, and
`values_at` takes a record's extents lent from the frame that rebuilt
them (`FlatIndex::with_record_at`) rather than returned through three
layers, which with the instructions level still cost a sixth in time.
The pass is 62.4M instructions; the probe with the arm's shape reads
17.4M entries a second against 19.9M before the compact record and 19.5M
writing full records, where it read 11-14M. What is left is the
extent's rebuild per key, which the partition path's in-place loop
never pays.

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

#### The writer's upkeep on a thread of its own

The deferred settle is a bill, and the settle bound only chooses which
operation pays it: a commit that crosses the bound, or the first scan
after a burst whose last commits did not. `Options::upkeep` gives the
work to a thread of the store's own instead (`Upkeep::Background`), so
it runs while the writer is elsewhere -- during the commit's own
barrier, and beside the next batch. It is the default at level 2;
`supdb-inline` is the arm that keeps the writer doing it.

What moves is the writer's `FormsState`: the block tables, the scan
snapshot and the log position both are current to. A commit lends it,
before its fdatasync so the pass overlaps the device, and names the
commit the pass is to bring it to -- the generation, the log length,
the entry count and the watermark, taken on the writer's thread, where
they are one commit's by construction. The thread runs the commit's own
`maintain_forms` through a `Reader` of its own, bounded to what the
writer named, since the writer goes on appending past it. The writer
takes the upkeep back at the first thing of its own that needs it -- a
scan through the writer, a publish, a handle's claim -- and waits only
for a pass in flight; puts, point reads and commits leave it lent. The
rules that keep one structure on two threads correct:

- Only one thread holds it. The cell is emptied through `&mut` and
  refilled through `&self` only while empty, so no field of it needs to
  be `Sync`.
- Every publish takes it back before the swap. The thread pins
  nothing, as the writer's own handle pins nothing, so the state it
  reads must not be retired under it, and the only thing that retires a
  state is the writer's publish.
- The forms it publishes are stamped with the commit it was named, not
  the writer's latest: stamped later, a handle at that later commit
  takes forms without the writes between. The threaded test at level 1
  kills that mutant in every run; under a hold the writer has committed
  nothing past the named commit, the two stamps agree, and the test at
  level 3 cannot tell them apart.
- Retired forms and snapshots are swept only by whoever holds the
  upkeep: swept by the writer while it is lent, a form the thread had
  just replaced could go under the thread's own read.
- A pass that panics -- a debug assertion in the checked profile -- is
  raised on the writer when it takes the upkeep back, rather than
  leaving the writer waiting on a thread that has nothing to give.
- `settle` joins the upkeep before anything else, because every other
  join touches the writer's upkeep and a touch takes it back from the
  thread untouched. The first version joined it last, and a test that
  asked for the thread's work after a settle found it had never run.

A commit that does not hold wakes the thread only for a batch of 256
writes or more; a smaller one waits for its poll, a hundred microseconds after a pass and
doubling to twenty milliseconds while nothing is lent, or for the read
that takes the upkeep back and files it itself. A wake is a futex call
and an interrupt to an idle core, which on this guest is a VM exit the
waking thread pays, and a wake at every commit cost the keeper 12-23%
of ycsb-A. The level is how eagerly a pass files: 1 by the commit's own
rules, 3 everything at every commit, and 2 everything while reads are
around -- a scan over the store within the last two stores' worth of
writes -- and the commit's rules otherwise, so a long run of writes
nobody reads pays nothing for a structure nobody walks.

Testing it found a defect older than it: a handle took the log length
it settles to and the watermark it reads under as two loads of words
the commit stored in turn, and could hold one commit's length with the
watermark before it ("Read and write concurrency" below has the marks
that fixed it).

Priced first with every commit lending and none waiting, `supdb-upkeep`
(level 2) against the default, sixteen pairs a rung, won the fully
unmerged lag point at a hundred and three hundred thousand keys, 1.28x
and 1.33x, and ycsb-D and F at three hundred thousand, and lost the
tenth-unmerged point at every rung, 0.38x at ten thousand to 0.79x at
thirty, and most of the small rungs' lag points besides. The probe
with the sweep's shape said why, and it was not the scans: at a hundred
thousand keys and a tenth unmerged, the burst took 10-13 ms against
20-24 inline, because the writer no longer settled, and the thread was
a commit or two behind when it ended, so the first scan waited 0.3-0.7
ms for the pass in flight and filed the rest itself, 0.4-1.6 ms against
10 µs inline, whose burst's last commit had filed everything. The
thread had moved the bill from the burst, which the sweep does not
time, to the first scan, which it does.

So with reads around, a commit holds: it wakes the thread, and returns
only once the thread has filed it, which costs the longer of the
barrier and the pass instead of their sum; at level 2 a batch too small
to wake the thread for is filed by the commit itself, where level 3
holds for it too; and with no reads around the thread still takes the
burst without anyone waiting for it. The
probe's first scan came back to inline's 6-17 µs at the tenth point,
and to 56-62 µs against 1.8-1.9 ms at the fully unmerged one, whose
snapshot the thread had extended during the burst. Priced again,
sixteen pairs a rung:

| | 10k | 30k | 100k | 300k |
|---|---|---|---|---|
| scan-lag, all of it unmerged | 0.76x (ns) | 0.89x (ns) | **1.59x** | **1.75x** |
| scan-lag, a tenth | 0.89x (ns) | 0.99x | 0.98x | 1.23x (ns) |
| scan-lag, a hundredth | 0.98x | 1.00x | 1.01x | 1.28x (14/16) |
| ycsb-D | 0.97x | 1.13x (13/16) | 1.00x | 1.08x (ns) |
| ycsb-B | 0.85x (3/16) | 1.08x | 0.92x (ns) | 1.04x |

Bold is 16/16 and holds under Holm; nothing else in the four tables
does, and no quantity loses by a margin that does. A second sitting
read the fully unmerged point at 0.78x, 0.75x, **1.77x** and **1.69x**
up the ladder, the two small rungs 3/16 and 2/16, and at three hundred
thousand the tenth and hundredth points at 1.20x (14/16) and 1.38x
(13/16). The large rungs' win holds across sittings; so does the small
rungs' loss at that one point, and that is what keeps the default
inline.

The small rungs' loss is not the engine's work. Callgrind over the
pass after the burst at ten thousand keys, 99 scans, counts 784,961
instructions inline and 784,886 with the thread; the forms by kind and
their layout are identical, and so is every count the suite keeps. Two
things the probe did separate: the thread's burst ends 5-10 ms sooner,
so the seal it started is still writing through the timed pass where
the inline burst's had finished, and on this guest a core just back
from idle runs the same pass at 1.2-3.1 µs a scan against 0.9 back to
back, which a writer parked on its hold is. Neither closes the gap
alone: waiting out the seal, spinning the writer awake, a single malloc
arena (0.85x and 0.88x) and forms rebuilt on the writer's thread each
narrowed it in some rounds and not others. What is left is below what
this machine can show -- no counters, and pages placed where the host
puts them -- and a pass of a hundred scans, two hundred microseconds,
is where it shows. The thread's own cost here is real to a caller
too, since a background seal after a burst competes with the reads
after it whoever leaves it running.

What the small rungs did say is that the hold took nothing off their
reads. Their settle bound is 200 and 600 writes against the sweep's
commits of a thousand, so the inline commits filed every batch
themselves and left the first scan nothing; the thread only moved that
work to another core and let the burst end sooner. So level 2 now
holds only where the commit's own rules would leave the batch to the
next read -- no scan since the last commit and the backlog under the
bound -- and otherwise does exactly what `Inline` does, asking the
rules through the one function the maintenance asks (`settle_due`,
without consuming its one-shot reasons). Sixteen pairs a rung:

| | 10k | 30k | 100k | 300k |
|---|---|---|---|---|
| scan-lag, all of it unmerged | 0.99x | 1.06x | **1.56x** | **1.66x** |
| scan-lag, a tenth | 0.99x | 1.16x | 1.04x | **1.31x** |
| scan-lag, a hundredth | 0.98x | 1.05x | 1.05x | 1.27x (14/16) |

Bold is 16/16. Nothing at the small rungs is marked. Against LMDB,
twelve pairs a rung, the arm wins every lag point at every rung: at
the fully unmerged one 2.10x, 1.70x, 1.09x and 1.28x up the ladder,
where the inline default reads 1.75x, 1.60x, 0.75x and 0.66x.

A second sitting of that rule read the tenth point at ten thousand keys
at 0.78x (1/16, holding under Holm) and the drained point before any
burst at 0.79x, where the rule holds nothing -- but it still lent the
thread the load's commits, with nobody reading, and the drained scans
read forms the thread had built. That lending bought the load nothing
the suite could see (0.95-1.06x), so level 2 no longer lends at all:
it is `Inline` but for the hold, and the thread starts at the first
commit that has something for it, so a store whose commits never hold
runs no thread. Priced once more, with the lending gone and the thread
still started at the first commit, the fully unmerged point reads 1.04x,
1.07x, **1.67x** and **1.63x**, the tenth at three hundred thousand
1.22x (14/16); the small rungs' lag points lean 0.83-0.86x at ten
thousand with nothing marked, in a table whose other quantities lean
0.88x and 1.14x as far. One quantity to watch: ycsb-E at three hundred
thousand has read 0.82-0.98x over five sittings, never marked. Its
commits are small and follow scans, so they never hold; ycsb-A's before
it do, with the scan pass earlier still counting as reads around, and
E then scans forms the thread patched.

A sitting with the lazy start read the fully unmerged point at 1.02x,
1.09x, **1.90x** and **1.71x**, the hundredth and tenth at three hundred
thousand at 1.36x (15/16) and 1.13x (14/16), nothing at the small rungs
marked -- and ycsb-E at a hundred thousand at 0.935x (2/16), its first
mark. Over the six sittings E has read below even in ten of the twelve
rung-sittings at a hundred and three hundred thousand, by 2-8%. That is
a cost, small and repeatable, and the default took it with the win:
level 2 is the default, against a pure-win rule it does not meet. The
window that makes the lag sweep's bursts "reads around" also makes
ycsb-A's updates hold for a scan that comes three mixes later. A
narrower window would stop A holding, and would stop the fully
unmerged burst holding past the same number of writes; whether the
burst keeps its win then is the next thing to price, not a thing this
section knows.

#### Commits that never hold, priced by the sum

At level 1 a commit hands its writes to the thread and neither holds for
it nor files them itself, so no commit does work for the reads. Priced
in one process against the same arms at level 2 (`supdb-hold` and
`supdb-ingesthold` now), ten pairs a size: ycsb-A 1.58x and 1.41x and
ycsb-F 1.23x and 1.29x on the buffered arm at a hundred and three hundred
thousand keys, 10 of 10 each, and 0.98-1.20x and 1.02-1.34x unresolved on
the durable arm, whose commits are mostly their fsync; the lag sweep's
passes 0.09-0.87x on the buffered arm and 0.24-0.92x on the durable, the
tenth point 0.34-0.66x at every size on both. The suite times a lag
point's pass and not its burst, and the hold is what moves the
conversion of the burst's writes out of the pass and into the burst.
Timed apart -- the lag probe, one rep a process, four rounds -- the burst
and the pass summed read lend at 0.48-1.01x of the default: the fully
unmerged point at 0.48x and 0.51x on the buffered arm at three hundred
and a hundred thousand keys, its burst 214 ms against 679 and its pass
116 against 14 at the larger, and the hundredth point level everywhere.
In the pass after a burst the upkeep thread took 0.03% of the samples,
and the scans built the blocks they read over pieces the burst's merges
had not yet folded, at about the merge path's speed: without the block
cache the fully unmerged point reads 2.7-5.0M entries a second, with the
forms 22-54M. Lend does less work than the hold and moves it to where the
suite does not time it; the pass after a burst is its cost, and
shortening that window is the background's job, not the commit's.

Level 1 is the default now, and the lag sweep times each burst beside its
pass (`updates_per_s_lagNpct`), so the trade reads in both quantities.

#### The freeze that leaves its carry to the next look

At level 1 the freeze is the one place the writer still does work for
the reads: it takes the upkeep home, waiting out a pass in flight, reads
the log, settles the whole backlog into the forms and publishes them,
and only then swaps the tables, carrying the forms across only if no
other publish came between its prepare and its swap. On the buffered
arm's fully unmerged burst at a hundred thousand keys a seal's landing
comes between them at most freezes, the freeze carries nothing and
drops the tables, and the pass builds every block itself. With
`Options::freeze_settles` off the freeze swaps the tables and touches
nothing else; the next look at the log, the thread's or the writer's,
carries the tables across it as it carries them across a landing, keeps
the frozen table's unsettled writes to settle through that table, and
finds every table live since its last look in `State::replaced`.
That freeze is the default now; `supdb-settlefreeze` and
`supdb-ingestsettlefreeze` keep the freeze that settles, priced in
"The dense-burst regime" below.

The first pricing read the durable arm's fully unmerged burst at 1.11x
and 1.27x at a hundred and three hundred thousand keys (8/8 each) and
the buffered arm's pass after it at 0.26-0.44x. The lag probe did not
reproduce the second: it read the store's counters between the burst and
the scans, and a counter read takes the upkeep home, so the probe waited
out the thread's pass before its clock started. Read inside the suite's
own loop, the writer's first scan after the burst was 17-82 ms of a pass
the rest of which took 4: a wait for the thread's pass in flight of
14-36 ms, then 26-34 ms settling the twelve to sixteen thousand frozen
writes the thread had not reached, at 2 µs a write, and in two reps of
seven a look that found a table missing and dropped every one, after
which the pass built the store.

The 2 µs was the general path. A settle copies a key's run from its own
chain when the chain holds a tombstone, which masks every older source,
and asked that only of live writes; a frozen write has the same chain,
and took a seek of every piece standing. Given the masked path, frozen
writes settled at 0.6-0.9 µs and the pass read 0.71-1.15x of the
default's. The missing table was one that went live, froze and landed
between two looks: the look's note held the table live at that look and
nothing after it, and the thread's passes ran longer than the freezes
came. `State::replaced` keeps every table replaced since the upkeep's
last look, which it publishes (`Shared::upkeep_lives`), up to eight;
with it no look dropped its tables in any rep, the thread's included.

Priced in one process against the default, eight pairs a rung, burst
and pass summed pair by pair: on the durable arm the fully unmerged
burst read 1.17x and 1.25x at a hundred and three hundred thousand keys
(8/8 each) and its pass 1.04x unresolved, summed 1.22x and 1.28x (8/8
each), with nothing moved at ten thousand. On the buffered arm the
burst read 1.66x at ten thousand (8/8) and level above it, and the pass
0.49x, 0.75x and 0.63x at the three rungs (2/8, 1/8, 0/8), summed 1.25x
at ten thousand (7/8) and 0.88x and 0.87x at a hundred and three
hundred thousand (1/8, 2/8).

What is left is capacity. Through the hundred-thousand-update burst the
thread did 90-110 ms of maintenance against a burst of about 70: some
eighty thousand settles at about 0.45 µs, and 18-28 ms of fills, most
of them blocks a sixteenth patch had made dense and the fill rebuilds
as copies, one per block per burst. It ends the burst 25-45 ms behind,
and the writer's first scan waits for the pass in flight and settles
the rest. The freeze that settles drops the tables at most freezes, and
the pass builds each block once, about 20-25 ms for the store. Over a
dense burst the per-write upkeep costs about four times what one build a
block after it does; on the durable arm, whose commits wait on their
fsync, the thread keeps up and the burst gains.

#### A cost model for the background work

What decides when the background works, how much and in what order is
the foreground's time, with reads weighted twice writes (`γ = 2`): a
lag point is priced by its burst plus twice its pass, which `bench ab`
prints beside the two rates it is made of, as `weighted_ms`, lower
better. Per write and per block, at a hundred thousand keys: a patch `p` ≈ 0.4-1.0 µs on the
upkeep thread, a block build `B` ≈ 15-25 µs, a ready block's walk
about 2.5 µs, the buffered writer `c_w` ≈ 0.75 µs a write and the
durable one 2-2.5. Three rules follow, block by block. A block's
writes before its next read cost `k·p` patched and `B` rebuilt, so past
`k* = B/p` (about forty) it is cheaper rebuilt once, and online the
choice is rent-or-buy. A patched form walks slower than a copy, and
rebuilding it repays only through later walks, so with no read coming
it never repays. And the thread keeps up only while
`ρ = c_t / c_w ≤ 1`, `c_t` its cost a write; past that, the deficit
`(ρ - 1)` of the burst lands on the first read at weight `γ`, or on
the writer at weight one if the writer is held back, but held in
lockstep the writer pays the thread's whole work, not the deficit.

The upkeep's cost a write is counted now (`upkeep_ms` against the
writes a point files). Measured over the lag sweep's largest burst on
the buffered arm, the lazy freeze's thread spent about 1.1 µs a write
against the writer's 0.75, a sixth of it rebuilding dense sparse forms
nothing would read before the burst patched them again. With
`Options::forms_convert_unread` off, the fill rebuilds a dense form
only where a scan has come since the fill before: the burst's builds
fell from about 1,570 to under a dozen, the thread to about 0.85 µs a
write, and the pass after it from about 44 ms to 26. Level 2 holds a
commit until the thread has filed it, and its burst cost the writer
plus the thread, 0.8 + 1.5 µs a write: at `γ = 2` it priced at
0.59x and 0.49x of the default at a hundred and three hundred
thousand keys on the buffered arm, while winning the sparse points
by 1.0-1.3x.

`Options::upkeep_lag_pct` holds a commit only while the thread trails by
more than a bound, and lets it go once the thread is within half of
it, and `Options::upkeep_batch_pct` keeps the thread from beginning a
pass for fewer writes, both shares of the store's keys. They did what they say and lost: a pass has a fixed
part `F` -- the log read, and the scan snapshot moved and every table's
bounds walked again at each state the thread looks at -- fitted at
about 0.76 ms beside 0.83 µs a write, so a bound that makes the thread
look often makes it slower than the writer, and no bound tried beat
the unbounded thread. The rule the model gives for the pass size is the
square-root one, `n* = √(N·F / (γ·p))`, about 6,500 writes for a
hundred-thousand-write burst, and with `F` as it is the bound cannot
be smaller than that. `F` itself is the next thing to cut.

Priced in one process, eight pairs a rung, burst plus twice the pass
summed over the lag points: `supdb-ingestbg` and `supdb-bg` (the lazy
freeze, the conversion gated on reads) against the default read 0.94,
1.05 and 1.17 on the buffered arm and 1.03, 1.19 and 1.00 on the
durable at ten, a hundred and three hundred thousand keys, the durable
burst at the largest point 1.19x and 1.14x (8/8) at the larger rungs
and nothing else held; `supdb-ingestbglag`, then with a bound of
sixteen thousand writes and a batch of eight thousand, read 0.98 and
0.89. The defaults stand.

#### The pass's fixed cost, and the bound priced again

The model's `F`, timed phase by phase on the upkeep thread over the
buffered arm's largest burst at a hundred thousand keys with the lag
bounded at sixteen thousand writes and the batch at eight: a pass of
about 5.3 ms was 4.2-4.9 ms of settle, 0.7-0.8 µs a write and
proportional, and 1.0-1.4 ms that was not -- the log read 0.15-0.25,
the snapshot moved 0.3-0.6, and 0.55-1.3 in the fill that completes the
tables, of which all but a few microseconds was the snapshot's bounds:
on every pass that replaced the snapshot, every table's block
boundaries walked against it (0.2 ms) and every key's cut in the
partition (0.5 ms, over about seven thousand keys), for the eight to
twenty blocks a burst had the thread build. The cuts are per block now
(`SnapBounds::cuts_of`): a block's keys cut inside its own ranks, so a
block's cuts are walked at its first build or walk from its first rank
and no other block's, behind a per-block flag the walker publishes
after them, the bounds being shared through the snapshot across
threads. A reader pays the same for the blocks it walks and nothing for
the rest; the thread pays for the blocks it builds.

Timing the bounded arm beside that found a second shape. One bounded
burst in four built the whole store -- 458-1,293 blocks against 8-34 --
in its pass, with every other count the same. The thread's pass skipped
itself whenever the state's generation had moved between the hand-over
and the pass, which a landing from the segment thread does every few
milliseconds in a dense burst, and reported the commit filed all the
same; a holding commit was released by those reports, the writer ran on
through its freezes, the thread's real look fell past the eight tables
the state keeps for it, and the writer's first scan dropped its tables.
A pass runs now against whatever state holds the table its three
bounds are of, since a landing moves the generation and not the table,
and reports filed only for a pass that ran (`upkeep_skipped` counts the
rest: none to one a burst).

With both, the bounded buffered arm's thread cost 0.70-0.99 µs a write
over the burst (median about 0.78, from 1.0-1.3 in the sweep before),
its burst 74-108 ms and its pass 4.5-10 ms, where the default's pass
is 24-38; eight runs of eight built 6-20 blocks. Priced in one process, eight pairs a rung, the decision quantity summed
over the lag points, lower better. `supdb-bg` against the default read
1.07, 1.07 and 0.92 on the buffered arm at ten, a hundred and three
hundred thousand keys, none resolved, and 0.99 and 0.82 (0/8, p=0.008)
on the durable arm at a hundred and three hundred thousand, its burst at
the largest point 1.21x (8/8). The bounded arm as typed counts, sixteen
thousand writes and eight thousand, read 1.12 (8/8) and 1.27 (7/8) at
ten and three hundred thousand keys buffered -- the count most of a
burst at one rung and a small share of the store at the other -- and
0.87 between them; as shares, a sixth of the store's keys for the bound
and a twelfth for the batch, 1.16, 1.03 and 1.07 buffered and 1.08,
0.93 and 0.98 durable, none resolved, its pass after the largest burst
2.4-2.8x the default's at a hundred thousand and its burst 0.67-0.81x.
The hold charges the writer what it saves the reads, one for one: what
is left in the pass is the settle of the writes the thread had not
reached, at the thread's own 0.7 µs a write, and the writer waiting for
the thread to reach them costs the same. The defaults stand; the
unbounded thread with the freeze that leaves its carry to the look is
the configuration to make the default once its buffered weighted sum
resolves, and the bound is not the lever.

#### The first quick row after the defaults moved

The row at b247414 was the first banked since f19abe5, seventy commits
before it, and its gate failed on eleven quantities with the host's
floors inside their bands and every comparator but RocksDB's bytes on
disk inside its own. Three were lag passes -- the tenth point at ten
thousand keys on two arms and at a hundred thousand on one -- at a
third of the window's rate: the never-hold default's trade (above, the
tenth point's pass at 0.34-0.66x under lend) against a window whose
every row held. The row's own bursts, which no row before it recorded,
put the default's burst plus twice its pass at 2.0 ms against LMDB's
4.1 at that point and 10 against 39 at the hundredth, at ten thousand
keys; 60 against 600 and 690 against 5,460 at three hundred thousand.
Eight were point reads: the default and the uncached arm at three
hundred thousand keys below every row, the p99 of five arms, and the
ratio to LMDB at 1.38 and 1.18 where the window held 1.76-2.13 and
1.40-1.93.

Three measurements read the eight. The store a point read meets after
the load is clean at every rung -- one partition, or two at three
hundred thousand, no piece and no unsealed key (`read_only`, which
loads, syncs and reads the pass in ten windows beside the store's
layout) -- and the default read 1.04x of the arm that runs its segment
work inline, 4 of 6 pairs, so nothing in flight after the sync is what
the pass paid. The suite's switch to consuming every value (b0cb464,
after f19abe5) prices the ratio from about 2x to 1.6x by its own
measurement. The rest is the host. The probe's fresh pass at a hundred
thousand keys read 2.8-3.8 million a second in one run and 2.2-3.0 in
another twenty minutes later, LMDB's 1.6-2.3 in both, with no fault, no
system time and no involuntary switch in either, and stores read in one
process spread 2.2-3.8 and 1.5-2.3 rep to rep; a second pass after a
second's pause read slower than the first in nine runs of nine and in
none of twelve more with the same pause, a spin or nothing between.
Alternated against the binary at 343a01d, the last whose point reads
were checked that way, four rounds with the order swapped and each
binary's default against its LMDB in one process: at a hundred thousand
keys 2.1-2.4 million reads a second on both with the ratio 1.10-1.33 on
both, which is the row's 1.18; at three hundred thousand 1.80-1.99 on
the old binary and 1.73-1.82 on this one, 0.91-0.97x in all four rounds
with the p99 0.89-0.94 against 0.94-0.98 µs, a margin one sitting has
seen and a second has not. The row is banked as the host it ran on, and
its point reads are what both binaries read there.

#### Where the reads lose now: the pass after a dense burst is a build

Ranked against their comparators in the row at b247414, the ten worst
read quantities are one shape: the scan pass after a write burst on the
buffered arm, the fully rewritten point at 0.17x of LMDB at a hundred
thousand keys and 0.22x at three hundred thousand, the tenth at
0.37-0.78x; nothing else on the read side is below LMDB. On the burst
plus twice the pass the arm is ahead at every point, its bursts three
to four times LMDB's, and the pass alone is five to six times slower
than LMDB's, which is what a reader meets. `lag_only` over the fully
rewritten point at a hundred thousand keys, three reps, the burst and
pass in milliseconds beside the thread's time and the blocks the pass
built: 94 and 24 with 55 ms of thread and 2,340 built; 70 and 32 with 26
and 1,536; 47 and 17 with 5 and 766; nine pieces and one partition at
the end of every pass, the partition merge of about 60 ms not landed.
LMDB's pass there is 3.6 ms, its burst 201. The pass rebuilds the
store's blocks at 15-25 µs each over nine pieces where a built block
walks in 2.5, and the thread's filing during the burst was dropped at
the settling freezes or rebuilt by the pass anyway: the rep whose thread
filed most ran its burst twice as slow and built the most blocks after.

The model reads it. A block here takes sixty to ninety writes before its
next read, past `k* = B/p` of about forty, so patching it per write is
the wrong strategy for this burst and rebuilding it once is right; what
is missing is the switch on density and the timing, a rebuild made by
the background after each landing and ahead of the readers rather than
by the first scan. The per-block rent-or-buy at the settle and the
builder-ahead thread exist; the policy that joins them does not.

#### The dense-burst regime: the lazy freeze stays, the bound and the cap do not

The policy was built as levers, each behind an option with an arm
keeping the other shape, and priced lever by lever in one process:
eight pairs a rung at a hundred and three hundred thousand keys, on the
buffered arm and the durable one, a lag point read as its burst plus
twice its pass (`weighted_ms`, lower better) beside the two rates it is
made of. The probe (`lag_only`) read the same bursts with the store's
counters, the first scan timed apart from the pass and its phases apart
from each other (`scan_take_us` and kin).

**The freeze that leaves its carry to the look is the default**
(`Options::freeze_settles` off; `supdb-settlefreeze` and
`supdb-ingestsettlefreeze` keep the settling one). Through the buffered
arm's fully rewritten burst at a hundred thousand keys the settling
freeze found a landing between its prepare and its swap at twelve to
sixteen freezes a burst and dropped the writer's tables each time, so
every form the thread had filed went with them and the pass after built
the store; and its wait for the pass in flight was the last place the
writer waited on work done for the reads. Left to the look, no table
was dropped in any rep. Priced, the settling freeze read the sweep's
weighted sum at 1.09x on the buffered arm at a hundred thousand keys
(6/8) and its ycsb-F at 0.83x (0/8, the one quantity under Holm), level
at three hundred thousand; on the durable arm level at a hundred
thousand and 1.11x at the fully rewritten point at three hundred
thousand (7/8), where its pass ran 1.65x faster (8/8) and its burst
0.88x (0/8): the backlog filed at the freeze is the pass's gain and the
burst's cost, and at `γ = 2` the cost is the larger.

**The bounded pass lost** (`Options::upkeep_pass_pct`, off;
`supdb-shortpass` and `supdb-ingestshortpass` keep the two-percent
bound). The first scan after the buffered burst waited 13.7 ms for the
thread's pass in flight in one run of two, most of its 17.9 ms, and a
reader waits for at most one pass, so the pass was bounded at two
percent of the store's keys, re-posting the rest. The wait fell to 1-5
ms and the same writes moved into the first scan's own settle, 27 ms at
a hundred thousand keys and 86-102 at three hundred thousand: the
thread's deficit over the burst lands on the first read whichever of
the two files it, and a bound only chooses which. Priced, the bounded
pass read the fully rewritten point at 1.07x and 1.23x of the unbounded
one's on the buffered arm (5/8 each) and level on the durable arm. Its
first version took the bound from the commit's generation against the
position the pass had read to, which a landing between the two made
zero, and re-posted an empty pass seventy thousand times in one burst;
the bound is applied in the log read and the frozen settle, where the
position is known.

**The conversions: a cap lost, and a yield won where the thread runs
behind.** A sparse form grown to sixteen deltas is rebuilt as a copy at
the next fill, and a fill on the upkeep thread that rebuilt every dense
form of a burst ran ten to fifteen milliseconds, which the writer's
first scan waited out. Two answers were built: a cap on the conversions
a fill makes, with the builder beside a reader's first scan converting
the rest (`Options::forms_convert_cap`, `Options::ahead_converts`;
`supdb-convertcap` and `supdb-ingestconvertcap`), and a yield, the fill
leaving the rest of its conversions the moment a newer commit is posted
or the writer wants the upkeep back (`Options::forms_convert_yield`;
`supdb-noyield` and `supdb-ingestnoyield` keep the fill that does not).
Against the yield alone the cap read the pass after the fully rewritten
burst at 0.77-0.93x in all four cells and the weighted sum within 5%
either way; against the fill that neither caps nor yields it read the
weighted sum at 0.92x and 0.89x on the buffered arm (6/8, 7/8) and
1.13x on the durable arm at three hundred thousand keys (8/8, under
Holm), its pass there at a fifth of the rate: a dense form the cap
leaves sparse is patched on until its replaced runs outweigh the live
ones, then dropped and built fresh -- twice the fresh builds, the
thread's last pass forty milliseconds longer, and the first scan waiting
out all of it. So the cap is off. The yield alone, against the fill
that never yields, read 1.13x on the buffered arm at a hundred thousand
keys (8/8, under Holm) and 1.03x at three hundred thousand, level on
the durable arm at a hundred thousand and 0.94x at three hundred
thousand (7/8), where the pass ran 1.6x faster with every dense form
converted through the burst. The yield pays where the thread runs behind
the writer and costs where it has slack; a yield only while the thread
is behind would take both, and waits on the thread's speed.

**What is left is the thread's speed.** With those settled, the buffered
arm's pass after the fully rewritten burst is one scan long: through the
probe at a hundred thousand keys the writer's first scan took 10-29 ms
of a pass of 17-36, 1-5 ms of it waiting for the thread's pass in flight
and the rest settling what the thread had not reached, and the 999
scans after it ran at 5-6 µs each, the drained rate; at three hundred
thousand the first scan was 98-112 ms of 118-132. The durable arm's
pass runs at the drained rate throughout. The thread filed at 0.66-0.99
µs a write against the writer's 0.65-0.97 and was busy for the whole
burst -- 59 ms of maintenance over a burst of 59 -- and still ended
behind by what the first scan then settled. Profiled through the burst
(perf's software clock, the samples split by thread id, since `perf
report --comm` had not filtered them), the thread's 161 ms over 222
thousand writes was: the settle loop's own 20%, the resolve of a key to
its block 23% (`owner_of` and its compares, a cold binary search a key
in arrival order), the splice 27% (`patch_block`, its moves and the
allocator), the sorts 5%, task switches 3%.

The resolve runs in key order now: a settle reads its batch's keys'
first sixteen bytes in the log's order, which is the arena's, sorts the
batch by them stably with the radix the snapshot uses, and resolves each
key with a seek galloped from the rank the key before it resolved to
(`OrdIndex::seek_exact_from`), over heads the last seek left warm, where
the seek from the top read a cold stride of heads a key. Alternating the
probe's two binaries four rounds, the order swapped each round, on the
buffered arm's fully rewritten burst: the pass after it read 0.68x at a
hundred thousand keys (7/8 pairs) and 0.84x at three hundred thousand
(4/4), its first scan 7 ms against 12 and 23 against 33, the settle
inside that scan 2 ms against 5 and 6 against 20; the burst did not
move, the burst plus twice the pass read 0.89x and 0.94x; the tenth
point read 0.87x at a hundred thousand and level at three hundred
thousand; and the durable arm's fully rewritten point read 0.92x (4/4).
What the first scan waits for now is the thread's pass in flight, 5-29
ms at three hundred thousand keys.

That wait was read three ways. The thread's fills are not in it: over
the fully rewritten burst the fill converts a few hundred dense forms at
most and builds under a dozen blocks fresh -- the thousands of fresh
builds the suite's counters show for a rep are the update mixes'. The
bounded pass, re-priced once the writer settled in key order, loses by
more than before: 1.17x and 1.05x at a hundred and three hundred
thousand keys on the buffered arm (7/8, 5/8) and 1.09x on the durable
arm at a hundred thousand (6/8), because the thread's small passes each
pay the pass's fixed cost, so it ends the burst further behind, and the
writer settles the leftover at the thread's own rate (`scan_settle_us`
up 3.1x where `upkeep_ms` fell to 0.9x) -- moving the work moves
nothing. And the thread, which runs in the idle scheduling class, is on
a core for 94-99% of its pass time (`upkeep_cpu_us` beside
`upkeep_ms`, on both arms at both rungs), so the scheduler is not the
gap either. What is left is that the thread's CPU per write equals the
writer's -- 0.5-0.85 µs each in one run of the buffered arm, the thread
within a tenth of the writer either way -- so it ends every dense burst
about a pass behind. Profiled after the resolve change, the thread's
time is the splice 34% (`patch_block`, the sparse form's sorted inserts
and their moves, the allocator), the settle loop's own 15%, key
compares 7%, the sorts 6%, the chain's tombstone check 4%: a few
percent each. The structural question is whether the sparse form a
clean block's first write makes is worth making at all for a burst of
uniformly random keys -- it walks at about 2.3 µs against a copy's
1.05, and the walk through the snapshot's run, which carries the values
in key order, is the comparison not yet measured. A settle that counted
a clean block's writes and built the copy at the density where one pays,
instead of patching a sparse form sixteen times and converting it,
would halve the thread's work on this burst and leave the pass walking
copies.

Measured, in a scratch build that gave a block its first write as an
empty wide form kept across every publish, so that its writes were
never patched and every walk of it folded its sources -- the partition,
the pieces and the snapshot's run -- as the open block's walk does:
alternated against the sparse forms three rounds, the thread's cost a
write did halve (0.27-0.45 µs against 0.58-0.66 across the points) and
the first scan's wait went (0.5 ms against 2.9 at the tenth point at a
hundred thousand keys, 2.6 against 9.2 at three hundred thousand); at
the tenth point, over a store with no piece standing, the pass read
level and the burst plus twice the pass 0.89x and 0.95x. At the fully
rewritten point the pass ran 3x longer, 23 ms against 8 at a hundred
thousand keys and 100 against 35 at three hundred thousand, the sum
1.19x and 1.38x. A walk there folds the eight or nine pieces the burst
sealed, their keys over the block read from each piece's records, and
the sparse form is the cache of that fold -- not of the memtable's
values, as its doc has it, which the snapshot carries in key order
anyway -- so a block of the rewritten store costs 8-12 µs a walk
without one against 2.3 with. The patches are how that cache is kept
without ever reading a piece: a key's chain holds a tombstone, which
masks every older source, so the patch copies the run from the chain.
What a build at the burst's end saves in patches it pays in piece reads.
The regime stands; the thread's cost a write is the lever that remains,
and a block with few overlay keys and no piece under it is the one
place the walk without a form wins.

#### The forms hold the pieces' fold alone, and the walk lays the snapshot over them

The scratch build above priced the shape at its worst, a block without
a form walked from its sources at every read. The arm built from it
keeps the form and changes what it holds (`Options::forms_pieces_only`;
`supdb-piecesonly`, `supdb-ingestpiecesonly`): a block's form is the
fold of the partition and the level-0 pieces, built once for a set of
pieces and never patched, and the memtables' keys reach a scan through
the scan snapshot, whose positions over the block the table has already
-- the main run's bounds, the bases' and the keys filed since -- laid
over whatever form the block holds by the walk itself: the partition's
records for a clean block, the deltas for a sparse one, the copy for a
dense one, a merge of two sorted streams where a key both hold is
emitted once (`emit_both`: the form's run unless a memtable tombstone
masks every source older than it). The settle resolves and files only
the keys created since the snapshot's run was built, for their cuts,
and splices nothing; a key written again reaches the walk through the
stale set as it did. A landing is then the one publish that changes a
block's fold, and the rebase drops the forms of the blocks the landed
piece has keys over, marks the table incomplete for them whether they
had a form or not, and leaves them to the fills, which build them from
the pieces again; the published copies carry a marker in their place.
A copy carried across a partition's rewrite survives only when the
rewrite was the one publish since the writer's last look: with more, a
piece may have landed and been merged away between the two, which no
piece set shows, and the copy that never folded it would stand as the
rewritten partition's.

Alternated as two arms of one probe binary, two rounds of three reps
with the order swapped, the probe's lag sweep at a hundred and three
hundred thousand keys on both arms, with the upkeep's cost a write
(`c_t`), the pass, the first scan's wait on the thread and the blocks
built read beside the rates. Where no piece lands the thread's cost did
what the shape says: at the ten-percent point `c_t` read 0.26-0.48 µs
against 0.75-1.02 on the durable arm at a hundred thousand keys and
0.28-0.40 against 0.80-1.16 at three hundred thousand, 0.24-0.31
against 0.57-1.07 and 0.31-0.43 against 0.54-0.77 on the buffered arm,
and the first scan's wait on the thread went with it -- 1.7-3.3 ms
against 9.5-14.1 on the buffered arm at three hundred thousand keys,
0.4-0.7 against 1.9-5.6 at a hundred thousand -- the burst a little
faster on every cell. But the pass after it runs slower: 4.9-8.2 ms
against 2.6-4.5 and 22-34 against 15-24 on the durable arm, 5.8-8.6
against 4.2-8.0 and 23-32 against 23-28 on the buffered, since every
scan of a block with memtable keys over it merges the snapshot's
window into the walk -- the window found, its keys and cuts gathered
into an overlay, each key's tombstone asked of the stale set and the
run -- where the sparse form the default patches is that merge done
once, read as one array with the values inline. Burst plus twice the
pass at that point: 18-29 against 17-24 and 75-102 against 63-88 on
the durable arm, 15-24 against 13-23 and 58-76 against 62-70 on the
buffered; level to worse. Where pieces land, every landing drops the
forms of every block the piece has keys over, which over a rewritten
store is every block, and the thread builds them all again: 11-14
thousand builds a burst against 1.5-1.7 thousand on the durable arm at
a hundred thousand keys, 37-41 against 3-5 thousand at three hundred
thousand, `c_t` above the default's (1.5-1.9 against 1.1-1.6), the
first scan waiting out the rebuilding pass, and the pass after the
fully rewritten burst 10-42 ms against 4-6 and 40-69 against 9-14 on
the two arms at a hundred thousand keys, 79-134 against 35-82 and
97-215 against 33-48 at three hundred thousand. Paired in one process
over the suite's workloads at a hundred thousand keys on the durable
arm, six pairs: ycsb-E read 0.45x (0/6), the writer's scans waiting
out the thread's rebuilding passes at every take-back (18.5 ms of
waits a rep against 2.2), the threaded scan mix 0.79x and 0.74x, the
lag points 0.79x, 0.47x and 0.12x and their weighted sum 1.40x (6/6),
the thread building 3.6x the blocks; the load, the point reads and the
plain scan level. The arm stands for pricing and is not the default.

Two of its costs went next. The landing folds now: where the partition
stands, a landed piece's run over each block it has keys in is merged
into the block's standing form in one pass over the piece's records for
the block (`fold_piece_block`), its tombstones masking the form's run
for the key and its values following it otherwise, pieces oldest first
-- a landed piece is newer than every piece a form holds -- with the
fill left only the blocks a table short of complete had no form for;
a rewritten partition keeps the drop. And the stale set, which every
overlaid key's emit asked twice, is a bitset over the live table's
slots in place of a hash set that after a burst held every key written
(`SlotSet`), and the walk's overlay fetches its keys' chains ahead as a
build's does (`prefetch_overlay`). Alternated as before, two rounds of
three, the thread's cost a write is now below the default's at every
point: at the hundred-percent point 0.66-1.02 µs against 1.21-1.54 on
the durable arm at a hundred thousand keys and 0.88-1.13 against
1.22-1.47 at three hundred thousand, 0.53-0.78 against 0.74-1.04 and
0.62-0.81 against 0.80-1.09 on the buffered arm, its work over the
burst 59-91 ms against 109-139 and 237-306 against 328-396 on the
durable arm, the builds a burst back to the default's count. Burst plus
twice the pass reads level at the one- and ten-percent points on both
arms at both rungs -- 18.6-23.0 against 19.1-22.7 and 70.9-94.3
against 62.2-77.4 on the durable arm at ten percent, 15.5-21.9 against
15.6-30.7 and 54.9-79.1 against 57.6-89.0 on the buffered -- and at the
hundred-percent point 166-201 against 176-222 and 559-612 against
504-691 on the durable arm, 86-135 against 81-140 and 384-424 against
303-401 on the buffered. The pass alone still trails, 1.3-2x at the
hundred-percent points, and both arms walk sparse forms there at three
hundred thousand keys on the buffered arm (7,616 walks against 7,151),
so the difference is the overlay itself: after a burst every memtable
key the walk lays over a block was written since the snapshot's runs
were copied and is read from its chain, two or three dependent misses a
key behind the prefetch, where the default's patched form holds the
value inline. That was the reading, and it was priced: the snapshot
carries no runs by default (`Options::snapshot_runs`, the `supdb-runs`
arm), so every overlaid key was read from its chain stale or not, and
with the runs on, the upkeep republishing the snapshot with its stale
runs copied again (`Reader::refresh_runs`, once a sixteenth of them are)
and every handle switching to the version with its filed keys kept, the
pass did not move at any point -- 4.9-7.3 ms against 4.8-5.5 at the
ten-percent point on the durable arm at a hundred thousand keys and
21-30 against 21-29 at three hundred thousand, 45-104 against 40-73 at
the hundred-percent point there -- while the thread paid the copies,
0.42-0.71 µs a write against 0.30-0.43 at ten percent. The arms carry
neither; the refresh stays as the mechanism that keeps the runs current
where a store asks for them. What the pass pays is not the chain but
the overlay: an `Over` gathered per key per block, and the emit's walk
over the sources for each, where the sparse form is one array with the
values inline. The step that remains is a walk that merges the
snapshot's window in place, without the gathering, and until it is
taken the arm reads level on burst plus twice the pass at the one- and
ten-percent points and trails by up to a third at the hundred-percent
ones.

#### The buffered lag points are one scan, and that scan is the thread's lag

The buffered arm's scan pass after a burst, in one process against
`lmdb-nosync`, read 0.33-0.36x at the fully rewritten point and
0.56-0.68x at the tenth, at a hundred and three hundred thousand keys,
the only significant losses on the board. Decomposed with the lag probe
in the suite's shape: the pass is one slow scan and then scans within
0-15% of LMDB's rate. At three hundred thousand keys the fully
rewritten pass was 26-54 ms with a first scan of 8-37 and the rest at
16-19 against LMDB's whole pass of 16; the tenth was 20-30 with a first
scan of 9-19 and the rest at 11.5 against 14. The first scan is the
wait for the upkeep thread's pass in flight plus the settle of what the
writer logged during it, about twice the size of that pass; and the
pass is large because the thread's cost a write is the writer's --
0.5-0.9 µs against 0.4-0.9 across the points -- so a pass covers what
was written during the one before and never shrinks. Nine pieces stand
at every pass's end, and the steady scans over the sparse forms and
copies are near LMDB's, so the pieces are not the cost there.

The thread's cost a write was then priced by instruction count under
callgrind, perf's software clock having spread it over a dozen leaves:
2,660 instructions a settled write at a hundred thousand keys, more
than half in the patch (the chain walked once for the tombstone and
again for the values, the key search through `memcmp`, a sum over every
entry for the bloat check), a seventh in the resolve, a tenth in the
loop itself and a fifth in the allocator. Three rounds of local cuts followed, each
measured against the head's binary alternated two rounds of three reps:
the first, which also narrowed the form prefetch to its entries and keys
and grew a full form one delta at a time past the dense count, read
worse at the fully rewritten point (a miss a splice into the values, a
reallocation a write on the forms that burst makes); the second and
third took the instructions to 2,056, 23% fewer with 17 of them the
allocator's, its share of the thread's samples from a fifth to a
fourteenth, and the thread's cost a write a fifth lower at the tenth
point at a hundred thousand keys in two sittings -- and the
pass and its first scan level everywhere within a rep spread that is a
factor of two for one build on this host. A change the thread's lag
would need is a third of its cost a write, and the cuts available
locally are each two to seven percent. The lever is parked.

What would move the cell is not a cheaper write but a settle that runs
beside the writer's scans instead of before them: the forms' upkeep is
one object with one holder, so a scan takes the whole of it back and
waits for a pass over every block to finish before it walks three.
Owned by shards of blocks, a scan would take back the shards it walks
and wait for a pass over those alone, and the thread would keep filing
the rest; or two filing threads over disjoint halves would halve the
lag at a core's price. Either is a redesign of the upkeep's ownership
and is its own move.

#### The pass after the rewritten burst: a rebuild deferred to the first reader, and the thread's slack

The durable arm's scan pass after the fully rewritten burst at three
hundred thousand keys read 0.66x of LMDB's in one sitting (25 ms against
17) and 0.95x in another, and the move that was to take it had been
contracted on the copy walk's rate -- copies at 19 million entries a
second where the partition's records walk at 48. The profile that was
its first step said otherwise. The pass's window, cut from a profile of
the suite's own process by the monotonic clock (`SUPDB_LAG_MARK`), was
mostly other threads: the burst's last seal writing its segment, a
partition merge, the landing taking its ranks, and the upkeep thread
building copies; and the profile distorted the pass itself five-fold,
since every one of those threads is sampled on four cores. So the probe
was given the scanning thread's CPU clock beside its wall clock, and read
the pass nine times in the suite's shape.

Three shapes. Four reps at 13-17 ms with the CPU equal to the wall, the
first scan 3-5 ms of settle and every block walked as a copy -- the
pass's own cost, and 4-5 ms of it over a clean pass's 8.5-10 is the copy
walk. Four reps at 36-66 ms with 11-24 of CPU: the first scan was 25-56
ms of it, all of that `scan_take_us`, the writer waiting for the upkeep
thread's pass in flight. Two reps at 21-37 ms, CPU-bound, the scans
walking sparse forms where the others walked copies. The counts named the
pass the first scan waited for: about 4,700 blocks built fresh by the
thread in the rewritten point (`fill_fresh`), 5-12 µs each, in one pass.
The partition merge the burst triggers lands near the burst's end; its
landing's rebase drops every sparse form over the rewritten partition,
whose cuts are ranks into records the merge replaced, and the fill after
it builds a copy for every overlaid empty slot -- a build the yield does
not cover, since the yield was written for conversions. Where the merge
landed early enough the thread finished before the first scan; where it
landed at the burst's end the first scan waited the whole pass out; where
it landed later still the scans walked what stood. The thread had been
on a core for about half the burst.

The conversions that would have made those blocks copies during the
burst never ran, because the fill yields them at any posted commit, and
through a burst a commit is always posted; the entry on the dense-burst
regime had written that a yield only while the thread is behind would
take both the gain and the cost, and the thread then had no slack. It
has slack on the durable arm now. `Options::forms_convert_behind` is the
allowance: a posted commit yields the fill's conversions only once the
thread is behind by more than that many committed writes it has not
read, a batch by default; the writer's own wait yields as before.
`supdb-postedyield` and `supdb-ingestpostedyield` keep the yield at any
posted commit.

Alternated as two probe binaries, three rounds of two reps on the durable
arm at three hundred thousand keys, the rewritten point's pass read
13-17.5 ms in every rep against 12-117 before, its first scan 1.8-5.1 ms
with a wait of 0.1-3.6 against waits of up to 83, and the thread
converted 4,687 blocks through the burst and built 4-11 fresh after it
where it had converted 40-1,006 and built 3,700-4,650; the burst did not
move, and the thread's time over the point was level over fewer, longer
passes. The suite's own pair, six pairs at three hundred thousand keys:
the rewritten point 1.19x faster than the old yield on the durable arm
and 1.42x on the buffered, the buffered point's burst plus twice its
pass 0.97x, and against LMDB the point read 1.19x where the morning's
sitting had read 0.66x. On the replaced host, six pairs: the durable
arm's rewritten point 1.23x of LMDB at three hundred thousand keys and
0.86x at a hundred thousand (3/6), its clean scan 1.10x; the buffered
arm's rewritten point 0.47x and 0.55x of `lmdb-nosync` there and its
tenth 0.65x and 0.60x, the cell the parked move on the settle beside the
scans owns, and on this host the largest loss on the board again. Twelve pairs, on a
faster host after the machine was replaced under the session: on the
buffered arm at three hundred thousand keys the point's burst plus twice
its pass 1.09x better than the old yield (9/12) and the scan mix
(ycsb-E) 1.10x (11/12, under Holm), the update mixes level; at a hundred
thousand keys the buffered sums 1.05-1.08x better (9/12) with one cost,
the read-mostly mix (ycsb-B) 2% slower (11/12, under Holm), the thread's
conversions running beside reads that never scan; on the durable arm at
a hundred thousand the sums 1.15x better (9/12) and everything else
level.

The copy walk's excess, taken apart. The probe's rewritten point with
the thread's passes done and the forms as they stand (`LAG_WAIT_MS`),
alternated in rotated rounds on the durable arm at three hundred
thousand keys, per scan with the first scan taken out: a copy walk
2.6-2.9 µs against a clean walk's 2.0-2.3. The first lever
pre-registered for it was the copy's prefetch -- the forty-eight lines
of a copy's three buffers, issued right before the search that needs
them, a fifth of the pass's samples on the prefetch instructions
themselves -- and it is refuted: with no prefetch the walk reads 4.0 µs
a scan, with the current block's prefetch cut or trimmed to its entries
and keys 2.7-2.9, with the next block's lines dripped through the
current block's walk 2.8-3.0, each over six to eighteen passes. The
prefetch pays through the next block, issued a block ahead, and the
samples on the instructions are where the memory wait lands, not a cost
a shape removes. The settle's prefetch of the next group's block, the
hottest instructions in `settle_each` by the same instrument, answers
the same way: entries and keys alone 0.45-0.54 µs a write on the first
scan against 0.43-0.71 whole, and none 0.47-0.59.

The second component is the first scan's. In the suite's shape the
pass's first scan is 1.7 ms of 9.6-10 at three hundred thousand keys
and 0.6 of 2.3-2.9 at a hundred thousand, and the trace says what it
does: the thread ran a pass for each of the burst's last commits and
each pass filed nothing, since `settle_due` declines a backlog under
`Options::forms_settle_backlog_pct` with no scan recent, so the burst's
tail -- two to three thousand writes at three hundred thousand keys --
waits for the first scan, which reads and files it at 0.45-0.55 µs a
write where the thread would have at 0.50-0.56. The thread's
one-millisecond fallback for a stopped writer runs the pass and the due
rule skips it. The option's own entry says the bound decides who files
a burst's tail; the first read after a burst is who.

The third is the copy's own search. The partition's seek has found the
cursor's rank, and the walk then searched the copy's entries for the
cursor again, six dependent misses into two buffers the prefetch above
exists to warm. After a burst of updates every copy's entries are the
block's records and nothing else -- an update patches an entry in place
and inserts none -- and the copy knows it (`CachedBlock::identity`,
set by the build over updates at known cuts, cleared by an insert for
good), so the walk starts at the seek's rank less the block's first
(`Options::copy_start_at_rank`; `supdb-copysearch` searches). On the
probe, eighteen passes a shape over nine rotated rounds, the start at
the rank read 2.90 µs a scan against the search's 2.98 with the first
block's prefetch kept and 2.83 against 3.23 without it, faster in six
and eight rounds of nine by each round's best pass: about a twentieth
of the walk, not the tenth predicted.
The suite's own pair, twelve pairs on the durable arm: at a hundred
thousand keys the search reads the rewritten point 0.91x of the start at
the rank (2/12 above it, p=0.039), at three hundred thousand the point
is within noise (6/12), and nothing else moves at either size, the mixes
and the clean scans included. The start at the rank is the default.

The bound itself, priced as the suite's own pair at one percent
(`BACKLOG=1`, `supdb-eager`), twelve pairs: at a hundred thousand keys
the tenth-rewritten point 1.17x (10/12, p=0.039) and the rewritten point
1.06x (7/12); at three hundred thousand both within noise (1.04x, 9/12);
the update mixes 0.87-0.94x in four to seven pairs of twelve at either
size, the trade the option's entry describes and not one the pairs
resolve. The bound stays at two.

What remains of the cell: the burst's tail, 1.7 ms of the pass at three
hundred thousand keys and 0.6 of 2.3-2.9 at a hundred thousand, which
the bound only moves between the commits and the first read, and a
filing by block at the walk, or a cheaper filing, would take; the copy
walk's half a microsecond a scan over the clean walk, which is the
copy's three buffers against the partition's one record stream; and the
single-thread scan on the ordered store, 0.68-0.88x of LMDB's cursor at
three hundred thousand keys, which is the clean record walk and not the
forms.

#### The first scan after the buffered burst: the thread's lag, and what the settle's searches were worth

The buffered arm's rewritten and tenth-rewritten points read 0.47x and
0.65x of `lmdb-nosync` at three hundred thousand keys, and the probe in
the suite's shape puts the whole of both losses in the pass's first
scan: at three hundred thousand keys the tenth-rewritten pass is 11-15
ms with its first scan 6-10, nearly all of it the take-back's wait for
the thread's pass in flight, and the rewritten pass 14-19 ms with its
first scan 6-12, split between that wait, the tail's settle and one
snapshot rebuild; at a hundred thousand the first scan is 1-2 ms of a
2.7-3.7 ms pass and 1-4 of 3.2-6.1. Without the first scan both passes
sit at the comparator's 7.6-7.7 ms or under it.

What the first scan waits for is the thread's lag. Traced pass by pass,
the thread ends the burst 10-40 thousand writes behind the writer in
one rep and 90-160 thousand in the next, in passes of 4-8 ms when close
and 26-71 ms when far, and the first scan pays the pass in flight and
then files what is left: 1.3 ms in the close reps, 16-20 in the far
ones. The thread's time over a three hundred thousand key burst is
150-210 ms against a burst of 150-200: its cost a write and the
writer's are the same number, so which side of the burst's end it lands
on is scheduling, and the pass is bimodal because of it. The thread's
time, cut to the burst window and the thread, is spread thin: the
settle's own loop 13-21%, memory moves 6-12%, the resolve 5-7%, the
sorts and the snapshot's extension a few percent each, and 7-47% in the
kernel zeroing pages. The probe counts the thread's minor faults now
(`flt`), from `/proc/self/task`: 10-15 thousand a burst at three
hundred thousand keys, of which about ten thousand are pages the
allocator gave back at a pass's end and faulted in again on the next,
measured by running the same binary with glibc's trimming turned off
through its tunables, where a process's second burst faults 0.5-4
thousand and its first 14-15 thousand either way. That is 5-10% of the
thread, not the half one rep's profile showed; a library does not set
the process's allocator, and keeping the settle's three arrays across
passes -- seventy-six bytes a write, the largest per-pass allocation --
moved neither the thread's time nor the passes in two alternations of
eight, so they are allocated as they were.

Two cuts stand, both the mechanism the copy walk's start already uses.
A patch found its entry in a copy by a binary search over the copy's
keys, and in a sparse form by one over its keys through a second
buffer, while the resolve had just computed the key's cut: the copy's
entry is the cut less the block's first rank where the copy holds the
block's records and nothing else (`CachedBlock::identity`), and a
sparse form's entries sort by cut, then the inserts below a record
before the record's own update, then by key among the inserts at one
cut, so the search probes the entries alone and reads a key only among
the inserts at the key's cut (`SparseBlock::find_at_cut`). Both are
under `Options::copy_start_at_rank`; `supdb-copysearch` and
`supdb-ingestcopysearch` search. Twelve pairs each: the copy's patch by
rank reads the thread's time 0.92x of the search's on the buffered arm
at a hundred thousand keys (10/12, p=0.039) and the lag sum 0.93x
(10/12, p=0.039), level at three hundred thousand where the burst's
patches land on sparse forms, the thread being too far behind to
convert; the sparse form's find by cut reads the buffered
tenth-rewritten point 1.08x at three hundred thousand keys (11/12,
p=0.006), the scan mix 1.05x (10/12, p=0.039), the thread's time 0.96x
on the durable arm (10/12, p=0.039), and the buffered rewritten point
1.3-1.4x by median at both sizes in nine pairs of twelve, which its
bimodality keeps from significance. Nothing read against either.

Refuted, and kept as an arm: dropping a block from six keys of it in a
batch and building it once (`forms_settle_rebuild_from`,
`supdb-ingestrebuild`), on the reasoning that the buffered passes
carry tens of a block's keys where the bound was priced against two.
Twelve pairs: the rewritten point 0.16x and 0.20x at three hundred and
a hundred thousand keys (0/12), the thread's time 1.27x, twice the
blocks built, the lag sum 1.7-1.9x worse. The blocks dropped are the
ones the first scan then builds.

What remains of the cell is the thread's cost a write against the
writer's, which no search or allocation decides: a settle resolves a
key's rank and splices its run, about half a microsecond, and the
buffered writer commits one in the same time. The first scan after a
burst inherits the difference, and the shapes that would change it --
a filing by block at the walk, forms that hold the pieces' fold alone
with the memtable overlaid at the walk (`forms_pieces_only`), or a
pass whose take-back cuts a settle short -- each move the work rather
than remove it for a pass that reads every block once.

#### The piece's ranks over the heads, and a locked exchange to look at a form

Every written key's rank in its partition is computed by three parties:
the settle resolves it on the upkeep thread, galloping over the ordered
index's heads from the rank the key before reached; the segment work
ranks a piece's keys against the partition before the publish, for the
merges and the reads; and the scan snapshot takes the same cuts lazily
per block. The second was done over the partition's records -- a gallop
and a binary search whose every probe read a record, a cold line each
-- and was two fifths of the segment work's thread's time over the
buffered arm's rewritten burst, by a profile cut to the burst's window
and that thread. It ranks over the heads now, as the settle does
(`Options::piece_ranks_by_heads`; `supdb-rankrecords` and
`supdb-ingestrankrecords` rank over the records), and the checked
profile asserts the two rankings agree on every piece. The suite reads
the segment work's thread's CPU now (`maint_cpu_us`). Twelve pairs: the
records' ranking costs that thread 1.62x the heads' on the buffered arm
at three hundred thousand keys (12/12, p<0.001), 1.40x at a hundred
thousand (11/12, p=0.006) and 1.85x on the durable arm at three hundred
thousand (12/12); the upkeep thread's time and every pass and mix are
within noise at twelve pairs. What the cut buys the first scan after a
burst is a core the upkeep thread no longer shares for that time; the
pairs do not resolve it.

The scan mix's decomposition, at three hundred thousand keys on the
durable arm with the probe that runs the mix twice: the second pass's
counted phases -- take, sync, settle, snapshot, install -- are five
milliseconds of ninety to a hundred, the thread under a millisecond,
the blocks built under a hundred; the pass swings between 65 and 150 ms
across rounds with every counter identical, and under one profile
LMDB's pass swung the same way in the same rounds, the slow passes
carrying the same sample count as the fast ones: the thread lost the
CPU, not the work. The pass's own profile is the walks (a quarter in
the block walk, a tenth in the prefetch), the twenty-five durable
commits' fsyncs, the harness's sink and its Zipfian generator, and one
cost that was the engine's alone: `Arc::make_mut` at 3-4%, called on
every block a scan walked through the handle's own table to ask whether
the form was wide. `make_mut` proves the pointer unique with a locked
exchange on the weak count before it can look. The slot is looked at
through a shared borrow first now (`wide_mut`); five rounds of the
probe read the second pass 0.76 to 0.80 million operations a second by
median against the search's 0.76, 0.68 to 0.88 against 0.76 to 0.88,
within five rounds' noise and in the direction the removed work says.

#### The buffered lag point in the row's shape: the thread's conversions and the first scan's wait

The quick row read the buffered arm's fully rewritten lag point at
0.17-0.59x of LMDB at every rung while every probe built for it read
one shape or the other, so the row was given its own counters
(`SUPDB_LAG_COUNTERS=1`): the scans' phases, the threads' time and pass
count, the faults by thread, the blocks built and the walks by form,
read before each burst and after its pass and never between. In that
shape at three hundred thousand keys the pass after the rewritten
burst read 12-24 ms in some reps and 40-212 in others, and the one
count that split them was the upkeep thread's passes over the burst:
4-9 in the slow reps, 13-45 in the fast, with the thread's CPU the same
160-230 ms in both. Nothing else moved: the published forms were never
copied before a patch (`forms_cloned` read zero in every rep), free
memory was flat, and the allocator's tunables changed nothing, so the
predecessor arm and the balloon were ruled out before anything was
changed.

Two levers the suite already had were priced first. The bounded pass
(`supdb-ingestshortpass`, `upkeep_pass_pct` at 2%) read the point at
0.434x of the default in twelve pairs, none above: the cap moves the
backlog from the thread's pass to the first scan's own settle, which
read 28-37 ms in every rep. The arm that converts no dense form unless
a scan has come since the last fill (`supdb-ingestbg`) read it at
0.751x, two pairs of twelve above, and E at 0.925x, none above: the
pass walked sixty-four-delta sparse forms, and those cost more than the
copies even at 1.6 walks a block.

A trace with one clock across the runner and the engine then gave the
mechanism as a timeline. In a slow rep the thread's pass in the middle
of the burst converted 2,000-2,800 dense forms while 27,000-133,000
writes waited behind it, and the next pass did the same; the burst
ended with 130,000-186,000 writes unfiled, the thread's last pass filed
them, and the first scan waited 13-38 ms for that pass (`take_back`),
then ran its 13 ms of scans. In a fast rep the same conversions fell
earlier, the thread was 6,000 writes behind when the burst ended, and
the scan waited a millisecond. The conversions yielded to nothing while
the writer was not waiting: the yield on a posted commit
(`forms_convert_behind`) measures how far the thread is behind as the
log position it may read to less the position it has read, and on the
thread the first is the pass's own bound, which the log read has just
reached, so the measure is zero in every pass and only the writer's wait
(`LEND_WANT`) ever yielded -- 14-250 skipped conversions in the pass
the first scan took back, none before it. That measure is to be
changed and priced on its own.

The conversion itself was priced in one process before it was changed.
The trace had stamped it at 15-35 µs a block by wall clock; a counter on
the thread's own clock, reported by both arms (`Db::convert_us`), read
11.1 µs from the sources and 6.8 from the form, and the difference was
the thread off its core beside the writer and the seal. A dense sparse
form holds every key written over the block with its run resolved --
the partition's values for an equal key, then the pieces' and the
memtables', a tombstone masking the older -- and every settle keeps it
so, which makes the copy one streaming merge of the form with the
block's records by the cuts the form holds (`copy_from_deltas`), where
the build from the sources gathered every key's run again through a
seek of each piece and a chain walk in the memtable. In the checked
profile the conversion builds the copy both ways and asserts they
agree, entry for entry (`CachedBlock::agrees`). Priced at three hundred
thousand keys, twenty-four pairs each: the buffered point's scans read
1.105x with the form's conversion, nineteen pairs above (p .007), and
the conversions 1.70x cheaper in all but one; the durable arm's thread
spent 7.9% less (twenty pairs above, p .002) with its lag points level;
E, B, D and F level in both. A first pairing of the durable arm had
read its rewritten point 6% against the change in ten pairs of twelve
(p .039) and the replication read it 6% for, fifteen of twenty-four:
the pair's position effect is real and a verdict at p .04 on one
pairing is not one.

The yield's measure was then made what its doc said -- the writes the
writer has lent past the commit the pass brings the upkeep to -- and
priced against the behaviour the dead measure had given, yielding to the
writer's wait alone, at three hundred thousand keys in twenty-four pairs.
It lost everywhere it moved: the thread converted 1,657 of the burst's
forms where it had converted 4,913, the pass after it walked 20% more
sparse forms and 10% fewer copies, and the buffered point read 0.65x
(twenty-one pairs above for the want-only shape, p < .001); the durable
arm's thread spent 11% more, patching forms it had left sparse, with its
points level. In the row's shape the slow reps stayed, with the first
scan waiting 47-65 ms for a pass that was settling, not converting, and
the fast reps slowed to 27-30 ms. So the default yields to the writer's
wait alone, now said as an infinite allowance, and the yield a thousand
behind is the arm (`supdb-behindyield`, `supdb-ingestbehindyield`). The
measure that never fired had, in effect, chosen the better shape, and a
conversion made early is also the cheaper form to patch for the rest of
the burst.

What the change did not do: the slow mode is still there, one rep in
four in the row's shape, because the thread's budget is what it was.
The burst writes three hundred thousand keys in 180-200 ms and the
thread's work for it is the settles at about 0.4 µs a write, 120 ms,
the conversions, 33 ms now against 52, and the log read and snapshot,
about 20; the thread is on a core for nearly the whole burst, and any
slip puts the burst's end on the first scan. The settle's per-write cost was then counted by kind,
and it is not the slow mode: a trace of every patch by the form it met
read 0.36-0.43 µs a write in every window, the copy in place cheapest
(0.27-0.34) and the sparse insert dearest (0.36-0.45), while the slow
windows had settled 138,000-165,000 writes before the burst ended where
the fast ones settled 230,000-263,000, at the same cost each. The
thread was never idle and never off its core between passes, so the
difference was inside the passes, and a split by phase put it in the
conversions: the same 4,688 conversions cost 43-82 ms across windows,
and in one pass 346 of them took 35 ms, 102 µs each, where the ab's
counter had read 6.6. Glibc's trimming was not it -- run with trimming
off and alternated, the faults stayed at 12-14 thousand a burst and
the conversions no cheaper -- so the thread was profiled inside the
burst windows, by thread. In the slow window 69% of its samples were in
the kernel and 57% in `clear_page_erms`, against 18-26% and 6-15% in
the fast ones, with the same twelve to fourteen thousand faults in
each: the count is constant and the price is not, about 0.7 µs a fault
where the guest still backed the page and about 9 where its balloon
had handed the page to the host, which is the shape the segment
writer's folios had. The faults were the conversions' copies: each was
sized with a record's worth of room per record, twice the form's
bytes, so none fit the chunks the forms before it had freed and the
heap grew about ninety megabytes a burst. Sized exactly
(`Options::copy_exact`, the room kept as `supdb-copyslack` and
`supdb-ingestcopyslack`), the conversions read 1.20x and 1.28x cheaper
in the two pairs of twenty-four (p .023 and .007), the rewritten point
1.09x and 1.03x (neither significant), the tenth-rewritten point 3%
against in one pair and 6% for in the other at p .023 each -- the
position effect again -- and E and the mixes level; six reps of the
row's shape read no slow mode, passes of 12-22 ms with the thread at
131-166 ms, but the thread's faults fell only to about eleven
thousand, a fifth and not the half predicted. The rest of the fresh
pages are the snapshot's arena, which copies every value written in
key order, about 36 MB a burst at this rung, the sparse forms' growth
and the settle's scratch; the next count is the faults by the call
that took them, and what the first scan waits for follows.

#### The buffered load's drain: two manifests for one partition

The quick row read the buffered arm's ordered load at 0.39x of LMDB
without sync at ten thousand keys, 0.55x at thirty, 0.83x and 0.74x at a
hundred and three hundred thousand. The runner prints the load's own
phases now (`SUPDB_LOAD_PHASES=1`: the commits' time, the first and the
slowest commit, and the closing sync), and they split the loss in two.
At the small rungs the commits are close -- 2.1-2.9 ms against 1.5 at ten
thousand keys -- and the closing sync is the loss: 7-23 ms against the
comparator's one fdatasync of 1.7-2. At three hundred thousand the sync
is level, 28-37 ms against 28-31, and the commits are 58-110 ms against
53-57, with a commit of 4 ms in the slow reps where a 32 MB seal closes
inside the load.

The drain at ten thousand keys, stamped step by step with strace beside
it: the direct segment's finish 0.8-1.0 ms on the seal's thread, its
fsync 1.2-1.35 and the ordered index file's 0.4, and then two manifest
publishes of 2.3 ms each -- the manifest written and fsynced (0.3-0.5),
renamed over the last (1.5-1.8 ms on this filesystem, ext4's flush of a
file renamed over another) and the directory fsynced (0.2). The first
publish lands the segment as a piece, since the buffered arm's seals
leave pieces so that its reads take the piece path; the second is the
drain's promotion of that one piece to the partition, by link. The store
is the same partition after either, and `seals_first_partition` already
names a first partition at the close where the flush partitions, so the
drain under `flush_schedules` names it too
(`Options::drain_names_partition`, the two publishes kept as
`supdb-ingestdrainpromote`). Priced in pairs of twenty-four: the ordered
load 1.34x and 1.33x at ten thousand keys (21 and 24 of 24, p < .001)
and the shuffled 1.26x and 1.23x, since its drain promoted a lone piece
the same way; 1.17x and 1.05x at thirty thousand (p .007 and .023); at
three hundred thousand, twelve pairs, the load 0.94x and nothing moved
at p < .05. A side cell at ten thousand read 3% against the change at p
.023 and 11% for it in the replication: the position effect, again. The
drain read 4.7-5.5 ms in the row's shape then, against 1.7-2 for the
comparator; what was left was the finish, the two data fsyncs and the
one manifest publish, two thirds of which was the rename.

The rename's price was then taken apart in forty rounds of each shape
on this filesystem. A manifest written and fsynced costs 0.3 ms; the
directory fsync 0.05-0.2; renaming it over the old manifest 1.2-1.4; and
writing it under a fresh name and unlinking the old one afterwards costs
the same 1.2 at the unlink. What costs is freeing an inode: the
filesystem discards the freed blocks, and a discard is a device round
trip here, as the strace of the suite's own teardown shows at 8 ms for
a data file and 20 for a forty-megabyte floor file. A publish that frees
nothing -- the old manifest hard-linked under a spare name, the new one
renamed over it, the directory fsynced -- costs 0.46-0.48 ms on its
path, and the spare's unlink, 1.2 ms, moves to the segment work's idle
tick, to the close, or to the next open (`Options::manifest_spare`; the
rename that frees is `supdb-manifestfree` and `supdb-ingestmanifestfree`).
The `manifest` name is complete at every moment either way. Priced in
pairs of twenty-four: the buffered ordered load 1.22x at ten thousand
keys (19 of 24, p .007) and 1.15x at thirty (22 of 24, p < .001), the
durable load 1.16x at ten thousand (22 of 24), the shuffled loads
1.11-1.14x, and every read cell level. The buffered drain reads 3.3-3.8
ms in the row's shape now; the comparator's sync 1.7-3.3.

The next round trip on the drain was the ordered index's. Every
segment's index (`ord-*.oidx`, `src/ordindex.rs`) was fsynced beside
the segment at a seal's, a merge's and a drain's landing, a device
round trip for a file that is a function of the segment's keys and
nothing else: the builder takes the keys in rank order, and the prefix,
the uniform length and the heads follow from them. The index is written
and renamed into place unsynced now, and an open that finds it missing,
torn, or describing another segment composes it again from the
segment's keys, writes it synced and counts the rebuild
(`Options::ord_durable`, `Db::ord_rebuilt`; the fsync is `supdb-oidxsync`
and `supdb-ingestoidxsync`). The contract test flips a byte in every
index of a store with a tombstone piece and deletes one, and requires the
rebuilt files to equal the written ones byte for byte. Priced by the
load's own phase print, the drain's median moved 0.4 ms in every cell
but one: the buffered ordered drain 3.3 and 3.0 ms against 3.4 and 3.5
at ten thousand keys in two pairings, 5.6 against 6.0 at thirty, the
durable drain 2.1 against 2.5 twice, the shuffled 6.0-6.2 against
6.5. In pairs of twenty-four the buffered ordered load read 1.04x (16
of 24) and then 1.09x (20 of 24, p .002), the durable load 1.06x and
1.03x (18 of 24, p .023), the shuffled load 1.04x (p .002) and 1.01x.

One read cell moved against it twice: the durable arm's lag-10 scans
read 7% faster in the arm that syncs, 20 and 18 of 24, while every
counter the pairing reports was identical, no seal falls inside the
sweep, and the scan phases' own timers were level. A runner built with
both arms syncing, the names the only difference, read the two at
parity (0.99x), and the arm that does not sync read what the shipping
arm had read at that cell in every earlier pairing; the faster figure
was the paired arm's, in that pairing only. A cell with no code path
between the arms that moves anyway is the pairing's, and the gate's own
rule applies to it: a figure implausibly good is a broken measurement
until someone looks.

The large rung's loss is the writer's, and it has the slow mode's
shape. Over three hundred thousand ordered keys the commits take 61-74
ms in four reps of seven, 89-108 in two and 203-236 in two, CPU equal
to wall throughout and the writer's faults 17,400 in every rep, where
the comparator's commits take 54-99 with 400 faults, its writes landing
in the page cache. The faults are the ordered memtable's -- 52% its
entry slabs, 27% its byte arena -- and the segment writer's per-record
state, 19%; the direct run writes every record to its segment already,
and the memtable copies the keys and values again for the reads a run
in progress serves. Profiled in its own window the writer's commit
phase is 29% memmove, about 23% fault handling, 5% the kernel's copy
for its writes, and the rest the arena's reserve, the record's end, the
CRC and the memtable's entries and chunks. The lever left on this cell
is the copy the direct run keeps.

The copy was then priced by taking it away. The writer's samples over
the ordered commits at three hundred thousand keys, with frame pointers
and cut to the writer's thread and the commit windows, were a third
the harness's own -- its batch's copies and its key generation, paid by
the comparator alike -- and of the engine's two thirds, a third the
ordered table (its entries, keys and chunks, half of it the first touch
of their pages), a third the segment writer (three copies of each value
on the way to its record, the CRC and the record's end) and a tenth the
kernel's copy into the page cache. Three shapes of the same lever were
built in turn, each priced against the copies in one process
(`supdb-ingestrefvals` against `supdb-ingest`, twelve pairs): the
table's committed values as references into the run's own segment, read
through a mapping of the temp file that grows with it, with the staged
batch a copy in an arena. The first pushed the writer's buffer to the
file at every commit so the references could be read at once: it halved
the writer's faults and read the durable load at 1.18x, and the buffered
load 4% slower, its commits thirteen milliseconds more CPU and its reads
after the drain 3% slower in twelve pairs of twelve, since the three
hundred writes of a hundred kilobytes that replaced thirty-six of a
megabyte cost their calls and left the segment in folios an eighth the
size. The second converted a head only once the buffer had carried its
record to the file and kept the copy in the arena meanwhile, in two
arenas the writer meant to alternate once every head was a reference --
and never did on the buffered arm, since a commit's batch always
straddles the buffer's piece and the queue of heads waiting was never
empty at a commit's end: the faults read as the copies', and the load
level. The third counts the chunks each arena holds that a head still
names, stamps the arena drained of its last with the readers' epoch, and
resets it at a later commit once no reader pinned before remains, turning
to the other arena every piece: the buffered writer's faults fell from
17,400 to 10,300 and its CPU 5%, the durable writer's CPU 13%, and the
loads read 1.04x and 1.05x at three hundred thousand and a hundred
thousand keys in twelve pairs, short of significance and of the tenth the
lever was registered to move; the durable load 1.03x, every read cell
level. The copies ship and the references are the arm. What is left of
the table -- its entries at thirty-two bytes a key, its keys, the staged
copy -- is as much as the arena was, and the shape not yet priced is a
run that keeps no table, serving its reads from the segment writer's own
sorted keys and record offsets.

#### The pin's fence, priced against a sweep that fences for it

A read pins the epoch it reads in by storing it in its slot of the
reader table and reading the epoch again; a publish bumps the epoch,
and the sweep that frees what the publish replaced reads the slots. The
two are a Dekker pair -- each side a store and then a load of the
other's word -- and one side has to fence between its two, or both can
miss the other: the pinner's store sits in its store buffer while its
load reads the old epoch, the sweep's load reads the slot before the
store lands, and the sweep frees a state the pinner is about to walk.
The pin fences, with a sequentially consistent store, an `xchg`, on
every read of every handle and, since the writer's operations pin
(`Options::writer_pins`), on every operation of the writer. The sweep
runs once a publish; the pin runs once a read.

The quick row on the 2.10 GHz host class read point reads about a
quarter behind the rows of a fortnight before, and a bisect by
alternation (`bench run` rows of each commit against a fixed older one,
two rounds each) named the commit that pinned the writer's operations.
An asymmetric pin was built on that reading (`Options::asym_pins`,
`supdb-asympin`): where the kernel offers `membarrier`, the table
registers for its expedited private command at its making
(`Readers::asym`), the pin is a plain store between two acquire loads
with a compiler fence to keep the second load after the store, and a
sweep issues one `membarrier` before it reads the slots, which runs a
full barrier on every thread of the process. The proof is
the one `membarrier`-flavoured RCU makes. The barrier falls at some
point of each pinner's instruction stream: a pinner whose store is
before that point has it visible to the sweep's reads after the
barrier, so the sweep sees the pin; one whose store is after it has its
second load after it too, and that load sees the bump the sweep made
before the barrier, so the pin retries and lands at or past the bump,
where it holds the object the publish installed and not the one the
sweep frees. The loads are acquire so that a pin at or past a bump sees
the swap that preceded the bump. A kernel without `membarrier` keeps
the fence on the pin whatever the option says.

The barrier's own price here, from a C probe making twenty thousand of
them: 12-14 µs a call with one, two or three sibling threads spinning
on other cores -- an interrupt of each, through the hypervisor -- and
0.1-0.2 µs with every sibling asleep. Once a publish that is nothing,
and the arm's first build cost the buffered arm's largest lag burst
its whole margin: 3,000 barriers in the burst of a hundred thousand puts at
a hundred thousand keys, the burst 95 ms against 50. The writer's
every operation asks at its end whether a state a landing replaced can
be freed, since a writer gone quiet makes no publish to ask at, and for
as long as the upkeep thread's pass pins the replaced state the answer
is no -- and each asking was a barrier. The sweep peeks first now
(`Readers::oldest_for`): it reads the slots without the barrier, and a
pin it sees at an epoch below every tag in the list means nothing frees
whatever the barrier would show, so none is issued; the unfenced
reading is trusted only to say no, since a pin it sees may be gone,
which frees nothing early, and a pin it misses is the barrier's to
find. About sixty barriers a burst after that, and the burst level
within the alternation's spread (48-66 ms against 42-58 in two rounds).

Moving the fence also found a hole in the sweep's order that predates
it. The sweep read the slots and then took the list of retired items,
and two threads retire -- the writer at its freezes, the segment work
at its landings -- so an item the other thread pushed between the two
steps was judged against pins read before its retirement bumped the
epoch, and a reader pinned at the epoch before, holding what the item
was, is one those pins could miss. Tens of nanoseconds a landing, and
nothing raised; the proof above needs every bump the sweep frees for to
precede the barrier, and the order gave it only the sweep's own. The
list is taken first now, and the lowest tag in it is what the peek
compares against.

Priced as the arm pair, `bench ab --pairs read`, twelve pairs after a
warmup, the fenced pin against the plain one, the single-thread read:
at a hundred thousand keys the fenced pin read 0.84-0.99x in eleven
pairs of twelve in one sitting, 0.90x at the median, and 0.85-1.16x in
the next, faster in eight of twelve; at three hundred thousand
0.81-1.09x, slower in nine of twelve, 0.96x at the median; at thirty
thousand the pairs split six and six, 0.71-1.35x; and the two- and
four-thread rates were too wide to read at any rung, 0.6-2.1x pair to
pair. Two sittings that disagree in sign at one rung are no result,
and the margin they disagree over is the few percent one `xchg` a read
should cost, so the fence stays the default and the barrier is the arm.
The alternation through `bench run` rows that the bisect used, three
rounds at thirty and a hundred thousand, had read the two engines flat
on reads at about a tenth's resolution, which agrees.

What the bisect had conflated came out of the same alternation. Its two
neighbours were not adjacent commits: between them sits the commit that
made the harness copy every value a read returns into a sink, on every
arm, a fixed cost a read that is a larger share of supdb's read than of
LMDB's, so supdb's ratio to the comparator fell there with the engine
unchanged, and the rows before it are not comparable to the rows after
for a point read. The fence is a few percent at most; the quick row's
quarter is the harness's copy and a loss confined to the C mix that the
bisect has yet to reach.

#### The compact record's extents on the point read

The bisect the pin's fence came out of had a second step to find: the
quick row's C mix, zipfian point reads, stood about a quarter behind the
rows of a fortnight before once the harness's copy and the fence were
accounted for, while the uniform read stood level. Alternated commit by
commit against a fixed older head, two rounds each at thirty and a
hundred thousand keys with LMDB as the control, the loss was a slope
rather than a step -- about 0.95x at the 22nd commit of the range, 0.98
at the 33rd, 0.93 at the 35th, 0.97 at the 36th, 0.84 at the 37th, 0.80
at the 39th and at the 49th, 0.72 from the harness's copy on -- and its
one large drop was the commit that introduced the compact inline
record, with the record off. What that commit changed on a point read:
`flatindex::lookup_full` began returning the record's extents as a
two-variant value, a borrowed slice or the one extent rebuilt from a
compact header, by value through the blob and into the read, where it
had returned a borrowed slice. The scan path met the same shape and was
given `with_record_at`, which rebuilds the extent on the index's frame
and lends it (0.88x scans otherwise, above); the point read was not.

The uniform read did not show it because a uniform read over a store
larger than the cache is its misses, and a fixed cost of a few tens of
nanoseconds is noise beside them; the zipfian read is in cache and is
made of fixed costs. `Blob::read_all` goes through `with_lookup` now,
the extents lent on the index's frame to `read_exts`, and
`Options::exts_by_value` -- `supdb-extsval` -- keeps the by-value shape
as the arm. Priced as the pair, twelve pairs after a warmup: the
by-value shape reads the C mix at 0.89x in twelve pairs of twelve at
thirty thousand keys and 0.91x in ten of twelve at a hundred thousand,
the D and F mixes 0.90x and 0.93x at thirty thousand on seven and five
pairs, the uniform read 0.93x on nine of twelve at thirty thousand and
level at a hundred thousand, the loads and the scans level. The
remaining slope -- the 35th commit's few percent, the 38th and 39th --
is below what two rounds of alternation resolve and is left to the
rows.

#### The segment work on a thread of its own

A seal's landing, the merges and piece merges, the promotions, the
manifest and the WAL's retirement run on a thread of the store's own
(`Options::publish_in_background`), publishing by compare-and-swap
beside the writer's freezes; `supdb-inlinemaint` and
`supdb-ingestinline` keep the writer driving them. The writer hands a
seal over and returns, and waits only when it would seal again before
the last seal had landed.

Priced in one process, twelve pairs a rung at ten thousand, a hundred
thousand and three hundred thousand keys, on both the durable arm and
the buffered one, nothing timed moved: no quantity held under Holm in
any of the six tables, and the marks that fell below it were fewer than
a coin's. The buffered arm at three hundred thousand read the
four-thread scan pass at 0.76x on two of twelve pairs; twenty-four pairs
read it at 1.14x the other way, on sixteen. The seal and publish counts
match to the pair. The move buys the writer nothing the suite can see,
because the landing it took off the writer's thread was never where the
writer's time went: the buffered arm's ycsb-F, the mix the cadence sweep
found paying for seals, read the same with the landing moved. What it
buys is the store's: a seal that finishes while the writer is away is
published, where before it waited for the writer's next commit, seal or
flush.

One count separated the arms on every pair: the forms held at the end of
a pass, about one in a hundred fewer with the thread, at identical
bytes. Counted by kind, every one of the difference was a marker -- the
empty wide form that tells a reader to build the block itself -- on a
table trusted and complete in both arms, over the same partitions and
pieces. A null slot there reads as a clean block, so the question was
whether the thread left a clean reading where a build was owed. It did
not: a fresh handle's scan of the whole store at the pass's end took
three thousand published forms, built one block, and matched the
writer's own scan entry for entry, in both arms, at both rungs checked.
The blocks the thread leaves null are clean; inline carries about a
hundred markers on clean blocks, which ask a reader for more than those
blocks need and lose nothing. What leaves them there inline is not
traced.

#### A seal lands in two phases

A seal's thread renames its segments into place unsynced and names
them to the segment work, which opens and publishes them and retires
the frozen table in the same state; the fsyncs follow, on the seal's
thread, and only when they are paid does the segment work write the
manifest, retire the WAL and end the seal. A reader waits for no fsync.
The seals in flight are a queue, landed in the order they were handed
and the next only once the one before it is durable, so landings keep
sequence order and one seal at most is between its phases; the frozen
tables are a list in the queue's order, the writer freezes into room
and its backpressure waits only when the list is full, for the oldest's
readable landing, and a table a `sync` hands without a freeze joins the
queue behind the frozen tables' seals. Between the phases no
manifest is written: it would name segments that may be torn and cover
a sequence the WAL still has to hold, so a merge that finishes in the
window lands after the seal, and the shaping does not run in it. A
store has a manifest from birth for the same reason; without one, open
took every `seg-` file as live and skipped the WAL behind it, which was
safe only while a segment was synced before it was renamed. The direct
run's close takes the same two phases, its hard link being its rename.

Priced with the probe of the seal's own landing at the suite's shape --
shuffled keys, one seal, the landing polled for -- the time from
`seal()` to the segments' visibility lost the fsyncs and nothing else;
what remains of the landing is the piece's open, the Bloom built over
its keys, which nothing here moved. `tests/db.rs` holds the store in the
window (`Db::hold_seal_durable`) with a merge finishing beside the seal
and checks that the manifest stands still until the seal is durable.

#### A sync that is only the durable write

The suite's supdb arms flush at `sync`: the tail sealed, the seal waited
for, the store partitioned, all inside the load window, so the read
passes after it answer from routed segments. `supdb-ingestsync` makes
`sync` the durable write and nothing more. With the landing on a thread
of its own a finished seal no longer waits for the writer, so the
question is what else a store left that way lacks. Against the flushing
buffered arm, twelve pairs a rung: the ordered load 1.6-2.4x and the
shuffled one 2.6-3.1x, ycsb-F up to 2.4x, and every read pass behind --
point reads 0.5-0.78x, scans 0.1-0.32x, the lag sweep 0.03-0.14x, ycsb-E
0.13-0.19x. The counts say why: after such a sync the store has no
partition at any rung, so no scan takes the block path at all, and what
pieces it has sit under the merge trigger; an ordered load is often not
even pieces, but one direct run still open, which only the writer can
close.

`Options::adaptive_shape` (`supdb-ingestshape`) gives that store two
things, neither of which the caller waits for. A sync hands its tail to
a seal, whatever the store's shape; and the segment work partitions a
store that has pieces and no partition as soon as anything reads it --
by promotion where the pieces are disjoint, by a merge of all of them
where not. A partitioning happens once in a store's life, so waiting
while the store is read gains nothing, and a store no one reads still
waits for the trigger. A first version priced the merge against the
scans' merge-path time at the store's measured write rate, and bought it
too late for any pass to see; a second let the sync wait for a seal
already in flight so as to seal the tail behind it, which gave the load
back most of what not flushing had won, and made the tail a second piece
overlapping the first, so that what one promotion would have partitioned
took a merge. A third left the tail live whenever a seal was in flight or
a partition existed, and after an ordered load that was the shape every
read pass paid for: the last run's keys in a live ordered table beside a
partition of the rest, searched by every point read of a key it did not
hold, and every scan building a block since a store with unsealed keys
never has its forms complete. The sync now hands the tail to a seal
without freezing it (`Db::hand_tail`): readers keep reading it as the
live table, the writer writes nothing into it, and it is replaced by
whichever side publishes first, the landing or the writer's next write.
The landing promotes by link a piece whose keys lie above its
partition's last key, which an ordered tail's do, so the ordered store
settles as two partitions and no unsealed key without a merge.

The load keeps its gain -- 1.6-2.4x ordered, 2.5-3.7x shuffled -- and
the passes over the ordered store come most of the way back: the first
point-read pass 0.63-0.81x, the scan passes 0.72-1.06x, the mixes level
at a hundred thousand. The lag sweep did not: 0.03-0.1x at every rung,
its last point included, which runs after the store is partitioned.
Traced, that last point was three things, none of them the reads. The
segment work's thread joined each seal as it was handed over and sat in
the join for the seal's whole run, landing nothing else meanwhile, so at
three hundred thousand keys the piece a promotion would have made a
partition waited behind a seal the burst had triggered; shaping waited
for a read that had counted, and only a read that saw a piece counted,
so at ten thousand keys, where the sweep ran inside the first seal,
nothing was counted when the piece landed; and the seal sorted the
frozen table by comparison, two arena reads a compare, 147 ms of its
715 at three hundred thousand keys. The thread polls its seal as it
polls its merges now, a promotion waits for no read, any read the block
path cannot serve counts, a seal over an empty store under
`adaptive_shape` names the first partition itself, and the seal takes
the snapshot's radix sort. Twelve pairs a rung against the flushing
buffered arm, after: the ordered load 1.5-2.2x and the shuffled
2.4-2.6x; the sweep's last point 0.87x at ten thousand keys, 1.03x at a
hundred thousand, 0.41x at three hundred thousand; its first two points
0.03-0.14x at every rung, and its 10% point 0.11x at ten thousand keys,
where everything but the last point runs inside the first seal's 19 ms.

What is left is two things. Every pass that runs while a seal is in
flight reads the frozen table: on the merge path where no partition
exists yet, 60 us a scan of a hundred at three hundred thousand keys
where the block path takes 3, and on the block path through blocks the
frozen table dirties, which is the 0.41x; the seal is 440-650 ms there
-- the sort 31 ms now, the rest the record walk, four dependent misses a
record, and the kernel's copy into fresh page-cache pages. And an
ordered load's last ten thousand keys at three hundred thousand stay a
live ordered table for the whole read phase: the sync finds a seal in
flight and leaves the tail, and after the landing finds a partition and
leaves it again, so every read binary-searches it whatever the key and
every scan builds its block over a table that is never complete --
point reads 0.61x, scans 0.58-0.71x, the threaded passes with them.
That is the trade a sync that only makes the store durable offers,
stated in this suite's terms: the load is faster by the work it no
longer waits for, and the reads that follow within that work's duration
pay for it instead, at the merge path's price rather than the flush's.

#### A landing priced where the reads are

The round that made a `sync` the durable write alone -- the segment
work on its thread, the two-phase landing, the seal-published snapshot,
the handed tail -- was priced against the shape arm in one process, and
the shape arm kept up. The flush arm's own numbers fell meanwhile, and
the quick row could not say so: the host was out of band that day and
the gate gave no verdict. Read again beside the round's first head, the
flush arm's fully-unmerged lag point stood at 0.3x of what it had at ten
thousand keys, 0.46x at a hundred thousand, 0.56x at three hundred
thousand, its ten-percent point at 0.7x at the two larger rungs. A bisect
of the round's four heads against LMDB in one process each -- the
comparator does not move between processes, the arm does -- put the fall
at ten thousand keys on the two-phase landing and at a hundred thousand
on the seal's snapshot, and a probe of the suite's sweep timing every
scan of every point beside the store's counters showed one shape behind
both: a pass of two-microsecond scans with one scan of 0.6-0.9 ms in it
at ten thousand keys, 2-17 ms at a hundred thousand, at the scan where a
publish landed -- a seal's piece, the switch to the seal's snapshot, a
merge's partitions. Before the round a seal landed after its fsyncs,
which put every landing after the pass; the round put them inside it.
Timers in the scan's phases named the cost: the writer's rebase of its
tables at a new state took the new piece's ranks against its partition
(`BuildCtx::rank_pieces`), 0.6 ms at ten thousand keys and 4.4 at a
hundred thousand, 17 for every piece over a partition a merge had
rewritten, because the segment work took them only after it published
and the writer's scan got there first; and the snapshot's extension,
which every point's first scan and every switch to the seal's snapshot
makes, found each new key's place by a binary search over the run,
fourteen cold lines a key, 1-2 ms for a thousand keys over ten thousand,
and sorted the batch through the arena, a millisecond for five thousand.

The segment work takes the ranks and the pieces' bounds before it
publishes now (`Maint::rank_before_publish`), at a seal's landing, a
merge's, a piece merge's and a promotion's, and the extension orders its
batch by prefix words and walks the run where the batch is dense in it,
galloping where it is sparse. Through the probe, the landing's scan at
ten thousand keys fell from 560-885 us to 173-248, a landing at a
hundred thousand from 4.5 ms to 0.46, and the switch's extension from
1.2-1.6 ms to 0.5-0.7; the ten-percent point's first scan at a hundred
thousand, an extension of five thousand keys, from 2.4 ms to 15 us.
Priced in one process against LMDB, the round's last head and this one
on the same day, twelve pairs a rung and six at the largest: the flush
arm's fully-unmerged lag point 1.98x of LMDB's to 2.69x at ten thousand
keys, 0.96x to 1.56x at a hundred thousand, 0.69x to 2.02x at three
hundred thousand; its ten-percent point 1.46x to 1.75x at three hundred
thousand; the point-read pass at ten thousand keys 1.02x to 1.80x; the
drained scan pass and the other lag points within noise. Against the
round's first head, again on the same day, the fully-unmerged point
stands at 0.9x of it at ten thousand keys and 1.2x at a hundred
thousand. Beside the shape arm, twelve pairs a rung, that arm's
fully-unmerged point reads 0.90x, 0.85x and 0.76x of the flush arm's,
where in the morning it read 1.61x, 1.01x and 1.61x of a flush arm that
was slow; its first two points stay at 0.04-0.1x, the merge path over
the frozen table while the seal runs, which is that arm's own question.

What the probe leaves: the switch to the seal's snapshot still costs
its first scan 0.7 ms at a hundred thousand keys and 2.8 at three
hundred thousand, once a seal; the writer's snapshot is dropped at its
own freeze (`Options::snapshot_carry` is off) and the first scan after
rebuilds it over the frozen table and the live one, 47-59 ms at three
hundred thousand, before the seal's own snapshot is there to switch to;
and the builder ahead's forms, installed by the scan that finds them
posted, are 3.5 ms of one scan at a hundred thousand. The carry was
priced once and found to move no timing quantity, before the segment
work had a thread and the seal a snapshot; it is due another pricing.

#### The cap that follows the reads, both ways

A seal cadence chosen once is right for one shape, and the question
was whether the store could pick its own: tighten the seal cap and the
merge trigger while reads pay for lag, relax them over a write-only
stretch. The signal is a read that lag costs -- a scan over unsealed
keys or level-0 pieces, a point read whose range has a piece -- counted
on the handle's own line, and the writer's next commit takes the
store's lag level from it: full at such a read since the commit before,
and what the writes since the last one leave of it over a relax span,
`lag_relax_pct` of the partitions' keys, so a burst with no reads in it
decays the level with its writes and not with time, which is the
sweep's shape. The cap the seal threshold takes is the line from
`seal_max_pct` at a level of zero to `cap_reading_pct` at the full
level, and the trigger the segment work asks is the line from
`l0_trigger` to `trigger_reading` (`Options::adaptive_cap`,
`Options::adaptive_trigger`). The level is keyed on the live table by
identity: keyed on the state's generation, which every landing bumps,
the first version counted the table's whole log again at every one.

Priced in one process against the arm it differs from by the one
option, twelve pairs a rung, the cap at two percent under reads against
ten:

| tighter under reads | 10k | 100k | 300k |
|---|---|---|---|
| seals, buffered | 2 / 2 | 11 / 15 | 13 / 23 |
| ycsb-D, buffered | 1.23x (ns) | **0.69x** | **0.84x** |
| ycsb-E, buffered | 0.95x (ns) | **0.73x** | 0.94x |
| ycsb-F, buffered | 1.02x (ns) | 0.98x (ns) | 0.78x |
| scan-lag, a tenth unmerged, buffered | 0.86x (ns) | 0.99x (ns) | 0.76x |
| scan-lag, all unmerged, buffered | 0.96x (ns) | 0.82x (ns) | 1.18x (ns) |
| seals, durable | 3 / 3 | 11 / 14 | 13 / 24 |
| scan-lag, a tenth unmerged, durable | 1.46x (ns) | 0.87x (ns) | **0.85x** |
| ycsb-D, durable | 0.82x (ns) | 0.49x (ns) | 0.94x (ns) |

Bold is 0/12 or 12/12 and holds under Holm; the rest of the table's
figures are the sign test's marks or noise. The trigger at two pieces
on top of the cap moved no timing quantity at any rung, buffered or
durable, and its seal and publish counts were the cap's: the level
moves at a commit and decays over the burst, so at the burst's end,
where the pieces are, the trigger stood at its idle value. The cap the
other way, thirty percent under reads against ten, cut the seals to
seven from eleven at a hundred thousand keys and nine from thirteen at
three hundred thousand, on both configurations, and moved no timing
quantity either: the mixes' one seal lands in F before any piece
exists, so nothing has counted lag when it is decided, and the lag
points do not price the seal count at all.

What they price, the probe that ran the suite's mixes on one store
with every window of a thousand operations split into its reads and
its commits found in the commits. The freeze's commit at a hundred
thousand keys, 5-6 ms on the writer's thread: the WAL fdatasync the
seal makes before it rotates, 2.5 ms, on the buffered arm whose commits
never sync; the settle the commit files before the freeze, 2 ms; the
freeze itself, 0.6; the directory fsync for the new WAL, 0.3. The
reads beside a seal in flight ran at their own rate to the nanosecond.
And with the cap tighter, D's freeze left an empty table, D's last
inserts -- keys above the store's greatest -- opened a direct run over
it, and E's first scans built every one of the store's 1,538 blocks:
the writer's rebase of its tables at the piece's landing found a live
table it did not know, the ordered one the run had installed, and
dropped every form. The switch is the writer's own act and is carried
at once now (`carry_switch`), as a freeze is: through the probe, E's
scans built 213 blocks in place of 1,538 and E ran at the plain arm's
rate, and the test that holds it built 32 blocks before and at most
two after. What a seal costs its readers
otherwise is the piece each point read consults until the merge,
about a hundred nanoseconds a read, and the first scan's snapshot over
the frozen table.

So the cadence stands at the cap as measured, and the signal and the
lines stay behind their options for the arm that prices them. A cap
that moves with the reads would have to be worth more than a seal's
5 ms on the writer and its landing inside a pass of reads, and at
these rungs the lag it removes is worth less than that in either
direction.

#### Seals that keep the log, and a first seal at the floor

The shape arm -- a `sync` that is the durable write alone, the store
shaped once something reads it -- loads at 1.6-2.4x the flush arm and
reads its first pass after the load at 0.05-0.08x. The question was
whether a writer that only logs and a background that always seals
could have both. Three levers, each behind its own option and priced
against the arm it adds to, twelve pairs a rung at 10k, 100k and 300k:

- **A seal keeps the WAL** (`seal_rotates_wal` off). The seal's end is
  the live file's sequence, the file is not synced or rotated, and it
  rotates by size at a commit, synced first -- replay carries each
  file's sequence into the next and refuses a gap, so a file rotated
  away with an unsynced tail could leave a store that does not open.
  The writer's time in the seals its commits start fell to 4-10% on the
  shape arm and 22-56% on the durable default, ycsb-F rose 1.2-1.3x on
  the shape arm, and nothing fell.
- **A commit defers** a seal the frozen slot cannot take
  (`seal_defers`), up to twice the threshold. Inert on the suite: the
  slot was almost never held at a threshold commit. Since the frozen
  tables became a list, it defers only while the list is full.
- **A fresh store's first seal comes at the floor** under the shape
  (`seal_first_floor`), so its load leaves a partition. The lag sweep's
  first point rose 11x at 100k and 7.5x at 300k -- and the shuffled load
  fell to 0.32x and 0.23x, with the point-read mixes at 0.83-0.87x.

The floor's load was the commits'. A probe timing the load's appends
and commits apart put the commits at 130-160 ms at a hundred thousand
keys and 890 ms at three hundred thousand, against 6 ms without the
floor: the settle files a batch whenever the backlog passes a share of
the partitions' keys, and with a partition in place from the first
megabyte every commit of the load filed its batch into block forms --
over a thousand blocks built, most invalidated by the next seal or
merge. Lending that work to the upkeep thread freed the commits and
left the reads after the load building those blocks themselves, at a
tenth of the rate. Counted as the load and the first pass together, the
flush arm takes about 156 ms at a hundred thousand keys and 467 at
three hundred thousand, the shape arm without the floor 116 and 334,
and with it 222 and 1,004: the decoupled arm is already the faster in
total, its first pass slow because the suite charges the flush arm's
drain to its load and the shape arm's deferred shaping to its reads,
and the floor moved work into the load rather than out of the path.

Kept: seals that keep the WAL as the default, since nothing fell, with
the rotating seal as the comparison arm (`supdb-rotate`,
`supdb-shaperotate`); the other two options off, with arms to price
them (`supdb-shapedefer`, `supdb-shapefloor`); a floor that
never closes a direct run, which at the floor had cut the ordered load
into partitions of a megabyte each. A rotation's bookkeeping moves
before its directory barrier, and a failed barrier is retried by every
commit before it writes: done after it, a failed fsync left the writer
on a file at sequence zero under the old id.

#### A frozen table sorted once

A frozen table is final, so its order is too, and it was sorted up to
three times: by the writer's first scan after the freeze, inside its
snapshot of frozen and live keys together; by the seal, for its records;
and by any reader of a state published while the seal ran, since the
seal's copy was published into the state the freeze made and no later
one -- and for a table a `sync` handed without a freeze, into none, the
table being live when the copy was made and frozen by the writer's next
write. The table keeps its own snapshot now (`FrozenSnaps`): whoever
sorts it first -- the seal, a freeze's carry, the keeper, a scan -- sets
it on the table, and every later reader takes it. A scan over a state
with a frozen table and no snapshot of its own carries the table's
forward with the live keys alone, where it sorted both; the seal takes
an order it finds, once it holds every entry, and builds its copy from
it rather than from a sort. `frozen_snaps` off is the shape before it,
priced by `supdb-nofrozen` and, under the shape, `supdb-shapenofrozen`.

The default arm saves no sort by it. A seal starts its own sort at the
freeze, ahead of any reader, and a reader that scans before the seal's
copy is set sorts the table alone where it sorted both tables before:
per pass at a hundred thousand keys, builds and the frozen table's sorts
came to 13-17 against 13-18 builds without it, and none of the pass's
timed quantities moved at ten thousand, a hundred thousand or three
hundred thousand keys. The shape is where it pays, because a handed
table's copy reached no reader: the sorts per pass fell from 4.6 to 3.6
at ten thousand keys, 16.3 to 13.5 at a hundred thousand and 22.9 to
21.1 at three hundred thousand. The lag sweep's first point of writes
read 1.5-2.4x faster at ten and a hundred thousand keys in both of two
sittings, and its next point slower at ten thousand in both, by 13% in
one and 3x in the other: both points read bimodal across reps in both
arms, so the change moves which shape a point lands in rather than
making one, and the sweep's four points summed came out level -- 444
against 455 ns an entry at ten thousand keys, 654 against 698 at a
hundred thousand, 476 against 487 at three hundred thousand, none
resolved by sign.

#### A live snapshot over the frozen table's own

The snapshot a state with a frozen table reads holds the live table's
entries alone, with the frozen table's own snapshot as its base, and a
read folds the base's run in beside the live runs: a scan's cursor takes
the least of four heads, and a block's overlay merges the base's run
over the block, the live run over it and the keys filed since, a key two
of them hold made one entry. One run held both tables before, so every
extension and every handle's build while the seal ran copied every
frozen entry, and the landing that sealed the frozen table dropped the
run with the frozen keys it could no longer hold. An extension copies the
live keys alone now, the writer's landing keeps its live runs and drops
the base, and the base's bounds against a partition are taken once on
the base, for every snapshot over it.

The suite has no arm for the run of both tables, so it was priced as two
binaries alternated in one sitting, each supdb arm over its comparator in
the same run. The durable arm's fully unmerged lag point at ten thousand
keys read 1.5x in two sittings of eight rounds over the quick ladder and
1.4x in a third of twelve over its two smallest rungs, in every round of
all three. Nothing else held across sittings. The shape arm's first lag
point read about 0.94x over the first two pooled, the new binary ahead in
18 of 64 rounds, and level in the third. Where a base stands beside a live
run the cursor folds two runs a key, and its peek and the advance after
it made four compares a key where one merged run made one; it folds once
now (`Snapshot::front`) and steps the runs it found on the key without
comparing again, one compare a key with a base and none with a run alone.
Against the binary before the fold, in the third sitting, the shape arm's
lag points read 1.1-1.3x in 7-9 of 12 rounds and the durable drained scan
0.9x in 2-3 of 12, where the cursor has no unsealed key to fold; neither
was held to a second sitting.

Generalised to a base a frozen table, the fold took each head into one
value -- a key's slot and run in every table -- and returned it with the
mask, and it stalled: the value went to the stack in stores of one, four,
eight and sixteen bytes and came out to the caller in loads of eight and
sixteen that spanned them, none of them forwarded, twice a key. Callgrind
counted the same 4.56 million instructions in the cursor before and after
over the shape arm's first lag point at ten thousand keys; perf put 4.6
times the samples on it, 42% of them on one sixteen-byte load from the
stack, and the point's median scan rose from 11.5 to 17.3 us, slower in
every round of a sitting. Its third point read faster beside it, the
slower points before leaving the upkeep thread time to build the blocks
the third point's scans would have built. The cursor finds the key and
the mask as two scalars now and reads a head's entries only where a
caller asks (`Snapshot::entries`): 12.0 us, and the third point back to
its two shapes in the old proportion. Against the binary before the
list, two sittings of twelve rounds over the two smallest rungs, each
arm over its comparator: no timed quantity was on one side in both. The
shape arm's first point read 0.95-0.97x in the first and 1.04-1.07x in
the second; the durable arm's 0.94-0.97x in both, ahead in 4-5 of 12
rounds, which the sign does not resolve.

#### Three frozen tables

The frozen tables are a list, oldest first and in the order their seals
were handed, of up to three (`FROZEN_CAP`): the writer freezes into room
at a seal, at a commit past the threshold and at its first write after a
`sync`'s hand-off, and waits for the oldest's readable landing only when
the list is full. A landing retires the front, and a table a `sync`
handed without a freeze joins the back when the writer freezes it. Every
read folds the frozen tables by age, each through its own snapshot as a
base of the live one.

Priced against the binary with one frozen table, two sittings of six
rounds over the 100k and 300k rungs, each arm over its comparator: the
shape arm's 1% and 10% lag points at 300k read 0.35x and 0.41x in the
first and 0.35x and 0.32x in the second, slower in every round of both,
and nothing else held a side across the two. The suite times a lag
point's scans and not its burst. Under one frozen table the burst's first
write after the sync's hand-off waited for the load's last seal --
42-354 ms of join wait in the 1% point's 3,000 updates -- and the scans
after it met a landed store; under three it waits for nothing, and the
scans run beside the seals still in flight. The sweep's own sequence,
the burst and the scans timed apart over six pairs a size in each of two
sittings, puts the 1% point at 0.29x and 0.52x of the one-table binary
summed (faster in every pair of both), the 10% point at 0.84x and 0.91x
(4 and 5 of 6), the 0% point at 0.88x and 0.91x (4 of 6 each), the 100%
point level, and nothing at 100k on one side.

#### Seals whenever the sealer is idle

`Options::seal_idle` (the `supdb-idle` and `supdb-shapeidle` arms) seals
at a commit under the threshold whenever no seal is writing a table --
the frozen list empty, no table handed -- and the live table holds
`SEAL_CAP_FLOOR` or more, so the unsealed backlog is at most what was
written while one seal ran, and a `sync` hands a small table where it
handed up to `seal_bytes` of one. It runs only in a stretch of writes
nobody reads, counted at each commit: a store read by key or by range
keeps its writes in the table, where a point read is one probe and a
scan reads them through the forms. While it runs the threshold is
`seal_bytes` alone, the cap and the first floor off, and a commit with
no scan near files no backlog into the forms.

Each of those three conditions was a loss before it was a rule. Sealing
regardless of reads put partitions under the shuffled load from its
first megabyte, and every commit of the load filed its batch into forms
the next seal replaced: the shuffled load at three hundred thousand keys
read 0.23x. A window of recency on that filing gave the load back and
left the point-read mixes' writes unfiled for the scan mix after them,
which filed them: ycsb-E at 0.79-0.94x on every rung. With no filing in
any quiet commit, the lag sweep's last burst at ten thousand keys went
to the upkeep's hold instead of its commits' own filing, and the scans
after it read 0.74x; that burst is near a scan, and files as it would
without the option.

Priced in three sittings of fourteen pairs, each arm against its
comparator in one process. The first sitting and the second's 300k pairs
ran on one host and the rest on a second. On the shape arm at 300k the
1% lag point read 3.22x, 3.43x and 4.17x, faster in 13, 14 and 14 of 14
pairs, and the 10% point 2.70x, 2.97x and 1.19x (11, 11, 12 of 14); at
10k the 0% point read 4.13x, 5.22x and 4.98x (14, 14, 13 of 14) and the
1% point 1.19x, 1.15x and 1.03x. Nothing else held a side in more than
one sitting, the loads and ycsb-E included: E read 0.97-1.06x on the
shape arm and 0.93-1.11x on the flush arm, whose sweep stayed level.

At 100k the two hosts disagree. On the first, the 1% and 10% points read
2.90x and 4.22x (14 and 13 of 14); on the second, 0.85x and 0.89x, then
0.77x and 0.79x, faster in 5 or 6 pairs of 14, while the 0% point read
1.37x and 1.43x (14 of 14 in both). The store's shape at each point says
why. On the second host the comparator's one table, the whole load
handed at the sync, lands within the 0% pass in two reps of four and
within the 1% pass in a third, promoted to one partition of a hundred
thousand keys, and the passes after it read 28-36 million entries a
second; the idle arm reaches the 1% pass with the partition its first
seal made at the floor, ten thousand keys, under five or six pieces of
up to thirty thousand that the shaping has not merged, and reads 22-23
million, or 28-32 where the shaping had merged most of them. The slow
reps of either arm are a pass run beside a seal in flight -- the
comparator's hundred thousand keys, or the idle arm's last backlog of
fifty-five thousand, which a slow seal let grow -- a cost the option
leaves as it was.

Summed by phase on the second host, the sweep's sequence with one rep a
process and the arms alternated, the idle arm took 0.89x of the
comparator's time over the whole sequence at 100k (less in 8 of 8
pairs), 0.82x over the load, the sync and the 0% pass (7 of 8), and
1.16x over the 10% burst and its pass (2 of 8); at 300k, 0.95x over the
sequence (4 of 6) and 0.50x over the 1% burst and its pass (6 of 6). The
option stays off: the gains are the shape arm's lag points and its
sweep's sum, and the 100k reading on the second host is one a default
would carry.

#### A block's pieces merged, not sorted

A block built over level-0 pieces gathers each piece's keys over it and
orders them; the order was a comparison sort through the keys
themselves, and on the idle seals' arm, whose seals leave nine or ten
pieces over a small partition, that sort was 18% of the lag sweep's
first pass at three hundred thousand keys, profiled. Each piece's keys
over a block are in key order already, so the runs are merged now, a
tournament over their heads compared by the keys' leading sixteen bytes
as two words (`merge_runs`); in the same pass the merge was 9.9%. A
merge that scanned every head for every key was 13.5%, near the sort's
cost at that many pieces. `overlay_merge` off is the sort, priced by
`supdb-sortover` and `supdb-shapesortover`.

Against the sort in one process, fourteen pairs a pairing, nothing held
for a table as a whole: the shape arm at ten thousand, a hundred
thousand and three hundred thousand keys, the flush arm at three hundred
thousand, and the idle seals' arm at a hundred thousand and three
hundred thousand once with each version of the merge. The two quantities
marked singly were passes the merge does not run in, threaded point
reads and threaded scans that took published forms. The pass it cuts
varies by tens of percent with where the seals land, and the shipping
arms seldom build over that many pieces. Two binaries alternated
instead, with the first version of the merge, read the idle arm's first
pass 0.90x, and the shape arm's 1% point, which builds nothing over
pieces, at a median scan 5% slower in all eight pairs: that is the
layout of two binaries and not the merge.

#### A scan that waits for the landing, measured before it was built

The rule proposed was that a scan over a store with much unsealed and a
seal in flight wait for the seal's readable landing and then read the
segments, since a scan beside a frozen table reads at a tenth of the
rate of one over the segment it becomes: on the shape arm at three
hundred thousand keys, about 42 µs a scan beside the load's seal against
4 µs after it. It was measured first, in two parts, and not built.

Scans that run back to back until the landing spend beside it exactly
the time a wait would have taken, and finish scans while they do: on the
idle seals' arm at a hundred thousand keys, a pass's 106 scans beside
its seal took 19.5 ms against a wait of 19.4 to the same landing. A wait
can only win if the scans slow the seal enough to repay the scans they
finish, a tenth of its time at three hundred thousand keys. They do not
slow it. The same load, synced, landed its seal sooner with scans run
back to back beside it than with nobody reading in 8 of 12 alternated
pairs at a hundred thousand keys and 10 of 12 at three hundred thousand,
over two runs of six: medians of 111 and 131 ms with nobody reading
against 63 and 102 scanning at a hundred thousand, 419 and 442 against
319 and 268 at three hundred thousand. Why is not established; the seal
sorted the table itself in every rep of both.

What the passes beside a seal do spend is elsewhere. On the shape arm at
three hundred thousand keys the pass's first scan was 73-75 ms of
135-147, all of it the frozen table's sort and copy for its snapshot,
the work the seal was doing for its own copy of the same table at the
same time; on the idle seals' arm, whose seals leave pieces over a small
partition, 84% of the pass was building block forms over the pieces
after the landing.

#### A scan that takes the seal's table, measured before it was built

The contract proposed: a reader asks for the table a seal is sorting; if
the seal has built it the reader takes it, and if not one side builds it
once, the reader reads it and the seal goes on from it. Half of that is
`FrozenSnaps`, where whoever finishes first sets the table's copy and
later readers take it. What it lacks is the wait, so a reader that
arrives while the seal sorts sorts the table again beside it. On the
arms traced -- the shape arm, `supdb`, `supdb-ingest` and the idle
seals' -- the one timed pass where that happens is the shape arm's first
lag pass; on `supdb` the upkeep thread sorts each table of the last
point's burst beside the seal, 2-5 ms on a core nothing timed waits on.
The wait was prototyped three ways and priced at three hundred thousand
keys, eight rounds alternated in one binary, before anything was built.

At that rung the load's last freeze leaves 290,000 keys to a seal that
starts 13-18 ms before the pass's first scan. The scan builds a key order
of the table in 45-52 ms, its keys copied and its values not; the seal
sorts the same table in 14-22 ms as a list of slots, which is not that
form, and then copies the keys with their values in 50-62 ms. A scan
over an order without the values reads each key's values through the
memtable, about 37 µs; over the seal's copy, about 10. Waiting for the
copy made the first scan 70 ms and the pass 1.04x (slower in 5 of 8).
Having the seal make the reader's form from its slots first, with the
reader waiting for that, made the first scan 26 ms against 51 and the
pass 1.08x (slower in 6 of 8): the scans after it ran at 37 µs, and the
copy came later by the time the form took. A wait saves at most the
seal's head start, and the seal's product that makes scans fast takes
longer than the reader's own sort.

At ten and a hundred thousand keys nothing is frozen: `sync` hands the
live table to the seal (`hand_tail`), the pass starts with it, and the
seal's copy is never published to a state that holds its table live, so
the hundred-thousand-key pass scans at 20-45 µs to its end where a
frozen table's scans read at 8-11 once its copy lands. Freezing the tail
at `sync` when the frozen list has room -- the hand-off avoided a freeze
because the store had one frozen slot then -- read that point 0.82x
(faster in 7 of 8), and the 10% point 1.54x at a hundred thousand keys
and 1.72x at three hundred thousand (slower in 6 and 5 of 8) and the
100% point at three hundred thousand 0.68x (faster in 5 of 8): it
changes what the burst after the sync meets, not only what the first
pass reads. Neither was kept.

#### The flush's path, one step at a time

The buffered arm's ordered load at three hundred thousand keys loaded at
the durable arm's rate, and its `sync`, a flush, took 127-177 ms against
54-60 for `lmdb-nosync`'s. The writer made no sync call of its own in
it: it waited. Timestamps on the steps put the flush's time in a chain.
The direct run that had reached the threshold just before the flush was
still finishing its index, 23-58 ms more; then its segment's fsync,
49-62 ms; then the landing's open of the segment, 25-35 ms, its key
index's checksums and the Bloom a level-0 piece is given at open; then
the promotion that relinks it as the partition, 9 ms. The landing came
after the fsync because a drain joined each seal's thread before landing
it, where the poll lands a seal readable as soon as its thread names its
segments. The drain lands the front seal readable first now and joins it
after, so the open runs inside the fsync. The promotion's 9 ms was its
reopen reading the key index's checksum row again, 6-7 ms of it, for a
link to a file whose open had just read it; it reads none now.

Priced as two binaries alternated a process at a time, ten to sixteen
pairs a rung, the buffered arm's ordered load's flush read 0.74x at
three hundred thousand keys (shorter in 9 of 10 pairs), 0.78x at a
hundred thousand (12 of 12) and 0.81x at ten thousand (13 of 16), the
load with it 0.86-0.90x, and the durable arm's ordered flush 0.77x (7 of
8); the batches, which the change does not touch, were level in all of
them, and so was the durable arm's shuffled flush, a seal and a
partition merge whose time the fsync does not lead: 0.99x and 0.97x at
three hundred thousand and a hundred thousand keys. What is left of the
ordered flush is the run's index and its fsync, the second about what
LMDB's whole sync costs for about the same bytes.

A buffered run's segment written back as it grew was tried after it and
not kept. It is the lever the first suite priced as write-behind
spreading -- the segment writer syncing every 4 MB as it streamed, so
its pages left in slices -- and found inert on the durable load, which
is why `seal_sync_every` ships at zero; tried again in its asynchronous
form, `sync_file_range` on each 2 MB piece of a direct run whose commits
do not sync it, it gave the same answer. The run's final fsync at three
hundred thousand keys fell from 43-48 ms to 27-30, traced in three
pairs, and nothing the suite times resolved: against the run without it
in one process, fourteen pairs a rung, the ordered load read 1.05x at
three hundred thousand and a hundred thousand keys and 1.06x at ten
thousand, where a run of a megabyte and a half fills no piece and the
writeback never ran. On the durable arm, whose commits sync the run, the
same writeback read the ordered load 1.10x slower, in seven of eight
pairs.

#### The ordered load as a streaming write: the sync was the device's, and then the close's

The buffered ordered load, in one process against `lmdb-nosync`, read
0.76x at a hundred thousand keys and 0.82x at three hundred thousand,
and the load probe in the suite's shape split it into the commits and
the sync the harness ends the load with: at three hundred thousand keys
118-136 ms of commits and 62-71 of sync against LMDB's 110-126 and
43-45. The commits were the writer's, and callgrind priced them at
1,592 instructions a key: the staging memtable's write, a pin a key, a
record encoded and CRC'd and written one at a time, the value encoded
once more to test the inline bound, and the writer's four vectors
doubling from empty at every power of two, which the per-batch probe
read as batches of 0.6-9.4 ms at the doublings against a median of
0.3. The sync was the close of the direct segment: its index finished,
the whole segment fsynced, and the landing's open walking every record
for the Bloom filter and the tombstone flag and reading the key section
again to verify its checksum row.

The writer's cuts, each alternated as two binaries against the one
before it. The landing takes the Bloom from the hashes the writer
already has and the tombstone flag from the writer, and opens a segment
this process just wrote without verifying the index: the durable sync
-34%, the buffered unchanged, since there the walk had run beside the
fsync and been hidden by it. The vectors sized for the run, the seal's
from its table's count and a direct run's from the seal threshold,
address space until touched: buffered commits -12% at three hundred
thousand keys and -26% at a hundred thousand, the doublings' batches
gone from the tail. A batch's records appended to one buffer and hashed
and written at the marker, a pin a batch through `Db::append_batch`,
and the inline bound tested from the value's length: 1,699 to 1,377
instructions a key by callgrind, the time within the probe's spread --
an alternation of one binary against itself in the same sitting read
its commit medians 7% apart, so a commit difference under ten
milliseconds at this rung is not the probe's to resolve. The buffer
has a rule the recovery walk imposes: a sync that writes the held
records ahead of their marker folds them into the marker's CRC, since
the walk hashes every record since the last marker; the first version
did not, no caller reached it, and a unit test now writes, syncs, writes,
marks and recovers both batches.

The sync was then taken apart with a C probe writing forty megabytes in
one-megabyte pieces to a file on this guest's disk. Plain writes and an
fsync: 35-41 ms, the same after three hundred milliseconds idle, since
nothing starts the writeback before the kernel's timer. A
`sync_file_range` write hint after each piece: the fsync fell to 22-27
ms -- and stayed there after three hundred milliseconds idle with a wait
over the whole file taking 0-3 ms, the writeback long done. The
twenty-four milliseconds were not the page cache's: the block device has
a write-back cache (`queue/write_cache` reads `write back`), and the
fsync's FLUSH makes the host write what it holds to its own disk, forty
megabytes at about 1.7 GB/s, and nothing but an fsync asks it to. An
fdatasync a megabyte paid it inline, 62-80 ms over the writes with a
closing fsync of 0.1-0.3. On a thread beside a writer paced at the
load's rate, an fdatasync every eight megabytes: five flushes of 31-35
ms in all, hidden under the writer, whose longest write stayed at half a
millisecond, and a closing fsync of 0.1-0.2 ms; every sixteen, 5-6 ms;
every four, the same 0.1-0.4 at 37 ms of flushes.

So the segment writer hands each piece to the device as it lands
(`SegmentOptions::early_writeback`) and, once the file is eight
megabytes long, starts a thread of its own behind the writer
(`WriterHelper`, `sync_ahead`), woken by each piece: an fdatasync on a
dup of the descriptor whenever eight megabytes are on the file past what
is synced, with the writer's own sync, the durable arm's at every batch,
marking what it synced so that writer's thread syncs nothing. The hint
had been tried on 2 MB pieces and not kept (above): the run's fsync fell
by the same third and nothing the suite times resolved, because the
close's other costs stood, and the durable arm read 1.10x slower in
seven of eight pairs. This time the hint alone, alternated as probes:
buffered sync 62-78 ms to 34-39 at three hundred thousand keys, 25-31
to 16-29 at a hundred thousand, the durable arm's sync 35 to 23 by the
median and its commits level over three rounds. The thread's syncs
beside it: 23-27 to 19-23 at a hundred thousand keys and level at three
hundred thousand -- strace showed the four fdatasyncs of 5-6 ms and the
closing fsync at 0.8 ms, the mechanism exactly as predicted, and the
phase unmoved.

Timers in the close named why. The load at three hundred thousand keys
is two direct runs: the first reaches the seal threshold a few
milliseconds before the load ends, at thirty-eight of forty megabytes,
so its whole close runs inside the sync phase -- the trailer built in
2.5 ms, the checksum row's re-read and CRC of the section 11.5, the
table and the ordered index 3.5-7, the index's write 0.6-10, the two
fsyncs 0.8 and 2.9 -- then the tail run's small close, then the landings
with three manifest writes and the promotion by link, 3.6-8 ms. The row
was the largest piece, and it went to the same thread: the helper reads
each landed piece back from the page cache and hashes the section's
16 KB checksum pieces as they land, up to the section's end the finish
names, and the finish takes its row from the thread and reads back only
the header's piece, where the file holds zeroes until the header is
written, and the pieces past where the thread reached, from the file
or the writer's buffer. A unit test holds the helper's row to the one a
re-read gives, and a segment written with the helper byte-identical past
the superblock to one written without. Alternated: buffered sync 36 to
26.5 ms by the median at three hundred thousand keys and 19.5 to 17 at
a hundred thousand, the durable arm's 22 to 15.

One cost came with it, in the suite's device-bytes quantity, which is
the process's `write_bytes` from `/proc/self/io`: the buffered arm read
1.3646 bytes written per byte of data against 1.3035 before, about a
megabyte a segment, while the device's own counter in
`/sys/block/vda/stat` wrote the same bytes for every build. That
counter is kept at dirtying time, per folio, and a one-megabyte write
makes a one-megabyte folio: the three patches the finish makes in place
-- the superblock, the section header, and the row's placeholder --
each re-dirtied a folio the hints had already written back, and a probe
read exactly 2.000 MB for three patches of a few hundred bytes, 1.008
with the first eight kilobytes written as their own folio. The row is
now written in sequence, after the join, and the head of the file the
finish patches -- superblock, reserve, section header -- goes to the
file as a write of its own ahead of the first piece. Paired
again against the old arm at a hundred thousand keys, the quantity
reads 1.3669 against 1.3662, the eight kilobytes of the head, and the
probe alternated against the build before reads the sync level, 21-38
ms against 25-42 at three hundred thousand keys.

The verdict, `bench ab` in one sitting, six pairs: the buffered load
0.973x of `lmdb-nosync` at three hundred thousand keys and 0.931x at a
hundred thousand (0.819x and 0.759x in the baseline sitting), the
durable load 1.150x and 1.109x of `lmdb` (0.990x and 1.139x), and the
shuffled loads 2.43x, 1.94x, 6.1x and 5.9x (2.17x, 1.82x, 4.33x, 5.65x),
since a seal's segment is written by the same writer. Against its own
old shape in one process, the new default loads 1.23x and 1.36x on the
buffered arm (`supdb-ingestlatewb`) and 1.15x on the durable
(`supdb-latewb`), in twelve pairs of twelve, where the hint alone on 2 MB
pieces had read that arm 1.10x slower. The arms keep the shape before all of it: every piece
dirty and unsynced until the close, the row read back. A reader sees
nothing of any of it; the segment's bytes are the same.

What the close still holds at three hundred thousand keys, for the next
move on this cell: the first run cut at the threshold a few milliseconds
before the load's end, so that a thirty-eight megabyte close sits in the
flush where smaller runs would hide all but the tail's under the load's
commits; the ordered index built and written whole at the finish and
fsynced cold, 2.9 ms; and the drain's three manifest writes, each an
fsync and a directory sync, where one publish would do.

The rule the probe gave: on this machine a sync's cost is the device's
flush of what the hints already wrote, and it is paid by whoever issues
the fsync, so a write path that will be fsynced at its end hands the
device its pieces as they land and has something off the critical path
issue the flushes behind it; and a quantity read from `/proc/self/io` is
a count of folios dirtied, so a patch made in place after a writeback is
a folio written twice to it, whatever the device sees.

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
  whole of the saving above and buys no list at all. None past the
  handle's commit is taken at any setting: one published at a later
  commit, or by the writer over what it has staged, names keys the
  handle's commit does not hold, and a scan would count them. The looser
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
  watermark's commit. Read to its end, a handle settled a staged write
  under the committed watermark and kept the old run after the commit,
  since the log had not moved. The live table's entries are read to the
  same mark too, the entry count of that commit: read to the raw
  length, a handle's scan snapshot held the writer's staged keys, whose
  values the watermark hid, and the walk counted each against the
  scan's limit with nothing to emit. The length, the count and the
  watermark are taken as one commit's: a commit writes its log length, entry count and
  watermark into the one of two marks the commit before it did not
  write, and then names it, and a reader takes a mark only while it
  stays named, so it never waits on the writer. Taken as two loads, a
  handle could hold a commit's length with the watermark before it and
  keep the old run the same way.

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
