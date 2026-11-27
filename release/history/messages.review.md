# Review of `sections/messages.md`

Scope: main at `b3a043dc`, the 49 fetched refs without the coordination branch, and GitHub through read-only calls. Each "mine" value comes from a route the section does not use: stored commit objects (`git cat-file`) read with `sed`, `awk` and Python where the section reads `git log` and `git show` formats with `grep`; `git log` listings and set difference where it uses `rev-list` ranges; the GitHub API where it uses git.

Result: 29 claims re-derived. 28 agree. One is stale against the host: GitHub holds ten `bank/quick-*` branches, the checkout nine. No figure in the section is miscounted. Rule 4 holds. Seven groups of figures have no command.

A remote ref `origin/release-candidate` now exists in the checkout at `b3a043dc`. It adds no commit. The section's `X` leaves out the coordination branch only, so its "other 12 `origin/*` refs" is now 13 and C21 prints one more line, with 0 commits ahead. The commit counts under `X` do not change. This review's commands leave the new branch out. [V26]

## Checks

| # | Claim | Theirs | Mine | Cmd | Agree |
|---:|---|---|---|---|---|
| 1 | Commits on main; squash commits; roots; merges | 32; 31; 1; 0 | 32; 31; 1; 0. GitHub lists 32 | V1, G1 | yes |
| 2 | Per-commit table, 32 rows: body bytes, `* ` lines, three trailer counts | table in §2 | the same 32 rows | V2 | yes |
| 3 | Per-commit table: author dates (32 rows), files and lines added and removed (31 rows) | table in §2 | the same rows | V3 | yes |
| 4 | Body bytes of #35, #36, #27, #32 | 345,260; 344,869; 339,043; 94,494 | the same | V2 | yes |
| 5 | Message bytes on main | 1,842,844 | 1,842,812 stored, and 32 newlines that `--format=%B` adds: 1,842,844 | V4 | yes |
| 6 | Body bytes on main; bodies over 64 KiB | 1,839,658; 8 commits, 1,651,006 | 1,839,626 stored and 32 newlines: 1,839,658; 8, 1,651,006 | V5 | yes |
| 7 | Trailer lines on main: `Co-Authored-By`, `Co-authored-by`, `Claude-Session`, `Signed-off-by`; distinct session links | 1,542; 39; 1,543; 0; 15 | the same | V6 | yes |
| 8 | Seven newest at `b3a043dc`, and at `8025423` | 790, 790, 9 links; 599, 599, 7 links | the same | V6 | yes |
| 9 | Bytes in the trailer lines | 199,853 | 199,853 | V7 | yes |
| 10 | Repeated bytes: #36; #36 without trailer lines; #32; all of main; all of main without trailer lines | 334,950; 299,217; 90,813; 770,460; 633,305 | the same | V8 | yes |
| 11 | Opening text. First 300 body bytes of #29, #30, #31, #32 are identical; #35 and #36 differ from each other and from those | same; differ | one digest for the four, two others for #35 and #36 | V9 | yes |
| 12 | Non-merge commits a branch adds, and those the previous pull head holds: #36 against #35; #32 against #31 | 604, 603; 89, 88 | the same | V10 | yes |
| 13 | `* ` lines that are a branch commit's title: #27, #35, #36 | 239; 236; 235 | the same | V11 | yes |
| 14 | Non-merge commits among each branch's newest 250: #6, #27, #35, #36 | 241; 239; 235; 234 | the same | V12 | yes |
| 15 | Squash bodies whose `* ` count equals the branch's non-merge commits | 22 of the other 24; #6 (241 of 676) and #14 (0 of 1) do not | 22 equal; #6 and #14 differ with those counts | V13 | yes |
| 16 | Authors over every ref | Claude 1,958; Bright Fulton 56; github-actions[bot] 9 | the same, with the same redacted addresses | V14 | yes |
| 17 | Committers over every ref | Claude 1,979; GitHub 35; github-actions[bot] 9 | the same | V14 | yes |
| 18 | Signature headers over every ref; PGP key ids | 1,978 SSH; 35 PGP; 10 none; one key id | the same; all 35 PGP signatures carry the id the section prints | V15 | yes |
| 19 | Pull refs: commits not on main summed over 37 refs; distinct; their message bytes; #36, #32, #6 | 4,785; 1,981; 2,642,481; 632; 92; 691 | the same | V16 | yes |
| 20 | Session links in the pull refs' messages off main | 18 | 18 | V6 | yes |
| 21 | #36 and #32 as GitHub reports them: commits, files, lines added, lines removed; title equals the commit title | 632, 6, 147, 80; 92, 2, 120, 1; yes | the same | G2 | yes |
| 22 | Description bytes of #36 and #32 | 1,862; 1,869 | 1,862; 1,869. Neither has a session link; both end with the footer; #36 has no heading | G2, V17 | yes |
| 23 | Banked rows in main's history; at `8025423` | 130 blobs, 67,067,767; 112 blobs, 53,523,449 | the same | V18 | yes |
| 24 | Paths under `scratch/` in main's 32 trees | 0 | 0 | V19 | yes |
| 25 | `bank/quick-*` branches | nine, each one bot commit on a main commit, #28 to #36 | nine in the checkout. GitHub has ten: `bank/quick-46057759452` is one bot commit on `b3a043dc` (#37) | V20, G3 | no: stale |
| 26 | Commits on branches only | 10: nine bank commits and one tip | 10, the same commits | V21 | yes |
| 27 | Co-author names and counts on main; names over every ref; addresses | six names (689, 603, 171, 67, 38, 13); seven; one | the same | V22 | yes |
| 28 | Body-size quantiles, and commits under 1,024, 2,048, 4,096, 8,192 bytes | 1, 1, 1,857, 8,379, 42,193, 276,681, 345,260, mean 57,489; 6, 8, 13, 15 | the same | V23 | yes |
| 29 | Titles: quartiles with and without the suffix; over 72 and over 100; comma, ", and", semicolon, capital | 56, 77, 96, 116, 184; 50, 72, 90, 110, 179; 22, 11; 22, 15, 5, 29 | the same | V24 | yes |

