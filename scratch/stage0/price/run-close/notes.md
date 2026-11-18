# run-close (pricing unit)

Base: HEAD 2e214887 (on top of 0c6748bb). Tree clean at start.

## Step
1. reading (done): close_run, leave_direct, close_direct, hand_tail, Db::sync, sync_wal, commit_direct.

## Design
- close_run (cold, when self.unsynced != 0 and not RUN_LOGGED): writer writes what it holds (no sync)
  before hand_seal; a ClosedRun {file opened by path (fresh description, so errseq reports a
  writeback error to it whatever the seal's description saw), durable: Arc<AtomicBool> set by the
  job after its fsyncs} pushed onto self.closed_runs.
- syncs: sync_wal top, Db::sync (inside unsynced!=0), commit_direct's due branch (EveryN arm):
  cold sync_closed_runs: durable -> drop; else fdatasync; failure -> wedge + keep_lost.

## Step 2 (done): tests in tests/durable.rs, section "run-close", seam Db::fail_next_closed_run_sync (FAIL_SYNC_CLOSED bit, read only in the fix's cold path).
before.log: 5 of 6 fail before the fix:
- a_later_commit_survives_a_kill_only_with_the_closed_run_before_it: 24/24 cases (2 arms x 6 closes x small/large), "killed with the later commit read back: 300 of 300 / 4000 of 12000 keys ... did not read back"
- a_sync_after_a_run_closed_into_a_seal_in_flight_makes_the_runs_commits_durable: 24 cases x (kill, cut) = 48 failures
- the_policys_due_sync_after_a_run_closed_covers_the_closed_run_first: 16/16 (12 log, 4 run): "200 of 200"
- a_sync_over_a_closed_run_whose_seal_failed_makes_it_durable_or_answers: 4/4, power cut after Ok sync loses 300/300
- a_failed_sync_of_a_closed_runs_file_is_answered_and_kept: "default-sync: ... answered Ok"
- a_run_closed_under_always_leaves_no_sync_owed: PASSES before (holds behaviour; not counted as evidence)

## Step 3 (done): fix in src/db.rs + src/db/vault.rs (keep/ has the copy incl. the two price_* fields)
- SegmentWriter::write_held = sync_with's write half (sync_with calls it, inline(always)).
- close_run: `if self.unsynced != 0 { close_owed }` (cold): write_held, open the temp by path (own description), push ClosedRun{file,path,durable}; job sets durable after seg.sync_all (+ord).
- syncs: commit/commit_staged/commit_direct/Db::seal (EveryN arm, when due) and seal_rotates_wal branch, Db::sync (plain check before the log), run_again (cold).
- seal-failed test: close held so the closing commit is answered before the seal fails (timing).
- Measuring fields: Options::price_run_close_old (close_owed returns None), price_run_close_syncrun_old (Db::sync's run_owed false).
## Step 4: neighbours running in background: neigh.sh -> n-<test>.log, neigh.status

## Step 5 (pricing) in progress
- bench built (bench-build.log exit 0); measuring arms: supdb-runcloseold, supdb-ingestruncloseold, supdb-ingestsyncruncloseold, supdb-ingestshaperuncloseold (price_run_close_old) and supdb-ingestsyncsyncrunold, supdb-ingestshapesyncrunold (price_run_close_syncrun_old, item 7).
- runner: pairs.sh <list>; pairs/status.txt; tables pairs/<A>-vs-<B>-<SIZE>-{1,2}.out
- keep/ currently holds the MEASURING tree (with price_* fields and bench arms).

### Pricing so far (B/A = old/new; >1 old faster)
- item7 (landed already, sync-run): ordered load 10k: ingestshape 0.676x/0.641 -> new at 0.64-0.68 of old in both sittings (1.56x/1.48x old faster); ingestsync 1.506/1.55. 300k: ingestshape 1.109 (p.039)/0.976, rerun 1.007/0.979 -> unresolved; ingestsync 0.988/0.967 level. Closing sync phase 10k: 1.33-1.38 ms vs 0.05-0.06 (old); 300k: 4.26-4.58 vs 3.25-3.45.
- unit, rates: supdb level both sizes; ingest 10k level, 300k 1.069 (p .039)/0.994 then rerun 0.953/0.992 -> unresolved; ingestshape level; ingestsync 10k level, 300k 1.032 (p .006)/0.971, rerun 1.015/0.986 -> unresolved. Reads/scans level everywhere.
- bytes: device_bytes_per_byte (/proc write_bytes) and bytes_on_disk +0.9% at 300k on ingestshape/ingestsync both sittings 12/12; /sys/block/vda/stat over the whole load_only life: 43.108 MB vs 43.103 MB (6 reps each) -> device writes level.
- stamped phase (loadphases sync, ordered, 300k ingestshape): 4.71 vs 3.76, 4.50 vs 3.38 ms -> +1.0-1.1 ms both sittings, sittings differ 0.2-0.4 -> RESOLVED cost (the closed run's fdatasync at the closing Db::sync, strace: fdatasync of direct-0 3.5 ms then open run 0.7 ms, vs open run 4.0 ms old). 10k: level (no threshold close).
- next: alt 1 (wait for the seal's durable landing instead of the fdatasync) built behind price_run_close_wait, priced the same way.

### Alt 1 (wait for the seal's durable landing in place of the fdatasync; arm supdb-ingestshaperunclosewait, supdb-ingestrunclosewait, field price_run_close_wait)
- ingestshape 300k ordered load: 1.382x (p .006) and 0.735x reversed -> new at 0.72-0.74 of old in both sittings; closing sync 31.7/33.2 ms vs 4.25/4.11. 10k level. ingest (flush) 300k level.
- -> resolved cost, larger than the main shape's.
- Alt 2 (the write made by the first log write after the close) moves the close's write only; the main shape's commits phase was level (35.16 vs 34.94, 35.64 vs 35.60 ms), and the cost is the closing Db::sync's fdatasync of the closed run, which alt 2 keeps: not built.

## Verdict: does not land (rule 13f). Every shape the brief allows costs on the closing Db::sync of the buffered arms that close with Db::sync (supdb-ingestshape at 300k): main shape +1.0-1.1 ms of 3.4-3.8 (phase, both sittings); alt 1 +27-29 ms (rate 0.72-0.74x).
- after.log: all 8 unit tests pass on the fix; before.log (6 tests, first two rounds) and before2.log (the 8 tests with price_run_close_old forced on): 7 fail, the Always test passes in both (holding test, not evidence).
- neighbours on the fix: n-durable 232/232, n-db 226, n-cipher 24, n-fuzz 5, n-segwriter 19, n-lib 404 (the db..lib runs straddled a one-line restructure of commit_staged; durable ran on the final engine minus that restructure).
- not-landed.patch = git diff of the measuring tree (fix + tests + docs + price_* fields + bench arms). Tree put back.
