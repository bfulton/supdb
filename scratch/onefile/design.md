# A store in one file: decisions so far

The owner's proposal, in the session that priced freeing retired files
(`scratch/reap/notes.md`): a file abstraction of the store's own over a
single real file, for control of what file operations cost and for mobile.
Nothing here is built. The measurements each decision rests on are in
`notes.md`, with the probe that makes each.

## Decided

- **One file a store.** Its head holds the manifest's two slots, written in
  place as the two slot files are now, never the one holding the newest
  manifest known durable. Everything else is extents: segments, their
  ordered indexes, their form files, and the log.
- **The manifest names extents** (offset, length) where it names files. The
  free space is what no manifest entry covers, derived at the open; no
  allocator state is durable.
- **Space is reused, never freed in steady state.** An extent is free once
  the manifest that stopped naming it is durable and nothing holds its
  segment (the holder check of `reaper-v1.patch`, a `Weak<Seg>`). No
  automatic shrink; a trim a caller asks for.
- **The log is extents of the same file**, from the same allocator: a
  rotation takes the next, a retirement gives one back. A commit syncs its
  own range (an `O_DSYNC` write), never the file.
- **Growth reserves and does not write** (`fallocate`, in large steps). The
  manifest keeps the highest offset ever landed; a landing under it is made
  durable by its range's writeback and the slot's fdatasync as the one
  flush (Linux, ext4 and XFS), one over it by the file's own sync. Where a
  file system moves blocks it is written to, the file's own sync always.
