# The browser reader

A supdb reader that runs in a browser, over an immutable index object fetched
out of object storage. Reader only: there is no writer here and there is not
meant to be.

```sh
web/build.sh ci        # build the module, measure it, record the size
web/test/run.sh        # build a real day index and read it in Chromium
```

| path | what |
|---|---|
| `supdb.mjs` | the library: three byte sources, one synchronous API |
| `cache.mjs` | the caching byte source over ranged HTTP -- budget, pages, eviction |
| `s3.mjs` | a minimal SigV4 range fetcher, the S3 adapter for `cache.mjs` |
| `worker.mjs` | the Web Worker the reader runs in, and why it has to |
| `build.sh` | builds the module and the floor, records `w3-bundle` |
| `floor/` | an empty cdylib with the same std surface -- the size control |
| `test/` | two real index files, a real browser, OPFS and ranged HTTP; `test/node.mjs` runs the error paths in Node, where the browser suite only walks the happy path |

## The one decision everything else follows from

`flatindex::lookup` returns a borrow into the index section. A borrow cannot
survive an `await`. So either the byte source is synchronous, or the reader API
turns inside out.

A browser byte fetch is asynchronous, and there were three ways out. The one
taken is **OPFS**: JavaScript downloads the object into the Origin Private File
System once, asynchronously, and every read after that goes through
`FileSystemSyncAccessHandle.read(buf, {at})`, which is synchronous. Nothing in
the Rust API changed shape.

That is only viable if a day's index can be downloaded whole, so that was
settled first and with a number: `w1-daysize` measures a day index at 36.14
bytes per log line over a 580 KB fixed cost, which puts a 32 MB download budget
at **912,522 log lines per day** at seven indexed fields. Above that, shard the
day -- logshed already writes one immutable object per sealed period, so a
10M-line day is eleven objects, each under budget and each skippable by a query
with a time range.

The cost of OPFS is that sync access handles only exist inside a Web Worker.
That is why `worker.mjs` exists and why it is not an implementation detail.

## Reading without downloading (R6)

Downloading whole was the right call for getting a reader working; it is not
the right long-run answer, because most queries touch a sliver of the object.
The third byte source keeps the object in object storage and holds only the
parts queries touch, under a byte budget -- and it changes nothing about the
synchronous-reader decision above, because of one observation: **a lookup is
already a plan.** `lookup` consults the key index and returns extents; the
block table maps extents to byte ranges; both sections are resident after
open. So the module can name every byte a query will read *before* reading
any of it, JavaScript fetches those ranges asynchronously, and the read then
runs synchronously and cannot miss. The `await` lives in JS; nothing inside
wasm suspends, so no Asyncify rewrite, no JSPI, no size cost against R3.3.

Three ABI calls carry the plans (framed as `u32 n`, then `n` pairs of
`u32 off, u32 len` -- absolute file offsets, always):

- `supdb_open_probe()` -- how many leading bytes the open needs first;
- `supdb_open_plan(head, len, object_len)` -- the ranges the open will read:
  the probe itself, the key index, the block table, and -- only for a store
  that was never cleanly closed and so still carries a log arena -- the redo
  log's emptiness word. Format knowledge stays in Rust; a superblock constant
  hand-copied into another module has drifted once already;
- `supdb_ranges(h, keys, len)` -- the data ranges a read of these keys will
  touch, deduped and merged. Input is `u32 nkeys`, then per key `u32 klen`
  plus bytes.

The granularity of a data plan is the **stored block**, not the extent: the
read path fetches whole blocks (verification and decompression want the
enclosing bytes), so an extent-granular plan would under-report. That the
plan is *exactly* what a read touches -- no more, no less -- is the property
the whole design rests on, so it is asserted with recorded reads rather than
argued: `tests/ranges.rs` natively, `w4-ranges` in `results/`, and the
browser test end to end.

`cache.mjs` is the byte source: sparse pages in OPFS (it survives reloads),
a budget in bytes that is a real file size, CLOCK eviction, and a hard rule
that the sync read path never fetches -- a read outside what `ensure()`
fetched throws `SupdbCacheMiss`, because zero-filling a miss would be the
browser quietly answering a different question, this project's least
favourite failure mode. Pages are 64 KiB: supdb seals blocks at 64 KiB so
one page usually holds one block read, and S3 range-GET economics are about
request count, not page size -- `ensure` coalesces adjacent missing pages
into one request. The fetcher is a parameter, `(offset, length) ->
{bytes, total}`; `httpRangeFetcher` covers anything that honours `Range`,
and `s3.mjs` supplies SigV4 on top for S3 itself. Credentials, the bucket
and which object to open stay with the caller, and are never persisted.

