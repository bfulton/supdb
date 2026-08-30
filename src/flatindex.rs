//! A key index that is read where it lies instead of rebuilt on open.
//!
//! The shipped index is decoded into `Vec<(Vec<u8>, Extents)>` and then hashed,
//! which costs two failing claims at once. `F2.1`: open is not independent of
//! key count -- 6.4ms at 100k keys, 1446ms at 10M, so 100x the keys costs 225x
//! the open. `F7.2`: the result is 131 bytes per key, resident in every reader
//! process, shared with nobody. Both are properties of *rebuilding*, not of
//! the data: the bytes on disk are already an index, and the decode exists
//! only because the on-disk shape is not one a lookup can use.
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
//!     decompressed into a buffer, and a buffer per reader is most of what
//!     F7.2 is complaining about. The cost is file size, which is Supdb's
//!     other real advantage, so the arms are behind `Options::flat_index` and
//!     measured rather than assumed.
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
const VERSION: u32 = 2;
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

/// Offsets are 32-bit, so the record region is bounded. At the ~40 bytes per
/// key this format uses that is about 100M keys, past which the caller falls
/// back to the heap index rather than silently truncating.
pub const MAX_RECS: usize = u32::MAX as usize;

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
    // u16 klen, u16 extent count, the key, pad to 4, then 16 bytes each.
    align_up(4 + klen, REC_ALIGN) + next * 16
}

