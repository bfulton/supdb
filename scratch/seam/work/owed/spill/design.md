# The long-name spill (R10, G-names): design

Owed item "spill". It amends `design.md` §3 (Names), §4 (Rename, link,
remove; Open) and the layout table, and fills `forms.md` R10. The crash
ordering is model-checked in `spill.py` beside this file; `notes.md` has
every run.

## 1. Which names can spill

The sealed entry (design.md §3) is

    'x' + b64url( fid16 ‖ nonce12 ‖ AEAD_Kf( pad16(field) ‖ ns u64 ‖ flags u8 ‖ prev_lo u64 ) ‖ tag16 )

so its decoded length is 16 + 12 + pad16(F) + 17 + 16 = 61 + pad16(F) bytes,
and its spelling is 1 + ceil(4 (61 + pad16(F)) / 3) characters. It fits
NAME_MAX = 255 iff 4 (61 + pad16(F)) ≤ 762, i.e. pad16(F) ≤ 129, i.e.
**pad16(F) ≤ 128: a logical name of at most 128 bytes is inline**
(128 → 189 bytes → 252 + 1 = 253 characters; 129..144 → 205 bytes → 275).

The name constructors (tree 682b2c09, u64 fields at their full 20 digits
since `{:08}`/`{:016}` are minimum widths; a fence is at most `FENCE_MAX`
bytes, db.rs:5032, two hex characters a byte, db.rs:5064):

| constructor | site | longest, FENCE_MAX = F | F = 16 | F = 32 |
|---|---|---|---|---|
| `par-{id}-{seq}-{lo}-{hi}.sup` (merge output, promotion) | db.rs:13530, 40737 | 51 + 4F | **115** | **179 spills** |
| `pcs-{id}-{seq}-{lo}-.sup` | db.rs:28886 | 51 + 2F | 83 | 115 |
| `ord-{id}-{seq}.oidx.tmp` | db.rs:127, 28315 | 54 | 54 | 54 |
| `zpg-{id}-{seq}.zpg.tmp` | zfile.rs:89, 6136 | 53 | 53 | 53 |
| `seg-{n}-{seq}.sup`, `par-{n}-{seq}--.sup` | db.rs:28246, 28252 | 50 / 51 | | |
| `zpg-dead-{n}.zpg`, `compact-`/`tier-`/`seal-{id}.tmp`, `direct-{id}.tmp` | zfile.rs:3727; db.rs:13534, 13802, 28889, 30001 | 37, 31-33 | | |
| `wal-{id}`, `spare-{id}`, `manifest.prev-{n}`, `manifest(.alt)` | db.rs:27951, 27955, 12598, 12333 | 24-34 | | |
| any of the above + `.pfcopy` (layer-internal, design §4) | | +7 | 122 | |

So **nothing spills today** (the margin is 128 − 115 = 13 bytes), and at
FENCE_MAX doubled **only the merge-output partition name spills** (179);
`pcs-` reaches 115 and stays inline. Spill thresholds: `par-` at F ≥ 20,
`pcs-` at F ≥ 39. With the widths a store actually reaches (id under 10^8,
seq under 10^16: 8 and 16 digits) `par-` is 35 + 2|lo| + 2|hi|, so at F = 32
a partition spills only when its fences are longer than 23 bytes on
average, i.e. in stores whose keys are that long; the suite's 16-byte keys
never spill. A store "whose every name spills" therefore cannot be built
from today's constructors at any FENCE_MAX: the worst is every partition.

## 2. The directory entry

Unchanged envelope, nonce, key and AAD. One bit of the sealed `flags` byte
is `SPILL`. For a spilled entry the field is

    slot u8 ‖ rtag[16] ‖ name_len u16      (19 bytes, padded to 32)

so a spilled spelling is 1 + ceil(4·93/3) = 125 characters. The flag is
inside the AEAD, so it cannot be flipped without failing the entry's tag.
`name_len` lets a reader size the read and refuse a record of another
length before decrypting. `rtag` is the name record's AEAD tag (§3): the
binding.

## 3. Where the spilled name lives

