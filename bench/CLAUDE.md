# Working in this repository

This directory is the benchmark suite for the engine in the directory
above, and its own cargo workspace so the engine's builds never pay for the
comparators. `DESIGN.md` is the specification and is short; read it first. This file is notes to whoever
picks the work up next with no memory of it: the rules, and the failure that
produced each one.

## What the suite is

A time series. `bench run` measures every arm over a ladder of store sizes
and writes one row under `runs/<scale>/`. A row holds raw per-rep samples
and the machine fields as read, and nothing derived: median, error bars and
the machine class are computed when `runs/` is read, so a change to any of
them recomputes history instead of stranding it. A regression is a row
whose error bars lie entirely on the worse side of the last ten rows' in the
same class. There are no claims and no expected states.

The previous suite -- 183 claims in a JSON file, adjudicated by `verify`
across three profiles with pins, arch guards and a `needs` field -- lived in
the supdb-bench repository and is in its history. Its `because` prose is
real reasoning about the engine and is worth reading when a number
surprises you. It was retired because a gate that adjudicated timing
comparisons on shared runners went red on an engine head that had not
changed, and because its largest run, at 100 MB, never left the page cache.

## Rules that stay, and why

**Interleave the arms.** Every arm in one process, round-robin within a
rep. Blocked execution confounds a comparison with anything that drifts
across the run; two runs on instances of the same nominal machine have
moved untouched comparator arms by half.

**Never compare two separate runs.** The only comparison is within a row.
Across rows, a quantity is compared only to its own history in its own
class.

**Never run two timing benchmarks at once.** Four cores measuring each
other is not a measurement. This includes your own agents.

**Two rows is not a band.** The gate says "no band yet" below
three prior rows rather than pretending.

**An A/B of two engine revisions is two runs, however you interleave
them.** There is no arm for "the other revision", so the only honest shape
is two binaries alternated within one sitting, several rounds each -- and
even that is not a result until an independent sitting says the same.
Pricing a branch here, a comparison of two full rows found a 32% load
regression and a 1.9x scan regression that were both the machine; the
alternating rerun found a 5% load regression at 300k and a 6% read
regression at 100k that the next sitting turned into no effect and a 24%
IMPROVEMENT. What survived every sitting was the large effects and the
quantities that are not rates: device bytes per byte stored came back
identical to three decimals every time, and it is what named the
mechanism; bytes on disk per byte stored, the store's files as
allocated after the load, is the other. Prefer the deterministic quantity for the WHY, demand a second
sitting for the WHETHER, and claim nothing from a margin that only one
sitting has seen.

**Nothing typed.** The window of ten is the one parameter and it is stated
once, in `DESIGN.md`. The moment a threshold appears in code, ask what
measured quantity it is standing in for.

**The checks are `scripts/check.sh`, and there is no second definition.**
Every gate this suite has broken has broken the same way: a check that was
not running, or one reporting a verdict it had not earned. The engine's
`scripts/check.sh` calls this one by group name and CI calls that.

**The tests run under `checked`; only what a measurement times runs under
`release`.** This side ran `cargo test --release` for a while after the
engine had moved off it -- so every test here, and every line of engine
code they reach, ran with the arithmetic and the invariants unchecked,
which is how a prefetch span that summed a rank and a scan's limit of
`usize::MAX` wrapped past a green engine test suite. There was no reason
for the difference; nobody had written one down, which is the shape of a
check that is not running rather than a decision. `[profile.checked]`
inherits `release` and turns both on. The `quick` group's binary stays on
`release`, because the checks cost time and time is what it measures. A
profile is worth as much as the proof it is in effect: both flags were
held to firing, a `usize::MAX + 1` that wraps silently under `release` and
panics under `checked`, and a `debug_assert!(false)` that is compiled out
under one and fails under the other.

## Shapes the bugs come in

**A gate can be red for a reason that is not the engine's.** A "not
resolvably slower" comparison gated on clearing a 5% minimum effect
recorded 0.935x on a shared runner on a head that had passed the same job
eighty-five minutes earlier. Near-zero true effects and a fixed floor are
the shape. The design answers it with bands drawn from the series rather
than a typed floor, and with the arms as the dimension rather than the
ratio between two near-identical arms.

**A guarantee is what the arm was opened with, not the row it sits in.**
`supdb-ingest` sat in the buffered row and committed durably on every
batch: no supdb arm set a sync policy, the engine's default is durable, and
the matching check let it through, because it skipped the durability axis
for the buffered group on the reasoning that a buffered metric does not
care when a commit lands -- true of a read, false of the group. So every
buffered load figure priced a durable supdb against comparators that were
not. The arm now sets the policy its row names, `Features` reports it from
the options the arm was opened with, and the check compares every axis
within a group.

**A red gate reads its own control first, and the control is the
comparators.** A quick row taken here failed with 121 of 1,106
quantities worse than every row in the window, and the row carried the
reason inside it: LMDB's scan at three hundred thousand keys, code no
engine change can touch, read 18.7M entries a second against 30.3-51.6M
in the ten priors of its class, its load 557k ops a second against
733-789k, and RocksDB's read 748k against 798k-1.05M. Ten of the eleven
workloads had a comparator among their regressions; the eleventh was
ycsb-C, which only reads. The comparators' median row-top against
window-floor was 0.92 and the default supdb arm's 0.86 on six
quantities, the fewest of any arm. The class is keyed on the CPU model,
the core count, the memory and virtualisation, which does not pin the
host a guest lands on, and the engine's own probes see the same
host-to-host spread (`docs/engine.md` on memory placement). So a row
whose comparators moved is a row about the machine: it cannot adjudicate
the change beside it, whichever way it reads, and the change is priced
within one sitting instead, and do not read its verdict as the change's.

Whether to bank it is a different question from whether to believe it. A
row from a machine that was *disturbed* -- its floors dipping while its
comparators stand -- is not history and is not banked. A row from a machine
that is what the fleet *is* now is banked, because the alternative is a
window that fossilises: three hosts in one day, on two classes, all came in
below a window measured six days earlier with every comparator below its
band too, and a gate whose control withholds every verdict is as useless as
one that cries regression. The evidence for the second case is the
comparators on independent hosts across several rows, never one row's
floors.

