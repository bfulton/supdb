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

No module carries a blanket lint allowance; `block` and `flatindex` allow
`dead_code` on the wasm target only, where their writer halves have no
caller. Nothing is exempt from the format gate, and everything holds to
`-D warnings`.

## Running the checks

`sh scripts/check.sh` runs every group -- build, test, lint, wasm, bench --
and CI calls the same script with the same names, so a green run here is a
green run there. Use a group name to run one. `quick` is a group too, the
suite's three-minute measurement; it is not in the default set because a
timing run needs the machine to itself, and CI gives it a job of its own.

The tests leave their stores under the temp directory, one per test and
process (`supdb-next-<name>-<pid>`), so a day of test runs on one box is
tens of thousands of them and the disk they fill; `rm -rf
$TMPDIR/supdb-next-*` between runs. On a box other runs share, give each
run a `TMPDIR` of its own and clean that: the glob takes every process's
stores, and a run whose store another run's cleanup removed mid-test
fails with a bare `NotFound` from whatever file operation came next --
eight tests of one run, one of them in six runs alone, before a syscall
trace showed the store's directory gone and nothing in the process that
removed it.

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
against the suite's hundred (`VALUE_SIZE`) and sixteen-byte keys. A
fifth skipped the `sync` the suite's load ends in, so its store at three
hundred thousand keys stayed two pieces through the whole sweep, where
the suite's second flush had joined the partitioning, and read every
point at a third of the suite's rate. A sixth read the store's counters
between a burst and its scans, and a counter read takes the writer's
upkeep home, waiting out the thread's pass in flight before the clock
started: it read the lazy freeze's pass level where the suite read it
at four tenths, and the wait it hid was half of the suite's first scan.
Read nothing between the phases the suite times that the suite does not
read. The
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

A window of a mix is several kinds of operation, and a rate over it
names none of them. The mixes' windows around a seal read at four to
ten times their neighbours, and two probes read that as the reads
slowed beside a seal in flight -- the shape a known problem has --
until a third split each window into its reads and its commits: the
reads were flat to the nanosecond and one commit in the window was
the whole excess, the freeze's, at five milliseconds. Time the kinds
apart before naming the one that is slow, and when the excess is one
operation, decompose that operation next.

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

**Two publishers, one pointer, and nothing a reader follows moves.** A
`Db` is the writer and one thread's; its segment work (`Maint`) -- a
seal's landing, the merges, the promotions, the manifest and the WAL
retirement -- runs on a thread of its own (`publish_in_background`), or
inline where the writer drives it; a `Reader` from `Db::reader` is
another thread's, `Send` and not `Sync`, with the scan snapshot and the
block tables of its own. What they share is published whole: the segment
set and the memtables, the live one and the frozen list, as one `State`
behind one pointer, swapped by a
compare-and-swap against the state it was made from and made again over
whichever won. The writer changes only the memtables (a freeze, a switch
of memtable) and the segment work only the segments -- with one exception
the writer consents to at the hand-off: the landing that retires a table
the writer handed to a seal without freezing it installs an empty live
table in its place, and the writer, which writes nothing into a handed
table, takes whichever table it finds live at its next write -- so a
retry takes the other's half afresh and nobody holds anybody off. A
memtable's arenas, entries and index never move once published. Everyone
who reads a state
pins the epoch it reads in through a slot in the reader table -- a handle
for each read, the writer for each of its operations, the segment work
and the upkeep thread for each of theirs -- and whoever replaces a state
frees it only past every pinned slot; nobody waits for a reader. A read
takes the state once, at its start, and holds it: a read that loaded the
state twice took an entry from one memtable to the chains of another when
a freeze landed between, and three reader threads found it in their
first minute. An operation that publishes moves what it holds to what it
published, and the writer's operations hold one state each: what the
segment work publishes meanwhile, the writer sees at its next. A table
the writer hands to a seal without freezing it -- `sync` under
`adaptive_shape` hands the live table whenever it holds anything, no
table is handed already and the segment work has a thread of its own, a
seal in flight or not, since seals in flight may fill the frozen list --
is replaced by whichever side publishes first: the
writer at its next write, freezing it under a fresh table once the frozen
list has room, or the landing, installing an empty one, each by
compare-and-swap on the same pointer; and the writer writes nothing into
it after the hand-off. The
isolation is the reader's: `Latest` honours the memtable's watermark at the
last commit, `Snapshot` pins a state and a watermark, `Dirty` honours none,
which is what the writer's own reads do. A `#[cfg(test)]` module asks the
compiler that `State` is `Send + Sync` and `Reader` is `Send`; that is what
keeps a cell out of a segment, a blob or a memtable, since a raw pointer
behind an atomic would let one in unasked.

