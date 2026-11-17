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

/* ==== the fix's candidates (work/fix/design-adjacent, work/fix/design-free) ====
 * Each keeps E5's A conventions: the stored layout of design.md section 3,
 * AES-256-GCM sealing of every page, record and frame, B's write calls
 * (run_commit) and their writeback hints, then fdatasync. */

/* verification hooks (E5_KEYS, E5_PLAIN): the key material and the logical
 * plaintext, for verify.py; off unless asked, outside every timed run */
static void dump_keys(const uint8_t *a, size_t an, const uint8_t *b, size_t bn) {
  const char *k = getenv("E5_KEYS");
  if (!k) return;
  FILE *o = fopen(k, "w");
  if (!o) die("E5_KEYS");
  fwrite(a, 1, an, o);
  if (bn) fwrite(b, 1, bn, o);
  fclose(o);
}
static FILE *plain_out;
static void dump_plain(const uint8_t *b, size_t n) {
  if (plain_out) fwrite(b, 1, n, plain_out);
}

/* The header page of design.md section 3, written once at create and
 * unsynced (pf_create's, factored): returns the session key's context. */
static EVP_CIPHER_CTX *pf_header(int fd, uint8_t file_id[16], uint8_t mode) {
  RAND_bytes(file_id, 16);
  uint8_t ks[32], kf[48], sk[32], t[16];
  RAND_bytes(ks, 32);
  RAND_bytes(sk, 32);
  EVP_CIPHER_CTX *cs = gcm_new(ks);
  memset(kf, 0, sizeof kf);
  seal(cs, file_id, (const uint8_t *)"pfkdf", 5, kf, 32, t);
  EVP_CIPHER_CTX *cf = gcm_new(kf);
  uint8_t hdr[PG];
  memset(hdr, 0, PG);
  memcpy(hdr, "SUPDBPF2", 8);
  put16(hdr + 8, 2);
  hdr[10] = 2; /* class Log */
  memcpy(hdr + 11, file_id, 16);
  RAND_bytes(hdr + 27, 12);
  uint8_t *body = hdr + 39;
  put64(body, 1);
  body[8] = 1;
  uint8_t *e = body + 9;
  RAND_bytes(e, 12);
  memcpy(e + 12, sk, 32);
  seal(cf, e, (const uint8_t *)"pfk", 3, e + 12, 32, e + 44);
  put64(e + 60, 0);
  e[68] = mode; /* ADJ: 0; JRP: interleaved address mode */
  put64(e + 69, 0);
  int body_len = 9 + 77;
  seal(cf, hdr + 27, (const uint8_t *)"pfh", 3, body, body_len, body + body_len);
  x_pwrite(fd, hdr, PG, 0);
  EVP_CIPHER_CTX_free(cs);
  EVP_CIPHER_CTX_free(cf);
  dump_keys(file_id, 16, sk, 32);
  return gcm_new(sk);
}

/* data page p sealed in place at (p, ctr): nonce 0x10|p u56|ctr u32, AAD
 * file_id|p|ctr|valid_len (design.md section 3) */
static void seal_page(EVP_CIPHER_CTX *s, const uint8_t fid[16], uint64_t p, uint32_t ctr, uint8_t *pg, uint8_t tag[16]) {
  uint8_t iv[12] = {0x10};
  memcpy(iv + 1, &p, 7);
  put32(iv + 8, ctr);
  uint8_t aad[30];
  memcpy(aad, fid, 16);
  put64(aad + 16, p);
  put32(aad + 24, ctr);
  put16(aad + 28, 0);
  seal(s, iv, aad, 30, pg, (int)PG, tag);
}

/* the plaintext of the pages not home, a window of WINP pages indexed by p */
#define WINP 1024ull
static void win_put(uint8_t *pt, uint64_t at, const uint8_t *b, size_t n) {
  while (n) {
    uint64_t o = at % (WINP * PG);
    size_t k = WINP * PG - o < n ? WINP * PG - o : n;
    memcpy(pt + o, b, k);
    at += k;
    b += k;
    n -= k;
  }
}
static const uint8_t *win_page(const uint8_t *pt, uint64_t p) { return pt + (p % WINP) * PG; }

static void wt_pwrite(int bfd, size_t back_len, uint64_t at, const uint8_t *b, size_t n) {
  if (at + n > back_len) die("backing too small");
  if (pwrite(bfd, b, n, at) != (ssize_t)n) die("pwrite backing");
}
static int wt_backing(size_t back_len) {
  int mfd = memfd_create("pf-backing", MFD_CLOEXEC);
  if (mfd < 0) die("memfd");
  if (ftruncate(mfd, back_len)) die("ftruncate");
  return mfd;
}

/* ---- ADJ: a Log commit with its record beside the data (design-adjacent sec. 3),
 * with the model's fix A1 (case 1 falls to case 4 when a home of H is in P) and
 * its rules A2 (synced_home = min(Q u {T}) of the record registered at the
 * sync's issue), A3 (case 2 needs H contiguous) and A4 (a carry meeting P falls
 * to case 4). Shape O: M = 8. Shape T: M = 0, case 4 for every m >= 1. */
#define ADJ_SLOTS 8
#define ADJ_SLOT0 8192ull
#define ADJ_SLOTB (2 * PG)
#define ADJ_RHEAD 32
#define ADJ_RBODY_FIXED (8 + 8 + 8 + 4 + 4 + 16 + 4 + 4 + 2 + 4) /* L sh F chain_g chain_ctr mac n_open n_pend tail_len n_ovr */

typedef struct {
  int kind; /* 0 none, 1 ring slots, 2 data area */
  int slot, nslots;
  uint64_t pg, npg; /* data pages whose homes hold it */
  uint64_t qmin;    /* min(Q u {T}) of its call: synced_home if a sync covers it (A2) */
} recloc;

typedef struct {
  int fd, wt, bfd;
  size_t back_len;
  uint64_t wt_calls;
  EVP_CIPHER_CTX *s;
  uint8_t file_id[16];
  uint32_t ctr;
  uint64_t seq, L;
  uint8_t *pt, *stage;
  uint64_t Q[256];
  int nq;
  uint8_t ot[256][20]; /* tag16|ctr of each home page in an open group, by p & 255 */
  int64_t chain_g;
  uint32_t chain_ctr;
  uint8_t chain_mac[16];
  uint16_t *ghome;
  size_t ngroups;
  uint8_t tpg[4][PG];
  uint64_t tpg_g[4];
  int ntpg;
  int M, mh[4], nmh;
  recloc pc, sy;
  uint64_t synced_home;
  /* counters */
  uint64_t cases[6], carried, recs, rec_pages_ring, rec_pages_data, rec_bytes, pages_home, tag_pages, home_runs, ring_writes;
  uint64_t fb_noncontig, fb_reach, fb_carryP, fb_qmax, fb_extent, fb_case1;
} adj;

