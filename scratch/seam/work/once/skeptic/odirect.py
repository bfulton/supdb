import os, mmap, sys
p = sys.argv[1]
with open(p, "wb") as f: f.write(os.urandom(1 << 16))
fd = os.open(p, os.O_RDONLY | os.O_DIRECT)
buf = mmap.mmap(-1, 4096)
n = os.preadv(fd, [buf], 4096)
print("O_DIRECT open and aligned pread ok:", n)
try:
    os.preadv(fd, [bytearray(4096)], 100)
    print("unaligned offset accepted (O_DIRECT not enforced?)")
except OSError as e:
    print("unaligned offset refused:", e.strerror, "-> O_DIRECT is enforced here")
os.close(fd); os.remove(p)
