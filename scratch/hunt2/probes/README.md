# Probes over a kept store

Two small binaries over the engine's reader, written for the second hunt
and worth keeping: a failing fuzz seed leaves its two stores in `TMPDIR`
(`supdb-next-fuzz-s<seed>-{z,r}-<pid>`), and these read them.

    cargo run -q --bin segcheck -- <store dir>
    cargo run -q --bin keyprobe -- <store dir> <key as hex>

`segcheck` opens every segment file and prints its key count, whether
its rank order is sorted (the first inversion if not), and its first and
last keys beside the fence its name carries. `keyprobe` reopens the
store with the options of fuzz seed 36 (edit them for another seed),
scans it and point-reads the key, then asks every segment file what it
holds for the key, tombstones marked: it is how the zero-width partition
and the value a scan listed twice were found.
