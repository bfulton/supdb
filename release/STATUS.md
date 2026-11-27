# Pre-tag passes: status

The coordinator's record for the passes between the feature freeze and the
tag. One row a pass: owner, branch or pull request, state, and what it
waits on. Working records live under `release/`; nothing here is open work
of the engine's, which goes in `ROADMAP.md`.

Coordination branch: `claude/supdb-pretag-coordination-4qj5t4`.
C0, the freeze commit: `b3a043dc` (#37), tree `7f58d776`
(`git rev-parse origin/release-candidate 'origin/release-candidate^{tree}'`).
Passes land as pull requests into `release-candidate`; Bright merges them,
and merges that branch to main for the tag.

## Passes

| Phase | Pass | Owner | Branch or PR | State | Waits on |
|---|---|---|---|---|---|
| 0 | Feature freeze | Bright | none | declared by Bright on 2026-11-27 with main at `b3a043dc`; a redo is accepted if main moves | nothing |
| 0 | History audit (A) | coordinator's subagents | this branch, `release/history.md` | running, read-only | nothing |
| 0 | History decision (B) | Bright | none | the plan's default is taken: keep history and fix forward; the audit's costs for the other two go to Bright with it | nothing, unless Bright overrules |
| 0 | Release-candidate branch, C0 | coordinator | `release-candidate`, cut at C0 = `b3a043dc` (tree `7f58d776`) | done; `ci` was green on all nine legs at that commit (push run 46057759527) | nothing |
| 0 | Baseline at C0 | measurement agent, in a cloud session of its own | `release/baseline.md` | static counts and commands prepared; the rows and the noise floor are about 39 hours of runs that need a machine to themselves, so they run apart from the passes | the run sheet's verification |
| 1 | Delete: options ledger, dead code, API surface | not assigned | `release/options.csv` | the ledger's mechanical columns are being generated | C0, decision 1 |
| 2 | Reshape: split `src/db.rs`, style | not assigned | none | not started | phase 1 |
| 3 | Verify: audits, then fixes | not assigned | `release/findings/` | not started | phase 2, decisions 2, 4, 7 |
| 4 | Tests: flake loop, runtime, coverage | not assigned | none | not started | the last fix of phase 3, decision 5 |
| 5 | Words: roadmap, docs, release files | not assigned | none | begun on main by #37 (`CLAUDE.md` as rules, the roadmap as short items) | the last fix of phase 3, decision 6 |
| 6 | Measure at the last code commit | measurement agent | none | not started | phase 5 |
| 7 | Tag and hand-off | Bright tags | none | not started | phase 6, decision 3 |

## Decisions needed from Bright

The plan's nine, and three the coordinator added. Questions of naming, layout
and convention are the coordinator's to settle by the default; what is
still open below is what changes what ships.

| # | Decision | Sets | State |
|---|---|---|---|
| 1 | Experimental options: stay in the public `Options` type, or move to a sub-struct or a feature | phase 1 | open; the ledger's counts are being prepared for it |
| 2 | Is encryption in this tag | phase 3's scope | yes: the tag is of main, and encryption is on main since #35 |
| 3 | Version and tag name; whether to publish to crates.io | phases 5 and 7 | open; `Cargo.toml` says 0.1.0 |
| 4 | The format compatibility promise that starts at the tag | phase 3 | open |
| 5 | Gate runtime budget and the number of flake-loop runs | phase 4 | open |
| 6 | Target size of `CLAUDE.md`, and what stays in it | phase 5 | largely made by #37 (24.7 KB of rules); confirm, and the same for `bench/CLAUDE.md` (80 KB) |
| 7 | Whether to commission the outside crypto review | phase 3 | open |
| 8 | History: keep and fix forward, re-message main, or publish from a fresh repository | phase 0, before C0 | the default, keep and fix forward, unless Bright overrules after reading `release/history.md` |
| 9 | Whether commits on main keep the `Co-Authored-By` and `Claude-Session` trailers | every pull request from the freeze on | the default stays, and no rule about trailers is written |
| 10 | Is main frozen at #37, and are `claude/e-recode-auto` and `claude/wal-ahead-beside-wip` parked | C0 | frozen at `b3a043dc`; both branches carry nothing main lacks (the audit's finding) and are left for Bright to delete |
| 11 | The release-candidate branch's name | every pass | `release-candidate` |
| 12 | Whether `release/` goes into the release candidate or stays on the coordination branch | the tag's contents | stays on the coordination branch, out of main and out of the tag |

## Where the plan and the repository differ

The plan was written against main at `8025423`. The repository's rules win
where they differ, and each difference is listed once here.

- Main is five pull requests later (#33 to #37). Encryption and the store
  in one file landed as #35. The plan's figures are re-measured in
  `release/baseline.md` and `release/history.md`.
- The plan's phase 5 cuts `CLAUDE.md` to rules and moves the incident
  catalog to `docs/`. #37 cut it to rules and points at the file as it
  stood at `e40d6283` instead. That pointer names a commit on main, so a
  re-message of main (decision 8, option 2) would have to remap it.
- The plan names `scripts/policy_defaults.py`. The file is
  `bench/scripts/policy_defaults.py`, run by the suite's lint in the
  `bench` group.
- The plan lists the gate's groups as build, test, stride, lint, wasm and
  bench. The default set is build, test, stride, onefile, lint, wasm,
  bench and lists (`scripts/check.sh`), and `lists` holds every commit
  after the base to a verified signature (`scripts/signed.sh`).
- The plan keeps findings under `release/findings/`. `CLAUDE.md` says open
  work goes in `ROADMAP.md` and nowhere else, so a finding still open when
  phase 5 starts moves to the roadmap and its file comes off.
- The plan counts three dependencies. `Cargo.toml` also has one
  dev-dependency, `ring`, the tests' and examples' cipher.
- The plan puts the wall time of each gate group and test binary in the
  baseline. Those are timings, so they are the measurement agent's, taken
  with the machine to itself, and are not in today's static counts.

## Escalations

None yet.
