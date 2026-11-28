# `unsafe` inventory: `src/db.rs`

Tree at `98e4e8f4` (74,318 lines). Read by an agent for this audit, read-only; nothing built or run for it. The findings that `unsafe.md` takes from here were checked against the code there.

`grep -n '\bunsafe\b' src/db.rs` finds 198 occurrences on 198 lines, none of them in comments or strings:

- 183 `unsafe { }` blocks. 5 are test-only: 29737, 29743 and 29747 in `store_lock_tests`, `#[cfg(all(test, unix))]`, and 72313 and 72331 in `shared_table`, `#[cfg(test)]`.
- 14 `unsafe impl`: 17081, 17082, 17259, 17260, 27987, 27988, 28261, 28262, 28282, 28285, 28288, 54430, 54431, 55360.
- 1 `unsafe extern "C" fn` type alias (27749 `type Mallinfo2`). This is a type, not an unsafe site. It is listed under U6.
- 0 `unsafe fn` and 0 `unsafe trait` in this file. db.rs calls `unsafe fn`s defined elsewhere: `blocks::populate_write`, `blocks::free`, `Source::free`, `OrdIndex::set_heads` and `ZMemo::tally_into`.

Key to the "checked?" column: **yes** = a runtime check or a type enforces the invariant. **test** = a test (often a `debug_assert` that fires under `--profile checked`) exercises it. **no** = nothing checks it.

"Pin discipline" below means the epoch protocol: a reader pins its slot and then loads the pointer. A retired object is pushed with a tag from `Readers::bump` and freed only when `tag <= oldest pinned` (`RetireList::sweep` 28204, `ByteArena::recycle_below` 17590, `MemTable::reclaim` 19868). It is a runtime protocol, not a type. The borrow handed out is tied to `&self`, not to a pin guard.

## Syscalls, libc, FFI

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| U1 | db.rs:417, 444, 448, 496 | block | `libc::syscall(SYS_ioprio_get/ioprio_set)` on this thread | integer arguments only, no pointer | nothing needed | yes (n/a). Aside: `with_idle_io` does not restore the priority if `f` panics, contrary to "whatever `f` does" at 431 |
| U2 | db.rs:18287, 18303 | block | `SYS_membarrier` register / private-expedited | integer arguments; `asym` set only when registration succeeded | return checked at 18287 | yes |
| U3 | db.rs:54915 | block | `mem::zeroed::<sched_param>()` + `sched_setscheduler(SCHED_IDLE)` | all-zero is valid for a POD C struct; pointer to a stack value | type (POD) | yes |
| U4 | db.rs:27877, 54932 | block | `clock_gettime` into a stack `timespec` | valid out-pointer | stack local, zero-initialised (27877 ignores the result, which is harmless) | yes |
| U5 | db.rs:27758 | block | `dlsym(RTLD_DEFAULT, c"mallinfo2")` | NUL-terminated name | `c""` literal | yes |
| U6 | db.rs:27761 (+ alias 27749) | block | **`transmute::<*mut c_void, unsafe extern "C" fn() -> mallinfo2>`** | the exported symbol named `mallinfo2` has glibc's signature and struct layout | SAFETY comment only; the module is gated on `target_env = "gnu"` (27747) | no |
| U7 | db.rs:27769, 27787 | block | call through the U6 pointer | as U6 | as U6 | no (inherits U6) |
| U8 | db.rs:27805, 27812 | block | `malloc_trim(0)`, `mallopt(M_MMAP_THRESHOLD)` | none (allocator-internal locking) | n/a | yes |
| U9 | db.rs:29737, 29743, 29747 (test-only) | block | `fork`, `_exit`, `waitpid` | the child does no allocation and takes no lock before `_exit` | SAFETY comment; the test body | test |

## `Vec::set_len`

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| V1 | db.rs:9146 | block | `wal_record`: raw `copy_nonoverlapping`/`write_bytes`/`write` into a `Vec`'s spare capacity, then **`set_len(at + total)`** | `total` bytes are reserved, and every byte of `[at, at+total)` is written before `set_len` | `pending.reserve(total)` at 9141. The bytes written are 4+4+8+1+`vn`+`klen`+`value.len()` = `FRAME_HEADER`(8, 8631) + `body`, which equals `total` exactly, so nothing uninitialised is exposed. (`body as u32` silently truncates a frame over 4 GiB: a wrong frame, not UB) | test (`wal_frames` 74001-74009 compares it with `frame`) |

