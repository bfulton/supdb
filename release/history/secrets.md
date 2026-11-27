# Secrets and private data

32 candidates from five scans were each judged twice: 1 real, 5 uncertain (4 distinct things), 26 benign.
No credential value was found in any ref, in any unreferenced commit that was read, in pull request text, or in any Actions record or log that was read. Nothing needs rotating before a decision about history. No escalation was written.
The real item is workflow configuration on main: it is fixed in the tree, not in history. The uncertain items are questions for the owner. A rewrite of main would remove none of the items from GitHub.

A candidate is real if either judge said real, benign if both said benign, uncertain otherwise. The 32 verdicts, with both judges' evidence, are in `$H/verdicts.json`. A value is shown as its first four characters and its length, `abcd[24]`. Commands run from `/home/user/supdb` at `b3a043dc` with:

    R="--exclude=origin/claude/supdb-pretag-coordination-4qj5t4 --remotes"
    H=/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/history/secrets
    python3 -I -c "import json,collections,sys; print(collections.Counter(e['verdict'] for e in json.load(open(sys.argv[1]))))" $H/verdicts.json   # benign 26, uncertain 5, real 1

Reach classes: main-tree (in the tree of `b3a043dc`), main-history, other-ref-only (pull refs and other branches), commit-metadata, github-text, actions-log, workflow-config.

## What was scanned

