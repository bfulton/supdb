# Apple Silicon, via localmost

Two `ext-kv --profile full --engines supdb,lmdb` runs on a self-hosted M-series
Mac, taken to answer one question: **does this host hold still?**

It does. Ratio drift between the two runs:

| | run 1 | run 2 | drift |
|---|---|---|---|
| load | 8.966x | 8.517x | -5.0% |
| read | 2.315x | 2.344x | +1.2% |
| scan | 1.031x | 1.058x | +2.7% |

Absolutes moved 0.8-6.0%. On the x86 cloud VM that produced everything in
`results/`, EXT.1 read 0.866x, 0.891x, 0.892x, 1.010x, 1.154x, 1.043x and
1.331x across seven equally rigorous runs with the code unchanged between
several of them, and LMDB's own load figure ranged from 508,205 to 1,034,797.

The Mac managed that **while busy**: loadavg was 4.21 for run 1 and 6.76 rising
to 9.61 during run 2. A loaded laptop is an order of magnitude steadier than an
idle shared VM, which is not what I expected and is the whole finding.

## Not citable, for two separate reasons

**Architecture.** These are aarch64 with 128-byte cache lines and 16 KiB pages.
Every claim in `claims.json` measured on x86 stays x86's. Nothing here
contradicts or replaces it.

**LMDB's load number is a platform artifact, not an engine result.** It loads
at 165,385 ops/s here against 508k-1,035k on Linux. LMDB commits durably on
every batch and Supdb does not (`durable_commit: false`), so this axis measures
what macOS does with fsync on APFS more than it measures either engine. The
8.97x is not a win and must not be quoted as one. Read `scan`, which reaches
parity at 1.03-1.06x where the same code on Linux gives 0.65x, and `read`,
where 2.3x is larger than Linux's 1.1-1.6x but at least measures the same
thing on both.

What these files are for is the *spread*, not the values.


## Second campaign: the axes x86 cannot read

With `.localmostrc` approved (strict sandbox, six declared hosts), three runs
on the current engine (8a4a2fc):

**Buffered load pair** -- the EXT.10 axis, twice: supdb-buffered vs
lmdb-nosync at 0.857x (p=0.0073) and 0.852x (p=0.1599), engines drifting
<=1.5% between runs. The sequential-arrival deficit is ~15%, not the 47% the
drifting x86 host suggested.

**Durable pair** -- EXT.9's shape under F_FULLFSYNC: 0.411x where Linux says
0.081x. LMDB's durable commit collapses 4x on macOS while supdb's improves;
the axis belongs to whichever engine forces less writeback under fsync,
confirming f36's ledger decomposition from a second platform.

Portability notes recorded with the runs: `load_rss_mb` and the device-byte
columns read zero on macOS (`/proc` does not exist); throughput and file
size are unaffected.

## Third campaign: pricing log-first under a real barrier

One run on e286a86, the commit that made durability points log-first
(`ext-kv-durable-pair.run2.json`): supdb-durable 70,316 ops/s vs lmdb
165,249, **0.426x** at p=0.0022, rel_iqr 5.2%/1.2%. Against run1 on the
pre-fix engine: lmdb, which nothing touched, moved 1.8%; supdb moved +5.4%.

That flatness is the finding. The same change moved Linux from 0.081x to
0.223x, because there the per-batch fsync was flushing a 68 MB mapping's
dirty pages and log-first shrank the flushed footprint to a few KB. On
macOS `F_FULLFSYNC` is a full device barrier whose cost barely depends on
the bytes riding it, so shrinking the footprint buys ~5%: the durable-point
cost here is the *barrier count*, and both engines pay exactly one per
batch. Two platforms, two different dominant terms, both now measured --
and the same conclusion from both: what remains on this axis is amortizing
work per point (the seal, the 64-shard block writes), not shrinking the
synced bytes further.

## Fourth campaign: the value log under the same barrier