## Mappings of files (MAP_SHARED: an external truncation is SIGBUS)

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| M1 | db.rs:10202 | block | `memmap2::Mmap::map` of the WAL for replay; `&mapped` is parsed in place | nothing writes or truncates the file while the view stands; the view is dropped before the caller truncates or appends | the store `LOCK` via `flock` (`StoreLock::take` 29683), which is **advisory**. Parsing after the map is safe slice code (`wal_head_lost` 9018, `replay_frames` 10236) | in-process: yes, by code order. Another process: **no**, so a truncation is SIGBUS |
| M2 | db.rs:14211 | block | **`slice::from_raw_parts(map.as_ptr(), map.len())`** over a `MmapRaw` of a run's temp file | the length is the mapping's own; nothing writes the file during `walk_run` | the length comes from `MmapRaw` itself. Every parse below is bounds-checked slicing (14217-14249) | length: yes. External truncation: **no** (SIGBUS) |
| M3 | db.rs:13322 | block | **`slice::from_raw_parts(map.as_ptr().add(p), end - p)`** in `RunWalk::bytes_at`: the file's bytes from record `i` to the last good marker | `BASE + offs[i] < end <= map.len()`; nothing writes `[first record, end)` while the walk stands | `walk_run`: `end` is set only at a marker whose length, count and CRC verify (14222-14237). `offs` is truncated to the committed count (14251), so every kept offset is below `end`. `end <= buf.len()` because the loop guard keeps `p + DIRECT_MARK_LEN <= buf.len()` (14222). `resume_run` writes only the head region and past `end` | yes (by construction from verified file bytes). External truncation: **no** (SIGBUS). In release, `end - p` would wrap if the invariant broke; it cannot break as written |
| M4 | db.rs:21752 | block | `MmapOptions::new().len(cap).map(file)`, **deliberately longer than the file** (`RunMap::open`) | no read reaches past `run_flushed`; the file is only appended and never truncated while mapped | runtime `assert!`s, not debug ones, before every read: 20010, 20027, 20600, 19405. Growth comes before the publish of the length (`grow_run_map` 21728) | yes in-process. External truncation: **no** (SIGBUS) |
| M5 | db.rs:15852 | block | `copy_nonoverlapping` of the ordered-index heads from the segment mapping into a `blocks::alloc` buffer | `p` holds `src.len()` bytes; `src` lies within the mapping | `src` comes from `OrdIndex::mapped_heads` (ordindex.rs:994, `.get(HEADER..HEADER+n*HEAD)`). `n` is from file bytes but checked at open (ordindex.rs:560-563: `checked_mul`, and the heads must fill the file exactly). `blocks::alloc` never returns null and asserts `len > 0` (blocks.rs:171-189) | yes. External truncation of the segment file: SIGBUS |
| M6 | db.rs:15855 | block | `OrdIndex::set_heads(p)` (an `unsafe fn`, ordindex.rs:1014) | the copy is whole and outlives every reader that loaded it | the copy completes on the line before. `HeadsBuf` frees it only through the retire list or with the segment (27994) | test (`debug_assert` in `set_heads`; `tests/fuzz.rs` counts `heads_owned`) |

