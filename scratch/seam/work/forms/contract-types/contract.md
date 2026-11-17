# The Lend contract: forms read through one interface whose lifetimes the compiler checks

This document was produced by reading the tree at `3df85d22`, the Pagefile design and experiments.md. Nothing was built, compiled or counted. Wherever a claim depends on the compiler, it is labelled a prediction until S0's count holds it.

## The rule

Encryption lives under two interfaces, and form code does not cross either of them.

- **Read side.** Form code reads a file's bytes only by asking a `Src` with a `Cx<'g>` token.
- **Write side.** Form code writes a file only through a class handle chosen by the file's I/O pattern.

What a form may assume about memory comes from `layer::Memory`, never from the kernel. A plain store and a store under a cipher compile the same form code. They differ only in which `Src`, which `Lend` and which `Memory` the store gives it.

What the compiler carries:
- a form that does not follow the read discipline does not compile;
- a form that borrows across a rest does not compile;
- a form that reads on a thread no one registered does not compile.

What the compiler cannot carry is listed by rule in §6. Each such rule is enforced in plain debug builds and in a paged test mode that every test runs under. A forgotten obligation is therefore a red test in the clear, not a silent difference under a cipher.

## 1. Read side

### 1.1 Tokens

```rust
pub struct Cx<'g>(PhantomData<&'g ()>);              // Copy, zero-sized: a borrow licence
pub trait Lease {                                     // who keeps the evictor waiting
    fn with<R>(&mut self, f: impl for<'g> FnOnce(Cx<'g>) -> R) -> R;
    fn rest(&mut self) -> bool;                       // true: crowded, a builder's cue to stop
    fn idle<R>(&mut self, wait: impl FnOnce() -> R) -> R;   // today's `unlent`
}
impl Lease for Free           { /* ZST: with = f(Cx), rest = false, idle = wait() */ }
impl Lease for &mut pager::Hold { /* today's Hold::rest */ }
impl Lease for &mut Entered   { /* today's slot page_pin / page_idle; rest only at depth 1 */ }
```

**Every read of file bytes needs a `Cx<'g>`, and every borrow it yields is `&'g [u8]`.** The higher-ranked `with` keeps `'g` inside the session, so a session's result cannot name `'g`. `rest` and `idle` take `&mut self`, so the compiler refuses to rest while any session borrow is alive.

This turns CLAUDE.md's argument, that at a rest "every local is a copy, a position or a table", into a type error. The same holds for merge_ranks re-asking its keys after a rest (db.rs:12316), the snapshot of `build_rest`'s locals, and `scan_rested` copying `last`.

**A `Lease` is obtainable in only three ways:**
- `Entered`, the reader-slot guard every operation already makes (db.rs:16351);
- `pager::Hold`, for slotless threads (pager.rs:144);
- `Free`, offered only by `Blob`'s public API over a caller's own source (§1.4).

No function in `db/` mints a `Cx` from nothing. A thread spawned bare therefore holds no `Cx`, and cannot call a reader. Today that mistake is the debug panic at `Paged::ensure` (pager.rs:1316), or a hang in `land_seal`; under this contract it is a compile error.

**Thread registration.** The engine spawns its 23 threads (grep) through one helper:

```rust
fn spawn_reading<F>(name, store: &Shared, f: F)
where
    F: FnOnce(Held<'_>) + Send + 'static,
```

`Held` is `&mut Hold` when the store is paged and `Free` when it is not. The helper's `Held` is the only way such a thread gets a `Lease`.

### 1.2 One byte interface, written once

`Sect<'g>` (flatindex.rs:611) stays the reader's interface, and is implemented for both `&'g [u8]` and `Pieces<'g, B>`. The change is where `'g` comes from: a session's `Cx`, no longer `&'a self` of the source.

**Every reader of file bytes is a function generic over `S: Sect<'g>`.** That covers parsers, walks, `willneed_*`, footprints, CRCs, form page reads, and future structures. Each is monomorphised twice from one body: once for a slice, once for pieces. The kind is still chosen at the refusal (`indexed!`, `scan_either`; blob.rs:941, 2412), so the plain path keeps today's code shape.

**What a plain store gets, and why the lend stays a runtime kind.** The two monomorphs exist today: there are 23 `Sect`-generic functions (maps/readpath.md). `Cx` adds no instantiation, so a plain store's monomorph is the one it runs now.

The lend could have been two types instead, `MmapBytes` against a `PagedBytes`. That would put a match per operation into `Seg`, or a type parameter on `Db`. CLAUDE.md records that the question is cheapest asked at the refusal the path already makes, so the lend stays a runtime kind.

