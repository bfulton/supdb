# T18 ratchet: notes

Step: 1 (reading). Base of run ea635218; HEAD 08070938 at start; tree clean.

## The tree against the brief (and the plan at 682b2c09)
- The plan has no section for this change; the brief is the spec.
- .github/CODEOWNERS: one line, /tests/data/couplings.txt @bfulton. As the brief says.
- tests/data holds four lists (couplings.txt 346 lines, door.txt 105, door-libc.txt 75,
  cost-entries.txt 342: 317 lines `-`+reason, 14 with probes) and crypt-v1.bin (binary).
- ci.yml jobs: test (check.sh build test stride lint bench, then `bench machine` printed to the
  summary), quick (check.sh quick), cost (base decided in YAML shell: PR base, else before, else
  HEAD~1 for '' or zeros -- so workflow_dispatch got HEAD~1 too), cross-arm (cargo in YAML with
  three CARGO_TARGET_* env), fscrypt (sudo sh scripts/fscrypt.sh up ... >> GITHUB_ENV, then
  check.sh fscrypt), wasm (check.sh wasm).
- check.sh groups: build test stride lint wasm bench quick cost fscrypt; default
  build test stride lint wasm bench. Every group already runs in some CI job.
- cost group's default base was `origin/main` (the tip, not the merge-base).
- forms.md line 255 already carries the owner's decision (commit 08070938).
- Real base of the branch: merge-base(HEAD, origin/main) = 5aad09dd; none of the four lists is
  there (all added on this branch: e7d3fe71 couplings, fa7e9356 door x2, c1546dc1 cost-entries).

## Step log (where things are)
- before.log: the CI test red on the YAML at HEAD (the cost job's base shell, lines 129-132; the
  cross-arm cargo, line 158; `cost` read as run by no job since its call carried an env prefix).
