# open-cost (followups 27, 38, 39, 57)

Base: HEAD c07c95a1 (run base 0c6748bb + other-slot, rename-arm, due-log, helper-race, drain-order).
Tree was clean at start.

## Plan
- Fold 27/48: the slot handles manifest_read_all opened are the ones fdatasynced (no second open/close).
- Fold 38: a plain store answers "is the named segment there" from the open's listing (sorted, binary search);
  under a cipher vault::on_device's stat stays.
- Fold 57: unnamed_refusals maps only the live segments (and runs' segments) whose name's fence holds the
  unnamed segment's last key, cached by name across unnamed segments.
- Fold 39: unnamed_refusals moved ahead of the log's replay (no memtable built for an open that refuses there).
- Also: the open's three directory listings (segments, logs/spares, runs' temp names) folded into one.
- Tests: trace notes (vault io trace, open-only paths): "read_slot" each slot file opened for reading,
  "on_device" each stat of a named segment, "open_holder" each holder mapped by the unnamed check.

## Step log
- step 1 (reading): done
step 2: before run; src/db.rs is the seam-only copy keep/seam-src_db.rs, the fix in keep/work-src_db.rs
step 4: neighbours running (neigh.sh -> n-*.log); measuring copies in keep/measuring
step 4 done: after.log whole suite green (exit 0). step 5: bench build
step 5: probe sitting 1 running (probe.sh 1 -> probe/s1-*)
probe s1 done (unnamed in s1 measured dirty copies: discarded); probe changed to sync its copies; s2 full and s3 unnamed running
- measuring.patch saved (the tree with price_open_cost_old/bare, the four arms, the probe bench/examples/open_cost.rs).
  The bench binaries (target/release/bench, examples/open_cost) are built from it; src/db.rs and vault.rs now
  edited to the landing shape (fields gone, open_slot_found gone) while the probe runs on the built binary.
