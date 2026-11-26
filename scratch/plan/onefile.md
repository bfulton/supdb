# Build plan: the store in one file

The owner's decision, 2026-11-26: "Let's build all this here because we
think this helps us clear the perf bar." Decisions and their probes:
`scratch/onefile/design.md`, `notes.md`. This file is the order of the
work and what each stage is held to. Nothing below is built at `5d7fbe2`.

## What it is, against the layer's design

`scratch/seam/design.md` (Pagefile) put one sparse stored file under each
logical file of a store under a cipher, with sealed names in the directory,
a header and a journal ring a file. The container is that design's lower
half done once for the whole store, and for the plain store too:

- **Kept from it:** the door's file type (`vault::File`, its S1), the five
  classes as contracts, a page of plaintext stored as one page with its tag
  apart, sessions and counter nonces, write-through into the pager, the
  crash model's method (E4), the fault-injection mode, every "what stays".
- **Replaced:** sealed names and their NAME_MAX budget (names live in the
  manifest, sealed with it); a stored file a logical file (an extent); the
  3 MiB prefix a file; headers and rings a file (the manifest's slots at
  the container's head, the log's extents); `rename`/`link`/`unlink` of
  stored files (manifest publishes; a free extent).
- **New:** an allocator over one file, derived from the manifest at the
  open; reuse gated on a durable manifest and on nothing holding the
  segment; growth reserved (`fallocate`) and a written-once mark; a
  landing's durability as its range's writeback and the head slot's sync;
  the log as extents; `copy_to` in `checkpoint`'s place.

Three layers, bottom up: **L0** the door's file type; **L1** the container
(`src/db/onefile.rs`, no cipher in it and no format of a segment); **L2**
the sealing over extents (the page, tag and session half of Pagefile).

## Experiments owed before code (cheapest first)

- **X1, the frees' ceiling.** `reaper-v1.patch` with nothing handed to the
  reaper until the close, paired with `supdb` at one and three million keys
  (the disk holds no more), load and lag. What every free is worth in the
  bursts and the passes after the load; O2's prediction. *Kill:* nothing
  resolved means the container stands on the mobile case and the segment
  write's 44% alone, and O2's pairings say so.
- **X2, one inode under many writers.** N buffered writers of megabyte
  pieces into N files against N extents of one file, with a small ranged
  commit beside them, in the open and under a memory bound: bytes a second
  and the commit's p99. *Kill:* extents losing a tenth or more of the
  writers' rate sends segment writers through `O_DIRECT`, priced there.
- **X3, a mapping an extent.** A cold scan and a pass of point reads over
  64 MB as a file of its own and as an extent of a 2 GB file mapped through
  an open of its own (its own readahead state): faults and time.
  *Kill:* a difference past the floor is designed for before O2.
- **X4, a timed pass for `checkpoint`** in the suite (make one, open it,
  read through it), so the links have a number before `copy_to` replaces
  them.

## Stages (the gate green at each end; the count level with the option off)

- **O1 -- the door's file type.** Pagefile's S1 as written: `vault::File`
  with the `Os` arm alone, the raw-descriptor sites through the door.
  Gate: `sh scripts/cost.sh <commit before>` level by function, `sh
  scripts/check.sh`.
- **O2 -- segments in extents, for the plain store.** `onefile.rs`: the
  allocator, reuse, growth and the written-once mark, an extent's mapping,
  a landing's durability. Segments, ordered indexes and form files in the
  data file; the log and the manifest's slots still files; the manifest
  names extents (a store format of its own, chosen at the create and read
  at the open; no rewrite of an old store yet). `Options::one_file`, off;
  arm `supdb-onefile`.
  - Tests: every test that opens a plain store run under the option by a
    switch (`SUPDB_TEST_ONEFILE=1`, as `SUPDB_TEST_STRIDE`), in a group of
    `scripts/check.sh`; the differential fuzz with it drawn; the crash
    states built by hand -- a torn write over an extent's previous segment,
    a crash between a manifest and the reuse it licenses, a landing above
    the written-once mark cut before the file's sync, a reader pinned on a
    retired extent across its reuse; mutants of each rule, and mechanical
    flips beside the list.
  - Pairings: `pair.sh supdb-onefile supdb` at 300 thousand and three
    million keys in core over the row's passes; at ten million under 1.25
    GiB and 1 GiB, `load,lag` and the row's order; `discards.py` and
    `inv.sh` over the arm, the frees at nothing and the names' calls gone.
    Prediction from X1 and the probes, written here before the pairing.
- **O3 -- one file.** The log as extents (rotation, retirement, the first
  extent's pre-write), the slots at the head, ordered ingest's run as an
  extent with its marker in the log, `copy_to` and the old name's fate.
  The crash oracle, the kill tests and `tests/runlog.rs` under the option;
  the states a rotation and a run's close leave built by hand.
  Pairings as O2, with the durable and buffered loads' phases
  (`SUPDB_LOAD_PHASES`, `SUPDB_DRAIN_STEPS`).
- **O4 -- the sealing over extents.** Pagefile's S2-S3 with extents below
  it: Once, Regen and Regen-lazy for segments, indexes and form files, the
  tags apart, the pager's chunk a page; the log sealed as today, in its
  extents. Arm `supdb-cipheronefile` beside `supdb-cipherauto`, the
  envelope the default still. Pairings: against `supdb-cipherauto`, and
  against `rocksdb-tuned-enc` and `lmdb-enc` in the cells the roster loses
  (`ROADMAP.md`, what the encrypted roster says).
- **O5 -- the run its own log under a cipher, the controller on.**
  Pagefile's S4-S5 (the Log class, `finish`), E4's model first.
- **O6 -- the defaults.** Each flipped only where the suite reads it better
  and the gate passes with it on: the plain store's layout, the cipher's.
  An old store's rewrite at its first open, the docs, the arms of the
  shapes before.

Every stage inherits `scratch/plan/g1.md` "What every unit inherits" and
the rule this branch learned today: a lever on file operations is priced at
the device and in the cells after the phase it acts in.
