# Review of `refs.md`

Checked at main = `b3a043dc` (tree `7f58d776`). Every git command runs from
the repository root. Commands marked API are GitHub MCP calls for
`bfulton/supdb`, read on 2026-11-27. `R` is the section's own table of
workflow runs, `history/figures/refs/runs_all.tsv` in the coordinator's
scratch directory; it is not in the repository.

Verdict: the inventory holds. 33 checks: 28 agree, 5 rows need a
correction. Seven corrections in all (C1 to C7); C6 and C7 concern
statements outside the table. Three are in the list of facts for the
history decision: the rows by where their commit lives (C1), the count of
commits across all refs (C2), and the count of citing lines (C3).

## Checks

Where the section used git, "mine" comes from other plumbing or from the
API; where it used the API, from git or from another API call.

| # | claim | theirs | mine | command | agree |
|---|---|---|---|---|---|
| 1 | main's commits | 32: one root, 31 squash, no merge | 32 commits, 1 root, 0 merges; the API lists 31 pull requests merged into `main` (#1 to #20, #27 to #37) | K1; API `list_pull_requests state=all fields=number,merged,base` | yes |
| 2 | branches on GitHub | 14; ten `bank/quick-*`; the tenth is `bank/quick-46057759452`; `main` alone is protected | 14 branches; ten `bank/quick-*`; `main` protected, 13 not | API `list_branches perPage=100` | yes |
| 3 | the tenth bank branch | tip `6df1f587`, 20:02 UTC, rows of `b3a043dc`, ARM, M1 and Xeon 6975P-C | tip `6df1f587`, committed 2026-11-27 20:02 UTC by the Actions bot; three rows named `-b3a043d.json` for those three classes | API `get_commit sha=6df1f587 detail=stats` | yes |
| 4 | pull requests | 37, none open; each head equals `refs/remotes/pull/<n>` | 37, all closed; 37 of 37 heads equal the local refs | API `list_pull_requests state=all fields=number,state,head`; K2 | yes |
| 5 | head branches | 26 names, none still exists | 26 distinct head names; none is among the 14 branches | the two API calls above | yes |
| 6 | commits per pull request | 329 for #1 ... 10 for #37 | all 37 equal; e.g. #1 329, #6 691, #27 361, #35 630, #36 632, #37 10 | K3 | yes |
| 7 | squash tree against head | equal for 30 of 31; #15 differs | 30 equal; #15 differs | K4 | yes |
| 8 | #15 differs by #14's change | same patch-id | the two diffs have the same per-file line counts (`CLAUDE.md` +7 −4, `docs/zpages-design.md` +4 −3) | K5 | yes |
| 9 | #14 merged 37 seconds before #15 | 37 s | `merged_at` 22:21:33Z and 22:22:10Z on 2026-11-04 | API `list_pull_requests fields=number,merged_at` | yes |
| 10 | #21 to #26 | #21 into `claude/e-recode-auto`; #22, #23, #25, #26 squashed into `claude/e-recode-stride-selector-g5f6hl` as `94356bc3`, `d57f8c4d`, `888ba371`, `c5e4d35a`; #24 closed unmerged | bases and merge states as stated (API). The four commits are single-parent, carry `(#22)`, `(#23)`, `(#25)`, `(#26)`, lie in `pull/27`, and each changes the same lines per file as its head's change. #24's two commits are in `pull/25`. | API `list_pull_requests fields=number,merged,base`; K6 | yes |
| 11 | `claude/e-recode-auto` | tip `c78fc75b`, 236 ahead, 11 behind, merge-base `5aad09dd`; in `pull/27`; nothing outside the pull heads | the same; in pulls 22, 23, 25, 26, 27, 35, 36; 0 commits outside | K7 | yes |
| 12 | `claude/wal-ahead-beside-wip` | tip `99fb80e0`, 183 / 11; one commit outside the pull heads; superseded by `a0a90f3d` | the same; the one commit is the tip; its parent `ae0bcb34` and `a0a90f3d` (7 files, +571 −8) are in `pull/27` | K7 | yes |
| 13 | nine fetched bank branches | each one bot commit on #28 to #36, three rows, rows record the parent, unsigned | nine tips equal the API's; each has one parent, a main commit #28 to #36; 3 rows added, all 3 named for the parent; no `gpgsig`; row bytes 1.90 to 2.04 MB; 291 to 363 figure files changed | K8; API `list_branches` | yes |
| 14 | classes if all ten were banked | ARM 10, M1 8, M2 Pro 2, Xeon 6975P-C 6, EPYC 9V45 2, 7763 1, 9V74 1 | nine fetched: ARM 9, M1 7, M2 Pro 2, Xeon 6975P-C 5, 9V45 2, 7763 1, 9V74 1; the tenth adds one each of ARM, M1 and Xeon 6975P-C | K9; check 3 | yes |
| 15 | main's quick rows | 86, four classes, all x86_64 Xeon, 4 cores | 86; four Xeon models (30, 29, 16, 11 rows) | K10 | yes |
| 16 | workflow runs | 506, all completed | `total_count` 506; `R` has 506 distinct run ids, all `completed` | API `actions_list list_workflow_runs perPage=1`; `cut -f1 "$R" \| sort -u \| wc -l; cut -f4 "$R" \| sort \| uniq -c` | yes |
| 17 | runs by workflow | cost 0, comparators 7, ooc-hosted 2, rocksdb-alloc 2, apple-silicon 115, quiet-bench 33, runner-smoke 7, bank-row 13, pr-caches 13, `zz-*` 4 each, Copilot review 68 | the same, and `ci` 238 | `cut -f2 "$R" \| sort \| uniq -c` | yes |
| 18 | `ci` on pushes to main | 31 squash commits, 26 success, 5 failure (#28, #30, #31, #35, #36); a 32nd run at `91436d52` | 26, 5, the same five; `91436d52` is no object here | K11 | yes |
| 19 | `bank-row` runs | 13: two on a throwaway branch, one failure at #27, ten successes at #28 to #37 | the same | `awk -F'\t' '$2=="bank-row" {print $6, $5}' "$R" \| sort \| uniq -c` | yes |
| 20 | legs of the `ci` run at PR 37's head | ARM failure, eight success | 9 jobs: `test (ubuntu-24.04-arm)` failure, the other eight success | API `actions_list list_workflow_jobs resource_id=46057692113` (the section used check runs) | yes |
| 21 | #37 merged 28 seconds after its `ci` run began | 28 s | run created 19:34:48Z, `merged_at` 19:35:16Z | the two API calls of checks 9 and 20 | yes |
| 22 | failed tests in the two tables | three legs and eight rows | the same test names, files and lines in the saved job logs; `cost: RED` in the two runs named | `grep -o -E 'test [a-z_0-9]+ \.\.\. FAILED\|signal: [0-9]+, SIG[A-Z]+\|cost: RED' history/figures/refs/logs_*.json \| sort \| uniq -c` | yes |
| 23 | hosted ARM failures | "three of five hosted ARM runs today" | 20 `ci` runs were created on 2026-11-27 UTC. From PR 35's head on there are six; the ARM leg failed in three. The run left out is main #35 (`09cadec0`), whose failed jobs were `cost` and `test (ubuntu-24.04)`. | K12 | no, see C4 |
| 24 | commits named by runs | 282 distinct, 59 in no fetched ref | 282; 59 not reachable from any local ref | `cut -f7 "$R" \| sort -u \| wc -l; cut -f7 "$R" \| sort -u \| comm -23 - <(git rev-list --all \| sort) \| wc -l` | yes |
| 25 | issues, tags, releases | none | no tag, no release, 0 issues; no local tag ref | API `list_tags`, `list_releases`, `list_issues`; `git for-each-ref refs/tags \| wc -l` | yes |
| 26 | titles | main's 31 subjects are the titles plus `(#n)` | 12 titles read from the API equal their subjects; the section's own title file equals all 31 | API `list_pull_requests fields=number,title`; `git log --format=%s origin/main` | yes |
| 27 | message sizes | #35 345,381; #36 344,967; #27 339,159; total 1,842,844 | 345,380; 344,966; 339,158; 1,842,812 | K13 | one byte each, see C5 |
| 28 | trailers and bullets | 1,542 and 1,543 lines, 15 links, 39 lower-case; 1,551 bullet lines, 894 distinct | 1,542; 1,543; 15; 39; 1,551; 894 | K14 | yes |
| 29 | signatures and bodies | 31 signature headers; committer GitHub on 31; 26 bodies open with a bullet | `%G?` reads `E` (a signature, key not at hand) on 31 and `N` on 1; 26 bodies open with `* ` | `git log --format='%G?' b3a043dc \| sort \| uniq -c`; K15 | yes |
| 30 | author address on the squash commits | one address, `brig`[23] | one address on 31 commits, `brig`[23]; the root commit has another, `nore`[21] | K16 | yes |
| 31 | rows by where their commit lives | 2 on main, 121 pull-only, 7 in no ref | 2 on main, 126 pull-only, 2 in no ref | K17 | no, see C1 |
| 32 | commits across all refs | 2,024; 2,012 from the pull heads | 2,023 on the fetched remote refs without this audit's branch; 2,012 from the pull heads. `git rev-list --count --all` also counts the audit's branch and rises with each of its commits (2,027 when last read). | K18 | no, see C2 |
| 33 | lines citing a main hash outside rows and figures | 73 in five files | 71 lines, 74 sites, in five files | K19 | no, see C3 |

Also read and found as stated: `CONTRIBUTING.md` lines 47 to 49 against
`ci.yml` lines 10 to 14; the cron lines `17 6 * * 1` and `41 7 * * 1`;
`bank-row` starts on a push to `main` and by hand, not on a tag; `ci` has
seven job keys and nine legs; `b0953fa9` is in `pull/25`, `d48a3f44` in
`pull/6`, `b3ffd258` in `pull/1`, and `445d5d5b` is no object here.

## Corrections

| | section says | corrected | command |
|---|---|---|---|
| C1 | "121 record a commit reachable only from a pull head, and seven record a commit in no ref"; "The 121 rows ... would name commits it does not hold" | 126 and 2. Five quick rows of September carry `-dirty` after the hash in `sha`. The section's loop passes the suffixed string to `git cat-file -e` and reads the five as absent. Their commits are in `pull/6`. The two rows in no ref are `...-5227305.json` and `...-14f8cc6.json`. | K17 |
| C2 | "The repository holds 2,024 commits across all refs" | 2,023: 2,012 under the pull heads, main's head, nine bank tips and the tip of `claude/wal-ahead-beside-wip`. `--all` also counts this audit's own branch, which grows as the audit commits. GitHub holds one more, the tenth bank tip. | K18 |
| C3 | "73 citing lines in five tracked files" | 71 lines or 74 sites. The section's loop adds per-hash line counts, so `ROADMAP.md` 552 and 572, which name two main hashes each, count twice. By file: `scripts/raises.txt` 62 lines (63 sites), `ROADMAP.md` 3 (5), `docs/engine.md` 3 (3), `tests/data/cost-entries.txt` 2 (2), `CLAUDE.md` 1 (1). | K19 |
| C4 | "failed in three of five hosted ARM runs today" | Three of the six `ci` runs from PR 35's head on. 20 `ci` runs were created that day; the ARM leg of the other 14 was not read. | K12 |
| C5 | message sizes | Each is one byte high: `git log --format=%B \| wc -c` counts the newline git appends. | K13 |
| C6 | "A re-message rewrites 32 hashes, two rows' `sha` fields" | 32 at most. The root commit carries no trailer line; if its message is left alone its hash stays and 31 change. The two rows' file names carry the short hash as well as their `sha` fields. | `git cat-file commit ab972300 \| grep -c -E '^(Co-Authored-By\|Claude-Session):'` (0); `git ls-tree -r --name-only b3a043dc -- bench/runs \| grep -c 5b8d64a` (2) |
| C7 | `R`'s columns: "run id, workflow, event, status, conclusion, head branch, head sha, created" | The file has 11 columns; three more follow. The section's `awk` fields are right. | `awk -F'\t' '{print NF}' "$R" \| sort -u` |

Addition: the section shows GitHub serving an unreferenced commit with
`91436d52`. The same holds for the three hashes the tree cites that are in
no fetched ref (`18aefe7`, `5227305`, `14f8cc6`): API `get_commit` returns
each.

## Redaction (rule 4)

No problem found. The section holds no email address, no URL, no session
link and no token-like string. Line 389 gives the author address in the
redacted form, `brig`[23], which matches the address on the commits.

```
grep -c -E '@[A-Za-z0-9.-]+[.][A-Za-z]{2,}|:[/][/]|claude[.]ai|session[_][0-9A-Za-z]{6,}|gh[pousr]_[A-Za-z0-9]{20,}|github[_]pat|BEGIN [A-Z ]*PRIVATE KEY' refs.md     # 0
```

## Figures without a command

- Every figure drawn from `R` needs the scratch table. The API call that built it is named; the table is not in the repository.
- The tips, tip dates and merge-base of the two `claude/*` branches.
- "its parent `ae0bcb34` is in `pull/27`".
- "four classes, all `x86_64` Xeon, 4 cores" for main's 86 quick rows.
- The class totals "if all ten were banked".
- "1.9 to 2.0 MB" and "291 to 363 files": the command covers one branch.
- "none of the 26 head ref names".
- "Main's 31 subjects equal the pull request's title".
- "37 seconds earlier" (#14 and #15) and "28 seconds after its `ci` run began" (#37).
- The triggers column of the workflow table, and the two cron times.
- "68 runs of Copilot's code review" and the `zz-*` counts.
- "Four are title only (#1, #2, #3, #5)" and "#14 ... carries its commit's message".
- "(`brig`[23])", the author address.
- "Branches whose tips date from the last three days".
- "all 32 messages total 1,842,844 bytes": the command prints the five largest.
- "three failures in five hosted ARM runs".

## The plan's figures

Each of the seven plan figures in the section's table was re-measured, and I
re-measured each by another route (checks 27 and 28 here; rows 28 and 30 of
the `citations` review for #32 and the blobs: 2 files, +120 −1; 3,998 blobs,
193.5 MiB, of which `bench/runs` 130 blobs, 64.0 MiB; no path under
`scratch/` in any of main's 32 trees). All agree, to the byte noted in C5.
The brief's "33 squash commits" and "nine bank branches" were re-measured by
the section and found stale; checks 1 and 2 confirm 32 commits and ten
branches.

## Commands

```
K1  git rev-list --parents b3a043dc | awk '{n++; if (NF==1) r++; if (NF>2) m++} END {print n, r+0, m+0}'      # 32 1 0
K2  git for-each-ref --format='%(refname:lstrip=3) %(objectname:short=8)' refs/remotes/pull | sort -n         # against the API's head.sha, 37 of 37
K3  for n in 1 6 27 35 36 37; do echo "$n $(git log --format=%H refs/remotes/pull/$n ^origin/main | wc -l)"; done
K4  git log --format='%H %s' origin/main | sed -n -E 's/^([0-9a-f]+) .*\(#([0-9]+)\)$/\1 \2/p' | while read c n; do [ $(git rev-parse $c^{tree}) = $(git rev-parse refs/remotes/pull/$n^{tree}) ] && echo equal || echo "differs #$n"; done | sort | uniq -c
K5  diff <(git diff --numstat refs/remotes/pull/15 c03088ac) <(git diff --numstat d539a606 0043b089) && echo same
K6  for p in 22:94356bc3 23:d57f8c4d 25:888ba371 26:c5e4d35a; do n=${p%%:*}; sq=${p##*:}; echo "$n $(git log -1 --format=%s $sq | grep -o -E '\(#[0-9]+\)$') $(git rev-list refs/remotes/pull/27 | grep -c ^$sq) $(git diff --numstat $(git merge-base $sq^ refs/remotes/pull/$n) refs/remotes/pull/$n | md5sum | cut -c1-8) $(git diff --numstat $sq^ $sq | md5sum | cut -c1-8)"; done
    for c in $(git rev-list refs/remotes/pull/24 ^origin/main); do git rev-list refs/remotes/pull/25 | grep -c $c; done
K7  for b in e-recode-auto wal-ahead-beside-wip; do r=origin/claude/$b; echo "$b $(git rev-parse --short=8 $r) $(git rev-list $r ^origin/main | wc -l) $(git rev-list origin/main ^$r | wc -l) $(git merge-base $r origin/main | cut -c1-8) $(git rev-list $r --not $(git for-each-ref --format='%(refname)' refs/remotes/pull) | cut -c1-8 | tr '\n' ' ')"; done
    git for-each-ref --contains origin/claude/e-recode-auto --format='%(refname:lstrip=3)' refs/remotes/pull | tr '\n' ' '
    for c in ae0bcb34 a0a90f3d; do git rev-list refs/remotes/pull/27 | grep -c ^$c; done; git show --numstat --format= a0a90f3d | awk '{a+=$1; d+=$2; n++} END {print n, a, d}'
K8  for r in $(git for-each-ref --format='%(refname)' refs/remotes/origin/bank); do p=$(git rev-parse $r^); echo "$(git rev-parse --short=8 $r) $(git rev-list --parents -1 $r | wc -w) $(git log -1 --format=%s $p | grep -o -E '#[0-9]+\)$') $(git diff-tree -r --no-renames --diff-filter=A --name-only $p $r -- bench/runs | wc -l) $(git diff-tree -r --no-renames --diff-filter=A --name-only $p $r -- bench/runs | grep -c -- "-${p:0:7}.json") $(git cat-file commit $r | sed '/^$/q' | grep -c '^gpgsig') $(git diff-tree -r --no-renames --name-only $p $r -- docs/figures | wc -l)"; done      # second field: 2 words, the commit and its one parent
K9  for r in $(git for-each-ref --format='%(refname)' refs/remotes/origin/bank); do for f in $(git diff-tree -r --no-renames --diff-filter=A --name-only $r^ $r -- bench/runs); do git show $r:$f | grep -m1 -o '"cpu_model": "[^"]*"'; done; done | sort | uniq -c
K10 git ls-tree -r --name-only b3a043dc -- bench/runs/quick | while read f; do git show b3a043dc:$f | grep -m1 -o '"cpu_model": "[^"]*"'; done | sort | uniq -c
K11 awk -F'\t' 'NR==FNR {split($0,a," "); pr[a[1]]=a[2]; next} $2=="ci" && $3=="push" && $6=="main" && ($7 in pr) {print $5, pr[$7]}' <(git log --format='%H %s' origin/main | sed -n -E 's/^([0-9a-f]+) .*\(#([0-9]+)\)$/\1 \2/p') "$R" | sort -k1,1 -k2,2n | awk '{c[$1]++; if ($1=="failure") f=f" #"$2} END {print c["success"], c["failure"], f}'
K12 awk -F'\t' '$2=="ci" && $8 ~ /^2026-11-27/' "$R" | wc -l; awk -F'\t' '$2=="ci" && $8>="2026-11-27T16:15" {print substr($8,12,5), $3, $5, substr($7,1,8)}' "$R" | sort
K13 for c in $(git rev-list b3a043dc); do echo "$(git cat-file commit $c | sed '1,/^$/d' | wc -c) $(git log -1 --format=%s $c | grep -o -E '#[0-9]+\)$')"; done | sort -rn | awk '{t+=$1} NR<=3 {print} END {print "total", t}'
K14 git rev-list b3a043dc | while read c; do git cat-file commit $c | sed '1,/^$/d'; done | grep -c '^Co-Authored-By:'     # likewise '^Co-authored-by:', '^Claude-Session:', '^\* '; "grep <pattern> | sort -u | wc -l" for the 15 links and the 894 distinct bullets
K15 git rev-list b3a043dc | while read c; do git cat-file commit $c | sed '1,/^$/d' | sed '1,/^$/d' | grep -m1 . | cut -c1-2 | grep -c -x -F '* '; done | sort | uniq -c      # 26 "1"
K16 git log --format=%ae b3a043dc | sort | uniq -c | awk '{print $1, substr($2,1,4) "[" length($2) "]"}'
K17 git grep -h -o -E '"sha": "[0-9a-f]{40}' b3a043dc -- bench/runs | cut -c9-48 | while read s; do if git merge-base --is-ancestor $s b3a043dc 2>/dev/null; then echo main; elif [ -n "$(git for-each-ref --contains $s refs/remotes/pull 2>/dev/null | head -1)" ]; then echo pull-only; else echo no-ref; fi; done | sort | uniq -c
    git grep -h -o -E '"sha": "[0-9a-f]{40}[^"]+"' b3a043dc -- bench/runs | cut -c9-48 | while read s; do git for-each-ref --contains $s --format='%(refname:lstrip=2)' refs/remotes/pull | head -1; done | sort | uniq -c      # 5 pull/6
K18 git rev-list --count $(git for-each-ref --format='%(refname)' refs/remotes | grep -v supdb-pretag-coordination); git rev-list --count --all; git rev-list --count --remotes=pull
K19 P='(e40d628|09cadec|99d44af|eceb66d|dbc2a2a|5b8d64a|d5679e4|37ea8f2)[0-9a-f]*'; git grep -I -w -E "$P" b3a043dc -- . ':!bench/runs' ':!docs/figures' | wc -l; git grep -I -o -w -E "$P" b3a043dc -- . ':!bench/runs' ':!docs/figures' | wc -l; git grep -I -c -w -E "$P" b3a043dc -- . ':!bench/runs' ':!docs/figures'
```
