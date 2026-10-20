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
   trial.** `ZFile::sample` over thirty-two groups of a partition: the
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
