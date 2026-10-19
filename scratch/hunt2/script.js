export const meta = {
  name: 'engine-fuzz-campaign',
  description: 'Adopt a differential fuzz of the engine against a model, cluster its failures, root-cause and fix each cluster with a deterministic regression test, review every fix, integrate and re-run',
  phases: [
    { title: 'Triage', detail: 'port the fuzz, run thousands of seeds, cluster the failures' },
    { title: 'Fix', detail: 'one agent per cluster: minimise, root-cause, fix, regression test' },
    { title: 'Review', detail: 'a fresh adversary per fix' },
    { title: 'Integrate', detail: 'merge the fixes, rerun the fuzz on fresh seeds, report what is left' },
  ],
}

const SP = '/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad'
const CTX = SP + '/hunt2/ctx.txt'
const HEAD = 'First read ' + CTX + ' completely: it is your brief and its rules bind you. '
const PARENT = 'claude/performance-dimensions-analysis-gijutv'

const TRIAGE = {
  type: 'object',
  properties: {
    branch: { type: 'string' },
    head: { type: 'string' },
    harness_file: { type: 'string' },
    run_command: { type: 'string' },
    seeds_run_cache_off: { type: 'integer' },
    seeds_run_cache_on: { type: 'integer' },
    failures_cache_off: { type: 'integer' },
    failures_cache_on: { type: 'integer' },
    clusters: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          signature: { type: 'string' },
          kind: { type: 'string', enum: ['wrong_answer', 'panic', 'error', 'hang', 'abort', 'other'] },
          cache: { type: 'string', enum: ['off', 'on', 'both'] },
          count: { type: 'integer' },
          seeds: { type: 'array', items: { type: 'integer' } },
          first_failure_text: { type: 'string' },
          config_notes: { type: 'string' },
        },
        required: ['id', 'signature', 'kind', 'cache', 'count', 'seeds', 'first_failure_text', 'config_notes'],
      },
    },
    summary: { type: 'string' },
    not_done: { type: 'array', items: { type: 'string' } },
  },
  required: ['branch', 'head', 'harness_file', 'run_command', 'seeds_run_cache_off', 'seeds_run_cache_on', 'failures_cache_off', 'failures_cache_on', 'clusters', 'summary', 'not_done'],
}