**The writer's upkeep is lent, never shared.** By default
(`Upkeep::Background(1)`) a commit lends the writer's `FormsState` --
its block tables and scan snapshot -- to a thread of the store's own and
returns without waiting for it; at level 2 (`supdb-hold`) a commit that
would leave its batch to the next scan returns only once the thread has
filed it. One thread holds it at a time, and one word says which:
every hand-over is a single operation on that word, the side giving it
up fills the cell before it and the side taking it empties the cell
after, so nothing in it needs to be `Sync`. A writer's touch of it takes
it back, waiting only for a pass in flight, and never on a lock: the
thread is niced, and a lock the writer took at every commit was one the
thread could be preempted holding. The thread pins a slot for each pass,
so a publish from the segment work needs nothing of it: the forms go
across as copies the publish makes (`State::carry_published`) and the
writer's own tables follow at its next look at the log
(`Reader::rebase_tables`). The writer's other publishes take it back
first; the freeze does not, swapping the tables and touching nothing
else (`Options::freeze_settles` is the freeze that did, kept for
pricing), and the next look carries the
tables across it as it does a landing, the frozen table's unsettled
writes kept to settle through that table and every table live since the
look kept for it in `State::replaced`. The forms a pass publishes carry the commit the writer named,
never the latest, or a handle at the latest takes forms without the
writes between; and `settle` joins it before anything else, because
every other join touches it. The threaded test runs at each level, since
a level that holds hides what a level that lends exposes.

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
magic moved for both, and so did the compact record, whose flag bit an old
reader takes for 32,768 extents and may find bytes enough to read as them.
The inline extension did not move it, because a reader
from before it errors on `Ext::INLINE` as an impossible block id; nor did the
key-section checksum row, whose header words are zero in every older file
and unread by every older reader, nor the segment's payload in a spare word
of the superblock extension, for the same reason: a zero there reads as
"not recorded" and the engine estimates it. Decide which case a change is
before writing it.

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
rename into place, publish to readers, fsync, then the manifest -- written,
fsynced and renamed, and the directory fsynced once for the segment's entry
and the manifest's together. The seal leaves the WAL open and ends at
its sequence, since replay skips every record a manifest covers; the log
rotates by size at a commit, synced and its directory entry synced
first, because replay refuses a sequence gap across files, and a closed
file retires at the next seal's durable landing, whose manifest covers
it (`seal_rotates_wal` is the arm that syncs and rotates at every seal
instead). Readers wait for no
fsync and the manifest does, and no manifest is written while a seal's
segments are published and unsynced -- it would name a segment that may be
torn and cover a sequence the WAL still has to hold -- so a merge that
finishes in that window lands after the seal (`Maint::collect` holds it,
`publish` asserts it). The seals in flight are a queue (`Maint::sealing`),
landed in the order they were handed and each in its two phases, and only
the oldest lands at all until it is durable, so one seal at most is between
its phases and no manifest covers a later seal's records before an earlier
one's are durable; the frozen tables are a list in the queue's order,
up to `FROZEN_CAP`, and a landing retires its front; a table `sync`
hands without a freeze is a seal in flight beside theirs that joins the
list's back only when the writer freezes it; the writer freezes into
room and waits only when the list is full, for the oldest's readable
landing; and the count of seals
(`in_seal`) is read before the state whoever decides against it, so a
count of zero is one whose landings the state shows. A crash between any
two of those leaves either a WAL that replays the whole memtable, with a
segment the manifest never named --
possibly torn -- swept at open, or a complete, named segment plus a WAL
whose sealed prefix is skipped by sequence. Every store has a manifest from
birth for that reason: without one, open takes every `seg-` file as live
and skips the WAL behind it, which was safe only while a segment was
fsynced before it was renamed, and that scan is kept for stores from before
manifests alone. A store's first seal under a flush
that partitions writes the partition's name directly when the piece is
tombstone-free and fits one, since that is what the flush's promotion would
link it as under a second publish; the first version named it so for a
store that does not partition on flush too, and the suite's ingest arm,
whose seal must leave a piece, scanned three times faster for one row.
Such a flush now promotes the piece itself, by link, and hands what
promotion cannot tile to a background merge the writer publishes at its
next commit (`flush_schedules`): before it the background waited for
`l0_trigger` pieces, a store of fewer stayed pieces for good, every scan
took the merge path, and the block cache had no partition to cache.
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
it holds. The first version of that took the length and then the
watermark, two loads of words the commit stored in that same order,
and a load that sees a store learns nothing of the stores after it: a
handle took one commit's length with the watermark before it, settled
that commit's writes under a watermark that hid them, read on past
them, and kept the old values for the rest of the state -- the same
failure, one commit wide. A test that reads a key and then scans from
it found it in about one run in two. A commit now writes its three
quantities into the mark the last commit did not write and then names
it, and a reader takes a mark only while it stays named. The rule: a
structure kept current by a log is current to a position in it, and
the position a handle may read to is the one its isolation names,
never the log's end; and quantities a reader must take together are
published as one, because no order of separate stores and loads makes
them one.

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

