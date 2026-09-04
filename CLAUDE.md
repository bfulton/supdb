# Working in this repository

Supdb is an embedded key-multivalue store, and a benchmark suite whose job is
to try to falsify the claims made about it. The two are meant to stay together.

## The rule that matters most

**A finding is not a number, it is a statement with a recorded expected state.**
`claims.json` holds every statement the project makes about the engine,
including the ones that currently fail. `verify` checks it against `results/`
and CI fails in **both** directions:

- a limitation that gets worse turns the build red;
- a limitation that gets **fixed** also turns the build red.

One exception, and only one: a claim may name a capability of the *host*
its experiment needs (`"needs": "drop_caches"`), and where the run reports
it could not reach that condition the claim is skipped rather than failed.
Dropping the page cache wants root, which a hosted CI runner does not have,
and failing there would report a fact about the machine as a fact about the
engine. Rule 3 makes such a finding `not_exercised`; `needs` is the other
half of it.

The second is not a mistake. Either the engine improved and the claim is stale,
or the experiment stopped testing anything. Both need a person to decide which.
So when you fix something, update `claims.json` in the same change — that edit
is the record that the fix was intentional.

## Where commentary goes

Two audiences, and they want opposite things. This file, the plan files and a
claim's `because` are notes to whoever picks the work up next with no memory of
it: dense, contextual, and worth keeping even when superseded. `README.md`, a PR
description and the crate docs are read by people who either already have the
context or do not want it, and for them a recounting of the moves and pivots is
noise on top of the summary.

So keep the outward-facing ones **factual, current, likely to stay current, and
simple**. In practice:

- **No counts that move.** Not the number of tests, assertions, claims, commits
  or files. They are wrong within a day and they never mattered.
- **No figures that move.** Directions and trades belong there; the numbers
  belong in `claims.json` and `results/`, where `verify` gates them. Cite the
  claim id instead and the reader gets a checked figure rather than a snapshot.
- **No history.** Not what a section used to say, not what the design document
  called it, not what was tried first. A reader who wants that follows the
  pointer.
- **No narrative of the change that produced it.** A cleanup pass that announces
  itself is the thing it was meant to remove.

The last one is the easiest to get wrong, because the writing feels like
diligence. It is not: the person who asked for the work already knows, and the
person who did not is being handed a changelog they did not ask for. Write the
result as though it had always been that way, and put the reasoning where
reasoning lives.

## Before adding a benchmark

Four rules, enforced in `src/bench/` rather than remembered:

1. **Nothing is measured once.** Use `Trial`, which runs configurations
   interleaved. Report a median with an interquartile range.
2. **A difference is not a difference until it clears `stats::compare`** — a
   Mann-Whitney U test *and* a minimum effect size. Do not hand-roll a
   comparison; the gate exists because the original design document reported a
   13.9% difference as a win against its own stated 15% noise floor, and
   `stats.rs` carries that case as a regression test.
3. **A finding whose precondition was not met is `Finding::not_exercised`,
   never `holds`.** This has already caught three false greens: an out-of-core
   experiment that compared warm against cold inside a dataset too big to be
   warm; a multi-process experiment that ran 8 readers against a 64-slot table;
   and a crash experiment that blamed the engine for crashing before any
   checkpoint existed. If a run cannot reach the condition, say so.
4. **Throughput never travels alone.** Latency distribution, peak RSS, and
   device-level write bytes come with it. Write amplification is measured from
   `/proc/self/io`, never inferred from file size — they are different
   quantities.

## Running the checks

`sh scripts/check.sh` runs every group -- build, test, lint, browser, claims,
suites -- and CI calls the same script with the same names, so a green run
here is a green run there. Use a group name to run one (`sh scripts/check.sh
browser`). Two things are deliberately outside it: `cross-arm`, which needs a
cross toolchain and qemu, and `--profile full`, which takes hours and is run
by hand when a number is going to be cited.

Keep it that way. Every gate this repository has broken has broken the same
way -- a check that was not running, or was reporting a verdict it had not
earned -- and a second definition of "the checks" is how that starts.

## Profiles

`ci` runs in seconds and is **never citable**; it proves the experiments still
run. `dev` is minutes. `full` is the only profile a published claim may cite,
and every record carries which it was.

Never run two timing benchmarks concurrently. Four cores measuring each other
is not a measurement.

