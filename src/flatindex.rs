//! A key index that is read where it lies instead of rebuilt on open.
//!
//! The engine this replaced decoded its index into `Vec<(Vec<u8>, Extents)>`
//! and then hashed it, which cost two things at once. Open was not independent
//! of key count -- 6.4ms at 100k keys, 1446ms at 10M, so 100x the keys cost
//! 225x the open -- and the result was 131 bytes per key, resident in every
//! reader process, shared with nobody. Both are properties of *rebuilding*,
//! not of the data: the bytes on disk are already an index, and the decode
//! existed only because the on-disk shape was not one a lookup can use.
//!
//! So write a shape a lookup can use. `indexlab` measured ten candidates for
//! this and the answer was not the elaborate one: `hash+flatfixed` -- an open
//! addressed hash of (tag, record offset) over a flat blob of records -- beat
//! the current heap index on point lookups at 10M keys (307ns against 370ns)
//! while being 1.6x smaller, and unlike the heap index it is mappable and
//! shared between processes. The two layouts that were *cleverer* about space
//! were both slower.
//!
//! Two things this format pays for deliberately:
//!
//!   * **It is stored uncompressed.** A compressed section has to be
//!     decompressed into a buffer, and a buffer per reader is exactly what a
//!     mapped index exists to avoid. The cost is file size, and it is paid
//!     knowingly: a reader shares the mapping and decodes nothing.
//!   * **It is native-endian.** Extents are read as `&[Ext]` straight out of
//!     the mapping with no decode step at all, which is the whole point, and
//!     that means a file written on a little-endian machine is not readable on
//!     a big-endian one. LMDB makes the same trade for the same reason.
//!
//! Every offset and length below comes out of a file that a corruption
//! experiment deliberately damages, so every one is checked against the bytes
//! actually present. The module returns `None` where the shipped decoder used
//! to panic the calling process.

use crate::index::Ext;

/// "SFIX", little-endian.
const MAGIC: u32 = 0x5849_4653;
const VERSION: u32 = 3;
/// Header size, padded so the hash region starts 8-byte aligned.
///
/// Version 1 was 128 and used every slot but the last. Version 2 adds the
/// fence, which needs three more. The version is bumped with the size: a
/// reader that took a v1 header for a v2 one would read the fence offset as a
/// record offset, and this repository has already shipped one misparse that
/// presented as file corruption.
const HEADER: usize = 192;
/// Bytes per hash slot: a tag in the top eight bits, a record offset below.
const SLOT: usize = 8;
/// Records are 4-aligned so an extent array can be borrowed as `&[Ext]`.
const REC_ALIGN: usize = 4;
/// Bytes per extent record: five little-endian u32s, `Ext`'s layout.
const EXT_BYTES: usize = std::mem::size_of::<Ext>();

/// Spare room left at the end of the record region, as a fraction of it.
///
/// This is what makes a checkpoint incremental. Growing a key's extent list
/// makes its record longer, so it cannot be edited where it lies -- but a new
/// copy can be written into the slack and published by storing its offset into
/// the key's existing hash slot, which is one aligned 8-byte write. No section
/// is rewritten, no superblock flips, and a reader either sees the old record
/// or the new one.
///
/// When the slack runs out the writer falls back to a full rewrite, which
/// reclaims every superseded record at the same time.
const SLACK_NUM: usize = 1;
const SLACK_DEN: usize = 2;

/// Every `stride`-th key, copied out contiguously, so an ordered seek can
/// binary-search a small hot array instead of the record region.
///
/// A seek was a binary search over the records themselves: about twenty probes
/// for a million keys, each landing at a scattered offset in a 36MB region,
/// each a cache miss. Measured, that is 1,637ns fixed per scan -- 61% of a
/// 50-entry scan and 44% of a 100-entry one -- while the per-entry walk costs
/// 20.8ns and is already competitive. The entire scan deficit against LMDB is
/// this seek; LMDB descends a B-tree in about three page touches.
///
/// The fence holds whole keys, not prefixes. The obvious encoding -- the first
/// eight bytes of each key -- is worthless on the keys this suite uses: they
/// are sixteen zero-padded ASCII digits, so for a million keys the first ten
/// bytes are identical on every one of them. Whole keys cost more space and
/// work for any key shape.
fn fence_target() -> usize {
    std::env::var("SUPDB_FENCE_TARGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16384)
}
const FENCE_MIN_STRIDE: usize = 16;

/// Stride for `n` keys: enough entries to narrow the search hard, few enough
/// that the fence stays small enough to sit in cache.
fn fence_stride(n: usize) -> usize {
    let want = n.div_ceil(fence_target()).max(FENCE_MIN_STRIDE);
    want.next_power_of_two()
}

/// The key section's checksum row: one CRC32C per piece of the section,
/// appended after the last region and named by two header words -- the
/// row's offset (zero when there is no row) and the piece shift. A piece is
/// the section's intersection with one 16 KiB page of the OBJECT, not a
/// 16 KiB span of the section: the sparse reader's host fetches whole object
/// pages, so verification then reads exactly the pages the host fetched and
/// never a byte more. The first piece is therefore short by the section's
/// offset within its page, which every caller passes as `base` (the
/// section's object offset modulo the piece). A section the store may edit in place
/// carries no row: a record is published there with one aligned store into
/// a mapping readers hold, and a piece checksum cannot be kept consistent
/// with that lock-free. Segments are write-once and always carry one.
pub const PIECE_SHIFT: u32 = 14;
const W_CRC_OFF: usize = 160;
const W_PIECE_SHIFT: usize = 168;

/// Number of pieces of a section `len` bytes long starting `base` bytes
/// into an object page.
pub fn piece_count(len: usize, shift: u32, base: u64) -> usize {
    let piece = 1u64 << shift;
    ((base % piece) as usize + len).div_ceil(piece as usize)
}

/// Bytes of the row for a section of `len` bytes starting `base` bytes into
/// an object page.
pub fn checksum_row_len(len: usize, shift: u32, base: u64) -> usize {
    piece_count(len, shift, base) * 4
}

/// The pieces of a section: `(start, end)` offsets within it, in order.
pub fn pieces(len: usize, shift: u32, base: u64) -> impl Iterator<Item = (usize, usize)> {
    let piece = 1usize << shift;
    let first = piece - (base % piece as u64) as usize;
    let mut at = 0usize;
    std::iter::from_fn(move || {
        if at >= len {
            return None;
        }
        let end = if at == 0 {
            first.min(len)
        } else {
            (at + piece).min(len)
        };
        let r = (at, end);
        at = end;
        Some(r)
    })
}

/// The row for `sec`, which is the whole section without its row, whose
/// object offset is `base` modulo the piece.
pub fn checksum_row(sec: &[u8], shift: u32, base: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(checksum_row_len(sec.len(), shift, base));
    for (a, b) in pieces(sec.len(), shift, base) {
        out.extend_from_slice(&crate::block::crc32(&sec[a..b]).to_le_bytes());
    }
    out
}

/// Name a row in the header words of a section `len` bytes long.
pub fn set_checksum_words(header: &mut [u8], len: usize) {
    header[W_CRC_OFF..W_CRC_OFF + 8].copy_from_slice(&(len as u64).to_le_bytes());
    header[W_PIECE_SHIFT..W_PIECE_SHIFT + 8].copy_from_slice(&u64::from(PIECE_SHIFT).to_le_bytes());
}

/// A complete section with its row named and appended; `base` is the
/// object offset the section will be written at.
pub fn with_checksums(mut sec: Vec<u8>, base: u64) -> Vec<u8> {
    let len = sec.len();
    set_checksum_words(&mut sec, len);
    let row = checksum_row(&sec, PIECE_SHIFT, base);
    sec.extend_from_slice(&row);
    sec
}

/// The checksum words of a section: `Ok(Some((crc_off, shift)))` for a
/// section naming a row, `Ok(None)` for one without, and `Err` for a row
/// named with a piece shift no writer produces -- a damaged word, which
/// must refuse the section rather than read it unverified (a flip of that
/// one byte was the first thing the reproducer found).
fn checksum_words(sec: &[u8]) -> std::result::Result<Option<(usize, u32)>, ()> {
    let crc_off = rd_u64(sec, W_CRC_OFF).ok_or(())? as usize;
    if crc_off == 0 {
        return Ok(None);
    }
    let shift = rd_u64(sec, W_PIECE_SHIFT).ok_or(())?;
    if !(10..=24).contains(&shift) {
        return Err(());
    }
    Ok(Some((crc_off, shift as u32)))
}

/// Verify every piece of a resident section against its row. `Err(piece)`
/// names the first mismatch; `Err(usize::MAX)` a row the section cannot
/// hold. `base` is the section's object offset.
pub fn verify_pieces(
    sec: &[u8],
    crc_off: usize,
    shift: u32,
    base: u64,
) -> std::result::Result<(), usize> {
    let n = checksum_row_len(crc_off, shift, base);
    let row = sec
        .get(crc_off..crc_off.checked_add(n).ok_or(usize::MAX)?)
        .ok_or(usize::MAX)?;
    for (i, (a, b)) in pieces(crc_off, shift, base).enumerate() {
        if crate::block::crc32(&sec[a..b]) != piece_crc(row, i).ok_or(usize::MAX)? {
            return Err(i);
        }
    }
    Ok(())
}

/// Where the fence lies in a section of `sec_len` bytes: from its offset
/// array to the start of whichever region comes next, or the section end.
/// `(0, 0)` for a section without a fence. Both writers put the fence
/// directly before another region, so the slack is alignment at most.
pub fn fence_span(h: &Header, sec_len: usize) -> (usize, usize) {
    if h.fence_n == 0 {
        return (0, 0);
    }
    let start = h.fence_offs_off;
    if start < HEADER || start > sec_len {
        return (0, 0);
    }
    let end = [h.hash_off, h.dir_base, h.recs_off, h.crc_off, sec_len]
        .into_iter()
        .filter(|&x| x > start)
        .min()
        .unwrap_or(sec_len)
        .min(sec_len);
    (start, end - start)
}

/// Piece `i`'s expected checksum out of a row.
pub fn piece_crc(row: &[u8], i: usize) -> Option<u32> {
    rd_u32(row, i * 4)
}

/// Offsets are 32-bit, so the record region is bounded. At the ~40 bytes per
/// key this format uses that is about 100M keys, past which the caller falls
/// back to the heap index rather than silently truncating.
pub const MAX_RECS: usize = u32::MAX as usize;

/// Bytes of the section header. A reader that holds only the header and the
/// fence -- `blob::SparseBlob`, over an object it will not fetch whole --
/// needs to know how many to ask for before it has parsed anything.
pub const HEADER_BYTES: usize = HEADER;

/// The section header, decoded on its own.
///
/// `FlatIndex::parse` decodes the same words and then checks every region
/// against the section it holds; this is the half that can be done with the
/// header alone, for a reader whose section is not resident and who will
/// fetch the regions it needs by offset. Every offset is relative to the
/// section start and is checked by the caller against the section length.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub generation: u64,
    pub nkeys: usize,
    pub hash_off: usize,
    pub hash_cap: usize,
    /// The *live* directory: the buffer `dir_state` publishes when there are
    /// two, else the only one.
    pub dir_off: usize,
    /// Where the directory region begins, live buffer or not.
    pub dir_base: usize,
    pub dir_cap: usize,
    pub recs_off: usize,
    pub recs_len: usize,
    /// Content end of the record region: past `recs_len` when in-place
    /// updates have written records into the slack.
    pub bump: usize,
    pub fence_offs_off: usize,
    pub fence_n: usize,
    pub fence_stride: usize,
    /// Where the checksum row starts, which is also the length of the
    /// content it covers; zero for a section without one.
    pub crc_off: usize,
    pub piece_shift: u32,
}