static size_t adj_rec_bytes(uint64_t n_open, uint64_t n_pend, uint64_t t) {
  return ADJ_RHEAD + ADJ_RBODY_FIXED + 20 * n_open + (8 + PG) * n_pend + t + 16;
}
static uint64_t adj_pages(size_t b) { return (b + PG - 1) / PG; }

static void adj_create(adj *f, const char *path, int wt, size_t back_len, int M) {
  memset(f, 0, sizeof *f);
  f->fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
  if (f->fd < 0) die("open adj");
  f->s = pf_header(f->fd, f->file_id, 0);
  f->pt = malloc(WINP * PG);
  if (posix_memalign((void **)&f->stage, PG, (4ull << 20))) die("stage");
  f->ngroups = back_len / (GROUP * PG) + 8;
  f->ghome = calloc(f->ngroups, sizeof *f->ghome);
  f->chain_g = -1;
  f->M = M;
  f->wt = wt;
  f->back_len = back_len;
  if (wt) f->bfd = wt_backing(back_len);
}

static int adj_protected_page(adj *f, uint64_t p) {
  recloc *r[2] = {&f->pc, &f->sy};
  for (int i = 0; i < 2; i++)
    if (r[i]->kind == 2 && p >= r[i]->pg && p < r[i]->pg + r[i]->npg) return 1;
  return 0;
}
static int adj_range_protected(adj *f, uint64_t lo, uint64_t n) {
  for (uint64_t p = lo; p < lo + n; p++)
    if (adj_protected_page(f, p)) return 1;
  return 0;
}
static int adj_slot_busy(adj *f, int s) {
  recloc *r[2] = {&f->pc, &f->sy};
  for (int i = 0; i < 2; i++)
    if (r[i]->kind == 1 && s >= r[i]->slot && s < r[i]->slot + r[i]->nslots) return 1;
  return 0;
}
static int adj_ring_slot(adj *f, int ns) {
  for (int s = 0; s + ns <= ADJ_SLOTS; s++) {
    int ok = 1;
    for (int k = 0; k < ns; k++)
      if (adj_slot_busy(f, s + k)) ok = 0;
    if (ok) return s;
  }
  fprintf(stderr, "adj: no free ring run of %d slots\n", ns);
  exit(2);
}
static int in_set(const uint64_t *v, int n, uint64_t p) {
  for (int i = 0; i < n; i++)
    if (v[i] == p) return 1;
  return 0;
}
static uint64_t adj_open_count(adj *f, uint64_t T, const uint64_t *pend, int np) {
  uint64_t c = 0;
  for (uint64_t p = (uint64_t)(f->chain_g + 1) * GROUP; p < T; p++)
    if (!in_set(pend, np, p)) c++;
  return c;
}

/* page p sealed into dst and marked home; a group whose 128 pages are home
 * closes: its tag page sealed, chained to the previous one's MAC (design-adjacent
 * sec. 3, "tag pages are chained") */
static void adj_home_page(adj *f, uint64_t p, uint8_t *dst) {
  memcpy(dst, win_page(f->pt, p), PG);
  uint8_t tag[16];
  seal_page(f->s, f->file_id, p, f->ctr, dst, tag);
  uint8_t *e = f->ot[p & 255];
  memcpy(e, tag, 16);
  put32(e + 16, f->ctr);
  f->ctr++;
  f->pages_home++;
  uint64_t g = p / GROUP;
  if (g >= f->ngroups) die("groups");
  if (++f->ghome[g] == GROUP) {
    if ((int64_t)g != f->chain_g + 1) die("adj: groups closed out of order");
    int k = f->ntpg++;
    if (k >= 4) die("adj: too many tag pages in a call");
    /* 128 entries of tag16|ctr u32 at a 24-byte stride, the previous tag
     * page's ctr, sealed as 4080 bytes with the page's own tag in its last
     * 16 (a reader walks the chain back from the record's (g, ctr, MAC):
     * as written the chain names no ctr and stores no MAC) */
    uint8_t *tp = f->tpg[k];
    memset(tp, 0, PG);
    for (uint64_t i = 0; i < GROUP; i++) {
      memcpy(tp + i * 24, f->ot[(g * GROUP + i) & 255], 20);
      memset(f->ot[(g * GROUP + i) & 255], 0, 20);
    }
    put32(tp + GROUP * 24, f->chain_ctr);
    uint8_t tiv[12] = {0x12};
    memcpy(tiv + 1, &g, 7);
    put32(tiv + 8, f->ctr);
    uint8_t taad[44];
    memcpy(taad, f->file_id, 16);
    put64(taad + 16, g);
    put32(taad + 24, f->ctr);
    memcpy(taad + 28, f->chain_mac, 16);
    seal(f->s, tiv, taad, 44, tp, (int)(PG - 16), tp + PG - 16);
    memcpy(f->chain_mac, tp + PG - 16, 16);
    f->chain_ctr = f->ctr;
    f->ctr++;
    f->chain_g = (int64_t)g;
    f->tpg_g[k] = g;
  }
}

/* the self-sufficient record of design-adjacent sec. 3.1, built at dst, sealed;
 * its stored page index `at`; returns its pages */
