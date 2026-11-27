# Review of `sections/size.md`

Scope: main at `b3a043dc`, the 49 fetched refs without the coordination branch, and GitHub through read-only calls. Each "mine" value comes from a route the section does not use: object ids from the pack indexes (`git show-index`) where the section walks `rev-list --objects`; packed sizes from index offsets where it reads `verify-pack`; reachability from `git ls-tree` of every commit; the current tree from a tar archive and the working files; clone sizes from a pack written to a pipe; commit classes from `git log` listings and set difference; the branch list from GitHub.

Result: 27 claims re-derived. 23 agree. Two are over-counts (the objects and packed bytes "beyond main" of the `claude/*` leftovers and of each pull request head). One estimate is high (the packed size of every ref). One is stale against the host (ten `bank/quick-*` branches, not nine). Rule 4 holds. Five figures have no command.

The checkout has moved since the section was written, in two ways. [V1]

- `HEAD` is on the coordination branch, ahead of `b3a043dc`, and that branch's objects are loose. `git rev-list --all` and `git cat-file --batch-all-objects` now reach more than the 49 refs. The section's commands name their refs and are not affected by this.
- A remote ref `origin/release-candidate` now exists at `b3a043dc`. It adds no commit and no object. The section's setup leaves out the coordination branch only, so three of its commands now print something else: S2 prints 50 refs and 13 branches, M9 lists the new branch at 0 ahead and 0 behind, and H2 prints nothing, because that branch holds all of main. This review's commands leave the new branch out, as they do the coordination branch.

## Checks

| # | Claim | Theirs | Mine | Cmd | Agree |
|---:|---|---|---|---|---|
| 1 | Refs in scope; tags; local `main` | 49 (12 branches, 37 pull heads); 0; `e40d6283` | the same | V1 | yes |
| 2 | Pack entries; distinct objects; objects in more than one pack | 33,269; 32,503; 724 | 33,269; 32,503; 724 (682 in two packs, 42 in three) | V2 | yes |
| 3 | Pack file bytes; `size-pack` | 109,380,266; 105.20 MiB | the same | V3 | yes |
| 4 | Objects, bytes and packed bytes by type | blob 22,294 / 2,776,522,646 / 74,110,335; tree 8,186 / 7,437,594 / 1,130,074; commit 2,023 / 5,555,268 / 2,411,269; all 32,503 / 2,789,515,508 / 77,651,678 | the same | V4 | yes |
| 5 | Every packed object is reachable from the 49 refs | yes | 32,503 reachable, none in one set only | V5 | yes |
| 6 | Main: objects, bytes, packed; blobs, trees, commits packed | 4,349 / 205,116,470 / 19,664,736; 19,079,750 / 138,735 / 446,251 | the same | V6 | yes |
| 7 | Blobs on main against blobs on other refs only | 3,998 / 202,889,720 / 19,079,750 against 18,296 / 2,573,632,926 / 55,030,585 | the same | V6 | yes |
| 8 | Every branch; other branches not main; pull heads only (objects, packed) | 13,513 / 27,181,503; 9,164 / 7,516,767; 18,990 / 50,470,175 | the same, as sums of stored copies | V6 | yes |
| 9 | Packed size of what is fetched: main; every branch; every ref | 19,664,736; 27,181,503 (estimate); 77,651,678 (estimate) | A pack written for those refs is 19,664,785; 26,573,782; 65,891,723 bytes | V7 | no: the estimate for every ref is 11.8 MB high |
| 10 | Rows in the current tree, and by scale | 130 / 67,067,767; quick 86 / 38,497,827; ooc 34 / 22,386,720; full 8 / 4,598,794; killed/ooc 2 / 1,584,426 | the same, from a tar archive and from the working files | V8 | yes |
| 11 | Row versions in history: at `8025423`; main; all refs; off main; on bank branches only | 112 / 53,523,449 / 7,130,584; 130 / 67,067,767 / 8,876,320; 188 / 95,425,143 / 12,525,728; 58; 27 / 17,888,453 / 2,914,821 | the same; off main is 58 / 28,357,376 / 3,649,408 | V9 | yes |
| 12 | Commits the 130 row names resolve to | 2 on main; 126 off main; 2 absent | the same | V10 | yes |
| 13 | `scratch/`: on main; all refs | 0; 2,426 / 289,522,464 / 30,533,069 | 0; 2,427 blob ids stand at a `scratch/` path. One of them, 2,140 bytes, is also `scripts/resident.py` on main, and the section files it there | V11 | yes |
| 14 | `scratch/` under #6; under #35 and #36 | 1,274 / 277,292,068; 1,153 / 12,230,396, the same set | 1,275 and 1,154 with that one blob; #35 and #36 hold the same set | V11 | yes |
| 15 | Current tree: files, bytes; distinct blobs, their bytes and packed; top-level table; files over 1 MB; figures; depth; longest path | 2,630 / 92,948,487; 2,172 / 91,029,029 / 12,112,316; table (f); three files; 2,274 / 10,515,967; 5; 143 | the same | V12 | yes |
| 16 | Commits: main; pull heads only; branch and pull head; branch only; all; merges | 32; 1,745; 236; 10; 2,023; 62 | the same | V13 | yes |
| 17 | The 40 largest blobs: largest; versions of `src/db.rs`; in main's tree; in main's history | 7,611,360; 37; 1; 3 more | the same. The introducing commits of rows 2, 5, 6 and 20 also agree | V14 | yes |
| 18 | Versions of `src/db.rs`: all refs; main | 698 / 1,165,383,287 / 7,241,613; 23 / 42,530,462 / 1,813,495 | the same | V14 | yes |
| 19 | Bank branches beyond main: objects, packed; blobs | 2,343 / 4,166,435; 2,244 = 27 rows and 2,217 figures | the same | V15 | yes |
| 20 | `claude/*` leftovers beyond main; objects in both sets | 7,313 / 5,681,955; 492 | 6,821 / 3,350,332; 0 | V15 | no |
| 21 | Pull request heads beyond main, table under (g) | objects and packed columns | lower in all 15 rows; commits column agrees | V16 | no |
| 22 | Squash trees equal to their pull head's tree | 30 of 31; #15 differs | the same | V17 | yes |
| 23 | Commit object bytes on main | 1,875,181 | 1,875,181 | V18 | yes |
| 24 | Squash items in main's messages; repeated from an older main commit | 1,551; 653 | the same | V19 | yes |
| 25 | Widest tree, all refs | `scratch/wave2`, 979 entries | 979 | V20 | yes |
| 26 | `bank/quick-*` branches; main commits on no other ref | nine; `b3a043dc` alone | nine in the checkout. GitHub has ten, and the tenth is one commit on `b3a043dc` | V21, G1 | no: stale |
| 27 | Message figures quoted from the other section: largest messages; message bytes on main; trailers on main, the seven newest and every ref | 345,381, 344,967, 339,159, 276,792; 1,842,844; 1,542 / 1,543 / 15; 790 / 790 / 9; 3,554 / 3,555 / 18 | the same | V22 | yes |