**A share taken of a sum that counts one thing twice.** The settle
bound is a share of the store's keys, and it summed every live segment:
a level-0 piece sealed from a burst of updates holds keys a partition
holds already, so each seal grew the bound, a commit's batch stopped
crossing it, and the commits after a seal's publish settled every other
time. Which of them was the burst's last came down to the seal thread's
timing, and the lag pass at thirty thousand keys read two shapes, one
in three reps at half speed, with every count the probe kept identical
in both; the first scan was filing a thousand writes no counter
counted. The rule: a share of the store is taken of its distinct keys,
the partitions', never of a sum over versions of them; and a pass that
reads two shapes while its counters agree is paying for something no
counter counts, so time its phases.

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

**A value built a field at a time and handed back whole.** The scan
snapshot's cursor, generalised to a base a frozen table, folded each
run's head into one value -- a key's slot and run in every table -- and
returned it with the mask: the fold wrote it to the stack in stores of
one, four, eight and sixteen bytes, and the return copied it out in loads
of eight and sixteen that spanned them, and a load that spans stores of
other widths is not forwarded from them, twice a key. Callgrind counted
the same instructions in the cursor before and after; perf put four and a
half times the samples on it, 42% on one sixteen-byte load from the
stack, and a lag point read 0.77x in every round of a sitting while the
point after it read 2.8x, its blocks built by the upkeep thread in the
time the slower scans gave it. The cursor finds the key and the mask as
two scalars now and reads the entries only where a caller asks. The rule:
a value assembled field by field and then moved whole costs a stall per
move that no instruction count shows; keep a hot path's results in
scalars, and when time moves with no instruction to account for it, look
in the annotation for a load from bytes just stored in pieces.

