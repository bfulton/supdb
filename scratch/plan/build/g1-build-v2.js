export const meta = {
  name: 'seam-g1-build',
  description: 'Build stage G1 of the encryption seam, one gated unit at a time from the scouted plan: the wall between the forms and the encryption modules, the door by symbol, the plain-cost job, the rewrites of code that answers wrong for a source that lends by the piece, and whole borrows made uncompilable outside the dispatch. Each unit: its red input shown red, the change, mutants, two read-only reviews, repair, the unit gate and a local commit; then the full gate and a critic. Nothing is pushed.',
  phases: [
    { title: 'Build', detail: 'one unit at a time: red input shown red, the change, mutants, callgrind rows', model: 'opus' },
    { title: 'Review', detail: 'two read-only skeptics per unit; repair until clean', model: 'opus' },
    { title: 'Commit', detail: 'unit gate and a local commit', model: 'opus' },
    { title: 'Gate', detail: 'sh scripts/check.sh, every default group, and the new groups', model: 'opus' },
    { title: 'Critic', detail: 'what the contract now holds, what it does not, what the owner decides', model: 'opus' },
  ],
}

const SP = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad'
const PLAN = SP + '/plan'
const W = PLAN + '/build'
const BASE = (args && args.base) || 'HEAD'
const ONLY = (args && args.only) || null
// A launch of some units alone (args.only) may leave the full gate and the critic to a later launch (args.tail === false).
const TAIL = !(args && args.tail === false)

const CONTEXT = `
You are working in /home/user/supdb (Rust; a read-optimized embedded store; the engine is src/db.rs and src/db/*.rs, the format's readers src/blob.rs, src/flatindex.rs, src/ordindex.rs, the byte-source seam src/bytes.rs), on the branch that is checked out. The store can be kept under a caller's cipher (src/db/vault.rs, src/db/pager.rs) and the owner's goal for that feature is: reliable, performant, and not fragile to maintain. "Not fragile" has a contract, ${SP}/seam/forms.md (acceptance tests Z, N, K, F and rules R0 to R15), adopted by ${SP}/seam/decisions.md (read items 9 and 11 to 16: they amend forms.md and they stand where the two differ). Stage G1 builds the first gates of that contract on today's code, before the page layer is built under them.

The plan is ${PLAN}/g1.md: eight units, each a section, with a section "What every unit inherits" that binds every unit. It was written from four read-only scouts (${PLAN}/borrows, door, wall, conform: notes.md) each re-derived by a skeptic (check.md, which stands where it corrects its notes), over an export of the tree at 682b2c09 (${PLAN}/tree). The tree has moved a long way since: some three hundred commits (git log --oneline --first-parent 682b2c09..HEAD), among them four merges of upstream (the stride form and its sparse kind in src/db/zfile.rs and src/db/zselect.rs, run sequence numbers, the store lock, the pre-written log, new bench arms), the plain store's durability fixes, directory guards, a manifest slot that names its log, a form file's failed chunk made the form's and not the store's (pager::Paged::regenerable), and a first cut of the plain-cost job (scripts/cost.sh, scripts/cost-hot.txt). So every line number, count and file size in the plan and in the scouts' notes is a hint from another tree: find the code by name; where the plan's count of something differs from the tree's, the tree is right and your notes say what moved and why; where something the plan says to build is already there, read it, hold it to the plan's red input, and build only what is missing; where the plan names code that is gone or reshaped, do what serves the rule and record the departure with its reason. scratch/HANDOFF.md in the repository says where the branch stands and what was done since the plan. The run's base is ${BASE}; units of this run before yours are committed on top of it (git log ${BASE}..HEAD) with notes under ${W}/<unit>/: read them first.

Tests known to fail now and then for reasons that are not a unit's, each to be read as its own before it is read as yours: under load the checked zpages binary's wall-clock waits (scratch/HANDOFF.md lists them; the gate's one copy passes), and tests/cipher.rs the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit ("the builds gave nothing back"), undiagnosed. A red run on one of those, in a test your diff does not touch, is run again alone five times and reported with both outputs; it is never skipped, ignored or loosened, and a second red of the group is a red.

The container this runs in is restarted every few hours without warning (uptime says when), and its files survive: a background command may be started again from its beginning by the harness, so a log is read by its own EXIT line and its start time, never by its presence. Keep ${W}/<unit>/notes.md current as you work (the step you are on, where a runner writes its results, which copy of each file is the finished one). Before the first mutant, keep the files you changed under ${W}/<unit>/keep/ with their sha256 sums; a mutant runner writes each verdict to its results file as it gets it and puts the file back, checked by its sum, after each.
`

