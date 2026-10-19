# Stage 3: scans through the compact form

The design is `scratch/design/zpages.md`, section 3 (the read paths: 3.4
the seek by the fence, 3.5 the scan, 3.6 values and extents, 3.7 the memo,
3.8 the overlay, 3.9 the sites, 3.10 the cursor), 6.2 (the arms) and 6.3
(the tests); stages 2a and 2b are what `src/db/zside.rs` holds today: the
point read and the count through a sidecar, its lifecycle, its filter and
its warm. This file is the cut of 3 into pieces, each a commit with its
tests, in the order they are built, and what each leaves for the next.
Stage 3 is the stage whose gate is `raw_touches == 0` on a covered
partition, for every shape of 3.9's table; stage 2's was not.

## Where 2b leaves the scans

Under `ZPages::Always` a scan of a covered partition is the raw form's,
counted `raw_scans` (`Seg::blob.scan_at` at `scan_partitions`, the cursor
of `scan_merged`, `values_at` in `emit_unsealed`), and the verdict of the
forced arm's sittings reads the scan cells as raw plus the z pages the
point reads touched. Three paths keep a position in a partition and ask
for its key again and again (3.10): `scan_partitions` seeks by the
ordered index (`seg.ord.seek(cursor, |r| seg.blob.key_at(r))`), walks by
`Blob::scan_at` in windows cut where the next unsealed key falls
(`BuildCtx::cut_at`: `seg.ord.seek_in` over the heads, then one
`key_at`), and emits an unsealed key's partition values by
`blob.values_at(rank, ..)`; `scan_merged` holds `pkey =
blob.key_at(prank)` across iterations, re-reads it after every emitted
key, and asks `blob.exts_at` for a piece's tombstone flag; the
block-cache machinery (`scan_blocks`, `walk_deltas`, `copy_from_deltas`,
`BoundsById`, `owner_of`) does the same over a block's ranks, and runs
only with forms, which the nocache arms never build. The raw form's
`key_at` is a directory word and a record read, and a covered partition
pays a raw page for each where the z form has the key in a decoded group
already.

## The pieces

1. **`ZSide::scan_at`**, the walk of 3.5: from a rank, the entry by
   `partition_point` over `first_rank`, the group through the thread's
   buffer (a first decode checks as the point read's does, and marks the
   page verified), the skip to the record by `record_key_next`, then each
   record parsed (`parse_record`) and its extents handed to
   `raw.read_exts` as the point read hands them, `seen` and `rank`
   advancing together, the next entry's group when a group is spent. An
   entry of `ZNOPAGE` hands the run to `raw.scan_at` for exactly its
   records, counted `nopage_scans`. The contract is `Blob::scan_at`'s:
   `limit` counts keys, a tombstone-only key counts and emits nothing, a
   rank past the end walks nothing, and the arithmetic saturates. Wired
   at the one site a drained store's scans take, `scan_partitions`'
   `seg.blob.scan_at`, through `Seg::scan_at_z(raw, rank, n, f)`: the
   cell's sidecar if installed and in service, else the raw walk counted
   `raw_scans` as today. Counters `scans` (walks served), `scan_pages`
   (groups decoded for them), `nopage_scans`. Tests: the unit test of
   `scan_at` against `Blob::scan_at` on the fixtures, every start rank
   and limit of 6.3's grid; `the_z_reader_agrees_with_the_blob_on_every_scan_window`
   through a `Db` (start in each fence key, its neighbours, the empty key,
   `zzz`, `0xff` x 8; limit in 1, 2, 3, 5, 17, 56, 57, 58, 114,
   `usize::MAX`) on the writer, a `Latest` and a `Snapshot` handle; the
   no-page test's scans across its records (`nopage_scans`); and the
   differential fuzz with the form forced in one store of the pair
   (`ZFUZZ_ZPAGES=1`: `opts` sets `zpages: Always`, `zpages_min_bytes: 0`
   on the store the knob names), run over the cache-off seeds, since a
   scan through the form against a scan through the raw form over the
   same writes is the strongest check a scan can have. Gate: every scan
   of a drained covered store counted `scans`, none `raw_scans`.

