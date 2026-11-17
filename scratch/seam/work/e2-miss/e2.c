// E2: the cost of a pager miss, as one operation.
// Build: gcc -O2 -pthread -o e2 e2.c -lcrypto
//
// Files (made once by `e2 setup DIR`, reused):
//   DIR/env.bin  today's envelope: a 4096-byte header page, then N stored chunks of
//                4096 bytes, each 4080 bytes of AES-256-GCM ciphertext and its 16-byte
//                tag; nonce id[0..8] || chunk u32 LE; AAD the 48-byte header
//                (crypt.rs: DATA_START 4096, stored_off = 4096 + c<<12, nonce()).
//   DIR/pg.bin   the design's page file (design.md section 3): data page p at
//                E(e) + 2 MiB + (p & 0xFFFF)*4096, E(e) = 1 MiB + e*258 MiB, its tag in a
//                32-byte entry of group g = p>>7's tag page at E(e) + (g & 511)*4096,
//                each tag page itself sealed (its MAC kept apart, as FINAL would carry it).
// Every run maps both read-only shared with MADV_RANDOM (as pager.rs Back::open does)
// and reads them in once (MADV_POPULATE_READ), so the source is in the page cache and
// mapped. The plaintext backing is a memfd mapped shared with MADV_NOHUGEPAGE
// (pager.rs map_shared). Between rounds the backing is given back with MADV_REMOVE in
// 1 MiB units, timed apart. Each op ends with the reader's load of the page through the
// backing mapping and a check of the plaintext's first word.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <immintrin.h>
#include <openssl/evp.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#ifndef MADV_POPULATE_READ
#define MADV_POPULATE_READ 22
#endif
#ifndef MADV_POPULATE_WRITE
#define MADV_POPULATE_WRITE 23
#endif

#define PG 4096UL
#define CH 4080UL
#define TAGL 16
#define MiB (1UL << 20)
#define RANGE 64

static size_t N = 102400; // stored pages; >= 100,000
static const char *DIR_;

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

// --- layout -------------------------------------------------------------------------
static inline uint64_t ext_base(uint64_t e) { return MiB + e * 258 * MiB; }
static inline uint64_t data_off(uint64_t p) { return ext_base(p >> 16) + 2 * MiB + (p & 0xFFFF) * PG; }
static inline uint64_t tag_off(uint64_t g) { return ext_base(g >> 9) + (g & 511) * PG; }
static inline uint64_t env_off(uint64_t c) { return 4096 + (c << 12); }
static inline uint32_t ctr_data(uint64_t p) { return (uint32_t)(p + (p >> 7)); }
static inline uint32_t ctr_tag(uint64_t g) { return (uint32_t)(g * 129 + 128); }
static inline uint64_t want_env(uint64_t c) { return c ^ 0xE0E0E0E0E0E0E0E0ull; }
static inline uint64_t want_pg(uint64_t p) { return p ^ 0xA5A5A5A5A5A5A5A5ull; }

typedef struct { uint8_t tag[16]; uint32_t ver; uint8_t rsv[12]; } tent; // 32 B

static uint8_t KEY_ENV[32], KEY_S[32], FILE_ID[16], ENV_HDR[48];

static void nonce_env(uint8_t n[12], uint64_t c) { memcpy(n, ENV_HDR + 8, 8); uint32_t x = (uint32_t)c; memcpy(n + 8, &x, 4); }
static void nonce_dom(uint8_t n[12], uint8_t dom, uint64_t idx, uint32_t ctr) {
  n[0] = dom;
  for (int i = 0; i < 7; i++) n[1 + i] = (uint8_t)(idx >> (8 * (6 - i)));
  memcpy(n + 8, &ctr, 4);
}
static int aad_data(uint8_t a[30], uint64_t p, uint32_t ctr) {
  memcpy(a, FILE_ID, 16); memcpy(a + 16, &p, 8); memcpy(a + 24, &ctr, 4); uint16_t v = 4096; memcpy(a + 28, &v, 2);
  return 30;
}
static int aad_tag(uint8_t a[28], uint64_t g, uint32_t ctr) {
  memcpy(a, FILE_ID, 16); memcpy(a + 16, &g, 8); memcpy(a + 24, &ctr, 4);
  return 28;
}

// --- cipher (OpenSSL EVP, AES-256-GCM) ------------------------------------------------
static EVP_CIPHER_CTX *ctx_new(const uint8_t *key, int enc) {
  EVP_CIPHER_CTX *c = EVP_CIPHER_CTX_new();
  if (!c) die("ctx");
  if (enc ? !EVP_EncryptInit_ex(c, EVP_aes_256_gcm(), NULL, key, NULL) : !EVP_DecryptInit_ex(c, EVP_aes_256_gcm(), NULL, key, NULL)) die("init");
  return c;
}
static void gcm_enc(EVP_CIPHER_CTX *c, const uint8_t iv[12], const uint8_t *aad, int aadl, uint8_t *buf, int n, uint8_t tag[16]) {
  int l;
  if (!EVP_EncryptInit_ex(c, NULL, NULL, NULL, iv)) die("e iv");
  if (aadl && !EVP_EncryptUpdate(c, NULL, &l, aad, aadl)) die("e aad");
  if (!EVP_EncryptUpdate(c, buf, &l, buf, n)) die("e upd");
  if (!EVP_EncryptFinal_ex(c, buf + l, &l)) die("e fin");
  if (!EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_GET_TAG, 16, tag)) die("e tag");
}
static inline int gcm_dec(EVP_CIPHER_CTX *c, const uint8_t iv[12], const uint8_t *aad, int aadl, const uint8_t *in, uint8_t *out, int n,
                          const uint8_t tag[16]) {
  int l, l2;
  if (!EVP_DecryptInit_ex(c, NULL, NULL, NULL, iv)) return 0;
  if (aadl && !EVP_DecryptUpdate(c, NULL, &l, aad, aadl)) return 0;
  if (!EVP_DecryptUpdate(c, out, &l, in, n)) return 0;
  if (!EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_SET_TAG, 16, (void *)tag)) return 0;
  return EVP_DecryptFinal_ex(c, out + l, &l2) > 0;
}

// --- rng ------------------------------------------------------------------------------
typedef struct { uint64_t s[4]; } rng;
static inline uint64_t rotl(uint64_t x, int k) { return (x << k) | (x >> (64 - k)); }
static uint64_t rnext(rng *r) {
  uint64_t *s = r->s, res = rotl(s[1] * 5, 7) * 9, t = s[1] << 17;
  s[2] ^= s[0]; s[3] ^= s[1]; s[1] ^= s[2]; s[0] ^= s[3]; s[2] ^= t; s[3] = rotl(s[3], 45);
  return res;
}
static void rseed(rng *r, uint64_t seed) {
  for (int i = 0; i < 4; i++) { seed += 0x9e3779b97f4a7c15ull; uint64_t z = seed; z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ull; z = (z ^ (z >> 27)) * 0x94d049bb133111ebull; r->s[i] = z ^ (z >> 31); }
}
static void shuffle(uint32_t *a, size_t n, rng *r) {
  for (size_t i = n - 1; i > 0; i--) { size_t j = rnext(r) % (i + 1); uint32_t t = a[i]; a[i] = a[j]; a[j] = t; }
}
static void fill_pattern(uint8_t *b, size_t n, uint64_t seed) {
  rng r; rseed(&r, seed);
  for (size_t i = 0; i + 8 <= n; i += 8) { uint64_t v = rnext(&r); memcpy(b + i, &v, 8); }
  for (size_t i = n & ~7UL; i < n; i++) b[i] = (uint8_t)i;
}

