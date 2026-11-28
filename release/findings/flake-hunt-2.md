# Flake hunt 2: the onefile group, the lib, the suite, the fuzz, the premises

The second of two hunt sessions. Its scope: the `onefile` group, the
library's unit tests (`cargo test --profile checked --lib`), the suite's
tests (the `bench` group's `cargo test --profile checked --lib --bins
--tests` under `bench/`), the fuzz past the gate's ranges under
`ZFUZZ_CIPHER=1` and `ZFUZZ_ONEFILE=1`, and the environment's premises.
The tree is `abeecbdd`. Times are UTC. No timing run was made.

This file is the handoff, kept current as the hunt goes.

## The table

| Test | Where seen | Cause | Evidence (command) | State |
|---|---|---|---|---|
| `tests/durable.rs` open-adoption tests (5: `acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment`, `an_open_makes_the_slot_it_takes_durable` and `_under_a_cipher`, `an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable`, `an_open_that_adopts_several_pending_pieces_makes_each_durable`) | `TMPDIR` on tmpfs (`flakes.md`, "Seen in the same logs") | **The test's premise.** The instruments `os_page_unwritten` (page flags) and `os_page_kept_unwritten` (a drop of clean pages, then `mincore`) read a page as written back once an fsync returns. On tmpfs nothing is behind the page: it is dirty for good and stays in the cache after `POSIX_FADV_DONTNEED`. So every page reads unwritten, fsynced or not. | `/home/user/hunt/pf.c` (below): on the disk, written `dirty=1`, fsynced `dirty=0`; on tmpfs, fsynced `dirty=1`, resident after DONTNEED. | Fix drafted (tests only), not yet run: both instruments ask a probe first, which writes and fsyncs a file of its own in `TMPDIR` and must read it as written back. Where it does not, they answer `None` and the tests hold the open's calls alone, saying so. A new test holds the probe to `statfs`'s filesystem type, both ways. |
| `tests/zpages.rs` `builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records` | a page-cache drop while it runs (the cost count drops it) | **The test's premise**: nothing else drops or reclaims the page cache between the build and the residency read. A drop fails the "kept cached" checks and makes the "dropped" checks pass without testing anything. | To take: the test under a drop every 3 s. | Planned: a witness file made at the fixture, its pages read at each residency check. A witness that lost pages fails the test with a message naming the premise. |
| `tests/zpages.rs` `a_merges_outputs_are_built_in_the_block_form_where_the_room_is_short`, and its sibling `a_merges_outputs_are_built_in_the_stride_form_at_their_finish` | `onefile` group, quiet, loop run q-7 (14:33): `zpg-dead-00000001.zpg: No such file or directory`, at `tests/common/mod.rs:163` | **The test's premise.** A landing that retires a segment renames its form file to a dead name (`zfile::zpg_dead_name`, `db.rs` at the retire), and the segment work unlinks it at its idle sweep (`Maint::sweep_spares`, after `collect`, `z_step` and the looks). The test's wait (finish builds ended, no merge in flight) does not cover the sweep. Its listing (`form_files`, every `zpg-` name) takes the dead name, and the sweep unlinks it before `store_file` reads it. Read before the sweep, a dead name is an old partition's form: the sibling would then call it "not a stride file". | `git apply release/findings/flake-hunt-2/sweep-probe.diff` at `abeecbdd` (the sweep held 2 s; the test waits for the unlink between listing and reading), then `PROBE_HOLD_SWEEP=1 PROBE_SWEEP=1 SUPDB_TEST_ONEFILE=1 $B --exact a_merges_outputs_are_built_in_the_block_form_where_the_room_is_short a_merges_outputs_are_built_in_the_stride_form_at_their_finish --test-threads 2 --nocapture`: both red 4 of 4, the loop's message, plain and in one file. | **Fixed in the test** (`2648e8d`). Both read `live_form_files`, the listing without dead names. Under the same hold: green 6 of 6. The whole `zpages` binary passes plain and with `SUPDB_TEST_ONEFILE=1`, and clippy passes on it. The tests that check what becomes of a dead name keep `form_files`. |
| `db::pager::tests::an_advice_given_before_the_envelope_is_the_envelopes_when_it_comes` | `cross-arm` (qemu), run 45998304848, at `5038d7e` | **The emulator.** qemu-user accepts `madvise` and passes it to no kernel. The first check ("not read random") holds by default, and the check after an advice reads `Some(false)`. At head the test is under `not(supdb_emulated)`, with that reason in its doc. | To take: `sh scripts/check.sh cross-arm` with the test's cfg as at `5038d7e` (red) and as at head (green). | Explained; fixed at head before this hunt. qemu red and green owed. |
| bench `tests/fresh_process.rs` `a_pass_in_its_own_process_answers_the_row_the_pass_in_this_one_does`, exit 2 | `test (ubuntu-24.04)`, runs 43629003662 and 43632926364, 2026-11-14, branch `claude/crash-fixes` | **The suite's refusal of a RAM-backed device, misreading a device-mapper disk.** The child printed `bench run: refused: the stores would be on dm-0, a RAM-backed block device`. The ARM job of both runs passed. All 240 completed `ci` runs were searched; no other failure has this shape. | `bench/CLAUDE.md`, "A device filed as virtual that was a disk's"; `env::virtual_block_device` and its sysfs-tree test at head. | Explained; fixed in #27 (`7650d890`, 2026-11-24), held by the env test, which has passed in every loop run here. A mutant of the old check is still to be shown red. |
| `tests/zpages.rs` `a_build_ahead_bought_while_in_flight_is_trusted_at_once_where_its_read_back_read_the_device` | `onefile` group, run 46023605914, at `de36622` | **Lead.** Failed in its fixture `stride_store_with_a_build_ahead_held`: `(build_starts, prebuilt, left_seals)` was `(17, 17, 16)`. One build ahead's read-back found pages of its own file in the cache after three drops (`zfile::reads_device`), so it gave up its word. Who put them there is not yet named. | To take: `sh /home/user/hunt/trace.sh OUT env SUPDB_TEST_ONEFILE=1 TMPDIR=… $B --exact <test>`, the trace's `mm_filemap_add_to_page_cache` lines read for the task that added pages in the build's region. | Open. |
| `tests/zpages.rs` `builds_ahead_under_a_cipher_seal_their_form_files_and_keep_them_cold` | `onefile` group, run 46007723722 | **Lead.** One envelope had 56 of its 56 pages cached after `vault::persist_form`'s cold path (`written_back(0, 0)`, its result discarded, then `drop_all`, before the rename). Who read it back, or whether the drop was skipped (pages dirty or mapped), is not yet named. | The same trace, with the `fadvise64` and `sync_file_range` syscalls beside it. | Open. |

## The loop

`/home/user/hunt/loop.sh LOAD N TAG` runs, N times under LOAD busy loops:

- `sh scripts/check.sh onefile`;
- the lib's tests;
- the suite's tests;
- `fuzz_range` under `ZFUZZ_CIPHER=1` and again under `ZFUZZ_ONEFILE=1`, 96 new seeds a run from 48 up;
- `concurrent_range` the same way, 16 new seeds a run from 8 up.

Each run has a `TMPDIR` of its own under `/home/user/hunt-tmp`, removed after the run. It writes a line a step to `/home/user/hunt/logs/summary.txt` (step, rc, the failing test names). A whole run is about 8 minutes on this four-CPU VM.

| Load | Runs | Failures |
|---|---|---|
| quiet | 10 of 10 (13:38-14:52) | 1: run 7, the merge test above (fixed) |
| 2 busy loops | started 14:53; 1 done at 15:11, clean | |
| 4 busy loops | 0 of 10 | |

## The premise runs

`/home/user/hunt/premise.sh MODE TAG` runs every test binary once: the crate's `cargo test --profile checked --no-fail-fast` and the suite's tests. The modes are `drop` (`echo 3 > /proc/sys/vm/drop_caches` every 3 s), `tmpfs` (`TMPDIR` on a tmpfs mount) and `mem` (a process holding all but 3 GiB of the available memory). Not yet run.

## Next steps

1. The 2-busy loop is running (`nohup sh /home/user/hunt/loop.sh 2 10 b2`). Then `… 4 10 b4`.
2. Trace the two zpages onefile failures. Build `$B` with `cargo test --profile checked --test zpages --no-run`, then run `SUPDB_TEST_ONEFILE=1` four at once beside four busy loops under `trace.sh` until one fails, and read who added the pages.
3. Run `premise.sh` in each mode. Then compile the durable fix in `/home/user/wt` and show it on tmpfs: old red, new green, and the probe's test against both of its mutants.
4. Run the pager test under qemu (`scripts/apt.sh gcc-aarch64-linux-gnu qemu-user-static`, then the `cross-arm` group) with the cfg as at `5038d7e`, red, and as at head, green.
5. Commit each fix alone. Run the whole gate on the head (`git fetch origin main` before `lists`) and push.
