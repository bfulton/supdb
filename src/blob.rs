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
use crate::index::Ext;
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
fn fixed_count(exts: &[Ext], stride: u64, width: u64) -> Option<u64> {
    if stride == 0 {
        return None;
    }
    let mut total = 0u64;
    for e in exts {
        // A fixed run says its width outright (format v6): exact when it is
        // the width asked about, and a definite no when it is not.
        if e.is_fixed() {
            if e.fixed_width()? as u64 != width {
                return None;
            }
            total += u64::from(e.records());
            continue;
        }
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
const MAGIC: u64 = 0x5355_5044_4200_0006;
const SUPER: u64 = 4096;
const SLOT: u64 = 512;
const SB_BYTES: usize = 144;
const SB_FIELDS: usize = 16;

/// The superblock page's extension: what a write-once segment adds after
/// the two slots so a sparse open can plan itself from the probe alone
/// (waves-plan.md, R7.1). Sixteen words -- magic, generation, then the
/// absolute offset and length of the fence, the directory, the hash region
/// and the checksum row, a copy of the block table and a copy of the fence
/// when the writer placed them in a head reserve, and the fence copy's
/// CRC -- then a copy of the key section's 192-byte header, then the FNV of
/// all of it. A store writes none of this; the page is zero there and the
/// magic says so.
const SX_OFF: usize = 1024;
const SX_MAGIC: u64 = 0x5355_5044_4253_5831;
const SX_WORDS: usize = 20;
/// Bytes of the extension: the words, the header copy, the checksum.
pub const SX_BYTES: usize = SX_WORDS * 8 + flatindex::HEADER_BYTES + 8;

/// A decoded superblock extension. Offsets are absolute.
#[derive(Clone, Copy, Debug)]
pub struct SuperExt {
    pub fence: (u64, u64),
    pub dir: (u64, u64),
    pub hash: (u64, u64),
    pub row: (u64, u64),
    /// The block table, when it lives in the head reserve.
    pub table_copy: Option<(u64, u64)>,
    /// A copy of the fence in the head reserve, with its CRC32C.
    pub fence_copy: Option<(u64, u64, u32)>,
    /// A copy of the checksum row in the head reserve (its length is
    /// `row.1`), so verification needs nothing from the section's end.
    pub row_copy: Option<u64>,
    /// A copy of the directory in the head reserve (length `dir.1`), with
    /// its CRC32C, so a directory-resident open needs nothing from the
    /// section either (R7.2).
    pub dir_copy: Option<(u64, u32)>,
    pub header: [u8; flatindex::HEADER_BYTES],
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Encode an extension for a segment of generation `generation`.
pub fn encode_super_ext(x: &SuperExt, generation: u64) -> Vec<u8> {
    let (tco, tcl) = x.table_copy.unwrap_or((0, 0));
    let (fco, fcl, fcc) = x.fence_copy.unwrap_or((0, 0, 0));
    let words: [u64; SX_WORDS] = [
        SX_MAGIC,
        generation,
        x.fence.0,
        x.fence.1,
        x.dir.0,
        x.dir.1,
        x.hash.0,
        x.hash.1,
        x.row.0,
        x.row.1,
        tco,
        tcl,
        fco,
        fcl,
        fcc as u64,
        x.row_copy.unwrap_or(0),
        x.dir_copy.map_or(0, |d| d.0),
        x.dir_copy.map_or(0, |d| d.1 as u64),
        0,
        0,
    ];
    let mut out = Vec::with_capacity(SX_BYTES);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&x.header);
    let h = fnv64(&out);
    out.extend_from_slice(&h.to_le_bytes());
    out
}

/// The extension out of the superblock page, when the page holds one for
/// this generation. `page` is the first `SUPER` bytes of the object; a
/// shorter buffer, a zero page, a wrong generation or a checksum that does
/// not match all read as "none", which is the store's case.
pub fn decode_super_ext(page: &[u8], generation: u64) -> Option<SuperExt> {
    let buf = page.get(SX_OFF..SX_OFF + SX_BYTES)?;
    let w = |i: usize| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap());
    if w(0) != SX_MAGIC || w(1) != generation {
        return None;
    }
    let body = &buf[..SX_BYTES - 8];
    if fnv64(body) != u64::from_le_bytes(buf[SX_BYTES - 8..].try_into().unwrap()) {
        return None;
    }
    let mut header = [0u8; flatindex::HEADER_BYTES];
    header.copy_from_slice(&buf[SX_WORDS * 8..SX_WORDS * 8 + flatindex::HEADER_BYTES]);
    Some(SuperExt {
        fence: (w(2), w(3)),
        dir: (w(4), w(5)),
        hash: (w(6), w(7)),
        row: (w(8), w(9)),
        table_copy: if w(11) > 0 {
            Some((w(10), w(11)))
        } else {
            None
        },
        fence_copy: if w(13) > 0 {
            Some((w(12), w(13), w(14) as u32))
        } else {
            None
        },
        row_copy: if w(15) > 0 { Some(w(15)) } else { None },
        dir_copy: if w(16) > 0 {
            Some((w(16), w(17) as u32))
        } else {
            None
        },
        header,
    })
}

// The write path is not compiled for wasm, so this can only be checked where
// it is -- which is every build that could have changed it.
#[cfg(not(target_family = "wasm"))]
const _: () = {
    assert!(MAGIC == crate::format::MAGIC, "superblock magic drifted");
    assert!(
        SB_BYTES == crate::format::SUPER_BYTES,
        "the superblock changed size and blob.rs still slices the old one"
    );
    assert!(SUPER == crate::format::SUPER);
    assert!(SLOT == crate::format::SLOT);
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
    // The whole superblock page: the two slots, and after them the
    // extension a segment writes so a sparse open can plan itself from
    // this one read (R7.1). A store's page is zero past the slots.
    SUPER
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
    Owned(Vec<u8>, u64),
}

impl Sec {
    fn read<B: Bytes>(src: &B, off: u64, len: usize) -> Result<Sec> {
        if src.slice_at(off, len).is_some() {
            return Ok(Sec::Lent { off, len });
        }
        let mut v = vec![0u8; len];
        src.read_at(off, &mut v)?;
        Ok(Sec::Owned(v, off))
    }

    fn get<'a, B: Bytes>(&'a self, src: &'a B) -> Result<&'a [u8]> {
        match self {
            Sec::Lent { off, len } => src
                .slice_at(*off, *len)
                .ok_or_else(|| short(*off, *len, src.len())),
            Sec::Owned(v, _) => Ok(v),
        }
    }

    fn off(&self) -> u64 {
        match self {
            Sec::Lent { off, .. } => *off,
            Sec::Owned(v, off) => {
                let _ = v;
                *off
            }
        }
    }

    fn len(&self) -> usize {
        match self {
            Sec::Lent { len, .. } => *len,
            Sec::Owned(v, _) => v.len(),
        }
    }
}

/// Everything `Blob::open` and `SparseBlob::open` check before either reads
/// a section: the byte order this reader can address, the superblock, and
/// the redo-log emptiness probe. One function so the two opens cannot
/// drift on what a valid object is.
fn open_head<B: Bytes>(src: &B) -> Result<Super> {
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
    // The whole page, as `open_ranges` plans it: the two slots and the
    // extension a segment may have written after them.
    let mut head = vec![0u8; open_probe() as usize];
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
    Ok(sb)
}

/// How much to check on the way out.
#[derive(Clone, Copy, Debug)]
pub struct BlobOptions {
    /// Verify block checksums. On by default, matching `ReadOptions`: a
    /// browser reading an object off a CDN has more reason to check than a
    /// process reading its own disk, not less.
    pub verify_checksums: bool,
    /// Verify the key index's checksum row when the section carries one:
    /// every piece at a resident open, each piece as it is first used in a
    /// sparse reader. On by default for the same reason.
    pub verify_index: bool,
    /// Sparse reader only: fetch the whole directory in the open wave and
    /// answer every directory slice from memory, so a lookup after open is
    /// one dependent read -- the records -- instead of two. Costs the
    /// directory (four bytes a key) at open; off by default (R7.2).
    pub resident_directory: bool,
}

impl Default for BlobOptions {
    fn default() -> Self {
        BlobOptions {
            verify_checksums: true,
            verify_index: true,
            resident_directory: false,
        }
    }
}

/// The key index as this reader holds it: the whole section, or -- for a
/// reader over an object it will not fetch whole -- its header and fence
/// alone, with every other region reached by offset through the source.
enum Index {
    Flat { key: Sec, idx: FlatIndex },
    Sparse(SparseIndex),
}

/// What `SparseBlob` keeps of the key index: where the section is, its
/// header, and a copy of the fence. Kilobytes, for an index of any size.
struct SparseIndex {
    off: u64,
    len: usize,
    hdr: flatindex::Header,
    fence: Vec<u8>,
    /// The checksum row, empty for a section without one, and one bit per
    /// piece already verified by this reader.
    crcs: Vec<u8>,
    verified: RefCell<Vec<u64>>,
    /// The directory, when `resident_directory` fetched it at open.
    dir: Option<Vec<u32>>,
    /// Whether the open planned itself from the superblock extension.
    from_ext: bool,
}

pub struct Blob<B: Bytes> {
    src: B,
    index: Index,
    blk: Sec,
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
        let sb = open_head(&src)?;
        let key = Sec::read(&src, sb.key_off, sb.key_stored as usize)?;
        let blk = Sec::read(&src, sb.blk_off, sb.blk_stored as usize)?;
        let idx = FlatIndex::parse(key.get(&src)?).ok_or_else(|| {
            corrupt(
                "the key index is not the flat format this reader understands (an older varint \
                 index, or damage in its header)",
            )
        })?;
        // The whole section is resident, so the whole row is checked here,
        // once, and no read path pays anything after (indexsum-plan.md).
        if idx.crc_off != 0 && opts.verify_index {
            if let Err(p) =
                flatindex::verify_pieces(key.get(&src)?, idx.crc_off, idx.piece_shift, sb.key_off)
            {
                return Err(corrupt(&format!(
                    "key index checksum mismatch in piece {p}"
                )));
            }
        }
        let blocks = MappedBlocks::parse(blk.get(&src)?)
            .ok_or_else(|| corrupt("the block table is not readable"))?;
        let verified = vec![0u64; (blocks.len() * block::MAX_CHUNK_CRCS).div_ceil(64)];
        Ok(Blob {
            src,
            // The generation of the index *section*, not of the superblock.
            // They are not always the same number -- a publish can move the
            // superblock on without writing a new index -- and `Reader`
            // reports the section's, because that is the state a reader is
            // actually looking at. `tests/blob.rs` caught this by requiring
            // the two readers to agree, which is what it is for.
            generation: idx.generation,
            index: Index::Flat { key, idx },
            blk,
            blocks,
            timestamp: sb.timestamp,
            opts,
            verified: RefCell::new(verified),
            raw_buf: Cell::new(Vec::new()),
            dec_buf: Cell::new(Vec::new()),
        })
    }

    /// The whole key index and its parsed form, or `None` for a sparse
    /// reader, which never reaches these paths from outside because
    /// `SparseBlob` does not expose them.
    fn flat(&self) -> Option<(&[u8], &FlatIndex)> {
        match &self.index {
            Index::Flat { key, idx } => Some((key.get(&self.src).ok()?, idx)),
            Index::Sparse(_) => None,
        }
    }

    // ------------------------------------------------------------ diagnostics --

    /// Whether the key index carries a checksum row (segments do; a store's
    /// in-place-editable index does not).
    pub fn index_checksummed(&self) -> bool {
        match &self.index {
            Index::Flat { idx, .. } => idx.crc_off != 0,
            Index::Sparse(s) => !s.crcs.is_empty(),
        }
    }

    /// Where the key index section starts in the object.
    pub fn index_offset(&self) -> u64 {
        match &self.index {
            Index::Flat { key, .. } => key.off(),
            Index::Sparse(s) => s.off,
        }
    }

    /// Number of distinct keys. R4.5.
    pub fn keys(&self) -> usize {
        match &self.index {
            Index::Flat { idx, .. } => idx.len(),
            Index::Sparse(s) => s.hdr.nkeys,
        }
    }

    /// Bytes of key index this reader addresses. R4.5.
    pub fn index_bytes(&self) -> usize {
        match &self.index {
            Index::Flat { key, .. } => key.len(),
            Index::Sparse(s) => s.len,
        }
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
        matches!(
            &self.index,
            Index::Flat {
                key: Sec::Lent { .. },
                ..
            }
        )
    }

    // ------------------------------------------------------------ lookup --

    fn blk_sec(&self) -> Result<&[u8]> {
        self.blk.get(&self.src)
    }

    /// The extents of a key, borrowed out of the index. No allocation, no
    /// decode -- this is the borrow `flatindex` exists to make possible, and
    /// it survives the byte-source abstraction on any source that can lend.
    pub fn lookup(&self, key: &[u8]) -> Option<&[Ext]> {
        let (sec, idx) = self.flat()?;
        idx.lookup(sec, key, flatindex::key_hash)
    }

    /// `lookup`, with the record's tail: the bytes of any inline runs, which
    /// `read_exts` needs to serve an extent that names `Ext::INLINE`.
    pub fn lookup_full(&self, key: &[u8]) -> Option<(&[Ext], &[u8])> {
        let (sec, idx) = self.flat()?;
        idx.lookup_full(sec, key, flatindex::key_hash)
    }

    /// `exts_at`, with the record's tail of inline runs.
    pub fn exts_at_full(&self, rank: usize) -> Option<(&[u8], &[Ext], &[u8])> {
        let (sec, idx) = self.flat()?;
        idx.at_full(sec, rank)
    }

    /// Rank of the first key at or after `key`, in key order. R4.4.
    pub fn seek(&self, key: &[u8]) -> usize {
        match self.flat() {
            Some((sec, idx)) => idx.seek_with(sec, key, true),
            None => self.keys(),
        }
    }

    /// The key at `rank` in key order.
    pub fn key_at(&self, rank: usize) -> Option<&[u8]> {
        let (sec, idx) = self.flat()?;
        idx.at(sec, rank).map(|(k, _)| k)
    }

    /// The key and extents at `rank`, borrowed from the mapping: the flags
    /// on the extents are how `db::Db` sees a tombstone without a probe.
    pub fn exts_at(&self, rank: usize) -> Option<(&[u8], &[Ext])> {
        let (sec, idx) = self.flat()?;
        idx.at(sec, rank)
    }

    /// Every value at `rank`, in append order: reads blocks, resolves
    /// nothing. With `seek` and the existing `key_at` this completes an
    /// ordered cursor, which is what `db::Db` merges segments with --
    /// advancing each source's rank instead of re-resolving every key in
    /// every source. A hash probe per key per source is what an ordered
    /// read must not pay, and it is the whole difference between a scan
    /// and a sequence of lookups.
    pub fn values_at<F: FnMut(&[u8])>(&self, rank: usize, mut f: F) -> Result<u64> {
        let Some((_, exts, tail)) = self.exts_at_full(rank) else {
            return Ok(0);
        };
        let mut n = 0u64;
        for e in exts {
            n += self.with_run(*e, tail, |run| {
                crate::index::each_value(run, e, &mut |v| f(v)).map_err(corrupt)
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
        self.plan_exts(exts, out)
    }

    /// The data ranges these extents reach: every block a non-inline one
    /// names, as the whole stored block. For a caller that already holds a
    /// key's extents -- a dictionary range walk -- and wants its values next.
    pub fn ranges_for_exts(&self, exts: &[Ext]) -> Result<Vec<(u64, u64)>> {
        let mut v = Vec::new();
        self.plan_exts(exts, &mut v)?;
        merge_ranges(&mut v);
        Ok(v)
    }

    fn plan_exts(&self, exts: &[Ext], out: &mut Vec<(u64, u64)>) -> Result<()> {
        for e in exts {
            // An inline run is in the index, which is resident after open:
            // it costs the plan nothing and fetches nothing.
            if e.is_inline() {
                continue;
            }
            let loc = self.loc_of(e.block)?;
            let (a, b) = chunk_span(&loc, e);
            out.push((loc.off + a as u64, (b - a) as u64));
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

    /// Byte ranges holding block payload, as (offset, stored length).
    ///
    /// Diagnostic. A corruption experiment that picks byte offsets uniformly
    /// mostly lands in padding or in an index section, so a "how much damage
    /// goes unnoticed" figure taken that way says more about the file's
    /// layout than about the engine's integrity checking. This lets a caller
    /// aim at bytes that actually carry data. An fsck-style tool would want
    /// the same thing.
    ///
    /// Every block in a segment's table is live: a segment is written once and
    /// nothing in it is ever superseded, so unlike a store there is no
    /// orphaned block whose damage would be undetectable and correctly so.
    pub fn block_extents(&self) -> Vec<(u64, u64)> {
        (0..self.blocks.len() as u32)
            .filter_map(|id| self.loc_of(id).ok())
            .filter(|loc| loc.stored > 0)
            .map(|loc| (loc.off, loc.stored as u64))
            .collect()
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
    fn verify(
        &self,
        id: u32,
        loc: BlockLoc,
        raw: &[u8],
        base: usize,
        lo: usize,
        hi: usize,
    ) -> Result<()> {
        // `raw` holds the block's bytes from `base` on: the whole block when
        // `base` is zero and `raw` is `stored` long, else the chunks a
        // partial read fetched (R7.3). `lo..hi` are block offsets.
        if !self.opts.verify_checksums || !block::checksums_on() {
            return Ok(());
        }
        let whole = |this: &Self| -> Result<()> {
            if base != 0 || raw.len() != loc.stored as usize {
                return Err(corrupt("a partial block read needs per-chunk checksums"));
            }
            if this.is_verified(id, 0) {
                return Ok(());
            }
            if block::crc32(raw) != loc.crc {
                return Err(corrupt("block checksum mismatch"));
            }
            this.set_verified(id, 0);
            Ok(())
        };
        if !loc.chunk_crc || hi > base + raw.len() || lo < base || lo >= hi {
            return whole(self);
        }
        let sec = match self.blk_sec() {
            Ok(s) => s,
            Err(_) => return whole(self),
        };
        for j in (lo / block::CHUNK)..=((hi - 1) / block::CHUNK) {
            let a = j * block::CHUNK;
            let b = ((j + 1) * block::CHUNK).min(base + raw.len());
            let (Some(want), true) = (
                self.blocks.chunk_crc(sec, id as usize, j),
                a < b && a >= base,
            ) else {
                return whole(self);
            };
            if self.is_verified(id, j) {
                continue;
            }
            if block::crc32(&raw[a - base..b - base]) != want {
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
    /// `with_extent`, for an extent that may be inline: then the run is a
    /// slice of the record's tail and no block is touched.
    fn with_run<R>(&self, e: Ext, tail: &[u8], f: impl FnOnce(&[u8]) -> Result<R>) -> Result<R> {
        if e.is_inline() {
            let a = e.off as usize;
            let b = a
                .checked_add(e.len as usize)
                .filter(|&b| b <= tail.len())
                .ok_or_else(|| corrupt("inline run runs past its record"))?;
            return f(&tail[a..b]);
        }
        self.with_extent(e, f)
    }

    fn with_extent<R>(&self, e: Ext, f: impl FnOnce(&[u8]) -> Result<R>) -> Result<R> {
        if e.is_inline() {
            return Err(corrupt("inline run read without its record"));
        }
        let loc = self.loc_of(e.block)?;
        let (a, b) = (
            e.off as usize,
            (e.off as usize).saturating_add(e.len as usize),
        );
        // What is fetched: the chunks the run spans when the block is plain
        // and carries per-chunk checksums, else the block (R7.3). The plan
        // in `plan_exts` names the same bytes, which is what keeps W4.1.
        let (c0, c1) = chunk_span(&loc, &e);
        let mut raw_buf = self.raw_buf.take();
        let out = (|| -> Result<R> {
            let raw = take(&self.src, loc.off + c0 as u64, c1 - c0, &mut raw_buf)?;
            if loc.is_plain() {
                if a < c0 || b > c0 + raw.len() {
                    return Err(corrupt("extent runs past its block"));
                }
                self.verify(e.block, loc, raw, c0, a, b)?;
                return f(&raw[a - c0..b - c0]);
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
                    self.verify(e.block, loc, raw, 0, 0, raw.len())?;
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
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], f: F) -> Result<u64> {
        let Some((exts, tail)) = self.lookup_full(key) else {
            return Ok(0);
        };
        self.read_exts(exts, tail, f)
    }

    /// Every value in `exts`, in order -- the extents and tail a
    /// `lookup_full` or `exts_at_full` returned, handed back so a caller that
    /// has already resolved a key and looked at its flags does not resolve it
    /// again. Returns how many.
    pub fn read_exts<F: FnMut(&[u8])>(&self, exts: &[Ext], tail: &[u8], mut f: F) -> Result<u64> {
        let mut n = 0u64;
        for e in exts {
            n += self.with_run(*e, tail, |run| {
                crate::index::each_value(run, e, &mut |v| f(v)).map_err(corrupt)
            })?;
        }
        Ok(n)
    }

    /// Every value of `key`, in append order, appended to `out` back to back
    /// with no framing between them. Returns how many. One buffer for the
    /// whole key rather than one view per record, which is what a caller
    /// holding a common trigram's hundreds of thousands of postings wants
    /// across a boundary that charges per allocation: fixed-width values come
    /// back as one array.
    pub fn read_concat(&self, key: &[u8], out: &mut Vec<u8>) -> Result<u64> {
        self.read_all(key, |v| out.extend_from_slice(v))
    }

    /// How many values two keys' ascending fixed-width runs have in common:
    /// a two-pointer walk over the runs where they lie, comparing `width`
    /// bytes at a time and copying nothing. The kernel EXT.17 said was
    /// missing. Each key's extents must be fixed runs of `width` in
    /// ascending value order (postings are) and the source must lend its
    /// bytes; anything else falls back to decoding both lists, so the
    /// answer is right either way and only the speed differs.
    pub fn intersect_fixed(&self, a: &[u8], b: &[u8], width: usize) -> Result<u64> {
        let (Some((ea, ta)), Some((eb, tb))) = (self.lookup_full(a), self.lookup_full(b)) else {
            return Ok(0);
        };
        let (ra, rb) = match (
            width,
            self.fixed_runs(ea, ta, width)?,
            self.fixed_runs(eb, tb, width)?,
        ) {
            (w, Some(ra), Some(rb)) if w > 0 => (ra, rb),
            _ => {
                let (mut va, mut vb) = (Vec::new(), Vec::new());
                self.read_exts(ea, ta, |v| va.push(v.to_vec()))?;
                self.read_exts(eb, tb, |v| vb.push(v.to_vec()))?;
                let (mut i, mut j, mut n) = (0, 0, 0u64);
                while i < va.len() && j < vb.len() {
                    match va[i].cmp(&vb[j]) {
                        std::cmp::Ordering::Equal => {
                            n += 1;
                            i += 1;
                            j += 1;
                        }
                        std::cmp::Ordering::Less => i += 1,
                        std::cmp::Ordering::Greater => j += 1,
                    }
                }
                return Ok(n);
            }
        };
        // Each key's runs as one stream of `width`-byte values, and a
        // two-pointer walk over the two streams. `chunks_exact` carries no
        // per-step bounds check, which a byte-offset cursor did, and on the
        // Zipf head's lists that check was the whole difference between this
        // and a merge over decoded integers.
        let mut ia = ra.iter().flat_map(|r| r.chunks_exact(width)).peekable();
        let mut ib = rb.iter().flat_map(|r| r.chunks_exact(width)).peekable();
        let mut n = 0u64;
        // Four- and eight-byte values compare as big-endian integers, which
        // orders exactly as the bytes do and costs one instruction where a
        // slice comparison costs a call.
        let cmp = |x: &[u8], y: &[u8]| -> std::cmp::Ordering {
            match width {
                4 => u32::from_be_bytes(x.try_into().unwrap())
                    .cmp(&u32::from_be_bytes(y.try_into().unwrap())),
                8 => u64::from_be_bytes(x.try_into().unwrap())
                    .cmp(&u64::from_be_bytes(y.try_into().unwrap())),
                _ => x.cmp(y),
            }
        };
        while let (Some(x), Some(y)) = (ia.peek(), ib.peek()) {
            match cmp(x, y) {
                std::cmp::Ordering::Equal => {
                    n += 1;
                    ia.next();
                    ib.next();
                }
                std::cmp::Ordering::Less => {
                    ia.next();
                }
                std::cmp::Ordering::Greater => {
                    ib.next();
                }
            }
        }
        Ok(n)
    }

    /// Every run of a key as a lent slice, when every extent is a fixed run
    /// of `width` and the source lends; `None` otherwise.
    fn fixed_runs<'a>(
        &'a self,
        exts: &[Ext],
        tail: &'a [u8],
        width: usize,
    ) -> Result<Option<Vec<&'a [u8]>>> {
        let mut v = Vec::with_capacity(exts.len());
        for e in exts {
            if e.fixed_width() != Some(width) {
                return Ok(None);
            }
            match self.run_slice(e, tail)? {
                Some(r) => v.push(r),
                None => return Ok(None),
            }
        }
        Ok(Some(v))
    }

    /// A run's bytes where they lie, for a source that lends: the record's
    /// tail for an inline run, a verified slice of a plain block otherwise.
    /// `None` when the block is compressed or the source cannot lend, and
    /// the caller must read through `with_run` instead.
    fn run_slice<'a>(&'a self, e: &Ext, tail: &'a [u8]) -> Result<Option<&'a [u8]>> {
        if e.is_inline() {
            let a = e.off as usize;
            let b = a
                .checked_add(e.len as usize)
                .filter(|&b| b <= tail.len())
                .ok_or_else(|| corrupt("inline run runs past its record"))?;
            return Ok(Some(&tail[a..b]));
        }
        let loc = self.loc_of(e.block)?;
        if !loc.is_plain() {
            return Ok(None);
        }
        let Some(raw) = self.src.slice_at(loc.off, loc.stored as usize) else {
            return Ok(None);
        };
        let (a, b) = (
            e.off as usize,
            (e.off as usize).saturating_add(e.len as usize),
        );
        if b > raw.len() {
            return Err(corrupt("extent runs past its block"));
        }
        self.verify(e.block, loc, raw, 0, a, b)?;
        Ok(Some(&raw[a..b]))
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
    /// prefix in front of each value of a prefixed run. A fixed run
    /// (format v6, every value one width) has no prefixes, so for it this is
    /// the payload alone.
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
            Some(exts) => fixed_count(exts, stride, width as u64),
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
            let n = fixed_count(exts, stride, width as u64);
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
            let Some((k, exts, tail)) = self.exts_at_full(rank) else {
                break;
            };
            for e in exts {
                // An inline run is right here in the record the walk is
                // already on: no block, no cache, no fetch.
                if e.is_inline() {
                    self.with_run(*e, tail, |run| {
                        crate::index::each_value(run, e, &mut |v| f(k, v)).map_err(corrupt)
                    })?;
                    continue;
                }
                // The fast path wants a lending source and a plain block:
                // anything else falls back, because a decompressed block
                // lives in a buffer this loop cannot hold across keys.
                let run = match cached {
                    Some((id, bytes)) if id == e.block => Some(bytes),
                    _ => match self.loc_of(e.block) {
                        Ok(loc) if loc.is_plain() => {
                            match self.src.slice_at(loc.off, loc.stored as usize) {
                                Some(bytes) => {
                                    self.verify(e.block, loc, bytes, 0, 0, bytes.len())?;
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
                        crate::index::each_value(run, e, &mut |v| f(k, v)).map_err(corrupt)?;
                    }
                    None => {
                        self.with_extent(*e, |run| {
                            crate::index::each_value(run, e, &mut |v| f(k, v)).map_err(corrupt)
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

// ------------------------------------------------------------ sparse index --

/// The byte ranges `SparseBlob::open` reads first: the superblock probe, the
/// key index *header*, the block table and the redo-log word. The fence
/// follows in a second plan (`open_sparse_fence_ranges`) once the header is
/// resident, because the header is what says where the fence is.
pub fn open_sparse_ranges(head: &[u8], object_len: u64) -> Result<Vec<(u64, u64)>> {
    open_sparse_ranges_opts(head, object_len, false)
}

/// `open_sparse_ranges`, with the directory included when `directory` is
/// set (`BlobOptions::resident_directory`). When the superblock page
/// carries a segment's extension, this one plan names everything the open
/// needs -- fence, block table, checksum row, and the directory if asked --
/// and the second plan is empty: two waves, or one when a host's first
/// probe already covered a head reserve holding the table and the fence.
pub fn open_sparse_ranges_opts(
    head: &[u8],
    object_len: u64,
    directory: bool,
) -> Result<Vec<(u64, u64)>> {
    let sb = pick_super(head, object_len)?;
    let mut v = vec![(0, open_probe())];
    match decode_super_ext(head, sb.generation) {
        Some(x) => {
            let hdr = flatindex::Header::parse(&x.header)
                .ok_or_else(|| corrupt("the superblock extension's header copy does not parse"))?;
            v.push(x.table_copy.unwrap_or((sb.blk_off, sb.blk_stored)));
            match x.fence_copy {
                Some((o, l, _)) => v.push((o, l)),
                None => v.push(x.fence),
            }
            if hdr.crc_off != 0 {
                v.push(match x.row_copy {
                    Some(o) => (o, x.row.1),
                    None => x.row,
                });
            }
            if directory {
                v.push(match x.dir_copy {
                    Some((o, _)) => (o, x.dir.1),
                    None => x.dir,
                });
            }
            round_to_pieces(&mut v, sb.key_off, &hdr);
        }
        None => {
            v.push((
                sb.key_off,
                (flatindex::HEADER_BYTES as u64).min(sb.key_stored),
            ));
            v.push((sb.blk_off, sb.blk_stored));
        }
    }
    if let Some(r) = log_probe_range(&sb) {
        v.push(r);
    }
    merge_ranges(&mut v);
    Ok(v)
}

/// The second open plan: the fence region. `index_header` is the first
/// `flatindex::HEADER_BYTES` of the key index section, resident after the
/// first plan. Empty for an index without a fence.
pub fn open_sparse_fence_ranges(
    head: &[u8],
    object_len: u64,
    index_header: &[u8],
) -> Result<Vec<(u64, u64)>> {
    open_sparse_fence_ranges_opts(head, object_len, index_header, false)
}

/// `open_sparse_fence_ranges`, with the directory when asked. Empty when
/// the superblock page carries an extension, since the first plan then
/// named everything.
pub fn open_sparse_fence_ranges_opts(
    head: &[u8],
    object_len: u64,
    index_header: &[u8],
    directory: bool,
) -> Result<Vec<(u64, u64)>> {
    let sb = pick_super(head, object_len)?;
    if decode_super_ext(head, sb.generation).is_some() {
        return Ok(Vec::new());
    }
    let hdr = flatindex::Header::parse(index_header).ok_or_else(|| {
        corrupt("the key index header is not the flat format this reader understands")
    })?;
    let (off, len) = fence_region(&hdr, sb.key_stored as usize)?;
    let mut v = vec![(sb.key_off + off as u64, len as u64)];
    if directory {
        v.push((sb.key_off + hdr.dir_off as u64, hdr.nkeys as u64 * 4));
    }
    if hdr.crc_off != 0 {
        // A checksummed index: the row itself, exactly; the header's own
        // piece, so the header is verified before any word of it is trusted
        // past this point; and the fence rounded out to the pieces that
        // verify it.
        let row = flatindex::checksum_row_len(hdr.crc_off, hdr.piece_shift, sb.key_off);
        v.push((sb.key_off + hdr.crc_off as u64, row as u64));
        v.push((sb.key_off, flatindex::HEADER_BYTES as u64));
        round_to_pieces(&mut v, sb.key_off, &hdr);
    }
    merge_ranges(&mut v);
    Ok(v)
}

/// Widen every range that lies in the checksummed content of the section
/// to whole pieces -- object pages, clamped to the section -- so what a
/// plan fetches is what its verification reads, and no more than a
/// page-fetching host was going to fetch anyway.
fn round_to_pieces(v: &mut [(u64, u64)], sec_off: u64, hdr: &flatindex::Header) {
    if hdr.crc_off == 0 {
        return;
    }
    let piece = 1u64 << hdr.piece_shift;
    let end = sec_off + hdr.crc_off as u64;
    for r in v.iter_mut() {
        let (a, b) = (r.0, r.0 + r.1);
        if a < sec_off || a >= end || r.1 == 0 {
            continue;
        }
        let a2 = (a / piece * piece).max(sec_off);
        let b2 = (b.div_ceil(piece) * piece).min(end);
        *r = (a2, b2 - a2);
    }
}

/// Where the fence lies in the section: from its offset array to the start
/// of whichever region comes next, or the section end. The blob's exact
/// length is in the array's last word, which is not resident yet, so the
/// region is bounded by layout instead. Both writers put the fence directly
/// before another region, so the slack is alignment padding at most.
/// The stored bytes a read of `e` fetches from its block: the 4 KiB chunks
/// the run spans when the block is plain and carries per-chunk checksums,
/// else the whole block -- compressed and unchunked blocks are verified and
/// decoded whole. Block-relative `(start, end)`.
fn chunk_span(loc: &BlockLoc, e: &Ext) -> (usize, usize) {
    let stored = loc.stored as usize;
    if !loc.is_plain() || !loc.chunk_crc {
        return (0, stored);
    }
    let a = (e.off as usize / block::CHUNK) * block::CHUNK;
    let b = (e.off as usize)
        .saturating_add(e.len as usize)
        .div_ceil(block::CHUNK)
        .saturating_mul(block::CHUNK);
    (a.min(stored), b.min(stored).max(a.min(stored)))
}

fn fence_region(h: &flatindex::Header, sec_len: usize) -> Result<(usize, usize)> {
    if h.fence_n != 0 && (h.fence_offs_off < flatindex::HEADER_BYTES || h.fence_offs_off > sec_len)
    {
        return Err(corrupt("the key index names a fence outside its section"));
    }
    Ok(flatindex::fence_span(h, sec_len))
}

/// `open_sparse_ranges`, reading the superblock through the source. For a
/// host whose bytes are reached only by offset -- the wasm module -- so the
/// two open plans need no bytes carried across the boundary.
pub fn sparse_open_ranges_via<B: Bytes>(src: &B) -> Result<Vec<(u64, u64)>> {
    sparse_open_ranges_via_opts(src, false)
}

pub fn sparse_open_ranges_via_opts<B: Bytes>(src: &B, directory: bool) -> Result<Vec<(u64, u64)>> {
    let mut head = vec![0u8; (open_probe()).min(src.len()) as usize];
    src.read_at(0, &mut head)?;
    open_sparse_ranges_opts(&head, src.len(), directory)
}

/// `open_sparse_fence_ranges`, reading the superblock and the index header
/// through the source; the first plan must be resident.
pub fn sparse_fence_ranges_via<B: Bytes>(src: &B) -> Result<Vec<(u64, u64)>> {
    sparse_fence_ranges_via_opts(src, false)
}

pub fn sparse_fence_ranges_via_opts<B: Bytes>(src: &B, directory: bool) -> Result<Vec<(u64, u64)>> {
    let mut head = vec![0u8; (open_probe()).min(src.len()) as usize];
    src.read_at(0, &mut head)?;
    let sb = pick_super(&head, src.len())?;
    if decode_super_ext(&head, sb.generation).is_some() {
        // The first plan named everything; the section header is not
        // resident and is not needed.
        return Ok(Vec::new());
    }
    let mut hb = vec![0u8; (flatindex::HEADER_BYTES as u64).min(sb.key_stored) as usize];
    src.read_at(sb.key_off, &mut hb)?;
    open_sparse_fence_ranges_opts(&head, src.len(), &hb, directory)
}

/// A reader over an object whose key index it will not fetch whole.
///
/// `Blob` copies the key index and block table at open and answers every
/// question from them. That is the right shape while a dictionary is small
/// and the wrong one the day it is not: a trigram index's dictionary grows
/// with the data, and "index fetched whole at open" becomes the download the
/// design exists to avoid. This reader keeps the section's header and its
/// fence -- kilobytes, for an index of any size -- and reaches the directory
/// and the records by offset through the source, so a *range* of the
/// dictionary costs the bytes of that range: a directory slice, then the
/// records it names. Everything is a plan first, as with `Blob::ranges_for`,
/// so a caching source fetches exactly what the walk will read and the walk
/// itself cannot miss.
///
/// It does no point lookups: those go through the hash, which is itself a
/// region to plan, and nothing needs it yet. A key's values are reachable
/// from a walk -- the extents come with the record, `ranges_for_exts` plans
/// their blocks and `read_exts` reads them; an inline run needs no plan.
pub struct SparseBlob<B: Bytes> {
    blob: Blob<B>,
}

impl<B: Bytes> SparseBlob<B> {
    pub fn open(src: B) -> Result<SparseBlob<B>> {
        SparseBlob::open_with(src, BlobOptions::default())
    }

    pub fn open_with(src: B, opts: BlobOptions) -> Result<SparseBlob<B>> {
        let sb = open_head(&src)?;
        let sec_len = sb.key_stored as usize;
        if sec_len < flatindex::HEADER_BYTES {
            return Err(corrupt("the key index is shorter than its header"));
        }
        // The superblock page whole: a segment's extension, when present,
        // carries the section header and every offset the open needs, so
        // nothing below reads the section's own header (R7.1).
        let mut page = vec![0u8; (SUPER.min(src.len())) as usize];
        src.read_at(0, &mut page)?;
        let ext = decode_super_ext(&page, sb.generation);
        let hb: Vec<u8> = match &ext {
            Some(x) => x.header.to_vec(),
            None => {
                let mut hb = vec![0u8; flatindex::HEADER_BYTES];
                src.read_at(sb.key_off, &mut hb)?;
                hb
            }
        };
        let hdr = flatindex::Header::parse(&hb).ok_or_else(|| {
            corrupt(
                "the key index is not the flat format this reader understands (an older varint \
                 index, or damage in its header)",
            )
        })?;
        // Every region a walk will address, checked against the section now,
        // so a damaged offset fails the open rather than a read.
        let fits = |a: usize, len: usize| {
            a >= flatindex::HEADER_BYTES && a.checked_add(len).is_some_and(|e| e <= sec_len)
        };
        if !fits(hdr.hash_off, hdr.hash_cap.saturating_mul(8))
            || !fits(hdr.dir_off, hdr.nkeys.saturating_mul(4))
            || !fits(hdr.recs_off, hdr.bump)
        {
            return Err(corrupt(
                "the key index header names a region outside its section",
            ));
        }
        let (foff, flen) = fence_region(&hdr, sec_len)?;
        // The fence: from the head reserve's copy when the writer left one,
        // checked against the CRC the extension carries; else from the
        // section, verified through its pieces below.
        let mut fence = vec![0u8; flen];
        let mut fence_from_section = flen > 0;
        if let Some((o, l, crc)) = ext.and_then(|x| x.fence_copy) {
            if l as usize == flen && flen > 0 {
                src.read_at(o, &mut fence)?;
                if block::crc32(&fence) != crc {
                    return Err(corrupt(
                        "the fence copy in the head reserve does not match its checksum",
                    ));
                }
                fence_from_section = false;
            }
        }
        if fence_from_section {
            src.read_at(sb.key_off + foff as u64, &mut fence)?;
        }
        let mut crcs = Vec::new();
        if hdr.crc_off != 0 && opts.verify_index {
            let n = flatindex::checksum_row_len(hdr.crc_off, hdr.piece_shift, sb.key_off);
            if hdr.crc_off.checked_add(n).is_none_or(|e| e > sec_len) {
                return Err(corrupt(
                    "the key index names a checksum row outside its section",
                ));
            }
            crcs = vec![0u8; n];
            let row_at = ext
                .and_then(|x| x.row_copy)
                .unwrap_or(sb.key_off + hdr.crc_off as u64);
            src.read_at(row_at, &mut crcs)?;
        }
        let (toff, tlen) = ext
            .and_then(|x| x.table_copy)
            .unwrap_or((sb.blk_off, sb.blk_stored));
        let blk = Sec::read(&src, toff, tlen as usize)?;
        let blocks = MappedBlocks::parse(blk.get(&src)?)
            .ok_or_else(|| corrupt("the block table is not readable"))?;
        let verified = vec![0u64; (blocks.len() * block::MAX_CHUNK_CRCS).div_ceil(64)];
        let pieces = if crcs.is_empty() { 0 } else { crcs.len() / 4 };
        // The directory whole, when asked: four bytes a key, and every
        // later lookup plans its records with no dependent read (R7.2).
        let mut dir_from_section = opts.resident_directory;
        let dir = if opts.resident_directory {
            let mut raw = vec![0u8; hdr.nkeys * 4];
            match ext.and_then(|x| x.dir_copy) {
                Some((o, crc)) => {
                    src.read_at(o, &mut raw)?;
                    if block::crc32(&raw) != crc {
                        return Err(corrupt(
                            "the directory copy in the head reserve does not match its checksum",
                        ));
                    }
                    dir_from_section = false;
                }
                None => src.read_at(sb.key_off + hdr.dir_off as u64, &mut raw)?,
            }
            Some(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .copied()
                    .map(u32::from_le_bytes)
                    .collect(),
            )
        } else {
            None
        };
        let sp = SparseBlob {
            blob: Blob {
                src,
                generation: hdr.generation,
                index: Index::Sparse(SparseIndex {
                    off: sb.key_off,
                    len: sec_len,
                    hdr,
                    fence,
                    crcs,
                    verified: RefCell::new(vec![0u64; pieces.div_ceil(64)]),
                    dir,
                    from_ext: ext.is_some(),
                }),
                blk,
                blocks,
                timestamp: sb.timestamp,
                opts,
                verified: RefCell::new(verified),
                raw_buf: Cell::new(Vec::new()),
                dec_buf: Cell::new(Vec::new()),
            },
        };
        // What was read out of the section is verified before it is trusted:
        // the header's piece unless the extension supplied the header, the
        // fence's pieces unless the copy did, the directory's if resident.
        if ext.is_none() {
            sp.verify_span(0, flatindex::HEADER_BYTES)?;
        }
        if fence_from_section {
            sp.verify_span(foff, flen)?;
        }
        if dir_from_section {
            sp.verify_span(hdr.dir_off, hdr.nkeys * 4)?;
        }
        Ok(sp)
    }

    /// Verify the pieces covering `[rel, rel + len)` of the section that this
    /// reader has not verified yet, reading each whole piece through the
    /// source -- which holds it, because every plan is rounded to pieces.
    fn verify_span(&self, rel: usize, len: usize) -> Result<()> {
        let s = self.sp();
        if s.crcs.is_empty() || len == 0 || rel >= s.hdr.crc_off {
            return Ok(());
        }
        // Pieces are object pages: piece p of this section is page
        // (first_page + p) intersected with the section's content.
        let piece = 1u64 << s.hdr.piece_shift;
        let first_page = s.off / piece;
        let end = s.off + s.hdr.crc_off as u64;
        let a = s.off + rel as u64;
        let b = (a + len as u64).min(end);
        let p0 = (a / piece - first_page) as usize;
        let p1 = (b.div_ceil(piece) - first_page) as usize;
        let mut buf: Vec<u8> = Vec::new();
        for p in p0..p1 {
            if s.verified.borrow()[p / 64] & (1u64 << (p % 64)) != 0 {
                continue;
            }
            let start = ((first_page + p as u64) * piece).max(s.off);
            let stop = ((first_page + p as u64 + 1) * piece).min(end);
            let n = (stop - start) as usize;
            buf.resize(n, 0);
            self.blob.src.read_at(start, &mut buf)?;
            let want = flatindex::piece_crc(&s.crcs, p)
                .ok_or_else(|| corrupt("the key index checksum row is short"))?;
            if block::crc32(&buf) != want {
                return Err(corrupt(&format!(
                    "key index checksum mismatch in piece {p}"
                )));
            }
            s.verified.borrow_mut()[p / 64] |= 1u64 << (p % 64);
        }
        Ok(())
    }

    fn sp(&self) -> &SparseIndex {
        match &self.blob.index {
            Index::Sparse(s) => s,
            Index::Flat { .. } => unreachable!("a SparseBlob holds a sparse index"),
        }
    }

    pub fn keys(&self) -> usize {
        self.blob.keys()
    }

    /// The byte source, for a caller that has to ensure a plan against it
    /// before a walk -- the shape `cache.mjs` has in the browser.
    pub fn source(&self) -> &B {
        &self.blob.src
    }

    pub fn index_bytes(&self) -> usize {
        self.blob.index_bytes()
    }

    pub fn version(&self) -> (u64, u64) {
        self.blob.version()
    }

    /// Whether the section carries a fence. Without one every range plan
    /// spans the whole dictionary -- correct, and not cheap.
    pub fn has_fence(&self) -> bool {
        let s = self.sp();
        flatindex::FenceView::parse(&s.fence, &s.hdr).is_some()
    }

    /// The ranks a range's keys can occupy, at fence granularity: `[r0, r1)`
    /// holds every key in `lo..hi` and at most one stride more at each end.
    fn rank_window(&self, lo: &[u8], hi: Option<&[u8]>) -> (usize, usize) {
        let s = self.sp();
        let n = s.hdr.nkeys;
        let Some(fv) = flatindex::FenceView::parse(&s.fence, &s.hdr) else {
            return (0, n);
        };
        let r0 = fv.window(lo, n).0;
        let r1 = match hi {
            Some(h) => fv.window(h, n).1.min(n),
            None => n,
        };
        (r0, r1.max(r0))
    }

    /// Phase one of a range plan: the directory slice for `lo..hi` (`None`
    /// above means to the end of the dictionary). Absolute, merged, and it
    /// reads nothing to produce.
    pub fn dictionary_plan(&self, lo: &[u8], hi: Option<&[u8]>) -> Vec<(u64, u64)> {
        let s = self.sp();
        let (r0, r1) = self.rank_window(lo, hi);
        if s.dir.is_some() {
            return Vec::new();
        }
        let entries = r1 - r0 + usize::from(r1 < s.hdr.nkeys);
        let mut v = vec![(
            s.off + s.hdr.dir_off as u64 + r0 as u64 * 4,
            entries as u64 * 4,
        )];
        round_to_pieces(&mut v, s.off, &s.hdr);
        merge_ranges(&mut v);
        v
    }

    /// Whether the directory was fetched at open (`resident_directory`), so
    /// phase one of every range plan is empty.
    pub fn directory_resident(&self) -> bool {
        self.sp().dir.is_some()
    }

    /// Whether the open planned itself from the superblock extension a
    /// segment writes, rather than from the section's own header.
    pub fn opened_from_extension(&self) -> bool {
        self.sp().from_ext
    }

    fn dir_slice(&self, r0: usize, r1: usize) -> Result<Vec<u32>> {
        let s = self.sp();
        let entries = r1 - r0 + usize::from(r1 < s.hdr.nkeys);
        if let Some(d) = &s.dir {
            return Ok(d[r0..r0 + entries].to_vec());
        }
        let mut raw = vec![0u8; entries * 4];
        self.verify_span(s.hdr.dir_off + r0 * 4, entries * 4)?;
        self.blob
            .src
            .read_at(s.off + s.hdr.dir_off as u64 + r0 as u64 * 4, &mut raw)?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect())
    }

    /// The record bytes ranks `[r0, r1)` occupy, from their directory
    /// entries: absolute start, length, and the record-relative base.
    fn record_span(&self, r0: usize, r1: usize, d: &[u32]) -> Result<(u64, usize, usize)> {
        let s = self.sp();
        let h = &s.hdr;
        let recs_abs = s.off + h.recs_off as u64;
        if r1 == r0 {
            return Ok((recs_abs, 0, 0));
        }
        let m = r1 - r0;
        let monotone = d.windows(2).all(|w| w[0] < w[1]);
        let (start, end) = if monotone {
            (
                d[0] as usize,
                if r1 < h.nkeys {
                    d[m] as usize
                } else {
                    h.recs_len
                },
            )
        } else {
            // An index updated in place after these keys were written holds
            // records out of key order, in the slack. The span is then the
            // lowest record named through the content end: wide, and still
            // exact about what the walk reads. A write-once segment and a
            // rewritten index never take this arm.
            (d[..m].iter().copied().min().unwrap_or(0) as usize, h.bump)
        };
        if end < start || end > h.bump {
            return Err(corrupt(
                "the directory names records outside the record region",
            ));
        }
        Ok((recs_abs + start as u64, end - start, start))
    }

    /// Phase two: the record bytes for `lo..hi`. Needs phase one's bytes
    /// resident, and reads exactly them. Absolute, merged.
    pub fn dictionary_plan_records(&self, lo: &[u8], hi: Option<&[u8]>) -> Result<Vec<(u64, u64)>> {
        let (r0, r1) = self.rank_window(lo, hi);
        let d = self.dir_slice(r0, r1)?;
        let (abs, len, _) = self.record_span(r0, r1, &d)?;
        let mut v = vec![(abs, len as u64)];
        round_to_pieces(&mut v, self.sp().off, &self.sp().hdr);
        merge_ranges(&mut v);
        Ok(v)
    }

    /// Every key in `lo..hi` in key order, with its extents and inline tail,
    /// reading exactly the two plans. `f` returns false to stop. Returns how
    /// many keys it handed out.
    pub fn dictionary_walk<F: FnMut(&[u8], &[Ext], &[u8]) -> bool>(
        &self,
        lo: &[u8],
        hi: Option<&[u8]>,
        mut f: F,
    ) -> Result<usize> {
        let (r0, r1) = self.rank_window(lo, hi);
        let d = self.dir_slice(r0, r1)?;
        let (abs, len, base) = self.record_span(r0, r1, &d)?;
        self.verify_span((abs - self.sp().off) as usize, len)?;
        // The records: lent when the source can, else copied into an aligned
        // buffer, because extents are borrowed in place and want 4-byte
        // alignment, which a byte vector does not promise.
        let mut words: Vec<u64> = Vec::new();
        let buf: &[u8] = match self.blob.src.slice_at(abs, len) {
            Some(sl) => sl,
            None => {
                words.resize(len.div_ceil(8), 0);
                // SAFETY: `words` owns at least `len` initialised bytes and
                // is not touched again while `bytes` lives.
                let bytes =
                    unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, len) };
                self.blob.src.read_at(abs, bytes)?;
                bytes
            }
        };
        let mut out = 0usize;
        let mut prev: Option<&[u8]> = None;
        for r in r0..r1 {
            let off = (d[r - r0] as usize)
                .checked_sub(base)
                .ok_or_else(|| corrupt("a directory entry points below the planned span"))?;
            let (key, exts, tail, _) = flatindex::parse_record(buf, off)
                .ok_or_else(|| corrupt("a dictionary record does not decode"))?;
            if prev.is_some_and(|p| p >= key) {
                return Err(corrupt("the dictionary is out of key order"));
            }
            prev = Some(key);
            if key < lo {
                continue;
            }
            if hi.is_some_and(|h| key >= h) {
                break;
            }
            out += 1;
            if !f(key, exts, tail) {
                break;
            }
        }
        Ok(out)
    }

    /// `dictionary_walk` reduced to what a ranking wants: each key and its
    /// record count, summed from the extents. No block is touched.
    pub fn dictionary_counts<F: FnMut(&[u8], u64) -> bool>(
        &self,
        lo: &[u8],
        hi: Option<&[u8]>,
        mut f: F,
    ) -> Result<usize> {
        self.dictionary_walk(lo, hi, |k, exts, _| {
            let n: u64 = exts.iter().map(|e| u64::from(e.records())).sum();
            f(k, n)
        })
    }

    /// The data ranges a key's extents reach; `read_exts` then reads them.
    pub fn ranges_for_exts(&self, exts: &[Ext]) -> Result<Vec<(u64, u64)>> {
        self.blob.ranges_for_exts(exts)
    }

    pub fn read_exts<F: FnMut(&[u8])>(&self, exts: &[Ext], tail: &[u8], f: F) -> Result<u64> {
        self.blob.read_exts(exts, tail, f)
    }
}
