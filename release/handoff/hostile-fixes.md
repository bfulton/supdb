# Hostile-input fixes: handoff

Branch `claude/supdb-pretag-hostile-fixes`, from `515d2cde`. Times UTC,
2026-11-28.

## Fixed

| high | commit | test (`tests/hostile.rs`) | harness after the fix |
|---|---|---|---|
| HI-1 zero `crc_off` read as no row | `2a53dfa` | `a_key_section_that_names_no_row_is_refused` | `crcoff 0..600: 600 cases, 321 opened, 279 refused, 0 failures`; `segment 0..20000: ... 0 failures` (the 26 misreads gone) |
| HI-2 file lengths sized allocations | `f46efbe` | `a_length_from_the_file_sizes_nothing_the_object_cannot_hold` | `segment 0..20000: 20000 cases, 14311 opened, 5689 refused, 0 failures`; `segforge 0..20000: 20000 cases, 16146 opened, 3854 refused, 0 failures` |
| HI-3 sparse open's unchecked sums | `4805f83` (+ `blob:` doc move) | `an_offset_at_the_top_of_the_range_panics_no_sparse_open` | `superfields: 8064 cases, 6670 opened, 1394 refused, 0 failures`, release and plain |
| HI-4 u64 words narrowed on wasm32 | `b62e6f9` | `a_header_word_past_32_bits_is_refused_and_never_narrowed`, through `web/hostile.test.mjs` (54 traps without, 0 with) | `wasm 0..14364: 14364 cases, 11480 opened, 0 traps, 0 misreads, 0 driver errors` |
| HI-5 keys past 65,535 / values past 4 GiB | `4e820ab`, `9688c74` | `a_key_past_65535_bytes_is_refused_at_every_write`, `a_value_past_4_gib_is_refused_at_every_write` | `bigkey: 3 cases, 1 opened, 2 refused, 0 failures`, release and plain |
| `MemTable::rebuild_index` UB (Miri) | `bb62fa9` | Miri, below | |

Each test was run red without its fix. The browser half: `sh scripts/check.sh
wasm` writes the cases (`SUPDB_HOSTILE_DUMP`) and runs
`node web/hostile.test.mjs web/supdb.wasm DIR`.

Miri (nightly, outside the gate), `MIRIFLAGS=-Zmiri-disable-isolation
cargo +nightly miri test --lib -- --exact db::put_index::NAME`, with and
without `-Zmiri-tree-borrows`: red before at `rebuild_index` ("strongly
protected"); after, `the_index_population_reaches_every_index_a_table_grows`,
`a_folding_table_hashes_one_way_at_both_ends` and
`a_put_overwrites_a_put_of_its_length_and_appends_anything_else` pass under
both. `a_table_of_puts_alone_grows_its_index_at_half_load` stops on an
`madvise` Miri does not support (no verdict); `two_keys_that_share_a_tag_are_two_entries`
did not finish in 40 minutes under Stacked Borrows and was not run under
Tree Borrows.

## Verdicts

- Gate: `all checks passed: build test stride onefile lint wasm bench lists`
  on `bb62fa9`, and again on `9688c74` (14:05 to 14:36), the head's code;
  `b4d49e5` and after add only this file.
- Fuzz past the gate's ranges on `bb62fa9`
  (`sh release/findings/hostile-input/fuzz-long.sh MODE 24 3024 50 3`):
  `cipher 24..3024: 60 blocks ok, 0 failed` (180,256 chunks decrypted);
  `onefile 24..3024: 60 blocks ok, 0 failed`.
- Count, `sh scripts/cost.sh 515d2cde`: RED, by these raises. The cost
  group has no raise file (`scripts/raises.txt` is the lists' only), so
  they stay red and go to the parent. Read paths unmoved (`read`, `read_s`,
  `scan*`: no engine function moved).
  - `Db::append_batch` (and `put_batch`): about 5 an op on `load_*` and
    `commit_*`, the whole-batch length pass (20 before `9688c74`).
  - `Db::append` / `put` / `delete`: 2 an op (`merge` probe, inlined into
    `merge_walk::main`), the length test of each write.
  - `SegmentWriter::end_with`: 1 a key, the 4 GiB test before
    `encode_run`.
  - `land`: `Blob::open_with` +270, `Sec::get` +192,
    `MemTable::index_insert` +152 over the run (open-path checks).
- wasm module: 133,566 bytes (54,204 gzip) against 131,565 (53,556) at
  the base, +2,001 (+648 gzip). The
  reason is the new refusals' checks and their error strings in the
  readers. No budget is enforced (`web/build.sh` prints the sizes).

## A key past 4 GiB in total across appends (run, 14:39 UTC, on `1a723c5`)

66 appends of 64 MiB to one key, a commit each, `common::options()`:
every commit `Ok`; `flush` `InvalidData: "segment writer: a key's values
exceed 4 GiB in one segment"`; `count` 66; a later commit `Ok` (not kept as
`Db::lost`); `close` the same error; reopen `Ok`, `count` 66, `read_all`
4,429,185,024 bytes; `close` the same error again. Nothing is lost and the
store opens, but no flush or close succeeds again: the writer puts a key's
values in a segment as one extent (`Extents::One`), whose length is 32 bits.

Fix not made, asked of the parent: refusing the append that carries a key
past 4 GiB needs the key's total across the live table and every segment on
every append (a lookup a write on the loads' path); writing such a run as
several extents is a record shape no writer makes today.

## Next

Waiting on the parent's choice above. Left for others: the cost raises above.
