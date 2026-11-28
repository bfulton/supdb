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

All of phase 1 is one branch, one commit a cluster, landed on
`release-candidate` once its whole gate is green; there is one pull request
in all, `release-candidate` into main at the end. The map of option to
cluster is `release/phase1-clusters.csv`; its `pr` column is the reviewer's
grouping, kept only as the order the clusters are taken in. What each
cluster must carry and the test functions it touches are in
`release/ledger-review/consistency-and-order.md`.

- First the small, isolated, mostly refuted removals.
- Then the read and scan paths, the forms, the compact form's chain in its
  dependency order, and the seal and segment writer.
- Last: the pin protocol, the drain and run close, the ordered run, the
  log, the manifest and the encryption.
- Then the kept experimental fields are hidden from the docs.

Clusters that touch different code are written in parallel in worktrees of
their own and brought onto one branch; the whole gate runs on the branch,
not on each piece.

## What every commit is held to

- `sh scripts/check.sh` green, and `sh scripts/cost.sh <the commit before>`
  green, once the gate reads C0 green against itself (the first pull
  request, `rc/cost-floor`, is that fix).
- In phase 1 a count may move down or sideways. A red passes only where
  every gained function is shown, by instruction, to be inlining that
  moved, padding by a sum by mnemonic, or register allocation in a function
  whose source the removal changed (a spill and its reload, named by
  instruction), with the probe's net at or under zero
  (`release/baseline.md`, 3.1). The first batch's one red was the last kind:
  `Reader::read_layers` +2 an op, a spill and a reload of `rsi`, in probes
  whose totals fell by 3 a read.
- The lists that only go down are lowered in the same commit:
  `tests/data/cost-entries.txt` for a removed seam, `tests/data/couplings.txt`
  for a removed branch that held a counted token.
- **The fuzz keeps its draws.** A removed option that `tests/fuzz.rs` draws
  keeps its draw call with the value discarded (`let _ = side.chance(25);`),
  so every seed runs the same cases after the commit as before it.
- Each deleted arm's line in `bench/DESIGN.md` goes with it.
- An agent that did not write it reads the diff and re-runs the gate
  before the branch lands.

## Amended since the reviews

- Decision 1 is revised: `Options` does not become `#[non_exhaustive]`.
  That attribute forbids struct literals outside the crate, and the tests,
  the suite and the examples hold 667 of them. Kept experimental and
  test-seam fields are hidden from the docs, and that is all the last
  cluster does.
- `CLAUDE.md`'s sentence that the comparison arms are not dead code names
  four options that go (`cursor_merge` stays as a test seam;
  `scan_merge`, `scan_snapshot_arena`, `flush_ranges` go). The pull request
  that removes the last of them rewrites the sentence.

## Batches

| batch | clusters (the reviewer's grouping) | branch | state |
|---|---|---|---|
| 1 | PR1: adaptive-lag, fit-trim, zpages-drop, seal-shape, upkeep-wake | `rc/phase1-pr1`, `2eca03bd` to `31c90d38` | full gate green; cost read by function (one spill, net -3 a read); an independent review passed every removal as keeping the default's behaviour |
| 2 | PR2, PR3, PR4 | the same branch | being written |
