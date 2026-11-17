# formsites: the skeptic's check

Scope: `sites.md`, `notes.md`, `sites.py` and `found.txt` in this directory,
read against `plan/tree/src` at 682b2c09. No Rust was compiled or run. Every
Python run used `timeout 600 nice -n 10 python3 -I`, one at a time. My scripts
and their output are in `check/`.

## Verdict

The list stands, with small corrections. A third derivation of my own found no
site that reads a form page and is missing from sites.md. By reading every
`None` arm I agree that no site turns a failed form read into an absence.

Two claims are stronger than what was shown:

1. **sites.py's completeness check guards against new names only.** Its
   TABLE is keyed by (file, function name), and functions with the same name
   are merged. Four mutants of my own pass it with exit 0: a page read added
   inside an existing META method, a new reader whose name is a META row, a
   new reader whose name is an OTHER row, and a form read in a file the
   searches do not open. The property "layers 1-2 are complete" holds on this
   tree, by my independent search. It is not held by the instrument against
   the next change.
2. **"The controller sees `damaged()` and rebuilds ... nothing in zfile.rs or
   db.rs changes" is false under a cipher.** I traced it by hand; it is not
   run. Under a cipher, retiring a form that went bad in service does not
   take its envelope off the form's name. The "rebuild" then attaches the
   same damaged envelope again instead of building a new one. The proposed
   pager-only change therefore gives fallback without repair. It also needs
   a vault change and a test (T5), both below.

## What I reran

- `sites.py <tree>/src`: `A 63, B 123, union 124; READS 62 (both 61, A only
  0, B only 1); unclassified 0`, exit 0. This matches the work.
- `sites.py --mutant 1..5`: M1-M4 exit 1 and M5 exit 0. This matches the work.
- `check/third.py <tree>/src`: the third derivation (below). It writes
  `check/third.out`: 121 functions flagged out of 2413 parsed. Most of the
  extra flags are noise from name collisions (`load`, `key`, `seek`),
  resolved by hand.
- `check/tok.py`: every non-zfile function that names a form cell or form
  type (`.z.get()`, `z_spare.get()`, `zc(`, `ZCursor::`, `PWalk::Z`,
  `PartReader`, `ZFile::attach`, `warm_file`). It found 62 functions. All 62
  are in `found.txt`, classified.
- `check/tok2.py`: the lines in each ABOVE/META function that touch the
  form. None calls a ZFile or ZCursor byte method directly. Each one goes
  through a layer-3 dispatch (`part_*_z`, `cache_*`, `pwalk_*`,
  `PartReader`), or calls only `damaged`, `id`, `pages`, `file_bytes`,
  `coded`, `resident_bytes` or advice.
- `check/sites_skeptic.py`, a copy of sites.py with my mutants 6-9, and
  `check/mut_third.py`, which runs third.py on the same mutants.

## The third derivation, and the diff

This method differs from both of sites.py's searches:

- Comments and string literals are stripped.
- Functions are delimited by brace matching, not by "the last fn above".
- Each function is attributed to its `impl` owner.
- Every `.rs` under `src` is searched, not only the 12 files that sites.py
  lists.
- Test modules are cut.
- The seeds are reads of a mapping (`self.map.slice_at/as_slice/read_at/...`,
  a local `map.slice_at`), the cursor's held borrow (`Group::Page(` /
  `Group::Coded(`), an `Opened::Borrowed/Coded` borrow, and `zpg_name` beside
  `vault::map|attach|warm_file`.
- The closure is a fixed point over calls of any ZFile/ZCursor method in the
  set, by name, across all files. Collisions are printed, not filtered by
  receiver.

Against sites.md, after resolving name collisions by reading:

- **zfile.rs:** every READS row of sites.md is found. I found no READS
  function that is not in sites.md. My extras are
  `Opened::bytes` (an accessor over held form bytes, which the work classes
  OTHER; it is of the same kind as `ZCursor::group`, which the work classes
  READS), and the pure checkers `first_checks`, `first_decode`,
  `verified_group_in` and `engine_filter`. Those take form bytes already
  read (the work names them as pure checkers). `engine_filter` and
  `ZFence::decode_resident` belong under row 3 (the resident part), which
  does not name them.
- **db.rs:** nothing missing. `Db::open` (28965) and `z_attach_on_disk`
  (42904) call `Db::z_attach`, and row 46 does not name them. Row 46 says
  "four attach calls" and lists five (33282, 33387, 43229, 43276, 43347). Five
  is right.
- **Other files:** `ordindex.rs`'s `self.map` is the ordered index's own
  mapping, and `vault::remove` is noise. `vault::map` call sites in db.rs
  (9038, 13737; `vault::read` 7669) take segment and run names; the only
  form path is `z_cross` 42437, which is advice only. zfile.rs's three
  `vault::map` sites are attach, warm (form) and `open_seg` (segment).

## Mutants of the instrument (mine)

| # | change to a copy of the tree | sites.py | third.py |
|---|---|---|---|
| M6 | `ZFile::file_bytes` (META) also does `self.map.slice_at(len-8, 8)` | **survives** (exit 0) | caught |
| M7 | `impl Drop for ZFile { fn drop(..) { self.map.slice_at(0,4096) } }` (name = META row `drop`) | **survives** | caught |
| M8 | `db/blocks.rs` gains `form_peek(path)` = `vault::map(path)..slice_at(0,1)` | **survives** (file not searched) | **survives** (no `zpg_name` beside it, so the path is opaque to text) |
| M9 | `ZCursor::sample(&self)` reads `self.zfile.page_bytes(0)` (name = OTHER row `sample`) | **survives** | caught |

So sites.py's exit 0 means "no new function name touches the form". It does
not mean "no new site reads a form page". To hold the list in a gate, key the
table by (file, impl owner, name), or better by span. Fail on a READS-seed
hit inside a non-READS row. Search every file. M8 is out of reach of any text
search. A path handed to a function from outside is the case for the "door
by symbol" check (G2), not for a grep.

## Corrections to sites.md and notes.md (reading, line numbers of the tree)

- **"None arms reachable only under a cipher; the unit tests drive them once,
  through count alone."** The integration test `tests/cipher.rs:1941`
  (`a_changed_byte_in_a_form_file_is_refused_or_the_form_is`) does reach them
  through real tag failures. But `Entered::finish` -> `end_slow` (db.rs:19064)
  replaces every answer with the poison, so **no existing test can see a
  `None` arm answer wrongly**. That is the stronger reason T4 is owed.
- **Finding 5 ("three failure paths go raw without `fail`") is
  incomplete.** More paths go raw without `fail` on a heap fence entry
  that does not decode or does not cover the rank:
  - `locate_rank` (`entry_at_rank` None -> `Fallback`)
  - `locate` (`fence.page(e)` None)
  - `ZFile::seek` (`fence.entry` None)
  - `scan_in` (two `ScanEnd::Fallback` returns at the top of the entry loop)
  - `ZCursor::seek` (`fence.entry` None)
  - `ZCursor::load` (`entry_at_rank` None -> `fall`)
  - `open_group`'s `u8::try_from(j)` (this one does `fail`)

  None of these reads a page, and none gives a wrong answer. They all leave
  the form in service. Under R8 they are "fence damage", which the attach's
  resident-part CRC is meant to make unreachable.
- **Row 14** says scan_in takes F. Two of its fallbacks are F- (above).
- **Row 46:** "four" should be five. The callers of `Db::z_attach` are
  `Db::open` (28965), `z_give` (33342) and `z_attach_on_disk` (42904).