**A window keyed on the host hides a move that lands with a host
change.** The class carries the CPU model's string as the hypervisor
reports it, and the same fleet reports a Xeon as `Intel(R) Xeon(R)
Processor @ 2.10GHz` on one host and `Intel(R) Xeon(R) Processor` on
another with twice the L3. A quick row on the first, after eleven rows
on the second, was gated against the seven rows of its own class from a
fortnight before, and flagged the load's device bytes per byte as better
than every one of them at every rung -- a quantity that had read the
same in all eleven rows of the other class, since the engine change that
moved it (the streamed ordered load, 61b7240) landed the day before the
fleet moved, and the first row of the new class had no priors to gate it
against. A bisect by the deterministic quantity, one rep a commit, named
it in five builds, and the device's own counter said what the count had
not: two fifths fewer bytes a load where the count read three and a half
percent (`CLAUDE.md`, profiling). The rule: a deterministic quantity
flagged against a short window is read across every class the series
has before it is read as new, and a move that lands on the day the
class changes is gated by nobody, so it is looked for there.

The gate does that reading itself now, from two controls the row carries.
The comparators are the exact one: their code is untouchable by an engine
change, so one below its own window withholds the arms' verdict in that
workload and quantity. The floors are the coarse one, global and judged on
medians rather than CIs -- a floor's samples range over a factor of two
within one run, so the CI rule fired on neither of the two rows that
produced this entry. A third floor went in with them, a pointer chase
around a 64 MiB buffer, because the other two are sequential and a host
whose memory latency has moved shows in neither. Neither control ever
fails a row: the run did not choose its machine.

Held to the rows that produced the rule, in both directions: as measured
they read "host: OUT OF BAND" and give no verdict, and with their floors
raised by hand the comparator control still withholds 72 and the gate
fails on the 4 it does not explain -- which is the check that a control
is not just the gate switched off. Its price, over the twenty-six rows
banked in this class: the floors call the machine out of band on five, and
in one of those it withholds a verdict that would otherwise have been
given.

**The ratio to the comparator is not more stable than the rate, so it
cannot stand in for it.** The obvious answer to a fleet that has moved is
to gate the ratio of an arm to the comparator beside it in the same row,
since a machine both arms run on cancels. It does not. Over the
twenty-five rows banked in one class, the coefficient of variation of
supdb's rate against that of supdb over LMDB: point reads at a hundred
thousand keys 18.3% against 13.2%, scans at three hundred thousand 10.9%
against 14.6%, ycsb-E 35.7% against 38.3%, the load at three hundred
thousand 22.8% against 23.2%, ycsb-D 10.5% against 8.7%. Two better, two
worse, one level -- because the two engines do not respond to a host the
same way, so a ratio carries both their noise instead of cancelling
either. Every engine quantity in a row does have a comparator of its own
guarantee to pair with, so the coverage was there; the stability was not.
This was checked before it was built.

**A one-sided bound passes a broken measurement.** A one-sided bound,
`ratio >= 0.90`, recorded a pass on a run where the ratio came out 8.5x --
on a store where the mechanism says the policy can only lose. A row whose
value is implausibly *good* is flagged, not passed.

**A number can arrive in the wrong type.** `J::u` once wrapped a `u64` into
an `i64` on the way to JSON; a wasm `u32::MAX` arrived in JavaScript as
`-1`. Rows are serde; there is no hand-rolled JSON to get this wrong in.

**A percentile at an exact boundary can round into the next bucket.**
`hist::percentile` does parts-per-million integer arithmetic for that
reason; `99.9 / 100.0` is not `0.999` in binary.

**A cache line size that is guessed is not a measurement.** The detector
once read a `/sys` path that does not exist on macOS and defaulted to 64
on a machine whose lines are 128. `cache_line_detected` records whether it
was read, and `apple-silicon.yml` fails if it was not.

**Per-record allocation in the harness is paid by every arm equally, and
so is invisible in every ratio.** `Batch` builds a batch without allocating
per record after that was found to cost as much as an engine's whole commit
path.

**A clock on the thread that releases the others starts late.** The
threaded read and scan passes release their threads on a barrier, and
the first version started the clock when the main thread's own `wait`
returned -- on a loaded box, tens of microseconds after the workers were
running, and a hundred scans of a hundred entries on four threads at ten
thousand keys were over by then: the smoke run recorded them at billions
of entries a second, a thousand times the single-threaded figure, with
nothing raised. Each thread times itself now, from its release to its
finish, and the aggregate is the span from the first release to the
last finish. The threads also split the single-threaded pass's
operations at first, so the pass on four threads was a quarter as long
and the smallest rung's scans were tens of microseconds, a few times the
scheduler's skew; every thread runs the whole pass now, and a pass on
four threads is as long as the pass on one.

**A library's cache of handles is a store the runner did not drop.**
heed keeps every environment it opens in a registry of its own and
closes one only once `prepare_for_closing` has taken the registry's
handle; the LMDB arm never called it. A pass opens two stores at one
path -- the ordered load's, then, after dropping it and removing the
directory, a fresh one for the shuffled load and the lag sweep -- and
heed answered the second open with the first environment, still mapped
over its removed file. So LMDB's shuffled load overwrote its ordered
load's keys in place and its lag sweep scanned the ordered load's tree,
while every other arm loaded and scanned a fresh store. Nothing raised:
the key set was the same, so the byte checks held, and the rates were
plausible. The full run found it by filling the disk: every LMDB store
opened since the start was still held, 4.5 GB of removed files when the
3M rung began, which its eight reps would have tripled. Every row
before the fix carries LMDB's `load-shuffled` and `scan-lag` from that
store, and it flattered the lag sweep: rows of one engine either side
of the fix read LMDB's lag scans at 0.6-0.8x of the old figures from
30k up, its plain scan steady. The first reading of the fix compared
the new row to the window, where hosts and engine changes move the
lag points by more than that, and called the shift unresolvable; the
row of the same engine before the fix was already banked and resolved
it. `a_store_reopened_where_its_predecessor_was_is_a_fresh_store`
holds every arm to the runner's sequence -- the first store's key gone
from the second, and nothing under the path still open. The rule: an
arm's drop closes everything its open made, and a store that was
removed and is still readable is the check that says it did not.