- ci_after.log: the same three tests green after the YAML change.
- ratchet-red: green, 48 cases (run by the lists group; its output is in the lists group's log).
- cross-arm.log, cross-arm2.log, pager-alone.log: the cross-arm group here (below).
- lint.log (red: clippy type_complexity in the new test), lint2.log (green, door included).
- after.log: the whole suite, --no-fail-fast.
- cost.log: `sh scripts/check.sh cost` with COST_BASE=HEAD through base.sh.
- mutants.py / mutants.results / mutant-<name>.log / mutants.md. keep/ + keep.sha256: the finished
  copies of every changed file (tests/source.rs re-kept after fmt).

## What was built
- scripts/base.sh: SUPDB_BASE, then SUPDB_PR_BASE, then SUPDB_PUSH_BEFORE (all zeros: HEAD~1, as
  the cost job did for a push that made its branch or tag), then origin/main
  (refs/remotes/origin/main); prints merge-base(HEAD, candidate). Shallow clone, unresolvable
  candidate, no origin/main, no merge-base: exit 2, message on stderr, nothing on stdout.
- scripts/ratchet.py (registry, readers, matching), scripts/ratchet.sh (base from base.sh or arg),
  scripts/raises.txt (declarations: list, key, from, to, reason; header only), scripts/ratchet-red.sh
  (steps B base.sh, L the check on scratch repos built from copies of the real lists, G the group).
- check.sh: `lists` (in the default set) and `cross-arm` (not in it); cost's base through base.sh,
  COST_BASE the override. Header made true.
- ci.yml: `lists` job (fetch-depth 0, event values in env); cost's YAML shell gone; cross-arm calls
  the group. tests/source.rs: ci_checks_only_through_check_sh_and_runs_every_group,
  a_ci_step_that_checks_outside_check_sh_fails, a_group_no_ci_job_runs_fails.
- .github/CODEOWNERS removed (the deletion is in the working tree, not staged).
- Docs: check.sh header, CLAUDE.md "Running the checks", README's command list, Cargo.toml's
  comment on supdb_emulated, tests/source.rs's module doc.

## Decisions (the brief left them to the unit)
- Growth by list: couplings.txt a total by (file, token, region), a new key from 0; door.txt each
  (kind, file, scope) a count, an allow moved to a new scope is growth there whatever fell where it
  left (the door's own test holds counts exactly by that key, so a move is a new exemption);
  door-libc.txt each name is 1; cost-entries.txt a line with `-` and a reason is 1, a line with
  probes 0, so a new reason or a probe given up for one is growth and a new probed entry is not.
- "Looks like a ratchet": every file under tests/data (recursively) that is text -- valid UTF-8
  with no NUL byte, any extension -- is a list the registry names, held (REGISTRY) or with why not
  (NOT_HELD, empty today). crypt-v1.bin is binary and outside it. scripts/cost-hot.txt is outside
  the rule (scripts/, and its `!`/`~` lines are the cost gate's own exclusions, which the brief
  keeps out of this change); it is a candidate for a later registration.
- Spent: a declaration line the base already has (multiset, the reason included) is printed as
  spent and never matched; removing it is never growth; any later change may remove it.
- The declaration file lives at scripts/raises.txt, beside the check (as cost-hot.txt beside the
  cost job), so it is not itself a tests/data list. Its key column is a string and its numbers
  integers: a later group-of-functions held to its net is a reader in REGISTRY and lines of the
  same file.
- base.sh prints the merge-base for every candidate, the override included; under CI a PR's HEAD
  is the merge commit, so merge-base(HEAD, base sha) = the base sha, as the cost job had it.
- Departures from today's cost job: workflow_dispatch (no event values) took HEAD~1 and now takes
  merge-base(HEAD, origin/main) -- on main that is HEAD itself, a self-check. Locally the cost
  group's default was origin/main's tip and is now its merge-base with HEAD.
- `lists` is in the default set: seconds, python3 and git, which every host of the gate has. It
  needs origin/main (or SUPDB_BASE) and a full clone, and says so where either is missing.

## Results (finished; the work is uncommitted)
- before.log red (YAML at HEAD); ci_after.log green; after.log whole suite 27 binaries, 1512
  passed, 0 failed, EXIT 0; lint2.log green (clippy, fmt, door with its red input).
- ratchet-red: 48 cases green; lists group green against the real base 5aad09dd
  (lists-real-base.log: all four lists new since the base, nothing to hold); green against
  c1546dc1^ (couplings, door, door-libc held, 0 raised; cost-entries new) and e7d3fe71
  (lists-g1-bases.log). No raise was let through, so the verdicts print none; the red input's
  l_declared case is the printed raise.
- cost.log: `COST_BASE=HEAD sh scripts/check.sh cost`: base.sh resolved 08070938 (HEAD, the
  commit before the change), cost-red green, cost: green, EXIT 0. The first try
  (cost.refused-cargo-toml.log) was refused by cost-red's V step: an uncommitted comment edit in
  Cargo.toml read as "the engine differs from HEAD". The Cargo.toml comment edit was taken back
  (its old wording, "the CI job ... cross-arm", is still true). Noise in the same log: cost-red's
  tree copy (git ls-files | tar) prints "tar: .github/CODEOWNERS: Cannot stat" while the
  deletion is uncommitted; the pipeline's status is tar -x's, so nothing failed. Goes away at
  the commit.
- Cross-arm: gcc-aarch64-linux-gnu, qemu-user-static and the rust target were installed here.
  The group's command is the YAML's (sh -x in cross-arm.log). Run 1: qemu internal SIGSEGV
  (emulator crash) after 15 tests. Run 2: 519 passed, 1 failed:
  db::pager::tests::a_view_says_what_is_in_memory_and_drops_and_asks_for_what_stands_behind_it
  ("the file did not come back"); alone five times under the same runner: failed all five
  (pager-alone.log). src/ is untouched by this change and the command is the one the YAML ran, so
  this is the cross-arm job's own state on this host (a test of the kernel's answer to advice that
  `supdb_emulated` does not compile out), not this unit's. Not verified: whether the hosted
  runner's qemu passes it. target/aarch64-unknown-linux-gnu was removed after.
- Mutants: 37, all killed (mutants.md). Tree put back, checked against keep/.

## What the gate holds / cannot see
- Holds: growth of each registered list against merge-base(HEAD, base), keyed as each list's
  purpose says; exact declarations; spent lines; a list removed; a text file under tests/data
  unregistered; CI run steps other than check.sh calls and listed set-up; actions off the list; a
  `shell:`; quoted or flow-style runs; groups no job runs.
- Cannot see: a list outside tests/data (scripts/cost-hot.txt is one); a raise hidden by changing
  the registry's reader in the same change (review); YAML constructs the line parser does not know
  beyond the ones it refuses (anchors/aliases, `defaults:` other than a bare `run:` key); whether a
  declared raise is wise (review, as forms.md says).

## Repair round 1 (three blocking findings, one in two copies)
Step: mutants running (mutants.run5.log, verdicts appended to mutants.results under
"=== round 1 (repair)"); then the neighbours, lint, the lists group whole.
- before-r1.log: the new YAML red edits in tests/source.rs, red on the old parser (the first,
  `if: false` on the lists step, passes; EXIT 101). before-r1-ratchet.log: the new L cases red on
  the old ratchet.py (l_deregistered exit 0: couplings moved to NOT_HELD with a raise reads green;
  l_base_registry_unread exit 0).
- tests/source.rs's ci_steps now reads every key of the workflow by where it stands (a stack of
  keys by indent): top (name on env jobs), on/push/workflow_dispatch/inputs, a job (strategy
  runs-on steps timeout-minutes env), a step (name uses run with env), strategy/matrix. Held:
  env entries anywhere to CI_ENV, name and value whole (CARGO_TERM_COLOR, ARMS from inputs, the
  two event values); an action's inputs to CI_WITH by action; a step's `if:` to CI_IF by value and
  action (always() on upload-artifact only); a job's `if:` refused; continue-on-error refused
  anywhere; shell refused; an unknown key, an anchor/alias, a `<<` key, a flow value other than
  branches/tags/runner, a list item outside steps, a key under `steps:` with no item, a step both
  run and action: red. A run under a refused condition counts no group, so `if: false` on the
  lists step or job is also "lists runs in no CI job".
