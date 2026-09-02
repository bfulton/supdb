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

The second is not a mistake. Either the engine improved and the claim is stale,
or the experiment stopped testing anything. Both need a person to decide which.
So when you fix something, update `claims.json` in the same change — that edit
is the record that the fix was intentional.

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

## Profiles

`ci` runs in seconds and is **never citable**; it proves the experiments still
run. `dev` is minutes. `full` is the only profile a published claim may cite,
and every record carries which it was.

Never run two timing benchmarks concurrently. Four cores measuring each other
is not a measurement.

## Layout

| path | what |
|---|---|
| `src/` | the engine, vendored from the design artifact **verbatim** |
| `src/bench/` | the measurement substrate — stats, histogram, plotting, env capture |
| `src/bytes.rs`, `src/blob.rs` | the read path over any byte source; compiles for wasm |
| `src/next.rs` | the next engine: WAL with atomic batches, memtable, sealed segments, partitioned compaction, deletes, `Txn` -- `docs/next-engine.md` |
| `src/bin/internal.rs` | falsification suite (f1–f7) |
| `src/bin/correctness.rs` | damaged files, model oracle, crash injection (c1–c3), crash injection for the next engine with power-loss emulation (c4) |
| `src/bin/logshed.rs` | day-index shape, size budget, browser-test fixture |
| `bench/external/` | Supdb inside other projects' evaluations (redb, LMDB, sled) |
| `web/` | the browser reader, its size control and its browser test |
| `results/` | committed measurements — the source of truth |
| `figures/` | generated from `results/`, never drawn by hand |
| `docs/architecture-review.md` | why every experiment here exists |

The engine modules carry scoped `#[allow(clippy::all, dead_code)]`. They were
vendored byte-for-byte from the design artifact and have since been changed
only to fix specific defects, each described in `claims.json`. **Do not
reformat them** — the architecture review cites line numbers in commit
`101a4e7`, and `results/baseline/` holds the measurements taken against that
revision. Everything in `src/bench/`, `src/bin/` and `bench/external/` holds to
`-D warnings`.

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
was filed under the third, so `EXT.1` compared a Supdb that never reaches the
device against an LMDB that fsyncs every batch and called it a 1.28x win, with
the difference in a table two lines away. The checksum axis was unequalized the
other way for exactly as long, and cost Supdb its read lead.

Equalize in **both** directions where the engines allow it, so a reader gets
the comparison for the guarantee they care about rather than the one that
flatters: `supdb-durable` and `lmdb` both commit per batch, `supdb-buffered`
and `lmdb-nosync` neither do. Where an axis cannot be equalized -- LMDB cannot
stop being transactional -- say which way the residual leans and read the
result as a bound: a loss is at least that large, a win is not yet a win.

The matched scorecard against LMDB, `full`:

| | Supdb | LMDB | |
|---|---|---|---|
| load, both durable (`EXT.9`) | 199,485/s | 572,416/s | **0.348x**, failing |
| load, neither (`EXT.10`) | 628,814/s | 652,367/s | no difference here; 0.85x on Apple Silicon, replicated |
| read (`EXT.11`) | 1,325,496/s | 917,928/s | 1.444x; 1.196x, 1.092x, 1.043x (a tie), 1.379x, 1.262x, 1.444x across six runs -- the comparator moves with the host; 2.42x on Apple Silicon, replicated |
| scan (`EXT.12`) | 17.4M/s | 18.6M/s | coin toss here (1.16x sig, then 0.93x nd, same night); 1.17x on Apple Silicon, replicated |

