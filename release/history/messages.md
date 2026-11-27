# Commit messages on main

Measured at `b3a043dc` (tree `7f58d776`), from `/home/user/supdb`. Each figure names its command by number; the commands are at the end.

Terms:

- **Main** is `git rev-list --first-parent b3a043dc`: 32 commits, which are 31 squash commits (#1 to #20 and #27 to #37) and one empty root commit. There are no merge commits. The coordination brief says 33. [C1]
- **Body bytes** are what `git show -s --format=%b` prints: the stored body and one newline. An empty body prints 1.
- **Every ref** is `X="--exclude=origin/claude/supdb-pretag-coordination-* --remotes"`: the other 12 `origin/*` refs and the 37 pull refs, 2,023 commits. The coordinator's own branch is left out because it moves during the audit. [C1]
- Addresses are shown as their first four characters and their length. Session links are counted and never printed.

## 1. The plan's figures, re-measured

The plan's figures were taken at `8025423` over its seven newest commits. Main is five pull requests later.

| Plan figure | At `b3a043dc` | Stale | Cmd |
|---|---|---|---|
| A squash body is the concatenated messages of its branch | Holds, with a limit. A body lists the branch's non-merge commits only, and the four largest list 241, 239, 236 and 235 of their branches' 676, 350, 603 and 604 (#6, #27, #35, #36). The non-merge commits among each branch's newest 250 number 241, 239, 235 and 234. Four bodies are empty (#1, #2, #3, #5). | incomplete | C2, C9 |
| One body is 339 KB (#27) | #27 is 339,043 bytes. #35 (345,260) and #36 (344,869) are larger. | yes | C2 |
| #32 changes 120 lines in two files and carries 94 KB | Unchanged: 2 files, 120 insertions, 1 deletion, 94,494 bytes. #36 is the larger case: 6 files, 147 insertions, 80 deletions, 344,869 bytes. | no | C2 |
| Four of the seven newest open with the same text | The four are #29 to #32. Two of them (#31, #32) are among the seven newest now. #36 repeats #35 without opening alike: 334,950 of its 344,869 body bytes are lines an earlier commit holds. | yes | C5, C6 |
| 599 `Co-Authored-By` and 599 `Claude-Session` lines, seven session links | Seven newest: 790, 790 and 9 links. All of main: 1,542 `Co-Authored-By`, 39 `Co-authored-by`, 1,543 `Claude-Session`, 15 links. | yes | C12 |
| 51 MB of file versions are banked rows under `bench/runs` | 67,067,767 bytes (64.0 MiB) in 130 blobs. At `8025423`: 53,523,449 bytes (51.0 MiB) in 112 blobs. | yes | C20 |
| Nothing under `scratch/` is on main | Holds. No tree of main's 32 commits has a `scratch/` path. | no | C20 |
| Two `claude/*` leftovers | Holds: `claude/e-recode-auto` and `claude/wal-ahead-beside-wip`. | no | C21 |
| Five unmerged `bank/quick-*`; nine exist | Nine exist and all nine are unmerged. Each is one `github-actions[bot]` commit on top of a main commit (#28 to #36), and no file it adds is on main. | yes | C21 |

## 2. Per-commit table

Dates are author dates in the commit's own zone, which is US Eastern for the 31 squash commits and UTC for the root. Title bytes include the ` (#n)` suffix. `"* " lines` counts body lines that start with `* `. [C2]

| commit | PR | date | title bytes | body bytes | "* " lines | Co-Authored-By | Co-authored-by | Claude-Session | Signed-off-by | session links | gpgsig | files | lines + | lines - |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|
| b3a043dc | #37 | 2026-11-27 | 70 | 3,405 | 10 | 10 | 1 | 10 | 0 | 1 | yes | 4 | 1,027 | 14,480 |
| e40d6283 | #36 | 2026-11-27 | 96 | 344,869 | 279 | 276 | 5 | 276 | 0 | 7 | yes | 6 | 147 | 80 |
| 09cadec0 | #35 | 2026-11-27 | 119 | 345,260 | 280 | 277 | 5 | 277 | 0 | 7 | yes | 106 | 78,024 | 2,944 |
| 99d44afa | #34 | 2026-11-27 | 78 | 1,760 | 3 | 3 | 1 | 3 | 0 | 1 | yes | 557 | 49,724 | 8,524 |
| eceb66d6 | #33 | 2026-11-27 | 133 | 68,333 | 47 | 47 | 1 | 47 | 0 | 1 | yes | 1087 | 613,429 | 16,789 |
| 8025423b | #32 | 2026-11-26 | 83 | 94,494 | 89 | 89 | 1 | 89 | 0 | 1 | yes | 2 | 120 | 1 |
| 3706d2bf | #31 | 2026-11-26 | 65 | 93,561 | 88 | 88 | 1 | 88 | 0 | 1 | yes | 5 | 742 | 38 |
| 5a5497be | #30 | 2026-11-26 | 98 | 88,765 | 82 | 82 | 1 | 82 | 0 | 1 | yes | 242 | 121,504 | 4,681 |
| dbc2a2a0 | #29 | 2026-11-25 | 70 | 42,193 | 40 | 40 | 1 | 40 | 0 | 1 | yes | 25 | 41,566 | 484 |
| 5b8d64ae | #28 | 2026-11-24 | 90 | 1,857 | 3 | 3 | 1 | 3 | 0 | 1 | yes | 11 | 447 | 171 |
| 7650d890 | #27 | 2026-11-24 | 114 | 339,043 | 284 | 281 | 5 | 281 | 0 | 6 | yes | 1736 | 1,642,221 | 25,156 |
| 5aad09dd | #20 | 2026-11-05 | 122 | 15,821 | 16 | 16 | 1 | 16 | 0 | 1 | yes | 5 | 798 | 85 |
| c614a23e | #19 | 2026-11-05 | 98 | 13,534 | 14 | 14 | 1 | 14 | 0 | 1 | yes | 5 | 316 | 13 |
| 55d07a98 | #18 | 2026-11-05 | 116 | 11,511 | 12 | 12 | 1 | 12 | 0 | 1 | yes | 6 | 188 | 35 |
| 2a667959 | #17 | 2026-11-05 | 102 | 8,379 | 8 | 8 | 1 | 8 | 0 | 1 | yes | 6 | 703 | 51 |
| 69b639a3 | #16 | 2026-11-05 | 82 | 3,503 | 3 | 3 | 1 | 3 | 0 | 1 | yes | 7 | 1,067 | 495 |
| c03088ac | #15 | 2026-11-04 | 99 | 7,088 | 4 | 4 | 1 | 4 | 0 | 1 | yes | 6 | 669 | 61 |
| 0043b089 | #14 | 2026-11-04 | 56 | 688 | 0 | 0 | 1 | 1 | 0 | 1 | yes | 2 | 11 | 7 |
| d539a606 | #10 | 2026-11-04 | 107 | 3,883 | 4 | 4 | 1 | 4 | 0 | 1 | yes | 17 | 2,368 | 286 |
| d5679e45 | #13 | 2026-11-04 | 143 | 10,758 | 5 | 5 | 1 | 5 | 0 | 1 | yes | 6 | 621 | 18 |
| 3fa224e8 | #12 | 2026-11-04 | 179 | 5,337 | 2 | 2 | 1 | 2 | 0 | 1 | yes | 9 | 719 | 5 |
| 5753ac82 | #11 | 2026-11-03 | 168 | 10,980 | 5 | 5 | 1 | 5 | 0 | 1 | yes | 8 | 1,126 | 109 |
| 4ed9b090 | #9 | 2026-11-03 | 184 | 23,198 | 12 | 12 | 1 | 12 | 0 | 1 | yes | 12 | 2,776 | 60 |
| e8227615 | #8 | 2026-11-03 | 91 | 3,815 | 2 | 2 | 1 | 2 | 0 | 1 | yes | 267 | 778 | 606 |
| c09918df | #7 | 2026-11-03 | 95 | 2,301 | 2 | 2 | 1 | 2 | 0 | 1 | yes | 794 | 58,377 | 12 |
| 37ea8f2a | #6 | 2026-11-03 | 109 | 276,681 | 241 | 241 | 1 | 241 | 0 | 3 | yes | 149 | 921,276 | 4,302 |
| c89f9d30 | #5 | 2026-09-06 | 62 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | yes | 5 | 682 | 193 |
| c8e8d94c | #4 | 2026-09-06 | 77 | 18,561 | 16 | 16 | 1 | 16 | 0 | 1 | yes | 275 | 6,723 | 31,769 |
| b4c7bbff | #3 | 2026-09-05 | 56 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | yes | 7 | 10 | 7 |
| a0f5ecb8 | #2 | 2026-09-04 | 64 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | yes | 82 | 4,769 | 1,477 |
| 3ee646ee | #1 | 2026-09-04 | 90 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | yes | 244 | 47,085 | 0 |
| ab972300 | root | 2026-08-22 | 14 | 76 | 0 | 0 | 0 | 0 | 0 | 0 | no | 0 | 0 | 0 |

Every squash commit carries a PGP `gpgsig` header. The root commit is unsigned and its tree is empty.

`"* " lines` over-counts sub-messages in #27, #35 and #36. Their branches hold squash commits of pull requests merged into the branch (#21, #22, #23, #25, #26), and those commits' own bodies are `* ` lists. The lines that match a branch commit's title are 239, 236 and 235. Of the other 24 non-empty squash bodies, 22 have as many `* ` lines as the branch has non-merge commits. The two that do not are #6, which lists 241 of 676, and #14, whose body is a single message with no `* ` line. [C8, C9]

Titles:

| PR | commit | title |
|---|---|---|
| #37 | b3a043dc | Docs: CLAUDE.md as rules, the roadmap as short items by priority (#37) |
| #36 | e40d6283 | The cost job by hand, and one build ahead at a time where a one-file test holds every word (#36) |
| #35 | 09cadec0 | Encryption under a caller's cipher or the kernel's key, an envelope for finished files, and the store in one file (#35) |
| #34 | 99d44afa | The bounded row under 1.25 GiB at 5fea7c0, and two roadmap items from it (#34) |
| #33 | eceb66d6 | Drains answered at the readable landing, packed copies that keep their ranks across merges, and the frozen tables held to bytes (#33) |
| #32 | 8025423b | Docs: the crash seam's entry, and what the last two pull requests left undone (#32) |
| #31 | 3706d2bf | The tests' crash ends the store's threads before it returns (#31) |
| #30 | 5a5497be | Handles that write under the held log, bursts lent to the upkeep thread, and a leaner settle (#30) |
| #29 | dbc2a2a0 | Point gets and puts beside FASTER, and a log held in the process (#29) |
| #28 | 5b8d64ae | zpages tests hold their clock while a builder job runs; workflows on Node 24 actions (#28) |
| #27 | 7650d890 | Out-of-core work since #20: the stride form, hot partitions served raw, ordered ingest in the log's sequence (#27) |
| #20 | 5aad09dd | The compact form installs at the load's end (on): resident and in-flight partitions covered, Suspect at the crossing (#20) |
| #19 | c614a23e | Two of the controller's builds ahead at once (on): the compact form installs a second into C (#19) |
| #18 | 55d07a98 | WAL frames dropped from the page cache once synced (on); the selector's fault model held to E (not calibrated) (#18) |
| #17 | 2a667959 | Forms fitted while the store is short of memory (on), and a memory budget capped at MemAvailable (#17) |
| #16 | 69b639a3 | Merges in flight beside each other, as an option (priced level, default one) (#16) |
| #15 | c03088ac | Merges keep each input's key across their walks; two refuted levers on the bounded lag passes (#15) |
| #14 | 0043b089 | Name PR #6's history where the docs named branches (#14) |
| #10 | d539a606 | bench: a killed pass is a cell, a cut run resumes from its ledger, and a RAM-backed device for counts (#10) |
| #13 | d5679e45 | Out of core: builds ahead at landing wait while scans are near; the segment work's loop plans a merge when none is in flight; merge trace (#13) |
| #12 | 3fa224e8 | Out of core: the builder ahead starts in core alone; a build asks for its pieces' directory words, a bounds walk for a piece's heads; boundary keys off the top level refuted (#12) |
| #11 | 5753ac82 | The buffered lag bursts: a WAL rotation leaves its frames to the policy, a burst's batches paced behind the upkeep thread, and the lag sweep's first point settled (#11) |
| #9 | 4ed9b090 | The bounded lag points: no piece ranked whole, cuts and merge keys off the heads, pages and heads asked for ahead, merges chained a range at a time, and the lag pass's instruments (#9) |
| #8 | e8227615 | The compact form's controller costs a read nothing in memory, and ships as the default (#8) |
| #7 | c09918df | The bounded roster's row under 1.25 GiB, and the suite's figures committed beside the rows (#7) |
| #6 | 37ea8f2a | The engine since 0.1.0: reader threads, ordered ingest, the block cache and the compact form out of core (#6) |
| #5 | c89f9d30 | Size a segment's head reserve without holding its records (#5) |
| #4 | c8e8d94c | Replace the falsification suite with the time-series suite, under bench/ (#4) |
| #3 | b4c7bbff | Gate each ordering on what its own statement claims (#3) |
| #2 | a0f5ecb8 | Release prep: manifest, license, gates, and the read advice (#2) |
| #1 | 3ee646ee | Supdb 0.1.0: the engine, the falsification suite, and the evidence for and against it (#1) |
| root | ab972300 | Initial commit |

## 3. Repeated text

"Repeated bytes" are the bytes of body lines of 40 or more bytes that also occur in the body of an earlier commit on main. The third figure leaves out `Co-Authored-By`, `Co-authored-by` and `Claude-Session` lines. [C6]

| PR | commit | body bytes | repeated bytes | of which not trailer lines | repeated share |
|---|---|---:|---:|---:|---:|
| #37 | b3a043dc | 3,405 | 607 | 0 | 18% |
| #36 | e40d6283 | 344,869 | 334,950 | 299,217 | 97% |
| #35 | 09cadec0 | 345,260 | 131,618 | 106,843 | 38% |
| #34 | 99d44afa | 1,760 | 431 | 0 | 24% |
| #33 | eceb66d6 | 68,333 | 6,092 | 0 | 9% |
| #32 | 8025423b | 94,494 | 90,813 | 79,407 | 96% |
| #31 | 3706d2bf | 93,561 | 86,867 | 75,590 | 93% |
| #30 | 5a5497be | 88,765 | 46,298 | 35,795 | 52% |
| #29 | dbc2a2a0 | 42,193 | 1,896 | 0 | 4% |
| #28 | 5b8d64ae | 1,857 | 431 | 0 | 23% |
| #27 | 7650d890 | 339,043 | 14,251 | 0 | 4% |
| #20 | 5aad09dd | 15,821 | 13,629 | 11,534 | 86% |
| #19 | c614a23e | 13,534 | 11,649 | 9,810 | 86% |
| #18 | 55d07a98 | 11,511 | 8,785 | 7,202 | 76% |
| #17 | 2a667959 | 8,379 | 4,060 | 2,989 | 48% |
| #16 | 69b639a3 | 3,503 | 431 | 0 | 12% |
| #15 | c03088ac | 7,088 | 559 | 0 | 8% |
| #14 | 0043b089 | 688 | 119 | 0 | 17% |
| #10 | d539a606 | 3,883 | 274 | 0 | 7% |
| #13 | d5679e45 | 10,758 | 5,605 | 4,918 | 52% |
| #12 | 3fa224e8 | 5,337 | 303 | 0 | 6% |
| #11 | 5753ac82 | 10,980 | 332 | 0 | 3% |
| #9 | 4ed9b090 | 23,198 | 1,595 | 0 | 7% |
| #8 | e8227615 | 3,815 | 305 | 0 | 8% |
| #7 | c09918df | 2,301 | 305 | 0 | 13% |
| #6 | 37ea8f2a | 276,681 | 8,255 | 0 | 3% |
| #5 | c89f9d30 | 1 | 0 | 0 | 0% |
| #4 | c8e8d94c | 18,561 | 0 | 0 | 0% |
| #3 | b4c7bbff | 1 | 0 | 0 | 0% |
| #2 | a0f5ecb8 | 1 | 0 | 0 | 0% |
| #1 | 3ee646ee | 1 | 0 | 0 | 0% |
| root | ab972300 | 76 | 0 | 0 | 0% |

Totals: 770,460 of main's 1,839,658 body bytes (42%) are repeated lines; 633,305 (34%) without trailer lines. [C6]

Groups that open with the same text (same first 2,000 body bytes) [C5], and the mechanism from the pull refs [C8]:

| Group | Opens alike | Non-merge commits on each branch that main's parent did not hold | Of those, held by the pull request merged just before |
|---|---|---|---|
| #12, #13 | yes | 2, 5 | #13: 2 |
| #16, #17, #18, #19, #20 | yes | 3, 8, 12, 14, 16 | 3, 8, 12, 14 |
| #29, #30, #31, #32 | yes | 40, 82, 88, 89 | 40, 82, 88 |
| #27, #35 | no | 350, 603 | #35: 350 |
| #35, #36 | no | 603, 604 | #36: 603 |
| #1, #2, #3, #5 | empty bodies | 325, 45, 3, 5 | none |

The mechanism is the same in every non-empty group. The branch was cut from, or had merged, a branch whose pull request was squashed first. Main received that work as one new commit, so the original commits were still not on main, and the next squash listed them again. #32 added 1 commit to #31's 88, #31 added 6, #30 added 42, and #36 added 1 to #35's 603.

#35 and #36 do not open alike. Each list holds about the newest 250 commits of its branch, so the two start at different commits. Without trailer lines, #36 shares 299,217 bytes with #35, and #35 shares 106,843 with #27. [C7]

## 4. Totals on main

| Figure | Value | Cmd |
|---|---:|---|
| Message bytes (`--format=%B`) | 1,842,844 | C11 |
| Body bytes (`--format=%b`) | 1,839,658 | C11 |
| Bodies over 64 KiB | 8 commits, 1,651,006 bytes | C19 |
| `Co-Authored-By:` lines | 1,542 | C12 |
| `Co-authored-by:` lines | 39 | C12 |
| `Claude-Session:` lines | 1,543 | C12 |
| `Signed-off-by:` lines | 0 | C12 |
| Bytes in those trailer lines | 199,853 | C12 |
| Distinct session links | 15 | C12 |
| Distinct co-author names | 6 | C13 |
| Distinct co-author addresses | 1, a noreply address | C13 |

Co-author names and line counts: Claude Opus 5.5 (689), Claude Fable 5.2 (603), Claude Fable 5.1 (171), Claude (67), Claude Sonnet 5.5 (38), Claude Opus 5 (13). [C13]

## 5. Identities

| Scope | Role | Name | Commits | Address | Class |
|---|---|---|---:|---|---|
| Main | author | Bright Fulton | 31 | `brig[23]` | other |
| Main | author | Claude | 1 | `nore[21]` | noreply or bot |
| Main | committer | GitHub | 31 | `nore[18]` | noreply or bot |
| Main | committer | Claude | 1 | `nore[21]` | noreply or bot |
| Every ref | author | Claude | 1,958 | `nore[21]` | noreply or bot |
| Every ref | author | Bright Fulton | 56 | `brig[23]` | other |
| Every ref | author | github-actions[bot] | 9 | `4189[53]` | noreply or bot |
| Every ref | committer | Claude | 1,979 | `nore[21]` | noreply or bot |
| Every ref | committer | GitHub | 35 | `nore[18]` | noreply or bot |
| Every ref | committer | github-actions[bot] | 9 | `4189[53]` | noreply or bot |

Distinct addresses, authors and committers together: 3 on main (2 noreply or bot, 1 other) and 4 over every ref (3 noreply or bot, 1 other). The one other address is the owner's. No commit on any ref carries an identity other than the owner's or a bot's. Co-author trailers over every ref carry seven names (Claude and six model names) under one noreply address. [C14]

The owner's 56 commits are 35 that GitHub committed (31 on main, 4 on other branches) and 21 that Claude committed on branches. [C14]

Signature headers over every ref: 1,978 SSH (committer Claude), 35 PGP (committer GitHub), 10 none (the root commit and the nine `github-actions[bot]` commits on `bank/quick-*`). [C16]

## 6. The pull refs, and what a re-message would draw on

Columns 3 to 5 are from git [C15]. Columns 2, 6, 7 and 8 are from GitHub's pull request list [C17]. Description bytes are the UTF-8 bytes of the description.

| PR | state | commits not on main | of which merges | message bytes | title equals commit title | description bytes | session links in description |
|---|---|---:|---:|---:|---|---:|---:|
| #1 | merged to main | 329 | 4 | 482,186 | yes | 4,883 | 1 |
| #2 | merged to main | 45 | 0 | 64,911 | yes | 4,266 | 1 |
| #3 | merged to main | 3 | 0 | 2,307 | yes | 1,822 | 1 |
| #4 | merged to main | 16 | 0 | 18,471 | yes | 4,320 | 1 |
| #5 | merged to main | 5 | 0 | 6,560 | yes | 5,993 | 1 |
| #6 | merged to main | 691 | 15 | 893,133 | yes | 8,985 | 1 |
| #7 | merged to main | 2 | 0 | 2,239 | yes | 3,171 | 1 |
| #8 | merged to main | 2 | 0 | 3,753 | yes | 3,912 | 1 |
| #9 | merged to main | 12 | 0 | 23,116 | yes | 14,094 | 1 |
| #10 | merged to main | 4 | 0 | 3,817 | yes | 4,002 | 1 |
| #11 | merged to main | 5 | 0 | 10,912 | yes | 7,248 | 1 |
| #12 | merged to main | 3 | 1 | 5,600 | yes | 5,675 | 1 |
| #13 | merged to main | 7 | 2 | 11,325 | yes | 5,907 | 1 |
| #14 | merged to main | 1 | 0 | 748 | yes | 1,138 | 1 |
| #15 | merged to main | 5 | 1 | 7,063 | yes | 3,673 | 1 |
| #16 | merged to main | 3 | 0 | 3,439 | yes | 3,067 | 1 |
| #17 | merged to main | 9 | 1 | 8,665 | yes | 3,396 | 1 |
| #18 | merged to main | 14 | 2 | 11,996 | yes | 2,696 | 1 |
| #19 | merged to main | 17 | 3 | 14,353 | yes | 2,634 | 1 |
| #20 | merged to main | 20 | 4 | 16,813 | yes | 2,873 | 1 |
| #21 | merged to another branch | 177 | 6 | 200,761 | n/a | 4,126 | 1 |
| #22 | merged to another branch | 339 | 8 | 414,194 | n/a | 8,202 | 1 |
| #23 | merged to another branch | 344 | 9 | 430,235 | n/a | 8,190 | 1 |
| #24 | closed, not merged | 2 | 0 | 2,364 | n/a | 3,614 | 1 |
| #25 | merged to another branch | 384 | 12 | 469,776 | n/a | 5,329 | 0 |
| #26 | merged to another branch | 353 | 9 | 473,547 | n/a | 6,512 | 1 |
| #27 | merged to main | 361 | 11 | 481,459 | yes | 7,661 | 1 |
| #28 | merged to main | 3 | 0 | 1,793 | yes | 5,261 | 1 |
| #29 | merged to main | 40 | 0 | 42,055 | yes | 3,171 | 1 |
| #30 | merged to main | 83 | 1 | 88,938 | yes | 4,495 | 0 |
| #31 | merged to main | 90 | 2 | 94,060 | yes | 4,775 | 1 |
| #32 | merged to main | 92 | 3 | 95,329 | yes | 1,869 | 0 |
| #33 | merged to main | 49 | 2 | 71,662 | yes | 4,737 | 1 |
| #34 | merged to main | 3 | 0 | 1,696 | yes | 1,523 | 0 |
| #35 | merged to main | 630 | 27 | 875,932 | yes | 7,160 | 0 |
| #36 | merged to main | 632 | 28 | 877,414 | yes | 1,862 | 0 |
| #37 | merged to main | 10 | 0 | 3,327 | yes | 1,471 | 0 |

Pull refs, totals [C15]:

| Figure | Commits | Message bytes |
|---|---:|---:|
| Sum over the 37 refs (a commit counts once for each ref that holds it) | 4,785 | 6,215,949 |
| Distinct commits under the pull refs and not on main | 1,981 (62 merges) | 2,642,481 |
| On branches only, under no pull ref and not on main | 10 | |

The 10 are the nine `bank/quick-*` commits and the tip of `claude/wal-ahead-beside-wip`. Every commit of `claude/e-recode-auto` is under a pull ref. [C21]

Every body line of 40 or more bytes in a squash commit on main also occurs in a commit message under that pull request's ref, apart from one 47-byte line in 24 of the 31: the `Co-authored-by` line that ends the squash body and names the branch commits' author. [C10]

The pull refs' messages hold 18 distinct session links. [C15]

What a re-message would draw on [C17]:

- All 31 pull request titles equal the commit title without its ` (#n)` suffix.
- All 31 have a description. None is empty and none is under 500 bytes. Nine are over 5,000 bytes (#5, #6, #9, #11, #12, #13, #27, #28, #35).
- Description bytes: minimum 1,138 (#14), quartiles 2,696, 4,002 and 5,675, maximum 14,094 (#9), total 137,740. That total is 7.5% of main's 1,842,844 message bytes.
- 25 of the 31 descriptions contain a session link, and 30 end with a "Generated by Claude Code" footer. The six without a link are #30, #32, #34, #35, #36, #37.
- Two descriptions have no Markdown heading (#36, #37). The other 29 have between one and eight.

## 7. Signatures as GitHub reports them

The `verification.verified` field and its reason could not be read. The GitHub MCP tools return a trimmed commit object (`sha`, `html_url`, `commit.message`, `commit.author`, `commit.committer`, `author`, `committer`) from both `get_commit` and `list_commits`, with no `verification` member. The count of verified and unverified commits is therefore not measured. [C17]

What was read:

| Source | Finding | Cmd |
|---|---|---|
| GitHub, `list_commits` on main | 32 commits. The committer account is GitHub's `web-flow` on 31 and `claude` on the root. The author account is the owner's on 31. | C17 |
| Local `gpgsig` headers | 31 of 32, all PGP, all naming key id `B5690EEEBB952194`. The root has none. | C16 |
| Local `git log --format=%G?` | `E` for the 31 (no public key in the local keyring), `N` for the root. | C16 |

## 8. Title style

Shape: one line that states what changed, as a noun phrase or a plain sentence, often several clauses joined by commas, semicolons or ", and". GitHub appends ` (#n)` at the merge. Counts over the 31 squash titles [C18]:

| Property | Count |
|---|---:|
| Ends with ` (#n)` | 31 |
| Ends with a period | 0 |
| Starts with a capital letter | 29 |
| Starts with an area and a colon (`Docs:` 2, `Out of core:` 2, `bench:` 1, `Release prep:` 1) | 6 |
| Starts with an imperative verb (Replace, Size, Gate, Name) | 4 |
| Starts with The, A, An or Two | 10 |
| Contains a comma | 22 |
| Contains ", and" | 15 |
| Contains a semicolon | 5 |
| Over 72 bytes without the suffix | 22 |
| Over 100 bytes without the suffix | 11 |

Length in bytes (all titles are ASCII), nearest-rank quartiles:

| | Minimum | Q1 | Median | Q3 | Maximum |
|---|---:|---:|---:|---:|---:|
| With ` (#n)` | 56 | 77 | 96 | 116 | 184 |
| Without | 50 | 72 | 90 | 110 | 179 |

Examples:

- `Size a segment's head reserve without holding its records (#5)` (62)
- `The tests' crash ends the store's threads before it returns (#31)` (65)
- `Docs: the crash seam's entry, and what the last two pull requests left undone (#32)` (83)

## 9. Body sizes against a size rule

Body bytes over main's 32 commits, nearest rank [C19]:

| Minimum | P10 | Q1 | Median | Q3 | P90 | Maximum | Mean |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 1,857 | 8,379 | 42,193 | 276,681 | 345,260 | 57,489 |

Commits whose body is under N bytes [C19]:

| N | Pass | Which |
|---:|---:|---|
| 1,024 | 6 | root, #1, #2, #3, #5, #14 |
| 2,048 | 8 | those and #28, #34 |
| 4,096 | 13 | those and #7, #8, #10, #16, #37 |
| 8,192 | 15 | those and #12, #15 |

Four of the six that pass at 1,024 have no body at all. The same rule applied to the pull request descriptions: 0 of 31 are under 1,024 bytes, 6 under 2,048, 16 under 4,096, 29 under 8,192. [C17]

## Commands

Run from `/home/user/supdb`. `X` is defined under Terms.

**C1** Counts of commits.

```
git rev-list --count b3a043dc; git rev-list --count --merges b3a043dc
git log --first-parent --format=%s b3a043dc | grep -c '(#[0-9]*)$'
git rev-list --count $X
```

**C2** Per-commit rows: hash, date, pull request, title bytes, body bytes, `* ` lines, the four trailer counts, session links, `gpgsig`, shortstat.

```
for c in $(git rev-list --first-parent b3a043dc); do printf '%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s\n' "$(git show -s --abbrev=8 --date=short --format='%h|%ad' $c)" "$(git show -s --format=%s $c | grep -o '(#[0-9]*)$' | tr -d '(#)')" "$(git show -s --format=%s $c | tr -d '\n' | wc -c)" "$(git show -s --format=%b $c | wc -c)" "$(git show -s --format=%b $c | grep -c '^\* ')" "$(git show -s --format=%b $c | grep -c '^Co-Authored-By:')" "$(git show -s --format=%b $c | grep -c '^Co-authored-by:')" "$(git show -s --format=%b $c | grep -c '^Claude-Session:')" "$(git show -s --format=%b $c | grep -c '^Signed-off-by:')" "$(git show -s --format=%b $c | grep -o 'claude\.ai/code/session_[A-Za-z0-9]*' | sort -u | wc -l)" "$(git cat-file -p $c | sed '/^$/q' | grep -c '^gpgsig')" "$(git show --shortstat --format= $c | tr -d '\n')"; done
```

**C5** Groups by opening text: the same first field means the same first 2,000 body bytes.

```
for c in $(git rev-list --first-parent b3a043dc); do echo "$(git show -s --format=%b $c | head -c 2000 | sha1sum | cut -c1-10) $(git show -s --format=%s $c | grep -o '(#[0-9]*)$')"; done | sort
```

**C6** Repeated lines per commit, oldest first: hash, body bytes, repeated bytes, repeated bytes without trailer lines.

```
git log --first-parent --reverse --abbrev=8 --format='%x01%h%n%b' b3a043dc | LC_ALL=C awk 'function fl(){ if(c!=""){print c, tot, rep, repnt; for(l in cur) seen[l]=1}; split("",cur); tot=0; rep=0; repnt=0 } /^\001/{fl(); c=substr($0,2); next} {tot+=length($0)+1} length($0)>=40{ if($0 in seen){rep+=length($0)+1; if($0 !~ /^(Co-Authored-By|Co-authored-by|Claude-Session): /) repnt+=length($0)+1}; cur[$0]=1 } END{fl()}'
```

**C7** Bytes of commit A's non-trailer lines that commit B's body holds (A, B = `e40d6283`, `09cadec0`; then `09cadec0`, `7650d890`).

```
git show -s --format=%b e40d6283 | LC_ALL=C awk 'length($0)>=40' | grep -vE '^(Co-Authored-By|Co-authored-by|Claude-Session): ' | grep -Fxf <(git show -s --format=%b 09cadec0 | LC_ALL=C awk 'length($0)>=40' | sort -u) | wc -c
```

**C8** A branch's non-merge commits that main's parent did not hold, and those an earlier pull request's head did not hold either. The difference is the shared count. Shown for #36 against #35; substitute the commit and the two pull numbers.

```
git rev-list --count --no-merges e40d6283^..refs/remotes/pull/36
git rev-list --count --no-merges e40d6283^..refs/remotes/pull/36 ^refs/remotes/pull/35
```

**C9** `* ` lines that are a branch commit's title, beside the branch's non-merge commits and those among its newest 250. Shown for #27.

```
LC_ALL=C comm -12 <(git show -s --format=%b 7650d890 | grep '^\* ' | cut -c3- | LC_ALL=C sort) <(git log --no-merges --format=%s 7650d890^..refs/remotes/pull/27 | LC_ALL=C sort) | wc -l
git rev-list --count --no-merges 7650d890^..refs/remotes/pull/27
git rev-list 7650d890^..refs/remotes/pull/27 | head -250 | git log --no-walk=unsorted --stdin --no-merges --format=%h | wc -l
```

**C10** Bytes of squash body lines (40 bytes or more) that no commit message under the pull ref holds.

```
git log --first-parent --format='%H %s' b3a043dc | sed -nE 's/^([0-9a-f]{8})[0-9a-f]* .*\(#([0-9]+)\)$/\1 \2/p' | while read M n; do echo "#$n $(LC_ALL=C comm -23 <(git show -s --format=%b $M | LC_ALL=C awk 'length($0)>=40' | sed 's/^\* //' | LC_ALL=C sort -u) <(git log --format=%B $M^..refs/remotes/pull/$n | sed 's/^\* //' | LC_ALL=C sort -u) | wc -c)"; done
```

**C11** Message and body bytes on main.

```
git log --first-parent --format=%B b3a043dc | wc -c; git log --first-parent --format=%b b3a043dc | wc -c
```

**C12** Trailer lines and session links. Replace `--first-parent` with `--first-parent -7` for the seven newest, and `b3a043dc` with `8025423` for the plan's baseline.

```
git log --first-parent --format=%b b3a043dc | grep -c '^Co-Authored-By:'
git log --first-parent --format=%b b3a043dc | grep -c '^Co-authored-by:'
git log --first-parent --format=%b b3a043dc | grep -c '^Claude-Session:'
git log --first-parent --format=%b b3a043dc | grep -ci '^Signed-off-by:'
git log --first-parent --format=%b b3a043dc | grep -E '^(Co-Authored-By|Co-authored-by|Claude-Session): ' | wc -c
git log --first-parent --format=%b b3a043dc | grep -o 'claude\.ai/code/session_[A-Za-z0-9]*' | sort -u | wc -l
```

**C13** Co-author names, and the count of distinct co-author addresses.

```
git log --first-parent --format=%b b3a043dc | grep -i '^Co-authored-by:' | sed -E 's/^[^:]*: *//; s/ *<.*//' | sort | uniq -c
git log --first-parent --format=%b b3a043dc | grep -i '^Co-authored-by:' | grep -o '<[^>]*>' | sort -u | wc -l
```

**C14** Identities with the address redacted and classed. Use `%cn|%ce` for committers and `$X` in place of `--first-parent b3a043dc` for every ref.

```
git log --first-parent --format='%an|%ae' b3a043dc | sort | uniq -c | awk -F'|' '{e=$2; print $1 "|" substr(e,1,4) "[" length(e) "]|" (e ~ /noreply/ ? "noreply-or-bot":"other")}'
git log --format='%ae%n%ce' $X | sort -u | awk '{c=($0 ~ /noreply/)?"noreply-or-bot":"other"; n[c]++} END{for(k in n) print k, n[k]}'
git log --author='Bright Fulton' --format=%cn $X | sort | uniq -c
git log --format=%b $X | grep -i '^Co-authored-by:' | sed -E 's/^[^:]*: *//; s/ *<.*//' | sort | uniq -c
```

**C15** Pull refs: number, commits not on main, their message bytes, merges among them; then the distinct totals and the session links.

```
for n in $(git for-each-ref --format='%(refname:lstrip=3)' refs/remotes/pull | sort -n); do echo "$n $(git rev-list --count refs/remotes/pull/$n ^b3a043dc) $(git log --format=%B refs/remotes/pull/$n ^b3a043dc | wc -c) $(git rev-list --count --merges refs/remotes/pull/$n ^b3a043dc)"; done
git rev-list --count --glob='refs/remotes/pull/*' --not b3a043dc; git rev-list --count --merges --glob='refs/remotes/pull/*' --not b3a043dc
git log --format=%B --glob='refs/remotes/pull/*' --not b3a043dc | wc -c
git rev-list --count $X --not b3a043dc --glob='refs/remotes/pull/*'
git log --format=%B --glob='refs/remotes/pull/*' --not b3a043dc | grep -o 'claude\.ai/code/session_[A-Za-z0-9]*' | sort -u | wc -l
```

**C16** Signature headers by committer over every ref; the key id in main's signatures (set `GNUPGHOME` to an empty scratch directory); git's own check.

```
git rev-list $X | while read c; do echo "$(git show -s --format=%cn $c)|$(git cat-file -p $c | sed '/^$/q' | grep '^gpgsig' | cut -c8-37)"; done | sort | uniq -c
for c in $(git rev-list --first-parent b3a043dc); do git cat-file -p $c | sed '/^$/q' | sed -n '/^gpgsig /,/END PGP SIGNATURE/p' | sed 's/^gpgsig //;s/^ //' | gpg --list-packets 2>/dev/null | grep -o 'keyid [0-9A-F]*$'; done | sort | uniq -c
git log --first-parent --format='%G?' b3a043dc | sort | uniq -c
```

**C17** GitHub, read-only, through the GitHub MCP tools. These figures cannot be reproduced from the checkout alone.

```
mcp__github__list_pull_requests(owner="bfulton", repo="supdb", state="all", perPage=100, sort="created", direction="asc", fields=["number","title","body"])
mcp__github__list_pull_requests(... fields=["number","title","state","merged","base","commits","merged_at"])
mcp__github__list_commits(owner="bfulton", repo="supdb", sha="main", perPage=100, fields=["sha","author","committer"])
mcp__github__get_commit(owner="bfulton", repo="supdb", sha="ab972300", detail="none")
```

Description bytes and title matches, from the first call's saved JSON result (`$F`):

```
python3 -I -c "import json,sys;[print(p['number'],len(p['title']),len((p.get('body') or '').encode())) for p in sorted(json.load(open(sys.argv[1])),key=lambda p:p['number'])]" "$F"
git log --first-parent --format=%s b3a043dc | sed -nE 's/^(.*) \(#([0-9]+)\)$/\2\t\1/p' | sort -n
```

**C18** Title lengths and shapes. Add `| sed -E 's/ \(#[0-9]+\)$//'` after the first `grep` for lengths without the suffix; the shape counts are `grep -c` of the pattern named in the table over the same list.

```
git log --first-parent --format=%s b3a043dc | grep '(#[0-9]*)$' | LC_ALL=C awk '{print length($0)}' | sort -n | awk '{a[NR]=$1} function q(p,  i){i=int(p*NR); if(i<p*NR)i++; return a[i]} END{print NR,a[1],q(.25),q(.5),q(.75),a[NR]}'
git log --first-parent --format=%s b3a043dc | LC_ALL=C grep -c '[^ -~]'
```

**C19** Body-size quantiles and the pass counts.

```
for c in $(git rev-list --first-parent b3a043dc); do echo "$(git show -s --format=%b $c | wc -c) $(git show -s --format=%s $c | grep -o '#[0-9]*)$' | tr -d ')')"; done | sort -n | awk '{a[NR]=$1; s+=$1; for(N=1024;N<=8192;N*=2) if($1<N) u[N]++; if($1>65536){b++; bs+=$1}} function q(p,  i){i=int(p*NR); if(i<p*NR)i++; return a[i]} END{print a[1],q(.1),q(.25),q(.5),q(.75),q(.9),a[NR],int(s/NR); print u[1024],u[2048],u[4096],u[8192],b,bs}'
```

**C20** Banked rows in main's history, and `scratch/`.

```
git rev-list --objects b3a043dc | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' | awk '$1=="blob" && $3 ~ /^bench\/runs\//{s+=$2; k++} END{print k, s}'
git rev-list --objects 8025423 | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' | awk '$1=="blob" && $3 ~ /^bench\/runs\//{s+=$2; k++} END{print k, s}'
git rev-list --objects b3a043dc | awk '$2 ~ /(^|\/)scratch(\/|$)/' | wc -l
```

**C21** Leftover branches: commits ahead of main, commits under no pull ref, added files that exist on main.

```
for r in $(git for-each-ref --format='%(refname)' refs/remotes/origin | grep -v 'origin/main$\|supdb-pretag-coordination'); do echo "${r#refs/remotes/origin/} $(git show -s --format=%an $r) ahead=$(git rev-list --count b3a043dc..$r) no-pull-ref=$(git rev-list --count $r --not b3a043dc --glob='refs/remotes/pull/*') added=$(git diff --name-only --diff-filter=A $(git merge-base b3a043dc $r) $r | wc -l) added-on-main=$(git diff --name-only --diff-filter=A $(git merge-base b3a043dc $r) $r | while read f; do git cat-file -e b3a043dc:"$f" 2>/dev/null && echo y; done | wc -l)"; done
```
