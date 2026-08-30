//! A read-only view of a sealed supdb file, over any `Bytes` source.
//!
//! `store::Reader` is the reader for a file this process can map. This is the
//! reader for a file it cannot: an immutable object fetched out of S3 into a
//! browser, addressed through a synchronous byte source. It is the same
//! format, the same `flatindex`, the same `block` decoder -- what changes is
//! only where the bytes come from, which is the whole of the seam the wasm
//! build needed.
//!
//! It compiles on every target, and on native it runs over a mapping and
//! borrows exactly what `store::Reader` borrows. That is deliberate: the arm
//! the browser runs is the arm the native test suite exercises, so the two
//! cannot drift into disagreement without `tests/blob.rs` saying so. That test
//! walks a real store with both readers and requires every key, every value
//! and every count to match.
//!
//! ## What it does that `store::Reader` does not
//!
//! `count(key)` -- the number of values under a key, without materialising
//! any of them. See the note on `count` for what that does and does not cost:
//! it is O(values), not O(extents), and the format is the reason.
//!
//! ## What it deliberately does not do
//!
//! No writing, no checkpointing, no reader table, no time travel. A sealed
//! day index has one generation and nobody is mutating it; the slot table
//! exists to stop a *writer* reusing space under a reader, and there is no
//! writer. Leaving it out is what keeps the wasm build to a reader.

use crate::block::{self, BlockLoc};
use crate::bytes::{short, take, Bytes};
use crate::flatindex::{self, FlatIndex, MappedBlocks};
use crate::index::{get_uvarint, Ext};
use std::cell::{Cell, RefCell};
use std::io::{Error, ErrorKind, Result};

fn corrupt(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, format!("supdb: {msg}"))
}

/// Bytes `put_uvarint` spends on `v`. The mirror of `index::put_uvarint`, and
/// the reason `count_fixed` can do its arithmetic without touching a block.
fn varint_len(mut v: u64) -> u64 {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// The record count of a run of fixed-width values, from the extent list
/// alone. `None` when the extents are not consistent with that width.
///
/// Two independent checks, and the second is what makes this usable. The
/// first is that the run is a whole number of strides -- necessary, and on
/// its own badly insufficient: a fixture of variable-length `k:i` strings
/// produced a run of 17 values whose bytes divided exactly by a stride of 4
/// and this returned 23. `tests/blob.rs` carries that case.
///
/// The second is `Ext::last`, the offset of the final record in the run,
/// which the format already stores so that reading the newest value is O(1).
/// For `n` records of one stride it must be exactly `(n - 1) * stride`. That
/// pins the arithmetic against a second quantity the writer recorded
/// independently, and it rejects the 17-value case: the 17th record starts at
/// 87 where 23 records would put the last at 88.
///
/// Still not a proof -- a run could be crafted to satisfy both -- so the
/// contract remains that the caller knows its own schema. It is the
/// difference between a check that catches an honest mistake and one that
/// does not.
fn fixed_count(exts: &[Ext], stride: u64) -> Option<u64> {
    if stride == 0 {
        return None;
    }
    let mut total = 0u64;
    for e in exts {
        let len = e.len as u64;
        if len == 0 || !len.is_multiple_of(stride) {
            return None;
        }
        let n = len / stride;
        if e.last as u64 != (n - 1) * stride {
            return None;
        }
        total += n;
    }
    Some(total)
}

/// Superblock magic and geometry.
///
/// Duplicated from `store.rs` rather than shared, because `store.rs` does not
/// compile for wasm -- it is the write path and it maps files -- and moving
/// the type out of it would renumber a file whose line numbers the
/// architecture review cites. The duplication is held honest by
/// `tests/blob.rs`, which opens a store written by `store.rs` and fails if
/// this decoder disagrees with it about anything.
const MAGIC: u64 = 0x5355_5044_4200_0001;
const SUPER: u64 = 4096;
const SLOT: u64 = 512;
const SB_BYTES: usize = 120;
const SB_FIELDS: usize = 13;

#[derive(Clone, Copy, Debug)]
struct Super {
    generation: u64,
    timestamp: u64,
    key_off: u64,
    key_stored: u64,
    key_uncompressed: u64,
    blk_off: u64,
    blk_stored: u64,
    blk_uncompressed: u64,
    high_water: u64,
}

impl Super {
    fn decode(buf: &[u8]) -> Option<Super> {
        if buf.len() < SB_BYTES {
            return None;
        }
        let f: Vec<u64> = (0..SB_FIELDS)
            .map(|i| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap()))
            .collect();
        if u64::from_le_bytes(buf[104..112].try_into().unwrap()) != MAGIC {
            return None;
        }
        // FNV-1a over the fields and the magic, exactly as the writer computes
        // it. Enough to reject a torn or never-written slot.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in f.iter().chain(std::iter::once(&MAGIC)) {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        }
        if u64::from_le_bytes(buf[112..120].try_into().unwrap()) != h {
            return None;
        }
        Some(Super {
            generation: f[0],
            timestamp: f[2],
            key_off: f[3],
            key_stored: f[4],
            key_uncompressed: f[5],
            blk_off: f[6],
            blk_stored: f[7],
            blk_uncompressed: f[8],
            high_water: f[12],
        })
    }
}

