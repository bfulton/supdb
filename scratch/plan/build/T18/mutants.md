# T18 mutants

Runner: mutants.py (each applied to the tree, the command run, the verdict appended to
mutants.results as it came, the file put back from keep/ and checked by its sha256).
Killer for scripts: `sh scripts/ratchet-red.sh` (its failing cases from mutant-<name>.log).
Killer for the YAML test: `cargo test --profile checked --test source -- ci_ a_ci_step a_group_no`.
Survivors: none. The first round of the YAML test's mutants was killed partly for a reason that
was not the case's (every red edit replaced the wasm call, so "wasm runs in no CI job" stood in
every verdict); the edits now add a step and assert that message absent, and round 2 re-ran them.

| mutant | what | killed by |
|---|---|---|
| B1 | base.sh: no origin/main falls back silently to HEAD | b_no_origin, b_empty_events, l_unresolved |
| B2 | base.sh: shallow clone not checked | b_shallow |
| B3 | base.sh: all-zeros before taken as a sha | b_first_push |
| B4 | base.sh: the candidate printed, no merge-base | b_origin, b_origin_empty_events, b_unrelated |
| B5 | base.sh: PR base ahead of the explicit override | b_override |
| B6 | base.sh: an unresolvable candidate falls back to HEAD | b_override_none, b_pr_none, b_before_none, b_first_push_root |
| R1 | couplings growth read by line count, not the count column | l_raise, l_declared, l_raise_on_branch, l_raised_again |
| R2 | couplings key by line (scope in it), not the total by file/token/region | l_moved_scope, l_raise, l_new_key, l_declared |
| R3 | a declaration matched on list and key, numbers ignored | l_wrong_to, l_wrong_from |
| R4 | a declaration matched with its list ignored | l_wrong_list |
| R5 | a declaration matched by key prefix | l_wrong_key |
| R6 | unregistered-list rule dropped | l_unregistered, l_unregistered_tsv |
| R7 | unregistered rule by `.txt` name only | l_unregistered_tsv |
| R8 | a list removed passes | l_removed |
| R9 | spent declarations not subtracted | l_spent, l_spent_lowered, l_raised_again(_declared) |
| R10 | spent judged by numbers alone (reason ignored) | l_raised_again_declared |
| R11 | a duplicate declaration consumes nothing | l_declared_twice |
| R12 | door key without its scope (an allow moved reads as no growth) | l_door_moved (and others) |
| R13 | cost-entries: a reason read as no exemption | l_entry_reason, l_entry_new_reason |
| R14 | a registered list neither side has passes | l_neither |
| S1 | ratchet.sh swallows the python's status | l_raise and every red L case |
| S2 | ratchet.sh: base.sh's failure falls back to HEAD | l_unresolved |
| G1 | check.sh lists: ratchet-red.sh's failure swallowed | G: "the group exited 0 where ratchet-red.sh failed" |
| G2 | check.sh lists: ratchet.sh's failure swallowed | G: "the group exited 0 where ratchet.sh failed" |
| Y1 | a group no CI job runs let through | a_group_no_ci_job_runs_fails (wasm run by nobody) |
| Y2 | set-up matched by prefix (`libclang.sh && cargo test`) | a_ci_step..: that case |
| Y3 | apt install with any tail (`&& make`) | a_ci_step..: that case |
| Y4 | any word after `sh scripts/check.sh` taken as a group | a_ci_step..: `wasm && cargo test` |
| Y5 | a block's first line read alone | a_ci_step..: the continued call |
| Y6 | `uses:` not held to the list | a_ci_step..: an unknown action |
| Y7 | `shell:` ignored | a_ci_step..: that case |
| Y8 | flow-style run ignored | a_ci_step..: that case |
| Y9 | a check.sh call found anywhere in the command (`COST_BASE=.. sh scripts/check.sh cost`) | a_ci_step..: that case |
| Y10 | an allow-listed command that is a check (`cargo test .. --lib` on CI_SETUP) | a_ci_step..: the cargo case |
| Y11 | a group said not run and run let through | a_group_no..: run and said not |
| Y12 | CI_NOT_RUN naming no group let through | a_group_no..: no such group |
| Y13 | CI_NOT_RUN with no reason let through | a_group_no..: no reason |

## Repair round 1 (mutants.run5.log; mutants.results under "=== round 1 (repair)")
Every mutant above re-run against the rewritten parser and ratchet.py: all 37 killed again (Y5,
Y7, Y8 re-aimed at the new code: the block's body, the `shell` branch, the flow-value check). The
runner now takes a list of (old, new) pairs for a mutant that must change two places (a check
removed and its key made known, so the mutant is green and not red for another reason).
The reviewers' extra mutants are red edits in the tests (a_ci_step_that_checks_outside_check_sh_fails,
a_group_no_ci_job_runs_fails), each shown red before the parser change (before-r1.log) and held by
the code mutants below; the registry's by l_deregistered (before-r1-ratchet.log).

