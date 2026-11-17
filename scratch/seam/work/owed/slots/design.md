# The manifest slot as a sealed record behind the door

Owed item "slots" of `decisions.md` decision 2: "the manifest slot as a
sealed record was not in the probe or the model; both come first". This is
the model half and the probe's specification. Tree `682b2c09`, read only;
one later unit of durability fixes to the open is not in it and was not
read. Every claim below is either a line of the tree I read (cited), a run
of the model beside this file (`notes.md` has each command and its count),
or marked as reasoning.

## 0. The answer

- A slot is **one sealed record at offset 0 of its file, rewritten whole in
  place** by the publish that chooses it. It is the Stream class's record
  format (head in the clear, ciphertext, tag; nonce drawn by the layer) at
  Replace's position (always offset 0, never appended). It is a third
  operation of the door, `replace(bytes)`, beside `append` (log) and the
  paged classes, and needs no header, no session table and no pages.
- **The nonce is the layer's**, drawn at every seal: the store's per-open
  random prefix and a count advanced at every seal attempt, never given
  back (today's `Nonces`, vault.rs:306-331).
- **The associated data binds** the record's kind (domain), the store's id
  (the marker's, vault.rs:365-377), the file's identity (its `file_id` and
  logical name, which carries the slot index) and the record's length. The
  sequence is not in the AAD: it is in the engine's plaintext (the
  manifest's own header, db.rs `manifest_encode`), which the tag covers,
  and the engine chooses by it. So the door knows nothing of sequences, and
  a manifest field is not an encryption change (decision 2's point).
- **No chain.** "Chained to the tag before" has no object for a record
  rewritten in place: per slot, the record before is the one this write
  destroys, so nothing can check it; across the two slots, the model shows
  a chain is sound only when it names the record the engine knows durable
  in the other slot (the engine's `SlotChoice`, which the door would then
  have to know), it buys nothing against a slot put back (rollbacks
  accepted 70,489 with it against 70,787 without), and it does not catch
  the one stale-record hazard the model found (H2 below). The job the
  Stream chain does for the log -- refuse a stale record at a position
  reused after a lost one -- is done for the slots by the sequence under
  the slot rule plus the open's sync of both slots.
- **A torn write reads as damage (TORN), never as empty**, and a slot
  whose tail is a longer older image's reads as the new record: the reader
  authenticates exactly the length the head names and reads nothing past
  it.
- **The open's rule changes in two places, both of them the plain store's
  as much as a cipher store's** (found by this model, not verified on the
  engine):
  - **H2**: the open must fdatasync **both** slot files before it removes a
    file or writes a slot, not only the one it takes. With only the taken
    slot synced, a slot the open passed over as torn can come back on the
    device as an older whole record newer than the one taken: 3,240
    violations at length 4 with no write error and no sync error, first
    `U K1` (an owed publish, a kill inside the next write of the same slot,
    then a power loss). E4's F6 fix (`open_sync`) synced both; the engine
    took the half that syncs the slot taken (db.rs:28750-28770).
  - **H1**: the open must read the slots from the device, not the page
    cache, after a failed fdatasync: Linux marks the pages of a failed
    writeback clean and keeps them in the cache, so a reopen after a
    failed slot sync (a kill, or a close and reopen in the same process)
    reads a record the device does not hold, its own fdatasync returns Ok
    over nothing, and the store counts it durable: `V X` (CLAIM, then LOSS
    on a power loss), 10,884 violations at length 4 with H2's included.
    The fix modelled: fdatasync both slot files, then drop their clean
    pages (`POSIX_FADV_DONTNEED`) or read them `O_DIRECT`, then read.
  - With both: 0 violations in every run (section 5).
- **Replace is unnecessary for S2.** Its only users are the two slots and
  the rename-shape arm's spares (design.md:90), which design.md already
  refuses by name under a cipher (design.md:94). What is lost if it goes is
  listed in section 6.

## 1. Today

- `Vault::seal_manifest` (vault.rs:1628-1652): `SUPDBMAE\x01 | seq u64 |
  len u32 | nonce | ciphertext | tag`, sealed under the store's key `K_s`,
  AAD `"manifest" | seq` (vault.rs:1753-1758), nonce from the vault's
  `Nonces` (per-open prefix, count). The plaintext is the engine's manifest
  (with its own CRC) and the segments' tokens and wrapped keys.
- `open_manifest` (vault.rs:1656-1681) refuses a wrong magic, a short slot
  and a failed tag; reads only `SLOT_HEAD + len` bytes, so a longer older
  image's tail is ignored.
- `manifest_write_slot_unsynced` (db.rs:12516-12536): `write_all_at(out,
  0)`, never truncated; the file returned for the caller to sync.
- `SlotChoice` (db.rs:12343-12376): a publish writes `1 - durable`;
  `landed` only after an fdatasync covering the slot's last write returned.
- `Maint::write_slot` (db.rs:40839-40870) takes the owed entry
  (`slot_sync_due.take()`) before the write, so a failed write leaves
  nothing to name the slot durable.
- `manifest_read_all` (db.rs:12709-12735): an empty file is skipped, a slot
  that fails is torn, valid ones sorted by seq; no valid and one torn is a
  refusal.
- `Db::open` (db.rs:28740-28775): reads every slot, takes the newest valid
  (after the pending rule), fdatasyncs **that slot's file**, `landed`, and
  only then sweeps.

What today's AAD does not bind: the slot index (a record moved into the
other file opens), the store id (`K_s` is per store, so this costs nothing
today), the length (the length is in the clear and covered only because a
GCM tag over a ciphertext of another length fails).

## 2. The record

```
stored at offset 0 of the slot's file:
  head (clear): magic u64 | version u8 | flags u8 | len u32 | nonce 12
  ciphertext of len bytes
  tag 16
AAD = "supdb slot v1" | store_id 16 | file_id 16 | logical name (padded) | len u32
key = K_s (or a key derived from it for the door's records; either: the
      nonce source is one per key)
plaintext = the engine's bytes, opaque to the door (today: the manifest
      with its CRC, then the segments' tokens and wrapped keys)
```

- **Who draws the nonce.** The door, at every `replace`, from the store's
  one nonce source for that key: an 8-byte prefix drawn at each open of the
  store and a 32-bit count, advanced when the nonce is taken (before the
  write), never given back, the prefix drawn again on overflow. Mutants:
  the count given back after a failed write (`count_reuse`): 9,319 NONCE
  at length 4, first `w1 W`; the prefix fixed per store (`prefix_fixed`):
  17,105 NONCE, first `K1 W` (a kill, reopen, the count restarts).
- **What the AAD binds, and why each.**
  - The slot (`file_id` and logical name). Without it a record moves
    between the files and is accepted (adversary model `noidx`: 123,152
    A_RELOC at length 5). Moving one is no worse than a put-back for the
    state answered (both are published states, and the slot rule's claims
    are about the bytes in the file, which a moved record is in), so the
    binding buys little: it makes a swap or a copy a refusal where it was a
    benign open, and it makes "the record of this file" a statement a test
    can hold. It costs nothing (AAD is not stored). The crash model cannot
    see it (no crash moves bytes between files: the `noidx` run is 0); the
    adversary model is where it shows.
  - The store id. Redundant with `K_s`, which is drawn per store; two
    directories copied from one store share both, so neither tells them
    apart. Bound because the door binds every record the same way, not for
    a threat.
  - The length. Bound so the reader's bound on what it authenticates is
    itself authenticated; the head's clear length is what lets the reader
    ignore a longer older image's tail without a stored file length.
  - Not the sequence (section 0), not a predecessor's tag (section 3).
- **What a read answers.** `EMPTY` when every byte is zero or the file has
  no bytes (made at the store's birth, `manifest_make_slots`, and never
  durably written); `VALID(plaintext)` when the head's length fits, the
  bytes are there and the tag checks under this AAD; otherwise `DAMAGED`.
  The door answers `Ok(None)`, `Ok(Some(bytes))` and `Err(damaged)`, and the
  engine's `manifest_read_all` maps them to its three cases as it does
  today (skip, valid, torn) -- it must not take the `Err` through `?`
  (design.md:320 names that trap for Replace).
- **A torn write** (a power loss keeping some sectors of the new image, a
  kill inside the write call, a write error part way) leaves either the new
  head over old bytes (tag fails), the old head over new bytes (tag fails),
  or the old image whole where none of the new sectors landed (a VALID
  older state, which the slot rule makes legal). It never reads as empty:
  the adversary mutant `torn_empty` makes two scribbled slots read as a
  store with no manifest, 66,932 A_EMPTY at length 5, which today's open
  answers with the pre-manifest scan that removes every `par-`/`pcs-` file
  (binding.md §5).
- **A slot whose tail is a longer older image's** reads as the new record:
  the reader takes `len` from the head, authenticates head, `len` bytes and
  the tag, and nothing after. The model's record lengths cycle (3, 2, 4
  units; 2, 4; 4, 3, 2 with one unit a sector) so every shorter-over-longer
  and longer-over-shorter tear is in it.