static uint64_t adj_build_rec(adj *f, uint8_t *dst, uint8_t kind, const uint64_t *pend, int np, uint64_t at) {
  uint64_t T = f->L / PG, t = f->L % PG;
  uint64_t n_open = adj_open_count(f, T, pend, np);
  size_t bytes = adj_rec_bytes(n_open, np, t);
  uint64_t pages = adj_pages(bytes);
  uint8_t *h = dst;
  memset(h, 0, ADJ_RHEAD);
  memcpy(h, "PFR1", 4);
  h[4] = 0;
  h[5] = kind;
  put16(h + 6, (uint16_t)pages);
  put64(h + 8, f->seq);
  /* attempt u24 at 16 = 0, rsv at 19 */
  uint8_t *b = h + ADJ_RHEAD;
  size_t o = 0;
  put64(b + o, f->L);
  o += 8;
  put64(b + o, f->synced_home);
  o += 8;
  put64(b + o, T); /* F: pages below T are home but those pending */
  o += 8;
  put32(b + o, (uint32_t)f->chain_g);
  o += 4;
  put32(b + o, f->chain_ctr);
  o += 4;
  memcpy(b + o, f->chain_mac, 16);
  o += 16;
  put32(b + o, (uint32_t)n_open);
  o += 4;
  for (uint64_t p = (uint64_t)(f->chain_g + 1) * GROUP; p < T; p++)
    if (!in_set(pend, np, p)) {
      memcpy(b + o, f->ot[p & 255], 20);
      o += 20;
    }
  put32(b + o, (uint32_t)np);
  o += 4;
  for (int i = 0; i < np; i++) {
    put64(b + o, pend[i]);
    memcpy(b + o + 8, win_page(f->pt, pend[i]), PG);
    o += 8 + PG;
  }
  put16(b + o, (uint16_t)t);
  o += 2;
  memcpy(b + o, win_page(f->pt, T), t);
  o += t;
  put32(b + o, 0); /* no overrides before finish */
  o += 4;
  put32(h + 20, (uint32_t)o);
  put32(h + 24, (uint32_t)at);
  if (ADJ_RHEAD + o + 16 != bytes) die("adj record size");
  uint8_t iv[12] = {0x11};
  put64(iv + 1, f->seq);
  /* attempt u24 = 0 at iv+9 */
  uint8_t aad[16 + ADJ_RHEAD];
  memcpy(aad, f->file_id, 16);
  memcpy(aad + 16, h, ADJ_RHEAD);
  seal(f->s, iv, aad, (int)sizeof aad, b, (int)o, b + o);
  memset(dst + bytes, 0, pages * PG - bytes);
  f->seq++;
  f->recs++;
  f->rec_bytes += bytes;
  return pages;
}

static void adj_rec_to_ring(adj *f, const uint64_t *pend, int np, uint64_t qmin) {
  uint64_t T = f->L / PG;
  size_t bytes = adj_rec_bytes(adj_open_count(f, T, pend, np), np, f->L % PG);
  int ns = (int)((adj_pages(bytes) + 1) / 2);
  int s = adj_ring_slot(f, ns);
  uint64_t off = ADJ_SLOT0 + (uint64_t)s * ADJ_SLOTB;
  uint8_t *dst = f->stage + (3ull << 20); /* the stage's last MiB */
  uint64_t pages = adj_build_rec(f, dst, 1, pend, np, off / PG);
  x_pwrite(f->fd, dst, pages * PG, off);
  f->ring_writes++;
  f->rec_pages_ring += pages;
  f->pc = (recloc){1, s, ns, 0, 0, qmin};
}

static void adj_write(adj *f, const uint8_t *b, size_t n) {
  if (f->wt) {
    wt_pwrite(f->bfd, f->back_len, f->L, b, n);
    f->wt_calls++;
  }
  uint64_t L0 = f->L, oldT = L0 / PG;
  win_put(f->pt, L0, b, n);
  f->L += n;
  uint64_t T = f->L / PG, t = f->L % PG;
  int m = (int)(T - oldT);
  uint64_t H[512];
  int nh = 0;
  for (int i = 0; i < f->nq; i++) H[nh++] = f->Q[i];
  for (uint64_t p = oldT; p < T; p++) {
    if (nh >= 512) die("adj: H");
    H[nh++] = p;
  }
  f->ntpg = 0;
  int kase;
  uint64_t qmin_after;
  if (m == 0) {
    /* case 0: inside a page, the record to a ring slot */
    kase = 0;
    qmin_after = f->nq ? f->Q[0] : T;
    adj_rec_to_ring(f, f->Q, f->nq, qmin_after);
    f->cases[0]++;
    return;
  }
  /* the m-hat of the next page-completing call: the largest m of the last four */
  int mhat = 1;
  for (int i = 0; i < f->nmh && i < 4; i++)
    if (f->mh[i] > mhat) mhat = f->mh[i];
  f->mh[f->nmh % 4] = m;
  f->nmh++;
  int h_contig = 1;
  for (int i = 1; i < nh; i++)
    if (H[i] != H[i - 1] + 1) h_contig = 0;
  int any_H_protected = 0;
  for (int i = 0; i < nh; i++)
    if (adj_protected_page(f, H[i])) any_H_protected = 1;
  int case1 = t == 0 && n % PIECE == 0 && L0 % PIECE == 0 && f->nq == 0;
  if (case1 && any_H_protected) f->fb_case1++; /* fix A1 */
  kase = 4;
  if (case1 && !any_H_protected)
    kase = 1;
  else if (!case1 && m <= f->M) {
    if (!h_contig) {
      f->fb_noncontig++; /* A3 */
    } else {
      /* case 2: one write over [home(min H), home(T) + r - 1] */
      uint64_t r = adj_pages(adj_rec_bytes(T % GROUP, 0, t));
      uint64_t lo = H[0], hi = T + r - 1;
      int extent_ok = (lo >> 16) == (hi >> 16);
      int prot_rec = adj_range_protected(f, T, r);
      if (extent_ok && !any_H_protected && !prot_rec)
        kase = 2;
      else if (!extent_ok)
        f->fb_extent++;
      else if (any_H_protected) {
        /* case 3: the record carrying H ahead of the next flush's reach */
        uint64_t cr = adj_pages(adj_rec_bytes(adj_open_count(f, T, H, nh), nh, t));
        uint64_t at = T + mhat + 2;
        if (nh > 2 * f->M + 2)
          f->fb_qmax++;
        else if ((at >> 16) != ((at + cr - 1) >> 16))
          f->fb_extent++;
        else if (adj_range_protected(f, at, cr))
          f->fb_carryP++; /* A4 */
        else
          kase = 3;
      } else
        f->fb_reach++; /* m > m-hat: the flush would reach the carried record */
    }
  }
  if (kase == 1 || kase == 4) {
    /* homes of H not in P, one write per contiguous run (case 1: one aligned
     * write per piece, which a run of whole pieces is); blocked pages stay */
    uint64_t keep[512];
    int nk = 0;
    int i = 0;
    while (i < nh) {
      if (adj_protected_page(f, H[i])) {
        keep[nk++] = H[i];
        i++;
        continue;
      }
      int j = i;
      size_t nst = 0;
      while (j < nh && !adj_protected_page(f, H[j]) && (j == i || H[j] == H[j - 1] + 1) &&
             (kase != 1 || j == i || H[j] % (PIECE / PG) != 0)) {
        adj_home_page(f, H[j], f->stage + nst * PG);
        nst++;
        j++;
      }
      x_pwritev(f->fd, &(struct iovec){f->stage, nst * PG}, 1, data_off(H[i]));
      f->home_runs++;
      i = j;
    }
    for (int k = 0; k < f->ntpg; k++) {
      x_pwrite(f->fd, f->tpg[k], PG, tagpage_off(f->tpg_g[k] * GROUP));
      f->tag_pages++;
    }
    memcpy(f->Q, keep, nk * sizeof *keep);
    f->nq = nk;
    qmin_after = nk ? keep[0] : T;
    adj_rec_to_ring(f, f->Q, f->nq, qmin_after);
    f->cases[kase]++;
    return;
  }
  if (kase == 2) {
    size_t nst = 0;
    for (int i = 0; i < nh; i++) adj_home_page(f, H[i], f->stage + (nst++) * PG);
    f->nq = 0;
    uint64_t pages = adj_build_rec(f, f->stage + nst * PG, 2, NULL, 0, data_off(T) / PG);
    x_pwritev(f->fd, &(struct iovec){f->stage, (nst + pages) * PG}, 1, data_off(H[0]));
    f->home_runs++;
    f->rec_pages_data += pages;
    for (int k = 0; k < f->ntpg; k++) {
      x_pwrite(f->fd, f->tpg[k], PG, tagpage_off(f->tpg_g[k] * GROUP));
      f->tag_pages++;
    }
    f->pc = (recloc){2, 0, 0, T, pages, T};
    f->cases[2]++;
    return;
  }
  /* case 3: the carry, nothing home, Q := H */
  uint64_t at = T + mhat + 2;
  uint64_t pages = adj_build_rec(f, f->stage, 3, H, nh, data_off(at) / PG);
  x_pwritev(f->fd, &(struct iovec){f->stage, pages * PG}, 1, data_off(at));
  f->rec_pages_data += pages;
  f->carried += nh;
  memcpy(f->Q, H, nh * sizeof *H);
  f->nq = nh;
  f->pc = (recloc){2, 0, 0, at, pages, H[0]};
  f->cases[3]++;
}

