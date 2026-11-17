// LD_PRELOAD fault injector. SHIM_PATH: substring of the fd's path that is
// eligible. Arm files (one-shot, removed when they fire):
//   SHIM_WRITE_ARM    next write/writev to a matching fd fails EIO, 0 bytes
//   SHIM_PARTIAL_ARM  next write/writev writes half its bytes, the one
//                     after it to a matching fd fails EIO
//   SHIM_SYNC_ARM     next fdatasync/fsync of a matching fd fails EIO
//                     without syncing
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

static ssize_t (*rw)(int, const void *, size_t);
static ssize_t (*rwv)(int, const struct iovec *, int);
static int (*rfds)(int);
static int (*rfs)(int);
static int fail_next = 0;

static int match_fd(int fd) {
    const char *sub = getenv("SHIM_PATH");
    if (!sub) return 0;
    char p[64], buf[4096];
    snprintf(p, sizeof p, "/proc/self/fd/%d", fd);
    ssize_t n = readlink(p, buf, sizeof buf - 1);
    if (n < 0) return 0;
    buf[n] = 0;
    return strstr(buf, sub) != NULL;
}

static int armed(const char *var) {
    const char *f = getenv(var);
    if (!f) return 0;
    if (access(f, F_OK) == 0) {
        unlink(f);
        return 1;
    }
    return 0;
}

static void note(const char *what, int fd) {
    char p[64], buf[4096];
    snprintf(p, sizeof p, "/proc/self/fd/%d", fd);
    ssize_t n = readlink(p, buf, sizeof buf - 1);
    if (n < 0) n = 0;
    buf[n] = 0;
    char line[4300];
    int l = snprintf(line, sizeof line, "SHIM %s fd=%d %s\n", what, fd, buf);
    if (!rw) rw = dlsym(RTLD_NEXT, "write");
    rw(2, line, l);
}

ssize_t write(int fd, const void *b, size_t n) {
    if (!rw) rw = dlsym(RTLD_NEXT, "write");
    if (fd > 2 && match_fd(fd)) {
        if (fail_next) {
            fail_next = 0;
            note("write EIO (after partial)", fd);
            errno = EIO;
            return -1;
        }
        if (armed("SHIM_WRITE_ARM")) {
            note("write EIO", fd);
            errno = EIO;
            return -1;
        }
        if (n > 1 && armed("SHIM_PARTIAL_ARM")) {
            fail_next = 1;
            note("write partial", fd);
            return rw(fd, b, n / 2);
        }
    }
    return rw(fd, b, n);
}

ssize_t writev(int fd, const struct iovec *iov, int cnt) {
    if (!rwv) rwv = dlsym(RTLD_NEXT, "writev");
    if (!rw) rw = dlsym(RTLD_NEXT, "write");
    if (fd > 2 && match_fd(fd)) {
        if (fail_next) {
            fail_next = 0;
            note("writev EIO (after partial)", fd);
            errno = EIO;
            return -1;
        }
        if (armed("SHIM_WRITE_ARM")) {
            note("writev EIO", fd);
            errno = EIO;
            return -1;
        }
        size_t tot = 0;
        for (int i = 0; i < cnt; i++) tot += iov[i].iov_len;
        if (tot > 1 && cnt > 0 && armed("SHIM_PARTIAL_ARM")) {
            fail_next = 1;
            note("writev partial", fd);
            size_t half = iov[0].iov_len / 2;
            if (half == 0) half = 1;
            return rw(fd, iov[0].iov_base, half);
        }
    }
    return rwv(fd, iov, cnt);
}

int fdatasync(int fd) {
    if (!rfds) rfds = dlsym(RTLD_NEXT, "fdatasync");
    if (fd > 2 && match_fd(fd) && armed("SHIM_SYNC_ARM")) {
        note("fdatasync EIO", fd);
        errno = EIO;
        return -1;
    }
    return rfds(fd);
}

int fsync(int fd) {
    if (!rfs) rfs = dlsym(RTLD_NEXT, "fsync");
    if (fd > 2 && match_fd(fd) && armed("SHIM_SYNC_ARM")) {
        note("fsync EIO", fd);
        errno = EIO;
        return -1;
    }
    return rfs(fd);
}