static void keys(void) {
  memset(KEY_ENV, 0x5a, 32);
  memset(KEY_S, 0x33, 32);
  for (int i = 0; i < 16; i++) FILE_ID[i] = (uint8_t)(0xF0 + i);
  memcpy(ENV_HDR, "SUPDBENV", 8);
  for (int i = 8; i < 48; i++) ENV_HDR[i] = (uint8_t)(i * 7 + 1);
}

static size_t G; // groups
static size_t pg_len(void) { return data_off(N - 1) + PG; }
static size_t env_len(void) { return env_off(N - 1) + PG; }

static void path(char *out, const char *name) { snprintf(out, 4096, "%s/%s", DIR_, name); }

static void setup(void) {
  char p[4096];
  EVP_CIPHER_CTX *ce = ctx_new(KEY_ENV, 1), *cs = ctx_new(KEY_S, 1);
  uint8_t *buf = aligned_alloc(PG, MiB);
  // envelope
  path(p, "env.bin");
  int fd = open(p, O_RDWR | O_CREAT | O_TRUNC, 0600);
  if (fd < 0) die("open env");
  memset(buf, 0, PG); memcpy(buf, ENV_HDR, 48);
  if (pwrite(fd, buf, PG, 0) != (ssize_t)PG) die("w hdr");
  for (size_t c0 = 0; c0 < N; c0 += 256) {
    size_t n = N - c0 < 256 ? N - c0 : 256;
    for (size_t j = 0; j < n; j++) {
      uint64_t c = c0 + j; uint8_t *s = buf + j * PG, iv[12];
      fill_pattern(s, CH, c * 2 + 1); uint64_t w = want_env(c); memcpy(s, &w, 8);
      nonce_env(iv, c);
      gcm_enc(ce, iv, ENV_HDR, 48, s, CH, s + CH);
    }
    if (pwrite(fd, buf, n * PG, env_off(c0)) != (ssize_t)(n * PG)) die("w env");
  }
  if (fsync(fd)) die("fsync env");
  close(fd);
  // page file
  path(p, "pg.bin");
  fd = open(p, O_RDWR | O_CREAT | O_TRUNC, 0600);
  if (fd < 0) die("open pg");
  if (ftruncate(fd, pg_len())) die("ft pg");
  memset(buf, 0, PG); memcpy(buf, "SUPDBPF2", 8); memcpy(buf + 16, FILE_ID, 16);
  if (pwrite(fd, buf, PG, 0) != (ssize_t)PG) die("w hdrA");
  G = (N + 127) / 128;
  tent *tier = calloc(N, sizeof(tent));
  uint8_t (*mac)[16] = calloc(G, 16);
  for (size_t p0 = 0; p0 < N; p0 += 256) { // 1 MiB aligned pieces (256 pages never cross an extent)
    size_t n = N - p0 < 256 ? N - p0 : 256;
    for (size_t j = 0; j < n; j++) {
      uint64_t q = p0 + j; uint8_t *s = buf + j * PG, iv[12], aad[30];
      fill_pattern(s, PG, q * 2 + 2); uint64_t w = want_pg(q); memcpy(s, &w, 8);
      uint32_t ctr = ctr_data(q);
      nonce_dom(iv, 0x10, q, ctr);
      int al = aad_data(aad, q, ctr);
      gcm_enc(cs, iv, aad, al, s, PG, tier[q].tag);
      tier[q].ver = ctr;
    }
    if (pwrite(fd, buf, n * PG, data_off(p0)) != (ssize_t)(n * PG)) die("w pg");
  }
  // tag pages, one 4 KiB write each, sealed
  for (size_t g = 0; g < G; g++) {
    memset(buf, 0, PG);
    for (size_t j = 0; j < 128 && g * 128 + j < N; j++) memcpy(buf + j * 32, &tier[g * 128 + j], 32);
    uint8_t iv[12], aad[28]; uint32_t ctr = ctr_tag(g);
    nonce_dom(iv, 0x12, g, ctr);
    int al = aad_tag(aad, g, ctr);
    gcm_enc(cs, iv, aad, al, buf, PG, mac[g]);
    if (pwrite(fd, buf, PG, tag_off(g)) != (ssize_t)PG) die("w tag");
  }
  if (fsync(fd)) die("fsync pg");
  close(fd);
  path(p, "macs.bin");
  fd = open(p, O_RDWR | O_CREAT | O_TRUNC, 0600);
  if (write(fd, mac, G * 16) != (ssize_t)(G * 16)) die("w macs");
  fsync(fd); close(fd);
  printf("setup N=%zu G=%zu env=%zu pg=%zu\n", N, G, env_len(), pg_len());
}

// --- run state ----------------------------------------------------------------------
static int fd_env, fd_pg, fd_mem;
static uint8_t *env_map, *pg_map, *back;
static size_t back_len;
static tent *tier;
static uint8_t (*gmac)[16];

enum { A_I, A_II, A_III, A_IV, A_V, A_VI, A_T1, A_T2, A_T3 };
static const char *ANAME[] = {"i", "ii", "iii", "iv", "v", "vi", "ii-t1", "ii-t2", "ii-t3"};
static int DROP = 0;
static int arm, T = 1, R = 7, SPLIT = 0, STRIDE = 1, SAMPLE_GROUP = 0, GBBUSY = 0, SRCZAP = 0;

#define NC 5
static const char *CNAME[][NC] = {
    [A_I] = {"fault", "copy", "cipher", "load", ""},
    [A_II] = {"tag", "fault", "copy", "cipher", "load"},
    [A_III] = {"tag", "fault", "pread", "cipher", "load"},
    [A_IV] = {"tag", "cipher", "pwrite", "mapfault", ""},
    [A_V] = {"populate", "copy", "cipher", "load", ""},
    [A_VI] = {"cipher", "pwrite", "popread", "load", ""},
    [A_T1] = {"tag", "fault", "copy", "cipher", "load"},
    [A_T2] = {"tag", "fault", "copy", "cipher", "load"},
    [A_T3] = {"tag", "fault", "copy", "cipher", "load"},
};

typedef struct {
  int id;
  EVP_CIPHER_CTX *de, *ds;
  uint8_t *scratch, *tierbuf;
  uint64_t *lat;          // [R][per]
  uint64_t (*comp)[NC];   // [R][per]
  size_t per;             // ops per round for this worker (max)
  size_t *done;           // [R] ops done per round
  long *minf, *majf;      // [R]
  size_t bad;
  uint64_t *wall;         // [R] worker's wall for its slice
  uint8_t *back; int fd;  // the backing this worker fills (MEMS=1: one memfd per worker)
} worker;

static uint32_t *items; static size_t nitems;
static uint32_t *subpage; // for group sampling: page chosen in group
static pthread_barrier_t bstart, bend;
static atomic_int gbdone[1024];
static worker *W;

