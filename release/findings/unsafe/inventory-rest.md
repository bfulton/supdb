# `unsafe` inventory: `src/` except `src/db.rs` (tree at 98e4e8f)

Read by an agent for this audit at `98e4e8f4`, read-only; nothing built or run for it. The findings that `unsafe.md` takes from here were checked against the code there. Line numbers are from `grep -n '\bunsafe\b'`.

## Counts

272 grep hits. 2 of them are not code: `src/lib.rs:86`, a doc comment, and `src/db/zselect.rs:4597`, a string literal in a test's list of banned tokens. That leaves **270 code sites: 207 production and 63 test-only.**

| file | prod | test | unsafe fn | unsafe impl |
|---|---|---|---|---|
| src/blob.rs | 2 | 0 | 0 | 0 |
| src/block.rs | 4 | 1 | 2 | 0 |
| src/bytes.rs | 18 | 0 | 0 | 2 |
| src/crypt.rs | 7 | 2 | 1 | 0 |
| src/flatindex.rs | 8 | 0 | 0 | 1 |
| src/lib.rs | 0 | 0 | 0 | 0 |
| src/ordindex.rs | 9 | 1 | 2 | 0 |
| src/wasmapi.rs | 18 | 0 | 16 | 0 |
| src/db/blocks.rs | 47 | 45 | 7 | 6 |
| src/db/dirkey.rs | 3 | 0 | 0 | 0 |
| src/db/onefile.rs | 6 | 1 | 0 | 0 |
| src/db/pager.rs | 37 | 6 | 3 | 4 |
| src/db/vault.rs | 13 | 0 | 0 | 0 |
| src/db/zfile.rs | 8 | 4 | 1 | 1 |
| src/db/zlz4.rs | 23 | 0 | 8 | 0 |
| src/db/zpages.rs | 4 | 3 | 0 | 0 |
| src/db/zselect.rs | 0 | 0 | 0 | 0 |

- **Kinds:** 40 `unsafe fn`, 14 `unsafe impl` (all `Send` or `Sync`), 0 `unsafe trait`. The other 216 are `unsafe { }` blocks.
- **Implicit unsafe:** `src/wasmapi.rs:46-52` is an `extern "C"` block, so it has no `unsafe` keyword (edition 2021). Its two host imports are listed as W1 and W2.

**Lint settings.** There is no `#![forbid(unsafe_code)]` or `#![deny(unsafe_code)]` anywhere, and no `unsafe_op_in_unsafe_fn` setting. The edition is 2021 (`Cargo.toml:4`), where that lint is allowed by default. `[lints.rust]` (`Cargo.toml:45`) sets only `unexpected_cfgs`. No clippy `undocumented_unsafe_blocks` or `missing_safety_doc` is configured. The only crate attributes are the door's `#![allow(clippy::disallowed_methods, clippy::disallowed_types)]`, in bytes.rs:40, crypt.rs:103, blocks.rs:32, dirkey.rs:53, onefile.rs:55, pager.rs:89 and vault.rs:158. Most `unsafe fn` bodies still wrap their operations in an inner `unsafe {}` block. The wasmapi fns do not.

Legend for the last column:
- **yes** means a runtime check or the type system enforces it.
- **test** means a test exercises it.
- **dbg** means only a `debug_assert!` checks it. The `checked` test profile enables those, so tests run them and release builds do not.
- **no** means nothing checks it.

---

## src/blob.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| BL1 | blob.rs:3040 | block | `recs.as_ptr().add(from)` for `prefetch_lines` (x86 `_mm_prefetch`) in a fixed-width scan | `from < recs.len()`. `off` comes from a directory word in file bytes. | blob.rs:3036-3038: `want = off.saturating_add(lead).min(recs.len())`, `from = asked.max(off)`, `if from < want` | yes |
| BL2 | blob.rs:3700 | block | `from_raw_parts_mut` over a `Vec<u64>` as `len` bytes (an aligned copy buffer for a sparse dictionary span) | buffer holds at least `len` initialised bytes, and the alignment of `u8` is no stricter | blob.rs:3698 `words.resize(len.div_ceil(8), 0)`. `len` comes from file-derived `record_span`, and only the allocation size depends on it. `read_at` bounds-checks the source. | yes |

## src/block.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| BK1 | block.rs:82, 100 | fn | `#[target_feature(sse4.2 / crc)] crc32c_hw`, using `_mm_crc32_u64`/`__crc32cd` intrinsics | the CPU has the feature | — | — |
| BK2 | block.rs:223, 239 | block | calls BK1 | the feature is detected first | block.rs:212-221 and 228-237: `is_aarch64_feature_detected!("crc")` / `is_x86_feature_detected!("sse4.2")` cached in `CRC_IMPL` | yes |
| BK3 | block.rs:655 | block (test-only) | calls BK1 in `checksum_tests::hardware_and_scalar_crc_agree` | the feature is detected | block.rs:637-643 early return | yes + test |

