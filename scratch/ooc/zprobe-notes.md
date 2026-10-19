# zprobe (standalone, outside the engine) -- smoke test only, 200k keys, no bound, niced
raw pages 117.0 B/key; z pages 68.3 B/key at 60.0 records/page (page-filling LZ4 groups).
In-memory read: raw 0.307 us, z 3.112 us (whole-page decompress each read): the compact form is ~10x slower
in memory in the probe (the engine's own raw read is ~0.7 us), so the regime switch must fire only when
misses are frequent: break-even is about 0.065 faults/read at 43 us a fault.
RocksDB (rocksdb-tuned) bytes_on_disk_per_byte at 3M: 1.010 -- uncompressed in this suite, so no compression
advantage to chase; LMDB readahead-off is the real out-of-core comparator on reads.