**A join that took back what it was about to wait for.** Under
`Upkeep::Background` the writer's upkeep is lent to a thread, and any
touch of it by the writer takes it back, waiting only for a pass in
flight and taking whatever the thread has not begun untouched. `settle`
joined the seal, the merges and the builder first and the upkeep last,
and each of those joins touches the upkeep, so by the time `settle`
asked the thread to finish, the writer already held it and nothing had
run. A model test that asked for the thread's work after a settle found
no pass at all. The rule: when joining several workers and joining one
resets another, join the one that would be reset first; and a test of
a background worker asserts that the worker did something, since every
check is green on work that was never handed over. The same test found
the converse once the commits held for their pass: under a hold the
writer commits nothing past the commit it named, so a pass stamping its
forms with the writer's latest commit instead of the named one is
invisible there, and only the level that never holds catches it. It
came again when that level became the default: the builder's check,
after the commit had lent its upkeep, read the upkeep for whether a
builder ran, took it home and dropped the commit no pass had begun,
so the arm that builds ahead at every publish had none of its commits
filed; and at a new generation it started a builder over the one whose
posted forms were the reason that commit's maintenance was due, which
the commit's own maintenance, running before the check, had always
taken first. A commit that lent hands the upkeep back if anything
after the hand-over took it home, and a builder whose forms wait to be
installed is left alone, as a running one was. The test that found
both failed 28 runs in 48 under contention and none alone.

**A format change priced on the path that reads it in bulk.** The
compact record was priced against `supdb`, whose scans walk a
partition's records in one loop that reads the compact form in place,
and read faster there. The buffered arm keeps its level-0 piece a
piece, so its scans take the merge path, which asks each source for a
key and then for its values, and `key_at` decoded the whole record for
the key: every key compared rebuilt the compact extent, twice with the
values read after, and those scans ran at half their rate for a week
with no row taken to show it. The quick row's gate named the arm; a
bisect in one sitting against its comparator named the two commits.
The rule: a change to what a record holds is priced on every path that
reads one -- the bulk walk, the merge's per-key calls, the point read
-- and an accessor answers only what it is asked, since a key read is
the most frequent call a merge or a seek makes.

**A list of raw pointers drops the pointers.** A replaced canonical
form and a replaced scan snapshot wait in lists until every reader
that could hold them has left, and the sweep freed each by hand as it
removed it. Nothing freed what was still waiting when the store
closed: the lists dropped their wrappers, and a wrapper of a raw
pointer frees nothing. The suite's stores closed with up to two
hundred forms and a snapshot waiting -- a pass's closing threaded
scans republish the blocks the mixes dirtied, and nothing sweeps after
them -- and `tests/leak.rs`, which counts every allocation, kept a
quarter of a megabyte per closed store until the wrappers were given a
`Drop` that frees. It was the suite's runner that showed it: with
freed memory returned between passes, what the process still held
climbed about 35 MB a rep at a million keys over every arm, and held
level once the wrappers freed. The rule: whatever owns a pointer it will free is a
type with a `Drop`, so the owner's end frees it, not only the path
that remembered to.

**A carry that required nothing to have moved.** The freeze carried
the writer's tables only over the state it had prepared them against,
and the prepare was a settle of the whole backlog, milliseconds on the
writer's path. A seal's landing from the segment work fell inside it at
the second freeze of the buffered lag sweep's largest burst, so the
freeze carried nothing and dropped the tables; every freeze and look
after it then refused for want of a table some scan had used, the
burst ended with none, and the pass built every block of the store at
a seventh of the rate the rows had shown. Retrying the prepare over the
landing restored the pass and doubled the burst, the settle being the
writer's; the freeze swaps the tables and nothing else now
(`freeze_settles` off), and the look carries them. The rule: an optimistic check
that fails falls back to work, never to a state nothing recovers from;
and a fallback that drops a structure must not also be what stops
anyone rebuilding it.

**A publish that dropped the marks an empty slot is read by.** A reader
takes a published form as the block and, with the table complete, an
empty slot as clean, so the writer publishes a mark for every block it
holds no form for, and the marks travel from state to state with the
copies a publish carries. The freeze that carries nothing published a
state with no form and no mark while the writer's tables still called
themselves complete, and a handle read the last block of a store
without one key the frozen table held for it -- the writer, walking its
own tables, was right. The tables carried across a freeze are
incomplete now until the fill makes them whole. The rule: where an
absence means something, whatever drops the things it is the absence
of drops the claim that gives it the meaning.