Also checked and in agreement: squash body lines of 40 bytes or more that no message under the pull ref holds are 47 bytes for #32 and 0 for #36 [V25]; the description statistics in §6 and §9 (total 137,740; quartiles 2,696, 4,002, 5,675; 0, 6, 16, 29 under the four sizes; nine over 5,000) follow from the section's own table.

## Corrections

1. **`bank/quick-*` branches (§1 last row, and every "every ref" figure).** The host has a tenth branch, `bank/quick-46057759452`, head `6df1f587`, parent `b3a043dc`. It is one `github-actions[bot]` commit dated 2026-11-27 15:02 Eastern that adds three rows under `bench/runs/quick/` named for `b3a043d`, and figure files. The checkout does not hold it [V20, G3]. The row should read: ten exist and all ten are unmerged, on #28 to #37. The figures in §5 and §6 that count every ref (2,023 commits, nine bot commits, ten unsigned, ten on branches only) are right for the 49 fetched refs and each is one short of the host.
2. **The count of main.** The coordination brief says 33 squash commits. The section's 32 (31 squash commits and the root) is right by three routes [V1, G1].

No other figure needs a change.

## The plan's figures

§1 lists nine. Each has a re-measured value and a command. Eight re-measured values agree with this review; the ninth is correction 1. The plan itself is not in the checkout, so whether the nine are all of its figures on this subject was not checked.

## Redaction (rule 4)

