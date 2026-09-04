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
