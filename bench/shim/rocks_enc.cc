// RocksDB's encrypted Env under a real cipher, for the encrypted
// comparator arms (`rocksdb-tuned-enc`, feature `enc`).
//
// RocksDB ships the mechanism and no cipher: `NewEncryptedEnv` wraps an Env
// so that every file it writes is encrypted through an `EncryptionProvider`,
// and the one block cipher in the tree is ROT13, for its tests. A
// deployment brings the provider, and the ones in use (TiKV's, MyRocks')
// are AES in counter mode through OpenSSL's EVP, a file's bytes encrypted
// in place at their offset, with a random IV a file kept in the clear at
// the file's head. This is that, stated in full so the arm can be named:
//
//   - AES-256-CTR, one key a store, a random 16-byte IV a file in the first
//     bytes of a 4096-byte prefix, the counter block for the 16 bytes at
//     file offset `o` being the IV plus `o / 16` as a 128-bit big-endian sum;
//   - a whole read or write in one EVP call (`BlockAccessCipherStream`'s
//     own `Encrypt` walks the range a 16-byte block a virtual call, which is
//     the interface's fallback and not what a deployment runs);
//   - a context a thread, keyed once: a call sets the counter alone.
//
// Not authenticated: counter mode has no tag, and RocksDB's own block
// checksums, which the suite's arms read without verifying, are computed
// over the plaintext. That is the comparator as it is deployed, and the
// difference from an authenticated store is the arm's to state, not this
// file's to paper over.
//
// The handle returned is c.cc's `rocksdb_env_t`, which the C API declares
// opaque and defines as the two fields below; the Rust side hands it to
// `rocksdb::Options::set_env` and its drop calls `rocksdb_env_destroy`,
// which deletes the Env and then this struct.

#include <atomic>
#include <cstdint>
#include <cstring>
#include <memory>
#include <string>

#include <openssl/evp.h>
#include <openssl/rand.h>

#include "rocksdb/env.h"
#include "rocksdb/env_encryption.h"

namespace {

using ROCKSDB_NAMESPACE::BlockAccessCipherStream;
using ROCKSDB_NAMESPACE::EncryptionProvider;
using ROCKSDB_NAMESPACE::Env;
using ROCKSDB_NAMESPACE::EnvOptions;
using ROCKSDB_NAMESPACE::Slice;
using ROCKSDB_NAMESPACE::Status;

constexpr size_t kPrefix = 4096;
constexpr size_t kIv = 16;
constexpr size_t kKey = 32;

std::atomic<uint64_t> next_key_id{1};

// A thread's EVP context and the key it was last given: a store has one
// key, so after a thread's first call each call sets the counter alone.
struct ThreadCtx {
  EVP_CIPHER_CTX* ctx = nullptr;
  uint64_t key_id = 0;
  ~ThreadCtx() {
    if (ctx != nullptr) {
      EVP_CIPHER_CTX_free(ctx);
    }
  }
};

thread_local ThreadCtx tls;

// `iv` plus `blocks`, as 128-bit big-endian integers.
void counter_at(const unsigned char iv[kIv], uint64_t blocks,
                unsigned char out[kIv]) {
  std::memcpy(out, iv, kIv);
  unsigned carry = 0;
  for (int i = kIv - 1; i >= 0; i--) {
    unsigned add = (blocks & 0xff) + carry;
    blocks >>= 8;
    unsigned sum = out[i] + add;
    out[i] = static_cast<unsigned char>(sum);
    carry = sum >> 8;
    if (blocks == 0 && carry == 0) {
      break;
    }
  }
}

class CtrStream final : public BlockAccessCipherStream {
 public:
  CtrStream(uint64_t key_id, const unsigned char key[kKey],
            const unsigned char iv[kIv])
      : key_id_(key_id) {
    std::memcpy(key_, key, kKey);
    std::memcpy(iv_, iv, kIv);
  }

  size_t BlockSize() override { return kIv; }

  // Counter mode is its own inverse.
  Status Encrypt(uint64_t off, char* data, size_t n) override {
    return Crypt(off, data, n);
  }
  Status Decrypt(uint64_t off, char* data, size_t n) override {
    return Crypt(off, data, n);
  }

 protected:
  void AllocateScratch(std::string&) override {}
  Status EncryptBlock(uint64_t, char*, char*) override {
    return Status::NotSupported("the stream encrypts by the range");
  }
  Status DecryptBlock(uint64_t, char*, char*) override {
    return Status::NotSupported("the stream decrypts by the range");
  }

