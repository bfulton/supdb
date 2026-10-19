//! The z form's engine side: the sidecar mapped and checked (`ZSide`), the
//! cell a partition holds it in (`ZCell`), the read of a key through it, and
//! the builder that writes one from a partition's own bytes (`build`).
//!
//! The layout is `zpages`, which is the one definition of a page, a trailer
//! and a fence and has nothing to do with a file or a thread. This is
//! everything that does: it maps, it checks what it maps against the segment
//! it was made from, and it answers a read from a page or declines to.
//!
//! # What a read promises
//!
//! A partition's point read or count, answered here, is the raw form's
//! answer or no answer: a read the sidecar cannot serve says so
//! ([`ZRead::Fallback`], [`ZRead::Out`]) and the caller reads the raw form
//! and counts it. Nothing that fails a check is ever answered as an absent
//! key, because a group that cannot be read is damage and an absent key is a
//! fact about the data; the first is counted and the second is not.
//!
//! The callback a read hands its values to is called only after every check
//! that can fail has passed, so a read that falls back has emitted nothing
//! and the raw form's read does not repeat a value.
//!
//! # What the cell promises
//!
//! A sidecar is installed once and never replaced or cleared before its
//! segment is dropped, so a reader holding the segment holds the sidecar.
//! A cell that could be cleared or replaced would have to free the sidecar it
//! held through the epoch, and `ZCell::get` would change with it.

use super::zpages::{
    bind32, check_entry, check_group, decode_group, fill_group, group_seek, page_view, parse_page,
    verify_page, FenceEntry, Fill, FillScratch, FillStats, GroupBuf, Images, NoPage, SegImages,
    Trailer, ZError, ZFence, ZNOPAGE, ZPAGE, ZTRAILER,
};
use crate::blob::{Blob, BlobOptions, ZSource};
use crate::bytes::{Bytes, MmapBytes};
use crate::flatindex::{parse_record, rd_u32, record_key};
use crate::index::{Ext, Exts};
use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Result, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

/// The size floor: a partition whose record region is under this many bytes
/// is declined and no file is written (`Options::zpages_min_bytes`). The form
/// takes at most `(1 - 0.59)` of the pages it covers out of the cache, and a
/// partition of 512 pages has 210 of them to give, 0.86 MB; sixteen sampled
/// pages at attach and the build's own passes are a larger share of a file
/// that small than the saving is. A size chosen before the thing was
/// measured: a wrong value costs a file or the saving of a few hundred
/// pages, never a fault, and `declined_small_bytes` says how much of the
/// store it left out.
pub(super) const ZMIN_RECS: usize = 2 << 20;

/// The sidecar is written in pieces of this many bytes at their own
/// alignment, as a segment is (`SegmentOptions::write_piece`): the page
/// cache sizes a folio by the write that makes it, and what the folios of a
/// file written this way compose to is not measured here.
const ZWRITE_PIECE: usize = 1 << 20;

/// The window of the raw records the build drops behind itself.
const ZDROP_WINDOW: usize = 8 << 20;

/// Pages whose checksums an attach reads: the first, the last and the ones
/// evenly spaced between, sixteen faults a file, once.
const ZSAMPLE_PAGES: usize = 16;

/// The sidecar's name: the two fields of the segment's name that a
/// promotion keeps, as the ordered index's (`Db::ord_name`), so a
/// promotion's link names no new file.
pub(super) fn zpg_name(id: u64, end_seq: u64) -> String {
    format!("zpg-{id:08}-{end_seq:016}.zpg")
}

/// The two fields of the segment named `seg` that its sidecar's name is
/// made of, or `None` for a name that is not a segment's. Two segments with
/// the same fields address one sidecar -- a promotion keeps them -- and
/// comparing the fields is the claim test, with no name made to compare.
pub(super) fn zpg_fields(seg: &str) -> Option<(u64, u64)> {
    Some((super::Db::name_id(seg)?, super::Db::name_end_seq(seg)?))
}

/// The sidecar a segment named `seg` would have, or `None` for a name that
/// is not a segment's.
pub(super) fn zpg_name_for(seg: &str) -> Option<String> {
    let (id, end_seq) = zpg_fields(seg)?;
    Some(zpg_name(id, end_seq))
}

/// Whether `name` is one of the sidecar's files or a build's temp name:
/// what a sweep of a store's directory takes for the form's.
pub(super) fn is_zpg_file(name: &str) -> bool {
    name.starts_with("zpg-")
}

/// Why an attach refused a file.
#[derive(Debug)]
pub(super) enum Reject {
    /// There is no file. Not a rejection: nothing was there to refuse.
    Missing,
    /// A file that is not a sidecar anyone can use: short, torn, a trailer
    /// or a resident part or a sampled page that does not check.
    Damage(String),
    /// A sidecar, whole, of something else: another segment, a record
    /// format this engine does not lay out, an engine whose layout differs.
    Identity(String),
}

/// What a read of a covered partition answered.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ZRead {
    /// The sidecar answered: `n` values (or the count), and whether a page
    /// was decoded to get there. A page is decoded for every key that falls
    /// in its range, the key it holds and the key it does not alike; a
    /// stored page is parsed where it lies and decodes nothing, and a key
    /// below the partition's first key is answered from the fence, touching
    /// no page.
    Served { n: u64, decoded: bool },
    /// A check failed on this read: the caller reads the raw form and
    /// counts a fallback.
    Fallback,
    /// The sidecar is out of service: the caller reads the raw form and
    /// counts it as served raw.
    Out,
}

/// Where a key lies in a sidecar, found or not, with the bytes it lies in.
enum Locate<'a> {
    /// The key is not in this partition: sorts below its first key, or
    /// among the records of the page it would be in, which was decoded to
    /// find that out unless it is stored or there was no page to open.
    Absent { decoded: bool },
    /// The record: its extents, its tail, and whether a page was decoded.
    Found {
        exts: Exts<'a>,
        tail: &'a [u8],
        decoded: bool,
    },
    /// A check failed.
    Fallback,
    /// Out of service.
    Out,
}

/// Why a page was not opened.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Fail {
    /// A check of the layout failed: the page is damaged, or is not the
    /// page its fence entry names.
    Check(ZError),
    /// A stored page's payload is not aligned for the extents
    /// `parse_record` borrows in place. A mapping never hands one out; a
    /// buffer that does is refused, since a record read from it is `None`
    /// and a probe loop reads `None` as an absent key.
    Misaligned,
}

impl From<ZError> for Fail {
    fn from(e: ZError) -> Fail {
        Fail::Check(e)
    }
}

/// Whether a stored payload can be parsed where it lies.
fn aligned_for_extents(payload: &[u8]) -> bool {
    (payload.as_ptr() as usize).is_multiple_of(std::mem::align_of::<Ext>())
}

/// A page's group at its first decode in this process: every check the
/// layout has, in the order that trusts the least first (the CRC, the tag,
/// the header, the zeros; the rank and the count against the fence entry
/// that names the page; the payload decoded to exactly `ulen`; the walk of
/// `count` whole records in ascending order, the first the entry's key),
/// and the group. Returned with its record count and whether a decode was
/// made. A stored page is parsed in place and only where its payload is
/// aligned.
///
/// These are `zpages::verify_page`'s checks, which the builder runs on
/// every page it writes, in the same order and through the same functions
/// (`check_entry`, `check_group`), with the one thing a mapping adds between
/// them, the alignment of a stored payload. They are not spelled twice: the
/// builder's guarantee that no page it writes fails a first decode holds
/// only while the two are one definition.
pub(super) fn first_decode<'a>(
    page: &'a [u8],
    bind: u32,
    entry: &FenceEntry<'_>,
    buf: &'a mut GroupBuf,
) -> std::result::Result<(&'a [u8], usize, bool), Fail> {
    let (h, payload) = parse_page(page, bind)?;
    check_entry(&h, entry)?;
    if h.stored && !aligned_for_extents(payload) {
        return Err(Fail::Misaligned);
    }
    let group = decode_group(&h, payload, buf)?;
    check_group(group, &h, entry)?;
    Ok((group, usize::from(h.count), !h.stored))
}

/// A verified page's group: the header's values and the decode, and nothing
/// the first decode already settled.
pub(super) fn verified_group<'a>(
    page: &'a [u8],
    buf: &'a mut GroupBuf,
) -> std::result::Result<(&'a [u8], usize, bool), Fail> {
    let (h, payload) = page_view(page)?;
    if h.stored && !aligned_for_extents(payload) {
        return Err(Fail::Misaligned);
    }
    let group = decode_group(&h, payload, buf)?;
    Ok((group, usize::from(h.count), !h.stored))
}

