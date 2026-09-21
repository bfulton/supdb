# Working in this repository

Supdb is a read-optimized embedded key-multivalue store. This repository is
the engine, its reader, and under `bench/` the suite that measures it
against LMDB and RocksDB. `bench/DESIGN.md` says how a thing is measured and
`bench/CLAUDE.md` carries that side's rules; this file is about how a thing
is built.

This file is notes to whoever picks the work up next with no memory of it:
dense, contextual, and it explains a rule by naming the failure that produced
it. `README.md`, a PR description and the crate docs are for people who either
already have the context or do not want it, so keep those factual, current and
simple: no counts that move, no standing figures (cite the claim id and the
reader gets a checked number instead of a snapshot; a figure attached to a
*change* belongs in the PR that made it, rounded), no history, and no narrative
of the change that produced the text.

## Layout

| path | what |
|---|---|
| `src/db.rs` | the engine: WAL with atomic batches, memtable, sealed segments, partitioned compaction, tombstones, `Txn`, and the `SegmentWriter` every segment is written by -- `docs/engine.md` |
| `src/format.rs` | the on-disk format's fixed quantities, owned by no writer |
| `src/block.rs`, `src/index.rs`, `src/flatindex.rs` | the format itself: blocks, extents, the flat key index -- `docs/index-theory.md` |
| `src/bytes.rs`, `src/blob.rs` | the read path over any byte source; compiles for wasm |
| `src/wasmapi.rs` | the C ABI the browser calls; hand-written because the module's size is budgeted |
| `web/` | the browser reader, its byte sources, the Worker it runs in, and the size control -- `web/README.md` |
| `tests/` | the engine's contract, the read paths held to each other, the format's damage cases |
| `bench/` | the benchmark suite: its own cargo workspace, the arms, the runner, the gate and the figures -- `bench/DESIGN.md` |

Two writers produce the format -- `Db` when it seals or compacts, and
`SegmentWriter` for sorted write-once input -- and three readers parse what
either produced: `Blob` over a mapped file, `Blob` over a copying source, and
`SparseBlob` over ranges. That is why `format.rs` belongs to none of them.

`block` and `index` carry a scoped `#[allow(clippy::all, dead_code)]`: style
not yet paid down, rather than code anyone may not touch. Nothing is exempt
from the format gate, and everything else holds to `-D warnings`.

## Running the checks

`sh scripts/check.sh` runs every group -- build, test, lint, wasm, bench --
and CI calls the same script with the same names, so a green run here is a
green run there. Use a group name to run one. `quick` is a group too, the
suite's three-minute measurement; it is not in the default set because a
timing run needs the machine to itself, and CI gives it a job of its own.

The tests leave their stores under the temp directory, one per test and
process (`supdb-next-<name>-<pid>`), so a day of test runs on one box is
tens of thousands of them and the disk they fill; `rm -rf
$TMPDIR/supdb-next-*` between runs.

Keep it that way. Every gate this repository has broken has broken the same
way: a check that was not running, or one reporting a verdict it had not
earned. CI never built the wasm module at all, so a link break in
`src/wasmapi.rs` survived until a toolchain update happened to surface it
locally. `scripts/fmt.sh` once swallowed "rustfmt could not run" behind
`|| true` and reported green for never having run, which is why it now tells
"formatting differs" apart from "did not run" and fails both. The tests ran
under the release profile, with overflow checks and debug assertions off,
so a prefetch span that added a rank to a scan's limit of `usize::MAX`
wrapped, passed every check green, and panicked in the first debug build
that reached it; the tests now run under `profile.checked`, the release
profile with both on, and the wasm module and the suite's binaries keep the
release profile because the checks cost code size there and time in a
measurement. A second definition of "the checks" is how the next one of
those starts.

The same failure has a shell-script form, and this environment invites
it: `set -e` does nothing here -- a subshell walks straight past a
failed `grep -q` with status 0 -- so a chain of `checks; then commit;
then push` that relied on it committed and pushed a tree the format gate
had just rejected. Gate on `&&`, on the verdict line and not on a `|
tail` that swallows the exit status, and prove the gate fires on a
known-red input before trusting it with a push.

## Profiling

