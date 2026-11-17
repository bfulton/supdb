export const meta = {
  name: 'stage0-tail',
  description: 'Finish stage 0 of the durability fixes after a third container restart: the full gate on the branch head, a reading by function of the instruction counts already taken against the base, the completeness critic, and a skeptic who checks the critic against the repository. Nothing is pushed.',
  phases: [
    { title: 'Gate', detail: 'sh scripts/check.sh, every default group, on HEAD', model: 'opus' },
    { title: 'Count', detail: 'read the callgrind outputs by function, base against HEAD', model: 'opus' },
    { title: 'Critic', detail: 'what is fixed, what is deferred, what the owner decides', model: 'opus' },
    { title: 'Check', detail: 'a skeptic holds the critic\'s account to the repository', model: 'opus' },
  ],
}

const S = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0'
const BASE = '3df85d22'

const CONTEXT = `
You are working in /home/user/supdb (Rust; the engine is src/db.rs and src/db/*.rs), on the branch that is checked out. This is stage 0 of a larger piece of work: fixing the plain (unencrypted) store's durability defects. Fifteen were found by reading; each was mapped, reproduced with failing tests at commit ${BASE}, and checked by a skeptic. This run fixes them one unit at a time. Units before yours are committed on top of ${BASE}: read "git log ${BASE}..HEAD" and their notes under ${S}/fix/ before you start, because the reproductions' line numbers and seams predate them. The first three units (delete-max, commit f8d130a8; log-errors, commit 0aa41df7; run-errors, commit 38378d1d) are committed and pushed. The first two built the kept error every later unit reuses (Db::lost, keep_lost, answered through pending_err) and the tests' file, tests/durable.rs; the third gave a direct run's writer its fault byte (AlignedWriter::fault: the RUN_FAULT_* seams and the RUN_ATTN_HELD/OWED/LOST states), writes at the offset the writer counts (pwritev_all), and Db::run_failed / Db::run_again. The container this runs in has been restarted twice, about three hours apart, and will be again: keep your notes file current as you work (the step you are on, where a runner writes its results), so that a restart loses a step and not the unit. A mutant runner writes its mutant into src/db.rs, and the second restart left one there: before the first mutant, keep the fixed files under your scratch directory with their sha256 sums, have the runner write each verdict to its results file as it gets it (not only at the end), and say in notes.md which copy is the fixed one.

Material (read what your unit needs; do not re-derive it):
- ${S}/report.md : the consolidated report. The table, the fix order, what needs the owner, and per defect the skeptic's reading: whether the state is reachable, the contract promised, the problems with the reproduction, the fix and its cost, and the mutants the fix's tests must fail under.
- ${S}/<defect-id>/test.md (the mapper's reading and test plan), notes.md, repro.patch (the reproducer's tests and seams against ${BASE}), run.log.
- ${S}/tests.patch : all fifteen reproductions as one formatted patch against ${BASE} (tests/s0_<id>.rs plus seams in src/db.rs and src/db/vault.rs), the seams' conflicts already merged by hand. Take test bodies and seams from here.
- ${S}/tests.log : every reproduction's failing run.
`