**Whole borrows become private to the interface.** `MmapBytes::as_slice` (bytes.rs:376), `Bytes::lend_at`, `Blob::flat` and `Blob::pieces` become `pub(in crate::bytes)` or `pub(in crate::blob)`, called only from `indexed!` and `scan_either`. A new reader that wants the section whole has no function to call. It writes over `Sect` and gets the paged path for free.

That removes the class of today's silent no-ops:
- `willneed_ranks` and `willneed_dir` answer 0 through `self.flat()?` (blob.rs:1239, 1276);
- `warm_file` sees a length of 0 (zfile.rs:3780);
- the ordered index's skipped CRC (ordindex.rs:413-428);
- the footprint that was zero under a cipher (CLAUDE.md, compact-form section).

These do not compile under the new visibility, and are rewritten over `Sect`.

**Raw slices from a mapping.** `std::slice::from_raw_parts` over a file mapping is allowed only in `bytes.rs` and `pager.rs`. The run's mapping (`RunMap`, db.rs:11436-11470) is the one exception. Its borrows come from an `Owed<'t>` view whose `'t` is the table's, because that backing is never given back while mapped (`MappedFile::owed`). That is a type, not a comment.

The other `from_raw_parts` sites in db.rs (8988, 9242, 9390, 9470, 16848, 16860) are arena or mapping code. Which of them address a file is not verified.

### 1.3 Signatures, before and after

| today | under the contract |
|---|---|
| `Blob::get(&self, key, f)` (via `indexed!`) | `Blob::get<'g>(&'g self, cx: Cx<'g>, key, f: impl FnMut(&'g [u8]))` |
| `Blob::scan_either<F>(&self, from, limit, f)` | `scan_either<'g, F: FnMut(&'g [u8], &'g [u8])>(&'g self, cx: Cx<'g>, …)` |
| `flatindex::lookup<S: Sect<'a>>(s: S, …)` | unchanged; `S` is now built from a `Cx<'g>` |
| `merge_ranks<R: MergeRest>(…, rest: R)` (db.rs:11982) | `merge_ranks<L: Lease>(mut l: L, …)`, a loop of `l.with(\|cx\| walk ≤ MERGE_REST)` then `l.rest()`. `NoRest` becomes `Free` |
| `Reader::scan_rested` (db.rs:20722) | `scan_body` inside `e.with(…)` per `SCAN_REST`, then `e.rest()` at depth 1 |
| `Reader::build_rest(&self) -> bool` | `fn build_block(&self, l: &mut impl Lease, …)` with `l.rest()` between blocks. The upkeep thread, the builder ahead and the writer's own operations hold an `&mut` lease; a caller's callback holds none |
| `Reader::unlent(\|\| wait)` / `unlend()` | `lease.idle(\|\| wait)` |
| `ZFile::page_bytes(&self, page) -> Option<&[u8]>` | `page_bytes<'g>(&'g self, cx: Cx<'g>, page) -> Option<&'g [u8]>`. `None` stays `ZFile::fail` (every site does this already: E0 item 6) |
| `PieceImages` / `Walked::{Mapped,Held}` (zfile.rs:4397) | one `Images` over `Sect`, walked under `L: Lease`. The held build stops being a second build |

**The rule a nested read relies on.** A scan's callback that reads through the same `Reader` gets a fresh `Entered` at depth 2. Its `rest` and `idle` are no-ops, decided by the runtime depth that `scan_in` already checks (db.rs:20770-20777). The types cannot see this, because the public API is `&self` and a callback may capture it. See §6.

### 1.4 Plain cost

- **`Cx` is zero-sized and passed by value.** The Rust ABI ignores zero-sized arguments, so it costs no register even in a function that is not inlined.
- **`Free::with` is `#[inline(always)]` and calls its closure once.** The closure is the walk's only taker. CLAUDE.md's measured costs came from a second taker (3-4 instructions an entry), a closure arm `by_piece` (2 and 7), and an `Option` on the path (3-4). None of those shapes is introduced.
- **The lease is passed by value as a zero-sized type for a plain store**, exactly as `NoRest` is passed today. It is never passed as `&mut Free`, which would be a pointer.
- **A walk split into sessions** (a merge every 256 steps, a scan every 1024) is one session for `Free`, because its `rest` is the constant `false`. The outer loop runs once.

These are predictions. The count that holds them is the §7 rows of design.md, plus the form rows (`read_only.rs` and `lag0_probe.rs` with `supdb-nocachezpages`), plus `merge_walk.rs 100000 4 1` for the session split. Threshold: zero instructions in any function counted per key, entry, block or step.

## 2. Write side: classes are types

