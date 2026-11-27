# Pre-tag passes: status

The coordinator's record for the passes between the feature freeze and the
tag. One row a pass: owner, branch or pull request, state, and what it
waits on. Working records live under `release/`; nothing here is open work
of the engine's, which goes in `ROADMAP.md`.

Coordination branch: `claude/supdb-pretag-coordination-4qj5t4`.
Main when this was last read: `b3a043dc` (#37), tree `7f58d776`
(`git rev-parse origin/main 'origin/main^{tree}'`).

## Passes

| Phase | Pass | Owner | Branch or PR | State | Waits on |
|---|---|---|---|---|---|
| 0 | Feature freeze | Bright | none | not declared: #37 landed on main on 2026-11-27 at 14:35 ET | Bright (decision 10) |
| 0 | History audit (A) | coordinator's subagents | this branch, `release/history.md` | running, read-only | nothing |
| 0 | History decision (B) | Bright | none | waits for the audit | decision 8 |
| 0 | Release-candidate branch, C0 | coordinator | not created | not started | the freeze, decisions 8 and 11 |
| 0 | Baseline at C0 | measurement agent | `release/baseline.md` | preparation running: static counts, commands, no timing | C0 |
| 1 | Delete: options ledger, dead code, API surface | not assigned | `release/options.csv` | the ledger's mechanical columns are being generated | C0, decision 1 |
| 2 | Reshape: split `src/db.rs`, style | not assigned | none | not started | phase 1 |
| 3 | Verify: audits, then fixes | not assigned | `release/findings/` | not started | phase 2, decisions 2, 4, 7 |
| 4 | Tests: flake loop, runtime, coverage | not assigned | none | not started | the last fix of phase 3, decision 5 |
| 5 | Words: roadmap, docs, release files | not assigned | none | begun on main by #37 (`CLAUDE.md` as rules, the roadmap as short items) | the last fix of phase 3, decision 6 |
| 6 | Measure at the last code commit | measurement agent | none | not started | phase 5 |
| 7 | Tag and hand-off | Bright tags | none | not started | phase 6, decision 3 |

## Decisions needed from Bright

The plan's nine, and three the coordinator adds.

| # | Decision | Sets | State |
|---|---|---|---|
| 1 | Experimental options: stay in the public `Options` type, or move to a sub-struct or a feature | phase 1 | open; the ledger's counts are being prepared for it |
| 2 | Is encryption in this tag | phase 3's scope | open; encryption is on main since #35, so the default is yes |
| 3 | Version and tag name; whether to publish to crates.io | phases 5 and 7 | open; `Cargo.toml` says 0.1.0 |
| 4 | The format compatibility promise that starts at the tag | phase 3 | open |
| 5 | Gate runtime budget and the number of flake-loop runs | phase 4 | open |
| 6 | Target size of `CLAUDE.md`, and what stays in it | phase 5 | largely made by #37 (24.7 KB of rules); confirm, and the same for `bench/CLAUDE.md` (80 KB) |
| 7 | Whether to commission the outside crypto review | phase 3 | open |
| 8 | History: keep and fix forward, re-message main, or publish from a fresh repository | phase 0, before C0 | open; needs `release/history.md`; with no decision the plan's default is to keep and fix forward |
| 9 | Whether commits on main keep the `Co-Authored-By` and `Claude-Session` trailers | every pull request from the freeze on | open |
| 10 | Is main frozen at #37, and are `claude/e-recode-auto` and `claude/wal-ahead-beside-wip` parked | C0 | open |
| 11 | The release-candidate branch's name, and leave for the coordinator to push it and open pull requests into it | every pass | open; this session may push only to its own branch |
| 12 | Whether `release/` goes into the release candidate or stays on the coordination branch | the tag's contents | open; default: stays on the coordination branch |

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
