// A C interface over FASTER's C++ key-value store (`FasterKv`), for the
// bench's `faster` arm: the suite's shape alone, 16-byte keys and 100-byte
// values, so nothing is serialized and no length is carried.
//
// FASTER is Microsoft's (github.com/microsoft/FASTER, MIT). Its sources are
// not this repository's: `scripts/faster.sh` fetches them to `src/` beside
// this file at the commit `PIN` names, upstream's last, the repository being
// no longer maintained (its successor is Garnet's Tsavorite, in C#). This
// is its C++ store, the one an in-process Rust arm can link.
//
// What the arm asks of it, and how:
// - Keys hash with FASTER's own byte hash (`FasterHash::compute` over the
//   16 bytes), as its own byte-keyed samples do.
// - Values carry a generation word ahead of the 100 bytes: an in-place
//   update (`PutAtomic`) locks it, copies and moves it on, and a read of a
//   record in the mutable region (`GetAtomic`) retries while it is locked
//   or moved. That is the pattern FASTER's own tests use for values wider
//   than a machine word (`GenLock`, `test/test_types.h`); without it an
//   update could only be a copy to the tail, which is not how FASTER is
//   meant to run, and a reader could see half a value.
// - Each thread that touches the store runs a session (`StartSession` on
//   the thread, `StopSession` before it ends), refreshing its epoch every
//   64 operations and completing pending operations every 1,600, the
//   cadence of FASTER's own benchmark (`benchmark-dir/benchmark.h`).
// - A read-modify-write is FASTER's own `Rmw` (`fkv_rmw`): one lookup, the
//   old bytes copied out and the new ones written under the same lock.
// - A read that goes pending (a record not in memory) is completed before
//   it returns, so the caller's buffer is filled when it returns.
// - Durability is CPR's, as FASTER runs it: an index checkpoint once, on
//   the store as opened (`fkv_checkpoint_index`, before anything is
//   timed), and at each `sync` a hybrid-log checkpoint (`fkv_checkpoint`,
//   FASTER's fold-over), returning when it is on the device. Recovery
//   takes the index checkpoint and replays the log from it, so the
//   log-only checkpoint is a durability point on its own; a full one at
//   every sync would also rewrite the index, which FASTER does not ask.

#include <atomic>
#include <cstdint>
#include <cstring>
#include <string>

#include "core/faster.h"
#include "device/file_system_disk.h"

using namespace FASTER::core;

typedef FASTER::environment::QueueIoHandler handler_t;
typedef FASTER::device::FileSystemDisk<handler_t, 1073741824ull> disk_t;

static constexpr uint32_t kKey = 16;
static constexpr uint32_t kValue = 100;

class Key {
 public:
  explicit Key(const uint8_t* b) { std::memcpy(k_, b, kKey); }
  Key(const Key& other) { std::memcpy(k_, other.k_, kKey); }
  inline static constexpr uint32_t size() { return static_cast<uint32_t>(sizeof(Key)); }
  inline KeyHash GetHash() const {
    return KeyHash{ Utility::FasterHash::compute(k_, kKey) };
  }
  inline bool operator==(const Key& other) const { return std::memcmp(k_, other.k_, kKey) == 0; }
  inline bool operator!=(const Key& other) const { return !(*this == other); }

 private:
  uint8_t k_[kKey];
};

class Value {
 public:
  Value() : gen_{ 0 } {}
  Value(const Value& other) : gen_{ other.gen_.load() } { std::memcpy(d_, other.d_, kValue); }
  inline static constexpr uint32_t size() { return static_cast<uint32_t>(sizeof(Value)); }

  /// Even: at rest. Odd: an in-place update is copying.
  std::atomic<uint64_t> gen_;
  uint8_t d_[kValue];
};

class ReadContext : public IAsyncContext {
 public:
  typedef Key key_t;
  typedef Value value_t;
  ReadContext(const uint8_t* key, uint8_t* out) : key_{ key }, out_{ out }, found_{ false } {}
  ReadContext(const ReadContext& other)
    : key_{ other.key_ }, out_{ other.out_ }, found_{ other.found_ } {}
  inline const Key& key() const { return key_; }
  /// A record in the read-only region: nobody writes it in place.
  inline void Get(const value_t& value) {
    std::memcpy(out_, value.d_, kValue);
    found_ = true;
  }
  /// A record in the mutable region: copied between two equal, even
  /// generations.
  inline void GetAtomic(const value_t& value) {
    uint64_t g1, g2;
    do {
      g1 = value.gen_.load(std::memory_order_acquire);
      if (g1 & 1) continue;
      std::memcpy(out_, value.d_, kValue);
      std::atomic_thread_fence(std::memory_order_acquire);
      g2 = value.gen_.load(std::memory_order_relaxed);
      if (g1 == g2) break;
    } while (true);
    found_ = true;
  }
  bool found() const { return found_; }