- **Reader mutant** (`nocheck`: a record whose head is there taken with its
  body and tag unchecked): 44,153 WRONG at length 4, first `w1` (a write
  failed after its first unit).

## 3. The chain: across, per slot, or absent

| choice | what it is | crash model | against a put-back |
|---|---|---|---|
| per slot | the record carries the tag of the record it overwrites | nothing to check: the predecessor is the bytes this write destroys, gone whether the write lands or not | nothing |
| across | the record carries the tag of the record the engine knows durable in the other slot; the open refuses a valid older other slot that is not it | 0 violations at length 4 and 5 (`chain=cross`); the mutant that chains to the previous publish wherever it went (`chain=last`) refuses legal states: 12,506 REFUSED at length 4, first `U W` | refuses some mixed pairs: MIXED 83,603 against 87,083 at length 5; ROLLBACK 70,489 against 70,787; every state it lets through is a published state, as without it |
| absent | | 0 | ROLLBACK and MIXED as above; every accepted state is published |

Recommendation: absent. The cross chain costs 16 bytes a slot and one
refusal path, and it needs the door to know which record the engine counts
durable in the other file -- the engine's slot rule inside the door, which
is the coupling decision 2 exists to remove -- for a refusal of mixed pairs
whose answers are published states anyway. It also does not catch H2: the
resurrected record names as its predecessor the record the open took, which
is still there (reasoning from the trace; a `chain=cross` run with the
engine's open rule was not made).

## 4. The adversary with the directory and not the key

What it can do (the adversary model enumerates every record a crash-free
history wrote, placed into either file, or nothing, or garbage; length 5,
283,854 directories over 9,331 histories):

- **Put back an older valid record, or an older pair.** The open answers a
  published state older than the newest acknowledged durable (ROLLBACK
  70,787). Nothing in the slots can say a newer one existed; decision 1 and
  CLAUDE.md ("what is not authenticated is the store taken back to an
  earlier state of itself") claim no more. Rolling back the slots alone
  usually leaves a manifest naming segments a later merge removed, which
  today's open refuses by name (a missing file); that is a refusal by
  accident, not a check.
- **Combine records the device never held together** (MIXED 87,083). Every
  answer is still a published state, the newest of the two it can read.
- **Swap the two files, or copy one record into both.** With the slot bound:
  a record in the file it was not sealed for is DAMAGED; a swap leaves both
  damaged, and the open refuses (as it would if the adversary deleted
  them). Without it: the open takes the newer, from the wrong file
  (A_RELOC).
- **Scribble or truncate.** A slot with bytes that do not authenticate is
  DAMAGED: the other slot is the store, and two such slots refuse the open.
  Deleting both slot files reads as a store with no manifest; under a
  cipher today's open refuses that where envelopes exist (CLAUDE.md, the
  crash discipline), which the door keeps.

What the record's binding prevents: a state no publish wrote (A_WRONG 0 in
every run), a record accepted in the slot it was not sealed for (A_RELOC 0
with the slot bound), a damaged pair read as no manifest (A_EMPTY 0 with
torn read as damage). It does not prevent rollback, nor a mix of published
records; and against a store copied directory and all, nothing tells the
copies apart.