thread_local! {
    /// The buffer a group decodes into: the thread's, taken out of its cell
    /// for a read and put back after, so that a callback that reads the
    /// store again finds the cell empty and makes a buffer of its own
    /// instead of sharing the one it is called inside (`Blob`'s scratch is
    /// the same). A thread that reads a covered partition keeps one for its
    /// life, as large as the largest group it has decoded in whole pages
    /// (`GroupBuf`) -- a few kilobytes for the records the suite writes, and
    /// at most a group's cap.
    static ZBUF: Cell<Option<GroupBuf>> = const { Cell::new(None) };
}

fn take_buf() -> GroupBuf {
    ZBUF.with(|c| c.take()).unwrap_or_default()
}

fn put_buf(b: GroupBuf) {
    ZBUF.with(|c| c.set(Some(b)));
}

fn bit(map: &[AtomicU64], p: usize) -> bool {
    map[p >> 6].load(Ordering::Relaxed) & (1u64 << (p & 63)) != 0
}

fn set_bit(map: &[AtomicU64], p: usize) {
    map[p >> 6].fetch_or(1u64 << (p & 63), Ordering::Relaxed);
}

fn popcount(map: &[AtomicU64]) -> u64 {
    map.iter()
        .map(|w| u64::from(w.load(Ordering::Relaxed).count_ones()))
        .sum()
}

/// Sidecar ids, one per attach: what a cache derived from one sidecar is
/// keyed by, so it cannot be read against another over the same range.
static NEXT_ZSIDE_ID: AtomicU64 = AtomicU64::new(1);

/// A sidecar, attached: its file mapped read-only, the fence and the
/// binding in the heap, and what this process has verified of its pages.
pub(super) struct ZSide {
    /// The file, mapped. Advised `MADV_RANDOM` at attach whenever the store
    /// advises its mappings at all (`ReadAdvice` other than `Normal`), and
    /// then left alone, as the ordered companion's mapping is, whatever
    /// mode the store is in at that moment or later: only point reads and
    /// counts read a sidecar -- the scans of a covered partition are walked
    /// raw -- and a point read is random in every phase of the store, so
    /// `Reader::advise`, which follows the workload over the segments, does
    /// not walk this. The stage that makes scans read sidecars is the one
    /// to decide what their advice is in a scan phase.
    map: MmapBytes,
    /// The mapping was advised random (`ZSide::advised`).
    advised: bool,
    /// Heap copies of the resident part's tables, never evicted.
    fence: ZFence,
    nkeys: usize,
    n_pages: usize,
    /// The hash every page's CRC is xored with, recomputed from the live
    /// segment and not taken from the file.
    bind: u32,
    /// One bit per page: the first-decode checks passed in this process.
    verified: Box<[AtomicU64]>,
    /// One bit per page: a check failed.
    bad: Box<[AtomicU64]>,
    /// Any check failed: the sidecar is out of service, and reads of the
    /// partition go to the raw form until it is dropped.
    damaged: AtomicBool,
    id: u64,
    /// The trailer this attach accepted.
    ident: Trailer,
    /// A test's: the file's bytes in a buffer of the test's own, read in
    /// place of the mapping from `skew` bytes in, so that a page can sit at
    /// an address a mapping never gives it.
    #[cfg(test)]
    skewed: Option<(Vec<u8>, usize)>,
}

impl ZSide {
    /// The sidecar at `path`, if it is the sidecar of the live segment
    /// `blob`, whose name carries `id` and `end_seq`. The checks, in order:
    /// the file's length and its trailer (every field the trailer's own
    /// decode checks); the trailer against what the live segment recomputes
    /// of itself -- its key count, record region, checksum row's place and
    /// CRC, hash capacity, file length, timestamp, generation, record
    /// version, file magic, and the name's two fields, with `bind32`
    /// recomputed from them; the resident part's CRC and the fence's
    /// tiling, from the resident part alone; and the first, the last and
    /// fourteen evenly spaced pages' CRCs and headers, against the fence
    /// entries that name them. A page body beyond those sixteen is checked
    /// at its first decode.
    ///
    /// The filter a trailer may carry is not read: none is built, and no read
    /// is answered from one.
    ///
    /// `advise` maps the file `MADV_RANDOM`, once checked.
    pub(super) fn attach(
        path: &Path,
        blob: &Blob<MmapBytes>,
        id: u64,
        end_seq: u64,
        advise: bool,
    ) -> std::result::Result<ZSide, Reject> {
        let damage = |e: ZError| Reject::Damage(e.to_string());
        let map = match MmapBytes::open(path) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(Reject::Missing),
            Err(e) => return Err(Reject::Damage(format!("the file does not open: {e}"))),
        };
        let len = map.len();
        if len < ZTRAILER as u64 || !len.is_multiple_of(ZPAGE as u64) {
            return Err(damage(ZError::Length));
        }
        let t = Trailer::decode(
            map.slice_at(len - ZTRAILER as u64, ZTRAILER)
                .ok_or_else(|| damage(ZError::Length))?,
        )
        .map_err(damage)?;
        if t.expected_file_len() != Some(len) {
            return Err(damage(ZError::Trailer(
                "the file is not the length its trailer says",
            )));
        }
        let facts = blob.z_source_facts().ok_or_else(|| {
            Reject::Identity("the segment has no checksum row to be bound to".into())
        })?;
        same_segment(&t, &facts, id, end_seq).map_err(|why| Reject::Identity(why.into()))?;
        let nkeys = usize::try_from(facts.nkeys)
            .ok()
            .filter(|&n| u32::try_from(n).is_ok())
            .ok_or_else(|| Reject::Identity("more keys than a fence counts".into()))?;
        let n_pages = usize::try_from(t.n_pages).map_err(|_| damage(ZError::Length))?;
        let res_len = (len - ZTRAILER as u64)
            .checked_sub(t.res_off)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| damage(ZError::Length))?;
        let res = map
            .slice_at(t.res_off, res_len)
            .ok_or_else(|| damage(ZError::Length))?;
        let (fence, _filter) = ZFence::decode_resident(res, &t).map_err(damage)?;
        let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
        let side = ZSide {
            map,
            advised: advise,
            fence,
            nkeys,
            n_pages,
            bind,
            verified: (0..n_pages.div_ceil(64))
                .map(|_| AtomicU64::new(0))
                .collect(),
            bad: (0..n_pages.div_ceil(64))
                .map(|_| AtomicU64::new(0))
                .collect(),
            damaged: AtomicBool::new(false),
            id: NEXT_ZSIDE_ID.fetch_add(1, Ordering::Relaxed),
            ident: t,
            #[cfg(test)]
            skewed: None,
        };
        side.check_sample().map_err(damage)?;
        if advise {
            side.map.advise_random();
        }
        Ok(side)
    }

    /// The first, the last and fourteen evenly spaced pages through
    /// `parse_page` -- the CRC under this sidecar's `bind32`, the tag, the
    /// header, the padding -- and their rank and count against the fence
    /// entry that names each. Every page where there are no more than
    /// sixteen. These are not first decodes: nothing is decoded and no page
    /// is marked verified.
    fn check_sample(&self) -> std::result::Result<(), ZError> {
        let take = self.n_pages.min(ZSAMPLE_PAGES);
        let mut want: Vec<usize> = (0..take)
            .map(|k| {
                if self.n_pages <= ZSAMPLE_PAGES {
                    k
                } else {
                    k * (self.n_pages - 1) / (ZSAMPLE_PAGES - 1)
                }
            })
            .collect();
        want.dedup();
        for i in 0..self.fence.len() {
            let Some(p) = self.fence.page(i).filter(|&p| p != ZNOPAGE) else {
                continue;
            };
            if want.binary_search(&(p as usize)).is_err() {
                continue;
            }
            let page = self.page_bytes(p as usize).ok_or(ZError::Length)?;
            let (h, _) = parse_page(page, self.bind)?;
            let entry = self
                .fence
                .entry(i, self.nkeys as u32)
                .ok_or(ZError::Fence)?;
            check_entry(&h, &entry)?;
        }
        Ok(())
    }

    fn page_bytes(&self, p: usize) -> Option<&[u8]> {
        #[cfg(test)]
        if let Some((bytes, skew)) = &self.skewed {
            let at = skew + p * ZPAGE;
            return bytes.get(at..at + ZPAGE);
        }
        self.map.slice_at((p as u64) * ZPAGE as u64, ZPAGE)
    }

    /// A check failed on page `p`: it and the sidecar leave service.
    #[cold]
    fn fail(&self, p: usize) {
        if p < self.n_pages {
            set_bit(&self.bad, p);
        }
        self.damaged.store(true, Ordering::Relaxed);
    }

    /// Where `key` lies among the records of this partition, opening the
    /// one page that could hold it.
    fn locate<'a>(&'a self, key: &[u8], buf: &'a mut GroupBuf) -> Locate<'a> {
        if self.damaged.load(Ordering::Relaxed) {
            return Locate::Out;
        }
        // A key below the first key is not in the partition.
        let Some(e) = self.fence.entry_of(key) else {
            return Locate::Absent { decoded: false };
        };
        let Some(p) = self.fence.page(e) else {
            return Locate::Fallback;
        };
        // Records the form does not hold are the raw form's.
        if p == ZNOPAGE {
            return Locate::Fallback;
        }
        let p = p as usize;
        let Some(page) = self.page_bytes(p).filter(|_| p < self.n_pages) else {
            self.fail(p);
            return Locate::Fallback;
        };
        let was_verified = bit(&self.verified, p);
        let opened = if was_verified {
            verified_group(page, buf)
        } else {
            match self.fence.entry(e, self.nkeys as u32) {
                Some(entry) => first_decode(page, self.bind, &entry, buf),
                None => Err(Fail::Check(ZError::Fence)),
            }
        };
        let (group, count, decoded) = match opened {
            Ok(g) => g,
            Err(Fail::Misaligned) => return Locate::Fallback,
            Err(Fail::Check(_)) => {
                self.fail(p);
                return Locate::Fallback;
            }
        };
        if !was_verified {
            set_bit(&self.verified, p);
        }
        let pos = match group_seek(group, count, key) {
            Ok(pos) => pos,
            Err(_) => {
                self.fail(p);
                return Locate::Fallback;
            }
        };
        if !pos.found {
            return Locate::Absent { decoded };
        }
        match parse_record(group, pos.off) {
            Some((_, exts, tail, _)) => Locate::Found {
                exts,
                tail,
                decoded,
            },
            None => {
                self.fail(p);
                Locate::Fallback
            }
        }
    }

    /// `key`'s values in this partition, handed to `f` in the order the raw
    /// form hands them. `raw` is the partition's own segment, which reads a
    /// run that lies in one of its blocks, as the raw form does.
    pub(super) fn read_all<F: FnMut(&[u8])>(
        &self,
        raw: &Blob<MmapBytes>,
        key: &[u8],
        f: &mut F,
    ) -> Result<ZRead> {
        let mut buf = take_buf();
        let r = match self.locate(key, &mut buf) {
            Locate::Absent { decoded } => Ok(ZRead::Served { n: 0, decoded }),
            Locate::Out => Ok(ZRead::Out),
            Locate::Fallback => Ok(ZRead::Fallback),
            Locate::Found {
                exts,
                tail,
                decoded,
            } => raw
                .read_exts(&exts, tail, &mut *f)
                .map(|n| ZRead::Served { n, decoded }),
        };
        put_buf(buf);
        r
    }

    /// How many values `key` has in this partition.
    pub(super) fn count(&self, key: &[u8]) -> ZRead {
        let mut buf = take_buf();
        let r = match self.locate(key, &mut buf) {
            Locate::Absent { decoded } => ZRead::Served { n: 0, decoded },
            Locate::Out => ZRead::Out,
            Locate::Fallback => ZRead::Fallback,
            Locate::Found { exts, decoded, .. } => ZRead::Served {
                n: exts.iter().map(|e| u64::from(e.records())).sum(),
                decoded,
            },
        };
        put_buf(buf);
        r
    }

    /// The sidecar is out of service.
    pub(super) fn damaged(&self) -> bool {
        self.damaged.load(Ordering::Relaxed)
    }

    /// Pages in the file.
    pub(super) fn pages(&self) -> usize {
        self.n_pages
    }

    /// Whether the mapping was advised random at attach.
    pub(super) fn advised(&self) -> bool {
        self.advised
    }

    /// Pages whose first-decode checks have passed in this process, and
    /// pages a check has failed.
    pub(super) fn verified_pages(&self) -> u64 {
        popcount(&self.verified)
    }

    pub(super) fn bad_pages(&self) -> u64 {
        popcount(&self.bad)
    }

    /// Heap bytes the sidecar keeps resident: the fence.
    pub(super) fn resident_bytes(&self) -> usize {
        self.fence.heap_bytes()
    }

    /// The file's bytes.
    pub(super) fn file_bytes(&self) -> u64 {
        self.map.len()
    }

    /// The decoded bytes of every group the file holds, as its trailer says.
    pub(super) fn group_bytes(&self) -> u64 {
        self.ident.total_ulen
    }

    /// This attach's number: no other sidecar attached in this process has
    /// it.
    pub(super) fn id(&self) -> u64 {
        self.id
    }

    /// The trailer this attach accepted.
    #[cfg(test)]
    pub(super) fn ident(&self) -> &Trailer {
        &self.ident
    }
}