The second-level rows for `docs/figures/`, `src/db/`, `bench/src/`, `scratch/plan/`, `scratch/seam/` and `bench/runs/`, and the six-file version table, were recomputed from the same listing as rows 7 and 11 and agree.

## Corrections

1. **`claude/*` leftovers beyond main (section (g), [G6]).** They hold 6,821 objects and 3,350,332 packed bytes that main does not reach, not 7,313 and 5,681,955. No object is in both the bank set and the leftover set; "492 objects are in both sets" should go. The 492 are 487 blobs and 5 trees that main reaches. `git rev-list --objects A ^B` leaves out what B's boundary trees hold, not everything B reaches. With the correction the two sets add up to the section's own "other branches, not main": 4,166,435 + 3,350,332 = 7,516,767. A third route, the full object list of each side and `comm`, gives 6,821. [V15, V23]

2. **Pull request heads beyond main (table under (g), [G7]).** Same cause. The commits column is right. Exact objects and packed bytes:

   | Pull request | Commits not on main | Objects, theirs | Objects, exact | Packed, theirs | Packed, exact |
   |---|---:|---:|---:|---:|---:|
   | #6 | 691 | 6,610 | 6,437 | 34,551,432 | 31,204,960 |
   | #36 | 632 | 12,627 | 12,600 | 15,524,841 | 15,502,460 |
   | #35 | 630 | 12,741 | 12,598 | 18,256,017 | 15,500,873 |
   | #27 | 361 | 10,302 | 8,851 | 9,961,808 | 5,169,355 |
   | #25 | 384 | 9,966 | 8,987 | 9,504,001 | 5,167,090 |
   | #26 | 353 | 10,171 | 8,815 | 9,749,293 | 5,121,728 |
   | #23 | 344 | 9,597 | 8,654 | 8,895,340 | 4,783,536 |
   | #22 | 339 | 9,435 | 8,501 | 8,460,527 | 4,486,501 |
   | #21 | 177 | 6,331 | 5,853 | 4,696,362 | 2,750,739 |
   | #32 | 92 | 945 | 942 | 2,232,820 | 2,228,870 |
   | #31 | 90 | 949 | 940 | 3,579,493 | 2,227,374 |
   | #30 | 83 | 1,063 | 912 | 3,313,881 | 2,210,002 |
   | #33 | 49 | 2,490 | 1,770 | 5,034,897 | 2,194,761 |
   | #1 | 329 | 3,209 | 2,943 | 2,724,986 | 2,064,466 |
   | #29 | 40 | 461 | 423 | 1,859,298 | 1,360,007 |

   The same 15 pull requests are over 1 MB. The order changes below #35. [V16, V23]

