"""Whether a data file needs a sync of its own: an extent's pages written
back by range, then the device flushed through another file's fdatasync.
Counts the device's flush requests (/sys/block/vda/stat, field 16)."""
import os, sys, time, ctypes, statistics as st
D = sys.argv[1]; os.makedirs(D, exist_ok=True)
MB = 1 << 20; libc = ctypes.CDLL(None, use_errno=True)
def sfr(fd, off, n, flags): libc.sync_file_range(ctypes.c_int(fd), ctypes.c_int64(off), ctypes.c_int64(n), ctypes.c_uint(flags))
def flushes(): return int(open('/sys/block/vda/stat').read().split()[15])
def ms(f):
    t = time.perf_counter(); f(); return (time.perf_counter() - t) * 1e3
rnd = os.urandom(MB)
data = os.open(os.path.join(D, 'data'), os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
for i in range(256): os.pwrite(data, rnd, i * MB)
os.fsync(data)
man = os.open(os.path.join(D, 'manifest'), os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
os.pwrite(man, rnd[:8192], 0); os.fsync(man)
# 1. does an fdatasync of a file with nothing dirty reach the device?
f0 = flushes(); t = ms(lambda: os.fdatasync(man)); print(f'fdatasync, nothing dirty:        {t:6.3f} ms, {flushes()-f0} flush')
f0 = flushes(); t = ms(lambda: (os.pwrite(man, rnd[:4096], 0), os.fdatasync(man))); print(f'4 KB slot written + fdatasync:   {t:6.3f} ms, {flushes()-f0} flush')
# 2. a landing: 64 MB into an extent, made durable three ways
S = 64
def own(rep):
    base = (rep % 3) * S * MB
    for i in range(S): os.pwrite(data, rnd, base + i * MB); sfr(data, base + i * MB, MB, 2)
    os.fdatasync(data)                                   # the data file's own sync
    os.pwrite(man, rnd[:4096], (rep % 2) * 4096); os.fdatasync(man)   # then the slot
def shared(rep):
    base = (rep % 3) * S * MB
    for i in range(S): os.pwrite(data, rnd, base + i * MB); sfr(data, base + i * MB, MB, 2)
    sfr(data, base, S * MB, 1 | 2 | 4)                   # the extent's pages at the device, no flush
    os.pwrite(man, rnd[:4096], (rep % 2) * 4096); os.fdatasync(man)   # the slot's sync is the flush
def fresh(rep):
    p = os.path.join(D, f'seg{rep}'); fd = os.open(p, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o644)
    for i in range(S): os.pwrite(fd, rnd, i * MB); sfr(fd, i * MB, MB, 2)
    os.fsync(fd); os.close(fd)
    dfd = os.open(D, os.O_RDONLY); os.fsync(dfd); os.close(dfd)   # its name
    os.pwrite(man, rnd[:4096], (rep % 2) * 4096); os.fdatasync(man)
    os.unlink(p)
for name, f in (('a fresh file, its name, the slot', fresh), ('an extent synced, then the slot', own), ('an extent written back, the slot the flush', shared)):
    xs = []; fl = []
    for rep in range(6):
        f0 = flushes(); xs.append(ms(lambda: f(rep))); fl.append(flushes() - f0)
    print(f'{name:44s} {st.median(xs):6.1f} ms  flushes {fl}')