Two instruments, and they disagree where it matters. Callgrind
(`valgrind --tool=callgrind --collect-atstart=no --toggle-collect='*supdb*Reader*scan*'`)
counts instructions exactly and attributes them without sampling noise,
which is what a fixed cost of a few hundred nanoseconds needs. It is
blind to waiting: on the scan path `Blob::scan_at` is 9% of the
instructions and a third of the time. Count what the toggle collects and
not what the probe times -- a warmup loop inside the toggle doubled every
figure once.

For time, `perf record -e cpu-clock`. There are no hardware counters
here: this is a Firecracker guest, `/sys/bus/event_source/devices/` has
no `cpu` and the CPU flags have no `arch_perfmon`, so cycles, cache
misses and branch misses cannot be had at all, and a container cannot
add them -- a container shares this kernel. Software sampling needs no
PMU and works. There is no `linux-perf` package for this kernel;
`apt-get install linux-tools-6.8.0-31` puts a 6.8 binary at
`/usr/lib/linux-tools-6.8.0-31/perf`, which samples fine against a newer
kernel. Callgrind's `--cache-sim=yes` is a model of a cache, not this
machine's, and is worth only what a model is worth.

Memory placement is a hidden variable here. The same bytes in a fresh
anonymous copy read 20% faster than the file mapping of them in three
runs and 20% slower in the fourth, paired window by window with the
same keys, and the engine's scan over the store paired at the blob's
own speed. The guest's pages land somewhere different each run and how
the host backs them cannot be seen from inside, so a question of ten
percent about translation, huge pages or folio size is not one this
machine can answer; `docs/engine.md` has the probe and both outcomes.

Profile a probe that does one thing. The scan probe's own `format!` per
iteration was 8% of its samples until the keys were built before the
loop.

Give the probe the suite's shape before believing it disagrees with the
suite. Four probes in one day failed to reproduce a figure from the lag
sweep and each was read as a refutation before it was read as a bad
model: one amortised the block builds over a hundred thousand scans
where the sweep does `size / scan_len` of them, one buffered its writes
differently from the mix driver, and all of them used forty-byte values
against the suite's hundred (`VALUE_SIZE`) and sixteen-byte keys. The
suite's own arms answer most of these questions without a probe at all,
and `bench ab` prices two of them in one process; reach for a probe only
when no pair of arms isolates what you are asking.

A pass that reads bimodal across rounds is two shapes, not noise. The
run keeper's lag pass at ten thousand keys read 2 µs a scan in four
rounds and 15-36 in three, and the rounds split on the probe's own
counters -- blocks built, the forms' bytes -- before they split on
time: the seal the burst triggers at that rung landed at its last
commit in the slow rounds, the writer's tables went with the publish,
and the pass built every block it met. Find the count that separates
the rounds before averaging them, and read the slow shape as a
measurement of its own.

## The suite lives in bench/, and it gates this repository

`bench/` is a time series. `bench run` measures every arm -- supdb's
shipping configurations and the comparator a user would otherwise pick,
durable against durable and buffered against buffered -- over a ladder of
store sizes, and writes one row of raw per-rep samples under
`bench/runs/<scale>/`. Nothing in a row is derived. The gate compares a new
row to the last ten rows of its machine class and fails when a quantity's
error bars lie entirely on the worse side of every one of them; a row
entirely on the *better* side is flagged rather than passed, because a
measurement that is implausibly good is a broken measurement until someone
looks. There are no claims and no expected states. The suite that had them
-- 183 claims adjudicated by `verify` -- was retired when its gate went red
on an engine head that had not changed; it is in the supdb-bench
repository's history. Nothing here cites one of its claims by id: an id
whose checker is gone is a pointer to nothing, and it comes back as a
number.

The ladder's small rungs are not a formality. A fresh memtable's first
write zeroed eleven megabytes of blocks it would never read, 3.7 ms that
five interleaved rounds of a probe at 300k keys could not see and the
quick row's ycsb-B at ten thousand keys, a pass under a millisecond,
showed as 3x. The scan snapshot's radix sort zeroed two histograms of
half a megabyte on every build, over a memtable that after the load
held nothing, and the faults on those pages were 400 us of the first
scan at every rung: the row's scan pass at ten thousand keys, a hundred
scans, sat at 0.64x LMDB's while a thousand scans through the probe
ran 1.4x it. Take the quick row before a change is called flat, and
read its smallest rung, and when a small pass trails, time its first
operation on its own.