| mutant | what | killed by |
|---|---|---|
| Y14 | a job's `if:` known and unchecked | `if: false` on wasm / `if: github.event_name ..` on lists edits |
| Y15 | a step's `if:` unchecked | `if: false` on the lists step ("an `if:` the test does not list") |
| Y16 | a run under a condition still counts its group | the loop asserting "lists runs in no CI job" |
| Y17 | continue-on-error known and unchecked | the step and job edits |
| Y18 | env held by name alone | `SUPDB_PR_BASE: HEAD~1` |
| Y19 | env unchecked | SUPDB_BASE: HEAD, RATCHET_RED_STEPS, COST_PROBES, the RUSTFLAGS edits |
| Y20 | an action's inputs unchecked | checkout `ref: HEAD~1` |
| Y21 | an input matched for any action | checkout `path: elsewhere` |
| Y22 | `if: always()` allowed on any step | always() moved onto the lists run |
| Y23 | anchors/aliases read | `name: &a x` |
| Y24 | `<<` read as a key (aliases off, `<<` a step key) | the `<<: *anchor` edits |
| Y25 | an unknown key passes | `defaults:`, `working-directory:` |
| Y26 | a step both run and action passes | that edit |
| Y27 | a list item outside steps read | `env:\n  - CARGO_TERM_COLOR: always` |
| Y28 | a key under `steps:` with no item attaches to the last step | `steps:\n  name: x` on wasm (a real hole found writing the case) |
| Y29 | a label at another indent read | the six-space `newc)` |
| Y30 | only the first alternative of `a | b)` read | `newa|newb)` |
| Y31 | `|` labels unread (the old reader) | `newa|newb)` |
| R15 | the base's registry not held | l_deregistered, l_deregistered_bare |
| R16 | the tree's registry not held to its own reader | l_own_unread |
| R17 | a base registry nobody can read taken as empty | l_base_registry_unread |
| R18 | the "base" registry read from the tree | l_deregistered, l_deregistered_bare |
Survivors: none (59 of 59 killed).

## Repair round 2 (mutants.run6.log; mutants.results under "=== round 2 (repair 2)")
Every mutant above re-run against the path-keyed parser: all 59 killed again (Y27, Y28 re-aimed at
`!in_steps` / `open && in_steps`). The reviewers' extra red edits are in the tests, shown red on the
round-1 parser in before-r2.log (the `inputs` job, the nested case). New mutants:

