//! Supdb -- an ingest-optimized store for append-heavy, deep-key workloads.
//!
//! The shape here is driven by measurements taken against Uppend, RocksDB,
//! LMDB and MapDB rather than by taste. Four findings set the design:
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

// The measurement substrate reads `getrusage`, `clock_gettime` and `sysconf`,
// none of which exist on wasm, and it has no business in a browser bundle
// anyway. Eleven of the twenty-nine errors a wasm build used to produce were
// this module alone.
#[cfg(not(target_family = "wasm"))]
pub mod bench;

// The engine modules below are vendored from the design artifact verbatim, so
// that the architecture review's line-level references stay valid and so a
// reader can compare what was measured against what was described. Style lints
// are scoped off here rather than fixed in place; the harness code above and
// in src/bin holds to -D warnings.
#[allow(clippy::all, dead_code)]
mod block;
#[allow(clippy::all, dead_code)]
mod freelist;
#[allow(clippy::all, dead_code)]
mod index;
// Not vendored -- written here, so it holds to -D warnings like the harness.
// On wasm its writer half (`plan`, `encode`, the slack and fence arithmetic)
// has no caller, because there is no writer there. Allowed on that target
// only, so the native build keeps telling the truth about dead code.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
mod flatindex;
#[allow(clippy::all, dead_code)]
pub mod keytable;
// The reader table is shared with a *writer*, and there is no writer on wasm.
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::all, dead_code)]
mod readers;
// `store` is the write path and it maps files. Seventeen of the twenty-nine
// wasm errors were here -- `write_all_at`, `read_exact_at`, `memmap2::Advice`
// -- and none of them is worth porting, because a browser has no file to write
// and no file to map. It is excluded rather than ported, and `blob` is the
// read path that survives the exclusion.
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::all, dead_code)]
mod store;

/// Where a reader's bytes come from. The seam the wasm build needed.
pub mod bytes;
/// The read path, over any `Bytes` source. Compiles on every target.
pub mod blob;
/// The C ABI the browser calls. Hand-written rather than generated, because
/// the whole point of R3.3 is the size of what ships.
#[cfg(target_family = "wasm")]
pub mod wasmapi;

pub use blob::{Blob, BlobOptions};
pub use bytes::{Bytes, SliceBytes, VecBytes};
#[cfg(not(target_family = "wasm"))]
pub use bytes::MmapBytes;
#[cfg(not(target_family = "wasm"))]
pub use store::{Options, ReadOptions, Readahead, Reader, Reclaim, Stats, Store, Sync};
