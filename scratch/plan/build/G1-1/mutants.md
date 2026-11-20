# G1-1 wall: mutants

Runner: mutants.py (each mutant applied to the working tree, `cargo test --profile checked
--test source` run, verdict appended to mutants.results, the file put back from keep/ and
checked by its sha256). Raw verdicts: mutants.results (two passes).

| mutant | of | verdict |
|---|---|---|
| string blanking off: a string's contents left in the flat text | plan | killed: every_file_closes_at_depth_zero, a_string_across_lines_is_blanked_whole, a_char_literal_holding_a_bracket_is_no_bracket |
| string blanking off: only the opening quote left in the flat text | plan (narrow form) | SURVIVED, equivalent: a quote is neither a bracket nor a word character, and the flat text feeds only bracket depth, attributes and keyword words, so no output can differ |
| comments counted | plan | killed: the wall, a_coupling_added_to_a_form_fails_as_growth (the comment case), and three more |
| cfg(all(test, ..)) not read as test | plan | killed: the wall (vault.rs's test module), an_attribute_with_commas_is_one_attribute, only_a_cfg_that_implies_test_is_test, a_test_coupling_is_counted_apart_from_code |
| growth of one passes (growth compared off by one) | plan | killed: a_coupling_added_to_a_form_fails_as_growth (2 > 1), a_test_coupling_is_counted_apart_from_code, the_encryption_side_naming_a_form_fails |
| shrinkage not checked | plan | killed: a_coupling_removed_fails_until_the_file_is_lowered |
| an unlisted file passing | plan | killed: a_file_on_neither_side_fails |
| door verbs counted | own | killed: the wall, the_tokens_count_as_their_patterns_say, and three more |
| a test field does not end at its comma | own | killed: a_test_field_ends_at_its_comma, a_test_field_of_a_literal_in_a_call_ends_at_its_comma |
| a cfg(test) block inside a fn not a test block | own | killed: the wall, a_test_if_let_inside_a_fn_is_a_test_block |
| the seam (bytes.rs) not scanned | own | first form did not compile (array length); second form `(SEAM, \|_\| Vec::new())` killed: the wall and three more |
| only measured keys compared (a listed key gone unseen) | own | killed: a_coupling_removed_fails_until_the_file_is_lowered |
| a stage outside STAGES accepted | own | SURVIVED first pass -> test a_line_of_the_file_the_gate_cannot_read_fails added -> killed |
| a key listed twice accepted | own | SURVIVED first pass -> same test -> killed |
| impl opens anywhere on a line | own | SURVIVED first pass: the one-line `-> impl` case was hidden because a fn on the same line takes the pending opener after it. Case added to an_impl_in_a_return_type_opens_no_impl (a return type on a line of its own) -> killed |
| any(test, x) read as test | own | killed: only_a_cfg_that_implies_test_is_test |
| a listed file gone passes | own | killed: a_file_on_neither_side_fails |
| the depth self-check never fails | own | killed: a_bracket_left_open_fails_the_self_check |

Tree put back: keep/SHA256SUMS checked after each mutant (the runner asserts it) and at the end
(`sha256sum -c`: source.rs OK, lex_mod.rs OK, couplings.txt OK).

## Repair round 1

Runner: mutants_r1.py (same mechanics; verdicts in mutants_r1.results, logs mutants_r1.pass1.log and
mutants_r1.log). The last edit after the runs (a `type Edit` alias for clippy's type_complexity in a
test helper) changes no behaviour.

Reviewers' red inputs (each an in-binary test, red before the change in before_r1.log, and a red.sh
script step, red by exit status in red_after_r1.log):

