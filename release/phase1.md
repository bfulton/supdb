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

All of phase 1 is one branch, one commit a cluster, merged by batch onto
the working branch (`claude/supdb-pretag-coordination-4qj5t4`) once its
whole gate is green and it is reviewed; there is one pull request in all,
that branch into main at the end. The map of option to
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
- `CLAUDE.md` at C0 names no option in its sentence that the comparison
  arms are not dead code (#37 rewrote it), so phase 1 does not edit
  `CLAUDE.md`.

## Batches

| batch | clusters (the reviewer's grouping) | branch | state |
|---|---|---|---|
| 1 | PR1: adaptive-lag, fit-trim, zpages-drop, seal-shape, upkeep-wake | `rc/phase1-pr1`, `2eca03bd` to `31c90d38` | full gate green; cost read by function (one spill, net -3 a read); an independent review passed every removal as keeping the default's behaviour; merged onto the working branch at `98e4e8f4` |
| 2 | PR2, PR3, PR4 | the same branch, `9fc05328` to `08b28998` | full gate green; cost read by function (reds in `Blob::scan_at_in`, `Reader::scan`, `settle_each`, `overlay_all`, each read as inlining or register allocation, every probe total down but `merge`'s clock-driven sweep); merged onto the working branch at `8e3f9d71`. The independent review ran the merged tree whole: gate green (42 minutes), the count with every probe total at or under the base and the same four function reds, each read by instruction as allowed, and the fuzz past the gate's ranges green under the cipher and in one file. It found one test of a kept default deleted with its neighbours (`zpages_stride_room`'s refusal at a partition's build), a doc line naming a deleted test, the shipped order of the sample left untested, and three refuted scan entry points kept with no engine caller; those four are fixed forward on `rc/review-b2-fixes`. Removing the held walker let the compiler inline the raw scan walk into `scan_whole`, a twentieth more a scan record; `08b28998` keeps the walk out of line, as at the base. `zpages_buy_crosses` is held for PR7, since three roster arms pin it |
| 3 | A: PR5, PR6, PR7, PR8, PR19 | `claude/supdb-pretag-p1-a`, a session of its own | started 2026-11-28 |
| 3 | B: PR9, PR10, PR11, PR12, PR20, PR21 | `claude/supdb-pretag-p1-b`, a session of its own | started 2026-11-28 |
| 3 | C: PR13 to PR18, the compact form's chain | `claude/supdb-pretag-p1-c`, a session of its own | started 2026-11-28 |
| 3 | D: PR22 to PR29, the seal to the encryption | `claude/supdb-pretag-p1-d`, a session of its own | started 2026-11-28 |
| 4 | PR30, the API | the working branch | after the four tracks are merged and their merged gate is green |

The four tracks of batch 3 start from `8e3f9d71` at once, each on a VM of
its own, each pushing a gated and counted head after every group. Each
track's branch is merged here as it comes, and the merged tree runs whole
(the gate, the count against the head before the merge, and the fuzz past
the gate's ranges) before the next is merged.