## Memtable arenas and slabs (raw blocks from `blocks::alloc`)

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| A1 | db.rs:17592, 17596, 17622, 17657, 17713, 17772, 17817, 17848, 17858, 19870, 19878, 20307, 20312, 20341, 20356 | block | `&mut *UnsafeCell::get()` on the writer-only cells (`ArenaTail`, `retired`, `free`, `staging`, `retired_runs`) through `&self` | only the single writer thread calls these methods; a shared table's writers use `reserve_shared*`, which never touches `tail` | SAFETY comments, plus the doc above `unsafe impl Sync for MemTable` (17077) | **no** runtime owner check and no type (see I1). The shared-table tests (72414, 72546, 72902, 73005, 73035) run concurrent writers but cannot detect a data race |
| A2 | db.rs:17651 | block | **`from_raw_parts(p.add(w), len)`** in `ByteArena::try_slice` | `w + len <= caps[b]`; the block is not reused while this reader is pinned | `assert!` at 17644 (runtime); null check 17640; pin discipline | yes (bounds); test (reuse: `tests/db.rs:3611` streamed-run recycle) |
| A3 | db.rs:17809, 17889 | block | **`from_raw_parts_mut(p.add(w), len)`**, a `&mut [u8]` built from `&self` (`write_with`, `fill`) | the range is this writer's alone and unpublished; there is no other live `&`/`&mut` to it | `block()` asserts non-null and `w + len <= cap` (17866-17873) | yes (bounds); exclusivity: **no** (discipline only) |
| A4 | db.rs:17897 | block | `copy_nonoverlapping` into a reserved range (`ByteArena::write`) | as A3 | `block()` asserts | yes (bounds) |
| A5 | db.rs:17922, 17927 | block | **`from_raw_parts(p.add(w), CHUNK_HDR)`** and `p.add(w + CHUNK_HDR)` (`chunk`) | `w + CHUNK_HDR <= cap` | `checked_sub(...).expect` at 17916-17919 (runtime) | yes |
| A6 | db.rs:17991 | block | **`from_raw_parts(self.value, len)`** (`ArenaChunk::value`); `len` comes from the chunk header (memory the writer wrote, not file bytes) | `len <= room` | `assert!` at 17987 | yes |
| A7 | db.rs:17939 | block | **`from_raw_parts(p.add(w), len)`** (`ByteArena::slice`) | the bounds, and the SAFETY claim "a block that lives as long as the arena" | `block()` asserts bounds. But the lifetime claim is **false for a recycled stream arena** (`recycle_below` 17590 nulls blocks and reuses their memory). The returned `&[u8]` is tied to `&self`, not to the pin, so it is protected only by the pin discipline | bounds: yes; lifetime: test (partial, `tests/db.rs:3611`) |
| A8 | db.rs:17954 | block | `p.add(w)` for a prefetch | `w < cap` | `if w < cap` at 17952 | yes |
| A9 | db.rs:18096 | block | `p.add(w)` (`Slab::slot_shared`) | `w < SLAB_BLOCK`; `b < SLAB_BLOCKS` | mask, plus `assert!` at 18068 | yes |
| A10 | db.rs:18110 | block | **`&*p.add(id & mask)` → `&T`** (`Slab::peek_shared`) over an entry another writer may be filling | the caller reads only the atomic `head` | SAFETY comment only. The claimant writes the plain fields `hash`, `key_off` and `klen` through raw pointers concurrently (21523-21529, 21594-21597), so a shared `&MemEntry` aliases a concurrent non-atomic write | **no** |
| A11 | db.rs:18156 | block | `ptr::write(p.add(w), v)` (`Slab::push`) into an uninitialised block | the number has not been handed to any reader | writer-only; `debug_assert` of `pushed == id` (18158-18164) | test |
| A12 | db.rs:18185 | block | **`&*p.add(id & mask)` → `&T`** (`Slab::get`) | `id <` the most ever pushed. Non-shared blocks are **not zeroed** (18149), so a larger `id` is a reference to uninitialised memory | only `#[cfg(debug_assertions)] assert!` (18178-18182); release checks just non-null (18184) | test (checked profile only) |
| A13 | db.rs:17274 | block | **`from_raw_parts(self.ptr, self.n)`** over `AtomicU64` slots (`Slots` deref) | `n` words, zero-allocated | `Slots::new` allocates `n*8` zeroed (17265); zero is a valid atomic | yes |
| A14 | db.rs:17281, 17552, 18002, 18009, 18013, 18090, 18206, 27994, 30523, 30698 | block | `blocks::free` / `Source::free` of a block (Drop, or the loser of an install CAS) | same `len`/`mapped` as at allocation; freed once | the owner's Drop; the loser path is never published | yes, by construction. `install_shared` (17552) also relies on "every writer of block `b` asks the same cap" (17536); nothing checks that |
| A15 | db.rs:17362 | block | `madvise(MADV_POPULATE_WRITE)` on the writer's anonymous block (`populate_ahead`) | `[start, stop)` lies in the allocation, rounded to pages | arithmetic at 17352-17357; the advice changes no bytes | yes |
| A16 | db.rs:19017, 19029 | block | `blocks::populate_write` (an `unsafe fn`) on fresh arena/slab blocks | a zeroed mapping nobody else has yet | `populate_shared` runs before the table is published (callers 49568, 73064, 73093) | yes (by order) |
| A17 | db.rs:18863, 19021, 19253, 19862 | block | `&*index` / `&*fresh` index pointer deref | non-null; freed only past pins | `index()` null-checks (19253). 19021 relies on the shared constructor always making an index (18913; only the ordered constructor, 19160, leaves it null, and an ordered table is never shared). 18863 null-checks | yes / test |
| A18 | db.rs:19857 | block | `Box::from_raw(old)` of the swapped-out index, pushed to `retired` | `old` was published by this table and is now exclusively ours after the swap | `swap` at 19853 | yes |
| A19 | db.rs:20295, 20307 | block | `&*run` deref; `Box::from_raw(old)` of the swapped `RunMap` | as A17/A18 | null check 20295; swap 20302 | yes |
| A20 | db.rs:19051, 19060 | block | **`&*(b.as_ptr() as *const AtomicU64/AtomicU32)`**, an atomic made from a `&[u8]` returned by `vals.slice` | 8- or 4-byte alignment; the bytes are only ever accessed atomically once published | alignment: `debug_assert` only (19050, 19059); records are 8-aligned by `reserve_shared_aligned`. Aliasing: a `&[u8]` over bytes other threads modify atomically is a Rust aliasing violation (formal UB, Miri-visible) | alignment: test; aliasing: **no** |
| A21 | db.rs:21161 | block | `&*(p.add(REC_LOCK) as *const AtomicU32)`, `&*(p.add(REC_HEAD) as *const AtomicU64)` (`lean_words`) | `p` is 8-aligned and inside the record header | `p` comes from a slot word decoded as `(low-1)*8` (21074) over a 64-aligned block | yes (by construction) |
| A22 | db.rs:21081, 21083, 21086 | block | `b.add(w)`, `read_unaligned::<u16>`, **`from_raw_parts(p.add(REC_HDR), key.len())`** | `REC_HDR + key.len()` lies within the block | `self.vals.block(r, REC_HDR + key.len())` asserts at 21077 | yes |
| A23 | db.rs:21113 | block | **`p.add(off)` for `_mm_prefetch`, up to `REC_HDR + klen + 2*CHUNK_HDR + LEAN_AHEAD`** (`ask_record`) | the SAFETY comment says "a hint, which faults on no address" | **nothing**. `block()` checked only `REC_HDR + klen`, so `add` can step past the block's allocation (UB for `add`, though a prefetch is harmless on hardware) | **no** |
| A24 | db.rs:21195 | block | **`from_raw_parts(v, len)`** handed to `f` in a non-rewriting table | `len <= room`; nothing writes the bytes again | `assert!` in `lean_chain` (21149); `rewrites_now()` false at 21187 | yes |
| A25 | db.rs:21214, 21228 | block | seqlock reader: `copy_nonoverlapping` of bytes a writer may be writing, then **`from_raw_parts(buf, len)`** | `len <= LEAN_COPY`; the copy is kept only when the lock is even and unchanged | match guard `len <= LEAN_COPY` (21210); fence and re-read (21222-21223). A non-atomic racing read is formally a data race (the usual seqlock caveat) | test (`readers_read_lean_values_writers_overwrite_lean_and_no_read_is_torn` 72902; the `lean_copied` counter 21224) |
| A26 | db.rs:21281, 21289 | block | **`from_raw_parts(old, len)`** to `f`, then `copy_nonoverlapping` over the value in place, through a `*mut` cast of a `*const` | this writer holds the record's lock; the lengths are equal | `shared_lock` (21258); length check 21275; `closed` re-read under the lock (21260). Provenance comes from the block's `*mut u8` | test (72546, 72902) |
| A27 | db.rs:21523, 21594 | block | `addr_of_mut!((*p).field).write(...)` into a shared slab entry | the entry `id` is this writer's alone; it is published by a later release store | `claimed.fetch_add` (21516) | test (shared_table) |
| A28 | db.rs:30567, 30588, 30600 | block | `SnapArena`: `copy_nonoverlapping`, **`from_raw_parts_mut(p.add(w), n)` (a `&mut` from `&self`)**, **`from_raw_parts(p.add(w), len)`** | `w + len <= cap(b)`; the range is reserved by this thread | `SnapArena::block` asserts non-null and the bounds (30540-30554, runtime) | yes (bounds); test (`snap_arena::offsets_cross_blocks_and_two_threads_append_without_overlap`) |

