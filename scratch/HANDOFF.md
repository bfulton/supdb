# Handoff: encryption branch, stopped 2026-11-18 16:50 UTC

Branch `claude/inspiring-brahmagupta-d7y8n7`. No pull request exists and none was asked for.
Everything below is on the remote; the working tree was clean at the checkpoint. Nothing is
running: the workflow was stopped, its background processes killed, and the scheduled check-in
(`trig_01N8Ncu2jhnmyZxE47jUuSm8`) disabled.

`scratch/` is this session's notes, kept in the repo so they survive the container. Remove it
before any merge or pull request.

## Where the branch stands

| commit | what | checked how |
|---|---|---|
| tip (the checkpoint commit) | notes and this file only | n/a |
| `ccc2d7b6`, `74215056` | "In progress" snapshots of the ninth unit (crash-test tidy): **unreviewed** | builds (`cargo test --profile checked --no-run`); the unit's own runs, below; no gate run by anyone on this exact tree |
| `1d77c353` | last commit that is reviewed and passed `sh scripts/check.sh build test lint` | unit gate green |
| `8b83dc03` | last commit the **full** gate (build test lint wasm bench) was green on | full gate green |

Commits between `8b83dc03` and `1d77c353` each passed build, test and lint; wasm, bench and the
quick row have not been run since `8b83dc03`. Subjects starting "In progress:" are snapshot
commits or the take-back of one; they were pushed to satisfy a stop hook and carry no review.

To drop the unreviewed unit: `git revert 74215056 ccc2d7b6` (or branch from `1d77c353`). To keep
it: finish its steps (below) and re-commit it under a real message.

## What exists

1. **Encryption (14-15 Nov), built and gated.** `supdb::crypt` (an envelope for a finished
   file, read natively and in the browser), `Encryption::Kernel` (fscrypt), `Encryption::Cipher`
   (a live store under a caller's cipher, larger than memory, with ordered ingest, the recycled
   log and the compact form). One option, three values. A store in the clear was counted at no
   extra instructions an operation against main on the paths counted. `CLAUDE.md` has the detail.
2. **Durability of the plain store (16-18 Nov).** Found while reviewing crash behaviour for the
   redesign; these predate the branch. Eight fixes ("stage 0", `f8d130a8`..`8b83dc03`), then the
   pricing run below. Every fix has a test that fails on the code before it.
3. **The redesign the owner asked for is designed and not built**: encryption at a layer under
   the store's forms (after compression, before the disk), so a form change needs no encryption
   change. Decisions: `scratch/seam/decisions.md` (17-22 are the owed items, concluded). First
   two stages planned in `scratch/plan/g1.md`, `g2.md`; the script for the first is
   `scratch/plan/build/g1-build.js`, written and never launched.

## The pricing run (stopped at unit 9 of 11)

Script `scratch/stage0/price/price-durability.js`; every agent's result is in
`scratch/stage0/price/journal.jsonl`; per-unit notes, mutants and patches under
`scratch/stage0/price/<unit>/`. A unit = fix, up to three review rounds (crash lens, cost lens)
each with a repair, then a commit behind `build test lint`. A fix that adds a wait, a syscall
or a write where nothing fails is paired in one process against the shape before it and lands
only if no pairing resolves a cost.