- **Design point "A rename-over gives the rebuilt file a fresh handle", and
  sites.md step 3, "`Vault::rename` gives the new name a fresh entry".**
  `Vault::rename` (vault.rs:1013) moves the entry: the old name's
  `Entry{page, disk}` goes to the new name. A fresh entry for the form's name
  comes only from the build's `vault::rename(tmp, out)` (zfile.rs:4360),
  which replaces whatever `out` held. The retire's rename does the opposite
  (next section).
- **Note A.** I checked that every `PartReader` is a local of one block
  function (`walk_block`, `deltas_for`, `walk_deltas*`, `copy_from_deltas`,
  `patch_block`). `build_rest` is called only from `abandon_slow`
  (db.rs:22676, 22684) and `ahead_look` (32359), whose docs say "between two
  blocks or two keys". This agrees with note A at the reading level. I did
  not trace every caller of `abandon_asked` for a cursor held across a key.

## The state that breaks "the controller ... rebuilds" (traced by hand)

Under a cipher, with the proposed change (a form chunk's failed tag is no
longer a poison):

1. A read meets a failed chunk of `x-T.z`. `page_bytes` returns `None`,
   `ZFile::fail` runs, and `damaged()` becomes true.
2. A rebuild is chosen. Inline: `zpages_settle_with` calls `z_retire(seg)`
   then `z_give(seg)` (db.rs:33560, `z_give` at 33564). Under Maint: `z_start` with
   `ZKind::Rebuild` calls `z_retire_cell` then `z_attach_on_disk`
   (db.rs:42944, 42966).
3. `z_retire_cell` calls `vault::rename(N, dead)` (db.rs:5425). A form name
   does not `pass` to the directory, so this is `Vault::rename`, which moves
   the map entry and nothing on disk. The envelope stays at `x-T.z`. On a
   plain store the same call is `std::fs::rename`, and the file is gone from
   `N`.
4. `z_give` (with `refused` false, since damage in service sets no
   `DECLINED_REFUSED`) and `z_attach_on_disk` both call `Db::z_attach` first.
   `Db::z_attach` calls `vault::companion(dir, seg, N)` (db.rs:33281). `N` is
   not in the table, and `x-T.z` exists. Companion inserts `N` with
   `disk = {token T, Form}` and with `page` taken from any entry of the same
   `Disk` that has a view, which is the dead entry's view of the damaged
   envelope (vault.rs:1509-1512). With no view it would make a fresh `sparse`
   view of the same damaged envelope anyway.
5. `ZFile::attach(N)` maps that view. If the trailer, the resident part and
   the 16 sampled pages miss the bad chunk, the attach succeeds. "Attached,
   nothing built" follows, and the damaged file is back in service. The next
   read of the bad page fails it again, and after `ZREBUILDS` rounds the
   partition is `DECLINED_DAMAGE`: raw for the segment's life. If the attach
   does hit the bad chunk, it refuses (`REFUSED`), and only then does a build
   run and replace `N`.
6. Later `vault::remove_file(dead)`: `Vault::remove` keeps the envelope,
   because `N` (from companion) shares its `Disk` (vault.rs:988-991).

Answers stay right throughout: the raw form serves. But the claim that the
store is "repaired, as the plain store does" does not hold under a cipher.
The same resurrection already happens today without damage, at the
ahead-coding check in `z_attach_on_disk` (db.rs:42889-42895): rename to a
dead name, then attach. Under a cipher this re-attaches the file in the
refused coding. That is a pre-existing divergence of the vault from the
plain store, not caused by this item. Today a damaged form under a cipher
never gets this far, because the poison stops the store first. The proposed
change makes the path live.

What it takes, stated only as the requirement: once a form's name is retired
under a cipher, the form's envelope must not be reachable under that name
again. Two ways would do it. One: `Vault::rename` of a `Kind::Form` entry to
a non-form name moves the envelope off the token name, or marks the token's
form dead. Two: `companion` refuses to adopt an envelope whose `Disk` a
non-form name holds. Either way it is a vault.rs change, beside the pager
change.

## The smallest sound change: what I agree with, what to add