static inline void flush_lines(const void *p, size_t n) {
  const char *c = (const char *)((uintptr_t)p & ~63UL);
  for (const char *e = (const char *)p + n; c < e; c += 64) _mm_clflush(c);
  _mm_mfence();
}

static inline int tag_of(worker *w, uint64_t p, uint8_t tag[16], uint32_t *ver) {
  // the tag-tier lookup: the design's tier holds verified tag pages in memory.
  if (arm == A_T2 || arm == A_T3) {
    uint64_t g = p >> 7;
    const uint8_t *st = pg_map + tag_off(g);
    memcpy(w->tierbuf, st, PG); // T3: a major fault here (the stored tag page is not resident)
    uint8_t iv[12], aad[28]; uint32_t ctr = ctr_tag(g);
    nonce_dom(iv, 0x12, g, ctr);
    int al = aad_tag(aad, g, ctr);
    if (!gcm_dec(w->ds, iv, aad, al, w->tierbuf, w->tierbuf, PG, gmac[g])) return 0;
    tent *e = (tent *)(w->tierbuf + (p & 127) * 32);
    memcpy(tag, e->tag, 16); *ver = e->ver;
    return 1;
  }
  tent *e = &tier[p];
  memcpy(tag, e->tag, 16); *ver = e->ver;
  return 1;
}

static int dec_page(worker *w, uint64_t p, const uint8_t *in, uint8_t *out, const uint8_t tag[16], uint32_t ver) {
  uint8_t iv[12], aad[30];
  nonce_dom(iv, 0x10, p, ver);
  int al = aad_data(aad, p, ver);
  return gcm_dec(w->ds, iv, aad, al, in, out, PG, tag);
}

// one miss; returns 0 on a check failure. c[] gets the split components (if SPLIT).
static int op(worker *w, uint64_t it, uint64_t *c) {
  uint8_t *back = w->back; int fd_mem = w->fd;
  uint64_t t0, t1, t2, t3, t4, t5;
  switch (arm) {
  case A_I: {
    uint64_t ch = it;
    uint8_t *dst = back + ch * CH;
    const uint8_t *src = env_map + env_off(ch);
    uint8_t iv[12];
    if (SPLIT) {
      t0 = now();
      ((volatile uint8_t *)dst)[0] = 0; ((volatile uint8_t *)dst)[CH - 1] = 0;
      t1 = now();
      memcpy(dst, src, CH);
      t2 = now();
      nonce_env(iv, ch);
      int ok = gcm_dec(w->de, iv, ENV_HDR, 48, dst, dst, CH, src + CH);
      t3 = now();
      uint64_t v = *(volatile uint64_t *)dst;
      t4 = now();
      c[0] = t1 - t0; c[1] = t2 - t1; c[2] = t3 - t2; c[3] = t4 - t3;
      return ok && v == want_env(ch);
    }
    if (DROP == 2) { ((volatile uint8_t *)dst)[0] = 0; ((volatile uint8_t *)dst)[CH - 1] = 0; return *(volatile uint64_t *)dst == 0; }
    memcpy(dst, src, CH);
    if (DROP == 1) return *(volatile uint64_t *)dst != 1;
    nonce_env(iv, ch);
    int ok = gcm_dec(w->de, iv, ENV_HDR, 48, dst, dst, CH, src + CH);
    return ok && *(volatile uint64_t *)dst == want_env(ch);
  }
  case A_II: case A_T1: case A_T2: case A_T3: {
    uint64_t p = it;
    uint8_t *dst = back + p * PG;
    const uint8_t *src = pg_map + data_off(p);
    uint8_t tag[16]; uint32_t ver;
    if (SPLIT) {
      t0 = now();
      int okt = tag_of(w, p, tag, &ver);
      t1 = now();
      ((volatile uint8_t *)dst)[0] = 0;
      t2 = now();
      memcpy(dst, src, PG);
      t3 = now();
      int ok = dec_page(w, p, dst, dst, tag, ver);
      t4 = now();
      uint64_t v = *(volatile uint64_t *)dst;
      t5 = now();
      c[0] = t1 - t0; c[1] = t2 - t1; c[2] = t3 - t2; c[3] = t4 - t3; c[4] = t5 - t4;
      return okt && ok && v == want_pg(p);
    }
    int okt = tag_of(w, p, tag, &ver);
    if (DROP == 2) { ((volatile uint8_t *)dst)[0] = 0; return okt && *(volatile uint64_t *)dst == 0; }
    memcpy(dst, src, PG);
    if (DROP == 1) return okt && *(volatile uint64_t *)dst != 1;
    int ok = dec_page(w, p, dst, dst, tag, ver);
    return okt && ok && *(volatile uint64_t *)dst == want_pg(p);
  }
  case A_III: {
    uint64_t p = it;
    uint8_t *dst = back + p * PG;
    uint8_t tag[16]; uint32_t ver;
    if (SPLIT) {
      t0 = now();
      tag_of(w, p, tag, &ver);
      t1 = now();
      ((volatile uint8_t *)dst)[0] = 0;
      t2 = now();
      if (pread(fd_pg, dst, PG, data_off(p)) != (ssize_t)PG) die("pread");
      t3 = now();
      int ok = dec_page(w, p, dst, dst, tag, ver);
      t4 = now();
      uint64_t v = *(volatile uint64_t *)dst;
      t5 = now();
      c[0] = t1 - t0; c[1] = t2 - t1; c[2] = t3 - t2; c[3] = t4 - t3; c[4] = t5 - t4;
      return ok && v == want_pg(p);
    }
    tag_of(w, p, tag, &ver);
    if (pread(fd_pg, dst, PG, data_off(p)) != (ssize_t)PG) die("pread");
    int ok = dec_page(w, p, dst, dst, tag, ver);
    return ok && *(volatile uint64_t *)dst == want_pg(p);
  }
  case A_IV: {
    uint64_t p = it;
    uint8_t *dst = back + p * PG;
    const uint8_t *src = pg_map + data_off(p);
    uint8_t tag[16]; uint32_t ver;
    if (SPLIT) {
      t0 = now();
      tag_of(w, p, tag, &ver);
      t1 = now();
      int ok = dec_page(w, p, src, w->scratch, tag, ver);
      t2 = now();
      if (pwrite(fd_mem, w->scratch, PG, p * PG) != (ssize_t)PG) die("pwrite");
      t3 = now();
      uint64_t v = *(volatile uint64_t *)dst;
      t4 = now();
      c[0] = t1 - t0; c[1] = t2 - t1; c[2] = t3 - t2; c[3] = t4 - t3;
      return ok && v == want_pg(p);
    }
    tag_of(w, p, tag, &ver);
    int ok = dec_page(w, p, src, w->scratch, tag, ver);
    if (pwrite(fd_mem, w->scratch, PG, p * PG) != (ssize_t)PG) die("pwrite");
    return ok && *(volatile uint64_t *)dst == want_pg(p);
  }
  case A_V: {
    uint64_t r = it, p0 = r * RANGE;
    size_t n = N - p0 < RANGE ? N - p0 : RANGE;
    uint8_t *dst = back + p0 * PG;
    int ok = 1;
    uint64_t cp = 0, cy = 0;
    t0 = SPLIT ? now() : 0;
    if (madvise(dst, n * PG, MADV_POPULATE_WRITE)) die("populate_write");
    t1 = SPLIT ? now() : 0;
    for (size_t j = 0; j < n; j++) {
      uint64_t p = p0 + j; uint8_t tag[16]; uint32_t ver;
      tag_of(w, p, tag, &ver);
      if (DROP == 2) continue;
      uint64_t a = SPLIT ? now() : 0;
      memcpy(dst + j * PG, pg_map + data_off(p), PG);
      uint64_t b = SPLIT ? now() : 0;
      if (DROP == 1) continue;
      ok &= dec_page(w, p, dst + j * PG, dst + j * PG, tag, ver);
      if (SPLIT) { uint64_t d = now(); cp += b - a; cy += d - b; }
    }
    t2 = SPLIT ? now() : 0;
    for (size_t j = 0; j < n; j++) ok &= *(volatile uint64_t *)(dst + j * PG) == want_pg(p0 + j);
    if (SPLIT) { t3 = now(); c[0] = t1 - t0; c[1] = cp; c[2] = cy; c[3] = t3 - t2; }
    (void)t2;
    return ok;
  }
  case A_VI: {
    uint64_t r = it, p0 = r * RANGE;
    size_t n = N - p0 < RANGE ? N - p0 : RANGE;
    uint8_t *dst = back + p0 * PG;
    int ok = 1;
    t0 = SPLIT ? now() : 0;
    for (size_t j = 0; j < n; j++) {
      uint64_t p = p0 + j; uint8_t tag[16]; uint32_t ver;
      tag_of(w, p, tag, &ver);
      ok &= dec_page(w, p, pg_map + data_off(p), w->scratch + j * PG, tag, ver);
    }
    t1 = SPLIT ? now() : 0;
    if (pwrite(fd_mem, w->scratch, n * PG, p0 * PG) != (ssize_t)(n * PG)) die("pwrite range");
    t2 = SPLIT ? now() : 0;
    if (madvise(dst, n * PG, MADV_POPULATE_READ)) die("populate_read");
    t3 = SPLIT ? now() : 0;
    for (size_t j = 0; j < n; j++) ok &= *(volatile uint64_t *)(dst + j * PG) == want_pg(p0 + j);
    if (SPLIT) { t4 = now(); c[0] = t1 - t0; c[1] = t2 - t1; c[2] = t3 - t2; c[3] = t4 - t3; }
    return ok;
  }
  }
  return 0;
}