3. **What a clone costs (section (g), first table).** The section's packed column is the sum of each object's smallest stored copy, and it says the two lower lines are estimates. A pack written for the same refs measures:

   | What is fetched | Section | Pack written | Difference |
   |---|---:|---:|---:|
   | `main` alone | 19,664,736 | 19,664,785 | +49 |
   | every branch | 27,181,503 | 26,573,782 | -607,721 |
   | every branch and every pull request head | 77,651,678 | 65,891,723 | -11,759,955 |

   The checkout's three packs came from separate fetches and cannot delta against each other, so the stored-copy sum is above what one pack needs. By the measured packs main is 30% of everything, the other branches 10% (6,908,997), and pull request heads alone 60% (39,317,941), where the section has 25%, 10% and 65%. Its statement that a plain clone is about 27.2 MB reads 26.6 MB, and "65% of the packed bytes" under "What bears on the three options" reads 60%. Nothing was written to the repository; a host packs with its own settings, so its figure will differ again. [V7]

4. **`bank/quick-*` branches.** The host has a tenth, `bank/quick-46057759452`, head `6df1f587`, one `github-actions[bot]` commit on `b3a043dc` that adds three quick rows named for `b3a043d` and figure files. The checkout does not hold it. On the host: ten bank branches; 30 quick rows on them, not 27; and `b3a043dc` is no longer on main alone, so every commit of main is reachable from a bank branch. The host lists 15 branches. Two are this audit's own (the coordination branch, and `release-candidate` at `b3a043dc`); the other 13 are the section's 12 and the tenth bank branch. Options 2 and 4 under "What bears on the three options" should say so. The byte figures for the bank branches need a fetch to re-measure. [V21, G1]

Not corrections, for the merged document:

- The section calls message bytes "bodies": #27 at 339,159 is the title, a blank line and the body. `messages.md` gives the body alone, 339,043. Both are right. The two sections should not show different numbers under one word.
- [F8] covers main's tip tree. In history there is one gitlink (mode 160000): `.claude/worktrees/agent-<id>` in commit `cbe9c17f`, under pull request #1's head only, removed by the next commit. Its target commit is in the repository. No ref's history has a symlink. [V24]

## The plan's figures

The section's first table lists eight. Each has a re-measured value and a label. All eight re-measured values agree with this review except the bank count, which is correction 4. `messages.md` lists a ninth plan figure, that a squash body is the concatenated messages of its branch; it is not a size and is not in this section. The plan itself is not in the checkout, so whether the list is complete was not checked.

## Redaction (rule 4)

- Full email addresses: none.
- Session links: none. The only `session_` text is the search pattern in M7 and M8.
- Private URLs: none. No URL of any kind is in the file.
- Token-like strings: none. The longest hexadecimal strings are 8 to 10 character object ids.

## Figures without a command

1. (a): "Every object in the packs is reachable from the 49 refs." [V5 prints it.]
2. (d): "The 27 rows on the `bank/quick-*` branches all name main's commits, #28 to #36, three each." D3 reads the 130 names in main's tree only.
3. (d): the two absent hashes. D3 prints counts, not which names.
4. (g): "492 objects are in both sets." No command, and not so (correction 1).
5. (h): "none on main" for merge commits. H1 counts merges over all refs only. [V13 prints both.]

Percentages (72%, 45%, 42%, 9%, 39%, 83%, and the 25 / 10 / 65 split) are derived from figures that have commands.