/// That the trailer is the live segment's: every quantity it records of the
/// segment equals what the segment recomputed of itself. The first mismatch
/// is named, the name's fields and the content's fingerprint first: two
/// segments written a second apart differ in their timestamps whatever else
/// they share, and a refusal that named the clock would say nothing of
/// whether the records were the same.
fn same_segment(
    t: &Trailer,
    f: &ZSource,
    id: u64,
    end_seq: u64,
) -> std::result::Result<(), &'static str> {
    if t.seg_id != id || t.seg_end_seq != end_seq {
        return Err("the sidecar names another segment");
    }
    if t.nkeys != f.nkeys {
        return Err("the key count differs");
    }
    if t.seg_row_crc != f.row_crc {
        return Err("the key section's checksum row differs");
    }
    if t.seg_recs_len != f.recs_len {
        return Err("the record region's length differs");
    }
    if t.seg_crc_off != f.crc_off {
        return Err("the checksum row's place differs");
    }
    if t.seg_key_off != f.key_off {
        return Err("the key section's offset differs");
    }
    if t.seg_hash_cap != f.hash_cap {
        return Err("the hash capacity differs");
    }
    if t.seg_file_len != f.file_len {
        return Err("the segment's length differs");
    }
    if t.seg_timestamp != f.timestamp {
        return Err("the segment's timestamp differs");
    }
    if t.seg_generation != f.generation {
        return Err("the segment's generation differs");
    }
    if t.rec_version != f.rec_version {
        return Err("the record format's version differs");
    }
    if t.rec_magic != f.rec_magic {
        return Err("the file format's magic differs");
    }
    if t.bind32() != bind32(id, end_seq, f.nkeys, f.row_crc) {
        return Err("the binding hash differs");
    }
    Ok(())
}

/// The cell a segment keeps its sidecar in: a pointer set once from null and
/// freed with the segment, read by an acquire load. Not a `OnceLock`, whose
/// initialiser takes a lock a reader could wait on.
pub(super) struct ZCell {
    p: AtomicPtr<ZSide>,
}