2. **The seek, the key and the cursor** (3.4, 3.10): `ZSide::seek(key)`
   by the fence and one group's walk, `ZSide::key_at(rank, f)` callback
   shaped, and `ZCursor` -- `cursor_seek`, `cursor_at`, `key()`,
   `rank()`, `values(raw, f)`, `with_exts(f)`, `advance()` -- a position
   in a partition that holds its group's buffer (taken from the thread's
   cell, put back at the drop) and steps by `record_key_next`, taking the
   next entry's group at a group's end and handing a `ZNOPAGE` entry to a
   raw sub-cursor for its records. `ZSide::cut_in(rank, end, uk)`, the
   answer `cut_at` gives, from the one group the fence says can hold `uk`
   and a walk of at most a group's records. Wired at every site of 3.9 the
   two scan paths have: `scan_partitions`' seek and `cut_at`,
   `emit_unsealed`'s `values_at`, `scan_merged`'s partition cursor (the
   key held across the advance copied into an owned buffer, 32 bytes, as
   3.10 says, the raw side keeping its borrow behind an enum dispatched
   once a partition), and the `exts_at` for flags. Each site is a
   `Seg::*_z` dispatch as in 1, raw when the cell is empty. Tests:
   `windows_that_cross_partitions_and_covered_with_uncovered_agree` (one
   partition without a sidecar, `hold_zpage_build` after `k`); a scan
   with an unsealed key inside the walk window (the `cut_at` shape) and
   one over a range with a level-0 piece (the `scan_merged` shape), each
   on the writer and a handle against a raw model; the fuzz as in 1 with
   its bursts and pieces, which reach `scan_merged` at every seed.

3. **`SegBlob`** (3.9): `Seg::blob` a newtype with no `Deref` and no
   forwarder for any name that has a z counterpart or a raw footprint --
   `key_at`, `lookup`, `read_all`, `read_all_by_value`, `count`, `scan`,
   `scan_at`, `values_at`, `exts_at`, `prefetch_*` -- forwarding only
   `keys`, `id`, `index_bytes`, `advise_random`, `advise_normal`; `raw()`
   the one door, which under `debug_assertions` counts `raw_touches` when
   the segment has a sidecar installed. The compiler then lists every
   site (about forty `key_at`, the rest the read methods), and each is
   decided: the z dispatch of 1 and 2, or `.raw()` on purpose with the
   reason beside it (the tombstone pre-pass over pieces, the pieces'
   cursors, the merges' and the open's own passes, `learn_prefix`,
   `same_blocks` sampled from the heads, `make_table`'s last key, the
   block-cache machinery the nocache arms never run). Test:
   `the_compact_form_touches_no_raw_page`, `raw_touches == 0` per
   operation kind and per shape of 3.9's table on a covered partition --
   a scan over a range with a piece, a scan with an unsealed key in the
   window, variable-length keys, a key straddling a `ZNOPAGE` entry, a
   landing's `same_blocks` -- on the writer and a `Latest` handle;
   `grep -n '\.raw()' src/db.rs` is the list of the deliberate reads.

