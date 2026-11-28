# Flake hunt 1: the `test` and `stride` groups, and the CI history

The tree is `abeecbdd`. Every time is UTC. No timing run was made. A probe
is a patch under `release/findings/flake-hunt-1/` that applies to
`abeecbdd`. It adds a seam behind an environment variable, and it is never
committed to `src/` or `tests/`.

Read with `release/findings/flakes.md` on `claude/supdb-pretag-flakes`,
which owns the cipher cache tests, `cipher_looks`, the room-following pager
test, the durable run-close, drain and fsync-failed-close tests, and the
two `tests/settle.rs` copy tests. Those are not taken up here.

## The table

| Test | Where seen | Cause | Evidence (a command) | State |
|---|---|---|---|---|
| `tests/db.rs` `a_commit_that_freezes_leaves_the_carry_to_the_upkeep_thread`: "the thread made the pass" | CI run 45910250515, `test (ubuntu-24.04)`, `stride`, main `3706d2bf`, 2026-11-26 23:36 | **The test's premise.** The upkeep thread counts a pass (`lend.passes.fetch_add`, `src/db.rs` `keep_upkeep`) after `upkeep_pass` returns, and the pass has published `forms_at` before it returns. The test waited for `forms_position() != usize::MAX` and then read `upkeep_counts()[0]` once, which can fall between the publish and the count. | `late-pass-count.diff`, then `PROBE_LATE_PASS_COUNT=1 $B --exact a_commit_that_freezes_leaves_the_carry_to_the_upkeep_thread` (B = `cargo test --profile checked --test db --no-run`) | Fix written (waits for the position and the count together), not yet proved |
| `tests/zpages.rs` `a_merges_outputs_are_built_in_the_block_form_where_the_room_is_short`: `NotFound` at the read of a listed form file | CI run 45987518586, `test (ubuntu-24.04)`, `stride`, `0b2699d5`, 2026-11-27 11:20 | **The test's premise (named from the code, not yet forced).** `form_files` lists every `zpg-*` name, `zpg-dead-*` included, and then reads each one. The segment work renames a retired partition's form file to a dead name and unlinks its spares at its idle tick (`Maint::sweep_spares`). Both happen on its own schedule, after `merges_in_flight() == 0`. | `list-gap.diff`, then `PROBE_LIST_GAP_MS=500 $B --exact a_merges_outputs_are_built_in_the_block_form_where_the_room_is_short --nocapture` (B = `--test zpages`) | Lead; to be forced |
| `tests/zpages.rs` `a_merges_outputs_have_their_form_files_when_it_lands`: `sample_refreshed 0` with `sel_installs 1` (zpages.rs:10329) | From @parent: phase 1 track A, 2 of 6 instances beside a whole gate run | **Lead.** The install can follow a sample already in flight that is collected at the same generation after the builds end. That reading is not stale, so `z_tick` refreshes nothing, and the selector installs from the sample. Two mechanisms reach the one end state, and the test asserts the second. The failing counts (`samples`) have not been read yet. | To do: six instances of `$B --exact a_merges_outputs_have_their_form_files_when_it_lands` beside a `check.sh test` run in another worktree | Lead |
| `tests/db.rs` `a_load_nobody_reads_files_no_forms`: the far arm built no forms | CI run 43642290174, `test (ubuntu-24.04)`, `a4fb791d`, 2026-11-14 | Open. Diagnostics were added in `8d1d68b8`, and it has not failed since in CI. | To do: the test six at a time beside four busy loops, at head and at `a4fb791df53b2f4fcfb3186d2e9e78c6dce6675f` | Open |
| `tests/db.rs` `reader_threads_over_forms_a_buffered_writer_files_at_its_commits`: "a scan went backwards: key 652 scanned 20 after 60" (a wrong answer) | CI run 45443958511, `test (macos-latest)`, `5a083d3b`, 2026-11-24 17:08 | **The engine, fixed.** A writer's drop left the forms it had published, and a look that rebased with every table gone kept the forms of partitions it held no table for. The handle took the published copy as the block. | `54fecff2` (PR #27): `a_writer_that_drops_its_forms_withdraws_what_it_published` and `a_look_withdraws_the_forms_of_a_partition_it_holds_no_table_for` build both states through seams and fail without their halves; `check_published` asserts it in a checked build | Fixed in history |
| `tests/db.rs` `the_sweep_at_an_operations_end_finds_the_pin_gone`: "(0, 300) waited under it / left: (0, 75)" | CI run 45439873596, `test (macos-latest)`, `stride`, `59454e02` | The test's premise: the segment work on its own thread pinned beside the sweep under the stride switch | `54fecff2` sets `publish_in_background = false` for the fixture, with the reason in the comment | Fixed in history |
| `tests/zpages.rs` `a_form_build_under_a_cipher_says_what_its_file_is_to_hold` | CI run 46022839187, `test (ubuntu-24.04-arm)`, `054bf219` | The test's premise: a baseline was read after the open while builds start at the open | `c16ec3dd` (forced by a 2 s sleep after the open) | Fixed in history |
| `tests/onefile.rs` `an_opens_publish_tells_the_table_its_manifest_is_on_the_device`: "the premise: nothing published since / left: 6" | CI run 46023605914, `test (macos-latest)`, `stride`, `de366221` | The test's premise: a form build of the closed store unfinished at the close was built by the next open | `0e6ec308` (forced with the thread held 1.5 s) | Fixed in history |
| `tests/zpages.rs` `a_spare_started_as_a_settle_ends_is_not_the_settles_to_sweep` | CI run 45487933741, `test (ubuntu-24.04)`, `stride`, main `5b8d64ae` | Gone: the spares and the test went in `745febd9` | `git show 745febd9 --stat` | Closed |
| `tests/zpages.rs` `builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records`: "forms 64/365" | CI run 41271819751, `b02e5b18`, 2026-11-01 | The engine: `WILLNEED` capped by the kernel at the device's request size | `d68b30af` (forced at a device of 256 KiB requests) | Fixed in history |
| `tests/zpages.rs` `the_model_moves_the_rung_between_fits_and_suspect_on_the_injected_budget`: "raw throughout" | CI run 41253995880, `18d808ca`, arm | The test's premise: it asserted that no build happened while builds ahead at landing start past half the budget | `786082d3` | Fixed in history |
| `tests/runlog.rs`, four tests: the binary was killed by SIGBUS under `stride` | CI runs 45886882900 and 45897245358 | The engine's tests' crash left the dead store's threads running, so a builder truncated a name a mapping read | `e8900661` (PR #31) | Fixed in history |
| `tests/db.rs` `the_trim_thread_counting_by_resident_bytes_trims_a_dead_arena_again` | CI runs 41567217677 and 45439873596 | A process-wide count in a shared binary | `5a083d3b` moved it to `tests/resident.rs` | Fixed in history |