const RULES = `
RULES (the owner's; they are not yours to reopen)
1. A store in the clear pays nothing: no instruction added per key, entry, block or step, counted with callgrind by function against the commit before, never argued and never read off a total. No new syscall, wait, lock or allocation on any path of a plain store; where a unit says a plain store's syscalls must be the same, strace shows it, before and after.
2. Nothing is switched off, refused or limited under a cipher to make a unit easier, and no Options arm is removed. No engine default is changed by this run: the owner's rule is that a default changes only after benchmarks confirm it is better, and this run takes no timing. A mechanism that starts to work under a cipher because of your unit, on a path no default reaches there, is said plainly in the notes with who prices it and when (the plan names the stage).
3. A gate is trusted only after it has failed on a known-red input, by its exit status: gate on "&&" and the verdict line, never on a "| tail" that swallows the status ("set -e" does nothing in this environment). The red input is a script step or a test, never an edit made by hand and described.
4. Red first. The unit's red test or red input goes in and is shown red before the change (${W}/<unit>/before.log), green after (after.log). What cannot be made red before the change is not evidence; say so.
5. Mutants: the ones the unit's section lists and at least three of your own on the lines you wrote. A survivor is a missing test or a check nothing reaches: add the test or take the check out. ${W}/<unit>/mutants.md records each and what killed it.
6. Tests hand read callbacks through tests/common's kv and vf, never a closure straight to a generic read call. No module gets a blanket lint allowance beyond the ones the plan names, each with its one-line reason; everything holds to -D warnings and the format gate.
7. Docs: README.md and crate docs are factual and current, no history, no counts that move. CLAUDE.md is notes for whoever picks the work up next, in that file's own manner; the plan's last unit writes most of it, so a unit before it adds to CLAUDE.md only a rule its own work taught, with the failure that taught it.
8. Mechanics. One cargo at a time; no timing or benchmark run (callgrind and strace are counts, and still run one at a time). Tests run as "cargo test --profile checked" with TMPDIR exported to a directory of its own under ${W}/tmp/<unit>, removed when you finish; never glob /tmp, and remove a directory of test stores by its literal path (a glob through a shell variable is refused by the harness). A long command goes in the background with its output and exit status in a log. Read df before anything that builds: about seven gigabytes are free, bench/target holds fourteen and target five, each bench binary is a third of a gigabyte, and the first thing to fail when the allowance is spent is a link ("ld terminated with signal 7"). A second target directory is allowed only where the plan asks for one (the door's) or scripts/cost.sh makes one, and is measured and reported; remove scratch copies and their target directories when the step that made them ends, pass or fail.
9. Git. Do not push. Do not stash, reset or check out over the working tree. Only the commit step commits. Edit nothing under scratch/.
10. Say what you did not verify. Where the plan marks something "not verified" or "inferred" and your unit depends on it, verifying it is your first step, and if it is false, stop and say what the unit should become instead of building around it.
`