- Found writing the cases: a key under `steps:` with no `- ` item attached to the previous job's
  last step (steps.last_mut()); an `open` flag scoped to the steps block now (mutant Y28).
- check_groups reads the labels between `case "$g" in` and `esac`: `a | b)` alternatives, `*`
  dropped, and a label-shaped line at another indent is an error (it cannot be read).
- scripts/ratchet.py: registered(text) reads the REGISTRY block's keys; the tree's own file must
  read as REGISTRY does (else exit 2), and every path the base's scripts/ratchet.py registered
  must be in the tree's REGISTRY (else red, whether deleted or moved to NOT_HELD). A base with no
  scripts/ratchet.py (the real base 5aad09dd) holds no registry; one whose REGISTRY cannot be read
  is exit 2. ratchet-red.sh's L fixture now carries copies of ratchet.py/ratchet.sh/base.sh so a
  case can change the tree's registry (own()); cases l_deregistered, l_deregistered_bare,
  l_base_registry_unread, l_own_unread, l_own_self. 53 cases.
- Docs: check.sh header (every key held, not the commands alone), CLAUDE.md (the rule the round
  taught: a check over one kind of line passes everything in another kind; a check whose scope is
  written in the tree is held to the scope at the base).
- keep/ re-taken after fmt (keep.sha256).
- Results (round 1): mutants 59/59 killed (mutants.md); keep/ checked after the run. lists group
  whole green (lists-group-r1.log: ratchet-red 53 cases green, then green against 5aad09dd, the
  merge-base with origin/main after a fetch: all four lists new since the base). Against
  c1546dc1^ and e7d3fe71 (lists-g1-bases-r1.log): green, 0 raised; neither base has
  scripts/ratchet.py, so the registry clause holds nothing on this branch today (the red input's
  l_deregistered is its proof). after-r1.log: tests/source.rs 72 passed. lint-group-r1.log:
  `sh scripts/check.sh lint` green (clippy -D warnings, fmt, door with its red input).