## 5. The open's rule, and what the model says of it

1. Open both slot files through the door (the logical names `manifest`,
   `manifest.alt`).
2. **fdatasync both** (H2). Today: the slot taken only.
3. **Read them from the device** (H1): drop their clean pages
   (`POSIX_FADV_DONTNEED` after the sync; a slot file is never mapped) or
   read with `O_DIRECT`. Today: through the page cache.
4. Classify each: EMPTY, VALID, DAMAGED.
5. Take the newest VALID by the sequence in its plaintext (then the pending
   rule as today). No VALID and one DAMAGED: refuse, leaving every file as
   found. Both EMPTY: the no-manifest rule as today.
6. `SlotChoice.landed(taken)`; only now sweep; the first publish writes the
   other slot.

Steps 2 and 3 are one or two fdatasyncs of nothing (CLAUDE.md: 0.03 ms each)
and a read of a page or two from the device, at open only, in both modes.
By decision 10 they are priced before they land; they belong with the plain
store's durability fixes (S0), not with the seam, and must be checked
against the open's later unit that is not in this tree.

Model (`slots.py`; ops W U O B E F w V s K X P; each run's command in
`notes.md`):

| protocol | length 4 | length 5 |
|---|---|---|
| the engine's rule (sync the taken slot, read the cache) | 10,884 violations: CLAIM 777, LOSS 1,704, LOSS_BRANCH 7,604, NO_SLOT 799 | not finished |
| the same, no write or sync errors (alphabet without V, s) | 3,240 LOSS_BRANCH (H2) | not finished |
| sync both, read the cache | 2,954 (H1 only: CLAIM, LOSS, NO_SLOT) | not finished |
| sync both, no errors | 0 | not finished |
| sync both and read the device | **0** (21,470 sequences, 63,772 kill states, 112,230 power-loss states) | **0** (246,974 sequences, 990,040 kill, 1,785,777 power-loss); also 0 with lengths 2,4; with one unit a sector; with two power losses; with the cross chain |

