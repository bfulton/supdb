export const meta = {
  name: 'zpages-wave1',
  description: 'Stages 0b (layout and sizing), 1b (point reads through the ordered index), 1d (selector core) and the forms-memory lead, each in its own worktree, each adversarially reviewed and fixed',
  phases: [
    { title: 'Build', detail: 'four tracks, one worktree each' },
    { title: 'Review', detail: 'a fresh reader per branch' },
    { title: 'Fix', detail: 'the findings, on a new branch' },
  ],
}

const SP = '/tmp/claude-0/-home-user-supdb/43547e2e-b745-59c0-b3a0-b32f7c7e7919/scratchpad'
const CTX = SP + '/wave1/ctx.txt'
const DESIGN = SP + '/design/zpages.md'
const HEAD = 'First read ' + CTX + ' completely: it is your brief and its rules bind you. The design is ' + DESIGN + '. '

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

const TRACKS = [
  {
    key: 'layout',
    branch: 'wave1-layout',
    prompt: 'TRACK A, branch wave1-layout: stage 0b of the design (section 8, row 0b; section 2 for the format; 7.1 for the sizing grid). Implement the layout half of the new module src/db/zpages.rs (declared in src/db.rs as a submodule, cfg(not(target_family = "wasm")), with the reason in a comment): the constants the design names in 2.1-2.3 (ZPAGE and the rest), the page header and its CRC (2.2), the trailer (2.3), fill_group (2.6: size a group so its compressed bytes fill one page, verified by decode and compare; a total function: any input yields a page or a defined decline), record_key_next (reading the key of the next record out of a decoded group), BLOOM_SIG and its pin test over 4,096 fixed keys, and encode and decode of the trailer and the fence entries. Nothing is wired to a read path, to Options or to the suite. Unit tests in the module: round trips; damage (a flipped byte fails the check the design says it fails); fill_group on adversarial input (empty, a record larger than a page, incompressible bytes, all-equal bytes, records of varied lengths, a group that ends exactly at capacity); determinism; the lz4_flex version and codec pinned as the design says. Then bench/examples/zsize.rs: run the real fill_group over the suite\'s record images, built the way the engine builds a stored compact record from a loaded partition (flatindex::stream_record or the engine\'s own encoder; check in the example that they are byte-equal to the stored records), at payload compressibility 0.5, 0.0 and 1.0 and for the four key shapes of 7.1; print records and bytes a page, the fill, compress calls a page, and decode, walk and CRC microseconds a page, and the occupancy at 10M keys. Run it (CPU only, a few minutes) and put the grid in the report. If records per page on the suite\'s shape come out under 36, stop and report: that is the design\'s stop rule.',
  },
  {
    key: 'ordread',
    branch: 'wave1-ordread',
    prompt: 'TRACK B, branch wave1-ordread: stage 1b of the design (section 8, row 1b; section 9.2 for L1; 7.2b "Faults by region"). Add Options::point_via_ord (default false). When set, a point read of a partition whose ordered index is uniform (a head is a whole key; OrdIndex::uniform) resolves the rank with OrdIndex::seek_exact, checks the key against the record, and reads the values with Blob::values_at; otherwise it takes the hash path unchanged. No new file, no format change, no change to any other path or to the default. Cover every site the design lists for a point read (read_all, and count if 9.2 says so). Add the arm supdb-nocacheordread (the supdb-nocache policy with the flag) and any twin the naming rules in bench/src/engines.rs require, mirrored at every site. Tests: the flag\'s reads equal the default\'s for every key of a store with partitions, level-0 pieces, a memtable, tombstones, multi-value keys and keys of several lengths (tests/db.rs\'s oracle is the pattern); a segment whose ordered index is missing or damaged falls back and answers the same; the arm differs from its default in the option alone (a bench test in the pattern of the existing arm tests). Do not run throughput measurements. Write, and do not run, the exact command lines for the paired 10M measurement the design registers (bench ab with --bound and the --ops-div option, which exists now) and for the one untimed perf record of the baseline arm\'s read cell, bucketed by the segment\'s regions.',
  },
  {
    key: 'select',
    branch: 'wave1-select',
    prompt: 'TRACK C, branch wave1-select: stage 1d of the design (section 8, row 1d; section 4 entire; 7.4 for the registered behaviours). Implement src/db/zselect.rs (declared in src/db.rs as a submodule, cfg(not(target_family = "wasm"))): the selector\'s pure core. ZSelect::step takes explicit inputs (a time, the model\'s predicted saving, the census windows, the current rung, the dwell and the rest 4.2-4.5 name) and returns a decision, per the transition table of 4.5, with its hysteresis and the one-switch-per-dwell rule. No clocks, no threads, no I/O, no engine types, so it is testable by replay. Every constant of 4.4 is a named const with the quantity it stands for. Replay tests, as traces: a square wave of fits and does-not-fit; a skewed workload that fits (never builds); a ramp through the window\'s edge; a flapping signal; a build that wastes; each of E1, E2, E3 exiting; in memory (never leaves FITS, never asks for a sample); the one-switch-per-dwell rule on every trace. Each asserts the behaviour 4.5 and 7.4 register. Wired to nothing; no machine time beyond building and testing.',
  },
  {
    key: 'formsmem',
    branch: 'wave1-formsmem',
    prompt: 'TRACK D, branch wave1-formsmem: the forms-memory lead (no section of the design; it is the gate the design\'s open question 6 names). Read docs/engine.md "Out of core: what the process holds beside the page cache", the CLAUDE.md shape entry "A structure maintained for a reader who was never coming", and in src/db.rs: Cached::bytes, shed, publish_form, publish_marker, drop_blocks, install_ahead, BlockTable, FormsState, State::forms and forms_bytes, retired_forms and RetiredForm, scan_cache_bytes, the copy_exact option. The question: at 3M keys the default arm\'s anonymous memory peaks at 760-820 MB at the lag sweep\'s full-rewrite point where the arm with no block cache holds 225-255 MB, and the arm whose cache is budgeted at 256 MB peaks the same (762-779 MB) with the writer\'s counted tables at 255 MB there, the allocator\'s in-use bytes at 483 MB and its free (retained) bytes at 287-303 MB. What holds the live bytes the budget does not count, and what are the allocator\'s free bytes? Find it by reading the code and by counting: add a census (a #[doc(hidden)] method on Db or Reader, or counters the bench prints in the lagmem line) of the capacity-based bytes of the writer\'s tables, the published forms (and whether a form the writer shed is still alive through a published Arc or a retired list), the scan snapshot, the forms\' Vec capacities against their lengths, the retired lists, and the frozen and live memtables\' arenas. Measure with: cd bench; env SUPDB_LAG_COUNTERS=1 TMPDIR=<private dir> ./target/release/bench run --scale quick --top 3000000 --bottom 3000000 --reps 1 --arms supdb-cache256 (stderr has the lagmem lines; the build is from your worktree, with the shared bench target) and the same for supdb-nocache and supdb. This is counting, not throughput; do not run other passes. Then fix what you find behind an Options flag, default off, with an arm pair mirrored at every site: the budget must bound the process (a shed retracts what it published, the bytes are counted by capacity, the slack is trimmed at build or the Vecs are built exact), and the allocator\'s retained free bytes are given back where the forms\' churn made them (a trim after a shed burst, or an arena the forms are built in). Verify with the same run that the budgeted arm\'s anonymous peak at the full-rewrite point falls to about the budget plus the nocache baseline, and report the before and after lagmem tables. The default\'s behaviour must not change (the flag is off). If what you find is not what this brief guesses, follow what you find.',
  },
]

