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

## Third registration: the default is declared, not searched for

Written after two full runs and before any run of the code described here.
Those two runs are superseded and are not evidence for what follows.

Three things they exposed, none of which is a result about the engine.

**The argmax was noise.** The two runs chose k=2 and k=1 as the best
threshold, 3.6% and 3.9% apart in opposite directions, and every finding was
gated on that choice -- so each run adjudicated a different policy and called
it the same claim. The default is now **declared**: `k=2`, the smallest
threshold that cannot thrash, because k=1 re-enters the kernel's default
advice on a single scan while no k above 1 ever reaches its threshold on a
workload that never scans twice in a row. `F66.1`, `F66.2`, `F66.3`, `F66.5`
and `F66.6` all test that declared value. The sweep's argmax is still
recorded, as context for what the default leaves on the table, and nothing is
gated on it.

**The scans never moved.** The start key was indexed by position within a
phase, so every cycle re-scanned the same regions -- warm after the first --
and collapsed to a single start key whenever a phase held one scan, which is
the phase-free workload `F66.6` drives. A scan that is always warm cannot
tell one advice from another, so `F66.6` was measuring nothing. The start now
walks the whole key space across the pass.

**`F66.3` was a median against a cliff.** Its two runs came in at 102.0% and
95.3% of fixed random against a hard 95% bar, and the arms differ only by a
counter. It is now `compare`, like `F66.6`.

`F66.6` also measured one arm twice whenever the argmax was 1, while its
evidence asserted a contrast between k=1 and the default that no arm had
tested. The thrash arm is now pinned at k=1 and asserted distinct from the
default.

| | outcome | reading |
|---|---|---|
| P9 | `F66.1`, `F66.2`, `F66.5` hold on the declared k=2 with the scans moving | the structural argument for the default survives a colder workload than the one that produced P1-P4 |
| P10 | `F66.5` fails: the declared default is more than 10% off the best k at some phase length | k=2's one-scan delay is not free at short phases, and either the default is k=1 with the thrash cost priced, or there is no single default |
| P11 | `F66.6` fails | a phase-free workload pays for the policy; adaptive ships opt-in whatever the phased arms say |

At `ci` this code gives P10 and P11 both landing, which is expected and is not
evidence: a `ci` scan phase is 8 calls, so entering the default advice one
scan late costs an eighth of the phase, against a hundredth at `full`. That
`ci` and `full` should disagree here is a property of the workload sizes and
is the reason `ci` is not citable.

## Outcome of the declared-default runs, and the fourth registration

Two `full` runs, agreeing on every finding.

`F66.1` **holds**: the default reaches the oracle. 99% and 97% of it, both
`NO DIFFERENCE`. There is nothing left for a better switching policy to win
on this actuator, which is the useful half of a tie.

`F66.2` **holds** at **7.227x** and **7.230x** over fixed `MADV_RANDOM`. The
phase split is the mechanism and is worth quoting: fixed random spends 12.70
seconds of a repetition in the scan phase against the policy's 1.69, and the
kernel default spends 3.33 seconds in the read phase against 0.09. Each fixed
arm loses a different phase; the policy loses neither.

`F66.3` **holds**: no cost when nothing ever scans, and zero switches.

`F66.5` and `F66.6` **failed on k=2**, and P10 and P11 both landed. The
declared default was 78% and 83% of the best k at some phase length, and
**33.2%** and **30.8%** of the better fixed advice on a workload with no
phases. On the same runs k=1 was 100% and **1.5x**.

So the structural argument that set the default at 2 was wrong, and it was
wrong in its unit. It ran: k=1 re-enters the kernel's default on a single
scan, so an alternating workload thrashes, and the safe default is the
smallest k that cannot. The thrash is real -- 456 switches a repetition --
and it is irrelevant, because a switch is a `madvise` at about 1.3 us and
being in the wrong mode for one cold scan of 500 entries is milliseconds. **k
counts calls, and a scan call is not worth a point read.** One scan carries
five hundred entries of evidence; requiring two consecutive scans demands a
thousand entries' proof of what the first call already established. That is
P6's lesson arriving through a different door: the counter's unit is work,
not calls, and at k=1 the distinction disappears because one call is enough.

P10 named this outcome in advance -- "either the default is k=1 with the
thrash cost priced, or there is no default" -- so what follows is the
registered decision procedure rather than a fit to the data.

| | outcome | reading |
|---|---|---|
| P12 | all six hold with the default declared at k=1 | the policy is "advise by the verb the caller used", with no hysteresis and no counter, and it may be the default |
| P13 | `F66.6` still fails at k=1 | no threshold is safe on a phase-free workload and adaptive ships opt-in |

A threshold of 1 is not a tuned constant, which is the part worth noticing.
It is no counter at all: `MADV_RANDOM` on a point read, the kernel's default
on a scan, decided by the call the engine is already inside. The hysteresis
was machinery added against a thrash that pricing shows costs 3% where being
in the wrong mode costs 3x.
