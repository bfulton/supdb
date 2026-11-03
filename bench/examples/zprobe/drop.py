"""drop.py FILE...: ask the kernel to drop each file's clean pages from the page cache. A dirty page stays; sync first."""
import os
import sys

for path in sys.argv[1:]:
    fd = os.open(path, os.O_RDONLY)
    os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
    os.close(fd)
