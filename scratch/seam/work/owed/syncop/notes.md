# syncop: syncs in flight together, completed in any order, or failed

Item owed by `work/fix/conclusion.md` (§1, §4, §6): "a model op that completes the newest sync
in flight", and the rule "synced = the newest sync, by issue, among those completed".
Nothing under /home/user/supdb touched; nothing else under `scratchpad/seam` written. No cargo,
compiler or bench. Every model run: `nice -n 10 timeout 600`, one process at a time, through
`batch.sh` (or by hand, the same way), each also bounded inside by `secs=550` (none hit it).

## What I read

- `decisions.md`, `work/fix/conclusion.md` (whole), `design.md` §4 lines 195-290 (sequencer at
  :233, pins at :245-251), `work/e4-model/notes.md`, `logrun.py`, `pfm.py` (device, LogH, sync),
  `e4.py`; `work/fix/model/README.txt`, `notes.md`, `stream.py`, `cand.py` (device, Orc, judge,
  Tally); `work/fix/skeptic/ooo.py` (the JRP out-of-order rerun).
- The engine, `plan/tree/src/db.rs` (682b2c09), by reading only: `Wal` 4201-4260, `Wal::sync`
  4686-4690, `frames_unsynced` 4793; `AlignedWriter::landed` 6274-6312, `synced_all` 6320-6332,
  `lose`/`answer_held` 6370-6411; `WriterHelper` 6708-6900; the run's `sync_with` 7591-7622;
  `SlotChoice` 12295-12380; `SyncHelper` 38505-38560; `land_readable` 39622-39735,
  `land_durable` 39781-39830, `publish_unsynced`/`sync_owed_slot`/`write_slot`/`sync_slot`
  40790-40880; the direct close's presync 30340-30510.

## One correction to the brief's premise (by reading E4's code, then confirmed by run)

`conclusion.md` says E4's driver "can only complete the oldest sync in flight". Its `'S'` op is
`sync_begin` + `sync_end` at once, beside any `'B'` still in flight, so `B 1 S 1 E` *is* a newer
sync completing first and an older one after it, under `pfm.LogH.sync_end`'s assignment rule.
E4 ran that alphabet (`base`, `mix`) at maxlen 5 and found nothing because the loss needs 7 ops
at R=5 and 9 at R=8 (below). What E4 could not reach was the length, not the order. Its `'S'`
also does not wait for the sequencer, so E4 modelled the design without the wait throughout.

## The model changes

Copied: `work/e4-model/{pfm,logrun,e4}.py` to `log/`, `work/fix/model/{cand,stream}.py` to
`stream/`. Every change is marked `SYNCOP`.

Device (both `pfm.Dev` and `cand.Dev`): `sync_fail(tok)`. A failed fdatasync begun at tb leaves
every object dirty at tb holding its durable image, any image written to it up to tb, or TORN,
and moves those images to `limbo`: Linux marks the pages clean after a failed writeback, so no
later sync makes them durable; an object written again after the failure and then synced is
durable again (limbo cleared). `pc()` shows the latest image; `any_views` includes limbo.
Reason: without this, "fail one" is only a missing acknowledgement and no protocol error about
failures could show as a LOSS.

Handle (`LogH` and `StreamH`), options:
- `wait` (the sequencer, design.md:233): a `'B'` that finds a sync in flight or queued is queued,
  and is issued when the one in flight completes, capturing what it covers then; the caller's
  `'S'` completes every sync ahead of it in order and then issues its own. Off: `'B'` and `'S'`
  issue at once beside whatever is in flight.
- `sync_rule`: `assign` (synced := what the sync that just completed covered; E4's `pfm` and the
  JRP §5.4 shape) or `max` (synced := the newest by issue among those completed; a completion
  of an older sync than the one `synced` came from changes nothing but the acknowledgement).
- `keep_err` (default on): the first failed sync is kept; no completion after it acknowledges or
  moves `synced`. Mutant `keep_err=False`: the next success acknowledges.
