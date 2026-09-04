# f68-prefetch: the engine knows the span, and the kernel is guessing

Written before anything is built. Outcome appended after.

## Why there is anything left here

`f66` and `f67` landed the adaptive advice and it ties an oracle that switches
at every true phase boundary (`F66.1`, `F67.1`). That is the useful half of a
tie: nothing further can be won by *switching better*. Whatever is left has to
come from a different actuator.

`madvise` has one rung above the kernel's default -- `MADV_SEQUENTIAL` -- that
this project has never measured, and one genuinely continuous dial:
`MADV_WILLNEED` over a range the caller chooses, which is not a hint about
policy but an explicit asynchronous fetch of exactly those bytes.

The second one is interesting because of something the engine already has and
does not use. `Db::scan(from, limit, f)` is *told* how far it will walk before
it touches a page, and `plan_exts` already computes the byte ranges a read
needs -- that is how the browser reader fetches over ranged HTTP (`W4.1`,
`W6.5`). Native reads throw that away and let the kernel guess.

## The feasibility probe

Python and ctypes over a 2 GB file, cold each time by unmap, `drop_caches`,
remap. **Not evidence and not a claim**: one host, one run an arm, no
interleaving, no statistics. It exists to decide what is worth building, and
it changed what that is.

A walk of the whole file made `MADV_SEQUENTIAL` look like the answer at
**12.5x**. At the span lengths a scan actually walks -- 200 spans of 2 MB
spread over the file -- it is worth **1.01x**. The ramp that pays over two
uninterrupted gigabytes never gets going in a bounded span, and a scan is
always a bounded span. Had the probe stopped at the first shape it would have
recommended the wrong dial.

What the bounded shape shows instead, over spans of 256 KiB to 8 MiB:

| span | kernel readahead, bytes fetched per byte read | `MADV_RANDOM` + `WILLNEED` |
|---|---|---|
| 256 KiB | 5.12x | 1.00x, and 2.47x faster |
| 1 MiB | 5.12x | 1.00x, and 4.07x faster |
| 2 MiB | 4.00x | 1.00x, and 2.85x faster |
| 8 MiB | 2.98x | 1.00x, and 1.98x faster |

Faster *and* cheaper, which is not the shape of a trade. The reason is that
the kernel cannot see where the span ends, so it reads past it into data the
scan will never touch; the engine can see, because the caller said.

## What would be built

`ReadAdvice::Adaptive` currently leaves `MADV_RANDOM` for the kernel's
readahead while scanning. The candidate replaces that with: stay in
`MADV_RANDOM` always, and issue `MADV_WILLNEED` over the byte range a scan is
about to walk, derived from the same plan the sparse reader builds.

If that works it is *simpler* than what ships, not more complex. There is no
phase to detect, no mode to switch, and no threshold -- the machinery `f66`
spent six findings justifying becomes unnecessary rather than better tuned.

## Registered predictions

| | outcome | reading |
|---|---|---|
| S1 | `F68.1` fails: `MADV_SEQUENTIAL` as the scan mode does not beat the kernel's default at engine scan lengths | the cheap rung is worth nothing here, and the whole-file probe was measuring a shape the engine never has |
| S2 | `F68.2` holds: `MADV_RANDOM` plus a span-sized `WILLNEED` beats today's adaptive scan by more than 1.5x | the dial is real on the engine's own read path |
| S3 | `F68.3` holds: it does so at about 1.0x read amplification against the kernel's 3-5x | the win is on both axes, and the device-bytes half is the one that does not drift with the host |
| S4 | `F68.4` holds: a policy that never leaves `MADV_RANDOM` ties or beats `Adaptive` | the switching machinery can retire |
| S5 | `F68.2` fails | a scan's bytes are not one contiguous span on the real read path -- key index, block table and blocks interleave -- so one `WILLNEED` per scan either misses them or fetches the wrong ones |

S5 is the one to expect. Every number above comes from walking contiguous
bytes, and a scan through `Blob` walks records whose values sit in blocks that
need not be adjacent, with index reads in between. The probe measured the
mechanism, not the path. If S5 lands, the question becomes whether
`plan_exts`'s ranges can be handed to `WILLNEED` in one call per scan rather
than one per extent, and that is a different experiment.

S1 is registered because it is the change I would have shipped on the strength
of the first probe shape, and writing down that it does not work is worth more
than quietly not doing it.

## Outcome

Two full runs an arm, and for the resident question eight.

**S1 landed: `MADV_SEQUENTIAL` is worth nothing here** (`F68.1`, recorded as
failing). The rung looked like a 12.5x answer on a whole-file walk and is
1.01x at the spans a scan walks. It never got an arm, and the reasoning that
made it not worth one is in the finding so nobody re-runs the flattering
shape.

**S2 and S3 landed** (`F68.2`, `F68.3`): 1.47-1.56x the shipped adaptive
advice at 1.18x read amplification against 3.44x -- 793 MB of device traffic
where the advice needs 2,310 and the kernel's own readahead needs 11,602.
Faster and cheaper at once, because the engine is told where the span ends.

**S5 did not land, and the reason is worth keeping.** It predicted one
contiguous `WILLNEED` per scan would fetch the wrong bytes, since a scan
interleaves index, block table and blocks. That is true of a contiguous span
and the probe only ever measured one. `prefetch_scan` does not use a span: it
walks the records the scan will cover and plans through `plan_exts`, the
planner the browser's ranged reads already needed. The machinery that answers
"which bytes does this read want" existed for a different reason and was
exactly what the native path lacked.

**S4 landed and does not carry the conclusion** (`F68.4`). A policy that never
switches mode does beat one that does, so phase detection is not what wins the
scan back. That would have retired the switching -- except for the cost.

**The cost is what decides it.** On a warm store that fits in memory the
planning is pure overhead, and eight full runs put prefetch at 1.038, 0.964,
0.933, 0.963, 0.922, 0.963, 0.976 and 0.964 of the adaptive advice. Six of
eight below one. `F68.6` states it as a bound rather than a tie because a tie
test cannot answer it twice running: at twenty-one repetitions the p-values
are 0.0000 and 0.0003 while the verdicts differ, since 7.8% clears the gate's
5% minimum effect and 3.7% does not.

So `ReadAdvice::Prefetch` ships as an option and `ReadAdvice::Adaptive` stays
the default. That is the same call `MADV_RANDOM` got, for the same reason: a
large win for a workload shape, chosen by somebody who knows their shape, and
not imposed on the many stores that fit in memory. `F67.3` asked the same
question of `Adaptive` and got a tie twice, which is why `Adaptive` is what
users get.

Two thresholds were restated in this experiment after watching them straddle,
which is a pattern that deserves naming rather than burying. `F68.2`'s 1.5x
bar was arbitrary and sat on top of a gate that already enforces an effect
size. `F68.6` moved from a tie test to a bound, and that one moves the
conclusion *against* the change -- a policy costing a few percent where most
stores live does not become the default -- which is the opposite of a bar
relaxed to get a pass.