| unit | result | commit | cost when nothing fails |
|---|---|---|---|
| other-slot: the open fdatasyncs both manifest slots | landed | `2b092211` | one more fdatasync an open (the two slot syncs together are about 0.12 ms of a 0.5 ms open at 10k keys; nothing measurable at 3M) |
| rename-arm: the rename shape of the manifest survives every crash state built for the slots | landed | `639cb0de` | none on the default shape |
| run-close: a closed run's commits covered by the next sync | **set aside** | `run-close/not-landed.patch` | closing `Db::sync` +0.95 / +1.12 ms at 300k (`supdb-ingestshape`); the alternative built read the ordered load 0.72-0.74x |
| due-log: a run's due commit syncs the log first where it owed a sync | landed | `e0f0ccc0` | none resolved |
| run-dir: a run's temp name made durable before its first ack | **set aside** | `run-dir/not-landed.patch` | first commit +0.16-0.29 ms at 300k durable (lazy shape); eager shape more |
| helper-race: the writer's helper thread syncs through its own file description | landed | `8d9b5c4f` | one `openat` for a `dup` per segment writer with a helper; verdict "unresolved": one stamped phase cell longer in one pairing of three, argued unreachable (followups 106, 117) |
| drain-order: prices an order stage 0 landed; folds two test seams | landed | `23b2ab34` | level; the old branch fired in 1 stamped drain of 208, so only that regime is priced (followups 118, 122) |
| open-cost: prices and trims what the open-time fixes cost | landed | `1d77c353` | open 0.98x of before at 10k, level above; **not reduced**: an open after a crash that left a merge's output unnamed is 1.5-1.6x the unchecked open (+160-180 ms at 3M keys), a ROADMAP item |
| tests-tidy | **in progress**, on the tip as snapshots | - | none (tests and test hooks) |
| engine-tidy, count-back | not started | - | - |
| full gate, callgrind count, critic | not run | - | - |

### The unit in progress (tests-tidy)

Done, per `scratch/stage0/price/tests-tidy/notes.md` and `mutants.md`: one helper that copies a
live store with the segment work paused (`Db::pause_segment_work`), all seventy live copy sites
moved to it; the file-call trace kept per directory; the slot's sync held without root; the
manifest-shape crossing killed at each step; the differential fuzz varying the sync policy
(`ZFUZZ_SYNCVARY`); twelve mutants, all killed once two tests were strengthened, and three of
stage 0's re-run under the pause; clippy and fmt clean; a whole-suite run with two reds read (one a timer bet it restated, one a memory test
under the stress's pressure, green alone).

Not done: its stress was 37 green runs of 160 when stopped (one more was an OOM kill, not a
failure; `stress-fixed.log`); the whole `db` and `lib` binaries on a quiet machine; the durable
binary under `SUPDB_TEST_UNPRIVILEGED=1`; both reviews; the unit gate; its commit message.

### Not started

- **engine-tidy**: small engine and docs items no pricing is owed for; the brief in the script
  lists them by followup number. Wording items found since: followups 113, 115, 124, 127, 129.
- **count-back**: bring each per-key and per-commit path back to the instruction count of the
  commit before stage 0. The count that found the regressions is `scratch/stage0/fix/count/`
  (`reading.md` first): `Db::commit_direct` +4 instructions a key, `pwritev_all` +38 a write, a
  slot write +53, and others. This is what holds "no cost to unencrypted operations"; until it
  is done that requirement is **not met** on the write paths named there.

## Known problems

1. **Two cipher tests fail intermittently, on the commits before this run too** (task 14,
   followups 119, 128, 131). `tests/cipher.rs`:
   `the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit` (1 run in 40) and
   `the_builders_rest_at_every_block_of_a_store_of_wide_values` (4-5 in 40; cache peak 6-8 MB on
   a 2 MB budget in a single process). About one run of that file in eight is red, so any gate
   can go red on them. The second may be the pager and not the test. Undiagnosed. The runs
   that found the rates: `scratch/stage0/price/open-cost/r10-flake-*.log` and `r10-flake.sh`.
2. **A cost already landed**: stage 0's sync fix (`10898a94`) made the 10k-key ordered load read
   0.64-0.68x on the buffered arms that end in `Db::sync` (the old code skipped a sync it owed);
   level at 300k. The quick gate may flag that cell (followup 90).
