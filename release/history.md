# History audit

The read-only audit of the repository's history that precedes the tag,
taken on 2026-11-27 at C0 = `b3a043dc` (tree `7f58d776`) over every fetched
ref: main, the branches, and the 37 pull-request heads
(`git fetch origin '+refs/pull/*/head:refs/remotes/pull/*'`). Each section
under `release/history/` was written by one agent and checked by another
that re-derived its figures by a different route; the `*.review.md` files
are those checks, and where they differ the figures below are the
reviewer's. Every figure there carries its command. The secrets section
was judged twice per candidate, by one judge arguing real and one arguing
benign.

## Verdict

- **No credential is in any ref, pull-request text or Actions log read.**
  Nothing needs rotating. Thirty-two candidates were found: one is real,
  and it is workflow configuration, not a leak (below); five need a look
  from the owner; 26 are benign (`release/history/secrets.md`).
- **History stays as it is, and is fixed forward** (the plan's option 1).
  It is the plan's default, and the audit finds nothing that argues for a
  rewrite:
  - no citation in the tree breaks under it;
  - a re-message of main (option 2) would replace 31 commits GitHub
    signed, rewrite 32 hashes, and leave every old commit served by GitHub
    under the pull-request refs, the `bank/quick-*` branches and GitHub's
    own copies of force-pushed commits;
  - a fresh repository (option 3) would leave 126 of the 130 banked rows,
    and 3,124 citing sites in 2,413 files, naming commits it does not
    hold.

## What the history holds

**Main** is 32 commits: 31 squash commits (#1 to #20, #27 to #37) and an
empty root (`git rev-list --count b3a043dc`). Its messages are 1,842,812
bytes (`git log --format=%B b3a043dc | wc -c`, less one newline a message).
Three bodies are 339 KB or more (#35, #36, #27). 42% of body bytes are lines
an earlier commit on main already holds. The cause is shown, not guessed:
a branch cut from a branch that still carried commits an earlier squash had
landed, and GitHub's default squash message, which lists the branch's
commits (`release/history/messages.md`). Trailers: 1,542 `Co-Authored-By`,
1,543 `Claude-Session`, 15 distinct session links.

**Size.** Main alone is about a quarter of the packed bytes. About two thirds of
all packed bytes are reachable only from pull-request heads, three fifths of
that under `scratch/` in #6, #35 and #36; no rewrite of main touches them.
Banked rows under `bench/runs` are 64 MiB in the tree and 45% of main's
packed bytes. `src/db.rs` has 698 versions over all refs, 42% of all blob
bytes (`release/history/size.md`).

**Citations.** The tree names 351 hashes at 3,261 sites
(`python3 release/tools/citations.py`, output in `release/citations.tsv`).
Of the commits: 8 are on main, cited at 78 sites, and 334 only under pull
refs; 6 hashes are other repositories'. Three are cited as commits and
resolve in no fetched ref: `18aefe7` in `docs/engine.md`, and the two quick
rows named for `5227305` and `14f8cc6`. GitHub still serves all three by
hash (`release/history/citations.md`). Every cited commit needs the pull
refs fetched to resolve, so a docs check built on the tool fetches them
first.

**Branches and pull requests.** No pull request, issue or run is open, and
there are no tags or releases. The two `claude/*` branches carry nothing
main lacks: `claude/e-recode-auto` is in #27, and
`claude/wal-ahead-beside-wip` adds one work-in-progress commit superseded
in #27's history. Ten `bank/quick-*` branches on GitHub (nine fetched here)
each hold one unsigned bot commit of three hosted quick rows and redrawn
figures. None became a pull request: the repository refuses pull requests
created by Actions. `bank-row` adds one at every push to main
(`release/history/refs.md`).

**CI at C0.** Main's head is green on all nine `ci` legs in its own push
run (46057759527). The same tree's pull-request run, and the last code
commit's push run, failed one test each:
`a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames`
(`tests/durable.rs`) on the hosted ARM leg in three of the last six ARM
runs, and `a_copys_identity_outlives_a_merge_that_kept_its_partitions_keys`
(`tests/settle.rs`) once on macOS. The cost count has no verdict at C0: it
left `ci` in #36, its workflow has never run, and its last two verdicts,
at #35, were red.

## Fixed forward

Each item goes to the pass that owns it, recorded in `release/STATUS.md`.

| Item | Where it is | Pass |
|---|---|---|
| `bank-row.yml` gives a write token to every job; only `propose` needs it | `.github/workflows/bank-row.yml`, the workflow-level `permissions:` | 3, supply chain |
| No action is pinned to a commit; five workflows set no `permissions:` | all ten workflows | 3, supply chain |
| `CONTRIBUTING.md` says `ci` does not run on pushes to main; it does | `CONTRIBUTING.md`, "How a change lands"; `.github/workflows/ci.yml` triggers | 5, docs |
| Three cited commits resolve in no ref | `docs/engine.md`; two rows under `bench/runs/quick` | 5, docs check |
| The `durable` test fails on hosted ARM; the `settle` test once on macOS | `tests/durable.rs`, `tests/settle.rs` | 4, flakes, or earlier if they block a pull request |
| The cost count has no verdict at C0 | `scripts/cost.sh` | 0, baseline: C0 counted against itself |
| Commit messages repeat landed text | every squash | every pass: each branch is cut fresh from `release-candidate`, so a squash lists that branch's commits only |

## For the owner

Four things only the owner can see. None blocks a pass.

1. **The self-hosted runner.** The repository has had a runner on your Mac
   registered under the bare `self-hosted` label, in a public repository
   that takes pull requests from forks. Today only dispatch-only workflows
   use it, and you started every run. Either set fork pull-request
   workflows to need approval for every outside contributor, or unregister
   the runner while it is unused (`release/history/secrets.md`, G4).
2. **An artifact link** in a working note under pull #6. If that artifact
   is shared, it opens for anyone with the address; unshare it if it
   should not (`release/history/secrets.md`, P5).
3. **Your personal email address** is the author address of 31 of main's
   commits, put there by GitHub's web merge. GitHub's email-privacy setting
   stops that for future merges; nothing removes it from existing commits.
4. **At the tag:** the ten `bank/quick-*` branches and the two `claude/*`
   branches are safe to delete. Deleting `claude/wal-ahead-beside-wip`
   loses one superseded work-in-progress commit.

## Not covered

What these tools cannot read: the repository's settings beyond the main
branch's protected flag (merge methods, the default squash message, the
content of the branch rules, the Actions pull-request setting), GitHub's
signature verification flag for each commit, Actions artifacts' contents,
and anything published outside the repository (forks, the write-up's
repository). The tenth `bank/quick-*` branch was pushed after the fetch and
read only through GitHub. Each section lists its own gaps.