## Layout

| path | what |
|---|---|
| `src/db.rs` | the engine: WAL with atomic batches, memtable, sealed segments, partitioned compaction, deletes, `Txn`, and the `SegmentWriter` every segment is written by -- `docs/engine.md` |
| `src/format.rs` | the on-disk format's fixed quantities, owned by no writer |
| `src/block.rs`, `src/index.rs`, `src/flatindex.rs` | the format itself: blocks, extents, the flat key index |
| `src/bytes.rs`, `src/blob.rs` | the read path over any byte source; compiles for wasm |
| `src/bench/` | the measurement substrate — stats, histogram, plotting, env capture |
| `src/bin/internal.rs` | falsification suite |
| `src/bin/correctness.rs` | damaged files (c1), crash injection with power-loss emulation (c4) |
| `src/bin/logshed.rs` | day-index shape, size budget, browser-test fixture |
| `bench/external/` | Supdb inside other projects' evaluations (redb, LMDB, sled, RocksDB) |
| `web/` | the browser reader, its size control and its browser test |
| `results/` | committed measurements — the source of truth |
| `figures/` | generated from `results/`, never drawn by hand |
| `docs/architecture-review.md` | why every experiment here exists |

There was a second engine until recently: the one vendored from the design
artifact, with its own writer, reader, freelist and key table. It is gone, and
`retire-plan.md` records what went with it and why. `block` and `index` still
carry a scoped `#[allow(clippy::all, dead_code)]` -- style not yet paid down,
rather than code anyone may not touch. Nothing is exempt from the format gate.
Everything else holds to `-D warnings`.

The exemption those two modules used to have is worth remembering, because it
was the same shape as every other gate failure here. It was justified in
`scripts/fmt.sh`, in `src/lib.rs` and in this file by the architecture review
citing line numbers in the exempt files. The review cites none, and nothing
had checked. Two of the three files turned out to be rustfmt-clean already, so
the whole exemption was buying a reformat of one file -- and the reason it
survived was that its justification read like one nobody needed to verify.

## Measuring a change to the engine

**Never compare two separate runs.** It was tried here and it does not work:
between a pre-fix and a post-fix run of the same suite, the three *unchanged*
comparators in the external benchmark moved by +20% to +43%. Almost all of the
apparent improvement was the machine.

To measure the cost of a change, put both arms behind a runtime flag and run
them **interleaved in one process**, as `f8-checksums` does for
`Options::checksums`. Space is the exception — file size is immune to drift and
can be compared across runs.

Device bytes have a trap of their own: the page cache sizes a folio by the
write that creates it, and a byte dirtied inside a 1 MB folio writes the
whole megabyte back. f57 pre-wrote a WAL in 1 MB pieces and every 100 KB
commit after that cost 11x its bytes at the device; in 4 KB pieces, 1.04x.
When device bytes move and the design says they should not, ask what size
the writes that first created those pages were.

And use `--profile full`. The same checksum cost measured at `dev` came out
"+3.0%, not significant"; at `full`, with the variance tight enough to resolve
it, it is +8.5% and unambiguous. An underpowered measurement is not a free
lunch, it is a measurement that could not see.

## Comparing against another engine

**Match the guarantees before ranking, or do not rank.** `Features::unmatched`
decides whether a pair may be compared at all and `ordering_of` emits
`not_exercised` when it may not, naming the axes. This is enforced because it
was not: `engines.rs` carried three fairness rules and only two of them
equalized, the third merely *recorded* what each engine promises. Durability
was filed under the third, so an early load ordering compared a Supdb that
never reaches the device against an LMDB that fsyncs every batch and called it
a 1.28x win, with the difference in a table two lines away. The checksum axis
was unequalized the other way for exactly as long, and cost Supdb its read
lead. Both of those orderings retired with the engine that made them, but the
rule is the reason `Features::unmatched` exists.

Equalize in **both** directions where the engines allow it, so a reader gets
the comparison for the guarantee they care about rather than the one that
flatters: `next` and `lmdb` both commit per batch, `next-nodrain` and
`rocksdb-tuned` neither drain. Where an axis cannot be equalized -- LMDB
cannot stop being transactional -- say which way the residual leans and read
the result as a bound: a loss is at least that large, a win is not yet a win.

