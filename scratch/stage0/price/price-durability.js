export const meta = {
  name: 'stage0-price-durability',
  description: 'Build, price and land the plain store durability fixes that stage 0 held back because each adds a write, a syscall or a wait to a path that runs when nothing fails: one unit at a time with the machine to itself, failing test first, the fix priced in one process against the shape before it, mutants, two read-only reviews, the unit gate and a local commit; then the tidy units, the full gate, an instruction count and a critic. A fix whose cost a pairing resolves is not landed: it is saved as a patch with its numbers. Nothing is pushed.',
  phases: [
    { title: 'Fix', detail: 'one unit at a time: tests shown failing, the fix, its pairings, mutants', model: 'opus' },
    { title: 'Review', detail: 'two read-only skeptics per unit; repair until clean', model: 'opus' },
    { title: 'Commit', detail: 'unit gate (build test lint) and a local commit', model: 'opus' },
    { title: 'Gate', detail: 'sh scripts/check.sh, every default group', model: 'opus' },
    { title: 'Count', detail: 'callgrind by function, the run\'s base against HEAD', model: 'opus' },
    { title: 'Critic', detail: 'what landed, what did not and its numbers, what the owner decides', model: 'opus' },
  ],
}

const S = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/stage0'
const P = S + '/price'
const BASE0 = '3df85d22'
const BASE = (args && args.base) || 'HEAD'
const ONLY = (args && args.only) || null

const CONTEXT = `
You are working in /home/user/supdb (Rust; the engine is src/db.rs and src/db/*.rs; the benchmark suite is its own cargo workspace under bench/), on the branch that is checked out. Stage 0 fixed the plain (unencrypted) store's durability defects in eight units, committed on top of ${BASE0} (git log ${BASE0}..${BASE}); their notes are under ${S}/fix/<unit>/ (notes.md, mutants.md), the report they worked from is ${S}/report.md, and ${S}/fixes.md is the critic's account of what they left, checked against the repository by a second reader (its section 3 says which unit of this run each open item was given to, section 4 lists concerns by number, section 5 what a caller now sees); ${S}/fix/count/reading.md is the instruction count of those eight commits against ${BASE0}, by function. This run takes what stage 0 held back: fixes that add a write, a syscall or a wait to a path that runs when nothing fails, which the owner's rule says are priced before they land. ${S}/fix/deferred.md has each reproduction's state and the smallest form of each fix; ${S}/fix/followups.md is the orchestrator's numbered list (the briefs cite its items by number). The run's base is ${BASE}: units of this run before yours are committed on top of it (git log ${BASE}..HEAD), with notes under ${P}/<unit>/; read them first.

The container this runs in is restarted every few hours without warning. Keep ${P}/<unit>/notes.md current as you work: the step you are on, where a runner writes its results, which copy of each file is the fixed one. Before the first mutant, and before a measuring arm goes in, keep the files you changed under ${P}/<unit>/keep/, each named by its path with the slashes as underscores and ".fixed" after it (src_db.rs.fixed, tests_durable.rs.fixed), with their sha256 sums in sha256.txt beside them, and keep that copy current whenever the tree is in a state worth keeping (never with a mutant or a measuring arm in it); a mutant runner writes each verdict to its results file as it gets it and puts the fixed file back, checked by its sum, after each.
`

const RULES = `
RULES (the owner's; they are not yours to reopen)
1. Performance is this product's niche. The brief names the one thing your unit may add to a success path (a write, a syscall, a wait, a check), and rule 13 prices it. Nothing else is added to the success path of a commit, a sync, a flush, a drain, a seal, a rotation, a manifest publish or a read: no other syscall, wait, allocation or lock; at most a load and a branch folded into a check the path already makes. A cost that only fires where something is owed (a cold path behind such a check) is the shape to prefer.
2. Nothing is disabled, limited or refused to make a fix easy. An arm in Options that keeps an older shape alive (bench/src/engines.rs lists the arms) stays, and stays sound. Never remove an arm or make the store refuse one.
3. Encryption has to work with everything. The store has three modes (Encryption::None, Kernel, Cipher). Where the code you change has a cipher arm, it gets the same fix and a test under Encryption::Cipher (tests/cipher.rs has the test cipher and its helpers); where the defect cannot exist under a cipher (a run's file is a memory file there and its commits are frames of the log), a test says so by holding the behaviour.
4. Failing test first. Before the fix, put the tests and their seams in, run them, and record the assertion each fails at (${P}/<unit>/before.log). After the fix they pass (after.log). A test that cannot be made to fail before the fix is not evidence; say so and do not count it. The reproductions exist: deferred.md says where each is and how it failed.
5. Tests live in tests/durable.rs (append a section). Hand read callbacks through tests/common's kv and vf, never a closure straight to a generic read call. A refusal is tested by its words and by the directory left as it was found. A test that needs a state holds it (hold_seal_landing, hold_seal_durable, hold_merge_landing); it never bets on timing, and a copy of a live store is taken under a hold of every landing that could fall inside it. Seams follow Db::fail_next_presync: #[doc(hidden)] pub, no cfg, free when unset.
6. Mutants. Run the mutants report.md and deferred.md list for your defects and at least five of your own on the lines you changed (a deleted line, a flipped operator, a swapped order, a check moved, the cold path never taken). A survivor means a missing test or a check nothing can reach: add the test, or take the check out. Record every mutant and the test that killed it in ${P}/<unit>/mutants.md.
7. An existing test that encodes the behaviour being fixed is restated to the new contract: never deleted, ignored or loosened. Any other existing test that goes red is a bug in your fix.
8. Every legit crash window in CLAUDE.md's paragraph "Crash discipline is an order, and every window in it is survivable" still opens as that paragraph says. tests/db.rs and tests/durable.rs build those windows on disk; they are the guard.
9. Docs. Stage 0 wrote these holes into the docs as named exceptions (the doc comments on SyncPolicy, EveryN, Db::sync, Db::commit; CLAUDE.md's "Crash discipline" paragraph and its "Shapes the bugs come in" entries; ROADMAP.md items). A fix that lands takes its exception out of each, says what is true now, and ticks or removes its ROADMAP item; CLAUDE.md gets an entry or an amendment in that file's own manner (it names the failure that produced the rule and ends in the rule; no standing figures). Crate docs are factual, with no history. A figure belongs to the commit message of the change that made it, rounded, with the pairing command that reproduces it.
10. Mechanics. One cargo at a time. Tests run as "cargo test --profile checked" with TMPDIR exported to a directory of its own under ${S}/tmp/<unit>, removed when you finish; never glob /tmp. A command that may run past a few minutes goes in the background with its output and exit status written to a log, and you wait on the log; read a verdict from the exit status or the "test result:" line, never through a pipe that swallows the status. About nine gigabytes of disk are free and the bench binary's links are large: make no new target directory and no worktree, and remove the stores a killed pairing leaves ($TMPDIR/supdb-ab-<pid>) by name.
11. Git. Do not push. Do not stash, reset or check out over the working tree. Only the commit step commits. Edit nothing under scratch/.
12. Say what you did not verify. Do not write a claim you did not check.
13. Pricing. A timing run needs the machine to itself: before each, "pgrep -af 'cargo|rustc|valgrind|/bench '" shows nothing but your own, and nothing else of yours runs beside it.
  a. The shape before your fix is kept, in the measuring tree only, behind a #[doc(hidden)] pub field of Options named price_<unit>_old (default false, the fix), with two bench arms in bench/src/engines.rs that are the twins of "supdb" and "supdb-ingest" with only that field set (name them supdb-<unit>old and supdb-ingest<unit>old; where the brief names another arm, a twin of that too). Check by reading that each twin differs from its arm in that field and nothing else.
  b. Build the suite: (cd bench && cargo build --release), log and exit status kept.
  c. Pair in one process, the way the docs cite a pairing: "OUT=${P}/<unit>/pairs sh bench/scripts/pair.sh NEW OLD SIZE [bench ab options]" (read the script and bench/DESIGN.md's section on "bench ab" for the options and what the table prints). Sizes 10000 and 300000; the durable pair and the buffered pair; the passes the brief names. Read each table per sitting; read stderr lines by the arm each names, never by their parity.
  d. Then the mechanism, with the suite's own stamps set for both arms through pair.sh's VAR=VALUE prefix (SUPDB_LOAD_PHASES=1 and SUPDB_DRAIN_STEPS=1; CLAUDE.md's Profiling section says what each prints): the phase that holds your change, arm against arm. Rates from a stamped pairing are not the verdict. Beside it a count: "strace -f -c" (and -tt -T -y where a wait matters) over bench/examples/load_only.rs for each arm, for which syscalls your fix adds and where.
  e. Verdict. A cost is resolved when a quantity reads on the worse side in both sittings of a pairing with the table's own error bars clear of level, or when the phase that holds the change is longer in both sittings by more than the sittings differ from each other. One sitting only: run that pairing once more; still split is "unresolved". Level everywhere, or unresolved, lands (the numbers go in the commit message). Never call a cell level from one sitting.
  f. A resolved cost: build the brief's alternative shape and price it the same way. If every shape the brief allows costs, the unit does not land: save "git diff" as ${P}/<unit>/not-landed.patch, put the tree back with "git apply -R" of that patch, confirm "git status --porcelain" is empty, and return land false with the numbers. That is a result, not a failure.
  g. Before the unit goes to review: save the measuring tree's diff as ${P}/<unit>/measuring.patch, then take the price_<unit>_old field and its bench arms out. The old shape is the defect and a wrong answer is not an arm. The pair outputs stay under ${P}/<unit>/pairs with their head.txt.
`

