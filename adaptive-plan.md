# f66-adaptive: can the read advice follow the workload, and what threshold?

Written before the first run. Outcome appended after.

## Why this is not the same question as f65

`f65-madvise` priced the two static settings and found a trade with a very
lopsided middle: `MADV_RANDOM` is worth **75.8x and 78.9x** on cold point reads
and costs the ordered scan **2.303x and 2.489x** (`F65.1`, `F65.3`). Neither
setting is right for a store that does both, which is why
`Options::advise_random` shipped defaulting off.

The ambition here is to stop choosing. A store has phases -- a compaction
window, a reporting scan, an hour of point lookups -- and the advice is a
per-mapping flag that costs almost nothing to change. If the engine can tell
which phase it is in quickly enough, it can have both sides of the trade.

## The three facts that make it plausible

A feasibility probe (Python, ctypes, this host, **not evidence and not a
claim** -- it exists to decide whether the experiment is worth building):

- a `madvise` switch over a 2 GB mapping costs **1.3 us** median, against
  roughly 4 ms for a single wrong cold read. About 3000 to 1.
- the modes differ in opposite directions independently of f65: cold random
  reads 5.9x faster advised, cold sequential 2.82x faster unadvised.
- **no inference is required.** The engine does not have to guess the phase
  from access addresses: `Blob::scan` and `Blob::read_all` are different
  calls. The phase signal is the operation type, and it is free.

That last point is what makes the detection latency interesting. It is not a
statistical estimate over a window; it is a counter over calls the engine
already makes.

## The policy, and why it is asymmetric

Being in NORMAL during point reads costs 75.8x. Being in RANDOM during a scan
costs 2.4x. That is a **30:1 asymmetry**, and it dictates the shape:

- leave NORMAL on the **first** point read -- one wrong read is the whole
  regret of a phase change in the expensive direction;
- enter NORMAL only after **k consecutive** scan operations, where k trades
  responsiveness against oscillation.

k is the number this experiment exists to find, and `F66.5` is whether one
value of it works well enough across phase lengths to be a default.

## Design

Arms, all interleaved in one process over one file, page cache capped by the
v1 memory controller and dropped between repetitions, exactly as f65:

| arm | advice |
|---|---|
| `normal` | never advised -- today's default |
| `random` | `MADV_RANDOM` throughout -- what f65 landed |
| `oracle` | switched by the harness at the true phase boundary |
| `adaptive-k` | the policy above, k in {1, 2, 4, 8, 16, 32, 64} |

`oracle` is the arm that makes this falsifiable. It is the bound on what any
policy could reach, so `adaptive` is judged against what is achievable rather
than against whichever static arm flatters it.

The workload is phased: alternating runs of point reads and ordered scans,
with the phase length swept as well as k, because the answer depends on the
ratio between them. Short phases are where oscillation lives.

Rule 3: no cap, or a file that fits inside it, and every finding is
`not_exercised`. Rule 4: every arm reports latency distribution, peak RSS and
device read bytes, and read amplification comes from `/proc/self/io`.

## Registered predictions

| | outcome | reading |
|---|---|---|
| P1 | `F66.1` holds: adaptive at its best k within 10% of oracle | the policy is good enough that the remaining gap is not worth more machinery |
| P2 | `F66.2` holds: adaptive beats `random` by >1.5x on a phased workload | it earns its keep against what f65 shipped |
| P3 | `F66.3` holds: adaptive costs <5% against `random` on a workload with no scans at all | the machinery is safe to leave on when it never fires |
| P4 | `F66.5` holds: one k is within 10% of the best k at every phase length | a single default exists, which is the whole ask |
| P5 | `F66.2` fails at short phase lengths | oscillation eats the gain; adaptive stays opt-in and the default stays as f65 left it |
| P6 | `F66.1` fails with adaptive far from oracle at every k | the operation type is a worse phase signal than it looks, most likely because a scan phase is a handful of long calls rather than many short ones -- in which case the counter should be over *pages touched*, not calls |

P6 is the one I would bet against and the one that would teach the most. A
scan is one call that touches thousands of pages; a point read is one call
that touches one. Counting calls treats those as equal, and if the phase
detector is fooled by that, the fix is to count work rather than calls.

## What would make this the default

`Options::advise_random` is a bool today. If P1 through P4 all hold it becomes
`ReadAdvice { Default, Random, Adaptive { enter_seq } }` with `Adaptive` the
default, and `F65.3`'s scan penalty stops being something a user has to know
about. If P5 or P6 lands, adaptive ships opt-in and the default stays where
f65 put it, with the reason recorded here.

## Second registration: the case a default has to survive

Written after the first full run answered P1 through P4 and before any run of
`F66.6`. The first run is not evidence for what follows and the code that
produced it did not contain this arm.

Everything above has phases. A default does not get to assume them. The
adversarial workload is the one with **no phase structure at all** -- a reader
that alternates a point read and a scan -- because that is where a counter
over consecutive scans has nothing to lock onto.

Threads are not the shape of this risk, which is worth writing down because it
is the first place one looks. `Blob` holds a `RefCell` and is deliberately not
`Sync`, so a `Db` is not shared across threads: every reader thread maps the
file itself and advises its own mapping, and two threads cannot fight over one
flag. What one thread can do is alternate.

| | outcome | reading |
|---|---|---|
| P7 | `F66.6` holds: on a perfectly alternating read/scan workload the default k is not resolvably slower than the better fixed advice | the policy degrades to the right fixed arm when there is no phase to find, and may be the default |
| P8 | `F66.6` fails | a phase-free workload pays for the policy, so adaptive ships opt-in whatever P1-P4 said |

The mechanism P7 rests on is worth stating in advance so that it is a
prediction rather than a reading: with no two scans ever consecutive, the
counter never reaches any k above 1, so every such arm stays in `MADV_RANDOM`
-- which is the right place to be on a workload whose reads are cold. If that
is right, `k=1` is the only arm that thrashes, and the smallest safe default
is the smallest k that is not 1. That would make the choice of k a structural
argument rather than the median of a sweep, and `F66.5` and `F66.6` would be
agreeing for different reasons.

If they disagree -- if `F66.5`'s most robust k is 1 -- the two are in tension
and the default is the smallest k that satisfies both, or there is no default.
