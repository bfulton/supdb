"""What a landing of 64 MB costs by what its blocks were before it: past
the file's end, reserved and never written (fallocate), written once with
zeros, written with a segment before. And what writing the zeros costs."""
import os, sys, time, ctypes, statistics as st
D = sys.argv[1]; os.makedirs(D, exist_ok=True)
MB = 1 << 20; S = 64; libc = ctypes.CDLL(None, use_errno=True)
def sfr(fd, off, n, flags): libc.sync_file_range(ctypes.c_int(fd), ctypes.c_int64(off), ctypes.c_int64(n), ctypes.c_uint(flags))
def falloc(fd, off, n):
    r = libc.fallocate(ctypes.c_int(fd), ctypes.c_int(0), ctypes.c_int64(off), ctypes.c_int64(n)); assert r == 0, ctypes.get_errno()
def flushes(): return int(open('/sys/block/vda/stat').read().split()[15])
def wsect(): return int(open('/sys/block/vda/stat').read().split()[6])
def ms(f):
    t = time.perf_counter(); f(); return (time.perf_counter() - t) * 1e3
rnd = os.urandom(MB); zero = bytes(MB)
man = os.open(os.path.join(D, 'manifest'), os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
os.pwrite(man, rnd[:8192], 0); os.fsync(man)
def slot(rep): os.pwrite(man, rnd[:4096], (rep % 2) * 4096); os.fdatasync(man)
def write(fd, base):
    for i in range(S): os.pwrite(fd, rnd, base + i * MB); sfr(fd, base + i * MB, MB, 2)
def run(name, prep, land, reps=6):
    xs = []; fl = []; pre = []
    for rep in range(reps):
        p = os.path.join(D, 'data'); fd = os.open(p, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
        os.pwrite(fd, rnd, 0); os.fsync(fd)             # a file with a first megabyte
        base = MB
        pre.append(ms(lambda: prep(fd, base)))
        f0 = flushes(); xs.append(ms(lambda: land(fd, base, rep))); fl.append(flushes() - f0)
        os.close(fd); os.unlink(p)
    print(f'{name:58s} {st.median(xs):6.1f} ms  flushes {fl[0]}   before it {st.median(pre):6.1f} ms')
def synced(fd, base, rep): write(fd, base); os.fdatasync(fd); slot(rep)
def ranged(fd, base, rep): write(fd, base); sfr(fd, base, S * MB, 7); slot(rep)
def nothing(fd, base): pass
def reserve(fd, base): falloc(fd, base, S * MB); os.fdatasync(fd)
def zeros(fd, base):
    for i in range(S): os.pwrite(fd, zero, base + i * MB); sfr(fd, base + i * MB, MB, 2)
    os.fdatasync(fd)
def segment(fd, base):
    write(fd, base); os.fdatasync(fd)
print('a landing of 64 MB, by what its blocks were                      landing            preparing them')
run('past the end; the data file synced, then the slot', nothing, synced)
run('reserved, never written; the data file synced, then the slot', reserve, synced)
run('reserved, never written; by range, the slot the flush (unsound)', reserve, ranged)
run('written with zeros; by range, the slot the flush', zeros, ranged)
run('written with a segment; by range, the slot the flush', segment, ranged)
run('written with a segment; the data file synced, then the slot', segment, synced)
