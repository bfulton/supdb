# delete-max (defect delete-above-max): notes

## What was wrong
`Db::put` and `Db::append` raise `max_key`, the key a write must be above
to open a direct run, but `Db::delete` did not. So a key deleted above the
store's greatest could then be appended into a direct run while its
tombstone was still in the log past what the manifest covered (its seal in
flight). The live store read the append correctly. A crash before the
seal's durable landing did not: recovery rebuilt the run's temp file as a
piece and replayed the log, the tombstone included, as newer than every
segment, and the acknowledged append read as deleted at that open and
every one after. Under EveryN the result was a state the store was never
in (a batch recovered without its first key).

## What changed (src/db.rs)
- `Db::delete` raises `max_key` exactly as `put` does: after
  `leave_direct`, before the log frame, `if key > max_key { clear; extend }`.
  It is unconditional, so a delete that ends a run raises it too.
- Doc comments only: the `max_key` field, `Db::max_key_of` (it counts
  tombstones, the memtable's above all) and `Db::delete` (a deleted key
  counts toward the greatest, so ordered appends go through the log until
  they pass it).

Why this shape: it is the one-line form the report asked for. It keeps the
invariant "a run opens only above every key a log frame past the manifest
names" by the guard every other write already keeps. Cost on the success
path: one byte compare per delete, plus a copy of the key when it is a new
greatest. No syscall, wait, lock or allocation beyond the existing Vec
reuse (`extend_from_slice` into a cleared Vec may allocate only when the
key is longer than any key before it, which `put` does the same way).

## Every other place a tombstone enters or max_key is derived (read)
- `Txn::delete` / `Txn::commit`: applies through `Db::delete`, so it is
  covered; it has a test.
- No other batch delete exists on `Db` (`delete` is the only one;
  `Wal::delete` and `MemTable::delete` are called only through it and
  through `put`, which raises).
- `put`: already raises.
- `Db::max_key_of` at open: memtable entries include tombstones (a delete of
  an unseen key makes an entry, `MemTable::delete` -> `write(.., None)`), and
  it walks every entry; held by a test and two mutants. Segments: it takes
  each segment's last key in the key index. By reading, a tombstone record
  has a key there; this was not checked by a test. It does not matter for
  this defect: a durable segment is ordered below any run opened after the
  open.