Two consequences for code in this repository:

- **The comparison arms in `Options` are not dead code.** `cursor_merge`,
  `scan_merge`, `scan_snapshot_arena`, `flush_ranges`, `compact` and their
  kin each keep an older shape alive behind a flag because the suite prices
  the new shape against it in one process -- comparing two separate runs
  does not work, the unchanged comparators move by tens of percent between
  them. Removing an arm removes the experiment; check `bench/src/engines.rs`
  first.
- **No standing figure is written down here.** A number belongs in a row and
  a figure is drawn from the rows by `bench figures`. A figure attached to a
  *change* belongs in the pull request that made it, rounded.

## Invariants a change must not break

**The read path is synchronous, and that is the constraint rather than an
accident.** `flatindex::lookup` returns a borrow into the index section, and a
borrow cannot survive an `await`, so `Bytes` is synchronous and the `await`
lives in JavaScript: the browser downloads the object into OPFS once and
every read after that is `FileSystemSyncAccessHandle.read`, or it asks the
module for the ranges a read will touch (`Blob::ranges_for`, `open_ranges`,
`SparseBlob::dictionary_plan`), fetches them, and then the read runs
synchronously and cannot miss. An `async fn` anywhere under `blob`,
`flatindex` or `bytes` turns the API inside out and buys an Asyncify rewrite
against a module size that is budgeted. Format knowledge stays in Rust for the
same reason a plan is computed there: a superblock constant hand-copied into
the JS side has drifted once already.

**One writer, any readers, and nothing a reader follows moves.** A `Db` is
the writer and one thread's; a `Reader` from `Db::reader` is another
thread's, `Send` and not `Sync`, with the scan snapshot and the block tables
of its own. What they share is published whole: the segment set and the two
memtables as one `State` behind one pointer that a seal, a join, a merge or
a freeze swaps, and a memtable whose arenas, entries and index never move
once published. A reader pins the epoch it reads in through its slot in the
reader table, and the writer frees a replaced state or index only past
every pinned slot; it never waits for a reader. A read takes the state once,
at its start, and holds it: a read that loaded the state twice took an
entry from one memtable to the chains of another when a freeze landed
between, and three reader threads found it in their first minute. The
isolation is the reader's: `Latest` honours the memtable's watermark at the
last commit, `Snapshot` pins a state and a watermark, `Dirty` honours none,
which is what the writer's own reads do. A `#[cfg(test)]` module asks the
compiler that `State` is `Send + Sync` and `Reader` is `Send`; that is what
keeps a cell out of a segment, a blob or a memtable, since a raw pointer
behind an atomic would let one in unasked.

**`Blob::zero_copy()` stays true on the native path.** `Bytes` has two halves
for one reason: `read_at` copies and every source can answer it; `slice_at`
lends and only a source backed by memory can. Native takes the second for
every access and copies nothing, which is the axis `flatindex` exists to win
and the one a byte-source abstraction most easily loses. `tests/blob.rs`
pins it, because a native reader that started copying would still pass every
correctness check.

**The three readers agree, and are tested against each other rather than
against themselves.** The failure mode of a second read path is not a crash
but a browser quietly answering a different question from the server.
`tests/blob.rs` requires a lending source and a copying one to agree on
every key, value and count, and `tests/dict.rs` holds `SparseBlob`'s ranged
dictionary walk to the whole reader's `scan_counts`. Those checks have caught
real differences: a reader reporting the superblock's generation where
another reported the index section's, and a `value_bytes` that counted the
varint length prefixes it claimed to exclude.

**The magic moves when a reader from before the change would misread rather
than error.** The question is never "did the format change" but "what does an
old reader do with the new file". The per-extent count word and the `FIXED`
flag each re-decode a run under the wrong encoding in an old reader, so the
magic moved for both. The inline extension did not move it, because a reader
from before it errors on `Ext::INLINE` as an impossible block id; nor did the
key-section checksum row, whose header words are zero in every older file
and unread by every older reader. Decide which case a change is before
writing it.