## src/bytes.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| BY1 | bytes.rs:407, 409 | impl | `Send`/`Sync` for `MmapBytes` (holds `*const u8`) | `own` keeps the mapping alive. `Own::Map(Mmap)` and `Own::Paged(Arc<dyn Paging>)` are Send+Sync because `Paging: Send + Sync` (bytes.rs:425) and `MapHeld: Send + Sync` (400). | trait bounds | yes (type) |
| BY2 | bytes.rs:502 | block | `memmap2::Mmap::map(&file)` (MAP_SHARED) inside the **public safe** `MmapBytes::open(path)` | no process truncates or rewrites the file while it is mapped. Truncation gives SIGBUS. A rewrite changes bytes behind a live `&[u8]`, which is UB. | SAFETY comment only ("supdb only ever appends") | **no** |
| BY3 | bytes.rs:553 | block | `from_raw_parts(ptr, lend)` in `lend_whole_map` | `lend` bytes of a file mapping are kept by `own` | from_map (512-518). The type-level seal is `whole_map_readers::Reader`. | yes (type), apart from BY2's truncation contract |
| BY4 | bytes.rs:586 | block | `from_raw_parts(ptr, len)` over the **whole** address range of a paged mapping (`lend_asked_map`) | the slice spans pages that other threads write (fills) and that are given back with `MADV_REMOVE`/`PROT_NONE`. The comment concedes it is "sound by what is read through it and not by its type". Reads must stay inside pages asked for under a live pin. | SAFETY comment only. Sealed to `StrideLendToken` (zfile). | **no**. A `&[u8]` over concurrently written memory is UB under Rust's aliasing model, whatever is read through it. |
| BY5 | bytes.rs:603 | block | `*ptr.add(at)` touch loop | `at < lend` | bytes.rs:599 `end = …min(self.lend)`, loop `at < end` | yes |
| BY6 | bytes.rs:649 | block | `from_raw_parts(at, len)` from `Paging::place` (paged mapping, slow path of `slice_at`) | the bytes are present and stay present "for as long as the pin or the hold". The returned lifetime is `&self`, not the pin's. | the range is checked at 645 (`off+len <= self.len`). Presence: `Paged::ensure`. The pin is only `debug_assert!(PINS>0)` (pager.rs:~2077). | yes (range), **dbg** (pin) |
| BY7 | bytes.rs:669 | block | `libc::mincore` over a page-aligned sub-range | the range is inside the mapping, and `vec` has one byte per page | 659-666: `end` is clamped to `lend`. If `ptr` is not page-aligned (`from_map_held` with an offset), the call fails with EINVAL and returns `None`. | yes |
| BY8 | bytes.rs:687, 814, 840 | block | `libc::sysconf(_SC_PAGESIZE)` | none (pure query) | — | yes |
| BY9 | bytes.rs:732, 744 | block | `from_raw_parts(ptr.add(off), len)` in `slice_at` / `lend_at` | `off+len <= lend` | 729-731 and 741-743: `checked_add` plus a compare | yes |
| BY10 | bytes.rs:772 | block | `from_raw_parts(at, n)` from `place_piece` (paged) | as BY6 | pager `place_piece` / `piece_from` (pager.rs:2066-2072, 2108-2121) | yes (range), **dbg** (pin) |
| BY11 | bytes.rs:824, 882, 909 | block | `madvise` with `MADV_DONTNEED`, `MADV_POPULATE_READ`, and `MADV_DONTNEED` over the whole mapping, on a read-only MAP_SHARED file mapping | the range is whole pages inside the mapping | clamped to `lend` (818, 868, 902). DONTNEED on a shared file mapping refaults the same bytes. An unaligned `ptr` gives EINVAL. | yes |
| BY12 | bytes.rs:845 | block | `mincore` of one page | the page lies inside the mapping | 834-842 | yes |

## src/crypt.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| CR1 | crypt.rs:670, 735 | block | `madvise` (`MADV_HUGEPAGE`, then `MADV_POPULATE_WRITE` in `Kept::populate`) on the anonymous kept mapping | the range is inside the mapping | 670: the whole mapping. 735: caller `slice_at` 1083-1085 has `from = plain_off(c) < plain_len`, `to <= end <= plain_len = map.len()`. | yes |
| CR2 | crypt.rs:753 / 756 | fn + block | `Kept::lend`: `from_raw_parts(map.as_ptr().add(off), len)` | every chunk in the range is `READY` (written once, then never written again). The range is inside `plain_len`. | READY comes from callers 1028 (`keep` returned true), 1096 (state walk, `knows` or `whole`) and 1032-1036. Bounds: `read_at` 1003-1006 and `slice_at` 1056-1058. `Per::of` is exact for n < 2^56. That bound comes from `Header::new` 202-208 (at most 2^32 chunks) and `MAX_CHUNK_SHIFT=24`. | yes. Tests in crypt.rs `mod tests` (flip and refusal tests at ~1540). |
| CR3 | crypt.rs:979 | block | `from_raw_parts_mut` over chunk `c` of the kept mapping for decryption | this thread moved the chunk EMPTY to BUSY with a CAS, and `plain_off(c)+n <= plain_len` | CAS at 962-966. `c < chunks` comes from `per.of(at)` with `at < plain_len`. The `Filling` guard resets the state on error or panic. | yes |
| CR4 | crypt.rs:1028, 1096 | block | calls CR2 | as CR2 | as CR2 | yes |
| CR5 | crypt.rs:1559, 2004 | block (test-only) | `from_raw_parts` over a refused chunk; `mincore` of the kept map | — | — | test |

The envelope header comes from file bytes. `checked_header` (437-460) requires `src.len() == stored_len()`, and `Header::new` bounds the chunk shift and the chunk count before any of the above runs.

## src/flatindex.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| FI1 | flatindex.rs:221 | block | `_mm_prefetch(t.as_ptr().add(a))` in `prefetch_slot` | `a + SLOT <= t.len()`. `ptr::add` past the allocation would be UB even for a prefetch. | `debug_assert!` at 215 only. `a = (h & mask) * SLOT`, with `mask = cap-1` from the writer's own table. | **dbg**. The data is internal (writer side). |
| FI2 | flatindex.rs:734 | block | `from_raw_parts(bytes.as_ptr() as *const Ext, n)`: reinterprets file bytes as `&[Ext]` | length `n*EXT_BYTES` is available, 4-byte alignment holds, and `Ext` is `repr(C)` with 5 `u32` fields, so every bit pattern is valid | 731 `rest.get(..n.checked_mul(EXT_BYTES)?)?`, alignment test at 732. `n` comes from the record header (file). | yes. Native-endian reinterpretation is a correctness issue on big-endian targets, not a safety one. |
| FI3 | flatindex.rs:1973 | impl | `Sync` for local `Slots(*mut AtomicU64, usize)` | every access is an atomic CAS on one slot. The pointer outlives the `thread::scope`. | scope at 1979 | yes (scope) |
| FI4 | flatindex.rs:1975 | block | `out.as_mut_ptr().add(hash_off) as *mut AtomicU64` | `hash_off + hash_cap*SLOT <= out.len()`, 8-byte alignment, zeroed slots | alignment checked at 1952 (`aligned`). The length is **not** checked on this path. The sequential path slices `out[hash_off..hash_off+hash_cap*SLOT]` with a panic check at 1958. The layout `p` establishes it. | alignment yes, length **no** (layout only). The data is internal. |
| FI5 | flatindex.rs:1997 | block | `&*slots.0.add(s)` | `s < hash_cap` | `s = h & mask` / `(s+1) & mask`. `debug_assert!(s < slots.1)` at 1995. Termination rests on the load bound (`HASH_MAX_LOAD`). | yes (mask) |
| FI6 | flatindex.rs:2425 | block | `_mm_prefetch(p.wrapping_add(a))` in `ask_record` | none. `wrapping_add` with no dereference is defined behaviour even out of bounds, and a prefetch does not fault. | — | yes |
| FI7 | flatindex.rs:3764 | block | `from_raw_parts(&r as *const BlockRec as *const u8, BLK_ENTRY)`: the bytes of a `repr(C)` struct (encode side) | no uninitialised padding. Padding is the explicit `_pad: [u8;7]` (3683-3692). | the struct definition | yes (type) |
| FI8 | flatindex.rs:3883 | block | `*(bytes.as_ptr() as *const BlockRec)`: reads a block-table entry from file bytes | exactly `BLK_ENTRY` bytes, alignment holds, all fields are integers | `MappedBlocks::get` 3875-3882: `i < n`, `sec.range(at, at+BLK_ENTRY)?`, alignment test. `n` is bounded by `parse`/`parse_in` (3820-3836). | yes |