const UNITS = [
  {
    key: 'other-slot',
    kind: 'fix',
    defects: ['followup 48: the open leaves the manifest slot it does not take to the kernel\'s writeback'],
    brief: `A defect found after stage 0, by a model of the two slots (${S}/../seam/work/owed/slots/notes.md, "H2", and check.md beside it, which ran it again: at length 4 with no failed sync the engine's rule loses 3,256 sequences and syncing both slots loses none). It is in the landed slot-choice fix and in open-order's open alike, for a plain store and under a cipher: Db::open fdatasyncs the slot it takes ("let f = vault::open_slot(dir.join(MANIFEST_SLOTS[m.slot]))" ... "f.sync_data()") and does nothing about the other slot file.

The trace. A publish writes slot B with its fdatasync owed (a drain's slot, answered before its sync). The next publish, into B again by the SlotChoice rule (A holds the newest manifest known durable), is killed part way through its write: a buffered write of more than a page stops at a page boundary on a fatal signal, so the page cache holds B torn. The reopen reads B as no manifest, takes A, fdatasyncs A, sweeps what A does not name, replays the log and goes on, acknowledging commits. Until the kernel writes B's torn image back (its own schedule, tens of seconds) or the store's next publish rewrites B, the device may hold the first publish's record in B whole, since its writeback could have run before the torn write dirtied the page again. A power cut in that window, and the next open reads B from the device as the newest valid slot: a state the open before passed over, naming segments that open swept (the open refuses, "is missing from the directory", at every open from then on, with the store's acknowledged commits in a log nothing reads), or, where those unlinks were not durable either, a store read from one manifest with a log written beside another.

Verify it on the engine first: it is a model's finding and nothing has built it. The slot-choice tests have what the state needs (Db::watch_slot_writes records every slot write and which a sync covered; tear_next_slot_write tears one; the power-cut states are built from which writes a sync covered). Build: the owed publish, the torn publish over it, a kill (the store dropped without its close: read how the slot tests make a kill), the reopen, commits acknowledged on the reopened store, then the power cut with B holding the first publish's record whole and A as the open synced it; require the next open to answer Ok with every acknowledged commit. If the engine as it stands passes, find out why before changing anything (does the reopen write B? does anything sync it?) and say so; a model's hazard the engine does not have is a result.

The fix to build if it fails: the open makes the slot it does not take durable as the page cache holds it, before it removes or writes anything: an fdatasync of the other slot file where it exists, of nothing where it is clean, beside the one the open makes already (in Db::open, the block that ends "slot.landed(m.slot); synced[m.slot] = m.seq;", ahead of Db::open_refusals). The other slot is opened without being made: vault::open_slot creates a file that is not there, which is a write before the checks, so a slot file that is absent is passed over (NotFound and nothing else). It is synced whatever it read as: torn, valid and older, or valid and newer and skipped for pending segments that did not read whole; in each the device is made to hold what the open decided from. Its failure is the open's answer, with every file as found (the open-order tests' terms). A seam for the test on the terms of rule 5, in the manner of Options::fail_open_slot_sync, which names the inode it failed for. The order is held the way open-order holds its pending fsyncs, by the call and not by the code's own word: the vault records a file fsync once it returns (vault::sync_file, read through Db::trace_dir_io; a test filters the record by its own directory, since it is process-wide), and the test requires both slots' fsyncs before the first slot opened for writing and before any name is gone from the directory. Route the taken slot's fsync through the same recorded call if that costs the open no syscall it does not make today (it is an fdatasync today and vault::sync_file is an fsync: keep the fdatasync, with a recorded call of its own if need be), which also gives the slot-choice unit's root-only page-flag check (followups.md item 28) a check that runs without root. Under the rename shape there is one manifest and nothing to add: say so and hold it with the existing arm test. Check the upgrade paths (a store from the rename shape, version 1 and 2 slots, manifest_make_slots creating a missing slot file) for an open of a slot file that is not there.

What this adds when nothing fails: at the open only, one open(2), one fdatasync of a file with nothing dirty and one close(2) (or none of the first and last if manifest_read_all's handles are kept, which unit open-cost folds); nothing on any other path. No pairing prices it (verdict "not-priced"); open-cost, later in this run, times the open with it.

Mutants at least: the second sync deleted; made after the sweep; made after open_refusals but before the sweep (say whether any test can tell, and take the stricter place if none can); made only where the other slot read as valid; made of the taken slot twice (the seam's inode tells them apart); its error swallowed; skipped under a cipher; the other slot opened with vault::open_slot (a store whose other slot file is absent must be left with no such file by an open that refuses). Docs (rule 9): the crate docs' and CLAUDE.md's Crash discipline sentences that say the open "fdatasyncs the slot it takes" name both slots; and CLAUDE.md's Shapes entry "A slot chosen by its sequence, beside a sync left owed" gains the corollary in that file's manner: of two copies kept so one stands while the other is written, an open that picks one settles what the device holds of the other before it acts on the choice, since a copy read as torn from the cache may be whole on the device.`,
  },
  {
    key: 'rename-arm',
    kind: 'fix',
    defects: ['fixes.md concern 7: no crash state is built under the rename shape of the manifest (Options::manifest_slots off)', 'followups 18, 32, 36, 55, 58'],
    brief: `The manifest has two shapes: two slot files written in place (the default) and the shape before them, kept as an arm (Options::manifest_slots off; read bench/src/engines.rs for the arms that set it): the manifest written under a temp name, renamed over the old one, the directory fsynced. Stage 0's seal-sync, slot-choice and open-order were built and tested under the slots. No test of theirs builds a kill or a power cut under the rename shape (${S}/fixes.md, concern 7), and five follow-ups read the arm, by reading alone, as unsound or unheld: 18 (under the arm land_durable's failure counts the seal out with no stop of the seal queue, so the next landing's publish fsyncs the directory again and names the segments: a retry with a manifest behind it), 32 (after a kill between a publish's rename and its directory fsync the open fdatasyncs the manifest file and not the directory, then sweeps; it rests on the journal ordering the rename before the unlinks), 36 (a store moved between the shapes is tested by its reads only), 55 (a store written under Options::manifest_pending and reopened in the rename shape takes pending names as live and fsyncs nothing), 58 (a refusal under the arm names a slot file). Rule 2: an arm stays sound.

Build the crash states under manifest_slots off, each as a test that says what must hold, and run them all before changing anything (before.log says which fail and at what): (a) the rename shape's form of every_arm_that_writes_a_slot_keeps_the_last_durable_one: after a kill or a power cut at each step of a publish (the temp written, renamed, the directory fsynced or not) the open finds a manifest whose every name is in the directory, with the log holding the writes since; (b) a publish whose directory fsync fails names nothing durable afterwards and retires no log, at that publish and at the next landing (followup 18; the seams that fail a slot's sync and the vault's sync_dir are there: read how the slot tests use them); (c) a kill between the rename and the directory fsync, an open, commits acknowledged, then a power cut that takes back what no fsync covered: the store opens with every acknowledged write (followup 32); (d) a power cut after a store written under manifest_pending is reopened in the rename shape (followup 55); (e) a crash across a change of shape, in both directions (followup 36); (f) open-order's refusals under the arm, by their words, naming the file that was read (followup 58). A state a crash cannot leave is not built: say why it cannot.

Whatever fails gets the smallest fix on the arm's own path. Nothing a store under the slots does changes; under the arm, when nothing fails, the open may gain one directory fsync before it removes anything (where (c) needs it) and the open of a store with pending names the fsyncs open-order makes under the slots (where (d) needs them), and nothing else: say exactly what you added and where. What passes before any change is a guard and is reported as one, with the mutant that shows it can fail. Say in the result which of followups 18, 32, 36, 55 and 58 are closed, so that the tidy units after you skip them. Docs (rule 9): the crash-discipline sentences that describe the rename shape say what is true of it now.`,
  },
  {
    key: 'run-close',
    kind: 'price',
    defects: ['run-tail-kill (test 1)', 'followup 4: a sync over a run closed into a seal in flight', 'followup 9: a sync over a closed run whose seal failed'],
    brief: `One path, three holes, all under SyncPolicy::EveryN in the clear: a direct (ordered) run that closes into a seal with commits no sync has covered and a tail its writer holds unwritten. (a) close_run leaves the held tail to the seal thread's finish while a later commit goes to the log at once, so a process kill between them keeps the later commit and loses the run's: not a prefix (run-tail-kill test 1; ${S}/run-tail-kill/, eight cases). (b) Db::sync after such a close answers Ok without covering the run's commits, by a kill and by a power cut (${S}/fix/sync-run/round2/closed-run-test.rs, 24 cases: six ways a run closes, two shapes, arms default and copyvals). (c) If that seal then fails, the queue stops and every sync answers Ok for good over commits that are durable nowhere (followups.md item 9, third case).

The contract to reach: a closed run's commits are covered by the next sync of any kind before that sync covers anything newer. That is Db::sync, the policy's due sync at a commit (the log's), and a run's own due sync if another run has opened since; and over a stopped queue a sync that cannot cover them answers the failure.

The shape to build and price (P1 of report.md plus the kept handle): at every way a run closes (leave_direct from an append the run cannot take, from every put and every delete; close_direct from Db::seal and from the commit's seal at the threshold; hand_tail), the writer's thread writes what the run's writer holds before the hand-off, with no sync; the writer keeps the closed run's file (a clone of the handle it has, no syscall) with the count of commits no sync covered, set in the close's cold path only when that count is not zero; and the syncs above, in a cold path behind one load and branch folded into a gate each already has, fdatasync that file first and clear it. When the seal has landed durable the entry is dropped with nothing to do. What this adds when nothing fails: under EveryN, one write of under a megabyte per run close moved from the seal thread to the writer (and the run's last piece becomes two writes, which changes the folios the page cache makes of the file), and one fdatasync at a sync that follows a close before the seal is durable. Under Always the count is zero and nothing is added: show that with strace.

Price: the ordered and shuffled loads and the read and scan passes after them (--passes load, then the read passes; the folio change is read on the reads), both pairs, both sizes; and the arm whose closing sync is Db::sync and not a flush (read bench/src/engines.rs for which arms those are), since the closing sync is the cell that moves. If the reads move with the bytes the same, read the file's folio composition as CLAUDE.md's Profiling section describes before calling it a cost.

Alternatives if the write at the close costs: (1) Db::sync and the due syncs wait for that seal's durable landing where the entry is set (a wait only where owed, no write moved); (2) the write made only by the first log write that follows the close while the seal has not written the tail. Under a cipher a run's commits are frames of the log and a sync covers them (a_sync_after_a_run_closed_into_a_seal_covers_its_logged_commits_under_a_cipher holds that today): keep it held.

Before any change, one measurement for the owner with nothing to land: what stage 0's sync-run fix costs the closing sync of the arms whose sync is Db::sync (followups.md item 7), by a measuring arm that restores the old Db::sync for an open run; report it under pricing, labelled as landed already.`,
  },
  {
    key: 'due-log',
    kind: 'price',
    defects: ['followup 5: a run\'s due commit beside log frames no sync covered'],
    brief: `Under EveryN in the clear, a run's due commit syncs the run's file and not the log, so a power cut can keep the run's commits and lose older frames of a table frozen before the run: not a prefix (deferred.md, "the due commit of a run beside unsynced log frames"; the test is ${S}/fix/sync-run/due-commit-test.diff and its failing run due-commit.log). By reading, the same under Always after a reopen of a log a buffered life wrote, whose replayed frames count unsynced.

The shape to build and price: a bool noted once per run, in commit_direct's run-open branch (cold), when the log owes a sync (unsynced frames, loose files, the directory barrier), asked inside the due branch beside the run's fdatasync (a load and a branch there), whose cold path syncs the log before the run's file. In the clear nothing writes the log while a run is open, so the bool is exact for the run's life; under a cipher log_run's due sync covers it already: hold that with a test. With unit run-close before this one the pair is closed in both directions: any sync covers everything older first. Say whether the two units' flags are one state (what the log or an earlier run owes) and make them one if they are.

Cost when it fires: one log fdatasync at the run's first due commit after a table was frozen unsynced; never when the log owes nothing. Price: the ordered load (the suite's never has log frames beside its run: show the syscalls are unchanged with strace -c, and the pairing level), and a shape where it fires: the lag sweep and the mixes after a shuffled load (--passes load,lag and the mixes), both pairs, both sizes. If no cell of the suite reaches the cold path, say so, count it with a probe of the suite's shape (bench/examples), and report the count instead of a rate.`,
  },
  {
    key: 'run-dir',
    kind: 'price',
    defects: ['run-temp-dir-barrier'],
    brief: `A run's temp file is created and its commits acknowledged with no fsync of the directory that holds its name: under Always, under EveryN at the due commit, and at Db::sync (${S}/run-temp-dir-barrier/, three tests, all fail at the barrier assertion; deferred.md has their state). A power cut can keep every synced record of the file and lose its name, and recovery reads the run from that name.

Two shapes, both to build behind the measuring field and price against no barrier: eager, vault::sync_dir after the run's file is created and before its first batch is written (one a run, on the commit path; an error fails the commit); and lazy, a flag paid at the run's first due sync, at Db::sync's cover of a run (open, and closed after unit run-close) and at the close's own sync, cleared only on success (report.md's mutant (e) dies only if every one of those pays it). Under Always the first commit is due, so the two differ only in order: issue the barrier where it shares the journal commit the file's fdatasync forces (CLAUDE.md's Profiling section has both the cost of a directory fsync here and the case where two syncs waited on one commit), and stamp it rather than assume it. Under a cipher nothing of a run is in the directory until its envelope: a test holds that no barrier is issued there.

Price: the ordered load at 10000 and 300000, both pairs (the first commit's phase is the mechanism: SUPDB_LOAD_PHASES prints the first commit apart), and a workload of many short runs if the suite has one (a store that alternates ordered stretches and other writes pays one barrier a run): if no cell does, count the barriers per run with strace and say what a run of N keys pays per key. Land the cheaper shape that is level; if only one is level, that one.`,
  },
  {
    key: 'helper-race',
    kind: 'price',
    defects: ['the helper thread\'s failed fdatasync read as clean by a commit in the instant before it is stored'],
    brief: `ROADMAP.md, "A helper sync in flight when the run's commit syncs" (read the whole item), and ${S}/fix/run-errors/notes.md under "Left out, and why". The segment writer's helper thread syncs behind the writer on the writer's file description; a helper fdatasync that failed has consumed the device's error for that description, and a commit whose own fdatasync returns Ok in the instant between the helper's syscall returning and the helper storing its error is acknowledged over the hole.

A model of overlapping syncs has since named the shape (${S}/../seam/work/owed/syncop/notes.md, and check.md beside it, which corrects it: read the check's "steal" ops Y and V): two syncs on one description, the kernel delivers the writeback error to one and the other returns Ok first; with no wait between them the model loses 99,360 sequences and with each sync on its own description, or a wait, none. The trace to transcribe into the test is "1 B B Y": a write, two syncs begun on the one description, the error delivered to the helper's, the writer's returning Ok first; the answer must be the failure.

Shapes, cheapest first: (1) the helper syncs through an open file description of its own (one open where the helper is spawned, nothing a commit), so the kernel reports a writeback error to the writer's description too, whichever of them syncs first; (2) a count the helper raises before its fdatasync and lowers after its store, which the commit's sync waits out before it reads the error (an atomic read-modify-write per helper sync and a load per run commit). Build (1). Its test cannot be a real device error (ROADMAP's dm-flakey item): the seams fail a sync by description, so make the seam say which description failed and hold that the writer's own sync then answers the failure with the helper's store delayed (a hold on the helper between its syscall and its store). If (1) cannot be held by any test here, say exactly what holds it and what does not. And two paths the report read and nothing tests (${S}/report.md, "H2" and "H3" under helper-presync-error; ${S}/fixes.md, concern 11): a helper's failed sync at a seal's finish and at a merge's finish, which join_helper has to catch before the segment is renamed into place. Every test that uses SegmentOptions::fail_helper_syncs today is a run writer's. Build both (a seal's output and a merge's piece with the helper's sync failed): the segment is named by no manifest and the failure is the landing's; a mutant that drops join_helper's error must fail each.

What it adds when nothing fails: one open and one close per segment writer that has a helper (a seal's output, a merge's piece, a run), and the helper's hints and syncs through the second description. Price: the loads and the drain (SUPDB_DRAIN_STEPS) at both sizes, both pairs, and the lag sweep's bursts (a seal a burst). Check with strace that the helper's sync_file_range hints and fdatasyncs are the same calls on the new descriptor, and with the page-cache reading CLAUDE.md describes that the file's folios are unchanged.`,
  },
  {
    key: 'drain-order',
    kind: 'price',
    defects: ['followup 17: the drain\'s durable landing after the writer\'s presync, unpriced', 'followup 19', 'followup 21'],
    brief: `Stage 0's seal-sync unit made a drain's run job (SealJob::awaits_writer) land durable only at the writer's own ask, after the writer's presync of the run's records, so that a failed presync is never named durable. That is correct and it serialised two things that overlapped: the durable landing no longer runs beside the presync, and a merge that finishes in the window waits for the ask (followups.md items 17, 21 and the "seal-sync as committed" section). It has not been priced.

First price what landed: a measuring field that restores the landing whenever the seal thread has returned (the unsound shape, for measurement only), paired on the ordered load at 10000 and 300000, both pairs, with SUPDB_DRAIN_STEPS=1 and SUPDB_LOAD_PHASES=1 for the drain's steps. If no cost is resolved, nothing of the order changes. If one is: build the cost reviewer's alternative, an "ok" the writer stores after its presync returns Ok, which lets the segment work land the job durable at its next look without waiting for the Drain ask's hop (a store beside an fdatasync on the writer; a load where the segment work already asks whether the job has returned), with a failed presync still stopping the queue first, and price that.

Whatever the pricing says, two small things land: the writer-thread presync's test seams become load-first (fail_presync.swap is an unconditional atomic read-modify-write on every presync today, and presync_hold a load before it: one byte, one load, as the run writer's fault byte does), and item 21: a flush whose seal() fails after close_run handed an awaits_writer job sends no Drain ask, so the job sits readable with the manifest window shut until the writer's next land_seal, settle or flush. Build the state in a test, then either make the failure path send the ask or say in the crate docs what a caller sees; a debug assertion that awaits_writer implies a draining writer where the reviewer asked for it (item 12).`,
  },
  {
    key: 'open-cost',
    kind: 'price',
    defects: ['followup 27: the open\'s fdatasync of the slot it takes, untimed', 'followup 38: a stat per named segment at every open', 'followup 39', 'followup 57: every live segment mapped for one key after a crash'],
    brief: `Stage 0 put three things on every open that were not timed: an open(2), an fdatasync and a close(2) of the manifest slot the open takes (slot-choice; nothing to write where the slot is clean, the rename shape included); a stat of every segment the slot names (open-order's open_refusals, through vault::on_device), where a plain store's own directory listing already says which names exist; and the log's replay moved ahead of the sweep. Since then unit other-slot of this run added a second fdatasync, of the slot not taken; and where an unnamed segment newer than every named one stands within what the slot covers (after a crash; never after a clean close) Db::unnamed_refusals maps every live segment to look one key up (followups.md item 57). Read followups.md items 27, 32, 33, 38 to 41 and 57 and the units' notes.

Price an open, which no cell of the suite times: a probe under bench/examples (the suite's shape: its keys and values, a store loaded and settled by the suite's own engine adapter, then opened and dropped a few hundred times, each open timed on its own, the first apart) at 10000, 300000 and 3000000 keys, for a store closed cleanly and for one copied under holds after commits no flush covered (a log to replay). Arm against arm in one process, alternating, with a measuring field that skips the slot's sync and the stats; and "strace -f -c" of one open of each for the calls. Then fold what is free to fold: the handle manifest_read_all already opened is the one synced (no second open and close); a plain store answers "is this name there" from the listing it has, with the stat kept where there is no listing to ask (under a cipher, vault::on_device); and the unnamed segment's last key is looked up in the partition whose fences hold it and the pieces over that range, not in every live segment, where the fences are already at hand (if they are not at that point of the open, say what it would take and leave it). Report an open's time and syscall count before stage 0's two units (the measuring field), as landed, and as folded.

The order the open keeps (the slot durable before anything is removed or written; nothing removed before the slot is checked) does not change: every open-order and slot-choice test stays green and their mutants stay killed (run the two units' mutant lists against the folded code where the lines moved).`,
  },
  {
    key: 'tests-tidy',
    kind: 'tidy',
    defects: ['followups 1, 22, 25, 28, 31, 34, 36, 46, 52, 56 and fixes.md concerns 6, 8, 10, 16b: tests that bet, sleep, hang, or pass without running'],
    brief: `No engine behaviour changes in this unit; seams may be added on the terms of rule 5. From followups.md:
- Item 1. tests/durable.rs copies a live store as its crash at about fifty call sites (copy_store), and one already went red for a seal landing inside the copy. One helper that copies a live store under holds of the seal's and the merge's landing, so that no test can forget; each call site moved to it, or shown in a comment to have nothing in flight. Then the whole durable binary run busy: four processes at a time, forty runs each, each with a TMPDIR of its own, every failure read. Every mutant that stage 0 credited to a test whose copy changed is run again (CLAUDE.md: a mutant's kill is read again once a race under the killing test is found); the units' mutants.md files say which test killed which.
- Items 22 and 25. Sleeps and waits in seal-sync's tests (500 ms in the inline presync test and the "both" variant, a 2 s wait in the writer-arm test, the 60 s watchdog): replace each with a hold where a hold exists, and say which remain and why.
- Item 28: a slot-choice test reads /proc/kpageflags, prints "not run" and passes without root: a verdict it did not earn. Open-order met the same shape and closed it without the privilege: its file fsyncs go through a call that records itself once it returns (vault::sync_file, Db::trace_dir_io), and SUPDB_TEST_UNPRIVILEGED makes a root run behave as the gate's. If unit other-slot routed the slot's fsync through a recorded call, hold the slot-choice line by that record and keep the page-flag read as a second word where it can be had; if it did not, do it here. Show the mutant the page flags alone killed is killed with SUPDB_TEST_UNPRIVILEGED set.
- Item 52: the vault's io trace is process-wide, stays on once a test turns it on, and grows with every recorded call of every test in the binary. Each test that reads it filters by its own directory (check each does) and turns it off or clears it when it has read; say what bounds it.
- Item 56: delete-max's an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes copies a live store while an unheld seal lands (its rare refusal, "names seg-... missing from the directory", is a copy no crash leaves). It is one of item 1's call sites: name it in the list and hold the landing across its copy.
- Item 31: confirm by a mutant that the inline arm of every_arm_that_writes_a_slot_keeps_the_last_durable_one kills a landed-nothing publish with the segment work inline.
- Item 34: the pending landing discards write_slot's owed answer. Build the state (an owed slot at a pending landing) or prove it unreachable and assert it (a debug assertion that the entry is None there).
- Item 36: a crash state across a change of manifest shape (slots to rename and back) in a_store_moved_between_the_manifest_shapes_keeps_its_writes, and a comment on the v2 fixture that is placed by parity.
- From ${S}/fixes.md, concerns 6, 8 and 10, and its 16b. (6) ROADMAP.md's item "A fall back past a damaged slot whose log retired" points at a test that lives only in the session's scratch (${S}/fix/open-order/log-retired-test.rs): put it in tests/durable.rs under the name the ROADMAP item cites, #[ignore]d with the reason in the attribute (it fails today and the item says why), and make the item point at it. (8) tests/db.rs a_manifest_publish_alternates_two_slots_and_the_open_takes_the_newest_valid was restated from "the other slot is exactly the publish before" to "is older", and its name still says alternates: name it for what a publish does now and assert the strongest thing that is true (the slot a publish did not write holds the newest manifest known durable). (10) run-errors' mutant O3 (the stuck writer never cleared) is killed only by the runner's timeout: the test loops for ever in AlignedWriter::write. Give the test an end of its own (the write refused once it has made no progress twice, if that is true of the engine; else a bound in the test's seam) so that the mutant fails it. (16b) the differential fuzz never varies the sync policy, and most of stage 0's defects were EveryN's: add the policy to the fuzz's option draw behind an environment variable in the manner of the ZFUZZ_ variables (a draw added to the shared stream changes every seed's operations: CLAUDE.md says how a failure is then attributed), run the gate's range and three ranges beyond it with it set, and read every failure (one seed outside the gate's range is known to fail on the main line: ROADMAP.md has it); and run-errors' "a leave_direct after a kept run error leaves a frozen table nothing lands" is tested only by a reopen: hold what the live store answers.
Mutants here are mutants of the tests' holds: a hold removed must make the stress run fail, or the hold is not what protects the test.`,
  },
  {
    key: 'engine-tidy',
    kind: 'tidy',
    defects: ['followups 6, 8, 12, 14, 20, 23, 24, 26, 29, 30, 35, 42, 45, 49, 53, 54, 55, 58 and fixes.md concerns 9, 12, 15, 16a: small engine and docs items no pricing is owed for'],
    brief: `Each change here is free when nothing fails (rule 1 with nothing allowed: a load and a branch folded into a check the path already makes, or less), and each engine change has a test that fails first or a debug assertion that a mutant trips. From followups.md:
- Item 8: Db::sync's log gate lost its "|| self.wal_dir_unsynced" clause as unreachable while commit_staged's due gate keeps it: make the two agree, and hold the invariant with a debug assertion where the flag is read (!wal_dir_unsynced || !loose_wals.is_empty() || lost.is_some()).
- Item 14: after the seal queue stops, a finished merge is held for good and the segment work's collect enters on every poll for the rest of the open: latch it out once the queue is stopped.
- Item 24: two mutants are killed only through Maint::publish's debug assertion, and on the segment work's own thread a tripped assertion is swallowed at the close. Decide whether a checked build propagates a segment-work panic at Db's drop or close (it must never panic inside a drop during another panic), build it if it is small, and say what it changes for a caller; otherwise a ROADMAP item with the reason.
- Item 26: a slot write that fails after write_slot gave an owed entry up loses the owed answer, so the log files behind it retire at the next durable landing and not the next publish: kept longer, never early. Hold "never early" with a test, or fix the order.
- Two hardenings of the slot choice from the same model's reading of the engine (${S}/../seam/work/owed/syncop/check.md, "pair 2"; neither changes a syscall): SlotChoice::landed assigns, which is sound only because the slot's sync is started and waited for on one thread and every slot write opens a descriptor of its own: a debug assertion that the sequence landed is never below the one held; and SyncHelper's start and wait have a "?" between them at the pending landing that can leave a finished sync's result in the channel for the next wait, harmless today only because a failed readable landing stops the seal queue: a job id handed out by start and required by wait, so a stale result is dropped and never taken for the next job's. A mutant of each trips its assertion or its test.
- Item 53 (hardening; decide by building the guard first): with both slot files unreadable beside seg- pieces only (a store before its first partitioning), the open still takes the scan kept for stores from before manifests, adopts every seg- file, torn or unsynced, and skips the log behind them. The reviewer's shape is "a slot file exists, none reads, a segment of any name stands: refuse". Before it: build as a guard the crash window it must not close, a store from before manifests whose first open under slots is killed inside its first slot write (a slot file torn, no other slot or an empty one, seg- files and a log), which has to open as it does today. If the files of that window and of two damaged slots cannot be told apart, nothing is built and ROADMAP.md says why; if they can (by what the upgrade writes first, or an order the upgrade could keep at no cost to any other open), build the refusal with its words tested and two mutants.
- Items 55 and 58 only where unit rename-arm of this run did not close them (read its result in its notes).
- Item 55: a store written under Options::manifest_pending and reopened with manifest_slots off takes pending names as live and fsyncs nothing (the open's pending fsyncs are under "opts.manifest_slots &&"), and a later publish in the rename shape names them durable. Build the state (a power cut after such a reopen) as a failing test, then make the open fsync a pending set whatever the slot shape. Open only.
- Item 58: under the rename shape a refusal names MANIFEST_SLOTS[m.slot]; check what file that is there and make the words name the file that was read.
- Item 42: Shared gained a test-only Vec (open_synced). Read ${S}/fix/count/ for a function whose count moved with no change to its source; if one did, keep the record outside Shared.
- Item 30: a manifest publish reads four test seams (watch, tear, fail, synced record) beside its write and its sync. Read ${S}/fix/count/ for Maint::publish by function; fold the four into one byte and one load, as the run writer's fault byte does, and show the count.
- Docs, items 45 and 54: Db::open's doc says the open "removes and writes nothing until the slot it takes is held to the directory", and the check of a pending slot's segments runs before that and may write an ordered index it had to compose again (Db::pending_whole, Seg::open): say "writes no slot", as CLAUDE.md does, or name the exception. CLAUDE.md's crash-discipline sentence "A crash leaves an unnamed segment only from a job whose publish never became durable" leaves out a merge's inputs beside outputs a durable slot names (the publish durable, the unlinks not run), which the check passes over by id: one clause.
- Docs, items 6, 23 and 35: the SyncPolicy paragraph's lines over the comment width; the loose wordings item 23 lists ("every seal" inline, the inline write that ends a run over a stopped queue with room, "has written its batch to the log first" for a direct run); CLAUDE.md's slot entry cut to the failure and the rule where it narrates review rounds about code that never landed, and its sentence about a pending slot under a cipher checked against what the open did.
- Docs for a hazard left open by choice (followups.md item 49; ${S}/../seam/decisions.md decision 18): after a failed fdatasync the kernel leaves the pages clean in its cache, so a reopen while the files are still cached reads what the device may not hold, and a fresh descriptor's fdatasync answers Ok over it. The crate docs' sentence that a failure is answered "until the store is reopened" says what a reopen does and does not establish, in one or two plain sentences with no history. ROADMAP.md's item beside the fault-injecting device (find it: the items that name dm-flakey) names every file an open reads back (the manifest slots, the log, a run's temp file, pending segments under manifest_pending) and the remedy for each with its reason: the slots read past the page cache, since a write to either slot in that state can destroy the only copy the device holds; the log's replayed frames written again into fresh pages (a new log file, or a seal of what was replayed), never over themselves; and that this guest's kernel has neither a device mapper nor fault injection, so the CI job with the privilege is where it is built and held. No engine change for it in this unit.
- From ${S}/fixes.md's concerns and its "no home" list. (9) log-errors' surviving mutants Sd, O5, E8 and E10 are equivalent only because every path to Db::sync_wal passes the kept error's refusal first: a debug assertion at its top that nothing is kept, and a mutant that trips it. (12) SegmentWriter::sync is public and undocumented, and now answers a helper's failed fdatasync at every later call and refuses every write after it: its doc says so, in two plain sentences. (15) The SyncPolicy doc comment is about a hundred lines of failure paths that name internals. A caller cannot read the contract off it: put the contract first in a few plain sentences per variant (what a commit that answers Ok promises, what a sync adds, what a failure answers and until when), then one short section for failures, with internals named only where a caller sets them; nothing true is dropped, it moves to the item it belongs to (Db::sync, Db::flush, Db::open). (16a) ROADMAP.md's items "A torn manifest slot on a device that tears one" and "The open's directory fsync held on a device" are read against the open as units other-slot and rename-arm left it. From the "no home" list: a failed grow of a run's mapping in commit_direct answers the key-ordering error and not the device's (${S}/fix/run-errors/notes.md, "Left out"): answer the device's, in the cold path; pages dropped after a real writeback error and read back through a run's mapping go into ROADMAP.md's item beside the fault-injecting device; a kept failure's later answers are new errors built from the first one's kind and text, so raw_os_error() is None on them (lost_answer): say so where the crate docs describe the kept failure, and put the errno in the text if it is not there; SyncPolicy::EveryN's commit count wraps at u32::MAX commits (${S}/fix/sync-run/notes.md): make the add saturating if that is free, and show it is by reading the assembly of the line.
- Items 20 and 29, by reading first: 20 (leave_direct and close_direct land a seal before the block tables are dropped, where the frozen list was full: is a carry of forms about to be dropped possible, and is it only work or is it wrong?) and 29 (under a cipher and manifest_pending, can a crash leave a pending envelope torn, given manifest_image syncs the envelope before the slot; if only damage can, the open's refusal there is the pager's and a comment says so; if a crash can, it is a defect: build it as a failing test and fix it).
Leave item 2 (a zero first sector with frames behind it, set aside where a commit frame verifies) and item 18 (the store that goes on past a failed publish) as the ROADMAP items they are: check each item's text says what is true now.`,
  },
  {
    key: 'count-back',
    kind: 'tidy',
    defects: ['fixes.md concerns 2, 3, 4: what stage 0 added to paths that run when nothing fails, by instruction count', 'followups 30, 42'],
    brief: `The instruction count of stage 0 against its base (${S}/fix/count/reading.md: read all of it, and count/rd/ beside it for how the figures were taken) found what the eight fixes cost where nothing fails. Rule 1 allowed a load and a branch folded into a check a path already made. CLAUDE.md holds a store in the clear to the count against the commit before the work, by function, and says so in two sentences that the count now contradicts (its paragraph on the plain store's price, "A store in the clear pays nothing an operation now", and the run's paragraph that puts Db::commit_direct under the main line's). Over the rule:
1. Db::commit_direct: +4 instructions a key, about +3,974 a commit of a thousand keys, in both ordered loads and the merge probe. The per-key loop's source did not change; run-errors (38378d1d) grew the function around it (the "nothing new" branch asking attends() and wal.pending, the sync's error handed to run_failed, run_fault_open) and the loop lost registers: by line, keys_in_stream reloaded each key, a pointer's drop glue, the slice compare.
2. pwritev_all: +37.6 a write: the IoSlice Vec collected through an out-of-line from_iter, and the fault compare.
3. SegmentWriter::sync out of line, about +21 a run sync; Db::sync_wal +14.7 a log sync (wal_dir_barrier out of line, the FAIL_SYNC_LIVE test, wal_trace.is_some).
4. A slot write: about +53 through Maint::publish, write_slot, vault::open_slot (no longer inlined), Maint::watch_slot (an out-of-line call whose body is one load) and sync_owed_slot. Unit engine-tidy of this run was to fold the publish's seams into one byte: read what it left.
5. Whatever this run's units added: count them yourself; their cost reviews read diffs, not counts.

The job: each per-key and per-commit path back to ${BASE0}'s count or under it, and each path beside a syscall back to a load and a branch. The shapes that have done exactly this before are in CLAUDE.md's paragraph on the plain store's price and the ones after it: the question rides a test the operation already makes; what a failure does is a function out of line that takes what it needs by reference (#[cold], #[inline(never)]), so the loop keeps its registers; a seam's byte is loaded once and its bits tested; a count that moved in a function nobody touched is read for what the function keeps live across the loop. Several shapes of each may have to be built and counted before one reads level: each is counted, none is guessed, and the notes keep every shape tried with its count. Behaviour does not change: every test stage 0 and this run added stays green, and the mutants of the lines you move are run again (the units' mutants.md files say which test killed which).

Method. The base's counts exist and are reused: ${S}/fix/count/base.<probe>.out, taken with ${S}/fix/count/run_counts.sh (the five probes and their arguments; the control is a point read, which must not move). Take HEAD's the same way into ${P}/count-back/ (copy the script, your own TMPDIR, one run at a time, bench examples built under the release profile in bench/), before your first change and after each shape; read by function with callgrind_annotate --auto=no (the reading says why agg.py's rows are not to be trusted), the function's own instructions and its inclusive count, per operation. Do not check the base's sources out over the tree: your work is in it. Where this run changed bench/examples so that a base output no longer compares (a probe's arguments or its loop), say so and compare what does.

What cannot come back is named, with its reason, where CLAUDE.md says "pays nothing": a delete now makes the compare against the store's greatest key that a put makes (the delete-max fix itself, about eighteen instructions a delete). Rewrite those two sentences of CLAUDE.md to what the final count shows, in that file's manner: by function, with the probe commands, no figure that the count did not produce.

One measurement for the owner with nothing to land: since f8d130a8 an append above a key that was deleted above the store's greatest goes through the log where it opened a direct run. With a probe of the suite's shape under bench/examples (an ordered load in which every hundredth batch first deletes a key above the greatest), count and time what such a load pays against the same load with no deletes, one run at a time, three runs each; report it as a property of a fix that has landed.`,
  },
]