| modality | scope | counts | result | cmd |
|---|---|---|---|---|
| Pattern scan: 48 families, 14 further patterns, and the literal values of 48 environment variables of the audit sandbox | every blob and commit object of the 50 remote refs | 2,023 commits (32 on main), 22,294 blobs, 2,776,522,646 bytes, 5,932 path names | 0 credentials; candidates P1 to P14 | a, b |
| gitleaks 8.16.0, 147 built-in rules, `--redact` | history by patch; history with merges against their first parent; the tree of `b3a043dc`; commit messages; the blobs its path allowlist skips | 1,957 commits; 2,007 commits; 2,630 files; 2,023 messages; 12,954 blobs | 0 findings in each of 5 runs; a synthetic control token is reported | c |
| Entropy scan | the same blobs and commits | 4,511 distinct tokens at or over the threshold | every token classified; candidates E1 to E3 | d |
| File names, key and signing material | 5,932 paths, 22,294 blobs, 2,023 commit headers | 12 key and signature patterns; 42 literal key sites in source | 0 credential-style file names; 0 key material; every literal key is a test, example or benchmark value | e |
| Encoded and command-line forms | the same blobs and commits | base64 of token prefixes, PEM and archives; `--token`-style flags | 0 | f |
| GitHub text | 37 of 37 pull requests: titles, bodies, 6 comments, 114 reviews, 94 review comments. There are 0 issues, releases and tags | 339,385 characters | 0 credentials; candidates G1, G7 | g |
| Workflow files | the 10 files at `b3a043dc`; all 48 versions, in any ref, of the 3 workflows that can use the owner's machine | | candidates G4 to G6 | h |
| Actions run records | 506 of 506 | 1,868,425 characters | 0 credentials; 493 hold commit trailers | i |
| Actions job logs | 35 logs saved, from the jobs of 506 runs | 9,122 lines, 880,286 characters | 0 credentials; token values are masked | j |
| Jobs that ran on the owner's machine | 35 jobs in 35 runs, 2026-08-24 to 2026-09-09 | logs: 10 read whole, 2 in part, 9 at the last line only, 10 gone (410), 4 never uploaded | names of the machine; no credential | k |
| Commits GitHub serves that no ref holds | 660, found from the 506 run records and from 3 hashes that main's tree cites | messages and identities of all 660, 913,887 bytes; trees checked for 182 | 0 credentials; 657 are copies of clone commits under other hashes | l |

    a  git rev-parse --symbolic $R | wc -l      # 50; 49 when the scans ran, origin/release-candidate (equal to main) is newer
       git rev-list $R --count; git rev-list --count origin/main      # 2023; 32
       git rev-list --objects $R | cut -d' ' -f1 | git cat-file --batch-check='%(objecttype) %(objectsize)' | awk '$1=="blob"{n++; s+=$2} END{print n, s}'
    b  python3 $H/pattern/report.py; python3 $H/pattern/selftest.py      # every pattern count, redacted; 48 red cases fire
    c  G="gitleaks detect --redact --no-banner --exit-code 0"; T=$H/scanner
       $G --source . --log-opts="$R" --report-path $T/gitleaks-A.json
       $G --source . --log-opts="$R --diff-merges=first-parent" --report-path $T/gitleaks-B.json
       git archive b3a043dc | tar -x -C $T/main-tree; $G --no-git --source $T/main-tree --report-path $T/gitleaks-C.json
       git log $R --format='%H%n%B' | $G --pipe --report-path $T/gitleaks-D.json
       python3 -I $T/allowlisted.py $R > $T/allowlisted-blobs.txt; git cat-file --batch < $T/allowlisted-blobs.txt | $G --pipe --report-path $T/gitleaks-E.json
       dpkg -s gitleaks | grep '^Version'; strings -n 8 /usr/bin/gitleaks | grep -c '^id = "'      # 8.16.0; 147
    d  python3 -I $H/scanner/entropy_scan.py $H/scanner $R; python3 -I $H/scanner/classify.py $H/scanner
    e  python3 -I $H/scanner/names_table.py $R; python3 -I $H/scanner/history_keys.py $H/scanner $R
       git cat-file --batch < $H/scanner/blobs.txt | LC_ALL=C grep -a -c -E 'PRIVATE KEY|BEGIN OPENSSH|BEGIN PGP|BEGIN CERTIFICATE|ssh-ed25519 AAAA|ssh-rsa AAAA|ecdsa-sha2-nistp|allowed_signers|allowedSignersFile|signingkey|AGE-SECRET-KEY|PuTTY-User-Key-File'      # 0
       git rev-list $R | git cat-file --batch | grep -a -E '^gpgsig ' | cut -c1-30 | sort | uniq -c      # 1,978 SSH, 35 PGP
    f  python3 -I $H/critic/extra_scan.py; sh $H/critic/cliflags.sh
    g  python3 $H/github/coverage.py $H/github/api; python3 $H/github/scan_prs.py $H/github/api $H/github/pr-scan-report.txt
    h  git ls-tree --name-only origin/main .github/workflows/ | wc -l; python3 $H/gap-2/wf_cond.py $H/critic/wf-blobs.txt
    i  python3 $H/gap-2/records.py $H/gap-2/api $H/critic/literals.txt; python3 -I $H/judge-real/r2/records_count.py $H/gap-2/api
    j  python3 -I $H/judge-real/r2/logscan.py $H/gap-2/api $H/github/api
    k  awk -F'\t' '$5=="self-hosted"' $H/gap-2/jobs.tsv | wc -l; python3 $H/gap-2/cover.py $H/gap-2
    l  python3 $H/gap-1/summary.py; python3 $H/gap-1/scan_text.py; sh $H/write/tree_union.sh $H

The GitHub side was read through the read-only API of this one repository; every result is saved under `$H/github/api`, `$H/gap-1/api` and `$H/gap-2/api`. Nothing outside the repository was fetched and no value was tested against a service.

## Real: 1

**G5. The job token of one workflow is wider than its jobs need.** Kind: workflow configuration; no credential value exists. Reach: workflow-config, main tree. Redacted form: none.

