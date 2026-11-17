# syncop: what each class keeps of the sync rules, and the test each stage writes

The evidence for everything here is in `notes.md`, with the commands to rerun it.

## The rules, separated

- **W, the wait.** A sync that finds one in flight waits for it, then issues its own
  (design.md:233).
- **M, the max rule.** `synced` is the newest sync, by issue, among those completed. A
  completion older than the one `synced` came from acknowledges what it captured and moves
  nothing else.
- **C, capture at issue.** A sync covers what was written when it was issued, never what is
  written when it completes.
- **K, the kept error.** The first failed sync answers every later completion until reopen, and
  no completion after it acknowledges or moves `synced`.
- **K', K's reach.** The kept error also answers syncs that were already in flight when the
  failure happened.

## Model results

| rule removed | Log, no wait | Log, wait | Stream, no wait | Stream, wait |
|---|---|---|---|---|
| M (assign instead) | LOSS: 17,088 (ooo R=5 len 7), 308,176 (tail R=8) | 0 (assign = max, identical counts) | 0: nothing `synced` protects | 0 |
| C | not run on Log | not run on Log | LOSS 107,218 | LOSS 247,998 |
| K | LOSS 29,284 | LOSS 31,438 | LOSS 4,214 | LOSS 3,820 |
| K' | LOSS 1,954 | unreachable (0) | LOSS 352 | unreachable (0) |

**M matters only where `synced` decides what may be written over:**
- the Log ring's pin of the last completed sync's span;
- the manifest's slot choice;
- JRP's P1.

**An append-only class has no such decision,** so a `synced` moved back only makes it sync again.
**W makes both M and K' unreachable,** because completions then come in issue order and no sync
is in flight beside a failing one.

## Log class (stage S2, `pagefile.rs`)

**Keep W, as the design has it.** **Also write `synced` by M**:
- it is one compare at a completion, and a completion is beside an fdatasync;
- it turns a future change that drops W into a correct one rather than a silent loss.

**Assert in the Handle's sync completion:** `debug_assert!(tok > self.synced_tok)` under the
sequencer.
- It fires on any change that lets completions reorder.
- Remove it only together with a decision that M carries the soundness.

**K stays as it is** (a failed sync is kept), and its reach is K': every sync in flight at the
failure answers it.

**Tests for S2:**
1. **The sequencer holds the second sync.**
   - The fault-injection mode holds a sync in flight.
   - A second sync request returns only after the first completes.
   - The second's coverage is taken when it issues.
   - Mutant: the wait removed, so the test fails.
2. **`synced` never moves back.**
   - A test seam switches the sequencer off.
   - Two syncs are issued; the newer completes, then the older.
   - The test then writes past one ring lap, cuts the power at every point (the crash-state
     API), and requires the newer sync's content.
   - Mutant: assignment instead of max. The model's counterexample to transcribe is
     `B 1 S 1 E 1 1 1 1` at R=8.
3. **A failure reaches the sync beside it.**
   - Two syncs are in flight with the seam on, and the newer one fails.
   - The older completes with `Ok` from the device.
   - It must answer the failure.
   - Mutant: K' dropped. The model's counterexample is `1 B B G E`.

## Stream class (stage S6, the log and the manifest slots as sealed records)

**M is not load-bearing.** Writing `synced` by M costs nothing and is still the rule to write
down.

**W is load-bearing only for failures (K').** Keep W, or keep K' explicitly.

**C and K are load-bearing in every shape.**

**Assert:**
- the record of what a sync covers is taken where the sync is issued: one place, before the
  fdatasync, never refreshed after it;
- `debug_assert!` that `synced` is monotone.

**Tests for S6:**
1. **K with two syncs in flight.**
   - Hold the first sync with the fault seam, then fail it.
   - A later sync must not acknowledge.
   - The power cut must keep every acknowledged record.
   - Mutant: K dropped.
2. **C.**
   - A sync is issued, a commit is written, and the sync completes.
   - The next `sync` with nothing new must still sync the commit's frames.
   - Mutant: coverage taken at completion. The model's counterexample is `B 1 B E` without W,
     and `B 1 S` with W.

The manifest slots under S6 are a Replace-like choice made by "known durable", so for them M
**is** load-bearing (engine pair 2 below). Keep the slots' syncs on one thread as today, and
assert their sequence monotone in the slot-choice update.

## The engine as it stands (read only)

**Pair 1: the segment writer and its helper.**
- An acknowledgement can rest on the helper's overlapping sync's outcome before the helper has
  published it: the K' shape, without W.
- Fix sketch: `begun`/`ended` counters, and `synced_all` waits for the overlapping helper sync to
  publish.
- It adds a wait on a success path, so it is priced first (decision 10).

**Pair 2: the pending manifest's helper.** Sound by W: one slot sync in flight, landed by
assignment after the wait.

**Pair 3: the presync and the seal's fsync.** Sound: the segment lands durable only after both
answers, with the writer's own word ordered after its presync.