const PRICING = {
  type: 'array',
  items: {
    type: 'object',
    properties: {
      what: { type: 'string' },
      command: { type: 'string' },
      sitting1: { type: 'string' },
      sitting2: { type: 'string' },
      reading: { type: 'string' },
    },
    required: ['what', 'command', 'sitting1', 'sitting2', 'reading'],
  },
}

const IMPL_SCHEMA = {
  type: 'object',
  properties: {
    unit: { type: 'string' },
    summary: { type: 'string' },
    land: { type: 'boolean' },
    verdict: { type: 'string', enum: ['level', 'unresolved', 'cost', 'not-priced'] },
    shape_landed: { type: 'string' },
    pricing: PRICING,
    success_path_cost: { type: 'string' },
    files_changed: { type: 'array', items: { type: 'string' } },
    tests_added: { type: 'array', items: { type: 'object', properties: { name: { type: 'string' }, failed_before: { type: 'string' }, passes_after: { type: 'boolean' } }, required: ['name', 'failed_before', 'passes_after'] } },
    mutants: { type: 'array', items: { type: 'object', properties: { mutant: { type: 'string' }, killed_by: { type: 'string' } }, required: ['mutant', 'killed_by'] } },
    survivors: { type: 'array', items: { type: 'string' } },
    restated_tests: { type: 'array', items: { type: 'string' } },
    not_built: { type: 'array', items: { type: 'string' } },
    cipher: { type: 'string' },
    contract_changes: { type: 'array', items: { type: 'string' } },
    not_verified: { type: 'array', items: { type: 'string' } },
    engine_changed: { type: 'boolean' },
    tree_clean: { type: 'boolean' },
  },
  required: ['unit', 'summary', 'land', 'verdict', 'shape_landed', 'pricing', 'success_path_cost', 'files_changed', 'tests_added', 'mutants', 'survivors', 'restated_tests', 'not_built', 'cipher', 'contract_changes', 'not_verified', 'engine_changed', 'tree_clean'],
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
    landed: { type: 'array', items: { type: 'object', properties: { unit: { type: 'string' }, sha: { type: 'string' }, holds: { type: 'string' }, price: { type: 'string' } }, required: ['unit', 'sha', 'holds', 'price'] } },
    not_landed: { type: 'array', items: { type: 'object', properties: { unit: { type: 'string' }, numbers: { type: 'string' }, choices: { type: 'string' } }, required: ['unit', 'numbers', 'choices'] } },
    concerns: { type: 'array', items: { type: 'string' } },
    owner_items: { type: 'array', items: { type: 'string' } },
    behaviour_changes: { type: 'array', items: { type: 'string' } },
    still_open: { type: 'array', items: { type: 'string' } },
    path: { type: 'string' },
    tree_clean: { type: 'boolean' },
  },
  required: ['landed', 'not_landed', 'concerns', 'owner_items', 'behaviour_changes', 'still_open', 'path', 'tree_clean'],
}

