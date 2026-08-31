# Proposed claims for f37-consolidate

Not yet in `claims.json`, deliberately: every `expect` below is provisional,
suggested by the `ci` smoke run only, and `ci` is never citable. Integrate
these after `f37-consolidate` has run at `--profile full` on the benchmark
host, with the expects set to what full actually recorded.

The ci smoke (5 reps + warmup, interleaved, 2,000 keys x 48 values x 100 B,
1 MB buffer) recorded: append 1.167x deferred over inline (p=0.0122), append
p99.9 1.287x shorter (p=0.0122), read-back 0.590x — deferred **loses** the
read axis (p=0.0122), device bytes 1.255x fewer (p=0.0056), file 0.794x
smaller (p=0.0040). Inline merged 30,000 times to deferred's 24,000 — the
deferred policy merges nearly as *often* (small suffix merges are what bound
the extent list), it just rewrites far less each time, so expect the
device-byte and throughput separation to widen at full, where runs outgrow
the 4 KiB size-class floor that dominates ci-scale merges.

## Entries to add to `findings`

```json
{
 "experiment": "f37-consolidate",
 "id": "F37.1",
 "expect": "holds",
 "because": "PROVISIONAL, awaiting full-profile measurement. Deferred (geometric suffix) consolidation lifts line-ordered append throughput over the inline whole-run merge: 1.167x at ci smoke. Inline rewrites O(n^2) bytes per key over its life; deferred amortizes to O(n log n) by rewriting an extent only into a run at least its own size. Cite the full numbers from results/ when they exist."
},
{
 "experiment": "f37-consolidate",
 "id": "F37.2",
 "expect": "holds",
 "because": "PROVISIONAL, awaiting full-profile measurement. Deferred consolidation shortens the append p99.9 (1.287x at ci smoke). This is F5.1's tail measured against the policy that causes it. Note what it does NOT shorten: the worst single append is the biggest cascade merge, which is O(run) under either policy -- deferral bounds how often the big rewrites happen, not how big the biggest one is."
},
{
 "experiment": "f37-consolidate",
 "id": "F37.3",
 "expect": "fails",
 "because": "PROVISIONAL, awaiting full-profile measurement. Recorded as failing on purpose: deferred consolidation taxes the read-back pass (0.590x at ci smoke), because a deferred key holds O(threshold + log n) extents where inline holds at most threshold, and Store::read_all pays per extent. This is the price of the write-side win and this entry is what keeps it from being forgotten. If the full-profile read cost is judged unacceptable, the policy needs a list-length cap before defer_merge can default on -- that decision belongs to whoever flips this entry."
},
{
 "experiment": "f37-consolidate",
 "id": "F37.4",
 "expect": "holds",
 "because": "PROVISIONAL, awaiting full-profile measurement. Deferred consolidation sends fewer bytes to the device for identical appended data (inline 1.255x more at ci smoke; expect a wider margin at full, where the inline rewrite is quadratic in a 256-value run instead of a 48-value one). The O(n^2) rewrite is a device cost before it is a latency cost, which is how W1.3 saw it first."
},
{
 "experiment": "f37-consolidate",
 "id": "F37.5",
 "expect": "holds",
 "because": "PROVISIONAL, awaiting full-profile measurement. The deferred store's file is no larger after close (0.794x at ci smoke): what deferral avoids is dead merge copies that reclaim has not caught up with, which is W1.3's dead-space mechanism. Space comparisons are drift-immune, but both arms are interleaved in one process anyway."
}
```

## Knock-on claims to re-examine if `defer_merge` ever defaults on

`defer_merge` ships **off**, so no existing claim moves in this change and CI
stays coherent. If full-profile numbers justify flipping the default, that
flip must re-run and re-examine, in the same commit:

- **F5.1** (`f5-latency`, expect `fails`): the latency tail is attributed to
  inline `merge_key`; a deferred default may move it to `holds`, and the
  `because` must be rewritten either way.
- **W1.3** (`w1-daysize`, expect `holds`): its `because` names this exact
  work ("the design panel's rank-2 item... exists to close most of the
  rest") and its 18.08x ratio should shrink substantially. The statement
  holds either way (term order still wins), but the number in the prose rots
  — re-read it out of `results/` per the standing rule.
- **W1.1 / W1.2** (day-index size): line-order was never the roll's shape,
  but re-run to confirm the fixed cost did not move.
- **EXT.6 / f12** file sizes and the logshed segment sizes: merged runs land
  in solo blocks under either policy; expect no movement, but space is cheap
  to re-check.
- **YCSB-E** (`ext-ycsb`, ungated): E's residual loss is attributed to
  publishing being O(key count), not to merging — do not expect this change
  to move it much; if it does move, that is a finding about the attribution.

## Prediction (falsifiable, recorded before the full run)

On the full-profile shape (4,000 keys x 256 values x 100 B, 1 MB buffer,
line-ordered):

- **F37.1 append throughput**: deferred/inline between **1.6x and 3.5x**
  (ci showed 1.167x at depth 48 where the per-merge floor dominates; the
  inline arm's rewrite grows linearly with depth, deferred's grows with its
  log, and depth rises 5.3x).
- **F37.5 file size**: deferred/inline between **0.35x and 0.7x**
  (0.794x at ci; the dead-copy share grows with depth on the inline arm).
- **F37.3 read-back**: deferred loses at **0.4x-0.8x** — worse than ci's
  0.590x is possible since the ladder deepens to ~9-11 extents/key.

If F37.1 at full lands under 1.3x, the mechanism is not doing what the
analysis says and should not ship as a default under any expects.
