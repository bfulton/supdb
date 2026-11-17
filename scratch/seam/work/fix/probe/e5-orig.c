// E5 of the Pagefile design (seam/design.md section 9): the device cost of a
// Log commit, against the plain ordered run's commit and the plain WAL's.
//
//   e5 <shape> <commit bytes> <commits> <dir> [label]
//
// shapes:
//   A   the design's Log commit (sections 3 and 4): per door write call the
//       full pages home in one pwritev at their stored offsets inside the
//       1 MiB-aligned data area, the tag page when a 128-page group completes,
//       one frame into the journal ring (TAIL of the bytes since the last
//       frame, HOME, GROUP, a CKPT every 256 KiB of frames), every page, tag
//       page and frame sealed with AES-256-GCM; then fdatasync. The plaintext
//       is also written through into a memfd backing (the run is opened
//       through open_mapped, section 4 "Write-through").
//   A0  A without the write-through (the device side alone).
//   B   the plain ordered run's commit today (db.rs:6743-6776, 6855-6936):
//       records and a 24-byte marker in the record stream, the whole 1 MiB
//       pieces they complete written at the mark with a writeback hint each,
//       the rest written at the sync, then fdatasync.
//   Cf  the plain WAL's commit (db.rs:4436-4460, 27065-27078): one write of the
//       batch's frames and its 21-byte commit frame, fdatasync, and the
//       drop of synced pages every 1 MiB (wal_drop_bytes, db.rs:4051); a
//       fresh file with its 8-byte header.
//   Cp  Cf over a file prefilled with zeros in 4 KiB writes and fsynced
//       before the window (prefill, db.rs:4237-4262; recycle_wal).
//   D   today's cipher run commit (log_run, db.rs:28860-28881): B's writes
//       into a memfd (the run's file is a memory file until its envelope),
//       its fdatasync of nothing, and the batch sealed as one WAL record
//       (vault.rs:1801-1815), written and fdatasynced. Its envelope at the
//       run's close is not in the window.
//
// The window holds the file's create (and A's header write) and every
// commit; it is bracketed by reads of /sys/block/vda/stat and
// /proc/self/io. A sync() and a pause come before it; a second read after a
// sync() following it gives the metadata the filesystem wrote back late.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <openssl/evp.h>
#include <openssl/rand.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

#define PG 4096ull
#define GROUP 128ull
#define RING_START 8192ull
#define RING_END (1ull << 20)
#define CKPT_EVERY (256ull << 10)
#define PIECE (1ull << 20)
#define RUN_PREFIX 4288ull   /* format::SUPER + flatindex::HEADER: a run's records start (db.rs:6953) */
#define MARK 24ull           /* DIRECT_MARK_LEN (db.rs:4076) */
#define WAL_COMMIT 21ull     /* len4 crc4 seq8 kind1 batch-crc4 (db.rs:4381-4417) */
#define WAL_HDR 8ull         /* wal_header (db.rs:4141) */
#define WAL_SEALED_HDR 68ull /* WAL_HEAD_SEALED + kind + a wrapped key (db.rs:4135), about */
#define REC_HEAD 16ull       /* len4 nonce12 (vault.rs:1801) */
#define TAGB 16ull
#define WAL_DROP (1ull << 20) /* wal_drop_bytes default (db.rs:4051) */

static void die(const char *m) {
  perror(m);
  exit(1);
}

static uint64_t now_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec;
}

/* ---- counters ---------------------------------------------------------- */
typedef struct {
  unsigned long long v[17];
} devstat;
static devstat read_dev(void) {
  devstat d;
  memset(&d, 0, sizeof d);
  const char *dev = getenv("E5_DEV");
  char sp[256];
  snprintf(sp, sizeof sp, "/sys/block/%s/stat", dev ? dev : "vda");
  FILE *f = fopen(sp, "r");
  if (!f) die("block stat");
  for (int i = 0; i < 17; i++)
    if (fscanf(f, "%llu", &d.v[i]) != 1) break;
  fclose(f);
  return d;
}
static uint64_t read_wbytes(void) {
  FILE *f = fopen("/proc/self/io", "r");
  if (!f) die("proc io");
  char line[256];
  uint64_t wb = 0;
  while (fgets(line, sizeof line, f))
    if (!strncmp(line, "write_bytes:", 12)) wb = strtoull(line + 12, 0, 10);
  fclose(f);
  return wb;
}

static struct {
  uint64_t pwritev, pwrite, write, writev, fdatasync, fsync, sfr, fadvise, bytes;
} sc;