Counts stay free, for any schema since format v5: every extent carries its
record count, so `count` and `scanCounts` are sums over the resident extent
lists exactly as `countFixed` and `scanCountsFixed` are arithmetic on them,
and a browser ranks a segment's whole term dictionary over ranged HTTP with
*zero* fetches after open -- recorded as W4.2 on the network axis, and as
W2.5 (4.5 ns a key against the fixed form's 5.2) on the CPU axis.

`readConcat(key)` returns a key's values back to back in one buffer with
the count, one boundary crossing and one copy per key where `lookup` frames
one view per record; a common trigram has hundreds of thousands of
postings, and fixed-width ones come back as one typed array.

Reads of small keys are free too, for a segment the next engine wrote: a run
of values up to 256 bytes lives inside its index record (`Ext::INLINE`), so
`readAll` on such a key is answered from the resident index and `ranges_for`
plans nothing. Only runs longer than that reach the data by plan.

**The premise, and when it expires.** All of this splits the object into
"index and block table, fetched whole at open" and "data, fetched by plan".
That split costs approximately nothing today because logshed's key
cardinality is bounded by its field schema -- a real segment is ~100 keys
and single-digit kilobytes of index over megabytes of postings, so the open
fetches a few pages and everything after is sparse (`w4-ranges` prices the
open at under 20 KB of a 31 MB object). It stops being cheap the day the
keys are unbounded: a trigram or free-text index has a dictionary that grows
with the data, and would need the *index* planned and fetched sparsely too.
That day has a reader now (R6.3, below); it arrived as a second open and a
handful of range calls, and the point-read reader above is unchanged.

## Reading the dictionary by range (R6.3)

`openSparse(wasm, cache)` never fetches the key index whole. It keeps the
section's 192-byte header and its fence -- the sampled keys `seek` already
binary-searches, kilobytes for an index of any size -- and reaches the
directory and the records by plan, exactly as data is reached by plan
above. The open is three small round trips (`supdb_open_sparse_plan(1)`:
superblock probe, index header, block table; `(2)`: the fence the header
locates; then `supdb_open_host_sparse`). A range of the dictionary is two
more: `supdb_dict_plan(lo, hi, 1)` names the directory slice for the
fence-bounded ranks, `(2)` names the record bytes those entries point at
-- one contiguous span, because both writers lay records out in key order
-- and `supdb_dict_counts(lo, hi)` walks it. `SparseReader.ensureDict`
does the two awaits; `dictCounts` then runs synchronously and cannot miss.
Values follow the same shape: the walk hands out each key's extents,
`dictRanges`/`ensureDictValues` plan and fetch the blocks behind them, and
`dictReadConcat(key)` reads one key's values through them. A sparse
reader refuses the point-read calls by name; a whole reader refuses the
range calls. Neither answers empty for a question it cannot see.

The plans are exact, on the recorded reads (`tests/dict.rs` natively over
135 ranges per index shape, `w5-dict` on the day index, and the browser
suite over ranged HTTP: three ranges fetch exactly the pages their plans
name that nothing before made resident). What `w5-dict` also says is that
at logshed's current dictionary sizes the 64 KiB page is the unit that
matters: the sparse open is 23,808 bytes but four pages, 279,856 against
the whole open's 869,680 for a 686 KB index (W5.1, recorded as failing
its 5% prediction; it crosses 5% near 5.6 MB of index), and a 210-key
field's two plans are 9,860 bytes but three pages (W5.2). Ranking a field
from the sparse reader costs 10 ns a key (W5.4). So the sparse reader's
cache opens at 16 KiB pages (`openSparse` through the worker passes
`pageSize`): the open is then 70,960 bytes, 8.8% of the whole open, and
a field's range sits at 0.59 of its share plus four pages (W5.5, W5.6);
`ensure` coalesces adjacent pages into one request, so a block read costs
the same number of requests at either page.

## What a lookup costs

A point lookup touches the key index section, one or two hash slots, one
record, and one block. The index and block table sections are read once at
open, because a source that cannot lend its bytes -- which an OPFS handle
cannot -- should pay per section rather than per lookup.

`count(key)` and `countFixed(key, width)` are two different things and the
difference is 28x. See `f28-count` and W2.1-W2.4 in `claims.json`: an `Ext`
records block, offset, byte length and the offset of the last record, and none
of those is a count, so the general count walks the values -- and walking them
is *not* cheaper than reading them, which is W2.1 and is recorded as failing.
A *fixed-width* posting list does not need to walk: its count is arithmetic on
`Ext::len`, checked against `Ext::last`, with no block touched. logshed's
postings are four-byte line ordinals, so `countFixed` is the call it makes.

Before format v5 the same was true of `scanCounts` versus `scanCountsFixed`,
by a factor of 283: the general form walked every posting in the range. The
count now lives in the extent record, both forms are O(extents), and the
general one is the faster of the two (W2.4 records the flip, W2.5 the new
bound): a whole day's term dictionary ranks in about 9 microseconds whatever
the value width. That is the answer to "does the browser need a scan, or
should the roll precompute the panels": it needs a scan, and precomputing buys
nothing.