const UNITS = [
  { key: 'G1-1', name: 'wall', engine: false, rows: 'none', extra: 'The lexer you write is shared by every later source test (tests/source.rs): keep it one module with its own unit tests. Decision 12 settles the granularity: the gate compares (file, token, region) totals and prints the scopes beside a failure. The scouts\' couplings.txt, token lists, lex.py and gen.py are under ' + PLAN + '/wall and are of the tree at 682b2c09: run gen.py over today\'s tree for today\'s counts, carry each line\'s stage across from the old file where its (file, token, scope) still exists, and give every line the old file has none for (the stride form, the directory guards, regenerable, whatever else is new) its stage by the rule the wall notes give for its kind, listing each such line in your notes for the reviewers. The counts in the section (259, 238, 23, 0, 6) are that tree\'s.' },
  { key: 'G1-2', name: 'door', engine: false, rows: 'none', extra: 'Decision 11 settles the thread half: not in the clippy config at G1; a per-file count of std::thread:: in the source test holds growth. The door\'s own target directory: build it once, report its size, and say in the notes whether the lint group can afford it on a box with this little disk (if not, propose the smallest change: the same target directory with a distinct profile, or cleaning it at the step\'s end).' },
  { key: 'G1-3', name: 'cost', engine: false, rows: 'this unit is the instrument', extra: 'Part of this unit is built: scripts/cost.sh BASE [probe ...] and scripts/cost-hot.txt (commits af56aac, 5ca5a84, 3f65bfb; CLAUDE.md, "sh scripts/cost.sh BASE is the count made one command", and scratch/HANDOFF.md, "The count-back, and how to hold it"). It builds six probes at BASE in a worktree with a target directory of its own and at the tree, counts each under callgrind and fails where a function of the hot list gains past COST_SLACK. Read it and hold it to this section: what it lacks is (a) the red input, shown red through the script by its exit status (the scratch-copy shape the section gives) and the self-check of the tree against itself reading zero; (b) rows 3, 7 and 8 and the second half of row 2: reads and scans through the compact form (supdb-nocachezpages), a scan over SCAN_REST (1024), a seal probe (bench/examples/seal_probe.rs) and Blob::scan_at through the public API (bench/examples/scan_at_probe.rs); (c) the cost group in scripts/check.sh, outside the default set, and its CI job; (d) the entry-point list in tests/source.rs with tests/data/cost-entries.txt; (e) the mutants the section lists, against the script as it now is. Upstream has moved six commits since the last count (the sparse stride form): your first whole run of the script is against the upstream head the branch last merged (git merge-base HEAD origin/claude/e-recode-stride-selector-g5f6hl; fetch that branch by name first), and a function of the hot list that reads over it is a finding to explain by its callers and callees before it is called a cost (CLAUDE.md has two false alarms of that kind written up). A whole run is about three quarters of an hour and builds a second bench workspace that it removes at its end.' },
  { key: 'G1-4', name: 'offsets', engine: true, rows: '1, 2, 4', extra: 'Decision 16: what this changes for a cipher store is behind the compact form or merge_drop_behind only; the commit message says so and names S5 as where it is priced.' },
  { key: 'G1-5', name: 'authenticated', engine: true, rows: '6, 8', extra: 'The guard matters as much as the red test: the cipher store\'s open stays within its bound of chunks (tests/cipher.rs).' },
  { key: 'G1-6', name: 'advice', engine: true, rows: '3, 2', extra: 'The proof that a plain store did not move is the strace the plan describes, kept under the unit\'s notes directory as before and after files with the diff. Decision 16 as for G1-4.' },
  { key: 'G1-7', name: 'sealed', engine: true, rows: '1, 2, 3, 6, 8', extra: 'The token pattern is marked not verified in the plan: its first build is your first step (rule 10). This unit changes a public signature of Bytes: its gate also runs the wasm and bench groups, and the wasm module\'s size must not grow past its budget (read how the wasm group checks it).' },
  { key: 'G1-8', name: 'docs', engine: false, rows: 'none', extra: 'Also bring the copy of the contract in the scratchpad up to what landed: ' + SP + '/seam/forms.md gets a short section "As built at G1" listing each rule\'s gate by file and test name, and what each gate cannot see. Do not edit scratch/ in the repository; the orchestrator copies the scratchpad\'s notes there.' },
]