/// A section of the file: lent by the source, or copied out of it once.
///
/// Copied *once*, at open, and never per lookup. A source that cannot lend --
/// an OPFS file handle -- therefore pays for the key index and the block table
/// exactly one time each, which is the cost model a browser wants anyway,
/// since it has already downloaded the whole object.
enum Sec {
    Lent { off: u64, len: usize },
    Owned(Vec<u8>),
}

impl Sec {
    fn read<B: Bytes>(src: &B, off: u64, len: usize) -> Result<Sec> {
        if src.slice_at(off, len).is_some() {
            return Ok(Sec::Lent { off, len });
        }
        let mut v = vec![0u8; len];
        src.read_at(off, &mut v)?;
        Ok(Sec::Owned(v))
    }

    fn get<'a, B: Bytes>(&'a self, src: &'a B) -> Result<&'a [u8]> {
        match self {
            Sec::Lent { off, len } => src
                .slice_at(*off, *len)
                .ok_or_else(|| short(*off, *len, src.len())),
            Sec::Owned(v) => Ok(v),
        }
    }

    fn len(&self) -> usize {
        match self {
            Sec::Lent { len, .. } => *len,
            Sec::Owned(v) => v.len(),
        }
    }
}

/// How much to check on the way out.
#[derive(Clone, Copy, Debug)]
pub struct BlobOptions {
    /// Verify block checksums. On by default, matching `ReadOptions`: a
    /// browser reading an object off a CDN has more reason to check than a
    /// process reading its own disk, not less.
    pub verify_checksums: bool,
}

impl Default for BlobOptions {
    fn default() -> Self {
        BlobOptions {
            verify_checksums: true,
        }
    }
}

pub struct Blob<B: Bytes> {
    src: B,
    key: Sec,
    blk: Sec,
    idx: FlatIndex,
    blocks: MappedBlocks,
    generation: u64,
    timestamp: u64,
    opts: BlobOptions,
    /// One bit per (block, chunk), so a chunk is checksummed once per reader
    /// and not once per value. Same argument as `Reader::verified`: an
    /// uncompressed block is handed out where it lies, so re-verifying it per
    /// read is O(block) work to return O(value) bytes.
    verified: RefCell<Vec<u64>>,
    /// Reused buffers for a source that cannot lend, and for decompression.
    ///
    /// `Cell` rather than `RefCell` on purpose: these are held across a
    /// user callback, and a callback that re-enters the reader should get a
    /// fresh buffer rather than a panic. A host callback here is JavaScript.
    raw_buf: Cell<Vec<u8>>,
    dec_buf: Cell<Vec<u8>>,
}

impl<B: Bytes> Blob<B> {
    pub fn open(src: B) -> Result<Blob<B>> {
        Blob::open_with(src, BlobOptions::default())
    }