impl Header {
    pub fn parse(sec: &[u8]) -> Option<Header> {
        if !is_flat(sec) {
            return None;
        }
        let nkeys = rd_u64(sec, 56)? as usize;
        let hash_cap = rd_u64(sec, 64)? as usize;
        let dir_base = rd_u64(sec, 80)? as usize;
        let recs_len = rd_u64(sec, 96)? as usize;
        let recs_cap = rd_u64(sec, 104)?.max(recs_len as u64) as usize;
        let dir_cap = rd_u64(sec, 144)? as usize;
        let dir_state = rd_u64(sec, 152)?;
        if hash_cap == 0 || !hash_cap.is_power_of_two() {
            return None;
        }
        let (dir_off, nkeys) = if dir_cap == 0 {
            (dir_base, nkeys)
        } else {
            let live_off = (dir_state >> 32) as usize;
            let live_n = (dir_state & 0xffff_ffff) as usize;
            let second = dir_base.checked_add(dir_cap.checked_mul(4)?)?;
            if (live_off != dir_base && live_off != second) || live_n > dir_cap {
                return None;
            }
            (live_off, live_n)
        };
        if nkeys > hash_cap {
            return None;
        }
        Some(Header {
            generation: rd_u64(sec, 8)?,
            nkeys,
            hash_off: rd_u64(sec, 72)? as usize,
            hash_cap,
            dir_off,
            dir_base,
            dir_cap,
            recs_off: rd_u64(sec, 88)? as usize,
            recs_len,
            bump: rd_u64(sec, 112)?.clamp(recs_len as u64, recs_cap as u64) as usize,
            fence_offs_off: rd_u64(sec, 120)? as usize,
            fence_n: rd_u64(sec, 128)? as usize,
            fence_stride: rd_u64(sec, 136)? as usize,
            crc_off: checksum_words(sec).ok()?.map_or(0, |w| w.0),
            piece_shift: checksum_words(sec).ok()?.map_or(0, |w| w.1),
        })
    }
}

/// One record out of any buffer: its key, extents, inline tail, and the
/// bytes it spans -- `record_full`'s decode, for a caller holding a copied
/// range of the record region rather than the mapped section. `off` is
/// relative to `buf`, and the extents are borrowed in place, so `buf` must
/// be 4-aligned where the record's extents fall (a buffer of `u32`s viewed
/// as bytes is; a `Vec<u8>` is not promised to be).
pub type ParsedRecord<'a> = (&'a [u8], &'a [Ext], &'a [u8], usize);

pub fn parse_record(buf: &[u8], off: usize) -> Option<ParsedRecord<'_>> {
    let klen = rd_u16(buf, off)? as usize;
    let n = rd_u16(buf, off + 2)? as usize;
    let key = buf.get(off + 4..off + 4 + klen)?;
    let e_at = off.checked_add(align_up(4 + klen, REC_ALIGN))?;
    let bytes = buf.get(e_at..e_at.checked_add(n.checked_mul(EXT_BYTES)?)?)?;
    if !(bytes.as_ptr() as usize).is_multiple_of(std::mem::align_of::<Ext>()) {
        return None;
    }
    let exts = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const Ext, n) };
    let mut tail_len = 0usize;
    for e in exts {
        if e.is_inline() {
            tail_len = tail_len.max((e.off as usize).checked_add(e.len as usize)?);
        }
    }
    let t_at = e_at + n * EXT_BYTES;
    let tail = buf.get(t_at..t_at.checked_add(tail_len)?)?;
    let len = align_up(t_at + tail_len - off, REC_ALIGN);
    Some((key, exts, tail, len))
}

/// The fence, held on its own: `fence_n + 1` offsets and the key blob they
/// index, copied out of a section that is otherwise not resident.
pub struct FenceView<'a> {
    offs: &'a [u8],
    blob: &'a [u8],
    n: usize,
    stride: usize,
}

impl<'a> FenceView<'a> {
    /// `region` is the bytes from `fence_offs_off` on; the blob follows the
    /// offsets directly and the last offset is its length. `None` if the
    /// region does not hold what the header says, in which case the caller
    /// has no fence and no cheap seek -- which is a fact about the file to
    /// report, not to guess around.
    pub fn parse(region: &'a [u8], h: &Header) -> Option<FenceView<'a>> {
        if h.fence_n == 0 || h.fence_stride == 0 || h.fence_n > h.nkeys {
            return None;
        }
        if h.fence_n != h.nkeys.div_ceil(h.fence_stride) {
            return None;
        }
        let offs_len = h.fence_n.checked_add(1)?.checked_mul(4)?;
        let offs = region.get(..offs_len)?;
        let blob_len = rd_u32(offs, h.fence_n * 4)? as usize;
        let blob = region.get(offs_len..offs_len.checked_add(blob_len)?)?;
        Some(FenceView {
            offs,
            blob,
            n: h.fence_n,
            stride: h.fence_stride,
        })
    }

    /// Bytes the fence region spans: offsets plus blob.
    pub fn region_len(&self) -> usize {
        self.offs.len() + self.blob.len()
    }

    fn key(&self, i: usize) -> Option<&'a [u8]> {
        let a = rd_u32(self.offs, i * 4)? as usize;
        let b = rd_u32(self.offs, (i + 1) * 4)? as usize;
        if a > b {
            return None;
        }
        self.blob.get(a..b)
    }

    /// The ranks a key can lie at: `[start, end]`, one stride wide, the same
    /// window `FlatIndex::seek_with` narrows with. The rank of the first key
    /// not less than `key` is at least `start` and at most `end`.
    pub fn window(&self, key: &[u8], nkeys: usize) -> (usize, usize) {
        let (mut lo, mut hi) = (0usize, self.n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.key(mid).map(|k| k.cmp(key)) {
                Some(std::cmp::Ordering::Less) => lo = mid + 1,
                _ => hi = mid,
            }
        }
        let start = lo.saturating_sub(1) * self.stride;
        let end = (lo * self.stride).min(nkeys).max(start);
        (start, end)
    }
}

#[inline]
fn align_up(n: usize, to: usize) -> usize {
    (n + to - 1) & !(to - 1)
}

fn rd_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
/// Read the byte-order mark, which is the one field stored native-endian.
fn rd_ne_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn rd_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn rd_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// What the writer needs to know before it can lay the section out.
pub struct Plan {
    pub hash_cap: usize,
    /// Entries reserved per directory buffer, and there are two of them when
    /// this exceeds the key count.
    ///
    /// The directory is the only thing standing between an insertion and an
    /// in-place checkpoint. Records have slack and the hash runs at half load,
    /// so both can take a new key where they lie -- but the directory is a
    /// sorted array of record offsets and an insertion shifts everything after
    /// it, which is not a change a concurrent reader can be allowed to catch
    /// half-done. So it is written into the inactive buffer and published with
    /// one aligned store of `dir_state`, exactly as a record is written into
    /// the slack and published with one store of its hash slot.
    ///
    /// Zero when no room for insertion was asked for, and then there is one
    /// buffer and the layout is what it always was. Doubling costs 4 bytes per
    /// key on an index that is about 57, and Supdb already loses the size
    /// axis, so it is opt-in rather than always.
    pub dir_cap: usize,
    pub recs_len: usize,
    /// Record bytes reserved, including slack for in-place updates.
    pub recs_cap: usize,
    /// Record offset of each key, in sorted order.
    pub rec_offs: Vec<u32>,
    /// Fence: number of entries, the stride they were sampled at, where the
    /// offset array starts and where the key blob starts.
    pub fence_n: usize,
    pub fence_stride: usize,
    pub fence_offs_off: usize,
    pub fence_blob_off: usize,
    pub fence_blob_len: usize,
    pub total: usize,
    /// Bytes with anything in them. The slack is a trailing run of zeroes that
    /// only in-place updates ever write to, so it does not have to be built in
    /// memory or written to disk to be reserved -- a file can hold it as a
    /// hole and read back the same zeroes for free.
    pub written: usize,
}

fn record_len(klen: usize, next: usize) -> usize {
    // u16 klen, u16 extent count, the key, pad to 4, then 20 bytes each.
    align_up(4 + klen, REC_ALIGN) + next * EXT_BYTES
}

/// `record_len` plus the record's tail: the bytes of its inline runs, padded
/// so the next record stays 4-aligned.
fn record_len_tail(klen: usize, next: usize, tail: usize) -> usize {
    record_len(klen, next) + align_up(tail, REC_ALIGN)
}

/// Append one record -- `encode`'s record layout, byte for byte -- to `out`,
/// for a writer that streams records as keys arrive instead of building the
/// section at the end. Returns the bytes appended.
pub fn stream_record(out: &mut Vec<u8>, key: &[u8], exts: &[Ext], tail: &[u8]) -> Option<usize> {
    if key.len() > u16::MAX as usize || exts.len() > u16::MAX as usize {
        return None;
    }
    let len = record_len_tail(key.len(), exts.len(), tail.len());
    let base = out.len();
    out.resize(base + len, 0);
    let rec = &mut out[base..];
    rec[0..2].copy_from_slice(&(key.len() as u16).to_le_bytes());
    rec[2..4].copy_from_slice(&(exts.len() as u16).to_le_bytes());
    rec[4..4 + key.len()].copy_from_slice(key);
    let mut at = align_up(4 + key.len(), REC_ALIGN);
    for e in exts {
        rec[at..at + 4].copy_from_slice(&e.block.to_le_bytes());
        rec[at + 4..at + 8].copy_from_slice(&e.off.to_le_bytes());
        rec[at + 8..at + 12].copy_from_slice(&e.len.to_le_bytes());
        rec[at + 12..at + 16].copy_from_slice(&e.last.to_le_bytes());
        rec[at + 16..at + 20].copy_from_slice(&e.count.to_le_bytes());
        at += EXT_BYTES;
    }
    rec[at..at + tail.len()].copy_from_slice(tail);
    Some(len)
}