Not in header A/B. A header is rewritten for every new session and its
overwrite is already the hazard of E4 F1 (a resumed Handle overwriting the
only durable header); a rename would add a header rewrite on a path that
has none, and a hard link needs two names live at once. The name lives in
**name pages**: four slots of one stored page each in the file's sealed
prefix, at stored offsets 8, 12, 16 and 20 KiB. The journal ring / `REC`
region starts at 24 KiB instead of 8 KiB (a layout constant; the ring
loses 16 of 1016 KiB). A file whose names never spill never writes them:
they stay holes, and it pays nothing.

A name record is one 4096-byte stored page, written whole by one `pwrite`:

    nonce12 ‖ AEAD_Kf( ns u64 ‖ len u16 ‖ name ‖ zeroes to the page ) ‖ tag16
    AAD = "pfname" ‖ store_id ‖ file_id ‖ slot u8 ‖ ns u64

Key K_f (the file key of design.md §3, derived from K_s and the file_id);
the nonce is random (96 bits), one per spilled create, rename or link,
which adds to the "tens a file" of random nonces under K_f that §3 already
counts (headers, wraps, sealed names). Longest spilled name: 4096 − 12 −
16 − 10 = 4058 bytes. A torn page fails its tag.

**Resolution** of a spilled entry: read slot `slot`, open it under K_f with
the AAD built from the *entry's* fid, slot and ns, and require its tag to
equal the entry's `rtag`. Only then is the record's name the entry's name.
The tag comparison is load-bearing (§6, M3b): the ns alone does not bind,
because the open restarts ns at the largest *listed* ns plus one
(design.md §4) and a record whose entry a power cut lost can carry an ns
the reopened store hands out again.

**Slot allocation.** The lowest slot that no entry of the file in the
layer's current (page-cache) directory references, unresolved entries
included. No directory sync is needed to reuse a slot: a ghost (an entry
the device still holds and the page cache no longer does) that references
a reused slot fails its `rtag` and is unresolved, and the engine never
needs a ghost (it unlinks a name only after a durable manifest without it,
and renames only from temps). The engine holds at most two spilled
spellings of one file at once (a promotion's link beside the old name; a
spilled temp beside its final name), so four slots leave two spare. A
fifth live spilled spelling is refused with an error and a debug assertion
(the model never reaches it at NS = 4 or NS = 2).

## 4. The verbs

- **create(name)**: if the name spills, write the record, then create the
  spelling. Record first, so no page-cache directory ever shows an entry
  without its record (the model shows the other order is also sound under
  the rule below; record-first is chosen so a mere process crash leaves no
  unresolved entry).
- **rename(a, b)**: b's record into a free slot (a's slot is still
  referenced), then `rename(2)` from a's spelling to b's. a's slot is free
  once a's spelling is gone from the layer's table. Contract: until the
  next `sync_dir` returns, a power cut may leave b's entry without b's
  record, so the file may have neither name. `rename(2)` itself promises a
  or b; the layer promises that only once b's record is durable. The
  engine renames only from names no manifest holds (temps: `compact-`,
  `tier-`, `seal-`, `.oidx.tmp`, `.zpg.tmp`, spare→wal), whose window is
  swept either way.
- **hard_link(a, b)**: a second record, in its own slot, and `link(2)`. A
  link's name is a second record, not a second header; both records stay
  while both spellings do. One Handle per file_id, as design.md §4.
- **remove_file(name)**: `unlink(2)` of the spelling; the slot is free.
- **rename-over** stays the verb of design.md §4 (rename to a new spelling
  with a higher ns, `sync_dir`, unlink the old spelling); a spilled target
  adds only its record, written first.