| input | before | after, killed by |
|---|---|---|
| `pub(crate) use db::vault::kept as keep_named;` in lib.rs + `crate::keep_named(path);` in warm_file | gate exit 0; test FAILED in before_r1.log | a_renamed_re_export_at_the_crate_root_fails (lib.rs vault::kept 1 > 0, item used 9 > 8, zfile alias 1 > 0); red.sh red |
| the same name added inside lib.rs's existing vault re-export braces | (new case) | a_renamed_re_export_at_the_crate_root_fails (item used 9 > 8) |
| src/db/zz.in + `include!("zz.in");` in warm_file | gate exit 0; FAILED before | code_from_outside_a_listed_file_fails (not a .rs file; then include! growth); red.sh red |
| `#[macro_export] macro_rules! kk {..}` in vault.rs + `crate::kk!(path);` | gate exit 0; FAILED before | a_macro_exported_by_the_encryption_side_fails (vault.rs macro 2 > 0); red.sh red |
| `let _ = Held::vault(h).counts();` in Db::open_refusals | gate exit 0; FAILED before | a_vault_call_spelled_as_a_path_fails (db.rs vault/pager method 25 > 24); red.sh red |
| `let _: Option<crate::DirState> = None;` in warm_file | gate exit 0; FAILED before | a_crate_root_alias_named_by_a_form_fails (zfile alias 1 > 0); red.sh red |

Mutants:

| mutant | of | verdict |
|---|---|---|
| sources() does not recurse | reviewer | killed: the wall and ten more ("listed file gone" for every src/db/*.rs) |
| DOOR filter inverted | reviewer | killed: the wall, the_tokens_count_as_their_patterns_say, four more |
| block comments do not nest (`depth = 1`) | reviewer | killed: lex::tests::a_block_comment_nests (new: `/* a /* b */ cipher */ c();`) |
| parse's region check deleted | reviewer | killed: the_file_lines_the_gate_cannot_read_are_named (new) |
| parse `c.len() < 5` | reviewer | killed: the_file_lines_the_gate_cannot_read_are_named (a five-column line) |
| the eight prefixes without a boundary before | reviewer | killed: the_newer_token_patterns_count_as_they_say (`record- respare- ...` count none) |
| the field word without the whitespace skip | reviewer | killed: the_newer_token_patterns_count_as_they_say (`vault (x)`) |
| a non-.rs file under src/ passes | own | killed: code_from_outside_a_listed_file_fails |
| the crate root not measured | own | first form did not compile (array length); `(&[], ..)` killed: the wall and five more |
| no crate-root aliases | own | killed: the wall, a_crate_root_alias_named_by_a_form_fails, four more |
| a `use` of the module alone not counted | own | SURVIVED first pass -> a_use_names_its_items_and_a_bare_module_is_one added -> killed |
| a renamed item counted under both names | own | killed: the wall and four more |
| the macro token gone | own | killed: a_macro_exported_by_the_encryption_side_fails |
| `vault` by its path not a call | own | killed: a_vault_call_spelled_as_a_path_fails, the_newer_token_patterns_count_as_they_say |
| include! not counted | own | killed: code_from_outside_a_listed_file_fails |
| #[path] not counted | own | killed: code_from_outside_a_listed_file_fails |
| the inner `#![path]` form not read | own | SURVIVED first pass -> code_from_outside_is_every_spelling_of_the_two added -> killed |

Tree put back after each (the runner asserts the sum); keep/SHA256SUMS checked at the end: all OK.

## Round 2 (mutants_r2.py -> mutants_r2.results, mutants_r2.log; files restored from keep/r2 by sum after each)

| mutant | killed by |
|---|---|
| rev: crate:: not a path start (super:: alone) | killed by a_coupling_moved_between_functions_passes, a_path_out_is_resolved_against_its_module, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, the_encryption_side_naming_the_engine_fails, the_forms_and_the_cipher_do_not_name_each_other |
| rev: super:: not a path start (crate:: alone) | killed by a_coupling_moved_between_functions_passes, a_path_out_is_resolved_against_its_module, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, the_encryption_side_naming_the_engine_fails, the_forms_and_the_cipher_do_not_name_each_other |
| rev: bytes not among the allowed (bytes counted as outside) | killed by a_coupling_moved_between_functions_passes, a_coupling_removed_fails_until_the_file_is_lowered, a_path_out_is_resolved_against_its_module, a_coupling_added_to_a_form_fails_as_growth, the_encryption_side_naming_the_engine_fails, the_forms_and_the_cipher_do_not_name_each_other |
| rev: bytes counted as an encryption module | killed by a_coupling_moved_between_functions_passes, a_crate_root_alias_named_by_a_form_fails, a_coupling_removed_fails_until_the_file_is_lowered, a_renamed_re_export_at_the_crate_root_fails, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| rev: vault_named without the skip before :: | killed by a_vault_name_may_be_spaced_or_broken_across_lines |
| rev: vault_named without the skip after :: | killed by a_vault_name_may_be_spaced_or_broken_across_lines |
| rev: enc_use_items items on the module word line | killed by a_use_names_its_items_and_a_bare_module_is_one |
| rev: alias matched as a substring | killed by a_crate_root_alias_is_a_whole_word |
| rev: renamed item counted under both names | killed by a_coupling_moved_between_functions_passes, a_coupling_removed_fails_until_the_file_is_lowered, a_use_names_its_items_and_a_bare_module_is_one, a_use_of_a_nested_group_reads_to_its_semicolon, a_renamed_re_export_at_the_crate_root_fails, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| rev: check keyed by (file, token) | killed by a_coupling_removed_fails_until_the_file_is_lowered, a_code_coupling_moved_into_a_test_fails_by_region, a_coupling_added_to_a_form_fails_as_growth, a_crate_root_alias_named_by_a_form_fails, a_macro_exported_by_the_encryption_side_fails, a_test_coupling_is_counted_apart_from_code, a_renamed_re_export_at_the_crate_root_fails, a_vault_call_spelled_as_a_path_fails, code_from_outside_a_listed_file_fails, the_encryption_side_naming_a_form_fails, the_encryption_side_naming_the_engine_fails |
| rev: use statement ends at the first } | killed by a_use_names_its_items_and_a_bare_module_is_one, a_use_of_a_nested_group_reads_to_its_semicolon |
| own: cfg_attr arm gone | killed by code_from_outside_is_every_spelling_of_the_two |
| own: cfg_attr path without its = | killed by code_from_outside_is_every_spelling_of_the_two |
| own: inline modules ignored | killed by a_coupling_moved_between_functions_passes, a_coupling_removed_fails_until_the_file_is_lowered, a_path_out_is_resolved_against_its_module, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| own: self::super not a start | killed by a_path_out_is_resolved_against_its_module |
| own: a glob in a group passes | SURVIVED |
| own: super after a segment not climbed | killed by a_path_out_is_resolved_against_its_module |
| own: extern crate not counted | killed by a_path_out_is_resolved_against_its_module |
| own: use crate as x not counted | killed by a_path_out_is_resolved_against_its_module |
| own: a path ending above the modules passes | killed by a_path_out_is_resolved_against_its_module |
| own: impl generics not excluded (encryption side) | killed by a_coupling_moved_between_functions_passes, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| own: impl own types not excluded (form side) | killed by a_coupling_moved_between_functions_passes, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| own: impl in any position | killed by a_coupling_moved_between_functions_passes, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, an_impl_names_its_self_type, the_encryption_side_naming_the_engine_fails, the_forms_and_the_cipher_do_not_name_each_other |
| own: impl where not a stop | killed by an_impl_names_its_self_type |
| own: impl lifetimes kept | killed by an_impl_names_its_self_type |
| own: impl generic -> closes | killed by an_impl_names_its_self_type |
| own: macro keywords not excluded | killed by a_coupling_moved_between_functions_passes, a_macro_from_outside_is_a_call_the_file_does_not_define, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| own: macro own not excluded | killed by a_macro_from_outside_is_a_call_the_file_does_not_define |
| own: macro without a bracket | killed by a_coupling_moved_between_functions_passes, a_macro_from_outside_is_a_call_the_file_does_not_define, a_coupling_removed_fails_until_the_file_is_lowered, a_coupling_added_to_a_form_fails_as_growth, the_forms_and_the_cipher_do_not_name_each_other, the_encryption_side_naming_the_engine_fails |
| own: include word not counted | killed by code_from_outside_is_every_spelling_of_the_two, code_from_outside_a_listed_file_fails |
| own: flat keeps literals | killed by lex::tests::the_flat_text_blanks_literals_and_keeps_attributes |
| own: macro token gone | killed by the_encryption_side_naming_the_engine_fails |
| own: a glob in a group passes | killed by a_path_out_is_resolved_against_its_module |