    pub fn open_with(src: B, opts: BlobOptions) -> Result<Blob<B>> {
        // The zero-copy read path reinterprets an extent array as `&[Ext]`,
        // which is native-endian, while every scalar in the file is written
        // little-endian. On a big-endian target those disagree and the reader
        // would misread a valid file rather than refuse it. Refused here
        // instead. Every browser is little-endian and so is every machine in
        // `results/`, so nothing is given up by saying so out loud.
        if cfg!(target_endian = "big") {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "supdb's index is little-endian on the wire and native-endian where it is \
                 addressed in place; those agree only on a little-endian machine, so this file \
                 is refused here rather than misread",
            ));
        }
        if src.len() < SUPER {
            return Err(corrupt("file too short to hold a superblock"));
        }
        let mut head = [0u8; (SLOT as usize) + SB_BYTES];
        src.read_at(0, &mut head)?;
        let a = Super::decode(&head[0..SB_BYTES]);
        let b = Super::decode(&head[SLOT as usize..SLOT as usize + SB_BYTES]);
        // Both slots are read and the valid one with the higher generation
        // wins, so a file whose last checkpoint was interrupted opens at the
        // previous complete state rather than failing.
        let sb = match (a, b) {
            (Some(x), Some(y)) => {
                if x.generation >= y.generation {
                    x
                } else {
                    y
                }
            }
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => return Err(corrupt("no valid supdb checkpoint")),
        };
        if sb.high_water > src.len() {
            return Err(corrupt(
                "the superblock describes more bytes than the object holds: it is truncated",
            ));
        }
        // A section that was compressed cannot be addressed where it lies, and
        // this reader has no reason to support the varint formats -- they are
        // what `flat_index` replaced, and a logshed day index is written by a
        // current writer with the current defaults.
        if sb.key_stored != sb.key_uncompressed || sb.blk_stored != sb.blk_uncompressed {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this file's index sections are compressed, so they cannot be read where they \
                 lie. Write it with the default options, which store both sections verbatim",
            ));
        }
        let key = Sec::read(&src, sb.key_off, sb.key_stored as usize)?;
        let blk = Sec::read(&src, sb.blk_off, sb.blk_stored as usize)?;
        let idx = FlatIndex::parse(key.get(&src)?).ok_or_else(|| {
            corrupt(
                "the key index is not the flat format this reader understands (an older varint \
                 index, or damage in its header)",
            )
        })?;
        let blocks = MappedBlocks::parse(blk.get(&src)?)
            .ok_or_else(|| corrupt("the block table is not readable"))?;
        let verified = vec![0u64; (blocks.len() * block::MAX_CHUNK_CRCS).div_ceil(64)];
        Ok(Blob {
            src,
            key,
            blk,
            // The generation of the index *section*, not of the superblock.
            // They are not always the same number -- a publish can move the
            // superblock on without writing a new index -- and `Reader`
            // reports the section's, because that is the state a reader is
            // actually looking at. `tests/blob.rs` caught this by requiring
            // the two readers to agree, which is what it is for.
            generation: idx.generation,
            idx,
            blocks,
            timestamp: sb.timestamp,
            opts,
            verified: RefCell::new(verified),
            raw_buf: Cell::new(Vec::new()),
            dec_buf: Cell::new(Vec::new()),
        })
    }

    // ------------------------------------------------------------ diagnostics --

    /// Number of distinct keys. R4.5.
    pub fn keys(&self) -> usize {
        self.idx.len()
    }

    /// Bytes of key index this reader addresses. R4.5.
    pub fn index_bytes(&self) -> usize {
        self.key.len()
    }

    /// Bytes of block table this reader addresses.
    pub fn block_table_bytes(&self) -> usize {
        self.blk.len()
    }

    pub fn blocks(&self) -> usize {
        self.blocks.len()
    }

    /// (generation, milliseconds) of the checkpoint this reader opened.
    pub fn version(&self) -> (u64, u64) {
        (self.generation, self.timestamp)
    }

    /// True when the index section is borrowed rather than copied.
    ///
    /// Diagnostic, and the thing `tests/blob.rs` asserts to keep R2.3 from
    /// rotting: a native reader that started copying its index would still
    /// pass every correctness test.
    pub fn zero_copy(&self) -> bool {
        matches!(self.key, Sec::Lent { .. })
    }

    // ------------------------------------------------------------ lookup --

    fn key_sec(&self) -> Result<&[u8]> {
        self.key.get(&self.src)
    }

    fn blk_sec(&self) -> Result<&[u8]> {
        self.blk.get(&self.src)
    }

    /// The extents of a key, borrowed out of the index. No allocation, no
    /// decode -- this is the borrow `flatindex` exists to make possible, and
    /// it survives the byte-source abstraction on any source that can lend.
    pub fn lookup(&self, key: &[u8]) -> Option<&[Ext]> {
        self.idx
            .lookup(self.key_sec().ok()?, key, flatindex::key_hash)
    }

    /// Rank of the first key at or after `key`, in key order. R4.4.
    pub fn seek(&self, key: &[u8]) -> usize {
        match self.key_sec() {
            Ok(sec) => self.idx.seek_with(sec, key, true),
            Err(_) => self.idx.len(),
        }
    }

    /// The key at `rank` in key order.
    pub fn key_at(&self, rank: usize) -> Option<&[u8]> {
        self.idx.at(self.key_sec().ok()?, rank).map(|(k, _)| k)
    }

    fn exts_at(&self, rank: usize) -> Option<(&[u8], &[Ext])> {
        self.idx.at(self.key_sec().ok()?, rank)
    }

    // ------------------------------------------------------------ blocks --

    fn loc_of(&self, id: u32) -> Result<BlockLoc> {
        // Checked against the table on every access rather than validated once
        // at open: validating eagerly is the O(key count) open this format
        // exists to remove, and `Reader::loc_of` carries the same note for the
        // same reason -- a `scan` that indexed the block table with a number
        // straight out of a damaged file panicked the calling process.
        self.blk_sec()
            .ok()
            .and_then(|sec| self.blocks.get(sec, id as usize))
            .ok_or_else(|| {
                corrupt(&format!(
                    "an extent names block {id} but the table has {}",
                    self.blocks.len()
                ))
            })
    }

    /// Has chunk `j` of block `i` already been checksummed by this reader?
    fn mark_verified(&self, i: u32, j: usize) -> bool {
        let slot = i as usize * block::MAX_CHUNK_CRCS + j;
        let (w, bit) = (slot / 64, 1u64 << (slot % 64));
        let mut v = self.verified.borrow_mut();
        match v.get_mut(w) {
            Some(cell) => {
                let seen = *cell & bit != 0;
                *cell |= bit;
                seen
            }
            // No room to remember: check every time rather than skip.
            None => false,
        }
    }

    /// Verify only the chunks `lo..hi` of a block actually touches.
    ///
    /// Whole-block verification is the fallback, never skipping: a block with
    /// no per-chunk checksums, a table too short to hold the row, or a range
    /// outside the block all fall back to hashing the block.
    fn verify(&self, id: u32, loc: BlockLoc, raw: &[u8], lo: usize, hi: usize) -> Result<()> {
        if !self.opts.verify_checksums || !block::checksums_on() {
            return Ok(());
        }
        let whole = |this: &Self| -> Result<()> {
            if this.mark_verified(id, 0) {
                return Ok(());
            }
            if block::crc32(raw) != loc.crc {
                return Err(corrupt("block checksum mismatch"));
            }
            Ok(())
        };
        if !loc.chunk_crc || hi > raw.len() || lo >= hi {
            return whole(self);
        }
        let sec = match self.blk_sec() {
            Ok(s) => s,
            Err(_) => return whole(self),
        };
        for j in (lo / block::CHUNK)..=((hi - 1) / block::CHUNK) {
            let a = j * block::CHUNK;
            let b = ((j + 1) * block::CHUNK).min(raw.len());
            let (Some(want), true) = (self.blocks.chunk_crc(sec, id as usize, j), a < b) else {
                return whole(self);
            };
            if self.mark_verified(id, j) {
                continue;
            }
            if block::crc32(&raw[a..b]) != want {
                return Err(corrupt("block checksum mismatch"));
            }
        }
        Ok(())
    }

    /// Hand `f` the bytes of one extent, however the block is stored.
    ///
    /// Three arms, matching `Reader::read_all`: a plain block is handed out
    /// where it lies, a chunked block has only the chunks the extent covers
    /// decompressed, and a block compressed as one stream is decompressed
    /// whole into scratch. The buffers are taken out of their `Cell`s and put
    /// back, so a callback that re-enters gets a fresh buffer instead of a
    /// panic.
    fn with_extent<R>(&self, e: Ext, f: impl FnOnce(&[u8]) -> Result<R>) -> Result<R> {
        let loc = self.loc_of(e.block)?;
        let (a, b) = (e.off as usize, (e.off as usize).saturating_add(e.len as usize));
        let mut raw_buf = self.raw_buf.take();
        let out = (|| -> Result<R> {
            let raw = take(&self.src, loc.off, loc.stored as usize, &mut raw_buf)?;
            if loc.is_plain() {
                if b > raw.len() {
                    return Err(corrupt("extent runs past its block"));
                }
                self.verify(e.block, loc, raw, a, b)?;
                return f(&raw[a..b]);
            }
            let un = loc.uncompressed as usize;
            if b > un {
                return Err(corrupt("extent runs past its block"));
            }
            let mut dec = self.dec_buf.take();
            let r = (|| -> Result<R> {
                if dec.len() < un {
                    dec.resize(un, 0);
                }
                if loc.chunked {
                    block::read_chunked_range(raw, un, a, b, &mut dec[..un])?;
                } else {
                    self.verify(e.block, loc, raw, 0, raw.len())?;
                    block::decompress_into(raw, &mut dec, un)?;
                }
                f(&dec[a..b])
            })();
            self.dec_buf.set(dec);
            r
        })();
        self.raw_buf.set(raw_buf);
        out
    }

    // ------------------------------------------------------------ the API --

    /// Visit every value of a key, in append order. Returns how many. R4.2.
    ///
    /// Note the return: the number of *values*. `store::Reader::read_all`
    /// returns the number of value *bytes*, which is a different quantity and
    /// has been read as a count more than once -- the requirements document
    /// this work was written against says "read_all returns a count", and it
    /// does not.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let Some(exts) = self.lookup(key) else {
            return Ok(0);
        };
        let mut n = 0u64;
        for e in exts {
            n += self.with_extent(*e, |run| {
                let mut p = 0usize;
                let mut seen = 0u64;
                while p < run.len() {
                    let len = get_uvarint(run, &mut p) as usize;
                    let end = p
                        .checked_add(len)
                        .ok_or_else(|| corrupt("record length overflows"))?;
                    if end > run.len() {
                        return Err(corrupt("record runs past the end of its extent"));
                    }
                    f(&run[p..end]);
                    seen += 1;
                    p = end;
                }
                Ok(seen)
            })?;
        }
        Ok(n)
    }

    /// How many values a key holds, without materialising any of them. R4.3.
    ///
    /// **This is O(values), not O(extents), and the format is why.** An `Ext`
    /// records where a run of values starts, how many bytes it is, and where
    /// its last record begins -- four `u32`s, none of which is a count. The
    /// values inside a run are length-prefixed varints laid end to end, so the
    /// only way to know how many there are is to step over them. There is no
    /// arithmetic that recovers the count from the extent list, and saying so
    /// is more useful than shipping something that quietly decodes.
    ///
    /// What it *does* avoid is everything after the length prefix: no value
    /// slice is bounds-checked into existence, nothing is handed to a
    /// callback, and across the wasm boundary -- which is where logshed calls
    /// this from -- no crossing happens per value at all. On a plain block it
    /// touches one byte per record and skips the payload, so it reads about
    /// one cache line per record's worth of stride rather than all of them.
    ///
    /// `f28-count` measures the three arms interleaved: this walk, the
    /// `read_all` it replaces, and `lookup` alone -- which is the cost an
    /// O(extents) count *would* have, and therefore the exact value of adding
    /// a per-extent count to the format. See `claims.json`, W2.
    pub fn count(&self, key: &[u8]) -> Result<u64> {
        let Some(exts) = self.lookup(key) else {
            return Ok(0);
        };
        let mut n = 0u64;
        for e in exts {
            n += self.with_extent(*e, |run| {
                let mut p = 0usize;
                let mut seen = 0u64;
                while p < run.len() {
                    let len = get_uvarint(run, &mut p) as usize;
                    let end = p
                        .checked_add(len)
                        .ok_or_else(|| corrupt("record length overflows"))?;
                    if end > run.len() {
                        return Err(corrupt("record runs past the end of its extent"));
                    }
                    seen += 1;
                    p = end;
                }
                Ok(seen)
            })?;
        }
        Ok(n)
    }

    /// Stored bytes under a key: value payload *plus* the varint length
    /// prefix in front of each value.
    ///
    /// This one is genuinely O(extents) -- `Ext::len` is the byte length of
    /// the run, so the sum touches no block at all, not even to fault a page
    /// in. It is the shape R4.3 asked for, on the quantity the format happens
    /// to record. For "is using the index cheaper than scanning the day", this
    /// is the better input anyway: it is how much work a `read_all` would be.
    ///
    /// It is not the sum of the value lengths. Each value costs its varint
    /// prefix too, and conflating the two is how `value_bytes` was wrong in
    /// the first version of this file -- one value of three bytes measured
    /// four. `tests/blob.rs` caught it.
    pub fn stored_bytes(&self, key: &[u8]) -> u64 {
        self.lookup(key)
            .map(|exts| exts.iter().map(|e| e.len as u64).sum())
            .unwrap_or(0)
    }

    /// The count in O(extents), for a key whose values are all `width` bytes.
    ///
    /// This is the escape hatch out of `count`'s O(values) walk, and it needs
    /// no format change: a fixed-width value carries a fixed-width length
    /// prefix, so a run of them is exactly `n * (width + varint_len(width))`
    /// bytes and the count falls straight out of the extent list.
    ///
    /// logshed's postings are four-byte line ordinals, so this is the call it
    /// should make. `f28-count` prices the difference.
    ///
    /// `None` when the stored bytes are not an exact multiple of the stride,
    /// which is what a key whose values are *not* all `width` bytes looks
    /// like. That check is necessary rather than sufficient -- a run of
    /// mixed widths can still sum to a multiple -- so a caller that is not
    /// sure of its own schema should verify it once against `count` rather
    /// than trust this. It is stated here because a silently wrong count is
    /// worse than a slow one.
    pub fn count_fixed(&self, key: &[u8], width: u32) -> Option<u64> {
        let stride = width as u64 + varint_len(width as u64);
        match self.lookup(key) {
            Some(exts) => fixed_count(exts, stride),
            // A key that is not there holds no values, which is a count.
            None => Some(0),
        }
    }

    /// Walk the dictionary in key order from `from`, with each key's value
    /// count. R4.4 -- this is what a "top paths" or "countries" panel needs.
    ///
    /// Returns how many keys were visited. Stops early when `f` returns false.
    pub fn scan_counts<F: FnMut(&[u8], u64) -> bool>(
        &self,
        from: &[u8],
        limit: usize,
        mut f: F,
    ) -> Result<usize> {
        let mut rank = self.seek(from);
        let mut seen = 0usize;
        while seen < limit {
            let Some((k, exts)) = self.exts_at(rank) else {
                break;
            };
            // Counted before the callback so the borrow of the index section
            // does not have to survive it.
            let mut n = 0u64;
            for e in exts {
                n += self.with_extent(*e, |run| {
                    let mut p = 0usize;
                    let mut c = 0u64;
                    while p < run.len() {
                        let len = get_uvarint(run, &mut p) as usize;
                        let end = p
                            .checked_add(len)
                            .ok_or_else(|| corrupt("record length overflows"))?;
                        if end > run.len() {
                            return Err(corrupt("record runs past the end of its extent"));
                        }
                        c += 1;
                        p = end;
                    }
                    Ok(c)
                })?;
            }
            seen += 1;
            rank += 1;
            if !f(k, n) {
                break;
            }
        }
        Ok(seen)
    }

    /// The same walk, counting in O(extents) for a fixed-width schema.
    ///
    /// This is the one a breakdown panel should call, and the difference is
    /// not a detail. `scan_counts` costs a `count` per key, so a dictionary
    /// scan is O(every posting in the range) -- for a day index that is the
    /// whole file. This is O(extents), so it is bounded by the dictionary
    /// rather than by the traffic, and no block is touched at all.
    ///
    /// `f` is handed `None` for a key whose stored bytes are not a multiple of
    /// the stride, meaning its values are not all `width` bytes; the caller
    /// can fall back to `count` for that key alone rather than for the scan.
    ///
    /// `f28-count` W2.4 measures the two against each other over a whole
    /// dictionary, because that is what settles whether a browser can answer
    /// top-N itself or whether the roll has to precompute it.
    pub fn scan_counts_fixed<F: FnMut(&[u8], Option<u64>) -> bool>(
        &self,
        from: &[u8],
        limit: usize,
        width: u32,
        mut f: F,
    ) -> Result<usize> {
        let stride = width as u64 + varint_len(width as u64);
        let mut rank = self.seek(from);
        let mut seen = 0usize;
        while seen < limit {
            let Some((k, exts)) = self.exts_at(rank) else {
                break;
            };
            let n = fixed_count(exts, stride);
            seen += 1;
            rank += 1;
            if !f(k, n) {
                break;
            }
        }
        Ok(seen)
    }

    /// Walk keys in order from `from`, visiting every value. R4.4.
    pub fn scan<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        mut f: F,
    ) -> Result<usize> {
        let mut rank = self.seek(from);
        let mut seen = 0usize;
        while seen < limit {
            let Some((k, exts)) = self.exts_at(rank) else {
                break;
            };
            for e in exts {
                self.with_extent(*e, |run| {
                    let mut p = 0usize;
                    while p < run.len() {
                        let len = get_uvarint(run, &mut p) as usize;
                        let end = p
                            .checked_add(len)
                            .ok_or_else(|| corrupt("record length overflows"))?;
                        if end > run.len() {
                            return Err(corrupt("record runs past the end of its extent"));
                        }
                        f(k, &run[p..end]);
                        p = end;
                    }
                    Ok(())
                })?;
            }
            seen += 1;
            rank += 1;
        }
        Ok(seen)
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use crate::bytes::{SliceBytes, VecBytes};

    /// The endianness guard is a claim about this build, so it is checked
    /// rather than commented. If this ever fires, `Blob::open` refuses and the
    /// zero-copy extent borrow is why.
    #[test]
    fn the_format_and_this_target_agree_on_byte_order() {
        const {
            assert!(
                cfg!(target_endian = "little"),
                "supdb's flat index is addressed in place as native-endian and written \
                 little-endian, so the two agree only here"
            )
        };
    }

    #[test]
    fn a_short_object_is_refused_rather_than_indexed() {
        let empty = vec![0u8; 16];
        assert!(Blob::open(SliceBytes(&empty)).is_err());
        let zeroed = vec![0u8; 8192];
        // 4096 bytes of zeroes is long enough to hold a superblock and is not
        // one: the magic is absent, so this is "no checkpoint", not a panic.
        let e = match Blob::open(VecBytes(zeroed)) {
            Ok(_) => panic!("a zeroed object is not a checkpoint"),
            Err(e) => e,
        };
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }
}
