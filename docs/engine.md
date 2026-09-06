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
  order does not qualify and sits near 0.42x. The transactions axis is
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
- **What the on-disk size ordering becomes** — segments plus a WAL will
  not beat LMDB on disk; that loss stands and gets re-priced honestly.

## What this does not promise

Multi-reader snapshots (MVCC beyond the single-writer borrow), or beating
LMDB out-of-core. Transactions it does promise now -- atomic batches,
rollback, read-your-writes -- and deletes that reclaim their bytes. The
guarantee set stays what `Features` can equalize, so every comparison
against another engine remains matched: the durable-commit axis is
equalizable in both directions, and the transactions axis no longer leaves
a residual on the engine's side.
