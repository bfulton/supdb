# M1 mutants

Runner: mutants.py (each verdict appended to mutants.results as it comes; each file put back and
checked against keep/SHA256SUMS after each mutant; logs mutant-<tag>.log).

| mutant | verdict | killed by |
|---|---|---|
| M1 the identity fix taken out whole (identity_for ignores the id, the patch takes any claim by rank) | KILLED | both sides' tests/settle.rs tests: upstream's `a_copys_identity_is_its_partitions_and_not_the_one_a_rewrite_left` and this branch's five `a_copys_claim_*` (the checked build's assertion beside the patch, src/db.rs "a copy's identity entry disagrees with its search") |
| M2 the patch's compare removed (`identity_of == blob \|\| key_eq` -> true) | KILLED | the same six tests |
| M3 the patch's compare inverted (`!key_eq`) | KILLED | the same six tests |
| M4 `identity_for` ignores the id | SURVIVED the store tests at first (the patch's compare covers the patch, and the walk's start under a stale claim is reached by no test: T17's M4b, pre-existing); KILLED after the new unit test `db::copy_claim::a_copys_claim_answers_for_the_partition_its_build_read_alone` (the predicate the walk's start rests on). First verdict kept as `M4r1-first` in mutants.results |
| M5 copy_block stamps identity_of 0 | KILLED | upstream's settle test (`block_cache_identity`: holds != claimed) |
| M6 head_bracket reads `top_eager` (upstream's field as merged by text) | KILLED | `ordindex::tests::a_heads_bracket_by_the_piece_is_the_mapped_ones` (index out of bounds under a mapping by the piece) |
| M7 the settle stores' name back to the colliding one | KILLED | the settle binary run whole, first of five runs: both colliding tests red |
| M8 copy_entry_at takes its own partition's claim alone (upstream's hint) | SURVIVED: equivalent by results. copy_entry_at aims a prefetch of the value's lines and nothing reads its answer for a result; which of the two shapes stands is a cost question, held by the count. The count chose M8's shape (land settle_each +5.13 -> -9.84 against upstream), so the tree now has it, and the mutant of the tree is the any-claim shape, equivalent by results the same way |
| M9 KEYS off the workflow test's environment list | KILLED | `ci_checks_only_through_check_sh_and_runs_every_group` |
| M10 Reader::piece_bounds off cost-entries | KILLED | `every_entry_point_is_counted_by_a_probe_or_says_why_not` |
| M11 the raise for Reader::block_cache_identity undeclared | KILLED | `sh scripts/check.sh lists` (ratchet: RED) |

Mutants of my own on lines written by hand: M3, M5, M6, M7, M9, M10, M11 (and M4 after the test).

## Repair round 1 (mutants_r1.py, verdicts in mutants-r1.results, logs mutant-r1-<tag>.log;
## the two held by the count by mutcount.py, mutants-r1.counts)