## src/ordindex.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| OI1 | ordindex.rs:953 / 960 | fn + block | `head_in_memory`: `p.add(i*HEAD).cast::<u64>().read_unaligned()` | `i < lim`, and `p` names `n` heads | `over` 557-561: `heads_at == HEADER && HEADER + n*HEAD == body`, checked against the mapping's length (file-derived `n`). `lim = n` or 0 (586). | yes |
| OI2 | ordindex.rs:749, 944 | block | calls OI1 | `rank < lim` / `i < lim` | 741 and 941 tests | yes |
| OI3 | ordindex.rs:975 | block | `from_raw_parts(p as *const [u8;HEAD], lim)` | as OI1, and the pointer stays valid for the caller's pin | `lim == 0` early return at 968 | yes (bounds). Copy lifetime: see OI6. |
| OI4 | ordindex.rs:985 | block | `p.add(lo*HEAD)` | `lo <= n` | `debug_assert!` at 982. Callers 1359 and 1495 pass `lo < hi <= n`. | dbg + callers |
| OI5 | ordindex.rs:1209, 1539 | block | `get_unchecked` in binary search / `select_lower_bound` probe | `m < held.len()`. The probe index is below the stride length. | 1204 `hi <= held.len()` with `a <= m < b <= hi`. `db.rs:25537` `select_lower_bound` probes only below `n` (read and confirmed). | yes (by construction), dbg at 1536 |
| OI6 | ordindex.rs:1014 | fn | `set_heads(p)`: stores a raw pointer to a heads copy | the copy is whole, unchanged, and allocated until `reset_heads` and past every reader that loaded the old pointer | the caller is in db.rs:15855 (out of scope, epoch) | contract only here |
| OI7 | ordindex.rs:2214 | block (test-only) | `set_heads(copy.as_ptr())` | `copy` outlives the reset (2219) | test | test |

## src/wasmapi.rs (wasm32 only; host = JavaScript)

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| W1 | wasmapi.rs:47-52, 67 | extern + block | host import `supdb_host_len()` | none (a scalar) | the result is `u32`, so `len < 2^32`. A JS length ≥ 2^32 is truncated by ToInt32 and gives a short length, which makes reads fail but does not cause UB. | yes |
| W2 | wasmapi.rs:83 | block | host import `supdb_host_read(off, dst.as_mut_ptr(), dst.len())` | the host writes only `[ptr, ptr+len)` | Rust side: 76-81 checks `off+len <= self.len <= u32::MAX`, `dst` is initialised, and a short write leaves zeroes, not UB. Nothing can stop JS writing anywhere in linear memory (trusted host). | yes (Rust side). Host trusted. |
| W3 | wasmapi.rs:202 | fn | `supdb_free`: `Vec::from_raw_parts(ptr, 0, len)` | `ptr` and `len` are exactly what `supdb_alloc` returned or was given. `vec![0;n]` capacity is exactly `n`, as documented. | JS `web/supdb.mjs:69-75` | **no**. A wrong length or a double free is allocator UB. |
| W4 | wasmapi.rs:291 | fn | `supdb_open_mem`: `Vec::from_raw_parts(ptr, len, len)` takes ownership | as W3, and not freed again | JS `web/supdb.mjs:349-354` | **no** |
| W5 | wasmapi.rs:267, 384, 445, 456, 472, 488, 513, 534, 561, 801 | fn | `slice::from_raw_parts(ptr, len)` over JS-supplied key, plan or range bytes | `ptr` is non-null and readable for `len` bytes. A null `ptr` is UB even when `len` is 0. JS always passes a `supdb_alloc` pointer (dangling but non-null for 0). | JS `withKey` (supdb.mjs:65-75) | **no**. JS-trusted. `supdb_ranges` then parses its buffer with `get()` and checked adds (395-414), so lengths *inside* the buffer are bounded. |
| W6 | wasmapi.rs:669, 692, 723, 753 | fn | `range_args`: as W5, skipping `hi` when `hlen == u32::MAX` | as W5 | JS `withRange` (supdb.mjs:510-516) passes `hp=0, hl=0xffffffff`, so `hptr` is unused | **no** (JS-trusted) |

wasmapi has no Rust tests of its own. The source comment at 40-45 says nothing in CI builds the module.