const IMPL_SCHEMA = {
  type: 'object',
  properties: {
    unit: { type: 'string' },
    summary: { type: 'string' },
    red_inputs: { type: 'array', items: { type: 'object', properties: { what: { type: 'string' }, red_before: { type: 'string' }, green_after: { type: 'boolean' } }, required: ['what', 'red_before', 'green_after'] } },
    files_changed: { type: 'array', items: { type: 'string' } },
    mutants: { type: 'array', items: { type: 'object', properties: { mutant: { type: 'string' }, killed_by: { type: 'string' } }, required: ['mutant', 'killed_by'] } },
    survivors: { type: 'array', items: { type: 'string' } },
    callgrind: { type: 'array', items: { type: 'object', properties: { row: { type: 'string' }, function: { type: 'string' }, before: { type: 'string' }, after: { type: 'string' } }, required: ['row', 'function', 'before', 'after'] } },
    plain_store_cost: { type: 'string' },
    cipher_changes: { type: 'string' },
    plan_departures: { type: 'array', items: { type: 'string' } },
    not_built: { type: 'array', items: { type: 'string' } },
    not_verified: { type: 'array', items: { type: 'string' } },
    engine_changed: { type: 'boolean' },
    blocked: { type: 'string' },
  },
  required: ['unit', 'summary', 'red_inputs', 'files_changed', 'mutants', 'survivors', 'callgrind', 'plain_store_cost', 'cipher_changes', 'plan_departures', 'not_built', 'not_verified', 'engine_changed', 'blocked'],
}

const REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['pass', 'repair'] },
    blocking: { type: 'array', items: { type: 'object', properties: { title: { type: 'string' }, detail: { type: 'string' }, where: { type: 'string' } }, required: ['title', 'detail', 'where'] } },
    nonblocking: { type: 'array', items: { type: 'string' } },
    extra_mutants: { type: 'array', items: { type: 'string' } },
  },
  required: ['verdict', 'blocking', 'nonblocking', 'extra_mutants'],
}

const COMMIT_SCHEMA = {
  type: 'object',
  properties: {
    committed: { type: 'boolean' },
    sha: { type: 'string' },
    gate: { type: 'string' },
    fixed_in_passing: { type: 'array', items: { type: 'string' } },
    problems: { type: 'array', items: { type: 'string' } },
  },
  required: ['committed', 'sha', 'gate', 'fixed_in_passing', 'problems'],
}

const GATE_SCHEMA = {
  type: 'object',
  properties: {
    green: { type: 'boolean' },
    groups: { type: 'array', items: { type: 'object', properties: { group: { type: 'string' }, verdict: { type: 'string' } }, required: ['group', 'verdict'] } },
    followup_commits: { type: 'array', items: { type: 'string' } },
    problems: { type: 'array', items: { type: 'string' } },
    head: { type: 'string' },
    tree_clean: { type: 'boolean' },
  },
  required: ['green', 'groups', 'followup_commits', 'problems', 'head', 'tree_clean'],
}

const CRITIC_SCHEMA = {
  type: 'object',
  properties: {
    holds: { type: 'array', items: { type: 'object', properties: { rule: { type: 'string' }, gate: { type: 'string' }, shown_red_by: { type: 'string' }, cannot_see: { type: 'string' } }, required: ['rule', 'gate', 'shown_red_by', 'cannot_see'] } },
    acceptance: { type: 'array', items: { type: 'object', properties: { test: { type: 'string' }, state: { type: 'string' } }, required: ['test', 'state'] } },
    concerns: { type: 'array', items: { type: 'string' } },
    owner_items: { type: 'array', items: { type: 'string' } },
    for_g2: { type: 'array', items: { type: 'string' } },
    path: { type: 'string' },
    tree_clean: { type: 'boolean' },
  },
  required: ['holds', 'acceptance', 'concerns', 'owner_items', 'for_g2', 'path', 'tree_clean'],
}

