# The benchmark suite

One question: how fast is supdb against what people would use instead, on the
workloads that matter, on the machines people actually run, and is that getting
better or worse.

The suite is a time series of measurements. There are no claims, no expected
states, and no thresholds anyone typed. A run appends a row; a regression is a
row outside the error bars its neighbours drew.

## Workloads

Five, plus three floors. Each yields one or more quantities.

| workload | shape | quantities |
|---|---|---|
| `load` | n keys in key order, 100-byte values, durable per batch | ops/s, device bytes written per byte stored, bytes on disk per byte stored |
| `load-shuffled` | the same keys in shuffled order | ops/s |
| `read` | uniform point reads over the loaded set, on one thread and again on 2 and on 4 | reads/s, p99 µs; reads/s per thread count |
| `scan` | `size/100` scans of 100 entries, from uniform random starts, on one thread and again on 2 and on 4 | entries/s; entries/s per thread count |
| `ycsb` | core A–F on the loaded store, zipfian, a sixth of the keys in operations per mix | ops/s per mix |
| `wal-floor` | framed 1,000-record batches appended to one file, one `fdatasync` each, no engine | ops/s |
| `scan-floor` | one `mmap` sequential walk of a file the top rung's size (capped at 4 GiB), no engine | bytes/s |
| `mem-floor` | one dependent load at a time around a permutation of a 64 MiB buffer's cache lines, no engine | chases/s |

Every workload runs at a ladder of store sizes, not one: keys at 1, 3, 10,
30 ... × 10⁴ up to the scale's cap. A number is a point on a curve, and the
curve is what shows where an engine's behaviour changes — most importantly
the knee where the store crosses the machine's memory. A geometric ladder
costs about 1.5× its largest rung, so the curve is nearly free.

`--bottom` starts the ladder above 10 000. The low rungs are cheap next to
the top one but not free, and when the question is at the top they are hours
spent re-answering what is already answered: on a 16 GiB M2 the rungs below
ten million keys took 7h17m of a run whose next rung alone needed about a
day. A row records the size of every measurement, so a bottom-limited run
contributes the points it took and claims nothing about the ones it skipped
— it is points rather than a curve, and the gate keys on size, so those
points still land in the series beside every other run's.

The floors are per-machine constants, not per-engine and not per-size. They
are what "as fast as possible" means on that host; an engine's distance from
them is the headroom left, and they are the gate's control over the host
(below). Three, because a host moves in three ways this suite feels: the
device's sync rate, the mapped sequential read, and the latency of a load
that nothing can predict, which is the shape of every table, index and
chain the engine walks and the one the other two floors cannot see. The scan floor's file fits in memory at `quick`
and is served from the page cache after its first walk, which is also what a
store that fits in memory sees; at `full` neither fits.

The load's two byte quantities are taken apart, and they are not the same
question. Device bytes written per byte stored is a flow: the device's
write counter across the load, what the engine made the disk do to take
the keys durably, amplification included. Bytes on disk per byte stored is
a state: what the store's files hold once the load is done and before any
read, every file under the arm's directory at any depth, in allocated
blocks rather than lengths, since a map file's length can run past what
was ever written to it.

Neither follows from the other. Ordered ingest cut supdb's device bytes
from 2.63 to 1.48 and left its 1.44 on disk exactly where it was, because
what went away was the WAL write and not a byte of the segment. The state
is also what the ladder's own top depends on, since `full` stops at the
rung where the store is 1.5x the machine's memory.

Both are read at one point, the moment the load's guarantee is met, so the
pair describes one moment rather than two. That is what an arm has on disk
when it says the load is durable and not what it settles to, so whatever an
engine leaves unmerged at that point -- a log tail, a memtable's worth not
yet sealed -- counts as a user would find it. What the choice of point
costs is measured rather than assumed: `rocksdb-tuned` holds its memtable
there on purpose, because flushing it would charge that arm a compaction
the others do not pay, and at 300 000 keys its WAL weighs 1.03 against the
1.00 of the SST it becomes.

YCSB-D reads uniformly over the loaded keys rather than skewed to the latest
inserts: the latest distribution needs a Zipfian over a count that grows
with every insert, and tracking that is not a cost to charge the engines.

