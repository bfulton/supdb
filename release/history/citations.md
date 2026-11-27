# Cited commits

Measured at `b3a043dc` (tree `7f58d776`). The tool is `release/tools/citations.py`
and its plain output is `release/citations.tsv`. The tool reads the tracked
working tree with `release/` left out; that tree is `b3a043dc`'s
(`git diff --stat b3a043dc -- . ':!release'` prints nothing).

```
python3 release/tools/citations.py            > release/citations.tsv   # one line per hash as cited
python3 release/tools/citations.py --sites    # one line per citing site: file, line, hash_as_cited, class
python3 release/tools/citations.py --abbrev   # how short a hash may be cut
```

The summary below is the tool's stderr line. Its exit status is 1 today, for
class 6.

## What counts as a citation

A token is 7 to 40 lowercase hex characters with no letter, digit or
underscore beside it. It is a citation when it names exactly one commit of
this repository, or when its context says commit: a row's file name and its
`sha` field, a backticked token with a hex letter, or a token after `at`,
`commit`, `since`, `upstream`, `show` or `diff`. In a source file a resolving
token counts only in a comment or in a row's file name. A row is read as its
file name and its string fields; its numbers are never read.

What that leaves out, and what it would have matched:

| left out | tokens | resolve to a commit |
|---|---|---|
| numbers in rows | 1,175,587 (1,010,810 distinct; 29 match a blob or tree prefix by chance) | 0 |
| `0x` literals of 7 to 40 hex digits | 43 (13 distinct) | 0 |
| the other tokens in source files (test keys, fixtures, numbers) | 401 | 0 |

Source files hold 485 tokens. 84 resolve, and all 84 are citations: 32 on
`//` comment lines and 52 row file names in `bench/src/row.rs`. The tool's
count of constants left out is 0. Samples:

| site | text |
|---|---|
| `src/db.rs:1761` | ``/// `4eb6172`). The first sitting, with`` |
| `src/db.rs:52114` | ``// no line of it runs (the same count's `work` at `e002b04`).`` |
| `bench/examples/zrec.rs:13` | ``//! 10` at `e38e44b`). This probe writes the same records ...`` |
| `bench/src/row.rs:485` | `("full/20260927T214755Z-f87b394.json", XEON_280),` |
| `bench/src/row.rs:491` | `("quick/20260914T045949Z-7344833.json", XEON_280),` (a hash of digits alone) |

A token shorter than 7 characters is never read. One backticked 4-digit line
number (`3486`, `docs/zpages-design.md:1034`) is a commit prefix by chance.

```
git grep -h -v -E '"(sha|rustc)":' b3a043dc -- bench/runs | grep -o -P '(?<![0-9A-Za-z_])[0-9a-f]{7,40}(?![0-9A-Za-z_])' | sort -u | git cat-file --batch-check | awk '{print ($NF=="missing") ? $NF : $2}' | sort | uniq -c
git grep -h -I -o -E '0x[0-9a-f]{7,40}' b3a043dc -- . ':!bench/runs' | sed 's/^0x//' | sort -u | git cat-file --batch-check | awk '{print $NF}' | sort | uniq -c
git grep -h -I -o -P '(?<![0-9A-Za-z_])[0-9a-f]{7,40}(?![0-9A-Za-z_])' b3a043dc -- '*.rs' '*.sh' '*.py' '*.yml' '*.toml' '*.mjs' '*.h' '*.c' '*.cc' '*.lock' | wc -l
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$1 ~ /\.rs$/ {print $1":"$2}' | while IFS=: read f l; do sed -n "${l}p" "$f"; done | grep -c -E '^\s*//|[0-9]{8}T[0-9]{6}Z-'
```

## Counts by class

