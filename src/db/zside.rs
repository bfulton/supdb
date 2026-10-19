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
//! A sidecar is installed into an empty cell and taken out of it only by
//! the writer's settle, when a page of it has failed a check and the
//! partition's is to be built again: what `take` removes is retired under
//! a fresh epoch and freed past every reader pinned at or before it, as a
//! replaced state is, so a reader that loaded the pointer inside an
//! operation keeps it for that operation. A cell with nothing in it is
//! freed with its segment.

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

/// A warm reads a sidecar's file into the page cache in chunks of this many
/// bytes, each asked for with `MADV_WILLNEED` and then touched a page at a
/// time, with a look at the cancel flag between chunks.
pub(super) const ZWARM_CHUNK: usize = 8 << 20;

/// The sidecar is written in pieces of this many bytes at their own
/// alignment, as a segment is (`SegmentOptions::write_piece`): the page
/// cache sizes a folio by the write that makes it. What the folios of a
/// built sidecar compose to is read from `/proc/self/pagemap` and
/// `/proc/kpageflags` over a mapping of the file; whether a smaller piece
/// would read better is for a pairing to say, and a plan's `piece` is where
/// it is changed.
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

/// The stored filter `words`, held to the running engine: the trailer's
/// signature and parameters are the engine's, the words are the size the
/// engine builds for `nkeys`, and every key of `fence` tests present. Each
/// failure is an identity refusal, since the file is whole and of something
/// this engine does not compute.
fn engine_filter(
    t: &Trailer,
    words: &[u8],
    fence: &ZFence,
    nkeys: usize,
) -> std::result::Result<super::BlockedBloom, Reject> {
    if !t.filter_matches_engine() {
        return Err(Reject::Identity(
            "the filter's layout is not this engine's".into(),
        ));
    }
    if words.len() != super::zpages::filter_bytes(nkeys) {
        return Err(Reject::Identity(
            "the filter is not the size this engine builds for the key count".into(),
        ));
    }
    let f = super::BlockedBloom::from_words(words)
        .ok_or_else(|| Reject::Identity("the filter is not whole blocks".into()))?;
    for i in 0..fence.len() {
        if fence.key(i).is_some_and(|k| !f.maybe_contains(k)) {
            return Err(Reject::Identity(
                "a key of the fence is not in the filter".into(),
            ));
        }
    }
    Ok(f)
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
    /// The key is a record the fence names no page for: one no page holds
    /// (`ZNOPAGE`), which the form was built without. The caller reads the
    /// raw form and counts a no-page read; not a failure and not a
    /// fallback, since the form said so from its fence. A key that falls
    /// in such an entry's range without being its record is answered absent
    /// from the fence, as a key below the first key is.
    NoPage,
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
    /// The key is a record the fence names no page for: the raw form's to
    /// read.
    NoPage,
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
    /// The filter the file stores, over every key of the partition: a
    /// read of a key it does not hold is answered absent from the heap,
    /// with no page touched. `None` for a sidecar built without one.
    filter: Option<super::BlockedBloom>,
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
    /// The filter the trailer says the file carries is read into the heap
    /// and held to the running engine before a read consults it: its
    /// signature, probes and bits a key are the engine's (`BLOOM_SIG`), its
    /// size is the one the engine builds for the key count, and every key
    /// of the fence tests present in it, which a filter of another layout
    /// over the same keys fails with certainty. A filter is a function of
    /// engine code the segment's checksum row cannot see, and a stored one
    /// whose layout the engine no longer computes would answer present keys
    /// absent with nothing raised, so each is an identity refusal.
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
        let (fence, filter) = ZFence::decode_resident(res, &t).map_err(damage)?;
        let filter = if t.filter {
            Some(engine_filter(&t, filter, &fence, nkeys)?)
        } else {
            None
        };
        let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
        let side = ZSide {
            map,
            advised: advise,
            fence,
            filter,
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
        // A key the filter does not hold is not in the partition: answered
        // from the heap, no page touched.
        if self.filter.as_ref().is_some_and(|f| !f.maybe_contains(key)) {
            return Locate::Absent { decoded: false };
        }
        // A key below the first key is not in the partition.
        let Some(e) = self.fence.entry_of(key) else {
            return Locate::Absent { decoded: false };
        };
        let Some(p) = self.fence.page(e) else {
            return Locate::Fallback;
        };
        // A record the form does not hold is the raw form's, and its
        // entry covers that one record, so a key in the entry's range that
        // is not the record's key is absent: the fence says so without a
        // page. An entry over more than one record is one this builder did
        // not write, and every key in it is the raw form's.
        if p == ZNOPAGE {
            return match self.fence.entry(e, self.nkeys as u32) {
                Some(entry) if entry.count == 1 && entry.key != key => {
                    Locate::Absent { decoded: false }
                }
                _ => Locate::NoPage,
            };
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
            Locate::NoPage => Ok(ZRead::NoPage),
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
            Locate::NoPage => ZRead::NoPage,
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

    /// Heap bytes the sidecar keeps resident: the fence and the filter.
    pub(super) fn resident_bytes(&self) -> usize {
        self.fence.heap_bytes() + self.filter.as_ref().map_or(0, |f| f.blocks.len() * 64)
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

/// The cell a segment keeps its sidecar in: a pointer set from null by
/// `install`, read by an acquire load, cleared by `take` -- whose caller
/// retires what it took through the epoch, as a replaced state is -- and
/// freed with the segment if it still holds anything. Not a `OnceLock`,
/// whose initialiser takes a lock a reader could wait on.
pub(super) struct ZCell {
    p: AtomicPtr<ZSide>,
}

impl ZCell {
    pub(super) const fn new() -> ZCell {
        ZCell {
            p: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// The sidecar, if one is installed. Valid for the operation that
    /// reads it, which pins an epoch: a sidecar `take` removed is retired
    /// under a later epoch and freed only past every pin at or before it,
    /// so a reader that loaded the pointer keeps it until it leaves.
    #[inline]
    pub(super) fn get(&self) -> Option<&ZSide> {
        let p = self.p.load(Ordering::Acquire);
        // SAFETY: set from `Box::into_raw` by `install`; cleared only by
        // `take`, whose caller frees it past every pinned reader, and by
        // `Drop`, which needs `&mut self`.
        unsafe { p.as_ref() }
    }

    /// The sidecar taken out of the cell, or null: the caller owns the
    /// pointer and retires it through the epoch (`RetiredZ`), since a
    /// reader pinned before the take may still be walking it.
    pub(super) fn take(&self) -> *mut ZSide {
        self.p.swap(std::ptr::null_mut(), Ordering::AcqRel)
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
    /// Failed builds tried again after their backoff; sidecars taken out
    /// of service by a page that failed a check, retired and built again;
    /// partitions declined for the life of their segment after the third
    /// such rebuild.
    pub build_retries: AtomicU64,
    pub rebuilds: AtomicU64,
    pub declined_damage: AtomicU64,
    /// Builds started on the builder's thread, and builds its cancel flag
    /// stopped: a merge took the target as an input, or the store closed.
    pub build_starts: AtomicU64,
    pub build_cancelled: AtomicU64,
    /// Records the builds held no page for, each an entry of the fence
    /// read from the raw form.
    pub build_nopage: AtomicU64,
    /// Warms of a sidecar's file (`warm_file`): how many, the bytes read,
    /// the bytes of chunks found resident and skipped, and the jobs' time
    /// in microseconds.
    pub warms: AtomicU64,
    pub warm_bytes: AtomicU64,
    pub warm_skipped: AtomicU64,
    pub warm_us: AtomicU64,
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
        ZEvents::add(&self.build_nopage, b.fill.nopages);
    }

    /// A warm, counted.
    pub(super) fn warmed(&self, w: &Warm) {
        ZEvents::add(&self.warms, 1);
        ZEvents::add(&self.warm_bytes, w.bytes);
        ZEvents::add(&self.warm_skipped, w.skipped);
        ZEvents::add(&self.warm_us, w.us);
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
    /// No record the partition holds fits a page: every one is an entry of
    /// no page, and a sidecar of those alone serves nothing.
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
/// for what the partition holds, and tried again once the segment's
/// backoff has passed (`Seg::z_retry_due`).
pub(super) const DECLINED_FAILED: u8 = 6;

/// The code `Seg::z_declined` keeps for a partition whose sidecar failed a
/// check after `ZREBUILDS` rebuilds: left raw for the segment's life, so a
/// defect the build cannot see or a medium that keeps failing is a counted
/// decline and not a build a cycle.
pub(super) const DECLINED_DAMAGE: u8 = 8;

/// How many times a segment's sidecar is built again after a page of it
/// failed a check.
pub(super) const ZREBUILDS: u8 = 3;

/// The name a retired sidecar is renamed to: a rename frees no inode, where
/// an unlink is a device round trip here, so the landing that retires a
/// segment renames and the idle tick unlinks; a dead name no live segment
/// claims, so a sweep at open or at the end of a settle removes what a
/// crash left.
pub(super) fn zpg_dead_name(n: u64) -> String {
    format!("zpg-dead-{n:08}.zpg")
}

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

/// What a warm of a sidecar's file did (`warm_file`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Warm {
    /// Bytes read into the page cache: the pages that were not resident.
    pub bytes: u64,
    /// Bytes of the chunks found wholly resident and left alone.
    pub skipped: u64,
    /// The job's elapsed time, in microseconds.
    pub us: u64,
}

/// `path` read into the page cache, chunk by chunk, so that the first reads
/// of a sidecar that is on disk and cold -- after a restart -- fault on
/// nothing: a cold file of `P` pages costs `r` reads `P(1 - e^(-r/P))`
/// major faults, as bad as the raw form. Each chunk the kernel reports
/// wholly resident is skipped; the rest is asked for with `MADV_WILLNEED`
/// and then read a page at a time, so the pages are there when this
/// returns and its time is the read's. `cancel`, looked at between chunks,
/// stops it where it is; what was read stays read.
pub(super) fn warm_file(path: &Path, cancel: Option<&AtomicBool>) -> Result<Warm> {
    let start = std::time::Instant::now();
    let map = MmapBytes::open(path)?;
    let len = map.0.len();
    let page = crate::bytes::os_page_size();
    let mut w = Warm::default();
    let mut sum = 0u64;
    let mut off = 0usize;
    while off < len {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            break;
        }
        let span = ZWARM_CHUNK.min(len - off);
        let pages = span.div_ceil(page);
        let resident = map.resident_pages(off, span);
        if resident >= pages {
            w.skipped += span as u64;
        } else {
            map.advise_willneed(off as u64, span);
            let mut at = off;
            while at < off + span {
                sum = sum.wrapping_add(u64::from(map.0[at]));
                at += page;
            }
            w.bytes += ((pages - resident) * page).min(span) as u64;
        }
        off += span;
    }
    std::hint::black_box(sum);
    w.us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    Ok(w)
}

#[derive(Debug)]
pub(super) enum Outcome {
    Built(Built),
    Declined(Decline),
    /// Stopped by its cancel flag; no file left.
    Cancelled,
}

/// Where a build's pieces go: in order, whole, one call a piece, at the
/// piece's own offset. The file for a build, a recorder for a test.
trait PieceSink {
    fn put(&mut self, at: u64, piece: &[u8]) -> Result<()>;
}

/// The build's sink: a piece written to the file whole, and handed to the
/// device with a hint at once, nothing waited for.
struct FileSink<'a>(&'a File);

impl PieceSink for FileSink<'_> {
    fn put(&mut self, at: u64, piece: &[u8]) -> Result<()> {
        let mut f = self.0;
        f.write_all(piece)?;
        super::start_writeback(self.0, at, piece.len() as u64);
        Ok(())
    }
}

/// Writes in whole pieces: bytes are buffered to `size` and the piece is
/// handed to the sink at its own alignment, nothing waited for. Every piece
/// is `size` bytes at a multiple of `size` but the last, which is what is
/// left: the page cache sizes a folio by the write that makes it and the
/// alignment of its offset, and a write of another shape is a file of other
/// folios with the same bytes.
struct Pieces<S: PieceSink> {
    sink: S,
    size: usize,
    buf: Vec<u8>,
    at: u64,
}

impl<S: PieceSink> Pieces<S> {
    fn new(sink: S, size: usize) -> Pieces<S> {
        assert!(size > 0, "a piece of no bytes");
        Pieces {
            sink,
            size,
            buf: Vec::with_capacity(size),
            at: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let room = self.size - self.buf.len();
            let take = room.min(rest.len());
            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buf.len() == self.size {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.sink.put(self.at, &self.buf)?;
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

/// What a build is asked, and the two sizes it works in. The floor and
/// whether the raw bytes are dropped are the store's options; the piece and
/// the window are the build's own constants, a plan's fields so that a test
/// can make them small and cross, with a store it can afford, the
/// boundaries a real one crosses.
#[derive(Clone, Debug)]
pub(super) struct BuildPlan {
    /// The record bytes below which the partition is declined
    /// (`Options::zpages_min_bytes`).
    pub floor: usize,
    /// Whether the raw records and directory are dropped from the cache
    /// behind the walk (`Options::zpages_drop_behind`).
    pub drop_behind: bool,
    /// The bytes of a write.
    pub piece: usize,
    /// The bytes of the raw record region passed between drops.
    pub window: usize,
    /// A build on a thread of its own looks here once a page: set, the
    /// build stops, removes its temp file and answers `Outcome::Cancelled`.
    /// A merge that takes the build's target as an input sets it.
    pub cancel: Option<std::sync::Arc<AtomicBool>>,
    /// A test's holds (`Db::hold_zpage_rename`): the build waits before its
    /// rename while the bit is set, so a test can land what it likes
    /// between a build's last look at the cancel and its file's name.
    pub hold: Option<std::sync::Arc<std::sync::atomic::AtomicU8>>,
}

impl BuildPlan {
    pub(super) fn new(floor: usize, drop_behind: bool) -> BuildPlan {
        BuildPlan {
            floor,
            drop_behind,
            piece: ZWRITE_PIECE,
            window: ZDROP_WINDOW,
            cancel: None,
            hold: None,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Acquire))
    }

    fn wait_rename(&self) {
        if let Some(h) = &self.hold {
            super::zbuild_hold_wait(h, super::ZHOLD_RENAME);
        }
    }
}

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
/// page cache behind the walk, a window at a time, where the plan says so.
///
/// A partition is declined, with no file written, for a record region
/// under the plan's floor, for a block table that is not empty (a run lies
/// outside its record), and when no record of it fits a page. A record no
/// page holds -- over the largest image a group decodes into, or one that
/// fits no page compressed or stored -- is an entry of the fence with no
/// page (`ZNOPAGE`), the raw form's to read, and the build goes on past it;
/// a key in that entry's range that is not the record's is absent from the
/// fence alone. A filter over every key, the ones no page holds included,
/// goes into region B beside the fence, with its signature and parameters
/// in the trailer. The temp name is removed on every way out the function
/// has, a decline after pages were written among them; a crash leaves it
/// for the sweep at open.
pub(super) fn build(
    seg_path: &Path,
    out: &Path,
    id: u64,
    end_seq: u64,
    plan: &BuildPlan,
) -> Result<Outcome> {
    build_seen(seg_path, out, id, end_seq, plan, &mut |_, _| {})
}

/// `build`, with `seen` called with each range of the segment's file the
/// build hands to the drop behind the walk, as offsets in the file and
/// before the range is dropped: a test's view of which bytes the build
/// meant to drop, which the page cache shows only as a sum.
fn build_seen(
    seg_path: &Path,
    out: &Path,
    id: u64,
    end_seq: u64,
    plan: &BuildPlan,
    seen: &mut dyn FnMut(usize, usize),
) -> Result<Outcome> {
    let file = File::open(seg_path)?;
    build_file_seen(file, out, id, end_seq, plan, seen)
}

/// `build` from a segment file already open: what the segment work hands a
/// build on a thread of its own, opened before the thread starts, so that a
/// promotion's unlink of the old name between the two does not fail it.
pub(super) fn build_file(
    file: File,
    out: &Path,
    id: u64,
    end_seq: u64,
    plan: &BuildPlan,
) -> Result<Outcome> {
    build_file_seen(file, out, id, end_seq, plan, &mut |_, _| {})
}

fn build_file_seen(
    file: File,
    out: &Path,
    id: u64,
    end_seq: u64,
    plan: &BuildPlan,
    seen: &mut dyn FnMut(usize, usize),
) -> Result<Outcome> {
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
    if recs_len < plan.floor {
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
        &file, base, recs, dir, nkeys, bind, &tmp, &facts, id, end_seq, plan, seen,
    );
    match made {
        Ok(Outcome::Built(b)) => {
            // The cancel's last look, before the file gets its name; a
            // build past it renames whatever landed meanwhile, and the
            // collect finds the target gone and the file an orphan.
            if plan.cancelled() {
                let _ = std::fs::remove_file(&tmp);
                return Ok(Outcome::Cancelled);
            }
            plan.wait_rename();
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
    plan: &BuildPlan,
    seen: &mut dyn FnMut(usize, usize),
) -> Result<Outcome> {
    let out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp)?;
    let mut w = Pieces::new(FileSink(&out), plan.piece);
    let images = SegImages::new(recs, dir, nkeys);
    let mut scratch = FillScratch::new();
    let mut verify_buf = GroupBuf::new();
    let mut fence = ZFence::new();
    let recs_at = recs.as_ptr() as usize - base;
    let dir_at = dir.as_ptr() as usize - base;
    let (mut rank, mut hint, mut pages) = (0usize, 0usize, 0u32);
    let mut total_ulen = 0u64;
    let mut dropped = 0usize;
    // The last record no page held, for the decline of a partition that
    // has nothing else.
    let mut last_nopage: Option<NoPage> = None;
    // Every key of the partition, the ones no page holds included, taken
    // from each record before the walk drops it from the cache: a read of a
    // key the filter rejects is answered absent from the heap.
    let mut bloom = super::BlockedBloom::with_capacity(nkeys);
    // The raw bytes the walk has passed, handed to the cache to forget.
    let mut forget = |from: usize, to: usize| {
        if plan.drop_behind && from < to {
            seen(from, to);
            drop_behind(base, file, from, to);
        }
    };
    while rank < nkeys {
        if plan.cancelled() {
            return Ok(Outcome::Cancelled);
        }
        let mut page = [0u8; ZPAGE];
        match fill_group(&images, rank, hint, bind, &mut scratch, &mut page) {
            Fill::Page(g) => {
                let first = images.image(rank);
                let key = record_key(first, 0)
                    .ok_or_else(|| super::err("z build: a record has no key"))?;
                let entry = FenceEntry {
                    index: fence.len(),
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
                for r in rank..rank + g.count {
                    let k = record_key(images.image(r), 0)
                        .ok_or_else(|| super::err("z build: a record has no key"))?;
                    bloom.insert(k);
                }
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
                if consumed.saturating_sub(dropped) >= plan.window {
                    forget(recs_at + dropped, recs_at + consumed);
                    dropped = consumed;
                }
            }
            Fill::NoPage(why) => {
                // A record no page holds is an entry of the fence with no
                // page: the read of it is the raw form's, and the pages
                // go on from the record after it. A partition of nothing
                // else is declined below.
                let first = images.image(rank);
                let key = record_key(first, 0)
                    .ok_or_else(|| super::err("z build: a record has no key"))?;
                let first_rank = u32::try_from(rank)
                    .map_err(|_| super::err("z build: a rank past four billion"))?;
                fence.push(first_rank, ZNOPAGE, key)?;
                bloom.insert(key);
                last_nopage = Some(why);
                rank += 1;
            }
            Fill::End => return Err(super::err("z build: the walk ran past the last record")),
            Fill::Mismatch => {
                return Err(super::err(
                    "z build: a page did not decode to the records it was built from",
                ))
            }
        }
    }
    forget(recs_at + dropped, recs_at + recs.len());
    forget(dir_at, dir_at + dir.len());
    if pages == 0 {
        return Ok(Outcome::Declined(Decline::NoPage(
            last_nopage.unwrap_or(NoPage::Unplaceable),
        )));
    }
    let (res, rf) = fence.encode_resident(&bloom.words())?;
    w.push(&res)?;
    let t = Trailer {
        n_pages: u64::from(pages),
        n_entries: fence.len() as u64,
        nkeys: nkeys as u64,
        res_off: u64::from(pages) * ZPAGE as u64,
        res_len: rf.res_len,
        keys_len: rf.keys_len,
        filter_off: rf.filter_off,
        filter_blocks: rf.filter_blocks,
        res_crc: rf.res_crc,
        filter: true,
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
        bloom_probes: super::BLOOM_PROBES,
        bloom_sig: super::BLOOM_SIG,
        bloom_bits_per_key: super::BLOOM_BITS_PER_KEY,
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
            build(&self.seg, &self.out, ID, END, &BuildPlan::new(0, true)).unwrap()
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
                ZRead::NoPage => panic!("every record of the fixture has a page"),
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
        let r = build(&fx.seg, &fx.out, ID, END, &BuildPlan::new(ZMIN_RECS, true)).unwrap();
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

        // A record no page holds -- random bytes that do not fit a page
        // compressed, an image over the cap a group decodes into -- is an
        // entry of the fence with no page, read from the raw form; the
        // partition is declined only when it holds nothing else.
        let mut rng = Rng(77);
        for (name, big, why) in [
            ("incompressible", rng.bytes(6000), NoPage::Incompressible),
            ("oversize", vec![0u8; 70_000], NoPage::Oversize),
        ] {
            let mut recs = suite(300, 3);
            recs.push((b"zzz-big".to_vec(), vec![big.clone()], false));
            let fx = Fixture::new(name, recs, 1 << 20);
            let Outcome::Built(b) = fx.build() else {
                panic!("{name}: built around the record no page holds")
            };
            assert_eq!(b.fill.nopages, 1, "{name}");
            let side = fx.attach().unwrap();
            assert_eq!(side.count(b"zzz-big"), ZRead::NoPage, "{name}");
            assert_eq!(
                z_values(&side, &fx.blob, b"zzz-big"),
                None,
                "{name}: the read is the raw form's"
            );
            agree(&side, &fx.blob, &fx.recs[..300]);
            let alone = Fixture::new(
                &format!("{name}-alone"),
                vec![(b"zzz-big".to_vec(), vec![big], false)],
                1 << 20,
            );
            assert!(
                matches!(alone.build(), Outcome::Declined(Decline::NoPage(w)) if w == why),
                "{name}: a partition of nothing a page holds is declined"
            );
            assert!(!alone.out.exists());
        }
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
                "damage: z form: the resident part is malformed: the filter is not where the entries end",
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
                "damage: z form: the resident part is malformed: the key offsets do not run from zero to the blob's length",
            ),
            (
                "filter_off",
                Box::new(|t| t.filter_off = 64),
                "damage: z form: the resident part is malformed: the filter is not where the entries end",
            ),
            (
                "filter_blocks",
                Box::new(|t| t.filter_blocks = 1),
                "damage: z form: the resident part is malformed: the filter is not where the entries end",
            ),
            (
                "res_crc",
                Box::new(|t| t.res_crc ^= 1),
                "damage: z form: the resident part's checksum does not match",
            ),
            (
                "filter",
                Box::new(|t| t.filter = false),
                "damage: z form: the trailer is inconsistent: the filter flag and the filter's blocks disagree",
            ),
            (
                "bloom_sig",
                Box::new(|t| t.bloom_sig = 9),
                "identity: the filter's layout is not this engine's",
            ),
            (
                "bloom_probes",
                Box::new(|t| t.bloom_probes = 9),
                "identity: the filter's layout is not this engine's",
            ),
            (
                "bloom_bits_per_key",
                Box::new(|t| t.bloom_bits_per_key = 9),
                "identity: the filter's layout is not this engine's",
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
        // The field no read depends on: the decoded bytes a trailer sums
        // for the ratio. A wrong value is accepted and harmless.
        std::fs::write(&fx.out, &good).unwrap();
        with_trailer(&fx.out, |t| t.total_ulen += 1);
        let side = fx.attach().unwrap_or_else(|e| panic!("total_ulen: {e:?}"));
        agree(&side, &fx.blob, &fx.recs);
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
                            ZRead::NoPage => panic!("every record of the fixture has a page"),
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

    // ---- the checks a good CRC does not make --------------------------------

    use crate::db::zpages::{encode_page, PageHeader, ZCAP};

    /// The images of page `p`'s records out of `bytes`, the whole sidecar.
    fn page_images(bytes: &[u8], p: usize, bind: u32) -> Vec<Vec<u8>> {
        let (h, payload) = parse_page(&bytes[p * ZPAGE..(p + 1) * ZPAGE], bind).unwrap();
        let mut buf = GroupBuf::new();
        let group = decode_group(&h, payload, &mut buf).unwrap().to_vec();
        let (mut out, mut off) = (Vec::new(), 0usize);
        for _ in 0..h.count {
            let (_, next) = crate::flatindex::record_key_next(&group, off).unwrap();
            out.push(group[off..next].to_vec());
            off = next;
        }
        assert_eq!(off, group.len());
        out
    }

    /// Page `p` of the sidecar at `path` rewritten in place as `images`, under
    /// the `first_rank` and `count` the caller names and with a CRC that
    /// passes: a page that is whole and is wrong. Written in place and never
    /// truncated, so a mapping a reader holds sees the page change.
    fn forge(path: &Path, p: usize, first_rank: u32, count: u16, images: &[Vec<u8>]) {
        let bytes = std::fs::read(path).unwrap();
        let t = Trailer::decode(&bytes[bytes.len() - ZTRAILER..]).unwrap();
        let group: Vec<u8> = images.concat();
        let payload = lz4_flex::block::compress(&group);
        assert!(payload.len() <= ZCAP, "the forged page fits");
        let h = PageHeader {
            first_rank,
            count,
            stored: false,
            ulen: group.len() as u32,
            clen: payload.len() as u16,
        };
        let mut page = [0u8; ZPAGE];
        encode_page(&mut page, &h, &payload, t.bind32()).unwrap();
        let f = OpenOptions::new().write(true).open(path).unwrap();
        f.write_all_at(&page, (p * ZPAGE) as u64).unwrap();
    }

    /// One record of a single inline value, as the writer lays it out.
    fn record_image(key: &[u8], value: &[u8]) -> Vec<u8> {
        let mut img = Vec::new();
        let e = Ext {
            block: Ext::INLINE,
            off: 0,
            len: value.len() as u32,
            last: 0,
            count: 1 | Ext::FIXED,
        };
        crate::flatindex::stream_record_as(&mut img, key, &[e], value, true).unwrap();
        img
    }

    /// The first page the attach does not sample, `skip` unsampled pages in.
    fn unsampled(pages: usize, skip: usize) -> usize {
        let sampled: Vec<usize> = (0..ZSAMPLE_PAGES)
            .map(|k| k * (pages - 1) / (ZSAMPLE_PAGES - 1))
            .collect();
        (1..pages - 1)
            .filter(|q| !sampled.contains(q))
            .nth(skip)
            .unwrap()
    }

    #[test]
    fn a_page_with_a_good_crc_and_the_wrong_records_is_never_an_answer() {
        // The CRC says the page is the bytes somebody wrote; it does not say
        // they are the page the fence names. Three ways to be wrong under a
        // good CRC, each of which a search of the group would answer without
        // noticing: a record short, two records in the wrong order, and a
        // first key that is not the fence's. The header's rank and count
        // against the entry, and the walk of the group, are what refuse
        // them, and the walk is what a read that skipped it would not have.
        let fx = Fixture::new("forged", suite(4000, 61), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = unsampled(t.n_pages as usize, 10);
        let side0 = fx.attach().unwrap();
        let entry = side0.fence.entry(p, side0.nkeys as u32).unwrap();
        let (first_rank, count) = (entry.first_rank, entry.count as u16);
        drop(side0);
        let images = page_images(&good, p, t.bind32());
        let n = images.len();
        assert_eq!(n, usize::from(count));
        let on_page: Vec<&Rec> = fx.recs[first_rank as usize..first_rank as usize + n]
            .iter()
            .collect();
        // The first record's key with a byte on the end: ascending, and not
        // the fence's key.
        let mut grown = on_page[0].0.clone();
        grown.push(0);
        let mut first_replaced = images.clone();
        first_replaced[0] = record_image(&grown, &on_page[0].1[0]);
        let mut swapped = images.clone();
        swapped.swap(0, 1);
        type Variant = (&'static str, u32, u16, Vec<Vec<u8>>);
        let variants: Vec<Variant> = vec![
            (
                "one record short",
                first_rank,
                count - 1,
                images[..n - 1].to_vec(),
            ),
            ("the first two swapped", first_rank, count, swapped),
            (
                "the first key is not the fence's",
                first_rank,
                count,
                first_replaced,
            ),
            // The right records under the wrong rank.
            ("the wrong rank", first_rank + 1, count, images.clone()),
        ];
        for (name, rank, cnt, imgs) in variants {
            std::fs::write(&fx.out, &good).unwrap();
            forge(&fx.out, p, rank, cnt, &imgs);
            let side = fx
                .attach()
                .expect("an unsampled page is not looked at by the attach");
            let mut declined = 0;
            for (k, vals, _) in &on_page {
                match z_values(&side, &fx.blob, k) {
                    Some(got) => assert_eq!(&got, vals, "{name}: {k:?} answered {got:?}"),
                    None => declined += 1,
                }
            }
            assert!(declined > 0, "{name}: nothing was refused");
            assert!(side.damaged(), "{name}: the sidecar stayed in service");
            assert_eq!(
                side.verified_pages(),
                0,
                "{name}: a refused page is verified"
            );
        }
    }

    #[test]
    fn a_sampled_page_with_a_good_crc_and_the_wrong_count_is_refused_at_attach() {
        // The attach's sample compares each page it reads with the fence
        // entry that names it, since a CRC that passes is no proof of that.
        let fx = Fixture::new("sampled", suite(4000, 62), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let images = page_images(&good, 0, t.bind32());
        let n = images.len();
        forge(&fx.out, 0, 0, (n - 1) as u16, &images[..n - 1]);
        assert!(
            matches!(fx.attach(), Err(Reject::Damage(_))),
            "a page of the sample that disagrees with its fence entry attached"
        );
        // And the rank, the other half of the compare.
        std::fs::write(&fx.out, &good).unwrap();
        forge(&fx.out, 0, 1, n as u16, &images);
        assert!(matches!(fx.attach(), Err(Reject::Damage(_))));
    }

    #[test]
    fn a_page_that_failed_its_first_decode_is_never_counted_verified() {
        // The verified bit is what lets a later read skip the page's CRC, so
        // it is set after every check has passed and not before: the sidecar
        // leaves service on the failure, which hides a bit set early from a
        // single reader, and a second reader that found the bit would serve
        // the page unchecked.
        let fx = Fixture::new("unverified", suite(4000, 63), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = unsampled(t.n_pages as usize, 5);
        let f = OpenOptions::new().write(true).open(&fx.out).unwrap();
        f.write_all_at(&[good[p * ZPAGE + 100] ^ 0x5a], (p * ZPAGE + 100) as u64)
            .unwrap();
        drop(f);
        let side = fx.attach().unwrap();
        let entry = side.fence.entry(p, side.nkeys as u32).unwrap();
        let key = fx.recs[entry.first_rank as usize + 1].0.clone();
        let mut got = 0;
        let r = side
            .read_all(&fx.blob, &key, &mut |_: &[u8]| got += 1)
            .unwrap();
        assert_eq!((r, got), (ZRead::Fallback, 0));
        assert_eq!(side.verified_pages(), 0, "a page that failed is verified");
        assert_eq!(side.bad_pages(), 1);
        assert!(side.damaged());
        // The sidecar is out of service, and what it is asked verifies nothing.
        assert_eq!(side.count(&fx.recs[0].0), ZRead::Out);
        assert_eq!(side.verified_pages(), 0);
    }

    #[test]
    fn a_verified_page_that_changes_underneath_is_never_answered_absent() {
        // Bit rot after the first decode: the page is read as verified, so
        // its CRC is not looked at again, and a group that no longer searches
        // is damage and not an absent key.
        let fx = Fixture::new("rot", suite(3000, 65), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = t.n_pages as usize / 2;
        let side = fx.attach().unwrap();
        let entry = side.fence.entry(p, side.nkeys as u32).unwrap();
        let (first_rank, count) = (entry.first_rank, entry.count as usize);
        let k = fx.recs[first_rank as usize + 3].clone();
        assert_eq!(z_values(&side, &fx.blob, &k.0), Some(k.1.clone()));
        assert_eq!(side.verified_pages(), 1);
        // The page rewritten whole under the open mapping: its last record is
        // cut in the middle, and the header still says `count` of them.
        let mut cut = page_images(&good, p, t.bind32());
        let last = cut.len() - 1;
        let half = cut[last].len() / 2 / 4 * 4;
        cut[last].truncate(half);
        forge(&fx.out, p, first_rank, count as u16, &cut);
        let (mut served, mut declined) = (0, 0);
        for i in 0..count {
            let (key, vals, _) = &fx.recs[first_rank as usize + i];
            match z_values(&side, &fx.blob, key) {
                Some(got) => {
                    assert_eq!(
                        &got, vals,
                        "{key:?} answered {got:?} after the page changed"
                    );
                    served += 1;
                }
                None => declined += 1,
            }
        }
        assert!(
            served > 0 && declined > 0,
            "{served} served, {declined} declined"
        );
        assert!(side.damaged() && side.bad_pages() == 1);
    }

    #[test]
    fn a_record_that_stops_parsing_after_its_page_was_verified_is_never_answered_absent() {
        // The search of a group reads a record's key and its length, which
        // for a compact record is the two bytes after the key and not the
        // header's other two; the record is parsed whole only once it is
        // found. A header the run contradicts is damage the search does not
        // see, and the page was verified before it came: the record that is
        // found and does not parse is the sidecar's damage, and a read that
        // took it for no record would answer a key it holds as missing.
        let fx = Fixture::new("rot-record", suite(3000, 68), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = t.n_pages as usize / 2;
        let side = fx.attach().unwrap();
        let entry = side.fence.entry(p, side.nkeys as u32).unwrap();
        let (first_rank, count) = (entry.first_rank, entry.count as u16);
        let (key, vals, _) = fx.recs[first_rank as usize + 3].clone();
        assert_eq!(z_values(&side, &fx.blob, &key), Some(vals.clone()));
        assert_eq!(side.verified_pages(), 1);
        let mut images = page_images(&good, p, t.bind32());
        // The compact header follows the key at the next multiple of four:
        // its run length, then its record count and flags, which here say
        // more records than the run has bytes.
        let at = (4 + key.len()).next_multiple_of(4);
        assert_eq!(
            u16::from_le_bytes([images[3][2], images[3][3]]),
            crate::flatindex::COMPACT,
            "a compact record"
        );
        images[3][at + 2] = 0xff;
        images[3][at + 3] = 0x1f;
        forge(&fx.out, p, first_rank, count, &images);
        let mut got = 0;
        let r = side
            .read_all(&fx.blob, &key, &mut |_: &[u8]| got += 1)
            .unwrap();
        assert_eq!((r, got), (ZRead::Fallback, 0));
        assert!(side.damaged() && side.bad_pages() == 1);
        // The records before it in the group were answered while it held.
        assert_eq!(side.count(&key), ZRead::Out);
    }

    #[test]
    fn a_second_install_is_refused() {
        let fx = Fixture::new("install", suite(2000, 67), 256);
        fx.side();
        let cell = ZCell::new();
        assert!(cell.get().is_none());
        assert!(cell.install(fx.attach().unwrap()));
        let first = cell.get().unwrap().id();
        assert!(
            !cell.install(fx.attach().unwrap()),
            "a cell holds one sidecar"
        );
        assert_eq!(cell.get().unwrap().id(), first);
    }

    #[test]
    fn the_trailer_sums_the_groups_and_the_file_is_the_length_the_build_returns() {
        let fx = Fixture::new("sums", suite(3000, 64), 256);
        let Outcome::Built(b) = fx.build() else {
            panic!("built")
        };
        let side = fx.attach().unwrap();
        let bytes = std::fs::read(&fx.out).unwrap();
        let mut sum = 0u64;
        for p in 0..side.pages() {
            sum += u64::from(
                page_view(&bytes[p * ZPAGE..(p + 1) * ZPAGE])
                    .unwrap()
                    .0
                    .ulen,
            );
        }
        assert_eq!(side.group_bytes(), sum);
        assert_eq!(b.total_ulen, sum);
        assert_eq!(side.file_bytes(), bytes.len() as u64);
        assert_eq!(b.file_bytes, bytes.len() as u64);
        assert_eq!(b.fill.pages as usize, side.pages());
    }

    // ---- what the build leaves behind ---------------------------------------

    /// The sidecar's temp name.
    fn tmp_of(out: &Path) -> PathBuf {
        let mut s = out.as_os_str().to_owned();
        s.push(".tmp");
        PathBuf::from(s)
    }

    #[test]
    fn a_build_over_records_out_of_order_fails_and_leaves_no_file() {
        // The builder runs the reader's own checks on every page it writes:
        // a segment whose directory names two records in each other's places
        // makes a group that is not in key order, which no checksum sees and
        // a search would answer wrong from. The build fails, and the failure
        // leaves neither the sidecar nor its temp name.
        let fx = Fixture::new("order", suite(3000, 63), 256);
        let map = MmapBytes::open(&fx.seg).unwrap();
        let base = map.slice_at(0, 1).unwrap().as_ptr() as usize;
        let blob = Blob::open(map).unwrap();
        let (_, dir) = blob.record_regions().unwrap();
        let dir_off = dir.as_ptr() as usize - base;
        drop(blob);
        // Two directory words swapped in a copy of the segment: ranks 40 and
        // 41 name each other's records.
        let mut copy = std::fs::read(&fx.seg).unwrap();
        let (a, b) = (dir_off + 4 * 40, dir_off + 4 * 41);
        for i in 0..4 {
            copy.swap(a + i, b + i);
        }
        let broken = fx.dir.join("par-00000008-0000000000000099--.sup");
        std::fs::write(&broken, copy).unwrap();
        let out = fx.dir.join(zpg_name(8, END));
        let r = build(&broken, &out, 8, END, &BuildPlan::new(0, true));
        assert!(r.is_err(), "records out of order built a sidecar: {r:?}");
        assert!(!out.exists());
        assert!(!tmp_of(&out).exists(), "a failed build left its temp name");
    }

    /// `p`'s pages dropped from the page cache: written back first, then
    /// `POSIX_FADV_DONTNEED`, which drops every clean page nothing maps.
    #[cfg(target_os = "linux")]
    fn evict(p: &Path) {
        use std::os::unix::io::AsRawFd;
        let f = File::open(p).unwrap();
        f.sync_all().unwrap();
        // SAFETY: a hint on an open descriptor.
        let rc = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        assert_eq!(rc, 0, "fadvise");
    }

    /// A sidecar's file evicted from the page cache and warmed: every page
    /// resident after, the bytes read the pages that were not; warmed
    /// again, nothing read and the whole file skipped; and a cancel set
    /// before it starts reads nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_warm_reads_the_cold_file_once_and_skips_a_resident_one() {
        let fx = Fixture::new("warm", suite(3000, 9), 1 << 20);
        let Outcome::Built(b) = fx.build() else {
            panic!("built")
        };
        let page = crate::bytes::os_page_size();
        let pages = |p: &Path| {
            let m = MmapBytes::open(p).unwrap();
            (m.resident_pages(0, m.0.len()), m.0.len().div_ceil(page))
        };
        evict(&fx.out);
        let (res, all) = pages(&fx.out);
        assert!(res < all / 4, "{res} of {all} resident after the eviction");
        let w = warm_file(&fx.out, None).unwrap();
        assert_eq!(
            (w.bytes, w.skipped),
            (b.file_bytes - (res * page) as u64, 0),
            "{w:?}"
        );
        let (res, all) = pages(&fx.out);
        assert_eq!(res, all, "resident after the warm");
        let again = warm_file(&fx.out, None).unwrap();
        assert_eq!((again.bytes, again.skipped), (0, b.file_bytes), "{again:?}");
        evict(&fx.out);
        let cancel = AtomicBool::new(true);
        let none = warm_file(&fx.out, Some(&cancel)).unwrap();
        assert_eq!((none.bytes, none.skipped), (0, 0), "{none:?}");
        let (res, all) = pages(&fx.out);
        assert!(
            res < all / 4,
            "{res} of {all} resident after a cancelled warm"
        );
    }

    #[test]
    fn a_record_no_page_holds_among_the_pages_is_one_entry_and_the_pages_go_on() {
        // The pages before the record are in the temp file when the fill
        // meets it; the entry is pushed and the pages after it follow, so
        // the sidecar is whole and the temp name is gone.
        let mut rng = Rng(77);
        let mut recs = suite(3000, 3);
        let big = b"0000000000001500-big";
        recs.insert(1500, (big.to_vec(), vec![rng.bytes(6000)], false));
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(recs[1501].0, big, "the record sorts among the suite's keys");
        let fx = Fixture::new("late-incompressible", recs, 1 << 20);
        let Outcome::Built(b) = fx.build() else {
            panic!("built")
        };
        assert_eq!(b.fill.nopages, 1);
        assert!(fx.out.exists() && !tmp_of(&fx.out).exists());
        let side = fx.attach().unwrap();
        agree(
            &side,
            &fx.blob,
            &fx.recs
                .iter()
                .filter(|r| r.0 != big)
                .cloned()
                .collect::<Vec<_>>(),
        );
        assert_eq!(side.count(big), ZRead::NoPage);
        // A key in the entry's range that is not the record's is absent
        // from the fence alone, and the record's own key is the raw form's.
        let above = [&big[..], &[0xffu8]].concat();
        assert_eq!(
            side.count(&above),
            ZRead::Served {
                n: 0,
                decoded: false
            }
        );
        assert_eq!(z_values(&side, &fx.blob, big), None);
        // The entry's page number is no page, and the pages after it are
        // numbered on from the one before.
        let e = side.fence.entry_of(big).unwrap();
        assert_eq!(side.fence.page(e), Some(ZNOPAGE));
        assert_eq!(
            side.fence.page(e + 1),
            side.fence.page(e - 1).map(|p| p + 1)
        );
    }

    #[test]
    fn a_build_over_a_stale_temp_file_is_a_whole_sidecar() {
        // A build a crash cut off leaves its temp name, and the next build of
        // the same name writes over it: from the first byte, and to the
        // length the sidecar is, or the file renamed into place carries what
        // the crashed build had after its end.
        let fx = Fixture::new("stale", suite(3000, 66), 256);
        std::fs::write(tmp_of(&fx.out), vec![0xA5u8; 4 << 20]).unwrap();
        assert!(matches!(fx.build(), Outcome::Built(_)));
        let side = fx
            .attach()
            .expect("a sidecar built over a stale temp file attaches");
        agree(&side, &fx.blob, &fx.recs);
        assert!(!tmp_of(&fx.out).exists());
    }

    /// A sink that keeps what it is given.
    struct Recorder(Vec<(u64, Vec<u8>)>);

    impl PieceSink for Recorder {
        fn put(&mut self, at: u64, piece: &[u8]) -> Result<()> {
            self.0.push((at, piece.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn pieces_are_cut_whole_at_their_own_offsets() {
        let mut rng = Rng(0x91ece);
        let stream = rng.bytes(10_000);
        for size in [1usize, 7, 4096, 4097] {
            for chunk in [1usize, 5, 4096, 9_999, 10_000] {
                let mut w = Pieces::new(Recorder(Vec::new()), size);
                for part in stream.chunks(chunk) {
                    w.push(part).unwrap();
                }
                // Nothing is written until a piece is whole, and the last
                // piece is what is left.
                let whole = stream.len() / size;
                assert_eq!(w.sink.0.len(), whole, "size {size}, chunk {chunk}");
                w.flush().unwrap();
                w.flush().unwrap();
                let got = &w.sink.0;
                assert_eq!(got.len(), stream.len().div_ceil(size));
                for (i, (at, piece)) in got.iter().enumerate() {
                    assert_eq!(*at, (i * size) as u64, "size {size}, chunk {chunk}");
                    let want = size.min(stream.len() - i * size);
                    assert_eq!(piece.len(), want, "piece {i} of size {size}");
                }
                let joined: Vec<u8> = got.iter().flat_map(|(_, p)| p.iter().copied()).collect();
                assert_eq!(joined, stream, "size {size}, chunk {chunk}");
                assert_eq!(w.at, stream.len() as u64);
            }
        }
        // Nothing pushed, nothing written.
        let mut w = Pieces::new(Recorder(Vec::new()), 4096);
        w.flush().unwrap();
        assert!(w.sink.0.is_empty() && w.at == 0);
    }

    #[test]
    fn a_sidecar_is_the_same_bytes_whatever_the_piece_and_the_window() {
        // The piece a build writes in and the window it drops the raw records
        // behind are choices about the page cache; the file is the same. The
        // plans cross, with a store a test can afford, the boundaries a real
        // build crosses: a piece smaller than the file, a window smaller than
        // the records, a piece that is not a whole number of pages.
        let fx = Fixture::new("plans", suite(6000, 71), 256);
        let map = MmapBytes::open(&fx.seg).unwrap();
        let base = map.slice_at(0, 1).unwrap().as_ptr() as usize;
        let blob = Blob::open(map).unwrap();
        let (recs, dir) = blob.record_regions().unwrap();
        let recs_at = recs.as_ptr() as usize - base;
        let (recs_end, dir_at) = (recs_at + recs.len(), dir.as_ptr() as usize - base);
        let dir_end = dir_at + dir.len();
        drop(blob);
        let mut reference: Option<Vec<u8>> = None;
        let plans = [
            (ZWRITE_PIECE, ZDROP_WINDOW, true),
            (ZPAGE, ZPAGE, true),
            (3 * ZPAGE, 5 * ZPAGE + 100, true),
            (7 * ZPAGE + 12, 64 << 10, true),
            (ZWRITE_PIECE, 64 << 10, false),
        ];
        for (piece, window, drop_behind) in plans {
            let _ = std::fs::remove_file(&fx.out);
            let plan = BuildPlan {
                piece,
                window,
                ..BuildPlan::new(0, drop_behind)
            };
            let mut seen: Vec<(usize, usize)> = Vec::new();
            let r = build_seen(&fx.seg, &fx.out, ID, END, &plan, &mut |a, b| {
                seen.push((a, b))
            })
            .unwrap();
            assert!(matches!(r, Outcome::Built(_)));
            let bytes = std::fs::read(&fx.out).unwrap();
            match &reference {
                None => reference = Some(bytes),
                Some(want) => assert_eq!(&bytes, want, "piece {piece}, window {window}"),
            }
            if !drop_behind {
                assert!(
                    seen.is_empty(),
                    "a build asked not to drop dropped {seen:?}"
                );
                continue;
            }
            // The directory last; the records before it, in order, each range
            // beginning where the one before ended and every range but the
            // last at least a window; together the whole region and no byte
            // of anything else.
            let (dirs, recs_seen) = seen.split_last().expect("something was dropped");
            assert_eq!(*dirs, (dir_at, dir_end), "the directory is dropped last");
            let mut at = recs_at;
            for (i, &(a, b)) in recs_seen.iter().enumerate() {
                assert_eq!(a, at, "range {i} begins where the one before ended");
                assert!(b > a, "range {i} is empty");
                if i + 1 < recs_seen.len() {
                    assert!(
                        b - a >= window,
                        "range {i} is {} bytes of a {window} window",
                        b - a
                    );
                }
                at = b;
            }
            assert_eq!(at, recs_end, "the records are dropped to their end");
            if window <= 64 << 10 {
                assert!(
                    recs_seen.len() > 1,
                    "a window under the region drops in turns"
                );
            } else {
                assert_eq!(
                    recs_seen.len(),
                    1,
                    "a window over the region drops once, at the end"
                );
            }
        }
    }

    // ---- the arms of a read ---------------------------------------------------

    /// The file's bytes in a buffer whose start is `extra` bytes past a
    /// multiple of eight, and where the file begins in it.
    fn file_at(bytes: &[u8], extra: usize) -> (Vec<u8>, usize) {
        let mut backing = vec![0u8; bytes.len() + 16];
        let start = (8 - backing.as_ptr() as usize % 8) % 8 + extra;
        backing[start..start + bytes.len()].copy_from_slice(bytes);
        (backing, start)
    }

    #[test]
    fn a_page_the_sidecar_cannot_lend_is_refused_by_a_read_and_not_answered_absent() {
        // Entropy: every page is stored, parsed where it lies, and a stored
        // page's payload has to sit where the extents `parse_record` borrows
        // are aligned. A mapping never gives one that does not; a source that
        // does is refused, and refused by the read and not only by the
        // function under it: the arm that handles the refusal is the one that
        // could answer a held key as missing.
        let mut rng = Rng(0xfeed);
        let mut recs: Vec<Rec> = (0..1500)
            .map(|_| (rng.bytes(16), vec![rng.bytes(100)], false))
            .collect();
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        recs.dedup_by(|a, b| a.0 == b.0);
        let fx = Fixture::new("lend", recs, 256);
        fx.side();
        let bytes = std::fs::read(&fx.out).unwrap();
        // A sidecar fresh from its attach: no page is verified yet, so a read
        // that meets a page meets it in its first decode.
        let mut side = fx.attach().unwrap();
        // Which pages are stored: entropy makes most of them so, and a group
        // that LZ4 does shorten is decoded into a buffer of the reader's own,
        // which is aligned whatever the source is.
        let stored = |side: &ZSide, k: &[u8]| {
            let p = side.fence.page(side.fence.entry_of(k).unwrap()).unwrap() as usize;
            page_view(&bytes[p * ZPAGE..(p + 1) * ZPAGE])
                .unwrap()
                .0
                .stored
        };
        let on_stored = fx.recs.iter().filter(|r| stored(&side, &r.0)).count();
        assert!(
            on_stored > 100 && fx.recs.len() - on_stored > 100,
            "{on_stored} of {} records on stored pages: the test wants both kinds",
            fx.recs.len()
        );
        // Misaligned by every skew that is not a multiple of four: a read
        // and a count of a key on a stored page both decline, and the sidecar
        // stays in service, since a source that cannot lend is no damage to
        // the file; a key on any other page is served as it is.
        let refused = |side: &ZSide, what: &str| {
            let (mut refusals, mut answers) = (0, 0);
            for (k, vals, _) in fx.recs.iter().step_by(11) {
                let mut got = Vec::new();
                let r = side
                    .read_all(&fx.blob, k, &mut |v: &[u8]| got.push(v.to_vec()))
                    .unwrap();
                if stored(side, k) {
                    assert_eq!(
                        (r, got.len()),
                        (ZRead::Fallback, 0),
                        "{what}: read of {k:?}"
                    );
                    assert_eq!(side.count(k), ZRead::Fallback, "{what}: count of {k:?}");
                    refusals += 1;
                } else {
                    assert_eq!(&got, vals, "{what}: read of {k:?}");
                    assert_eq!(served(side.count(k)), vals.len() as u64);
                    answers += 1;
                }
            }
            assert!(
                refusals > 10 && answers > 0,
                "{refusals} refused, {answers} answered"
            );
            assert!(!side.damaged() && side.bad_pages() == 0, "{what}");
        };
        for extra in [1usize, 2, 3, 5, 6, 7] {
            side.skewed = Some(file_at(&bytes, extra));
            refused(&side, &format!("first decode, skew {extra}"));
        }
        for p in 0..side.pages() {
            let stored_page = page_view(&bytes[p * ZPAGE..(p + 1) * ZPAGE])
                .unwrap()
                .0
                .stored;
            assert!(
                !(stored_page && bit(&side.verified, p)),
                "page {p} was refused and is verified"
            );
        }
        // Aligned, from a buffer of the test's own: every key is served, a
        // stored page in place with nothing decoded.
        side.skewed = Some(file_at(&bytes, 0));
        for (k, vals, _) in &fx.recs {
            let mut got = Vec::new();
            let r = side
                .read_all(&fx.blob, k, &mut |v: &[u8]| got.push(v.to_vec()))
                .unwrap();
            let want = ZRead::Served {
                n: vals.len() as u64,
                decoded: !stored(&side, k),
            };
            assert_eq!(r, want);
            assert_eq!(&got, vals);
        }
        // The pages are verified now, and a read that meets one in the
        // verified path is refused the same way.
        for extra in [1usize, 3, 6] {
            side.skewed = Some(file_at(&bytes, extra));
            refused(&side, &format!("verified, skew {extra}"));
        }
        // A source that has no such page at all is the sidecar's damage.
        side.skewed = Some((Vec::new(), 0));
        let k = &fx.recs[fx.recs.len() / 2].0;
        assert_eq!(side.count(k), ZRead::Fallback);
        assert!(side.damaged() && side.bad_pages() == 1);
        assert_eq!(side.count(k), ZRead::Out);
    }

    #[test]
    fn records_the_form_holds_no_page_for_are_the_raw_forms_and_pages_are_found_by_number() {
        // A fence entry that names no page (`ZNOPAGE`) is a run of records the
        // form was built without, and a read of them is the raw form's: the
        // sidecar says so and does not answer absent. The entries after it
        // are not at their own index in the file: the page of entry `i` is
        // the number the fence gives, and a reader that took the entry's
        // index for it would open another entry's page and find the damage
        // check, rightly, unwilling.
        let fx = Fixture::new("nopage", suite(4000, 81), 256);
        fx.side();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let (fence, filter) =
            ZFence::decode_resident(&good[t.res_off as usize..good.len() - ZTRAILER], &t).unwrap();
        let (pages, k) = (t.n_pages as usize, 5usize);
        assert!(pages > k + 3);
        let mut bytes: Vec<u8> = Vec::new();
        for p in (0..pages).filter(|&p| p != k) {
            bytes.extend_from_slice(&good[p * ZPAGE..(p + 1) * ZPAGE]);
        }
        let mut cut = ZFence::new();
        let mut next = 0u32;
        for i in 0..fence.len() {
            let page = if i == k {
                ZNOPAGE
            } else {
                next += 1;
                next - 1
            };
            cut.push(fence.first_rank(i).unwrap(), page, fence.key(i).unwrap())
                .unwrap();
        }
        // The filter goes with the fence: the keys are the same.
        let (res, rf) = cut.encode_resident(filter).unwrap();
        bytes.extend_from_slice(&res);
        let dropped = page_view(&good[k * ZPAGE..(k + 1) * ZPAGE]).unwrap().0;
        let mut t2 = t.clone();
        t2.n_pages -= 1;
        t2.res_off = t2.n_pages * ZPAGE as u64;
        t2.res_len = rf.res_len;
        t2.res_crc = rf.res_crc;
        t2.filter_off = rf.filter_off;
        t2.filter_blocks = rf.filter_blocks;
        t2.total_ulen -= u64::from(dropped.ulen);
        bytes.extend_from_slice(&t2.encode());
        std::fs::write(&fx.out, &bytes).unwrap();
        let side = fx
            .attach()
            .expect("a fence with an entry of no page attaches");
        let (lo, hi) = (
            fence.first_rank(k).unwrap() as usize,
            fence.first_rank(k + 1).unwrap() as usize,
        );
        assert_eq!(hi - lo, usize::from(dropped.count));
        let (mut served, mut declined) = (0, 0);
        for (i, (key, vals, _)) in fx.recs.iter().enumerate() {
            let mut got = Vec::new();
            let r = side
                .read_all(&fx.blob, key, &mut |v: &[u8]| got.push(v.to_vec()))
                .unwrap();
            if (lo..hi).contains(&i) {
                assert_eq!((r, got.len()), (ZRead::NoPage, 0), "record {i}");
                assert_eq!(side.count(key), ZRead::NoPage, "count of record {i}");
                declined += 1;
            } else {
                assert_eq!(&got, vals, "record {i}");
                assert!(matches!(r, ZRead::Served { .. }), "record {i}: {r:?}");
                served += 1;
            }
        }
        assert_eq!(declined, hi - lo);
        assert!(served > 3 * declined, "{served} served");
        assert!(!side.damaged() && side.bad_pages() == 0);
        // A key that falls in the run and is no record is the filter's to
        // answer, before the fence is asked: absent, no page touched, or
        // the fence's answer where the filter admits it by chance. The
        // fence's own answer for such a key is the raw form's, since the
        // entry covers more than one record: asked with the filter set
        // aside.
        let inside = [fx.recs[lo].0.as_slice(), &[0u8]].concat();
        let answer = side.count(&inside);
        assert!(
            answer
                == ZRead::Served {
                    n: 0,
                    decoded: false
                }
                || answer == ZRead::NoPage,
            "{answer:?}"
        );
        let mut side = side;
        side.filter = None;
        assert_eq!(
            side.count(&inside),
            ZRead::NoPage,
            "the fence's own answer for a key in a run of several records"
        );
    }
}
