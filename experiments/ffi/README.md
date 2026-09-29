# The boundary experiment: a point read borrowed against copied, across a C ABI

What a language boundary costs a point read, measured rather than argued.
The question it was built for: the engine's read is short, so a fixed cost
at the boundary -- the call, a copy of the value, a foreign runtime's
object -- takes a larger share of it than of a slower engine's, and the
lead measured natively could be narrower through a binding. Two halves:

- `capi/` is a native C ABI over the engine, ten functions, the suite's
  `supdb` arm opened and read through the writer's own handle. A read is
  `Reader::read_all`, which lends each value to a callback. `get_borrow`
  hands that pointer out -- valid until the next commit, flush or close,
  LMDB's rule with the transaction replaced by the store's quiescence --
  and `get_copy` fills the caller's buffer. `bench_native` runs the
  suite's read pass inside the library, so the same loop is measured with
  no boundary crossed per read.
- `harness.c` opens a supdb store through that ABI, an LMDB store through
  liblmdb and a RocksDB store through librocksdb's C API in one process,
  loads each with the same keys and values (the arms' shapes: durable
  batches of a thousand, supdb drained, RocksDB's memtable held, the
  `rocksdb-tuned` options), verifies every key on every store and that a
  borrowed pointer still reads right after the pass, then runs the suite's
  read pass -- uniform keys, sixteen decimal digits, seed 7 -- as fifteen
  variants: the in-library loop, the ABI borrowed, borrowed and the first
  byte touched, the last byte touched, memcpy'd, the copying entry point,
  and the same shapes for the comparators. Variants rotate so none is
  always first, and rep by rep each pair of interest gets a sign test.
- `supdbpy.c` is the smallest CPython binding over the ABI written the
  way a binding for a hot loop is: a type holding the handle, single-
  argument methods, `get` returning bytes (one copy, py-lmdb's default)
  and `view` returning a memoryview over the store's bytes (no copy,
  py-lmdb's `buffers=True`). `bench.py` reads the stores the harness
  loaded through it, py-lmdb and rocksdict.

Results are banked by date beside this file, one sitting each, with the
machine they were taken on.

## Running it

    apt-get install liblmdb-dev librocksdb-dev
    python3 -m venv .venv && .venv/bin/pip install lmdb rocksdict
    PYTHON=.venv/bin/python sh run.sh /tmp/ffi-stores 300000 100 7
    PYTHON=.venv/bin/python sh run.sh /tmp/ffi-stores-4k 100000 4096 7

`run.sh` builds the ABI crate, the harness and the extension, loads the
stores and runs both halves. The harness reopens existing stores when run
without `load`.

## What is and is not measured

The read pass counts value bytes by their length, as the suite's does, and
never touches the value: that is the "borrowed" figure, the cost of
finding a value and being handed a pointer to it. Whether a consumer then
pays for the bytes depends on where they lie relative to what the lookup
already fetched, which the touch and copy variants price. The Python loop
has its own floor, printed first, and the per-call cost of a binding is
what it adds over the C figure for the same read.

The supdb store is opened with the arm's options restated here (segment
checksums off, compact records, the rest the engine's defaults) rather
than through the suite's adapter, which cannot be linked beside a second
RocksDB. If the arm's options move, this restatement is a second copy of
them.
