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

Keep it that way. Every gate this repository has broken has broken the same
way: a check that was not running, or one reporting a verdict it had not
earned. CI never built the wasm module at all, so a link break in
`src/wasmapi.rs` survived until a toolchain update happened to surface it
locally. `scripts/fmt.sh` once swallowed "rustfmt could not run" behind
`|| true` and reported green for never having run, which is why it now tells
"formatting differs" apart from "did not run" and fails both. A second
definition of "the checks" is how the next one of those starts.

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
fsync, rename into place, fsync the directory, then reset the WAL; a crash
between any two of those leaves either a WAL that replays the whole memtable
or a complete segment plus a WAL whose sealed prefix is skipped by sequence.
Replay applies the frames between commit frames whole or not at all -- a
partial batch used to replay as whole, and the first test written against
the contract found it. `settle` is what joins an in-flight seal; `sync` does
not, and an experiment that assumed otherwise measured a 286,000-key seal it
had never joined.

## Shapes the bugs come in

The reproducers for the previous engine's defects retired with it. The
shapes are worth keeping, because this engine can take them too.

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

**Arithmetic that underflows fails quietly and expensively.** A size-class
calculation underflowed for every block of 4 KiB or less, which is every
block a store of short postings produces. Debug builds panicked; release
builds wrapped and reserved 7,680 bytes for a tiny placement, so every small
store paid about 1.9x on every section it wrote -- visible as size, never as
a fault.

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
- The durable ordered load trails LMDB and RocksDB, and shuffled arrival
  inverts both. Quote the pair, never one: the `load` and `load-shuffled`
  figures.
- The index layout study found smaller and faster points on the frontier
  that the shipping layout does not occupy (`docs/index-theory.md`).