## src/db/blocks.rs (the engine's anonymous-block allocator; it is not a block decoder)

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| BA1 | blocks.rs:147, 153 | block | `sysconf`; `mincore(p, len)` over a `LIVE` block | `vec` has one byte per page. A range that is no longer mapped fails and writes nothing. | 149-150 | yes |
| BA2 | blocks.rs:181 | block | `alloc` / `alloc_zeroed` with `Layout(len, 64)` | `len > 0` | `assert!` at 172, null `assert!` at 189 | yes |
| BA3 | blocks.rs:204 / 208, 216 | fn + blocks | `populate_write`: `MADV_POPULATE_WRITE`, falling back to `write_volatile(p.add(off), 0)` every 4096 bytes | the range is writable, zeroed, and nobody else's | callers 307 and 326: freshly mapped, zeroed blocks | contract (callers yes) |
| BA4 | blocks.rs:226 / 235, 241 | fn + blocks | `free`: `munmap` or `dealloc` according to `maps(len, mapped)` | `p` came from `alloc` with this `len` and `mapped`, and nothing reads it afterwards | one rule, `maps()` (166-168), shared by alloc and free. Callers below. | contract, plus tests in `mod tests` |
| BA5 | blocks.rs:307, 326 | block | calls BA3 on a block just mapped | as BA3 | just allocated | yes |
| BA6 | blocks.rs:317 | block | `write_bytes(p, 0, len)` on a block from the pool | the pool held a block of exactly `len` bytes | `BlockPool::take` 437-440 matches the exact length | yes + test (1866-1868) |
| BA7 | blocks.rs:338 / 347 | fn + block | `Source::free`: to the pool or to `free` | as BA4 | — | contract + test |
| BA8 | blocks.rs:461 | block | `free` of held blocks in `BlockPool::release` | the blocks were taken out under the lock and are nobody's | `std::mem::take` under the lock (457) | yes |
| BA9 | blocks.rs:484, 485 | impl | `Send`/`Sync` for `BlockPool` | the held blocks are addresses (`usize`) owned under a Mutex | **redundant**: every field is already Send+Sync (`Mutex<Vec<(usize,usize)>>` and atomics) | yes (would be automatic) |
| BA10 | blocks.rs:496 / 509, 525 | fn + blocks | `regrow`: `mremap(MREMAP_MAYMOVE)`, or `alloc` + `copy_nonoverlapping(p,q,used)` + `free` | `p` is from alloc/regrow with `old`, `used <= old < new`, and no reader remains | `debug_assert!` at 503. MAP_FAILED `assert!` at 510. | dbg + test (2093-2122) |
| BA11 | blocks.rs:693 / 736, 750 | fn + blocks | `Pool::give` and `drain`: `free` of buffers | the buffer is a mapping counted by `lend`, and nobody's | under the `kept` lock | contract + test (1565-1687) |
| BA12 | blocks.rs:807, 809 | impl | `Send` (`T: Copy+Send`) / `Sync` (`T: Copy+Sync`) for `BigVec<T>` (raw `map: *mut T`) | it owns its buffer exactly as `Vec<T>` does. `pool: Option<&'static Pool>` is Sync. | bounds mirror `Vec`'s | yes (sound) |
| BA13 | blocks.rs:889 | block | `regrow` of a pooled buffer shorter than asked | `p` is a pooled mapping of `len` bytes, and `used = 0` | `Pool::take` | yes |
| BA14 | blocks.rs:899 | block | `copy_nonoverlapping(heap.as_ptr(), p, heap.len())` when becoming a mapping | `cap >= heap.len()`; the buffers are distinct | callers pass `want >= heap.len()` (outgrow 913-918, with_capacity on an empty heap). A pooled buffer is used only when `len >= bytes`. | yes (by construction) |
| BA15 | blocks.rs:941 | block | `copy_nonoverlapping(map, q, len)` + `pool.give(old)` in `grow_map` | `q` has at least `bytes > len` elements | `Pool::take(.., or_largest=false)` returns only `len >= bytes` (657-659) | yes |
| BA16 | blocks.rs:954 | block | `regrow(map, bytes, new, len*size, true)` | as BA10 | — | yes |
| BA17 | blocks.rs:1002, 1020, 1039 | block | `map.add(len).write(v)`; `copy_nonoverlapping(s, map.add(len), s.len())`; `map.add(i).write(v)` | `len < cap`, `cap - len >= s.len()`, `i < n <= cap` | 998-1000, 1016-1018, 1033-1036 (grow first) | yes + test |
| BA18 | blocks.rs:1060, 1071 | block | `Deref`/`DerefMut`: `from_raw_parts(map, len)` | the first `len` values are written, and `map` is aligned (mmap is page-aligned, and the const `FITS` gives align ≤ 64) | every writer keeps `len` (BA17) | yes |
| BA19 | blocks.rs:1080 | block | `Drop`: `pool.give` or `free` | `bytes` is the mapping's length | — | yes + test |
| BA20 | blocks.rs:1156, 1158 | impl | `Send`/`Sync` for `SlabInner` (`p: *mut u8`) | cells are disjoint and each is handed to one holder at a time under the lock | `Slab::take` 1186-1197 / `SlabCell::drop` | yes. The aliasing discipline is the holder's (see BA22). |
| BA21 | blocks.rs:1213 | block | `SlabInner::drop`: `free(p, bytes, true)` | every cell has dropped (each holds the `Arc`) | Arc refcount | yes (type) |
| BA22 | blocks.rs:1233 | block | `SlabCell::as_ptr`: `p.add(i*cell)` | `i < cells` | `take` (1190-1193) | yes (offset). But `as_ptr(&self) -> *mut u8` is a safe fn, and `SlabCell` is `Sync`. Exclusive writes depend on the one holder, `GroupBuf`, owning the cell (zpages.rs:790). |
| BA23 | blocks.rs:1257, 1285, 1299 | block | `mmap` anonymous (`map`, `map_huge`); trims the head and tail with `munmap`; `MADV_HUGEPAGE` | `len + HUGE` mapped, `huge_trim` arithmetic | MAP_FAILED `assert!`. **The `munmap` results are unchecked**, and a `len` that is not a page multiple makes the tail `munmap` fail with EINVAL. Only an address-space leak, not UB. | partly |
| BA24 | blocks.rs:1338 / 1340, 1348, 1354 / 1357 | fn + blocks | `unmap` (`munmap`, `debug_assert_eq!(r,0)`); non-unix `alloc_zeroed` / `dealloc` | as BA4 | — | dbg |
| BA25 | blocks.rs:1416-2152 (45 sites) | block (test-only) | `from_raw_parts(_mut)` over test blocks, `free`/`give`/`regrow` calls, `ptr.read`/`write`, reads of pooled bytes | the test's own blocks | — | test |

## src/db/dirkey.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| DK1 | dirkey.rs:76, 275 | block | `write_volatile(b, 0)` key zeroing | `b` is a live `&mut u8` | — | yes (type) |
| DK2 | dirkey.rs:226 | block | `libc::ioctl(fd, request, arg as *mut T)` in `ask<T>`, a **safe generic** fn | `T` is the `repr(C)` struct that `request` encodes, with the size the request expects | `const _: () = assert!(size_of::<…>() == …)` at 198-202 fixes the struct sizes. Nothing ties a `T` to its `request`; the private callers do. | partly (sizes), pairing **no** |

## src/db/onefile.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| OF1 | onefile.rs:613, 693, 1395 | block | `fallocate`, `posix_fadvise(DONTNEED)`, `sync_file_range` on an owned fd | none involving memory | — | yes |
| OF2 | onefile.rs:1280 | block | `MmapOptions::offset(loc.base).len(len).map(&own)`: MAP_SHARED of an extent of the shared data file | the extent is not cut or rewritten while it is mapped. `base+cap` lies within the file. | `hold` (`Using`) keeps the room out of the free list until the mapping drops. `mapped` flag set at 1263; a move under a mapping is refused (1518-1522). `adopt` checks `base+cap <= end` (~1184). The engine never shrinks the data file (only `set_len` at create 464 and growth 633). | yes (in process). An external truncation gives SIGBUS; nothing can prevent that. |
| OF3 | onefile.rs:1346 | block | writes `*__errno_location()` / `*__error()` | thread-local errno | — | yes |
| OF4 | onefile.rs:1651 | block | `pwritev` with `IoSlice` as `iovec` | IoSlice is ABI-compatible; count ≤ 1024 | 1636 `iov[..min(1024)]` | yes |
| OF5 | onefile.rs:1901 | block (test-only) | mapping + `mincore` | — | — | test |

