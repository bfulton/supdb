# The benchmark suite

How fast is supdb against what you would otherwise use, on the workloads
that matter, on the machines people actually run — and is that getting
better or worse.

The suite is a time series. A run measures every arm over a ladder of store
sizes and writes one row under `runs/`. Nothing in a row is derived; median,
error bars and the machine class are computed when the series is read.
[`DESIGN.md`](DESIGN.md) is the specification.

## Running it

```sh
cd bench
cargo build --release
./target/release/bench run --scale quick        # about three minutes; gates a PR
./target/release/bench run --scale full         # hours, on a quiet machine
./target/release/bench gate runs-ci/quick/*.json # a row against the series
./target/release/bench figures --scale quick    # every figure, from runs/
./target/release/bench machine                  # the host as a row records it
```

`quick` tops its ladder at 300 000 keys. `full` sizes its ladder to the
machine: the store crosses 1.5× memory wherever it runs. A row lands at
`runs/<scale>/<utc>-<sha7>.json`; commit the ones worth keeping.

Building the RocksDB comparator runs bindgen, which needs the libclang
shared library (`libclang.so` on Linux, `libclang.dylib` on macOS). If the
build cannot find one, `eval "$(sh scripts/libclang.sh)"` exports what the
host needs: the toolchain's directory, and on macOS a run-time search path
for it (on a Debian-family host it installs `libclang-dev` when nothing
unversioned exists). CI runs the same script.

## What is measured

| workload | shape |
|---|---|
| `load` | keys in order, 100-byte values, durable per batch |
| `load-shuffled` | the same keys in a shuffled order |
| `read` | uniform point reads over the loaded set |
| `scan` | ordered scans of 100 entries from uniform starts |
| `ycsb-A` … `ycsb-F` | the YCSB core mixes on the loaded store |
| `wal-floor`, `scan-floor` | what the device does with no engine in the way |

Arms: `supdb`, `supdb-noadvice`, `lmdb`, `rocksdb-tuned` (durable per
batch); `supdb-ingest`, `lmdb-nosync`, `rocksdb-nosync` (buffered). Every
comparison is within a guarantee.

## Checks

`sh scripts/check.sh` runs build, test, lint and one quick run; the engine's
`scripts/check.sh` and CI call the same groups by the same names.