| mutant | what | killed by |
|---|---|---|
| X1 | `inputs` read by the parent's name (`[.., "inputs"] => true`, the round-1 shape) | the `inputs` job with `if: false` (and `needs:`) |
| X2 | the first `esac` at any depth ends the groups' case | the three nested-case edits ("newgroup") |
| X3 | labels read at every depth | the nested `a)` at another indent (the label refusal stands in for "newgroup") |
| X4 | a `case`/`esac` of another shape let through | `true && case "$x" in` |
| X5 | a listed trigger missing passes | pull_request dropped, push.tags dropped |
| X6 | a trigger's value unread (both directions) | push.branches [never] |
| X7 | triggers not collected | the green test (every listed trigger missing) |
| X8 | base.sh: the zeros branch deleted (zeros fall to origin/main) | b_first_push (exit 2, and the sha) |
| X9 | base.sh: the first push's candidate HEAD, not HEAD~1 | b_first_push (the sha), b_first_push_root |
| X10 | NOT_HELD read by its values, not its keys | l_not_held |
| X11 | couplings: rows of src/db/vault.rs skipped (the reviewers' predicted survivor) | l_raise_every_file |
| X12 | door: kind cfgout skipped | l_door_every_file |
| X13 | couplings: the test region skipped | l_raise_every_file |
| X14 | a NOT_HELD path the tree lacks passes | l_not_held_typo |
| X16 | door: key (kind, file) summed, no duplicate refusal | l_door_moved (reached through door.txt), l_door_every_file |
| X15 | env's children accepted under any `env` (`[.., "env"]`) | SURVIVED: equivalent (below) |

R12 (door key without scope) was re-run: killed, but by the duplicate-key refusal (exit 2) in every
case, so X16 was added to show l_door_moved reached through door.txt itself; it is.
R9 (spent not subtracted) re-run: l_spent red with the line kept, as the reviewers predicted.
X15 is equivalent, not a hole: a key `env` is known only at [], ["jobs", _] and ["jobs", _,
"steps"] (TOP, JOB, STEP); anywhere else the arm of its own parent flags it ("the key `env`", shown
by the env-under-strategy edit, red with and without X15), and under an action's `with:` it is an
input CI_WITH does not list. So the children of an `env` are only ever read at the three paths the
precise arm names, and the wider pattern reads the same set; the path is kept for the reader's
sake (it says where an environment stands) and no check depends on it.
Totals round 2: 75 run, 74 killed, 1 equivalent survivor (X15).

## Repair round 3 (mutants.run7.log, then mutants.run8.log; mutants.results under "=== round 3")
The CI killer now runs every CI test (ci_, a_ci_step, a_group_no, a_workflow, a_set_up,
every_file_under). A mutant may create a file (E1) and run under an environment (E10, Z1, Z2:
`env SUPDB_PR_BASE=<none> SUPDB_PUSH_BEFORE=<none> SUPDB_BASE=HEAD`, steps narrowed).

| mutant | what | killed by |
|---|---|---|
| E1 | .github/workflows/extra.yml, on pull_request, `run: cargo test` (created in the tree) | every CI test: "a workflow the test does not list" |
| E2 | apple-silicon.yml's `sh scripts/check.sh machine` -> `cargo test --profile checked` (the reviewers' `bench machine | tee` line is gone: the step is the group's call now) | "is no call of scripts/check.sh", machine run by no job |
| E3 | couplings: region `test` skipped | l_raise_every_file |
| E4 | libclang.sh writes `RATCHET_RED_STEPS=G` to $GITHUB_ENV | ci_checks_only.. (set-up scripts read), a_set_up_step.. |
| E5 | base.sh: any all-zero string is a first push | b_short_zero, b_short_zeros (new) |
| E6 | `fresh` by (list, key, from, to), the reason dropped | l_raised_again_declared |
| E7 | couplings: any region | l_bad_region, l_bad_region_both (new: a typo on both sides is exit 2) |
| E8a/E8b | CI_APT's name class takes `;` / `&` | the new `valgrind;cargo test`, `valgrind&&cargo test` edits |
| E9 | check.sh cost: COST_BASE dropped | G's g_cost (new): `cost: the base is <HEAD~1>` beside SUPDB_PR_BASE=HEAD |
| E10 | G without clean_env: G's two inner calls carry none now (each case sets what it turns on); G green under the made-up env (g-fake-env-r3.log), and G1/G2 under it | E10_G1, E10_G2: killed |
| Z1 | B's base() without clean_env, under the made-up env | b_no_origin, b_pr_none, b_before_none, b_empty_events |
| Z2 | L's check() without clean_env, under the made-up env | l_unresolved |
| Z3 | a listed non-check workflow may run on pull_request | a_workflow..: pull_request edit |
| Z4 | an unlisted workflow passes | a_workflow..: extra.yml, nested, notes.txt |
| Z5 | a listed workflow missing passes | a_workflow..: comparators/apple gone |
| Z6 | push's children unread | a_workflow..: branches ['**'], tags |
| Z7 | a bare `push:` passes | a_workflow..: `push:` |
| Z8 | an inline `on:` read | a_workflow..: `on: [pull_request]` |
| Z9 | `'on':` read as `on` (quotes stripped) | SURVIVED: the strip was a check nothing reaches (an `'on':` is red as "no one `on:`" either way); taken out |
| Z9b | "no one `on:`" not reported | a_workflow..: `'on':`, `true:` |
| Z10 | the printing rule off (stdout to $GITHUB_ENV unheld) | a_set_up_step..: fscrypt's printed line |
| Z11 | `emit` unheld | a_set_up_step..: `emit SUPDB_BASE` |
| Z12 | GITHUB_ENV off the runner files | a_set_up_step..: the RATCHET_RED_STEPS and COST_PROBES lines |
| Z13 | pull_request.paths items not collected | every CI test (APPLE_ON's paths missing) |
| Z14 | the workflows read without subdirectories | every_file_under_the_workflows_is_read |
| Z15 | ci.yml's edited text ignored (the disk's used) | a_ci_step.., a_group_no.. |
| Z16 | a line to stderr counted as a write | a_set_up_step.., ci_checks_only.. (fscrypt's `>&2` lines) |
| Z17 | `sudo sh X` not read as a script | a_set_up_step.. |
| Z18 | the assert that apple-silicon.yml calls the machine group removed | SURVIVED: the group's coverage over the workflows holds it already (machine run by no job); taken out |

Round 3b (mutants.run8.log): every mutant of tests/source.rs, libclang.sh and the workflows re-run
on the finished file (59): 56 killed; X6 not applied (its text named CI_ON, which ci_steps takes as
`on` now; rewritten and killed in round 3c); X15 survived, equivalent as in round 2; Z8 (an inline
`on:` read) survived once the quote strip was gone: an inline `on:` leaves no trigger under it and
is red as "an `on:` with no trigger", so the block check was reached by nothing and was taken out
(Z8b, that error removed, is killed). Round 3c (mutants.run9.log): X6, Z3, Z6, Z7, Z8b, Z9b on the
file without it: all killed.
Totals round 3: 30 new (E1-E10, Z1-Z17) + Z8b, Z9b; survivors Z9, Z18, Z8 each a check nothing
reached, taken out; X15 equivalent. Every earlier mutant of the test file killed again.
