# The paged-source contract, held by the gate

Tree `3df85d22`, read only. Nothing below was built or run, and every cost and run time is a prediction.

## The owner's requirement, stated so it can be tested

A change to a form, or a new one, must pass `sh scripts/check.sh` without:
- an edit to `vault.rs`, `pager.rs`, `pagefile.rs` or `crypt.rs`;
- a cipher test written for it;
- its author knowing what a cipher is.

If the change breaks under a cipher, the form's own tests have to fail. They must fail in the clear, in the checked profile, on the author's first run.

The approach keeps the code as it is (Pagefile, explicit asks). The rules the cipher imposes today are restated as rules of a paged source, which has nothing to do with the cipher, and the gate runs every existing test against a paged source in the clear.

The census found that the read-side disciplines (D1-D17) are what stays under Pagefile. This contract does not remove them. It moves where a broken discipline fails:
- today it is silent in the clear and in core;
- under this contract it is a failed test of the form, for any path a test reaches.

## Two vocabularies, and the wall between them

**Form side.** This is every module that writes or reads bytes:
- format, block, index, flatindex, blob, bytes' readers;
- ordindex, zpages, zfile, zselect;
- db.rs outside the door.

Form code may name the I/O API: `vault::{create(path, Class), open, map, rename, hard_link, remove_file, names, sync_dir}`, `vault::File`, `MmapBytes`, `Sect`, `Pieces`, `slice_at`, and the pin API. The pin API is today's `vault::hold`, `Reader::unlent`/`unlend`, the rests and `vault::check`, renamed out of the cipher's vocabulary into a `pin::` module: `pin::hold`, `pin::idle`, `pin::rest`, `pin::verify`.

Form code may not name:
- `cipher`, `Cipher`, `Encryption::Cipher`, `StoreCipher`;
- `vault::{kept, persist, persist_form, companion, paged}`, `Kind`;
- `pager::`, `ATTN_PAGED`, `crypt::`.

**Encryption side.** This is `vault.rs`, `pager.rs`, `pagefile.rs` and `crypt.rs`. It may not name:
- a form's name or suffix: `seg-`, `pcs-`, `par-`, `.oidx`, `zpg`, `wal-`, `manifest`, `direct-`;
- a form type: `Blob`, `SegmentWriter`, `ZFile`, `OrdIndex`, `Wal`, `Db::`, `ord_name_for`.

There is one exception: the marker's own name.

**Enforcement.** A source-reading test, `the_forms_and_the_cipher_do_not_name_each_other`, in the shape of `the_engine_names_no_file_but_through_the_door` (vault.rs:2876-2905).
- It reads every file under `src/` through `include_str!`, with comments stripped. Today's test reads db.rs alone.
- It works as a **ratchet**. `tests/data/couplings.txt` lists (file, token, count) for today's tree.
- The test fails when a count grows or a pair that is not listed appears.
- When a count shrinks, the test fails until the file is lowered. That keeps the ratchet tight.

Pagefile's stages lower the counts:
- S3 lowers `Kind`, `persist*`, `companion` and tokens;
- S4 lowers `RUN_LOGGED` and `log_run`;
- S5 lowers `cipher_controller` and `zpages_under_cipher`;
- S6 lowers `LogCipher` and `passes()`.

A new form that names the cipher is red on its first commit. Under this design the smallest refactor is the rename to `pin::`, which is mechanical. Nothing else is moved before it has to be.