static long rus(int maj) {
  struct rusage u;
  getrusage(RUSAGE_THREAD, &u);
  return maj ? u.ru_majflt : u.ru_minflt;
}

static void *work(void *arg) {
  worker *w = arg;
  for (int r = 0; r < R; r++) {
    pthread_barrier_wait(&bstart);
    long m0 = rus(0), j0 = rus(1);
    uint64_t ws = now();
    size_t k = 0;
    for (size_t i = w->id; i < nitems; i += T) {
      uint64_t it = items[i];
      if (SAMPLE_GROUP) it = (uint64_t)items[i] * 128 + subpage[items[i]];
      if (arm == A_T1) flush_lines(&tier[it], 32);
      if (arm == A_T2) flush_lines(pg_map + tag_off(it >> 7), PG);
      uint64_t c[NC] = {0};
      uint64_t t0 = now();
      int ok = op(w, it, c);
      uint64_t t1 = now();
      if (!ok) w->bad++;
      w->lat[r * w->per + k] = t1 - t0;
      if (SPLIT) memcpy(w->comp[r * w->per + k], c, sizeof c);
      k++;
    }
    w->wall[r] = now() - ws;
    w->done[r] = k;
    w->minf[r] = rus(0) - m0;
    w->majf[r] = rus(1) - j0;
    pthread_barrier_wait(&bend);
    if (GBBUSY) while (!atomic_load_explicit(&gbdone[r], memory_order_acquire)) { __asm__ volatile("pause"); }
  }
  return NULL;
}

static size_t resident(uint8_t *p, size_t len) {
  size_t n = len / PG;
  unsigned char *v = malloc(n);
  if (mincore(p, len, v)) die("mincore");
  size_t c = 0;
  for (size_t i = 0; i < n; i++) c += v[i] & 1;
  free(v);
  return c;
}

static void drop_tags(void) {
  // the tag regions: unmap, then drop from the page cache (order-0 folios: written a page at a time)
  for (uint64_t e = 0; e <= (G - 1) >> 9; e++) {
    size_t gs = e * 512, ge = G < gs + 512 ? G : gs + 512;
    size_t len = (ge - gs) * PG;
    if (madvise(pg_map + ext_base(e), len, MADV_DONTNEED)) die("tag dontneed");
    if (posix_fadvise(fd_pg, ext_base(e), len, POSIX_FADV_DONTNEED)) die("tag fadvise");
  }
}
static size_t tags_resident(void) {
  size_t c = 0;
  for (uint64_t e = 0; e <= (G - 1) >> 9; e++) {
    size_t gs = e * 512, ge = G < gs + 512 ? G : gs + 512;
    c += resident(pg_map + ext_base(e), (ge - gs) * PG);
  }
  return c;
}

static void pct(uint64_t *v, size_t n, double div, double *med, double *p10, double *p90, double *p99, double *mean) {
  qsort(v, n, sizeof *v, cmpu);
  double s = 0;
  for (size_t i = 0; i < n; i++) s += v[i];
  *med = v[n / 2] / div; *p10 = v[n / 10] / div; *p90 = v[n * 9 / 10] / div; *p99 = v[n * 99 / 100] / div; *mean = s / n / div;
}

static void openmaps(void) {
  char p[4096];
  path(p, "env.bin");
  fd_env = open(p, O_RDONLY);
  path(p, "pg.bin");
  fd_pg = open(p, O_RDONLY);
  if (fd_env < 0 || fd_pg < 0) die("open (run setup first)");
  struct stat st;
  fstat(fd_env, &st); if ((size_t)st.st_size != env_len()) { fprintf(stderr, "env.bin size %ld != %zu: rerun setup with this N\n", (long)st.st_size, env_len()); exit(1); }
  fstat(fd_pg, &st); if ((size_t)st.st_size != pg_len()) { fprintf(stderr, "pg.bin size mismatch\n"); exit(1); }
  env_map = mmap(0, env_len(), PROT_READ, MAP_SHARED, fd_env, 0);
  pg_map = mmap(0, pg_len(), PROT_READ, MAP_SHARED, fd_pg, 0);
  if (env_map == MAP_FAILED || pg_map == MAP_FAILED) die("mmap src");
  madvise(env_map, env_len(), MADV_RANDOM);
  madvise(pg_map, pg_len(), MADV_RANDOM);
  G = (N + 127) / 128;
  // read once: into the page cache, and mapped (the steady state of a file read before)
  if (madvise(env_map, env_len(), MADV_POPULATE_READ)) die("pop env");
  for (uint64_t e = 0; e <= (N - 1) >> 16; e++) { // data regions and tag regions of the page file
    size_t ps = e << 16, pe = N < ps + 65536 ? N : ps + 65536;
    if (madvise(pg_map + data_off(ps), (pe - ps) * PG, MADV_POPULATE_READ)) die("pop pg data");
    size_t gs = e * 512, ge = G < gs + 512 ? G : gs + 512;
    if (madvise(pg_map + ext_base(e), (ge - gs) * PG, MADV_POPULATE_READ)) die("pop pg tags");
  }
  size_t re = resident(env_map, env_len()), rd = 0;
  for (uint64_t e = 0; e <= (N - 1) >> 16; e++) { size_t ps = e << 16, pe = N < ps + 65536 ? N : ps + 65536; rd += resident(pg_map + data_off(ps), (pe - ps) * PG); }
  printf("# resident: env %zu/%zu pages, pg data %zu/%zu, pg tags %zu/%zu\n", re, env_len() / PG, rd, N, tags_resident(), G);
  // the tier: every tag page verified once
  path(p, "macs.bin");
  int fm = open(p, O_RDONLY);
  gmac = malloc(G * 16);
  if (read(fm, gmac, G * 16) != (ssize_t)(G * 16)) die("macs");
  close(fm);
  tier = aligned_alloc(64, ((N * sizeof(tent)) + 63) & ~63UL);
  EVP_CIPHER_CTX *ds = ctx_new(KEY_S, 0);
  uint8_t *b = aligned_alloc(PG, PG);
  for (size_t g = 0; g < G; g++) {
    uint8_t iv[12], aad[28]; uint32_t ctr = ctr_tag(g);
    nonce_dom(iv, 0x12, g, ctr);
    int al = aad_tag(aad, g, ctr);
    if (!gcm_dec(ds, iv, aad, al, pg_map + tag_off(g), b, PG, gmac[g])) { fprintf(stderr, "tag page %zu fails\n", g); exit(1); }
    for (size_t j = 0; j < 128 && g * 128 + j < N; j++) memcpy(&tier[g * 128 + j], b + j * 32, 32);
  }
  free(b);
  EVP_CIPHER_CTX_free(ds);
}

