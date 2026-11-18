A run's due commit syncs the log first wherever the log owed a sync when the run opened

What a caller saw, in the clear: a run's commit the policy syncs
fdatasynced the run's file and not the log. A run can open over a log
that still owes a sync -- the frames of a hashed table frozen before the
run under EveryN, a log file a rotation left loose, or the frames an
open replayed (under Always too, after a reopen of a buffered life) --
and a power cut after the due commit kept the run's commits and lost
those older ones, which is not a prefix of the commit order. The tests
read it as "kept 20 of the run's 20 keys and lost 150 of the 150
committed to the log before the run", and the same for a loose file and
a reopen.

What the fix does. Where a run opens (commit_direct's run-open branch,
once a run, cold) it notes whether the log owes: unsynced frames or a
loose file (Direct::log_owed); not under a cipher, where log_run's due
sync covers the log already. The due branch asks the note; its cold
path (Db::run_due_log_first) asks the log again, syncs it if it still
owes, clears the note, then syncs the run. Db::run_again, the sync of an
owed batch that stands in for a due one, syncs the log first too while
the note stands. A log sync whose error sync_wal does not keep (a loose
file that would not open, a failed header chain) marks the batch owed by
the run's writer (SegmentWriter::owe_unsynced), so a later call with
nothing new writes and syncs it again, log first, and never answers Ok
over it.

When nothing fails: a run's due commit pays one load and one branch; a
run's open pays two loads and a compare in its cold branch. Where the
log owes, one log fdatasync at the run's first due commit. No cell of
the suite reaches the cold path: zero trace lines in every sitting and
probe.

Pricing, against the old shape kept in the measuring tree only
(Options::price_due_log_old with arms supdb-duelogold and
supdb-ingestduelogold, the twins of supdb and supdb-ingest):
- OUT=... sh bench/scripts/pair.sh supdb supdb-duelogold 10000
  --passes load,lag,mixes: split across sittings and pairings (one
  sitting the lag cells faster for the fix, the other one lag cell
  slower; the replication the load and lag0 faster for the fix, the
  other sitting nothing marked): no cost resolved.
- the same at 300000: nothing marked in either sitting.
- pair.sh supdb-ingest supdb-ingestduelogold 10000 --passes
  load,lag,mixes, three pairings: marks in one sitting only of two, and
  the third pairing level: unresolved.
- the same at 300000, three pairings: single-sitting marks within a few
  percent either way: unresolved.
- SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1 pair.sh ... --passes load, the
  ordered commits phase, fix against old: durable 300k about 206 and 210
  ms against 224 and 196, 10k 7.1 and 8.3 against 6.7 and 8.2; buffered
  300k 38.2 and 38.3 against 38.0 and 38.9, 10k 2.0 and 1.9 against 1.9
  and 1.9: split, or within the gap between sittings.
- strace -f -c over bench/examples/load_only.rs 300000 1 and
  lag_only.rs 300000 1, each arm: the same syscalls (supdb 303
  fdatasync either arm on the load, 627 against 628 on the lag sweep).

tests/durable.rs gains a due-log section that builds each state on disk
and cuts the power after the due commit, on every run arm and under a
cipher: frames of a frozen table, a loose file, a reopen under Always,
a log sync that fails, a loose file that will not open (under EveryN
and under Always), an owed batch written again by a commit of nothing
and by a sync, a log that owes nothing (no log sync), and holding tests
for the windows still open. Each reproduction failed before the fix and
passes after. The tests were held to about fifty mutants over four
rounds; two survive, argued as cost only (the note left set after the
cold path, in run_due_log_first and in run_again: a later due commit
asks the log again and finds nothing owed). The doc comments on
SyncPolicy, Direct::log_owed and the two cold paths, the comment in
Db::sync, CLAUDE.md's ordered-ingest
paragraph and its entry "A fast path gated on the case it was written
for", and ROADMAP.md say what is true now.

Left open:
- A run's commits reach the device before its first due sync by other
  ways -- the helper thread that fdatasyncs every sync_ahead of file
  (the only sync a buffered run gets), kernel writeback, and a run
  closed before its first due commit, synced by the flush's presync and
  its seal's fsync -- and a power cut then may keep those and lose
  older log frames. Held open by
  a_runs_commits_its_helper_synced_before_a_due_one_outlive_older_log_frames_until_it
  and a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames;
  ROADMAP.md names the shapes, each to be priced first.
- The note covers the log alone. A run closed into a seal still in
  flight, with commits its policy left unsynced, and then a later run
  whose due commit syncs the log and its own file: a power cut keeps the
  later run's commits and loses the closed run's. The docs say "every
  commit of the log before it (not those of a run closed earlier into a
  seal still in flight)", and
  a_runs_due_commit_after_a_run_closed_into_a_held_seal_outlives_that_runs_commits
  holds the window open; the ROADMAP item for a sync over a run closed
  into a seal in flight is widened to it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GEwcALZsCzTm28PHd4bQza
