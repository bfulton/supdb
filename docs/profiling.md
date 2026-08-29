# Profiling

Three findings in this project were wrong in ways a better rig would have
caught in one command: a scan benchmark that never dereferenced a key, a space
figure that double-counted a structure, and 180 ns per lookup attributed to
memory that turned out to be varint decoding. This is what the measurement
setup should be, what it currently is, and what is missing.

## What each question needs

| question | tool | here? |
|---|---|---|
| how long does it take? | `src/bench` — interleaved trials, median/IQR, Mann-Whitney gate | yes |
| how many distinct cache lines and pages does the access pattern *demand*? | `indexlab trace` — address tracer | yes, exact |
| how many misses would a given cache hierarchy incur? | **cachegrind** — simulated, deterministic | yes |
| how many misses did the *real* hardware incur? | `perf stat` (PMU) | **no** |
| which instruction and which data structure missed? | `perf record`, `perf mem` (PEBS/IBS) | **no** |
| where did every cycle go — frontend, backend, bad speculation, retiring? | `toplev` (TMA), VTune, uProf | **no** |
| false sharing between writer threads? | `perf c2c` | **no** |
| allocation count, size, lifetime, access pattern? | `valgrind --tool=dhat` | yes |
| page faults, syscalls, context switches? | perf software events, `strace` | yes |

## Why the hardware tiers are unavailable

This machine is a Firecracker guest. `perf` is installed and
`perf_event_paranoid` is set to −1, and every hardware event still returns
`<not supported>`: Firecracker does not virtualise the PMU, and that is a
deliberate design choice rather than a configuration gap. No amount of
installing fixes it.

**To get the hardware tiers, the benchmark host must be one of:**

- **bare metal** — everything works;
- **QEMU/KVM with `-cpu host,pmu=on`** — `perf stat` and `perf record` work;
  PEBS/IBS support varies by host CPU and kernel;
- a cloud instance that exposes the PMU — AWS bare-metal (`*.metal`) does,
  ordinary shared instances generally do not.

Once there, install `linux-tools-$(uname -r)` and `pmu-tools` (for `toplev`).

**`toplev` is the highest-value addition.** Top-Down Microarchitecture Analysis
classifies every cycle as retiring, bad speculation, frontend bound, or backend
bound, then drills in — backend splits into memory bound (L1/L2/L3/DRAM) versus
core bound. The varint finding took an address tracer, a subtraction argument,
and a purpose-built control layout to establish. `toplev` would have printed
"core bound" and ended the discussion.

## What we have instead, and why one part of it is better

**Cachegrind is deterministic.** The same binary and input produce identical
miss counts every run, because it is a simulation rather than a sample. That is
strictly worse than a PMU for *fidelity* — it models a cache, not this cache —
and strictly better for *regression detection*, because wall-clock numbers are
too noisy to gate in CI while miss counts are exact.

The cache model is pinned explicitly rather than detected, so the numbers mean
the same thing on any host:

```sh
valgrind --tool=cachegrind --cache-sim=yes \
  --D1=32768,8,64 --LL=8388608,16,64 --cachegrind-out-file=/dev/null \
  ./target/release/indexlab probe --layout hash+flat --keys 300000 --lookups 30000
```

`indexlab probe` builds one layout, performs a fixed number of lookups, and
exits, because cachegrind attributes to the process. Run it once with
`--lookups 0` and subtract to isolate the lookup cost from the build.

Measured this way, at 300k keys:

| layout | D1 rd misses/lookup | LLd rd misses/lookup | measured hit @10M |
|---|---|---|---|
| heap-hash | 5.58 | 4.08 | 366 ns |
| hash+flat | 4.67 | 2.86 | 494 ns |
| hash+flatfixed | 4.70 | 2.93 | **314 ns** |
| hash+paged | 7.91 | 3.35 | 682 ns |

`hash+flat` and `hash+flatfixed` differ only in how the extent is encoded, and
the simulation confirms it: their miss counts are the same to within 1.5%. They
are 180 ns apart. Whatever separates them is not memory — which is the
conclusion the address tracer reached independently, by a different method.

**The two software methods answer different questions and should both be run.**
The tracer measures *demand* — distinct lines and pages the access pattern
touches, exactly, with no cache model. Cachegrind measures *misses* given a
model, accounting for what stays resident. Agreement between them, as here, is
much stronger evidence than either alone.

## Worked example: why Supdb loses bulk ingest (EXT.10)

`EXT.10` has Supdb loading at 0.542x of an LMDB that is not syncing either.
An append-structured store beaten 1.85x at bulk ingest by a B-tree is a defect
rather than a tradeoff, and no timing harness can say where it went. Three
tools, one driver (`external loadprof`, which loads and exits so the counters
attribute to one access pattern):

