# Retiring the original engine

Two engines have been in the tree since `src/next.rs` shipped. `Store` is the
vendored one from the design artifact; `Db` is the one every current comparison
and every browser path is measured on. Keeping both costs more than the second
engine is worth: it doubles the surface a reader has to hold, and it lets
`claims.json` gate code nobody runs.

The governing rule for the claim disposition, and it is the whole reason this is
a considered change rather than a delete: **a claim has to be about current
code.** A finding that cannot be re-measured is not a limitation on the books,
it is a fossil. So Store's claims come out of `claims.json` rather than being
parked in it with a marker, and their results come out of `results/`. Git holds
the record; the gate holds live code.

## What `Store` actually owns that `Db` needs

Almost nothing, which is the finding that made this tractable. The grep looks
alarming -- `store::` appears throughout `next.rs` and `blob.rs` -- but all but
fifteen of those are doc comments pointing at the file format's original
description. The real coupling is:

- `MAGIC`, `SUPER`, `SLOT`, `SUPER_BYTES` -- the file format, not the engine.
  `next.rs` writes superblocks with them; `blob.rs` keeps its own copies and
  *asserts* they match, which is why a drift has never shipped.
- `enc_phase` -- two calls in `flatindex.rs`, a timing print behind an env var.

So the first step is an extraction, not a deletion: those move to `src/format.rs`
and stop belonging to an engine. `write_section_raw` is Store's own and goes
with it.

## What retires with it

`src/store.rs`, `src/readers.rs`, and -- because Store, the internal suite and
the Store reproducers are their only callers -- `src/freelist.rs` and
`src/keytable.rs`. Then `tests/known_bugs.rs`, `tests/valuelog.rs`,
`tests/consolidate.rs`; `soak`, `supbench`, `recover`; `correctness`'s c1-c3;
the external suite's `supdb`, `supdb-durable`, `supdb-buffered` arms; and the
forty experiments in `internal.rs` that drive `Store::open`.

Two capabilities go with it and have no equivalent in `Db`: `open_as_of` /
`open_as_of_time` (read a store as of an older generation or wall-clock time)
and `Reclaim` (a retention policy over superseded extents). **No claim asserts
either.** They are unexercised surface, not proven features, and that is the
argument for letting them go rather than porting them: nothing here can say
whether they work.

## The claim triage

Of the 264 findings, forty experiments carrying about a hundred claims drive
`Store`. They are not one kind of thing, and the split decides the work:

- **Store's own machinery** -- checkpoint shape, the redo log, the arena, the
  mmap writeback ledger, reader open, sync policy, consolidation, thread
  scaling. `Db` does not have these mechanisms, so the findings are not
  re-pointable and not true of anything shipping. These retire, claims and
  results together.
- **The shared format and read path** -- checksums, the flat index, the block
  table, fences, chunk CRCs, counts, the index-layout pair, the analytics
  kernels. These are findings about code that is still live; they merely
  happen to have been measured through `Store`'s writer. Deleting them would
  drop coverage of current code, so they get re-pointed at `SegmentWriter` and
  `Blob` and re-measured. Numbers may move, and where one does the claim
  records the new value with the reason.

The second bucket is why this is staged rather than one commit: a re-pointed
experiment is a new measurement, and a re-measured claim needs `full`.

## Prediction, registered before the work

1. **The extraction is inert.** Moving four constants and `enc_phase` out of
   `store.rs` changes no bytes in any file and no measurement. If any recorded
   result moves, the extraction was not inert and something was wrong about
   what those constants meant.
2. **The re-pointed format experiments hold, and two do not.** Checksums,
   fences, chunk CRCs and the block table should read the same through a
   segment as through a store -- same decoder, same sections. The two I expect
   to move are `f28-count`, because a segment inlines runs under 256 bytes and
   a store never does, so the count arm reads a record where it used to read a
   block; and `f11-flatindex`/`f33`-style index-size figures, because a
   segment's key section is laid out records-first and carries a checksum row.
3. **Nothing about `Db` moves.** No result file under a next-engine or external
   next arm should change. This is the one that would indicate a mistake: if
   retiring the old engine moves the new engine's numbers, the two were sharing
   something this plan says they do not.

## Stages, each green and committed

1. `src/format.rs`; `next.rs`, `blob.rs`, `flatindex.rs` point at it.
2. Move the remaining live consumers off `Store`: logshed's day roll writes
   through `SegmentWriter`, and the fixtures in `tests/blob.rs`, `dict.rs`,
   `ranges.rs`, `segwriter.rs` are written by one too. `tests/blob.rs` is the
   one that matters -- it holds the two read paths to the same answers, and it
   has to keep doing that over a file the shipping writer produced.
3. Re-point the shared-format experiments; re-measure at `full`.
4. Delete the Store experiments, tests, bins and external arms.
5. Delete `store.rs`, `readers.rs`, `freelist.rs`, `keytable.rs`.
6. `claims.json` and `results/`: the retired claims out, the re-measured ones
   updated. `results/baseline/` goes -- it is the pre-fix baseline of a file
   that no longer exists.
7. `CLAUDE.md`, `README.md`, `src/lib.rs`, `docs/`: one engine.

