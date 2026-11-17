# Deferred from stage 0 (written by the sync-run unit)

Each item below is a reproduction that still fails on the tree the sync-run
unit leaves (3df85d22 + delete-max, log-errors, run-errors, log-at-open
committed, sync-run uncommitted). None of their tests is in tests/durable.rs:
they would fail.

## run-tail-kill, test 1 (a_later_wal_commit_survives_a_process_kill_only_with_the_run_before_it)

State: reproduced at 3df85d22 (stage0/run-tail-kill/run.log), all eight cases
(direct_stream on/off, small/large, ended by a lower key or by Db::seal) fail
at the prefix assertion. Not run again on this tree; nothing in sync-run
touches close_run, so by reading it still fails the same way: the run's held
tail (records past the last whole 1 MiB piece, or the copy arms' buffer) is
written only by the seal thread's finish, while a later commit below the
greatest goes to the log at once. A kill between them keeps the later commit
and loses the run's.

Fix (P1, report.md): write what the run's writer holds on the writer's
thread before hand_seal, on every exit (leave_direct, Db::seal through
close_direct, hand_tail), with no sync. Cost: one write of under 1 MiB per
run close under EveryN, moved from the seal thread onto the writer, and the
last piece split into two writes (folio composition changes). Priced first
(pair.sh, buffered ingest arms, 10000 and 300000, --passes load and the read
passes). The power-cut sibling needs the shared log/run sequence (owner's).
Mutants for its tests: report.md, run-tail-kill.

Note for whoever takes it: sync-run's Db::sync now writes and syncs the open
run (Db::sync_run), so hand_tail's case under adaptive_shape is covered for
a sync; the close at a lower key and at Db::seal are not -- see the next
item, which is the same close seen from a `sync`.

## run-temp-dir-barrier (all three tests)

State: reproduced at 3df85d22 (stage0/run-temp-dir-barrier/run.log), all
three fail at the barrier assertion (no directory fsync between the run's
temp file's create and the first acknowledgement: under Always, under
EveryN(2) at the due commit, and at Db::sync). Not run on this tree; nothing
in sync-run adds a directory fsync, so by reading all three still fail.
Its third test (sync over an open run) now has the run's data synced by
Db::sync (sync-run), so only its barrier assertion is missing.

Fix (report.md): vault::sync_dir after PieceWriter::create in
commit_direct's open branch (one per run, on the commit path, priced first
at 10000 under both arms), or lazily at the first due sync -- and then also
in Db::sync_run (sync-run's), or mutant (e) survives.

## Found by sync-run, not built: the due commit of a run beside unsynced log frames

State: reproduced on this tree (sync-run/due-commit.log; the test is
sync-run/due-commit-test.diff, applied after sync-run's section of
tests/durable.rs). EveryN(5): three commits to the log (a hashed table,
values too large to inline), Db::seal (its seal held before its fsyncs, so no
manifest names it), then two commits of a run. The fifth commit is due and
syncs only the run's file (commit_direct's due branch), never the log, whose
frames of commits 1-3 stay unsynced: a power cut keeps commits 4-5 and loses
1-3 ("lost 150 of 150 keys of commits 1-3", log synced 0 of 42821 bytes). A
prefix violation of EveryN's contract (an unsynced tail is lost whole).
By reading only (no test): the same under Always after a reopen of a log a
buffered life wrote, whose replayed frames count unsynced and which Db::seal
under Always does not sync when nothing is staged.

Not built because it adds a syscall (the log's fdatasync) to a commit's
success path under EveryN when the log owes one (rule 1; and the brief:
"a commit under EveryN is untouched"). Smallest form to price: a bool set
in commit_direct's run-open branch (cold, once a run) when the log owes a
sync (frames_unsynced || loose_wals || wal_dir_unsynced), asked inside the
due branch beside the run's fdatasync (a load and a branch there), whose cold
path syncs the log before the run. In the clear nothing writes the log while
a run is open (leave_direct closes the run first), so the bool is exact for
the run's life; under a cipher log_run's due sync_wal already covers it.
Cost when it fires: one log fdatasync at the run's first due commit after a
hashed table was frozen unsynced; never under EveryN(u32::MAX).
Docs (sync-run round 2): SyncPolicy no longer promises it -- the first
paragraph names an open run as the exception to "a tail lost whole", the
replay paragraph says a policy sync of a run's file leaves the log as it
was, and the ordered-run paragraph names the window (the frames of tables
frozen before the run, or replayed by the open, durable only at a sync, a
flush or their seal's landing).

## Found by sync-run's review, not built: a sync over a run closed into a seal in flight

State: reproduced on this tree. The test is now
sync-run/round2/closed-run-test.rs (round 2 extended it; it supersedes
round1/closed-run-test.rs), appended after sync-run's section of
tests/durable.rs; its failing run is sync-run/round2/closed-before.log, all
24 cases fail: six closes (an append of a key below the run's greatest, a
delete above it, a put above it, an append above it of a 1000-byte value
that does not inline, Db::seal, and the run's own last commit crossing
seal_bytes) x two shapes x arms default and copyvals; the closing write's
own commit to the log survives in every case. Every way a run closes
leads here: leave_direct from append (any key goes_direct refuses: at or
below the greatest, empty, longer than u16::MAX, a value that does not
inline), from every put and from every delete; and close_direct from
Db::seal, which commit calls at the seal threshold (seal_while_idle never
seals an ordered live table). Round 1's description (below) named the
lower-key and Db::seal shapes only. Under EveryN(u32::MAX), a run of 300 keys (three commits of
a hundred) or 12000 (twelve of a thousand), closed by a key below its
greatest appended and committed or by Db::seal, the seal held by
hold_seal_landing (its finish, which writes the held tail, runs after the
hold); then db.sync() answers Ok. Killed (copy_store) after the Ok: 300 of
300, and 4000 of 12000 (the held tail), of the run's keys are lost. A power
cut (the run's file cut to its synced length before the close, the log to
its synced length): every run key lost. The lower key's commit, in the log,
survives every case: not a prefix. Arms default and copyvals.

Why: close_direct -> close_run takes the run (self.direct = None) and hands
its writer, held tail and all, to the seal job; Db::sync then sees no run,
syncs the log, zeroes `unsynced` and answers Ok without joining the seal.
Under a cipher the run's commits are log frames and a sync covers them
(held by a_sync_after_a_run_closed_into_a_seal_covers_its_logged_commits_under_a_cipher,
which passes; it is not evidence of a fix). A flush covers it (held by
a_flush_after_a_run_closed_into_a_seal_makes_the_runs_commits_durable,
passing, likewise not evidence).

Not built: the fix needs either a wait on another thread inside Db::sync
(rule 1: a sync's success path gets no new wait) or a write on the run's
close (run-tail-kill's P1, priced first). Smallest forms, both with a bool
set in close_run's cold path when `unsynced != 0` under EveryN (once a run),
asked in Db::sync beside its existing gates: (1) sync waits for that seal's
durable landing (land the queue to durable, as flush's drain does); (2) P1
writes the held tail on the writer at the close and the run's File is kept
(an Arc of the writer's handle, no syscall), and sync's cold path
fdatasyncs it -- one fdatasync only where owed, the allowance sync_run had,
but it needs P1's write first. Done when the round-2 test passes on every
run arm (it builds all six closes). ROADMAP.md: "A sync over a run closed into
a seal in flight". Docs restated (SyncPolicy, EveryN, Db::sync) so none
promises it. Mutant X6 (Db::seal's ordered branch zeroing `unsynced`) is
killed by a_runs_commits_count_toward_the_policys_sync_after_it_closes, not
by this test.
