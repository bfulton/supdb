# `unsafe` in `src/`: what each site relies on, and what checks it

Audited at `98e4e8f4`, 2026-11-28, between 09:20 and 11:30 UTC (and a rerun of Miri's timeouts after), on a machine of its own. Read-only to the engine. The nightly toolchain (`rustc 1.102.0-nightly`, `miri 0.1.0 (c82ce15637 2026-11-27)`) was used for Miri and ThreadSanitizer only, outside `sh scripts/check.sh`. `rust-toolchain.toml` and every script are unchanged. Nothing in the repository refused the nightly: `cargo +nightly` outranks the file, and `scripts/toolchain.sh`'s check runs only where a script calls it.

`grep -n '\bunsafe\b'` finds 470 occurrences in `src/`: 198 in `src/db.rs` and 272 in the other files. Two of those are not code (a doc comment at `lib.rs:86`, a string at `zselect.rs:4597`). That leaves:
- 399 `unsafe { }` blocks;
- 40 `unsafe fn`;
- 28 `unsafe impl`, every one `Send` or `Sync`;
- 1 `unsafe extern "C" fn` type;
- no `unsafe trait`.

68 are in test modules. No lint asks for SAFETY comments or forbids `unsafe` anywhere (there is no `unsafe_code`, `unsafe_op_in_unsafe_fn` or `undocumented_unsafe_blocks` setting). Even so, nearly every block carries a SAFETY comment.

Every site's row (what it does, the invariant, where it is established, whether anything checks it) is in two inventories beside this file:
- `unsafe/inventory-db.md`: `src/db.rs`, about 60 rows grouped by mechanism;
- `unsafe/inventory-rest.md`: every other file, by file.

The findings below are the sites where the invariant is not held by anything that runs. US-0 to US-7 and US-9 were checked against the code for this file, and US-0, US-1 and US-9 were run. US-8's list is taken from the inventories as they give it.

**No length or offset from the file reaches `unsafe` code unchecked.** Each path from file bytes into an `unsafe` block was traced and found bounded first:
- the run file's record slice (`db.rs:13322`, bounded by verified markers);
- the ordered index's heads (`ordindex.rs:557-563`, `checked_mul` and an exact fill);
- the LZ4 group decoder (`zlz4.rs`: every unchecked read and write sits behind a check on the group's lengths);
- extent counts into `flatindex.rs:734`;
- the block table count into `flatindex.rs:3883`;
- the envelope header into `crypt.rs:756`/`979` and `pager.rs:1623`;
- the one-file store's extents into its mapping (`onefile.rs:1280`, `base + cap <= end` in `adopt`).

The hostile-input harness (`hostile-input.md`) agrees. Across more than 100,000 mutated segments and stores, every panic, large allocation and misread it found was in safe code. Built under AddressSanitizer, 22,410 of those cases gave no ASan report.

## Findings

| id | sev | finding | where | proposed fix |
|---|---|---|---|---|
| US-0 | **high** | **Miri finds undefined behaviour in the memtable's put index, on every growth of it.** `MemTable::rebuild_index(&self, old: &Index, rd)` builds a table twice the size and swaps it in. Then, while `old` is still a live, protected `&Index` argument, it turns the same allocation into a `Box` (`Box::from_raw`, pushed onto `retired`) and calls `reclaim`, which frees it at once when no reader is pinned. Stacked Borrows rejects the `Box::from_raw` (a unique retag under a protected shared reference). Tree Borrows rejects the free (a deallocation under a protected reference). The caller (`index_insert`) never reads `old` again, so nothing misbehaves today. But a reference argument is `dereferenceable` for the whole call, and this one dies inside it. This is not driven by input: any table that grows its put index takes this path. 3 of the 5 `db::put_index` tests reach it (the fourth timed out, the fifth passed). | `src/db.rs:19841-19862` (`rebuild_index`), called from `index_insert` at `19807-19812` | Read `old.slots.len()` and clone `old.slots.src` before the swap, and pass the old table as the raw pointer `self.index.swap` returns, never as `&Index`. Keep `Box::from_raw` and the free away from any live reference. Run `db::put_index` under Miri in CI (it is pure apart from `blocks::alloc`'s anonymous mappings, which Miri supports). |
| US-1 | medium | **The shared table's sequence lock copies a value with plain loads and stores while another thread writes it: a data race, which the Rust memory model calls undefined behaviour.** `lean_overwrite` and `rewrite_in_place` write a value's bytes with `copy_nonoverlapping` under the record's lock word. `lean_read` copies the same bytes with a plain copy between two readings of the word, and keeps the copy only where the word did not move. ThreadSanitizer reports 8 data races on exactly these pairs, in `db::shared_table` (2), `tests/share.rs` (3) and `tests/shared.rs` (3), while every test passes. The protocol is sound on hardware, and the torn copy is thrown away. But a plain read racing a write is UB in the model, so the compiler is entitled to assume it does not happen. | `src/db.rs:21289` (`lean_overwrite`), `src/db.rs:21036` (`rewrite_in_place`), against `MemTable::lean_read` (`read_live_lean`) | Copy the value with relaxed atomic loads and stores of words and bytes (`AtomicU64`/`AtomicU8` views of the block, which is what `atomic_memcpy` and crossbeam's `AtomicCell` do), on both sides. Keep the lock word's fences as they are. Run `release/findings/unsafe/tsan.sh run shared` until it reports no race. |
| US-2 | medium | **Two safe public functions run AVX2 or AVX-512 code on the caller's word that the CPU has it.** `decompress_in_flat_on(isa, ..)` and `decompress_more_flat_on(isa, ..)` are safe `pub fn`s that call the `#[target_feature(enable = "avx2"/"avx512f")]` loops for whatever `FlatIsa` they are handed. Their doc says the CPU "must have" it, and nothing checks. On a CPU without the feature, safe code reaches undefined behaviour, in practice an illegal instruction. The module is `#[doc(hidden)] pub mod zlz4`, and its one outside caller is `bench/examples/zdecode.rs:209-222`, which passes `Avx512` unconditionally. The machine here has AVX-512, so the failure was not shown. | `src/db/zlz4.rs:792`, `907`; the loops at `1198`, `1218` | Make both `unsafe fn` with a `# Safety` section, or check `is_x86_feature_detected!` inside and fall back to `flat_isa()`. |
| US-3 | medium | **Shared references are made over memory other threads write without atomics.** Nothing at the type level holds the conditions these rely on:<br>- `Slab::peek_shared` returns `&MemEntry` while the entry's claimant writes its non-atomic fields (`hash`, `key_off`, `klen`) through raw pointers (`db.rs:21523`, `21594`). Its SAFETY comment rests on "the caller reads only the atomic", which the signature does not enforce.<br>- `rec_word64`/`rec_word32` make `&AtomicU64`/`&AtomicU32` out of a `&[u8]` that other threads change atomically: a shared `&[u8]` over bytes that change. Alignment is checked only by `debug_assert`.<br>- `MemTable`'s hand-written `Send`/`Sync` let writer-only `UnsafeCell`s be mutated through `&self`, and only the call graph keeps a non-writer thread out.<br>- `lend_asked_map` makes one `&[u8]` over a paged mapping's whole range while pages are filled and given back. Its comment says it is "sound by what is read through it and not by its type".<br>None of this was observed misbehaving, and TSan reported no race at these sites. It is UB by the aliasing model, not by any run. | `src/db.rs:18102-18111` (`peek_shared`), `19045-19060` (`rec_word64/32`), `17081-17082` (`MemTable` Send/Sync); `src/bytes.rs:586` (`lend_asked_map`) | Give `peek_shared` a return type that exposes only the atomic, such as `&AtomicU32` or a projection, never `&T`. Build `rec_word*` from a raw pointer into the block, never from a `&[u8]`, and make the alignment a hard `assert!` (once per record, off the hot path) or a type. For `MemTable`, put the writer-only cells behind a token only the writer holds (`&mut WriterToken`). For `lend_asked_map`, lend by the page asked for. |
| US-4 | low | **`prefetch_lines` is a safe function that does pointer arithmetic outside the allocation.** It calls `ptr.add(off)`, which requires both pointers in bounds. `prefetch_span` hands it `ptr.wrapping_sub(lead)`, up to 63 bytes before the buffer. Other callers (`db.rs:25223-25224`, `zfile.rs:2961`, and `ask_record` at `db.rs:21113`, which reaches up to `REC_HDR + klen + 2*CHUNK_HDR + LEAN_AHEAD`) pass spans that can run past a block's end. This is UB by `add`'s contract. A prefetch faults on no address, so nothing happens on hardware. | `src/db.rs:25498-25518` (`prefetch_lines`), `25424`, `21113`; `src/db/zfile.rs:2961` | Use `wrapping_add` inside `prefetch_lines`. It is already a hint, so it needs no in-bounds pointer. |
| US-5 | low | **`MmapBytes::open` is safe and public, and a file another process truncates or rewrites under it is SIGBUS, or bytes changing under a live `&[u8]`.** Its SAFETY comment states the contract ("the file must not be truncated underneath the mapping"), and nothing enforces it. The store's `LOCK` is advisory (`flock`). The same contract stands behind the WAL's, the run's and the ordered index's mappings (`db.rs:10202`, `13322`, `14211`, `15852`, `21752`), `vault.rs:1859`/`2943`, `pager.rs:1187` and `onefile.rs:1280`. This is the standard memory-mapping trade, and supdb never shrinks a file it names ("a file a store names never changes"). | `src/bytes.rs:496-503` | Document the contract on `MmapBytes::open` and in the crate docs ("a mapped file must not be truncated or written by anyone while a reader holds it"). Optionally, make it `unsafe fn open` as `memmap2::Mmap::map` is, keeping a safe `open` for the store's own files behind the `LOCK`. |
| US-6 | low | **The wasm host ABI's `u32` offset reaches JavaScript signed.** `supdb_host_read(off: u32, ptr: *mut u8, len: u32)` is answered by `(off, ptr, len) => cache.readInto(off, ..)` in `openSyncHandle`, `openCached` and `openSparse`, with no `>>> 0`. An offset or pointer at or past 2 GiB arrives negative, and the read fails rather than misreading. That breaks the rule "a wasm `u32` arrives in JavaScript signed: normalize to unsigned at the boundary". Separately, the exported `extern "C"` functions trust their pointer and length arguments wholly. They are the ABI, and JavaScript is the trusted caller. | `web/supdb.mjs:370`, `412`, `613`; `src/wasmapi.rs:49-52`, `202-801` | Use `off >>>= 0; ptr >>>= 0; len >>>= 0` in each `supdb_host_read`. Say in `wasmapi.rs` that every export's pointer and length must be what the module handed out. |
| US-7 | low | **Two crate-private safe functions take raw ownership.** `ZCell::keep(p: *mut ZFile)` stores a pointer that `Drop` later frees with `Box::from_raw`. The `RetiredZ`/`RetiredForm`/`RetiredSnap` and `Op`/`WriterOp` wrappers are tuple structs built in safe code, and their `Drop` frees or dereferences the pointer they hold. Wrapping one pointer twice is a double free, and only the call sites prevent it. Today's callers are correct. | `src/db/zfile.rs:6507`, `6534`; `src/db.rs:27970`, `28269`, `28277`, `29838`, `29853` | Make the constructors `unsafe fn` with the ownership stated, or take a `Box<ZFile>` and convert inside. |
| US-8 | low | **Invariants checked only by `debug_assert!`, so release builds check nothing:**<br>- `Slab::get` (an id past what was pushed gives `&T` to memory not yet written);<br>- `Slab::push`;<br>- `Lend::lend`;<br>- `Reader::state` (a `&State` tied to `&self`, not to the pin);<br>- the pin before a paged lend (`Paged::ensure`, `PINS > 0`);<br>- `Stream`'s `Sync` and its ring;<br>- `ordindex.rs:985`; `flatindex.rs:221`; `blocks.rs` `regrow`'s preconditions.<br>The tests run them all under `--profile checked`. | `src/db.rs:18156`, `18185`, `31760`, `55761`; `src/db/pager.rs:2077`, `2392`, `2531`; the rest as listed | Where a check is one compare off the hot path, make it `assert!`. Otherwise leave it, and say in the SAFETY comment that the checked profile is what holds it. |
| US-9 | low | **The store's own threads outlive the test that made them.** ThreadSanitizer reports 4 thread leaks: the `go_away` thread (`db.rs:52145`, from `Db::create`) and the upkeep thread (`db.rs:56268`, from `hand_over_upkeep`), in `tests/share.rs` and `tests/jobs_mapped.rs`. This is not memory unsafety. It means a `Db` dropped at a test's end leaves threads that were not joined. | `src/db.rs:52145`, `56268` | Check that `close` and `Drop` join both. If they are detached by design, say so where they are spawned. |

