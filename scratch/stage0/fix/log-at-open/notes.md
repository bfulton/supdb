# log-at-open (sync-completion-order -> "replayed frames marked synced", wal-fresh-header): notes

Base: HEAD 38378d1d (delete-max, log-errors, run-errors committed on 3df85d22). Uncommitted:
src/db.rs, tests/durable.rs (section "log-at-open"), CLAUDE.md, ROADMAP.md.

## What was wrong
1. Replayed frames marked synced. `Wal::open_append` set `synced = written` for whatever replay kept,
   on the reading that it was read back from the device; after a process kill it was read from the
   page cache. `Db::sync` then found nothing unsynced and answered Ok without an fdatasync; the
   first flush's drain read `wal_unsynced` false and was answered before its slot's fdatasync; and
   the older log files the open replayed were not on `loose_wals` (the open started it empty), so
   the first sync would chain the live header over a file a power cut could still cut, and the open
   after that cut refused a gap at a chained boundary. A rotation right after a reopen (a commit of
   nothing over a replayed file past `seal_bytes`) closed the replayed file CHAINED.
   The defect as named (a sync completing out of order) was refuted by the reproducer; not touched.
2. wal-fresh-header. A log file no sync covered (fresh store, a file a rotation began, the log under
   a durable direct run, a buffered store's log a flush acknowledges past) can lose its first page
   to a power cut with its size kept; replay refused eight zero bytes as "not a supdb WAL" (plain)
   or "not this store's log" (cipher), at every open.

## What changed (src/db.rs)
- `wal_head_lost(buf)`: the first min(len, 512) bytes all zero. `Wal::replay` reads such a file as a
  log with nothing in it (returns (from, 0, false)) before the plain/sealed split, so one line covers
  both arms. A zero header over a first sector that holds anything else stays refused.
- `Db::open`: records per file whether replay applied a frame (`applied`). Older files that applied
  frames go on `loose_wals` (oldest first); the live file is `settled` (its synced mark = written)
  only if replay applied nothing from it or the open's cut of a torn tail fdatasynced it; otherwise
  synced = 0 and `frames_unsynced` is true. A header the open writes (file missing, or cut to
  nothing after a lost header) is LOOSE when `loose_wals` is non-empty, CHAINED otherwise.
- `Wal::open_append(path, id, seq, kind, settled)`.
- Seams (#[doc(hidden)] pub, no cfg, free when unset):
  - `Db::trace_wal_syncs(on)` / `wal_syncs_traced()`: every fdatasync of a log file that returned Ok
    (live, loose, a recycled rotation's prefill) with the length it covered, and every chained-header
    rewrite (`None`), in order. Each record is an `if let Some` beside the syscall; `Wal::chain` takes
    the trace as an argument and records only on its loose path (after its own open+write).
  - `Db::hold_owed_slot(on)` (a bit, SEAL_HOLD_OWED_SLOT, on the existing `seal_hold`) and
    `owed_slots_held()`: `sync_owed_slot` waits on the bit before its fdatasync (one load beside it),
    `publish_unsynced` counts a slot only while the bit is set (one load beside the slot's write).
  The reproduction's `Wal::fsynced` store on every `Wal::sync` was not taken (rule 5: a store on the
  commit path); the trace replaces it.
- Docs: Db::open, Db::sync, SyncPolicy, `frames_unsynced`, `open_append`, `wal_head_lost`.

## When a zero header is read as empty (decided; round 1 restated)
Model: a power cut keeps or loses a sector (512 bytes) whole; an fdatasync of a log file covers its header with its frames; a chained header is written only by a sync that has synced every file before it first.
Rule (one line, `Wal::replay`, before the plain/sealed split): a log file whose first min(len, 512) bytes are zeroes replays nothing and returns ended = false. What follows from it, case by case:
- Newest file, any kind of boundary before it, Always or EveryN: nothing of it was acknowledged (an acknowledged frame of it would mean a sync that covered its header). Read as empty, cut to nothing, given a fresh header (LOOSE if an older replayed file is loose, else CHAINED). Tests: a_store_just_created_* (plain, cipher), acknowledged_direct_run_commits_*, a_flush_survives_*_first_page_* (plain, cipher; page and whole), a_log_file_a_rotation_began_* (Always, chained boundary), a_header_an_open_writes_beside_an_unsynced_closed_file_is_loose (loose boundary).
- Older file, its successor LOOSE: reachable by a power cut (no sync covered it); the gap before the successor's first frame ends the log there, the successor's frames dropped, which is allowed because a loose successor says nothing was synced across the boundary. Test: an_older_log_file_whose_unsynced_header_a_power_cut_lost_opens_as_the_logs_end.
- Older file, its successor CHAINED: unreachable by a power cut (the chain was written after that file was synced whole), so it is damage; the open refuses "wal sequence gap" and leaves the directory as found. Tests (round 1): a_zeroed_older_log_file_before_a_chained_one_is_refused_as_damage{,_under_a_cipher}; they kill X5 (end the log at the zero file), which would delete the chained file's acknowledged frames silently.
- Valid frames behind a zero first sector (newest file, either policy): under Always they are unsynced or damage; the open cannot tell them apart (it does not know the last life's policy, and a frame boundary may be inside the lost sector), so they are dropped. Test: frames_behind_a_lost_first_sector_are_dropped_as_unsynced, with 512 bytes lost (frames kept in 512..4096) and with the page lost (kills X6). Damage that zeroes exactly a first sector of acknowledged frames reads as an empty log: a stated limitation.
- Zero header bytes with anything non-zero in the rest of the first sector: not a lost sector, refused "not a supdb WAL" with files unchanged. Test: a_zeroed_header_over_frames_in_its_first_sector_is_refused_as_damage; and a_foreign_log_header_is_refused.
- recycle_wal: a fresh file is written through and synced with its header at creation, and a recycled file's header is rewritten in place over one an earlier life synced; a crash leaves neither zero, so the rule meets them only as damage, or as a prefill of zeroes with no frame in the sector (empty, right). The round-0 ROADMAP item (a recycled header's rewrite lost with this life's frames surviving behind the old header) is withdrawn in round 1: `Wal::recycle` writes the new header at 0 and this life's frames from 8 (plain) or WAL_SEALED_LEN (sealed), all in sector 0, and the first commit frame's batch CRC covers bytes from 8, so this life's frames replay only where sector 0 is the new one; the old header with this life's frames behind it needs a tear inside a sector, which this model excludes. The ROADMAP's device item now says the reading takes a sector to be kept or lost whole, and that a tear inside one would undo both the first-sector rule and the recycled rewrite.
- Sealed log under a cipher: the same rule, applied before `Wal::resume`. The cipher's recycled rule (a sealed header whose key is not this file's reads as empty when `recycles`) is unchanged.

## Tests (tests/durable.rs, section "log-at-open"); before.log, after-mine*.log
Failed before the fix (18), each at:
- a_sync_after_a_reopen_makes_the_replayed_writes_durable_{buffered,always,under_a_cipher}: every_key_of "sync after a reopen: key b0000 ... is gone after it".
- a_flush_after_a_reopen_is_answered_only_once_its_writes_are_durable_{buffered,always}: same, "flush after a reopen".
- a_sync_after_a_reopen_makes_a_replayed_loose_log_file_durable: "the closed file's frames: r0000000 reads back once" (before the fix the sync issued nothing, the live header stayed loose, and the open after the cut ended the log at the boundary, dropping what the sync acknowledged).
- a_rotation_after_a_reopen_leaves_the_replayed_file_loose: open refused "wal sequence gap".
- a_header_an_open_writes_beside_an_unsynced_closed_file_is_loose: first open refused "not a supdb WAL".
- a_store_just_created_..._unsynced_header{,_under_a_cipher}: open refused ("not a supdb WAL" / "not this store's log").
- acknowledged_direct_run_commits_survive_...: both opens refused.
- a_flush_survives_..._first_page_of_a_never_synced_log{,_under_a_cipher}: both page and whole cases refused (the whole case now runs; the loop collects).
- a_log_file_a_rotation_began_whose_header_a_power_cut_lost_...: refused.
- an_older_log_file_whose_unsynced_header_a_power_cut_lost_opens_as_the_logs_end: refused.
- frames_behind_a_lost_first_page_are_dropped_as_unsynced: refused.
Passed before; guards, not evidence of the fix, each held to mutants:
- a_reopen_with_nothing_replayed_owes_its_log_no_sync (control: closed; killed after a flush; killed with a torn tail the open cuts and syncs; killed beside a covered closed file) -- trace empty after sync and flush. Kills S3, O2, O3, O7, O8, O10.
- a_sync_that_chains_a_loose_boundary_makes_the_chained_header_durable -- kills O6 (the log unit's survivor).
- a_zeroed_header_over_frames_in_its_first_sector_is_refused_as_damage, a_foreign_log_header_is_refused -- words + files unchanged; kill W1, O4.
- a_failed_log_write_in_a_recycled_log_loses_no_commit_acknowledged_after_it, a_store_whose_log_rotated_after_a_failed_write_survives_a_power_cut (plain and recycled): the log unit's open items; pass, no fix needed.
Power cut model: `power_cut_logs` cuts each log file to the longest length a traced fdatasync of it covered in any life (never below the header), reverts a header chained after its file's last sync to LOOSE, and keeps whole the files named (a later file's unsynced frames written back). A kill is a copy of the files (`killed`), the store dropped and never forgotten, since a store under a cipher is one open per process.
Skeptic problems dealt with: the seam store on Wal::sync replaced by a trace behind is_some; the whole-file case runs; the rotation sibling under Always has a test; the older-files-on-loose-list case has a test; rotation after a reopen has a test (by a commit of nothing over a file past seal_bytes -- by-size rotation after a write already closed loose, and a seal syncs first).

## Mutants: 21 run, 21 killed (mutants.md).

## Restated tests: none. No existing test went red.

## Runs
- neighbours.log (first form of the fix): durable 92, db 226, cipher 24, lib 404, fuzz 5, segwriter 19, all ok.
- after.log: whole suite --no-fail-fast + clippy (see bottom).
- fmt: `sh scripts/fmt.sh --check` failed on this unit's files only; applied with `sh scripts/fmt.sh`, then --check exit 0.

## Not verified
- No real power cut or device; the states are built by hand (ROADMAP item added). Which states ext4/xfs defaults reach is the skeptics' reasoning, not observed.
- No callgrind count of the seams' is_some checks on the commit path (rule 10 forbids timing; callgrind not run).
- The recycled-header defect above: reasoned, no test written (it would fail with no fix in this unit).
- vault/pager/memory files: no analogue found by reading (envelopes are synced before a slot names them; the marker is synced at creation; memory files have no fsync to trust).
- after.log: whole suite `cargo test --profile checked --no-fail-fast`, every binary ok (20 result lines, 0 failed); clippy `--release --all-targets -D warnings` first failed on an unused initial `loose` in open_append (every branch assigns it now), fixed to `let loose;`, clippy exit 0, durable rerun 92/92, fmt --check exit 0.

## Repair round 1 (in progress)
Blocking: (B1) CLAUDE.md says flush syncs the replayed files; (B2) zero older file before a CHAINED successor untested, mutant "ended=true" survives; (B3) ROADMAP recycled-header item claims a refusal the sector model excludes. Plus 9 extra mutants (X1..X9).
Step 1 done (src): Db::wal_opened = the open's cut fdatasync (path, len), set by the sync's own expression; trace_wal_syncs(true) seeds the trace with it. Round-0 tests kept at durable.rs.round0.
Step 2 done (tests): control seeds checked; frames_behind_... renamed ..._first_sector_... looping 512/4096 (X6); new: a_zeroed_older_log_file_before_a_chained_one_is_refused_as_damage{,_under_a_cipher} (B2, X5), a_reopen_that_cuts_a_torn_tail_makes_the_replayed_writes_durable{,_under_a_cipher} (X1, X8), a_failed_sync_of_a_loose_log_file_leaves_the_live_header_loose{,_under_a_cipher} (X2), a_sync_after_a_reopen_syncs_every_replayed_older_file_oldest_first (X3, X7).
Next: build+run section; docs B1/B3; mutants X1-X9 + own R1-R3 via mut2.py.
Step 3: docs B1/B3 done; fmt applied; keep2/ holds the tree; mut2.py running (log mut2-run.log). After: verify sums, rerun durable+neighbours, clippy, notes/mutants.md.
Step 4: mutants run once (mut2-run-first.log); the three-file test was flaky (seals landing), rewritten with hold_seal_landing, 360 stress runs ok; re-running X7 X9 R2.
Step 5: rerunning round-0 mutants on the round-1 tree: mut0r1.py -> mut0r1-run.log

## Repair round 1, finish (after the third restart)
Step 1: tree matched keep2/sums. Diff read: no mutant. Found the round's seams (trace_wal_syncs etc.) inserted between fail_next_wal_sync's doc and its fn; moved after it.
Step 2: B1, B2 (+X5), B3 confirmed dealt with in the tree. Nonblocking taken: cipher variant of a_flush_after_a_reopen (rule 3), Db::flush doc, wal_head_lost doc (a file the open begins is not written through), CLAUDE.md "eight zero bytes" -> first sector, "unless the device changed bytes it had synced", ROADMAP item for a zeroed first sector over frames that verify. fmt --check exit 0; durable 100/100 (r1b-durable.log).
keep3/ is THE FIXED TREE now (db.rs, durable.rs, CLAUDE.md, ROADMAP.md; keep3/sums). keep2 is superseded.
Step 3: mut0r1.py (pointed at keep3, writes mut0r1-results.json after each verdict) for S4 S5 W1-W5 O1-O11; log mut0r1b-run.log.
Step 3 done: all round-0 mutants killed on keep3 (mut0r1b-results.json, O9 restated). Step 4: whole suite -> all-r1b.log
Step 4 done: whole suite --no-fail-fast EXIT 0, every binary ok, durable 100/100, db 226/226 (all-r1b.log); clippy exit 0 (clippy-r1b.log); fmt --check exit 0 (fmt-r1c.log). TMPDIR removed.
Step 5 done: mutants.md has the keep3 table. Repair round 1 COMPLETE; the work is uncommitted in the tree, matching keep3/sums.
Review items: B1 (CLAUDE.md flush sentence) done in round 1; B2 (zero older file before a chained successor, X5) done (a_zeroed_older_log_file_before_a_chained_one_is_refused_as_damage{,_under_a_cipher}); B3 (ROADMAP recycled item) restated into the device item's sector model. Nonblocking taken in this finish: doc placement, cipher flush test, Db::flush doc, wal_head_lost doc on a file the open begins, CLAUDE.md wording x2, ROADMAP item for a zero first sector over frames that verify. Not taken: the stale all-status.log (left as a record; superseded by all-r1b.log); "once" in the success_path_cost account is corrected in the result (the first flush is answered behind its slot until a sync clears loose_wals).

## Repair round 2 (extra mutants E1-E5 + five unnamed; no blocking)
Step 1: tree matches keep3/sums at start. keep3/ is THE FIXED TREE. Runner: mut3.py, results to mut3-results.json after each verdict, log mut3-run.log.
Step 2: control test gained case 4 (covered closed file + lost live header): header chained, no chain at first sync. Building.
Step 3: keep4/ is THE FIXED TREE now (db.rs unchanged from keep3; durable.rs with case 4; sums in keep4/sums). Running mut3.py (E1-E5, U6-U10) on keep4/db.rs; each verdict appended to mut3-results.jsonl as it lands; log mut3-run.log. If src/db.rs != keep4/db.rs after a restart, copy keep4/db.rs back.
Step 4: mutants done (all killed but U10, equivalent: ended => next == from, the one 'true' return in replay_frames answers from). Took '!ended &&' out of applied.push in src/db.rs. keep4/db.rs is now superseded by keep5 (to be made after fmt/test).
keep5/ is THE FIXED TREE (db.rs with the clause out). Step 5: whole suite -> all-r2.log, status all-r2-status.log
Step 4b: mutants.md updated with round 2 table.
Step 5 done: whole suite `cargo test --profile checked --no-fail-fast` on keep5 exit 0, every binary ok (durable 100/100, db 226/226, cipher 24, lib 404, fuzz 5, segwriter 19, zpages 136 ...; all-r2.log), clippy --release --all-targets -D warnings exit 0 (clippy-r2.log), fmt --check exit 0 (fmt-r2b.log). TMPDIR removed. Tree matches keep5/sums.
Repair round 2 COMPLETE. Changes this round: tests/durable.rs control test case 4 ("killed beside a covered closed file, the live header lost"), which kills E3/U7; src/db.rs `applied.push(next != from)` (U10 equivalent, clause removed, comment). No doc or contract changed (the crash-discipline paragraph already says a header the open writes is loose only where an older file is loose). The case passes on keep3 too: it is a mutant guard, not before/after evidence.

## Repair round 3 (nine extra mutants Y1-Y9; no blocking)
Step 1: tree matches keep5/sums at start. keep5/ is THE FIXED TREE. Runner will be mut4.py, results to mut4-results.jsonl after each verdict, log mut4-run.log.
Plan: Y1..Y9 = the round's nine extra mutants in list order (Y3 = seal_hold_wait deleted, Y9a = owed_slots_held fetch_add deleted; each also run with S4); Y5 (raw sync at open) expected to survive: no in-process seam sees a raw syscall, so check the fixed tree once by strace (count, no timing) and add a ROADMAP item. Step 2: a control test for the owed-slot seam (a flush whose log owes nothing leaves its slot owed and the closed log file it covers until the slot syncs), so the flush tests' seam is held by a test that needs it to fire.
Step 2 debug: src/db.rs temporarily has an eprintln in retire_covered; restore from keep5/db.rs
Step 2 finding (debug runs r3-dbg.log, r3-dbg2.log; src restored to keep5 after each, cmp ok): with the store's merges on, the drain joins a merge whose synced publish supersedes the owed slot BEFORE the answer (Maint::drain -> join_compact -> join_merge -> publish takes slot_sync_due). So in a_flush_after_a_reopen_* the S4 "kill" was the instrument's: owed_slots_held counted a slot already superseded, and the test reverted a slot that was synced. Fix of the instrument: publish's take of an owed slot decrements the count while held (src change, seam only). Tests: a_flush_after_a_reopen runs again with compact off (no merge), where S4 is a real bug; control test uses compact off + seal_rotates_wal.
Step 3: keep6/ is THE FIXED TREE (instrument decrement in Maint::publish, flush tests with merges off, control test); sums keep6/sums. Next: mut4.py Y1..Y9 + combos + own Z, results mut4-results.jsonl
keep6 refreshed (control test second half, merges on: owed==0, file retired). keep6 is THE FIXED TREE.
Step 4: mut4.py running (log mut4-run.log, verdicts mut4-results.jsonl). If src/db.rs != keep6/db.rs after a restart, copy keep6/db.rs back.
Step 4 done: mut4 all killed but Y5 (raw sync at open, survivor), Y7, Y8a (equivalent). Step 5: strace of the clean-reopen control on the fixed binary (r3-strace.*)
Step 5 done (strace: no sync of a reopened log's files in cases 0,1,3; clean-2/clean-4 the open's cuts and case 4's later sync, r3-strace.txt). Docs: CLAUDE.md amendment under 'A second mechanism that masks the first in a test', ROADMAP item for Y5. keep6 refreshed. Step 6: whole suite + clippy -> all-r3.log, all-r3-status.log
Step 6b: mutants.md has the round 3 table
Step 6 done: whole suite `cargo test --profile checked --no-fail-fast` on keep6 exit 0, 20 result lines all ok (durable 101, db 226, cipher 24, lib 404, zpages 136, segwriter 19 ...; all-r3.log), clippy --release --all-targets -D warnings exit 0 (clippy-r3.log), fmt --check exit 0 (fmt-r3c.log). TMPDIR removed. Tree matches keep6/sums.
Repair round 3 COMPLETE. src change: Maint::publish takes a superseded owed slot off owed_slots_held while the hold is on (seam only; inside the rare `owed` branch beside the slot's write), and its docs. Tests: a_flush_after_a_reopen_* run again with compact off (the honest S4 kill), new control a_drain_answered_before_its_slots_sync_keeps_the_log_file_the_slot_covers (kills Y3, Y9a, Y9aS4, Z1, Z2, Z4). Survivors: Y5 (ROADMAP + strace by hand), Y7 and Y8a equivalent. Docs: CLAUDE.md amendment, ROADMAP item.

## Commit step
Step C1: tree checked against keep6/sums; gate running -> gate.log, gate-status.log
Step C2: commit message drafted at commit-msg.txt; waiting on gate
Step C3: gate RED: durable a_rotation_after_a_reopen_leaves_the_replayed_file_loose (manifest names seg-...402 missing from the crash copy; gate.log). Not committing. Rerunning the test alone -> rerun.log
Step C4: rerun: alone 0/40 fails, whole durable binary 3/10 fails, same assertion (rerun.log, rerun-whole-fail-*.log). Test flake: copy_store of a live dir while a seal (seal_bytes 4096) lands. NOT COMMITTED; tree unchanged, matches keep6/sums. TMPDIR removed.

## Repair round 10 (gate red: a_rotation_after_a_reopen flake under the whole binary)
Step 1: tree matches keep6/sums at start. keep6/ is THE FIXED TREE. Reading the test.
Step 2: test edited (hold_seal_landing around the commits and the copy, asserts after the release); rotate_wal_by_size doc corrected (it said the rotation syncs first). Building; keep7 after the runs.
Step 3: whole durable binary 40 runs, 0 fail (r10-stress.log; was 3/10). Next: format, keep7, re-run mutants the test is credited with, then the gate.
Step 4: fmt --check exit 0 (fmt-r10.log). keep7/ is THE FIXED TREE now (sums keep7/sums). Next: mut10.py re-runs S1 S3 S4 S5 O8 O10 Z1S4 Z3 on keep7 (verdicts to mut10-results.jsonl as they land, log mut10-run.log). If src/db.rs != keep7/db.rs after a restart, copy keep7/db.rs back.
Step 5: mut10 done, all killed (mutants.md round 10). src matches keep7. Step 6: gate sh scripts/check.sh build test lint -> gate-r10.log, status gate-r10-status.log
Step 6 done: gate `sh scripts/check.sh build test lint` EXIT 0, "all checks passed: build test lint", every test binary ok (durable 101, db 226; gate-r10.log). CLAUDE.md amended (A test that bet a seal would still be running: the copy) after the gate started; keep7 refreshed with it. TMPDIR removed.
Repair round 10 COMPLETE. Tree matches keep7/sums. Not committed.
## Commit step (final): gate -> gate-final.log, gate-final-status.log
Committed cdb5296b; TMPDIR removed.

## Orchestrator's note, 2026-11-17 04:40 UTC: the commit's name on the branch
The commit step committed this unit as cdb5296b. The orchestrator had pushed an in-progress snapshot of the
unit to the remote branch earlier (acec5f49, "In progress: the log at an open, not gated"), so the unit's
commit was made again with the same tree, message, author and dates on top of it, behind one commit that
takes the snapshot back (0127fe3d): on the branch the unit is a424903d. `git diff cdb5296b a424903d` is
empty. In `git log 3df85d22..HEAD` the two "In progress: ..." commits are the snapshot and its taking back;
they are no unit's work, and together they change nothing.