## src/db/pager.rs (the cipher's plaintext cache)

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| PG1 | pager.rs:963 | block | `gettid()` | none | — | yes |
| PG2 | pager.rs:1187 | block | `Mmap::map(&file)` of an envelope (MAP_SHARED) | the envelope is written once and never truncated | `Back::over` 1201 `checked_header` requires `len == stored_len`. An external truncation gives SIGBUS. | SAFETY comment (in process) |
| PG3 | pager.rs:1326, 1327 | impl | `Send`/`Sync` for `Paged` (`base: *mut u8` to a MAP_SHARED memfd mapping) | a chunk is written only by the thread that moved it to BUSY. It is read only when HOT or COLD. Pages are given back only past every pin and `Hold`. | the state machine (`slow`, `rescue`, evictor `Grace`/`Hold`) | dbg. `debug_assert!(PINS > 0)` in `ensure` (~2077); `SUPDB_PAGER_GUARD` mprotect under debug; fuzz and tests. |
| PG4 | pager.rs:1361, 1509, 1660, 2154, 2210, 2239, 2769, 2786, 2871 | block | `posix_fadvise(DONTNEED)`; `madvise` with `POPULATE_READ` (whole own mapping), `WILLNEED`, `DONTNEED` and `NOHUGEPAGE`, and `mincore`, over the envelope's or own mapping | the address range is inside the mapping | each range is clamped with `.min(back.map.len())` / `self.mapped`, guarded by `if to > from` / `from < to` (1655-1658, 2142-2163, 2193-2205, 2223-2236, 2765, 2799-2810, 2829-2836). The chunk index comes from the plaintext offset, which is below `len`. | yes |
| PG5 | pager.rs:1383 / 1385, 1391; 2285 | fn + block | `populate_read` (`MADV_POPULATE_READ`) | `p` is page-aligned inside a mapping that lives through the call | 2282-2284 `lo < hi`, `hi.min(map.len())` | yes |
| PG6 | pager.rs:1400, 1405 | block | `memfd_create`; `File::from_raw_fd` | the fd is fresh and owned | `fd < 0` check | yes |
| PG7 | pager.rs:1615 | block | `mprotect(RW)` under the guard (debug only) | the chunk's pages are inside the mapping | `.min(self.mapped)` at 1612 | yes |
| PG8 | pager.rs:1623 | block | `from_raw_parts_mut(base.add(c*CHUNK), n)`: the decrypt destination | the chunk is BUSY by this thread, and `c*CHUNK+n <= len <= mapped` | `chunk_len(c)` (1574) with `c < state.len()` (callers index `state[c]`). The envelope is read through `back.map.get(at..at+n+TAG)` (1594), which is bounded. `Back::over` checks `chunk_shift == CHUNK_SHIFT` (1202). `back()` checks `plain_len == len` (1520). | yes |
| PG9 | pager.rs:1694 | block | `from_raw_parts(base.add(c*CHUNK), dst.len())` in `copy_present` | the chunk is present, and `dst.len()` equals the chunk's length | `state.get(c)` HOT/COLD at 1688. The caller (Stream 2593-2597) passes `into` of `chunk_len(c)`. Not given back before the holder's next rest. | yes (bounds). Rest contract: dbg/test. |
| PG10 | pager.rs:1912, 1915 | block | `give_back` (`MADV_REMOVE`) and `mprotect(NONE)` over a unit's whole pages | every reader that could hold a borrow has been waited out | evictor: DYING mark, `Grace` epoch, `Hold` eras (module docs 26-48). BUSY spin at 1898-1902. | dbg / test (the store-buffering fences are SeqCst) |
| PG11 | pager.rs:2037, 2699 | block | `munmap` of `Paged` / `Stream` address space in `Drop` | no borrow outlives the last holder | Arc drop | yes (type, via Arc) |
| PG12 | pager.rs:2389, 2392 | impl | `Send`/`Sync` for `Stream`, which holds `UnsafeCell<Ring>` and raw pointers | **one thread only** uses a stream | `debug_assert` owner check in `chunks` (2524-2529), debug builds only | **dbg**. Release builds have no check. |
| PG13 | pager.rs:2478 | block | `mmap(PROT_NONE, MAP_NORESERVE)` placeholder address space | none | MAP_FAILED check | yes |
| PG14 | pager.rs:2531 | block | `&mut *self.ring.get()` | no concurrent access, and no borrow of the ring outlives the call | as PG12 | **dbg** |
| PG15 | pager.rs:2573 | block | `from_raw_parts_mut(put, n)` into a ring buffer | `ring.take(k*CHUNK)` handed out `(b+1-from)*CHUNK` bytes, nothing lent in this stretch or the one before overlaps them, and `n <= CHUNK` | `take` 2654-2683. `place` 2724 checks `off+len <= len`, so `b` is a real chunk index. | yes (bounds). Overlap rests on the rest-epoch logic. |
| PG16 | pager.rs:2585 | block | `from_raw_parts(r.at + (c-first)*CHUNK, n)` from an earlier run | the run is lent in this stretch or the one before, and `first <= c < end` | the `find` predicate at 2582 | yes |
| PG17 | pager.rs:2642 | block | `write_bytes(at, 0xA5, cap)` poisons free buffers (debug guard) | the buffer is not lent in the last two stretches | `s.rest + 1 < rest` | yes |
| PG18 | pager.rs:2690 | block | `Box::from_raw(slice_from_raw_parts_mut(at, cap))` in `Ring::drop` | each buffer came from `Box::into_raw(vec![0;cap].into_boxed_slice())` (2675) | `take` | yes |
| PG19 | pager.rs:2853 | block | `mmap(MAP_SHARED, RW)` of a memfd | — | MAP_FAILED check | yes |
| PG20 | pager.rs:2905 / 2908 | fn + block | `give_back`: `MADV_REMOVE` | whole pages of a shared writable memfd mapping, and no reader | caller PG10 | as PG10 |
| PG21 | pager.rs:3014, 3486, 3705, 3768, 3789, 4258 | block (test-only) | `from_raw_parts` of a paged base; `posix_fadvise` | — | — | test |