- `err_later_only` (MUTANT): the kept error answers only syncs issued after the failure; a sync
  already in flight at the failure acknowledges on its own success. This is the Linux shape
  where two fdatasyncs on one open file description race and the one that checks second gets
  `Ok` (the engine reading below).
- Stream only: `early` (default on): `sync()` answers at once when `synced` covers everything
  written, as the engine's sync asks the log what it owes; a fresh open counts nothing
  replayed as synced (`synced = -1`), as the engine's open marks do. Mutant
  `cover_at_completion`: a sync covers what is written when it completes, not at issue.
The oracle is unchanged: `Orc.acked` is already a max over the acknowledged history indexes
(what each sync captured at its issue), so the oracle is the caller's truth and only the
handle's own `synced` is under test.

Driver ops added (`logrun.py`, `stream.py`): `'N'` completes the newest in flight, `'F'`/`'G'`
fail the oldest/newest in flight; `'E'` completes the oldest (as before); `'B'` goes through the
sequencer. Families: Log `log/syncop.py` `ooo` (writes 1,5, S, B, E, N; at least one overlap: two
`B` outstanding or `S` beside a `B`; at most 2 outstanding; `N` only with 2 outstanding),
`fail` (adds F, G; at least one failure), `tail` (a fixed shape long enough to lap the ring:
pre, B, writes, S or B..N, writes, E, then up to `maxlen` writes). Stream `ooo`, `ooox` (adds X
and P, at most one each, nothing in flight at a crash), `sfail` (adds F, G). The Stream class is
run with `chain=1`, the shape decision 2 takes.

Regression check of the copy, before any new op was used (`log/`):
`python3 e4.py inflight maxlen=8 R=8` -> 1,312,998 any, 0 violations;
`... pin_inflight=False` -> 206,729 of 1,765,712. Both are E4's published numbers.

## Runs: Log class (`log/`, results in `results/log_batch.txt`, `log_batch2.txt`)

| command (`python3 syncop.py ...`) | sequences | states any / proc | violations |
|---|---|---|---|
| `tail maxlen=4 R=5 sync_rule=assign` (no wait) | 810 (470 blocked) | 68,748 / 6,884 | **17,294 LOSS**, first `B 1 S E 1 1 1` |
| `tail maxlen=4 R=5 sync_rule=max` | 810 (437 blocked) | 76,780 / 6,986 | 0 |
| `tail maxlen=4 R=8 sync_rule=assign` | 810 (0 blocked) | 790,272 / 8,857 | **308,176 LOSS**, first `B 1 S 1 E 1 1 1 1` |
| `tail maxlen=4 R=8 sync_rule=max` | 810 | 915,408 / 8,857 | 0 |
| `tail maxlen=4 R=5 wait=True sync_rule=assign` / `=max` | 810 (371 blocked) | 92,691 / 7,385 each | 0 / 0 |
| `tail maxlen=4 R=8 wait=True sync_rule=assign` / `=max` | 810 | 673,482 / 8,857 each | 0 / 0 |
| `ooo maxlen=7 R=5 sync_rule=assign` | 29,872 (2,288 blocked) | 16,957,042 / 254,517 | **17,088 LOSS**, first `B 1 S 1 1 E 1` |
| `ooo maxlen=7 R=5 sync_rule=max` | 29,872 (2,288 blocked) | 16,928,818 / 254,517 | 0 |
| `ooo maxlen=7 R=5 wait=True sync_rule=assign` | 29,872 (1,648 blocked) | 18,541,642 / 255,469 | 0 |
| `ooo maxlen=6 R=5 wait=True sync_rule=max` | 5,595 | 1,598,185 / 40,078 | 0 |
| `ooo maxlen=6 R=5 sync_rule=assign` (no wait) | 5,595 | 1,472,067 / 40,030 | 0 (too short: the loss needs 7 ops at R=5) |
| `fail maxlen=6 R=5 sync_rule=max` (no wait) | 5,893 (16 blocked) | 2,054,675 / 40,834 | 0 |
| `fail maxlen=6 R=5 wait=True sync_rule=assign` | 5,893 | 2,057,623 / 40,834 | 0 |
| MUTANT `fail ... sync_rule=max keep_err=False` (no wait) | 5,893 | as above | **29,284 LOSS**, first `1 B F S` |
| MUTANT `fail ... wait=True sync_rule=assign keep_err=False` | 5,893 | as above | **31,438 LOSS**, first `1 B F S` |
| MUTANT `fail ... sync_rule=max err_later_only=True` (no wait) | 5,893 | as above | **1,954 LOSS**, first `1 B B G E` |
| MUTANT `fail ... wait=True sync_rule=max err_later_only=True` | 5,893 | 2,057,623 / 40,834 | 0: unreachable under the wait |
| `tail maxlen=4 R=8 wait=True/False sync_rule=max pin_inflight=False` | 810 | 673,482 / 975,456 | 0 / 0: this family does not test the in-flight pin; E4's `inflight` family does (206,729 above) |

