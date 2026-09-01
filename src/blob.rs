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
/// compile for wasm -- it is the write path and it maps files -- so this
/// reader cannot call into it.
///
/// A duplicated format constant is a liability and this one has already gone
/// off. `store.rs` grew two superblock fields, the encoded size went from 120
/// bytes to 136 and the magic to version 2, and this decoder went on slicing
/// 120 -- reading a prefix that no longer reached the checksum, and reporting
/// a perfectly healthy file as "no valid supdb checkpoint". That is the same
/// trap the commit which added those fields describes eight instances of
/// inside `store.rs` itself; this was the ninth, in another module.
///
/// Two things catch it now. `tests/blob.rs` opens a store written by
/// `store.rs` and fails on six paths at once, which is how it was found. And
/// the constants below are asserted equal to `store.rs`'s at compile time on
/// every native build, which says *why* in one line instead of six stack
/// traces.
const MAGIC: u64 = 0x5355_5044_4200_0005;
const SUPER: u64 = 4096;
const SLOT: u64 = 512;
const SB_BYTES: usize = 144;
const SB_FIELDS: usize = 16;

// The write path is not compiled for wasm, so this can only be checked where
// it is -- which is every build that could have changed it.
#[cfg(not(target_family = "wasm"))]
const _: () = {
    assert!(MAGIC == crate::store::MAGIC, "superblock magic drifted");
    assert!(
        SB_BYTES == crate::store::SUPER_BYTES,
        "the superblock changed size and blob.rs still slices the old one"
    );
    assert!(SUPER == crate::store::SUPER);
    assert!(SLOT == crate::store::SLOT);
    // The fields, then the magic, then the checksum.
    assert!(SB_BYTES == SB_FIELDS * 8 + 16, "field count disagrees");
};

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
    /// The redo log arena (fields 13 and 14). `log_len` is the arena's
    /// *capacity*, not its used bytes -- a mid-life full rewrite allocates a
    /// fresh arena with a single zero length-word at its head, which is how
    /// replay knows to stop immediately. A cleanly *closed* store records no
    /// arena at all: `Store::close` drops it, because a store nothing will
    /// append to again has no use for 4 MB of reserved zeroes that every
    /// download of the file pays for.
    ///
    /// `store::Reader` replays the log; this reader does not, deliberately --
    /// replay is a write-path concern and the wasm build exists to not carry
    /// one. That is only sound when the log holds no records, so where an
    /// arena exists, `open` probes its first length word and refuses a
    /// nonzero one: those records are newer than everything in the index by
    /// construction, and a reader that ignored them would quietly serve the
    /// previous state. A sealed object -- a logshed segment after its
    /// closing checkpoint -- never trips this, having no arena to probe; the
    /// probe fires only for a store that was never cleanly closed, a
    /// writer's working file or a crash leftover, which was never this
    /// reader's contract to serve.
    log_off: u64,
    log_len: u64,
}

impl Super {
    fn decode(buf: &[u8]) -> Option<Super> {
        if buf.len() < SB_BYTES {
            return None;
        }
        let f: Vec<u64> = (0..SB_FIELDS)
            .map(|i| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap()))
            .collect();
        if u64::from_le_bytes(buf[SB_FIELDS * 8..SB_FIELDS * 8 + 8].try_into().unwrap()) != MAGIC {
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
        if u64::from_le_bytes(buf[SB_FIELDS * 8 + 8..SB_BYTES].try_into().unwrap()) != h {
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
            log_off: f[13],
            log_len: f[14],
        })
    }
}

