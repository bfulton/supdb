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
