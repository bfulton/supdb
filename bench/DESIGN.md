# The benchmark suite

One question: how fast is supdb against what people would use instead, on the
workloads that matter, on the machines people actually run, and is that getting
better or worse.

The suite is a time series of measurements. There are no claims, no expected
states, and no thresholds anyone typed. A run appends a row; a regression is a
row outside the error bars its neighbours drew.

## Workloads

Five, plus two floors. Each yields one or more quantities.

| workload | shape | quantities |
|---|---|---|
| `load` | n keys in key order, 100-byte values, durable per batch | ops/s, device bytes written per byte stored |
| `load-shuffled` | the same keys in shuffled order | ops/s |
| `read` | uniform point reads over the loaded set | reads/s, p99 µs |
| `scan` | one ordered pass over everything | entries/s |
| `ycsb` | core A–F on the loaded store, zipfian, a sixth of the keys in operations per mix | ops/s per mix |
| `wal-floor` | framed 1,000-record batches appended to one file, one `fdatasync` each, no engine | ops/s |
| `scan-floor` | one `mmap` sequential walk of a file the top rung's size (capped at 4 GiB), no engine | bytes/s |

Every workload runs at a ladder of store sizes, not one: keys at 1, 3, 10,
30 ... × 10⁴ up to the scale's cap. A number is a point on a curve, and the
curve is what shows where an engine's behaviour changes — most importantly
the knee where the store crosses the machine's memory. A geometric ladder
costs about 1.5× its largest rung, so the curve is nearly free.

The floors are per-machine constants, not per-engine and not per-size. They
are what "as fast as possible" means on that host; an engine's distance from
them is the headroom left. The scan floor's file fits in memory at `quick`
and is served from the page cache after its first walk, which is also what a
store that fits in memory sees; at `full` neither fits.

YCSB-D reads uniformly over the loaded keys rather than skewed to the latest
inserts: the latest distribution needs a Zipfian over a count that grows
with every insert, and tracking that is not a cost to charge the engines.

## Arms

Every comparison is guarantee-matched: durable against durable, buffered
against buffered. An arm is either a shipping supdb configuration or the
comparator a user would otherwise pick.

| guarantee | supdb | comparators |
|---|---|---|
| durable per batch | `supdb` (default), `supdb-noadvice` | `lmdb`, `rocksdb-tuned` |
| buffered | `supdb-ingest` | `lmdb-nosync`, `rocksdb-nosync` |

Every shipping option is an arm because a user can choose it and deserves the
number. An option that is never better than the default on any machine class
is a question the series answers.

All arms in one process, interleaved one round at a time, so a machine that
drifts drifts across all of them.

## Scale

Two. `quick` gates pull requests; `full` is the number.

| scale | top of the ladder | reps | where |
|---|---|---|---|
| `quick` | 300 000 keys — measured once: 160 s with every arm, the six YCSB mixes and the floors on a 4-core VM | 5 | every pull request, on a GitHub Actions runner |
| `full` | the rung at which the store is at least 1.5× the machine's memory | 7 | on demand or scheduled, on a quiet machine |

`full`'s top is a function of the machine, not a constant, so its curve
crosses the memory line everywhere it runs — the 16 GB box and the 4 GB VM
alike. The out-of-core regime is where an embedded store on a small VM
lives, and the old suite's largest run (100 MB) never entered it.

A rep is one complete pass of a workload for one arm. Arms are round-robined
within a rep; one warmup pass is discarded.

## Rows

One file per run: `runs/<scale>/<utc>-<engine-sha7>.json`. Nothing in it is
derived; everything is what was read or measured. The file is JSON; this is
its shape in outline:

```
utc, sha, rustc, scale
machine:
  arch, cpu_model, cpus, mem_total_kb, page_size,
  cache_line, cache_line_detected, l1d, l2, l3,
  kernel, governor, thp, smt_on, pmu_available, aslr_disabled,
  virtualised
measurements[]:
  workload, arm, size, quantity, unit, samples[]
```

`size` is the ladder rung in keys. The floors carry no size.