 protected:
  Status DeepCopy_Internal(IAsyncContext*& context_copy) {
    return IAsyncContext::DeepCopy_Internal(*this, context_copy);
  }

 private:
  Key key_;
  uint8_t* out_;
  bool found_;
};

class UpsertContext : public IAsyncContext {
 public:
  typedef Key key_t;
  typedef Value value_t;
  UpsertContext(const uint8_t* key, const uint8_t* value) : key_{ key } {
    std::memcpy(v_, value, kValue);
  }
  UpsertContext(const UpsertContext& other) : key_{ other.key_ } {
    std::memcpy(v_, other.v_, kValue);
  }
  inline const Key& key() const { return key_; }
  inline static constexpr uint32_t value_size() { return sizeof(value_t); }
  /// A new record at the tail: nobody else sees it yet.
  inline void Put(value_t& value) {
    value.gen_.store(0, std::memory_order_relaxed);
    std::memcpy(value.d_, v_, kValue);
  }
  /// In place: lock the generation, copy, release it moved on.
  inline bool PutAtomic(value_t& value) {
    uint64_t g = value.gen_.load(std::memory_order_relaxed);
    do {
      if (g & 1) {
        g = value.gen_.load(std::memory_order_relaxed);
        continue;
      }
      if (value.gen_.compare_exchange_weak(g, g + 1, std::memory_order_acquire)) break;
    } while (true);
    std::memcpy(value.d_, v_, kValue);
    value.gen_.store(g + 2, std::memory_order_release);
    return true;
  }

 protected:
  Status DeepCopy_Internal(IAsyncContext*& context_copy) {
    return IAsyncContext::DeepCopy_Internal(*this, context_copy);
  }

 private:
  Key key_;
  uint8_t v_[kValue];
};

/// Read-modify-write as FASTER's one operation (`Rmw`), the operation its
/// own benchmark's second workload is made of: the value as it stood into
/// `out`, and the new bytes in its place.
class RmwContext : public IAsyncContext {
 public:
  typedef Key key_t;
  typedef Value value_t;
  RmwContext(const uint8_t* key, const uint8_t* value, uint8_t* out, int* found)
    : key_{ key }, out_{ out }, found_{ found } {
    std::memcpy(v_, value, kValue);
  }
  RmwContext(const RmwContext& other)
    : key_{ other.key_ }, out_{ other.out_ }, found_{ other.found_ } {
    std::memcpy(v_, other.v_, kValue);
  }
  inline const Key& key() const { return key_; }
  inline static constexpr uint32_t value_size() { return sizeof(value_t); }
  inline static constexpr uint32_t value_size(const value_t&) { return sizeof(value_t); }
  /// No record under the key: nothing to read, and the new bytes are it.
  inline void RmwInitial(value_t& value) {
    value.gen_.store(0, std::memory_order_relaxed);
    std::memcpy(value.d_, v_, kValue);
    *found_ = 0;
  }
  /// The record is in the read-only region, where nobody writes it: read
  /// there, and written as a new record at the tail.
  inline void RmwCopy(const value_t& old_value, value_t& value) {
    std::memcpy(out_, old_value.d_, kValue);
    value.gen_.store(0, std::memory_order_relaxed);
    std::memcpy(value.d_, v_, kValue);
    *found_ = 1;
  }
  /// In the mutable region: read and replaced in place, under the
  /// generation's lock as `PutAtomic` takes it.
  inline bool RmwAtomic(value_t& value) {
    uint64_t g = value.gen_.load(std::memory_order_relaxed);
    do {
      if (g & 1) {
        g = value.gen_.load(std::memory_order_relaxed);
        continue;
      }
      if (value.gen_.compare_exchange_weak(g, g + 1, std::memory_order_acquire)) break;
    } while (true);
    std::memcpy(out_, value.d_, kValue);
    std::memcpy(value.d_, v_, kValue);
    value.gen_.store(g + 2, std::memory_order_release);
    *found_ = 1;
    return true;
  }

 protected:
  Status DeepCopy_Internal(IAsyncContext*& context_copy) {
    return IAsyncContext::DeepCopy_Internal(*this, context_copy);
  }

 private:
  Key key_;
  uint8_t v_[kValue];
  uint8_t* out_;
  int* found_;
};

typedef FasterKv<Key, Value, disk_t> store_t;

static constexpr uint64_t kRefreshInterval = 64;
static constexpr uint64_t kCompletePendingInterval = 1600;
static thread_local uint64_t tl_ops = 0;
static thread_local uint64_t tl_serial = 0;

static inline void tick(store_t* s) {
  ++tl_ops;
  if (tl_ops % kRefreshInterval == 0) {
    s->Refresh();
    if (tl_ops % kCompletePendingInterval == 0) s->CompletePending(false);
  }
}