**Lifetime caveat for PG3, PG9, PG15 and BY6/BY10.** The slices that `MmapBytes::slice_at`/`piece_at` hand out over a paged mapping or a stream have lifetime `&MmapBytes`. They are actually valid only until the pin ends (cache) or until the holder's next rest (`Stream`). The type does not express this, so a borrow kept across a rest compiles. The module docs, the debug pin assertion, `SUPDB_PAGER_GUARD` and the fuzz are what hold it.

## src/db/vault.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| VA1 | vault.rs:390 | block | `write_volatile` key zeroing | live `&mut` | — | yes |
| VA2 | vault.rs:407 | block | `getrandom(buf[at..])` | pointer and length of the unfilled tail | `at < buf.len()`; `n < 0` handled | yes |
| VA3 | vault.rs:914 | block | `clock_gettime` into a local | — | — | yes |
| VA4 | vault.rs:1859 | block | `Mmap::map(mem)` of a memfd whose writer has finished | not resized or written while mapped | SAFETY comment only | contract |
| VA5 | vault.rs:2943 | block | `Mmap::map(f)` of a store file (log, run) in `File::map` | not cut under the mapping | SAFETY comment only | contract |
| VA6 | vault.rs:2799, 2847, 3080, 3097, 3224 | block | `pwritev` (count capped at 1024), `sync_file_range` ×2, `posix_fadvise`, `syscall(SYS_kcmp)` | `IoSlice` matches `iovec`; no other memory is involved | 2803 `.min(1024)`. `off_t::try_from` at 3093. | yes |
| VA7 | vault.rs:2881 | block | `ManuallyDrop::drop(&mut self.ext)` | dropped exactly once | the only caller is `File::drop` 2765 | yes |
| VA8 | vault.rs:3187, 3197 | block | `open("/proc/self/fd/N")` with a NUL-terminated stack buffer; `from_raw_fd` | NUL termination (the buffer is 24 bytes, of which at most 10 are digits); the fd is fresh | the array literal at 3170, `fd < 0` check | yes |

## src/db/zfile.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| ZF1 | zfile.rs:2730 | impl | `Send` for `TallyPtr(*const StrideTally)` (atomics) | the pointee outlives the memo, wherever it is sent | ZF2's contract | contract |
| ZF2 | zfile.rs:3275 / 3294 | fn + block | `tally_into` stores `&StrideTally` as a raw pointer; `&*t` deref | `t` outlives the memo: a reader-table slot of the `Shared` that the handle holds an `Arc` of | caller db.rs:33802 (out of scope) and test 18640 | contract + test. `StrideTally::add` is a non-atomic load+store (2719), so concurrent counts lose increments, which is benign. |
| ZF3 | zfile.rs:6470 | block | `ZCell::get`: `p.as_ref()` on an `AtomicPtr<ZFile>` (**safe fn** returning `&ZFile`) | the pointer is from `Box::into_raw`, and the epoch frees a taken one only past every pin | the epoch in db.rs. `ZFile: Sync` and `ZCell: Send+Sync` asserted in db.rs:71717-71718 (test `threading`). | contract (pin not typed) + Sync test |
| ZF4 | zfile.rs:6521, 6534 | block | `Box::from_raw(p)` (CAS refused; `Drop`) | `p` came from `Box::into_raw` and is freed once | `install` / `replace` make the pointers. **But `ZCell::keep(p: *mut ZFile)` (6507) is a safe fn** that stores any raw pointer, which `Drop` later frees. The callers in db.rs:69469 and 69551 pass `Box::into_raw` or a pointer taken from another cell. | **no** (type) |
| ZF5 | zfile.rs:10209, 10210 | block | `mem::zeroed::<rusage>()`, `getrusage(RUSAGE_THREAD)` | `rusage` is plain old data | — | yes |
| ZF6 | zfile.rs:12025, 14426, 14566, 18640 | block (test-only) | `read_volatile` inside a lent slice; `posix_fadvise` ×2; `tally_into` | — | — | test |

## src/db/zlz4.rs (LZ4 decoder; input is attacker-controlled file bytes)

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| ZL1 | zlz4.rs:833, 834, 837 | block | the entry point `decompress_in_with` calls `flat_on`, then `fast_loop::<true>` (padded) or `fast_loop::<false>` | `len <= src.len()`; `olim = oend - 64`; padded means `src.len() - len >= PAD_IN (64)`; unpadded means `ilim = min(src.len()-18, len-1)` | 823 `len > src.len()` refused; 826 `oend >= FAST_OUT`; 832 / 835 tests | yes |
| ZL2 | zlz4.rs:947, 948, 953 | block | as ZL1, for `decompress_more_with`, with a caller-supplied `Part` | `st.ip <= len`, `st.op <= dst.len()`, `op <= oend - 64`, `ostop = min(stop, olim)` | 925 and 935 checks | yes |
| ZL3 | zlz4.rs:999 / 1019 | fn + block | `fast_loop`: raw `*sp.add(ip)` token read; 16/64-byte `copy_nonoverlapping` literal moves; `read_unaligned` u16 offset; 8/16/32-byte match moves; `near_copy`; `wild_copy` | see the analysis below | in-loop checks: `offset.wrapping_sub(1) >= op`, which refuses 0 and anything past the output start (1065); `lit > len - ip` (1036); `lit > olim - op` and the unpadded `iend - ip >= 64 && iend - ip - lit >= 32` (1041); `ml > mlim - op` goes to `match_checked` (1082); `ip >= len` exit (1057); `var_len` is fully bounds-checked through `get()` (1487-1497) | yes. Fuzz/tests in `zlz4::tests`. |
| ZL4 | zlz4.rs:1149 / 1159 | fn + block | `flat_on::<FLAT>`: dispatches to `flat_loop`, or to the `#[target_feature(avx2/avx512f)]` copies | the CPU has the feature | `flat_isa()` (769-781) runtime detection, **only** through `decompress_in_flat` / `decompress_more_flat`. The **public safe** `decompress_in_flat_on` (792) and `decompress_more_flat_on` (907) take an arbitrary `FlatIsa` from the caller. | **no** for the `_on` fns (see (a)) |
| ZL5 | zlz4.rs:1180 / 1190 | fn + block | `fast_turn` = one padded fast-loop turn (`ostop = op`) | `fast_loop::<true>`'s, with `op <= olim` | caller flat_loop 1343: `op <= flim <= ostop <= olim` | yes |
| ZL6 | zlz4.rs:1200 / 1210, 1220 / 1230 | fn + block | `flat_loop_avx2` / `flat_loop_avx512` (`target_feature`) | the CPU feature, plus `flat_loop`'s | as ZL4 | as ZL4 |
| ZL7 | zlz4.rs:1259 / 1285 | fn + block | `flat_loop`: branch-free turn; reads token, `e1`, offset and the ml byte without a length check; 64-byte literal move; 2×32-byte match move | `op <= flim = min(oend-128, ostop)`; `ip <= len+2` at a turn (padding is 64) | `oip > len` refused before any read at `lip` (1293); `(lit\|ml) > 64 \|\| offset < 32 \|\| offset > op2` defers to `fast_turn` (1315); final `oip == len` with `lit <= 64` (1296-1303) | yes |
| ZL8 | zlz4.rs:1343 | block | calls `fast_turn` | `op <= flim` | loop guard at 1286 | yes |
| ZL9 | zlz4.rs:1367 / 1369 | fn + block | `wild_copy::<32>`: moves of 32 until `d >= end` | `s` readable and `d` writable for `len+31` bytes | caller: `lit <= olim - op` (dst room 64 ≥ 31); padded `lit <= len - ip` (src padding 64); unpadded `iend - ip - lit >= 32` | yes |
| ZL10 | zlz4.rs:1393 / 1395 | fn + block | `near_copy`: 8 byte-wise copies, then 8-byte moves from `step` back | `0 < offset < 8`, `m = op - offset`, `dp` writable to `op+ml+7` | short match: `ml <= 18`, `op <= olim+14`, so writes end ≤ `olim+14+25 < oend`. Long match: `op+ml <= mlim = oend-32`. | yes |