**A note that held one of the things its look would need.** The look
that carries the writer's tables across a freeze reads the writes it
has not read from every table live since its last look, and its note
held the one live at that look and nothing after it. At the buffered
lag sweep's largest burst a table froze and landed every few
milliseconds while a pass ran longer than that, so a table went live,
froze and landed between two looks, nobody held it, and the look
dropped every table: the thread rebuilt the store in the burst, and
when the writer's first scan was the look, the pass built every block.
The state keeps the tables replaced since the upkeep's last look now
(`State::replaced`, trimmed at the count the look publishes). The rule:
what a reader that runs behind will need is kept by the side that
retires it, until the reader says it has passed it, never by the
reader's own record of where it last was.

**A fast path gated on the case it was written for.** A settle copies a
written key's run from its own chain when the chain holds a tombstone,
since a tombstone masks every older source, and the gate asked whether
the write was the live table's. A write read from a table frozen since
-- what the freeze that leaves its carry to the next look settles --
has the same chain and the same tombstone, and took the general path,
a seek of every piece standing, at about six times the price: the
writer's first scan after the buffered lag sweep's largest burst spent
thirty milliseconds there. The rule: a fast path is gated on the
property that makes it valid, not on the source it was first written
for.

**A pass that reported done for a turn it skipped.** The upkeep thread's
pass checked that the state was the commit's by its generation and, where
a landing from the segment thread had moved it between the hand-over and
the pass, filed nothing -- and reported the commit filed all the same. A
commit holding for the thread (`Options::upkeep_lag`) was released by
those reports, the writer ran on through its freezes, and the thread's
real look fell past the tables the state keeps for it (`State::replaced`):
the writer's first scan after the burst dropped its tables and built the
store, in one bounded burst in four, with every count the probe kept the
same in both shapes but the blocks built. The pass runs now against
whatever state holds the table its bounds are of -- a landing moves the
generation, not the table -- and reports filed only for a pass that ran.
The rule: a worker reports progress for work it did and never for a turn
it took, and a check that a thing is the one named compares the quantity
the work depends on, not a counter that other things also move. It came
back as a bound: the pass that files at most a share of the store took
its share from the commit's generation against the log position it had
read to, a landing between the two read as a position of zero, and the
pass read nothing and re-posted itself seventy thousand times in one
burst. The bound is applied where the position is known now, in the log
read and the frozen tables' settle, and never computed from a count
beside it.

**A derivative walked for every key when a few blocks asked.** The
snapshot's cuts -- where each unsealed key cuts the partition's walk --
were taken for the whole run in one forward walk when the bounds were,
since the first version built every overlaid block and needed them all.
An upkeep pass that replaces the snapshot took them again: half a
millisecond over seven thousand keys, on every other pass of a dense
burst, for the eight or twenty blocks the burst had the thread build. A
block's keys cut inside its own sixty-four ranks, so each block's cuts
are walked on its first build or walk and no other's, seeded at the
block's first rank, behind a per-block flag published after them since
the bounds are shared through the snapshot across threads. The rule is
the one the empty-join entry gave, read the other way: a derivative of
one object per item is taken per item at the item's first use when the
items used are a fraction, and the whole-run walk is a choice to be
re-made when the fraction changes.