Against LMDB, matched on durability *and* transactions: durable load **0.825x** of LMDB
in the latest canonical run, the first with the borrowed batch in the
harness (`EXT.22`; 0.694x the run before, 0.49-0.51x the two before that --
the move is piece promotion, because the canonical load's keys ascend and
now route by rename with no merge; a uniformly random order sits near
0.42x, F55.3), point reads **2.2-2.5x** over the three runs with inline
runs and 1.4-1.6x over the seven before (`EXT.23`, ten consecutive holds),
ordered scan 0.90x in the latest run after six ties (`EXT.24`). Leaving
partitioning to compaction no longer separates the arms at this load
(`EXT.25`, `EXT.26`: ties, because the trigger fires at the fourth 32 MB
seal either way). Its story is in `docs/engine.md`; every number there
is under the same gate.

On Apple Silicon the same pair reads 3.30x and 3.18x, scans 1.20x twice,
and loads at a tie (0.99x, 0.96x, both no difference) because under
F_FULLFSYNC the barrier count is the floor for both engines
(`results/apple-silicon/`, fifth campaign).

Where the durable load's instructions go is measured, not guessed
(`docs/profiling.md`, f58): 1,359 an appended record, of which the
engine is 677 -- the WAL frame 227 with a 92-instruction CRC, the
memtable probe about 180 and nearly every cache miss -- and the harness
640, because the external suite's `write_batch` takes owned vectors and
allocates two per record for every engine alike. Compute is the third
slice of the x86 durable load after the barrier and the seal wait; the
cheapest moves were a borrowed batch in the harness (done: 1,037 a
record) and a per-batch CRC (done: 968, at two more L1 misses, no
wall-clock claim, kept for the one-CRC-one-batch invariant).

Against RocksDB, the engine it is shaped like (`rocksdb`, `rocksdb-nosync`
in the external suite; defaults with compression and read-side checksum
verification off, so the pair is matched): durable ordered load **0.778x**
(`EXT.28`, failing), point reads **7.62x** (`EXT.29`), ordered scan
**5.95x** (`EXT.30`), shuffled durable load **1.18x** (`EXT.31`); RocksDB
keeps the smallest file, 109.8 MB against 167.8. Tuned as deployed
(`rocksdb-tuned`: a 256 MB block cache the data fits in, a Bloom filter,
four background threads) the pair reads **6.45x** and scans **4.70x**
(`EXT.33`, `EXT.34`), because the tuning moved RocksDB's read only from
195,729 to 232,697 a second at 1M keys; the load stays at 0.688x
(`EXT.32`) and the shuffled load a tie (`EXT.35`). So the reads may be
quoted against RocksDB either way, and the load goes to RocksDB either way.

The seal wait in the durable load is the drain, not backpressure: f60
found zero joins that blocked on an unfinished seal under either key
order and the manifest at 2% of the seal phase; 74% is the last memtable
being sealed and partitioned because the adapter's `sync` drains, which
RocksDB's `sync` (an fsync of its WAL) does not. So the drain is matched
both ways (drain-plan.md; `next-nodrain`, `rocksdb-tuned-drain`): with
neither draining the durable ordered load is a **tie** (0.904x, `EXT.37`)
and the shuffled load **2.37x** (`EXT.41`), with both draining 0.815x
(`EXT.36`); point reads lead 4.7x undrained and 7.1x drained (`EXT.38`,
`EXT.40`); and the ordered scan of an undrained store was 8.6x slower than
of a routed one (2.9M against 24.7M entries/s, `EXT.39`, then failing at
0.68x of tuned RocksDB; now 5.98M and 1.29x, holding, with 1.08x on the
replication -- the rest of this paragraph is why). That was read as
the k-way merge over unrouted sources, and f63 says it was not: scans that
start inside a segment cost 53 ns an entry under the merge against 31
routed (F63.4, 1.7x), and entries served from the memtable's range 124
(F63.3, 2.3x). The 16x that f62 measured was the **sorted snapshot of the
unsealed keys**, which `Db::scan` builds on the first scan after a commit,
one `Vec<u8>` per key at 300 ns a key, over a memtable that behind `sync`
still had its frozen twin beside it -- a 286,000-key seal in flight that
the experiment never joined. The build now keeps the keys in one arena,
radix-orders the hash slots by key offset so the copy is sequential, and
sorts 24-byte prefix records: 10 ms against 58 at 142k unsealed keys, 32
against 314 at 428k (F63.1), which moves f62's measurement 2.28x on its
own (F63.2). `Db::unsealed_keys()` exists so an experiment can check the
shape it built, and `settle` is what joins an in-flight seal; `sync` does
neither (scansnap-plan.md).