## `MaybeUninit::assume_init`

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| X1 | db.rs:20962, 20984, 35026, 35120, 35126 | block | **`assume_init`** of `MaybeUninit<&[u8]>` for the first `n` entries | the first `n` are written | `live_small_locked` writes `buf[n]` before `n += 1` and returns `None` at `LIVE_SMALL` (20546-20556). 35013-35015 is the copy branch, bounded by `live_copy.len() > LIVE_SMALL` | yes |
| X2 | db.rs:35114 | block | **`assume_init`** of `runs[..held]` | the first `held` are written | `runs[held].write(..); held += 1` (35085-35086) | yes |
| X3 | db.rs:72313, 72331 (test-only) | block | **`assume_init_drop`** on a `Box<MaybeUninit<MemTable>>` | written and not borrowed afterwards | `room.write(...)` just before | test |

## Epoch-protected pointer derefs (pin discipline)

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| E1 | db.rs:31773, 31827, 32430, 32458, 36553, 50537, 50636, 50867, 54993, 62674, 62779 | block | `&*state_ptr` (`&State`) | the caller is pinned, or is the writer, which frees no state while it reads; never null after open | pin before load (31823-31825, 32427-32428, 32455-32456); `retire_state` 61075 | test (`debug_assert` depth in `Reader::state` 31760). The `&State` is tied to `&self`, not to the pin: `release(&self)` while holding it would compile |
| E2 | db.rs:26892, 36915, 37610, 38233, 38350, 38386, 38548, 43073, 43581, 43630 | block | `&*CanonicalForm` from a state's form slot | pinned; a replaced form is freed past every pin | pin discipline; the replace path in `canonical`/`publish` (38382-38392) | test (fuzz / forms tests) |
| E3 | db.rs:37791, 38669, 38745, 43569, 43639 | block | `&*Snapshot` from `state.snap` | pinned; retired at `epoch + 1` with a bump after (37806-37814) | pin discipline | test |
| E4 | db.rs:37769 | block | **`Arc::increment_strong_count(p)` + `Arc::from_raw(p)`** (`adopt_snapshot`) | `p` is still alive between the load and the increment | the pin; retire tag `epoch+1` (37806) | test |
| E5 | db.rs:43598, 43652 | block | `&*rf.0` / `&*rs.0` inside `RetireList::census` | the list is taken by swap, so no sweep frees it meanwhile | `census` swap (28146) | test (`lockfree::a_retire_list_frees_each_item_once_and_never_early` 73571) |
| E6 | db.rs:11196, 11205, 69550 | block | `&*ZFile` to `drop_cache` a form file just taken out of its cell | owned here between `take` and `push` | `ZCell::take` / `replace` | yes (by order) |
| E7 | db.rs:29838, 29853 | block | `&*self.0` in `Drop for Op/WriterOp` (`*const Reader`) | the `Db` outlives the guard (it is made and dropped inside a `&mut self` method) | SAFETY comment; the guard is constructible in safe code from any pointer | no (discipline) |
| E8 | db.rs:33802 | block | `ZMemo::tally_into(&slot)` (an `unsafe fn`, zfile.rs:3275), which keeps a raw pointer to the slot | the slot outlives the memo | the `Arc<Shared>` field is declared before `zmemo` (28468 vs 28599), so it drops first. That is sound only because `ZMemo` has no `Drop` that reads `tally` | yes (no use after drop today; fragile) |
| E9 | db.rs:34808 | block | `read_volatile(&self.attn)` | a reference to its own field | type | yes |