```rust
vault::create_once(path)    -> OnceFile     // seg/pcs/par and their temps
vault::create_log(path)     -> LogFile      // run temp, wal-, spare-
vault::create_replace(path) -> ReplaceFile  // manifest slots
vault::create_regen(path, Verify::{Open, Lazy}) -> RegenFile   // .oidx, form files
LogFile::finish(self) -> OnceFile           // consuming: no write after finish compiles
```

**How a file's class is chosen.** It follows the file's I/O pattern, never its form:
- written once and then read: Once;
- appended with commits that must survive: Log;
- rewritten whole in place: Replace;
- derivable from other files: Regen, with Open or Lazy by whether a plain open verifies it whole.

A new file kind therefore needs no layer code. Its author answers four questions about I/O.

**What each handle offers.** Each handle offers only what its contract permits:
- `ReplaceFile` has `replace(bytes)` and no `write_at`;
- only `LogFile` has `set_len`, which is TRUNC as a restart (E4);
- `OnceFile` has `write_at` and patches before finish.

**What replaces `vault::persist`.** Every `sync_data` on a handle is the class's durable sync, so nothing has to be remembered beside an fsync. Advice is a method of the handle (`writeback`, `advise`), so the raw-descriptor sites go:
- `start_writeback`, db.rs:6071;
- `fadvise_dontneed*`, db.rs:14904;
- zfile's `drop_written_range`, `drop_behind` and `swap_ahead`, zfile.rs:3920-4046.

`as_os()` exists only under `#[cfg]` on the plain mmap call sites that design.md §2 lists.

**Output naming needs a witness.** Renaming a `OnceFile` into a manifest-named spelling takes `&Clean`, which is minted by `lease.clean()?`. That is today's `vault::check` (db.rs:12865, 13033), and for `Free` it is a zero-sized `Ok`. A merge that forgets the check does not compile.

**Plain cost.** One match beside each syscall, as design.md §7 says. The risk to count is the handle's width inside `AlignedWriter`, `Wal` and `SegmentWriter`, which can change inlining in `write_buf`. The rows are `work_probe.rs 100000 whole`, `load_only.rs`, and a seal probe.

## 3. WAL and manifest (decision 2)

Under the contract they are a `LogFile` and a `ReplaceFile`, so the answer is option **(a)**.

The reason is the requirement, not tidiness. `LogCipher`, `passes()` and `seal_manifest` are form knowledge in the vault: a frame shape, a recycle rule (vault.rs:562; db.rs:4103-4580). A WAL change such as a new frame layout, a chain byte or a new rotation shape is an encryption change for as long as they exist. The branch's recycled-log work is the precedent.

Gate it as design.md does: after E4 holds TRUNC, finish and the recycle lag, and after the fuzz runs with fault injection. Until then a WAL change is not free, and that should be said.

## 4. Models that assume the page cache (decision 3, E7)

The forms' models read one interface:

```rust
trait Memory {
    fn budget(&self) -> u64;          // bytes a resident page of the store may be assumed to cost
    fn misses(&self) -> u64;          // reads that went to the device, by this thread
    fn miss_price(&self) -> Duration; // what one costs
    fn resident(&self, src, off, len) -> f64;
}
```

**The plain implementation is today's code, moved:**
- the cgroup limit less anonymous memory (zfile.rs:5199-5270);
- `ru_majflt` (zfile.rs:4942-4955; db.rs:18384-18390);
- `mincore`.

`zselect`, the sampler, `choose_coding`, the builds ahead and the drop-behind call only this interface. A gate test bars `/proc`, `cgroup`, `getrusage` and `mincore` outside `layer/`.

**The cipher implementation is the layer's own business.** E7 found the bound split into 337 MiB of plaintext and 240 MiB of ciphertext, which accounts for 69% of the out-of-core gap. A lever on that is D8 (drop the ciphertext behind the fill, or fill by `pread` and count `ru_inblock`). Under this contract that lever is an edit to `layer::Memory` and the pager alone, and no form changes. Today it would touch the controller, the census and the sampler.

The controller stays off under a cipher until its model is held to E7's measurement: faults 0.433-0.447 a read. Turning it on becomes a layer change too.

## 5. Enforcement

- **The compiler:**
  - no `Cx`, no read;
  - no borrow across a rest or an idle;
  - no whole borrow outside `indexed!` and `scan_either`;
  - no write after `finish`;
  - no output named without `Clean`;
  - no Replace written in part;
  - a class is required at create;
  - a const assert that the longest logical name (`51 + 4·FENCE_MAX`) fits the layer's inline name budget;
  - a const assert that the form page size is a multiple of `layer::PAGE` (4096).