## Commands

Run from `/home/user/supdb`. Setup first; it writes one temporary directory outside the checkout.

```sh
R=$(git show-ref | awk '$2 ~ /^refs\/remotes\// && $2 !~ /supdb-pretag-coordination|origin\/release-candidate$/{print $2}'); B=$(echo "$R" | grep /origin/); P=$(echo "$R" | grep /pull/); T=$(mktemp -d)
# packed bytes of each object: next offset less its own, smallest over the packs
for i in .git/objects/pack/*.idx; do git show-index < $i | sort -n | awk -v e=$(($(stat -c %s ${i%.idx}.pack)-20)) 'NR>1{print h,$1-o} {o=$1;h=$2} END{print h,e-o}'; done | sort | awk '{if(!($1 in d)||$2<d[$1])d[$1]=$2} END{for(k in d)print k,d[k]}' > $T/packed
# objects reachable from the given refs, by listing every commit's tree
objs(){ (git rev-list "$@"; git log --format=%T "$@"; git log --format=%H "$@" | while read c; do git ls-tree -r -t $c | awk '$2!="commit"{print $3}'; done) | sort -u; }
# ids on stdin -> per type: count bytes packed; then all
tot(){ git cat-file --batch-check='%(objecttype) %(objectsize) %(objectname)' | awk 'NR==FNR{p[$1]=$2;next}{n[$1]++; s[$1]+=$2; d[$1]+=p[$3]; N++; S+=$2; D+=p[$3]} END{for(t in n) printf "%s %d %d %d; ", t, n[t], s[t], d[t]; print "all", N, S, D}' $T/packed -; }
objs $R > $T/all; objs b3a043dc > $T/main; objs $B > $T/br
msg(){ git cat-file commit $1 | sed '1,/^$/d'; }; body(){ msg $1 | sed '1,/^$/d' | awk 1; }

# V1: refs, branches, pull heads, tags, local main; commits HEAD has beyond b3a043dc, loose objects and commits under --all (all three above 0, 0 and 2,023); the new branch; remote refs the section's setup now selects
echo "$(echo "$R" | wc -l) $(echo "$B" | wc -l) $(echo "$P" | wc -l) $(git show-ref --tags | wc -l) $(git show-ref refs/heads/main | cut -c1-8)"; git rev-list --count b3a043dc..HEAD; find .git/objects -type f -path '.git/objects/[0-9a-f][0-9a-f]/*' | wc -l; git rev-list --count --all
git show-ref | awk '$2 ~ /origin\/release-candidate$/{print substr($1,1,8), $2}'; git for-each-ref --format='%(refname)' refs/remotes | grep -vc supdb-pretag-coordination
# V2: pack entries, distinct objects, objects in one, two, three packs
for i in .git/objects/pack/*.idx; do git show-index < $i | awk '{print $2}'; done | sort | uniq -c | awk '{c[$1]++; n++; e+=$1} END{print e, n, c[1], c[2], c[3]}'
# V3: pack file bytes; packs and indexes in MiB
stat -c %s .git/objects/pack/*.pack | awk '{s+=$1} END{print s}'; stat -c %s .git/objects/pack/*.pack .git/objects/pack/*.idx | awk '{s+=$1} END{printf "%.2f\n", s/1048576}'
# V4: every packed object by type
cut -d' ' -f1 $T/packed | tot
# V5: reachable objects; ids in the packs or the reachable set alone
wc -l < $T/all; cut -d' ' -f1 $T/packed | sort | comm -3 - $T/all | wc -l
# V6: main; other refs only; every branch; other branches not main; pull heads only
tot < $T/main; comm -23 $T/all $T/main | tot; tot < $T/br; comm -23 $T/br $T/main | tot; comm -23 $T/all $T/br | tot
# V7: bytes of a pack written to a pipe for main, every branch, every ref (nothing is stored)
echo b3a043dc | git pack-objects --revs --stdout -q --delta-base-offset --threads=1 | wc -c
echo "$B" | git pack-objects --revs --stdout -q --delta-base-offset --threads=1 | wc -c
echo "$R" | git pack-objects --revs --stdout -q --delta-base-offset --threads=1 | wc -c
# V8: rows in the tree from a tar archive, by scale and in all; from the working files
git archive b3a043dc bench/runs | tar -tvf - | awk '$1 ~ /^-/{split($6,a,"/"); k=a[3]; if(k=="killed")k=k"/"a[4]; c[k]++; s[k]+=$3; N++; S+=$3} END{for(k in c)print k, c[k], s[k]; print "all", N, S}'
find bench/runs -type f -printf '%s\n' | awk '{n++; s+=$1} END{print n, s}'
# V9: row versions: main; at 8025423; all refs; off main; on bank branches and off main
rows(){ git log --format=%H "$@" | while read c; do git ls-tree -r $c -- bench/runs | awk '{print $3}'; done | sort -u; }
rows b3a043dc | tot; rows 8025423 | tot; rows $R | tot; rows $R | comm -23 - $T/main | tot; rows $(echo "$B" | grep /bank/) | comm -23 - $T/main | tot
# V10: what the 130 row names resolve to
git ls-tree -r --name-only b3a043dc -- bench/runs | sed -E 's/.*-([0-9a-f]+)\.json$/\1/' | while read h; do t=$(git rev-parse -q --verify "$h^{commit}" 2>/dev/null) || { echo absent; continue; }; git merge-base --is-ancestor $t b3a043dc && echo main || echo off-main; done | sort | uniq -c
# V11: scratch/ entries in main's trees; blob ids at a scratch/ path over all refs, and those main reaches; under #6, #35, #36
scr(){ git log --format=%H "$@" | while read c; do git ls-tree -r $c -- scratch | awk '{print $3}'; done | sort -u; }
git log --format=%H b3a043dc | while read c; do git ls-tree -r $c -- scratch; done | wc -l; scr $R | tot; scr $R | comm -12 - $T/main | tot
for n in 6 35 36; do scr refs/remotes/pull/$n | tot; done; scr refs/remotes/pull/35 | cmp -s - <(scr refs/remotes/pull/36) && echo same
# V12: tree: files and bytes; by top level; over 1 MB; figures; deepest and longest path; distinct blobs
git archive b3a043dc | tar -tvf - | awk '$1 ~ /^-/{n++; s+=$3} END{print n, s}'
git archive b3a043dc | tar -tvf - | awk '$1 ~ /^-/{n=split($6,a,"/"); k=(n>1?a[1]"/":"(root)"); c[k]++; s[k]+=$3} END{for(k in c)print s[k], c[k], k}' | sort -rn
git archive b3a043dc | tar -tvf - | awk '$1 ~ /^-/ && $3>1000000{print $3, $6}' | sort -rn
git archive b3a043dc docs/figures | tar -tvf - | awk '$1 ~ /^-/{n++; s+=$3} END{print n, s}'
git archive b3a043dc | tar -tvf - | awk '$1 ~ /^-/{n=split($6,a,"/"); c[n]++; if(n>m)m=n; if(length($6)>L)L=length($6)} END{print m, c[m], L}'
git ls-tree -r b3a043dc | awk '{print $3}' | sort -u | tot
# V13: pull-only commits; branch-only commits; merges over all refs; merges on main
git log --format=%H $P | sort -u | grep -vcFf <(git log --format=%H $B | sort -u); git log --format=%H $B | sort -u | grep -vFf <(git log --format=%H $P | sort -u) | grep -vcFf <(git log --format=%H b3a043dc); git log --format=%P $R | awk 'NF>1' | wc -l; git log --format=%P b3a043dc | awk 'NF>1' | wc -l
# V14: the 40 largest blobs: largest; versions of src/db.rs (all refs, main); how many of the 40 are versions of it, are in main's history, are in main's tree
cut -d' ' -f1 $T/packed | git cat-file --batch-check='%(objecttype) %(objectsize) %(objectname)' | awk '$1=="blob"{print $2, $3}' | sort -k1,1nr -k2,2 | head -40 > $T/top; head -1 $T/top
git log --format=%H $R | while read c; do git rev-parse -q --verify $c:src/db.rs; done | sort -u > $T/dbrs; tot < $T/dbrs; git log --format=%H b3a043dc | while read c; do git rev-parse -q --verify $c:src/db.rs; done | sort -u | tot
awk '{print $2}' $T/top | sort | comm -12 - $T/dbrs | wc -l; awk '{print $2}' $T/top | sort | comm -12 - $T/main | wc -l; awk '{print $2}' $T/top | sort | comm -12 - <(git ls-tree -r b3a043dc | awk '{print $3}' | sort -u) | wc -l
# V15: bank branches beyond main; claude leftovers beyond main; objects in both
objs $(echo "$B" | grep /bank/) | comm -23 - $T/main > $T/bank; tot < $T/bank
objs refs/remotes/origin/claude/e-recode-auto refs/remotes/origin/claude/wal-ahead-beside-wip | comm -23 - $T/main > $T/claude; tot < $T/claude; comm -12 $T/bank $T/claude | wc -l
# V16: each pull head beyond main: number, then objects, bytes, packed
for n in $(seq 1 37); do echo "$n $(objs refs/remotes/pull/$n | comm -23 - $T/main | tot | sed 's/.*all //')"; done | sort -k4,4nr
# V17: squash trees against their pull head's tree
git log --format='%T %s' b3a043dc | sed -nE 's/^([0-9a-f]+) .*\(#([0-9]+)\)$/\1 \2/p' | while read t n; do [ "$(git log -1 --format=%T refs/remotes/pull/$n)" = $t ] && echo same || echo "differs#$n"; done | sort | uniq -c
# V18: commit object bytes on main
git rev-list b3a043dc | while read c; do git cat-file commit $c | wc -c; done | awk '{s+=$1} END{print s}'
# V19: squash items, and those an older main commit lists
git rev-list --reverse b3a043dc | while read c; do body $c | sed -n "s/^\* /$c /p"; done | awk '{c=$1; l=substr($0,42); N++; if((l in f) && f[l]!=c) T++; if(!(l in f)) f[l]=c} END{print N, T}'
# V20: most entries scratch/wave2 has in any commit under the pull heads
git log --format=%H $P | while read c; do git ls-tree $c:scratch/wave2 2>/dev/null | wc -l; done | sort -n | tail -1
# V21: bank branches in the checkout; the host's tenth is not here; main commits no other fetched ref holds
git show-ref | grep -c 'refs/remotes/origin/bank/quick-'; git cat-file -e 6df1f587af5138fa3fc80b3ffd6e03a7b1625f3d 2>/dev/null && echo present || echo absent; git log --format=%H b3a043dc | grep -vFf <(git log --format=%H $P $(echo "$B" | grep -v 'origin/main$') | sort -u) | cut -c1-8
# V22: message bytes as --format=%B prints them, largest four; on main; trailers and links on main, seven newest, every ref
git rev-list b3a043dc | while read c; do echo "$(( $(msg $c | wc -c) + 1 )) $(echo $c | cut -c1-8)"; done | sort -rn | head -4; git rev-list b3a043dc | while read c; do msg $c | wc -c; done | awk '{s+=$1+1} END{print s}'
X='import sys,re;d=sys.stdin.buffer.read();print(len(re.findall(rb"(?m)^Co-Authored-By:",d)),len(re.findall(rb"(?m)^Claude-Session:",d)),len(set(re.findall(rb"claude\.ai/code/session_\w+",d))))'
git rev-list b3a043dc | git cat-file --batch | python3 -c "$X"; git rev-list -7 b3a043dc | git cat-file --batch | python3 -c "$X"; git rev-list $R | git cat-file --batch | python3 -c "$X"
# V23: third route for corrections 1 and 2: both full object lists, then set difference (here the leftovers; substitute refs/remotes/pull/27)
git rev-list --objects origin/claude/e-recode-auto origin/claude/wal-ahead-beside-wip | cut -c1-40 | sort -u | comm -23 - <(git rev-list --objects b3a043dc | cut -c1-40 | sort -u) | wc -l
# V24: tree entries that are not regular files, in any commit of any ref: commit, mode
git log --format=%H $R | while read c; do git ls-tree -r $c | awk -v c=$c '$1!="100644" && $1!="100755"{print substr(c,1,8), $1}'; done
rm -rf $T
```

GitHub, read-only, through the GitHub MCP tools. These cannot be reproduced from the checkout alone.

```
G1  mcp__github__list_branches(owner="bfulton", repo="supdb", perPage=100)                                                                -> 15 branches, ten of them bank/quick-*
    mcp__github__list_commits(owner="bfulton", repo="supdb", sha="bank/quick-46057759452", perPage=2, fields=["sha"])                     -> 6df1f587, then b3a043dc
    mcp__github__get_commit(owner="bfulton", repo="supdb", sha="6df1f587af5138fa3fc80b3ffd6e03a7b1625f3d", detail="stats", perPage=100)  -> author, date, the three row files
```
