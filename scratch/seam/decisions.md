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
