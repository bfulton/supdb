# The owed items, concluded: what they change, stage by stage

2026-11-17. Four items, each re-derived by a skeptic. Paths are under
`seam/work/owed/`. Where a check corrects its work, the check stands. All four
checks read "stands with corrections". One claim does not stand: spill's
"0 violations at n=5". Nothing below builds on it. Nothing here was compiled or
run on the engine. Every engine statement comes from reading the export at
682b2c09 or from a model. The later unit of open fixes is in neither.

This note amends `decisions.md` only where the amendment is listed under
"Amendments to record". It does not restate the items.

## Amendments to record in decisions.md

1. **Decision 2, the slots: no chain.** A slot is one sealed record at offset
   0, rewritten whole.
   - The per-slot chain cannot be checked.
   - A cross-slot chain is sound only if it names the other slot's
     known-durable record, which puts `SlotChoice` inside the door.
   - Chaining to the previous publish refuses legal states: 12,506 REFUSED
     (`slots/design.md` §3; `slots/check.md`).
   - The cross chain does not catch H2: 3,256 LOSS_BRANCH, the same count as
     without it (`slots/check.md`, rerun).
   - The log and the spare keep the chain as decided.
2. **Decision 2, the slot's key.** `slots/design.md` §2 seals under K_s with a
   per-open nonce prefix. That contradicts the seam's rule that K_s seals
   nothing under a random nonce (`design.md`:6, 157), and the check found
   nothing in `fix/conclusion.md` that amends it (`slots/check.md`).
   - Take the Stream class's key instead: one derived from the file's K_f,
     with a prefix per Handle. This follows the rule already written. It is
     not a new choice.
   - With a per-file key, a record moved between slot files fails by its key.
     The slot index in the AAD is then redundant. It costs nothing, so keep
     it.
   - Do not cite it as a protection. The check found the set of records an
     open can be made to take is identical with and without the index, in
     9,331 of 9,331 histories (`slots/check/reloc_outcomes.py`).
   - The property to keep is that the engine takes a slot's index from the
     file's position (db.rs:12723).
3. **Decision 2, consequence list: Replace leaves S2.** Its users are the
   two slots and `manifest.prev-*`, and the second is the rename shape, which
   is refused under a cipher. The work took this from `design.md`:85-94; the
   check confirmed it in the tree (db.rs:12580-12610, 40966).
4. **Decision 4, the leakage list.** Add: "that a file's name exceeds 128
   bytes", visible as allocated name pages (`spill/notes.md`). This applies
   only if the spill is built (owner question 4).

## Stage by stage

### Stage 0 and "the fixes held back for pricing" (Order 1-2)

These are fixes to the plain store, found here. Each is listed with its bug
under "The engine as it stands" below. Each starts from a failing test and
is priced under decision 10 (slot syncs at open only).

- **H2.** The open fdatasyncs both slot files before it sweeps or writes a
  slot.
  - Test: a slot torn by `Db::tear_next_slot_write` after an owed publish,
    then a kill, a reopen, and a power cut emulated by restoring the device
    image the test kept. The open must not take a state older than the torn
    slot's surviving record.
  - Mutant: the engine's sync of the taken slot only.
  - Source: `slots/design.md` §7, fault half.
- **H1, for the slots.** After its syncs, the open reads both slots past the
  page cache.
  - Test: `Db::fail_next_slot_sync`, then a reopen in the same process. The
    open must not count that slot durable.
  - Owed first: what the open does when its sync of the slot it does not
    take fails. Neither the work nor the check modelled it
    (`slots/check.md`, missed).
  - The scope beyond the slots is owner question 1.
- **Pair 1, the writer and its helper.**
  - Test: an injected helper sync failure whose error is stored after the
    writer's own fdatasync has returned 0. The run commit or `sync` must not
    answer `Ok`.
  - The fix sketch is begun/ended counters, so that `synced_all` waits for an
    overlapping helper sync to publish its outcome. That adds a wait on a
    success path, so it is priced first (`syncop/notes.md`; the shape is
    corrected in `syncop/check.md`).
- **Pair 2, the slot choice.**
  - `debug_assert!` that the landed sequence is monotone in
    `SlotChoice::landed`.
  - A job id echoed from `SyncHelper::start` to `wait`, so a `?` between them
    (db.rs:39669-39701) cannot leave a stale result for the next wait
    (`syncop/check.md`).
  - Neither changes a success path's syscalls.

### G1 (the wall, the door)