const LENSES = [
  {
    key: 'holds',
    text: `Your lens is whether the unit holds what it says. Read the unit's section of the plan and the rule of forms.md it serves, then the diff. Is the gate real: was it shown red on the plan's red input by its exit status, can it pass without running (a missing config, a skipped platform, an empty list, a swallowed status), and can the thing it bars be written another way it does not see (an alias, a re-export, a method on a value, a macro, a cfg region it does not lex, a spelling of an allow it does not count)? Try three ways round it by reading and name each as an extra mutant or a blocking item. For a rewrite: is it right for both kinds of source, mapped and by the piece, and under a cipher (walk a read through the pager: what is asked, what is pinned, what a rest may give back); did the red test fail before for the reason it names; would it pass with the change halved? Did the unit switch anything off or narrow it under a cipher? Does every departure from the plan have a reason that is a fact about the code?`,
  },
  {
    key: 'cost',
    text: `Your lens is what a store in the clear pays, and what the next person pays. By function, list everything the diff adds on any path of a plain store: instruction, load, branch, syscall, allocation. The callgrind rows the plan names for this unit must be there, by function, before and after, from the same probe sources: read them, and read the diff for a function the rows do not cover. A visibility or type-level change that "should compile to the same code" is shown by the rows, not said. For a unit that changes syscalls or advice, the strace before and after is the evidence: read both files. Then maintenance: does the gate ask the next author for something reasonable (a count lowered, a line with a reason), does its failure message say what to do, is its run time and disk reported, does it double a check another gate already makes, are the allows each given a true reason, is a public signature changed in a way an outside implementor of Bytes would feel (and are the wasm and bench groups run where the plan says)? Is each doc sentence true, with no count that moves? Propose up to five mutants most likely to survive.`,
  },
]

function sectionOf(u) {
  return `Unit ${u.key} "${u.name}" in ${PLAN}/g1.md. Read that section whole, the section "What every unit inherits", the plan's "Changes to the contract" and "Not verified" lists, and the scout notes and checks the section cites.${u.extra ? ' ' + u.extra : ''}`
}

function implementPrompt(u) {
  return `${CONTEXT}
YOUR UNIT: ${sectionOf(u)} You build it. You do not commit.
${RULES}
Steps:
0. "git status --porcelain" must be empty before you start; if it is not, touch nothing and return with blocked saying what you found.
1. Read the plan as above, forms.md for the rule your unit serves, decisions.md, and the code by name. Start ${W}/${u.key}/notes.md with what in the tree differs from what the plan read at 682b2c09.
2. Anything your unit depends on that the plan marks not verified: verify it now (rule 10).
3. The red test or red input, shown red: ${W}/${u.key}/before.log.
4. The change, as the section says. Where the tree makes the section wrong, do what serves the rule and record the departure with its reason.
5. Green: your tests, the neighbours the change touches, then the whole suite once with --no-fail-fast in the background (${W}/${u.key}/after.log)${u.engine ? ', and once more as the stride group runs it (read scripts/check.sh for its command)' : ''}; the lint group's commands (read scripts/check.sh), and the format step.
6. Callgrind rows named for this unit (${u.rows}), by function, the commit before against the tree, from the same probe sources; from G1-3 on, through scripts/cost.sh. For a unit with no rows, say why no compiled engine code changed (the diff shows it).
7. Mutants (rule 5). Put the tree back exactly.
8. Finish notes.md: what the gate or the rewrite holds, what it cannot see, each departure from the plan, what you did not verify.
Leave the work uncommitted in the working tree. Return the structured result (blocked is the empty string unless you stopped); it is data for the next step, not a message to a person.`
}

function reviewPrompt(u, lens, round, impl) {
  return `${CONTEXT}
YOU ARE A REVIEWER, round ${round + 1}, of: ${sectionOf(u)} The unit's work is uncommitted in the working tree: read it with "git status" and "git diff", with ${W}/${u.key}/ (before.log, after.log, mutants.md, notes.md) beside it. You are read-only: edit no tracked file, start no build of changed sources, run no mutant yourself (describe it under extra_mutants and the repair step runs it). You may run an already built test by exact name under a TMPDIR of your own (${W}/tmp/review-${u.key}-${lens.key}) and must remove that directory after.
${RULES}
${lens.text}

Default to "repair" when you find a real problem and to "pass" when you do not. Blocking: a gate that can pass without holding its rule or was not shown red, a rewrite wrong for a kind of source or under a cipher, a cost to a plain store, a callgrind row or strace the plan asked for that is missing, something switched off or narrowed under a cipher, an arm removed, a departure from the plan with no factual reason, a doc sentence that is false. Style and wording are nonblocking. Give file and line, and say what you did not check.

The builder's own account (data, to be checked, not believed):
${JSON.stringify(impl, null, 1).slice(0, 14000)}`
}