The next engine (`src/next.rs`), matched on durability *and* transactions
since it gained atomic batches and `Txn`: durable load **0.825x** of LMDB
in the latest canonical run, the first with the borrowed batch in the
harness (`EXT.22`; 0.694x the run before, 0.49-0.51x the two before that --
the move is piece promotion, because the canonical load's keys ascend and
now route by rename with no merge; a uniformly random order sits near
0.42x, F55.3), point reads **2.2-2.5x** over the three runs with inline
runs and 1.4-1.6x over the seven before (`EXT.23`, ten consecutive holds),
ordered scan 0.90x in the latest run after six ties (`EXT.24`). Leaving
partitioning to compaction no longer separates the arms at this load
(`EXT.25`, `EXT.26`: ties, because the trigger fires at the fourth 32 MB
seal either way). Its story is in `docs/next-engine.md`; every number there
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
`EXT.40`); and the ordered scan of an undrained store is 8.6x slower than
of a routed one (2.9M against 24.7M entries/s, `EXT.39`). That was read as
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
(`ext-loadshape`, full) has the next engine at 284,938 ops/s against
LMDB's 48,041, **5.93x** (6.64x replicated with the borrowed batch),
because a durable commit of a thousand random
keys dirties about as many B-tree leaf pages and the fsync writes them
all; the next engine's own ordered arm in that run is 0.653x, so the
canonical load's ascending keys are the one arrival order that flatters
the B-tree. Quote the two together. The plan for that run predicted the
opposite (shape-plan.md), which is why it is written down.

`EXT.9` has moved three times, each for a decomposed reason: 6,735 -> 54,333
ops/s when `Options::index_inserts` stopped every batch rewriting the whole
key index; -> ~152,800 when durability points went log-first with a single
fsync (f36's ledger had convicted mmap writeback under the per-batch fsync at
87.4% of all device bytes); -> ~200,000 when the log started carrying VALUES
(`Options::log_values`), so a durability point appends unsealed bytes and
seals nothing -- blocks are written later, full, on the store's own schedule.
Write amplification went 270x -> 105x -> 13.2x -> ~7x. Still ~3x behind, so
still recorded as failing; what remains is the per-batch append+fsync+section
work against LMDB's single page-chain commit, and the macOS F_FULLFSYNC pair
says the floor there is the fsync count itself. The value-log step was nearly
refuted by its own gate: the first version scanned every key per point for
unlogged bytes, an O(keys^2) tax invisible at f36's 200k keys (1.435x ahead)
and fatal at EXT.9's 1M (0.149x); a per-shard queue of keys with unlogged
bytes -- `dirty`'s twin -- removed it, and both 1M runs are kept.

`EXT.10` cannot currently be read at all. Two consecutive runs gave 1.06x and
0.60x because `lmdb-nosync`, which nothing here touches, moved 85% between
them. Supdb's own arm moved 5%. Treat the load axis as unmeasured on this host
until it is taken somewhere quieter, and note that nothing shipped this session
should move it: that load never checkpoints per batch, so the log and
`index_inserts` are both inert there.

Two runs is the minimum for a number here, and it is the comparator that tells
you whether to believe it: the durable arm moved 1.0% between the pair while
`lmdb-nosync` moved 85%, which is the whole difference between a result and a
coincidence.

Rule 4 is why two of those numbers are legible at all. The suite reported
throughput, read latency and file size and neither of the other two the rule
names, until it did: the durable arm sends **29.9 GB to the block layer for
116 MB of data**, a write amplification of 270x against LMDB's 2.1x, and
leaves a 7.35 GB file. `checkpoint` being O(key count) was on the books as a
time cost and is a device cost of the same origin.

## The second reader

There are now two read paths. `store::Reader` maps a file; `blob::Blob<B>`
reads through a `Bytes` source and so runs where there is no file to map — a
browser, over an object fetched out of S3. Same format, same `flatindex`, same
`block` decoder. A second read path is a liability, because its failure mode is
not a crash but a browser quietly answering a different question from the
server, so `tests/blob.rs` opens a store written by `store.rs` and requires the
two to agree on every key, every value, every count and the checkpoint
identity. It has already caught two: `Blob` reporting the superblock's
generation where `Reader` reports the index section's, and a `value_bytes` that
counted the varint length prefixes it claimed to exclude.

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
up to `NextOptions::inline_bytes` (256 by default) inside the record itself,
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
`Store` still writes the original order and never inlines, `Blob` reads both,
and `store::Reader` does not serve inline runs (a next-engine segment is read
through `Blob`). A v5 reader from before the extension errors on the block id
rather than answering wrongly, so the magic did not move.

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

