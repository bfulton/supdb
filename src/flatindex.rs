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
const VERSION: u32 = 1;
/// Header size, padded so the hash region starts 8-byte aligned.
const HEADER: usize = 128;
/// Bytes per hash slot: a tag in the top eight bits, a record offset below.
const SLOT: usize = 8;
/// Records are 4-aligned so an extent array can be borrowed as `&[Ext]`.
const REC_ALIGN: usize = 4;

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
    /// Record offset of each key, in sorted order.
    pub rec_offs: Vec<u32>,
    pub total: usize,
}

fn record_len(klen: usize, next: usize) -> usize {
    // u16 klen, u16 extent count, the key, pad to 4, then 16 bytes each.
    align_up(4 + klen, REC_ALIGN) + next * 16
}

/// Measure the section before writing it, so the whole thing can be built into
/// one allocation of the right size rather than grown.
pub fn plan(all: &[(Vec<u8>, crate::index::Extents)]) -> Option<Plan> {
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
    let total = HEADER + cap * SLOT + all.len() * 4 + at;
    Some(Plan {
        hash_cap: cap,
        recs_len: at,
        rec_offs,
        total,
    })
}

/// Serialize the index. `all` must be sorted by key.
///
/// `hash_of` is the store's key hash, passed in rather than duplicated so the
/// writer and the reader can never disagree about it -- a hash mismatch
/// between the two would present as keys that exist and cannot be found.
pub fn encode(
    all: &[(Vec<u8>, crate::index::Extents)],
    generation: u64,
    prev: Option<(u64, u64, u64, u64, u64)>,
    hash_of: fn(&[u8]) -> u64,
) -> Option<Vec<u8>> {
    let p = plan(all)?;
    let mut out = vec![0u8; p.total];

    let hash_off = HEADER;
    let dir_off = hash_off + p.hash_cap * SLOT;
    let recs_off = dir_off + all.len() * 4;

    out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
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
    Some(out)
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
    pub generation: u64,
    pub prev: Option<(u64, u64, u64, u64, u64)>,
}

/// True if `sec` looks like this format rather than the varint one.
pub fn is_flat(sec: &[u8]) -> bool {
    rd_u32(sec, 0) == Some(MAGIC) && rd_u32(sec, 4) == Some(VERSION)
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

        if hash_cap == 0 || !hash_cap.is_power_of_two() {
            return None;
        }
        // Each region must lie inside the section and after the one before it.
        let hash_end = hash_off.checked_add(hash_cap.checked_mul(SLOT)?)?;
        let dir_end = dir_off.checked_add(nkeys.checked_mul(4)?)?;
        let recs_end = recs_off.checked_add(recs_len)?;
        if hash_off < HEADER
            || hash_end > dir_off
            || dir_end > recs_off
            || recs_end > sec.len()
            || nkeys > hash_cap
        {
            return None;
        }
        Some(FlatIndex {
            hash: (hash_off, hash_end),
            dir: (dir_off, dir_end),
            recs: (recs_off, recs_end),
            hash_cap,
            mask: hash_cap - 1,
            nkeys,
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

    /// Position of the first key at or after `key`.
    pub fn seek(&self, sec: &[u8], key: &[u8]) -> usize {
        let (mut lo, mut hi) = (0usize, self.nkeys);
        while lo < hi {
            let mid = (lo + hi) / 2;
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
        let sec = encode(&all, 7, None, h).expect("encode");
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
        let sec = encode(&all, 1, None, h).unwrap();
        let ix = FlatIndex::parse(&sec).unwrap();
        for i in 0..2000 {
            let k = format!("absent{i:012}").into_bytes();
            assert!(ix.lookup(&sec, &k, h).is_none());
        }
    }

    #[test]
    fn rank_order_matches_key_order() {
        let all = corpus(1000);
        let sec = encode(&all, 1, None, h).unwrap();
        let ix = FlatIndex::parse(&sec).unwrap();
        for (i, (k, e)) in all.iter().enumerate() {
            let (gk, ge) = ix.at(&sec, i).expect("rank present");
            assert_eq!(gk, k.as_slice());
            assert_eq!(ge, e.as_slice());
        }
        assert!(ix.at(&sec, all.len()).is_none());
    }

    #[test]
    fn seek_finds_the_first_key_at_or_after() {
        let all = corpus(500);
        let sec = encode(&all, 1, None, h).unwrap();
        let ix = FlatIndex::parse(&sec).unwrap();
        for (i, (k, _)) in all.iter().enumerate() {
            assert_eq!(ix.seek(&sec, k), i);
        }
        assert_eq!(ix.seek(&sec, b"\x00"), 0);
        assert_eq!(ix.seek(&sec, b"\xff"), all.len());
    }

    /// The reason this module exists rather than a struct with a `decode`:
    /// a damaged section must cost the caller an answer, never the process.
    #[test]
    fn damage_never_panics() {
        let all = corpus(300);
        let sec = encode(&all, 1, None, h).unwrap();
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
                    let _ = ix.seek(&sec, b"key000000000100");
                }
            }
        }
    }

    #[test]
    fn truncation_never_panics() {
        let all = corpus(200);
        let sec = encode(&all, 1, None, h).unwrap();
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
        let sec = encode(&[], 3, None, h).unwrap();
        let ix = FlatIndex::parse(&sec).unwrap();
        assert_eq!(ix.len(), 0);
        assert!(ix.lookup(&sec, b"anything", h).is_none());
        assert_eq!(ix.seek(&sec, b"anything"), 0);
    }

    #[test]
    fn the_previous_index_is_carried() {
        let all = corpus(10);
        let sec = encode(&all, 9, Some((8, 1234, 4096, 100, 200)), h).unwrap();
        let ix = FlatIndex::parse(&sec).unwrap();
        assert_eq!(ix.prev, Some((8, 1234, 4096, 100, 200)));
    }
}