function repairPrompt(u, round, blocking, extra, source) {
  return `${CONTEXT}
YOUR UNIT, repair round ${round + 1}: ${sectionOf(u)} The unit's work is uncommitted in the working tree, with its notes under ${W}/${u.key}/. ${source} found the problems below. Deal with each: fix it, or show with a test, a run or a line of code why it is not a problem (and say so). Run every extra mutant; a survivor means a missing test or a check nothing reaches. Keep everything the unit already holds. If this round changes compiled engine code, the unit's callgrind rows are counted again. Then the unit's tests, the neighbours, the lint commands and the format step, and notes.md and mutants.md brought up to date. You do not commit.
${RULES}
BLOCKING:
${JSON.stringify(blocking, null, 1)}

EXTRA MUTANTS TO RUN:
${JSON.stringify(extra, null, 1)}

Return the structured result for the unit as it now stands (all of it). Set engine_changed to true only if this round changed anything under src/ that compiles into a release build.`
}

function commitPrompt(u, impl, unresolved) {
  const more = u.key === 'G1-7' ? ' and then "sh scripts/check.sh wasm" and "sh scripts/check.sh bench" (a public signature of Bytes changed)' : (u.key === 'G1-3' ? ' and then "sh scripts/check.sh bench" (the suite gained probes)' : '')
  const groups = u.engine ? 'build test stride lint' : 'build test lint'
  return `${CONTEXT}
YOU COMMIT: ${sectionOf(u)} Its work is uncommitted in the working tree; its notes are under ${W}/${u.key}/.
${RULES}
Steps:
1. git status --porcelain and git diff --stat: only what the unit's section says it changes belongs. Remove stray files that are not the unit's (scratch copies, logs, a target directory under the tree that .gitignore does not cover) and say what you removed.
2. Run the unit gate from a clean TMPDIR of your own, in the background with the exit status captured: "sh scripts/check.sh ${groups}"${more} (the stride group runs the test group's binaries again with the stride form forced, and is in a unit's gate wherever the unit changes compiled engine code). Read each group's verdict from its exit status and the "test result:" lines. A test binary that ends with code 101 and no "test result:" line is a watchdog's exit (CLAUDE.md). Wait for it: the gate has its verdict when its log has the exit status you wrote after it (a cold build takes tens of minutes, a warm one a few), and a gate left running when you answer is stopped with you. Never answer before that line is there; never commit, and never return committed false, on a gate that has not finished. How to wait: write the exit status to the log after the command ("...; echo EXIT $? >> log"), then block on that line with the Monitor tool (an until-loop that greps the log for "^EXIT") or a foreground command that does the same, again and again until the line is there. A turn that ends while the gate still runs is taken as your answer, and the gate goes on running under whoever comes next: if you must answer without a verdict, stop the gate's processes first and say so.
3. Red for something mechanical (formatting, a clippy lint, a doc link): fix it, list it under fixed_in_passing, run the failed group again. Red for anything else: do not commit; return committed false with the failing test's name and output.
4. One commit. The subject is one sentence naming what is now held, in the manner of "git log --oneline -15" (no prefix tag). The body says what the gate or the rewrite holds and how it was shown red, what it cannot see, what a store in the clear pays (the rows, by function, or why none), what changes under a cipher and who prices it, the mutants it was held to, and what is left open. No model or tool names, no counts that will move presented as standing facts. Write the message to a file and use git commit -F. It ends, after a blank line, with the attribution lines your own harness gives you for commits (a Co-Authored-By line and a Claude-Session line); if it gives you none, copy the two trailer lines of the previous commit (git log -1 --format=%B).
5. Do not push. Confirm git status --porcelain is empty and return the new sha.

The unit's account:
${JSON.stringify(impl, null, 1).slice(0, 9000)}

Reviewers' items still open after the last repair (put any that are true into the commit body's "left open"):
${JSON.stringify(unresolved, null, 1).slice(0, 4000)}`
}

