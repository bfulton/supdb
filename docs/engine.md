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
  publication — that this design deletes rather than optimizes. What
  "durable" is held to is `tests/kill.rs`: the test runs itself as a child
  that commits batches of a hundred keys without end and acknowledges each
  once its commit has returned, kills it with SIGKILL after a random count
  of acknowledgements, ten rounds a run, and finds every acknowledged batch
  in the reopened store, the one batch the child may have been inside whole
  or absent, and nothing beyond it. That is process death, with the device's
  word taken for what it synced; power loss, where a device may lie about
  its sync, needs a rig that pulls the plug, and is not promised here.
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
  of the durable load. **The incremental merge was the design's largest
  outstanding debt**, named independently by the compaction-policy run and
  the 1M-key run; partitioned compaction, range merges and piece promotion
  (below) are what paid it.
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
both ways. RocksDB's 32 MB block cache and absent filter are its shipped
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
forms carried across the publish (`supdb-carry`, an arm since removed) read 2.1-2.3 µs a
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
key at every block's first rank, and dropped the sparse forms, whose deltas
splice at ranks in the old partition's records, until the look resolved
each delta's cut again and kept them (`Options::rebase_recuts_sparse`,
with the pass after a full rewrite under a bound). In the probe the slow
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
(level 2; the arm is `supdb-hold` now) against the default, sixteen pairs a rung, won the fully
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

#### A buffered commit files its burst itself

The pass after a buffered burst was the first scan's: with the lag
sweep's counters read around each point (`SUPDB_LAG_COUNTERS`), the
pass at ten thousand keys minus the first scan's phases -- the wait for
the thread's pass in flight, the log read, the settle of the burst's
tail and the snapshot's refresh -- was 0.1-0.17 ms at every lag point,
the drained store's 0.1-0.2, and the phases were 0.2 ms at a tenth
unmerged and 0.3-1.0 at all of it; at thirty thousand the scans were
0.4-0.47 ms at every point against phases of 0.5-1.9. The scans over
the forms the thread had built ran at the clean rate, and the whole
loss against LMDB (0.23-0.63x at those two points, every rung) was the
burst's filing deferred to the scan. The thread had no window to pay it
in: the sweep's bursts at ten thousand keys are one commit each (99,
900 and nine of 1,000 writes, `Plan::batch`), and at three hundred
thousand the thread's 0.6 µs a write against the commit's 0.5 left it a
pass or two behind at the end of three hundred commits. `supdb-ingesthold`
had read its lag passes 2.6-6x faster at ten thousand keys with no pass
of the thread at all: level 2 holds only for a batch its own rules would
leave, and the bound of two percent made every commit of a thousand file
itself, so what it had priced there was the writer filing inline.

So a commit near reads whose batch is worth waking the thread for files
it itself, forced past the settle bound, and lends the thread nothing
(`Options::upkeep_inline_bursts`); a smaller batch lends as before. Priced
against `supdb-ingest` in one process, 24 pairs at ten and thirty
thousand keys and 12 at a hundred and three hundred thousand, the arm
read the lag scans at all of it unmerged 4.7x, 2.6x, 1.9x and 2.1x up
the ladder, at a tenth 2.6x, 2.5x, 2.2x and 2.9x, at a hundredth 0.99x
(a batch of 99, lent), 1.35x, 1.31x and 1.38x, every one 24/24 or 12/12;
the bursts at 0.66x, 0.64x, 0.59x and 0.49x at all of it, 0.56-0.67x and
0.44x at a tenth; and the mixes within noise at every rung. The thread's
time over a pass fell from 3.4 ms to 0.2 at ten thousand keys and from
250 ms to 26 at three hundred thousand. The bursts cost more than the
settle the scan had paid: the sum of a burst and its weighted pass at
all of it unmerged read 1.19x, 1.40x, 1.69x and 1.91x of the default's,
because the thread had filed 40-60% of a burst of nine commits beside
the writer and nearly all of a burst of three hundred, and the inline
fill pays all of it on the writer, at 0.2 µs a write at ten thousand
keys and 0.8 at three hundred thousand, where the forms no longer fit a
core's cache. The predictions had the sum level at the small rungs and
within 1.3x at the large ones; the share the thread was doing for free
was the quantity the first scan's phases could not show.

Against `lmdb-nosync`, 12 pairs: at ten thousand keys the lag scans at
all of it 1.47x, at a tenth 1.12x, at a hundredth 0.91x (lent, as
before), the bursts 1.27x, 1.48x and 2.67x, the sums 0.70-0.86 of
LMDB's, the mixes 1.55-2.4x with ycsb-E 0.73x as before; at three
hundred thousand the lag scans 1.10x, 1.40x and 1.49x, the bursts
1.38-1.96x, the sums 0.62-0.74, the mixes 1.59-3.0x with E 0.90x. The
default before it read those scan cells at 0.23-0.63x and the bursts
at 1.7-5.7x.

The durable store paired the same way (`supdb` against the arm) read its
lag scans 1.15-1.37x faster and its bursts 0.52-0.75x, the sums 1.5-1.75x
worse -- and its lag scans already read 0.92-1.51x of LMDB's in the quick
row, because a durable commit's fsync is the window the buffered commit
lacks: the thread files a batch of a thousand in 0.5-0.8 ms while the
device takes 0.7-1. So the rule is the commit's barrier: a commit that
syncs lends its burst, one that does not files it. The buffered default
flipped and the durable one is unchanged; `supdb-ingestlendbursts` lends
every batch as before, and `supdb-lendbursts` is the durable store under
the same option, which its commits never reach. The shipped default
paired against the lending arm read the same as the arm had against the
old default: at ten thousand keys the lag scans 4.4x and 2.7x at all of
it and a tenth, the bursts 0.67x and 0.62x, and at three hundred
thousand 1.85x and 2.4x against 0.44x and 0.42x, the mixes within noise
at both; the durable store against `supdb-lendbursts`, identical code,
marked two mixes at p=.04 and the plain scan at p=.006 in twelve pairs,
which is what a null pairing reads on this machine.

What the inline fill gives up is the thread's share beside a long
burst, and at three hundred thousand keys that share was most of the
filing. A commit that lends while the thread is keeping up and holds
or files only when it falls behind -- a pipeline a batch deep -- would
keep that share for a burst of hundreds of commits and still leave the
first scan a batch at most, which at that rung is a tenth of its pass;
at ten thousand keys a batch is a tenth of the store and the first
scan's share of it is the cell, so the pipeline cannot flip the small
rungs and the inline fill cannot keep the large rungs' burst. The
quantity that separates them is a batch's filing time against the pass
it would land on, and the engine measures the first; the second is the
reader's. The settle's own cost a write, 0.2 µs at ten thousand keys
and 0.8 at three hundred thousand, is the lever under both shapes.

The threshold that decides "worth waking for" is one rung of the ladder
wide. `UPKEEP_WAKE_WRITES` is 256, and the lag sweep's one-percent burst
is one batch of `size/100` writes: a hundred at ten thousand keys, lent
and taken back by the first scan, and three hundred at thirty thousand,
filed inline. So the one-percent point at ten thousand keys read level
with LMDB in every row (0.95-1.05x) while the point above it read
1.3-1.4x, and the first scan's settle of the 99 keys and the forms it
materialized counted 570,000 instructions over the pass's hundred
scans, 196 an entry against 139 for the sealed walk alone and 158 with
the burst filed at the commit (`Options::upkeep_inline_min`). Filing
every batch inline, `supdb-ingestinlineall` against `supdb-ingest` in
eight pairs each way: at ten thousand keys the one-percent scans 1.57x
and 1.60x (15 wins of 16), the burst 0.47x (0 of 16), their sum level,
and ycsb-A 0.78x and 0.86x, F 0.79x and 0.88x; at thirty thousand,
where both arms file the burst, A 0.65x and 0.53x and F 0.75x and
0.59x, the lag points level. A mix's batches of a hundred cross the same
threshold, and filing them on the mix's own thread costs it far more
than the alert thread's re-reads beside it did; and the burst at 0.47x
would fall under LMDB's (0.86M updates a second against our 1.5M at
ten thousand keys in the row). The threshold stays, as an option.

The filing's allocations were a smaller lever than their count said:
a block materialized clean answered from the table's bounds without an
overlay built to say so, one shared `Arc` for every clean slot, the
chain-offset scratch kept on the forms state and the filed lists born
with room took the point from 196 instructions an entry to 191, with
367 mallocs a pass where it made 498; the forms' own storage and the
lists' first allocations are what remain, 60% of the mallocs left.

What moved the point was splitting the fill. It is two halves: the
settle of the batch, each key resolved to its block and filed into
whatever form the block holds, about 150,000 instructions for a
hundred keys; and the forms for the blocks the scans will walk, which
`complete_forms` builds for every block of the table at the commit
where the scans build them lazily for only the blocks they touch, the
rest of the 427,000. A commit near reads whose batch is under the
threshold settles it there when a scan ran since the last commit, and
leaves the forms to the scans (`Options::upkeep_settle_after_scans`;
`supdb-ingestlazysettle` leaves the settle to the scan too). The
signal is the one the regime already keeps, the scan count unchanged
since the last commit, so a mix whose commits no scan separates --
A, B, D, F -- never reaches it, and E and the sweep's bursts do. Under
callgrind at ten thousand keys the one-percent burst's commit went from
115,000 instructions to 290,000 and the point's scans from 1.92M to
1.74M, 174 an entry, their sum level within 3,000, the first scan's
settle phase zero; over a rep's mixes the commits grew 123,000 and the
scans fell 1,260,000. Timed on a host out of band, the burst read 0.70x
and 0.62x of the arm that leaves the settle to the scan, and the scans
nothing a 0.3 ms pass can resolve; by the counts, on the row's host,
the point stands at about 1.08x of LMDB from level and the burst at
about 1.2x from 1.8x, two wins in place of a win and a level, the sum
of the two phases unchanged.

#### The scan's own clock

ycsb-E, the one mix that lost to LMDB on the buffered arm (0.73x at ten
thousand keys, 0.90x at three hundred thousand), is 1,584 scans of fifty
entries and one commit at the smallest rung. The suite prints the store's
counters around each mix now as it does around each lag point
(`SUPDB_MIX_COUNTERS`, one `mixcounters` line a mix), and the line split
the mix: the six phases a scan ran before its walk -- the upkeep taken
back, the log read, the settle, the snapshot's refresh, the builder's
check and its forms' install -- summed to 0.35-0.45 ms of E's 1.5, each
25-40 ns a scan at both rungs, which is what a clock read costs and not
what any of those phases do when nothing has moved. Callgrind over E's
interval, cut by a dump at every batch flush, put the clock paths at 17%
of the mix's instructions.

The phases are timed only when asked for now (`Options::scan_timers`, off
by default; the suite asks under its counters variables, and
`supdb-timers` and `supdb-ingesttimers` price the asking). Against the
timing arm, 24 pairs at ten thousand keys and 12 at three hundred
thousand: ycsb-E 1.30x and 1.25x, the scan workload 1.19x and 1.10x, the
lag passes 1.16-1.30x and 1.05-1.14x, the mixes that scan nothing within
noise. Against `lmdb-nosync`, twelve pairs: ycsb-E 0.97x at ten thousand
keys, unresolved, from 0.73x, and 1.24x at three hundred thousand from
0.90x; the scan workload 1.16x and 1.26x; the lag passes 1.06-2.05x and
1.34-1.81x. A first pairing had read level everywhere and was nearly
recorded as a refutation: the pairing script exported `SUPDB_LAG_COUNTERS` for its
counters lines, under which the suite times every arm, so the arms did
not differ. The counters mode costs what it measures, about a quarter of
a scan of fifty entries over ten thousand keys, and a figure read with it
on is a figure of the timed shape.

The same interval compared the two engines by instruction: our E ran
16.3M to LMDB's 21.4M with the same harness in both, 8,840 a scan against
11,960, and took 1.4x as long; the clean scan workload ran 13,880 a scan
of a hundred against LMDB's 22,210 at level time. A scan of ours stalls
about twice as much per instruction as LMDB's leaf walk, on a dirtied
store and a clean one alike, and that -- the walk's memory access, not
its arithmetic -- is where the rest of E and the plain scans' margin is.

A time profile cut to E's windows (each mix's edges are stamped under
`SUPDB_LAG_MARK` now) put the block walk's own code at 28% of the mix,
the harness's zipfian generator and value copies at a third, the walk's
prefetches at 6%, and about a sixth in the first scan of the mix: the
settle of the backlog the four mixes before it left, with its sorts and
allocations, and the forms the scans then built for the blocks those
mixes had dirtied. At ten thousand keys the upkeep thread made no pass
through those four mixes: their commits of a hundred writes are below
the batch that wakes it, and its poll had backed off to twenty
milliseconds through the read workloads before them.

Taking the prefetches away (`Options::scan_prefetch` off, the arm
`supdb-ingestnoprefetch`) read E 1.04x at ten thousand keys, unresolved,
and the lag passes 1.0-1.09x; at three hundred thousand, where the store
is past the cache, the lag passes read 0.80-0.93x and E 0.97x, 11/12 and
12/12. The prefetches stay. Paired again on this host after the E
window's profile put a tenth of its samples on the prefetch
instructions themselves: at three hundred thousand the shape without
them read the lag passes 0.67x and 0.79x, the mixed scans 0.81-0.83x
and E 0.76x, 5/5 each; at a hundred thousand E level and the mixed scans
1.09-1.18x without them, 4/8 and 5/8; at thirty thousand nothing
resolved. What the instructions stall on is the span: sixty-four lines
asked for at once against a core's dozen fills in flight, so the span
is an option now (`Options::scan_prefetch_bytes`), and a kilobyte was
priced against the four: E 1.02x at a hundred thousand keys, 7/8, with
the drained lag point 0.87x and the rewritten one 0.90x; at three
hundred thousand the plain scan 0.85x, the lag points 1.10-1.16x in two
pairs of five, E 0.97x. No direction survives both rungs, so the span
stays at four kilobytes and the kilobyte is the arm `supdb-prefetch1k`
and `supdb-ingestprefetch1k`.

The narrowest mix on this host is D, level with LMDB at thirty and a
hundred thousand keys where C reads 1.25-1.8x, and the mixes' timers
by kind say why: a D read costs 0.62 µs against a C read's 0.27 for
us, 0.64 against 0.37 for LMDB. D's reads are uniform, as C's are; what
differs is the store they read, after A and F have put a sixth of the
keys into the memtable. A probe over that store priced the sources a
read consults -- the partition alone 0.5 µs a read at a hundred
thousand keys, 0.9 to 1.0 with the memtable holding a sixth, 0.5 again
once it is sealed into a piece whose filter answers for it -- and its
profile put the excess not in the memtable's probe, a tenth of it, but
in the partition's own lookup, 0.30 to 0.65 µs a read: the memtable's
arena and index, and the block forms the upkeep keeps current for
every block the updates dirtied, a second copy of the partition, have
outgrown the cache with it. Against the arm with no block forms
(`supdb-nocache`, durable), D read 1.20x, B 1.18x, F 1.13x and A 1.06x
without them, 4-5/6, with D's reads 0.66 µs against 0.72 and its
commits 520 µs against 770 -- the thread's pass over the hundred keys a
commit lends, outrunning the fsync it works beside -- while E read 0.34x
and the rewritten lag point 0.15x, which is what the forms are for. The
forms are kept current through A, B, F and D, four mixes with no scan
in them, for E. Filing them only while scans are around, and leaving a
scan-free stretch's writes listed by block for the first walk to merge
(`added`, as a handle's extension files them), is the lever this
decomposition leaves, priced by the mixes' sum.

Waking the thread for the mixes' small batches was priced three ways
(`Options::upkeep_wake_parked`, the arm `supdb-ingestwake`), each
against the shape that wakes only for a batch worth waking for, 24 pairs
at ten thousand keys. Woken whenever its poll had grown past its
shortest, or past the wait it grants a short batch, the thread filed the
mixes' writes beside them -- its passes 12 to 21, the blocks it built 0
to 149 -- and ycsb-E read 1.18-1.19x, but ycsb-A 0.84-0.86x, ycsb-F
0.82-0.85x and B 0.95x, 23/24 and 21/24; woken only at its full
back-off it was never woken, since the reads before the mixes are
shorter than the poll's climb to twenty milliseconds. At three hundred
thousand keys nothing moved either way. A wake is a VM exit on this
guest and the passes run on a sibling of the writer's core, and
whichever of the two it is, the two mixes that write pay more than the
one that scans gains. The option is off, and the mixes' backlog stays
where the regime puts it: filed by the first scan that needs it.

Where ycsb-E stands: 0.97x of LMDB at ten thousand keys, unresolved at
twelve pairs, from 0.73x; 1.24x at three hundred thousand from 0.90x.
Two levers were refuted, and the move stops here. The sixth of E that
is the first scan's filing of the mixes' backlog is the shape to price
next if the cell is to flip at the small rung: the settle's cost a write
at a store that fits the cache, and the sorts and allocations around it.

A scan's first record sits behind three serial fills: the ordered
index's head stride, eight lines asked for at once; the directory word
at the rank the stride's select lands on, read by the record hint
before it can issue anything, since the hint's span is the span the
two directory words name; and the first record. The stride window is
fixed before the stride's lines are issued, so the directory lines of
the window are hinted at the same moment now and the record hint's
span is estimated from the mean record length, read once when the
index is parsed, so the second word is never on the chain: two serial
fills, not three (`Options::seek_prefetch_dir`; `supdb-dirplain` and
`supdb-ingestdirplain` keep the three). The directory is 120 KB at
thirty thousand keys and 1.2 MB at three hundred, touched about once a
line in a pass, so the word is an L3 fill at every rung from thirty
thousand, and the mem-floor prices one at 50 to 70 ns here; at ten
thousand the store sits in the L2 and there is nothing to hide. Priced
by the per-scan mean after the first scan, paired rep by rep in both
orders: at a hundred thousand keys the durable arm read the new shape
faster by 44 to 92 ns at the median of four sittings of twelve and
twenty-four pairs and the buffered arm by 59 and 0 in twelve pairs and
level in twenty-four; at three hundred thousand 4 to 154 ns in four
sittings; at ten thousand 109 ns against and 67 for, in the two orders.
None of it clears a sign test on this host, whose rep spread on one arm
is 1.25 to 2.11 µs a scan at a hundred thousand keys. The profile is
what resolves it: twenty-one scan passes of each arm in one process at
three hundred thousand keys, 35,000 samples an arm, put 3.4% of the
plain arm's samples on one instruction of the record walk and 0.5% of
the new arm's, every other hot instruction within a few tenths of a
percent between them -- a stall of about 55 ns a scan, the fill the
window takes off the chain. The per-scan median the harness prints was
rounded to a tenth of a microsecond and could not have seen it; it
prints to the nanosecond now.

The buffered one-percent burst at a million keys read 0.35x of
RocksDB's and 0.93x of LMDB's in the row while the ten-percent burst
read level and the durable arm ran the same burst five times faster,
so the deciding pairing was the arm that lends every batch
(`supdb-ingestlendbursts`) against the shipping one, at a million keys
with the lag counters on, eight pairs in each order. The lent burst
ran 1.4x and 3.2x the inline one on the one- and ten-percent points,
and the pass after paid it back: on the one-percent point the first
scan's settle of the nine thousand keys the thread had not filed was
3.6 ms and the pass 37 ms against 27, and on the ten-percent point the
first scan waited 27-32 ms for the thread's pass in flight and settled
12 ms, the pass 71-75 ms against 30, so the weighted sums read level
to a tenth in the inline arm's favour (1.04x and 0.89x on the
one-percent point, 1.04x and 1.11x on the ten-percent) and the regime
stands. What the inline fills spend is not per write: the ten-percent
burst's hundred fills cost 1.4 ms each where the one-percent burst's
ten cost 5.5, and the difference is the tables the drain had emptied,
over which each fill of a thousand uniform keys builds the block forms
of the thousand blocks it touched, cold, from the partition's records,
while the pass after it builds fourteen thousand blocks in both arms.
The profile of the burst's window by symbol, on the writer's thread
over six one-percent bursts, puts no one item in charge: the settle's
resolve of each key to its block over the ordered index's heads, cold
after the drain and the pass (`owner_of_from`, 13%); the copies (11%);
the fill's walk of every slot of the table, fifteen thousand a fill
whatever the batch (`complete_forms`, 7%); the allocator (10%); the
page faults and the log's writeback landing on the writer (13%); the
settle itself (5%); and the forms' builds (6%). The ten-percent burst's
fills, over warm heads and tables, spend the same symbols at a quarter
of the price a key. So the one-percent cell is the fill machinery run
cold ten times, and its levers are one per item, each its own move:
the slot walk bounded to the slots the batch touched, the resolve's
stride hinted a key ahead, the forms sized by count rather than room,
each priced on this burst and on the mixes before it is the default.

Paired the same way at ten thousand, a hundred thousand, three hundred
thousand, a million and three million keys (`pair.sh
SUPDB_LAG_COUNTERS=1 supdb-ingest supdb-ingestlendbursts SIZE --passes
load,lag --reps 8` at `37ea8f2`, six pairs a sitting at the two largest
rungs), lending every batch reads the sweep's weighted sum at
0.88-0.90x, 0.74x, 0.61x, 0.55x and 0.43x of the inline fill's, resolved
in both orders from a hundred thousand up and at the fully rewritten
point at every rung but ten thousand; the bursts 1.4-4x faster, the
passes after the tenth and the full rewrite 1.8-3.5x slower, the
one-percent point level everywhere but three million, where it reads
0.89x and 0.92x. The thread files a burst's batch at once now, so the
pass after a lent burst no longer pays what it paid when the inline fill
was chosen. The inline fill still holds the lag scan cells against LMDB
at the smallest rungs, which lending would read at about 0.3x of today's
at ten thousand; the pipeline a batch deep described above is the shape
that would keep both, and it is not built.

The larger part of the one-percent cell was not the fill. The lending
arm ran its own one-percent burst at 5 µs a write where its second and
third one-percent bursts, run after it through the lag probe
(`LAG_PCTS=0,1,2,3,10 lag_only supdb-ingestlendbursts 1000000 3` at
`37ea8f2`), ran 0.6-1.2, and `strace -f -tt -T` over the probe put the
difference in one call: the burst's third commit crossed `seal_bytes` of
the live WAL, the rotation fdatasynced the file it closed, 31 MB of
frames a buffered store had never synced, 43.5 ms on the writer's
thread, and then the directory; at three million keys the two took
106 ms of the lending arm's 92-147 ms burst. The shuffled load leaves
the live file within a few commits of the boundary at both rungs, so
every rep paid it, and the durable arm's rotation, every frame of it
synced at its commit, paid nothing. A rotation leaves the frames the
policy left unsynced now; the file it begins says so in its header
kind; the first sync by policy syncs the closed file before the live one
and chains the kind; and replay reads a gap at a loose boundary as the
log's end, where at a chained one it refuses the open (`Db::rotate_wal`,
`Wal::replay`). The probe reads the shipped arm's first one-percent
burst at 24-34 ms and its second at 28-29 at the commit that made the
change, against 71-76 and 22-27 before it (`LAG_PCTS=0,1,2 lag_only
supdb-ingest 1000000 1`, three runs each). Paired in one process against
the shape before, kept as an arm in the measuring tree alone, at a
million keys over the load and lag passes, six pairs a sitting in both
orders: the one-percent burst 2.6x and 2.7x (6/6 each), its weighted sum
0.75x and 0.65x (6/6 each), the tenth and the full rewrite within noise
with their bursts 24 against 61 ms, 186 against 182 and 2,327 against
2,394 in the counters, and the loads and the scans level.

The pipeline described above is built (`Options::upkeep_pace_pct`,
`Hand::Pace`): of the batches a buffered commit near reads would file
itself, the first after a scan still is, and the rest, each at most a
hundredth of the store's keys, are lent, the commit returning once the
thread trails by at most that hundredth. Two depths were priced, each
against the inline fill (`supdb-ingestnopace`) and against lending every
batch (`pair.sh SUPDB_LAG_COUNTERS=1 supdb-ingest ARM SIZE --passes
load,lag --reps 8`, six pairs a sitting at a million and three million,
at the commit that added the option). A batch deep -- the commit holding
until the thread had filed the batch before it -- the pace read the
sweep's weighted sum at 0.77x and 0.83x of the inline fill's at a
million keys and at 1.35x and 1.39x of lending's: every batch was a pass
of the thread's, and a pass has a fixed part, the log read, the snapshot
moved and the bounds walked, which read 2 ms a pass of a thousand writes
at a million keys and 3.2 at three million against 1.0-1.5 of filing, so
the thread did twice the work a batch and the writer waited for it. With
the depth the share, the hundredth, the thread's passes cover several
batches where it falls behind, and against the inline fill the pace
reads the sum at 0.79x and 0.82x at three hundred thousand keys, 0.75x
and 0.81x at a million and 0.64x and 0.64x at three million, 8/8 or 6/6
in both orders, the bursts 1.3-4.7x faster and the scans after the tenth
and the full rewrite at 0.6-0.9x; at a hundred thousand 0.85x and 1.02x,
and at ten thousand, where a batch of a thousand is a tenth of the store
and is filed in the commit as before, a control. Against lending every
batch the pace reads 1.21x and 1.29x at a million and 1.23x and 1.29x at
three million: lending's bursts run 1.7-2.5x faster still, since the
thread's filing is cheapest in the long passes a run-ahead writer leaves
it -- a block dirtied by many writes is built once where the pace
patches it a few batches at a time -- and its scans after the tenth and
the full rewrite run 2-7.8x slower, the residue the first scan settles.
The share is where the two are traded: the pass in flight the first scan
waits for is at most a hundredth of the store's fill, and what lending
saves on the burst past that is what the pace leaves. The lever that
left, the thread gathering half the share before it begins a pass while
the writer runs ahead, so that its passes are longer while the first
scan's wait stays bounded, was built on `Options::upkeep_batch_pct`'s
gate and priced the same way, eight pairs a sitting at three hundred
thousand keys and six at a million and three million, both orders, at
the commit after the option's. It read level: against the inline fill
0.83x and 0.74x, 0.83x and 0.70x and 0.66x and 0.66x where the pace
without it read 0.79x and 0.82x, 0.75x and 0.81x and 0.64x and 0.64x;
against lending 1.26x and 1.28x at a million and 1.39x and 1.30x at
three million where it read 1.21x and 1.29x and 1.29x and 1.23x; and
the one-percent burst at three million, thirty batches with a gather of
fifteen, at 0.6-0.8x of the pace's without it in all four sittings. The
counters say why it could not gain. The thread trails the writer and
takes what it finds, so its passes over the full rewrite already held
4.3 batches at a million keys and 14.5 at three million, 232 and 207
passes, and the gather made them 5.6 and 15.5, 179 and 193; the fixed
part those passes cost is three tenths of the thread's time at a
million keys (232 passes at 2 ms of 1,580) and a ninth at three million
(207 at 3.2 ms of 5,944), the share bounds the trail at ten and thirty
batches, so no gather under it reaches lending's eight to ten passes,
and one of the whole share has the thread idle while the writer fills
to it and the writer wait the pass out. What lending's passes save
besides is the visit: a block is visited once a pass for the writes the
pass holds for it, and over a rewrite each block takes its sixty-four
writes in about fifty-five visits under the pace against eight to ten
under lending; the visit is priced by its misses, so the thread's
per-write work, its time less the fixed parts over the writes, reads
1.1x lending's at a million keys and 1.6x at three million (1.76 µs a
write against 1.1). The gather is not kept. The pace's burst is the
thread's throughput -- the burst reads within a tenth of the thread's
time at both rungs -- and the thread's pass under it is profiled against
lending's (`SUPDB_LAG_MARK=1 perf record -e cpu-clock -F 4000 -k
CLOCK_MONOTONIC` over `lag_only ARM SIZE 2`, the samples cut to the full
rewrite's burst window and the `supdb-upkeep` thread, at `18aefe7`). Of
what the paced thread costs over lending's, 426 ms of 1,850 at a million
keys and 2,450 of 6,200 at three million, three quarters and two thirds
are in `settle_each` and `cache_seek_exact`, with the builds and the
patches level between the arms: the visits, and the resolve. Half of
`settle_each`'s samples sit on `prefetch_form`'s loop, the next block's
form fetched whole before its one or two keys are patched into it -- the
fetch is the visit's price, and the pace makes 5.6 times lending's
visits -- and the resolve's samples sit on the gallop's head loads. A
seek that galloped over the index's in-memory top level and then
searched the one stride the answer lies in, its eight lines asked for at
once, was built for the resolve and priced against the gallop as an arm
at three hundred thousand, a million and three million keys, both
orders, and read level: the symbol kept its tenth of the thread in the
profile of either arm, 740 of 7,228 samples against 821 of 7,406 at a
million and 2,767 of 25,855 against 3,064 of 24,843 at three million,
and no cell resolved in either order. The gallop's probes for a key two
hundred ranks past the last are a dozen lines, and at 200 ns a key they
were 15-20 ns apiece, L2 and L3 hits: one miss into a cold stride costs
what a dozen of them do, and the seek is not kept. What is left of the
paced burst is the visit, a form's three buffers fetched for a patch that
reads a few lines of two of them, 5.6 visits a block for lending's one
under a trail the share bounds. The `lagburst` line carries the writer's
CPU over the burst beside its wall now, and `lagcounters` its holds, and
they say the same from the writer's side (`SUPDB_LAG_COUNTERS=1 bench ab
--arms supdb-ingest,supdb-ingest --size 3000000 --passes load,lag --reps
6`): over the full rewrite at three million keys the writer's CPU is 3.2-3.5
s of a burst of 7.4-9.3, its holds 3.1-4.1 s, and over the ten-percent
burst 0.3 s of 0.7-1.4 with holds of 0.26-1.04 -- the writer's own work is
lending's burst, and the rest is the thread's throughput waited for at the
share. The one-percent burst there read bimodal, one rep in six at 110-290
ms against 24-70, and it was the hold too, 47 ms of a slow burst's 48 ms
gap: the drain the sweep's first point ends in left the merge of its pieces
running into the burst, so the pieces' keys were in no partition and the
share, a hundredth of the partitions' keys, fell short of a burst of a
hundredth by a batch or two, which held at the last commits for a pass the
merge had made long. The sweep joins that merge at its first point now
(`Engine::settle`) and the burst reads one shape, 22-48 ms in twenty-eight
bursts with holds of 0-9 ms (`CLAUDE.md`, the point that was not the store
it said).

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
landing's rebase dropped every sparse form over the rewritten partition
then (they are kept now, `Options::rebase_recuts_sparse`),
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

The thread is who now (`Options::upkeep_files_bursts`). The bound was
drawn for the mixes, whose commits are a hundred operations with reads
between them, and the thread applied it to a burst's batches of a
thousand: under a bound at three million keys, two percent of the store
is sixty thousand writes, the whole one-percent burst and the tail of the
ten-percent one, and with the lag pass divided by thirty the first scan's
settle was 14 of the pass's 26 ms at one percent and 50 of 69 at ten,
the thread lent every batch and filing none -- thirty passes, 2.7 ms --
and the cells at a quarter of LMDB's. A pass lent a commit of
`UPKEEP_WAKE_WRITES` writes or more since the thread was last caught up
settles what it was lent whatever the bound says, by the commit's own
size: the sum lent would do, and three of a mix's commits unfiled cross
it. In core the thread had been filing most of a burst anyway, the bound
being two batches at a hundred thousand keys and six at three hundred
thousand, and the pairing reads so (`supdb-deferbursts` is the arm that
waits, twelve pairs a sitting, both orders): level at a hundred thousand
keys, with the thread's passes and keys the same in both arms; at three
hundred thousand the lag scans at one, ten and all of the store 1.28x
and 1.31x, 1.37x and 1.19x, 1.13x and 1.10x, the bursts within noise, the
mixes level, and the thread 1.1x the passes and blocks it was.
Under the bound at three million keys, three pairs in each order with the
pass divided by thirty, the thread filing its bursts read two shapes: the
pass after the ten-percent burst at 8-16 ms where the thread had finished
its last pass before the scans came and 30-85 where the first scan waited
for a pass in flight, against 25-50 for the arm that waits for the bound,
whose first scan settles the residue itself; the one-percent pass 12-28
against 25-35. One sitting read the option 1.9x and 6.4x on the two
points' scans and the other 0.93x and 0.87x, which is the thread's phase
at the burst's end and not the option. A pass settles a batch and then
builds a form for every block the burst dirtied, 30-55 ms a pass here,
and the residue the first scan meets is anything from nothing to a pass.
`Options::upkeep_yields` has the pass give the upkeep back at its next
step when the writer asks, so that a reader waits for a step and not a
pass, and against the pass waited out it read the one-percent point
1.15x and 1.38x and the ten-percent point 0.62x and 0.78x, the two points'
bursts and weighted passes summed 8-10% worse in both orders: the scan
then settles a pass's worth of residue itself where the thread had
finished it with the blocks built. So the default waits, and the yield is
the arm `supdb-passyield`. What the divided pass weights thirty times is
the first scan; an undivided pass over the same store reads the eager
builds as forms its scans find built, and the quick row's lag points in
core read 1.3-2.6x of LMDB with the same engine.

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

#### The take-back's wait, measured again

The first scan after a burst takes the writer's upkeep back and waits
for the pass in flight, and a probe once put that wait at 25-56 ms on
the durable arm at three hundred thousand keys, when the pass in flight
was the rebuild a landing left -- every block of the store built fresh
-- which the conversions through the burst and the ranks taken before
the publish have since removed. Measured again in the row's shape with
the lag counters, six reps of the durable arm: at three hundred thousand
keys the scans' take phase is 0.07 ms in 21 of 24 lag points and 1.3-1.6
in three, their settle phase 0.8-2.5 ms at every point with a burst, in
passes of 6.8-11.8 ms; at a hundred thousand the tenth-rewritten point
waits 0.23-0.58 ms in every rep with a settle of nothing, and the other
points wait nothing and settle 0.3-0.5. The thread's passes over a burst
are half a millisecond each, one batch's filing, and the blocks it built
are a fraction of the store's -- 167 of 1,564 at a hundred thousand
keys, 757 of 4,693 at three hundred -- so no pass in flight is a
rebuild. What a first scan pays is the last batch's filing, either by
waiting for the thread's pass of it or, where the thread has not begun,
by settling it itself at about a microsecond a write against the
thread's 0.8; a bound on the wait -- the fresh fills yielding to the
writer's want as the conversions do -- moves that filing from the one
phase to the other and leaves the pass's sum where it is, which is what
the bounded pass had done before. The move registered for the rebuild
found none, and nothing was built; the cost it would have moved is one
batch of a burst, ten to twenty-five percent of a lag pass at these
rungs, and shrinks only as a settle does.

#### The buffered load's write path: six copies of a value, and the pages they touch

With the manifest in its slots the buffered ordered load stood at 0.72x
of the comparator at ten thousand keys and 0.77x at three hundred
thousand, its commits 2.4 ms against 1.5 for the same ten batches and
its drain 2.5 against 1.8. A profile of the writer's thread over the
ordered load's commits, cut to the window by the phase marks, put the
fault path at about forty percent of its samples -- the kernel zeroing
fresh pages alone at 26% -- against one and a half percent of the
comparator's, whose records go into page buffers it reuses and then,
by `pwrite`, into page-cache pages the kernel allocates without a fault
or a zeroing; `memmove` was 17% of ours and the kernel's copy into the
page cache 10%. Ten thousand records of a hundred and sixteen bytes
touch 3.7 MB of fresh pages here: the ordered table's arena, about 330
pages; the segment writer's piece buffer, 256; the entries, the keys,
the key section's own copy of them and the index builder's, the rest.
And a value is copied six times between the caller's slice and the
file: into the arena at the append, and at the commit into the run's
staging, its encoding, the record buffer, the batch buffer, and the
piece buffer, before the kernel copies it once more.

Three levers were priced against that split, each as an arm in one
process. Populating the buffers' pages ahead of the fill (`Options::prefault`)
is above. A piece buffer of 256 KB (`SegmentOptions::write_piece`,
`supdb-piece256` and `supdb-ingestpiece256`) took the 196 fresh pages
it should have and the commits from 2.34 ms to 2.22, and moved the load
by nothing resolvable: 1.04x and 1.03x in twenty-four pairs at ten
thousand keys, 0.98x and 0.95x in twelve at three hundred thousand; it
stays an arm, with one observation open -- the scan pass right after
the drain read 1.05-1.10x with the smaller pieces in five pairings of
five, three at p < .05, and no mechanism for it is known. The third
lever is the record path: a one-value run is the value itself under the
fixed flag, carried inline in its record, so a `put_one` on the segment
writer streams the record from the caller's slices into the batch and
the three buffers between the arena and the batch are not visited
(`Options::direct_put_one`; the three calls are `supdb-recordsteps`
and `supdb-ingestrecordsteps`). The bytes are the same, and
`tests/segwriter.rs` requires them to be, with the batch markers on
and off, over values inline, in blocks and empty. By the load's phases
the commits read 2.20 ms against 2.44 at ten thousand keys in one
sitting and 2.12 against 2.38 in another, below in 20 loads of 26, the
faults identical and the drain level; the shuffled load, which never
enters a direct run, read level in both. By rate the buffered ordered
load read 1.12x in twenty-four pairs and then 0.98x in twenty-four, and
0.94x and then 1.03x in twelve at three hundred thousand: a tenth of
the commits is four percent of the load, under what the rate resolves
at that many pairs against the drain's and the host's variance, and
the first sitting's p .007 was the position effect the lag sweep has
shown before. The path ships on the phase column, since the code it
changes is one phase's alone and nothing else in the load can have
moved; every read, scan, lag and mix cell was level in all four
pairings.

#### The table's arena as the record stream, and the folios a write leaves

The fourth lever takes the staged copy away altogether: under
`Options::direct_stream` the ordered table's arena holds each record as
the file takes it -- the key, the one-value compact extent and the
value, laid out by `flatindex::record_into` at the append -- and the
table's heads reference the value inside its record, so a read of a key
in the run in progress decodes nothing it did not before. At the commit
the segment writer accounts for each record (`put_encoded`), hashes the
batch's bytes from the arena for its marker and for the key section's
checksum row, and lays the marker into the arena behind them; the file
takes the arena's bytes directly, a whole write piece at a time as the
buffer path's pieces reached it, and the tail of a batch waits in the
arena -- it is the table's, and goes nowhere -- for the piece it
completes, or for the sync or the close. The copy count between the
caller's slice and the file is one, the kernel's. A record that would
straddle an arena block is laid in two parts and its value copied once
for the reads; a key over 65,535 bytes does not enter a run. The bytes
are the buffer path's byte for byte, which `tests/db.rs` requires of the
finished segments and of the file a crash leaves under both shapes,
with the same batches recovered from each. The device's own count of the load's writes, read around eighteen loads of ten thousand keys for the engine before the stream and after it, is 7.6 MB a load against 4.4, two fifths fewer, where the suite's dirtied-bytes quantity moved three and a half percent; the quick gate found it a fortnight later, flagged against a window of the host's own class (`bench/CLAUDE.md`, the window keyed on the host).

It took four shapes to price, and the first three lost for reasons that
had nothing to do with the copies. The first wrote and hinted at the
checksum row's piece, 16 KB, which the hashing walks: seventy-seven
`sync_file_range` calls and ninety-seven writes over a load of ten
thousand keys where the buffer path made two and three, a tenth of the
writer's thread in the virtio doorbell and a fifth more in the kernel's
copy, the commits at three times their time; `strace` counted them
after a profile had named the symbols. The second wrote the batch at
every commit, in one write of its hundred and sixteen kilobytes at
whatever offset it fell on: the commits came in under the buffer
path's, and every read over the segment trailed, by 3-9% at three
hundred thousand keys and in the read's p99 at every rung, over a file
byte for byte the same -- the kernel sizes a page-cache folio by the
write that creates it and the alignment of its offset, and twenty-two
writes of a batch left ten times the folios two writes of a megabyte
did. The third held the stream for whole pieces and lost 3% on the
reads still: nine of a 40 MB partition's forty megabytes were ladders
of 512 KB down to 4 KB where the buffer's were one folio each, read
from `/proc/kpageflags` over the mapping, because a piece that spanned
an arena block went to the file in two writes, the second at the
offset the block's end fell on. A piece goes as one `writev` now, its
arena parts the vector, and the close's tail rides the index's first
buffered write the same way, so the two shapes leave the page cache in
the same folios but for the head's eight kilobytes.

By the load's phases at ten thousand keys the commits read 1.77 ms
against 2.26 and 1.73 against 2.12 in the two sittings of the whole-piece
shape, below in 21 loads of 26, the writer's faults 919 to 668 -- the
staged copy's pages, as predicted -- its CPU 2.8 ms to 2.3, the drain
level; by rate the buffered ordered load read 1.107x and 1.118x and
1.117x in three sittings at ten thousand keys, two of them at p < .01,
and 1.137x, 1.095x and 0.958x in twelve pairs at three hundred thousand,
where the rate does not resolve a tenth against the host. With the folios the same the read
side was level for the first time: the read's p99 at ten thousand keys
read 1.02x and 0.99x in the two pairings where the three shapes before
had read 1.15-1.30x, and at three hundred thousand the point reads read
0.99x and the p99 1.01x, in twelve pairs, where the shapes before had
read 0.91x, 0.96x and 0.97x and 1.06x, 1.03x and 1.03x. The shuffled load, which never
enters a run, read level throughout. The copies are the arm now
(`supdb-copyvals` and `supdb-ingestcopyvals`), and the references into
the run's file (`supdb-refvals`) stay the third shape beside them.

#### The manifest's round trip, and the millisecond a woken thread costs here

The drain's last device round trip is the manifest's: once the seal
thread's fsync returns, the segment work writes the slot naming the new
segment and fdatasyncs it, 0.3-1.6 ms at ten thousand keys against the
segment's own fsync of about one, by `strace` over twenty-four drains. The
lever was to write the slot at the seal's readable landing, naming the
seal's segments as pending (`Options::manifest_pending`; the manifest's
third version carries a flag byte behind each name), and make it durable
with the directory's entries while the seal thread's fsync is in flight,
so that the three wait on the one journal commit that fsync forces and
the durable landing writes nothing. The invariant that no manifest is
written while a seal's segments are published and unsynced becomes a rule
at the open: a pending name is the store's only once the segment reads
back whole, its key section's checksum row and every block's checksum
over a walk of every record -- so never without block checksums, which
the suite's arms run without, where the open falls back -- and a slot
whose pending segments do not is the crash window's: the slot before it
is the store, the torn segments are swept as unnamed, and the WAL has the
writes, because under this shape a seal's WAL and temp name retire only
once a slot naming its segments durable is itself durable, the next
publish's or the close's. Only the newest slot's pending names are in
doubt, since the slot before it was written by a publish that waited for
that seal's durable landing. A reader from before the flags skips a slot
it cannot parse for one it can, and would take the older state for the
store and sweep the newer segments, so the open rewrites both slots in
the third version when it finds one of an older, the newer sequence
first. `tests/db.rs` holds the torn and the whole window, with and
without block checksums, the close, and the upgrade.

It bought nothing, and `strace` says why in three shapes. Written after
the landing's opens and publish, the slot's fdatasync began as the seal's
fsync ended and paid its own commit: the drain 2.56 to 2.44 ms, below in
10 loads of 13, the rate level. Written first and handed to a thread
spawned for its fsyncs, the slot was on the page cache 135 µs into the
seal's fsync and the thread's fdatasync began 1.16 ms later, 0.24 ms after
that fsync had ended: the drain 2.28 to 2.36. Handed to a thread kept
parked between landings and woken by a channel, the fdatasync began 1.25
ms after the write, again past the seal's fsync: 2.47 to 2.52. A thread
woken from idle on this guest reaches its first syscall about a
millisecond on, spawned or parked alike -- a vCPU halted is the host's to
wake -- so no hand-off can put the slot's sync inside a commit that lasts
a millisecond, and issuing it on the landing's own thread would hold the
publish to readers for the commit's length instead. The shape stays as
the arm `supdb-manifestpending` and `supdb-ingestmanifestpending`, the
manifest's third version stays the format, and the open's rules stay,
since a store the arm wrote opens under the default.

#### The pages a load touches, and the keys the writer kept twice

The ordered load's remaining cells against LMDB moved by tens of percent
between sittings of one tree, and the durable load at three hundred
thousand keys ran in two shapes: eight loads in thirteen at 170-260 ms
of commits with the writer's CPU at 100-170, five at 370-470 with the
CPU at 280-340, the fault count the same 17.7 thousand in every one. A
CPU-clock profile cut to each load's window on the writer named the
excess: `clear_page_erms` at 100-135 ms in the slow windows against under
three in the fast, the kernel's copy into the page cache at up to 70
against five, the buddy allocator's list walks at up to 60 -- the same
pages, zeroed and copied at a different price. A fresh page costs this
guest 0.14 µs to zero when the guest still backs it and 6-8 when the
balloon has reported the block it is in to the host, and which pages a
load gets depends on what the rep before it freed: every slow load
followed an LMDB rep, whose store of 150 MB had just been dropped, and
beside another supdb arm there was no slow shape at all. The suite makes
the pages fresh on purpose -- `release_freed_memory` trims the allocator
after every store it drops, since the ten-million rung otherwise held
13 GB in glibc's arenas -- and the glibc tunables that stop its give-back
moved nothing, since the trim undoes them; with both off the writer's
faults reached zero by the sixth load and the slow shape stayed, which is
how the partner was found.

What the engine owns is the count. A streamed run first-touched about
240 bytes of anonymous memory a key for a 116-byte record: the arena
stream (116), the segment writer's key, span, offset and hash (44), the
table's entry (38), the ordered index builder's key and start (20), the
table's key (16). LMDB first-touches its page cache and some four hundred
pages besides. The writer's two copies of every key were kept for the
close -- the key section's trailer, the ordered index and the landing's
Bloom were composed from them -- while the table it streams from already
held every key with its record and every entry in key order. With
`direct_derive_keys` the writer counts the keys and keeps the last for
the order check, and the close composes all three from the table in one
pass: `stream_trailer` takes the keys by position and fills the directory
and the hash slots as it goes, hashing each key once for the slots and
the Bloom, and tells `ordindex::Composer` each key, which writes its head
and keeps nothing. A record whose value straddled into the second arena
has no stream offset in its head, so its start is the record before it,
or that plus a marker's length when the record's header -- its key's
length, its extent count, its key -- is not there; `tests/db.rs` writes
forty thousand keys past an arena block through both shapes and holds
every segment and ordered index byte for byte.

The first version of the close read the entries five times and the keys
three -- the offsets for the directory and again for the slots, the keys
for the hashes, and the index builder's uniform pass and its heads -- and
at three hundred thousand keys the drain grew by 8.7 ms, as much as the
commits fell; fused to the one pass, the drain grew 4 and the commits
fell 8.7 and 13.6 in two sittings. The faults fell by 4.7 thousand a load
at three hundred thousand keys and 160 at ten thousand in every sitting,
the quarter predicted. By the phases' sum the ordered load read 2% and
4% faster at ten thousand keys over two sittings of twenty-four pairs
(commits -0.17 and -0.23 ms, the drain +0.08 and +0.07), 3% and 7% at
three hundred thousand buffered (unfused and fused), and 10% at three
hundred thousand durable in the fused sitting, where the copies arm met
more of the slow shape; the unfused durable sitting read the drain's
growth and no commit saving, a 5% loss by the sum, which is what the
fusing was for. No read, scan or mix cell moved outside noise in the
buffered pairings. The copies are the arm `supdb-keycopies` and
`supdb-ingestkeycopies`.

#### The drain stamped, and the manifest's sync after the answer

The buffered ordered load's drain at ten thousand keys read 2.2 to 2.7 ms
against the comparator's single fsync of 1.7 to 2.0, and the suite's
phase column could say no more than that. Stamped at every point where
its chain crosses a thread or waits on the device (`SUPDB_DRAIN_STEPS`,
one `drainsteps` line a flush and one `finishsteps` line a segment
finish), the drain decomposed as: the writer's close 0.07 ms; the seal
thread's start 0.03 to 0.2 after the hand-over; the segment's finish
0.65 to 0.9, of which the trailer's composition 0.29 to 0.47 (fifty to
eighty first-touched pages and ten thousand keys at some twenty-five
nanoseconds each), its write 0.04 to 0.13 and the superblock's three
writes 0.13 to 0.35; the ordered index's file 0.06 to 0.2; the segment's
fsync 0.9 to 1.6 beside the segment work's readable landing of 1.0 to
1.5, the two ending together; the manifest slot's write and fdatasync
0.24 to 0.56; and the writer's wake 0.03. At three hundred thousand keys
the finish is 22 to 44 ms of a 26 to 54 ms drain, the trailer's
composition 19 to 28 of it -- three hundred thousand keys at sixty to
seventy nanoseconds, which the next section takes apart; the guess made
here, the FNV a third and the slots' stores the rest, was wrong on both
counts -- and its 6 MB write 3 to 22.

Two things the stamps corrected. A thread woken here reaches its first
stamp 15 to 200 µs after the wake, spawned or parked alike; the
millisecond the manifest's helper took to its fdatasync in the pending
shape was the device's, an fdatasync queued behind the seal's fsync in
flight, not the wake. And the slot's round trip sat on the writer's
wait for no reason the durability needed: the segments are fsynced and
their directory entries durable before the slot is written, so a flush
now returns once the slot is written and the segment work fdatasyncs it
after the answer, retiring the WAL and the temp names behind that sync
(`Options::flush_waits_manifest`, off). A crash inside the window leaves
a torn or absent newest slot, which the open ignores for the slot
before it and recovers the run from its temp name, as it does for any
publish torn between write and sync; a newer publish supersedes the owed
slot; `settle` and the close sync it first. Priced against the waiting
shape in one process over three sittings of twenty-four pairs at ten
thousand keys, the publish step fell from 391 µs to 22, the drain column
by 0.13, 0.52 and 0.39 ms, and the load read 1.06x, 1.15x and 1.08x
(p .064, .023, .023); the durable load at ten thousand 1.10x and level;
three hundred thousand level, where the slot is a fiftieth of the
drain. The waiting shape is the arm `supdb-waitmanifest` and
`supdb-ingestwaitmanifest`.

The buffered drain carried one more device wait the durability never
asked for. A flush commits what is staged through `Wal::commit`, which
fdatasyncs the log whatever the policy, so a buffered store's drain
began with the whole log's sync on the writer's path before the seal
was handed over -- 2.4 ms at ten thousand keys and 3.5 at thirty, read
in the drain's stamps (`handed`) and in `strace` as the sync phase's
first syscall -- for frames the segment makes durable before the flush
is answered, in a file the landing retires. The flush commits under the
policy now, as a commit does (`Options::drain_syncs_wal`, off), and
since the log's durability was what covered the window between the
answer and the slot's fdatasync, a drain whose log holds unsynced
frames is answered behind its slot, as the waiting shape above is:
half a millisecond in most drains, two in a few. Paired in one process
against the old shape, the drain ended 0.5 to 2.2 ms earlier at ten
thousand keys in four sittings and the shuffled load read 1.32x, 0.95x,
1.12x and 1.08x -- the level sitting had the seal thread's own phases a
millisecond slower, within what this host moves them by -- and 1.09x
and 1.22x at thirty thousand (p .008 both). The ordered load has no log
to sync and did not move; the durable store's log is synced at every
commit, and its flush skips a sync the log holds nothing for. The old
shape is the arm `supdb-walsync` and `supdb-ingestwalsync`. The first
version of the wait asked whether the log's synced length was short of
its written length, and the two differ from the file's birth -- the
header is written and never synced -- so every drain of a store whose
log never syncs waited for its slot as well: a buffered store's direct
runs, and a durable store's, whose records sync in the run's own file.
The stamps read it as 0.4 to 0.9 ms between the landing and the publish
of every direct run's drain, where the shape before had read 0.05, and
the ordered load at ten thousand keys fell to 0.90x of LMDB in one
sitting; the wait asks whether the log holds frames past its header
now (`Wal::frames_unsynced`). The stamps
cover the hashed table's seal now as well (`sorted`, `snapped`, and the
finish's steps), and they put the rest of that drain at ten thousand
keys in the seal thread: the sort 0.4 ms, the snapshot's copy 1.0 to
1.5, the records and the finish 2.5 to 3.3, the segment's fsync 1.6
beside the directory's, and the slot 0.5.

The direct run's drain had never been stamped under the durable policy:
every figure above is the buffered arm's, and the one durable figure,
2.1 ms, predates the slots. One sitting of each arm against itself at
ten thousand keys, twenty-six drains each, puts the durable drain at
1.87 ms and the buffered at 1.94, and splits them the same way, with a
stamp at the landing's publish to readers (`readers`) that is new. The
writer's work before the hand-off is 0.11 ms. The presync is nothing
under the durable policy, where every commit synced the run, and 0.85
ms for the buffered run's 1.2 MB, which the seal thread's finish hides
whole. The finish is 0.98 ms: the trailer composed in 0.34, the tail
and the trailer's 360 KB written in 0.45, the superblock's three
patches in 0.11. The landing opens and publishes the segment 0.17 ms
after it is linked, and from there the trailer's fsync, 0.52 ms
durable and 0.62 buffered, beside the directory's on the segment
work's thread, then the join, the slot's write and the reply, 0.13,
hold the answer: 0.48 ms durable and 0.58 buffered past the point
readers have the segment. Under the durable policy that wait is the
trailer's durability alone, since the records were durable at every
commit and recovery reads them from the temp name's markers; the
finish, two thirds of it the trailer's write and the superblock's
patches rather than its composition, is the rest of the chain and
within the order. The commits beside it read 6.9 ms durable, the ten
fdatasyncs 0.49 to 0.54 ms each in `strace`, and 2.0 buffered. The
inline finish (`supdb-closeinline`) read the sync column within 0.1 ms
and the commits 0.85 ms apart in opposite directions in the two orders,
a position effect, and stays an arm. What the stamps leave within the
contract is the finish's write and patches; what they leave past it is
an answer at the readable landing for the durable policy, which the
crash-discipline paragraph does not grant and the owner decides.

The snapshot's copy was the next thing taken away, and it came back. A
drain's seal copies the frozen table's chains into a snapshot for the
scans over the store while the seal runs, as any seal does, and under a
drain the writer waits and reads nothing, so the copy read as a cost
paid for nobody: fresh pages at the host's price, 1.0 to 1.5 ms at ten
thousand keys and 5 to 7 at thirty. Without it the seal writes its
records from the table's chains, given the copy's own lookahead, and
the records phase grew by what the copy had cost at both rungs -- 2.8
to 4.1 and 4.7 ms at ten thousand, 8.4 to 11.7 and 13.8 at thirty --
with the drain's end level in four sittings (the shuffled load 0.94x,
1.08x, 1.09x and 0.98x against the copy, none resolved) and the
durable drain 0.9 ms longer. So the copy is the cheaper of two walks
over the table: one pass with its lookahead and a sequential read after
it, against a walk interleaved with the writer's work on every record;
its pages were not the price. The shape without it is the arm
`supdb-nodrainsnap` and `supdb-ingestnodrainsnap`
(`Options::drain_snapshot`). What is left in that phase is the
writer's own work on a record, some 230 ns of it at ten thousand keys
from a sequential arena, which the next decomposition takes apart.

Callgrind over the seal's job (`--toggle-collect` on the job's own
symbol, with `--demangle=no`, since the toggle never matched the
demangled closure's name) counted some 2,350 instructions a record at
ten thousand keys: 600 in the snapshot's copy, five small `memcpy`
calls and three arena resolutions a record; 950 in the record writer,
of which `end_with` 800 -- the run encoded from the staged value, the
record from the run, and the record copied into the aligned buffer
with a checksum call a record; 290 in the finish; the rest in the
sort, the Bloom and the ordered index. A key whose chain holds one
value and no tombstone, every key of a load, goes through the
writer's one-value path now (`Options::seal_put_one`,
`SegmentWriter::put_one`), the record laid out once from the value:
172 instructions a record fewer by count, 7% of the job, and the
records phase at thirty thousand keys 1.1 and 2.5 ms shorter in two
sittings with the drain's end 1.3 to 1.5 ms earlier; at ten thousand
the phase moved within the host's noise and no rate resolved either
rung. The general path for every key is the arm `supdb-sealrun` and
`supdb-ingestsealrun`, and a test holds the two to the same bytes.

The direct run's close was taken apart the same way, with the stamps
and the loads' phases at ten thousand keys: the seal thread's start 0.2
ms after the flush's entry, its finish 1.1 to 1.3 beside the writer's
fdatasync of the records of 1.4, the ordered index and the link 0.15,
the trailer's fsync 0.8 to 1.0, the landing and the slot 0.1 -- 2.6 to
2.8 in all, against the comparator's one fdatasync of 2.3 to 3.0. Two
changes were priced on it, and both came back. The writer's helper,
which syncs a segment behind the writer, starts only once the file is
eight megabytes long (`SegmentOptions::sync_ahead`), so every run under
that is synced whole at its close; at a megabyte the close's sync at
thirty thousand keys fell from 3.0 ms to 0.5 and the load read 1.10x
and 1.16x against the old threshold, neither resolved by the pairs, with
a hundred thousand and three hundred thousand level in the drain and in
the commits -- but at ten thousand the one sync the helper made began at
the run's last piece, the close's own sync queued behind it and the
finish's join of the thread waited for it, 0.3 ms on the drain; and at
two megabytes, no thread for a ten-thousand-key run, the close's sync
at thirty thousand fell to 1.3 ms while the finish ended 1.5 to 2 ms
later with the thread beside it, the load 0.87x and 0.89x of the old
threshold in two sittings. The threshold stays at eight; what the
thread's sync saves the close, its presence beside the writer's last
pieces and the finish costs more at those sizes, by a mechanism the
stamps did not name. The finish on the writer's own thread, warm from
the load, with the records' fdatasync on a thread of its own
(`Options::close_finish_inline`), was the other, and it read the other
way too: the finish took 1.8 to 1.9 ms on the writer against 1.1 to 1.3
on the seal thread, because the trailer's write waits for the journal
commit the records' fdatasync forces, on whichever thread writes it, so
the finish ends when the sync ends in either shape and its composition
was never the critical path; and the seal thread, spawned after the
finish, wrote the ordered index 0.3 ms later than before. The drain
read 3.5 ms against 2.8, and the shape is the arm `supdb-closeinline`
and `supdb-ingestcloseinline`. What bounds the drain at that rung is
the device: the records' sync and the trailer's, two round trips of a
file the load has just written, and the second cannot begin before the
first is through.

The scan pass at ten thousand keys read level with LMDB on this host,
and the harness times every scan of the pass on its own now
(`SUPDB_SCAN_OPS=1`, a `scanops` line a pass for every arm with the
first scans beside the median; the pass is marked for a profile too).
It put the first scan of ours at 18 to 29 µs at ten thousand keys and 32
to 51 at thirty, against 2 to 6 for the scans after it and 6 to 11 for
the comparator's first, in a pass of 250 to 500 µs. A probe over the
store the load leaves, with the engine's scan timers, split the first
scan into its phases and callgrind counted it at 42,000 instructions
against 7,600 for the second: 18,000 of them a radix sort over the
empty memtable, whose masks of the bytes that vary among the records
read every byte as varying over no records and ran sixteen histogram
passes for nothing, in the snapshot the first scan builds over a store
just drained; and 14,000 in the block table's allocations, a loop
writing `None` per block twice. The sort returns for a run of fewer
than two and the slots are allocated as a run; the first scan counts
23,000 now. In the suite's shape its column did not move within its
spread, 21 to 29 µs at ten thousand keys in two sittings after as
before, since the sort was a fifth of it by count and the rest is cold
lines -- the log read, the snapshot, the table's seeks and the walk's
own -- that the comparator pays its own share of; the pass read 1.35x
and 1.23x against LMDB at ten and thirty thousand keys in those
sittings, where it had read level in one before, which is the host's
spread between sittings and not the sort's. The count is the verdict.

#### The finish's fills, asked ahead

The segment finish's trailer -- fences, directory, hash slots, composed
in one pass over the keys -- read 0.47 ms at ten thousand keys and 18
at three hundred thousand in the streamed run's close, 0.16 and 8 in a
seal whose writer kept its keys, and the stamps could say no more than
that. Four instruments said where it went. The time fitted against the
finish's first-touched pages across every drain of a sitting is 394 µs
plus 2.2 µs a page at ten thousand keys and 15.8 ms plus 2.9 a page at
three hundred thousand: forty and fifty-three nanoseconds a key of work,
the rest the host's price for fifty and eleven hundred pages. Callgrind
over the finish counts 376 instructions a key, a hundred and sixty-seven
of them in the key source's read -- the FNV sixty-five, the record's
start forty-one -- ninety-five in the closure that adds the key to the
Bloom and tells the composer, fifty-three in the composer, thirty-six in
the slot fill. A micro-probe of the FNV over the suite's sixteen-byte
keys reads 4 ns a key, one key at a time or four interleaved alike, and
the Bloom's modulo 1.5: neither is the twenty nanoseconds the notes had
given the FNV. And perf over the finish at three hundred thousand keys
puts four fifths of the fill loop's samples on the branch after the
slot probe's load and a third of the closure's on the first store into
the Bloom's block: two random loads a key, the slot in a four-megabyte
table and the block in a filter of three hundred and seventy kilobytes,
each missing the core's own cache while the records, keys and entries
stream past it. A micro-probe of the two fills over the same shapes
reads 15 ns a key at three hundred thousand and 11 with the slot and the
block hinted sixteen keys ahead, 10 and 9.7 at ten thousand, where both
tables sit in the second-level cache.

So the fills ask ahead (`SegmentOptions::lookahead`): the slot fill
takes each key sixteen before its slot is probed and hints the home
slot meanwhile, placing the keys in the order given so the section is
the plain fill's bytes, which a unit test holds; the Bloom's fill holds
sixteen FNV words in a ring, hints each word's block as it arrives and
sets the bits of the word sixteen back, the filter the same whatever
order its bits are set in. Both writers of the trailer take it, the
streamed run's close and the seal that kept its keys, and the landing's
Bloom from kept hashes. Priced against the plain fills in one process
over two sittings of twelve pairs, the arms' order swapped between them,
the streamed close's trailer at three hundred thousand keys read 4.0 and
4.8 ms less within pairs buffered (10 and 11 of 12 pairs) and 3.1 and
6.6 less durable (11 and 9 of 12), the drain 3.7 and 5.1 less buffered
and 4.8 and 4.3 durable, and the load's rate followed in sign without
reaching a verdict (1.03x and 1.10x buffered; the durable rate split,
0.91x then 1.10x, in a sitting whose shuffled cell read 0.87x at p .04
and then 1.05x -- the position effect the rules name). At ten thousand
keys the trailer read level in the buffered pairings and 0 and 35 µs
less in the durable, as the probe said it would. The plain fills are the
arm `supdb-fillplain` and `supdb-ingestfillplain`.

What the fits also say is what the fills do not touch: the pages. The
trailer's buffer is fresh anonymous memory at every finish, fifty pages
at ten thousand keys and thirteen hundred at three hundred thousand,
and the finish pays the host's price for each -- a fiftieth of the
trailer at ten thousand keys where the guest still backs the pages, a
quarter to a half where the host does. That is the next lever, and it is
sized by the pages, not by the keys.

#### The records' sync beside the finish

With the trailer's fills asking ahead, the buffered ordered load's drain
at ten thousand keys read about two milliseconds in two serial parts:
the seal thread's finish, ordered index and link, 0.85 ms of CPU with
the device idle, and then the segment's fsync, 1.1 ms of device with
the CPU idle, the directory fsync the landing makes running beside it
on one journal commit. Nothing had been synced before the finish: the
helper that syncs a segment behind its writer starts once eight
megabytes are unsynced, and a run of ten thousand keys is a megabyte
and a bit. The device's fdatasync, probed on fresh files with the
pieces hinted as the writer hints them, costs a third of a millisecond
and then about 0.85 ms a megabyte -- 0.33 for four kilobytes, 1.4 for a
megabyte, 6.9 for eight -- and 0.03 with nothing dirty, so the flush is
priced by what is in the device's write-back cache and a redundant sync
costs nothing. The probe of the drain's own shape, the records' sync
issued on a thread as the CPU work began, read 14% less than the serial
shape with the final sync at a third of its length; the trailer's write
stalls under the concurrent sync in some reps, 0.85 ms, on the
journal's locked phase and not on allocation, since preallocating the
file changed nothing.

So a flush's close syncs the records first (`Options::close_presync`):
the writer, which waits for the drain anyway, takes a second handle on
the run's file before the seal thread is given the writer, hands the
seal, and fdatasyncs the records on its own thread while the seal
thread composes the trailer; the seal's fsync then flushes the trailer
alone, and the landing's directory fsync beside it the same. Durability
moves nowhere: the fsync the landing waits for is still made, over
less. A table handed to a seal without a flush (`hand_tail`) does not
presync, since that writer is not waiting. Priced against the late sync
in one process at ten thousand keys buffered, the drain read 1.9 ms
against 2.9 within the same sitting, 0.71 ms less within pairs in 21 of
24, the data fsync 0.56 against 1.05 and the landing 0.65 against 1.14,
and the load 1.18x (21 of 24, p < .001); the second sitting, arms
swapped, 1.74 against 2.37, 0.51 ms less in 22 of 24, the fsync 0.54
against 0.95, the load 1.15x (19 of 24, p .007). Against the comparator
in the same sittings the buffered load at ten thousand keys read 1.01x
twice, level for the first time. At three hundred thousand keys the
final fsync did not move, 3.3 ms either way: there the dirty bytes at
the fsync are the trailer's own, six megabytes of directory and slots
the finish has just written, and the presync covers only the records the
helper had not yet synced, two milliseconds under a fifteen-millisecond
finish -- the drain 3.1 ms less within pairs in 7 of 12 and then 2.8
more in 8 of 12, the load level both times. The durable arm, whose
records every commit fdatasyncs, pays the redundant sync 50 µs on the
writer's path and reads level at ten thousand keys (the drain 18 µs
less in 13 of 24, then 97 more in 17 of 24); at three hundred thousand
its drain is level within pairs (6 of 12 both sittings) while its rate
read 0.91x and 0.95x (p .15, .04) with the difference in the commits'
column, which the change does not touch and the host's page price moves
by half between reps -- noted as unresolved, as the rules have it. The
late sync is the arm `supdb-latesync` and `supdb-ingestlatesync`.

#### The record stream in a ring

A buffered ordered load of three hundred thousand keys read level with
the comparator in one sitting and at half its rate in the next, and the
variance was the loss. Every minor fault of a pairing recorded with its
instruction (`perf record -e minor-faults -c 1`) put the writer's load at
thirteen thousand first touches of anonymous memory -- 178 bytes a key:
the record stream 116, the slab entry 38, the key's copy 16 -- against
the comparator's four hundred, whose dirty pages come from a pool it
reuses and reach the file by `write`; in a dear sitting the same load's
commits read 147 to 370 ms against the comparator's 51 to 63 at the same
fault count, seven to twenty-four microseconds a page, while the
kernel's copies into the page cache, which both engines make for the
same forty megabytes, cost it nothing of the kind. So the lever was the
comparator's own shape: a bounded, reused footprint in front of `write`,
and not the stream's thirty-five megabytes touched once each. The
reference-holding table (`Options::direct_table_refs`), measured the same
day, took a fifth of the faults off and read slower, its staging arenas
rotating at a piece and not recycled.

The stream's arena runs in a ring now (`Options::direct_recycle`): its
blocks are a megabyte, the writer's piece, and at each commit, once the
batch is on the file, every block the file holds whole is taken out of
the arena under a fresh epoch -- its pointer nulled, so a reader that
finds the block gone reads the record through the run's mapping, the
one the reference-holding table reads every record through, at the
stream's base plus its offset -- and its memory is handed out again for
the next block once no reader is pinned at or before that epoch. A
reader that loaded the pointer first is pinned below the epoch, and the
memory is reused only past every such pin, so the invariant that
nothing a reader follows moves holds by the epoch rather than by never
freeing. The ring is two live blocks and the ones awaiting their epoch,
a few megabytes whatever the run grows to; the reserve for the offsets
stays what it was, so a run addresses four gigabytes of stream.

Priced against the whole arena in one process over two sittings with
the arms' order swapped, the writer's faults a load fell from 13,050 to 5,030 at three hundred
thousand keys and from 4,390 to 2,130 at a hundred thousand in every
load of both, and stayed at five hundred at ten thousand, where the run
fits the ring; the commits' column read 40 ms against 53 and 41 against
54 at three hundred thousand buffered, the load 1.22x (10 of 12, p .04)
and then 1.05x (9 of 12), the durable load 1.15x (p .04) and 1.10x, the
hundred-thousand load 1.11x and 1.10x, ten thousand level throughout,
all in cheap sittings where a page cost under two microseconds. One cost
moved rather than vanished, and the suite's own shape shows it: the
mixes that follow a load in the same store had been finding the thirty-
five megabytes the load first-touched warm in the heap, and with the
ring they first-touch their own -- the mix counters read seven hundred
and seventy faults in the update mix's thread where the whole arena's
had read none, a thousand pages over the mixes against the eight
thousand the load no longer touches -- so the update-heavy mixes and
the rewriting lag point at three hundred thousand keys read a few
percent behind the arm, cells this engine leads the comparator by half
to twice over, and the sum of the phases over a store is near level in a
cheap sitting. What the ring wins is the load against the comparator in
a dear sitting, where the thirteen thousand first touches had cost it
half to two thirds of its rate and the comparator's four hundred cost it
nothing, and a run's footprint of a few megabytes in place of its whole
length. The whole arena is the arm `supdb-norecycle` and
`supdb-ingestnorecycle`.

#### The keys read in their records

With the stream in a ring, a buffered ordered load of three hundred
thousand keys first-touched five thousand pages against the comparator's
four hundred, and the fault record by symbol put twelve hundred of them
in the table's copy of every key: sixteen bytes a key, kept beside a
record that holds the same bytes four in from its start. The copy served
the lookups a run in progress answers, the scan snapshot's build, the
seal's sort and the close's one pass over the keys -- the trailer's slots,
the Bloom and the ordered index's heads -- and every one of them can read
the key where the record holds it.

An entry of a streamed table names its key in the stream now
(`Options::direct_keys_in_stream`): the record's offset plus the four
bytes of its header, read through the arena's block while the block is
live and through the run's mapping once the ring has taken the block
out, as a value is; and the record's own offset, which the close asks
for every key, is the key's less the header, where before it was worked
back from the head. Two kinds of key keep a copy: one whose record runs
across two of the arena's blocks with the key on the seam, since the
blocks are not contiguous and a read across a seam copies out, and one
past two gigabytes of a run, since the key offset is a word of
thirty-two bits; both are marked by the word's top bit, which no stream
offset below the limit carries. A seam falls inside a key once in about
every ten megabytes of the suite's records, a few keys in a load of
three hundred thousand, and a unit test puts one on the first seam in
both record shapes. The segment a run closes is byte for byte what the
copies composed; the test that held the derived close to the copies'
bytes drives a third store this way and compares it the same.

Priced against the copies in one process over two sittings with the
arms' order swapped, the writer's faults a load fell from 5,050 to 3,880
at three hundred thousand keys, 2,120 to 1,730 at a hundred thousand and
495 to 455 at ten thousand -- the copy's pages, in every load of both
sittings -- and the commits with them, 36.5 to 33.0 ms and 36.8 to 33.4,
12.6 to 11.4 and 12.8 to 11.5, 1.51 to 1.40 and 1.52 to 1.44. The close
pays for part of it: its one pass reads each key out of a record of a
hundred and thirty bytes instead of a copy of sixteen, so it streams the
record section through the mapping where it streamed an eighth of that
through the heap, and the trailer step read 15.8 ms against 13.1 and
16.2 against 14.0 at three hundred thousand buffered, in every pair of
the second sitting, 4.9 against 4.3 at a hundred thousand, level at ten
thousand, with the step's faults the same in both arms where the
mapping's pages were mapped sixteen a fault and up to eighteen hundred
more in one durable sitting where they were not. The composer needs the
key's bytes at the first and last keys' common prefix, which is known
only at the close, so a hash kept in the entry would spare the pass
nothing. And a third of the pages the load no longer touches are touched
by the first update mix after it in the same store, as the ring's were:
the mix's thread read 670 faults against 60, and ycsb-A at three hundred
thousand buffered read 0.97x, 0.95x and 0.95x of the copies' arm over
three pairings, a cell this engine leads the comparator by nearly three
to one, while the read-only mixes read level. The load's sum of phases
read 3.46 ms against 3.59 and 3.56 against 3.68 at ten thousand, 30.7
against 31.5 and 31.7 against 31.8 at a hundred thousand, 78.8 against
81.9 and 83.8 against 81.2 at three hundred thousand; the load 1.04x in
both sittings at ten thousand (33 of 48 pairs), 1.04x and 1.00x at a
hundred thousand, 1.03x, 0.95x and 1.08x at three hundred thousand over
three pairings, and the durable load 1.02x twice at ten thousand and
1.01x and 1.03x at three hundred thousand, all in sittings where a fresh
page cost about three microseconds, which makes the pages and the
bandwidth near a wash there; in a dear sitting the twelve hundred pages
cost seven to twenty-four microseconds each and the pass's bandwidth
costs what it costs here, which is the case the change is for. The
copies are the arm `supdb-keysarena` and `supdb-ingestkeysarena`.

#### The entry in three words

What a load first-touched after the keys moved into their records was
the entries first: thirty-two bytes a key, a sixty-four-bit hash, two
thirty-two-bit words for the key's span, the head and a sixty-four-bit
count, twenty-four hundred pages of the thirty-nine hundred at three
hundred thousand keys. The hash index derived a slot from the hash's low
half and a slot's tag from its high half, and kept the whole hash in the
entry so that a rebuild could reinsert by it and a probe could compare
it before the key. An entry keeps the high half alone now, the slot is
indexed by that same half (`slot_of`), and the probe's compare of the
entry's hash is gone, since the slot's tag is the same test; the key's
length is sixteen bits, which is what a record's length word allows and
every write checks; the count is thirty-two bits, more values of one key
than the arena addresses. Twenty-four bytes, asserted by a test, no arm
-- two layouts of the entry cannot share a process -- and the faults
are the verdict: the writer's faults a load read 3,270 against the 3,860 to 3,880
of the three pairings before, 1,550 against 1,720 to 1,740 and 437
against 454 to 457 at three hundred thousand, a hundred thousand and
ten thousand keys buffered, 3,050 against 3,600 and 453 against 468
durable -- the eight bytes a key, at every rung -- with the commits
level, 32.5 ms at three hundred thousand against 33.0 to 33.4, in the
same sitting as the pairings above. The hash tables the mixes write
take the same entry and the same index, with the slot drawn from the
tag's half; their cells are the gate's to watch.

#### The ordered table's one-word entry

An ordered table's entry was the hash table's: a twenty-four byte record
of the hash's high half, the key's span, the chain's head and its count,
each first-touched once as the load appends it -- 1,755 pages of a three
hundred thousand key load's 3,300, more than half. An ordered table has
no index, so the hash is read by nothing; its keys sit in their records
in the stream, so the key's span is the record's offset plus four; it
takes one value a key, so the count is one; and the head is the record's
tail, which the record's layout gives from the key's length and the
value's. The entry is one word now (`Options::direct_slim_entries`,
`MemTable::slim`): the record's offset in the stream in the high half,
the key's length and the value's in the low, and `MemTable::entry`
materialises the head and the span from it in arithmetic; a record that
straddles a block of the stream, a key copied past the bias or a value
longer than the word's field keeps a fat entry, which the word names
behind a flag bit. Readers take every entry through one view (`Entry`),
whichever shape the table keeps it in; the writer's stores -- a hash
table's chain head and count, the reference arm's rewritten heads -- go
to the fat entry itself (`MemTable::fat`), which an ordered table never
reaches. The arm `supdb-fatentries` and `supdb-ingestfatentries` keeps
every entry twenty-four bytes. Priced in one
process over two sittings with the arms' order swapped: the writer's
first touches per ordered load 2,990 to 1,820 at three hundred thousand
keys, 1,440 to 1,050 at a hundred thousand and 440 to 400 at ten
thousand -- sixteen bytes a key -- in every pair of both sittings; the
ordered load's commits by median 2% and 25% shorter at the top rung,
where the fat entry drew the second sitting's dear reps, 6-8% at the
two below, in both sittings; the load's rate 1.05x and 0.98x at the
top rung, 1.02x and 1.04x, 1.04x and 1.03x below; the cells on hash
tables, which the word does not touch, scattered with the second-named
arm behind in both sittings, as every pairing's do.

#### The ordered table's write log as a range

A handle reads a memtable's write log to list the slots created since
its snapshot, and the log is a slab of one word a write. An ordered
table takes each key above its last and nothing else -- no put, no
delete, no second value -- so every write to one is a new entry and
write `i` is entry `i`: the log held the sequence 0, 1, 2 and so on, a
fresh page every thousand writes, a tenth of a three hundred thousand
key load's first touches, for what the entry count already said. The
log is the entries' range now (`Options::direct_log_range`): `log_len`
is the entry count, `log_at(i)` is `(i, new)`, nothing is pushed, and
the mark a commit publishes carries the count under both names. The arm
`supdb-ordlog` and `supdb-ingestordlog` keeps the slab. Priced in one
process over two sittings with the arms' order swapped and a third with
the mixes timed by kind: the writer's first touches a load 3,300 to
3,000 at three hundred thousand keys, 1,540 to 1,440 at a hundred
thousand, 450 to 440 at ten thousand, in every pair; the load's rate
level, 1.02x and 0.98x by sitting at the top rung, as a fifth of a
millisecond among forty must be; every read cell level, and the one mix
that read 7% one way in two sittings read 9% the other in the third.
The truncation of a run's uncommitted tail, which takes the entries
back to the last commit when a smaller key leaves the run, left the
slab's tail standing; the table takes no write after it, so nothing
read it, and the range has no tail to leave.

#### The upkeep thread kept alert

ycsb-E at ten and thirty thousand keys read 0.90x to 0.97x of the
comparator in every row of a fortnight while the durable store read it
at 1.1x to 1.3x, and a timer on each kind of operation in the mix
(`SUPDB_MIX_KINDS`, both arms alike) put the whole gap in the scans:
1,585 scans of fifty at 691 ns against 633 at ten thousand keys, the
mix's one commit level. The same arms read our hundred-entry scans on
the drained store 1.21x the comparator's, and the durable store's E
scans ran at 532 ns on the same keys; what the buffered scans paid was
the store the mixes before E had left. The mix counters said what: at
ten thousand keys E's scans built a hundred and fifty blocks, every
block of the store, where the durable store's E built sixteen, because
the durable store's upkeep thread had built them during the update
mixes, six passes beside A's fsyncs and nine beside F's, and the
buffered store's thread had run no pass at all. A batch of a hundred
writes, the mixes' size, is lent to the thread but is below the batch a
commit wakes it for, and the thread's back-off, doubled from a hundred
microseconds to twenty milliseconds across the read passes before the
mixes, slept through the two milliseconds the mixes take. The arm that
lends the bursts' filing too made E's scans slower, 709 ns, and the lag
cells a third of what they were, and the option that wakes the thread
for any batch once it is parked over a millisecond had been priced and
turned off: E 1.18x, A and F 0.85x, the wake being a VM exit here.

The thread stays alert now (`Options::upkeep_alert`): for a hundred
milliseconds after its last pass or hand-over it parks for at most two
hundred microseconds at a time while reads are around, on its own timer
and its own core, and climbs its back-off only past that; and the commit
that finds reads newly around, or finds the thread parked longer than
its poll, wakes it once, since a thread chooses a park's length only when
it parks and the first shape of this slept through three mixes in five
inside a park it had chosen before the reads began. With it the thread
runs four to six passes beside A and two to six beside F at ten thousand
keys in every rep, E builds sixteen blocks, and E's scans run at 526 ns.
Two probes priced what a thread beside the writer costs on this guest
before the shape was chosen: a spinning thread on another core slows an
L2-resident dependent-load loop on the writer's by nothing and a
memory-bound one by nothing, a streaming one by five percent, and a
thread that wakes every two hundred microseconds for twenty of work the
same; the writer's own wake is what the earlier option paid for, and
this shape pays for none. What remains is what the pass's own work
costs the writer beside it -- its reads of the memtable lines the writer
is writing, re-owned at the next write -- which the kinds timer reads
as about five microseconds a commit of a hundred updates and fifteen to
twenty-five nanoseconds a point read while a pass runs, on the mixes
that write.

Priced against the back-off as it was over two sittings with the arms'
order swapped, the timer by kind on for every arm: E's scans at ten
thousand keys read 564 ns against 705 and then 524 against 667, at
thirty thousand 556 against 718 and then 562 against 715; the cell 1.22x
and 1.20x in the first sitting (p .004, .039) and 1.23x at both in the
second, every pair of sixteen and of twelve; against the comparator with
it, E 1.19x to 1.20x and 1.14x to 1.21x at the two rungs where the rows
had read 0.90x to 0.97x, the burst's updates at the one percent lag
point 1.8x and its scans level to 1.7x, and no other buffered cell
behind the comparator at either rung. The mixes that write beside the
passes pay for the filing: A at ten thousand keys 0.532 ms against 0.440
and F 0.642 against 0.512 in the first sitting, about nine microseconds
a commit of a hundred updates and twenty nanoseconds a point read, B and
D within a few percent, the same at thirty thousand; at a hundred
thousand the two sittings disagree in sign on A and on the lag points
within eight pairs' noise, with E's scans level to the nanosecond, and
at three hundred thousand nothing moved or the update mixes gained in
both -- so A and F at the small rungs give back a tenth to a fifth of a
lead of 1.4x to 2.3x over the comparator, which is where a loss against
our own rows will read in the quick row's gate. What that tax is was
bisected by experiment: a pass that pins, parks for fifty microseconds
and leaves costs A four percent and the reads nothing; one that files
and builds nothing costs the whole of it with a third of E's gain; the
full pass costs the same and gives E the rest. The filing re-reads the
hot entries the writer's zipfian updates are rewriting, and each re-read
is an ownership the next update pays for, beside the memtable streamed
through the shared cache. The poll's length does not move it, two
hundred, five hundred or a thousand microseconds alike, because a thread
once awake runs a pass per posted commit at its shortest poll; the
upkeep's batch option, which waits for five percent of the store's keys
before a pass, trades a twentieth of E's gain for A level and F still a
fifth behind, and three cache-line separations -- the arena's writer
words, the state's forms-bytes counter, the pins' walk at an operation's
end -- each moved nothing and were taken out again. The durable store's
thread has paid the same beside the fsyncs since the upkeep was lent,
with no cell to show it against. The back-off as it was is the arm
`supdb-noalert` and `supdb-ingestnoalert`.

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

The finish was then timed step by step. At ten thousand keys it is
530-890 µs: the key section's trailer built in 80-250, the checksum row
in 280-365, the writer's last buffer flushed in 120-360, everything else
under ten; at three hundred thousand it is 7-8 ms, five of them the
trailer's build and two its write, the row 0.1 since the helper thread
hashes pieces as they land on a file that long. The row at the small
rungs was every piece read back through eighty preads and hashed, on a
segment too small for the helper to have started. Hashing the pieces on
the writer as their bytes were buffered, hot, took the finish's 0.3 ms
and put it in the commits -- sync 2.57 against 2.82 ms, commits 2.31
against 2.17, the load level at ten thousand keys and 0.3 ms slower at
thirty -- because the CRC costs the same wherever it runs and only the
preads were the finish's own; the writer was hashing every record twice,
once for the commit marker and once for the row. One pass yields both
when the pieces' CRCs are combined into the batch's
(`block::crc32_combine`, zlib's shift matrices built once), so the
marking writer folds its row out of the marker's pass and its helper
only syncs (`SegmentOptions::row_on_write`; the row hashed at the close
is `supdb-rowatclose` and `supdb-ingestrowatclose`). In pairs of
twenty-four the buffered ordered load read 1.06x at ten thousand keys
(17 of 24) and 1.06x at thirty (21 of 24, p < .001), the durable load
1.04x at ten thousand, with the commits level and the drain 0.3-0.8 ms
shorter; at three hundred thousand, twelve pairs, the load read 0.97x
with nothing resolved. The cells after the drain moved the other way at
the small rungs: the lag pass with no updates read 1.26x and 1.23x in
the arm that still reads the section back, 24 and 22 of 24, and the
point reads 1.14x in the durable arm, because the read-back had just
pulled the whole section through the cache for them. Those passes are a
hundred scans or reads over a megabyte, forty microseconds against the
three hundred the read-back cost, so the sum is the drain's, and the
cell is read beside the step that warmed it.

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
level. The references are the arm, and the copies were the shape that
shipped until the table's arena became the record stream (below). What
is left of the table -- its entries at thirty-two bytes a key, its keys, the staged
copy -- is as much as the arena was, and the shape not yet priced is a
run that keeps no table, serving its reads from the segment writer's own
sorted keys and record offsets. Priced again at ten thousand keys once
the manifest had its slots, by the load's phases in twelve pairs: the
commits 2.5 ms against 2.4, below the copies in 2 loads of 13, with
1,015 faults against 930 -- the mapping's pages and the arenas' both --
and the load 0.96x in 24 pairs, 1.04x in 12 at three hundred thousand,
every other cell level. At the small rung the mapping costs what the
copy did, and the arm stays an arm.


#### The manifest in two slots

The drain's publish was the next round trips on it. A publish wrote the
manifest to a temp name, fsynced it, linked the old manifest under a
spare name, renamed the new one over it and fsynced the directory: two
device round trips after the segment's own fsync, 0.6 ms for the temp
file's fsync and 0.4 for the directory's in the drain's strace at ten
thousand keys, and the spare's unlink later, a discard of 1.2-3.8 ms of
device time off the path. The manifest is two slot files now, `manifest`
and `manifest.alt`: each publish is a sequence past the last and writes
the slot of the sequence's parity in place, fdatasynced and never
truncated, and the open takes the newest slot whose checksum holds, a
torn write being the slot it ignores. Both slots exist from the store's
birth or its first open under this, so no publish creates a directory
entry (`Options::manifest_slots`; the rename shape is
`supdb-manifestrename` and `supdb-ingestmanifestrename`). The header
carries the sequence and a version-2 magic, the checksum covers the
sequence with the body, and a version-1 manifest reads as sequence zero.

The first version dropped the directory fsync as the manifest's own, and
it was the segment's too: a seal's job leaves its rename, and a direct
run's close its link, to the publish that names them, so a slot written
without it would have retired a WAL behind a name that was not durable.
The fsync runs at the seal's readable landing now, on the segment work's
thread once the readers have the segments, while the seal's thread
fsyncs their data -- and on this filesystem the two wait on one journal
commit: in the drain's strace the directory's fsync ends 0.1 ms after
the segment's in both of the load's drains, 0.2 ms of wall for the
ordered load's where it had cost 0.4 as a round trip of its own after
the manifest's, and the publish is then one fdatasync of 0.3 ms. A
merge's and a promotion's landing fsync the directory themselves before
their publish, as they did. Priced by the load's phases in twelve pairs
at ten thousand keys: the ordered drain's median 2.5 ms against 3.0,
below the rename shape in 11 of 13 loads, the shuffled 5.9 against 6.4
in 10 of 13, the commits identical at 2.4. By rate, the buffered ordered
load read 1.04x in twenty-four pairs and 1.07x in twelve at ten thousand
keys and 1.11x in twelve at three hundred thousand, none resolved at p
.05, since the commits' faults move a load by more than its drain; the
durable load 1.01x, and every read, scan, lag and mix cell level.

#### Pages populated ahead of the fill

The commits were then the other half of the buffered load's loss at ten
thousand keys, 2.4 ms against the comparator's 1.5 for the same ten
batches, and the writer's faults said where: 925 against 45. Ten
thousand records of a hundred and sixteen bytes are 1.16 MB, and the
load touches 3.7 MB of fresh pages for them -- the ordered table's arena
(about 330 pages, each key and value copied in), the segment writer's
piece buffer (256, a megabyte touched once), the entries, the key
section's buffer -- where the comparator's records go by `pwrite` into
page-cache pages the kernel allocates without a fault or a zeroing. A
probe priced the alternatives on this guest: a fresh anonymous page
costs 0.9-1.0 µs by fault and 0.44-0.48 by `madvise` with
`MADV_POPULATE_WRITE` over a block of them, the kernel's zeroing being
the half that stays. So the arena's blocks and the piece buffer were
given a window populated ahead of the fill (`Options::prefault`,
`populate_ahead`), and the load's phase print read the commits at 2.13
ms against 2.38, below in 13 loads of 13, 0.25 ms for about six hundred
pages at half a microsecond each.

The first window was a fixed quarter megabyte, and the pairing refuted
it where the writes are few: a fresh table's first write populated
sixty-four pages an arena, thirty microseconds, for the lag sweep's
one-percent burst of a hundred writes over four pages, whose updates
read 0.66x in 24 pairs of 24, and for ycsb-B's, 0.87x; the load read
1.03x (18 of 24). The window grows with the fill now, as much again as
is populated between 16 KB and 256 KB, and the second pairing had the
small cells back within noise -- ycsb-B 0.97x at p .02, the one-percent
burst 0.94x unresolved -- the commits at 2.24 ms against 2.44 in 10 of
13, and the load nothing by rate: 0.998x in 24 pairs at ten thousand
keys and 1.09x in 12, 0.95x in 12 at three hundred thousand, the
durable load level. A lever that moves a phase by a tenth and the load
by nothing is one whose saving the rest of the load spends, and the
small write sets paid for it in both versions, so the option ships off
and the populated shape is the arm, `supdb-populate` and
`supdb-ingestpopulate`. What the fault count says stands: the fresh
pages are the commits' gap, and a design that stages records in
user-space buffers pays them where one that writes into the page cache
does not.

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
publishes now (`Maint::rank_before_publish`; the ranks are an arm's
since, `Options::piece_ranks_whole`, under "against LMDB with its
readahead off" below), at a seal's landing, a
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
  away with an unsynced tail could leave a store that does not open
  (the sync came off the writer's thread later; "The buffered
  one-percent burst at a million keys", above).
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

#### Out of core: what a miss costs on this host

Every banked row is in-core -- the full rows top out at three million
keys on a sixteen-gigabyte box -- so before an out-of-core rung is
built the question is what a miss costs where it would be measured. A
probe placed itself in a cgroup v1 group bounded at 256 MiB and read a
1 GiB file written outside the group, fsynced and dropped from the
cache, 10,000 random 4 KiB preads under `POSIX_FADV_RANDOM`: p50 38 µs,
p90 51, p99 120, nine reads in ten under 50 µs, one block I/O per read
(9,796 of 10,000 reached the device) at 35 µs each by the block layer's
own clock; the same offsets again after a second drop read p50 35, and
sixty seconds after the write p50 34. A disk answers a cold 4 KiB read
in a hundred microseconds or more and the second pass no faster than
the first; this is the host's page cache behind the virtio device, and
it kept a file the guest wrote for at least a minute. So an
out-of-core rung on this host measures misses served at 35 µs: the
counts a bounded pass makes -- faults and block I/Os per operation --
are what transfer to a disk, and the rates are this host's. 128 KiB
sequential preads ran at 133 µs each, 830 MB/s; a 4 KiB write with
fdatasync 188 µs at p50, one flush each.

The kernel honours `MADV_WILLNEED` under `MADV_RANDOM`, which is the
mechanism a cold scan's span prefetch needs: over the dropped file
mapped `MADV_RANDOM`, one touch and then four dependent pages 4 KiB
apart cost 4.6 major faults, 4.6 block I/Os and 182 µs an iteration;
with `madvise(MADV_WILLNEED)` over the four between the first touch and
their reads, 0.9 faults, 1.9 I/Os and 82 µs, the four pages fetched as
one I/O behind the first. A cold scan of a hundred entries touches an
index stride, a directory page and three or four record pages in
series today; asked for together they are two round trips, not six.

#### Out of core: what the process holds beside the page cache

Under a bound the engine's anonymous memory is page cache it does not
have, so the first bounded rung asked what the process holds, phase by
phase. The suite's three-million-key rung ran under an eight-gibibyte
group with the group's `rss` and `cache` sampled twice a second beside
the phase marks (`SUPDB_LAG_MARK`), one arm a process. The ordered
load and the passes after it hold 50-150 MB of anonymous memory on every
supdb arm and 20 on LMDB. The shuffled load is
where the arms part: the default ended it at 814-865 MB resident, the
arm with no block cache at 190-287, the arm whose forms wait for a
reader's scan at 180-315, and LMDB at 25, with a single malloc arena
(`MALLOC_ARENA_MAX=1`) moving the default's figure to 697-835, so the
memory was allocation and not the arenas' retention. Every sixteenth
page fault with its call chain (`perf record -e page-faults -c 16
--call-graph dwarf`), cut to the two shuffled loads' windows, put 1.4 GB
of the 3.3 GB faulted on the upkeep thread, 630 MB of it in
`BuildCtx::copy_block` under `materialize`, which builds a form, and
500 in `Reader::splice_run` under `patch_block`, which grows one. The
writer's thread, and the seal threads that inherit its name, took 1.3
GB, 750 MB of it in the seal's job and 260 in the memtable's chunks,
and the segment work 620, the compaction's output. With the forms
maintained at every commit and nothing gating them on a reader, each
batch of random writes overlaid every block of the store and the
thread built and patched forms over all of it, which the next landing
dropped, and the scans that came after the drain read 3.4 M entries/s
on the first lag pass where the arm that filed nothing read 25 (one rep
each, ops divided by ten). Under a one-gibibyte bound the default was
killed in its first rep at three million keys, `anon-rss` at the limit
and the group's `cache` at 32 KB, every file page reclaimed under it;
the phase it was in is not known.

The commit's maintenance files nothing far from a scan now -- none in
the store's life or two stores' worth of writes since the last, the
`reads_around` question the upkeep's hand-over already asks -- and the
forms wait for the first scan, which builds its own (`Options::forms_settle_far`,
off; `supdb-settlefar` and `supdb-ingestsettlefar` file as before). A
test writes a partitioned store in random order through the handle the
suite holds and asserts no block built, then that the arm builds some
and both scan every key. Priced against the arm in one process at a
hundred thousand and three hundred thousand keys, both policies, twelve
pairs a sitting and two sittings with the order swapped: level on every
quantity, nothing holding under Holm and the per-quantity marks at the
coin's rate with their signs swapping between sittings, since at those
rungs the filing ran on a niced thread beside a four-core box and the
memory was tens of megabytes. The same probe at three million read the
new default's shuffled load ending at 217-252 MB resident, the first lag
pass after it at 28.6 M entries/s, and the lag sweep's own forms still
taking the process to 960 MB, which is the next question: the forms over
a store a piece spans are larger than the store. The same probe read the
arm with its cache budgeted at 256 MB (`supdb-cache256`) peaking at
840 MB in the lag sweep and the plain-readahead arm at 953, against 417
for the arm with no cache. Under the gibibyte the default, with the
change above in, and the 256 MB arm were killed at three million keys,
`anon-rss` at the limit with a different thread allocating each time;
the plain-readahead arm was not run under it, and a peak of 953 MB
leaves the page cache no room there. The budget counts the
forms' contents -- keys, values and entries -- and not the memory they
cost: the vectors' slack, the arenas' retention of what a shed form
freed, and the copies a publish carries into the state for every
handle, which are outside any handle's budget. A bounded rung's first
rows are the arm with no cache against LMDB, and the forms' memory is
the lead the rows name.

With the forms gated the next bounded rung found two more structures
kept for readers on a load nobody read, both on the seal's side. A
shuffled load of five million keys under 640 MiB (`ab --passes load`,
the arm against itself) killed the default and the combined `Auto` arm
and peaked the forced-drop arm at 600 MB, while the arm with no cache
ran in 318. A census by owner every hundred batches
(`bench/examples/load_census.rs`, in a memory group of 640 MiB) named
the first: every seal copied its frozen table's chains into a snapshot
for the scans over the store while it ran (`Options::seal_snapshot`),
the table's bytes again, set on the table and held until it lands, and
three frozen tables stood with three copies, 380 MB of the bound. A seal
of a table frozen far from every scan -- the `reads_around` question,
asked of the scans on either path (`Shared::scans_merged` counts the
merge path's, which nothing counted) -- builds none now and writes its
records from the table's chains (`Options::seal_snapshot_far`;
`supdb-sealcopyfar` and `supdb-ingestsealcopyfar` build as before). A
scan that comes while such a seal runs sorts the frozen table itself. A
drain's seal builds its copy as `drain_snapshot` says: the first lag
point after a shuffled load of thirty thousand keys read 0.89-0.97x
without the drain's copy in six sittings of six, at the same
instructions a scan to within two hundred in three hundred scans and
over a segment identical byte for byte, but a probe of that point read
two arms that differ in nothing there at 0.92-1.09x in both orders, so
the point is the memory its scans run over, and the drain keeps the
shape its rows were taken in.

With the copies gone the default still peaked at 529 MB in some loads
and 280 in others in one process, and the count that split them was the
writer's wait for room at its freezes (`join_wait_ms`): 1.3-2.4 s in the
slow loads, none in the fast, never any in the arm with no cache. The
landings had fallen behind the freezes. A profile of the load by thread
put the segment work's thread at three times the no-cache arm's CPU: a
fifth of it in the pieces' ranks against their partitions, taken before
each publish so that the first scan would not pay for them
(`Maint::rank_before_publish`), and most of the rest in the device
doorbells and context switches of the major faults that walk took over
partition pages the bound had pushed out. The next merge folds those
pieces away before anything reads them. A landing while the writer's
last seal was far from reads publishes its pieces without ranks or
bounds now, a flush's landings counted near since reads follow a drain,
and a reader that comes takes them as it takes them for a piece opened
from disk (`Options::land_ranks_far`; `supdb-ranksfar` and
`supdb-ingestranksfar` take them as before). The census lines had not
moved for either: the census counts what the store holds, and the
allocator's bytes in use beside it (`mallinfo2`) were what said the
excess was live and not retained.

The suite's own probe after both, every shipping arm through the
ordered and the shuffled load twice in one process, peaked the default
at 312 MB, `Auto` at 338, the forced-drop arm at 333 and the arm with no
cache at 278, the peak in each the shuffled load's closing sync. In core
neither gate moved a load: twelve pairs a sitting at thirty thousand to
three million keys, both policies, two sittings with the order swapped,
read the ordered and the shuffled load at 0.88-1.22x of the arm that
builds with the sign swapping between sittings, nothing marked in both,
while the arm's segment work spent 6-20% more CPU at one and three
million keys on copies (578 ms of them at three million) and 25-27% more
on ranks at three million, in ten to twelve pairs of twelve in both
sittings.

What the sync held after both was the drain's merge, which rewrites
every partition a shuffled load has put pieces in, and to place its
cuts it kept every distinct key of its inputs whole with an offset
each, 24 bytes a key: 120 MB at five million keys, 190 with the
vectors' doubling, the sync's largest live owner. A cut reads a key's
first sixteen bytes and nothing more -- a join is a key at an even rank
cut to the fence's length, and a key is below such a cut exactly when
its own first sixteen bytes are -- and a fence's cuts are taken from
that fence's keys alone, so the merge pushes its keys in order through a
slicer that holds only the fence being walked, cut short, and takes its
slices as the walk leaves it (`db::Slicer`); with no fences, before the
first partitioning, it holds every key cut short, as few as there are
then. The slices are the old ones exactly: the old cut ran beside the
new one under every merge of the fence tests, the merge and partition
tests of the engine's suite and 770 seeds of the differential fuzz in
four ranges, and was held equal by an assertion before it was taken
out. The bounded load's sync then read 285-329 MB on the default and
288-307 on `Auto` over three loads each, with the drain's copy back in
it.

The same probe answered a question the first bounded numbers raised. A
bounded row divides its read passes' operation counts for time, and at
a division of ten three of the four lag points at three million read
the default at 0.25-0.4x of LMDB and the one-percent point at 0.87x,
where the row before it, at one million with whole counts, read
1.2-1.9x. One rep at three million with the quick scale's whole counts
and no bound read the default at 1.85x, 1.9x, 1.7x and 1.03x of LMDB
over the four points, so there is no cliff between one million and
three. The two passes' times fit a one-time cost of tens of
milliseconds in the first scan, which a tenth of the scans spread over
a tenth of the entries; and once the forms stopped being filed far from
a scan the divided pass's first point read 28.6 M entries/s, so the
division did not make the pass trail, it magnified what the stale forms
cost. A pass whose first operation builds what the pass needs is
measured by its count, and a count divided for time is a different
pass; the bounded rows' division is read with that in mind, and the
first scan of a divided pass is to be reported on its own, as
`SUPDB_SCAN_OPS` reports it for the scan pass.

#### Out of core: what the budget counted and the process paid

The arm whose block cache is budgeted at 256 MB (`supdb-cache256`) held what the arm with no
budget held. At three million keys, the quick scale's whole counts and one process an arm, the lag
sweep's full-rewrite point read 808-1,028 MB of anonymous memory at the end of its pass in it
(median 935, ten reps in five sittings) against 195-304 in the arm with no cache (median 234, eight
reps), with the budget's count at 255 MB and the allocator's bytes in use at 483-538. `lagmem`
could say that and nothing about the 230 MB between the count and the use.

A census of the process by owner (`Reader::memory_census`, the `lagcensus` line under
`SUPDB_LAG_COUNTERS`) counts the writer's forms by kind, each with the bytes the budget counts in it
beside the bytes its buffers allocate, the forms the state publishes (those a table shares and
those only the state holds), the retired lists, the scan snapshots, the memtables by who holds them
and the segments' derived caches, each structure once, by the first owner reached. Its sum came to
92-106% of the allocator's bytes in use at the 38 points past 200 MB in use (the mapped chunks,
16-144 MB, `mallinfo2` counts apart), which is what made the rest a question with an answer. It
found four things, none of them a leak.

The forms cost 1.6-1.8 times what the budget counted. The cache held 41,000-43,000 sparse forms,
228-240 MB of contents in 372-391 MB of buffers, and 2,000-3,400 copies, 16-28 MB in 31-54; or,
where the pass had rebuilt them, 30,840 copies, 256 MB in 452. The default's 46,876 copies cost
440 MB for the 389 they held. A build asked for room for every overlay key, the ones that replace a
record as much as the ones that add, so a copy built over a store every key of which had been
rewritten had room for twice its records; a patch grew a sparse form's three vectors to the dense
count at once and by doubling past it, so a block a burst had patched forty times sat in the
capacity of sixty-four; and a copy patched with a longer run appended to its values and doubled.

Nothing shed for a patch. The budget was kept where a scan built a block and where the read-ahead
installed one, and a burst's patches grow the forms of blocks that hold one, so a burst the upkeep
thread kept pace with built the cache up to the size of the store (525 MB in use where one such
burst ended, the others filing their forms in the pass that followed) and the first scan after it
shed the difference into the allocator's free lists.

The shed of a block in another partition's table left the form it had published in the state, and
this was the engine's before the flag and whatever the flag says. The shed from the table in hand
retracts it with a marker (`unlist`); the branch for any other partition took the slot and the bytes
and nothing else, so a reader's handle could take a form the writer files nothing into any more. The
first write into the block then trips the check in `settle_each` that a block the writer holds no
form for has none published as the block, in the checked profile, and in a build without it the
handle answered the values from before the write: all three rounds of the reproducer disagreed with
the model. Any store with a budget (`scan_cache_bytes`, as in `supdb-cache256`), a claimed handle
and writes was exposed. The lag sweep's census, which claims no handle, read zero published forms
only the state holds, and the suite's harness does not check the values it reads, so which of its
rows served a stale one is not known. The retraction is unconditional now.
`a_block_shed_from_any_partition_takes_its_published_form_with_it` builds the shape (a budget of six
kilobytes, a handle claimed, scans from elsewhere until the blocks held have been shed and built
again, then rounds of writes read through the handle and the writer against the model), and
`a_budgeted_cache_answers_the_model_with_a_handle_claimed` is the seeded differential run that found
it.

And what the sheds freed stayed resident. A freed chunk goes back to the arena that allocated it,
and glibc gives an arena's memory to the system from its top. The forms a burst's patches built on
the upkeep thread were freed by the scans that shed them on the writer's, and at the pass's end one
arena was 152 of its 154 MB free in 19,000 chunks of 8 KB, beside a cache a quarter of that size
(`malloc_info` by arena, the `lagarenas` line). `malloc_trim` returns the whole pages inside free
chunks, but `mallinfo2`'s free count includes what it has returned, so the resident bytes the
allocator keeps beyond its use are read as the anonymous bytes less its in-use and mapped bytes
(`held` below). The arena is not all of it: the arm with no cache keeps 112-226 MB beyond use at
the same point, the memtable and snapshot blocks the load's seal threads freed in arenas nothing
allocates from now, and that is every arm's, the forms' or not. Last, 37-44 MB of retired snapshots
waited for a sweep that only a commit made, in three reps of ten.

`Options::forms_to_fit` (arms `supdb-tofit` and `supdb-ingesttofit`, and `supdb-cache256tofit`
beside the budget) counts a form by the capacity of its buffers, builds it to the size it came out
(`shrink_to_fit` at the end of a build) and grows it by a quarter and not by doubling, so that a
sparse form reaches the dense count in about six reallocations of each buffer where it took one, the
price of the slack and a part of the upkeep thread's extra CPU below. Three parts come with it, each
an option of its own so that it is priced by taking it away: the budget kept after a patch
(`forms_fit_shed`, the arm `supdb-cache256tofitnoshed`), the retired snapshots and forms swept when
an engine thread's operation ends (`forms_fit_sweep`, `supdb-cache256tofitnosweep`), and the
allocator trimmed once handles have freed that many bytes of forms and it holds that much more free
than after its last trim (`forms_trim_bytes`, 32 MB, `supdb-cache256tofitnotrim`). The retraction of
a shed's publication is not among them. The measurements below are of the flag as first built, when
the trim ran on the segment work's thread and the sweep skipped every operation end after one that a
reader's pin had held, for as long as the number waiting was the same. That guard is the likeliest
reason 37-40 MB of retired snapshots still stood at the pass's end in three of the twelve reps with
the flag, as in three of the ten without it: not a pin that stayed, but nothing sweeping them once
it had gone, since a thread that reads and writes no more retires nothing and the number never
moved. A probe shows that, and the lag sweep itself was not checked:
`the_sweep_at_an_operations_end_finds_the_pin_gone` pins a handle, retires under it, lifts it and
reads, and the guard left them waiting. The sweep retries after sixty-four operation ends that
skipped it now, and only for a handle that holds its upkeep, as the commit's sweep is (a writer that
has lent it leaves the sweeping to the thread that has it); what it frees at the pass's end has not
been measured since. The full-rewrite point, median (range) over the reps of the sittings,
in megabytes:

| arm | `forms_to_fit` | pass's end | burst's end | in use | held beyond use | forms counted / buffers | reps |
|---|---|---|---|---|---|---|---|
| `supdb-nocache` | | 234 (195-304) | 298 (240-364) | 22 (22-45) | 166 (112-226) | | 8 |
| `supdb-cache256` | off | 935 (808-1,028) | 598 (515-889) | 517 (483-538) | 314 (211-403) | 256 / 452 | 10 |
| `supdb-cache256tofitnotrim` | on, no trim | 802 (751-881) | 518 (496-659) | 328 (318-341) | 346 (305-445) | 256 / 256 | 4 |
| `supdb-cache256tofit` | on | 578 (381-695) | 430 (312-743) | 326 (271-411) | 144 (88-236) | 256 / 256 | 12 |
| `supdb` | off | 856 (782-929) | 892 (774-929) | 514 (503-525) | 228 (209-390) | 389 / 440 | 6 |
| `supdb-tofit` | on | 774 (680-794) | 771 (709-819) | 464 (450-519) | 204 (187-231) | 389 / 389 | 6 |

The burst's end is read from the kernel before any scan (`lagburst`), and the larger of the two
samples is what a bound meets: 935 for the budgeted arm, 632 with the flag (523-743). Against the
arm with no cache and the budget -- 234 and 256, 490 at the pass's end -- the flag takes the
budgeted arm's excess from 445 MB to 88, and not all of that is the forms': the arm with no cache
has no trim, and the trim also returns part of the load's own retention (144 MB beyond use with the
flag against 166 for the arm with no cache), so the baseline is higher than the same arm trimmed
would read, and what is the forms' alone is the part the arm without the trim prices. A pass's end
within a hundred megabytes of 490 is reached at the median (578) and not in every rep (381-695).
The pairs agree in sign, six reps of six in the three sittings that ran both, by 282-404 MB at the
pass's end. The default's saving is 80 MB, the slack and little else, since the default has no
budget to keep and its cache is the store's: 774 MB at this point with the flag, so whether the
default policy runs under a bound at three million keys is not answered here, and wants a budget
derived from the bound or the policy restricted by it. The flag off reads as before in every
sitting (buffers 1.65-1.77 times the contents in the budgeted arm, 1.13 in the default), and the
flag on reads 1.00 at the full-rewrite point and 1.10 at the tenth-rewritten in the budgeted arm
(1.12 for `supdb-tofit` there).

Where the saving comes from is two things, and the arm without the trim
(`supdb-cache256tofitnotrim`, `Options::forms_trim_bytes` zero) prices them apart: kept to fit alone
the budgeted arm's bytes in use fell by 189 MB and its pass's end by 133, since what the sheds freed
stayed resident (346 MB beyond use against 314), and the trim returned the other 224 (144 beyond
use), the four pairs agreeing in sign by 180-228 MB. The trim by itself, probed (`SUPDB_LAG_TRIM=1`,
a `malloc_trim` after each point's census, the `lagtrim` line), returned 250 and 300 MB of the
flag-off arm's pass's end (1,025 to 726 in 94 ms, 941 to 691 in 32): most of what that arm holds
beyond its use is whole free pages that a trim returns. The strided trim leaves the tail of a shed
burst resident: the probe returned 145 and 250 MB more at the end of a pass with the flag on (726 to
581, 1,021 to 771), which a trim when the frees stop would return, and which the pass's end, read
the moment the last scan returns, would not show.

What the flag costs. A trim is a `malloc_trim(0)` over every arena, 30-200 ms at this size, and one
to seven were made over a pass (`lagcensus`'s `trims`, `looks` and `look_ms`; the looks that decided
not to trim cost a few milliseconds). It was made on the segment work's thread, between its jobs, so
a seal's landing, a flush's answer or a merge's publish that arrived meanwhile waited for it; it
runs on a thread of its own now (`trim_loop`), and
`the_segment_work_goes_on_while_a_trim_is_in_progress` holds it at the point a trim would begin and
flushes. What stays is the arena's lock: `malloc_trim` returns each arena's pages with that arena's
lock held, and a thread that allocates or frees in it waits, which in the arena of the forms is the
upkeep thread's next allocation and the writer's free of a form it sheds. A standalone probe of
that, on a machine that was busy, with 160 MB of 8 KB free chunks in one arena: the trim took 9-17
ms and single small allocations of a thread in that arena stalled for 5.6-10.9 ms during it, in
three runs. The upkeep thread's CPU over the burst read 3.2-4.0 s with the flag against 3.2-3.6
without (medians 3.7 and 3.4), the segment work's 0.3-1.2 s against 0.25-0.56, and the blocks the
pass built did not move; those are of the flag as first built. The writer's rates, the drain and the
lag points under the flag were not measured, nor the faults of the pages a trim returns and a build
takes again (the `lagcounters` line of the same run has them by thread): no pairing was run, and the
arms exist to run one (`supdb-cache256` against `supdb-cache256tofit` and the three arms that take a
part away, `supdb` against `supdb-tofit`, on the quick row and in `bench ab`).

A request to the trim on builds as well as frees, meant to return the load's own retention at the
burst's start, was tried and dropped: the budgeted arm read 636 and 580 at the pass's end and 290
and 732 at the burst's end in a sitting, no lower than the same arm without it, and a look cost
30 ms of the thread (36 looks and 1.1 s in the first rep).

What is left, and where the next lead is. The arm with no cache holds 195-304 MB at this point,
112-226 of it resident bytes beyond use that the load's threads left in their arenas, so the load's
own retention is a lead of its own, in every arm. The write path holds 60-260 MB beyond the cache
while a burst is in flight or a reader's pin holds what a publish replaced -- a retired memtable and
a retired snapshot, 36-43 MB each when present, two frozen tables and three snapshots in the rep
that read 1,019 MB -- which no budget counts and no sweep can free under a pin, and it swings the
same arm's pass by that much from rep to rep with the census saying where, which is why the
medians above are over every rep there is. The budget is a handle's, and the copies a publish
carries into the state for every other handle are outside it (the published forms only the state
holds read zero in these runs, which have one handle). And the budgeted arm was not run under a
bound -- a group limit of a gibibyte at three million keys -- where the process's anonymous bytes
and the page cache compete and the arms' rows would say whether 630 MB is enough.

Both were followed under a bound, at ten million keys, where every cached arm was killed by the
lag sweep's tenth or full rewrite. What the write path held under a pin was the larger part: the
upkeep thread pins one state for its pass, out of core a pass resolves a key a major fault and runs
for seconds, and every table that landed meanwhile waited behind the pin with its snapshot copy.
The thread now gives its forms up once the tables landed under its pin, or the tables a look has
fallen behind, cost more than an eighth of the memory available, and the store reads as far from
reads until it is scanned (`Options::upkeep_abandons`, `upkeep_keep_bytes`); and the install of the
builder's forms no longer reads the partition (`ahead_sends_bounds`). The default's cache, the
store's own size after a rewrite, has a budget of its own from the same reading of the memory
available, counted by capacity (`Options::scan_cache_auto`): below it the cache is the unbounded one
exactly, and the first time it passes the budget it binds, as `scan_cache_bytes` with
`forms_to_fit` has it from the start -- a budget that binds from the start costs the full-rewrite
point its rate in core at sizes where it never binds, which is why this one waits.

The load's own retention named above was the allocator's, and the engine keeps its blocks out of
the allocator now (`Options::blocks_mapped`). The memtables' arena blocks (four megabytes, one in a
recycled stream), their slabs (half a megabyte to a megabyte and a half) and their hash indexes, and
the scan snapshots' arenas, came from glibc, whose mmap threshold starts at 128 KB and rises to the
size of the first large chunk it frees, so every such block after the first came from the heap and
stayed in it when its table dropped, until a trim or a reuse. At ten million keys the lag sweep held
150-620 MB beyond its bytes in use (`lagmem`'s anon less in use less the allocator's own mapped
chunks) and 1.3 GB of anonymous memory at the full rewrite, which under 1.25 GiB was the kill, and
the compact form's budget, which subtracts the process's anonymous memory, bought late by as much.
One run with glibc told to map every allocation of a megabyte or more and to trim a heap's top past
four (`GLIBC_TUNABLES=glibc.malloc.mmap_threshold=1048576:glibc.malloc.trim_threshold=4194304`
around `bench ab --arms supdb,supdb-nocache --size 10000000 --passes load,lag --reps 1 --ops-div 10`
under `SUPDB_LAG_COUNTERS=1`, beside the same run as shipped, at `f2381c3`) held 0-135 beyond use
at every point, so the owner was the large blocks and not the forms' chunks; what the forms' arena
still held free there, 96 MB at the full rewrite, is the trim's to return. The engine maps those
blocks from the kernel itself now, from 128 KB (`blocks::MAP_FROM`), and unmaps each at its drop
(`src/db/blocks.rs`), so a landing gives its table's memory back at once and the process's anonymous
bytes read the engine's own; a block's pages are fresh at their first touch where the heap's were
touched before, which is the price. `supdb-heapblocks` keeps the heap shape, and the pairing reads the two level in core, twelve pairs a
sitting at three hundred thousand keys in both orders (`pair.sh supdb supdb-heapblocks 300000`), the
loads and the lag bursts within a tenth with their signs swapping between the sittings and nothing
marked over the table; at three million keys with the memory lines on (`pair.sh SUPDB_LAG_COUNTERS=1
supdb supdb-heapblocks 3000000 --passes load,lag --reps 2`) the mapped shape holds half the heap
shape's retention at the ten-percent and the full-rewrite points, rep for rep in both orders, and
what it still holds beyond use there is the forms' chunks and the writers' vectors.
The `lagmem` line prints the mapped bytes (`mapped_mb`) beside the allocator's in use and free, and
the resident count the trim reads (`Options::trim_by_resident`) takes them out.

The first bounded row where both engines leave core banked at `5bf1f88` (`sh bench/scripts/ooc.sh
1342177280`, `bench/runs/ooc/20261103T040436Z-5bf1f88.json`: ten million keys under 1.25 GiB, three
reps, the roster's seven arms, each arm's rep in a process of its own, the read passes divided by
ten). Against LMDB with its readahead off, the configuration its documentation gives for a store
larger than memory, the controller arm (`supdb-autozpages`) reads level on one thread and 1.7x on
three, scans 1.3x on both, takes the read-heavy mixes at 1.3-2x (B 2.0, C 1.3, D 1.7) and the update
mixes at 4x (A 4.3, F 4.1), with E at 0.8x, and loads at 1.3x ordered and 11.6x shuffled -- at a
tenth of LMDB's major faults a point read (0.06 against 0.07 for the reads that go raw before the
form installs, and none in the mixes) where the raw form takes three times LMDB's. The default arm,
whose raw form of 1.4 GB does not fit the bound while LMDB's 1.3 GB nearly does, reads at 0.5x,
scans at 0.16x and takes C and D at 0.2-0.3x, the regime the compact form exists for; three of the
raw arms read alike, and the arm without the read advice pays 2.2 refaults a read for the readahead
the advice turns off. What the row does not win: the lag points, the controller arm's ten-percent
point at 0.3x and every arm's full-rewrite point at 0.03x, which is the merge streaming a store
rewritten whole while the scans walk its pieces. At three million keys, where both fit, every supdb
arm reads 1.6-1.9x, scans 1.2-1.4x and takes the mixes at 1.5-7x, E level.

#### Out of core: against LMDB with its readahead off

The first bounded rung at ten million keys under a gibibyte read the
arm with no block cache beside LMDB as a user gets it and with its
readahead off, the lines of the warmup rep, one rep each and the banked
rep not yet in. LMDB's default readahead is a loss out of core on its
own: a fault reads 74 KB and a point read 19.8 KB from the device, 12
times the arm's, and the arm reads at 54 thousand a second to its 33
thousand, 147 thousand to 52 thousand on three threads. The LMDB a user
configures for the regime turns it off, and that arm is the comparison:
0.27 faults and 1.1 KB a point read to the arm's 0.41 and 1.7 KB, 82
thousand reads a second to 54 thousand on one thread and 195 thousand to
147 thousand on three, scans at 2.03 million entries a second to 0.80 on
one thread and 4.82 million to 4.08 on three. Writes go the other way:
mix A at 244 thousand operations a second to 65 thousand, the ordered
load at 1.35 million to 0.94, and the shuffled load faulted 1.5 million
times on LMDB's writer where the arm's writer faulted none. The
prediction written before the line printed (0.27 +- 0.03 faults, 65-85
thousand reads a second, scans between 0.8 and 1.83 million) held for
the reads and missed the scans, which were faster than the range: the
readahead-off arm takes 1.1 faults a scan where the readahead-on arm
took 0.44 with 69 KB a fault, and was faster for it.

A point read of this suite's records touches two pages, the hash slot and
the record whose value is inline (`Options::inline_bytes`). The slots are
touched by every read, so they stay resident and take their share of the
cache before any record does, and the faults a read takes are the share
of the records the rest of the cache cannot hold: a file of 1.5 GB (the
three-million ratio of 1.31 bytes on disk a byte carried up), 0.94 GB of
cache and about 0.19 GB of slots (a power-of-two table of 8-byte slots,
19 bytes a key at this partition size) predict 0.40-0.44 faults a read,
and 0.41 was measured; LMDB's 1.33 GB file with branch pages in the
megabytes predicts 0.30, and 0.27 was measured. The cold single-thread
scan pass reads 2.9 faults a scan where the threaded passes read 1.3,
which fits the ordered index's heads (80 MB for the store) being faulted
in by the first scans and found resident by the threaded passes; the
counts do not separate the two.

What a compact form would buy follows from the same model and is
arithmetic, not a measurement. The suite's records, sixteen digits and a
hundred bytes of which half is a repeated phrase, compress to 0.57 of
their size in blocks of 4-8 KiB under LZ4, the engine's codec, and to
0.53-0.55 under zlib at level 1, measured offline over 200,000 of them
with the Python modules, and to nothing one value at a time; a block
decompresses in 2-3 microseconds there and compresses at about 0.6 GB a
second. Records held as groups that each fill one 4 KiB page, some 60
of them, would be 67 bytes a key and about 0.7 GB at ten million keys
where the raw region is 1.3 GB, under a cache of 0.94. The hash is then
the smaller lever: dropping it from the point read's path frees at most
0.19 GB, about a seventh of the faults. `lz4_flex` is the engine's
already (`src/block.rs`), and the ordered index is the pattern for a
form file the store derives, checks against its segment and regenerates.

The banked rep of the same rung (one sample a cell, so no error bars;
the second pass agreed with the warmup's on the reads) fills in the rest
of the cells for the arm with no block cache, LMDB as shipped and LMDB
with readahead off:

| 10M keys, 1 GiB | no block cache | LMDB | LMDB, readahead off |
|---|---|---|---|
| point reads a second, 1 / 3 threads | 54k / 132k | 33k / 53k | 79k / 192k |
| faults a point read | 0.46 | 0.27 | 0.27 |
| scan entries a second, 1 / 3 threads | 0.76M / 3.56M | 2.11M / 3.39M | 2.06M / 4.68M |
| faults a scan, 1 / 3 threads | 3.0 / 1.5 | 0.41 | 1.1 |
| ordered / shuffled load, ops a second | 1.51M / 318k | 0.97M / 19k | 0.92M / 24k |
| mix A / B / C, ops a second | 224k / 174k / 107k | 54k / 164k / 175k | 64k / 193k / 239k |
| mix D / E / F, ops a second | 42k / 49k / 146k | 36k / 94k / 53k | 87k / 138k / 67k |
| lag scans at 0 / 1 / 10 / 100 percent, entries a second | 1.0M / 9.9M / 1.5M / 0.05M | 0.27M each | 0.85M / 2.6M / 0.83M / 0.80M |
| lag bursts at 1 / 10 / 100 percent, updates a second | 1.10M / 0.82M / 0.78M | 8-9k | 16k |

Against the LMDB a user configures for the regime the arm wins the
loads (1.6x ordered, 13x shuffled), mixes A and F (3.5x and 2.2x), the
lag bursts (48-68x) and the one-percent lag scan (3.9x), and trails the
point reads (0.68x), the scans (0.37x on one thread, 0.76x on three),
mixes C, D and E, which are Zipfian reads and uniform reads and scans
(0.45x, 0.48x, 0.35x), and the full-rewrite lag scan, at 0.07x. Mixes C
and E fault 0.215 and 0.285 an operation against 0.075 and 0.121. The
full-rewrite lag scan faults 37 times a scan on the scanning thread and
has 371 KB a scan read by background threads; neither count says why,
and the candidates, untested, are the seek into the burst's piece, which
has no ordered index and binary-searches its records, and the merge the
burst starts streaming the store while the pass runs.

Neither candidate was the pass's first half, and the second was its
second. Timed scan by scan under the bound (`SUPDB_SCAN_OPS`, the
`lagops` line) beside the device's and the group's counters over the
pass (`lagdev`), the full-rewrite pass of 50-54 s over ten thousand
scans was a first scan of 18.5-19.8 s, one tenth of 17 s in its middle,
and eight tenths of 1.5-1.8 s around a median scan of 1.5 ms, with
8.1-8.6 GB read from the device. The first scan was the writer's rebase
ranking every piece standing against its partition whole
(`BuildCtx::rank_pieces`): the burst's landings had been far from reads
and took no ranks (`Options::land_ranks_far`), three to four hundred
pieces stood, and ranking them read every piece's records through major
faults, half the device's bytes of the pass; the middle tenth was the
same ranking again, at the first scan after the merge's landing, every
piece re-ranked against the partitions the merge had rewritten. Nobody
ranks a piece whole now (`Options::piece_ranks_whole`, off;
`supdb-rankswhole` and `supdb-ingestrankswhole` do, the segment work at
a landing near reads and a reader that meets a piece without them): a
build finds each piece key's cut where its walk of the partition stops
at the key, or by one seek over the block's heads where it does not
walk, the path every build took for a piece without ranks. The first
version of that cost the ten-percent pass a quarter to two fifths more
major faults than the ranks had: the cut's compare read the partition's
record at the cut, a page a sparse build touches for nothing else, for
an answer the ordered index's heads have wherever a head is a whole key
(`OrdIndex::seek_in_exact`); the heads answer it now, and the sweep's
1.6 million cuts read no record (`cut_asks`, `cut_record_reads`). Paired
at ten million keys under 1.25 GiB, two sittings of a warmup and a pair
each, `supdb` against `supdb-rankswhole` (`bench ab --arms
supdb,supdb-rankswhole --size 10000000 --bound 1342177280 --passes
load,lag --ops-div 10 --reps 1`, under `SUPDB_SCAN_OPS=1` and
`SUPDB_LAG_COUNTERS=1`): the full-rewrite pass 18.8-19.7 s against
49.6-52.8 in every sweep, its first scan 111-120 ms against 18.5-19.4 s,
the device's reads over it 3.9-4.4 GB against 8.1-8.6, the cell 2.6x
and 2.8x; the ten-percent point's first scan 47-61 ms against 460-865,
and its pass 3.4-3.7 s against 3.4-4.8 with the compare on the heads
where it had read 4.0-4.4 against 3.8 with the compare on the records;
the one-percent point, whose pass is the first scan settling the burst,
and the drained scans level. In core the two arms run the same code
wherever no piece stands, which is every drained pass -- `cut_asks`
reads zero over the whole sweep at a hundred and three hundred thousand
keys -- and the pairings there, twelve pairs a sitting in both orders,
disagree in sign between the sittings on every cell that moved, nothing
marked in both: the 300k sittings read the drained scan pass 1.09x for
the whole arm twice, one marked, over a store with no piece at that pass
(`pieces` 0 after the load) and with the two arms' per-scan medians
interleaved (`scanops`, 2.6-3.3 us against 2.9-3.4), which is the
position effect and not the arm. What the full-rewrite pass still is: a
scan of a hundred keys over a store rewritten in random order finds its
keys in as many pieces, some 35 major faults a scan at the device's
latency, while the merge that folds the pieces away streams its inputs
twice -- once for its slices and once for their values -- through a
cache that cannot hold them, 3.9-4.4 GB read over a pass of 19 s. Paired
against LMDB with its readahead off the same way (`bench ab --arms
supdb,lmdb-nordahead ...`, a warmup and a pair), whose lag passes run at
100-120 us a scan at every point here with 1.7-1.9 faults a scan, the
sweep read 0.56x at the drained point, 0.78x at one percent, 0.37x at
ten and 0.07x at the full rewrite, where the row had read 0.03x; the
scanning thread took 3.4, 0.5, 4.9 and 23 major faults a scan.

The faults a scan still takes at the full rewrite are the pieces': a
block's updates sit in as many pieces as the burst's seals that touched
its range, a page in each, and the build read them as it met them, one
fault at a time, each a device round trip taken alone -- while the
table's bounds had said where every piece's run over the block lies
before a record of it was read. A block's build asks the kernel for every
piece's run first now (`Blob::willneed_ranks`, `MADV_WILLNEED` over the
span the directory gives; `Options::pieces_willneed`; `supdb-nowillneed`
and `supdb-ingestnowillneed` read as before), where the controller's gate
is away from the raw form, the store's own measure of being out of core;
in core the gate is raw and nothing is asked, since an `madvise` a piece
a block would cost a build more than it builds, and the count
(`piece_willneeds`) reads zero through the in-core sweep. Paired at ten
million keys under 1.25 GiB, a warmup and a pair (`bench ab --arms
supdb,supdb-nowillneed --size 10000000 --bound 1342177280 --passes
load,lag --ops-div 10 --reps 1`): the full-rewrite pass 12.2 and 12.7 s
against 15.7 and 19.0, its median scan 0.85-0.97 ms against 1.32-1.63,
the scanning thread's major faults 11 a scan against 26 with the
device's reads over the pass the same 3.6-4.2 GB, the cell 1.5x; the
ten-percent point within the spread both ways (2.7-4.0 s against
3.1-3.7), since a ten-percent burst's pieces are 130 MB and sit in the
cache beside the merge that streams them, so there is nothing to ask
ahead for, and its five faults a scan are the ones the drained point
takes too, three and a half of them the partition's own record pages;
the one-percent point and the drained scans level. A page asked for
ahead and reached before it has arrived is a fault the kernel counts as
minor, since the folio is there, so the thread's major faults fall by
more than its waits do and the pass is read by its time. What is left:
the partition's own pages a raw scan touches, three to four a scan where
LMDB's leaves are two, which is the compact form's to shorten and a
reason the row's sweep runs after the read passes that install it; and
the merge's two passes over its inputs.

The partition's own pages were asked for the same way next
(`Options::blocks_willneed`; `supdb-noblockwillneed` and
`supdb-ingestnoblockwillneed` read as before): a clean block's records
from the seek, which is the drained store's whole walk, and a block's
records ahead of its build, for a partition no form file serves and with
the gate away from raw, at most 64 KB an ask (`WILLNEED_CAP`), since a
scan's limit can be every rank. The first version asked whether the
pages were resident or not, and lost the one-percent point 1.4x while
winning the other three: that point's pages the burst's settle had just
pulled in, and an `madvise` over resident pages still walks them in the
kernel, ten to twenty microseconds a scan of five. An ask asks `mincore`
of the span's first page first now and asks for nothing where it is
resident (`Bytes::resident_at`), the pieces' asks the same; over the
sweep at ten million keys 23% of the spans considered were asked for
(`willneed_asks` against `block_willneeds` and `piece_willneeds`).
Paired at ten million keys under 1.25 GiB, a warmup and a pair (`bench
ab --arms supdb,supdb-noblockwillneed --size 10000000 --bound 1342177280
--passes load,lag --ops-div 10 --reps 1`): the drained point 1.12 and
1.26 s against 1.88 and 1.94, its scanning thread 0.9 major faults a
scan against 2.7, the cell 1.7x; the ten-percent point 2.7 and 2.8 s
against 1.9 and 4.0, 2.5 faults a scan against 4.9, the cell 1.5x in the
pair; the full-rewrite point 9.9 and 10.1 s against 10.6 and 13.1, 7.3
faults against 12.7, 1.3x; the one-percent point 0.61 and 0.62 s against
1.0 and 1.6 here, and against 0.49 and 0.50 in the pairing before the
residency check, where it had read 0.70 and 0.77 -- its scans after the
first 20 us against 6, the checks and the asks that found a page cold.
Against LMDB's figures from the pairing before these two asks, the
drained point stands level (1.1-1.3 s against 1.15-1.17), the
one-percent point at 0.6x, which is the first scan settling the burst
where LMDB has nothing to settle, the ten-percent point at 0.4x and the
full-rewrite point at 0.12x, where the row had 0.03x.

The merge's two passes over its inputs were the other half of the device's
bytes under the full rewrite: the first pass reads every input's keys for
the slices it cuts, the second reads them again for the values, and both
read the records, so a store of 1.3 GB rewritten whole streamed 5.2
through a cache that holds a third of it. The first pass reads each
input's keys off its ordered index's heads now where a head is a whole
key (`Options::merge_keys_by_heads`, `OrdIndex::whole_key_at`; eight
bytes a key, the index opened beside the records and its prefix learned
off the first key as `Seg::open` learns it), and off the records only
for an input whose keys are not one length; `supdb-mergerecordkeys` and
`supdb-ingestmergerecordkeys` read as before, and the merges' inputs are
counted by their source (`merge_head_inputs`, `merge_record_inputs`).
The two shapes write the same files byte for byte, the superblock's
clock aside, which `tests/fences.rs` holds over keys of one length and of
two. Paired at ten million keys under 1.25 GiB, a warmup and a pair
(`bench ab --arms supdb,supdb-mergerecordkeys ...`): the background
threads' reads an operation fell 1.75x through the full-rewrite burst
and 1.6x through the one-percent burst, where the merges' first passes
run, while every lag cell's pair and warmup disagreed in sign and the
pass's device reads stayed the scans' own (1.75-1.89 GB against
1.73-2.06), since the first passes fall mostly in the burst. In core a
run has one or two merges (ten and twelve inputs at a hundred and three
hundred thousand keys), the arms run the same code otherwise, and the
pairings there, twelve pairs a sitting in both orders, disagree in sign
between the sittings on every cell that moved, the update mix's marked
cell included (1.41x for the records' pass in one sitting at a hundred
thousand keys, 1.14x for the heads' in one at three hundred thousand,
1.32x for the records' in the other), a mix whose rate the seal's timing
makes bimodal at these rungs.

The next reading was wrong, and is kept as an arm for the record
(`Options::merge_drop_behind`, off; `supdb-mergedropbehind` and
`supdb-ingestmergedropbehind` drop). It said that the second pass's
input pages, read once and retired at the landing, stand in the cache as
the newest for nothing and push out what the scans beside the merge are
reading, so the merge should drop them behind itself (`MADV_DONTNEED`
and `POSIX_FADV_DONTNEED` over the records a chunk behind the walk,
`Blob::drop_records`). Paired at ten million keys under 1.25 GiB, a
warmup and a pair, the drop read slower in every sweep: the drained lag
point 1.13-1.26 s against 0.88-1.05, the ten-percent point 3.1 s against
2.2-2.4 with 3.2 major faults a scan against 2.4, the full rewrite
9.8-10.7 against 9.3-9.9, and more read from the device at every point
(644-694 MB against 449-508 over the ten-percent pass), 3.3 GB dropped
over the sweep. A merge's inputs are the live pieces and the partition
under rewrite, which is what every scan beside it reads until the
landing, so the pages the merge had just streamed were the scans' next
hits and the kernel's recency was keeping the right ones. The rule the
refutation gives: a page is dropped behind a walk only when nothing else
reads the object the walk is consuming, and a merge's inputs are read by
everyone until they are gone.

The one-percent point's first scan is the burst's settle, and a resolve is
a seek over one stride of a partition's heads, a page cold under the
bound: the settle of the burst's hundred thousand keys took its faults one
at a time on one thread, the whole of that scan. The heads are asked for
ahead of the resolves now (`Options::heads_willneed`;
`supdb-noheadwillneed` and `supdb-ingestnoheadwillneed` seek them as they
meet them), gated as the records' asks are, in two shapes by the batch's
density in the partition as the pages its keys resolve to read it: dense,
each key's page the one before's or the next, and the asks are runs of
sixty-four kilobytes of heads from the page after the one the key resolved
to, kept half a megabyte ahead as eight requests (`SETTLE_RUN`,
`SETTLE_LEAD`); sparse, and a cursor sixty-four keys ahead in the batch
asks each key's stride, found from the top level in memory
(`SETTLE_AHEAD`), which reads no page the batch will not. Paired at ten
million keys under 1.25 GiB, a warmup and a pair (`bench ab --arms
supdb,supdb-noheadwillneed --size 10000000 --bound 1342177280 --passes
load,lag --ops-div 10 --reps 1`): the one-percent point's first scan 248
and 276 ms against 427 and 460, its settle 216 and 249 against 387 and
401, the pass 0.45 and 0.46 s against 0.65 and 0.66 with 2,700 and 3,000
major faults against 5,200 and 6,400, the cell 1.46x; the ten-percent
point 2.4 and 2.8 s against 2.7 and 4.5; the full rewrite 9.6 and 9.8
against 9.8 and 10.9, 7.2 major faults a scan against 8.9; the drained
point 1.0 and 1.2 against 1.2 and 1.3; 74,000 asks over the sweep and
2,300 of them submitted, the rest finding their first page resident.
Three shapes were paired before it, the same way. An ask a key, the
stride's page a window of keys ahead, took the first scan to 259 and 286
ms against 463 and 482 and the full-rewrite pass to 11.3 s against 10.0,
half a second of it a syscall a key over ten million resolves. Runs of
sixty-four kilobytes asked half a run ahead kept a tenth of the gain and
the whole of the loss, which was not the settle's: a block's build asked
for the page of heads its cuts seek the microsecond before the seek, which
is the fault with a syscall before it -- the records' ask, already issued,
is what that fault overlaps -- and the pass took more major faults an
operation with it than without, 9.0 against 7.4 at the full rewrite and
1.1 against 0.9 at the drained store, where it saved nothing; the build
asks nothing now. Runs of 256 KB kept half a run ahead ran the settle in
317 ms against 399 and saved three faults in ten: at five keys a page half
a run is 0.8 ms of resolves, under the run's own round trip and transfer,
so the settle waited on most of its runs, and the lead is sized by the
resolves between the ask and the touch at the sparsest density the dense
shape admits, not by the pages it covers. In core the gate is raw and
`head_asks` reads zero through the sweep.

What the pass met after the full-rewrite burst was the burst's pieces, 336
of them, seventeen a range, a page of each in every block's build (527,000
piece runs asked for over a sweep, twenty a block), and they stood for the
whole pass: the planner ran at a seal's durable landing alone
(`Maint::landed_decisions`), a merge's own landing planned nothing, and one
merge took every due range, so after the burst's last seal one merge over
the whole store was in flight, landed all nineteen ranges at once at its
end, and nothing started another while the pass read. A merge's landing
plans the next now, while reads are around -- the store's scan life moved
since the last landing -- and at the base trigger (`Options::merge_chains`;
`supdb-nomergechain` and `supdb-ingestnomergechain` plan at seals alone),
and a merge takes the due ranges in key order up to a cap of partition
bytes, at least one, leaving the rest to the next plan
(`Options::merge_bytes_cap`, 64 MB, a range at ten million keys;
`supdb-mergeuncapped` and `supdb-ingestmergeuncapped` take them all), so
out of core the ranges land one at a time while in core, where a store's
partitions are megabytes, nothing splits. A settle joins the merge in
flight and plans nothing, as before; the landings' plans and the merges
landed are counted (`SealWaits::plans`, `SealWaits::merges`; `merge_plans`
and `merges_landed` in the suite). Paired at ten million keys under
1.25 GiB, a warmup and a pair (`bench ab --arms supdb,supdb-nomergechain
--size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps
1`): the full-rewrite pass 6.9 s in both rounds against 10.7 and 10.0, 6.2
major faults a scan against 8.6 and 1.3-1.5 GB read from the device
against 1.9-3.8, with the burst's rate level (428,000 updates a second
against 435,000) and 286 pieces standing at the pass against 336, the cell
1.385x, after 1.33x in the sitting before; the ten-percent point 0.88x on
a burst 8% slower and a pass faster in one round and slower in the other,
with the same 24 pieces and the same 3.1 faults a scan in both arms; the
one-percent point 0.92x, 1.16x in the sitting before; the drained point
level; the sweep's sum 1.23x. Three shapes were paired before this one,
the same way, and each taught something the previous had hidden. Chained
without a cap the pass met the same 336 pieces, the one merge in flight
through the whole of it, so the cap is what drains them. Capped at 256 MB
and chained through the burst as well, the pass met 108 to 192 pieces and
ran 5.3 to 9.5 s against 9.0 to 11.7, but the chained merges ran beside the
burst's writes and the burst's rate fell a seventh (378,000 against
433,000), the weighted cell 0.95x with the ten-percent and one-percent
points 0.93x and 0.95x: so the chain is gated on reads around, and a
write-only stretch keeps its seals' plans. Gated on reads at the trigger
reads lower (`l0_trigger_now`), the full rewrite read 1.33x and the
ten-percent point 0.73x: its pass took 2.9 major faults a scan against 2.0
with the same 24 pieces standing, the chain rewriting partitions of
seventy megabytes to fold a piece or two each, one after another, through
the pass, since that trigger paces the seals' plans, a merge a seal at
most, and gave a chain no pace. The suite's weighted cell charges the
burst and the pass and nothing after, so a policy that leaves the pieces
for later wins both cells and pays in the pass nobody measured: the pieces
standing at each point are read beside the cells (`pieces` in the lag
counters). In core the arms run the same code (`bench/scripts/pair.sh supdb
supdb-nomergechain 100000` and `300000`, twelve pairs a sitting in both
orders): one and two merges landed in each arm, eleven and thirteen plans
each, and no cell was marked in either sitting, since the merges land
under the load with no read around and the partitions fit a merge's cap
whole.

Paired against LMDB with its readahead off at the head that holds all of
these (`bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound
1342177280 --passes load,lag --ops-div 10 --reps 1`, a warmup and a pair):
the drained pass 0.78 and 0.86 s against 1.02 and 1.20, the one-percent
pass 0.34 and 0.43 against 0.33 and 0.35 (its first scan the burst's
settle, 217 and 331 ms), the ten-percent pass 2.5 and 2.7 s against 0.86
and 1.0 with 3.0 major faults a scan against 1.8, and the full-rewrite pass
6.3 and 6.5 s against 1.1 with 5.9 faults a scan against 2.0 -- 0.17x
where the row had 0.03x. LMDB's bursts run at 14,000 updates a second
under the bound against our 350,000 to 430,000, so the suite's weighted
cells, the burst and the pass together, read 1.9x, 1.4x and 4.5x in our
favour at the three points and 3.6x over the sweep, and the shuffled load
10x. What the full-rewrite pass still pays is the pieces standing at its
start and the merges' streaming beside its scans, and the ten-percent
pass's three faults a scan against LMDB's 1.8 are the next thing to take
apart.

Every major fault of a sweep recorded with the instruction that took it
(`perf record -e major-faults -c 1 -d -k CLOCK_MONOTONIC` around the
pairing above, cut to the passes by `SUPDB_LAG_MARK`'s stamps) put the
ten-percent pass's 69,000 faults 54% in `Reader::cache_key_at` over the
partitions' records -- the builder ahead's splice bounds of every block it
built, a block's first key at a record a page -- 20% in the copies out of
them and 8% in the asks for them, and the full-rewrite pass's 85,000 in
the pieces' records and directory words (`overlay_runs`,
`Blob::willneed_ranks`), the pieces' heads a bounds walk reads
(`table_bounds`) and the compact form's pages. Three levers came of it.
The first was refuted and is kept as an arm (`Options::boundary_keys_from_top`,
off; `supdb-boundarytop` and `supdb-ingestboundarytop`): a block's first
key is in the ordered index's top level, in memory, where the heads are
whole keys, and read there the ten-percent pass's faults fell from 72,000
to 46,000 in both rounds of a pairing while its pass ran slower, 3.18 and
2.89 s against 2.68 and 2.49, and its install phase grew from 25 and 21 ms
to 133 and 148: the builder ahead, freed of its record faults, posted its
forms faster, and the scanning thread paid to install them. The other two
ask the kernel for pages a walk already knows: a block's build asks for
its pieces' directory words together before it asks for their records
(`Options::pieces_dir_willneed`, `Blob::willneed_dir`; `supdb-nodirwillneed`
and `supdb-ingestnodirwillneed`), since the record asks read a piece's
directory word for each run's start, a cold page each, one at a time; and
a walk of a piece's keys against a partition's block boundaries, at a
landing or at a table's making, asks for the piece's heads over the
partition's range first (`Options::bounds_heads_willneed`,
`ask_bounds_heads`; `supdb-noboundsheadwillneed` and
`supdb-ingestnoboundsheadwillneed`), since the heads are advised random and
the walk reads them in order. Each was paired against its arm, a warmup
and a pair at ten million keys under 1.25 GiB: the full-rewrite pass's
major faults fell 15% with each (52,800 and 57,900 against 67,400 and
67,800 for the directory words, 58,600 and 64,700 against 69,700 and
75,900 for the heads), 30% with both against the head before them, and
its pass moved 2-4% the right way in three rounds of four, 5.9-6.8 s
against 6.5-6.9, inside the spread a pairing resolves; the other points
read level both ways. Both are gated as the records' asks are, and an
in-core sweep at a hundred thousand keys reads every count of them at
zero, so they ship on: what each costs where it fires is a syscall a piece
a block, and what it buys is the faults the counts name.

The refutation named its producer, and the producer was the cost. Out of
core a burst dirties every block of the store -- a ten-percent burst at ten
million keys is six writes a block -- and the builder ahead built a form
for every one of them from partition pages the bound had pushed out, for
scans that read a few thousand blocks of a hundred and fifty thousand. It
starts in core alone now, the controller's gate at the raw form
(`Options::ahead_out_of_core`, off then; `supdb-aheadooc` and
`supdb-ingestaheadooc` kept it running out of core, the arms now named
`supdb-aheadincore` and `supdb-ingestaheadincore` for the gate's other
side, and `supdb-noahead`
starts no builder at any size). Two sittings at ten million keys under
1.25 GiB agree: against no builder at all, the ten-percent pass took
31,000 major faults against 69,000-83,000 and ran 3.02 and 2.76 s against
3.34 and 2.96; against the arm that keeps it out of core, 30,000 and
31,000 against 68,000 and 70,000, 2.74 s in both rounds against 3.10 and
3.15, the full rewrite 7.09 and 6.83 against 9.62 and 7.39, the scanning
thread's install of builder forms under a millisecond against 15-47 ms,
and the sweep's weighted sum 1.03x and 1.108x in the gate's favour. The
one-percent point read 0.9x in the first sitting and 1.1x in the second.
In core the gate is raw and the builder starts as before.
It runs out of core again now, for the partitions whose pages the cache holds and only once a state has been scanned a hundred thousand times (`Options::ahead_out_of_core`, on; `ahead_resident_pct`, 90 percent of the form file's pages where one serves, else the record region's, by `mincore` at the builder's turn to the partition; `ahead_out_of_core_scans`; `supdb-aheadincore` and `supdb-ingestaheadincore` start it in core alone, the shape before, `supdb-aheadcold` builds whatever the cache holds and `supdb-aheadeager` waits for no scan). The refuted shape was the builder reading from the device for scans that read a few blocks; E's is the opposite -- the one mix that scans, it begins with no block cache and builds 137,000 forms inside its own window from a form resident whole, while a core sits idle. With the builder out of core E found most of them built on the builder's thread and read 1.10x and 1.07x in two sittings (`--arms supdb,supdb-aheadooc`, the arm as it was named then, `--size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2` at `1f4d2b2`), 1.04x and 0.98x in two more with the gates (`--arms supdb,supdb-aheadincore` at `8f5aaf2`), the other mixes moving 7-13% both ways within each sitting with no builder in them; and on the lag sweep the builder was refuted on, whose passes are ten thousand scans divided, it never starts (`ahead_starts` 0 in both arms of `--arms supdb,supdb-aheadincore --passes load,lag --ops-div 10 --reps 1` at `8f5aaf2`, the weighted sum 1.02x and the points 0.78-1.19x by position). The row's divided E is 158,000 scans, so the gate is crossed in its last third and the lever is the undivided E's.

In core the builder started at a state's first scan, and at three million keys under 1.25 GiB, which the controller reads as in core through the lag sweep's first three points, the pass after the ten-percent burst read two shapes, 13-19 ms and 20-41, against LMDB's 23. The pass is three thousand scans, thirteen milliseconds, over a ninth of the store's blocks, and three one-time costs decide it: the first scan's wait for the upkeep's pass in flight, 2-9 ms in some reps, its settle of what that pass left, 1.5-5, and the builder's forms, some thirty thousand, installed in one step of 3-7 ms in the middle of the pass (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb-noahead --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 6`, both orders, at `f50d31e`: 13-20 ms without the builder in twelve passes of twelve, 20-32 with it in seven of twelve; a profile cut to the pass has the builder's thread in `Snapshot::extend` and its compares beside the scans for 13-34 ms of it). In core the arm without the builder read every lag pass within its sittings' spread at a hundred thousand, three hundred thousand and a million keys, and E at 0.82x at a million in four pairs of five (`bench ab --arms supdb,supdb-noahead --size N`, ten, eight and five reps, at `cbfbaee`): the builder's worth is a state scanned many times over, and a pass shorter than its run pays for forms it never walks. A scan starts it in core once the state's scans number an eighth of its blocks now (`Options::ahead_in_core_scans_div`; `supdb-aheadfirst` starts it at the first scan), by when the scans have walked about a fifth of the blocks themselves; out of core the count above decides, and a commit that starts the builder starts it as before. Paired at `ec95e31` under 1.25 GiB at three million keys (`--arms supdb,supdb-aheadfirst`: the lag sweep alone with six reps and the two variables, both orders, and `--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --ops-div 10` with three reps and with four, both orders each), the ten-percent pass read 12-29 ms against 14-34 in twelve passes each, medians 16.6 and 19.3; E 1.20x, 1.16x, 1.00x and 1.25x, eleven pairs of fourteen; D 1.05-1.14x in all four sittings; and the scan pass 1.07x, 0.86x, 1.20x and 1.24x. In core at a million keys in both orders and at three hundred thousand (`--size 1000000 --reps 5`, `--size 300000 --reps 8`) nothing moved beyond its spread. The pass after the full rewrite is out of the option's reach, and was read as its cost for an hour: it read 23-86 ms against 18-79 in the same twelve passes, and 19-71 without the builder against 14-80 with it in the twelve before them, four sittings one way, while the trace had the controller in its census before that pass in every rep (`zselect ... gate 1 census true`), where the builder waits for a hundred thousand scans in both arms. That pass waits 20-60 ms in one rep of three whatever the builder does, and a first reading of the wait -- the scanning thread freeing the segments a landing retired -- was refuted by a trace of its syscalls before anything was built on it (`strace -f -tt -T` over the sweep, cut to the pass by the lag marks: no `munmap` or `unlink` of the thread's inside any pass). What the trace does show there is the census read as out of core: the scan asks the kernel about every block it builds, 1,400 `mincore` calls and 240 `madvise` in a pass of three thousand scans over a store that is resident, which is open with the first scan's wait.

That pass, the scan after the full rewrite at three million keys under 1.25 GiB, was the one cell of its rung the row at `ee84efe` lost, 43, 54 and 88 ms against LMDB's 25-35, and what it paid for was a second build of every form. Its sweeps read two shapes that the counts separate before the times do (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_MIX_EVENTS=1 bench ab --arms supdb,supdb-rebasedropsshed --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 6` at `ac36b0b`, the arm being the default as it stood, and the point's `lagcounters-events` line beside its `lagops`, `lagdev` and `lagcensus`): in one the upkeep thread builds each of the store's forty-seven thousand blocks once over the burst, a conversion of the sparse form the ten-percent burst left it; in the other up to a hundred thousand, the second builds from the sources and sized for an overlay as large as the block, a hundred megabytes more in use, the burst a second longer, and the pass after it meeting a hundred to two thousand sparse forms whose walks read records the bound had pushed out, up to two hundred major faults in a pass of three thousand scans, the scanning thread's voluntary switches one for one. Two things built a form twice. The fills' room gate (`SettleRoom`, below) shed the forms the fills had built when the group's refaults moved inside the burst, forty thousand at once, and while the cache's budget is not bound the commit's own fill builds a form for every overlaid block that holds none, so the pass that shed them built each again: a shed its own pass undoes, in five sweeps of the arm's fifteen. There is no shed while that fill stands to undo it (`Options::forms_settle_sheds_unbound`; `supdb-settleshedunbound` sheds), and the list the shed reads, which only a shed had emptied and which in memory grew by an entry a build for the store's life, is pruned to what a shed could take (`Reader::prune_settle_built`). And the look that carries a table across the merge that rewrites its partition over the same blocks dropped the partition's sparse forms, a delta's cut being a rank in the old records, so the landings inside a burst sent two thousand blocks back to the fill and the ones a late landing left with few keys since stood sparse at the pass; the look resolves each delta's cut again and keeps the form, whose runs are the keys' whole runs and newer than anything the merge folded in (`Options::rebase_recuts_sparse`; `supdb-rebasedrops` drops, and `tests/settle.rs` holds both options to their effect). Paired at `ac36b0b` in both orders and in the row's order (the command above, the arms the other way, and `--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --reps 3`), the pass read 15-41 ms against 15-136 in fifteen sweeps each, medians 20 and 27.5 and the default ahead in the median of each of the three sittings, with no major fault in any of its passes, two to seven sparse forms at the pass against a hundred to two thousand, and forty-seven thousand builds a burst in every sweep. Against LMDB the sweep's four passes read 1.4-2.3x, five pairs of five each (`--arms supdb,lmdb-nordahead --passes load,lag --ops-div 10 --reps 5` at `d7d0acf`). In core a cut is resolved again for a dozen forms a sweep at a million keys and for none at three hundred thousand, and nothing moved (`bench ab --arms supdb,supdb-rebasedrops --size 300000 --reps 8` and `--size 1000000 --reps 6`, the arms the other way, at `d7d0acf`); at ten million keys under the bound the lag sweep resolves none, its forms given up for the merge path. What is left of the pass is its first scan: up to 22 ms of wait for the upkeep's pass in flight and settle of what that pass left, on a base of fourteen.

The wait is the upkeep thread's rate, and so is a fifth of the burst before it. Under the syncing pace the thread trails a burst by at most a quarter of a percent of the store (`Options::upkeep_pace_syncs_bp`), and what it cannot file inside that trail holds the writer: over the full rewrite at three million keys under 1.25 GiB the thread ran 7.0-7.4 s of a burst of 9.5-10.3, the writer held two and a half of them, and the first scan after the burst waits out whatever pass is in flight. The counts the sweep already printed said what a pass holds there (`SUPDB_MIX_EVENTS=1`'s `lagcounters-events` line: `settle_runs`, `settle_keys`, `settle_blocks`): 2.70 million keys in 2.62 million visits to a block over twelve to nineteen hundred passes, so a pass is a couple of thousand keys, every visit is one key's, and the keys stand sixteen hundred ranks apart -- where the settle's loops were tuned under the buffered pace's passes (`Options::upkeep_pace_pct`), a hundredth of the store, several keys a block and two hundred ranks apart. A profile of the burst cut to the thread (`perf record -a -g -e cpu-clock -F 2000 -k CLOCK_MONOTONIC` from the lag mark that ends the ten-percent pass to the one that ends the full rewrite's, around `SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 1` at `58f54f3`) put 30% of it in the settle's own loop, on loads of lines nothing had touched since the block's last visit, and 17% in the gallop from the last answer (`cache_seek_exact`), which over that distance is a dozen probes and a binary search of ten more, each a line that waits on the one before. Five asks ahead took them, each an option with the loop as it stood for its arm, each sized by what a pass holds now. The stride of heads a raw resolve will search is named from the ordered index's top level, which is in memory, a few keys before the seek, and asked of the core then (`Options::settle_seek_ahead`, `OrdIndex::plan_seek` and `seek_exact_planned`; `supdb-settlegallop` gallops). The header of the form a key lands in is asked for by its slot's pointer, ahead of the ask for the form's buffers that reads it (`Options::settle_form_ahead`; `supdb-settleformnear`). A patch into a copy by its rank asks for the one entry and the run it writes, by the key's cut, where the form was asked for whole, forty-eight lines that missed two of the patch's own (`Options::settle_patch_at_cut`; `supdb-settlepatchwhole`). The written value's lines are asked for with its chunk's header (`Options::settle_value_ahead`; `supdb-settlevaluenear`). And the block table's own lines for a key -- its slot, the promoted copy's, the reads' and its list of created keys -- are asked for further ahead still (`Options::settle_table_ahead`; `supdb-settletablenear`). `tests/settle.rs` holds each to firing with its option and not without, the planned seek to the seek's answer from every carried position (`ordindex`'s tests), and a batch filed under either seek to the scan. Paired at `4b8ff45`, all five against none (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_MIX_EVENTS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-settlenoahead --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 5`, and the arms the other way), the thread's settle step over the full rewrite read 4.9-5.0 s against 7.8-8.2, its CPU 6.2 against 8.4-8.8, the writer's holds 1.4 against 2.8-3.2, and the burst 9.3-9.6 s against 10.9-11.6, 1.17x and 1.21x; the ten-percent burst 1.17x and 1.20x and the one-percent 1.13x and 1.12x; and the pass after the full rewrite 16.5 and 17.6 ms against 21.0 and 22.4, its first scan a tenth to a whole millisecond against four. The thread's passes doubled, 2,300 against 1,200, since a thread that keeps up is handed less at a time, and the fill's walk over the table, a pass's fixed part, went from 0.3 s to 0.9-1.1 with them. Each taken away alone, the same command with its arm's name, on the host the machine was restarted onto between the first pairing and these, where the burst takes seven seconds and not nine: the planned seek read the settle step 4.6 s against 6.9 and 3.7 against 4.3, the burst 1.07x in both orders; the patch by its cut 3.8 against 4.1 and 3.6 against 4.2, the thread's CPU 7-9% lower and the burst 1.02-1.03x; and the header's, the value's and the table's asks each within four percent of its arm on the step and three on the burst, theirs the direction in most of their six sittings and none resolved alone, since what one of them asks for sits beside what the others do -- on the trees that added them one at a time, before the patch asked for its lines by the cut, the header's had read the step 7-8% lower and the table's 9%. In core at three hundred thousand keys (`bench ab --arms supdb,supdb-settlenoahead --size 300000 --passes load,lag --reps 30` at `4b8ff45`, both orders) the full rewrite's burst read 1.03x and 1.07x and the loads, the smaller bursts and their passes within five percent either way; the pass after the full rewrite read 0.98x and 0.94x, where two sittings on the tree before the commit had read 0.95x and 1.01x, a few percent the pass may owe and no sitting resolves.

One more was built and is off. Out of core a settle asks the kernel for the heads its resolves will seek, and each ask first reads its own page's residency, a `mincore` a key, a tenth of the thread at three million keys under 1.25 GiB, where the store sits at the controller's census with nearly all of its ordered indexes in memory. One look at a partition's whole index standing for a hundred milliseconds in the asks' place (`Options::heads_look_ms`; `supdb-headslook` is the look) took that tenth off the thread's CPU and read the burst one to three percent longer in both orders, the writer held a quarter to a half longer: every major fault of a sweep recorded with its address and laid to its file (`perf record -a -e major-faults -c 1 -d -g -k CLOCK_MONOTONIC` around `bench ab --arms supdb,supdb-headslook --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 5` on the tree `4b8ff45` committed) put three to five thousand of the thread's faults a burst on the ordered indexes under the look where an ask a key takes 250-720, since under the bound the indexes' pages leave the cache all through the burst, between two looks as between two seeks. The same record named a part of the pass's tail: its scans took eleven to twenty major faults on those pages in one full-rewrite pass of three, in the start seek, and those were the passes that read 28-42 ms where the others read fifteen.

The sittings on the second host carried something the first had not: thirty of their ninety full-rewrite bursts, either arm's, ended with commits the pace had not held, and the pass after twenty-eight of those read 28-162 ms and once 715, half of them over a hundred, where the sixty others read a median of sixteen; none of the thirty bursts on the host before had. The counters split the bursts before the times did. `lagcounters`' `holds` is the commits the pace held, 2,700 of 2,700 in every other burst and 2,363 to 2,668 in these, with the first scan's `sync` phase up to 10 ms where it is a tenth of one and its `settle` up to 118: the burst's last thirty to three hundred and forty commits had been neither paced nor filed, and the first scan read their log and settled them. Both stops are the same test, whether scans are around, two stores' worth of writes since the last one being far from reads, and both kept the count the same way: the commit's hand-over by its batch and the pass by the log's growth (`Db::hand_over_upkeep`, `Reader::settle_due`), each "the whole log where the generation has changed since". The generation is bumped by every publish, and a landing from the segment work publishes over the live table it leaves alone, so every landing -- a seal's two and a merge's, for every hundred commits or so of a full rewrite -- added that table's whole log to both counts again; with one store's worth written the counts ended near two, under the line on the first host and over it for the burst's last few percent on the second, where the same binary runs the burst in seven seconds and not nine. Both counts are keyed on the live table now (`State::lives`), as the lag tick's has been since the same mistake was found there, and `tests/db.rs` holds each to the writes across a freeze and a held seal's landing. It is a fix for a wrong count and has no arm; it was priced on the tree that measured, with the old count kept behind an option no commit holds (`supdb-writesbygen`, `bench ab --arms supdb,supdb-writesbygen --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 8` with the counters above, and the arms the other way): all sixteen bursts held 2,700 commits and their passes read 12-23 ms, where the old count held 2,592 and 2,597 in two of its sixteen, whose passes read 69 and 63 ms, 40 and 42 of them the first scan's settle. In the row's whole order at three million keys (`--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --reps 2`), through the mixes at ten million (`--reps 1`, no lag sweep) and on the lag sweep there (`--reps 2`), no cell the count reaches moved the same way in both orders; at ten million keys the thread files nothing of the sweep and no commit is paced, so there is nothing for it to move. The single-threaded scan pass, which makes no commit, read 0.93x and 0.82x in the row's order at two reps and 1.10x and 1.09x at six (`--passes load,read,read-threaded,scan,scan-threaded,mixes --reps 6`): that pass's spread on this host, between arms that run the same code through it.

What was left of the pass's tail was the device's. Every seek of a raw partition reads the ordered index's heads, eight bytes a key in a mapped file, and at three million keys under 1.25 GiB the store sits at the controller's census through the full rewrite, read through the raw form with the page cache at its limit: the heads' pages leave between two seeks however often they are read, the upkeep thread asks the kernel for every stride it will seek and checks each ask's page first, a `mincore` a key, and a scan's start seek that finds its page gone waits on a device the merges are writing to. A partition the controller is leaving to the raw form has its heads copied into the process's own memory now, where they are not the cache's to take and the bound counts them either way, and the index reads the copy (`Options::heads_owned`, `OrdIndex::set_heads`, `Maint::z_heads_look`; `supdb-headsmapped` reads the mapping throughout); the mapping's pages are let go, and a copy is given back where a form file comes to serve its partition, retired under a fresh epoch and freed past every reader pinned before it. Which partitions are left to the raw form took three rules to say, and the first two were priced at ten million keys by their counters before any rate (`SUPDB_MIX_EVENTS=1`'s `heads_owned` and `heads_disowned`): whatever the raw form was serving at the look made ninety-two copies over a lag sweep there and gave back ninety-two, since a partition a merge lands is served raw for the tick before its file is attached and every partition for the second between the load's crossing and its install, and the copies at the crossing read the index files from the device beside the first scans, a pass of 502 ms against 404; waiting for a controller's step that had bought nothing still made eight to twenty at the crossing, where the controller sits a moment on a purchase its model has predicted. The rule that stands is the controller's own account: under the form's gate a partition it has declined (the hot-raw policy's release, a sliver under the floor), and at the census every partition while its model says the raw form fits, the census being the group's pressure (`heads_left_raw`); only while scans are near, a few partitions a look. `tests/zpages.rs` holds each branch, the copies given back to waiting for a pinned handle, and the index's answers through a copy to its answers through the mapping. Paired at `7b90eb8` (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_MIX_EVENTS=1 bench ab --arms supdb,supdb-headsmapped --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 8`, and the arms the other way): over the full rewrite the thread's settle step read 3.4 s against 4.5 and 3.3 against 4.6, its CPU 5.3 against 6.0-6.1, the writer's holds 1.1-1.2 against 1.4 and the burst 1.04x and 1.06x, with twenty-three to thirty-four copies a burst, one for each partition the merges land; the pass after it read 17.9 ms against 20.4 and 20.2 against 18.9, the mapping's pages having stayed through these two sittings' passes. In the two sittings of the tree before the last rule, which is the same rule at this size, they had not: the mapping's passes read 30-50 ms in six of sixteen, with 40 to 585 major faults in the group across each, where none of the copies' sixteen passed 20 ms. At ten million keys the sweep makes no copy and the two arms run the same code (`--size 10000000 --reps 2`); through the mixes there the policy's released partitions are copied, eight a run, and no cell read the same way in both orders at one rep each (`--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --reps 1`); nor in the row's whole order at three million at two.

#### Out of core: the second form

A pairing this section cites is given as the command that reproduces it, `pair.sh A B SIZE [bench ab options]` (`bench/scripts/pair.sh`: `bench ab` over the two arms, one sitting in each order), at the commit it ran at; a binary built at that commit runs it (`BIN=`), and every arm named exists at the head as well. A rerun reproduces the comparison, not the rates, which this host moves by tens of percent between sittings. A count a sentence quotes -- decodes, faults by mix, the controller's events, the census -- was read with the variable before the command set, which turns supdb's own timers on as well, so a rate against a comparator is read from a run without it. Pairings made before this history's root (2026-10-20) cite the root, `18c3a30`, the first commit here that holds their arms. The design the form was built to is `docs/zpages-design.md`.

A point read of the raw form touches two regions of a partition, the hash slot and the record, and the cache is charged for every page either lives on. The second form is a derived file beside each partition, `zpg-<id>-<end_seq>.zpg` (`Options::zpages`, off by default): the partition's records verbatim in groups, each compressed with LZ4 (the engine's codec) to fill one 4 KiB page, and a fence of the first key of every page held on the heap. A point read of a covered partition binary-searches the fence, reads one page, decodes its group and walks to the key; it touches nothing of the raw segment, so the raw hash region and record region leave the working set and what is resident is one population of pages, a little over half the raw records' size at this suite's values. The form file is bound to its segment by content (key count, record length, the key section's checksum row, the build's timestamp, the segment's id and end sequence), so a copy under another segment's name, or one whose segment was rewritten under the same name, is refused and the raw form serves; every page carries a checksum xor'd with the binding; a page that fails a check is read from the raw form and counted, never answered absent. It is derived data: written unsynced from a temp name, renamed into place, swept at open when no live segment claims its name. Point reads and counts read it; scans, the pieces, the frozen tables and the live table are read as before.

A form file is built on a thread of the store's own at idle priority -- the CPU's idle class, and the device's unless `Options::zpages_io_idle` is off, which `supdb-nocacheautozpagesfairio` priced against the controller's arm under a bound and found level, the class never having limited the build -- one at a time, its walk asking the kernel for the raw records four mebibytes ahead of itself (`Options::zpages_build_ahead`; `supdb-nocacheautozpagesnoahead` asks nothing, the shape whose every wait fetched the device's 128 KB readahead and whose build of six hundred megabytes under a bound took 66-93 s where the window takes 29-60), from the segment's file opened by the segment work before the thread starts; a merge that takes the partition as an input cancels the build, and a build that finishes after the landing that retired its partition leaves a file the segment work renames to a dead name and unlinks at its idle tick, where a freed inode costs nobody a wait. The landing that retires a segment renames its form file the same way. A form file a page check took out of service is retired past every reader pinned on it, as a replaced state is, and the partition's is built again, three times a segment at most; a build that fails is tried again after a backoff of a minute doubling to an hour. Under `ZPages::Always` a partition's build starts as the partition lands (`Options::zpages_eager`, on), and the settle of the form, which the suite's `sync` calls, waits for the builds in flight and starts the ones no landing did; `supdb-nocachezpagessettle` is the shape that leaves every build to the settle. Priced at three million keys in core, the load pass alone, two sittings with the arms swapped: the ordered load reads 1.2-1.3x with the builds started at the landings, its closing sync a third of what it was (about 0.5 s against 1.6 s) while the commits rise by a tenth of the difference, the builds running on the builder's idle-class thread beside the commits where the sync had run them one after the other; the shuffled load reads level in both sittings, with four to eight builds of twenty-five superseded by its merges and a third more build bytes for the same files on disk (`pair.sh supdb-nocachezpagessettle supdb-nocachezpages 3000000 --reps 6 --passes load --ops-div 10` at `18c3a30`, the sync and the commits read with `SUPDB_LOAD_PHASES=1`). A record no page holds -- over the largest image a group decodes into, or fitting no page compressed or stored -- is an entry of the fence with no page, and the build goes on past it: a read of that record is the raw form's and counted as such (`nopage_reads`), a key in the entry's range that is not the record's is answered absent from the fence, and the partition is declined only when no record of it fits a page. A filter over every key of the partition, the records no page holds included, goes into the resident part beside the fence (`BlockedBloom`, the structure the level-0 tail is guarded by), with its signature and parameters in the trailer: a read of a key the filter does not hold is answered absent from the heap before the fence is searched, no page touched, and an attach refuses a stored filter whose signature is not the running engine's, whose size is not the one the engine builds for the key count, or in which any key of the fence tests absent -- a filter is a function of engine code the segment's checksum row cannot see, and a stale one would answer present keys absent with nothing raised. Once a form file is attached, its file is read into the page cache by a job on the builder's thread (`Options::zpages_warm`, `zfile::warm_file`): `MADV_WILLNEED` and a touch of each page in chunks of eight megabytes, the chunks `mincore` reports resident skipped, cancelled where a merge retires the partition, once per form file per process, and the settle of the form waits for it as it waits for a build. A form file just built is resident and skipped whole; one on disk at a restart is cold, and a divided read pass over a cold file faults as the raw form does, which is the warm's reason. The headroom a bound leaves is not consulted yet. `supdb-nocachezpagesnowarm` is the arm with the warm off, chosen by hand against `supdb-nocachezpages` and not priced yet: the forced arm's `sync` builds a form file and the warm after its attach finds it resident, so the warm's work is a restart's, and the runner has no pass that reopens a loaded store cold. A scan's walk of a covered partition goes through the form file too (`ZFile::scan_at`, the bulk walk of `scan_partitions`): the entry the rank falls in by the fence's first ranks, its group decoded once into the thread's buffer and the walk skipping to the record by `record_key_next`, each record parsed and its extents read as the point read reads them, the next entry's group when one is spent, and a record no page holds handed to the raw form for exactly its run (`nopage_scans`); a check that fails ends the walk at a record boundary and the raw form walks the rest, counted a fallback and a raw walk. The counts are `scans` (walks served), `scan_groups` (groups decoded for them) and `raw_scans` (walks the raw form made under the form: no form file in the cell, one out of service, the rest after a check). The seek a scan starts a partition with, the cut where an unsealed key falls inside the walk and the partition's values at an unsealed key's emit go through the form file too (`ZFile::seek`, `ZFile::cut_in`, `ZFile::values_at`): the fence names the one group the key can be in and that group is opened, where the ordered index's seek took its heads and a key read of the raw records at each probe. The merge path a store with a level-0 piece takes (`scan_merged`) walks a covered partition by a cursor (`zfile::ZCursor`): the group held in the cursor's own buffer across the keys it lends, the next entry's group opened at the group's end, a run of records no page holds walked through the raw form for its ranks, and a check that fails under the cursor making the rest of its walk the raw form's with the form file out of service, so that every key is still answered; the cursor advances at the top of the turn after the one that emitted its key, since the key it lends is in use until that turn's end, and its counts are filed as it leaves the partition, a served walk or a raw one. The pieces' cursors are raw, since a piece has no form file. The raw form is behind one door (`SegBlob`, `Seg::raw`): the segment's blob forwards only what has no raw footprint, the key count, the id, the index size and the advice, so every read of the records goes through the form file's dispatch or through the door on purpose -- the raw side each dispatch falls to, the `cursor_merge` arm, the pieces' cursors and the tombstone pre-pass over them, the block cache's machinery, which builds its forms from the records and which the nocache arms never run, and the segment work's own passes at a landing, a promotion or the open -- and under debug assertions a touch through the door on a segment with a form file installed is counted (`Reader::raw_touches`), which the tests hold to zero over a covered partition's point reads, counts and scans, drained, with unsealed keys inside the window, over a level-0 piece, after a merge's landing and beside a record no page holds, on the writer and a handle, with the record no page holds the positive control (`tests/zpages.rs`, `the_compact_form_touches_no_raw_page`). A compressed page is opened through the reader handle's memo of decoded groups (`Options::zpages_memo`, on; `zfile::ZMemo`): a thousand and twenty-four entries, direct-mapped by the page plus a multiple of the form file's attach identity and capped at eight megabytes of decoded bytes a handle, each an `Arc` to its buffer, so a read of a page the handle decoded before pays the walk of the group and not the decode, and holds its group across a callback's nested read, which may displace the entry without taking the group away; the memo is consulted for a page the process has verified, since a hit repeats no check as the verified path repeats none, and a first decode's group goes in as it passes; a form file retired takes a new identity at its next attach, so its entries never hit again. The memo is the handle's and not the thread's, since a thread's kept the groups of every store the thread had closed. Its hits and misses are counted on the handle's line (`memo_hits`, `memo_misses`), and `supdb-nocachezpagesnomemo` is the arm without it, priced in core at three million keys in two sittings with the arms swapped (`pair.sh supdb-nocachezpages supdb-nocachezpagesnomemo 3000000 --reps 6 --passes load,read,mixes --ops-div 10` at `18c3a30`): the Zipfian C mix reads about 1.5x with the memo and the E scan mix about 3x, six pairs of six in both sittings, the uniform point reads level by the mean with their p99 a fifth higher, and the rest within the coin. The buffers of displaced entries that no read still holds are kept for the next decodes, since a buffer allocated for every decode the memo took had cost the uniform reads a tenth. Priced out of core at ten million keys under a gibibyte, the operations divided by ten, two sittings with the arms swapped (`pair.sh supdb-nocache supdb-nocachezpages 10000000 --reps 6 --bound 1073741824 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `18c3a30`): the scan pass through the form runs at 9.6-10.4x of the raw form's on one thread and 6.1-6.7x on two and three, with no fault a scan against the raw form's 3.1 and 1.6; the point reads at 2.7-2.8x, 4.0-4.4x and 5.0-5.1x; the mixes A 1.4x, B 2.3-2.5x, C 4.3x, D 3.6-4.0x and F 1.5-1.6x; and E 1.12x in both sittings with its faults an operation down twenty-fold, the one cell short of the design's registration, whose time is to be split by kind before anything is changed for it; the loads 0.74-0.79x, the builds in the forced arm's sync. In core at three million keys the same pairing reads the scans at 0.30x on one thread and 0.25x on two and three, the point reads at 0.17-0.19x and the mixes at 0.19-0.74x (`pair.sh supdb-nocache supdb-nocachezpages 3000000 --reps 6 --passes load,read,scan,scan-threaded,mixes --ops-div 10` at `18c3a30`): the decode against a walk that reads its records in place, which is why the form is the controller's to enter past memory and not a default. Two arms move the scan's own levers: `supdb-nocachezpagesheads` (`Options::zpages_seek_ord`) seeks a covered partition, and cuts at an unsealed key inside its walk, by the ordered index's heads and the raw records as the scan did before the form served it, with the walk after the seek the form file's; priced against the fence's seek out of core in two sittings with the arms swapped (`pair.sh supdb-nocachezpages supdb-nocachezpagesheads 10000000 --reps 6 --bound 1073741824 --passes load,scan,scan-threaded,mixes --ops-div 10` at `18c3a30`), the fence reads 1.2x on one thread and 1.1x on two with a quarter fewer faults a scan, level on three threads and across the mixes, and E level by rate with a tenth of the faults, since the walk after the fence's seek finds the seek's group in the memo and adds no decode while the heads' seek faults on the heads and a raw record a probe, so the fence stays and the heads are the arm; and `supdb-nocachezpagesprefetch` (`Options::zpages_scan_prefetch`, off) has a walk that wants more keys than its first page holds ask the kernel once, at that page, for the pages it will cross after it -- the keys still wanted over the mean records a page, sixteen pages at most, `MADV_WILLNEED`, counted `prefetches` -- so that out of core the walk finds them coming in instead of faulting on each, where in core the advice is a syscall for pages already there; priced the same way (`pair.sh supdb-nocachezpages supdb-nocachezpagesprefetch 10000000 --reps 6 --bound 1073741824 --passes load,scan,scan-threaded,mixes --ops-div 10` at `18c3a30`), it reads 1.2x on the scan pass straight after a load, whose walks fault the form files in at 0.6 a scan and with the advice at 0.25, and 0.94-0.98x on the warm threaded passes, where the advice is a syscall for pages already there; the mixes level. Those are the two regimes the design named, so the option stays off and is the controller's lever, turned on by the census's fault rate over the scans and never by a flag. The overlay walk of a covered partition -- `scan_partitions`, the path a store with no level-0 piece takes, with the memtables' keys laid over the partition's -- goes through the same cursor now (`Options::zpages_walk_cursor`, on; `ZCursor::walk_until`): the cursor walks to each unsealed key inside the window emitting the partition's keys below it, lends the record under it to the emit when the key is the partition's too, steps past it and walks on, its group held across all of it, so an unsealed key costs the records between. The walk cut by rank that came first -- a seek to the unsealed key, a fresh walk after it and a values read at its emit, each opening the group again -- is the arm `supdb-nocachezpagesrankwalk`, and the E mix is where the two differ: the suite's Zipfian is not scrambled, so the mixes' updates and E's scan starts pile onto the lowest keys, a window of fifty there crosses about sixteen unsealed keys, and the walk by rank opened a group thirty-four times for it (`SUPDB_MIX_COUNTERS=1 bench ab --arms supdb-nocachezpagesrankwalk,supdb-nocachezpages --size 3000000 --reps 2 --passes load,mixes --ops-div 10` at `18c3a30`, the counts per mix) where the cursor opens one or two, which is what E's time was: 1.12x of the raw form out of core with its faults down twenty-fold, and twice the cost of the pass's scan of a hundred in core. Priced in two sittings each with the arms swapped (`pair.sh supdb-nocachezpages supdb-nocachezpagesrankwalk 3000000 --reps 6 --passes load,scan,mixes --ops-div 10`, and the same at 10000000 with `--bound 1073741824 --passes load,scan,scan-threaded,mixes`, at `18c3a30`), the cursor reads E at 1.8-1.9x of the walk by rank in core at three million keys and 1.6x out of core at ten million under a gibibyte, six pairs of six every time, with the scan passes, the other mixes and the loads level and the group opens fourteen and twenty-seven times fewer; against the raw form out of core E stands at 1.6-1.8x where it read 1.12x, its scan of fifty costing about what the pass's scan of a hundred does, so what is left of E is the overlay's own work and the decodes the memo misses. The block cache's reads of a covered partition go through the form file too, while the gate serves it (`Options::zpages_block_cache`, on): a block's merged form is built from the form file's records, the walks between a form's keys and over a clean block go through it (`Reader::cache_scan`, `ZFile::scan_at` for each run between two cuts, counted a walk each), the partition's own record at an unsealed key's emit is read through it (`cache_values`), and a block's boundary keys, the last key a table keeps, the cuts of the unsealed keys and the block a written key belongs to are answered by the fence and one group (`cache_key_at` through `ZFile::key_at_into`, `cache_cut`, `cache_seek_exact` through `ZFile::seek`), on the writer, a handle, the upkeep thread's understudy and the builder ahead alike, each through its own memo; the raw prefetch of a block's records is not issued under the form, whose walk asks for its own pages. What stays raw is a piece's side of every join the cache makes -- its bounds and ranks against the partition, its records at an overlay key's emit -- since a piece has no form file, and the segment work's own joins at a landing, which have no handle. The key of a record no page holds is the fence's, since its entry covers that one record and names its key, so the last key of a partition whose last record no page holds costs no touch; the record's values stay the raw form's. `tests/zpages.rs` holds a cache-on store to zero touches through the same phases the nocache store is held through, with the option off and with the gate set to the raw form as the positive controls, and the windows test counts the walks the block path makes through the form. Before this the cache's forms were built from the records through the door and its walks stayed raw, so with the cache on the form served scans nothing and the nocache arms were where the scans were priced; `supdb-cache64tofitautozpagesraw` keeps that shape. Under the form a clean block's walk is a group decoded a walk, five microseconds a page at five million keys, where the raw walk streamed its records, so the cache's promotion of a block the reads have walked `promote_entries` entries' worth (`Options::promote_entries`, a copy beside the cheap form for a block walked as deltas) has a second shape under the form (`Options::zpages_promote_copies`, off as shipped): the copy stands in the handle's slot as the block's one form, a clean block included, judged by the form the walk read (the canonical form as much as the handle's own) and read before the canonical form once it stands, listed and shed under the budget like any form and patched by the writes into it; the bulk walk of a partition with nothing unsealed over it, which would decode a group a page every time, goes through the blocks under it so the blocks read again are promoted. `supdb-cache64tofitautozpagespromote` is the arm, and the differential fuzz runs it under `ZFUZZ_PROMOTE`. Priced against the combined arm at five million keys under 640 MiB in both orders (`pair.sh supdb-cache64tofitautozpages supdb-cache64tofitautozpagespromote 5000000 --reps 3 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `436040c`) it loses every mix, E at 0.7-0.8x and the point-read mixes at a third to a half, faulting on every one where the combined arm faults on none: its copies covered the hottest blocks, whose pages the memo held already, so E's walks through the form fell by three tenths and its decodes by three percent, and the copies' 64 MB came out of a page cache the form files had just fit, so every mix paid the form's pages faulting back in. Under a bound the form just fits, a structure that trades page cache for decoded bytes buys a fault for the decode it saves; the decodes left are the memo's misses on the warm tail, and the arm stays off. The differential fuzz runs with the form forced in one store of its pair (`ZFUZZ_ZPAGES=1`), every read, count and scan of a covered store against the raw store's over the same writes, with the block cache on in the seeds `ZFUZZ_CACHE` gives it.

The controller (`ZPages::Auto`, `src/db/zselect.rs`, the design's section 4) is what makes the form a choice and not a configuration: in memory the form loses every read cell to the decode, out of core it wins every one, and only a store that reads its own faults can tell which regime it is in. The selector is a pure function of its state and an explicit input -- the model, the sampler's reading, the census, the builder's report -- with the phases FITS, SUSPECT, building, COMPACT and the trial and every constant named for the quantity it stands for, and a test holds the module to naming no clock, thread, file or engine type, which is why the platform's reading lives in `zfile.rs` beside the file it is for. What a read sees of it is one word: `Shared.zgate`, on a line of its own, `GATE_RAW`, `GATE_CENSUS` or `GATE_Z`, pinned at the raw form by `Off` and at the compact form by `Always` and moved by the controller's tick alone, and every dispatch to the form -- the point read, the count, the walk, the seek, the cut, the values, the cursor -- asks it first, so a cell attached under the raw gate is served raw and counted nothing, which is how the trial will read the raw form with the form files still attached. Counted by callgrind over twenty thousand uniform gets of a partitioned store of ten thousand keys under `Off`, the probe before the gate and after it (`bench/examples/read_only.rs`, the toggle on `supdb_bench::engines::supdb_get`): 16,609,943 instructions against 16,609,938, 830 a get both ways, since the gate's load stands where the cell's load stood. The tick runs on the segment work's thread every census interval, reads the model once a second -- the cache budget from the platform (the v1 memory controller's group where the process has one, else the unified hierarchy's least of `memory.max` and `memory.high` over the path to its mount, else the machine, less the anonymous memory and a margin of a twentieth and sixty-four mebibytes at least, and held under the machine's `MemAvailable` less the same margin, since neither a group's limit nor the machine's total sees the other processes beside the store; `zfile::read_budget`, the parsers ported from the suite's `bound.rs` and never shared with it), the group's refaults a second, and the raw form's predicted faults a read over the partitions' two populations (`Seg::foot`: the hash regions and the record regions, one page of each a read, `zselect::model_raw`; not the index section, which holds the record region for a record whose value is inline, so that a section-sized hash population counted the records twice for as long as the controller shipped, and predicted 1.25 faults a read at five million keys under 640 MiB where the census measured 0.27; the counts carry both populations, `ZCounts::model_hash_bytes` and `model_record_bytes`, and `tests/zpages.rs` holds them to the files' headers and a store whose populations fit its budget to FITS) -- and publishes the decision: the gate, the phase and the selector's counts. Under `Auto` where the controller cannot run (the segment work without a thread of its own, an unpinned writer, a platform that is not Linux) the store opens all the same and behaves as `Off`, counted (`auto_inert`), where `Always` beside an unpinned writer is refused: a request that cannot be honoured is an error and a default that cannot be honoured is not. A test's seam (`Options::zpages_inject`, `ZInject`) gives a store its own budget, refault rate and clock, so a dwell of minutes replays in milliseconds and a test needs no bound. With this piece the rung moves between FITS and SUSPECT on the model alone, and nothing is built: the census, the sampler and the builds under the controller are the pieces after it.

The census (S2, the design's 4.3) is the controller's second signal, and it is in band. What the engine cannot see is that a read blocked: the first touch of lent bytes is a page fault inside `seg_read`, so it is inferred from the thread's own counters and never by timing lookups against a threshold. A counted handle of a store under `Auto` keeps a window (`ZWindow`) whenever the gate is not the raw one: opened by the first operation, it brackets `n` operations -- sixty-four at first, then resized at each close toward a hundred milliseconds of the handle's wall -- or as many as three census ticks hold for a sparse handle, between two readings of the thread's major faults (`getrusage(RUSAGE_THREAD)`, the counter the suite reads from `/proc/thread-self/stat`, held equal to it by a test; `zfile::thread_major_faults`), its CPU clock and the wall its operations spent inside their calls, and the first operation past its end closes it. A window counts only on one thread, under one gate and of one class, point reads and counts or scans, since a scan of a hundred touches three pages where a read touches one; the rest are dropped and counted, mixed or moved. Its sums go onto the handle's slot, on lines the handle alone writes (fifteen words, and the slot is three lines now): the class's operations and faults, and over the windows with no commit inside -- the state's generation and the live table's committed log length at the open against the close -- the operations' busy wall, the thread's CPU and the faults, since a window with a commit counts for the faults an operation and not for the price of a fault, which would carry the commit's fsync. The busy wall is the operations' own and not the handle's, which the design had: a handle idle between its calls would read its idleness as fault time, milliseconds a fault, and the test that holds it sleeps two milliseconds between its reads. The tick sums the slots once an interval while the census is on, differences the sums into the interval's `Census`, counts the threads whose windows closed, takes the group's refaults over the interval from the platform's counter (or the seam's rate), and prices a fault as the ratio of the commit-free sums since the census began, `(busy wall - CPU) / faults`, so that one preempted window prices nothing alone; the least CPU an operation took by class, in windows the raw form served and in windows the compact form served, is the hit cost and the compact cost, a running minimum on the slot. The seam supplies the faults a window reads (`ZInject::add_faults`) and a price of a fault (`set_fault_cost`, the busy wall then the window's CPU plus its faults at that price), so the arithmetic is held to the nanosecond with no bound: the price comes back as set from commit-free windows and from none with a commit, a scan-only handle makes scan windows with their faults and its hit cost, and a window of both classes, or one closed on another thread, counts for nothing. In FITS no window is opened and an operation pays a load and a branch at its start and at its end; under `Off` and `Always` one load and a branch, decided once at the handle's claim. A scan window carries the keys its scans emitted too, for the mean scan length the sampler prices a scan through the form by. Counted by callgrind as the gate was, the piece-1 probe against this one in one sitting, three runs each over twenty thousand uniform gets of a ten-thousand-key store under `Off` (`supdb-nocache`, the toggle on `supdb_bench::engines::supdb_get`): 16,564,501-16,570,598 instructions against 16,724,496-16,732,332, 828 a get against 836, eight a get, four of them in `leave` and four in the read's own frame, and the same binary that read 830 a get in the sitting before reads 828 in this one, so the spread between sittings is two a get and within one under half. The engine's counts, the tick's last costs and the selector's held costs are in `ZCounts` and the `zpage_census_*` and `zpage_sel_*` rows.

The sampler and the builds under the controller (the design's 2.6.1, 4.5 and 5) are what SUSPECT buys the form with. When the selector wants a reading of the partitions as they stand (`Decision::sample`) the segment work runs a sample job on a thread of its own at idle priority (`zfile::sample` over every live partition through its live mapping): thirty-two groups a partition formed at evenly spaced ranks with the real `fill_group`, each fill and each decode-plus-walk timed on the thread's clock, and the fills' touches' major faults with the wall they waited. The job's reading goes to the selector once, stamped with the generation it was taken at -- a digest of the live partitions with their cells and declines, which a merge, a promotion, an attach or a retire moves and a seal does not -- so a reading of other partitions is dropped and asked for again. From it the model's compact-form prediction (`model_z` over the form's bytes, the pages estimated from the counted groups' records with a fence entry a page and the filter, against the resident part; each rank's group is the last of a chain of three fills hinted by the one before, `zfile::SAMPLE_CHAIN`, since a fill with no predecessor builds a page well short of full, and the sample that counted one such group a rank estimated the form at the records' uncompressed count a page, 1.7x its file -- 626 MB for files of 360 at five million keys -- so the model's compact rate read 0.22-0.28 a read for a form that fit its budget whole and faulted 0.000-0.008 under the forced arm; the counts carry the estimate, `ZCounts::sample_w_z_bytes`, and two tests hold it to the build's pages and the store's files within a tenth), the cost of a read through the form (the hit's prior plus the decode a page at a quarter more for a cold page) and of a scan (the pages a scan of the census's mean length touches), the price of a fault where the sample faulted, and the build's price: the fills' CPU a record over the keys through `ski_rental_price`, or the attach's reads at the price of a fault where every partition's form file is already on disk, which `Auto` keeps dormant at open and attaches in place of a build when the form is wanted. A partition the sampler declines -- under the floor, a record in a block, no group formed -- is declined on its segment as the builder would decline it and left out of the coverage. Where the decision wants builds (`Decision::build`: building, or COMPACT unless the wasted-build budget suspended them) the segment work attaches every dormant form file in one turn and starts one build at a time as `Always` does, warming each after its install; where it does not, a build in flight is cancelled, as a merge cancels one whose target it takes. Each tick reports the builds collected since the last by fate -- installed, superseded by a landing that retired the target, failed -- and the coverage, the footprint bytes of the partitions with a cell over those of the partitions not declined; the selector installs at nine tenths, and the gate is stored after the cells as before. A retention that ends with the form unused (`Decision::retire`) takes every cell out through the epoch and gives the file a dead name for the idle sweep, as a page that failed a check did. The counts are in `ZCounts` (`samples`, `sampled`, `sample_declined`, `sample_us`, the selector's `sel_samples`, `sel_builds`, `sel_installs`, `sel_cancelled`, `sel_suspended`, `sel_exit_e1..3`, `sel_retired`, the rent and the price `sel_excess_ns` and `sel_c_build_ns`, and `retired`) and the rows of the same names; `SUPDB_ZPAGES_TRACE=1` prints a `zsample` line a reading. The read and count dispatch asked for `Always` before the gate until this piece: a store under `Auto` in COMPACT read every key raw, with the gate at the form and the cells installed, because every test of the gate had run under `Always` -- the first `Auto` store driven to COMPACT found it in its first run. The dispatch asks for either option that may serve the form now and the gate decides. E2's footprint and the trial (the design's 4.7, `docs/zpages-design.md`) are the exit for a workload that would fit the raw form: a skewed reader arriving while the form serves costs CPU and no faults, which E1 cannot see and E3 cannot either while the form pays. While the form serves a handle under the controller, the handle's memo keeps one bit a compact page it opens, by form file, and the distinct count (`ZMemo::track`, `touch` at `open_group`, which every point read, count and walk passes); at each window's close the count goes onto the slot (`zcpages`), a count and not a sum, cleared with the bitmap by the first operation under another gate, so the tick's sum over the slots is an upper bound on the union, up to the threads' count loose over one hot set. The bound in bytes is drawn from the store's own quantities and never a cell's: a compact page's keys hash to as many hash pages, capped at the index section, and its records lie over a page's share of the record region and one more. Under the raw gate the operation pays nothing for it: the switch runs at a window's abandon and otherwise only where the census runs. The selector takes the bound as `Input::footprint`, and the rest is its own: three ten-second measures with the form's faults under the suspect threshold and the bound under half the budget start the trial, the gate goes back to the census with the cells kept, the trial waits for the raw form's refill (the bound's pages at the price of a fault, or the faults themselves, or five intervals of low evidence) and three intervals judge it; a raw form that faults goes back to COMPACT and doubles the trial's back-off, one that does not leaves the form, kept for the retention. Priced by callgrind as the gate and the census were, the piece-3a probe against this one over twenty thousand gets of a ten-thousand-key store: through the form (`supdb-nocachezpages`, where every get opens a page and asks the flag) 17,626,944 instructions against 17,563,360, 881 a get against 878, and on the raw path (`supdb-nocache`) 16,747,068 against 16,687,068, 837 against 834 -- three fewer a get on both, because the census hooks had grown past what the compiler inlines and cost seventeen instructions of call and prologue for a flag that said no, so each is a flag inlined into the operation and the rest out of line now, and the page touch itself is a load and a branch on the memo's own flag, outside its cell: the first version asked through the cell's borrow, and read eighteen a get. The engine's tests hold the footprint's count and bound, the trial through the gate both ways, E3 for a form that stops paying, and a square wave of faulting and fitting reads to a switch a dwell; the trial's fill path is at this scale the wall's -- three times a page's worth of faults at a millisecond -- and the selector's own replays cover the fill. The tests through the seam price a fault at a millisecond rather than this host's forty microseconds, because an unoptimized decode costs sixty microseconds a page where an optimized one costs five, and at forty the form would not pay in the dev profile. The suite's arms for it are `supdb-autozpages` and `supdb-nocacheautozpages`, `supdb` and `supdb-nocache` with the form the controller's to choose and nothing else moved (`bench/DESIGN.md`): a pass asks nothing of the form under them, since what the controller builds and reads through is its decision and the `zpage_*` rows carry it, and only a form file out of service refuses a pass. In memory the controller is priced by the pairing against `supdb` (the design's 7.5, `pair.sh supdb supdb-autozpages 100000 --reps 12`, and at 300000, at `ed734a8`): at a hundred and three hundred thousand keys, twelve pairs a sitting and two sittings with the arms swapped, nothing is marked under Holm and no mark keeps its sign in the second sitting, and the arm's rows carry no build, sample, census window or install -- the selector's steps, one a census interval, and its S1 evaluations, one a second, are the whole of its work. What the arm did cost a read in memory was named by count and not by the pairing: behind the census hooks' flag every read under `Auto` paid the call, the window's copy out of its cell and the gate's load to be sent back under the raw gate, and the form dispatch's own out-of-line gate check beside them, 124 instructions of a point read's 830 by callgrind (`bench/examples/read_only.rs` under `valgrind --tool=callgrind --collect-atstart=no --toggle-collect='<supdb_bench::engines::Supdb as supdb_bench::engines::Engine>::get'`, `supdb` against `supdb-autozpages` at 100000), which the pairings at a hundred and three hundred thousand keys had read as 5-28% on the point-read mixes in three sittings of four and level in the fourth, a mix there being three to fifteen milliseconds and bimodal with the host. The hooks test the state they work on now, a window open on the handle or the gate away from raw, and the dispatch asks the gate inline: 12 instructions a read over `supdb`, the checks themselves, and the pairings at both rungs read level in both orders (`pair.sh supdb supdb-autozpages 100000 --passes load,read,scan,mixes --reps 12`, and at 300000), as the pairing at three million does. The controller is the engine's default on that count and those pairings (`ZPages::Auto`): the raw form alone is the arm now, `supdb-nozpages`, the pair against `supdb` that prices the controller, and in the rosters in the controller arm's place, so the in-core rows banked before the default moved carry the raw form under `supdb` and the controller under `supdb-autozpages`, and the rows since carry the controller under `supdb`. Under a bound (the design's 7.4, `SUPDB_ZPAGES_TRACE=1 pair.sh supdb-nocachezpages supdb-nocacheautozpages 5000000 --reps 3 --bound 536870912 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `d462107`: the forced arm against the controller's at five million keys under 512 MiB, both orders) the controller loads faster than the forced arm, which builds at every seal and merge, installs its form files in every rep, and its passes after the install read level with the forced arm's; the passes before it read raw, since the build starts by the ski rental after the first reads and lands tens of seconds after that. With the model's two inputs right (the populations, and the sampler's estimate chained; the same pairing with `--bound 671088640` at `9f24e20`) the rent is paid twenty-odd seconds after the first read, on the point reads' own measured saving, and the install lands thirty to forty-five seconds after that, inside the threaded scans; the mixes after it read level with the forced arm's in both orders, and the controller's loads read faster, since the forced arm builds at every seal; the rows taken before those inputs were right (the same pairing at `d462107` under 512 MiB and at `dbac367` under 640 MiB) read the rent paid at forty to sixty seconds on the scans' saving alone, with the compact rate predicted at a quarter of a fault a read for a form that fit. The entry priced against the rent is the eager one (`Options::zpages_build_eager`, off as shipped; `supdb-nocacheautozpageseager` is the arm): the first form bought at SUSPECT on the model's prediction once the sampler has priced the partitions and they have stood for `zselect::EAGER_SETTLE` with nothing of the segment work in flight (`zselect::Input::rewriting`: a merge, a promotion or a seal, whose landing would supersede the build), the rent and the measured half of the saving clause waived for that one purchase and every purchase after a cancel or an exit the rent's, since the grid of constant workloads looped under an entry that could buy again after every exit. Priced at five million keys under 640 MiB with both of the model's inputs right (`SUPDB_ZPAGES_TRACE=1 pair.sh supdb-nocacheautozpages supdb-nocacheautozpageseager 5000000 --reps 3 --bound 671088640 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `2e46588`), the eager arm buys at fourteen to sixteen seconds into every rep and installs at forty-four to seventy-three, where the rent buys at twenty-two to twenty-five and installs at fifty-seven to seventy-eight: a head start of eight seconds spent inside a build whose wall under the readers' I/O ranges thirty to fifty-seven in both arms, and every cell level within noise in both orders. The entry stays an arm, since it buys on no evidence and the rent now pays within ten seconds of it. The block cache beside the compact form under a bound (`supdb-cache64tofitautozpages`, the cache's merged forms over the hot blocks kept to fit 64 MB with the form the controller's; `pair.sh supdb-nocacheautozpages supdb-cache64tofitautozpages 5000000 --reps 3 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `3df2036`) loses every read cell to the controller's arm, the mixes at 0.2-0.65x: the block cache's walks stay on the raw form, so its scans drag the raw records through the page cache and evict the compact form the point reads need, 0.21 faults a read in the C mix against 0.001. A second form accompanies the records only if the other paths never touch them, and a cache whose walks touch them cannot sit beside it; the cache's forms are built from the compact form's records now and its walks go through it (`Options::zpages_block_cache`, above), and the shape priced here is the arm `supdb-cache64tofitautozpagesraw`. Priced the same way with the cache's reads through the form (`pair.sh A supdb-cache64tofitautozpages 5000000 --reps 3 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `642840f`, A each of `supdb-nocacheautozpages`, `lmdb-nordahead` and `supdb-cache64tofitautozpagesraw`): against the controller's arm without the cache the combined arm wins every mix, E 1.4x and 1.8x with its scans at 6-8 µs against 11-13, D a quarter, the rest within a sixth, and faults on none of them, with the passes before the install level, since the cache costs the raw stretch nothing; against the same cache reading raw it reads the mixes at 1.7-5.6x with the raw arm's 0.05-0.36 faults a read gone; and against LMDB with its readahead off E stands at 0.55x and 0.62x, still a loss, with A and F at 3-5x, B at 1.2-1.5x, and C and D about level inside LMDB's own swings. At five million keys the Zipfian head's blocks hold fewer than `CACHE_DENSE` unsealed keys apiece, so E walks sparse forms through the form file rather than copies -- 1.33 walks and 0.96 groups opened a scan against the nocache arm's 0.73 -- and the win is the merge of the unsealed keys folded into the deltas; what is left of E against LMDB is in that walk (a cursor held across a sparse walk's cuts, the dense threshold, the memo's size), unpriced. The block-cache arm's memory under a bound killed it in a shuffled load at first (`pair.sh supdb-nocacheautozpages supdb-cache64tofitautozpages 5000000 --reps 3 --bound 671088640 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `59dea0a`, killed in the shuffled load in both orders): the seal's and the landing's work for readers who were not there, fixed since (`CLAUDE.md`, "A structure maintained for a reader who was never coming"). (Priced before the sampler's estimate was right, the eager entry's builds were cancelled within seconds by the vanish rule, which held the reads' measured saving against a compact rate predicted at a quarter of a fault a read for a form that fit: `SUPDB_ZPAGES_TRACE=1 pair.sh supdb-nocacheautozpages supdb-nocacheautozpageseager 5000000 --reps 3 --bound 671088640 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `7f01c42`.)

The shift from the raw form to the compact one is brought to the load's end by seven changes, each priced against the shape before it in one process (`SUPDB_ZPAGES_TRACE=1 pair.sh A B 5000000 --reps 3 --bound 671088640 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `5971f16`, A B each of `...drop` and `...dropland50`, `...dropsampleidle` or `...dropnowritercensus`, and `...dropland50` and `...keepbehind`, `...noattacheager`, `...attachwarm`, `...dropland75` or `...nobuildsync`, the first and the last at 3000000 as well, and `...dropland50` and `...coldskip` at `78471c7`, every arm named `supdb-cache64tofitautozpages...`). The builds ahead start at the landings once the raw populations pass a percent of the budget (`Options::zpages_land_pct`, fifty as shipped; read at every tick, `Maint::z_land_look`), beside the seals in flight and never beside a merge, the oldest partition first by the sequence and the id it was made under, and only from records the page cache still holds at least half of by `mincore` (`LAND_RESIDENT`; a partition found colder is marked and left to the controller's build, `ZCounts::ahead_cold`). Below the selector's line to enter a build ahead leaves the raw form's cache as it found it: it drops no record behind it, since the raw form still serves, and writes its own file back and drops it two pieces at a time (`BuildPlan::drop_output`, `FileSink`), so a store that fits pays for the bet in its load and in nothing its reads touch -- a store of three million keys under 640 MiB, which fits, read at a twelfth of its rate after builds ahead that left their files cached and dropped its records, since the refaults read as the group's pressure and bought them. Past the line -- the model's raw faults a read at `zselect::ZSelect::theta_enter` -- the raw form cannot be held, and a build's reads leave the records it read the page cache's most recently used, so the kernel evicted the records of the partitions still to be built before them and the builder found every other one cold: past it a build ahead drops its records behind it and keeps its file, and the look that first finds the store past it turns every partition built under it the same way, its records dropped and its file asked back (`Options::zpages_land_drop_past`, `Maint::z_cross`, `ZCounts::ahead_swapped`), which took the files on disk at the sample from twelve to sixteen of nineteen to eighteen or nineteen. The line is the selector's and not the budget's: drawn at the budget, a 3M store that ended its load a few percent past a budget the process's own memory had shrunk, the model reading a few faults in a hundred reads, swapped out a quarter of its records and read its read pass ten times slower than the controller without the builds ahead. A file built ahead before the sampler's coding pick takes the options' coding and is given back and built again where the pick refuses it (`ZCounts::ahead_recoded`). The sampler runs at the CPU's normal class (`Options::zpages_sample_idle`, off): under `SCHED_IDLE` beside the read pass it took nine to sixteen seconds and read its wall less its CPU as six to seven milliseconds a fault where the readers' faults took forty microseconds, and the attach of files on disk was priced above their build; a reading with some files on disk prices their attach and the build of the rest. The writer's own reads are counted by the census through its engine slot while the gate is the census's (`Options::zpages_writer_census`, on), a writer operation that is not a read adding nothing to a window's busy wall, since a store read through its writer gave the rent no evidence and the suite's single-threaded passes read through the writer; under the form's gate they are not, since the exits read the handles' windows as before, and counted there too they cost the writer's mixes after the install a twentieth to a quarter in two sittings. Where the sampler finds form files on disk for the share of the keys whose attach installs the form (`zselect::COVERED_ENOUGH`) the purchase is mostly an attach, and it is bought on the model's prediction once the partitions have settled for `zselect::EAGER_SETTLE` (`Options::zpages_attach_eager`, on; `zselect::ZSelect::set_attach_eager`), once, every other clause of the buy standing and the model's predicted saving past the line where the rent would take the group's pressure for it: the rent's evidence, the group's refaults past half the faults over a trailing window, comes eight to nine seconds after a load past the budget, since the records the store dropped itself come back as faults the kernel does not count as refaults, and the group's refaults after a load are the load's as much as the store's. Paired against the rent alone the entry read the read pass 1.8x in both orders. And the build turn attaches every form file on disk before it starts a warm (`Options::zpages_attach_first`, on), where it had taken the first partition due a job and attached one file a warm: paired against that turn, the install comes 2.2-2.7 s after the load where it came at nine to eleven, and the read pass reads 1.8-2.1x. A file built ahead below the line, written back and dropped as it was built, is synced once named, on the builder's thread (`Options::zpages_build_sync`): `sync_file_range` asks for no flush of the device's write-back cache, so the next fsync anyone issued paid the flush of every such file, and at three million keys, where the builds end after the load's last commit, that was the first twenty-five commits of ycsb-B, 74-1,690 ms where they take ten (the mixes' commits timed by kind, `SUPDB_MIX_KINDS`); with the sync they take 10-13 and B reads 2.8x and 17x in the two orders. A file kept dirty past the line is left to the kernel's writeback, since its sync wrote its pages, 50-160 ms each at five million keys, on the thread racing the eviction front. Past the line a build ahead takes a partition whatever share of its records the page cache holds, one found cold before included (`Options::zpages_land_cold_past`): the controller builds the same partitions from the device after its buy, later and under the readers, and with the bar kept past the line three or four partitions went cold in about a third of the reps at five million keys, too few files stood on disk for the attach entry, and the install came at eleven to twenty-six seconds; with it every rep of eight installed at 2.2-3.7 s. Against LMDB with its readahead off (`lmdb-nordahead`), as its documentation configures a store larger than memory, at five million keys under 640 MiB, the shape with the cold partitions built reads E at 1.12x and 1.50x in the two orders, every pair ahead, the single-threaded read pass level with its p99 at 0.66x and 0.77x -- its first seconds read the raw form before the install -- and every other cell ahead by 2.5x to 12x, the install 2.2-4.5 s after the load in every rep (`SUPDB_ZPAGES_TRACE=1 pair.sh lmdb-nordahead supdb-cache64tofitautozpagesdropland50 5000000 --reps 3 --bound 671088640 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `020186f`; the `Auto` shape is `Options::zpages` at `Auto` with the 64 MB block cache kept to fit, not the default, which leaves the form off). Against LMDB as it opens by default (`lmdb`, its readahead on) the same pairing reads E at 1.59-1.61x and the read pass at 1.8-2.1x, which is LMDB's readahead and not the engine (`78471c7`). At five million keys under 640 MiB the install comes 2.2 s after the load's end in most reps where the controller without the builds ahead installs at forty to eighty, and paired against it in both orders the read pass reads 1.31-1.39x, its threaded passes 6.3-7.9x, the scan pass 7.9-8.8x and its threaded passes 5.3-5.4x and 2.5-3.7x, the ordered load 0.85-0.89x and the shuffled 0.99-1.04x, the mixes by position; at three million keys, which fits, the read and scan passes read level and the ordered load 0.83-0.91x. Seventy-five against fifty at five million read the load a tenth faster and the read pass at 0.56-0.60x. The arms are `supdb-cache64tofitautozpagesdropland{25,50,75,100}`, `...dropsampleidle`, `...dropnowritercensus`, `...dropland50keepbehind`, `...dropland50noattacheager`, `...dropland50attachwarm`, `...dropland50nobuildsync` and `...dropland50coldskip`. The `cache64tofit` arms pin their percent and the raw drop, zero and off unless their name says otherwise, since their pairs were priced against that shape; every other arm follows the engine, which ships fifty and the drop on. The suite's `Policy` once carried defaults of its own for both, off and zero, so the plain controller arm `supdb-autozpages` ran without either for as long as those defaults stood, and every reading of the controller at scale through it was of a shape the engine does not ship (`bench/CLAUDE.md`, a policy default that restated the engine's; `bench/scripts/policy_defaults.py` holds the two apart now).

The builds ahead wait while scans are near (`Options::zpages_land_scans_far`, on): from the store's first scan until a writer's seal finds two stores' worth of writes since the last one (`Shared::scans_far_at`), since the controller buys on its point reads' faults and a store that is scanned and not point-read gives it nothing to buy on. The lag sweep's store at ten million keys under 1.25 GiB is past the share of its budget the builds ahead start at and is never bought, and it built a form file at every landing of its bursts' merges, ninety-three through the sweep: the ten-percent pass read 580-680 MB and wrote 380-450 MB at the device, where with no builds ahead (`supdb-noland`) it read 64-105 and wrote nothing and ran in half the time, and LMDB's reads 70 MB (`SUPDB_ZPAGES_TRACE=1 SUPDB_MERGE_TRACE=1 SUPDB_LAG_MARK=1 bench ab --arms supdb,supdb-noland --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 1`; the controller's trace stamps its epoch on the lag marks' clock, `zepoch mono=`). The store the ordered load made builds its files in the load and the point-read pass after it, before any scan, and with no builds ahead the read and scan passes after the load ran at a fifth and a tenth, so the builds are gated rather than removed. Paired against the shape before the gate (`bench ab --arms supdb,supdb-landnearscans --size 10000000 --bound 1342177280 --ops-div 30 --reps 1`, at the commit that made the gate the default, and the `supdb-noland` pairing at `888348d`) in two sittings: the ten-percent pass 2.2x and 1.4x, the sweep's weighted sum 1.71x and 1.03x, the shuffled load 1.19x and 0.96x, the read and scan passes after the load level or ahead, and the mixes, half a second each at that rung, either way by sitting. The first version of the gate read nearness off a flag the writer's seal set, and the one-percent burst is too small to seal, so the flag kept the write-only load's reading and the builds ran on through the ten-percent burst; the count of scans is read instead, and a seal records only the count at which it found the store far.

The merge chain plans the next merge at a merge's landing while reads are around (`Options::merge_chains`), so it needs a merge in flight to go on, and where a burst's last merge landed before the pass's first scan nothing started another while the store was only read: the full-rewrite pass at ten million keys under 1.25 GiB, undivided, ran 76.6 s with 286 pieces at its end in one round and 31.6 s with six in the other (`SUPDB_LAG_COUNTERS=1 bench ab --arms supdb,supdb-chainwrites --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 1` at `5648ab5`). The segment work's loop plans a merge when none is in flight and a scan has come since its last look now (`Options::merge_plan_idle`, on; `supdb-noidleplan`), and paired undivided against the shape without it the sweep reads level to a seventh ahead, the stall being intermittent. The arm that chains through the bursts' writes as well (`Options::merge_chains_writes`) read ahead of the default by the suite's weighted sum in three sittings because its chain always had a merge in flight; on top of the idle plan it reads 1.06x of the default's time over the sweep undivided, the full rewrite 1.08x and the shuffled load at 0.83x of its rate, and stays off.

Against LMDB undivided at that head (`SUPDB_SCAN_OPS=1 SUPDB_LAG_COUNTERS=1 SUPDB_MERGE_TRACE=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 1` at `d5679e4`) the drained pass reads 2.7x, the one-percent pass 2.35x, the ten-percent pass 0.75x and the full rewrite 0.48x, the sweep's weighted sum 7.4x ours, since LMDB's full-rewrite burst runs 613 s against 23. LMDB reads every pass at about a hundred microseconds a scan and two major faults, with nothing else in the process at the device. The full rewrite's pass is its merges: twenty-four of them back to back, a partition and fourteen pieces each, 0.4-0.75 s apiece, the pieces from 342 to 20 over the pass's first thirteen seconds, while its first three tenths read 750, 425 and 254 us a scan and the rest 85-136. A merge's walks read every input's key again at every step, fifteen reads an output key where the two inputs a step advances need one each; they keep each input's key at its rank now and read an input again only when a step advances it (`Options::merge_walk_cached`; `supdb-mergewalkrescan` and `supdb-ingestmergewalkrescan` read every input at every step), and write the same files byte for byte, which `tests/fences.rs` holds through partition and piece merges. A merge's own account under the trace (`mergejob`: its wall, its thread's CPU and its major faults at the end of each pass) prices it in core (`bench/examples/merge_walk.rs`, a million keys in a partition and fourteen pieces): the key pass 147-239 ms against 320-337, the value pass 476 ms against 641-741 in the merges whose output pages came cheap. The value pass read 1.9 s in the other merges whatever the walk, with 54-64% of the job's samples in the kernel's copy into fresh page-cache pages against 7-9% in the fast ones; the probe's first merge of a round paid it and its second did not, which is the host's price for a fresh page and not the merge's work. Paired undivided at ten million keys under 1.25 GiB, a warmup and a pair (`bench ab --arms supdb,supdb-mergewalkrescan --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 1` under the trace): the shuffled load's closing drain, which merges the whole store, took 15.2 and 17.5 s against 29.5 and 31.5, the load 1.31x; the merges inside the full-rewrite pass summed 10.1 and 10.9 s against 12.2 and 14.2, the pass 18.8 and 19.1 s against 20.3 and 22.6 and its weighted cell 1.11x; the other lag points, where no merge runs inside the pass, moved either way by round. Under the bound a merge's open of its inputs was a third of its wall, 150-170 ms with some two hundred major faults against 55 ms of CPU: each input's key section verified again against its checksum row, a read of the whole section, which the store had verified at its own open or taken unverified at the landing of a segment it wrote. Skipping it under the store's trust was refuted (`Options::merge_verifies_inputs`, on; `supdb-mergenoverify` and `supdb-ingestmergenoverify` skip it): paired the same way, the merges inside the full-rewrite pass took their open's three to four thousand faults in their value pass instead, 2,600-2,800 there against none, and summed 10.4 and 12.1 s against 10.1 and 10.8. The key section holds the records whose values are inline, every record of the suite's shape, so the verification was the merge's one sequential read of its inputs, and the value pass after it ran from the cache -- the rule the drain's read-back gave, that a step reading what the next step reads warms the cache for it, met again at the merge.

What is left of the ten-percent pass against LMDB is not its faults. A record of every major fault over the pass (`perf record -e major-faults -c 1 -d -k CLOCK_MONOTONIC` around `bench ab --arms supdb,supdb ... --reps 0`, cut to the pass by its lag marks) put 193,520 of 193,748 on the scanning thread, in `Blob::scan_at`, the values' copy and `Reader::scan_seek`, 1.9 a scan to LMDB's 2.0. The process beside them held 520-580 MB of anonymous memory where LMDB's holds 65, the allocator 190 of it in use and 290-380 free and resident, so the page cache the scans had was 700-780 MB to LMDB's 1,190, and a fault read 6.7-7.5 KB from the device to LMDB's 3.9. The burst's allocations of 128 KB or more were 272 MB of transient buffers (an allocation shim's record by call site: the seal's writer and sort, the upkeep thread's settle and radix sorts, the scan snapshot's extension), 7 MB of them live at its end; a fixed mmap threshold maps them and returns them at their free (`Options::malloc_mmap_threshold`, refuted), and paired arm against arm it left the same 290 MB free at the point, the pass 18.7 s in both rounds against 14.3 and 15.5 with more faults, and the sweep's weighted sum 1.045x worse: the free bytes are small chunks, and the cache they cost does not decide the pass. Nor do the kernel's read-arounds: the segments advised `MADV_RANDOM` for the store's life (`ReadAdvice::Random`, the arm `supdb-advicerandom`), paired the same way, read 7.2-7.7 KB a fault against 6.8-8.0 and the pass in 15.9 and 15.7 s against 16.9 and 14.7, the full-rewrite pass faster in the pair and the one-percent pass slower, the weighted sum 0.96x, inside the spread: the bytes a fault reads are the spans the scans ask for (`Options::blocks_willneed`, `Options::pieces_willneed`), which the kernel reads whatever the advice. What the scanning thread spends over the pass is the kernel's: a cpu-clock profile cut to the pass and the thread put three quarters of its 8.8 s of CPU there, a quarter of it in the virtio doorbell (`iowrite16`) and a sixth in the switches back from its waits, and 1.5 s in the engine's own code.

Two merges in flight over disjoint ranges (`Options::merge_parallel` at two; `supdb-mergepar2`, `supdb-ingestmergepar2`, and `supdb-nozpagesmergepar2` beside `supdb-nozpages`) pay the full rewrite's merge debt sooner and leave the pass where it was, so the default runs one. Paired undivided at ten million keys under 1.25 GiB, two pairs a sitting (`SUPDB_SCAN_OPS=1 SUPDB_LAG_COUNTERS=1 SUPDB_LAG_MARK=1 SUPDB_MERGE_TRACE=1 bench ab --arms supdb,supdb-mergepar2 --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2` at `82ba73a`, and `--arms supdb-nozpages,supdb-nozpagesmergepar2` at `8e83964`), sixteen to twenty of the full-rewrite pass's twenty to twenty-four merges started beside another in every rep, and the last landed 4.8-6.5 s into the pass against 8.0-10.7, each merge a seventh to a third longer in wall at the same CPU and with a quarter fewer major faults. Without the compact form in either arm the pass's first tenth, where the merges run, read 6.3 and 6.6 s against 8.6 and 8.8, and the nine tenths after it read 1.0-1.8 s apiece by rep in either arm, so the pass read 16.6 and 20.8 s against 21.5 and 21.0 and its weighted cell 0.96x, unresolved. With the form the cell is the controller's: in one sitting the parallel arm bought its form during the ten-percent pass and the default during the full rewrite's, the passes 12.8 s against 17.4 (`SUPDB_ZPAGES_TRACE=1`, a pair), and in the other the default served two to three times the scans through form files and read 18.9 and 14.9 s against 18.5 and 17.2. The sweep's weighted sum read 0.99x and 1.02x, and the loads level.

ycsb-E at ten million keys is lost to the bound and not to the engine's layout. Run whole, the load and the six mixes with nothing divided (`SUPDB_MIX_KINDS=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --passes load,mixes --ops-div 1 --reps 2` at `0b544e2`), in core it reads E at 1.20x of LMDB and C, D, B, F and A at 1.47x, 1.42x, 3.2x, 6.3x and 7.8x; under 1.25 GiB (`--bound 1342177280`, the same pairing) it reads E at 0.53x, its scans 4.3 us of CPU an operation against LMDB's 2.3 and 0.040 major faults an operation against 0.016. Every one of those faults lands in the compact form's files, in `view_with` as a group's page is opened (`perf record -e major-faults -c 1 -d` over the pairing, the data addresses mapped to their files), and the memory they want went to the process: sampled from outside the bound's group every half second, E held 380-400 MB of anonymous memory and 855-875 MB of page cache where LMDB held 17 and 1,231. By the census after E (`SUPDB_MIX_CENSUS=1`) the anonymous memory is the block cache's forms (114,000 sparse forms and 6,400 copies, 150 MB of buffers over 99 MB of contents), the memtable the update mixes left unsealed (90 MB, the seal grown to the store over four times the trigger), the scan snapshot's copy of it (36 MB over 9 used) and 77 MB the allocator held free. Three levers on that memory were paired against the default the same way, two pairs each, at `254be3f`: the forms fitted to their contents (`Options::forms_to_fit`, `supdb-tofit`) held 38 MB less through E and read E 1.14x, A 1.25x, F 1.21x, D 1.07x and C 1.03x in both pairs, B level; the seal held at its floor out of core (`Options::seal_floor_out_of_core`, `supdb-sealfloorooc`) held 150 MB less and read E 0.70x and D and F 0.78x, the pieces it left costing the scans more than the memory freed; and the block cache reading the raw records where the form serves the point reads (`Options::zpages_block_cache` off, `supdb-cacheraw`) read E 0.67x with 3.6 times the faults, the raw records E walks not fitting beside the form. The forms by themselves are not the gap either: the raw-tail coding forced (`supdb-cache64tofitzpagesrecordrawdrop`), which walks in place with no decode and is 1.19 GB against LMDB's 1.33 GB file, read E at 0.35x of LMDB with five times its faults, since beside the process's 290 MB it had 980 MB of page cache where LMDB's file had 1,231; and the LZ4 form with the block cache held to 64 MB (`supdb-cache64tofit`) took fewer faults than LMDB, 0.011 an operation, and read E at 0.94x of the default, the decode and the rebuilds the small cache makes costing what the faults had. What is left of E against LMDB under the bound is the process's memory beside the form files and the CPU of reading through the compressed form, about half of the scanning thread's samples in the decode, the fence, the page view and the group's cursor.

The fit is taken where the store is short of memory and nowhere else (`Options::forms_fit_when_short`, `supdb-fitshort`), since in core at three hundred thousand keys it resolved nothing either way and a fitted form reallocates as it grows. The segment work asks once a second whether the partitions' raw bytes, the hash and record populations a point read touches, exceed the budget the compact form's controller is decided by -- the group's limit or the machine's less the anonymous memory and the margin, held under the machine's `MemAvailable` less the margin -- so a store that fits never fits its forms and runs the default's code. Paired against the default under 1.25 GiB in two sittings (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 bench ab --arms supdb,supdb-fitshort --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2` at `6658da4`, the second with `SUPDB_ZPAGES_TRACE=1`), it read E 1.11x and 1.31x, ahead in all four pairs, with the major faults an operation at a half and three fifths of the default's and the cache at the end 104 MB against 158; the point-read mixes read from 0.78x (C, the first sitting) to 1.11x (A, the second), each sitting's controller installing within a third of a second of the other arm's, so those cells are unresolved. Against `supdb-tofit` in one sitting (`--arms supdb-tofit,supdb-fitshort`), which fits from the open and runs the same code through C, it read C 1.09x, A 1.18x and E 0.97x: the spread over the point mixes is the sittings', and the option's own look costs nothing a pairing sees. The fit changes the forms a burst's pass builds, so it was priced on the lag sweep under the same bound with no compact form in either arm (`SUPDB_SCAN_OPS=1 SUPDB_LAG_COUNTERS=1 SUPDB_LAG_MARK=1 SUPDB_MERGE_TRACE=1 bench ab --arms supdb-nozpages,supdb-nozpagesfitshort --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2` at `6658da4`): sixteen trims in its arm against none, the anonymous memory at the passes lower in three readings of four (499 and 719 MB at the ten-percent pass against 763 and 610, 673 and 638 at the full rewrite against 790 and 827), the one-percent pass 1.2 and 1.9 s against 2.0 and 3.2, the ten-percent pass split by rep, the full rewrite level, and the sweep's weighted sum 0.89x of the time in both pairs, the loads level. So the option is on by default, the trim thread starting at the look that first finds the store short, so a store that is never short has none; `supdb-nofitshort` is the shape before it, and `supdb-nozpagesnofitshort` beside `supdb-nozpages` (the arms above were `supdb-fitshort` and `supdb-nozpagesfitshort` at `6658da4`, where the option was off).

The selector's fault model was then held to E as a calibration, registered before the sitting ran: fed each arm's own anonymous memory over E, does it predict E's major faults an operation? The prediction is the engine's own (`Maint::z_model`): `0.95 * model_z(B, f, W_z) * (50 p + 1)`, `B` the limit less the anonymous memory and the margin, `W_z`, `f` and `p` the picked coding's size, always-resident part and pages a key from the arm's last `zsample` line before E, 50 the suite's scan length and 0.95 E's share of scans. The rule registered was calibration where the prediction is within 1.5x of the measurement on five arms of seven and the arms rank in measured order, a zero prediction against a nonzero measurement counting as outside. Seven arms, one rep each (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 1` at `ef57433` over the pairs `supdb,supdb-nofitshort`, `supdb-tofit,supdb-sealfloorooc`, `supdb-cacheraw,supdb-cache64tofit` and `supdb-cache64tofitzpagesrecordrawdrop,supdb`, with `scripts/resident.py` over the store four seconds into each E): it failed, none of seven within the factor. The six arms on the block coding held 217-405 MiB anonymous, a `B` of 811-999 MiB against a form of 745 MiB, and were predicted no fault at all where they took 0.011-0.028 an operation; the page cache beside the form held the WAL, 99-102 MiB of frames E never reads (3 MiB in the seal-floor arm, whose seals rotate it), the level-0 pieces at 65-71 MiB and, in two arms, raw partitions, so the form stood 0-64 MiB short of whole where `B` gave it 66-254 MiB of room. The forced raw-tail arm, whose 1,134 MiB form against a `B` of 908 is the one the model calls short, was predicted 0.49 faults an operation and took 0.076: the residency was right, 80% of the form resident against 79% predicted, and E's Zipfian reads fell on the resident part, which a model of reads uniform over the form's pages cannot see. And the arm whose block cache reads the raw records (`supdb-cacheraw`), the most faulting at 0.167, has no prediction, since the model has no term for a scan of the raw form. So the model counts the store's own page cache as the form's and its reads as uniform, and as it stands it predicts nothing a split of the bound across the consumers could be decided by; nothing was built on it.

What the calibration's residency read did name is a tenant: the WAL, whose frames a replay at the open is the only reader of, held 99-102 MiB of E's page cache while the form stood short of whole. The log now drops its frames from the page cache once a sync has covered them, a megabyte at a time and never the page that holds its tail, which the next append writes into (`Options::wal_drop_bytes`). Paired under the bound in two sittings with the arms in both orders (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 bench ab --arms supdb,supdb-waldrop --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2` at `7389614`, then `--arms supdb-waldrop,supdb`), the log held 2 MiB of the cache four seconds into E against 101-102, the form was resident whole, E's major faults went from 0.027 and 0.034 an operation to none, and E read 1.53x and 1.22x, ahead in all four pairs; the six mixes took 10% and 3% less time in all. The write mixes' commits read 2-10% slower in both sittings (B's in every rep, A's and F's on the mean), which the drop's own cost does not cover: timed alone, a drop is 0.13 ms a megabyte of log, a few milliseconds a mix, and an append and its fdatasync take as long with the drops as without (`posix_fadvise` beside a 13 KB append and fdatasync, alternated). The reclaim it might have moved is not there to move: the kernel's memcg reclaim events over a pair (`vmscan:mm_vmscan_memcg_reclaim_begin` and `_end` through tracefs, its clock on CLOCK_MONOTONIC beside the phase marks) put reclaim in the loads, in C before the form installs and in E with the frames kept, and none at all in B, A, F or D in either arm. Under `strace -f -T` over B the drop's own calls were 13 for 2.4 ms in the whole mix and the commits' difference lay in their fdatasyncs, 563 ms against 517 at medians of 607 and 574 us, in the six reps of four sittings that have read B's commits slower with the drop; its cause is open, at 3-5% of the write mixes against E's 20-50%. On by default; `supdb-walkeep` keeps the frames, and was `supdb-waldrop` at `7389614`, where the option was off.

C, the first mix after the load, read 0.53x of LMDB under the same bound, and its time was the raw form's: the controller installed the compact form three to four seconds into C, and the trace said why (`SUPDB_ZPAGES_TRACE=1`). The builds ahead at landing keep up with an ordered load while it runs, a partition every tenth to quarter second, but the load's drain lands its last five partitions at its end, and with the two passed over as cold seven of twenty-nine waited for one builder after the load, at 0.25-0.6 s each beside C's reads; the install waits for nine tenths of the footprint, and the sample before it waited for the last build ahead, since the next one took the builder in the tick that freed it. Two builds ahead at once, the second in a slot of its own (`Options::zpages_ahead_builders`), and the sample asked for before the next build ahead (`Options::zpages_sample_between`) were paired against the default (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zahead2gap --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2` at `671ed1d`, then `--arms supdb-zahead2gap,supdb`): 24 of 28 files on disk at the load's end against 19-22, the install a second into C against three to four, C's major faults 0.035 an operation against 0.06-0.07, and C 1.46x and 1.39x, ahead in all four pairs, the other mixes and the loads within the sittings' spread. Taken apart (`--arms supdb-zahead2,supdb-zahead2gap`), the sample's order added nothing beside the second builder, C 0.97x with the install at the same moment, since two builders finish the builds ahead before the order matters. The second builder is the default; `supdb-zahead1` is the one builder, `supdb-zsamplegap` the sample's order on top of the default, and at `671ed1d`, where both were off, the arms were `supdb-zahead2`, `supdb-zsamplegap` and `supdb-zahead2gap`.

With two builders the install still came a second or more into C, and what held it was taken apart one pairing at a time, the trace of each naming the next. The controller suspected on the second of two hot model readings a second apart, so the crossing at the drain's landing was one to two seconds old when SUSPECT came; the look the builds at landing take at every tick reads the model now at the tick its raw prediction first reaches the line, and one reading suspects (`Options::zpages_suspect_at_crossing`). That alone read C level: the sample waited for both builders' slots to be empty at once, 1.3 s after the drain, and a sample started inside the drain priced a store a partition short, whose raw-tail pick the builds ahead between it and the next sample followed and that sample refused. A partition whose raw records are resident, or whose build ahead is in flight, counts as covered now, for the attach, its price and the install (`Options::zpages_cover_resident`), and the sample waits for no build ahead: a build ahead past the line drops its records behind it, so a partition resident when its build started was not by the reading's end, and counted uncovered it refused the attach and was never sampled again once the files had landed, the store reading the whole of C and B raw in one rep. Two holds more were the builder's: the buy's attach of the files on disk waited for the builder's slot while a build ahead held it, and in key order the warms of the files attached took the slot for three seconds while the drain's last two partitions waited for their builds, into B, which read 0.85x in both reps until a build went before any warm. And the eager attach waited out the two-second settle after the drain; a step whose census saw reads with no segment work in flight counts as settled too (`zselect::ZSelect::set_attach_on_reads`), since a load does not read. Paired at `3811909` (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb-zcrossing,supdb --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2`, then `--arms supdb,supdb-zcrossing`), C read 1.36x and 1.32x, ahead in all four pairs, its major faults a third, the install 0.5-0.8 s into C against 0.8-1.4; the other mixes read 0.87-1.01x in the first sitting and 0.99-1.21x in the second, by the order, and the loads level. Flipped, the default met stores the pairings never made: a store whose records all fit has every partition resident, and counted covered with no form there it installed with no form file attached, its reads all raw and the census in COMPACT reading them as the form's, so the stand-ins count only beside a form at least half there, on disk for the sample and attached for the install (`stands_in`), which the drain's case, 24 to 26 files of 29 on disk, is well past. Both are the default; `supdb-zinstallwaits` is the shape before them, and at `3811909`, where they were off, the arms were `supdb-zcrossing` and `supdb-zcoverres` (resident and in-flight partitions covered, suspected on the ticks). The sample's order between builds ahead (`zpages_sample_between`), on in the arm, changes nothing beside them, since the sample waits for no build ahead.

The row divides its read passes by ten out of core (`--ops-div 10`, `bench/DESIGN.md`), so the cell it measures is a C of 167,000 reads, and against LMDB in that shape the install's lateness was most of it: C read 0.68x (150,000 reads a second against 220,000; `SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 10 --reps 2` at `8f5aaf2`), 39,000 of its reads served raw before the install at seven times a compact read's cost, while E read 1.12x at the median (0.67 and 0.86 s against 0.75) and D 1.05x. The selector's trace, stamped with the store's clock and printed at every SUSPECT step (`zselect t=`), put the crossing at the drain's landing, which is C's start; the fill sample's thread ran for the 0.4 s after it -- every partition filled from thirty-two ranks three times in each of two codings, for a form whose bytes were on disk, 25 to 26 files of 29 -- and the reading it gave stood unbought for the 0.5 s after that, since the buy on the prediction needs the model's compact prediction, the model has none until it has the form's size, and the model is recomputed at its period (`zselect::S1_PERIOD`, a second), so the reading waited for the period's next reading. Two changes. A reading's landing resets the period, so the model is recomputed with the form's size at the next step and the reading is bought a step after it stands (`Maint::z_reading`, one arithmetic for a sample's sums and a reading from the files; a fix, not an arm). And where the files on disk, with the partitions that stand in for theirs, cover the keys the install needs, the reading is made from the files on the segment work's thread in the tick that wants it (`Options::zpages_reading_from_files`, on; `supdb-zsamplefills` is the fill sample for every reading): each file's length, pages and resident part from its trailer (`zfile::file_facts`), ten pages of four of them decoded and walked for the cost of a decode (`zfile::time_file_decodes`), the fill's price a record from the builds' recorded CPU over the bytes they decoded, and a partition without a file priced at the ratio of those with one. Paired at `016ee12` (`--arms supdb,supdb-zsamplefills --ops-div 10 --reps 2`, the other options as above, then `--arms supdb-zsamplefills,supdb`): the install 0.18 and 0.41 s after the crossing against 0.65 and 0.79 -- the fill sample's arm installs sooner than at `8f5aaf2` too, by the period's reset -- and C 1.40x in the first sitting (607 and 793 ms against 852 and 1,027, its major faults 0.036 an operation against 0.077) and 1.94x in the second (484 and 575 against 940 and 1,004); B, D and F read 8-24% in the fill arm's favour at both sittings' medians, each on one fast rep of that arm against two level reps of the default (B 319 ms against 443 in its other rep, D 533 against 1,072, F 509 against 638), and nothing in the two arms differs past C's install, so the direction is noted and unresolved. Against LMDB (`--arms supdb,lmdb-nordahead --ops-div 10 --reps 2` at `016ee12`) C reads 1.43x (292,000 reads a second against 205,000; 570 and 641 ms against 813 and 823, its major faults 0.024 an operation against LMDB's 0.084), E 1.10x (241,000 against 219,000) and D 0.93x (287,000 against 310,000), the last two within the sittings' spread, as at `8f5aaf2` they read 1.12x and 1.05x. What the reading costs is the trailers' pages and forty decodes, under a millisecond on files just written; what is left of C's raw stretch is the census's: the buy waits for a step whose census has seen sixty-four reads, which lands two or three steps after C's first read.

The row's own shape then said what the pairings' had not. The row runs the read pass, its threaded pass, the scan pass and its threaded pass before the mixes, and those passes warm LMDB's B-tree: in the banked row at `bf5f130` (`bench/runs/ooc/20261106T172437Z-bf5f130.json`, the roster under 1.25 GiB) LMDB at ten million keys faults on half a percent of its reads and runs at 1.18 million a second in C and 384,000 in E, near its rate in core, where the pairings above, which ran the load and the mixes alone, met it cold at a fifth of that. Against the warm LMDB the row's cells at ten million are C 0.74x, E 0.60x and the scrambled E 0.86x, with B 1.65x, D 1.49x, the scan pass 1.32x and the loads and the write mixes ahead by 1.4x to 14x; our rates in those cells rose 1.4x and 1.8x against the row before (`5bf1f88`), and LMDB's 2.5x with its faults halved. Reproduced a rep at a time in the row's pass order (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 1` at `bf5f130`): C 0.88x, E 0.47x, D 0.98x, the single-threaded scan pass 0.68x with the threaded passes 1.3-1.5x ours, LMDB at 0.002-0.006 faults an operation and ours at none; without the threaded passes before the mixes (`--passes load,read,scan,mixes`) LMDB stays colder and reads 0.55x of ours in C and 0.82x in E. So the cells lost at 1.25 GiB are a race of instructions against a B-tree in core, and ours are the compact form's decode: a point read decodes one group of two kilobytes to answer, a scan of a hundred keys decodes the groups it crosses, and E's scans run at 4.5 µs against LMDB's 2.15 where the profile of our thread puts a quarter of it in the decode. The decode-free coding does not help at this bound: the raw-tail form, forced against the block coding in the same shape (`--arms supdb-cache64tofitzpages,supdb-cache64tofitzpagesrecordraw`, the rest as above), read C 1.05x -- the decode is not the most of a point read -- and E 0.50x with 0.075 major faults a scan, since its files are 1.52x the block coding's and E's own memory takes a fifth of the budget from the page cache, as the re-code policy found above; and the raw form without the compact one reads C at 0.12x with 0.15 faults a read (`supdb-nozpages` in the row), since our raw store with its hash slots is 1.6 GB where LMDB's file is 1.3 and nearly fits. The form's regime begins where LMDB leaves core too: under 1 GiB in the row's shape (`--bound 1073741824`, the rest as above) LMDB faults on 6% of C's reads and 10% of E's and ours fault on none, and every cell is ours -- C 3.1x, E 1.57x, D 4.5x, B 3.3x, the scan pass 4.4x single-threaded and 4.9-5.3x threaded, the loads 1.4x -- with no pass killed, where the engine before the mapped blocks (`Options::blocks_mapped`) was killed under that bound. The roster's bound of 1.25 GiB was chosen so that both engines read from the device at ten million keys, and the row says LMDB mostly does not there once the read passes have warmed it; whether the roster's bound moves to the gigabyte, where the design's regime holds, is a decision for `bench/DESIGN.md`, and the 1.25 GiB cells stand as what they are in the meantime.
The lever at that bound is the raw form itself, for the part of the store that is hot. In core the compact form answers a point read at 2.6 times the raw form's time and a scan of fifty at 2.5 (`bench ab --arms supdb,supdb-cache64tofitzpages --size 3000000 --passes load,mixes --reps 2` at `49a7551`, every file resident in both arms: C 0.38x, E 0.39x), the decode a quarter of it and the rest the fence, the directory and the walk of the group; and the suite's Zipfian is not scrambled, so at ten million keys four fifths of its point reads, and of E's scan starts, fall in the first partition of twenty-nine, 53 MB raw against 27 in the form, and a tenth more in the next three. The budget has room for a few partitions raw: through the mixes the process holds 100-200 MB of anonymous memory beside the form's 782, so 200-300 MB of the 1.25 GiB is page cache the form does not need. The hot-raw policy (`Options::zpages_hot_raw`) spends it on the hot partitions' raw form. At every tick with the form installed it reads each partition's window -- a served form file's decodes and opens, a raw partition's own reads (sampled one in sixteen on the segment's word, `Seg::hot_read`), each priced at the store's mean decode for a heat that rises at once and decays over a second, kept by the partition's lower fence so a merge's output inherits it -- and the served partition the window read most, a key of it against a key of the median partition, is hot past the re-code policy's floor when it stands over the median by half (`HOT_RAW_SKEW`) with at least 128 sampled events behind it, so a uniform pass, under which every partition reads at the median, releases nothing, and a thin window at the install cannot. The release is two looks: the raw pages -- the index section from the hash slots through the records, and the ordered index's heads -- are asked of the kernel at one, and at a later one, once nine tenths of the records are resident, the form file is taken out of its cell and retired through the epoch with its pages dropped, the file left on disk; a take at the ask would have sent the hot reads to pages the device had not delivered. The form files and the raw partitions together stay within three quarters of the budget for a release (`HOT_RAW_FILL`), four or five partitions at this rung, and past the fill share the coding pick already leaves to the page cache's other tenants (`zselect::FORM_FILL`) the coldest raw partition's file is attached again and its raw pages dropped, the gap between the lines the budget's fall through a write mix; a served partition twice as hot as the coldest raw one takes its room the same way; and the refaults decide nothing, as the re-code policy found. Three readings of the heat were wrong before this one (`CLAUDE.md`, the policy that took its gate from the policy beside it), and the first version waited for the builder as the re-code policy does, which never looked through the mixes at this rung. Paired in the row's pass order at `b23ee73` (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-znohotraw --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 2`), the policy releases the first three or four partitions inside C and B, a look apart, and E reads 1.34x and 1.38x of the arm without it in two sittings, with the point-read mixes within the host's spread -- C and D swapped sign between sittings while E held, and F read 1.08-1.22x -- and E's time with its hot partitions raw is the block cache's work over the 95,000 keys the write mixes leave unsealed in the hot range, 46,000 blocks built and 8,300 minor faults in a divided E, beside the seventh of its scans that start past the raw partitions and decode 153,000 groups. Against LMDB in the same shape (`--arms supdb,lmdb-nordahead`, the rest as above): E 0.82x from 0.47x, C 0.77x, where C is 170 ms long and its hot partition is raw only from its last look, B 1.73x, D 1.19x, A 5.2x, F 4.2x, the loads 1.35x and 13x, and the uniform scan passes 0.67-0.88x, which the policy does not touch. What is left of C and the scan passes is the compact form's speed in core, and what is left of E is the overlay's.
That speed was counted next, in core at three hundred thousand keys where every file is resident, with the suite's shape of read and scan alone (`bench/examples/read_only.rs`, `bench/examples/scan_only.rs`, each under `valgrind --tool=callgrind --collect-atstart=no --toggle-collect` on the harness's `get` or `range`, the forced form `supdb-cache64tofitzpages` against the raw `supdb`). A compact point read is 6,000 instructions against the raw form's 950, half of them the LZ4 decode of a group of two kilobytes, the rest the seek through the group's records (530), the dispatch (360), the fence's word tree (210), the group's open (180) and the page's view (130); a compact scan of a hundred keys was 82,000 against the raw walk's 13,500, and only 30,000 of them the decode. The decoded group holds the raw record images end to end, and the walk read each through `parse_record` and `read_exts`, the general path for any shape of record, at five hundred instructions a key where the raw walk's loop for the common shape -- one inline fixed run, the shipped writer's record -- reads the record in place for a hundred and thirty-five. The group walk and the merge cursor's walk take the same in-place read now (`flatindex::compact_fixed_at`, one definition beside the raw walk's), every other shape still through the parse: the compact scan is 67,000 instructions, 44% of it the decode and 9% the walk, and the probe reads it 1.1-1.25x faster at that rung with the raw walk 3.7x faster still. The decode is two instructions a byte on the suite's records -- short matches, a sequence every ten bytes -- and is what a compact scan costs in core from here; a raw-tailed group avoids it at half again the bytes, which the bound at this rung does not hold. The row's cells at ten million did not move with it (`--arms supdb,lmdb-nordahead`, the row's pass order as above, two reps: the scan pass 0.87x against 0.88x the sitting before, E 0.79x against 0.82x): the same 67,000 instructions take nine microseconds a scan there and six at three hundred thousand, so at the bound the compact scan is the memory's -- the form's pages, the fence's levels, the memo's slots -- and a cut in its instructions is read in core, where it is one.
Two of E's symbols at that point were instruments and not work, and one of them the profile's own. The census window stamped the clock at every operation's start and end for the busy wall its fault price is drawn from, and the profile put 73 of the thread's 962 samples in `clock_gettime` against LMDB's 11; it stamps one operation in sixteen now where a window is a thousand operations long (`Options::zpages_census_sample`; `supdb-zbusyevery` stamps every one), and paired in the row's shape at `da4e7a1` (`bench ab --arms supdb,supdb-zbusyevery --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 2`) E reads 0.98x of the arm that stamps every operation in both reps, the count's direction under the pairing's resolution. The profile taken again without the harness's counter variables, which turn the scan timers on for the supdb arm alone, put the thread's samples in `clock_gettime` at one of 973 where there had been 73 of 962: the instrument the profile ran under was most of what it had read, and the stamps the rest (`CLAUDE.md`, an instrument of the profile read as the engine's). The block walk's software prefetch of the records (`Options::scan_prefetch`) was 8% of the thread's samples, the stall of issuing sixty-four lines at once; paired against the arm without it in the same shape (`supdb-noprefetch`, at `124e162`) every mix read slower without it, 0.87-0.95x, and the scan pass 0.92x -- but the hot-raw release landed at different moments in the two arms (`zpage_rawserves` 396,000 against 156,000), which moves the point-read mixes the prefetch never touches, so that pairing names nothing; paired in core at ten million keys, where the store is raw throughout and the controller buys nothing (`bench ab --arms supdb,supdb-noprefetch --size 10000000 --bound 8589934592 --passes load,mixes --ops-div 10 --reps 2` at `da4e7a1`), E reads 1.23x with the prefetch in both reps and the point-read mixes within a twentieth, so the stall the profile shows is the walk's own wait moved ahead of it, and the prefetch stands.
The count under the profile named the next lever. The form dispatch (`part_scan_z`) and the block cursors' placements ran 66,000 times for E's 26,000 scans into covered partitions, 2.5 a scan, and the first reading of it was the block boundaries': the block walk's clean run took a block as clean by the form in its slot, and the cold partitions' blocks had none, so a scan of fifty built one block, walked it, and dispatched again at the next. The run builds the blocks it reaches now and takes the clean ones (`Options::scan_run_builds`; `supdb-norunbuilds` is the run that stops at an unbuilt block), and the promotion after a walk credits each block of a run on its own ranks, which the first shape had not: paired in the row's shape at `da4e7a1` (`bench ab --arms supdb,supdb-norunbuilds`, the rest as above), E reads 0.96x for the arm without it in both reps and 3.5% in a second sitting with the counters on, under the resolution, with the clean walks a quarter fewer and the dispatch count unmoved (`zpage_scans` 66,228 against 65,946 in E) -- so the boundaries were not the dispatches' cause.
The dispatches were the sparse blocks': each block's walk and each block's build over a covered partition placed a cursor of its own (`Options::zpages_block_cursor`), and two in three of a covered scan's blocks hold an unsealed key, the distinct keys the write mixes leave being spread flatter than the draws that made them. One cursor is held across the blocks a scan walks in a partition now (`Options::zpages_scan_cursor`; `supdb-blockcursors` places one a block), the clean runs and the sparse walks through it, and the builds keep their own, since a build leaves the cursor at its keys' cuts, behind which the walk starts, and placed afresh there it decoded the group it held again (E 4% slower, groups opened up 6%, in the shape that lent it). Paired in the row's shape at `da4e7a1` (`bench ab --arms supdb,supdb-blockcursors`, the rest as above): E's dispatches 48,800 against 59,800 and its groups 171,000 against 178,000 in one rep with the counters on, E 1.06x there and 1.00x over a sitting of two whose point-read mixes and scan pass, which the cursor never touches, moved a tenth and more; the count stands and the rate is unresolved. What is left of a covered scan is about five groups decoded where it spans 2.7, the builds' among them, and a covered scan costs about five times a raw one; the covered share is the Zipfian's tail past the partitions the hot-raw policy's room holds raw, and the room is the budget's fall through the write mixes -- the memtable's arena 35 MB, the allocator's retention 16, and in E itself the block cache's forms and snapshots 50 more (`SUPDB_MIX_CENSUS=1`).
The covered scans were the loss, and the room the policy left idle was the first lever on them. At E's start the group's `memory.stat` held 845 MB of page cache and 121 MB of anon under the 1,342 MB limit (`scripts/resident.py` over the store beside `memory.stat` at the mixes' phase marks): the form files 703 MB resident, the three raw partitions 79, the ordered indexes 25, the WAL 30; the hot-raw policy's ask line, three quarters of a budget that had already subtracted the anon and a margin, read full at three releases with a quarter of the limit unused. The release is asked into the room the group measures now (`Options::zpages_hot_raw_slack`: the limit less its anon and cache, the margin and a tenth of the budget for the write mixes' growth), and the skew it rates a candidate on is the last eight looks' sampled events (`HOT_RAW_WINDOW`), since the Zipfian's fourth to eighth partitions read 40-80 a look under the point-read mixes against the floor of 128 and the room alone asked for a fourth partition only at E's own start; paired in the row's shape at `96c35b6` (`bench ab --arms supdb,supdb-zhotline --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 2`) it released six partitions a rep against three, E 1.05x in both reps (304,000 against 288,000 operations a second), 1.13x in a sitting with the counters on and 1.09x in a second sitting of two (328,000 against 300,000), D 1.06-1.07x; a release's 42 MB device read and 22 MB form drop land on the mix they fall in, and the second sitting put three asks and three releases inside A in both reps, where the line arm had none, and A's 833 durable commits ran 0.59-0.60 s against 0.48-0.54: the readahead's requests ahead of the commits' fdatasyncs at a device whose scheduler serves reads first, the device-side cost of a release that the first sitting had put on B (0.75x) and the counters run on nothing (A 1.04x, B 0.96x, their commits level). The pages a release brings in cost the first mix that reads them a fault in sixteen pages, the kernel's fault-around, which a `MADV_POPULATE_READ` at the release was built to save and the test's own count refuted, seven faults over ninety-one pages without it.
What the covered scan paid beside its decodes was the cursor's own bookkeeping, a view of the page built at every group it opened and twice more where it was placed by rank -- the header parsed and checked, the directory's keys walked, 28 ms of E's 591 and the header line's first touch 21 more -- and the memo asked only after. The cursor holds the view across the page's groups and asks the memo before any view now (`Options::zpages_cursor_view`): paired in the row's shape at `96c35b6` (`bench ab --arms supdb,supdb-zviewperopen`, the rest as above) the views over the row fell from 117,400 to 63,800 and E read 1.07x in both reps (340,000 against 317,000). The decodes were half the covered scan, seven groups of 1.6 KB at 0.74 µs each, a quarter of the decoder's warm rate, and the profile's source lines put 98 of the decoder's 171 samples on its copy of the compressed input: the input's lines, cold at every group. The cursor asks for the next group's lines at each open now, the next group of the page by its directory or the first kilobyte of the next entry's page (`Options::zpages_group_ahead`): paired at `96c35b6` (`bench ab --arms supdb,supdb-znogroupahead`, the rest as above) E read 1.21x in both reps (360,000 against 298,000) where the arm's policy had released one partition more than the default's. The probe that had read a prefetch of the whole page's lines as a tenth of a decode (`bench/examples/zlines.rs`) cycled hot pages, whose lines the hardware's prefetcher was already streaming; in the walk the group after the one being parsed is the next thing read, nothing streams it, and the dozen prefetch instructions a group buy its lines a group's parse ahead. The ask's readahead under the idle I/O class, the segment work's thread lowered for the call and given back (`Options::zpages_hot_ask_idle`, `with_idle_io`), was built for the second sitting's A -- three asks inside it, 128 MB of reads ahead of its 833 fdatasyncs at a device whose scheduler serves reads first -- and paired level at `96c35b6` (`bench ab --arms supdb,supdb-zhotaskidle`, the rest as above: E 1.05x in both reps with the idle class, D 0.95x in both, A split), so the ask keeps the thread's class and the idle class is the arm.
The profile of E at `9ae6781` put 8.7% of the thread in the block walk's prefetch of the next copy block, with fifty-nine of its seventy-three samples on the prefetch instructions, which read as forty-eight lines asked for in one burst outrunning the core's fill buffers. Paced through the walk of the block before, four lines of each of the next block's three buffers every sixteen entries and the rest at the walk's end (`Options::scan_prefetch_paced`, `Pace`), the walk read E 0.90x in both reps (`bench ab --arms supdb,supdb-pacedprefetch`, the rest as above; 296,000 against 330,000 operations a second, the paced arm's policy releasing one partition more), so the burst stands and the pace is the arm: the samples were the lines' arrival and not the issuing, and the burst at a block's start has the walk of the whole block as its lead. Beside it two shapes the same profile named: the sparse form's search within a cut's run, which walked the run by key where E's inserts past the last partition's end share one cut, is a binary search with a compare for a key above the run; and the retired form files' sweep at an operation's end, a walk of the reader table's slots at every operation while a pin held the files, is under the forms' retry guard.

At `793f4aa` E's remaining gap was taken apart by a profile with call graphs (`perf record -a -e cpu-clock -F 999 --call-graph dwarf,16384` over the E window, `prof-e-row3.sh`'s shape) and a run with the counters, the events and the kinds on (`SUPDB_MIX_COUNTERS=1 SUPDB_MIX_EVENTS=1 SUPDB_MIX_KINDS=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 1`): the harness's sink copies 3% of the thread, the scan path's own block builds 10-12% (15,396 sparse, 29,538 clean, 619 copies over 158,000 scans), the covered path's group opens 15.6% with the decode 11.2% (144,349 decodes for about 17,000 covered scans, the memo answering 7%), E's commits 71-93 ms of its 590-630 ms mostly the durable fdatasync behind the device, and the per-operation sweeps 3%. Six options came of it, each paired in the row's shape (`bench ab --arms supdb,<arm> --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 2`, two sittings): the seeded start seek (`supdb-zseekrank` the shape before; E 1.20x in one rep, the scan pass 0.82x in both until the drained store's one-walk shortcut was given the seeded cursor, then 1.25x in both and 0.87x in one rep of a clean third sitting, E 1.20x, 0.91x and 0.89x by sitting, the dispatches one a scan and the memo's hits down by the second placements'; on, the count the verdict, and off after a fourth sitting in the row's whole order read E 0.97x for it -- below); the remembered pin (`supdb-sweepwalks`; 27,000-50,000 shortcuts a sitting against none, E 0.97x and 1.10x for the walks, the count the verdict); the walk's own decodes (`supdb-zwalkowns`; 31 million owned, the scan pass 1.02-1.05x in three reps of four, E 0.98x and 1.05x, unresolved and off); the values prefetched whole (`supdb-copyprefetchwhole`; E 0.95x and 1.23x, unresolved and off); the growth share at five percent (`supdb-zhotgrowth5`; twelve releases against twelve, level); and the skew bar at the median where the candidate fits the room (`supdb-zhotskewroom`; eighteen releases a rep against six, E 0.72x in both sittings with 0.037 major faults an operation against 0.0014, C 1.23x and 2.0x, refuted and off). The hot-raw policy's trace under the counted run showed the sixth partition held cold at 1.09x the median with 55-115 MB of room unused, and the releases a rep -- five, six, seven, eight across the sittings -- moving E's covered share with them.

The pairings until then stopped at the unscrambled mixes, and the row runs the scrambled ones after them (`ycsbs-*`, YCSB's scrambled Zipfian, `bench/DESIGN.md`); the row banked at `8028e78` read the scrambled E at ten million keys under 1.25 GiB at 0.59x of LMDB, 63,000-75,000 operations a second with 0.06 major faults an operation, where the row at `bf5f130` had read 0.86x with none. Paired through the row's whole order at `793f4aa` (`bench ab --arms supdb,supdb-zhotline --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --ops-div 10 --reps 2`), the room against the line read E 1.10x (250,000 against 227,000) and the scrambled E 0.76x (58,000 against 76,000), with 0.077 major faults an operation against 0.021 and sixteen releases against four. The seed was paired in the same order (`--arms supdb,supdb-zseekrank` at `793f4aa`): E 0.97x, the scan pass level, so the seed went off. A way back for a raw partition read under the median partition's reads a key (`Options::zpages_hot_cold_back`) was built against the room's retention, and its trace showed a partition released, sent back and released again inside E: a raw partition's scan counted one read and the same scan over a form file the seven groups it opened, so the policy read a raw partition under scans as a seventh as hot as it was. Its scans count the groups a form walk would open now (`Seg::hot_scan`), a fix in both arms. Paired so on `6ae0570`'s tree while the way back was the default (`bench ab --arms supdb,supdb-zhotstay` in the same order, two sittings, which at `6ae0570` is `--arms supdb-zhotcoldback,supdb`), the way back made fifteen to nineteen moves a sitting and read E 0.73x (179,000 against 247,000) in one sitting and level (244,000 against 238,000) in the next, the scrambled E 1.42x and 1.12x; the scrambled E's faults sat at 0.06-0.08 an operation in both arms of both sittings, and at 0.009 for the line arm in a third pairing on the same tree (`--arms supdb,supdb-zhotline`, the way back on in both). So the way back is the arm, and what keeps the scrambled E's faults up is the room's releases, not their staying raw.

The faults were the way back's own. Paired in the same order with the census and the events on (`SUPDB_MIX_CENSUS=1 SUPDB_MIX_EVENTS=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zhotline --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --ops-div 10 --reps 1` at `170042b`), every mix with a major fault had a back move in it and every mix with releases alone had none: two backs in the scrambled E in each arm, as its block cache took the budget from 1,020 to 962 MB and the form files and the raw partitions passed the fill share, and a swap's back in the scrambled D. A release asks for the raw pages at one look and flips at a later one, once nine tenths of them are resident; a back attached the form file at the look that decided it, and a form file is advised random from its attach, so its pages came in under the scans a 4 KB fault at a time -- 11,000 faults in a divided E of 166,000 operations, two form files' pages. The way back takes the release's two steps now (`Options::zpages_hot_back_ask`): the form file's pages asked of the kernel at the look that decides the move, under the ask's I/O class, while the raw form serves, and the file attached at a later look once nine tenths of its pages are resident or the two-second wait is out (`hot_back_waited` counts the second). Every ask in the pairings below was taken at the look after it with its pages in. Paired at `05008c0` (`bench ab --arms supdb,supdb-zbacknow`, the same passes, two reps), the scrambled E read 1.72x (96,600 against 56,100 operations a second) with no major faults against 0.084 an operation, in both reps; in a second sitting through the row's whole order, the lag sweep after the mixes (`--arms supdb-zbacknow,supdb ... --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --ops-div 10 --reps 2`), 1.21x (93,000 against 76,700) with none against 0.069, in both reps. The scrambled D, where a swap's back falls in some reps, read 1.13x and 0.94x. The cells before the first back in either arm (the unscrambled A, B and F, the lag sweep's one- and ten-percent points, whose moves fell between the timed passes or in no rep at all) read 0.78-1.03x for the option across the two sittings, by position, since the two arms run the same code there.

The lag sweep's full rewrite at three million keys under 1.25 GiB, a store that fits, read its pass at 0.05x of LMDB in the row at `8028e78`. Taken apart at `6ae0570` with the counters and the per-scan timers on (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,lmdb-nordahead --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 1`, and `--arms supdb-cache256tofit,supdb-nocache` beside it), the pass was 433 ms against LMDB's 27, its median scan 55 µs against 8.2, the scanning thread's CPU 277 ms and 4,733 voluntary switches in 3,000 scans; its phases put 116 ms in installing the builder ahead's forms and its scans built 11,033 blocks, where the arm with a 256 MB budget read the pass in 96 ms with a median of 8.0 µs. Paired one arm at a time against the default at `034edc8` (`--arms supdb,supdb-noahead`, `supdb,supdb-cachenoauto` and `supdb,supdb-tofit`, `--reps 2`), the arm with no budget of the cache's own read the pass 12.6x faster (36-38 ms against 452-559, the median scan 4.7 µs), the arm without the builder ahead 4.0x, and the arm whose forms are built to fit 0.53x: the cause was the budget, an eighth of the limit at 160 MB binding under a working set of 388 MB, with the cache shedding and building again. A profile cut to the pass (`perf record -a -e cpu-clock -g -k CLOCK_MONOTONIC` from the ten-percent point's end, the samples cut to the lag marks by thread) put a fifth of the scanner's samples in the allocator's own code and 16% in futex calls on its arena locks, beside three builder-ahead threads, one a merge landing, that used 350 ms of CPU inside the 429 ms window. The budget grows into the room the partitions leave of the controller's page-cache budget at each crossing now (`Options::scan_cache_auto_room`; `supdb-cacheeighth` is the eighth). Paired at `7310dd0` (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb-cacheeighth --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2`, two sittings) the pass read 6.7x and 9.1x, 45-80 ms against 304-525, the median scan 4.6-5.0 µs, and the sweep's weighted sum 2.0x, the budget growing once a store and the cache holding 439 MB with 618-713 MB of anonymous memory at the pass. In the row's whole order at three million keys (`--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled`) the cache never crossed in either arm, and at ten million keys under the same bound (`--size 10000000 --reps 1`) the room was nil and it never grew. The first scan of the pass waits 10-50 ms for the upkeep thread's last fill of the burst, which is what the pass has left against LMDB's.

The scrambled E at three million keys under 1.25 GiB, a store in core, read 0.84x of LMDB in the row at `8028e78`. With the counters on (`SUPDB_MIX_KINDS=1 SUPDB_MIX_COUNTERS=1 SUPDB_MIX_EVENTS=1 bench ab --arms supdb,lmdb-nordahead --size 3000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --ops-div 10 --reps 1` at `7310dd0`) its scans took 235 ms against LMDB's 145, the settle 43 ms of them for 40,827 writes the scrambled write mixes left scattered over 24,459 blocks, against 14 ms for the unscrambled E's 30,030 over 10,807, and the walks were 445 copies, 55,351 sparse and 22,882 others against 45,470, 19,455 and 8,099. A filing of a raw partition's writes by the ordered index's samples, their cuts left to the builds (`Options::settle_lazy_raw`), filed 44,000 keys a sitting and read the scrambled E 1.06x and 1.02x in two sittings (`bench ab --arms supdb-settlelazyraw,supdb --size 3000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --ops-div 10 --reps 2` at `0deae36`, every rep its way), and the lag sweep level: an arm. Undivided the mix read 0.92x (`--ops-div 1 --reps 1` at `0deae36`, profiled with `perf record -a -e cpu-clock -F 1999 -g` over the mix's window): the scanning thread's CPU was 1.84 s in a window of 2.47 s against LMDB's 1.88 in 2.29, with 2,310 samples of the merge threads beside it, and its run-queue wait 11 ms by `/proc/<tid>/schedstat`. Timed by kind (`SUPDB_MIX_KINDS=1`, the same pairing undivided at `0deae36`), its 249 commits took 710 ms against LMDB's 244 and its scans 1,503 against 1,437; the unscrambled E's commits took 228 against 258. `strace -f --seccomp-bpf -e trace=fdatasync,fsync,sync_file_range -T` attached for the mix's window read the commit thread's fdatasync at 0.81 ms in the unscrambled E, at most 1.5, and in the scrambled one 119 under a millisecond, 63 of 1-3, 57 of 3-10 and 10 longer, with the merges' writer helpers syncing 8 MB every 20 ms or so in 1-4.5 ms each. A merge's writers syncing every megabyte (`Options::merge_sync_ahead`; `bench ab --arms supdb-mergesync1m,supdb` at `8fa06a4` in the same order undivided, two sittings, and `--passes load,lag` at three and ten million keys) made 1,203-1,228 merge syncs against 167 and read the scrambled E's commits at 638 ms against 631 and 520 against 597, and the lag sweep at three million keys 12% worse by its weighted sum in both reps: refuted, an arm. The seals' and merges' threads at the idle I/O class on a device scheduled by `mq-deadline`, which honours the classes (`--arms supdb,supdb-bgidle` in the same order undivided at `8fa06a4`'s tree, both orders), read the scrambled E's commits at 863 ms against 724 and 756 against 673: what the commits wait on beside the merges is neither the device cache's backlog of their output nor the queue's order, and is open.

The full-rewrite pass at three million keys after the room-sized budget is a first scan and a steady walk: the scans after the first at 4.6-5.5 µs a scan against LMDB's 8, and the first at 44-69 ms, which waits for the upkeep thread's last fill of the burst or settles the residue the thread had not filed, 38-58 ms of settle by the phase timers (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_MIX_EVENTS=1 bench ab --arms supdb,supdb-passyield --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2` at `8fa06a4`'s tree, both orders). The pass that yields between its steps (`supdb-passyield`) read that pass at 0.82-1.20 s against 62-92 ms in both orders, its scans settling and building what the thread had: refuted again, in core. The row divides the pass by ten, so the first scan is weighed ten times in the cell, and a settle that files only the blocks a scan reaches is the lever left.

E is the mix left behind LMDB once C installs at the load's end, and its time is the form's decodes, so the re-code policy the forced form has was given to the controller (`Options::zpages_recode_auto`): once the form is installed, the hottest block-coded partition holding at least a twentieth of the decode time is re-coded to the raw tails while the form with their extra stays within 85% of the budget, and past 95% the coldest raw-tailed one goes back, the refaults deciding nothing, since under the forced form they gave 229 raw tails back over the mixes against 77 taken. Paired at `bd6d9f4` (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zrecodeauto --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2`) it did what it was written to: the three hottest partitions raw-tailed in B, two more in D, those two given back in E with no flip after. And E read 0.87x in both reps, with 0.0135 major faults an operation where the default takes none: through E the budget falls from 1.08 GB to 0.87 GB as the mix's own memory grows, so the 36 MB the raw tails added were past what the page cache had left, whatever the fill and evict lines said of the form alone. At this bound E's form has no room to grow; the option is off and `supdb-zrecodeauto` is the arm. What E pays for beside the decodes is the overlay the write mixes leave: 318k updated keys unsealed in the memtable over the Zipfian's hot range, where E's scans start, so the mix builds 268k blocks over them, 149k by the scans and 119k by the upkeep thread, and its scans wait 1.0 s in all for the thread's pass after each of E's 834 commits (`SUPDB_MIX_COUNTERS=1`, the `take` phase). The pass that yields to a waiting scan (`supdb-passyield`, `--arms supdb,supdb-passyield` at `cdb8e4e`) took that wait to 0.1 s and E read 0.98x in both reps: the scans built 138k blocks themselves where the thread had built them, so the wait was the work changing threads and not a cost of its own. What the wait was, the pass's steps say, timed apart on the thread's clock (`Db::pass_steps`, the `pass_*_us` counts): over E's 832 passes the settle of its 83,000 inserts took 19 ms and the snapshot's refresh 0.3, and the complete-table fill -- a form built for every block an unsealed key overlays and no form stands on, the builder ahead's work done on the commit's pass where the builder is off, as it is out of core -- built 143 forms a pass, 119,000 over the mix, 684 ms, while the scans waited 735 ms on it; and the walks the scans made met the same forms with those builds and without them (`walk_copy` 2,011,652 in both arms), so every one was for a block no scan read. The fill gives the writer its upkeep back at its next block now, the settle before it whole and the pass published (`Options::upkeep_fill_yields`, on; `supdb-fillholds` is the shape that waited), and the pairing read it level: `SUPDB_MIX_KINDS=1 SUPDB_MIX_COUNTERS=1 SUPDB_MIX_EVENTS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-fillholds --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2` at `1f4d2b2` read E 0.98x, the scans' wait 60 ms against 788 and the thread's passes 260 ms against 768, with the main thread's minor faults over E doubled, 26,000 to 52,000, as the thread's fell from 33,000 to 5,000. The profile of the pair (`perf record -e cpu-clock/freq=499/ -e page-faults/period=1/ -k CLOCK_MONOTONIC`, the same arms at one rep, cut to E's window by thread) put the writer's CPU over E at 6.14 s against 5.46 with the yield, 0.25 s of it kernel time the holding arm's writer did not have, and the faults by instruction in the allocator (`_int_malloc`, 20,000), the copies into fresh buffers (`memmove` 9,800, `memset` 7,900) and the overlay builds (4,800): E is the only mix that scans, so it begins with no block cache and builds 137,000 forms, 97 MB, inside its own window, every page a first touch, and under a group at its limit a first touch is a reclaim, about 10 µs. The thread's fill had been first-touching those pages while the writer waited for it; its forms dropped, the writer's own builds found the pages warm in the allocator's free lists, so the wait the yield removed came back as the writer's faults. Not a churn: the census (`SUPDB_MIX_CENSUS=1`) read the trim thread trimming nothing through the mixes in either arm (`trims 0`), the allocator holding 76-91 MB free beside 210 in use at E's end, and `supdb-notrim`, a diagnostic arm with no trim thread, read the same faults. The yield stands as the shape whose cost is the cache's building alone, and the building is the lever.

A cold decode is the codec's as well as the format's, so the form's own groups were given to every codec at hand (`bench/examples/zcodec.rs`, `cargo run --release --features codecs --example zcodec -- --size 300000` in `bench/` at `cd2ee19`): each group's records compressed alone as the form stores them, every decode checked against the records byte for byte, and each codec timed cycling all 21,624 groups, its order rotated round by round. Against the shipped encoder's bytes (`lz4_flex`'s greedy parse, 0.593 of the records), zstd stores 0.55-0.58 at levels -3 to 3 and 0.51 with a dictionary trained on the groups -- half of each value is random bytes, so a half is the floor -- and decodes 1.5-2.3x slower at every level alike, so its cost on a group of 1.7 KB is its block's and not its parse's; Snappy decodes 1.2x slower; LZAV 7.1 (`bench/vendor/lzav`, upstream's header) decodes in 0.89 of the time at 0.580 for 1.3x the encoder's CPU, and in 0.81 at 0.574 by its higher-ratio compressor for six times it; and the LZ4 format with a better parse -- the engine's lazy parse (`Options::zpages_parse`), or liblz4's high-compression mode from level 3 -- decodes in 0.78-0.80 of the time at 0.569 for 2.3-3x the encoder's CPU, 52 sequences a group against 62, each a branch the cold decode stalls on. No other format pays at this group size, and the lever is the parse. Paired on the default at ten million keys under 1.25 GiB (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zparse --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2`, and `--arms supdb-zparse,supdb`, at `cd2ee19`) the parse doubled the builders' CPU, 9 s to 18 s, so the builds ahead fell behind the load: the mixes began with ten partitions still to build where the default had five or six, and the last landed in B, where the default's land within C's first two seconds. In the first sitting B's reads were 13% slower and E's scans 5% faster in both reps; the second, on a host whose D moved by two fifths between reps of one arm, split on E either way. The parse stays off and `supdb-zparse` is the arm: what it buys belongs after the install, a served partition re-encoded once the builders are free, and not in the builds the install waits for.

The parse was then deferred past the install (`Options::zpages_reparse`): the builds keep the greedy encoder, and once the form is installed and the builder has nothing due, each look of the re-code policy builds the hottest block-coded partition holding at least a twentieth of the decode time again by the parse and swaps it into its cell, once while its segment lives. Paired at ten million keys under 1.25 GiB (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zreparse --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2`, and `--arms supdb-zreparse,supdb`, at `d94690a`) it re-parsed eight partitions a rep -- two or three in B, one or two in A, three or four in D -- the form 1.6-2% smaller at the end, and E's scans read 1.4% faster over the four pairs, while the mixes it built in paid for its writes: D's commits 21% slower and A's 9% in both orders, each rebuilt file's bytes flushed by the durable commits' fsyncs as any build's during a write mix are, and D's reads 4% slower beside the builds. A fifth off the cold decode of the partitions E reads most moved E by a seventieth, so on this host the decodes the parse reaches are a smaller share of E's scans than the profile before it read; the option is off and `supdb-zreparse` is the arm.

The lag sweep's store is the shuffled load's, whose last merge rewrites every partition at once, and its form installed only inside the full rewrite's pass: the sample at the load's end found two files of twenty-four on disk, since the builds ahead wait while scans are near and the sweep had scanned, and with nothing to attach the controller rented -- its scans' faults accrued against the build's price (`zselect::ski_rental_price`) for about twelve seconds of wasted fault time -- so the pass after the ten-percent rewrite walked the raw form at 1.7 major faults a scan, 9.4-12.3 s against LMDB's 8.2 (`SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-nozpages --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 1` at `4fc997a`, the trace read against the lag marks). Past the selector's line the builds ahead may run beside scans, and a dormant file whose landing brings the files on disk to the install's coverage has the selector sample again, so the attach on the prediction follows (`Options::zpages_ahead_past_scans`). Paired at ten million keys under 1.25 GiB (`... bench ab --arms supdb,supdb-zaheadscans --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2`, and the arms the other way, at `4fc997a`) the form installed inside the first pass in every rep and the arm moved the work rather than removing it: the pass after the one-percent rewrite read 1.5-1.6x faster in all four pairs, on a form resident with no device read at all, and the clean store's pass before it 1.3-1.6x slower in all four, the twenty-four builds reading 1.7-2.0 GB from the device and writing 0.75 beside its scans; the ten-percent pass read faster in three pairs of four and 1.2x slower in the fourth, its form half out of the page cache -- the installed arm's process held 690-770 MB at that pass's end against the raw arm's 450, a snapshot's tables and a replaced memtable of a hundred megabytes each beside the allocator's retention -- and the full rewrite's pass was level, its merges the pass's own. The sweep's weighted sum read 1.20x for the arm in one sitting and 1.14x against it in the other, on the fourth pair's slow pass. The option is off: the install is right, and the lever is where the files are built -- at the merge's output as each partition is finished, from the pages the merge just wrote, before any pass -- and what the installed store holds beside its form.

A merge's outputs are given their form files as each is finished (`Options::zpages_build_at_finish`; `supdb-nozfinish` is the arm without it): the merge's thread starts a builder over the output it has just renamed into place, while the pages it wrote are in the page cache -- at most `zpages_ahead_builders` at once, the merge waiting for a slot -- where the store is past the share of its budget the builds ahead at landing start at, past its budget, or the form installed, with the builds ahead's plan (past the selector's line or installed, the raw records behind a build are dropped and its file kept; under it the file's pages go and the records stay). The merge does not wait for its builds, and a build's end has the controller count the files again. Through the lag sweep at ten million keys under 1.25 GiB the builds read warm, 135-137 a sitting with 8-11 major faults among them and 20 s of CPU in 22 of wall, where the builds ahead at landing read the same partitions back from the device. Three controller rules then stood between the files and the install, each right for a build and wrong for an attach, and the first pairing of the shape read as a loss with the form never installed in three reps of four (`CLAUDE.md`): the wasted-build budget's suspension, set during the load's merge chain, refused the attach as it refuses a build; a sampler's reading is of the partitions at a generation and under the chain's landings none ever reached a step, forty-one samples through one pass; and an output being built at its finish was not yet a build in flight. With the suspension holding off builds alone, the reading refreshed against a count of the files in place of a sample (`Maint::z_sample_refresh`), and the in-flight builds counted, the arm installed its form at the sweep's first point in every rep on one sample and kept it, and paired against the default both ways (`SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zfinish --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2`, and the arms the other way, at `4dc34f1`) it read every pass faster in all eight pairs but the full rewrite's, which was level: the clean store's pass 2.4-3.0 s against 3.5-3.8, the pass after the one-percent rewrite 1.4-1.6 against 1.8-2.8, after the ten-percent rewrite 7.0-7.9 against 9.1-10.4, after the full rewrite 11.2-11.5 against 11.3-11.8, with the bursts within six percent either way and the ordered and the shuffled loads level. The default arm installed only inside the full rewrite's pass, by its own build, in every rep. A pairing's table is read with its arms in the order they were given: `bench ab` prints them so, and the second order's columns read as the arm's loss until the pass times were listed by run position. What the full rewrite's pass still pays with the form installed is the chain's own: twenty merges land inside it in both arms, and the arm's scans take 0.8-1.0 major faults each where the default, building its form from the pass's own reads, ends the pass with a form it has just written.

Where the sweep stands against LMDB with the builds at finish the default, at ten million keys under 1.25 GiB against `lmdb-nordahead`, two reps each way (`SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2` at `05b504e`): the clean store's pass 2.5-2.8 s against LMDB's 8.9-9.0, the pass after the one-percent rewrite 1.4-1.5 against 6.8-7.2, after the ten-percent rewrite 4.3-4.8 against 9.0-9.5, and after the full rewrite 11.0-11.6 against 9.1 -- the one pass the sweep still loses, at 0.8x -- with the scans' major faults at 0.22, 0.00, 0.49 and 0.93 a scan against LMDB's 1.5-1.9 at every point, the bursts 33-54x LMDB's rate, and the loads 1.36x and 13.8x, ordered and shuffled. The full rewrite's pass is where the chain's twenty merges land, and what it pays is listed above.

The idle I/O class for the seal and merge threads (`Options::background_io` Idle, which the previous engine priced on the commit path and nothing had priced under a bound; `supdb-bgidle` is the arm) was the obvious lever on that pass and lost it threefold. Paired against the default at ten million keys under 1.25 GiB, one order (`SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-bgidle --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2` at `f473092`), the full rewrite's pass read 31.8-32.0 s against 10.4-11.2 in both reps, with 160 pieces standing at its end against 6, 4.4 GB read at the device inside it against 2.7, 360,000 major faults against 96,000, and its tenths flat at 250-390 where the default's fall from 488 to 32 as its chain drains; the ten-percent pass 4.3 s against 3.6-3.8 with a third more faults and the same 24 pieces; the one-percent pass, the bursts and the loads level. The merges at the idle class yield the device to the scans' faults, and the scans pay for it at every piece the merges have not folded: under a bound a merge's I/O is the readers' I/O deferred, since a scan touches a page of every piece standing over its range, and a class that serves the readers first serves them the pieces. The drained store's pass read faster under the arm (2.5-2.7 s against 3.1-3.6, a third of the device bytes) in the one order run; the second order was not run, since the full rewrite's loss is a count and not a rate, and that reading stands unresolved. What the pass pays is the chain's drain time inside it -- two thirds of the pass in its first two tenths, while the merges read and write at the device beside the scans -- so the lever is the chain's speed, not its priority.

The chain's speed was then taken from the shape its reads have: a merge's one sequential read of each input is the open's verification of its key section, through a mapping, one kernel readahead window a wait (128 KB on this device, which takes 4 MiB a request), each wait a device round trip behind the scans' faults -- the shape whose ask-ahead took the controller's build from 80-93 s to 29-60 s under the same contention. `Options::merge_read_ahead` (`supdb-mergeahead`, four-mebibyte windows, `BlobOptions::read_ahead`) asks for the section ahead of that walk and for the records ahead of the second pass, and paired against the default in one order at ten million keys under 1.25 GiB (`SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_ZPAGES_TRACE=1 SUPDB_MERGE_TRACE=1 bench ab --arms supdb,supdb-mergeahead --size 10000000 --bound 1342177280 --passes load,lag --ops-div 1 --reps 2` at `2e5c232`) it fired -- 641 windows and 2.2 GB asked over the arm's two reps, none in the default -- and moved nothing: the full rewrite's pass 10.5-11.2 s against 10.8, its tenths the same shape (494/209/56 against 505/208/66 at the start, 35 at the end), 2.6 GB read and 1.9 GB written at the device inside it in both arms, the ten-percent pass 3.3-4.2 against 4.0-4.2, the bursts and the loads level. A prefetch is a lever on a wait, and the chain here is not waiting: with the scans' faults beside it the device carries 4.5 GB in the eleven seconds, and asking for the bytes sooner changes neither the bytes nor the device. The option stays off, an arm. What is left of the pass is the chain's bytes inside it -- the whole store rewritten once, read and written, beside the scans' own faults -- and the one lever on those is when they are paid: the chain run through the burst itself (`Options::merge_chains_writes`), which on top of the idle plan read the sweep at 1.06x and the full rewrite at 1.08x of the default's time, so paying them earlier paid more. Three levers priced, the pass stands where the design puts it: a store rewritten once at the device's rate while it is read, against a comparator that rewrote in place during a burst thirty-five times longer, and read beside the burst it is the trade the sweep's weighted sum reads at 13.9x in the engine's favour.

That reading was the sum's, and the row reads the cells. The roster divides the lag sweep's passes by ten, so the pass after the full rewrite is the first tenth of its scans, the tenth where the chain's drain sits, and the row read it at 0.15x of LMDB at ten million keys under 1.25 GiB while the burst before it read 29x. The chain through the burst was paired again on the row's own division at `05008c0` (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-chainwrites --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2`, which at `52b488c` is `--arms supdb-nochainwrites,supdb`): the full rewrite's pass read 3.12x (488,000 entries a second against 157,000) with 33-36 pieces standing at its end against 104, 0.26-0.31 GB read and 0.39-0.40 written at the device inside it against 1.71-1.75 and 1.26-1.30, the burst 0.74x (313,000 updates a second against 424,000), the one- and ten-percent passes 1.08x and 1.21x, in both reps; through the row's whole order, the mixes and the lag sweep after them (`--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag`), 2.95x with the burst at 0.68x and the one-percent pass at 1.32x, the ten-percent pass at 0.90x. No merge lands in the row's mixes in either arm (`SUPDB_MIX_COUNTERS=1 SUPDB_MIX_EVENTS=1`, `merges_landed` 0 in every mix), so the option acts in the shuffled load and the sweep's bursts alone, and the mixes' cells run the same code. Undivided (`--ops-div 1`, the same arms) the full rewrite's pass read 1.47x, 10.1-10.7 s against 14.8-15.1 and LMDB's 9.1, its first tenth 211-215 ms against 663-693, the burst 0.73x and the sweep's weighted sum level (0.995x). The chain runs through the burst by default now (`Options::merge_chains_writes`): the pass, the cell the sweep lost most, gains threefold in the row's division, and the burst, a cell won twenty-nine times over, keeps twenty. What the divided pass pays is the drain that is still left at the burst's end, its first four tenths 1.3 s of its two against later tenths of 110 ms each, near LMDB's pace, beside the chain's last merges and a third of the pieces.

Its faults were not the pieces'. Every major fault of the full rewrite's pass recorded with the file it landed in (`perf record -e major-faults -c 1 -g -d` between the pass's lag marks, `prof-majf.sh`'s shape, at `52b488c`) put 11,600 of 11,939 in the form files, in the page view's parse of a group's page, and 276 in the pieces: eleven form files of twenty-nine were read cold end to end, about three pages a scan. Each had been built at a merge's finish with its pages kept (`drop false` on the build's trace line) five to nine seconds before the pass, and the page cache's residency by kind at the pass's start (`scripts/resident.py` at the lag marks) read 497 MB of the 746 MB of form files resident, beside 164-183 MB of the burst's pieces, 59-118 MB of merge outputs not yet dropped and 63-67 MB of ordered indexes: a page written once and never read is the first the kernel reclaims, and the burst's last seconds of writes took the forms written before them. Out of core with the form installed, scans that come back after a stretch of a second without one now have the form files under nine tenths resident asked for whole, on a thread of their own, once no merge is in flight (`Options::zpages_rewarm_scans`; `supdb-norewarm` asks for nothing): asked at the pass's start beside the chain's last merges, the pages were pushed out again, 117,000 refaults against 25,000 and the pass 0.84x, so the ask waits for the chain. Paired at ten million keys under 1.25 GiB (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb-norewarm,supdb --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2` at `cd01ed1`), the ten-percent pass read 1.24x with 0.35 major faults a scan against 0.62, the one-percent 1.21x, the full rewrite's 0.99x with 0.86 against 1.24; through the row's whole order (`--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag`) the ten-percent pass 1.81x with 0.05 faults a scan against 0.79, the full rewrite's 1.13x with 1.01 against 1.39, both in both reps, and every rewarm look in the mixes found no form file cold. The full rewrite's pass is still the chain's: its last merges land a second into it. Two merges in flight (`supdb-mergepar2`, now on top of the chain) finished the chain before the pass and read it 1.94x and 1.45x in two sittings, and read the clean store's pass and the one-percent pass slower in three reps of four: in one of them the sample came in the middle of the shuffled load, over fourteen partitions of twenty-four, and picked the raw-tail coding for a store that grew past it, which the refresh from the files on disk does not pick again.

The clean store's pass and the one-percent pass at ten million keys under 1.25 GiB read two shapes in the row at `d8cece6`, 0.61 and 0.80 million entries a second in two reps and 2.33 in the third for the first, 0.82x, 0.89x and 2.13x of LMDB's, and the row's own quantities for the point split the reps before any probe: 1.6 and 1.5 major faults a scan against 0.28, 9-10 refaults against 0.4, 27-33 KB a scan read by other threads against none, and 160-340 µs a device read against 38. The shuffled load's last merge is where it starts. The line the builds at a merge's finish take their posture from is read over the partitions (`Maint::z_land_look`), which hold a part of a shuffled load's keys until the drain's merge lands, so that merge's twenty-four form files, 746 MiB, are written back and dropped as they are built, the bet a build ahead makes; its landing takes the store past the line, and the look that finds it there drops the records and asks every file back (`Maint::z_cross`), 1.1-1.4 s before the sweep's first scan (the `zfinish`, `zahead` and `lagmark` lines of `SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 sh scripts/ooc.sh 1342177280 --arms supdb --bottom 10000000 --reps 6 --out DIR` at `d8cece6`). Two readings of that second were built on and each was wrong in a way its own pairing showed. The files kept at their build instead -- the merge's posture judged against the store it would leave, its inputs' pieces counted with the partitions -- were not there at the landing: the sweep's first pass read 776, 361 and 719 ms with them against 348, 672 and 411, the sampler's timed decodes faulting in two reps of three in both arms, since a page written once and never read is the first the kernel reclaims and the merge's later outputs took the files written before them, and a file kept is left dirty besides; that shape is not in the tree. A settle that waited for the ask's read to finish then found nothing in flight to wait for: in two sweeps of three it read 361 and 593 MiB of the 746 resident and not a page more over the quarter second it looked. The ask is advice, and under a group at its limit the kernel reads what it has pages for and drops the rest without a word, so what the first scans met was a form half in memory, read a fault at a time under the files' random advice. A settle now waits for the form files the store has asked for, and where two looks in a row find no page more it asks again for the files still short, the records of a partition its form serves dropped first for the room, up to three times (`Options::zpages_settle_joins_asks`, `Maint::z_join_asks`; `supdb-zsettleloose` returns without the wait). Paired in the row's whole order (`SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-zsettleloose --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --ops-div 10 --reps 3`, and the arms the other way with `--reps 4`, at `7e0fba7`), the clean store's pass took 387-546 ms in the arm's seven sweeps and 407-893 in the other's, two of its seven at 838 and 893: in the two sweeps where the wait found 353 and 382 MiB resident one ask again brought 677 and 742 in a third of a second, and in the other five it found 673-746 at its first look and waited for nothing. The pass's other threads read 0.2 KB a scan in the arm against 10 in the first sitting, a device read 38 µs against 248, and every other cell sat within its sittings' spread both ways; at three million keys no settle looked (`--size 3000000`, the arm's `zpage_ask_joins` at zero). The read is not saved: the files are still written, dropped and read back, and the lever on that is the kernel's order of reclaim, which a build cannot set by writing a file once. The ordered load's store crosses inside its load and the suite reads it after a `sync`, which joins nothing, so its read pass still begins beside whatever the crossing's ask left.

The first scan after the one-percent burst was a third to a half of its divided pass, 129-212 ms of 310-430 at ten million keys under 1.25 GiB (`SUPDB_SCAN_OPS=1`'s `lagops` line). The lag counters split it: in three reps of four the scan waited 89-172 ms to take its upkeep back from the thread's pass in flight (`take`), and in the fourth it settled the burst's residue itself in 114 (`settle`); the thread's passes over the burst took 233-305 ms of its CPU. Profiled from the mark before the pass and cut to the scan's window by time (`perf record --no-buildid-cache -a -e cpu-clock -F 4999 -g -k CLOCK_MONOTONIC` from the `lagmark` that ends the burst, at `8dc2da4`), the scanning thread spent 27-30% in the LZ4 decoder and 13-15% in the page views beneath the settle's resolve, and 9-12% in the settle's own loop: the burst's 99,494 writes into covered partitions are filed under their blocks with their cuts left for the build (`Options::zpages_settle_span`) only where the form's fence and the page's directory put the key's group inside one block of sixty-four records, and a group straddles a block's edge often enough that the rest fell to the exact seek, which opens the group and decodes it. The ordered index's top level holds one sample a block in memory (`OrdIndex::floor_stride`), which names the block of any key whose head ties no sample without a read of the form; a covered partition's settle asks it first now and the form's span only on a tie (`Options::settle_covered_samples`; `supdb-settlespan` files by the span alone). Paired at ten million keys under 1.25 GiB (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb-settlespan,supdb --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2` at `9a9f812`, and with the arms swapped in the row's whole order), the thread's passes over the burst took 125-180 ms, the scan waited 0.3-1.2 ms and settled 35-87, and the groups the pass opened fell from 117,000-125,000 to 100,000-108,000: the one-percent pass read 1.46x and 1.72x in the two sittings, its first scan 36-106 ms against 129-212, and the ten-percent pass 1.04x and 1.19x. The full rewrite's pass, whose settle is a hundredth of a millisecond in both arms, read 1.13x and 0.88x, and in the row's order the point-read mixes the change does not reach, C among them, moved by up to a sixth both ways, which is the sitting's resolution.

The full rewrite's pass at three million keys under 1.25 GiB, a store in core, read 0.60x of LMDB's in the row at `74b9dea`, and its first scan was three fifths of it: in the lag counters (`ZL_SIZE=3000000 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-settlelazyraw --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2` at `189042f`) it waited 4-41 ms to take its upkeep back from the thread's pass in flight (`take`) or settled 12-102 ms of the burst's residue itself (`settle`), in passes of 38-175 ms. A durable commit lends every batch to the thread, which files it through the fsync, and nothing bounded how far the thread trailed: the pace the buffered commits have (`Options::upkeep_pace_pct`) is for commits that would otherwise file their batch in the commit. The raw partitions' settle by the ordered index's samples (`supdb-settlelazyraw`) read that pass slower, 61-70 ms with the scan's wait at 29-31, since the builds then resolved what the settle had left. A syncing commit's batch of a burst near reads is paced now too, while the store is in core, within a quarter of a percent of the store's keys (`Options::upkeep_pace_syncs_bp`, the commit returning after its fsync once the thread trails by at most that or the batch; `supdb-nopacesyncs` lends unpaced). Paired at three million keys (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb-nopacesyncs,supdb --size 3000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 3` at `71554b8`, and with the arms swapped in the row's whole order at `--reps 2`), the full rewrite's pass read 3.29x and 2.43x, 22-48 ms against 39-175, the tenth's 1.02x and 1.41x and the hundredth's 1.20x and 1.39x; the bursts read 0.77x and 0.86x for the full rewrite, the writer holding 0.8-2.8 s of 9.6-12.2, and 0.70-0.85x for the smaller two. The thread's passes over the full rewrite doubled, 1,100-1,150 against 457-558, its settle a second longer and its fill's walk over the blocks 0.8 s longer, since a pass's fixed part is paid more often. In core with no bound (`--size 1000000` and `--size 300000`, `--passes load,lag --reps 4`), the full rewrite's pass read 1.43x and 1.20x and its burst 0.83x and 0.91x. No mix holds: a mix's batches of a hundred are under `Options::upkeep_inline_min`, and every mix of both arms counted no hold (`SUPDB_MIX_COUNTERS=1`). Out of core it is not paced: the first version paced every store, and at ten million keys under 1.25 GiB, where the default's thread files little of the full rewrite and its resolves are major faults, the bursts read 0.29x, 0.44x and 0.86x, the one-percent burst under RocksDB's, and the full rewrite's pass 0.74x, on the tree before the gate, which no commit holds; gated, both arms count no hold there in the same pairing at ten million keys.

In the row's order the same pass read 0.2-0.5x of LMDB's again (`ZL_SIZE=3000000 SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,lmdb-nordahead --size 3000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --ops-div 10 --reps 3` at `b77e5e5`): 99-245 ms against 28, the first scan 57-216 of it, waiting up to 59 ms for the thread's pass in flight and settling 40-130 itself. The pace had stopped part of the way into the burst. The controller's trace put its census four seconds into a burst of eight, entered on the group's refault rate, 39 a second at one step and 13,000 at the next, with the block cache grown to 439 MB of the bound and the burst's merges running; the pace held at the raw form's gate alone. The lag-only pairings that priced that gate had met the same census and read the pass at 27-48 ms, the first scan settling 5-13 ms of what the unpaced stretch left. The pace stops only at the compact form's gate now (`Options::upkeep_pace_syncs_census`; `supdb-pacesyncsraw` stops at any gate but the raw form's). Paired in the row's order (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb-pacesyncsraw --size 3000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --ops-div 10 --reps 3` at `ac7a816`, and again with the arms swapped and `SUPDB_MIX_COUNTERS=1`), the full rewrite's pass read 5.3x and 15.0x, 15-25 ms against 37-392 and LMDB's 28; every batch of the burst held (2,700 holds against 1,209-1,597), and the burst read 0.85x and 0.83x, fourteen times LMDB's still. At ten million keys under the same bound (`--passes load,lag --reps 2`) the form is installed during the load and every burst of both arms counted no hold. No mix of either arm held. The write mixes, which run the same code in both arms, read 0.8-0.9x for the default in both sittings, and timed by kind (`SUPDB_MIX_KINDS=1`, `--passes load,read,read-threaded,scan,scan-threaded,mixes --reps 4`) their time is the commits' fdatasyncs, 0.41-0.78 ms a commit from process to process in both arms.

The one-percent pass at three million keys under 1.25 GiB read 0.87x of LMDB's in the row at `580892e`: its first scan was 3-9 ms of a pass of 14-31, waiting 1.4-8 ms for the upkeep thread's pass in flight and settling up to 4 more of the burst's trail itself, with the scans after it at 3-5 µs. The thread's profile over the full rewrite's burst at that rung (`perf record -e cpu-clock` cut to the thread between the ten-percent pass's end and the full rewrite's start) put a fifth of it in the settle's filing, a sixth in the exact seek that resolves each write and a tenth in the fill, which walks every block of every partition at every pass its tables are incomplete -- 25 ns a block, a millisecond a pass, 1.3 s of the thread's 7.1 over that burst -- while the writer was held for 2.4-2.6 s of the burst's 9.3. Two levers on the trail were priced and left as arms. The syncing pace at a twentieth of a percent (`supdb-pacesyncs5`) read the passes 1.09-1.75x and the bursts 0.75-0.88x, the full rewrite's under RocksDB's, since every batch became a pass of the thread's and the fill's walk with it. The settle's and the bounds' head asks keyed on the form installed rather than on the census (`Options::out_of_core_installed`, `supdb-oocz`) -- the store sits at the census from its load on, and asked for heads 580,000-690,000 times a sweep with 1,500 of them submitted, a `mincore` each -- read F 1.13-1.22x, A 1.03-1.20x and E 0.86-0.98x in three sittings of the row's order; the same switch over every mechanism the census turns on read A 0.63x and the scan pass 0.82x, and over the record asks as well E 0.95x, since that store does refault under its bound. What ships is the fill left for a later pass while the writer has posted a newer commit (`Options::upkeep_fill_skips_posted`; `supdb-fillposted` fills as before): the burst held the writer for nothing, the first scan waited 0.2-0.8 ms and settled nothing, and in the row's order (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb-fillposted --size 3000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --ops-div 10 --reps 2` at `d22c2ad`, three sittings, both orders) the one-percent pass read 1.12-1.84x, its burst 1.16-1.63x, the other bursts 1.00-1.14x, A, F and the scrambled A 1.05-1.44x, the two-thread scan beside the mixes 0.85-0.99x and E either way, the scans after the bursts building more of their own blocks where the fill had built them. At ten million keys (`--size 10000000`, two sittings) the lag sweep's forms are off and its fill never runs, and the mixes read level or better.

The full rewrite's pass at ten million keys under 1.25 GiB read 0.71x of LMDB's in the row at `3a35695`, and the same row put the arm with no block cache (`supdb-nocache`) at 1.69x the default on that pass, with 0-0.14 major faults an operation against the default's 0.45-0.98. The pass is read through the form files, 945 MB of them beside a page cache of about 800, and the process held 431-445 MB of anonymous memory with 152 in use (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb-norewarm --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2` at `ac7a816`), the rest free chunks resident in the arenas, a third of them in one arena of 290,000 chunks. The rewarm trimming the allocator before its asks (`Options::zpages_rewarm_trims`) returned 79-170 MB at each rewarm, and paired (`... --arms supdb,supdb-rewarmnotrim --reps 2`, then the arms swapped at `--reps 3`, on the tree of `0f23188` with the trim the default) the full rewrite's pass read 1.29x and 1.12x with a third of the faults; the ten-percent pass read 0.80x and 0.73x, under RocksDB's where the default is level with it, and the bursts level and then 0.84-0.88x. The rewarm after the load trims the free chunks the bursts after it would take again, and a page given back is a fresh page the host prices at its next touch. Off; `supdb-rewarmtrim` trims. The pass's first tenth, 330-570 ms where each tenth after it is 75-160, is the first scan's snapshot over the 388,000 unsealed keys the burst leaves (50-80 ms) and the cold form files' faults beside the chain's last merges before the rewarm's asks land. A profile of the scanning thread over the pass, beside the same pass with no block cache (`perf record -e cpu-clock -k CLOCK_MONOTONIC` between the lag marks), put the default at 846-909 ms of CPU and the arm with no cache at 497-565: the decode is a quarter of both, and the default's difference is the block path's builds for blocks no later scan reads, the kernel asks ahead of them (`mincore` and `madvise`, a sixth of the thread), and 978-3,148 major faults against 191-205. The asks pay for themselves where the block path runs -- without the spans asked (`supdb-noblockwillneed`) the full rewrite's pass read 0.68x, without the pieces' (`supdb-nowillneed`) the one-percent pass 0.75x -- so the cost is the block path itself where its blocks are met once.

The block path builds a form for every block a scan walks and repays the build only where a later scan walks the block again. Out of core at ten million keys under 1.25 GiB the lag sweep's passes start uniformly over 156,000 blocks and meet each about once: the full rewrite's pass walked 24,700 blocks and built 23,500 of them. Out of core the writer now measures that share at every scan's start -- whether its block, the ordered index's stride for the key read off the samples, was a recent scan's start, in a direct-mapped table of 1,024 slots over windows of 512 scans -- and where it is under an eighth it gives its forms up the way a change of generation drops them and takes the merge path, until the share passes a quarter, the store is back in core, or a handle the caller holds is live (`Options::scan_forms_off_unreused`; `supdb-formson` keeps them). The handles were the first version's mistake: every handle ran the regime over its own starts, and the threaded scans beside the mixes read 0.85x in two sittings, since three threads reading the same blocks through the writer's published forms is a reuse no handle's own starts show. Paired at ten million keys under 1.25 GiB (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 bench ab --arms supdb,supdb-formsoff --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2`, and in the row's whole order, `--passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --reps 3`, on the tree of `d422a8a` with the regime an arm), the full rewrite's pass read 2.2-2.6x, the ten-percent pass 1.4-2.2x and the one-percent 1.3-1.5x, the bursts 1.05-1.2x, scan-mixed and the threaded passes level, ycsb-E 1.02x -- its Zipfian starts recur, and the forms come back at its 512th scan -- and ycsbs-E 0.89-1.01x over five sittings, about a twentieth slower. One sitting of five read the ten-percent pass the other way in both reps; a rep of a later sitting in that shape decoded no group of the compact form through the whole sweep, its form installed and serving nothing, and the arm with the forms on met a milder form of the same in that sitting.

In one rep of those sittings the lag store's compact form was installed and decoded no group through the whole sweep, the ten-percent pass at 0.95 s where the arm's other reps read 0.28-0.35 and the full rewrite's at 1.5 against 0.40-1.29. The form was the raw tails. The controller's trace put the store in SUSPECT nineteen seconds into a load of thirty-six, on the refaults its own merges made, and the reading from the files on disk, fourteen partitions and 4.8 million keys, put the raw-tail form at 601 MB under a fill share of 940 (`zselect::FORM_FILL` of a budget of 1.11 GB). The pick held for every build after it: the store doubled, every output's form file was built raw-tailed at its finish, 1.14 GB of them asked for at the crossing against 0.75 in the other reps, and the install served a form the size of the raw records. A raw-tail pick is taken back now where the partitions' records have grown since it past the fill share of the budget it was made under (`Maint::z_repick`, at every build's plan and every refresh), and a form file on record in the coding the pick refuses covers nothing in the files' cover, so the attach waits for the selector's sample and the block coding's builds (`tests/zpages.rs`: a store that doubles after its pick builds the block coding after the growth, one rewritten at the same size keeps its raw tails). The shape before is no arm, as the pick at the budget's edge was none: a pick made against inputs that have moved is a wrong answer.

The take-back costs what the early pick bought. In one of two sittings of the row's whole order at ten million keys under 1.25 GiB at `f95e79c` the default arm's reading came in the load, the controller in SUSPECT on the refaults of the load's own merges before a read: it picked the raw tails, took the pick back near the load's end and gave back the twenty-four form files its merges had finished raw-tailed, 173 builds against 150 in the arm beside it, and three more sittings of other arms on that tree read the same, 181-188 builds against 148-150. The raw tails are a form for a reader -- one a point read or a scan decodes nothing of -- and a reading before any read has no reader to judge one for. A reading taken before the store has served a read or a scan picks the block coding now, which fits wherever the raw tails would, and the first reading after one picks again (`Maint::z_pick_raw_ok`, counted as `zpage_coding_unread`). In the row's whole order at ten million keys (`SUPDB_LAG_COUNTERS=1 bench ab --arms supdb-walappend,supdb-walprefill --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag --ops-div 10 --reps 2`, both orders, on the tree of `91bf79f` before its pre-write ahead, where the two shapes were the default and the recycling arm) one arm a sitting took its reading in the load, picked the block coding and counted it, and neither arm picked the raw tails, took a pick back or gave a file back.

Two merges in flight (`Options::merge_parallel` at two) were priced as a trade on top of the chain through the burst: the full rewrite's pass ahead, the clean store's and the one-percent pass behind. Both losses had causes of their own that are fixed now -- the clean store's pass met form files the burst had pushed cold, which the rewarm asks for again, and the one-percent pass's first scan settled through the form's decodes, which the samples' filing skips -- and paired again at ten million keys under 1.25 GiB (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-mergeone --size 10000000 --bound 1342177280 --passes load,lag --ops-div 10 --reps 2` at `7bc2e15`, and with the arms swapped in the row's whole order), two merges read the full rewrite's pass 1.56x and 1.88x, the clean store's 1.14x and 1.53x, the one-percent pass 1.61x and level and the ten-percent pass 0.97x and 0.95x; the bursts read 0.82-1.32x and the mixes, in the row's order, level to a tenth ahead. In core they lose nothing: at three million keys under the same bound in the row's whole order every lag pass read 1.13-1.80x and the bursts 1.05-1.11x, and at a million with no bound (`--passes load,lag,mixes --reps 4`) the lag cells sat within the sitting's noise and F read 1.23x in four reps of four. Two is the default; `supdb-mergeone`, `supdb-ingestmergeone` and `supdb-nozpagesmergeone` keep one.

At ten million keys the shape above reads as designed where the store is past its line, and as a measurement's artefact where it is not (`SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb-autozpages,lmdb-nordahead --size 10000000 --bound BYTES --passes load,read --reps 1` at `ad54c29`, one rep each way). Under 1.5 GiB the raw form is 1.38 GB against a budget of 1.40-1.49 GB, under the line: the builds ahead run at every landing from the twentieth partition, twenty-seven of twenty-nine within a quarter second of each landing with no major fault, nothing is dropped or swapped, the read pass divided by thirty is some two seconds, its reads fault at 0.06-0.08 a read on the files beside the records, and SUSPECT arrives as the pass ends -- a controller whose census ticks once a second and whose entry needs a sample cannot act inside a pass that short, and the sampler, which had waited for every build still due rather than the one in flight, would have waited past the pass in any case (`Maint::z_prebuild_pending`). Under 1 GiB the store is past the line in its load: SUSPECT at ten seconds, the sample finding twenty-three of twenty-four files on disk, the cross swapping those partitions' records for their files (`zahead ... crossed swapped 23 of 24`), the rest built behind the load's last landings, the buy at the load's end and the install about two seconds into a read pass divided by three. That pass reads 2.97x LMDB's with its readahead off (195,000 reads a second against 65,700, 0.036 major faults a read against 0.245, 148 bytes read a read against 1,002), the load 1.21x. With LMDB first and the scan pass added, the read pass reads 4.06x (222,800 against 54,900 reads a second, 0.025 major faults a read against 0.305, p99 59 µs against 106), the scan pass 4.54x (6.87 million entries a second against 1.52 million, no major fault a scan against 1.26) and the load 1.11x; LMDB's store of ten million keys is 1.3 GB and out of core under the bound as ours is. One rep each way, so the figures are the regime's and not a cell's: a pairing of three reps in each order is what a row needs, and the roster's read pass has to be long enough for the controller to act in -- divided by thirty it is not.

The decode was priced before the next structure for the form's reads was chosen (`bench/examples/zdecode.rs`: `cargo run --release --features lz4-c --example zdecode -- --size 300000 --split 1,2,4,8,16,32` in `bench/` at `02c2d64`). By instruction, callgrind over the harness's `get` and `range` in core at three hundred thousand keys: a point read through the form is 60% LZ4 decompression and 13% the seek inside the group, and E's scan on the merge path 6% decompression with the overlay's merge the rest; by time under the bound, the combined arm at five million keys under 640 MiB with the E and C windows cut by their marks (a perf script of that sitting, not kept): the decode is 35% of E and 37% of C, the group's open, seek and fence 13-14%. The probe builds the forced form at three hundred thousand keys in the suite's shape, reads every compressed page back and decodes each with the engine's decoder and with liblz4's `LZ4_decompress_safe`, held equal byte for byte: 7,230 bytes a page decoded from 4,040, 17,600 instructions a page against 14,400, and cycling the file's pages 3.4-3.8 µs a page against 2.8-3.4 (0.83-0.91). Decoding one page over and over reads 1.1-1.2 µs against 0.9-1.0, a third of the cycling figure, and the cache level is not what separates them: cycling 1, 4, 16, 64 and 256 pages of a file that fits L2 reads 0.7, 1.0, 2.0, 4.5 and 3.5 µs a page, the plateau the whole file reads at -- a page decoded over and over is one whose tokens the branch predictor has learned, so the decode of these records is misprediction-bound, nine nanoseconds a token cycling and three hot, and the cycling figure is a read's. A fifth of the decode is a fourteenth of C and a twentieth of E, inside what a pairing resolves on those cells, for a C library in the engine, so the decoder stayed `lz4_flex` (the engine's own decoder replaced it later, below). What the count points at is what a read decodes: a point read decodes the whole group, seven kilobytes, for a record of a hundred and twenty, and the memo saves the hot pages' decodes and nothing of a miss's size. The same probe cuts each group at record boundaries into k runs compressed apart (`--split`; the whole group recompressed is the page's payload byte for byte): four runs cost 6% of the form's bytes (0.590 of the records against 0.558) and take the point read's decode from 3.6 to 1.0 µs, eight cost 12% and take it to 0.55, sixteen 24% and 0.32, and the scan's decode of every run is level with the whole group's at every k. At five million keys under 640 MiB the C mix reads 2.16 µs an op against LMDB's 1.14 with the decode 37% of it, so four runs would take about a quarter off the op and eight a third, before anything a finer memo entry saves. That is the structure to build: the page a container of several groups of a target decoded size, one fence entry and one CRC a page and a directory of the groups inside it, so a point read decodes one group, a scan decodes the groups it walks, and the memo's entry is a group and not a page.

The page is a container of groups now (`src/db/zpages.rs`, the layout's `ZPG2`): one to sixteen groups, each compressed on its own, a directory after the header naming each group's first record index, its payload's place and length, its decoded length and, for every group but the first, the key of its first record, and the groups cut at `Options::zpages_group_bytes` of records (two kibibytes as shipped, four groups a page at the suite's shape; zero is the page as one group, the shape before). A point read finds the page by the fence as before and the group by the directory's keys, decodes that group alone and seeks in it; a scan decodes the groups it walks, the cursor stepping to the page's next group before the next page; the memo's entry is a group (`zfile::ZMemo`, four thousand and ninety-six slots under the same eight megabytes); the first-decode checks and the verified bit are a group's, with the page's CRC, header and directory checked at each group's first decode and the walk holding every key of a group below the next group's first; the builder verifies every group of every page it writes; the sampler prices the compact form's read by the decode of one group and a scan's by the groups it crosses; and the fill stages a group to the target, compresses it once, and trims only the page's last group to the room, where the one-group shape keeps its hint, its growth rounds and its turns. The tag and the magic moved (`ZPG2`, `SUPDZPG2`): a `ZPG1` reader would take the directory for the payload, so a form file of the old layout is refused at its trailer and rebuilt. `supdb-cache64tofitautozpagesonegroup` is the combined arm with the target at zero, `supdb-nocachezpagesonegroup` the forced arm with the same, and `supdb-cache64tofitautozpagesgroup1k` cuts at a kibibyte; the pairings at five million keys under 640 MiB are `pair.sh supdb-cache64tofitautozpagesonegroup supdb-cache64tofitautozpages 5000000 --reps 3 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` and the same of `supdb-cache64tofitautozpages` against `...group1k` at `e38e44b`, and of `supdb-nocachezpagesonegroup` against `supdb-nocachezpages` at `4e9cde9`. The memo's slot for a group is a Fibonacci hash of the group's number and the form file's identity over every slot (`Options::zpages_memo_hash`, on): the number alone, a page's groups taking sixteen of them, left a form file of the shipped cut a quarter of the slots, and the hash decodes 12-13% fewer groups in the Zipfian point-read mixes and 5% fewer in E's scans, the rates unresolved at five million keys under 640 MiB; `supdb-cache64tofitautozpagesmemolinear` is the shape before. Two levers for the shift to the form and for E were priced there and stay off as arms: the form files built ahead of the controller's decision once the store passes its budget and left dormant (`Options::zpages_prebuild`, `supdb-cache64tofitautozpagesprebuild`), which brought the install 4-5 s nearer the load's end and read the raw read pass at 0.65x, its builds reading the raw records from the device beside that pass's faults; and a block-cache scan's block taken from the fence and the page's directory with the rank sought only for a block not held as a copy (`Options::zpages_seek_span`, `ZFile::owner_span`, `supdb-cache64tofitautozpagesspan`), which cut E's decodes 7% and read level, the seeks it skipped being memo hits. The settle files a written key under the block the fence and the directory name, with its cut left to the build that reads the block (`Options::zpages_settle_span`, on; `supdb-cache64tofitautozpagesexactcut` resolves every cut at the settle through a seek, the shape before): E's first scan settles the backlog the mixes before it left, and the exact resolve was most of that settle, for blocks E walks wide, by key, without the cut; the settle reads about two fifths shorter and E decodes about an eighth fewer groups in every rep of both orders, the rate unresolved, and no other mix runs the settle. A verified page's directory is read trusted -- its header's bounds and where each group's key lies, none of the cross-checks between the directory's values that the first decode made once -- and a read takes a page's view once for the group it wants and the open of it (`Options::zpages_trusted_views`, on; `supdb-cache64tofitautozpagescheckedviews` and the forced `supdb-nocachezpagescheckedviews` are the shape before, every view checked); the settle's lazy filing of a sorted batch finds each key's fence entry from the one before (`ZFence::entry_of_from`). Profiled on the combined arm at five million keys under 640 MiB, the checked view was 8.5% of the E mix's main thread and 15% of D's, two or three views a group opened, beside the decode's 27%; by callgrind a point read through the form is 5,018 instructions trusted against 5,315 checked, the views 144 against 437. Asking the memo for a verified page's group before any view of the page, the entry carrying the group's first record and count so that a hit reads nothing of the page (`Options::zpages_memo_first`), is off: paired on with the trusted views in four sittings of two pairings, the pair read 1-4% slower in time on every mix for 2-6% fewer instructions, a cost the count does not show, read as the memo slot's miss serialized ahead of the page's where the view's work had overlapped them; `supdb-cache64tofitautozpagesmemofirst` is the arm. A block over a covered partition is read through one cursor held across it (`Options::zpages_block_cursor`, on; `db::PartReader` over `zfile::ZCursor`): a build or a walk reads the records between the overlay's keys by the cursor's walk, finds each key's cut where the walk stops at the key (`ZCursor::walk_until`) or, for a sparse build that reads no records, where the cursor moved to the key from the group it holds stands (`ZCursor::skip_to`, the fence searched from the entry held and the group kept while the key lies in it), and reads the record an overlay key replaces from the cursor standing on it, so a block opens each group it crosses once; the cuts of its memtable keys are left to it (`SnapBounds::cuts_of` and `BuildCtx::filed_cuts` resolve nothing under it), and a sparse block's walk orders the form's deltas against the memtables' keys by key, which is the order of their cuts. The shape before opened a cursor for every run between two keys, sought every cut from the fence (the cuts of a block's snapshot keys at the first build, the filed keys' at each) and read every replaced record by rank, each opening the key's group again, and sought the overlay key's cut at every turn of a sparse block's walk before the key was taken; profiled on the combined arm at five million keys under 640 MiB the cuts alone were 5.9% of the E mix's main thread. The reader's walk (`ZCursor::walk_run`) leaves a group it spent at its limit to the next read (`ZCursor::resume`), where the overlay walk's `walk_until` opens the next group at once and stands on its first record: opened at once, the next group was a decode for nobody at every run that ended on a group's last record, 0.7% of E's groups decoded in the first pairing, and the second read the decodes equal to within a group with the memo's asks a fifth fewer and the walks' opens nearly halved. `supdb-cache64tofitautozpagesperrun` is the shape before, with the walk's order by key; the pairings are `pair.sh supdb-cache64tofitautozpagesperrun supdb-cache64tofitautozpages 5000000 --reps 5 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `ca0878a`, two sittings. The memo's size is an option (`Options::zpages_memo_bytes`, eight mebibytes as shipped, `ZMemo::sized` scaling the slots with the cap; `supdb-cache64tofitautozpagesmemo4`, `...memo16` and `...memo32` are the other sizes): by count sixteen mebibytes decode 8% fewer groups in E and 12% fewer in C, thirty-two 14% and 16% fewer, with the point reads' major faults level, and paired at five million keys under 640 MiB sixteen moves neither E nor C and reads D slower in both orders, thirty-two reads C a tenth slower in both orders with E level, and four (`...memo4`) decodes 5.5% more in E and reads it slower in both orders; the memo pays for the groups that come back while still warm in the last-level cache and a hit on a cold group costs the walk its lines from memory, about what the decode would have, so eight is at the knee for this shape and stays, and the memo's size is not a lever on E. A group may hold its records one by one, each decodable alone (`Options::zpages_record_coding`; the layout's `ZCODING_RECORD`, the trailer's version 2, the coding each group's own in the high bit of its directory entry's decoded length, since a record alone whose head passes the page -- a long key, which LZ4 shrinks and a head never is -- is one block where only that holds it): a coded group's payload is an offset table, each record's head front-coded against the group's first record's and its tail LZ4-compressed against the first record's tail, which is stored raw as the dictionary, so a point read decodes the one record it wants (`zpages::decode_record`, the seek over the heads `zpages::record_group_seek`), a page's first touch checks a coded group by its heads (`zpages::check_coded_group`, `zfile::first_checks`) and a group decoded whole is its records decoded one after another into the same bytes the block coding decodes to (`zpages::decode_record_group`), which is how the verify, the memo and every reader of a decoded group read a coded group unchanged. It is an arm (`supdb-cache64tofitautozpagesrecord`, the forced `supdb-nocachezpagesrecord`) and the block coding ships (`ZCODING_BLOCK`, one LZ4 block a group): paired at five million keys under 640 MiB in two pairings of two sittings, the record coding reads the point-read mixes' gets at 0.69-0.84 of the block's time in every pair (C 0.69, B 0.70-0.74, D 0.74-0.78, A 0.72-0.80, F 0.83-0.84), every read decoding its one record where the block's reads hit the memo two fifths of the time and decoded a group otherwise, and reads E's scans at 1.22-1.26 in 23 pairs of 24, with the write mixes' commits 7-33% slower, because a scan's memo miss decodes the group whole and a group decoded through its records costs 2.2x the block decode's instructions by callgrind (a field parse, three small copies and a decoder call a record, against LZ4's bulk copies at seven instructions a byte), about a microsecond a group in E's conditions; a decoder shaped for the record was no faster than the library's and was taken out again. The form is 14% larger under it too. E being the cell the form has left to win, the coding that costs it is not the default. The probe that chose the coding and the dictionary is `bench/examples/zrec.rs` (`cargo run --release --example zrec -- 5000000 3 200000 zrec,supdb-nocachezpages,lmdb-nordahead,rocksdb-tuned` at `4a42a01`; `-- 1000000 2 100000 zrec,zrec-sampled,zrec-local,zrec-hybrid,zrec-nodict,zrec-raw,zrec-group,supdb-nocachezpages,lmdb-nordahead --payload suite`, and `--payload drift`, at `7f2a1ef`), and the coding's pairing is the one `Options::zpages_record_coding` names. A group's decode asks for the line of the memo entry its put will displace before it begins (`Options::zpages_memo_ahead`, on; `zfile::Memo::prefetch_victim`): the put tries the displaced group's `Arc` for its last reference, a load of a line last touched when that entry was made, and that miss was 2.9% of the E mix's thread at five million keys under 640 MiB; profiled arm against arm the recycle's samples go from 2.0% of the thread to 0.25%, and the pairing reads the memo-heavy mixes 3-6% for the hint in two thirds of the pairs and no cell against it; `supdb-cache64tofitautozpagesmemonoahead` is the shape before. The fence may be searched through a tree over the entries' words (`Options::zpages_fence_tree`, off; `zpages::ZFence::index`, built at the attach): a balanced tree in breadth-first lines of eight nodes, one word a level with the line three levels down asked for first, the whole key compared only where entries tie on the word, held to the search over the keys over every shape of fence and key. It is the arm (`supdb-cache64tofitautozpagesfencetree`): built for the fence's search at a twentieth of E's thread, read as a binary search missing on two lines a probe, it took more of the thread than the search it replaced, since a partitioned store's fences are small -- nineteen of some 4,900 entries at five million keys, a hundred kilobytes each, in the second-level cache for the partitions a Zipfian reads -- and a search of instructions and L2 hits has no misses for a tree to save. The search's instructions were then taken for the lever the tree had missed: over a partition's fence of some five thousand entries the search over the keys makes thirteen probes, each a line of the offsets and a line of the key blob and a branch the predictor mispredicts half the time, 7% of E's thread at ten million keys under 1.25 GiB (`perf record -e cpu-clock` cut to E's window, at `26db1fe`). The fence's words laid flat, eight bytes an entry, searched with no branch on the data (`ZFence::index_words`, the first shape), took the same share of the thread, and so did a static B-tree of the words, eight a node and a cache line a level, four dependent loads where the binary search makes thirteen (5-6% in profiles at `8f5aaf2`): the cost was not the search's loads but the key read after it, since the tie walk down from the first entry above by word read the entry's key to learn whether its word tied, whatever the search over the words had saved. The tree's descent sees the greatest word at or below the key's, so where no entry ties the word the entry before the first above is the answer and no key is read (`ZFence::above_by_word_wide`, the tie with the rank). Profiled arm against arm in one sitting the search took 3.9% of E's thread against 6.3% over the keys, E 1.05x and C 1.10x in the same rep (`perf record -e cpu-clock -F 499` over `SUPDB_MIX_KINDS=1 SUPDB_MIX_EVENTS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-fencekeys --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 1` at `016ee12`, cut to each arm's E window). The tree over the words is the default (`Options::zpages_fence_words`, on; `supdb-fencekeys` is the search over the keys), and the balanced tree stays the arm it was. A scan's start key may be kept with the block its partition holds it in, so that a scan starting where a scan started skips the partition's seek (`Options::scan_start_cache`, off; `db::StartCache`, a direct-mapped table keyed against the partition's blob identity, a key over sixteen bytes never kept, the hit's block walked as the span arm walks it): built for the E mix's Zipfian starts, half of them among four thousand keys at five million, it hit a fifth of the starts, since 63,000 puts a pass churn 4,096 slots, and every seek it skipped was a memo hit, the cheapest fifth -- in the arm pair's profile the seek's subtree lost 4% of its samples and the scan's own gained as much in the cache's lookups, the decodes unchanged and the pairing level; `supdb-cache64tofitautozpagesstartcache` is the arm. The settle's counts stand on the per-mix events line beside it (`Db::settle_density`, `Db::start_cache_counts`): E's first scans file the backlog the mixes before it leave, 43 settles over 49,000 keys at about a microsecond a key, no piece over 2% of E. Every group of a form may be stored verbatim, LZ4 never tried (`Options::zpages_store_all`, off): the stored group is a kind the format has had since the fill first met a group LZ4 did not shrink, read in place from the mapping with no copy and no memo, so a form of nothing else decodes nothing -- `tests/zpages.rs` holds it to the compressed form in every read, count and window and to zero decodes -- and at five million keys under 640 MiB it is about 620 MB of file where the LZ4 form is 360 and the process leaves 594: paired forced against forced (`supdb-nocachezpagesstored` against `supdb-nocachezpages`, two sittings) it reads E 1.275 (slower), C 1.354, D 2.831 and the scan pass 0.60x, with 0.31 major faults a scan in the scan pass and 0.06 in E where the LZ4 form takes none, since the tail's four percent of pages that do not fit go to the device. The arm stays as the record of the fit; `supdb-cache64tofitzpages` is the forced form with the combined arm's block cache and budget, the control a coding the controller's size model would refuse is priced against, and `supdb-cache64tofitzpagesstored` the same stored. The record coding's tails may be stored raw instead (`Options::zpages_raw_tails`, off, with `zpages_record_coding`): the heads front-coded as that coding has them, the values the records' own bytes, a decode a copy a record, about 108 bytes a record: forced against the LZ4 form at five million keys under 640 MiB it reads the point-read mixes 7-19% faster in both orders and E a tenth slower, and with the block cache beside it loses every cell on 0.016-0.03 major faults an operation, the cache's anonymous memory being the margin its 556 MB of file had; `supdb-nocachezpagesrecordraw` and `supdb-cache64tofitzpagesrecordraw` are the arms, off, for the bounds they fit. Whether the process's own anonymous memory was the margin it missed by was asked with the memo off and the allocator trimmed once at the end of `zpages_settle`, after every partition has its form file (`Options::zpages_trim_install`, off; `supdb-cache64tofitzpagesrecordrawlean` is the arm): paired against the LZ4 form with the same cache at five million keys under 640 MiB it still takes 0.02-0.03 major faults an operation in D and E and reads them 1.45-1.57x slower in every pair, the point-read mixes 1.3-1.45x faster by the decodes they no longer do, and the census the harness prints after each mix (`SUPDB_MIX_CENSUS`) reads the process's anonymous bytes level between the arms, since what a trim before the mixes returns the mixes' own allocations take back; the memory a form competes with is what the process holds during the reads, not after the build. The trim thread may be asked to look at the allocator at the end of every upkeep pass as well (`Options::upkeep_trim_look`, off; `supdb-cache64tofitzpagesrecordrawleantrim` is the arm), the stride deciding as it does for a shed: paired the same way the looks happened and the trims did not, since the stride counts the allocator's free bytes and the seal threads re-use the dead arenas' chunks, so their pages come back resident while the count stands still, and the faults and the process's anonymous bytes read as before. The stride may count the anonymous bytes resident beyond the allocator's use instead (`Options::trim_by_resident`, off; `supdb-cache64tofitzpagesrecordrawleanresident` is the arm), `RssAnon` less the allocator's in-use and mmapped bytes, which moves with the pages the free count cannot see: at a stride of eight megabytes it fired four to nine times through the mixes, and the process's anonymous bytes through them and the faults read as without it, since what a trim returns the mixes touch next; the pairings are `SUPDB_MIX_CENSUS=1 pair.sh supdb-cache64tofitzpages supdb-cache64tofitzpagesrecordrawleanresident 5000000 --reps 5 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` and the same of `lmdb` against `...leanresident`, which read E's faults equal to LMDB's, at `f4c5cb4`. Under `Auto` the coding is the selector's to choose (`Options::zpages_coding_auto`, on; `zselect::choose_coding`): the sampler sizes the raw-tail record coding beside the block coding, the raw tails are built where the model predicts that form faults nothing against `zselect::FORM_FILL` of the budget, 85%, the rest left to the page cache's other tenants, and the block coding otherwise, and each form file names its own coding, so a store may carry both. The line was the budget itself first, and a pick holds for the builds after it while the budget moves -- at ten million keys under 1.25 GiB it reads 1.2 GB at the load's end and 0.87 through E, as the mixes' own memory grows -- so a form that fits at the sample is one the mixes may not hold: there the raw-tail form sat at 97-99% of the budget, the sampler counted 28 partitions with one in flight in one pass of four and 29 in the other three (1168 MB against a budget of 1199, and 1190 against 1198), the pick flipped with the count, and the pass that bought the raw tails built 29 files at the larger coding over the block-coded ones the builds ahead had left and refaulted 4,000-28,000 pages a second through every mix where the other three refaulted nothing, C 8.8 s against 3.2-3.4 and E 13.3 against 5.9-6.7, the table's tell `zpage_builds_raw_tails` 29 against 0 (`SUPDB_MIX_KINDS=1 SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms supdb,supdb-mergeahead --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2` at `cb4f02a`, two arms that differ in nothing the controller reads, the fourth pass's `zsample` line against the first three). The fill share is the re-code policy's line, one constant for both, and it picks the block coding at both counts there (paired again with the same command at `a12f8a2`: 29 partitions sampled in three passes and 28 in the fourth, the block coding picked in all four, `zpage_builds_raw_tails` 0 in both arms, and C 3.0-3.8 s, D 3.9-4.7 and E 5.7-6.1 in every pass, one shape); it also gives up the raw tails at 736 MiB below, where that form was 91% of the budget and read 1.2-7.7x -- at five million keys they are picked from a limit of about 790 MiB now -- since the three points measured, a win at 91%, a wash at 94% and a loss of 2-2.6x at 97-99%, put the line's cost under its saving. The window is the budgets between the raw-tail form's bytes and the raw store's, about a seventh of the store at the suite's shape: forced where both fit (three million keys under 640 MiB) the raw-tail form reads the point-read mixes 1.25-1.95x faster than the block form and the scan pass at 0.70x, since a scan decodes a raw-tail group as a parse and a copy a record; the gear picks the block coding at five million under 640 MiB, where the raw-tail form would fault, and the raw tails at 736 MiB, where the point-read mixes read 1.2-2.4x faster than the options' coding; with the in-place walk and the seek over the heads below, the raw-tail form reads every mix at or above the block form's where both fit, so the selector's choice ships and `supdb-cache64tofitautozpagescodingfixed`, the options' coding wherever the controller builds, is the shape before: paired against it at five million keys the default reads level at 640 MiB, where both build the block coding, picks the raw tails at the 720 MiB edge in every rep, where they fault a little in D and E (0.01-0.02 an operation) and the cells split by sitting with nothing resolved against it, and reads 1.2-7.7x at 736 MiB, where it also buys earlier; the pairings are `pair.sh supdb-cache64tofitautozpagescodingfixed supdb-cache64tofitautozpages 5000000 --reps 5 --bound N --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` for N 671088640, 754974720 and 771751936, at `04f1808`. A raw-tail coded group is read where it lies by both scan walks, the block cache's cursor and the form file's bulk scan (`Options::zpages_inplace_walk`, on; `zfile::Group::Coded`): the position is a record index, the key is assembled from the group's first head and the record's suffix, and the extents and the inline value are parsed from the raw tail in the page (`flatindex::parse_record_tail`, the half of `parse_record` past the key), with a record whose tail is compressed or whose extents cannot be read where they lie decoded on its own and nothing put in the memo. It took the scan pass under the raw-tail form from 0.70x of the block form's to 1.13-1.21x where both fit, with E level, so the raw-tail form reads every cell at or above the block form's where it fits; `supdb-cache64tofitzpagesrecordrawdecode` is the shape before. The coding may differ by partition (`Options::zpages_raw_tails_first`, off; `supdb-cache64tofitzpageshot4` and `...hot1` are the arms): the first partitions by key order raw-tailed and the rest block-coded, the upper bound of a coding chosen by heat, which `Db::zpage_heat` reads as each form file's decodes; at five million keys under 640 MiB the first partition holds half the E mix's decodes and the first four three quarters, and raw tails on four read the Zipfian mixes 12-21% faster and the uniform read pass a fifth slower past the fit, on one the point-read mixes 10-15% faster with E level, since that partition's decodes run warm. Once a form file serves a partition, the partition's raw file and its ordered index are dropped from the page cache (`Options::zpages_drop_raw`, on; `Seg::drop_raw_pages`): the store's own mappings of them unmapped (`MADV_DONTNEED`) and then the files' clean pages dropped (`POSIX_FADV_DONTNEED`), since a cache drop skips a page a mapping holds and the open's check of the key section's checksum row maps all of it. Under `Auto` the drop waits for the gate's turn to the form and drops every attached partition's at once (`Options::zpages_drop_at_install`, on; `supdb-cache64tofitautozpagesdropattach` drops at each attach), since an attach under the census gate leaves the reads raw; paired at five million keys under 640 MiB the combined arm with the drop reads the mixes after the install 1.2-1.7x of the arm without it, the passes before the install level. The build drops the records behind it already, and what it left was the key section, which the raw point read probes, and the ordered index, 85 and 38 MiB beside the LZ4 form's 373 at five million keys under 640 MiB by `mincore`, touched by no read through a form; with them gone the form four partitions raw-tailed faults nothing where it faulted 0.013 a read, and reads every cell at or above the LZ4 form's, the read pass 1.07-1.10x. A raw read after it, a merge's or the controller's trial, reads them back from the device, so it is priced on the forced arms (`supdb-cache64tofitzpagesdrop`, `supdb-cache64tofitzpageshot4drop`); the pairings are `SUPDB_MIX_CENSUS=1 pair.sh supdb-cache64tofitzpages A 5000000 --reps 5 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` at `90530a9`, A each of the two. A served partition's form file may be built again in the other coding and swapped into its cell while the store serves (`Options::zpages_recode`, off, the forced form; `ZCell::replace`, the old form file retired under the epoch): once a second the segment work reads each form file's decode time, one decode in sixteen timed (`ZFile::decode_ns16`), and a raw-tailed one's group opens priced at a decode (`ZFile::opens16`), into a heat decayed over ten seconds, re-codes the hottest block-coded partition to the raw tails while the form files' bytes stay within the platform's budget, gives the coldest raw-tailed one back to make room for one twice as hot, and gives the coldest back over fifty refaults a second, holding the bytes there for a minute; a form file reads its coding from its own directories (`ZFile::coded`). At five million keys under 640 MiB it fills the budget to about 480 MiB of form without a fault, but on the uniform read pass, where every partition decodes alike, so it reads level with the drop arm and behind the key-order arms in the Zipfian mixes; a build during a mix costs the durable commits the device's flush of its bytes; at ten times the operations the mixes grow the process past the forms' room and the policy gives its raw tails back, which reads D and E a tenth to a quarter faster than the key-order arm that keeps them, and B and C slower. `supdb-cache64tofitzpagesrecodedrop` is the arm. The form file's seek -- a scan's start and a cut -- seeks a record-coded group over its heads as the point read does (`record_group_seek`), nothing decoded and nothing put in the memo: it had opened the group with the decode and sought the decoded bytes, a whole-group copy-decode the in-place walk never used, 1.5 a scan in E; with it the raw-tail form reads every mix faster than the block form where both fit, E by a fifth: `SUPDB_MIX_CENSUS=1 pair.sh supdb-cache64tofitzpages supdb-cache64tofitzpagesrecordrawlean 5000000 --reps 5 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10`, and the same against `...recordraw` at 3000000, at `907db3f`. Two probes beside them are kept as the record of what a cold decode costs (`bench/examples/zlines.rs`, `zdecode --hc`): the lines a decode reads, asked for ahead at the view, at the fence or the whole page, buy a tenth of a cold decode at most, because a decoder on input its predictor has not learned stalls on its branches and not its loads; and liblz4's high-compression mode, fewer tokens for the same format, takes 16% off the cold decode and 4% off the form at level 9, with liblz4's own decoder another 14-16% faster. The encoder's half is the engine's own now, in Rust (`Options::zpages_parse`, off; `db::zlz4`): a hash-chain search and a lazy parse, the longest match at a position taken unless the next position starts a longer one, the positions a match covers inserted and not searched, which writes 0.960 of the greedy encoder's bytes in 51.9 sequences a group against 62.1 at 10.6 µs a group against `lz4_flex`'s 3 -- liblz4's high-compression mode writes a tenth of a percent less at level 9 in about 16.5 and a sixth of a percent less at level 12 in 41 -- and the shortest path over every position it replaced (`Effort { lazy: 0 }`) wrote the same bytes in twice the time. Paired at five million keys under 640 MiB on the forced form it reads the point and scan passes 7-9% faster in both orders and every mix's reads 5-10% faster by kind; on the combined arm its build, at twice the greedy encoder's CPU, is bought six seconds later and runs fifteen longer, and the install twenty seconds late costs the mixes after it more than the better groups save, so it is an arm (`supdb-cache64tofitzpagesdropparse`, `supdb-cache64tofitautozpagesparse`). The decoder's half is the engine's own as well and ships (`Options::zpages_own_decoder`, on; `zlz4::decompress_in`): liblz4's fast loop in Rust, a run of fewer than fifteen literals one move of sixteen bytes and a long one fixed moves to sixty-four, a short match moves of eight, eight and two and a long one moves of sixteen, a group with sixty-four bytes of its page after it run with no input bound but its block's, and the rare long run and near match out of line so the fast turn keeps its registers. It decodes a form group in 3,355 instructions against liblz4's 3,438 and `lz4_flex`'s 4,112 and cold in 0.84-0.88 of `lz4_flex`'s time; Miri finds nothing in it over its tests' damaged blocks; a form file takes it at its attach, so each store decodes by its own options; and paired at five million keys under 640 MiB no cell reads slower in both orders, with E 1.00 on the forced form and 1.06 on the combined arm by the two orders' mean, where the first version, 4,460 instructions a group, had read the combined arm's E at 0.88 and 0.93. A turn with no branch on a length, the run moved sixty-four bytes and the match forty-eight whatever their lengths, took the branch model's misses away and moved no cold figure, and was taken out. `supdb-cache64tofitzpagesdropflexdec` and `supdb-cache64tofitautozpagesflexdec` are the arms with `lz4_flex`; the probe is `cargo run --release --features lz4-c --example zdecode -- --size 1000000 --rounds 11` and the pairings `pair.sh supdb-cache64tofitzpagesdropflexdec supdb-cache64tofitzpagesdrop 5000000 --reps 5 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` and the same of `supdb-cache64tofitautozpagesflexdec` against `supdb-cache64tofitautozpages`, at `16b6893`.

The form file a re-code replaced may be kept, attached and unserved, beside the one it serves (`Options::zpages_recode_keep`, off; `Seg::z_spare`, `ZCell::flip`): a partition's re-codes after its first are then the two files exchanged in the cell, the incoming one's pages asked for in windows of the device's largest request (`ZFile::prefetch_all`) and the outgoing one's dropped from the cache through its own handle (`ZFile::drop_cache`), since a kept file's name is gone once the build's rename replaced it -- no build, no bytes written, no device flush for the durable commits beside it. A reopen serves whichever coding holds the name and keeps nothing; the kept file goes with the segment, or with the served one where a check retires it. `tests/zpages.rs` holds the exchange to no build and every read and scan right across it; `supdb-cache64tofitzpagesrecodekeepdrop` is the arm. Both codings may be stored from write time (`Options::zpages_both`, off; `ZKind::Spare`): once the served form files are in, the builder builds the other coding of every partition, the hottest first, and keeps it unserved with its pages dropped, and the policy then only exchanges files -- the raw-tailed set chosen whole each look among the hottest partitions whose raw tails fit the budget, a served raw-tailed partition counted twice against the order and the floor, at most 32 MiB of incoming pages a look, a look every 200 ms beside a build in flight, a heat that rises to the rate a partition shows at once and fades over ten seconds, and nothing moved while nothing reads; under refaults a flip asks for none of the incoming file's pages. At five million keys under 640 MiB it reads E 1.04, D 1.08 and B 1.07 against the LZ4 form with the drop and A 0.92: nothing before the suite's first Zipfian mix says where its head is, so the policy pays one mix of flips, each a prefetch on the device the durable commits sync to; at ten times the operations it reads C 1.10, B 1.14, A 1.08 and the read pass 1.08-1.12, and F 0.95 and E 0.97 where both arms fault; `supdb-cache64tofitzpagesbothdrop` is the arm.

Out of core the suite's ycsb-E inherits the update mixes' writes: their commits file nothing into the forms, since no scan runs between them and their writes are under the backlog bound, and E's first scans file the 49,000 keys and build the blocks over them, a sixth of its thread at five million keys under 640 MiB -- the work a B-tree does at the write. A commit near reads may file its batch whatever the backlog (`Options::forms_settle_every`, off when this was priced and on since `f33f68b`, below) and its fill build the blocks the settle filed into that held no form (`Options::forms_settle_builds`, the same; the fill's own build of every overlaid block runs only without a cache budget). The filing alone moved nothing E reads -- a settle patches only the blocks a scan has formed -- and cost F and D a tenth; with the builds, paired against the forced form with the first eight partitions raw-tailed and the raw pages dropped in four sittings, E read 1.07-1.17x in every one with every other cell level or faster and nothing faulting. `supdb-cache64tofitzpageshot8dropsettleevery` and `supdb-cache64tofitzpageshot8dropsettlebuilds` are the arms, and `tests/settle.rs` holds each to its effect; the pairings are `pair.sh supdb-cache64tofitzpageshot8dropsettleevery supdb-cache64tofitzpageshot8drop 5000000 --reps 5 --bound 671088640 --passes read,read-threaded,scan,scan-threaded,mixes --ops-div 10` and the same of `...settlebuilds`, each run twice, at `16679e4`. On the combined arm, which stands at its fit line, the forms the commits build cost D a quarter on major faults, since they hold the memory from the write to E's scans; the fill builds only while the memory group's file refaults hold still and sheds what it built unwalked when they move (`SettleRoom`), which brings D to 0.86 and no further, because a shed form's memory stays in the allocator's arena -- a build at the write has to be decided before the memory is taken. So the fill builds only while the budget the bound leaves exceeds what the reads keep resident -- the served form files, and the key sections and ordered indexes of partitions whose raw pages are not dropped -- and with no room a commit files only what the backlog bound asks, since the filing alone costs F and D a tenth: gated so, the combined arm reads every cell level, nothing faulting, and hot8 with the drop reads E 1.07x in both sittings. In core nothing resolved in five pairs a sitting (`supdb-settlebuilds`, `supdb-ingestsettlebuilds`, `supdb-cache64tofitautozpagessettlebuilds` are the arms).

Both are the default now, and `supdb-settlelate` and `supdb-ingestsettlelate` are the shape that leaves a batch under the backlog bound to the scans. What decided it was the scrambled mixes the row has run since: the scrambled Zipfian's writes fall over the whole key range, so the scrambled write mixes leave tens of thousands of scattered keys unfiled where the unscrambled ones leave the head of the store, and the scrambled E after them settled and built over all of them in its first scans. Paired in the row's order under 1.25 GiB at `0e3f284`, where the default was still the late shape and the arm `supdb-settlebuilds` (`bench ab --arms supdb,supdb-settlebuilds --size 10000000 --bound 1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled --ops-div 10 --reps 3`, the arms the other way, and both orders at `--size 3000000 --reps 4`), the scrambled E read 1.33x and 1.20x at ten million keys and 1.56x and 1.48x at three, in every pair of the four sittings, and E 1.13x, 1.05x, 1.13x and 1.29x. The cells that gave ground are the ones whose commits now do the filing: the scrambled A read 0.81-0.86x in all four sittings, and with the lag sweep in the order (the same passes and `lag`, `--reps 3`, three sittings) the one-percent burst at ten million keys, which follows a pass of scans, read 0.68x and the pass after it 0.86x in the one sitting at that size, three pairs of three, with the other lag cells in no direction. The row at `d8cece6` has the nearer comparator at a third of the scrambled A at three million keys and under a fifth at ten, and at under a third of that burst; the pass after the burst it has behind LMDB already, at 0.81x in three reps that read 0.51-1.76x, and that sitting's 0.86x is the one reading of the change there. In core nothing resolved (`bench ab --arms supdb,supdb-settlelate --size N --reps 12` at `f33f68b`, ten thousand keys, a hundred thousand in both orders, three hundred thousand); the scrambled D read 0.87-0.93x in all four sittings and the scrambled E 0.84-0.97x, which is open.

The overlay was priced before anything was built for it, by a probe's step in the mix driver (`SUPDB_MIX_FOLD=E`, `Engine::fold`): before E and outside its window, the memtable sealed, the merges joined and the controller left to cover the rewritten partitions again. Paired against LMDB at ten million keys under 1.25 GiB at `d94690a` (`SUPDB_MIX_FOLD=E SUPDB_MIX_KINDS=1 bench ab --arms supdb,lmdb-nordahead --size 10000000 --bound 1342177280 --passes load,mixes --ops-div 1 --reps 2`, and the same with the variable unset), the fold took 29 s -- the write mixes touch every partition, so its merge took all twenty-nine and 87 pieces as inputs and the controller built every form file again -- and E then read 0.76x of LMDB where the sitting without it read 0.95x, E's scans 8.2-8.8 s against 6.8-7.6, with no major fault in either: E over a store just rewritten pays in CPU what its forms and caches had held, more than the overlay it no longer walks. The overlay is not E's gap to fold away. The same sitting read the other cells against LMDB on this host at A 5.9x, F 4.8x, B 1.9x, D 1.18x and C 1.04x, the ordered load 1.23x and the shuffled 12.7x.

`ZPages::Always` forces it (the suite's `supdb-nocachezpages`; its `sync` calls `Db::zpages_settle`, which builds the missing form files and waits). The counters it keeps are the `zpage_*` rows `bench ab` prints: reads, decodes, raw serves, fallbacks, damage and identity refusals, builds, declines by reason, and the form files' size at the end of a pass.

The first sitting at ten million keys under a 1 GiB bound, the arm with no block cache against the same arm with the form forced, twelve pairs in three chunks of four after their own warmups (the order alternates within each), the read passes at a tenth of the operations:

| | raw | forced | |
|---|---|---|---|
| faults a point read, 1 / 2 / 3 threads | 0.45 / 0.41 / 0.41 | 0.000 / 0.000 / 0.000 | 12 of 12 pairs |
| bytes read from the device a read, 1 thread | 1,852 | 0 | 12 of 12 |
| point reads a second, 1 / 2 / 3 threads | 31k / 60k / 77k | 196k / 419k / 614k | 6.4x / 7.0x / 8.0x, 12 of 12 |
| p99 of a point read | 163 µs | 11 µs | |
| ordered load, ops a second | 936k | 523k | 0.56x: the build is inside `sync` |
| shuffled load, ops a second | 218k | 193k | 0.89x, 3 of 12 above |
| bytes on disk a byte stored | 1.30 | 1.91 | |

The raw arm's rate in that sitting is below the 54k a second it read when the rung was first banked, and the forced arm does not wait on the device, so the ratio moves with the host and the faults do not; the comparison with LMDB is not made here, an arm pair holds two arms. Every partition had its form file, and no read fell to the raw form or failed a check.

The second sitting, the arms' order swapped, twelve pairs after a warmup in one run of forty-one minutes:

| | raw | forced | |
|---|---|---|---|
| faults a point read, 1 / 2 / 3 threads | 0.47 / 0.43 / 0.43 | 0.000 / 0.000 / 0.000 | 12 of 12 pairs |
| bytes read from the device a read, 1 / 2 / 3 threads | 1,933 / 1,762 / 1,760 | 0 / 0 / 0 | 12 of 12 |
| point reads a second, 1 / 2 / 3 threads | 45k / 85k / 112k | 198k / 422k / 610k | 4.4x / 5.0x / 5.5x, 12 of 12 |
| p99 of a point read | 109 µs | 10 µs | |
| ordered load, ops a second | 1,158k | 678k | 0.59x |
| shuffled load, ops a second | 311k | 260k | 0.84x, 9 of 12 |
| bytes on disk a byte stored | 1.30 | 1.91 | |
| bytes written to the device a byte stored | 1.34 | 1.95 | |

Fifty-seven partitions in each pass, fifty-seven form files built and attached, 343,000 pages holding 2.48 GB of records in 1.42 GB of file; six million reads through the form, every one decoded, none raw, none fallen back, nothing declined, refused or swept. The forced arm reads at the same rate in both sittings, 196k and 198k a second on one thread, and the raw arm at 31k and 45k: the ratio is the host's that day, the faults a read are the form's and read zero in every pair of both sittings. The load and the disk read where they were registered (`docs/zpages-design.md`, section 7.3, with the ranges corrected before the run: the ordered load 0.40-0.60x, the shuffled load 0.65-0.85x, about 1.89 bytes on disk and 1.92 written a byte stored; `pair.sh supdb-nocache supdb-nocachezpages 10000000 --reps 12 --bound 1073741824 --passes load,read,read-threaded --ops-div 10`): the ordered load at 0.59x, the shuffled load at 0.84x, bytes on disk at 1.91 and device bytes at 1.95 a byte stored. What the verdict leaves to the bounded row is the comparison with LMDB and the scan cells, which stay raw until the form serves scans. One chunk of the sitting died in the shuffled load's last `sync` with `merge slices leave a key unassigned`, the merge's check that its output slices tile the keys of its inputs. The forced arm's `sync` waits for the segment work and so returns that error; the raw arm's flush returns before the merges it leaves finish, and an error from them at the store's close is dropped with the store, so the pairing cannot say whether the raw arm meets it too. The chunk was run again.

In memory the forced form is the wrong choice. At three million keys, which the bound holds, with no division of the reads and eight pairs, it read at 0.21x, 0.18x and 0.17x of the raw form's rate on one, two and three threads, a read of 4.5 µs where the raw one is 0.9, for the page decode in every read where the raw read walks a resident hash; its p99 is 9.5 µs against 1.6. It is an arm for the bound, and something has to decide between the forms per store (`src/db/zselect.rs` is the controller's pure core).

A form file written in pieces of a megabyte (`ZWRITE_PIECE`) sits in the page cache as one megabyte folio per megabyte written and a ladder in the tail, in 64 KB pieces as 64 KB folios, in 4 KB pieces as single pages; after eviction a random read through a `MADV_RANDOM` mapping brings back folios of one page whatever the write was. So the composition of what is resident changes during a read pass, and the faults a read are a property of the cache's state as well as of the form. Counted, not timed, on a 400,000-key store with 8 MiB partitions.

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

It found one thing before it held. A seal then rotated to a fresh WAL (seals keep the WAL now and the log rotates by size; `Options::seal_rotates_wal` is the arm that rotates at every seal) whose
eight-byte header is written and not synced until the first commit into
it, and replay refused a WAL shorter than its magic -- so a power loss in
that window left a store that would not open. A prefix of the magic is
now an empty WAL, `open` truncates and rewrites it, and the seal fsyncs
the directory as soon as the new WAL exists, since commits into it are
acknowledged from then on and an fdatasync of a file does not promise
the entry that names it. One directory barrier per seal, off the
per-commit path.

The comparators' check, registered before its first run. The suite's comparator arms are configured and driven by the same hand that wrote the engine, and the suite's own history has a comparator wrong for months while every check passed (the buffered RocksDB as it ships, `bench/DESIGN.md`). A job of the repository's CI (`comparators.yml`, by hand and weekly) holds the suite's comparator cells to the comparators' own tools on one hosted runner in one job: RocksDB's db_bench built from the very tree the suite links -- the source `librocksdb-sys` vendors at the version `bench/Cargo.lock` pins, with no compression library, as the suite's build has none -- and run with the tuned arm's options (compression off, a 256 MB block cache with the index and filter blocks in it, ten Bloom bits a key, 128 MB write buffers four deep merged two at a time, level 0 compacting at eight files, four background jobs), the load in synced batches of a thousand, N uniform point reads on one thread and seeks with ninety-nine nexts for the suite's scans of a hundred entries; and, since LMDB ships no benchmark tool, a C program of a hundred lines against the LMDB `lmdb-master-sys` vendors (`bench/tools/lmdb_bench.c`), the suite's shapes in its own doc, with one read transaction held across the reads as the suite's arm holds its own. Beside them the suite's `rocksdb-tuned` and `lmdb` arms run the quick ladder in the same job, and `bench/scripts/comparators.py` prints, at every rung, the suite's load, point-read and scan cell beside the tool's. The rule: the suite's cell is at least 0.7x the tool's on every quantity the tool measures, or the job fails and names the quantity. A cell under the margin is a comparator the suite configures or drives badly -- the finding is the arm's, to fix and bank a row for -- and never a reading of the engine; a cell far above the tool's is the tool driven badly, printed and not failed. The tools' flags live in `bench/scripts/comparators.sh`, so a reader can run both halves. The margin the cells hold is read off the job's summary once it has run, and `bench/DESIGN.md` states it.

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
- ~~Recycling WAL files~~ — **the default** (`Options::recycle_wal`;
  `supdb-walappend` is the log grown by its appends). An fdatasync into
  blocks already allocated and written carries no inode change through the
  journal. The first measurement priced it on the ordered and uniform
  loads, whose commits are few and large, and the pool's one-time
  pre-write paid back what they saved. The write mixes are where the
  commits are: paired in the row's whole order under 1.25 GiB
  (`SUPDB_MIX_KINDS=1 SUPDB_LAG_COUNTERS=1 bench ab --arms
  supdb,supdb-walappend --size 3000000 --bound 1342177280 --passes
  load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag
  --ops-div 10 --reps 2` at `91bf79f`, both orders, and at `--size 10000000`
  with one rep each way), the commits of A and F and their scrambled forms
  took 0.26-0.48 ms against 0.48-0.72 at three million keys and 0.33-0.42
  against 0.47-0.69 at ten, and those mixes read 1.1-2.1x, B and D
  1.06-1.16x and the shuffled load 1.03-1.14x; the lag passes, the reads
  and the scans sat within their sittings' spread both ways, and ycsb-C,
  which commits once, read 0.81-1.09x with its gets level over four reps in
  the mixes' own order. Three things stood between the first shape and that.
  A recycled file is overwritten in place, and a commit's last page,
  written in part into a page the cache no longer holds, is read from the
  device first, on the commit's path: every write into a pre-written or
  recycled file is padded in zeros to its page's end now, and the next
  write overwrites them from the true tail, so the log is as long as
  before and no page is read back (`Options::wal_recycle_pads`). The pair
  the store pre-wrote, the live file and a spare of 32 MiB each, was 64 MiB
  of page cache under a bound if kept resident, and a device read under
  the commits if dropped and asked back ahead; a file pre-written whole has
  its pages dropped once written, and one pre-written ahead holds a
  mebibyte of them at most. And the pre-write was 64 MiB written and synced to
  create a store, 32 MiB at rest in every store however little it held,
  and as many gigabytes as a test run makes stores: a file not yet written
  through is pre-written a mebibyte past its tail as the commits reach it
  now (ahead of which commits is below), the zeros flushed by the next
  commit's own fdatasync with one journal commit for the growth, and no
  spare is made until a retired file is one (`Options::wal_ahead_bytes`;
  zero is the whole-file pre-write, `supdb-walprefill`). Paired against
  the whole file on the mixes alone
  (`--arms supdb,supdb-walprefill --size 3000000 --bound 1342177280
  --passes load,mixes --ops-div 10 --reps 3`), A's commits took 0.28-0.32
  ms against 0.18-0.30 and F's 0.25-0.37 against 0.30-0.32, A 1.11x for the
  whole file in two reps of three and F level: most of what the whole file
  saved, for none of its footprint. A store whose commits do not sync
  recycles its retired files and pre-writes nothing, and a reopened store
  pre-writes as a created one does. The measurement trap the first run
  found still holds: pre-written in 1 MB pieces the file left 1 MB
  page-cache folios, and every 100 KB commit wrote a megabyte back — 2.2x
  the device bytes, 11.2x in isolation; it is pre-written in pages.
  The mebibyte went in ahead of every commit from a file's first, priced
  on the mixes at three and ten million keys, and was wrong at both ends
  of a commit's size. The quick row after it read ycsb-B at ten thousand
  keys at half of LMDB's, a cell every row before had at 1.5-1.7x: B there
  is one commit in a pass of a millisecond and a half, and it wrote and
  flushed the megabyte the mixes after it rode. Paired in core against the
  appended log (`bench ab --arms supdb,supdb-walappend --size 10000 --reps
  12`, both orders, and `--size 30000` and `300000`, at `f33f68b`), B read
  0.32-0.40x at ten thousand keys and 0.57x at thirty; the shuffled load
  0.73-0.83x and still 0.91x at three hundred thousand; the full rewrite's
  burst 0.76-0.86x; and A 1.27-1.60x and F 1.15-1.4x. The device's prices
  say why (`cargo run --release --example walsync` in `bench/`, at
  `f50d31e`, two sittings an hour apart: six mebibytes of commits of one
  size, appended against pre-written a megabyte at a step). The
  pre-written log read 1.4-1.8x of the appended at four to twenty-four
  kilobytes a commit, 1.1-1.3x at thirty-two and forty, 0.96x and 1.20x at
  forty-eight, 0.92-0.97x at fifty-six and sixty-four, and 0.59-0.85x from
  eighty up. A megabyte of zeros, written and flushed, adds 2.3-2.8 ms to a
  megabyte of a load's commits; a commit that lands in pre-written pages
  saves about a tenth of a millisecond, the journal commit its append would
  force; a megabyte pre-written in four steps costs 0.6-2.2 ms more than in
  one; and an append of a hundred kilobytes, which the kernel takes in
  large folios, syncs for less than the same bytes laid over page-sized
  zeros. The suite's write mixes commit a hundred writes, 16.8 KB, and its
  loads and bursts a thousand, 133-167 KB (`strace -f -y -e
  trace=pwrite64,fdatasync` over `bench ab`). Two shapes were built and
  paired before those prices
  were read, and each moved the megabyte without taking it away: a reach as
  long as the file (B 2.4-2.8x of the first shape at ten thousand keys, A
  0.81x, F 0.61-0.70x, the one-percent burst 0.28-0.33x, the shuffled load
  still 0.78-0.84x of the appended log), and steps doubling from 64 KiB
  for commits within 32 KiB (A 0.77x and F 0.57-0.64x of the first shape, B
  and the one-percent burst 0.79-0.96x of the appended log for a first step
  nothing after them used). At that rung no schedule wins: the mixes write
  two thirds of a megabyte in forty commits, the journal commits they could
  save are four milliseconds at most and the megabyte of zeros to hold
  them two and a half with its extensions beside, so a schedule only
  chooses the cell that pays. What ships
  (`Options::wal_ahead_small_runs`; `supdb-walaheadflat` is the first
  shape): a commit over 32 KiB appends, pre-writes nothing and ends the
  run; a commit within it counts toward a run of small commits, and where
  it does not fit what is pre-written it pre-writes the reach once the run
  has written a reach itself; a run goes on across a rotation. Paired in
  core at `f50d31e` (`bench ab --arms supdb,supdb-walappend --size N --reps
  12` and `--arms supdb,supdb-walaheadflat`, both orders at ten and thirty
  thousand keys, one sitting of ten reps at a hundred thousand and of eight
  at three hundred thousand): against the appended log nothing resolved at
  ten thousand keys, where the two write the same bytes; at thirty thousand
  the scrambled F, the first mix past the run's megabyte, read 1.54x and
  1.80x; at a hundred thousand A read 1.27x, D 1.25x, F 1.76x and the
  scrambled A and F 1.48x and 1.27x, nine and ten pairs of ten; at three
  hundred thousand A, F and their scrambled forms 1.23-1.33x. Against the
  first shape B read 2.8x at ten thousand keys in both orders and 1.6-1.9x
  at thirty, the shuffled load 1.16-1.39x up to a hundred thousand and the
  full rewrite's burst 1.10-1.25x, and A read 0.64-0.78x up to thirty
  thousand and 0.50x at a hundred thousand, where the megabyte the run
  appends as its evidence is most of the mix: the price of buying on
  evidence, which the first shape charged to B. Under 1.25 GiB in the row's
  whole order (`SUPDB_LAG_MARK=1 SUPDB_ZPAGES_TRACE=1 bench ab --arms
  supdb,supdb-walaheadflat --size 3000000 --bound 1342177280 --passes
  load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled,lag
  --ops-div 10 --reps 3`, the arms the other way, both at `--size
  10000000`, and `--arms supdb,supdb-walappend` at three million keys with
  three reps and at ten with two): against the first shape the shuffled
  load read 1.09x in both orders at three million keys and 1.02-1.04x at
  ten, the bursts 1.07-1.27x at ten in five cells of six, and the mixes
  within their sittings' spread; against the appended log A, F and their
  scrambled forms read 1.24-1.38x at three million keys in every pair, and
  1.20-1.43x at ten, and the shuffled load 1.21x and 1.10x, since a load
  rotates into the files it retired. The passes after the one-percent and
  the ten-percent bursts at three million keys are not resolved between
  the shapes: after the one-percent burst they read slower than the first
  shape's in the row's order, 21 ms against 17 in six pairs of six, and
  faster in three pairs of four of the lag sweep alone
  (`SUPDB_LAG_COUNTERS=1 SUPDB_SCAN_OPS=1` and `--passes load,lag --reps
  4`), and the ten-percent pass reads two shapes under either, 13-19 ms
  or 20-41, in seventeen and fourteen passes whose counters agree. What
  makes the two shapes is not the log. Paired against `supdb-noahead`
  (`--arms supdb,supdb-noahead --passes load,lag --reps 6` with the two
  variables, both orders, at `f50d31e`) the ten-percent pass read 13-20 ms
  without the builder ahead in twelve passes of twelve and 20-32 with it
  in seven of twelve: the builder's forms are installed in one step of 3-7
  ms inside a pass of thirteen that visits a ninth of them, and in some
  reps of both arms the first scan waits 2-9 ms for the upkeep's pass in
  flight. Both are open.
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
