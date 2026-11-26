"""What a store in one preallocated file would pay for its file operations,
beside what one in a file a segment pays: the primitives, on this device."""
import os, sys, time, threading, mmap, statistics as st, ctypes
D = sys.argv[1]; os.makedirs(D, exist_ok=True)
MB = 1 << 20
libc = ctypes.CDLL(None, use_errno=True)
SFR_WAIT_BEFORE, SFR_WRITE, SFR_WAIT_AFTER = 1, 2, 4
def sfr(fd, off, n, flags):
    libc.sync_file_range(ctypes.c_int(fd), ctypes.c_int64(off), ctypes.c_int64(n), ctypes.c_uint(flags))
def ms(f):
    t = time.perf_counter(); f(); return (time.perf_counter() - t) * 1e3
def med(xs): return st.median(xs)
def p99(xs): return sorted(xs)[max(0, int(len(xs) * 0.99) - 1)]
rnd = os.urandom(MB)
def aligned(n):
    m = mmap.mmap(-1, n); m.write(rnd[:n] if n <= MB else rnd * (n // MB)); return m

def big(path, mb):
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
    for i in range(mb): os.pwrite(fd, rnd, i * MB)
    os.fsync(fd); return fd

# 1. A commit: 16.8 KB made durable, 300 of them.
N, CB = 300, 16800
rec = rnd[:CB]
def commits(kind):
    p = os.path.join(D, 'log-' + kind)
    out = []
    if kind == 'append':
        fd = os.open(p, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o644)
        for i in range(N):
            out.append(ms(lambda: (os.pwrite(fd, rec, i * CB), os.fdatasync(fd))))
    elif kind == 'prewritten':
        fd = big(p, 8)
        for i in range(N):
            out.append(ms(lambda: (os.pwrite(fd, rec, i * CB), os.fdatasync(fd))))
    elif kind == 'prewritten-dsync':
        os.close(big(p, 8)); fd = os.open(p, os.O_WRONLY | os.O_DSYNC)
        for i in range(N):
            out.append(ms(lambda: os.pwrite(fd, rec, i * CB)))
    elif kind == 'prewritten-direct-dsync':
        os.close(big(p, 8)); fd = os.open(p, os.O_WRONLY | os.O_DSYNC | os.O_DIRECT)
        blk = aligned(20480)
        for i in range(N):
            out.append(ms(lambda: os.pwrite(fd, blk, i * 20480)))
    os.close(fd); os.unlink(p)
    return out
print('-- a commit of 16.8 KB made durable (ms: median, p99)')
for k in ('append', 'prewritten', 'prewritten-dsync', 'prewritten-direct-dsync'):
    xs = commits(k); print(f'{k:26s} {med(xs):6.3f} {p99(xs):7.3f}')

# 2. The same commit beside 256 MB being written: in the commit's own file, and in another.
def beside(where, how):
    logp = os.path.join(D, 'c-log'); datp = os.path.join(D, 'c-data')
    lfd = big(logp, 300)                     # the log region is the first 8 MB; data from 16 MB on
    dfd = lfd if where == 'same' else big(datp, 300)
    wfd = dfd
    if how == 'direct':
        wfd = os.open(logp if where == 'same' else datp, os.O_WRONLY | os.O_DIRECT)
    buf = aligned(MB)
    stop = [False]; wrote = [0]
    def writer():
        i = 0
        while not stop[0]:
            off = (16 + (i % 256)) * MB
            os.pwrite(wfd, buf, off)
            if how == 'hinted': sfr(wfd, off, MB, SFR_WRITE)
            wrote[0] += 1; i += 1
    t = threading.Thread(target=writer); t.start()
    time.sleep(0.05)
    out = []
    for i in range(N):
        out.append(ms(lambda: (os.pwrite(lfd, rec, i * CB), os.fdatasync(lfd))))
    stop[0] = True; t.join()
    if wfd != dfd: os.close(wfd)
    if dfd != lfd: os.close(dfd); os.unlink(datp)
    os.close(lfd); os.unlink(logp)
    return out, wrote[0]
print('-- the same commit beside a writer of megabyte pieces (ms: median, p99; MB the writer wrote)')
for where in ('other', 'same'):
    for how in ('buffered', 'hinted', 'direct'):
        xs, w = beside(where, how)
        print(f'{where:5s} file, writer {how:8s} {med(xs):7.3f} {p99(xs):8.3f}   {w} MB in {sum(xs)/1e3:.2f} s')

# 3. A segment of 64 MB made durable.
S = 64
def seg(kind):
    out = []
    for rep in range(5):
        if kind == 'fresh file':
            p = os.path.join(D, f's{rep}')
            def f():
                fd = os.open(p, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o644)
                for i in range(S): os.pwrite(fd, rnd, i * MB)
                os.fsync(fd); os.close(fd)
            out.append(ms(f)); os.unlink(p)
        else:
            p = os.path.join(D, 'container')
            if rep == 0: cfd = big(p, 4 * S)
            base = (rep % 3) * S * MB
            if kind == 'extent, fdatasync':
                def f():
                    for i in range(S): os.pwrite(cfd, rnd, base + i * MB)
                    os.fdatasync(cfd)
            elif kind == 'extent, ranged writeback + fdatasync':
                def f():
                    for i in range(S):
                        os.pwrite(cfd, rnd, base + i * MB); sfr(cfd, base + i * MB, MB, SFR_WRITE)
                    sfr(cfd, base, S * MB, SFR_WAIT_BEFORE | SFR_WRITE | SFR_WAIT_AFTER)
                    os.fdatasync(cfd)
            elif kind == 'extent, O_DIRECT + fdatasync':
                dfd = os.open(p, os.O_WRONLY | os.O_DIRECT); buf = aligned(MB)
                def f():
                    for i in range(S): os.pwrite(dfd, buf, base + i * MB)
                    os.fdatasync(dfd)
            out.append(ms(f))
            if kind.startswith('extent, O_DIRECT'): os.close(dfd)
            if rep == 4: os.close(cfd); os.unlink(p)
    return out
print('-- a segment of 64 MB written and made durable (ms: median of 5)')
for k in ('fresh file', 'extent, fdatasync', 'extent, ranged writeback + fdatasync', 'extent, O_DIRECT + fdatasync'):
    xs = seg(k); print(f'{k:40s} {med(xs):7.1f}   {[round(x) for x in xs]}')

# 4. Giving 64 MB up.
print('-- 64 MB given up (ms: median of 5)')
xs = []
for rep in range(5):
    p = os.path.join(D, f'u{rep}'); os.close(big(p, S)); xs.append(ms(lambda: os.unlink(p)))
print(f'{"unlink a file":40s} {med(xs):7.1f}')
p = os.path.join(D, 'container'); cfd = big(p, 6 * S); xs = []
FALLOC_FL_KEEP_SIZE, FALLOC_FL_PUNCH_HOLE = 1, 2
for rep in range(5):
    xs.append(ms(lambda: libc.fallocate(ctypes.c_int(cfd), ctypes.c_int(FALLOC_FL_KEEP_SIZE | FALLOC_FL_PUNCH_HOLE), ctypes.c_int64(rep * S * MB), ctypes.c_int64(S * MB))))
print(f'{"punch a hole in the container":40s} {med(xs):7.1f}')
print(f'{"reuse the extent":40s} {0.0:7.1f}')
os.close(cfd); os.unlink(p)

# 5. The names a landing makes: a small file made durable under a temp name, renamed, its directory synced, an old one unlinked.
dfd = os.open(D, os.O_RDONLY); xs = []
for rep in range(50):
    old = os.path.join(D, f'm{rep}'); tmp = old + '.tmp'; new = os.path.join(D, f'm{rep+1}')
    if rep == 0:
        fd = os.open(old, os.O_CREAT | os.O_WRONLY, 0o644); os.write(fd, rnd[:4096]); os.fsync(fd); os.close(fd)
    def f():
        fd = os.open(tmp, os.O_CREAT | os.O_WRONLY, 0o644); os.write(fd, rnd[:4096]); os.fsync(fd); os.close(fd)
        os.rename(tmp, new); os.fsync(dfd); os.unlink(old)
    xs.append(ms(f))
print('-- a 4 KB file made durable under a temp name, renamed, the directory synced, its predecessor unlinked (ms)')
print(f'{"median, p99":40s} {med(xs):7.3f} {p99(xs):7.3f}')
slot = os.open(os.path.join(D, 'slot'), os.O_CREAT | os.O_RDWR, 0o644); os.write(slot, rnd[:8192]); os.fsync(slot); xs = []
for rep in range(50):
    xs.append(ms(lambda: (os.pwrite(slot, rnd[:4096], (rep % 2) * 4096), os.fdatasync(slot))))
print(f'{"the same 4 KB written in place, fdatasync":40s} {med(xs):7.3f} {p99(xs):7.3f}')