static std::atomic<int> g_index_done{ 0 };
static std::atomic<int> g_log_done{ 0 };
static void index_cb(Status result) { g_index_done.store(result == Status::Ok ? 1 : -1); }
static void log_cb(Status result, uint64_t) { g_log_done.store(result == Status::Ok ? 1 : -1); }

extern "C" {

/// A store at `dir` (created if it is not there), with `table_size` hash
/// buckets (a power of two), `log_bytes` of the hybrid log in memory (a
/// multiple of the log's page size) and `mutable_fraction` of those pages
/// updated in place, the rest read-only and copied to the tail on update.
/// Null on failure.
void* fkv_open(const char* dir, uint64_t table_size, uint64_t log_bytes,
               double mutable_fraction) {
  try {
    std::string path{ dir };
    if (path.empty() || path.back() != '/') path.push_back('/');
    return new store_t{ table_size, log_bytes, path, mutable_fraction };
  } catch (...) {
    return nullptr;
  }
}

void fkv_close(void* h) { delete static_cast<store_t*>(h); }

void fkv_start_session(void* h) { static_cast<store_t*>(h)->StartSession(); }

void fkv_stop_session(void* h) {
  auto* s = static_cast<store_t*>(h);
  s->CompletePending(true);
  s->StopSession();
}

/// 0, or -1 if FASTER refused the write.
int fkv_upsert(void* h, const uint8_t* key, const uint8_t* value) {
  auto* s = static_cast<store_t*>(h);
  UpsertContext ctx{ key, value };
  auto cb = [](IAsyncContext* ctxt, Status result) {
    CallbackContext<UpsertContext> context{ ctxt };
  };
  Status r = s->Upsert(ctx, cb, ++tl_serial);
  if (r == Status::Pending) s->CompletePending(true);
  tick(s);
  return (r == Status::Ok || r == Status::Pending) ? 0 : -1;
}

/// The value's bytes into `out` (kValue of them): 1, 0 if the key is not
/// there, -1 on an error.
int fkv_read(void* h, const uint8_t* key, uint8_t* out) {
  auto* s = static_cast<store_t*>(h);
  ReadContext ctx{ key, out };
  static thread_local int pending_found;
  pending_found = 0;
  auto cb = [](IAsyncContext* ctxt, Status result) {
    CallbackContext<ReadContext> context{ ctxt };
    pending_found = (result == Status::Ok) ? 1 : (result == Status::NotFound ? 0 : -1);
  };
  Status r = s->Read(ctx, cb, ++tl_serial);
  int got;
  if (r == Status::Ok) {
    got = 1;
  } else if (r == Status::NotFound) {
    got = 0;
  } else if (r == Status::Pending) {
    s->CompletePending(true);
    got = pending_found;
  } else {
    got = -1;
  }
  tick(s);
  return got;
}

/// Read-modify-write: the value as it stood into `out` (kValue bytes) and
/// `value` in its place, as one operation of FASTER's. 1, 0 if the key was
/// not there (it is now, holding `value`), -1 on an error.
int fkv_rmw(void* h, const uint8_t* key, const uint8_t* value, uint8_t* out) {
  auto* s = static_cast<store_t*>(h);
  static thread_local int found;
  found = -1;
  RmwContext ctx{ key, value, out, &found };
  auto cb = [](IAsyncContext* ctxt, Status result) {
    CallbackContext<RmwContext> context{ ctxt };
  };
  Status r = s->Rmw(ctx, cb, ++tl_serial);
  if (r == Status::Pending) s->CompletePending(true);
  tick(s);
  return (r == Status::Ok || r == Status::Pending) ? found : -1;
}

/// An index checkpoint, waited for: once, on the store as opened.
int fkv_checkpoint_index(void* h) {
  auto* s = static_cast<store_t*>(h);
  g_index_done.store(0);
  Guid token;
  if (!s->CheckpointIndex(index_cb, token)) return -1;
  while (g_index_done.load() == 0) {
    s->CompletePending(false);
    s->Refresh();
  }
  s->CompletePending(true);
  return g_index_done.load() == 1 ? 0 : -1;
}

/// A hybrid-log checkpoint, waited for: 0 once it is on the device, -1 if
/// FASTER would not start one or it failed.
int fkv_checkpoint(void* h) {
  auto* s = static_cast<store_t*>(h);
  g_log_done.store(0);
  Guid token;
  if (!s->CheckpointHybridLog(log_cb, token)) return -1;
  while (g_log_done.load() == 0) {
    s->CompletePending(false);
    s->Refresh();
  }
  s->CompletePending(true);
  return g_log_done.load() == 1 ? 0 : -1;
}

}  // extern "C"
