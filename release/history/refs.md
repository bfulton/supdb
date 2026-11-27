## Refs, pull requests and CI: the freeze inventory

Measured at main = `b3a043dc` (tree `7f58d776`). Git figures come from the
fetched refs in the checkout. GitHub figures were read on 2026-11-27 after
20:20 UTC, when every workflow run had completed. Commands run from the
repository root. `R` is the table of all 506 workflow runs,
`history/figures/refs/runs_all.tsv` in the coordinator's scratch directory,
built from `actions_list list_workflow_runs perPage=100` pages 1 to 6
(columns: run id, workflow, event, status, conclusion, head branch, head
sha, created).

Three statements in the audit's brief no longer hold:

- The checkout's `HEAD` is `67404563`, two commits ahead of main, both
  adding files under `release/` only. Every command below names
  `origin/main`.
- Main is 32 commits: one initial commit and 31 squash commits, not 33
  (`git rev-list --count origin/main`;
  `git log --format=%s origin/main | grep -cE '\(#[0-9]+\)$'`).
- GitHub holds ten `bank/quick-*` branches. The tenth,
  `bank/quick-46057759452`, was pushed at 20:02 UTC and is not among the
  fetched refs (`list_branches`).

### The plan's figures, re-measured

| Plan figure (at `8025423`) | At `8025423` | At `b3a043dc` | Stale |
|---|---|---|---|
| Largest squash body 339 KB (#27) | 339,159 bytes | #35 is 345,381 bytes and #36 is 344,967; #27 is third; all 32 messages total 1,842,844 bytes | yes |
| #32 changes 120 lines in two files and carries 94 KB | 120 insertions, 1 deletion, 2 files; 94,579 bytes | same | no |
| Four of the seven newest open with the same text | #29 to #32 | two of the seven newest (#31, #32); three groups on all of main: #29 to #32, #16 to #20, #12 and #13 | yes |
| 599 `Co-Authored-By` and 599 `Claude-Session` lines, seven session links | 599, 599, 7 (seven newest) | 1,542 and 1,543 lines on all of main, 15 distinct session links; 790, 790 and 9 in the seven newest | yes |
| 51 MB of file versions under `bench/runs` | 51.0 MiB, 112 blobs | 64.0 MiB, 130 blobs, a third of the 193.5 MiB of blobs in main's history | yes |
| Nothing under `scratch/` on main | 0 paths | 0 paths, at the head and in history | no |
| Two `claude/*` leftovers, five unmerged `bank/quick-*` | | the same two leftovers; ten `bank/quick-*`, all unmerged | yes |

Two more facts about the messages. #36's body repeats #35's: 275 of its
276 bullet lines are in #35's body. Main's bodies hold 1,551 bullet lines,
894 of them distinct. GitHub also adds a lower-case `Co-authored-by:` line
to some squash bodies: 39 on main.

```sh
for c in $(git rev-list origin/main); do echo "$(git log -1 --format=%B $c | wc -c) $(git log -1 --format=%s $c | grep -oE '\(#[0-9]+\)$')"; done | sort -rn | head -5
git show --shortstat --format= 8025423b
for c in $(git rev-list origin/main); do b=$(git log -1 --format=%b $c | grep -m1 .); [ -n "$b" ] && echo "$(echo "$b" | md5sum | cut -c1-8) $(git log -1 --format=%s $c | grep -oE '#[0-9]+')"; done | sort | awk '{n[$1]++; m[$1]=m[$1]" "$2} END{for(k in n) if(n[k]>1) print n[k]":"m[k]}'
git log --format=%B origin/main | grep -c '^Co-Authored-By:'      # -7 8025423b for the plan's scope
git log --format=%B origin/main | grep -c '^Claude-Session:'
git log --format=%B origin/main | grep '^Claude-Session:' | sort -u | wc -l
git log --format=%B origin/main | grep -c '^Co-authored-by:'
git rev-list --objects origin/main | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' | awk '$1=="blob"{t+=$2; if($3 ~ /^bench\/runs\//){n++; r+=$2}} END{printf "%d blobs %.1f MiB of %.1f MiB\n", n, r/1048576, t/1048576}'
git log --format= --name-only origin/main -- scratch | sort -u | wc -l
comm -12 <(git log -1 --format=%b 09cadec0 | grep '^\* ' | sort -u) <(git log -1 --format=%b e40d6283 | grep '^\* ' | sort -u) | wc -l
git log --format=%b origin/main | grep '^\* ' | sort -u | wc -l    # without sort -u | wc: grep -c
```

### (a) The two `claude/*` branches

| Branch | Tip | Tip date (UTC) | Ahead / behind main | Merge-base | Against the pull heads | Verdict |
|---|---|---|---|---|---|---|
| `claude/e-recode-auto` | `c78fc75b` | 2026-11-18 13:30 | 236 / 11 | `5aad09dd` (#20) | the tip is an ancestor of `pull/27` (and of 22, 23, 25, 26, 35, 36); no commit of it is outside the pull heads | merged, through #27 |
| `claude/wal-ahead-beside-wip` | `99fb80e0` | 2026-11-15 01:40 | 183 / 11 | `5aad09dd` (#20) | its parent `ae0bcb34` is in `pull/27`; the tip commit is in no pull head | superseded |

`pull/27` is tree-equal to its squash `7650d890`, so main holds everything
`claude/e-recode-auto` holds. The branch carries nothing main lacks.

`claude/wal-ahead-beside-wip` adds one commit, "WIP: the log's pre-write
made beside the commit that buys it": `src/db.rs` +198 −7, `tests/db.rs`
+96, `bench/src/engines.rs` +47, `bench/DESIGN.md` +3, `bench/src/run.rs`
+2. The finished form of that work is `a0a90f3d`, "The log's pre-write
beside the commit that buys it, refuted as an arm" (7 files, +571 −8), in
`pull/27`'s history. Main has the option (`wal_ahead_beside`, 15
occurrences in `src/db.rs`) and the WIP's test. The branch carries no
unmerged work.

```sh
git rev-list --left-right --count origin/claude/e-recode-auto...origin/main
git for-each-ref --contains origin/claude/e-recode-auto --format='%(refname:short)' refs/remotes/pull
git rev-list --count origin/claude/e-recode-auto --not --remotes=pull          # 0
git diff --quiet 7650d890 refs/remotes/pull/27; echo $?                        # 0
git log --format='%h %s' origin/claude/wal-ahead-beside-wip --not --remotes=pull
git diff --numstat origin/claude/wal-ahead-beside-wip^ origin/claude/wal-ahead-beside-wip
git merge-base --is-ancestor a0a90f3d refs/remotes/pull/27; echo $?            # 0
git show --shortstat --format= a0a90f3d
git grep -c 'wal_ahead_beside' origin/main -- src/db.rs
git grep -c 'fn a_syncing_stores_pre_write_is_made_beside_the_commit_that_buys_it' origin/main -- tests/db.rs
```

### (b) The `bank/quick-*` branches

Each branch is one commit by `github-actions[bot]` on the main commit its
run measured. It adds three quick-scale rows under `bench/runs/quick/`
(1.9 to 2.0 MB) and redraws 291 to 363 files under `docs/figures/quick/`.
The branch name ends in the `bank-row` run id.

| Branch | Tip | Tip date (UTC) | `sha` in its rows | Row classes |
|---|---|---|---|---|
| `bank/quick-45487933794` | `6059bcb9` | 11-24 21:45 | `5b8d64ae` (#28) | ARM, M1, AMD EPYC 7763 |
| `bank/quick-45608444124` | `ee79ee10` | 11-25 12:20 | `dbc2a2a0` (#29) | ARM, M1, AMD EPYC 9V45 |
| `bank/quick-45897245375` | `d352eec7` | 11-26 21:44 | `5a5497be` (#30) | ARM, M1, Xeon 6975P-C |
| `bank/quick-45910250402` | `36dbfafe` | 11-26 23:43 | `3706d2bf` (#31) | ARM, M1, Xeon 6975P-C |
| `bank/quick-45913743471` | `2c177571` | 11-27 00:20 | `8025423b` (#32) | ARM, M1, Xeon 6975P-C |
| `bank/quick-45976421143` | `47e67b41` | 11-27 10:11 | `eceb66d6` (#33) | ARM, M2 Pro, Xeon 6975P-C |
| `bank/quick-45987642033` | `8876767e` | 11-27 11:32 | `99d44afa` (#34) | ARM, M1, AMD EPYC 9V45 |
| `bank/quick-46033466206` | `ba1e4ddd` | 11-27 17:04 | `09cadec0` (#35) | ARM, M1, Xeon 6975P-C |
| `bank/quick-46050828509` | `6942c5bb` | 11-27 19:04 | `e40d6283` (#36) | ARM, M2 Pro, AMD EPYC 9V74 |
| `bank/quick-46057759452` | `6df1f587` | 11-27 20:02 | `b3a043dc` (#37) | ARM, M1, Xeon 6975P-C |

Classes in full: ARM is `aarch64/unknown/4c/16g/hyper-v`; M1 is
`aarch64/apple-m1-virtual/3c/8g/hypervisor`; M2 Pro is
`aarch64/apple-m2-pro-virtual/5c/16g/hypervisor`; the x86 rows are
`x86_64/<cpu>/4c/16g/` with `hyper-v` (AMD) or `hypervisor` (Intel).

- Verdict for all ten: unmerged. None of the 30 row files is on main.
- No pull request has a bank branch as its head. The workflow tries to
  open one and falls back to the branch where Actions may not create pull
  requests.
- Main holds no row of a hosted class: its 86 quick rows are in four
  classes, all `x86_64` Xeon, 4 cores. Every bank commit message records
  the gate's line "0 prior rows (window 10)" and "ok: nothing worse than
  the window (0 flagged)" for each row.
- If all ten were banked the classes would hold: ARM 10 rows, M1 8, M2 Pro
  2, Xeon 6975P-C 6, EPYC 9V45 2, EPYC 7763 1, EPYC 9V74 1.
- The bot's commits carry no signature. `scripts/signed.sh`, in the
  `lists` group, refuses a commit after the base without one.
- `bank-row` has 13 runs: two on a throwaway branch, one failure on main
  at #27, and ten successes on main at #28 to #37, one branch each. Every
  further push to main adds a branch.

```sh
for r in $(git for-each-ref --format='%(refname:short)' 'refs/remotes/origin/bank/*'); do echo "$r $(git rev-parse --short=8 $r) $(git log -1 --format=%cI $r) $(git rev-list --left-right --count $r...origin/main)"; done
r=origin/bank/quick-46050828509; for f in $(git diff --name-only --diff-filter=A $(git merge-base $r origin/main) $r -- bench/runs); do git cat-file -e origin/main:$f 2>/dev/null && echo on-main || echo not-on-main; git show $r:$f | python3 -c "import json,sys,math;d=json.load(sys.stdin);m=d['machine'];print(d['sha'][:8],m['arch'],m['cpu_model'],m['cpus'],2**round(math.log2(m['mem_total_kb']/1048576)),m['virtualised'])"; done
git for-each-ref --contains origin/bank/quick-46050828509 --format='%(refname:short)' refs/remotes/pull     # empty
r=origin/bank/quick-46050828509; git ls-tree -r -l $r -- bench/runs/quick | grep e40d628 | awk '{s+=$4} END{print s}'; git diff --name-only --no-renames $(git merge-base $r origin/main) $r -- docs/figures | wc -l
git ls-tree -r --name-only origin/main -- bench/runs/quick | wc -l                                              # 86
git log -1 --format=%B origin/bank/quick-46050828509 | grep -E 'prior rows|^ok:'
for r in $(git for-each-ref --format='%(refname)' 'refs/remotes/origin/bank/*'); do git cat-file commit $r | sed '/^$/q' | grep -c '^gpgsig'; done
awk -F'\t' '$2=="bank-row"{print $6, $5}' "$R" | sort | uniq -c
# the tenth branch: get_commit 6df1f587 (detail none); get_file_contents bench/runs/quick/ at refs/heads/bank/quick-46057759452
```

### (c) Pull requests 1 to 37

Every number from 1 to 37 is a pull request. None is open. For all 37 the
head GitHub reports equals `refs/remotes/pull/<n>`. No head branch still
exists: the branch list holds none of the 26 head ref names. "Commits" is
the count reachable from the head and not from main.

| # | Title | State | Head ref | Head (GitHub = `refs/remotes/pull/<n>`) | Commits | Squash on main | Squash tree vs head |
|---|---|---|---|---|---|---|---|
| 1 | Supdb 0.1.0: the engine, the falsification suite, and the… | merged | `claude/supdb-architecture-review-6mkcis` | 30c355e6 | 329 | 3ee646ee | equal |
| 2 | Release prep: manifest, license, gates, and the read advi… | merged | `claude/supdb-architecture-review-6mkcis` | 3b4f2b2b | 45 | a0f5ecb8 | equal |
| 3 | Gate each ordering on what its own statement claims | merged | `claude/supdb-architecture-review-6mkcis` | 9af0b0d7 | 3 | b4c7bbff | equal |
| 4 | Replace the falsification suite with the time-series suit… | merged | `claude/supdb-architecture-review-6mkcis` | dfb83730 | 16 | c8e8d94c | equal |
| 5 | Size a segment's head reserve without holding its records | merged | `claude/supdb-architecture-review-6mkcis` | e80fd9d3 | 5 | c89f9d30 | equal |
| 6 | The engine since 0.1.0: reader threads, ordered ingest, t… | merged | `claude/recover-consolidate-branches-5rwvr5` | 5bf1f886 | 691 | 37ea8f2a | equal |
| 7 | The bounded roster's row under 1.25 GiB, and the suite's… | merged | `claude/ooc-bounded-row` | 57f88100 | 2 | c09918df | equal |
| 8 | The compact form's controller costs a read nothing in mem… | merged | `claude/auto-in-core` | d1f5f971 | 2 | e8227615 | equal |
| 9 | The bounded lag points: no piece ranked whole, cuts and m… | merged | `claude/lag-under-bound` | 9cfe291f | 12 | 4ed9b090 | equal |
| 10 | bench: a killed pass is a cell, a cut run resumes from it… | merged | `claude/bench-resumable-runs` | 0f347f29 | 4 | d539a606 | equal |
| 11 | The buffered lag bursts: a WAL rotation leaves its frames… | merged | `claude/wal-rotation-off-the-writer` | 14d44ef0 | 5 | 5753ac82 | equal |
| 12 | Out of core: the builder ahead starts in core alone; a bu… | merged | `claude/ooc-fault-asks` | e231c3d4 | 3 | 3fa224e8 | equal |
| 13 | Out of core: builds ahead at landing wait while scans are… | merged | `claude/ooc-merge-debt` | 31510a77 | 7 | d5679e45 | equal |
| 14 | Name PR #6's history where the docs named branches | merged | `claude/doc-pr-refs` | c7da29d2 | 1 | 0043b089 | equal |
| 15 | Merges keep each input's key across their walks; two refu… | merged | `claude/ooc-full-rewrite` | 3e60d434 | 5 | c03088ac | differs by #14 |
| 16 | Merges in flight beside each other, as an option (priced… | merged | `claude/parallel-merges` | 0b544e25 | 3 | 69b639a3 | equal |
| 17 | Forms fitted while the store is short of memory (on), and… | merged | `claude/e-out-of-core` | ef574338 | 9 | 2a667959 | equal |
| 18 | WAL frames dropped from the page cache once synced (on);… | merged | `claude/memory-model` | 6387059e | 14 | 55d07a98 | equal |
| 19 | Two of the controller's builds ahead at once (on): the co… | merged | `claude/build-ahead` | aaa840f1 | 17 | c614a23e | equal |
| 20 | The compact form installs at the load's end (on): residen… | merged | `claude/c-early-install` | 16ba2e05 | 20 | 5aad09dd | equal |
| 21 | The open after a kill: a run's file finished where it lie… | merged into `claude/e-recode-auto` | `claude/crash-fixes` | 8d1d68b8 | 177 | — | — |
| 22 | FASTER as a comparator: the faster arm, a guarantee of it… | merged into `claude/e-recode-stride-selector-g5f6hl` | `claude/faster-peer` | 2f73388a | 339 | — | — |
| 23 | Pin the toolchain, lint the macOS target locally, unlock… | merged into `claude/e-recode-stride-selector-g5f6hl` | `claude/toolchain-pin-and-lock` | 331bc809 | 344 | — | — |
| 24 | CI caches the suite's target: the rust-cache path relativ… | closed, unmerged | `claude/funny-faraday-1ipfti` | d3084dd0 | 2 | — | — |
| 25 | CI set-up: libclang loaded at run time, one cache for the… | merged into `claude/e-recode-stride-selector-g5f6hl` | `claude/funny-faraday-1ipfti` | 38de9e8d | 384 | — | — |
| 26 | One build for every engine in the suite: the comparators'… | merged into `claude/e-recode-stride-selector-g5f6hl` | `claude/comparator-build-flags` | 06d2beca | 353 | — | — |
| 27 | Out-of-core work since #20: the stride form, hot partitio… | merged | `claude/e-recode-stride-selector-g5f6hl` | 7fda1f94 | 361 | 7650d890 | equal |
| 28 | zpages tests hold their clock while a builder job runs; w… | merged | `claude/zpages-waits-checkout` | 2ca6ea91 | 3 | 5b8d64ae | equal |
| 29 | Point gets and puts beside FASTER, and a log held in the… | merged | `claude/beat-faster-g5f6hl` | 21d5f91f | 40 | dbc2a2a0 | equal |
| 30 | Handles that write under the held log, bursts lent to the… | merged | `claude/beat-faster-g5f6hl` | 9b01c531 | 83 | 5a5497be | equal |
| 31 | The tests' crash ends the store's threads before it retur… | merged | `claude/beat-faster-g5f6hl` | 22ad0fd8 | 90 | 3706d2bf | equal |
| 32 | Docs: the crash seam's entry, and what the last two pull… | merged | `claude/beat-faster-g5f6hl` | d50531d2 | 92 | 8025423b | equal |
| 33 | Drains answered at the readable landing, packed copies th… | merged | `claude/e-recode-stride-selector-g5f6hl` | e2f97b71 | 49 | eceb66d6 | equal |
| 34 | The bounded row under 1.25 GiB at 5fea7c0, and two roadma… | merged | `claude/e-recode-stride-selector-g5f6hl` | 0b2699d5 | 3 | 99d44afa | equal |
| 35 | Encryption under a caller's cipher or the kernel's key, a… | merged | `claude/inspiring-brahmagupta-d7y8n7` | 0e6ec308 | 630 | 09cadec0 | equal |
| 36 | The cost job by hand, and one build ahead at a time where… | merged | `claude/inspiring-brahmagupta-d7y8n7` | ef2cbbfd | 632 | e40d6283 | equal |
| 37 | Docs: CLAUDE.md as rules, the roadmap as short items by p… | merged | `claude/fervent-babbage-4qzwll` | 7d91b1b6 | 10 | b3a043dc | equal |

- Main's 31 subjects equal the pull request's title followed by `(#n)`,
  all 31.
- #15: the squash `c03088ac` differs from `pull/15` by exactly #14's
  change (the same patch-id). #14 merged on the same base 37 seconds
  earlier.
- #21 to #26 did not target main. #21 was merged into
  `claude/e-recode-auto` by merge commits, and its head is an ancestor of
  `pull/27`. #22, #23, #25 and #26 were squash-merged into
  `claude/e-recode-stride-selector-g5f6hl` as `94356bc3`, `d57f8c4d`,
  `888ba371` and `c5e4d35a`, each with the patch-id of its head's change
  and each in `pull/27`'s history. All five are on main through #27.
- #24 is the one pull request closed unmerged. It held two commits to
  `.github/workflows/ci.yml` and `apple-silicon.yml` (+36 −7). Both are
  ancestors of `pull/25`, opened from the same branch, so the work is on
  main through #25 and #27.

```sh
n=15; git rev-parse --short=8 refs/remotes/pull/$n; sq=$(git log --format='%h %s' origin/main | grep -E "\(#$n\)$" | cut -d' ' -f1); git diff --quiet $sq refs/remotes/pull/$n; echo "$sq $?"; git rev-list --count refs/remotes/pull/$n --not origin/main
git diff refs/remotes/pull/15 c03088ac | git patch-id --stable; git diff d539a606 0043b089 | git patch-id --stable
git merge-base --is-ancestor refs/remotes/pull/21 refs/remotes/pull/27; echo $?
sq=94356bc3; n=22; git diff $(git merge-base $sq^ refs/remotes/pull/$n) refs/remotes/pull/$n | git patch-id --stable; git diff $sq^ $sq | git patch-id --stable
git merge-base --is-ancestor refs/remotes/pull/24 refs/remotes/pull/25; echo $?; git diff --stat 5aad09dd refs/remotes/pull/24
# titles, states, bases, heads: list_pull_requests state=all (37 entries)
```

### (d) Issues, tags, releases

None of each. The repository has no issue, open or closed
(`list_issues`: total 0), no tag (`list_tags`, `git tag | wc -l`) and no
release (`list_releases`).

### (e) CI

`ci` has nine legs. `ef2cbbfd` and `e40d6283` have the same tree, as do
`7d91b1b6` and `b3a043dc`. #37 changes four documentation files and no
code.

| Leg | PR 36 head `ef2cbbfd`, run 46040597250 | main `e40d6283`, push, run 46050828621 | PR 37 head `7d91b1b6`, run 46057692113 | main `b3a043dc`, push, run 46057759527 |
|---|---|---|---|---|
| test (ubuntu-24.04) | success | success | success | success |
| test (ubuntu-24.04-arm) | success | failure | failure | success |
| test (macos-latest) | success | failure | success | success |
| quick | success | success | success | success |
| lists | success | success | success | success |
| cross-arm | success | success | success | success |
| enc | success | success | success | success |
| fscrypt | success | success | success | success |
| wasm | success | success | success | success |
| Run | success | failure | failure | success |

Other runs at these commits: `pr-caches` (job `delete`) succeeded at the
close of #36 and of #37; `bank-row` succeeded at `e40d6283` and at
`b3a043dc`. `apple-silicon` did not run for #36 or #37; its paths did not
change. #37 was merged 28 seconds after its `ci` run began.

The three failed legs, each one test in a `--no-fail-fast` run:

| Run | Leg | Test | Message |
|---|---|---|---|
| main `e40d6283` | macOS | `tests/settle.rs`, `a_copys_identity_outlives_a_merge_that_kept_its_partitions_keys` | `settle.rs:2211`, "the copies carried across: 0 claimed, 0 of their partitions" |
| main `e40d6283` | ARM | `tests/durable.rs`, `a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames` | `durable.rs:94`, "the closed run's link: the case was not reached" |
| PR 37 head | ARM | the same `durable` test | `durable.rs:13918`, `NotFound` |

The same `durable` test also failed on ARM at PR 35's head (run
46032017759). That is three failures in five hosted ARM runs: that run
and the table's four. Both tests are in the tree at main's head.

**What CONTRIBUTING.md says and what runs.** `CONTRIBUTING.md` line 47
says the gate does not run on pushes to main. `ci.yml` lines 10 to 14
trigger on a push to main, on any tag, and on pull requests. `ci` has run
on the push of each of main's 31 squash commits: 26 succeeded and five
failed (#28, #30, #31, #35, #36). One of the two files is wrong.

**Evidence that the tree at main's head is green.**

- Every leg has one green run on the head commit itself: push run
  46057759527, nine of nine.
- Over the four runs of the last two trees: seven legs are green four of
  four; macOS is green three of four; ARM is green two of four.
- Missing: a second green ARM run, or a fix for the `durable` test.
- Missing: a cost count. `cost` left `ci` in #36 and the `cost` workflow
  has never run. Its last two verdicts, as a `ci` job at PR 35's head and
  at main `09cadec0`, were "cost: RED" (hot lines reached by no probe).
- Missing: any run of `ooc-hosted`, `comparators` or `rocksdb-alloc` at a
  commit on main, and any run on the self-hosted runner since September.
- A tag push starts `ci` (`tags: ['*']`) and does not start `bank-row`.

Failures of `ci` on pushes to main and on final pull request heads since
#28, for the flake budget:

| Run | Leg | What failed |
|---|---|---|
| main #28 `5b8d64ae` | ubuntu | `tests/zpages.rs`, `a_spare_started_as_a_settle_ends_is_not_the_settles_to_sweep` |
| main #30 `5a5497be` | ubuntu | `tests/runlog.rs` binary killed by SIGBUS |
| main #31 `3706d2bf` | ubuntu | `tests/db.rs`, `a_commit_that_freezes_leaves_the_carry_to_the_upkeep_thread` |
| PR 34 head `0b2699d5` | ubuntu | `tests/zpages.rs`, `a_merges_outputs_are_built_in_the_block_form_where_the_room_is_short` |
| PR 35 head `0e6ec308` | ARM; cost | the `durable` test above; cost: RED |
| main #35 `09cadec0` | ubuntu; cost | `tests/settle.rs`, `a_copy_of_fixed_size_records_is_one_stream_until_a_write_it_cannot_take`; cost: RED |
| main #36 `e40d6283` | macOS; ARM | the two tests above |
| PR 37 head `7d91b1b6` | ARM | the `durable` test above |

Whether a later pull request fixed each of the first six is not
established here. #34, #35 and #37 were merged with a failed or unfinished
`ci` run.

**Workflows that run by hand, on a schedule or after a merge.**

| Workflow | Triggers at main | Runs | Last run | Commit | Note |
|---|---|---|---|---|---|
| `cost` | by hand | 0 | none | | never run as a workflow |
| `comparators` | by hand; Mondays 06:17 UTC | 7 | by hand, 2026-11-24, success | `445d5d5b`, in no ref now | no scheduled run is recorded |
| `ooc-hosted` | by hand | 2 | push, 2026-11-24, success | `b0953fa9`, a throwaway commit in `pull/25` | never run by hand |
| `rocksdb-alloc` | by hand; Mondays 07:41 UTC | 2 | push, 2026-11-24, success | `b0953fa9` | no scheduled or by-hand run |
| `apple-silicon` | pull requests touching four paths; by hand | 115 | pull request, 2026-11-27 16:15, success | `0e6ec308` (PR 35 head) | last by-hand run 2026-08-29, cancelled |
| `quiet-bench` | by hand, self-hosted runner | 33 | 2026-09-10, failure | `d48a3f44`, in `pull/6` | last success 2026-09-02 |
| `runner-smoke` | by hand, self-hosted runner | 7 | 2026-08-24, cancelled | `b3ffd258`, in `pull/1` | last success the same day |
| `bank-row` | push to main; by hand | 13 | push, 2026-11-27 19:35, success | `b3a043dc` | left `bank/quick-46057759452` |
| `pr-caches` | pull request closed | 13 | 2026-11-27 19:35, success | `7d91b1b6` (PR 37) | |

GitHub also lists two workflows whose files were never on main,
`zz-caches` and `zz-probe` (four runs each, 2026-11-23 and 24, from
throwaway commits in `pull/25`), and 68 runs of Copilot's code review.

```sh
git diff --quiet e40d6283 refs/remotes/pull/36; echo $?; git diff --quiet b3a043dc refs/remotes/pull/37; echo $?; git diff --numstat e40d6283 b3a043dc
git show origin/main:CONTRIBUTING.md | sed -n '47,49p'; git show origin/main:.github/workflows/ci.yml | sed -n '10,15p'
awk -F'\t' '$7 ~ /^(b3a043dc|e40d6283|ef2cbbfd|7d91b1b6)/ {print $1,$2,$3,$5,substr($7,1,8)}' "$R"
awk -F'\t' '$2=="ci" && $3=="push" && $6=="main"{print $5, substr($7,1,8)}' "$R"       # 32 runs; one is at 91436d52, not on main
for w in cost comparators ooc-hosted rocksdb-alloc apple-silicon quiet-bench runner-smoke bank-row pr-caches; do echo "$w $(awk -F'\t' -v w=$w '$2==w' "$R" | wc -l) $(awk -F'\t' -v w=$w '$2==w' "$R" | sort -t$'\t' -k8,8r | head -1 | cut -f3,5,7,8)"; done
git grep -c 'fn a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames' origin/main -- tests/durable.rs
# legs: actions_list list_workflow_jobs 46057759527 and 46050828621; pull_request_read get_check_runs 36 and 37
# failed tests: get_job_logs run_id=<run> failed_only=true return_content=true, searched for '... FAILED', 'panicked at', 'signal:'
```

### (f) In flight

- Open pull requests: none (`list_pull_requests state=open`).
- Workflow runs in progress: none; all 506 are completed.
- Branches whose tips date from the last three days: the ten
  `bank/quick-*` branches and this audit's coordination branch. The two
  `claude/*` leftovers are older. The tools give commit dates, not push
  times.
- Bank-row proposals awaiting a decision: ten branches, 30 rows.
- `release/STATUS.md` names one other session in flight. No branch of it
  is on GitHub.

### (g) Settings

Read directly: `main` is protected; the other 13 branches are not
(`list_branches`).

Observed, not read from settings:

- Main has no merge commit. Its 31 pull request commits are committed by
  GitHub, carry a signature header and end in `(#n)`. That is squash
  merging in practice.
- 26 bodies open with a bullet list of the branch's commit messages. Four
  are title only (#1, #2, #3, #5). #14, a one-commit pull request, carries
  its commit's message. The bullet list is the form of GitHub's "pull
  request title and commit details" default.
- A green `ci` run is not required of the merging account: #34, #35 and
  #37 merged without one.
- No bank branch became a pull request in ten successful `bank-row` runs.
- Every merged pull request's head branch is gone.

These tools cannot read: the default branch setting, which merge methods
are allowed, the default squash message, automatic deletion of head
branches, the content of main's protection (required checks, required
signatures, force-push and deletion rules, who may bypass), whether
Actions may create pull requests, caches, secrets and variables, and push
times.

```sh
git rev-list --merges --count origin/main
git log --format='%cn' origin/main | sort | uniq -c
for c in $(git rev-list origin/main); do git cat-file commit $c | sed '/^$/q' | grep -c '^gpgsig'; done | grep -c 1      # 31
for c in $(git rev-list origin/main); do git log -1 --format=%b $c | grep -m1 . | grep -c '^\* '; done | grep -c 1         # 26
```

### Facts that bear on the history decision

- A force-push does not remove commits from GitHub. `91436d52` was main's
  head on 2026-08-22, is in no ref now, and GitHub still serves it by hash
  (`get_commit`). Of the 282 distinct commits that workflow runs name, 59
  are in no fetched ref.
- The repository holds 2,024 commits across all refs: 32 on main and
  2,012 reachable from the pull heads.
- Of main's 130 banked rows, two record a commit on main (both
  `5b8d64ae`), 121 record a commit reachable only from a pull head, and
  seven record a commit in no ref. The 30 proposed bank rows all record
  commits on main, and their file names carry the short hash.
- Main's own hashes are cited in tracked text outside the rows and
  figures: `scripts/raises.txt` on 62 lines (in the reason column, which
  the check does not match), `ROADMAP.md` five times, `docs/engine.md`
  three, `tests/data/cost-entries.txt` twice, `CLAUDE.md` once.
- Main's squash commits are signed by GitHub. Commits made again with new
  messages would be signed by whoever makes them, and `scripts/signed.sh`
  wants a signature on each.
- A fresh repository has no pull refs. The 121 rows, and every document
  that cites a commit by `refs/pull/<n>/head`, would name commits it does
  not hold.
- The author address on main's 31 squash commits is one personal address
  (`brig`[23]); the committer is GitHub's no-reply address.

```sh
git rev-list --count --all; git rev-list --count --remotes=pull
git ls-tree -r --name-only origin/main -- bench/runs | wc -l
m=$(git rev-list origin/main); for f in $(git ls-tree -r --name-only origin/main -- bench/runs); do s=$(git show origin/main:$f | python3 -c "import json,sys;print(json.load(sys.stdin)['sha'])"); echo "$m" | grep -q "^$s" && echo on-main || { git cat-file -e "$s^{commit}" 2>/dev/null && echo pull-only || echo absent; }; done | sort | uniq -c
for c in $(git rev-list origin/main); do s=$(echo $c | cut -c1-7); git grep -c "$s" origin/main -- . ':!bench/runs' ':!docs/figures'; done
cut -f7 "$R" | sort -u | wc -l; cut -f7 "$R" | sort -u | while read c; do git cat-file -e "$c^{commit}" 2>/dev/null || echo absent; done | wc -l
```

### State of the freeze

Nothing waits to be landed: no pull request is open and no run is in
progress. Before a release-candidate branch is cut from main, the owner
decides or the coordinator parks the following.

1. **The ten bank-row branches.** Bank the 30 rows or delete the
   branches. Banking needs a person to open the pull requests, and the
   bot's unsigned commits fail `lists` as they stand. Decide also whether
   `bank-row` keeps running on each push to main during the freeze.
2. **The two `claude/*` leftovers.** Neither carries unmerged work.
   `claude/e-recode-auto` is merged through #27;
   `claude/wal-ahead-beside-wip` is superseded by `a0a90f3d`. Both can be
   deleted; their commits stay under the pull heads, except the one WIP
   commit `99fb80e0`.
3. **The ARM leg.** `a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames`
   failed in three of five hosted ARM runs today, and a `settle` test
   failed once on macOS at the last code commit. Fix them, or accept the
   head's single green run as the baseline.
4. **`CONTRIBUTING.md` against `ci.yml`.** The gate runs on pushes to
   main; the document says it does not. Change one.
5. **The cost count.** It has no verdict at `e40d6283` or `b3a043dc`, and
   its last two verdicts were red.
6. **The history decision.** The facts above are its inputs. A re-message
   rewrites 32 hashes, two rows' `sha` fields and 73 citing lines in
   five tracked files, and changes who signs main. It does not remove the old
   commits from GitHub.
7. **The gate of record for the tag.** A tag push runs the nine `ci` legs
   and nothing else.
8. **Housekeeping on GitHub.** `zz-caches` and `zz-probe` are still
   listed as workflows; no file on main defines them.
