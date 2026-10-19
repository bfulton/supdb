# Stage 2b: the compact form's lifecycle

The design is `scratch/design/zpages.md`, sections 5 (lifecycle), 2.5 and
2.7 (binding and integrity), 6.2 (arms) and 6.3 (tests); stage 2a is what
`src/db/zside.rs` and `Db::zpages_settle` hold today. This file is the
cut of 2b into pieces, each a commit with its test, in the order they are
built, and what each leaves for the next. Nothing here is timed: the one
pairing 2b adds is the warm arm's, written here and run when the machine
is free.

## Where 2a stands against 5.x

| design | 2a | 2b |
|---|---|---|
| sidecar named `(id, end_seq)`, claimed by the same test as the ordered index (5.5, 5.6) | yes | -- |
| attach at open under `Always`, dormant under `Off` | yes; `Off` keeps a live segment's file (the reopen test holds it: a store that ran without the option may run with it again), where 5.5 says `Off` removes. Kept as 2a has it: removal is the reversal of `Auto` (4.8), stage 4's | -- |
| sweep at open of every `zpg-` file no live segment claims, temp names included (5.5, 5.7) | yes | dead names join the sweep |
| the build on the writer's thread inside `zpages_settle` | yes | a thread of its own (piece 4) |
| retire on merge (5.6) | the sidecar is unlinked inline in `retire_seg`, a freed inode on the landing's path | renamed to a dead name, unlinked at the idle tick (piece 2) |
| a page that fails its first decode (2.7) | fallback counted, the page marked, the sidecar out of service for the segment's life | retired through the epoch, renamed dead, rebuilt at most three times a segment (pieces 1, 3) |
| a failed build (5.7) | counted, the segment declined for its life | retried after a backoff of 60 s doubling to an hour (piece 3) |
| the cell (4.8) | set once, freed with the segment | clearable: swapped to null and pushed on a retire list swept past every pinned reader (piece 1) |
| `ZNOPAGE` entries (2.1, 2.7) | the layout has them and the reader reads them raw; the builder declines the partition instead | the builder writes them (piece 6) |
| the Bloom filter and `bloom_sig` (2.5) | the trailer has the fields; none is built or read | built, checked at attach, read for absent keys (piece 7) |
| warm (5.4) | none | a job after the build, `Options::zpages_warm`, the arm `supdb-nocachezpagesnowarm` (piece 8) |
| the crash windows (5.7) | the sweep covers the temp name, the unnamed segment's sidecar and the orphan; the attach refuses a torn file | every row of the table built on disk and opened (piece 5) |
| `a_zpages_life` in `tests/leak.rs` | none | piece 9 |

## Where it stands

