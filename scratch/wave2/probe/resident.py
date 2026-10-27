"""Page-cache residency of every file under a directory, by kind: mmap each
file privately and ask mincore which pages are resident (nothing is faulted
in). Usage: resident.py DIR TAG"""
import ctypes, mmap, os, re, sys
from collections import defaultdict
libc = ctypes.CDLL("libc.so.6", use_errno=True)
PAGE = os.sysconf("SC_PAGE_SIZE")
def resident(path):
    size = os.path.getsize(path)
    if size == 0:
        return 0, 0
    fd = os.open(path, os.O_RDONLY)
    try:
        mm = mmap.mmap(fd, size, flags=mmap.MAP_PRIVATE, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    finally:
        os.close(fd)
    try:
        n = (size + PAGE - 1) // PAGE
        vec = (ctypes.c_ubyte * n)()
        buf = (ctypes.c_char * size).from_buffer(mm)
        r = libc.mincore(ctypes.c_void_p(ctypes.addressof(buf)), ctypes.c_size_t(size), vec)
        del buf
        if r != 0:
            return size, -1
        bits = [b & 1 for b in vec]
        DEC.append((size, [sum(bits[i * n // 10:(i + 1) * n // 10]) * PAGE for i in range(10)]))
        return size, sum(bits) * PAGE
    finally:
        mm.close()
def kind(name):
    if name.endswith(".zpg") or "zpg" in name:
        return "form"
    m = re.match(r"([a-z]+)", name)
    k = m.group(1) if m else name
    if name.endswith(".ord") or "ord" in name.split(".")[-1]:
        return k + ".ord"
    return k
tot = defaultdict(lambda: [0, 0, 0])
DEC = []
deciles = defaultdict(lambda: [0] * 10)
for root, _, files in os.walk(sys.argv[1]):
    for f in files:
        p = os.path.join(root, f)
        try:
            size, res = resident(p)
        except OSError:
            continue
        t = tot[kind(f)]
        t[0] += 1; t[1] += size; t[2] += max(res, 0)
        if DEC:
            for i, v in enumerate(DEC[-1][1]):
                deciles[kind(f)][i] += v
            DEC.clear()
line = " ".join(f"{k}:{v[0]}f:{v[1]>>20}MB:{v[2]>>20}MB" for k, v in sorted(tot.items()))
print(f"resident {sys.argv[2]} | {line}", flush=True)
for k in ("par", "ord"):
    if k in deciles:
        print(f"   {k} by tenth of each file, MB: " + " ".join(str(v >> 20) for v in deciles[k]), flush=True)