On YCSB, matched and undrained (`EXT.42`-`EXT.45`, five repetitions):
update-heavy A **1.74x**, read-only C **2.45x**, short-scan E **1.28x**,
read-modify-write F **2.20x** against tuned RocksDB, every update a
replacing `put` in a durable 100-record batch. The first run of that
suite read 0.14x on A because the adapter's `write_batch` appends, which
is the load verb and not an update; the row is not recorded and the
lesson is in ycsb-plan.md. On E the undrained arm trails its own drained
shape 4.3x and LMDB 9x: the unrouted scan, once more.

Under shuffled arrival the same matched pair inverts. `EXT.27`
(`ext-loadshape`, full) has the engine at 284,938 ops/s against
LMDB's 48,041, **5.93x** (6.64x replicated with the borrowed batch),
because a durable commit of a thousand random
keys dirties about as many B-tree leaf pages and the fsync writes them
all; the engine's own ordered arm in that run is 0.653x, so the
canonical load's ascending keys are the one arrival order that flatters
the B-tree. Quote the two together. The plan for that run predicted the
opposite (shape-plan.md), which is why it is written down.

Two lessons from that engine's load numbers are worth keeping, because they
are about method rather than about the code that is gone.

The first is that a fix can be refuted by its own gate. When the redo log
started carrying values, the first version scanned every key at every
durability point looking for unlogged bytes -- O(keys) a point, O(keys^2) a
load. It was invisible at 200k keys, where it measured 1.435x ahead, and
fatal at 1M, where it measured 0.149x. The suite caught it because the
canonical run is large; a smaller one would have shipped it.

The second is that the comparator tells you whether to believe a number. Two
consecutive runs of the same unchanged load gave 1.06x and 0.60x, because the
LMDB arm that nothing here touches moved 85% between them while Supdb's own
arm moved 5%. An axis whose comparator moves like that is unmeasured on this
host, whatever ratio the run prints.

Two runs is the minimum for a number here.

Rule 4 is why the worst of that engine's behaviour was ever legible. The suite
reported throughput, read latency and file size and neither of the other two
the rule names, until it did -- and then a load that wrote 116 MB of data was
seen sending 29.9 GB to the block layer, a write amplification of 270x against
LMDB's 2.1x. A cost that had been on the books as a time cost was a device
cost of the same origin, and nothing but the rule would have shown it.

## The reader, and the ways it can quietly disagree

`blob::Blob<B>` reads through a `Bytes` source, so the same code serves a
mapped file and a browser reading an object out of S3. `Blob<MmapBytes>` is
the native path and lends its bytes; `Blob` over a source with no memory
behind it copies. That difference is the liability, because its failure mode
is not a crash but a browser quietly answering a different question from the
server, so `tests/blob.rs` writes a segment and requires a lending source and
a copying one to agree on every key, every value, every count -- and pins
`Blob::zero_copy()`, because a native reader that started copying would still
pass every correctness check.

The agreement checks have caught real differences: a reader reporting the
superblock's generation where another reported the index section's, and a
`value_bytes` that counted the varint length prefixes it claimed to exclude.

Nothing in that path is asynchronous, and that is the constraint rather than an
accident. `flatindex::lookup` returns a borrow into the index section and a
borrow cannot survive an `await`, so the byte source is synchronous: JS
downloads the object into OPFS once, and every read after that is
`FileSystemSyncAccessHandle.read`. That is only viable because a day fits —
`w1-daysize` puts a 32 MB download at 911,192 log lines — and it is why that
was measured before any of it was built. `web/README.md` has the rest.

`Bytes` has two halves for one reason: `read_at` copies and every source can
answer it, `slice_at` lends and only a source backed by memory can. Native
takes the second for every access and copies nothing, which is the axis
`flatindex` exists to win and the one a byte-source abstraction is most likely
to lose. `Blob::zero_copy()` is asserted in the test, because a native reader
that started copying would still pass every correctness check.

