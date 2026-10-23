# Patches drafted and not applied, for the next lever

Each is a Python script of exact-once replacements against the tree as it
stands after the commit that added it; run it from anywhere, then build,
test and pair as `HANDOFF.md` says. A script that fails its assertion names
the anchor that moved.

- `memo-size-engine.py`, `memo-size-suite.py`: the memo's cap as an option
  (`Options::zpages_memo_bytes`, 8 MiB as shipped) with arms at 16 and 32
  MiB (`supdb-cache64tofitautozpagesmemo16`, `...memo32`), for counting the
  decodes E and the point-read mixes make under each before any rate is
  read; the counts are deterministic, so one `bench ab --reps 1` of the two
  arms gives both. At the commit that read a block through one cursor
  (`Options::zpages_block_cursor`) the engine script's anchors still match
  and the suite script's first anchor does not (the arm fields moved under
  the two arms added since); re-anchor it on `zperrun`, the last arm's
  field, before running it. The engine script's `db.rs` half writes the
  file directly, not through its `edit` function, so a dry run made by
  disarming `edit` applies that half: run it only to apply it, on a tree
  whose `db.rs` is committed, and check `git diff` after.

The lever C drafts that were here -- a block's snapshot keys cut by one
cursor moved forward key by key, `Options::zpages_cut_cursor` -- were
superseded by the change that landed as `Options::zpages_block_cursor`: a
block read through one cursor for its runs, its cuts and the records its
overlay keys replace (`stage4.md`, "A block read through one cursor"),
which is what the counts asked for once the walk's opens were counted
beside the cuts'.