- **sync_dir()**: first, for every Handle with a name record written since
  its last fdatasync *began*, wait for a sync in flight that began after
  the record or issue one; then fsync the directory. The rule is
  load-bearing (§6, M2). It is placed here because the engine names a file
  in a manifest only after a `sync_dir` that covers its name (merge outputs
  db.rs:13686, tier 13854, promotion 40764, first-partition seal 28903, a
  seal's readable landing 39732), and the promotion links a file it never
  fsyncs, so nothing else makes the record durable before the manifest
  that relies on it. With no spilled name pending the rule is a test of an
  empty set: no syscall.

`names()` returns logical names. No engine code reads a flag, a slot or a
record: the spill is wholly below the door (R0).

## 5. The open, and what it costs

1. Each entry is decrypted from its spelling alone, as today. An inline
   entry costs no read.
2. A `SPILL` entry costs one 4 KiB read of its slot page and one AEAD open,
   read with `pread` into a buffer and not through the pager (it is not
   plaintext the store reads, so it is not a `fill`). For a file the open
   opens anyway (a live segment), the read is 0..24 KiB in one syscall
   with the headers, kept for the Handle's first open: no extra device
   round trip. Only entries whose flag says `SPILL` read; inline entries,
   and every name today, pay nothing.
3. A store at FENCE_MAX doubled with long keys pays one page read and one
   decrypt per partition entry at open, and nothing for its `pcs-`,
   `seg-`, `ord-`, `zpg-` and log entries. `tests/cipher.rs`'s bound
   (`c.fills <= 6 * files`, line 486) is unchanged; a counter
   `CipherCounts::name_reads` is added and the test asserts it equals the
   spilled entries listed (zero today).
4. An entry that authenticates and whose record does not resolve is
   **withheld** from the name table. Then:
   - if the manifest the open takes names a file with no resolving entry,
     the engine's existing refusal for a missing name fires, with every
     file left as found (CLAUDE.md, `Db::open_refusals`);
   - otherwise the entry is the crash window's: it is handed to the
     engine as a temp logical name `spill-<fid hex>-<ns>.tmp`, which the
     open's `.tmp` sweep (design.md §4, "Temps a crash leaks") removes after
     its slot is checked and synced. If E0 item 4 keeps `.tmp` names, the
     layer leaves and counts them instead: a leak, never a loss.
   Why sweeping it is safe: the entry authenticates under K_f, so this
   store wrote it; its record not resolving means no `sync_dir` returned
   after it was made (the rule syncs the record first), and the engine
   names nothing in a manifest before such a `sync_dir`, so no manifest on
   the device can need it -- the model holds exactly that (UNNAMED never
   fires under the design). Removing it unlinks one spelling; another
   spelling of the same inode keeps the file. An entry that fails its own
   tag is still left in place, as design.md §4 says.
5. Duplicates for one logical name: the higher ns is kept and the other
   spelling unlinked, as design.md §4 says.

## 6. What the model holds (details and commands in notes.md)

`spill.py`, the E4 device (`pfm.Dev`, copied unchanged), the engine's
create/rename/link/unlink paths (C, T, D, P, R, U), each also cut at its
first barrier with the process dying there, process crashes and power
cuts with the store reopened and run on. Properties in every power-loss
and page-cache state: MISNAMED (no entry resolves to a name it was not
made with), AMBIG (no two entries resolve to one name undecidably, and the
one kept is the newest), UNNAMED (every name the manifest on the device
holds resolves).

| run | result |
|---|---|
| design, n = 5, all 14 ops, subset | 0 violations of 15,343,583 power-loss and 2,699,250 page-cache states |
| design, n = 5, prefix (ext4) | 0 of 6,557,058 + 2,461,688 |
| design, n = 5, NS = 2; and with entry first | 0 of 15,343,583 + 2,699,250 each; the slot table never full |
| M1 flag ignored | caught: MISNAMED from the first op (`C`) |
| M2 no rule for the gap (sync_dir without the records) | caught: UNNAMED, first `P` (promotion) |
| M2 with entry first | caught: UNNAMED, `P` |
| M3 name not bound (whatever record is in the slot) | caught: MISNAMED, `TP` (the link's entry lands before its record, and the slot still holds the spilled temp's record, so the link reads as the temp's name) |
| M3b bound by (ns, slot) only, no rtag | caught: MISNAMED, `pLP` (a record whose link the cut lost, ns handed out again) |
| M4 slot 0 always | caught: UNNAMED, `CP` |
| M5 ns restarts at 1 | caught: AMBIG (an older entry kept), `CXR` |

## 7. Tests owed with it (S2)

- G-names: every constructor at its longest inputs (u64 fields at 20
  digits, fences at FENCE_MAX) either has a logical name ≤ 128 bytes or is
  flagged spilled; and a test that drives FENCE_MAX-sized fences doubled
  through a synthetic constructor.
- A store with a synthetic long-named file through create, rename, link,
  remove, rename-over and reopen, read back by name.
- The M2 state built through a seam (the record's page lost under a
  durable link) and the open's answer: refusal when the manifest needs the
  name, sweep otherwise.
- `name_reads` in `tests/cipher.rs`.
- Mutants: the six above, each held by a test, not only by the model.