- `.github/workflows/bank-row.yml:24-25` grants `contents: write` and `pull-requests: write` to the whole workflow. They are the only two such lines on main.
- Only the `propose` job uses the token (line 140, then `git push` and `gh pr create` at lines 155-156).
- The three `row` legs hold the same token. Their checkout sets no `persist-credentials`, so the token stays for later steps; those steps run `Swatinem/rust-cache@v2`, a third party's action at a tag, and the suite's build.
- Triggers are push to main and dispatch, so no fork reaches it. The token is the run's automatic one: it ends with the job and the logs mask it.
- Its pushes land: 9 `bank/quick-*` branches, each one unsigned bot commit. The repository refuses the bot's pull request (log of job 140713260909).
- Judges: one real ("a real over-scoped token, not a leaked one"), one uncertain.
- Action, in the tree before the tag: workflow-level `contents: read`; the two write permissions on `propose` only; `persist-credentials: false` on the `row` checkout; `Swatinem/rust-cache` pinned to a commit. Nothing to rotate. For the owner: whether a tag rule keeps the Actions token from creating release tags.

      git grep -n -E 'contents: write|pull-requests: write' origin/main -- .github/workflows      # 2 lines
      git show origin/main:.github/workflows/bank-row.yml | grep -n -E 'uses:|github\.token|git push|gh pr create'
      git grep -c 'persist-credentials' origin/main -- .github/workflows | wc -l      # 0
      git for-each-ref refs/remotes/origin/bank | wc -l; git log -1 --format='%an %G?' origin/bank/quick-46050828509      # 9; bot, N

## Uncertain: 5 candidates, 4 things

**P1. The owner's personal e-mail address in commit author fields.** Reach: commit-metadata. Redacted form: `brig[23]`.

- It is the author address of 56 commits: 31 of main's 32, and 25 on pull refs only. GitHub's web merge is the committer of 35 and the sandbox of 21. It is in no file and no message.
- 35 of the 56 author dates carry a non-UTC offset (2 distinct offsets), which places the author in a region.
- The 22 unreferenced commits of gap1:X2 carry it as author and committer.
- Judges: one uncertain (three months of web merges point to a standing choice; only the owner can say), one benign (the owner's own identity on the owner's repository; `LICENSE` names the same person).
- Action: ask the owner before the tag. To keep it, nothing. To stop it from here on, GitHub's e-mail privacy setting. To remove it from main, 31 commits are rewritten, which changes every hash the docs cite and still leaves the address on the 25 pull-ref commits, the 22 unreferenced commits and GitHub's copies.

      git log $R --format=%ae | grep -v -c noreply; git log origin/main --format=%ae | grep -v -c noreply      # 56; 31
      git log $R --format='%ae %cn' | grep -v noreply | awk '{print $2}' | sort | uniq -c      # 21 Claude, 35 GitHub
      git log $R --format='%ae %ad' --date=format:%z | grep -v noreply | awk '{print ($2=="+0000")?"utc":"other"}' | sort | uniq -c      # 35 other, 21 utc

**P5 and E2. A claude.ai artifact link.** Reach: other-ref-only, one pull ref. Redacted form: `claude.ai/artifact/VnQp[22]`.

- One link, in two versions of a working note. Its location is in `$H/verdicts.json` and is left out here.
- The note says a page of benchmark charts was published to it. An artifact page is private unless its owner shares it, and a shared one opens by its address alone.
- Not read: whether the page is shared, and what it holds today.
- Judges: one uncertain, one benign.
- Action: the owner opens the artifact's sharing setting. Private: nothing. Shared and holding only the public charts: nothing, or unshare. Nothing removes the address from the pull ref.

**G4. A self-hosted runner that a public repository reaches by a bare label.** Reach: workflow-config (`quiet-bench.yml:98`, `runner-smoke.yml:32`, `.localmostrc:37-69`). Redacted form: none.

