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

![Load, read and scan against LMDB and RocksDB](https://raw.githubusercontent.com/bfulton/supdb-bench/main/figures/ext-kv.svg)

![YCSB core workloads against LMDB and RocksDB](https://raw.githubusercontent.com/bfulton/supdb-bench/main/figures/ext-ycsb.svg)

Every comparison is matched: an engine is not ranked against another until
both promise the same thing about durability, transactions and checksums, and
where an axis cannot be equalized the result is read as a bound rather than a
ranking. The figures are drawn from the committed measurements in
[supdb-bench](https://github.com/bfulton/supdb-bench); the claim ids below
name the checked numbers there.

- **Point reads lead** LMDB and RocksDB tuned as it would be deployed
  (`EXT.23`, `EXT.33`).
- **Ordered scans lead** tuned RocksDB and are level with LMDB (`EXT.24`,
  `EXT.34`).
- **YCSB** A (update-heavy), C (read-only), E (short scans) and F
  (read-modify-write) **lead** tuned RocksDB (`EXT.42`–`EXT.45`).
- **The durable ordered load trails** both LMDB and RocksDB (`EXT.22`,
  `EXT.28`). Under shuffled arrival the ordering inverts and the load leads
  LMDB (`EXT.27`), because a durable commit of scattered keys dirties about
  as many B-tree pages as it has keys. The two are one result; quote them
  together.
- **Space goes to RocksDB.** Blocks are stored uncompressed by default, which
  is what a point read that decompresses nothing costs; per-segment LZ4
  (`SegmentWriter::set_compress`) buys part of it back (`W6.8`).
- **Out of core, reads fall off a cliff.** Once the file outgrows the memory
  that can cache it, every miss is a synchronous page fault (`F1.2`, `F1.4`).

Every statement above is a claim with a recorded expected state in
[supdb-bench](https://github.com/bfulton/supdb-bench), whose CI reruns the
suite against each change here and fails when a claim moves in either
direction -- including when a known limitation gets fixed.

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
sh scripts/check.sh            # build, test, lint, wasm -- what CI runs
rustup target add wasm32-unknown-unknown && sh web/build.sh   # the browser module
```

## Documentation

| where | what |
|---|---|
| `docs/engine.md` | the engine's design and the measurements each decision cites |
| `docs/index-theory.md` | the index layout, and what theory predicts that measurement does not show |
| `web/README.md` | the browser reader |
| [supdb-bench](https://github.com/bfulton/supdb-bench) | the experiments, the claims, the results and the figures |

## Status

A prototype. The on-disk format is not yet stable: it changes its magic
whenever an older reader would misread a newer file, and refuses the file
rather than serving wrong bytes.

## License

MIT. See `LICENSE`.