- **A gate that holds the form-site list needs a different key.** If a gate
  holds R8's list of sites that read a form page, it cannot be `sites.py` as
  written. That script keys by (file, function name), searches 12 files, and
  passes four mutants of the skeptic's: a read added inside an existing META
  method, two new readers that reuse a classified name, and a read in an
  unsearched file.
  - The gate's input is the table keyed by (file, impl owner, name), or by
    span. It fails on a READS seed inside a non-READS row, and it searches
    every file (`formsites/check.md`, M6-M9).
  - A path handed in from outside (M8) is out of reach of any text search. It
    is held by the door by symbol, which is already G1's job, and by no grep.

### G2a (the conform pass on today's pager, the poison draw)

- **The poison draw cannot hold the form sites' `None` arms on today's
  pager.** `Entered::finish` -> `end_slow` (db.rs:19064-19070) replaces every
  answer with the poison, so no test that runs through the store can see a
  `None` arm answer wrongly (`formsites/check.md`).
  - T4 in `formsites/sites.md` is the test that holds those arms. It is a
    zfile unit seam that makes `page_bytes` answer `None` for one page,
    before and after that page is verified, with the memo off and on, across
    the listed methods. Each must answer `Fallback`/`Out`/fell, never
    `Served` with an absence.
  - T4 is owed whatever R8's shape, so it lands with G2a. It also gives the
    poison draw an input for forms: G2a's gate counts T4.

### S1 (typed handles; verify at publish; R8's `Result<_, Damaged>`)

- **The site list for R8 is `formsites/sites.md`** with the check's
  corrections:
  - row 46 has five attach calls, not four;
  - `Opened::bytes` belongs with `ZCursor::group`;
  - row 3 names `engine_filter` and `ZFence::decode_resident`;
  - seven more fence-entry paths go raw without `fail` and leave the form in
    service: `locate_rank`, `locate`, `ZFile::seek`, `scan_in` ×2,
    `ZCursor::seek`, `ZCursor::load`.
  The check's third derivation found no site missing from layers 1-2. Its
  completeness claim covers layers 1-3, not ABOVE.
- **What the page layer's interface must offer the sites.** Items 1-6 of
  `sites.md` §"What the page layer's interface must offer", plus the check's
  item 7: a file's name and its bytes are one identity. Retiring or renaming
  a Regen-lazy file makes its old bytes unreachable under the live name.
  Today's vault keys envelopes by (token, kind), so it does not hold this.
- **Fence damage under R8.** The fence-entry fallbacks above are "fence
  damage". R8 should decide whether the attach's resident-part CRC makes
  them unreachable, or whether they `fail` as page damage does.

### S2 (`pagefile.rs`, the long-name spill, the extended model)

- **No Replace class** (Amendment 3). E4 F6's session findings become moot.
- **Log class sync rules.**
  - W (the wait) stays and is load-bearing. The work said W was
    load-bearing only for failures and that M alone would make dropping W
    correct. The check withdraws both. On one file description, an `Ok` can
    return over a failed writeback whose error went to the other sync. With
    no wait, the max rule, K and K' on: 99,360 LOSS. With the wait: 0
    (`syncop/check.md`, the steal ops Y/V).
  - W may be dropped only where each sync owns its own description.
  - Also write `synced` by the max rule (M): with the wait removed and syncs
    succeeding, assignment loses 308,176 at R=8.
  - `debug_assert!(tok > synced_tok)` at completion.
  - Tests:
    1. A held sync makes the second request wait.
    2. With the sequencer off by a seam: the newer sync completes, then the
       older, then a ring lap, then a power cut. The newer content survives.
       Transcribe `B 1 S 1 E 1 1 1 1` at R=8.
    3. A failure reaches a sync already in flight: `1 B B G E`.
    4. **Added by the check, the steal:** two syncs on one description. The
       error is delivered to one, the other returns `Ok` first, and the
       answer must be the failure: `1 B B Y`.
  - Every test that holds the in-flight pin runs at maxlen ≥ 7; at 6 the
    pin is unreachable (`syncop/check.md`).
  - The oracle keeps its own max of what was acknowledged. With the oracle
    assigning instead, the naive rule's loss is invisible. The crash-state
    API's oracle in the tests must keep a max too.