- Full email addresses: none. Addresses appear as four characters and a length.
- Session links: none. The only `session_` text is the search pattern in C2, C12 and C15.
- Private URLs: none. No URL of any kind is in the file.
- Token-like strings: one, at line 278, a 16-character hexadecimal PGP key id. It identifies the public key that signed GitHub's merge commits. It is not a credential.
- Names: people are named by the name on their commits. Line 277 names two GitHub accounts, which are services.

## Figures without a command

1. Terms: "the other 12 `origin/*` refs and the 37 pull refs". C1 counts commits, not refs.
2. §2: "US Eastern for the 31 squash commits and UTC for the root". C2 prints the date without its zone. [V3 prints the zones: 26 at -0500, 5 at -0400, 1 at +0000.]
3. §2: that #21, #22, #23, #25 and #26 are squash commits of pull requests merged into a branch.
4. §5: the split of the 35 GitHub-committed commits into 31 on main and 4 elsewhere. C14 prints 35.
5. §6: the session-link column of the table, "25 of the 31 descriptions contain a session link", "30 end with a footer", and the heading counts. C17's Python prints number, title length and body bytes only.
6. §6 and §9: the description total, quartiles and under-N counts. They can be worked out from C17's list by hand; no command prints them.
7. §8: the shape counts. C18 says "`grep -c` of the pattern named in the table" and gives no pattern.

## Commands

Run from `/home/user/supdb`. Setup first.