const RULES = `
RULES (the owner's; they are not yours to reopen)
1. Performance is this product's niche. The success path of a commit, a sync, a flush, a drain, a seal, a rotation, a manifest publish and every read gets no new syscall, no new wait on another thread, no new allocation and no new lock. The most a fix may add there is a load and a branch folded into a check the path already makes. If a correct fix needs more than that, do not build that part: say so under not_built with the smallest form you would price, and build the rest.
2. Nothing is disabled, limited or refused to make a fix easy. An arm in Options that keeps an older shape alive (bench/src/engines.rs lists the arms) stays, and stays sound; if a fix cannot hold under an arm, say so under not_built. Never remove an arm or make the store refuse one.
3. Encryption has to work with everything. The store has three modes (Encryption::None, Kernel, Cipher). Where the code you change has a cipher arm (for example Wal::write seals its pending frames as one record), the cipher arm gets the same fix and a test under Encryption::Cipher (tests/cipher.rs has the test cipher and its helpers). Where the defect has an analogue in a cipher-only mechanism (vault::persist, the memory files, the pager), check it by reading, write its failing test if a seam reaches it, fix it if the fix is local, and report it otherwise.
4. Failing test first. Before the fix, put the tests and their seams in, run them, and record the assertion each fails at (${S}/fix/<unit>/before.log). After the fix they pass (after.log). A test that cannot be made to fail before the fix is not evidence; say so and do not count it.
5. Tests live in tests/durable.rs, one test binary for all of stage 0 (the first unit creates it; later units append a section). Do not add fifteen binaries. Hand read callbacks through tests/common's kv and vf, never a closure straight to a generic read call (CLAUDE.md, the test group's compile). Take the bodies from tests.patch and deal with every problem the skeptic listed against them in report.md. A refusal is tested by its words and by the directory left as it was found. Seams follow Db::fail_next_presync: #[doc(hidden)] pub, no cfg, and free when unset: at most an is_some or a load beside a syscall the path already makes, never a store on a hot path.
6. Mutants. report.md lists, per defect, the mutants the fix's tests must fail under. Run each of them and at least three of your own on the lines you changed (a deleted line, a flipped operator, a swapped order). A mutant that survives means a missing test or a check nothing can reach: add the test, or take the check out. Keep the fixed files aside before the first mutant and put them back byte for byte after each one; record every mutant and the test that killed it in ${S}/fix/<unit>/mutants.md.
7. An existing test that encodes the behaviour being fixed is restated to the new contract: never deleted, ignored, or loosened into passing. List each under restated_tests with the reason. Any other existing test that goes red is a bug in your fix.
8. Every legit crash window in CLAUDE.md's paragraph "Crash discipline is an order, and every window in it is survivable" still opens as that paragraph says. tests/db.rs builds those windows on disk; they are the guard.
9. Docs. CLAUDE.md is notes for whoever picks the work up next: for each shape of bug you fix, one entry under "Shapes the bugs come in" in that file's own manner (it names the failure that produced the rule and ends in the rule; no standing figures, no counts that move), or an amendment to an entry already there when the shape has come back; and amend the "Crash discipline" paragraph wherever the order or a contract in it changed. Crate docs (the doc comments on SyncPolicy, Db::commit, Db::sync, Db::flush, Db::open) say what is true now, factually and with no history. README.md only if a sentence in it is now false. Anything a test could not show (a real errseq needs a dm-flakey device) is an open item in ROADMAP.md.
10. Mechanics. One cargo at a time, and no timing or benchmark run at all. The tests run as "cargo test --profile checked" (never the dev profile for a whole binary). Every cargo test and every test binary you run has TMPDIR exported to a directory of its own under ${S}/tmp/<unit> (a run without it leaves its stores in the system temp directory, and one such run already did); remove the directory when you finish; never glob /tmp. A command that may run past a few minutes goes in the background with its output and its exit status written to a log, and you wait on the log; read a verdict from the exit status or the "test result:" line, never through a pipe that swallows the status. About six gigabytes of disk are free: make no new target directory and no worktree. If the Write tool refuses a path under the scratchpad, write the file from the shell.
11. Git. Do not push. Do not stash, reset, or check out over the working tree: the unit's work lives there, uncommitted, until the commit step. Only the commit step commits.
12. Say what you did not verify. Do not write a claim you did not check.
`

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

const COUNT_SCHEMA = {
  type: 'object',
  properties: {
    ran: { type: 'boolean' },
    rows: { type: 'array', items: { type: 'object', properties: { probe: { type: 'string' }, function: { type: 'string' }, base: { type: 'string' }, head: { type: 'string' }, delta: { type: 'string' } }, required: ['probe', 'function', 'base', 'head', 'delta'] } },
    reading: { type: 'string' },
    tree_clean: { type: 'boolean' },
    head: { type: 'string' },
    problems: { type: 'array', items: { type: 'string' } },
  },
  required: ['ran', 'rows', 'reading', 'tree_clean', 'head', 'problems'],
}

