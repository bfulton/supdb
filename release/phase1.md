# Phase 1: what goes, and in what order

The options ledger (`release/options.csv`) is judged: every row has a
category, a decision and a note with its evidence. Five agents judged a
fifth each against one brief (`release/ledger-review/brief.md`); two
reviewers then read the whole ledger, one for wrong removals
(`release/ledger-review/removals.md`) and one for consistency across the
slices and for the order of the work
(`release/ledger-review/consistency-and-order.md`). Both reviews are
applied. Where they disagreed, the row is kept.

## What the ledger now says

| category | rows | phase 1 |
|---|---|---|
| inline | 154 | removes the option and the other branch, keeps the default's behaviour |
| delete-refuted | 100 | removes the option and the refuted branch |
| delete-unread | 6 | nothing sets it; removed as an inline is |
| test-seam | 63 | stays, hidden; its arms may go, a test holds a state with it |
| keep-experimental | 99 | stays, hidden; it prices a shipped default or an open roadmap item needs it |
| keep-stable | 31 | stays public |

`python3 -B -c "import csv,collections; print(collections.Counter(r['category'] for r in csv.DictReader(open('release/options.csv'))))"`

260 options are removed, of 453. No roster arm, no cost probe's arm and no
default changes: the deletion review checked every removal for each of
those mechanically.

## The order

Thirty pull requests into `release-candidate`, one cluster a commit, each
under about 1,500 changed lines by the reviewer's estimate. The map of
option to cluster and pull request is `release/phase1-clusters.csv`; the
table of pull requests, what each must carry, and the test functions each
touches are in `release/ledger-review/consistency-and-order.md`, "The pull
requests, in order".

- First the small, isolated, mostly refuted removals (PR1 to PR3), so the
  gate and the review are proven where little can go wrong.
- Then the read and scan paths, the forms, the compact form's chain in its
  dependency order, and the seal and segment writer.
- Last, in small pieces: the pin protocol, the drain and run close, the
  ordered run, the log, the manifest and the encryption (PR24 to PR29).
- PR30 hides the kept experimental fields from the docs.

The pull requests are sequential, never parallel: nearly every one edits
the arm tables of `bench/src/engines.rs`.

## What every pull request is held to

- `sh scripts/check.sh` green, and `sh scripts/cost.sh <the commit before>`
  green, once the gate reads C0 green against itself (the first pull
  request, `rc/cost-floor`, is that fix).
- In phase 1 a count may move down or sideways. A red passes only where
  every gained function is shown, by instruction, to be inlining that moved
  with a net at or under zero, or padding by a sum by mnemonic
  (`release/baseline.md`, 3.1).
- The lists that only go down are lowered in the same commit:
  `tests/data/cost-entries.txt` for a removed seam, `tests/data/couplings.txt`
  for a removed branch that held a counted token.
- **The fuzz keeps its draws.** A removed option that `tests/fuzz.rs` draws
  keeps its draw call with the value discarded (`let _ = side.chance(25);`),
  so every seed runs the same cases after the commit as before it.
- Each deleted arm's line in `bench/DESIGN.md` goes with it.
- A reviewer that did not write the pull request reads the diff and re-runs
  the gate.

## Amended since the reviews

- Decision 1 is revised: `Options` does not become `#[non_exhaustive]`.
  That attribute forbids struct literals outside the crate, and the tests,
  the suite and the examples hold 667 of them. Kept experimental and
  test-seam fields are hidden from the docs, and that is all PR30 does.
- `CLAUDE.md`'s sentence that the comparison arms are not dead code names
  four options that go (`cursor_merge` stays as a test seam;
  `scan_merge`, `scan_snapshot_arena`, `flush_ranges` go). The pull request
  that removes the last of them rewrites the sentence.