**A form dropped after the copy stood as the block.** A publish from
the segment work copies the old state's published forms into the new one,
and the writer marks a block dirty at its next look unless the new state
holds the very form its table does. The first version compared only the
blocks the table held a form for: a pass that shed or widened a block over
the old state after the copy left the old form standing as the block in
the new one, and three reader-thread tests read a key's older value after
its newer one. The rule is the one "a form dropped by its writer stayed
published" already gave -- a copy handed out needs a tombstone when the
kept copy is dropped -- and it applies to every copy, however it was made.
It came a third time in review, before it landed: the landing that
installs an empty live table for a table `sync` handed without a freeze
copied the published forms as every landing does, and a freeze files the
writer's backlog before it publishes where a hand-off files nothing, so
the copies were current to a position short of the handed table's end
while the piece the landing published held the rest; the writer's next
maintained commit stamped the new state's position over them, and a
handle's scan at that commit read a key short of the last batch while the
point read beside it, through the piece, was right. That landing carries
no form now, as a freeze that carries nothing does. The corollary: a copy
is current to the log position its source was filed to, and a publish that
carries copies across a change of the live table has to know that position
was the table's end.

**A rename under a merge.** A promotion hard-links a partition under a
new name to close its fence, and a merge's landing removes its inputs by
name. A promotion decided while a merge held that partition left it live
beside the outputs that replaced it: two generations of partitions over
one range, and the next merge over them found keys outside every fence it
was given. Inline it needed a merge still running at the next landing;
with the landings on a thread of their own it happened in the first run.
Nothing a merge in flight holds is promoted now. The rule: an identity a
job holds by name is not renamed under it, and a check that the job's
inputs are still what it took is the job's to make, not the reader's.

**A piece cut at fences a merge replaced before it landed.** A seal cuts
its table at the partitions' fences as they stand at the freeze, and a
merge may land before the seal does. Reads were ready for a piece over
ranges it was not cut at -- a store with one consults every piece -- and
merges and promotions were not: a merge over some of the ranges such a
piece covers took it as an input and refused its keys past them, and a
promotion beside it would have made pieces younger than it into
partitions, which read as older than every piece. A merge's fences are
grown to cover every input now, and a range such a piece overlaps is
merged, never promoted. The rule: when two jobs cut one space and either
may land first, whatever reads the result must take either order. A
promotion at every landing made the stale cut the common case: an
ordered load's tail was named against the last partition while the
piece ahead of it was in flight, that piece's landing promoted it and
closed the partition, and the tail landed wide over two ranges and went
to a merge of both, at every landing, where a link would do. A piece
open above and cut at least as low as the last range is promoted on its
keys now, the check a promotion makes anyway, since the fence in a name
says nothing the keys do not.

**A name decided against a store a seal in flight will change.** The
seal that names the first partition asked only whether the store had a
segment. With a table handed to a seal beside one already in flight,
both asked over an empty store and both were named the first partition,
two partitions over the whole range, and the landing's tiling assertion
was what said so. The count of seals in flight is asked first now, and
the state after it, so a seal counted out is one whose landing the
state shows. The rule: a decision about the store's shape is made
against the store plus everything in flight that will change it, and
the order of the two reads is the order the other side wrote them in.

**A precondition left to the callers.** Promotion without a merge is for
a store with no partitions yet, and two of its three callers asked
first. The full flush asked on every round, and it stayed latent inline;
with the merges collected on a thread of their own, a flush met five
pieces a merge in flight had held back, disjoint in key order, and made
them partitions from the bottom over ranges partitions already held.
Nothing raised, and a quarter of the keys read back empty. The function
asks now, and every segment publish asserts that the partitions tile the
key space. The rule: a function correct only in some state checks that
state itself, and an invariant reads route by is asserted where it is
published, not found where it is read.

**A join that blocked the thread everything lands through.** The
segment work's thread took each seal as it was handed over and joined
its thread at once, so for the seal's whole run -- 700 ms at three
hundred thousand keys -- it landed nothing else: a merge that finished
waited, and a piece that could have been promoted was not, while every
read took the merge path. Nothing raised, and the writer never waited,
since it waits only on the flag the landing clears. The thread polls
the seal as it polls the merges now. The rule: a thread that serialises
several publishes blocks on none of them; it waits on the set, and a
join is for a caller that asked for that one thing.