const LENSES = [
  {
    key: 'crash',
    text: `Your lens is correctness under failure. Does the fix close the hole on every path that reaches it (each sync policy, hashed and ordered tables, every way a run closes, the direct run's arms, background and inline segment work, the cipher arm), without opening another window? Walk the states by hand: at each point where a process kill, a power cut or a device error can land, what does the next call or the next open answer, and is that a state the store acknowledged, a prefix of its commits? Does every legit crash window of CLAUDE.md's "Crash discipline" paragraph still open? Did the tests fail before the fix for the reason they name (read before.log), and would any still pass with the fix deleted or halved (name the mutant)? Does any test bet on timing where it should hold a state? Is the measuring field gone, with nothing of the old shape left reachable? Is anything claimed that was not checked?`,
  },
  {
    key: 'cost',
    text: `Your lens is cost and contract. List, by function, everything the diff adds on a path that runs when nothing fails: instruction, load, store, branch, syscall, wait, lock, allocation. The brief allows one named thing, and the pairings price it; anything else beyond a load and a branch folded into an existing check is blocking. Then read the pricing as a skeptic: do the two arms of each pairing differ in the measuring field and nothing else (read the measuring patch); was each cell read per sitting and by arm name; does the pairing reach the path the fix changed (the phase stamps or a count must show the mechanism ran in the new arm and not in the old); is "level" claimed from two sittings; is there a cell that shares the changed path and was not priced (CLAUDE.md: a change priced on the cell it was made for is read on every cell that shares the path); does a figure in the docs or the commit notes say more than the tables do? For each Options arm the changed code runs under, is the fix sound and the arm intact? What does a caller observe differently, and is each change in the crate docs and tested by its words? Did every doc that named this hole as an exception lose the exception (rule 9)? Propose up to five mutants most likely to survive.`,
  },
]

