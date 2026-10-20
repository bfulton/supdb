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