/// The section header and the trailer for a records-first section: the
/// records were streamed starting at `HEADER`, `recs_len` bytes of them, and
/// the trailer -- fences, directory, hash slots, each aligned -- follows
/// them. `key_at(i)` returns the i-th key (for the fence samples) and
/// `hashes[i]` its hash. Returns (header, trailer, section total).
///
/// The layout is one `FlatIndex::parse` accepts because every region is
/// addressed by offset; it differs from `encode`'s only in order, and
/// `tests/segwriter.rs` holds the two to the same answers on every read.
pub fn stream_trailer<'a>(
    recs_len: usize,
    rec_offs: &[u32],
    key_at: &dyn Fn(usize) -> &'a [u8],
    hashes: &[u64],
    generation: u64,
) -> Option<(Vec<u8>, Vec<u8>, usize)> {
    let n = rec_offs.len();
    if hashes.len() != n || recs_len > MAX_RECS {
        return None;
    }
    let recs_off = HEADER;
    let mut cap = 1usize;
    while cap < n * 2 {
        cap = cap.checked_mul(2)?;
    }
    cap = cap.max(16);
    let mask = cap - 1;

    let mut t: Vec<u8> = Vec::new();
    let mut at = recs_off + recs_len; // absolute section offset of t's end
    fn pad_to(t: &mut Vec<u8>, at: &mut usize, align: usize) {
        let want = align_up(*at, align);
        t.resize(t.len() + (want - *at), 0);
        *at = want;
    }
    // Fences.
    pad_to(&mut t, &mut at, 4);
    let stride = fence_stride(n);
    let fence_n = n.div_ceil(stride);
    let fence_offs_off = at;
    if fence_n > 0 {
        let offs_len = (fence_n + 1) * 4;
        let mut blob: Vec<u8> = Vec::new();
        let mut offs: Vec<u8> = Vec::with_capacity(offs_len);
        for i in 0..fence_n {
            offs.extend_from_slice(&(blob.len() as u32).to_le_bytes());
            blob.extend_from_slice(key_at(i * stride));
        }
        offs.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        t.extend_from_slice(&offs);
        t.extend_from_slice(&blob);
        at += offs_len + blob.len();
    }
    // Directory.
    pad_to(&mut t, &mut at, 4);
    let dir_off = at;
    for o in rec_offs {
        t.extend_from_slice(&o.to_le_bytes());
    }
    at += n * 4;
    // Hash slots: 8-aligned, `encode`'s probe and tag, byte for byte.
    pad_to(&mut t, &mut at, 8);
    let hash_off = at;
    let hash_start = t.len();
    t.resize(hash_start + cap * SLOT, 0);
    for (i, &h) in hashes.iter().enumerate() {
        let tag = ((h >> 56) | 1) & 0xff;
        let packed = (tag << 56) | rec_offs[i] as u64;
        let mut s = (h as usize) & mask;
        loop {
            let a = hash_start + s * SLOT;
            if t[a..a + 8] == [0u8; 8] {
                t[a..a + 8].copy_from_slice(&packed.to_le_bytes());
                break;
            }
            s = (s + 1) & mask;
        }
    }
    at += cap * SLOT;
    let total = at;

    let mut h = vec![0u8; HEADER];
    h[0..4].copy_from_slice(&MAGIC.to_ne_bytes());
    h[4..8].copy_from_slice(&VERSION.to_le_bytes());
    for (i, v) in [
        generation,
        0u64,
        0u64,
        0u64,
        0u64,
        0u64,
        n as u64,
        cap as u64,
        hash_off as u64,
        dir_off as u64,
        recs_off as u64,
        recs_len as u64,
        recs_len as u64,
        recs_len as u64,
        fence_offs_off as u64,
        fence_n as u64,
        stride as u64,
        0u64,
        0u64,
    ]
    .iter()
    .enumerate()
    {
        h[8 + i * 8..16 + i * 8].copy_from_slice(&v.to_le_bytes());
    }
    Some((h, t, total))
}

/// Measure the section before writing it, so the whole thing can be built into
/// one allocation of the right size rather than grown.
/// `insert_slack` is how many keys beyond the current set the directory should
/// have room for. Zero reproduces the single-buffer layout byte for byte.
pub fn plan(all: &[(&[u8], &crate::index::Extents)], insert_slack: usize) -> Option<Plan> {
    plan_inline(all, &[], insert_slack, true)
}

/// `plan`, with a tail of inline-run bytes per key (`tails` is empty or one
/// entry per key) and a choice about the half-again record slack. The slack
/// exists so a store whose keys gain extents can publish updates in place;
/// an immutable segment never will, so its writer turns it off and saves the
/// bytes.
pub fn plan_inline(
    all: &[(&[u8], &crate::index::Extents)],
    tails: &[&[u8]],
    insert_slack: usize,
    record_slack: bool,
) -> Option<Plan> {
    let mut cap = 1usize;
    while cap < all.len() * 2 {
        cap = cap.checked_mul(2)?;
    }
    cap = cap.max(16);

    let mut rec_offs = Vec::with_capacity(all.len());
    let mut at = 0usize;
    for (k, exts) in all {
        if k.len() > u16::MAX as usize {
            return None;
        }
        let n = exts.as_slice().len();
        if n > u16::MAX as usize {
            return None;
        }
        if at > MAX_RECS {
            return None;
        }
        rec_offs.push(at as u32);
        let tail = tails.get(rec_offs.len() - 1).map_or(0, |t| t.len());
        at = at.checked_add(record_len_tail(k.len(), n, tail))?;
    }
    if at > MAX_RECS {
        return None;
    }
    // Half again, so a store whose keys gain extents can publish updates
    // without rewriting anything -- unless the caller says the section is
    // never edited in place.
    let slack = if record_slack {
        at * SLACK_NUM / SLACK_DEN
    } else {
        0
    };
    let recs_cap = at.checked_add(slack)?;
    if recs_cap > MAX_RECS {
        return None;
    }

    // The fence samples every `stride`-th key. `fence_n + 1` offsets, so an
    // entry's key is the span between its offset and the next.
    let stride = fence_stride(all.len());
    let fence_n = all.len().div_ceil(stride);
    let fence_blob_len: usize = (0..fence_n)
        .map(|i| all[i * stride].0.len())
        .try_fold(0usize, |a, b| a.checked_add(b))?;

    // One buffer when nothing asked for insert room, two when something did.
    let dir_cap = if insert_slack == 0 {
        0
    } else {
        all.len().checked_add(insert_slack)?
    };
    let dir_bytes = if dir_cap == 0 {
        all.len() * 4
    } else {
        dir_cap.checked_mul(8)?
    };
    let fence_offs_off = HEADER + cap * SLOT + dir_bytes;
    let fence_offs_len = if fence_n == 0 { 0 } else { (fence_n + 1) * 4 };
    let fence_blob_off = fence_offs_off + fence_offs_len;
    // Records are 4-aligned within the section, and the blob is bytes, so the
    // record region is realigned after it.
    let recs_off = align_up(fence_blob_off + fence_blob_len, REC_ALIGN);
    let total = recs_off + recs_cap;
    Some(Plan {
        hash_cap: cap,
        dir_cap,
        recs_len: at,
        recs_cap,
        rec_offs,
        fence_n,
        fence_stride: stride,
        fence_offs_off,
        fence_blob_off,
        fence_blob_len,
        total,
        written: recs_off + at,
    })
}

/// Serialize the index. `all` must be sorted by key.
///
/// `hash_of` is the store's key hash, passed in rather than duplicated so the
/// writer and the reader can never disagree about it -- a hash mismatch
/// between the two would present as keys that exist and cannot be found.
/// `insert_slack` reserves directory room, and a second buffer to publish into,
/// so a later checkpoint can add keys without rewriting the section. Zero
/// reproduces the single-buffer layout byte for byte.
pub fn encode(
    all: &[(&[u8], &crate::index::Extents)],
    generation: u64,
    prev: Option<(u64, u64, u64, u64, u64)>,
    hash_of: fn(&[u8]) -> u64,
    insert_slack: usize,
    parallel: bool,
) -> Option<(Vec<u8>, usize)> {
    encode_inline(
        all,
        &[],
        generation,
        prev,
        hash_of,
        insert_slack,
        true,
        parallel,
    )
}