| class | where it resolves | hashes | spellings | sites |
|---|---|---|---|---|
| 1 | a commit on `origin/main`'s first-parent line | 8 | 10 | 78 |
| 2 | reachable from a pull ref, not on main | 334 | 441 | 3,046 |
| 3 | reachable only from another branch | 0 | 0 | 0 |
| 4 | a non-commit object | 0 | 0 | 0 |
| 5 | ambiguous as written | 0 | 0 | 0 |
| 6 | cited as a commit, in no fetched ref | 3 | 5 | 5 |
| 7 | another repository's | 6 | 6 | 132 |
| all | | 351 | 462 | 3,261 |

The sites are in 2,417 files. 342 commits are cited at 3,124 sites in 2,413
files; 109 of them under two spellings (a row's 7-character name and its
40-character `sha`).

```
python3 release/tools/citations.py > /dev/null          # the stderr line: hashes and sites by class
awk -F'\t' 'NR>1 {n[$3]++; s[$3]+=$6} END {for (c in n) print c, n[c], s[c]}' release/citations.tsv
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' 'NR>1 {n++; f[$1]++} END {print n, length(f)}'
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$4<=2 {n++; f[$1]++} END {print n, length(f)}'
```

Where the sites are:

| source | class 1 | class 2 | class 6 | class 7 |
|---|---|---|---|---|
| `docs/engine.md` | 3 | 348 | 1 | |
| `ROADMAP.md` | 5 | 23 | | |
| `CLAUDE.md` | 1 | | | |
| `bench/CLAUDE.md`, `bench/DESIGN.md` | | 10 | | |
| `docs/runs-form-design.md`, `docs/zpages-design.md` | | 32 | | |
| `experiments/ffi/results-2026-09-29.md` | | 1 | | |
| `scripts/raises.txt` | 63 | 20 | | |
| `tests/data/cost-entries.txt`, `tests/data/couplings.txt` | 2 | 5 | | |
| `src/db.rs`, `bench/examples/zrec.rs` (comments) | | 32 | | |
| `bench/src/row.rs` (row file names) | | 52 | | |
| rows: file name | 2 | 126 | 2 | |
| rows: `sha` and `rustc` fields | 2 | 126 | 2 | 130 |
| `docs/figures/**.svg` (one caption each) | | 2,271 | | |
| `bench/vendor/faster/PIN`, `bench/Cargo.lock` | | | | 2 |

```
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' 'NR>1 {g=$1; if (g ~ /^bench\/runs\//) g=($2==0 ? "rows: file name" : "rows: field"); else if (g ~ /^docs\/figures\//) g="docs/figures"; c[g"\t"$4]++} END {for (k in c) print c[k]"\t"k}' | sort -k2
```

The 2,271 figure captions name nine commits, all class 2. The figures are
generated from the rows.

## Class 1: the main-line commits cited

This is the remap list of option 2. Main's first-parent line holds 32
commits: 31 squash merges (#1 to #20, #27 to #37) and the initial commit.
Eight are cited; 24, the head `b3a043dc` among them, are cited nowhere in the
tree.

| commit | pull request | cited as | sites | files | also reachable from pull refs |
|---|---|---|---|---|---|
| `e40d6283` | #36 | `e40d6283` | 1 | `CLAUDE.md:9` | 37 |
| `09cadec0` | #35 | `09cadec0`, `09cadec` | 3 | `ROADMAP.md` (523, 552, 572) | 36-37 |
| `99d44afa` | #34 | `99d44af` | 2 | `ROADMAP.md` (552, 572) | 35-37 |
| `eceb66d6` | #33 | `eceb66d` | 46 | `scripts/raises.txt` | 34-37 |
| `dbc2a2a0` | #29 | `dbc2a2a` | 18 | `scripts/raises.txt` 16, `tests/data/cost-entries.txt` 2 | 30-37 |
| `5b8d64ae` | #28 | `5b8d64a`, 40 characters | 5 | two rows under `bench/runs/ooc` (name and `sha` each), `scripts/raises.txt:80` | 29-37 |
| `d5679e45` | #13 | `d5679e4` | 1 | `docs/engine.md:9066` | 10, 14-37 |
| `37ea8f2a` | #6 | `37ea8f2` | 2 | `docs/engine.md` (4636, 4654) | 7-37 |

Total: 8 commits, 10 spellings, 78 sites, 7 files. 65 of the 78 sites are
reason text in the gate's lists (`scripts/raises.txt` 63,
`tests/data/cost-entries.txt` 2).