## Ownership reclaim (`Box::from_raw`, `Arc::from_raw`)

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| O1 | db.rs:17032, 28352, 28412, 37821, 50541, 55903, 62765 | block | `Box/Arc::from_raw(p)` of a pointer **never published** (its CAS lost) | not visible to anyone | the CAS failure branch | yes |
| O2 | db.rs:28422, 28438, 55791, 55800, 55808, 55912, 55943 | block | `Box::from_raw` of a pointer **taken by `swap(null)`** | the swap gives exclusive ownership | atomic swap | yes |
| O3 | db.rs:17044, 21699, 21704, 26941, 26948, 28252, 28295 | block | `Box/Arc::from_raw` in `Drop` of the owner (`SetOnce`, `MemTable`, `State`, `RetireList`, `Shared`) | owned, with no reader left | `&mut self` in `Drop`; for `State`, `forms_moved` (26934) avoids a double free of carried forms | yes (type: `&mut`) |
| O4 | db.rs:27970, 28269, 28277 | block | `Drop for RetiredZ/RetiredForm/RetiredSnap`: `Box/Arc::from_raw(self.0)` | each raw pointer is wrapped exactly once and dropped only past every pin | the push sites (11199, 38392, 37807); the sweep | test (lockfree). But the tuple-struct constructors are safe code: wrapping a pointer twice is a double free, and nothing checks it |
| O5 | db.rs:61079 | block | `Box::from_raw(old)` of a swapped-out `State` | the caller swapped it out | `swap_state` CAS (50857) | yes |
| O6 | db.rs:17014 | block | **`Arc::increment_strong_count` + `from_raw`** (`SetOnce::get`) | set from `into_raw`, never replaced, freed only with the cell | `set` CAS from null (17022) | yes (type: the cell owns the count) |