- Established: the repository has had a self-hosted runner in the `Default` group with the bare label `self-hosted`. 35 job records name such a runner (4 runner names, one machine). The two workflows that use it are dispatch-only. No version of them in any ref put a `pull_request` or `push` job on it, and the owner started all 50 dispatch runs.
- Established: `ci.yml` and `apple-silicon.yml` run on `pull_request`, so pull requests from forks are an expected event.
- Not established: whether a workflow that a fork's pull request adds would be given to this runner. That rests on the repository's approval setting for fork pull request workflows, on the runner app's own job filter, and on whether a runner is registered today. None is readable with the tools used. The last run that could use the runner is dated 2026-09-10.
- `.localmostrc` limits a job to 5 network hosts, 19 read paths and 1 write path. The write path is the account's own cargo directory, not a per-job one.
- No credential or private datum is exposed. Judges: both uncertain.
- Action, for the owner before the tag draws outside pull requests: require approval for every outside contributor's workflow run; confirm the runner app refuses jobs from `pull_request` events; narrow the write path to the registry and git caches; or remove the runner's registration while it is unused. A runner group limited to named workflows closes it whatever the settings.

      awk -F'\t' '$5=="self-hosted"' $H/gap-2/jobs.tsv | wc -l; python3 $H/gap-2/wf_cond.py $H/critic/wf-blobs.txt      # 35; no pull_request or push job on the machine
      python3 $H/gap-2/records.py $H/gap-2/api $H/critic/literals.txt; python3 -I $H/judge-real/gh_runs.py $H/github/api      # 50 dispatch runs, actors; last run 2026-09-10
      git grep -n -E '^\s*pull_request:' origin/main -- .github/workflows      # apple-silicon.yml, ci.yml, pr-caches.yml
      git show origin/main:.localmostrc | awk '/^ +allow:/{s="allow"} /^ +read:/{s="read"} /^ +write:/{s="write"} /^ +- /{n[s]++} END{print n["allow"], n["read"], n["write"]}'      # 5 19 1

**G6. Actions on moving refs, and a token scope that rests on a repository setting.** Reach: workflow-config, all ten files. Redacted form: none.

- 39 `uses:` lines and none pinned to a commit: `actions/checkout@v7` 15, `Swatinem/rust-cache@v2` 13 (the one third-party action), `actions/upload-artifact@v7` 6, `actions/cache/*@v6` 2, `actions/download-artifact@v8` 1, the owner's reusable workflow `bfulton/localmost/.github/workflows/check.yaml@main` 2.
- 5 of the 10 files declare no `permissions:` (`ci.yml`, `comparators.yml`, `cost.yml`, `ooc-hosted.yml`, `rocksdb-alloc.yml`). Their token is read-only today (log of job 140704291307), by a repository setting.
- No workflow names a secret or a variable, and none uses `pull_request_target` or `workflow_run`.
- Judges: one uncertain, one benign. No exposure exists today; one needs a third party's tag, or the owner's other repository, to change. A moved tag meets a write token only in `bank-row.yml` (G5) and the owner's machine only in the two dispatch workflows (G4).
- Action: hardening, not a blocker. Pin `Swatinem/rust-cache` and the reusable workflow to commits; add `permissions: contents: read` to the 5 files.

      git grep -h -o -E 'uses: *[^ #]+' origin/main -- .github/workflows | sort | uniq -c      # 39 lines, 7 distinct
      git grep -h -E 'uses: *[^ ]+@[0-9a-f]{40}' origin/main -- .github/workflows | wc -l      # 0
      git grep -L -E '^\s*permissions:' origin/main -- .github/workflows      # 5 files
      git grep -n -E 'secrets\.|vars\.|pull_request_target|workflow_run' origin/main -- .github/workflows | wc -l      # 0

## Benign: 26 candidates

