# Decomposing the Apple Silicon read lead

Written before the first `full` run, so the predictions cannot be fitted to
the answer.

## The fact to decompose

`EXT.11` (supdb-buffered vs lmdb, uniform point reads, 1M keys, 100B values)
is a tie on the x86 cloud host — 1.243x p=0.37 and 1.179x p=0.13 across two
full runs — and a replicated win on Apple Silicon: 2.42x and 2.41x, p=0.0022,
rel_iqr under 1.3% (`results/apple-silicon/ext-kv-buffered-read.run{1,2}.json`).
Nothing on the books says why. Candidate mechanisms, none yet asserted
anywhere:

- **(a) 128-byte cache lines.** Supdb's flatindex probe touches ~1 line;
  LMDB's descent touches several per node. A wider line forgives a single
  probe completely and a node search only partially.
- **(b) 16 KiB pages.** TLB reach: a descent touches ~depth distinct pages
  per lookup, a hash probe ~2, so page-count relief compounds differently.
- **(c) O(1) probe vs O(log n) descent.** Depth itself, priced differently
  per level by the two memory systems.
- **(d) Something else** — value handling, memory bandwidth, mmap fault
  behavior, plain instruction throughput.

## The experiment: `ext-readdecomp`

One new mode in `bench/external` (`external readdecomp`). Three workload axes
in **one process**, every cell and both engines interleaved round-robin per
rep, one warmup discarded, every ordering through `stats::compare`. Nothing
is compared across runs; the cross-architecture comparison is of *which
findings hold where*, never of numbers.

At `full` (defaults; all overridable):

| axis | cells | holds constant |
|---|---|---|
| key count | 100k / 1M / 4M keys, uniform | value 100B |
| hot subset | uniform over first 4,096 / 262,144 key ids of the 1M store | keys 1M, value 100B |
| value size | 8B / 100B / 1KB | keys 1M, uniform |

