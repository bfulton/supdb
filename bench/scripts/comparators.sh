#!/bin/sh
# The suite's comparator cells held to the comparators' own tools, on one
# machine in one job (`comparators.yml`; the rule is registered in
# docs/engine.md, the comparators' check): RocksDB's db_bench built from the
# very tree the suite links -- the source `librocksdb-sys` vendors, at the
# version Cargo.lock pins -- with the tuned arm's options, and a C program
# of a hundred lines against the LMDB `lmdb-master-sys` vendors
# (tools/lmdb_bench.c), since LMDB ships no tool; then the suite's own
# `rocksdb-tuned` and `lmdb` arms over the quick ladder, and the table
# scripts/comparators.py draws of the suite's cell against the tool's on
# the load, the point reads and the scans at every rung.
#
#   sh bench/scripts/comparators.sh [OUT]    # OUT defaults to comparators-ci
#
# Needs cmake, a C and C++ toolchain, gflags (libgflags-dev) and the Rust
# toolchain. Gated on explicit tests and never on `set -e`, which this
# environment's shell walks past (CLAUDE.md).
cd "$(dirname "$0")/.." || exit 2
out=${1:-comparators-ci}
rm -rf "$out"
mkdir -p "$out/tools" || exit 2

# The vendored sources, fetched into the registry if they are not there.
cargo fetch >/dev/null 2>&1 || { echo "comparators: cargo fetch failed" >&2; exit 2; }
reg=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/ 2>/dev/null | head -n 1)
rocks_ver=$(awk '/^name = "librocksdb-sys"/ {getline; sub(/.*\+/, ""); gsub(/"/, ""); print}' Cargo.lock)
rocks_src=$(ls -d "$reg"librocksdb-sys-*+"$rocks_ver"/rocksdb 2>/dev/null | head -n 1)
lmdb_src=$(ls -d "$reg"lmdb-master-sys-*/lmdb/libraries/liblmdb 2>/dev/null | head -n 1)
if [ -z "$rocks_src" ] || [ -z "$lmdb_src" ]; then
  echo "comparators: the vendored sources are not in the registry (rocksdb $rocks_ver: '$rocks_src'; lmdb: '$lmdb_src')" >&2
  exit 2
fi
echo "comparators: RocksDB $rocks_ver from $rocks_src"
echo "comparators: LMDB from $lmdb_src"

# db_bench, from the vendored tree, with no compression library, as the
# suite's build has none.
if ! cmake -S "$rocks_src" -B "$out/rocksdb-build" -DCMAKE_BUILD_TYPE=Release \
    -DWITH_BENCHMARK_TOOLS=ON -DWITH_GFLAGS=ON -DWITH_TESTS=OFF -DWITH_TOOLS=OFF \
    -DWITH_CORE_TOOLS=OFF -DWITH_TRACE_TOOLS=OFF -DWITH_SNAPPY=OFF -DWITH_LZ4=OFF \
    -DWITH_ZLIB=OFF -DWITH_ZSTD=OFF -DWITH_BZ2=OFF -DPORTABLE=1 -DFAIL_ON_WARNINGS=OFF \
    -DROCKSDB_BUILD_SHARED=OFF > "$out/tools/cmake.log" 2>&1; then
  echo "comparators: cmake failed" >&2
  tail -n 30 "$out/tools/cmake.log" >&2
  exit 2
fi
if ! cmake --build "$out/rocksdb-build" --target db_bench -j"$(nproc 2>/dev/null || echo 4)" > "$out/tools/build.log" 2>&1; then
  echo "comparators: building db_bench failed" >&2
  tail -n 30 "$out/tools/build.log" >&2
  exit 2
fi
db_bench="$out/rocksdb-build/db_bench"
[ -x "$db_bench" ] || { echo "comparators: no db_bench at $db_bench" >&2; exit 2; }

# The LMDB tool, against the vendored LMDB.
if ! cc -O2 -o "$out/tools/lmdb_bench" tools/lmdb_bench.c "$lmdb_src/mdb.c" "$lmdb_src/midl.c" \
    -I"$lmdb_src" -lpthread 2> "$out/tools/cc.log"; then
  echo "comparators: building lmdb_bench failed" >&2
  cat "$out/tools/cc.log" >&2
  exit 2
fi

# The suite's two comparator arms over the quick ladder.
cargo build --release --bin bench || exit 2
./target/release/bench run --scale quick --out "$out" --arms rocksdb-tuned,lmdb || exit 2

# The tools at the quick ladder's rungs. db_bench: the tuned arm's options
# (compression off, a 256 MB block cache with the index and filter blocks
# in it, ten Bloom bits a key, 128 MB write buffers four deep merged two
# at a time, level 0 compacting at eight files, four background jobs), the
# load in synced batches of a thousand, N point reads, and seeks with 99
# nexts each for the suite's scans of a hundred entries. lmdb_bench: the
# suite's map size and shapes, in its own doc.
for n in 10000 30000 100000 300000; do
  rm -rf "$out/tools/dbb"
  if ! "$db_bench" --benchmarks=fillseq,readrandom,seekrandom --num="$n" --reads="$n" \
      --key_size=16 --value_size=100 --batch_size=1000 --sync=1 --seek_nexts=99 \
      --compression_type=none --cache_size=268435456 --bloom_bits=10 \
      --cache_index_and_filter_blocks=1 --write_buffer_size=134217728 \
      --max_write_buffer_number=4 --min_write_buffer_number_to_merge=2 \
      --level0_file_num_compaction_trigger=8 --max_background_jobs=4 --threads=1 \
      --verify_checksum=0 --statistics=0 --histogram=0 --db="$out/tools/dbb" \
      --use_existing_db=0 > "$out/tools/rocksdb-$n.txt" 2>&1; then
    echo "comparators: db_bench failed at $n" >&2
    tail -n 20 "$out/tools/rocksdb-$n.txt" >&2
    exit 2
  fi
  rm -rf "$out/tools/lmdb" && mkdir -p "$out/tools/lmdb" || exit 2
  # `lmdb_map_gb`: three times the payload, at least 8 GB; 8 at every quick rung.
  if ! "$out/tools/lmdb_bench" "$out/tools/lmdb" "$n" 8 1000 100 > "$out/tools/lmdb-$n.txt" 2>&1; then
    echo "comparators: lmdb_bench failed at $n" >&2
    cat "$out/tools/lmdb-$n.txt" >&2
    exit 2
  fi
done
rm -rf "$out/tools/dbb" "$out/tools/lmdb"

python3 scripts/comparators.py "$out" | tee "$out/comparators.md"
status=$?
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && cat "$out/comparators.md" >> "$GITHUB_STEP_SUMMARY"
exit $status
