//! Supdb -- a read-optimized store for append-heavy, deep-key workloads.
//!
//! The four findings below are the *design document's*, taken against Uppend,
//! RocksDB, LMDB and MapDB, and they are kept because they explain why the
//! code is shaped as it is. Two have since been overtaken by the project's
//! own measurements, which is why the line above no longer says
//! ingest-optimized:
//!
//! - Finding 1's "nothing here may compromise the ingest path" did not hold:
//!   the durable ordered load is behind both LMDB and tuned RocksDB
//!   (`EXT.22`, `EXT.32`). What the engine wins is reads against that same
//!   pair (`EXT.23`, `EXT.33`).
//! - Finding 4's choice between size and warm reads was made, and it was made
//!   for reads: blocks are stored uncompressed by default, so a point read
//!   decompresses nothing and the file is the larger for it. Compression is
//!   per segment rather than global, and `W6.8` prices it on a real index.
//!
//! Those claim ids name the previous suite's claims, in the supdb-bench
//! repository's history; the live measurements are `bench/`. The four as
//! stated:
//!
//! 1. Append throughput and flush cost are what an append-only log actually
//!    wins (2-2.6x and 17-39x respectively), and they survived every
//!    correction made to the benchmark. Nothing here may compromise the
//!    ingest path.
//! 2. Compressing one key's values alone accomplishes nothing when a key
//!    holds only a kilobyte: 960 MB of data became 1,245 MB stored, versus
//!    1,242 MB with compression switched off. A compressor needs a window, so
//!    extents from many keys are packed into a shared block and compressed
//!    together.
//! 3. The position catalog cost 402 bytes per key to record ten 30-bit file
//!    offsets -- 107x the information-theoretic floor. Index compactness is a
//!    first-class concern, not an afterthought.
//! 4. A store that compresses has to choose between size and warm reads
//!    unless it caches *decompressed* blocks. RocksDB gets both for exactly
//!    this reason; Uppend has no cache layer and so must pick one.
//!
//! Sealing is also where the data is sorted by key, because the batch is
//! already in hand and being copied. That produces sorted runs, which is what
//! ordered scans will need later, at close to no cost.

// The format modules below came from the design artifact rather than being
// written here. `block` and `index` still have their style lints scoped off
// rather than paid down, which is all that is left of the distinction: they
// are formatted by the same gate as every other file, and everything else
// holds to -D warnings.
/// The on-disk format's fixed quantities: the superblock magic and geometry.
/// Not owned by any writer -- two of them exist and three readers parse what
/// either produced.
mod format;

#[allow(clippy::all, dead_code)]
mod block;
#[allow(clippy::all, dead_code)]
/// The extent types the index is built from. Public because a writer needs
/// to name what it is writing; the format is otherwise reached through
/// `Blob`.
pub mod index;
// Not vendored -- written here, so it holds to -D warnings like the harness.
// On wasm its writer half (`plan`, `encode`, the slack and fence arithmetic)
// has no caller, because there is no writer there. Allowed on that target
// only, so the native build keeps telling the truth about dead code.
/// The read path, over any `Bytes` source. Compiles on every target.
pub mod blob;
/// Where a reader's bytes come from. The seam the wasm build needed.
pub mod bytes;
/// The engine: a WAL, a memtable and threads that seal and merge -- none of
/// which a browser has, and all of which want files to write and map.
/// Excluded from the wasm build rather than stubbed; `blob` is the read path
/// that carries over.
#[cfg(not(target_family = "wasm"))]
pub mod db;
#[cfg_attr(target_family = "wasm", allow(dead_code))]
/// The flat key index, including the builders a segment writer drives.
/// Public for the same reason as `index`: a bulk writer for sorted,
/// write-once input is a legitimate second producer of this format, and
/// f46 prices one. The lint allowance is the only concession to making the
/// module public -- its `len()` methods predate the exposure.
#[allow(clippy::len_without_is_empty)]
pub mod flatindex;
/// The C ABI the browser calls. Hand-written rather than generated, because
/// the whole point of R3.3 is the size of what ships.
#[cfg(target_family = "wasm")]
pub mod wasmapi;

pub use blob::{Blob, BlobOptions, SparseBlob};
#[cfg(not(target_family = "wasm"))]
pub use bytes::MmapBytes;
pub use bytes::{Bytes, SliceBytes, VecBytes};
/// The engine's surface, at the crate root: `supdb::Db` rather than
/// `supdb::db::Db`. `Options` configures the engine and `SegmentOptions` the
/// segments it writes, which is the way round a caller meets them -- the
/// names were the other way round while a second engine owned the shorter
/// one.
#[cfg(not(target_family = "wasm"))]
pub use db::{
    BackgroundIo, Db, Options, ReadAdvice, SegmentOptions, SegmentWriter, SyncPolicy, Txn,
};