const results = []
let stopped = null
const units = ONLY ? UNITS.filter(u => ONLY.includes(u.key)) : UNITS

for (const u of units) {
  log(`unit ${u.key} ${u.name}: building`)
  let impl = await agent(implementPrompt(u), { label: `build:${u.key}:${u.name}`, phase: 'Build', model: 'opus', schema: IMPL_SCHEMA })
  if (!impl) { stopped = `${u.key}: the builder returned nothing`; break }
  if (impl.blocked) { results.push({ unit: u.key, name: u.name, impl, unresolved: [], commit: null }); stopped = `${u.key}: blocked: ${impl.blocked}`; break }

  let unresolved = []
  const MAX_REVIEWS = 3
  for (let round = 0; round < MAX_REVIEWS; round++) {
    const reviews = (await parallel(LENSES.map(l => () =>
      agent(reviewPrompt(u, l, round, impl), { label: `review:${u.key}:${l.key}:r${round + 1}`, phase: 'Review', model: 'opus', schema: REVIEW_SCHEMA })
    ))).filter(Boolean)
    const blocking = reviews.flatMap(r => r.blocking || [])
    const extra = reviews.flatMap(r => r.extra_mutants || [])
    log(`unit ${u.key}: review ${round + 1}: ${blocking.length} blocking, ${extra.length} extra mutants`)
    unresolved = blocking
    if (!blocking.length && !extra.length) break
    const fixed = await agent(repairPrompt(u, round, blocking, extra, 'Two reviewers'), { label: `repair:${u.key}:r${round + 1}`, phase: 'Review', model: 'opus', schema: IMPL_SCHEMA })
    if (fixed) impl = fixed
    if (!blocking.length && !(fixed && fixed.engine_changed)) {
      log(`unit ${u.key}: the reviewers passed it and the extra mutants changed no engine code; no further review`)
      unresolved = []
      break
    }
    if (round === MAX_REVIEWS - 1) log(`unit ${u.key}: committed after the last repair without a further review; open items are in the result`)
  }
  if (impl.blocked) { results.push({ unit: u.key, name: u.name, impl, unresolved, commit: null }); stopped = `${u.key}: blocked in repair: ${impl.blocked}`; break }

  let c = await agent(commitPrompt(u, impl, unresolved), { label: `commit:${u.key}`, phase: 'Commit', model: 'opus', schema: COMMIT_SCHEMA })
  if (c && !c.committed) {
    log(`unit ${u.key}: the unit gate was red; one repair and one more try`)
    const fixed = await agent(repairPrompt(u, 9, (c.problems || []).map(p => ({ title: 'the unit gate is red', detail: p, where: 'sh scripts/check.sh' })), [], 'The commit step, whose gate was red,'), { label: `repair:${u.key}:gate`, phase: 'Commit', model: 'opus', schema: IMPL_SCHEMA })
    if (fixed) impl = fixed
    c = await agent(commitPrompt(u, impl, unresolved), { label: `commit:${u.key}:2`, phase: 'Commit', model: 'opus', schema: COMMIT_SCHEMA })
  }
  results.push({ unit: u.key, name: u.name, impl, unresolved, commit: c })
  if (!c || !c.committed) { stopped = `${u.key}: not committed`; break }
  log(`unit ${u.key}: committed ${c.sha}`)
}

