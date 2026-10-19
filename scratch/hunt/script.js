export const meta = {
  name: 'snapshot-hunt',
  description: 'Find, force, fix and review the defect that lets a Snapshot handle read a write made after it (and the cut_known assertion beside it)',
  phases: [
    { title: 'Hunt', detail: 'three investigators with different methods, a second round if nobody has a forced failure' },
    { title: 'Fix', detail: 'the cause, with a deterministic regression test' },
    { title: 'Review', detail: 'a fresh adversary tries to break the fix and finds its siblings' },
  ],
}

const SP = '/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad'
const CTX = SP + '/hunt/ctx.txt'
const HEAD = 'First read ' + CTX + ' completely: it is your brief and its rules bind you. '
const PARENT = 'claude/performance-dimensions-analysis-gijutv'

const HUNT = {
  type: 'object',
  properties: {
    label: { type: 'string' },
    branch: { type: 'string' },
    head: { type: 'string' },
    summary: { type: 'string' },
    reproducer: {
      type: 'object',
      properties: {
        file: { type: 'string' },
        how_to_run: { type: 'string' },
        failures: { type: 'integer' },
        runs: { type: 'integer' },
        profile: { type: 'string' },
      },
      required: ['file', 'how_to_run', 'failures', 'runs', 'profile'],
    },
    root_cause_confirmed: { type: 'boolean' },
    root_cause: { type: 'string' },
    forcing_schedule: { type: 'string' },
    first_defect_and_second_defect_same_cause: { type: 'string' },
    candidates: { type: 'array', items: { type: 'object', properties: { what: { type: 'string' }, tested_how: { type: 'string' }, outcome: { type: 'string' } }, required: ['what', 'tested_how', 'outcome'] } },
    commit_that_introduced_it: { type: 'string' },
    not_done: { type: 'array', items: { type: 'string' } },
  },
  required: ['label', 'branch', 'head', 'summary', 'reproducer', 'root_cause_confirmed', 'root_cause', 'forcing_schedule', 'first_defect_and_second_defect_same_cause', 'candidates', 'commit_that_introduced_it', 'not_done'],
}

const FIX = {
  type: 'object',
  properties: {
    branch: { type: 'string' },
    head: { type: 'string' },
    summary: { type: 'string' },
    cause: { type: 'string' },
    regression_tests: { type: 'array', items: { type: 'string' } },
    loops: { type: 'array', items: { type: 'object', properties: { what: { type: 'string' }, runs: { type: 'integer' }, failures_before: { type: 'integer' }, failures_after: { type: 'integer' } }, required: ['what', 'runs', 'failures_before', 'failures_after'] } },
    commands_run: { type: 'array', items: { type: 'object', properties: { command: { type: 'string' }, outcome: { type: 'string' } }, required: ['command', 'outcome'] } },
    siblings: { type: 'array', items: { type: 'string' } },
    concerns: { type: 'array', items: { type: 'string' } },
    not_done: { type: 'array', items: { type: 'string' } },
    notes_for_docs: { type: 'array', items: { type: 'string' } },
  },
  required: ['branch', 'head', 'summary', 'cause', 'regression_tests', 'loops', 'commands_run', 'siblings', 'concerns', 'not_done', 'notes_for_docs'],
}

const REVIEW = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['sound', 'fix', 'rethink'] },
    findings: { type: 'array', items: { type: 'object', properties: { severity: { type: 'string', enum: ['blocking', 'major', 'minor'] }, where: { type: 'string' }, problem: { type: 'string' }, suggestion: { type: 'string' } }, required: ['severity', 'where', 'problem', 'suggestion'] } },
    loops: { type: 'array', items: { type: 'object', properties: { what: { type: 'string' }, runs: { type: 'integer' }, failures: { type: 'integer' } }, required: ['what', 'runs', 'failures'] } },
    unverified: { type: 'array', items: { type: 'string' } },
  },
  required: ['verdict', 'findings', 'loops', 'unverified'],
}

const METHODS = [
  {
    key: 'stress',
    prompt: 'METHOD: stress and instrument. Write a standalone test file in your worktree that reproduces the first defect on the current head WITHOUT the second form (copy the scenario from the brief; no zpages), loop it with the built executable over seeds and TMPDIRs, and RAISE the rate: more rounds, more writes between the snapshot and its scans, a second reader thread hammering scans, Background(1..3) and Inline upkeep, the seal held and not held, a lower thread priority for the upkeep, smaller partitions, more blocks. Report the rate of every variant. At each failure capture everything the engine can say about the key: Reader::debug_key (src/db.rs) for the writer and the handle, the handle\'s pinned state generation, mark (length, count, watermark), the forms position of the writer and of the published forms, the snapshot length it adopted, snapshot_builds, upkeep_counts, canonical_forms. Then name the mechanism from the capture, not from a story, and force it with a schedule (a hold hook or a deliberately placed yield in a scratch patch) so that it fails every time.',
  },
  {
    key: 'bisect',
    prompt: 'METHOD: history. Using a standalone reproducer you write (the first defect without the second form), find the commit that introduced the defect: run it on the head, then walk back through git log of the parent branch (git bisect run is fine if you write the script; each step builds in the dev profile, loops the reproducer 300 times or until it fails, and treats a clean 600 as good; remember that a rate of 1 in 200 needs the loop to be long enough and that a commit before the block cache existed cannot be bad). Then read that commit\'s diff and everything it made possible, and say which line lets a handle at an older position read a newer write. The second defect (the cut_known assertion) may have been introduced by the same commit or another: bisect it the same way. Report the commit, the evidence (counts per commit, both sides of the boundary), and a forcing schedule for the mechanism you name.',
  },
  {
    key: 'audit',
    prompt: 'METHOD: audit by reading, then force. Read, in this order, every Shapes entry of CLAUDE.md about marks, carries, tables, looks, forms, snapshots, handles and the upkeep, then the code they name, then the whole path of a Snapshot handle\'s FIRST scan over a block-cache store whose writer has committed more since the snapshot: where the handle gets each block\'s form (its own table, a published canonical form, an adopted scan snapshot, the builder ahead\'s install, Cached::Block copies), what bounds each of them to the handle\'s mark (log length, entry count, watermark, state generation), which of them is shared and mutated in place by the writer or the upkeep thread, and what the debug_assert in cut_known assumes about the walk and the cuts. Produce a list of candidate holes, each with a concrete schedule that would produce the failure, then FORCE each in a scratch patch with a hold hook or a yield and report which fail deterministically. A candidate that cannot be forced is reported as such with the attempt. Stop at the first mechanism you force, and then check whether it also explains the assertion.',
  },
]