static void adj_sync(adj *f) {
  /* the record of the newest completed call, registered at the issue */
  recloc reg = f->pc;
  x_fdatasync(f->fd);
  f->sy = reg;
  f->synced_home = reg.qmin; /* A2 */
}
static void adj_hint(adj *f, uint64_t at, uint64_t len) { x_sfr(f->fd, data_off(at / PG), len); }

/* ---- JRP: journal, then record page (design-free sec. 5), one RP per call ---- */
#define J_RING0 PG
#define J_RING1 (1ull << 20)
#define J_DATA0 256ull
#define J_TD (64ull << 10)
#define J_TH (128ull << 10)
#define J_FHEAD 16
#define J_RPHEAD 32

typedef struct {
  int fd, wt, bfd;
  size_t back_len;
  uint64_t wt_calls;
  EVP_CIPHER_CTX *s;
  uint8_t file_id[16];
  uint32_t ctr;
  uint64_t seq, L, H, dead, prev;
  uint8_t *pt, *stage, *fb;
  uint8_t (*tags)[16];
  uint64_t ring_head, pin_lo;
  uint64_t (*fq)[2];
  size_t fq_h, fq_t, fq_cap;
  int64_t newest_rp, synced_base_rp;
  uint64_t (*dt)[2];
  size_t ndt, dt_cap;
  /* counters */
  uint64_t homings, homing_pages, rp_slots, rp_bytes, frames, frame_bytes, journaled, wraps, ring_full, pages_home;
} jrp;

static void jrp_create(jrp *f, const char *path, int wt, size_t back_len) {
  memset(f, 0, sizeof *f);
  f->fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
  if (f->fd < 0) die("open jrp");
  f->s = pf_header(f->fd, f->file_id, 1);
  /* the journal ring, fallocated at create (design-free sec. 5.1) */
  if (fallocate(f->fd, 0, J_RING0, J_RING1 - J_RING0)) die("fallocate ring");
  f->pt = malloc(WINP * PG);
  if (posix_memalign((void **)&f->stage, PG, (4ull << 20))) die("stage");
  f->fb = malloc(2ull << 20);
  f->tags = malloc(1100 * 16);
  f->fq_cap = 1 << 16;
  f->fq = malloc(f->fq_cap * sizeof *f->fq);
  f->dt_cap = 1024;
  f->dt = malloc(f->dt_cap * sizeof *f->dt);
  f->ring_head = f->pin_lo = J_RING0;
  f->newest_rp = f->synced_base_rp = -1;
  f->wt = wt;
  f->back_len = back_len;
  if (wt) f->bfd = wt_backing(back_len);
}

static int jrp_ring_alloc(jrp *f, uint64_t need, uint64_t *at) {
  if (f->pin_lo <= f->ring_head) {
    if (f->ring_head + need <= J_RING1) {
      *at = f->ring_head;
      return 1;
    }
    if (J_RING0 + need <= f->pin_lo) {
      f->wraps++;
      *at = J_RING0;
      return 1;
    }
    return 0;
  }
  if (f->ring_head + need <= f->pin_lo) {
    *at = f->ring_head;
    return 1;
  }
  return 0;
}
/* the oldest pinned frame: the first after the synced base RP (P1); frames
 * after the newest RP (P3) are newer still. One thread, no sync in flight
 * beside a write here, so P2 is empty. */
static void jrp_repin(jrp *f) {
  while (f->fq_h < f->fq_t && (int64_t)f->fq[f->fq_h % f->fq_cap][0] <= f->synced_base_rp) f->fq_h++;
  f->pin_lo = f->fq_h < f->fq_t ? f->fq[f->fq_h % f->fq_cap][1] : f->ring_head;
}
static uint64_t jrp_slot(jrp *f, uint64_t p) {
  /* the homing that wrote p: the last with first <= p */
  size_t lo = 0, hi = f->ndt;
  while (hi - lo > 1) {
    size_t mid = (lo + hi) / 2;
    if (f->dt[mid][0] <= p) lo = mid;
    else hi = mid;
  }
  return J_DATA0 + p + f->dt[lo][1];
}