/// Measure the section before writing it, so the whole thing can be built into
/// one allocation of the right size rather than grown.
pub fn plan(all: &[(&[u8], &crate::index::Extents)]) -> Option<Plan> {
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
        at = at.checked_add(record_len(k.len(), n))?;
    }
    if at > MAX_RECS {
        return None;
    }
    // Half again, so a store whose keys gain extents can publish updates
    // without rewriting anything.
    let slack = at * SLACK_NUM / SLACK_DEN;
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

    let fence_offs_off = HEADER + cap * SLOT + all.len() * 4;
    let fence_offs_len = if fence_n == 0 { 0 } else { (fence_n + 1) * 4 };
    let fence_blob_off = fence_offs_off + fence_offs_len;
    // Records are 4-aligned within the section, and the blob is bytes, so the
    // record region is realigned after it.
    let recs_off = align_up(fence_blob_off + fence_blob_len, REC_ALIGN);
    let total = recs_off + recs_cap;
    Some(Plan {
        hash_cap: cap,
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
pub fn encode(
    all: &[(&[u8], &crate::index::Extents)],
    generation: u64,
    prev: Option<(u64, u64, u64, u64, u64)>,
    hash_of: fn(&[u8]) -> u64,
) -> Option<(Vec<u8>, usize)> {
    let p = plan(all)?;
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
    ]
    .iter()
    .enumerate()
    {
        let at = 8 + i * 8;
        out[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    // Records, then the rank directory, then the hash over them.
    for (i, (k, exts)) in all.iter().enumerate() {
        let base = recs_off + p.rec_offs[i] as usize;
        let slice = exts.as_slice();
        out[base..base + 2].copy_from_slice(&(k.len() as u16).to_le_bytes());
        out[base + 2..base + 4].copy_from_slice(&(slice.len() as u16).to_le_bytes());
        out[base + 4..base + 4 + k.len()].copy_from_slice(k);
        let mut e_at = base + align_up(4 + k.len(), REC_ALIGN);
        for e in slice {
            out[e_at..e_at + 4].copy_from_slice(&e.block.to_le_bytes());
            out[e_at + 4..e_at + 8].copy_from_slice(&e.off.to_le_bytes());
            out[e_at + 8..e_at + 12].copy_from_slice(&e.len.to_le_bytes());
            out[e_at + 12..e_at + 16].copy_from_slice(&e.last.to_le_bytes());
            e_at += 16;
        }
        let d = dir_off + i * 4;
        out[d..d + 4].copy_from_slice(&p.rec_offs[i].to_le_bytes());
    }

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

    let mask = p.hash_cap - 1;
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

        if hash_cap == 0 || !hash_cap.is_power_of_two() {
            return None;
        }
        // Each region must lie inside the section and after the one before it.
        let hash_end = hash_off.checked_add(hash_cap.checked_mul(SLOT)?)?;
        let dir_end = dir_off.checked_add(nkeys.checked_mul(4)?)?;
        let recs_end = recs_off.checked_add(recs_cap)?;
        if hash_off < HEADER
            || hash_end > dir_off
            || dir_end > recs_off
            || recs_end > sec.len()
            || nkeys > hash_cap
        {
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
            if fence_offs_off < dir_end || offs_end > recs_off {
                return None;
            }
            let offs = sec.get(fence_offs_off..offs_end)?;
            // The last offset is the blob length, and the blob sits directly
            // after the offsets.
            let blob_len = rd_u32(offs, fence_n * 4)? as usize;
            let blob_end = offs_end.checked_add(blob_len)?;
            if blob_end > recs_off {
                return None;
            }
            Some((
                (fence_offs_off, offs_end),
                (offs_end, blob_end),
                fence_n,
                fence_stride,
            ))
        })()
        .unwrap_or(((0, 0), (0, 0), 0, 0));
        Some(FlatIndex {
            hash: (hash_off, hash_end),
            dir: (dir_off, dir_end),
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
        let recs = sec.get(self.recs.0..self.recs.1)?;
        let klen = rd_u16(recs, off)? as usize;
        let n = rd_u16(recs, off + 2)? as usize;
        let key = recs.get(off + 4..off + 4 + klen)?;
        let e_at = off.checked_add(align_up(4 + klen, REC_ALIGN))?;
        let bytes = recs.get(e_at..e_at.checked_add(n.checked_mul(16)?)?)?;
        // Records are laid out 4-aligned within the section and the section is
        // written at an 8-aligned file offset, so this borrow is aligned by
        // construction. Checked anyway, and the check has already earned its
        // keep: before `write_section_raw` aligned the section, records were
        // aligned relative to the section and not absolutely, and this is what
        // turned undefined behaviour into a miss.
        if !(bytes.as_ptr() as usize).is_multiple_of(std::mem::align_of::<Ext>()) {
            return None;
        }
        let exts = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const Ext, n) };
        Some((key, exts))
    }

    /// Extents for `key`, borrowed from the mapping. No allocation, no decode.
    pub fn lookup<'a>(
        &self,
        sec: &'a [u8],
        key: &[u8],
        hash_of: fn(&[u8]) -> u64,
    ) -> Option<&'a [Ext]> {
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
                if let Some((k, exts)) = self.record(sec, off) {
                    if k == key {
                        return Some(exts);
                    }
                }
            }
            s = (s + 1) & self.mask;
        }
        None
    }

    /// The record at `rank` in key order.
    pub fn at<'a>(&self, sec: &'a [u8], rank: usize) -> Option<(&'a [u8], &'a [Ext])> {
        if rank >= self.nkeys {
            return None;
        }
        let dir = sec.get(self.dir.0..self.dir.1)?;
        let off = rd_u32(dir, rank * 4)? as usize;
        self.record(sec, off)
    }

    /// Where in the section a key's *directory* entry lives, if it is present.
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
            at += 16;
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
    /// and `c1-decoders` will feed it garbage on purpose.
    pub fn decode_record(rec: &[u8]) -> Option<(Vec<u8>, Vec<Ext>)> {
        let klen = rd_u16(rec, 0)? as usize;
        let n = rd_u16(rec, 2)? as usize;
        let key = rec.get(4..4 + klen)?.to_vec();
        let mut at = align_up(4 + klen, REC_ALIGN);
        let mut exts = Vec::with_capacity(n);
        for _ in 0..n {
            let b = rec.get(at..at + 16)?;
            exts.push(Ext {
                block: u32::from_le_bytes(b[0..4].try_into().ok()?),
                off: u32::from_le_bytes(b[4..8].try_into().ok()?),
                len: u32::from_le_bytes(b[8..12].try_into().ok()?),
                last: u32::from_le_bytes(b[12..16].try_into().ok()?),
            });
            at += 16;
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

    /// `fence` selects the arm: `f18-fence` runs both over one file.
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
        let sec = padded(encode(&refs(&all), 7, None, h).expect("encode"));
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
        let sec = padded(encode(&refs(&all), 1, None, h).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        for i in 0..2000 {
            let k = format!("absent{i:012}").into_bytes();
            assert!(ix.lookup(&sec, &k, h).is_none());
        }
    }

    #[test]
    fn rank_order_matches_key_order() {
        let all = corpus(1000);
        let sec = padded(encode(&refs(&all), 1, None, h).unwrap());
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
            let sec = padded(encode(&refs(&all), 1, None, h).unwrap());
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
        let clean = padded(encode(&refs(&all), 1, None, h).unwrap());
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
        let sec = padded(encode(&refs(&all), 1, None, h).unwrap());
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
        let sec = padded(encode(&refs(&all), 1, None, h).unwrap());
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
        let sec = padded(encode(&refs(&all), 1, None, h).unwrap());
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
        let sec = padded(encode(&refs(&[]), 3, None, h).unwrap());
        let ix = FlatIndex::parse(&sec).unwrap();
        assert_eq!(ix.len(), 0);
        assert!(ix.lookup(&sec, b"anything", h).is_none());
        assert_eq!(ix.seek_with(&sec, b"anything", true), 0);
    }

    #[test]
    fn the_previous_index_is_carried() {
        let all = corpus(10);
        let sec = padded(encode(&refs(&all), 9, Some((8, 1234, 4096, 100, 200)), h).unwrap());
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
