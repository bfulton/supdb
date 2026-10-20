export const meta = {
  name: 'zpages-wave2',
  description: 'Stage 2a: the forced compact-form arm for point reads and counts, built in its own worktree, reviewed through three lenses, fixed',
  phases: [
    { title: 'Build', detail: 'the form file, its attach, the read, the arm and the tests' },
    { title: 'Review', detail: 'three fresh readers: invariants, tests that could pass for the wrong reason, cost and footprint' },
    { title: 'Fix', detail: 'every finding, on a new branch' },
  ],
}

const SP = '/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad'
const CTX = SP + '/wave2/ctx.txt'
const DESIGN = SP + '/design/zpages.md'
const BASE = args.base
const BRANCH = 'wave2-zproof'
const HEAD = 'First read ' + CTX + ' completely: it is your brief and its rules bind you. The design is ' + DESIGN + '. The base branch is ' + BASE + ' (in the shared repository: run git switch -C <your branch> ' + BASE + ' at the start; your worktree was cut from an unrelated commit). '

const BUILD = {
  type: 'object',
  properties: {
    branch: { type: 'string' },
    head: { type: 'string' },
    summary: { type: 'string' },
    files: { type: 'array', items: { type: 'string' } },
    commands_run: { type: 'array', items: { type: 'object', properties: { command: { type: 'string' }, outcome: { type: 'string' } }, required: ['command', 'outcome'] } },
    results: { type: 'array', items: { type: 'object', properties: { what: { type: 'string' }, value: { type: 'string' } }, required: ['what', 'value'] } },
    deviations_from_design: { type: 'array', items: { type: 'string' } },
    concerns: { type: 'array', items: { type: 'string' } },
    not_done: { type: 'array', items: { type: 'string' } },
    notes_for_docs: { type: 'array', items: { type: 'string' } },
  },
  required: ['branch', 'head', 'summary', 'files', 'commands_run', 'results', 'deviations_from_design', 'concerns', 'not_done', 'notes_for_docs'],
}

const REVIEW = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['sound', 'fix', 'rethink'] },
    findings: { type: 'array', items: { type: 'object', properties: { severity: { type: 'string', enum: ['blocking', 'major', 'minor'] }, where: { type: 'string' }, problem: { type: 'string' }, suggestion: { type: 'string' } }, required: ['severity', 'where', 'problem', 'suggestion'] } },
    unverified: { type: 'array', items: { type: 'string' } },
  },
  required: ['verdict', 'findings', 'unverified'],
}

const LENSES = [
  {
    key: 'invariants',
    prompt: 'LENS: the engine\'s invariants. Read CLAUDE.md "Invariants a change must not break" and every entry of "Shapes the bugs come in" and hold the diff to each: the read takes the state and the gate once; nothing a reader follows moves (the cell is installed once and freed with the Seg: prove it against the epoch and the pin protocol, including a Seg dropped while a reader that loaded its cell is pinned); the form file is bound by content and a copy under another segment\'s name, a segment rewritten under the same id and end_seq, a stale temp file, a half-written file and a crash between the rename and the attach are all refused or swept; a derived thing keyed by position or by name outliving what it was derived from; an absence that means something (a failed check must never answer absent: find every path that returns 0 values or a zero count and show it cannot come from a failed z check); tombstone flags and multi-value runs and records with block-backed extents read exactly as the raw form reads them; the three readers still agree and the wasm module is unchanged in size and behaviour; Options::default() behaviour unchanged; the arm differs from its default in the option alone. Fuzz it: write a seeded differential run against the model with puts, multi-value keys, deletes, seals, merges and promotions, reads through the writer and through claimed handles, Inline and Background upkeep, a few hundred seeds, and report what it found. Break the gate, the CRC and the fallback in a scratch copy and see what notices.',
  },
  {
    key: 'tests',
    prompt: 'LENS: tests that could pass for the wrong reason. For every test the diff adds, break the thing it guards in a scratch copy (a mutation per guard: skip the verify_page call, skip the CRC check, skip the tiling check, accept a form file of another segment, return absent on a failed check, ignore the binding, drop the fallback counter, treat STORED as compressed, read the wrong page, off-by-one in the walk, wrong rank, the raw-serve branch always taken, the z branch never taken) and report which mutations no test noticed. A mechanical operator-flip pass over the new non-test code (scratchpad wave1 has a script: ' + SP + '/wave1/ordread-tools and the select reviewer\'s ' + SP + '/rev/exp/automut.py) is worth running on the core files. Check that each test asserts the path was taken (reads > 0 for Always, == 0 for Off) and that a store which silently fell back to the raw form would fail them. Run the suite twice and the new tests under load (four processes at once, each with its own TMPDIR) and report flakes with their cause. Check the damage tests really flip bytes inside pages that the reads then touch (not padding, not the fence), and that damage in a page nobody reads is not claimed as caught.',
  },
  {
    key: 'cost',
    prompt: 'LENS: cost and footprint. (1) The default path: callgrind the point read and the count with the option Off, base against head, on a release build at 100k keys, and report instructions a read; the resident cost of the option must be one load and one not-taken branch. (2) The pages a covered point read touches: on a small store built with the arm, drop the page cache for the raw segment and the form file (posix_fadvise DONTNEED, files not mapped), run a few thousand reads, and read mincore (or /proc/self/pagemap) of the raw segment\'s record, directory and hash regions and of the form file: the raw regions must be untouched, the form file must show one page a read; report the numbers. A read that touches the raw record region beside the z page defeats the form. (3) Memory: the fence and anything else on the heap, per 10M keys, from the code and from a measured 1M-key build; threads; Send and Sync; the build\'s bytes written, bytes read and CPU on a 1M-key store (counting, not timing). (4) The suite side: every site s1-s15 of design 6.2 that the arm needs, name() and guarantee() and open() agreement for the new arm, the one_pass refusal of a forced arm with form files not whole (make it fire on a known-red input), counters printed by ab, nothing added to ARMS or OOC_ARMS, figures.rs entries if the arm can reach a figure, and cargo test --profile checked for bench --lib. (5) Re-derive the PREDICTIONS from the numbers the report gives and say which would refute the thesis.',
  },
]

