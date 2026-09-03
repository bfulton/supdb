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

Prediction 1 held: the extraction moved no recorded number. Prediction 3 held
so far: no next-engine result moved.

## What is left, and the one part that costs

The internal suite's thirty-nine `Store` experiments split two ways, and the
split is the last real decision:

- **Retire** (Store's own machinery, not re-pointable, thirty-three of them):
  f2-open, f3-multiproc, f4-durability, f5-latency, f6-threads, f7-index,
  f12-compress, f13-sync, f15-scancache, f16-slack, f17-gather, f19-coldscan,
  f21-writerverify, f22-storescan, f23-madvise, f24-autoreadahead, f25-arena,
  f26-buffer, f27-ckptshape, f29-redolog, f30-insertindex, f31-loadphases,
  f33-indexsize, f34-parallelindex, f35-indexauto, f36-commit,
  f37-consolidate, f38-fanout, f39-walfloor, f40-filter, f41-segroute,
  f46-segwrite, and c1-c3 in the correctness suite.
- **Re-point and re-measure** (the shared format and read path, still live
  code): f1-outofcore, f8-checksums, f11-flatindex, f14-blocktable, f18-fence,
  f20-chunkcrc, f28-count. These are findings about code that ships; they were
  merely measured through the old engine's writer. Each needs a `full` run,
  which is the hours in this plan.

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