- **Debug assertions, plain builds included:**
  - a `Lease` dropped without `finish(result)`. That is today's `end_slow` (db.rs:18134): the answer passes through the lease's check;
  - the class contracts: a Log file's override count, Replace written whole, Once not read-trusted before finish;
  - a session longer than N ensured bytes without a rest, so a missing rest shows up as a test failure;
  - a wait primitive called while the thread holds an active session.
- **Gate tests:**
  - the door test (vault.rs:2876-2905) widened to all of `src/`;
  - the same test barring `as_raw_fd`, `posix_fadvise`, `sync_file_range`, `File::open`, `from_raw_parts` outside the allowlist, and std `Condvar::wait`, `JoinHandle::join` and `thread::sleep` in `db/` outside the wrappers that take `&mut Lease`;
  - a `the_longest_logical_name_seals_under_name_max`-style budget test.
- **A paged test mode** (`cfg(test)` or `Options::test_paged`). An identity cipher, plain names, `lend = 0`, a cache of 16 KiB and `SUPDB_PAGER_GUARD` semantics. `scripts/check.sh` runs the whole test group once under it. Every new reader, arm and job is then exercised by the ask path without its author writing a cipher test.
- **Fuzz.** `ZFUZZ_CIPHER=1` draws options as the plain fuzz does (D11), with fault injection.

## 6. What the types cannot carry

| rule | why not | carried by |
|---|---|---|
| a nested read in a callback must not rest | the public API is `&self`, and a callback may capture it | runtime depth in `Entered` (exists: db.rs:20770); a paged-mode test of a scan from a scan's callback |
| rest often enough | frequency is not a type | the debug "ensured bytes since rest" assertion in paged mode |
| a timeslice lost while pinned | invisible to code | nothing; `longest_wait_us` |
| choosing the right class | semantics | the four-question table; the debug class contracts in plain |
| `Option` meaning both absence and damage | readers return `Option` | per-file failure in `Paged::slow` (E0) plus the lease's end check |
| a model that is wrong about two caches | measurement | E7 numbers held in a test of `layer::Memory` |
| a form's speed under a cipher | measurement | a cipher twin of each new arm in the pairing a change cites (owed; not automated) |
| plain-path instruction count | the compiler does not count | callgrind rows per function |

## 7. History items and future changes, answered

- **Kept read side.** `Sect`/`Pieces`, `scan_pieces`, the rests and the holds stay. They stop being a discipline because they become the only API. A new reader can only be written over `Sect`, a new walk only rests through a `Lease`, and a new thread only reads through `spawn_reading`.
- **Silent no-ops.** `willneed_*`, `warm_file`, the CRC skip and the zero footprint are fixed by the visibility change, as §1.2 says.
- **The held build.** The controller's held build (`Walked::Held`) merges into the one build generic over `Lease`.
- **Things design.md says need a new layer verb.** Each of these is a change to the layer alone, and never to a form:
  - a value log;
  - O_DIRECT or io_uring;
  - more than 4 patches;
  - truncating a large log;
  - a checkpoint across directories;
  - a lock;
  - a read that does not keep what it reads.

  The contract asks that the override cap be a capacity of the layer, held by a debug assertion in plain builds. It must not be a number tied to the run's two finish patches.
- **Longer fences.** A logical name longer than the inline budget is sealed into the file's header and flagged in its entry. The directory entry stays short, and `FENCE_MAX` decouples from NAME_MAX.
- **Unchanged.** The 4080-byte envelope geometry exists only while envelopes do (S6).

## 8. Migration (each step gated on the plain count being level)

- **L0.** Introduce `Cx`, `Lease`, `Free`, `spawn_reading`, and lease-passing signatures, with no behaviour change. `MergeRest` becomes `Lease`. Count the rows.
- **L1.** Make whole borrows private. Rewrite `willneed_*`, `warm_file`, the footprint and the ordered index's CRC over `Sect`.
- **L2.** Typed class handles over `File::Os` (design S1). The `Clean` witness.
- **L3.** `layer::Memory`, with the cgroup and fault code moved behind it.
- **L4.** Paged test mode in `check.sh`; the widened door test; the cipher fuzz with option draws.
- Then Pagefile's S2-S6 proceed. A form change after L4 compiles against the interfaces above, and the gate exercises it paged.

**Not verified:**
- that `with` and the zero-sized lease leave the plain loops' instruction counts unchanged;
- how far the `&mut` lease can be threaded through `Reader`'s `&self` builders without restructuring;
- which `from_raw_parts` sites address files;
- the cost of running the full test group twice in the gate.