**A checksum that cannot see a corruption is not a checksum for it.** Block
checksums cannot see a flipped bit in an index record -- a flipped `FIXED`
bit re-decodes the run with no error -- which is what the key section's row
of per-piece CRC32C words is for. A store's in-place-editable index carries
no row, because a record is published there with one aligned store into a
mapping readers already hold and a piece checksum cannot follow that
lock-free; `index_checksummed()` says which kind a reader has.
`tests/segwriter.rs` flips every seventh byte of a segment's key section and
requires each to fail the open. Its first run found a flip of the piece-shift
word that made the row look *absent* and opened clean, which is why a row
named with an impossible shift is damage rather than absence.

**Both writers emit the same shape unless a measurement says otherwise.**
`Db` passes `Options::inline_bytes` to its seal and its compaction exactly as
`SegmentWriter` does. This file once said the opposite, and three findings
were built on a difference that did not exist. If you are relying on a shape
difference between the two writers, measure it.

**`count_fixed` claims a count only when two independent quantities agree**:
the run is a whole number of strides *and* `Ext::last` is exactly
`(n-1)*stride`. Divisibility alone was tried: a run of 17 variable-length
values divided exactly by a stride of 4 and the first version answered 23.
Two quantities is still not a proof, so the contract is that the caller
knows its schema; the `FIXED` flag makes it exact where the writer could
prove it.

**Crash discipline is an order, and every window in it is survivable.**
Commit is a WAL append and one fdatasync; the batch is durable or its tail
frame fails its CRC and replay stops before it. Seal is write to a temp name,
fsync, rename into place, then publish -- the manifest written, fsynced and
renamed, and the directory fsynced once for the segment's entry and the
manifest's together -- then reset the WAL; a crash between any two of those
leaves either a WAL that replays the whole memtable or a complete segment
plus a WAL whose sealed prefix is skipped by sequence, and a segment the
manifest never named is swept at open. A store's first seal under a flush
that partitions writes the partition's name directly when the piece is
tombstone-free and fits one, since that is what the flush's promotion would
link it as under a second publish; the first version named it so for a
store that does not partition on flush too, and the suite's ingest arm,
which keeps its piece a piece, scanned three times faster for one row.
Replay applies the frames between commit frames whole or not at all -- a
partial batch used to replay as whole, and the first test written against
the contract found it. `settle` is what joins an in-flight seal; `sync` does
not, and an experiment that assumed otherwise measured a 286,000-key seal it
had never joined.

Ordered ingest has its own log and one more window. A run of keys above the
store's greatest goes to an ordered memtable and, at each commit, to a
segment open for append: the batch's records, then a commit marker carrying
their CRC, length and count, then one fdatasync on that file. The WAL never
sees them (`direct_ingest: false` is the arm that still sends them there).
Recovery walks the temp file's record stream to the last marker whose three
quantities agree with the records before it and rewrites what precedes it as
a piece; what follows is a batch nobody was told was durable. The CRC is
there because a crash can land the page a marker is on and not a page of its
batch, and a record whose bytes came back zero still parses. The close is
the seal's shape with one difference: the finished segment is hard-linked
under its piece name rather than renamed, and the temp name stays until the
manifest names the segment, because a name the manifest lacks is swept at
open and the temp name is what recovery reads; a temp name whose id the
manifest already names is that window's leftover, removed at open. The first
version of this path kept the hash memtable for reads and closed the segment
on the commit thread, and measured slower than the WAL it bypassed at three
million keys and at thirty: the memtable insert was a quarter of the load
and the close a tenth. A path that keeps the structure it was built to
bypass has bypassed nothing.

## Shapes the bugs come in

The reproducers for the previous engine's defects retired with it. The
shapes are worth keeping, because this engine can take them too.

**A derived thing keyed by position outlives the thing it was derived
from.** A level-0 piece's ranks -- each key's cut in the partition it is
aligned to, taken once when the piece is published -- were kept for the
piece's life, and a piece sealed while a merge of its range ran was kept
across the merge's publish under a new partition over the same fences, so
every cut below a key the merge folded in was behind by that key. Nothing
raised in release: a block built through a stale cut emitted the piece's
key where the old partition had it. The checked profile's tests reached
it as a debug assertion in about one run in twenty, once the seal's
timing put a piece inside a merge, and the chain that landed the reader
handle went red on a change of four ampersands. The ranks now carry the
blob id of the partition they were taken against and are taken again at
a merge's publish; `tests/db.rs` makes the merge long and the seal short
so the case is reached every run. The rule: a cache derived from one
object is keyed by that object's identity, never by its place or its
range, because a publish that keeps the position and replaces the object
is exactly what a merge does.

