# Phase 1, track B: handoff

Branch `claude/supdb-pretag-p1-b`, the reviewer's groups PR9, PR10, PR11,
PR12, PR20 and PR21 in that order (`release/phase1.md`,
`release/ledger-review/consistency-and-order.md`). Times are UTC.

## Done

| group | commits | gate (`sh scripts/check.sh`) | count (`sh scripts/cost.sh BASE`) |
|---|---|---|---|
| PR9 | `95da018` forms-convert, `d80073c` forms-off | passed | against `8e3f9d7`: green |
| PR10 | `9ee3910` commit-forms, `ed5c6c6` snapshot-build | passed | against `d80073c`: red, `settle_each` +11 an op on `land` (register allocation, bisected to `9ee3910`); left for the coordinator to fix on the merged tree |
| PR11 | `c940f90` settle-patch, `d536f9d` settle-lean | passed | against `d80073c`: red, `land` +10.12 net: `settle_each` +11.36 an op (stack moves), `patch_block` +0.383 (`splice_run` inlined), `overlay_all` +0.047 (net negative); same disposition as PR10 |
| PR12 | `cca2f0e` zpages-controller, `62c73dc` zpages-ahead | passed | against `d536f9d`: green |
| PR20 | `22a55f9` rewarm | passed on the second run (the first: the cipher flake below) | against `62c73dc`: green; `scan_z` +15.60 an op in the allocator within its bound (`_int_malloc` +12.76 of 124), `scan_long_z` -25.07, `merge` -4.23 (`sweep_retired_states` -5.08) |
| PR21 | `fc93666` block-cache, `9386db8` merge-chain | passed | against `4932fe0`: green; every probe fell, `read_z` -7.27 an op, `read_s` -7.00, `scan_long_z` -24.72, `work` -2.08: `Options::tracks_points` went with `merge_chains_writes_uncovered`, and with it the early returns in `note_read` and `note_read_mode` |
| follow-ups | `e33a0f2` the fuzz's `shares()` no longer reads `ZFUZZ_SNAPKEPT`; `4932fe0` the in-place patch test names `settle_lean().0` as the patches' own count, held to a mutant (`patch_in_place` never taken) that fails it with "the puts over the copies in place: 0"; `a05999e` puts `shares()` on one line, which the format gate asks for since `e33a0f2` | `lists`, `settle`, `source` and a fuzz range passed | test-only |

## Held back, or done otherwise than the ledger says

- PR20: `supdb-nozjobsrewarm` went with the rewarm cluster. It is track A's
  PR7 arm and differed from `supdb-nozjobs` only by `rewarmraw:
  Some(true)`, which the rewarm cluster removes.
- PR21: `scan_cache_auto` is marked inline in the ledger, but two tests
  hold the cache with no budget of its own through it
  (`tests/db.rs` `overlay_model_z`'s `None`, and
  `a_replacing_write_patches_a_copy_in_place`, whose compared bytes the
  auto budget moves). It stays a hidden seam that loses its arm
  `supdb-cachenoauto`.

## Flakes met

- `tests/cipher.rs` `a_store_larger_than_its_cache_reads_as_itself_within_it`
  failed once in PR20's first gate, in the onefile group: six memory-file
  descriptors where it allows four. It counts `/proc/self/fd` for the
  whole binary, so another test's files count against it. 16 onefile runs
  of the binary passed at `22a55f9` and at `62c73dc`, and 8 plain loaded
  runs each; the rerun gate passed. Not on the known list.

## Positional removals checked

Every argument removed from a positional list in this track's commits,
at every call site, old against new: `carry_published` (`pieces_only`,
three sites), `overlay_model_z` (`pieces_only`, five sites),
`windows_over` (`lazy`, four sites), `settle_key` (`gathers`, three
sites), `Snapshot::extend` (`unfolded`, `lean`, three sites),
`raw_rewarm_files` (its only argument). Each dropped the removed option's
argument and left the others in place. No tuple, array or bit set read by
index lost an entry: fuzz draws keep their slots as `let _ =`, and the
counter arrays (`forms_events`, `settle_lean`) are unchanged in length.

- PR21: the doc of `the_raw_cache_arm_differs_from_the_combined_arm_in_the_cache_reads_alone`
  sat on the parallel merge arms' test, which went with its arms; it was
  moved onto its own test.

## State

Track B is done: PR9, PR10, PR11, PR12, PR20 and PR21, each gated and
counted, at `9386db8`, with this file after it. Open for the coordinator:
the `settle_each` and `patch_block` reds of PR10 and PR11 on the merged
tree, and the overlap of `supdb-nozjobsrewarm` with track A's PR7.
