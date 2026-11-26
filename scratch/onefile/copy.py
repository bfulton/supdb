"""What a checkpoint by copy costs: a gigabyte of one file copied into a new
file and made durable, with the source in the page cache and out of it."""
import os, sys, time
D = sys.argv[1]; os.makedirs(D, exist_ok=True)
MB = 1 << 20; G = 1024
rnd = os.urandom(MB)
src = os.path.join(D, 'src'); fd = os.open(src, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
for i in range(G): os.pwrite(fd, rnd, i * MB)
os.fsync(fd)
def copy(cold):
    if cold: os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
    dst = os.path.join(D, 'dst'); out = os.open(dst, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o644)
    t = time.perf_counter(); off = 0
    while off < G * MB:
        n = os.copy_file_range(fd, out, 64 * MB, off, off)
        if n == 0: break
        off += n
    tc = time.perf_counter(); os.fsync(out); ts = time.perf_counter()
    os.close(out); os.unlink(dst)
    return tc - t, ts - tc
for cold in (False, True, False, True):
    c, s = copy(cold)
    print(f'1 GiB copied, source {"cold" if cold else "cached"}: copy {c:5.2f} s + sync {s:5.2f} s = {c+s:5.2f} s')
os.close(fd); os.unlink(src)