**A runner that opens hundreds of stores keeps what each one freed.**
glibc keeps what a thread frees in that thread's arena and gives a heap
back to the system only from its top, so a store dropped at the end of
a pass stayed resident, and the stores after it opened in what was
left. At the full run's 10M rung the arenas held 13 GB taken from the
system with 1.8 GB of it in use; the store's own files were down to a
tenth of a gigabyte of page cache, and from the rung's third rep every
pass read at a tenth to a twentieth of the reps before, faulting on
every page. Nothing raised, and the rungs below it fit in what was
left. `malloc_trim(0)` in the live process took it from 12.9 GB
resident to 2.1 GB. The runner now releases freed memory after every
store a pass drops, and the pass line prints what the process holds as
the next pass opens, so a climb shows while it is happening.
`a_dropped_stores_freed_memory_is_returned_before_the_next_pass`
reproduces the shape and released 0 of its 300 MB without the release.
It came back inside one store's life: the shuffled load's merges at ten
million keys left 742 MB of freed memory in the arenas with the engine
holding 11, and under a bound of 1 GiB the lag sweep's full rewrite put
the process over the bound with the engine's own bytes at a quarter of
it (`lagmem`: anon 871 MB, in use 247, free 593 at the ten-percent point;
killed at the next). The runner returns the freed memory after the
shuffled load and after every lag point's pass now, between the timed
phases and never inside one. The release has a price, and it is the
load's: a store that opens in
memory just handed back faults its pages in where it used to be given
the passes before it, warm. In one process releasing only before the
odd reps, the default arm's ordered load read slower in all eight
pairs (0.70-0.98, about 0.9), the other supdb arms in 18 of 24, and
LMDB, which allocates next to nothing, in 9 of 16; reads, scans and the
mixes showed no direction. So the rows before the release flattered
every loading arm that allocates, and the first full row after it
failed its gate on the load alone, the engine unchanged. The rule:
what one pass leaves in the process is a variable of every pass after
it, and a run is only as interleaved as its passes are independent.
What the trim cannot return came next, under a bound: twenty to thirty
megabytes of the allocator's fragments a store, so the fifth arm of a
rep opened 0.3 GB above the first at ten million keys and the kernel
killed it at its own full rewrite under 1.25 GiB, where the same arm
runs four reps alone under the same bound. A bounded run gives each
arm's rep a process of its own now (`Plan::fresh_process`, `run`
spawning its own binary for the pass inside the group), and in core the
arms share the process as they always have, since the rows' history is
theirs. The rule's corollary: a residue a trim cannot return is a
process's, and only a process boundary gives the next pass a floor the
passes before did not raise.

**A read that never read the value priced the lookup alone.** Every
arm's point read and scan summed the lengths of the values it was lent
-- a pinned slice, a slice of the mapping, a borrow through a callback --
and read no byte of one. No caller gets a value to leave it unread, and
the omission was not even: an LMDB leaf keeps the key beside its value,
so the first touch of the value lands on lines the lookup loaded, while
supdb keeps values in blocks apart from its index, where that touch is a
miss the suite never charged. Measured outside the suite, a point read
that consumed its hundred bytes read 1.6x of LMDB where the row said 2x.
Every read and scan now copies each value into its thread's own buffer
(`consume`), and the read and scan series stepped at that commit on
every arm, comparators included. The rule: a workload does what its
caller would with the answer, or it measures something the caller never
sees. The first row banked after such a step fails its gate on the
quantities the step moved, against a window that predates it, and is
banked on its controls -- floors and comparators in band -- so the next
window carries the new definition; the engine's part of the step was
read by alternating two binaries in one sitting, never from the row.

**A host that moved on an axis no floor read.** A quick row read the
buffered arm's load at three hundred thousand keys at 0.41x of LMDB,
five slow reps of five, where the two rows before read 1.1-1.2x and
every arm's load at that rung was down a fifth to a half, with the
comparators in band and all three floors within their window. Paired an
hour later in one process with the row's own binary, the arm loaded at
its old rate and its one slow rep had the shape the profiles know: the
same fault count as the fast reps at three to four times the CPU, in
the kernel's zeroing of its fresh anonymous pages and its copy into
fresh page-cache pages. A probe of the price itself read 1.1-1.3 µs a
page while the fresh footprint fit what had recently been freed and
5-15 past it, and which price a load gets depends on what the pass
before it freed and how long ago -- the arm before it in `ARMS`, a
variable nobody chose. The write piece's size was not it: a probe
wrote the same forty megabytes in pieces of a megabyte, 64 KB and 4 KB
at the same price per page in the same state. The fourth floor reads
that axis (`page-floor`), sampled after every rep of every rung as well
as at the start because the price moves within a row, and it holds the
pages it touched to the row's end, since a floor that freed them would
hand the next pass a cheaper load than the one before had. The rule: a
control reads the axis the measurement moves on, where and when the
measurement is taken; and a floor that gives back what it measured has
measured the pass after it instead.

**A floor inside the pool the loads exhaust.** The fresh-page floor
read 1.0-1.4 µs a page in all twenty-five samples of the first row that
carried it, while every arm's load at the top rung read a third to a
half of the rows before, LMDB's included. Its thousand pages drew from
the pool of recently freed pages, and a forty-megabyte load draws past
it: probes put the pool at about what the pass before had freed, cheap
for seconds, and everything past it at the host's price. So the floor
read the price the loads were not paying. Rather than a floor sized to a
load, which would consume the pool it measured and prime the next pass
unevenly, the runner primes the pool itself before every store a pass
opens -- a touch and free of 512 bytes a key of the rung, between 16 and
512 MB -- so every load meets a pool larger than its footprint whatever
ran before it, and the first row after it banks on its controls against
a window that predates the definition. The prime's own price per page
is printed on the pass line and paired by `ab`, never banked. The rule:
a control reads the quantity at the footprint the measurement has, not
at a sample's; and when the quantity is the machine's and the
measurement cannot be made independent of it, make the measurement
independent of it and read the quantity beside it.

**As many reader threads as cores measures the scheduler.** The
threaded passes ran on two and on four threads, four being the gating
class's cores, and the four-thread cells read two to four times the
spread of the one- and two-thread cells within a row and across rows --
23-29% against 3-10% at thirty thousand keys -- for LMDB as much as for
supdb, and LMDB runs no thread of its own. Four readers on four vCPUs
share them with the harness's thread, the engines' threads, the kernel
and the host, so the cell was the scheduler's and read our ratio to LMDB
on the 30k scans from 0.65x to 2.1x across rows where the counts below
sat within a fifth. The counts derive from the class's cores now
(`run::thread_counts`): two, doubling, and one fewer than the cores as
the top, which keeps one core for everything a reader is not. The gate
names the threaded quantities by their suffix and its test derives the
counts from the same function for every core count, so a count can
never be measured and then refused. The rule: a cell that reads the
machine for both engines is the machine's, and a count of threads that
leaves nothing for the rest of the process measures the rest of the
process.