const kindNote = u => u.kind === 'price'
  ? 'This is a pricing unit: rule 13 applies in full.'
  : u.kind === 'fix'
  ? 'This is a fix unit for a defect found since stage 0: a test that fails first, the smallest fix, mutants. Nothing is priced by a pairing (verdict "not-priced", land true unless you cannot finish it, or the engine turns out not to have the defect, which you then show), rule 13 does not apply, and rule 1 allows the one thing the brief names where the brief names it, and nothing else.'
  : 'This is a tidy unit: nothing is priced (verdict "not-priced", land true unless you cannot finish it), rule 13 does not apply, and rule 1 allows nothing on a success path beyond a load and a branch folded into a check the path already makes.'

function implementPrompt(u) {
  return `${CONTEXT}
YOUR UNIT: "${u.key}" (${u.defects.join('; ')}). ${kindNote(u)} You build it: tests in and shown failing, the fix${u.kind === 'price' ? ', its pairings and their verdict' : ''}, then the mutants. You do not commit.

${u.brief}
${RULES}
Steps:
0. "git status --porcelain" must be empty before you start. If it is not, touch nothing, and return land false with tree_clean false and what you found.
1. Read git log ${BASE0}..HEAD (subjects, and the diffs of the commits that touch the code you will change), deferred.md and followups.md for your items, report.md for your defects, and the code you will change with every caller of it. Start ${P}/${u.key}/notes.md.
2. Tests and their seams in, run before any fix: ${P}/${u.key}/before.log, with the assertion each fails at.
3. The fix, smallest change that closes the hole on every path${u.kind === 'price' ? ', with the shape before it kept behind the measuring field (rule 13a)' : ''}. Read the diff afterwards for anything on a success path.
4. Tests and neighbours (cargo test --profile checked --test durable; --test db; --test cipher; --lib; --test fuzz; --test segwriter where you touched the writer), then once the whole suite with --no-fail-fast in the background: ${P}/${u.key}/after.log.
${u.kind === 'price' ? `5. Price it (rule 13b to 13f), the machine to itself. Every pairing's command, both sittings' cells and your reading go in notes.md and in the result's pricing list. If the unit does not land, follow 13f and stop here.
6. Take the measuring field and its arms out (13g); build and run the unit's tests again on the tree that will land.` : '5. (No pricing.)\n6. (No measuring field.)'}
7. Mutants (rule 6) on the tree that will land, into ${P}/${u.key}/mutants.md. Put the tree back exactly.
8. Docs (rule 9). Run "sh scripts/fmt.sh" the way scripts/check.sh's lint group checks it and fix what it reports.
9. Finish notes.md: what was wrong, what you changed and why that shape, what each test holds, the pricing table, what you left out and why, what you did not verify.
Leave the work uncommitted in the working tree (or, for a unit that does not land, the tree clean and the patch saved). Return the structured result; it is data for the next step, not a message to a person. Set engine_changed to true if anything under src/ changed.`
}

function reviewPrompt(u, lens, round, impl) {
  return `${CONTEXT}
YOU ARE A REVIEWER of unit "${u.key}" (${u.defects.join('; ')}), review round ${round + 1}. ${kindNote(u)} The unit's work is uncommitted in the working tree: read it with "git status" and "git diff", with ${P}/${u.key}/ (before.log, after.log, mutants.md, notes.md, measuring.patch, pairs/) beside it. You are read-only: edit no tracked file, start no build of changed sources, run no mutant and no timing run yourself (describe a mutant under extra_mutants and the repair step runs it). You may run an already built test by exact name under a TMPDIR of your own (${S}/tmp/review-${u.key}-${lens.key}) and must remove that directory after.

What the unit was asked to do:
${u.brief}
${RULES}
${lens.text}

Default to "repair" when you find a real problem and to "pass" when you do not: a blocking item is a wrong or incomplete fix, a window still open or newly opened, a legit crash window that now refuses, a test that does not test what it names or bets on timing, a cost on a success path the brief did not allow, a pricing that does not show what it claims, the measuring field left in, an arm broken or removed, something disabled or limited, a cipher arm left unfixed, or a claim in the docs that is false. Style and wording are nonblocking. Give file and line for each item, and say what you did not check.

The implementer's own account (data, to be checked, not believed):
${JSON.stringify(impl, null, 1).slice(0, 16000)}`
}

function repairPrompt(u, round, blocking, extra, source) {
  return `${CONTEXT}
YOUR UNIT: "${u.key}" (${u.defects.join('; ')}), repair round ${round + 1}. ${kindNote(u)} The unit's work is uncommitted in the working tree, with its notes under ${P}/${u.key}/. ${source} found the problems below. Deal with each: fix it, or show with a test or a line of code why it is not a problem (and say so). Run every extra mutant; a survivor means a missing test or an unreachable check. Keep everything the unit already fixed. ${u.kind === 'price' ? 'If this round changes code on a path that runs when nothing fails, or a reviewer showed a pairing does not reach the mechanism, the affected pairings are run again: put the measuring field back from measuring.patch (adapted), price as rule 13 says, and take it out again.' : ''} Then run the unit's tests and the neighbours again, the format step, and update notes.md and mutants.md. You do not commit.

What the unit was asked to do:
${u.brief}
${RULES}
BLOCKING:
${JSON.stringify(blocking, null, 1)}

EXTRA MUTANTS TO RUN:
${JSON.stringify(extra, null, 1)}

Return the structured result for the unit as it now stands (all of it, not only this round's changes; the pricing list whole). Set engine_changed to true only if this round changed anything under src/.`
}

function commitPrompt(u, impl, unresolved) {
  return `${CONTEXT}
YOU COMMIT unit "${u.key}" (${u.defects.join('; ')}). Its work is uncommitted in the working tree; its notes are under ${P}/${u.key}/.
${RULES}
Steps:
1. git status --porcelain and git diff --stat: the change is the engine's sources, tests, CLAUDE.md, ROADMAP.md and docs${u.key === 'open-cost' ? ', and a probe under bench/examples' : ''}. Nothing else belongs: remove stray files that are not the unit's and say what you removed. Search the diff for "price_" : the measuring field and its bench arms must be gone (rule 13g); if they are not, do not commit, and return committed false saying so.
2. Run the unit gate from a clean TMPDIR of your own, in the background with the exit status captured: "sh scripts/check.sh build test lint"${u.key === 'open-cost' ? ', and then "sh scripts/check.sh bench", since bench/ changed' : ''}. Read each group's verdict from its exit status and the "test result:" lines. A test binary that ends with code 101 and no "test result:" line is a watchdog's exit (CLAUDE.md). Wait for it: the gate has its verdict when its log has the exit status you wrote after it (a cold build takes tens of minutes, a warm one a few), and a gate left running when you answer is stopped with you. Never answer before that line is there; never commit, and never return committed false, on a gate that has not finished. How to wait: write the exit status to the log after the command ("...; echo EXIT $? >> log"), then block on that line with the Monitor tool (an until-loop that greps the log for "^EXIT") or a foreground command that does the same, again and again until the line is there. A turn that ends while the gate still runs is taken as your answer, and the gate goes on running under whoever comes next: if you must answer without a verdict, stop the gate's processes first and say so.
3. If the gate is red for something mechanical (formatting, a clippy lint, a doc link), fix it, list it under fixed_in_passing, and run the failed group again. If it is red for anything else, do not commit: return committed false with the failing test's name and output.
4. Commit everything that belongs: one commit. The subject is one sentence naming the behaviour now held, in the manner of "git log --oneline -15" (no prefix tag). The body says what a caller saw, the mechanism in a few sentences, what the fix does and what it adds when nothing fails${u.kind === 'price' ? ', the pricing (each pairing as the command that reproduces it, with the measuring arm named as kept in the measuring tree only, and the cells rounded, per sitting)' : ''}, what the tests hold and how many mutants they were held to, and what is left open. No model or tool names. Write the message to a file and use git commit -F. The message ends, after a blank line, with the attribution lines your own harness gives you for commits (a Co-Authored-By line and a Claude-Session line); if it gives you none, copy the two trailer lines of the previous commit (git log -1 --format=%B).
5. Do not push. Confirm git status --porcelain is empty and return the new sha.

The unit's account:
${JSON.stringify(impl, null, 1).slice(0, 11000)}

Reviewers' items still open after the last repair (put any that are true into the commit body's "left open"):
${JSON.stringify(unresolved, null, 1).slice(0, 4000)}`
}

const results = []
let stopped = null
const units = ONLY ? UNITS.filter(u => ONLY.includes(u.key)) : UNITS

for (const u of units) {
  log(`unit ${u.key}: building (${u.kind})`)
  let impl = await agent(implementPrompt(u), { label: `fix:${u.key}`, phase: 'Fix', model: 'opus', schema: IMPL_SCHEMA })
  if (!impl) { stopped = `${u.key}: the implementer returned nothing`; break }
  if (!impl.land) {
    results.push({ unit: u.key, kind: u.kind, defects: u.defects, impl, unresolved: [], commit: null })
    if (!impl.tree_clean) { stopped = `${u.key}: did not land and left the tree unclean`; break }
    log(`unit ${u.key}: not landed (${impl.verdict}); its patch and numbers are kept, the tree is clean`)
    continue
  }

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
    if (fixed && !fixed.land) break
    if (!blocking.length && !(fixed && fixed.engine_changed)) {
      log(`unit ${u.key}: the reviewers passed it and the extra mutants changed no engine code; no further review`)
      unresolved = []
      break
    }
    if (round === MAX_REVIEWS - 1) log(`unit ${u.key}: committed after the last repair without a further review; open items are in the result`)
  }
  if (!impl.land) {
    results.push({ unit: u.key, kind: u.kind, defects: u.defects, impl, unresolved, commit: null })
    if (!impl.tree_clean) { stopped = `${u.key}: a repair found it does not land and left the tree unclean`; break }
    log(`unit ${u.key}: not landed after a repair (${impl.verdict})`)
    continue
  }

  let c = await agent(commitPrompt(u, impl, unresolved), { label: `commit:${u.key}`, phase: 'Commit', model: 'opus', schema: COMMIT_SCHEMA })
  if (c && !c.committed) {
    log(`unit ${u.key}: the unit gate was red; one repair and one more try`)
    const fixed = await agent(repairPrompt(u, 9, (c.problems || []).map(p => ({ title: 'the unit gate is red', detail: p, where: 'sh scripts/check.sh build test lint' })), [], 'The commit step, whose gate (sh scripts/check.sh build test lint) was red,'), { label: `repair:${u.key}:gate`, phase: 'Commit', model: 'opus', schema: IMPL_SCHEMA })
    if (fixed) impl = fixed
    c = await agent(commitPrompt(u, impl, unresolved), { label: `commit:${u.key}:2`, phase: 'Commit', model: 'opus', schema: COMMIT_SCHEMA })
  }
  results.push({ unit: u.key, kind: u.kind, defects: u.defects, impl, unresolved, commit: c })
  if (!c || !c.committed) { stopped = `${u.key}: not committed`; break }
  log(`unit ${u.key}: committed ${c.sha}`)
}