## Where it stands

Done, each green and pushed:

1. `src/format.rs`, and the read paths pointed at it.
2. The read-path tests write segments. Three shapes changed and each is now
   pinned rather than assumed: an inline run plans no fetch, one `end()` emits
   one extent, and the store-versus-segment comparisons became the writer's
   own two layouts.
3. logshed's roll writes segments. W1.3 retired -- the writer takes keys in
   byte order, so there is no line-order arm to compare against -- and W1.1
   and W1.2 were re-taken at `full`: 28.03 B/line over 478,045 fixed against
   36.13 over 632,616, so the 32 MB budget holds 1,179,916 lines rather than
   911,192.
4. The external suite fields one engine. EXT.1-EXT.14 retired with the three
   `supdb` arms. ext-analytics kept its claims and changed its fixture, since
   it was always `Blob` against LMDB's DUPFIXED with only the file under it
   written by the old engine.

5. The thirty-two `Store`-machinery experiments are gone from the internal
   suite, with their claims, metrics, recorded results and figures. What is
   left in `internal.rs` is the seven shared-format experiments and the next
   engine's own.

Prediction 1 held: the extraction moved no recorded number. Prediction 3 held
so far: no next-engine result moved.

## What is left, and the one part that costs

The internal suite's thirty-nine `Store` experiments split two ways, and the
split is the last real decision:

- **Retire** -- done for the thirty-two in `internal.rs`. Still to go:
  `c2-oracle` and `c3-crash` in the correctness suite, which are the model
  oracle and crash injection for the old engine (`c4-crash` is the next
  engine's). Retiring `c2-oracle` leaves the next engine without a
  differential model oracle, which is a real gap and should be written down
  rather than discovered later.
- **Re-point and re-measure** (the shared format and read path, still live
  code): f1-outofcore, f8-checksums, f11-flatindex, f14-blocktable, f18-fence,
  f20-chunkcrc, f28-count, and `c1-decoders` in the correctness suite. These
  are findings about code that ships; they were merely measured through the
  old engine's writer. Each needs a `full` run, which is the hours in this
  plan. `c1-decoders` also needs `Blob::block_extents` -- it aims damage at
  bytes that actually carry payload, and only the old reader can currently
  say where those are.

Then: delete `store.rs`, `readers.rs`, `freelist.rs`, `keytable.rs`,
`tests/known_bugs.rs`, `tests/valuelog.rs`, `tests/consolidate.rs`, the `soak`,
`supbench` and `recover` binaries; drop the retired claims; and rewrite
`CLAUDE.md`, `README.md`, `src/lib.rs` and `docs/` for one engine.

One thing to decide when the canonical numbers are next taken rather than now:
the committed `results/ext-*.full.json` still record runs that included the
retired arms. They are accurate records of runs that happened, and no claim
cites them any more, so they stay until the next canonical `full` run replaces
them. Taking that run is the right last step of the retirement, not a step in
the middle of it.

## The canonical run, taken and rejected

`ext-kv --profile full`, ten engines at 1M keys, seven repetitions, on the
current head with the RocksDB arms built (`results/ext-kv.full.run5-postretire.json`).
It is recorded and it does not replace the canonical file, because the
comparators moved more than the engine did.

Run over run, the load rate of the arms this repository does not touch:

| arm | ratio |
|---|---|
| lmdb-nosync | 0.516 |
| rocksdb-tuned-drain | 0.639 |
| redb | 0.734 |
| rocksdb-nosync | 0.777 |
| rocksdb-tuned | 0.861 |
| rocksdb | 0.903 |
| lmdb | 0.941 |

A 45-point spread on code nothing here changed. The engine's own arms --
0.764, 0.779, 0.920 -- sit inside it, so the run says nothing about the load
axis either way.

`verify` did flag one flip: `EXT.24` recorded `holds` against an expected
`fails`, the ordered scan reading 1.269x of LMDB where the canonical run has
0.899x. That is the comparator and not the engine: LMDB's scan fell from
23.7M to 17.6M entries/s (0.743x) while the engine's fell from 23.6M to 22.4M
(0.950x). Flipping a claim on it would have recorded a fact about the host as
a fact about the engine, which is the failure this project exists to avoid.
The claim stays `fails` until a quiet host says otherwise.

**Prediction 3 held on the axis that could have refuted it.** The retirement
touched `PieceWriter` and `Options`, both on the read path, and the point read
came back at **0.994x** of the previous canonical run -- 1,913,379 against
1,902,515 ops/s -- with the ordered scan at 0.950x. If collapsing the writer
enum or slimming the options struct had cost anything, the read is where it
would show, and it did not.

What remains is one canonical `full` campaign -- ext-kv, ext-ycsb,
ext-loadshape, ext-analytics -- taken on a host that is not also building.
Two runs is the minimum, as ever.

Building the RocksDB arms needs `LIBCLANG_PATH` pointing at a directory
holding a file named exactly `libclang.so`; `clang-sys` does not match the
versioned `libclang-18.so.1` this image ships, and without the arms the run
silently omits `EXT.28`-`EXT.41`.
