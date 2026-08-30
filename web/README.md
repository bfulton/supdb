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
| `supdb.mjs` | the library: two byte sources, one synchronous API |
| `worker.mjs` | the Web Worker the reader runs in, and why it has to |
| `build.sh` | builds the module and the floor, records `w3-bundle` |
| `floor/` | an empty cdylib with the same std surface -- the size control |
| `test/` | a real index file, a real browser, a real OPFS handle |

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

## What a lookup costs

A point lookup touches the key index section, one or two hash slots, one
record, and one block. The index and block table sections are read once at
open, because a source that cannot lend its bytes -- which an OPFS handle
cannot -- should pay per section rather than per lookup.

`count(key)` and `countFixed(key, width)` are two different things and the
difference is 28x. See `f28-count` and W2.1-W2.3 in `claims.json`: an `Ext`
records block, offset, byte length and the offset of the last record, and none
of those is a count, so the general count walks the values. A *fixed-width*
posting list does not need to -- its count is arithmetic on `Ext::len` with no
block touched. logshed's postings are four-byte line ordinals, so `countFixed`
is the call it makes.

## Size

Measured by `build.sh` into `results/w3-bundle.*.json`, against the budget in
`src/bin/logshed.rs`. There is no binding generator: the ABI is eleven
hand-written C functions passing integers and byte ranges, because a
generator's shim and descriptor sections are exactly what the budget is about.

`floor/` is why the number is legible. A wasm cdylib in Rust is not small
before any of your code is in it, so the floor is built the same way with the
same standard-library surface and none of supdb. The difference is supdb's
actual marginal cost.

## Endianness

Every scalar in the file is written little-endian, but the zero-copy read path
reinterprets an extent array as `&[Ext]`, which is native-endian. Those agree
only on a little-endian machine. `Blob::open` refuses a big-endian target
explicitly rather than misreading a valid file there. Every browser is
little-endian, so this costs nothing -- but it is checked rather than assumed,
and `store::Reader` has the same latent hazard and does not check it.