**A replay that builds the thing without the last step of the live
path.** A reopen replays the WAL into the memtable through the same
`append` and `delete` a live write takes, and stopped there: the live
path ends in a commit that records the watermark, so the replayed table
carried none, and a reader handle under `Latest` saw nothing of what the
WAL had committed until the writer's first commit after the open. Every
test read a reopened store through the writer's own handle, which
honours no watermark. The builder ahead of the reader found it, reading a
reopened store at the watermark and building as if the memtable were
empty; the open commits what it replayed now, and a test reads a
reopened store through a handle. The rule: a path that rebuilds a
structure ends where the live path ends, and a reader that honours a
mark the live path sets is the test that tells the two apart.

**A path only one arm exercises is a path nothing tests.** A delete was never
marked dirty, and the checkpoint asked to carry it dropped it, leaving the key
readable at its old extents. It was invisible for as long as every insertion
forced a full rewrite, because a rewrite reads the tombstone directly. Turning
a flag on is what exposed it, and the bug was older than the flag.

**The sharp edges of a log are in the bookkeeping around it, never in the
append.** A value-carrying log queued a key twice when it sealed, re-queued
and sealed again inside one interval, and logged the same delta twice. A
replay applied records over newer index state because nothing said which was
newer. A durability point acked before the table that named its blocks was
synced, so a crash at exactly that point left a log naming blocks the
recovered table did not have. The WAL recycler has an edge of the same kind
at the device: the page cache sizes a folio by the write that creates it, so
a WAL pre-written in 1 MB pieces made every 100 KB commit after it cost 11x
its bytes; in 4 KB pieces, 1.04x.

**A clean test result proves nothing about a path the test never took.** The
first reproducer for the replay-ordering bug came back green and was
inconclusive until a path trace showed it had never reached the arm it was
written for. `tests/db.rs` emulates every crash window by constructing the
exact on-disk state the window leaves behind, for that reason.

**A size chosen before the thing is measured is a guess, and a wrong guess
here is never a fault.** A segment's head reserve has to be sized before the
first key is written, and it holds the block table, the checksum row and
copies of the fence and the directory. Too small and the pieces that do not
fit go after the data, costing the sparse reader a round trip; too large and
every segment carries zeroes forever. Neither raises anything, so a floor
under it was wrong in both directions at once and nothing said so. `reserve`
computes it instead, and it computes it by *calling the planner the writer
calls* rather than by restating the layout -- a second copy of that
arithmetic is a second definition of the format, and two definitions drift
the first time one is edited. What is left over is one four-byte rounding,
because the checksum row's length depends on where the section lands and
that depends on the answer.

**Arithmetic that underflows fails quietly and expensively.** A size-class
calculation underflowed for every block of 4 KiB or less, which is every
block a store of short postings produces. Debug builds panicked; release
builds wrapped and reserved 7,680 bytes for a tiny placement, so every small
store paid about 1.9x on every section it wrote -- visible as size, never as
a fault.

**A zero-length compare is not free when its pointer is dangling.** An
open fence was `Vec::new()`, and every read that reached the first partition
or a level-0 piece, and every scan that started there, compared its key
against that empty slice. The compare is a `memcmp` with a length of zero,
which glibc's AVX-512 variant answers by building a byte mask from the
length and issuing a masked load through one operand and a masked compare
through the other before anything looks at the length: a zero mask
suppresses the fault but not the address translation, and an empty `Vec`'s
pointer is a dangling non-null address no page table maps, so every call
paid a failed page walk that no TLB entry could cache. Measured with a
run-time length of zero: 81 ns with the dangling pointer as the left
operand, 19 ns as the right, 2 ns on a real pointer. The scan's cursor
compare had the fence on the left, a fifth of a scan's fixed cost at 300k
keys; the read path's `may_hold` had it on the right, and the read
measurement did not move. It was found by peeling a scan apart one call at
a time after every layer of the engine had been cleared. The fence
compares now go through `Seg::below_lo` and `Seg::cursor_from`, which do
not compare an empty fence at all. The rule is general: a slice that may be
empty, and whose bytes live in a `Vec`, must not be handed to a byte
compare without an `is_empty` check first, and a zero-length operation is a
real operation until a measurement says otherwise.