| class | candidates | counts | reach | cmd |
|---|---|---|---|---|
| claude.ai session links, the tool's attribution trailer: an identifier with no token | P4, E3, G1, G2, gap1:X3, gap2:X5 | 1,996 of 2,023 commit messages, 27 of main's 32; 18 distinct ids, 15 on main; 2 of the ids in 31 blobs under `scratch/`; 30 of 37 pull request bodies (31 links, 10 distinct); 493 of 506 run records (1,505 links, 16 distinct) | commit-metadata, github-text, actions-log | m |
| Names of the owner's self-hosted machine: machine name `blue[8]`, 4 runner names `loca[34]`, paths under one local account whose name is the public login. The machine name is part of the runner name | P9, P10, G3, gap2:X1 to X4 | 2 commit messages, on pull/1 only; 35 job records; 235 paths in the saved self-hosted logs; 1 hosted watchdog log read of 37; 2 run records | commit-metadata, actions-log | n |
| Id of a disabled scheduled task, `trig_01N8[24]` | P6, E1 | 1 id in 3 blobs | other-ref-only (pull/35, pull/36) | o |
| A third party's addresses and name in a vendored licence header | P2 | `alek[24]`, `info[16]`, `Alek[14]`; `bench/vendor/lzav/lzav.h:14,18` | main-tree | p |
| Public mailing-list names inside archive URLs | P3 | `open[27]`, `open[26]`, 1 blob | other-ref-only | b |
| Sandbox ids: container ids in cgroup paths, session scratch directory ids | P7, P8 | 39 distinct (33 and 6); 4 lines in main's tree: `bench/src/bound.rs:856,858,872`, `docs/zpages-design.md:2051` | main-tree, other-ref-only | q |
| A gitlink to a tool worktree, with no content | P11 | 1 path, added and removed on pull/1 | other-ref-only | r |
| Names of two cloud-key environment variables, no value | P12 | 1 line in 1 blob | other-ref-only (pull/35, pull/36) | s |
| Persons named in public attributions: a licence line, an algorithm credit, a profiler's banner | P13 | `Brig[13]`, `Howa[14]`, `Jose[18]` | main-tree, other-ref-only | s |
| Names of other repositories of the owner | P14, G7 | `bfulton/localmost` on 3 lines of main's tree; `supdb-bench` at `bench/CLAUDE.md:21` and `bench/DESIGN.md:1462`; a drafts path in the body of #22 | main-tree, github-text | t |
| Unreferenced commits, and the owner's identity on 22 of them | gap1:X1, gap1:X2 | 660 commits; 615 carry a clone commit's message word for word, 42 differ only in cited commit ids, 3 have no twin; 52 file objects absent from the clone, each rebuilt from clone objects | no ref | l |

    m  git log $R --grep='claude.ai/code/session_' --format=%H | wc -l; git log origin/main --grep='claude.ai/code/session_' --format=%H | wc -l      # 1996; 27
       git log $R --format=%B | grep -o -E 'claude\.ai/code/session_[0-9A-Za-z]+' | sort -u | wc -l      # 18; 15 with origin/main
       python3 -I $H/judge-benign/prlinks.py $H/github/api $H/judge-benign; python3 -I $H/judge-real/r2/sessions_check.py $H/gap-2/api
    n  python3 $H/judge-real/r2/runner_check.py $H/gap-2/api $H/github/api; python3 $H/judge-real/r2/pathshapes.py $H/gap-2/api $H/github/api
       awk -F'\t' '$4=="pickup-watchdog" && $5=="hosted"' $H/gap-2/jobs.tsv | wc -l      # 37
    o  git for-each-ref --contains 68fb0d82 --format='%(refname:short)' refs/remotes; python3 -I $H/judge-real/scan_all.py $H/judge-real      # rows trig_id
    p  git grep -n 'Email:' origin/main -- bench/vendor/lzav/lzav.h
    q  git grep -n -E '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' origin/main -- . ':!bench/runs' | cut -d: -f2,3      # 4 lines
       python3 -I $H/scanner/classify.py $H/scanner      # class B2, 39 distinct ids
    r  git log $R -m --diff-filter=AD --name-status --format=%h -- .claude
    s  python3 $H/pattern/pass2.py; python3 $H/pattern/pass3.py
    t  git grep -n 'bfulton/localmost' origin/main | cut -d: -f2,3; git grep -n 'supdb-bench' origin/main -- bench/CLAUDE.md bench/DESIGN.md | cut -d: -f2,3

Matches that are no candidate, by scan:

