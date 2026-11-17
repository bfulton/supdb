# slot-choice (defect owed-slot): notes

STATUS: repair round 2 done, uncommitted in the working tree. THE FIXED COPIES
ARE r2/keep/{db.rs,durable.rs,db_test.rs,common_mod.rs}.fixed (sha256 in
r2/keep/sha256.txt, matching the tree; common_mod.rs = tests/common/mod.rs).
Round 1's were r1/keep/db.rs.fixed, r1/keep/durable.rs.fixed, r1/keep/db_test.rs.fixed
(sha256 in r1/keep/sha256.txt, matching the tree). keep/*.fixed are round 0's
and are superseded. Round 0's text below is kept as it was written, except
where round 1 corrects it (see "REPAIR ROUND 1"). keep/db.rs.seamonly is the
pre-fix engine with the seam only (built by seam_only.py from HEAD), used for
before.log; it is NOT the fixed one.

## What was wrong
The manifest's two slots were chosen by the publish sequence's parity. A
drain's slot N is written with its fdatasync owed (publish_unsynced); a merge's,
promotion's or second seal's publish N+1 in the same drain went into the other
slot -- the one holding N-1, the newest durable manifest -- and dropped the owed
sync. A power cut tearing N+1 left the device N-1 (whose retirements had run) or
both slots torn: the open refused with acknowledged commits unreachable. Same at
open: a slot read from the page cache after a kill was taken as the store and
synced by nothing, and the first publish overwrote the durable one.

## What changed (src/db.rs)
- `SlotChoice { durable, synced }`: a publish writes `next()` = the slot that
  does not hold the newest manifest known durable; `landed(slot, seq)` only
  where an fdatasync covering that write returned: Maint::publish after its
  write+sync, sync_owed_slot after sync_data, the pending arm after the
  helper's wait(), the open's own writes. Two publishes with no sync between
  write the same slot twice. `manifest_slot(seq)` (parity) removed; the slot
  is a parameter of manifest_write_slot(_unsynced)/manifest_write_shape.
- slot_sync_due carries (File, slot, seq).
- Manifest carries the slot it was read from; Db::open fdatasyncs the slot it
  takes (vault::open_slot + sync_data) right after choosing it, before the
  orphan sweep and before its own writes; the upgrade's second write goes into
  the old slot after the first is durable.
- pending_whole walked a pending segment with Blob::scan, which a piecewise
  (cipher) source refuses, so under a cipher every pending slot read as torn and
  the open fell back to a slot whose log was retired ("wal sequence gap"). Now
  scan_either. Found by the arm test (arm-dbg.log); a separate, cipher-only
  defect of the manifest_pending arm, fixed because it is one line.
- Seam (#[doc(hidden)] pub, no cfg): SlotWrite, SlotWatch, Db::watch_slot_writes;
  Shared::{slot_watch, slot_watching, slot_synced}; Maint::{watch_slot,
  slot_synced} gated on one relaxed load of slot_watching each. The open seeds
  slot_synced with what it synced, at Shared's construction.
- Docs: Options::manifest_slots, Db::open; CLAUDE.md crash-discipline
  paragraph (two places), a new shape entry ("A slot chosen by its sequence,
  beside a sync left owed"), amendments to the replay entry (the open's slot)
  and to "A path only one arm exercises" (pending under a cipher);
  docs/engine.md "The manifest in two slots"; ROADMAP item for a real torn
  slot write on dm-flakey.

Success-path cost: no syscall, wait, lock or allocation added to a commit, sync,
flush, drain, seal or publish. The parity compute is replaced by a field load
and `1 - durable`; a publish whose sync returned stores two words into Maint's
own SlotChoice; the seam adds a relaxed load before the write and one after the
sync. At open: one fdatasync of the slot read (cheap when clean; not timed).

## Tests (tests/durable.rs, section "slot-choice")
- a_publish_after_a_drains_owed_slot_never_overwrites_the_last_durable_slot
  (+_under_a_cipher): 4 drains, each seal slot deferred and a merge/promotion
  publish after it; crash states at every armed slot write, checked against the
  model at that flush (per-round check, os_check_built).
- a_second_seal_landed_by_a_drain_...: two seals landed by one drain.
- an_open_that_takes_an_unsynced_slot_... (+cipher): kill state with the newest
  slot owed; the reopened store's first publish's crash states.
- an_open_makes_the_slot_it_takes_durable (+cipher): independent instrument --
  /proc/self/pagemap + /proc/kpageflags say the slot the open took is clean and
  the other (control) still dirty. Needs root; prints "not run" otherwise.
- every_arm_that_writes_a_slot_keeps_the_last_durable_one: flush_waits_manifest,
  manifest_pending (plain and cipher, full rounds so the log retires), drain_syncs_wal.
- a_store_moved_between_the_manifest_shapes_keeps_its_writes: manifest_slots off
  with and without manifest_spare, alternating with slots across reopens.
- a_drain_after_an_owed_slot_was_synced_keeps_that_slot (+cipher): drains that
  publish nothing after their slot, seal_rotates_wal so the owed sync retires a
  log file; kills S5.
Guards are fix-independent (window = a deferred write followed by the next
before its sync, read from the engine's sync record), never the defect's slot.
Restated: tests/db.rs a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid.

## Logs
before.log (seam-only engine: 7 tests fail at their contract assertions),
arm-dbg.log (pending under cipher refused before scan_either), after-own*.log,
neigh-*.log, after.log (whole suite --no-fail-fast, exit 0), final.log,
final-durable.log, clippy.log, fmt-check.log, mutants.md, mut-results.jsonl.

## Not verified
- No device was seen to tear a slot write; the tests build the tear by hand.
- That fdatasync of one slot file leaves the other's page unwritten is reasoned.
- The kpageflags test only runs as root; in CI as a non-root user it prints
  and returns (a green that ran nothing).
- The open's added fdatasync was not timed (rule 10: no timing runs).
- The sweep-before-sync order (O6) is unreachable by any state the engine
  writes; kept, untested.
- before.log for every_arm/a_drain_after/a_store_moved: not run on the
  pre-fix engine (the seam-only engine does not watch the pending arm). The
  arm test's pending-cipher failure on the slot fix before scan_either is in
  arm-dbg.log / after-own2.log.

## REPAIR ROUND 1 (in progress)
Step log (latest last). Round-0 fixed copies kept at r1/db.rs.round0 (= keep/db.rs.fixed).
- step 1: reading the code for the three blocking items.
- step 2: editing src/db.rs (SlotChoice without synced; Maint::write_slot taking the owed entry before the write; tear seam; open-sync fail option)
- step 3: tests appended to tests/durable.rs (tear x2, open-sync x2); running them on the fixed engine, log r1/new-after.log
- step 3b: cipher open test allows the vault's pre-read sweep of envelopes neither slot names (they postdate the publish); rerun x5 -> r1/new-after2.log
- step 3c: cipher open test protects the files the older slot names (open of a copy with both slots = older); rerun x5 -> r1/new-after3.log
- step 4: ROUND-1 FIXED COPIES: r1/keep/db.rs.fixed, durable.rs.fixed, db_test.rs.fixed (sha256 in r1/keep/sha256.txt). These supersede keep/*.fixed (round 0). If src/db.rs differs from r1/keep/db.rs.fixed and r1/mutant-current.txt exists, a mutant is in the tree: copy the fixed one back.
- step 5: before run of the tear tests on the round-0 behaviour plus seams (mutant R1-owed-taken-after-write) -> r1/before.log
- step 6: mutant runner r1/mut1.py running all R1 mutants; verdicts appended to r1/mut-results.jsonl, stdout r1/mut-run.log
- step 7: runner run 1 stopped after 4 verdicts (two pattern misses after fmt; both owed-landed-before-sync mutants SURVIVED: no test failed a slot sync). Results kept at r1/mut-results-run1-partial.jsonl. Added seam Db::fail_next_slot_sync (Maint::sync_slot) and test a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off(+cipher). Fixed copies must be re-saved after this (step 8).
- step 8: fixed copies re-saved (r1/keep, sha256.txt). Runner run 2 started: all 16 R1 mutants -> r1/mut-results.jsonl, r1/mut-run2.log
- step 9 (pending): after run 2, replace the appended a_failed_slot_sync block in tests/durable.rs with r1/add-syncfail-test.rs (owed_only variant added; run 2 showed the owed-sync mutants survive the merge variant), re-save fixed, rerun survivors
- step 9 done: owed_only variant in; tear test control (open skips the torn slot); new test a_second_seals_slot_torn_over_an_owed_slot_...(+cipher); removed seam-only synced[older] seed (survivor R1-upgrade-seed-deleted). Running new tests -> r1/new-after5.log
- step 10: fixed copies re-saved (r1/keep). Runner run 3 on survivors -> r1/mut-run3.log (results appended to r1/mut-results.jsonl)
- step 11: run 3 done (all killed but R1-pending-owed-not-taken; R1-upgrade-landed-older pattern gone with the removed line). Whole suite --no-fail-fast -> r1/after.log

### Round 1 summary (final)
Blocking 1 (the open's fdatasync had no root-free guard; O6 had been called
unobservable):
- I added the seam Options::fail_open_slot_sync (#[doc(hidden)], one load at
  open). The open fails at its slot's fdatasync with the seam's words, before
  it has swept anything.
- I added the test an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it
  (+cipher). It builds the reviewer's kill state for real: the store copied at
  a merge's or a promotion's synced slot write, then the written slot and the
  files written since overlaid. It then checks three things. The failed open
  refuses with the seam's words. The directory is left as found: every file
  and its bytes in plain; under a cipher, every file the older slot names,
  because the vault's sweep, which runs before any read, removes envelopes
  neither slot names, a crash's leftovers whichever slot the device holds.
  Control: a successful open does remove some of those files.
- The test kills R1-O6 and R1-S3a without root.
- mutants.md's O6 entry is corrected.
Blocking 2 (a torn write over an owed slot was named durable by the owed
sync):
- Maint::write_slot is now the one slot write that publish, publish_unsynced
  and the pending landing make. It takes slot_sync_due before the write. On
  the success path these are the same operations as before, only moved, plus
  in publish_unsynced and the pending path a take and the existing test-hold
  load.
- A sync of the owed descriptor after the write has begun would cover this
  write's bytes, so the owed entry is never synced after it.
- Seam Db::tear_next_slot_write: half the image is written, then an error. It
  fires only over an owed slot.
- Tests: a_slot_write_torn_over_an_owed_slot_is_not_named_durable_by_the_owed_sync
  (+cipher), the merge or promotion over the drain's slot, with a control that
  the tear left a slot the open skips; and
  a_second_seals_slot_torn_over_an_owed_slot_... (+cipher), publish_unsynced
  over publish_unsynced.
- before.log: on the round-0 shape plus the seams (mutant
  R1-owed-taken-after-write), plain and cipher each lost 5000 of 20000
  acknowledged keys.
Extra (from the extra mutants): no test failed a slot sync.
- Seam Db::fail_next_slot_sync (Maint::sync_slot, one load beside the
  fdatasync).
- Tests a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off
  (+cipher), where the publish's own sync fails, and
  a_slot_whose_owed_fdatasync_failed_... (+cipher), where the owed sync fails.
- These kill the landed-before-sync mutants on both paths.
Blocking 3 (SlotChoice::synced stored on every publish):
- The field is removed. SlotChoice is { durable }, and landed(slot) is one
  store.
- The open keeps its synced sequences in a local array that seeds
  Shared::slot_synced.
- The open's seed for the upgrade's second write is removed. Nothing read it
  (mutant R1-upgrade-seed-deleted survived).
- The fix's minimum success-path cost, stated plainly: one store of
  `durable` into Maint's own SlotChoice per publish whose fdatasync returned,
  where parity needed no state. The rest is a field load and `1 - durable` in
  place of the parity compute, plus the seams' relaxed loads beside syscalls.
Survivor: R1-pending-owed-not-taken. No state the tests build reaches an owed
slot at a pending landing (see mutants.md).
Docs: CLAUDE.md (the shape entry is amended twice, and the crash-discipline
sentence on a publish before the owed sync), and docs/engine.md (the slots
paragraph).
Logs (r1/):
- new-after*.log: the new tests on the fix.
- before.log.
- mut-run*.log and mut-results*.jsonl.
- after.log: the whole suite with --no-fail-fast, exit 0. durable has 161
  tests, db 226, lib 404.
- clippy.log: exit 0.
- fmt-check.log: exit 0.
The test TMPDIR (stage0/tmp/slot-choice) is removed.
Not verified in round 1:
- I did not run the tests as a non-root user. R1-S3a is killed by the seam
  test, which reads nothing privileged, but the kpageflags test still prints
  "not run" and passes when it cannot read the page flags.
- The vault's sweep running before the open's slot sync is reasoned safe, not
  tested: it removes only envelopes neither slot names.
- That the pending path never meets an owed slot is reasoned for the states
  the tests reach. I did not prove it for every state.

## REPAIR ROUND 2 (in progress)
Step log (latest last). Round-1 fixed copies stay at r1/keep (superseded once r2/keep exists).
- step 1: blocking item: slot_sync_due is Option<File> now; sync_owed_slot takes slot = self.slot.next(), seq = self.manifest_seq (invariant documented on the field). Edited src/db.rs.
- step 2: built; durable whole + db slot tests on round-2 engine -> r2/after-unit.log
- step 3: r2/after-unit.log: durable 161 pass, db slot tests pass. ROUND-2 FIXED COPIES: r2/keep/{db.rs,durable.rs,db_test.rs}.fixed, sha256 in r2/keep/sha256.txt (supersede r1/keep). If r2/mutant-current.txt exists, a mutant is in src/db.rs: copy r2/keep/db.rs.fixed back.
- step 4: runner r2/mut2.py, all mutants -> r2/mut-results.jsonl (one line per verdict), stdout r2/mut-run.log; then E1 again with MUT_SKIP=an_open_makes_the_slot_it_takes_durable (root-only test) -> r2/mut-run-e1.log
- step 5 done: runner finished (r2/mut-results.jsonl). Killed: E2 E3 E4 E7 E8 E9 R2-owed-slot-is-durable R2-owed-seq-stale R2-owed-not-landed. E1 killed only by the root-only kpageflags test; with it skipped, SURVIVED. E5 and E6 SURVIVED. Tree restored (sha matches r2/keep).
- step 6: adding (a) the inode of the descriptor the open synced to fail_open_slot_sync's words + an assertion in an_open_whose_slot_sync_fails (E1/E2 without root); (b) the helper's fdatasync failable through fail_next_slot_sync (job carries the flag) + a pending-arm test (E5); (c) Options::tear_open_slot_write tearing the open's first slot write + an upgrade test in durable.rs (E6); crc32c_from and a v2 builder moved to tests/common. r2/keep is STALE during this step (tree edited on purpose).
- step 7: the (a)(b)(c) changes in; new tests pass (r2/new-after.log). r2/keep RE-SAVED (now also common_mod.rs.fixed = tests/common/mod.rs). Runner pass 2: E1 (MUT_SKIP root test), E2, E5, E6, R2-helper-fail-ignored, R2-open-tear-ignored, R2-helper-fail-no-stop -> r2/mut-results.jsonl, r2/mut-run2.log
- step 8: mutants pass 2 done (E1 killed without root, E2 E6 killed, E5 SURVIVED = equivalent, see mutants.md). Upgrade test's assert made a bool compare (no byte dump). r2/keep RE-SAVED. Whole suite + clippy + fmt -> r2/after.log, r2/clippy.log, r2/fmt-check.log (r2/whole.sh)
- step 9: clippy flagged the upgrade test's index loop; fixed (test only), test rerun r2/new-after2.log. r2/keep RE-SAVED (final). Whole suite r2/after.log exit 0 (durable 164, db 226, lib 404), clippy exit 0, fmt exit 0. TMPDIR removed.

### Round 2 summary (final)
Blocking (owed entry carried a slot and a sequence): slot_sync_due is
Option<File>. sync_owed_slot lands self.slot.next() and records
self.manifest_seq for the seam; the invariant (while owed, the slot is
next() and the sequence is manifest_seq, because every slot write and every
sequence bump takes the entry first in write_slot, and every slot.landed
follows such a write) is written on the field. Mutants R2-owed-slot-is-durable,
R2-owed-seq-stale, R2-owed-not-landed killed.
Extra mutants (mutants.md, Round 2): E2 E3 E4 E7 E8 E9 killed as they stood.
- E1 survived without root. fail_open_slot_sync's words now carry the inode of
  the descriptor the open synced; an_open_whose_slot_sync_fails holds it to the
  slot taken. Killed without root (r2/before.log).
- E5 survives: equivalent, because a failed helper sync stops the seal queue
  and no slot is written until a reopen. Seam: the helper's job carries a fail
  flag taken from fail_next_slot_sync (one relaxed load beside the channel
  send, on the pending landing only). New test
  a_pending_slot_whose_fdatasync_failed_stops_the_slots_until_a_reopen
  (+cipher) holds the premise (it kills R2-helper-fail-ignored).
- E6 survived. Options::tear_open_slot_write (#[doc(hidden)]) tears the first
  slot write the open makes, whichever it is (one bool param through the
  open's writers, open-time only). New test
  an_upgrade_never_writes_first_over_the_slot_the_store_was_read_from (plain
  only: a cipher store has no version-2 slot). Killed.
- crc32c_from moved to tests/common, with manifest_v2_of (the v2 image of a
  v3 manifest); db.rs's upgrade test uses it (same bytes, one definition).
Docs: CLAUDE.md shape entry amended (seam names its object; upgrade order; the
owed entry's two words).
Success-path cost (whole unit): no syscall, wait, lock or allocation added to
commit, sync, flush, drain, seal, rotation, publish or read. Per publish whose
fdatasync returned: one store of SlotChoice::durable into Maint. next() is a
field load and 1 - durable in place of the parity compute. The owed entry
is Option<File>, as large as before the fix's round 0. Seams: relaxed loads
beside the slot writes/syncs (watch, synced record, tear, fail), and on the
pending landing one relaxed load beside the helper's channel send, the job
tuple one bool wider. At open: one fdatasync of the slot read.
Not verified in round 2: no run as a non-root user (E1's non-root kill is by
the test skipping the root test, as root); E2's non-kill by
an_open_that_takes_an_unsynced_slot is reasoned, not traced; E5's equivalence
is by reading the two stop_seals callers.

## REPAIR ROUND 3 (in progress)
Step log (latest last). Tree at start matched r2/keep (sha256 checked). r2/keep is the fixed copy until r3/keep exists.
- step 1: reading the code for the three blocking items (parity doc; open sync vs cipher sweep).
- step 2: src/db.rs: the parity doc on MANIFEST_MAGIC restated; Db::open's cipher sweep (vault().sweep() and the on_disk listing) moved to after the slot's fdatasync, so the docs' "before the open removes any file" holds under a cipher. tests/durable.rs an_open_whose_slot_sync_fails: the cipher exception removed (nothing may change in either mode); control: the successful open removes a file the older slot names, and under a cipher sweeps an envelope neither names. r2/keep is STALE now. Running the open tests -> r3/open-after.log
- step 3: src/db.rs: the open's run recovery records its slot sync (synced[to]) as the other open writes do (seam record only). tests/durable.rs: OsTracker.bare (an armed write whose other slot held no manifest on the device); every_arm gains "inline" (publish_in_background off); a_failed_slot_sync asserts the engine's sync record at the first write after the failure; new tests the_first_publish_after_create_keeps_off_the_slot_create_wrote(+cipher), a_store_opened_without_a_manifest_keeps_the_slot_its_open_wrote, an_open_that_recovers_a_run_keeps_the_slot_that_names_it (for extra mutants X1 X2 X3 X5 X6). Running -> r3/new-after.log
- step 4: ROUND-3 FIXED COPIES r3/keep/{db.rs,durable.rs,db_test.rs,common_mod.rs}.fixed, sha256 in r3/keep/sha256.txt (supersede r2/keep). If r3/mutant-current.txt exists, a mutant is in src/db.rs: copy r3/keep/db.rs.fixed back. New tests pass: r3/new-after.log.
- step 5: runner r3/mut3.py, all 12 mutants (X1-X9 extra, R3 own) -> r3/mut-results.jsonl (one line per verdict), stdout r3/mut-run.log
- step 6: docs: CLAUDE.md shape entry amended (round 3), docs/engine.md sentence on the cipher sweep. A Db::open doc edit (the sweep named) was made into src/db.rs WHILE the runner ran by mistake: the runner's restore drops it; REAPPLY it after the runner and re-save r3/keep.
- step 7: runner done (r3/mut-results.jsonl): all killed but X4 (equivalent) and X7 (survived). Db::open doc edit reapplied. Tear test now asserts the torn write was over an owed slot (for X7). r3/keep RE-SAVED. Running tear tests on the fix -> r3/tear-after.log, then X7 again -> r3/mut-run-x7.log
- step 8: X7 killed (tear test's new assertion), X4 equivalent (r3/mut-run-x7.log). fmt applied; r3/keep RE-SAVED. Whole suite + clippy + fmt check -> r3/after.log r3/clippy.log r3/fmt-check.log (r3/whole.sh)
- step 9: whole suite r3/after.log exit 0 (durable 168, db 226, lib 404), clippy exit 0, fmt check exit 0. TMPDIR removed. r3/keep matches the tree.

### Round 3 summary (final)
FIXED COPIES: r3/keep/*.fixed (sha256 r3/keep/sha256.txt), matching the tree.
Blocking 1 (parity doc): the comment above MANIFEST_MAGIC now says a publish never writes the slot holding the newest manifest known durable (SlotChoice) and the open fdatasyncs the slot it takes before removing a file or writing over the other.
Blocking 2/3 (open sync vs cipher sweep): fixed in code, not by narrowing the words: Db::open runs vault().sweep() (and the on_disk listing) after the slot's fdatasync. The sweep depends only on the vault's adopted files from manifest_read_all; pending_whole reads pending segments, which no sweep removes. The Db::open doc, CLAUDE.md and docs/engine.md say the sweep is included. an_open_whose_slot_sync_fails drops its `under` exception: nothing may change in either mode; the control holds that a successful open removes a file the older slot names and, under a cipher, sweeps an envelope neither slot names. Mutants R3-sweep-before-sync and R3-sweep-deleted are killed.
Extra mutants: everything is killed except X4, which is equivalent. X1, X2, X3, X5 and X6 needed new tests or assertions; X7 needed the tear test's assertion. The open's run recovery now records its slot sync for the seam, as the other writes the open makes do.
CLAUDE.md shape entry amended (round 3).
Not verified: no run as a non-root user. X4's equivalence is by reading.
STEP (commit stage): gate running, log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/fix/slot-choice/gate.log, status /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0/fix/slot-choice/gate.status
STEP: committed 30ecd93d52fe670f89cf69652afc8fa2651ddf80