The 1M/uniform/100B cell is the anchor (EXT.11's own shape) and is shared by
all three axes. 500k reads per cell per rep, 7 reps + 1 warmup. Stores are
built once and swept warm (the `ext-sweep` precedent — rebuilding a 4M-key
LMDB store per rep fits no host's budget), so absolute ratios here are not
EXT.11's and must not be averaged with it; only shapes within the record are
read.

Hot cells use *contiguous* key ids so the touched bytes are compact in both
engines — adjacent leaves for LMDB, adjacent value blocks for Supdb. The
residual leans against Supdb: its hash probes stay scattered across the whole
index section even in the hot cell, so it keeps a TLB cost LMDB sheds, and a
hot-cell lead is therefore conservative.

Three findings, each gated, each `not_exercised` if the pair is unmatched,
an arm is missing, or any cell misses a read:

- **EXT.19** — the lead grows with key count (per-rep ratio at 4M vs 100k).
- **EXT.20** — the lead survives the cache-resident hot set (pair ordering at
  hot=4096; the companion comparison `EXT.20_lead_hot_vs_uniform` records how
  much the lead moved).
- **EXT.21** — the lead is independent of value size (per-rep ratio at 8B vs
  1KB; holds only on `no_difference`).

Companion comparisons in every record: per-cell pair orderings, and each
engine's own hot-vs-full and 4M-vs-100k sensitivity — which is what says *who*
moved when a ratio moves.

## Prediction table

Each row is written before any `full` run. "Lead" = the per-rep
supdb-buffered/lmdb read ratio. The mechanism must explain the *difference*
between the hosts, so every row is read jointly across the two.

| # | Outcome | Convicts | Because |
|---|---|---|---|
| P1 | EXT.19 **holds on both hosts** (lead grows with n on ARM and x86) | (c) depth | Descent deepens with log n on any architecture; a hash probe does not. If the growth is steeper on ARM, depth is the mechanism and (a)/(b) set the per-level price — c amplified by the memory system. |
| P2 | EXT.19 **fails on both** — ARM lead large and flat in n, x86 flat at ~1x | per-access (a/b) or compute (d), not depth | A depth mechanism cannot produce a lead that is the same at 100k keys (shallow tree, mostly cached) as at 4M (deep, DRAM-resident). Go to P3–P5. |
| P3 | EXT.20 **holds on ARM** with the lead ~undiminished (`lead_hot_vs_uniform` no_difference or greater) | (d)/(c)-as-compute; **acquits (a) and (b)** | A lead that persists when every touched byte is cache-resident never needed the memory system. What remains is dependent-access count and instructions, which this suite cannot split further without counters — that is the honest stopping point, and the record says so. |
| P4 | EXT.20 **fails on ARM** (hot lead collapses toward 1x) while the uniform lead is ~2.4x | (a) or (b) — the win needs misses to exist | Split with the 256k cell: **lead present at hot=256k ≈ full but dead at 4k** → per-miss cost, line width, (a); **lead absent at 256k too, present only uniform-over-1M** → it needs the page working set, not just DRAM misses → TLB reach, (b). The 256k cell overflows cache on both hosts but touches ~30–45MB, an order of magnitude fewer pages than the full store. |
| P5 | EXT.21 fails **Greater** (lead widest at 8B, compressed at 1KB) on both hosts | consistent with (a)/(b)/(c) — the lead lives in the lookup | The lookup is the only structurally different part; value bytes cost both engines the same. This is the expected companion to any of P1–P4 and mostly serves as a cross-check. |
| P6 | EXT.21 fails **Less** (lead grows with value size) | (d) value handling / bandwidth; acquits a/b/c | If the differential scales with bytes copied out, it is not the index walk at all. Would also predict the ARM lead reappearing in scans, which EXT.12 contradicts — so this outcome would demand a re-derivation. |
| P7 | EXT.21 **holds** (flat in value size) | ambiguous — a fixed per-read overhead on LMDB's side | A constant absolute gap shows as a ratio that shrinks with per-read cost; exactly flat suggests proportional costs everywhere, hard to reconcile with a pure lookup mechanism. Read together with the per-cell absolute numbers before concluding. |
| P8 | Any finding `not_exercised`, or the two Mac runs disagree on any verdict | nothing | Two runs is the minimum for a number here. A verdict that flips between them is drift, not evidence — same rule as EXT.10. |

Who-moved check, applied to every row: `lmdb_hot4096_vs_full` against
`supdb-buffered_hot4096_vs_full`. If LMDB gains much more from cache
residency than Supdb does, LMDB was the engine paying the memory system on
the uniform shape — corroborates P4; if both gain alike, corroborates P3.

## Dispatch plan (in order)

Never concurrently with any other timing benchmark. Each Mac run fits the
9-minute cap with margin: builds are one-time (~60s of store loading, the
durable-LMDB 4M store dominating), reads ~40s of measurement per full pass,
total measure step ~2.5–3.5 min plus the cached ~45s cargo build.

1. **Mac run 1** — workflow `quiet-bench.yml` on branch with this change:
   - `engines`: `supdb-buffered,lmdb`
   - `suite`: `readdecomp`
   - `internal`: *(empty)*
   - `args`: *(empty — the full-profile defaults are the design)*
   - expected wall: 5–7 min end to end.
2. **Mac run 2** — identical inputs. Replication, not averaging: the verdicts
   must agree run to run before any is believed.
3. **x86 run 1** (parent session, on the VM, serialized with everything else):
   `./target/release/external readdecomp --profile full --engines supdb-buffered,lmdb --out <run1-dir>`
   — ~3 min; needs ~4 GB free in `$TMPDIR` (override `TMPDIR` if `/tmp` is
   tmpfs — the 4M and 1KB stores total ~4 GB on disk at peak).
4. **x86 run 2** — same, different `--out` dir (the writer names the file
   `ext-readdecomp.full.json`, so a shared dir overwrites run 1).

If a Mac run brushes the cap, shrink with `args` in this order:
`--reads 250000` (halves measurement, keeps every cell), then
`--value-sizes 8` (drops the 1KB store, the most expensive build after 4M).

## Bookkeeping

- `verify` walks `claims.json` → results, so committing `ext-readdecomp`
  records before adding claims is safe; nothing gates them until claims
  EXT.19/EXT.20/EXT.21 exist. When they are added, note that `verify` reads
  `results/ext-readdecomp.full.json` — the per-host copies under
  `results/apple-silicon/`-style directories are records, not gates, and the
  claims entries should say which host's verdict they pin.
- `ci` runs of this mode are smoke only, like everything at that profile; the
  tiny-store verdicts it prints are shape checks, not evidence.