4. **The memo** (3.7): a per-thread direct-mapped memo of decoded groups,
   1,024 entries capped at 8 MiB of decoded bytes a thread, keyed by
   `(side id, page)`, an `Rc` to each buffer so a re-entrant callback
   holds its group without a take-and-put; a hit skips the decode and
   still pays the in-group walk. `Options::zpages_memo` (on), the arm
   `supdb-nocachezpagesnomemo`. The cursor of 2 takes its groups through
   the memo. Tests: a hit rate asserted on a Zipfian read set over one
   partition (the memo's own counters `memo_hits`, `memo_misses`), the
   re-entrant callback, the cap held, a retired sidecar's entries never
   hit. Priced by mix C in the ooc pairing (7.3: 260-350k with it,
   185-230k without) and in core by the C mix at 300k, where it must not
   lose.

5. **The heads arm** (3.4): `Options::zpages_seek_ord`, the seek of a
   covered partition by the ordered index's heads as today instead of
   the fence, `supdb-nocachezpagesheads`; prices the seek by the fence.
   No test beyond the arm's own (the pair differs in the option alone).

6. **The prefetch** (3.5): a scan of `L` keys asks for the pages it will
   walk after the first with one `MADV_WILLNEED`, `k = min(ceil((L -
   1)/mean_count), 16)` pages, counted `prefetches`. The design gates it
   on the census's fault rate, which is stage 4b's; until then it is
   `Options::zpages_scan_prefetch` (off) and an arm
   (`supdb-nocachezpagesprefetch`), priced in the ooc pairing's scan
   cells and never issued in core.

7. **The tests of 6.3 that are the scan's**: `tests/db.rs`'s
   `scan_path_arms()` gains `("zpages", ..)` and `("nocachezpages", ..)`
   so a live write over the frozen table is one key on every scan path;
   the twins of the scan-model tests with `zpages: Always` (the nocache
   twin with `scan_block_cache: false`, so the merge and bulk-walk paths
   carry the form); `readers_on_threads_keep_answering_through_seals_and_merges_with_zpages`;
   `a_run_of_every_shape_reads_the_same` through the scan.

8. **The measurements**: `ab` in core at 300k and 3M with the scan passes
   and the mixes (`supdb-nocache` against `supdb-nocachezpages`), where
   the form is expected to lose the scans (the decode) and the mixes B,
   D, E to straddle; the ooc pairing at 10M under 1 GiB with
   `--passes load,read,read-threaded,scan,scan-threaded,mix` at
   `--ops-div 10`, two sittings with the arms swapped, read against 7.3's
   scan and mix cells; the memo and the heads arms each against the
   forced arm in one process. Every verdict in two sittings.

Each piece: its tests under `checked`, lint, the zpages binary's whole
run and the fuzz over its cache-off seeds with the form forced; the full
gate at the stage's end, and the lifecycle stress of 2b's stop rule
again, since the scans add a reader of every group. Done when 7.3's scan
and mix cells have a verdict in two sittings and `raw_touches == 0` holds
on every shape of 3.9's table.

## Where it stands

Piece 1 is in: `ZSide::scan_at` with `ZScan` (the keys visited, the
groups decoded, the no-page runs, and how it ended), `ZFence::entry_at_rank`,
`part_scan_z` at `scan_partitions`' walk with the raw walk for what the
sidecar cannot serve, the counts `scans`, `scan_groups`, `nopage_scans`
and `raw_scans` restated as the walks that went raw, the slot two lines
for them (pinned at 256 bytes). The fuzz takes `ZFUZZ_ZPAGES=1`, the form
forced in the `z` store of its pair with no size floor; forty seeds with
the cache off read clean at the commit, and the sweeps of a hundred and
twenty with the cache off and on are what the piece's gate adds. The tests of the
scan's windows run on the writer, a `Latest` handle and a `Snapshot`
handle, drained and with unsealed keys over the partitions; the walks of a
scan are counted by the partition they cross. The block cache's walks of a
covered partition stay raw and uncounted until piece 3 lists their sites.

## What 2b's sittings leave open, read before 3's

- The eager build's pairing (`eager-pairing.sh`, `eager-ab-1.out` and
  `-2`, two sittings with the arms swapped) priced the design's
  start-at-landing against the settle: the ordered load 1.33x and then
  1.21x for the eager arm, six pairs of six both times; the shuffled load
  0.995x and 1.015x, level, with 29-33 builds for 25 partitions (the
  merges' rewrites supersede 4-8) and a third more build bytes for the
  same files. The phases say why: at three million keys the ordered
  load's closing sync fell from about 1.6 s to 0.5 s and the commits rose
  about 0.2 s, the builds running beside them on the idle-class thread.
  The eager build is the default now and `supdb-nocachezpagessettle` the
  arm that leaves every build to the settle. The flip under the gate was
  green once and read four runs of twelve red beside a build: a sidecar
  the open refuses is built again by the segment work's first turn after
  the open under the eager build, and the tests of what an open leaves
  read that state while the build raced them; they open with the builds
  left to the settle now, the eager rebuild has a test of its own, and
  the stress of 2b's stop rule read twenty of twenty after it.
- The warm cannot be priced by `ab` as it stands: the forced arm's
  `sync` builds a sidecar and the warm after its attach finds it
  resident, so the warm's work is a restart's, and the runner has no
  pass that reopens a loaded store cold. A `reopen` pass -- the store
  closed, its files dropped with `posix_fadvise(DONTNEED)`, opened again
  and read -- is the instrument, and belongs to the runner before the
  pairing `supdb-nocachezpages,supdb-nocachezpagesnowarm` means anything.
