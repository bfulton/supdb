# Decisions on the encryption seam

2026-11-17. The owner handed the open decisions over: "Use your best
judgement. The goal is an encryption feature which is reliable, performant,
and not fragile to maintain." Each decision below names which of the three it
was made on and what was measured for it. Nothing here is built yet.

Where the notes disagree, the later one stands, in this order: this file;
`work/fix/conclusion.md` (it amends `design.md` §3, §4, §8 and stages S4 and
S6); `forms.md` (the contract and stages G1 to G3); `binding.md` (decision 1);
`once.md` and `page.md` (E8 and E9, decision 3); `design.md`;
`experiments.md`.

## The bar every stage is held to

- **Reliable.** Every crash window a stage opens is in the crash model before
  it is built and in a fault-injected fuzz after; a fix starts from a failing
  test; tests are held to mutants; a failed sync is never retried.
- **Performant.** A store in the clear pays no instruction per key, entry,
  block or step: counted with callgrind by function against the merge base,
  never argued. A mechanism under a cipher is priced on the cell it exists
  for before its default is chosen.
- **Not fragile.** A form change touches no encryption module and needs no
  cipher test; a change that would be wrong under a cipher fails the author's
  own tests in the clear or does not compile.

## Decisions

1. **Store-level integrity once a run is its own log: option (c) of
   `binding.md`.** The run is bound to the log at its three boundaries, bound
   again at a recovering open, with an end rule for the unsynced band. Agreed
   with the owner earlier; recorded here for completeness. It costs nothing a
   commit under `Always`. It is built in the plain store, which has the same
   exposure, and only when stage S4 is taken.

2. **The log, the spare and the manifest slots go behind the door as sealed
   records** (the Stream class of `work/fix/conclusion.md`, each record's
   associated data chained to the tag before it). Not as paged Log files, and
   not sealed by the engine as today.
   - Performant: on the device the records behind the door cost what today's
     sealed log costs, and that costs what the plain log costs; the paged Log
     lost a fifth to a quarter at every commit size from 4 KiB up.
   - Not fragile: the engine keeps no sealing code, so a new frame kind or
     manifest field is not an encryption change.
   - Reliable: sound in the crash model once the chain is in; the unchained
     shape adopted a stale record after a reopen.
   - Consequence: the run temp is the only Log file and is never resumed, so
     `continue` sessions, `TRUNC`, the resumed-header sync, the re-home after
     an unclean open and the session-table compaction leave the Log class.
   - Owed before it is built: the manifest slot as a sealed record was not in
     the probe or the model; both come first.

3. **One plaintext budget, owned by the layer and read by forms only through
   `Memory`** (option (b)). The caller's `cache_bytes` is a cap on it.
   - Performant: misses a read follow the plaintext held, so the budget is the
     out-of-core lever; a fixed large budget was killed in a load.
   - Not fragile: no option resolves differently by encryption mode in form
     code, so the controller stops being switched off under a cipher.
   - Reliable: the margin is a gate on the stage that builds it, not a number
     chosen here: a load and a read survived in every sitting, two stores in
     one process, and a caller whose own memory grows.

4. **The leakage listed in `design.md` §12.4 is accepted and documented** in
   the README and `SECURITY.md`: file count, sizes to the page, commit sizes
   to within a few tens of bytes, rewrite patterns to a watcher of the device,
   reopen counts, links of one file, timing, plaintext in memory. No padding
   by default, since it is bytes and time on every commit; padding commit
   records to buckets stays a roadmap option.

5. **Linux only through S7**, said in the README. A portable plaintext backing
   is a roadmap item, priced in the same pairing when it is built.

6. **The sparse stored prefix is accepted**, with apparent against allocated
   size documented. The prefix sizes are layout constants, looked at again at
   S2 against a count of files a store: with decision 2 most files have no
   ring. A small-file layout stays on the roadmap.

7. **A log whose first sector reads as zeroes is an empty log**, as the
   durability fix built it; a zero header over a first sector that holds
   anything else is damage and refused.

8. **`sync` does not answer a stopped seal queue; `flush` does.** What `sync`
   promises is in the log and holds. Answering it there is a load in every
   `sync` for nothing a caller loses.

9. **The contract of `forms.md` §3 is adopted whole.** The wall and the door
   by symbol, whole borrows private, the conform pass over today's pager, one
   `Memory` interface, the cost job. G1 and G2 land before S1, so the layer is
   built under them.