`samples` is the raw per-rep values — five or seven floats. Median,
confidence interval (CI) and spread are computed when the series is read, so a change to the statistic
recomputes history rather than stranding it.

`machine` is read, not classified. The class — which rows are comparable —
is derived when reading `runs/`, from `arch`, `cpu_model`, `cpus`,
`mem_total_kb` and `virtualised`. Change the classifier and history
re-buckets. `virtualised` is new: `kvm`, `firecracker`, `none`, from DMI or
the cpuinfo hypervisor flag. A noisy VM is a class like any other.

Rows are committed like any other change. `quick` runs on GitHub Actions write theirs as
a workflow artifact; a person commits the ones worth keeping. Bands come from
whatever is in `runs/`.

## Error bars

For each measurement, the CI is a percentile bootstrap of the median over its
samples, seeded from the values so it recomputes identically. With five to
seven samples that interval is essentially the sample range — coarse, and
true. It makes the gate conservative on a noisy machine, which is the right
direction to be wrong in.

## The gate

For each (class, workload, arm, size, quantity), take the last 10 rows at
the same scale in `runs/` for that class. The new row **regresses** if its CI lies
entirely on the worse side of every one of those rows' CIs. A row with a
regression fails. A row better than every prior CI is flagged, not failed:
it is either a win or a broken measurement, and a person should know which.

Fewer than three prior rows: no band, and the gate says so.

That is the whole rule. The window is the only parameter and it is stated
once, here.

## Figures

A figure states one thing, and the thing is its title — a sentence, not a
label: *Point reads stay ahead of LMDB until the store leaves memory*, not
*Read throughput vs. keys*.

The form is a curve per arm over the size ladder. Never a bar chart of one
size: a bar hides the knee, and the knee is the finding.

- x is store size in keys, log scale, ticks at the ladder rungs and nowhere
  else. y is the quantity; linear from zero unless the range forces log.
- One curve per arm: the default in ink, the shipping option in one accent,
  both comparators in one grey told apart by dash. The palette was computed,
  not chosen — an all-grey ladder failed the normal-vision separation check
  between its two lightest greys. Each curve is labelled at its right end in
  its own colour. There is no legend.
- The CI is a light band behind the curve. No whiskers, no markers unless
  the points are sparse enough to need them.
- A vertical rule where the store crosses `mem_total`, labelled *memory*.
  A horizontal rule for the floor where one applies, labelled *mmap floor*
  or *one-barrier floor*; a floor more than three times above every curve
  would flatten them into the axis, so it is stated in a note instead.
- Two axes and nothing else: no frame, no gridlines, no fill, no shadow.
  Tick labels are sparse and in the unit's natural form — 10⁴, 10⁵, not
  10000, 100000 — with the unit stated once on the axis.
- The title is the message in numbers: supdb's factor against each
  comparator at the top rung, in whichever direction the quantity is good.
  The context and the provenance (class, engine commit, date, reps) are
  the two lines under it.
- One typeface, two sizes. Black on white. A palette that reads in
  greyscale and to a colour-blind reader.

The rules are Doumont's: maximise the signal-to-noise ratio, put the message
where the eye lands first, and remove anything the reader would not miss.
Every figure is SVG, drawn from `runs/` by one program, so a figure that
disagrees with the data is a bug in that program and not a stale file.

## Machines

The series is columns, one per class. Nothing is the canonical machine. The
README figure is drawn per class from the latest `full` row, stamped with
its engine commit and date.

## Where the old suite goes

Removed from `main` and reachable in the history before this design landed:
`claims.json`, `results/`, `verify`, `figures`, the plan files, the `internal`
and `browser` experiments. The `because` prose is real reasoning and stays
reachable there. The browser reader's correctness checks — three readers
agreeing, ranges exact, dictionary walks matching — are engine tests and move
there.
The two floors become workloads.

## What stays true from before

Interleave the arms. Never compare two separate runs. Never run two timing
benchmarks at once. Two rows is not a band. Those are why a red is believed.