static void jrp_write(jrp *f, const uint8_t *b, size_t n) {
  if (f->wt) {
    wt_pwrite(f->bfd, f->back_len, f->L, b, n);
    f->wt_calls++;
  }
  uint64_t L2 = f->L + n;
  uint64_t J2 = L2 - f->H * PG;
  win_put(f->pt, f->L, b, n);
  int home = n >= J_TD || J2 >= J_TH;
  uint64_t at = 0, flen = J_FHEAD + 8 + n + 16;
  if (!home && !jrp_ring_alloc(f, flen, &at)) {
    home = 1;
    f->ring_full++;
  }
  if (home) {
    uint64_t Lp = L2 / PG, np = Lp - f->H;
    if (np > 1024) die("jrp: an RP covers at most 1024 pages");
    uint32_t ctr0 = f->ctr;
    for (uint64_t k = 0; k < np; k++) {
      uint8_t *dst = f->stage + k * PG;
      memcpy(dst, win_page(f->pt, f->H + k), PG);
      seal_page(f->s, f->file_id, f->H + k, f->ctr++, dst, f->tags[k]);
    }
    f->pages_home += np;
    uint64_t t = L2 % PG;
    uint64_t rp_slot = J_DATA0 + Lp + f->dead;
    size_t body = 8 + 8 + 8 + 4 + 16 * np + 8 + 8 + 4 + 2 + t;
    size_t bytes = J_RPHEAD + body + 16;
    uint64_t n_rp = adj_pages(bytes);
    uint8_t *h = f->stage + np * PG;
    memset(h, 0, J_RPHEAD);
    memcpy(h, "SUPDBRP1", 8);
    put16(h + 8, (uint16_t)n_rp);
    h[10] = 1; /* RP */
    put64(h + 11, f->seq);
    put32(h + 19, 0); /* attempt */
    put32(h + 23, (uint32_t)body);
    uint8_t *q = h + J_RPHEAD;
    size_t o = 0;
    put64(q + o, L2);
    o += 8;
    put64(q + o, Lp);
    o += 8;
    put64(q + o, f->H);
    o += 8;
    put32(q + o, ctr0);
    o += 4;
    memcpy(q + o, f->tags, 16 * np);
    o += 16 * np;
    put64(q + o, f->dead + n_rp);
    o += 8;
    put64(q + o, f->prev);
    o += 8;
    put32(q + o, 0); /* no overrides */
    o += 4;
    put16(q + o, (uint16_t)t);
    o += 2;
    memcpy(q + o, win_page(f->pt, Lp), t);
    o += t;
    if (o != body) die("jrp body");
    uint8_t iv[12] = {0x14};
    put64(iv + 1, f->seq);
    uint8_t aad[16 + 2 + 8 + J_RPHEAD];
    memcpy(aad, f->file_id, 16);
    memcpy(aad + 16, "rp", 2);
    put64(aad + 18, rp_slot);
    memcpy(aad + 26, h, J_RPHEAD);
    seal(f->s, iv, aad, (int)sizeof aad, q, (int)body, q + body);
    memset(h + bytes, 0, n_rp * PG - bytes);
    /* one pwritev of [pages][RP] at slot 256 + H + dead: every slot fresh */
    x_pwritev(f->fd, &(struct iovec){f->stage, (np + n_rp) * PG}, 1, (J_DATA0 + f->H + f->dead) * PG);
    if (f->ndt == f->dt_cap) {
      f->dt_cap *= 2;
      f->dt = realloc(f->dt, f->dt_cap * sizeof *f->dt);
    }
    if (np) {
      f->dt[f->ndt][0] = f->H;
      f->dt[f->ndt][1] = f->dead;
      f->ndt++;
    }
    f->newest_rp = (int64_t)f->seq;
    f->seq++;
    f->prev = rp_slot;
    f->dead += n_rp;
    f->H = Lp;
    f->homings++;
    f->homing_pages += np;
    f->rp_slots += n_rp;
    f->rp_bytes += bytes;
  } else {
    /* a TAIL frame {from = L, bytes} at the ring head */
    uint8_t *h = f->fb;
    memset(h, 0, J_FHEAD);
    put16(h, 0x4a46);
    h[2] = 1; /* TAIL */
    put64(h + 3, f->seq);
    put32(h + 11, (uint32_t)(8 + n));
    put64(h + J_FHEAD, f->L);
    memcpy(h + J_FHEAD + 8, b, n);
    uint8_t iv[12] = {0x11};
    put64(iv + 1, f->seq);
    uint8_t aad[16 + 2 + J_FHEAD];
    memcpy(aad, f->file_id, 16);
    memcpy(aad + 16, "fr", 2);
    memcpy(aad + 18, h, J_FHEAD);
    seal(f->s, iv, aad, (int)sizeof aad, h + J_FHEAD, (int)(8 + n), h + J_FHEAD + 8 + n);
    x_pwrite(f->fd, h, flen, at);
    if (f->fq_t - f->fq_h >= f->fq_cap) die("jrp: frame queue");
    f->fq[f->fq_t % f->fq_cap][0] = f->seq;
    f->fq[f->fq_t % f->fq_cap][1] = at;
    f->fq_t++;
    f->seq++;
    f->ring_head = at + flen;
    f->frames++;
    f->frame_bytes += flen;
    f->journaled += n;
  }
  f->L = L2;
  jrp_repin(f);
}
static void jrp_sync(jrp *f) {
  int64_t base = f->newest_rp; /* the newest RP at or below the seq issued */
  x_fdatasync(f->fd);
  f->synced_base_rp = base;
  jrp_repin(f);
}
static void jrp_hint(jrp *f, uint64_t at, uint64_t len) {
  uint64_t p0 = at / PG, p1 = (at + len) / PG;
  if (p1 > f->H) p1 = f->H;
  if (p1 <= p0) return;
  uint64_t s0 = jrp_slot(f, p0), s1 = jrp_slot(f, p1 - 1) + 1;
  x_sfr(f->fd, s0 * PG, (s1 - s0) * PG);
}

/* ---- the WAL's record per commit: Ds (today), J (design-adjacent sec. 10, with
 * the model's chain fix), S (design-free sec. 4, with the chain fix) ---- */
typedef struct {
  int fd, kind; /* 'D' LogCipher, 'J' journal, 'S' stream */
  EVP_CIPHER_CTX *c;
  uint8_t *o;
  uint8_t file_id[16], store_id[16], prefix[8], prev_tag[16];
  uint64_t written, synced, dropped, count;
  uint64_t recs, rec_over;
} swal;

