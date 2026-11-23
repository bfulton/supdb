# G1-3 cost: notes

Started 2026-11-20 12:20 UTC at HEAD 352ec963 (tree clean).

## What the tree has that the plan (682b2c09) did not
- scripts/cost.sh and scripts/cost-hot.txt exist (af56aac, 5ca5a84, 3f65bfb): six probes
  (read, scan, load_durable, load_buffered, work, merge), base in a worktree with this tree's
  example sources, own target $W/tb removed at the end; comparator inline python; fails on a
  hot-list function gaining > COST_SLACK (0.5) an op. No red input, no self-check, no cost
  group in check.sh, no CI job.
- No feature is needed for symbols: bench's [profile.release] has debug = 1 (line tables);
  the script sets no RUSTFLAGS. The plan's "a cargo feature" has nothing to carry.
- Upstream head the branch last merged: merge-base HEAD origin/claude/e-recode-stride-selector-g5f6hl
  = 9bfc4362 (merged as bb365602, after 4358827, the head counted against before it). Upstream is now at cd7e9dc5 (10 more).
- No bench/examples/seal_probe.rs, scan_at_probe.rs; no tests/data/cost-entries.txt.
- tests/source.rs exists (G1-1 wall, G1-2 door), lexer tests/lex/mod.rs.
- impl Db has no read method: reads reach a Db through Deref<Target = Reader> (db.rs, impl
  Deref for Db). Pub fns at HEAD: impl Reader 71, impl<B: Bytes> Blob<B> 67, impl Db 187.
- SCAN_REST = 1024 (db.rs); scan_rested is entered only where zcensus_begin(true) says a
  cipher store and limit > SCAN_REST.
- Disk: 11 GB free at start; bench/target 11 GB, librocksdb-sys build alone 2.4 GB, so a
  second bench target is ~3-4 GB.

## Log (restart at 12:30 killed attempt 1; attempt 2 from 12:31)
- 12:34 read the draft in the tree: cost.sh (11 probes, COST_HEAD/COST_*_TARGET), cost-diff.py
  (comparator, EMPTY side check, hot list empty -> exit 2), cost-red.sh (A comparator, B pager
  branch at read_all -> RED, C probe-only change -> green), seal_probe.rs, scan_at_probe.rs,
  lag0_probe len arg, tests/source.rs entry-point list, tests/data/cost-entries.txt (325 lines).
  Missing: check.sh cost group, CI job, self-check step, the runs.
- 12:34 removed run1 (worktree + 1.3G partial tb). Whole run 1 vs upstream 9bfc4362:
  COST_DIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run1 TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3 sh scripts/cost.sh 9bfc4362 ; log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run1.log (EXIT line at end).
- 12:36 before.log written: HEAD's inline comparator passes an empty side (a3, exit 0) and an
  empty hot list (exit 0); HEAD has no cost-red.sh, no cost group (unknown group, exit 2), no COST_HEAD,
  no cost-entries.txt. Fixtures in /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/before (remove at the end).
- 12:36 edits (no cargo; run1 in flight, cost.sh/cost-diff.py/bench examples untouched while it runs):
  cost-red.sh gains step S (tree vs itself: green and "0 functions moved"), COST_RED_TARGET (shared
  target kept across runs), COST_RED_STEPS (steps as functions, run in the order named; unknown step
  fails). check.sh gains group cost (cost-red then cost.sh ${COST_BASE:-origin/main}, each || exit 1),
  outside ALL. ci.yml gains job cost (fetch-depth 0, valgrind, base = PR base / push before / HEAD~1).
  Step A alone: ok; an unknown step: exit 1.
- 12:48 run1 (cost.sh vs 9bfc4362, all 11 probes) EXIT 1 at 12:47:35, 13 min whole, base target
  ~? GB. FINDING: the hot list's lines never match callgrind's names. callgrind prints
  `<supdb::db::Reader>::read_all_in::<G>::{closure#5}` and `<supdb::blob::Blob<supdb::bytes::MmapBytes>>::...`;
  the list says `supdb::db::Reader::read_all`, `supdb::blob::Blob<B>::scan_at`. The only hits were
  substrings of the probe functions inside generic arguments (`supdb_range::{closure#0}` in a monomorph's
  name), so the read probe could catch nothing. Fixing cost-diff.py: canonical names (generic args
  stripped, `<T>::f` -> `T::f`) matched by path prefix; hot list rewritten from the functions the probes run.
- 12:51 cost-diff.py: canon() (generic args stripped, <T>/<T as Tr> read as T), compare by
  canonical name (monomorphs summed), hot lines canonical, matched by path prefix ('::' = module/type),
  NO HOT per probe, SEEN file -> cost.sh names lines no probe reached. cost-hot.txt rewritten (86 lines)
  from the functions run1's probes ran (py/dump.py). Step A gains a5 a6 a7 (callgrind-form names). A ok.
  Re-compared run1's counts (cmp3.*): explained in the section below.