- Pattern scan (cmd b; rows by `python3 $H/pattern/rows.py`): 3 no-reply and bot addresses, and every one of the 2,023 commits has one of them as committer; `/home/user` paths in 345 blobs and other sandbox and system paths in 57; one IPv4 value, loopback; 12 cloud ARN and endpoint strings, all templates or variables, with one account number, the public owner id of Ubuntu images; 23 distinct key and token assignments in 617 blobs, all code or declared test keys; 157 lock-file checksums, 113 commit ids of this repository, 55 other object ids, 74 SHA-256 digests; URLs of 23 hosts, claude.ai the only one that is not a public site; 188 banked row files, whose 22 machine keys hold no host, user, directory or environment field.
- Entropy scan (cmd d), 4,511 tokens: 121 commit ids of this repository, 157 lock-file checksums, 59 commit ids of rewritten or foreign history, 74 digests, 197 mangled symbols, 642 paths that hold a sandbox id, 3,220 paths, flags and identifiers made of words, 21 other literals, 20 in the candidates.
- The values of the 48 environment variables of the audit sandbox, 39 of them with sensitive names, occur in no blob and no commit, except one whose value is the loopback literal (`python3 $H/pattern/pass2.py`).
- GitHub text and sampled logs (cmd g; `python3 $H/github/scan_logs.py $H/github/api $H/github/log-scan-report.txt`): 80 product links with no session id, 179 GitHub URLs, 129 hosted-runner paths, 19 masked values, 5 loopback lines, 118 machine-written review texts.
- Run records and self-hosted logs (`python3 $H/gap-1/scan_text.py -v`; `python3 $H/gap-2/logs.py $H/gap-2/api $H/gap-2/jobs.tsv`): 290 commit ids, 529 GitHub URLs, 1,012 logins; 10 masked values, 16 proxy lines on loopback, 24 token-scope lines, all read-only.

## Other findings, for the later audits

Workflow configuration:

- `ci.yml` runs on the push of any tag, so pushing the release tag starts it. `bank-row.yml` does not run on tags. `git show origin/main:.github/workflows/ci.yml | sed -n '10,13p'`
- Each push to main makes `bank-row.yml` push one more `bank/quick-<run id>` branch. A tenth was pushed at 2026-11-27T20:02Z and is not in the clone (log of job 140713260909).
- Two workflows registered on GitHub, `zz-caches.yml` and `zz-probe.yml`, have files on no ref tip. Their 10 commits are reachable through pull/25 only, and their run logs are public. `git log $R --oneline -- .github/workflows/zz-caches.yml .github/workflows/zz-probe.yml | wc -l`
- The reusable workflow decides which runner the two dispatch workflows use. It reads a repository variable, `LOCALMOST_HEARTBEAT`, a timestamp that public logs print (log of job 103046877154).
- On 2026-08-30 the checkout action on the owner's machine copied the account's git configuration file into the job's temp directory (3 log lines, content not printed). Later logs show the sandbox refusing that read (4 lines). `python3 $H/gap-2/logs.py $H/gap-2/api $H/gap-2/jobs.tsv`
- Logs lapse 90 days after their job; job records and run records do not. Deleting the 35 runs with a self-hosted job removes their logs and records. `python3 $H/gap-2/cover.py $H/gap-2`

Crypto hygiene:

- Constant keys outside test modules: `[0x5a; 32]` at `examples/cipher_store.rs:122`, `bench/src/enc.rs:30`, `bench/src/engines.rs:4223` and `:16792`, `bench/examples/merge_cipher.rs:50`; `[7u8; 32]` at `examples/crypt_cost.rs:67`. Only `bench/src/enc.rs:30` says at the site what the key is. `git grep -n -E '\[0x5a; *32\]|\[7u8; *32\]' origin/main -- examples bench/src bench/examples`
- `supdb::crypt::encrypt_with_id` (`src/crypt.rs:396`) is public and takes the object id from its caller. A chunk's nonce is the id's first 8 bytes and the chunk index (`src/crypt.rs:344-350`), so one id used twice under one key repeats nonces. Its doc comment says what it is for.
- No literal nonce or salt and no key derivation is on a library path. Keys and ids come from the operating system (`src/db/vault.rs:407,421`, `src/crypt.rs:387`). `git grep -n -E 'getrandom|/dev/urandom' origin/main -- src/db/vault.rs src/crypt.rs`
- The LMDB comparator shim derives its nonce from the page number and the transaction id (`bench/shim/lmdb_enc.c:63-68`). It is benchmark code.