**The dictionary can be read by range without holding the index (R6.3).**
`blob::SparseBlob` keeps the key section's header and fence and plans a
range as a directory slice and then the record span it names; the walk
reads exactly those two plans, and `tests/dict.rs` holds it to the whole
reader's answer over 135 ranges per index shape, on a recording source,
and through a source that serves only what was ensured. It exists for the
day a dictionary is too large to fetch whole; `w5-dict` prices it on the
day index and found the 64 KiB cache page, not the bytes, to be the unit
that matters at today's sizes (W5.1 and W5.2 recorded as failing their
byte predictions by page geometry; W5.3 exactness and W5.4 speed hold;
at 16 KiB pages, which the browser's sparse reader now uses, the open is
8.8% of the whole open and a field's range 0.59 of its bound, W5.5, W5.6).
It is a third read path, and carries the second's liability: its failure
mode is a quiet different answer, which is why every range is checked
against `scan_counts` rather than against itself.

**A count costs a lookup, and it took a format change to make it so (v5).**
`f28-count` runs four arms interleaved. Resolving a key and stopping is 94 ns;
the general `count` is 94; reading every value is 2,345. Before format v5 the
count walked the run's length prefixes and cost 2,493 ns -- what reading cost
-- because an `Ext` was block, offset, byte length and the offset of the last
record, and none of those is a count; skipping a payload does not skip the
cache lines it sits in. A per-extent count was priced then at under 20 ns of
saving for four bytes an extent and declined (W2.3's first form). When
variable-width counts became a requirement it was built instead of a
companion file: the four bytes are paid by every extent (20-byte records),
and the top bit of the count is the tombstone flag deletes ride on. W2.1 and
W2.2 flipped with it and say so. `count_fixed` and `scan_counts_fixed` survive
and are no longer special: the general `scan_counts` ranks a 2,000-key
dictionary at 4.5 ns/key against the fixed form's 5.2 (W2.4 fails, W2.5
holds), so a day's whole term dictionary ranks in about 9 µs for any schema
and nothing has to be precomputed at roll time. A file written before v5 is
refused by its magic rather than misread.

**A small run lives in its index record, and a read of it touches no block.**
Since the inline extension of v5, the segment writer stores a run of values
up to `Options::inline_bytes` (256 by default) inside the record itself,
after the extents; its extent names `Ext::INLINE` instead of a block. A point
read then costs the hash slot and the record -- two cache misses fewer than
the block table row and the block at a million keys, which f53 measured
(F53.1) as the largest read gain in the project -- and `ranges_for` plans no
fetch for it, so a browser reading a small key over ranged HTTP fetches
nothing after open. The prices are on the sequential walks, where wider
records mean more bytes per key (F53.3, F53.4), and they are recorded beside
the gain. To let those records stream, the writer lays the key section out
records-first -- header, records, then fences, directory and hash slots --
which `FlatIndex::parse` accepts because every region is named by offset;
The writer emits either layout -- `set_inline_max(0)` gives the original
order with every run in a block -- and `Blob` reads both, which is what
`tests/segwriter.rs` holds them to. A v5 reader from before the extension errors on the block id
rather than answering wrongly, so the magic did not move.

**A cold sparse open is one or two round trips, and a cold search three
(R7, waves-plan.md).** logshed measured seven dependent round trips for a
first page of search results over a cold cache on a real day, five of
them the store's before a posting byte moved. Three things removed them,
each measured by `w6-waves` through a host that models the browser's
cache -- an `ensure` that brings in a page is one wave, and a finding is a
count, not a timing. A write-once segment writes an **extension into the
spare part of the superblock page** (a copy of the key header and the
offsets of fence, directory, hash and checksum row), so the sparse open's
first plan names everything and its second is empty: two waves from a
page-sized probe against the store's three (W6.1). With
`SegmentWriter::set_head_reserve` the writer leaves a reserve after the
page and fills it at finish with the block table, the row, a copy of the
fence and a copy of the directory when they fit, and a host whose first
probe covers the reserve (`openSparse(wasm, cache, {probe})`) opens in one
wave (W6.2), at 1.63% of the file for 128 KiB on the fixture (W6.7).
`BlobOptions::resident_directory` fetches the directory in the open wave,
so a lookup after open is at most the records' wave (W6.3) and a cold
search is open, records, postings -- two on the fixture, three by
construction, from six (W6.4). And a **data read fetches the 4 KiB chunks
an extent spans**, not its block, when the block is plain and carries
per-chunk checksums: the rare key's postings wave is two chunks where
logshed's two-hit word read 920 KiB (W6.5); W4.1's exactness holds on the
chunk plan as it did on the block plan. The third ask, small values inline
in the record, was already the segment writer's (`inline_max`, 256 bytes
since v5's inline extension) and never the store's: a segment answers the
rare key at the dictionary with no postings wave (W6.6), and the
recommendation to the roll is to write through `SegmentWriter`, which
`logshed build`'s day already sorts for.

