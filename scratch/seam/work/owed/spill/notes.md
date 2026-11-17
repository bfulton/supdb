# Notes: the long-name spill and its crash ordering (owed item "spill")

Directory: `scratchpad/seam/work/owed/spill/`. Nothing under
`/home/user/supdb` touched; nothing else under `scratchpad/seam` written.
No cargo, no compiler, no timing. Every model run was one Python process
under `nice -n 10` and `timeout 600`, one at a time.

Files: `design.md` (the design), `spill.py` (the model), `pfm.py` (copied
unchanged from `work/e4-model/` for its `Dev` and `Tally`; `cmp` says
identical), `results/` (each run's output).

## What I read

- `decisions.md` whole; `forms.md` R10, G-names, A3, A8, the S2 line and the
  not-verified list (grep, lines 22-58, 115-169, 217, 264).
- `design.md` §1-§4 (lines 1-341: the door, classes, layout, the sealed name
  and its arithmetic, keys and nonces, rename/link/remove, the open, the
  temps' sweep) and §12.
- `work/fix/conclusion.md` (grep for names: the WAL's name hidden under a′).
- `work/e4-model/notes.md` whole; `dirmodels.py` whole; `pfm.py` lines
  1-262 (device, world) and 945-983 (Tally).
- `work/owed/slots/notes.md` head (how a sibling item was laid out).
- Tree 682b2c09: db.rs 5025-5070 (`FENCE_MAX`, `fence_lo`, `hex`),
  12594-12602 (`manifest.prev-`), 13520-13540 (merge output `par-`),
  28240-28320 (`seg_name`, `first_partition_name`, `name_field`,
  `ord_name`), 28875-28905 (`pcs-`, `seal-` and its `sync_dir`),
  40725-40770 (promotion's link and `sync_dir`), and the `sync_dir` sites
  (13686, 13854, 28104, 28384, 28903, 39732, 40764) with six lines before
  each; zfile.rs 80-92 (`zpg_name`), 6130-6140 (`.zpg.tmp`), grep of every
  `format!` that builds a file name in db.rs and db/*.rs.
  `tests/cipher.rs` 460-495 and 1700-1720 (the open's fills bounds).
- Not read: the later unit of open fixes; `vault.rs` beyond the grep hits;
  the engine's open sweep code itself (db.rs:27632 as cited by design.md).

## Design choices, each with its reason

1. **Inline budget 128 bytes**, computed: decoded entry 61 + pad16(F) bytes,
   fits NAME_MAX iff pad16(F) ≤ 128 (design.md §1 has the arithmetic).
   Today's longest is `par-` at 115; at FENCE_MAX = 32 `par-` is 179 and
   spills, `pcs-` 115 does not. Nothing spills today.
2. **The flag is a bit of the sealed flags byte**, so it is under the
   entry's tag; the spilled field is slot, rtag, length (32 padded), a
   125-character spelling, the same length as an inline name of 17-32 bytes.
3. **Name pages, not header A/B**: four one-page slots at stored 8-24 KiB.
   Reason: a header is rewritten per session (E4 F1's hazard), a rename
   would add a header rewrite, and a link needs two names live. Pages are
   holes for every file that never spills. The ring/REC region moves to
   start at 24 KiB.
4. **Binding by the record's tag in the entry (rtag)**, with fid, slot and
   ns in the record's AAD. Reason: the ns alone does not bind, because the
   open restarts ns at the largest listed ns plus one (design.md §4) and a
   lost entry's record can carry an ns handed out again (M3b, `pLP`).
   Rejected: restarting ns above every record's ns, which needs every name
   page read at open.
5. **Record before the directory op.** Not load-bearing under rule 6
   (entry-first passes, below); chosen so that no page-cache directory ever
   holds an entry without its record.
6. **`sync_dir` syncs the files with name records written since their last
   sync began, then the directory.** Load-bearing (M2). Placed there
   because every engine path that names a file in a manifest calls
   `sync_dir` after the directory op and before the manifest (sites read
   above), and the promotion never fsyncs the file it links. This is a
   sync the engine did not issue (design.md §4 says none is), issued only
   where a spilled name is pending; its cost on the promotion path (a
   one-page fdatasync a promoted partition) is unpriced (decision 10).
7. **Slot allocation by page-cache references, no directory sync to
   reuse.** A ghost entry that names a reused slot fails its rtag. Four
   slots; the engine holds at most two spilled spellings of one file.
8. **The open**: a spilled entry costs one 4 KiB pread and one AEAD open,
   outside the pager (not a `fill`), merged with the header read for files
   the open opens anyway; inline entries cost nothing. An unresolved
   spilled entry is withheld: the engine's missing-name refusal fires if
   the manifest needs it; otherwise it is presented as
   `spill-<fid>-<ns>.tmp` for the `.tmp` sweep (or left and counted if
   E0 item 4 keeps `.tmp`). Swept, not refused, because rule 6 means no
   manifest can need an entry whose record is not durable; the model's
   UNNAMED check is that claim.
9. **Rename to a spilled name is weaker than `rename(2)` until the next
   `sync_dir`** (the file may have neither name after a power cut). Stated
   as the layer's contract; the engine renames only from temps.

## The model (`spill.py`)

Built on `pfm.Dev` (objects whole or TORN, directory ops in any in-order
subset until `sync_dir`, or a prefix; power-loss states enumerated before
every file fdatasync and every directory fsync completes and at the end;
page-cache states after every step). New: sealed spellings with an inline
or spilled payload, name records as per-file slot objects, the layer's
create/rename/link/unlink/rename-over/sync_dir, the engine's paths, an open
that resolves, refuses or sweeps and reseeds ns, and runs continued after a
process crash (X) or across every power-loss state (L). A lowercase op runs
until its first barrier and the process dies there, so a run can go on from
a state where a record or an entry landed alone. The manifest is one object
written and synced whole (TORN reads as the durable image, standing in for
the engine's two slots). Checks: MISNAMED, AMBIG, UNNAMED (spill.py's
docstring).

Mapping of the brief's properties: "a spilled entry whose record does not
seal to it is never taken for a file the store named" = MISNAMED never fires
and an unresolved entry is withheld (the sweep then cannot remove a needed
name: UNNAMED in every later state); "no two entries resolve to one logical
name" = AMBIG (equal ns, or the open keeps an older entry); "a name a durable
manifest holds always resolves" = UNNAMED, checked against whatever manifest
image the crash state holds, durable or newer.

## Runs (from the spill directory; each output is in `results/`)

Design (all exit 0, 0 violations):

| command | runs | any | proc | secs |
|---|---|---|---|---|
| `nice -n 10 timeout 600 python3 spill.py n=4` | 3,511 | 253,982 | 96,658 | 2.7 |
| `... spill.py n=4 alpha=CTDPRUXLctdpru` | 4,735 | 767,174 | 177,606 | 3.8 |
| `... spill.py n=5 alpha=CTDPRUXLctdpru` | 43,999 | 15,343,583 | 2,699,250 | 79 |
| `... n=5 alpha=CTDPRUXLctdpru dir_mode=prefix` | 43,999 | 6,557,058 | 2,461,688 | 47 |
| `... n=5 alpha=CTDPRUXLctdpru NS=2` | 43,999 | 15,343,583 | 2,699,250 | 74 (table never full) |
| `... n=5 alpha=CTDPRUXLctdpru order=entry_first` | 43,999 | 15,343,583 | 2,699,250 | 77 |

Mutants, `nice -n 10 timeout 600 python3 spill.py n=4 alpha=CTDPRUXLctdpru <m>`
(all caught):

| mutant | first ops | verdicts |
|---|---|---|
| M1 `flag=ignored` | `C` | 489,930 MISNAMED (the stub read as the name) |
| M2 `dsrule=False` | `P` | 113,899 UNNAMED: link and `sync_dir` durable, record not, manifest `{L4}` durable |
| M2 `dsrule=False order=entry_first` | `P` | 113,899 UNNAMED |
| M2 `dsrule=False dir_mode=prefix` | `P` | 67,844 UNNAMED |
| M3 `bind=none` | `TP` | 1,543 MISNAMED: the link's entry lands, its record does not, the slot holds the spilled temp's record |
| M3 `bind=none dir_mode=prefix` | `TP` | 826 MISNAMED |
| M3b `bind=seq` | `pLP` | 40 MISNAMED: the cut promotion's record landed and its link did not; reopened, ns is handed out again into the same slot |
| M4 `alloc=fixed` | `CP` | 23,412 UNNAMED (the promotion's record overwrites the live name's) |
| M5 `ns_open=zero` | `CXR` | 6,582 AMBIG (rename-over after a reopen gets a lower ns; the open keeps the old file) |

Earlier smaller runs (n=2, n=3) gave the same first counterexamples, except
that M3b was not reachable before the lowercase (cut) ops were added: a power
cut only between whole engine ops never leaves a record without its entry
for the next op to meet. That was a hole in the model, fixed before the
results above.

## Not verified

- The model has one store directory, at most five engine ops, files with
  one data object and up to four name slots, and a manifest that is one
  atomic object (the engine's two-slot rules are E4 F6's and the slots
  item's, not re-checked here). Counts are model states, nothing real.
- Concurrency is not modelled: the seal's fsync on its thread beside the
  segment work's `sync_dir`, and the per-Handle sequencer's "a sync in
  flight that began after the record" are argued, not checked.
- Failed writes and failed syncs of a name record, and `sync_dir` failing,
  are not modelled.
- The name pages' interaction with the header and session protocol (E4's
  Log class) is not modelled: records are under K_f and independent of
  sessions by construction; a Log file (wal, spare, run temp) never has a
  long name today, and `.pfcopy` compaction of a spilled Log name is not
  modelled.
- The engine's sweep is modelled as "remove every name the manifest does
  not hold"; the real sweep's kinds, the run temp's recovery and
  `manifest_pending`'s pending names are not in the model. Whether every
  engine path that names a file in a manifest calls `sync_dir` after its
  directory op is by reading the seven `sync_dir` sites listed, not by a
  search of every manifest publish.
- The ns order across crashes is checked only as the AMBIG property over
  the paths modelled; the design's ns rule itself (max listed + 1) was not
  re-derived.
- Costs: the extra fdatasync on the promotion path, the open's one page per
  spilled entry, and the merged header-and-names read are not measured. That
  no name spills today is arithmetic over the constructors found by grep of
  `format!` sites (one test-only `seg-{compact}.sup` in zpages.rs ignored);
  a constructor built another way would be missed.
- The `.tmp` presentation of an unresolved entry depends on E0 item 4,
  undecided.
- Leakage: a spilled file's name pages are allocated blocks, so a reader of
  the device learns that a file's name is longer than 128 bytes; not in
  decision 4's list.