static int env_int(const char *k, int d) { const char *v = getenv(k); return v ? atoi(v) : d; }

static void run(const char *aname) {
  arm = -1;
  for (int i = 0; i < (int)(sizeof ANAME / sizeof *ANAME); i++) if (!strcmp(aname, ANAME[i])) arm = i;
  if (arm < 0) { fprintf(stderr, "arm?\n"); exit(1); }
  DROP = env_int("DROP", 0);
  SPLIT = env_int("SPLIT", 0); STRIDE = env_int("STRIDE", 1); GBBUSY = env_int("GBBUSY", 0); SRCZAP = env_int("SRCZAP", 0);
  SAMPLE_GROUP = (arm == A_T1 || arm == A_T2 || arm == A_T3) || env_int("GROUP", 0);
  if (R > 1024) R = 1024;
  openmaps();
  // the backing
  fd_mem = memfd_create("supdb", MFD_CLOEXEC);
  size_t plain = arm == A_I ? N * CH : N * PG;
  back_len = (plain + PG - 1) / PG * PG;
  if (ftruncate(fd_mem, back_len)) die("ft mem");
  back = mmap(0, back_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd_mem, 0);
  if (back == MAP_FAILED) die("mmap mem");
  madvise(back, back_len, MADV_NOHUGEPAGE);
  // the items
  if (SAMPLE_GROUP) nitems = G;
  else if (arm == A_V || arm == A_VI) nitems = (N + RANGE - 1) / RANGE;
  else nitems = (N + STRIDE - 1) / STRIDE;
  items = malloc(nitems * sizeof *items);
  subpage = malloc(G * sizeof *subpage);
  size_t per = nitems / T + 1;
  W = calloc(T, sizeof *W);
  pthread_barrier_init(&bstart, NULL, T + 1);
  pthread_barrier_init(&bend, NULL, T + 1);
  pthread_t th[64];
  for (int t = 0; t < T; t++) {
    worker *w = &W[t];
    w->id = t; w->per = per; w->back = back; w->fd = fd_mem;
    if (env_int("MEMS", 0) && t > 0) {
      w->fd = memfd_create("supdb", MFD_CLOEXEC);
      if (ftruncate(w->fd, back_len)) die("ft mem2");
      w->back = mmap(0, back_len, PROT_READ | PROT_WRITE, MAP_SHARED, w->fd, 0);
      if (w->back == MAP_FAILED) die("mmap mem2");
      madvise(w->back, back_len, MADV_NOHUGEPAGE);
    }
    w->de = ctx_new(KEY_ENV, 0); w->ds = ctx_new(KEY_S, 0);
    w->scratch = aligned_alloc(PG, RANGE * PG); memset(w->scratch, 1, RANGE * PG);
    w->tierbuf = aligned_alloc(PG, PG); memset(w->tierbuf, 1, PG);
    w->lat = calloc((size_t)R * per, 8);
    if (SPLIT) w->comp = calloc((size_t)R * per, sizeof *w->comp);
    w->done = calloc(R, sizeof(size_t)); w->minf = calloc(R, sizeof(long)); w->majf = calloc(R, sizeof(long)); w->wall = calloc(R, 8);
    pthread_create(&th[t], NULL, work, w);
  }
  uint64_t *gb = calloc(R, 8); size_t *gbpages = calloc(R, sizeof(size_t)); size_t *tagres = calloc(R, sizeof(size_t));
  rng rr; rseed(&rr, 0xE2 + arm * 1000 + T);
  for (int r = 0; r < R; r++) {
    for (size_t i = 0; i < nitems; i++) items[i] = (uint32_t)(SAMPLE_GROUP || arm == A_V || arm == A_VI ? i : i * STRIDE);
    shuffle(items, nitems, &rr);
    if (SAMPLE_GROUP) for (size_t g = 0; g < G; g++) { size_t n = N - g * 128 < 128 ? N - g * 128 : 128; subpage[g] = (uint32_t)(rnext(&rr) % n); }
    if (arm == A_T3) { drop_tags(); tagres[r] = tags_resident(); }
    if (SRCZAP) { madvise(env_map, env_len(), MADV_DONTNEED); madvise(pg_map + ext_base(0) + 2 * MiB, pg_len() - ext_base(0) - 2 * MiB, MADV_DONTNEED); }
    pthread_barrier_wait(&bstart);
    pthread_barrier_wait(&bend);
    int nb = env_int("MEMS", 0) ? T : 1;
    gbpages[r] = 0;
    for (int b = 0; b < nb; b++) gbpages[r] += resident(W[b].back, back_len);
    uint64_t t0 = now();
    for (int b = 0; b < nb; b++)
      for (size_t u = 0; u < back_len; u += MiB) {
        size_t l = back_len - u < MiB ? back_len - u : MiB;
        if (madvise(W[b].back + u, l, MADV_REMOVE)) die("remove");
      }
    gb[r] = now() - t0;
    atomic_store_explicit(&gbdone[r], 1, memory_order_release);
    if (env_int("SLEEPMS", 0)) usleep(1000 * env_int("SLEEPMS", 0)); // freed memory reported to the host meanwhile
  }
  for (int t = 0; t < T; t++) pthread_join(th[t], NULL);
  // stats
  size_t bad = 0; for (int t = 0; t < T; t++) bad += W[t].bad;
  size_t div = (arm == A_V || arm == A_VI) ? RANGE : 1; // ns per page
  double ppo = (arm == A_V || arm == A_VI) ? RANGE : 1;  // pages per op
  // pooled over rounds 1..R-1
  size_t tot = 0; for (int t = 0; t < T; t++) for (int r = 1; r < R; r++) tot += W[t].done[r];
  uint64_t *all = malloc(tot * 8); size_t k = 0;
  long mf = 0, jf = 0; size_t ops = 0;
  for (int t = 0; t < T; t++) for (int r = 1; r < R; r++) { for (size_t i = 0; i < W[t].done[r]; i++) all[k++] = W[t].lat[r * per + i]; mf += W[t].minf[r]; jf += W[t].majf[r]; ops += W[t].done[r]; }
  double med, p10, p90, p99, mean;
  pct(all, tot, div, &med, &p10, &p90, &p99, &mean);
  double pages = ops * ppo;
  // round 0
  size_t n0 = 0; for (int t = 0; t < T; t++) n0 += W[t].done[0];
  uint64_t *a0 = malloc(n0 * 8); k = 0; long mf0 = 0, jf0 = 0;
  for (int t = 0; t < T; t++) { for (size_t i = 0; i < W[t].done[0]; i++) a0[k++] = W[t].lat[i]; mf0 += W[t].minf[0]; jf0 += W[t].majf[0]; }
  double m0, q10, q90, q99, mn0;
  pct(a0, n0, div, &m0, &q10, &q90, &q99, &mn0);
  // give-back per page present, rounds 1..
  double gbpp[1024]; int ng = 0;
  for (int r = 1; r < R; r++) if (gbpages[r]) gbpp[ng++] = (double)gb[r] / gbpages[r];
  for (int i = 1; i < ng; i++) for (int j = i; j > 0 && gbpp[j] < gbpp[j - 1]; j--) { double x = gbpp[j]; gbpp[j] = gbpp[j - 1]; gbpp[j - 1] = x; }
  double gbmed = ng ? gbpp[ng / 2] : 0, gbmin = ng ? gbpp[0] : 0, gbmax = ng ? gbpp[ng - 1] : 0;
  double gbpres = 0; for (int r = 1; r < R; r++) gbpres += gbpages[r]; gbpres /= (R - 1);
  // aggregate throughput: pages per second per round, from the slowest worker's wall
  double thr = 0; for (int r = 1; r < R; r++) { uint64_t mx = 0; size_t pr = 0; for (int t = 0; t < T; t++) { if (W[t].wall[r] > mx) mx = W[t].wall[r]; pr += W[t].done[r]; } thr += pr * ppo / (mx / 1e9); }
  thr /= (R - 1);
  printf("RESULT mems=%d drop=%d arm=%s threads=%d split=%d stride=%d group=%d gbbusy=%d srczap=%d N=%zu rounds=%d ops=%zu pages_per_op=%.0f "
         "median_ns=%.0f p10_ns=%.0f p90_ns=%.0f p99_ns=%.0f mean_ns=%.0f minflt_pp=%.3f majflt_pp=%.4f "
         "gb_ns_pp_med=%.0f gb_min=%.0f gb_max=%.0f gb_pages=%.0f agg_pages_per_s=%.0f r0_median_ns=%.0f r0_mean_ns=%.0f r0_minflt_pp=%.3f r0_majflt_pp=%.4f bad=%zu",
         env_int("MEMS", 0), DROP, ANAME[arm], T, SPLIT, STRIDE, SAMPLE_GROUP, GBBUSY, SRCZAP, N, R, ops, ppo, med, p10, p90, p99, mean, mf / pages, jf / pages, gbmed, gbmin, gbmax, gbpres, thr, m0, mn0,
         mf0 / (n0 * ppo), jf0 / (n0 * ppo), bad);
  if (arm == A_T3) { size_t mx = 0; for (int r = 0; r < R; r++) if (tagres[r] > mx) mx = tagres[r]; printf(" tag_pages_resident_at_round_start_max=%zu", mx); }
  printf("\n");
  // per-round medians
  printf("ROUNDS arm=%s threads=%d", ANAME[arm], T);
  for (int r = 0; r < R; r++) {
    size_t n = 0; for (int t = 0; t < T; t++) n += W[t].done[r];
    uint64_t *a = malloc(n * 8); size_t q = 0;
    for (int t = 0; t < T; t++) for (size_t i = 0; i < W[t].done[r]; i++) a[q++] = W[t].lat[r * per + i];
    qsort(a, n, 8, cmpu);
    printf(" r%d:%.0f/gb%.0f", r, a[n / 2] / (double)div, gbpages[r] ? (double)gb[r] / gbpages[r] : 0);
    free(a);
  }
  printf("\n");
  if (SPLIT) {
    printf("SPLIT arm=%s threads=%d", ANAME[arm], T);
    uint64_t *v = malloc(tot * 8);
    for (int ci = 0; ci < NC; ci++) {
      if (!CNAME[arm][ci][0]) continue;
      size_t q = 0; double s = 0;
      for (int t = 0; t < T; t++) for (int r = 1; r < R; r++) for (size_t i = 0; i < W[t].done[r]; i++) { v[q] = W[t].comp[r * per + i][ci]; s += v[q]; q++; }
      qsort(v, q, 8, cmpu);
      printf(" %s_med=%.0f %s_mean=%.0f %s_p90=%.0f", CNAME[arm][ci], v[q / 2] / (double)div, CNAME[arm][ci], s / q / div, CNAME[arm][ci], v[q * 9 / 10] / (double)div);
    }
    printf("\n");
    free(v);
  }
}