**A segment writer can compress its blocks, and the encoding decides
whether that is worth anything (R7.4).** `SegmentWriter::set_compress`
takes the path `write_block` always had: chunked above the chunk
size so a point read decompresses one chunk, verbatim when compression
does not pay, and a verbatim block now carries per-chunk checksums so
`chunk_span` plans by chunk rather than whole. On logshed's day it saves
**19.9%** of the file, against the 25% predicted (`W6.8`, recorded as
failing). Two things that measurement found. The same day stored as
absolute ordinals compresses **0.0%**, byte for byte identical, because
LZ4 matches repeated sequences and a rising counter has none -- so both
arms of the comparison store deltas, and logshed's 2x is a property of
their encoding rather than of the flag. And 19.9% is far under the 2x LZ4
gets on the blocks themselves, because inline runs put every run under
256 bytes in the key section, which is not compressed; on a Zipf
dictionary that is most of the terms, so inlining and compression pull
against each other. What is still whole-block is a *compressed* block
read by range: `with_extent` hands `read_chunked_range` the whole buffer,
and fetching the chunk directory and then the chunks an extent spans is
what would let a browser raise its block size (segcompress-plan.md).

**A segment's key index is checksummed, and a flipped bit in it fails the
open.** Every block was checksummed and verified once per reader (f8); the
key index was not, and v6 made that a quiet misread rather than a theory:
a flipped `FIXED` bit re-decodes a run under the other encoding with no
error, and a flipped offset or count always could. A segment's key section
now ends in a row of CRC32C words, one per 16 KiB piece, named by two
header words that were spare; `Blob::open` verifies every piece once and
no read pays anything after, and `SparseBlob` rounds its plans to pieces
and verifies each the first time it uses it. A store's in-place-editable
index carries no row -- a record is published there with one aligned
store into a mapping readers hold, and a piece checksum cannot follow that
lock-free -- and `index_checksummed()` says which kind a reader has.
`tests/segwriter.rs` flips every seventh byte of a segment's key section
and requires each to fail the open; its first run found a flip of the
piece-shift word that made the row look absent and opened clean, which is
why a row named with an impossible shift is now damage rather than
absence. The magic did not move: the words are zero in every older file
and unread by every older reader (indexsum-plan.md).

**A run of one width is stored without prefixes, and reading it is a
memcpy (v6).** The segment writer, and the store when it seals a key's
pending bytes or consolidates its extents, check whether every value in the
run has the same length; if so the values go back to back with no varint
prefixes and the extent carries `Ext::FIXED` (bit 30 of the count word,
beside the tombstone bit), the width being `len / records`. Mixed runs keep
the prefixed form and every reader branches on the flag through
`index::each_value`. The superblock magic moved to v6 so a reader from
before the flag refuses the file. `ext-analytics` is where it was priced:
reading a term's whole posting list went from **0.307x** of LMDB's DUPFIXED
to parity or better in five runs (1.20x, 1.19x, 1.25x nd, 1.15x, 1.20x nd;
`EXT.18`, now holds), the intersection of two lists from **0.769x** to
**1.15-1.19x** (`EXT.17`, now holds) with `Blob::intersect_fixed`, a
two-pointer walk over both keys' runs in place that compares 4- and 8-byte
values as big-endian integers, and the day index shrank from 5.02 MB to
4.05 against LMDB's 7.33. The kernel's first form was slower than the naive
decode-then-merge at `full` (0.842x of LMDB) because of a bounds-checked
slice compare per step; the record of that is in `fixedrun-plan.md`, and
the naive merge stays in the checksums-on arm so every run prices the
kernel against it. Two consequences to know: `stored_bytes` counts payload
only for a fixed run, since there are no prefixes to count; and a flipped
FIXED bit in an index record re-decodes the run quietly instead of
failing, which the block checksum cannot see and the key index section's
own checksum, not yet built, would. `count_fixed(width)` is exact for a
fixed run because the flag says what the caller had to assume; for a
prefixed run it is still the two-quantity check below.