- Nothing lowers or recomputes `max_key` at runtime (only the three raises
  and the open's `max_key_of` assign it).
- `leave_direct` moves an open run's uncommitted tail to the log; those keys
  were above the old greatest and raised it already.

## Encryption
- Kernel: identical to the plain store (the kernel encrypts the same files).
- Cipher: a run's commits go to the log as well (`RUN_LOGGED`, `log_run`),
  behind the tombstone, and its file is a memory file lost at a crash, so a
  recovery replays both in order. The defect has no analogue there by
  reading. `an_append_after_a_delete_survives_a_crash_under_a_cipher` holds
  that, in both windows; it passes on the unfixed engine, so it is a guard
  and not evidence of the fix. No cipher-only mechanism (vault::persist,
  memory files, pager) is involved.

## Tests (tests/durable.rs, created by this unit)
The file holds stage 0 as one binary, a section per defect. Shared helpers:
`dir`, `read_vec` (through `common::vf`), `copy_store` (a kill's state),
`wait_for`, `open_run`, `key`, `under_cipher`.

| test | before the fix | holds |
|---|---|---|
| an_append_after_a_delete_survives_a_crash_before_the_deletes_seal_writes | FAILS at durable.rs:212, "plain-BeforeWrite-Delete-Append, the first open: a write acknowledged after the key's delete reads back after the crash", left [] | the defect, seal held before it writes |
| an_append_after_a_delete_survives_a_crash_between_the_deletes_seal_phases | FAILS at :212, same, BetweenPhases | seal readable but not durable |
| an_append_batch_after_a_delete_survives_a_crash_before_the_deletes_seal_writes | FAILS at :212 | `append_batch` |
| an_append_after_a_transactions_delete_survives_a_crash_before_its_seal_writes | FAILS at :212 | `Txn::delete` |
| an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes | FAILS at :212 (run on the engine with the fix removed, i.e. 3df85d22's `delete`; appended to before.log) | a delete that ends an open run; the only killer of mutant (b) |
| a_buffered_crash_never_recovers_a_batch_without_its_first_key | FAILS at durable.rs:435, "a recovered batch holds its first key, appended after that key's delete", left [] | EveryN, kill |
| an_append_after_a_replayed_delete_survives_a_crash_before_its_seal_writes | passes (not evidence of the defect) | `max_key_of` counting replayed tombstones; kills mutants (d) and (g) |
| an_append_after_a_delete_second_in_its_batch_survives_a_crash_before_its_seal_writes | FAILS at durable.rs:232 on the engine with the raise removed (round 1, m-a2 log) | the delete behind a staged write; the only killer of a raise guarded by an empty batch |
| an_append_after_a_lone_delete_survives_a_crash_before_its_seal_writes | FAILS at durable.rs:232 likewise | a tombstone alone in a batch into an empty table |
| a_put_after_a_delete_survives_a_crash_before_the_deletes_seal_writes | passes (control, not counted) | put opens no run |
| an_append_after_a_delete_survives_a_crash_under_a_cipher | passes (guard, not counted) | the cipher arm |

Every crash test asserts its premises (the delete's batch replayed, the
later batch's last key recovered), a live-store control before and after the
seal lands, and the outcome at two successive opens.

### Problems the skeptic listed, and what was done
- The reproduction asserted `open_run` ("the window was reached"). That is
  the mechanism the fix removes: with it the fixed engine fails the test at
  the premise. It is dropped for appends; the window is shown instead by the
  before-fix failures and by the premise that the delete's batch replayed
  from the log (it is past the manifest). The put control keeps its
  `!open_run` assertion. Mutant (c) shows the tests still see a run opening
  wrongly.
- The header's "a power cut leaves the same" is gone: the copy is described
  as a kill's state. The run's temp file has no directory fsync; that is
  run-temp-dir-barrier, a later unit.
- The reopen-then-seal test the skeptic asked for (mutant d) is added.
- The buffered test's batch arithmetic (1,000 then 9,500 hundred-byte
  values) is kept; after the fix the appends go through the log and the
  premise (key 500999 recovered) holds from the log's writes. Its doc says
  both paths.
- The two seal windows are separate tests (and the cipher test loops over
  both, since it is a guard).

## Docs
- CLAUDE.md: the ordered-ingest paragraph of "Crash discipline" now defines
  the store's greatest as every key a segment holds or a log frame names, a
  delete's included, and why. The entry "A sibling function without its
  sibling's guard" is amended with this as the shape come back (a quantity
  that guards an order between two logs, kept by every write that reaches
  either), ending in the rule.
- ROADMAP.md: "One sequence for the log and the runs" under Engine: stores
  written before this fix may hold the pattern on disk and open with the
  run's values masked; the shared sequence repairs them.
- README.md: its line "keys arriving above the store's greatest" is left as
  it is; it is a summary and "greatest" there can be read either way. Not
  changed.
- Crate docs on SyncPolicy, commit, sync, flush, open: nothing in them
  changed truth; untouched.

## Runs
- before.log: 5 fail, 3 pass (the controls/guard/replayed); then the
  ends-a-run test on the engine with the fix taken out (mutant a's run).
- after-durable.log: 9 of 9 at first (8; the ends-a-run test was added
  after), after.log: `cargo test --profile checked --no-fail-fast`, every
  binary `test result: ok`, exit 0 (lib 404, db 226, cipher 24, fuzz 5,
  segwriter 19, zpages 136, durable 9, ...).
- clippy.log: `cargo clippy --profile checked --lib --test durable -- -D
  warnings`, exit 0. `sh scripts/fmt.sh --check`, exit 0.

## Not verified
- The doc comment on `Db::delete` was edited while the full suite was
  compiling; the suite's binaries may predate that one comment. No code
  changed after keep/db.rs other than that doc comment.
- The release-profile `cargo clippy --all-targets` the lint group runs was
  not run (checked-profile clippy over the lib and the new test only, to
  spare the disk); the wasm and bench groups were not run.
- No pairing or timing was run (rule 10); the cost claim (one compare per
  delete) is by reading. A delete-heavy ordered load would now send appends
  between the old greatest and a deleted key through the log; the suite has
  no such workload, not checked by a pairing.
- That a segment's tombstone-only last key is counted by `max_key_of` is by
  reading only.
- Stores already on disk holding the pattern are not repaired (ROADMAP).

## Repair round 1
Reviewers' extra mutants, all run (mutants.md, round 1):
- A raise under a partial guard: `!mem().is_empty()` was killed by 7
  tests; `wal.pending.is_empty()` survived every test there was, since each
  deleted the key first in its batch. Added
  `an_append_after_a_delete_second_in_its_batch_...` (kills it alone) and
  `an_append_after_a_lone_delete_...` (a lone tombstone, whose batch has
  nothing else to show it replayed, so `holds_the_write` takes the delete's
  kind and checks key(10) at its loaded value there). Both fail before the
  fix at the final assertion (before.log, round 1 section).
- `max_key_of`'s segment loop removed, and its compare flipped: both survive
  durable.rs and are killed by tests/db.rs
  (a_crash_before_the_manifest_lands_keeps_the_pre_merge_store: a run
  opened below a reopened store's greatest is named against the last
  partition's fence and the merge refuses it). Held, so no durable test.
- append_pinned's direct-branch raise deleted: survives durable.rs, killed
  by about thirty tests of tests/db.rs (one hung; the binary was stopped by
  hand at 37 minutes).
- The raise moved after `wal.delete`, after `mem().delete`, and `>=`:
  equivalent, survive; nothing between the positions reads `max_key`.
CLAUDE.md: the entry now lists the batch positions tested; the ordered-ingest
paragraph's hand-wrapped lines were reflowed (no wording change).
Runs this round: after-r1.log, cargo test --profile checked --no-fail-fast;
scripts/fmt.sh --check exit 0. db.rs did not change this round.
An accident this round: a shell heredoc left unquoted ran its backticked
text, which started a second `cargo test --profile checked --no-fail-fast`
beside after-r1's, without this unit's TMPDIR, for about two minutes before
it was stopped. Its stores, if any were left, are under the system temp
directory by its test binaries' pids; they were not removed (rule 10 forbids
a glob over /tmp and the pids were not recorded). after-r1's db binary ran
beside it for that time.
after-r1.log: every binary ok but leak (1) and zpages (3: two form-file
advice tests and a_retired_form_file_outlives_its_last_reader_and_no_longer).
Cause: the run's TMPDIR was spelled with `..` ($S/../../tmp/delete-max), and
those tests match /proc/self mapping paths against the temp path as given;
the kernel prints it canonical. leak-dotdot.log reproduces the leak failure
with the `..` spelling alone; after-r1-rerun.log, the same binaries under the
canonical spelling of the same directory: leak 1/1, zpages 136/136, durable
11/11, exit 0. The engine did not change between the two. (The mutant runs
used the `..` spelling too, for durable and db only, neither of which reads
mapping paths; both are green in after-r1.) That those tests need a canonical
TMPDIR is a fragility of theirs, not touched here.
clippy-r1.log: `cargo clippy --profile checked --lib --test durable -- -D
warnings`, exit 0.

## Repair round 2
src/db.rs did not change this round. Tests added to tests/durable.rs:
- `an_append_after_a_delete_its_seal_commits_survives_a_crash_before_the_seal_writes`
  (`Del::SealCommits`: the delete staged, committed by the held seal with no
  `commit`). Fails before the fix at durable.rs:238, the final assertion
  (before.log, round 2 section); kills mutant (u), the raise moved into
  `Db::commit`.
- `a_delete_below_the_greatest_leaves_it_where_it_was` and
  `an_open_keeps_the_greatest_its_segments_hold`, over a helper that loads
  keys 0..100 into several partitions (seal_bytes 4 KiB, l0_trigger 2,
  partition_bytes None), changes the greatest (a delete of key(5) flushed;
  a reopen), appends keys 6..100, and reads every key against a model live,
  after a kill, flushed and settled, and at a reopen of each. Pass on the
  unfixed engine (guards, not evidence); kill (q), (i) and (m) in
  durable.rs.
Mutants: mutants.md, round 2. Killed: q, r, s, u, i, m. Survivors, each with
the reason it cannot be told apart: t (cipher, the run is logged), v (the
guard is true wherever close_direct succeeds), w (the segments' first keys
still bound the greatest at the last partition's fence; probe read every key
back), x (as h).
The reviewers' proposed killer for (u), a delete and an append of the same
key in one batch, cannot open a run at all: `direct_can_open` requires
`self.wal.pending.is_empty()`. Not added.
CLAUDE.md: the entry names the seal-commits case, the lowering (q) and the
open (i), and says the raise is made where the write is made, not at the call
that makes it durable.
Runs: after-r2-durable.log (14/14), after-r2.log (the whole checked suite,
--no-fail-fast, canonical TMPDIR), clippy-r2.log, fmt-r2.log (exit 0).

## Repair round 3
src/db.rs did not change this round. The reviewers' seven extra mutants
were run (mutants.md, round 3). Killed: y3, y4, y5b (y5 died at a debug
assertion about pinning rather than an outcome, so it was rerun inside an
operation), y7, and y2 once a test reached it. Equivalent survivors: y1
(no run opens with `direct_ingest` off, and an open recomputes the
greatest) and y6 (an equal key copies the same bytes).
- y2, `leave_direct` not re-appending the run's uncommitted tail to the
  log: it survived because the ends-a-run test committed the load before
  its delete, so the run had no tail. The test now leaves ten keys
  uncommitted in the run before the delete, and reads them live and at
  two opens of the crash copy; y2 then fails it at the tail's first
  recovered read. That assertion is a guard for `leave_direct`, outside
  this unit's change; the test's main assertion still fails on the
  engine with the raise removed (a4, before.log round 3).
CLAUDE.md, ROADMAP.md: unchanged this round (the entry already names the
delete that ends a run).
Runs: after-r3-durable.log (14/14, exit 0), after-r3.log (the whole checked
suite, --no-fail-fast, canonical TMPDIR), clippy-r3.log, fmt-r3.log.