The same test widens the door to all of `src/` (the census's D10). Outside the door modules (`vault`, `pagefile`, `bytes`, `pager`), it bars:
- `File::open`, `OpenOptions`, `fs::*`, `memmap2::Mmap::map`;
- `as_raw_fd`, `posix_fadvise`, `sync_file_range`, `libc::madvise`.

That catches `swap_ahead`'s `File::open` (zfile.rs:3972), which the door test misses today. It also catches the raw-descriptor advice sites that design §2 routes through the door.

## The conformance pass

**What it is.** The pass is a configuration in the clear that exists only under `cfg(debug_assertions)`, so the checked profile has it and release and bench builds do not.

`vault::resolve(&mut Options)` runs once per `Db::create` or `open`. When `SUPDB_CONFORM=1` is set and `encryption` is `None`, it substitutes `Encryption::Cipher` with three settings:
- an **identity** `Aead` (`StoreCipher::identity()`, test-only);
- a cache of `SUPDB_CONFORM_CACHE`, 4 pages by default;
- the pager in **eager** mode.

In eager mode, every rest and every end of an operation:
1. unpins;
2. wakes the evictor;
3. waits until the evictor has finished a pass that began after the unpin;
4. pins again.

That wait has a watchdog. The watchdog names the slot it waited for, using the thread id written at the pin and `/proc/self/task/<tid>/comm`. This is the instrument CLAUDE.md says was built twice and taken out both times.

`SUPDB_PAGER_GUARD=1` is set with the pass, so a page that has been given back faults when it is read instead of reading as zeroes (pager.rs:1557-1577).

**Where it runs.** A new group, `conform`, in `scripts/check.sh`'s **default** set, so it cannot become "a check that was not running". It runs:

```
SUPDB_CONFORM=1 SUPDB_PAGER_GUARD=1 cargo test --profile checked --no-fail-fast
```

It reuses the test group's binaries, so the compile is not paid twice.

**Run time: not verified.** The gate already runs the fuzz under a 16 KiB cache (tests/fuzz.rs:1260-1286), so a whole-suite pass in that regime is plausible. CI gives it a job of its own, beside the test group.

**Opt-outs.** A test that reads stored bytes, residency or names by path cannot run in this pass. It calls `common::plain_files_only("reason")`, which returns early under the pass. The source test requires each call to match a line of `tests/data/conform-skips.txt`, so a skip is listed and reviewed, not silent. How many tests need it is not verified (census, D1).

### Which rules each mechanism holds

Each rule a form author is held to is stated without the cipher.

**R1. Read stored bytes by asking for them.** Use `slice_at` or a `Sect`, never `as_slice()`, `Blob::flat()` or a whole borrow. A reader written only over the whole borrow reads a key as absent.
- In the pass, `as_slice()` on a mapping has length 0 (bytes.rs:375-378) and `flat()` answers `None`. The form's own tests then fail as wrong answers.
- The ratchet counts `as_slice(` and `.flat(` outside `bytes.rs` and `blob.rs`'s dispatch.
- The census lever D5: `flat()` made private to `blob.rs`.

This also catches the silent no-ops found in passing. `willneed_ranks` and `willneed_dir` return 0 through `flat()` (blob.rs:1239-1242, 1276-1279), and `warm_file` reads a length of 0 (zfile.rs:3783-3784). Each is fixed through `MmapBytes`' `Paging::willneed`/`resident`. A ratchet line bars `flat()` in any function named `willneed*`, `forget*`, `warm*` or `drop*`.

**R2. A borrow is valid until the next rest or the end of the operation.**
- The pass gives the pages back at every rest, and the guard turns a read through a stale borrow into a fault at the read.
- What it cannot hold: a path no test reaches; a borrow across a rest whose pages happened to be refilled (eager mode makes this rare, not impossible).

**R3. A walk longer than a bound rests.** In the pass, the pager counts the chunks filled under one pin interval. A debug assertion fires past `REST_BOUND` and names the pin's site.
- Proposed value: 4 MiB, twice the widest rest stride today. Calibration is not verified.
- This turns a missing rest from a slowdown nobody sees into a failed test.

**R4. A thread that reads and owns no reader slot holds the pin (`pin::hold`).** `Paged::ensure`'s assertion (pager.rs:1316-1320) fires on every read in the pass.
- Design change: today the assertion hangs the writer when it fires on the segment work's thread (`land_seal`). Under the pass, every thread the store spawns runs inside `catch_unwind`, and a caught panic aborts the process. The run is then red, not hung.

**R5. Never wait (fsync, join, park, condvar) while pinned, unless inside `pin::idle`.** In debug builds this holds in the clear as well as in the pass, by an assertion of `PINS == 0 || IDLE` placed in:
- the door's `sync_data`/`sync_all`;
- the engine's park and condvar wrappers;
- a `pin::join` that the ratchet makes the only `.join()` in `src/`.

In the pass, the wait-for-evictor ring also hangs, and the watchdog names it. Waits that block outside these wrappers (a lock taken elsewhere, a timeslice) are not held.

**R6. Output that depends on what was read is named only after a verify.** A failed read must surface as an error, never as a key that is absent.
- Design change (the census's D7): the verify moves into `Maint::publish` (and the run's close), so a merge or a new job cannot forget it. Today it is at db.rs:12865 and 13033.
- A new read entry point must go through the handle's begin and end. The compiler cannot hold that.
- The fuzz holds it with a **poison** draw in the pass: the identity cipher fails the tag of the k-th fill of a seed. The run must then answer either an error or the model's answer, and never a third value. This holds R6 for every entry point and job the fuzz drives. Entry points the fuzz does not drive are held by review.

**R7. A file is created with its I/O class, and the class's contract holds in the clear.**
- The compiler: `vault::create(path, Class)` takes the class as a required parameter.
- Debug assertions in the **plain** `File::Os` arm (the census's D2):
  - Log: at most 4 patches below the tail, and `finish` before a link or rename to a non-Log name;
  - Once: no write after `finish`;
  - Replace: whole rewrites only;
  - every name within the sealed budget (115 logical bytes).

So a form that patches a fifth page of a run, or lengthens `FENCE_MAX` until `par-` names no longer fit, fails in its plain tests on its first run. That is the day it needs a layer change, not after the cipher work.

**R8. A memory model reads memory through one function.** `zfile::read_budget` (zfile.rs:5210) is that function.
- The ratchet bars reading `memory.max`, `RssAnon`, `getrusage` or `ru_majflt` anywhere else.
- Design change: under a cipher, `read_budget` subtracts the pager's held plaintext (E7; grep found no subtraction).
- What this cannot hold: whether the model is right. Two caches in series have never been held to a measurement (E7: 0.433-0.447 faults a read). That needs a pairing.

**R9. Every option a form adds is drawn by the fuzz.** A source test reads `Options`' public fields and requires each one to be:
- named in `tests/fuzz.rs`'s option draw, or
- listed in `tests/data/fuzz-exempt.txt` with a reason.

The gate's cipher ranges then draw those options unconditionally, where today they depend on `ZFUZZ_OPTVARY` (tests/fuzz.rs:293-303), which the gate does not set. They also run with the block cache on. This is the census's D11, and it closes "new arms are never exercised under a cipher".

**R10. A form page read that fails takes the form out of service, not the store.** The census's D6: a failed form read is redone on the raw form at the operation level, instead of at each `page_bytes` site. Until then, the ratchet counts `page_bytes(` sites against a list, which is E0 item 6.

## Write side, WAL and manifest

**Write side.** Under Pagefile the write side owes the form nothing beyond R7:
- the class is an I/O fact the author already knows;
- `finish` is held by R7;
- an fsync is the engine's own, so there is no `persist` beside it;
- a new file kind needs no vault code.

The fault-injection crash mode of design §5 runs in the fuzz under the pass. A "crash" discards each file's writes since its last sync. The power-loss order is then held for every form the fuzz drives, in the clear.

**WAL and manifest (decision 2).** The contract requires (a): they move under the layer as Log and Replace files. Under (b), `LogCipher` and `seal_manifest` are form knowledge in the vault, so every WAL frame or manifest field change is an encryption change, which is exactly what the owner refuses. Until S6 they are lines in the ratchet.

## A store in the clear pays nothing

The pass costs a plain store nothing, because it exists only under `debug_assertions`. The env read is in `resolve`, once per open, and compiled out of release.

The rest is held by a `cost` group (a CI job, callgrind, not timing):
1. The tree is built twice: as shipped, and with `--cfg supdb_plain_only`. That cfg makes `Own::Paged` (bytes.rs), `File::Pg`, `ATTN_PAGED`'s paged arm and `MmapBytes::piecewise` uninhabited or constant.
2. Each build runs the counted rows of design §7, one row each: `read_only.rs supdb 100000 1 n-none`, `lag0_probe.rs supdb 100000 1`, `work_probe.rs 100000 whole`, `merge_walk.rs 100000 4 1`, `load_only.rs supdb 100000 1`, `read_only`/`lag0_probe` with `supdb-nocachezpages`, and a seal probe.
3. The group compares self instructions by function, using `callgrind_annotate`.

The group fails on any increase in any function.

The difference is taken within one tree, so a form change does not move it. Only the cipher's presence does. This is how "under what a pairing resolves is not nothing" becomes a gate rather than a sitting. Whether rustc removes every arm of an uninhabited variant is not verified. The comparison against `origin/main` stays a PR-time count by review, because form changes legitimately move it.

## What no mechanism holds

- Paths that no test or fuzz seed reaches.
- Whether a cost is right under a cipher:
  - rank walks asking record by record;
  - the two-cache split, which E7 puts at about 69% of the out-of-core gap;
  - whether the controller's model is accurate.

  These are pairings, not gate checks.
- Read entry points the fuzz does not drive (R6).
- Changes that need a new I/O verb or limit. R7 surfaces them in the clear on day one, but they are still layer changes, though form-agnostic:
  - a value log, where a mapped Log file pins all of its plaintext and the 49th session copies it;
  - io_uring or O_DIRECT reads;
  - more than 4 patches;
  - truncating a large Log file;
  - a checkpoint across directories;
  - a lock;
  - a read that keeps nothing;
  - fences that pass the name budget.
- The cipher's own correctness. The pass uses an identity cipher, so crypto stays with `tests/cipher.rs`, the pagefile unit tests, and the existing gate fuzz under a real cipher.

## Migration (gate green at each step)

1. The source test with the ratchet, and the door widened. No code change; the counts are recorded.
2. The `conform` group and `plain_files_only`. Fix whatever it finds: expected are the advice no-ops and `warm_file`.
3. The `pin::` renames. The ratchet goes down.
4. The thread wrapper (R4), the wait assertions (R5), and the verify moved to publish (R6).
5. R9 and the fuzz draws, including poison.
6. The `cost` group.
7. Pagefile S1-S6, each lowering the ratchet. R7's Os-arm assertions arrive with S1's `Class`.