## Evidence

### ThreadSanitizer

```sh
export CARGO_TARGET_DIR=$(mktemp -d)       # a target of its own
sh release/findings/unsafe/tsan.sh build   # the lib's tests and six integration binaries, std rebuilt under TSan
sh release/findings/unsafe/tsan.sh run lib db::shared_table   # and the other targets below
```

`tsan.sh` runs `cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu --profile checked` with `RUSTFLAGS=-Zsanitizer=thread` and `TSAN_OPTIONS=halt_on_error=0`, two test threads, and writes one log per target to `release/findings/unsafe/out/`. Run 10:04:05 to 10:05:46 UTC:

| target | tests | data races | thread leaks |
|---|---|---|---|
| lib `db::shared_table` | 36 passed | 2 (`rewrite_in_place` at 21036, `lean_overwrite` at 21289, each against a reader's copy) | 0 |
| lib `db::lockfree` | 2 passed | 0 | 0 |
| lib `db::threading` | 2 passed | 0 | 0 |
| lib `db::snap_arena` | 2 passed | 0 | 0 |
| lib `db::pager` | 24 passed | 0 | 0 |
| `share` | 21 passed | 3 (`lean_overwrite` via `Reader::write_shared` against `lean_read` via `read_live_lean`) | 2 |
| `shared` | 14 passed | 3 (the same pair) | 0 |
| `settle` | 33 passed | 0 | 0 |
| `jobs_mapped` | 1 passed | 0 | 2 |
| `jobs_pooled` | 1 passed | 0 | 0 |
| `fuzz concurrent_range` (default seeds) | 1 passed | 0 | 0 |

A target with reports exits 101 with every test passing. That is the sanitizer's exit, not a failure. TSan does not model a standalone `atomic::fence`, so a report on a fence-ordered hand-off would be read before being called a race. Each of the eight above is a plain `memcpy` racing a plain read of the same bytes, which is a race whatever the fences order.

### Miri

```sh
export CARGO_TARGET_DIR=$(mktemp -d)
cargo +nightly miri setup
sh release/findings/unsafe/miri.sh          # every test of the pure modules, one process a test, 10 minutes each
```

`miri.sh` runs each test of the modules with no file mapping and no syscalls beyond the clock (`db::zlz4`, `flatindex`, `block`, `blob`, `bytes`, `index`, `reserve`, `db::radix`, `db::lockfree`, `db::snap_arena`, `db::put_index`, `crypt`: 102 tests). Each runs as `cargo +nightly miri test --lib -- --exact <test>` with `MIRIFLAGS=-Zmiri-disable-isolation`, under the default Stacked Borrows, under a timeout of 600 s, three at a time. It writes `release/findings/unsafe/out/miri-summary.txt`, one line a test. Run 10:06:50 to 11:30:15 UTC. For about its first hour, the module-at-a-time loop this script replaced was still running beside it, so the timeouts below were taken on a machine shared with a second Miri process:

| outcome | tests | what |
|---|---|---|
| pass | 72 | `blob` 3, `block` 7, `bytes` 3, `crypt` 19, `db::lockfree` 1, `db::put_index` 1, `db::radix` 2, `db::snap_arena` 2, `db::zlz4` 8, `flatindex` 14, `index` 1, `reserve` 11 |
| **Undefined Behavior** | 3 | `db::put_index::{a_folding_table_hashes_one_way_at_both_ends, a_table_of_puts_alone_grows_its_index_at_half_load, the_index_population_reaches_every_index_a_table_grows}`: US-0. With `-Zmiri-tree-borrows`, the first is again rejected, at the free (`release/findings/unsafe/out/miri-tb-put_index.log`). |
| test fails under Miri only | 6 | `flatindex::block_tests::{chunk_checksums_round_trip, every_block_round_trips}`, `flatindex::compact_tests::a_compact_record_gives_back_the_extent_it_was_written_from`, and `flatindex::tests::{a_directory_span_is_the_words_the_ranks_read_by, every_key_is_found_with_its_extents, rank_order_matches_key_order}`. Each panics on a `None` from `MappedBlocks::parse` or a lookup: the alignment checks in front of the `BlockRec` and `&[Ext]` casts refuse a `Vec<u8>` that Miri does not place at a 16-byte address, as `malloc` does. The casts are sound. The refusal is a reader behaviour: `hostile-input.md` HL-3 shows it natively (every shape lent at +1..+7 is refused). |
| unsupported by Miri | 5 | `bytes::tests` ×3 (file-backed `mmap`), `crypt::tests::{a_section_lent_once_is_not_walked_again, pages_asked_for_ahead_stay_inside_the_bound}` (`madvise(MADV_POPULATE_WRITE)` at `crypt.rs:736`) |
| timeout (600 s) | 16 | `db::zlz4` ×5 (the damaged-block and part-at-a-time decodes, every effort, the kept tables), `flatindex::{fill_tests::the_sections_length_is_the_composers, tests::damage_never_panics, tests::the_fence_never_changes_where_a_seek_lands}`, `reserve::tests::the_planner_holds_aggregates_rather_than_the_input`, `db::radix` ×2, `db::lockfree::a_retire_list_frees_each_item_once_and_never_early`, `db::put_index::two_keys_that_share_a_tag_are_two_entries`, `crypt::tests::{a_changed_bit_anywhere_is_refused, the_whole_object_decrypts_to_what_was_encrypted, the_writer_writes_whole_megabytes}` |

MIRI_RERUN

The script writes `release/findings/unsafe/out/miri-summary.txt`. This run's copy is `miri-summary-600s.txt`, and each test's log is under `out/miri/`.

Under Miri, `flat_isa()` reports the baseline, so the AVX loops (US-2) are not exercised. The `db.rs` memtable code (US-1, US-3, US-4) cannot run under Miri, because it allocates through `blocks::alloc`'s anonymous mappings.
