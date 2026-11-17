export const meta = {
  name: 'stage0-fix-durability-3',
  description: 'Continue fixing the plain store durability defects reproduced at 3df85d22 after a second container restart, from the fourth unit on, one unit at a time: failing test first, fix, mutants, two independent read-only reviews, repair, unit gate and a local commit; then the full gate, an instruction count of the write path against the base, and a completeness critic. Nothing is pushed.',
  phases: [
    { title: 'Fix', detail: 'one unit at a time: tests in and shown failing, the fix, mutants', model: 'opus' },
    { title: 'Review', detail: 'two read-only skeptics per unit; repair until clean', model: 'opus' },
    { title: 'Commit', detail: 'unit gate (build test lint) and a local commit', model: 'opus' },
    { title: 'Gate', detail: 'sh scripts/check.sh, every default group', model: 'opus' },
    { title: 'Count', detail: 'callgrind by function, 3df85d22 against HEAD', model: 'opus' },
    { title: 'Critic', detail: 'what is fixed, what is deferred, what the owner decides', model: 'opus' },
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

const UNITS = [
  {
    key: 'log-at-open',
    resumed: true,
    roundsDone: 1,
    defects: ['sync-completion-order', 'wal-fresh-header'],
    brief: `Two defects of the log at open. (1) Filed as sync-completion-order; the named mechanism was refuted, and what was reproduced is "replayed frames marked synced": Wal::open_append sets synced = written for frames it read back at replay, which after a process kill may be in the page cache and not on the device, so a sync or flush after the reopen answers Ok and a power cut then loses them. Fix: frames found at replay are not known durable (synced starts at the header), and older log files replayed at open go on the loose list, so the first sync by policy covers them; a chained header beside an unsynced predecessor must not make a later power cut refuse the open. A clean reopen (nothing replayed) issues no fdatasync: hold that with a control test and a counting seam. (2) wal-fresh-header: a log file no sync ever covered (a store just created; a fresh file after a rotation; a store whose commits all went to a direct run) can come back from a power cut with its first page as zeroes, and every open then refuses with "not a supdb WAL". Fix at the open, never at the write: reading an all-zero header of a file no sync covered as a log with nothing in it. Do not add a sync at creation or at a rotation (CLAUDE.md, "A sync the policy never asked for, at the rotation this time").

Work out and state exactly when a zero header may be read as empty, for each of: the newest file and an older one, a loose and a chained boundary, SyncPolicy::Always and EveryN, a recycled log (Options::recycle_wal: a recycled file carries its last life's header, not zeroes), and the sealed log under a cipher (its header is a wrapped key; CLAUDE.md has the recycled case there). Under Always an acknowledged frame implies an fdatasync that covered the header too, so valid frames behind a zero header are either unsynced (dropping them is allowed) or media damage (which should be refused if it can be told apart): decide, test both, and name the tests for what they hold.

Also left open by the log unit, and yours if it is real: nothing holds that a loose log file's chained header is synced by the sync that chains it (that unit's mutant O6 survived); and no test makes the power-cut half of a rotation after a failed write was put back, or runs a recycled log (Options::recycle_wal) through a failed write.`,
  },
  {
    key: 'sync-run',
    defects: ['sync-ordered-buffered', 'run-tail-kill'],
    brief: `Defect sync-ordered-buffered, and the second test of run-tail-kill, which the same fix answers. Under SyncPolicy::EveryN with a direct run open, Db::sync never writes or syncs the run's held tail, skips sync_wal because the table is ordered, and zeroes the unsynced count: a sync() answered Ok is lost to a power cut, and to a process kill. Fix: Db::sync makes an open run's commits durable (the held tail written, the run's file synced) and syncs the log whenever it holds unsynced frames, ordered table or not. The only syscalls added are inside Db::sync, whose contract is durability, and only when something is unsynced; a commit under EveryN is untouched. Also check by reading what the skeptic filed beside it: whether Db::seal (and the policy's own due sync at the Nth commit) under EveryN leaves frames or a run's commits unsynced that its contract says are durable; write the failing test and fix it if it is the same gate, and report it if it is a different contract.

NOT this unit, and their tests are not added (they would fail): run-tail-kill's first test (close_run leaving the held tail to the seal thread while a later log commit is written at once; its fix moves a write onto the writer at each run close, and the shared sequence also restores the prefix, so it is priced first) and run-temp-dir-barrier (a directory fsync on a commit path, priced first). Write both to ${S}/fix/deferred.md with the state of their reproductions.

Also left open by the log unit, and yours: Db::sync under a cipher, with a run open under EveryN, skips the log's sync when the run left frames in the log unsynced (a run's commits go to the log there, Db::log_run). The fix that drops the ordered-table gate should cover it; a test under Encryption::Cipher holds it.`,
  },
  {
    key: 'seal-sync',
    defects: ['seal-after-failed-sync', 'helper-presync-error', 'manifest-pending-rewrite'],
    brief: `What the store does after a segment's sync fails: seal-after-failed-sync, the P1 and P2 halves of helper-presync-error, and the close half of manifest-pending-rewrite. Today a seal whose data fsync fails after its readable landing does not wedge (the wedge is taken only "if !landed"), so the next manifest names the segment durable and covers its sequence, and the log that held its records retires; the directory-fsync failure has the same outcome. Under EveryN the close's presync error never reaches the seal job, so the next flush names and acknowledges the segment (P1); under Options::close_finish_inline the presync is joined after the readable landing (P2); and under Options::manifest_pending, Maint::stop names a seal whose fsync failed durable.

One rule: a segment whose sync failed (its data's, its directory entry's, the presync of its records) is never named durable by any manifest, its sequence is never covered, and the log that holds its records is never retired, by any path: land_front, Maint::publish, publish_unsynced, the pending-manifest path, the direct run's close in both arms, Maint::stop and drop. Shared::seal_wedged exists; make it hold on every one of those paths, and never retry the fsync. Readers keep reading what was published. The writer learns of it at its next flush or sync (say exactly which calls answer Err and with what words, and test the words).

Constraint for P1: no wait is added before a seal's readable landing and no syscall on the success path. The presync's result has to reach whoever names the segment durable, and a failed presync wedges as a failed seal fsync does. If you find that correctness needs the readable landing itself to wait for the presync, do not build that: report it under not_built for pricing. Under a cipher a segment's durability is its envelope (vault::persist, and manifest_image writing the envelope of any segment nobody has): check the same rule there by reading, test it if a seam reaches it, fix it if local. Keep the manifest_pending arm sound (its open half is the last unit's).

Also left open by the two units before, and yours: after a kept failure of a run's fdatasync the run's close refuses to finish the file, so its seal job fails before its readable landing and the seal queue wedges until reopen (the run unit's notes); and a failed close of an ordered run (leave_direct after close_direct fails) leaves the ordered table live, and the write that ended the run then panics at the memtable's asserts (the log unit's commit, left open). Decide what the writer does there, test it and fix it: a write must not panic, reads go on, and the calls that answer the caller say the device's error. The run unit's not_built also names a race (a helper fdatasync that has failed in the kernel and not yet stored its error is read as clean by a commit): if closing it needs an atomic on the helper's every sync, leave it not_built for pricing.`,
  },
  {
    key: 'slot-choice',
    defects: ['owed-slot'],
    brief: `Defect owed-slot (default configuration). The manifest is two slot files written in place; a publish picks its slot by parity. A drain's slot may be written and its fdatasync left owed to the segment work's thread; a publish that follows before that sync overwrites the other slot, which is the last durable one, and drops the owed sync. A power cut that tears that write leaves no durable slot that names the store, and the open refuses with acknowledged commits unreachable. The same holds at open: a slot read from the page cache after a process kill may not be on the device, and the first publish after the open overwrites the one that is. Fix: a publish never overwrites the last slot known durable until another slot is known durable (choose the slot by durability, not parity), "known durable" is recorded when a sync returns and never when it is issued, and the open makes the slot it read durable before anything overwrites the other. No fdatasync is added to a commit, a drain, a flush or a publish: the owed sync stays where it is, on the segment work's thread. At open one fdatasync of the slot read is the cost (free when nothing is dirty). The existing test a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid encodes parity: restate it to the new rule (the open still takes the newest valid slot). Hold the arms: Options::manifest_slots off (the rename shape), flush_waits_manifest, manifest_pending, drain_syncs_wal, manifest_spare. Under a cipher the slots are sealed and written through the vault: same rule, and a test.`,
  },
  {
    key: 'open-order',
    defects: ['slot-damaged-rollback', 'both-slots-unreadable', 'manifest-pending-rewrite'],
    brief: `The open's order: slot-damaged-rollback, both-slots-unreadable (hardening: the skeptic showed no engine path reaches it, only outside damage), and the open half of manifest-pending-rewrite. Today the open sweeps (unlinks) every segment its chosen slot does not name before it has decided whether that slot is the store. With the newest slot damaged it falls back to the older one, deletes the newer segments and answers Ok with flushed keys gone; with no readable slot beside partitions it deletes them all and answers Ok. Under Options::manifest_pending the open rewrites pending names it verified from the page cache as durable with no fsync.

Fix: the open removes nothing until a slot is chosen and checked against the directory, and it refuses, by name and with every file left as found, when what the chosen slot names cannot be the store. The line has to be drawn so that every state a crash of this engine leaves still opens, so draw it by coverage: an unnamed segment may be swept only if its records are covered by what the chosen slot names plus what the log replays (the torn-publish window: the seal's segment on disk and synced, the newest slot torn, the log not yet retired, so the older slot and the replay are the whole store). A fallback past a slot is refused when the older slot names a file that is gone, or when a segment it does not name holds records neither it nor the log covers (the log was retired behind the damaged slot, or a direct run's temp name was). Work the rule out against each window of the "Crash discipline" paragraph, including ordered ingest's temp names, a promotion's links, the manifest spare, the ordered index files, form files (x-*.z, never named by a manifest) and a store from before manifests (a_store_from_before_manifests_opens_from_its_segment_names must still pass). For the pending arm: the open fsyncs the pending segments it adopts, their ordered index files and the directory before it writes the slot that names them durable (recovery only). Each refusal is tested by its words (a NotFound further in must not satisfy the test) and by the directory's names and bytes unchanged. Under a cipher the open's sweep is of envelopes neither slot names: hold the same order there.`,
  },
]

const IMPL_SCHEMA = {
  type: 'object',
  properties: {
    unit: { type: 'string' },
    summary: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    tests_added: { type: 'array', items: { type: 'object', properties: { name: { type: 'string' }, failed_before: { type: 'string' }, passes_after: { type: 'boolean' } }, required: ['name', 'failed_before', 'passes_after'] } },
    mutants: { type: 'array', items: { type: 'object', properties: { mutant: { type: 'string' }, killed_by: { type: 'string' } }, required: ['mutant', 'killed_by'] } },
    survivors: { type: 'array', items: { type: 'string' } },
    restated_tests: { type: 'array', items: { type: 'string' } },
    not_built: { type: 'array', items: { type: 'string' } },
    success_path_cost: { type: 'string' },
    cipher: { type: 'string' },
    contract_changes: { type: 'array', items: { type: 'string' } },
    not_verified: { type: 'array', items: { type: 'string' } },
    engine_changed: { type: 'boolean' },
  },
  required: ['unit', 'summary', 'files_changed', 'tests_added', 'mutants', 'survivors', 'restated_tests', 'not_built', 'success_path_cost', 'cipher', 'contract_changes', 'not_verified', 'engine_changed'],
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

const LENSES = [
  {
    key: 'crash',
    text: `Your lens is correctness under failure. Does the fix close the defect on every path that reaches it (each sync policy, hashed and ordered tables, the direct run's arms, background and inline segment work, the cipher arm), without opening another window? Walk the states by hand: at each point where a process kill, a power cut or a device error can land, what does the next call or the next open answer, and is that a state the store acknowledged? Does every legit crash window of CLAUDE.md's "Crash discipline" paragraph still open? Did the tests fail before the fix for the reason they name (read before.log), and would any of them still pass with the fix deleted or halved (name the mutant)? Was an existing test restated in a way that loosens it? Is anything claimed that was not checked?`,
  },
  {
    key: 'cost',
    text: `Your lens is cost and contract. List, by function, every instruction, load, store, branch, syscall, wait, lock or allocation the diff adds on a path that runs when nothing fails; anything beyond a load and a branch folded into a check the path already makes is blocking (rule 1). Are the seams free when unset? For each Options arm the changed code runs under (bench/src/engines.rs lists the arms), is the fix sound and the arm intact? Was anything disabled, refused or limited (rule 2)? What does a caller observe differently now (an error returned, an open refused), and is each change stated factually in the crate docs and tested by its words? Is the CLAUDE.md entry accurate and in that file's manner, with no standing figure? Are the mutants enough: propose up to five more (a flipped operator, a deleted line, a swapped order, a check moved) most likely to survive.`,
  },
]

function implementPrompt(u) {
  return `${CONTEXT}
YOUR UNIT: "${u.key}" (defects: ${u.defects.join(', ')}). You implement it: tests in and shown failing, then the fix, then the mutants. You do not commit.

${u.brief}
${RULES}
Steps:
1. Read git log ${BASE}..HEAD, ${S}/report.md for your defects (the table row, the mutants, the skeptic's section), and each defect's test.md and notes.md. Read the code you will change, and every caller of it.
2. Put your unit's tests into tests/durable.rs (create it if you are the first unit, with a header comment saying what the file holds; otherwise append a section) with the seams they need, and run them before any fix: save the run to ${S}/fix/${u.key}/before.log and record the assertion each fails at.
3. Fix it. Smallest change that closes the defect on every path. Read the diff afterwards for anything on a success path (rule 1).
4. Run your tests, then the neighbours (cargo test --profile checked --test durable; --test db; --test cipher; --lib; --test fuzz; --test segwriter where you touched the writer), then once the whole thing with --no-fail-fast in the background. Save to ${S}/fix/${u.key}/after.log.
5. Mutants (rule 6), into ${S}/fix/${u.key}/mutants.md. Put the tree back exactly.
6. Docs (rule 9). Run "sh scripts/fmt.sh" in the way scripts/check.sh's lint group checks it and fix what it reports (read the script first: one mode checks, one applies).
7. Write ${S}/fix/${u.key}/notes.md: what was wrong, what you changed and why that shape, what each test holds, what you left out and why, what you did not verify.
Leave the work uncommitted in the working tree. Return the structured result; it is data for the next step, not a message to a person. Set engine_changed to true.`
}

function reviewPrompt(u, lens, round, impl) {
  return `${CONTEXT}
YOU ARE A REVIEWER of unit "${u.key}" (defects: ${u.defects.join(', ')}), review round ${round + 1}. The unit's work is uncommitted in the working tree: read it with "git status" and "git diff" (and tests/durable.rs), with ${S}/fix/${u.key}/ (before.log, after.log, mutants.md, notes.md) beside it. You are read-only: edit no tracked file, start no build of changed sources, run no mutant yourself (describe it under extra_mutants and the repair step runs it). You may run an already built test by exact name under a TMPDIR of your own (${S}/tmp/review-${u.key}-${lens.key}) and must remove that directory after.

What the unit was asked to do:
${u.brief}
${RULES}
${lens.text}

Default to "repair" when you find a real problem and to "pass" when you do not: a blocking item is a wrong or incomplete fix, a window still open or newly opened, a legit crash window that now refuses, a test that does not test what it names, a cost on a success path, an arm broken or removed, something disabled or limited, a cipher arm left unfixed, or a claim in the docs that is false. Style and wording are nonblocking. Give file and line for each item, and say what you did not check.

The implementer's own account (data, to be checked, not believed):
${JSON.stringify(impl, null, 1).slice(0, 14000)}`
}

function repairPrompt(u, round, blocking, extra, source) {
  return `${CONTEXT}
YOUR UNIT: "${u.key}" (defects: ${u.defects.join(', ')}), repair round ${round + 1}. The unit's work is uncommitted in the working tree, with its notes under ${S}/fix/${u.key}/. ${source} found the problems below. Deal with each: fix it, or show with a test or a line of code why it is not a problem (and say so). Run every extra mutant; a survivor means a missing test or an unreachable check. Keep everything the unit already fixed. Then run the unit's tests and the neighbours again, the format step, and update notes.md and mutants.md. You do not commit.

What the unit was asked to do:
${u.brief}
${RULES}
BLOCKING:
${JSON.stringify(blocking, null, 1)}

EXTRA MUTANTS TO RUN:
${JSON.stringify(extra, null, 1)}

Return the structured result for the unit as it now stands (all of it, not only this round's changes). Set engine_changed to true only if this round changed anything under src/ (a new test, a note or a doc alone is false).`
}

function commitPrompt(u, impl, unresolved) {
  return `${CONTEXT}
YOU COMMIT unit "${u.key}" (defects: ${u.defects.join(', ')}). Its work is uncommitted in the working tree; its notes are under ${S}/fix/${u.key}/.
${RULES}
Steps:
1. git status --porcelain and git diff --stat: the change is the engine's sources, tests/durable.rs (and tests/cipher.rs or tests/db.rs where a test was added or restated), CLAUDE.md, ROADMAP.md and docs. Nothing else belongs: remove stray files that are not the unit's (scratch output, logs), and say what you removed.
2. Run the unit gate from a clean TMPDIR of your own, in the background with the exit status captured: "sh scripts/check.sh build test lint". Read each group's verdict from its exit status and the "test result:" lines. A test binary that ends with code 101 and no "test result:" line is a watchdog's exit (CLAUDE.md).
3. If the gate is red for something mechanical (formatting, a clippy lint, a doc link), fix it, list it under fixed_in_passing, and run the failed group again. If it is red for anything else, do not commit: return committed false with the failing test's name and output.
4. Commit everything that belongs: one commit. The subject is one sentence naming the behaviour now held, in the manner of "git log --oneline -15" (no prefix tag). The body says what a caller saw, the mechanism in a few sentences, what the fix does, what the tests hold and how many mutants they were held to, and what is left open. No standing figures, no model or tool names. Write the message to a file and use git commit -F. The message ends, after a blank line, with the attribution lines your own harness gives you for commits (a Co-Authored-By line and a Claude-Session line); if it gives you none, copy the two trailer lines of the previous commit (git log -1 --format=%B).
5. Do not push. Confirm git status --porcelain is empty and return the new sha.

The unit's account:
${JSON.stringify(impl, null, 1).slice(0, 9000)}

Reviewers' items still open after the last repair (put any that are true into the commit body's "left open"):
${JSON.stringify(unresolved, null, 1).slice(0, 4000)}`
}

function resumePrompt(u) {
  return `${CONTEXT}
YOUR UNIT: "${u.key}" (defects: ${u.defects.join(', ')}). It is implemented, it has had its first review, and its first repair round was nearly done when the container was restarted: the work is uncommitted in the working tree (git status, git diff), with notes under ${S}/fix/${u.key}/. You finish that repair round. You do not redo it, and you do not commit.

What the unit was asked to do:
${u.brief}
${RULES}
Where the restart left it, as the orchestrator found it (check each statement yourself before trusting it):
- ${S}/fix/${u.key}/notes.md ends in "Repair round 1 (in progress)", steps 1 to 5; mutants.md has the round's table. ${S}/fix/${u.key}/review-r1.json holds the implementer's first result and the two first-round reviews: three blocking items (B1 the CLAUDE.md sentence on a flush, B2 a zero older file before a chained successor, B3 the ROADMAP recycled-header item) and nine extra mutants.
- keep2/ holds the repaired tree (src/db.rs as db.rs, tests/durable.rs as durable.rs, CLAUDE.md, ROADMAP.md) with sha256 sums in keep2/sums. The restart fell while the runner mut0r1.py had its mutant S4 written into src/db.rs; the orchestrator copied keep2/db.rs back over src/db.rs, and all four files then matched keep2/sums. worktree-at-restart.patch is git diff with the mutant in, worktree-clean.patch without it.
- The extra mutants and the round's own (X1..X9, R1..R3) are run: mut2-results-first.json, and mut2-results.json for X7, X9 and R2 run again after the three-file test was made to hold its seals. X9 survives and mutants.md argues it equivalent.
- Step 5 was the round-0 mutants run again on the repaired tree (mut0r1.py, logs m0r1-<id>.log): S1, S2 and S3 were killed (mut0r1-run.log); S4 was in flight and has no verdict; S5, W1..W5 and O1..O11 were not run. The runner wrote its results file only at its end, so there is none.
- Not run since the repair: the whole test suite, clippy, the format check.
Steps:
1. Verify the tree against keep2/sums (sha256sum) and read the diff of src/db.rs since HEAD once, for anything that reads like a mutant.
2. For each blocking item and each extra mutant in review-r1.json, find what the round did about it (the diff, notes.md, the mutant results). Anything not dealt with, deal with now: fix it, or show with a test or a line of code why it is not a problem.
3. Run the round-0 mutants that have no verdict on the repaired tree (S4, S5, W1..W5, O1..O11), each verdict written to a results file as it comes, the fixed file put back and its sum checked after each. A survivor means a missing test or an unreachable check.
4. Run the whole test suite once, in the background with its exit status captured (cargo test --profile checked --no-fail-fast, TMPDIR your own), and the lint group's two commands (read scripts/check.sh). Everything green, or fixed.
5. Bring notes.md and mutants.md up to date: what the round changed, each mutant and what killed it, the survivors and why, what is left out and why.
Leave the work uncommitted in the working tree. Return the structured result for the unit as it now stands (all of it); set engine_changed to true.`
}

const results = []
let stopped = null

for (const u of UNITS) {
  log(`unit ${u.key}: implementing (${u.defects.join(', ')})`)
  let impl = await agent(u.resumed ? resumePrompt(u) : implementPrompt(u), { label: `${u.resumed ? 'resume' : 'fix'}:${u.key}`, phase: 'Fix', model: 'opus', schema: IMPL_SCHEMA })
  if (!impl) { stopped = `${u.key}: the implementer returned nothing`; break }

  let unresolved = []
  const MAX_REVIEWS = 3
  for (let round = u.roundsDone || 0; round < MAX_REVIEWS; round++) {
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

  let c = await agent(commitPrompt(u, impl, unresolved), { label: `commit:${u.key}`, phase: 'Commit', model: 'opus', schema: COMMIT_SCHEMA })
  if (c && !c.committed) {
    log(`unit ${u.key}: the unit gate was red; one repair and one more try`)
    const fixed = await agent(repairPrompt(u, 9, (c.problems || []).map(p => ({ title: 'the unit gate is red', detail: p, where: 'sh scripts/check.sh build test lint' })), [], 'The commit step, whose gate (sh scripts/check.sh build test lint) was red,'), { label: `repair:${u.key}:gate`, phase: 'Commit', model: 'opus', schema: IMPL_SCHEMA })
    if (fixed) impl = fixed
    c = await agent(commitPrompt(u, impl, unresolved), { label: `commit:${u.key}:2`, phase: 'Commit', model: 'opus', schema: COMMIT_SCHEMA })
  }
  results.push({ unit: u.key, defects: u.defects, impl, unresolved, commit: c })
  if (!c || !c.committed) { stopped = `${u.key}: not committed`; break }
  log(`unit ${u.key}: committed ${c.sha}`)
}

let gate = null
let count = null
if (!stopped) {
  gate = await agent(`${CONTEXT}
YOU RUN THE FULL GATE on HEAD, after every unit of this run has committed (git log ${BASE}..HEAD).
${RULES}
Run "sh scripts/check.sh" (every default group: build test lint wasm bench) in the background from a clean TMPDIR of your own, exit status captured, and wait for it; it takes a while, and the bench group builds the suite. Read each group's verdict from its exit status, never through a pipe. If a group is red for something a unit's commit broke mechanically (a struct literal in bench/, formatting, a lint, a committed figure the figures check redraws), fix it and commit it as its own small follow-up commit in the repository's manner, ending with the same two attribution lines the unit commits end with (read one with git log -1), then run that group again. If it is red for anything else, do not paper over it: report the failing test and its output. Never skip, ignore or loosen a test. Do not push. Finish with git status --porcelain empty and return HEAD.`,
    { label: 'gate:full', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })

  count = await agent(`${CONTEXT}
YOU COUNT what the fixes cost a store when nothing fails: instructions by function, ${BASE} against HEAD, under callgrind. This is a count, not a timing run; still, run one thing at a time.
${RULES}
CLAUDE.md's Profiling section and its paragraph on the plain store's price say how: the probes are bench/examples (load_only.rs for an ordered load through the direct run, work_probe.rs with "whole" for a writer's round through the log and the hash table, read_only.rs with "n-none" as the control that must not move, merge_walk.rs for a merge), built in bench/ under the release profile, each run under "valgrind --tool=callgrind" with the toggle on the engine function named exactly, and read by function with callgrind_annotate, the functions' own instructions side by side, never the total alone. Read each example's source for its arguments before running it. Also count a buffered writer (an arm with SyncPolicy::EveryN, such as supdb-ingest) for the ordered load, since two fixes sit on that path.

Both sides are built from HEAD's example sources. There is one target directory and little disk, so do it in the tree: build HEAD's examples and run the counts; then put the base's engine in place with "git checkout ${BASE} -- src" (the examples and bench/ stay HEAD's; if they do not build against the base's engine, say which seam they miss and count only what builds), build, run the same counts, and put HEAD back with "git checkout HEAD -- src" and "git status --porcelain" empty. Keep the callgrind outputs and the annotations under ${S}/fix/count/, not the binaries.

Report per probe and per function that moved: the base's count, HEAD's, and the difference per operation (per commit, per key, per read), for at least Db::commit, Db::commit_direct, the log's write and frame functions, the sync paths, the segment writer's append and sync, Db::delete, the manifest publish and the open. Say which fix each difference belongs to (read the diff), and whether it is a load and a branch on an existing check or more than that. Do not fix anything; do not commit. Return HEAD and whether the tree is clean.`,
    { label: 'count:callgrind', phase: 'Count', model: 'opus', schema: COUNT_SCHEMA })
}

const critic = await agent(`${CONTEXT}
YOU ARE THE COMPLETENESS CRITIC for this run. Read-only on the repository (no edits, no builds); you write one file, ${S}/fixes.md (from the shell if the Write tool refuses).
${stopped ? 'The run STOPPED EARLY: ' + stopped + '. The working tree may hold that unit uncommitted work; say exactly what state it is in (git status, git diff --stat) and do not touch it.' : 'Every unit committed.'}
This run continues two earlier ones that container restarts cut. Units delete-max (commit f8d130a8), log-errors (commit 0aa41df7) and run-errors (commit 38378d1d) were committed by them and are not in the results below: read their commits and their notes under ${S}/fix/delete-max, ${S}/fix/log-errors and ${S}/fix/run-errors, and count them.

Read git log ${BASE}..HEAD with each commit's diff, tests/durable.rs, ${S}/report.md, ${S}/fix/*/notes.md and mutants.md, and ${S}/fix/deferred.md. Then answer, defect by defect for all fifteen in report.md's table:
- fixed: which commit, which tests hold it (by name), and in one sentence what they hold. Check that each reproduction in ${S}/tests.patch has a descendant in tests/durable.rs or a stated reason it has none.
- deferred or not built: why, and exactly what it needs (a pricing run and which pair, the shared log and run sequence, a device that can fail, the owner's word).
- concerns: a fix that looks incomplete on some path (a sync policy, an Options arm, the cipher arm), a restated test that got weaker, a mutant that survived, a refusal with no test of its words, a doc sentence that is now false, a success-path cost. Give file and line.
- behaviour_changes: every way a caller now sees something different (a call that answers Err where it answered Ok, an open that refuses where it opened), one line each, in plain words an owner can say yes or no to.
- owner_items: what is the owner's to decide and what each choice costs.
Write it all to ${S}/fixes.md, short lines, most important first. Return the same as structured data.

The units' results (data):
${JSON.stringify(results.map(r => ({ unit: r.unit, defects: r.defects, commit: r.commit, summary: r.impl && r.impl.summary, not_built: r.impl && r.impl.not_built, survivors: r.impl && r.impl.survivors, restated: r.impl && r.impl.restated_tests, contract_changes: r.impl && r.impl.contract_changes, cost: r.impl && r.impl.success_path_cost, cipher: r.impl && r.impl.cipher, not_verified: r.impl && r.impl.not_verified, unresolved: r.unresolved })), null, 1).slice(0, 60000)}

The full gate: ${JSON.stringify(gate).slice(0, 4000)}
The instruction count: ${JSON.stringify(count).slice(0, 8000)}`,
  { label: 'critic', phase: 'Critic', model: 'opus', schema: CRITIC_SCHEMA })

return { stopped, results, gate, count, critic }
