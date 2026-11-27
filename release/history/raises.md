# Gap 1: the remap and the `lists` group

Measured at `b3a043dc`. `P` is main's 32 commits at seven characters each.
`T` stands for the commit a re-message makes of `b3a043dc`: another hash, the
same tree. `R` is the remap commit, made on `T`.

## What holds

- `scripts/raises.txt` has 149 raise lines. 62 of them name a main commit,
  at 63 sites, all in the reason column. [G1]
- By commit: `eceb66d` (#33) 46 sites, `dbc2a2a` (#29) 16, `5b8d64a` (#28) 1.
  48 of the reasons call the commit main's and 15 call it upstream. [G1]
- The file came to main in #35 with all 149 lines, and it is one blob at
  #35, #36 and #37. `scripts/ratchet.py` and the four lists it registers came
  with it. Main's 29 older commits hold none of them. [G2]
- A declaration is five columns, the reason the fifth. Declarations are
  compared as a multiset. One that is in the tree and not at the base has to
  match an observed raise, unless its list is new since the base. [G3]
- `refs.md` says the check does not match the reason column. The match is on
  four columns and the identity on five, so an edited reason is a new
  declaration.

## Verdicts

The table is `scripts/ratchet.py`'s own `main`, evaluated in memory. The tree
is `b3a043dc`'s files with `raises.txt` changed as the row says and the two
reasons in `tests/data/cost-entries.txt` edited; each hash is replaced by
`0000000`. Nothing is written, and no gate group was run. [G4]

| `raises.txt` in `R` | base `b3a043dc` (the tree of `T`) | base `99d44afa` (#34) | base `ab972300` (root) |
|---|---|---|---|
| the 63 sites edited | RED: 62 declarations match no raise, 87 spent | green: 149 not held yet | green: 149 not held yet |
| the 62 lines deleted | green: 87 spent | green: 87 not held yet | green: 87 not held yet |
| all 149 lines deleted | green | green | green |
| left as it is | green: 149 spent | green: 149 not held yet | green: 149 not held yet |

No list grows in any row: each of the four reads 0 raised. The 62 red lines
are 58 for `cost-entries.txt`, 3 for `door.txt` and 1 for `couplings.txt`.

## The base each event hands `R`

`scripts/base.sh` prints the merge-base of HEAD and its candidate. [G5]

| how `R` reaches main | base | edited lines |
|---|---|---|
| pull request into the re-messaged main | `T`, the request's base | RED |
| push after the force-push, a squash merge included | `T`, the push's before | RED |
| tag pushed at `R` | `HEAD~1`, which is `T` | RED |
| inside the force-push | merge-base of `R` and `b3a043dc`: the root if the root keeps its hash, none if it does not (exit 2) | green, or exit 2 |
| no event | merge-base of `R` and `origin/main`: `T` after the force-push, the root or none before it | RED after; green or exit 2 before |

`R`'s own run confirms the remap only where its base is `T`. Inside the
force-push the run holds nothing of `raises.txt`, whatever the file says. The
confirming commands at `R` are `sh scripts/ratchet.sh <T>` and
`sh scripts/ratchet.sh ab972300`. Both need git and python3 only. What else
the force-push's own run meets is the critic's gap 5.

## What the remap does with the 62 lines

They cannot be edited. The choices that are green against every base:

| choice | sites left naming an old main hash | note |
|---|---|---|
| delete the 62 | 0 | 87 lines stay: 20 cite a commit a pull ref holds, 67 cite none [G6] |
| delete all 149 | 0 | The file is its 14 header lines, 802 bytes. `scripts/ratchet-red.sh` copies the header only. [G7] |
| leave | 63 | The three hashes stay reachable from `refs/pull/37/head`. 48 reasons then call main's a commit that is not on main. [G8] |

Either deletion is green against `b3a043dc` today. It can land as an
ordinary change before the owner decides, and it is the same change under
all three options. Only `scripts/ratchet.py` reads the raise lines. [G7]
The 149 lines stay readable in the trees of #35, #36 and #37, which a
re-message keeps. Under option 3 the 63 sites, and the 20, would name
commits the fresh repository does not hold.

## The edit list: 15 sites

| where | sites | what reads it |
|---|---|---|
| `CLAUDE.md` 9; `ROADMAP.md` 523, 552, 572; `docs/engine.md` 4636, 4654, 9066 | 9 on 7 lines | No gate opens these files. [G9] |
| `tests/data/cost-entries.txt` 364, 368 | 2 | `lists` reads column 1 and whether column 2 is `-`. `tests/source.rs` wants a reason that is not empty. [G10] |
| `bench/runs/ooc/20261125T014102Z-5b8d64a.json`, `bench/runs/ooc/20261125T044116Z-5b8d64a.json` | 4: two file names, two 40-character `sha` fields | the `bench` group [G11] |

- `eceb66d6` (#33) is cited in `raises.txt` alone. The remap is of seven
  commits, not eight.
- `bench figures` draws the latest row of each class by `utc`, and a figure
  names the short hash of the row it is drawn from. No figure names
  `5b8d64a`, so neither row is drawn. `every_banked_row_keeps_its_class`
  parses every row and holds 52 of them by name; neither row is among them.
  [G11]
- The two rows can also stay as they are. 126 of main's 130 rows already
  record a commit that a pull ref holds and main does not (`citations.md`,
  "Where the sites are"), and `5b8d64ae` would be one more. The list is then
  11 sites on 9 lines in four files, and six commits.

## Not run

- `sh scripts/check.sh lists`. Its red input makes scratch repositories.
- The `bench` group, which needs the compiler. What is said of the two rows
  is by reading.

## Commands

Run from `/home/user/supdb`. G4 and G6 read the working tree's `scripts/` and
`tests/data/`, which are `b3a043dc`'s
(`git diff --quiet b3a043dc -- scripts tests/data && echo same`).

```sh
P=$(git rev-list b3a043dc | cut -c1-7 | paste -sd'|')
# G1: raise lines; lines naming a main commit; sites; sites outside the reason column; by commit; by wording
git show b3a043dc:scripts/raises.txt | grep -v '^#' | grep -c .
git show b3a043dc:scripts/raises.txt | grep -v '^#' | grep -c -E "\b($P)"
git show b3a043dc:scripts/raises.txt | grep -v '^#' | grep -o -E "\b($P)" | wc -l
git show b3a043dc:scripts/raises.txt | grep -v '^#' | grep . | cut -f1-4 | grep -c -E "\b($P)"
git show b3a043dc:scripts/raises.txt | grep -v '^#' | grep -o -E "\b($P)" | sort | uniq -c
git show b3a043dc:scripts/raises.txt | grep -v '^#' | grep -o -E "(main|upstream)('s)?[^a-z]{0,12}(eceb66d|dbc2a2a|5b8d64a)" | awk '{print $1}' | sort | uniq -c
# G2: main commits that touch the file; its blob at #37, #36, #35; what each base holds
git log --format=%h b3a043dc -- scripts/raises.txt
git rev-parse b3a043dc:scripts/raises.txt e40d6283:scripts/raises.txt 09cadec0:scripts/raises.txt
for c in $(git rev-list b3a043dc); do n=0; for p in tests/data/couplings.txt tests/data/door.txt tests/data/door-libc.txt tests/data/cost-entries.txt scripts/raises.txt scripts/ratchet.py; do git cat-file -e $c:$p 2>/dev/null && n=$((n+1)); done; echo $n; done | sort | uniq -c
# G3: the identity, the multiset, the match
git show b3a043dc:scripts/ratchet.py | sed -n '36,42p;205,218p;279,296p'
# G4: the verdict table
V='import re,sys,subprocess as s; g=lambda *a: s.run(["git",*a],capture_output=True,text=True).stdout; n={"__name__":"ratchet","__file__":"scripts/ratchet.py"}; exec(g("show","b3a043dc:scripts/ratchet.py"),n); P=r"\b(%s)" % "|".join(h[:7] for h in g("rev-list","b3a043dc").split()); how,base=sys.argv[1:]; e=lambda t: re.sub(P,"0000000",t); f={"edit":e,"delete62":lambda t: "".join(l for l in t.splitlines(True) if l[0]=="#" or not re.search(P,l)),"delete149":lambda t: "".join(l for l in t.splitlines(True) if l[0]=="#"),"leave":lambda t: t}[how]; m={n["RAISES"]:f,"tests/data/cost-entries.txt":e}; b=n["at_base"]; n["in_tree"]=lambda root,p: m.get(p,lambda t:t)(b(root,"b3a043dc",p)); sys.exit(n["main"](".",base))'
for base in b3a043dc 99d44afa ab972300; do for how in edit delete62 delete149 leave; do o=$(python3 -c "$V" $how $base); echo "$how $base: $(echo "$o" | tail -1), $(echo "$o" | grep -c 'matches no raise') no raise, $(echo "$o" | grep -c 'spent (at the base)') spent, $(echo "$o" | grep -c 'not held yet (') not held yet"; done; done
python3 -c "$V" leave b3a043dc | grep -E '^  tests/data'
python3 -c "$V" edit b3a043dc | grep 'matches no raise' | sed -E 's/declares (\S+) .*/\1/' | sort | uniq -c
# G5: the candidates; what CI passes; the events
git show b3a043dc:scripts/base.sh | sed -n '42,62p'
git show b3a043dc:.github/workflows/ci.yml | sed -n '10,14p;166,170p'
# G6: raise lines by the classes of the commits they cite (1 main, 2 pull ref); 149 less the three counts is 67
python3 release/tools/citations.py --sites 2>/dev/null | awk -F'\t' '$1=="scripts/raises.txt"{l[$2]=l[$2] $4} END{for(k in l) n[l[k]]++; for(k in n) print n[k], k}'
# G7: header lines and bytes; the red input's copy; the files that name raises.txt
git show b3a043dc:scripts/raises.txt | grep -c '^#'; git show b3a043dc:scripts/raises.txt | grep '^#' | wc -c
git show b3a043dc:scripts/ratchet-red.sh | sed -n '160p'
git grep -l -F raises.txt b3a043dc | cut -d: -f2
# G8
for h in eceb66d dbc2a2a 5b8d64a; do git merge-base --is-ancestor $h refs/remotes/pull/37 && echo $h; done | wc -l
# G9: the 11 text sites; gate lines that open the three documents
git grep -I -n -o -E "\b($P)[0-9a-f]*" b3a043dc -- . ':!bench/runs' ':!docs/figures' ':!scripts/raises.txt' | cut -d: -f2-
git grep -n -E '(read_to_string|open|cat|grep|awk|sed)[^#\n]*(ROADMAP\.md|CLAUDE\.md|engine\.md)' b3a043dc -- tests scripts bench/scripts bench/src bench/tests .github | wc -l
# G10
git show b3a043dc:scripts/ratchet.py | sed -n '122,138p'; git show b3a043dc:tests/source.rs | sed -n '4400,4411p'
# G11: the rows and their sha; how a figure picks and names its row; figures and sources that name the rows
for f in $(git ls-tree -r --name-only b3a043dc -- bench/runs | grep 5b8d64a); do git show b3a043dc:$f | python3 -c "import json,sys; d=json.load(sys.stdin); print(len(d['sha']), d['sha'][:8])"; done
git show b3a043dc:bench/src/figures.rs | sed -n '178,190p;381,383p'; git show b3a043dc:bench/scripts/figures.sh | sed -n '30,35p'
git grep -l 5b8d64a b3a043dc -- docs/figures | wc -l; git grep -c 5b8d64a b3a043dc -- bench/src | wc -l
git show b3a043dc:bench/src/row.rs | sed -n '472,480p;562,574p'
git show b3a043dc:bench/src/row.rs | sed -n '480,540p' | grep -c -E '\("(quick|full|ooc)/[0-9]{8}T[0-9]{6}Z-[0-9a-f]{7}\.json"'; git show b3a043dc:bench/src/row.rs | sed -n '480,540p' | grep -c -E '\("ooc/'
```