Properties, and the mutant that breaks each on the fixed protocol (length 4):

| property | mutant | caught |
|---|---|---|
| the open never takes a state not published (WRONG) | `nocheck` | 44,153 WRONG |
| at least as new as the newest acknowledged durable (LOSS, LOSS_BRANCH) | `land_issue` (durable where the sync is issued) | 36,182 LOSS + CLAIM + NO_SLOT |
| no state with both slots invalid after one was durable (NO_SLOT) | `parity` (slot by the sequence's parity) | 27,843 NO_SLOT, 1,147 LOSS; first `U U` |
| a slot named durable is on the device (CLAIM) | `keep_owed` (owed entry not given up before a write into its slot) | 498 CLAIM, 445 LOSS, 112 NO_SLOT; first `U w1 O` |
| the open's sync | `open_sync=none` | 2,956 LOSS, 7,634 LOSS_BRANCH, 1,462 NO_SLOT, 1,260 CLAIM; first `U X` |
| AAD binds the slot | `noidx` | crash model 0 (cannot see it); adversary 123,152 A_RELOC |
| nonces | `count_reuse`, `prefix_fixed` | 9,319 and 17,105 NONCE |
| torn is damage | `torn_empty` | crash model 0; adversary 66,932 A_EMPTY |
| the cross chain names the durable record | `chain=last` | 12,506 REFUSED |

## 6. Replace for S2

The sealed-record slot makes the paged Replace class unnecessary: its users
are the two slot files and `manifest.prev-*`, the rename shape's spares,
and that arm is refused under a cipher by design.md:94. S2's `pagefile.rs`
then has Once, Log, Regen and Regen-lazy, and the door has `replace` beside
`append` for records.

Lost if it goes:
- E4's F6 Pg-specific work (sessions created at the slot's first write, the
  session drop rule): moot, since a record has no sessions.
