# Stage 4: the controller

The design is `scratch/design/zpages.md`, sections 4 (form selection), 5
(lifecycle), 6 (the option's sites, the arms, the tests, the
instrumentation) and 7.4-7.5 (what it must read under the bound and in
memory). This file is the plan cut into pieces, each gated and committed
on its own, and the record of what each found. Stage 3 closed with the
form priced on every read cell: it wins every one out of core and loses
every one in core (`stage3.md`), so the whole of the form's value is in
entering it past memory and leaving it in core, which is what the
controller is.

## What exists, and what does not

The selector exists: `src/db/zselect.rs`, `ZSelect::step(&Input) ->
Decision`, a pure function of its state and an explicit input -- the
model (`Model`: `phi_raw`, `phi_z`, the refault rate, the budget), the
sampler's reading (`Sampler`: costs, `c_build`, declined, generation),
the census (`Census`: the read and scan windows' operations and faults,
the mixed windows, the group's refaults, the threads, the measured
costs), the builder's report (`Build`: coverage, installed, superseded,
failed) and E2's footprint -- with the phases FITS, SUSPECT, building,
COMPACT and the trial, the events, the counts, the model functions
(`model_raw` by Che's approximation over the hash and record populations,
`model_z`, `ski_rental_price`) and every constant named for the quantity
it stands for; its replays and the two hundred operator flips hold it.
Nothing of the glue exists: no `ZPages::Auto`, no gate word, no cgroup
reader, no sampler of the partitions, no census in the handles' slots,
no tick that steps the selector, no `ZInject`, no auto arm, no trace.
`Blob::z_source_facts` gives a partition's footprint facts; `Always`
builds at every landing and attaches at open with no gate at all.

## The pieces

1. **The gate and the option.** `ZPages::Auto` (the default stays `Off`;
   the flip is stage 6 behind 7.8(b)); `Shared.zgate`, one aligned word,
   `GATE_RAW` / `GATE_CENSUS` / `GATE_Z`, pinned at 2 by `Always` and 0 by
   `Off`; every dispatch to the form (`part_read_z`, `part_count_z`,
   `part_scan_z`, the seek, the cut, the values, the cursor) takes the
   form only when the gate says `GATE_Z`, so an attached cell under the
   trial is served raw by the gate; `check_options` refuses nothing for
   `Auto` and runs it inert (`auto_inert`) where the segment work has no
   thread, the writer is unpinned or the platform is not Linux; the
   budget reader (cgroup v1 `memory.stat` through `/proc/self/cgroup` and
   `/proc/self/mountinfo`, v2 `memory.max`/`memory.high`, else `MemTotal`;
   `anon`; the margin), ported from `bench/src/bound.rs` and never shared
   with it; the footprint `H`, `R` kept on each `Seg` from
   `z_source_facts`; `Maint::z_step` stepping the selector every 100 ms
   with S1 once a second under `Auto` and acting on `Decision::gate`;
   `Options::zpages_inject` (`ZInject`: the budget, the refault rate and
   the clock a test chooses, per store); `zpage_counts` gains the gate,
   the phase and `auto_inert`. Tests: `an_auto_store_that_fits_builds_nothing`
   (in memory: gate 0, no build, no file, no thread beyond the segment
   work, the directory `Off`'s); the degrade test; a budget under the
   footprint reaches SUSPECT in two ticks and a budget over it relaxes in
   five, through the injected clock; `Always` and `Off` read as they did
   (every stage-3 test). The in-memory price: one relaxed load and a
   branch on the point read, counted by callgrind over the exact name.
2. **The census.** The handle's window: `zdown`, the faults by
   `getrusage(RUSAGE_THREAD)` at open and close, the thread's CPU and
   wall, the `gettid` token, the class (read, scan, mixed), whether a
   commit fell inside; opened only when the gate says census, closed at
   `N` operations or 250 ms, `N` resized to a 100 ms window; the sums on
   the slot's line, taken by the tick into `Census` with the costs
   (`t_f` from commit-free windows, `t_hit` the least CPU a raw read
   took, `t_z` in windows the form served) and the group's refaults from
   `memory.stat`. `ZInject` injects the faults a window reads so a test
   needs no bound. Tests: windows with commits count for `phi` and not
   for `t_f`; a scan-only handle reaches evidence; mixed windows are
   counted and dropped; a handle that moved threads yields nothing; FITS
   runs no countdown.
3. **The sampler, the build under the controller, install, retire, the
   trial.** Split in two sittings once piece 2 was in: 3a is the sampler
   on the build thread (`ZJob::Sample`: thirty-two groups a partition
   through `fill_group`, each fill and each decode-plus-walk timed on the
   thread's clock, `W_z` from the pages estimate with the fence and the
   filter, the build CPU from the fills' cost over the keys, the attach's
   cost where the form files are on disk, declined where the pages would
   not be fewer bytes than the records), `Input::generation` as a digest
   of the live partitions and their cells, `Decision::sample` and
   `Decision::build` driving `z_next`/`z_start` as `Always` does, the
   `Build` report (coverage by the partitions' footprint bytes, the
   collected builds' fates), `Model::phi_z` from `model_z` over the
   sampler's `W_z`, form files on disk kept dormant under `Auto` and
   attached when the form is wanted, `Decision::retire` through
   `z_retire_cell` and the spares; with the tests the model's traces
   reach without a footprint: builds when the measure says blocked,
   serves raw until whole, returns to raw when the limit is lifted and
   gives the disk back, a skewed workload that fits never builds, the
   price is paid before a build, a cancelled build counts as a switch, a
   cold start is not a capacity miss, two stores do not share a measure,
   a store far past the cache still builds, a neighbour's pressure opens
   the build clause, builds suspend after four of eight are superseded.
   3b is E2's footprint (the handle's bitmap of the compact pages it
   touched, posted at the window's close, the raw bound from the store's
   own quantities) and the trial through the gate, with the trial's,
   the square wave's and E3's tests, and the take-back's file set. `ZFile::sample` over thirty-two groups of a partition: the
   compressed size (so `W_z` and `phi_z`), the decode-plus-walk cost
   (`t_z`), the build CPU estimate (`c_build` through
   `ski_rental_price`), declined where the form would not pay;
   `Decision::sample` asks for it and `generation` changes it (a
   partition made, rewritten or retired; a form file attached or
   retired); `Decision::build` lets `z_step` build uncovered partitions,
   one at a time as `Always` does; `Build` reports coverage by bytes and
   the collected builds' fates; the cells install first and the gate
   last; form files on disk attach lazily when the form is wanted, with
   the attach as `c_build`; `Decision::retire` takes every cell through
   the epoch to `retired_z`, the file to a dead name and the idle sweep;
   E2's per-handle bitmap of touched pages and the footprint bound; the
   trial through the gate. Tests: the selector set of 6.3, each trace
   fed to the engine through `ZInject` and the injected clock --
   `auto_builds_when_the_measure_says_blocked_and_the_limit_is_below_the_footprint`,
   `auto_serves_raw_until_the_form_file_is_whole`,
   `auto_returns_to_raw_when_the_limit_is_lifted_and_gives_the_disk_back`,
   `a_skewed_workload_that_fits_never_builds`,
   `a_square_wave_switches_at_most_once_per_dwell`,
   `the_trial_judges_the_refilled_state`,
   `two_stores_in_one_process_do_not_share_a_measure`,
   `a_cold_start_is_not_a_capacity_miss`,
   `the_ski_rental_price_is_paid_before_a_build`,
   `a_cancelled_build_counts_as_a_switch`, `a_form_that_stops_paying_is_left`,
   `a_store_far_past_the_cache_still_builds`,
   `a_neighbours_pressure_opens_the_build_clause`,
   `builds_suspend_after_four_of_eight_are_superseded`,
   `a_take_back_returns_the_disk_and_the_file_set`.
4. **The arms, the counters, the trace and the measurements.**
   `supdb-autozpages` and `supdb-nocacheautozpages`; `rung_at_end`,
   `switches`, `trials`, `trials_back`, `mixed_windows`, `auto_inert`,
   `build_suspended` in the rows; `SUPDB_ZPAGES_TRACE=1`, a line a
   decision; then 7.4 read under the bound: at three million keys FITS
   with nothing built, at ten million SUSPECT within two ticks, the price
   paid in about nine seconds on one thread, the build and the warm, then
   COMPACT, the read cells a staircase and the cells after the entry read
   against the forced arm; a restart with form files on disk reaching
   COMPACT in about a second; the limit lifted to four gibibytes
   relaxing to FITS and the directory equal to a store that never built;
   and 7.5 in memory: the quick row's deterministic quantities identical
   to three decimals, the rates level against the arm run against
   itself, no extra thread in `SUPDB_THREAD_SPANS`.

Each piece: its tests under `checked`, lint, the zpages binary's whole
run, the fuzz over its seeds with the form forced, the lifecycle stress,
and a pairing where the piece touches a measured path. The block cache's
walks stay raw through all of it: the controller decides the form, and
whether the cache's builds read through it is a question for after the
controller reads right.

## Where it stands

The rename of the form's file from "sidecar" to "form file" (`ZFile`,
`zfile.rs`, `form_files`, the counters, the docs and the notes) went in
first so the controller is written in the right vocabulary.

Piece 1 is in: `ZPages::Auto` (the default stays `Off`); `Shared.zgate`
(`ZGate`, one aligned word, pinned by `Off` and `Always`, moved by the
tick) and `Reader::z_on`, asked first by every dispatch to the form --
`part_read_z`, `part_count_z`, `part_scan_z`, `part_seek_z`,
`part_cut_z`, `part_values_z`, `pwalk_at`, `pwalk_seek` and the overlay
walk's cursor branch -- so a cell under the raw gate is served raw and
counted nothing; `z_auto_inert` (the segment work without a thread, an
unpinned writer, not Linux) and `ZCounts::auto_inert`; the platform's
reading (`zfile::read_budget`: v1's group through `/proc/self/cgroup`
and `/proc/mounts` and its `memory.stat`, else v2's least limit over the
path, else `MemTotal` and `RssAnon`; `Budget::of` with the margin) with
the parsers pure in `zselect` and their texts tested, this host's hybrid
shape among them; `Seg::foot`; `Maint::z_tick` every census interval
with the model every S1 period, `z_model`, `z_apply` (the gate stored
last after the phase and the counts; `SUPDB_ZPAGES_TRACE=1` a line a
model reading or event); `ZInject` (`Options::zpages_inject`: the
budget, the refault rate, the clock); the gate, the phase, `auto_inert`
and the selector's counts in `ZCounts` and the `zpage_*` rows; `Db::zgate_set`
for a test. Tests: `an_auto_store_that_fits_builds_nothing` (gate 0,
FITS, no build started, no form file, the directory `Off`'s file for
file, the tick counted), the degrade test (inline, unpinned: inert and
counted; `Always` unpinned refused), the model moving the rung through
the injected budget and clock (SUSPECT on the second hot tick with the
gate at the census and every read raw; FITS after five calm ticks), and
the gate deciding the form an attached cell serves (a store under
`Always`, the gate set to 1 and 0 by hand: nothing counted through the
form; back at 2: served). Two mutants held to firing: the point read
ignoring the gate, and the tick never running. Two things learned on the
way: the selector's purity test (no clock, thread, file or engine type
in `zselect.rs`) refused the platform's reading and the seam, which is
right -- the pure parsers stay and the reads live beside the file they
are for; and a test that counted the process's threads for a build
thread read other tests' stores in the same binary, so it counts the
store's own `build_starts`. The in-memory price, counted by callgrind
over twenty thousand gets of a ten-thousand-key store under `Off`
(`bench/examples/read_only.rs` built from the commit before and after,
the toggle on `supdb_bench::engines::supdb_get`, the exact name):
16,609,943 instructions against 16,609,938, 830 a get both ways; the
gate's load stands where the cell's load stood, so the design's three
instructions are none.

Piece 2 is in: the window on the handle (`ZWindow`, `Reader::zcensus_begin`
at the three operations' start after `note_read`, `zcensus_end` in `leave`
at depth zero, `zwindow_open`, `zwindow_close`), opened under any gate
but the raw one for a counted handle of a store under `Auto` where the
controller runs (`Reader::zcensus`, decided at the claim), closed at `n`
operations or three ticks, resized toward a hundred milliseconds
(`ZWINDOW_MIN/MAX/TARGET/TICKS`); the thread's major faults by
`getrusage(RUSAGE_THREAD)` (`zfile::thread_major_faults`, the constant
named locally since libc 0.2.189 does not export it for this target, held
equal to `/proc/thread-self/stat`'s field by a unit test), the CPU clock
the engine already reads, the operations' busy wall, the thread identity
by `ThreadId` (no syscall) and the commit position as `note_read` reads
it; fifteen sums on the slot (three lines, the size assertion moved to
384), the mins as `f64` bits; the tick's `Maint::z_census` (the sums
differenced, the threads by slots whose windows moved, the refaults from
the platform's counter each tick while the census is on or from the
seam's rate, the price of a fault as the cumulative ratio, the mins over
the slots), `ZEvents::ticks` a handle's window counts against,
`census_costs` and `sel_costs`; `ZInject::add_faults`, `set_fault_cost`;
`ZSelect::measured` and `threads`; twenty `census_*` and `sel_*` fields in
`ZCounts` and as many `zpage_*` rows. Tests: the price of a fault from
commit-free windows only, to the nanosecond, and the rate from all; a
scan-only handle; mixed windows dropped and counted; a handle moved
between threads; a handle idle between its calls; FITS and `Always` run
no window. Six mutants held to firing: the thread check, the commit
check, the countdown under FITS, a mixed window kept, the handle's wall
for the busy wall, the scan class lost. Learned on the way: the design's
window wall was the handle's, which reads a caller's idle between calls
as fault time, so the busy wall is the operations' own, at a clock read
per operation in the census phases and none in FITS; the selector's
`threads` defaults to one, so no test asserts zero; a tight loop's second
window is a hundred thousand operations after the resize, so the tests
close windows through the ticks, and the read that closes one is an
operation of the next; `Maint` reaches `shared` through its deref to the
handle, so a borrow of the reader table holds the whole of `self` and the
tick clones the `Arc` first. The in-memory price, counted by callgrind in one sitting, three runs of each probe: 828 instructions a get for piece 1's binary (which read 830 in its own sitting: two a get between sittings, under half a get within one) against 836 for this piece's, eight a get, four in `leave` and four in the read's frame -- the two loads and branches of the handle's flag, with what the compiler reloads around them; the design budgeted a load and a branch at the start alone, and the end hook is the busy wall's, which the design did not have. About one percent of the read's instructions, to be read in the rate by the alternation of the two binaries when the arms exist (piece 4), which can only say it is not more.

