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
mod flatindex;
#[allow(clippy::all, dead_code)]
pub mod keytable;
#[allow(clippy::all, dead_code)]
mod readers;
#[allow(clippy::all, dead_code)]
mod store;

pub use store::{Options, ReadOptions, Reader, Reclaim, Stats, Store, Sync};