// --- isolated components -------------------------------------------------------------
static void base(void) {
  openmaps();
  size_t n = N;
  uint64_t *lat = malloc(n * 8);
  rng rr; rseed(&rr, 77);
  uint32_t *perm = malloc(n * 4);
  for (size_t i = 0; i < n; i++) perm[i] = (uint32_t)i;
  double med, p10, p90, p99, mean;
#define REPORT(name) do { pct(lat, n, 1, &med, &p10, &p90, &p99, &mean); printf("BASE %-58s median_ns=%.0f p10=%.0f p90=%.0f mean=%.0f\n", name, med, p10, p90, mean); } while (0)
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); lat[i] = now() - t0; }
  REPORT("clock pair");
  int fm = memfd_create("b", MFD_CLOEXEC); if (ftruncate(fm, n * PG)) die("ft");
  uint8_t *m = mmap(0, n * PG, PROT_READ | PROT_WRITE, MAP_SHARED, fm, 0); madvise(m, n * PG, MADV_NOHUGEPAGE);
  uint8_t *warm = aligned_alloc(PG, PG); memset(warm, 2, PG);
  EVP_CIPHER_CTX *ds = ctx_new(KEY_S, 0), *de = ctx_new(KEY_ENV, 0);
  for (int rep = 0; rep < 2; rep++) { // rep 0 builds page tables; rep 1 is what is reported
    shuffle(perm, n, &rr);
    for (size_t i = 0; i < n; i++) { volatile uint8_t *q = m + (size_t)perm[i] * PG; uint64_t t0 = now(); *q = 1; lat[i] = now() - t0; }
    if (rep) REPORT("memfd fresh page, write fault (1-byte store)");
    for (size_t i = 0; i < n; i++) { volatile uint8_t *q = m + (size_t)perm[i] * PG; uint64_t t0 = now(); *q = 2; lat[i] = now() - t0; }
    if (rep) REPORT("memfd present page, write hit (random)");
    for (size_t u = 0; u < n * PG; u += MiB) madvise(m + u, MiB, MADV_REMOVE);
  }
  // memfd read fault on a page present in the memfd but not mapped (pwrite'd)
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); if (pwrite(fm, warm, PG, (size_t)perm[i] * PG) != (ssize_t)PG) die("pw"); lat[i] = now() - t0; }
  REPORT("pwrite 4096 into a memfd hole");
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) { volatile uint8_t *q = m + (size_t)perm[i] * PG; uint64_t t0 = now(); (void)*q; lat[i] = now() - t0; }
  REPORT("read fault: memfd page present, not mapped (+fault-around)");
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); if (pwrite(fm, warm, PG, (size_t)perm[i] * PG) != (ssize_t)PG) die("pw"); lat[i] = now() - t0; }
  REPORT("pwrite 4096 over a present memfd page");
  for (size_t u = 0; u < n * PG; u += MiB) madvise(m + u, MiB, MADV_REMOVE);
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); if (pwrite(fm, warm, 2048, (size_t)perm[i] * PG) != 2048) die("pw"); lat[i] = now() - t0; }
  REPORT("pwrite 2048 into a memfd hole (partial page)");
  for (size_t u = 0; u < n * PG; u += MiB) madvise(m + u, MiB, MADV_REMOVE);
  // memcpy from the cold ciphertext mapping
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); memcpy(warm, pg_map + data_off(perm[i]), PG); __asm__ volatile("" ::: "memory"); lat[i] = now() - t0; }
  REPORT("memcpy 4096 from a cold ciphertext page into a warm buffer");
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); memcpy(warm, env_map + env_off(perm[i]), CH); __asm__ volatile("" ::: "memory"); lat[i] = now() - t0; }
  REPORT("memcpy 4080 from a cold envelope chunk into a warm buffer");
  // the cipher, warm
  uint8_t tag[16], iv[12]; nonce_dom(iv, 0x10, 1, 2);
  { EVP_CIPHER_CTX *ce = ctx_new(KEY_S, 1);
    uint8_t *ct = aligned_alloc(PG, PG), *b2 = aligned_alloc(PG, PG); memset(ct, 9, PG);
    gcm_enc(ce, iv, FILE_ID, 30, ct, PG, tag);
    for (size_t i = 0; i < n; i++) { memcpy(b2, ct, PG); uint64_t t0 = now(); int ok = gcm_dec(ds, iv, FILE_ID, 30, b2, b2, PG, tag); lat[i] = now() - t0; if (!ok) die("dec"); }
    REPORT("AES-256-GCM decrypt 4096 in place, warm (EVP)");
    for (size_t i = 0; i < n; i++) { uint64_t t0 = now(); int ok = gcm_dec(ds, iv, FILE_ID, 30, b2, b2, PG, tag); lat[i] = now() - t0; (void)ok; }
    REPORT("AES-256-GCM decrypt 4096 in place, warm, tag fails (EVP)");
    uint8_t iv2[12]; memcpy(iv2, iv, 12);
    gcm_enc(ce, iv2, ENV_HDR, 48, ct, CH, tag);
    for (size_t i = 0; i < n; i++) { memcpy(b2, ct, CH); uint64_t t0 = now(); int ok = gcm_dec(de, iv2, ENV_HDR, 48, b2, b2, CH, tag); lat[i] = now() - t0; (void)ok; }
    REPORT("AES-256-GCM decrypt 4080 in place, warm, wrong key (EVP)"); // same work, key differs
    EVP_CIPHER_CTX_free(ce);
  }
  // decrypt out of place from the cold ciphertext mapping into a warm buffer (arm iv's cipher)
  shuffle(perm, n, &rr);
  for (size_t i = 0; i < n; i++) {
    uint64_t p = perm[i]; uint8_t a[30], nv[12]; uint32_t ver = tier[p].ver;
    nonce_dom(nv, 0x10, p, ver); int al = aad_data(a, p, ver);
    uint64_t t0 = now(); int ok = gcm_dec(ds, nv, a, al, pg_map + data_off(p), warm, PG, tier[p].tag); lat[i] = now() - t0; if (!ok) die("dec2");
  }
  REPORT("decrypt 4096 from a cold ciphertext page into a warm buffer");
  // MADV_REMOVE costs
  for (int busy = 0; busy < 1; busy++) {
    for (size_t i = 0; i < n; i++) m[i * PG] = 1;
    size_t k = 0;
    for (size_t u = 0; u < n * PG; u += MiB) { uint64_t t0 = now(); madvise(m + u, MiB, MADV_REMOVE); lat[k++] = now() - t0; }
    size_t nn = n; n = k; REPORT("MADV_REMOVE 1 MiB, 256 pages present (per call)"); n = nn;
  }
  { size_t k = 0; shuffle(perm, n, &rr);
    for (size_t i = 0; i < n; i++) {
      if (perm[i] >= n / RANGE) continue; // a random order over the ranges
      uint64_t t0 = now(); if (madvise(m + (size_t)perm[i] * RANGE * PG, RANGE * PG, MADV_POPULATE_WRITE)) die("pw64"); lat[k++] = now() - t0;
    }
    size_t nn = n; n = k; REPORT("MADV_POPULATE_WRITE 64 fresh memfd pages (per call)"); n = nn; }
  for (size_t u = 0; u < n * PG; u += MiB) madvise(m + u, MiB, MADV_REMOVE);
}