- **The long-name spill: designed, with one required fix** (`spill/design.md`
  as amended by `spill/check.md`).
  - Layout: name pages at stored 8-24 KiB; the ring and REC region starts at
    24 KiB. A SPILL bit in the sealed flags. The spilled field holds the
    slot, the record's tag (rtag) and the length, spelling 125 characters.
    The inline budget is 128 bytes, a computed constant.
  - Binding: resolution requires the record's tag to equal the entry's rtag.
    (ns, slot) alone fails (mutant `bind=seq`, ops pLP).
  - `vault::sync_dir` fdatasyncs every file with a pending name record
    before it fsyncs the directory. This is an exception to "no sync the
    engine did not issue". It fires only with a spilled name pending. It is
    unpriced. It is load-bearing on the link path specifically (`dslink`
    mutant, 74,681 UNNAMED).
  - **Required fix, and a bug in `design.md` §4 itself.** When the open
    resolves a duplicate name, it must fdatasync the kept entry's file (for
    a spilled entry) and fsync the directory before it unlinks the other
    entry.
    - Without the fix, a process crash between a rename-over's rename and its
      `sync_dir`, then a power cut, gives UNNAMED: 5,797 (subset, `rXC`) and
      324 (prefix/ext4, `CrXC`) at n=4, with cuts at later barriers.
    - Under the subset order this is design.md's own rename-over failing for
      inline names too. So §4's "keeps the higher name_seq and unlinks the
      other" changes whether or not the spill is built.
    - With the fix: 0 at n=4 for `cutk` 1-4 (subset and prefix), and 0 at
      n=5 for `cutk` 1-3 (subset).
  - Gaps the design must close before it is built (`spill/check.md`, by
    reading):
    1. A promotion's link writes a name record into a finished file, which
       has no descriptor. Say how the layer gets one, and that `FINAL` and
       Once trust do not cover the name pages.
    2. §5.4's "unresolved means no `sync_dir` returned" is false for a ghost
       whose slot was reused, and under damage. Presenting a damaged durable
       entry as `spill-*.tmp` sweeps it, so `Db::unnamed_refusals` never
       judges it. With a damaged newest slot, that removes a merge output
       the engine would have refused over.
    3. "Merged with the header read" needs eager header reads at listing,
       which the open does not do (headers are read lazily at step 5).
  - Tests:
    - G-names, over the constructors;
    - synthetic long-name verbs and reopen;
    - the M2 state through a seam;
    - the duplicate-resolution cut above (a rename-over whose `sync_dir` has
      not run, a kill, a reopen, a power cut);
    - a KEPT_OLDER check, that the open keeps the newest entry made. The
      work's AMBIG never judged what the open keeps, and `keep=low` survived
      it;
    - `name_reads` asserted in `tests/cipher.rs`.
  - Two of the design's sentences are held by no property:
    - "a's slot is still referenced" (`rfree=first` survives);
    - "reseed ns at max listed + 1" (`ns_open=resolved` survives).
    Either write a property for each or drop the sentence.
  - Corrections: `seg-` is 49 bytes, not 50. The `.pfcopy` suffix is moot
    for `par-`. The constructor list stands, re-derived from the fence
    sources.

### S3 and S4 (Once and Regen wired; the Log class only if S3 pays)

- **The Once class's REC slot choice needs a model before S3 wires Once.**
  The slot that is not the last completed sync's is a known-durable choice
  in which M is load-bearing without W, as the manifest's is. Neither the
  work nor the check modelled it (`syncop/check.md`, missed).

### G3 and S5 (`Memory`; the controller on under a cipher)

- **A rebuild under a cipher re-attaches the envelope it retired.** Once a
  form failure stops poisoning the store (R8, or owner question 2),
  turning the controller on under a cipher makes the following path live
  (`formsites/check.md`, traced by hand):
  1. `z_retire_cell`'s `vault::rename(N, dead)` moves only the map entry.
  2. `vault::companion` re-adopts `x-T.z` under `N`, reusing the dead
     entry's view.
  3. A damaged form returns to service until `ZREBUILDS` rounds pass, after
     which the partition is `DECLINED_DAMAGE`.
  Answers stay right; the repair does not happen. S5 must therefore carry:
  - the vault change: a retired form's envelope is unreachable under its
    name, either by a rename that moves it off the token name or by a
    companion that refuses a `Disk` a non-form name holds;
  - T5: a new attach id, `rebuilds == 1`, `declined_damage == 0`, the old
    bytes gone, with the two mutants named in the check.
  The same path exists today without damage, at the ahead-coding check
  (db.rs:42889-42895). Under a cipher it re-attaches a file in the coding
  that was refused (see below).

### S6 (the log and the slots as sealed records; the default flips)

- **`replace(bytes)` in the door.**
  - The record format is `slots/design.md` §2, with Amendments 1 and 2:
    head in the clear, length bound by the head, no chain, key from K_f.
  - The nonce is drawn by the door, with a count never given back.
  - The door answers EMPTY / VALID / DAMAGED. `manifest_read_all` maps
    DAMAGED to torn and never propagates it with `?`.
  - Two scribbled slots must read as a refusal, not as no manifest (66,932
    A_EMPTY under `torn_empty`).
  - Say that EMPTY widens from a zero-length slot (db.rs:12719-12721) to one
    of all zeroes, or keep zero-length.