 private:
  Status Crypt(uint64_t off, char* data, size_t n) {
    if (n == 0) {
      return Status::OK();
    }
    unsigned char ctr[kIv];
    counter_at(iv_, off / kIv, ctr);
    ThreadCtx& t = tls;
    if (t.ctx == nullptr) {
      t.ctx = EVP_CIPHER_CTX_new();
      if (t.ctx == nullptr) {
        return Status::MemoryLimit("EVP_CIPHER_CTX_new");
      }
    }
    int ok;
    if (t.key_id == key_id_) {
      ok = EVP_EncryptInit_ex(t.ctx, nullptr, nullptr, nullptr, ctr);
    } else {
      ok = EVP_EncryptInit_ex(t.ctx, EVP_aes_256_ctr(), nullptr, key_, ctr);
      t.key_id = ok == 1 ? key_id_ : 0;
    }
    if (ok != 1) {
      return Status::IOError("EVP_EncryptInit_ex");
    }
    int len = 0;
    // A range that starts inside a block: the keystream before it spent.
    size_t skip = off % kIv;
    if (skip != 0) {
      unsigned char pad[kIv] = {0};
      if (EVP_EncryptUpdate(t.ctx, pad, &len, pad, static_cast<int>(skip)) !=
          1) {
        return Status::IOError("EVP_EncryptUpdate");
      }
    }
    unsigned char* p = reinterpret_cast<unsigned char*>(data);
    while (n > 0) {
      size_t piece = n < (1u << 30) ? n : (1u << 30);
      if (EVP_EncryptUpdate(t.ctx, p, &len, p, static_cast<int>(piece)) != 1) {
        return Status::IOError("EVP_EncryptUpdate");
      }
      p += piece;
      n -= piece;
    }
    return Status::OK();
  }

  uint64_t key_id_;
  unsigned char key_[kKey];
  unsigned char iv_[kIv];
};

class Aes256CtrProvider final : public EncryptionProvider {
 public:
  explicit Aes256CtrProvider(const unsigned char key[kKey])
      : key_id_(next_key_id.fetch_add(1)) {
    std::memcpy(key_, key, kKey);
  }

  const char* Name() const override { return "supdb_bench_aes256ctr"; }

  size_t GetPrefixLength() const override { return kPrefix; }

  Status CreateNewPrefix(const std::string&, char* prefix,
                         size_t len) const override {
    if (len < kIv) {
      return Status::InvalidArgument("a prefix shorter than its IV");
    }
    std::memset(prefix, 0, len);
    if (RAND_bytes(reinterpret_cast<unsigned char*>(prefix), kIv) != 1) {
      return Status::IOError("RAND_bytes");
    }
    return Status::OK();
  }

  Status AddCipher(const std::string&, const char*, size_t, bool) override {
    return Status::NotSupported("one key, given at creation");
  }

  Status CreateCipherStream(
      const std::string&, const EnvOptions&, Slice& prefix,
      std::unique_ptr<BlockAccessCipherStream>* result) override {
    if (prefix.size() < kIv) {
      return Status::Corruption("a file with no IV in its prefix");
    }
    result->reset(new CtrStream(
        key_id_, key_, reinterpret_cast<const unsigned char*>(prefix.data())));
    return Status::OK();
  }

 private:
  uint64_t key_id_;
  unsigned char key_[kKey];
};

}  // namespace

// c.cc's definition, which the C API keeps opaque.
struct rocksdb_env_t {
  ROCKSDB_NAMESPACE::Env* rep;
  bool is_default;
};

extern "C" {

// An Env over the default one that encrypts every file under `key`, 32
// bytes; destroyed by `rocksdb_env_destroy`.
rocksdb_env_t* supdb_bench_rocks_encrypted_env(const unsigned char* key) {
  auto provider = std::make_shared<Aes256CtrProvider>(key);
  rocksdb_env_t* out = new rocksdb_env_t;
  out->rep = ROCKSDB_NAMESPACE::NewEncryptedEnv(Env::Default(), provider);
  out->is_default = false;
  return out;
}

// The counter block for the 16 bytes at block `blocks` of a file whose IV
// is `iv`: the arithmetic alone, for the test that holds it.
void supdb_bench_rocks_counter_at(const unsigned char* iv, uint64_t blocks,
                                  unsigned char* out) {
  counter_at(iv, blocks, out);
}

}  // extern "C"
