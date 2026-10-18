# zprobe

A probe, outside the engine, of what a compact form of the records does to the page cache under a
memory bound. It writes the suite's records (a sixteen-digit key and a hundred-byte value, half of it a
repeated phrase) into 4 KiB pages two ways, raw and as LZ4 groups that each fill one page, and reads them
at random through a mapping with readahead off, counting major faults. `HASH_MB` maps a file that every
read touches once, standing for the engine's resident hash; `BALLAST_MB` holds anonymous memory, standing
for what the engine keeps beside its cache.

It is the physics of the page cache and nothing else: fixed-size records, no tombstones, no multi-value keys,
no overlay of unsealed keys, one thread. What it decided, and the numbers, are in `prereg.md`; the design
that follows from it is `docs/engine.md`'s out-of-core section.

```sh
cargo build --release --example zprobe
Z=target/release/examples/zprobe; D=/some/scratch/dir
$Z build $D 10000000 190                       # raw.bin, z.bin, their fences, a 190 MB hash.bin; about 2 GB
python3 examples/zprobe/drop.py $D/raw.bin $D/z.bin $D/hash.bin
BALLAST_MB=100 sh examples/zprobe/bound.sh 1073741824 $Z read $D 10000000 z 0 1000000 4
```

`bound.sh` makes a cgroup v1 memory group under the one the shell is in and runs the command in it; it
needs the memory controller writable, as `scripts/ooc.sh` does. Drop the files from the cache before each
run and give each run a group of its own: a page another group paid for is a hit that the bound did not
charge.