**A log read past the mark it honours.** A reader handle under
`Latest` reads the memtable's write log to settle its cached blocks,
and read it to its end: a key's write staged and not yet committed was
settled under the committed watermark, which left the value out as it
must, and when the commit landed the log had not moved, so the block
kept the old run for as long as it was cached while the point read
beside it, through the memtable, answered the new value. Every test
read a written key through the writer's own handle, which honours no
mark, or through a handle that had not cached the block. The handle
reads the log only to the length it had at the commit whose watermark
it holds, taken before the watermark since a commit stores the length
first, and the builder ahead takes its three quantities in that order
too. The rule: a structure kept current by a log is current to a
position in it, and the position a handle may read to is the one its
isolation names, never the log's end.

**A slot table whose slots share a line.** The reader table's slots
were adjacent words, eight to a cache line, and every read stores its
handle's slot twice, at the pin and at the unpin, so four handles
claimed in order stored to one line from four cores on every read, and
four threads read a partitioned store of ten thousand keys at one
thread's rate. Nothing raised and nothing was wrong: the first row
with threaded reads showed four threads at 2.5x one, and the probe with
the suite's shape showed 1.0x. A slot per line reads 3.8x. The rule: a
word one thread writes on every operation lives on a line no other
thread writes, and adjacent words, the layout nobody chose, are the bug
until the layout is chosen. It came back once as counters: every scan
through a handle bumped seven to thirteen shared statistics, and four
handles read the threaded scan mix at ten thousand keys at 0.46x of
LMDB, 0.91x with the counters off. A handle's statistics live on its
slot's line now, and the one word the writer must see, the regime's
scan signal, a handle bumps once per commit and not once per scan.

**A join that walked the long side for an empty short one.** A block
table maps a source's positions against a partition's block boundaries
by walking the source and reading each boundary key once, and read the
boundary before asking whether the source had a position left. A
snapshot of no unsealed keys is what every scan pass over a store just
flushed starts from, so the first scan read a cold line per block for
nothing: 500 µs of a first scan of 700 at three hundred thousand keys,
a tenth of the suite's scan pass at every rung from a hundred thousand
up, and nothing raised because every bound was right. The suite's own
rule found it -- time the pass's first operation on its own -- and the
same scan carried two more costs of the kind: the ordered index's top
level built at the first seek, and a builder thread spawned to find
nothing to build. It came back at once for the store the mixes leave:
the same walks with sources that have keys, a record read per boundary
and per piece key, made again by every handle and by the writer at
every published state, an eighth of each thread's pass in the threaded
scan mix. The rule: a walk over two sorted sides costs the side the
loop runs over, so it stops the moment the other side is spent, reads
the cheapest form each side has (the index's heads before the
records), and its answer, a function of two immutable objects, is
taken once and kept with one of them under the other's identity; and a
structure a pass needs once is built where the pass is not timed -- at
open, by the seal that made the segment -- or not at all.

**Two merges over one range that did not take pieces by age.** Level
0 is newer than the level below it, every piece of it, so a merge that
writes the level below may take only a prefix of a range's pieces by
age. A piece merge held a range's pieces as inputs; a partition merge
started beside it excluded those inputs and took the piece sealed
after them, and folded the newer piece under the older ones. A key's
values came back out of order, and a key whose older values a
tombstone had masked lost them when the partition merge dropped the
tombstone as one that had nothing older left to mask. The crash oracle
found it in its first run: nothing raised, every file was well-formed.
No partition merge starts while a piece merge runs; a piece merge may
start beside a partition merge because that merge's inputs are the
range's oldest pieces. The rule: two writers of one level order must
agree on which of them takes the old end, and an exclusion by name is
not an ordering by age.