3. **Windows left open, documented in `CLAUDE.md` and `ROADMAP.md`**: no order on the device
   between a run's file and the log before the run's first due commit (four ways the bytes get
   there first; followup 97); a damaged slot whose log retired; a failed sync across a reopen
   (left open by decision 18); no test over a device that really fails writeback.
4. **Reviews not closed**: the last repair of rename-arm, due-log, helper-race, drain-order and
   open-cost landed with no review after it (followups 82, 102, 116, 123, 132). The critic was
   to read them.
5. **Host-dependent tests**: one holds about a thousand descriptors (fails under a 1024 limit,
   followup 112); two use `kcmp` (seccomp, followup 109). The CI runner's limits are unread.
6. A control never run: an arm paired against its own twin, to say whether a measuring field
   alone moves the drain's "readable" stamp (followup 118).

## Decisions waiting for the owner

1. Finish the pricing run's last units, or go straight to the redesign. My recommendation was to
   finish (count-back holds the no-cost requirement; the redesign builds on these crash paths).
2. The two set-aside fixes. Cheaper shapes are sketched and unbuilt: run-close with the closed
   file's fdatasync on the helper thread, joined by the next sync (followup 89); run-dir with
   the run opened on a temp name a directory fsync the store already makes has covered
   (followup 103). If neither prices clean, I would land the cheapest correct one.
3. Whether the 10k cost in "known problems" 2 is accepted.

## Suggested order from here

1. Task 14, the two flaky cipher tests, before anything that gates.
2. Finish tests-tidy (reviews, unit gate, commit), then engine-tidy and count-back, then the
   full gate, the callgrind count and a critic pass over the whole run.
3. The cheaper shapes of run-close and run-dir, priced.
4. `scratch/plan/build/g1-build.js`, then the second stage, then the page layer in stages.

## How to pick the run back up

- A workflow run cannot be resumed by id from a new session. Relaunch the script with
  `args {base: "<sha the units build on>", only: ["engine-tidy", "count-back"]}`; `only` filters
  the unit list and the full gate, count and critic follow. For tests-tidy, add a paragraph to
  its brief saying the work is on the branch and what is left, or finish it by hand.
- The script's briefs point at the session scratchpad (`S`, `P` at its top); repoint them at
  `scratch/` in the repo. Syntax check: `node scratch/stage0/price/stubcheck.mjs <script>`.
- `scratch/stage0/fix/land.sh` and `snapk.sh` are the landing and snapshot helpers; they assume
  the scratchpad's paths too. They never force-push.
- One timing run at a time; no cargo beside a pairing. Each run its own `TMPDIR`, cleaned by
  name: a whole durable binary leaves hundreds of stores. Disk is a fixed allowance (7.9 GB
  free at the stop); stale binaries under `bench/target/release/examples` are the first to go.
- The container restarted three times on 18 Nov; a restart kills a run. Keep a checked copy of
  each file a mutant runner edits, with sums, so a tree left holding a mutant can be put back.
- A stress of the durable binary is about 4.4 minutes a run with four at once: size it in hours.

## Where things are

| path | what |
|---|---|
| `scratch/stage0/fix/followups.md` | every open item, numbered to 134 |
| `scratch/plan/orchestrator-notes.md` | the session's log, by time |
| `scratch/stage0/price/` | the pricing run: script, journal, per-unit notes, mutants, measuring and not-landed patches |
| `scratch/stage0/fix/` | stage 0: per-fix notes, the instruction count (`count/`), `deferred.md` |
| `scratch/stage0/report.md`, `fixes.md` | stage 0's report and its critic's concerns |
| `scratch/seam/` | the redesign: `design.md`, `decisions.md`, maps of the read and write paths |
| `scratch/plan/` | the first two build stages and the first stage's script |

Session task list at the stop: 8 pricing run (in progress), 9 first two build stages (script
written), 10 the page layer in stages, 11 durability items with no unit, 12 and 13 the cheaper
shapes, 14 the flaky cipher tests, 5 the original security recommendations still on the roadmap.