**A count that began only once there was something to count.** Shaping
waited for a read that had counted, and a read counted only when it saw
a piece, so a store read while its first seal was in flight -- no
segment at all -- had nothing counted when the piece landed, and the
shaping waited for the next read and the poll after it; at ten thousand
keys the sweep was over first. Any read the block path cannot serve
counts now, and a promotion, which rewrites nothing, waits for no count
at all. The rule: a signal that gates a decision is raised by the
condition the decision is about -- here, a read the shape made slow --
and not by a proxy for it that happens to be easy to test.

**A sort priced by its compares.** The seal sorted the frozen table's
entries with a comparison sort keyed on the arena's keys, two random
reads a compare and five million compares at three hundred thousand
keys: 147 ms of a 715 ms seal, a fifth, while the scan snapshot over the
same keys sorted them by radix over their sixteen-byte prefixes in 30.
Nothing pointed at it; the seal was off the writer's path and its time
was nobody's until the reads waited on it. The rule: a sort over keys
that live behind a pointer is priced by its misses, not its compares,
and when one structure already sorts the same keys the way that costs
less, the other takes that way.

**A publish that left its derived work to whoever looked first.** A
level-0 piece's ranks -- each key's cut in the partition it is aligned
to -- and its bounds -- where the partition's block boundaries fall in
it -- are functions of two immutable segments, taken once and kept on
the piece under the partition's id. The segment work took them after it
published the piece, so the writer's next scan found them missing at its
rebase and took them itself, on the read path: 0.6 ms at ten thousand
keys and 4.4 at a hundred thousand for a seal's piece, 17 for every
piece over a partition a merge had rewritten, in scan passes of 0.2 and
4.5 ms; a promotion, whose re-opened partition has a fresh id, took none
at all. It hid for as long as a landing came after the fsyncs, which put
it after every pass of the sweep; the two-phase landing put it inside
them, and the flush arm's fully-unmerged lag point read 0.3x of what it
had, at every rung, with the shape arm beside it flat. A host-normalised
bisect against LMDB named the commit, a probe timing each scan of the
pass beside the store's counters named the scan, and timers in the
scan's phases named the call. The ranks and the bounds are taken before
the publish now. The rule: what a publish is to derive is derived before
the publish is visible, since the first reader to find the cache empty
is the one that pays for it; and a cost that timing hides is a cost --
price a landing where the reads are, not where it happens to fall.

**A merge priced for a batch of a hundred.** The scan snapshot's
extension found each new key's place in the run by a binary search over
the rest of the run: right for a batch of a hundred over a hundred
thousand, and fourteen cold lines a key for a thousand over ten
thousand, which is the shape of every lag point's first scan and of the
switch to the seal's snapshot -- two milliseconds of a scan pass of four
at a hundred thousand keys. It sorted the batch by comparing keys through
the arena, two block lookups a compare, a millisecond for five thousand.
The batch sorts by its prefix words now, as the build does, and the
merge walks the run where the batch is dense in it and gallops where it
is sparse. The rule is the one the seal's sort gave, applied to a
search: a search over a run that lives behind a pointer is priced by its
misses, and a merge of two sides within a factor of a hundred of each
other walks the longer side rather than searching it, since a walk
streams and a search does not.

**A read measured fast because the write paid for it.** A first seal
at the floor left a partition behind a load, and the pass after the
load read eleven times faster -- because every commit of the load, with
a partition in place, filed its batch into block forms that the next
seal or merge invalidated: 890 ms of commits at three hundred thousand
keys against a load of 155. The row showed a read win and a load loss
in two cells, and only their sum said the change was a loss. The rule:
price a change that moves work between phases by the phases' sum; a
structure built on a commit in a write-only stretch is paid for by the
writer whether anything reads it or not.

**A page made for the reader on the writer's path.** The segment
writer wrote 2 MB pieces at 2 MB offsets so the page cache would hold a
segment in folios a mapping takes with one entry each, a gain to a scan
this machine never showed. A 2 MB folio takes a whole free 2 MB block,
and this guest's balloon hands free 2 MB blocks back to its host two
seconds after they free, so every such write faulted on the host for
each of its pages: the buffered ordered load ran at a third of LMDB's
from that commit on, 45% of a fresh process's load in the kernel's copy
into the page cache, and nothing the engine counts could see it. The
rule: work one path does for the other side's benefit is priced on the
path that pays it, and a gain never measured is no reason to pay
anything.