10. **A durability fix that adds a wait, a syscall or an unfolded load to a
    success path is priced before it lands.** It lands when no pairing
    resolves a cost; otherwise the numbers come to the owner.

## Order

1. Stage 0: the plain store's durability fixes (running).
2. The fixes held back for pricing, with the machine to itself.
3. G1: the wall, the door, whole borrows private, the silent no-ops rewritten.
4. G2: the conform pass on today's pager, both tiers, and the poison draw.
5. S1: typed file handles through the door; threads and waits through `pin::`.
6. S2: `pagefile.rs` standalone, with the long-name spill and the extended
   crash model.
7. S3: Once and Regen files wired behind an arm, paired against today's shape.
8. S4 with decision 1, only if S3's load pairing says the Log class pays.
9. G3 and S5: `Memory`, then the controller on under a cipher.
10. S6: the log and the manifest as sealed records behind the door; the
    default flips; the envelope arm goes.
11. S7: WARM, only if its instruction count comes in under the bar.

A stage that fails its own gate stops there and comes to the owner with the
numbers.

## The G1 and G2 plan (2026-11-17, after the scouting)

The scouting (`plan/g1.md`, `plan/g2.md`, each scout's notes re-derived by a
skeptic in `plan/*/check.md`) amends `forms.md` in eleven places, listed at
the end of each plan. All eleven are taken: each comes from a constraint that
was checked (what Rust's visibility can say, what today's pager gives back,
a cipher test that holds an open to a handful of chunks), none from
convenience. Three were left as questions.

11. **The thread half of the door waits for S1**, where the `pin::` wrappers
    are built. At G1 a per-file count of `std::thread::` holds growth.
    Not fragile: barring it at G1 is an allow on 55 functions, each "owed to
    S1", and a gate that starts with 55 exemptions teaches the next one.

12. **The wall compares totals by (file, token, region)**, with the scopes
    printed beside a failure. Not fragile: a gate that asks for an approved
    edit every time a coupled function moves or is renamed is one people
    learn to edit without reading. Growth anywhere still fails.

13. **The hidden accessors are built** (`Db::partition_fences`,
    `Db::form_fence_keys`, answered from the state and never from the
    directory). Reliable: about fifty tests come under the conform pass now
    where they would wait for S3. Not fragile: a test that asks the store is
    not a test of how the store is kept.

14. **An unpinned writer is refused under a cipher** (G2-1). It is the rule
    the store already has for `writer_pins` off, sound only while nothing
    else frees what the writer reads, applied to the evictor, which does.
    The option is a pricing arm (`supdb-nopin`), no feature of a store; the
    refusal is said in the README's list of what a cipher store does not
    take, with its reason.

15. **G2 is split.** G2a is on today's pager and stays through S3. G2b (a
    cache of a few pages exactly, the guard exact to the chunk, the
    crash-state API that brings the byte-level crash tests under the pass)
    follows S3, since each needs eviction by the page. Until then the pass
    holds no crash window on the paged path, and says so.

16. **The rewrites of G1-4 and G1-6 change what a cipher store does only
    behind the compact form or `merge_drop_behind`**, neither a default
    there. Their pricing is S5's, and the commit messages say it.

Order within the stages is the plans'. Stage G1 follows the priced
durability fixes; its three gate units (wall, door, cost) change no
instruction of any build.

## The owed items, concluded (2026-11-17)

Four items were owed before later stages: the manifest slot as a sealed
record, the model's op for syncs that overlap, the long-name spill, and the
list of sites that read a form page. Each was built in Python models or read
from the tree, re-derived by a skeptic, and concluded in
`work/owed/conclusion.md`, which is the stage-by-stage list of what they
change; where a check corrects its work, the check stands. Nothing in them was
compiled or run on the engine. They amend the decisions above in four places
and left four questions, decided here on the same rule as before.

Amendments:

- **Decision 2, the slots carry no chain.** A slot is one sealed record at
  offset 0, rewritten whole. A chain to the previous publish refuses legal
  states, a chain across the slots needs the engine's slot choice inside the
  door, and neither catches the one hazard the model found. The log and the
  spare keep the chain as decided.
- **Decision 2, the slot's key is the file's.** A key derived from the
  file's own key with a prefix a handle, as the Stream class has, which is the
  key rule already written (the store's key seals nothing under a random
  nonce). The slot's index stays in the associated data and is cited as
  protecting nothing: what keeps a moved record harmless is that the engine
  takes a slot's index from the file it read.