let gate = null
let count = null
if (!stopped) {
  const gatePrompt = `${CONTEXT}
YOU RUN THE FULL GATE on HEAD, after every unit of this run that landed has committed (git log ${BASE}..HEAD).
${RULES}
Run "sh scripts/check.sh" (every default group: build test lint wasm bench) in the background from a clean TMPDIR of your own, exit status captured, and wait for it; the bench group builds the suite. The container may have been restarted since the last build: run "sh scripts/deps.sh" first and go on only at its "all deps ready" line. How to wait: write the exit status to the log after the command ("...; echo EXIT $? >> log"), then block on that line with the Monitor tool (an until-loop that greps the log for "^EXIT") or a foreground command that does the same, again and again until the line is there. A turn that ends while the gate still runs is taken as your answer, and the gate goes on running under whoever comes next: if you must answer without a verdict, stop the gate's processes first and say so. Nobody else writes the branch or the tree while you run: if HEAD's tree changes under you, say so and run again. Read each group's verdict from its exit status, never through a pipe. If a group is red for something a unit's commit broke mechanically (a struct literal in bench/, formatting, a lint, a committed figure the figures check redraws), fix it and commit it as its own small follow-up commit in the repository's manner, ending with the same two attribution lines the unit commits end with (read one with git log -1), then run that group again. If it is red for anything else, do not paper over it: report the failing test and its output. Never skip, ignore or loosen a test. Do not push. Finish with git status --porcelain empty and return HEAD.`
  gate = await agent(gatePrompt, { label: 'gate:full', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })
  if (!gate || !gate.green) {
    log('the full gate answered without a green verdict; one more run of it before anything is counted')
    const again = await agent(gatePrompt + `\n\nThis is the second run of this step. The first answered: ${JSON.stringify(gate, null, 1).slice(0, 3000)}\nIf it left a check.sh running, stop its processes and remove its TMPDIR before you start. If a group was red for a real reason, do not run it again: report it.`, { label: 'gate:full:2', phase: 'Gate', model: 'opus', schema: GATE_SCHEMA })
    if (again) gate = again
  }

  count = await agent(`${CONTEXT}
YOU COUNT what this run's fixes cost a store when nothing fails: instructions by function, ${BASE} against HEAD, under callgrind. This is a count, not a timing run; still, run one thing at a time.
${RULES}
CLAUDE.md's Profiling section and its paragraph on the plain store's price say how, and ${S}/fix/count/ holds stage 0's count done the same way (read its notes for the probes, the toggles and what did not build): bench/examples load_only.rs (an ordered load through the direct run; also under a buffered arm, supdb-ingest), work_probe.rs with "whole" (a writer's round through the log and the hash table), read_only.rs with "n-none" (the control that must not move), merge_walk.rs (a merge), each run under "valgrind --tool=callgrind" with the toggle on the engine function named exactly, read by function with callgrind_annotate, the functions' own instructions side by side, never the total alone.

Both sides are built from HEAD's example sources. One target directory and little disk, so do it in the tree: build HEAD's examples and run the counts; then put the base's engine in place with "git checkout ${BASE} -- src" (the examples and bench/ stay HEAD's), build, run the same counts, and put HEAD back with "git checkout HEAD -- src" and "git status --porcelain" empty. Keep the callgrind outputs and annotations under ${P}/count/, not the binaries.

Report per probe and per function that moved: the base's count, HEAD's, the difference per operation, for at least Db::commit, Db::commit_direct, Db::sync, the run's close (close_run, leave_direct, close_direct), the log's write and sync, the segment writer's append, sync and helper, Maint::publish and the open. Say which unit each difference belongs to and whether it is the thing that unit's brief allowed. Do not fix anything; do not commit. Return HEAD and whether the tree is clean.`,
    { label: 'count:callgrind', phase: 'Count', model: 'opus', schema: COUNT_SCHEMA })
}