**zlz4 bounds analysis.** This is the one decoder whose unchecked accesses could see hostile lengths. I traced every unchecked read and write:

- **Turn start.** Each turn starts at `op <= ostop <= olim = oend - 64`, or `<= flim = oend - 128` in the flat loop.
- **Input on the unpadded path.** Each turn also starts at `ip <= ilim <= src.len() - 18`. The token, a 16-byte literal move from `ip+1`, and the 2-byte offset at `ip + 1 + lit` with `lit <= 14` together end at `ilim + 17 < src.len()`.
- **Input on the padded path.** A turn starts at most two bytes past the block. Every unbounded read is within 49 bytes of that start, inside the 64-byte padding.
- **Long runs and long matches.** Their lengths come from `var_len` (fully checked), and each is compared with the remaining room before any move.
- **Match sources.** They are refused unless `1 <= offset <= op`.
- **Overlap.** Each `copy_nonoverlapping` source is at least the move width behind its destination: 8 for 8-byte moves, 16 for 16, 32 for 32. The `near_copy` steps are at least 8.
- **The `match_checked` fallback.** It uses safe slices. The raw pointers are not used after `dst` is borrowed again, because the loop breaks.

No path lets a length or offset from the block's bytes reach a raw access without one of these bounds. The one gap is the CPU-feature contract in ZL4.

## src/db/zpages.rs

| id | file:line | kind | what it does | invariant | established at | checked? |
|---|---|---|---|---|---|---|
| ZP1 | zpages.rs:846, 860 | block | `from_raw_parts_mut(base_mut(), n / room)` over a `GroupBuf` (`Vec<u64>` or a slab cell) | `capacity_bytes() >= n` (resp. `room`), the bytes are initialised (zeroed at creation), the borrow is unique | `bytes_for` grows at 834-840. `room = capacity.min(n+slack)` at 857. `n` is bounded by `ulen <= ZMAX_ULEN` checks (524, 699, 1021, 1148, 1258, 1726), although an unbounded `n` would only change the allocation size. | yes |
| ZP2 | zpages.rs:876 | block | `&mut *(base_mut() as *mut [u8; STRIDE_KEY_MAX])` | capacity ≥ `STRIDE_KEY_MAX` | grown at 872-874 | yes |
| ZP3 | zpages.rs:910 | block | `from_raw_parts(base(), n)` in `GroupBuf::bytes` | `n <= capacity_bytes()` | `assert!` at 907 | yes |
| ZP4 | zpages.rs:5438, 5449, 6351 | block (test-only) | `from_raw_parts(_mut)` over `Vec<u64>` as bytes | — | — | test |

---

## (a) Sites where nothing checks the invariant (no runtime check, no type, no test), most dangerous first

1. **ZL4/ZL6: `zlz4::decompress_in_flat_on` and `decompress_more_flat_on` (zlz4.rs:792, 907).** These are public safe functions (`supdb::db::zlz4` is `pub`). They run the `#[target_feature(enable="avx512f"/"avx2")]` loops (1200, 1220) for any `FlatIsa` the caller passes, with no CPU check. On a CPU without AVX-512 this is UB in practice: SIGILL, or wrong code. `bench/examples/zdecode.rs:209-222` calls `FlatIsa::Avx512` unconditionally (`ours_flat5`, `ours_flat5_end`). It would be sound if these fns were `unsafe` or checked `isa <= flat_isa()`.
2. **BY2: `MmapBytes::open` (bytes.rs:502).** This is a public safe fn mapping any path MAP_SHARED. The same contract sits behind VA4 vault.rs:1859, VA5 2943, PG2 pager.rs:1187 and OF2 onefile.rs:1280. Another process truncating a mapped segment, envelope or data file makes any lent `&[u8]` read raise SIGBUS. Rewriting it in place changes bytes under a live shared borrow, which is UB. The engine itself never shrinks these files, but nothing can stop outside writers (`flock` on `LOCK` binds only cooperating opens).
3. **BY4: `MmapBytes::lend_asked_map` (bytes.rs:586).** It creates one `&[u8]` over a paged mapping's whole range while other threads fill pages (`from_raw_parts_mut` at PG8) and the evictor `MADV_REMOVE`s them. Under Rust's aliasing rules this is UB whatever is read. The SAFETY comment says it is "sound by what is read through it and not by its type". It works in practice only because every read stays inside pages asked for under a live pin.
4. **ZF4: `ZCell::keep(p: *mut ZFile)` (zfile.rs:6507).** It is a safe fn, yet `ZCell::drop` later passes the stored pointer to `Box::from_raw` (6534). Any caller in the `db` module can hand it a pointer that is not a `Box`, or one already retired, and get a double free or an invalid free without writing `unsafe`. Today's callers (db.rs:69469, 69551) are correct. `ZCell::get` (6470) is the same kind of thing: a safe fn handing out `&ZFile` whose validity depends on an epoch pin that the signature does not mention.
5. **W3/W4/W5/W6: wasm exports (wasmapi.rs:202-801).** They trust JS-supplied `ptr`/`len` completely. `supdb_free` or `supdb_open_mem` with a length other than the one allocated corrupts the allocator. A null pointer to any `from_raw_parts`, even with length 0, is UB. Nothing can check this on the Rust side. There are no Rust tests, and the source says CI does not build the wasm module. `web/supdb.mjs` passes allocator pointers today. A smaller bug at the same boundary: `supdb_host_read`'s `off` and `ptr` arrive in JS as signed i32 and are not normalised with `>>> 0` (supdb.mjs:370-383, 412-424). An object or memory over 2 GiB then fails its reads. That is a bug, not UB, but it breaks the CLAUDE.md boundary rule.
6. **FI4: the parallel hash fill (flatindex.rs:1975, 1997).** `out.as_mut_ptr().add(hash_off)` and the per-slot derefs have no check that `hash_off + hash_cap*SLOT <= out.len()`. Only `s < hash_cap` is `debug_assert`ed. The sequential path slices with a panic check. A layout bug would write out of bounds here silently, where the other path panics. The inputs are internal.
7. **DK2: `dirkey::ask<T>` (dirkey.rs:226).** This safe generic fn passes `&mut T` to `ioctl` for any request number. Struct sizes are const-asserted, but nothing pairs `T` with `request`. A wrong pairing in a future caller lets the kernel write past `T`.
8. **BA22: `SlabCell::as_ptr(&self) -> *mut u8` (blocks.rs:1233).** `SlabCell` is `Sync` through BA20. Only `GroupBuf`'s exclusive ownership keeps two writers off one cell. That ownership is enforced at the use site, not by `SlabCell`.
9. **BA23: `munmap` results unchecked in `map_huge` (blocks.rs:1301, 1304).** A `len` that is not a page multiple makes the tail `munmap` fail with EINVAL. This leaks address space and is not UB.