let gate = null
if (!stopped && TAIL) {
  const gatePrompt = `${CONTEXT}
YOU RUN THE FULL GATE on HEAD, after every unit of this run has committed (git log ${BASE}..HEAD).
${RULES}
Run "sh scripts/check.sh" (every default group: build test stride lint wasm bench) in the background from a clean TMPDIR of your own, exit status captured, and wait for it; the bench group builds the suite. How to wait: write the exit status to the log after the command ("...; echo EXIT $? >> log"), then block on that line with the Monitor tool (an until-loop that greps the log for "^EXIT") or a foreground command that does the same, again and again until the line is there. A turn that ends while the gate still runs is taken as your answer, and the gate goes on running under whoever comes next: if you must answer without a verdict, stop the gate's processes first and say so. Nobody else writes the branch or the tree while you run: if HEAD's tree changes under you, say so and run again. Then the groups this run added outside the default set, each on its own (read scripts/check.sh for their names: the cost group at least), one at a time. Read each group's verdict from its exit status, never through a pipe. If a group is red for something a unit's commit broke mechanically (formatting, a lint, a struct literal in bench/), fix it and commit it as its own small follow-up commit in the repository's manner, ending with the same two attribution lines the unit commits end with (read one with git log -1), then run that group again. Red for anything else: report the failing test and its output; never skip, ignore or loosen a test. Report each new group's run time and the disk it used. Do not push. Finish with git status --porcelain empty and return HEAD.`
  gate = await agent(gatePrompt, { label: 'gate:full', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })
  if (!gate || !gate.green) {
    log('the full gate answered without a green verdict; one more run of it before anything is counted')
    const again = await agent(gatePrompt + `\n\nThis is the second run of this step. The first answered: ${JSON.stringify(gate, null, 1).slice(0, 3000)}\nIf it left a check.sh running, stop its processes and remove its TMPDIR before you start. If a group was red for a real reason, do not run it again: report it.`, { label: 'gate:full:2', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })
    if (again) gate = again
  }
}

if (!TAIL) return { stopped, results, gate: null, critic: null }

const critic = await agent(`${CONTEXT}
YOU ARE THE COMPLETENESS CRITIC for this run. Read-only on the repository (no edits, no builds); you write one file, ${W}/g1-built.md (from the shell if the Write tool refuses).
${stopped ? 'The run STOPPED EARLY: ' + stopped + '. The working tree may hold that unit\'s uncommitted work; say exactly what state it is in (git status, git diff --stat) and do not touch it.' : 'Every unit committed.'}

Read git log ${BASE}..HEAD with each commit's diff, ${W}/*/notes.md and mutants.md, ${PLAN}/g1.md, ${PLAN}/g2.md, forms.md and decisions.md. Then answer:
- holds: for each rule of forms.md that G1 was to gate (R0 the wall, R1 the door, R2 whole borrows, R3 advice, the cost job of section 5), the gate by file and test or script name, the red input that showed it red, and what it cannot see (be specific: a way to write the barred thing that passes).
- acceptance: forms.md's four acceptance tests (Z, N, K, F), each as it stands after G1: held, partly held and by what, or not yet and which stage holds it.
- concerns: a gate that can pass without running, a rewrite not shown right under a cipher, a plain-store row that moved, a departure from the plan with a weak reason, a doc sentence that is false. File and line.
- for_g2: what G2's plan (g2.md) must change because of what G1 built or found (names that moved, a hook that now exists, a count that differs).
- owner_items: what is the owner's to decide and what each choice costs.
Write it all to ${W}/g1-built.md, short lines, most important first. Return the same as structured data.

The units' results (data):
${JSON.stringify(results.map(r => ({ unit: r.unit, name: r.name, commit: r.commit, summary: r.impl && r.impl.summary, red: r.impl && r.impl.red_inputs, survivors: r.impl && r.impl.survivors, callgrind: r.impl && r.impl.callgrind, plain: r.impl && r.impl.plain_store_cost, cipher: r.impl && r.impl.cipher_changes, departures: r.impl && r.impl.plan_departures, not_built: r.impl && r.impl.not_built, not_verified: r.impl && r.impl.not_verified, unresolved: r.unresolved })), null, 1).slice(0, 60000)}

The full gate: ${JSON.stringify(gate).slice(0, 5000)}`,
  { label: 'critic', phase: 'Critic', model: 'opus', schema: CRITIC_SCHEMA })

return { stopped, results, gate, critic }