Run 3 on 3f08f11, the head that carries `Options::log_values`
(`ext-kv-durable-pair.run3-valuelog.json`): supdb-durable 83,865 ops/s vs
lmdb 171,108, **0.490x** at p=0.0022, rel_iqr 1.9%/2.0%. The comparator
moved 3.5% over the run2 pair; supdb moved **+19.3%**.

The third campaign ended by predicting exactly this experiment: it said the
synced bytes were already off the table and "what remains on this axis is
amortizing work per point (the seal, the 64-shard block writes)". The value
log is that amortization -- a durability point now appends unsealed bytes
and seals nothing -- and it bought 19.3% under a barrier whose cost the
bytes cannot move. Against the same change's +30.6% on Linux, the gain
compressed but did not vanish: the seal-and-section work was a real minority
term even under `F_FULLFSYNC`, and the residual 2.04x gap with both engines
paying exactly one barrier per batch is the barrier-count floor, now
measured on both sides of the change.

This is also the first run on this host with the device-byte column
populated (`proc_pid_rusage` landed between run2 and run3): supdb-durable
sends 831.9 MB per load against lmdb's 199.1, ~7.2x amplification against
1.7x -- and the x86 record for the identical workload reads ~834 MB, the
two platforms agreeing on the write path's footprint to within 0.3%.

Aside, observed but not gated (no finding is emitted for it): in this pair
supdb-durable read at 2.25M ops/s against lmdb's 1.07M on the loaded store.
The x86 read comparison (`EXT.11`) uses the buffered arm and cannot
separate the engines; if a read lead exists anywhere, this host is where
to measure it properly.

## The read axis, where x86 could not answer (replicated)

The same day EXT.11 flipped to `fails` on x86 -- 1.243x at p=0.37 and 1.179x
at p=0.13, two runs unable to separate the engines -- the buffered pair on
this host separated them at the first attempt
(`ext-kv-buffered-read.run1.json`): reads 2,590,359/s against 1,066,747,
**2.428x at p=0.0022**, rel_iqr 0.2%/0.3%; warm scan 62.0M entries/s against
52.8M, **1.174x at p=0.0022**. The durable pair taken 30 minutes earlier
corroborates from a different supdb arm: 2.25M reads/s against the same
lmdb 1.07M, comparator agreeing across the two runs to 0.2%.

Run 2 (`ext-kv-buffered-read.run2.json`) replicates it: reads 2.414x at
p=0.0022 (2,584,672 vs 1,070,531 -- each arm agreeing with run 1 to under
0.4%), scan 1.178x, and it held that tightness under loadavg 4.6-5.6. The
honest statement is architecture-conditional: on x86 the read paths cannot
be told apart; on Apple Silicon (128-byte lines, 16 KiB pages) supdb reads
2.4x faster and scans 1.17x faster. Which of the two mechanisms -- the
flatindex probe touching one line where a B-tree descent touches several,
or the page size quartering LMDB's tree depth-to-bytes ratio -- carries the
difference is not yet decomposed; do not guess it into prose.

## The mechanism of the read lead, decomposed and replicated

Two runs of `ext-readdecomp` (`ext-readdecomp.run{1,2}.json`), verdicts
agreeing on every axis that clears the gate:

- **Depth acquitted.** The lead does not grow from 100k to 4M keys -- it
  shrinks if anything (0.663x nd, then 0.641x at p=0.03). An O(log n)
  descent against an O(1) probe would show the opposite.
- **Memory geometry acquitted, compute convicted.** Cache-resident, the
  lead does not merely survive -- it widens to 4.758x and 4.505x (2.6-3.0x
  over the uniform working set). A lead that needed 128-byte lines or
  16 KiB pages to exist would die when nothing misses; this one grows when
  memory stalls stop masking it. The probe simply executes less work than
  the descent, and Apple Silicon's wide core makes the difference visible
  where the x86 host's noise floor does not.
- **Value size: unresolved.** 0.756x at p=0.03 in run 1, 1.158x nd in run
  2 -- did not replicate, recorded as noise.

Practical reading: the 2.4x buffered read lead on this host is not an
artifact of Apple's memory system; it is the flatindex probe being cheaper
than a B-tree walk, visible wherever the core is wide enough to show it.