## Lock-free lists and lent cells

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| L1 | db.rs:28152, 28178, 28217, 28227, 28228, 28229, 28232 | block | `RetireList` node walks, splice writes, sweep frees | nodes are reached only after a `swap` took the chain; no single-node pop, so no ABA | `census`/`sweep` swap (28146, 28211); `splice` CAS (28181) | test (73571) |
| L2 | db.rs:54374, 54385, 54403, 54422 | block | `IdCache` node walk / push / drop | nodes are immutable once published and freed only in `Drop` | CAS push (54405); `&mut` in `Drop` | test (`an_id_cache_answers_each_id_with_its_own_value`) |
| L3 | db.rs:30170, 30180, 30191 | block | `FormsCell` `UnsafeCell<Option<FormsState>>`: deref, refill from `&self`, discriminant read | it is emptied only through `&mut self`, filled through `&self` only while empty, on one thread | `Reader` is `!Sync` (it holds many `Cell`s, 28484ff); `lend_fs` takes `&mut self` | yes (type) |
| L4 | db.rs:55767, 55834, 55872, 55888 | block | `Lend` cell write/take by whichever side the state word names | each hand-over is one release/acquire op on `state`; `lend` only when `state == 0` | CAS at 55827 and 55862; `fetch_and` at 55915. `lend`'s precondition is only a `debug_assert_eq!` (55761) | test (checked profile); release: no |

## Prefetch pointer arithmetic

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| P1 | db.rs:25509, 25518 | block | `ptr.add(off)` in **safe `pub(crate) fn prefetch_lines(ptr, bytes)`** | `[ptr, ptr+bytes)` lies inside one allocation ("every address here is inside the buffer") | **nothing**: it is a safe fn with an unchecked precondition. Callers that break it: db.rs:25424 (`ptr.wrapping_sub(lead)` starts before the allocation), 25223/25224 (16 bytes from an element that may be the last), zfile.rs:2961 (`p.wrapping_sub(16)`) | **no** |
| P2 | db.rs:25723, 26014, 40864 | block | `p.add(at)` / `vals.as_ptr().add(v0)` / `list.as_ptr().add(list.len())` for a prefetch | in bounds: `at < n`; `v0 <= vals.len()`; one past the end with `capacity > len` | local checks (25721, 26012, 40861) | yes |

## `unsafe impl Send/Sync`