How the assign rule loses (`B 1 S 1 E 1 1 1 1`, R=8): B is issued over the empty file; one byte;
S issues and completes (synced = its span, acknowledging the byte); one byte; E completes B, and
the assignment moves `synced` back to B's span. The ring now pins B's span and the head span,
and not S's base CKPT and frame; the writes after it (a CKPT every K=2 frames) take those slots;
a power loss that keeps none of the unsynced frames leaves only B's span, the empty file. Under
`max`, E acknowledges nothing new and leaves `synced` at S's span.

"Blocked" is E4's: a write that finds the ring full of pinned frames raises and the run is
dropped (design.md:251's "the write waits for the sync in flight" is not modelled). At R=8 no
`tail` run blocked; the R=5 counts above drop 5-8% of `ooo`'s sequences that way.

## Runs: Stream class (`stream/`, results in `results/stream_batch.txt`)

A first batch passed `wait=False` to `stream.py`, whose argument parser turned it into the truthy
string `'False'`, so every "no wait" Stream run had the sequencer on; and `stream.py fail` is the
old failed-write family, not the new one. Both outputs are kept as
`results/*_INVALID_waitparse.txt` and none of their numbers is used. The parser now reads
`True`/`False`, the new family is `sfail`, and the batch was rerun:

| command (`python3 stream.py ...`, all `chain=1`) | sequences | states any / proc | violations |
|---|---|---|---|
| `ooo maxlen=7 wait=True sync_rule=assign` | 29,872 | 695,636 / 51,035 | 0 |
| `ooo maxlen=7 wait=False sync_rule=max` | 29,872 | 695,861 / 51,035 | 0 |
| `ooo maxlen=7 wait=False sync_rule=assign` | 29,872 | 695,861 / 51,035 | **0** |
| `ooox maxlen=7 wait=True sync_rule=assign` | 62,342 | 1,412,132 / 235,244 | 0 |
| `ooox maxlen=7 wait=False sync_rule=max` / `=assign` | 62,342 | 1,416,726 / 235,676 each | 0 / **0** |
| `sfail maxlen=6 wait=True sync_rule=assign` | 5,893 | 106,671 / 9,245 | 0 |
| `sfail maxlen=6 wait=False sync_rule=max` / `=assign` | 5,893 | 109,688 / 9,245 each | 0 / 0 |
| MUTANT `ooo maxlen=7 wait=False sync_rule=max cover_at_completion=1` | 29,872 | 706,345 | **107,218 LOSS**, first `B 1 B E` |
| MUTANT `ooo maxlen=7 wait=True sync_rule=max cover_at_completion=1` | 29,872 | 852,414 | **247,998 LOSS**, first `B 1 S` |
| MUTANT `sfail maxlen=6 wait=False sync_rule=max keep_err=False` | 5,893 | 109,688 | **4,214 LOSS**, first `1 B F S` |
| MUTANT `sfail maxlen=6 wait=True sync_rule=max keep_err=False` | 5,893 | 106,671 | **3,820 LOSS**, first `1 B F S` |
| MUTANT `sfail maxlen=6 wait=False sync_rule=max err_later_only=True` | 5,893 | 109,688 | **352 LOSS**, first `1 B B G E` |
| MUTANT `sfail maxlen=6 wait=True sync_rule=max err_later_only=True` | 5,893 | 106,671 | 0: unreachable under the wait |

**The brief's expectation does not hold for Stream: the naive rule is not caught, and cannot
be.** Assign and max give identical state counts and 0 violations with and without the wait. The
Stream class keeps nothing that `synced` protects: records are only appended, nothing is
written over while the file is open, and the one consumer of `synced` is the "nothing owed"
answer, which a `synced` moved *back* by an older completion makes stricter (it syncs again),
never looser. The model is not blind to the mark: a mark that moves *forward* past what a sync
covered (`cover_at_completion`) is caught at once, with and without the wait.
What the Stream class does need from out-of-order completion is the failure rule: a failure
must answer every sync that was in flight when it happened, not only the later ones
(`err_later_only`, 352 LOSS); the wait makes that case unreachable, because a sync in flight
alongside the failing one cannot exist.

## What each class must keep, and where it is asserted (the design is `design.md` here)

See `design.md` in this directory. In one line each:
- **Log (S2):** keep the wait; keep the max rule as well, which costs one compare; a debug
  assertion that completions arrive in issue order under the sequencer; a test that holds a
  sync in flight, issues a second one, and checks the second waits; a test with the sequencer
  switched off by a seam, checking `synced` never moves back.
- **Stream (S6):** the max rule is not load-bearing; the wait is, for failures; the coverage
  captured at issue and the kept error are load-bearing in every shape. Assert the capture at
  issue, and test the kept error with two syncs in flight.

## The engine as it stands (read, not run)

Three places where one file is fdatasynced by more than one thread.

**1. A segment's writer and its helper (`WriterHelper`, db.rs:6708-6900; `synced_all`
6320-6332; the run's `sync_with` 7591-7622).** Both sync one open file description: the helper
gets `file.try_clone()` (6775), a dup. "Synced" is two marks:
- the helper's `shared.synced`: `fetch_max` by the helper (6858) and by the writer (6327-6329).
  That is the max rule. It only paces the helper (`landed - synced >= ahead`, 6836), and no
  acknowledgement rests on it;
- the writer's own `self.synced = self.written` (6330), set after its own fdatasync. The writer's
  syncs all run on its own thread, so they are serialized and the assignment is its own capture.

An acknowledgement (a run's commit, `sync`, `flush`) rests on the writer's fdatasync and on
`shared.error` being 0 when `synced_all` loads it (6322-6325). The helper stores the error with
a `compare_exchange` *after* its `sync_data` returns (6838-6852). Linux reports a writeback error
once per open file description: whichever fdatasync checks second returns `Ok` over the pages
the failed writeback marked clean.

So the following order is possible:
- the helper's fdatasync meets the error and consumes it;
- the writer's fdatasync, in flight beside it or issued just after, returns `Ok`;
- `synced_all` loads `error == 0` before the helper's store lands;
- the commit is acknowledged.

That is the model's `err_later_only` mutant: caught in Log (1,954) and in Stream (352) without a
wait, and unreachable with one. **This is a hazard by reading: an acknowledgement that rests on
the older, overlapping sync's outcome before that outcome is published.** The window is from
the helper's return from `sync_data` to its store: a few instructions, or a descheduling of the
niced helper thread.

The later failure is still caught: the helper's error is stored, and the next `synced_all` or
the finish (`join_helper`, which joins the thread first, so there is no race there) answers it.
So one acknowledgement at most is wrong per failure, and it is the one a power cut then loses.

The candidate fix needs a wait on the success path, so it is priced before it lands (decision
10); it is not priced here:
- the helper bumps `begun` before `sync_data` and publishes `ended` after storing the error;
- `synced_all` reads `begun` after its own fdatasync returns, waits until `ended` reaches it,
  then loads `error`.

The cost is two loads when no helper sync overlaps, and a wait only when one does. A mutex
around both syncs (a sequencer) is the other shape.

**2. The pending manifest's helper (`SyncHelper`, db.rs:38505-38560; `land_readable`
39646-39720).** "Synced" is `SlotChoice::durable`, set by *assignment* in `landed(slot)`
(12358-12360), at three call sites:
- after `helper.wait()` (39701-39702);
- in `sync_owed_slot` (40815-40828);
- in `publish` after its own sync (40953).

All three run on the segment work's thread. The helper's sync is started and then waited for
before any other slot is written or synced. `write_slot` gives up an owed slot before it writes
(40850-40860) rather than syncing it beside the new write. So at most one slot sync is in flight
and completions are in issue order. **The assignment is sound only through that wait.** If two
slot syncs could complete out of order, assignment would mark the older slot "durable", and the
next publish would write over the newest durable one: the two-torn-slots case CLAUDE.md's "A slot
chosen by its sequence" names. No acknowledgement rests on an older sync here. What I have not
checked: whether each slot write opens a fresh descriptor (`manifest_write_slot_unsynced` returns
a `File`; its body was not read). A recommended assertion is a debug check that the manifest
sequence named by `landed` never goes back.

**3. The writer's presync beside the seal thread's fsync (db.rs:30352-30508).**
- The default arm: the presync runs on the writer's thread after `hand_seal` (30491-30505) on
  `w.file_clone()` (30353-30355), the same description. The seal thread's `seg.sync_all()`
  (30425) runs in `after`. The two can overlap and complete in either order.
- The `close_finish_inline` arm: the presync thread is joined (30421-30424) before
  `seg.sync_all()`, so the two are serialized.

In both arms "synced" for the segment is neither sync alone. The job lands durable only at the
writer's own ask (`SealJob::awaits_writer`), which follows the presync's result. A failed
presync wedges the queue before that ask (30502-30503). A failed seal fsync fails the job. Since
whichever sync checks second may get `Ok` over the other's failure, both answers are needed, and
the order is right: the writer learns its own presync's answer before it gives the word, and the
job's answer comes with its return.

The run's helper (pair 1) may still be syncing during the presync. Its failure reaches the
segment through the finish, which joins it before reading the error. **No acknowledgement rests
on one of the two alone.** Read only.

**The Wal (db.rs:4686-4690)** is synced only through `&mut self` on the writer's thread. Its
`synced = written` is the capture of a sync no other thread overlaps. Its consumers
(`frames_unsynced`, `drop_synced`, `lose_unsynced`) are of the Stream kind: a mark moved back
would make them stricter.

## Not verified

- **The engine findings are read, not run.** Pair 1's window is not reproduced: no test and no
  injected interleaving were built. It depends on Linux reporting a writeback error to one
  fdatasync per open file description. That is CLAUDE.md's account (db.rs:6334-6338, 6729-6731),
  not checked on this kernel here. The fix sketch is not priced.
- I did not read `manifest_write_slot_unsynced`, `publish`'s sync line by line, or
  `SealJob::awaits_writer`'s implementation. They are cited from the comments around them
  (39587-39600, 30480-30490).
- **The Log model:**
  - only the families above;
  - R=5 and R=8, K=2, G=2, PB=4;
  - at most 2 syncs outstanding, so 3 or more in flight was not explored;
  - no `X`/`P` mixed with out-of-order completion: `syncop.py` has no crash ops, and resume was
    not re-run with `N`/`F`/`G`;
  - no `finish`, `TRUNC` or patches with the new ops;
  - runs that block on a full ring are dropped (5-8% at R=5, 0 at R=8 in `tail`).
- **The Stream model:**
  - `chain=1` only;
  - SU=3;
  - maxlen 7 (`ooo`, `ooox`) and 6 (`sfail`);
  - the Journal format was not run.
- **The failure model:**
  - one failure: later syncs of a lost handle are not acknowledged, so a second failure adds
    nothing;
  - a failed sync's pages are all treated as possibly lost, with no partial writeback order;
  - a failure that reports while the writeback of some pages succeeded is covered only as a
    subset of views.
- **The sequencer is modelled with its queue's capture taken at issue,** when the wait ends.
  Whether S2's sequencer captures at request or at issue is a design choice that was not
  modelled both ways.
- JRP and ADJ were not re-run under the new driver; the skeptic's `ooo.py` covers JRP.
