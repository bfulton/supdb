# syncop: the skeptic's check

My copies are under `check/`. That holds `log/` and `stream/`, which are the work's copies with my
ops and mutants marked `CHECK`, and `e4orig/`, which is E4's original `pfm.py`/`logrun.py` plus a
hand runner. Every run was `nice -n 10 timeout 600`, one at a time.

## Reran

| run | result | work's figure |
|---|---|---|
| `log/e4.py inflight maxlen=8 R=8` (on my patched copy) | 1,312,998 any, 0 | same |
| `log/syncop.py tail maxlen=4 R=8 sync_rule=assign` | 308,176 LOSS, first `B 1 S 1 E 1 1 1 1` | same |
| `log/syncop.py ooo maxlen=6 R=5 sync_rule=max` | 1,472,067 any, 0 | (work ran len 7) |
| `e4orig/hand.py 8 B_1_S_1_E_1_1_1_1` (E4's own code, unmodified) | 128 LOSS | confirms the work's correction: E4 could express the order; it lacked only the length |
| `e4orig/hand.py 5 B_1_S_1_1_E_1` | 20 LOSS | same |

Not rerun:
- the work's ooo maxlen=7 runs for assign, max and wait (about 280 s each);
- the Log fail batch;
- the Stream ooo/ooox batch.

I read their outputs in `results/` and ran neighbouring configurations instead.

## My mutants (each must be caught)

| mutant | where | result |
|---|---|---|
| M1: `sync_rule=min` (the max compare reversed) | Log, tail R=8 | **358,726 LOSS**, caught |
| M2: `pin_inflight=False`, no wait, max | Log, ooo maxlen=7 R=5 (348 s) | **91,008 LOSS**, first `1 B 1 1 1 B E`, caught. The ooo len-7 family does reach the in-flight pin. At maxlen 6 it does not: 0 of 1,920,067. |
| M3: oracle mutant `orc_assign=True` (`Orc.ack` assigned, not maxed) with `sync_rule=assign` | Log, tail R=8 | **0 violations**: the loss is NOT seen. The check catches the naive rule only because the oracle itself keeps the max. Keep `Orc.acked`'s max, and add a comment there, or the check goes blind exactly where the protocol is wrong. |
| M4: Stream `early_ignores_err` (the early "nothing owed" answer ignores the kept error) | Stream, sfail maxlen=6, with and without the wait | **0, NOT caught** |

On M4: after any failure that matters, `synced < at`, so the guard is unreachable in this model.
The engine rule "a kept error answers every later sync" is therefore held by no loss here, and
S6's test of it has to assert the answer, not a loss.

## The hand state that breaks a claimed property: errseq's one report on a shared description

The work's failure model has two behaviours:
- a failed sync's error is published before any Ok completion (`F`/`G`, then `err_later_only`);
- a successful device completion makes the pages durable.

It cannot express the case that matters on one open file description. Two fdatasyncs wait on the
same failed writeback; the error is reported to one of them; the other returns `Ok` first, while
the failure is still unpublished.

I added ops for this:
- `Y`: the newest in flight returns Ok over a failed writeback, and the other reports the error later;
- `V`: the same with the oldest.

Results:
- `check/log/syncop.py steal maxlen=6 R=5 sync_rule=max` (keep_err on, so K' on): **99,360 LOSS**, first `1 B B Y`.
- The same with `wait=True`: 0 (unreachable).
- Log hand runs `1_B_B_V`, `1_B_B_V_1_S`, `B_1_B_V_E`, `1_B_S_V`: 33 LOSS.
- `check/stream/stream.py steal maxlen=6 chain=1 wait=False sync_rule=max`: **14,244 LOSS**, first `1 B B Y`.
- The same with `wait=True`: 0.

Consequences:
1. **"Log without the wait is sound under the max rule" holds only for successful syncs, or for
   syncs on separate open file descriptions.** With failures on one description, M plus K plus K'
   loses data. M alone does not turn "a future change that drops W" into a correct one.
2. **Stream: "keep W, or state K' explicitly" is wrong.** K' cannot answer a sync that has already
   returned Ok. The only alternatives to W are:
   - each concurrent sync on its own description;
   - an Ok that is not believed until every overlapping sync has published its outcome, which is
     itself a wait.

   So W, or an equivalent wait at completion, is load-bearing for failures in both classes.
3. **Engine pair 1 is the `Y`/`V` shape, not `err_later_only`.**
   - `err_later_only` means the error is published first and the Ok is answered anyway. The
     engine's `synced_all` catches that: it loads `error` after its own fdatasync (db.rs:6320-6332).
   - The window the work describes is the steal: the helper's `sync_data` consumes the EIO and
     its `compare_exchange` is not yet done (6838-6852) when the writer's fdatasync returns 0 and
     `synced_all` loads 0.
   - The hazard itself is confirmed by reading.
   - The work's begun/ended fix sketch closes it in both orders (read, not run). A helper sync
     that begins after the writer's fdatasync returned cannot steal an error the writer already
     checked.

## Engine pairs, read (not run)

- **Pair 1.** As above: a real window, one acknowledgement per failure. Not reproduced.
- **Pair 2** (the pending manifest's `SyncHelper`).
  - Each slot write opens a fresh descriptor (`vault::open_slot`, db.rs:12527). This is the item
    the work left unread. So slot syncs never share a description, and errseq stealing does not
    apply.
  - `SlotChoice::landed` assigns (12357-12359). It is sound because `start` (39669) and `wait`
    (39701) bracket the helper job on the segment work's thread, and nothing between them writes
    or syncs a slot.
  - The error path: a `?` between `start` and `wait` (`Seg::open_landed`, `rank_before_publish`)
    leaves the helper's result unread in its channel. A later `wait` would read that stale result
    and land the wrong slot.
  - This is safe only because a failed `land_readable` stops the seal queue (39129-39160), so the
    pending path is not reached again in that open.
  - Recommend that the monotone assert the work proposes also checks that every `wait` matches
    its `start`, for example by a job id echoed back.
- **Pair 3.** I agree it is sound. Either sync may get the stolen Ok, but both answers are needed
  before the job lands durable (the presync's error wedges the queue, the seal's error fails the
  job). The run's helper is joined before its error is read at the finish.
- **Wal.** One thread, by `&mut self` (read at 4686-4690 only). I did not check every descriptor
  the log shares.

## Not checked

- Three or more syncs in flight (`inf=3`).
- The steal ops combined with X/P, or with partial writeback.
- Whether Linux's errseq reports a writeback error once per file description: taken from
  CLAUDE.md and the code's comments, not tested on this kernel.
- The Stream model only checks crashes with nothing in flight (`ooox` requires `k == 0` at X/P).
  A power cut with two syncs in flight is untested there.
- The Once class's REC slot choice ("the slot that is not the last completed sync's", design.md
  Sync, Once). It is a known-durable choice like the manifest slots, so M is load-bearing there
  without W. It was not modelled by the work or by me.
- JRP and ADJ under the new ops.