Every one of the eight is an ancestor of `refs/remotes/pull/37`. Of main's 32
commits, 31 are reachable from a pull ref; the head `b3a043dc` is the one
that is not.

```
awk -F'\t' '$3==1' release/citations.tsv
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$4==1 {n++; f[$1]++} END {print n" sites in "length(f)" files"; for (k in f) print f[k], k}'
git rev-list --first-parent b3a043dc | wc -l; git log --first-parent --format=%s b3a043dc | grep -c -E '\(#[0-9]+\)$'
for c in $(git rev-list --first-parent b3a043dc); do [ -z "$(git for-each-ref --contains $c refs/remotes/pull | head -1)" ] && git log -1 --format=%h $c; done
```

## Class 2: commits on pull refs

334 commits, 3,046 sites. No rewrite of main touches them. They resolve only
in a checkout that has fetched `refs/pull/*/head`.

- 113 of the 334 are reachable from exactly one pull ref: 89 from #6, 20
  from #33, 2 from #13, 1 from #20, 1 from #26.
- One cited commit, `f3bc29c` (`bench/CLAUDE.md:1120`), is reachable only
  from the ref of #26, a pull request that did not merge.
- 118 of the 334 are also reachable from `origin/claude/e-recode-auto`.
  Class 3 is empty: no citation depends on a branch alone, so deleting the
  two `claude/*` leftovers and the nine `bank/quick-*` branches breaks none.

```
awk -F'\t' '$3==2 && $4 !~ /[,-]/ {print $2" "$4}' release/citations.tsv | sort -u | awk '{c[$2]++} END {for (k in c) print "#"k": "c[k]}'
awk -F'\t' '$3==2 && $4=="26"' release/citations.tsv
git rev-list origin/claude/e-recode-auto | sort | comm -12 - <(awk -F'\t' '$3==2 {print $2}' release/citations.tsv | sort -u) | wc -l
```

## Rows

130 row files. Every file name is the start of its `sha` field. Five quick
rows of September carry `-dirty` after the hash.

| directory | rows | name a main commit | a pull-ref commit | another-branch commit | nothing |
|---|---|---|---|---|---|
| `bench/runs/full` | 8 | 0 | 8 | 0 | 0 |
| `bench/runs/ooc` | 34 | 2 | 32 | 0 | 0 |
| `bench/runs/quick` | 86 | 0 | 84 | 0 | 2 |
| `bench/runs/killed/ooc` | 2 | 0 | 2 | 0 | 0 |
| all | 130 | 2 | 126 | 0 | 2 |