static size_t swal_header(swal *w, uint8_t *h) {
  uint8_t key[32];
  RAND_bytes(key, 32);
  RAND_bytes(w->file_id, 16);
  RAND_bytes(w->store_id, 16);
  RAND_bytes(w->prefix, 8);
  size_t n;
  if (w->kind == 'D') {
    /* WAL_HEAD_SEALED + kind + a wrapped key (db.rs:4134, 4200-4212) */
    memcpy(h, "SUPDBWE", 7);
    h[7] = 4;
    RAND_bytes(h + 8, 60);
    n = 68;
  } else if (w->kind == 'J') {
    /* the journal's clear name and kind, its file_id and its session key wrapped */
    memcpy(h, "SUPDBPJ", 7);
    h[7] = 4;
    memcpy(h + 8, w->file_id, 16);
    RAND_bytes(h + 24, 60);
    n = 84;
    memcpy(w->prev_tag, h + 68, 16); /* the chain starts at the header's tag */
  } else {
    /* SUPDBST1, version, class, file_id, flags, prev_fid_lo, nonce, sealed hdr_seq, tag */
    memcpy(h, "SUPDBST1", 8);
    put16(h + 8, 1);
    h[10] = 5;
    memcpy(h + 11, w->file_id, 16);
    h[27] = 0;
    put64(h + 28, 0);
    uint8_t kf[48], t[16], iv[12];
    RAND_bytes(iv, 12);
    memset(kf, 0, sizeof kf);
    EVP_CIPHER_CTX *ks = gcm_new(key);
    seal(ks, w->file_id, (const uint8_t *)"pfkdf", 5, kf, 32, t);
    EVP_CIPHER_CTX *cf = gcm_new(kf);
    memcpy(h + 36, iv, 12);
    put64(h + 48, 1);
    seal(cf, iv, (const uint8_t *)"pfs-h", 5, h + 48, 8, h + 56);
    /* K_rec = AEAD_Kf(0x15|0, 0^32).ct */
    uint8_t kr[48], riv[12] = {0x15};
    memset(kr, 0, sizeof kr);
    seal(cf, riv, NULL, 0, kr, 32, t);
    memcpy(key, kr, 32);
    EVP_CIPHER_CTX_free(ks);
    EVP_CIPHER_CTX_free(cf);
    memcpy(w->prev_tag, h + 56, 16);
    n = 72;
  }
  w->c = gcm_new(key);
  {
    uint8_t km[32 + 16 + 16 + 8 + 16];
    memcpy(km, key, 32);
    memcpy(km + 32, w->file_id, 16);
    memcpy(km + 48, w->store_id, 16);
    memcpy(km + 64, w->prefix, 8);
    memcpy(km + 72, w->prev_tag, 16);
    dump_keys(km, sizeof km, NULL, 0);
  }
  return n;
}

static void swal_commit(swal *w, uint64_t pending) {
  uint8_t *o = w->o;
  size_t head, n;
  if (w->kind == 'D') {
    /* vault.rs:1801-1815 seal_into: len u32 | nonce12 | ct | tag16, AAD id|at|len */
    head = REC_HEAD;
    put32(o, (uint32_t)pending);
    uint8_t iv[12];
    memcpy(iv, w->prefix, 8);
    put32(iv + 8, (uint32_t)w->count++);
    memcpy(o + 4, iv, 12);
    memcpy(o + head, src, pending);
    uint8_t aad[20];
    put64(aad, 7);
    put64(aad + 8, w->written);
    put32(aad + 16, (uint32_t)pending);
    seal(w->c, iv, aad, 20, o + head, (int)pending, o + head + pending);
  } else if (w->kind == 'J') {
    /* len u32 | seq u64 | attempt u24 | ct | tag16; nonce 0x13|seq|attempt;
     * AAD file_id | clear part | the previous record's tag */
    head = 15;
    put32(o, (uint32_t)pending);
    put64(o + 4, w->count);
    o[12] = o[13] = o[14] = 0;
    uint8_t iv[12] = {0x13};
    put64(iv + 1, w->count);
    w->count++;
    memcpy(o + head, src, pending);
    uint8_t aad[16 + 15 + 16];
    memcpy(aad, w->file_id, 16);
    memcpy(aad + 16, o, 15);
    memcpy(aad + 31, w->prev_tag, 16);
    seal(w->c, iv, aad, (int)sizeof aad, o + head, (int)pending, o + head + pending);
  } else {
    /* len u32 | nonce12 (prefix8|count u32) | ct | tag16, under K_rec;
     * AAD "pfs"|store_id|file_id|stored offset|len|the previous record's tag */
    head = REC_HEAD;
    put32(o, (uint32_t)pending);
    uint8_t iv[12];
    memcpy(iv, w->prefix, 8);
    put32(iv + 8, (uint32_t)w->count++);
    memcpy(o + 4, iv, 12);
    memcpy(o + head, src, pending);
    uint8_t aad[3 + 16 + 16 + 8 + 4 + 16];
    memcpy(aad, "pfs", 3);
    memcpy(aad + 3, w->store_id, 16);
    memcpy(aad + 19, w->file_id, 16);
    put64(aad + 35, w->written);
    put32(aad + 43, (uint32_t)pending);
    memcpy(aad + 47, w->prev_tag, 16);
    seal(w->c, iv, aad, (int)sizeof aad, o + head, (int)pending, o + head + pending);
  }
  n = head + pending + TAGB;
  memcpy(w->prev_tag, o + head + pending, 16);
  dump_plain(src, pending);
  if (w->kind == 'D')
    x_write(w->fd, o, n); /* Wal::write's write_all on the open_append file (db.rs:4450) */
  else
    x_pwrite(w->fd, o, n, w->written); /* the layer owns the offset */
  w->written += n;
  w->recs++;
  w->rec_over += n - pending;
  x_fdatasync(w->fd);
  w->synced = w->written;
  uint64_t floor = w->synced & ~(PG - 1);
  if (floor >= w->dropped + WAL_DROP) {
    x_fadvise_dontneed(w->fd, w->dropped, floor - w->dropped);
    w->dropped = floor;
  }
}

/* a file prefilled with zeros in 4 KiB writes and fsynced, outside the window
 * (prefill, db.rs:4237-4262), as E5's Cp */
