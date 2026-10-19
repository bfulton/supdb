"""The boundary experiment, foreign-runtime half: the same stores the C
harness loaded, read from Python through each engine's own binding --
supdb through supdbpy (a C extension over capi/), LMDB through py-lmdb,
RocksDB through rocksdict -- borrowed where the binding offers it and
copied where it does not, the suite's keys, interleaved and repeated.

Usage: bench.py <root> <keys> <value_bytes> <reps>
"""
import sys, time, statistics
import lmdb, rocksdict, supdbpy

root, keys, vlen, reps = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
MASK = (1 << 64) - 1

def suite_keys(n, seed=7):
    s = seed or 0x9E3779B97F4A7C15
    out = []
    for _ in range(n):
        s ^= (s << 13) & MASK; s ^= s >> 7; s ^= (s << 17) & MASK
        out.append(("%016d" % (s % n)).encode())
    return out

KEYS = suite_keys(keys)

sup = supdbpy.Store(f"{root}/supdb")
env = lmdb.open(f"{root}/lmdb", readonly=True, lock=False, max_dbs=4,
                map_size=keys * (16 + vlen) * 4 + (256 << 20))
opts = rocksdict.Options(raw_mode=True)
bbo = rocksdict.BlockBasedOptions()
bbo.set_block_cache(rocksdict.Cache(256 << 20))
bbo.set_bloom_filter(10.0, False)
bbo.set_cache_index_and_filter_blocks(True)
opts.set_block_based_table_factory(bbo)
# Read-write, not read-only: a read-only open leaves the WAL unreplayed and
# the load's tail unread. The open replays it and flushes to level 0.
rdb = rocksdict.Rdict(f"{root}/rocksdb", opts)

def value_of(k, n):
    m = int(k)
    return bytes(((m * 31 + i) & 0xFF) for i in range(n))

# Every binding answers the first key with the loaded value.
k0 = KEYS[0]; want = value_of(k0, vlen)
with env.begin(buffers=True) as txn:
    assert bytes(txn.get(k0)) == want, "lmdb"
assert rdb[k0] == want, "rocksdict"
assert sup.get(k0) == want, "supdbpy.get"
assert bytes(sup.view(k0)) == want, "supdbpy.view"
assert sup.len_of(k0) == vlen, "supdbpy.len_of"

def loop_floor():
    f = len; t = time.perf_counter_ns(); n = 0
    for k in KEYS: n += f(k)
    return time.perf_counter_ns() - t

def sup_len():
    g = sup.len_of; t = time.perf_counter_ns(); n = 0
    for k in KEYS: n += g(k)
    return time.perf_counter_ns() - t, n

def sup_view():
    g = sup.view; t = time.perf_counter_ns(); n = 0
    for k in KEYS: n += len(g(k))
    return time.perf_counter_ns() - t, n

def sup_bytes():
    g = sup.get; t = time.perf_counter_ns(); n = 0
    for k in KEYS: n += len(g(k))
    return time.perf_counter_ns() - t, n

def lmdb_view():
    with env.begin(buffers=True) as txn:
        g = txn.get; t = time.perf_counter_ns(); n = 0
        for k in KEYS: n += len(g(k))
        return time.perf_counter_ns() - t, n

def lmdb_bytes():
    with env.begin(buffers=False) as txn:
        g = txn.get; t = time.perf_counter_ns(); n = 0
        for k in KEYS: n += len(g(k))
        return time.perf_counter_ns() - t, n

def rocks_bytes():
    g = rdb.get; t = time.perf_counter_ns(); n = 0
    for k in KEYS: n += len(g(k))
    return time.perf_counter_ns() - t, n

variants = [("supdb len_of (no value object)", sup_len), ("supdb view (no copy)", sup_view), ("supdb bytes (one copy)", sup_bytes),
            ("lmdb buffers=True (no copy)", lmdb_view), ("lmdb bytes (one copy)", lmdb_bytes),
            ("rocksdict bytes (one copy)", rocks_bytes)]
ns = {name: [] for name, _ in variants}
floor = []
for r in range(reps):
    floor.append(loop_floor())
    for j in range(len(variants)):
        name, f = variants[(j + r) % len(variants)]
        t, n = f()
        assert n == keys * vlen, (name, n)
        ns[name].append(t)
print(f"\n{'variant':32s} {'min ns':>10s} {'med ns':>10s} {'max ns':>10s}   reps={reps} keys={keys} value={vlen}")
print(f"{'python loop, len(key) only':32s} {min(floor)/keys:10.1f} {statistics.median(floor)/keys:10.1f} {max(floor)/keys:10.1f}")
for name, _ in variants:
    v = ns[name]
    print(f"{name:32s} {min(v)/keys:10.1f} {statistics.median(v)/keys:10.1f} {max(v)/keys:10.1f}")
