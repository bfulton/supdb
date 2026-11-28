# Phase 1, track A: PR5, PR6, PR7, PR8, PR19

Branch `claude/supdb-pretag-p1-a`, from `8e3f9d71`. Times are UTC.

## Done

| group | commits | gate | count |
|---|---|---|---|
| PR5: point-read-layers, heads, upkeep-burst | 1fa6ec6, f1fce69, bc0fb20 | green: build, test, stride and onefile on the tree before a lifetime fix in 1fa6ec6; build, lint, wasm, bench and lists after it | `sh scripts/cost.sh 8e3f9d7`: RED, `Reader::settle_each` +13.9 an op on `land` (net +13.48), spills and reloads over the whole function after `heads_look_ms`'s `asked_of` went (f1fce69). Taken as reported: the coordinator fixes `settle_each` once on the merged tree, after track B's PR11. |
| PR6: memory-blocks, shared-table | 17676c4, db68a9b, fac7f89, 11d426f | 11d426f: `all checks passed: build test stride onefile lint wasm bench lists` (fac7f89's lint was red on an unused `mut`, which 11d426f removes) | at db68a9b against bc0fb20: `Db::append_pinned` +0.27 on `work` (a spill and reload of `%rbx`, net -6.86) passes, by the coordinator's reading of a layout change; `supdb_get` +2 on `read` (net +2) is fixed by fac7f89, which takes the shared table's lean read out of line: `read` -23 and `read_s` -10 an op under bc0fb20 (`COST_HEAD=<tree at fac7f89> sh scripts/cost.sh bc0fb20 read read_s`). The whole count at 11d426f (`sh scripts/cost.sh bc0fb20`): its one red is `append_pinned`, passed; read -23, read_s -10, read_z -9.7, land -16.1 an op. |
| PR7: memory-jobs (+zpages_buy_crosses), ranks-bounds | 2df676b, 9c97dee | 9c97dee: `all checks passed: build test stride onefile lint wasm bench lists` | `sh scripts/cost.sh edd45a6`: RED, two functions, each reproduced to the instruction on a second count. `Db::seal_table::{closure#4}` +0.499 on `merge` (2df676b): one more register copy a step, the BigVec's pool field gone, with the loop's padding `nopl` turned `data16 nopw` at the same count; the probe nets +8.97, then +3.86, swung by the bounded `sweep_retired_states`. `Reader::complete_forms` +0.491 on `land` (9c97dee): eight more stack loads, stores and copies a partition of its loop (5,474 passes), the rank calls gone from it; the probe nets +2.76, then +0.92. Both register allocation in a function whose source the commit changed, both with the probe's net above zero: HELD, reported. |

## Held back, and why

- PR7's `zpages_buy_crosses`: settled, not held. With `jobs_mapped` and the
  option inlined, `supdb-zbuycross`, `supdb-jobsmappedbuycross` and
  `supdb-jobspooledbuycross` each pin only defaults, so all three go with
  it, as the ledger's `jobs_mapped` row says.
- `supdb-nozjobsrewarm` is removed here (PR7) and by track B's PR20: at the
  merge, take the deletion (agreed with the coordinator).
- Partly removed, the rest left for a later pass: `Seg.ranks` and the
  piece-rank readers (PR7, ranks-bounds; nothing fills them now),
  `BlockTable.dense` (PR19, promote; no path fills it now),
  `MemTable::new_shared`'s arguments (PR6; every caller passes `true`),
  `SegmentOptions::buffers_mapped` (PR7; always set).
- `onefile::takes` keeps its `tier-` prefix (PR19, compaction), so a store
  an older build left a piece merge's temp name in still has it taken.
- Flake candidates, handed to the flake hunts: `tests/zpages.rs`
  `a_merges_outputs_have_their_form_files_when_it_lands` (2 of 6 failed
  run six at once beside a gate, sample_refreshed 0); `tests/db.rs`
  `readers_on_threads_keep_answering_beside_three_frozen_tables` under
  `SUPDB_TEST_STRIDE=1` (a watchdog, "a freeze waited for a held seal: the
  list had no room", once in a gate beside other tests; 3/3 alone and 12/12
  six at a time after).
- Not run for any group: the fuzz past the gate's ranges under
  `ZFUZZ_CIPHER=1` and `ZFUZZ_ONEFILE=1`.

## The positional check (track C's tuple bug)

Every group's diff was read for a removal from a tuple, array, bit set or
positional argument list whose remaining entries are read by position. One
site: `tests/zpages.rs`'s window tests took the store's shape as seven bools
by position, and the promote and scan-seek commits each removed one; every
call was checked, and 5d8964f gives the helper a `Windows` of named fields
with an assert that each reaches the store as the option it names. The
compaction commit renumbers the zland trace's `held_by` codes, read only by
the names array beside them in the same function. Nothing else qualifies:
the counters and arrays that went went whole or are named fields, and
`forms_events`, `upkeep_counts` and `form_choices` keep their indices.

## In progress

PR8 and PR19 are written, built under `--profile checked` commit by
commit, and their touched tests, `tests/source.rs`, the suite's `engines::`
tests and `bench/scripts/policy_defaults.py` pass at each. They sit in the
worktree `/home/user/supdb-wt`, branch `wt-pr6`, on top of this branch's
head, not pushed:

| group | commits (worktree), oldest first |
|---|---|
| PR8: pace, builder-ahead, upkeep-bounds | "The syncing pace is the model's, and the buffered pace goes"; "The builder ahead starts at a scan or a commit, and its refuted triggers go"; "The upkeep's bounds that lost go, and its abandon and conversion are the code" |
| PR19: promote, zpages-scan-seek, the windows' shape, compaction | "No block is promoted to a copy by its reads or its starts"; "A scan seeks its start through the fence and a group, as shipped"; "The windows' test takes its shape by name"; "No piece merge runs beside the partition merge, as shipped" |

## Next step

For each group in turn (PR8's base this branch's head, PR19's base PR8's head):

    cd /home/user/supdb
    git merge --ff-only <the group's head in wt-pr6>
    sh scripts/check.sh            # to its last line
    sh scripts/cost.sh <base>      # alone on the machine
    git push origin claude/supdb-pretag-p1-a

read each red by function and by instruction under phase1.md's rule,
update this file, push, and report to the coordinator.
