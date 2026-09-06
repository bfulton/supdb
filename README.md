# Supdb

A read-optimized embedded key-multivalue store, in Rust. A key holds an
ordered run of values; appends are cheap, and the on-disk layout spends space
to make point reads and ordered scans fast. One reader serves both a
memory-mapped file on a server and a browser fetching byte ranges out of
object storage: the read path compiles to wasm and answers the same question
from either source.

- A durable commit is one WAL append and one `fdatasync`. Batches are atomic,
  and `Txn` builds one.
- Data lives in immutable sealed segments behind a flat hash index. Compaction
  partitions by key range, so a read routes to one segment.
- Deletes are tombstones the merge collects. Small runs are stored inline in
  the index record, so reading them touches no data block; a run of one width
  is stored without prefixes and read as a memcpy.
- Every data block and every piece of the key index is checksummed; a damaged
  file fails to open rather than answering wrongly.

## Benchmarks

The suite in [`bench/`](bench/) measures supdb against LMDB and RocksDB on
ordered and shuffled loads, point reads, ordered scans and the YCSB core
mixes, over a ladder of store sizes from ten thousand keys to past the
machine's memory, and against two floors: a durable framed append with no
engine, and a mapped sequential read of a file. Every comparison is
guarantee-matched, durable against durable and buffered against buffered.
A run writes one row of raw samples; `bench figures` draws every figure
from the committed rows, and `bench gate` fails a change whose row is worse
than the last ten of its machine class. [`bench/DESIGN.md`](bench/DESIGN.md)
is the specification.

What the curves show, in words: point reads and the read-heavy YCSB mixes
lead both comparators; the durable ordered load trails both, and shuffled
arrival inverts that; ordered scans lead RocksDB and trail LMDB; once the
store leaves memory, reads fall off a cliff, because every miss is a page
fault. The figures carry the numbers.

## Usage

```toml
[dependencies]
supdb = { git = "https://github.com/bfulton/supdb" }
```

A store: append values to keys, commit, read them back.

```rust
use supdb::{Db, Options};

let mut db = Db::create(std::path::Path::new("./store"), Options::default())?;

db.append(b"user:42", b"logged in");
db.append(b"user:42", b"opened report");
db.put(b"config", b"v2");            // replace: delete and append in one batch
db.commit()?;                        // the durability point

let mut tx = db.begin();             // atomic: all of it or none of it
tx.append(b"user:42", b"logged out");
tx.delete(b"config");
tx.commit()?;

db.read_all(b"user:42", |v| println!("{}", String::from_utf8_lossy(v)))?;
let n = db.count(b"user:42")?;       // costs a lookup, not a read
db.scan(b"user:", 100, |key, value| { /* in key order */ })?;
db.close()?;
```

A write-once segment: sorted input in, one immutable file out, read by the
same reader the store uses.

```rust
use supdb::{Blob, MmapBytes, SegmentOptions, SegmentWriter};

let path = std::path::Path::new("./day.sup");
let mut w = SegmentWriter::create(path, &SegmentOptions::default())?;
for (key, values) in sorted_input {   // keys in byte order
    w.begin(key)?;
    for v in values { w.value(v); }
    w.end()?;
}
w.finish(1)?;

let seg = Blob::open(MmapBytes::open(path)?)?;
seg.read_all(b"term", |v| { /* zero-copy borrow into the mapping */ })?;
```

With the whole input in hand, `SegmentWriter::write_sorted` writes the same
bytes and sizes the segment's head reserve exactly, so a reader's first probe
covers the index without a second round trip and a small segment does not
carry a large one's worth of zeroes. `supdb::reserve` answers the same
question on its own, from lengths or from totals.

The same segment in a browser, over ranged HTTP from a Web Worker:

```js
import { openSparse } from "./supdb.mjs";
import { CachedBytes, httpRangeFetcher } from "./cache.mjs";

const cache = await CachedBytes.open({
  name: "day",                          // sparse pages persist in OPFS under this name
  fetcher: httpRangeFetcher(url),
  budgetBytes: 32 << 20,
});
const reader = await openSparse(wasm, cache);
const values = reader.lookup(new TextEncoder().encode("term"));
```

`web/README.md` covers the three byte sources -- memory, OPFS, and a
budgeted page cache over HTTP or S3 -- and why the reader has to run in a
Worker.

## Building

```sh
cargo build --release
cargo test --release
sh scripts/check.sh            # build, test, lint, wasm, bench -- what CI runs
sh scripts/check.sh quick      # one quick-scale measurement, on an otherwise idle machine
rustup target add wasm32-unknown-unknown && sh web/build.sh   # the browser module
```

## Documentation

| where | what |
|---|---|
| `bench/DESIGN.md` | the benchmark suite: workloads, arms, the ladder, the gate, the figures |
| `docs/engine.md` | the engine's design and the measurements each decision cites |
| `docs/index-theory.md` | the index layout, and what theory predicts that measurement does not show |
| `web/README.md` | the browser reader |

## Status

A prototype. The on-disk format is not yet stable: it changes its magic
whenever an older reader would misread a newer file, and refuses the file
rather than serving wrong bytes.

## License

MIT. See `LICENSE`.
