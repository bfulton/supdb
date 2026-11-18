# helper-race mutants

Runner: mut.py (each mutant written into src/db.rs from keep/src_db.rs.fixed, `cargo test --profile checked
--no-fail-fast --test durable -- helper`, verdict by exit status, src/db.rs put back and checked by its sha256
after each). Verdicts in mut-results.txt, logs mut-<name>.log. M1-M2 ran before the 06:05 restart, M3-M9 after it.
Tree checked = keep/src_db.rs.fixed (477ce24d...) after the last.

| mutant | change | verdict | killed by |
|---|---|---|---|
| M1 helper-dup-not-reopen | helper_description returns `self.file.try_clone()` (the shape before: one description) | KILLED | a_marked_segments_sync_beside_its_helpers_failed_sync_answers_it, a_run_commit_beside_a_helper_sync_that_met_an_error_answers_it(_under_a_cipher) |
| M2 join-helper-error-dropped (the brief's) | join_helper: `h.finish().unwrap_or(None)` | KILLED | a_seal_whose_helper_sync_failed_is_named_by_no_manifest, a_merge_whose_helper_sync_failed_lands_nothing |
| M3 finish-ignores-helper-error | WriterHelper::finish drops the stored error | KILLED | the seal and merge tests (the merge's at its 120 s bound in ss_commit_answer) |
| M4 no-fallback-error-propagates | helper's open failure returned as the write's error | KILLED | a_writer_whose_helper_cannot_open_its_file_writes_without_one |
| M5 fallback-is-dup | open failure falls back to a duplicate of the writer's descriptor | KILLED | a_writer_whose_helper_cannot_open_its_file_writes_without_one |
| M6 fallback-not-done-retries | open failure does not set helper_done (asked again every piece) | KILLED | a_writer_whose_helper_cannot_open_its_file_writes_without_one (refused() == 1) |
| M7 fd-digits-unreversed | /proc/self/fd path digits in reverse order | KILLED | ..._under_a_cipher (the only test whose file's fd has two digits) |
| M8 run-open-assigns-fault | Db's run open assigns the fault byte instead of OR-ing (seam delivery) | KILLED | both store-level run commit tests |
| M9 reopen-path-off-by-one | digits written one byte early, over the path's last '/' | KILLED | every helper test (10 of 11) |

No survivors. M7 is held by one test only, by the fd number the cipher test's run happens to get; a fd of two
digits in the plain tests is not built on purpose.

## Repair round 1 (the reviewers' extra mutants)

Runner: mut2.py (same discipline as mut.py: written into src/db.rs from keep/src_db.rs.fixed, `cargo test
--profile checked --no-fail-fast --test durable -- helper` plus the lib's streaming_writes tests, verdict by exit
status, src/db.rs put back and checked by sum). Results: mut2-results.txt (first pass, before the round's new
tests), mut3-results.txt (E4 again; its E5/E7 were cut by an edit and rerun), mut4-results.txt (after the tests).
The reviewers' ten items are eight distinct mutants (E1 and E5 were each listed twice; the three-digit M7 rerun is E3).

| mutant | change | first pass | final | killed by / disposition |
|---|---|---|---|---|
| E1 reopen-wronly | helper's reopen O_WRONLY for O_RDONLY | KILLED | KILLED | db::streaming_writes::a_long_segment_is_synced_behind_its_writer (asserts the helper hashed), a_helpers_description_is_its_own_at_every_width_of_the_writers_descriptor (asserts the helper's description reads the file) |
| E2 reopen-no-cloexec | O_CLOEXEC dropped | KILLED | KILLED | a_helpers_description_is_its_own_at_every_width... (asserts FD_CLOEXEC on the helper's descriptor) |
| E3 M7 at widths | path digits unreversed, plain tests with writer fds of 2, 3 and 4 digits (none a palindrome) | KILLED | KILLED | the width test and five plain helper tests (it no longer rests on the cipher test's fd) |
| E4 same-description-inverted | HelperFault::reaches answers the same description | KILLED | KILLED | the width test, a_marked_segments_sync_beside..., both run-commit tests (plain and cipher) |
| E5 sync-at-stop-false | an armed helper takes no sync once stop is asked | SURVIVED x3 | KILLED x3 | a_finish_answers_a_helper_sync_taken_at_the_stop (new; seam HelperFault::start_after_stop starts the helper only after the stop). Rep 0 of the final pass also failed the seal test: without the seam the seal test reaches its failure by timing in 5 runs of 6, so the stop's sync is held by the new test, not by the seal/merge tests |
| E6 told-other-dropped | reaches reports to every other-description sync, not once | SURVIVED | removed | unreachable: the first reported error sets the writer's lost state (RUN_ATTN_LOST) and sync_attended answers the kept error before it asks the seam again, so a second ask never happens; `told_other` taken out of HelperFault |
| E7 fallback-after-own-on | the no-helper fallback taken after own_on/hands_over changed | SURVIVED | KILLED | db::streaming_writes::a_writer_with_no_helper_keeps_its_own_row (new lib test: the writer with no helper still writes its checksum row; the Blob open did not catch it because a segment over sync_ahead with no row still opens) |
| E8 seamed-thread-spawned-plain (mine) | the seamed helper spawned as the unseamed instantiation | -- | KILLED | a_finish_answers_a_helper_sync_taken_at_the_stop |
| E9 start-never-waits (mine) | start_after_stop's wait for the stop removed | -- | KILLED | a_finish_answers_a_helper_sync_taken_at_the_stop |

No survivors in the final tree. E6's check is gone rather than tested.

## Repair round 2 (the reviewers' eight extra mutants)

Runner: mut5.py (same discipline: written into src/db.rs from keep/src_db.rs.fixed, `cargo test --profile
checked --no-fail-fast --lib --test durable -- helper streaming_writes`, verdict by exit status, src/db.rs put
back and checked by sum after each). First pass on the round-1 tree: mut5-results.txt; final pass on the round-2
tree: mut6-results.txt (logs mut6-<name>-0.log). The reviewers' third and eighth items are one mutant (R3).

| mutant | change | first pass | final | killed by / disposition |
|---|---|---|---|---|
| R1 unseamed-helper-dups | the unseamed `spawn::<false>` given `self.file.try_clone()?` (the old shape, production only) | SURVIVED | KILLED | a_helper_with_no_seam_syncs_on_a_description_of_its_own (new: a writer with no seam, marked and unmarked; fds_on + kcmp: the helper's fd is another description, O_RDONLY, CLOEXEC, closed at the finish) |
| R2 wb-bit-only-if-armed-at-create | `out.fault |= RUN_FAULT_WB` only where the seam is armed at create | SURVIVED | KILLED | a_marked_segments_sync_beside_its_helpers_failed_sync_answers_it, restated to run with the seam armed before the writer is made and after (the after case fails). No test created its writer before arming: the merge test's writer is made after `before_second` arms |
| R3 met-after-hold (items 3 and 8) | `met` recorded after the hold | KILLED | KILLED | a_marked_segments_sync_beside..., the width test, both run-commit tests (plain, cipher) |
| R4 reopen-rdwr | reopen O_RDWR | SURVIVED | KILLED | the new no-seam test and the width test, whose assertion is now `accmode == O_RDONLY` (was `!= O_WRONLY`). Held rather than argued: the helper reads and syncs and writes nothing, so a write description is more than it needs, and what ext4 does at the release of a write description is not checked here |
| R5 synced-all-ignores-helper-error | synced_all no longer reads `h.shared.error` | KILLED | KILLED | the four fail_helper_syncs tests: a_due_commit_is_not_acknowledged_over_a_failed_helper_sync, a_marked_segments_sync_reports_its_helpers_failed_sync, a_commit_under_always_is_not_acknowledged_over_a_failed_helper_sync, a_sync_over_a_run_whose_helper_sync_failed_answers_it_and_keeps_it (that seam fails no real sync, so the kernel reports nothing to the writer and the word is the only answer) |
| R6 counted-every-sync | `HelperSeams::counted` outside the `if let Err` | SURVIVED | KILLED | db::streaming_writes::an_unarmed_seams_helper_counts_no_failure (new lib test: an unarmed seam's helper makes two successful syncs, held by its flush count, and `failed()` reads 0 before and after the finish) |
| R7 no-wb-bit | `out.fault |= RUN_FAULT_WB` dropped | KILLED | KILLED | both run-commit tests, the width test and a_marked_segments_sync_beside... (as the reviewer expected) |

No survivors in the final tree. Round 2 changed tests only (tests/durable.rs and one test in src/db.rs's
streaming_writes module); no engine code, so no pairing was run again.

## Repair round 3 (the reviewers' seven extra mutants)

Runner: mut7.py (same discipline: written into src/db.rs from keep/src_db.rs.fixed, `cargo test --profile
checked --no-fail-fast --lib --test durable -- helper streaming_writes` under `timeout`, verdict by exit status,
src/db.rs put back and checked by sum after each). First pass on the round-2 tree: mut7-results.txt (N6's first
entry is a compile error of the mutant as first written; its valid first pass is the line after it, TMO=600).
Final pass on the round-3 tree: mut8-results.txt (TMO=900; logs mut7-<name>.log, overwritten by the final pass).
The reviewers' items 4, 5 and 7 are two mutants each (N4a/b, N5a/b, N7a/b).

| mutant | change | first pass | final | killed by / disposition |
|---|---|---|---|---|
| N1 reopen-opath | reopen with O_PATH\|O_CLOEXEC | KILLED | KILLED | a_helper_with_no_seam_syncs_on_a_description_of_its_own (its finish expects Ok; the helper's fdatasync is EBADF), db::streaming_writes::a_long_segment_is_synced_behind_its_writer, an_unarmed_seams_helper_counts_no_failure, the_helpers_row_is_the_re_reads, a run test and the merge test. Also killed by the flags pin below in the final pass |
| N2 fallback-after-row-taken | the reopen asked after `own_row_so_far` took the writer's row, before `own_on` is cleared | SURVIVED | KILLED | a_writer_whose_helper_cannot_open_its_file_writes_without_one, restated to run at a stride of 96 KiB as well as 16 KiB: at 16 KiB the helper is asked for before the key section's first whole piece, so the row taken was empty and dropping it changed nothing (the reason the lib test and the durable test passed it). On the other arms (marked with `row_on_write`, so `helper_hashes` false; or `row_on_write` off, so `own_on` false) `hands_over` is false, nothing is taken, and the mutant is the fixed code by construction |
| N3 sync-seam-assigned | sync_attended `fail = reaches(..)` for `fail \|= ..` | SURVIVED | KILLED | a_sync_failed_by_a_test_beside_a_helper_seam_answers_the_failure (new: a writer with a helper seam unarmed and armed, and a store's run with `SegmentOptions::helper_fault`, each with a test's failed fdatasync armed; the sync and the run's commit must fail). The combination is test seams only; it is held rather than declared unreachable, since a test can make it |
| N4a reopen-nonblock | `\| O_NONBLOCK` | SURVIVED | KILLED | the width test and the no-seam test, whose assertion is now that the helper's fdinfo flags equal those of the same file opened read-only by `std::fs::File::open` (`O_RDONLY\|O_CLOEXEC` and what the kernel adds, O_LARGEFILE), flag for flag |
| N4b reopen-noatime | `\| O_NOATIME` | SURVIVED | KILLED | the same two tests |
| N5a err-when-not-helper-hashes | refused open answered as the write's error where `!helper_hashes` | KILLED | KILLED | a_writer_whose_helper_cannot_open_its_file_writes_without_one (the marked arm) |
| N5b err-when-helper-hashes | the same where `helper_hashes` | KILLED | KILLED | that test (the unmarked arm) and db::streaming_writes::a_writer_with_no_helper_keeps_its_own_row. Both branches of `helper_hashes` are reached with the refusal |
| N6 synced-only-if-ok | `s.synced.fetch_max(landed)` only where the helper's sync returned Ok | KILLED (hang, timeout 124) | KILLED | a_helpers_failed_sync_is_not_tried_again (new: the finish run on a thread and held to returning within 60 s, and to answering the failure). Not more correct: the helper retries a failed range, at once and with no park; under an armed seam it retries at its stop for good, so the finish never joins it (first pass: four tests hung until the runner's timeout, none failed); on a real device a retry on its own description is answered Ok over pages the failed one marked clean, which CLAUDE.md's rule refuses ("a sync that fails is kept and never retried"). No test reads `s.synced` after a failure but the helper's own loop: `synced_all` answers the stored error before it touches `s.synced`. The binary still exits 124 under the mutant, since the other armed tests hang at their finish; the new test is the one that names it |
| N7a run-open-take-before-keys | Db's run open takes `run_fault_open` right after `PieceWriter::create`, before `set_deferred_keys`/`reserve_keys`/`set_marks` | SURVIVED | SURVIVED | equivalent: none of the three writes the file or touches `fault` (`set_deferred_keys` sets a field, `reserve_keys` reserves memory, `set_marks` sets `marks`/`defers`/`helper_hashes`), so a seam taken before them fires at the same first write |
| N7b run-open-assign-keep-wb | `fault = take(..) \| (fault & RUN_FAULT_WB)` | SURVIVED | SURVIVED | equivalent: on a writer the run open keeps, `fault` is 0 or `RUN_FAULT_WB` at that line. `SegmentWriter::create` sets only `RUN_FAULT_WB`; a seam bit is set only by `fail_next_*`/`run_fault`, never before the run is open; and an attention bit (`RUN_ATTN_*`) only by a failed write or sync, of which the only one before the line is the header's `write_all` in `create`, where `defers` is still false, so `AlignedWriter::defer` answers it as `Err` and `create`'s `?` drops the writer. The reviewer's variant that also cleared create-time attention bits clears bits no kept writer can have; no test can arm one, and a check for it would be a check nothing reaches |

Round 3 changed tests only (tests/durable.rs: the flags pin and `open_flags`, the second stride of the no-/proc
test, two new tests) and CLAUDE.md (the fallback's stride). No engine code changed and no reviewer item said a
pairing missed the mechanism, so no pairing was run again.