- 12:51 self-check run (all probes, tree vs HEAD):
  COST_DIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run2 COST_BASE_TARGET=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/rt TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/../tmp/G1-3 sh scripts/cost.sh HEAD; log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run2.log
- 13:05 run2 (self-check, tree vs HEAD, all probes) EXIT 1 at 13:03:46 (13 min): every hot
  function level except zfile::read_budget +0.84/op in load_buffered (the controller's cgroup read on
  a clock; it moved -0.42 in run1's load_buffered). Non-hot functions move between two runs of one
  build (memcmp +0.83/op in read; malloc, memcpy in merge). Changes: '!' lines in the hot list
  (read_budget out); cost-diff prints "N hot functions ran, K of them moved"; S asserts K=0 (not
  "0 functions moved", which run2 shows is false for libc). Re-compared run2 with them: green in all
  11 probes, toggled probes 0 hot moved, untoggled 1-4 hot moved at +-0.01/op. A gains a8/a9 (! and ::).
  Deleted rt/release/examples (1.1G) for disk: 5.1G free.
- 13:05 cost-red whole: COST_RED_TARGET=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/rt TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/../tmp/G1-3 sh scripts/cost-red.sh; log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/red1.log

## run1: the tree against upstream 9bfc4362, by function (cmp3.* in this directory)
Totals, an op: read -7.36, read_z -0.55, scan -52.6, scan_long -52.6, scan_z -566, load_durable -3.4,
load_buffered -8.0, work -11.1, merge -198.5, seal -0.35, scan_at +239.4. Verdict RED (by function).
Every hot gain read by its callers and callees:
- Moves between functions, the loss beside the gain within an instruction or two: read_all{closure#5}
  -187 -> read_all_in{closure#5} +189 (the read split for the cipher's entered/finish, CLAUDE.md "The
  question rides a test"); Reader::leave -51 -> leave_looked +35; Reader::enter/leave -> Db::op +17/+35,
  WriterOp::drop +19/+38 (WRITER_OP depth, CLAUDE.md); Blob::scan_at -69 -> scan_whole +61, Blob::flat
  -35 inlined into scan_seek +15 (CLAUDE.md "scan_seek fifteen over by its own count"); merge:
  values_at -> values_at_pieces (238 = 238, a rename), flat -315 and merge_ranks closure -284 into
  key_at +252 and compact_run (compact_run<NoRest>, +112 canonical); read_z: read_exts -95 -> each_value
  +64 and read_exts_inline +15 (CLAUDE.md "Out of line for the form's reads"), part_read_z +22.5.
  Net over the hot functions an op: read -7.0, read_z -0.9, scan -17.7, scan_z -572, work -34.6.
- scan_z: view_with +14.2, open_at +10.2, parse_page +7.3, ask_group_ahead +7.2 a scan with their call
  counts identical on both sides (5094, 8226, 1452, 8226 calls; callgrind's call lines) and zpages.rs
  unchanged since 9bfc4362: inlining moved, beside walk_impl -600 a scan.
- merge: hot functions +102.8 an op between them, total -198.5: the inlining above (compact_run took in
  flat and the merge_ranks closure).
- load_durable zfile::read_budget +0.64: the controller's clock (run2 read it +0.84 against itself).
- scan_at (row 8, new): scan_at_probe::scans +236.5 a scan of 100 entries, FlatIndex::search +3: REAL,
  not a move. Line counts (ann/linediff.py over callgrind_annotate --auto=yes): the walk is now
  walk_fixed! (blob.rs, `$rank += 1` 200 a scan against `rank += 1` 100, `$recs.get(e_at..e_at + 4)`
  200 against 100) -- about two instructions an entry more in the walk a caller's own callback
  instantiates -- and the flat index's search takes a `Sect` (search::<&[u8]>, `sec.part` for `sec.get`).
  The engine's own instantiation of the same walk (scan probe, Blob::scan_at_in) reads +2 a scan,
  0.02 an entry. So a caller of the public Blob::scan_at pays ~2.4 instructions an entry over upstream;
  no count before this unit had the row. Not fixed here (this unit changes no engine code): it is the
  next unit's or the owner's to take, and cost.sh against upstream will keep reading it red.
- 13:22 red1 (cost-red A S B C) EXIT 0 at 13:22:25, 17 min: S 0 hot moved green; B RED supdb_get +7.00/op; C green. Then cargo test source.
- 13:26 lint group green (lint3.log; fmt fixed in source.rs: SHAPE const, rustfmt was not idempotent on the long literal); bench clippy/fmt/workflows/policy green (benchlint2.log); source 69 ok (source1.log). Whole suite: TMPDIR=$T cargo test --profile checked --no-fail-fast > /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/after.suite.log
- 13:32 suite run 1 (after.suite.dotdot.log): leak 1 + zpages 4 failed -- TMPDIR had '..' ($S/../tmp), the CLAUDE.md case (/proc/self/maps prefix). Rerun with the literal path.
- 13:38 suite run 2 green: 27 binaries, EXIT 0 at 13:38:41 (after.suite.log). keep/ written with SHA256SUMS (12 files). Mutants: python3 /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/mutants.py M1_totals ... ; results /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/mutants.results, per-mutant logs mutant-*.log, runner log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/mutants.run.log
- 14:16 mutants pass 1 (mutants.results): 14 killed, 2 survived: O9 (substring match; a10 added: read_all is not read_all_untimed) and O3 (toggle dropped: added step T, a thousand-step loop at every commit, which the point read's toggle must not see). Running T alone, then O9 and O3 again. log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/redT.log
- 14:33 mutants pass 2: O9 killed by A a10, O3 killed by T; O8 killed in a scratch worktree (o8_*.log). Final whole cost-red: log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/red2.log

## Where it stands (the finished copy of each file is the tree's; keep/ holds them with sums)
Files: scripts/cost.sh, scripts/cost-diff.py (new), scripts/cost-hot.txt, scripts/cost-red.sh (new),
scripts/check.sh (group cost), .github/workflows/ci.yml (job cost), bench/examples/lag0_probe.rs
(len argument), bench/examples/seal_probe.rs and scan_at_probe.rs (new), tests/source.rs (entry-point
list), tests/data/cost-entries.txt (new), CLAUDE.md (the cost.sh paragraph: the probes now, and the rule
the hot list's names taught).

What the gate holds: every function on the hot list (canonical names, path-prefix match, `!` out),
per probe, base against head, within 0.5 instructions an operation; a probe with no hot function or a
side that collected nothing is red; the red input (cost-red.sh A S B T C) shows the comparator red and
green where it must over fixtures in callgrind's own spelling, the tree against itself with no hot
function moved, a load and a branch at the top of read_all red, a cost outside the toggle green, and a
probe-only change green, each by exit status and verdict line. The entry-point list holds every pub fn
of impl Reader, impl Db and impl<B: Bytes> Blob<B> to a probe the default run counts or a reason.

What it cannot see: a row missing inside a probe that is named (forms.md §6); a path no probe takes
(Blob::get and the rest of the public Blob readers have reasons, not probes); the untoggled probes
(loads, work, merge) count their setup too, so a per-key cost there is diluted by nothing but read in
a probe whose instructions include the store's open and close; functions inlined everywhere today
(the run names those lines, e.g. Reader::read_all, Blob::seek, Db::put, Db::delete, flatindex::lookup)
are seen only through the function they were inlined into, which the list must also name; and a move of
code between two listed functions reads as a gain in one (against upstream, every listed move did).

Departures from the plan:
1. No cargo feature: bench's [profile.release] already has debug = 1, which is all callgrind needs
   for names; RUSTFLAGS are never set.
2. The hot list is matched by canonical name and path prefix, not substring: the draft's substrings
   matched no engine function (run1). The comparison is by canonical name, monomorphs summed.
3. The red input is a script with five steps (A S B T C), not one scratch edit: S is the plan's
   self-check made standing; T exists because O3 survived without it; C holds that the probe's
   sources are the head's on both sides (the plan's mutant 3).
4. The self-check reads zero for every hot function in a toggled probe, not "every function": libc's
   and the allocator's counts move between two runs of one build (run2: memcmp +0.83/op in read), the
   untoggled probes' hot functions move by +-0.01/op, and zfile::read_budget (the controller's clock)
   by +0.84/op, which `!` takes out.
5. cost-red.sh refuses to run S/B/T/C when the engine or the suite in the tree differs from HEAD.
6. The entry-point list holds every pub fn of Db, not only its reads (a Db reads through Deref).
7. The CI job's base: the pull request's base, else the push's before, else HEAD~1.

Callgrind rows (this unit is the instrument; it changes no engine code -- `git diff --stat -- src`
is empty): run1 (vs upstream 9bfc4362, all 11 probes) RED with every hot gain explained above but
scan_at's, which is a real +2.4 instructions an entry on the public Blob::scan_at for a caller's own
callback; run2 (vs HEAD, all 11) green in every hot function but read_budget, since taken out.
Rows 7 (seal) and 8 (scan_at) and row 3 (read_z, scan_z) and row 2's second half (scan_long) exist.

Under a cipher: nothing; no engine code changed, no option, no default.

Not verified: the CI job (never run on a hosted runner: its time, valgrind there, the base
expression on push and on dispatch); `sh scripts/check.sh cost` as one command was run once with
COST_BASE=HEAD (see below), never against origin/main; whether callgrind's names are spelled the same
under another toolchain (the canonical form assumes rustc's v0-style `<T>::f::<G>` printing, which
this toolchain gives); the whole run's disk on a host without bench/target already built (here the
base's target was 4-5 GB and the head reused bench/target).
- 14:51 red2 EXIT 1: A S B ok, T's head build failed -- rt had grown to 9.0G (a build of engine+suite+probe per copy path: s, b, t, c, the O8 worktree), 1.7G free. And step T copied to $R/t, the default shared target's own path. Fixed: one copy path $R/h for every step. Freed rt examples and supdb deps (6.8G free). Final: COST_BASE=HEAD COST_RED_TARGET=$S/rt COST_BASE_TARGET=$S/rt COST_DIR=$T/cw sh scripts/check.sh cost; log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/after.cost.log
- 15:18 after.cost.log EXIT 0 at 15:17: cost-red A S B T C ok and cost.sh HEAD green in all 11 probes (toggled: 0 hot moved). source test 69 ok after the last cost.sh edit; fmt clean; keep sums clean. Removed rt (the shared target), tmp/G1-3, before/ fixtures, ann .out copies. after.log composed. Not committed.
8. (found while building) cost-red.sh copies the tree at one path ($R/h) for every step: at a path a
   step the shared target held a build per path (9.0 GB, a head build failed for disk), and step T's
   copy at $R/t was the default shared target's own path, which a run without COST_RED_TARGET would
   have copied into and then removed.
9. cost.sh names the hot lines no probe reached only on a whole (default) run.

## Repair round 1 (started 15:30, uptime 2:55)
Reviewers: (B1/B3) toggled probes check only listed functions -- install_ahead, make_table, an outlined
helper pass; (B2) entry points read db.rs and blob.rs only. Plan for the round:
- cost-diff.py: in a toggled probe (OWN prefixes given) every function of the engine, the suite or the
  probe's crate is hot, and a library generic instantiated over them is keyed apart ("(over the engine)")
  and hot too; `!` still takes out. Evidence it is safe: run2 (tree vs itself) owned moves in the toggled
  probes are 0 but seal's AlignedWriter::write_buf_with +8 total (r1py/own.py). Against upstream (run1) the
  owned moves in toggled probes are the ones already explained.
- PARTIAL: a side whose rows do not sum to its PROGRAM TOTALS fails (extra mutant: --threshold dropped).
- One slack, cost-diff's default, 0.05 an op (extra mutants: slack 5 survives; commit_direct loop at
  ~0.3/key survives 0.5). run2's hot noise in untoggled probes max 481 instr = 0.0048/op.
- cost.sh: the suite's per-op code (SINK, consume, supdb_get, supdb_range, Supdb's Engine get/range) must
  be the same text at BASE and the head, else exit 2 (extra mutant: engines.rs offsets an engine gain).
  9bfc4362's text is the head's (checked).
- cost-red: fixtures a11-a17; step U (install_ahead load+branch, scan probe, RED), step V (suite guard),
  step G (check.sh cost propagates both scripts' failures). check.sh passes ${COST_PROBES:-}.
- tests/source.rs: entry points from every file under src/.
- 15:34 run3: cost.sh 9bfc4362 (all probes, new rule), COST_DIR=$S/run3 COST_BASE_TARGET=$S/rt; log $S/run3.log
- 15:46 run3 (cost.sh 9bfc4362, all probes, new rule) EXIT 1 at 15:45:42, 11 min. Every owned function
  that moved in a toggled probe is on the list already, and every gain the same as run1's, explained above.
  New at the tighter slack: load_durable/load_buffered Db::op +0.07, WriterOp::drop +0.08 (the WRITER_OP
  depth, as in work), pwritev_all +0.09 against writev_all -0.10 (a rename: the run's writer writes at the
  offset it counts, durability fix). install_ahead and make_table did not move against upstream. scan_at
  stays the one real cost (+2.4/entry, task #17). Unreached lines named as before.
- 15:40 cost-red A G V green (V with a second copy: consume's #[inline] -> #[inline(never)]).
- 15:46 red3: whole cost-red (A G V S B U T C), COST_RED_TARGET=$S/rt; log $S/red3.log
- 16:12 red3 EXIT 0 at 16:12 (24 min): A G V S B U T C ok; S 0 of 11 hot moved; B supdb_get +7; U install_ahead +7. keep2/ written (12 files + SHA256SUMS). Script mutants: python3 $S/mutants2.py -> $S/mutants2.results, logs mutant2-*.log
- 17:02 mutants2 EXIT 0 at 17:00: 29 of 29 killed (mutants2.results), sums clean. Engine mutants: python3 $S/emutants.py -> $S/emutants.results, logs emutant-*.log, runner log $S/emutants.run.log
- 17:29 emutants EXIT 0 at 17:29: 6 of 6 killed. Now lint (lint4.log), source test (source2.log), then check.sh cost with COST_BASE=HEAD (after.cost2.log).
- 17:31 lint5 green (fmt fixed source.rs; keep2 resummed), source2 69 ok, bench fmt ok. after.cost2: COST_BASE=HEAD check.sh cost (log $S/after.cost2.log)
- 17:32 after.cost2 restarted (first try killed at 3.7 GB free; rt/release/examples 3.9 GB removed)
- 18:02 after.cost2 EXIT 0 (29 min): cost-red ok, cost.sh HEAD green in all 11, toggled 0 hot moved
  with every owned function hot. Removed rt, tmp/G1-3, fixture copies. before-round1.log /
  after-round1.log written. keep2 sums clean. Not committed.

## Where it stands after round 1 (the tree's copy of each file is the finished one; keep2/ with sums)
Round-1 changes: scripts/cost-diff.py (OWN: every engine/suite/probe function hot in a toggled probe,
library generics over them keyed "(over the engine)"; PARTIAL; DEFAULT_SLACK 0.05, the one definition),
scripts/cost.sh (passes OWN for toggled probes, no slack default of its own, the suite guard: exit 2
where SINK/consume/supdb_get/supdb_range/Supdb's get/range differ, attributes included),
scripts/cost-red.sh (fixtures a11-a18, steps G, V, U; default A G V S B U T C), scripts/check.sh
(${COST_PROBES:-}), scripts/cost-hot.txt (header: what the list is for), tests/source.rs (entry
points from every file under src/, a case per submodule), CLAUDE.md (the rule the review taught).
Departures this round:
10. Blocking 1's "UNLISTED" fail and adding install_ahead/make_table to the list: not done; blocking 3's
    shape instead, which holds more (every owned function a toggle collects is hot, so no list need be
    complete there). install_ahead is kept off the list on purpose: step U holds the toggle rule by it
    and fails if it is ever listed.
11. Slack 0.5 -> 0.05 an op: the reviewers' commit_direct loop (0.41/key) and slack mutants needed it;
    run2 and after.cost2 put the hot noise in the untoggled probes at <=0.005/op. CI's noise there is
    not measured.
12. The suite's sources are not copied into the base (the base's engine cannot build the head's suite);
    its per-operation code is compared and the run refused where it differs.
Not verified (added): the untoggled probes' hot noise on another machine (CI) against the 0.05 slack;
check.sh cost on CI; the suite guard's item extraction by brace count (a brace in a string or comment
of those items would mislead it; none today).

## Repair round 2 (started 18:07, uptime 5:37)
Reviewers: (1,3) the slack is per op, so a cost a block or a piece passes; (2) check.sh's sentence of
the threshold is wrong; (4) the untoggled probes hold the list alone. Ten extra mutants.
- Data first: run2/out (the tree against itself, all 11 probes, kept) re-read by function
  (r2noise.py): toggled probes, every sourced function 0 moved but seal's write_buf_with +8; libc in
  toggled probes moves with addresses (read memcmp +0.83/op = 4.2%, scan_z _int_malloc +2.45/scan =
  0.5%), memcpy 0 except seal +0.01/op. Untoggled, every supdb:: function: clock functions
  (parse_mount +0.98/op load_buffered, read_budget, parse_kb, z_tick), merge's
  Reader::sweep_retired_states +2.0/op (walks the reader table while a background pin holds), the
  rest at most 0.0075/op (load_durable join_seal).
- cost-diff.py rewritten: args BASE HEAD OPS HOT NAME MODE OWN; MODE toggle: every function with a
  source file (not ./, /usr/, ???) hot at TOGGLE_SLACK 64 instructions over the run; libc (no
  source) red past LIB_SLACK 2/op and LIB_REL 5% both. MODE whole: every function of OWN by name and
  a bare name with a workspace-relative file hot at WHOLE_SLACK 0.01/op. `~` lines out of whole only
  (the five above). COST_SLACK gone (one definition each, in cost-diff.py).
- cost.sh: mode/own per probe; BASE checked to name a commit (rev-parse) and every step that relied
  on `set -e` (worktree add, mkdir, cp) checked by its own status.
- cost-red: step A rewritten (fixtures a11/a12 whole slack, a19/a20/a12t toggle slack, a21 libc,
  a22 std, a24 new fn, a25 `~`, a26 no_mangle, a27 mode refused); step K (new per-key helper from
  commit_direct, load_durable, RED); S's check reads the toggle line.
- tests/source.rs: trait impls' fns (Type::Trait::fn) and item-position macros (Type::name!) are
  entry points; cost-entries gains Reader::Drop::drop, Db::Drop::drop, Db::Deref::deref,
  Db::DerefMut::deref_mut.
- Re-compared offline: run2 (self) green in all 11 with the new rules (recmp.sh); run3 (vs upstream
  9bfc4362, still the merge-base after fetching the branch at 18:15) RED, see "run3 by the new rules".
- before-round2.log: the round-1 comparator (keep2) green over a 1/64-an-op toggled gain, a new
  per-key helper with no toggle, a no_mangle helper and a +50% memcpy; the tree's red on all four.
- 18:40 keep3/ written with sums. Engine mutants: python3 $S/emutants2.py, results
  $S/emutants2.results, logs emutant2-*.log, listings em2/<name>/out (round-1 comparator run on them).
- 19:07 final: COST_BASE=HEAD check.sh cost (cost-red A G V S B U K T C + cost.sh HEAD all 11), log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/after.cost3.log
- 19:05 emutants2 EXIT 0 (E9 E12 E10 E11 E14 E15 red; E11 was inlined by LTO, E11b keeps it out of line:
  red on the bare name). E15/E9 runs named merge-probe noise not in run2: Readers::oldest_for +-400k,
  WriterOp::drop +-75k, Maint::collect/sweep_spares ~0.01/op: `~` lines added, `~probe: path` scoped form
  for WriterOp::drop (merge only), fixture a28. Source mutants X11/X12 red in the tree (db.rs restored, sum
  checked). X13 bad ref: round-1 exit 128 (git), now exit 2 with the ref named; step G holds it.
  mutants3 (mine) 13/13.
- 19:20-20:15 after.cost3.log: COST_BASE=HEAD check.sh cost EXIT 0 (see after-round2.log). Listings kept in
  run4-out/ (.out removed). rt and cw removed.
- lint7 green (fmt reflowed tests/source.rs), source4 69 ok. keep3/ resummed.

## run3 by the new rules (the tree against upstream 9bfc4362)
RED, as before, and the new reds all read by callers and callees: toggled probes -- the moves already
explained (leave -> leave_looked, read_all -> read_all_in, read_exts -> each_value/read_exts_inline, flat into
scan_seek, zpages inlining) now each red alone since the slack is per run; seal: std::fs::File::write_all_at
+672 and OpenOptions::_open +201 against AlignedWriter::write_all -35,986 (the writer writes at the offset it
counts, pwrite; the helper opens its own description once a segment), write_buf_with +134. Untoggled:
Db::op/WriterOp::drop against Reader::enter/leave (WRITER_OP depth), pwritev_all against writev_all (a
rename), Supdb::with_policy +8,289 once (the suite's open, each side's own suite), Wal::replay_frames
+17.4M against Wal::replay -19.2M (the log parsed through a mapping), sweep_retired_states +3.05M against
leave -13.65M and Readers::oldest_for -0.4M, WriterHelper::run +39,862 (0 at the base, which inlined it; once
a segment), vault::names/locate/sync_dir_seamed, manifest_encode_with, Db::name_field, Db::seal (once a file
or a publish: the door), merge's key_at/values_at_pieces/compact_run/exts_at/whole_key_at/Emitter::enter/
seal_table closure/Db::append against flat/merge_ranks closure/values_at/merge_keys (the inlining run1
explained). scan_at_probe::scans +236 a scan stays the one real cost (task #17).

## Where it stands after round 2 (the tree's copy of each file is the finished one; keep3/ with sums)
Round-2 changes: scripts/cost-diff.py (modes toggle/whole; TOGGLE_SLACK 64 a run, WHOLE_SLACK 0.01 an op,
LIB_SLACK 2 an op with LIB_REL 5%; every sourced function hot under a toggle, every engine function by name
and every bare-named workspace function with no toggle; `~` and `~probe: path`), scripts/cost.sh (mode and
own per probe, COST_SLACK gone, BASE checked, the set -e steps checked), scripts/cost-hot.txt (header,
`~` lines), scripts/cost-red.sh (A rewritten, K, G's bad ref, S's check), scripts/check.sh (the threshold
as it is), tests/source.rs and tests/data/cost-entries.txt (trait fns and item macros), CLAUDE.md (the
slack paragraph: what the second review taught, the numbers the untoggled probes cannot see).
Departures this round:
13. The libc rule is a share and a floor (2 an op and 5% of the function's count), not a zero: libc's
    counts follow the addresses they are handed (memcmp +0.83 a read against itself), so a pure libc gain
    under both is not seen; no engine copy on the raw point read or scan exists to mutate (the engine
    lends), and the one through the form (fence_word) could not be lengthened in the probe's keys.
14. In the untoggled probes the noise was taken from two self-checks (run2 recompared, after.cost3), not
    one whole run reading zero: zero is not what they read (the segment work's loop polls on its clock),
    and the functions that move past a two-hundredth are `~` lines, nine of them, one scoped to the merge.
15. Fixed costs over a thousand instructions in one function of a store's open or close read red in the
    untoggled probes now (run3: the suite's open +8,289). That is the reviewers' rule applied, and a run
    against a base whose open changed reads red by design; the reader reads such a line by its callers.
16. Trait methods are keyed Type::Trait::fn and item macros Type::name!.
Not verified (added): the untoggled noise past two samples (a flake there is a `~` line to add, read off
the run that flaked); the CI job's noise on a hosted runner at the new slacks; the reviewers' prediction
for E9 and E15 (they were red under the round-1 gate already: the mutants as written change the loops
they sit in).
Under a cipher: nothing; no engine code changed this round (git diff --stat -- src empty).

## Repair round 3 (started 19:56, uptime 7:25)
Reviewers: (1,2) `~` lines take functions out of the untoggled probes unbounded, three of them run in no
toggled probe, and `~Maint::` is the whole type on evidence for two methods; (3) CLAUDE.md's slack numbers
and "exact under a toggle" are wrong. Ten extra mutants.
- Noise data (r3/noise.py over run4-out and em2/*/out bases, 2-4 samples a probe): parse_mount 0.49/op,
  read_budget 0.44, z_tick 0.04, parse_kb 0.02, Maint::collect 0.0116, sweep_spares 0.0097, z_step 0.0075,
  join_seal 0.0067; merge: sweep_retired_states 11.6/op (2.3M), oldest_for 2.0, WriterOp::drop 0.375; every
  other Maint method <= 0.0032/op; sweep_retired_states/oldest_for in the loads/work <= 0.0037/op.
- Plan: `~` lines carry a bound an op (3x the measured worst, the max of the lines that apply); Maint:: narrowed
  to the methods measured past 0.005; sweep/oldest_for/WriterOp::drop `~` in the merge alone. New toggled
  probes: land (land_probe.rs: segment work and upkeep inline, toggle on the probe's rounds, holds Maint:: and
  the writer's sweep), commit_durable/commit_buffered (load_only, toggle on Supdb's write_batch: the writer's
  appends and commit exact, a per-piece cost seen), scan_long_z (supdb-nocachezpages, len 2000). seal probe 4
  segments of 25,000 (a per-segment cost x4).
- 20:13 run5: cost.sh 9bfc4362 (15 probes, round-3 rules), COST_DIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3/cw5 COST_BASE_TARGET=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/rt; log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run5.log; listings kept to /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run5-out
- 20:05-20:12 land_probe built (bench/examples/land_probe.rs). Self-noise under its toggle (r3/land*): first shape
  (seal not settled before the scan, platform read) moved read_budget, join_seal, the clock's std fns; a stood
  injected clock made the landings fall elsewhere (lower_bound/splice_run by millions): dropped. Shipped shape:
  budget and refaults through ZInject, settle after each seal: 4 runs, only join_seal +332 and Timespec::now +64.
  commit_durable/commit_buffered (load_only, toggle on Supdb's write_batch), 3 runs each: landed +14/+63, the rest 0.
  seal 25000x4: 3 runs, 0. scan_long_z: 2 runs, 0.
- cost-diff.py: `~` lines carry a bound an op (refused without), the largest that applies, unscoped in whole mode,
  scoped in the named probes whatever their mode; the bound replaces the slack. cost-hot.txt `~` section rewritten.
  recmp.sh over run4-out green in all 11; E15/E9/E14 HEAD bases against run4 green.
- cost-red A: fixtures for bound red/green, larger of two, refused, scoped in toggle. A ok.
- tests/source.rs: entry_aliases (a type alias of an entry type, chains), blanket trait impls named *::Trait::fn;
  found *::Aead::{encrypt,decrypt} (impl<A: Aead> Aead for &A), listed. cost-entries maps the new probes.
- keep4/ with sums (13 files). mutants4.py P1-P6 killed (cost-red A). Q1-Q5 (source) wait for run5 (one cargo).
- emutants3.py written (F1,F2,F5-F10, F8b), runs after run5.
- 20:25 run5 (cost.sh 9bfc4362, the merge-base still, after fetching the branch at 20:12) EXIT 1, RED. Old
  probes: every gain the one run3 explained (below, "run3 by the new rules"). New probes:
  scan_long_z: view_with/open_at/ask_group_ahead/parse_page the zpages inlining scan_z reads, x the 20 groups
  of a 2000-entry scan, beside walk_impl's loss (total -11,500 a scan); unlink_chunk +5.3 a scan (+42%) and
  _int_malloc +36 a scan (+3%): the allocator, read again in the self-check.
  commit_durable/commit_buffered: append_batch -7.0 an op, commit_direct -1.0; gains: Db::op/WriterOp::drop against
  Reader::enter (WRITER_OP), pwritev_all and its IoSlice Vec::from_iter (111 calls) against writev_all (the
  run's writer writes at the offset it counts), sync_run/stream_flush/SegmentWriter::create a few hundred
  over the run (the durability fixes' checks, once a run or a sync). Net -8.2 an op over the hot functions.
  land: total +277 an op, CachedBlock::lower_bound::{closure#0} +19.3M and splice_run +8.1M (inclusive
  splice_run 17.3M -> 45.9M over the same 75,044 calls from patch_block): splice_run's source differs in one
  test, `blk.identity` -> `blk.identity_of(table.blob)` (324a395a, the fix CLAUDE.md records as "A claim about
  a partition's records carried across its rewrite"): the base took a copy carried across a merge's rewrite
  for its block's records and patched by rank, the wrong-answer shape; the head searches. A correctness fix's
  price on a store with merges, no cost of the cipher's. The rest are moves (enter/leave -> Db::op/WriterOp,
  flat -> key_at/scan_whole, values_at -> values_at_pieces, emit_over inlined).
  Listings kept in run5-out/.
- 20:27 run6: self-check, cost.sh HEAD (15 probes), COST_DIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3/cw6 COST_BASE_TARGET=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/rt; log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/run6.log, listings run6-out
- 20:33 run6 EXIT 0 green (15 probes, self-check). Now mutants4 Q1-Q5 then emutants3.
- 20:40 source: X14/X15 (blanket, alias) green under keep3 (r3/before-source.log), red now; Q4/Q5 survived: the alias filter and the tr.is_some() test were checks nothing reaches, taken out; Q1-Q3 killed on the new code. emutants3 started: log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/emutants3.run.log, results /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/emutants3.results
- 21:42 emutants3 done: F1 F2 F5 F6 F7 F8 F8b F10c killed; F9 red in merge (two opens, 2,428 > 2,000), green in work; F10 (at == 0) never ran in the seal (+16, layout); F10b 11 a segment green (under 16 a segment); F10c 41 a segment red. Now lint.
- 21:43 lint, bench lint green (r3/lint1.log, r3/benchlint.log). Final: COST_BASE=HEAD check.sh cost, log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/after.cost4.log

## Where it stands after round 3 (the tree's copy of each file is the finished one; keep4/ with sums)
Round-3 changes: scripts/cost-diff.py (`~path BOUND`: a bound an op in place of the slack, the largest that
applies, unscoped in the probes with no toggle, scoped in the named probes whatever their mode; a `~` line
with no bound refused; the docstring's toggle claim as measured), scripts/cost-hot.txt (the `~` section as
bounds, Maint:: narrowed to collect/sweep_spares/z_step/join_seal/z_tick, the sweep/oldest_for/WriterOp::drop
in the merge alone, land's and the commit probes' waits; the header's numbers), scripts/cost.sh (probes
scan_long_z, commit_durable, commit_buffered, land; seal at 25000x4; the suite guard holds write_batch),
scripts/cost-red.sh (A: the bound red and green, the larger of two, refused without, scoped in a toggle),
scripts/check.sh (the cost group's text names the bounds), bench/examples/land_probe.rs (new),
bench/examples/seal_probe.rs (doc), tests/source.rs (type aliases resolved, blanket trait impls `*::`),
tests/data/cost-entries.txt (*::Aead::*, the new probes), CLAUDE.md (the slack paragraph corrected: the
toggle's exactness as measured, the three slacks as numbers, the rule the third review taught).
Departures this round:
17. The reviewers offered a bound on `~` lines or a toggled probe for the functions they took out; both are
    built: bounded lines, and `land`, a toggled probe of the segment work on the writer's thread. The
    probe's store is inline (publish_in_background off, Upkeep::Inline), not the shipped background shape:
    under a toggle only the toggled thread is collected, and in the shipped shape the segment work runs on
    a thread no toggle of the writer's reaches. Its platform readings go through ZInject (a budget of 1 TiB,
    no refaults), since the writer's settle room reads the cgroup on a period.
18. Commit probes added (the toggle on Supdb's write_batch): not asked for by name; they are what gives the
    loads' per-commit and per-piece steps a slack of 64 over the run in place of 1,000.
19. The seal probe writes four segments of 25,000 in place of one of 100,000, so a cost a segment is
    four times its count.
20. tests/source.rs: two checks the mutants found unreachable were taken out (the alias filter's entry-type
    test, the blanket rule's trait test).
Not verified (added): the bounds rest on two to four self-check samples a probe; a flake past one is a
bound to raise, read off the run that flaked. land's shape (inline segment work) is not the shipped one, so a
cost only the background thread's own loop pays is held by the bounded whole probes alone. The CI job now
runs fifteen probes, its time on a hosted runner not measured (timeout 150 min).
Under a cipher: nothing; no engine code changed (git diff --stat -- src empty).
- 22:22 after.cost4: cost-red A G V S B U K T C ok; the group's cost.sh HEAD then failed at the head build, No space left on device (bench/target + rt). Freed rt/release/examples (3.2G); cost.sh HEAD again alone, log /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3/after.cost5.log
- 22:29 after.cost5 EXIT 0 green (15 probes). Removed rt and tmp/G1-3. keep4 sums clean; src unchanged. Not committed.
