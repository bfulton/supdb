import os, time, sys
D = sys.argv[1]; os.makedirs(D, exist_ok=True)
MB = 1 << 20
buf = os.urandom(MB)
SIZE = int(sys.argv[2]) if len(sys.argv) > 2 else 64
def write(path, trunc, size=SIZE):
    flags = os.O_CREAT | os.O_WRONLY | (os.O_TRUNC if trunc else 0)
    fd = os.open(path, flags, 0o644)
    t = time.perf_counter()
    for i in range(size):
        os.pwrite(fd, buf, i * MB)
    tw = time.perf_counter()
    os.fsync(fd)
    ts = time.perf_counter()
    os.close(fd)
    return (tw - t) * 1e3, (ts - tw) * 1e3
def free(path):
    t = time.perf_counter(); os.unlink(path); return (time.perf_counter() - t) * 1e3
res = {'fresh': [], 'recycled': []}
seed = os.path.join(D, 'old')
write(seed, True)
for rep in range(6):
    # fresh: write a new file, then free the old one (what a merge does: new output, old input unlinked)
    new = os.path.join(D, f'new{rep}')
    w, s = write(new, True)
    f = free(seed)
    res['fresh'].append((w, s, f))
    seed = new
    # recycled: write the next file over the dead one's blocks, nothing freed
    dead = seed
    nxt = os.path.join(D, f'rec{rep}')
    os.rename(dead, nxt)
    w, s = write(nxt, False)
    res['recycled'].append((w, s, 0.0))
    seed = nxt
for k, v in res.items():
    v = v[1:]
    print(k, 'write ms', [round(a, 1) for a, _, _ in v], 'fsync ms', [round(b, 1) for _, b, _ in v], 'free ms', [round(c, 1) for _, _, c in v],
          'total', round(sum(a + b + c for a, b, c in v) / len(v), 1))