- **A promotion is a manifest publish.** No link.
- **A checkpoint is a full copy** (the owner's decision): the live extents
  and a manifest written into a new file, a store of its own, as
  `Db::checkpoint` promises now and as its fallback across file systems
  already does. The extents being copied are held from reuse for the copy's
  length, in the process; no store refers to another's file. About 1.2-1.4
  s a gibibyte on this disk. Copied by the kernel's own call, so that a
  file system that can clone does.

Considered and set aside: the manifest as a file of its own so that a
second manifest is a store over shared extents (instant, but the origin
has to keep and release pins, and the checkpoint cannot be deleted, moved
or shipped by ordinary tools); a ring format for the log.

## Open

- The name of the copy. "Checkpoint" came with the links (RocksDB's name
  for a directory of them) and elsewhere means a durability point, FASTER's
  in this suite among them; "snapshot" is taken by the read view
  (`Reader::snapshot`). A full copy of a live single-file store is LMDB's
  `mdb_env_copy` and SQLite's backup. Candidates: `copy_to`, `backup_to`.
- Whether this is built on the encryption branch or as the line after it.
- Ordered ingest: a run appends into an extent and its commits put their
  marker in the log; today the run's file is its own log.
- Direct synchronous commits in the shared file: 0.13-0.16 ms a commit
  beside any background writer, which they starve (`login.py`). A lever on
  what a commit pays for background bytes, or a hazard; not designed.
- Extents contiguous on the file, or units mapped side by side: the readers
  lend from one mapping of a segment.
- What a torn write over an extent's previous contents leaves, by test.
- No benchmark opens a checkpoint: a timed pass for it comes before the
  change, so that the links have a number.
- The ceiling of the frees alone: a run with nothing freed until the close,
  at three million keys for the disk's sake.

## O2 as built: the decisions made writing it (2026-11-26)

What `scratch/plan/onefile.md` left to the stage, decided here before the
code and held by the tests named.

**A directory in memory, not extents in the engine.** The engine keeps
naming its files. The door sends a call to the store's table
(`src/db/onefile.rs`) where the store is kept in one file and the name is
one the table holds; a rename, a link and a removal are changes to the
table. The cipher's vault is the precedent: names behind the door, their
places in the manifest. The engine's 66 thousand lines name segments by
file name in forty-three removals and fifteen renames; none of them
changes.

**A file is one extent.** A reader maps `(offset, length)` of the data
file through an open of its own, and `MmapBytes` is the kind it was
(`Own::Map`), so no reader of the format is touched. The price is a writer
that outgrows its room: it grows in place where the room after it is free,
and is moved, its bytes copied, where it is not. A writer says what it
will hold where it knows (`vault::expect`: a merge's output, a form
build, a seal), so a move is a wrong guess and never a fault. A mapped
file is never moved. Several extents a file was set aside: it stitches
mappings, breaks readahead at each seam, and puts a list behind every raw
offset the engine hands the kernel.

**Which names.** A name is the table's where it is made as one of the
kinds the table takes (`onefile::takes`: `seal-`, `compact-`, `tier-`,
`ord-`, `zpg-`) or is renamed or linked from one. Every other name is the
directory's: the log, the slots, the lock, and in this stage a run of
ordered ingest and the segment it is linked as. The manifest lists a place
for each table name; a segment name with no place is a file of its own.
So a store in one file holds both kinds until O3 moves the run in.

**A manifest names only bytes the device holds, every kind.** Today the
ordered index is written unsynced and an open checks it; in reused space
an unsynced file's extent can hold an older file's bytes that pass a check
made for another reason, so the rule is the segment's for every file: a
row is listed once a sync has covered the file's last write. A file a
store regenerates asks to ride (`ExtFile::ride`) and is covered by the
next sync of the data file, which is the sync of the segment it was
written beside; one nothing has covered is left out of the manifest and is
free space after a crash, which is its contract. The seal's index rides
the seal's own sync and costs no flush, and a merge's is written ahead of
its piece's sync in a store in one file (`Emitter::index_first`), so both
are in the manifest their landing writes: a store killed straight after a
landing opens with no index to compose (`tests/onefile.rs`,
`an_index_is_named_by_the_manifest_that_names_its_segment`). A form
file is built behind its segment's landing and rides nothing of it; the
segment work lists what is left over on its thread's turn once the store
is quiet, and at its stop (`Maint::one_look`).

**Step A of a landing's durability: the data file's own sync.** Every
sync of a file of the table is `fdatasync` of the data file through the
handle's own description: contractual on any filesystem, and its error
is the handle's (errseq is a description's). It also writes whatever the
other writers have dirty, which the probe priced at nothing for three
writers (`writers.py`). Step B, behind an option and priced against A:
range writeback and one flush for a landing into blocks written before
(`sync_file_range` then a synchronous write of a page of the head, which
flushes the device and reads this description's errors); a landing into
blocks never written keeps the file's sync, since their metadata is the
file's. That needs the table to know which free ranges are instantiated,
which it learns as files retire.

**The check for a slot that is not this file's.** The newest durable
slot's rows are whole by construction. A slot an open falls back to past
a damaged one may list room that has been written over since, where the
plain store finds a name that is not there; the row carries the checksum
of the file's first page, and `vault::on_device` answers it. It catches a
file written at the same offset and no other overlap, which is damage the
segment's own checksums are for. The checksum is taken from the writes
that made the page (`onefile::Head`): the table keeps a file's first page
as its writers make it, a page at the most, and gives it up for the sum
where the file is first listed. It was first read back at that listing,
and a form file built ahead and dropped from the page cache as it is
written was read from the device for it, a page a file on the segment
work's thread, with the page left in the cache; the controller's tests
found it the day they first ran over a store in one file
(`OneCounts::head_reads` counts a read-back, which is left for a page
written or cut after its listing).

**Reuse.** Room goes back when no name, handle or mapping holds it, and
where a manifest listed it, once a manifest of a later sequence is known
durable (`One::durable`, called where `SlotChoice::landed` is). The
pending arm and the rename shape of the manifest are refused beside the
option: the first names a segment before its sync, the second has no slot
whose durability is told.

**A checkpoint is a copy, of files.** `checkpoint_file` links, and copies
where a link is refused; a link out of the data file is refused, so each
file is copied out and the checkpoint of a store in one file is a store
of files, opened by its manifest like any other. `copy_to` and one file
at the destination are O3's.

**A test's crash.** `Db::crash` forgets a store and leaves its threads
running; a merge of the forgotten store would write through its handles
into room the store opened after it has given away. The table is killed
there (`One::kill`): no write through it after, none in flight. A real
crash has no such thread.

**What the option is under a cipher.** Nothing until O4: the vault's
layout stands. Under the kernel's key the data file is one more file of
the directory.

**What a store of files pays for the second kind of file: nothing, and
how.** The first shape made the door's file an enum of the two kinds and
gave a mapped part of a file a third kind of `MmapBytes`. Counted against
the commit before (`sh scripts/cost.sh 84a94f4 commit_durable
commit_buffered seal land read scan`): twenty instructions a commit in the
run's writer, whose `pwritev` closure no longer inlined, four a sync, one a
key in the seal probe, six and seven a scan in `scan_seek` and `raw_scan`,
which match the mapping's kind where they ask for a piece. Each a cost. The
shape that pays nothing:

- *The mapping.* A mapped part of a file is `Own::Map` like any mapped
  file, and what it holds on to is a field beside the kind
  (`MmapBytes::_hold`), dropped after the mapping. No match of the kind has
  a third arm. `MmapBytes` is eight bytes larger.
- *The file.* `vault::File` is a descriptor at one place in both kinds and
  an extent where it is one (`os`, `ext`). A call asks `ext` once, which is
  two instructions a call; a caller that makes a test of its own before the
  call carries the kind there and calls `File::plain`, which asks nothing.
  Under a file of the data file `os` is a descriptor of nothing
  (`/dev/null`, read-only): a write through it is refused, a sync of it is
  an error, so a call that should have asked fails where it is made and
  never writes the data file at an offset that is another file's.
- *The writer.* A segment's writer asks one byte before every write and
  one before every sync (`AlignedWriter::fault`). A file of the data file
  sets a bit of it for the writer's life (`RUN_FILE_PLACED`), so its writes
  and syncs take the paths past those compares, where the call is made on
  the file as the kind it is, and the paths under them call `plain`. The
  writeback hint after each piece is the handle's own for such a file
  (`File::hint_writes`), the writer's being a call on `plain`.
- *The log and the slots.* Files of the directory in this stage, by their
  names (`onefile::takes`): their commit-path calls are `plain`.
- *The durable manifest's signal.* `Maint::slot_synced` asked one flag for
  a test's record; a store in one file is a second bit of it, and the
  table is told on the cold side.
- *The listing look.* On the segment work's loop, beside its park, and at
  its stop. Not at a settle: a settle is counted in the landing probe, and
  a look there was two instructions a settle.

**The public writer's loop, and what was tried on it.** After the mapping
and the writer's fast paths, the seal probe (`SegmentWriter` through its
public calls, `begin`, `value`, `end` written into the probe's own loop)
still read one instruction a key over the base in the probe's function,
with no source of that loop changed. Eight builds of the probe, each
counted with instruction dumps (`valgrind --tool=callgrind --dump-instr=yes
--dump-line=no --toggle-collect=seal_probe::seal`, summed by mnemonic and
laid side by side):

| build | loop | per-record write |
|---|---|---|
| base `84a94f4` | 0 | 0 |
| base, a drop hook on the 4-byte file | 0 | 0 |
| base, the writer's file field widened to 16 bytes and nothing else | +3 | 0 |
| head, file of two fields, finish written into the caller | +1 | 0 |
| head, file of two fields, finish out of line | +3 | 0 |
| head, file a bare descriptor and a table by its number, finish out of line | +3 | +2 |
| the same, finish inlinable; the writer's fast paths on the general calls; the writer's write path and constructor as the base has them | +3 | +2 (each) |
| head, file of two fields, the boundary kept | +4 | -7 |

The differences are where the compiler keeps the loop's values: register
moves, one or two more values on the stack, the same loads and stores of
the writer's fields at the same offsets. A field twelve bytes wider is
enough to move it (row three), and so is everything else, in either
direction; the bare descriptor, built to give the writer its old layout
back, read worse and was taken out. What was taken back is real work on
the same path: every record written asked how far the next write boundary
was by a remainder, a divide a record (`AlignedWriter::until_boundary`); the
boundary is kept now and asked by a subtraction and a compare, seven
instructions a record fewer, so the probe reads three a key under the base
by its total with the loop's four over inside it.

One build of the eight had no count at all: valgrind 3.22 reads no symbols
from an executable whose last writable segment begins on a page boundary
with the RELRO segment ending there, so the toggle met no function and
collected nothing. A string a few bytes longer moved it. `scripts/cost.sh`
would read such a build as a probe in which no line names a function.

**The same loop on the stage's last tree, and what was in it all along.**
The count taken again once the stage was whole (`sh scripts/cost.sh
84a94f4`) read the probe's function nine a key over the base where the
table's last row has four, and the probe two over by its total: by
mnemonic eleven moves and an add more, four address computations fewer,
a jump, and one padding `nop` for another. Nothing of the writer had
changed since the table; the table's own third row says a field's width
moves this loop, and what had changed was elsewhere in the unit. What
gave it back was real work the listings had carried through all eight
builds: `SegmentWriter::layout` called twice a key for a choice made at
the first key, twenty-four instructions a call. With the answer inline
and the choice out of line (`layout_first`) the probe reads 53 a key
under the base, its own function two over with the answer in it. The
merge's step a key had the same shape, a piece's finish written into
`Emitter::leave`, twenty-one a key in the base (`leave_piece`). What the
count still reads red by function is in `ROADMAP.md` ("A verdict from
the count"): callees inlined into callers that lost more, names that
changed, and sixty-five instructions a segment at its making and its
finish.

**Growth, and a file that outgrows its room.** The data file is made
longer by what is asked for and an eighth of its length besides, sixteen
pages at the least (`STEP`); a mebibyte at the least for a day, which made
a store of a few hundred kilobytes reserve megabytes and filled a disk
with the tests' stores. A file that outgrows its room takes the free room
behind it; where that room runs to the data file's end and falls short,
the data file is made longer for the rest; only a file with another
behind it is moved. A file is placed at the power of two at or over its
size, a page at the least and a mebibyte at the most, so that a piece
written at a piece's offset in the file is at one in the data file and
the page cache makes of it the folio it made of a file of its own.

**What an unlink did, done here.** Room is given back in three steps: set
aside where its last holder lets go, its pages dropped from the page
cache with the table's lock not held (`POSIX_FADV_DONTNEED`), and then
free, or waiting where a manifest on the device may still name it. A
store of files drops a retired file's pages at its unlink; kept, they are
what a reclaim under a bound spares while it takes the files written
since. And each mapping is made through an open of the data file of its
own, closed once the mapping stands: the kernel's readahead window and
its count of a mapping's misses are the open file's, and a store of files
has them a segment (`CLAUDE.md`, "Room given back that kept its pages").

**The tests' seams.** By the directory, for a helper that has a path and
no store: `Db::one_names`, `Db::one_place`, `Db::one_read`; by the store,
`Db::one_counts`. The tests' own helpers go through them
(`tests/common`: `store_paths` lists a store's files as paths whichever
way it is kept, and `store_place`, `store_len`, `store_file` and
`cached_pages` answer for a path where its bytes lie), so a test that
lists files and then opens one by its path fails on the path under the
switch where a listing of the directory would have found nothing. In the table, under `cfg(test)`: `tight`, a first room
of the first write's length, and `mid_sync`, a call inside a file's sync.

## O4 as built: a store under a cipher in the data file (2026-11-26)

**The least form.** Under a cipher a store's kept files are memory files
while they are written and envelopes after, and the envelope was a file
of the directory under a token (`x-<token>`, `.o` an ordered index's,
`.z` a form file's). In a store kept in one file the envelope is a file
of the vault's own table (`Vault::one`), an extent of `store.one` under
the same name; nothing else of the envelope, the pager or the keys
moved. The door's operations on an envelope go through six helpers of
the vault's (`env_create`, `env_open`, `env_back`, `env_exists`,
`env_remove`, `env_rename`), each the table's call or the directory's.
A view's backing is a mapping of the envelope's range made through an
open of its own (`One::map_held`), the room held for as long as the
mapping stands (`pager::Back::over`), and a drop of its pages is a drop
of that range of the data file. Pagefile's page-aligned chunks with the
tags apart are not built here: nothing O4's cells measure asks for them,
and the stage that does is the run sealed as it is written (O5), whose
envelope cannot open with a header that holds its length.

**The manifest.** The one-file format's (`STORE_FORMAT_ONE`), the table's
section behind the names, sealed whole as a slot is under a cipher: the
names the store has its files under are in the sealed manifest, the
tokens and wrapped keys behind it, and the section lists each envelope's
place under its token's name. An open reads its slots and makes their
segments known first, and makes the table what the slot it takes lists
after (`One::adopt`), so the ordered indexes are found then
(`Vault::adopt_ords`). The store says how it is kept: an open asks for
the cipher and nothing else.

**The syncs.** A segment's envelope is synced through its own handle,
which answers its writes' errors; its ordered index's is written ahead
of it with no sync and rides that one, so a segment is one flush of the
device and the manifest that names it lists its index. The first
version wrote both with no sync and synced the data file once through
the table's handle at the call's end. A writeback error is reported to
each open file once: one that another thread's sync through the table's
handle had already taken would have been answered `Ok` to the segment's
writer, and a manifest named the segment. Read off O2's own comment on
`One::sync` ("a segment is covered by its own handle's sync alone")
before any test went near it; the files layout's rule on a kept sync's
error is the same rule.

**Which table.** Two lookups. `vault::one_under` is the plain store's
table and nothing else, asked by a plain store's writers for what only
it wants of them (a seal's pieces' room said ahead, a merge's index
written before its piece's sync). `vault::one_of` is either store's, by
the directory the engine names its files in or the store's own: the
manifest on the device told to the table, the segment work's look at
what no manifest lists (`Maint::one_look`, the names by `Vault::beside`),
and the tests' seams. One lookup for both was the first version, and
gave a seal under a cipher the plain store's announcement of its pieces'
sizes, which the cache's room reads.

**A kill, in a test.** `Db::crash` leaves a store under a cipher held
open in the process (the vault's registration is the forgotten store's),
so a kill of one is a copy of its directory with the segment work
paused, opened elsewhere (`tests/onefile.rs`, `kill`): the slots, the
log and the data file as the death leaves them and no memory file. The
tests of `tests/cipher.rs` take the tests' switch (`base`), and read a
store's envelopes by name while it is open, a closed store kept in one
file having their names in its sealed manifest alone.