// --- give-back after fills in each order, idle or beside busy threads ---------------------
static atomic_int gb_spin_stop;
static void *spinner(void *a) { (void)a; while (!atomic_load(&gb_spin_stop)) __asm__ volatile("pause"); return NULL; }
static uint8_t *gbm; static size_t gbn; static uint32_t *gbperm;
static void *gb_filler(void *a) { (void)a; for (size_t i = 0; i < gbn; i++) gbm[(size_t)gbperm[i] * PG] = 1; return NULL; }
static void gbtest(void) {
  size_t n = N, len = n * PG;
  int fm = memfd_create("supdb", MFD_CLOEXEC); if (ftruncate(fm, len)) die("ft");
  gbm = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, fm, 0); madvise(gbm, len, MADV_NOHUGEPAGE);
  gbn = n; gbperm = malloc(n * 4);
  rng rr; rseed(&rr, 99);
  int busy = env_int("BUSY", 0), other = env_int("OTHER", 0), reps = env_int("REPS", 6);
  for (int rep = 0; rep < reps; rep++) {
    for (int order = 0; order < 2; order++) {
      for (size_t i = 0; i < n; i++) gbperm[i] = (uint32_t)i;
      if (order) shuffle(gbperm, n, &rr);
      if (other) { pthread_t f; pthread_create(&f, NULL, gb_filler, NULL); pthread_join(f, NULL); }
      else gb_filler(NULL);
      pthread_t sp[8];
      atomic_store(&gb_spin_stop, 0);
      for (int k = 0; k < busy; k++) pthread_create(&sp[k], NULL, spinner, NULL);
      if (busy) usleep(20000);
      size_t pres = resident(gbm, len);
      uint64_t t0 = now();
      for (size_t u = 0; u < len; u += MiB) if (madvise(gbm + u, MiB, MADV_REMOVE)) die("rm");
      uint64_t dt = now() - t0;
      atomic_store(&gb_spin_stop, 1);
      for (int k = 0; k < busy; k++) pthread_join(sp[k], NULL);
      printf("GB rep=%d order=%s busy=%d other_thread_filled=%d pages=%zu ns_per_page=%.0f us_per_MiB=%.1f\n", rep, order ? "random" : "seq", busy, other, pres,
             (double)dt / pres, (double)dt / (len / MiB) / 1e3);
    }
  }
}