/// `encode`, with each key's inline runs (`tails`, empty or one per key,
/// laid down after that key's extents) and the record slack under the
/// caller's control. See `plan_inline`.
#[allow(clippy::too_many_arguments)]
pub fn encode_inline(
    all: &[(&[u8], &crate::index::Extents)],
    tails: &[&[u8]],
    generation: u64,
    prev: Option<(u64, u64, u64, u64, u64)>,
    hash_of: fn(&[u8]) -> u64,
    insert_slack: usize,
    record_slack: bool,
    parallel: bool,
) -> Option<(Vec<u8>, usize)> {
    let p = plan_inline(all, tails, insert_slack, record_slack)?;
    // Only the part with anything in it. The header still describes the full
    // reserved region, so a reader maps the slack and an in-place update
    // writes into it; it simply is not built here and not written.
    let mut out = vec![0u8; p.written];

    let hash_off = HEADER;
    let dir_off = hash_off + p.hash_cap * SLOT;
    let recs_off = p.total - p.recs_cap;

    // Native-endian, and the only field here written that way: this section
    // is the one a lookup addresses in place, handing back `&[Ext]` borrowed
    // from the mapping, so it is meaningful only on the byte order that wrote
    // it. On a little-endian machine these bytes are what `to_le_bytes`
    // produced, so no file already written changes. `is_flat` then rejects a
    // section of the other order instead of reinterpreting its extents.
    out[0..4].copy_from_slice(&MAGIC.to_ne_bytes());
    out[4..8].copy_from_slice(&VERSION.to_le_bytes());
    let (pg, pt, po, ps, pu) = prev.unwrap_or((0, 0, 0, 0, 0));
    for (i, v) in [
        generation,
        pg,
        pt,
        po,
        ps,
        pu,
        all.len() as u64,
        p.hash_cap as u64,
        hash_off as u64,
        dir_off as u64,
        recs_off as u64,
        p.recs_len as u64,
        p.recs_cap as u64,
        // Bump cursor: where the next in-place update writes its record.
        p.recs_len as u64,
        p.fence_offs_off as u64,
        p.fence_n as u64,
        p.fence_stride as u64,
        // Entries reserved per directory buffer, 0 when there is one buffer.
        p.dir_cap as u64,
        // The publish word: which directory is live, and how many keys it
        // holds. Packed into one u64 so an insertion, which changes both,
        // becomes a single aligned store rather than two writes a reader could
        // catch between. Zero means "the header's dir_off and nkeys stand",
        // which is the single-buffer layout.
        if p.dir_cap == 0 {
            0
        } else {
            ((dir_off as u64) << 32) | all.len() as u64
        },
    ]
    .iter()
    .enumerate()
    {
        let at = 8 + i * 8;
        out[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    // Records, then the rank directory, then the hash over them.
    //
    // A key's record goes at `rec_offs[i]`, which is a prefix sum, so a range
    // of keys writes a *contiguous* range of record bytes; its directory entry
    // goes at `i * 4`. Both regions are disjoint per key and the two regions
    // are disjoint from each other, so this loop splits cleanly across threads
    // with no synchronisation and no atomics -- the offsets were already
    // computed by `plan`.
    let _t_enc = std::time::Instant::now();
    let write_records = |recs: &mut [u8], dir: &mut [u8], from: usize, upto: usize| {
        // `from == upto` on an empty index, and there is no first record to
        // take a base from.
        if from >= upto {
            return;
        }
        let rec_base = p.rec_offs[from] as usize;
        for (i, (k, exts)) in all.iter().enumerate().take(upto).skip(from) {
            let (k, exts) = (*k, *exts);
            let base = p.rec_offs[i] as usize - rec_base;
            let slice = exts.as_slice();
            recs[base..base + 2].copy_from_slice(&(k.len() as u16).to_le_bytes());
            recs[base + 2..base + 4].copy_from_slice(&(slice.len() as u16).to_le_bytes());
            recs[base + 4..base + 4 + k.len()].copy_from_slice(k);
            let mut e_at = base + align_up(4 + k.len(), REC_ALIGN);
            for e in slice {
                recs[e_at..e_at + 4].copy_from_slice(&e.block.to_le_bytes());
                recs[e_at + 4..e_at + 8].copy_from_slice(&e.off.to_le_bytes());
                recs[e_at + 8..e_at + 12].copy_from_slice(&e.len.to_le_bytes());
                recs[e_at + 12..e_at + 16].copy_from_slice(&e.last.to_le_bytes());
                recs[e_at + 16..e_at + 20].copy_from_slice(&e.count.to_le_bytes());
                e_at += EXT_BYTES;
            }
            if let Some(tail) = tails.get(i) {
                recs[e_at..e_at + tail.len()].copy_from_slice(tail);
            }
            let d = (i - from) * 4;
            dir[d..d + 4].copy_from_slice(&p.rec_offs[i].to_le_bytes());
        }
    };

    let threads = if parallel {
        std::thread::available_parallelism()
            .map(|t| t.get())
            .unwrap_or(1)
            .min(8)
    } else {
        1
    };
    if threads < 2 || all.len() < 64 * 1024 {
        let (head, recs) = out.split_at_mut(recs_off);
        let dir = &mut head[dir_off..dir_off + all.len() * 4];
        write_records(recs, dir, 0, all.len());
    } else {
        let per = all.len().div_ceil(threads);
        let (head, mut recs) = out.split_at_mut(recs_off);
        let mut dir = &mut head[dir_off..dir_off + all.len() * 4];
        let mut parts: Vec<(&mut [u8], &mut [u8], usize, usize)> = Vec::with_capacity(threads);
        let mut from = 0usize;
        while from < all.len() {
            let upto = (from + per).min(all.len());
            // The record bytes this range owns run to the next range's first
            // record, or to the end of what was written for the last range.
            let rec_end = if upto == all.len() {
                recs.len()
            } else {
                p.rec_offs[upto] as usize - p.rec_offs[from] as usize
            };
            let (r, rrest) = recs.split_at_mut(rec_end);
            let (d, drest) = dir.split_at_mut((upto - from) * 4);
            parts.push((r, d, from, upto));
            recs = rrest;
            dir = drest;
            from = upto;
        }
        std::thread::scope(|sc| {
            for (r, d, a, b) in parts {
                let f = &write_records;
                sc.spawn(move || f(r, d, a, b));
            }
        });
    }

    // `store` is cfg'd off the wasm build; the timing hook goes with it.
    #[cfg(not(target_family = "wasm"))]
    crate::format::enc_phase("recs", _t_enc);
    let _t_enc = std::time::Instant::now();
    // The fence: every stride-th key copied out, with one more offset than
    // entries so an entry's key is the span to the next.
    let mut blob_at = p.fence_blob_off;
    for i in 0..p.fence_n {
        let k = all[i * p.fence_stride].0;
        let o = p.fence_offs_off + i * 4;
        out[o..o + 4].copy_from_slice(&((blob_at - p.fence_blob_off) as u32).to_le_bytes());
        out[blob_at..blob_at + k.len()].copy_from_slice(k);
        blob_at += k.len();
    }
    if p.fence_n > 0 {
        let o = p.fence_offs_off + p.fence_n * 4;
        out[o..o + 4].copy_from_slice(&(p.fence_blob_len as u32).to_le_bytes());
    }

    // `store` is cfg'd off the wasm build; the timing hook goes with it.
    #[cfg(not(target_family = "wasm"))]
    crate::format::enc_phase("fence", _t_enc);
    let _t_enc = std::time::Instant::now();
    let mask = p.hash_cap - 1;
    // The hash is two thirds of the encode: the probe writes land at random
    // slots in a table that is 16MB at a million keys, so it is miss-bound
    // rather than compute-bound, which is the shape more memory-level
    // parallelism helps most.
    //
    // Claiming a slot with compare-exchange makes it safe to do from several
    // threads -- each key ends up owning exactly one slot. It does change
    // where colliding keys land: the sequential loop always gives the earlier
    // key the earlier slot, and here it goes to whichever thread arrived
    // first. Every lookup still finds every key.
    //
    // I wrote "and the file is no longer reproducible byte for byte" here and
    // then checked it, which was wrong: two *sequential* builds of the same
    // input already differ, so this introduces no reproducibility that was
    // there to lose. A superblock timestamp is enough on its own. The claim
    // that survives is narrower -- the index section's slot order becomes
    // scheduling-dependent -- and it is not a cost anything in this repository
    // currently relies on.
    let aligned = (out.as_ptr() as usize + hash_off).is_multiple_of(8);
    if !parallel || threads < 2 || all.len() < 64 * 1024 || !aligned {
        for (i, (k, _)) in all.iter().enumerate() {
            let h = hash_of(k);
            // The tag is forced non-zero so an occupied slot is never all zero,
            // which is what marks a slot empty.
            let tag = ((h >> 56) | 1) & 0xff;
            let packed = (tag << 56) | p.rec_offs[i] as u64;
            let mut s = (h as usize) & mask;
            loop {
                let at = hash_off + s * SLOT;
                if rd_u64(&out, at) == Some(0) {
                    out[at..at + 8].copy_from_slice(&packed.to_le_bytes());
                    break;
                }
                s = (s + 1) & mask;
            }
        }
    } else {
        use std::sync::atomic::{AtomicU64, Ordering};
        /// The slot array, shared across the scope. Every write is a
        /// compare-exchange on one `AtomicU64`, and the region is checked
        /// 8-aligned above, so this is a shared `&[AtomicU64]` in all but
        /// spelling.
        struct Slots(*mut AtomicU64, usize);
        // SAFETY: the pointer addresses `hash_cap` slots inside `out`, which
        // outlives the scope; every access below is an atomic on one slot.
        unsafe impl Sync for Slots {}
        let slots = Slots(
            unsafe { out.as_mut_ptr().add(hash_off) as *mut AtomicU64 },
            p.hash_cap,
        );
        let per = all.len().div_ceil(threads);
        std::thread::scope(|sc| {
            for t in 0..threads {
                let from = t * per;
                let upto = ((t + 1) * per).min(all.len());
                if from >= upto {
                    break;
                }
                let slots = &slots;
                let rec_offs = &p.rec_offs;
                sc.spawn(move || {
                    for (i, (k, _)) in all.iter().enumerate().take(upto).skip(from) {
                        let h = hash_of(k);
                        let tag = ((h >> 56) | 1) & 0xff;
                        let packed = (tag << 56) | rec_offs[i] as u64;
                        let mut s = (h as usize) & mask;
                        loop {
                            debug_assert!(s < slots.1);
                            // SAFETY: `s` is masked to the table size.
                            let cell = unsafe { &*slots.0.add(s) };
                            if cell
                                .compare_exchange(0, packed, Ordering::Relaxed, Ordering::Relaxed)
                                .is_ok()
                            {
                                break;
                            }
                            s = (s + 1) & mask;
                        }
                    }
                });
            }
        });
    }
    Some((out, p.total))
}

/// A validated view of a mapped index: where the three regions are, and
/// nothing copied out of them.
///
/// Deliberately holds offsets rather than borrows. `Reader` owns the mapping,
/// so an index that borrowed from it would make `Reader` self-referential;
/// and re-parsing the header on every lookup to dodge that would put fifteen
/// header reads on a path whose whole purpose is to be short. The section is
/// passed back in at each call instead.
pub struct FlatIndex {
    hash: (usize, usize),
    dir: (usize, usize),
    /// Entries reserved per directory buffer, and where the pair starts.
    /// Zero `dir_cap` means one buffer and no room to insert.
    dir_cap: usize,
    dir_base: usize,
    recs: (usize, usize),
    hash_cap: usize,
    mask: usize,
    nkeys: usize,
    /// Absolute offset of the record region within the section, and how much
    /// of it is reserved. `bump` is where the next in-place update lands.
    recs_off: usize,
    recs_cap: usize,
    bump: usize,
    /// Fence offsets, fence blob, entry count and stride. Zero entries means
    /// no fence, and `seek` falls back to searching the records.
    fence_offs: (usize, usize),
    fence_blob: (usize, usize),
    fence_n: usize,
    fence_stride: usize,
    pub generation: u64,
    pub prev: Option<(u64, u64, u64, u64, u64)>,
    /// The checksum row, when the section carries one: its offset (the
    /// covered length) and piece shift. `Blob::open` verifies it.
    pub crc_off: usize,
    pub piece_shift: u32,
}

/// The hash the flat index is built with.
///
/// It lives here rather than in `store.rs` because `store.rs` is the write
/// path and does not compile for wasm, while `blob.rs` -- the reader that
/// does -- has to agree with the writer about this function exactly. `encode`
/// already takes the hash as a parameter for the same reason: a writer and a
/// reader that disagreed about it would present as keys that exist and cannot
/// be found, which is the hardest class of bug this format can have.
///
/// FNV-1a. Not a fast hash, deliberately: FxHash was tried for the store's
/// internal maps and made the write path ten times slower, because the keys
/// were structured and near-identical and multiply-rotate hashing clusters on
/// exactly that shape.
#[inline]
pub fn key_hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// True if `sec` looks like this format rather than the varint one.
pub fn is_flat(sec: &[u8]) -> bool {
    rd_ne_u32(sec, 0) == Some(MAGIC) && rd_u32(sec, 4) == Some(VERSION)
}

impl FlatIndex {
    /// Validate the header and take borrows of the three regions.
    ///
    /// Only the header is checked here -- that is the entire point, an open
    /// that does work proportional to the key count is the thing being
    /// removed. Per-record bounds are checked at the point of use instead, so
    /// a damaged record costs that one lookup rather than the open.
    pub fn parse(sec: &[u8]) -> Option<FlatIndex> {
        if !is_flat(sec) {
            return None;
        }
        let generation = rd_u64(sec, 8)?;
        let prev_gen = rd_u64(sec, 16)?;
        let prev_ts = rd_u64(sec, 24)?;
        let prev_off = rd_u64(sec, 32)?;
        let prev_stored = rd_u64(sec, 40)?;
        let prev_unc = rd_u64(sec, 48)?;
        let nkeys = rd_u64(sec, 56)? as usize;
        let hash_cap = rd_u64(sec, 64)? as usize;
        let hash_off = rd_u64(sec, 72)? as usize;
        let dir_off = rd_u64(sec, 80)? as usize;
        let recs_off = rd_u64(sec, 88)? as usize;
        let recs_len = rd_u64(sec, 96)? as usize;
        let recs_cap = rd_u64(sec, 104)?.max(recs_len as u64) as usize;
        let bump = rd_u64(sec, 112)?.clamp(recs_len as u64, recs_cap as u64) as usize;
        let fence_offs_off = rd_u64(sec, 120)? as usize;
        let fence_n = rd_u64(sec, 128)? as usize;
        let fence_stride = rd_u64(sec, 136)? as usize;
        let dir_cap = rd_u64(sec, 144)? as usize;
        let dir_state = rd_u64(sec, 152)?;

        if hash_cap == 0 || !hash_cap.is_power_of_two() {
            return None;
        }
        // `dir_state` is what an in-place insertion publishes, so it is the
        // authority when present -- and it comes out of a file a corruption
        // experiment damages on purpose, so it is checked rather than trusted.
        // The live directory has to be one of the two reserved buffers and no
        // longer than one of them holds.
        let (dir_off, nkeys) = if dir_cap == 0 {
            (dir_off, nkeys)
        } else {
            let live_off = (dir_state >> 32) as usize;
            let live_n = (dir_state & 0xffff_ffff) as usize;
            let second = dir_off.checked_add(dir_cap.checked_mul(4)?)?;
            if (live_off != dir_off && live_off != second) || live_n > dir_cap {
                return None;
            }
            (live_off, live_n)
        };
        // Each region must lie inside the section and after the one before it.
        let hash_end = hash_off.checked_add(hash_cap.checked_mul(SLOT)?)?;
        // The whole reserved directory region, not just the live half: the
        // inactive buffer is written by an insertion and must be inside the
        // section too.
        let dir_region_end = if dir_cap == 0 {
            dir_off.checked_add(nkeys.checked_mul(4)?)?
        } else {
            rd_u64(sec, 80)? as usize + dir_cap.checked_mul(8)?
        };
        let dir_end = dir_region_end;
        let recs_end = recs_off.checked_add(recs_cap)?;
        // Regions are addressed by offset, so their order in the section is
        // free: `encode` lays out hash, directory, fences, records, and the
        // segment writer streams records first and puts the rest after them.
        // What is enforced is that every region sits inside the section past
        // the header and that no two overlap.
        let regions = [
            (hash_off, hash_end),
            (dir_off, dir_end),
            (recs_off, recs_end),
        ];
        for (a, b) in regions {
            if a < HEADER || b > sec.len() || a > b {
                return None;
            }
        }
        for i in 0..regions.len() {
            for j in (i + 1)..regions.len() {
                let ((a0, a1), (b0, b1)) = (regions[i], regions[j]);
                if a0 < b1 && b0 < a1 && a0 != a1 && b0 != b1 {
                    return None;
                }
            }
        }
        if nkeys > hash_cap {
            return None;
        }

        // The fence is an optimisation, so a fence that does not check out is
        // dropped rather than failing the open: the records are still the
        // authority and searching them is still correct, just slower. What is
        // not acceptable is trusting an offset out of a damaged file.
        let (fence_offs, fence_blob, fence_n, fence_stride) = (|| {
            if fence_n == 0 || fence_stride == 0 || fence_n > nkeys {
                return None;
            }
            if fence_n != nkeys.div_ceil(fence_stride) {
                return None;
            }
            let offs_end = fence_offs_off.checked_add(fence_n.checked_add(1)?.checked_mul(4)?)?;
            if fence_offs_off < HEADER || offs_end > sec.len() {
                return None;
            }
            let offs = sec.get(fence_offs_off..offs_end)?;
            // The last offset is the blob length, and the blob sits directly
            // after the offsets.
            let blob_len = rd_u32(offs, fence_n * 4)? as usize;
            let blob_end = offs_end.checked_add(blob_len)?;
            if blob_end > sec.len() {
                return None;
            }
            // The fence region may not overlap the others either.
            for (a, b) in [
                (hash_off, hash_end),
                (dir_off, dir_end),
                (recs_off, recs_end),
            ] {
                if fence_offs_off < b && a < blob_end && a != b && fence_offs_off != blob_end {
                    return None;
                }
            }
            Some((
                (fence_offs_off, offs_end),
                (offs_end, blob_end),
                fence_n,
                fence_stride,
            ))
        })()
        .unwrap_or(((0, 0), (0, 0), 0, 0));
        // A row that does not fit is a damaged header word, not a missing
        // row; refuse rather than read unverified.
        // The row's exact length depends on the section's object offset,
        // which the section does not know; the reader checks that when it
        // verifies. Here: the row starts inside the section, past the header.
        let (crc_off, piece_shift) = match checksum_words(sec).ok()? {
            Some((off, shift)) => {
                if off < HEADER || off > sec.len() {
                    return None;
                }
                (off, shift)
            }
            None => (0, 0),
        };
        Some(FlatIndex {
            hash: (hash_off, hash_end),
            dir: (dir_off, dir_end),
            dir_cap,
            dir_base: rd_u64(sec, 80)? as usize,
            recs: (recs_off, recs_end),
            hash_cap,
            mask: hash_cap - 1,
            nkeys,
            recs_off,
            recs_cap,
            bump,
            fence_offs,
            fence_blob,
            fence_n,
            fence_stride,
            generation,
            crc_off,
            piece_shift,
            prev: if prev_off > 0 {
                Some((prev_gen, prev_ts, prev_off, prev_stored, prev_unc))
            } else {
                None
            },
        })
    }

    pub fn len(&self) -> usize {
        self.nkeys
    }

    /// The key and extents of the record at `off` within the record region.
    #[inline]
    fn record<'a>(&self, sec: &'a [u8], off: usize) -> Option<(&'a [u8], &'a [Ext])> {
        self.record_full(sec, off).map(|(k, e, _)| (k, e))
    }

    /// `record`, plus the record's tail: the bytes of its inline runs, sized
    /// from the extents that name `Ext::INLINE`. Empty for a record without
    /// one, which is every record `Store` writes.
    fn record_full<'a>(
        &self,
        sec: &'a [u8],
        off: usize,
    ) -> Option<(&'a [u8], &'a [Ext], &'a [u8])> {
        // Records are laid out 4-aligned within the section and the section is
        // written at an 8-aligned file offset, so the extent borrow inside
        // `parse_record` is aligned by construction. It checks anyway, and the
        // check has already earned its keep: before `write_section_raw`
        // aligned the section, records were aligned relative to the section
        // and not absolutely, and this is what turned undefined behaviour
        // into a miss.
        let recs = sec.get(self.recs.0..self.recs.1)?;
        parse_record(recs, off).map(|(k, e, t, _)| (k, e, t))
    }

    /// Extents for `key`, borrowed from the mapping. No allocation, no decode.
    pub fn lookup<'a>(
        &self,
        sec: &'a [u8],
        key: &[u8],
        hash_of: fn(&[u8]) -> u64,
    ) -> Option<&'a [Ext]> {
        self.lookup_full(sec, key, hash_of).map(|(e, _)| e)
    }

    /// `lookup`, with the record's tail of inline runs.
    pub fn lookup_full<'a>(
        &self,
        sec: &'a [u8],
        key: &[u8],
        hash_of: fn(&[u8]) -> u64,
    ) -> Option<(&'a [Ext], &'a [u8])> {
        let hash = sec.get(self.hash.0..self.hash.1)?;
        let h = hash_of(key);
        let tag = ((h >> 56) | 1) & 0xff;
        let mut s = (h as usize) & self.mask;
        // Bounded by the table size: a full table would otherwise spin here,
        // and the table is only ever half full, but the bound comes out of a
        // file so it is enforced rather than trusted.
        for _ in 0..self.hash_cap {
            let packed = rd_u64(hash, s * SLOT)?;
            if packed == 0 {
                return None;
            }
            if packed >> 56 == tag {
                let off = (packed & 0x00ff_ffff_ffff_ffff) as usize;
                if let Some((k, exts, tail)) = self.record_full(sec, off) {
                    if k == key {
                        return Some((exts, tail));
                    }
                }
            }
            s = (s + 1) & self.mask;
        }
        None
    }

    /// Where an insertion would write the next directory, and how big a set it
    /// may hold. `None` when this index was built without room to insert.
    ///
    /// The inactive buffer, always: the live one is being read.
    pub fn spare_dir(&self) -> Option<(usize, usize)> {
        if self.dir_cap == 0 {
            return None;
        }
        let second = self.dir_base + self.dir_cap * 4;
        let spare = if self.dir.0 == self.dir_base {
            second
        } else {
            self.dir_base
        };
        Some((spare, self.dir_cap))
    }

    /// The word that publishes a directory: which buffer, and how many keys.
    pub fn dir_state(at: usize, nkeys: usize) -> u64 {
        ((at as u64) << 32) | nkeys as u64
    }

    /// Byte offset of the publish word within the section.
    pub const DIR_STATE_AT: usize = 152;

    /// The record at `rank` in key order.    /// The record at `rank` in key order.
    pub fn at<'a>(&self, sec: &'a [u8], rank: usize) -> Option<(&'a [u8], &'a [Ext])> {
        self.at_full(sec, rank).map(|(k, e, _)| (k, e))
    }

    /// `at`, with the record's tail of inline runs.
    pub fn at_full<'a>(
        &self,
        sec: &'a [u8],
        rank: usize,
    ) -> Option<(&'a [u8], &'a [Ext], &'a [u8])> {
        if rank >= self.nkeys {
            return None;
        }
        let dir = sec.get(self.dir.0..self.dir.1)?;
        let off = rd_u32(dir, rank * 4)? as usize;
        self.record_full(sec, off)
    }

    /// Where a key that is *not* present would claim a hash slot.
    ///
    /// The probe stops at the first empty slot, which is where a lookup for
    /// this key would give up, so storing there is what makes it findable.
    /// Returns None if the key is already present -- the caller wants
    /// `slot_of` for that -- or if the table is too full to take another,
    /// which is the load factor `plan` sizes for.
    pub fn slot_for_insert(
        &self,
        sec: &[u8],
        key: &[u8],
        hash_of: fn(&[u8]) -> u64,
    ) -> Option<usize> {
        let hash = sec.get(self.hash.0..self.hash.1)?;
        let h = hash_of(key);
        let tag = ((h >> 56) | 1) & 0xff;
        let mut s = (h as usize) & self.mask;
        for _ in 0..self.hash_cap {
            let at = s * SLOT;
            let packed = rd_u64(hash, at)?;
            if packed == 0 {
                return Some(self.hash.0 + at);
            }
            if packed >> 56 == tag {
                let off = (packed & 0x00ff_ffff_ffff_ffff) as usize;
                if let Some((k, _)) = self.record(sec, off) {
                    if k == key {
                        return None;
                    }
                }
            }
            s = (s + 1) & self.mask;
        }
        None
    }

    /// The rank a key sits at, or would be inserted at to keep the order.
    pub fn rank_for(&self, sec: &[u8], key: &[u8]) -> usize {
        self.seek_with(sec, key, true).min(self.nkeys)
    }

    /// The live directory's entries, for building the next one.
    pub fn dir_entries<'a>(&self, sec: &'a [u8]) -> Option<&'a [u8]> {
        sec.get(self.dir.0..self.dir.0 + self.nkeys * 4)
    }

    /// Where in the section a key's *directory* entry lives, if it is present.    /// Where in the section a key's *directory* entry lives, if it is present.
    ///
    /// The flat index carries two ways to reach a record: the hash, for a
    /// point lookup, and a rank-ordered directory of record offsets, for a
    /// scan. An in-place update writes a new record into the slack and
    /// republishes it by storing its offset -- and it has to store that offset
    /// in *both*, or the two disagree.
    ///
    /// They did. `checkpoint_in_place` updated the hash and not the directory,
    /// so after any in-place checkpoint `read_all` returned the new value and
    /// `scan` returned the old one, silently, for every key updated that way.
    /// A reduced reproducer is `a_scan_sees_what_an_in_place_checkpoint_wrote`.
    ///
    /// The rank is unchanged by an update -- only an insertion moves ranks,
    /// and `checkpoint_in_place` declines those -- so this is a search for
    /// where the key already sits, not a re-sort.
    pub fn dir_slot_of(&self, sec: &[u8], key: &[u8]) -> Option<usize> {
        let rank = self.seek_with(sec, key, true);
        if rank >= self.nkeys {
            return None;
        }
        let (k, _) = self.at(sec, rank)?;
        if k != key {
            return None;
        }
        Some(self.dir.0 + rank * 4)
    }

    /// Where in the section a key's hash slot lives, if the key is present.    /// Where in the section a key's hash slot lives, if the key is present.
    ///
    /// The one thing an incremental update needs: the address of the word to
    /// store into. Returns the slot's byte offset within the section.
    pub fn slot_of(&self, sec: &[u8], key: &[u8], hash_of: fn(&[u8]) -> u64) -> Option<usize> {
        let hash = sec.get(self.hash.0..self.hash.1)?;
        let h = hash_of(key);
        let tag = ((h >> 56) | 1) & 0xff;
        let mut s = (h as usize) & self.mask;
        for _ in 0..self.hash_cap {
            let at = s * SLOT;
            let packed = rd_u64(hash, at)?;
            if packed == 0 {
                return None;
            }
            if packed >> 56 == tag {
                let off = (packed & 0x00ff_ffff_ffff_ffff) as usize;
                if let Some((k, _)) = self.record(sec, off) {
                    if k == key {
                        return Some(self.hash.0 + at);
                    }
                }
            }
            s = (s + 1) & self.mask;
        }
        None
    }

    pub fn bump(&self) -> usize {
        self.bump
    }

    pub fn set_bump(&mut self, at: usize) {
        self.bump = at.clamp(0, self.recs_cap);
    }

    /// Serialize one record, for writing into the slack.
    ///
    /// Returned separately from the store that publishes it, because the two
    /// have to happen in that order and nothing else may see the record in
    /// between: the bytes go down first, the slot that points at them second.
    pub fn encode_record(key: &[u8], exts: &[Ext]) -> Option<Vec<u8>> {
        if key.len() > u16::MAX as usize || exts.len() > u16::MAX as usize {
            return None;
        }
        let mut out = vec![0u8; record_len(key.len(), exts.len())];
        out[0..2].copy_from_slice(&(key.len() as u16).to_le_bytes());
        out[2..4].copy_from_slice(&(exts.len() as u16).to_le_bytes());
        out[4..4 + key.len()].copy_from_slice(key);
        let mut at = align_up(4 + key.len(), REC_ALIGN);
        for e in exts {
            out[at..at + 4].copy_from_slice(&e.block.to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&e.off.to_le_bytes());
            out[at + 8..at + 12].copy_from_slice(&e.len.to_le_bytes());
            out[at + 12..at + 16].copy_from_slice(&e.last.to_le_bytes());
            out[at + 16..at + 20].copy_from_slice(&e.count.to_le_bytes());
            at += EXT_BYTES;
        }
        Some(out)
    }

    /// The inverse of `encode_record`, for a buffer that is not a mapping.
    ///
    /// `record` borrows `&[Ext]` straight out of a mapping and is the read
    /// path; this copies, and exists for the redo log, whose records are read
    /// once at open and applied to an in-memory table rather than served. It
    /// takes the same length and alignment care as `record` does, because a
    /// log is exactly as likely to be damaged as any other part of the file
    /// and the damage tests feed it garbage on purpose.
    pub fn decode_record(rec: &[u8]) -> Option<(Vec<u8>, Vec<Ext>)> {
        let klen = rd_u16(rec, 0)? as usize;
        let n = rd_u16(rec, 2)? as usize;
        let key = rec.get(4..4 + klen)?.to_vec();
        let mut at = align_up(4 + klen, REC_ALIGN);
        let mut exts = Vec::with_capacity(n);
        for _ in 0..n {
            let b = rec.get(at..at + EXT_BYTES)?;
            exts.push(Ext {
                block: u32::from_le_bytes(b[0..4].try_into().ok()?),
                off: u32::from_le_bytes(b[4..8].try_into().ok()?),
                len: u32::from_le_bytes(b[8..12].try_into().ok()?),
                last: u32::from_le_bytes(b[12..16].try_into().ok()?),
                count: u32::from_le_bytes(b[16..20].try_into().ok()?),
            });
            at += EXT_BYTES;
        }
        Some((key, exts))
    }

    /// Reserve room for a record in the slack. Returns its section offset and
    /// its offset relative to the record region, or None if the slack is out.
    ///
    /// Bumping the cursor is the writer's business -- there is one writer, so
    /// no contention -- but the *published* cursor in the header must only
    /// move after the records below it are written, or a crash mid-checkpoint
    /// leaves the header claiming bytes that were never filled in.
    pub fn reserve(&mut self, len: usize) -> Option<(usize, u32)> {
        let end = self.bump.checked_add(align_up(len, REC_ALIGN))?;
        if end > self.recs_cap {
            return None;
        }
        let rel = self.bump as u32;
        let at = self.recs_off + self.bump;
        self.bump = end;
        Some((at, rel))
    }

    /// The header word holding the bump cursor, so the writer can publish it.
    pub const BUMP_AT: usize = 112;

    /// What a slot must contain to point at `rel` for `key`.
    pub fn slot_value(key: &[u8], rel: u32, hash_of: fn(&[u8]) -> u64) -> u64 {
        let h = hash_of(key);
        let tag = ((h >> 56) | 1) & 0xff;
        (tag << 56) | rel as u64
    }

    /// Position of the first key at or after `key`.
    /// The fence key at entry `i`, borrowed from the blob.
    fn fence_key<'a>(&self, sec: &'a [u8], i: usize) -> Option<&'a [u8]> {
        if i >= self.fence_n {
            return None;
        }
        let offs = sec.get(self.fence_offs.0..self.fence_offs.1)?;
        let a = rd_u32(offs, i * 4)? as usize;
        let b = rd_u32(offs, (i + 1) * 4)? as usize;
        if a > b {
            return None;
        }
        let blob = sec.get(self.fence_blob.0..self.fence_blob.1)?;
        blob.get(a..b)
    }

    /// The rank range a key can lie in, according to the fence.
    ///
    /// Entry `i` is the key at rank `i * stride`, so finding the last entry not
    /// greater than the target bounds the answer to one stride. The narrowing
    /// is a binary search over a few hundred kilobytes of contiguous keys
    /// rather than over tens of megabytes of scattered records.
    fn fence_window(&self, sec: &[u8], key: &[u8]) -> (usize, usize) {
        if self.fence_n == 0 || self.fence_stride == 0 {
            return (0, self.nkeys);
        }
        let (mut lo, mut hi) = (0usize, self.fence_n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.fence_key(sec, mid).map(|k| k.cmp(key)) {
                Some(std::cmp::Ordering::Less) => lo = mid + 1,
                // A fence entry that will not read sorts as "not less", the
                // same rule the record search uses. That alone does not make
                // the fence safe -- see `seek_with`, which checks the answer.
                _ => hi = mid,
            }
        }
        // `lo` is the first entry >= key, so the answer is at or after the
        // entry before it and strictly before `lo`'s own stride boundary.
        let start = lo.saturating_sub(1) * self.fence_stride;
        let end = (lo * self.fence_stride).min(self.nkeys).max(start);
        (start, end)
    }

    /// `fence` selects the arm, so both can be run over one file.
    pub fn seek_with(&self, sec: &[u8], key: &[u8], fence: bool) -> usize {
        if fence && self.fence_n > 0 {
            let (lo, hi) = self.fence_window(sec, key);
            let r = self.search(sec, key, lo, hi);
            // The fence is a hint, and a hint out of a file a corruption
            // experiment damages has to be checked against the authority. A
            // fence entry that will not read widens the window harmlessly, but
            // one whose *bytes* were flipped still compares -- just wrongly --
            // and can move the window past the answer. Two record probes catch
            // that, against the twenty the fence saved, and a fence that fails
            // them costs a full search rather than a wrong answer.
            if self.brackets(sec, key, r) {
                return r;
            }
        }
        self.search(sec, key, 0, self.nkeys)
    }

    /// Is `r` the first rank whose key is not less than `key`?
    fn brackets(&self, sec: &[u8], key: &[u8], r: usize) -> bool {
        if r > self.nkeys {
            return false;
        }
        if r > 0 {
            match self.at(sec, r - 1).map(|(k, _)| k.cmp(key)) {
                Some(std::cmp::Ordering::Less) => {}
                _ => return false,
            }
        }
        if r < self.nkeys {
            match self.at(sec, r).map(|(k, _)| k.cmp(key)) {
                Some(std::cmp::Ordering::Less) => return false,
                None => return false,
                _ => {}
            }
        }
        true
    }

    fn search(&self, sec: &[u8], key: &[u8], mut lo: usize, mut hi: usize) -> usize {
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.at(sec, mid).map(|(k, _)| k.cmp(key)) {
                Some(std::cmp::Ordering::Less) => lo = mid + 1,
                // A record that will not decode sorts as "not less", which
                // keeps the search terminating on damaged input.
                _ => hi = mid,
            }
        }
        lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Extents;

    /// `encode` returns only the bytes with anything in them; the file holds
    /// the rest as a hole that reads back as zeroes. A test reading the section
    /// has to see what a reader sees, so it fills the hole in.
    /// The encoder borrows keys out of the shard arenas rather than copying
    /// them; a test that owns its corpus lends it the same way.
    fn refs(all: &[(Vec<u8>, Extents)]) -> Vec<(&[u8], &Extents)> {
        all.iter().map(|(k, e)| (k.as_slice(), e)).collect()
    }

    fn padded(x: (Vec<u8>, usize)) -> Vec<u8> {
        let (mut v, reserve) = x;
        v.resize(reserve, 0);
        v
    }

    fn h(key: &[u8]) -> u64 {
        let mut x: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in key {
            x ^= b as u64;
            x = x.wrapping_mul(0x1000_0000_01b3);
        }
        x
    }

    fn ext(n: u32) -> Ext {
        Ext {
            block: n,
            off: n * 2,
            len: n * 3,
            last: n * 4,
            count: n + 1,
        }
    }

    fn corpus(n: usize) -> Vec<(Vec<u8>, Extents)> {
        let mut v: Vec<(Vec<u8>, Extents)> = (0..n)
            .map(|i| {
                let k = format!("key{i:012}").into_bytes();
                let mut e = Extents::None;
                // Every eleventh key carries several extents, so the
                // multi-extent path is exercised rather than assumed.
                for j in 0..(if i % 11 == 0 { 3 } else { 1 }) {
                    e.push(ext(i as u32 + j));
                }
                (k, e)
            })
            .collect();
        v.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn every_key_is_found_with_its_extents() {
        let all = corpus(5000);
        let sec = padded(encode(&refs(&all), 7, None, h, 0, false).expect("encode"));
        let ix = FlatIndex::parse(&sec).expect("parse");
        assert_eq!(ix.len(), all.len());
        assert_eq!(ix.generation, 7);
        for (k, e) in &all {
            let got = ix.lookup(&sec, k, h).expect("present");
            assert_eq!(got, e.as_slice(), "extents differ for {k:?}");
        }
    }

    #[test]
    fn absent_keys_are_absent() {
        let all = corpus(2000);
        let sec = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        for i in 0..2000 {
            let k = format!("absent{i:012}").into_bytes();
            assert!(ix.lookup(&sec, &k, h).is_none());
        }
    }

    #[test]
    fn rank_order_matches_key_order() {
        let all = corpus(1000);
        let sec = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        for (i, (k, e)) in all.iter().enumerate() {
            let (gk, ge) = ix.at(&sec, i).expect("rank present");
            assert_eq!(gk, k.as_slice());
            assert_eq!(ge, e.as_slice());
        }
        assert!(ix.at(&sec, all.len()).is_none());
    }

    /// The fence is an optimisation, so the only thing that makes it safe is
    /// that it cannot change an answer. Both arms are held to the same one for
    /// every present key, every gap between keys, and both ends.
    #[test]
    fn the_fence_never_changes_where_a_seek_lands() {
        for n in [0usize, 1, 17, 64, 65, 5000] {
            let all = corpus(n);
            let sec = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
            let ix = FlatIndex::parse(&sec).expect("parse");
            let mut probes: Vec<Vec<u8>> = Vec::new();
            for (k, _) in &all {
                probes.push(k.clone());
                let mut before = k.clone();
                before.pop();
                probes.push(before);
                let mut after = k.clone();
                after.push(0xff);
                probes.push(after);
            }
            probes.push(Vec::new());
            probes.push(vec![0xff; 32]);
            for p in &probes {
                assert_eq!(
                    ix.seek_with(&sec, p, true),
                    ix.seek_with(&sec, p, false),
                    "n={n} disagreed on {p:?}"
                );
            }
        }
    }

    /// Damage anywhere in the fence must not change an answer.
    ///
    /// The first version of this test damaged the whole section and required
    /// the two arms to agree, which is not a property anyone can have: a
    /// flipped byte in the *records* leaves the array unsorted, and then both
    /// arms binary-search a broken order and land in different wrong places.
    /// The records are the authority, so what has to hold is that consulting
    /// the fence never changes what the authority says -- which is why
    /// `seek_with` checks the fence's conclusion against two records rather
    /// than trusting it.
    #[test]
    fn a_damaged_fence_never_changes_an_answer() {
        let all = corpus(2000);
        let clean = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
        // Header fields: the fence offsets start at 120, the records at 88.
        let fence_from = rd_u64(&clean, 120).unwrap() as usize;
        let fence_to = rd_u64(&clean, 88).unwrap() as usize;
        assert!(fence_from > 0 && fence_from < fence_to);
        let truth: Vec<usize> = all
            .iter()
            .step_by(53)
            .map(|(k, _)| {
                let ix = FlatIndex::parse(&clean).unwrap();
                ix.seek_with(&clean, k, false)
            })
            .collect();
        let mut damaged = 0usize;
        for i in (fence_from..fence_to).step_by(7) {
            let mut sec = clean.clone();
            sec[i] ^= 0xff;
            let Some(ix) = FlatIndex::parse(&sec) else {
                continue;
            };
            damaged += 1;
            for ((k, _), want) in all.iter().step_by(53).zip(&truth) {
                assert_eq!(
                    ix.seek_with(&sec, k, true),
                    *want,
                    "byte {i} of the fence changed a seek"
                );
            }
        }
        assert!(damaged > 100, "only {damaged} damaged sections were parsed");
    }

    #[test]
    fn seek_finds_the_first_key_at_or_after() {
        let all = corpus(500);
        let sec = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        for (i, (k, _)) in all.iter().enumerate() {
            assert_eq!(ix.seek_with(&sec, k, true), i);
        }
        assert_eq!(ix.seek_with(&sec, b"\x00", true), 0);
        assert_eq!(ix.seek_with(&sec, b"\xff", true), all.len());
    }

    /// The reason this module exists rather than a struct with a `decode`:
    /// a damaged section must cost the caller an answer, never the process.
    #[test]
    fn damage_never_panics() {
        let all = corpus(300);
        let sec = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
        for i in 0..sec.len() {
            for bit in [0x01u8, 0x80] {
                let mut d = sec.clone();
                d[i] ^= bit;
                if let Some(ix) = FlatIndex::parse(&d) {
                    for (k, _) in all.iter().take(40) {
                        let _ = ix.lookup(&sec, k, h);
                    }
                    for r in 0..ix.len().min(40) {
                        let _ = ix.at(&sec, r);
                    }
                    let _ = ix.seek_with(&sec, b"key000000000100", true);
                }
            }
        }
    }

    #[test]
    fn truncation_never_panics() {
        let all = corpus(200);
        let sec = padded(encode(&refs(&all), 1, None, h, 0, false).unwrap());
        for cut in 0..sec.len() {
            if let Some(ix) = FlatIndex::parse(&sec[..cut]) {
                for (k, _) in all.iter().take(20) {
                    let _ = ix.lookup(&sec, k, h);
                }
                for r in 0..ix.len().min(20) {
                    let _ = ix.at(&sec, r);
                }
            }
        }
    }

    #[test]
    fn an_empty_index_round_trips() {
        let sec = padded(encode(&refs(&[]), 3, None, h, 0, false).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        assert_eq!(ix.len(), 0);
        assert!(ix.lookup(&sec, b"anything", h).is_none());
        assert_eq!(ix.seek_with(&sec, b"anything", true), 0);
    }

    #[test]
    fn the_previous_index_is_carried() {
        let all = corpus(10);
        let sec =
            padded(encode(&refs(&all), 9, Some((8, 1234, 4096, 100, 200)), h, 0, false).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        assert_eq!(ix.prev, Some((8, 1234, 4096, 100, 200)));
    }
}

// ------------------------------------------------------------ block table --

/// "SBLK", little-endian.
const BLK_MAGIC: u32 = 0x4B4C_4253;
/// Header, padded so the first entry lands 8-aligned.
const BLK_HEADER: usize = 16;

/// A block table entry as it sits in the file.
///
/// Deliberately *not* `BlockLoc`. `BlockLoc` carries two `bool`s, and a `bool`
/// has exactly two valid bit patterns -- borrowing an array of them out of a
/// file that a corruption experiment deliberately damages would be undefined
/// behaviour, not a wrong answer. Every field here is an integer, so every bit
/// pattern is valid and a damaged entry yields nonsense the existing bounds
/// checks reject rather than something the compiler may assume cannot happen.
#[derive(Clone, Copy)]
#[repr(C)]
struct BlockRec {
    off: u64,
    stored: u32,
    uncompressed: u32,
    cap: u32,
    crc: u32,
    /// bit 0 solo, bit 1 chunked, bit 2 per-chunk checksums present.
    flags: u8,
    _pad: [u8; 7],
}

const BLK_ENTRY: usize = std::mem::size_of::<BlockRec>();
/// Bytes of per-chunk checksums stored per block.
const CRC_ROW: usize = crate::block::MAX_CHUNK_CRCS * 4;

impl BlockRec {
    #[inline]
    fn to_loc(self) -> crate::block::BlockLoc {
        crate::block::BlockLoc {
            off: self.off,
            stored: self.stored,
            uncompressed: self.uncompressed,
            cap: self.cap,
            crc: self.crc,
            solo: self.flags & 1 != 0,
            chunked: self.flags & 2 != 0,
            chunk_crc: self.flags & 4 != 0,
        }
    }
    #[inline]
    fn of(b: &crate::block::BlockLoc) -> BlockRec {
        BlockRec {
            off: b.off,
            stored: b.stored,
            uncompressed: b.uncompressed,
            cap: b.cap,
            crc: b.crc,
            flags: (b.solo as u8) | ((b.chunked as u8) << 1) | ((b.chunk_crc as u8) << 2),
            _pad: [0; 7],
        }
    }
}

/// Serialize the block table as a flat array.
///
/// The varint encoding it replaces was five varints and a flag byte per block,
/// decoded into a `Vec` on every reader open. Callgrind measured that decode at
/// 34% of all instructions in a checkpoint-heavy workload: it is O(block
/// count), block count grows with overwrite churn, and every open paid it
/// again. Same treatment the key index already gets, for the same reason.
/// The block table, followed by a fixed row of per-chunk checksums per block.
///
/// The checksums sit here rather than inside the blocks because a plain block
/// is sliced straight out of the mapping and has to stay byte-for-byte what
/// the extent offsets say it is. The row is fixed width so a block's
/// checksums are at a computed offset with nothing to store per block; a block
/// that has none carries a row of zeroes and its `chunk_crc` flag clear.
pub fn encode_blocks(
    blocks: &[crate::block::BlockLoc],
    chunk_crcs: &[[u32; crate::block::MAX_CHUNK_CRCS]],
) -> Vec<u8> {
    let crcs_off = BLK_HEADER + blocks.len() * BLK_ENTRY;
    let mut out = vec![0u8; crcs_off + blocks.len() * CRC_ROW];
    // Native-endian for the same reason as the index section's: `BlockRec` is
    // reinterpreted in place rather than decoded.
    out[0..4].copy_from_slice(&BLK_MAGIC.to_ne_bytes());
    out[4..8].copy_from_slice(&(BLK_ENTRY as u32).to_le_bytes());
    out[8..16].copy_from_slice(&(blocks.len() as u64).to_le_bytes());
    for (i, b) in blocks.iter().enumerate() {
        let r = BlockRec::of(b);
        // SAFETY: BlockRec is repr(C), Copy and all-integer, so its bytes are
        // its representation and its padding is explicit.
        let src =
            unsafe { std::slice::from_raw_parts(&r as *const BlockRec as *const u8, BLK_ENTRY) };
        let at = BLK_HEADER + i * BLK_ENTRY;
        out[at..at + BLK_ENTRY].copy_from_slice(src);
        if let Some(row) = chunk_crcs.get(i) {
            for (j, c) in row.iter().enumerate() {
                let at = crcs_off + i * CRC_ROW + j * 4;
                out[at..at + 4].copy_from_slice(&c.to_le_bytes());
            }
        }
    }
    out
}

/// A validated view of a mapped block table: where the entries are, and
/// nothing copied out of them.
///
/// Offsets rather than a borrow, so `Reader` -- which owns the mapping -- does
/// not become self-referential. Same shape as `FlatIndex`.
#[derive(Clone, Copy)]
pub struct MappedBlocks {
    off: usize,
    n: usize,
}

/// Is this section the flat block table rather than the varint one?
///
/// The reader used to decide by "uncompressed, so probably flat", and fall
/// back to the varint decoder for anything the flat parser refused. That
/// fallback cannot tell "this is the older format" from "this is the newer
/// format and I would not read it", so it answered a scan of a perfectly good
/// store with 83 blocks where the table held 91 -- a wrong answer, not an
/// error. The magic is the discriminator; whether to *map* it is a separate
/// question.
pub fn is_block_section(sec: &[u8]) -> bool {
    rd_ne_u32(sec, 0) == Some(BLK_MAGIC)
}

/// The flat block table copied out into an owned Vec.
///
/// The same bytes `MappedBlocks` borrows, so the two are a fair pair to
/// measure against each other: one open that copies, against a borrow that
/// revalidates per access.
pub fn decode_blocks(sec: &[u8]) -> Option<Vec<crate::block::BlockLoc>> {
    let meta = MappedBlocks::parse(sec)?;
    let mut out = Vec::with_capacity(meta.len());
    for i in 0..meta.len() {
        out.push(meta.get(sec, i)?);
    }
    Some(out)
}

impl MappedBlocks {
    /// `None` for the varint format, for an entry size this build disagrees
    /// with, and for bytes the mapping did not land aligned -- each a reason to
    /// fall back to the decoder rather than to fail.
    pub fn parse(sec: &[u8]) -> Option<MappedBlocks> {
        if rd_u32(sec, 0)? != BLK_MAGIC || rd_u32(sec, 4)? as usize != BLK_ENTRY {
            return None;
        }
        let n = rd_u64(sec, 8)? as usize;
        let bytes = sec.get(BLK_HEADER..BLK_HEADER.checked_add(n.checked_mul(BLK_ENTRY)?)?)?;
        if !(bytes.as_ptr() as usize).is_multiple_of(std::mem::align_of::<BlockRec>()) {
            return None;
        }
        Some(MappedBlocks { off: BLK_HEADER, n })
    }

    pub fn len(&self) -> usize {
        self.n
    }

    /// The entry at `i`, read out of `sec`.
    ///
    /// Bounds are re-checked against the slice rather than trusted from
    /// `parse`: the section comes from a mapping a corruption experiment
    /// damages, and a shorter slice must be rejected rather than indexed.
    #[inline]
    /// The stored checksum of chunk `j` of block `i`.
    ///
    /// `None` when the block has no per-chunk checksums, when the section is
    /// too short to hold the row, or when the chunk is out of range -- each a
    /// reason to fall back to the whole-block checksum rather than to accept
    /// an unverified read.
    pub fn chunk_crc(&self, sec: &[u8], i: usize, j: usize) -> Option<u32> {
        if i >= self.n || j >= crate::block::MAX_CHUNK_CRCS {
            return None;
        }
        let base = self.off.checked_add(self.n.checked_mul(BLK_ENTRY)?)?;
        let at = base
            .checked_add(i.checked_mul(CRC_ROW)?)?
            .checked_add(j * 4)?;
        rd_u32(sec, at)
    }

    pub fn get(&self, sec: &[u8], i: usize) -> Option<crate::block::BlockLoc> {
        if i >= self.n {
            return None;
        }
        let at = self.off.checked_add(i.checked_mul(BLK_ENTRY)?)?;
        let bytes = sec.get(at..at.checked_add(BLK_ENTRY)?)?;
        if !(bytes.as_ptr() as usize).is_multiple_of(std::mem::align_of::<BlockRec>()) {
            return None;
        }
        // SAFETY: length and alignment checked here; BlockRec is repr(C), Copy
        // and all-integer, so every bit pattern of these bytes is a valid value.
        let r = unsafe { *(bytes.as_ptr() as *const BlockRec) };
        Some(r.to_loc())
    }
}

#[cfg(test)]
mod block_tests {
    /// One distinguishable checksum row per block, so a test can tell a row
    /// read from the right block from one read from its neighbour.
    /// Per-chunk checksums must come back out where they went in.
    ///
    /// `Store::open` reads them from this section to rebuild the appender's
    /// copy and writes them out again at the next checkpoint. If they came
    /// back zeroed, the flag saying a block has them would still be set, and
    /// every reader afterwards would verify live data against zero.
    #[test]
    fn chunk_checksums_round_trip() {
        let blocks: Vec<crate::block::BlockLoc> = (0..40)
            .map(|i| crate::block::BlockLoc {
                off: 4096 * (i as u64 + 1),
                stored: 1000,
                uncompressed: 1000,
                cap: 4096,
                chunked: false,
                solo: false,
                chunk_crc: true,
                crc: 7 + i,
            })
            .collect();
        let rows = crc_rows(blocks.len());
        let sec = encode_blocks(&blocks, &rows);
        let meta = MappedBlocks::parse(&sec).expect("parse");
        assert_eq!(meta.len(), blocks.len());
        for (i, row) in rows.iter().enumerate() {
            assert!(
                meta.get(&sec, i).expect("block").chunk_crc,
                "flag lost at {i}"
            );
            for (j, want) in row.iter().enumerate() {
                assert_eq!(
                    meta.chunk_crc(&sec, i, j),
                    Some(*want),
                    "block {i} chunk {j}"
                );
            }
        }
    }

    fn crc_rows(n: usize) -> Vec<[u32; crate::block::MAX_CHUNK_CRCS]> {
        (0..n)
            .map(|i| {
                let mut row = [0u32; crate::block::MAX_CHUNK_CRCS];
                for (j, c) in row.iter_mut().enumerate() {
                    *c = (i as u32) << 8 | j as u32;
                }
                row
            })
            .collect()
    }

    use super::*;
    use crate::block::BlockLoc;

    fn corpus(n: usize) -> Vec<BlockLoc> {
        (0..n)
            .map(|i| BlockLoc {
                chunk_crc: i % 3 == 0,
                off: (i as u64) * 4096 + 7,
                stored: i as u32 * 3,
                uncompressed: i as u32 * 5,
                cap: i as u32 * 7,
                crc: i as u32 * 11,
                solo: i % 3 == 0,
                chunked: i % 5 == 0,
            })
            .collect()
    }

    #[test]
    fn every_block_round_trips() {
        let blocks = corpus(5000);
        let sec = encode_blocks(&blocks, &crc_rows(blocks.len()));
        let m = MappedBlocks::parse(&sec).expect("parse");
        assert_eq!(m.len(), blocks.len());
        for (i, b) in blocks.iter().enumerate() {
            let g = m.get(&sec, i).expect("present");
            assert_eq!(
                (
                    g.off,
                    g.stored,
                    g.uncompressed,
                    g.cap,
                    g.crc,
                    g.solo,
                    g.chunked
                ),
                (
                    b.off,
                    b.stored,
                    b.uncompressed,
                    b.cap,
                    b.crc,
                    b.solo,
                    b.chunked
                ),
                "block {i}"
            );
        }
        assert!(m.get(&sec, blocks.len()).is_none());
    }

    /// Why `BlockRec` exists rather than mapping `BlockLoc` directly: a damaged
    /// flag byte must produce a wrong answer, never a `bool` holding a value
    /// the compiler is entitled to assume impossible.
    #[test]
    fn damage_never_produces_an_invalid_bool() {
        let blocks = corpus(200);
        let sec = encode_blocks(&blocks, &crc_rows(blocks.len()));
        for i in 0..sec.len() {
            let mut d = sec.clone();
            d[i] ^= 0xff;
            if let Some(m) = MappedBlocks::parse(&d) {
                for j in 0..m.len().min(200) {
                    if let Some(b) = m.get(&d, j) {
                        std::hint::black_box((b.solo, b.chunked, b.off));
                    }
                }
            }
        }
    }

    #[test]
    fn truncation_is_rejected_not_misread() {
        let blocks = corpus(100);
        let sec = encode_blocks(&blocks, &crc_rows(blocks.len()));
        for cut in 0..sec.len() {
            if let Some(m) = MappedBlocks::parse(&sec[..cut]) {
                for j in 0..m.len() {
                    let _ = m.get(&sec[..cut], j);
                }
            }
        }
    }
}