| | Supdb | LMDB | ratio |
|---|---|---|---|
| instructions (20k keys) | 234,915,197 | 211,668,372 | 1.11x |
| D refs | 72,525,951 | 59,014,034 | 1.23x |
| **D1 misses** | 1,294,495 | 402,320 | **3.22x** |
| **LL misses** | 328,317 | 146,879 | **2.24x** |
| LLd read misses | 39,701 | 5,874 | 6.76x |
| dhat live blocks at peak (50k keys) | 50,305 | 2,077 | 24.2x |
| dhat total blocks | 252,546 | 100,159 | -- |

**The negative result is the useful one.** Supdb executes only 1.11x the
instructions for a 1.85x wall-clock loss, so the gap is not compute and no
amount of shaving the `put` path will find it. It takes 3.2x the L1 data
misses. Bulk ingest here is memory-bound.

dhat names the structure. Both engines run the same driver, which allocates
twice per key, so subtract 100,000 blocks from each: Supdb makes about three
heap allocations per key and LMDB makes none. Attributed:

- `store.rs:1539-1540`, in `put` -- two per key. `Pending::default()` starts an
  empty `Vec`, `put_uvarint` allocates it at about 8 bytes, and
  `extend_from_slice` of a 100-byte value immediately reallocates.
- `store.rs:1989`, in `checkpoint_inner` -- one per dirty key, from
  `sh.keys.key_at(idx).to_vec()` copying every dirty key onto the heap to
  build `changed`. On a bulk load every key is dirty.

The allocation *count* is worth perhaps 9% of the load. The allocation
*pattern* is worth much more, and it is what the miss counts are showing: each
buffered value lands in its own malloc block, so writing the load scatters
across 40MB of small objects, while LMDB writes sequentially into B-tree
pages. 50,305 live blocks against 2,077 is the same fact from the other side.

That is a design defect rather than a tuning one, and it has a shape:
`Store::put` buffers per key, where `append` already stages into a shared
block builder at seal time. A per-shard arena that pending values are appended
into, with each key entry holding an offset and length, removes all three
allocations and makes the write pattern sequential. It is the change the miss
counts argue for, and it should be measured the way everything here is --
both arms behind a flag, interleaved in one process -- rather than assumed.

## Reproducibility, independent of tooling

These matter more than the tools and cost nothing. Every one is now checked at
runtime and recorded in each result by `Env::warnings()`, so a number cannot be
cited without its caveats travelling with it.

| setting | why | how |
|---|---|---|
| governor `performance`, turbo off | frequency drift is indistinguishable from a code change | `cpupower frequency-set -g performance` |
| SMT off | a sibling thread can halve throughput on a shared core | `echo off > /sys/devices/system/cpu/smt/control` |
| dedicated core | scheduler migration adds cold-cache restarts | `isolcpus=` at boot, then `taskset -c` |
| ASLR off | allocation addresses change cache-conflict behaviour per run | `setarch -R` |
| THP set explicitly | measured at a few percent here, but must not differ between arms | `/sys/kernel/mm/transparent_hugepage/enabled` |
| swap off | an out-of-core result may otherwise measure swap | `swapoff -a` |

## The rule that outranks all of it

**Never compare two separate runs.** Between a pre-fix and post-fix run of the
external suite, the three *unchanged* comparators moved by +20% to +43%. Put
both arms behind a runtime flag and interleave them in one process, as
`f8-checksums` does. Where that is impossible — the huge-page experiment, which
toggles a global kernel setting — say so, and treat the result as indicative
rather than as clearing the significance gate.

## Winning on more than one architecture

The measurements in this repository are x86-64 with 64-byte cache lines and
4 KiB pages. Two things follow.

**Claims are pinned by architecture as well as by profile.** A layout threshold
calibrated for 64-byte lines is not a claim about a machine with 128-byte
lines, and `verify` now skips rather than fails such a claim, the same way it
handles a profile it cannot evaluate.

**"ARM" is not one target.** Graviton is 64 B / 4 KiB, the same geometry as
x86. Apple Silicon is 128 B / 16 KiB. For every question this project has
asked, those are different machines: distinct-lines-per-lookup roughly halves
on Apple Silicon, and TLB reach is four times better before huge pages are
considered.

A prediction worth recording before it is tested, since predicting first is the
only way a measurement can surprise you: **on 128-byte lines the compact
layouts should gain relative to the heap layout.** `heap-hash` chases three
pointers into unrelated regions and a wider line fetches more bytes it does not
use; `hash+flatfixed`'s 34-byte record straddles a line far less often, and
`packed`'s 16-record restart group falls from four lines to two. If that is
wrong, the cache-line story is not the mechanism and something else is.

`bench/aws/` runs the whole suite on a bare-metal instance of either
architecture, applying the hygiene above, and brings the results back. See
`bench/aws/README.md` for instance choices.

## Running what is available

```sh
bench/profile.sh            # cachegrind miss counts across the index layouts
./target/release/indexlab trace --keys 10000000   # distinct lines and pages per lookup
valgrind --tool=dhat ./target/release/indexlab probe --layout heap-hash --keys 200000
```