Piece 3a is in: `zfile::sample` (`Sample`, `SAMPLE_GROUPS`; a unit test
over six thousand of the suite's records, the floor's decline among it),
the sample job (`ZSampling`, `Maint::z_sample_start`/`z_sample_collect`:
the reading summed into `zselect::Sampler` with `Costs`, `c_build` as the
ski-rental price of the fills' CPU over the keys or the attach's reads at
the price of a fault, `declined` where every partition is, the generation
it was taken at; `z_sample_size` kept for `Model::phi_z` and
`phi_z_scan`), `z_generation` (an FNV digest of the live partitions by
name with their cells and declines), `z_report_take` (the collect's fates
by `z_report`, the coverage by `Seg::foot` bytes), `z_drive` (retire
through `z_retire_cell` and the spares; collect; attach every dormant
file and start one build where builds are wanted, cancel a build in
flight where they are not; a sample job where a reading is wanted),
`z_next` building under `z_want_build` as under a settle, the scan keys
in the census windows (`zcskeys`, `z_scan_len`), the counts and rows,
`ZSelect::c_build`. Tests (`tests/zpages.rs`, through the seam with the
price of a fault at a millisecond, since an unoptimized decode costs
sixty microseconds a page where an optimized one costs five and at this
host's forty-microsecond fault the form would not pay in the dev
profile): builds when the measure says blocked; serves raw until the
form file is whole; returns to raw when the limit is lifted and gives the
disk back; a skewed workload that fits never builds; a cold start is not
a capacity miss; the ski-rental price is paid before a build (the rent
against the price at every stretch); a cancelled build counts as a
switch; two stores do not share a measure; a neighbour's pressure opens
the build clause; a store far past the cache still builds (two faults a
raw read, the hash page and the record page, against the form's one);
form files on disk are dormant under `Auto` and attached when the form is
wanted. Six mutants held to firing: the dispatch asking for `Always`, a
coverage always whole, a retire skipped, no cancel below building, the
sampler's reading never fed, a dormant file not attached. Found on the
way: the read and count dispatch asked for `Always` before the gate, so
an `Auto` store in COMPACT read every key raw -- the first test that drove
`Auto` to COMPACT caught it, and the piece-1 gate test had run under
`Always` only. Not in this piece: the wasted-build budget's engine test
(the report counts a superseded build; a test that supersedes four of
eight needs merges under held builds) and the generation's test against
a merge mid-reading.

Piece 3b is in: the footprint on the handle's memo (`Memo::track`,
`touched`, `distinct`; `ZMemo::tracking`, `track`, `footprint_pages`,
`touch` from `open_group`), `Reader::zfootprint_track` at a window's
abandon and under the census gates, the slot's `zcpages` (seventeen
census words now) posted at a window's close under the form's gate,
`Maint::z_footprint` (the bound from the store's quantities; `ZEvents`
and `ZCounts` `footprint_pages`/`footprint_bytes` and their rows) fed as
`Input::footprint`. Tests: the footprint counts the compact pages a
handle touched (ten keys, the spread, the drop and the restart); the
trial judges the refilled state (a one-page hot set, since the bound
charges a page's keys with as many hash pages and the budget is a
mebibyte, and the wide set never trials); a trial that finds the raw
form faulting goes back; a form that stops paying is left (E3); a
square wave switches at most once a dwell. Four mutants held to firing:
no touch, the footprint never fed, the footprint not dropped, the bound
without the hash. Learned: the trial's wall at this scale is a fifth of
a second (three times sixty-five pages at a millisecond), so the engine
test judges on the cached hot page and the fill path is the selector's
replays'; the stress of piece 3a's gate found the census test's window
count load-sensitive (a stretch's windows ran long under twelve
processes, the resize cut the next stretch in two), and it asserts the
commit-free count and not the window count now. The price by callgrind, the piece-3a probe against this one: through the form 881 instructions a get against 878, on the raw path 837 against 834 -- three fewer on both, since the census hooks had grown out of line (seventeen instructions of call and prologue a read through the form for a flag that said no: each is a flag inlined and the rest out of line now), and the page touch asks the memo's flag outside its cell, a load and a branch where the cell's borrow had read eighteen a get.
The chain's bench step linked red once on the way, `ld terminated with
signal 7` with the disk's allowance spent on a second bench target and
the probes, as the notes say it does; cleared, the group ran green.

Piece 4, the arms: `Policy::zpages_auto`, `Supdb::create_auto_zpages`
and `create_nocache_auto_zpages`, the names, the guarantee, the
dispatch, the `zpage_auto` row, `ZState::auto`, the refusal that asks
nothing of the form under the controller but a form file in service
(`zpages_refusal`, its test's `auto` cases), the design's roster, and
`each_auto_arm_differs_from_its_base_in_the_controller_alone`. The
measurements -- 7.5's quick-row quantities and the in-memory pairing
of `supdb,supdb-autozpages` at a hundred and three hundred thousand keys
in two sittings, 7.4's bounded rung against the forced arm -- are the
sittings after it. Piece 3b's gate found the two trial tests red under
`checked` in every run, the plain one and the stress's twelve, green
under `dev`: the test helper that closes a window gave its closing read
a fault, for the census test that counts every read's, so a hot set
that faults nothing read a fault in sixty-five, over the suspect
threshold of an optimized build (a decode at ten microseconds against
a fault at a millisecond) and under the dev profile's (a decode at
sixty), and E2 never saw the raw form fit. Found through a trace of
E2's state (`ZSelect::e2_state`, printed on the controller's trace line
now); the closing read takes no fault, and the one test that counts the
closing read's has a close of its own. The rule is the small rungs'
again: a threshold a test's fixture sits near is crossed by the profile
that moves the costs under it, so a test's inputs are read against every
threshold on the path under both profiles before its green is believed.
Piece 4's gate then reported the same two tests red in every run: its
checked compile of `tests/db.rs` had been killed (`SIGKILL`, the memory
kill the notes describe, with nothing else compiling) and the script ran
the binaries on disk, which were the tree before the fix -- a check
reporting a verdict it had not earned; the gate script stops on a failed
build now. The same stress found three tests of the earlier tree
load-sensitive: the fits test read the platform's own budget and
refaults, and under twelve processes the group's refaults made the
controller suspect a store that fits (it takes the seam's budget now,
the platform's reading being the platform test's); the merge-retire
test read a build's temp name for a live partition, started after it
had collected the builds, as a file that outlived its segment (a temp
name of a live segment stands now); and the tick wait of twenty seconds
was too short for a segment work turn under that load once (a minute).
The gate rerun with the machine free was killed the same way, and
`dmesg` named it: the memory cgroup's kill of `rustc` at 13.3 GiB of
anonymous memory, compiling the checked `db` test binary, against the
group's limit of 13.4. The tree before (a0f18e6) compiled it at a
sampled peak of 13.17 GiB, so the gate had sat two percent under the
limit and the twenty-two lines of source since (the dispatch fix and
two accessors) crossed it -- a creep over the two days the compact
form and the controller added eighteen thousand lines to the engine,
not a step. The peak comes in the codegen's last two minutes, where
the debug info is emitted; the same compile with line tables alone
(`debug = 1`) peaked at 6.96 GiB in 17 minutes against 13.17 in 24,
with the line tables the same size in the binary (12.9 MB against
13.6) and the location lists (100 MB) and half the type info gone. The
checked profile asks for line tables now, as the suite's manifest
already did, and the gate's sampler reads the peak of every binary it
compiles, which is the proof the manifest's setting is in effect.
The in-memory pairing (7.5) was taken on b4cc915
(`scratch/wave2/auto-pairing.sh`, the tables
`auto-ab-{100000,300000}-{1,2}.out`): `supdb` against
`supdb-autozpages`, twelve pairs a sitting, two sittings with the arms
swapped, 206 quantities a table. Nothing is marked under Holm in any
sitting, and no mark keeps its sign in the second: at a hundred thousand
keys the shuffled load read 0.89x for the controller's arm (1/12, p
.006) and D 1.10x (10/12, p .039) in the first sitting and 1.01x and
0.94x in the second; at three hundred thousand the lag sweep's 1% point
read 1.43x (11/12, p .006) and its 0% point 1.24x for the controller's
arm in the first sitting and 1.02x and 1.08x in the second, and
`publish_ms` 0.59x and then 1.12x -- position effects, every one. The
controller's arm reports no build start, no sample, no census window
and no install in any sitting: its nonzero `zpage_*` rows are the stores
opened under `Auto` (2 a rep), the selector's steps (15 at a hundred
thousand keys, 45-47 at three hundred thousand, one a census interval),
its S1 evaluations (3 and 6, one a second) and the thread count it saw
(2). In memory the controller costs nothing the pairs resolve and builds
nothing, which is what 7.5 asked. The quick ladder for the controller's
arm beside its base (`bench run --scale quick --arms
supdb,supdb-autozpages`, the row kept as `quick-auto-5df834c.json` and
not banked: the quick roster is the twelve shipping arms and the
comparators, and the controller's arms are the bounded roster's) reads
the eight quantities a load fixes -- bytes on disk and device bytes per
byte at the four rungs -- identical to four decimals; a `run` row
carries no `zpage_*` counters, those are the pairing's, and the thread
spans the suite prints are the harness's own threads, so the
controller's threads are read off its counters, no sampler and no build
started. 7.5 holds. The quick row of 5df834c itself, banked, ran on a
host out of band (its memory floor at half the window's), so its rates
have no verdict, and it flagged the load's device bytes per byte for
every supdb arm at every rung as better than every prior row of its
class: the window was this host's seven rows from before October 11,
the quantity had read the same in the eleven rows since on another
host, a bisect by the deterministic quantity named the streamed ordered
load (61b7240, October 6), and the device's counter read a cut of two
fifths where the suite's count read three and a half percent
(`devbytes-*.txt`; `bench/CLAUDE.md`).

The bounded pairing (7.4, `auto-ooc-pairing.sh forced`: the forced
form's arm against the controller's at ten million keys under a
gibibyte, six pairs after the warmup, the reads divided by ten) has its
first sitting (`auto-ooc-forced-ab-1.out`, the trace in `.err`); the
container restarted inside the second sitting's warmup, and that
sitting is run again with the harness's phase marks on and every
stderr line stamped, since the first sitting's trace had no clock
against the passes. A diagnostic rep of the controller's arm alone
(`bench run --scale ooc`, one rep, the same bound and division;
`auto-ooc-diag.err` stamped, `auto-ooc-diag-threads.log` the process's
threads and the device every two seconds) laid the controller's phases
against the passes. What the two say so far, every figure the first
sitting's or the diagnostic's:

- The controller suspects during the load, once the raw form outgrows
  the budget (phi_raw past one at about eight seconds of a fourteen
  second load), samples the store as it grows (twelve samples a rep,
  the last over twenty-nine partitions: build CPU 8-23 s, c_build
  12-34 s), and starts the build by the ski rental about fifty-five
  seconds after the first read, when the read pass (22 s, raw, 0.49
  faults a read) and the threaded reads are already over.
- The build then takes 136-196 s of wall a rep (the dwell at the
  install is ten times the build's wall), beside the threaded scan
  passes -- `size` scans a thread, five hundred million entries over
  the raw form -- which hold the device at 99-100% busy, 140-196 MB/s,
  with 35-50 thousand major faults a second; the build thread, at idle
  CPU and idle I/O class, gets 4-19% of a core and 30-160 faults a
  second, the device's leftovers. Its CPU estimate was 8-22 s.
- So every read pass before the mixes runs raw: the read pass at 0.38x
  of the forced arm's rate with 7.4x its faults, the threaded reads
  0.19-0.23x, the scans 0.08-0.16x (consistent in all six pairs). The
  load reads 1.25x, since the controller builds nothing during it where
  the forced arm builds at every seal and merge (94 builds a load to
  29).
- The install lands 8-10 s before the mixes in every rep, and the mixes
  run in COMPACT. In the pairing they read 0.25-0.63x of the forced
  arm's with 0.07-0.45 major faults an operation (C, which only reads,
  0.23); in the diagnostic rep alone they read 0.75-0.90x of the forced
  arm's sitting-one rates with no faults at all. Why the pairing's form
  files are cold at the mixes and the diagnostic's are not is the open
  question the stamped second sitting is for.
- The verdict so far: the controller decides right and late. It knows
  during the load that the store will not fit and has priced the build,
  and still waits the rental's fifty seconds, then runs the build at
  the device's idle class behind the very faults the build exists to
  remove, so the readers pay the build's price in faults ten times over
  (a c_build of 12-34 s of CPU against 136-196 s of wall). The levers
  to price as arms, after the second sitting: the build at the load's
  close once SUSPECT stands and the sampler has priced it (what the
  forced arm pays for in the load); the build's reads at the readers'
  I/O class, sharing the device instead of waiting for it; and the
  rental's price with the device term the sampler can see (t_f times
  the raw pages a build must read while the readers fault).

The stamped second sitting was cut by a second container restart at
fifty-seven minutes, five of its six pairs done and no table written
(`auto-ooc-forced-ab-2.err` holds the five, stamped); the first restart
had come fifty minutes into the previous bounded run, and no shorter run
has died, so a bounded pairing is three pairs a process now
(`auto-ooc-fairio.sh`), under half an hour, and a verdict that needs
more pairs takes a second process. The five stamped reps answer the
open question: in every one the controller's mixes ran in COMPACT at
the forced arm's rates (C in 0.58-0.67 s against the forced arm's
0.56-0.70, E in 2.4-3.6 against 2.7-3.6), so the cold mixes of the
first sitting were that hour's host, which the quick row's floors had
read out of band and which restarted twice since; and they repeat the
shape rep for rep: the build starts at 72-117 s, after a read pass of
24-36 s (the forced arm's 9-15) and the threaded reads, and installs at
237-279 s, 150-180 s of wall a build.

The first lever is an option and an arm: `Options::zpages_io_idle`,
whether the form's threads (the build, the warm, the sampler) take the
device at the idle I/O class, on as shipped; off, they read at the
readers' class and share the device with their faults.
`supdb-nocacheautozpagesfairio` is the controller's bounded arm with it
off, and the pair against `supdb-nocacheautozpages` under the bound
prices it. The build thread reads its class back from the kernel
(`ioprio_get`) and counts it (`ZCounts::build_io_class`, the row
`zpage_build_io_class`), so the test holds the option to what the
kernel says and not to what the thread asked; the mutant that idles
whatever the option says fails the test's off case.

The class, priced (`auto-ooc-fairio5m-ab-{1,2}.out`, five million keys
under 512 MiB, three pairs a process, both orders; the ten-million runs
died with the container, and the floors and the harness do not fit the
256 MiB floor at three million, the cgroup killed that run in its first
two minutes): refuted. Nothing is marked in either table, the mixes'
marks flip sign with the order, the read and scan passes read level.
The stamped reps say why: the build's wall is 80-93 s under both
classes in every rep but the first of each process, whose passes end
before any build can (the install lands at 133-147 s into a rep whose
last mix ends near 150, under either class). The class is not what
limits the build. The diagnostic's sampler had the build thread at 44
major faults a second (the median, 117 at most) at ten million keys,
and the device's readahead is 128 KB, so the build's rate is one
readahead window a wait, 5-15 MB/s, each wait a device round trip
behind the readers' queue (`mq-deadline`, `nr_requests` 256, a device
rotational by its own account), whatever the request's class: the
build's reads are the page faults of a mapping walked by rank, with
nothing asked ahead. The next lever is the window, not the class: the
raw records asked for ahead of the walk in megabytes (`MADV_WILLNEED`,
as the warm already does by chunk), so that one wait fetches a megabyte
and not 128 KB, and the build's wall is the device's bandwidth share
and not its round trips.

That window is an option and an arm now: `Options::zpages_build_ahead`,
the bytes of raw records a build asks for ahead of its walk
(`MADV_WILLNEED`, a window at a time, the next asked once the walk is
within half a window of the last), zero as shipped until priced, the
bytes asked counted (`ZCounts::build_ahead_bytes`, the row
`zpage_build_ahead_bytes`); `supdb-nocacheautozpagesahead` is the
controller's bounded arm with four mebibytes, and `auto-ooc-ahead-5m.sh`
is its pairing against the controller's arm, five million keys under
512 MiB, three pairs a process, both orders, stamped. A unit test holds
the ask to the kernel (a cold copy of a segment resident within a moment
once asked, a second copy left cold), the integration test holds the
count to the record section asked once, whole, and each has the mutant
that fails it alone.

The window, priced (`auto-ooc-ahead5m-ab-{1,2}.out`, five million keys
under 512 MiB, three pairs a process, both orders): it holds. The
build's wall falls from 66-93 s to 29-60 s in every rep of both
sittings (medians near 83 and 47), so the install lands 30-45 s
earlier, at 88-112 s into a rep against 119-150; the bytes asked are
the record section, whole, in every rep. What the earlier install buys
depends on where it falls against the passes, which in the suite's
shape is the boundary between the threaded scans and the mixes: in the
first sitting the window's installs came before the mixes and the
shipped shape's inside them, and the mixes read 1.6-5.0x (C 5.0x, D
3.7x, B 2.7x, F 2.6x, A 2.0x, E 1.6x) with the three-thread scan 2.9x;
in the second the shipped shape's installs met mixes that started a
few seconds later, two reps of three, and the mixes read nearer level
(D 1.45x and E 1.16x for the window, A 0.91x) with the three-thread
scan 3.0x. The passes before any build, the load, the read pass and
the single scan, read within five percent with the sign changing
between the sittings, position; the threaded reads and the two-thread
scan the same. Nothing is marked at three pairs, since the sign test
cannot mark at three; the two orders agree on the wall, the install
and every cell after it. Four mebibytes is the default now, the shape
without a window the arm (`supdb-nocacheautozpagesnoahead`), under the
gate before the controller is paired against the forced arm again.

7.4 again with the window (`auto-ooc-forced5m-ab-{1,2}.out`, the forced
arm against the controller's, five million keys under 512 MiB, three
pairs a process, both orders): the load reads 1.16x and 1.24x for the
controller, which builds nineteen form files a rep where the forced arm
builds forty-seven at its seals and merges; the controller installs in
every rep (the forced arm has nothing to install, its files come with
the load); and the mixes, which now run after the install in every
rep, read level in both orders (0.96-1.09x and 0.91-1.03x, no sign
kept). What the controller still pays is the shift's latency: the read
pass at 0.40x in both orders with six times the faults, the threaded
reads at 0.16-0.23x, the single and two-thread scans at 0.09-0.13x,
all before the build has landed, and the three-thread scan 0.27x and
0.87x with the install inside it. Nothing marked at three pairs; the
two orders agree on every cell. So the window closed the second half
of the gap (the passes after the install) and left the first (the
passes before it): the build starts 50-60 s after the first read by the
rental, and lands 30-60 s after that.

The next lever is the rental's wait. The controller enters SUSPECT
during the load, when the raw form outgrows the budget, and has the
sampler's price within seconds; it then waits for the readers to waste
the build's price in faults before it buys, by design (the entry is a
price, not a streak, so a workload that stops loses at most twice the
price). At the load's close the build is at its cheapest -- the records
the load wrote are resident and the device idle -- and every read that
comes before it lands is a fault the model already predicted. The arm
to price: the build bought at SUSPECT once the sampler has priced it,
without the rental's wait (`supdb-nocacheautozpageseager`), whose loss
case is a store that is loaded, outgrows the budget and is never read
(one build's CPU on a spare core at idle priority), against the
rental's case, measured here: a hundred seconds of raw reads. The
load's cost is the cell to watch, since the forced arm pays 16-24% of
its load for building at every seal.

The eager entry is an option and an arm: `Options::zpages_build_eager`
has the selector buy at SUSPECT once the sampler has priced the
partitions and they have stood for `zselect::EAGER_SETTLE` (two
seconds) since they last changed, the rent and the measured half of the
saving clause waived; the settle is there because a store under rewrite
-- a load's seals and merges -- would have its build cancelled by the
next merge, and a cancelled build is a switch that costs a dwell. The
eager entry is a one-shot, spent by any cancel or exit: the first
purchase is the prediction's and every later one the rent's, since the
grid of constant workloads looped under an eager entry that could buy
again after every exit -- a form that does not pay was bought, left and
bought again once a dwell, forever, where the rent asks the evidence
each time. Off as shipped; `supdb-nocacheautozpageseager` is the controller's bounded arm
with it on, `auto-ooc-eager-5m.sh` its pairing against the controller's
arm. The replays hold the eager selector to buying where the rent still
waits (eight seconds of the ten-million cell's uniform reads, the rent
short of the price) and to waiting out a store whose partitions change
at every step; the grid of constant workloads comes to rest under the
eager entry as without it (it changes when the form is first bought and
nothing after); the store test buys without a read under the seam where
the shipped store does not; and the mutants, eager ignored and the
settle ignored, each fail the test written for them.

The eager entry's first sitting (`auto-ooc-eager5m-ab-1.out`, five
million keys under 512 MiB, three pairs) and the two reps its second
sitting wrote before the cgroup killed the process at 516 MB of
anonymous memory, in the shuffled load's drain, with no build running
(`auto-ooc-eager5m-ab-2.err`; the harness itself nears the bound there
at this rung, and the next bounded pairings run under 640 MiB). The
eager arm buys at 6-11 s into the rep, two seconds after the load's
last change of partitions, and installs at 29-63 s in five reps of
seven, against the shipped entry's 96-120: so the threaded scans read
2.3x and 2.4x, the mixes level, the read pass 0.84x since the build now
runs inside it, sharing the device. In the other two reps the eager
build was cancelled at 18-20 s by a merge landing after the load -- the
settle of two seconds since the last change of partitions passed while
the load's trailing merges were still in flight -- and the dwell a
cancelled build costs put the next purchase at 105-135 s, later than
the rent's. That reading was wrong, and the next paragraph has what the trace
says.

The cancels were the vanish rule's, not a merge's. Both of the first
sitting's cancelled builds ended in `BuildCancelled`, the event of
`Cancel::Vanished` (the measured saving under `theta_enter / 2` in every
class with evidence for `VANISH_INTERVALS` evidence intervals, three
census intervals of a hundred milliseconds), two and five seconds into
the threaded read pass, and a merge's supersession is not that event.
The trace carried no census, so it was extended: every interval of a
build prints the census windows, the group's refaults, the evidence's
saving by class, the vanish run, the thresholds, the model's two
populations, the generation and the segment work in flight
(`auto-ooc-eager-diag.sh`, the same two arms at five million keys under
640 MiB, the passes cut to the load and the two read passes, four
pairs). Every eager rep bought the form at the sampler's reading and
cancelled it within 0.3-1.2 s, with nothing in flight and the generation
unchanged, on evidence like this: the reads' window 14,316 operations
and 3,810 faults, 0.27 a read, against the model's `phi_z` of 0.215, a
saving of 0.05; and the model's `phi_raw` for the same reads was 1.246.
Solving `model_raw` back from 1.246 at the tick's budget of 501.6 MB and
records of 620.1 MB gives a hash population of 716 MB, bigger than the
records, and `Seg::foot` was handing the model the index section as the
hash population: the record region lies inside the key section for a
record whose value is inline, every record of the suite's shape, so the
records were counted twice and the raw form predicted to miss twice
where it misses once; with the hash region alone (67 MB, eight bytes a
slot) the model answers 0.30 for the reads that measured 0.27. The fix is
`foot` returning the hash region and the record region, the counts
carrying both (`ZCounts::model_hash_bytes`, `model_record_bytes`), a
test holding them to the files' headers and a store whose populations
fit its budget to FITS (both fail with the section as the population),
and the trace printing them. Consequences: every bounded row of the
controller so far ran under a `phi_raw` about four times the truth --
SUSPECT entered at once, the predicted-saving clause always passed, the
model's error (`note_raw`) a fault a read, and E2's raw footprint bound
overstated by the same factor -- so the 7.4 verdict against the forced
arm stands as a measurement of that controller and is to be taken again
with the model right. The segment-work signal (`Input::rewriting`) is
still a sound guard for an eager buy, a build a landing would supersede,
and is the change after this one; it is not what the sittings needed.
What they say now: at 640 MiB the eager entry buys on a prediction
(0.30 raw against 0.215 compact, the pressure term on) and the vanish
rule cancels on the measurement (0.27 against 0.215, 0.05 a read, under
half of `theta_enter`) within a second, and that is the controller
reading a form that saves the point read little at this bound -- the
form's pages are the raw records' size at the suite's incompressible
values, 626 MB against 620, and its saving is the hash region's 67 MB
and the second touch -- where the first sitting's threaded scans read
2.3x under it. Whether the eager entry is worth shipping is a question
for the scan class and for the pairing with the model right.

The signal is in. `Input::rewriting` is set by the segment work at every
tick a merge, a promotion or a seal is in flight (`compacting`,
`tiering`, `sealing`), and the selector's `absorb` resets the eager
settle's clock at every step it is set, so the settle counts from the
rewrite's end and not from the last landing. The replay holds an eager
selector to buying nothing for five seconds with the signal up and to
buying once the settle has passed from its clearing; the store test
holds a seal in flight (`hold_seal_landing`) and the eager store to
buying nothing through eighty ticks, then buying once the seal has
landed and the settle has passed; the mutants, the selector ignoring
the signal and the segment work never raising it, each fail the test
written for them. The selector as shipped reads nothing of it. Next:
the eager pairing at 640 MiB with the model right
(`auto-ooc-eager-5m.sh`), read for the scan class as much as the
reads.

The pairing with the model right (`auto-ooc-eager5m640-ab-{1,2}.*`,
five million keys under 640 MiB, three pairs after the warmup, both
orders; `ab-events.py` in the scratchpad lists each rep's events from
its load start): the eager arm bought at 11-12 s into every rep, six
seconds after the load's end and at the sampler's reading, and the
vanish rule cancelled every one of the eight builds 0.3-17 s later, in
the threaded read pass or the scan pass after it, on the read class's
evidence alone -- its measured saving 0.010-0.040 a read against
`theta_enter / 2` of 0.02-0.11 -- since the read class is the one with
evidence then and the rule asks every class that has any. The scan
class, which the form is for, had no evidence yet: the base arm's rent
bought in every rep at 44-58 s, inside the threaded scan pass, on the
scan class's saving of 0.3-1.0 a scan, and installed at 56-116 s (eight
of eight). The eager arm's one-shot was spent at the cancel, the
cancel's dwell (`max(60 s, 10 x the build's wall)`) held the rent's buy
to 72-81 s, and it installed at 86-94 s in four reps of eight and not at
all in the other four. The cells read the position and the install's
timing and nothing of the entry: the scans 0.9-1.2x either way, the
mixes 0.77-1.74x in both directions across the two sittings, the loads
level. So the eager entry as designed loses at this rung and bound,
and the trace names the design's gap rather than the arm's: a build
bought on a prediction for both classes is judged by the vanish rule on
whichever class the pass order measures first, and at this suite's
values the point read's saving is a fraction of the threshold while the
scan's is ten times it. A vanish rule that judges a build on the class
whose predicted saving bought it, or an entry that waits for evidence
in that class, is the next design question; neither is built here,
since the rent's entry, which waits for the scan class's own evidence,
is the shape the rows know and it now installs at 56-116 s with the
window where it installed at 96-120 before. Beside it, the base
controller with the model right: SUSPECT at the load's end as before
(the pressure term), the buy by the rent at 44-58 s, the model's
`phi_raw` 0.37 at the install against a measured 0.31-0.40 a read.
The design's 7.4 at 640 MiB with the populations right
(`auto-ooc-forced5m640-ab-{1,2}.*`, three pairs after the warmup, both
orders) read the controller against the forced arm as before in shape --
the controller's load 1.23x and 1.26x the forced arm's in the two
sittings (the forced arm builds at every seal), SUSPECT at the load's end,
the rent's buy at 43-56 s on the scan class's saving and the install at
55-113 s in every rep, the mixes after it level -- and one cell said what
the model was still getting wrong: the forced arm's point reads under its
form took 0.0077 and 0.0000 faults a read in the two sittings, and its
scans 0.000, against the raw form's 0.35-0.39 and 2.5-2.7, while the
model's compact rate (`phi_z`) at the install read 0.24-0.28. The form
fits the budget whole: the counts put the controller's nineteen form
files at 360 MB (`zpage_file_bytes_at_end`, 85,766 pages over 620 MB of
records) and the sampler's estimate at 626 MB (`zsample ... w_z`), 1.74x
the files and the records' own size -- 150,000 pages of 33 records, the
uncompressed count a page, where the build's pages hold 58. The sampler
filled each of its thirty-two groups with no hint, and the filler's doc
says what that makes ("a page well short of full ... whoever sizes a
partition from such calls has to chain a few first"); its test asserted
the estimate's arithmetic and that the form was under the records, which
a 1.7x estimate still was. So every rate the model drew from `W_z` was
off by that: `model_z` 0.22-0.28 where the form faulted nothing, the
predicted saving 0.09 where the forced arm measured 0.3-0.4, the measured
saving the rent and the vanish rule take (`phi_meas_raw - phi_z`) a
quarter of the truth, and the eager entry's eight cancels above were the
vanish rule reading a saving of 0.01-0.04 that was 0.3. The sample
chains three fills a rank now and counts the last
(`zfile::SAMPLE_CHAIN`); `ZCounts::sample_w_z_bytes` carries the
estimate, the suite's row `zpage_sample_w_z_bytes` beside the files'
bytes; the unit test holds the estimate to the build's pages and file
within a tenth and the engine test to the store's files, and both fail
with a chain of one. Both pairings are to be taken again with it, since
every controller verdict so far -- the 7.4 verdicts at 512 and 640 MiB
and the eager entry's -- was drawn on a compact rate three times the
truth: the rent's accrual, the predicted-saving clause, the vanish rule
and E3 all read `phi_z`.

The design's 7.4 with both inputs right (`auto-ooc-forced5m640s-ab-{1,2}.*`,
five million keys under 640 MiB, three pairs after the warmup, both
orders). The sampler's estimate reads 360.00 MB for files of 360.06
(`zsample ... w_z 360002128` against `zpage_file_bytes_at_end`
360062976), and the build's CPU it prices 4.7 s where the unchained fills
had said 12-14, so `c_build` is 7.1 s where it was 18-21. The controller
enters SUSPECT at the load's end as before, and buys at 23-27 s in every
rep -- inside the threaded read pass, on the point reads' own measured
saving of 0.27-0.33 a read in six reps of eight, the model's compact rate
now near zero for a form that fits, and on the scans' 0.38-1.16 in the
two whose threaded reads were shorter -- where the sittings before read
43-58 s on the scan class alone; the install lands at 57-72 s in eight
reps of eight, the build's wall 30-45 s under the readers' I/O, where it
landed at 55-116 before and 96-120 before the window. After the install
the mixes read level with the forced arm's, 0.83-1.0x in the forced-first
sitting and 0.89-1.17x in the other, every mix's faults a read under a
hundredth in both arms; the read and scan passes read raw (0.35-0.38 and
2.6-2.7 faults an operation against the forced arm's 0.006-0.008 and
0.000), since they run before the buy; and the controller's loads read
1.09x and 1.15x the forced arm's in one order and 1.03x and 1.21x in the
other, the forced arm building at every seal. So the gear shift at this
rung and bound is the rent's twenty-odd seconds and the build's
thirty-odd, and the price of the controller against the forced form is
the read and scan passes it serves raw before the install; what the
forced arm pays for serving them compact is its loads. The eager entry's
pairing with both inputs right (`auto-ooc-eager5m640s-ab-{1,2}.*`):
the eager arm buys at 14-16 s in every rep, no build cancelled (the
read class's measured saving is 0.26-0.33 a read now against a
`theta_enter / 2` of 0.03-0.12), and installs at 44-73 s; the rent buys
at 22-25 s and installs at 57-78 s. Eight seconds of head start, spent
inside a build whose wall under the readers' I/O ranges 30-57 s in both
arms, and the cells read level within noise in both orders (the mixes
0.88-1.10x either way, the loads 1.00-1.04x, the reads and scans
position-bound). So the eager entry buys nothing the rows can see at
this rung and bound, and it buys on no evidence -- its loss case, a
store loaded past its budget and never read, stands -- where the rent
now pays within ten seconds of it; the arm stays and the default stays
the rent. The gear shift is the build's wall now: twenty seconds of rent
and thirty to fifty of build, and the second is the lever left.

## Against the comparators out of core

Every bounded pairing above paired supdb's arms with each other; the one
comparator table out of core (`docs/engine.md`, ten million keys under a
gibibyte) priced the raw form against LMDB with its readahead off:
point reads 0.68x, scans 0.37x and 0.76x on one and three threads, the
mixes C, D and E 0.45x, 0.48x and 0.35x, the loads and the write-heavy
mixes won. `ooc-cmp-5m.sh` pairs the form against the comparators at
five million keys under 640 MiB, three pairs after the warmup in both
orders.

The forced form against that LMDB (`ooc-cmp-lmdbforced-ab-{1,2}.*`):
point reads 1.7-2.4x on one to three threads, scans 2.2-3.1x, the
ordered load 1.2-1.4x and the shuffled 11x, the mixes A, B, C, D and F
1.1-4.4x, and E 0.55x and 0.56x. LMDB takes 0.10-0.15 faults a point
read and 0.33-0.56 a scan at this bound; the form takes none, since its
360 MB of files fit the budget and LMDB's 670 MB file does not, so this
rung measures the form in cache against a B-tree partly out of it, the
regime the form is for and not the one where neither fits. E's loss is
CPU, not faults: the form faults nothing there and reads at 12.5 µs an
operation against LMDB's 7.

The controller against the same LMDB (`ooc-cmp-lmdbauto-ab-{1,2}.*`):
it buys at 23-32 s and installs at 56-71 s in every rep, so the read
pass and the one- and two-thread scan passes run raw and read
0.52-0.67x and 0.26-0.64x, the raw form's standing; the three-thread
scans, under the form, 2.4-2.8x; the loads 1.4-1.7x and 13-15x; the
mixes A, B, C, D and F 1.1-5.7x; E 0.54x and 0.56x. What ships pays the
raw form's deficit for about a minute of reads after a load that
outgrows the budget, and then wins every cell but E.

RocksDB as tuned under 640 MiB: the cgroup killed the harness in both
orders, at 653 MB of anonymous memory in RocksDB's load
(`ooc-cmp-rocksforced-ab-{1,2}.err` end at the load's first seconds; the
kernel's record: `Memory cgroup out of memory: Killed process (bench)
anon-rss:653140kB`). The tuned shape configures a 256 MB block cache and
four write buffers of 128 MB, 768 MB before the harness's own, which no
bound under a gibibyte holds; it is paired at the suite's own bounded
rung instead, ten million keys under a gibibyte, two pairs a sitting and
one sitting a run (`ooc-cmp-rocks-10m.sh`). Both sittings
(`ooc-cmp-rocks10m-ab-{1,2}.*`, two pairs after the warmup, the arms
swapped) agree: the form reads 3.0-3.5x RocksDB's point reads on one
thread, 6.8-7.0x on two and 8.9-9.2x on three, scans 10-13x, loads
1.17-1.28x ordered, and takes every mix -- A 2.2-2.6x, B 4.3-4.5x, C
6.0-6.5x, D 4.6-6.1x, E 9.9-10.5x, F 2.6-2.8x; the shuffled load read
1.16x for the form in one order and 1.17x for RocksDB in the other, a
position effect, level. The device rows say the margin: RocksDB's tuned
shape spends 768 MB of the gibibyte on its own buffers and block cache,
and what the page cache has left does not hold its files, so it reads
3-4.5 KB from the device a point read (0.7-1.1 refaults) and 16 KB a scan
(about four), while the form's 720 MB of files fit beside the harness
and fault 0.07 a read on one thread and nothing on three. The form in
cache against RocksDB out of it, which is the configuration a user
deploys at this bound.

What E is made of, and what would win it. E is 95% scans of fifty from
a Zipfian key and 5% inserts above the store's greatest key, and its
scans start in the Zipfian head where the earlier mixes' updates sit
unsealed, about sixteen memtable keys a window. In memory the quick row
at three hundred thousand keys reads E at 560k operations a second with
the block cache (1.45x LMDB's 387k) and 133k without it (0.34x): the
block cache's per-block merged forms, the records with the unsealed
keys folded in, are what win E, and the bounded arms run without them
because the cache is anonymous memory under a bound. The compact form
took the nocache arm's E from 0.35x of LMDB (raw, ten million under a
gibibyte) to 0.55x by removing its faults; the rest is the nocache
walk's CPU. The decomposition (`e-decomp-5m.sh`: the mixes' timer by
kind and the form's events per mix, under the forced arm against LMDB)
says where the twelve microseconds go -- the decode where the memo
misses the head, the merge of the unsealed keys, or the walk -- and the
arm to price after it is `supdb-cache256tofitautozpages`, the bounded
block cache beside the controller's compact form (the edits are staged),
paired against `supdb-nocacheautozpages` on the mixes: what the Zipfian
head's short scans gain from the merged forms against what the point
reads lose to the page cache the cache displaces.

The decomposition (`e-decomp-ab.*`, the forced form against LMDB at five
million keys under 640 MiB, the load and the mixes alone, two pairs; the
mixes' timer by kind and the form's counters per mix, which the events
line carries now under `SUPDB_MIX_COUNTERS` with `SUPDB_MIX_EVENTS`):
E's time is the scans' alone, 975-1,297 ms over 79,198 scans of fifty
against LMDB's 489-805, the commits 38-51 ms on both arms, so 11-16 µs a
scan against 6-10. Under the form a scan touches 1.9 groups, 1.16 of
them memo hits and 0.73 opened from the file (57,543 of 79,199 scans,
the memo's hit rate 61% over the Zipfian head with its 1,024 slots and
8 MiB), and at 5-6 µs a decode the opens are 320-350 ms of the scans'
time, a quarter to a third; the rest, 8-12 µs a scan, is the walk with
the merge of the unsealed head, about sixteen memtable keys a window.
The point-read mixes read the memo the same way: C hits 61%, B 40%, A
23%, F 14%, D 1% (uniform). So two levers, in the order of their share:
the merge, which the block cache's merged forms fold in advance (the
combined arm, `supdb-cache256tofitautozpages`, gated and built), and the
memo's size over the head, a constant today (`ZMEMO_SLOTS`,
`ZMEMO_BYTES`), which a larger memo would price on the decodes alone.

The combined arm with a 256 MB cache did not fit the bound: both
sittings at five million keys under 640 MiB were killed by the cgroup at
650 MB of anonymous memory, in the shuffled load of the warmup rep
(`cache-e5m256-ab-{1,2}.err`, the kernel's record `anon-rss:650784kB`
and `650764kB`), the cache at its bound beside the shuffled load's
memtable and the harness. The Zipfian head's hot blocks are a small set,
so the arm is `supdb-cache64tofitautozpages` now, with `supdb-cache64tofit`
as its base. Taken again under 640 MiB it died in the same place at the
same size, 651 MB in the shuffled load's first seconds in both orders
(`cache-e5m64-640-ab-{1,2}.err`), so the growth is not the cache's bound
but what the block-cache arm holds beside it in a shuffled load -- the
suite's own figures had the `tofit` arm's process at 578 MB median at
three million keys, and the nocache arm's drain at 516 MB under 512 MiB
once -- and it is read against the passes with the process's anonymous
memory sampled, unbounded, for both arms (`anon-probe-5m.sh`,
`anon-probe-rss.txt` beside `anon-probe.err`), before the bound for
the pairing is chosen.

The probe, unbounded (`anon-probe-rss.txt`): both arms peak in the
shuffled load, 321-379 MB for the combined arm and 332-339 for the
controller's, and sit at 50-143 MB through the mixes, the combined arm
40-80 MB above the controller's at every point. Under the bound
(`anon-probe640-rss.txt`): the controller's shuffled load peaks at 333
MB and the combined arm's first at 399, both surviving, and the kill
comes in the combined arm's second rep's shuffled load, which climbs
from 143 MB to 635 in eleven seconds -- the last three at a hundred
megabytes a second -- while the page cache is squeezed from 486 MB to
11. FROZEN_CAP is three tables of 32 MB, so the pile is not the
memtables; what the block-cache arm holds that the controller's does
not, under seals and merges slowed by a page cache reclaimed to nothing,
is the write path's retention the census named once before (the forms'
copies a publish carries and the snapshots over the frozen tables, held
while the segment work's pins stand), and the owner is a census sampled
through the load to name, which no instrument prints there yet. For the
E question the shuffled load is not needed: `cache-e-noshuf-5m.sh` pairs
the arms on the reads, the scans and the mixes alone under 640 MiB, and
the block-cache arm's memory under a bound in a shuffled load is a
standing item of its own.

The pairing without the shuffled load (`cache-e5m-noshuf-ab-{1,2}.*`,
three pairs, both orders) refutes the combined arm, and by mechanism:
E reads 0.65x and 0.58x of the controller's under it, the other mixes
0.19-0.58x, the point reads 0.73-0.93x and the scans 0.25-0.88x. The
form serves the combined arm's point reads as it serves the
controller's (`zpage_reads` 83,334 in C under both, the same decodes
and memo hits) and none of its scans (`zpage_scans` 0 against
1,801,236): the block cache's walks stay on the raw form, so its scans
-- the scan passes and E's -- drag the raw records through the page
cache and evict the compact form the point reads need, and the working
set is the raw form and the compact one together. The faults say it:
C takes 0.208 faults a read under the combined arm against 0.001, E
0.363 against 0.003, the three-thread scans 0.76 a scan against 0.013.
This is the rule the compact form was built under, that a second form
may accompany the records only if the other paths never touch them,
read from the other side: a cache whose walks touch the raw records
cannot sit beside a form that keeps them out of the cache. What would
win E is the block cache's merged forms built from the compact form's
records instead of the raw ones, so that no path touches the raw form
under the bound -- a design item for the walk, not an arm; the arm
stays as the record of why. The levers left for E as the engine stands
are the memo's size over the head (a quarter to a third of E's scan
time is the decode of the groups the memo misses) and the merge of the
unsealed head in the walk itself.

## The block cache's forms built from the compact form

The design item the refutation named is built: the block cache's reads
of a covered partition go through its form file while the gate serves
it (`Options::zpages_block_cache`, on; `docs/engine.md`). `BuildCtx`
carries the handle it builds for, and every read of a partition's
records the cache's machinery made through the door -- the clean walk
and the walks between a form's keys (`scan_at` by rank), the cut of an
unsealed key inside a walk (`cut_at`), the partition's own record at an
overlay key's emit (`values_at`), the block boundaries a table's bounds
read (`block_bounds_of`), the last key a table keeps, the key a lazy
scan resumes from, the seek a scan starts a partition with and the
block a written key belongs to (`owner_of`) -- asks the handle first
(`Reader::cache_scan`, `cache_values`, `cache_cut`, `cache_seek_exact`,
`cache_key_at`), which dispatches to the form file as the nocache paths
do (`ZFile::scan_at`, `values_at`, `cut_in`, `seek`, and a `key_at_into`
written for the boundary keys, which answers a record no page holds
from its fence entry) and to the door otherwise. The raw prefetch of a
block's records is not issued under the form. What stays raw is a
piece's side of every join -- a piece has no form file -- and the
segment work's joins at a landing, which have no handle. The
understudy and the builder ahead are handles with memos of their own,
so the upkeep thread's builds and conversions go through the form too.

Held by `tests/zpages.rs`: the no-raw-touch test's phases (drained,
unsealed keys inside the windows, over a piece, after the merge, beside
a record no page holds) run with the cache on and count zero, with two
positive controls -- the option off counts touches, and the gate set to
the raw form with the cells kept (the trial's shape) counts touches
while the gate back at the form counts none; the windows test counts
the block path's walks through the form (one a run between cuts). The
first cache-on run found one touch: the last key a table keeps was the
record no page holds, read raw by `key_at`, where the seek had always
answered such a record from its fence entry; `key_at_into` answers it
from the fence now. The differential fuzz with the form forced and the
cache on in every seed (`ZFUZZ_ZPAGES=1 ZFUZZ_CACHE=all`) read sixteen
seeds clean, and a second range with the options varied was killed by
my own cleanup -- `rm -rf /tmp/supdb-next-*` issued before the chain
with the fuzz still running in `/tmp`, the exact failure `CLAUDE.md`
names, `seal create: No such file or directory` -- and is run again in
a `TMPDIR` of its own.

The arm for the shape before it is `supdb-cache64tofitautozpagesraw`
(the cache's reads through the door under the controller's form), so
the change is priced in one process: `zcache-all.sh` runs the three
pairings at five million keys under 640 MiB without the shuffled load,
both orders -- the combined arm against the controller's nocache arm
(`zcache-e5m-ab-*`: E), against LMDB with its readahead off
(`zcache-lmdb5m-ab-*`: whether E is won), and against the raw-reads
arm (`zcache-raw5m-ab-*`: what the change moved).

The fuzz run in its own `TMPDIR` found four seeds of forty-eight
(26, 31, 48, 53) whose scans disagreed with the model, each with the
cache on and no partition in the store, and the same four fail on the
base commit (133a237) with the cache alone and no form file, in a
worktree built from it: pre-existing, and not the block cache's reads
through the form. A probe stamping each log read with its caller named
it: the check's `z.reader()` claims a handle, which reads the writer's
log for the forms (`publish_for_handle`) and, with no partition to hang
them on, refreshes nothing; the writer's next scan on the merge path
found the log read already (`moved` false), returned from the refresh
before filing the keys created since into the snapshot's side runs, and
walked a snapshot short of them -- 106 keys in seed 26. The scan asks
whether the list has grown past what the snapshot has filed now, and a
test in `tests/db.rs` builds the shape with a memtable alone and with
pieces (the direct runs off, since ascending keys from an empty store
open a run whose close at a lower key is a freeze, and a freeze rebuilds
the snapshot and hides the case): red with the fix off, green with it
on, and the four seeds, sixty-four seeds in two configurations and the
concurrent range clean under it. The fix goes in after the chain that
gates the block cache change, as a commit of its own.

The pairings (`zcache-e5m-ab-{1,2}.*`, `zcache-lmdb5m-ab-{1,2}.*`; the
raw-reads pairing below; the container restarted inside the third
pairing's first sitting and it was run again on the same binary,
`zcache-raw-5m.sh`). Against the controller's nocache arm, the combined
arm with its reads through the form wins every mix in both orders --
E 1.38x and 1.80x (3 of 3 pairs each way), D 1.24x and 1.27x, C 1.16x
and 1.04x, F 1.08x and 1.12x, A 1.07x and 1.05x, B 1.00x and 1.04x --
and faults on none of them, where the nocache arm takes 0.001-0.01 a
read; three pairs mark nothing under Holm, and the sign holds in every
cell of both orders. E's time by kind is the scans' alone, 6.1-7.6 µs
a scan against the nocache arm's 10.6-12.9, the commits level at 31-49
ms a mix in both. The passes before the install -- the point reads at
twelve seconds, the scans at twenty-four, the install at 51-64 s in
both arms -- read level: point reads 0.90-0.98x in one order and
1.04-1.07x in the other, the scan passes 1.06-1.08x on one thread,
0.96-1.02x on two, 1.10-1.43x on three, with the faults the same, so
the cache costs the raw stretch nothing. The counters say what the
cache does with E: 104,943 walks through the form file for 79,198
scans (1.33 a scan, the runs between a sparse form's cuts, each a walk
of its own), 75,768 groups opened from the file (0.96 a scan, the memo
hitting 73% of 285,495 looks) against the nocache arm's 57,550 opens
(0.73 a scan, 61% of 149,533), and 42 upkeep passes a mix, one a
commit, against 1-4: at five million keys the Zipfian head's blocks
hold fewer than `CACHE_DENSE` unsealed keys apiece, so E walks sparse
forms through the form file rather than copies, and the win is the
merge of the unsealed keys folded into the deltas, not the decodes,
which the cache's walk opens more of.

Against LMDB with its readahead off, E stands at 0.55x and 0.62x in the
two orders (0.52-0.75x by rep), still a loss; A 4.9x and 4.2x, F 3.4x
and 3.3x, B 1.2x and 1.5x; C 0.53x and 1.33x, which is LMDB's two
shapes and not ours -- its C reads 543-913k a second across the reps
where ours reads 372-624k with its gets at 1.7-2.1 µs in every rep; D
0.82x and 0.74x, ours 4.1-5.8 µs a get with a group decoded for 77,551
of 78,487 reads through the form (the mix reads the tail uniformly and
the memo holds nothing of it). The passes before the install read the
raw form, as every controller pairing has: point reads 0.36x, scans
0.12x on one thread, at four and fourteen times LMDB's faults. The
sitting's LMDB E is 209-265k a second where the earlier comparator
sitting read it at 149-151k, so the move since the nocache arm's
0.54-0.56x is not read across the two sittings; within this one the
cache took E from the nocache arm's shape to 0.55-0.62x of LMDB's. What
is left of E at 6-7 µs a scan against LMDB's four: the hot blocks
walked as sparse forms, each run between cuts a `scan_at` of its own
with a fence look, a memo look and the skip to the rank, and a decode
for 0.96 of them -- levers to price are a cursor held across a sparse
walk's cuts as the merge path holds its own (`ZCursor::walk_until`),
the dense threshold against the memory it buys, and the memo's size
over the head; none is built here.

The raw-reads pairing (`zcache-raw5m-ab-{1,2}.*`, the combined arm with
the cache's reads raw against the same arm with them through the form,
both orders) prices the change itself in one process: the form-reads
arm over the raw-reads arm reads E 2.8x and 2.6x, C 5.1x and 5.6x, D
4.6x and 3.8x, B 2.9x and 2.8x, F 1.9x and 1.7x, A 1.8x and 1.8x, 3 of
3 pairs in every cell of both orders, with the raw-reads arm faulting
0.05-0.36 a read across the mixes and the form-reads arm 0.000; E's
scans read 18-23 µs under the raw reads against 6.4-9.7 under the
form's. The passes before the install are identical (point reads
1.02x, scans 0.98x, the faults the same), since neither arm has a form
to read yet. The refutation's mechanism, read from the other side: the
cache's raw walks evicted the form the point reads needed, and with
the walks through the form the cache keeps the compact working set and
adds its merged forms to it.

## The hot blocks as copies under the form

What is left of E under the combined arm is the decodes: the sampler on
the pairing's run prices a decode at 5.2 µs a page (`zsample ...
decode_per_page`), and the counters put the mixes' cost on it -- C's
2.0 µs a get is its 0.39 misses a read times a decode, D's 4-6 µs a get
a decode a read, and E's 0.96 groups opened a scan about 5 of its 7 µs.
The cache has a mechanism for exactly that, the promotion of a block the
reads have walked `promote_entries` entries' worth to a merged copy,
and it counted only blocks walked as deltas, since a clean block's raw
walk streamed the records and had nothing to gain; under the form a
clean block's walk is a decode a walk. The second shape
(`Options::zpages_promote_copies`): the copy stands in the handle's slot
as the block's one form, clean blocks included, judged by the form the
walk read -- a handle at the writer's commit takes the canonical form
and builds no slot of its own, which is where the first version never
fired, in a test over a drained store -- and read before the canonical
form once it stands; listed and shed under the budget like any form
(the copy beside the cheap form was never shed, so under a bound it
would have pushed the budget over and shed everything else); and the
clean-throughout shortcut, the bulk walk of a partition with nothing
unsealed over it, is skipped under it, since the bulk walk decodes a
group a page every time and only the walk through the blocks promotes.
The threshold under the form is a block's worth of entries, sixty-four:
a copy's build costs the decode the walk pays anyway, so a block
repays a copy at its second touch, where the raw crossover was a few
hundred entries. `tests/zpages.rs` holds a window to the model with the
blocks promoted under a budget that sheds them, and holds a block read
three times to a copy that walks no form, touches no raw page, and
stays right through a write spliced into it, with the option off as the
control; `ZFUZZ_PROMOTE` runs the differential fuzz over it. The arm is
`supdb-cache64tofitautozpagespromote`, priced by `zcache-promote-5m.sh`
against the combined arm and against LMDB.

Refuted (`zcache-promote5m-ab-{1,2}.*`, `zcache-promotelmdb5m-ab-{1,2}.*`),
in both orders and by mechanism. The promotion arm over the combined
arm reads E 0.81x and 0.71x, C 0.44x and 0.33x, D 0.52x and 0.44x, B
0.50x and 0.53x, A 0.69x and 0.76x, F 0.85x and 0.83x, 0 of 3 pairs in
every cell, and faults on every mix where the combined arm faults on
none: C 0.064 a read, D 0.107, B 0.038, F 0.026, A 0.016, E 0.018
against 0.003. E's walks through the form fell by three tenths (74,081
against 104,943 a mix) and its decodes by three percent (73,445 against
75,768): the copies covered the hottest blocks, whose pages the memo
held already, so the walks they replaced were memo hits, and the
decodes come from the colder blocks a copy never reaches. The copies'
64 MB came out of a page cache that just fit the form files -- the
combined arm's 0.000 faults a read on every mix is the form files
resident whole -- so every mix pays the form's pages faulting back in,
C's gets at 5.5-7 µs against 2.0 with the same 32,772 decodes, D's at
11-15 against 4-6. Against LMDB the arm reads E 0.46x and 0.48x, C 0.29x
and 0.31x, where the combined arm read 0.55-0.62x and about level. The
passes before the install are the combined arm's, since the promotion
waits for the form. The rule the refutation gives: under a bound that
the compact form just fits, a structure that trades page cache for
decoded bytes loses by the fault it buys for the decode it saves --
sixty microseconds against five -- and the decodes left are the memo's
misses on the warm tail, which no copy of the hot head reaches; what
would cut them is a cheaper decode or a memo that holds more in the
same bytes, and the decode is priced next, by instruction, before
either is built. The arm stays as the record, off as shipped.

## The decode priced

Counted before anything else is built for E, since the "5 of 7 µs" the
promotion was built on took the sampler's decode-plus-walk figure for
the decode alone (`zsample ... decode_per_page`, which walks every
record of the group it decodes). Callgrind over `e_passes` at three
hundred thousand keys in core, the forced nocache form, toggled on the
harness's `get` and `range` (valgrind names the generic functions in
their angle-bracket form, `<supdb::db::Reader>::scan_partitions::<...>`,
and a toggle on the plain path collects nothing; the harness's
`<Supdb as Engine>::get` and `::range` are outlined and hold the whole
read): the point read's instructions are 60% LZ4 decompression and 13%
the seek inside the group (`group_seek`), the fence's entry 4%, so a
point read through the form is the decode; E's scan on the merge path
is 6% decompression, 29% the walk itself, 11% key compares (`memcmp`),
and about 20% the memtable's chain reads for the unsealed keys
(`ByteArena::block` 8%, `has_tomb` 3%, `live_offs_into` 3%,
`Snapshot::key_of` 3%, `entry`, `value_at`), 83,000 instructions a scan
of fifty. The time profile in core agrees (perf over the same probe,
the E window): decompression 13% of E's samples, `memcmp` 15%, the walk
19%, the memtable's reads 15%. So E is the overlay's merge, not the
decode, and the point reads are the decode; the bounded profile of the
combined arm at five million keys (`perf-bound.sh`, `perfab/`) is what
says where the block path's 7 µs a scan goes under the regime that
matters, and a cheaper decode is the point reads' lever before it is
E's.

The bounded profile (`perf-bound.sh`, one rep of the combined and the
nocache arms at five million keys under 640 MiB, the E and C windows
cut by their marks on CLOCK_MONOTONIC, `perfab/`): the combined arm's E
is LZ4 decompression 35%, the group's open, seek and fence 13%
(`open_group` 4.3, `group_seek` 3.9, `entry_of` 3.1, `part_scan_z` 1.9),
the block walk 10% (`scan_blocks` 5.1, `cache_scan` 3.5, `each_value`
1.3), the harness's Zipfian generator 11% (`pow` 7.9, `run::ycsb` 3.4,
paid by every arm alike), context switches 3.6% and value moves 2.9%;
the nocache arm's E is decompression 19%, the memtable's merge of the
unsealed keys 26% (`has_tomb` 11, `entry` 6, `Snapshot::seek` 3.7,
`ByteArena::block` 3.1, `key_of` 2.3), key compares 10%, the walk 7%,
the cursor's opens 3%. So the cache took the merge and the compares out
and left the decode, which is now a third of E and, with the group
machinery, half; C under either arm is decompression 37-38%, the group
seek 6-7%, the open and the fence 8%, and the harness's generator 22%.
In core at three hundred thousand keys E's decode was 6% by
instruction and 13% by time because the memo hit far more often there;
at five million the memo misses 27% of 3.6 looks a scan, 0.96 decodes.
A decode is 5,400-10,700 instructions a page (callgrind's inclusive
cost over the calls it collected, E's pages at the low end). The
levers, in order of what they cost to price: the decoder itself
(liblz4's C decoder against `lz4_flex`'s on these exact pages, a
micro-probe over a built form file), the memo's misses for the same
bytes (associativity), and the page's size against the file's
(a smaller group is less decode a lookup and finer memo entries for
the same bytes, and a format change).

## The decoder and the cut priced

`bench/examples/zdecode.rs` (`--features lz4-c`, the `lz4` crate as an
optional dependency so nothing the suite measures links the C library):
the forced form built at three hundred thousand keys in the suite's
shape, every compressed page read back, each decoded with the engine's
`lz4_flex::block::decompress_into` and with liblz4's
`LZ4_decompress_safe`, the two held equal byte for byte, and timed. One
form file of 4,972 pages, 20.1 MB compressed from 36.0 MB, 7,233 bytes
a page decoded, 0.558.

| decoder | Ir a page (callgrind) | cycling 4,972 pages, ns a page | one page hot, ns |
|---|---|---|---|
| lz4_flex 0.11.6, unsafe | 17,562 | 3,393 / 3,534 / 3,763 (three runs) | 1,112-1,188 |
| liblz4 1.10.0 (`LZ4_decompress_safe`, -O3) | 14,405 | 2,831 / 3,044 / 3,408 | 888-1,000 |
| liblz4 / lz4_flex | 0.82 | 0.83 / 0.86 / 0.91 | 0.76-0.83 |

The cycling figure is three times the hot one, and the cache level is
not it: cycling the first 1, 4, 16, 64 and 256 pages of a 515-page file
(all of it in L2) reads 712, 1,014, 2,049, 4,522 and 3,450 ns a page,
the same 3.3-3.5 µs plateau from a few hundred distinct pages up as
over the 20 MB file -- a page decoded over and over is one whose tokens
the branch predictor has learned, so the hot figure is nobody's and the
cycling figure is a read's. The decode is misprediction-bound on these
records: 7,230 bytes in 360-odd tokens at nine nanoseconds a token
cycling, three hot. (`zdecode-300k-{1,2}.out`,
`zdecode-split-300k-1.out`, `cg/zdecode-{flex,c}.out` in the
scratchpad.)

A fifth of the decode is a fourteenth of C and a twentieth of E under
the bounded profile's shares, inside what a pairing resolves on those
cells, for a C library in the engine (built with `cc`, native only, the
wasm reader's decoder left as it is). Declined: the decoder stays
`lz4_flex`.

What the count points at is what a read decodes. A point read decodes
the whole group, seven kilobytes, for one record of a hundred and
twenty; the memo saves the hot pages' decodes and nothing of a miss's
size. The same probe cuts each group at record boundaries into `k` runs
of about equal bytes compressed apart with the engine's encoder (the
whole group recompressed is the page's payload byte for byte, the
check), and times one run a page -- the point read's decode under the
cut, the run rotating with the page so every position is read -- and
every run of every page, the scan's:

| k | compressed / decoded | over the page's own | bytes a run | one run, lz4_flex / liblz4 ns | all runs, lz4_flex / liblz4 ns |
|---|---|---|---|---|---|
| 1 | 0.558 | 1.000 | 7,233 | 3,584 / 3,063 | 3,623 / 3,037 |
| 2 | 0.569 | 1.020 | 3,616 | 1,954 / 1,585 | 3,623 / 3,216 |
| 4 | 0.590 | 1.058 | 1,808 | 1,020 / 856 | 3,608 / 3,057 |
| 8 | 0.628 | 1.125 | 904 | 547 / 474 | 3,566 / 3,050 |
| 16 | 0.692 | 1.240 | 452 | 321 / 291 | 3,521 / 3,033 |
| 32 | 0.807 | 1.447 | 226 | 236 / 219 | 3,467 / 3,140 |

Four runs cost 6% of the form's bytes and take the point read's decode
to 0.28 of the group's; eight cost 12% and take it to 0.15; the scan's
decode is level at every `k`, since a scan decodes the runs it walks and
they sum to the group. At five million keys under 640 MiB the C mix
reads 2.16 µs an op against LMDB's 1.14 (0.53x, `zcache-lmdb5m-ab-*`),
37% of it the decode: 0.8 µs, so most of C's reads hit the memo and
the misses cost the 3.4; four runs would take about 0.55 µs off the op
and eight 0.68, 0.7-0.77x of LMDB from 0.53, before anything the memo's
finer entries save. That is the structure to build: the page a
container of several groups of a target decoded size, one fence entry
and one CRC a page and a directory of the groups inside it, so a point
read decodes one group and a scan decodes the groups it walks, and the
memo's entry is a group and not a page.

## The page as a container of groups

The cut, built: a form file's page holds one to sixteen groups, each
compressed on its own, with a directory after the header naming each
group's first record index, its payload's place and length, its decoded
length and, for every group but the first, the key of its first record
(`src/db/zpages.rs`, tag `ZPG2`, magic `SUPDZPG2`; a `ZPG1` reader would
take the directory for the payload, so an old file is refused at its
trailer and rebuilt). The fill stages a group to
`Options::zpages_group_bytes` of records (two kibibytes as shipped),
compresses it once, and trims only a page's last group to the room,
sizing that group from the page's own ratio first so the room costs a
trim and not a compress call and a trim; at a target of zero the page
is one group grown to fill it, the shape before, with its hint, its
growth rounds and its turns, which is the arm
`supdb-cache64tofitautozpagesonegroup`; `...group1k` cuts at a kibibyte.
A point read finds the page by the fence and the group by the
directory's keys and decodes that group alone; a scan decodes the
groups it walks, the cursor stepping to the page's next group before
the next page; the memo's entry is a group, 4,096 slots under the same
8 MB; the sampler prices a read by the decode of one group and a scan
by the groups it crosses.

Two things the first build of it taught. A page is verified whole at
its first decode, every group walked, and not the group read alone:
the directory's key for a group is held to the group's first record
only by that group's walk, and a key the directory names above the
true first record sends a read of that record to the group before,
where it is absent, with nothing raised -- the forged-page test caught
it on the first run, answering a key on the page `[]`. And the
directory can name a group only by a record: an image with no key to
parse, or one not above the group before (never a record of a
partition), closes the page before it, so the fill stays total on the
adversarial images its tests draw and a page of entropy images holds
one group of them under the cut.

At the suite's shape the fill reads 55.9 records a page in 3.99 groups
at a fill of 0.991, against 58.3 in one group; the cut's price in pages
is held by a test to 2-12%.

Pairings at five million keys under 640 MiB (`zgroup-5m.sh`, head
dda952d, three pairs after the warmup in both orders, each sitting nine
minutes). The cut against the page as one group
(`zgroup5m-ab-{1,2}.*`, the combined arm against
`supdb-cache64tofitautozpagesonegroup`): the form's files 391 MB against
361, 8.5% for the cut, 93,333 pages against 85,932; the mixes, the one
pass that reads the form in these sittings, for the cut in both orders,
3 of 3 pairs in every cell but F's first sitting (2 of 3) -- C 1.61x
and 1.63x, D 1.76x and 1.99x, B 1.54x and 1.46x, F 1.12x and 1.56x, A
1.41x and 1.16x, E 1.31x and 1.21x -- with no faults in any mix of
either arm, since the mixes' Zipfian head is in core, and the same
decodes a mix (C: 83,334 reads, 38,302 decodes against 39,275, the memo
hitting 54% in both), so the move is the decode's bytes alone, a group
of 1.8 KB where a page's 7 KB was. The point reads, the threaded reads
and the scans read level with the faults level (point reads 0.95-0.98x
in one order and 1.01-1.03x in the other, the one-thread scan 0.94x and
then 1.07x, the three-thread scan 1.38x and then 0.85x, which is the
threaded cell's spread), because none of them read the form: in every
rep of both arms the controller went SUSPECT 1.5 s into the read pass,
sampled for nine seconds, started the build sixteen seconds after the
read pass had ended and installed twenty-seven seconds later, inside
the threaded scans, so the read passes paired the raw form against
itself (0.26 faults a read, the raw rate the census has read at this
rung since the model was fixed). The scans' counters say what the cut
costs where the form is read in bulk: the memo misses 4.1x and 4.0x,
the groups a scan crosses 4.3x, each a decode of a quarter the bytes.
`supdb-nocachezpagesonegroup`, the forced arm with the page as one
group, is the pair that prices the cut on a point read through the
form from the pass's first read (`zgroup-forced-5m.sh`, below).

The cut against LMDB with its readahead off (`zgrouplmdb5m-ab-{1,2}.*`):
A 5.1x and 4.7x, F 3.8x and 3.4x, B 1.8x and 1.8x, D 1.6x and 1.5x, C
1.16x and 1.09x (2 of 3 and 1 of 3), E 0.64x and 0.71x; the point reads
0.41x and 0.40x at 3.1-3.5x LMDB's faults and the one-thread scans
0.13x and 0.16x at nine to ten times, the raw form's passes as above.
Beside the sitting before the cut (C 0.53x and 1.33x, D 0.82x and
0.74x, E 0.55x and 0.62x), the mixes a point read dominates moved by
about the in-process factor the cut pairing read; the comparator moves
between sittings, so the cut's verdict is the in-process pair's and
this table says where the arm stands.

The cut at a kibibyte against two (`zgroup1k5m-ab-{1,2}.*`,
`supdb-cache64tofitautozpagesgroup1k` against the combined arm): the
files 424 MB against 391, 8.2% again, the memo misses and the groups a
scan crosses 2.0x; the mixes 0.88-1.09x for the kibibyte in one order
and 0.98-1.22x in the other, B 0.88x and then 1.20x, so nothing the two
sittings resolve, and the point reads and scans level as above. Two
kibibytes stays: a second 8% of the file for a decode half the size that
no cell could see.

The cut on a point read through the form from the pass's first read
(`zgroup-forced-5m.sh`, `zgroupforced5m-ab-{1,2}.*`: the forced arm
against `supdb-nocachezpagesonegroup`, built at the load's sync, both
orders, same rung and bound): no fault in any pass of either arm, since
the forced arm's 361 and 391 MB of files fit the budget, so every cell
is the decode's. The point reads 1.47x and 1.45x for the cut on one
thread, 1.81x and 1.86x on two, 1.81x and 1.83x on three, 3 of 3 pairs
in every cell of both orders, the read's p99 9.3 µs against 13.6 and
10.7 against 13.1; the one-thread read at 263k and 258k a second
against 179k and 178k is 1.8 µs a read saved of 5.6, where the probe's
split at four had priced the decode 3.6 µs to 1.0 (the memo serves a
sixth of a group's reads either way: 525k hits of 3.27M reads against
281k). The mixes C 1.82x and 1.81x, D 1.74x and 1.55x, B 1.52x and
1.45x, A 1.43x and 1.09x, and E 0.98x and 1.06x and F 1.04x and 1.10x
level, E being the scans' and F the writes'. The scans 1.25x and then
0.89x on one thread and level threaded, with the groups a scan crosses
3.1x and the memo misses 2.4x: the scan decodes the same bytes in four
pieces and pays the fence and memo look per group, and the two sittings
disagree on its sign. So the cut's price is 8% of the file and nothing
a scan cell resolves, and its saving is the point read's 1.5x in core
and the mixes a point read dominates.

## The form files built ahead of the shift

The gear shift at five million keys under 640 MiB took 36-49 s from the
load's end to the install, twenty-odd of them the rent and the sampler
and thirty to fifty the build re-reading 620 MB of raw records from the
device beside the readers' faults. A form file on disk is bought at the
attach's price (the restart path, `zselect::Sampler::c_build`), so the
change builds the form files ahead and leaves them dormant
(`Options::zpages_prebuild`, off): once the store's raw populations pass
its budget, the model's own test, and nothing is landing or merging, one
partition at a time, the newest first, on the builder's idle-class
thread, written to disk unattached while the raw form serves; the sampler
waits for them, so its reading finds every file on disk; only before the
first install, since the exit from COMPACT retires the files while the
populations stay past the budget. `tests/zpages.rs` holds a store past
its budget to building every file ahead, unattached, with no read through
the form, the sampler held while a build ahead is held, and the install
an attach with no build started past the ones made ahead; a store that
fits to building nothing; and a store that left COMPACT on the trial and
retired its files to building none again. Five mutants of the mechanism
(the install guard, the dormant collect, the sampler's wait, the budget
test, the cancel of a dormant build) are each killed by those tests. The
store's drop lifts the test's build holds now, as it lifted the seal's:
a test that failed under a build hold hung in the builder's join.

Refuted (`zprebuild5m-ab-{1,2}.*`, `supdb-cache64tofitautozpagesprebuild`
against the combined arm, three pairs after the warmup, both orders; the
second order rerun on the same binary after a container restart cut it).
The raw read pass read 0.66x and 0.65x, 3 of 3 pairs in both orders, its
faults a read 1.53x and 1.50x: the builds ahead ran beside the read pass
and read the raw records back from the device beside its faults, the
contention the change was to take off the shift, moved into the pass
before it. The install came 36-37 s after the load's end against the
base's 41, since the sampler's nine seconds still followed the builds and
the warm of the attached files took eight; the two-thread scans read
1.20x and 1.22x for it, landing after the earlier install, the
three-thread scans 1.36x and 0.99x, and the mixes level in both orders
(B 0.85x and then 1.18x, the rest 0.95-1.16x). So a build after the load
is a build under the readers whenever it runs; what would take it off
everyone's path is a build while the raw records are still in the page
cache, at the landing, which the forced arm does and pays for at the
load, and the budget test does not pass until the load's last partitions
at this rung. The arm stays as the record, off as shipped.

## E's start seek skipped for a block held as a copy

E under the cut, profiled with DWARF unwinding over one rep of the
combined arm at five million keys under 640 MiB (the E window cut by its
phase marks on CLOCK_MONOTONIC, the main thread's samples): the seek
through the form file 25% inclusive -- the scan's own start seek 19%
(`scan_blocks` into `cache_seek_exact`), the settle's resolves of the
earlier mixes' 49,035 unsealed keys 7% -- LZ4 decompression 27%, half of
it under the seeks, the block forms' builds 11%, the page view's checks
6% (`view_checked`, at every group open, the memo's hits included), the
fence's search 5%, and the harness's Zipfian generator about ten, which
LMDB's E pays alike. The settle's phase timer reads 60-68 ms of E's
roughly 500.

A copy is walked from the start key by its own search
(`CachedBlock::lower_bound`), so the scan's seek decoded a group to learn
a rank only the block of which it used. `Options::zpages_seek_span` takes
the block from the fence and the page's directory where the start's
group lies in one block (`ZFile::owner_span`, no group opened) and seeks
the rank only for a form walked by rank; `span_starts` counts the scans
that skipped it, the windows test is held to a positive count with the
option on and none with it off over a dense run of updates that straddles
a block boundary, and the seek's unit test holds the span to the exact
owner for every probe. Three mutants (the span as the group's first
block, the span short by one, the copy's start taken at an unsought rank)
are each killed.

Level (`zspan5m-ab-{1,2}.*`, the arm against the seek for every scan, both
orders): E's group decodes fell 7% (163,719 against 176,834 a mix) and its
memo looks 12%, and E read 2-3% slower with the span in both orders, 1 of
3 pairs each; every other cell moved with its position, the second arm of
a pair faster in both sittings (the scan pass 1.15x for the span and then
1.09x against it, B 1.13x and then 1.30x against). The seeks it skipped
were mostly memo hits on the Zipfian head, worth about thirteen thousand
decodes a pass, some 13 ms of E; the span's own fence and directory
lookup, with a second seek for the 44% of starts whose block is not a
copy, cost what that saved. The option stays off, the arm
(`supdb-cache64tofitautozpagesspan`) as the record.

## The memo's slots hashed

The memo's slot was the group's number in the form file (the page times
sixteen, the most groups a page holds, plus the group) plus a multiple of
the identity, mod 4,096. Under the shipped cut a page holds about four
groups, so twelve of every sixteen numbers belonged to indices no page
had: a form file's groups reached a quarter of the slots, and pages 256
apart collided -- about 1,024 entries, 1.8 MB, where the cap is 8 MB.
`Options::zpages_memo_hash` (on) hashes the group's number and the
identity by a Fibonacci multiply over every slot; the memo's unit test
finds its collisions over the identities and the pages rather than
assuming them, and holds a form file of the shipped cut to reaching more
than half the slots, where the shape before reaches exactly a quarter.
`supdb-cache64tofitautozpagesmemolinear` is the shape before.

Priced (`zmemo5m-ab-{1,2}.*`, both orders): the counts per mix are the
same to the unit in both sittings, the hash decoding 12% fewer groups in
C (34.5k against 39.3k a mix), 13% fewer in B, 5% fewer in E's scans
(167.9k against 176.8k), D the same (its reads are uniform over the tail),
and the point reads' decodes 7% fewer over the run, 3 of 3 pairs both
ways. The rates moved with their position in both sittings, the second
arm of a pair slower whichever it was (A 0.89x for the hash and then
1.11x, E 0.90x and then 1.05x, F 0.86x and then 1.02x), so none resolves;
the change does strictly fewer decodes by a count that does not move, and
stays on. E's 5% says its hot set outgrows the memo's eight megabytes
however the slots are spread: what is left there is the cap against the
page cache, which the promotion's refutation says this bound has none to
spare for.

## The settle's cuts left to the builds

`Options::zpages_settle_span` (on) files a written key under the block the
form file's fence and the page's directory name (`ZFile::owner_span`) where
that block holds no form or a wide one, with its cut left unresolved
(`LAZY_CUT`); a build that reads the block's filed keys resolves them for
that block (`BuildCtx::filed_cuts`), and a wide walk takes the cut as the
overlay's unknown one and places the key by key, as it already did for a
snapshot key whose cut was never taken. `supdb-cache64tofitautozpagesexactcut`
resolves every cut at the settle through a seek, the shape before.
`scratch/wave2/zcut-5m.sh`: five million keys under 640 MiB, three pairs
after the warmup, both orders, the per-mix counters on.

E's settle (the scans' `settle` phase, ms, per rep, 8 reps an arm over the
two sittings):

| arm | settle | groups decoded |
|---|---|---|
| exact | 64.0-85.3 (median 68) | 167,839-167,934 |
| lazy | 39.1-43.6 (median 40) | 147,035-147,133 |

Every rep of both orders, 27 ms off a settle of 68 and 20,800 fewer group
decodes (12%). The lazy cuts filed: 40,878 of the 44,857 distinct keys; the
builds resolved none of them, since E walks the blocks it meets wide and a
wide walk needs no cut. No other mix runs the settle (A, C, D and F read 0.00
in both arms; B's upkeep passes decode 13 groups against 72). E's pass did
not move by a margin the reps resolve: 503-588 ms lazy against 509-598
exact, the scans' time 458-535 against 466-553, a spread of 40 ms a rep
around a saving of 27. The table's other cells move with the column, not
the arm (the scan pass's single thread lower in the second column of both
sittings, its three threads higher in both, D higher in both), which is the
pairing's position effect; the mechanism reaches none of them. Kept on: a
phase that only E runs, shorter in all sixteen reps, and nothing it reaches
worse.

## A race in the promotion arm, found by the fuzz under load

The differential fuzz with the form forced, the cache on and the blocks
promoted to copies in their slots (`ZFUZZ_ZPAGES=1 ZFUZZ_CACHE=all
ZFUZZ_PROMOTE=1`, the arm `supdb-cache64tofitautozpagespromote`'s option,
`Options::zpages_promote_copies`) fails a debug assertion in the settle's
apply -- "a block the writer holds no form for has none published as the
block" -- in seed 48 of one run over 0..120, and in seed 43 (seal 2 KB,
partitions of 4 KB, `Upkeep::Background(2)`, the segment work on its own
thread) in 3-5 runs of a hundred with four processes running the seed at
once. It is older than this session's changes: the same stress fails 5 of
100 on b5ea3ae, and 3 of 100 with the settle's lazy cuts on and off alike;
and it is the promotion's: the seed passes 100 of 100 under the same
stress with the promotion off. In a release build the assertion is gone
and the state it guards is a block the writer holds no form for whose
published form a handle may take, the shape "a form dropped by its writer
stayed published" names, so it is a wrong-answer risk in that arm. The
arm is off as shipped and was refuted on its rates.

Taken apart with an event ring of every publish, marker, retraction,
carry and stale look (removed after), the failing runs read the same
sequence, ending at partition 3, block 1: the writer publishes a copy into
gen 57; the segment work's landing carries gen 57's forms into gen 58, the
copy among them; the writer, its operation still holding gen 57, drops the
copy and publishes the marker into gen 57's slot, which the landing has
already copied; the writer's next look rebases its tables over gen 58,
finds the copy published where its table holds no form, and marks the
block dirty for the next publish (`rebase_tables`, the `(None, Some(f))`
arm); and the settle in the same look writes into the block before any
publish, with the copy standing, which the assertion catches. In a release
build with `commit_forms_build` on, the next publish replaces the copy
before it moves `forms_at`, and a handle trusts no form until then, so no
read takes it; with `commit_forms_build` off, `publish_dirty` returns
before publishing the dirty blocks and the position moves past them. The
look now publishes the marker at once, as its sibling branches do for what
they drop. `a_form_dropped_under_a_landings_carry_is_retracted_at_the_writers_look`
(`tests/db.rs`) leaves the state through a seam
(`Reader::forget_form_unretracted`: the table's form dropped, nothing
published), holds a seal's landing across it so the landing carries the
form, and commits a write into the block after a handle's scan: with the
look as it was it fails with the fuzz's own assertion, with the fix it
passes. The seed's stress: 9 failures in 220 runs before, none in 200 on
the fix with the event ring and the 400 below without it.

## E as it stands, profiled, and the page view read once

E on the combined arm (`supdb-cache64tofitautozpages`), profiled with perf
on cpu-clock and DWARF stacks over the mixes of two reps at five million
keys under 640 MiB (`scratch/wave2/../prof/profile-e.sh`: perf attached to
the harness at each mixes pass's start and detached at E's end, the
samples cut to E's window by the phase marks and to the main thread,
4,492 samples; perf slows E's scans about 1.3x, so the shares are read and
the times are not). The counts are the pairings': 79,198 scans, 1.33 walks
through the form a scan, 1.86 groups decoded a scan, 4.0 memo looks a scan
at a 54% hit rate, 24,007 blocks built, 78,631 block walks as copies,
30,410 as sparse forms and 18,321 clean or wide, the settle of the
earlier mixes' 49,035 writes at the first scan.

By self time: LZ4 decompression 26.5%; `view_checked` 8.5%; the fence's
binary search `entry_of` 5.2%; `scan_blocks` 5.1%; `open_group` 4.7%;
memcpy 4.4% (the copy walks' values into the harness's sink);
`cache_scan` 3.6%; the harness's Zipfian generator and clock about 12.5%
(`pow`, `exp`, `log`, `run::ycsb`, the vdso); `prefetch_block` 2.3%;
`group_seek` 2.2%; `Memo::recycle` 2.0%; the settle (`settle_each`, the
radix sort, `sync_log`) about 3%. By caller: `ZFile::seek` is 27% of the
thread inclusive -- 22% the scans' start seeks through `cache_seek_exact`
and 5.4% the builds' cuts through `part_cut_z` -- and inside it the group
open 15% (the start group's decode where the memo misses), `entry_of`
4.7%, the view 4.1%, `group_seek` 2.4%; `materialize` is 10.3%, of which
`cuts_of` 5.9%, a seek from the fence's top per snapshot key over a block;
the block walks' opens through `cache_scan` and `part_scan_z` 20.5%. The
view's 8.5% comes from `seek` (4.1), `group_of_index` (1.6), `owner_span`
(1.2, the settle's lazy filing), `open_group` (1.1) and `verified_group`
(0.5): two or three full validations of a page's directory per group
opened, since the group's index took one, the open took another, and a
miss's decode a third. In D, where every read decodes, the view is 15% and
`entry_of` 11.5%.

So E's remaining time on this arm is the decode (a quarter), the lookup
machinery around it (a third: the views, the fence search, the memo's
bookkeeping, the group seeks), the walk and emit (a fifth), the harness
(an eighth), and the one-off settle and builds (a seventh) -- and the
harness's share is LMDB's too.

**The page view read once, trusted** (`Options::zpages_trusted_views`, on;
`supdb-cache64tofitautozpagescheckedviews` and the forced
`supdb-nocachezpagescheckedviews` are the shape before). A verified page's
directory is read by `view_trusted`: the header's bounds and where each
group's key lies, none of the cross-checks between the directory's values
that the first decode made once and `view_checked` re-made at every open
(the keys ascending, the firsts ascending below the count, each payload
where the one before ends, the lengths summing); every slice a view hands
out stays bounds-checked where it is taken, a group whose first is past
the next's is no group (`PageView::group`), and the decode bounds the
group's length itself (`decode_group`). A read takes a page's view once,
for the group it wants and the open of it (`scan_in`, `seek`, `locate`,
`locate_rank`, the cursor's `open_at`), and the settle's lazy filing finds
each key's fence entry from the one before (`ZFence::entry_of_from`, a
gallop from the hint, the same answer as the search from the top for every
key and every hint, held by its test). Beside it, as an option of its own,
**the memo asked before any view** (`Options::zpages_memo_first`, off;
`supdb-cache64tofitautozpagesmemofirst` is the arm): a verified page's
group asked of the memo first, the memo's entry carrying the group's first
record index and count, so a hit reads nothing of the page, where the
shape before asks after the view, which says whether the group is
compressed. The unit tests hold the trusted view to the checked view on
every page of a built form file -- the header, every group's entry, key
and payload, the lookups by index and by key -- and to never panicking on
a page whose header, directory or keys were damaged after verification,
one byte at a time, whatever is asked of the view; a memo hit's first and
count to a whole-file scan and a seek of every key through the hits
answering as through the decodes, under the memo asked first and after;
and the three mutants (the key offsets left unset, the entry's first
stored as zero, the gallop stopped at the first entry at or below) each
kill their test. `tests/zpages.rs` runs the windows over a covered store
with the views checked and with the memo asked first as well, and the fuzz
turns the one off and the other on in some seeds under `ZFUZZ_OPTVARY`.

Counted exactly (callgrind over the suite's probes in core, a hundred
thousand keys, the forced form so that every read is the form's from the
first: `read_only` for the point read, `e_passes` for E's scans,
`supdb-nocachezpagescheckedviews` against `supdb-nocachezpages`, two
hundred thousand reads and thirty-two thousand scans a run, the memo asked
first in both). A point read through the form is 5,018 instructions with
the views trusted against 5,315 with them checked, 297 fewer (5.6%): the
view's 437 instructions a read became 144, and the decode (2,338), the
group seek (603) and the fence's search (573) are the same to the
instruction, as they should be, since the change touches nothing of them.
An E scan on the nocache arm's merge path is 78,167 against 80,119, 1,952
fewer (2.4%), of which the cursor's page open 1,414. The memo asked first
moves no count: it reorders a lookup and a touch.

Paired, first as one change -- the trusted views with the memo asked
first, both on, against the shape before (`scratch/wave2/zviews-5m.sh`
and `zviews2-5m.sh`, `zviews5m-ab-{1,2}.*` and `zviews5mb-ab-{1,2}.*`:
three pairs after the warmup a sitting, both orders, twice). The counts
are identical in every rep of both arms -- E's 147k groups decoded and
170k memo hits, C's 83,334 reads and 34.5k decodes, D's 77.5k decodes --
so the arms do the same work and nothing fell back to the raw form. The
rates read against the change: over the four sittings' twelve pairs a
mix, the arm with it on is slower in 9 of 12 on E (the scans' time, the
geometric mean 1.015), 8 of 12 on C (1.043), 7 of 12 on A, B and F
(1.02-1.04), 6 of 12 on D (1.013) -- 44 of 72 pairs, every mix's mean
against, 1-4%, where the floor of a sitting read off the passes before
the install, which both arms serve with the raw form and identical code,
is about a tenth (the scan pass 0.93x in one order and 1.12x in the
other). A lean of a few percent inside that floor, but the same way in
every mix across four sittings, for a change that executes 2-6% fewer
instructions: a cost the count does not show. Of the two parts, the view
is nearly the whole of the instruction saving and touches the same bytes
in the same order; the memo asked first saves nothing by instruction and
moves the memo slot's load ahead of the page's first touch, two misses
that the shape before had overlapped under the view's work and the
reorder serializes -- a reading, not a measurement, since this guest has
no cycle counters (`CLAUDE.md`, profiling). So the change was split: the
views trusted ship, the memo asked first is an arm, off, and the trusted
views alone were paired against the shape before with five pairs a sitting
(`zviews3-5m.sh`, `zviews5mc-ab-{1,2}.*`).

The trusted views alone against the shape before (`zviews5mc-ab-{1,2}.*`,
five pairs after the warmup a sitting, both orders): the counts identical
again, and the rates mixed inside a floor that read the scan pass of
identical code at 0.905x in one order and 0.994x in the other, the pairs
swinging a quarter either way. Over the ten pairs a mix, by the gets' and
the scans' own time, the trusted arm is faster on the two mixes that read
the form most -- C 4 of 10 pairs slower, the geometric mean 0.980; E 4 of
10, 0.984 -- and slower on the rest: B 8 of 10 (1.067), A 8 of 10 (1.063),
D 7 of 10 (1.046), F 6 of 10 (1.052); 37 of 60 in all. No mechanism puts
the trusted view behind the checked one on A, B, D or F and ahead on C and
E -- it reads the same bytes in the same order and executes strictly fewer
instructions on every path, and D, all misses, is where it saves the most
by count -- so the split is read as the host's, and the lean the two
pairings before carried is left with the memo asked first, which the
split's pairing did not run. What ships: the views trusted, on, by the
exact count, with the rates unresolved and the two heaviest readers of the
form for; the memo asked first, off, with its lean recorded. The rule the
session adds: a change that reorders two loads is priced in time as well
as by instruction, since the count cannot see a miss moved from under a
stretch of work to ahead of it; and a perf share attributed to a function
is partly the miss that function happens to be the first to touch, so a
share cut out by instruction is not a share of time saved until a pairing
says so -- here `view_checked`'s 8.5% of E's samples bought 2.4% of E's
instructions and nothing the rows can see.

## A block read through one cursor

The profile of E on the combined arm (the section before) put the builds'
cuts at 5.9% of the main thread and the block walks' opens through
`cache_scan` at 20.5%, and the code behind both was a read of the same
group several times over. A block's build or walk over a covered partition
read the partition in runs between the overlay's keys, and each run went
through `Reader::cache_scan`, which made a `ZCursor` at the run's rank --
the fence's entry by rank, the page's view, the directory's group by
index, the group opened through the memo or decoded, the records parsed
from the group's start to the rank -- and dropped it at the run's end; the
cut of each overlay key, where the run ends, was a seek from the fence
(`cuts_of` for the snapshot's keys at a block's first build,
`filed_cuts` for the filed keys at every build, `cache_cut` for a key
neither had resolved), which opened the key's group again; and the record
an overlay key replaces was read by rank through `cache_values`, which
located and opened the group a third time. Over a block of sixty-four
records in four or five groups with `k` overlay keys that is about `2k + 5`
group opens and `k` fence searches for the four or five opens the walk
needs, and the counters put E's builds at 13.2 memo asks a block (24,007
blocks built a pass, 316,780 asks). The sparse walk under
`forms_pieces_only` (`walk_deltas_over`, an arm's path) had one more: it
ordered the form's deltas against the memtables' keys by cut, and sought
the overlay key's cut at every turn of the loop before the key was taken.

**The block read through one cursor** (`Options::zpages_block_cursor`, on;
`supdb-cache64tofitautozpagesperrun` is the shape before). A build or a
walk holds one reader over its partition for the block (`db::PartReader`
over `zfile::ZCursor`): the runs between the overlay's keys are the
cursor's walk (`walk_until`), consecutive, so the cursor stands where the
next run begins less the record an overlay key was emitted in place of,
which it steps over; an overlay key's cut is where the walk stops at the
key (`walk_until(Some(key))`, the records below it emitted on the way),
which is `cache_cut`'s answer with the records read once; and the record
the key replaces is read from the cursor standing on it (`ZCursor::values`),
as the overlay walk of `scan_partitions` reads it. A sparse build
(`deltas_for`) reads no records, so its cuts are the cursor moved to each
key from the group it holds (`ZCursor::skip_to`): the fence searched from
the entry held (`entry_of_from`), the page's directory for the group, the
group kept where it is the one held and the position is not past the key,
and the records parsed forward from the position -- and where the position
is the cut of the key before and its record is not below this key, the cut
is the position and nothing is searched, since every record before it is
below the key before. The cuts of a covered partition's memtable keys are
left to the build under the option (`cuts_of` and `filed_cuts` resolve
nothing, `Over::cut` reads "the build searches"), and the pieces-only
sparse walk orders its two lists by key, which is the order of their cuts
(the two keys' cuts order as the keys do, and at one cut the memtable key
first where it is not above the form's, as before), so it seeks nothing
before a key is taken; the arm keeps that order too, so the pairing prices
the cursor alone. A fall -- a page that fails a check under the cursor --
files the cursor's counts and hands the rest of the block to the dispatch
as before, which reads the raw form. The counts are filed as a partition
walk's are (`pwalk_close`): a block's walk is one served walk now where
each run was one, so `zpage_scans` falls by design, and the groups decoded
and the memo's asks are the mechanism's measure.

The unit tests hold the cursor moved to a key to the seek a key
(`ZFile::cut_in`) over the suite's shape and over every record shape --
every record's key, a key between every two records, below the first and
past the last, in key order from an unplaced cursor and then in reverse
order from another -- and count the memo's asks: one per compressed group
of the file in key order (a stored group is parsed where it lies and asks
nothing), where the seeks ask once a key; a cursor moved onto a page that
fails its first decode answers every key before the page, `None` for one on
it, and has fallen with the form file out of service. `tests/zpages.rs`
runs the windows over a covered store with a cursor a run and a seek a cut
as well, and `tests/db.rs` holds the pieces-only sparse walk with the
compact form to the overlay model; the fuzz turns the cursor off in some
seeds under `ZFUZZ_OPTVARY`. Five mutants each kill their test: the cursor's
fast path without the key order (the reverse-order probes), the held
group's check without the position's (the same), the step over a replaced
record removed (six windows tests), the pieces-only merge taking equal
keys apart (five pieces-only model tests in `tests/db.rs`; the windows
tests and forty fuzz seeds let that one through, since the path is the
arm's -- `walk_deltas_over` runs under `forms_pieces_only` alone -- which
is why the pieces-only model now runs with the compact form too), and the
step from a spent group without the resume (the lazy end's test, below).

Paired against the shape before (`scratch/wave2/zblockcur-5m.sh`,
`zblockcur5m-ab-{1,2}.*`: five pairs a sitting after the warmup, both
orders, the per-mix counters on, read by `pairing_read.py` as the k-th rep
of each arm against the k-th of the other). The counts first, deterministic
across the reps of an arm to a few hundredths of a percent and identical in
every mix but E, which alone builds blocks: E's walks the counters see fall
from 104,943 a pass to 57,904, since a block's walk is one where each run
was one; its memo asks from 316,780 to 256,593 (the hits from 169,682 to
108,430, the groups re-opened after the walk had opened them, which the
memo held); and its groups decoded rise from 147,098 to 148,162, 0.7%, with
24,007 blocks built in both. The rise named a cost of its own: the cursor's
bulk walk opened the next group whenever a run ended at a group's last
record -- the limit reached and the group spent in one step -- where the
per-run walk this replaced opened nothing past its last record, and a
block's last record ends a group once in fourteen; the opens of groups
past a block's end are misses as often as not. The reader's walk leaves a
spent group to the next read now (`ZCursor::walk_run`, with `resume`
called by every read from the position, and the unit test counting the
memo's asks across a walk that ends at a group's last record); the overlay
walk's `walk_until` keeps the cursor on the next group's first record, as
its test holds and its caller reads, and pays that open at a scan's end
still -- the no-cache arm's lever, unpaired, on a path a scan with the
block cache never takes (it returns from `scan_blocks`), so the pairing
below prices the committed code exactly. The pairing was run again. The
rates of the first pairing, over the twelve pairs a mix by the mixes' own
times: E's scans 0.923 of the shape before, slower in 2 of 12 pairs, in
both orders (0.900 and 0.945 by sitting); C, which builds nothing and
reads the form by point reads alone, 1.000 (7 of 12), the control the
pairing carries; B 0.958, A 0.953, F 0.942 and D 0.965 by total, the
update mixes' commits 0.93-0.94, since a commit's own fill of its batch's
blocks is a build; the floors from the passes before the install, identical
code, read 1.008 and 1.084 in one order and 0.976 and 0.960 in the other.

Paired again with the walk leaving a spent group to the next read
(`zblockcur2-5m.sh`, `zblockcur5mb-ab-{1,2}.*`, the same shape of
sitting): E's groups decoded read 147,097 a pass against the shape
before's 147,098, equal to within a group, so the rise was the eager open
and nothing else; the memo asks 254,358 against 316,780, a fifth fewer;
the walks 57,904 against 104,943; 24,007 blocks built in both; and every
other mix's counts identical, as before. The counts move by about seventy
across the reps of either arm, the same range in both, which is the memo's
state from the mixes before. The rates over the twelve pairs: E's scans
0.977 of the shape before (5 of 12 slower; 0.968 and 0.985 by sitting), C
1.003 (5 of 12), the update mixes' commits mixed this time (A 1.03, F 1.12
with 10 of 12 slower, B 0.85, D 0.98), the floors from identical code
0.96-1.06. Over the four sittings of the two pairings E's scans read for
the change in every sitting, by 1.5% to 10%, with the control level in all
four, and the commits' column swung both ways between the pairings: so the
rates say E moves the way the counts say, by an amount the host decides
sitting to sitting, and the figure that stands is the counts' -- a fifth of
E's memo asks and nearly half of its walks' opens gone, the decodes
unchanged, the same answers (the windows, the model and the fuzz). What a
memo ask costs is a hash, a compare, an `Arc` clone and its return to the
recycler, and the fence and directory searches a seek made before it, a
few hundred instructions each, so sixty thousand of them are about a
hundredth of E's pass; the lever's larger half, the walks' opens, saved
nothing by decode because the walk had just put the group in the memo.

The rule the lever adds: count a path's opens as a block pays them -- the
walk's, the cuts' and the replaced records' together -- before pricing any
one of them, since the profile named the cuts (5.9%) and the opens the
walk itself repeated were the larger part by count; and a change that
holds a group across a walk is read in the decodes as well as the asks,
since a group opened for a read that never comes is a miss as often as
not. What E pays now, by the counts, is the decodes themselves -- 147,000
groups a pass of 105,000 scans, 1.4 a scan, 46% of the memo's asks missing
-- and the memo's size and admission (`next/README.md`) are the levers on
that; the decode's own cost is the codec's.


## The memo's size, counted

The handle's memo of decoded groups (`zfile::ZMemo`) is direct-mapped, four
thousand and ninety-six slots under eight mebibytes, and with the block
read through one cursor E asks it 254,000 times a pass at five million
keys under 640 MiB and misses 147,000 of them: a decode a miss, 1.4 a
scan, the largest single cost E has left by the profile (the LZ4 decode a
quarter of the main thread before the cursor, more after it). The memo's
size is now an option (`Options::zpages_memo_bytes`, eight mebibytes as
shipped, `ZMemo::sized` scaling the slots with the cap at the shipped ratio
of a slot per two kibibytes), with two arms, `supdb-cache64tofitautozpagesmemo16`
and `...memo32`. A larger memo is memory the bound takes from the page
cache that holds the form files, so the count comes first and the rent is
the pairing's to price.

Counted (`zmemo-count-5m.sh`, `zmemo{16,32}count5m-ab.*`: one rep of the
combined arm against each size, the per-mix counters on; the counts are
deterministic to a few hundredths of a percent). Groups decoded a pass:
E 147,083 at eight mebibytes, 134,933 at sixteen (8.3% fewer), 125,950 at
thirty-two (14.4% fewer), the hit rate 42%, 47% and 50%; C 34,466, 30,488
(11.5% fewer) and 29,023 (15.8% fewer), the point-read mix's groups being
the Zipfian head's, which a larger memo holds more of. The other mixes
follow C in proportion (B's decodes 32,470 at eight). Over the whole run
the point reads' decodes fall 7.7% and 12.4%, and the scan passes' groups
move 0.4% and 2.7%, since a scan of a cold store streams through groups the
memo cannot help with. The main thread's minor faults in C rise from 1,600
to 4,200 and 8,000 a mix, the memo's larger buffers first-touched as it
fills, a few milliseconds a mix at this guest's price for a fresh page; the
point reads' major faults read level in the one rep (0.98x and 0.96x of
the shipped size), which is the question the pairing has to answer with
more reps, since the page cache the memo takes is the form files'.

Paired, sixteen against eight (`pair-5m.sh zmemo16 ...`, `zmemo16-ab-{1,2}.*`,
five pairs a sitting, both orders, read as the pairings before). The counts
as counted: E 135,050 groups decoded against 147,093, C 30,506 against
34,546, the memo's asks the same within a hundred. The rates over the
twelve pairs: E's scans 0.997 (7 of 12 slower; 1.037 in the sitting where
the larger memo ran second and 0.958 where it ran first, the sign with the
order, so a position effect and no more); C's gets 1.007 (7 of 12; 1.024
and 0.990 by sitting); D 1.065 by total, slower in 9 of 12 and in both
orders (1.030 and 1.093), the uniform reads that hit the memo once in
eighty asks and pay its larger footprint for nothing; F's gets 1.062 (8 of
12), B 0.961 and A 0.977 the other way; the floors of identical code read
1.025 and 1.066 in one order, 0.976 and 1.014 in the other, and the point
reads' major faults 0.974x and 1.017x, level. So eight percent of E's
decodes and twelve of C's, taken away, moved neither mix's time: a decode
saved by the memo is a group read cold from the memo's buffer instead,
thirty-two lines of it for the parse that follows, where the decode writes
the group into the thread's buffer and the parse reads it hot -- a reading
the cycle counters this guest lacks could settle and the pairing cannot,
and the one that fits a memo whose hits cost what its misses cost. The
main thread's minor faults rise by two thousand a mix in C and nine
hundred in E, the larger buffers first-touched, a millisecond or two
against mixes of 140 and 550.

Thirty-two against eight (`zmemo32-ab-{1,2}.*`): the counts as counted
again, E 125,940 groups decoded against 147,100, C 29,036 against 34,512,
D 74,730 against 77,525 with its hits 3,760 against 960. The rates over
the twelve pairs: C's gets 1.115, slower in 10 of 12 and in both orders
(1.023 and 1.216 by sitting), the mix that saved the most decodes by
share; D's gets 1.066, slower in 10 of 12 and in both orders (1.114 and
1.020); E's scans 1.026 (7 of 12; 1.083 and 0.972, the sign with the
order); B 1.019, A 1.010, F 1.028 by the gets; the floors of identical
code 0.976 and 0.995 in one order, 0.999 and 1.007 in the other, and the
point reads' major faults 0.997x and 0.987x, level again. So the memo
that decodes a sixth fewer groups in C reads C a tenth slower in both
orders, and the one that decodes a seventh fewer in E reads E level. The
page cache is not what it costs -- the faults say so -- and the decodes
saved are real; what the larger memo adds is the footprint its hits read
through: a hit hands the walk a group whose lines were last touched when
the memo took it, and a memo of thirty-two mebibytes beside the fence,
the directories and the raw records the point reads touch is more than
this guest's last-level cache holds, so the hit's thirty-two lines come
from memory where the decode's came from the buffer the decode had just
written. Eight stays, and the two sizes are arms.

And four against eight (`zmemo4-ab-{1,2}.*`), to read the other side of
the knee: E decodes 155,180 groups a pass against 147,100 (5.5% more, the
hit rate 39% against 42%), C 38,750 against 34,530 (12% more), D 77,990
against 77,525. The rates: E's scans 1.078, slower in 8 of 12 and in both
orders (1.030 where the smaller memo ran second, which the floors favoured
by six percent, and 1.128 where it ran first), A's total 1.080 (10 of 12,
both orders), D 1.061 (8 of 12, both orders), C 0.977 (2 of 12; 0.934 and
1.023, the sign with the order), B 0.989; the floors read the second arm
faster by 4-8% in both sittings, so the position effect is of that size
here, and E's loss stands beyond it. So the memo pays for the groups that
come back while they are still warm -- the Zipfian head's, which four
mebibytes hold short of and eight hold -- and not for the ones that come
back cold, which sixteen and thirty-two add: a hit on a cold group costs
the walk its thirty-two lines from memory, about what the decode would
have, and the larger memo's footprint costs the point reads besides. Eight
is at the knee for this shape and stays; the three sizes are arms. The
rule the sizes give: a cache of decoded things is priced by the warmth of
its hits and not by its hit rate, so a size is paired at both sides of the
shipped one before the hit rate is read as the lever, and the count that
would decide it here -- the lines a hit's parse misses against a decode's
-- is one this guest's missing counters cannot give; a micro-probe over
more distinct groups than the last-level cache holds, timed as the decoder
was (`bench/examples/zdecode.rs`), is the instrument to build if the memo
is taken up again.

## The per-record form, probed against the comparators

The question put was whether a form whose records decode on their own
beats LMDB and RocksDB on a cell the engine loses, before it is built into
the engine. `bench/examples/zrec.rs` writes the suite's records (sixteen-byte
keys in order, the suite's hundred-byte values) as such a form -- groups of
sixteen records, the group's first key raw, every record's key front-coded
against it as the shared prefix's length and the suffix, every value
compressed on its own with LZ4 against a dictionary of the segment's first
sixty kilobytes of values, and a table of sixteen record offsets at the
group's head -- maps it, advises it random, and runs three read-only mixes
against it, the forced group form (`supdb-nocachezpages`), LMDB with its
readahead off and RocksDB, in one process with the arms interleaved within
a round: 200,000 uniform point reads (the suite's read pass), 200,000
Zipfian point reads (C's) and 20,000 Zipfian scans of fifty (E's scans
without its inserts), each timed with the process's major faults around it
and the bytes it answered. The byte totals are the check that the arms
answered the same question: every arm reads 20.0 MB a point-read mix and
100.0 MB a scan mix with zero misses, in every round of both runs. A point
read decodes one record, found by binary search over the group's offsets
with the key compared through its prefix length and suffix; a scan decodes
exactly the fifty records it emits, assembling each key. What the probe
does not do: inserts (D's and E's five percent, and the updates the mixes
before E leave at the Zipfian head), the controller, the block cache, any
write path; so it prices the form's own cost per read and per scanned
record and nothing of the overlay walk. The runs are `zrec-incore-5m.log`
and `zrec-bound-5m.log`, read by `zrec_summarize.py` (medians over the
rounds, and the median of the per-round ratios to LMDB).

The form is the group form's size: 361.9 MB at five million keys, 72.4
bytes a record keys included, against the form file's 360 -- the
dictionary finds the value's fifty-byte motif where the group's
compression found it in the neighbouring records. The probe's build takes
73-85 s, fifteen microseconds a record, because `compress_into_with_dict`
rebuilds the dictionary's hash table on every call; a builder in the
engine keeps that table across records, and the figure is the probe's and
not the form's.

In core (three rounds; medians, with the per-round range of the ratio to
LMDB): the per-record form reads a uniform point read in 1.87 µs, a
Zipfian one in 0.84 and a scan of fifty in 4.16; the forced group form
5.08, 1.68 and 6.06 with 1,147 MB of files (the raw records, the form and
the index); LMDB 1.65, 0.79 and 2.02 with 665 MB; RocksDB as tuned 6.90,
4.10 and 16.16 with 584. Against LMDB the per-record form reads 1.12x
(1.03-1.13) on the uniform reads, 1.01x (0.95-1.16) on the Zipfian and
1.92x (1.42-2.18) on the scans: level on point reads, since both are
bound by the lines a read misses and neither decodes much, and half
LMDB's speed on scans, which is fifty decodes of about sixty nanoseconds
each where LMDB's cursor steps and copies. Against the group form it is
0.37x, 0.50x and 0.69x, all of it the decode: the group form decodes seven
kilobytes for a record of 116 bytes.

Under the bound (640 MiB, five million keys, three rounds): the per-record
form reads 1.80, 0.75 and 3.68 µs with no major faults in any mix, since
362 MB fits the cgroup whole; the forced group form 5.83, 1.78 and 6.39
with 0.006, 0.001 and 0 faults an operation; LMDB 15.41, 5.61 and 8.37
with 0.186, 0.056 and 0.070 faults an operation, its 665 MB tree past what
the group holds; RocksDB at its default options (`rocksdb-nosync`, whose
reads are a durable RocksDB's and whose policy touches the load alone,
since the tuned arm's 256 MB block cache does not survive this bound)
22.01, 11.32 and 36.52 with 630 MB of files and no major faults, because
it reads with `pread` and its misses are read calls, so only its time
counts. The ratios to LMDB, median of the per-round ratios with the range:
the per-record form 0.12 (0.10-0.16) on the uniform reads, 0.15
(0.12-0.16) on the Zipfian, 0.44 (0.37-0.58) on the scans; the group form
0.38 (0.38-0.43), 0.33 (0.31-0.37) and 0.77 (0.64-1.02); RocksDB 1.57,
2.12 and 4.36. The per-record form against the group form, same process,
same rounds: 0.31 (0.27-0.38), 0.42 (0.36-0.48) and 0.58 (0.43-0.74). So
under the bound the per-record form reads six to eight times faster than
LMDB on point reads and 2.3 times on scans, ten to fifteen times faster
than RocksDB, and three times faster than the engine's own form on
uniform reads, 2.4 on Zipfian reads and 1.7 on scans, in every round --
and since both supdb forms fit the bound and fault nothing, the whole of
that last gap is the group decode.

What it shows and what it does not. The record above ("Against the
comparators out of core") already has the forced group form winning these
read-only cells against LMDB under the bound, and the probe agrees (0.33-0.77x
of LMDB's time); the cells the engine loses there are E, at 0.55x, and the
controller arm's passes before its install, and E's loss is in the mixes'
shape -- its scans cross the unsealed updates the mixes before it leave at
the Zipfian head, and the settle and the builds are a seventh of it -- which
the probe cannot run. What the probe measures is the cost the form itself
adds to a read and to each record a scan emits, and that is what a
per-record form changes: the profile of E on the combined arm put the LZ4
decode at 26.5% of the main thread and `ZFile::seek` at 27% inclusive with
the scans' start seeks 22% of it, each start seek a group decoded to place
the cursor, and the overlay walk's re-entries after each unsealed key a
group each (1.8 opens a scan after the cursor kept its group). A record
decoded instead of a group at every one of those is the lever the shares
name; what it buys E is a measurement the suite has to make, not a figure
to be drawn from the shares.

The scan is where the per-record form pays, and in core it pays to LMDB:
fifty decodes a scan against a cursor's fifty steps. Three levers there,
none of them probed: the decoder (the C library at 0.83-0.91 of
`lz4_flex` cycling, "the decoder and the cut"), values stored raw with
only the keys coded (about 107 bytes a record, 535 MB at this rung, which
sits at the edge of this bound beside the process), and a value coding
cheaper than LZ4's token walk for records this short. Under the bound the
scan reads 0.44x of LMDB anyway, since LMDB faults on its tree.

Building it into the engine: the form file's page becomes a container of
per-record-decodable groups -- the page's CRC and fence entry as they are,
a directory of record offsets in the group, the first key raw and the rest
front-coded, the dictionary once per form file in the trailer -- the
cursor's walk decodes per record, a point read decodes one, the memo caches
nothing a point read needs (a decoded group is the thing it exists to
cache), and the builder keeps the compressor's dictionary state across
records. The fence, the directory of groups and the controller do not
change. The arm is the group form, which is what every bounded row so far
has measured.

## The dictionary and the value coding, probed

The two choices a build of the per-record form has to make were probed in
the same example before any engine code (`bench/examples/zrec.rs`, the
shapes; `zrec-shapes-suite-1m.log`, `zrec-shapes-drift-1m.log`, read by
`zrec_summarize.py`): a million keys in core, two rounds, 100,000 uniform
and 100,000 Zipfian point reads and 10,000 Zipfian scans of fifty a mix,
every arm answering the same bytes with zero misses, on the suite's payload
and on a drifting one. The suite's values are half a motif shared by every
value in the store and half entropy, the payload a segment dictionary
learns from any sixty kilobytes of it; the drifting payload keeps the halves
and gives every two hundred records a motif of their own (`--payload drift`,
`DRIFT_EVERY`), so a value shares its motif with twelve groups of
neighbours and with nothing far away -- the payload a segment dictionary
cannot learn and a group's own redundancy can, and the case the per-record
form's size parity with the group form had not been tested on. The shapes:
`zrec` (the segment's first sixty kilobytes of values as the dictionary),
`zrec-sampled` (six hundred values sampled across the segment), `zrec-local`
(no segment dictionary; a group's first record stored raw and the
dictionary for the fifteen after it), `zrec-hybrid` (the segment dictionary
with the group's raw first record appended, assembled per group),
`zrec-nodict` (every value compressed alone), `zrec-raw` (values stored raw,
only the keys coded) and `zrec-group` (the sixteen records compressed as one
block, the group form's shape at this grouping, the control). A value LZ4
does not shrink is stored raw under every shape.

Bytes a record, keys and headers included, suite payload then drifting:
`zrec` 72.3 and 108.6; `zrec-sampled` 75.8 and 106.2; `zrec-local` 80.5 and
75.6; `zrec-hybrid` 75.0 and 75.6; `zrec-nodict` and `zrec-raw` 108.5 and
108.6 (a hundred-byte value alone has nothing LZ4 can match: the motif's
window is shorter than the motif); `zrec-group` 72.3 and 70.1; the engine's
forced form 229.7 and 226.4 (its raw records, its form file and its index
together); LMDB 133.1 on both. So the segment dictionary is the suite
payload's: on the drifting payload it compresses nothing and the form is
the raw form's size, which out of core is half again the faults. The local
dictionary holds on both payloads, within 11% of the group control on the
suite's and 8% on the drifting one, because a group's first record carries
the motif its fifteen neighbours need, and the hybrid takes the suite
payload's 7% back (75.0 against 80.5) and matches the local dictionary on
the drifting one. The sampled dictionary learns nothing a first-values one
does not at six hundred samples over five thousand motifs.

Time, in core (medians of the two rounds; the ratio to LMDB, median of the
per-round ratios, in the logs). Point reads: every per-record shape reads
level with LMDB on both payloads -- 0.88-1.07x on the uniform reads and
0.73-1.14x on the Zipfian, the ranges overlapping -- against the group
control's 1.32-1.95x and the engine's form's 1.83-2.44x; the dictionary
decides nothing a point read's time can see, since one record is decoded
whatever the dictionary. Scans, suite payload: `zrec-local` 2.45 µs,
`zrec-hybrid` 2.74, `zrec` 2.90, `zrec-sampled` 3.66, `zrec-group` 3.29, the
engine's form 3.72, `zrec-raw` and `zrec-nodict` 1.73, LMDB 1.53; drifting
payload, where most values of the segment-dictionary shapes are stored raw
and decode as copies: 1.76, 1.83, 1.77, 1.38, 1.71, 3.99, 1.44 and 1.39,
LMDB 1.33. So a scan's time over LMDB's is the decodes it makes and nothing
else: fifty raw copies read 1.13x of LMDB, fifty decodes against a hot
hundred-byte dictionary 1.6x, against a sixty-kilobyte one 1.8-1.9x, and
the group control's three whole-group decodes 2.1x. The local dictionary's
decode is the cheapest of the compressed shapes by 10-15% on scans, the
dictionary being in the lines the walk already holds, where the segment
dictionary is sixty kilobytes the decode's matches reach into; two rounds
do not resolve that difference and the sizes do.

The choices, then. The value coding is LZ4 per record: raw values cost half
again the bytes for a scan a quarter faster in core, and out of core the
bytes are faults, so the compressed form is the one for the regime the form
exists for; the scan's remaining cost over LMDB is the decoder's per-record
price, 15-25 ns a record here, a lever for later (a decoder shaped for a
hundred-byte record, or the C library) and not a layout choice. The
dictionary is the group's own first record, stored raw: it needs no segment
dictionary in the trailer, no state kept across records at the build, no
per-group assembly at the read, and it holds the form's size within a tenth
of the group form's on both payloads where the segment dictionary holds it
on one. The segment dictionary appended to it is the arm, worth 7% of the
bytes on the suite's payload and nothing on the drifting one, with a trailer
slot for it that an empty dictionary leaves as the local shape. The rule the
study gives: a compression ratio measured on one payload is that payload's,
and a form whose ratio depends on where in the file a value's redundancy
lies is tested on a payload whose redundancy is somewhere else before the
ratio is read as the form's.

## The record coding in the engine

Built from the plan in `next/README.md` after the two probes, and changed
where the engine said otherwise. The form file's groups hold their records
one by one now (`Options::zpages_record_coding`, on; `zpages::ZCODING_RECORD`),
the choice per group in the high bit of the directory entry's decoded-length
word (`D_CODED`; a group's length never reaches the bit), the trailer at
version 2 so that a reader from before refuses the file and rebuilds it and
this reader refuses the files before, and the header's spare byte zero as it
was. A coded group's payload is an offset table of `u16`s, then each record
as its head (`[klen u16][n u16][key]`) front-coded against the first record's
head -- the prefix length and the suffix, as varints and bytes -- its image
length, its tail's stored length, and the tail (the image from its aligned
tail start) LZ4-compressed against the first record's tail, which is stored
raw as the dictionary; a tail LZ4 does not shrink is stored raw, and so is
every tail under eight bytes (`zpages::encode_record_group`). A group decoded
whole is its records decoded one after another into the bytes the block
coding decodes to (`decode_record_group`), which is how the verify at the
build, the first decode's checks, the memo and every reader of a decoded
group -- the cursor's walk among them -- read a coded group unchanged; a
point read of a verified page seeks the group's heads where they lie
(`record_group_seek`, the keys compared through the prefix and the suffix
without assembling them) and decodes its one record into the read's buffer
(`decode_record`), asking the memo nothing (`ZFile::locate`); the page is
still touched for the footprint, which the first version forgot and the
footprint test found. The block coding stays as the arms
`supdb-cache64tofitautozpagesblock` and `supdb-nocachezpagesblock`.

Two things the plan had wrong, both found by the existing tests. The coding
was to be the page's, in the header's spare byte; the integration test of
records larger than a page counted a third record no page held under it,
and it was the 5,000-byte key: the block coding compresses a key's run and a
head is never compressed, so a record alone whose head passes the page is
the block's if the block holds it. The fill tries the record coding first
and falls back to one LZ4 block for a single record only (`FillScratch::compress`);
a group of more records that passes the room is trimmed and tried coded
again, as the block coding's groups are trimmed, since the block compresses
a tenth tighter and would otherwise take every page's last group. And the
memo test's invariant, every read opened a page through the memo, is the
block coding's; under the record coding no read of a verified page asks
the memo and every read decodes its record, which the test asserts now for
that coding, with the memo's misses the pages' first decodes.

What the sizing tool says of it (`bench/examples/zsize.rs --coding record`,
`zsize-{block,record}-300k.txt`: 300,000 keys of the suite's shape, one
round, 400 sampled pages). The record coding's form is 14% larger: 86.97
bytes a key against 76.34, 47.1 records a page against 53.7, three groups a
page against four, one group in a hundred stored verbatim (the coding did
not shrink it) against none; at ten million keys 0.87 GB of pages against
0.76, 95% of the 937 MB the 1 GiB bound leaves against 83%, so the top rung
of the out-of-core ladder is a closer fit under this coding. The fill is
1.5x slower, 25.7 µs a page against 17.2 and 5.5 s against 3.2 at ten
million keys, one thread: LZ4's compress against a dictionary sets up a
table per call, once a record here where the block coding set up once a
group, and this is the lever on the build if the install's time is read as
the coding's. A group decoded whole through its records costs 0.649 µs hot
against the block's 0.271 and 1.631 cold against 1.014 -- per record about
43 ns hot, the decoder's per-call setup over a hundred-byte input -- which is
what the cursor's walk and a page's first decode pay for a group, while a
point read pays one record of it, about a sixth of the block's group hot and
less cold. The checks over a page (`verify`) read 3.1 µs against 2.3 for the
same reason. Nothing else moved: the walk, the parse and the CRC read the
same.

So the engine's point reads should gain by about the decode a read saves,
and its scans pay the decode constant on every group the memo misses, 1.6x
cold; the probe's scans read the other way (`zrec-local` 2.45 µs a scan of
fifty against the group control's 3.29, in core), where the probe's group
control decoded a group without a memo in front of it. What the suite's E
makes of the two is the pairing's to say, not this tool's, and the levers
if E loses are named: a decoder shaped for a hundred-byte record against a
dictionary (lz4_flex's per-call setup is most of 43 ns), the cursor decoding
the records it emits rather than the group, worth little for a scan of
fifty that crosses three groups, and the segment dictionary appended to the
first record's tail for the bytes.

Held by: the layout module's five new tests (round trip and seek against
the decoded group's over every record shape the module's tests know, the
forged payloads -- an offset into the table or past the payload, a prefix
past the first head, an image length not whole words or shorter than its
tail, a tail cut short, a first record coded against anything -- a page of
either coding read as the other's, a stored group never coded, the fill
under the coding building pages the reader decodes with a point read of
every record), the reader's every-key and cursor-against-seek tests under
both codings, the memo test's two invariants, the integration suite (100),
the fuzz under the option varied (`record_coding` off in a quarter of the
seeds under `ZFUZZ_OPTVARY`), and the test of a piece kept across a merge
holding the merge's landing (`Db::hold_merge_landing`), which bet a merge of
four hundred thousand keys would outlast a seal of twenty and lost once under
the gate's load.

## The record coding paired, and what E's loss was made of

The pairing (`pair-5m.sh zcoding supdb-cache64tofitautozpagesblock
supdb-cache64tofitautozpages`, `zcoding-ab-{1,2}.*`, the tree `a78ea7e`:
five pairs a sitting after the warmup, both orders, five million keys under
640 MiB, read by `pairing_read.py` as the arm each line names). Over the
twelve pairs the record coding reads the point-read mixes faster in every
pair: C's gets 0.689 of the block coding's time (0 of 12 slower; 0.715 and
0.664 by sitting), B's 0.704, D's 0.742, F's 0.831, A's 0.799 (1 of 12), the
mixes' totals 0.707, 0.756, 0.785, 0.981 and 1.003 -- the suite's ops a
second 1.46x, 1.32x and 1.33x on C, B and D in the first sitting, 1.51x,
1.37x and 1.34x in the second -- and reads E's scans at 1.262 (11 of 12
slower; 1.253 and 1.271 by sitting), E's total 1.270, with the write
mixes' commits 1.07 to 1.33 slower besides (A's 1.074 in 9 of 12, F's 1.065
in 8, E's 1.332 in 9). The counters: under the block coding 42% of C's
reads hit the memo and the rest decoded a group (48,822 hits, 34,512
misses of 83,334 reads); under the record coding no read asks the memo and
every read decodes its record (5 hits, 83,306 decodes), and the reads are
still a third faster, so a record's decode is cheap against the memo's hit
as well as against a group's decode. E decodes fewer groups under the record
coding, 130,861 a pass against 147,116, with the memo's hits 109,224 against
107,242, and pays about a microsecond more for each: 648 ms a pass against
503. A commit's fill of its batch's blocks decodes groups as well, which is
the write mixes' loss.

Where E's microsecond a group came from, by count. The sizing tool at
300,000 keys (`zsize-record-300k.txt`) read a group decoded whole through
its records at 0.649 µs hot against the block's 0.271 and 1.631 cold against
1.014, and callgrind over the tool's build with the collection toggled on
`decode_group` counted the record coding's decode at 2.2x the block's
instructions, 81.3M against 36.9M over the same records: `decode_record_group`
itself 26%, libc's memcpy 19% (three calls a record, of twenty to a hundred
bytes), the record's field parsing (`coded_rec`, `get_var`) 16%, the rest
bounds checks and arithmetic -- about 2,000 instructions a record against
the block decode's 920, which is LZ4's bulk copies at seven instructions a
byte against a structure that pays a fixed cost a record. That is 0.4-0.6 µs
of the microsecond; the pairing's end counters gave the rest: every page of
the form is verified over a run (`zpage_verified_pages_at_end` 106,241
against 93,333, the forms' page counts), and a page's first decode decodes
every group of it to check the keys, 3.0 µs a page under the record coding
against 1.8 by the tool's `verify` column, so the first touch of a page cost
E 1.2 µs more under the record coding, and E, whose Zipfian starts range
over the store, first-touches tens of thousands of pages a rep.

Two things tried against it, one kept. A decoder and an encoder of
standard LZ4 blocks shaped for a record against a dictionary, allocation-free
(the library's compress builds a table per call), on the reading that the
library's per-call setup was the record's cost: held to the library byte for
byte both ways on every shape, and slower -- a group decoded through them
read 0.93 µs hot against 0.66 through the library, the fill 51 µs a page
against 26, and a loop of eight-byte copies tried for the records' pieces
read a third more instructions than the memcpy calls it replaced, since the
compiler made no single moves of them -- so both were taken out again; the
per-record cost is the structure's and not the library's call. And the first
touch of a coded group verifies it by its heads (`zpages::check_coded_group`;
`zfile::first_checks`, which `first_decode` and the point read's fast path
share): the fields of every record in range, the image lengths summing to the
group's, the keys strictly ascending from the one the fence or the directory
names and below the next group's first -- what `walk_group` checks of a
decoded group, less the tails, whose decode is checked at the read that wants
them, a tail that decodes short or not at all being damage there as a record
that does not parse is in a decoded group. The point read's fast path runs on
a page not yet verified too now, the first checks first. Held by the head
walk's own test (a first key not the one named, keys out of order, a key at
the next group's first, lengths that do not sum) beside the forged-payload
tests. The pairing after it is the next section.

## The record coding paired again, and made the arm

With a coded group verified by its heads at a page's first touch, the same
pairing (`pair-5m.sh zcoding2 ...`, `zcoding2-ab-{1,2}.*`): over twelve pairs
in both orders the point-read mixes' gets read 0.682 (C, 0 of 12 slower),
0.748 (B), 0.751 (A), 0.780 (D) and 0.794 (F, 1 of 12) of the block coding's
time, the totals 0.697, 0.790, 0.943, 0.819 and 0.936; the write mixes'
commits, which the first pairing read 7-33% slower, are level now (A 1.000,
F 1.009, D 1.037, B 0.864, E 1.048) -- a commit's fill of its blocks touches
pages first, and the heads made that touch what the block coding's was; and
E's scans read 1.236, slower in all twelve pairs (1.222 and 1.249 by sitting),
E's total 1.218. So the first touch was the commits' loss and about three
points of E's, and the rest of E's loss, a fifth, is the memo misses' decode
of a group through its records, 131,000 of them a pass at 2.2x the block
decode's instructions, which no decoder of the records made cheaper and
which the structure sets: a field parse, three copies and a decoder call a
record against LZ4's bulk copies at seven instructions a byte, so a group
decoded through its records costs at least what the block decode costs and
in practice two to three times it. A scan's path needs groups decoded whole,
through the memo, and a point read's needs one record: the coding that
serves the second costs the first, and the shape that would serve both --
one LZ4 block a group whose records reference only the group's first, with
an entry a record into the block and the heads kept beside it for the seek,
so that the whole decode is the block's and a record's decode is its own --
is another layout, a tenth larger again, and would hold E level rather than
win it.

E being the cell the form has left to win, the record coding is the arm and
the block coding ships (`Options::zpages_record_coding` off;
`supdb-cache64tofitautozpagesrecord` and `supdb-nocachezpagesrecord` are the
arms, renamed from the block arms built the other way round). What this leaves
in the engine as shipped: the per-group coding bit (zero in every group the
block coding writes), the trailer at version 2 (a form file from before is
rebuilt once), the page's first checks factored (`zfile::first_checks`, the
same checks for a block group as before), the coded group's own paths inert
under the default, and the record coding whole behind the arm, with its tests
and the fuzz turning it on in a quarter of its seeds under `ZFUZZ_OPTVARY`.
The point-read gain is real and banked as the arm: a quarter to a third of the
point-read mixes' time at five million keys under 640 MiB, for a form 14%
larger and E a fifth slower. The rule the two pairings give: a form is priced
on every path that reads it before its coding is chosen, and a group decoded
through its records is a decode of every record, so a structure that makes
one record cheap to read is priced on the readers that want the group.

## E profiled again, the decode's lines probed, and two levers on the lines a read touches

At `3a885a9` (the record coding as the arm), E on the combined arm at five
million keys under 640 MiB, profiled as before (`scratch/wave2/prof/profile-e.sh`,
`perf record -e cpu-clock` at 3 kHz with DWARF chains over each mixes pass,
cut to E's windows by the harness's marks; the samples' text made with
`perf script --no-inline`, since perf's inline resolution through
`addr2line` ran at two kilobytes a second over this binary): three windows,
1,972 ms, 5,575 samples on the harness's thread at 2,827 a second, so the
thread was on its core for the whole of E -- the form fits the bound and E
faults nothing -- at 7.8 µs of CPU a scan (the harness's own 1.6 of it: the
Zipfian's `pow` 11.8%, the op loop 4.5%, the clock 4%). The engine's share,
by inclusive sample: the start seek (`cache_seek_exact` → `ZFile::seek`)
20.0%, of which the fence's search 4.7, the page's view 3.1, the group's
open 10.3 and the seek inside the group 1.9; the block reader's walks
(`PartReader::scan`) 15.3%; the clean runs through `ZFile::scan_at` 11.6%;
the builds (`materialize`) 10.0%, 6.5 of it `deltas_for`; the settle 7.5%;
the loop's own bookkeeping 5.6%; and across all of those the LZ4 decode
23.8%, 1,327 samples over 147,000 decodes a pass, which is 1.0 µs a decode
-- the figure `zsize` gives a decode whose page's lines were flushed, against
0.27 µs with them warm. The counters beside it: 79,198 scans a pass, 57,904
of them through the form file, 147,100 groups decoded and 107,300 memo
hits, 24,007 blocks built, walks 78,631 over copies, 30,410 over sparse
forms and 18,321 over clean blocks, 50,978 scans starting in a copy; the
arm without the block cache decodes 155,400 and runs E at 12.6 µs a scan
against 6.3, its time in the merge with the memtable (`has_tomb` 11.7%,
`memcmp` 11.3%, the snapshot's seeks) and not in decodes. So the decode is
E's largest single cost and runs at the cold figure, and the seek's share of
the decodes, a third by the open's callers, is spent for nothing on the
scans that start in a copy, which the copy answers without it -- the lever
`zpages_seek_span` was built for and did not take, since its own fence and
directory lookups and the copy's six-miss search cost what the seek had.

The lines a decode reads, probed (`bench/examples/zlines.rs`): a store of
a million keys built under the forced form, its pages read whole, and one
group of every page decoded in a random page order, cold, under five shapes
-- the view then the decode, as the engine has it; the group's payload lines
asked for after the view; the page's first line before the view and the
payload after; the page's first half before the view; the whole page before
the view -- with the output buffer warm, rotated over two thousand buffers as
the memo's recycled ones are, and with a spin between the hint and the
decode (`zlines-probe-1m.log`, `zlines-probe-out-1m.log`). The payload hint takes a cold
decode of 900 ns to 820, 9% at most in any shape and within the rounds'
spread; the whole page asked for is slower (1,019), the half page level, the
output buffer's lines asked for level (1,243 against 1,222 cold), and the
scan shape, every group of the page after one view, moves under none of
them. So the lines are not what a cold decode costs: the engine's 1.0 µs a
decode is the cycling figure `zdecode` found for a decoder on input its
predictor has not learned (the same groups cycled over a file that fits L2
read the same plateau), and a prefetch issued where the engine first knows
the page or the group buys a tenth of the decode, a fiftieth of E. Refuted
as a lever and not built; the probe stays as the record.

The decode's other lever, probed (`zdecode --hc LEVEL`, every group
recompressed by liblz4's high-compression mode and the cycling decode
timed over the new payloads, `zlines-hc-1m.log`): fewer tokens decode faster.
Level 1 shrinks the form 1.8% and the cold decode 2%; level 4, 4.1% and
10%; level 9, 4.1% and 16.5% (598 ns against 716); level 12, 4.2% and 18%.
liblz4's own decoder is 0.84-0.86 of `lz4_flex`'s cycling, as the earlier
probe read it, and over the HC payloads 508 ns a group, 0.71 of the
engine's decode as it stands. Both are liblz4, a C library the engine does
not link (the bench's `lz4` crate under its `lz4-c` feature is how the
probe has it); the compressor would be the builder's alone and the native
build's, since the browser reader decodes nothing of the form, and level 9
compresses at 13 µs a group, 2.3x the build's CPU at five million keys.
Whether the engine takes a C dependency is the user's decision, so the
figures are recorded and nothing is built.

What the profile left as pure-Rust levers on the lines a read touches, both
small and both on every read through the form:

- **The fence's search** (`entry_of`, 4.7% of E's thread, 8.6% of C's by
  the same recording's C windows): a binary search over 93,000 entries at
  five million keys whose every probe below the hot top levels read a line
  of the key offsets and a line of the key blob, dependent. `ZFence::index`
  builds a tree over the entries' words -- the eight bytes after the bytes
  every non-empty key shares, taken from the first non-empty key and the
  last, since the keys ascend; zero for the empty key, the open lower fence
  -- as a balanced tree laid out breadth first in lines of eight nodes, so
  a search reads one word a level and asks for the line three levels down
  before it reads one of its nodes, and compares whole keys only where
  entries tie on the word, from the top of the tie. The search over the keys
  stays as the shape before and as the oracle the tree is held to over
  every shape of fence and key (`the_trees_search_answers_as_the_search_over_the_keys`).
  Twelve bytes an entry beside the fence's sixteen and the keys.
  `Options::zpages_fence_tree` (on), `supdb-cache64tofitautozpagesfencesearch`
  the shape before.
- **The memo's victim** (`Memo::recycle`, 2.9% of E's thread and 6% of
  C's): a put displaces the slot's entry and tries its `Arc` for the last
  reference, a load of the counts' line last touched when that entry was
  made, thousands of decodes ago -- a cold miss a decode, on no instruction
  worth the name. The decode now asks for that line before it begins
  (`Memo::prefetch_victim`, the slot the put will take being a function of
  the group's number), and a decode is long enough to bring it.
  `Options::zpages_memo_ahead` (on), `supdb-cache64tofitautozpagesmemonoahead`
  the shape before.

Both paired at five million keys under 640 MiB, two sittings each, five
pairs a sitting, both orders (`pair-5m.sh zlines-fence ...`,
`pair-5m.sh zlines-memo ...`); the verdicts follow.

The fence's tree paired (`zlines-fence-ab-{1,2}.*`, A the tree, B the search
over the keys): over twelve pairs in both orders every cell is inside the
position band and the directions are mixed -- B/A on the gets C 1.002 (6 of
12 pairs B slower), B 1.033 (7), A 0.924 (3), F 0.937 (4), D 0.987 (5); E's
scans 1.023 (6); the totals 1.003, 1.040, 0.984, 0.968, 0.995 and 1.017 --
with the counters identical arm to arm (the same groups decoded, memo hits
and misses, blocks built and walks, to within the reps' own spread). The
point-read mixes under this bound are the form's faults more than its CPU:
C takes about 2,900 major faults a pass, 125 ms of gets for 83,000 reads,
and the thread is on its core for a tenth of that (the profile's C windows
sampled it at 262 a second against 2,827 in E), so a lever on the CPU of a
point read shows there only in core; and E's 2% is the position's. The
pairing rules a loss out and cannot see the gain, which is misses, so the
verdict for both levers is read in the profile of the arm pair, below,
where the symbol each lever is aimed at either loses its samples or does
not.

The memo's victim hint paired (`zlines-memo-ab-{1,2}.*`, A the hint, B
without it): over twelve pairs in both orders B/A on the gets C 1.065 (8 of
12 pairs B slower), B 1.050 (9), A 0.976 (6), F 1.008 (6), D 0.981 (6); E's
scans 1.035 (8; 1.109 in all six pairs of the second sitting, 0.965 in the
first); the totals 1.064, 1.076, 0.934, 0.946, 0.973 and 1.020; the counters
identical arm to arm. The direction favours the hint in the memo-heavy
mixes (C, B, E) and is nowhere outside the band, so this pairing too rules
a loss out and leaves the gain to the profile. One cell of the fence
pairing is noted for the profile to check: A's and F's gets read 0.924 and
0.937 for the shape before (B slower in 3 and 4 pairs of 12, the same sign
in both sittings), the two mixes whose gets run between 418 fsyncing
commits a pass, where C's, B's and D's gets -- the same reads without the
commits -- read level; the tree changes nothing a commit does, and the
settle's resolve of a sorted batch searches from the entry before, not from
the top.

The arm pairs profiled (`profile-e.sh` with each pair, two pairs after a
warmup, three E windows an arm; `scratch/wave2/prof/` has the aggregates as
`ycsb-E-<arm>.txt`): the fence's tree arm against the search over the keys,
the E windows 1,986 and 2,040 ms, 5,688 and 5,731 samples -- `entry_of`'s
own samples 240 (4.22%) with the tree against 192 (3.35%) without, the
hinted gallop's compare 67 against 57, everything else within the arms'
noise. The tree costs more, not less. The reading: the store at five
million keys is nineteen partitions of some 263,000 keys, so a fence is
some 4,900 entries, a hundred kilobytes of offsets and keys, and the
partitions a Zipfian reads keep theirs in the second-level cache; the
search over the keys was instructions and L2 hits, not the misses the
first profile's self time was read as, and the tree's prefetches, bounds
checks and word compares are instructions added to a search that had no
misses to lose. The lever was priced on a fence of 93,000 entries that the
store does not have. `Options::zpages_fence_tree` is off and
`supdb-cache64tofitautozpagesfencetree` is the arm, for a store whose one
fence would not fit a cache. The memo's hint arm against the shape before,
the E windows 1,988 and 1,779 ms, 5,623 and 5,062 samples: `Memo::recycle`
14 samples (0.25%) with the hint against 101 (2.00%) without, the miss it
was aimed at gone; the rest of the profile moved together, the hint arm's
whole pass 12% longer in this sitting with the decode, the view and the
fence all slower alike, which is the host between the two arms' windows
(the pairing's two sittings had the same cell at 0.965 and 1.109, and the
first profile's three windows of one arm spread 11%). The hint ships on:
the miss it removes is a fifth of a microsecond a scan by the first
profile, the pairing reads the memo-heavy mixes 3-6% for it in two thirds
of the pairs and no cell against it, and it costs two prefetches a decode.
The C windows cannot be read from these recordings: `perf record -p`
attaches some two hundred milliseconds after the mark that starts it, and
the C mix is 130-200 ms long, so C's windows hold the tail of C or nothing
(162, 166, 230 and 0 samples); a point-read profile needs perf attached
before the pass, cut by the marks afterwards.

The rule, for the profiling notes: a self time is read as misses only after
the structure's size is read against the cache per instance -- a fence
here is a partition's, not the store's -- and a lever on misses is held to
the profile of the arm pair, where the symbol it is aimed at loses its
samples or does not, since a pairing resolves nothing under a tenth and an
instruction count sees no miss.

## The settle's share of E, counted; the span arm looked at again

The settle is 7.5% of E's thread by the profile at `3a885a9`, and E
commits 4,167 inserts a pass in 42 batches, which read as ten microseconds
an inserted key -- too much for a key filed under the last block, which is
wide and takes a push. The per-mix events line carries the settle's counts
now (`settle_runs`, `settle_keys`, `settle_blocks`, `settle_rebuilds`,
`lazy_cuts_filed`, `lazy_cuts_resolved`; `Db::settle_density`), and they
say what the settle is: 43 runs a pass settling 48,993 keys over 17,651
blocks (`zspan-quick-ab.err`), the writes of the mixes before E -- A's and
F's updates, B's and D's inserts, the 49,035 unsealed keys the counters
show -- which their own commits do not file, since nothing scans there
(`Options::forms_settle_far` off, for the shuffled load's memory), so E's
first scans file the backlog: 1.1 µs a key, once a pass. Of the keys, 40,805
are filed lazily under the block the fence and the directory name
(`Options::zpages_settle_span`) and 8,188 fall in a group that straddles a
block boundary and take the exact seek, with a decode for most, since the
memo rarely holds the head's groups between two settles; by the profile's
subtree (`aggregate.py --under settle_each`) the resolve is two fifths of
the settle (the lazy keys' page views a sixth, the straddlers' seeks and
decodes a quarter), the sort a sixth, the patches a third, and the fresh
pages of the batch's own vectors a twelfth. No piece is over two percent
of E, and the sum cannot move to the mixes before without charging them
the same work, so the settle is not a lever on E; the counts stay on the
line.

The same run (two pairs after a warmup, one order) looked at
`supdb-cache64tofitautozpagesspan` again, the arm that takes a scan's block
from the fence and the directory and seeks the rank only for a block not
held as a copy, which read 2-3% slower in both orders when it was priced:
E's scans 0.878 (the span arm faster in two pairs of three), D's gets 0.865
in three of three, C's, A's and F's gets 1.07-1.10 in two of three -- a
quick look, not a verdict -- with the deterministic counters saying the
mechanism works: 44,652 of E's 79,198 scans start in a copy without a seek
(`starts 6326/44652`), E decodes 137,400 groups against 147,100 and asks
the memo 210,000 times against 254,000. The levers since the arm was priced
-- the trusted views, the block cursor, the copy start at the rank -- moved
what the seek and the copy's search cost, so the arm is paired again in
full (`pair-5m.sh zspan2 ...`); the verdict follows.

The verdict (`zspan2-ab-{1,2}.err`, five pairs a sitting after a warmup,
both orders): E's scans 0.990 over twelve pairs with the span arm slower in
seven, 0.940 in the first sitting and 1.043 in the second, the gets of C
1.040 (slower in eight of twelve; 0.973 and then 1.111), D 0.994, A 1.004,
F 1.016, and every count the same as the quick look's: 137,400 groups
decoded against 147,100, 72,300 memo hits against 107,300, the walks and
the blocks built equal. Level, a position effect wide in both directions
and nothing a tenth from one; the arm stays off. What it says about the
start: the seek the arm skips is a memo hit two times in three (the
decodes fall by the third that misses), so the seek's own cost is the
fence's search, the view, the group's search and the memo's lookup, and
the arm pays most of that again finding the block from the fence and the
directory and then walking the copy from the key by its own search. A
lever on the start has to skip the fence and the view too, which only a
memory of the key's block can.

## The start cache: a scan's start key mapped to its block

E's scans start where E's scans started: the mix's Zipfian, unscrambled,
puts half of a pass's 79,000 starts among the top four thousand keys, and
the scan's first step is the partition's seek of the start -- the fence's
search, the page's view, a group opened and sought -- a fifth of E's
thread by the profile at `3a885a9` (`cache_seek_exact` and `ZFile::seek`
under `scan_blocks`), with a third of the groups it decodes decoded for a
start the copy that holds the block would have found by its own search.
A start is a function of the key and the partition's blob, so the handle
keeps the block each start key resolved to, against the blob's identity --
a number no other open blob in the process has, so a partition a merge
replaced or a promotion re-opened is a miss and never a stale block -- in a
direct-mapped table of thirty-two-byte slots made at the first put
(`Options::scan_start_cache`, 4,096 entries as shipped, 128 KB a handle,
counted in the census's `misc`; `db::StartCache`). A hit names the block
alone and the walk begins there as it does under the span arm: a copy
walks from the key by its own search and a block held any other way
seeks the rank where that arm seeks it. A key over sixteen bytes is never
kept, since a hit has to be exact and a wrong block walks the wrong
records with nothing raised; a slot holds the whole key and the compare
is of the bytes. The put is made only where the seek ran, after it, with
the block the seek's owner rank falls in. The counts are on the per-mix
events line (`start_cache_hits`, `start_cache_puts`;
`Db::start_cache_counts`), the fuzz runs a quarter of its option-varying
seeds with the cache off, and `tests/zpages.rs` holds a store with the
cache to the one without over scans from repeated starts -- drained, with
unsealed keys, and with those keys sealed into a piece -- through the
writer and a handle, with a key over sixteen bytes among the starts.
`supdb-cache64tofitautozpagesnostartcache` is the shape before, every scan
seeking its start; the pairing (`zstartcache-ab-{1,2}.err`) and the arm
pair's profile follow.

The verdict (`zstartcache-ab-{1,2}.err`, five pairs a sitting after a
warmup, both orders; `prof/ycsb-E-supdb-cache64tofitautozpages-startcachepair.txt`
and its `nostartcache` partner): level, and a lever aimed at the wrong
seeks. E's scans read 0.981 over twelve pairs with the arm without the cache
the faster in both sittings (0.983, 0.979), the other mixes within the ±5%
the mixes that never consult it show (B's gets 1.049 and A's 1.026 with
hits and puts at zero there). The counts, the same in every rep: 15,800
hits and 63,400 puts over E's 79,200 starts a pass -- a fifth, where the
Zipfian's mass puts half of the starts among four thousand keys -- because
a direct-mapped table of 4,096 slots is churned by 63,000 puts a pass,
fifteen a slot, so a key survives between two of its starts only where
fewer puts than that reach its slot in between; and the groups decoded the
same, 147,100 a pass in both arms, with the memo asked 91,500 times against
107,300: every seek the hits skipped was a memo hit. The hits are the
hottest keys' seeks, whose fence entry, page and group sit in the core's
caches and the memo, the cheapest fifth of the seeks; the expensive seeks
are the cold ones, with a decode, at keys that never repeat. The arm
pair's profile says the same in samples: the seek's subtree
(`cache_seek_exact`) at 21.0% of E's thread with the cache against 21.5%
without, 1,140 samples against 1,189, a loss of 4% for a fifth of the
seeks skipped, and `scan_blocks`' own samples up from 320 to 377 with the
cache's lookups and puts -- 128 KB of slots touched at random -- as much as
the seek lost; the decode 25.1% against 25.8%, the windows 1,901 ms against
1,936. Off; `supdb-cache64tofitautozpagesstartcache` is the arm, at 4,096
entries, on in a quarter of the fuzz's option-varying seeds. Two bounds
for whoever returns to the start: an exact-key cache of any size reaches
at most the starts that repeat by key, 53% of E's by the Zipfian's
arithmetic (37,000 distinct keys among 79,200 draws), and its hits are the
cheap seeks by construction; a cache of blocks by key range would reach
the starts that repeat by block, about 78% (17,000 distinct blocks), but a
hit saves the seek only where the block is a copy -- the hot blocks again,
since a block held any other way needs the rank, and the rank's seek
decodes the group the walk decodes first anyway. What the start costs E
is the cold start's decode where the block is a copy, the third of the
start decodes the span arm skipped for a level result. The start is not
where E's time is to be had; the decode is (25% of the thread, 147,000
groups a pass, 2.5 a form walk, nearly every one a miss since the form
walks are the cold tail), then the record walks (`walk_run` 12%, the
clean runs 12%), the builds 11%, the settle 7%, and the harness's
generator and clock about a sixth.

## The fuzz's red on this tree: the snapshot arena's reservation of no bytes

The option-varying fuzz range (`ZFUZZ_OPTVARY`, seeds 0-300) went red on
this tree at one seed, 242, and at no other alone: a panic in the upkeep
thread, `snapshot arena: an offset past what was written`, from
`Snapshot::seek` under `BuildCtx::snap_span` in `Reader::make_table`, four
runs in five with the seed alone, the store's drop then aborting the
process. The start cache was off in the seed's options, and the commit
before this tree (`d0636fa`), with its fuzz patched to consume the one draw
this tree adds so that every seed's operations are the same, fails the seed
four runs in six the same way: a bug older than the change, reached because
a draw added to the fuzz's one random stream moves every seed's operations
after it. The assertion, made to name its quantities, said what: offset 0,
length 0, the arena's tail at zero and no block allocated -- the empty key,
the live table's only key after the flush. The snapshot's build reserves
the keys' bytes in one batch (`SnapArena::reserve_bytes`), which answered no
bytes with the tail and no block where the per-key reservation (`reserve`)
made the block; the read of the key, the next partition's fence sought in
the snapshot, asked for the block nobody had made. A read or a write of no
bytes touches no block now and the two reservations agree; `tests/settle.rs`
builds the shape (two partitions, the empty key alone unsealed, a scan
through the writer and a handle, with and without the snapshot's runs) and
fails on the guards removed with the same message, and `db::snap_arena`
holds the arena to it. The attribution's worktree and binaries are
`$S/wt-head` and `$S/wt-head-target`; the sweep that named the seed ran
every seed alone (`$S/tmp-fuzz-single`).

## E against LMDB: the block cache's budget, refuted as the lever

The user's question -- why LMDB is untouchable in E, and why the engine
cannot do what it does with the right gear shift -- and the answer the
notes already held: at five million keys under 640 MiB LMDB's 670 MB file
does not fit and its E still runs at about 4 µs a scan with 0.33-0.56
faults a scan, so a fault there is a few microseconds (the page cache's
minor faults on its mapping), while the form fits whole and wins the point
reads 1.7-2.4x, the pure scan pass 2.2-3.1x and every other mix, and loses
E alone at about 6 µs a scan. E mixes inserts with scans on the same hot
keys; a B-tree applies each insert to its leaf page at write time and its
scan reads one structure, where the engine merges the memtable into every
scan at read time -- the forms built (11% of E's thread, 24,007 a pass), the
settle (7%), the sparse walks' cuts, the seek (0.6 µs a scan) and the
decode of every group a walk crosses (1.45 µs a scan). The plan agreed:
the budget first, then whatever keeps the hot head's copies from staying,
then the tail's decode.

The budget (`supdb-cache128tofitautozpages`, `supdb-cache256tofitautozpages`,
two creators on the combined arm's policy; `zbudget256-ab-{1,2}.err`, five
pairs a sitting, both orders): E's scans 0.975 over twelve pairs with the
256 MB arm slower in four, 0.950 and 1.000 by sitting, every other cell
within a few percent either way -- level -- and the counts identical in
every rep of both arms: 24,007 forms built a pass, the walks 78,631 copies,
30,410 sparse and 18,321 other, 147,100 groups decoded, the memo asked the
same. A budget four times the default sheds nothing E rebuilds, so the
24,007 builds are not churn: they are E's first touch of each block it meets
in a pass, a slot made for every block a scan crosses, most of them clean,
and the covered blocks' copies on their second touch. The hot head's copies
stay as they are; the pairing against 128 MB was stopped as redundant once
the counts read the same. What the 24,000 builds are by kind, and how many
the budget sheds, is counted next (`Db::build_kinds`: copy, sparse, wide,
clean, the second-touch copies and the sheds, on the events line), since
the lever is in the kinds and not in the budget.

The same profile (the start cache pair's default arm, `prof/ycsb-E-...-startcachepair-{decompress_internal,materialize,cache_seek_exact}.txt`),
taken apart for the plan. The decode's one caller is `decode_group`, 25% of
E's thread, and by the subtrees it sits in: the scans' start seeks 8.5%,
the builds 2.7%, the walks the remaining 14%. The seek (`cache_seek_exact`,
21.0%) is that decode 8.5%, the fence's search 3.3%, the page view 3.0%,
the group's search 1.8%, the group open's own 1.2%, the memo 1.1% and the
seek's own 0.9%: about 0.7 µs of a scan without the decode, 1.15 with it.
The builds (`materialize`, 10.9%) are the sparse forms: `deltas_for` is 7.7%
of the thread, the cursor opened at each delta's cut 3.4% of that, the
builds' decodes 2.7%, a memcmp 1.0% and the fence's walk from the entry
held 0.9% -- the dirty tail's blocks, a few unsealed keys each, built at
their first touch in the pass. So E's thread against LMDB's: the decode
1.35 µs a scan, the builds 0.6, the settle 0.36, the seek's search 0.7
against a B-tree descent of perhaps 0.3, and the record walks about a
fifth of the thread, which a B-tree's leaf walk pays alike. The decode is
three quarters of the gap, and it is the cold tail's: the walks' 14% is
groups no memo holds, the seeks' 8.5% the cold starts, and the hot head,
held as copies, decodes nothing after its first pass.

The kinds (`zkinds-ab-1.err`, two reps of the default and the 128 MB arm in
one order, the counts the same in every rep of both): of E's 24,007 builds
a pass, 14,789 are clean blocks -- the overlay check coming back empty, a
slot and nothing built -- 8,890 sparse forms, 327 copies and one wide
block; the covered rule's second-touch copy fired 0 times and the budget
shed 0 blocks, at 64 MB as at 128. So the hot head is 327 blocks held as
copies from their first touch and walked 78,600 times a pass with no decode,
exactly the structure a B-tree's resident leaves are; the warm middle is
8,890 blocks with a few unsealed keys each, a sparse form built at the
first touch (3.7 µs each, `deltas_for` and the cursor opened at each
delta's cut: 7.7% of E) and walked 3.4 times a pass through the form, a
decode a group; the cold tail is 14,789 clean blocks walked 1.2 times a
pass, a decode a group. Nothing here is churn and nothing here is the
budget: it is the form's decode on every walk that is not the head's, plus
the sparse builds. The in-core rows say the same from the other side
(`bench/runs/full/20260929T025048Z-5eabd8d.json`, ycsb-E, durable arms):
`supdb` with the block cache reads 1.36x, 1.48x and 1.41x of LMDB at three
hundred thousand, one million and three million keys, `supdb-nocache` 0.21x,
and `supdb-cache256` 1.01-1.34x -- the per-block forms over raw records win
E in core by half as much again, and under the bound the same forms walk
the form's groups instead of raw records and read 0.55-0.62x. The whole of
E's deficit under the bound is the decode the form puts under every walk
that the head's copies do not cover.

Under the bound the process holds 75-77 MB of anonymous memory through the
mixes beside 520-560 MB of file pages (the bound's cgroup sampled every
three seconds, `zkinds-memsample.log`): the form files' 360 MB and
what else of the store's files the earlier passes left resident, under a
limit of 671 MB. LMDB there allocates next to nothing and its 670 MB file
is all but resident, so its 0.33-0.56 faults a scan are the mapping's
minor faults, not the device's: at this rung LMDB is in core, and so are
we, through a form whose decode is three quarters of the gap. A form with
the keys front-coded and the values stored -- about 530 MB by the records'
arithmetic, 105 bytes a record -- would hold 607 MB of the 671 with the
process's 77 beside it, which fits, and its walk reads records as the raw
walk does; the next rung it does not fit, and the LZ4 form is the one that
fits there. That is the gear: the lightest coding that fits the bound,
chosen as the controller chooses the form, by what fits and what faults.
Built next as a group coding beside the block and the record codings, the
forced and combined arms paired under the bound at five million keys, E
and the point reads read together.

## The stored-whole form, refuted at this bound by its size

The format already has a stored group (`GroupEntry::stored`, the payload the
images verbatim, read in place from the mapping with no copy and no memo),
which the fill gives a group LZ4 does not shrink. `Options::zpages_store_all`
gives it to every group; `supdb-nocachezpagesstored` is the forced arm and
`supdb-cache64tofitzpagesstored` the forced form with the block cache, beside
`supdb-cache64tofitzpages`, the forced form with the combined arm's cache
and budget, added as the control a coding the controller's size model would
refuse is priced against. `tests/zpages.rs` holds the stored form to the
compressed one in every read, count and window, to zero decodes, and to a
file at least a quarter larger. Paired forced against forced at five million
keys under 640 MiB (`zstored-ab-{1,2}.err`, five pairs a sitting, both
orders): the stored form reads E's scans 1.275 (slower) over twelve pairs,
eleven of twelve, C's gets 1.354 in twelve of twelve, D's 2.831 in twelve of
twelve, F's 1.578, A's and B's 1.30, the scan pass at 0.60x -- and the reason
is one column: major faults, 0.31 a scan in the scan pass, 0.19 a point read
in the read pass, 0.06 a scan in E, 0.11 a read in D, 0.018 in C, where the
LZ4 form takes none. The images are 120 bytes a record at the suite's shape
and the stored form about 620 MB of file against the LZ4 form's 360, and the
cgroup holds 671 MB less the process's 50-77 MB: the form is some 96%
resident, the tail's misses go to the device at tens of microseconds each,
and E's decode-free walks are invisible under them (D, which reads the tail
uniformly, says it loudest). The same shape as the promotion arm's
refutation: under a bound the LZ4 form just fits, anything that trades page
cache for decoded bytes loses by the faults it buys, and the stored form
trades 260 MB of it. The arm stays as the record of the fit, off; at a bound
the stored form fits, it is the lightest coding there is.

The fitting variant is the record coding with its tails stored raw
(`Options::zpages_raw_tails`, built next): the heads front-coded against the
group's first record as that coding has them, the values the records' own
bytes, a decode a copy a record, about 108 bytes a record and some 556 MB at
five million -- under the 594 MB the process leaves, where 620 was not. The
record coding's reader already copies a raw tail where it finds one, so the
change is the encoder's alone; `supdb-nocachezpagesrecordraw` and
`supdb-cache64tofitzpagesrecordraw` are the arms, paired as the stored ones
were.

The raw-tail record coding, forced against the LZ4 form (`zrawtails-ab-{1,2}.err`,
five pairs a sitting, both orders): the point-read mixes read for it in both
sittings -- B's gets 0.828 (slower in two of twelve), D's 0.810 (none of
twelve), F's 0.813 (one), A's 0.880 (three), C's 0.930 (three) -- since a
point read copies one record of about 108 bytes where the LZ4 form decodes a
group of two kilobytes or hits the memo; and E's scans read 1.098 against it,
slower in nine of twelve, 1.100 in six of six in the second sitting, since a
scan decodes its groups whole and the coding's decode is a copy a record,
seventeen varint parses and thirty-four small copies a group, about what the
LZ4 block decode costs, over 12% more groups (175,000 against 155,000 a pass:
a page holds fewer of the larger payloads and a group that passes the room
is trimmed). The form fits at steady state -- the mixes take 0.001-0.005
major faults an operation against the LZ4 form's none -- but the passes
right after the build do not: 0.14 a read in the read pass and 0.09-0.10 a
scan in the scan pass (0.58x), the build's own traffic (600 MB of raw records
read, 556 of form written, through 594 of cache) having pushed the form's
pages out, which the LZ4 form's 360 survive. What E needs from this coding is
the value read in place: the payload holds every tail raw and contiguous,
`record_group_seek` already walks the heads without a decode, and a cursor
that walked a coded group by record -- the head rebuilt from the group's
first into a small buffer, the tail lent from the payload -- would decode
nothing for a scan, a few hundred lines of `ZCursor` (its position a record
index for a coded group where it is a byte offset for a decoded one). The
cached pairing (`zrawtailscache`, the forced form with the block cache and
budget, raw tails against LZ4) says what E reads with the hot head's copies
beside it.

With the block cache beside it (`zrawtailscache-ab-{1,2}.err`,
`supdb-cache64tofitzpagesrecordraw` against `supdb-cache64tofitzpages`, the
forced form with the combined arm's cache and budget, whose E runs at the
Auto default's 450-500 ms a pass): the raw-tail form loses every cell it won
forced -- E's scans 1.462 (slower) in twelve of twelve, D's gets 1.272 in
twelve of twelve, C's 1.299 in nine, B's 1.118 in ten, A and F level -- and
the faults say why: 0.016 major faults a scan in E, 0.028 a read in D, 0.013
in C, 0.15-0.20 an operation in the passes, where the LZ4 form takes none.
The cache's anonymous memory -- the copies, the sparse forms, the tables --
is the margin the forced arm had left, and without it the 556 MB form is a
form the tail's reads fetch from the device. So at five million keys under
640 MiB, beside the block cache that wins E in core, the LZ4 form is the
lightest coding that fits, and the gear the plan named picks it here. The
two lighter codings stay as arms, off: at a bound they fit -- this store
under 768 MiB, or three million keys under this one -- the raw-tail coding
reads the point-read mixes 7-19% faster for its one-record copies, and the
stored form decodes nothing at all. E at this rung is bounded by the LZ4
decode (a quarter of its thread, irreducible but by the codec's own speed)
and the merge bookkeeping (the sparse builds and the settle, a sixth), and
neither coding reaches it. The census of what the process holds under the
bound, by owner, is read next (`zcensus-ab-1.err`), since the fit's margin
is the one quantity that could still change the answer.

## The fit check: the raw-tail form with the memo off and a trim after the install

The question left by the raw-tail pairings was whether the 556 MB form
missed the fit by the process's own anonymous memory, which could be
lowered, or by its size against the bound, which cannot. Two levers on the
anonymous bytes were put into one arm: the memo of decoded groups off
(`zpages_memo`, eight mebibytes a handle, and nothing a one-record copy
needs), and the allocator trimmed once after the form files' install
(`Options::zpages_trim_install`, `malloc_trim` at the end of
`zpages_settle`, whichever home the segment work has), so that what the
builds' threads and the load left free in their arenas is the file's before
the mixes start. `supdb-cache64tofitzpagesrecordrawlean` is the arm; the
harness prints the engine's census after each mix under `SUPDB_MIX_CENSUS`
(a `mixcensus` line a mix, the lag pass's `lagcensus` without the arenas
and the maps), and `census_read.py` reads them by arm and mix. Paired
against `supdb-cache64tofitzpages` at five million keys under 640 MiB, five
pairs a sitting, both orders (`zfit-ab-{1,2}.{out,err}`, read with
`pairing_read.py` and `census_read.py`).

The fit is lost all the same. The lean arm takes 0.022-0.024 major faults a
read in D and 0.029-0.031 a scan in E in both sittings, 0.128-0.129 a read
in the read pass right after the build and 0.069-0.081 a scan in the scan
pass, where the LZ4 arm takes none anywhere; A, B and C take none under
either. D reads 1.45-1.5x slower in eleven pairs of twelve and E 1.5-1.57x
slower in twelve of twelve, by the faults: E's 2,400 a mix (83,334
operations, 79,198 of them scans) at the 65-75 µs a fault D and the read
pass price it at are 160-180 ms of the 260 ms E loses a mix, and the rest
is the memo's: with it off every group a scan opens is a copy-decode,
269,763 a mix against the LZ4 arm's 147,182 decodes and 107,176 hits, and
the settle reads 92 ms against 53. The point-read mixes read the other
way, and by exactly the decodes they no longer do: C's gets 0.716 (124.9
to 91.2 ms a mix, 34,366 LZ4 decodes gone at about a microsecond each),
B's 0.693 (135.3 to 100.8, 32,376 decodes), A's 0.766 (85.3 to 61.2,
16,052), F level -- the forced pairing had them at 0.81-0.93 with the memo
on, so for a point read under the raw tails the memo was a cost and not a
saving, since a one-record copy is as cheap as a hit.

The census says why the levers did not buy the margin. The process's
anonymous bytes during the mixes are 64-99 MB under the lean arm against
68-88 under the LZ4 arm -- lower by four in C, higher by eleven in E -- while
the allocator's bytes in use are 15 MB lower in every mix (the memo's
eight, and the groups it held): the trim's return is taken back by the
mixes' own allocations, the memtable's arena (19 MB of capacity by D), the
snapshot (5.7), the block cache's tables (12 in E), the harness, and what
those free again stays resident in the arenas, so the lean arm's resident
free bytes beyond use are 47 MB in E where the LZ4 arm's are 21, and the
minor faults a mix are higher under it (E 5,200 against 2,318), the pages
the trim returned faulted in again. A trim before the mixes lowers what
the process holds at the trim and nothing of what it holds in the mixes,
which is where the form's pages compete. The engine's own census is 4-38 MB
of the 16-67 in use; the rest is the harness's.

So at five million keys under 640 MiB the raw-tail form's 556 MB of file
do not fit beside the 90 MB the process needs during the mixes and the
raw pages the block cache's builds and the settle read, by a few percent
of the form's pages, and the process's anonymous memory is not the lever
that closes it. The in-place walk of a record-coded group, which would
take E's decode out, is not built: it would save E's 1.45 µs a scan of
decode and E would still pay 0.03 faults a scan at 65-75 µs each, about 2
µs, before the memo's loss. Both lighter codings stay as arms for the
bounds they fit (this store under 768 MiB, or three million keys under
this one), where the raw tails with the memo off read the point-read
mixes 1.3-1.5x faster than the LZ4 form; the trim is an arm, off, refuted
as a lever on the fit; and the controller's choice of a coding, when it is
made, is made on the form's size against what the mixes leave, which the
`mixcensus` line now reads.

## The fit check, second half: whose bytes the resident free memory is, and a trim where the mixes are

The census's arenas line (`SUPDB_MIX_CENSUS=2`, the lag pass's `lagarenas`
and `lagmaps` after each `mixcensus`; `zfit-arenas.err`, three reps of the
lean arm against the LZ4 arm, the mixes alone) says whose frees the lean
arm's resident free bytes are after E: the main arena, the harness's and
the writer's, 33-52 MB taken from the system with 0.7-19 MB free and 28-31
resident; three or four arenas of 9-29 MB each entirely free in one or two
chunks and 5.6-8.7 MB resident apiece, the arenas the build threads and the
mixes' seal threads left; and an arena of 3,589 free chunks holding 8 MB,
the upkeep thread's. About 40-50 MB of the 72-89 MB the process holds in E
is free memory resident in those arenas, which is what a trim returns,
and the trim after the install had returned it once: the mixes' seal
threads take the dead arenas over, allocate into them and free again, and
the pages come back resident under the same chunks.

So the trim was moved to where the mixes are: `Options::upkeep_trim_look`
asks the trim thread (`forms_to_fit` with `forms_trim_bytes`) to look at
the allocator at the end of every upkeep pass, the stride deciding
(`trim_due`), and not only when a handle has freed a stride of forms; the
install trim counts with the thread's trims and sets the floor its stride
is counted from. `supdb-cache64tofitzpagesrecordrawleantrim` is the lean
arm with it, paired against `supdb-cache64tofitzpages` as before
(`zfit2-ab-{1,2}`, two sittings, five pairs each). `tests/db.rs` holds the
look to the pass end: with a stride no shed reaches, the looks are the
passes' alone, some with the option and none without.

Refuted again, and the counters say how. The looks happened -- 970-980 by
the end of E, 30-50 µs each by the cumulative `look_ms` -- and the trims
did not: two over the store's life, the install's and one in the first
mix, since `trim_due` trims when the allocator's free bytes have grown a
stride past the floor the last trim left, and the free bytes do not grow:
the seal threads re-use the dead arenas' chunks, so the pages come back
resident while `mallinfo2`'s free count stands still. The process's
anonymous bytes in E are 81-102 MB (median 92) against the lean arm's
72-99 and the LZ4 arm's 76-106; in C, B and A 57-65 against 64-71, seven
MB lower. The faults stand: 0.027-0.031 major faults a scan in E and
0.013-0.026 a read in D in both sittings, 0.12-0.13 a read in the read
pass and 0.05-0.065 a scan in the scan pass, none in A, B or C, none
anywhere under the LZ4 arm. E's scans read 1.393 (slower) in twelve pairs
of twelve and D's gets 1.449 in twelve of twelve; the faults are E's whole
loss now, 2,200-2,560 a mix at 65 µs each against the 162 ms E loses, so
the memo's lost hits cost less than the first check priced them. The
point-read mixes read 0.65-0.76 (C's gets 0.672, B's 0.653, A's 0.759, F's
0.892), as before.

What the two checks say together: at five million keys under 640 MiB the
raw-tail form's 556 MB and the process's 90 MB in the mixes are the
limit's 671 MB with nothing left for the pieces the mixes seal, the log,
the raw pages the block cache's builds and the settle read, or the
kernel's own, so the tail's reads and D's reads of the newest keys fault at
a few percent; the process's anonymous memory is not the lever that
closes it, since a trim before the mixes is taken back and a trim counted
by the allocator's free bytes never fires on pages that come back under
the same chunks, and a trim counted by resident bytes beyond use would
return at most the 40 MB that are free, for a working set that is over
the limit by about that much before the pieces are counted. The same
total is where LMDB sits in core at this rung with its 650 MB file and a
process of almost nothing, which is the gear E would need: the raw-tail
form, a process footprint of LMDB's order, and the in-place walk that
decodes nothing, each necessary and none sufficient alone. The lighter
codings and both trims stay as arms, off; the per-pass look is a sound
mechanism that found nothing to do here, and the record of what a trim
can return is the census's.

## The coding gear: the selector chooses the raw-tail coding where that form fits

The user's direction after the fit checks: first the gear, the controller
picking the raw-tail coding where its form fits with margin, since the
raw-tail form reads the point-read mixes faster wherever it fits; then the
lean process. The gear is `Options::zpages_coding_auto`: under `Auto` the
sampler fills the raw-tail record coding beside the options' coding
(`z_sample_start`, one `FillShape` each, the fills' CPU 10-20% more than one
coding's in the sample job's wall), and at the collect
`zselect::choose_coding` picks the raw tails where `model_z` predicts that
form faults nothing against the budget -- the budget the model already
prices against, the limit less the process's anonymous memory and a margin
of a twentieth or 64 MiB -- and the block coding otherwise; the reading fed
to the selector is the chosen form's, the pick holds for the builds after it
(`z_start`'s plan), and each form file names its own coding, so a store may
carry both. The line is at no predicted fault at all, since the fit checks
had priced a form that faults a little (1.4-1.5x slower in D and E for
0.02-0.03 faults an operation) against one that wins 1.3x where it fits;
`zselect::tests` holds the choice to the two measured points and the line
itself, `tests/zpages.rs` drives an eager `Auto` store under two injected
budgets to the raw tails built and to the block coding built, and
`supdb-cache64tofitautozpagescodingauto` is the arm. The counts carry the
picks, the builds under the raw tails and both forms' estimated bytes
(`zpage_coding_picks_*`, `zpage_builds_raw_tails`, `zpage_sample_w_z_raw_bytes`).

**Where both codings fit, forced** (`zgearforced-ab-{1,2}`, three million keys
under 640 MiB, `supdb-cache64tofitzpages` against `supdb-cache64tofitzpagesrecordraw`,
the block cache and the memo on both, no major fault in either arm): the
raw-tail form reads the point-read mixes faster in both orders -- D's gets
1.8-1.95x, C's 1.37-1.46x, B's 1.25-1.32x, F's 1.25x, A's 1.0-1.1x, the read
pass 1.53x -- and the scans slower: the scan pass at 0.70-0.72x in both
orders and E 0.89x and 1.02x, since a scan decodes its groups whole and a
raw-tail group's decode is a parse and a copy a record over 12% more groups,
where the block coding's is one LZ4 decode. The point read is where the
coding pays: one record copied out of the payload against a group decoded
or a memo hit. The in-place walk of a raw-tail group, which would make a
scan's read a lend from the payload and take the scan pass's loss out, is
the next piece of the lean-process direction and is not built here.

**Where the raw store fits, Auto** (`zgearauto3m-ab-{1,2}`, three million
keys under 640 MiB, `supdb-cache64tofitautozpages` against the gear): the
controller stays FITS, samples nothing and builds nothing in either arm
(`zpage_builds` 0), and the pairing reads level (0.94-1.0) -- the gear changes
nothing where no form is built, and the raw store under 640 MiB at three
million keys is served raw. **Where neither fits** (`zgearauto5m-ab-{1,2}`,
five million under 640 MiB): the gear sizes the raw-tail form at 595 MB
against the block form's 391 (`w_z_raw` in the `zsample` trace line, the
measured files 556 and 360) and picks the block coding in every rep, both
arms build the same nineteen files, and the pairing reads 1.0-1.1 with the
point-read mixes 3-10% slower under the gear in 6-9 pairs of twelve, inside
what the sitting's order moves (below). The gear's window is the budgets
between the raw-tail form's bytes and the raw store's -- at the suite's
shape the raw-tail form is 86% of the raw files, the hash index and the
record headers being what it saves -- so a store that has outgrown its
budget by less than a seventh is where the controller builds and the raw
tails fit.

**Inside the window, Auto** (`zgearauto5m720-ab-{1,2}` and
`zgearauto5m736-ab-{1,2}`, five million keys under 720 and 736 MiB, the
census on for the second). At 720 MiB the pick flipped with the order:
the raw-tail form's 595 MB plus its resident 10 against a budget of the
limit less the process's anonymous memory at the sample's moment less
64 MiB fits only while the process holds under about 84 MB, and the arm
that ran second in a rep held more, so one sitting picked the raw tails in
every rep and the other the block coding in every rep -- a knife edge, read
per sitting and never as the table's gmean, which would mix the two picks.
At 736 MiB the gear picked the raw tails in every rep of both orders and
built nineteen raw-tailed files, and the pairing says two things at once.
Where the gear picked raw tails its mixes read faster than the Auto
default's in both orders: in the sitting where the default had built its
block form, C's gets 2.2x and D's 2.4x, B's 1.4x, A's 1.2x, F's 1.3x, E's
scans 1.26x and the scan pass 1.12x, with the default's mixes faulting
0.01-0.04 an operation and the gear's 0.000-0.003; in the other sitting the
default built nothing in the median rep (`zpage_builds` 0, its mixes at
0.10-0.16 major faults a read in C and D) and the gear read 5-6x in C and
D. The controller's rent is what differs: at a rung the raw store misses by
little, `phi_raw` is small and the excess climbs slowly toward the build's
price, and the gear's reading -- the raw-tail form's cheaper decode -- makes
the form's predicted saving larger and the buy earlier, so the pairing
measures when each arm bought as much as what it built, and the Auto
default's own buy at this rung is slow or absent within the passes, which
is a finding about the rent and not about the coding. The clean reading of
the coding is the forced pairing above: point reads 1.25-1.95x, the scan
pass 0.70x, E 0.89-1.02.

What the gear is, then: correct in its choice at every rung paired (the
block coding where the raw-tail form would miss, the raw tails where the
model says it fits), with a window that is the budgets between the raw-tail
form's bytes and the raw store's, about a seventh of the store at the
suite's shape, and a knife edge at its lower end where the process's
anonymous memory at the sample's moment decides; inside the window the
point-read mixes read 1.2x to several times faster than the default and
the scans pay the whole-group copy-decode, 0.70x on the scan pass forced.
It stays an arm, off, until the in-place walk of a raw-tail group takes the
scan's cost out, which is the lean-process direction's first piece, since a
gear that trades the scans for the point reads is not the default for a
store that does both.

## The in-place walk of a raw-tail group

The raw-tail form's scans paid for the coding the point reads won by: a
scan decoded each group whole, a parse and a copy a record over 12% more
groups, and the scan pass read 0.70x of the block form's where both fit.
The walk reads a raw-tail coded group where it lies now
(`Options::zpages_inplace_walk`, on; `zfile::Group::Coded`,
`Opened::Coded`): the group is held as its payload in the mapping, the
position is a record index, each record's key is assembled from the
group's first head and the record's suffix into the cursor's key buffer
(sixteen bytes at the suite's shape, a copy LMDB's leaf does not make),
and its extents and inline value are parsed from the raw tail where it
lies by `flatindex::parse_record_tail`, the half of `parse_record` past
the key, one definition for both. A record whose tail is compressed after
all, or whose extents cannot be read where they lie (the non-compact
shape's extent array has an alignment the payload does not keep), is
decoded on its own; a group is judged raw-tailed from its last record,
since the first is stored raw whatever the coding. Nothing goes into the
memo for such a group, and the page is verified at its first touch as a
first decode verifies it, every group by its own check. The open hands
the payload back as a kind of its own (`Opened::Coded`) only to a caller
that asked (`open_group`'s `inplace`): the seek a key and the point read
read an opened group as decoded bytes and get the decode as before. Two
walks share the shape -- the cursor's (`ZCursor::walk_until`, `walk_run`,
`skip_to`, `advance_until`, `key`, `values`) and the bulk scan's
(`ZFile::scan_in`, which the block cache's clean walks and the scans with
the cache off take) -- and both read in place. `coded_key_into` refuses a
prefix longer than the first head where it would have panicked on a
forged payload, and `check_coded_group` fails such a record.
`supdb-cache64tofitzpagesrecordrawdecode` is the shape before. The tests:
the cursor's walk from every start, its position's reads rank by rank and
its seeks over the suite's shape and over every record shape, in place
and with the walk off, with the groups counted by kind (the shapes' long
keys leave groups the builder codes as blocks, decoded either way), the
memo asked nothing for a group read in place; a forged raw-tail page
whose compact header names a run longer than its tail, under a good CRC,
which falls the walk to the raw form with every key and value right; and
the codings test's scans counted in place (`zpage_inplace_groups`).

The first pairing said the walk was in and bypassed: the cursor read
74,000 groups in place over the mixes while the scans decoded 14 million,
since the block cache's clean walks go through the bulk scan path and not
the cursor, and only the counter beside the rates said so (the rates read
E 1.12-1.22x for the walk, which was the cursor's share). With both paths
in place (`zinplace2-ab-{1,2}`, `supdb-cache64tofitzpages` against
`supdb-cache64tofitzpagesrecordraw`, three million keys under 640 MiB,
both forms fitting, no major fault in either arm): 14.4 million groups a
rep read in place and 1.5 million decoded by scans where the block form
decodes 12.5 million; the raw-tail form reads the point-read mixes
1.2-1.5x faster than the block form in both orders (C 1.43-1.53x, B
1.23-1.50x, D 1.20-1.51x, A 1.0-1.05x, the read pass 1.41-1.53x), the
scan pass 1.13-1.21x faster in both orders where it read 0.70-0.72x
before the walk, and E and F level (E 1.05x and 0.91x, F 1.14x and
0.85x by order). The walk priced alone (`zinplacewalk2-ab-{1,2}`,
`supdb-cache64tofitzpagesrecordrawdecode` against the default): the scan
pass 1.85x and 2.1x faster in the two orders, E 1.21x and 1.13x, the
point-read mixes level to the order's noise (0.77-1.29 both ways), which
the walk does not touch.

So the raw-tail form is the better form on every cell where it fits, by
1.2-1.5x on the point-read mixes and the scan pass and level on E and F,
and the coding gear's trade is gone: what it picks where the form fits
loses nothing. The gear stays off one cycle longer, since its pick at the
window's lower edge flips with the process's memory at the sample's
moment and the Auto default's buy at a near-fit rung is slow; making it
the default is the next decision, and the in-place point read -- one
record lent from the payload instead of copied -- is the next piece of
the form.

## The lean process, third fit check: a trim counted by the resident bytes beyond use

The second fit check left one lever untried: a trim counted by what is
resident and free rather than by the allocator's free count, which stands
still while a thread re-uses a dead arena's chunks and frees them again. The
trim thread counts its stride that way now under
`Options::trim_by_resident` (off): `RssAnon` of `/proc/self/status` less
`mallinfo2`'s in-use and mmapped bytes, read at every look, the floor after a
trim re-read the same way (`heap::resident_beyond_use`, `heap::held`); an
allocation nobody has touched counts as in use and not as resident, so the
count runs under the truth and a trim comes late, never for nothing.
`supdb-cache64tofitzpagesrecordrawleanresident` is the lean arm with the look
at every pass end and the count, paired against `supdb-cache64tofitzpages`
at five million keys under 640 MiB with the census on (`zleanres-ab-{1,2}`),
and against LMDB (`zleanlmdb-ab-{1,2}`), the cell.

The tests (`tests/db.rs`): a dead arena built to glibc's own arithmetic -- a
thread raises the dynamic mmap threshold with one allocation under its cap,
takes thirty-two megabytes in pieces of one from its heap, touches them,
frees them and exits with a keeper of two megabytes allocated after them
alive, since glibc carves from its bins before its top and a free run under
a live chunk is a bin chunk a trim reaches where a free top under the trim
threshold is not -- reads as that much more resident beyond use and a trim
returns at least half of it; and a store with the look, an eight-megabyte
stride and the count trims again when a second thread re-touches the arena,
which the free count never sees. The first shape of the dead arena had no
keeper and its pieces coalesced into the heap's top, and `malloc_trim`
returned nothing of them: a heap's top is shrunk only past the trim
threshold, twice the dynamic mmap threshold, up to sixty-four megabytes.
The four tests that make or count a trim share a lock, since a trim is of
the whole process and a test's dead arena was returned by another test's
trim between its two readings.

At the default stride of thirty-two megabytes the count never fired: one
trim over the store's life, the install's, in 930-980 looks, with the
process 31-42 MB resident beyond use through E (anonymous 83-98 MB, in use
52) -- 15-25 MB over the floor the install trim left. The lean-resident arm
read the fit as the lean arms before it: 0.055 and 0.026 major faults a
scan in E by sitting, 0.051-0.053 a read in D, 0.041-0.044 a scan in the
scan pass, where the LZ4 form takes none; E's scans 992 and 797 ms against
584 and 583, the 195 ms between the sittings for 2,400 more faults, about
80 us a fault; D's gets 1.93x slower in twelve pairs of twelve and E 1.65x
in twelve, the point-read mixes C 0.82 and B 0.85 (faster), A 1.07 and F
1.28 (slower) by gets. The stride is eight megabytes for the arm now, and
the pairing at it follows.

Against LMDB the lean-resident arm wins every cell but E: ycsb-A 5.1x,
B 2.2x, C 2.3x, D 2.0x, F 4.4x the operations a second, the scan pass 2.1x
on one thread and 2.5x on three, and E 0.66x, in every pair of both
orders. And E's faults are LMDB's: 0.0243 a scan for the arm against
0.0253 for LMDB, whose 650 MB file does not fit the bound either (its D
0.068 a read against our 0.0125, its scan pass 0.227 against 0.059), while
LMDB's E scans read 528-535 ms median (279 in the fastest rep) against
our 892-997. So E against LMDB at this rung is not the fit, and the lean
process is not its lever: the LZ4 form, the default's coding, reads E at
583 ms with no fault at all, 1.1x LMDB's faulting median, which puts a
fault-free LMDB a good half faster than our best E. What E pays is the
walk's own time -- the overlay merge at the Zipfian head, the first
group's decode at every seek (the lean arm still decodes 1.5 groups a scan
in E, the seek's group and the skip's reopen, where the walk between them
is in place), the settle (121 ms of the lean arm's E against 51 of the
LZ4 arm's) -- and the pages are LMDB's problem as much as ours.

At eight megabytes the count fired -- four to nine trims in the mixes
over the install's one, by the census at E's end (`zleanres8-ab-{1,2}`) --
and nothing moved: the lean-resident arm's anonymous bytes in E read 82-88
MB against the LZ4 arm's 76-84 in the same sittings, and in C, B and A
52-68 against 45-70; its faults 0.047 and 0.021 a scan in E by sitting and
0.046-0.053 a read in D, as before; E 1.55x slower in twelve pairs of
twelve and D 1.90x in twelve, the point-read mixes 0.90-0.91 (faster), F
1.33 (slower). The pages a trim returns are the pages the mixes touch
next -- the seal threads' buffers, the memtable's growth, the forms --
so a trim inside the mixes lowers what the process holds at the trim and
nothing of what it holds through the mix, which is where the form's pages
compete; the count was the right count, and the lever is refuted in its
third form as in its first two. `trim_by_resident` stays an arm, off.

The rule: before a cell's fit is chased, the comparator's faults on the
same cell are read beside the engine's, since a comparator that faults as
much and reads faster names the engine's own cost as the gap; and a count
that was to replace one that never fired is read first for whether it
fired, at the stride it was given.

## The form file's seek over a coded group's heads, and where E's time is on both forms

Two profiles at five million keys under 640 MiB (`scratch/wave2/prof/profile-e.sh`,
perf over each mixes pass, the samples cut to E's window and the main thread;
`$S/profE`, `$S/profL`), the first of the forced LZ4 form beside LMDB and the
second beside the lean raw-tail arm, before any change. The LZ4 form's E thread
(8,470 samples a sitting): the LZ4 decode 24.9%, every one a cold group -- the
counters put 147,000 decodes a mix beside 107,000 memo hits, 2.5 and 1.85 a
scan, since the window of fifty crosses 2.6 groups of 2 KB and half the starts
fall in the Zipfian's cold tail -- `scan_blocks` 6.9%, the page view 4.2%, the
memo's get and the open 6.3%, the fence's search 4.4%, the walks and the runs
5.5%, the prefetch 2.5%, the settle about 2%; the harness's generator and
clock a sixth, and of that the `pow` of the Zipfian's zeta sum over the
store's keys, built at every mix's start before the mix's clock starts, a
tenth of the window and none of the measured time. The group opens by caller:
the cursor's `open_at` 14%, `ZFile::seek` 10.6%, the bulk scan 8.3%. LMDB's E
thread beside it (4,828 samples): `mdb_cursor_next` 20.8%, the page and node
searches 15%, the cursor get 3.8%, about two fifths of its thread the engine's
against four fifths of ours, which puts our engine's CPU a scan at about three
times LMDB's, and the LZ4 decode alone at what LMDB's whole walk costs.

The lean raw-tail arm's E thread (9,942 samples, on its core 81% of the window
where the LZ4 form's is 95%, the rest the device): the record coding's
whole-group decode -- `coded_rec` 6.3%, the copies 9.2%, `get_var` 1.7%,
`decode_record_group` and `decode_record_into` 2.2% -- about a fifth of the
thread, 87,000 groups a mix, 1.5 a scan, in a form whose walks decode nothing;
the faults' kernel share a sixth (`iowrite16` 8.3%, the task switches 4.6%,
the folio and fault handlers 2.7%); `coded_record_to` 4.7%, the in-place
walk's own records; `scan_blocks` 5.8%, the page view 4.5%, the fence 4.2%.
The decodes' caller was `ZFile::seek`: the scan's start seek and the cut's
opened the group as a walk opens one, with the decode, and sought the decoded
bytes, where the point read had sought the same group over its heads since the
record coding (`record_group_seek`); under the in-place walk the decode served
nobody, and under the block coding it was the group the walk would decode
anyway, hit in the memo at the walk's open.

`ZFile::seek` seeks a coded group over its heads now, nothing decoded and
nothing put in the memo, the page verified at its first touch as a first
decode verifies it; the block coding's seek is as it was. The skip-to test
holds the suite's shapes under the record coding to no decode and no memo ask
for a seek, and was held to firing on the tree before: the record-coded
fixture's seeks asked the memo 6,296 times. Paired at five million keys under
640 MiB (`zheads-ab-{1,2}`, the lean arm against the LZ4 form, both orders, the
census on): the lean arm's scans decode nothing (`zpage_scan_groups` 0 a mix
against 87,000), and against the LZ4 form it reads C 0.74 and B 0.70 (faster)
in every pair, A 0.80 and F 0.94 by gets, D 1.42 (slower) in eleven pairs of
twelve and E 1.41 in twelve, where the pairing before the change read C 0.82-
0.91, B 0.85-0.90, A 1.04-1.07, F 1.28-1.33, D 1.90-1.93 and E 1.55-1.65; its
faults as before, 0.027-0.030 a scan in E and 0.020-0.023 a read in D, which
is where D's and E's loss now lies, with the settle beside it.

Where both forms fit (three million keys under 640 MiB, `zheads3m-ab-{1,2}`,
`supdb-cache64tofitzpagesrecordraw` against the LZ4 form, no fault in either
arm) the raw-tail form reads every mix faster now: C 0.78, B 0.73, A 0.74 and
F 0.80 of the LZ4 form's time by gets, D 0.69, and E 0.80 by scans in ten
pairs of twelve, with the scan pass 1.25-1.74x by order, where before the
heads' seek E read level (1.05 and 0.91 by order) and the point-read mixes
1.2-1.5x. So the raw-tail form reads every cell faster than the block form
wherever it fits, and where it does not (five million under 640 MiB) it wins
the point-read mixes and loses D and E on the faults the block form does not
take; the gear's trade is a fit's, which is the controller's measure. What E
still pays beside LMDB at five million under 640 MiB is on the LZ4 form the
cold decode of 2.5 groups a scan and the walk around it, and on the raw-tail
form the faults and the settle; the raw-tail form's scan at three million,
decoding nothing, reads 0.80 of the LZ4 form's, which is about LMDB's
fault-free rate by the profile's shares.

## Raw tails where the reads are: the decodes by partition, and the hottest partitions raw-tailed

The raw-tail form loses D and E at five million keys under 640 MiB on its
faults alone (the settle's excess in E is the same faults, half of the
settle's samples in `owner_span_from` reading form pages that are not
resident, its CPU level with the LZ4 arm's), and the LZ4 form's E thread is a
quarter cold decode. A probe counted each form file's opens, decodes and memo
hits by partition, printed after each mix under `SUPDB_MIX_HEAT`
(`heat_read.py` differencing consecutive mixes); what stays of it is the
decodes alone, counted once in sixteen decodes per thread so that no reader
thread writes a shared line on every decode (`ZFile::decodes16`,
`Db::zpage_heat`). The suite's
Zipfian is unscrambled, so the reads pile onto the low keys: of E's 147,000
decodes a mix, 49.7% fall in the first of nineteen partitions, 61.9% in the
first two and 74.3% in the first four, each 290,000 keys and 21 MB of LZ4
form (32 MB raw-tailed); B and A put 54% and 52% of theirs in the first, and
D, whose reads land evenly, 5-6% in each.

So raw tails on the first partitions cost tens of megabytes where the whole
raw-tail form cost 196 over the LZ4 form's 360. `Options::zpages_raw_tails_first`
(an experiment, off) builds the first N partitions by key order in the
record coding with raw tails and the rest in the block coding; key order
stands in for a heat no build can know before the reads, so these arms are
the lever's upper bound and not a policy. `supdb-cache64tofitzpageshot4` and
`supdb-cache64tofitzpageshot1` are the forced LZ4 arm with four and one.
`tests/zpages.rs` holds a store under the option to the raw form's answers in
every read, count and scan, exactly the first partitions' builds raw-tailed,
the scans reading those groups in place, and the raw-tailed files larger a
key.

Hot4 against the LZ4 form (`zhot4-ab-{1,2}`, both orders, five pairs a
sitting, the census and the heat on): no major fault in any mix for either
arm; E's decodes 37,326 a mix against 147,000, three quarters gone as the
probe said; E's scans 0.877 of the LZ4 form's time in twelve pairs of twelve,
C's gets 0.79, B 0.84, A 0.80, F 0.84, D 0.97 (level), the process's
anonymous bytes in E level (76-100 MB against 76-94). And the read pass, the
uniform point reads right after the build, read 0.78-0.80 of the LZ4 form's
rate in both orders, with 0.013-0.014 major faults a read where the LZ4 form
takes none, and its threaded reads 0.74-0.89; the scan pass level to the
order's noise with 0.007-0.017 faults a scan. A pass that reads every
partition needs the whole form resident, and the 44 MB over the LZ4 form's
360 pushed it past the fit at this rung, where the Zipfian mixes' working
set still fits. The LZ4 form reads that pass at 1.79x LMDB's, so the loss is
a penalty on a cell already won, and key order is not a rule a store can
ship.

Hot1 against the LZ4 form (`zhot1-ab-{1,2}`): no fault anywhere, the read
pass level (0.96-0.98, the threaded reads 0.99-1.01), the scan pass level; C's
gets 0.86 of the LZ4 form's time, B 0.85, A 0.88, F 0.90 in ten to twelve
pairs of twelve, D 1.00; and E 0.977, level, though the first partition's
raw tails took E's decodes from 147,000 a mix to 73,000. Hot4 took the next
36,000 and read E 0.877. So the first partition's 74,000 decodes were worth
about 0.13 µs each and p1-p3's 36,000 about 1.4: the first partition's
groups come back so often that their decodes run warm, at about what the
in-place walk of the same records costs, and the cold decodes are the ones
a raw tail saves, at the cycling figure the decoder probe gave. The heat
that chooses a partition's coding is the cost of its decodes, not their
count; the hottest partition is not the most profitable.

Hot4 against LMDB (`zhot4lmdb-ab-{1,2}`): hot4 wins every cell but E in both
orders -- the read pass 1.46-1.71x LMDB's, A 4.9-5.8x, F 4.1-4.9x, B 2.2-2.3x,
C 1.27-1.44x, D 1.45-1.88x, the scan pass 1.21-1.23x, none of the mixes
faulting where LMDB takes 0.005-0.04 a operation -- and reads E at 0.71-0.75x,
LMDB's E faulting 0.011-0.017 a scan in this sitting against 0.025 in the
one before and running 1.4-1.6x faster than it did there. Hot4's E is about
6 µs a scan of the engine's own time with most of its decodes gone, where
LMDB's is about 4.6 with its faults; the walk around the decode is E's gap
now.

What it says. A coding chosen per partition is the lever the probe pointed
at, and the arms by key order bound it: up to a fifth off the Zipfian mixes
where the extra bytes fit, a fifth onto a uniform pass where they do not,
and the pass's fit line at this rung between 371 and 404 MB of form against
a budget the controller's model puts at about 486 MiB. A shipping choice has
three inputs the arms do not have: the heat as decode cost by partition,
which no build can know before the reads and which moves (the suite's
uniform passes run before its Zipfian mixes), so a partition is re-coded
while the store serves; the fit, measured by the census's faults and not by
the model's budget, which is too generous by about the hot4 arm's extra;
and a way back when either turns. Both arms stay, off.

## E with most of its decodes gone: where hot4's E thread goes beside LMDB's

Profiled at five million keys under 640 MiB, hot4 beside LMDB in one sitting
(`scratch/wave2/prof/profile-e.sh`, perf over each mixes pass, cut to E's
window and the main thread): E's scans 480-623 ms against LMDB's 339-388, LMDB
faulting little in this sitting. Of hot4's thread (6,024 samples, the
harness's Zipfian setup a tenth of the window and outside the mix's clock):
the start seek 15% (`cache_seek_exact` and `ZFile::seek`: the page view 2.8%,
the heads' search 4.3%, and the cold partitions' group opens and decodes),
the block walk through the cursor 18% (`PartReader::scan`, `walk_run` 15%),
the clean walks 13% (`cache_scan`, `part_scan_z`), the block builds over the
mixes' unsealed updates 10.4% with their deltas 6.3% (`BuildCtx::materialize`,
`deltas_for`), the settle 6.5%, the LZ4 decode of the partitions past the
first four 7.4%. In the walk, the record's first parse (`coded_rec` from
`walk_run`, 10.8% inclusive) is the first touch of the record's bytes and
the second parse of the same record inside `coded_record_to` 1.1%, so the
in-place walk's cost is the memory it reads, as LMDB's `mdb_cursor_next`
(19% of its thread) is. LMDB's seek is about as large as ours (its page and
node searches 15%); what LMDB has no counterpart of is the overlay of the
mixes' unsealed updates on E's blocks, a sixth of our thread, since LMDB
updates its tree in place. No single lever is left in E's walk; the
overlay's builds and the seek are the two largest pieces.

## What stays resident beside a form, and the raw pages dropped at its install

The fit line hot4 crossed was read from the kernel and not the model. A probe
walked every file of the store with `mincore` at the read pass's start and
end and at E's end, by kind and by tenth of each file
(`scratch/wave2/probe/resident.py`, beside the pairing by
`scratch/wave2/probe/watch.sh`; `zhot4-resident.txt`). Under the LZ4 form at five million keys
under 640 MiB the store's files held 496 MiB: the form files 373, whole; the
partitions' raw files 85 MiB of 682, all of it in their last two tenths,
the key section, which the raw point read's hash probes and the open's
check of the section's checksum row reads whole; the ordered indexes 38 MiB,
whole. Under hot4 the form files held 397-417 of 418, the raw files 70 and
the ordered indexes 29: the page cache gave the form's extra 45 MiB out of
the raw key section and the indexes, which no read through a form touches,
and still came short of the form at the read pass's start. The build drops
the records behind it already; the key section and the indexes were what
it left, and a cache drop does not take them, since the open's mapping of
the key section holds its pages and `POSIX_FADV_DONTNEED` skips a mapped
page (an `MADV_COLD` over the mappings first only deactivated them and the
residency did not move).

`Options::zpages_drop_raw` (an experiment, off) unmaps the store's own
mappings of a partition's raw file and its ordered index once a form file
serves the partition (`MADV_DONTNEED`, which changes no byte of a shared
file mapping: a later touch faults the page back), then drops the files'
clean pages (`POSIX_FADV_DONTNEED`), counted as `zpage_raw_dropped`.
`tests/zpages.rs` holds a store under it to the raw form's answers, to one
drop per form file installed, and to its raw files' residency, by
`mincore`, under a quarter of what the same store holds without it.
`supdb-cache64tofitzpagesdrop` and `supdb-cache64tofitzpageshot4drop` are
the forced LZ4 arm and its hot4 arm with it.

The drop alone against the LZ4 form (`zdrop-ab-{1,2}`): no major fault in
either arm in any pass, since the LZ4 form fit without it, and no minor
faults more: the read pass 1.01 and 0.97 of the LZ4 form's rate, the scan
pass 1.19 and 1.16, the mixes 0.94-1.05 in the first sitting and 0.90-0.96
in the second, three or four pairs of five each, which nothing in the two
arms' reads differs by, and under the tenth a pairing resolves.

Hot4 with the drop against the LZ4 form (`zhot4drop-ab-{1,2}`): no major
fault in either arm in any pass, where hot4 without it took 0.013 a read in
the read pass; the read pass 1.07 and 1.10x the LZ4 form's rate, threaded
1.11-1.13 and 1.04-1.11, the threaded scans 1.08-1.10 in both orders; C
1.27 and 1.25, B 1.12 and 1.32, E 1.13 and 1.20; the single-threaded scan
pass 1.14 and 0.98. A, F and D read 0.85 and 1.05, 0.89 and 1.20, 0.98 and
1.02, the slower column the same in both orders whichever arm sat in it,
and their time is the commits': 418 commits of a durable store, each an
fdatasync of the log, 180-280 ms of A's 270-390 against the gets' 60-85.
Whether those commits were the arm's was asked of the device. A third
sitting ran with a sampler beside it (`scratch/wave2/probe/iorun.sh`:
`iosample.py` every 50 ms reads the global and the group's dirty bytes and
`/sys/block/vda/stat`'s writes and flushes, `io_read.py` cuts them to the
phasemarks; `zhot4drop-io.*`). Both arms made the same device work in every
mix: A 11.5 MiB in 1,650 writes and 418 flushes, F the same, B, D and E a
megabyte and 41-42 flushes. The mixes build, seal and merge nothing, the
upkeep thread ran a millisecond a mix, and the commits' time overlapped rep
for rep, A's 210-269 ms under the LZ4 form against 163-279. So A, F and D
are the device's flushes, whose sign moved with the sitting, and no pairing
of two arms that share the write path resolves them. What did differ is
where the bytes went before the mixes. The LZ4 arm's ordered indexes,
written unsynced, sat 38-39 MiB dirty in the group through every pass and
were removed with the store unwritten, the store's life being shorter than
the kernel's thirty seconds; the drop wrote them back at the install (the
load's device bytes 1,180-1,185 MiB against 1,095-1,098: the form's 45 MiB
more, and the indexes). And the group's page cache held 449-464 MiB under
hot4 with the drop against 497-512 under the LZ4 form without it.

What it says. The drop widens what the bound holds for a form by the raw
key section and the indexes, about 120 MiB at this rung -- the group's cache
48 MiB smaller under a form 45 MiB larger -- at a cost no pass shows, and with
it hot4 reads every cell at or above the LZ4 form's. It stays an
experiment, off. A raw read after it (a merge's inputs, and under `Auto`
the controller's trial and its exit) reads the key section back from the
device, and the trial measures the raw form's rate from what the drop left,
so under `Auto` the drop changes what the controller measures; the forced
arms are where it is priced. The question it opens is the fit of the
raw-tail form under it, 195 MiB over the LZ4 form's at this rung.

## The fit line under the drop

Two arms place it: `supdb-cache64tofitzpageshot8drop`, the first eight
partitions by key order raw-tailed with the drop, and
`supdb-cache64tofitzpagesrecordrawdrop`, the whole raw-tail form with it,
each paired against the LZ4 form without the drop at five million keys
under 640 MiB in both orders, with the census, the heat and the io sampler
(`scratch/wave2/probe/iowatch.py`, which follows the newest bench group
across a run of several processes; `zhot8drop-io.samples`,
`zrawdrop-io.samples`).

Hot8 with the drop, 444 MiB of form against 373 (`zhot8drop-ab-{1,2}`): no
major fault in either arm in any pass, and hot8 faster in every cell of
both sittings -- the read pass 1.10 and 1.17x, threaded 1.08-1.15, the scan
pass 1.12 and 1.19, threaded 1.13-1.14; C 1.33 and 1.28, B 1.24 and 1.23, E
1.13 and 1.19, F 1.16 and 1.12, D 1.35 and 1.04, A 1.07 and 1.03, the reads
and E in nine or ten pairs of ten. The group's page cache held 474-489 MiB
through the passes against the LZ4 arm's 496-511 (38 of those its ordered
indexes' dirty pages), the process's anonymous bytes in E 80-104
against 77-90, its bytes in use 67 MiB in both and the difference the
allocator's free bytes, rep to rep. So about 47 MiB of the bound were left
in E.

The whole raw-tail form with the drop, 568 MiB (`zrawdrop-ab-{1,2}`): the
form does not fit. The group's cache stood at 511-558 MiB beside 102-107 of
anonymous bytes, at the bound, and the reads faulted: 0.076-0.082 major
faults a read in the read pass, which read 0.44 and 0.47x, the threaded
reads 0.48-0.79; D 0.51 and 0.56 at 0.048 faults an operation, E 0.93 and
0.55 at 0.018 and 0.080; while B and C, whose reads stay in the first
partitions, read 1.10-1.30x with 0.001-0.003 faults.

So at this rung the drop moves the fit line from between 373 and 418 MiB of
form to between 444 and 568, and by the room hot8 leaves, about 490: the
bound less the process's anonymous bytes in E and the 45 MiB of other
cache the group holds (the log, the harness's own text). A form
whose extra bytes go to the partitions the reads decode most, up to that
line -- key order is the decode order here, the suite's Zipfian being
unscrambled -- reads every cell above the LZ4 form's; past it, the uniform passes
pay the device for every page that does not fit. That is the shape a
coding chosen per partition by its decode cost can ship: the raw tails
built where the decodes are, while the store serves, to a line read from
the group's faults rather than the model's budget, and given back when the
faults come.

Hot8 with the drop against LMDB (`zhot8lmdb-ab-{1,2}`), LMDB faulting 0.05
a read, 0.13 a scan and 0.015 in E in this sitting: the read pass 2.63 and
2.32x LMDB's, threaded 2.3-3.3x, the scan pass 1.51 and 1.15x, A 5.5 and
5.1x, F 4.4 and 4.3x, B 2.3 and 2.4x, D 1.73 and 1.56x, C 1.46 and 1.43x,
and E 0.75 and 0.69x -- where hot4 read it 0.71-0.75x. The decodes the next
four raw-tailed partitions took were worth the LZ4 form's cells and not
E's against LMDB; E's gap is the walk around the decode, as the profile of
hot4's thread put it.

## Re-coding a partition while it serves, by what its decodes cost

The key-order arms know where the decodes will be before a read is made;
a store does not. `Options::zpages_recode` (an experiment, off, the forced
form) builds a served partition's form file again in the other coding and
swaps it into the segment's cell (`ZCell::replace`, the old form file
retired under the epoch as a taken one is; the build's rename over the
name unlinked it already). A policy on the segment work's turn decides,
once a second: each form file's decode time over the second, from the
decode that `count_decode` samples, one in sixteen per thread, now timed
where the option is on (`ZFile::decode_ns16`, the decodes outside the memo
counted too); the platform's budget (`zfile::read_budget`, the limit less
the anonymous bytes and the model's margin); and the group's file refaults
a second. It re-codes the block-coded partition whose decodes cost the
most to the raw tails, past a floor of 2 ms a second, while the form files'
bytes with its estimated extra stay within the budget; over fifty refaults
a second it gives the last re-code back and holds the form files' bytes
there for a minute. A form file knows its own coding from its groups'
directories (`ZFile::coded`). `tests/zpages.rs` drives the policy through
the injection seam: the hot partition re-coded alone, every read, count and
scan right across each swap and no read falling back to the raw form, the
re-code given back at the refaults and held by the ceiling, re-coded again
once the hold has passed, and nothing re-coded with no room in the budget.
The trace (`SUPDB_ZPAGES_TRACE`) prints each look's decision (`zrecode`).

Paired at five million keys under 640 MiB (`supdb-cache64tofitzpagesrecodedrop`,
the drop arm with the policy): against the drop arm (`zrecode-ab-{1,2}`)
level by the sign of each sitting, and against hot8 with the drop
(`zrecodehot8-ab-{1,2}`) behind it in C (0.69-0.76), B (0.71-0.78) and E
(0.88-0.93) in both sittings. Neither arm faulted. The trace says why: the
policy found the fit by the budget on its own, re-coding nine or ten
partitions a rep to about 480 MiB of form with no fault, but it
spent the room on the uniform read pass, where every partition decodes as
much as every other and the one-second heat picks them in whatever order
the second's noise gives (p3, p17, p16, p18, p13, p1, p0, p12 in one rep);
by the scans it was at the budget, and the Zipfian mixes, a few hundred
milliseconds each at the suite's scale, came too late and too short for a
one-second look to move it. Where the budget rose in the mixes, a build ran
during B and A, and B's gets read level (median 128 ms against 132, twelve
reps an arm) while its commits read slower (53 against 32): a build writes
some 30 MiB of form file, and the durable log's next fdatasync pays the
device's flush of those bytes, about 0.85 ms a megabyte here, as `CLAUDE.md` prices it.

What it says. The mechanism holds -- the swap, the retire, the budget's fit
under the drop -- and the heat it needs is not a second's: a uniform pass
makes every partition look as hot as the next, and only a heat that
outlives a pass, with a way to swap a raw-tailed partition the reads have
left for a block-coded one they have come to, can find where the decodes
stay. A raw-tailed partition decodes nothing, so its heat is its group
opens, priced at what they would cost decoded. A build is not free to the
writer either: its bytes are a device flush the durable commits pay. The
suite's scale gives a policy a few looks in the mixes, so it is priced on
a longer shape (`--ops-div 1`) next, beside the key-order bound.

Its second version (`zrecode2hot8-ab-{1,2}`): a partition's heat kept by
its name and decayed over ten seconds; a raw-tailed form file's heat its
record-coded groups opened without a decode, counted one in sixteen per
thread (`ZFile::opens16`) and priced at the block-coded files' mean
decode; with no room, the coldest raw-tailed partition given back to make
it when the hottest block-coded one is twice as hot; over the refault
floor, the coldest given back. `tests/zpages.rs` holds the swap: room for
one raw-tailed partition, the first heated re-coded, the heat moved to
another, the first given back and the second re-coded into its room, every
read and scan right. Priced at `--ops-div 1` (ten times the operations, a
read pass of some forty seconds and mixes of several each) against hot8
with the drop; the pairing against the drop arm was stopped by the
session's limit on a background run in its first sitting
(`zrecode2drop-ab-1.err`, no table). At that scale the mixes' writes grow
the process's anonymous bytes by about 150 MiB, the budget falling from 472
MiB to 315, below the LZ4 form's 373, and both arms fault in A, D, E and F.
The policy re-coded eight partitions a rep in the read pass, the decay no
longer than the pass, and gave all eight back as the refaults rose to
3,000-8,700 a second in the mixes, ending at the LZ4 form. Against hot8
with the drop, whose raw tails stay: D 1.24 and 1.12x, E 1.18 and 1.10x,
on fewer faults (D 0.079-0.089 an operation against 0.129-0.134, E
0.025-0.029 against 0.057-0.064); B 0.99 and 0.81, C 0.97 and 0.80, where
nothing faults and hot8's raw tails serve the Zipfian head; A and F by the
sitting's sign, the read pass level.

What the second version says. The way back is worth having: when the
process grows into the room the forms had, giving the raw tails back takes
faults off the mixes that a static choice keeps. The way in follows
whatever pass is running, and the suite's passes, a uniform one first and
the Zipfian mixes after, each outlast any decay short enough to follow the
next, while a build cycle costs seconds and a device flush to the writer.
The key-order arms bound what the way in could win; a store whose hot set
holds still is the shape it needs, which a heat kept across the store's
restarts would give it at its build, and the suite does not run.