The other race-shaped failures in the history were each fixed by a change
to the test's own wait: `dfb83730`, `a4fa9a70`, `2f73388a`, `ab62ac36`,
`7fda1f94`, `21d5f91f`, and `331bc809` for the hot-raw policy test. The
full listing is the CI section below.

For the other hunt (the `onefile` group, the lib, the suite):
- `tests/zpages.rs` `a_build_ahead_bought_while_in_flight_is_trusted_at_once_where_its_read_back_read_the_device` (run 46023605914, `onefile`)
- `builds_ahead_under_a_cipher_seal_their_form_files_and_keep_them_cold` (run 46007723722, `onefile`)
- `db::pager::tests::an_advice_given_before_the_envelope_is_the_envelopes_when_it_comes` (run 45998304848, `cross-arm`)
- `a_pass_in_its_own_process_answers_the_row_the_pass_in_this_one_does`, exit 2 on ubuntu-24.04

All of these were sent to @parent.

## The loop

`sh scripts/check.sh test stride` on `abeecbdd`, with `TMPDIR` a directory
of its own and its stores removed between runs. The driver is
`release/findings/flake-hunt-1/loop.sh LOAD N`.

| Load | Runs | Failures |
|---|---|---|
| quiet | 3 whole (13:25 to 14:10), all green; under way | none |
| two busy loops | not started | |
| four busy loops | not started | |

## The CI history

Read through the Actions API: every workflow, every page, and the jobs of
every attempt. `ci` had 240 runs, the oldest from 2026-08-22. 199 jobs
failed and 32 were cancelled. The logs of 33 had expired. No job timed
out, and no log shows a watchdog's exit, a full disk or an `ld` bus error.
Most failures were deterministic and were fixed by a named commit:

- the macOS zpages controller tests and the bench `/proc` reads (`9e9a48fd`)
- `a_merges_asks_ahead_only_where_its_inputs_are_not_resident` (`7a6d5142`)
- `an_ask_ahead_brings_the_cold_pages_in` (`b02e5b18`)
- the macOS thread clock (`b02e5b18`)
- the 71 macOS cipher tests (`5241ae34`)
- the qemu advice tests (`786082d3`, `21d5f91f`, `dd83d582`)
- clippy releases, lint and fmt, and the cost gate before it left the default set
- runner-image breakage

The flakes are the rows above.

## Next steps

1. Build the worktree with both probes: `cargo test --profile checked --test db --test zpages --no-run`.
2. Force B: the old test should be red and the fixed test green, under `PROBE_LATE_PASS_COUNT=1`. Then the mutant: remove `lend.passes.fetch_add`, and the fixed test fails.
3. Force G: under `PROBE_LIST_GAP_MS`, check whether the listing holds a `zpg-dead-*` name or a name that vanishes. If so, fix the test to read the files the store's live segments claim, with the segment work paused over the reads (`Db::pause_segment_work`).
4. I2: six instances beside a gate run, and read `samples` in the failing counts.
5. H: six at a time beside four busy loops.
6. The loop: the rest of the quiet runs, then 10 under two busy loops and 10 under four.