**How the roll writes decides the file size, by 5.17x (was 22.6x).** Appending
a day's postings in log-line order wrote 831 MB where grouping them by term
first writes 36.7 — 44,629 inline merges against zero, which is F5.1's latency
tail showing up on the space axis. Deferred consolidation
(`Options::defer_merge`, default on since f37 priced it at 3.963x on
fragmenting appends for a 0.762x read-back cost) cut the line-ordered day to
190 MB, so the penalty is 5.17x now — but it still grows with the day, and any
tool that builds an index here still sorts by key first.

## Known-failing on purpose

Do not "fix" these casually; each is load-bearing evidence and each is
described in `claims.json`:

- A reopened store declares history before the reopen broken. `Store::open`
  does not carry the reuse log across, so `history_from` is set to the
  generation opened and older snapshots are refused rather than served.
- Reader open grows with key count, though no longer in proportion to it: 20x
  for 100x the keys, and what remains is the block table rather than the key
  index. The index is 57 bytes per key in a mapped section readers share; it
  was 131 bytes per key, heap-resident and duplicated per process.
- Write throughput barely scales with writer threads.
- `checkpoint` is O(key count): it rewrites the whole key index rather than
  what changed. The durability *curve* now has a usable point on it -- a
  20,000-op window sustains ~199k ops/s with about 2MB at risk -- but that came
  from making writes faster, not from fixing the floor.

If you fix one, the corresponding claim must change from `fails` to `holds` in
the same commit, and the review in `docs/` should be updated to say so.

Fixed so far, with reproducers kept in `tests/known_bugs.rs`: delete
resurrection, the double-free that handed one slot to three blocks, decoder
panics on damaged input, silently-served corruption (now checksummed), a
checkpoint that appended three index sections and released none of them, and a
reader that fed a flat block-table section to the varint decoder and reported
the misparse as file corruption, and an in-place checkpoint that republished a
record into its hash slot and not its directory entry -- so `read_all` returned
the new value and `scan` the previous one, silently, for every key it touched.

Also fixed: a logged durability point could be lost WHOLE, store refusing to
open. `checkpoint_to_log` fsynced extent records naming blocks sealed in the
same batch, but the block-table section mapping those ids to offsets was
written after the fsync and rode unsynced -- so a crash at the ack point left
a log naming blocks the recovered table did not have, and `open` refused the
file with "index names a block the table does not have". Every durable batch
that sealed a block had this window, from the day the redo log shipped; no
crash test ever placed a crash between the ack and the section writes. The
log is now self-describing: a `Blocks` record carries the table extension in
the same CRC'd, generation-stamped stream, before any extent record that
needs it, under the same fsync. The reproducer emulates the crash by
restoring the pre-batch superblock slots -- exactly what rides unsynced
behind an arena fsync -- and runs both log shapes.

Also fixed, before it shipped: the first value-carrying log walked every key
in every shard at every durability point to find the ones with unlogged
bytes. O(keys) per point, O(keys^2) per load: invisible at 200k keys, fatal
at 1M. The fix is `Shard::log_queue`, and its first version had a bug the
release suite caught the same hour -- a key that seals, re-queues and seals
again inside one interval was queued twice and logged the SAME delta twice,
four copies of one value in the reproducer. Dedupe at the gather. The lesson
is the panel's again: the sharp edges of a log are all in the bookkeeping
around it, never in the append.

Also fixed: under `Sync::EveryN` and `Sync::Interval`, every logged point
fsynced -- log-first had quietly made EveryN mean Always. The log append is
now unconditional and the fsync obeys the policy, which restores EveryN's
stated contract (bounded loss, amortized flush) on the log path.

