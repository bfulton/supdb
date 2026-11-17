# Forms without encryption changes: what the requirement means, and the contract that holds it

Tree `3df85d22`, read only. Nothing was built, compiled, run, counted or timed for this note. Every instruction figure is either CLAUDE.md's record or a prediction. The inputs are `design.md` (Pagefile), `experiments.md` (E0-E7), and five notes under `work/forms/`: `census`, `history`, `future`, `contract-types` (the Lend contract), and `contract-tests` (the paged-source contract), along with the two skeptics' readings of them.

The requirement, verbatim: *"Whatever the design we'll need to be able to make form changes easily without constant encryption changes."* Earlier the owner also said: encryption must work with everything; a plain store pays nothing; performance is the niche; and the seam goes "after the compression interface and before the disk interface so I wouldn't have to know too much about the DB forms."

**The conclusion in one line.** Pagefile meets the requirement for what a form *writes*. It does not meet it for how a form *reads, waits, spawns, or sizes itself*. A contract can close most of that gap. The parts are: whole borrows and raw pointers made private; the door and the memory reads barred by symbol; a gate job that runs the existing tests through the page layer in the clear; one `Memory` interface for the forms' models; and a ratchet that only goes down. The compiler cannot carry the read-path pins and rests on `Reader` without a cost the plain store would pay. Those stay runtime disciplines, held by tests on the paths that tests reach.

---

## 1. What the requirement means: acceptance tests

"Easily without encryption changes" is met when all four of these hold for a form change:

- **(Z) Zero lines** change under `src/db/vault.rs`, `src/db/pager.rs`, `src/db/pagefile.rs` or `src/crypt.rs`.
- **(N) No cipher test** is written for the change. Its own tests, run by the gate, are enough.
- **(K) No cipher knowledge** is needed by the author beyond two things: a file's I/O class, and "read stored bytes by asking".
- **(F) Fail loud in the clear.** If the change would be wrong under a cipher, the change's own tests fail on the author's first `sh scripts/check.sh`, in the clear, or the change does not compile. The test must not need a cipher, a small cache, or an out-of-core store set up by hand.

Speed under a cipher is a separate property (§6). The owner cares about it too, but it cannot be part of "easily".

### A. Named form changes that must land with (Z), (N), (K) and (F)

Each is a test case of the design. The design passes when each could be written by someone who has never opened the encryption modules. "Today" is the tree at `3df85d22`. "Pagefile" is design.md through S6. "Contract" is Pagefile plus §3.