`count_fixed` claims a count only when two independent quantities agree: the
run is a whole number of strides, *and* `Ext::last` — the offset of the final
record, stored so that reading the newest value is O(1) — is exactly
`(n-1)*stride`. Divisibility alone is not enough and was not: a run of 17
variable-length values divided exactly by a stride of 4 and the first version
answered 23. Two quantities is still not a proof, so the contract is that the
caller knows its schema; `tests/blob.rs` carries the case either way.

**A roll sorts by key first, and now it has no choice.** The writer takes keys
in byte order and nothing else, so the arrival order of a day's log lines
cannot reach the file: the sort is between them. That closed an axis rather
than winning it -- the previous engine would accept line-ordered appends and
charge several times the file for them, which is why `w1-daysize` used to
carry a line-order arm. What is left is the day's own size, and `W1.1` and
`W1.2` are where the bytes per line and the download budget live.

## Known-failing on purpose

Roughly a third of the claims in `claims.json` are recorded as `fails`, and
that is the file working rather than the project failing. Most of them are
registered predictions that the run refuted -- a plan file said a lever would
buy something, the measurement said it did not, and the finding stays on the
books so the idea cannot quietly come back. Do not "fix" one casually: each is
load-bearing evidence and each carries its reason in its `because`.

If you fix one, the corresponding claim must change from `fails` to `holds` in
the same commit, and the review in `docs/` should be updated to say so. The
gate fails in both directions precisely so that flipping one is a decision
somebody made rather than a thing that happened.

The engine's own standing limitations, as opposed to refuted predictions:

- Out-of-core reads fall off a cliff. Once the file exceeds the memory that
  can cache it, throughput drops by about three orders of magnitude and the
  latency distribution goes bimodal -- every miss is a synchronous page fault
  (`F1.2`, `F1.4`). This is the mapped read path's shape, not a bug.
- The durable ordered load is behind LMDB and behind RocksDB (`EXT.22`,
  `EXT.28`), and shuffled arrival inverts both. Quote the pair, never one.
- The index layout study found smaller and faster points on the frontier that
  the shipping layout does not occupy (`F9.3`, `F9.5`, `F9.7`).

## What the retired engine taught, that still applies

The original engine is gone (`retire-plan.md`), and with it the reproducers
for a decade of its defects. The bugs are not worth recounting; the shapes
they came in are, because they are shapes this engine can take too.

**A path only one arm exercises is a path nothing tests.** A delete was never
marked dirty, and the checkpoint that was asked to carry it dropped it,
leaving the key readable at its old extents. It was invisible for as long as
every insertion forced a full rewrite, because a rewrite reads the tombstone
directly. Turning a flag on is what exposed it -- and the bug was older than
the flag.

**The sharp edges of a log are in the bookkeeping around it, never in the
append.** A value-carrying log queued a key twice when it sealed, re-queued
and sealed again inside one interval, and logged the same delta twice. A
replay applied records over newer index state because nothing said which was
newer. A durability point acked before the block table that named its blocks
was synced, so a crash at exactly that point left a log naming blocks the
recovered table did not have.

**A clean test result proves nothing about a path the test never took.** The
first reproducer for the replay-ordering bug came back green and was
inconclusive until a path trace showed it had never reached the in-place arm.

**Arithmetic that underflows fails quietly and expensively.** A size-class
calculation underflowed for every block of 4 KiB or less -- which is every
block a store of short postings produces. Debug builds panicked; release
builds wrapped and reserved 7,680 bytes for a tiny placement, so every small
store paid about 1.9x on every section it wrote, visible to benchmarks as
size rather than as a fault.

**A cap is a property of the process, not of the experiment that asked for
one.** `env::cap_memory` put the process inside a cgroup limit and nothing
lifted it, so one experiment's 16 MB ceiling stayed in force for everything
after it and the next allocation past it was killed. Seventeen experiments of
thirty-three never ran, on any host where the cap actually worked -- and on a
host where it silently failed, the suite ran to the end and looked fine.
`env::cap_guard()` is the fix and the lesson is the shape: a check that
reports a verdict it has not earned.
