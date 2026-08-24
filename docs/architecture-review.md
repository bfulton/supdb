# Supdb: architecture review

A critique of the design document and prototype, in three parts: whether each architectural
decision is defensible against the state of practice, what the gaps are, and what a
benchmark program would have to contain to constitute proof.

Read against the artifact and the ~1,400 lines of engine and ~2,100 lines of harness in it.
Everything below marked **[code]** was read off the prototype source rather than inferred
from the prose.

---

## Verdict

The document is unusually honest and the central mechanism is real. Three decisions —
chunk-granular compression inside a packed block, the `last` offset in the extent, and
sorting the seal batch — are correctly reasoned, cheap, and genuinely good. The "What I got
wrong" section is better methodology than most published storage-engine work.

But the honesty is unevenly applied. It is thorough about *benchmark* error and thin about
*architectural* error. Three things are true at once:

1. **The design is not yet a database.** There is no way to reopen an existing store for
   writing. `Store::create` unconditionally truncates and there is no `Store::open`. **[code]**
   Every benchmark writes a fresh file. This is not in the Known Gaps list and it is larger
   than everything that is.
2. **The reader-side design contradicts its own premise.** The stated architecture is
   LMDB's — mmap, one writer, many reader *processes*. But each reader materializes the
   entire key index on the heap: one `Vec<u8>` allocation per key plus a `2N`-slot hash
   table, built at open. **[code]** That is Bitcask's design, not LMDB's, and it carries
   Bitcask's costs: `O(N)` open, and per-process RAM proportional to key count with nothing
   shared between processes. Ten reader processes means ten copies of the index.
3. **The headline claim is not supported by the measurements shown.** "Beats RocksDB on
   every benchmark in RocksDB's own `db_bench`" is four benchmarks out of roughly forty,
   single-threaded, memory-resident, with `db_bench`'s open cost excluded from a timer where
   Supdb's open is `O(N)` and RocksDB's is not — and one of the four wins (13.9%) falls below
   the document's own stated significance threshold of 15%, reported without error bars.

None of this makes the idea wrong. It makes the current evidence much narrower than the
document's framing, in a document whose main asset is that it doesn't do that.

---

# Part 1 — The architectural decisions

## 1.1 Decisions that hold up

These are correct, well-argued, and I would not change them.

**Chunk the compressed block, and make the chunk small.** This is the actual discovery of the
project. Decoupling the compression window from the read granularity is the right resolution
of a genuine tension, the measurement (251k → 402k reads/s at 1 KB) is convincing, and the
chunk directory is stored in the block so old blocks stay readable when the dial moves. Good.

**Carry `last` in the extent.** Four bytes for an `O(1)` `read_last` on a run of hundreds.
Correct and cheap.

**Sort each seal batch by key.** Free at seal time, unrecoverable later, and it is what makes
the block-local scan locality work. Correct.

**Solo blocks bypass the block cache.** "A solo block serves exactly one key, so caching it
can never produce a hit for another key while evicting one that would" is exactly right and
is a genuinely subtle observation.

**Reuse and relocate rather than punch holes.** The conclusion is right, and for a better
reason than portability: a released block is still valid data until something overwrites it,
and hole-punching destroys that immediately. The insight that a block is named by id so
relocation touches one index entry is the right structural property.

**The visitor API, and the confession about it.** Moving from allocation-per-value to a
visitor was worth 6×, and *the same handicap was left in the LMDB adapter and inflated the
comparison*. Disclosing that is the single most credibility-buying sentence in the document.

## 1.2 Decisions where the conclusion is right and the argument is not

### Compressing across keys rather than within one

The measurement is decisive (960 MB → 1,245 MB compressed vs 1,242 MB uncompressed) and the
conclusion — a compressor needs a window — is correct.

**What's missing:** the state-of-practice answer to "my records are too small for the
compressor to find anything" is not "pack them together." It is **a trained dictionary**.
Zstd's dictionary mode exists specifically for the sub-kilobyte-record case, RocksDB exposes
it as `compression_dict`, and it typically delivers 2–4× on records where undictionaried
compression delivers nothing — while preserving per-record decode granularity, which is
exactly what the chunking machinery was then built to recover.

The document never considers it. Packing is probably still the right call (it also buys I/O
and cache locality that a dictionary does not), but the argument as written establishes
"per-key raw LZ4 fails," not "packing is the best available fix." The missing arm is
**per-key zstd with a trained dictionary**, and it directly tests the architectural premise:
if it matches packed-and-chunked on both ratio and read amplification, a large amount of the
block/chunk/solo machinery is unnecessary.

Related: the engine is LZ4-only, with no codec identifier reserved in `BlockLoc`'s flag byte
(only `solo` and `chunked`). **[code]** Zstd at level 1 is competitive with LZ4 on speed and
substantially better on ratio, and file size is the one axis the document concedes.

### Merge a key's extents inline, past a threshold

The per-key, demand-driven trigger is a good idea and the sharp-knee table (threshold 4/8/16/32)
is a useful measurement.