**An order that held by name.** The live segments sort partitions first and
then the level-0 pieces, and the pieces sorted by fence and then by name.
Every piece a seal makes after the first partitioning is named `pcs-` with
its fences; one sealed before it is named `seg-` with the empty fence, and
so are the pieces aligned to the first partition, and `pcs` sorts before
`seg`: an older piece came after a newer one, a read took the older piece's
tombstone as the newest source and answered a version it had already
answered past. It held for as long as the merge took the unaligned piece
before a read met the pair, and reader threads beside a writer that sealed
every few hundred puts met it in their first minute. The pieces over one
fence now order by the sequence their names carry, and a read's
"oldest to newest" is a property of that order and not of the names.

**A form dropped by its writer stayed published.** The writer keeps
its own copy of every block form and publishes a clone for readers to
take; a reader takes a published form as the block and, with the table
complete, an empty slot as clean. When a block's overlay outgrew every
form but the wide one, the writer dropped its copy and built the block
wide, a form it never publishes, so the block's published slot kept
the form from before it went wide, and a reader handle read the last
block of a store short of three hundred of the five hundred keys
inserted past the end. No test read a block gone wide through a
handle; the writer's own reads walk its own tables and were right. The
slot now carries a mark whenever the writer drops a block's form or
holds it wide, and a debug assertion at the settle holds the rule: a
block the writer has no form for has none published as the block. The
shape is general: two copies of a structure, one kept current and one
handed out, drift the first time the current one is dropped rather
than replaced, and the handed-out copy needs a tombstone for that.

**A flag set before the call that resets it.** `maintain_forms` set
"the writes are filed into the tables" and then read the log, and the
log read, finding the generation moved, dropped the tables and cleared
the flag. Nothing set it again when the commit's own fill made the
tables, because the two paths that had always made them, the scan and
the install, set it themselves beside their call. So a writer whose
first table over a state was the commit's fill -- a handle's scan asked
for the maintenance, not the writer's own -- filed nothing into its
forms for the rest of the state, and its scans answered a key's values
short of every write since while the point read beside them answered
right. Every test scanned through the writer before it wrote, which
made the tables the other way. The flag is set where the table is made
now. The rule: a flag that means "this structure exists" is set by the
code that makes the structure, never by a caller that expects to, and
a reset on one path is a reset on every path that shares it.

**A divide an entry in a walk that needed none.** The record walk
divided a run's length by its count for the stride, and `chunks_exact`
divided again for the remainder: a dependent chain of two 32-bit
divides, in a loop of twenty-five cycles an entry, for a quantity that
is one in the common record. Callgrind showed it as five instructions
on one line; the divider's latency is what it cost. The rule: in a
loop measured in cycles an entry, read the assembly for the
instructions whose cost is not their count -- a divide, a call through
a pointer, a store the next load depends on -- and give the common
shape a path without them.

**A sentinel that crosses the wasm boundary changes sign.** A wasm `u32`
arrives in JavaScript as a signed i32, so a failure sentinel of `u32::MAX`
arrives as -1 and a comparison against 4294967295 can never match. Every
error check in `web/supdb.mjs` was dead for as long as it compared raw, and a
reader over an object that failed to open answered `[]` for every key. The
convention is normalize to unsigned at the boundary, compare unsigned. In the
same file: the host imports are named with `wasm_import_module = "env"`
because the bare `extern` block silently stopped linking on a toolchain
update, and the host ABI's 32-bit offsets refuse an object at or over 4 GiB
at open rather than wrapping.

## Standing limitations

These are the engine's, as opposed to refuted predictions. Each is a curve
in the suite's figures, at `full` scale where it says so:

- Out-of-core reads fall off a cliff. Once the file exceeds the memory that
  can cache it, throughput drops by orders of magnitude and the latency
  distribution goes bimodal -- every miss is a synchronous page fault. This
  is the mapped read path's shape, not a bug; it is the `read` curves past
  the memory line.
- The durable ordered load trailed LMDB and RocksDB until ordered ingest
  went straight into segments; the suite's rows since say where it stands,
  and shuffled arrival still inverts both. Quote the pair, never one: the
  `load` and `load-shuffled` figures.
- The index layout study found smaller and faster points on the frontier
  that the shipping layout does not occupy (`docs/index-theory.md`).
