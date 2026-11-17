# Skeptic's check: item "slots" (the manifest slots as sealed records)

Read: the item's notes.md, design.md, slots.py and adversary.py whole;
decisions.md decisions 1-10; the seam design.md class table (80-100) and
key rule (6, 134-157); fix/design-free/design.md §4 (the Stream class);
e4-model/replace.py 170-190 and notes.md F6; tree 682b2c09 db.rs
12333, 12539-12545, 12575-12610, 12709-12735, 28730-28812, 40955-40985,
vault.rs 1620-1690, 1745-1762. No cargo, no timing. Every run below was one
`nice -n 10 timeout 600 python3` process from `check/`, which holds my copy
of the models (slots.py with six options added, all default-off) and
`reloc_outcomes.py`.

## Reran (all reproduce unless noted)

| command (in check/) | result |
|---|---|
| `slots.py maxlen=4` | 10,884 (LOSS 1,704, LOSS_BRANCH 7,604, NO_SLOT 799, CLAIM 777), first `V X`, `U K1` -- as claimed |
| `slots.py maxlen=4 open_read=device` | 0 |
| `slots.py maxlen=5 open_read=device` | 0 of 246,974; 990,040 / 1,785,777 states -- as claimed |
| `slots.py maxlen=4 alpha=WUOBEFwKXP` | **3,256** LOSS_BRANCH, states 33,294 / 71,813; notes say 3,240 and 33,280 / 71,313 |
| `slots.py maxlen=4 alpha=WUOBEFwKXP open_sync=both` | 0, states **31,871 / 49,645**; notes say 31,857 / 49,192 |
| the ten mutants of notes.md at length 4 with `open_read=device` | every count matches results/mutants_len4.txt |
| `adversary.py maxlen=5`, `noidx=1`, `torn_empty=1` | 0 / 123,152 A_RELOC / 66,932 A_EMPTY -- as claimed |

The two alphabet-restricted counts do not reproduce from the model on disk
(the zero and the first counterexample do); they were probably taken from an
earlier slots.py. Small, but they are cited as figures.

## Mutants of my own (length 4, `open_read=device`)

| option (check/slots.py) | what it breaks | result |
|---|---|---|
| `take_old=1` | the open takes the oldest valid slot | 13,433 LOSS, first `W` |
| `keep_inflight=1` | a write into a slot un-holds the owed entry but not a sync in flight on the helper | 935: CLAIM 336, LOSS 487, NO_SLOT 112; first `B U E` |
| `land_unheld=1` | a completed in-flight sync lands whether or not its entry was given up | 935, same shape |
| `nolandcheck=1 keep_owed=1` | the CLAIM check off: do other checks still see keep_owed? | 788 LOSS/NO_SLOT, first `U w1 O` -- yes |
| `nolandcheck=1 land_issue=1` | same for land_issue | 49,664 LOSS/NO_SLOT -- yes |
| `ignore_fail=1` | errseq not propagated to entries opened before a failure (`Eng.failed` a no-op) | **0 change anywhere**: engine rule 10,884, sync-both 2,954, fixed 0 |
| `w_whole=1` (model extension) | a write that reports failure after every byte reached the cache | **16,774 WRONG** on the fixed protocol, first `w w2` |
| `chain=cross` under the engine's open rule, no errors | does the cross chain catch H2? (design §3 said no, by reasoning) | 3,256 LOSS_BRANCH, the same as without it: confirmed, now by run |

## What is wrong

1. **The `noidx` mutant is caught only by a property that restates the
   binding.** A_RELOC is "the open took a record sealed for the other
   slot", i.e. the binding itself. `check/reloc_outcomes.py 5` shows that in
   all 9,331 histories the set of records the open can be made to take is
   identical with and without the slot index in the AAD; the binding only
   turns some benign swaps and copies into refusals. The design says "buys
   little", which is honest; the properties list's "a record is accepted
   only in the slot file it was sealed for ... mutant caught: true" is a
   tautology, not evidence. Relocation is harmless because the engine takes
   a slot's index from the file it read (`Manifest { slot: i, .. }`,
   db.rs:12723), never from the plaintext; that, not the AAD, is the
   property to keep, and nothing in the design states it.