```sh
R=$(git show-ref | awk '$2 ~ /^refs\/remotes\// && $2 !~ /supdb-pretag-coordination|origin\/release-candidate$/{print $2}'); B=$(echo "$R" | grep /origin/); P=$(echo "$R" | grep /pull/)
msg(){ git cat-file commit $1 | sed '1,/^$/d'; }            # the stored message
body(){ msg $1 | sed '1,/^$/d' | awk 1; }                    # the stored body, last line newline-terminated
bodyb(){ echo $(( $(msg $1 | sed '1,/^$/d' | wc -c) + 1 )); } # body bytes as --format=%b prints them
T='import sys,re;d=sys.stdin.buffer.read();print(*[len(re.findall(p,d)) for p in (rb"(?m)^Co-Authored-By:",rb"(?m)^Co-authored-by:",rb"(?m)^Claude-Session:",rb"(?mi)^Signed-off-by:")],len(set(re.findall(rb"claude\.ai/code/session_\w+",d))))'

# V1: commits, roots, merges; squash titles
git log --format=%P b3a043dc | awk '{n++; if(NF==0)r++; if(NF>1)m++} END{print n, r+0, m+0}'; git rev-list b3a043dc | while read c; do msg $c | awk 'NR==1'; done | grep -cE '\(#[0-9]+\)$'
# V2: per commit: hash, body bytes, "* " lines, Co-Authored-By, Co-authored-by, Claude-Session
git rev-list b3a043dc | while read c; do echo "$(echo $c | cut -c1-8) $(bodyb $c) $(body $c | LC_ALL=C awk '/^\* /{i++} /^Co-Authored-By:/{a++} /^Co-authored-by:/{b++} /^Claude-Session:/{s++} END{print i+0, a+0, b+0, s+0}')"; done
# V3: author dates in the commit's zone; the zones; files, lines added, lines removed
git rev-list b3a043dc | git cat-file --batch | python3 -c "import sys,re,datetime as D;print(*[D.datetime.fromtimestamp(int(t),D.timezone(D.timedelta(minutes=int(z[0]+'1')*(int(z[1:3])*60+int(z[3:]))))).date().isoformat() for t,z in re.findall(r'(?m)^author .*> (\d+) ([+-]\d{4})$',sys.stdin.buffer.read().decode('latin1'))],sep='\n')"
git rev-list b3a043dc | while read c; do git cat-file commit $c | sed -n '/^author /{s/.* //p;q}'; done | sort | uniq -c
git rev-list --min-parents=1 b3a043dc | while read c; do echo "$(echo $c | cut -c1-8) $(git diff --numstat $c^ $c | awk '{f++; a+=$1; d+=$2} END{print f+0, a+0, d+0}')"; done
# V4: stored message bytes, commits, sum
git rev-list b3a043dc | while read c; do msg $c | wc -c; done | awk '{s+=$1; n++} END{print s, n, s+n}'
# V5: stored body bytes, commits, sum; bodies over 64 KiB and their bytes
git rev-list b3a043dc | while read c; do msg $c | sed '1,/^$/d' | wc -c; done | awk '{s+=$1; n++; if($1+1>65536){k++; b+=$1+1}} END{print s, n, s+n, k, b}'
# V6: Co-Authored-By, Co-authored-by, Claude-Session, Signed-off-by, distinct links: main; seven newest; seven newest at 8025423; pull refs off main
git rev-list b3a043dc | git cat-file --batch | python3 -c "$T"
git rev-list -7 b3a043dc | git cat-file --batch | python3 -c "$T"
git rev-list -7 8025423 | git cat-file --batch | python3 -c "$T"
git rev-list $P ^b3a043dc | git cat-file --batch | python3 -c "$T"
# V7: bytes in trailer lines
git rev-list b3a043dc | while read c; do body $c; done | LC_ALL=C awk '/^(Co-Authored-By|Co-authored-by|Claude-Session): /{s+=length($0)+1} END{print s}'
# V8: repeated bytes of one commit (here #36), without trailer lines, and over main
rep(){ body $1 | LC_ALL=C awk 'length($0)>=40' | LC_ALL=C grep -Fxf <(for p in $(git rev-list $1^ 2>/dev/null); do body $p; done | LC_ALL=C awk 'length($0)>=40' | sort -u); }
rep e40d6283 | wc -c; rep e40d6283 | grep -vE '^(Co-Authored-By|Co-authored-by|Claude-Session): ' | wc -c; rep 8025423b | wc -c
for c in $(git rev-list b3a043dc); do rep $c; done | wc -c; for c in $(git rev-list b3a043dc); do rep $c; done | grep -vE '^(Co-Authored-By|Co-authored-by|Claude-Session): ' | wc -c
# V9: digest of the first 300 body bytes
for c in dbc2a2a0 5a5497be 3706d2bf 8025423b e40d6283 09cadec0; do echo "$c $(msg $c | sed '1,/^$/d' | head -c 300 | md5sum | cut -c1-8)"; done
# V10: non-merge commits #36 adds, and those under #35 (substitute 8025423b, 32, 31)
git log --no-merges --format=%H refs/remotes/pull/36 | grep -vcFf <(git log --format=%H e40d6283^); git log --no-merges --format=%H refs/remotes/pull/36 | grep -vFf <(git log --format=%H e40d6283^) | grep -cFf <(git log --format=%H refs/remotes/pull/35)
# V11: "* " lines that are a branch commit's title (substitute 09cadec0 and 35, e40d6283 and 36)
awk 'NR==FNR{t[$0]++;next} t[$0]-->0{n++} END{print n}' <(git log --no-merges --format=%s refs/remotes/pull/27 --not 7650d890^) <(body 7650d890 | sed -n 's/^\* //p')
# V12: non-merge commits among a branch's newest 250
for p in "37ea8f2a 6" "7650d890 27" "09cadec0 35" "e40d6283 36"; do set -- $p; echo "#$2 $(git log -250 --format=%p $1^..refs/remotes/pull/$2 | awk 'NF<2' | wc -l)"; done
# V13: "* " lines against the branch's non-merge commits, per squash commit
git rev-list b3a043dc | while read c; do n=$(msg $c | awk 'NR==1' | sed -nE 's/.*\(#([0-9]+)\)$/\1/p'); [ -n "$n" ] && echo "$n $(body $c | grep -c '^\* ') $(git log --no-merges --format=%H refs/remotes/pull/$n | grep -vcFf <(git log --format=%H $c^))"; done | awk '{print ($2==$3) ? "equal" : "differs #"$1" "$2" "$3}' | sort | uniq -c
# V14: authors, then committers, over every ref, addresses redacted
git rev-list $R | while read c; do git cat-file commit $c | sed -n '/^author /{p;q}'; done | sed -E 's/^author (.*) <([^>]*)>.*/\1|\2/' | sort | uniq -c | awk -F'|' '{print $1"|"substr($2,1,4)"["length($2)"]"}'
git rev-list $R | while read c; do git cat-file commit $c | sed -n '/^committer /{p;q}'; done | sed -E 's/^committer (.*) <([^>]*)>.*/\1|\2/' | sort | uniq -c | awk -F'|' '{print $1"|"substr($2,1,4)"["length($2)"]"}'
# V15: signature kinds over every ref; issuer key ids of the PGP signatures, redacted
git rev-list $R | while read c; do git cat-file commit $c | sed '/^$/q' | grep -o '^gpgsig -----BEGIN [A-Z]* SIGNATURE' || echo none; done | sort | uniq -c
git rev-list $R | git cat-file --batch | python3 -c "import sys,re,base64,collections;d=sys.stdin.buffer.read().decode('latin1');print(collections.Counter((lambda b:b[b.find(b'\x09\x10')+2:][:8].hex().upper()[:4]+'[16]')(base64.b64decode(''.join(l.strip() for l in s.split('\n') if l.strip() and ':' not in l and not l.strip().startswith('=')))) for s in re.findall(r'gpgsig -----BEGIN PGP SIGNATURE-----\n(.*?)\n -----END PGP',d,re.S)))"
# V16: pull refs: commits off main summed; distinct; one ref (here #36); message bytes of the distinct set
for n in $(seq 1 37); do git log --format=%H refs/remotes/pull/$n | grep -vcFf <(git log --format=%H b3a043dc); done | awk '{s+=$1} END{print s}'
for n in $(seq 1 37); do git log --format=%H refs/remotes/pull/$n; done | sort -u | grep -vcFf <(git log --format=%H b3a043dc)
git log --format=%H refs/remotes/pull/36 | grep -vcFf <(git log --format=%H b3a043dc)
for n in $(seq 1 37); do git log --format=%H refs/remotes/pull/$n; done | sort -u | grep -vFf <(git log --format=%H b3a043dc) | while read c; do msg $c | wc -c; done | awk '{s+=$1+1} END{print s}'
# V17: UTF-8 bytes, session links, headings of a description; $F holds the "body" member of G2's result as one JSON string
python3 -I -c "import json,sys,re;s=json.load(open(sys.argv[1]));print(len(s.encode()),len(re.findall(r'session_\w+',s)),len(re.findall(r'(?m)^#{1,6} ',s)))" "$F"
# V18: banked rows in main's history; at 8025423
git log --format=%H b3a043dc | while read c; do git ls-tree -r -l $c -- bench/runs; done | awk '{print $3, $4}' | sort -u | awk '{n++; s+=$2} END{print n, s}'
git log --format=%H 8025423 | while read c; do git ls-tree -r -l $c -- bench/runs; done | awk '{print $3, $4}' | sort -u | awk '{n++; s+=$2} END{print n, s}'
# V19: scratch/ in any of main's trees
git log --format=%H b3a043dc | while read c; do git ls-tree -d $c scratch; done | wc -l
# V20: bank branches in the checkout; the host's tenth is not here; no row on main is named for b3a043d
git show-ref | grep -c 'refs/remotes/origin/bank/quick-'; git cat-file -e 6df1f587af5138fa3fc80b3ffd6e03a7b1625f3d 2>/dev/null && echo present || echo absent; git ls-tree -r --name-only b3a043dc -- bench/runs | grep -c b3a043d
# V21: commits on branches only
git log --format=%H $B | sort -u | grep -vFf <(git log --format=%H $P | sort -u) | grep -vcFf <(git log --format=%H b3a043dc)
# V22: co-author names and distinct addresses on main; the same counts over every ref with $R
git rev-list b3a043dc | git cat-file --batch | python3 -c "import sys,re,collections;d=sys.stdin.buffer.read();print(collections.Counter(m.decode() for m in re.findall(rb'(?mi)^co-authored-by: *(.*?) *<',d)).most_common(),len(set(re.findall(rb'(?mi)^co-authored-by:.*<([^>]*)>',d))))"
# V23: body-size quantiles, mean, and counts under the four sizes
git rev-list b3a043dc | while read c; do bodyb $c; done | python3 -c "import sys,math;b=sorted(map(int,sys.stdin));q=lambda p:b[math.ceil(p*len(b))-1];print(b[0],q(.1),q(.25),q(.5),q(.75),q(.9),b[-1],sum(b)//len(b),[sum(x<N for x in b) for N in (1024,2048,4096,8192)])"
# V24: titles: count, quartiles with the suffix | without | over 72, over 100, comma, ", and", semicolon, capital
git rev-list b3a043dc | while read c; do msg $c | awk 'NR==1'; done | python3 -c "import sys,re,math;t=[l.rstrip('\n') for l in sys.stdin if re.search(r'\(#\d+\)$',l.rstrip())];a=sorted(len(x.encode()) for x in t);b=sorted(len(re.sub(r' \(#\d+\)$','',x).encode()) for x in t);q=lambda v,p:v[math.ceil(p*len(v))-1];print(len(t),a[0],q(a,.25),q(a,.5),q(a,.75),a[-1],'|',b[0],q(b,.25),q(b,.5),q(b,.75),b[-1],'|',sum(x>72 for x in b),sum(x>100 for x in b),sum(',' in x for x in t),sum(', and' in x for x in t),sum(';' in x for x in t),sum(x[0].isupper() for x in t))"
# V26: the new branch and its head; commits it has beyond main; commits under the section's X; origin refs beside the coordination branch
git show-ref | awk '$2 ~ /origin\/release-candidate$/{print substr($1,1,8), $2}'; git rev-list --count b3a043dc..refs/remotes/origin/release-candidate; git rev-list --count --exclude='origin/claude/supdb-pretag-coordination-*' --remotes; git for-each-ref --format='%(refname)' refs/remotes/origin | grep -vc supdb-pretag-coordination
# V25: squash body bytes no message under the pull ref holds (here #32; substitute e40d6283 and 36)
body 8025423b | LC_ALL=C awk 'length($0)>=40' | sed 's/^\* //' | LC_ALL=C sort -u | LC_ALL=C comm -23 - <(git log --format=%H refs/remotes/pull/32 | grep -vFf <(git log --format=%H 8025423b^) | while read c; do msg $c | awk 1; done | sed 's/^\* //' | LC_ALL=C sort -u) | wc -c
```

GitHub, read-only, through the GitHub MCP tools. These cannot be reproduced from the checkout alone.

```
G1  mcp__github__list_commits(owner="bfulton", repo="supdb", sha="b3a043dcad52f64b69af6590cafbeb94beb55d98", perPage=100, fields=["sha"])        -> 32 entries
G2  mcp__github__pull_request_read(method="get", owner="bfulton", repo="supdb", pullNumber=36)   and pullNumber=32                          -> commits, changed_files, additions, deletions, title, body
G3  mcp__github__list_branches(owner="bfulton", repo="supdb", perPage=100)                                                                  -> 15 branches, ten of them bank/quick-*
    mcp__github__list_commits(owner="bfulton", repo="supdb", sha="bank/quick-46057759452", perPage=2, fields=["sha"])                       -> 6df1f587, then b3a043dc
    mcp__github__get_commit(owner="bfulton", repo="supdb", sha="6df1f587af5138fa3fc80b3ffd6e03a7b1625f3d", detail="stats", perPage=100)    -> author, date, the three row files
```
