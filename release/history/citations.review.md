# Review of `citations.md`

Checked at `b3a043dc` (tree `7f58d776`). The checkout's `HEAD` is ahead
of it by commits under `release/` only. Every command runs from the
repository root. Commands marked API are GitHub MCP calls for
`bfulton/supdb`.

Verdict: the section's counts hold. 35 checks: 27 agree, 8 rows need a
correction or a qualifier (seven corrections, C1 to C7). None changes the
remap list of option 2 as the tree stands (8 commits, 78 sites, 7 files).
Two (C4, C5) change what banking the `bank/quick-*` branches would add to it.

## The tool against its output

| check | result | command |
|---|---|---|
| `citations.py` output against `release/citations.tsv` | identical, 463 lines | K1 |
| the tool's stderr summary | class 1: 8 hashes, 78 sites; class 2: 334, 3,046; class 6: 3, 5; class 7: 6, 132; classes 3 to 5 empty; exit status 1 | `python3 release/tools/citations.py >/dev/null; echo $?` |
| the tree the tool read is `b3a043dc`'s | no difference outside `release/` | `git diff --stat b3a043dc -- . ':!release' \| wc -l` (0) |

## Checks

"Mine" was derived without the tool unless the command says otherwise.

| # | claim | theirs | mine | command | agree |
|---|---|---|---|---|---|
| 1 | commits on main cited, and their sites | 8 commits, 78 sites | 8 commits, 78 sites: `e40d628` 1, `09cadec` 3, `99d44af` 2, `eceb66d` 46, `dbc2a2a` 18, `5b8d64a` 5, `d5679e4` 1, `37ea8f2` 2 | K2 | yes |
| 2 | cited commits not on main, and their sites | 334 commits, 3,046 sites | 334 commits, 3,046 sites | K3 | yes |
| 3 | no resolving token is left out (rows' numbers, code, `0x` literals) | 0 left out | 0: every 7 to 40 hex word of every text blob resolves at the same 3,124 sites the tool lists | K3 | yes |
| 4 | files holding a resolving citation | 2,413: 2,271 figures, 128 rows, 14 text files | 2,413: 2,271, 128, 14 | K4 | yes |
| 5 | class 5, ambiguous spellings | 0 | 0 of 462 spellings match more than one object | K5 | yes |
| 6 | class 6, three hashes in no fetched ref | `18aefe7`, `5227305`, `14f8cc6` unresolvable | no object under any of the three prefixes locally. GitHub serves all three by hash (commits of 2026-11-03, 11-24 and 11-25). | K6, and API `get_commit sha=<hash> detail=none` for each | yes, with an addition |
| 7 | row file name is the start of its `sha` | 130 of 130; five carry `-dirty` | 130 of 130; five `-dirty`, all quick rows of September | K7 | yes |
| 8 | rows by where their commit lives | 2 main, 126 pull-ref, 0 other branch, 2 nowhere | 2 main, 126 pull-only, 2 in no ref | K8 | yes |
| 9 | distinct commits the rows name | 109 | 109 | `git grep -h -o -E '"sha": "[0-9a-f]{40}' b3a043dc -- bench/runs \| sort -u \| wc -l` | yes |
| 10 | main commits outside every pull ref | `b3a043dc` alone | `b3a043dc` alone | `git rev-list b3a043dc --not --remotes=pull \| cut -c1-8` | yes |
| 11 | the eight class 1 commits are ancestors of `pull/37` | 8 of 8 | 8 of 8 | K9 | yes |
| 12 | class 2 commits in exactly one pull ref | 113: 89 in #6, 20 in #33, 2 in #13, 1 in #20, 1 in #26 | 113: 89, 20, 2, 1, 1 | K10 | yes |
| 13 | `f3bc29c` is reachable only from #26 | `pull/26` | `pull/26` and no other remote ref | `git for-each-ref --contains f3bc29c --format='%(refname:lstrip=2)' refs/remotes` | yes |
| 14 | class 2 commits also in `origin/claude/e-recode-auto` | 118 | 118 | K11 | yes |
| 15 | hex tokens among the rows' numbers | 1,175,587; 1,010,810 distinct; none a commit; 29 a blob or tree prefix | 1,175,587; 1,010,810; 0 commits; 29 (20 blobs, 9 trees) | K12 | yes |
| 16 | hex tokens in source files | 485; 84 resolve | 485; 84 resolve (`bench/src/row.rs` 52, `src/db.rs` 31, `bench/examples/zrec.rs` 1) | K13 | yes |
| 17 | `0x` literals of 7 to 40 hex digits | 43, 13 distinct, 0 resolve | 42, 12 distinct, 0 resolve | K14 | count differs by one; see C7 |
| 18 | spellings by length | 7: 336; 8: 8; 40: 107 | 7: 336 (2,984 sites); 8: 8 (12); 40: 107 (128) | K3 | yes |
| 19 | commits cited under two spellings | 109, "a row's 7-character name and its 40-character `sha`" | 109: 107 are a row name and its `sha`; 2 are a 7- and an 8-character form | K15 | count yes; wording no, see C2 |
| 20 | shortest unambiguous length | 9 among objects, 6 among commits | 9 among the objects and 6 among the commits of every ref (32,521 objects and 2,026 commits when read; both rise with this audit's own commits) | K16 | yes |
| 21 | nine 7-character citations need 6 characters | nine listed | all nine: two objects share the 5-character prefix, one the 6-character prefix | K17 | yes |
| 22 | "at `<hash>`" sites, and those naming a main commit | 358; 5 | 358; 5 (`CLAUDE.md:9`, `ROADMAP.md:552` twice, `docs/engine.md:4636` and `9066`) | K18 | yes |
| 23 | (`CLAUDE.md`, *entry*) pointers | 20 in 8 files, 21 matches | 21 matches in 9 files; one is the defining line, `CLAUDE.md:10` | K19 | yes |
| 24 | row names in text outside rows and figures | 52 in `bench/src/row.rs`, 7 in docs, 2 fixtures | `row.rs` 53 (one a fixture; the other 52 exist under `bench/runs`), `ledger.rs` 1, `docs/engine.md` 2, `docs/zpages-design.md` 5 | K20 | yes |
| 25 | class 7, compiler hashes in the rows | 52, 24, 30, 24 | `e408947bf` 52, `2d8144b78` 24, `b940084d7` 30, `0ef1bc386` 24; none is an object here | K21 | yes |
| 26 | backticked `3486` is a commit prefix by chance | one site, `docs/zpages-design.md:1034` | two sites, lines 1034 and 1550 | ``git grep -n -o -E '`3486`' b3a043dc -- docs \| cut -d: -f2,3`` | no, see C1 |
| 27 | sizes of main's messages | #27 339,159; #35 345,381; #36 344,967; all 32: 1,842,844 bytes | 339,158; 345,380; 344,966; 1,842,812 | K22 | one byte each, see C3 |
| 28 | #32's change and message | 2 files, +120 −1; 94,579 bytes | 2 files, +120 −1; 94,578 bytes | `git diff --numstat 8025423b^ 8025423b`; K22 | one byte, see C3 |
| 29 | trailers on main | 1,542 `Co-Authored-By`, 39 `Co-authored-by`, 1,543 `Claude-Session`, 15 links | 1,542; 39; 1,543; 15 | K23 | yes |
| 30 | blobs in main's history | 3,998, 193.5 MiB; `bench/runs` 130, 64.0 MiB | 3,998, 193.5 MiB; 130, 64.0 MiB | K24 | yes |
| 31 | `bank/quick-*` branches | nine, 27 rows, each named for a main commit, none on main | nine fetched: each one commit on a main commit (#28 to #36), 3 rows each, 27 in all. GitHub holds ten. | K25; API `list_branches perPage=100` | fetched refs yes; GitHub no, see C4 |
| 32 | what banking adds to the remap list | 27 row files, 54 sites | 27 row files, and on each branch 290 to 362 figure captions that name the main commit | K26 | no, see C5 |
| 33 | commits a clone of the branches resolves | 8 of 342 | 126 today (8 on main and 118 on `claude/e-recode-auto`); 8 once that branch is deleted | K27 | qualifier, see C6 |
| 34 | 47 spellings resolved one at a time (10 class 1, 26 class 2, 5 class 6, 6 class 7) | class, full hash and pull list as in the TSV | 47 of 47 agree | K28 | yes |
| 35 | tokens in `CLAUDE.md`, `ROADMAP.md`, `docs/engine.md` | 1, 28 and 352 sites | 1 of 1, 28 of 28 and 351 of 797 tokens resolve; the one other with a hex letter is `18aefe7`; the remaining 445 are numbers | K29 | yes, none missed |

No digit-only token is counted by chance. Eight digit-only hashes are cited
outside rows and figures (`2027778`, `3811909`, `5606209`, `6694659`,
`7389614`, `9648114` and two row names in `row.rs`); each stands after "at"
in backticks or in a row's file name.

Two mixed hex fragments in prose are not citations and the tool rightly
skips them: `docs/zpages-design.md:2051` holds a local scratch path whose
directory name splits into hex words. That line is a session-scoped local
path in a tracked document.

## Corrections

| | section says | corrected | command |
|---|---|---|---|
| C1 | `3486` at one site | two sites: `docs/zpages-design.md` lines 1034 and 1550 | row 26 |
| C2 | 109 commits under two spellings, "a row's 7-character name and its 40-character `sha`" | 107 are a row name and its `sha`. Two are doc spellings of 7 and 8 characters: `09cadec`/`09cadec0` and `00745f1`/`00745f1a`. | K15 |
| C3 | message sizes | Each figure is one byte high: `git log --format=%B \| wc -c` counts the newline git appends. Raw sizes: #35 345,380; #36 344,966; #27 339,158; #32 94,578; all 32 messages 1,842,812. | K22 |
| C4 | nine `bank/quick-*` branches | Nine are fetched. GitHub holds ten: `bank/quick-46057759452` (tip `6df1f587`, 2026-11-27 20:02 UTC) adds three rows named `-b3a043d.json`. Ten branches carry 30 rows. | API `list_branches`; API `get_commit sha=6df1f587 detail=stats` |
| C5 | banking adds 27 row files (54 sites) | Each bank commit also redraws `docs/figures/quick`. On the nine fetched branches 290, 305, 356, 317, 317, 362, 317, 317 and 329 figure files name the branch's main commit in a caption. Main today has no figure caption naming a main commit. With the tenth branch the rows alone are 30 files (60 sites), and the cited main commits rise from 8 to 12 (#30, #31, #32 and #37 join). | K26 |
| C6 | "a clone of the branches alone resolves 8 of the 342 commits" | 126 while `claude/e-recode-auto` exists; 8 after it is deleted. | K27 |
| C7 | 43 `0x` literals, 13 distinct | 42 whole lowercase literals, 12 distinct. The section's pattern also takes the lowercase head of `0x100000001B3` (`bench/scripts/charts.py:77`). None resolves either way. | K14 |

Addition to class 6: the three hashes are not lost. GitHub serves each by
full or short hash although no ref holds it. A fetch by hash would resolve
them; a fetch of branches and pull refs does not.

## Redaction (rule 4)

No problem found. The section holds no email address, no URL, no session
link and no token-like string.

```
grep -c -E '@[A-Za-z0-9.-]+[.][A-Za-z]{2,}|:[/][/]|claude[.]ai|session[_][0-9A-Za-z]{6,}|gh[pousr]_[A-Za-z0-9]{20,}|github[_]pat|BEGIN [A-Z ]*PRIVATE KEY' citations.md     # 0
```

## Figures without a command

- 1,175,587, the total of hex tokens among the rows' numbers. The command given prints distinct tokens by object type only.
- 43, the total of `0x` literals. The command prints distinct tokens only.
- "one backticked 4-digit line number (`3486`)".
- "The 2,271 figure captions name nine commits": the count of nine.
- "109 of them under two spellings".
- The pull request number of each class 1 commit, and the list "#1 to #20, #27 to #37".
- `bench/src/gate.rs:386` and `bench/src/figures.rs:383`: no command shows the two lines.
- "each sit one commit above a main squash commit (#28 to #36)".
- The subject of the commit that added the `14f8cc6` row.
- "about 4,525 distinct body lines" for #35 and #36 (4,528 and 4,524; the command gives only the 4,505 shared).
- "At `8025423` it was 112 blobs, 51.0 MiB": the commands name `b3a043dc` only.
- "two `claude/*` leftovers" in the plan table.

## The plan's figures

Each of the eight plan figures in the section's table was re-measured, and I
re-measured each by another route (rows 27 to 31 above, and R17, R18 of the
`refs` review). Seven agree. The bank figure is right for the fetched refs
and stale for GitHub (C4). The brief's "33 squash commits" was also
re-measured by the section: main is 32 commits, one root and 31 squash
commits (`git rev-list --parents b3a043dc | awk '{n++; if (NF==1) r++} END {print n, r}'` prints `32 1`).

## Commands

```
K1  python3 release/tools/citations.py 2>/dev/null | cmp - release/citations.tsv && echo same
K2  for c in $(git log --format=%h --abbrev=7 b3a043dc); do n=$(git grep -I -o -w -E "$c[0-9a-f]*" b3a043dc -- . | wc -l); m=$(git ls-tree -r --name-only b3a043dc | grep -c -- "-$c"); [ $((n+m)) -gt 0 ] && echo "$c $((n+m))"; done
K3  python3 -c "
import re,subprocess as s,collections as C
g=lambda *a:s.run(('git',)+a,capture_output=True,text=True,errors='replace').stdout
A=g('rev-list','--all').split();M=set(g('rev-list','b3a043dc').split());d=C.defaultdict(list)
for c in A:d[c[:7]].append(c)
T=re.compile(r'(?<![0-9A-Za-z_])[0-9a-f]{7,40}(?![0-9A-Za-z_])');n=C.Counter();h=C.defaultdict(set);L=C.Counter()
for t in T.findall(g('grep','-I','-h','-o','-w','-E','[0-9a-f]{7,40}','b3a043dc','--','.'))+T.findall(g('ls-tree','-r','--name-only','b3a043dc')):
  r=[c for c in d.get(t[:7],[]) if c.startswith(t)]
  if len(r)==1:k='main' if r[0] in M else 'not main';n[k]+=1;h[k].add(r[0]);L[len(t)]+=1
print({k:(len(h[k]),n[k]) for k in n},dict(L))"
      # {'main': (8, 78), 'not main': (334, 3046)} {8: 12, 7: 2984, 40: 128}
K4  python3 -c "
import subprocess as s,collections as C
g=lambda *a:s.run(('git',)+a,capture_output=True,text=True,errors='replace').stdout
d=C.defaultdict(list)
for c in g('rev-list','--all').split():d[c[:7]].append(c)
f=set()
for l in g('grep','-I','-o','-w','-E','[0-9a-f]{7,40}','b3a043dc','--','.').split('\n')[:-1]:
  _,p,t=l.split(':',2)
  if len([c for c in d.get(t[:7],[]) if c.startswith(t)])==1:f.add(p)
print(len(f),dict(C.Counter('figures' if p.startswith('docs/figures/') else 'rows' if p.startswith('bench/runs/') else 'text' for p in f)))"
K5  cut -f1 release/citations.tsv | tail -n +2 | while read t; do [ $(git rev-parse --disambiguate=$t | wc -l) -gt 1 ] && echo $t; done | wc -l
K6  for t in 18aefe7 52273051bfc36d3ff3c5e541e0692fcba99eaf01 14f8cc68292f5aa838486b690ab722303fafcc3c; do git rev-parse --disambiguate=$t | wc -l; done
K7  for f in $(git ls-tree -r --name-only b3a043dc -- bench/runs); do n=${f##*-}; n=${n%.json}; git show b3a043dc:$f | grep -m1 -o '"sha": "[^"]*"' | cut -d'"' -f4 | awk -v n=$n '{print (index($0,n)==1 ? "prefix" : "MISMATCH"), substr($0,41)}'; done | sort | uniq -c
K8  git grep -h -o -E '"sha": "[0-9a-f]{40}' b3a043dc -- bench/runs | cut -c9-48 | while read s; do if git merge-base --is-ancestor $s b3a043dc 2>/dev/null; then echo main; elif [ -n "$(git for-each-ref --contains $s refs/remotes/pull 2>/dev/null | head -1)" ]; then echo pull-only; else echo no-ref; fi; done | sort | uniq -c
K9  for c in e40d6283 09cadec0 99d44afa eceb66d6 dbc2a2a0 5b8d64ae d5679e45 37ea8f2a; do git merge-base --is-ancestor $c refs/remotes/pull/37 && echo y; done | wc -l
K10 awk -F'\t' '$3==2{print $2}' release/citations.tsv | sort -u | while read c; do r=$(git for-each-ref --contains $c --format='%(refname:lstrip=3)' refs/remotes/pull); [ $(echo "$r" | wc -l) = 1 ] && echo $r; done | sort -n | uniq -c
K11 awk -F'\t' '$3==2{print $2}' release/citations.tsv | sort -u | while read c; do git merge-base --is-ancestor $c origin/claude/e-recode-auto && echo y; done | wc -l
K12 git grep -h -v -E '"(sha|rustc)":' b3a043dc -- bench/runs | grep -o -w -E '[0-9a-f]{7,40}' | wc -l          # total; add "| sort -u" before wc for distinct
    git grep -h -v -E '"(sha|rustc)":' b3a043dc -- bench/runs | grep -o -w -E '[0-9a-f]{7,40}' | sort -u | awk 'NR==FNR {p[$1]; next} (substr($1,1,7) in p)' <(git rev-list --all | cut -c1-7) - | while read t; do git rev-parse -q --verify "$t^{commit}"; done | wc -l     # 0
    python3 -c "
import subprocess as s,re,bisect
g=lambda *a:s.run(('git',)+a,capture_output=True,text=True).stdout
o=sorted({l.split(' ')[0] for l in g('rev-list','--all','--objects').split('\n') if l})
t=set(re.findall(r'(?<![0-9A-Za-z_])[0-9a-f]{7,40}(?![0-9A-Za-z_])',g('grep','-h','-v','-E','\"(sha|rustc)\":','b3a043dc','--','bench/runs')))
print(sum(1 for x in t if (lambda i:i<len(o) and o[i].startswith(x))(bisect.bisect_left(o,x))))"     # 29
K13 git grep -I -h -o -w -E '[0-9a-f]{7,40}' b3a043dc -- '*.rs' '*.sh' '*.py' '*.yml' '*.toml' '*.mjs' '*.h' '*.c' '*.cc' '*.lock' | wc -l
    git grep -I -h -o -w -E '[0-9a-f]{7,40}' b3a043dc -- '*.rs' '*.sh' '*.py' '*.yml' '*.toml' '*.mjs' '*.h' '*.c' '*.cc' '*.lock' | sort | uniq -c | while read n t; do git rev-parse -q --verify "$t^{commit}" >/dev/null && echo $n; done | paste -sd+ | bc
K14 git grep -h -I -o -E '0x[0-9a-fA-F]+' b3a043dc -- . ':!bench/runs' | grep -x -E '0x[0-9a-f]{7,40}' | wc -l          # add "| sort -u" before wc for distinct
K15 awk -F'\t' 'NR>1 && $3<=2 {n[$2]++; l[$2]=l[$2]" "length($1)} END {for (h in n) if (n[h]>1) print l[h]}' release/citations.tsv | sort | uniq -c
K16 git rev-list --all --objects | cut -c1-40 | sort -u | awk '{n=0; while (n<40 && substr($0,1,n+1)==substr(p,1,n+1)) n++; if (n>m) m=n; p=$0} END {print NR" objects need "m+1}'
    git rev-list --all | sort | awk '{n=0; while (n<40 && substr($0,1,n+1)==substr(p,1,n+1)) n++; if (n>m) m=n; p=$0} END {print NR" commits need "m+1}'
K17 for t in 02c2d64 170042b 186e0ef 77ae295 825c0f4 a2f3908 a4dc700 c78fc75 f4c5cb4; do echo "$(git rev-parse --disambiguate=${t:0:5} | wc -l)/$(git rev-parse --disambiguate=${t:0:6} | wc -l)"; done | sort | uniq -c     # 9 2/1
K18 git ls-tree -r --name-only b3a043dc | grep -v -E '^(bench/runs|docs/figures)/' | while read f; do git show "b3a043dc:$f"; done | grep -a -o -E 'at `[0-9a-f]{7,40}`' | wc -l
    git grep -I -n -o -w -E 'at `(e40d628|09cadec|99d44af|eceb66d|dbc2a2a|5b8d64a|d5679e4|37ea8f2)[0-9a-f]*`' b3a043dc | cut -d: -f2,3
K19 git grep -I -c -E '\(`CLAUDE\.md`,' b3a043dc | awk -F: '{n++; s+=$3} END {print n" files, "s" matches"}'
K20 git grep -I -o -E '[0-9]{8}T[0-9]{6}Z-[0-9a-f]{7,40}' b3a043dc -- . ':!bench/runs' ':!docs/figures' | cut -d: -f2 | sort | uniq -c
K21 git grep -h -o -E 'rustc [0-9.]+ \([0-9a-f]+' b3a043dc -- bench/runs | awk '{print $3}' | sort | uniq -c
K22 for c in $(git rev-list b3a043dc); do echo "$(git cat-file commit $c | sed '1,/^$/d' | wc -c) $(git log -1 --format=%s $c | grep -o -E '#[0-9]+\)$')"; done | sort -rn | awk '{t+=$1} NR<=3 {print} END {print "total", t}'
    git cat-file commit 8025423b | sed '1,/^$/d' | wc -c
K23 git rev-list b3a043dc | while read c; do git cat-file commit $c | sed '1,/^$/d'; done | grep -c '^Co-Authored-By:'     # likewise '^Co-authored-by:' and '^Claude-Session:'; "grep '^Claude-Session:' | sort -u | wc -l" for the links
K24 for c in $(git rev-list b3a043dc); do git ls-tree -r -l $c; done | awk '{print $3, $4, $5}' | sort -u -k1,1 | awk '{t+=$2; n++; if ($3 ~ /^bench\/runs\//) {r+=$2; m++}} END {printf "%d blobs %.1f MiB; bench/runs %d blobs %.1f MiB\n", n, t/1048576, m, r/1048576}'
K25 for r in $(git for-each-ref --format='%(refname)' refs/remotes/origin/bank); do p=$(git rev-parse $r^); echo "$(git rev-parse --short=8 $r) $(git diff-tree -r --no-renames --diff-filter=A --name-only $p $r -- bench/runs | wc -l) $(git diff-tree -r --no-renames --diff-filter=A --name-only $p $r -- bench/runs | grep -c -- "-${p:0:7}.json") $(git log -1 --format=%s $p | grep -o -E '#[0-9]+\)$')"; done
K26 for r in $(git for-each-ref --format='%(refname:short)' refs/remotes/origin/bank); do p=$(git rev-parse --short=7 $r^); git grep -l -I -w -E "$p[0-9a-f]*" $r -- docs/figures | wc -l; done
    git grep -l -I -w -E '(e40d628|09cadec|99d44af|eceb66d|dbc2a2a|5b8d64a|d5679e4|37ea8f2)[0-9a-f]*' b3a043dc -- docs/figures | wc -l     # 0
K27 awk -F'\t' 'NR>1 && $3<=2 {print $2}' release/citations.tsv | sort -u | comm -12 - <(git rev-list origin/main origin/claude/e-recode-auto origin/claude/wal-ahead-beside-wip $(git for-each-ref --format='%(refname)' refs/remotes/origin/bank) | sort) | wc -l
K28 for t in <spelling>; do git cat-file -t $t; git rev-parse --verify $t^{commit}; git merge-base --is-ancestor $t origin/main; git for-each-ref --contains $t --format='%(refname:lstrip=3)' refs/remotes/pull; done
      # the sample: every class 1, 6 and 7 line of release/citations.tsv, and the class 2 lines picked by: awk -F'\t' 'NR>1 && $3==2' release/citations.tsv | awk 'NR%17==3'
K29 for f in CLAUDE.md ROADMAP.md docs/engine.md; do git show b3a043dc:$f | grep -o -E '\b[0-9a-f]{7,40}\b' | while read t; do git rev-parse -q --verify "$t^{commit}" >/dev/null && echo commit || { case $t in *[a-f]*) echo "no-object $t";; *) echo number;; esac; }; done | sort | uniq -c; done
```
