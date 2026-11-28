# Pre-tag passes: hand-off

Everything lands on one branch, `claude/supdb-pretag-coordination-4qj5t4`,
which becomes the one pull request into main at the end (Bright,
2026-11-28). `release/` comes off in that branch's last commit. This file
is where to pick the work up; `release/STATUS.md` has the decisions and the
per-phase record, `release/phase1.md` the rules every removal is held to.

The work is being wound down by stages because the account's weekly usage
is near its limit (Bright, 2026-11-28, about 9:20 AM Eastern). Each session
pushes after every finished unit and keeps a hand-off file current, so a
stop at any moment loses at most the unit in progress.

## What the branch holds

Read with `git log --first-parent b3a043dc..HEAD`.

- C0 = `b3a043dc` (#37), main at the freeze.
- The cost count made deterministic (#38's `2fc48db0`, merged at `6f800c28`).
- Phase 1, the options the ledger removes: batches 1 and 2 (the reviewer's
  groups PR1 to PR4, `98e4e8f4`, `8e3f9d71`) and batch 2's review fixes
  (`2bf53fec`); track B's PR9 (`793cec2b`); tracks A, C and D's first
  groups, PR5, PR13 and PR22 (`64f1a7b2`, `7d08e666`, `37670e5b`); track
  C's PR14 (`e927955f`).
- The supply-chain fixes (`8ae4c5f6`): workflow permissions and pins,
  bank-row's build and push in two jobs, dispatch inputs held to digits,
  `publish = false`, `lz4_flex` 0.11.7 and the floors raised.
- The audits' findings (`release/findings/`), and the flakes session's test
  fixes to `e7828131` (`ca5d54ef`).

Rounds run on the merged tree (`release/tools/round.sh`):

| round | head | gate groups | cost against the round before | fuzz past the ranges |
|---|---|---|---|---|
| 1 | `793cec2b` | all green | green | green |
| 2 | `abeecbdd` | all green but one test: the settle copy's abandon, twice (named since: an engine race, fix in `claude/supdb-pretag-flake-settle`) | red: `compact_run` +2.0 and `Reader::settle_each` +13.9 an op, both spills carried (below) | green |
| 3 | `e927955f` | all green | green | green |
| 4 | `68c68ad0` | all green; holds track C's fix to PR14 (`607ed7d0`: removing a flag from `Db::z_attach`'s positional tuple shifted the raw-drop test onto the decode timer's slot, so a store with `zpages_drop_raw` off dropped anyway; now a named struct and a test) and the six hostile-input fixes | red by the hostile-input fixes' checks: `Db::append_batch` +13.8 an op (probes +4.8), `merge_walk::main` +2.0, `SegmentWriter::end_with` +1.0, the open path's few hundred over the run; and `compact_run` +13.5 an op on `merge`, read by instruction as inlining that moved (the k-way walk laid out again around `end_with`, which took `key_hash` and `BigVec::push` in), with the `merge` probe's total down 21 an op once the 4 GiB cut lands | green |

## Branches still to merge

Each session's branch, what it holds, and its hand-off file. Merge in this
order where they touch the same code, and run a round after each batch of
merges. A conflict in `bench/src/engines.rs` or `tests/fuzz.rs` between two
tracks is two removals side by side: keep both removals, and read the lines
after the hunk, since a hunk can end inside a block whose brace is common
(`abeecbdd` fixed one that did not).

| branch | what | state | hand-off |
|---|---|---|---|
| `claude/supdb-pretag-p1-a` | PR5 to PR8, PR19 | PR5 merged; PR6 pushed (`db68a9b`, gate green, a read-path spill `supdb_get` +2 an op being looked at); PR7, PR8, PR19 in progress | `release/handoff/p1-a.md` |
| `claude/supdb-pretag-p1-b` | PR9 to PR12, PR20, PR21 | PR9 merged; PR10 to PR12 pushed (`62c73dc`), gates green, merge once its `settle_patch_in_place` assertion and the `ZFUZZ_SNAPKEPT` leftover are pushed | `release/handoff/p1-b.md` |
| `claude/supdb-pretag-p1-c` | PR13 to PR18 | PR13, PR14 merged; PR15 on | `release/handoff/p1-c.md` |
| `claude/supdb-pretag-p1-d` | PR22 to PR29 | PR22 merged; PR23, PR24 in progress | `release/handoff/p1-d.md` |
| `claude/supdb-pretag-hostile-fixes` | the six highs of `release/findings/hostile-input.md` and `unsafe.md` | done and merged at `68c68ad0`: gate green twice, fuzz past the ranges green; the browser reader opens all 14,364 crafted cases with no trap and no misread; the wasm module grows to 133,566 bytes (cap 131,556 at C0: the reason is the new refusals' checks and their messages); the count is red by those checks on the write and open paths, read paths unmoved | `release/handoff/hostile-fixes.md` |
| `claude/supdb-pretag-flakes` | the known flakes | five fixed in their tests and merged; the pager's placeholder debt fixed in the engine (`58810f2`); the adoption overshoot (an engine design question), the builders' rest test and `cipher_looks` open | `release/findings/flakes.md` |
| `claude/supdb-pretag-flake-settle` | the settle copy's abandon | cause named and forced (a writer's freeze judges the upkeep by a position read before its publish); fix in progress at all three sites | `release/findings/flake-settle.md` |
| `claude/supdb-pretag-flake-hunt-1` | `test` and `stride` loops, CI history | CI history read; loops under load in progress | `release/findings/flake-hunt-1.md` |
| `claude/supdb-pretag-flake-hunt-2` | `onefile`, lib, suite, fuzz, environment premises | loops in progress | `release/findings/flake-hunt-2.md` |
| `release-measure` | the baseline at C0 | rows on two host classes (`release/baseline.md`, 7.17); floor tables in core done, bounded in progress | `measure/LOG.md` on that branch |

## Carried on the merged tree

- **Spills.** `Reader::settle_each` (batch 2, track A's PR5, track B's
  PR10), `Reader::patch_block` (track B's PR10, `splice_run` inlined) and,
  if track A does not settle it, `supdb_get` on the point read: each read by
  instruction as register allocation after a removal, each with its probe
  above zero. They are fixed once all the tracks that move them are
  merged, and counted against C0 (`release/phase1.md`).
- **`compact_run`** +2.0 an op on `merge` (track C's PR13): counted again at
  PR18's end; one try at a shape, else declared in `scripts/raises.txt`
  with its reading.
- **The hostile-input fixes' raises**, each a check a wrong answer needed:
  `Db::append_batch` and `put_batch` about 5 an op on `load_*` and
  `commit_*` (the batch's length pass), `Db::append`, `put` and `delete` 2
  an op (the length test of a write), `SegmentWriter::end_with` 1 a key,
  and the open path's checks on `land` (`Blob::open_with`, `Sec::get`,
  `MemTable::index_insert`). The count has no raise file of its own: the
  pull request names them with their reasons.
- **Positional lists.** Every track checks its removals for a tuple, array,
  bit set or positional list whose later entries shifted (track C's PR14
  bug), and records the check in its hand-off file.
- **PR30**, the last group of phase 1: `#[doc(hidden)]` on the kept
  experimental and test-seam fields.

## After phase 1

- Phase 2: split `src/db.rs` (the renames file read by
  `scripts/cost-diff.py` lands first, `release/baseline.md` 7.15).
- Phase 3's mediums (`release/findings/hostile-input.md`, `unsafe.md`):
  mid-log WAL damage read as a torn tail; a valid segment refused under
  `inline_max` 0; the shared table's sequence lock copying with plain
  `memcpy`; safe public `zlz4` functions running AVX-512 unchecked;
  references made over memory others write. And the earlier audits' items
  (`release/findings/guarantees.md`, `format.md`, `crypto.md`).
- Phase 4: the flake hunt's loops to dry on the last merged tree.
- Phase 5: `docs/engine.md` still names removed options; the roadmap,
  README and release files.
- Phase 6: measure at the last code commit, C0's kept binaries paired
  beside it on the same host.
- Phase 7: Bright tags.

## Bright's

- Version and tag name, and whether to publish to crates.io (until then
  `publish = false`).
- The items in `release/STATUS.md`, "For Bright", and `release/history.md`,
  "For the owner".
- Closing #38 and deleting `release-candidate` and the session branches
  once merged.