The rows name 109 distinct commits. The two that name a main commit are
`bench/runs/ooc/20261125T014102Z-5b8d64a.json` and
`bench/runs/ooc/20261125T044116Z-5b8d64a.json` (#28's squash commit).

Hash-valued fields in a row are `sha` and the compiler's own hash inside
`rustc` (class 7). The `build` stamp is a flags string and holds no hash.

No file under `bench/src` runs git. `bench/build.rs` stamps
`git rev-parse HEAD`, `bench/src/gate.rs:386` compares `sha` for identity, and
`bench/src/figures.rs:383` prints its first seven characters.

Not in the tree: the nine `origin/bank/quick-*` branches each sit one commit
above a main squash commit (#28 to #36) and each add three quick rows, 27 in
all. All 27 are named for a main commit, and none is in main's tree.

```
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$2==0 {sub(/\/[^\/]*$/,"",$1); c[$1" class "$4]++} END {for (k in c) print c[k], k}' | sort -k2
python3 release/tools/citations.py > /dev/null          # "rows whose name and sha disagree: 0"
git grep -h -o -E '"sha": "[0-9a-f]{40}[^"]+"' b3a043dc -- bench/runs | sed -E 's/"sha": "[0-9a-f]{40}//' | sort | uniq -c
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$2==0 {print $3}' | sort -u | wc -l
git grep -n -E 'Command::new\("git|rev-parse|is-ancestor|merge-base' b3a043dc -- bench/src | wc -l
for b in $(git for-each-ref --format='%(refname:short)' refs/remotes/origin/bank); do git diff --no-renames --name-only --diff-filter=A b3a043dc $b -- bench/runs; done | sed -E 's/.*-([0-9a-f]+)\.json/\1/' | sort | uniq -c
```

## Class 5 and class 6

Class 5: none. Every spelling names one object.

Class 6: three hashes, five sites. Each is broken today under any option.

| site | cited as | context |
|---|---|---|
| `docs/engine.md:4744` | `18aefe7` | "... the `supdb-upkeep` thread, at `18aefe7`)" |
| `bench/runs/quick/20261124T050458Z-5227305.json` | `5227305` (file name) | row |
| the same file, line 3 | `52273051bfc36d3ff3c5e541e0692fcba99eaf01` | `sha` |
| `bench/runs/quick/20261125T090158Z-14f8cc6.json` | `14f8cc6` (file name) | row |
| the same file, line 3 | `14f8cc68292f5aa838486b690ab722303fafcc3c` | `sha` |

The commit that added the second row has the subject "bench: the quick row at
14f8cc6 (main's code on #26's build)". No other file names either row or
either hash.

```
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$4==5 || $4==6'
git grep -n -I -E '5227305|14f8cc6|18aefe7' b3a043dc -- . ':!bench/runs'
```

## Abbreviations

| spelling length | spellings | sites |
|---|---|---|
| 7 | 336 | 2,984 |
| 8 | 8 | 12 |
| 40 | 107 | 128 |

The shortest length at which no two objects here share a prefix is 9; for
commits alone it is 6. `git rev-parse --short b3a043dc` prints 8 characters.
The object count is the checkout's and moves with every new commit.

No citation is one character from ambiguity. Nine, each 7 characters and
class 2, are two characters from it: their commits need 6 characters among
all objects. They are `02c2d64`, `170042b`, `186e0ef`, `77ae295`, `825c0f4`,
`a2f3908`, `a4dc700`, `c78fc75`, `f4c5cb4`.

```
awk -F'\t' 'NR>1 && $3<=3 {n[length($1)]++; s[length($1)]+=$6} END {for (k in n) print k, n[k], s[k]}' release/citations.tsv
python3 release/tools/citations.py --abbrev 2>/dev/null
```

## Class 7: other repositories

| hash | sites | where |
|---|---|---|
| `332ea7da532a688acf648ee60c4f63a1caeaf1f7` | 1 | `bench/vendor/faster/PIN` (FASTER) |
| `e13ca993e8ccb9ba9847cc330696e02839f328f7` | 1 | `bench/Cargo.lock:814`, the version of `tikv-jemalloc-sys` |
| `e408947bf`, `2d8144b78`, `b940084d7`, `0ef1bc386` | 52, 24, 30, 24 | the `rustc` field of the rows |

No workflow pins an action by hash, and no Cargo manifest names a git source.

```
awk -F'\t' '$3==7 {print $1, $6}' release/citations.tsv
git grep -n -E 'uses: .*@[0-9a-f]{40}' b3a043dc -- .github | wc -l; git grep -n -E 'git\+https|rev = "' b3a043dc -- '*.lock' '*.toml' | wc -l
```

## Pointer forms a re-message touches

| form | count | note |
|---|---|---|
| "this file as it stood at `e40d6283`" | 1, `CLAUDE.md:9` | The hash is #36's squash commit, class 1. |
| (`CLAUDE.md`, *entry*) | 20 pointers in 8 files (21 matches, one the line that defines the form) | Each resolves through `CLAUDE.md:9`. One hash carries all 20. |
| a command or a statement "at `<hash>`" | 358 sites | 5 of them name a main commit: `CLAUDE.md:9`, `ROADMAP.md:552` twice, `docs/engine.md` 4636 and 9066. |
| `git diff <hash> <hash>` | 3 | `ROADMAP.md:572` compares two main commits (`99d44af`, `09cadec`). The trees do not change, so the diff is the same under the new hashes. |
| `git show <hash>:<path>` | 2 | Both class 2. |
| "main #30-#33 (`eceb66d`'s merge)", "upstream `dbc2a2a`", "main `5b8d64a` (#28)" | 63 | Reason text in `scripts/raises.txt`. Its header says a line the base already has is spent and may be removed by any later change. |
| a row's file name and `sha` | 2 rows | Above. |
| a row named by file name elsewhere | 52 in `bench/src/row.rs`, 7 in docs | All class 2. Two more matches are test fixtures (`bench/src/row.rs`, `bench/src/ledger.rs`) and name no row. |
| a pull request by number (`#27`, `refs/pull/<n>/head`) | | Unchanged by a re-message. |

```
git grep -n 'as it stood at' b3a043dc
git grep -I -o -E '\(`CLAUDE\.md`,' b3a043dc | wc -l; git grep -I -l -E '\(`CLAUDE\.md`,' b3a043dc | wc -l
git grep -I -o -E 'at `[0-9a-f]{7,40}`' b3a043dc | wc -l
git grep -I -o -n -E 'at `(e40d6283|09cadec0?|99d44af|eceb66d|dbc2a2a|5b8d64a|d5679e4|37ea8f2)`' b3a043dc
git grep -I -o -E 'git (diff|show) [0-9a-f]{7,40}' b3a043dc
git grep -I -o -E '[0-9]{8}T[0-9]{6}Z-[0-9a-f]{7,40}' b3a043dc -- . ':!bench/runs' ':!docs/figures' | awk -F: '{c[$2]++} END {for (k in c) print c[k], k}'
```

## What a remap would touch

Option 1 (keep history): no hash moves. The five class 6 sites stay to be
fixed forward.

Option 2 (re-message main):

- The remap list is 8 commits, 10 spellings, 78 sites in 7 files: `CLAUDE.md`
  1, `ROADMAP.md` 5, `docs/engine.md` 3, `scripts/raises.txt` 63,
  `tests/data/cost-entries.txt` 2, and two rows.
- Two row file NAMES would change, both `...-5b8d64a.json` under
  `bench/runs/ooc`, with the `sha` field inside each. No doc, figure or
  source file names those two rows, and no figure caption names a main
  commit.
- Without a remap no citation stops resolving. All eight old hashes stay
  reachable from `refs/pull/37/head`. They would then name commits that are
  no longer on main.
- 334 commits at 3,046 sites are class 2 and do not move.
- The nine `bank/quick-*` branches would sit on main's old commits. Banking
  their 27 rows first adds 27 row files (54 sites, 27 file names) to the
  list.

Option 3 (a fresh repository): none of the 342 cited commits exists there.
3,124 sites in 2,413 files (2,271 figure captions, 128 rows, 14 text files)
would resolve only against the lab repository's pull refs.

Under every option the check needs the pull refs: a clone of the branches
alone resolves 8 of the 342 commits from main.

## Plan figures re-measured at `b3a043dc`

| plan figure (at `8025423`) | at `b3a043dc` | stale |
|---|---|---|
| one squash body is 339 KB (#27) | #27 is 339,159 bytes. #35 is 345,381 and #36 is 344,967. Main's 32 messages total 1,842,844 bytes. | yes: no longer the largest |
| #32 changes 120 lines in two files and carries 94 KB | 2 files, 120 insertions, 1 deletion; 94,579 bytes | no |
| four of the seven newest open with the same text | #29 to #32 still open with the same 400 bytes. They are four of the twelve newest. Of the seven newest (#31 to #37), two do. #35 and #36 share 4,505 of about 4,525 distinct body lines. | yes |
| 599 `Co-Authored-By`, 599 `Claude-Session`, seven distinct session links | over the same seven commits: 599, 599, 7. Over the seven newest now: 790, 790, 9. Over all 32: 1,542 (1,581 with 39 spelled `Co-authored-by`), 1,543, 15. | yes |
| 51 MB of file versions are rows under `bench/runs` | 130 blobs, 64.0 MiB (67.1 MB), of 193.5 MiB in 3,998 blobs. At `8025423` it was 112 blobs, 51.0 MiB. Each row has one version. | yes |
| nothing under `scratch/` is on main | 0 paths in main's history | no |
| two `claude/*` leftovers | `e-recode-auto`, `wal-ahead-beside-wip` | no |
| five unmerged `bank/quick-*` of nine | nine exist; all nine carry three rows absent from main's tree | yes |

```
for c in $(git rev-list --first-parent b3a043dc); do printf '%s %s\n' "$(git log -1 --format=%B $c | wc -c)" "$(git log -1 --format=%h $c)"; done | sort -rn | awk '{t+=$1} NR<=3 {print} END {print t, NR}'
git show --stat --format= 8025423b | tail -1; git log -1 --format=%B 8025423b | wc -c
for c in $(git rev-list --first-parent -12 b3a043dc); do echo "$(git log -1 --format=%b $c | head -c 400 | md5sum | cut -c1-8) $(git log -1 --format=%h $c)"; done
comm -12 <(git log -1 --format=%b 09cadec0 | sort -u) <(git log -1 --format=%b e40d6283 | sort -u) | wc -l
git log --first-parent --format=%B b3a043dc | grep -c '^Co-Authored-By:'; git log --first-parent --format=%B b3a043dc | grep -c -i '^Co-Authored-By:'; git log --first-parent --format=%B b3a043dc | grep -c '^Claude-Session:'; git log --first-parent --format=%B b3a043dc | grep '^Claude-Session:' | sort -u | wc -l
git log --first-parent -7 --format=%B 8025423b | grep -c '^Co-Authored-By:'; git log --first-parent -7 --format=%B 8025423b | grep '^Claude-Session:' | sort -u | wc -l
git log --first-parent -7 --format=%B b3a043dc | grep -c '^Co-Authored-By:'; git log --first-parent -7 --format=%B b3a043dc | grep -c '^Claude-Session:'; git log --first-parent -7 --format=%B b3a043dc | grep '^Claude-Session:' | sort -u | wc -l
git rev-list --objects b3a043dc -- bench/runs | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' | awk '$1=="blob" {s+=$2; n++} END {printf "%d blobs, %.1f MiB\n", n, s/1048576}'
git rev-list --objects b3a043dc | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' | awk '$1=="blob" {s+=$2; n++} END {printf "%d blobs, %.1f MiB\n", n, s/1048576}'
git rev-list --objects b3a043dc | grep -c ' scratch/'
for b in $(git for-each-ref --format='%(refname:short)' refs/remotes/origin/bank); do git diff --no-renames --name-only --diff-filter=A b3a043dc $b -- bench/runs | wc -l; done | sort | uniq -c
```

## Not covered

- Hashes cited inside commit messages and pull request descriptions. Only
  the tree was read.
- `release/` is left out of the scan. `release/STATUS.md`, added after
  `b3a043dc`, names main commits.
- The check was not run in a hosted checkout. It needs main's whole history
  and `refs/pull/*/head` fetched; without them every class 2 hash reads as
  class 6.
- Where the three class 6 commits went was not traced beyond the fetched
  refs.