const CRITIC_SCHEMA = {
  type: 'object',
  properties: {
    fixed: { type: 'array', items: { type: 'object', properties: { defect: { type: 'string' }, sha: { type: 'string' }, tests: { type: 'string' }, holds: { type: 'string' } }, required: ['defect', 'sha', 'tests', 'holds'] } },
    deferred: { type: 'array', items: { type: 'object', properties: { defect: { type: 'string' }, why: { type: 'string' }, needs: { type: 'string' } }, required: ['defect', 'why', 'needs'] } },
    concerns: { type: 'array', items: { type: 'string' } },
    owner_items: { type: 'array', items: { type: 'string' } },
    behaviour_changes: { type: 'array', items: { type: 'string' } },
    path: { type: 'string' },
    tree_clean: { type: 'boolean' },
  },
  required: ['fixed', 'deferred', 'concerns', 'owner_items', 'behaviour_changes', 'path', 'tree_clean'],
}


const CHECK_SCHEMA = {
  type: 'object',
  properties: {
    corrections: { type: 'array', items: { type: 'object', properties: { claim: { type: 'string' }, found: { type: 'string' }, fixed_in_file: { type: 'boolean' } }, required: ['claim', 'found', 'fixed_in_file'] } },
    missed: { type: 'array', items: { type: 'string' } },
    confirmed: { type: 'string' },
    not_checked: { type: 'array', items: { type: 'string' } },
    path: { type: 'string' },
  },
  required: ['corrections', 'missed', 'confirmed', 'not_checked', 'path'],
}

const STATE = `
WHERE THE RUN STANDS. All eight units are committed and on the remote; HEAD is 8b83dc03 and the tree is clean. In order on top of ${BASE} (git log ${BASE}..HEAD; the commits whose subject starts "In progress: " are snapshots and their take-backs, which change nothing between them, and 682b2c09 is design notes under scratch/): f8d130a8 delete-max, 0aa41df7 log-errors, 38378d1d run-errors, a424903d log-at-open, 10898a94 sync-run, 02b7a440 seal-sync, 2e8b173d slot-choice, 8b83dc03 open-order. The units' own results are in ${S}/fix/results-run3.json for the last five (their "sha" fields are the commits before they were re-made on the remote's head: match by subject) and in the notes under ${S}/fix/<unit>/ for all eight. ${S}/fix/followups.md is the orchestrator's numbered list of what the reviews and its own reading left (items 1 to 58), with which later unit each goes to; ${S}/fix/deferred.md is what was held back for pricing. The container has been restarted three times: write what you find to your file as you go.
`

const HOW_TO_WAIT = `How to wait for a long command: run it in the background with its exit status written to the log after it ("...; echo EXIT $? >> log"), then block on that line with the Monitor tool or a background Bash "until grep -q '^EXIT' log; do sleep 5; done", again until the line is there. A turn that ends while the command still runs is taken as your answer and the command goes on running under whoever comes next: if you must answer without a verdict, stop its processes first (the check.sh, its cargo and the test binary under it) and say so.`

const gatePrompt = `${CONTEXT}
${STATE}
YOU RUN THE FULL GATE on HEAD (8b83dc03). The run's first attempt at this step answered after two minutes with no verdict and left its check.sh running; it was stopped and its TMPDIR removed. Nothing has run the wasm or the bench group on any of the eight commits.
${RULES}
1. "git status --porcelain" is empty and "pgrep -af 'cargo|rustc|valgrind|check.sh'" shows nothing but your own; two readers of text files run beside you and build nothing. The container was restarted, so run "sh scripts/deps.sh" first and go on only at its "all deps ready" line.
2. Run "sh scripts/check.sh" (every default group: build test lint wasm bench) with TMPDIR exported to ${S}/tmp/gate2 (made empty first), log ${S}/fix/gate/check2.log. ${HOW_TO_WAIT} It takes a while: the test group is minutes once built, and the bench group builds the suite.
3. Read each group's verdict from the script's own verdict line and the exit status, never through a pipe; a test binary that ends with code 101 and no "test result:" line is a watchdog's exit (CLAUDE.md). The script stops at the first red group: after a red group is dealt with, run the groups after it too, so that every group has a verdict.
4. A group red for something a unit's commit broke mechanically (a struct literal in bench/ that lacks a new Options field, formatting, a lint, a committed figure the figures check redraws): fix it, run that group again, and commit it as its own small commit in the repository's manner, ending with the same two attribution lines the unit commits end with (git log -1 --format=%B 8b83dc03). Red for a failing test: do not paper over it and never skip, ignore or loosen a test. Run that one test again by name a few times alone ("cargo test --profile checked --test <file> <name> -- --exact", your own TMPDIR) and report how often it fails and its output; ${S}/fix/followups.md item 56 names one test of tests/durable.rs known to copy a live store under an unheld seal, whose rare failure reads "names seg-... missing from the directory": if that is the one, say so, and run the group again for its verdict.
5. Remove ${S}/tmp/gate2. Do not push. Finish with "git status --porcelain" empty and return HEAD, each group's verdict, and any follow-up commit's sha.`

