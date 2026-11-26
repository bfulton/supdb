"""Whether the log can live in the data file: a commit of 16.8 KB made
durable in a file of its own and in a region of the data file, beside a
writer of megabyte pieces in the data file, in the open and under a memory
bound the writer's pages are reclaimed against."""
import os, sys, time, threading, mmap, statistics as st, ctypes
D = sys.argv[1]; BOUND = int(sys.argv[2]) if len(sys.argv) > 2 else 0
os.makedirs(D, exist_ok=True)
MB = 1 << 20; libc = ctypes.CDLL(None, use_errno=True); RWF_DSYNC = 2
def sfr(fd, off, n, flags): libc.sync_file_range(ctypes.c_int(fd), ctypes.c_int64(off), ctypes.c_int64(n), ctypes.c_uint(flags))
def ms(f):
    t = time.perf_counter(); f(); return (time.perf_counter() - t) * 1e3
def p99(xs): return sorted(xs)[max(0, int(len(xs) * 0.99) - 1)]
rnd = os.urandom(MB)
def aligned(n):
    m = mmap.mmap(-1, n); m.write(rnd[:n]); return m
if BOUND:
    grp = None
    for l in open('/proc/self/cgroup'):
        if ':memory:' in l: grp = '/sys/fs/cgroup/memory' + l.strip().split(':', 2)[2] + '/onefile-probe'
    os.makedirs(grp, exist_ok=True)
    open(grp + '/memory.limit_in_bytes', 'w').write(str(BOUND))
    open(grp + '/cgroup.procs', 'w').write(str(os.getpid()))
SPAN = 2048 if BOUND else 256      # MB the writer walks: past the bound, so its pages are new each lap
datap = os.path.join(D, 'data'); logp = os.path.join(D, 'log')
dfd = os.open(datap, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
for i in range(16 + SPAN): os.pwrite(dfd, rnd, i * MB)
os.fsync(dfd); os.posix_fadvise(dfd, 0, 0, os.POSIX_FADV_DONTNEED)
lfd = os.open(logp, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
for i in range(8): os.pwrite(lfd, rnd, i * MB)
os.fsync(lfd)
N, CB = 300, 16800; rec = rnd[:CB]; blk = aligned(20480)
def run(where, how, hinted):
    path = datap if where == 'data file' else logp
    if how == 'fdatasync':
        fd = dfd if where == 'data file' else lfd
        def commit(i): os.pwrite(fd, rec, i * CB); os.fdatasync(fd)
    elif how == 'ranged':
        fd = os.open(path, os.O_WRONLY | os.O_DSYNC)     # a write that syncs its own range and no other
        def commit(i): os.pwrite(fd, rec, i * CB)
    else:
        fd = os.open(path, os.O_WRONLY | os.O_DIRECT | os.O_DSYNC)
        def commit(i): os.pwrite(fd, blk, i * 20480)
    stop = [False]; wrote = [0]
    def writer():
        i = 0
        while not stop[0]:
            off = (16 + (i % SPAN)) * MB
            os.pwrite(dfd, rnd, off)
            if hinted: sfr(dfd, off, MB, 2)
            wrote[0] += 1; i += 1
    t = threading.Thread(target=writer, daemon=True); t.start(); time.sleep(0.2)
    w0 = wrote[0]; t0 = time.perf_counter()
    xs = [ms(lambda: commit(i)) for i in range(N)]
    el = time.perf_counter() - t0; w = wrote[0] - w0
    stop[0] = True; t.join()
    if how != 'fdatasync': os.close(fd)
    os.fsync(dfd)
    print(f'log in {where:10s} {how:10s} writer {"hinted" if hinted else "plain ":6s}  commit median {st.median(xs):8.3f} p99 {p99(xs):8.3f} ms   writer {w/el:7.0f} MB/s')
print(f'bound {BOUND >> 20} MiB' if BOUND else 'no bound')
for hinted in (False, True):
    for where in ('own file', 'data file'):
        for how in ('fdatasync', 'ranged', 'direct'):
            run(where, how, hinted)
os.close(dfd); os.close(lfd); os.unlink(datap); os.unlink(logp)
