/* LMDB's page-encryption hook under a real cipher, for the encrypted
 * comparator arms (`lmdb3-enc`, feature `enc`).
 *
 * LMDB's 1.0 line ships the mechanism and a sample module: a page is
 * handed to an application's function with a key, the page's number and
 * transaction as an initialization vector, and room for a tag
 * (`mdb_env_set_encrypt`, `MDB_enc_func`), and `crypto.c` beside it fills
 * the function with libsodium's ChaCha20-Poly1305. This is that function
 * under AES-256-GCM through OpenSSL's EVP, the cipher the engine's own arm
 * runs (`supdb-cipher`, through `ring`) and the library RocksDB's arm runs
 * its own under (`rocks_enc.cc`), in the sample module's shape:
 *
 *   - the nonce is the sample's, twelve bytes: the low four of the page's
 *     number and the eight of its transaction's id;
 *   - the output is written from the input, never in place. That is the
 *     sample's shape and it is load-bearing: two transactions can each
 *     decrypt one page into the buffer they share (a transaction records a
 *     page as decrypted in its own copy of the chunk's bits, `mdb.c`'s
 *     `mdb_rpage_get`), the second while the first reads it, and a function
 *     that only ever writes the plaintext there writes over each byte with
 *     itself. A wrapper that copies the ciphertext into the output and
 *     decrypts it there (`heed3`'s, at the versions read) hands the first
 *     reader a page of ciphertext for the length of a decryption, which
 *     three reader threads meet inside a pass as `MDB_PAGE_NOTFOUND`;
 *   - a context a thread and a direction, keyed once: a page sets the
 *     nonce alone.
 *
 * `struct val` is lmdb.h's `MDB_val`, restated so that this file needs no
 * header of LMDB's: the binding crate names none to its dependents.
 */

#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include <openssl/evp.h>

struct val {
  size_t mv_size;
  void *mv_data;
};

#define KEY 32
#define NONCE 12
#define TAG 16

struct dir {
  EVP_CIPHER_CTX *ctx;
  unsigned char key[KEY];
  int keyed;
};

static _Thread_local struct dir dirs[2];

int supdb_bench_lmdb_encfunc(const struct val *src, struct val *dst,
                             const struct val *key, int encdec) {
  if (key[0].mv_size != KEY || key[1].mv_size < 16 || key[2].mv_size != TAG ||
      dst->mv_size != src->mv_size || src->mv_size > (1u << 30)) {
    return 1;
  }
  encdec = encdec != 0;
  uint64_t pgno, txnid;
  memcpy(&pgno, key[1].mv_data, 8);
  memcpy(&txnid, (const char *)key[1].mv_data + 8, 8);
  uint32_t low = (uint32_t)pgno;
  unsigned char nonce[NONCE];
  memcpy(nonce, &low, 4);
  memcpy(nonce + 4, &txnid, 8);

  struct dir *d = &dirs[encdec];
  if (d->ctx == NULL) {
    d->ctx = EVP_CIPHER_CTX_new();
    if (d->ctx == NULL) {
      return 1;
    }
  }
  if (!d->keyed || memcmp(d->key, key[0].mv_data, KEY) != 0) {
    d->keyed = 0;
    if (EVP_CipherInit_ex(d->ctx, EVP_aes_256_gcm(), NULL, key[0].mv_data,
                          NULL, encdec) != 1) {
      return 1;
    }
    memcpy(d->key, key[0].mv_data, KEY);
    d->keyed = 1;
  }
  if (EVP_CipherInit_ex(d->ctx, NULL, NULL, NULL, nonce, encdec) != 1) {
    return 1;
  }
  if (!encdec && EVP_CIPHER_CTX_ctrl(d->ctx, EVP_CTRL_GCM_SET_TAG, TAG,
                                     key[2].mv_data) != 1) {
    return 1;
  }
  int len = 0, fin = 0;
  if (EVP_CipherUpdate(d->ctx, dst->mv_data, &len, src->mv_data,
                       (int)src->mv_size) != 1) {
    return 1;
  }
  /* On the way in, this is where a tag that does not match is refused. */
  if (EVP_CipherFinal_ex(d->ctx, (unsigned char *)dst->mv_data + len, &fin) !=
      1) {
    return 1;
  }
  if (encdec && EVP_CIPHER_CTX_ctrl(d->ctx, EVP_CTRL_GCM_GET_TAG, TAG,
                                    key[2].mv_data) != 1) {
    return 1;
  }
  return 0;
}