static void prefill_zeros(int fd, uint64_t need) {
  uint8_t zeros[4096];
  memset(zeros, 0, sizeof zeros);
  for (uint64_t at = 0; at < need; at += PG) x_write(fd, zeros, PG);
  x_fsync(fd);
}

static void o_wc(void *c, const uint8_t *b, size_t n) {
  dump_plain(b, n);
  adj_write((adj *)c, b, n);
}
static void o_hn(void *c, uint64_t at, uint64_t len) { adj_hint((adj *)c, at, len); }
static void r_wc(void *c, const uint8_t *b, size_t n) {
  dump_plain(b, n);
  jrp_write((jrp *)c, b, n);
}
static void r_hn(void *c, uint64_t at, uint64_t len) { jrp_hint((jrp *)c, at, len); }

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

static int is(const char *a, const char *b) { return !strcmp(a, b); }

int main(int argc, char **argv) {
  if (argc < 5) {
    fprintf(stderr, "usage: e5 <A|Aw|A0|Ap|Ap0|B|D|Ow|O0|Tw|T0|Rw|R0|Cf|Cp|Ds|Dsp|J|Jp|S|Sp> <commit bytes> <commits> <dir> [label]\n");
    return 1;
  }
  const char *shape = argv[1];
  uint64_t n = strtoull(argv[2], 0, 10);
  uint64_t commits = strtoull(argv[3], 0, 10);
  const char *dir = argv[4];
  const char *label = argc > 5 ? argv[5] : "-";
  int is_a = shape[0] == 'A';
  int is_o = shape[0] == 'O' || shape[0] == 'T';
  int is_r = shape[0] == 'R';
  int is_run = is_a || is_o || is_r || is("B", shape) || is("D", shape);
  int is_swal = is("Ds", shape) || is("Dsp", shape) || is("J", shape) || is("Jp", shape) || is("S", shape) || is("Sp", shape);
  int swal_pre = is_swal && shape[strlen(shape) - 1] == 'p';
  int a_pre = is("Ap", shape) || is("Ap0", shape);
  int a_wt = (is("A0", shape) || is("Ap0", shape)) ? 0 : is("Aw", shape) ? 2 : 1;
  int c_wt = (shape[1] == 'w') ? 2 : 0; /* Ow, Tw, Rw: write-through by pwrite; O0, T0, R0: none */
  if (!is_a && !is_o && !is_r && !is_swal && !is("B", shape) && !is("D", shape) && !is("Cf", shape) && !is("Cp", shape)) {
    fprintf(stderr, "unknown shape %s\n", shape);
    return 1;
  }
  uint64_t add = is_run ? n + MARK : n + WAL_COMMIT;
  src_len = add + RUN_PREFIX + PIECE + 64;
  src = malloc(src_len + 64);
  RAND_bytes(src, (int)src_len);
  if (getenv("E5_PLAIN")) plain_out = fopen(getenv("E5_PLAIN"), "w");

  char path[4096], path2[4096];
  if (is_a || is_o || is_r || is("J", shape) || is("Jp", shape) || is("S", shape) || is("Sp", shape)) {
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

  wal w;
  memset(&w, 0, sizeof w);
  swal sw;
  memset(&sw, 0, sizeof sw);
  uint64_t prefill_sect = 0, prefill_fsync = 0, prefill_bytes = 0;
  if (is("Cp", shape) || swal_pre) {
    /* the spare prefilled before the window, as recycle_wal leaves it */
    devstat p0 = read_dev();
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) die("open prefilled wal");
    uint64_t need = 128 + commits * (add + 32);
    need = (need + PG - 1) / PG * PG;
    prefill_zeros(fd, need);
    if (is("Cp", shape)) w.fd = fd;
    else sw.fd = fd;
    prefill_fsync = 1;
    prefill_bytes = need;
    devstat p1 = read_dev();
    prefill_sect = p1.v[6] - p0.v[6];
    memset(&sc, 0, sizeof sc);
  }
  if (a_pre) {
    /* vault::preallocate's Pg arm (section 4, Preallocate) */
    devstat p0 = read_dev();
    int pfd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (pfd < 0) die("open Ap");
    uint64_t pages = (RUN_PREFIX + commits * add) / PG + 2;
    uint64_t need = data_off(pages);
    prefill_zeros(pfd, need);
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
  adj o;
  jrp jr;
  plainf pl = {-1};
  run r = {0, 0};
  int memfd = -1;
  size_t back = ((RUN_PREFIX + commits * add) / PG + 2) * PG;
  int statfd = -1;
  if (is_a) {
    pf_create(&f, path, a_wt, back, a_pre);
    statfd = f.fd;
  } else if (is_o) {
    adj_create(&o, path, c_wt, back, shape[0] == 'O' ? 8 : 0);
    statfd = o.fd;
  } else if (is_r) {
    jrp_create(&jr, path, c_wt, back);
    statfd = jr.fd;
  } else if (is("B", shape)) {
    pl.fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (pl.fd < 0) die("open B");
    statfd = pl.fd;
  } else if (is("D", shape)) {
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
    statfd = w.fd;
  } else if (is("Cf", shape)) {
    w.fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (w.fd < 0) die("open Cf");
    uint8_t h[WAL_HDR] = {'S', 'U', 'P', 'D', 'B', 'W', 'L', 4};
    x_write(w.fd, h, WAL_HDR);
    w.written = WAL_HDR;
    statfd = w.fd;
  } else if (is("Cp", shape)) {
    uint8_t h[WAL_HDR] = {'S', 'U', 'P', 'D', 'B', 'W', 'L', 4};
    x_pwrite(w.fd, h, WAL_HDR, 0);
    if (lseek(w.fd, WAL_HDR, SEEK_SET) < 0) die("lseek");
    w.written = WAL_HDR;
    statfd = w.fd;
  } else {
    sw.kind = shape[0] == 'D' ? 'D' : shape[0];
    if (!swal_pre) {
      sw.fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
      if (sw.fd < 0) die("open swal");
    }
    sw.o = malloc(add + 64);
    uint8_t h[128];
    size_t hn = swal_header(&sw, h);
    x_pwrite(sw.fd, h, hn, 0);
    if (lseek(sw.fd, hn, SEEK_SET) < 0) die("lseek");
    sw.written = hn;
    statfd = sw.fd;
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
    } else if (is_o) {
      run_commit(&r, a, o_wc, o_hn, &o);
      c1 = now_ns();
      adj_sync(&o);
    } else if (is_r) {
      run_commit(&r, a, r_wc, r_hn, &jr);
      c1 = now_ns();
      jrp_sync(&jr);
    } else if (is("B", shape)) {
      run_commit(&r, a, b_wc, b_hn, &pl);
      c1 = now_ns();
      x_fdatasync(pl.fd);
    } else if (is("D", shape)) {
      run_commit(&r, a, b_wc, b_hn, &pl);
      x_fdatasync(memfd);
      c1 = now_ns();
      wal_commit(&w, n + WAL_COMMIT);
    } else if (is_swal) {
      c1 = c0;
      swal_commit(&sw, a);
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
  fstat(statfd, &st);
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
  printf(" dev_wr_ios=%llu dev_wr_sect=%llu dev_flush=%llu dev_discard_ios=%llu dev_discard_sect=%llu", d1.v[4] - d0.v[4],
         d1.v[6] - d0.v[6], d1.v[15] - d0.v[15], d1.v[11] - d0.v[11], d1.v[13] - d0.v[13]);
  printf(" sect_per=%.2f ios_per=%.3f flush_per=%.3f disc_per=%.3f late_sect=%llu late_ios=%llu", (d1.v[6] - d0.v[6]) / C,
         (d1.v[4] - d0.v[4]) / C, (d1.v[15] - d0.v[15]) / C, (d1.v[11] - d0.v[11]) / C, d2.v[6] - d1.v[6],
         d2.v[4] - d1.v[4]);
  printf(" wbytes=%lu wbytes_per=%.0f", (unsigned long)(wb1 - wb0), (wb1 - wb0) / C);
  printf(" sys_pwritev=%lu sys_pwrite=%lu sys_write=%lu sys_writev=%lu sys_fdatasync=%lu sys_fsync=%lu sys_sfr=%lu "
         "sys_fadvise=%lu handed=%lu handed_per=%.0f",
         (unsigned long)sc.pwritev, (unsigned long)sc.pwrite, (unsigned long)sc.write, (unsigned long)sc.writev,
         (unsigned long)sc.fdatasync, (unsigned long)sc.fsync, (unsigned long)sc.sfr, (unsigned long)sc.fadvise,
         (unsigned long)sc.bytes, sc.bytes / C);
  printf(" seals=%lu seal_bytes=%lu st_size=%ld st_blocks=%ld", (unsigned long)n_seal, (unsigned long)seal_bytes,
         (long)st.st_size, (long)st.st_blocks);
  if (is_a)
    printf(" wt_calls=%lu frames=%lu frame_bytes=%lu ckpts=%lu ckpt_bytes=%lu wraps=%lu pages_home=%lu tag_pages=%lu "
           "tail_bytes=%lu",
           (unsigned long)f.wt_calls, (unsigned long)f.frames, (unsigned long)f.frame_bytes, (unsigned long)f.ckpts,
           (unsigned long)f.ckpt_bytes, (unsigned long)f.wraps, (unsigned long)f.pages_home, (unsigned long)f.tag_pages,
           (unsigned long)f.tail_bytes);
  if (is_o)
    printf(" wt_calls=%lu c0=%lu c1=%lu c2=%lu c3=%lu c4=%lu carried=%lu recs=%lu rec_pages_ring=%lu rec_pages_data=%lu "
           "rec_bytes=%lu pages_home=%lu tag_pages=%lu home_runs=%lu ring_writes=%lu fb_noncontig=%lu fb_reach=%lu "
           "fb_carryP=%lu fb_qmax=%lu fb_extent=%lu fb_case1=%lu",
           (unsigned long)o.wt_calls, (unsigned long)o.cases[0], (unsigned long)o.cases[1], (unsigned long)o.cases[2],
           (unsigned long)o.cases[3], (unsigned long)o.cases[4], (unsigned long)o.carried, (unsigned long)o.recs,
           (unsigned long)o.rec_pages_ring, (unsigned long)o.rec_pages_data, (unsigned long)o.rec_bytes,
           (unsigned long)o.pages_home, (unsigned long)o.tag_pages, (unsigned long)o.home_runs,
           (unsigned long)o.ring_writes, (unsigned long)o.fb_noncontig, (unsigned long)o.fb_reach,
           (unsigned long)o.fb_carryP, (unsigned long)o.fb_qmax, (unsigned long)o.fb_extent, (unsigned long)o.fb_case1);
  if (is_r)
    printf(" wt_calls=%lu homings=%lu homing_pages=%lu rp_slots=%lu rp_bytes=%lu frames=%lu frame_bytes=%lu journaled=%lu "
           "wraps=%lu ring_full=%lu pages_home=%lu",
           (unsigned long)jr.wt_calls, (unsigned long)jr.homings, (unsigned long)jr.homing_pages,
           (unsigned long)jr.rp_slots, (unsigned long)jr.rp_bytes, (unsigned long)jr.frames,
           (unsigned long)jr.frame_bytes, (unsigned long)jr.journaled, (unsigned long)jr.wraps,
           (unsigned long)jr.ring_full, (unsigned long)jr.pages_home);
  if (is_swal) printf(" recs=%lu rec_over=%lu", (unsigned long)sw.recs, (unsigned long)sw.rec_over);
  if (prefill_fsync)
    printf(" prefill_bytes=%lu prefill_sect=%lu prefill_fsync=%lu", (unsigned long)prefill_bytes,
           (unsigned long)prefill_sect, (unsigned long)prefill_fsync);
  printf("\n");
  fflush(stdout);
  if (plain_out) fclose(plain_out);
  const char *dump = getenv("E5_DUMP");
  if (dump) {
    FILE *of = fopen(dump, "w");
    if (of) {
      for (uint64_t i = 0; i < commits; i++)
        fprintf(of, "%lu %lu %lu\n", (unsigned long)lat[i], (unsigned long)lw[i], (unsigned long)ls[i]);
      fclose(of);
    }
  }
  if (is_a) close(f.fd);
  if (is_o) close(o.fd);
  if (is_r) close(jr.fd);
  if (pl.fd >= 0 && pl.fd != memfd) close(pl.fd);
  if (memfd >= 0) close(memfd);
  if (w.fd > 0) close(w.fd);
  if (sw.fd > 0) close(sw.fd);
  if (!getenv("E5_KEEP")) {
    unlink(path);
    unlink(path2);
  }
  return 0;
}
