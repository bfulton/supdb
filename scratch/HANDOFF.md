# Handoff: encryption branch, stopped 2026-11-20 04:10 UTC

Branch `claude/inspiring-brahmagupta-d7y8n7`. No pull request exists and none was asked for.
Everything is on the remote and the working tree is clean. Nothing is running. The handoff
before this one (18 Nov) is in history at `a4f3764`; this file replaces it.

`scratch/` is session notes kept in the repo so they survive the container. **Remove it before
any merge or pull request.**

## The story in one paragraph

The branch adds encryption to supdb (`Encryption::Cipher`, `Encryption::Kernel`, the `crypt`
envelope) and, found along the way, a long list of crash-safety fixes to the plain store. It
works and is gated. The owner's problem is that merging upstream hurts, because the cipher's
code is threaded through the engine. The owner's direction (19 Nov): **finish our work here so
that we integrate with upstream once**, scope "everything doable here", order "count-back, then
build the layer". The count-back is done. The page layer (the redesign that isolates
encryption) is designed and planned and **not started**. That is the next piece of work.

## Standing instructions from the owner

- Fix bugs and flaky tests. A performance cost is not "accepted": understand it, do not regress.
- A store with no encryption pays nothing per operation for the feature. `scripts/cost.sh` is
  the check (below).
- A directory fsync that fails: make the names durable another way (guards); if that fails too,
  the store stays open for reads and its writes answer the failure. Implemented (`f364cf4`,
  `9a668be`).
- Push to `claude/inspiring-brahmagupta-d7y8n7` and to no other branch
  (`git push origin HEAD:claude/inspiring-brahmagupta-d7y8n7`, up to 4 tries), whenever there is
  a commit to push. An earlier copy of this note said "only after `sh scripts/check.sh` is green
  on the tree"; the owner corrected that on 20 Nov: "The standing rule should be to change the
  engine default only after benchmarks confirm it's better, why would I care about pushing
  commits?" So pushing waits for nothing, and the gate is for what a commit claims, not for the
  push.
- An engine default changes only after benchmarks confirm it is better (the owner, 20 Nov), and
  after `sh scripts/check.sh` has passed with the new default in place (`CLAUDE.md`).
- No pull request unless asked. No model ids in commits. Commit trailer: the `Co-Authored-By`
  and `Claude-Session` lines the session gives.
- Preferences: be logical, test locally, do not speculate, no shortcuts.

## Where the branch stands

| commit | what | checked how |
|---|---|---|
| tip | this note only | not gated; no code changed since `3f65bfb` |
| `3f65bfb` | count-back fix and its notes in `CLAUDE.md` | full gate green (build test stride lint wasm bench), 20 Nov |
| `6365fdf` | last merge of upstream, at upstream `4358827` | full gate green after `e1de138` |

Upstream (`origin/claude/e-recode-stride-selector-g5f6hl`) has moved since: **6 commits ahead,
head `9bfc436`** (a sparse stride form, off by default; bench key shapes; an ooc row). A trial
merge at the stop: **9 conflict hunks in 4 files** (`bench/src/engines.rs` 3, `src/db/zfile.rs`
4, `src/db.rs` 1, `tests/zpages.rs` 1). Not merged, by the owner's "integrate once".

The history holds "In progress:" snapshot commits from 18 Nov (`7421505`, `ccc2d7b`, `e0badc1`).
Their code has passed the full gate several times since; the main line merges by squash.

## What was done since the 18 Nov handoff

All pushed, each behind a green gate. `git log --first-parent a4f3764..HEAD` is the list.

1. **Upstream merged three times** (`eecfdc0`, `fddc12a`/`6dd58a6`, `6365fdf`): the stride form,
   run sequence numbers, the store lock, the pre-written log. Fallout fixed after each.
2. **The two set-aside durability holes, closed at no measured cost.**
   - run-dir: a run opens on a spare temp name a directory fsync the store already makes has
     covered (`d6734a9`, `Options::run_spare`; arm `supdb-runnospare`). Two directory fsyncs a
     10k durable load against three; throughput level.
   - run-close: covered by the run sequence numbers the upstream merge brought (a sync covers
     the runs that closed loose) plus `21ee2bd`.
   - a failed directory barrier is made by guards, a hard link per name (`f364cf4`); a kept
     failure leaves a `dir-lost` mark the next open acts on (`9a668be`).