**A threaded cell at the small rungs that read the guest's vCPUs.** The
three-thread scan cells at thirty and a hundred thousand keys read 0.94x
of LMDB in the first row that carried them, 2 and 3 wins of 5, with
both engines spreading 34-59% where the one-thread cells spread 6-18%,
and the loss was taken for the engine's: the upkeep thread as a fourth
runnable thread beside three readers. Paired against the arm whose
thread backs off, the cells did not move. `SUPDB_THREAD_SPANS` was
written to take the pass apart, one line a pass with each thread's own
wall and CPU, the release and finish skew, and every other thread's run
and wait across it, and it said: the skew was 8-58 µs of a span of 0.5
to 5 ms; the engine's threads ran 0-0.4 ms of a free core; and the
three threads ran uneven by 1.5-3x in both engines, the slowest a
random index, with wall equal to CPU, so the slow thread had its vCPU
and the vCPU was slow. A control with no engine in it, three threads of
identical fixed work released by a barrier, ran the same way: a thread's
own CPU time 1.4-2x its siblings' at random, for tens of milliseconds,
and every thread at 0.6x for its first millisecond or two after its
vCPU had idled ten milliseconds, which a threaded pass's fresh threads
on vCPUs the single-threaded phase before left idle always are. The
span is the slowest thread's wall, so a pass of a few milliseconds reads
which vCPU the host shorted, for both engines alike; the same arms paired
on another host read the cells at 1.2-1.7x in both orders. The rule: a
cell whose length is within the host's own unevenness is the host's for
every engine, and the statistic a threaded pass reports has to be one
that length can carry -- the per-thread columns are read before the
span, and a threaded pass as short as a thread hand-off is a floor, not
a measurement. The threaded scan passes run `size` scans a thread now,
the threaded read pass's count, tens of milliseconds at the smallest
rung as the read passes are; the first row after it banks on its
controls as a definition step.

**A policy default that restated the engine's.** `Policy` says what an
arm differs from `supdb` by, and two of its fields -- the controller's
drop of the raw pages a form serves and its builds ahead at landing --
carried defaults of their own, off, while the engine's had moved to on:
every arm that pinned neither, the plain controller arm among them, ran
the controller without both, and every measurement of it at scale was
of a shape the engine does not ship, read for a week as the controller
reaching its form late. A field that follows the engine is an `Option`
resolved through `unwrap_or(supdb::Options::default().<field>)`, as the
seal cap and the trigger already were, and the arms whose pairs were
priced against the old shape pin it. `scripts/policy_defaults.py` runs
in `lint` and holds every field the policy maps as its own value or its
negation to the engine's default. The rule: a default stated twice is
two defaults, and an arm that pins nothing inherits the engine's rather
than restating it.