static void x_pwritev(int fd, struct iovec *iov, int n, off_t off) {
  size_t want = 0;
  for (int i = 0; i < n; i++) want += iov[i].iov_len;
  ssize_t r = pwritev(fd, iov, n, off);
  if (r < 0 || (size_t)r != want) die("pwritev");
  sc.pwritev++;
  sc.bytes += r;
}
static void x_pwrite(int fd, const void *b, size_t n, off_t off) {
  ssize_t r = pwrite(fd, b, n, off);
  if (r < 0 || (size_t)r != n) die("pwrite");
  sc.pwrite++;
  sc.bytes += r;
}
static void x_write(int fd, const void *b, size_t n) {
  ssize_t r = write(fd, b, n);
  if (r < 0 || (size_t)r != n) die("write");
  sc.write++;
  sc.bytes += r;
}
static void x_writev1(int fd, const void *b, size_t n) {
  struct iovec v = {(void *)b, n};
  ssize_t r = writev(fd, &v, 1);
  if (r < 0 || (size_t)r != n) die("writev");
  sc.writev++;
  sc.bytes += r;
}
static void x_fdatasync(int fd) {
  if (fdatasync(fd)) die("fdatasync");
  sc.fdatasync++;
}
static void x_fsync(int fd) {
  if (fsync(fd)) die("fsync");
  sc.fsync++;
}
static void x_sfr(int fd, off_t off, off_t len) {
  if (sync_file_range(fd, off, len, SYNC_FILE_RANGE_WRITE)) die("sync_file_range");
  sc.sfr++;
}
static void x_fadvise_dontneed(int fd, off_t off, off_t len) {
  posix_fadvise(fd, off, len, POSIX_FADV_DONTNEED);
  sc.fadvise++;
}

/* ---- AES-256-GCM ------------------------------------------------------- */
static uint64_t n_seal, seal_bytes;
static EVP_CIPHER_CTX *gcm_new(const uint8_t key[32]) {
  EVP_CIPHER_CTX *c = EVP_CIPHER_CTX_new();
  if (!c) die("ctx");
  if (!EVP_EncryptInit_ex(c, EVP_aes_256_gcm(), NULL, key, NULL)) die("init");
  return c;
}
static void seal(EVP_CIPHER_CTX *c, const uint8_t iv[12], const uint8_t *aad, int aadl, uint8_t *buf, int n,
                 uint8_t tag[16]) {
  int l;
  if (!EVP_EncryptInit_ex(c, NULL, NULL, NULL, iv)) die("iv");
  if (aadl && !EVP_EncryptUpdate(c, NULL, &l, aad, aadl)) die("aad");
  if (n && !EVP_EncryptUpdate(c, buf, &l, buf, n)) die("upd");
  if (!EVP_EncryptFinal_ex(c, buf + n, &l)) die("fin");
  if (!EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_GET_TAG, 16, tag)) die("tag");
  n_seal++;
  seal_bytes += n;
}
static void put64(uint8_t *p, uint64_t v) { memcpy(p, &v, 8); }
static void put32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }
static void put16(uint8_t *p, uint16_t v) { memcpy(p, &v, 2); }

/* ---- the payload --------------------------------------------------------- */
static uint8_t *src; /* random bytes; what a write hands down does not matter to the device */
static size_t src_len;

/* ---- A: the Pagefile Log ------------------------------------------------- */
static uint64_t tag_region(uint64_t e) { return (1ull << 20) + e * (258ull << 20); }
static uint64_t data_off(uint64_t p) { return tag_region(p >> 16) + (2ull << 20) + (p & 0xFFFF) * PG; }
static uint64_t tagpage_off(uint64_t p) { return tag_region(p >> 16) + ((p >> 7) & 511) * PG; }

typedef struct {
  int fd;
  int wt;          /* write-through on */
  uint8_t *back;   /* memfd backing, mapped */
  int bfd;         /* its descriptor, for a write-through by pwrite (wt == 2) */
  uint64_t wt_calls;
  size_t back_len;
  EVP_CIPHER_CTX *s; /* session key */
  uint8_t file_id[16];
  uint32_t ctr;
  uint64_t seq;
  uint64_t len;        /* logical length */
  uint8_t tail[PG];    /* the tail page's plaintext */
  uint64_t frame_from; /* logical offset the next frame's TAIL starts at */
  uint8_t gtags[GROUP][32];
  uint8_t *stage; /* 2 MiB staging buffer, reused */
  uint8_t tagpg[4][PG + 16];
  uint64_t tagpg_p[4];
  uint8_t tagmac[4][16];
  uint32_t tagver[4];
  int ntagpg;
  uint8_t *fb; /* frame buffer */
  size_t fb_cap;
  uint64_t ring_head, pin_lo, since_ckpt;
  uint64_t home, synced_home;
  uint8_t (*gm)[20];
  size_t ngm, gm_cap;
  /* stats */
  uint64_t frames, frame_bytes, ckpts, ckpt_bytes, wraps, pages_home, tag_pages, tail_bytes;
} pf;

