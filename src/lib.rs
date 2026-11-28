//! Supdb -- a read-optimized store for append-heavy, deep-key workloads.
//!
//! The four findings below are the *design document's*, taken against Uppend,
//! RocksDB, LMDB and MapDB, and they are kept because they explain why the
//! code is shaped as it is. Two have since been overtaken by the project's
//! own measurements, which is why the line above no longer says
//! ingest-optimized:
//!
//! - Finding 1's "nothing here may compromise the ingest path" did not survive
//!   measurement: the durable ordered load is behind both LMDB and tuned
//!   RocksDB. What the engine wins is reads against that same pair.
//! - Finding 4's choice between size and warm reads was made, and it was made
//!   for reads: blocks are stored uncompressed by default, so a point read
//!   decompresses nothing and the file is the larger for it. Compression is
//!   per segment rather than global, and is measured on a real index.
//!
//! The live measurements are in `bench/`. The four as stated:
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
//!
//! ## What the read path does not hand out
//!
//! A reader asks its byte source for the bytes it is about to read
//! (`Bytes::slice_at`, `Bytes::read_at`), which every kind of source
//! answers. The read path's own borrows of a section whole -- a section
//! lent on the bounds check alone, which a source that lends by the piece
//! refuses, and the record regions as slices -- and the address behind a
//! paged mapping are not reachable from outside the crate. The examples
//! that do not compile come in groups, and each group is followed, or
//! preceded where it continues the one before, by an example that does:
//! the call a caller outside the crate makes instead.
//!
//! A mapping is not lent as it lies by a call of its own:
//!
//! ```compile_fail
//! fn whole(p: &std::path::Path) -> std::io::Result<usize> {
//!     Ok(supdb::MmapBytes::open(p)?.as_slice().len())
//! }
//! ```
//! ```compile_fail
//! fn whole(p: &std::path::Path) -> std::io::Result<usize> {
//!     Ok(supdb::MmapBytes::open(p)?.lend_whole_map().len())
//! }
//! ```
//! ```no_run
//! fn whole(p: &std::path::Path) -> std::io::Result<usize> {
//!     Ok(supdb::Bytes::len(&supdb::MmapBytes::open(p)?) as usize)
//! }
//! ```
//!
//! A section lent on the bounds check alone, which answers nothing for a
//! source that lends by the piece, is the read path's own; it is not called
//! from outside the crate, and a caller asks instead:
//!
//! ```compile_fail
//! fn section<B: supdb::Bytes>(b: &B) -> Option<&[u8]> {
//!     b.lend_at(0, 1)
//! }
//! ```
//! ```no_run
//! fn section<B: supdb::Bytes>(b: &B) -> Option<&[u8]> {
//!     b.slice_at(0, 1)
//! }
//! ```
//!
//! nor with the token it takes, which only the read path makes, whether it
//! is named or inferred (a call whose argument diverges compiles, and never
//! reaches the method; one made in unsafe code is outside what a type can
//! refuse):
//!
//! ```compile_fail
//! fn section<B: supdb::Bytes>(b: &B) -> Option<&[u8]> {
//!     b.lend_at(0, 1, supdb::blob::lend_seal::Token::new())
//! }
//! ```
//! ```compile_fail
//! fn section<B: supdb::Bytes>(b: &B) -> Option<&[u8]> {
//!     b.lend_at(0, 1, Default::default())
//! }
//! ```
//!
//! and a source outside the crate keeps the default lend, since it cannot
//! write the method's signature:
//!
//! ```compile_fail
//! struct Mine(Vec<u8>);
//! impl supdb::Bytes for Mine {
//!     fn len(&self) -> u64 {
//!         self.0.len() as u64
//!     }
//!     fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
//!         dst.copy_from_slice(&self.0[off as usize..off as usize + dst.len()]);
//!         Ok(())
//!     }
//!     fn lend_at(&self, off: u64, len: usize, _: supdb::blob::lend_seal::Token) -> Option<&[u8]> {
//!         self.0.get(off as usize..off as usize + len)
//!     }
//! }
//! ```
//! ```compile_fail
//! struct Mine(Vec<u8>);
//! impl supdb::Bytes for Mine {
//!     fn len(&self) -> u64 {
//!         self.0.len() as u64
//!     }
//!     fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
//!         dst.copy_from_slice(&self.0[off as usize..off as usize + dst.len()]);
//!         Ok(())
//!     }
//!     fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {
//!         self.0.get(off as usize..off as usize + len)
//!     }
//! }
//! ```
//! ```
//! struct Mine(Vec<u8>);
//! impl supdb::Bytes for Mine {
//!     fn len(&self) -> u64 {
//!         self.0.len() as u64
//!     }
//!     fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
//!         dst.copy_from_slice(&self.0[off as usize..off as usize + dst.len()]);
//!         Ok(())
//!     }
//!     fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
//!         self.0.get(off as usize..off as usize + len)
//!     }
//! }
//! ```
//!
//! What stands behind a paged mapping is the engine's to implement:
//!
//! ```compile_fail
//! fn named<T: supdb::bytes::Paging>(_: &T) {}
//! ```
//! ```
//! fn named<T: supdb::bytes::Bytes>(_: &T) {}
//! ```
//!
//! A segment's record regions are not handed out as slices; where they lie
//! is:
//!
//! ```compile_fail
//! fn regions(b: &supdb::Blob<supdb::MmapBytes>) -> usize {
//!     b.record_regions().map_or(0, |(r, _)| r.len())
//! }
//! ```
//! ```compile_fail
//! fn regions(b: &supdb::Blob<supdb::MmapBytes>) -> usize {
//!     usize::from(b.record_pieces().is_some())
//! }
//! ```
//! ```
//! fn regions(b: &supdb::Blob<supdb::MmapBytes>) -> usize {
//!     b.record_region_place().map_or(0, |(_, n)| n)
//! }
//! ```