These are guarded only by `debug_assert!` or debug-only machinery, so release builds rely on the discipline alone:
- **PG12/PG14:** `Stream`'s `Sync` and its `UnsafeCell<Ring>` single-thread rule (pager.rs:2392, 2531), with the owner check at 2524.
- **BY6/BY10 and PG3:** the pin required before a paged lend (`debug_assert!(PINS>0)` in `Paged::ensure`, pager.rs:~2077), backed by the evictor protocol, the `SUPDB_PAGER_GUARD` mprotect, and the fuzz.
- **OI4** (ordindex.rs:985), **FI1** (flatindex.rs:221), **BA10** (`regrow`'s `new > old && used <= old`) and **BA24** (`munmap == 0`).

## (b) Lengths or offsets from file bytes reaching unsafe code without a bounds check

**None found.** These are the file-derived paths I traced, each bounded before the unsafe:

- **LZ4 group bytes** pass through `zpages::decode_group` to `zlz4::decompress_in` and its flat variants. All bounded (see the zlz4 analysis). Output sizes come from `ulen <= ZMAX_ULEN` at zpages.rs:524, 699, 1021 and others, and the destination buffer is sized `n` (+slack) by `bytes_for`.
- **Record header `n`** reaches `parse_record_tail` and then FI2 (flatindex.rs:734). Bounded by `rest.get(..n*EXT_BYTES)` with a checked multiply, plus the alignment test.
- **Block table `n`** passes through `MappedBlocks::parse`/`parse_in` and `get` to FI8 (flatindex.rs:3883). Bounded by `sec.range(at, at+BLK_ENTRY)` and `i < n`.
- **Ordered index `n`, `heads_at`** reach OI1 and OI3. Bounded by `HEADER + n*HEAD == body` (ordindex.rs:557-561).
- **Directory word `off`** reaches the BL1 prefetch (blob.rs:3040). Bounded by `.min(recs.len())`.
- **Envelope header** (plain length, chunk shift) reaches CR2, CR3 and PG8. Bounded by `Header::new` (crypt.rs:183-210), `checked_header`'s `src.len() == stored_len()` (crypt.rs:450), `Back::over`'s shift check (pager.rs:1202) and `back.map.get(at..at+n+TAG)` (pager.rs:1594, 1252).
- **Onefile manifest extents** `(base, len)` reach the OF2 mapping. Bounded by `base + cap <= end` in `adopt` (onefile.rs:~1184).

The remaining exposure is not a missing bounds check. It is the mapping contract in (a)2: file bytes that change or shrink after they are checked.

## (c) Pure modules that could run under Miri

These have no file mapping, no libc and no FFI in the code their tests reach. Their I/O goes through `SliceBytes`/`VecBytes`.

| file | unsafe here | test modules (`mod tests` paths) |
|---|---|---|
| src/db/zlz4.rs | 23 (the decoder) | `db::zlz4::tests` |
| src/flatindex.rs | 8 | `flatindex::fill_tests`, `flatindex::record_tests`, `flatindex::cap_tests`, `flatindex::compact_tests`, `flatindex::tests`, `flatindex::block_tests` |
| src/blob.rs | 2 | `blob::ahead_tests`, `blob::tests` |
| src/block.rs | 4 (+1 test) | `block::checksum_tests` |
| src/index.rs | 0 | `index::tests` |
| src/reserve.rs | 0 | `reserve::tests` |
| src/db/zselect.rs | 0 | `db::zselect::tests` |
| src/format.rs | 0 | (none) |

Caveats:
- **zlz4 is the highest-value Miri target.** Under Miri, `flat_isa()` returns `Base`, because CPU feature detection there sees only compile-time features. The AVX2/AVX-512 copies would need `-C target-feature` to be exercised.
- **x86 intrinsics.** `flatindex` and `blob` reach `_mm_prefetch` (flatindex.rs:221, 2425; db.rs `prefetch_lines`), and `block` reaches the `_mm_crc32_*` intrinsics. Check that the pinned Miri supports them, or run Miri on a non-x86 target, where these are compiled out. `flatindex` also spawns `thread::scope` threads for builds of 64 Ki keys or more. Miri handles them, slowly.
- **Not pure:**
  - `crypt.rs`: anonymous `memmap2` plus `libc::madvise`/`mincore`, and `/dev/urandom`. `Header` and `Per` alone are pure.
  - `zpages.rs`: `GroupBuf` can hold a `blocks::SlabCell`, which goes through libc `mmap` at 128 KiB or more, and its tests use `temp_dir` files (zpages.rs:10045).
  - `ordindex.rs`: `vault::map`, `MmapBytes` and `BigVec`.
  - `bytes.rs`, `wasmapi.rs`, `blocks.rs`, `dirkey.rs`, `onefile.rs`, `pager.rs`, `vault.rs`, `zfile.rs`.