// --- steady state: a bounded backing, uniform random reads, the least recent unit given back ---
// Single thread. Only misses are timed; faults per miss from getrusage over the measured phase.
// Arm i: units of 256 chunks = 255 pages (pager.rs unit_chunks); arm ii: units of 256 pages.
static void steady(const char *aname, double frac) {
  arm = !strcmp(aname, "i") ? A_I : A_II;
  openmaps();
  fd_mem = memfd_create("supdb", MFD_CLOEXEC);
  size_t plain = arm == A_I ? N * CH : N * PG;
  back_len = (plain + PG - 1) / PG * PG;
  if (ftruncate(fd_mem, back_len)) die("ft mem");
  back = mmap(0, back_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd_mem, 0);
  madvise(back, back_len, MADV_NOHUGEPAGE);
  size_t upi = 256, U = (N + upi - 1) / upi, budget = (size_t)(frac * N);
  size_t unit_bytes = arm == A_I ? 255 * PG : 256 * PG;
  uint8_t *pres = calloc(N, 1); uint64_t *last = calloc(U, 8); uint32_t *cnt = calloc(U, 4);
  worker w = {0}; w.back = back; w.fd = fd_mem; w.de = ctx_new(KEY_ENV, 0); w.ds = ctx_new(KEY_S, 0); w.scratch = aligned_alloc(PG, RANGE * PG); w.tierbuf = aligned_alloc(PG, PG);
  rng rr; rseed(&rr, 0x5EAD + arm);
  size_t warm = 2 * N, meas = (size_t)env_int("ACCESSES", 6 * (int)N), present = 0;
  uint64_t *lat = malloc(meas * 8); size_t nm = 0, hits = 0, evicts = 0, bad = 0; uint64_t gbt = 0, clock_ = 0;
  long m0 = 0, j0 = 0;
  for (size_t a = 0; a < warm + meas; a++) {
    if (a == warm) { m0 = rus(0); j0 = rus(1); }
    uint64_t it = rnext(&rr) % N, u = it / upi;
    last[u] = ++clock_;
    if (pres[it]) { if (a >= warm) hits++; continue; }
    uint64_t c[NC]; uint64_t t0 = now(); int ok = op(&w, it, c); uint64_t t1 = now();
    if (!ok) bad++;
    pres[it] = 1; cnt[u]++; present++;
    if (a >= warm) lat[nm++] = t1 - t0;
    while (present > budget) {
      size_t v = SIZE_MAX; uint64_t best = UINT64_MAX;
      for (size_t k = 0; k < U; k++) if (cnt[k] && last[k] < best && k != u) { best = last[k]; v = k; }
      uint64_t g0 = now();
      size_t l = back_len - v * unit_bytes < unit_bytes ? back_len - v * unit_bytes : unit_bytes;
      if (madvise(back + v * unit_bytes, l, MADV_REMOVE)) die("rm");
      if (a >= warm) { gbt += now() - g0; evicts++; }
      for (size_t k = v * upi; k < (v + 1) * upi && k < N; k++) pres[k] = 0;
      present -= cnt[v]; cnt[v] = 0;
    }
  }
  long mf = rus(0) - m0, jf = rus(1) - j0;
  double med, p10, p90, p99, mean;
  pct(lat, nm, 1, &med, &p10, &p90, &p99, &mean);
  printf("STEADY arm=%s frac=%.2f N=%zu budget_items=%zu accesses=%zu misses=%zu hit_rate=%.3f median_ns=%.0f p10_ns=%.0f p90_ns=%.0f p99_ns=%.0f mean_ns=%.0f "
         "minflt_per_miss=%.3f majflt_per_miss=%.4f gb_ns_per_miss=%.0f evictions=%zu bad=%zu\n",
         ANAME[arm], frac, N, budget, meas, nm, (double)hits / meas, med, p10, p90, p99, mean, (double)mf / nm, (double)jf / nm, (double)gbt / nm, evicts, bad);
}

// --- a loop for perf: does a full-page pwrite into a shmem hole zero the page? ------------
static void zloop(int how) { // 0: pwrite 4096 into holes; 1: write fault by a 1-byte store; 2: pwrite 2048 into holes
  size_t n = N, len = n * PG;
  int fm = memfd_create("supdb", MFD_CLOEXEC); if (ftruncate(fm, len)) die("ft");
  uint8_t *m = mmap(0, len, PROT_READ | PROT_WRITE, MAP_SHARED, fm, 0); madvise(m, len, MADV_NOHUGEPAGE);
  uint8_t *src = aligned_alloc(PG, PG); memset(src, 7, PG);
  uint32_t *perm = malloc(n * 4); for (size_t i = 0; i < n; i++) perm[i] = (uint32_t)i;
  rng rr; rseed(&rr, 5);
  uint64_t tot = 0; size_t ops = 0;
  for (int rep = 0; rep < env_int("REPS", 10); rep++) {
    shuffle(perm, n, &rr);
    uint64_t t0 = now();
    for (size_t i = 0; i < n; i++) {
      size_t o = (size_t)perm[i] * PG;
      if (how == 0) { if (pwrite(fm, src, PG, o) != (ssize_t)PG) die("pw"); }
      else if (how == 1) m[o] = 1;
      else { if (pwrite(fm, src, 2048, o) != 2048) die("pw2"); }
    }
    tot += now() - t0; ops += n;
    for (size_t u = 0; u < len; u += MiB) madvise(m + u, MiB, MADV_REMOVE);
  }
  printf("ZLOOP how=%d ns_per_op=%.0f\n", how, (double)tot / ops);
}

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: e2 setup DIR | e2 base DIR | e2 run DIR ARM THREADS [ROUNDS]\n"
                    "  arms: i ii iii iv v vi ii-t1 ii-t2 ii-t3; env: NPAGES SPLIT STRIDE GROUP GBBUSY SRCZAP\n");
    return 2;
  }
  DIR_ = argv[2];
  if (getenv("NPAGES")) N = strtoul(getenv("NPAGES"), 0, 10);
  keys();
  G = (N + 127) / 128;
  if (!strcmp(argv[1], "setup")) setup();
  else if (!strcmp(argv[1], "base")) base();
  else if (!strcmp(argv[1], "gb")) gbtest();
  else if (!strcmp(argv[1], "zloop")) zloop(argc > 3 ? atoi(argv[3]) : 0);
  else if (!strcmp(argv[1], "steady")) { if (argc < 5) die("steady DIR ARM FRAC"); steady(argv[3], atof(argv[4])); }
  else if (!strcmp(argv[1], "run")) {
    if (argc < 5) die("args");
    T = atoi(argv[4]);
    if (argc > 5) R = atoi(argv[5]);
    run(argv[3]);
  } else { fprintf(stderr, "?\n"); return 2; }
  return 0;
}