- A paged read of a large manifest: a record is read and authenticated
  whole, so the manifest is held in memory whole at the open (it is today:
  `read_to_end`, db.rs:12717).
- A home for any future file rewritten whole in place that is too large to
  hold in memory, or that something wants to map. None exists in the tree
  (design.md's class table); one would bring Replace back with its model.
- The single crash model for slots and pages together: the slots' model is
  now this one, which has no pages, tag tier or pager.

What is not lost: torn reads as damage, the newest valid by sequence, the
slot rule, and the plain-store parity of section 5 (a plain store has no
door and keeps its CRC; the door's record wraps the same bytes).

## 7. The device probe owed (specified, not run)

Not run now: another job is using the machine and a probe is a timing run.

- **Question.** Does a slot rewritten as a sealed record through the door
  cost what today's engine-sealed slot costs, at every slot write the
  engine makes, and what do the open's added steps (H1, H2) cost?
- **Shapes**, one process, paired per rep, rotation varied per sitting:
  - `T`: today: `seal_manifest` bytes, `pwrite` at 0, `fdatasync`;
  - `D`: the door's record (head, ciphertext, tag), `pwrite` at 0,
    `fdatasync` -- the same syscalls, so the question is the bytes and the
    CPU (the AAD, the head);
  - `P`: the plain slot (`manifest_encode`, CRC), for the plain store's cost;
  - `O_T`, `O_D`: the open's read of both slots: today (open, read through
    the cache, fdatasync the taken slot) against the fixed rule (open both,
    fdatasync both, `FADV_DONTNEED` both, read both), and the same with
    `O_DIRECT` reads, from a warm cache and a cold one (`echo 1 >
    drop_caches` is not available in the guest without root; use a fresh
    file and a write-back first).
- **Sizes.** Manifests of 2, 50, 400 and 4,000 segments (today's slot holds
  each segment's name, flag, token and wrapped key: about 30 + 16 + 60
  bytes a name; the sizes cross one and several pages), slots that shrink
  and that grow between writes.
- **Patterns.** The two slots alternating with a sync each (`W W W`), a
  drain's owed slot written over before its sync (`U W`), and the pending
  slot's sync on a helper thread.
- **Reps.** 30 pairs a cell in three sittings, as `fix/probe` ran; ratios
  as median [min-max] k/30. Read `/sys/block/vda/stat` beside each cell
  (a slot write is one request a slot; anything more is the extent tree).
- **Kill rules, fixed before the run.** `D/T` over 1.05 in any sitting at
  any size kills `D` as specified (look at the AAD's length first); `O`'s
  added cost over 0.5 ms at the open goes to the owner under decision 10.
- **Fault half** (no timing; owed to S0's tests, not to the probe): a slot
  torn by `Db::tear_next_slot_write` after an owed publish, a kill, a
  reopen, and a power cut emulated by putting back the device image the
  test kept (H2); a failed slot sync (`Db::fail_next_slot_sync`) followed
  by a reopen in the same process (H1: today's engine reads the cache).

## 8. What this does to the stages

- **S0** gains H1 and H2: the open syncs both slot files and reads them past
  the cache. Both are plain-store fixes found by model, and both are
  checked against the open's later unit (not in this tree) before they are
  built; each starts from a failing test (section 7, fault half).
- **S2** drops Replace from `pagefile.rs`.
- **S6** adds `replace(bytes)` to the door as specified in section 2, the
  open's classification of a slot (empty, valid, damaged), and the tests:
  the record moved between files refused, two damaged slots refused and not
  read as no manifest, a torn slot over a longer older image, the nonce
  never repeated across a failed write and a reopen.