| id | file:line | kind | what it does | invariant it relies on | where established | checked? |
|---|---|---|---|---|---|---|
| I1 | db.rs:17081, 17082 | impl | **`Sync` + `Send` for `MemTable`** | the writer-only `UnsafeCell`s (A1) are touched by one writer thread. Everything readers follow is release-published after its bytes are written. A recycled byte is reused only past every pin | SAFETY comment (17077-17080) and the type's doc. `MemTable` sits in `Arc`s that the upkeep, seal and reader threads share, and nothing at the type level stops one of them calling a writer-only `&self` method | test (partial). `threading::the_state_is_shared_and_the_handle_is_sent` (71707) asserts the impls exist, and the shared_table concurrency tests exercise them. No owner check |
| I2 | db.rs:17259, 17260 | impl | `Send` + `Sync` for `Slots` (a raw `*mut AtomicU64` block) | words reached only as atomics; freed once in `Drop` | the `Deref` gives `&[AtomicU64]` | yes |
| I3 | db.rs:27987, 27988 | impl | `Send` + `Sync` for `HeadsBuf` (a raw `*mut u8`) | written once before the address is published, read only after | no method writes through it | yes (no mutating API) |
| I4 | db.rs:28261, 28262 | impl | `Send`/`Sync` for `RetireList<T: Send>` | nodes move between threads only through the atomic head; an item is touched only by the one sweep or census that took it. `census` hands `&T` to the calling thread while it holds the item exclusively, so `T: Send` suffices | swap/CAS (L1) | test (73571) |
| I5 | db.rs:28282, 28285, 28288 | impl | `Send` for `RetiredForm` (`*mut CanonicalForm`), `RetiredSnap` (`*const Snapshot`), `RetiredZ` (`*mut ZFile`) | the pointee is touched only to free it, past pins; the pointee is `Send` | `Snapshot` and `ZFile` are asserted `Send + Sync` in the threading test (71713, 71717). **`CanonicalForm` is not asserted** | test (Snapshot, ZFile); no (CanonicalForm) |
| I6 | db.rs:54430, 54431 | impl | `Send`/`Sync` for `IdCache<T: Send + Sync>` | nodes are immutable after publish; values are `Arc<T>` | the bound on `T` | yes |
| I7 | db.rs:55360 | impl | **`Sync` for `Lend`**, unconditional (it holds an `UnsafeCell<Option<FormsState>>`) | only the holder that the state word names reaches the cell; `FormsState: Send` | the protocol (L4); `FormsState: Send` is implied by the `sent::<Reader>()` assertion (71719), since `Reader` holds `FormsCell(UnsafeCell<Option<FormsState>>)` | test |

## Test-only

U9 (29737, 29743, 29747) and X3 (72313, 72331), above.

---

## (a) Sites where nothing checks the invariant (most dangerous first)

