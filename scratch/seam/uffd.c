// userfaultfd pager probes. Build: gcc -O2 -pthread -o uffd uffd.c
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/userfaultfd.h>
#include <poll.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#define PG 4096UL
static inline uint64_t now(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec;
}
static void die(const char *s) { perror(s); exit(1); }
static int cmpu(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return x < y ? -1 : x > y;
}
static void stats(const char *name, uint64_t *v, size_t n, uint64_t wall, size_t pages) {
  qsort(v, n, sizeof *v, cmpu);
  double sum = 0;
  for (size_t i = 0; i < n; i++) sum += v[i];
  printf("%-44s n=%zu median=%.2fus p99=%.2fus mean=%.2fus wall/fault=%.2fus wall/page=%.3fus\n", name, n,
         v[n / 2] / 1e3, v[n * 99 / 100] / 1e3, sum / n / 1e3, wall / 1e3 / n, wall / 1e3 / pages);
}

static int uffd_open(int flags) { return syscall(SYS_userfaultfd, O_CLOEXEC | flags); }

static void avail(void) {
  int fd = uffd_open(0);
  printf("userfaultfd(0): %s\n", fd >= 0 ? "ok" : strerror(errno));
  int fd2 = uffd_open(UFFD_USER_MODE_ONLY);
  printf("userfaultfd(UFFD_USER_MODE_ONLY): %s\n", fd2 >= 0 ? "ok" : strerror(errno));
  int dev = open("/dev/userfaultfd", O_RDWR | O_CLOEXEC);
  if (dev < 0) printf("/dev/userfaultfd open: %s\n", strerror(errno));
  else {
    int f3 = ioctl(dev, USERFAULTFD_IOC_NEW, O_CLOEXEC);
    printf("/dev/userfaultfd USERFAULTFD_IOC_NEW: %s\n", f3 >= 0 ? "ok" : strerror(errno));
  }
  struct uffdio_api api = {.api = UFFD_API, .features = 0};
  if (ioctl(fd, UFFDIO_API, &api)) die("UFFDIO_API");
  printf("UFFDIO_API features=0x%llx ioctls=0x%llx\n", (unsigned long long)api.features, (unsigned long long)api.ioctls);
  struct { uint64_t b; const char *n; } F[] = {
      {UFFD_FEATURE_PAGEFAULT_FLAG_WP, "PAGEFAULT_FLAG_WP"}, {UFFD_FEATURE_EVENT_FORK, "EVENT_FORK"},
      {UFFD_FEATURE_EVENT_REMAP, "EVENT_REMAP"}, {UFFD_FEATURE_EVENT_REMOVE, "EVENT_REMOVE"},
      {UFFD_FEATURE_MISSING_HUGETLBFS, "MISSING_HUGETLBFS"}, {UFFD_FEATURE_MISSING_SHMEM, "MISSING_SHMEM"},
      {UFFD_FEATURE_EVENT_UNMAP, "EVENT_UNMAP"}, {UFFD_FEATURE_SIGBUS, "SIGBUS"},
      {UFFD_FEATURE_THREAD_ID, "THREAD_ID"}, {UFFD_FEATURE_MINOR_HUGETLBFS, "MINOR_HUGETLBFS"},
      {UFFD_FEATURE_MINOR_SHMEM, "MINOR_SHMEM"}, {UFFD_FEATURE_EXACT_ADDRESS, "EXACT_ADDRESS"},
      {UFFD_FEATURE_WP_HUGETLBFS_SHMEM, "WP_HUGETLBFS_SHMEM"}, {UFFD_FEATURE_WP_UNPOPULATED, "WP_UNPOPULATED"},
      {UFFD_FEATURE_POISON, "POISON"}, {UFFD_FEATURE_WP_ASYNC, "WP_ASYNC"},
#ifdef UFFD_FEATURE_MOVE
      {UFFD_FEATURE_MOVE, "MOVE"},
#endif
  };
  printf("features:");
  for (size_t i = 0; i < sizeof F / sizeof *F; i++) if (api.features & F[i].b) printf(" %s", F[i].n);
  printf("\n");
  // register ioctls per mapping type
  for (int k = 0; k < 2; k++) {
    int u = uffd_open(0);
    struct uffdio_api a = {.api = UFFD_API, .features = api.features & (UFFD_FEATURE_MINOR_SHMEM | UFFD_FEATURE_MISSING_SHMEM | UFFD_FEATURE_THREAD_ID)};
    if (ioctl(u, UFFDIO_API, &a)) die("api2");
    char *p;
    if (k == 0) p = mmap(0, 1 << 20, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    else {
      int m = memfd_create("x", 0); ftruncate(m, 1 << 20);
      p = mmap(0, 1 << 20, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
    }
    uint64_t modes[] = {UFFDIO_REGISTER_MODE_MISSING, UFFDIO_REGISTER_MODE_MINOR, UFFDIO_REGISTER_MODE_WP};
    const char *mn[] = {"MISSING", "MINOR", "WP"};
    for (int j = 0; j < 3; j++) {
      struct uffdio_register r = {.range = {(uintptr_t)p, 1 << 20}, .mode = modes[j]};
      int rc = ioctl(u, UFFDIO_REGISTER, &r);
      printf("  %s register %s: %s ioctls=0x%llx%s%s%s\n", k ? "memfd" : "anon", mn[j], rc ? strerror(errno) : "ok",
             (unsigned long long)r.ioctls, (r.ioctls >> _UFFDIO_COPY) & 1 ? " COPY" : "",
             (r.ioctls >> _UFFDIO_CONTINUE) & 1 ? " CONTINUE" : "", (r.ioctls >> _UFFDIO_WRITEPROTECT) & 1 ? " WRITEPROTECT" : "");
      struct uffdio_range ur = {(uintptr_t)p, 1 << 20};
      ioctl(u, UFFDIO_UNREGISTER, &ur);
    }
  }
}

/* ---------- fault benchmark ---------- */
enum { M_COPY, M_MINOR_FILL, M_MINOR_PRE };
static int g_uffd, g_mode, g_block, g_shm;
static char *g_base, *g_alias, *g_src;
static size_t g_srcpages = 16384; // 64 MiB source
static size_t g_npages;
static volatile int g_stop;
static uint64_t g_handled;

static void fill(char *dst, size_t pidx, size_t n) {
  for (size_t j = 0; j < n; j++) {
    memcpy(dst + j * PG, g_src + ((pidx + j) % g_srcpages) * PG, PG);
    *(uint64_t *)(dst + j * PG) = pidx + j;
  }
}

static void pin_self(int cpu) {
  cpu_set_t s; CPU_ZERO(&s); CPU_SET(cpu, &s); pthread_setaffinity_np(pthread_self(), sizeof s, &s);
}
static void *handler(void *arg) {
  if (getenv("PIN")) pin_self(atoi(getenv("PIN")) == 1 ? 0 : 3 - (int)(uintptr_t)arg);
  size_t bb = (size_t)g_block * PG;
  char *stage = aligned_alloc(PG, bb);
  memset(stage, 1, bb);
  struct pollfd pf = {.fd = g_uffd, .events = POLLIN};
  while (!g_stop) {
    struct uffd_msg m;
    ssize_t r = read(g_uffd, &m, sizeof m);
    if (r < 0) {
      if (errno == EAGAIN) { if (!getenv("SPIN")) poll(&pf, 1, 50); continue; }
      if (errno == EINTR) continue;
      die("read uffd");
    }
    if (m.event != UFFD_EVENT_PAGEFAULT) continue;
    size_t off = ((uintptr_t)m.arg.pagefault.address - (uintptr_t)g_base) / bb * bb;
    size_t pidx = off / PG, n = g_block;
    if (pidx + n > g_npages) n = g_npages - pidx;
    if (g_mode == M_COPY) {
      fill(stage, pidx, n);
      struct uffdio_copy c = {.dst = (uintptr_t)g_base + off, .src = (uintptr_t)stage, .len = n * PG};
      if (ioctl(g_uffd, UFFDIO_COPY, &c)) {
        if (errno == EEXIST) { struct uffdio_range w = {(uintptr_t)g_base + off, n * PG}; ioctl(g_uffd, UFFDIO_WAKE, &w); }
        else die("UFFDIO_COPY");
      }
    } else {
      if (g_mode == M_MINOR_FILL) fill(g_alias + off, pidx, n);
      struct uffdio_continue c = {.range = {(uintptr_t)g_base + off, n * PG}};
      if (ioctl(g_uffd, UFFDIO_CONTINUE, &c)) {
        if (errno == EEXIST) { struct uffdio_range w = {(uintptr_t)g_base + off, n * PG}; ioctl(g_uffd, UFFDIO_WAKE, &w); }
        else die("UFFDIO_CONTINUE");
      }
    }
    __atomic_fetch_add(&g_handled, 1, __ATOMIC_RELAXED);
  }
  return 0;
}

struct fargs { size_t first, nblk; uint64_t *lat; size_t bad; };
static void *faulter(void *arg) {
  struct fargs *a = arg;
  if (getenv("PIN")) pin_self(atoi(getenv("PIN")) == 1 ? 0 : (int)(a->first / (a->nblk ? a->nblk : 1)));
  size_t *perm = malloc(a->nblk * sizeof *perm);
  for (size_t i = 0; i < a->nblk; i++) perm[i] = i;
  unsigned s = (unsigned)a->first * 2654435761u + 1;
  for (size_t i = a->nblk - 1; i > 0; i--) { size_t j = rand_r(&s) % (i + 1); size_t t = perm[i]; perm[i] = perm[j]; perm[j] = t; }
  for (size_t i = 0; i < a->nblk; i++) {
    size_t pidx = (a->first + perm[i]) * g_block;
    volatile uint64_t *p = (volatile uint64_t *)(g_base + pidx * PG);
    uint64_t t0 = now();
    uint64_t v = *p;
    uint64_t t1 = now();
    a->lat[i] = t1 - t0;
    if (v != pidx) a->bad++;
  }
  // verify the rest of each block (fault-around pages) without timing
  for (size_t i = 0; i < a->nblk; i++)
    for (int j = 1; j < g_block; j++) {
      size_t pidx = (a->first + i) * g_block + j;
      if (pidx < g_npages && *(volatile uint64_t *)(g_base + pidx * PG) != pidx) a->bad++;
    }
  return 0;
}

static void setup_region(int shm, int mode, size_t npages) {
  g_shm = shm; g_mode = mode; g_npages = npages;
  size_t len = npages * PG;
  g_uffd = uffd_open(O_NONBLOCK);
  if (g_uffd < 0) die("userfaultfd");
  struct uffdio_api api = {.api = UFFD_API, .features = shm ? (UFFD_FEATURE_MISSING_SHMEM | (mode != M_COPY ? UFFD_FEATURE_MINOR_SHMEM : 0)) : 0};
  if (ioctl(g_uffd, UFFDIO_API, &api)) die("UFFDIO_API");
  if (shm) {
    int m = memfd_create("pager", 0);
    if (ftruncate(m, len)) die("ftruncate");
    g_base = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
    if (mode != M_COPY) g_alias = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
    if (mode == M_MINOR_PRE) for (size_t i = 0; i < npages; i++) fill(g_alias + i * PG, i, 1);
  } else {
    g_base = mmap(0, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  }
  if (g_base == MAP_FAILED) die("mmap");
  madvise(g_base, len, MADV_NOHUGEPAGE);
  uint64_t rm = mode == M_COPY ? UFFDIO_REGISTER_MODE_MISSING
              : mode == M_MINOR_PRE ? UFFDIO_REGISTER_MODE_MINOR
              : (UFFDIO_REGISTER_MODE_MISSING | UFFDIO_REGISTER_MODE_MINOR);
  struct uffdio_register r = {.range = {(uintptr_t)g_base, len}, .mode = rm};
  if (ioctl(g_uffd, UFFDIO_REGISTER, &r)) die("UFFDIO_REGISTER");
}

static void faultbench(int shm, int mode, int block, int nf, int nh, size_t nfaults) {
  g_block = block; g_stop = 0; g_handled = 0;
  size_t per = nfaults / nf;
  nfaults = per * nf;
  setup_region(shm, mode, nfaults * block);
  pthread_t h[8], f[8];
  struct fargs fa[8];
  for (int i = 0; i < nh; i++) pthread_create(&h[i], 0, handler, (void *)(uintptr_t)i);
  uint64_t *lat = malloc(nfaults * sizeof *lat);
  uint64_t t0 = now();
  for (int i = 0; i < nf; i++) {
    fa[i] = (struct fargs){.first = i * per, .nblk = per, .lat = lat + i * per};
    pthread_create(&f[i], 0, faulter, &fa[i]);
  }
  for (int i = 0; i < nf; i++) pthread_join(f[i], 0);
  uint64_t wall = 0; // wall excludes the untimed verify pass: approximate with sum of lat / nf instead
  (void)t0;
  size_t bad = 0;
  for (int i = 0; i < nf; i++) bad += fa[i].bad;
  g_stop = 1;
  for (int i = 0; i < nh; i++) pthread_join(h[i], 0);
  for (size_t i = 0; i < nfaults; i++) wall += lat[i];
  wall /= nf; // per-thread-serial time; aggregate throughput = nfaults/wall
  char name[128];
  snprintf(name, sizeof name, "%s %s blk=%d F=%d H=%d", shm ? "memfd" : "anon",
           mode == M_COPY ? "COPY" : mode == M_MINOR_FILL ? "MISSING+alias-fill+CONTINUE" : "MINOR(prefilled)+CONTINUE", block, nf, nh);
  stats(name, lat, nfaults, wall, nfaults * block);
  printf("   handled=%llu bad=%zu\n", (unsigned long long)g_handled, bad);
  munmap(g_base, g_npages * PG);
  if (g_alias) munmap(g_alias, g_npages * PG), g_alias = 0;
  close(g_uffd);
  free(lat);
}

/* ---------- eviction ---------- */
static void evict(int shm) {
  size_t npages = 65536; // 256 MiB
  g_block = 1; g_stop = 0; g_handled = 0;
  setup_region(shm, M_COPY, npages);
  // populate with UFFDIO_COPY from this thread, no fault
  char *stage = aligned_alloc(PG, 1 << 20);
  for (size_t off = 0; off < npages * PG; off += 1 << 20) {
    fill(stage, off / PG, 256);
    struct uffdio_copy c = {.dst = (uintptr_t)g_base + off, .src = (uintptr_t)stage, .len = 1 << 20, .mode = UFFDIO_COPY_MODE_DONTWAKE};
    if (ioctl(g_uffd, UFFDIO_COPY, &c)) die("prepop copy");
  }
  int memfd = -1;
  if (shm) { // find the memfd behind g_base: reopen via /proc/self/map_files is messy; use MADV_REMOVE
  }
  (void)memfd;
  const char *how = shm ? "MADV_REMOVE" : "MADV_DONTNEED";
  int adv = shm ? MADV_REMOVE : MADV_DONTNEED;
  // 1 MiB calls
  uint64_t t0 = now();
  size_t half = npages * PG / 2, n1 = 0;
  for (size_t off = 0; off < half; off += 1 << 20, n1++)
    if (madvise(g_base + off, 1 << 20, adv)) die("madvise 1M");
  uint64_t t1 = now();
  printf("%s %s 1MiB: %.2fus/call (%.3fus/page) n=%zu\n", shm ? "memfd" : "anon", how, (t1 - t0) / 1e3 / n1, (t1 - t0) / 1e3 / n1 / 256, n1);
  // single page calls
  size_t n2 = 0;
  t0 = now();
  for (size_t off = half; off < half + (64 << 20); off += PG, n2++)
    if (madvise(g_base + off, PG, adv)) die("madvise 4K");
  t1 = now();
  printf("%s %s 4KiB: %.2fus/call n=%zu\n", shm ? "memfd" : "anon", how, (t1 - t0) / 1e3 / n2, n2);
  // a call over an already-empty page
  t0 = now();
  for (size_t off = half; off < half + (64 << 20); off += PG) madvise(g_base + off, PG, adv);
  t1 = now();
  printf("%s %s 4KiB already empty: %.2fus/call\n", shm ? "memfd" : "anon", how, (t1 - t0) / 1e3 / n2);
  // confirm refault through handler
  pthread_t h;
  pthread_create(&h, 0, handler, 0);
  size_t bad = 0, cnt = 0;
  for (size_t p = 0; p < npages; p += 7, cnt++)
    if (*(volatile uint64_t *)(g_base + p * PG) != p) bad++;
  g_stop = 1;
  pthread_join(h, 0);
  printf("   refault check: touched=%zu handled=%llu bad=%zu (pages beyond evicted span were never evicted, handled < touched expected)\n",
         cnt, (unsigned long long)g_handled, bad);
  munmap(g_base, npages * PG);
  close(g_uffd);
}

/* ---------- syscalls with buffer in a missing page ---------- */
static void sysbuf(int usermode) {
  size_t npages = 16;
  g_block = 1; g_stop = 0; g_handled = 0;
  g_uffd = uffd_open(O_NONBLOCK | (usermode ? UFFD_USER_MODE_ONLY : 0));
  if (g_uffd < 0) die("uffd");
  struct uffdio_api api = {.api = UFFD_API, .features = UFFD_FEATURE_MISSING_SHMEM};
  if (ioctl(g_uffd, UFFDIO_API, &api)) die("api");
  int m = memfd_create("s", 0); ftruncate(m, npages * PG);
  g_base = mmap(0, npages * PG, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
  g_npages = npages; g_mode = M_COPY;
  struct uffdio_register r = {.range = {(uintptr_t)g_base, npages * PG}, .mode = UFFDIO_REGISTER_MODE_MISSING};
  if (ioctl(g_uffd, UFFDIO_REGISTER, &r)) die("reg");
  pthread_t h;
  pthread_create(&h, 0, handler, 0);
  char path[] = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/sysbuf.tmp";
  int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
  ssize_t w = write(fd, g_base, PG); // source buffer in missing page: kernel copy_from_user
  int we = errno;
  uint64_t v = 0; pread(fd, &v, 8, 0);
  printf("%s write(fd, missing page): ret=%zd %s; file word=%llu (expect 0)\n", usermode ? "USER_MODE_ONLY" : "default", w,
         w < 0 ? strerror(we) : "", (unsigned long long)v);
  char buf[PG]; memset(buf, 0x5a, PG);
  pwrite(fd, buf, PG, 0);
  ssize_t rr = pread(fd, g_base + 3 * PG, PG, 0); // dest buffer in missing page: copy_to_user write fault
  int re = errno;
  printf("%s read(fd, into missing page): ret=%zd %s; byte=%#x\n", usermode ? "USER_MODE_ONLY" : "default", rr,
         rr < 0 ? strerror(re) : "", rr > 0 ? (unsigned char)g_base[3 * PG + 100] : 0);
  printf("   handled=%llu\n", (unsigned long long)g_handled);
  g_stop = 1; pthread_join(h, 0);
  unlink(path); close(fd); close(g_uffd);
  munmap(g_base, npages * PG);
}

/* ---------- baselines ---------- */
static void baselines(void) {
  size_t n = 100000, len = n * PG;
  uint64_t *lat = malloc(n * sizeof *lat);
  for (int k = 0; k < 4; k++) {
    char *p;
    if (k < 2) { int m = memfd_create("b", 0); ftruncate(m, len); p = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0); }
    else p = mmap(0, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    madvise(p, len, MADV_NOHUGEPAGE);
    unsigned s = 7;
    size_t *perm = malloc(n * sizeof *perm);
    for (size_t i = 0; i < n; i++) perm[i] = i;
    for (size_t i = n - 1; i > 0; i--) { size_t j = rand_r(&s) % (i + 1); size_t t = perm[i]; perm[i] = perm[j]; perm[j] = t; }
    int write_ = k & 1;
    uint64_t W = 0;
    for (size_t i = 0; i < n; i++) {
      volatile char *q = p + perm[i] * PG;
      uint64_t t0 = now();
      if (write_) *q = 1; else (void)*q;
      lat[i] = now() - t0; W += lat[i];
    }
    const char *nm[] = {"memfd first touch read (no uffd)", "memfd first touch write (no uffd)", "anon first touch read (zero page)", "anon first touch write (no uffd)"};
    stats(nm[k], lat, n, W, n);
    // hits on the same pages
    W = 0;
    for (size_t i = 0; i < n; i++) {
      volatile char *q = p + perm[i] * PG;
      uint64_t t0 = now(); (void)*q; lat[i] = now() - t0; W += lat[i];
    }
    if (k == 1) stats("   hit: read of mapped memfd page (random)", lat, n, W, n);
    munmap(p, len); free(perm);
  }
  // timer overhead
  uint64_t W = 0;
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); lat[i] = now() - t0; W += lat[i]; }
  stats("clock_gettime pair overhead", lat, n, W, n);
  // pread 4 KiB from page-cached file (disk file and memfd)
  for (int k = 0; k < 2; k++) {
    size_t flen = 256 << 20;
    int fd;
    char path[] = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/pread.tmp";
    if (k == 0) fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600); else fd = memfd_create("pr", 0);
    char *b = aligned_alloc(PG, 1 << 20); memset(b, 3, 1 << 20);
    for (size_t o = 0; o < flen; o += 1 << 20) pwrite(fd, b, 1 << 20, o);
    if (k == 0) fsync(fd);
    char *dst = aligned_alloc(PG, PG); memset(dst, 0, PG);
    for (size_t o = 0; o < flen; o += PG) pread(fd, dst, PG, o); // warm
    unsigned s = 9; W = 0;
    for (size_t i = 0; i < n; i++) {
      off_t o = (off_t)(rand_r(&s) % (flen / PG)) * PG;
      uint64_t t0 = now(); if (pread(fd, dst, PG, o) != PG) die("pread"); lat[i] = now() - t0; W += lat[i];
    }
    stats(k == 0 ? "pread 4KiB, ext4? file in page cache" : "pread 4KiB, memfd", lat, n, W, n);
    close(fd); if (k == 0) unlink(path);
  }
  // memcpy 4 KiB cost
  char *src = aligned_alloc(PG, 64 << 20), *dst = aligned_alloc(PG, PG);
  memset(src, 1, 64 << 20);
  uint64_t t0 = now();
  for (size_t i = 0; i < n; i++) { memcpy(dst, src + (i % 16384) * PG, PG); __asm__ volatile("" ::: "memory"); }
  printf("memcpy 4KiB (64MiB source): %.3fus\n", (now() - t0) / 1e3 / n);
  free(lat);
}

/* ---------- UFFD_FEATURE_SIGBUS: synchronous, the faulting thread installs ---------- */
static __thread char *b_stage;
static void busf(int sig, siginfo_t *si, void *uc) {
  (void)sig; (void)uc;
  char *a = (char *)((uintptr_t)si->si_addr & ~(PG - 1));
  size_t pidx = (a - g_base) / PG;
  if (!b_stage) b_stage = aligned_alloc(PG, PG);
  fill(b_stage, pidx, 1);
  struct uffdio_copy c = {.dst = (uintptr_t)a, .src = (uintptr_t)b_stage, .len = PG};
  if (ioctl(g_uffd, UFFDIO_COPY, &c) && errno != EEXIST) _exit(3);
}
static void sigbus_bench(int shm, int nf, size_t n) {
  struct sigaction sa = {.sa_sigaction = busf, .sa_flags = SA_SIGINFO};
  sigaction(SIGBUS, &sa, 0);
  g_block = 1; g_mode = M_COPY; g_npages = n;
  size_t len = n * PG;
  g_uffd = uffd_open(0);
  struct uffdio_api api = {.api = UFFD_API, .features = UFFD_FEATURE_SIGBUS | (shm ? UFFD_FEATURE_MISSING_SHMEM : 0)};
  if (ioctl(g_uffd, UFFDIO_API, &api)) die("api sigbus");
  if (shm) { int m = memfd_create("sb", 0); if (ftruncate(m, len)) die("ft"); g_base = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0); }
  else g_base = mmap(0, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  madvise(g_base, len, MADV_NOHUGEPAGE);
  struct uffdio_register r = {.range = {(uintptr_t)g_base, len}, .mode = UFFDIO_REGISTER_MODE_MISSING};
  if (ioctl(g_uffd, UFFDIO_REGISTER, &r)) die("reg");
  uint64_t *lat = malloc(n * sizeof *lat);
  pthread_t f[8]; struct fargs fa[8]; size_t per = n / nf;
  for (int i = 0; i < nf; i++) { fa[i] = (struct fargs){.first = i * per, .nblk = per, .lat = lat + i * per}; pthread_create(&f[i], 0, faulter, &fa[i]); }
  size_t bad = 0; uint64_t W = 0;
  for (int i = 0; i < nf; i++) { pthread_join(f[i], 0); bad += fa[i].bad; }
  for (size_t i = 0; i < per * nf; i++) W += lat[i];
  char nm[64]; snprintf(nm, sizeof nm, "%s UFFD SIGBUS in-thread COPY F=%d", shm ? "memfd" : "anon", nf);
  stats(nm, lat, per * nf, W / nf, per * nf);
  printf("   bad=%zu\n", bad);
  // syscall with buffer in a missing page under SIGBUS
  int p[2]; if (pipe(p)) die("pipe");
  madvise(g_base, PG, shm ? MADV_REMOVE : MADV_DONTNEED);
  ssize_t w = write(p[1], g_base, PG);
  printf("   write(pipe, missing page) under FEATURE_SIGBUS: ret=%zd %s\n", w, w < 0 ? strerror(errno) : "");
}

/* ---------- SIGSEGV + mprotect pager ---------- */
static char *s_base, *s_alias; static size_t s_n;
static void segv(int sig, siginfo_t *si, void *uc) {
  (void)sig; (void)uc;
  char *a = (char *)((uintptr_t)si->si_addr & ~(PG - 1));
  if (a < s_base || a >= s_base + s_n * PG) { signal(SIGSEGV, SIG_DFL); return; }
  size_t pidx = (a - s_base) / PG;
  fill(s_alias + pidx * PG, pidx, 1);
  mprotect(a, PG, PROT_READ);
}
struct sargs { size_t first, n; uint64_t *lat; size_t bad; };
static void *sfaulter(void *arg) {
  struct sargs *a = arg;
  unsigned s = a->first + 3;
  size_t *perm = malloc(a->n * sizeof *perm);
  for (size_t i = 0; i < a->n; i++) perm[i] = i;
  for (size_t i = a->n - 1; i > 0; i--) { size_t j = rand_r(&s) % (i + 1); size_t t = perm[i]; perm[i] = perm[j]; perm[j] = t; }
  for (size_t i = 0; i < a->n; i++) {
    size_t pidx = a->first + perm[i];
    uint64_t t0 = now(); uint64_t v = *(volatile uint64_t *)(s_base + pidx * PG); a->lat[i] = now() - t0;
    if (v != pidx) a->bad++;
  }
  return 0;
}
static void sigsegv_bench(int nf, size_t n) {
  struct sigaction sa = {.sa_sigaction = segv, .sa_flags = SA_SIGINFO};
  sigaction(SIGSEGV, &sa, 0);
  s_n = n;
  int m = memfd_create("sg", 0); ftruncate(m, n * PG);
  s_base = mmap(0, n * PG, PROT_NONE, MAP_SHARED, m, 0);
  s_alias = mmap(0, n * PG, PROT_READ | PROT_WRITE, MAP_SHARED, m, 0);
  uint64_t *lat = malloc(n * sizeof *lat);
  pthread_t t[8]; struct sargs sa_[8];
  size_t per = n / nf;
  for (int i = 0; i < nf; i++) { sa_[i] = (struct sargs){.first = i * per, .n = per, .lat = lat + i * per}; pthread_create(&t[i], 0, sfaulter, &sa_[i]); }
  size_t bad = 0; uint64_t W = 0;
  for (int i = 0; i < nf; i++) { pthread_join(t[i], 0); bad += sa_[i].bad; }
  for (size_t i = 0; i < per * nf; i++) W += lat[i];
  char nm[64]; snprintf(nm, sizeof nm, "SIGSEGV+fill+mprotect F=%d", nf);
  stats(nm, lat, per * nf, W / nf, per * nf);
  printf("   bad=%zu\n", bad);
  // eviction by mprotect back to NONE, single page, while other threads idle
  uint64_t t0 = now();
  for (size_t i = 0; i < per * nf; i += 2) mprotect(s_base + i * PG, PG, PROT_NONE);
  printf("   mprotect(PROT_NONE) 4KiB evict: %.2fus/call\n", (now() - t0) / 1e3 / (per * nf / 2));
  // syscall into PROT_NONE page
  int fd = open("/dev/null", O_WRONLY);
  ssize_t w = write(fd, s_base + 0 * PG, PG); // page 0 now NONE
  printf("   write(/dev/null, PROT_NONE page): ret=%zd %s (dev/null does not read the buffer)\n", w, w < 0 ? strerror(errno) : "");
  int p[2]; pipe(p);
  w = write(p[1], s_base + 0 * PG, PG);
  printf("   write(pipe, PROT_NONE page): ret=%zd %s\n", w, w < 0 ? strerror(errno) : "");
  munmap(s_base, n * PG); munmap(s_alias, n * PG); free(lat);
  signal(SIGSEGV, SIG_DFL);
}

static void *busy(void *arg) {
  struct sched_param sp = {0}; pthread_setschedparam(pthread_self(), SCHED_IDLE, &sp);
  pin_self((int)(uintptr_t)arg);
  for (;;) __asm__ volatile("pause");
}
int main(int argc, char **argv) {
  if (getenv("BUSY")) for (int i = 0; i < 4; i++) { pthread_t t; pthread_create(&t, 0, busy, (void *)(uintptr_t)i); }
  g_src = aligned_alloc(PG, g_srcpages * PG);
  for (size_t i = 0; i < g_srcpages * PG; i++) g_src[i] = (char)(i * 131);
  const char *c = argc > 1 ? argv[1] : "all";
  if (!strcmp(c, "avail")) avail();
  else if (!strcmp(c, "fault")) {
    // fault <shm> <mode> <block> <nf> <nh> <nfaults>
    faultbench(atoi(argv[2]), atoi(argv[3]), atoi(argv[4]), atoi(argv[5]), atoi(argv[6]), atol(argv[7]));
  } else if (!strcmp(c, "evict")) evict(atoi(argv[2]));
  else if (!strcmp(c, "sysbuf")) sysbuf(atoi(argv[2]));
  else if (!strcmp(c, "base")) baselines();
  else if (!strcmp(c, "sigbus")) sigbus_bench(atoi(argv[2]), atoi(argv[3]), atol(argv[4]));
  else if (!strcmp(c, "segv")) sigsegv_bench(atoi(argv[2]), atol(argv[3]));
  return 0;
}