33 mutants (11 the reviewers', 22 own; the last, 'extern crate token gone', added with item 4 of the notes and killed by the_crate_named_by_another_name_fails). One survivor on the first pass: 'a glob in a group passes' -- no input had `super::{x, *}`; the unit test a_path_out_is_resolved_against_its_module got `use super::{pager, *};`, and the re-run (last row) killed it. Taken out before the mutants as checks nothing reached: impls()'s search for `for` (the self type is the header's last path whether or not a `for` precedes it), the macro token's `!=` test and outside_at's `==` test (each already excluded by the bracket or the `=` it needs), walk_path's `super` arm after the climb was moved to the loop's top.

Reviewer's notes answered: enc_use_items line_of mutant was killed (the unit test asserts the line of an item after a second module word); alias-as-substring killed by a new test (MyDirState/DirStateX); renamed-item killed in the gate and by a new form-shaped unit test (`use super::vault::kept as k;` -> [k]); (file, token) keying killed by a new test that moves zfile's code vault::kept into a #[cfg(test)] fn (code 0<1, test 1>0); first-} end killed by a new nested-group test (`use db::{x::{a}, vault::kept};`).
| own: extern crate token gone (its red before) | killed by the_crate_named_by_another_name_fails |

## Round 3 (runner mutants_r3.py, results mutants_r3.results, logs mutants_r3.log, mutants_r3_rerun.log)
Each applied to tests/source.rs alone, the binary run, the file put back from keep/r3 by its sum.

| mutant | killed by |
|---|---|
| rev: `use .. as` renames not resolved | a_use_tree_is_read_to_its_leaves, a_seam_type_renamed_or_aliased_by_a_form_fails; red.sh: only the `use .. as WallM` step goes WRONG (red_under_mutant_r3.log), the `type` step stays red by its own mechanism |
| rev: SEAM out of the paths-out scan | a_seam_re_export_of_the_engine_fails; red.sh: only the seam step goes WRONG |
| rev: parse `c.len() < 5` | the_file_lines_the_gate_cannot_read_are_named (it does hand parse a five-column line) |
| rev: path_group `super` arm without `q.pop()` | a_super_inside_a_group_climbs_and_a_super_alias_inside_is_inside (new). Writing it found a defect: a group element's `super` was also read by the outer loop as a path from the module; fixed (`seen`) |
| rev: paths_out `as` guard dropped | the same test: `mod t { use super as v; }` in the vault (rustc accepts `use super as v;`), so the guard is reached and stays |
| rev: form-side impl `!generics.contains` dropped | a_seam_type_renamed_or_aliased_by_a_form_fails (`impl<Hold: Copy> WallProbe for Hold {}` green) |
| rev: STD_MACROS without debug_assert_eq | first run SURVIVED (no encryption file calls it; my test built its calls from the list itself). The test now writes the std macros out as literals and holds the list's length: killed by every_std_macro_is_exempt_until_a_form_defines_one_by_its_name |
| rev: STD_MACROS without matches | the gate over today's tree and five more, and every_std_macro_... |
| rev: STD_MACROS without todo (no encryption file uses it) | first run SURVIVED, same cause; after the fix killed by every_std_macro_... |
| rev: STD_MACROS without thread_local | the gate over today's tree and five more |
| own: STD_MACROS gains a non-std entry | every_std_macro_... (the length) |
| own: shadowed set ignored | a_form_macro_named_like_a_std_one_fails, every_std_macro_... |
| own: shadowed set empty | a_form_macro_named_like_a_std_one_fails |
| own: type aliases not resolved | a_use_tree_is_read_to_its_leaves, a_seam_type_renamed_... |
| own: no fixed point (one pass) | a_use_tree_is_read_to_its_leaves |
| own: an alias of an alias not followed | a_use_tree_..., a_seam_type_renamed_... |
| own: a use naming an encryption type by its last segment not an alias | a_use_tree_... |
| own: a type's right side module word without its `::` | a_use_tree_... |
| own: group elements not marked seen | a_super_inside_a_group_... |
| own: seen not consulted | a_super_inside_a_group_... |
| own: `$` not skipped in a use tree | a_use_tree_... |
| own: a group read only to its first element | a_use_tree_... |
| own: `aliased` ignored in the form-side impl check | a_seam_type_renamed_... |

Taken out instead of tested: the shadowed set's exclusion of the encryption files (first draft). Any
macro_rules there counts as `macro` already, so the exclusion changed no verdict any input could reach.
Survivors at the end: none.

## Round 10
No extra mutants were asked for, and this round wrote no line of the unit: the working copies are
keep/r3's, byte for byte (sums in keep/r3/SHA256SUMS). Rounds 1-3 stand as recorded above.
