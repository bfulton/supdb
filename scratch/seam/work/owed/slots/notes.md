# Notes: the manifest slots as sealed records (owed item "slots")

Directory: `scratchpad/seam/work/owed/slots/`. Nothing under `/home/user/supdb`
was touched, no cargo, no compiler, no timing run. Every model run was one
Python process under `nice -n 10` and `timeout 600`, one at a time.

## What I read

- `decisions.md` whole (decision 2's "owed before it is built").
- `work/fix/conclusion.md` whole; `work/fix/model/README.txt`, `notes.md`,
  `stream.py` whole, `cand.py` (device, world, tally).
- `work/e4-model/notes.md` (F1, F6 and the caveats), `replace.py` and
  `cmp_replace.py` whole.
- `binding.md` §1-§7 (for what decision 1 claims about rollback and the
  damaged-slot cases).
- `design.md`: the class table (lines 85-99), §3 layout and records, §4
  write/sync/trust for Replace (196-262, 315-355).
- Tree `682b2c09`: vault.rs 306-331 (`Nonces`), 362-449 (the marker, its
  store id), 854-873, 1626-1681 (`seal_manifest`, `open_manifest`),
  1750-1758 (`SLOT_MAGIC`, `slot_aad`); db.rs 12296-12376 (the manifest's
  doc, `MANIFEST_SLOTS`, `SlotChoice`), 12430-12536 (`manifest_encode`,
  `manifest_image`, `manifest_open`, `manifest_write_slot(_unsynced)`),
  12700-12735 (`manifest_read_all`), 28700-28800 (`Db::open`'s slot read,
  the sync of the slot taken, the sweep after), 38625-38650 (`Maint.slot`,
  `slot_sync_due`), 39640-39710 (the pending slot and its helper),
  40775-40990 (`publish_unsynced`, `sync_owed_slot`, `write_slot`,
  `sync_slot`, `publish`).
- Not read: the unit of open fixes after `682b2c09`; the rename shape
  (`manifest_write`); the upgrade of v1/v2 slots; `pending_whole`.

## Design choices and reasons

Written out in `design.md`; in one line each:

1. A slot is one sealed record at offset 0, rewritten whole: the Stream
   record's format at Replace's position, a `replace(bytes)` door op. No
   header, sessions or pages: nothing in a slot needs them, and each is a
   second thing to tear.
2. The nonce is the door's: per-open random prefix, count advanced at the
   seal and never given back (today's `Nonces`). The mutants say why.
3. AAD = domain, store id, file id and logical name (the slot index), len.
   The sequence stays in the engine's plaintext under the tag, so the door
   knows nothing of sequences (decision 2's "not fragile").
4. No chain: per slot it is unverifiable, across slots it needs the
   engine's `SlotChoice` in the door and buys nothing measurable against a
   put-back (adversary runs), and its wrong form refuses legal states.
5. Torn reads as damage, never empty; a longer older tail is ignored by the
   head's length.
6. The open syncs both slots and reads them past the cache (H2, H1 below),
   then takes the newest valid by the plaintext's sequence.

## The model

`slots.py` (crash model) and `adversary.py` (directory adversary). The
copies of `stream.py`, `cand.py` and `replace.py` beside them are
unchanged references; `slots.py` is new code that follows their device
(sectors atomic, any subset of unsynced writes at a power loss, the page
cache kept at a kill) and adds what they did not have:

- **per-file fdatasync** (cand.py's sync is global): a completed sync of a
  file begun at tb fixes that file's sectors written at or before tb;
- **a failed fdatasync**: those writes leave the dirty list, the device may
  hold any of them or what it held, the page cache keeps the newest (Linux
  marks the pages clean after a failed writeback), and every sync of that
  file owed or in flight through a descriptor opened before reports the
  failure (errseq);
- **two slot files**, records of cycling lengths in units, SU units a
  sector, so a rewrite tears at every sector boundary and a kill or a
  write error leaves a unit prefix in the page cache;
- **the engine's slot protocol** (SlotChoice, the owed entry taken before
  the write, the pending helper's sync on another thread, completed oldest
  or newest first), and the open (read, choose, sync, landed).

Ops: W U O B E F w V s K X P (slots.py's docstring). Sequences up to a
length, every one enumerated; at a kill the run reopens from the page
cache, at a power loss from every device view, and goes on; every sequence
is judged at its end (cache and every device view), so with every prefix
enumerated every instant is covered.

Properties: WRONG (a record no publish wrote whole), LOSS (older than the
newest the engine counted durable), LOSS_BRANCH (a record an earlier open
passed over, which the store has since contradicted), NO_SLOT (no valid
slot: refused or no manifest), CLAIM (the engine counts a slot durable
whose record the device does not hold exactly), REFUSED (the chain refused
a legal state), NONCE.

Scale: 2 slot files of 3 sectors of 2 units (one run: 4 sectors of 1
unit); records of 2-4 units; at most one power loss (one run: two) and two
kills a sequence. Exhaustive, no sampling.

## Runs (cwd this directory; each `nice -n 10 timeout 600 python3 ...`)

### The engine's rule and its two holes

| command | sequences | kill / power states | result |
|---|---|---|---|
| `slots.py maxlen=4` | 21,470 | 61,908 / 152,577 | 10,884: LOSS 1,704, LOSS_BRANCH 7,604, NO_SLOT 799, CLAIM 777 |
| `slots.py maxlen=4 alpha=WUOBEFwKXP` (no failed sync) | 10,278 | 33,280 / 71,313 | 3,240 LOSS_BRANCH, first `U K1` |
| `slots.py maxlen=4 alpha=WUOBEFwKXP open_sync=both` | 10,278 | 31,857 / 49,192 | 0 |
| `slots.py maxlen=4 open_sync=both` | 21,470 | 59,868 / 112,683 | 2,954: LOSS 1,492, NO_SLOT 686, CLAIM 776; first `V X` |
| `slots.py maxlen=4 open_read=device` | 21,470 | 63,772 / 112,230 | **0** |
| `slots.py maxlen=5 open_read=device` | 246,974 | 990,040 / 1,785,777 | **0** (49.7 s) |

**H2, first counterexample `U K1` then the end check.** U: seq 1 into slot
1, its sync owed. K1: seq 2 into slot 1, the process killed after its
first unit: the page cache holds slot 1 torn. The reopen reads slot 1 torn
and slot 0 (seq 0) valid, takes seq 0, fdatasyncs slot 0 only. A power
loss now: slot 1's device sectors may be seq 1's whole (its writeback ran
before K1's write dirtied the sector again), and the open takes seq 1, a
state the previous open passed over and the store has since contradicted
(the first open's sweep may have removed what it names, and replayed the
log beyond it). Syncing both slots at the open writes the torn image back
and closes it: 0. E4's F6 fix (`open_sync` in replace.py) syncs both;
db.rs:28750-28770 syncs the taken slot. Plain-store hazard by the model;
not verified on the engine.

**H1, first counterexample `V X`.** V: seq 1 into slot 1, its fdatasync
fails (no retry); the device holds slot 1's old bytes or some of the new,
the page cache the new record, clean. X: the reopen reads the cache, takes
seq 1, its fdatasync of slot 1 has nothing dirty and returns Ok: CLAIM.
The first publish then writes slot 0, the only record the device held; a
power loss gives LOSS (seq 0 under an acknowledged seq 1) or NO_SLOT.
Reading the slots past the cache (fdatasync both, drop their clean pages,
read) closes it: 0. Plain-store hazard by the model; not verified on the
engine; it rests on Linux keeping a failed writeback's pages clean in the
cache and not reporting the error to a descriptor opened after it was
seen, which is the documented errseq behaviour and was not tested here.

### The fixed protocol, other shapes (length 5)

| command | sequences | kill / power states | result |
|---|---|---|---|
| `slots.py maxlen=5 open_read=device lens=2,4` | 246,974 | 1,517,189 / 2,743,516 | 0 |
| `slots.py maxlen=5 open_read=device lens=4,3,2 SU=1 NSEC=4` | 246,974 | 2,017,149 / 8,062,750 | 0 |
| `slots.py maxlen=5 open_read=device maxP=2` | 260,964 | 1,043,001 / 1,902,325 | 0 |
| `slots.py maxlen=5 open_read=device chain=cross` | 246,974 | 990,040 / 1,785,777 | 0 |

The same batch's four length-5 runs of the engine's rule (`""`,
`alpha=WUOBEFwKXP`, the same with `open_sync=both`, and `open_sync=both`)
were stopped unfinished when the step ended: the engine's rule and its
decomposition are claimed at length 4 only.

### Mutants on the fixed protocol (`open_read=device`), length 4

| mutant (option) | result | first |
|---|---|---|
| slot by the sequence's parity (`parity=1`) | 35,156: NO_SLOT 27,843 any + 6,067 proc, LOSS 1,147 + 99 | `U U` (NO_SLOT), `W U U` (LOSS) |
| durable where the sync is issued (`land_issue=1`) | 66,158: LOSS 36,182, CLAIM 16,494, NO_SLOT 13,090 | `U` (LOSS) |
| owed entry not given up before a write into its slot (`keep_owed=1`) | 1,286: CLAIM 498, LOSS 445 + 195, NO_SLOT 112 + 36 | `U w1 O` |
| the open not syncing (`open_sync=none`) | 13,324: LOSS_BRANCH 7,634, LOSS 2,956, NO_SLOT 1,462, CLAIM 1,260 | `U X` (LOSS) |
| AAD without the slot's index (`noidx=1`) | **0 in the crash model** (no crash moves a record between files); caught by the adversary model below | |
| reader without its tag check (`nocheck=1`) | 44,153 WRONG | `w1` |
| count given back after a failed write (`count_reuse=1`) | 9,319 NONCE (reuse) | `w1 W` |
| prefix per store, not per open (`prefix_fixed=1`) | 17,105 NONCE (reuse) | `K1 W` |
| torn read as empty (`torn_empty=1`) | 0 in the crash model (the durable slot always stands); caught by the adversary model | |
| chain to the previous publish (`chain=last`) | 12,506 REFUSED | `U W` |

Every required mutant is caught: parity, land at issue, owed not given up
and the open not syncing by the crash model; the AAD without the slot's
index by the adversary model only, and I do not claim the crash model
catches it. `open_sync=taken` with `open_read=device` is 0 because the
device read syncs both itself; the hole of syncing only the taken slot is
the `alpha=WUOBEFwKXP` pair of runs above.

### The chain (length 4, `open_read=device`)

`chain=cross`: 0. `chain=last`: 12,506 REFUSED (above). `chain=cross
noidx=1`: 0. `chain=cross keep_owed=1`: 1,342 (as keep_owed, plus 56
REFUSED).

The cross chain under the engine's open rule (does it catch H2?) was not
run; design.md §3's "it does not catch H2" is reasoning from the trace.

### The adversary (`adversary.py`, length 5: 9,331 histories, 283,854 directories)

| command | claimed violations | ROLLBACK | MIXED | REFUSED |
|---|---|---|---|---|
| `adversary.py maxlen=5` | 0 | 70,787 | 87,083 | 126,651 |
| `adversary.py maxlen=5 chain=cross` | 0 | 70,489 | 83,603 | 130,131 |
| `adversary.py maxlen=5 noidx=1` | **123,152 A_RELOC** | 98,844 | 197,969 | 37,324 |
| `adversary.py maxlen=5 chain=cross noidx=1` | 101,597 A_RELOC | 91,362 | 154,859 | 80,434 |
| `adversary.py maxlen=5 torn_empty=1` | **66,932 A_EMPTY** | 70,787 | 87,083 | 59,719 |

First A_RELOC: no ops, file 1 holds the birth record sealed for slot 0,
file 0 empty: accepted. First A_EMPTY: no ops, both files garbage: read as
a store with no manifest. ROLLBACK first: `W`, file 0 = the birth record,
file 1 empty. A_WRONG is 0 in every run (the adversary only places whole
records, so `nocheck` is a crash-model mutant: 0 here).

What the adversary model does not have: records from another store
(another key: the tag fails, by construction of the model's key check, not
exercised), a directory copied whole (indistinguishable by design), the
names layer (it writes files by logical slot), records torn by hand (it
writes whole records or garbage).

Results files: `results/mutants_len4.txt`, `results/chain_len4.txt`,
`results/adversary_len5.txt`, `results/len5.txt`,
`results/fix_device_len5.txt`.

## Not verified

- **Nothing on the engine.** H1 and H2 are model findings about the plain
  store's open as much as a cipher store's; neither was built as a test,
  and the unit of open fixes after `682b2c09` was not read: it may change
  either.
- H1 rests on Linux's behaviour after a failed writeback (pages clean and
  kept, the error not reported to a descriptor opened after it was seen);
  not tested here, and not checked per filesystem.
- That `POSIX_FADV_DONTNEED` after an fdatasync drops a slot file's clean
  pages (it skips mapped, locked or dirty pages; slot files are read with
  `read`, db.rs:12717), or that an `O_DIRECT` read is available under the
  door. The fix is modelled as "the read sees the device".
- No cost: the record's bytes and CPU against today's, and the open's two
  syncs and device reads, are specified in design.md §7 and not run.
- Not modelled: `manifest_pending`'s skip of a newest slot whose pending
  segments do not read back (the open then takes an older slot); the
  rename shape and its spares; the upgrade of version 1 and 2 slots; the
  open's own fdatasync failing; a write that fails after its bytes landed;
  the segments, the log and the sweep behind a slot (a state is a record,
  not a store); more than two kills or power losses a sequence; records
  longer than 4 units or files of more than 3 sectors.
- The engine continues after every write or sync error in the model (a
  superset of what it does); no violation of the fixed protocol depended on
  that, but nothing either depended on it being the engine's behaviour.
- The cross chain against H2 under the engine's open rule: not run.
- The engine's rule at length 5: stopped unfinished; length 4 only.
- design.md's claims about Replace's users are from design.md's table
  (lines 85-94), not from a fresh search of the tree.
