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
rather than a tradeoff, and no timing harness can say where it went.

**Subtract the baseline, or the answer is wrong.** The first version of this
table compared raw totals from `loadprof --keys 20000` and concluded 1.11x on
instructions and 3.22x on D1. Both were understated, because the driver's own
setup dominates at that size: `Payload::new` alone accounts for 28% of D1
write misses and 46% of last-level write misses, and it is common to every
engine. Run each engine at `--keys 0` and subtract, exactly as this file
already says to do for `indexlab probe --lookups 0`. Per key of real work:

| per key, 20k keys | Supdb | LMDB | ratio |
|---|---|---|---|
| instructions | 4,358 | 3,191 | 1.37x |
| D1 misses | 57.7 | 13.3 | **4.34x** |
| **LL misses** | **9.4** | **0.45** | **20.96x** |

| totals, dhat at 50k keys | Supdb | LMDB |
|---|---|---|
| live blocks at peak | 50,305 | 2,077 |
| bytes at peak | 39.6 MB | 20.3 MB |

**Bulk ingest here is DRAM-bound, and only for Supdb.** LMDB takes
essentially no last-level misses per key. Supdb takes 9.4, which at roughly
80ns each is about 750ns of its measured 1,037ns per key -- most of the load.
The instruction gap is 1.37x and cannot explain a 1.85x wall-clock loss;
the memory gap is 21x and comfortably can.

dhat names the structure. Both engines run the same driver, which allocates
twice per key, so subtract 100,000 blocks from each: Supdb makes about three
heap allocations per key and LMDB makes none. Attributed:

- `store.rs:1539-1540`, in `put` -- two per key. `Pending::default()` starts an
  empty `Vec`, `put_uvarint` allocates it at about 8 bytes, and
  `extend_from_slice` of a 100-byte value immediately reallocates.
- `store.rs:1989`, in `checkpoint_inner` -- one per dirty key, from
  `sh.keys.key_at(idx).to_vec()` copying every dirty key onto the heap to
  build `changed`. On a bulk load every key is dirty.

The allocation *count* is the smaller half. The *pattern* is what the misses
are showing: each buffered value lands in its own malloc block, so a load
scatters writes across 39.6MB of small objects -- five times the 8MB
last-level cache, touched in hash order -- while LMDB appends into pages that
stay resident until they are written out. 50,305 live blocks against 2,077 is
the same fact from the other side.

That is a design defect rather than a tuning one, and it has a shape.
`Store::put` buffers per key, where `append` already stages into a shared
block builder at seal time. A per-shard arena that pending values are appended
into, with each key entry holding an offset and length, removes all three
allocations and makes the write pattern sequential. It is the change the miss
counts argue for, and it gets measured the way everything here is -- both arms
behind a flag, interleaved in one process -- rather than assumed.

## When a miss profile points the wrong way

`EXT.13` has Supdb 3.3x behind LMDB on keys arriving in order, which is the
common shape. Three tools were pointed at it and the first two misled.

**cachegrind, subtracted, per key of a 50k sequential load:**

| | Supdb | LMDB | ratio |
|---|---|---|---|
| instructions | 3,755 | 3,303 | 1.14x |
| D1 misses | 59.0 | 13.2 | 4.47x |
| LL misses | 18.0 | 0.29 | **62.5x** |

Sixty-two times the DRAM traffic per key reads like a scattered write path,
and there is an obvious story to hang on it: Supdb shards by key hash, so
keys arriving in order land in 64 different places while a B-tree fills one
page. That story is wrong. `cg_annotate` puts **1% of those misses in the
hash probe** and the rest in `checkpoint_inner`, `seal_shard` and the memcpy
inside them.

**Timing the phases directly, 1M keys in order:**

| phase | Supdb | LMDB |
|---|---|---|
| put | 0.420s (43%) | 0.341s (70%) |
| flush + checkpoint | 0.591s (57%) | 0.149s (30%) |

The put path is within 1.17x. The whole gap is the flush.

**Timing inside the checkpoint** (`SUPDB_CKPT_PHASES=1`):

| | |
|---|---|
| sort 1M keys | 0.057s |
| encode the index | 0.089s |
| **write the sections** | **0.406s** |
| fsync | 0.0007s |

68.5MB of index at 169 MB/s. So the checkpoint is not sorting or encoding, it
is *writing*, and what it writes is a structure LMDB does not have: F11.4
prices the mapped index at +73.5% on the file, deliberately, because a section
read in place cannot be compressed.

**What this rules out, which is the point.** Parallelising the index build
across shards is the obvious move and its ceiling is sort + encode: 0.146s of
a 0.985s load, so 15% at best, for a background thread in a single-writer
design. The 41% is I/O on a structure whose size is a format decision already
measured elsewhere. A miss count says where misses are, not which phase is
slow, and an instruction count says neither.

## The put path, and the profile that had to be thrown away

The whole-load decomposition (F31.1) puts 43% in `put`, against LMDB's
equivalent 0.100s lower. That difference is larger than everything lever 2
saved and it had never been looked at, because it is the phase that was
never the suspect.

**The first attempt at this profile was invalid and looked fine.** `loadprof`
syncs at the end, and cachegrind attributes to the process, so the trace was
the put path *plus* `checkpoint_inner` and `seal_shard` -- and those dominate.
The giveaway was `checkpoint_inner` at 13.6% of write misses in what was
supposed to be a put-only run. `--skip-sync` exists for this, and the fix is
worth stating: a profile of "phase X" that contains phase Y is not a noisy
profile, it is a profile of something else.

Measured properly, 200k keys, baseline-subtracted, per key:

| | Supdb | LMDB | |
|---|---|---|---|
| instructions | 1,739 | 3,450 | Supdb uses **half** |
| D1 misses | 26.1 | 13.2 | 2.0x |
| **LL misses** | **10.2** | **0.25** | **41x** |

**Supdb's put path is memory bound and LMDB's is compute bound.** Supdb does
half the work per key and is still 1.29x slower in wall clock. `cg_annotate`
puts 56% of the read misses and 55% of the writes in `__memcpy`, which is the
value being copied into the shard arena -- inherent, since a buffered write
has to buffer. LMDB reuses a small set of dirty pages that stay cache
resident; Supdb accumulates into arenas that are written once and never read,
so every cache line is compulsory.

That is a design property rather than a defect, and it bounds what tuning the
put path can return. What it does not excuse is redundant work, and there was
some: `put` called `get_or_insert(key)` and then `index_of(key)` -- two hash
probes of the same key -- directly below a comment reading "a put probes once
rather than twice". `slot_or_insert` returns the index, so it does now:

| put path, per key | before | after |
|---|---|---|
| instructions | 1,739 | **1,543** |
| D1 misses | 26.1 | 26.0 |

Exact, because cachegrind is a simulation and does not need repetitions.
11.3% of the path's instructions, and no claim is made about wall clock: the
path is memory bound, the saving is compute, and this host cannot resolve
either at that size. An instruction count is the right unit for a change like
this precisely because it is not a timing.

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