2. **The key contradicts the seam's key rule.** design §2: "key = K_s (or a
   key derived from it ...)" with a nonce prefix drawn per open of the
   store. design.md:6 and :157 say K_s seals nothing under a random nonce
   (it only derives K_f per file_id); conclusion.md does not amend that, and
   the Stream class the item says it adopts uses K_rec derived from K_f with
   a prefix per Handle (design-free §4). The record must be under a
   K_f-derived key; then a record moved between slot files fails by key, and
   the slot index in the AAD is redundant. The model has one key `Km`, so it
   cannot distinguish the choices.
3. **The model's errseq is dead.** notes.md lists "every sync of that file
   owed or in flight through a descriptor opened before reports the failure"
   as something the model adds. `ignore_fail=1` changes no verdict in any
   protocol: a held entry never shares a slot with another failing sync in
   the engine's ops (a write into a slot gives its entries up first). H1 is
   modelled only by the reopen's sync returning Ok. Not a protocol fault;
   the model's claim is larger than what it exercises.
4. **Stale figures** for the H2-only runs (above).
5. **adversary.py mislabels**: (EMPTY, EMPTY) is the no-manifest path, not a
   refusal, and is counted REFUSED; under `torn_empty` (EMPTY, GARB) also
   reads as no manifest and is counted REFUSED, not A_EMPTY. The mutant is
   still caught; the REFUSED column overstates refusals.

## What is missed

1. **H1 is not a slot hazard; it is every file an open reads back and then
   trusts after one fdatasync.** By the same device model (pages of a failed
   writeback clean in the cache; a later fdatasync returns Ok over them):
   a reopen after a failed log fdatasync replays frames the device may not
   hold, the first sync by policy counts them synced, and the commits
   acknowledged after the reopen are appended behind a hole that a power loss
   turns into the log's end; the same holds for a run temp recovered and for
   pending segments read back under `manifest_pending` (CLAUDE.md: "a
   segment read back after a kill was read out of the page cache"). Reasoning
   only, not modelled. Fixing H1 for the slots alone, as S0 proposes, leaves
   these, and reading the whole log past the cache is not "a page or two".
2. **The open's fdatasync of the slot it does not take can now fail** (an
   unseen writeback error of a torn image, reported to the new descriptor),
   and §5 does not say whether that refuses the open. Not modelled.
3. **A replace that fails after landing.** `w_whole=1`: 16,774 WRONG on the
   fixed protocol. Buffered `write_all_at` cannot report an error after its
   last byte lands, so today it is unreachable; the door's `replace` must
   promise the same (no step after the write that can fail), or the engine
   must treat a failed publish as possibly landed. Whether such a record is
   harmful in the engine (does a failed landing remove a file it names) was
   not checked.
4. **Decision 2 says "chained"; the design drops the chain.** That amends a
   decision and belongs in decisions.md, not only in the item's design.
5. **EMPTY widened.** Today only a zero-length slot is skipped
   (db.rs:12719-12721); the design reads "every byte zero" as EMPTY too. No
   crash state reaches it (slot 0 is durable from birth), but it moves two
   zero-filled files from a refusal to the no-manifest path; unstated.

## Stands

- H2 is in the export: db.rs:28797-28812 fdatasyncs only the taken slot;
  E4's `open_sync` synced both (replace.py:178-183). The model's
  counterexample `U K1` + power loss is a correct reading of the device
  model, and syncing both closes it (0).
- The cross chain neither catches H2 nor changes the records an adversary
  can make the open take; dropping it loses refusals of mixed pairs only.
- Replace's users: `manifest.prev-*` is made only by `manifest_write`, the
  rename shape (db.rs:12580-12610), reached only in the non-slot branch of
  publish (db.rs:40966); the slots are its only other users. Dropping
  Replace from S2 is supported.
- All five required mutants make the models report: parity, land_issue,
  keep_owed, open_sync=none in the crash model; noidx in the adversary
  model only, under the tautological property (wrong, 1).

## Hand state tried

`B s W` then a power loss (a sync in flight fails after a later synced
write to the same slot): `sync_end` of the failed token finds no pending
write at or before its begin (the later sync cleared them), so the device
keeps the later record; no violation, and none by the model. And `V U O X`
(a failed sync, then an owed publish into the same slot synced): the new
descriptor's fdatasync covers the new dirty sectors; no violation.

## Not checked

The later unit of open fixes (not in the export or in /home/user/supdb,
which is at 682b2c09); Linux's failed-writeback behaviour per filesystem;
any engine test; the engine rule at length 5; `manifest_pending`'s skip;
the open's own sync failing; costs; the names layer's spelling of the slot
files and where their file_id comes from before the manifest is read.