const critic = await agent(`${CONTEXT}
YOU ARE THE COMPLETENESS CRITIC for this run. Read-only on the repository (no edits, no builds, no timing runs); you write one file, ${P}/priced.md (from the shell if the Write tool refuses).
${stopped ? 'The run STOPPED EARLY: ' + stopped + '. The working tree may hold that unit\'s uncommitted work; say exactly what state it is in (git status, git diff --stat) and do not touch it.' : 'Every unit either committed or was set aside with its patch and numbers.'}

Read git log ${BASE}..HEAD with each commit's diff, ${P}/*/notes.md, mutants.md and pairs/, ${S}/fix/deferred.md, ${S}/fix/followups.md and ${S}/fixes.md. Then answer:
- landed: per unit, the commit, what its tests hold in one sentence, and its price as the tables show it (per sitting; say where a figure in a commit message says more than its table).
- not_landed: per unit, the numbers that kept it out, each shape tried, and the choices left (accept the cost, another shape, leave the hole documented), with what each costs a caller.
- still_open: every item of deferred.md, of followups.md (by number) and of stage 0's fixes.md that no commit of this run or of stage 0 closes, with what it needs.
- concerns: a fix incomplete on some path (a sync policy, a way a run closes, an Options arm, the cipher arm), a pairing that does not reach its mechanism, a cell that shares a changed path and was not priced, a test that bets on timing, a surviving mutant, a doc that still names a closed hole as an exception or says something false. File and line.
- behaviour_changes: every way a caller now sees something different, one line each, in plain words an owner can say yes or no to.
- owner_items: what is the owner's to decide and what each choice costs.
Say also, by reading the code as it stands and the units' tests: whether run-tail-kill's power-cut sibling (a later log commit synced, the run's file before it not) is closed by the sync order units run-close and due-log built, or still waits on the shared log and run sequence; what else waits on that sequence (a store written before f8d130a8 that holds a run of a key deleted above the greatest); and for every item ${S}/fixes.md lists as having no home, whether this run gave it one.
Write it all to ${P}/priced.md, short lines, most important first. Return the same as structured data.

The units' results (data):
${JSON.stringify(results.map(r => ({ unit: r.unit, kind: r.kind, commit: r.commit, land: r.impl && r.impl.land, verdict: r.impl && r.impl.verdict, shape: r.impl && r.impl.shape_landed, pricing: r.impl && r.impl.pricing, summary: r.impl && r.impl.summary, not_built: r.impl && r.impl.not_built, survivors: r.impl && r.impl.survivors, restated: r.impl && r.impl.restated_tests, contract_changes: r.impl && r.impl.contract_changes, cost: r.impl && r.impl.success_path_cost, cipher: r.impl && r.impl.cipher, not_verified: r.impl && r.impl.not_verified, unresolved: r.unresolved })), null, 1).slice(0, 70000)}

The full gate: ${JSON.stringify(gate).slice(0, 4000)}
The instruction count: ${JSON.stringify(count).slice(0, 8000)}`,
  { label: 'critic', phase: 'Critic', model: 'opus', schema: CRITIC_SCHEMA })

return { stopped, results, gate, count, critic }
