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
