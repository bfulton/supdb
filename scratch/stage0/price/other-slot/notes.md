# other-slot (followup 48) -- notes

Base 0c6748bb, no units before this one. Tree clean at start.

## Step
- step 1 done (read). step 2: seam + tests going in (tests/durable.rs, new section "other-slot" at the end).

## Plan
- vault: `open_slot_existing` (read-only, no create, NotFound -> None) and `sync_slot` (fdatasync, traced "sync_slot" with the real path); trace "open_slot" records the real path (so a cipher store's slots filter by the store dir).
- Db::open: both slots synced (taken via the traced call, other where it exists) before open_refusals.
- seam Options::fail_open_other_slot_sync (names the inode).
- step 2 done: before.log. Defect verified on the engine (plain and cipher): the power cut after the
  reopen opens to "refused: the manifest slot manifest names pcs-...sup, which is missing from the
  directory". an_open_makes_no_slot_file_it_does_not_find(+cipher) pass before the fix (they are for the
  open_slot mutant, not for the defect) -- not counted as failing-first evidence.
- step 4 running: neighbours into /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/other-slot/n-<test>.log
- step 4 neighbours green (n-*.log); code restructured (seam error through one Result), durable re-run green. keep/ current. step 7 mutants next: runner /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/other-slot/mut.py, results /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/other-slot/mut-results.txt
- step 7 mutants done: mutants.md (13 run; M3 expected survivor, M12 equivalent, M13 unreachable).
- step 8 docs done (crate docs in db.rs: Db::open's doc, the manifest's doc, the open's comment;
  CLAUDE.md Crash discipline + "A replay that builds..." + corollary in "A slot chosen by its sequence";
  ROADMAP torn-slot item wording). fmt.sh --check green after one rewrap.
- step 4 whole suite running: after.log

## Finished (step 9)
What was wrong: Db::open fdatasynced the manifest slot it takes and left the other to the kernel's
writeback. Verified on the engine (followup 48's trace): a drain's owed slot B, the next publish into
B killed part way (cache: B torn), the reopen takes A, sweeps the owed publish's segment, acknowledges
commits; a power cut with the device's B holding the owed publish whole opens to "refused: the
manifest slot manifest names pcs-...sup, which is missing from the directory" -- plain and cipher
(before.log, run 2).

What changed:
- src/db/vault.rs: open_slot_found (File::open of the real path, no create, NotFound -> None);
  sync_slot (fdatasync, then a "sync_slot" trace record with the real path); the "open_slot" trace now
  records the real path too (so a cipher store's slots filter by its directory).
- src/db.rs Db::open: both slots, the taken one and the other, opened with open_slot_found and
  fdatasynced through sync_slot, in the block before open_refusals; an absent file is passed over;
  either's failure is the open's answer. Options::fail_open_other_slot_sync (seam, names the inode).
  The taken slot is now opened read-only (was vault::open_slot, write+create): same syscalls.
- Docs: Db::open doc, the manifest's doc comment, the open's comment; CLAUDE.md Crash discipline, the
  "A replay that builds" entry, and the corollary in "A slot chosen by its sequence"; ROADMAP wording.

Tests (tests/durable.rs):
- a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen (+_under_a_cipher): the
  state above, the power cut built from the calls (the other slot's device bytes = torn image only
  where the open's sync_slot of it is in the trace), then the seam: a failed second sync is the
  open's answer, names the other slot's inode, directory as found (control: the Ok open removes).
- an_open_makes_no_slot_file_it_does_not_find (+cipher): rename-shape store, no manifest.alt; opens
  that refuse (rename and slots options) leave the directory as found; an Ok rename-shape open makes
  no manifest.alt. Passes before the fix (it holds the open_slot mutant, not the defect).
- an_open_makes_the_slot_it_takes_durable (+cipher), restated (rule 7): both slots on the device by
  page flags (root), control a file outside the store; and both slots' sync_slot in the trace, which
  runs without root (followup 28).
- the three pending-adoption tests gain: both slots' sync_slot before the slot write.

Pricing: none (verdict not-priced). Success-path cost: at Db::open only, one open(2), one fdatasync
of a slot with nothing dirty, one close(2) where the other slot file exists; one failed open(2) where
it does not (a rename-shape store). Nothing on any other path.

Not verified: no timing of the open; no real power cut (the device state is built by hand, as the
slot tests do); M10 is killed only by the root-only page-flag tests; M3/M12/M13 survive (see
mutants.md); an open with no manifest read (both slots unreadable) syncs neither slot -- not in this
unit's brief (followup 53 territory); the rename shape (manifest_slots off) still runs the loop and
syncs manifest.alt if one is left from a slots life.

## Repair round 1 (reviewers' extra mutants)
- step R1-R2 done: r2-durable.log green (189), keep/ current, extra mutants in mutants.md (round 1). step R3: neighbours r1-n-{db,cipher,fuzz,lib}.log, r1-lint.log, r1-fmt.log
  failure is never recorded); torn test gains: no sync_slot record of the failed slot (kills E2/E3), and
  the first open_slot after the reopen's flush comes after both sync_slot (an open_slot record read under
  a cipher, for E4). Tree is in the repair state (not yet copied to keep/). Running durable: r1-durable.log
- (update) R1-R2 done: r2-durable.log green (189), keep/ current (sums in keep/sha256.txt), extra mutants recorded in mutants.md round 1. R3: neighbours r1-n-{db,cipher,fuzz,lib}.log, r1-lint.log, r1-fmt.log.
- R3 done: neighbours green (db 226, cipher 24, fuzz 5, lib 404), lint and fmt green. Round 1 finished; tree = keep/.

## Repair round 2 (reviewers' extra mutants R1-R7)
- R1 (skip the other sync when `skipped`) is NOT equivalent: the open writes the skipped slot only
  after its sweep, so a kill between leaves the device's earlier record (an owed publish naming a
  segment the sweep removed). Test added inside a_torn_publish_over_an_owed_slot (plain only, the slot
  forged with common::crc32c_from): osl_skipped / osl_pending_of in tests/durable.rs, using
  tear_open_slot_write to stop the open after the sweep. Tree: tests/durable.rs edited (not yet in keep/).
- R step: mut3.py running (results mut3-results.txt, logs mut3-R*.log); it restores keep/ by sha after each
- CLAUDE.md: one clause on the skipped slot added (keep/ updated)
- R2 stress: 40 runs of the torn tests 4 at a time beside the whole durable binary: all pass (r2-stress-load.log = durable 189 green on this tree). Neighbours next: r2-n-*.log
- Round 2 finished: mutants R1 (killed by the new osl_skipped section), R3/R6/R7 killed, R4 equivalent
  (as M3), R5 invisible cost mutant (held by reading), R2 not buildable (no Option holds landings at
  open; 40 stressed runs, premises never fired). Neighbours green (r2-n-{db 226,cipher 24,fuzz 5,lib 404}),
  durable 189 (r2-stress-load.log), lint and fmt green (fmt rewrapped one closure in the new test).
  keep/ current, final.diff updated, TMPDIR removed. src/ unchanged this round.
step: commit gate running, log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/price/other-slot/gate.log
step: committed 4a7efafc43668d15d38f4b8dd85602e3f8fb9ca2