const countPrompt = `${CONTEXT}
${STATE}
YOU READ THE INSTRUCTION COUNTS that were taken before the restart: what the eight fixes cost a store when nothing fails, by function, ${BASE} against HEAD, under callgrind. The counts are done and you run nothing but callgrind_annotate over the outputs that exist: no cargo, no valgrind (the full gate is building beside you).
${RULES}
What is there, under ${S}/fix/count/: run_counts.sh (the five probes and their arguments: an ordered durable load, an ordered buffered load, a writer's whole round, a point read as the control that must not move, a merge), notes.md (both sides were built from HEAD's bench/examples; "base" is ${BASE}'s src under them), head.<probe>.out and base.<probe>.out with their .self.txt and .incl.txt annotations, cmp.<probe>.self.txt (a first comparison by agg.py: read the script before trusting its columns), <side>.<probe>.stdout for what each probe printed, and src.diff. Check first that every probe of both sides exited 0 (head.status, base.status) and that the control probe's count is the same on both sides; if a file is missing or a probe failed, say which and count what is there.

Report, per probe and per function that moved: the base's count, HEAD's, and the difference per operation (per commit, per key, per read: read each example's source under bench/examples for how many of each it does at these arguments). At least for Db::commit, Db::commit_direct, Db::commit_staged, the log's write, frame and sync functions, Db::sync, the segment writer's append and sync (AlignedWriter, SegmentWriter), Db::delete, Maint::publish and the manifest's slot write, the seal's landing, Db::open and Db::create, and the vault's sync_dir and open_slot (a test's trace flag is loaded beside each). Read "git diff ${BASE} HEAD -- src" where a function moved, and say which fix the difference belongs to and whether it is a load and a branch on a check the path already made or more than that. A function whose count moved with no change to its source or its callees' sources is read for what changed around it (inlining, a struct's layout: Shared gained a test-only Vec, followups.md item 42) before the move is called the compiler's. Totals alone are not a reading: CLAUDE.md's paragraph on the plain store's price says why.

Write the table and the reading to ${S}/fix/count/reading.md as you go, and return the same: rows for the functions that moved (and one row per probe for its total), the reading in plain sentences, ran true if both sides were complete.`

const criticPrompt = (count) => `${CONTEXT}
${STATE}
YOU ARE THE COMPLETENESS CRITIC for stage 0. Read-only on the repository (no edits, no builds: the full gate is running beside you and its verdict will be added to your file after you); you write one file, ${S}/fixes.md (from the shell if the Write tool refuses), section by section as you go.

Read git log ${BASE}..HEAD with each unit commit's diff, tests/durable.rs, ${S}/report.md, ${S}/fix/results-run3.json, ${S}/fix/*/notes.md and mutants.md, ${S}/fix/deferred.md and ${S}/fix/followups.md. Then answer, defect by defect for all fifteen in report.md's table:
- fixed: which commit (the sha on the branch now), which tests hold it (by name), and in one sentence what they hold. Check that each reproduction in ${S}/tests.patch has a descendant in tests/durable.rs or a stated reason it has none.
- deferred or not built: why, and exactly what it needs (a pricing run and which pair, the shared log and run sequence, a device that can fail, the owner's word). followups.md says which unit of the next run each was given to: say where one has no home.
- concerns: a fix that looks incomplete on some path (a sync policy, an Options arm, the cipher arm), a restated test that got weaker, a mutant that survived, a refusal with no test of its words, a doc sentence that is now false (crate docs, CLAUDE.md, ROADMAP.md, README.md), a success-path cost. Give file and line. followups.md holds the orchestrator's; yours are the ones it does not have, and any of its you find wrong.
- behaviour_changes: every way a caller now sees something different (a call that answers Err where it answered Ok, an open that refuses where it opened, a call that waits or syncs where it did not), one line each, in plain words an owner can say yes or no to.
- owner_items: what is the owner's to decide and what each choice costs.
Write it all to ${S}/fixes.md, short lines, most important first, with a line "Full gate: pending" near the top. Return the same as structured data.

The instruction count's reading: ${JSON.stringify(count).slice(0, 12000)}`