static void pf_create(pf *f, const char *path, int wt, size_t back_len, int prefilled) {
  memset(f, 0, sizeof *f);
  f->fd = open(path, prefilled ? O_RDWR : (O_CREAT | O_RDWR | O_TRUNC), 0644);
  if (f->fd < 0) die("open A");
  RAND_bytes(f->file_id, 16);
  /* K_f = AEAD_Ks(nonce = file_id[0..12], 0^32).ct, then a session key
   * wrapped under K_f, and header A sealed under K_f: written once, unsynced;
   * the first fdatasync covers it (section 4, Sync). */
  uint8_t ks[32], kf[48], sk[32], t[16], iv[12];
  RAND_bytes(ks, 32);
  RAND_bytes(sk, 32);
  EVP_CIPHER_CTX *cs = gcm_new(ks);
  memset(kf, 0, sizeof kf);
  seal(cs, f->file_id, (const uint8_t *)"pfkdf", 5, kf, 32, t);
  EVP_CIPHER_CTX *cf = gcm_new(kf);
  uint8_t hdr[PG];
  memset(hdr, 0, PG);
  memcpy(hdr, "SUPDBPF2", 8);
  put16(hdr + 8, 2);
  hdr[10] = 2; /* class Log */
  memcpy(hdr + 11, f->file_id, 16);
  RAND_bytes(hdr + 27, 12); /* the body's nonce */
  uint8_t *body = hdr + 39;
  put64(body, 1); /* hdr_seq */
  body[8] = 1;    /* one session */
  uint8_t *e = body + 9;
  RAND_bytes(e, 12);
  memcpy(e + 12, sk, 32);
  seal(cf, e, (const uint8_t *)"pfk", 3, e + 12, 32, e + 44);
  put64(e + 60, 0);  /* base_page */
  e[68] = 0;         /* restart */
  put64(e + 69, 0);  /* first_seq */
  int body_len = 9 + 77;
  seal(cf, hdr + 27, (const uint8_t *)"pfh", 3, body, body_len, body + body_len);
  x_pwrite(f->fd, hdr, PG, 0);
  EVP_CIPHER_CTX_free(cs);
  EVP_CIPHER_CTX_free(cf);
  f->s = gcm_new(sk);
  (void)iv;
  if (posix_memalign((void **)&f->stage, PG, (2ull << 20) + 64)) die("stage");
  f->fb_cap = 1 << 20;
  f->fb = malloc(f->fb_cap);
  f->gm_cap = 1024;
  f->gm = malloc(f->gm_cap * 20);
  f->ring_head = f->pin_lo = RING_START;
  f->wt = wt;
  if (wt) {
    int mfd = memfd_create("pf-backing", MFD_CLOEXEC);
    if (mfd < 0) die("memfd");
    if (ftruncate(mfd, back_len)) die("ftruncate");
    f->back = mmap(NULL, back_len, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
    if (f->back == MAP_FAILED) die("mmap backing");
    f->bfd = mfd;
    f->back_len = back_len;
  }
}

static uint64_t ring_alloc(pf *f, uint64_t need) {
  if (f->pin_lo <= f->ring_head) {
    if (f->ring_head + need <= RING_END) return f->ring_head;
    if (RING_START + need <= f->pin_lo) {
      f->wraps++;
      return RING_START;
    }
  } else if (f->ring_head + need <= f->pin_lo)
    return f->ring_head;
  fprintf(stderr, "ring full of pinned spans: head %lu pin %lu need %lu\n", (unsigned long)f->ring_head,
          (unsigned long)f->pin_lo, (unsigned long)need);
  exit(2);
}

/* one frame appended at fb+at: clear header, sealed body (already laid out
 * at fb+at+16), tag */
static size_t frame_seal(pf *f, size_t at, uint8_t kind, size_t body_len) {
  uint8_t *h = f->fb + at;
  put16(h, 0x5046);
  h[2] = 0; /* session idx */
  h[3] = kind;
  put64(h + 4, f->seq);
  put32(h + 12, (uint32_t)body_len);
  uint8_t iv[12] = {0x11};
  put64(iv + 1, f->seq);
  uint8_t aad[29];
  memcpy(aad, f->file_id, 16);
  put64(aad + 16, f->seq);
  put32(aad + 24, (uint32_t)body_len);
  aad[28] = 0;
  seal(f->s, iv, aad, 29, h + 16, (int)body_len, h + 16 + body_len);
  f->seq++;
  f->ctr++;
  return 16 + body_len + 16;
}

static void fb_room(pf *f, size_t need) {
  if (need > f->fb_cap) {
    f->fb_cap = need * 2;
    f->fb = realloc(f->fb, f->fb_cap);
  }
}

/* One write call through the door: append n bytes. */
static void pf_write(pf *f, const uint8_t *b, size_t n) {
  static uint8_t hometags[512][20]; /* version and tag of each page this call sends home */
  uint64_t first_home = UINT64_MAX;
  size_t nst = 0;
  f->ntagpg = 0;
  if (f->wt) {
    if (f->len + n > f->back_len) die("backing too small");
    if (f->wt == 2) {
      /* through the backing's descriptor: shmem allocates the page in the
       * write, with no fault on the mapping */
      if (pwrite(f->bfd, b, n, f->len) != (ssize_t)n) die("pwrite backing");
      f->wt_calls++;
    } else
      memcpy(f->back + f->len, b, n);
  }
  while (n) {
    size_t at = f->len % PG;
    size_t take = PG - at < n ? PG - at : n;
    memcpy(f->tail + at, b, take);
    f->len += take;
    b += take;
    n -= take;
    if (f->len % PG == 0) {
      uint64_t p = f->len / PG - 1;
      uint8_t *dst = f->stage + nst * PG;
      memcpy(dst, f->tail, PG);
      uint8_t iv[12] = {0x10};
      uint64_t pe = p;
      memcpy(iv + 1, &pe, 7);
      put32(iv + 8, f->ctr);
      uint8_t aad[30];
      memcpy(aad, f->file_id, 16);
      put64(aad + 16, p);
      put32(aad + 24, f->ctr);
      put16(aad + 28, (uint16_t)0); /* valid_len: 4096 as 0 */
      uint8_t *ent = f->gtags[p & (GROUP - 1)];
      uint8_t tagb[16 + 16];
      /* the cipher writes its tag at buf+n in this helper; the stage has
       * room only for the page, so seal into the page and take the tag
       * from a scratch tail */
      seal(f->s, iv, aad, 30, dst, (int)PG, tagb);
      memcpy(ent, tagb, 16);
      put32(ent + 16, f->ctr);
      memset(ent + 20, 0, 12);
      if (nst >= 512) die("more than 512 pages in one call");
      put32(hometags[nst], f->ctr);
      memcpy(hometags[nst] + 4, tagb, 16);
      f->ctr++;
      if (first_home == UINT64_MAX) first_home = p;
      nst++;
      f->home = p + 1;
      f->pages_home++;
      if ((p & (GROUP - 1)) == GROUP - 1) {
        int k = f->ntagpg++;
        if (k >= 4) die("too many groups in one call");
        memcpy(f->tagpg[k], f->gtags, PG);
        uint8_t tiv[12] = {0x12};
        uint64_t g = p >> 7;
        memcpy(tiv + 1, &g, 7);
        put32(tiv + 8, f->ctr);
        uint8_t taad[28];
        memcpy(taad, f->file_id, 16);
        put64(taad + 16, g);
        put32(taad + 24, f->ctr);
        seal(f->s, tiv, taad, 28, f->tagpg[k], (int)PG, f->tagmac[k]);
        f->tagver[k] = f->ctr;
        f->tagpg_p[k] = p;
        f->ctr++;
        if (f->ngm == f->gm_cap) {
          f->gm_cap *= 2;
          f->gm = realloc(f->gm, f->gm_cap * 20);
        }
        put32(f->gm[f->ngm], (uint32_t)g);
        memcpy(f->gm[f->ngm] + 4, f->tagmac[k], 16);
        f->ngm++;
        memset(f->gtags, 0, sizeof f->gtags);
      }
      if (nst * PG >= (2ull << 20)) die("a call of more than 2 MiB");
    }
  }
  /* 1. the full pages home, one pwritev (contiguous within an extent here) */
  if (nst) {
    if ((first_home >> 16) != ((first_home + nst - 1) >> 16)) die("an extent crossed");
    struct iovec v = {f->stage, nst * PG};
    x_pwritev(f->fd, &v, 1, data_off(first_home));
  }
  /* 2. the tag page of each group completed */
  for (int k = 0; k < f->ntagpg; k++) {
    x_pwrite(f->fd, f->tagpg[k], PG, tagpage_off(f->tagpg_p[k]));
    f->tag_pages++;
  }
  /* 3. the frame: TAIL of the bytes since the previous frame that are not
   * home, HOME for the pages sent home, GROUP for each tag page */
  uint64_t tail_start = f->len / PG * PG;
  if (tail_start < f->frame_from) tail_start = f->frame_from;
  size_t tail_n = f->len - tail_start;
  size_t body = 0;
  fb_room(f, 64 + tail_n + 13 + nst * 20 + 4 * 28 + 16 + 16 + 16 + PG + GROUP * 20 + f->ngm * 20 + 64);
  uint8_t *p = f->fb + 16;
  if (tail_n) {
    p[body] = 1;
    put64(p + body + 1, tail_start);
    put32(p + body + 9, (uint32_t)tail_n);
    memcpy(p + body + 13, f->tail + (tail_start % PG), tail_n);
    body += 13 + tail_n;
    f->tail_bytes += tail_n;
  }
  if (nst) {
    p[body] = 2;
    put64(p + body + 1, first_home);
    put32(p + body + 9, (uint32_t)nst);
    body += 13;
    memcpy(p + body, hometags, nst * 20);
    body += nst * 20;
  }
  for (int k = 0; k < f->ntagpg; k++) {
    p[body] = 3;
    put64(p + body + 1, f->tagpg_p[k] >> 7);
    put32(p + body + 9, f->tagver[k]);
    memcpy(p + body + 13, f->tagmac[k], 16);
    body += 29;
  }
  size_t flen = frame_seal(f, 0, 1, body);
  f->frames++;
  f->frame_bytes += flen;
  f->since_ckpt += flen;
  size_t total = flen;
  int ck = 0;
  if (f->since_ckpt > CKPT_EVERY) {
    /* a CKPT at write time, behind the frame in the same ring write */
    uint8_t *c = f->fb + total + 16;
    size_t cb = 0;
    c[cb] = 5;
    put64(c + cb + 1, f->len);
    put64(c + cb + 9, f->synced_home);
    put64(c + cb + 17, f->home);
    cb += 25;
    memcpy(c + cb, f->tail, PG);
    cb += PG;
    uint32_t open_n = (uint32_t)(f->home & (GROUP - 1));
    put32(c + cb, open_n);
    cb += 4;
    for (uint32_t i = 0; i < open_n; i++) {
      memcpy(c + cb, f->gtags[i] + 16, 4);
      memcpy(c + cb + 4, f->gtags[i], 16);
      cb += 20;
    }
    put32(c + cb, 0); /* no overrides */
    cb += 4;
    put32(c + cb, (uint32_t)f->ngm);
    cb += 4;
    memcpy(c + cb, f->gm, f->ngm * 20);
    cb += f->ngm * 20;
    size_t clen = frame_seal(f, total, 5, cb);
    f->ckpts++;
    f->ckpt_bytes += clen;
    total += clen;
    ck = 1;
  }
  uint64_t at = ring_alloc(f, total);
  x_pwrite(f->fd, f->fb, total, at);
  if (ck) {
    f->pin_lo = at + flen;
    f->since_ckpt = 0;
  }
  f->ring_head = at + total;
  f->frame_from = f->len;
}

static void pf_hint(pf *f, uint64_t at, uint64_t len) {
  /* a writeback hint over logical [at, at+len), translated to the stored
   * data pages (section 2, writeback) */
  uint64_t p0 = at / PG;
  x_sfr(f->fd, data_off(p0), len);
}

static void pf_sync(pf *f) {
  x_fdatasync(f->fd);
  f->synced_home = f->home;
}

/* ---- the direct run's write calls (db.rs:6855-6936, 5707-5754) ----------- */
typedef struct {
  uint64_t written, end;
} run;

typedef void (*wcall_fn)(void *, const uint8_t *, size_t);
typedef void (*hint_fn)(void *, uint64_t, uint64_t);

static void run_commit(run *r, uint64_t add, wcall_fn wc, hint_fn hn, void *ctx) {
  uint64_t end = r->end + add;
  r->end = end;
  /* mark_stream -> stream_flush(false): the whole pieces the batch completed */
  uint64_t target = end / PIECE * PIECE;
  while (r->written < target) {
    uint64_t stop = (r->written / PIECE + 1) * PIECE;
    if (stop > target) stop = target;
    wc(ctx, src, stop - r->written);
    r->written = stop;
    if (stop % PIECE == 0) hn(ctx, stop - PIECE, PIECE);
  }
  /* sync -> stream_flush(true): the rest */
  if (r->written < end) {
    wc(ctx, src, end - r->written);
    r->written = end;
  }
}

static void a_wc(void *c, const uint8_t *b, size_t n) { pf_write((pf *)c, b, n); }
static void a_hn(void *c, uint64_t at, uint64_t len) { pf_hint((pf *)c, at, len); }
typedef struct {
  int fd;
} plainf;
static void b_wc(void *c, const uint8_t *b, size_t n) { x_writev1(((plainf *)c)->fd, b, n); }
static void b_hn(void *c, uint64_t at, uint64_t len) { x_sfr(((plainf *)c)->fd, at, len); }

/* ---- the WAL (db.rs:4436-4478) ------------------------------------------- */
typedef struct {
  int fd;
  uint64_t written, synced, dropped;
  EVP_CIPHER_CTX *c; /* D: the log's cipher */
  uint8_t *sealed;
  uint64_t nonce_ctr;
} wal;

static void wal_commit(wal *w, uint64_t pending) {
  if (!w->c) {
    x_write(w->fd, src, pending);
    w->written += pending;
  } else {
    uint8_t *o = w->sealed;
    put32(o, (uint32_t)pending);
    uint8_t iv[12];
    put64(iv, w->nonce_ctr++);
    put32(iv + 8, 0x5157);
    memcpy(o + 4, iv, 12);
    memcpy(o + REC_HEAD, src, pending);
    uint8_t aad[20];
    put64(aad, 7);
    put64(aad + 8, w->written);
    put32(aad + 16, (uint32_t)pending);
    seal(w->c, iv, aad, 20, o + REC_HEAD, (int)pending, o + REC_HEAD + pending);
    size_t n = REC_HEAD + pending + TAGB;
    x_write(w->fd, o, n);
    w->written += n;
  }
  x_fdatasync(w->fd);
  w->synced = w->written;
  uint64_t floor = w->synced & ~(PG - 1);
  if (floor >= w->dropped + WAL_DROP) {
    x_fadvise_dontneed(w->fd, w->dropped, floor - w->dropped);
    w->dropped = floor;
  }
}

/* ---- the driver ------------------------------------------------------------ */
static int cmp_u64(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return x < y ? -1 : x > y;
}
static double pct(uint64_t *v, size_t n, double q) {
  uint64_t *s = malloc(n * sizeof *s);
  memcpy(s, v, n * sizeof *s);
  qsort(s, n, sizeof *s, cmp_u64);
  size_t i = (size_t)(q * (n - 1) + 0.5);
  double r = s[i] / 1000.0;
  free(s);
  return r;
}

int main(int argc, char **argv) {
  if (argc < 5) {
    fprintf(stderr, "usage: e5 <A|A0|B|Cf|Cp|D> <commit bytes> <commits> <dir> [label]\n");
    return 1;
  }
  const char *shape = argv[1];
  uint64_t n = strtoull(argv[2], 0, 10);
  uint64_t commits = strtoull(argv[3], 0, 10);
  const char *dir = argv[4];
  const char *label = argc > 5 ? argv[5] : "-";
  int is_run = !strcmp(shape, "A") || !strcmp(shape, "A0") || !strcmp(shape, "Ap") || !strcmp(shape, "Aw") || !strcmp(shape, "B") || !strcmp(shape, "D");
  int is_a = shape[0] == 'A';
  int a_pre = !strcmp(shape, "Ap");
  int a_wt = !strcmp(shape, "A0") ? 0 : !strcmp(shape, "Aw") ? 2 : 1;
  /* what a commit appends: a run's records and its marker; a WAL batch's
   * frames and its commit frame */
  uint64_t add = is_run ? n + MARK : n + WAL_COMMIT;
  src_len = add + RUN_PREFIX + PIECE + 64;
  src = malloc(src_len + 64);
  RAND_bytes(src, (int)src_len);

  char path[4096], path2[4096];
  if (is_a) {
    /* a sealed spelling: x + 252 base64url characters (section 3, Names) */
    char nm[254];
    static const char b64[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    uint8_t r[252];
    RAND_bytes(r, sizeof r);
    nm[0] = 'x';
    for (int i = 0; i < 252; i++) nm[i + 1] = b64[r[i] & 63];
    nm[253] = 0;
    snprintf(path, sizeof path, "%s/%s", dir, nm);
  } else {
    snprintf(path, sizeof path, "%s/e5-%s-%d.f", dir, shape, getpid());
  }
  snprintf(path2, sizeof path2, "%s/e5-%s-%d.wal", dir, shape, getpid());

  /* Cp: the spare prefilled before the window, as recycle_wal leaves it */
  wal w;
  memset(&w, 0, sizeof w);
  uint64_t prefill_sect = 0, prefill_fsync = 0, prefill_bytes = 0;
  if (!strcmp(shape, "Cp")) {
    devstat p0 = read_dev();
    w.fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (w.fd < 0) die("open Cp");
    uint8_t zeros[4096];
    memset(zeros, 0, sizeof zeros);
    uint64_t need = WAL_HDR + commits * add;
    need = (need + PG - 1) / PG * PG;
    x_write(w.fd, zeros, WAL_HDR);
    uint64_t at = WAL_HDR;
    while (at < need) {
      uint64_t k = need - at < PG ? need - at : PG;
      x_write(w.fd, zeros, k);
      at += k;
    }
    x_fsync(w.fd);
    prefill_fsync = 1;
    prefill_bytes = need;
    devstat p1 = read_dev();
    prefill_sect = p1.v[6] - p0.v[6];
    memset(&sc, 0, sizeof sc);
  }

  if (a_pre) {
    /* vault::preallocate's Pg arm (section 4, Preallocate): zeros at the
     * stored offsets every commit will write -- the ring, the tag region
     * and the data pages -- in 4 KiB writes, then sync_all */
    devstat p0 = read_dev();
    int pfd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (pfd < 0) die("open Ap");
    uint8_t zeros[4096];
    memset(zeros, 0, sizeof zeros);
    uint64_t pages = (RUN_PREFIX + commits * add) / PG + 2;
    uint64_t need = data_off(pages);
    for (uint64_t at = 0; at < need; at += PG) x_write(pfd, zeros, PG);
    x_fsync(pfd);
    close(pfd);
    prefill_fsync = 1;
    prefill_bytes = need;
    devstat p1 = read_dev();
    prefill_sect = p1.v[6] - p0.v[6];
    memset(&sc, 0, sizeof sc);
  }
  sync();
  usleep(300000);
  devstat d0 = read_dev();
  uint64_t wb0 = read_wbytes();
  struct rusage ru0, ru1;
  getrusage(RUSAGE_SELF, &ru0);
  uint64_t t0 = now_ns();

  pf f;
  plainf pl = {-1};
  run r = {0, 0};
  int memfd = -1;
  if (is_a) {
    size_t back = ((RUN_PREFIX + commits * add) / PG + 2) * PG;
    pf_create(&f, path, a_wt, back, a_pre);
  } else if (!strcmp(shape, "B")) {
    pl.fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (pl.fd < 0) die("open B");
  } else if (!strcmp(shape, "D")) {
    memfd = memfd_create("run", MFD_CLOEXEC);
    if (memfd < 0) die("memfd");
    pl.fd = memfd;
    w.fd = open(path2, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (w.fd < 0) die("open D wal");
    uint8_t key[32];
    RAND_bytes(key, 32);
    w.c = gcm_new(key);
    w.sealed = malloc(add + 64);
    uint8_t h[WAL_SEALED_HDR];
    RAND_bytes(h, sizeof h);
    x_write(w.fd, h, sizeof h);
    w.written = sizeof h;
  } else if (!strcmp(shape, "Cf")) {
    w.fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (w.fd < 0) die("open Cf");
    uint8_t h[WAL_HDR] = {'S', 'U', 'P', 'D', 'B', 'W', 'L', 4};
    x_write(w.fd, h, WAL_HDR);
    w.written = WAL_HDR;
  } else if (!strcmp(shape, "Cp")) {
    uint8_t h[WAL_HDR] = {'S', 'U', 'P', 'D', 'B', 'W', 'L', 4};
    x_pwrite(w.fd, h, WAL_HDR, 0);
    if (lseek(w.fd, WAL_HDR, SEEK_SET) < 0) die("lseek");
    w.written = WAL_HDR;
  } else {
    fprintf(stderr, "unknown shape %s\n", shape);
    return 1;
  }
  uint64_t t_create = now_ns() - t0;

  uint64_t *lat = malloc(commits * sizeof *lat), *lw = malloc(commits * sizeof *lw),
           *ls = malloc(commits * sizeof *ls);
  for (uint64_t i = 0; i < commits; i++) {
    uint64_t a = add + (i == 0 && is_run ? RUN_PREFIX : 0);
    uint64_t c0 = now_ns(), c1;
    if (is_a) {
      run_commit(&r, a, a_wc, a_hn, &f);
      c1 = now_ns();
      pf_sync(&f);
    } else if (!strcmp(shape, "B")) {
      run_commit(&r, a, b_wc, b_hn, &pl);
      c1 = now_ns();
      x_fdatasync(pl.fd);
    } else if (!strcmp(shape, "D")) {
      run_commit(&r, a, b_wc, b_hn, &pl);
      x_fdatasync(memfd); /* the run's sync, of a memory file */
      c1 = now_ns();
      wal_commit(&w, n + WAL_COMMIT);
    } else {
      c1 = c0;
      wal_commit(&w, a);
    }
    uint64_t c2 = now_ns();
    lat[i] = c2 - c0;
    lw[i] = c1 - c0;
    ls[i] = c2 - c1;
  }
  uint64_t t1 = now_ns();
  getrusage(RUSAGE_SELF, &ru1);
  devstat d1 = read_dev();
  uint64_t wb1 = read_wbytes();
  sync();
  usleep(100000);
  devstat d2 = read_dev();

  struct stat st;
  fstat(is_a ? f.fd : (!strcmp(shape, "B") ? pl.fd : w.fd), &st);
  double cpu_us = ((ru1.ru_utime.tv_sec - ru0.ru_utime.tv_sec) * 1e6 + (ru1.ru_utime.tv_usec - ru0.ru_utime.tv_usec) +
                   (ru1.ru_stime.tv_sec - ru0.ru_stime.tv_sec) * 1e6 + (ru1.ru_stime.tv_usec - ru0.ru_stime.tv_usec));
  double sys_us = (ru1.ru_stime.tv_sec - ru0.ru_stime.tv_sec) * 1e6 + (ru1.ru_stime.tv_usec - ru0.ru_stime.tv_usec);
  double C = (double)commits;
  printf("label=%s shape=%s n=%lu commits=%lu add=%lu", label, shape, (unsigned long)n, (unsigned long)commits,
         (unsigned long)add);
  printf(" med_us=%.1f p90_us=%.1f p99_us=%.1f mean_us=%.1f", pct(lat, commits, 0.5), pct(lat, commits, 0.9),
         pct(lat, commits, 0.99), (t1 - t0 - t_create) / 1e3 / C);
  printf(" wpart_med_us=%.1f spart_med_us=%.1f spart_p90_us=%.1f", pct(lw, commits, 0.5), pct(ls, commits, 0.5),
         pct(ls, commits, 0.9));
  printf(" create_us=%.1f window_ms=%.1f cpu_us_per=%.1f sys_us_per=%.1f", t_create / 1e3, (t1 - t0) / 1e6, cpu_us / C,
         sys_us / C);
  printf(" dev_wr_ios=%llu dev_wr_sect=%llu dev_flush=%llu dev_discard_sect=%llu", d1.v[4] - d0.v[4], d1.v[6] - d0.v[6],
         d1.v[15] - d0.v[15], d1.v[13] - d0.v[13]);
  printf(" sect_per=%.2f ios_per=%.3f flush_per=%.3f late_sect=%llu late_ios=%llu", (d1.v[6] - d0.v[6]) / C,
         (d1.v[4] - d0.v[4]) / C, (d1.v[15] - d0.v[15]) / C, d2.v[6] - d1.v[6], d2.v[4] - d1.v[4]);
  printf(" wbytes=%lu wbytes_per=%.0f", (unsigned long)(wb1 - wb0), (wb1 - wb0) / C);
  printf(" sys_pwritev=%lu sys_pwrite=%lu sys_write=%lu sys_writev=%lu sys_fdatasync=%lu sys_fsync=%lu sys_sfr=%lu "
         "sys_fadvise=%lu handed=%lu handed_per=%.0f",
         (unsigned long)sc.pwritev, (unsigned long)sc.pwrite, (unsigned long)sc.write, (unsigned long)sc.writev,
         (unsigned long)sc.fdatasync, (unsigned long)sc.fsync, (unsigned long)sc.sfr, (unsigned long)sc.fadvise,
         (unsigned long)sc.bytes, sc.bytes / C);
  printf(" seals=%lu seal_bytes=%lu st_size=%ld st_blocks=%ld", (unsigned long)n_seal, (unsigned long)seal_bytes,
         (long)st.st_size, (long)st.st_blocks);
  if (is_a)
    printf(" wt_calls=%lu", (unsigned long)f.wt_calls);
  if (is_a)
    printf(" frames=%lu frame_bytes=%lu ckpts=%lu ckpt_bytes=%lu wraps=%lu pages_home=%lu tag_pages=%lu tail_bytes=%lu",
           (unsigned long)f.frames, (unsigned long)f.frame_bytes, (unsigned long)f.ckpts, (unsigned long)f.ckpt_bytes,
           (unsigned long)f.wraps, (unsigned long)f.pages_home, (unsigned long)f.tag_pages,
           (unsigned long)f.tail_bytes);
  if (prefill_fsync)
    printf(" prefill_bytes=%lu prefill_sect=%lu prefill_fsync=%lu", (unsigned long)prefill_bytes,
           (unsigned long)prefill_sect, (unsigned long)prefill_fsync);
  printf("\n");
  const char *dump = getenv("E5_DUMP");
  if (dump) {
    FILE *o = fopen(dump, "w");
    if (o) {
      for (uint64_t i = 0; i < commits; i++)
        fprintf(o, "%lu %lu %lu\n", (unsigned long)lat[i], (unsigned long)lw[i], (unsigned long)ls[i]);
      fclose(o);
    }
  }
  if (is_a) close(f.fd);
  if (pl.fd >= 0 && pl.fd != memfd) close(pl.fd);
  if (memfd >= 0) close(memfd);
  if (w.fd > 0) close(w.fd);
  if (!getenv("E5_KEEP")) {
    unlink(path);
    unlink(path2);
  }
  return 0;
}