Also fixed: log replay applied records over newer index state. A logged
checkpoint leaves records in the arena and restores its keys to `dirty`,
which usually keeps every later checkpoint on the log path too -- the masking
that hid this. But a delete shrinks a record to a few bytes, so tombstoning
logged keys let the next checkpoint go in place: index newest, arena older,
nothing saying which. On crash-reopen, replay resurrected all forty deleted
keys. Every log record now carries its checkpoint's generation inside the
CRC'd frame, the superblock carries `index_gen` (last index-updating
checkpoint), and both replay paths apply a record only if its stamp is newer.
Found by writing the reproducer for a suspicion the design panel had also
flagged ("replay ordering across record kinds is the sharp edge") -- the
first repro came back clean and was inconclusive until a path trace showed
it never reached the in-place arm, which is its own lesson: a clean result
proves nothing about a path the test never took.

Also fixed: `freelist::class_of` underflowed for every length of 4 KiB or
less -- which is every block a store of short postings produces. Debug builds
panicked on the subtraction; release builds wrapped and filed the block into
the largest sub-class of the smallest octave, so `capacity_for` reserved 7,680
bytes per tiny placement and every small store silently paid ~1.9x on every
section it wrote, visible to benchmarks as size rather than as a fault.
Reported by the logshed session from a three-posting repro whose 65,536-byte
file the fixed arithmetic reproduces exactly. Found the same day: a closed
store kept its 4 MB redo-log arena in the file -- close() now drops what
nothing can ever append to, which took the fixed cost of a day-index segment
from 4.8 MB back to 618 KB.

Also fixed: `live_key_off` named the live key section only when the index was
flat *and* could be adopted for in-place editing. With the varint layout it
stayed `None`, so the pruning loop compared every historical key section
against "the live one", found no match, and released the one still in use. It
was survivable for as long as every checkpoint rewrote the key section, because
then the previous one really was superseded. The redo log breaks that
assumption -- a logged checkpoint publishes no key section, so the last one
outlives its generation -- and the next block table was placed on top of the
index every reader was using. It presented as lz4 failing to decompress a
section nobody had written there, which is three layers away from the cause.

Also fixed: a delete was never marked dirty. `checkpoint_in_place` and the redo
log both publish only what `dirty` names, so a tombstone they were asked to
carry was dropped and the key stayed readable at its old extents. It was
invisible for as long as any insertion forced a full rewrite -- a rewrite reads
every key from the shards and sees the tombstone directly -- so turning
`Options::index_inserts` on is what exposed it. The bug is older than the flag,
which is the argument against leaving things behind flags: a path only one arm
exercises is a path nothing tests.

Also fixed: `put` probed the key table twice per call -- `get_or_insert` and
then `index_of` -- immediately below a comment saying it probes once. One
probe now, 11.3% of the put path's instructions, measured with cachegrind
because the path is memory bound and the saving is compute: 1,739 to 1,543
instructions per key, D1 misses unchanged at 26. No wall-clock claim is made
and none should be.

Also fixed: a store recorded nothing about the byte order that wrote it. Every
scalar goes to disk little-endian, but the two structures that make this format
fast are addressed in place regardless -- `flatindex` hands back `&[Ext]`
borrowed out of the mapping, and a block table's records are reinterpreted
rather than decoded -- so a file is self-consistent only on the byte order that
wrote it. A big-endian-written store would have been read, with every extent
field silently byte-swapped, rather than refused. The three magics are now
written `to_ne_bytes`, which is a byte-order mark that costs nothing and
changes no file already written: identical bytes on a little-endian machine,
and swapped on any other, so the magic check itself does the refusing. Both
open paths name it rather than reporting damage.

That last one needs a reopen to show: a fresh store takes the full-rewrite path
until an index section exists with a matching key count, and every scan test
here built its store in one session, so the in-place path was never under test
when a scan was checked. `c2-oracle` does not exercise reopen-then-update
either, which is why the differential oracle did not catch it. Until it does,
that shape is covered only by its reproducer.