- Probe: open_cost <arm> <size> <opens> <clean|log|unnamed>, variants folded/landed/bare alternating on the same files.
  s1 unnamed measured unsynced copies (the slot fdatasync paid the journal's writeback of 400 MB): discarded.
- Mutant runner: mut.py (whole durable binary per mutant, verdicts appended), list to write once keep/*.fixed exists.
step 5: pair.sh runs (pairs/), --passes load
step 6: landing tree in keep/*.fixed (sha256.txt); measuring.patch saved; arms and probe out
step 7: mutants running (mut.py mutants.json -> mut-results.txt, logs mut-*.log); the fixed copies keep/*.fixed are put back after each

## What was wrong (cost, not correctness)
Stage 0's open (slot-choice, open-order, other-slot) put on every open: each manifest slot file opened a second
time for its fdatasync (open_slot_found), a stat per named segment (vault::on_device -> exists) beside the
directory listing it had already made, two more listings of the directory (wal/spare loop, runs' temp names in
unnamed_refusals), the log replayed before unnamed_refusals (a refusal there replayed for nothing), and after a
crash every named segment (and runs' segments) mapped whole to look one key up.

## What changed (src/db.rs, src/db/vault.rs)
- manifest_read_all returns the slot files it opened beside the manifests; the open fdatasyncs those
  descriptors (vault::open_slot_found removed). Seams and the inode-naming failure unchanged.
- One listing (vault::names), sorted; on_disk filtered from it; the wal/spare loop and unnamed_refusals' run
  temp names read it. open_refusals: in the clear binary_search the listing; under a cipher vault::on_device
  as before (the listing there is the vault's names, from the slot).
- unnamed_refusals runs right after open_refusals, before the replay (followup 39).
- unnamed_refusals: candidates (named + runs' segments beside a temp name) filtered by Db::name_takes(name, key)
  (the par-/pcs- name's fences [lo,hi); a seg- name or one that does not parse takes every key); each candidate
  mapped once (HashMap cache) and the walk stops at the first holder.
- vault::note(what, path): a trace record for the open's own reads (read_slot, on_device, open_holder, replay),
  one relaxed load when tracing is off; on open-only paths.

## Tests (tests/durable.rs, section open-cost)
- an_open_opens_each_slot_file_once (+_under_a_cipher): read_slot records, one per slot file; both sync_slot.
  Before: ["manifest","manifest","manifest.alt","manifest.alt"].
- a_plain_open_finds_its_named_segments_in_its_listing: no on_device record for a plain open. Before: the three
  partitions' names.
- an_open_under_a_cipher_looks_for_each_named_envelope: held, not fixed (passes before and after): one
  on_device per named segment, each once. Not counted as failing-first evidence.
- an_open_after_a_crash_reads_only_the_segments_over_an_unnamed_segments_keys: three partitions a-/m-/z-,
  a merge over m- held at its landing and copied; every open_holder record names a named segment whose
  fences take in the m- range. Before: the a- partition mapped.
- an_open_that_refuses_an_unnamed_segment_replays_no_log_first: flushed run, damaged newest slot, a write in
  the log; refused with OO_KEYS, directory as found, and no replay record. Before: replayed wal-00000000.

## Pricing (rule 13)
Probe (measuring tree only, in measuring.patch): bench/examples/open_cost.rs <arm> <size> <opens> <mode>; the
store loaded through the suite's adapter (Supdb::create / create_ingest, suite keys and 100-byte values in
batches of 1000), sync + settle; then opened N times with three Options alternating over the same files:
folded (this unit), landed (price_open_cost_old: stage 0 as landed), bare (price_open_cost_bare: no slot
sync, no named-segment check, no unnamed check). Modes: clean (closed), log (1% rewritten in commits no flush
covered, copied under hold_seal_landing+hold_merge_landing), unnamed (1% rewritten and flushed with merges held
at their landing, copied there and each copy synced; re-copied before every open, since an open sweeps).
Results probe/s1-*, s2-*, s3-* (s1 unnamed discarded: unsynced copies), summary probe/summary.txt.

Median us, folded / landed / bare, sitting 1 and 2 (unnamed: 2 and 3):
- supdb 10000 clean: 495.9/503.8/377.1 ; 493.8/501.3/371.5   (folded/landed 0.984, 0.985)
- supdb 10000 log:   609.4/620.5/450.3 ; 511.9/522.2/387.4   (0.982, 0.980)
- supdb 10000 unnamed: 2047/2079/1331 ; 2159/2204/1396        (0.985, 0.979)
- supdb 300000 clean: 9202/9155/9012 ; 6436/6416/6177        (1.005, 1.003)
- supdb 300000 log: 10290/10327/10044 ; 6929/6902/6704       (0.996, 1.004)
- supdb 300000 unnamed: 49050/48640/30929 ; 48442/49048/31161 (1.008, 0.988)
- supdb 3M clean: 93242/92525/92336 ; 90260/90237/90415      (1.008, 1.000)
- supdb 3M log: 100135/99304/100292 ; 97761/98145/97871      (1.008, 0.996)
- supdb 3M unnamed: 475385/470628/292423 ; 458153/473208/303131 (1.010, 0.968)
- supdb-ingest 10000 clean: 546.3/549.6/392.8 ; 472.1/481.5/348.0 (0.994, 0.980)
- supdb-ingest 10000 log: 577.8/580.8/418.9 ; 596.7/604.9/448.8 (0.995, 0.986)
- supdb-ingest 10000 unnamed: 2051/2029/1328 ; 2237/2212/1367 (1.011, 1.011)
- supdb-ingest 300000 clean: 8131/8103/7854 ; 5438/5410/5262 (1.003, 1.005)
- supdb-ingest 300000 log: 10196/10174/9997 ; 7224/7236/7039 (1.002, 0.998)
- supdb-ingest 300000 unnamed: 49547/48443/30698 ; 49494/49526/31900 (1.023, 0.999)
- supdb-ingest 3M clean: 78796/78733/78243 ; 77852/78382/77720 (1.001, 0.993)
- supdb-ingest 3M log: 98766/98739/98119 ; 96292/95948/95844 (1.000, 1.004)
- supdb-ingest 3M unnamed: 462478/455360/294665 ; 465806/461624/290599 (1.016, 1.009)
Reading: folded against landed is a step under 2% better at 10000 keys clean and log in both sittings, both
arms (0.980-0.995), and level elsewhere (0.968-1.023, signs disagreeing between sittings). Not a cost.
Stage 0's price (bare against landed): 10000 keys a quarter of the open (about 120-150 us, the two slot
fdatasyncs: bare and folded differ in exactly those two syscalls); 300000 keys 2-4%; 3M level; after a crash
with a merge's outputs unnamed, the open takes 1.5-1.6x (+18 ms at 300000, +160-180 ms at 3M), not attributed
by a stamp (ROADMAP item added).

Syscalls in one open's window (strace -f, cut at the probe's OPENCOST-MARK/END writes; strace/summary.txt):
- supdb 10000 clean: folded 151, landed 166, bare 149 (folded: openat 20 vs 24, close 19 vs 23, getdents64
  10 vs 14, statx 19 vs 20, fstat 5 vs 7; fdatasync 2, bare 0).
- supdb 10000 log: 155 / 170 / 153. supdb-ingest 10000 clean: 150 / 166 / 149.
- supdb 300000 unnamed: 206 / 234 / 183 (openat 30 vs 36, statx 30 vs 36, mmap 12 vs 14, munmap 4 vs 6).

pair.sh (13c), the four arms twins of supdb / supdb-ingest differing by `price` alone (read in the patch):
`OUT=.../pairs sh bench/scripts/pair.sh supdb supdb-opencostold SIZE --passes load` and
`supdb-ingest supdb-ingestopencostold`, SIZE 10000 and 300000, two sittings each. No cell of the suite opens a
store (bench/src/engines.rs makes stores with Db::create alone), so these are a control: load ops/s
0.921-1.042x with signs disagreeing between sittings, every sign p >= 0.146, 0-2 marked * of 254 where a coin
marks 12.7: level. The head.txt files record the tree as it stood when the pairs ran (src already edited to
the landing shape); the binary was the measuring tree's, built before.
13d (stamped pairing, strace over load_only): not run; the change is in Db::open, which no load, drain or
other suite phase runs, so there is no phase that holds it; the probe and its strace are the mechanism's
measurement instead.
Verdict: level (folded vs landed); lands.

## Not verified
- (round 1: N10 is now held by the descriptor test; see below.)
- The unnamed-state residual (+160-180 ms at 3M) is not attributed by a stamp; the guess that it is the
  whole opens of the unnamed segments and holders (key-section checksum rows) is unverified.
- The routing assumes a segment holds only keys its name's fences take in; it is the invariant reads route by
  (Seg::may_hold), argued, not tested beyond the suite's states.
- Under a cipher the unnamed check does not run (unchanged); the cipher arm of the slot fold is tested
  (an_open_opens_each_slot_file_once_under_a_cipher), the listing fold has no cipher arm by design (held).
- The full suite (after.log) ran on the measuring tree; the landing tree ran durable, db, cipher, fuzz, lib
  (l-*.log) and the 26 mutant runs of durable; clippy -D warnings and fmt --check on the landing tree.

## Repair round 1 (started)
Blocking: N10 survives; an_open_opens_each_slot_file_once holds nothing in the landing tree. Plan (a): sync_slot
records the descriptor it synced, manifest_read_all's note records the descriptor it opened; the test matches.
Plus ten extra mutants. Tree at round start == keep/*.fixed (sums checked).
- r1: note_file/IO_DESC added (vault.rs), read_slot noted after the read with its descriptor, sync_slot through note_file; test asserts (fd, offset) of read == sync. keep/*.fixed refreshed (round 1 state).
- r1 step: mutants running: mut.py r1-mutants.json -> r1-mut-results.txt (logs mut-R1-*.log); fixed copies put back after each
- r1: runner stopped after 4 mutants (E1 survived) to add a seg- holder test; src/db.rs restored from keep, sums OK. Remaining mutants (E2..E10) to rerun from r1-mutants-rest.json
- r1: test an_open_after_a_crash_holds_an_unnamed_segments_keys_to_pieces_without_fences added (seg- holders); E1 against it red by hand (r1-seg-e1.log). keep refreshed. runner 2: r1-mutants-rest.json (E1 + E2..E10) -> r1-mut-results.txt
- r1: test an_open_after_a_crash_reads_no_piece_whose_fences_leave_an_unnamed_segments_key_out added; E8 red by hand (r1-pcs-e8.log). keep refreshed. runner 3: E8+E7 again (r1-mutants-e8.json), then neighbours (neigh.sh -> l-*.log)

## Repair round 1 (what changed)
- src/db/vault.rs: vault::note_file(what, path, &File): under the trace, the call recorded as note does and,
  in a record of its own (IO_DESC, read by vault::traced_descriptors / Db::trace_slot_descriptors), the
  descriptor's number and its open file description's offset. sync_slot records through it; one relaxed
  load when tracing is off, as the inline check it replaces.
- src/db.rs: manifest_read_all notes read_slot after read_to_end through note_file (was note before the read:
  same one load, no syscall). Db::trace_slot_descriptors (#[doc(hidden)] pub, a test's).
- tests/durable.rs: an_open_reads_each_slot_once holds read (fd, offset == len) == sync (fd, offset) per slot
  file. New: an_open_after_a_crash_holds_an_unnamed_segments_keys_to_pieces_without_fences (seg- holders,
  kills E1); an_open_after_a_crash_reads_no_piece_whose_fences_leave_an_unnamed_segments_key_out (pcs-
  holders, kills E8). Neither is failing-first evidence of a fix: they hold the routing against mutants.
  The descriptor test fails against N10 (before.log, round-1 section).
- CLAUDE.md: the open-cost entry amended (the first test held the fold only against the shape it replaced;
  rule: a fold that removes a call is held by the identity of what the remaining call used).
- Pricing not re-run: the round adds no syscall, wait or allocation to an open with tracing off (a trace
  check moved after the read and the sync's inline trace check made a call to the same check), and no
  suite pairing runs Db::open (the load pairings were a control). The probe's numbers stand for the code
  as priced; this round's code was not probed.
- r1 done: neighbours green (l-*.log), clippy -D warnings and fmt --check exit 0 (r1-clippy.log, r1-fmt.log); TMPDIR removed.
## Repair round 2
- r2 start: tree == keep/*.fixed (sums OK). Extra mutants r2-mutants.json (X1..X8) -> r2-mut-results.txt (logs mut-R2-*.log), runner mut.py, fixed copies put back after each
- r2: X1 survived; test an_open_after_a_crash_holds_an_unnamed_segments_key_at_its_ranges_lower_fence added (passes on fixed, r2-lo.log); keep refreshed; X1 rerun as R2-X1b (r2-mutants-x1.json -> r2-mut-results.txt)
- r2: fmt.sh exit 0 (r2-fmt.log); neighbours running (neigh.sh -> l-*.log), then clippy (r2-clippy.log)
- r2 done: extra mutants X2-X8 killed; X1 (`key <= lo`) survived, test
  an_open_after_a_crash_holds_an_unnamed_segments_key_at_its_ranges_lower_fence added, X1 then killed. No src
  change this round, so no pricing re-run (the probe's numbers stand; no pairing was shown to miss the mechanism).
  Neighbours green (l-*.log: durable 272, db 226, cipher 24, fuzz 5, lib 406), fmt.sh 0, clippy -D warnings 0
  (r2-clippy.log). TMPDIR removed. Tree == keep/*.fixed.
## Repair round 10
- r10 start: tree == keep/*.fixed (sums OK). Blocking: cipher test the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit
  failed once in the gate at its first part (before any reopen). Step: reproduce the flake on the tree and on HEAD's src
  (the unit's base, written in mutant-style and put back from keep by sum); runner r10-flake.sh -> r10-flake-*.log
- r10: flake runner on the tree (tag tree, 10x3) running
- r10: tree 10x3 (three processes at once): target 0 of 30; the_builders_rest_at_every_block_of_a_store_of_wide_values
  failed 6 of 30 at its cache peak (6.5-8.4 MB of a 2 MB budget): three processes at once are each other's stalls
  (CLAUDE.md: the cache-bound tests run one at a time); not read as anything. Next: tree 40x1 (tag tree1), then base 40x1.
- r10: tree1 40x1: target 0 of 40; the_builders_rest_at_every_block_of_a_store_of_wide_values failed 5 of 40 (cipher.rs:845,
  peak 6.8-8.6 MB of 2 MB + 8 MB slack... i.e. over budget+slack? see below). That test reopens. Step now: BASE run --
  src/db.rs and src/db/vault.rs are HEAD's (the unit's base) in the tree while r10-flake.sh base 40 1 runs; afterwards
  put back from keep/*.fixed and check sums. IF THE CONTAINER RESTARTED HERE: put src_db.rs.fixed / src_db_vault.rs.fixed back.
- r10: BASE (HEAD's src/db.rs, vault.rs) 40x1: the target failed 1 of 40 with the same assertion (cipher.rs:1666, "the builds
  gave nothing back", evicted_units 704) and the wide-values test 4 of 40 (cipher.rs:845) -- both predate the unit. Tree put back
  from keep (sums OK). Logs r10-flake-{tree,tree1,base}.log and their -*.out.
- r10: gate (sh scripts/check.sh build test lint) running -> r10-gate.log
- r10 done: gate sh scripts/check.sh build test lint EXIT 0 on the tree (r10-gate.log; cipher 24 passed, durable 272, db 226,
  lib 406, lint incl. clippy -D warnings and the format check). No src change this round, so no pricing re-run and no new
  mutants (the round's extra list was empty). The cipher flakes are the base's: the_compact_form_is_built_and_read_within_a_cache_
  the_store_does_not_fit 1 of 40 runs on HEAD's src (0 of 70 on the tree), the_builders_rest_at_every_block_of_a_store_of_wide_values
  4 of 40 on HEAD's src and 5 of 40 on the tree; neither failing assertion is reached by code the unit changed (the first fails in
  the part before any Db::open; every function the diff changes is called only from Db::open). TMPDIR removed; tree == keep/*.fixed.