1. **A10, db.rs:18110** `Slab::peek_shared` returns a `&MemEntry`. At the same moment the entry's claimant writes `hash`, `key_off` and `klen` through raw pointers (21523-21529, 21594-21597). A shared reference over memory that is being mutated non-atomically is UB. The "caller reads only the atomic" rule is unenforced. If a caller ever reads `key_off`/`klen` through it, it gets a torn or garbage offset, which is then used to index the arena.
2. **A1 / I1, db.rs:17081-17082 and the writer-only cells** (17592 … 20356). `MemTable` is `Sync` by hand. `reserve`, `rewind`, `recycle_below`, `rebuild_index`, `set_run`, `push_chunk` and others mutate `UnsafeCell`s through `&self`, and the only guard is "the one writer". A call from the upkeep, seal or another shared-table thread races on `ArenaTail`, which can hand out overlapping reservations (aliased `&mut [u8]` through A3) or double-push retired blocks, causing a double free. Tests exist but cannot see the race. I list it here because no type or owner check exists.
3. **A20, db.rs:19051, 19060** `&AtomicU64/AtomicU32` made from a `&[u8]` (`vals.slice`) over bytes other threads modify atomically. The alignment is only a `debug_assert`, and the aliasing is formal UB. With a misaligned `at` in release it would be a misaligned atomic (a fault on some targets, a split lock on x86).
4. **P1, db.rs:25509/25518** `prefetch_lines` is a safe fn that does `ptr.add(off)` assuming in-bounds. Callers at db.rs:25424, 25223/25224 and zfile.rs:2961 pass ranges that start before or end after the allocation. That is UB by `add`'s contract; LLVM may assume `inbounds`. The hardware effect is nil. Fix: use `wrapping_add`, or make the fn `unsafe`.
5. **A23, db.rs:21113** `ask_record` does `p.add(off)` past the `REC_HDR + klen` range that `block()` verified, up to `+ 2*CHUNK_HDR + LEAN_AHEAD`, possibly beyond the block's end. This is the same class as P1.
6. **M1, M2/M3, M4, M5, db.rs:10202, 14211, 13322, 21752, 15852** MAP_SHARED mappings of the WAL, the run file, the run map (mapped past EOF on purpose) and the segment's heads. The in-process invariants are checked. The only guard against another process truncating or rewriting the file is the advisory `flock` `LOCK` (29683). A truncation gives SIGBUS on the next touch: a process kill, not silent corruption. For M4, any touch past EOF gives SIGBUS even without truncation, which is why the `run_flushed` asserts are not debug-only.
7. **O4, db.rs:27970, 28269, 28277 (and E7, 29838/29853)** `RetiredZ`, `RetiredForm`, `RetiredSnap`, `Op` and `WriterOp` are tuple structs built in safe code from a raw pointer, and their `Drop` frees or dereferences it. Wrapping a pointer twice, or wrapping one still published, is a double free or use-after-free, and only discipline prevents it.
8. **A7, db.rs:17939** `ByteArena::slice`'s SAFETY claim ("a block that lives as long as the arena") is false for recycled stream arenas. The `&[u8]` is bounded by `&self`, not by the pin, so a caller holding it past an unpin reads memory `block_at` has handed out again. (There is partial test coverage via the recycle test.)
9. **A14 detail, db.rs:17536-17552** `install_shared` stores `caps[b]` before its CAS and assumes every racer asks the same `cap`. If two different caps ever met on one block (a large range and a stride on the same block), `caps[b]` would overstate the allocation and every bounds `assert!` in `block()`/`chunk()` would then pass for out-of-bounds reads. By construction it cannot happen today (large ranges start past `shared_tail` and `made_ahead`); nothing asserts it.
10. **U6/U7, db.rs:27761, 27769, 27787** `transmute` of `dlsym("mallinfo2")` to a fixed fn type. It trusts that the name implies glibc's ABI. Low risk under `target_env = "gnu"`.

Weakly checked (debug-only, so release builds have no check) and worth knowing: A12 `Slab::get` (18185; an id beyond `pushed` gives `&T` to uninitialised memory), L4 `Lend::lend` (55761), A11, and E1 `Reader::state` (31760).

## (b) File-derived length or offset reaching unsafe code without a bounds check

None found in `src/db.rs`. These are the paths where file bytes reach unsafe code, all bounded:

- **Run file → M3 (13322):** `walk_run` maps the file (14205) and builds `buf` from the map's own length (14211). Record lengths come from `RunWalk::parse` → `flatindex::parse_record`, which uses safe `get` slicing (13290-13304). `end` is set only at a marker whose length, count and CRC32 verify (14222-14237). `offs` is cut to the committed records (14251). So `BASE + offs[i] < end <= map.len()` before `from_raw_parts` at 13322. The lengths inside a record (`key()`, 13329) are safe-indexed.
- **WAL → M1 (10202):** only the mapping is unsafe; every frame length is consumed by safe slicing (`wal_head_lost` 9018, `replay_frames` 10236).
- **Segment ordered index → M5/M6 (15852/15855):** `n` comes from the file header and is checked at open (ordindex.rs:560-563: `checked_mul(HEAD)`, and the heads must fill the file exactly). `mapped_heads` slices with `.get` (ordindex.rs:994). The copy length is that slice's length.
- **RunMap → M4:** offsets come from the writer's own references, not from file bytes; runtime `assert!`s against `run_flushed` stand before every read.

The one tangent is P1. `prefetch_lines` is fed lengths derived from mapped segment data in other files (blob.rs:3040; zfile.rs:1099, 5371). Those are hints only and are outside this file's scope. Their pointer arithmetic is unchecked (see (a) 4).