const checkPrompt = (gate, count, critic) => `${CONTEXT}
${STATE}
YOU CHECK THE CRITIC. ${S}/fixes.md is the critic's account of stage 0: what each of fifteen defects got, what is left, what a caller sees differently, what the owner decides. The owner will read it and act on it, so every line of it has to be true of the repository as it stands. You are read-only on the repository (no builds, no test runs); you edit ${S}/fixes.md in place.
1. Every "fixed" row: the commit is on the branch (git log ${BASE}..HEAD), the tests it names exist by those names in tests/durable.rs (or tests/db.rs, tests/cipher.rs), and the sentence about what they hold is what the test asserts: read the test. A test named that does not exist, or that asserts something weaker, is a correction.
2. Every behaviour change: find the code that makes it so and the doc that says it (the crate docs on Db::open, Db::sync, Db::commit, Db::flush, SyncPolicy; README.md). One that the code does not do, or does only under some policy or arm, is a correction; one the code does that the list lacks is a miss. Look in particular at: every call that now answers a kept failure until the store is reopened; the opens that now refuse (their words); what a write does when it ends a run over a stopped seal queue; what an open now fsyncs.
3. Every concern with a file and line: the line says what the concern says.
4. What is missing: a defect of report.md's fifteen with no row; a "not_built" or "not_verified" entry of a unit (results-run3.json and the units' notes) that fixes.md does not carry; an item of ${S}/fix/followups.md marked new or standing that neither fixes.md nor a unit of the next run names (the next run's units and their briefs are in ${S}/price/price-durability.js: read the UNITS list); a ROADMAP.md item the units added that reads as done or as false.
5. Replace the line "Full gate: pending" with the gate's verdict below, group by group, in one or two lines; and put the instruction count's reading in three lines at most where the file speaks of cost.
Correct the file in place, each correction marked "(corrected: ...)" in a few words; add a short section "Not checked" for what you could not hold to the repository. Return the corrections, what was missed, what you confirmed in a sentence or two, and what you did not check.

The full gate: ${JSON.stringify(gate).slice(0, 5000)}
The instruction count: ${JSON.stringify(count && { ran: count.ran, reading: count.reading, problems: count.problems }).slice(0, 6000)}
The critic's structured answer: ${JSON.stringify(critic).slice(0, 30000)}`

phase('Gate')
const [gate, cc] = await parallel([
  async () => {
    let g = await agent(gatePrompt, { label: 'gate:full', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })
    if (!g || !g.green) {
      log('the full gate answered without a green verdict; one more run of it')
      const again = await agent(gatePrompt + `\n\nThis is the second run of this step. The first answered: ${JSON.stringify(g, null, 1).slice(0, 3000)}\nIf it left a check.sh running, stop its processes and remove its TMPDIR before you start. A group that was red for a real reason (a failing test that fails again alone) is reported, not run until it passes.`, { label: 'gate:full:2', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })
      if (again) g = again
    }
    return g
  },
  async () => {
    const count = await agent(countPrompt, { label: 'count:read', phase: 'Count', model: 'opus', schema: COUNT_SCHEMA })
    const critic = await agent(criticPrompt(count), { label: 'critic', phase: 'Critic', model: 'opus', schema: CRITIC_SCHEMA })
    return { count, critic }
  },
])
const count = cc && cc.count
const critic = cc && cc.critic
if (!gate || !gate.green) log('the full gate is not green: read its groups before anything is launched on this head')
const check = await agent(checkPrompt(gate, count, critic), { label: 'critic:check', phase: 'Check', model: 'opus', schema: CHECK_SCHEMA })
return { gate, count, critic, check }