**The supporting argument is wrong.** "A batch compaction measured 18.7 seconds, which is a
stall whether or not a separate thread runs it" is not true. A background thread converts a
18.7-second stop-the-world stall into 18.7 seconds of background work overlapping foreground
progress on other cores — on 4 cores that is a ~25% throughput tax, not a 100% latency event.
The real arguments for inline merging are *predictability*, *no daemon*, and *cost
proportional to damage*, and those are good arguments. The one given is not.

**The real cost of the choice is unmeasured.** `merge_key` synchronously reads, decompresses,
concatenates, recompresses and writes a key's whole run **while holding both the shard lock
and the appender lock**. **[code]** That is a multi-millisecond stall on an arbitrary
unlucky `append`, blocking every other writer thread. **There is not a single latency
percentile anywhere in the document — every number is a throughput mean.** For an
ingest-optimized engine, p99.9 append latency is the number that decides whether it is usable,
and it is the number that inline merging is most likely to lose on.

**The write-amplification comparison is apples-to-oranges.** "A levelled LSM typically runs
10–30×; this is 1.15×" compares a device-level, whole-lifetime figure to one computed from
file size over a run that ends while the dataset still fits in RAM, with a 512 MB write buffer
absorbing the fragmentation that would otherwise force merges — and without producing the
global sort order or bounded read amplification that LSM compaction is buying with that 10–30×.
The honest comparators are size-tiered/universal compaction (which also lands at ~2–5×), and
the honest measurement is **bytes actually written to the device**, from `/proc/diskstats`,
not inferred from file length.

The knee itself deserves root-causing rather than tuning around: 8 → 16 extents doubles the
work but costs **6.5×** in read throughput (39,078 → 6,008/s). That nonlinearity is a bug
signature, not a tuning curve. Likely candidates: `Extents` spilling from the inline `One`
variant to a heap `Vec`, and 16 independent chunk decodes per read with the block cache
bypassed (see 2.8).

### Do not use a multiply-rotate hash

The observation is real and the 10× regression is worth recording. **The mechanism given is
wrong, and the fix chosen is the slow one.**

FxHash's weakness is not "multiply-rotate clusters on decimal keys." It is that FxHash has
**no finalizer**: the last input word passes through one multiply, so the low bits — precisely
the bits `h & mask` selects for a power-of-two table — are barely mixed. Any structured key
set that varies in its last bytes hits this. The document generalizes from the right
observation to the wrong rule ("avoid multiply-rotate"), when the actual rule is "avoid an
unfinalized hash, or don't take its low bits."

The state-of-practice fixes, in order: **wyhash / xxh3 / rapidhash / komihash** (all finalized,
all handle a 16-byte key in ~5 cycles), or FxHash plus a `fmix64` finalizer, or simply taking
the *high* bits. FNV-1a is the slow option: byte-at-a-time with a serially dependent multiply,
so a 16-byte key costs ~16 dependent multiplies (~80 cycles of pure latency) **on the critical
path of every put and every get**. Given that the key table measured 463 ns/put and 17% of the
write path, this is plausibly a measurable fraction of it.

And the codebase now contains **three different, inconsistent hashes** **[code]**:

| site | function | bits used for the slot |
|---|---|---|
| `store::shard_of` | FNV-1a, no finalizer | `h >> 32` (high) |
| `keytable::hash` | FNV-1a **+ `h ^ (h>>29)`** | `h & mask` (low) |
| `store::key_hash` (reader) | FNV-1a, **no finalizer** | `h & mask` (low) |

The reader's hash is strictly weaker than the writer's and uses the low bits — the exact
combination measured as catastrophic. It probably works, because FNV-1a's final xor touches
the low byte directly, but it is unexamined and it should not differ from the writer's.

There is also **no seed**. **[code]** An embedded store accepting arbitrary user keys with an
unseeded, non-cryptographic hash and linear probing has an unbounded worst case. A per-store
random seed costs nothing.

The 20-minute experiment that would replace this anecdote with a result: {FNV, FNV+finalizer,
FxHash, FxHash+fmix64, wyhash, xxh3} × {fixed-width decimal, sequential u64 BE, UUIDv4,
UUIDv7, reverse-domain, adversarial collisions}, reporting throughput **and mean/max probe
length**. Probe length is the diagnostic; throughput alone is what produced the wrong
generalization in the first place.

### Gate reuse on registered readers, not on a timer

Replacing a magic constant with a published generation is the right move, and the document is
right that this is what LMDB does. **The implementation is the weakest of the three known
options**, and it has two reachable holes (detailed in Part 2, §2.2 and §2.3).

The three options, ranked:

1. **Epoch-based reclamation** (FASTER, SIGMOD'18; also RCU, and the general EBR literature).
   A monotonic epoch counter, no wall clock, no timeout, provably safe. This is the right
   answer and it is a published framework solving exactly this problem.
2. **LMDB's actual mechanism** — a reader table where liveness is decided by **POSIX file
   locks held by the reading process**, so the OS reclaims a dead reader's slot at process
   exit, deterministically. `mdb_reader_check` exists precisely because timeouts don't work.
3. **What Supdb does** — a 30-second wall-clock heartbeat that is **never refreshed after
   acquisition** **[code]**, so a reader alive longer than 30 seconds is declared abandoned
   and has its data reused underneath it.

The document says the table "replaces a guess with the actual answer." It replaces one guess
(8 checkpoints) with another (30 seconds), and it did not notice because the harness that
found every other defect opens and closes readers in a tight loop and so can never hold one
for 30 seconds.

## 1.3 Decisions that are not argued at all

### mmap

This is presented as inherited from uppend rather than chosen. It is the single most
consequential decision in the engine and it needs to answer Crotty, Leis and Pavlo, *"Are You
Sure You Want to Use MMAP in Your Database Management System?"* (CIDR 2022), which enumerates
four failure modes. Supdb's position on each:

| Crotty et al. | Supdb |
|---|---|
| **Transactional safety** — the OS can flush dirty pages at any time | **Avoided, and this deserves saying.** Readers map read-only; the writer uses `pwrite`, never stores through a mapping. This is the correct design and the document should claim it. *Except* the reader table, which every reader maps **read-write over the whole file** (§2.6). |
| **I/O stalls** — every access can major-fault, invisibly, with no async I/O, no prioritization, no controlled readahead | **Fully exposed.** There is not one `madvise` call in the engine. **[code]** This is exactly why "cold data larger than memory" is unmeasured, and it is why that measurement will be unflattering. |
| **Error handling** — a failed page read delivers **SIGBUS**, not an error | **Fully exposed, and reachable.** See §2.5: the writer truncates the file while readers may have it mapped. |
| **Performance** — page-table contention, single-threaded eviction, TLB shootdown | Unmeasured; only shows up out-of-core and multi-threaded, neither of which was run. |

mmap is still defensible — LMDB is the existence proof — but LMDB pairs it with a
**never-shrink-under-readers** invariant and **process-lock liveness**, and Supdb has adopted
neither.

### No checksums on data

The 120-byte superblock is checksummed with FNV-1a. **Nothing else in the file is.** **[code]**
No block checksum, no index checksum, no per-chunk checksum. RocksDB checksums every block
(crc32c/xxh3) and this is not optional in any modern engine.

The consequence is worse here than elsewhere, because the engine's whole read path is
zero-copy from a mapping: a bit flip, a misdirected write, a partially written block after a
crash, or a reused-space collision produces either a decompression failure (best case), a
panic (§2.9), or **silently wrong data returned to the caller** — LZ4 will happily decode
many corrupted inputs into plausible bytes. The code's own comments acknowledge it is decoding
bytes that "may not be a chunk directory at all"; the answer to that is a checksum, not
defensive length parsing.

FNV-1a is also the wrong function for the superblock. It is fine for detecting a never-written
slot, which is all the comment claims, but crc32c is hardware-accelerated and strictly better.

### Where Supdb sits in the literature

The document has zero citations, which for a design claiming rigor is itself the gap. The
lineage is clear and naming it would sharpen the argument rather than weaken it:

- **Bitcask** — append-only data files plus a fully in-memory hash index of key → (file,
  offset). This is precisely Supdb's reader. Bitcask's known costs (RAM ∝ key count, slow
  startup, no ordered iteration) are Supdb's, and Bitcask's mitigation — **hint files**, a
  compact prebuilt index image — is the direct answer to Supdb's `O(N)` open.
- **WiscKey** (FAST'16) — key-value separation: index points at values in a log. Supdb's
  extents are this, and WiscKey's central unsolved problem is **value-log garbage collection**,
  which is what `freelist.rs` + `defragment()` are. Same problem, same lineage.
- **LMDB** (Chu, 2011) — cited implicitly for the reader table; should be cited explicitly,
  including for the two mechanisms not adopted.
- **FASTER** (SIGMOD'18) — log-structured store + in-memory hash index + **epoch protection**.
  The closest modern analogue, and its epoch framework is the rigorous version of `readers.rs`.
- **The RUM conjecture** (Athanassoulis et al., EDBT'16) — read/update/memory, pick two.
  Supdb picks read and update and **spends memory** (a fully resident index). Stating that
  explicitly reframes several "wins" as trades, which is more defensible than presenting
  them as free.
- **Monkey** (SIGMOD'17) / **Dostoevsky** (SIGMOD'18) — the levelled↔tiered continuum has a
  closed-form cost model. `merge_threshold` is a point on it; four measurements where an
  analytic model plus measurement is available is the difference between tuning and rigor.
- **PebblesDB** (SOSP'17) — sorted fragments without full compaction, which is exactly the
  acknowledged "sorted runs but no global order" gap.
- **SuRF** (SIGMOD'18) — succinct range filters. The `readmissing` win should be stated as
  "we replaced Bloom filters with a fully resident index, trading memory for filter-free
  misses," which is a RUM statement, not a free win.

---

# Part 2 — Defects in the prototype

Ranked by severity. All read off the source.

### 2.1 A store cannot be reopened for writing — **critical, and not in Known Gaps**

`Store::create` opens with `.truncate(true)` and initializes `generation: 0`, `blocks: vec![]`,
`off: SUPER`. There is no `Store::open`; `Store::reopen` returns a `Reader`. So the only way to
get a writable store is to destroy the existing one.

Everything downstream is affected: "crash recovery" means *a reader can read what a crashed
writer left*, not that the store resumes. `recover.rs` is 20 lines and opens a `Reader`.
The soak, the concurrency harness and every benchmark write from scratch. Whatever the
recovery path costs — rebuilding the key table, the block table, the free list, and the live
refcounts from the last superblock — is unwritten and unmeasured.

### 2.2 A reader loses its protection after 30 seconds — **critical**

`readers::acquire` writes the heartbeat once at claim time. **No code anywhere refreshes it.**
`STALE_MILLIS = 30_000`, and both `acquire` (for slot stealing) and `oldest` (for the reuse
floor) treat a slot older than that as abandoned. A reader held open for 31 seconds — an
ordinary analytical scan — is silently dropped from the reuse floor and has its blocks
rewritten underneath it.

The concurrency harness cannot catch this: it opens and closes readers in a tight loop.

### 2.3 `Reclaim::AfterReads` is missing the bound that `AfterDelay` has — **high**

```rust
Reclaim::AfterReads => live().unwrap_or(self.generation),
Reclaim::AfterDelay(n) => { let by_delay = …; live().map_or(by_delay, |o| o.min(by_delay)) }
```

The comment on `AfterReads` explains correctly why the floor must not exceed the current
generation ("a reader may have opened on that checkpoint a moment ago and not yet claimed its
slot") — and then applies that bound **only when no reader is registered**. With one or more
readers registered, `live()` returns `Some(oldest)`, which can be *ahead of* a
just-arriving reader's generation, and the newly opening reader races unprotected.
`AfterDelay` gets this right with `.min()`. One-line fix:
`live().map_or(self.generation, |o| o.min(self.generation))`.

### 2.4 The 65th reader is unsafe, not degraded — **high**

`SLOTS = 64`. When the table is full, `acquire` returns `None`, `Reader::claim` returns `None`,
and the reader proceeds **unregistered**. The comment says it then "falls back on the grace
window for safety," but under `AfterReads` there is no grace window whenever another reader is
registered — `live()` returns a floor that ignores the unregistered reader entirely. Exceeding
the reader limit should fail the open or block; it currently returns a reader that can be
overwritten.

### 2.5 `defragment()` and `close()` destroy older states silently, and can SIGBUS live readers — **high**

`defragment()`:
- moves live blocks into holes and **records nothing in `reuse_log`**, unlike `write_block`,
  which pushes `(off, cap, generation)` on every reuse. So the `Reader::is_overwritten` guard
  — the entire mechanism for making a stale snapshot fail loudly instead of returning wrong
  bytes — is blind to defragmentation.
- **ignores `self.opts.reclaim` entirely.** It will relocate blocks and truncate under
  `Reclaim::Never`, the policy whose documented contract is "never reuse, and never release."
  This is a recurrence of the exact defect the document confesses to ("a retention policy
  promising never to release space was releasing it anyway").
- calls `file.set_len(end)`. `close()` does too, twice. **Shrinking a file that readers have
  mapped means SIGBUS on next access**, not an error — the process dies. LMDB never shrinks
  under live readers for this reason.

`defragment()` is also `O(max_moves × nblocks)`: it rescans every block to find the best fit
on each move.

### 2.6 Every reader maps the entire file read-write — **high**

```rust
fn claim(path: &Path, generation: u64) -> Option<(usize, MmapMut)> {
    let table = unsafe { MmapMut::map_mut(&file) }.ok()?;
```

To publish 32 bytes into the reserved first page, each reader obtains a writable mapping of
the **whole database**. Any wild write in the host process — and this is an embedded library
living inside someone else's address space — corrupts arbitrary data, with no checksum to
catch it. Should be `MmapOptions::new().len(SUPER).map_mut()`.

### 2.7 The key index is fully materialized per reader — **high, architectural**

`Reader::build` decodes the index into `entries: Vec<(Vec<u8>, Extents)>` — one heap
allocation per key — then builds a `2N`-slot hash table over it. At 10M keys that is 10M
allocations, several hundred MB of RSS, and an open cost linear in the key count, **paid per
process, shared with nobody**.

This is the deepest tension in the design. The premise is LMDB's many-reader-process model;
the implementation forfeits the property that makes that model work, which is that LMDB's
B+tree lives *in the shared mapping* and costs a second reader process essentially nothing.

The fix is well-trodden: serialize the index as a **mmap-able, binary-searchable, zero-copy
structure** — a prefix-compressed sorted key block with a sparse restart array (LevelDB's
block format), an FST (Lucene's terms index), or a learned index (RadixSpline / PGM) — so
open is `O(1)` and the index is shared across processes by the page cache. This also
subsumes the acknowledged "checkpoint writes the whole key index" gap, since a shared,
persistent index format is a prerequisite for making it incremental.

**DONE — and the simplest of those options won.** `indexlab` measured ten candidate
layouts before anything was built, and the prefix-compressed and learned-index families
both lost to the plainest one: an open-addressed hash of (tag, record offset) over a flat
blob of fixed-width records. `src/flatindex.rs` is that, and `Options::flat_index` is on by
default. At 5M keys, `--profile full`:

| | decoded | mapped |
|---|---|---|
| open | 738 ms | **0.29 ms** (2537×, p=0.0022) |
| point read | 2334 ns | 1893 ns (1.25×, p=0.0022) |
| index | 186 B/key resident, per process | **57 B/key, file-backed and shared** |
| file | 394 MB | 683 MB (+73.5%) |

**F2.2 moved from `fails` to `holds`** — reader open is sub-linear in key count. F2.1 still
fails: 20× for 100× the keys is sub-linear, not independent, and what remains is the *block*
table, still decoded per entry. F7.1 and F7.2 also still fail, because an index of N keys
holds N records and 57 B/key is above that claim's 32-byte bar — but the per-process
multiplier this section objects to is gone. Ten reader processes now share one copy.

The price is +73.5% on disk, paid deliberately: a section read in place cannot be
compressed. Space is the axis this engine has to spare.

Two things found on the way are worth more than the speedup:

- **A pre-existing leak.** Every checkpoint appended three index sections and released
  none of them — 9.2 B/key per checkpoint, forever, in the shipped engine. It hid under a
  compressed index for the project's whole life; the flat format made it seven times
  dearer and therefore visible. Sections are now reclaimed and both arms measure zero
  permanent growth per checkpoint. This is the space half of the "checkpoint writes the
  whole key index" gap named above; the time half is still open.
- **`history_from` was a lie.** It is the field that says how far back time travel is
  intact once space is reclaimed, and it was hardcoded to zero — claiming unlimited
  history while, by its own documentation, reclaimed blocks could already have been
  overwritten. It now reports the truth.

### 2.8 The block cache is configured, documented as central, and effectively unused — **medium**

- `Options::cache_blocks` is declared and defaulted to 4096 and **never read**. `Reader::build`
  hardcodes `BlockCache::new(4096)`.
- `read_all` routes `loc.chunked || loc.solo` to the thread-local `SCRATCH` path, which never
  consults the cache. **Every compressed block larger than `chunk_size` is `chunked`** — that
  is the entire packed-block population, i.e. the dominant read path. The `self.block(id)`
  call that uses the cache is reached only for compressed blocks *smaller than one chunk*.
- `scan` uses its own single-block `cached` variable, not the cache.

So `BlockCache` is near-dead code, while `lib.rs`'s finding #4 — "a store that compresses has
to choose between size and warm reads unless it caches *decompressed* blocks. RocksDB gets
both for exactly this reason" — presents it as the reason the design works. The warm-read wins
actually come from chunk-granular decode into scratch. That is arguably a *better* design, but
the narrative credits the wrong mechanism, and the tuning knob advertised to users does
nothing.

`BlockCache` is also FIFO, not LRU (`order.push_back` only on insert, and `put` returns early
on a hit), which the doc comment does not say.

### 2.9 Corrupt or reused bytes panic instead of erroring — **medium**

`get_uvarint` reads `buf[*pos]` with no bounds check and shifts without a width check. `emit`
and `scan` do `get_uvarint` then slice `&extent[p..p+n]` unvalidated. `read_chunked_range`
validates its directory carefully — precisely because it knows those bytes may have been
reused — and then hands the decoded buffer to `emit`, which does not.

In an embedded library, a panic is the host application crashing. Every decoder reachable from
arbitrary file bytes must return `Err`, and this is the code the fuzzer should hit first.

### 2.10 `history_from` is always zero — **medium**

`checkpoint()` writes `history_from: 0` unconditionally. `Reader::open_as_of` and
`open_as_of_time` both gate on it, and the error message quotes it back to the user
("history is intact only from generation {}"). The guard can never fire, and the field is a
permanent lie in the on-disk format. Same pattern as the "dead code" guard in the confessions
section — a check that is never exercised looks like it works.

### 2.11 `scan()` skips the overwritten-range check — **medium**

`read_all` calls `check_extent` on every extent. `scan` does not. So on a store running under
a reclaiming policy, a snapshot read through `scan` silently returns whatever now occupies
those bytes, where the same read through `read_all` errors. Two read paths with different
safety contracts.

### 2.12 No single-writer enforcement — **medium**

No `flock`, no `O_EXCL`, no pid or writer-generation in the header. **[code]** Two processes
calling `Store::create` on the same path both truncate and both write. The entire safety
argument rests on an invariant the format does not enforce and cannot detect the violation of.

### 2.13 Refcount fragility — **low, but it hides bugs**

`Appender::release` does `if *n > 0 { *n -= 1 }` then frees on zero. The saturating guard means
a double-release on a block with refcount 2 walks it 2→1→0 and frees it while another key
still points there — silently, with no assertion. I could not construct a reachable path in the
current code, but the guard is there to suppress exactly the symptom that would reveal one.
It should be a `debug_assert!`.

### 2.14 Harness bugs

- **`soak.rs` under-reports live data by 211×.**
  `let live_mb = live as f64 * (keys as f64 / (keys as f64 / 211.0)) / 1048576.0 / 211.0;`
  The inner expression is exactly `211.0`, so it cancels the `/ 211.0` and the line reduces to
  `live / 1048576.0` — the raw bytes from the 1-in-211 sample, reported as the total. Any
  space-amplification conclusion drawn from the file-MB-vs-live-MB columns is wrong by that
  factor. Three further columns in that table (`free MB`, `reused`, `merges`) are printed as
  literal `"-"` placeholders.
- **`supbench.rs` reports a hardcoded zero.** `let merged = if no_compact { 0 } else { 0 };`
  is printed as `merged_keys`.

Both are in the class the document already confesses to: a reported number that is not a
measurement.

### 2.15 There is not a single test

No `#[test]` anywhere in the engine. **[code]** No unit tests, no property tests, no fuzzing,
no `miri` (there are five `unsafe` blocks), no TSAN (there is cross-process shared memory), no
`loom` (there is a hand-rolled lock-free claim protocol). Six manually-run binaries whose
output a human reads is the entire verification story. For a storage engine this is the
largest process gap, and it is why the defects above survived.

---

# Part 3 — Gaps

**Format and compatibility.** `MAGIC` encodes a version (`…0001`) that nothing reads as a
version — `decode` only tests equality, so any change is a hard break with no migration path.
No codec identifier reserved. No per-block checksum field. No feature flags. Endianness is
explicit and correct throughout, which is good; alignment for the cross-process
`&[AtomicU64]` view of the reader table is satisfied in practice but asserted nowhere, and
`AtomicU64` is not lock-free on every target — on those it is not shared-memory safe at all.

**Operations.** No reopen-for-write (§2.1). No incremental checkpoint, so the durability
interval is bounded below by `O(nkeys)` — meaning there is *no* point on the
durability/throughput curve usable by a transactional workload: either lose up to
`buffer_bytes` (512 MB by default) or pay a full index rewrite per commit. No backup or
snapshot-to-directory. No online repair or verification tool. No statistics beyond `Stats`,
no tracing, no way to observe merge or reclaim behavior in production.

**Data model.** No transactions, no cross-key atomicity finer than a checkpoint, no snapshot
isolation for iterators (acknowledged). No random access *within* a key's value list — for a
key-multivalue store, `values[i..j]` is an obvious operation the extent layout supports
cheaply, and only `first`/`last` exist. `read_first`/`read_last` return `i32` (a record
*length*), which is a benchmark-shaped API, not a user-shaped one. No range delete, no TTL,
no column families, no secondary indexes, no merge operators.

**Portability.** `prefetch` is x86_64-only with a no-op fallback; nothing has run on ARM.
The engine is Unix-only (`FileExt::write_all_at`). The design explicitly argues suitability
for network filesystems (the anti-hole-punch argument) and has never been run on one — where
both mmap coherence and the cross-process atomics in the reader table are exactly the things
that break.

**Concurrency.** `seal_shard` acquires the appender mutex *inside* its per-extent loop, and
`flush_builder` takes it twice consecutively. **[code]** Under multiple writer threads this
will convoy. `supbench` does have a multi-threaded append path (default 4 threads), but no
multi-threaded result is reported anywhere in the document, and RocksDB is never run
multi-threaded at all.

---

# Part 4 — The benchmark program that would constitute proof

The current suite is better than most, and the `readmissing_dense` design (absent keys drawn
from *inside* the populated range, because `db_bench`'s version tests the easy case) shows
genuinely good instinct. What follows is what would take it from "a well-measured hypothesis"
to a result a skeptical reviewer could not dismiss.

## Tier 0 — make any number trustworthy

The document establishes σ ≈ 55,000 ops/s on the write path and states its own rule: *"nothing
under ~15% means anything without repetition."* Then it reports `fillrandom @10M` at 1.14×
— **13.9%, below its own threshold** — with no error bars. Fix the methodology first, or
everything downstream inherits the doubt.

- **n ≥ 7 interleaved runs per cell.** Report median and IQR (or a bootstrap CI), never a
  single number. Interleave engines within a run, as the document already does elsewhere.
- **A significance gate in the harness**: a claimed win that does not clear the measured
  noise floor is emitted as "no difference," automatically.
- **Environment capture** in every result record: kernel, filesystem and mount options,
  device model and queue depth, page size, THP setting, CPU governor, SMT state, and whether
  the run was pinned. Pin to a `cpuset`, fix the governor, disable turbo drift.
- **Always report open and close as separate columns.** Never let an `O(N)` open hide outside
  the timer (see Tier 1 #2).
- **Measure write amplification at the device** (`/proc/diskstats` deltas), not from file
  length. The current 1.15×-vs-10-30× comparison is not measuring the same quantity.
- **Report space amplification as a time series** (live bytes / file bytes), not one
  end-of-run number.
- Every claim gets one command and one machine-readable result file. The document is already
  close to this; make it total.

## Tier 1 — the six experiments most likely to falsify the design

Run these first, precisely because they are the ones expected to hurt.

**1. Out-of-core.** Dataset at 4× and 8× RAM. `fillrandom`, `readrandom` (uniform *and*
Zipfian 0.99), `readseq`, `seekrandom`. This is where mmap without `madvise`, without async
I/O and without eviction control meets an LSM that controls all three. It is the single
largest unmeasured risk and the document knows it.

**2. The open-amortization curve.** Total wall-clock cost per read as a function of
reads-per-process, from 1 to 10⁷, at 10⁵ / 10⁶ / 10⁷ keys, **including process spawn and
index build**. Plot the crossover against RocksDB and LMDB. This is the most informative
single chart the project could produce: it directly tests uppend's founding premise (many
short-lived reader processes) against Supdb's `O(N)` open, and it determines whether the
readseq and readrandom wins survive contact with a real usage pattern.

**3. Multi-process readers.** The stated premise, still entirely untested. N processes × M
readers, live writer, all five reclaim policies. Must include: **> 64 concurrent readers**
(to exercise slot exhaustion, §2.4), **readers held open > 30 s** (§2.2), a reader killed
with `SIGKILL` while holding a slot, and a reader `SIGSTOP`ped past the stale window.

**4. Durability-matched throughput.** Sweep checkpoint interval against RocksDB's WAL sync
modes to produce a **throughput vs. data-loss-window curve** for both engines, plus
`fillsync`. Without this axis, "beats RocksDB on fillrandom" compares two engines that have
made different, undisclosed promises. This is also where the `O(N)` checkpoint's real cost
becomes visible.

**5. Latency distributions.** p50 / p90 / p99 / p99.9 / p99.99 / max for append, put and read
under steady load, as CDFs (HdrHistogram). Specifically instrument the `merge_key` stall and
the `checkpoint` stall. Zero percentiles currently exist, and inline merging under two locks
is the design's most likely tail-latency liability.

**6. Write-thread scaling.** 1 / 2 / 4 / 8 / 16 threads, both engines. The per-extent appender
lock (§Part 3) predicts poor scaling; RocksDB scales well. A single-threaded win that inverts
at 8 threads is a materially different claim.

## Tier 2 — representativeness

**7. YCSB A–F**, uniform and Zipfian(0.99), in-memory and out-of-core. No KV engine is taken
seriously without it, and E (scan-heavy) and F (read-modify-write) hit exactly the paths
Supdb has not exercised.

**8. `db_bench --benchmarks=mixgraph`.** This is the highest-value single addition and it is
already in the tool being used. It implements the workload model from Cao et al., *"Characterizing,
Modeling, and Benchmarking RocksDB Key-Value Workloads at Facebook"* (FAST'20) — whose central
finding is that **`db_bench`'s uniform-random key distribution is unrepresentative of every
production workload they measured**. Every Supdb benchmark uses uniform random keys. Skew
changes cache behavior, merge frequency and reclaim pressure, and it is the most likely place
for the current results to move.

**9. Real traces.** The Twitter production cache traces (Yang et al., OSDI'20) — 54 published
workloads with real key-size, value-size and skew distributions. Replay at least three.

**10. The rest of `db_bench`.** `seekrandom`, `readreverse`, `overwrite`, `updaterandom`,
`readwhilewriting`, `readrandomwriterandom`, `multireadrandom`, `fillsync`, `compact`. Until
these are run, "every benchmark in RocksDB's own `db_bench`" should read "four of `db_bench`'s
benchmarks." The claim as written is the one sentence most likely to cost the document its
credibility, in a document whose main asset is credibility.

**11. Compression corpora.** At least four: text-ish, JSON-ish, incompressible binary, and
high-cardinality identifiers. Report ratio *and* read amplification per corpus. **Include a
per-key zstd-with-trained-dictionary arm** — this is the direct test of the packing premise
(§1.2), and the one experiment that could show a large part of the block/chunk/solo machinery
to be unnecessary.

**12. Key-shape adversarial suite.** Fixed-width decimal, sequential u64 BE, UUIDv4, UUIDv7,
reverse-domain, long shared prefixes, and a deliberately colliding set. Report throughput
**and mean/max probe length**, across the six hash candidates from §1.2. This converts the
FxHash anecdote — currently the document's weakest technical claim — into its strongest.

**13. Shape sweeps.** Value sizes 8 B / 100 B / 1 KB / 100 KB / 10 MB plus a lognormal mix
(the 4 KiB free-list size-class floor and the 16 KB solo threshold both have cliffs in here,
and the floor is a plausible contributor to the one lost axis). Multivalue depth 1 / 10 / 10² /
10³ / 10⁵ / 10⁶ — the architecture's own axis, currently sampled at exactly two points.

## Tier 3 — the field

**14. Expand the comparator set.** Currently RocksDB, LMDB, MapDB, uppend. Missing:

- **redb** — the closest philosophical sibling in Rust: single writer, many readers, MVCC,
  copy-on-write B-tree, and deliberately *not* mmap-based. The most informative comparison
  available, because it isolates the mmap decision.
- **Pebble** — a modern, well-tuned LSM without RocksDB's legacy configuration surface.
- **fjall** and **sled** — the Rust embedded field.
- **SQLite (WAL mode)** — the actual default an embedded-database user reaches for, and the
  baseline every reader recognizes.
- **LevelDB** — the historical baseline `db_bench` was written against.
- **DuckDB** — for the scan axis.

**15. Two bounds, which matter more than any competitor.**

- An **in-memory `HashMap`/`BTreeMap`** upper bound, to show how much of the index cost is
  intrinsic.
- **RocksDB configured down to Supdb's actual promises**: `disable_wal`, `checksum=kNoChecksum`,
  a memtable as large as `buffer_bytes`, compaction effectively disabled or universal with
  high triggers, no transactions, no snapshots. A design with no WAL, no checksums, no
  reopen, no transactions and a 512 MB buffer *should* beat stock RocksDB on `fillrandom`.
  The interesting number is what remains after subtracting the features — that is the number
  that measures the engine rather than the promises.

**16. Retire the JNI caveat.** Use C++ `db_bench` for RocksDB (already done for one table) and
a Rust LMDB binding (`heed`) for LMDB. The cross-language asterisk is honestly disclosed and
well-controlled with MapDB, but it is now avoidable, so avoid it.

## Tier 4 — correctness as evidence

A fast wrong answer is not a result. These belong in the benchmark story.

**17. Differential testing against an oracle.** Randomized operation sequences against a
`BTreeMap<Vec<u8>, Vec<Vec<u8>>>` model, with shrinking (`proptest`). A hundred lines that
would have found §2.9 and probably §2.11 and §2.13.

**18. `cargo-fuzz`** on `read_chunked_range`, `read_chunks_into`, `get_uvarint`,
`Super::decode`, `decode_reuse_log` and the key-index decoder. The code's own comments say
these bytes may be arbitrary; that makes them the fuzz targets.

**19. Exhaustive crash injection.** ALICE (Pillai et al., OSDI'14) or CrashMonkey (OSDI'18)
methodology: enumerate crash points and reorderings across the checkpoint's write sequence.
This is cheap here precisely because the sequence is short — data, `sync_data`, superblock,
`sync_data` — so *exhaustive* is achievable, which is rare and would be a genuinely strong
claim. Add torn-sector and bit-flip injection (`dm-flakey`, `dm-error`) to demonstrate the
checksum path once §1.3 is fixed. One `SIGABRT` at one point is an anecdote.

**20. `miri` on the unsafe, TSAN on the reader table, `loom` on the acquire/release/oldest
protocol.** The claim protocol is hand-rolled lock-free code over shared memory; `loom` exists
for exactly this and would likely surface §2.3 mechanically.

**21. A history checker for the reader/writer contract.** The contract is stated precisely
enough to model-check: generations advance monotonically, every open yields a complete
checkpoint state, no read observes a partial one. Record histories, check offline.

**22. Long soak.** 24 hours, not the current 180-second default, tracking RSS, file size,
free-list fragmentation, merge rate and latency percentiles over time. Fix §2.14 first, or the
space-amplification column is off by 211×.

## The single artifact worth building

One harness that, for every cell of
`(engine × workload × dataset-size × thread-count × durability-setting × key-distribution)`,
emits:

> median throughput with IQR over ≥ 7 interleaved runs · full latency CDF · file size ·
> peak RSS · **bytes actually written to the device** · open and close cost · and a
> correctness-oracle pass/fail

with raw results committed alongside the claims. The engine is already good enough that this
would be worth doing; the document's framing is currently ahead of what it can support, and
this is what would close that distance.

---

# Part 5 — Suggested order

**Before any more benchmarking:**
1. `Store::open` — reopen for writing (§2.1). Without it the rest is a prototype, not an engine.
2. Reader heartbeat refresh, or replace the whole scheme with epoch-based reclamation (§2.2, §1.2).
3. The `AfterReads` `.min()` fix and reader-table exhaustion behavior (§2.3, §2.4).
4. Per-block checksums (§1.3). Everything about corruption handling depends on this existing.
5. Bounds-check every decoder; return `Err`, never panic (§2.9).
6. A property test against a `BTreeMap` oracle, and fuzz targets on the decoders (§4.17, §4.18).

**Then the measurements that could falsify the design:** Tier 1, in order — out-of-core, the
open-amortization curve, multi-process readers, durability-matched throughput, latency
distributions, write-thread scaling.

**Then the architectural questions the measurements will have made answerable:** the mmap-able
shared index (§2.7), incremental checkpoints, and the zstd-dictionary arm that tests whether
packing was necessary at all (§1.2).

**And narrow the headline claim now**, to the four `db_bench` benchmarks actually run,
single-threaded and memory-resident, with error bars. The document's greatest asset is that it
tells you where it is weak. That one sentence is the place it doesn't.