3. **Correctness items off the roadmap** (19 Nov evening):
   - `8b19c8e` a panic on the segment work's thread stops the seal queue with the panic as its
     cause; the writer no longer hangs (`Db::panic_segment_work` is the test seam).
   - `29460a2` a manifest slot names the log file it needs (`Manifest::needs_log`, behind the
     format word, old readers ignore it); an open refuses a slot whose log is gone.
   - `9ecc5cf` an unnamed run's piece past the slot with its temp name gone is refused.
   - `09b6a78` the same fall back held under a cipher.
   - `f4e055f` a log whose lost first sector hides frames that verify is set aside
     (`wal-N.aside`), not cut. Plain stores only.
   - `a4cef33` the open orders a delete from before deletes raised the greatest key against the
     run after it (a cov file records the run's first key).
   - `5107a65` under a cipher a form file's failed chunk takes the form out of service, not the
     store (`pager::Paged::regenerable`).
   - `324a395` fuzz seed 1179: a copy's claim to be its block's records names its partition.
4. **Flaky tests.** The wide-values builders test (`e0badc1`), the recycled-log test's detection
   (`840fd25`), several zpages waits moved from tick counts to `tick_until_built` (`1d05892`,
   `e1de138`). The compact-form cipher flake ("the builds gave nothing back") did not recur and
   was **not** diagnosed.
5. **The 10k ordered-load cost of `10898a94`**: explained, no regression against the
   comparators; nothing changed.
6. **Count-back** (`af56aac`, `5ca5a84`, `3f65bfb`), next section.

## The count-back, and how to hold it

`sh scripts/cost.sh BASE [probe ...]` builds six bench probes at BASE (in a worktree, from this
tree's example sources) and at the tree, runs each under callgrind, prints every function's own
instructions side by side, and fails where a function listed in `scripts/cost-hot.txt` gains
more than `COST_SLACK` (0.5) instructions an operation. About 45 minutes. It builds a second
copy of the bench workspace while it runs and removes it at the end, so read `df` first; the
machine need not be quiet, since it counts instructions. Run as:

```bash
COST_DIR=/tmp/claude-0/cost/w sh scripts/cost.sh origin/claude/e-recode-stride-selector-g5f6hl
```

Against upstream head `4358827`, at `3f65bfb` (negative is fewer than upstream):

| probe | per operation |
|---|---|
| read (point read, 200k) | -7.0 |
| scan (1000 scans of 100) | -52.6 |
| load_durable (100k keys) | -7.8 |
| load_buffered | -5.8 |
| work (writer's round, 300k) | -12.4 |
| merge (200k) | -207 |

The one real finding: `forms_off_scan` and `scan_seek_seeded`, upstream's, were inlined in
upstream's build and called out of line in ours (+8.4 a scan). Each keeps its first test inline
now. Two false alarms are written up in `CLAUDE.md` beside the count ("`sh scripts/cost.sh BASE`
is the count made one command").

**Not covered by `cost.sh` yet** (the plan's G1-3 wanted them): reads and scans through the
compact form (`supdb-nocachezpages`), a scan over `SCAN_REST` (1024), a seal probe
(`SegmentWriter::create_with`), and `Blob::scan_at` through the public API. Upstream's 6 new
commits have not been counted against.

## The zpages stress: upstream's tests are fragile under load, not ours

Running the checked `zpages` test binary four copies at once, four threads each (16 tests on 4
cores), three rounds: ours failed 10 runs of 12; upstream's own build (`4358827`) failed 12 of
12, with as many failures or more, in the same tests. So it is not this branch's regression.
The gate (one copy, four threads) passes. The failures are wall-clock waits and controller
states never reached: `scans_after_a_quiet_stretch_have_the_cold_form_files_asked_for` (most
often; its `drive_to` never reaches COMPACT, 3 of 7 form files built, 4 builds started),
`the_trial_judges_the_refilled_state`, the hot-raw policy tests, `the_sample_starts_between_...`,
`the_reading_at_the_crossing_...`, `the_selector_builds_the_raw_tails_...`,
`readers_read_through_a_partitions_conversions_both_ways`,
`handles_on_threads_read_stride_files_while_merges_retire_them` (`builds_collected` waits 30 s).
The pattern that fixed the ones already done: `drive_to` (a count of rounds) to `drive_to_built`
(rounds while a build is in flight, a minute of wall), `tick_until` to `tick_until_built`. A
bulk replacement broke tick-exact tests once; do it test by test. Planned for the integration.

## What is left, in the owner's order

### 1. The page layer (task 12). Stage G1 under way: two units of eight landed.

**As of 20 Nov 12:15 UTC.** The stage's workflow script is `scratch/plan/build/g1-build-v2.js`,
launched one unit a launch: `Workflow {scriptPath, args: {base:
"ea6352181f342b7bffd75ec8c88df92bc7b78ff6", only: ["G1-<n>"], tail: false}}`, the same base for
every unit; after G1-8 a launch with `only: ["none"]` and no `tail: false` runs the full gate
and the critic. A unit is a build, up to three rounds of two reviews and a repair, then its gate
and one local commit, which whoever launched it pushes. A container restart kills the run and
leaves its files: resume with `resumeFromRunId` and the same args, plus `args.resume` (a
build) or `args.notes` (any other step, by its label) to tell the killed step what the tree
holds; a note for a step changes no earlier prompt, so the steps before it replay.

- **G1-1 `wall` landed: `e7d3fe71`.** `tests/source.rs` over a lexer in `tests/lex/`, the
  couplings in `tests/data/couplings.txt`, `.github/CODEOWNERS` naming the owner for that file
  (the owner has not been asked whether they want it: say so). Its notes, mutants and red
  inputs are in `scratch/plan/build/G1-1/`. Its third review's three findings were repaired and
  not reviewed again; the stage's closing critic reads the whole.
- Its gate went red once on a cipher test that bet on the segment work's timing; that test is
  fixed (`ac512887`).
- **G1-2 `door` landed: `fa7e9356`.** The library's file, mapping, advice, sync and
  process-read calls are barred by symbol outside the door's modules (vault, bytes, pager,
  crypt, dirkey, blocks): a clippy run of its own over the library (`scripts/door/run.sh`, in
  the lint group, config `scripts/door/clippy.toml`), its red input (`scripts/door/red.sh`),
  and what clippy cannot see held by text in `tests/source.rs` against `tests/data/door.txt`
  and `tests/data/door-libc.txt`. No compiled engine code changed (lint attributes, one test
  moved out of `vault.rs`). Its notes and mutants are in `scratch/plan/build/G1-2/`. Open, for
  the closing critic and the owner: all three review rounds asked for repairs and the third
  repair was not reviewed again; the unit is 1,845 lines of `tests/source.rs`, and its libc
  class test reads the libc source Cargo.lock resolves out of cargo's registry, so a libc bump
  that adds a string-taking call fails it until the call is barred or listed, and a vendored
  or offline layout fails it outright -- a maintenance cost to weigh against what it holds;
  the config's names were resolved on x86_64 only; `CLAUDE.md` still names the deleted
  `the_engine_names_no_file_but_through_the_door` and says no module carries a blanket lint
  allowance, both G1-8's to correct.
- **Next: G1-3 `cost`**, then `offsets`, `authenticated`, `advice`, `sealed`, `docs`.

What follows was written before the stage began.

Design: `scratch/seam/design.md` (final), decisions in `scratch/seam/decisions.md`, the contract
in `scratch/seam/forms.md`. One module, `src/db/pagefile.rs`, behind the `vault::` door, that
knows pages, lengths, versions, sync points, sealed names and a durability class a file, and no
format. Plaintext page p is one stored 4096-byte page with its tag kept apart, which also fixes
the "form page is two chunks" loss that keeps the controller off under a cipher.

Plan: `scratch/plan/g1.md` (eight units: wall, door, cost, offsets, authenticated, advice,
sealed, docs), then `scratch/plan/g2.md` (ten units, a conformance group), then the layer.
`scratch/plan/build/g1-build.js` is a workflow script for G1, written and never launched.

Read before starting:
- **The plans' line numbers are stale.** They cite export `682b2c09`, from before three upstream
  merges (about 250 commits) and everything in "what was done" above. Every `db.rs:NNNN` has to
  be found again by symbol.
- **G1-1's inputs are in the repo now** (`scratch/plan/wall`, `door`, `borrows`, `conform`,
  `wallcheck`: the scouts' notes and checks, `couplings.txt`, the token lists, `lex.py`,
  `gen.py`), copied on 20 Nov from the scratchpad of the session that made them. They are of
  the tree at `682b2c09`: the counts will differ on today's tree. One file over a megabyte was
  left out (`wall/db.reg`, which `gen.py` writes again). `scratch/plan/build/g1-build-v2.js` is
  the G1 script brought up to today's tree.
- **G1-3 is partly built**: `scripts/cost.sh` and `scripts/cost-hot.txt`. What it lacks is
  listed under the count-back.
- The process each unit inherits is at the top of `g1.md`: red input first, the change, mutants
  against its tests, two reviews, gate, commit; nothing switched off under a cipher to make a
  unit easier; no instruction added to a plain store's operations.
- A workflow (many subagents) needs the owner's say-so in the new session.
- Whether to merge upstream's 6 commits before starting is the owner's call: "integrate once"
  says no, but the layer will be built against a `zfile.rs` upstream has since changed by 1,700
  lines. Raise it.

### 2. Performance items (task 13). Not started. `ROADMAP.md` has each.

- The controller under a cipher: its model held to a measurement, and pricing the store's own
  cache as well as the kernel's. Likely folds into the layer (whole-page chunks).
- An open after a crash, priced by its parts (+160-180 ms at 3M keys where a merge's output is
  unnamed).
- A log file retired off the drain (an `unlink` of 14 ms on a cipher's ordered drain).
- Also open under "Engine" in `ROADMAP.md`: an envelope written in one pass / beside its
  segment, an ordered run under a cipher as its own log, a miss at less than a fault, the read
  under a cipher at the plain store's rate, a read of one key larger than the cache.

### 3. Correctness items still open in `ROADMAP.md`

- A fall back held by a segment's last key: one way left, a merge output that folded in a run's
  piece (media damage alone; wants the segment to say it is a merge's output).
- What a guard does not cover: the segment work's directory fsyncs stop the seal queue instead
  of guarding.
- The items "held on a device that makes them" (log errors, power cuts, a torn slot, a seal's
  failed sync): need a device that really fails, which this container has not got.
- From the 18 Nov note, not re-checked: the last repairs of five pricing units landed with no
  review after them (followups 82, 102, 116, 123, 132); the tests-tidy unit's two reviews were
  never run; the engine-tidy wording items (followups 113, 115, 124, 127, 129); a test that
  holds about a thousand descriptors and two that use `kcmp`, against a CI runner's limits.
  `scratch/stage0/fix/followups.md` numbers every item to 134 and has not been updated since
  18 Nov, so check an item against the tree before working it.

### 4. Integrate once (task 14)

Merge upstream's head (9 hunks at the stop), fix the fragile zpages tests above, run
`scripts/cost.sh` against the new upstream head, run the gate, then a stress of the `zpages`,
`durable` and `cipher` binaries, then the quick row (`sh scripts/check.sh quick`, a timing run,
machine to itself; after a default changes, open the whole roster at the smallest rung first).
Remove `scratch/`. Then the owner decides about a pull request.

## Working here

- The gate is `sh scripts/check.sh` with its own `TMPDIR` (`TMPDIR=/tmp/claude-0/tg`), about 20
  minutes with the build caches warm and longer from cold; run it in the background and wait on
  a marker line in its log. Do not edit the tree
  while it runs. Do not run a stress beside it.
- Clean test stores by literal path (`rm -rf /tmp/claude-0/tg/supdb-*`); a glob through a
  variable was blocked by a safety check.
- Disk: 11 GB free at the stop. `bench/target` is 12 GB and `target` 4.7 GB.
- The container restarted once on 20 Nov mid-stress and three times on 18 Nov. Background jobs
  die with it; commits survive. Commit before long runs.
- All of `db.rs`'s file operations go through `vault::`; a unit test reads the source for it.
  `-D warnings` everywhere.
- Scripted edits (python `assert s.count(old)==1`) failed without writing several times when the
  assert sat after an early return; read the result back.
- A stress of `zpages` four at a time is 5-9 minutes a round; of `durable`, about 4.4 minutes a
  run.
- Leftovers that can go: worktrees under the session scratchpad (`mg`, `rc`, `rd`, branches
  `rc-work`, `rd-work` with one local WIP commit `b2e4143`, a run-close shape that was
  superseded), and `/tmp/claude-0/*.fix`, `*.bak`, `z*.out`.

## Where things are

| path | what |
|---|---|
| `CLAUDE.md` | the rules and every lesson, by failure; the count-back is under "A live store under the caller's cipher" |
| `ROADMAP.md` | open and closed items; "Engine" holds the cipher and durability ones |
| `scripts/cost.sh`, `scripts/cost-hot.txt` | the plain store's cost by function |
| `scratch/seam/` | the redesign: `design.md`, `decisions.md`, `forms.md`, maps of the read and write paths |
| `scratch/plan/` | `g1.md`, `g2.md`, the G1 script, the 17-18 Nov session log |
| `scratch/stage0/` | the durability fixes: per-fix notes, the pricing run, `fix/followups.md` |

Session task list at the stop: 12 the page layer (pending), 13 the performance items (pending),
14 integrate once (pending); 1-11 done.