**A point whose premise the drain no longer kept.** The lag sweep's
first point ends in `sync`, "so the first point is the fully merged
store", and was, until the flush handed the merge of the pieces it
leaves to the segment work and was answered before it: the store at the
point held level-0 pieces in most reps and none in some, its scans built
forms over them (`eng` 2,000-10,000 at a point that had built nothing),
and the merge ran into the next point, where the buffered arm's
one-percent burst read one rep in six at a third of its rate for it
(`CLAUDE.md`, the sweep's first point). `Engine::settle` joins what
`sync` leaves in flight, nothing for the comparators, and the sweep calls
it at its first point outside every timed window; the pieces the
background leaves below its trigger still stand, and `lagcounters`
counts them, as it now carries the writer's holds and `lagburst` the
writer's CPU over the burst beside its wall. The first row after it
banks on its controls, as a definition step does. The rule: a pass's
comment about the store's shape is a claim the counters check, and an
engine's `sync` is what its contract says and not what a pass assumes
of it.

**A kill that ended the run, and a warmup no row held.** A bounded run
gave each pass a process of its own and still read the bound's kill of
one as the end of the run: the parent returned the kill's status, wrote
no row, and every cell the arms before it had measured went with it --
two hours of a roster at ten million keys when the fifth arm's full
rewrite was killed in its second rep, with the phase it was in, as the
engine's notes say of one such kill, not known. And every arm ran a
warmup rep under the bound, a quarter of the row, for a result the row
discards: a warmup warms a process, and a pass in a fresh one has
nothing warmed. A killed pass is a cell now (`run::PassOutcome`), named
by its arm, rung, rep and the phase it fell after, and the run goes on;
a row with one goes under `runs/killed/` with the kill's status, and a
run of fresh processes starts at rep one (`run::first_rep`). The row's
cells enter it through one function, live or replayed, and a run writes
them to a ledger as it goes (`ledger`), so a run cut by anything --
the kill, a timeout, a hand, the disk -- is taken up from its last
whole rep (`bench run --resume`). The rule: a pass that was made
independent so that it could be isolated is a checkpoint, and a run is
what its ledger holds; a result the run discards is work the run does
not do; and a failure a process boundary was built to contain is
reported by the boundary, not relayed through it.

**A run the session does not hold is the container's to end.** A roster
of two hours was started detached, with a message scheduled for its end,
and the container was restarted within minutes of the session going
idle, twice, each time taking the rep in flight; the ledger took the run
up from its last whole rep, and four hours went to learning that a
scheduled message wakes a session and keeps no machine. A long run is
held by a tracked background task for its whole length -- a watcher that
ends when the ledger's next `rep-<size>-<n>.json` appears, armed again
rep by rep, since a tracked task has a limit of its own -- and the
ledger is what makes that limit harmless. And nothing reads the scratch
logs wholesale beside one: a search over a gigabyte of old logs for a
counter's name ran two minutes beside a ten-million-key pass, on the
device the pass was faulting from, and the rep was taken again from its
start. Beside a timing run only named files are opened. And a
tracked task with nothing in the foreground did not hold a later session
either: a queue of pairings started as one was cut by a restart twenty
minutes in. What held every run after it was the run detached with
`nohup` and the session waiting on it in the foreground, ten minutes a
wait at most, one wait after another, with the queue's script skipping
the pairings whose tables were already whole. A restart of the session's
worker takes a detached run with it all the same, twice in one morning,
each time inside the second sitting of a pairing against a comparator at
ten million keys, forty minutes a pairing: `RESUME=1 scripts/pair.sh`
keeps a sitting whose table is whole and runs the other, so a restart
costs the sitting in flight and not the pairing.

**A stamp is the build's, a task has a limit, and a script a run reads is
the run's.** One roster was cut three times in a morning and none of the
three was the engine's. Its binary was built with the batch's docs still
uncommitted, so its stamp read `-dirty` and the row it was banking rung by
rung was no row of the series, which nothing said until the container's
restart cut the run and the partial row was opened; the restart had moved
the session to a host of another class, which a resume refuses, as it must.
The run after it was held by a tracked task of two hours, the most one may
have, and the roster on that host takes two and three quarters: stopped in
its last rep and taken up from the ledger. And that resume was refused by
the roster's own script, edited an hour earlier to ask the binary for its
stamp, a command the binary in flight predated. `scripts/ooc.sh` refuses a
binary stamped dirty before anything is measured now (`bench stamp`;
`OOC_DIRTY=1` for a look that will not be banked); a roster is planned as
sittings of under two hours with the ledger between them; and nothing a
run in flight reads, its binary or its scripts, is edited until it has
banked. The rule: a row's provenance is fixed at the build and checked at
the start, since a check at the end is two hours late; and a run that can
be resumed is resumed by exactly what started it.

**A device the series does not measure.** Out of core a rate is the
faults an operation takes times the device's answer to one, about 40 µs
on this fleet's disk, and the faults are the kernel's reclaim over the
engine's page touches, which a probe read identically on the disk and on
zram under the same bound, 0.77 refaults a read on each, where zram
answers a fault in about 4. So a pairing with its stores on zram reads
the disk's counts at a tenth of the wait, and its rates are nobody's:
the comparator pays the same cheap faults, and the controller, which
prices a fault by timing it, is a different controller there unless its
price is pinned. `scripts/zram.sh` runs `ab` or `pair.sh` on one for the
counts, the faults, refaults and bytes an operation takes, the `zpage_*`
counters and the group's memory. Three things keep it out of the
series, since a reader's care is not a guard: `run` refuses a RAM-backed
device under its stores (`env::ram_backed_device`, every device the
kernel files as virtual), so no row can come from one; `ab` names the
device at its table's head and foot; and every row records the device
under its stores (`MachineInfo::device`). It is a diagnostic, for the
loop between edits at three million keys where a pass is seconds, and
the figure a note quotes is the disk's. The rule: a count is the
engine's and a rate is the device's, so a device that is not the
machine's is for counts alone, and the suite refuses to bank it rather
than trusting anyone to remember.

**A dispatch whose frame grew by an arm at a time.** `open` matched
the arm's name and built the arm in place, and every constructor was
inlined into it with its `Supdb` -- a few kilobytes, the engine's
options among them -- in a slot of its own: three hundred arms made a
frame of two megabytes, the default stack of a test's thread, and the
two arms past it aborted the bench's library tests with a stack
overflow in a test that never called `open` by name. Nothing pointed
at the frame: gdb at the fault, each frame's stack pointer against its
caller's, read 1.98 MB for `open` and a few kilobytes for every other
frame. A supdb arm hands its constructor to `supdb_arm`, which is not
inlined, and `open`'s frame is a kilobyte and a half. The rule: a
match whose arms each build a large value is sized by its arms, not by
the one it takes, so the value is built behind a call the match does
not inline; and a stack overflow is read off the frames' sizes before
the stack is made larger.

**A pairing has a control too, and it is the cells its option cannot reach.**
Six pairings of one afternoon, four reps each with the arms' order
alternating, at ten million keys under the bound, read the ordered load, the
threaded reads or the first read-only mix a twentieth to a sixth apart in one
direction in every pair of four, or in three: cells that run before the
option under test can act, the same code in both arms. The arm listed second
was the slower in four of the pairings and the faster in a fifth, whichever
arm it was. So four pairs of four at a tenth was inside what a sitting did to
identical code, and one such reading, the scan pass 8% against an option that
only removes decoding from it, with the load 6% the same way in the same
table, would have been written down as the option's cost. `ab`'s sign test
answers whether the pairs agree, not whether the sitting leaned. The cells an
option cannot reach are read first, as a row's comparators are: an effect is
read against their lean in the same sitting, one no larger than it is
unresolved however many pairs agree, and the second sitting lists the arms
the other way. It was measured once with nothing to reach: an arm of
the stride form paired with the default at three million keys under
1.25 GiB, where the raw form fits and neither arm builds, installs or
converts a form file, so the two ran the same code through the row's
passes, one rep an order. A read 0.71x and 0.81x, the scrambled D 0.73x and
0.85x, the threaded reads 0.83-0.95x, and the scrambled A, B and F 1.1-1.3x,
each in both orders (`pair.sh supdb-zstrideroomconv supdb 3000000 --bound
1342177280 --passes load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled
--reps 1 --ops-div 10`, at `52c6d51`, read beside its counts: no build, no
install). A tenth or more apart in one direction in both orders is what
identical code read in five cells of twenty-two here, three tenths in the
worst; a pairing of one rep an order resolves a factor and a half and
nothing under it.

**A floor held inside the bound it runs beside.** The page floor holds
the pages it touched to the row's end, so that a floor that gave them
back would not hand the next pass a cheaper load (the entries above).
Under a bound the parent that holds them is inside the group, and every
sample is 4 MB: taken at a sitting's start and after every rep of every
rung, it left the roster's parent at 37-45 MB of anonymous memory through
the ten-million rung, where a pairing's parent holds 9. The arms of a
rep share the parent, so the row is fair between them; but the bound a
pass runs under shrinks by 4 MB a rep, and it is 30-40 MB under the bound
a pairing at the same `--bound` runs under. LMDB at ten million keys under
1.25 GiB sits at the edge of fitting: paired against itself under the
bound less 32 MiB, its faults matched the roster's to two figures -- 0.058
a read against the roster's 0.054-0.060 and 0.034 at the full bound, 0.143
a scan on three threads against 0.112-0.150 and 0.039 -- and its
three-thread scan rate fell from 45 to 25-26 million entries a second,
while the supdb arm faulted in neither harness (`sh scripts/pair.sh
lmdb-nordahead lmdb-nordahead 10000000 --bound 1308622848 --passes
load,read,read-threaded,scan,scan-threaded,mixes --ops-div 10 --reps 2`
against the same at `--bound 1342177280`, at `03fdbe9`). The bound is the
pass's now: each pass's process joins a group of its own with the limit
on it before it runs anything, the run's process is in none of them, and
each pass records what its group held at its start (`bound-start`), so
the arms' equal memory is a recorded fact and not an assumption
(`bound::Bound::pass`, `run::audit_note`). LMDB against itself in a
pairing and LMDB in the roster at ten million keys under 1.25 GiB then read
the same faults to two figures, 0.067 a scan and 0.029 a read on one
thread and 0.023 a scan on three, in both positions of the pairing and in
the roster, every pass beginning with a megabyte or so charged to its
group (`bench ab --arms lmdb-nordahead,lmdb-nordahead --size 10000000
--bound 1342177280 --ops-div 10 --reps 1` beside `sh scripts/ooc.sh
1342177280`, at `c9599c2`); the roster runs a second time
under 1 GiB at ten million keys, where LMDB is clearly out of core, so a
cell at the edge is read beside one that is not. The first row after it
is a definition step, banked on its controls. The rule: whatever the
harness holds is kept out of the bound the measured process runs under,
by the structure and not by holding little, and the bound each pass got
is read from the kernel at the pass's start; and a quantity that grows
rep by rep in the parent is a drift in every cell that can feel it.

**A bound under a bound, and a sitting that was the machine's.** A
roster cut at its first rung read the scan pass after the ten-percent
burst at three million keys under 1 GiB at 0.71x of LMDB, three reps of
three, where the row before had it at twice; a pairing the same evening
read it slow in twelve reps of twelve in both of its arms, the new
default and the shape before it, and the per-rep history of the cell
across the day's pairings fell by commit, so two commits were suspected
in turn. The pass's profile cut to its window (`perf record -e cpu-clock
-c 25000 -k CLOCK_MONOTONIC`, the samples between the pass's `lagmark`
lines, by thread and symbol) put the whole excess in the sparse walk's
reads of the partition's mapped records and the copies of their values,
with the thread's CPU equal to its wall, no fault of either kind in the
group, no byte read at the device and every counter of the engine the
same as a fast pass's. Then the guest was restarted, and the same binary
read the same pass at a third of its time, 1.8x and 2.1x of LMDB in
eight pairs of eight (`SUPDB_SCAN_OPS=1 sh scripts/pair.sh supdb
lmdb-nordahead 3000000 --bound 1073741824 --reps 4 --ops-div 10 --passes
load,lag`, at `8666eb9`). The shape has not come back, and what it was
is not known: a cost of memory already mapped and already read, on the
machine as it stood that evening. What it was not, each by a count: a
read from the device, a fault, the group's limit (the group stood a third
under it), either commit, or the first touch of pages read back, which a
cold start made on purpose leaves that pass as it was
(`SUPDB_LAG_SQUEEZE_MB=300 SUPDB_LAG_REPEAT=1 SUPDB_SCAN_OPS=1 bench ab
--arms supdb,supdb --size 3000000 --bound 1073741824 --passes load,lag
--reps 2 --ops-div 10`). The hunt found two things that were real. A
page the cache dropped and read back costs its first reader 0.3 to 1.5 µs
below the guest, the price moving fivefold with the machine's state
(`examples/firsttouch.rs`: 1.2-1.5 before the restart, 0.3-0.5 after),
which is the clean store's pass up to a third longer in the reps whose
load pushed raw files out and nothing at the passes after it. And the pass's
group is made under the group the run was started in, which on this
host is the tool's own, limited at 13.35 GiB and hierarchical: after a
day's builds its page cache stood at the limit, and a group at its limit
takes pages from every group under it at each charge, however far the
pass is under its own bound. The first sweep of each of two sittings
begun so had 37 and 69 MB of its store's files pushed out by its load's
end where the same sweep under an emptied parent had none (`zrewarm raw
asked`, under
`SUPDB_ZPAGES_TRACE=1`, the parent filled by reading the build
directories twice so that its pages are on the active list); the pass's
own charges then evict a gigabyte of the parent's cache and the passes
after it have their room, so it is the first pass of a sitting that was
squeezed, in every pairing started behind a build. Each pass records the
least room a limited group above it had at its start now (`bound-start
room_above_bytes`, `bound::room_above_at`), the run reports a pass that
began with less than its bound (`run::room_note`), and `scripts/room.sh`
drops the page cache before a roster or a bounded sitting and reads the
run's binary back, since the drop takes the binary's pages with the
rest and the first pass would fault its own code in under its bound.
The record's own first roster died three seconds in, behind a green
gate: the new quantity had no direction in the gate's list, the one push
that refuses such a quantity (`Samples::push`) is on `run`'s path and not
on `ab`'s, and no test had sent a bounded pass's start through it. One
does now (`a_bounded_passes_start_enters_the_row`), and a bounded `run`
at one small rung is the ten-second smoke before a roster of three hours
(`bench run --scale ooc --bound 268435456 --top 30000 --bottom 30000
--reps 1 --ops-div 10 --passes load,read,lag --arms supdb,lmdb-nordahead
--out DIR`).
The probes stay for the next sitting that reads so: `SUPDB_LAG_REPEAT`
makes a point's same scans again after its pass, which tells a cost of
what the scans were first to touch from a cost of the walk;
`SUPDB_LAG_SQUEEZE_MB` and `SUPDB_LAG_TOUCH` make a cold start and take
its first touches back out; and `lagops` carries the machine's TLB
shootdowns and function-call interrupts over the pass. The lag sweep
draws the same scan keys at every point, so a point's pass reads the
records the points before it read: a slow pass after a fast one over
the same keys is not those pages' first touch, which the hunt took
hours to notice. The rules: a cell lost in both arms of a pairing is the
sitting's before it is either arm's, and a cell's history by commit is
also its history by hour, so the baseline's binary is run again in the
sitting at hand before a commit is suspected; a bound made inside
another group's is two bounds, and the room above it is read from the
kernel at the pass's start as the pass's own charge is; and a step that
makes a measurement's precondition puts back what it took that the
measurement needs.

**A guarantee matched on one kind of crash.** The buffered group was
matched on the sync: no arm in it reaches the device at a commit. The
recovery workloads' first run held each arm's reopened store to every
batch its killed writer had been acknowledged, on the reading that an
unsynced commit is in the kernel and outlives its process, which LMDB's
and RocksDB's documentation says of theirs and the engine's says of its
WAL. `supdb-ingest` failed it at the first rung: the ordered load
bypasses the WAL, and a direct run's records go to the file a megabyte
piece at a time, so the commits behind the last whole piece were in the
dead process's memory -- two batches of ten at ten thousand keys, eight
of a hundred at a hundred thousand, the same count every rep, and none
after the shuffled load, which the WAL takes. The durable arm, whose
every commit syncs the run, held every batch. So a buffered arm's loss
is a quantity of the row (`unrecovered_batches`) and a durable arm's is
a failed pass. The engine writes every commit of a run to its file now,
the quantity reads zero for every arm, and the test of the workload
holds it there (`tests/recover.rs`); the rows banked before that read
the count the paragraph gives. The rule: a guarantee has more than one axis -- what a machine's
crash may take and what a process's may -- and a group is matched on
each one a user could mean, by a workload that kills something, since
an axis nobody tested is matched by assumption.

**A device filed as virtual that was a disk's.** `run` refuses a
RAM-backed device under its stores, and asked whether the device's entry
in sysfs lies under the kernel's virtual devices, where zram, brd and
loop devices are filed. A device-mapper volume and an md array are filed
there too, and are the disks under them. A hosted x86 runner whose work
disk was a device-mapper volume (`dm-0`) was refused as RAM-backed, and
`tests/fresh_process.rs` failed there with the engine and the suite
unchanged, in a job the ARM runner beside it passed. A mapped volume is
judged by the devices its `slaves` directory names now, each link
resolved before it is read, since a slave beside its volume is linked as
`../../zram0` and names no directory on the way
(`env::virtual_block_device`); the test builds sysfs's tree, a volume
over a partition, over zram, over another volume and over both. The
rule: a class read off where the kernel files a thing is held to
everything else filed there, so the directory's other members are listed
before its name is trusted; and a refusal that guards the series is run
on the hosts the series' CI runs on, where the first one to differ finds
it.

**A workflow that never runs can be syntactically invalid for months.**
Both self-hosted pickup watchdogs arrived with a block of an older draft
pasted after their `exit 1`. `scripts/workflows.sh` parses every `run:`
block and rejects any `${{ }}` inside one; it runs in `lint`. It refuses
`ubuntu-latest` as a job's runner too. That label moved to Ubuntu 26.04
between two runs of one afternoon, where the libclang lookup found
nothing (it wanted a -dev package's name for the library then, and
looked in the highest LLVM directory alone; it takes the image's own
now) and `qemu-user-static` is not a package, and three jobs of `ci`
failed in their set-up on a tree that had not changed; `ci` was given
its image by name that day, and the five workflows that run by hand or
after a merge kept the label for another day, each to fail the same way
the first time it was wanted. A workflow nobody runs is fixed with the
ones that ran, by a rule the lint holds, and not when it is next needed.
And it holds a cache step's `workspace -> target` to a target that is a
workspace's own directory: every workflow said `bench -> bench/target`,
the action takes the target as a path inside the workspace, and the step
cached a directory that does not exist while each job built RocksDB from
its sources twice (the engine's `CLAUDE.md`, running the checks). A
setting whose effect nothing reads is held to the thing it names existing.
And it holds every cache step of `bench -> target` alone to one shared
key, and every other step off it: keyed by their jobs' names, the jobs
that build the suite's release binary each kept a copy of one build,
four from one round of the workflows that run by hand, and the
repository passed the ten gigabytes GitHub allows it (the same
paragraph of the engine's notes).

**A generator more regular than the thing it stood for.** The first
ids were one to a stratum of the whole 64-bit space, key `n` a random
point of the `n`th of as many strata as keys: ascending with the index,
no two alike, and nothing like random ids to a structure that guesses a
key's place from its value, since key `n` lies within one stride of `n`
strides in and a straight line through the first id and the last puts
every key within a key of its place. The search the sparse stride form
was to be priced on guesses by that line, and over those ids its guess
was never off by more than two of its samples, which the probe of the
search said before the form was built on it: a search that never has to
look is not the one random ids get. The ids are dealt to cells by a hash of the key's number now, so a
stretch of the space holds as many as independent draws would put there,
some cells half again their share and some none, and a test holds the
cells' counts to that spread (`workload::Ids`,
`the_ids_are_spread_as_independent_draws_are`). The rule: a synthetic
input is held to the statistic the code under test is priced by -- here
the count of keys in a stretch, at every scale -- and not to looking
random, which a stratified sample does to the eye.

**A loop of independent loads times the core's width, not a load.** The
probe of a key's search (`examples/keysearch.rs`) first drew its keys
ahead and read them in a loop, and read a dense stride's read at 39 ns
over ten million keys, where a miss alone is twice that: the reads share
nothing, so the core had ten of them in flight at once and the loop's
time a read was a tenth of one. A point read in a store is the next thing
its caller does with the last one's answer, so the probe feeds each read's
result into the next key's choice, which makes them one at a time. A
probe of a read that misses is serial by a data dependency, or it
measures how many misses the core overlaps.

**An arm with no stronger promise to configure is a guarantee of its
own.** FASTER has no setting under which a write outlives its process
before a checkpoint, and the arm was first filed with the buffered group,
whose every commit is in the kernel. The matching check refused it beside
RocksDB, on `reopen_for_write`, and the first fix made the check skip that
axis and `ordered_scan` for everybody, which is the shape of the entry
above: an axis dropped so that an arm would fit a group it is not in. The
arm is `Guarantee::Memory` now, alone there, and the check is as it was.
Its cells beside the buffered group's are a reader's comparison, made
with the difference said, never the suite's.

**A cell an arm cannot have is no sample, not a NaN one.** A phase a
pass leaves out leaves a NaN where its rate would be, which was every
phase `bench ab --passes` left out and nothing in a row, since a row was
always every phase of every arm. `faster` has no ordered scan, its scan
rate went into the row as five NaNs, serde wrote them as null, and the
first quick row with the arm in it ran its eighteen minutes, was banked,
and was then refused whole by `Row::read` at the gate: one null among ten
thousand lines. A sample enters a row in one place (`Samples::push`), and
that place drops a NaN now, so an arm that cannot run a workload and a row
of some passes (`bench run --passes`) both write rows the gate and the
figures read. The check that found it was the quick group, the only one
that reads a row it has just written.

**A comparator's own benchmark can be the broken one.** FASTER's C++
benchmark at the pinned commit tests `result == Status::Ok` after an
upsert (`cc/benchmark-dir/benchmark.h`, since a commit of February 2025)
and so logs a line for every upsert that succeeds: its half-reads run on
one thread came in at an eighth of what the same store does with the test
turned round, and the first reading of it was that the suite's shim was
four times faster than the store's own tool. A comparator held to its own
tool is held to the tool as somebody last ran it; read the loop before
the rate. The upstream repository is no longer maintained, which is why
the pin is its last commit and why nobody else will have met this.

**A verdict that was its pipe's, and a yardstick that did less than the
arm.** `scripts/comparators.sh` printed its table through `tee`, and a
pipe's status is its last command's: the table's script answered a cell
under the margin with 1 and the job passed, for as long as the check had
run. With the exit the table's, the cells were under 0.7x in four runs of
five, hosted and here, and all but one of the cells were LMDB's scans,
near 0.8x of the tool where RocksDB's scans read above db_bench's. The
loops said why. The harness copies every value an arm
reads into its sink, db_bench's seekrandom copies its iterator's value
at every step ("to make sure we read them"), and the C program that
stands in for LMDB's tool read one byte of each. Copying the value, the
program scans a tenth to a fifth slower at the rungs to a hundred
thousand keys, eleven alternated runs of each build, and the scan cells
read 0.92-1.33x of it in the two runs after; a cursor opened by every
scan, as the arm's range iterator opens one, moved nothing. The copy is
the C library's, called through a pointer the compiler has to load:
written as a plain `memcpy` of a run-time length the compiler made it a
`rep movs` in place, and that build scanned at a third to a half of the
rate, a yardstick wrong the other way. The rules: a script's verdict is its own
exit, and a red input is run through the job once to see it fail; a
tool written here to stand beside an arm is read against the harness's
loop line by line, since one that does less work reads as an arm driven
badly; and a copy in a benchmark is the routine the thing measured
calls, read in the disassembly, never whatever the compiler makes of
the source line. The job keeps its db_bench by a restore step and a
save step of its own, since the one cache action that does both saves
only where the job passed, and a job that fails on its table had built
the tool for nothing.

## One repository

The suite was a separate repository carrying the engine as a submodule for
as long as it was large: claims, results, plan files, browser tests and a
launcher. Once it was ten files and a time series, the second repository
cost more than it guarded -- a paired pull request for every change that
touched both, a submodule pointer and a branch override so CI tested the
right engine, and the engine's release profile silently ignored under the
other workspace. Now the pull request's own commit is the engine under test
and the row's one `sha` names both.

Building RocksDB runs bindgen, which needs libclang, and every workflow
calls `scripts/libclang.sh` for it -- one definition. The first CI run that
built RocksDB at all failed on both Linux (only a versioned `.so` on the
image) and macOS (dyld could not find `@rpath/libclang.dylib` at run time),
and each got a workaround: a -dev package installed with apt in every
Linux job, for the unversioned name the linker wants, and on macOS an
rpath in `RUSTFLAGS`, since SIP strips `DYLD_*` before a workflow step's
shell starts. Both were the price of one line of `Cargo.toml`: the
`rocksdb` crate loads libclang when its build script runs by default, the
suite had turned the crate's defaults off for the compression libraries,
and the build script linked libclang instead. With `bindgen-runtime` named
again the library is opened by its path, the runtime library an image
already has will do, and the script checks for it, with clang's own
headers, and installs nothing where it is there: the apt step had been
twenty seconds of every Linux job and, on a day the mirror was slow, five
minutes, as long as the build the cache had just saved. Two libclang
packages are not one: without clang's headers beside the library bindgen
ends at `'stdbool.h' file not found`, so the script asks for both. FASTER
is the other comparator a host has to be readied for, and
`scripts/faster.sh` is its one definition, called wherever
`scripts/libclang.sh` is: it is not a crate, so the script fetches its
sources at the commit `vendor/faster/PIN` names into a directory git
ignores and installs the three libraries they link, by the engine's
`scripts/apt.sh` and so with the package lists the host has, and `build.rs` fails
the build where they are missing or another commit's. The arm exists on
Linux x86-64 alone (`cfg(faster)`, set by `build.rs`), so nothing on
macOS or ARM builds or needs it. Cargo takes profiles from the root of the workspace being
built, which here is this directory, so the engine's release profile is
repeated in `Cargo.toml` and cargo warns on every build that it is ignoring
the engine's -- the warning is expected, the repetition is what keeps the
measured engine the shipped one.

**A picture in a document is a figure that cannot go stale.** The README
carries three of the suite's figures, and the whole set is committed under
`docs/figures/`, which breaks the rule that a figure is drawn and never
stored -- unless the stored copy is held to its source the way a formatted
file is held to the formatter. `bench/scripts/figures.sh --check` draws the
set from `runs/` again and fails where the committed set differs, in the
`figures` group every check run and CI run takes; a banked row's commit
redraws the set, and a renderer change redraws every figure, which is the
diff that reviews it. The renderer is a function of the rows alone, no
clock and no host in it but what the row records, and its test draws the
same rows twice and holds the bytes equal, since a check that compares a
drawing to a redrawing is only as good as the drawing's determinism. The
rule: a derived file may be committed when a check regenerates it and
fails on a difference, and never otherwise.

**Rows from the hosted runners.** `bank-row.yml` runs the quick ladder on
each hosted runner after a merge to `main` and proposes the rows as one
pull request, the figures redrawn and each row's gate verdict in the body;
merging it is a person's decision, as `quiet-bench.yml`'s rule already
says, and the workflow never runs on a pull request or a self-hosted
runner. The runner label is not the class: the three most recent quick
jobs on `ubuntu-latest` ran on an EPYC 9V45, an EPYC 7763 and an EPYC
9V74, three classes under one label, so a band for a hosted class forms
as its own rows arrive, slowly for that label and faster for the ARM and
macOS runners. A hosted row is never read against a Firecracker row, which
the class key forbids anyway, and no default changes on a hosted number
alone: the runners are shared machines whose model varies between runs,
and the row's floors judge the machine before its cells are read. A
bounded row can be taken there too, by hand (`ooc-hosted.yml`): the
runners are cgroup v2, where a process cannot bound itself from inside a
leaf -- a group with processes in it may not hand the memory controller to
a child -- so the job starts the run inside a transient systemd service
with the limit on it and no swap, and the suite attaches to the group it
finds itself in, holding its `memory.max` to the `--bound` it was asked
for and refusing a `max` or any other number (`bound::Bound::attach_at`).
That path cannot run on this host, which is v1 with nothing delegated on
its unified mount, so it is held by its parsers' tests and an attach
against a directory of plain files, and the job's first run is its first
exercise. A hosted bounded row is a class of its own, the bound in it, and
is read against nothing from here.