phase('Hunt')
const reports = []
for (let round = 0; round < 2; round++) {
  const prior = reports.length ? '\n\nTHE REPORTS OF THE ROUND BEFORE (JSON; treat them as claims to check, not facts):\n' + JSON.stringify(reports) : ''
  const got = (await parallel(METHODS.map(m => () => agent(
    HEAD + 'TASK (round ' + (round + 1) + '): ' + m.prompt + ' Your label is ' + m.key + round + '; create the branch hunt-' + m.key + round + ' from ' + PARENT + ' (git switch -C hunt-' + m.key + round + ' ' + PARENT + ') and commit your reproducer and any forcing patch there. Report the branch and head.' + prior,
    { label: 'hunt:' + m.key + ':r' + (round + 1), phase: 'Hunt', isolation: 'worktree', schema: HUNT },
  )))).filter(Boolean)
  reports.push(...got)
  log('round ' + (round + 1) + ': ' + got.map(g => g.label + (g.root_cause_confirmed ? ' CONFIRMED' : ' open')).join(', '))
  if (got.some(g => g.root_cause_confirmed)) break
}

const confirmed = reports.filter(r => r.root_cause_confirmed)
if (confirmed.length === 0) return { outcome: 'no confirmed cause', reports }

phase('Fix')
const fix = await agent(
  HEAD + 'TASK: fix the defect. The investigators\' reports are below (JSON). First verify the confirmed cause yourself by running its forcing schedule on the head and seeing it fail; if two reports name different causes, force both and decide. Then, in your own worktree (git switch -C hunt-fix ' + PARENT + '), fix the cause (and the second defect, the cut_known assertion, if it is the same cause or has one of its own you can force); add a regression test that FAILS DETERMINISTICALLY without the fix (it forces the schedule with the engine\'s hold hooks or a new #[doc(hidden)] hook in the style of hold_seal_landing, and the test says which mutation of the fix it catches); run the standalone reproducer in a loop of at least 1500 runs in the checked profile with zero failures (and report the same loop on the unfixed head for the rate); look for the same mistake at every sibling site (the same shape in another path that adopts, carries or reads a published structure past a mark) and fix or report each; keep the gates (fmt, clippy -D warnings in the root and bench, cargo test --profile checked --no-fail-fast for the root, serialised as the brief says). Add the CLAUDE.md "Shapes the bugs come in" entry in this file\'s voice (the failure, how it was found, the rule). Commit on hunt-fix.\n\nREPORTS (JSON):\n' + JSON.stringify(reports),
  { label: 'fix:snapshot', phase: 'Fix', isolation: 'worktree', schema: FIX },
)
if (!fix) return { outcome: 'fix agent returned nothing', reports }

phase('Review')
const review = await agent(
  HEAD + 'TASK: adversarially review branch ' + fix.branch + ' (head ' + fix.head + '). Do not trust the author. Run the standalone reproducer in the checked profile for at least 1000 runs, Inline and Background, and report failures; break the fix in a scratch copy (each line of it) and see which test notices; read every place that adopts, carries or reads a published structure past a mark (the sibling sites) and say which are exposed to the same mistake; check that the regression test is deterministic (run it 200 times) and fails on the unfixed head; check the cost of the fix on the paths it touches (instruction counts with callgrind on a release build, 100k keys: the scan path and the point read, base against head) and that the default behaviour is otherwise unchanged. Report findings with severity, where, the problem and a concrete suggestion; verdict sound only if you tried hard and found nothing that matters.\n\nAUTHOR REPORT (JSON):\n' + JSON.stringify(fix),
  { label: 'review:fix', phase: 'Review', schema: REVIEW },
)
return { reports: reports.map(r => ({ label: r.label, confirmed: r.root_cause_confirmed, cause: r.root_cause, commit: r.commit_that_introduced_it })), fix: { branch: fix.branch, head: fix.head, cause: fix.cause, summary: fix.summary, loops: fix.loops, siblings: fix.siblings, concerns: fix.concerns }, review }