// The format modules below came from the design artifact rather than being
// written here; every one holds to -D warnings now, `block` and `flatindex`
// with `dead_code` allowed on the wasm target alone, where their writer
// halves have no caller.
/// The on-disk format's fixed quantities: the superblock magic and geometry.
/// Not owned by any writer -- two of them exist and three readers parse what
/// either produced.
mod format;

/// Blocks: the unit a segment's values are packed and checksummed in. On
/// wasm its writer half (`BlockBuilder`, `compress`, the chunk writer and
/// the checksum row) has no caller, because `db` is not compiled there.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
mod block;
/// The extent types the index is built from. Public because a writer needs
/// to name what it is writing; the format is otherwise reached through
/// `Blob`.
pub mod index;
// On wasm its writer half (`plan`, `encode`, the slack and fence arithmetic)
// has no caller, because there is no writer there. Allowed on that target
// only, so the native build keeps telling the truth about dead code.
/// The read path, over any `Bytes` source. Compiles on every target.
pub mod blob;
/// Where a reader's bytes come from. The seam the wasm build needed.
pub mod bytes;
/// An encrypted object read as its plaintext, through the same seam: the
/// envelope, and the hook a caller's cipher fits.
pub mod crypt;
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
/// one is priced against the general path. The lint allowance is the only
/// concession to making the module public -- its `len()` methods predate
/// the exposure.
#[allow(clippy::len_without_is_empty)]
pub mod flatindex;
/// The ordered index a scan seeks through: one companion file per segment,
/// native only. The browser reader does not have it and does not need it --
/// its dictionary walks go through `Blob` as they always did.
#[cfg(not(target_family = "wasm"))]
pub mod ordindex;
pub mod reserve;
/// The C ABI the browser calls. Hand-written rather than generated, because
/// the size of what ships is budgeted.
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
pub use db::dirkey::{self, DirKey, DirState};
/// A live store under a cipher the caller supplies (`Encryption::Cipher`): the
/// cipher as the store takes it, what it counts, the name a store's marker
/// gives its key, and the store handed from one of the caller's keys to
/// another.
#[cfg(not(target_family = "wasm"))]
pub use db::vault::{
    key_id as cipher_key_id, rewrap as cipher_rewrap, CipherCounts, PagerCounts, StoreCipher,
};
/// A test's counts of a store kept in one file (`Options::one_file`).
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub use db::OneCounts;
#[cfg(not(target_family = "wasm"))]
pub use db::{
    BackgroundIo, Db, Encryption, HelperFault, Isolation, Options, ReadAdvice, Reader,
    SegmentOptions, SegmentWrite, SegmentWriter, SyncPolicy, Txn, Upkeep, ZInject, ZPages, ZStride,
    MAX_KEY_LEN, MAX_VALUE_LEN, MERGE_AHEAD_WINDOW, STORE_FORMAT, STORE_FORMAT_OLDEST,
};