- **The ordering contract `replace` must promise.** A replace that reports
  failure has not landed whole. A replace that fails after its last byte
  reached the cache gives 16,774 WRONG on the fixed protocol (`w_whole`,
  `slots/check.md`). Today's buffered `write_all_at` cannot do this.
  Alternatively, the engine treats a failed publish as possibly landed.
  Whether such a landing harms the engine was not checked.
- **The open's slot rule is the S0 rule.** S6 adds no open steps of its own.
- **Stream class sync rules.**
  - C (coverage captured once, at issue) and K (the kept error) are
    load-bearing in every shape. Assert both.
  - W is load-bearing (the steal: 14,244 LOSS without it, 0 with it). Keep
    W. The work's "keep W or state K'" is withdrawn.
  - M costs nothing; write it.
  - The slot syncs stay on one thread, with the monotone assert from S0.
  - Tests:
    - the kept error with two syncs in flight;
    - C: `B 1 B E`;
    - the steal: `1 B B Y`;
    - a kept error answers every later sync. Assert the answer: the model's
      early-answer guard is unreachable (`early_ignores_err` survives), so a
      test that waits for a loss holds nothing.
- **Tests for the slots** (`slots/design.md` §8):
  - a record moved between files is refused;
  - two damaged slots are refused;
  - a torn slot over a longer older image;
  - no nonce repeats across a failed write and a reopen.

### S7 (WARM)

Nothing here changes S7.

## The engine as it stands (bugs and hardening for the plain store's queue)

1. **H2 (model, and the tree read by the check).** The open fdatasyncs only
   the slot it takes (db.rs:28797-28812). A slot it passed over as torn in
   the page cache can come back after a power loss as an older whole record
   that is newer than the one it took. That gives 3,256 LOSS_BRANCH at
   length 4 with no write or sync error (`slots/check.md`). E4's F6 fix
   synced both slots; the engine kept half of it.
2. **H1 (model).** After a failed slot fdatasync, which is never retried, a
   reopen reads the record from the page cache. The open's own fdatasync
   returns `Ok` over clean pages, and the record is counted durable when the
   device does not hold it.
   - It rests on Linux keeping a failed writeback's pages clean and cached.
     That was not tested on this kernel.
   - The check's `ignore_fail=1` changes no verdict. So H1 rests on the
     reopen's sync returning `Ok`, not on errseq propagation.
   - By the same device model, the same shape covers the WAL replayed after
     a failed log sync, a recovered run temp, and pending segments read back
     under `manifest_pending`. This is reasoning only, not modelled.
3. **Pair 1, the writer and its helper on one description (read, not run).**
   The helper's `sync_data` on a `try_clone` (db.rs:6775, 6838-6852) can
   consume a writeback error and not yet have stored it when the writer's
   fdatasync returns 0 and `synced_all` loads nothing (6320-6332). A run
   commit or `sync` is then acknowledged over a hole. The shape is the
   steal, not `err_later_only` (`syncop/check.md`).
4. **Pair 2, the slot choice (read).**
   - `SlotChoice::landed` assigns (12357-12359). It is sound because the slot
     sync is started and waited for on one thread, and because each slot
     write opens a fresh descriptor (`vault::open_slot`, 12527), so no steal
     can happen there.
   - A `?` between `start` (39669) and `wait` (39701) leaves a stale result in
     the channel. That is harmless only because a failed `land_readable`
     stops the seal queue. Harden it with the job id.
5. **Pair 3, the presync beside the seal's fsync (read): sound.** The job
   lands durable only at the writer's word after the presync, and in the
   inline arm the two are serialized.
6. **Form-page reads (read): none can answer wrong.** Every `None` arm of
   layers 2-3 falls back to the raw form. No site turns a failed read into
   `Absent` or `Served{n:0}`.
   - The arms are reachable only under a cipher, where the poison overrides
     the answer anyway. No existing test can see one answer wrongly.
   - Hardening, not bugs:
     - the seven fence-entry fallbacks and three more paths leave the form
       in service without `fail`;
     - two sites re-ask through `open_group` instead of failing where they
       meet the `None`.
7. **The vault under a cipher (read): a retired form envelope is
   re-adopted.**
   - Today this happens at the ahead-coding check (db.rs:42889-42895): a form
     renamed to a dead name and then attached comes back in the refused
     coding.
   - A `persist_form` / `remove(dead)` race that could unlink a fresh
     envelope is plausible from reading and not confirmed
     (`formsites/check.md`).
   - The controller is off under a cipher by default
     (`zpages_under_cipher`), so neither is reached in a default store.
