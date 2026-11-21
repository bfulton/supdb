# T17 mutants

Runner: mutants.py (each an edit of one finished file in keep/, by its sha256 in keep.sha256;
the verdict written to mutants.results as it comes; the file put back and checked by its sum).
Logs: mutant-<name>.log. Tests: `cargo test --profile checked --no-fail-fast` over the targets
named in the runner.

| mutant | what it does | verdict |
|---|---|---|
| M1_claim_taken_without_proof | 324a395a taken out at the patch: a carried copy's rank taken under any claim, no compare | KILLED: all three claim tests (debug assertion "a copy's identity entry disagrees with its search") |
| M2_compare_inverted | the patch's compare inverted | KILLED: the three claim tests |
| M3_search_fallback_dropped | the search where the compare fails replaced by the rank | KILLED: claim tests, a_settles_seeks_planned_ahead... |
| M4_scan_start_claim_without_proof | (first shape) the scan start's `starts_at` proof replaced by true | SURVIVED: the proof was taken out (notes) |
| M13_starts_at_takes_an_equal_key_as_below | (first shape) `starts_at` bound off by one | SURVIVED: taken out with M4 |
| M4b_scan_start_takes_a_stale_claim | 324a395a taken out at the scan's start (pre-existing line) | SURVIVED: equivalent in every state the notes find reachable; not this change's line; reported |
| M5_scan_piecewise_refusal_dropped | the whole walk's refusal of a source by the piece dropped (pre-existing) | SURVIVED in tests/blob.rs, tests/cipher.rs and the lib: the record walk answers what the windowed walk does. A unit test killed it (blob::tests, `ByPiece`), and was taken out again: it names `piecewise` in src/blob.rs's test region, a couplings raise, and scripts/ratchet-red.sh's l_deregistered case goes red under any real raise of tests/data/couplings.txt (lists.log). Reported |
| M6_seek_pieces_answers_the_end | `seek_pieces` answers the key count | KILLED: a_source_that_lends_by_the_piece_answers_the_same |
| M7_rank_not_moved_by_the_fast_walk | walk_fixed!'s rank not moved at its end | KILLED: a_scan_hands_out_the_bytes_read_all_does, scanning_walks_the_dictionary_... |
| M8_search_reads_the_next_word | `FlatIndex::search` reads the directory word after its probe | KILLED: the same two |
| M9_search_unresolved_answers_hi | the search's unresolved-regions return gives `hi` | SURVIVED: equivalent on a path `FlatIndex::parse` makes unreachable (every region inside the section); two shapes without the branch counted red (notes, s7) |
| M10_planned_heads_le | the planned seek's heads compared `<=` | KILLED: ordindex unit tests, settle tests |
| M11_bytes_kept_on_an_appended_run | the form's bytes not counted again after an appended run | KILLED: five tests (debug assertion of the kept bytes, and budgets) |
| M12_bytes_kept_ignores_clone | bytes kept where the patch cloned a shared copy | KILLED: a_burst_lent_to_the_upkeep_thread_is_filed_before_the_first_scan |
| M14_key_eq_second_word_unread | `key_eq` reads one word | SURVIVED first; KILLED after the claim test's 16-byte keys and its early record |
| M15_key_eq_tail_unread | `key_eq` reads no tail | SURVIVED first; KILLED after the claim test's 19-byte keys |
| M16_plan_seek_empty_top_taken_late | `plan_seek` takes the late top level for a mapped file | SURVIVED first; KILLED by a_mapped_index_plans_from_the_top_level_read_at_its_open |

The brief's list: "the fix of 324a395a taken out" is M1 (and M4b at the scan's start); "the new
check taken out or inverted" M1/M2; "the search's fallback dropped" M3; "the scan's piecewise
refusal dropped" M5 (survivor, above). Own: M6-M16.

## Repair round 1 (runner mutants_r1.py over keep-r1/, verdicts mutants-r1.results, logs mutant-R1_*.log)