| mutant | verdict | killed by |
|---|---|---|
| X1 splice_run always compares (`identity_of == table.blob \|\|` gone) | SURVIVED the settle tests: equivalent by results, as expected (the compare is a proof, not a filter). HELD BY THE COUNT: land +340,001 over the run, `Reader::splice_run` 12,557,727 -> 12,908,644 (mutcount.py, same build and environment both ways): the short-circuit is the land probe's three and a half an op |
| X2 `key_eq` compares the first sixteen bytes alone | KILLED | `a_copys_claim_across_its_partitions_rewrite_with_keys_of_seventeen_bytes`, `..._past_sixteen_bytes` (tests/settle.rs) |
| X3 `CachedBlock::agrees` back to upstream's `self.identity \|\| !other.identity` | SURVIVED the settle and db tests and the checked fuzz in the clear with `ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1`, seeds 5000..5400 four at a time: no store state the tests make has a converted copy and its build claim two partitions. KILLED (X3b) by a unit test of the predicate added, `db::copy_claim::a_copy_agrees_with_a_build_only_where_it_claims_the_builds_partition`, as M4 was |
| X4 `head_bracket` reads `top()[(rank / TOP_STRIDE).saturating_sub(1)]` | KILLED | `ordindex::tests::a_heads_bracket_holds_the_key_and_nothing_it_does_not_say` (the mapped bracket test, `a_heads_bracket_by_the_piece_is_the_mapped_ones`, holds the two readers to each other and passes, as it should: both readers run the mutant -- it is the cipher's test, not the bracket's) |
| X5 CI_ENV's KEYS value `${{ inputs.arms }}` | KILLED | `ci_checks_only_through_check_sh_and_runs_every_group` (tests/source.rs:5511): the value is held, not the key alone |
| X11 CI_ENV's KEYS value `${{ inputs.key }}` (ci.yml keeps `inputs.keys`) | KILLED | the same |
| X6 the drain loop's `e.to_string().contains(cause)` dropped, engine right | SURVIVED, as it must with a right engine; the assertion's hold is the pair below |
| X6e the engine answers the segment work's failure at a commit with another error (`take_maint_err`'s error replaced) | KILLED | `a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken`, at the loop's assertion (tests/durable.rs:7245) |
| X6eb X6e with the assertion dropped | SURVIVED: so the assertion is the one check of the test that holds the error's identity |
| X7 `identity_of != table.blob \|\| key_eq` (the rank taken without a compare exactly under a stale claim) | KILLED | the five `a_copys_claim_*` settle tests, by the checked assertion beside the patch |
| X8 `head_bracket`'s `\|\| prefix.len() != self.pfx` dropped | KILLED (after the test) | `ordindex::tests::a_heads_bracket_is_refused_under_a_prefix_learned_short`, added: `learn_prefix` takes a key shorter than the common prefix, and the bracket built from that prefix is refused. Before it the two bracket tests learned the whole prefix and could not tell |
| O1 seek_in_exact's held search: `<= h` | KILLED | `ordindex::tests::the_windowed_seek_says_whether_its_rank_is_the_key`, `an_index_read_through_a_copy_of_its_heads_answers_the_same` |
| O2 the held search's window short of its last head | KILLED | the same |
| O3 the writer's read not counted (ModeNote) | KILLED | `a_third_merge_runs_only_in_a_stretch_of_writes_nobody_reads` (tests/fences.rs:1298) |
| O4 `note.quiet.set(quiet)` dropped (`Db::note_read_mode`) | SURVIVED the fences and seal-idle tests: the note's `quiet` is read at one place, the settle's `quiet` under `seal_idle` far from reads, which no test reached (upstream's line has the same hole). KILLED (O4b) by a test added, `a_quiet_stretch_under_the_idle_seals_files_no_backlog` (tests/db.rs): under `seal_idle` and `forms_settle_far`, the segment work inline, a stretch of commits with no read builds no block and the same stretch with a point read before each commit builds some |
| O5 `m.quiet_out.set(false)` dropped (the writer's read clears the stretch once) | SURVIVED (and O5b, against the quiet-stretch test too): equivalent by results, the flag stored false at every read instead of once. HELD BY THE COUNT: scan probe +1,997 over the run, two an op (mutcount.py) |
| O6 with_policy: `supdb-cipherlog` keeps its run | KILLED (after the test) | `engines::tests::the_cipher_arms_differ_from_supdb_cipher_in_their_one_option` (bench/src/engines.rs), added: no test held an arm's options before |

Mutants of this round's own lines: O1-O6 (seek_in_exact, ModeNote, with_policy), X8's and X3's tests.

## Repair round 2 (scripts: r2/bounds-wide.sh, r2/tmut.sh, r2/fmut.sh, mutants_r2.py, mutcount2.py;
## verdicts in r2/*.log, mutants-r2.results, mutants-r2.counts)

| mutant | verdict | killed by |
|---|---|---|
| Extra 1 / 9: the bounds race made wide (`hold_upkeep_pass` in the store with the option on across round 0's commit and the seal), the round-1 test | RED 5/5, the red's exact counts `[(229, 234), (0, 926)]` (r2/bounds-wide-before.log); on upstream's head 7c62153e red 5/5 the same (r2/bounds-wide-upstream.log). The second walker named by a backtrace a walk: not a concurrent IdCache miss, not a second rank_before_publish, but the upkeep's fill of the burst's snapshot, missing in the red store. Fixed test green 5/5 under it |
| BM1 the fix's `upkeep: Inline` taken out, the race wide | KILLED 5/5 (r2/tmut-bm1.log) |
| BM2 the fix's deltas taken out, the fill made to differ (`commit_forms_build: !on`) | KILLED 3/3 (r2/tmut-bm2.log); the fix under the same edit green 3/3 (r2/tmut-filldiff.log) |
| BM3 `before` taken above the commit's fill | KILLED 3/3 (r2/tmut-bm3.log) |
| the stride steer's asks never taken (`noasks`), the round-1 test | RED 3/3 (r2/fmut-before-noasks.log); under load with the asks, 13 of 24 red here and 16 of 24 on upstream's head (r2/convert-load-before.log, convert-load-upstream.log); the fixed test green 3/3 under noasks and 24/24 under load |
| FM1 the settled asks taken out | KILLED 2/2 under noasks |
| FM2 the loop's test inverted | KILLED 2/2 under noasks |
| FM3 the asks not settled | KILLED 2/2 under noasks ("no partition left the stride form in 64 asks, each settled") |
| Extra 2 / 5: `seek_in_exact` `hi <= held.len()` -> `hi < held.len()` | SURVIVES the tests (equivalent by results, as expected). NOT HELD BY THE COUNT EITHER: land probe 476,629,942 -> 476,579,738, `Reader::cut_raw` 2,308,218 -> 2,258,274, the mutant 50,204 instructions (four a call) CHEAPER: windows that end at the last head read cheaper through the head-at-a-time loop in this build. An equivalent mutant that no gate can hold, recorded with its count; the cheaper shape is an engine change and a recount of fifteen probes, not this round's (named for G1-7's count) |
| Extra 6: `heads_held` returns `&[]` always | HELD BY THE COUNT: land +28,090,014 over the run (476,630,462 -> 504,720,476), `cut_raw` +71,785; the rest in the settle's seeks inlined elsewhere. Tests green, as expected |
| Extra 3 (E3a, the literal reading): `m.quiet_out.set(false)` ahead of `if m.quiet_out.get()` | KILLED: `a_third_merge_runs_only_in_a_stretch_of_writes_nobody_reads` (the test is then never true and the writer's read never ends the stretch) |
| E3b (the reviewer's intent, stored at every read: `let was = get(); set(false); if was`) | SURVIVES the tests (equivalent by results); HELD BY THE COUNT: scan probe 13,439,957 -> 13,441,955, two an op, as O5 |
| Extra 4 (E4, E4b): the cipher block moved after the encryption, guarded on `opts.encryption` being `Cipher` | SURVIVES, and is EQUIVALENT: `encryption` is `Cipher` exactly where `cipher && !dir_key`, and no arm sets both (`dir_key` takes the kernel whatever `cipher` says, and no constructor sets the two). The test reads the store's options and its `cipher_counts`, not the arm's name; there is no input that tells the two shapes apart, so no test is owed |
| Extra 7 (E7): the dirkey arm given `Encryption::None` | SURVIVED the bench tests (no test opens `supdb-dirkey` off an encrypting filesystem). KILLED (E7b) by a test added, `engines::tests::the_dirkey_arm_asks_the_kernel_and_the_cipher_arms_a_cipher`, over `encryption_of`, the match pulled out of with_policy |
| E9 (own): the cipher arms' cache bound dropped (`bytes => c.cache_bytes(bytes)` -> `_ => c`) | SURVIVED; KILLED (E9b) once the same test opened `supdb-cipher64` and `supdb-cipher` and held the first's `cache.budget` to 64 MiB |
| Extra 8 (E8): the writer's read counted only on the first after a quiet commit | KILLED: `a_third_merge_runs_only_in_a_stretch_of_writes_nobody_reads` |

Mutants of this round's own lines: BM1-BM3, FM1-FM3, E9; E7b/E9b against the test written for them.

## Repair round 3 (scripts: mutants_r3.py, r3/run.sh, r3/tmut.py; verdicts in mutants-r3.results,
## mutant-r3-*.log, r3/*.log)

| mutant | verdict | killed by |
|---|---|---|
| Extra 3 (B3): `encryption_of` `(true, _)` -> `(true, false)`, `(true, true)` given the cipher | KILLED, by a test failure, not a compile error | the `encryption_of(true, true, 0)` assertion (bench/src/engines.rs, `the_dirkey_arm_asks_the_kernel_and_the_cipher_arms_a_cipher`) |
| Extra 4 (B4): the `cipher_log` line dropped | KILLED | `the_cipher_arms_differ_from_supdb_cipher_in_their_one_option` ("supdb-cipherlog loads through the log") |
| Extra 7: `= opts.direct_ingest && !cipher_log` -> `= !cipher_log` | EQUIVALENT: no Policy sets `direct_ingest`, so the operand was the engine's default, true, for every arm: a check nothing reached. TAKEN OUT: the line is `if cipher_log { opts.direct_ingest = false; }` now. Its nearest mutant there, B7 `if true`, KILLED (supdb-cipher keeps supdb's ordered ingest) |
| Extra 8 (B8): `0 => c` -> `0 => c.cache_bytes(1 << 30)` | SURVIVED against round 2's test (B8b); KILLED once the test held `supdb-cipher`'s budget to that of a store under the same cipher with no bound, opened beside it, within a quarter |
| X1 (own) `if cipher_log` -> `if !cipher_log` | KILLED | the cipher-arms test |
| X2 (own) `direct_ingest = false` -> `= true` | KILLED | the cipher-arms test |
| X3 (own) `0 => c.cache_bytes(64 << 20)` | KILLED | the encryption test's `assert_ne!(arm, 64 << 20)` |
| X4 (own, of the test) the unbounded store opened through `encryption_of` itself | SURVIVED, as a control must: it makes the check compare the arm's path with itself, which is why the test builds its `StoreCipher` directly |
| Extra 1: bounds test, `before` above round 0's commit, Inline kept | GREEN 24/24 eight at once; RED 2/2 when one store's commit is made not to fill (`filldiff`), where the test as it stands is green 2/2. The narrowing stays (a gate on a platform reading, the fills' room, is outside the count) and the test says why |
| Extra 9: bounds test, `before` after `db.seal()` | GREEN 3/3: equivalent by the store's order (`seal()` returns before the segment work's landing walks; the mutant's own near-case assertion holds in every run), not a hole |
| Extras 2/5: `zpages_convert_ask(nth)` -> `(0)` | SURVIVED 3/3 noasks, 12/12 loaded, 6/6 alone; Extra 6: bound `64` -> `1`, SURVIVED 3/3, 12/12, 6/6. Counted: one ask in each of 160 seeds. The turns and the bound were a check nothing reached: TAKEN OUT, replaced by three settled asks of the first partition, held to a move each way (below) |
| FA (own) two asks, not three | KILLED 2/2: "moved it in and out (1, 1) times" |
| FB (own) the asks not settled | KILLED 2/2: (0, 0) |
| FC (own) the check takes only moves all one way (`abs_diff != 3`) | KILLED 2/2: (1, 2) |
| FD (engine) a block file never asked back into the stride form (`if stride \|\| askable` -> `if stride`) | KILLED 2/2: (0, 1) |
| FE (own) the settled asks taken out, under noasks | KILLED 2/2: "no partition left the stride form in seeds 0..4" |