phase('Build')
const b = await agent(
  HEAD + 'TASK: build stage 2a as the brief says. Create the branch ' + BRANCH + ' from ' + BASE + '. Report the branch and the head commit hash.',
  { label: 'build:zproof', phase: 'Build', isolation: 'worktree', schema: BUILD },
)
if (!b) return { error: 'the build agent returned nothing' }

phase('Review')
const reviews = (await parallel(LENSES.map(l => () => agent(
  HEAD + 'TASK: adversarially review the work on branch ' + b.branch + ' (head ' + b.head + '), the stage 2a proof slice. See it with git log and git diff in the repository (the branch is visible from any worktree; the base is ' + BASE + '). Do not trust the author\'s report below; read the code, run the narrow tests that exercise it (cargo test --profile checked with a PRIVATE CARGO_TARGET_DIR under ' + SP + '/wave2/tgt-review-' + l.key + ' and a private TMPDIR; no throughput runs), and look for the ways it breaks. ' + l.prompt + ' Report findings with severity, where, the problem and a concrete suggestion; verdict sound only if you tried hard and found nothing that matters.\n\nAUTHOR\'S REPORT (JSON):\n' + JSON.stringify(b),
  { label: 'review:' + l.key, phase: 'Review', schema: REVIEW },
)))).filter(Boolean)

phase('Fix')
const findings = reviews.flatMap(r => r.findings.map(f => ({ ...f })))
let fixed = null
if (findings.length > 0) {
  fixed = await agent(
    HEAD + 'CONTINUATION: a previous attempt at this task was stopped by a container restart, and two instances of it shared one worktree (read AGENT-NOTE.txt there). Its UNCOMMITTED work, about 900 lines in src/db.rs, src/db/zpages.rs, src/db/zfile.rs, tests/zpages.rs and bench/src/engines.rs, is in /home/user/supdb/.claude/worktrees/wf_d4132f43-9fb-5 (branch wave2-zproof-fix, no commits of its own); its scratch is under ' + SP + '/wave2/fix. It did not compile at its last note. Start by saving it: git -C /home/user/supdb/.claude/worktrees/wf_d4132f43-9fb-5 diff > ' + SP + '/wave2/prev-fix.patch, and copy its untracked files (git ls-files --others --exclude-standard there); then in your own fresh worktree, after the git switch below, apply the patch, get it compiling, and READ EVERY HUNK against the findings before you keep it: it is a draft by two agents who divided src and tests between them and had not yet met on the names. SCOPE: the findings include defects that exist WITHOUT the second form (the compaction fence error, the tiling debug_assert and the writer hang, wrong raw scan answers, the block-cache copy disagreement, the cut_known assertion, a Snapshot handle reading a later write): do NOT fix those here, two hunts own them. The empty key as the first write of a fresh memtable is fixed on branch fix-emptykey (commit e2e1301, with its test): do not fix it again; if your tests write an empty key first, merge nothing, leave that case #[ignore]d with a comment naming that branch. Everything about the second form, the bench arm and its tests (the flaky bench lib test that races the allocator test, the form file advice following the store\'s mode, the orphan and temp sweep at open, the decodes counter, the refusals, the registered predictions P4-P6 to recentre, the +3 instruction figure, the missing tests of the reviews) is yours. Keep the invariants reviewer\'s oracle (' + SP + '/wave2/review-inv/keep/zreview.rs) out of your branch. TASK: fix the review findings on branch ' + b.branch + '. In your fresh worktree run git switch -C ' + BRANCH + '-fix2 ' + b.branch + ' and work there. For each finding either fix it (and add the test that fails without the fix) or reject it with a reason you can defend from the code; do not accept a finding you have not checked. Keep the gates of the brief. Commit on ' + BRANCH + '-fix2 and report that branch and its head, what you fixed, what you rejected and why.\n\nFINDINGS (JSON):\n' + JSON.stringify(reviews) + '\n\nORIGINAL REPORT (JSON):\n' + JSON.stringify(b),
    { label: 'fix:zproof', phase: 'Fix', isolation: 'worktree', schema: BUILD },
  )
}
return { build: { branch: b.branch, head: b.head, summary: b.summary, deviations: b.deviations_from_design, concerns: b.concerns, not_done: b.not_done }, reviews: reviews.map(r => ({ verdict: r.verdict, n: r.findings.length, blocking: r.findings.filter(f => f.severity === 'blocking').length })), fixed: fixed && { branch: fixed.branch, head: fixed.head, summary: fixed.summary, concerns: fixed.concerns, not_done: fixed.not_done } }