| mutant | what it does | verdict |
|---|---|---|
| R1_M5_refusal_dropped | M5 again: `scan_at_in`'s refusal of a source by the piece dropped | KILLED: tests/blob.rs `the_whole_walks_refuse_a_source_that_lends_by_the_piece` (new: `scan_at_paced` and `run_walk().scan_at` over the counting source that lends by the piece refuse with `Unsupported` and hand nothing out; `scan_at` walks it). M5's survivor row above is closed by it |
| R1_X1_planned_paged_arm_never | the planned seek's by-the-piece arm never taken (the reviewers' `hi <= lim -> true`, on this round's shape `lim == 0 -> false`) | KILLED: ordindex `an_index_read_by_the_piece_seeks_as_the_mapped_one` (the empty held heads' slice panics) |
| R1_X2_key_eq_no_length_guard | `key_eq` without its length test | KILLED: the 16-byte claim test (the folded key, the 16-byte key at its rank plus `x`) |
| R1_X3_seek_pieces_none_answers_zero | `seek_pieces` answers 0 for a reader with no section whole or by the piece | KILLED: tests/blob.rs `a_seek_over_a_section_that_does_not_read_answers_the_end` (new: a source that stops lending after the open) |
| R1_X4_search_no_rank_guard | `FlatIndex::search` without `mid < nkeys` | SURVIVED, equivalent: both callers (`seek_with`) search below the key count, and the directory region bounds the rest. Taken out, the scan_at probe read 282 instructions a scan worse (R1d, notes): the test stays, and is a check nothing reaches false |
| R1_X5_bytes_kept_both_halves | in place `bytes_kept = true` and the found-append branch sets `bytes_kept = !cloned` | KILLED: five settle/db tests (budget, the kept bytes' assertion) |
| R1_X6_key_eq_seventeen_unread | `a.len() == 16` -> `a.len() <= 17` | KILLED: new 17-byte claim test (four keys a 16-byte prefix, the 17th byte deciding) |
| R1_X7_rank_taken_on_full_blocks | the rank taken unproven on a copy of a whole block | SURVIVED first (every claim test's copy under 64 entries); KILLED by the new `a_copys_claim_across_its_partitions_rewrite_in_a_full_block` |
| R1_X8_planned_always_by_piece | every planned seek through the by-the-piece arm (`lim == 0 -> true`; the reviewers' `hi < lim`) | SURVIVED the tests, as expected (equivalent answers): the cost gate is its test, see the cost row below |
| R1_X9_bytes_kept_after_the_match | `bytes_kept = !cloned` set after the match on every path | KILLED: many settle/db tests |
| R1_O1_plan_late_never | `plan_seek` never takes `plan_seek_late` | KILLED: `an_index_read_by_the_piece_seeks_as_the_mapped_one` |
| R1_O2_plan_late_empty_index_plans | `plan_seek_late` without its `n == 0` test | SURVIVED first (an empty index's plan of an empty window answers as no plan); KILLED by new ordindex `an_index_of_no_keys_plans_nothing` (the doc's `None` for an empty index) |
| R1_O3_held_tie_never | the held heads' tie arm never taken | KILLED: ordindex planned tests, settle |
| R1_O4_unchecked_probe_le | the unchecked probe `<` -> `<=` | KILLED: ordindex planned tests, settle |
| R1_O5_stride_cut_past_hi | the stride cut to the heads' end, not the plan's `hi` | SURVIVED the tests: equivalent answers (the sample at `hi` is not below the query, so the lower bound is the same), more probes: the cost gate is its test, see below |
| R1_O6_key_at_pieces_none | `key_at_pieces` answers None | KILLED: `a_source_that_lends_by_the_piece_answers_the_same`; WITHDRAWN with its line: `key_at_pieces` (R1b) was taken back out (R1e, notes), so the line is not in the tree |
| R1_O7_tie_by_piece_untied | `exact_at` never calls `exact_tied` | KILLED: ordindex tests, settle, cipher |
| cost: R1_X8 under `cost.sh 9bfc4362 land` | as R1_X8 | the gate names it: `HOT GAINED: OrdIndex::seek_planned_by_piece +146.958 an op` (settle_each -130.149, net +16.8 a key); $U/r1/cost-R1_X8_planned_always_by_piece.log. The probe reads `cost: RED` on the tree already (the moves of step 3), so the verdict is the named line, not the exit |
| cost: R1_O5 under `cost.sh 9bfc4362 land` | as R1_O5 | the gate names it: `HOT GAINED: Reader::settle_each +105.628 an op`, land +61.77 an op; $U/r1/cost-R1_O5_stride_cut_past_hi.log |

The reviewers' list, by this round's shape: `hi <= self.lim -> true` is R1_X1 (`lim == 0 -> false`,
the arm the condition chose); `hi <= lim -> hi < lim` is R1_X8 (every plan through the by-the-piece
arm: answers equal, the gate names it); `key_eq`'s length guard R1_X2, `a.len() <= 17` R1_X6;
`seek_pieces` `None => 0` R1_X3 (listed twice by the reviewers, run once); `FlatIndex::search`'s
guard R1_X4 (no caller passes `hi = nkeys + 1`: `seek_with` passes the fence window cut at the key
count, or `0..nkeys`); the two `bytes_kept` halves together R1_X5 and moved after the match R1_X9;
the rank on full blocks R1_X7; M5 again R1_M5. Own: R1_O1..R1_O7 (O6 withdrawn).
Not re-run this round: M1-M3 (324a395a taken out, the compare inverted, the search fallback
dropped), whose lines are unchanged since they were killed; M10 and M16, whose lines this round
rewrote, are replaced by R1_O4 and R1_O1.

## Repair round 2 (runner mutants_r2.py over keep-r2/, verdicts mutants-r2.results, logs mutant-R2_*.log)

| mutant | what it does | verdict |
|---|---|---|
| R2_A_rank_by_cumulative_seen | walk_fixed!: `$rank += $seen - walked_from` -> `$rank += $seen` | KILLED in tests/cipher.rs (14 tests); tests/blob.rs passed it (mixed-length records never reach a second window) |
| R2_B_window_rank_from_first_window | scan_pieces: the rank moved by the cumulative count each window (`rank += before` after the walk) | KILLED in tests/cipher.rs; tests/blob.rs passed it, as R2_A |
| R2_A2 / R2_B2 | R2_A / R2_B held to tests/blob.rs alone, after the new test | KILLED: `a_walk_by_the_piece_across_its_windows_answers_as_the_mapped_one` |
| R2_C_own_claim_inverted | `blk.identity == table.blob ||` -> `!=` | KILLED: five claim tests, by the debug assertion "a copy's identity entry disagrees with its search" alone |
| R2_D_tie_reads_head_before | `heads.get(lo)` -> `heads.get(lo.saturating_sub(1))` | KILLED: ordindex planned-seek tests, settle tests |
| R2_E_plan_late_by_n | plan_seek: `top_eager.is_empty()` -> `self.n == 0` | KILLED: `an_index_read_by_the_piece_seeks_as_the_mapped_one` |
| R2_F_stride_from_base | `heads[base.min(hi)..hi]` -> `heads[base..hi.max(base)]` | SURVIVED, equivalent (no plan has lo > hi). The order is asserted now (debug_assert, red-first test `a_plan_out_of_order_or_past_the_heads_is_refused_in_a_checked_build`); R2_F2, the mutant under the assertion, survives as equivalent under it |
| R2_O1_plan_order_unasserted | the new assertion `true` | KILLED: the new test |
| R2_O2_plan_order_half_unasserted | the assertion without `lo <= hi` | KILLED: the new test |
| R2_O3_plan_end_unasserted | the assertion without `hi <= heads.len()` | KILLED: the new test (its plan past the heads) |
| R2_O4_assert_too_strict | `lo <= hi` -> `lo < hi` | KILLED: ordindex planned tests and settle tests (empty strides are real plans) |
| R2_G_claim_and_compare | `||` -> `&&` at the patch | SURVIVED the tests (equal answers), as expected; the cost comparator names it: HOT GAINED `CachedBlock::lower_bound::{closure#0}` +193.378 an op, `Reader::splice_run` +63.744 (r2/cost-R2_G_claim_and_compare.log) |
| R2_H_bytes_kept_in_place_always | `bytes_kept = !cloned` -> `true` in place only | KILLED: the debug assertion "a run written in place moved its form's bytes" (a cloned in-place patch under the capacity count, a_burst_lent_to_the_upkeep_thread_is_filed_before_the_first_scan) and a budget test |
| R2_I_search_unresolved_answers_hi | round 0's M9 again | SURVIVED, equivalent (FlatIndex::parse holds the regions inside the section; the check has not moved) |
| M4b under the fuzz | the scan start's claim of any partition, ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1, plain 0..2500 x5, cipher 0..450 x3 | SURVIVED: 1808 scan starts over a stale claim reached, every one at the block's first entry, where the rank and the search agree; the line is 324a395a's (r2/fuzz-M4b-*.log, r2/m4b-stale-starts.txt) |