- **Agree:** the class belongs on `Back`, set in `Vault::back_of` from
  `disk.kind`. Every `Back` is built through `back_of`: vault.rs:932, 936,
  1082 (`unmapped`), 1243 (`settled`), and nowhere else. `Back::open` has
  only that caller. A `Paged` is shared only by mem `Arc` or `Disk::same`
  (token and kind), so a Form-class `Back` never sits under a Seg/Ord name.
  `fill`'s three errors are no envelope (NotFound), a short envelope
  (UnexpectedEof) and a failed tag. Only the last two can occur with a
  `Back`, and both are the file's own. `checked_header` requires the
  envelope's length to equal `stored_len` (crypt.rs:446), so a flipped
  header length cannot make `Paged::make` allocate past the file. No durable
  job reads a form file: merges read `seg.raw()` (db.rs:13737). The build's
  "a record that does not read ... the store's cache remembers"
  (zfile.rs:4341) is about the **segment** (Kind::Seg), which keeps
  poisoning.
- **Add T5 (owed):** under a cipher, with `ZPages::Always` and then the
  controller, make one page of an attached form fail after the attach (a
  changed chunk away from the trailer, the resident part and the sampled
  pages, or a pager seam). Read it, then `zpages_settle`. Require: a **new**
  attach id for the partition, `bad_pages == 0`, `rebuilds == 1`,
  `declined_damage == 0`, reads served by the form again, and the old
  envelope's bytes gone from `x-T.z`. On the tree plus the pager change
  alone, T5 should fail at step 5 above. That would hold the vault change.
  Mutants: companion that rebinds a dead view (T5 red), and a retire that
  leaves the envelope (T5 red).
- **T1's `refused == 0`:** a flip in the zero padding after the envelope
  header, or in the header, makes `Back::open` fail. That surfaces as
  `vault::map` Err -> `Reject::Damage`, with no poison, so the open
  should not refuse. I found no other read of a form envelope at open
  (`Vault::len`'s header parse is not called for forms at open). This is
  reading, not run.
- **T3** is the test that holds "File arm returns false over the zeroed
  chunk". The work's reasoning is right that T1 alone may pass that mutant.
  With `zpages_trusted_views` a verified page re-read as zeroes goes
  through `view_trusted`, which is exactly the wrong-answer case.

## What the page layer's interface must offer: one item to add

7. **A file's name and its bytes are one identity.** Retiring or renaming a
   Regen-lazy file under any name makes its old bytes unreachable under the
   live name. A later "is it on disk?" or attach under the live name sees
   either nothing or the rebuilt file, never the retired one. Today's vault
   keys envelopes by (token, kind), so two names share one envelope, and
   item 5 of sites.md ("a rename over the file gives the new file a fresh
   handle") is not enough.

## What I did not check

- No Rust compiled or run: the pager change, T1-T5 and their mutants are
  unrun. The resurrection in steps 1-6 is a reading of vault.rs and db.rs,
  not an execution. In particular I did not confirm that the controller
  path reaches `z_attach_on_disk` with `refused == false` for an in-service
  damage under `Auto`; I read it for `ZKind::Rebuild`.
- Whether the attach's sampled pages hit a given changed chunk depends on
  the file. T5 has to choose the chunk, or seam it.
- `abandon_asked` callers inside key loops were not traced for a held
  `ZCursor` across a rest.
- Calls through trait objects or function pointers, and paths built in one
  function and read in another (M8), are outside every text search here,
  mine included.
- tests/ was not searched for readers (they are not sites). wasmapi.rs,
  crypt.rs and blocks.rs were searched by third.py, with no hits.
- The `remove(dead)` / `persist_form` ordering race was not checked.
  `persist_form` renames the new envelope over `x-T.z` and only then names
  it under the lock, so a `remove(dead)` between the two could unlink the
  new envelope when no entry holds the `Disk` yet. It is plausible from
  reading and not confirmed. It belongs to the vault, not to this item.