- **Decision 2, Replace leaves S2.** Its only users were the two slots and
  the rename shape's spare, and the rename shape is refused under a cipher.
- **Decision 4's leakage list** gains nothing: the spill is not built (20).

17. **The open settles both slots (H2), now.** The open fdatasyncs the slot
    it takes and leaves the other to the kernel's writeback, so a slot read
    as torn from the cache can be whole on the device for some seconds, and a
    power cut there hands the next open a state the last one passed over. It
    is the plain store's defect as much as a cipher store's and is the first
    unit of the pricing run (`other-slot`): a failing test first, then one
    more fdatasync at the open, of nothing where the file is clean.
    Reliable: a store that refuses every open with its commits in its log is
    the worst answer the engine has. Performant: nothing off the open.

18. **A failed sync across a reopen (H1) is not fixed on a model.** After a
    failed fdatasync Linux leaves the pages clean in the cache; a reopen
    reads them, and a fresh descriptor's fdatasync answers Ok over bytes the
    device may not hold. It is every file an open reads back (the slots, the
    log, a run's temp file, pending segments), and the remedies differ: the
    slots need a read past the cache, since writing either slot there can
    destroy the only copy the device holds; the log's replayed frames need
    to be written again into fresh pages (a new log file, or a seal of what
    was replayed, which is what the other engines do at the end of a
    recovery), never over themselves, where a torn sector would cost frames
    that were durable. None of it can be tested here: this guest's kernel has
    no device mapper and no fault injection, and a fix held only by a seam
    that pretends the device answered is a fix held by its own assumption.
    So: the docs say what is true (after a failed sync what the device holds
    is not known, and a reopen while the kernel still caches the files reads
    what the device may not hold); ROADMAP's item beside the fault-injecting
    device names all four files and the remedies with their reasons; and the
    CI job that has the privilege is where it is built and held. Told to the
    owner as the one durability hazard left open by choice.

19. **A failed tag on a form file under a cipher stays store-fatal until R8
    lands with the layer (S1)**, where it becomes the file's alone, as a
    plain store's is. Not fragile: the interim would be built on today's
    pager and built again at S1. Two things move to S1 with it, not later:
    the vault's change that makes a retired form's envelope unreachable
    under its name (file-local failure makes the re-adoption path live for
    any store that builds the form, and `ZPages::Always` is one today), and
    its test (T5: a new attach id, one rebuild, no decline, the old bytes
    gone). T4, the unit seam that makes a form page's read answer nothing at
    each listed site, lands with G2a, where it is the poison draw's only
    input for the forms. The form-site list for R8 is
    `work/owed/formsites/sites.md` as its check corrects it; a gate over it
    is keyed by (file, impl owner, name) and searches every file.

20. **A form failure is counted and repaired, never surfaced**, as in a
    plain store. An error or an event there is an API no store has today,
    for a failure the store recovers from by itself.

21. **The long-name spill is designed and not built.** No name spills at
    today's `FENCE_MAX` (the longest is 115 bytes of 128). A compile-time
    assertion that every name constructor fits the inline budget at
    `FENCE_MAX` stands in its place, with the design and its check
    (`work/owed/spill/`) for whoever grows the fence. Not fragile: the spill
    is two to three hundred lines, a sync the engine did not issue, three
    gaps its own check found, and no store to run it. This is no limit on a
    store: every store's names fit, and a change that would make one not fit
    does not compile. R10 is restated so. The duplicate-name fix the spill's
    model found is the names layer's whether or not anything spills: the
    open, resolving two entries for one name, makes the kept one durable and
    fsyncs the directory before it unlinks the other.

22. **The sync rules of the classes are the check's.** A sync waits for one
    in flight on the same file description (two syncs of one description can
    hand the device's error to one and Ok to the other); what is synced is
    written as a maximum, never assigned; coverage is taken once, where the
    sync is issued; a kept error answers every later sync. Each has its test
    by the model's trace, run at a length where the in-flight state is
    reachable. The Once class's record slot choice is modelled before S3
    wires it. For today's engine the same analysis gives one defect, the
    segment writer's helper syncing on the writer's own description, already
    a unit of the pricing run, and two assertions for the slot choice.

Still owed: the slot record's device probe (a timing run, before S6); the
model of the Once class's slot choice (before S3); the budget margin gate
(G3).