/// Decode the superblock pair in `head` and validate what it describes,
/// exactly as `open` will. One function, because `open_ranges` must promise
/// the same ranges `open` goes on to read, and two copies of "which slot
/// wins" is how they would come to differ.
fn pick_super(head: &[u8], object_len: u64) -> Result<Super> {
    if object_len < SUPER {
        return Err(corrupt("file too short to hold a superblock"));
    }
    let want = (SLOT as usize) + SB_BYTES;
    if head.len() < want {
        return Err(short(0, want, head.len() as u64));
    }
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
    if sb.high_water > object_len {
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
    Ok(sb)
}

/// Does the arena the superblock names hold a record? Four bytes decide:
/// replay stops at the first zero length-word, so a nonzero first word is a
/// record this reader would be ignoring. See `Super::log_off` for why that
/// is refused rather than tolerated. An arena too small for a length word
/// cannot hold a record and reads as empty.
fn log_probe_range(sb: &Super) -> Option<(u64, u64)> {
    (sb.log_len >= 4).then_some((sb.log_off, 4))
}

/// How many leading bytes of the object `open` must see before every other
/// byte it will read can be named. `open_ranges` turns those bytes into the
/// rest of the plan.
pub fn open_probe() -> u64 {
    SLOT + SB_BYTES as u64
}

/// The byte ranges `Blob::open` will read, from the first `open_probe()`
/// bytes of an `object_len`-byte object. Sorted, merged, absolute.
///
/// This is the open-time half of the planning seam (R6.2): a caching byte
/// source fetches `0..open_probe()`, hands the bytes here, fetches what comes
/// back, and `open` then runs synchronously with no read it can miss. The
/// plan is the superblock probe itself plus the key index and block table
/// sections -- the two sections `open` copies out once and keeps, which is
/// what makes every later `lookup` a pure plan over resident bytes.
///
/// Refuses everything `open` would refuse about the superblock -- no valid
/// checkpoint, a truncated object, compressed sections, an unreplayed redo
/// log -- so a caller that gets ranges back knows the open will not stumble
/// on the header either.
pub fn open_ranges(head: &[u8], object_len: u64) -> Result<Vec<(u64, u64)>> {
    let sb = pick_super(head, object_len)?;
    let mut v = vec![
        (0, open_probe()),
        (sb.key_off, sb.key_stored),
        (sb.blk_off, sb.blk_stored),
    ];
    // The redo-log probe: four bytes `open` reads to prove the log empty.
    if let Some(r) = log_probe_range(&sb) {
        v.push(r);
    }
    merge_ranges(&mut v);
    Ok(v)
}

/// Sort byte ranges and merge the overlapping and the adjacent.
///
/// Adjacency merges too, because the consumer of a plan is a range fetcher
/// and two touching HTTP ranges are strictly worse than one. Zero-length
/// entries are dropped: they name no byte.
fn merge_ranges(v: &mut Vec<(u64, u64)>) {
    v.retain(|(_, len)| *len > 0);
    v.sort_unstable();
    let mut out = 0usize;
    for i in 0..v.len() {
        if out > 0 && v[i].0 <= v[out - 1].0 + v[out - 1].1 {
            let end = (v[i].0 + v[i].1).max(v[out - 1].0 + v[out - 1].1);
            v[out - 1].1 = end - v[out - 1].0;
        } else {
            v[out] = v[i];
            out += 1;
        }
    }
    v.truncate(out);
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
        // Everything `open` checks about the header lives in `pick_super`,
        // shared with `open_ranges` so the plan and the open cannot drift.
        let sb = pick_super(&head, src.len())?;
        // The log-emptiness probe: this reader does not replay the redo log,
        // which is only sound when there is nothing to replay. See
        // `Super::log_off`. Four bytes, because replay itself stops at the
        // first zero length-word -- the arena describes its own extent.
        if let Some((off, _)) = log_probe_range(&sb) {
            let mut word = [0u8; 4];
            src.read_at(off, &mut word)?;
            if word != [0u8; 4] {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "this store's redo log holds records newer than its index, and this reader \
                     does not replay a log. It reads sealed objects; seal the store with a full \
                     checkpoint (a rolled day index already is) before reading it here",
                ));
            }
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

    /// Every value at `rank`, in append order: reads blocks, resolves
    /// nothing. With `seek` and the existing `key_at` this completes an
    /// ordered cursor, which is what `next::Db` merges segments with --
    /// advancing each source's rank instead of re-resolving every key in
    /// every source. A hash probe per key per source is what an ordered
    /// read must not pay, and it is the whole difference between a scan
    /// and a sequence of lookups.
    pub fn values_at<F: FnMut(&[u8])>(&self, rank: usize, mut f: F) -> Result<u64> {
        let Some((_, exts)) = self.exts_at(rank) else {
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

    // ------------------------------------------------------------ planning --

    /// The byte ranges a read of `key` will touch in the source. R6.2.
    ///
    /// A lookup is already a plan: it consults the key index and returns
    /// extents, and reads no data. An extent names a block, the block table
    /// names the block's bytes, and both sections are resident after `open` --
    /// so every byte a `read_all`, `count` or `scan` of this key will ask the
    /// source for is known before any of them runs. This hands that knowledge
    /// out, so a host that fetches lazily can fetch first, asynchronously,
    /// and the read then runs synchronously and cannot miss.
    ///
    /// **The granularity is the stored block, not the extent.** The read path
    /// reads a whole block from the source per extent -- `with_extent` takes
    /// `BlockLoc::stored` bytes at `BlockLoc::off` however the block is
    /// encoded, and verification and decompression both want the enclosing
    /// bytes, not the extent's slice of them. A plan at extent granularity
    /// would under-report, and the exactness test in `tests/ranges.rs` is
    /// built to catch exactly that.
    ///
    /// **What it does not cover, and until when.** The superblock probe, the
    /// key index and the block table are read at `open` (planned by
    /// `open_ranges`) and are resident from then on; this call names only the
    /// data reads that come after. That split is cheap today because the
    /// sections are small when key cardinality is bounded -- a logshed
    /// segment is ~100 keys of index over megabytes of postings, since terms
    /// come from fields with tens of values each. It stops being cheap the
    /// day the keys are unbounded -- a trigram or free-text index -- and the
    /// index would then need to be planned and fetched sparsely too. The
    /// ranges here are absolute file offsets with no assumption that the
    /// caller holds the rest of the file, so that day changes the host, not
    /// this ABI.
    ///
    /// Sorted, merged (overlapping and adjacent), absolute. Empty for a key
    /// that is not there: no extents, no bytes, and the read will answer zero
    /// values without touching the source.
    pub fn ranges_for(&self, key: &[u8]) -> Result<Vec<(u64, u64)>> {
        let mut v = Vec::new();
        self.plan_key(key, &mut v)?;
        merge_ranges(&mut v);
        Ok(v)
    }

    /// One plan for a set of keys: the union of each key's ranges, deduped
    /// and merged. This is the form a range fetcher wants -- two keys in the
    /// same block cost one fetch, and adjacent blocks cost one request.
    pub fn ranges_for_many(&self, keys: &[&[u8]]) -> Result<Vec<(u64, u64)>> {
        let mut v = Vec::new();
        for key in keys {
            self.plan_key(key, &mut v)?;
        }
        merge_ranges(&mut v);
        Ok(v)
    }

    fn plan_key(&self, key: &[u8], out: &mut Vec<(u64, u64)>) -> Result<()> {
        let Some(exts) = self.lookup(key) else {
            return Ok(());
        };
        for e in exts {
            let loc = self.loc_of(e.block)?;
            out.push((loc.off, loc.stored as u64));
        }
        Ok(())
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
    ///
    /// Asking and recording are separate on purpose, and the order is the
    /// point: a chunk is marked only *after* its checksum matched. The first
    /// version test-and-set before comparing, so one failed verification
    /// poisoned the bitmap and the very next read of the same block served
    /// the corrupt bytes as already-verified -- an error that reported once
    /// and then stopped. `store::Reader::verify_range` always had the right
    /// order; `tests/blob.rs` now pins this one.
    fn is_verified(&self, i: u32, j: usize) -> bool {
        let slot = i as usize * block::MAX_CHUNK_CRCS + j;
        let (w, bit) = (slot / 64, 1u64 << (slot % 64));
        // Out of room means never remembered: check every time rather than skip.
        self.verified
            .borrow()
            .get(w)
            .is_some_and(|cell| cell & bit != 0)
    }

    fn set_verified(&self, i: u32, j: usize) {
        let slot = i as usize * block::MAX_CHUNK_CRCS + j;
        let (w, bit) = (slot / 64, 1u64 << (slot % 64));
        if let Some(cell) = self.verified.borrow_mut().get_mut(w) {
            *cell |= bit;
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
            if this.is_verified(id, 0) {
                return Ok(());
            }
            if block::crc32(raw) != loc.crc {
                return Err(corrupt("block checksum mismatch"));
            }
            this.set_verified(id, 0);
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
            if self.is_verified(id, j) {
                continue;
            }
            if block::crc32(&raw[a..b]) != want {
                return Err(corrupt("block checksum mismatch"));
            }
            self.set_verified(id, j);
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

    /// How many values a key has. O(extents): every extent carries its
    /// record count (`Ext::count`), so nothing in a block is touched.
    ///
    /// It was not always so. f28 measured the walk this used to be at
    /// 2,493 ns against 2,516 to read every value (W2.1): skipping a payload
    /// does not skip the cache lines it sits in. The count moved into the
    /// extent record with format v5, for four bytes an extent.
    pub fn count(&self, key: &[u8]) -> Result<u64> {
        let Some(exts) = self.lookup(key) else {
            return Ok(0);
        };
        Ok(exts.iter().map(|e| u64::from(e.records())).sum())
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
            // Read out of the extent records before the callback so the borrow
            // of the index section does not have to survive it.
            let n: u64 = exts.iter().map(|e| u64::from(e.records())).sum();
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
    ///
    /// Resolves each block ONCE for a run of keys that share it, rather
    /// than per key. A segment written by a seal or a merge holds its keys
    /// in key order (both sort before writing), so consecutive keys land in
    /// the same block and the per-key `loc_of` + slice + buffer dance was
    /// re-deriving an answer it already had. f45 priced that indirection at
    /// 60.1ns of an ordered scan's 90.8, against 14.5 for walking the index
    /// -- the cost is here, not in resolving keys.
    ///
    /// The cache holds only what a borrowed source can lend. A copying
    /// source, or a compressed or chunk-CRC'd block, takes the original
    /// path, which is also the one `tests/blob.rs` checks against
    /// `store.rs`.
    pub fn scan<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        mut f: F,
    ) -> Result<usize> {
        let mut rank = self.seek(from);
        let mut seen = 0usize;
        let mut cached: Option<(u32, &[u8])> = None;
        while seen < limit {
            let Some((k, exts)) = self.exts_at(rank) else {
                break;
            };
            for e in exts {
                // The fast path wants a lending source and a plain block:
                // anything else falls back, because a decompressed block
                // lives in a buffer this loop cannot hold across keys.
                let run = match cached {
                    Some((id, bytes)) if id == e.block => Some(bytes),
                    _ => match self.loc_of(e.block) {
                        Ok(loc) if loc.is_plain() => {
                            match self.src.slice_at(loc.off, loc.stored as usize) {
                                Some(bytes) => {
                                    self.verify(e.block, loc, bytes, 0, bytes.len())?;
                                    cached = Some((e.block, bytes));
                                    Some(bytes)
                                }
                                None => None,
                            }
                        }
                        _ => None,
                    },
                };
                match run {
                    Some(bytes) => {
                        let (a, b) = (
                            e.off as usize,
                            (e.off as usize).saturating_add(e.len as usize),
                        );
                        if b > bytes.len() {
                            return Err(corrupt("extent runs past its block"));
                        }
                        let run = &bytes[a..b];
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
                    }
                    None => {
                        self.with_extent(*e, |run| {
                            let mut p = 0usize;
                            while p < run.len() {
                                let len = get_uvarint(run, &mut p) as usize;
                                let end = p
                                    .checked_add(len)
                                    .ok_or_else(|| corrupt("record length overflows"))?;
                                if end > run.len() {
                                    return Err(corrupt(
                                        "record runs past the end of its extent",
                                    ));
                                }
                                f(k, &run[p..end]);
                                p = end;
                            }
                            Ok(())
                        })?;
                    }
                }
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
