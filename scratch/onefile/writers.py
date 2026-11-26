"""X2: one inode under many writers. Three writers of megabyte pieces, each
making 192 MB durable, into three files of their own, into three reserved
extents of one file, and into three extents of one file written before;
beside them a commit of 16.8 KB synced by its own range, in a log of its
own or in the one file. In the open, or under a memory bound (argv[2])."""
import os, sys, time, threading, statistics as st, ctypes
D = sys.argv[1]; BOUND = int(sys.argv[2]) if len(sys.argv) > 2 else 0
os.makedirs(D, exist_ok=True)
MB = 1 << 20; S = 192; N = 3; libc = ctypes.CDLL(None, use_errno=True)
def sfr(fd, off, n, flags): libc.sync_file_range(ctypes.c_int(fd), ctypes.c_int64(off), ctypes.c_int64(n), ctypes.c_uint(flags))
def falloc(fd, off, n): assert libc.fallocate(ctypes.c_int(fd), ctypes.c_int(0), ctypes.c_int64(off), ctypes.c_int64(n)) == 0
def p99(xs): return sorted(xs)[max(0, int(len(xs) * 0.99) - 1)]
rnd = os.urandom(MB)
if BOUND:
    for l in open('/proc/self/cgroup'):
        if ':memory:' in l: grp = '/sys/fs/cgroup/memory' + l.strip().split(':', 2)[2] + '/onefile-probe'
    os.makedirs(grp, exist_ok=True)
    open(grp + '/memory.limit_in_bytes', 'w').write(str(BOUND))
    open(grp + '/cgroup.procs', 'w').write(str(os.getpid()))
def run(shape):
    paths = []
    if shape == 'files':
        fds = [os.open(os.path.join(D, f'w{i}'), os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644) for i in range(N)]
        bases = [0] * N; paths = [os.path.join(D, f'w{i}') for i in range(N)]
        logp = os.path.join(D, 'log'); lbase = 0
        lfd0 = os.open(logp, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
        for i in range(8): os.pwrite(lfd0, rnd, i * MB)
        os.fsync(lfd0); os.close(lfd0); paths.append(logp)
    else:
        one = os.path.join(D, 'one'); fd = os.open(one, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
        for i in range(8): os.pwrite(fd, rnd, i * MB)          # the log's region, written once
        if shape == 'reserved': falloc(fd, 8 * MB, N * S * MB)
        else:
            for i in range(N * S): os.pwrite(fd, rnd, (8 + i) * MB)
        os.fsync(fd); os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        fds = [fd] * N; bases = [(8 + i * S) * MB for i in range(N)]
        logp = one; lbase = 0; paths = [one]
    lfd = os.open(logp, os.O_WRONLY | os.O_DSYNC)
    done = [0.0] * N; go = threading.Barrier(N + 1)
    def writer(i):
        go.wait(); t = time.perf_counter()
        for k in range(S):
            os.pwrite(fds[i], rnd, bases[i] + k * MB); sfr(fds[i], bases[i] + k * MB, MB, 2)
        if shape == 'written':
            sfr(fds[i], bases[i], S * MB, 7); os.pwrite(lfd, rnd[:4096], lbase + 7 * MB)   # its range, then a flush through the log
        else:
            os.fdatasync(fds[i])
        done[i] = time.perf_counter() - t
    ts = [threading.Thread(target=writer, args=(i,)) for i in range(N)]
    for t in ts: t.start()
    go.wait(); xs = []; i = 0
    while any(t.is_alive() for t in ts):
        t0 = time.perf_counter(); os.pwrite(lfd, rnd[:16800], lbase + (i % 300) * 16800); xs.append((time.perf_counter() - t0) * 1e3); i += 1
    for t in ts: t.join()
    os.close(lfd)
    for f in set(fds): os.close(f)
    for p in paths: os.unlink(p)
    rate = N * S / max(done)
    print(f'{shape:9s} writers {rate:6.0f} MB/s durable ({max(done):5.2f} s)   commit median {st.median(xs):7.2f} p99 {p99(xs):7.2f} ms  x{len(xs)}')
print(f'bound {BOUND >> 20} MiB' if BOUND else 'no bound')
for rep in range(2):
    for shape in ('files', 'reserved', 'written'):
        run(shape)