impl ZCell {
    pub(super) const fn new() -> ZCell {
        ZCell {
            p: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// The sidecar, if one is installed. Valid as long as the cell is
    /// borrowed: it is installed once and never cleared or replaced while
    /// the segment stands.
    #[inline]
    pub(super) fn get(&self) -> Option<&ZSide> {
        let p = self.p.load(Ordering::Acquire);
        // SAFETY: set from `Box::into_raw` by `install` and freed only by
        // `Drop`, which needs `&mut self`; nothing clears or replaces it.
        unsafe { p.as_ref() }
    }

    /// `z` installed if the cell is empty; whether it was.
    pub(super) fn install(&self, z: ZSide) -> bool {
        let p = Box::into_raw(Box::new(z));
        match self
            .p
            .compare_exchange(std::ptr::null_mut(), p, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            Err(_) => {
                // SAFETY: ours, and the cell refused it.
                drop(unsafe { Box::from_raw(p) });
                false
            }
        }
    }
}

impl Drop for ZCell {
    fn drop(&mut self) {
        let p = *self.p.get_mut();
        if !p.is_null() {
            // SAFETY: from `Box::into_raw`, freed once, with no reader left:
            // the segment that held the cell is being dropped.
            drop(unsafe { Box::from_raw(p) });
        }
    }
}

/// What the building thread did, counted where it happens and read by
/// whoever asks: written by the thread that builds and attaches (the
/// writer's, in `Db::zpages_settle` and at open), never by a read.
#[derive(Default)]
pub(super) struct ZEvents {
    /// Sidecars built, the pages and bytes of them, pages that hold their
    /// group verbatim, and the decoded bytes of every group.
    pub builds: AtomicU64,
    pub build_pages: AtomicU64,
    pub build_bytes: AtomicU64,
    pub build_stored: AtomicU64,
    pub build_group_bytes: AtomicU64,
    /// Compress calls the builds made.
    pub build_calls: AtomicU64,
    /// Builds that failed -- an I/O error, a page that did not pass the
    /// reader's checks, a file the attach after refused -- and builds a
    /// merge outran.
    pub build_failed: AtomicU64,
    pub build_orphaned: AtomicU64,
    /// Partitions declined, by reason, and the record bytes of the ones
    /// declined for their size.
    pub declined_small: AtomicU64,
    pub declined_small_bytes: AtomicU64,
    pub declined_block_backed: AtomicU64,
    pub declined_oversize: AtomicU64,
    pub declined_incompressible: AtomicU64,
    pub declined_unplaceable: AtomicU64,
    /// Sidecars attached, and files an attach refused: damaged, or bound to
    /// another segment.
    pub attached: AtomicU64,
    pub rejected_damage: AtomicU64,
    pub rejected_identity: AtomicU64,
    /// Files a sweep of the directory removed: sidecars no live segment
    /// claims, and the temp names of builds that did not finish.
    pub swept: AtomicU64,
    /// What the last refusal said: a count says a file was refused and not
    /// which check refused it, and a test that breaks a field wants to see
    /// that it was the check for the field.
    pub last_reject: std::sync::Mutex<String>,
}

impl ZEvents {
    pub(super) fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }

    /// An attach's refusal, counted by kind. A missing file is not one.
    pub(super) fn rejected(&self, r: &Reject) {
        let why = match r {
            Reject::Missing => return,
            Reject::Damage(why) => {
                ZEvents::add(&self.rejected_damage, 1);
                format!("damage: {why}")
            }
            Reject::Identity(why) => {
                ZEvents::add(&self.rejected_identity, 1);
                format!("identity: {why}")
            }
        };
        if let Ok(mut last) = self.last_reject.lock() {
            *last = why;
        }
    }

    /// A decline, counted by reason.
    pub(super) fn declined(&self, d: Decline) {
        match d {
            Decline::Small(bytes) => {
                ZEvents::add(&self.declined_small, 1);
                ZEvents::add(&self.declined_small_bytes, bytes as u64);
            }
            Decline::BlockBacked => ZEvents::add(&self.declined_block_backed, 1),
            Decline::NoPage(NoPage::Oversize) => ZEvents::add(&self.declined_oversize, 1),
            Decline::NoPage(NoPage::Incompressible) => {
                ZEvents::add(&self.declined_incompressible, 1)
            }
            Decline::NoPage(NoPage::Unplaceable) => ZEvents::add(&self.declined_unplaceable, 1),
        }
    }

    /// A build, counted.
    pub(super) fn built(&self, b: &Built) {
        ZEvents::add(&self.builds, 1);
        ZEvents::add(&self.build_pages, b.fill.pages);
        ZEvents::add(&self.build_bytes, b.file_bytes);
        ZEvents::add(&self.build_stored, b.fill.stored);
        ZEvents::add(&self.build_group_bytes, b.total_ulen);
        ZEvents::add(&self.build_calls, b.fill.calls);
    }
}

/// Why a partition was not given a sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decline {
    /// The record region is under the floor.
    Small(usize),
    /// A record's run lies in a block, so the value is read from the raw
    /// segment whatever the form holds.
    BlockBacked,
    /// A record that no page holds.
    NoPage(NoPage),
}

impl Decline {
    /// The code a segment keeps for the life of the decline
    /// (`Seg::z_declined`); zero is none.
    pub(super) fn code(self) -> u8 {
        match self {
            Decline::Small(_) => DECLINED_SMALL,
            Decline::BlockBacked => 2,
            Decline::NoPage(NoPage::Oversize) => 3,
            Decline::NoPage(NoPage::Incompressible) => 4,
            Decline::NoPage(NoPage::Unplaceable) => 5,
        }
    }
}

/// The code `Seg::z_declined` keeps for a partition under the floor, which a
/// pass that asks whether a store is covered counts apart from the ones that
/// could not be (`Db::zpage_counts`).
pub(super) const DECLINED_SMALL: u8 = 1;

/// The code `Seg::z_declined` keeps for a build that failed: not declined
/// for what the partition holds, but not tried again by this segment.
pub(super) const DECLINED_FAILED: u8 = 6;

/// The code `Seg::z_declined` keeps for a partition whose sidecar file an
/// attach refused, which was counted when it was: a build replaces the file
/// and the attach is not asked again of the same file, or it would be
/// counted twice. Unlike the others it is not a reason to leave the
/// partition raw, and a settle still gives it a sidecar.
pub(super) const DECLINED_REFUSED: u8 = 7;

/// What a build made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Built {
    /// Pages, and what `fill_group` did to make them.
    pub fill: FillStats,
    /// The file's length.
    pub file_bytes: u64,
    /// The decoded bytes of every group.
    pub total_ulen: u64,
}

#[derive(Debug)]
pub(super) enum Outcome {
    Built(Built),
    Declined(Decline),
}

/// Writes in whole pieces: bytes are buffered to `ZWRITE_PIECE` and the
/// piece is written at its own alignment, with a hint to the device for
/// it, nothing waited for.
struct Pieces<'a> {
    file: &'a File,
    buf: Vec<u8>,
    at: u64,
}