Every workload runs one thread against the engine, and `read` and `scan`
run again on two and on four reader threads, so a row shows where an
arm's throughput stops scaling against LMDB and RocksDB at the same
count. Each count is a quantity of its own on the workload it repeats:
`reads_per_s_2t`, `reads_per_s_4t`, `entries_per_s_2t`,
`entries_per_s_4t`. The threaded passes run on the store as loaded, after
the single-threaded pass they repeat and before the mixes, with the writer
idle. Every thread reads through a handle of its own -- supdb's
`Db::reader`, an LMDB read transaction begun on the thread, RocksDB's
shared handle -- and draws its keys from a uniform generator seeded apart
from the others'. Every thread runs the single-threaded pass's
operations, so a pass on four threads is as long as the pass on one and
not a quarter of it, and the quantity is total operations over the pass;
they are released together, each times itself from its release to its
finish, and the quantity is the aggregate throughput over the span from
the first release to the last finish, so the spawning and the opening are
outside the clock. Every thread's bytes are held to
what the loaded store holds, because a reader that answered a different
store would post a throughput like any other. The counts are one, two and
four because the class the suite gates on has four cores; a class with
more runs the same counts, since the counts name the quantities and the
gate names every quantity. The threaded scans run once more after the
mixes, on the store they leave, as the workload `scan-mixed`: the keys
the mixes updated and inserted are unsealed there, so every handle's
pass builds the blocks it walks over them, and what the pass measures
is an engine's cost of reading past its own unsealed writes on several
threads; the pass on the store as loaded walks clean partitions and
measures the walk. Every thread's bytes are held to at least the loaded
store's, since a mix only adds to a key. Still on the backlog: the mixes threaded,
readers beside a writer, and writers beside each other, which is an
engine question first, since supdb is single-writer. An engine's own
threads are the engine's: supdb seals, merges and, with the block cache,
builds the cache ahead of a scan on threads of its own, as RocksDB
compacts on its own; a workload's thread count is the count of threads
reading, and the figure is their throughput with the engine doing what it
does beside them.

## Arms

Every comparison is guarantee-matched: durable against durable, buffered
against buffered. An arm is either a shipping supdb configuration or the
comparator a user would otherwise pick.

| guarantee | supdb | comparators |
|---|---|---|
| durable per batch | `supdb` (default), `supdb-forms`, `supdb-noadvice`, `supdb-nocache`, `supdb-cache256` | `lmdb`, `rocksdb-tuned` |
| buffered | `supdb-ingest` | `lmdb-nosync`, `rocksdb-nosync` |

Every shipping option is an arm because a user can choose it and deserves the
number. `supdb-forms` is where the range-read structure is kept: the writer
holds every overlaid block's form current at each commit, so a reader walks
one structure, against the default where each reader merges the partition
with the unsealed keys for itself. The pair prices the trade directly --
what the writer pays at its commits against what a scan past unsealed
writes costs -- and `scan-mixed`, the threaded scans on the store the mixes
leave, is the workload it was added for. A cache is matched like a guarantee: `supdb`'s block cache has no
bound, as LMDB's page cache has none but the machine's, and `supdb-cache256`
bounds it at the 256 MB `rocksdb-tuned` runs its block cache on, so each
comparator is read against the arm on the same memory. An option that is never better than the default on any machine class
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
alike. It is also why `full` is not a job with a timeout on it: on a 16 GiB
M2 its top rung is 222 million keys, and the ladder measured 344s at 300 000
against 19 815s at ten million — about 4.3× a rung for 3.3× the keys.