History and refs:

- Main was replaced once. Run 32578043042 is a push to main at `91436d52` on 2026-08-22, and that commit is not in main's history. Its 21 commits are among the 660 that GitHub serves by id.
- The 660 stay served whatever is done to main, to the pull refs or to the tag. Only GitHub Support can remove unreferenced commits.
- A rewrite of main leaves the old history on GitHub in 37 pull refs, the `bank/*` branches, pull request bodies and 506 run records.
- 10 commits carry no signature: the 9 bot commits on `bank/*` and main's root commit `ab972300`. `for c in $(git rev-list $R); do git cat-file commit $c | sed '/^$/q' | grep -q '^gpgsig' || echo $c; done | wc -l`
- Two banked rows in main's tree carry a `sha` that is no object of any fetched ref: `bench/runs/quick/20261124T050458Z-5227305.json` and `bench/runs/quick/20261125T090158Z-14f8cc6.json`. GitHub serves both commits by id.

## Not covered, and gaps that stay open

Critic's gaps (K1 to K10):

- K1, unreferenced commits: closed in part. All 660 messages and identities were scanned. The trees of 478 were checked by no reader; they are taken to equal their twins' trees, as the 182 checked ones do or were rebuilt to. Unreferenced commits that no run record names and no file cites cannot be listed with these tools.
- K2, logs of the self-hosted period: closed in part. 9 self-hosted logs that GitHub still serves were read at their last line only, one of them at its last 30 lines. They lapse between 2026-11-28 and 2026-12-01, and the last self-hosted log on 2026-12-08. 36 of the 37 hosted watchdog logs and most hosted logs were not read.
- K3, artifact contents: open. 5 artifacts were listed for 3 runs and none was opened. One was uploaded from the owner's machine.
- K4, refs and text newer than the fetch: open, for the last step before the tag. The tenth `bank/*` branch, the session branch and the audit report itself were scanned by no track. This section holds the session scratch path, an id of the P8 class, once: in the line that sets `H`.
- K5, state only the owner can read: open. Secret-scanning alerts and push protection, forks, the fork-approval setting, rulesets, the lists of secrets and variables, caches, wiki, Pages, Discussions, the edit history of pull request bodies (the body of #23 says it was corrected), commit comments, check output, step summaries.
- K6, whether linked things are open: open, the owner's. The artifact page (P5), the 18 sessions, and the visibility of `bfulton/localmost` and of the suite's former repository. If that repository is public and holds this project's earlier history, it needs the same audit.
- K7, cross-search: closed. The judges corrected P9: it names the machine, on pull/1 only. The saved GitHub text holds no artifact link, task id, container id or scratch path, and the owner's address in commit identity fields only. `sh $H/write/k7.sh $H`
- K8, commit date offsets: closed, measured under P1.
- K9, the plaintext of `tests/data/crypt-v1.bin` (2 versions): open. It is ciphertext under a key the tests declare, and no track decrypted it.
- K10, what the tag publishes beyond git: open, for the release step. Source archives hold every tracked file, and `Cargo.toml` sets no `publish`, `include` or `exclude`. `git show b3a043dc:Cargo.toml | grep -c -E '^(publish|include|exclude)'`

Limits of the scans themselves:

- One rule engine ran, gitleaks 8.16.0. A secret under 24 characters, built from words, or split across lines matches no pattern used.
- Persons named in prose outside a list of 286 first names and four surnames.
- Commit signature blocks were searched for the environment literals only. The API returns no signature block for the 660 unreferenced commits.
- Of the logs of 506 runs, 35 were saved. The text of the two other repositories was not read.
- Runs created after the listing, and GitHub's test-merge commits (`refs/pull/<n>/merge`).