const FIX = {
  type: 'object',
  properties: {
    cluster_id: { type: 'string' },
    branch: { type: 'string' },
    head: { type: 'string' },
    summary: { type: 'string' },
    root_cause: { type: 'string' },
    evidence: { type: 'string' },
    minimal_repro: { type: 'string' },
    regression_tests: { type: 'array', items: { type: 'string' } },
    cluster_seeds_after: { type: 'object', properties: { runs: { type: 'integer' }, failures: { type: 'integer' } }, required: ['runs', 'failures'] },
    fresh_seeds_after: { type: 'object', properties: { runs: { type: 'integer' }, failures: { type: 'integer' }, control_failures_unfixed: { type: 'integer' } }, required: ['runs', 'failures', 'control_failures_unfixed'] },
    commands_run: { type: 'array', items: { type: 'object', properties: { command: { type: 'string' }, outcome: { type: 'string' } }, required: ['command', 'outcome'] } },
    siblings: { type: 'array', items: { type: 'string' } },
    concerns: { type: 'array', items: { type: 'string' } },
    not_done: { type: 'array', items: { type: 'string' } },
    notes_for_docs: { type: 'array', items: { type: 'string' } },
  },
  required: ['cluster_id', 'branch', 'head', 'summary', 'root_cause', 'evidence', 'minimal_repro', 'regression_tests', 'cluster_seeds_after', 'fresh_seeds_after', 'commands_run', 'siblings', 'concerns', 'not_done', 'notes_for_docs'],
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

const INTEGRATE = {
  type: 'object',
  properties: {
    branch: { type: 'string' },
    head: { type: 'string' },
    summary: { type: 'string' },
    merged: { type: 'array', items: { type: 'string' } },
    fuzz_after: { type: 'object', properties: { seeds_cache_off: { type: 'integer' }, failures_cache_off: { type: 'integer' }, seeds_cache_on: { type: 'integer' }, failures_cache_on: { type: 'integer' } }, required: ['seeds_cache_off', 'failures_cache_off', 'seeds_cache_on', 'failures_cache_on'] },
    remaining_clusters: { type: 'array', items: { type: 'string' } },
    gates: { type: 'array', items: { type: 'object', properties: { command: { type: 'string' }, outcome: { type: 'string' } }, required: ['command', 'outcome'] } },
    concerns: { type: 'array', items: { type: 'string' } },
    not_done: { type: 'array', items: { type: 'string' } },
  },
  required: ['branch', 'head', 'summary', 'merged', 'fuzz_after', 'remaining_clusters', 'gates', 'concerns', 'not_done'],
}

phase('Triage')
const triage = await agent(
  HEAD + 'TASK: TRIAGE. Create the branch hunt2-base from ' + PARENT + ' (git switch -C hunt2-base ' + PARENT + ') and merge the branch fix-emptykey into it (it fixes the empty-key first write). Adopt the reviewer\'s fuzz as a test file of the repository: read mkbase.py and run it to produce the version for an engine without the second form, put the result at tests/fuzz.rs (a standalone integration test; keep the seed range and every knob as environment variables as the original has them; a default of a few seeds that run in seconds so it can live in the gate later; no dependence on anything outside the repository), make it build and run on the head, and commit it. Then run it: at least 2,500 consecutive seeds with the block cache OFF and at least 1,000 with the cache ON (ZFUZZ_CACHE), in the checked profile in a private target (flock the compile), three processes at a time with a timeout per seed so a hang is a result and not a stall (the machine is shared and its load average may be 20-50: a seed that times out is rerun ALONE with a longer timeout before it is called a hang, and a seed that completes alone is a slow seed, not a failure), each seed in its own TMPDIR that is removed after; record every failure with its seed, its configuration line, and its first failure text. CLUSTER the failures by what failed (the assertion text or error text with the numbers stripped; the shape of a wrong answer: a value out of order, a duplicate, a missing value, a deleted key listed, a count; a hang; an abort), re-run each failing seed twice to see whether it is deterministic (the same failure at the same step) and say so, and for each cluster list at least three seeds, preferring the ones whose runs are shortest. A failure that reproduces only with the cache ON goes in a cluster of its own with cache set to on. Do not fix anything here, except what it takes to make the harness build and report. Report the branch and head, the exact command that reruns any seed, the counts, and the clusters.',
  { label: 'triage', phase: 'Triage', isolation: 'worktree', schema: TRIAGE },
)
if (!triage) return { outcome: 'triage returned nothing' }
log('triage: ' + triage.clusters.length + ' clusters; off ' + triage.failures_cache_off + '/' + triage.seeds_run_cache_off + ', on ' + triage.failures_cache_on + '/' + triage.seeds_run_cache_on)
const mine = triage.clusters.filter(c => c.cache !== 'on').slice(0, 6)
const skipped = triage.clusters.filter(c => !mine.includes(c)).map(c => c.id + ' (' + c.signature + ', cache ' + c.cache + ')')
if (mine.length) log('fixing: ' + mine.map(c => c.id).join(', ') + '; left to the other hunt or over the cap: ' + skipped.join('; '))

phase('Fix')
const results = await pipeline(
  mine,
  c => agent(
    HEAD + 'TASK: FIX CLUSTER ' + c.id + '. Create the branch hunt2-fix-' + c.id + ' from ' + triage.branch + ' (git switch -C hunt2-fix-' + c.id + ' ' + triage.branch + '); it has the fuzz at ' + triage.harness_file + '. The cluster: ' + JSON.stringify(c) + '. Rerun command: ' + triage.run_command + '. (1) Reproduce it, deterministically if the seed is, and MINIMISE it: the smallest configuration and step count that still fails, then a test written by hand with the engine\'s hold hooks (hold_seal_landing, hold_seal_durable, Upkeep::Inline, publish_in_background off) so that it fails EVERY time on the unfixed head, not by luck. (2) Find the root cause by reading the code the failure passes through and by taking mechanisms away one at a time; name the line. Read CLAUDE.md\'s shapes about the code you end up in. (3) Fix the cause, not the symptom: no retry, no tolerance, no swallowing an error that is a symptom; if the engine refuses a configuration, say why in check_options and test the refusal; if a debug_assert is wrong, say why with the invariant it was to hold. (4) The regression test(s) must fail deterministically on the unfixed head and pass on the fixed one; say which line removed breaks which test. (5) Rerun every seed of the cluster, then a FRESH range of at least 1,000 seeds the fix has not seen, in the checked profile, and a CONTROL range of the same size on the unfixed head, so the claim is a rate against a rate. (6) Look for the same mistake at every sibling site and fix or report each. (7) Keep the gates of the brief (fmt, clippy -D warnings in the root and bench, cargo test --profile checked --no-fail-fast for the root, serialised). (8) Add the CLAUDE.md "Shapes the bugs come in" entry in that file\'s voice (the failure, how it was found, the rule), placed at the end of the shapes. Commit on your branch. If you find that two clusters have one cause, say so and fix the cause once.',
    { label: 'fix:' + c.id, phase: 'Fix', isolation: 'worktree', schema: FIX },
  ),
  (fx, c) => {
    if (!fx) return null
    return agent(
      HEAD + 'TASK: REVIEW the fix of cluster ' + c.id + ' on branch ' + fx.branch + ' (head ' + fx.head + '; base ' + triage.branch + '). Do not trust the author. (a) Run the fuzz in the checked profile on at least 1,500 FRESH seeds, cache off and cache on, three at a time with a timeout per seed, and report failures by cluster (anything not in the author\'s cluster is a finding for the parent, with its seed). (b) Break the fix in a scratch copy, line by line, and see which test notices. (c) Run the regression test(s) 200 times each and on the unfixed head (they must fail there every time). (d) Read every sibling site of the same mistake and say which are exposed. (e) Check that the fix does not cost the default paths: instruction counts with callgrind on a release build, 100k keys, point read and scan, base against head, if the fix is on those paths. Report findings with severity, where, the problem and a concrete suggestion; verdict sound only if you tried hard and found nothing that matters.\n\nAUTHOR REPORT (JSON):\n' + JSON.stringify(fx),
      { label: 'review:' + c.id, phase: 'Review', schema: REVIEW },
    ).then(r => ({ cluster: c, fix: fx, review: r }))
  },
)

const done = results.filter(Boolean)
phase('Integrate')
const integrated = await agent(
  HEAD + 'TASK: INTEGRATE. Create the branch hunt2-integrated from ' + triage.branch + ' and merge into it, one at a time, the fix branches below (resolve conflicts with care: two fixes may touch one function; keep both fixes\' tests). For each review finding of severity blocking or major that its author did not already address, fix it here (small, with a test) or report it as not done with the reason. Then: run the fuzz on at least 3,000 FRESH seeds with the cache off and 1,500 with the cache on (checked profile, three at a time, a timeout per seed) and report the failures by cluster, which are the work still to do; run fmt, clippy -D warnings (root and bench) and cargo test --profile checked --no-fail-fast for the root (serialised, one checked compile of tests/db.rs at a time). Do not run scripts/check.sh whole.\n\nFIXES AND REVIEWS (JSON):\n' + JSON.stringify(done.map(d => ({ cluster: d.cluster.id, branch: d.fix.branch, head: d.fix.head, summary: d.fix.summary, review: d.review && { verdict: d.review.verdict, findings: d.review.findings, loops: d.review.loops } }))) + '\n\nCLUSTERS NOT FIXED (cache on, or over the cap): ' + skipped.join('; ') + '\n\nTRIAGE (JSON):\n' + JSON.stringify(triage),
  { label: 'integrate', phase: 'Integrate', isolation: 'worktree', schema: INTEGRATE },
)
return { triage: { branch: triage.branch, head: triage.head, clusters: triage.clusters.map(c => ({ id: c.id, signature: c.signature, kind: c.kind, cache: c.cache, count: c.count })), counts: { off: [triage.failures_cache_off, triage.seeds_run_cache_off], on: [triage.failures_cache_on, triage.seeds_run_cache_on] } }, fixes: done.map(d => ({ cluster: d.cluster.id, branch: d.fix.branch, head: d.fix.head, root_cause: d.fix.root_cause, review: d.review && d.review.verdict })), integrated }
