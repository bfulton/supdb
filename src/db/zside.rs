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
//! Retirement through the epoch, which a later stage adds, changes that and
//! changes `ZCell::get` with it.

use super::zpages::{
    bind32, decode_group, fill_group, group_seek, page_view, parse_page, verify_page, walk_group,
    FenceEntry, Fill, FillScratch, FillStats, GroupBuf, Images, NoPage, SegImages, Trailer, ZError,
    ZFence, ZNOPAGE, ZPAGE, ZTRAILER,
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

/// The sidecar a segment named `seg` would have, or `None` for a name that
/// is not a segment's.
pub(super) fn zpg_name_for(seg: &str) -> Option<String> {
    Some(zpg_name(
        super::Db::name_id(seg)?,
        super::Db::name_end_seq(seg)?,
    ))
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
    /// was decoded to get there, which a stored page and an absent key are
    /// not.
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
    /// among the records of the page it would be in.
    Absent,
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
pub(super) fn first_decode<'a>(
    page: &'a [u8],
    bind: u32,
    entry: &FenceEntry<'_>,
    buf: &'a mut GroupBuf,
) -> std::result::Result<(&'a [u8], usize, bool), Fail> {
    let (h, payload) = parse_page(page, bind)?;
    if h.first_rank != entry.first_rank || u32::from(h.count) != entry.count {
        return Err(Fail::Check(ZError::Fence));
    }
    if h.stored && !aligned_for_extents(payload) {
        return Err(Fail::Misaligned);
    }
    let group = decode_group(&h, payload, buf)?;
    walk_group(group, usize::from(h.count), Some(entry.key))?;
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
    /// the same).
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
    /// The file, mapped. Its own advice: random, as the store's point reads
    /// are, where the store's mode is.
    map: MmapBytes,
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
    /// The filter a trailer may carry is not read: this stage builds none
    /// and answers no read from one.
    pub(super) fn attach(
        path: &Path,
        blob: &Blob<MmapBytes>,
        id: u64,
        end_seq: u64,
        random: bool,
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
        };
        side.check_sample().map_err(damage)?;
        if random {
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
            if h.first_rank != entry.first_rank || u32::from(h.count) != entry.count {
                return Err(ZError::Fence);
            }
        }
        Ok(())
    }

    fn page_bytes(&self, p: usize) -> Option<&[u8]> {
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
            return Locate::Absent;
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
            return Locate::Absent;
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
            Locate::Absent => Ok(ZRead::Served {
                n: 0,
                decoded: false,
            }),
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
            Locate::Absent => ZRead::Served {
                n: 0,
                decoded: false,
            },
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
/// segment equals what the segment recomputes of itself. The first mismatch
/// is named.
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
    if t.seg_row_crc != f.row_crc {
        return Err("the key section's checksum row differs");
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
            Decline::Small(_) => 1,
            Decline::BlockBacked => 2,
            Decline::NoPage(NoPage::Oversize) => 3,
            Decline::NoPage(NoPage::Incompressible) => 4,
            Decline::NoPage(NoPage::Unplaceable) => 5,
        }
    }
}

/// The code `Seg::z_declined` keeps for a build that failed: not declined
/// for what the partition holds, but not tried again by this segment.
pub(super) const DECLINED_FAILED: u8 = 6;

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