Do not size a run from that. It is a whole-rung figure averaged over seven
arms, and per arm the law holds for one engine and breaks for the other. One
pass at ten million took supdb 467s and LMDB 665s; at thirty million, LMDB
2 624s — 3.9×, as the ladder says — and supdb 10 288s, which is 22×. A pass
at a hundred million then ran 27.5 hours without supdb finishing it, already
past 9.6× its own thirty-million pass. Size a run from the arm that is slowest
at the rung you want, and treat any rung past the one you have measured as
unbounded until you have. (The run those figures come from also carried an
ordered-index leak, since fixed, so supdb's share of them is an upper bound.) The
out-of-core regime is where an embedded store on a small VM lives, and the
old suite's largest run (100 MB) never entered it.

A rep is one complete pass of a workload for one arm. Arms are round-robined
within a rep; one warmup pass is discarded.

`bench ab` is the instrument for choosing between two arms; the series is
not. It runs both in one process with the order swapped every other rep,
so neither stands first more often than the other, pairs them rep by rep
so a drift that lifts or drops both cancels, and reports a sign test over
the pairs rather than two medians. It also reports what the engine counts
-- forms held, snapshot builds, snapshot merges -- because a count is
exact where a whole-pass throughput is not. The first question put to it
had defeated three rows of this series: maintaining the canonical forms
regardless read 1.156x, then 1.186x, then 1.247x the other way on the
threaded scan mix. Fifteen pairs give it as 1.075x on thirteen of fifteen
(p 0.007) and `scan-lag` at full lag as 0.859x on fourteen of fifteen
(p 0.001) -- a real trade rather than a coin, and neither sign is the one
a single row happened to show.

The counts are the half that no timing gives, and they have to be read
as carefully as a timing. Maintaining regardless holds 3,099 forms and
2.8 MB at the end of a pass against 1,537 and 1.3 MB, twice the memory,
while reads take a form 24,225 times either way, on 6,000 of 12,000
scans through a caller's handle. That equality was read here as "the
forms built at a commit are not the ones read", and it is not: both arms
maintain by the end of a pass, since the gate moves when maintenance
starts and not whether it happens. The arm that actually separates them
is `supdb-settle`, which settles at a commit and builds no form -- takes
fall to zero and the threaded scan mix to 0.585x on four threads, all
fifteen pairs. Forms at a commit are worth 1.71x there and cost 1.172x
on `scan-lag` and 1.130x on ycsb-E. A count says what happened; which
two things to put beside it is still the hard part.

Swept over `--len`, `ab` separates what a scan pays once from what it
pays per entry, which is the difference between a structural cost and a
loop. Four threads at a hundred thousand keys, seven pairs a point,
against LMDB, fitting microseconds per scan over lengths 1, 10, 100 and
1000:

| | fixed per scan | per entry |
|---|---|---|
| `scan`, drained | 232 ns vs 126 (1.84x) | 3.99 ns vs 4.81 (**0.83x**) |
| `scan-mixed`, unsealed keys | 301 ns vs 134 (2.24x) | 6.91 ns vs 5.84 (1.18x) |

So the walk itself is faster than LMDB's and the losses are two things
that can be named: a fixed cost of about 230-300 ns a scan against its
130, which is the whole of why short scans read 1.7x to 2.0x behind, and
an unsealed key costing 6.91 ns against this engine's own 3.99 on a
drained store. At a length of a hundred those are 167 ns and 290 ns of a
993 ns scan, and closing both would put `scan-mixed` at about 0.70 us
against LMDB's 0.682. That is what the aggregate throughput of a row
cannot tell anyone, and it took four `ab` runs.

A counter has to be read where the thing happens. These said no read ever
took a form until the instrument was fixed: a pass opens two stores, the
loaded one and a fresh one for the shuffled load and the lag sweep, and
the counters were read from the second, where no reader handle scans at
all. Use `ab` for an option; use the series for a commit.

**The arm order is the same in every rep, and the arm that goes first pays
for it.** Interleaving was meant to spread a drifting machine across the
arms, and it does, but position is not drift: `plan.arms[0]` is first in
every rep, so whatever a rep's first pass pays it pays in the same arm every
time. Measured over four rows at one commit, every supdb arm beat `supdb` on
the load -- 1.041x, 1.076x, 1.058x, 1.072x, 1.072x for the arms in positions
two to six -- and `supdb-cache256`, whose budget is above anything the quick
ladder reaches, was 1.072x with fifteen of sixteen rungs ahead. On
`scan-lag` at full lag the same arm was 1.083x. Rotating the roster so that
one arm ran first instead inverted both: the arm that had been 1.076x on the
load became 0.934x, and 1.081x on `scan-lag` became 0.943x. So a cross-arm
ratio against `supdb` on those two quantities carries about 7% that belongs
to the order, and the direction always favours the arm being tried. What
survives the swap is what the suite can say: `scan-mixed` on four threads
read 1.135x with the arm third and 1.136x with it first.

What the penalty is made of, measured by timing each pass's load in two
parts and printing it by position. It is a fixed cost and not a rate: the
arm at position zero was dearer than the five behind it by 1.167x at ten
thousand keys, 1.062x at thirty, 1.100x at a hundred and 0.979x at three
hundred -- a few milliseconds, which is a fifth of a twelve-millisecond
load and a hundredth of a three-hundred-millisecond one. It is not the
teardown: removing the store a pass leaves took 0 to 6 ms for every arm
alike, position zero included. Part of it is the device: syncing and
letting it settle before each pass cut the penalty to 1.067x, 1.056x and
1.019x at the first three rungs, so the first arm of a rep does wait on
what the pass before it queued -- position zero is the only arm whose
predecessor is the previous rep's last, `rocksdb-nosync`, which never
flushes and hands the kernel its memtable at close. A sync without the
settle did not reproduce that, so queued writeback is part of the cause
and not all of it.

Two corrections were tried and neither is confirmed, so neither landed. A
discarded pass at the head of every rep, so that no measured arm stands
first -- the rule rep zero already follows one rung up -- removed the
penalty at a hundred thousand keys in one run (1.095x to 1.016x) and left
it at ten and thirty thousand; made a whole discarded pass rather than a
load, it removed it at thirty thousand (1.143x to 1.039x) and left it at a
hundred. The scatter is the instrument, not the fix: the same statistic on
`ycsb-C` and on `scan`, where the arms differ by an option that cannot move
either much, wandered between 0.838x and 1.059x across those runs. One run
of five reps cannot resolve a six-to-fifteen percent effect at these rungs,
so confirming any fix needs about three rows per condition, and what is
written above survives only because the rotation test compares one
quantity between two orderings rather than across conditions.

Nothing here corrects for it yet, then, and not because the interventions
are unclean so much as unmeasured: at the two smallest rungs a load is 11-37 ms
and settling the device before each pass moves it by about as much as the
bias does, so the cure is inside the error of the disease. Rotating the
roster would spread the cost rather than remove it, and with eleven arms
and five reps it cannot spread it evenly, so it would trade a bias that is
named for one that is not. Until the residual is found, read a cross-arm
ratio on `load` or `scan-lag` against `supdb` as carrying a few percent
that is the order's, and confirm any ratio worth acting on by running the
roster with the arm somewhere else.

Disk, because a run has run a host out of it. One pass builds its store and
the store is dropped when the pass ends, so the peak is one arm's, never the
ladder's or the matrix's: measured, about 3.1x the rung's records at the
small rungs and 1.9x at a hundred million, where a fixed seal size amortises
over more keys. LMDB's map is sized at three times the records but grows with
what is written, so it costs its data and not its map. The scan floor's file
is reclaimed once the floors are measured rather than held for the rung
phase, and a rung whose filesystem cannot hold it is refused before it
starts rather than dying on ENOSPC hours in.

## Rows

One file per run: `runs/<scale>/<utc>-<engine-sha7>.json`. Nothing in it is
derived; everything is what was read or measured.

The file is written when each rung finishes, not once at the end, and every
write replaces the last — so the row on disk always holds the rungs that
have completed. Two `full` runs were lost whole for want of that: both were
killed by a job timeout, the artifact step never ran, and the only survivor
was whatever the log had printed. A rung past the memory line takes days,
which is long enough that the rungs below it must not depend on it. The file is JSON; this is
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

### The machine is judged first

A class is the architecture, the CPU model, the core count, memory and
whether the host is virtualised — which does not pin the host a guest lands
on. So the floors are read before the engine's quantities: when a floor's
CI is below every row in the window, this is not the host the window was
measured on, and every quantity that moves with the machine gets **no
verdict** instead of a regression. The quantities that do not move with it
— `device_bytes_per_byte` and `bytes_on_disk_per_byte`, arithmetic on what
the engine stored — are judged as always. A floor below its window is never
itself a failure: it is the machine, and the run did not choose it.

The case this answers cost a day. A quick row failed with 121 of 1,106
quantities regressed; LMDB's scan at three hundred thousand keys, code no
engine change can touch, read a third of its window, ten of eleven
workloads had a comparator among their regressions, and the row's own wal
floor stood at 0.74 of the window's lowest. The reading took a day and the
row still could not say anything about the change beside it. The control
makes that reading the gate's, not a person's.

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

## Where the old suite went

Removed: the claims file, the results archive, the verifier, the committed
figures, the plan files, and the internal and browser experiments. Its
reasoning is in the history of the supdb-bench repository for anyone who
wants it. The browser reader's correctness checks -- three readers agreeing,
ranges exact, dictionary walks matching -- are engine tests and moved there.
The two floors became workloads.

## What stays true from before

Interleave the arms. Never compare two separate runs. Never run two timing
benchmarks at once. Two rows is not a band. Those are why a red is believed.
