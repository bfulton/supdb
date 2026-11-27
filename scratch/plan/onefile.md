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

**Run, 2026-11-26** (`scratch/onefile/notes.md`): X1 resolved nothing in
core at one and two million keys, a store that frees nothing within a few
percent of `supdb` either way, so its kill clause holds: on this fleet the
container's own gain for the plain store is the segment's write and sync
and the names, and the cells the encrypted roster loses (`ROADMAP.md`: the
ordered load's double write and its envelope sealed whole at the close, a
form page across two chunks) are the sealing's, O4 and O5. X2 passed, one
inode costing three writers nothing measured. X3 read a cold fault about 3%
dearer through an extent of a large file, at the edge of its spread. X4 is
not built. The order below is therefore O1, O2 for the segments alone, O4
and O5 on top of it, and O3 (the log and the slots into the file, the one
file a store that mobile wants) after them.

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
  - **The prediction (written before any pairing of the arm).** In core,
    300 thousand and three million keys: every read, scan and mix cell
    inside the floor the default reads against itself (up to a fifth at
    these rungs); the loads and the lag bursts within a few percent
    either way, as X1's store that freed nothing read. A landing has no
    rename and no directory fsync of its own (0.6 ms by `prims.py`), which
    at these rungs is under what a pairing resolves. Under a bound at ten
    million keys: the full rewrite's burst ahead by what the segment work
    spent freeing (7.4 s of 26.6 traced, `inv.py`), 1.05x to 1.2x, with
    more merges landed in it and so the pass after it level or ahead by
    the pieces it meets; the clean store's pass and the read passes level,
    a cold fault 3% dearer at most (X3); the loads level, a block's first
    use 12 ms in 64 MB (`grow.py`) against a fresh file's names.
    `discards.py` over the arm: the frees at nothing but the runs' files
    and the log's.
  - **What would refute it, named before it is looked for.** (1) A file's
    own sync is an fdatasync of the data file, which writes back every
    dirty page of every file in flight: a seal's durable landing waiting
    for a merge's megabytes (`login.py`'s 189 ms, where X2's writers each
    synced their own). Read in the seals' waits (`seal_waits`) and the
    drain's stamps in both arms before the rates. (2) Every mapping is
    made through the table's one open of the data file, so the kernel's
    readahead state and its count of mapping misses are one for the
    store, where a store of files has one a segment and X3 mapped its
    extent through an open of its own: read in the major faults a read
    and a scan of the bounded passes, and the lever, an open a mapping,
    priced only if they are over. (3) Writes into reserved blocks convert
    their extents at each writeback's end (the kernel's log said so 136
    times over one test run): read in the loads' sync phases. (4) Room
    retired is not dropped from the page cache when it is given back, so
    under a bound a retired segment's pages stand until reclaim finds
    them, where an unlink drops a file's at once: read in the group's
    cache by the pass (`groupmem.py`), and the lever is a drop of the
    range where its last holder goes.
  - **In core, as read (2026-11-26, the tree before O2's commit over
    `911256e`).** `sh scripts/pair.sh supdb-onefile supdb 300000` and
    `... 3000000 --reps 6`, in `bench/`: no cell on the one side in both
    orders with its pairs behind it. At 300 thousand keys whichever arm
    was listed second read ahead on the shuffled load (1.08x, 1.05x) and
    on the pass after the full rewrite (1.08x in both). At three million
    the cells that leaned the same way twice were the full rewrite's
    burst, 1.06x for the store in one file in both orders (four pairs of
    six and three), and B, the scrambled D and the scrambled F against it
    by 3-10% (one to three pairs of six), each inside what the default
    reads against itself; the one-percent burst read 0.87x in six pairs
    of six in one order and 1.13x in four of six in the other. The
    ordered load's bytes on disk are 2.5% more in every pair, the data
    file's reserved end. As predicted: nothing in core.
  - **Under 1.25 GiB at ten million keys, as read, and the fourth
    refutation taken.** `sh scripts/pair.sh SUPDB_LAG_COUNTERS=1
    SUPDB_SCAN_OPS=1 supdb-onefile supdb 10000000 --bound 1342177280
    --passes load,lag --reps 2 --ops-div 10`, the tree before the drop
    below. The full rewrite's burst 1.24x and 1.23x for the store in one
    file in every pair, the prediction's upper end. And the passes
    refuted the rest of it: after the full rewrite 0.36x and 0.28x, the
    clean store's 0.66x and 0.72x, every pair, with the pass after the
    rewrite refaulting 110 thousand pages against 25, taking 9,060 major
    faults against none and reading 430 MB at the device against 98
    (`lagdev`). Refutation (4) as named: room a merge retires kept its
    pages in the page cache, written once and read by nobody, which is
    how the kernel ranks the files written since, so a reclaim took
    those. The pages are dropped where the last holder of the room lets
    go, before the room is free (`One::dropped`). The same command
    again: the burst 1.19x and 1.18x in every pair, the pass after the
    rewrite 0.98x and 0.84x (one pair of two, none of two), the clean
    store's 1.03x and 0.91x, the sweep's weighted sum 0.86x and 0.98x of
    the default's time. Read again at three reps and under 1 GiB, with
    the row's passes, below.
  - **Three reps, both bounds, and the second refutation taken.** With
    the drop (`... --reps 3`, and the row's passes `--passes
    load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled`,
    under `--bound 1342177280` and `1073741824`): the full rewrite's
    burst 1.20x and 1.18x under 1.25 GiB and 1.22x and 1.21x under 1
    GiB, every pair of twelve; and under 1.25 GiB the pass after it
    still 0.52x and 0.74x, no pair of six, with the scanning thread
    taking 2,700 to 4,900 major faults in five reps of six where the
    default's took none to 430, a twentieth of a second more on every
    tenth of the pass; under 1 GiB the one-thread read pass after the
    load 0.95x and 0.87x, no pair of six. Refutation (2) as named: every
    mapping was made through the table's one open of the data file, so
    the kernel's readahead window and its count of a mapping's misses,
    which stops the reading around a fault past a hundred, were one for
    the store. The comment over the call said an open of the mapping's
    own, as X3's probe had mapped; the code did not. A mapping is made
    through its own open now (`Using::map`). The same pairings again:
    the scanning thread's major faults in that pass 70 to 520 against 0
    to 500, the pass 1.14x (three pairs of three) and 0.92x (one of
    three), every burst of the sweep ahead in both orders (1.22x and
    1.23x, 1.14x and 1.10x, 1.14x and 1.24x), the sweep's weighted sum
    1.00x and 0.79x of the default's time; and under 1 GiB the
    one-thread read pass 1.00x and 0.99x. What still leans there after
    three reps: the three-thread read pass 0.91x in both orders (one
    pair of six) and the one-thread scan pass 0.90x and 0.95x (one of
    six), read again at six reps below; C in two shapes by the hot-raw
    policy's release, in both arms.
  - **Six reps of the read and scan passes under 1 GiB.** The same
    command with `--passes load,read,read-threaded,scan,scan-threaded
    --reps 6`: the three-thread read pass 0.94x in both orders, behind in
    ten pairs of twelve; the one-thread read pass level, at 1.14x to
    1.17x the default's refaults; the scan passes level. Not explained,
    and open in `ROADMAP.md` with what else leans.
  - **The controller's tests, and a read the manifest made.** The
    form-file tests' controller stores had not taken the tests' switch
    (`CLAUDE.md`, a switch's first run). Under it one test of eighteen
    was the engine's: a manifest that listed a file read the file's first
    page back for the row's checksum, a device read a file for a form
    file built ahead and dropped as it is written, on the segment work's
    thread, the page left in the cache. The table keeps a file's first
    page as its writes make it (`onefile::Head`). The pairings above are
    of the tree before it; at ten million keys it is a page for each of
    the builds ahead a load makes, two dozen.
  - **The row under 1.25 GiB at the stage's commit** (`b22446f`, the
    command above with the row's passes, three reps): the shuffled load
    1.06x and 1.03x in six pairs of six, and no other cell on one side in
    both orders; the scrambled F, behind in six pairs of six before each
    mapping had an open of its own, 0.94x and 1.15x.
- **O3 -- one file.** The log as extents (rotation, retirement, the first
  extent's pre-write), the slots at the head, ordered ingest's run as an
  extent with its marker in the log, `copy_to` and the old name's fate.
  The crash oracle, the kill tests and `tests/runlog.rs` under the option;
  the states a rotation and a run's close leave built by hand.
  Pairings as O2, with the durable and buffered loads' phases
  (`SUPDB_LOAD_PHASES`, `SUPDB_DRAIN_STEPS`).
- **O4 -- a store under a cipher kept in one file.** Built in the least
  form its cells call for (the owner: "Try to stay focused and
  efficient"): the envelope as it is -- header, chunks of 4,080 bytes
  with their tags inline, the pager over it unchanged -- and each
  envelope a file of the vault's own table in `store.one` under the name
  it had in the directory (`x-<token>`, `.o`, `.z`). The manifest is the
  one-file format's with the table's section behind it, the whole sealed
  as a slot is; the log, the slots, the marker and the lock are files of
  the directory still. A segment's envelope is synced through its own
  handle, which answers its writes' errors, and its index's envelope is
  written ahead of it with no sync and rides that one
  (`Vault::persist_with`, `ord_envelope`): a segment and its index under
  one flush (`CLAUDE.md`, one sync for a call's envelopes).
  What Pagefile's S2-S3 drew larger -- chunks of whole pages with the
  tags apart, the classes -- waits for the stage that needs it: O5, where
  a run sealed as it is written cannot have a header that holds its
  length. Arms `supdb-cipheronefile` beside `supdb-cipher`,
  `supdb-cipherautoonefile` beside `supdb-cipherauto`,
  `supdb-ingestcipherautoonefile` buffered.
  - **Predicted, before the first pairing** (`supdb-cipherautoonefile`
    against `supdb-cipherauto`, ten million keys under 1.25 GiB,
    `--passes load,lag`, then the row's passes). The mechanisms that
    differ: no inode is freed, so the segment work's 5.4 s of 25 in
    frees through the full rewrite (`CLAUDE.md`, a cost moved off the
    threads that paid it) is not paid; no envelope has a name of the
    directory's, so a landing makes no directory fsync; an envelope's
    sync is a sync of the data file, which covers whatever else is dirty
    in it; the page cache drops a retired envelope's pages at the room's
    last holder and not at an unlink; every read is the pager's, through
    a mapping of the envelope's range made through an open of its own.
    So: the full rewrite's burst ahead, 1.05x to 1.2x (the plain store
    read 1.2x; under a cipher the merges' decrypt and seal are more of
    the burst and the frees less); the other bursts level to a few
    percent ahead; the loads level to 1.05x; every scan pass level, the
    cache and the chunks being the same. What would refute it: (1) the
    pass after the full rewrite behind, with more pieces standing at it
    -- landings later, each sync waiting on other merges' dirty
    envelopes in the one file; (2) a pass behind with more major faults
    on the scanning thread and the same pieces -- the kernel's readahead
    or the drop of a range behaving differently over a range of one file
    than over a file; (3) a burst behind -- the reservation's unwritten
    extents converted at each sync (`ROADMAP.md`, what leans); (4) the
    group killed -- a memory file and its envelope's pages in the data
    file both standing where the files layout had dropped one.
  - **Read: an envelope written with no word of its size.** The first
    pairing (`SUPDB_LAG_COUNTERS=1 sh scripts/pair.sh
    supdb-cipherautoonefile supdb-cipherauto 10000000 --bound 1342177280
    --passes load,lag --reps 2 --ops-div 10`, at `c624353`) read the
    loads' device bytes a byte stored 1.3x to 1.5x the default's and the
    full rewrite's burst level, neither predicted: an envelope was
    written into the table with nothing said of its length, so it was
    given a step of room and grown or moved, its bytes copied, as its
    megabyte pieces came. The header knows the stored length before the
    first byte, and says it now (`Vault::write_envelope_at`, `One::expect`;
    `an_envelope_is_given_the_room_it_fills`). The same command at
    `e9e63f3`: device bytes a byte stored 3.1 against 3.1, no file moved
    or grown in either sitting (`one_moves`, `one_grown`), the ordered
    load 1.06x and 0.97x, the shuffled load 1.16x and 1.24x in four pairs
    of four, the full rewrite's burst 1.17x and 1.24x in four pairs of
    four -- the prediction's top.
  - **Read: the first three lag points are a coin, in both arms.** The
    clean store's pass and the passes after the one- and ten-percent
    bursts read up to twice apart between reps of one arm, and a table
    of two reps a sitting read them for or against by which reps it
    held. By the pass's own counts (`lagcounters`, `groups`) each rep is
    served by stride files (no group decoded) or by block files (eighty
    thousand groups a pass), by whether the store in the stride form
    fitted the conversion's room at the load's end (`zconvert room`,
    980 to 1,010 MB against a block form of 842): the default in the
    stride form in six reps of ten and the one-file arm in three, and
    shape by shape the arms level (the pass after the one-percent burst
    about 120 ms in the stride form and 205 in the block form, in
    either arm).
  - **Read: the pass after the full rewrite behind, refuted as (1)
    named it and not by its mechanism.** 0.43x and 0.32x, no pair of
    four, the scanning thread at 7,800 to 17,000 major faults against
    900 to 6,000, with the pieces standing the same (21 to 37 against
    26 to 37). Over eleven reps an arm (the pairings and three traced
    reps) the pass's time is one line through the megabytes the device
    is written inside it, in both arms: 684 ms and 1.78 ms a megabyte,
    the arms' mean residuals 45 ms either side of it; the default
    writes 85 to 290 MB there in ten reps of eleven, a last seal's
    envelope, and read 1.5 s in the one rep it wrote 500; the one-file
    arm writes 650 to 1,270 in its seven reps at `e9e63f3`. A merge trace of one rep an arm
    (`SUPDB_MERGE_TRACE=1 SUPDB_LAG_MARK=1 SUPDB_LAG_COUNTERS=1 bench ab
    --arms supdb-cipherautoonefile,supdb-cipherauto --size 10000000
    --bound 1342177280 --passes lag --reps 1 --ops-div 10`, each
    `mergejob` laid to the `lagmark` windows) says whose megabytes: the
    merges retire the same input a second in both arms, 127 and 132 MB a
    second of a merge's wall and 278 and 281 a second of its CPU, three
    in flight for most of the burst in each; the one-file burst is
    18.1 s against 22.2, so 4.5 GB is merged inside it against 5.35, and
    eight merges of 600 MB end inside the pass where the default has one
    of 75. The writer is 5 s of CPU in either burst and waits for the
    rest on its commits' fdatasyncs of the log, which is where the frees
    the one file does not make come back as a faster burst. So the
    layout costs the pass nothing a count shows, and the burst it makes
    faster ends a gigabyte of merging earlier in the debt. What the
    merges wait on, half their wall in their value pass in both arms, is
    the next reading (`ROADMAP.md`).
  - **Read: the row's passes, and the read pass after the load.** The
    row's order at `e9e63f3` (`sh scripts/pair.sh supdb-cipherautoonefile
    supdb-cipherauto 10000000 --bound 1342177280 --passes
    load,read,read-threaded,scan,scan-threaded,mixes,scan-mixed,mixes-scrambled
    --reps 2 --ops-div 10`): both arms end in the stride form, the
    shuffled load 1.08x and 1.26x in four pairs of four, the scrambled D
    and E ahead in both orders, and most other cells with whichever arm
    was listed second; the one-thread read pass after the load 0.64x and
    1.05x, at 1.7x the major faults a read in both orders. Six reps an
    order of that pass alone (`SUPDB_ZPAGES_TRACE=1 SUPDB_LAG_MARK=1 ...
    --passes load,read --reps 6 --pairs "read "`): 0.87x in six pairs of
    six, then 1.03x in three of six; over the twelve reps an arm the
    medians 187 and 203 thousand reads a second, ahead in three pairs of
    twelve. Not resolved, and what the traces say of the pass is both
    arms': the crossing asks 380 to 570 MB of form files back a quarter
    second before the reads begin or half a second after (six reps of
    twelve after for this arm, three for the default), the form is
    installed 0.55 to 1.1 s into the pass, a thousand reads made before
    it, and the last two form files, 64 MB each, take 2 to 4.6 s of wall
    for 0.3 s of CPU to build inside the pass, their thread 70% of its
    samples in a page's read from the device
    (`filemap_read_folio`) behind that read-back, while the reads of
    their partitions fault on the raw form. Every major fault of one
    rep's pass laid to its file (`perf record -a -e major-faults -c 1 -d
    -k CLOCK_MONOTONIC`, the envelopes by `pagerback`'s place) is the
    same in both arms: 20 thousand on form files, 17 thousand on the
    load's pieces. One thing was the layout's: every thread's state
    sampled fifty times a second (`/proc/<pid>/task/*/stat` and `stack`)
    had the builder 0.72 s of one pass in `fallocate`, waiting for block
    bitmaps behind the same read-back -- the data file made longer, 80
    times a row, since the eighth it grows by was held to 64 MiB. Held
    to a gibibyte (`onefile::AHEAD_MOST`): 46 growths a row, and none,
    none and 0.02 s in the allocator over three reps' read passes. The
    pass's rate with it is not paired yet.
- **O5 -- the run its own log under a cipher, the controller on.**
  Pagefile's S4-S5 (the Log class, `finish`), E4's model first; and with
  it the sealing O4 left as it was: chunks of whole pages, the tags
  apart, sealed as the file is written.
- **O6 -- the defaults.** Each flipped only where the suite reads it better
  and the gate passes with it on: the plain store's layout, the cipher's.
  An old store's rewrite at its first open, the docs, the arms of the
  shapes before.

Every stage inherits `scratch/plan/g1.md` "What every unit inherits" and
the rule this branch learned today: a lever on file operations is priced at
the device and in the cells after the phase it acts in.