| # | change | today | Pagefile | contract |
|---|---|---|---|---|
| A1 | a new segment layout (record, block, flat index, trailer field) | Z holds unless a reader borrows whole; F fails (wrong answers under a cipher only) | Z; F fails for a new whole-borrow reader | Z, F (whole borrows do not compile; conform pass) |
| A2 | a form page size or a coding change in the compact form | Z fails (4080-byte chunk against `ZPAGE`, pager.rs:108-125) | Z (pages are 4096); F needs `ZPAGE % layer::PAGE == 0` | Z, F (const assert) |
| A3 | a new file kind (a sidecar, an index beside a segment) | Z fails (`Kind`, companions, sweep rules, vault.rs:468-538, 1395-1510) | Z with a class at create; names must fit the sealed budget | Z, F (class required by type; name budget asserted in plain debug) |
| A4 | a new WAL frame kind or manifest field | Z fails (`LogCipher`, `passes()`, `seal_manifest`) | Z only after S6 (decision 2a) | same; decision 2a is required |
| A5 | a new background job that reads segments (a merge variant, a rebuild, a verifier) | Z holds, F fails: it owes `vault::hold`, rests, `unlent`, and `check`, and forgetting is a debug panic, a hang in `land_seal`, or silent loss | unchanged from today | F through `spawn_reading`, the wait assertions, the publish-time verify, and the conform pass |
| A6 | a new reader or walk over a section | F fails (`flat()` or `as_slice()` silently answers absent or 0) | unchanged | F (compiler, plus conform) |
| A7 | a second compact structure beside the records, chosen by the controller (the owner's out-of-core strategy) | Z fails (the controller is off under a cipher, `zpages_under_cipher` db.rs:3782, `cipher_controller` db.rs:11529) | Z fails until S5, and the two-cache model is unbuilt | Z only once `Memory`'s cipher implementation exists (§4, decision 3); otherwise this is the one switch-off that remains |
| A8 | longer fences (`FENCE_MAX` up) | free | **breaks**: the sealed name is 253 of 255 characters at the longest `par-` (design §12) | Z once long names spill into the header (owed before S3) |
| A9 | a new advice site (willneed, drop-behind, residency) | silent no-op under a cipher (blob.rs:1239 and 1276 through `flat()`, zfile.rs:3783-3784) | unchanged | F (advice through the byte source's methods; advice-count parity in conform) |
| A10 | a new `Options` arm that changes a read or write path | not run under a cipher at all (the gate's cipher fuzz forces only its defaults, tests/fuzz.rs:1252-1286) | unchanged | F (every option drawn by the cipher fuzz, or listed as exempt with a reason) |
| A11 | a new mapped write-ahead structure (a value log) | needs vault work | needs a layer verb (a mapped Log pins its plaintext; 48 sessions) | still a layer change, form-agnostic: the honest exception (§6) |

### B. Gate tests that must exist, and fail on a known-red input before they are trusted

- **G-wall**, `the_forms_and_the_cipher_do_not_name_each_other`. A source test over all of `src/`, a ratchet against `tests/data/couplings.txt`.
  - Form modules name none of: `cipher`, `Cipher`, `vault::{kept,persist,persist_form,companion,paged}`, `Kind`, `pager::`, `ATTN_PAGED`, `crypt::`, `zpages_under_cipher`, or `under_cipher`-shaped predicates.
  - The encryption modules name no form: no `seg-`, `pcs-`, `par-`, `.oidx`, `zpg`, `wal-`, `manifest` or `direct-`, and no `Blob`, `SegmentWriter`, `ZFile`, `OrdIndex`, `Wal` or `Db::`. The marker's own name is the exception.
  - A count may not grow. A count that shrinks must be lowered in the file in the same commit.
  - Red input: add `vault::kept(` to zfile.rs.
- **G-door**, by symbol, not by text. `clippy.toml` `disallowed_methods`/`disallowed_types` cover:
  - `std::fs::*`, `File::open`/`create`/`options`, `OpenOptions`, `memmap2::*`;
  - `libc::{open, mmap, madvise, posix_fadvise, sync_file_range, mincore, getrusage}`;
  - `std::thread::{spawn, Builder::spawn, park, sleep}`, `JoinHandle::join`, `Condvar::wait*`, `mpsc::Receiver::recv*`.

  `#[allow]` only in `vault`, `pagefile`, `bytes`, `pager`, `layer::Memory`, and the wrappers of §3. CI runs clippy with `-D warnings` already ("everything holds to `-D warnings`").
  - This replaces today's text test (vault.rs:2876-2905). That test reads db.rs only and misses zfile.rs:3972's `File::open`, `File::options()`, `fs::read_to_string` and aliases.
  - Red input: `std::fs::File::open` in zfile.rs.
- **G-conform**, a CI job (§3, R2). The whole test group under `SUPDB_CONFORM=1`. Red input: a reader that calls `as_slice()` internally (before privatisation lands), or a borrow kept past `rest()`.
- **G-cost**, a CI job. Callgrind by function against the merge base (§5). Red input: the 6-instruction shape CLAUDE.md records at 55c3e526's predecessor.
- **G-options**, a source test. Every `pub` field of `Options` is drawn by `tests/fuzz.rs`'s option draw, or is listed in `tests/data/fuzz-exempt.txt` with a reason.
- **G-names**, a property test. Every name constructor driven at its longest inputs either seals within NAME_MAX or spills into the header.

---

## 2. Where Pagefile meets it, and where it does not: the count

The history replay (`work/forms/history/notes.md` §3) lists **40 changes that features forced on this branch** for the cipher, across nine commits:

- `15d0335e`, `eb377fe4`, `38567647`, `55c3e526`, `17c363a5`, `d0816fce`, `0ea02c5f`, `b4d555bc`, `81ca5d3e`;
- about 13,900 lines under `src/` and 2,900 in tests;
- four features switched off at some point: the recycled log, ordered ingest, the compact form, and its controller (still off);
- one check silently skipped: the ordered index's CRC (ordindex.rs:413-428).

The 40 changes fall into these groups:

| fate under Pagefile | count |
|---|---|
| gone outright (write-side form knowledge: memfd table, tokens, `Kind`, companions, persist sites, the FADV drop, two views, `under_cipher`) | 8 |
| gone only if S4 or S6 land (`log_run`, `LogCipher`, the sealed slot, recycle) | 4 |
| already gone on the branch | 1 |
| change (ordered-index CRC by the piece, advice by Handle, form damage is the file's, controller at S5) | 4, plus T39 gaining fault injection |
| stay | 21 |
| not addressed (rank walks ask record by record, R20; two caches in series, C37) | 2 |

**Pagefile alone.** 13 of 40 go, all on the write side, and that side caused three of the four switch-offs. The read side stays whole (design §6 "Stays", about 2,800 lines of ask path). That is where most of the lines and most of the bugs were: the six out-of-cache pins, `d0816fce`, the zero footprint, the attach that held nothing, four snapshot mutants, and every plain-cost regression. Pagefile also *adds* new couplings: a class per file, `finish`, a cap of 4 overrides, the name budget (A8, a regression), `manifest_slots: false` refused under `Pg` (a new switch-off), and the `File` enum widening. So by the acceptance tests, Pagefile passes A2, A3 (with A8 broken), and A4 after S6. It fails A5, A6, A7, A9 and A10.

**With the contract**, the 23 that stay or are unaddressed split into three groups. The mapping is mine, not verified row by row against every site.
- **8: infrastructure built once.** A new form inherits these and writes nothing: R16, R17, R18, R19, R22, R28, R29, W9.
- **13: obligations a new feature re-owes, now held by a mechanism.**
  - rests (R21, R25, R26, R30) by the conform pass and the rest-bound assertion;
  - holds (R23, R33) by `spawn_reading` and the `ensure` assertion;
  - `unlent` waits (R27) by the wait assertions;
  - `check` (R24) by verify-at-publish and the poison draw;
  - the held build (R32) by one build generic over the lease;
  - advice (R34) by advice parity;
  - form damage (R35) by `Result<_, Damaged>`;
  - plain cost (P38) by G-cost;
  - tests (T39, T40) by the option draws and conform.
- **2: still held by measurement only.** R20 (speed) and C37 (the model). C36 (the controller) moves from "off" to "on once `Memory`'s cipher implementation is held to E7".

So the requirement's (F) is met for 13 of the 15 obligations, on paths a test reaches. Two remain measurement problems, and §6 says so.

---

## 3. The contract to adopt

The two proposals were combined rule by rule. The skeptics' main findings are accepted:

1. **On `Reader`'s read path the store's kind is a runtime value.** `enter_by` builds the same `Entered` for both stores (db.rs:17999-18046), and the plain scan skips the rested walk at db.rs:20765-20771. A lease type there would make the plain path either run the session loop or compile every reader twice. So the Lend contract's compile-time pins and rests apply **only to jobs that own their lease by value and choose it once**, as `compact_job` does today (db.rs:11969-11973): merges, the zfile build, the open, the attach and the sampler. They do not apply to `Reader`.
2. **`Free` must not be mintable in `db/`**, or a bare thread compiles.
3. **Eager eviction at every operation end** costs a thread hand-off (15-200 µs here) per operation. It would fire the minute watchdogs and serialise the threaded tests that find races.
4. **A `--cfg plain_only` cost comparison** is blind to what the contract itself adds, and it rebuilds RocksDB.
5. **Advice no-ops change only speed**, so a pass that checks answers cannot see them.
6. **The conform pass never reaches out-of-core paths** unless the memory model is told the store is out of core.

| # | rule | enforced by |
|---|---|---|
| R0 | **The wall.** Form code names only the I/O API (`vault::{create(path, Class), open, map, rename, hard_link, remove_file, names, sync_dir}` and the typed handles), `MmapBytes`, `Sect`/`Pieces`/`slice_at`, and the `pin::` API (`hold`, `idle`, `rest`, `join`, `verify`: today's `vault::hold`, `unlent`/`unlend`, rests, `vault::check` renamed out of the cipher's vocabulary). The encryption modules name no form. | **gate test** G-wall (ratchet). Token lists miss new spellings: review, until the list grows. |
| R1 | **The door, by symbol.** No file, mapping, advice, sync, thread spawn, park, join, condvar wait or sleep outside the door, bytes, pager, `layer::Memory` and the `pin::` wrappers. | **compiler-adjacent**: clippy `disallowed_methods`/`types` under `-D warnings` (G-door). Resolves paths, so aliases do not escape. |
| R2 | **Read by asking.** Stored bytes are read through `slice_at` or a `Sect`. `MmapBytes::as_slice`, `lend_at`, `Blob::flat`, `Blob::pieces` **and `Paging::base`** (pub today, bytes.rs:300-302) become `pub(in crate::bytes)` or `pub(in crate::blob)`, used only by the dispatch (`indexed!`, `scan_either`). | **compiler** (visibility). Runtime cost nil (predicted). Today's silent no-ops (blob.rs:1239, 1276; zfile.rs:3783; the ordered index's CRC skip, ordindex.rs:413-428; the zero footprint) stop compiling and are rewritten. |
| R3 | **Advice is a method of the byte source, written once.** `willneed`/`forget`/`resident` are methods every source answers: the slice by `madvise`, the paged source by the Handle's ciphertext mapping, which exists already as `bytes::Paging` (census: bytes.rs:481-632). No advice site takes an address. | **compiler** (R2 removes the address). **gate test**: conform counts advice issued per call site and asserts the same count as the run in the clear, so a site that compiles but does nothing for `Pieces` fails. |
| R4 | **Borrows end at a rest; jobs own their lease.** A job that reads under a hold takes `L: Lease` by value, with `Free` (plain) carrying `const SESSION: usize = usize::MAX` and `rest() = false`, and between sessions keeps positions, not borrows (the skeptic's fix for `merge_ranks`' `Vec<&[u8]>`, db.rs:12305-12318). `Reader`'s read path keeps today's runtime split at the test it already makes (`zcensus_begin` and the attn bits) and is not given a lease type. | **compiler** for jobs (higher-ranked `with`, `&mut rest`). For `Reader`: the **conform pass** (eager eviction at every rest, plus the guard; below). |
| R5 | **Threads.** Every store thread that reads is spawned by `pin::spawn_reading(name, store, \|held\| ..)`. `Free` is constructible only inside the crate's dispatch. `Cx` is `!Send`. Every store thread runs under `catch_unwind`, aborting under debug assertions, so an assertion on the segment work's thread is red and not a hang in `land_seal`. | **compiler** (R1 bars `thread::spawn` in `db/`; `Free`'s constructor is private). **debug assertion** `Paged::ensure` (pager.rs:1316) as backstop. |
| R6 | **No wait under a pin unless idle.** | **debug assertion** `PINS == 0 \|\| IDLE` in every handle's `sync_data`/`sync_all`/`write`, and in the `pin::` wrappers for park, condvar wait and join. This runs in plain checked builds, so every test holds it. A timeslice lost while pinned: **nothing**. |
| R7 | **Long walks rest.** | **debug assertion** under conform: chunks filled per pin interval above `REST_BOUND` panics, naming the pin site. The bound is uncalibrated; set it from today's rest strides (`SCAN_REST` 1024 keys, `MERGE_REST` 256 steps) before turning it on. |
| R8 | **Verify before an output goes live.** Each job verifies at its end, before its outputs are named (today's sites db.rs:12865, 13033), **and** `Maint::publish` asserts the store's failure latch is clear for anything it publishes. Form-page reads of Regen and Regen-lazy files return `Result<&'g [u8], Damaged>`, `#[must_use]`, never `Option`. | **compiler** (`Result`, `must_use`). **debug assertion** at publish (backstop for a new job that publishes another way). **gate test**: the fuzz's poison draw (the identity cipher fails the k-th fill; the answer must be an error or the model's value). |
| R9 | **Every file has an I/O class, chosen by its I/O pattern.** The four questions concern I/O, never form. Typed handles: `OnceFile`, `LogFile` (`finish(self) -> OnceFile`, `set_len` only here), `ReplaceFile` (`replace(bytes)` only), `RegenFile`. | **compiler** (constructor and handle types). **debug assertion** in the plain `Os` arm of each class's contract: the override cap, no write after `finish`, whole Replace. Plain debug also mirrors `Pg`'s trust rule: a Once file with no `finish`, opened by a process that did not write it, reads as untrusted. **Review** for the choice of class in a new recovery shape. |
| R10 | **Names fit, or spill.** A logical name longer than the inline budget spills into the file's sealed header, with a flag in the directory entry, so `FENCE_MAX` no longer couples to `NAME_MAX`. | **compiler** (const assert for constant-built names). **gate test** G-names (property test over name constructors). The spill is **not designed**; owed before S3. |
| R11 | **Page geometry.** `layer::PAGE = 4096` is exported. Every form page size is a multiple of it. | **compiler** (const assert in zpages.rs and format.rs). |
| R12 | **Memory comes from one interface.** `trait Memory { budget, misses, miss_price, resident }`. The plain implementation is today's code moved: cgroup less anon (zfile.rs:5199-5270), `ru_majflt` (zfile.rs:4942-4950), `mincore` (db.rs:14936-14958), `/proc/thread-self` (db.rs:13062, 13074). The controller, the sampler, `choose_coding` and every future sizing model read only this. **No `Options` field is resolved differently by encryption mode outside the layer**: no more `zpages_under_cipher`. | **compiler-adjacent** (R1 bars the syscalls and `/proc` reads outside `layer::Memory`). **gate test**: zselect's replays run against a one-cache and a two-cache `Memory`; G-wall bars a cipher predicate in `Options` resolution. Model **accuracy**: measurement only. |
| R13 | **Every option is exercised under the layer.** | **gate test** G-options. The gate's cipher fuzz ranges draw the option variations with the block cache on (D11), and fault injection runs under conform. |
| R14 | **A store in the clear pays nothing** per key, entry, block or step. | **gate job** G-cost: by-function self instructions against the merge base, over §5's rows. A **source test** keeps a checked-in list of read entry points, each mapped to a probe, so a new entry point without a probe fails. |
| R15 | **Speed under the layer is visible as a number.** | **gate test** in conform: asks per point read, per scan entry and per merge step, held as a ratchet. A new walk that asks record by record shows a number before any pairing. The pairing itself: **review**. |

### The conform pass (R2, R4, R7 and R13's vehicle)

- **What it is.** Compiled under `cfg(debug_assertions)` only, so it is in the checked profile and out of release, the bench binaries and the shipped crate. `vault::resolve`, called once per open, substitutes `Encryption::None` with a page-layer store when `SUPDB_CONFORM=1`. That store has:
  - an identity `Aead`;
  - clear names;
  - `lend = 0`.
- **No rebuild.** It is an environment variable over the same `checked` binaries, so it costs a second test run, not a second build.
- **Two tiers.**
  - **Tier 1 (every test).** A cache of normal size; eviction at every `rest()`, and at a seeded sample of operation ends (one in 64; the seed printed); `SUPDB_PAGER_GUARD=1`, tightened so that a chunk is `PROT_NONE` until filled. With Pagefile's 4096-byte chunk this is exact; today's guard sees only pages given back (pager.rs:1557-1566).
  - **Tier 2 (a named subset, plus the fuzz).** A cache of 4 pages, eviction at every operation end, and an injected out-of-core budget through the existing `ZInject` seam. With that budget, `Memory` says "out of core", and the controller, builds ahead, `pieces_willneed`, `blocks_willneed`, `zpages_drop_raw` and the crossing actually run. The test asserts by counters that the controller bought and that the forms served.
- **Opt-outs.**
  - A test that reads stored bytes or names by path calls `common::plain_files_only`, and every such call must have a line with a reason in `tests/data/conform-skips.txt`.
  - Crash tests stay inside conform through a layer test API that builds crash states: discard since sync, and tear. Today `tests/db.rs` writes those states by path.
  - The counts the skeptic took suggest how many files are exposed: `tests/zpages.rs` has 136 tests and about 250 filesystem references, `tests/db.rs` 226 and about 149, `tests/segwriter.rs` 19 and about 41. The actual count of opt-outs is **not verified**.
- **What the pass holds.** Behaviour on the paths a test reaches, with no cipher in the author's head. That is (F).

---

## 4. Changes to the design, and the owner's open decisions

### Sections

- **§1 (seam).** Add the second half of the seam, the *read contract*. The layer's vocabulary is pages and classes; the forms' vocabulary toward it is `Sect`/`slice_at`, the advice methods, `pin::` and `Memory`. State that the explicit-ask pager stays, and that this choice is why read-side disciplines exist. The transparent alternative (§11) was rejected on cost and portability, and the owner should hear that rejection in those terms: it is the price of keeping R4-R7.
- **§2 (door).** Replace `File { Os, Pg }` plus a class argument with the typed handles (R9). Advice becomes a handle method, so the raw-descriptor sites at db.rs:6071, 14904 and zfile.rs:3920-4046 go through it. Keep the discriminant out of per-key structs (a class type parameter on the writer, as `MergeRest` is today) to protect `write_buf`'s inlining.
- **§3 (names).** Add the long-name spill (R10). Remove "refused by name under `Pg`" for `manifest_slots: false`, which violates R0'. Make rename-over a verb with a class rule.
- **§4 (trust).** Form-page reads become `Result<_, Damaged>`. Keep the store-wide latch for the operation's end (`Entered::finish`, db.rs:16351-16362) as the backstop for any read that drops a failure.
- **§6 (goes and stays).** Add: `Paging::base` and the whole borrows go private; the advice no-ops are rewritten; the `pin::` rename; `spawn_reading` (23 spawn sites, per the Lend contract, not re-counted); `layer::Memory`.
- **§7 (plain cost).** Replace "counted at S1" with G-cost as a standing job (§5).
- **§11 (rejected).** Add the Lend contract's lease on `Reader` and `--cfg plain_only`, with the reasons above.
- **§12.** Below.

### Stages, gate green at each end

- **S0** (from E7's notes): the plain durability fixes. Unchanged.
- **G1, before S1.** G-wall and G-door at today's counts. Privatise `flat`, `as_slice`, `lend_at`, `pieces` and `Paging::base`, and rewrite the four silent no-ops (R2, R3). These are fixes to today's shape and land without Pagefile. Count the rows (§5).
- **G2, before S1.** The conform pass, tier 1, on **today's** envelope pager, which already has `lend = 0` and the guard. Fix what it finds. Then add tier 2 with the injected budget, and the poison draw. Doing this before the layer means Pagefile's own stages are built under it.
- **S1** with the typed handles (R9) and `spawn_reading`, catch-unwind and the wait assertions (R5, R6). Verify at publish (R8).
- **S2** with the class contracts asserted in the plain `Os` arm and the trust mirror. The long-name spill is designed and model-checked (E4's model extended).
- **S3-S4** as in experiments.md.
- **G3, before S5.** `layer::Memory` (R12): the plain implementation moved, and the cipher implementation built and held to E7 (0.433-0.447 faults a read, about 337 MiB plaintext beside about 240 MiB ciphertext). S5 then deletes `zpages_under_cipher` and `cipher_controller`, and G-wall's count for them goes to zero.
- **S6** with decision 2(a), which is then required (below).
- **S7** WARM, unchanged.

### Decision 2: WAL and manifest under the layer, or sealed by the engine

The requirement settles it: **(a)**. Under (b), `LogCipher`, `passes()` and `seal_manifest` stay in the encryption modules for good, and every frame kind or manifest field that touches what they seal is an encryption change, which fails A4. Option (b) is no longer a valid choice. It is only a waypoint, and a date for leaving it should be set.

Until S6 lands:
- G-wall lists those three as couplings, ratcheted down at S6.
- Whether a *frame kind* alone (as against the record's framing) touches `LogCipher` today is **not verified**. If `LogCipher` seals whole records as opaque bytes, frame changes already pass (Z), and only manifest fields and log-file shapes do not.
- S6 stays gated on E4's extended model and the fault-injected fuzz, run under conform.

### Decision 3: the plaintext budget, and memory held once

The requirement changes the question. Under (a) or (b) as worded, the forms' models (zfile.rs:5199-5270) see one cache, the kernel's, and the cipher holds two. Two consequences follow:
- every controller-chosen form is either switched off under a cipher, which fails A7 and is the pattern the owner rejected;
- or it is mis-sized: E7 attributes about 69% of the out-of-core gap to the bound split three ways.

The contract's answer:
- **The budget is the layer's, and it is read by forms only through `Memory`.**
- Whether the default is the caller's `cache_bytes` (a) or cgroup-derived (b) becomes a choice inside `Memory`'s cipher implementation, invisible to forms.
- "Memory held once" is the layer's lever, also invisible to forms. The candidates are E7's: drop a page's ciphertext behind its fill; page-granular eviction; a budget point between 64 and about 268 MiB.

Recommendation:
- Decide decision 3 as "**one budget, owned by the layer, exposed through `Memory`**".
- Build that interface at G3 with the plain implementation moved verbatim.
- Choose (a) or (b) and the E7 lever by pairing, as layer-only edits.
- The thing that must not happen again: an `Options` default that differs by encryption mode in form code (R12).

None of the levers is priced. E7's follow-ups (`mincore` overlap, a mid budget point) come before G3's cipher implementation.

---

## 5. What it costs

**Refactor size.** Estimates from the notes, not counted line by line:

| part | lines (est.) |
|---|---|
| G1: privatisation, four no-op rewrites, `Paging::base` callers | 300-600 |
| G-wall, G-door (clippy config, allowlists, ratchet file and test) | 150-250, plus `#[allow]` at about 30 sites |
| G2: conform resolve, eager-eviction hooks, tightened guard, rest-bound counter, advice parity, asks ratchet, crash-state test API, opt-out list | 600-900 code, plus 200-400 of test edits |
| R5/R6/R8: `spawn_reading` (23 sites), catch-unwind, wait wrappers, verify at publish, `Result<_, Damaged>` at the form-page sites (list owed, E0 item 6) | 400-700 |
| R4: leases for jobs (`MergeRest` generalised; `Walked::{Mapped,Held}` folded into one build generic over the lease) | 300-500, net of deletions |
| R9: typed handles, on top of S1's 350 | about 200 |
| R10: long-name spill, plus the E4 model extension | 200-300, plus the model |
| R12: `layer::Memory` with the plain implementation moved | about 300; the cipher implementation is unpriced |
| G-options, G-names, G-cost scripts | about 300 |

The total is roughly **3,000-4,500 lines** on top of Pagefile's own (design §6: about 4,150 new). The biggest single risk is the opt-out edits to `tests/zpages.rs` and `tests/db.rs`.

**Gate time. None of it is measured.**
- G-wall and G-door: seconds (a source read, and clippy, which already runs).
- G-conform tier 1: a second run of the test group on the same binaries. Today's tests take about four minutes; the slowdown from `lend = 0` and sampled eviction is unknown.
- Tier 2: the cipher-cache tests had to run one at a time already (`one_at_a_time`), so tier 2's subset must too.
- **Put both tiers in their own CI job, not in `check.sh`'s default set, until their time is measured.** This is the same treatment the `quick` group gets.
- Watch for code 101 with no summary: CLAUDE.md says to read it as a watchdog before anything else.
- G-cost: two builds (merge base and head) of the bench examples, plus eight probes under callgrind: tens of minutes, CI only. Use a cargo feature, never `RUSTFLAGS`, so RocksDB's build is not invalidated.

**Callgrind rows owed.** By function, `callgrind_annotate` side by side, each probe built at `origin/main` (later, the merge base) from this tree's example sources and at the tree:
1. `read_only.rs supdb 100000 1 n-none`: the point read.
2. `lag0_probe.rs supdb 100000 1`: the scan, including `emit_unsealed` and `walk_fixed`. Run at a limit under and over `SCAN_REST` (1024), because the plain scan must not enter a session loop.
3. Rows 1 and 2 with `supdb-nocachezpages`: the form's point read and scan (`ZFile::view_of`, `read_exts_inline`).
4. `merge_walk.rs 100000 4 1`: `merge_ranks` and its closure under `Free` with positions held between sessions.
5. `work_probe.rs 100000 whole`: `write_buf`'s inlining with the typed handle.
6. `load_only.rs supdb 100000 1`: `commit_direct`, a seal's open and landing, the ordered index's open check.
7. A seal probe: `SegmentWriter::create_with`.
8. `Blob::scan_at` through the public API, apart from the engine's scan. That is where the kind dispatch must come before the session, so that each walk is its callback's only taker.

Each is run at G1, G2, S1, R4 and G3. The threshold is zero added instructions in any per-key, per-entry, per-block or per-step function. A function whose count moved with no change to its source is read for what it keeps live before the move is called the compiler's.

---

## 6. What no mechanism enforces, stated plainly

- **A timeslice lost while pinned.** The evictor waits for the oldest pin. Nothing annotates a deschedule. The evictor's longest-wait counter stays in the cipher tests as an alarm, not a rule.
- **Paths no test and no fuzz seed reaches.** The conform pass holds what a test runs. A new job behind an option that nothing draws is held only by G-options' list, and an exemption is a line someone approves.
- **Speed under a cipher.** The conform pass checks correctness at a tiny cache. The asks ratchet (R15) shows a walk that asks record by record as a number. Whether a form is *fast* under a cipher remains a pairing per feature: the compact form lost at 0.60-0.75x and was found last. New arms are not paired under the cipher automatically.
- **Hot walks are written twice**, a slice walk and a windowed walk, because folding them cost the plain scan 3-5 instructions an entry. "Written once over `Sect`" holds for correctness, and the windowed walk is a performance obligation that only R15 and a pairing see.
- **The model's accuracy.** `Memory`'s two-cache implementation is held to one measurement (E7). Its predictions elsewhere are not held. The prediction-beside-measurement trace that CLAUDE.md prescribes is the only check.
- **The choice of class for a new recovery shape.** The plain trust mirror catches the commonest mistake (a Once file read before `finish`). A recovery that reads a partly written file is classed by review.
- **A nested read in a caller's callback must not rest.** This is the runtime depth check at db.rs:20770 and nothing static.
- **Verbs the layer does not have.** A value log (A11), O_DIRECT or io_uring reads, more than 4 overrides, truncating a large Log file, a checkpoint across directories, a lock, a read that keeps nothing. Each still needs layer code, though never form knowledge. The requirement's "easily" holds for forms and not for new I/O patterns.
- **The ratchets' honesty.** `couplings.txt`, `conform-skips.txt` and `fuzz-exempt.txt` can be edited in the PR that needs them. Each line carries a reason, and growth is flagged for the owner's approval (CODEOWNERS on the three files). It is still review.
- **Token completeness.** G-wall's cipher vocabulary is a list. A cipher branch spelled in new words passes until the list grows. G-door is by symbol and does not have this hole.
- **The plain cost against main** is a CI job and depends on the probe list being complete. The entry-point list makes a missing probe red, but not a missing *row* inside an existing probe (for example, a scan at a limit nobody runs).
- **Not verified, from this note:**
  - that the lease by value for jobs leaves the plain merge loop's count unchanged;
  - that the clippy lints resolve every alias in this crate's usage;
  - the conform pass's run time and opt-out count;
  - whether `LogCipher` is touched by a frame-kind change;
  - which `from_raw_parts` sites in db.rs (8988, 9242, 9390, 9470, 16848, 16860) address file mappings;
  - the long-name spill's crash ordering.