8. **Long names: nothing spills today.** The longest name is `par-` at 115
   of 128 bytes. At `FENCE_MAX` doubled, only the merge-output `par-` spills.

## Still owed

- **The slot record's device probe** (`slots/design.md` §7): `T`/`D`/`P` at 2,
  50, 400 and 4,000 segments, the open's `O_T`/`O_D` from warm and cold, and
  the kill rules fixed in advance (`D/T` > 1.05 kills `D`; the open's added
  cost > 0.5 ms goes to the owner). It is not run here because another job
  has the machine. It is owed before S6, and the open's half before H1/H2
  land.
- **The fixes to check against the later open unit.** H1, H2 and the pair-2
  hardening are checked against the open-fix unit not in this tree, and
  each gets a failing engine test.
- **The engine's slot rule at length 5.** Its runs were stopped unfinished;
  it is claimed at length 4 only.
- **Not modelled for the slots:**
  - the open's sync of the untaken slot failing;
  - `manifest_pending`'s skip of a newest slot;
  - the v1/v2 slot upgrade;
  - the names layer's spelling of slot files and where their file id comes
    from before the manifest is read.
- **Not checked:** that `FADV_DONTNEED` after an fdatasync drops the slot
  files' clean pages, or that `O_DIRECT` is usable under the door.
- **H1 beyond the slots** (WAL, run temp, pending segments): a model.
- **syncop:**
  - the steal ops combined with crashes (X/P) or partial writeback;
  - three or more syncs in flight;
  - JRP and ADJ under the new ops;
  - the Once REC choice;
  - pair 1's window reproduced by injection;
  - Linux's one-report-per-description errseq behaviour, on this kernel.
- **spill:**
  - the fix at n=5 for `cutk` ≥ 4 and under prefix at n=5;
  - concurrency, failed name-record writes or syncs, a failing `sync_dir`,
    damage (gap 2 above), `.pfcopy`;
  - an audit that every publish naming a file calls `sync_dir` after its
    directory op (nine sites were read, not every publish);
  - the price of the extra fdatasync and of the open's name pages;
  - the `.tmp` presentation depends on E0 item 4, still undecided.
- **formsites:**
  - T1-T5 and every mutant are written down, not run;
  - whether the controller path reaches `z_attach_on_disk` with
    `refused == false` for damage met in service under `Auto`;
  - `abandon_asked` callers inside key loops, for a `ZCursor` held across a
    rest;
  - tests/ was not searched.

## Questions only the owner can answer

1. **How far does H1's fix reach?** It reads past the page cache after a
   failed sync.
   - (a) Slots only. One or two device page reads and two fdatasyncs of
     nothing, at open. The WAL, run temp and pending segments keep the same
     exposure, documented. The exposure needs a failed sync followed by a
     reopen within the cache's life.
   - (b) Every file the open reads to recover. Recovery after a failure would
     read the whole replayed log and any run temp from the device. That is
     unpriced, and its cost scales with the log's size at every open, unless
     the store can tell an open after a failure from one after a clean close,
     which nothing durable records today.
   - (c) None. Document that a failed sync followed by a reopen in the
     cache's life may report durable what is not.
2. **A failed tag on a form file under a cipher, before the page layer.**
   - (a) Make it file-local now, which is the plain store's policy. This
     takes the pager class on `Back`, the vault change, and T1-T5. It costs
     no success path, by the design in `sites.md`, and it makes the
     re-adoption path (engine item 7) live, so the vault change is required.
   - (b) Keep it store-fatal until R8 lands in the layer. This is louder than
     a plain store, but nothing new is built twice.
   R8 already decides file-local for the layer, so this question is only one
   of timing.
3. **Should a form failure be surfaced to the caller?** With file-local
   failure it is counted and repaired, and the caller sees nothing.
   Surfacing it costs an API (an error or an event) the plain store does not
   have today.
4. **Build the long-name spill at all at S2?** No name spills at today's
   `FENCE_MAX`.
   - (a) Build it as amended. That is 200-300 lines by forms.md's estimate,
     plus the duplicate-resolution fix, the three gaps, the tests, and an
     unpriced sync rule. All of it exists for fences over about 19 bytes,
     which no store reaches today.
   - (b) A const assert that every constructor fits the inline budget at
     `FENCE_MAX`, with the spill left on the roadmap until `FENCE_MAX` grows.
     This is cheaper and leaves nothing to rot. The duplicate-resolution fix
     to `design.md` §4 is needed either way.