phase('Build')
const out = await pipeline(
  TRACKS,
  t => agent(
    HEAD + t.prompt + '\n\nWork in your worktree; create the branch ' + t.branch + ' there at the start (git switch -c ' + t.branch + ') and commit to it. Report the branch and the head commit hash.',
    { label: 'build:' + t.key, phase: 'Build', isolation: 'worktree', schema: BUILD },
  ),
  (b, t) => {
    if (!b) return null
    return agent(
      HEAD + 'TASK: adversarially review the work on branch ' + b.branch + ' (head ' + b.head + '), made against the design for track "' + t.key + '". See it with git log and git diff in the repository (the branch is visible from any worktree: e.g. git diff $(git merge-base HEAD ' + b.branch + ') ' + b.branch + '; the base is the commit the branch was cut from). Do not trust the author\'s report below; read the code, run the narrow tests that exercise it (cargo test --profile checked with the shared target dirs and a private TMPDIR; no throughput runs), and look for the ways it breaks: the CLAUDE.md invariants and shapes (derived data keyed by identity, publish and epoch rules, wasm, the three readers agree, arms differing in one option, a path no test takes, a constant with two meanings), unwritten edge cases, tests that pass for the wrong reason (a test must fail when the thing it guards is broken: break it and see where the cost is small), claims in the report that the diff does not support, and divergence from the design that the report does not own up to. Report findings with severity, where, the problem and a concrete suggestion; verdict sound only if you tried hard and found nothing that matters.\n\nAUTHOR\'S REPORT (JSON):\n' + JSON.stringify(b),
      { label: 'review:' + t.key, phase: 'Review', schema: REVIEW },
    ).then(r => ({ b, r }))
  },
  (br, t) => {
    if (!br) return null
    const { b, r } = br
    const todo = r ? r.findings.filter(f => f.severity !== 'minor' || true) : []
    if (!r || todo.length === 0) return { b, r, fixed: null }
    return agent(
      HEAD + 'TASK: fix the review findings on branch ' + b.branch + ' for track "' + t.key + '". In your fresh worktree run git switch -c ' + t.branch + '-fix ' + b.branch + ' and work there. For each finding either fix it (and add the test that fails without the fix) or reject it with a reason you can defend from the code; do not accept a finding you have not checked. Keep the gates of the brief (fmt, clippy both crates, the tests under the checked profile). Commit on ' + t.branch + '-fix and report that branch and its head, what you fixed, what you rejected and why.\n\nFINDINGS (JSON):\n' + JSON.stringify(r) + '\n\nORIGINAL REPORT (JSON):\n' + JSON.stringify(b),
      { label: 'fix:' + t.key, phase: 'Fix', isolation: 'worktree', schema: BUILD },
    ).then(f => ({ b, r, fixed: f }))
  },
)
return out.filter(Boolean).map(x => ({
  build: x.b && { branch: x.b.branch, head: x.b.head, summary: x.b.summary, deviations: x.b.deviations_from_design, concerns: x.b.concerns, not_done: x.b.not_done, results: x.b.results },
  review: x.r && { verdict: x.r.verdict, findings: x.r.findings.length, blocking: x.r.findings.filter(f => f.severity === 'blocking').length },
  fixed: x.fixed && { branch: x.fixed.branch, head: x.fixed.head, summary: x.fixed.summary, concerns: x.fixed.concerns, not_done: x.fixed.not_done },
}))