## Round trips (R7)

`openSparse(wasm, cache, { probe, directory })`. A segment's superblock
page carries an extension -- a copy of the key header and the offsets of
the fence, directory, hash region and checksum row -- so the open's first
plan names everything it needs and the second plan is empty: two round
trips from a page-sized probe, where a store's open is three. A segment
written with a head reserve (`SegmentWriter::set_head_reserve`) keeps its
block table, checksum row, a copy of the fence and a copy of the directory
right after the superblock page; pass `probe: 4096 + reserve` and the open
is one round trip. `directory: true` fetches the directory whole in the
open wave, so every dictionary plan's first phase is empty and a lookup
after open is one round trip, the records. A data read then fetches the 4
KiB chunks a run spans, not its block. Cold, a search is open, records,
postings; `w6-waves` records the counts on the day fixture, and
`waves-plan.md` the reasoning. Small runs -- under 256 bytes -- are inline
in a segment's index record and cost no postings round trip at all; a
`Store`-written index never inlines, which is the case for writing the
roll through `SegmentWriter`.

A segment's blocks can be compressed (`SegmentWriter::set_compress`),
chunked so a point read decompresses one chunk. It is worth 19.9% on
logshed's day when postings are stored as deltas and nothing at all when
they are stored as absolute ordinals, because LZ4 needs repeated bytes.
Runs under 256 bytes are inline in the key section and are never
compressed, so inlining and compression trade against each other.

A segment's key index carries a checksum row -- one CRC32C per 16 KiB
piece, after the hash region. The whole reader verifies every piece at
open. A piece is the section's intersection with one 16 KiB page of the
object -- the same pages `cache.mjs` fetches -- so the sparse reader, which
fetches the row in its second open plan and rounds every dictionary plan
out to pieces, verifies exactly the pages the host fetched and never asks
for more than a page-fetching host was going to fetch anyway. A piece is
verified the first time a plan's bytes are used, so a damaged directory
entry or record is an error rather than a different answer even over a
partial fetch. A walk over a piece already verified reads less than it planned;
`tests/dict.rs` holds reads within plans rather than equal to them. A store
written by `Store` (not a segment) has no row and is read as before.

`countFixed` and `scanCountsFixed` answer `null` rather than guessing when a
key's values are not all the width you named. Since format v6 a run whose
values share one width is stored without length prefixes and its extent says
so, and for such a run the count is exact: the flag records the width, so the
answer is `len / width` and nothing is assumed. For a run written with mixed
widths the old two-quantity check still applies, and that is a check, not a
proof -- the contract is that you know your own schema. Reading a fixed run
is a copy of its bytes rather than a decode, which is where `ext-analytics`
took the full-list read from 0.31x of LMDB's DUPFIXED to parity (EXT.18) and
the intersection to 1.15x (EXT.17).

## Size

Measured by `build.sh` into `results/w3-bundle.*.json`, against the budget in
`src/bin/logshed.rs`. There is no binding generator: the ABI is twenty
hand-written C functions passing integers and byte ranges, because a
generator's shim and descriptor sections are exactly what the budget is about.
R6's planning seam is the first thing to have moved the marginal number --
4,225 gzipped bytes, visible in W3.3 -- which is what the number is for.

`floor/` is why the number is legible. A wasm cdylib in Rust is not small
before any of your code is in it, so the floor is built the same way with the
same standard-library surface and none of supdb. The difference is supdb's
actual marginal cost.

## Signedness at the boundary

A wasm `u32` return arrives in JavaScript as a *signed* i32, and a `u64` as a
signed BigInt, so the failure sentinels (`u32::MAX`, `u64::MAX`) arrive as
`-1` and `-1n`. The first version of `supdb.mjs` compared them unnormalized,
which made every error check in the library dead: a reader over an object
that failed to open answered `[]` for every key, and a lookup whose block
failed its checksum came back empty — an under-return, which is the one
thing this index may never do, reported by the first downstream integration.
The convention now is one rule applied everywhere: normalize to unsigned at
the boundary (`v >>> 0`, `BigInt.asUintN(64, v)`), compare unsigned, return
unsigned. `web/test/node.mjs` holds the door shut, from the zeroed-object
repro down to a corrupt block byte throwing on every read rather than only
the first.

## Endianness

Every scalar in the file is written little-endian, but the zero-copy read path
reinterprets an extent array as `&[Ext]`, which is native-endian. Those agree
only on a little-endian machine. `Blob::open` refuses a big-endian target
explicitly rather than misreading a valid file there. Every browser is
little-endian, so this costs nothing -- but it is checked rather than assumed,
and `store::Reader` has the same latent hazard and does not check it.