**A test that bet a seal would still be running.** Tests that needed a
seal in flight across a commit left it unheld, on the bet that the seal
thread could not finish first, and with the segment work inline a commit
lands a seal that has. When seals got faster the bets lost, a different
test each gate run, and the first of them had been losing one run in
three under load before anything changed. They hold the seal now
(`hold_seal_landing`, and `hold_seal_durable` where only the oldest may
land). The rule: a test that needs a state holds it, and a bet on timing
loses the first time the thing it bets against gets faster.

**A mark taken whole and then read in part.** A commit publishes the
log's length, the entry count and the watermark as one mark, and a
handle under `Latest` or `Snapshot` took the length and the watermark
and left the count: it read the live table's entries to their raw
length, the writer's staged keys among them. A staged key's values sit
past the watermark, so nothing read back wrong by value and every test
that compared values passed; but a scan's snapshot held the key, and
the walk counted it against the limit with nothing to emit -- a scan of
ten beside ten staged keys answered five. The snapshot went into the
state for other handles, and a handle adopted any published snapshot at
or past its own length, so a handle pinned at one commit counted a later
commit's keys the same way. It surfaced as a debug assertion in the
reader-thread test, where the store's first key opens a direct run and
the writer's next, smaller key leaves it: the run's uncommitted tail is
truncated from the ordered table, and a handle whose snapshot named that
entry read past the table's length. A bisect named the commit where the
test began to catch it; the three tests written for the fix fail on the
commit before it too. Every pin now takes all three quantities, a
live-table lookup honours the count, and a handle adopts no published
snapshot past it, as the builder ahead and the keeper already refused
one. The rule: a mark published as one is taken as one, every position
a read names -- in the log, the entries, the arena -- is bounded by the
mark's quantity for it, and a value the watermark hides is still a key
until the count hides it too.

**A carry that took its own publish for the only one.** The writer
carries its tables across a publish it makes itself -- a freeze, the
switch to an ordered table at a direct run's start -- since it knows what
it changed, and leaves any other publish to its next look at the log,
which rebases the tables across it. The freeze carried only over the
state it had settled; the switch carried over whatever state it
replaced. A seal emptied the live table, the segment work landed it
readable, and before the writer looked at its log a key above the
store's greatest opened a direct run: the switch carried the tables
across the landing as well, so they had no bounds for the landing's
piece and the snapshot named the frozen table the landing had retired,
and every scan through the writer walked the last block without a key
of the piece -- all of them, since every key sat above the last
partition's last key -- while a point read and a handle's scan found
them. Nothing raised. The test that caught it passed alone and failed
under load, in about a third of its runs with seals that keep the log
and a twelfth with seals that rotate it, so it surfaced only when the
default changed. The switch carries now only
when it is the one publish since the tables' state, and hands the rest
to the rebase. The rule: a carry across one's own publish holds only
over the state it was prepared against, and the check is that state's
identity or generation, never the one field the carry changes.

**An arm sound only beside the default it was priced with.** An arm
keeps an older shape alive for pricing, and `supdb-nopin` kept the
writer's reads unpinned, which is sound only while the writer is the one
thread that publishes. The segment work then moved to a thread of its
own and publishing in the background became the default, and the arm
kept running beside it: the segment work replaced a state and freed it
while the writer's commit built a block through one of its segments, and
the suite faulted in about a third of the arm's runs. No test ran the
arm at all, so nothing said so until a pairing crashed. A store now
refuses an unpinned writer beside another publisher, and the arm runs
its segment work inline as it did when the pins were priced. The rule:
an option sound only under another's value is checked against it where
the store opens, and an arm that keeps an old shape keeps that shape's
other settings with it.

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