impl<'a> Pieces<'a> {
    fn new(file: &'a File) -> Pieces<'a> {
        Pieces {
            file,
            buf: Vec::with_capacity(ZWRITE_PIECE),
            at: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let room = ZWRITE_PIECE - self.buf.len();
            let take = room.min(rest.len());
            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buf.len() == ZWRITE_PIECE {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let mut f = self.file;
        f.write_all(&self.buf)?;
        super::start_writeback(self.file, self.at, self.buf.len() as u64);
        self.at += self.buf.len() as u64;
        self.buf.clear();
        Ok(())
    }
}

/// Drop `[from, to)` of the file behind the build: the pages of its own
/// mapping first, then the page cache's, because the cache skips a page a
/// mapping still holds. The window is trimmed to whole pages inside it, and
/// the pages another mapping holds -- the readers' hot raw pages -- survive
/// the second call by design.
#[cfg(target_os = "linux")]
fn drop_behind(base: usize, file: &File, from: usize, to: usize) {
    use std::os::unix::io::AsRawFd;
    let a = from.next_multiple_of(ZPAGE);
    let b = to & !(ZPAGE - 1);
    if a >= b {
        return;
    }
    // SAFETY: a read-only shared mapping of a file, advised over a range
    // inside it: the call drops this process's page-table entries for the
    // range and nothing else, and the next touch faults the page back in.
    unsafe {
        libc::madvise((base + a) as *mut libc::c_void, b - a, libc::MADV_DONTNEED);
        libc::posix_fadvise(
            file.as_raw_fd(),
            a as libc::off_t,
            (b - a) as libc::off_t,
            libc::POSIX_FADV_DONTNEED,
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn drop_behind(_base: usize, _file: &File, _from: usize, _to: usize) {}

/// The sidecar of the segment file at `seg_path`, written to `out`.
///
/// The segment is opened as a merge opens its input: a mapping of its own,
/// advised for the sequential walk the build makes (the live mapping is
/// random after the first point read), and the key index's row not verified
/// again, since the open that made the segment live did. The records are
/// read through the directory, in rank order, each as the bytes
/// `parse_record` consumes for it and never as the difference of two
/// directory words: a segment closed from a direct run keeps its commit
/// markers inside the record region. A group of them is filled from the
/// previous page's count (`fill_group`), and the page it makes is run
/// through the reader's own first-decode checks (`verify_page`) before it is
/// written, so a defect that would fail every first decode cannot pass the
/// build and loop the rebuild.
///
/// The file is written to `out` with `.tmp` appended, in pieces, the
/// resident part and the trailer last, and renamed into place, unsynced: a
/// derived file the store regenerates. The raw records are dropped from the
/// page cache behind the walk, a window at a time.
///
/// A partition is declined, with no file written, for a record region
/// under `floor`, for a block table that is not empty (a run lies outside
/// its record), and for a record that no page holds. No filter is built.
pub(super) fn build(
    seg_path: &Path,
    out: &Path,
    id: u64,
    end_seq: u64,
    floor: usize,
) -> Result<Outcome> {
    let file = File::open(seg_path)?;
    // SAFETY: the contract `MmapBytes::open` takes: a sealed segment is
    // never truncated under a mapping.
    let map = MmapBytes(unsafe { memmap2::Mmap::map(&file)? });
    let base = map.0.as_ptr() as usize;
    let blob = Blob::open_with(
        map,
        BlobOptions {
            verify_checksums: false,
            verify_index: false,
            ..Default::default()
        },
    )?;
    blob.advise_normal();
    let facts = blob.z_source_facts().ok_or_else(|| {
        super::err("z build: the segment has no checksum row to bind a sidecar to")
    })?;
    let (recs, dir) = blob
        .record_regions()
        .ok_or_else(|| super::err("z build: the segment's records are not resident"))?;
    let recs_len = usize::try_from(facts.recs_len).unwrap_or(usize::MAX);
    if recs_len < floor {
        return Ok(Outcome::Declined(Decline::Small(recs_len)));
    }
    if blob.blocks() > 0 {
        return Ok(Outcome::Declined(Decline::BlockBacked));
    }
    let nkeys = blob.keys();
    let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
    let tmp = {
        let mut s = out.as_os_str().to_owned();
        s.push(".tmp");
        std::path::PathBuf::from(s)
    };
    let made = write_pages(
        &file, base, recs, dir, nkeys, bind, &tmp, &facts, id, end_seq,
    );
    match made {
        Ok(Outcome::Built(b)) => {
            std::fs::rename(&tmp, out).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })?;
            Ok(Outcome::Built(b))
        }
        other => {
            let _ = std::fs::remove_file(&tmp);
            other
        }
    }
}

/// The pages, the resident part and the trailer of the sidecar of the
/// segment whose records and directory these are, written to `tmp`. `base`
/// is the address of the segment's mapping and `file` its descriptor, for
/// the drop-behind.
#[allow(clippy::too_many_arguments)]
fn write_pages(
    file: &File,
    base: usize,
    recs: &[u8],
    dir: &[u8],
    nkeys: usize,
    bind: u32,
    tmp: &Path,
    facts: &ZSource,
    id: u64,
    end_seq: u64,
) -> Result<Outcome> {
    let out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp)?;
    let mut w = Pieces::new(&out);
    let images = SegImages::new(recs, dir, nkeys);
    let mut scratch = FillScratch::new();
    let mut verify_buf = GroupBuf::new();
    let mut fence = ZFence::new();
    let recs_at = recs.as_ptr() as usize - base;
    let dir_at = dir.as_ptr() as usize - base;
    let (mut rank, mut hint, mut pages) = (0usize, 0usize, 0u32);
    let mut total_ulen = 0u64;
    let mut dropped = 0usize;
    while rank < nkeys {
        let mut page = [0u8; ZPAGE];
        match fill_group(&images, rank, hint, bind, &mut scratch, &mut page) {
            Fill::Page(g) => {
                let first = images.image(rank);
                let key = record_key(first, 0)
                    .ok_or_else(|| super::err("z build: a record has no key"))?;
                let entry = FenceEntry {
                    index: pages as usize,
                    first_rank: u32::try_from(rank)
                        .map_err(|_| super::err("z build: a rank past four billion"))?,
                    count: g.count as u32,
                    page: pages,
                    key,
                };
                verify_page(&page, bind, &entry, &mut verify_buf).map_err(|e| {
                    super::err(&format!("z build: a page fails the reader's checks: {e}"))
                })?;
                fence.push(entry.first_rank, pages, key)?;
                w.push(&page)?;
                total_ulen += g.ulen as u64;
                pages += 1;
                rank += g.count;
                hint = g.count;
                // The raw records the walk has passed, a window at a time:
                // the directory says where the next one begins.
                let consumed = if rank < nkeys {
                    rd_u32(dir, rank * 4).map_or(recs.len(), |o| o as usize)
                } else {
                    recs.len()
                }
                .min(recs.len());
                if consumed.saturating_sub(dropped) >= ZDROP_WINDOW {
                    drop_behind(base, file, recs_at + dropped, recs_at + consumed);
                    dropped = consumed;
                }
            }
            Fill::NoPage(why) => return Ok(Outcome::Declined(Decline::NoPage(why))),
            Fill::End => return Err(super::err("z build: the walk ran past the last record")),
            Fill::Mismatch => {
                return Err(super::err(
                    "z build: a page did not decode to the records it was built from",
                ))
            }
        }
    }
    drop_behind(base, file, recs_at + dropped, recs_at + recs.len());
    drop_behind(base, file, dir_at, dir_at + dir.len());
    let (res, rf) = fence.encode_resident(&[])?;
    w.push(&res)?;
    let t = Trailer {
        n_pages: u64::from(pages),
        n_entries: fence.len() as u64,
        nkeys: nkeys as u64,
        res_off: u64::from(pages) * ZPAGE as u64,
        res_len: rf.res_len,
        keys_len: rf.keys_len,
        filter_off: 0,
        filter_blocks: 0,
        res_crc: rf.res_crc,
        filter: false,
        total_ulen,
        seg_recs_len: facts.recs_len,
        seg_crc_off: facts.crc_off,
        seg_key_off: facts.key_off,
        seg_hash_cap: facts.hash_cap,
        seg_file_len: facts.file_len,
        seg_timestamp: facts.timestamp,
        seg_generation: facts.generation,
        seg_row_crc: facts.row_crc,
        rec_version: facts.rec_version,
        rec_magic: facts.rec_magic,
        seg_id: id,
        seg_end_seq: end_seq,
        bloom_probes: 0,
        bloom_sig: 0,
        bloom_bits_per_key: 0,
    };
    w.push(&t.encode())?;
    w.flush()?;
    let file_bytes = w.at;
    debug_assert_eq!(Some(file_bytes), t.expected_file_len());
    Ok(Outcome::Built(Built {
        fill: scratch.stats,
        file_bytes,
        total_ulen,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::zpages::ZHDR;
    use crate::db::{SegmentOptions, SegmentWriter};
    use std::os::unix::fs::FileExt;
    use std::path::PathBuf;

    struct Rng(u64);

    impl Rng {
        fn word(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.word() % n as u64) as usize
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| (self.word() >> 24) as u8).collect()
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("supdb-next-zside-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A key, its values in append order, and whether its run is flagged a
    /// tombstone.
    type Rec = (Vec<u8>, Vec<Vec<u8>>, bool);

    /// A segment written as the engine's own writer writes one.
    fn write_segment(path: &Path, recs: &[Rec], inline_max: usize) {
        let mut w = SegmentWriter::create(path, &SegmentOptions::default()).unwrap();
        w.set_inline_max(inline_max);
        for (k, vals, tomb) in recs {
            w.begin(k).unwrap();
            for v in vals {
                w.value(v);
            }
            w.end_with(*tomb).unwrap();
        }
        w.finish(1).unwrap();
    }

    /// The suite's record: sixteen digits and a hundred bytes of which half
    /// is a repeated phrase at a phase drawn for each and half entropy.
    fn suite(n: usize, seed: u64) -> Vec<Rec> {
        let mut rng = Rng(seed | 1);
        let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
        (0..n)
            .map(|i| {
                let phase = rng.below(motif.len());
                let mut v: Vec<u8> = (0..50).map(|j| motif[(phase + j) % motif.len()]).collect();
                v.extend(rng.bytes(50));
                (format!("{i:016}").into_bytes(), vec![v], false)
            })
            .collect()
    }

    /// Runs of every shape the format has, all inline and all small enough
    /// for a page once compressed: one value, two, seventeen, two hundred and
    /// fifteen hundred; a fixed width and a prefixed run; zero-length
    /// values; a tombstone with nothing under it, and one over a run; and
    /// the empty key first.
    fn shapes() -> Vec<Rec> {
        let mut rng = Rng(0x5eed_1234_9876);
        let mut recs: Vec<Rec> = vec![(Vec::new(), vec![b"the empty key".to_vec()], false)];
        let mut id = 0u32;
        let mut key = |tag: &str| {
            id += 1;
            format!("{tag}-{id:05}").into_bytes()
        };
        for n in [1usize, 2, 17, 200, 1500] {
            // A fixed width, and a pattern a page's compressor takes whole.
            recs.push((
                key("fixed"),
                (0..n).map(|i| vec![(i % 3) as u8, 0, 0, 7]).collect(),
                false,
            ));
            // Prefixed: widths vary, among them none.
            recs.push((
                key("varied"),
                (0..n).map(|i| vec![b'a' + (i % 3) as u8; i % 7]).collect(),
                false,
            ));
        }
        recs.push((key("zeros"), vec![Vec::new(); 5], false));
        recs.push((key("zero"), vec![Vec::new()], false));
        recs.push((key("tomb"), vec![], true));
        recs.push((
            key("tombrun"),
            (0..40).map(|i| vec![i as u8; 3]).collect(),
            true,
        ));
        for _ in 0..300 {
            let n = 1 + rng.below(6);
            let k = key("rand");
            recs.push((
                k,
                (0..n)
                    .map(|_| {
                        let l = rng.below(60);
                        vec![b'x'; l]
                    })
                    .collect(),
                false,
            ));
        }
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        recs.dedup_by(|a, b| a.0 == b.0);
        recs
    }

    const ID: u64 = 7;
    const END: u64 = 99;

    struct Fixture {
        dir: PathBuf,
        seg: PathBuf,
        out: PathBuf,
        blob: Blob<MmapBytes>,
        recs: Vec<Rec>,
    }

    impl Fixture {
        fn new(name: &str, recs: Vec<Rec>, inline_max: usize) -> Fixture {
            let dir = dir(name);
            let seg = dir.join("par-00000007-0000000000000099--.sup");
            write_segment(&seg, &recs, inline_max);
            let blob = Blob::open(MmapBytes::open(&seg).unwrap()).unwrap();
            Fixture {
                out: dir.join(zpg_name(ID, END)),
                dir,
                seg,
                blob,
                recs,
            }
        }

        fn build(&self) -> Outcome {
            build(&self.seg, &self.out, ID, END, 0).unwrap()
        }

        fn attach(&self) -> std::result::Result<ZSide, Reject> {
            ZSide::attach(&self.out, &self.blob, ID, END, false)
        }

        /// A sidecar built and attached.
        fn side(&self) -> ZSide {
            assert!(matches!(self.build(), Outcome::Built(_)));
            self.attach().expect("the sidecar it just built attaches")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn raw_values(blob: &Blob<MmapBytes>, key: &[u8]) -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        blob.read_all(key, |x| v.push(x.to_vec())).unwrap();
        v
    }

    /// What the sidecar answers for `key`, or `None` where it declines.
    fn z_values(side: &ZSide, blob: &Blob<MmapBytes>, key: &[u8]) -> Option<Vec<Vec<u8>>> {
        let mut v = Vec::new();
        match side
            .read_all(blob, key, &mut |x: &[u8]| v.push(x.to_vec()))
            .unwrap()
        {
            ZRead::Served { n, .. } => {
                assert_eq!(n as usize, v.len(), "the count the read returned");
                Some(v)
            }
            _ => None,
        }
    }

    fn served(r: ZRead) -> u64 {
        match r {
            ZRead::Served { n, .. } => n,
            other => panic!("not served: {other:?}"),
        }
    }

    /// Keys around the records: below the first, above the last, between
    /// two, one byte either side of each key, a proper prefix of each, and
    /// an extension of each.
    fn probes(recs: &[Rec]) -> Vec<Vec<u8>> {
        let mut p: Vec<Vec<u8>> = vec![Vec::new(), vec![0], vec![0xff; 8], b"zzzzzzzz".to_vec()];
        for (k, _, _) in recs {
            p.push(k.clone());
            p.push([k.as_slice(), &[0u8]].concat());
            p.push([k.as_slice(), &[0xffu8]].concat());
            if let Some((&last, head)) = k.split_last() {
                p.push(head.to_vec());
                let mut below = head.to_vec();
                below.push(last.wrapping_sub(1));
                p.push(below);
                let mut above = head.to_vec();
                above.push(last.wrapping_add(1));
                p.push(above);
            }
        }
        p
    }

    /// Every probe answers as the segment's own reader answers it, values
    /// and count both.
    fn agree(side: &ZSide, blob: &Blob<MmapBytes>, recs: &[Rec]) {
        for key in probes(recs) {
            let want = raw_values(blob, &key);
            assert_eq!(
                z_values(side, blob, &key).as_ref(),
                Some(&want),
                "read of {key:?}"
            );
            assert_eq!(
                served(side.count(&key)),
                blob.count(&key).unwrap(),
                "count of {key:?}"
            );
        }
        assert!(!side.damaged());
    }

    #[test]
    fn a_sidecar_answers_every_key_as_the_segment_does() {
        for (name, recs, inline) in [
            ("suite", suite(3000, 11), 256),
            ("shapes", shapes(), 1 << 20),
        ] {
            let fx = Fixture::new(name, recs, inline);
            let side = fx.side();
            assert!(side.pages() > 1, "{name}: more than one page");
            agree(&side, &fx.blob, &fx.recs);
            // Every page a read opened was checked, once, and none failed.
            assert!(side.verified_pages() > 0 && side.bad_pages() == 0);
            // The second pass finds every page verified and answers the same.
            let before = side.verified_pages();
            agree(&side, &fx.blob, &fx.recs);
            assert_eq!(side.verified_pages(), before, "{name}");
        }
    }

    #[test]
    fn a_read_hands_over_nothing_before_its_checks_have_passed() {
        // The callback is called only once every check that can fail has
        // passed: a read the sidecar declines is read from the raw form by
        // the caller, and a value already handed over would be handed over
        // twice. A byte of a page the attach does not sample is flipped, so
        // the sidecar attaches and the page fails at its first decode.
        let fx = Fixture::new("before", suite(4000, 12), 256);
        assert!(matches!(fx.build(), Outcome::Built(_)));
        let pages = fx.attach().unwrap().pages();
        assert!(
            pages > 3 * ZSAMPLE_PAGES,
            "pages enough to leave some unsampled"
        );
        let sampled =
            |p: usize| (0..ZSAMPLE_PAGES).any(|k| k * (pages - 1) / (ZSAMPLE_PAGES - 1) == p);
        let unsampled: Vec<usize> = (1..pages - 1).filter(|&p| !sampled(p)).collect();
        let p = unsampled[unsampled.len() / 2];
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fx.out)
            .unwrap();
        let mut b = [0u8; 1];
        let at = (p * ZPAGE + 100) as u64;
        f.read_at(&mut b, at).unwrap();
        f.write_at(&[b[0] ^ 0x5a], at).unwrap();
        drop(f);
        let side = fx
            .attach()
            .expect("a damaged page the sample misses attaches");
        let (mut served, mut declined) = (0usize, 0usize);
        for (k, vals, _) in &fx.recs {
            let mut got = 0usize;
            match side
                .read_all(&fx.blob, k, &mut |_: &[u8]| got += 1)
                .unwrap()
            {
                ZRead::Served { n, .. } => {
                    assert_eq!((n as usize, got), (vals.len(), vals.len()));
                    served += 1;
                }
                ZRead::Fallback | ZRead::Out => {
                    assert_eq!(got, 0, "a read that declines has emitted nothing");
                    declined += 1;
                }
            }
        }
        assert!(
            served > 0 && declined > 0,
            "{served} served, {declined} declined"
        );
        assert!(side.damaged() && side.bad_pages() == 1);
    }

    #[test]
    fn a_partition_that_cannot_have_a_sidecar_is_declined_for_its_reason() {
        // Under the floor.
        let fx = Fixture::new("small", suite(200, 1), 256);
        let r = build(&fx.seg, &fx.out, ID, END, ZMIN_RECS).unwrap();
        assert!(matches!(r, Outcome::Declined(Decline::Small(n)) if n > 0 && n < ZMIN_RECS));
        assert!(!fx.out.exists(), "no file for a declined partition");
        assert!(!fx.out.with_extension("zpg.tmp").exists());

        // A run in a block.
        let mut recs = suite(300, 2);
        recs.push((b"zzz-big".to_vec(), vec![vec![7u8; 9000]], false));
        let fx = Fixture::new("blocks", recs, 256);
        assert!(fx.blob.blocks() > 0);
        assert!(matches!(
            fx.build(),
            Outcome::Declined(Decline::BlockBacked)
        ));
        assert!(!fx.out.exists());

        // A record no page holds: random bytes that do not fit a page
        // compressed, and an image over the cap a group decodes into.
        let mut rng = Rng(77);
        let mut recs = suite(300, 3);
        recs.push((b"zzz-big".to_vec(), vec![rng.bytes(6000)], false));
        let fx = Fixture::new("incompressible", recs, 1 << 20);
        assert!(matches!(
            fx.build(),
            Outcome::Declined(Decline::NoPage(NoPage::Incompressible))
        ));
        assert!(!fx.out.exists());
        let mut recs = suite(300, 4);
        recs.push((b"zzz-big".to_vec(), vec![vec![0u8; 70_000]], false));
        let fx = Fixture::new("oversize", recs, 1 << 20);
        assert!(matches!(
            fx.build(),
            Outcome::Declined(Decline::NoPage(NoPage::Oversize))
        ));
        assert!(!fx.out.exists());
    }

    #[test]
    fn a_page_that_does_not_compress_is_stored_and_read_in_place() {
        // Keys and values of entropy: a group's LZ4 output is no shorter
        // than the group, and the page holds the group verbatim.
        let mut rng = Rng(0xfeed);
        let mut recs: Vec<Rec> = (0..2000)
            .map(|_| (rng.bytes(16), vec![rng.bytes(100)], false))
            .collect();
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        recs.dedup_by(|a, b| a.0 == b.0);
        let fx = Fixture::new("stored", recs, 256);
        let Outcome::Built(b) = fx.build() else {
            panic!("built")
        };
        assert!(b.fill.stored > 0, "stored pages");
        let side = fx.attach().unwrap();
        agree(&side, &fx.blob, &fx.recs);
        // A stored page is parsed where it lies: nothing is decoded.
        let key = &fx.recs[fx.recs.len() / 2].0;
        let mut seen = 0;
        let r = side
            .read_all(&fx.blob, key, &mut |_: &[u8]| seen += 1)
            .unwrap();
        assert_eq!(seen, 1);
        let ZRead::Served { decoded, .. } = r else {
            panic!("served")
        };
        let stored_page = {
            let e = side.fence.entry_of(key).unwrap();
            let p = side.fence.page(e).unwrap() as usize;
            page_view(side.page_bytes(p).unwrap()).unwrap().0.stored
        };
        assert_eq!(decoded, !stored_page);
    }

    /// A page whose payload sits at a given misalignment, as a buffer a
    /// source other than a mapping might hand out.
    fn page_at(page: &[u8; ZPAGE], skew: usize) -> (Vec<u8>, usize) {
        let mut backing = vec![0u8; ZPAGE + 16];
        let base = backing.as_ptr() as usize;
        let start = (8 - base % 8) % 8 + skew;
        backing[start..start + ZPAGE].copy_from_slice(page);
        (backing, start)
    }

    #[test]
    fn a_stored_page_at_a_misaligned_address_is_refused_not_missed() {
        // A stored page whose payload is not aligned for the extents
        // `parse_record` borrows in place reads as no record, and a probe
        // that took none for absent would answer a held key as missing. The
        // reader refuses the page, and a caller counts the refusal.
        let mut rng = Rng(5);
        let mut keyed: Vec<(Vec<u8>, Vec<u8>)> =
            (0..20).map(|_| (rng.bytes(16), rng.bytes(100))).collect();
        keyed.sort();
        let recs: Vec<Vec<u8>> = keyed
            .iter()
            .map(|(k, v)| {
                let mut img = Vec::new();
                let e = Ext {
                    block: Ext::INLINE,
                    off: 0,
                    len: 100,
                    last: 0,
                    count: 1 | Ext::FIXED,
                };
                crate::flatindex::stream_record_as(&mut img, k, &[e], v, true).unwrap();
                img
            })
            .collect();
        let bind = 0xABCD_1234;
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(g) = fill_group(&recs, 0, 0, bind, &mut s, &mut page) else {
            panic!("a page")
        };
        assert!(g.stored, "random bytes are stored");
        let (h, _) = parse_page(&page, bind).unwrap();
        let key0 = keyed[0].0.clone();
        let entry = FenceEntry {
            index: 0,
            first_rank: 0,
            count: g.count as u32,
            page: 0,
            key: &key0,
        };
        let mut buf = GroupBuf::new();

        // Aligned: parsed in place, no decode, and the group is found.
        let (aligned, at) = page_at(&page, 0);
        let p = &aligned[at..at + ZPAGE];
        assert_eq!(p.as_ptr() as usize % 8, 0);
        let (group, count, decoded) = first_decode(p, bind, &entry, &mut buf).unwrap();
        assert!(!decoded && count == usize::from(h.count));
        assert!(group_seek(group, count, &key0).unwrap().found);
        let (group, count, decoded) = verified_group(p, &mut buf).unwrap();
        assert!(!decoded && count == usize::from(h.count));
        assert!(group_seek(group, count, &key0).unwrap().found);

        // Misaligned by every skew that is not a multiple of four: refused,
        // at the first decode and after.
        for skew in [1usize, 2, 3, 5, 6, 7] {
            let (backing, at) = page_at(&page, skew);
            let p = &backing[at..at + ZPAGE];
            assert_ne!((p.as_ptr() as usize + ZHDR) % 4, 0, "skew {skew}");
            assert_eq!(
                first_decode(p, bind, &entry, &mut buf).unwrap_err(),
                Fail::Misaligned,
                "skew {skew}"
            );
            assert_eq!(
                verified_group(p, &mut buf).unwrap_err(),
                Fail::Misaligned,
                "skew {skew}"
            );
        }
    }

    #[test]
    fn a_callback_that_reads_again_leaves_the_value_it_was_lent_alone() {
        // The group a value is lent from is in the thread's buffer. A read
        // made inside the callback decodes another page: if it shared the
        // buffer it would overwrite the bytes the outer callback holds.
        let fx = Fixture::new("reentry", suite(3000, 21), 256);
        let side = fx.side();
        let (a, b) = (&fx.recs[10], &fx.recs[2500]);
        let mut seen = false;
        let r = side
            .read_all(&fx.blob, &a.0, &mut |outer: &[u8]| {
                assert_eq!(outer, &a.1[0][..]);
                let inner = z_values(&side, &fx.blob, &b.0).unwrap();
                assert_eq!(inner, b.1);
                // Read again byte by byte, volatile: a slice lent for a call
                // is one the compiler may take to be unchanged under any
                // call made inside it, and what is asked is whether the
                // call changed it.
                let again: Vec<u8> = (0..outer.len())
                    // SAFETY: inside the slice the callback was lent.
                    .map(|i| unsafe { std::ptr::read_volatile(outer.as_ptr().add(i)) })
                    .collect();
                assert_eq!(again, a.1[0], "the outer value was overwritten");
                seen = true;
            })
            .unwrap();
        assert!(seen && served(r) == 1);
    }

    #[test]
    fn a_sidecar_of_another_segment_is_refused() {
        // The same ids, the same keys, other values: the hard case. Sidecar
        // one planted where sidecar two belongs.
        let one = Fixture::new("other-1", suite(2000, 31), 256);
        let two = Fixture::new("other-2", suite(2000, 32), 256);
        one.side();
        two.side();
        std::fs::copy(&one.out, &two.out).unwrap();
        // The records differ, so the fingerprint of them does; two segments
        // written a second apart differ in their clocks as well, which is
        // not what is named.
        match two.attach() {
            Err(Reject::Identity(why)) => {
                assert!(why.contains("checksum row"), "{why}");
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
        // And the name's own fields.
        one.side();
        let b = &one.blob;
        assert!(matches!(
            ZSide::attach(&one.out, b, ID + 1, END, false),
            Err(Reject::Identity(_))
        ));
        assert!(matches!(
            ZSide::attach(&one.out, b, ID, END + 1, false),
            Err(Reject::Identity(_))
        ));
        // No file is no rejection.
        let _ = std::fs::remove_file(&one.out);
        assert!(matches!(one.attach(), Err(Reject::Missing)));
    }

    /// A refusal as words: its kind, and the check that made it.
    fn reject_text(r: &Reject) -> String {
        match r {
            Reject::Missing => "missing".into(),
            Reject::Damage(why) => format!("damage: {why}"),
            Reject::Identity(why) => format!("identity: {why}"),
        }
    }

    /// The trailer of the file at `path`, and a rewrite of it with a field
    /// changed, its CRC and `bind32` made again.
    fn with_trailer(path: &Path, change: impl FnOnce(&mut Trailer)) {
        let mut bytes = std::fs::read(path).unwrap();
        let at = bytes.len() - ZTRAILER;
        let mut t = Trailer::decode(&bytes[at..]).unwrap();
        change(&mut t);
        bytes[at..].copy_from_slice(&t.encode());
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn every_field_of_the_trailer_wrong_is_refused() {
        let fx = Fixture::new("trailer", suite(2500, 41), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t0 = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        type Edit = Box<dyn Fn(&mut Trailer)>;
        // Each field changed, with the refusal it gets: the kind of it and
        // the words of the check that made it. A field that some other
        // check would also refuse is not shown to have a check of its own by
        // a refusal alone, so the check is named: the segment's identity is
        // compared field by field, and a field whose comparison was removed
        // would still be refused, by the binding hash, in other words.
        let edits: Vec<(&str, Edit, &str)> = vec![
            (
                "n_pages",
                Box::new(|t| t.n_pages += 1),
                "damage: z form: the trailer is inconsistent: res_off is not the pages' length",
            ),
            (
                "n_entries",
                Box::new(|t| t.n_entries += 1),
                "damage: z form: the resident part is malformed: the region is not the entries alone",
            ),
            (
                "nkeys",
                Box::new(|t| t.nkeys += 1),
                "identity: the key count differs",
            ),
            (
                "res_off",
                Box::new(|t| t.res_off += ZPAGE as u64),
                "damage: z form: the trailer is inconsistent: res_off is not the pages' length",
            ),
            (
                "res_len",
                Box::new(|t| t.res_len += 1),
                "damage: z form: the resident part's checksum does not match",
            ),
            (
                "keys_len",
                Box::new(|t| t.keys_len += 1),
                "damage: z form: the resident part is malformed: the region is not the entries alone",
            ),
            (
                "filter_off",
                Box::new(|t| t.filter_off = 64),
                "damage: z form: the resident part is malformed: the region is not the entries alone",
            ),
            (
                "filter_blocks",
                Box::new(|t| t.filter_blocks = 1),
                "damage: z form: the trailer is inconsistent: the filter flag and the filter's blocks disagree",
            ),
            (
                "res_crc",
                Box::new(|t| t.res_crc ^= 1),
                "damage: z form: the resident part's checksum does not match",
            ),
            (
                "filter",
                Box::new(|t| t.filter = true),
                "damage: z form: the trailer is inconsistent: the filter flag and the filter's blocks disagree",
            ),
            (
                "seg_recs_len",
                Box::new(|t| t.seg_recs_len += 4),
                "identity: the record region's length differs",
            ),
            (
                "seg_crc_off",
                Box::new(|t| t.seg_crc_off += 4),
                "identity: the checksum row's place differs",
            ),
            (
                "seg_key_off",
                Box::new(|t| t.seg_key_off += 4096),
                "identity: the key section's offset differs",
            ),
            (
                "seg_hash_cap",
                Box::new(|t| t.seg_hash_cap *= 2),
                "identity: the hash capacity differs",
            ),
            (
                "seg_file_len",
                Box::new(|t| t.seg_file_len += 4096),
                "identity: the segment's length differs",
            ),
            (
                "seg_timestamp",
                Box::new(|t| t.seg_timestamp += 1),
                "identity: the segment's timestamp differs",
            ),
            (
                "seg_generation",
                Box::new(|t| t.seg_generation += 1),
                "identity: the segment's generation differs",
            ),
            (
                "seg_row_crc",
                Box::new(|t| t.seg_row_crc ^= 1),
                "identity: the key section's checksum row differs",
            ),
            (
                "rec_version",
                Box::new(|t| t.rec_version += 1),
                "identity: the record format's version differs",
            ),
            (
                "rec_magic",
                Box::new(|t| t.rec_magic ^= 1),
                "identity: the file format's magic differs",
            ),
            (
                "seg_id",
                Box::new(|t| t.seg_id += 1),
                "identity: the sidecar names another segment",
            ),
            (
                "seg_end_seq",
                Box::new(|t| t.seg_end_seq += 1),
                "identity: the sidecar names another segment",
            ),
        ];
        for (field, edit, why) in &edits {
            std::fs::write(&fx.out, &good).unwrap();
            with_trailer(&fx.out, |t| edit(t));
            match fx.attach() {
                Err(r) => assert_eq!(&reject_text(&r), why, "a trailer with {field} changed"),
                Ok(_) => panic!("a trailer with {field} changed attached"),
            }
        }
        // The fields no read depends on: the decoded bytes a trailer sums
        // for the ratio, and the filter's parameters, which a trailer with
        // no filter does not read. A wrong value is accepted and harmless.
        for (field, edit) in [
            (
                "total_ulen",
                Box::new(|t: &mut Trailer| t.total_ulen += 1) as Edit,
            ),
            ("bloom_sig", Box::new(|t: &mut Trailer| t.bloom_sig = 9)),
            (
                "bloom_probes",
                Box::new(|t: &mut Trailer| t.bloom_probes = 9),
            ),
        ] {
            std::fs::write(&fx.out, &good).unwrap();
            with_trailer(&fx.out, |t| edit(t));
            let side = fx.attach().unwrap_or_else(|e| panic!("{field}: {e:?}"));
            agree(&side, &fx.blob, &fx.recs);
        }
        // The file as built still attaches.
        std::fs::write(&fx.out, &good).unwrap();
        let side = fx.attach().unwrap();
        assert_eq!(side.ident(), &t0);
        // And a torn trailer: every byte of it flipped in turn.
        for at in (good.len() - ZTRAILER..good.len()).step_by(7) {
            let mut bad = good.clone();
            bad[at] ^= 0x10;
            std::fs::write(&fx.out, &bad).unwrap();
            assert!(
                matches!(fx.attach(), Err(Reject::Damage(_))),
                "a trailer flipped at {at} attached"
            );
        }
        // A file cut short, and one of the wrong length.
        for len in [0, ZPAGE, good.len() - ZPAGE, good.len() - 1, good.len() + 1] {
            let mut bad = good.clone();
            bad.resize(len, 0);
            std::fs::write(&fx.out, &bad).unwrap();
            assert!(
                matches!(fx.attach(), Err(Reject::Damage(_))),
                "a file of {len} bytes attached"
            );
        }
        // A page gone from the middle with the trailer whole at the end: a
        // file of a whole number of pages that is not the length its trailer
        // says, which is refused for that before anything is read from it.
        let mut bad = good[..ZPAGE].to_vec();
        bad.extend_from_slice(&good[2 * ZPAGE..]);
        std::fs::write(&fx.out, &bad).unwrap();
        match fx.attach() {
            Err(r) => {
                let text = reject_text(&r);
                assert!(
                    text.starts_with("damage:")
                        && text.ends_with("the file is not the length its trailer says"),
                    "{text}"
                );
            }
            Ok(_) => panic!("a file a page short attached"),
        }
    }

    #[test]
    fn a_flipped_byte_in_the_pages_is_never_an_answer() {
        // Every seventh byte of the page region, one at a time. The first,
        // the last and fourteen pages between are checked at attach, so a
        // flip in one of those is a refusal; a flip in any other page is
        // met at the page's first decode, which declines the read, marks
        // the page bad and takes the sidecar out of service. No flip is
        // ever a wrong answer, and none panics.
        let fx = Fixture::new("flips", suite(1500, 51), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let pages = t.n_pages as usize;
        assert!(pages > ZSAMPLE_PAGES + 4, "{pages} pages");
        let sampled: Vec<usize> = (0..ZSAMPLE_PAGES)
            .map(|k| k * (pages - 1) / (ZSAMPLE_PAGES - 1))
            .collect();
        let f = OpenOptions::new().write(true).open(&fx.out).unwrap();
        let (mut refused, mut declined, mut quiet) = (0usize, 0usize, 0usize);
        for at in (0..pages * ZPAGE).step_by(7) {
            let p = at / ZPAGE;
            f.write_at(&[good[at] ^ 0x21], at as u64).unwrap();
            match fx.attach() {
                Err(Reject::Damage(_)) => {
                    assert!(sampled.contains(&p), "page {p} was not sampled");
                    refused += 1;
                }
                Err(other) => panic!("byte {at}: {other:?}"),
                Ok(side) => {
                    assert!(!sampled.contains(&p), "byte {at} of sampled page {p}");
                    // Every key of the damaged page, and a few of the rest.
                    let mut hit = 0;
                    for (i, (k, vals, _)) in fx.recs.iter().enumerate() {
                        let e = side.fence.entry_of(k).unwrap();
                        let on_it = side.fence.page(e) == Some(p as u32);
                        if !on_it && i % 97 != 0 {
                            continue;
                        }
                        let mut got = Vec::new();
                        match side
                            .read_all(&fx.blob, k, &mut |v: &[u8]| got.push(v.to_vec()))
                            .unwrap()
                        {
                            ZRead::Served { .. } => assert_eq!(&got, vals, "key {i}"),
                            ZRead::Fallback | ZRead::Out => {
                                assert!(got.is_empty(), "key {i}: values before a decline");
                                hit += 1;
                            }
                        }
                    }
                    if hit > 0 {
                        assert!(side.damaged() && side.bad_pages() >= 1, "byte {at}");
                        declined += 1;
                    } else {
                        quiet += 1;
                    }
                }
            }
            f.write_at(&[good[at]], at as u64).unwrap();
        }
        assert!(refused > 0 && declined > 0, "{refused} {declined} {quiet}");
        // The bytes a page does not use are covered too: the CRC is over the
        // whole page, so a flip in the padding is damage and not a quiet pass.
        assert_eq!(quiet, 0, "a flip that no read of its page noticed");
    }

    #[test]
    fn the_fence_finds_the_entry_a_key_falls_among() {
        let fx = Fixture::new("fence", shapes(), 1 << 20);
        let side = fx.side();
        let f = &side.fence;
        // The empty key is the first record's, so nothing sorts below it.
        assert_eq!(f.key(0), Some(&b""[..]));
        assert_eq!(f.entry_of(b""), Some(0));
        for i in 0..f.len() {
            let k = f.key(i).unwrap().to_vec();
            assert_eq!(f.entry_of(&k), Some(i), "an entry's own key finds it");
            let past = [k.as_slice(), &[0u8]].concat();
            assert_eq!(f.entry_of(&past), Some(i), "a key just past it stays");
            if i + 1 < f.len() {
                let next = f.key(i + 1).unwrap();
                assert!(f.entry_of(next) == Some(i + 1));
            }
        }
        assert_eq!(f.entry_of(&[0xff; 64]), Some(f.len() - 1));
    }
}
