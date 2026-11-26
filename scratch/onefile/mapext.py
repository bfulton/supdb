"""X3: a mapping of an extent against a file of its own. 64 MB read cold
through a mapping -- every page in order, then 4,000 pages at random under
MADV_RANDOM -- as a file and as an extent a gibibyte into a 2 GB file,
mapped through an open of its own."""
import os, sys, time, mmap, random, resource
D = sys.argv[1]; os.makedirs(D, exist_ok=True)
MB = 1 << 20; S = 64; rnd = os.urandom(MB)
def mk(path, mb):
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_TRUNC, 0o644)
    for i in range(mb): os.pwrite(fd, rnd, i * MB)
    os.fsync(fd); os.close(fd)
mk(os.path.join(D, 'file'), S); mk(os.path.join(D, 'one'), 2048)
def majflt(): return resource.getrusage(resource.RUSAGE_SELF).ru_majflt
def cold(path):
    fd = os.open(path, os.O_RDONLY); os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED); os.close(fd)
random.seed(7); pages = [random.randrange(S * 256) for _ in range(4000)]
def run(name, path, off):
    out = []
    for kind in ('scan', 'points'):
        cold(path)
        fd = os.open(path, os.O_RDONLY)                 # an open of its own: its own readahead state
        m = mmap.mmap(fd, S * MB, mmap.MAP_SHARED, mmap.PROT_READ, offset=off); os.close(fd)
        if kind == 'points': m.madvise(mmap.MADV_RANDOM)
        f0 = majflt(); t = time.perf_counter(); x = 0
        if kind == 'scan':
            for p in range(0, S * MB, 4096): x += m[p]
        else:
            for p in pages: x += m[p * 4096]
        out.append((kind, (time.perf_counter() - t) * 1e3, majflt() - f0)); m.close()
    print(f'{name:22s} ' + '   '.join(f'{k}: {t:7.1f} ms, {f:5d} major faults' for k, t, f in out))
for rep in range(3):
    run('a file of its own', os.path.join(D, 'file'), 0)
    run('an extent of one file', os.path.join(D, 'one'), 1024 * MB)
