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
- **Stay off upstream (the owner, 22 Nov, in full: "On main vs upstream. Stay wholly off of
  upstream.").** It answered a report that had asked what the lists' and the cost job's base
  should be while upstream's work is not on the main line. Read here as: from that message on,
  nothing is done with `origin/claude/e-recode-stride-selector-g5f6hl`. It is not fetched,
  merged, pushed to, counted against or named as any gate's or CI job's base; a base is the main
  line (what `scripts/base.sh` takes from the event) or the commit a change began at. This
  replaces "Merging upstream is fine" (20 Nov) and "May want to merge upstream changes again"
  (21 Nov) for everything after it. What the branch already holds of upstream (five merges, the
  last a row at `7c712c1f`, merged as `60921870` an hour before the message) is left as it is:
  taking it out was not asked and would be a rewrite of the branch. The reading is the
  launcher's, said back to the owner the same hour; if the owner meant less (the bases alone)
  or more, this bullet is what changes.

## Where the branch stands

| commit | what | checked how |
|---|---|---|
| tip | this note only | not gated; no code changed since `3f65bfb` |
| `3f65bfb` | count-back fix and its notes in `CLAUDE.md` | full gate green (build test stride lint wasm bench), 20 Nov |
| `6365fdf` | last merge of upstream, at upstream `4358827` | full gate green after `e1de138` |

Upstream (`origin/claude/e-recode-stride-selector-g5f6hl`) was merged again on 21 Nov at
`7c62153e` (`22dcd115`; section 1 below has what the merge held and left), and its rows twice
after, the last at `7c712c1f` (`60921870`, 22 Nov). **That was the last: the owner's instruction
of 22 Nov (standing instructions, above) is to stay wholly off upstream**, so it is no longer
fetched or merged, and how far behind it the branch is, is not this work's question.

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

### 1. The page layer (task 12). Stage G1 is closed: eight units, the full gate green on `31b33339`, the critic's account in `scratch/plan/build/close/g1-built.md`. Five changes stand between it and G2, in the order below.

**As of 22 Nov 11:24 UTC.** The stage's workflow script is `scratch/plan/build/g1-build-v2.js`,
launched one unit a launch: `Workflow {scriptPath, args: {base:
"ea6352181f342b7bffd75ec8c88df92bc7b78ff6", only: ["G1-<n>"], tail: false, since: <note>}}`,
the same base for every unit; after G1-8 a launch with `only: ["none"]` and no `tail: false`
runs the full gate and the critic. `since` (from G1-4 on) is the launcher's note of what landed
after the plan was written, read by every step of the launch; G1-4's is in
`scratch/plan/build/G1-4/launch-args.json` and the later units take the same text with what
has landed added. From G1-4 on a unit's gate also runs the `lists` group, and `wasm` for a
unit that changes compiled engine code. A unit is a build, up to three rounds of two reviews and a repair, then its gate
and one local commit, which whoever launched it pushes. A container restart kills the run and
leaves its files: resume with `resumeFromRunId` and the same args, plus `args.resume` (a
build) or `args.notes` (any other step, by its label) to tell the killed step what the tree
holds; a note for a step changes no earlier prompt, so the steps before it replay.

- **G1-1 `wall` landed: `e7d3fe71`.** `tests/source.rs` over a lexer in `tests/lex/`, the
  couplings in `tests/data/couplings.txt`, `.github/CODEOWNERS` naming the owner for that file
  (the owner was asked on 20 Nov and decided against it: see "The owner's decision" below). Its notes, mutants and red
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
- **G1-3 `cost` landed: `c1546dc1`.** `sh scripts/check.sh cost` (outside the default set, and
  a CI job) runs `scripts/cost-red.sh`, the gate's red input, and then `scripts/cost.sh BASE`:
  fifteen probes built at BASE from this tree's example sources and at the tree, each under
  callgrind, every function's own instructions compared by canonical name
  (`scripts/cost-diff.py`, the list and the bounds in `scripts/cost-hot.txt`), and
  `tests/source.rs` holds every public entry point of `Reader`, `Db` and `Blob` to a probe or
  a reason in `tests/data/cost-entries.txt`. No engine code changed. A container restart at
  12:30 UTC killed its first build sixteen minutes in; the run was resumed with a note. Its
  notes, its mutants (reviewers' and its own, four passes) and the two whole runs it rests on
  are in `scratch/plan/build/G1-3/` (`against-upstream-9bfc4362.log`, `against-itself.log`).
  All three review rounds asked for repairs; the third repair (the `~` lines given bounds, the
  `land`, commit and long compact-form scan probes, type aliases and blanket trait impls in the
  entry-point test) was read by the launcher and by no reviewer: the closing critic's.
- **What the cost gate says of this branch against upstream `9bfc4362`** (the upstream head
  last merged), since task 17 landed as change `T17`, **`c584cdc3`**: every one of the fifteen
  probes is at or under upstream by its total, and the gate still exits red by function, on
  moves. Before the change two probes were over: `scan_at`, the public `Blob::scan_at` with a
  caller's own callback, +239 instructions a scan of a hundred entries, and `land`, +274 a key
  rewritten. What was found and done, each by function and by instruction
  (`scratch/plan/build/T17/notes.md`):
  - `scan_at`, now 373 a scan under: the public walk moves its rank once where its loop ends,
    `Blob::seek`'s arm by the piece is out of line, and `FlatIndex::search` resolves its two
    regions once before its loop (the engine's own scans read about 300 a scan lower for that
    too).
  - `land`, now 55 to 62 a key under: the cause was `324a395a`'s search by key in
    `Reader::splice_run` wherever a copy's claim names another partition (the reading of G1-3's
    one run, verified). The fix stays; the patch takes the entry its rank names only where a
    key compare (`key_eq`) proves that entry is the key in hand, and searches otherwise, so the
    shape is sound by a check it makes. `tests/settle.rs` holds it over keys of 4, 16, 17 and
    19 bytes and a whole block, red with the compare removed, with `324a395a` taken out, and
    with the search dropped.
  - `Reader::settle_each`, whose source is upstream's, read 37 a key over: the ordered index's
    cold arms that return (`head_absent`, `top_late`) were inlined into it and kept the query's
    head on the stack across the calls. Both arms are out of line (`plan_seek_late`,
    `seek_planned_by_piece`) and the stride's probes read unchecked over the slice the plan
    cuts; it reads 9 a key under, with 8 of codegen growth still inside it, outweighed and not
    removed. `CLAUDE.md` has the rule (a cold path that returns into a hot path is priced in
    the caller).
  - The whole run and the pairs: `scratch/plan/build/T17/r1/cost-whole-R1e.log` and
    `pairs-whole-R1e.txt` (probe, gained, lost, net). Every gainer in the other thirteen probes
    is paired with its loser but two, neither this change's and both small: `read_z`'s
    `zpages::view_with` +3 a read, and `merge`'s small gainers inside that probe's slack. The
    pairs for `scan_z`, `scan_long_z` and `merge` were read off function totals, not by
    instruction.
  - Under a cipher, by the cipher's twins of the probes: the scan by the piece 199 a scan
    lower, the patch into a carried copy 231 a key lower in all, and inside that the settle's
    planned seek 67 a key higher, since an index read by the piece takes the two arms out of
    line now. Two shapes that took the 67 back each cost the plain store and were refused; it
    is the page layer's to take back (its index is not read by the piece through a cold arm).
    The cipher twin of `land` was a scratch copy of the probe
    (`scratch/plan/build/T17/r1/land_cipher_scratch.rs`), not a probe of the gate.
  - Two review rounds; the second passed in both lenses with items that a second repair then
    worked (ten more mutants, a test of the walk by the piece across several windows, a
    checked-build assertion that a plan is in order). That repair's diff was read by the
    launcher and by no reviewer: one `debug_assert!` and two tests.
  - **Owed, none taken:** timing. `sh scripts/check.sh quick` at the tree for the scan pass
    and the lag sweep's bursts, and `bench/scripts/pair.sh supdb-cipher supdb 300000 --passes
    lag` beside the same pairing at `c7a0c933` for the planned seek under a cipher. The hints
    ahead of a patch and a walk take any claim, as upstream's did, so a carried copy is
    prefetched as upstream prefetched it.
  - **Open:** the scan's start over a carried copy (`blk.identity == seg.blob.id()`,
    `324a395a`'s line) is held by no test: a mutant that takes any claim there survives the
    tests and 2,950 fuzz seeds, which reach a stale claim at a scan's start 1,808 times and
    every time at the block's first entry, where the rank and the search agree. `key_eq`'s
    arm for keys under sixteen bytes compares two empty slices by a call of no bytes, which
    this file's own rule says to guard (the empty key in a carried copy, one key a store).
    The new tests in `tests/blob.rs` hand closures straight to `Blob::scan_at`, where
    `tests/common`'s `kv` would compile one copy.
- **What the cost gate cannot do yet, for the closing critic and the integration:** a move
  between two functions reads red with nothing to say it is a move, so a pull request against
  the main line fails its cost job as the branch stands, with every total under upstream; the
  way through is undecided, and `T17`'s paired table is its input (a declared group of
  functions held to its net, valid against one base, is the candidate, shaped like the
  lists' declared raises below). Also open: the `~`
  bounds rest on two to four self-checks a probe; `land` runs the segment work inline, not
  on its own thread; the CI job and valgrind's noise on a hosted runner are unmeasured;
  `sh scripts/check.sh cost` with its default base has not been run; a whole run wants
  several gigabytes free; libc's and the allocator's functions are held only loosely; `Txn`,
  `RunWalk` and the `SparseBlob` readers are outside the entry-point list.
- **The owner's decision (20 Nov 17:38 UTC), task 18, landed: `4ceda21e`.** A list's "only goes
  down" is held by a check, not by `.github/CODEOWNERS` (removed), and every workflow that
  runs on a change checks only through `sh scripts/check.sh <group>`. `scripts/base.sh` is the
  one definition of the commit a change is made on (an override, a pull request's base, a
  push's before, else the merge-base with `origin/main`; an unresolvable base fails, never a
  fallback), and the cost group takes its base from it. The `lists` group (in the default
  set) runs `scripts/ratchet-red.sh` and then `scripts/ratchet.py` through `ratchet.sh`: each
  registered list (`couplings.txt`, `door.txt`, `door-libc.txt`, `cost-entries.txt`'s reasons
  in place of a probe) at the merge-base against the tree, growth read by the list's own key
  and number, the registry itself held to the base's; an intended raise is a line of
  `scripts/raises.txt` (list, key, from, to, reason) matched in all four, and a line the base
  already has is spent. `cross-arm` and `machine` (the Apple Silicon capture, which
  `apple-silicon.yml` checked by hand) are groups outside the default set. `tests/source.rs`
  reads every file under `.github/workflows`: a workflow on a change's event is read key by
  key (an `if`, a `continue-on-error`, an `env` that sets a base or narrows a group, a check
  outside `check.sh` are red; a group no job runs is red), the measurement workflows are
  listed with why and held to their triggers, and any other file is red. Run as change `T18`
  with `scratch/plan/build/change-v1.js` (the stage's script with the unit taken from its
  args); its notes, mutants and launch arguments are in `scratch/plan/build/T18/`. All three
  review rounds asked for repairs; the third (every workflow read, the `machine` group, the
  set-up scripts' writes to the job's environment) was read by the launcher and by no
  reviewer. Open: the `machine` group has not run on a Mac; what a set-up's binary or an
  action writes to a job's environment is not read; the cost gate's declared moves are not
  built (`raises.txt` is shaped to take them); the launcher's choice that a raise is declared
  rather than impossible has not been answered by the owner.
- **`19d078d1`: the pager's residency test compiled out under `supdb_emulated`.** The change
  above installed the cross toolchain and qemu-user here and ran `cross-arm` for the first
  time on this branch: one test red, five runs of five, the pager's ask ahead that qemu-user
  passes to no kernel. Green after (519 passed). No hosted runner has run any job of this
  branch: the first pull request is where `cross-arm`, `machine`, `cost` and `lists` first run
  there.
- **G1-4 `offsets` landed: `97412a31`.** `Blob::willneed_ranks`, `willneed_dir` and
  `record_offset` answer for a reader by the piece as for a whole one (they answered 0 or
  `None`), and `record_region_place` for both kinds. Each dispatches on `Blob::flat_lent` (a
  borrow through `Sec::lend`, the `lend_at` test with no error made) to a
  `#[cold] #[inline(never)]` `*_by_piece` arm over the same `Sect`-generic helper; the offset is
  `Sect::start_in`. `tests/blob.rs` holds the two kinds of reader to each other and to the
  file through a source that records every ask (`Advised`), and holds the piece reader to
  asking for under a quarter of the record region; `tests/fences.rs` holds where a merge's
  drops behind it begin (the base's mechanism, held by no test before). The plain store's
  count: `sh scripts/cost.sh 869a4fee read read_z scan scan_long scan_z scan_long_z merge
  land` green; `record_offset` 51 instructions a call where the base had 55, and kept
  `#[inline(never)]` because inlined into `compact_run` the merge row read RED (+4.0 an
  operation; `r2/red9.sh` is that red input by script). Two review rounds: the first asked
  for a repair (a callee-saved register kept across `not_lent`'s call cost the slice arm two
  instructions a call), the second passed with no blocking item, and its repair added tests
  only, read by the launcher and by no reviewer. Notes, mutants, the scratch probes
  (`g14_count.rs`, `g14_drop.rs`, `g14_gate.rs`), the listings and the count logs are in
  `scratch/plan/build/G1-4/`. Open, each owed to a named place:
  - No `cost.sh` probe reaches the arms that call these a key or a block at a time: one with
    the gate set away from the raw form (`Db::zgate_set`, a toggle on the main thread's scans)
    and one merging under `merge_drop_behind` and `merge_read_ahead` are owed to the cost job.
    Until then three cost-only mutants (the attributes off the piece arms, `flat_lent` made
    through `flat`) are held by the unit's scratch counts and by no gate.
  - What the pager does with the advice it is now given (`Paging::willneed`, `forget`) was
    not run: no store under a cipher with `ZPages::Always` had its asks counted. S5 or G1-6.
  - `compact_run` takes `record_offset(0)` of every input of every merge, dropping or not
    (`src/db.rs`, where `dropped_to` is made): under a cipher that is now a directory word
    read through the pager an input and then ignored. Take it only where the input drops.
  - `Sect::start_in` is a new required method of a public trait (no implementor outside the
    crate's own two); `record_region_place` answers the file's offset for a whole reader
    now, which G1-6, where `record_region_span` goes, has to know.
  - The attributes off `willneed_dir_by_piece` alone read about 12,000 instructions fewer
    over the gate probe's run: a cheaper shape for whoever counts those rows next.
  - Owed to S5, no timing taken: `bench/scripts/pair.sh supdb-cipherauto supdb-cipher 5000000
    --bound 671088640 --passes read` at this commit and the one before.
- **G1-5 `authenticated` landed: `ce095eb9`.** `Bytes::authenticated` (default false) says
  whether a source has checked every byte it lends. `Blob::open_with` skipped the key
  section's checksum row for any section held by the piece; it now sends such a section to
  `verify_row_by_piece` (`#[cold] #[inline(never)]`), which skips the row only over an
  authenticated source and otherwise verifies it through `flatindex::verify_pieces_in`, the
  row asked for once and each piece on its own. `MmapBytes` answers true for a paged mapping
  and false for a mapped file, so a store under a cipher opens as it did (the open bound in
  `tests/cipher.rs` is what fails when a paged mapping answers false) and the change is for
  an outside source that lends by the piece. `tests/blob.rs` flips the last byte of every
  piece, the short first one and the one ending at the row among them, and forges a piece
  whose CRC moves in its top byte alone (a one-byte flip cannot tell a three-byte compare
  from a four-byte one). The plain store's count: `sh scripts/cost.sh ea0962c2 load_durable
  load_buffered commit_durable commit_buffered seal scan_at merge land` green;
  `Blob::open_with` reads three instructions more a segment opened in the two loads (265 to
  268) and the same in the merge and the landing, nothing a key. Three review rounds; the
  first repair was cut short (its agent ended its turn to wait for a background run, and the
  harness took its answer as it stood), the second finished what it owed (every mutant, the
  full test run, lint, wasm, the bench group, the recount), and the third added the per-piece
  flips, read by the launcher and by no reviewer. Notes, mutants, the mutant scripts, results
  and count logs are in `scratch/plan/build/G1-5/`. Open, each owed to a named place:
  - The three instructions a segment opened: a reviewer's shape that may cost nothing (the
    old `if .. && key.pieces(&src).is_none() { whole arm }` kept, with an `else if let
    Some(p)` to the cold function) was not counted. G1-7 counts row 6 and edits the same
    dispatch.
  - `MmapBytes::authenticated` is `self.map().is_none()`, spelled so that `bytes.rs` gains no
    wall token; it is exact while `Own` has two variants and `map()` matches both with no
    wildcard. A third variant that answers `None` there would answer authenticated: say so
    at `map()` and at `Own` (G1-7, which edits both).
  - Docs that do not yet say what the code does: `BlobOptions::verify_index` (the row is
    skipped over an authenticated piece source and verified a piece an ask over any other),
    `BlobOptions::read_ahead` (the walk by the piece ignores it), `Paging`'s contract (an
    implementor lends only bytes it has checked; `InMemory` in `src/ordindex.rs`'s tests
    checks nothing and answers authenticated through `MmapBytes::paged`),
    `Bytes::authenticated`'s "a reader" (`OrdIndex::open` skips its CRC on `piecewise`
    alone, which is right only because its sources come from `vault::map`), and
    `verify_row_by_piece`'s "whole" (it means every byte). G1-8.
  - `flatindex::verify_pieces_in` is a new public function, and it trusts `Sect::range` to
    return exactly the length asked for. Not verified: that no outside crate implements the
    doc-hidden public `Paging` (G1-7 makes it `pub(crate)`).
- **For every launch from G1-6 on:** an agent never ends its turn to wait for a background
  command; it waits in a foreground command with a long timeout. The launch note says so.
- **G1-6 `advice` landed: `eab987d1`.** A store's residency and advice are asked of its byte
  source by offset. One span question, `Bytes::resident_share(off, len) -> Option<f64>`,
  answered for a mapped file (by `mincore`) and a paged mapping (by the pager) alike, in place
  of `MmapBytes::resident_pages`, `paged_resident` and `resident_count`, `db::resident_share`
  over a borrowed slice, `OrdIndex::mapped` and `Blob::record_region_span`, all gone with
  every caller moved. `Blob::record_region_place` and `record_dir_place` give a region's
  place in the file for both kinds of reader; the plain build's asks and drops
  (`ask_ahead`, `drop_behind`, the block form's and `write_stride`'s) go by offset through
  the mapping, Linux-only as they were; the three descriptor sites go through
  `Seg::records_by_descriptor`; `warm_file` measures `map.len()` and asks through
  `advise_willneed`. Sites the plan did not name were found by the unit's own list and
  handled (`ZFile::pages_resident`, `Seg::redrop_ord_pages`, `read_back_residency`,
  `heads_all_resident`, `raw_read_span`). Under a cipher, behind `ZPages::Always` or
  `merge_read_ahead` and on no default's path: the warm asks for the envelope and decrypts
  nothing, the form file's and the ordered index's residency are asked of the pager where
  they had decrypted the whole file to `mincore` it, a merge asks ahead for its inputs'
  ciphertext, and a paged reader's records are dropped. The plain store: `sh scripts/cost.sh
  f9e83900 read read_z scan scan_long scan_z scan_long_z merge land` green, and the plain
  build's `madvise`/`fadvise`/`mincore` call shapes held to the base's by strace, made a gate
  with an exit status over the one test whose shapes are stable
  (`scratch/plan/build/G1-6/strace/gate.sh`). Two review rounds: the first found the plain
  build issuing `madvise(WILLNEED)` off Linux where the base issued nothing (the Linux gate
  put back) and the cipher warm's ask held by no assertion (a test reads the envelope's
  residency after the warm); the second passed, and its repair added tests only, read by the
  launcher and by no reviewer. Notes, mutants, scripts, results, the strace scripts and count
  logs are in `scratch/plan/build/G1-6/`. Open, each owed to a named place:
  - The pager keeps no count of the advice it is given, so what the crossing and the build
    ask of it under a cipher is held by outcomes only. S5.
  - `warm_file` touches pages through `Bytes::lend_at`, and a new test
    (`a_drop_behind_unmaps_the_window_and_no_page_past_it`) reads the mapping's address
    through `MmapBytes::as_slice()`: G1-7 seals the first and deletes the second, and has to
    give each another way.
  - The item allow on `ask_file` (kept for its `posix_fadvise`) covers any barred call added
    to the same function: a mutant that opens by `File::open` there survives. Move the
    advice into a small allowed helper, as `db.rs` does, so the function carries no allow
    (S1, or the door's next edit).
  - The stride form's plain build is not under the strace (no test builds one there); the
    strace's merge test read four new shapes in one head run of three, put down to which
    partition a background build took and not confirmed by more base runs.
  - The merge row's libc `memcpy` read +3.6 and then +12.9 an operation over the base in two
    runs of the same code, green under the libc rule and above the base both times: count
    the tree against itself twice before it is called noise.
  - Not built for macOS or aarch64; `cross-arm` not run. `tests/blob.rs`'s
    `holds_to_the_file` now compares `record_region_place` of two readers where one side was
    taken by pointer subtraction; the independent check left is `record_offset(0)`.
  - Owed to S5, no timing taken: `bench/scripts/pair.sh supdb-cipher64 supdb-cipher 1000000
    --reps 4` with an arm that sets `merge_read_ahead`, and `pair.sh supdb-cipherauto
    supdb-cipher 5000000 --bound 671088640 --passes read`.
- **Upstream merged again: `22dcd115` (a change called M1), with `2a0d750a` after it.** The
  owner asked (21 Nov: "May want to merge upstream changes again."); it went in before G1-7,
  which edits the files upstream is editing. Merged: `origin/claude/e-recode-stride-selector-g5f6hl`
  at `7c62153e`, 31 commits past `9bfc4362`. It ran through `scratch/plan/build/change-v1.js`
  from the brief `scratch/plan/build/M1/brief.txt`, three review rounds. What stands:
  - **One shape of the copy's identity.** Both sides had fixed a copy's claim about a
    partition's records carried across its rewrite. The merged tree has upstream's
    representation (`identity: bool`, `identity_of`, `CachedBlock::identity_for`) with this
    branch's key compare kept at the patch (`Reader::splice_run` takes the rank's entry where
    the claim is of the table's partition or one key compare at that entry proves it, and
    searches otherwise), chosen on the land probe's count. Both sides' `tests/settle.rs` tests
    stay, each red with the fix out. `CLAUDE.md`'s two entries are one.
  - **Text-clean and wrong, twice.** Upstream's `OrdIndex::head_bracket` read `self.top`, a
    field this branch made a method (`top()`, which is also what is right for an index by
    the piece; a new ordindex test holds the bracket by the piece to the mapped one). Both
    sides added `tests/settle.rs` tests whose stores were named `copy-identity-rewrite`: one
    directory for two tests of one process, red 15 runs of 15 until this branch's were
    renamed. Then every function both sides touched was listed and its merged body held to
    the base plus both sides' changes (`both/both.py`, `union.py`), and every changed file
    to the exact per-line sum (`whole3.py`: 74 of 84, the other ten the hand-resolved ones).
  - **Two engine edits beyond the merge, for the count.** Upstream's read-mode note
    (`ModeNote`) is a cell a field where it was one cell copied whole in and out a read:
    `Reader::scan` reads 21 instructions a scan under upstream where the plain merge read 11
    over. `OrdIndex::seek_in_exact` searches the heads held in memory unchecked with the
    window bounded once (a mapping by the piece holds none and takes the head-at-a-time
    loop): `Reader::cut_raw` under upstream. Both read by two reviewers and by the launcher.
  - **This branch's gates over upstream's code.** The workflows test admits the quick job's
    `KEYS` beside `ARMS`. Two cost entries for upstream's doc-hidden test seams
    (`Reader::block_cache_identity`, `Reader::piece_bounds`), their raises declared in
    `scripts/raises.txt`, the first raises this branch declared. The wall needed none; the
    door is clean; upstream's added code calls nothing G1-6 removed. `docs/figures` was
    behind upstream's own ooc rows and the bench group red on it: redrawn by
    `sh bench/scripts/figures.sh`.
  - **Under a cipher.** Each new upstream mechanism read for what bytes it reads: the bounds
    walk (the index's top level, a record by the piece on a tie), the landing that leaves
    bounds unread and the quiet third merge (atomics), the fence-stepping cursor (the fence in
    memory, the page by `slice_at`), the sparse walk's key step (a field). The differential
    fuzz beyond the gate: seeds 1000 to 4000 under a cipher with a 16 KiB cache and in the
    clear, block cache on, options varied, and 1000 to 2000 with the compact form forced: no
    failure; the counters show the cipher runs draw upstream's options. Every arm of the
    roster and every cipher arm opened once at ten thousand keys, no refusal.
  - **The count against `7c62153e`.** All fifteen probes under upstream by their totals
    (`cost-r3.log`; the four that run the bench code again in `cost-r4.log`): read -38 an
    operation, scan -382, merge -215, land -344, the loads -6 to -8. The gate still exits red
    by function, as it did against `9bfc4362`: each gainer is paired with its loser in the
    notes, is once a store, or is on the list below.
  - **Two of upstream's tests were flaky, each red on upstream's head alone, each fixed in
    the test.** `a_bounds_walk_places_a_boundary_by_its_head_and_reads_its_record_only_on_a_tie`:
    every store walks the keys twice (the upkeep thread's fill, the landing's walk), and a
    seal that freezes the burst before the lent fill runs leaves one store a walk short; red
    5 of 5 with the upkeep thread held; the test fills inline and counts the walks since the
    burst was filed. The stride group's conversion steer in the fuzz: a seed's asks, each
    replaced by the next and taken only at the segment work's turn, can all go untaken beside
    seals and merges (16 of 24 red on upstream's head six at once); the test asks the first
    served partition three times after the writer's last settle, settling each, and holds a
    move each way. A durable test that read a stopped queue's flag and then a commit's answer
    waits for the answer it reads.
  - **The lists' check answered by its base** (`2a0d750a`, the launcher's, after the merge):
    the two raises were let through against the branch's tip before the merge and read red
    against the main line, which predates every list. A declaration for a list the base does
    not have holds nothing now and is said so; three cases in `scripts/ratchet-red.sh`, three
    mutants red. `sh scripts/check.sh lists` is green against both bases.
  Notes, mutants, scripts, results and count logs: `scratch/plan/build/M1/` (and `T19/` under
  it for the lists' fix). Open, each owed to a named place:
  - **The cost gate against upstream cannot fail usefully**: its verdict is red before and
    after any change, so a new regression reads like the reconciled list. A baseline of
    accepted gainers, or a declared group held to its net, that `scripts/cost-diff.py` reads
    (`scripts/raises.txt` is shaped to carry it). And four gainers in the merge probe have no
    loser named since T17: `OrdIndex::whole_key_at` +10.9 an operation,
    `Db::seal_table`'s closure +5, `Db::append` +4, `Emitter::enter` +3; `Maint::join_seal`
    reads +0.034 against its bound of 0.010 in the land probe (0.027 at T17). A change of its
    own, before the integration.
  - `tests/cipher.rs` `the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit`
    ("the builds gave nothing back") went red once more, in the stride group, and is still
    undiagnosed: `scratch/plan/build/M1/cipherflake-red.txt` has its counts. Task 14.
  - `heads owned` is never reached under a cipher (the heads' copy is a mapped file's):
    an option that silently does nothing there. The page layer's, or a change of its own.
  - Upstream's new defaults (`bounds_by_heads`, `land_bounds_read`,
    `merge_parallel_quiet: 3`) were priced on plain stores; the cipher arms inherit them
    unpriced, and no cipher arm with them off exists. Owed to S5: each cipher arm against
    the same arm with the option off, at 300,000 keys and, for the bounds options, under a
    bound at ten million.
  - The bounds test no longer runs the walk on the lent-fill path, and its fills' room was
    not driven closed by real refaults (a forced fill-off stands in). The fuzz's three-ask
    shape ran over the gate's seeds only. The suite's `Supdb::with_policy` costs about 9,000
    instructions more once a store than upstream's (an `Options` that holds `Arc`s, built by
    struct update): the suite's open, not the engine's.
  - The walk's start under a stale claim is still reached by no store test (T17's M4b); a
    unit test of `identity_for` holds the predicate.
- **G1-7 `sealed` landed: `41a07cf6`.** The read path's whole borrows are sealed to their
  readers by the compiler. `Bytes::lend_at` takes a zero-sized `blob::lend::Token` whose
  constructor is `pub(in crate::blob)`, so outside the crate it can be neither called nor
  overridden. `MmapBytes::as_slice` is gone; `MmapBytes::lent<T: whole::Reader>(&self, _: T)`
  takes the token of the reader that may hold a mapped file whole, the ordered index's
  (`OrdIndex::over`, `mapped_heads`) or the stride form's (four functions of
  `src/db/zfile.rs`, upstream code the plan predates), the trait sealed in a private module
  of `src/bytes.rs`. `Paging` is crate-private and `Paging::base` takes `bytes::Base(())`.
  `Blob::record_regions` and `record_pieces` are one crate-private `Blob::records()`
  returning `Records::{Whole, Pieces}`, every caller naming both arms. `warm_file` touches
  through `MmapBytes::touch` (nothing of a paged mapping). Red inputs: compile-fail doctests
  in `src/lib.rs`, each beside a twin that compiles; `scripts/seal-red.sh` in the lint group
  (a copy of the crate in which each barred call compiles at home and fails at its own line
  abroad), with `scripts/seal-red-red.sh` as its own red input; and a source test
  (`tests/source.rs`, `seal_check`) for what the compiler cannot see: a token's module
  growing a second way to make one, a token made apart from its call, a new caller inside a
  reader's module. The plain store: `sh scripts/cost.sh e9a6abff` over all fifteen probes
  green, no hot function moved. The wall's count of the seam's paths out goes 0 to 3, one
  `use` a token, declared in `scripts/raises.txt`. Three review rounds: the first held the
  unit to a token on `lent` (built without one to save the raise), the second and third to
  spellings the source test missed (a renamed import, a call by type, a name handed to a
  macro); the third repair, the check alone, read by the launcher and by no reviewer. Notes,
  mutants, scripts and count logs: `scratch/plan/build/G1-7/`. Open, each owed to a named
  place:
  - **The source test's weight, for the closing critic.** It is some nine hundred lines of
    `tests/source.rs`: a reading of identifiers off the lexer, three lists of allowed sites
    (`SEAL_DECLS`, `SEAL_CALLS`, `SEAL_MACRO_WORDS`) and a rule that refuses the words `lent`
    and `lend` inside any macro's tokens, with four test assertions excepted by file, scope
    and text. `lent` is a common word in this engine (the upkeep is lent), so an ordinary
    `assert!` that names such a local trips it, and a reformatted excepted line does too. It
    guards against a colleague routing round the seal on purpose. Two ways to lighten it,
    neither built: give the seal's method and token modules names nothing else uses, so the
    macro rule needs no exceptions and the reading by position could become the word
    anywhere; or cut the text half back to the construction sites. Decide with the critic's
    read, as a change of its own.
  - **The stride form does not work under a cipher.** Its four readers take the mapped file
    whole (`lent`), which is nothing for a paged mapping, so under `ZPages::Always` with a
    cipher `attach_stride` and `check_stride` refuse silently and the partition is served in
    the block form. From before this unit (through `as_slice`). Owed to the page layer's
    stage for the compact form (S5), with a test of today's behaviour first.
  - Two shapes counted and refused are in the notes for whoever tries them again: a lendable
    length on `Bytes` with the lend a function of `blob` (`Blob::scan_whole` and the public
    `scan_at` over), and G1-5's reviewer's dispatch in `Blob::open_with` (over in the merge
    and land probes), so G1-5's three instructions a segment opened stay.
  - `scripts/seal-red.sh` and `seal-red-red.sh` add `cargo check` builds of a copy of the
    crate to every lint run; their time is in the closing gate's log.
  - Docs for G1-8: the prose of `lend_at`, `lent`, `Paging`, `touch` and the crate docs'
    section on what the read path does not hand out (one sentence of it is history);
    `docs/zpages-design.md` still names `record_regions`.
  - Not built for macOS or aarch64; `cross-arm` not run.
- **Two more durable helpers answered the segment work's failure by the stop's flag
  (`067d9fa8`, the launcher's).** `ss_no_room_for_the_run` and
  `ss_handed_over_a_stopped_queue` waited for the stopped queue's flag and then read commits
  and seals on the bet that the first commit would answer the failure; one went red once in
  G1-7's whole run. Both take the failure with `ss_commit_answer` now. Shown by a scratch
  spin of a set width between the flag and the failure's leaving (in the engine for the hunt,
  taken out): the tests as they stood red 24 runs of 25 at a millisecond, the fixed ones
  never at eleven widths, the whole durable binary under three widths with no test red.
  `scratch/plan/build/T20/` has the sweep.
- **G1-8 `docs` landed: `581e11cc`.** Comments and docs alone (the diff of `src` and
  `bench/src` with comments aside is empty). `CLAUDE.md` says what holds the wall, the door,
  the seal, the lists and the count as the gates are now, and its Layout table has the
  stage's scripts and lists; the docs the earlier units left untrue or with history in them
  are corrected (`BlobOptions::verify_index` and `read_ahead`, `Bytes::authenticated` and
  `resident_share`, `Paging`, `lend_at`, `lent`, `touch`, `verify_row_by_piece`, `ModeNote`,
  the crate docs' section on what the read path does not hand out, `docs/zpages-design.md`'s
  header). `README.md` and the crate docs say what a store under a cipher does, each from
  the code: the controller leaves it in the raw form unless `zpages_under_cipher` is set
  and `ZPages::Always` builds the compact form; the stride form is not built there (a build
  that asks for it builds the block form and counts the decline, `zfile::StrideNo::Kept`);
  the heads' copy (`Options::heads_owned`) is not made. `ROADMAP.md` has an entry for each
  of the last two with what would make it work. Its red input is a scratch script
  (`scratch/plan/build/G1-8/docs_red.py`, sixteen checks that hold a doc sentence to a line
  of code or a test's name; 62 findings on the tree before, none after), not a gate in the
  tree. Two review rounds. Open: no test holds the stride form's decline or the missing
  heads' copy under a cipher (the docs say both from the code; owed to S5); the look that
  would copy the heads still asks the kernel for their ciphertext before it finds nothing
  to copy, where the compact form is on under a cipher (in `ROADMAP.md`).
- **The stage's closing gate and critic (22 Nov, on `31b33339`).** `sh scripts/check.sh`
  green, every default group in one run (build test stride lint wasm bench lists); the
  lists group against the stage's base green; the count against upstream's head `7c62153e`
  with every probe's total under upstream (read -40 an operation, scan -382, merge -198,
  land -337, the loads and commits -6 to -8) and no flagged function new beside the merge's
  logs but one of 140 instructions over the seal probe's whole run
  (`std::fs::OpenOptions::_open`, unexplained). The critic read the tree and the units'
  notes and wrote `scratch/plan/build/close/g1-built.md`: what each gate holds, the red
  input that showed it, and a way to write the barred thing that passes it. Read it before
  touching a gate. What it found, by weight, and what each calls for:
  1. **Three gates give no verdict on a pull request into the main line.** The cost group
     exits red by function against any base (moves) and against the main line counts weeks
     of upstream's work; the lists hold nothing against a base that predates them, which the
     main line does until this branch lands; and `cross-arm` died in two parallel runs of
     five with a crash inside QEMU ("QEMU internal SIGSEGV", never a test's failure; green
     one thread at a time, 530 tests; green five of five at `19d078d1` with 519).
  2. **The seal's source test trips on ordinary code.** Any `lent` or `lend` inside a macro
     is red (the engine has `to.lent`), its exceptions and `SEAL_DECLS` lines are keyed by
     exact text that rustfmt can rewrap, and any `.base(x)` call anywhere in `src` reads as
     the seal's. Recommended: rename the seal's words to names nothing else uses, drop the
     exceptions and read the word anywhere.
  3. **`OrdIndex::whole_key_at` is probably a real cost a key in a merge** (+10.9
     instructions an operation in the merge probe since T17): it reads `top()` and `head()`,
     each with a cold arm that returns, where upstream read a field and asserted. Three
     smaller gainers beside it are undecomposed.
  4. **Three mechanisms do nothing under a cipher**, which the owner's rule does not allow
     as a property: the stride form (declined at the build), the heads' copy (not made), the
     quiet third merge (runs only where the compact form is installed). `README.md`'s
     "every option a store in the clear runs with, a store under a cipher runs with" is true
     of what is accepted and false of what acts. G2's conformance run needs the first two
     fixed, or it lists them as seams.
  5. **A whole borrow the seal does not see**: a new walk inside `src/blob.rs` over `flat()`,
     `Sec::get` or `flat_lent`, or a new caller of `ZFile::stride_bytes`, compiles, passes
     every gate and answers nothing under a cipher. G2's conformance pass is what holds it,
     at run time.
  6. Smaller: red inputs kept out of the tree run nowhere (G1-6's strace gate, G1-8's
     `docs_red.py`, G1-4's two probes); the door's libc test reads cargo's registry and
     fails offline; `compact_run` reads a directory word an input of every merge under a
     cipher and throws it away; the ordered index's CRC skip keys on `piecewise` where
     `Bytes::authenticated` now exists.
  It found nothing wrong in the commits it read (the merge's two engine edits, the lists'
  fix, the durable helpers' fix, the merge's bench test); it did not read the last repairs
  of G1-1, G1-2, G1-3, T18, T17 and G1-5 line by line.
- **Next, each a change of its own through `scratch/plan/build/change-v1.js`, in this
  order:** (1) the seal's words renamed and its source test lightened; (2) the `cross-arm`
  crash found and the test QEMU cannot run kept from the emulated run alone; (3) the cipher
  test that fails now and then diagnosed (task 14); (4) `whole_key_at` and the three small
  gainers decomposed and brought down, each held by the count against the commit before it
  (task 20; the baseline of accepted moves against a named upstream commit that this note
  planned is not built, since no base is upstream's now: the cost job's base is the event's,
  the main line for a pull request, and against the main line it counts everything this
  branch merged from upstream until the main line holds that work, so until then the rule
  "a plain store pays nothing" is held change by change, against the commit before); (5) the
  stride form and the heads' copy made to work under a cipher by the piece, with tests, and
  the docs' sentence made true. Then G2 (`scratch/plan/g2.md`), its plan first brought up
  to this tree: the critic's section "For G2" lists what moved. Upstream is not fetched or merged
  (the owner, 22 Nov; it was, before each launch, until then).
- **The owner's to decide, none of it blocking:** whether the lists should land on the
  main line ahead of this branch so that a pull request is held by them (today they hold
  pushes only); what the cost job's base is in CI before upstream itself is on the main
  line (answered 22 Nov, as read here: the main line and never upstream, which is what
  `scripts/base.sh` already takes for a pull request); whether the door's libc
  test should keep reading cargo's registry.
  Every unit from here is launched with a build note: a CI job it adds is
  `sh scripts/check.sh <group>` with nothing in the YAML but event values, a list it adds is
  registered in `scripts/ratchet.py`, and a move between functions the cost group reads red is
  said in its notes with callers and callees, the gate not weakened.

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

(Written 20 Nov, before the owner's instruction of 22 Nov to stay wholly off upstream: the
integration is with the main line, when the owner asks for it, and no step of it fetches,
merges or counts against upstream's branch. What follows is the old plan, kept for its other
steps.) Merge upstream's head (9 hunks at the stop), fix the fragile zpages tests above, run
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
