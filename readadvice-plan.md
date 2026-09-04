# ReadAdvice: making the workload-following advice the engine's default

Written before any of it is built. Outcome appended after.

## What is already settled, and what is not

`f66-adaptive` found the policy and its threshold, and both are registered:
advise `MADV_RANDOM` on a point read, the kernel's default on a scan, switch
on the first call of the other kind. It beats a fixed `MADV_RANDOM` 7.65x and
7.94x on a phased workload (`F66.2`), ties an oracle that knows every phase
boundary (`F66.1`), costs nothing when nothing ever scans (`F66.3`), and is
still 1.5x the better fixed advice when the workload has no phases at all
(`F66.6`).

None of that was measured on a `Db`. It was measured on **one `Blob` over one
mapping**, which is the mechanism and not the engine. Three differences matter
and none is settled:

1. **A store has many mappings.** Every `Seg` maps its own file, so a
   transition is one `madvise` per live segment rather than one. f66's switch
   cost was 1.3 us against a wrong-mode scan in the milliseconds -- a ratio
   with room in it, but the numerator scales with the segment count and the
   denominator does not.
2. **The memtable is not advised at all.** A read that the memtable answers
   pays the policy's branch and gets nothing for it, and a store under write
   load answers a large share of reads that way.
3. **Segments come and go.** A seal or a merge opens new segments, and they
   have to inherit the mode the store is currently in rather than the option's
   initial value. Getting this wrong is silent: reads stay correct and only
   the advice is stale, which is exactly the kind of bug that survives every
   correctness test.

## The change

`Options::advise_random: bool` becomes:

```rust
pub enum ReadAdvice { Default, Random, Adaptive }
```

`Default` and `Random` are today's two settings. `Adaptive` is the policy, and
the threshold is not a parameter because `F66.5` and `F66.6` say it is one --
a knob whose only good value is known is a knob nobody should have.

`Db` holds the current mode and the segments it has advised. `read_all` and
`count` put it in `Random`, `scan` puts it in `Default`, each a no-op when the
mode already matches. `Seg::open` takes the store's current mode so a segment
from a seal or a merge inherits it.

## Registered predictions

| | outcome | reading |
|---|---|---|
| Q1 | `F67.1` holds: on a phased workload over a real `Db` with several segments, `Adaptive` beats `Default` and beats `Random` | the mechanism survives the move from one mapping to a store, and the default may flip |
| Q2 | `F67.2` holds: on a workload with no phases, `Adaptive` is not resolvably slower than the better fixed setting | what `F66.6` showed for one mapping holds for N |
| Q3 | `F67.3` holds: with the store fully in memory, `Adaptive` is not resolvably slower than `Default` | the policy costs nothing where it can win nothing, which is the case most users are in and the one that decides whether it is safe as a default |
| Q4 | `F67.4` holds: a segment opened by a seal or a merge is in the store's current mode | the inheritance bug above is absent, checked rather than asserted |
| Q5 | `F67.1` fails, or `F67.3` fails | per-segment switching costs more than the mechanism buys, and `ReadAdvice` ships with `Default` as the default |

Q3 is the one to watch and the one f66 could not ask. Every f66 arm ran
against a file eight times the page cache it was given, because that is where
the advice can matter. A store that fits in memory is where most stores are,
the advice can win nothing there, and all it can do is cost -- N `madvise`
calls per phase change plus a branch per operation. If that is measurable,
`Adaptive` is a bad default however well it does out-of-core.

Q4 is not a timing question and does not need a `full` profile: it opens a
store, puts it in one mode, forces a seal, and asks the new segment what it
is. It is here because "the advice is stale" has no symptom a correctness
test would catch.

## What the outcome decides

Q1 through Q4 all holding is the only case in which `Adaptive` becomes the
default. Q5 in either form ships the enum with `Default` as the default and
`Adaptive` available, and records here which of the two costs bit.
