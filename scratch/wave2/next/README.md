# Patches drafted and not applied, for the next lever

Each is a Python script of exact-once replacements against the tree as it
stands after the commit that added this directory; run it from anywhere,
then build, test and pair as `HANDOFF.md` says. A script that fails its
assertion names the anchor that moved.

- `leverc-engine.py`, `leverc-suite.py`, `leverc-test.py`: a block's
  snapshot keys cut by one cursor moved forward key by key
  (`ZCursor::skip_until`, `Options::zpages_cut_cursor` on, the arm
  `supdb-cache64tofitautozpagescutseek`), the test `the_cut_cursor_agrees_with_the_seek_a_key`,
  and the suite's arm. `cuts_of` was 5.9% of E's main thread, a seek from
  the fence's top per snapshot key over a block. Apply the engine script,
  then the suite's, then the test's; the test's windows hook takes a seventh
  flag, so `windows_over_with`'s six-flag callers change with it.
- `memo-size-engine.py`, `memo-size-suite.py`: the memo's cap as an option
  (`Options::zpages_memo_bytes`, 8 MiB as shipped) with arms at 16 and 32
  MiB (`supdb-cache64tofitautozpagesmemo16`, `...memo32`), for counting the
  decodes E and the point-read mixes make under each before any rate is
  read; the counts are deterministic, so one `bench ab --reps 1` of the two
  arms gives both. The engine script's anchors expect lever C applied; it
  falls back to the trusted-views field otherwise.