The largest one is `Store::read_all`. A writer can read its own sealed, staged
and pending state, so a read after a write no longer needs a checkpoint and a
fresh `Reader`; a scan refreshes with `publish` rather than `checkpoint`,
because it needs the writes visible and not durable. `EXT.3` moved from 13.5x
to 0.76x. It also moved the mixed YCSB workloads against LMDB from 0.07-0.14x
to 18.9x on A and 18.4x on F -- but do not read those as wins. They are
unmatched: LMDB commits durably on every batch there and Supdb does not, and
`ext-ycsb` emits no cross-engine finding, so nothing gated them. EXT.9 prices
that difference at 91x on a 1000-op batch, and YCSB batches at 100, so a
matched YCSB-A is not merely slower, it is currently unrunnable at `full` --
which is itself the finding. YCSB-E is the one still losing even unmatched, at
0.43x, because publishing rewrites index structure in proportion to the key
count rather than to what changed.

Two of the four above have moved from `fails` to `holds`. F2.2: reader open is
sub-linear in key count since the key index became a mapped section
(`src/flatindex.rs`, `Options::flat_index`). F4.2: a usable durability point
exists since block compression was turned off by default (`f12-compress`
prices that at 3.6x on reads, 30x on scans, 3.8x on writes, for 1.04x the
disk). F2.1 still fails — sub-linear is not independent — and so does F4.1, at
38x.

The compression change also took the size axis away: `EXT.6` moved from `holds`
to `fails`, since Supdb stores 168.6MB where LMDB stores 126.9. That was traded
knowingly, and scans are what it bought — `EXT.5` went from 4.7x slower than
LMDB to 0.96x of it, which is `no_difference` at p=0.37 rather than a lead.
Two earlier versions of that sentence were wrong in the same way twice: it
claimed 1.29x from a `ci` run, which is never citable, and then 0.65x from a
`full` run whose result file had since been regenerated underneath it. `verify`
compares the recorded verdict against `expect` and never reads the prose, so a
number quoted in a `because` can rot for as long as nobody re-derives it. When
you cite a figure here or in `claims.json`, read it out of `results/` first.

Scan is the one axis where Supdb and LMDB cannot be told apart *warm*, and it
took a methodology fix to see that. Cold they can: `EXT.12` scans at 0.785x
with checksums equalized. The two suites measure different things and both are
right -- `ext-sweep` builds one store per engine and sweeps it repeatedly, so
it walks a warm structure, while `ext-kv` loads a fresh store per repetition
and scans it once. Do not average them. `ext-sweep` used to decompose scan cost by fitting
`a + b*n` over lengths 1..400 and report both coefficients: `EXT.7` had Supdb
the faster walker and `EXT.8` had it paying the larger constant. The marginal
cost of an entry falls from about 89ns to 15 over that range before settling
near 20, and a straight line through it lands its intercept *above* the measured
cost of a one-entry scan — 952ns of "fixed cost" for a scan observed to finish
in 692, and the same for LMDB and redb. Both quantities are now measured rather
than fitted: the floor is the observed n=1 point, the per-entry cost is the
difference quotient between the top two lengths. Measured that way neither axis
separates the engines, `EXT.7` moved to `fails` and `EXT.8` to `holds`, and all
three scan measurements finally agree. A model is a claim about the data and
belongs under the same gate as everything else; `full_range_fit` stays in every
`ext-sweep` record so the refuted one is visible rather than deleted.

The external suite repeats and interleaves its engines, like everything in
`src/bench/`. It did not always: it ran each engine once, and `EXT.1` read
0.70x, 1.03x, 0.998x, 1.13x and 0.85x across five such runs, flipping between
holding and failing on margins as small as 0.2%. Seven repetitions settle it at
0.866x with p=0.0106. If you add an engine or a metric there, it repeats too.