Pieces 1 to 5 are in (`tests/zpages.rs`: the damaged sidecar retired
and rebuilt three times then declined, the retired mapping kept by a
pinned handle and freed at its release, the failed build retried after
its backoff, the crash windows of 5.7 built on disk and opened, a build
in flight cancelled by the merge that takes its target, and a build
past its last cancel check whose target died leaving a dead name). The
builds run inside the settle of the form, where the pairing measured
them; `Options::zpages_eager`, the design's start-at-landing, is an
option to be priced before it is the default. Piece 6 is in: a record
no page holds is a fence entry with no page and the build goes on, a
partition of nothing else is declined for the last record's reason,
and a key in such an entry's range that is not the record's is absent
from the fence (the entry covers one record; one over more is the raw
form's for every key). The test's keys of 200, 1,000 and 5,000 bytes
all fit a page compressed, so the records no page holds are a 9,000-byte
value of random bytes and a 70,000-byte value, over `ZMAX_ULEN`, under
`inline_bytes` of 128 KiB. Piece 7 is in: the build inserts every key
into a `BlockedBloom` as it passes the record, the words go into region
B with the fence and the parameters into the trailer, the attach holds
the stored filter to the engine by signature, by size for the key count
and by every fence key testing present, and the locate asks the filter
before the fence. The words have one encoding (`BlockedBloom::words`,
`from_words`), which the layout's pin test hashes. The reversed-filter
test fails the attach on a fence key; the absent-key decode counts fall
to the filter's false positives. Piece 8 is in: a warm is a job of the
builder's thread (`ZJob::Warm` beside `ZJob::Build`, one at a time),
scheduled by `z_next` for an attached, undamaged sidecar the process has
not warmed, marked before the thread starts; inline, the settle warms on
its own thread. `warm_file` reads the file in 8 MiB chunks, skipping a
chunk `mincore` reports wholly resident, `MADV_WILLNEED` then a touch of
each page, so the job's time is the read's and the pages are there when
it returns; the headroom check waits for the bound (stage 3), so the
file is read whole. The test holds the exact identity the mechanism
gives: the warm reads every page the attach's own reads left cold and
no other, on a partition large enough that the attach's sampled pages,
read before the mapping is advised random, do not pull the whole file
in by readahead (a 280 KB sidecar came in 67 pages of 69 that way). The
arms `supdb-nocachezpagesnowarm` and `supdb-nocachezpageseager` are in
the suite, unpriced; the pairing for a free machine is the one written
above. 9 is next.

## The pieces

1. **The cell retires through the epoch.** `ZCell::take` swaps the
   pointer to null and the caller pushes it on `Shared::retired_z`, a
   `RetireList<RetiredZ>` under a fresh epoch; `RetiredZ`'s `Drop` frees
   the sidecar (the mapping with it). Swept where the forms and the
   snapshots are (`sweep_retired_forms`, the writer's `leave`, the
   segment work's tick), and counted in `sweep_at_leave`'s number
   waiting. `ZCell::get` is then valid for the operation that read it,
   which pins; `Seg::z` keeps its acquire load. Test: a `Snapshot` handle
   pinned before a retirement keeps reading the old sidecar's values and
   the mapping stays in `/proc/self/maps` until the handle releases, and
   goes then and not at the next retirement.
2. **Dead names.** `retire_seg` renames an unclaimed sidecar to
   `zpg-dead-<N>.zpg` and queues it on `Maint::spares`, unlinked at the
   idle tick as the manifest spares are; inline (no thread) it unlinks
   as before. `is_zpg_file` covers the dead names, so the sweep at open
   and at the end of a settle removes what a crash left. The merge test
   asserts, after the merges land and before any settle: no live-named
   sidecar of a retired segment, dead names at most, and none after the
   idle tick.
3. **Damage and failure are bounded, not final.** A sidecar out of
   service is retired by the next settle round (piece 1), its file
   renamed dead, and the partition rebuilt: at most three times a
   segment (`Seg::z_rebuilds`), after which `declined_damage` and the
   partition stays raw. A failed build is retried after a backoff
   (`Seg::z_retry_at`: 60 s doubling to an hour), not declined for the
   segment's life; a test's `fail_next_zpage_build` stays. Counters:
   `rebuilds`, `declined_damage`, `build_retries`.
4. **The builder on a thread of its own.** `supdb-zbuild`, idle I/O and
   CPU priority, one job at a time, a cancel flag checked once a page and
   before the rename; started and collected by `Maint::z_step` on the
   segment work's loop (after `shape_if_read`, before `sweep_spares`),
   never joined there; cancelled by `join_compact` and `join_tier` for a
   target among their inputs, and by `stop`. The install on the segment
   work's thread: liveness by `Arc::ptr_eq` or `(id, end_seq)` plus the
   binding, attach, `install`. A finished build whose target is gone is
   renamed dead and counted `build_orphaned`. The segment work hands the
   builder the open `File`, so a promotion between spawn and open does
   not fail it. `zpages_settle` under `Always` waits for the builder's
   rounds instead of building; the inline home (`MaintHome::Here`)
   builds as 2a did. `hold_zpage_build` holds the thread before its
   first write, for the tests that need a build in flight.
5. **The crash windows of 5.7**, each built on disk from a store that
   has a sidecar, then opened: a `.tmp`, a complete unattached file, a
   short file, a zero trailer, zero pages, a torn page in the middle, a
   finished orphan under a dead name, an unnamed `.sup` with its sidecar,
   a promotion's two names, a re-handed id. Each asserts what the open
   leaves and what the reads answer, and that the counters name it.
6. **`ZNOPAGE`.** The builder writes a fence entry with no page for a
   record no page holds (oversize, incompressible past `STORED`,
   unplaceable) and goes on; the reader reads those records raw and
   counts `nopage_reads`. The partition is declined only when every
   record would be such an entry. Test: keys of 200, 1,000, 5,000 bytes
   and a 9,000-byte value among compressible neighbours.
7. **The filter.** Built at the build (`BlockedBloom` over the keys),
   stored in region B with `bloom_sig`, `bloom_probes` and
   `bloom_bits_per_key` in the trailer; at attach the signature and the
   parameters must equal the engine's and every fence key must test
   positive; a point read of a key the filter rejects is answered absent
   without a page. `BLOOM_SIG` is pinned by a unit test over 4,096 fixed
   keys. Test: a permuted filter, an altered signature, each refused
   (`rejected_identity`) and never answered absent.
8. **Warm.** After the rename, on the build thread, `MADV_WILLNEED` in
   8 MiB chunks skipping what `mincore` reports resident, within the
   headroom the bound leaves (`Options::zpages_warm`, on). The arm
   `supdb-nocachezpagesnowarm` (6.2's sites s1-s15 as the arm needs
   them), and its pairing written down for a free machine: `bench ab
   --arms supdb-nocachezpages,supdb-nocachezpagesnowarm --size 10000000
   --bound 1073741824 --ops-div 10 --reps 6 --passes load,read`.
9. **`a_zpages_life`** in `tests/leak.rs`: a store with the form, a
   merge replacing a sidecar, a handle holding a retired one, closed with
   one unswept; nothing allocated outlives the drop and no mapping of
   the directory survives it.

Each piece: its tests under `checked`, lint, and the zpages binary's
whole run; the full gate at the stage's end. The oracle and the
scan-model twins of 2a run at every piece. Done when 8's table is met:
the store survives every crash window of 5.7 and `a_zpages_life` leaks
nothing.