- Not re-run this round: the cost group (cost.log, an hour; base.sh and the cost group's lines
  untouched this round, check.sh's change is its header comment) and cross-arm (YAML and group
  untouched this round). src/ untouched: no callgrind rows to count again.
- Cannot see (beyond the earlier list): the workflow's triggers (`on:`) are read for their keys
  only; a change that drops `pull_request:` would leave every group run on pushes alone and read
  green. A person's input (ARMS, inputs' defaults) is allowed by name and value, not judged.

## Repair round 2 (one blocking item, ten extra mutants)
Step: mutants running (mutants.run6.log, verdicts under "=== round 2" in mutants.results); then
the neighbours (tests/source.rs whole), lint group, the lists group whole.
- before-r2.log: the new red edits red on the round-1 parser (a job renamed `inputs` with
  `if: false` read green; a nested `case`/`esac` in build ended the groups' case early). EXIT 101.
- tests/source.rs ci_steps: every arm keyed on the whole path (`["on","workflow_dispatch","inputs"]`,
  `["jobs", _]`, `["jobs", _, "steps", "env"]` ...), never on the parent's name; a list item only
  under `["jobs", _, "steps"]`. Triggers held: CI_ON (push, push.branches [main], push.tags ['*'],
  pull_request, workflow_dispatch) matched whole both ways (an unlisted trigger value red, a listed
  one missing red); a dispatch's inputs stay read by their keys.
- check_groups: depth of `case`/`esac` (first word `case` opens unless the line closes too; last
  word `esac` closes); labels read at depth 1 only; ends where the groups' case closes; any other
  line with the word `case`/`esac` (comments stripped) is "a `case` or `esac` the test cannot read".
- New red edits: job `inputs` + `if: false` (job's `if:` and lists runs in no job); jobs named
  inputs/push/workflow_dispatch/matrix/with/on with `needs:`; env under a strategy; pull_request
  dropped; push.branches [never]; push.tags dropped; a key under workflow_dispatch; three nested
  case shapes then a new group; an unreadable `case`.
- ratchet-red.sh: l_raise_every_file / l_door_every_file (one line of each file and region / each
  kind and file raised, every raised key named), l_not_held, l_not_held_typo. 57 cases (red-r2.log).
- Docs: check.sh header (keys by path, triggers), CLAUDE.md (the round's rule: a key by the
  whole path to it, with the three failures). keep/ re-taken (keep.sha256).
- Results (round 2): mutants 74 of 75 killed, X15 equivalent (mutants.md); keep/ checked after the
  run, every file the same as the tree. after-r2.log: tests/source.rs 72 passed. lint-group-r2.log:
  `sh scripts/check.sh lint` green. lists-group-r2.log: ratchet-red 57 cases green, then green
  against 5aad09dd (merge-base with origin/main after a fetch; all four lists new since the base).
  lists-g1-bases-r2.log: green against c1546dc1^ and e7d3fe71, 0 raised.
- The reviewers' predicted survivors: the vault.rs skip (X11) and the pull_request / branches edits
  are killed now (by l_raise_every_file and CI_ON), so they were missing tests, as predicted.
- Not re-run this round: the cost group (base.sh's branches and the cost lines untouched; the round
  changed only base.sh's red input through mutants) and cross-arm (YAML and group untouched). src/
  untouched: no callgrind rows to count again.
- Cannot see (updated): the triggers are now held by value; a person's input to workflow_dispatch
  stays read by its keys alone. A word `case`/`esac` inside a string of check.sh's groups (e.g.
  `echo case`) reads as an unreadable case: red, conservatively.

## Repair round 3 (one blocking item: workflows other than ci.yml; ten extra mutants)
Step: mutants running (mutants.run7.log; verdicts under "=== round 3 (repair 3)" in
mutants.results); then tests/source.rs whole, lint group, lists group whole.
- before-r3.log: the new test reading every workflow, red on the tree with apple-silicon.yml as it
  was (cargo build, `bench machine | tee`, two greps, working-directory: EXIT 101).
- apple-silicon.yml's check is the `machine` group of scripts/check.sh now (not in the default
  set; refuses any host but Darwin arm64, exit 2 here: machine-here.log). The workflow keeps its
  pull_request paths (plus scripts/check.sh, the group's own source) and runs
  `sh scripts/check.sh machine`. The group keeps `bench machine`'s exit status, which the YAML's
  `| tee` had dropped (the greps caught a default anyway). Not run on a Mac here.
- tests/source.rs: every file under .github/workflows (recursively, any name) is on CI_CHECKS
  (ci.yml with CI_ON, apple-silicon.yml with APPLE_ON; held by ci_steps, groups counted over both)
  or CI_OTHER (six, each with why; triggers read and held to workflow_dispatch, schedule and
  push with `branches: [main]` alone; an inline, quoted or missing `on:` red), else red; a listed
  file missing is red. ci_steps takes its trigger table; `permissions: contents: read` and
  pull_request.paths items known. Set-up scripts on CI_SETUP (`sh X`/`sudo sh X`) read: a line
  naming GITHUB_ENV/PATH/OUTPUT/STATE or BASH_ENV, and in a script whose output the step sends to
  $GITHUB_ENV a line that prints to stdout, is on CI_SETUP_WRITES whole; `emit NAME` in libclang.sh
  names LIBCLANG_PATH or RUSTFLAGS. Tests: every_file_under_the_workflows_is_read (a temp tree),
  a_workflow_the_test_does_not_hold_fails, a_set_up_step_that_writes_the_environment_unlisted_fails,
  and APT `;`/`&&`/`|` inside a word cases.
- ratchet-red.sh: b_short_zero, b_short_zeros (exit 2), l_bad_region, l_bad_region_both,
  l_bad_kind (exit 2), G's g_cost (COST_BASE=HEAD~1 beside SUPDB_PR_BASE=HEAD: `cost: the base
  is <HEAD~1>`). G's two inner calls no longer carry clean_env: each case sets the variable its
  verdict turns on (or fails before a base is read), so the outer environment decides nothing;
  g-fake-env-r3.log is G green under made-up SUPDB_PR_BASE/PUSH_BEFORE/SUPDB_BASE, and the group's
  mutants run under the same (E10_*). Where clean_env decides (B, L) it stays; Z1/Z2 show it. 63 cases.
- Docs: CLAUDE.md's sentence ("does nothing else that checks") rewritten to what the test holds,
  with the rule the round taught; check.sh header (the workflows, the machine group).
- Cannot see (round 3): what a set-up's binary or an action writes to the job's environment (only
  the scripts' lines are read); a script line that writes GITHUB_ENV without naming it.
- Results (round 3): before-r3.log red (EXIT 101, the tree's apple-silicon.yml); after-r3.log
  tests/source.rs 75 passed, EXIT 0; lint-group-r3.log `sh scripts/check.sh lint` green (two
  clippy findings in the new test fixed first: a type alias CiOn, a useless format!);
  lists-group-r3.log green: ratchet-red 63 cases, then against 5aad09dd (merge-base with
  origin/main after a fetch; all four lists new since the base); lists-g1-bases-r3.log green
  against c1546dc1^ (three lists held, 0 raised) and e7d3fe71 (couplings held, 0 raised);
  red-r3.log ratchet-red alone green; g-fake-env-r3.log G green under a made-up environment;
  machine-here.log: the machine group refuses this host, exit 2. Mutants: mutants.md, round 3.
  keep/ (12 files, keep.sha256) is the finished copy of every changed file, checked equal to the
  tree after the runs. Two checks taken out as reached by nothing (the quote strip of `on`, the
  inline-`on:` block check) and one assert (apple-silicon calls machine) held by coverage already.
- Not re-run this round: the cost group (its lines and base.sh unchanged; check.sh gained the
  machine group and header text) and cross-arm (YAML and group unchanged). src/ untouched: no
  callgrind rows. The machine group was not run on a Mac (none here); `bench machine` on this
  host prints the two keys in the format the group greps (`"cache_line_detected": true`).
- TMPDIR (plan/build/tmp/T18) removed at the end.
