//! The z form's engine side: the form file mapped and checked (`ZFile`), the
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
//! answer or no answer: a read the form file cannot serve says so
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
//! A form file is installed into an empty cell and taken out of it only by
//! the writer's settle, when a page of it has failed a check and the
//! partition's is to be built again: what `take` removes is retired under
//! a fresh epoch and freed past every reader pinned at or before it, as a
//! replaced state is, so a reader that loaded the pointer inside an
//! operation keeps it for that operation. A cell with nothing in it is
//! freed with its segment.

use super::zpages::{
    bind32, check_coded_group, check_entry, check_group, cmp_coded_key, coded_first, coded_header,
    coded_key_into, coded_rec, decode_group_with, decode_record, fill_page, filter_bytes,
    group_seek, page_view, parse_page, record_group_seek, verify_page_with, view_trusted,
    walk_group, FenceEntry, Fill, FillScratch, FillStats, GroupBuf, Images, NoPage, PageView,
    SegImages, Trailer, ZError, ZFence, ZCODING_BLOCK, ZGROUP_BYTES, ZMAX_GROUPS, ZNOPAGE, ZPAGE,
    ZTRAILER,
};
use crate::blob::{Blob, BlobOptions, ZSource};
use crate::bytes::{Bytes, MmapBytes};
use crate::flatindex::{parse_record, parse_record_tail, rd_u32, record_key, record_key_next};
use crate::index::Ext;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering as KeyOrder;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Result, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;

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

/// A warm reads a form file into the page cache in chunks of this many
/// bytes, each asked for with `MADV_WILLNEED` and then touched a page at a
/// time, with a look at the cancel flag between chunks.
pub(super) const ZWARM_CHUNK: usize = 8 << 20;

/// The form file is written in pieces of this many bytes at their own
/// alignment, as a segment is (`SegmentOptions::write_piece`): the page
/// cache sizes a folio by the write that makes it. What the folios of a
/// built form file compose to is read from `/proc/self/pagemap` and
/// `/proc/kpageflags` over a mapping of the file; whether a smaller piece
/// would read better is for a pairing to say, and a plan's `piece` is where
/// it is changed.
const ZWRITE_PIECE: usize = 1 << 20;

/// The window of the raw records the build drops behind itself.
const ZDROP_WINDOW: usize = 8 << 20;

/// The window a kept form file put back in service is asked for in
/// (`ZFile::prefetch_all`): the device's largest request here
/// (`max_sectors_kb`), which is what the kernel submits of one advice.
const ZPREFETCH_WINDOW: usize = 4 << 20;

/// Pages whose checksums an attach reads: the first, the last and the ones
/// evenly spaced between, sixteen faults a file, once.
const ZSAMPLE_PAGES: usize = 16;

/// The form file's name: the two fields of the segment's name that a
/// promotion keeps, as the ordered index's (`Db::ord_name`), so a
/// promotion's link names no new file.
pub(super) fn zpg_name(id: u64, end_seq: u64) -> String {
    format!("zpg-{id:08}-{end_seq:016}.zpg")
}

/// The two fields of the segment named `seg` that its form file's name is
/// made of, or `None` for a name that is not a segment's. Two segments with
/// the same fields address one form file -- a promotion keeps them -- and
/// comparing the fields is the claim test, with no name made to compare.
pub(super) fn zpg_fields(seg: &str) -> Option<(u64, u64)> {
    Some((super::Db::name_id(seg)?, super::Db::name_end_seq(seg)?))
}

/// The form file a segment named `seg` would have, or `None` for a name that
/// is not a segment's.
pub(super) fn zpg_name_for(seg: &str) -> Option<String> {
    let (id, end_seq) = zpg_fields(seg)?;
    Some(zpg_name(id, end_seq))
}

/// Whether `name` is one of the form files or a build's temp name:
/// what a sweep of a store's directory takes for the form's.
pub(super) fn is_zpg_file(name: &str) -> bool {
    name.starts_with("zpg-")
}

/// Why an attach refused a file.
#[derive(Debug)]
pub(super) enum Reject {
    /// There is no file. Not a rejection: nothing was there to refuse.
    Missing,
    /// A file that is not a form file anyone can use: short, torn, a trailer
    /// or a resident part or a sampled page that does not check.
    Damage(String),
    /// A form file, whole, of something else: another segment, a record
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
    /// The form file answered: `n` values (or the count), and whether a page
    /// was decoded to get there. A page is decoded for every key that falls
    /// in its range, the key it holds and the key it does not alike, unless
    /// the thread's memo holds its group from a decode before (`Memo`); a
    /// stored page is parsed where it lies and decodes nothing, and a key
    /// below the partition's first key is answered from the fence, touching
    /// no page.
    Served { n: u64, decoded: bool },
    /// A check failed on this read: the caller reads the raw form and
    /// counts a fallback.
    Fallback,
    /// The form file is out of service: the caller reads the raw form and
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

/// What a scan of a covered partition did (`ZFile::scan_at`): the keys
/// visited, as `Blob::scan_at` counts them; the groups decoded for them;
/// the runs of records no page holds that the raw form walked; and how it
/// ended -- served whole, or stopped at a record boundary by a check that
/// failed or a form file out of service, in which case the caller walks the
/// rest from `seen` on through the raw form, since a walk that stops
/// between two records has emitted every value of the ones before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ZScan {
    pub seen: usize,
    pub decoded: u64,
    /// Groups read in place, record by record, without a decode
    /// (`Opened::Coded`).
    pub inplace: u64,
    pub nopage: u64,
    /// Whether the walk asked for the pages after its first at once
    /// (`Options::zpages_scan_prefetch`).
    pub prefetches: u64,
    pub end: ScanEnd,
}

/// How a scan through the form file ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScanEnd {
    /// Every key asked for, or every key the partition has from the rank.
    Served,
    /// A check failed: the form file leaves service, and the rest is the raw
    /// form's.
    Fallback,
    /// The form file was out of service before the first record.
    Out,
}

/// What a seek or a cut through a form file answered (`ZFile::seek`,
/// `ZFile::cut_in`, `ZCursor::seek`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ZSeek {
    /// The rank; how the key there compares with the key sought, `Equal`
    /// for the key itself and `Greater` for the first key past it or the
    /// end; and whether a page was decoded to find it.
    Served {
        rank: usize,
        at: KeyOrder,
        decoded: bool,
    },
    /// A check failed: the form file leaves service, and the caller asks the
    /// raw form and counts a fallback.
    Fallback,
    /// The form file is out of service: the raw form's.
    Out,
    /// The key falls in an entry no page holds that covers more than one
    /// record, which this builder does not write: the raw form's to say,
    /// as a read of such an entry is.
    NoPage,
}

/// Where a cursor's group is: parsed in place in a stored page of the
/// mapping, decoded in the cursor's own buffer, or nowhere -- a run of
/// records no page holds, or a form file that failed a check under the
/// cursor, both read through the raw form -- or past the end.
enum Group<'a> {
    None,
    Page(&'a [u8]),
    Buf(usize),
    /// The memo's, `n` bytes of it, held for the group's life in the
    /// cursor.
    Memo(Arc<GroupBuf>, usize),
    /// A record-coded group whose tails are stored raw, held as its payload
    /// where it lies in the mapping and read record by record in place
    /// (`ZFile::inplace`): the position is a record index, the key is
    /// assembled from the first record's head and the record's suffix into
    /// the cursor's `keybuf`, and the values are parsed from the raw tail
    /// (`parse_record_tail`), so a walk decodes nothing. A record whose tail
    /// is compressed after all, or whose extents cannot be read where they
    /// lie, is decoded on its own into the cursor's buffer.
    Coded(&'a [u8], usize),
    Raw,
}

/// Whether a record-coded group's tails are raw, judged from its last
/// record: the first is stored raw whatever the coding, and a group built
/// under `raw_tails` has every tail raw. A group judged raw with a
/// compressed tail inside it, which the record coding without `raw_tails`
/// leaves where LZ4 did not shrink one, is still read right: that record is
/// decoded on its own.
fn raw_tail_group(payload: &[u8], count: usize) -> bool {
    count >= 1
        && coded_rec(payload, count, count - 1)
            .map(|r| r.tail_is_raw())
            .unwrap_or(false)
}

/// A raw-tail coded group where it lies: its payload, its record count and
/// the first record's head, which every other record's key is assembled
/// against (`coded_record_to`).
#[derive(Clone, Copy)]
struct CodedGroup<'a> {
    payload: &'a [u8],
    count: usize,
    head0: &'a [u8],
}

impl<'a> CodedGroup<'a> {
    /// The group over `payload`, or the error of a first record that does
    /// not parse.
    fn open(payload: &'a [u8], count: usize) -> std::result::Result<Self, ZError> {
        let (head0, _) = coded_first(payload, count)?;
        Ok(CodedGroup {
            payload,
            count,
            head0,
        })
    }
}

/// Record `i` of a coded group handed to `f` as the walk hands a decoded
/// record: its key assembled into `key`, and its values read where the raw
/// tail lies when the record parses there, else from the record decoded on
/// its own into `scratch`. `Ok(false)` is damage -- a record that does not
/// parse either way -- for the caller to fail the page on.
fn coded_record_to<F: FnMut(&[u8], &[u8])>(
    raw: &Blob<MmapBytes>,
    g: &CodedGroup<'_>,
    i: usize,
    key: &mut Vec<u8>,
    scratch: &mut GroupBuf,
    f: &mut F,
) -> Result<bool> {
    let (payload, count, head0) = (g.payload, g.count, g.head0);
    let Ok(r) = coded_rec(payload, count, i) else {
        return Ok(false);
    };
    if !coded_key_into(head0, &r, key) {
        return Ok(false);
    }
    if r.tail_is_raw() {
        if let Some(h) = coded_header(head0, &r) {
            if let Some((exts, tail, _)) = parse_record_tail(h, r.tail) {
                let k: &[u8] = key.as_slice();
                raw.read_exts(&exts, tail, |v: &[u8]| f(k, v))?;
                return Ok(true);
            }
        }
    }
    let Ok(img) = decode_record(payload, count, i, scratch) else {
        return Ok(false);
    };
    let Some((k, exts, tail, _)) = parse_record(img, 0) else {
        return Ok(false);
    };
    raw.read_exts(&exts, tail, |v: &[u8]| f(k, v))?;
    Ok(true)
}

/// A position in a covered partition (3.10), for the merge that walks a
/// partition a key at a time beside the pieces over it
/// (`Reader::scan_merged`): the entry the rank falls in, its group held in
/// the cursor's own buffer across the calls that walk it, the record's
/// offset in the group and its key's span. `key` lends the key where it
/// lies in the group, and `advance` moves on to the next record of the
/// group or opens the next entry's. A run of records no page holds is
/// walked through the raw form for exactly those ranks, and a check that
/// fails under the cursor makes the rest of its walk the raw form's, with
/// the form file out of service, so that every key is still answered.
///
/// The cursor holds the thread's group buffer for its life; a read of a
/// covered partition made inside the walk makes a buffer of its own, as a
/// callback's nested read does.
pub(super) struct ZCursor<'a> {
    memo: &'a ZMemo,
    zfile: &'a ZFile,
    raw: &'a Blob<MmapBytes>,
    rank: usize,
    /// The rank the current group's records end at; the key count past the
    /// last entry or once the cursor is the raw form's.
    end: usize,
    /// The rank the current entry's records end at.
    page_end: usize,
    /// The current entry's index in the fence, its page, the group in the
    /// page and the rank of the group's first record.
    eidx: usize,
    page: usize,
    gidx: usize,
    gstart: usize,
    /// The record's offset in the group, its key's offset and length, and
    /// the next record's offset.
    off: usize,
    key_at: (usize, usize),
    next: usize,
    group: Group<'a>,
    buf: GroupBuf,
    /// The key at the position of a raw-tail coded group, assembled
    /// (`Group::Coded`).
    keybuf: Vec<u8>,
    /// Groups decoded, groups read in place without a decode, runs of
    /// records no page holds met and prefetch asks made, for the handle's
    /// counters.
    pub(super) decoded: u64,
    pub(super) inplace: u64,
    pub(super) nopage: u64,
    pub(super) prefetches: u64,
    /// Whether the bulk walk has asked for the pages ahead of it
    /// (`walk_until`): once a cursor, as `scan_at` asks once a walk.
    asked: bool,
    fell: bool,
    /// The key the last `skip_to` moved to, and whether the position is
    /// still its cut -- every record before it below it -- which a later
    /// key at or above it reads its answer from without a search
    /// (`skip_to`); a walk or a step from the position ends the claim.
    last: Vec<u8>,
    last_ok: bool,
}

impl<'a> ZCursor<'a> {
    fn new(memo: &'a ZMemo, zfile: &'a ZFile, raw: &'a Blob<MmapBytes>) -> ZCursor<'a> {
        ZCursor {
            memo,
            zfile,
            raw,
            rank: 0,
            end: 0,
            page_end: 0,
            eidx: 0,
            page: 0,
            gidx: 0,
            gstart: 0,
            off: 0,
            key_at: (0, 0),
            next: 0,
            group: Group::None,
            buf: take_buf(),
            keybuf: Vec::new(),
            decoded: 0,
            inplace: 0,
            nopage: 0,
            prefetches: 0,
            asked: false,
            fell: false,
            last: Vec::new(),
            last_ok: false,
        }
    }

    /// A cursor at `rank`, the key count or past it for the end.
    pub(super) fn at(
        memo: &'a ZMemo,
        zfile: &'a ZFile,
        raw: &'a Blob<MmapBytes>,
        rank: usize,
    ) -> ZCursor<'a> {
        let mut c = ZCursor::new(memo, zfile, raw);
        c.rank = rank;
        c.load();
        c
    }

    /// A cursor nowhere yet, placed by its first `skip_to`, which opens
    /// the group its key falls in: a batch of cuts over a block opens no
    /// group the batch does not need, where a cursor placed at the block's
    /// first rank opened that rank's group whether or not a key fell in it.
    pub(super) fn unplaced(
        memo: &'a ZMemo,
        zfile: &'a ZFile,
        raw: &'a Blob<MmapBytes>,
    ) -> ZCursor<'a> {
        ZCursor::new(memo, zfile, raw)
    }

    /// A cursor at the first key at or after `key`, by the fence and the one
    /// group it falls in (`ZFile::seek`), opened once for the seek and the
    /// walk after it; or why the form file cannot say, which is never
    /// `Served`, and the caller seeks the raw form.
    pub(super) fn seek(
        memo: &'a ZMemo,
        zfile: &'a ZFile,
        raw: &'a Blob<MmapBytes>,
        key: &[u8],
    ) -> std::result::Result<ZCursor<'a>, ZSeek> {
        if zfile.damaged.load(Ordering::Relaxed) {
            return Err(ZSeek::Out);
        }
        let mut c = ZCursor::new(memo, zfile, raw);
        let Some(e) = zfile.fence.entry_of(key) else {
            c.load();
            return Ok(c);
        };
        let Some(entry) = zfile.fence.entry(e, zfile.nkeys as u32) else {
            return Err(ZSeek::Fallback);
        };
        let first = entry.first_rank as usize;
        if entry.page == ZNOPAGE {
            if entry.count != 1 {
                return Err(ZSeek::NoPage);
            }
            c.rank = if entry.key == key { first } else { first + 1 };
            c.load();
            return Ok(c);
        }
        let Some(j) = zfile.group_of_key(entry.page as usize, key) else {
            return Err(ZSeek::Fallback);
        };
        if !c.open_at(&entry, j) {
            return Err(ZSeek::Fallback);
        }
        // A raw-tail coded group opened in place is sought over its heads
        // (`record_group_seek`), its position a record index; a decoded one
        // by the walk of its records, its position a byte offset. The first
        // version sought every group by the walk, and a group read in place
        // answers no bytes for it, so the seek failed the page and took the
        // form file out of service at the merge path's first scan over such
        // a partition, with every key still answered through the raw form.
        let sought = match &c.group {
            Group::Coded(payload, _) => {
                record_group_seek(payload, c.end - c.gstart, key).map(|pos| (pos.index, pos.index))
            }
            _ => group_seek(c.group(), c.end - c.gstart, key).map(|pos| (pos.index, pos.off)),
        };
        let (index, off) = match sought {
            Ok(at) => at,
            Err(_) => {
                zfile.fail(entry.page as usize);
                return Err(ZSeek::Fallback);
            }
        };
        c.rank = c.gstart + index;
        if c.rank >= c.end {
            c.next_group();
        } else {
            c.off = off;
            c.place();
        }
        Ok(c)
    }

    /// The rank at the position: the key count at the end.
    pub(super) fn rank(&self) -> usize {
        self.rank
    }

    /// The rest of the walk is the raw form's, after a check failed.
    pub(super) fn fell_back(&self) -> bool {
        self.fell
    }

    /// The group as the last open left it.
    fn group(&self) -> &[u8] {
        match &self.group {
            Group::Page(p) => p,
            Group::Buf(n) => self.buf.bytes(*n),
            Group::Memo(g, n) => g.bytes(*n),
            Group::None | Group::Raw | Group::Coded(..) => &[],
        }
    }

    /// The walk from here to the partition's end is the raw form's.
    fn fall(&mut self) {
        self.fell = true;
        self.group = Group::Raw;
        self.end = self.zfile.nkeys;
        self.page_end = self.zfile.nkeys;
        self.off = 0;
    }

    /// Group `j` of entry `entry`'s page opened as the cursor's group, at
    /// its first record; an entry no page holds is the raw form's run.
    /// `false` where it could not be, the cursor fallen.
    fn open_at(&mut self, entry: &FenceEntry<'_>, j: usize) -> bool {
        let first = entry.first_rank as usize;
        self.eidx = entry.index;
        self.page_end = first
            .saturating_add(entry.count as usize)
            .min(self.zfile.nkeys);
        self.off = 0;
        if entry.page == ZNOPAGE {
            self.nopage += 1;
            self.group = Group::Raw;
            self.gidx = 0;
            self.gstart = first;
            self.end = self.page_end;
            return true;
        }
        self.page = entry.page as usize;
        self.gidx = j;
        // Where the group lies once opened: a stored group that is aligned
        // is parsed where it lies in the mapping, anything else is decoded
        // into the buffer, whose borrow by the open ends with the open.
        let view = self
            .zfile
            .page_bytes(self.page)
            .and_then(|page| self.zfile.view(self.page, page).ok());
        let Some((opened, gfirst, count, decoded)) = self.zfile.open_group(
            self.memo,
            entry,
            j,
            &mut self.buf,
            view.as_ref(),
            self.zfile.inplace,
        ) else {
            self.fall();
            return false;
        };
        self.gstart = first.saturating_add(gfirst).min(self.zfile.nkeys);
        self.end = self.gstart.saturating_add(count).min(self.page_end);
        if decoded {
            self.decoded += 1;
        }
        // The memo's group is held by its `Rc`; a borrowed one is the
        // buffer's or the page's, and its borrow ends here.
        // A coded group read in place is the page's payload, taken from
        // the view, whose borrow is the mapping's and not the buffer's.
        let placed = match opened {
            Opened::Memo(g, n) => Some(Group::Memo(g, n)),
            Opened::Coded(_) => view.as_ref().and_then(|v| v.payload(j)).map(|payload| {
                self.inplace += 1;
                Group::Coded(payload, count)
            }),
            Opened::Borrowed(_) => match view {
                Some(v) => match (v.group(j), v.payload(j)) {
                    (Some(g), Some(payload)) if g.stored => Some(Group::Page(payload)),
                    (Some(g), _) => Some(Group::Buf(g.ulen as usize)),
                    _ => None,
                },
                None => None,
            },
        };
        let Some(group) = placed else {
            self.zfile.fail(self.page);
            self.fall();
            return false;
        };
        self.group = group;
        true
    }

    /// The position at the end of a group: the next group of the same
    /// page where there is one, opened at its first record, else the
    /// entry the rank falls in.
    fn next_group(&mut self) {
        if self.rank < self.page_end && !matches!(self.group, Group::Raw | Group::None) {
            if let Some(entry) = self.zfile.fence.entry(self.eidx, self.zfile.nkeys as u32) {
                if self.open_at(&entry, self.gidx + 1) && !matches!(self.group, Group::Raw) {
                    self.place();
                }
                return;
            }
        }
        self.load();
    }

    /// The record at `off` parsed for its key and the record after it: a
    /// record that does not parse in a group the checks let through is
    /// damage, and the walk falls.
    fn place(&mut self) {
        if let Group::Coded(payload, count) = &self.group {
            let (payload, count) = (*payload, *count);
            let ok = match (
                coded_first(payload, count),
                coded_rec(payload, count, self.off),
            ) {
                (Ok((head0, _)), Ok(r)) => coded_key_into(head0, &r, &mut self.keybuf),
                _ => false,
            };
            if !ok {
                self.zfile.fail(self.page);
                self.fall();
                return;
            }
            self.next = self.off + 1;
            return;
        }
        let Some((k, next)) = record_key_next(self.group(), self.off) else {
            self.zfile.fail(self.page);
            self.fall();
            return;
        };
        let base = self.group().as_ptr() as usize;
        self.key_at = (k.as_ptr() as usize - base, k.len());
        self.next = next;
    }

    /// The position loaded for `self.rank`: the entry it falls in opened
    /// and the walk to its record.
    fn load(&mut self) {
        self.off = 0;
        if self.rank >= self.zfile.nkeys {
            self.group = Group::None;
            self.end = self.zfile.nkeys;
            return;
        }
        if self.fell || self.zfile.damaged.load(Ordering::Relaxed) {
            self.fall();
            return;
        }
        let Some(entry) = self.zfile.entry_at_rank(self.rank) else {
            self.fall();
            return;
        };
        let j = if entry.page == ZNOPAGE {
            0
        } else {
            let i = self.rank - entry.first_rank as usize;
            match self.zfile.group_of_index(entry.page as usize, i) {
                Some(j) => j,
                None => {
                    self.fall();
                    return;
                }
            }
        };
        if !self.open_at(&entry, j) || matches!(self.group, Group::Raw) {
            return;
        }
        if matches!(self.group, Group::Coded(..)) {
            // The position in a group read in place is the record's index.
            self.off = self.rank - self.gstart;
            self.place();
            return;
        }
        for _ in self.gstart..self.rank {
            let Some((_, next)) = record_key_next(self.group(), self.off) else {
                self.zfile.fail(self.page);
                self.fall();
                return;
            };
            self.off = next;
        }
        self.place();
    }

    /// The key at the position, or `None` past the end.
    pub(super) fn key(&self) -> Option<&[u8]> {
        debug_assert!(
            self.rank < self.end || matches!(self.group, Group::None | Group::Raw),
            "a spent group is resumed before it is read"
        );
        match self.group {
            Group::None => None,
            Group::Raw => self.raw.key_at(self.rank),
            Group::Coded(..) => Some(self.keybuf.as_slice()),
            Group::Page(_) | Group::Buf(_) | Group::Memo(..) => {
                let (at, len) = self.key_at;
                self.group().get(at..at + len)
            }
        }
    }

    /// The values at the position, in the raw form's order, and how many.
    pub(super) fn values<F: FnMut(&[u8])>(&self, f: &mut F) -> Result<u64> {
        debug_assert!(
            self.rank < self.end || matches!(self.group, Group::None | Group::Raw),
            "a spent group is resumed before it is read"
        );
        match self.group {
            Group::None => Ok(0),
            Group::Raw => self.raw.values_at(self.rank, &mut *f),
            Group::Coded(payload, count) => {
                // The record where it lies, or decoded on its own into a
                // buffer of this call's, since the position's own buffer
                // is not for a shared borrow to write; a record that parses
                // neither way is the group changed under the mapping.
                let mut n = 0u64;
                let mut key = Vec::new();
                let mut scratch = GroupBuf::new();
                let read = match CodedGroup::open(payload, count) {
                    Ok(g) => coded_record_to(
                        self.raw,
                        &g,
                        self.off,
                        &mut key,
                        &mut scratch,
                        &mut |_k: &[u8], v: &[u8]| {
                            n += 1;
                            f(v);
                        },
                    )?,
                    Err(_) => false,
                };
                if read {
                    Ok(n)
                } else {
                    self.zfile.fail(self.page);
                    self.raw.values_at(self.rank, &mut *f)
                }
            }
            Group::Page(_) | Group::Buf(_) | Group::Memo(..) => {
                match parse_record(self.group(), self.off) {
                    Some((_, exts, tail, _)) => self.raw.read_exts(&exts, tail, &mut *f),
                    None => {
                        // The first decode walked every record of the group
                        // whole, so this is a group changed under the mapping:
                        // the page out of service and the raw form's values.
                        self.zfile.fail(self.page);
                        self.raw.values_at(self.rank, &mut *f)
                    }
                }
            }
        }
    }

    /// One record on: the next of the group, or the next group's first,
    /// or the next entry's.
    pub(super) fn advance(&mut self) {
        self.last_ok = false;
        self.resume();
        if matches!(self.group, Group::None) {
            return;
        }
        self.rank += 1;
        if self.rank >= self.end {
            self.next_group();
        } else if !matches!(self.group, Group::Raw) {
            self.off = self.next;
            self.place();
        }
    }

    /// The bulk walk from the position, for the overlay walk
    /// (`Reader::scan_partitions` under `Options::zpages_walk_cursor`):
    /// every record from here handed to `f` as `ZFile::scan_at` hands
    /// them, until `limit` records have been, or the key under the cursor
    /// is not below `until`, or the partition ends; the cursor stands at
    /// the record it stopped at. Answers how many it handed over and how
    /// the key it stands at compares with `until`: `Equal` for `until`
    /// itself, `Greater` for a key past it, and `Greater` at the end or at
    /// the limit, where no key was compared. The group is held across the
    /// records and across the caller's turns, so a walk cut at every
    /// unsealed key opens one group a group crossed (the next group of the
    /// page, then the next page's), where the walk cut by rank opened one
    /// for its seek, one for its fresh walk and one for its emit at each. A run of records no page holds, and the rest of a walk
    /// after a check failed, go a key at a time through the raw form, as
    /// the merge path reads them.
    pub(super) fn walk_until<F: FnMut(&[u8], &[u8])>(
        &mut self,
        until: Option<&[u8]>,
        limit: usize,
        f: &mut F,
    ) -> Result<(usize, KeyOrder)> {
        self.walk_impl(until, limit, f, false)
    }

    /// `walk_until` for a block's reader (`PartReader::scan`, `scan_until`),
    /// which reads its block in consecutive runs and asks nothing of the
    /// position between them but another run, a step or the record under
    /// a cut: where the limit falls at a group's last record the next group
    /// is left to the next read from the position, which resumes it
    /// (`resume`), since the run may be the block's last and the group would
    /// be opened for nobody -- the walk a run at a time that the reader
    /// replaced opened nothing past its last record, and the first pairing
    /// of the reader decoded 0.7% more groups in E for these opens. `key`
    /// and `values` are not asked of the position before the next read
    /// resumes it (they assert so under the checked profile); `rank` is
    /// right either way. The overlay walk keeps `walk_until`, whose caller
    /// reads the key under the cursor after a walk.
    pub(super) fn walk_run<F: FnMut(&[u8], &[u8])>(
        &mut self,
        until: Option<&[u8]>,
        limit: usize,
        f: &mut F,
    ) -> Result<(usize, KeyOrder)> {
        self.walk_impl(until, limit, f, true)
    }

    fn walk_impl<F: FnMut(&[u8], &[u8])>(
        &mut self,
        until: Option<&[u8]>,
        limit: usize,
        f: &mut F,
        lazy_end: bool,
    ) -> Result<(usize, KeyOrder)> {
        self.last_ok = false;
        self.resume();
        let mut n = 0usize;
        while n < limit {
            if matches!(self.group, Group::None) {
                return Ok((n, KeyOrder::Greater));
            }
            if matches!(self.group, Group::Raw) {
                let Some(k) = self.raw.key_at(self.rank) else {
                    return Ok((n, KeyOrder::Greater));
                };
                if let Some(u) = until {
                    match k.cmp(u) {
                        KeyOrder::Less => {}
                        order => return Ok((n, order)),
                    }
                }
                self.raw.values_at(self.rank, &mut |v: &[u8]| f(k, v))?;
                n += 1;
                self.advance();
                continue;
            }
            if !self.asked {
                self.asked = true;
                let past = (limit - n).saturating_sub(self.page_end - self.rank);
                if self.zfile.ask_ahead(self.page, past) {
                    self.prefetches += 1;
                }
            }
            if let Group::Coded(payload, count) = &self.group {
                // A raw-tail coded group: each record's key assembled and
                // its values read where its tail lies, nothing decoded.
                let (payload, count) = (*payload, *count);
                let raw = self.raw;
                let (mut i, mut rank, end) = (self.off, self.rank, self.end);
                let mut stop: Option<KeyOrder> = None;
                let mut damaged = false;
                match CodedGroup::open(payload, count) {
                    Ok(g) => {
                        let head0 = g.head0;
                        while n < limit && rank < end {
                            let Ok(r) = coded_rec(payload, count, i) else {
                                damaged = true;
                                break;
                            };
                            if r.plen > head0.len() {
                                damaged = true;
                                break;
                            }
                            if let Some(u) = until {
                                match cmp_coded_key(head0, &r, u) {
                                    KeyOrder::Less => {}
                                    order => {
                                        stop = Some(order);
                                        break;
                                    }
                                }
                            }
                            if !coded_record_to(raw, &g, i, &mut self.keybuf, &mut self.buf, f)? {
                                damaged = true;
                                break;
                            }
                            i += 1;
                            rank += 1;
                            n += 1;
                        }
                    }
                    Err(_) => damaged = true,
                }
                self.rank = rank;
                if damaged {
                    self.zfile.fail(self.page);
                    self.fall();
                    continue;
                }
                if rank >= end {
                    if lazy_end && n >= limit {
                        self.off = i;
                        return Ok((n, KeyOrder::Greater));
                    }
                    self.next_group();
                    continue;
                }
                self.off = i;
                self.place();
                if let Some(order) = stop {
                    return Ok((n, order));
                }
                continue;
            }
            // The held group: its records parsed where they lie, until it
            // is spent, the limit is reached or `until` is met.
            let (mut off, mut rank, end) = (self.off, self.rank, self.end);
            let mut stop: Option<KeyOrder> = None;
            let mut damaged = false;
            {
                let group = self.group();
                while n < limit && rank < end {
                    let Some((k, exts, tail, len)) = parse_record(group, off) else {
                        damaged = true;
                        break;
                    };
                    if let Some(u) = until {
                        match k.cmp(u) {
                            KeyOrder::Less => {}
                            order => {
                                stop = Some(order);
                                break;
                            }
                        }
                    }
                    self.raw.read_exts(&exts, tail, &mut |v: &[u8]| f(k, v))?;
                    off += len;
                    rank += 1;
                    n += 1;
                }
            }
            self.rank = rank;
            if damaged {
                // A record that does not parse in a group the checks let
                // through is damage: the page out of service, and the rest
                // of the walk the raw form's, every key still answered.
                self.zfile.fail(self.page);
                self.fall();
                continue;
            }
            if rank >= end {
                if lazy_end && n >= limit {
                    // `walk_run`'s end: the run ended at the group's last
                    // record, and the next group is the next read's to
                    // open (`resume`).
                    self.off = off;
                    return Ok((n, KeyOrder::Greater));
                }
                self.next_group();
                continue;
            }
            self.off = off;
            self.place();
            if let Some(order) = stop {
                return Ok((n, order));
            }
        }
        Ok((n, KeyOrder::Greater))
    }

    /// A group a walk spent to its last record (`walk_until` at its limit):
    /// the next group opened now, at its first record, where the walk left
    /// it to whoever reads on. Every read from the position goes through
    /// it first; `key` and `values` expect it done.
    pub(super) fn resume(&mut self) {
        if self.rank >= self.end
            && matches!(
                self.group,
                Group::Page(_) | Group::Buf(_) | Group::Memo(..) | Group::Coded(..)
            )
        {
            self.next_group();
        }
    }

    /// The position moved to the first record whose key is not below
    /// `until`, or to the end, and how that key compares with `until`:
    /// `Equal` for `until` itself, `Greater` for a key past it and at the
    /// end. For a batch of keys in key order it is the seek a key
    /// (`ZFile::cut_in`) with the group kept: where the position is the cut
    /// of the key before and its record is not below `until`, the cut is
    /// the position and nothing is searched; else the entry and the group
    /// `until` falls in are found as a seek finds them, the fence searched
    /// from the entry held (`ZFence::entry_of_from`) and the page's
    /// directory, and the group held is kept where it is that group and
    /// the position is not past `until`, so the records are parsed from
    /// the position on and each once -- the cuts of a block's overlay keys
    /// (`PartReader::cut`), where a seek a key opened the group and parsed
    /// it from its first record for every key. A key in another group, or
    /// below the position, opens its group as the seek would, at its first
    /// record. The keys are read and nothing else. `None` where the form
    /// file cannot say -- out of service, an entry or a page that fails a
    /// check, a record that does not parse -- after which the cursor has
    /// fallen (`fell_back`) and the caller takes the cut from the raw form.
    pub(super) fn skip_to(&mut self, until: &[u8]) -> Option<KeyOrder> {
        if self.fell || self.zfile.damaged.load(Ordering::Relaxed) {
            self.fall();
            return None;
        }
        self.resume();
        // The position is the cut already: every record before it is below
        // the key before, which is at or below this one; at the end, every
        // record is.
        if self.last_ok && self.last.as_slice() <= until {
            match self.key().map(|k| k.cmp(until)) {
                None => return Some(KeyOrder::Greater),
                Some(KeyOrder::Less) => {}
                Some(order) => {
                    self.remember(until);
                    return Some(order);
                }
            }
        }
        // Where `until` lies: the entry from the one held, and the group in
        // its page; a key below every entry's is the first record's.
        let e = self
            .zfile
            .fence
            .entry_of_from(until, self.eidx)
            .unwrap_or(0);
        let Some(entry) = self.zfile.fence.entry(e, self.zfile.nkeys as u32) else {
            self.fall();
            return None;
        };
        let j = if entry.page == ZNOPAGE {
            0
        } else {
            match self.zfile.group_of_key(entry.page as usize, until) {
                Some(j) => j,
                None => {
                    self.fall();
                    return None;
                }
            }
        };
        let held = e == self.eidx
            && j == self.gidx
            && !matches!(self.group, Group::None)
            && self
                .key()
                .is_some_and(|k| k.cmp(until) != KeyOrder::Greater);
        if !held {
            if !self.open_at(&entry, j) {
                return None;
            }
            self.rank = self.gstart;
            if !matches!(self.group, Group::Raw) {
                self.place();
            }
        }
        let order = self.advance_until(until);
        if self.fell {
            return None;
        }
        self.remember(until);
        Some(order)
    }

    /// The key `skip_to` moved to, kept for the next.
    fn remember(&mut self, until: &[u8]) {
        self.last.clear();
        self.last.extend_from_slice(until);
        self.last_ok = true;
    }

    /// The records from the position parsed for their keys until one is not
    /// below `until`, or the end: the held group first, then the page's
    /// next and the next entry's, each opened once. How the key it stands
    /// at compares with `until`, `Greater` at the end; a record that does
    /// not parse is damage, the page out of service and the cursor fallen,
    /// which the caller reads before the answer.
    fn advance_until(&mut self, until: &[u8]) -> KeyOrder {
        loop {
            match self.group {
                Group::None => return KeyOrder::Greater,
                Group::Raw => {
                    if self.fell {
                        return KeyOrder::Greater;
                    }
                    let Some(k) = self.raw.key_at(self.rank) else {
                        return KeyOrder::Greater;
                    };
                    match k.cmp(until) {
                        KeyOrder::Less => self.advance(),
                        order => return order,
                    }
                }
                Group::Coded(payload, count) => {
                    // The heads compared where they lie, no key assembled
                    // until the one the position stops at.
                    let (payload, count) = (payload, count);
                    let (mut i, mut rank, end) = (self.off, self.rank, self.end);
                    let mut stop: Option<KeyOrder> = None;
                    let mut damaged = false;
                    match coded_first(payload, count) {
                        Ok((head0, _)) => {
                            while rank < end {
                                let Ok(r) = coded_rec(payload, count, i) else {
                                    damaged = true;
                                    break;
                                };
                                if r.plen > head0.len() {
                                    damaged = true;
                                    break;
                                }
                                match cmp_coded_key(head0, &r, until) {
                                    KeyOrder::Less => {
                                        i += 1;
                                        rank += 1;
                                    }
                                    order => {
                                        stop = Some(order);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(_) => damaged = true,
                    }
                    self.rank = rank;
                    if damaged {
                        self.zfile.fail(self.page);
                        self.fall();
                        return KeyOrder::Greater;
                    }
                    if rank >= end {
                        self.next_group();
                        continue;
                    }
                    self.off = i;
                    self.place();
                    if self.fell {
                        return KeyOrder::Greater;
                    }
                    return stop.unwrap_or(KeyOrder::Greater);
                }
                _ => {
                    let (mut off, mut rank, end) = (self.off, self.rank, self.end);
                    let mut stop: Option<KeyOrder> = None;
                    let mut damaged = false;
                    {
                        let group = self.group();
                        while rank < end {
                            let Some((k, next)) = record_key_next(group, off) else {
                                damaged = true;
                                break;
                            };
                            match k.cmp(until) {
                                KeyOrder::Less => {
                                    off = next;
                                    rank += 1;
                                }
                                order => {
                                    stop = Some(order);
                                    break;
                                }
                            }
                        }
                    }
                    self.rank = rank;
                    if damaged {
                        self.zfile.fail(self.page);
                        self.fall();
                        return KeyOrder::Greater;
                    }
                    if rank >= end {
                        self.next_group();
                        continue;
                    }
                    self.off = off;
                    self.place();
                    if self.fell {
                        return KeyOrder::Greater;
                    }
                    return stop.unwrap_or(KeyOrder::Greater);
                }
            }
        }
    }
}

impl Drop for ZCursor<'_> {
    fn drop(&mut self) {
        put_buf(std::mem::take(&mut self.buf));
    }
}

/// A group opened (`ZFile::open_group`): where its bytes are.
enum Opened<'a> {
    /// A stored page parsed where it lies in the mapping, or a group
    /// decoded into the thread's buffer with the memo off.
    Borrowed(&'a [u8]),
    /// From the memo, or decoded into a buffer the memo now holds: `n`
    /// bytes of it.
    Memo(Arc<GroupBuf>, usize),
    /// A raw-tail coded group's payload, undecoded, for a caller that
    /// asked to read it record by record where it lies (`open_group` with
    /// `inplace`): never handed to a caller that reads the bytes as a
    /// decoded group.
    Coded(&'a [u8]),
}

impl Opened<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Opened::Borrowed(b) | Opened::Coded(b) => b,
            Opened::Memo(g, n) => g.bytes(*n),
        }
    }
}

/// Where a key lies in a form file, found or not, with the bytes it lies in.
enum Locate<'a> {
    /// The key is not in this partition: sorts below its first key, or
    /// among the records of the page it would be in, which was decoded to
    /// find that out unless it is stored or there was no page to open.
    Absent { decoded: bool },
    /// The record: the group it lies in as it was opened, its offset in
    /// the group, the page, and whether a page was decoded. The caller
    /// parses the record out of the group, which the `Opened` keeps alive
    /// -- an `Rc` into the memo where the group came from there.
    Found {
        opened: Opened<'a>,
        off: usize,
        page: usize,
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

/// A group opened: its bytes, the index in the page of its first record,
/// its record count, and whether a decode was made.
type Open<'a> = (&'a [u8], usize, usize, bool);

/// A page's group `j` at the page's first decode in this process: every
/// check the layout has, in the order that trusts the least first (the
/// CRC, the tag, the header, the directory, the zeros; the rank and the
/// count against the fence entry that names the page; every group's
/// payload decoded to exactly its `ulen` and walked, its whole records in
/// ascending order, the first the key named for it and every one below the
/// next group's), and group `j`. Every group of the page is decoded and
/// walked, not `j` alone: the directory's key for a group is held to that
/// group's first record only by its walk, and a key the directory names
/// above the group's true first record would send a read of the record to
/// the group before, where it is absent, with nothing raised -- so a page
/// is verified whole, once, and every read of it after decodes the one
/// group it wants. A stored group is parsed in place and only where its
/// payload is aligned; the other groups' walks decode into a buffer of the
/// thread's own for the check.
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
    j: usize,
    buf: &'a mut GroupBuf,
    own: bool,
) -> std::result::Result<Open<'a>, Fail> {
    let view = first_checks(page, bind, entry, Some(j), own)?;
    let g = view
        .group(j)
        .ok_or(Fail::Check(ZError::Header("no such group in the page")))?;
    let payload = view.payload(j).ok_or(Fail::Check(ZError::Length))?;
    if g.stored && !aligned_for_extents(payload) {
        return Err(Fail::Misaligned);
    }
    let group = decode_group_with(&view, j, buf, own)?;
    check_group(group, &view, j, entry)?;
    Ok((group, usize::from(g.first), usize::from(g.count), !g.stored))
}

/// A page's first checks, every one that needs no group of it decoded for
/// the caller: the CRC, the tag, the header and the directory, the fence
/// entry, and every group's own check -- a record-coded group by its heads
/// (`check_coded_group`), a block or a stored group decoded into the
/// thread's verify buffer and walked (`check_group`) -- but for `except`,
/// which the caller decodes and checks itself. The view, for the caller to
/// go on with.
pub(super) fn first_checks<'a>(
    page: &'a [u8],
    bind: u32,
    entry: &FenceEntry<'_>,
    except: Option<usize>,
    own: bool,
) -> std::result::Result<PageView<'a>, Fail> {
    let view = parse_page(page, bind)?;
    check_entry(&view, entry)?;
    let mut scratch = take_verify_buf();
    let groups = (0..view.groups())
        .filter(|&k| Some(k) != except)
        .try_for_each(|k| {
            let g = view
                .group(k)
                .ok_or(ZError::Header("no such group in the page"))?;
            if g.coded {
                let payload = view.payload(k).ok_or(ZError::Length)?;
                check_coded_group(payload, &view, k, entry)
            } else {
                let other = decode_group_with(&view, k, &mut scratch, own)?;
                check_group(other, &view, k, entry)
            }
        });
    put_verify_buf(scratch);
    groups?;
    Ok(view)
}

/// A verified group of a page: the header's and the directory's values and
/// the decode, and nothing the first decode already settled.
#[cfg(test)]
pub(super) fn verified_group<'a>(
    page: &'a [u8],
    j: usize,
    buf: &'a mut GroupBuf,
) -> std::result::Result<Open<'a>, Fail> {
    let view = page_view(page)?;
    verified_group_in(&view, j, buf, false)
}

/// `verified_group` over a view the caller took of the page: the group the
/// directory names decoded into `buf`, or parsed where it lies.
pub(super) fn verified_group_in<'a>(
    view: &PageView<'a>,
    j: usize,
    buf: &'a mut GroupBuf,
    own: bool,
) -> std::result::Result<Open<'a>, Fail> {
    let g = view
        .group(j)
        .ok_or(Fail::Check(ZError::Header("no such group in the page")))?;
    let payload = view.payload(j).ok_or(Fail::Check(ZError::Length))?;
    if g.stored && !aligned_for_extents(payload) {
        return Err(Fail::Misaligned);
    }
    let group = decode_group_with(view, j, buf, own)?;
    Ok((group, usize::from(g.first), usize::from(g.count), !g.stored))
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

thread_local! {
    /// The buffer a page's first decode walks its other groups through:
    /// the thread's, beside the one the read decodes its own group into.
    static ZVERIFY: Cell<Option<GroupBuf>> = const { Cell::new(None) };
}

fn take_verify_buf() -> GroupBuf {
    ZVERIFY.with(|c| c.take()).unwrap_or_default()
}

fn put_verify_buf(b: GroupBuf) {
    ZVERIFY.with(|c| c.set(Some(b)));
}

/// The pages a scan asks for ahead of its walk at most
/// (`Options::zpages_scan_prefetch`).
const ZPREFETCH_PAGES: usize = 16;

/// How a form file's compressed pages meet the handle's memo: not at all
/// (`Options::zpages_memo` off), at the slot the group's number gives
/// (`Options::zpages_memo_hash` off, the shape before the hash), or at the
/// hashed slot. A bool is the memo on hashed or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MemoShape {
    Off,
    Linear,
    Hashed,
}

impl From<bool> for MemoShape {
    fn from(on: bool) -> MemoShape {
        if on {
            MemoShape::Hashed
        } else {
            MemoShape::Off
        }
    }
}

/// The memo's slots, its cap on the decoded bytes it holds, and the
/// displaced buffers it keeps for the next decodes, a handle. The slots
/// went from a thousand and twenty-four to four thousand and ninety-six
/// when the entry became a group and not a page, a quarter the bytes, so
/// that the cap and not the slot count bounds what the memo holds.
pub(super) const ZMEMO_SLOTS: usize = 4096;
pub(super) const ZMEMO_BYTES: usize = 8 << 20;
const ZMEMO_FREE: usize = 4;

/// The memo of decoded groups (3.7): a reader handle's, direct-mapped over
/// its slots and capped at `cap` bytes of the groups it holds, keyed by the
/// form file's attach identity, the page and the group in it. A hit hands
/// back the group as it was decoded and pays the in-group walk alone; an
/// entry is an `Arc` to its buffer, so a read holds its group across a
/// callback's nested read, which may displace the entry from the memo
/// without taking the group away. A form file retired takes a new identity
/// at its next attach, so its entries never hit again and are displaced as
/// the slots turn over. The slot is the group's number in the file (the
/// page times the most groups a page holds, plus the group) and the form
/// file's identity, mixed by a Fibonacci hash and taken mod the slots
/// (`Memo::slot`): the number alone left twelve of every sixteen slots to
/// group indices a page of the shipped cut never reaches, so a form file's
/// groups took a quarter of the slots and pages 256 apart collided, and
/// the hash spreads consecutive groups over every slot with few collisions
/// until the slots are nearly full. The slots are made at the first put,
/// so a handle that reads no covered partition pays nothing for them.
///
/// A handle's and not a thread's: a thread's memo kept the groups of every
/// store the thread had closed, ninety kilobytes a store in the leak test,
/// where a handle's goes with the handle; and a handle is what the memory
/// census counts by.
struct Memo {
    slots: Vec<Option<MemoEntry>>,
    n: usize,
    cap: usize,
    bytes: usize,
    /// Where the next displacement for the cap begins.
    sweep: usize,
    /// The buffers of displaced entries no read still holds, for the next
    /// decodes: a decode into a buffer made for it was an allocation of
    /// whole pages a miss, and a uniform read set, which misses every
    /// time, read a tenth slower with the memo than without it for that
    /// alone (`scratch/wave2/memo-ab-v1-*.out`).
    free: Vec<GroupBuf>,
    hits: u64,
    misses: u64,
    /// E2's footprint (design 4.7): while the handle reads the compact form
    /// under the controller, one bit a page it touched, by form file, and
    /// the distinct pages so far; off and empty otherwise.
    track: bool,
    touched: std::collections::HashMap<u64, Vec<u64>>,
    distinct: u64,
}

struct MemoEntry {
    zfile: u64,
    page: u32,
    index: u8,
    group: Arc<GroupBuf>,
    len: usize,
    /// The index in the page of the group's first record, and its record
    /// count, as the directory names them: what a hit answers with so that
    /// it reads nothing of the page (`ZFile::open_group`).
    first: u16,
    count: u16,
}

impl Memo {
    fn new(slots: usize, cap: usize) -> Memo {
        Memo {
            slots: Vec::new(),
            n: slots.max(1),
            cap,
            bytes: 0,
            sweep: 0,
            free: Vec::new(),
            hits: 0,
            misses: 0,
            track: false,
            touched: std::collections::HashMap::new(),
            distinct: 0,
        }
    }

    /// A page touched, for the footprint; nothing while it is not kept.
    fn touch(&mut self, zfile: u64, page: u32, n_pages: usize) {
        if !self.track {
            return;
        }
        let words = n_pages.div_ceil(64).max(1);
        let map = self
            .touched
            .entry(zfile)
            .or_insert_with(|| vec![0u64; words]);
        if map.len() < words {
            map.resize(words, 0);
        }
        let (w, b) = ((page >> 6) as usize, page & 63);
        if let Some(word) = map.get_mut(w) {
            if *word & (1u64 << b) == 0 {
                *word |= 1u64 << b;
                self.distinct += 1;
            }
        }
    }

    /// A displaced entry's buffer kept for a later decode, where no read
    /// still holds its group.
    fn recycle(&mut self, old: MemoEntry) {
        self.bytes -= old.len;
        if self.free.len() < ZMEMO_FREE {
            if let Ok(buf) = Arc::try_unwrap(old.group) {
                self.free.push(buf);
            }
        }
    }

    /// A buffer for a decode: a displaced entry's, or a new one.
    fn take_free(&mut self) -> GroupBuf {
        self.free.pop().unwrap_or_default()
    }

    /// The entry a put for `(zfile, page, index)` would displace, the line
    /// its `Arc`'s counts and buffer header sit in asked for ahead of the
    /// decode whose put recycles it (`recycle`); nothing where the slot is
    /// empty or the slots are not made. The counts sit just before the
    /// group in the `Arc`'s allocation.
    fn prefetch_victim(&self, zfile: u64, page: u32, index: u8, hashed: bool) {
        if self.slots.is_empty() {
            return;
        }
        let i = self.slot(zfile, page, index, hashed);
        if let Some(e) = &self.slots[i] {
            let p = Arc::as_ptr(&e.group) as *const u8;
            // SAFETY-free: a prefetch is a hint that faults on no address,
            // and the address is the allocation's start, sixteen bytes
            // before the data an `Arc` points at.
            super::prefetch_lines(p.wrapping_sub(16), 80);
        }
    }

    fn slot(&self, zfile: u64, page: u32, index: u8, hashed: bool) -> usize {
        if !hashed {
            // The shape before the hash (`Options::zpages_memo_hash` off):
            // the group's number plus a multiple of the identity.
            return (page as usize)
                .wrapping_mul(ZMAX_GROUPS)
                .wrapping_add(usize::from(index))
                .wrapping_add((zfile as usize).wrapping_mul(0x9E37_79B9))
                % self.n;
        }
        let group = u64::from(page)
            .wrapping_mul(ZMAX_GROUPS as u64)
            .wrapping_add(u64::from(index));
        let h = group
            .wrapping_add(zfile.wrapping_mul(0xD6E8_FEB8_6659_FD93))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        ((h >> 32) as usize) % self.n
    }

    /// The group memoised for `(side, page, index)`, counted a hit: its
    /// buffer, the bytes of it, and the group's first record index and
    /// count.
    fn get(
        &mut self,
        zfile: u64,
        page: u32,
        index: u8,
        hashed: bool,
    ) -> Option<(Arc<GroupBuf>, usize, u16, u16)> {
        if self.slots.is_empty() {
            return None;
        }
        let i = self.slot(zfile, page, index, hashed);
        match &self.slots[i] {
            Some(e) if e.zfile == zfile && e.page == page && e.index == index => {
                self.hits += 1;
                Some((Arc::clone(&e.group), e.len, e.first, e.count))
            }
            _ => None,
        }
    }

    /// `group` memoised for `(side, page, index)`, which is a miss -- every
    /// decode of a compressed group with the memo on ends here, a lookup
    /// before it or not: the slot's occupant displaced, and for the cap
    /// further entries from a rotating sweep until the bytes held fit, the
    /// entry just put kept, so the bytes never exceed the cap unless this
    /// one group does.
    #[allow(clippy::too_many_arguments)]
    fn put(
        &mut self,
        zfile: u64,
        page: u32,
        index: u8,
        hashed: bool,
        group: Arc<GroupBuf>,
        len: usize,
        first: u16,
        count: u16,
    ) {
        self.misses += 1;
        if self.slots.is_empty() {
            self.slots = (0..self.n).map(|_| None).collect();
        }
        let i = self.slot(zfile, page, index, hashed);
        if let Some(old) = self.slots[i].take() {
            self.recycle(old);
        }
        self.slots[i] = Some(MemoEntry {
            zfile,
            page,
            index,
            group,
            len,
            first,
            count,
        });
        self.bytes += len;
        let n = self.slots.len();
        let mut turns = 0;
        while self.bytes > self.cap && turns < n {
            let j = self.sweep;
            self.sweep = (self.sweep + 1) % n;
            turns += 1;
            if j == i {
                continue;
            }
            if let Some(old) = self.slots[j].take() {
                self.recycle(old);
            }
        }
    }
}

/// A handle's memo (`Memo`) behind a `RefCell`: borrowed for a lookup or a
/// put and never across a decode or a callback, so a callback's nested
/// read through the same handle finds it free.
pub(super) struct ZMemo {
    memo: RefCell<Memo>,
    /// Whether the footprint is kept, outside the memo's cell: every page
    /// open asks, and under `Always` and in the census phases that do not
    /// serve the form the answer is no, at a load and a branch rather
    /// than the cell's borrow -- eighteen instructions a point read
    /// through the form with the borrow, counted, against the three the
    /// flag costs.
    track: Cell<bool>,
}

impl ZMemo {
    /// A memo at the shipped cap (`Options::zpages_memo_bytes` as it
    /// defaults): the tests' handle; a store's handles take `sized`.
    #[cfg(test)]
    pub(super) fn new() -> ZMemo {
        ZMemo::sized(ZMEMO_BYTES)
    }

    /// A memo capped at `bytes` of decoded groups (`Options::zpages_memo_bytes`),
    /// its slots scaled with the cap at the shipped ratio -- a slot per two
    /// kibibytes, a group's decoded size at the suite's shape -- so the cap
    /// and not the slot count bounds what it holds, as `ZMemo::new` has it.
    pub(super) fn sized(bytes: usize) -> ZMemo {
        let slots = (bytes / (ZMEMO_BYTES / ZMEMO_SLOTS)).max(1);
        ZMemo::of(Memo::new(slots, bytes))
    }

    fn of(memo: Memo) -> ZMemo {
        ZMemo {
            memo: RefCell::new(memo),
            track: Cell::new(false),
        }
    }

    /// A test's: a memo of `slots` slots under `cap` bytes.
    #[cfg(test)]
    fn with(slots: usize, cap: usize) -> ZMemo {
        ZMemo::of(Memo::new(slots, cap))
    }

    /// The hits and misses since the last take, for the handle's counters
    /// (`Reader::zpage_counts`: `memo_hits`, `memo_misses`).
    pub(super) fn take_counts(&self) -> (u64, u64) {
        let mut m = self.memo.borrow_mut();
        let r = (m.hits, m.misses);
        m.hits = 0;
        m.misses = 0;
        r
    }

    fn get(
        &self,
        zfile: u64,
        page: u32,
        index: u8,
        hashed: bool,
    ) -> Option<(Arc<GroupBuf>, usize, u16, u16)> {
        self.memo.borrow_mut().get(zfile, page, index, hashed)
    }

    /// Whether the footprint is kept, and the switch: turned off, the bitmap
    /// and its count are dropped, since a footprint is of one stretch of
    /// the compact form serving.
    pub(super) fn tracking(&self) -> bool {
        self.track.get()
    }

    pub(super) fn track(&self, on: bool) {
        self.track.set(on);
        let mut m = self.memo.borrow_mut();
        m.track = on;
        if !on {
            m.touched = std::collections::HashMap::new();
            m.distinct = 0;
        }
    }

    /// The distinct compact pages touched since the footprint began.
    pub(super) fn footprint_pages(&self) -> u64 {
        self.memo.borrow().distinct
    }

    #[inline]
    /// `Memo::prefetch_victim`, through the cell.
    fn prefetch_victim(&self, zfile: u64, page: u32, index: u8, hashed: bool) {
        self.memo
            .borrow()
            .prefetch_victim(zfile, page, index, hashed);
    }

    fn touch(&self, zfile: u64, page: u32, n_pages: usize) {
        if self.track.get() {
            self.memo.borrow_mut().touch(zfile, page, n_pages);
        }
    }

    /// A buffer for a decode the memo will take: a displaced entry's, or
    /// a new one.
    fn fresh_buf(&self) -> GroupBuf {
        self.memo.borrow_mut().take_free()
    }

    #[allow(clippy::too_many_arguments)]
    fn put(
        &self,
        zfile: u64,
        page: u32,
        index: u8,
        hashed: bool,
        group: Arc<GroupBuf>,
        len: usize,
        first: u16,
        count: u16,
    ) {
        self.memo
            .borrow_mut()
            .put(zfile, page, index, hashed, group, len, first, count)
    }
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

/// Form file ids, one per attach: what a cache derived from one form file is
/// keyed by, so it cannot be read against another over the same range.
static NEXT_ZSIDE_ID: AtomicU64 = AtomicU64::new(1);

/// A form file, attached: its file mapped read-only, the fence and the
/// binding in the heap, and what this process has verified of its pages.
pub(super) struct ZFile {
    /// The file, mapped. Advised `MADV_RANDOM` at attach whenever the store
    /// advises its mappings at all (`ReadAdvice` other than `Normal`), and
    /// then left alone, as the ordered companion's mapping is, whatever
    /// mode the store is in at that moment or later: only point reads and
    /// counts read a form file -- the scans of a covered partition are walked
    /// raw -- and a point read is random in every phase of the store, so
    /// `Reader::advise`, which follows the workload over the segments, does
    /// not walk this. The stage that makes scans read form files is the one
    /// to decide what their advice is in a scan phase.
    map: MmapBytes,
    /// The file, open, for the advice a mapping cannot give: the clean
    /// pages of a form file a re-code took out of service dropped from the
    /// cache once its name is gone (`ZFile::drop_cache`), since a mapping's
    /// own advice reaches only the pages it maps. `None` where it did not
    /// open.
    file: Option<std::fs::File>,
    /// The mapping was advised random (`ZFile::advised`).
    advised: bool,
    /// Heap copies of the resident part's tables, never evicted.
    fence: ZFence,
    /// The filter the file stores, over every key of the partition: a
    /// read of a key it does not hold is answered absent from the heap,
    /// with no page touched. `None` for a form file built without one.
    filter: Option<super::BlockedBloom>,
    nkeys: usize,
    n_pages: usize,
    /// The hash every page's CRC is xored with, recomputed from the live
    /// segment and not taken from the file.
    bind: u32,
    /// The block coding's groups decoded by the engine's own decoder
    /// (`Options::zpages_own_decoder`), set by the store after the attach
    /// as `inplace` is; `lz4_flex`'s where not.
    pub(super) own_decoder: bool,
    /// One bit per page: the first-decode checks passed in this process,
    /// over every group of it.
    verified: Box<[AtomicU64]>,
    /// One bit per page: a check failed.
    bad: Box<[AtomicU64]>,
    /// Any check failed: the form file is out of service, and reads of the
    /// partition go to the raw form until it is dropped.
    damaged: AtomicBool,
    /// The groups decoded from this form file, in sixteens: a thread bumps
    /// it once every sixteen decodes it makes (`count_decode`), so that the
    /// count names where the decodes land -- half of the E mix's at five
    /// million keys in the first of nineteen partitions -- without a shared
    /// write on every decode from every reader thread.
    pub(super) decodes16: AtomicU64,
    /// The time those counted decodes took, nanoseconds, where this form
    /// file times them (`ZFile::time_decodes`): the heat a re-code prices
    /// (`Options::zpages_recode`), since a decode the predictor and the
    /// cache have warm costs a tenth of a cold one and the count reads them
    /// alike -- the first partition's decodes at five million keys were
    /// worth 0.13 µs each to a raw tail, the next three's 1.4.
    pub(super) decode_ns16: AtomicU64,
    /// The record-coded groups opened without a decode -- a point read's
    /// record, a seek over the heads, a walk in place -- counted in
    /// sixteens per thread where the decodes are timed: a raw-tailed form
    /// file decodes nothing, so its heat is the groups it opens, priced at
    /// what a decode costs (`Options::zpages_recode`).
    pub(super) opens16: AtomicU64,
    /// The counted decodes are timed (`Options::zpages_recode`).
    pub(super) time_decodes: bool,
    /// The sampled pages' groups are record-coded ([`ZCODING_RECORD`]):
    /// the coding this form file was built in, read from its own
    /// directories at the attach.
    coded: bool,
    /// This attach's identity, which the memo keys its groups by.
    id: u64,
    /// The trailer this attach accepted.
    ident: Trailer,
    /// Whether compressed pages are opened through the handle's memo
    /// (`Options::zpages_memo`), and whether its slot is the hashed one
    /// (`Options::zpages_memo_hash`).
    memo: bool,
    memo_hashed: bool,
    /// Whether a verified page's directory is read trusted
    /// (`Options::zpages_trusted_views`, `ZFile::view`), and whether its
    /// group is asked of the memo before any view
    /// (`Options::zpages_memo_first`, `ZFile::open_group`).
    trusted: bool,
    memo_first: bool,
    /// A raw-tail coded group read where it lies, record by record, by the
    /// cursor (`Options::zpages_inplace_walk`; `Group::Coded`), instead of
    /// decoded whole into the buffer.
    pub(super) inplace: bool,
    /// Whether a decode asks for the line of the memo entry its put will
    /// displace before it begins (`Options::zpages_memo_ahead`,
    /// `ZFile::open_group`).
    memo_ahead: bool,
    /// Whether a scan asks for the pages it will cross after its first
    /// (`Options::zpages_scan_prefetch`).
    prefetch: bool,
    /// A test's: the file's bytes in a buffer of the test's own, read in
    /// place of the mapping from `skew` bytes in, so that a page can sit at
    /// an address a mapping never gives it.
    #[cfg(test)]
    skewed: Option<(Vec<u8>, usize)>,
}

impl ZFile {
    /// The form file at `path`, if it is the form file of the live segment
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
    /// `advise` maps the file `MADV_RANDOM`, before its own sampled reads.
    /// `fence_tree` builds the fence's search tree (`ZFence::index`,
    /// `Options::zpages_fence_tree`); `memo_ahead` is
    /// `Options::zpages_memo_ahead`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn attach(
        path: &Path,
        blob: &Blob<MmapBytes>,
        id: u64,
        end_seq: u64,
        advise: bool,
        memo: impl Into<MemoShape>,
        prefetch: bool,
        trusted: bool,
        memo_first: bool,
        fence_tree: bool,
        memo_ahead: bool,
    ) -> std::result::Result<ZFile, Reject> {
        let memo = memo.into();
        let damage = |e: ZError| Reject::Damage(e.to_string());
        let map = match MmapBytes::open(path) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(Reject::Missing),
            Err(e) => return Err(Reject::Damage(format!("the file does not open: {e}"))),
        };
        // The advice before the attach's own reads -- the trailer, the
        // resident part, the sampled pages: under the kernel's default each
        // would pull a readahead window in around it, most of a small
        // form file and megabytes of a large one that the reads may never
        // want. Half of a four-megabyte file, measured, with the advice
        // after the sample; a twentieth with it here.
        if advise {
            map.advise_random();
        }
        let file = std::fs::File::open(path).ok();
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
        let (mut fence, filter) = ZFence::decode_resident(res, &t).map_err(damage)?;
        if fence_tree {
            fence.index();
        }
        let filter = if t.filter {
            Some(engine_filter(&t, filter, &fence, nkeys)?)
        } else {
            None
        };
        let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
        let mut zfile = ZFile {
            map,
            file,
            advised: advise,
            fence,
            filter,
            nkeys,
            n_pages,
            bind,
            own_decoder: false,
            verified: (0..n_pages.div_ceil(64))
                .map(|_| AtomicU64::new(0))
                .collect(),
            bad: (0..n_pages.div_ceil(64))
                .map(|_| AtomicU64::new(0))
                .collect(),
            damaged: AtomicBool::new(false),
            decodes16: AtomicU64::new(0),
            decode_ns16: AtomicU64::new(0),
            opens16: AtomicU64::new(0),
            time_decodes: false,
            coded: false,
            id: NEXT_ZSIDE_ID.fetch_add(1, Ordering::Relaxed),
            ident: t,
            memo: memo != MemoShape::Off,
            memo_hashed: memo == MemoShape::Hashed,
            trusted,
            memo_first,
            inplace: true,
            memo_ahead,
            prefetch,
            #[cfg(test)]
            skewed: None,
        };
        zfile.coded = zfile.check_sample().map_err(damage)?;
        Ok(zfile)
    }

    /// The first, the last and fourteen evenly spaced pages through
    /// `parse_page` -- the CRC under this form file's `bind32`, the tag, the
    /// header, the directory, the padding -- and their rank and count
    /// against the fence entry that names each. Every page where there are no more than
    /// sixteen. These are not first decodes: nothing is decoded and no page
    /// is marked verified. Whether the first group of a sampled page is
    /// record-coded, which is the coding the file was built in.
    fn check_sample(&self) -> std::result::Result<bool, ZError> {
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
        let mut coded = false;
        for i in 0..self.fence.len() {
            let Some(p) = self.fence.page(i).filter(|&p| p != ZNOPAGE) else {
                continue;
            };
            if want.binary_search(&(p as usize)).is_err() {
                continue;
            }
            let page = self.page_bytes(p as usize).ok_or(ZError::Length)?;
            let view = parse_page(page, self.bind)?;
            let entry = self
                .fence
                .entry(i, self.nkeys as u32)
                .ok_or(ZError::Fence)?;
            check_entry(&view, &entry)?;
            coded |= view.group(0).is_some_and(|g| g.coded);
        }
        Ok(coded)
    }

    fn page_bytes(&self, p: usize) -> Option<&[u8]> {
        #[cfg(test)]
        if let Some((bytes, skew)) = &self.skewed {
            let at = skew + p * ZPAGE;
            return bytes.get(at..at + ZPAGE);
        }
        self.map.slice_at((p as u64) * ZPAGE as u64, ZPAGE)
    }

    /// A check failed on page `p`: it and the form file leave service.
    #[cold]
    fn fail(&self, p: usize) {
        if p < self.n_pages {
            set_bit(&self.bad, p);
        }
        self.damaged.store(true, Ordering::Relaxed);
    }

    /// Entry `entry`'s group `j`, opened the one way every read opens one:
    /// through the first decode's checks where this process has not
    /// verified the page and through the verified path where it has,
    /// decoded into `buf`, taken from the memo, or parsed in place for a
    /// stored group that is aligned; with the index in the page of its
    /// first record, its record count and whether a decode was made.
    /// `None` where the page is out of range or a check failed -- the page
    /// and the form file are out of service then -- or a stored group is
    /// misaligned, which a mapping never is; the caller answers a fallback.
    /// `view` is the page's view where the caller took one to find the
    /// group, else one is taken here (`ZFile::view`). Under
    /// `Options::zpages_memo_first` a verified page's group is asked of the
    /// memo before any view, the entry carrying the group's first record
    /// and count, so a hit reads nothing of the page; as shipped the memo
    /// is asked after the view, which says whether the group is compressed,
    /// the order the shape before had. With `inplace`, a raw-tail coded
    /// group is handed back as its payload, undecoded, for a caller that
    /// reads it record by record (`ZCursor`, `Group::Coded`); every other
    /// caller reads the bytes as a decoded group and passes `false`.
    fn open_group<'a>(
        &'a self,
        memo: &ZMemo,
        entry: &FenceEntry<'_>,
        j: usize,
        buf: &'a mut GroupBuf,
        view: Option<&PageView<'a>>,
        inplace: bool,
    ) -> Option<(Opened<'a>, usize, usize, bool)> {
        let p = entry.page as usize;
        let Some(page) = self.page_bytes(p).filter(|_| p < self.n_pages) else {
            self.fail(p);
            return None;
        };
        memo.touch(self.id, entry.page, self.n_pages);
        let Ok(index) = u8::try_from(j) else {
            self.fail(p);
            return None;
        };
        let was_verified = bit(&self.verified, p);
        if self.memo_first && self.memo && was_verified {
            if let Some((g, n, first, count)) =
                memo.get(self.id, entry.page, index, self.memo_hashed)
            {
                return Some((
                    Opened::Memo(g, n),
                    usize::from(first),
                    usize::from(count),
                    false,
                ));
            }
        }
        let own;
        let view: &PageView<'a> = match view {
            Some(v) => v,
            None => match self.view(p, page) {
                Ok(v) => {
                    own = v;
                    &own
                }
                Err(_) => {
                    self.fail(p);
                    return None;
                }
            },
        };
        let Some(g) = view.group(j) else {
            self.fail(p);
            return None;
        };
        // A raw-tail coded group is read where it lies (`Group::Coded`):
        // the page verified as a first decode verifies it, every group by
        // its own check, and the payload handed back with nothing decoded
        // and nothing for the memo to hold.
        if inplace && g.coded && self.inplace {
            if let Some(payload) = view.payload(j) {
                if raw_tail_group(payload, usize::from(g.count)) {
                    if !was_verified {
                        match first_checks(page, self.bind, entry, None, self.own_decoder) {
                            Ok(_) => set_bit(&self.verified, p),
                            Err(Fail::Misaligned) => return None,
                            Err(Fail::Check(_)) => {
                                self.fail(p);
                                return None;
                            }
                        }
                    }
                    self.count_open();
                    return Some((
                        Opened::Coded(payload),
                        usize::from(g.first),
                        usize::from(g.count),
                        false,
                    ));
                }
            }
        }
        // A compressed group goes through the memo: the group as a decode
        // before left it, for a page this process has verified, since a
        // hit repeats no check as the verified path repeats none; else a
        // decode into a buffer of its own that the memo takes, the page's
        // first decode verifying every group of it. A stored group is
        // parsed where it lies and has nothing to memoise.
        if self.memo && !g.stored {
            if was_verified && !self.memo_first {
                if let Some((g, n, first, count)) =
                    memo.get(self.id, entry.page, index, self.memo_hashed)
                {
                    return Some((
                        Opened::Memo(g, n),
                        usize::from(first),
                        usize::from(count),
                        false,
                    ));
                }
            }
            // The entry this decode's put will displace, its line asked for
            // now: the put tries the displaced group's `Arc` for its last
            // reference, a load of a line last touched when that entry was
            // made, thousands of decodes ago, and the decode is long enough
            // to bring it (`Options::zpages_memo_ahead`).
            if self.memo_ahead {
                memo.prefetch_victim(self.id, entry.page, index, self.memo_hashed);
            }
            let mut fresh = memo.fresh_buf();
            let tick = self.decode_tick();
            let (first, count, ulen) = {
                let opened = if was_verified {
                    verified_group_in(view, j, &mut fresh, self.own_decoder)
                } else {
                    first_decode(page, self.bind, entry, j, &mut fresh, self.own_decoder)
                };
                match opened {
                    Ok((g, first, count, _)) => (first, count, g.len()),
                    Err(Fail::Misaligned) => return None,
                    Err(Fail::Check(_)) => {
                        self.fail(p);
                        return None;
                    }
                }
            };
            if !was_verified {
                set_bit(&self.verified, p);
            }
            self.count_decode(tick);
            let g = Arc::new(fresh);
            memo.put(
                self.id,
                entry.page,
                index,
                self.memo_hashed,
                Arc::clone(&g),
                ulen,
                first as u16,
                count as u16,
            );
            return Some((Opened::Memo(g, ulen), first, count, true));
        }
        let tick = self.decode_tick();
        let opened = if was_verified {
            verified_group_in(view, j, buf, self.own_decoder)
        } else {
            first_decode(page, self.bind, entry, j, buf, self.own_decoder)
        };
        match opened {
            Ok((g, first, count, decoded)) => {
                if !was_verified {
                    set_bit(&self.verified, p);
                }
                if decoded {
                    self.count_decode(tick);
                }
                Some((Opened::Borrowed(g), first, count, decoded))
            }
            Err(Fail::Misaligned) => None,
            Err(Fail::Check(_)) => {
                self.fail(p);
                None
            }
        }
    }

    /// Page `p`'s view over its bytes `page`: with its directory trusted
    /// (`view_trusted`) for a page this process has verified, under
    /// `Options::zpages_trusted_views`, and through every check of
    /// `page_view` otherwise. What every lookup of a group in a page goes
    /// through, so the two shapes differ here and nowhere else.
    fn view<'a>(&self, p: usize, page: &'a [u8]) -> std::result::Result<PageView<'a>, ZError> {
        if self.trusted && bit(&self.verified, p) {
            view_trusted(page)
        } else {
            page_view(page)
        }
    }

    /// Page `p`'s view, where it has bytes and they parse; `None` takes the
    /// page out of service.
    fn view_of(&self, p: usize) -> Option<PageView<'_>> {
        let v = self
            .page_bytes(p)
            .filter(|_| p < self.n_pages)
            .and_then(|page| self.view(p, page).ok());
        if v.is_none() {
            self.fail(p);
        }
        v
    }

    /// The group of page `p` that holds the record at index `i` of the
    /// page, by the directory; `None` where the page's values do not
    /// parse, the page out of service.
    fn group_of_index(&self, p: usize, i: usize) -> Option<usize> {
        let j = self
            .page_bytes(p)
            .and_then(|page| self.view(p, page).ok())
            .map(|v| v.group_of_index(i));
        if j.is_none() {
            self.fail(p);
        }
        j
    }

    /// The group of page `p` that `key` would be in, by the directory's
    /// first keys; `None` where the page's values do not parse, the page
    /// out of service. A directory a flipped byte changed is the first
    /// decode's to refuse: the group chosen is then opened and the page's
    /// CRC read before anything is answered from it.
    fn group_of_key(&self, p: usize, key: &[u8]) -> Option<usize> {
        let j = self
            .page_bytes(p)
            .and_then(|page| self.view(p, page).ok())
            .map(|v| v.group_of_key(key));
        if j.is_none() {
            self.fail(p);
        }
        j
    }

    /// The entry rank `rank` falls in, for a rank below the key count: a
    /// fence that does not cover the rank is damage, since the tiling check
    /// held every rank below the key count to an entry.
    fn entry_at_rank(&self, rank: usize) -> Option<FenceEntry<'_>> {
        u32::try_from(rank)
            .ok()
            .and_then(|r| self.fence.entry_at_rank(r))
            .and_then(|e| self.fence.entry(e, self.nkeys as u32))
            .filter(|e| {
                let first = e.first_rank as usize;
                rank >= first && rank < first.saturating_add(e.count as usize)
            })
    }

    /// Where the record at `rank` lies: its entry by the fence's first
    /// ranks, the page's group by the directory, the group opened, the walk
    /// to the record by `record_key_next`. A rank past the end is absent; an
    /// entry no page holds is the raw form's.
    fn locate_rank<'a>(&'a self, memo: &ZMemo, rank: usize, buf: &'a mut GroupBuf) -> Locate<'a> {
        if self.damaged.load(Ordering::Relaxed) {
            return Locate::Out;
        }
        if rank >= self.nkeys {
            return Locate::Absent { decoded: false };
        }
        let Some(entry) = self.entry_at_rank(rank) else {
            return Locate::Fallback;
        };
        if entry.page == ZNOPAGE {
            return Locate::NoPage;
        }
        let p = entry.page as usize;
        let first = entry.first_rank as usize;
        let Some(view) = self.view_of(p) else {
            return Locate::Fallback;
        };
        let j = view.group_of_index(rank - first);
        let Some((opened, gfirst, _, decoded)) =
            self.open_group(memo, &entry, j, buf, Some(&view), false)
        else {
            return Locate::Fallback;
        };
        let mut off = 0usize;
        for _ in first + gfirst..rank {
            let Some((_, next)) = record_key_next(opened.bytes(), off) else {
                self.fail(p);
                return Locate::Fallback;
            };
            off = next;
        }
        Locate::Found {
            opened,
            off,
            page: p,
            decoded,
        }
    }

    /// Where `key` lies among the records of this partition, opening the
    /// one page that could hold it.
    fn locate<'a>(&'a self, memo: &ZMemo, key: &[u8], buf: &'a mut GroupBuf) -> Locate<'a> {
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
        let Some(entry) = self.fence.entry(e, self.nkeys as u32) else {
            self.fail(p);
            return Locate::Fallback;
        };
        let Some(view) = self.view_of(p) else {
            return Locate::Fallback;
        };
        let j = view.group_of_key(key);
        // The record coding's point read: the group's heads are sought
        // where they lie and the one record is decoded into the read's
        // buffer, nothing else of the group and nothing of the memo. A page
        // not yet verified takes its first checks first -- its coded groups
        // by their heads, nothing of them decoded -- and is verified by
        // them; a group that is stored verbatim, or one LZ4 block because
        // only that held it, is opened as under the block coding.
        if let Some(g) = view.group(j).filter(|g| g.coded) {
            if !bit(&self.verified, p) {
                let Some(page) = self.page_bytes(p) else {
                    self.fail(p);
                    return Locate::Fallback;
                };
                match first_checks(page, self.bind, &entry, None, self.own_decoder) {
                    Ok(_) => set_bit(&self.verified, p),
                    Err(Fail::Misaligned) => return Locate::Fallback,
                    Err(Fail::Check(_)) => {
                        self.fail(p);
                        return Locate::Fallback;
                    }
                }
            }
            {
                // The page is touched, for the footprint, as `open_group`
                // touches it.
                memo.touch(self.id, entry.page, self.n_pages);
                let Some(payload) = view.payload(j) else {
                    self.fail(p);
                    return Locate::Fallback;
                };
                let count = usize::from(g.count);
                self.count_open();
                let pos = match record_group_seek(payload, count, key) {
                    Ok(pos) => pos,
                    Err(_) => {
                        self.fail(p);
                        return Locate::Fallback;
                    }
                };
                if !pos.found {
                    return Locate::Absent { decoded: false };
                }
                return match decode_record(payload, count, pos.index, buf) {
                    Ok(rec) => Locate::Found {
                        opened: Opened::Borrowed(rec),
                        off: 0,
                        page: p,
                        decoded: true,
                    },
                    Err(_) => {
                        self.fail(p);
                        Locate::Fallback
                    }
                };
            }
        }
        let Some((opened, _, count, decoded)) =
            self.open_group(memo, &entry, j, buf, Some(&view), false)
        else {
            return Locate::Fallback;
        };
        let pos = match group_seek(opened.bytes(), count, key) {
            Ok(pos) => pos,
            Err(_) => {
                self.fail(p);
                return Locate::Fallback;
            }
        };
        if !pos.found {
            return Locate::Absent { decoded };
        }
        Locate::Found {
            opened,
            off: pos.off,
            page: p,
            decoded,
        }
    }

    /// `key`'s values in this partition, handed to `f` in the order the raw
    /// form hands them. `raw` is the partition's own segment, which reads a
    /// run that lies in one of its blocks, as the raw form does.
    pub(super) fn read_all<F: FnMut(&[u8])>(
        &self,
        memo: &ZMemo,
        raw: &Blob<MmapBytes>,
        key: &[u8],
        f: &mut F,
    ) -> Result<ZRead> {
        let mut buf = take_buf();
        let r = match self.locate(memo, key, &mut buf) {
            Locate::Absent { decoded } => Ok(ZRead::Served { n: 0, decoded }),
            Locate::Out => Ok(ZRead::Out),
            Locate::Fallback => Ok(ZRead::Fallback),
            Locate::NoPage => Ok(ZRead::NoPage),
            Locate::Found {
                opened,
                off,
                page,
                decoded,
            } => match parse_record(opened.bytes(), off) {
                Some((_, exts, tail, _)) => raw
                    .read_exts(&exts, tail, &mut *f)
                    .map(|n| ZRead::Served { n, decoded }),
                None => {
                    // The first decode walked every record whole: a group
                    // changed under the mapping, the page out of service.
                    self.fail(page);
                    Ok(ZRead::Fallback)
                }
            },
        };
        put_buf(buf);
        r
    }

    /// How many values `key` has in this partition.
    pub(super) fn count(&self, memo: &ZMemo, key: &[u8]) -> ZRead {
        let mut buf = take_buf();
        let r = match self.locate(memo, key, &mut buf) {
            Locate::Absent { decoded } => ZRead::Served { n: 0, decoded },
            Locate::Out => ZRead::Out,
            Locate::Fallback => ZRead::Fallback,
            Locate::NoPage => ZRead::NoPage,
            Locate::Found {
                opened,
                off,
                page,
                decoded,
            } => match parse_record(opened.bytes(), off) {
                Some((_, exts, _, _)) => ZRead::Served {
                    n: exts.iter().map(|e| u64::from(e.records())).sum(),
                    decoded,
                },
                None => {
                    self.fail(page);
                    ZRead::Fallback
                }
            },
        };
        put_buf(buf);
        r
    }

    /// `Blob::scan_at` through the form file: the keys from rank `from` on,
    /// at most `limit` of them, each key's values handed to `f` in the raw
    /// form's order, and how many keys were visited -- a key whose values
    /// are all gone counts and emits nothing, a rank past the end walks
    /// nothing, and the arithmetic saturates, as the raw walk's does. The
    /// entry the rank falls in is found once, by the fence's first ranks,
    /// and the group in its page by the directory; the group is decoded
    /// into the thread's buffer (a first decode makes the checks the point
    /// read makes, and marks the group verified), the walk skips to the
    /// record by `record_key_next` and parses each record from there,
    /// handing its extents to `raw.read_exts` as the point read does; a
    /// spent group is followed by the page's next, then the next entry's. An
    /// entry no page holds hands the raw form exactly its records. The
    /// pages touched are the groups the walk crosses and nothing of the
    /// raw segment's records, directory or hash, except where a run lies
    /// in a block, which the raw form reads as it would.
    pub(super) fn scan_at<F: FnMut(&[u8], &[u8])>(
        &self,
        memo: &ZMemo,
        raw: &Blob<MmapBytes>,
        from: usize,
        limit: usize,
        f: &mut F,
    ) -> Result<ZScan> {
        let mut out = ZScan {
            seen: 0,
            decoded: 0,
            inplace: 0,
            nopage: 0,
            prefetches: 0,
            end: ScanEnd::Served,
        };
        if self.damaged.load(Ordering::Relaxed) {
            out.end = ScanEnd::Out;
            return Ok(out);
        }
        let mut buf = take_buf();
        let r = self.scan_in(memo, raw, from, limit, f, &mut buf, &mut out);
        put_buf(buf);
        r.map(|()| out)
    }

    /// The walk of `scan_at`, with the thread's buffer lent to it; a check
    /// that fails ends it at a record boundary with `out.end` saying so.
    #[allow(clippy::too_many_arguments)]
    fn scan_in<F: FnMut(&[u8], &[u8])>(
        &self,
        memo: &ZMemo,
        raw: &Blob<MmapBytes>,
        from: usize,
        limit: usize,
        f: &mut F,
        buf: &mut GroupBuf,
        out: &mut ZScan,
    ) -> Result<()> {
        let nkeys = self.nkeys;
        let mut rank = from;
        let mut asked = false;
        // The key of a record read in place, assembled (`Opened::Coded`).
        let mut key: Vec<u8> = Vec::new();
        while out.seen < limit && rank < nkeys {
            // The entry the rank falls in, and how far its records go: a
            // fence that does not cover the rank is damage, since the
            // tiling check held every rank below the key count to an
            // entry.
            let entry = u32::try_from(rank)
                .ok()
                .and_then(|r| self.fence.entry_at_rank(r))
                .and_then(|e| self.fence.entry(e, self.nkeys as u32));
            let Some(entry) = entry else {
                out.end = ScanEnd::Fallback;
                return Ok(());
            };
            let first = entry.first_rank as usize;
            let end = first.saturating_add(entry.count as usize).min(nkeys);
            if rank < first || rank >= end {
                out.end = ScanEnd::Fallback;
                return Ok(());
            }
            let want = (limit - out.seen).min(end - rank);
            if entry.page == ZNOPAGE {
                // The records the form does not hold: the raw form's walk,
                // for exactly them. One that stops short is the raw form's
                // own damage, which the caller's raw walk meets again and
                // names.
                let n = raw.scan_at(rank, want, &mut *f)?;
                out.nopage += 1;
                out.seen += n;
                rank += n;
                if n < want {
                    out.end = ScanEnd::Fallback;
                    return Ok(());
                }
                continue;
            }
            let p = entry.page as usize;
            // The page's view once, for the group the rank falls in and for
            // each group of it opened below.
            let Some(view) = self.view_of(p) else {
                out.end = ScanEnd::Fallback;
                return Ok(());
            };
            let mut j = view.group_of_index(rank - first);
            // The page's groups from the one the rank falls in, each
            // opened once and walked to its end, the limit or the entry's
            // end.
            while out.seen < limit && rank < end {
                let Some((opened, gfirst, count, decoded)) =
                    self.open_group(memo, &entry, j, buf, Some(&view), self.inplace)
                else {
                    out.end = ScanEnd::Fallback;
                    return Ok(());
                };
                if matches!(opened, Opened::Coded(_)) {
                    // A raw-tail coded group read where it lies: the
                    // records from the one the rank names, each handed
                    // over by `coded_record_to`, nothing decoded. The
                    // payload is taken from the view, whose borrow is the
                    // mapping's, so the buffer is free for a record that
                    // has to be decoded on its own.
                    out.inplace += 1;
                    if !asked {
                        asked = true;
                        let past = (limit - out.seen).saturating_sub(end - rank);
                        if self.ask_ahead(p, past) {
                            out.prefetches += 1;
                        }
                    }
                    let Ok(g) = view
                        .payload(j)
                        .ok_or(ZError::Length)
                        .and_then(|pl| CodedGroup::open(pl, count))
                    else {
                        self.fail(p);
                        out.end = ScanEnd::Fallback;
                        return Ok(());
                    };
                    let gstart = first + gfirst;
                    let gend = gstart.saturating_add(count).min(end);
                    let want = (limit - out.seen).min(gend.saturating_sub(rank));
                    let from = rank.saturating_sub(gstart);
                    for i in from..from + want {
                        if !coded_record_to(raw, &g, i, &mut key, buf, f)? {
                            self.fail(p);
                            out.end = ScanEnd::Fallback;
                            return Ok(());
                        }
                        out.seen += 1;
                        rank += 1;
                    }
                    j += 1;
                    continue;
                }
                let group = opened.bytes();
                if decoded {
                    out.decoded += 1;
                }
                // The pages the walk will cross after this one, asked for
                // at once (3.5, `Options::zpages_scan_prefetch`): the keys
                // still wanted past this entry over the mean records a
                // page, at most `ZPREFETCH_PAGES`, at the first page the
                // walk opens and never again. `MADV_WILLNEED` reads them
                // in the background; a page the walk never reaches was a
                // read the device made for nothing, which is the bound's
                // reason.
                if !asked {
                    asked = true;
                    let past = (limit - out.seen).saturating_sub(end - rank);
                    if self.ask_ahead(p, past) {
                        out.prefetches += 1;
                    }
                }
                let gstart = first + gfirst;
                let gend = gstart.saturating_add(count).min(end);
                let want = (limit - out.seen).min(gend.saturating_sub(rank));
                // The skip to the record the rank names, then each record
                // from there: a record that does not parse is a group the
                // checks let through, which is damage all the same.
                let mut off = 0usize;
                for _ in gstart..rank {
                    let Some((_, next)) = record_key_next(group, off) else {
                        self.fail(p);
                        out.end = ScanEnd::Fallback;
                        return Ok(());
                    };
                    off = next;
                }
                for _ in 0..want {
                    let Some((k, exts, tail, len)) = parse_record(group, off) else {
                        self.fail(p);
                        out.end = ScanEnd::Fallback;
                        return Ok(());
                    };
                    raw.read_exts(&exts, tail, &mut |v: &[u8]| f(k, v))?;
                    off += len;
                    out.seen += 1;
                    rank += 1;
                }
                j += 1;
            }
        }
        Ok(())
    }

    /// The pages a walk will cross after page `p`, asked for at once
    /// (`Options::zpages_scan_prefetch`): `past` the keys still wanted past
    /// the page's entry, over the mean records a page, at most
    /// `ZPREFETCH_PAGES` and never past the last page. `MADV_WILLNEED`
    /// reads them in the background; a page the walk never reaches was a
    /// read the device made for nothing, which is the bound's reason.
    /// Whether it asked: nothing with the option off, nothing wanted past
    /// the page, or no page after it.
    fn ask_ahead(&self, p: usize, past: usize) -> bool {
        if !self.prefetch || past == 0 {
            return false;
        }
        let mean = (self.nkeys / self.n_pages.max(1)).max(1);
        let k = past
            .div_ceil(mean)
            .min(ZPREFETCH_PAGES)
            .min(self.n_pages.saturating_sub(p + 1));
        if k == 0 {
            return false;
        }
        self.map
            .advise_willneed(((p + 1) * ZPAGE) as u64, k * ZPAGE);
        true
    }

    /// `Blob::values_at` through the form file: the values of the record at
    /// `rank`, handed to `f` in the raw form's order, or why not -- what a
    /// scan's emit of one partition record asks (`Reader::emit_unsealed`).
    pub(super) fn values_at<F: FnMut(&[u8])>(
        &self,
        memo: &ZMemo,
        raw: &Blob<MmapBytes>,
        rank: usize,
        f: &mut F,
    ) -> Result<ZRead> {
        let mut buf = take_buf();
        let r = match self.locate_rank(memo, rank, &mut buf) {
            Locate::Absent { decoded } => Ok(ZRead::Served { n: 0, decoded }),
            Locate::Out => Ok(ZRead::Out),
            Locate::Fallback => Ok(ZRead::Fallback),
            Locate::NoPage => Ok(ZRead::NoPage),
            Locate::Found {
                opened,
                off,
                page,
                decoded,
            } => match parse_record(opened.bytes(), off) {
                Some((_, exts, tail, _)) => raw
                    .read_exts(&exts, tail, &mut *f)
                    .map(|n| ZRead::Served { n, decoded }),
                None => {
                    // The first decode walked every record whole: a group
                    // changed under the mapping, the page out of service.
                    self.fail(page);
                    Ok(ZRead::Fallback)
                }
            },
        };
        put_buf(buf);
        r
    }

    /// `Blob::key_at` through the form file: the key of the record at `rank`
    /// copied into `out`, for what the block cache asks of a partition by
    /// rank and keeps -- a block's boundary key, the last key a table holds,
    /// the key a lazy scan resumes from (`Reader::cache_key_at`). Answers as
    /// `values_at` does: served with one key, or none past the end; a record
    /// no page holds the raw form's; a check that failed a fallback; out of
    /// service. `out` is left as it was unless a key was served. The key of
    /// a record no page holds is the fence's: its entry covers that one
    /// record and names its key, so the key is answered from the heap as
    /// the seek answers it, and only the record's values are the raw
    /// form's -- the last key of a partition whose last record no page
    /// holds is what a block table keeps, and it was a touch of the raw
    /// form until the fence answered it.
    pub(super) fn key_at_into(&self, memo: &ZMemo, rank: usize, out: &mut Vec<u8>) -> ZRead {
        if !self.damaged.load(Ordering::Relaxed) && rank < self.nkeys {
            if let Some(e) = self.entry_at_rank(rank) {
                if e.page == ZNOPAGE && e.count == 1 {
                    out.clear();
                    out.extend_from_slice(e.key);
                    return ZRead::Served {
                        n: 1,
                        decoded: false,
                    };
                }
            }
        }
        let mut buf = take_buf();
        let r = match self.locate_rank(memo, rank, &mut buf) {
            Locate::Absent { decoded } => ZRead::Served { n: 0, decoded },
            Locate::Out => ZRead::Out,
            Locate::Fallback => ZRead::Fallback,
            Locate::NoPage => ZRead::NoPage,
            Locate::Found {
                opened,
                off,
                page,
                decoded,
            } => match record_key_next(opened.bytes(), off) {
                Some((k, _)) => {
                    out.clear();
                    out.extend_from_slice(k);
                    ZRead::Served { n: 1, decoded }
                }
                None => {
                    // The first decode walked every record whole: a group
                    // changed under the mapping, the page out of service.
                    self.fail(page);
                    ZRead::Fallback
                }
            },
        };
        put_buf(buf);
        r
    }

    /// `Blob::seek` through the form file (3.4): the rank of the first key at
    /// or after `key`, by the fence and one group's walk. A key below the
    /// first key is rank zero and one above every key is the key count,
    /// each `Greater` with nothing decoded; a key in an entry no page holds
    /// is placed by the entry's one key, since the entry covers that one
    /// record. The seek takes the fence (heap) and the one page the key
    /// falls in, where the ordered index takes its heads and a key read of
    /// the raw records for each probe.
    pub(super) fn seek(&self, memo: &ZMemo, key: &[u8]) -> ZSeek {
        if self.damaged.load(Ordering::Relaxed) {
            return ZSeek::Out;
        }
        let Some(e) = self.fence.entry_of(key) else {
            return ZSeek::Served {
                rank: 0,
                at: KeyOrder::Greater,
                decoded: false,
            };
        };
        let Some(entry) = self.fence.entry(e, self.nkeys as u32) else {
            return ZSeek::Fallback;
        };
        let first = entry.first_rank as usize;
        if entry.page == ZNOPAGE {
            if entry.count != 1 {
                return ZSeek::NoPage;
            }
            return if entry.key == key {
                ZSeek::Served {
                    rank: first,
                    at: KeyOrder::Equal,
                    decoded: false,
                }
            } else {
                ZSeek::Served {
                    rank: first + 1,
                    at: KeyOrder::Greater,
                    decoded: false,
                }
            };
        }
        let Some(view) = self.view_of(entry.page as usize) else {
            return ZSeek::Fallback;
        };
        let j = view.group_of_key(key);
        // A record-coded group is sought over its heads where it lies
        // (`record_group_seek`), as the point read seeks one, nothing
        // decoded and nothing put in the memo; the page is verified at its
        // first touch as a first decode verifies it, every group by its own
        // check. The first version opened the group as a walk opens one,
        // every record copy-decoded for the raw-tail form, and under the
        // in-place walk the decode served nobody: 1.5 groups a scan in E,
        // the start seek's and the cut's, where the walk decoded none.
        if let Some(g) = view.group(j).filter(|g| g.coded) {
            let p = entry.page as usize;
            if let (Some(page), Some(payload)) = (
                self.page_bytes(p).filter(|_| p < self.n_pages),
                view.payload(j),
            ) {
                memo.touch(self.id, entry.page, self.n_pages);
                if !bit(&self.verified, p) {
                    match first_checks(page, self.bind, &entry, None, self.own_decoder) {
                        Ok(_) => set_bit(&self.verified, p),
                        Err(Fail::Misaligned) => return ZSeek::Fallback,
                        Err(Fail::Check(_)) => {
                            self.fail(p);
                            return ZSeek::Fallback;
                        }
                    }
                }
                self.count_open();
                return match record_group_seek(payload, usize::from(g.count), key) {
                    Ok(pos) => ZSeek::Served {
                        rank: first + usize::from(g.first) + pos.index,
                        at: if pos.found {
                            KeyOrder::Equal
                        } else {
                            KeyOrder::Greater
                        },
                        decoded: false,
                    },
                    Err(_) => {
                        self.fail(p);
                        ZSeek::Fallback
                    }
                };
            }
        }
        let mut buf = take_buf();
        let r = match self.open_group(memo, &entry, j, &mut buf, Some(&view), false) {
            None => ZSeek::Fallback,
            Some((opened, gfirst, count, decoded)) => {
                match group_seek(opened.bytes(), count, key) {
                    Ok(pos) => ZSeek::Served {
                        rank: first + gfirst + pos.index,
                        at: if pos.found {
                            KeyOrder::Equal
                        } else {
                            KeyOrder::Greater
                        },
                        decoded,
                    },
                    Err(_) => {
                        self.fail(entry.page as usize);
                        ZSeek::Fallback
                    }
                }
            }
        };
        put_buf(buf);
        r
    }

    /// The ranks the owner of `key` lies in -- the rank of the first key at
    /// or after `key` where that key is `key`, the rank before it otherwise,
    /// which is the rank whose block a block cache files the key under --
    /// by the fence and the page's directory alone, no group opened:
    /// `lo..hi` over the group the key falls in, or the one rank the fence
    /// or the group's first key names. A key below the first key is owned
    /// by rank zero. `None` where the form file cannot say without a decode,
    /// or at all: out of service, an entry of several records no page
    /// holds, a page whose view does not check (taken out of service, as
    /// `group_of_key` takes it).
    pub(super) fn owner_span(&self, key: &[u8]) -> Option<(usize, usize)> {
        let mut none = usize::MAX;
        self.owner_span_from(key, &mut none)
    }

    /// `owner_span` for a batch of keys in key order, each from the entry
    /// the one before was found in: `hint` is that entry, taken as the
    /// fence's `entry_of_from` takes it and left at the entry found, so a
    /// run of keys over one page costs a compare or two each where the
    /// search from the top costs the fence's depth (the settle's filing of
    /// a sorted batch, `Reader::lazy_block`). `usize::MAX` is no hint.
    pub(super) fn owner_span_from(&self, key: &[u8], hint: &mut usize) -> Option<(usize, usize)> {
        if self.damaged.load(Ordering::Relaxed) {
            return None;
        }
        let Some(e) = self.fence.entry_of_from(key, *hint) else {
            return Some((0, 1));
        };
        *hint = e;
        let entry = self.fence.entry(e, self.nkeys as u32)?;
        let first = entry.first_rank as usize;
        if entry.page == ZNOPAGE {
            return (entry.count == 1).then_some((first, first + 1));
        }
        let p = entry.page as usize;
        let Some(v) = self.page_bytes(p).and_then(|page| self.view(p, page).ok()) else {
            self.fail(p);
            return None;
        };
        let j = v.group_of_key(key);
        let g = v.group(j)?;
        let lo = first + usize::from(g.first);
        let named = if j == 0 { Some(entry.key) } else { v.key(j) };
        if named == Some(key) {
            return Some((lo, lo + 1));
        }
        Some((lo, lo + usize::from(g.count)))
    }

    /// `BuildCtx::cut_at` through the form file: the first rank in
    /// `rank..end` whose key is not below `uk`, or `end`, with how that
    /// rank's key compares to `uk`. The seek answers for the whole
    /// partition: a rank below `rank` means the key at `rank` is past `uk`,
    /// since the keys are distinct and ascend.
    pub(super) fn cut_in(&self, memo: &ZMemo, rank: usize, end: usize, uk: &[u8]) -> ZSeek {
        if rank >= end {
            return ZSeek::Served {
                rank: end,
                at: KeyOrder::Greater,
                decoded: false,
            };
        }
        match self.seek(memo, uk) {
            ZSeek::Served {
                rank: r,
                at,
                decoded,
            } => {
                let (rank, at) = if r < rank {
                    (rank, KeyOrder::Greater)
                } else if r >= end {
                    (end, KeyOrder::Greater)
                } else {
                    (r, at)
                };
                ZSeek::Served { rank, at, decoded }
            }
            other => other,
        }
    }

    /// The form file is out of service.
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

    /// Heap bytes the form file keeps resident: the fence and the filter.
    pub(super) fn resident_bytes(&self) -> usize {
        self.fence.heap_bytes() + self.filter.as_ref().map_or(0, |f| f.blocks.len() * 64)
    }

    /// A group's decode begun: whether it is the one in sixteen this
    /// thread counts (`ZFile::decodes16`), with its start where this form
    /// file times the decodes it counts (`ZFile::time_decodes`).
    fn decode_tick(&self) -> (bool, Option<std::time::Instant>) {
        thread_local! {
            static TICK: Cell<u32> = const { Cell::new(0) };
        }
        let due = TICK.with(|t| {
            let v = t.get().wrapping_add(1);
            t.set(v);
            v % 16 == 0
        });
        (
            due,
            (due && self.time_decodes).then(std::time::Instant::now),
        )
    }

    /// A group's decode done: counted, and its time added, where
    /// `decode_tick` said so.
    fn count_decode(&self, tick: (bool, Option<std::time::Instant>)) {
        if tick.0 {
            self.decodes16.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(t0) = tick.1 {
            let ns = t0.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            self.decode_ns16.fetch_add(ns, Ordering::Relaxed);
        }
    }

    /// A record-coded group opened without a decode, counted once in
    /// sixteen per thread where the decodes are timed (`ZFile::opens16`).
    fn count_open(&self) {
        if !self.time_decodes {
            return;
        }
        thread_local! {
            static TICK: Cell<u32> = const { Cell::new(0) };
        }
        let due = TICK.with(|t| {
            let v = t.get().wrapping_add(1);
            t.set(v);
            v % 16 == 0
        });
        if due {
            self.opens16.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The form file is the record coding's (`ZFile::coded`).
    pub(super) fn coded(&self) -> bool {
        self.coded
    }

    /// The file's bytes.
    pub(super) fn file_bytes(&self) -> u64 {
        self.map.len()
    }

    /// This form file's pages out of the cache: the mapping's entries
    /// dropped (`MADV_DONTNEED`), then the file's clean pages
    /// (`POSIX_FADV_DONTNEED`), which a drop of a mapped page skips. For the
    /// form file a re-code took out of service and kept
    /// (`Options::zpages_recode_keep`): a reader still walking it faults the
    /// page back, and the bytes are the same.
    pub(super) fn drop_cache(&self) {
        self.map.advise_dontneed();
        if let Some(f) = &self.file {
            super::fadvise_dontneed(f);
        }
    }

    /// The whole file asked for in the background, a window of
    /// `ZPREFETCH_WINDOW` at a time, since the kernel submits one request's
    /// worth of an advice and drops the rest: the form file a re-code put
    /// back in service, whose pages its drop took. The bytes submitted.
    pub(super) fn prefetch_all(&self) -> u64 {
        let len = self.map.len();
        let mut off = 0u64;
        let mut sent = 0u64;
        while off < len {
            let n = (len - off).min(ZPREFETCH_WINDOW as u64) as usize;
            sent += self.map.advise_willneed(off, n) as u64;
            off += n as u64;
        }
        sent
    }

    /// The decoded bytes of every group the file holds, as its trailer says.
    pub(super) fn group_bytes(&self) -> u64 {
        self.ident.total_ulen
    }

    /// This attach's number: no other form file attached in this process has
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
        return Err("the form file names another segment");
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

/// The cell a segment keeps its form file in: a pointer set from null by
/// `install`, read by an acquire load, cleared by `take` -- whose caller
/// retires what it took through the epoch, as a replaced state is -- and
/// freed with the segment if it still holds anything. Not a `OnceLock`,
/// whose initialiser takes a lock a reader could wait on.
pub(super) struct ZCell {
    p: AtomicPtr<ZFile>,
}

impl ZCell {
    pub(super) const fn new() -> ZCell {
        ZCell {
            p: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// The form file, if one is installed. Valid for the operation that
    /// reads it, which pins an epoch: a form file `take` removed is retired
    /// under a later epoch and freed only past every pin at or before it,
    /// so a reader that loaded the pointer keeps it until it leaves.
    #[inline]
    pub(super) fn get(&self) -> Option<&ZFile> {
        let p = self.p.load(Ordering::Acquire);
        // SAFETY: set from `Box::into_raw` by `install`; cleared only by
        // `take`, whose caller frees it past every pinned reader, and by
        // `Drop`, which needs `&mut self`.
        unsafe { p.as_ref() }
    }

    /// The form file taken out of the cell, or null: the caller owns the
    /// pointer and retires it through the epoch (`RetiredZ`), since a
    /// reader pinned before the take may still be walking it.
    pub(super) fn take(&self) -> *mut ZFile {
        self.p.swap(std::ptr::null_mut(), Ordering::AcqRel)
    }

    /// `z` installed in place of whatever the cell held, which is handed
    /// back for the caller to retire through the epoch as `take`'s is: a
    /// reader that loaded the old pointer before the swap may still be
    /// walking it, and one after it finds `z`, never an empty cell.
    pub(super) fn replace(&self, z: ZFile) -> *mut ZFile {
        self.p.swap(Box::into_raw(Box::new(z)), Ordering::AcqRel)
    }

    /// The cell's form file and `kept`'s exchanged, where `kept` holds one:
    /// a reader finds the old or the new, never an empty cell, and one that
    /// loaded the old keeps it, since `kept` holds it on and frees nothing.
    /// A cell of kept form files is never read by a reader, and only the
    /// segment work's thread changes one, so the two stores need not be
    /// one. Whether there was one to exchange.
    pub(super) fn flip(&self, kept: &ZCell) -> bool {
        let k = kept.p.load(Ordering::Acquire);
        if k.is_null() {
            return false;
        }
        let old = self.p.swap(k, Ordering::AcqRel);
        kept.p.store(old, Ordering::Release);
        true
    }

    /// `p`, a form file taken out of another cell, kept in this one; what
    /// this one held is handed back for the caller to retire through the
    /// epoch, as `take`'s is.
    pub(super) fn keep(&self, p: *mut ZFile) -> *mut ZFile {
        self.p.swap(p, Ordering::AcqRel)
    }

    /// `z` installed if the cell is empty; whether it was.
    pub(super) fn install(&self, z: ZFile) -> bool {
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
    /// Form files built, the pages and bytes of them, pages that hold their
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
    /// Failed builds tried again after their backoff; form files taken out
    /// of service by a page that failed a check, retired and built again;
    /// partitions declined for the life of their segment after the third
    /// such rebuild.
    pub build_retries: AtomicU64,
    pub rebuilds: AtomicU64,
    pub declined_damage: AtomicU64,
    /// Partitions re-coded while served (`Options::zpages_recode`): to the
    /// raw tails, back to the block coding, and re-codes that built nothing
    /// to install -- failed, declined, cancelled or outrun.
    pub recodes: AtomicU64,
    pub recodes_back: AtomicU64,
    pub recode_failed: AtomicU64,
    /// The re-code policy's looks, one a period it read the heat at.
    pub recode_looks: AtomicU64,
    /// Re-codes made by exchanging the served form file with the kept one.
    pub recode_flips: AtomicU64,
    /// Spares built and kept (`Options::zpages_both`), and spares that kept
    /// nothing.
    pub spare_builds: AtomicU64,
    pub spare_failed: AtomicU64,
    /// Builds started on the builder's thread, and builds its cancel flag
    /// stopped: a merge took the target as an input, or the store closed.
    pub build_starts: AtomicU64,
    pub build_cancelled: AtomicU64,
    /// Builds collected dormant, ahead of the controller's decision
    /// (`ZCounts::prebuilt`).
    pub prebuilt: AtomicU64,
    /// The I/O class the kernel reported for the last build thread
    /// (`ZCounts::build_io_class`).
    pub build_io_class: AtomicU64,
    /// The raw record bytes the builds asked for ahead of their walks.
    pub build_ahead_bytes: AtomicU64,
    /// Records the builds held no page for, each an entry of the fence
    /// read from the raw form.
    pub build_nopage: AtomicU64,
    /// Warms of a form file (`warm_file`): how many, the bytes read,
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
    /// Form files attached, and files an attach refused: damaged, or bound to
    /// another segment.
    pub attached: AtomicU64,
    pub rejected_damage: AtomicU64,
    pub rejected_identity: AtomicU64,
    /// Files a sweep of the directory removed: form files no live segment
    /// claims, and the temp names of builds that did not finish.
    pub swept: AtomicU64,
    /// What the last refusal said: a count says a file was refused and not
    /// which check refused it, and a test that breaks a field wants to see
    /// that it was the check for the field.
    pub last_reject: std::sync::Mutex<String>,
    /// The controller's (`ZPages::Auto`, `zselect`), published by the
    /// segment work's tick for whoever asks: the phase (`Phase::code`),
    /// whether the handles run the census, and the selector's counts --
    /// steps, model ticks, switches, trials and trials that came back.
    pub phase: AtomicU64,
    pub census: AtomicU64,
    pub sel_steps: AtomicU64,
    pub sel_s1: AtomicU64,
    pub sel_switches: AtomicU64,
    pub sel_trials: AtomicU64,
    pub sel_trials_back: AtomicU64,
    /// The census's: the ticks taken, which a handle's window counts its
    /// length against; the mixed windows and the threads the selector
    /// holds; the costs the tick last measured and the ones the selector
    /// holds, nanoseconds (0: none), in the order of `Costs`: a fault, a
    /// read and a scan that hit, a read and a scan through the form.
    pub ticks: AtomicU64,
    pub sel_mixed: AtomicU64,
    pub sel_threads: AtomicU64,
    pub census_costs: [AtomicU64; 5],
    pub sel_costs: [AtomicU64; 5],
    /// The sampler's: jobs run, partitions sampled in them, partitions the
    /// sample declined, and the jobs' wall in microseconds; and the
    /// selector's counts of what it asked and did: samples wanted, builds
    /// started, installed, cancelled and suspended, the exits by kind, and
    /// retentions that ended with the form files retired.
    pub samples: AtomicU64,
    pub sampled: AtomicU64,
    pub sample_declined: AtomicU64,
    pub sample_us: AtomicU64,
    /// The sampler's estimate of the form's bytes over the partitions it
    /// counted, at its last reading (`Sample::w_z` summed).
    pub sample_w_z: AtomicU64,
    /// Where the selector chooses the coding (`Options::zpages_coding_auto`):
    /// the raw-tail form's bytes at the same reading, the codings chosen
    /// (block, then raw tails) and the builds started under the raw tails.
    pub sample_w_z_raw: AtomicU64,
    pub coding_picks: [AtomicU64; 2],
    pub builds_raw_tails: AtomicU64,
    /// The partitions whose raw pages were dropped at their form file's
    /// install (`Options::zpages_drop_raw`).
    pub raw_dropped: AtomicU64,
    pub sel_samples: AtomicU64,
    pub sel_builds: AtomicU64,
    pub sel_installs: AtomicU64,
    pub sel_cancelled: AtomicU64,
    pub sel_suspended: AtomicU64,
    pub sel_exits: [AtomicU64; 3],
    pub sel_retired: AtomicU64,
    /// The rent toward the build's price and the price, nanoseconds, as
    /// the selector holds them.
    pub sel_excess_ns: AtomicU64,
    pub sel_c_build_ns: AtomicU64,
    /// E2's footprint as the tick last computed it: the compact pages the
    /// handles touched, summed, and the raw bound in bytes.
    pub footprint_pages: AtomicU64,
    pub footprint_bytes: AtomicU64,
    /// The model's two populations as the tick last summed them over the
    /// partitions, bytes: the hash regions and the record regions.
    pub model_hash: AtomicU64,
    pub model_records: AtomicU64,
    /// Form files the controller retired: taken out of their cells through
    /// the epoch and given dead names for the idle sweep.
    pub retired: AtomicU64,
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
        ZEvents::add(&self.build_ahead_bytes, b.asked);
    }

    /// A warm, counted.
    pub(super) fn warmed(&self, w: &Warm) {
        ZEvents::add(&self.warms, 1);
        ZEvents::add(&self.warm_bytes, w.bytes);
        ZEvents::add(&self.warm_skipped, w.skipped);
        ZEvents::add(&self.warm_us, w.us);
    }
}

/// Why a partition was not given a form file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decline {
    /// The record region is under the floor.
    Small(usize),
    /// A record's run lies in a block, so the value is read from the raw
    /// segment whatever the form holds.
    BlockBacked,
    /// No record the partition holds fits a page: every one is an entry of
    /// no page, and a form file of those alone serves nothing.
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

/// The code `Seg::z_declined` keeps for a partition whose form file failed a
/// check after `ZREBUILDS` rebuilds: left raw for the segment's life, so a
/// defect the build cannot see or a medium that keeps failing is a counted
/// decline and not a build a cycle.
pub(super) const DECLINED_DAMAGE: u8 = 8;

/// How many times a segment's form file is built again after a page of it
/// failed a check.
pub(super) const ZREBUILDS: u8 = 3;

/// The name a retired form file is renamed to: a rename frees no inode, where
/// an unlink is a device round trip here, so the landing that retires a
/// segment renames and the idle tick unlinks; a dead name no live segment
/// claims, so a sweep at open or at the end of a settle removes what a
/// crash left.
pub(super) fn zpg_dead_name(n: u64) -> String {
    format!("zpg-dead-{n:08}.zpg")
}

/// The code `Seg::z_declined` keeps for a partition whose form file an
/// attach refused, which was counted when it was: a build replaces the file
/// and the attach is not asked again of the same file, or it would be
/// counted twice. Unlike the others it is not a reason to leave the
/// partition raw, and a settle still gives it a form file.
pub(super) const DECLINED_REFUSED: u8 = 7;

/// What a build made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Built {
    /// Pages, and what `fill_page` did to make them.
    pub fill: FillStats,
    /// The file's length.
    pub file_bytes: u64,
    /// The decoded bytes of every group.
    pub total_ulen: u64,
    /// The raw record bytes asked for ahead of the walk (`BuildPlan::ahead`).
    pub asked: u64,
}

/// What a warm of a form file did (`warm_file`).
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
/// of a form file that is on disk and cold -- after a restart -- fault on
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

/// Ask the kernel for `[from, to)` of the mapping at `base` ahead of a walk
/// (`MADV_WILLNEED`): the reads are issued at once and the walk's faults
/// then find the pages in flight or resident, so one wait of the walk's
/// fetches the window and not the device's readahead. The start is taken
/// down to its page; the call is advice and its failure is nobody's.
#[cfg(target_os = "linux")]
fn ask_ahead(base: usize, from: usize, to: usize) {
    let a = from & !(ZPAGE - 1);
    if a >= to {
        return;
    }
    // SAFETY: a read-only shared mapping of a file, advised over a range
    // inside it; the kernel reads pages in and changes nothing else.
    unsafe {
        libc::madvise((base + a) as *mut libc::c_void, to - a, libc::MADV_WILLNEED);
    }
}

#[cfg(not(target_os = "linux"))]
fn ask_ahead(_base: usize, _from: usize, _to: usize) {}

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
    /// The bytes of the raw records asked for ahead of the walk, a window
    /// at a time (`Options::zpages_build_ahead`); zero asks for nothing,
    /// and the walk's page faults fetch the device's readahead and no more.
    pub ahead: usize,
    /// The decoded bytes a page's groups are cut at
    /// (`Options::zpages_group_bytes`, `FillScratch::group_bytes`); zero is
    /// one group a page.
    pub group_bytes: usize,
    /// The coding of the groups that compress (`Options::zpages_record_coding`,
    /// `FillScratch::coding`): `ZCODING_RECORD` as the engine ships,
    /// `ZCODING_BLOCK` the shape before.
    pub coding: u8,
    /// Every group stored verbatim, LZ4 never tried
    /// (`Options::zpages_store_all`, `FillScratch::store_all`).
    pub store_all: bool,
    /// Under the record coding, every tail stored raw
    /// (`Options::zpages_raw_tails`, `FillScratch::raw_tails`).
    pub raw_tails: bool,
    /// The block coding's groups written by the parsing encoder
    /// (`Options::zpages_parse`, `FillScratch::parse`).
    pub parse: bool,
    /// The pages the build writes checked through the engine's own decoder
    /// (`Options::zpages_own_decoder`, `FillScratch::own_decode`), the one
    /// the store's readers take.
    pub own_decode: bool,
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
            ahead: 0,
            group_bytes: ZGROUP_BYTES,
            coding: ZCODING_BLOCK,
            store_all: false,
            raw_tails: false,
            parse: false,
            own_decode: false,
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

/// The form file of the segment file at `seg_path`, written to `out`.
///
/// The segment is opened as a merge opens its input: a mapping of its own,
/// advised for the sequential walk the build makes (the live mapping is
/// random after the first point read), and the key index's row not verified
/// again, since the open that made the segment live did. The records are
/// read through the directory, in rank order, each as the bytes
/// `parse_record` consumes for it and never as the difference of two
/// directory words: a segment closed from a direct run keeps its commit
/// markers inside the record region. A group of them is filled from the
/// previous page's count (`fill_page`), and the page it makes is run
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
        super::err("z build: the segment has no checksum row to bind a form file to")
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

/// The pages, the resident part and the trailer of the form file of the
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
    scratch.group_bytes = plan.group_bytes;
    scratch.coding = plan.coding;
    scratch.store_all = plan.store_all;
    scratch.raw_tails = plan.raw_tails;
    scratch.parse = plan.parse;
    scratch.own_decode = plan.own_decode;
    let mut verify_buf = GroupBuf::new();
    let mut fence = ZFence::new();
    let recs_at = recs.as_ptr() as usize - base;
    let dir_at = dir.as_ptr() as usize - base;
    let (mut rank, mut pages) = (0usize, 0u32);
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
    // The raw bytes ahead of the walk, asked for a window at a time: the
    // next window is asked for once the walk is within half a window of
    // the end of what was asked, so between half a window and a whole one
    // is in flight ahead of it.
    let (mut asked, mut asked_bytes) = (0usize, 0u64);
    while rank < nkeys {
        if plan.cancelled() {
            return Ok(Outcome::Cancelled);
        }
        if plan.ahead > 0 && asked < recs.len() {
            let at = rd_u32(dir, rank * 4)
                .map_or(recs.len(), |o| o as usize)
                .min(recs.len());
            if at + plan.ahead / 2 >= asked {
                let to = asked.saturating_add(plan.ahead).min(recs.len());
                ask_ahead(base, recs_at + asked, recs_at + to);
                asked_bytes += (to - asked) as u64;
                asked = to;
            }
        }
        let mut page = [0u8; ZPAGE];
        match fill_page(&images, rank, bind, &mut scratch, &mut page) {
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
                verify_page_with(&page, bind, &entry, &mut verify_buf, plan.own_decode).map_err(
                    |e| super::err(&format!("z build: a page fails the reader's checks: {e}")),
                )?;
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
        asked: asked_bytes,
    }))
}

// ---- the controller's platform side ---------------------------------------

use super::zselect::{
    parse_cgroup_v1_path, parse_cgroup_v2_path, parse_limit, parse_meminfo_total, parse_mount,
    parse_stat_v1, parse_stat_v2, parse_status_rss_anon, Budget, V1_NO_LIMIT,
};
use std::time::Duration;

/// A test's seam for the quantities the controller reads from the platform
/// (`Options::zpages_inject`), one per store and never a process-wide
/// static, since a test binary runs many stores as threads of one process:
/// the cache budget in place of the group's, the group's refault rate, and
/// the clock the tick reads, which a test advances so that a dwell of
/// minutes replays in milliseconds. A field left at its sentinel is read
/// from the platform.
#[doc(hidden)]
#[derive(Debug)]
pub struct ZInject {
    /// The budget `B`, bytes; 0 for the platform's.
    pub budget: AtomicU64,
    /// The group's refaults a second, as the bits of an `f64`; `u64::MAX`
    /// for the platform's. The census takes the interval's refaults from
    /// it too.
    pub refault_rate: AtomicU64,
    /// The clock, nanoseconds since the store's epoch; 0 for the monotonic
    /// clock.
    pub clock_ns: AtomicU64,
    /// The major faults a handle's window reads, a count the test moves,
    /// in place of the thread's own, from the seam's creation: zero until
    /// the test adds some. It began as a sentinel for "the thread's" until
    /// the first add, so a window opened before a test's first fault and
    /// closed after it took the thread's count as its baseline and the
    /// seam's as its end, and read one fault short whenever the thread had
    /// taken one real major fault before the test's first read -- the test
    /// binary's own text paged in after a large build, once in a gate run.
    pub faults: AtomicU64,
    /// The price of a fault, nanoseconds: a window's busy wall is then its
    /// CPU plus its faults at that price, so a test needs no bound to
    /// have faults cost anything; 0 for the wall as spent.
    pub fault_cost_ns: AtomicU64,
    /// The decode of a page the sampler prices the compact form by,
    /// nanoseconds, in place of the one it times; 0 for the timed one. The
    /// selector's thresholds are a function of it, and a timed decode is
    /// the build profile's: a hundred microseconds with a third of noise
    /// in the dev profile against five optimized, so a test's premise
    /// that sat on a threshold held in one profile and not the other.
    pub decode_cost_ns: AtomicU64,
}

impl Default for ZInject {
    fn default() -> ZInject {
        ZInject {
            budget: AtomicU64::new(0),
            refault_rate: AtomicU64::new(u64::MAX),
            clock_ns: AtomicU64::new(0),
            faults: AtomicU64::new(0),
            fault_cost_ns: AtomicU64::new(0),
            decode_cost_ns: AtomicU64::new(0),
        }
    }
}

impl ZInject {
    pub fn new() -> ZInject {
        ZInject::default()
    }

    /// The budget the tick reads, in place of the platform's.
    pub fn set_budget(&self, bytes: u64) {
        self.budget.store(bytes, Ordering::Release);
    }

    /// The refault rate the tick reads, in place of the platform's.
    pub fn set_refault_rate(&self, per_s: f64) {
        self.refault_rate.store(per_s.to_bits(), Ordering::Release);
    }

    /// The injected clock moved on by `d`; from the first call the tick
    /// reads this clock and not the monotonic one.
    pub fn advance(&self, d: Duration) {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1);
        self.clock_ns.fetch_add(ns, Ordering::AcqRel);
    }

    /// The injected budget, if one is set.
    pub fn budget(&self) -> Option<u64> {
        match self.budget.load(Ordering::Acquire) {
            0 => None,
            b => Some(b),
        }
    }

    /// The injected refault rate, if one is set.
    pub fn refault_rate(&self) -> Option<f64> {
        match self.refault_rate.load(Ordering::Acquire) {
            u64::MAX => None,
            bits => Some(f64::from_bits(bits)),
        }
    }

    /// The injected clock, if a test has moved it.
    pub fn clock(&self) -> Option<Duration> {
        match self.clock_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// `n` more faults for the windows to read.
    pub fn add_faults(&self, n: u64) {
        let was = self.faults.load(Ordering::Acquire);
        self.faults.store(was.saturating_add(n), Ordering::Release);
    }

    /// The price of a fault the windows charge, in place of the wall.
    pub fn set_fault_cost(&self, d: Duration) {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1);
        self.fault_cost_ns.store(ns, Ordering::Release);
    }

    /// The injected fault count: the seam's from its creation, so that a
    /// window's two readings are of one counter.
    pub fn faults(&self) -> Option<u64> {
        Some(self.faults.load(Ordering::Acquire))
    }

    /// The injected price of a fault, if one is set.
    pub fn fault_cost(&self) -> Option<Duration> {
        match self.fault_cost_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// The decode of a page the sampler prices the compact form by, in
    /// place of the one it times; and the decode the re-code policy prices
    /// a form file's decodes at, in place of the time they took
    /// (`Options::zpages_recode`).
    pub fn set_decode_cost(&self, d: Duration) {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1);
        self.decode_cost_ns.store(ns, Ordering::Release);
    }

    /// The injected decode of a page, if one is set.
    pub fn decode_cost(&self) -> Option<Duration> {
        match self.decode_cost_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }
}

/// The calling thread's major faults, cumulative, as the kernel counts
/// them for the thread (`getrusage(RUSAGE_THREAD)`): the suite's own
/// measure (`/proc/thread-self/stat`'s field), read through the call
/// that costs a syscall and no file. `None` off Linux and where the call
/// fails. `RUSAGE_THREAD` is Linux's 1, named here because the libc
/// crate this tree builds with does not name it for this target.
pub(super) fn thread_major_faults() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        const RUSAGE_THREAD: libc::c_int = 1;
        // SAFETY: a zeroed rusage is a valid out-buffer for the call's one
        // write.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(RUSAGE_THREAD, &mut ru) } == 0 {
            return Some(ru.ru_majflt as u64);
        }
    }
    None
}

/// What a sample of a partition says its form would be (design 2.6.1):
/// [`SAMPLE_GROUPS`] pages formed at evenly spaced ranks with the real
/// `fill_page`, each fill and each page's decode-plus-walk of every group
/// timed on the thread's own clock, and the major faults the fills' touches
/// took, with the wall they waited, as the first measured price of a fault
/// the selector has.
/// Taken over the live mapping: the fills read the records the groups are
/// made of, which is what a build reads of them.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Sample {
    /// The partition's keys and its record bytes.
    pub nkeys: u64,
    pub recs_len: u64,
    /// Pages counted -- the last of each rank's chain of [`SAMPLE_CHAIN`]
    /// fills -- the records in them, and the groups in them, which a point
    /// read decodes one of.
    pub groups: u64,
    pub records: u64,
    pub zgroups: u64,
    /// The pages the form is estimated to take, the keys over the counted
    /// groups' mean count, and the bytes of the form: the pages, a fence
    /// entry a page and the filter.
    pub pages_est: u64,
    pub w_z: u64,
    /// The form's resident part, estimated: the fence entries and the
    /// filter.
    pub resident_est: u64,
    /// The fills' thread CPU, nanoseconds, over `records`: the build's CPU
    /// a record.
    pub fill_ns: u64,
    /// The decodes' and walks' thread CPU over the `zgroups` groups of the
    /// `groups` pages: what a read through the form pays a group.
    pub decode_ns: u64,
    /// The sample's wall less its CPU, nanoseconds, and the major faults it
    /// took: the price of a fault, where any were taken.
    pub wait_ns: u64,
    pub faults: u64,
    /// Declined: the floor, a block-backed record, or no group formed.
    pub declined: Option<Decline>,
}

/// Ranks a sample fills from, evenly spaced.
pub(super) const SAMPLE_GROUPS: usize = 32;
/// Fills a sample chains from each rank, each hinted by the one before, the
/// last counted: a fill with no predecessor builds a page well short of
/// full (`fill_page`), the records' uncompressed count where a chained
/// page holds what its group compresses to, and the build's pages are one
/// chain from the partition's first record. Counting the unchained group
/// estimated the form at 1.7x its file -- 626 MB for a file of 360 at five
/// million keys of the suite's values -- and every rate the model drew
/// from it. Three: the second fill grows by the slack the first's bytes
/// leave and the third fits what the second compressed to.
pub(super) const SAMPLE_CHAIN: usize = 3;

/// `blob`'s sample, see [`Sample`]; `floor` is the record bytes under which
/// the partition is declined (`Options::zpages_min_bytes`).
/// The fill's shape a sample and a build take from the options: the
/// decoded bytes a group is cut at, the coding of the groups that compress,
/// whether every group is stored verbatim or every tail raw, and which
/// encoder writes the block coding's groups (`Options::zpages_group_bytes`,
/// `z_coding`, `zpages_store_all`, `zpages_raw_tails`, `zpages_parse`).
#[derive(Clone, Copy, Debug)]
pub(super) struct FillShape {
    pub group_bytes: usize,
    pub coding: u8,
    pub store_all: bool,
    pub raw_tails: bool,
    pub parse: bool,
    pub own_decode: bool,
}

impl Default for FillShape {
    fn default() -> FillShape {
        FillShape {
            group_bytes: ZGROUP_BYTES,
            coding: ZCODING_BLOCK,
            store_all: false,
            raw_tails: false,
            parse: false,
            own_decode: false,
        }
    }
}

impl FillShape {
    /// The scratch's shape set from this one.
    fn apply(&self, scratch: &mut FillScratch) {
        scratch.group_bytes = self.group_bytes;
        scratch.coding = self.coding;
        scratch.store_all = self.store_all;
        scratch.raw_tails = self.raw_tails;
        scratch.parse = self.parse;
        scratch.own_decode = self.own_decode;
    }
}

pub(super) fn sample(
    blob: &Blob<MmapBytes>,
    id: u64,
    end_seq: u64,
    floor: usize,
    shape: FillShape,
) -> Result<Sample> {
    let facts = blob.z_source_facts().ok_or_else(|| {
        super::err("z sample: the segment has no checksum row to bind a form file to")
    })?;
    let (recs, dir) = blob
        .record_regions()
        .ok_or_else(|| super::err("z sample: the segment's records are not resident"))?;
    let recs_len = usize::try_from(facts.recs_len).unwrap_or(usize::MAX);
    let mut s = Sample {
        nkeys: facts.nkeys,
        recs_len: facts.recs_len,
        ..Sample::default()
    };
    if recs_len < floor {
        s.declined = Some(Decline::Small(recs_len));
        return Ok(s);
    }
    if blob.blocks() > 0 {
        s.declined = Some(Decline::BlockBacked);
        return Ok(s);
    }
    let nkeys = blob.keys();
    let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
    let images = SegImages::new(recs, dir, nkeys);
    let mut scratch = FillScratch::new();
    shape.apply(&mut scratch);
    let mut buf = GroupBuf::new();
    let mut page = [0u8; ZPAGE];
    let flt0 = thread_major_faults().unwrap_or(0);
    let cpu0 = super::thread_cpu_ns();
    let wall0 = std::time::Instant::now();
    let mut last_nopage: Option<NoPage> = None;
    let mut key_bytes = 0u64;
    for i in 0..SAMPLE_GROUPS {
        let mut rank = i * nkeys / SAMPLE_GROUPS;
        if rank >= nkeys {
            break;
        }
        // The chain from this rank, each fill after the one before (the
        // one-group shape takes the count before as its hint) and only the
        // last counted and timed (`SAMPLE_CHAIN`); a chain cut short by the
        // partition's end or a record no page holds counts nothing for
        // this rank.
        for step in 1..=SAMPLE_CHAIN {
            let c0 = super::thread_cpu_ns();
            match fill_page(&images, rank, bind, &mut scratch, &mut page) {
                Fill::Page(g) => {
                    if step < SAMPLE_CHAIN {
                        rank += g.count;
                        continue;
                    }
                    let c1 = super::thread_cpu_ns();
                    s.fill_ns = s.fill_ns.saturating_add(c1.saturating_sub(c0));
                    let view = parse_page(&page, bind).map_err(|e| {
                        super::err(&format!("z sample: a page fails its own parse: {e}"))
                    })?;
                    for j in 0..view.groups() {
                        let group = decode_group_with(&view, j, &mut buf, shape.own_decode)
                            .map_err(|e| {
                                super::err(&format!("z sample: a page fails its own decode: {e}"))
                            })?;
                        let count = view.group(j).map_or(0, |g| usize::from(g.count));
                        walk_group(group, count, None, None).map_err(|e| {
                            super::err(&format!("z sample: a group fails its own walk: {e}"))
                        })?;
                    }
                    s.decode_ns = s
                        .decode_ns
                        .saturating_add(super::thread_cpu_ns().saturating_sub(c1));
                    s.groups += 1;
                    s.zgroups += view.groups() as u64;
                    s.records += g.count as u64;
                    key_bytes += record_key(images.image(rank), 0).map_or(0, |k| k.len() as u64);
                }
                Fill::NoPage(np) => {
                    s.fill_ns = s
                        .fill_ns
                        .saturating_add(super::thread_cpu_ns().saturating_sub(c0));
                    last_nopage = Some(np);
                    break;
                }
                Fill::End => break,
                Fill::Mismatch => {
                    return Err(super::err("z sample: a page fails its own decode"));
                }
            }
        }
    }
    s.faults = thread_major_faults().unwrap_or(0).saturating_sub(flt0);
    let cpu = super::thread_cpu_ns().saturating_sub(cpu0);
    s.wait_ns = (wall0.elapsed().as_nanos().min(u64::MAX as u128) as u64).saturating_sub(cpu);
    if s.groups == 0 {
        s.declined = Some(last_nopage.map_or(Decline::Small(recs_len), Decline::NoPage));
        return Ok(s);
    }
    let mean_count = s.records as f64 / s.groups as f64;
    s.pages_est = (nkeys as f64 / mean_count).ceil().max(1.0) as u64;
    let mean_key = key_bytes / s.groups;
    s.resident_est = s
        .pages_est
        .saturating_mul(8 + mean_key)
        .saturating_add(filter_bytes(nkeys) as u64);
    s.w_z = s
        .pages_est
        .saturating_mul(ZPAGE as u64)
        .saturating_add(s.resident_est);
    Ok(s)
}

/// The platform's reading: the v1 memory controller's group where the
/// process has one (this host is a hybrid whose v1 line wins), else the
/// unified hierarchy's group with the least of `memory.max` and
/// `memory.high` over the path to its mount, else the machine's total and
/// the process's own anonymous memory; and `None` where even `/proc/meminfo`
/// cannot be read. Off Linux there is no reading.
#[cfg(target_os = "linux")]
pub fn read_budget() -> Option<Budget> {
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    let total = parse_meminfo_total(&read(Path::new("/proc/meminfo"))?)?;
    let cg = read(Path::new("/proc/self/cgroup")).unwrap_or_default();
    let mounts = read(Path::new("/proc/mounts")).unwrap_or_default();
    if let Some(path) = parse_cgroup_v1_path(&cg) {
        if let Some(mount) = parse_mount(&mounts, "cgroup") {
            let dir = Path::new(&mount).join(path.trim_start_matches('/'));
            if let Some(st) = read(&dir.join("memory.stat")).map(|t| parse_stat_v1(&t)) {
                let limit = st
                    .limit
                    .filter(|l| *l < V1_NO_LIMIT)
                    .unwrap_or(total)
                    .min(total);
                return Some(Budget::of(
                    limit,
                    st.rss.saturating_add(st.rss_huge),
                    st.refaults,
                ));
            }
        }
    }
    if let Some(path) = parse_cgroup_v2_path(&cg) {
        if let Some(mount) = parse_mount(&mounts, "cgroup2") {
            let mount = Path::new(&mount);
            let leaf = mount.join(path.trim_start_matches('/'));
            if let Some(st) = read(&leaf.join("memory.stat")).map(|t| parse_stat_v2(&t)) {
                let mut limit = total;
                let mut dir = leaf.as_path();
                loop {
                    for f in ["memory.max", "memory.high"] {
                        if let Some(l) = read(&dir.join(f)).and_then(|t| parse_limit(&t)) {
                            limit = limit.min(l);
                        }
                    }
                    if dir == mount {
                        break;
                    }
                    match dir.parent() {
                        Some(p) if p.starts_with(mount) => dir = p,
                        _ => break,
                    }
                }
                return Some(Budget::of(limit, st.anon, st.refaults));
            }
        }
    }
    let anon = read(Path::new("/proc/self/status"))
        .and_then(|t| parse_status_rss_anon(&t))
        .unwrap_or(0);
    Some(Budget::of(total, anon, None))
}

#[cfg(not(target_os = "linux"))]
pub fn read_budget() -> Option<Budget> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::zpages::ZCODING_RECORD;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_platform_gives_a_reading_on_this_host() {
        let b = read_budget().expect("linux reads /proc/meminfo at least");
        assert!(b.limit > 0 && b.budget < b.limit, "{b:?}");
        assert!(b.anon < b.limit, "{b:?}");
    }

    #[test]
    fn the_injection_seam_reads_as_the_platform_until_set() {
        let i = ZInject::new();
        assert_eq!(
            (i.budget(), i.refault_rate(), i.clock()),
            (None, None, None)
        );
        i.set_budget(5);
        i.set_refault_rate(0.0);
        i.advance(Duration::from_millis(1500));
        assert_eq!(i.budget(), Some(5));
        assert_eq!(i.refault_rate(), Some(0.0));
        assert_eq!(i.clock(), Some(Duration::from_millis(1500)));
        i.advance(Duration::from_secs(1));
        assert_eq!(i.clock(), Some(Duration::from_millis(2500)));
        // The fault count is the seam's from its creation, zero, so that
        // a window opened before the first add and closed after it reads
        // two values of one counter.
        assert_eq!(
            (i.faults(), i.fault_cost(), i.decode_cost()),
            (Some(0), None, None)
        );
        i.add_faults(3);
        i.add_faults(4);
        i.set_fault_cost(Duration::from_micros(40));
        i.set_decode_cost(Duration::from_micros(5));
        assert_eq!(i.faults(), Some(7));
        assert_eq!(i.fault_cost(), Some(Duration::from_micros(40)));
        assert_eq!(i.decode_cost(), Some(Duration::from_micros(5)));
    }

    /// A sample of a partition: thirty-two ranks, at each a chain of fills
    /// hinted by the one before with the last counted and timed, the pages
    /// estimated from the counted groups' records within a tenth of the
    /// pages the build writes and the form's bytes within a tenth of the
    /// file's, and the declines the builder makes made here first. A fill
    /// with no predecessor builds a page well short of full, and a sample
    /// that counted one such group a rank estimated the records'
    /// uncompressed count a page: 1.7x the file at five million keys, and
    /// every rate the model drew from it.
    #[test]
    fn a_sample_estimates_the_form_within_a_tenth_of_the_build() {
        let fx = Fixture::new("sample", suite(20_000, 0x5A4), 1 << 16);
        let s = sample(&fx.blob, ID, END, 0, FillShape::default()).unwrap();
        assert_eq!(s.declined, None, "{s:?}");
        assert_eq!(s.nkeys, 20_000);
        assert_eq!(s.groups, SAMPLE_GROUPS as u64, "{s:?}");
        assert!(s.records >= s.groups, "{s:?}");
        let mean = s.records as f64 / s.groups as f64;
        assert_eq!(s.pages_est, (20_000.0 / mean).ceil() as u64, "{s:?}");
        assert!(s.w_z < s.recs_len, "the form under the records: {s:?}");
        assert_eq!(s.w_z, s.pages_est * ZPAGE as u64 + s.resident_est, "{s:?}");
        assert!(s.fill_ns > 0 && s.decode_ns > 0, "timed: {s:?}");
        let Outcome::Built(b) = fx.build() else {
            panic!("the fixture builds")
        };
        let pages = s.pages_est as f64 / b.fill.pages as f64;
        assert!(
            (pages - 1.0).abs() < 0.1,
            "{} pages estimated, {} built: {s:?}",
            s.pages_est,
            b.fill.pages
        );
        let bytes = s.w_z as f64 / b.file_bytes as f64;
        assert!(
            (bytes - 1.0).abs() < 0.1,
            "{} bytes estimated, the file {}: {s:?}",
            s.w_z,
            b.file_bytes
        );
        // The builder's own declines, made first: the floor.
        let d = sample(&fx.blob, ID, END, usize::MAX, FillShape::default()).unwrap();
        assert!(matches!(d.declined, Some(Decline::Small(_))), "{d:?}");
        assert_eq!((d.groups, d.w_z), (0, 0), "{d:?}");
    }

    /// The thread's major faults are read, and are the count the suite
    /// reads from `/proc/thread-self/stat` (its twelfth field after the
    /// command), so the engine and the row agree on what a fault is.
    #[test]
    fn the_threads_major_faults_are_the_procfs_count() {
        let Some(a) = thread_major_faults() else {
            if cfg!(target_os = "linux") {
                panic!("Linux reads them");
            }
            return;
        };
        let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
        let (_, rest) = stat.rsplit_once(')').unwrap();
        let proc_majflt: u64 = rest.split_whitespace().nth(9).unwrap().parse().unwrap();
        let b = thread_major_faults().unwrap();
        assert!(
            a <= proc_majflt && proc_majflt <= b,
            "{a} {proc_majflt} {b}"
        );
    }
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
            std::env::temp_dir().join(format!("supdb-next-zfile-{name}-{}", std::process::id()));
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
        /// The handle's memo the tests read through.
        memo: ZMemo,
        /// The group coding the fixture builds its form file with: the
        /// block coding unless a test asks for the other, and the record
        /// coding's tails stored raw where a test asks for that.
        coding: u8,
        raw_tails: bool,
    }

    impl Fixture {
        fn new(name: &str, recs: Vec<Rec>, inline_max: usize) -> Fixture {
            Fixture::with_coding(name, recs, inline_max, ZCODING_BLOCK)
        }

        fn with_coding(name: &str, recs: Vec<Rec>, inline_max: usize, coding: u8) -> Fixture {
            Fixture::with_shape(name, recs, inline_max, coding, false)
        }

        fn with_shape(
            name: &str,
            recs: Vec<Rec>,
            inline_max: usize,
            coding: u8,
            raw_tails: bool,
        ) -> Fixture {
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
                memo: ZMemo::new(),
                coding,
                raw_tails,
            }
        }

        fn build(&self) -> Outcome {
            let plan = BuildPlan {
                coding: self.coding,
                raw_tails: self.raw_tails,
                ..BuildPlan::new(0, true)
            };
            build(&self.seg, &self.out, ID, END, &plan).unwrap()
        }

        fn attach(&self) -> std::result::Result<ZFile, Reject> {
            ZFile::attach(
                &self.out, &self.blob, ID, END, false, true, false, true, false, false, true,
            )
        }

        /// Attached with the advice, the memo and the scan's prefetch as given.
        fn attach_opts(
            &self,
            advise: bool,
            memo: bool,
            prefetch: bool,
        ) -> std::result::Result<ZFile, Reject> {
            ZFile::attach(
                &self.out, &self.blob, ID, END, advise, memo, prefetch, true, false, false, true,
            )
        }

        /// Attached with the fence's tree and the memo's victim hint as
        /// given (`Options::zpages_fence_tree`, `Options::zpages_memo_ahead`).
        fn attach_lines(
            &self,
            fence_tree: bool,
            memo_ahead: bool,
        ) -> std::result::Result<ZFile, Reject> {
            ZFile::attach(
                &self.out, &self.blob, ID, END, false, true, false, true, false, fence_tree,
                memo_ahead,
            )
        }

        /// Attached with the memo off: for a test of the decode path,
        /// whose second read of a page must decode it as the first did --
        /// a page changed under the mapping, a page a buffer cannot lend,
        /// a stored page beside a compressed one. With the memo on, a
        /// page's group is served as it was decoded while it holds.
        fn attach_nomemo(&self) -> std::result::Result<ZFile, Reject> {
            ZFile::attach(
                &self.out, &self.blob, ID, END, false, false, false, true, false, false, true,
            )
        }

        /// A form file built and attached.
        fn zfile(&self) -> ZFile {
            assert!(matches!(self.build(), Outcome::Built(_)));
            self.attach().expect("the form file it just built attaches")
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

    /// What the form file answers for `key`, or `None` where it declines.
    fn z_values(
        memo: &ZMemo,
        zfile: &ZFile,
        blob: &Blob<MmapBytes>,
        key: &[u8],
    ) -> Option<Vec<Vec<u8>>> {
        let mut v = Vec::new();
        match zfile
            .read_all(memo, blob, key, &mut |x: &[u8]| v.push(x.to_vec()))
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
    fn agree(memo: &ZMemo, zfile: &ZFile, blob: &Blob<MmapBytes>, recs: &[Rec]) {
        for key in probes(recs) {
            let want = raw_values(blob, &key);
            assert_eq!(
                z_values(memo, zfile, blob, &key).as_ref(),
                Some(&want),
                "read of {key:?}"
            );
            assert_eq!(
                served(zfile.count(memo, &key)),
                blob.count(&key).unwrap(),
                "count of {key:?}"
            );
        }
        assert!(!zfile.damaged());
    }

    #[test]
    fn a_form_file_answers_every_key_as_the_segment_does() {
        for (name, recs, inline, coding) in [
            ("suite", suite(3000, 11), 256, ZCODING_BLOCK),
            ("shapes", shapes(), 1 << 20, ZCODING_BLOCK),
            ("suite-rec", suite(3000, 11), 256, ZCODING_RECORD),
            ("shapes-rec", shapes(), 1 << 20, ZCODING_RECORD),
        ] {
            let fx = Fixture::with_coding(name, recs, inline, coding);
            let zfile = fx.zfile();
            assert!(zfile.pages() > 1, "{name}: more than one page");
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            // Every page a read opened was checked, once, and none failed.
            assert!(zfile.verified_pages() > 0 && zfile.bad_pages() == 0);
            // The second pass finds every page verified and answers the same:
            // under the record coding, through the one-record point read.
            let before = zfile.verified_pages();
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            assert_eq!(zfile.verified_pages(), before, "{name}");
            assert_eq!(zfile.bad_pages(), 0, "{name}");
        }
    }

    /// The fence's tree and the memo's victim hint
    /// (`Options::zpages_fence_tree`, `Options::zpages_memo_ahead`), in
    /// every combination, answer every key as the segment does, twice over,
    /// with the fence indexed only where the tree is asked for.
    #[test]
    fn the_fences_tree_and_the_memos_hint_answer_as_the_segment_does_in_every_combination() {
        let fx = Fixture::new("lines", suite(3000, 17), 256);
        assert!(matches!(fx.build(), Outcome::Built(_)));
        for (tree, ahead) in [(false, false), (true, false), (false, true), (true, true)] {
            let zfile = fx.attach_lines(tree, ahead).unwrap();
            assert_eq!(zfile.fence.indexed(), tree, "tree {tree}");
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            assert_eq!(zfile.bad_pages(), 0, "tree {tree} ahead {ahead}");
        }
    }

    /// A cursor moved to a key (`ZCursor::skip_to`) cuts a batch of keys
    /// in key order where a seek a key (`ZFile::cut_in`) cuts each: every
    /// record's key, keys between records, below the first and past the
    /// last, over the suite's shape and over every record shape; it opens
    /// each compressed group once over the batch, where the seeks open one
    /// a key; and keys out of order are answered as the seek from the top
    /// answers them, the group opened again.
    #[test]
    fn a_cursor_moved_to_a_key_agrees_with_the_seek_a_key() {
        for (name, recs, inline_max, coding, raw_tails) in [
            ("skipto-suite", suite(3000, 41), 256, ZCODING_BLOCK, false),
            ("skipto-shapes", shapes(), 1 << 20, ZCODING_BLOCK, false),
            (
                "skipto-suite-rec",
                suite(3000, 41),
                256,
                ZCODING_RECORD,
                false,
            ),
            (
                "skipto-shapes-rec",
                shapes(),
                1 << 20,
                ZCODING_RECORD,
                false,
            ),
            (
                "skipto-suite-raw",
                suite(3000, 41),
                256,
                ZCODING_RECORD,
                true,
            ),
            ("skipto-shapes-raw", shapes(), 1 << 20, ZCODING_RECORD, true),
        ] {
            let fx = Fixture::with_shape(name, recs, inline_max, coding, raw_tails);
            let zfile = fx.zfile();
            let nkeys = fx.recs.len();
            let mut probes: Vec<Vec<u8>> = vec![Vec::new(), b"\x00".to_vec()];
            for (k, _, _) in &fx.recs {
                probes.push(k.clone());
                let mut above = k.clone();
                above.push(0);
                probes.push(above);
                if let Some(&last) = k.last() {
                    let mut below = k.clone();
                    *below.last_mut().unwrap() = last.wrapping_sub(1);
                    probes.push(below);
                }
            }
            probes.push(vec![0xff; 20]);
            probes.sort();
            probes.dedup();
            // The seeks, each from the rank the one before reached, as a
            // block's cuts were taken.
            let mut want = Vec::with_capacity(probes.len());
            let mut r = 0usize;
            let mut seek_decoded = false;
            for k in &probes {
                match zfile.cut_in(&fx.memo, r, nkeys, k) {
                    ZSeek::Served { rank, at, decoded } => {
                        want.push((rank, at));
                        seek_decoded |= decoded;
                        r = rank;
                    }
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!(
                want.last().map(|w| w.0),
                Some(nkeys),
                "the last probe is past every key"
            );
            // A record-coded group is sought over its heads: the suite's
            // shapes under that coding, every group coded, decode nothing
            // for a seek and ask the memo for nothing; the block coding
            // decodes the group the key falls in.
            let seek_memo = fx.memo.take_counts();
            if name.starts_with("skipto-suite") && coding == ZCODING_RECORD {
                assert!(
                    !seek_decoded && seek_memo == (0, 0),
                    "{name}: the seeks over the heads decoded nothing: memo {seek_memo:?}"
                );
            } else {
                assert!(
                    seek_decoded,
                    "{name}: the seeks decoded the groups they fell in"
                );
            }
            let mut cur = ZCursor::unplaced(&fx.memo, &zfile, &fx.blob);
            let got: Vec<(usize, KeyOrder)> = probes
                .iter()
                .map(|k| {
                    let at = cur.skip_to(k).expect("the form file answers");
                    (cur.rank().min(nkeys), at)
                })
                .collect();
            assert!(!cur.fell_back());
            let (decoded, inplace) = (cur.decoded, cur.inplace);
            drop(cur);
            assert_eq!(got, want, "{name}");
            let (hits, misses) = fx.memo.take_counts();
            assert_eq!(misses, decoded, "{name}: a miss a decode");
            // A stored group is parsed where it lies and asks the memo
            // nothing, and so is a raw-tail coded group (`Group::Coded`),
            // which the random shapes leave where LZ4 shrank no tail; every
            // other group is asked for once.
            let compressed: usize = (0..zfile.pages())
                .map(|p| {
                    let v = page_view(zfile.page_bytes(p).unwrap()).unwrap();
                    (0..v.groups())
                        .filter(|&j| v.group(j).is_some_and(|g| !g.stored))
                        .count()
                })
                .sum();
            assert_eq!(
                (hits + misses) as usize + inplace as usize,
                compressed,
                "{name}: one open a compressed group, or a read in place"
            );
            assert_eq!(
                inplace > 0,
                raw_tails || name == "skipto-shapes-rec",
                "{name}: the raw tails, and the shapes' uncompressible ones, are read in place"
            );
            // Out of key order: every key answered as the seek from the top
            // answers it, the group opened again where the cursor was past.
            let mut cur = ZCursor::unplaced(&fx.memo, &zfile, &fx.blob);
            for k in probes.iter().rev() {
                let want = match zfile.cut_in(&fx.memo, 0, nkeys, k) {
                    ZSeek::Served { rank, at, .. } => (rank, at),
                    other => panic!("{other:?}"),
                };
                let at = cur.skip_to(k).expect("the form file answers");
                assert_eq!(
                    (cur.rank().min(nkeys), at),
                    want,
                    "{name}: key {k:?} out of order"
                );
            }
            assert!(!cur.fell_back());
        }
    }

    /// A block reader's walk whose limit falls at a group's last record
    /// (`walk_run`) opens the next group for nobody: the memo is asked for
    /// the group the walk opened and no other until the next read from the
    /// position -- a run, a step or a key sought -- which opens it then and
    /// stands where the walk left off; the overlay walk's `walk_until`
    /// stands on the next group's first record at once, its key readable.
    #[test]
    fn a_walk_that_ends_at_a_groups_last_record_leaves_the_next_group_to_the_next_read() {
        let fx = Fixture::new("walk-group-end", suite(3000, 41), 256);
        let zfile = fx.zfile();
        let v = page_view(zfile.page_bytes(0).unwrap()).unwrap();
        assert!(v.groups() > 1, "a page of several groups");
        let count = v.group(0).unwrap().count as usize;
        fx.memo.take_counts();
        let mut cur = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
        let mut seen = Vec::new();
        let (n, at) = cur
            .walk_run(None, count, &mut |k: &[u8], _: &[u8]| seen.push(k.to_vec()))
            .unwrap();
        assert_eq!((n, at), (count, KeyOrder::Greater));
        assert_eq!(seen.len(), count);
        assert_eq!(
            cur.rank(),
            count,
            "the position is the next group's first record"
        );
        let (hits, misses) = fx.memo.take_counts();
        assert_eq!(hits + misses, 1, "the one group the walk opened");
        // The next read opens it and goes on from there.
        let (n, _) = cur
            .walk_run(None, 1, &mut |k: &[u8], _: &[u8]| seen.push(k.to_vec()))
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            seen.last().map(|k| k.as_slice()),
            Some(fx.recs[count].0.as_slice())
        );
        let (hits, misses) = fx.memo.take_counts();
        assert_eq!(hits + misses, 1, "the next group, opened once");
        // A step from a spent group is a step into the next.
        let mut cur = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
        cur.walk_run(None, count, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        cur.advance();
        assert_eq!(cur.rank(), count + 1);
        assert_eq!(cur.key(), Some(fx.recs[count + 1].0.as_slice()));
        // And a key sought from one is sought from the next group's first.
        let mut cur = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
        cur.walk_run(None, count, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(cur.skip_to(&fx.recs[count].0), Some(KeyOrder::Equal));
        assert_eq!(cur.rank(), count);
        // The overlay walk's bulk walk opens the next group at once and
        // stands on its first record.
        fx.memo.take_counts();
        let mut cur = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
        cur.walk_until(None, count, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(cur.rank(), count);
        assert_eq!(cur.key(), Some(fx.recs[count].0.as_slice()));
        let (hits, misses) = fx.memo.take_counts();
        assert_eq!(
            hits + misses,
            2,
            "the group walked and the next, opened at once"
        );
    }

    /// A cursor moved onto a page that fails its first decode answers
    /// `None` and has fallen, with the form file out of service, and the
    /// keys before the page were answered: the caller takes the rest from
    /// the raw form. A byte of a page the attach does not sample is
    /// flipped, so the form file attaches and the page fails at its first
    /// decode.
    #[test]
    fn a_cursor_moved_onto_a_damaged_page_falls_back() {
        let fx = Fixture::new("skipto-damaged", suite(4000, 12), 256);
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
        let zfile = fx
            .attach()
            .expect("a damaged page the sample misses attaches");
        let nkeys = fx.recs.len();
        let first_on_p = (0..zfile.fence.len())
            .find_map(|e| {
                zfile
                    .fence
                    .entry(e, nkeys as u32)
                    .filter(|en| en.page as usize == p)
                    .map(|en| en.first_rank as usize)
            })
            .expect("a page the fence names");
        assert!(first_on_p > 0 && first_on_p < nkeys);
        let mut cur = ZCursor::unplaced(&fx.memo, &zfile, &fx.blob);
        for (i, (k, _, _)) in fx.recs[..first_on_p].iter().enumerate() {
            assert_eq!(cur.skip_to(k), Some(KeyOrder::Equal), "key {i}");
            assert_eq!(cur.rank(), i);
        }
        assert!(!cur.fell_back() && !zfile.damaged());
        let k = &fx.recs[first_on_p].0;
        assert_eq!(cur.skip_to(k), None, "a key on the damaged page");
        assert!(cur.fell_back() && zfile.damaged());
        assert_eq!(cur.skip_to(k), None, "fallen for good");
    }

    #[test]
    fn a_read_hands_over_nothing_before_its_checks_have_passed() {
        // The callback is called only once every check that can fail has
        // passed: a read the form file declines is read from the raw form by
        // the caller, and a value already handed over would be handed over
        // twice. A byte of a page the attach does not sample is flipped, so
        // the form file attaches and the page fails at its first decode.
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
        let zfile = fx
            .attach()
            .expect("a damaged page the sample misses attaches");
        let (mut served, mut declined) = (0usize, 0usize);
        for (k, vals, _) in &fx.recs {
            let mut got = 0usize;
            match zfile
                .read_all(&fx.memo, &fx.blob, k, &mut |_: &[u8]| got += 1)
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
        assert!(zfile.damaged() && zfile.bad_pages() == 1);
    }

    #[test]
    fn a_partition_that_cannot_have_a_form_file_is_declined_for_its_reason() {
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
            let zfile = fx.attach().unwrap();
            assert_eq!(zfile.count(&fx.memo, b"zzz-big"), ZRead::NoPage, "{name}");
            assert_eq!(
                z_values(&fx.memo, &zfile, &fx.blob, b"zzz-big"),
                None,
                "{name}: the read is the raw form's"
            );
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs[..300]);
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
        assert!(b.fill.stored > 0, "stored groups");
        let zfile = fx.attach_nomemo().unwrap();
        agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
        // A stored page is parsed where it lies: nothing is decoded. (The
        // memo off, so that the compressed page beside it decodes again.)
        let key = &fx.recs[fx.recs.len() / 2].0;
        let mut seen = 0;
        let r = zfile
            .read_all(&fx.memo, &fx.blob, key, &mut |_: &[u8]| seen += 1)
            .unwrap();
        assert_eq!(seen, 1);
        let ZRead::Served { decoded, .. } = r else {
            panic!("served")
        };
        let stored_group = {
            let e = zfile.fence.entry_of(key).unwrap();
            let p = zfile.fence.page(e).unwrap() as usize;
            let v = page_view(zfile.page_bytes(p).unwrap()).unwrap();
            v.group(v.group_of_key(key)).unwrap().stored
        };
        assert_eq!(decoded, !stored_group);
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
        // One group, so that the group opened is the one the page holds.
        s.group_bytes = 0;
        let mut page = [0u8; ZPAGE];
        let Fill::Page(g) = fill_page(&recs, 0, bind, &mut s, &mut page) else {
            panic!("a page")
        };
        assert_eq!((g.groups, g.stored), (1, 1), "random bytes are stored");
        let h = parse_page(&page, bind).unwrap().header;
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
        let (group, _, count, decoded) = first_decode(p, bind, &entry, 0, &mut buf, false).unwrap();
        assert!(!decoded && count == usize::from(h.count));
        assert!(group_seek(group, count, &key0).unwrap().found);
        let (group, _, count, decoded) = verified_group(p, 0, &mut buf).unwrap();
        assert!(!decoded && count == usize::from(h.count));
        assert!(group_seek(group, count, &key0).unwrap().found);

        // Misaligned by every skew that is not a multiple of four: refused,
        // at the first decode and after.
        for skew in [1usize, 2, 3, 5, 6, 7] {
            let (backing, at) = page_at(&page, skew);
            let p = &backing[at..at + ZPAGE];
            assert_ne!((p.as_ptr() as usize + ZHDR) % 4, 0, "skew {skew}");
            assert_eq!(
                first_decode(p, bind, &entry, 0, &mut buf, false).unwrap_err(),
                Fail::Misaligned,
                "skew {skew}"
            );
            assert_eq!(
                verified_group(p, 0, &mut buf).unwrap_err(),
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
        let zfile = fx.zfile();
        let (a, b) = (&fx.recs[10], &fx.recs[2500]);
        // The memo one slot wide: the inner read's put displaces the outer
        // group's entry, which the outer read holds through its `Arc`.
        let small = ZMemo::with(1, ZMEMO_BYTES);
        let mut seen = false;
        let r = zfile
            .read_all(&small, &fx.blob, &a.0, &mut |outer: &[u8]| {
                assert_eq!(outer, &a.1[0][..]);
                let inner = z_values(&small, &zfile, &fx.blob, &b.0).unwrap();
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

    /// The memo: a direct-mapped table under a byte cap. An entry put is
    /// found until displaced, another form file's same page is not it, a put
    /// that passes the cap displaces others until the bytes held fit with
    /// the entry just put kept, and the hits and misses count.
    #[test]
    fn the_memo_holds_its_cap_and_counts_its_hits() {
        let g = |n: usize| (Arc::new(GroupBuf::new()), n);
        let mut m = Memo::new(8, 100);
        assert!(m.get(1, 0, 0, true).is_none());
        let (b, n) = g(40);
        m.put(1, 0, 0, true, b, n, 0, 1);
        assert_eq!(m.get(1, 0, 0, true).map(|(_, n, _, _)| n), Some(40));
        assert!(m.get(2, 0, 0, true).is_none(), "another form file's page 0");
        // The same group of another form file in the same slot, found over
        // the identities: only the file compare tells them apart, and this
        // is the one way to reach it.
        let other = (2..)
            .find(|&z| m.slot(z, 0, 0, true) == m.slot(1, 0, 0, true))
            .unwrap();
        assert!(
            m.get(other, 0, 0, true).is_none(),
            "another form file's page 0, in the same slot"
        );
        // The page's other groups are entries of their own, in a slot of
        // their own here.
        assert_ne!(
            m.slot(1, 0, 1, true),
            m.slot(1, 0, 0, true),
            "the slots this builds"
        );
        let (b, n) = g(40);
        m.put(1, 0, 1, true, b, n, 0, 1);
        assert_eq!(m.bytes, 80);
        let (b, n) = g(40);
        m.put(1, 0, 2, true, b, n, 0, 1);
        assert!(m.bytes <= 100, "{}", m.bytes);
        assert!(m.get(1, 0, 2, true).is_some(), "the entry just put stays");
        assert!(
            m.get(1, 0, 3, true).is_none(),
            "a group not put is not there"
        );
        assert_eq!((m.hits, m.misses), (2, 3), "a miss a put");
        // A group over the cap on its own is held alone.
        let (b, n) = g(150);
        m.put(1, 3, 0, true, b, n, 0, 1);
        assert_eq!(m.bytes, 150);
        assert!(m.get(1, 3, 0, true).is_some() && m.get(1, 0, 2, true).is_none());
        // The same page put again replaces, and the bytes follow.
        let (b, n) = g(10);
        m.put(1, 3, 0, true, b, n, 0, 1);
        assert_eq!(m.bytes, 10);
        // Another page in the same slot, found over the pages: the
        // displaced entry's buffer, which nothing else holds, is kept for a
        // later decode and handed out once; the sweep above filled the free
        // list to its cap, so it is emptied first.
        m.free.clear();
        let alias = (4u32..)
            .find(|&p| m.slot(1, p, 0, true) == m.slot(1, 3, 0, true))
            .unwrap();
        let (b, n) = g(10);
        m.put(1, alias, 0, true, b, n, 0, 1);
        assert!(m.get(1, 3, 0, true).is_none() && m.get(1, alias, 0, true).is_some());
        assert_eq!(m.free.len(), 1, "the displaced buffer kept");
        let _ = m.take_free();
        assert!(m.free.is_empty());
        // A group a read still holds is displaced and not recycled.
        let held = Arc::new(GroupBuf::new());
        m.put(1, 20, 0, true, Arc::clone(&held), 10, 0, 1);
        let before = m.free.len();
        let again = (21u32..)
            .find(|&p| m.slot(1, p, 0, true) == m.slot(1, 20, 0, true))
            .unwrap();
        let (b, n) = g(10);
        m.put(1, again, 0, true, b, n, 0, 1);
        assert!(
            m.get(1, 20, 0, true).is_none() && m.get(1, again, 0, true).is_some(),
            "the same slot"
        );
        assert_eq!(m.free.len(), before, "a held group is not recycled");
        drop(held);
        // The slots a form file of the shipped cut reaches: four groups a
        // page over as many groups as there are slots take more than half
        // of them, where the group's number alone reached a quarter --
        // twelve of every sixteen numbers belong to indices such a page
        // never has.
        let big = Memo::new(ZMEMO_SLOTS, ZMEMO_BYTES);
        let mut reached = std::collections::HashSet::new();
        for p in 0..(ZMEMO_SLOTS / 4) as u32 {
            for j in 0..4u8 {
                reached.insert(big.slot(7, p, j, true));
            }
        }
        assert!(
            reached.len() > ZMEMO_SLOTS / 2,
            "{} slots of {} reached",
            reached.len(),
            ZMEMO_SLOTS
        );
        // The shape before the hash reached a quarter.
        let linear: std::collections::HashSet<usize> = (0..(ZMEMO_SLOTS / 4) as u32)
            .flat_map(|p| (0..4u8).map(move |j| (p, j)))
            .map(|(p, j)| big.slot(7, p, j, false))
            .collect();
        assert_eq!(linear.len(), ZMEMO_SLOTS / 4);
    }

    /// Reads through the memo: the first read of each compressed page
    /// decodes it and the memo takes the group, a second pass over the
    /// same keys hits for every one, a form file attached anew (a new
    /// identity) hits none of the entries the old one left, and with the
    /// memo off every read decodes and the memo counts nothing.
    #[test]
    fn the_memo_serves_the_second_read_of_a_page_and_never_another_attach() {
        let fx = Fixture::new("memo", suite(3000, 41), 256);
        let zfile = fx.zfile();
        let views = (0..zfile.pages()).map(|p| page_view(zfile.page_bytes(p).unwrap()).unwrap());
        let (mut groups, mut stored) = (0u64, 0u64);
        for v in views {
            groups += v.groups() as u64;
            stored += u64::from(v.header.stored.count_ones());
        }
        assert_eq!(stored, 0, "the suite's values compress");
        assert!(groups > zfile.pages() as u64, "more groups than pages");
        fx.memo.take_counts();
        let read_all = |zfile: &ZFile| {
            let mut decoded = 0u64;
            for r in &fx.recs {
                let got = zfile
                    .read_all(&fx.memo, &fx.blob, &r.0, &mut |_: &[u8]| {})
                    .unwrap();
                match got {
                    ZRead::Served { n, decoded: d } => {
                        assert_eq!(n, r.1.len() as u64);
                        decoded += u64::from(d);
                    }
                    other => panic!("{other:?}"),
                }
            }
            decoded
        };
        let n = fx.recs.len() as u64;
        let d1 = read_all(&zfile);
        let (h1, m1) = fx.memo.take_counts();
        assert_eq!(
            (m1, d1, h1 + m1),
            (groups, groups, n),
            "a miss and a decode a group, the rest hits"
        );
        let d2 = read_all(&zfile);
        assert_eq!(
            (fx.memo.take_counts(), d2),
            ((n, 0), 0),
            "the second pass hits for every read"
        );
        let other = fx.attach().unwrap();
        let d3 = read_all(&other);
        assert_eq!(
            (fx.memo.take_counts(), d3),
            ((n - groups, groups), groups),
            "a new identity misses every group once"
        );
        let off = ZFile::attach(
            &fx.out, &fx.blob, ID, END, false, false, false, true, false, false, true,
        )
        .unwrap();
        let d4 = read_all(&off);
        assert_eq!(
            (fx.memo.take_counts(), d4),
            ((0, 0), n),
            "the memo off: every read decodes"
        );
    }

    /// The trusted view of a verified page (`Options::zpages_trusted_views`)
    /// agrees with the checked view on every page of a built form file --
    /// the header, every group's entry, key and payload -- so nothing a read
    /// takes from a directory differs between the two shapes; and over a
    /// directory damaged after the verification, which the trusted view
    /// does not check, every value it hands out is bounds-checked where it
    /// is taken: no byte flipped in a page's header, directory or keys makes
    /// a view panic, whatever is asked of it.
    #[test]
    fn a_trusted_view_agrees_with_the_checked_view_and_never_panics_on_damage() {
        for (name, recs, inline_max) in [
            ("trusted-suite", suite(3000, 41), 256),
            ("trusted-shapes", shapes(), 1 << 20),
        ] {
            let fx = Fixture::new(name, recs, inline_max);
            let zfile = fx.zfile();
            assert!(zfile.pages() > 0);
            for p in 0..zfile.pages() {
                let page = zfile.page_bytes(p).unwrap();
                let c = page_view(page).unwrap();
                let t = view_trusted(page).unwrap();
                assert_eq!(c.header, t.header, "page {p}");
                assert_eq!(c.groups(), t.groups(), "page {p}");
                for j in 0..=c.groups() {
                    assert_eq!(c.group(j), t.group(j), "page {p} group {j}");
                    assert_eq!(c.key(j), t.key(j), "page {p} group {j}");
                    assert_eq!(c.payload(j), t.payload(j), "page {p} group {j}");
                }
                for i in [0usize, 1, 7, usize::from(c.header.count) - 1] {
                    assert_eq!(
                        c.group_of_index(i),
                        t.group_of_index(i),
                        "page {p} index {i}"
                    );
                }
                for k in [&b""[..], b"0", b"rand-00100", b"\xff\xff"] {
                    assert_eq!(c.group_of_key(k), t.group_of_key(k), "page {p} key {k:?}");
                }
            }
            // Damage after the verification: the header, the directory and
            // the keys, one byte at a time, every value over the page.
            let probes: [&[u8]; 4] = [b"", b"0", b"rand-00100", b"\xff\xff"];
            for p in [0, zfile.pages() / 2, zfile.pages() - 1] {
                let page = zfile.page_bytes(p).unwrap();
                let mut copy = page.to_vec();
                for at in 0..256.min(copy.len()) {
                    for flip in [0x01u8, 0x80, 0xff] {
                        copy[at] ^= flip;
                        if let Ok(v) = view_trusted(&copy) {
                            let n = v.groups();
                            for j in 0..=n {
                                let _ = (v.group(j), v.key(j), v.payload(j));
                            }
                            for i in [0usize, 1, 255, usize::from(v.header.count)] {
                                let _ = v.group_of_index(i);
                            }
                            for k in probes {
                                let _ = v.group_of_key(k);
                            }
                        }
                        copy[at] ^= flip;
                    }
                }
                assert_eq!(copy, page, "the page restored");
            }
        }
    }

    /// A memo hit asked before any view (`Options::zpages_memo_first`)
    /// answers the group's first record index and count from its entry,
    /// reading nothing of the page: a scan of the whole form file and a seek
    /// of every key answer, through the hits, as they did through the
    /// decodes that filled the memo. Asked after the view, trusted or
    /// checked, the same from the view.
    #[test]
    fn a_memo_hit_carries_the_groups_first_and_count() {
        for (trusted, memo_first) in [(true, true), (true, false), (false, false)] {
            let fx = Fixture::new(
                match (trusted, memo_first) {
                    (true, true) => "memo-first",
                    (true, false) => "memo-first-after",
                    _ => "memo-first-checked",
                },
                suite(3000, 41),
                256,
            );
            assert!(matches!(fx.build(), Outcome::Built(_)));
            let zfile = ZFile::attach(
                &fx.out, &fx.blob, ID, END, false, true, false, trusted, memo_first, false, true,
            )
            .unwrap();
            type Emitted = Vec<(Vec<u8>, Vec<u8>)>;
            let scan = |zfile: &ZFile| -> (Emitted, u64) {
                let mut out: Emitted = Vec::new();
                let r = zfile
                    .scan_at(
                        &fx.memo,
                        &fx.blob,
                        0,
                        usize::MAX,
                        &mut |k: &[u8], v: &[u8]| out.push((k.to_vec(), v.to_vec())),
                    )
                    .unwrap();
                assert!(matches!(r.end, ScanEnd::Served), "{:?}", r.end);
                (out, r.decoded)
            };
            let seeks = |zfile: &ZFile| -> Vec<(usize, KeyOrder)> {
                fx.recs
                    .iter()
                    .map(|r| match zfile.seek(&fx.memo, &r.0) {
                        ZSeek::Served { rank, at, .. } => (rank, at),
                        other => panic!("{other:?}"),
                    })
                    .collect()
            };
            fx.memo.take_counts();
            let (first_pass, d1) = scan(&zfile);
            assert!(d1 > 0, "the first pass decodes");
            assert_eq!(
                first_pass.len(),
                fx.recs.iter().map(|r| r.1.len()).sum::<usize>()
            );
            let seeks1 = seeks(&zfile);
            assert!(seeks1.iter().all(|&(_, at)| at == KeyOrder::Equal));
            fx.memo.take_counts();
            let (second_pass, d2) = scan(&zfile);
            let seeks2 = seeks(&zfile);
            let (hits, misses) = fx.memo.take_counts();
            assert_eq!((d2, misses), (0, 0), "the second pass hits for every group");
            assert!(hits > 0);
            assert_eq!(
                first_pass, second_pass,
                "trusted {trusted} memo first {memo_first}"
            );
            assert_eq!(seeks1, seeks2, "trusted {trusted} memo first {memo_first}");
        }
    }

    #[test]
    fn a_form_file_of_another_segment_is_refused() {
        // The same ids, the same keys, other values: the hard case. Form file
        // one planted where form file two belongs.
        let one = Fixture::new("other-1", suite(2000, 31), 256);
        let two = Fixture::new("other-2", suite(2000, 32), 256);
        one.zfile();
        two.zfile();
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
        one.zfile();
        let b = &one.blob;
        assert!(matches!(
            ZFile::attach(
                &one.out,
                b,
                ID + 1,
                END,
                false,
                true,
                false,
                true,
                false,
                false,
                true
            ),
            Err(Reject::Identity(_))
        ));
        assert!(matches!(
            ZFile::attach(
                &one.out,
                b,
                ID,
                END + 1,
                false,
                true,
                false,
                true,
                false,
                false,
                true
            ),
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
        fx.zfile();
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
                // Eight, so that the filter's place, computed from the
                // keys' padded end, is the check that fires and not the
                // offsets' or the filter's by turns as the padding falls.
                Box::new(|t| t.keys_len += 8),
                "damage: z form: the resident part is malformed: the filter is not where the entries end",
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
                "identity: the form file names another segment",
            ),
            (
                "seg_end_seq",
                Box::new(|t| t.seg_end_seq += 1),
                "identity: the form file names another segment",
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
        let zfile = fx.attach().unwrap_or_else(|e| panic!("total_ulen: {e:?}"));
        agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
        // The file as built still attaches.
        std::fs::write(&fx.out, &good).unwrap();
        let zfile = fx.attach().unwrap();
        assert_eq!(zfile.ident(), &t0);
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
        // the page bad and takes the form file out of service. No flip is
        // ever a wrong answer, and none panics.
        let fx = Fixture::new("flips", suite(1500, 51), 256);
        fx.zfile();
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
                Ok(zfile) => {
                    assert!(!sampled.contains(&p), "byte {at} of sampled page {p}");
                    // Every key of the damaged page, and a few of the rest.
                    let mut hit = 0;
                    for (i, (k, vals, _)) in fx.recs.iter().enumerate() {
                        let e = zfile.fence.entry_of(k).unwrap();
                        let on_it = zfile.fence.page(e) == Some(p as u32);
                        if !on_it && i % 97 != 0 {
                            continue;
                        }
                        let mut got = Vec::new();
                        match zfile
                            .read_all(&fx.memo, &fx.blob, k, &mut |v: &[u8]| got.push(v.to_vec()))
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
                        assert!(zfile.damaged() && zfile.bad_pages() >= 1, "byte {at}");
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
        let zfile = fx.zfile();
        let f = &zfile.fence;
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

    use crate::db::zpages::{encode_page, PageGroup};

    /// The images of page `p`'s records out of `bytes`, the whole form file:
    /// every group's, in order.
    fn page_images(bytes: &[u8], p: usize, bind: u32) -> Vec<Vec<u8>> {
        let view = parse_page(&bytes[p * ZPAGE..(p + 1) * ZPAGE], bind).unwrap();
        let mut buf = GroupBuf::new();
        let mut group = Vec::new();
        for j in 0..view.groups() {
            group.extend_from_slice(decode_group_with(&view, j, &mut buf, false).unwrap());
        }
        let (mut out, mut off) = (Vec::new(), 0usize);
        for _ in 0..view.header.count {
            let (_, next) = crate::flatindex::record_key_next(&group, off).unwrap();
            out.push(group[off..next].to_vec());
            off = next;
        }
        assert_eq!(off, group.len());
        out
    }

    /// Page `p` of the form file at `path` rewritten in place as one group
    /// of `images`, under the `first_rank` and `count` the caller names and
    /// with a CRC that passes: a page that is whole and is wrong. Written in
    /// place and never truncated, so a mapping a reader holds sees the page
    /// change.
    fn forge(path: &Path, p: usize, first_rank: u32, count: u16, images: &[Vec<u8>]) {
        forge_groups(path, p, first_rank, &[(count, images.to_vec(), Vec::new())]);
    }

    /// A group as `forge_groups` takes it: its record count, its images and
    /// the key the directory names it by (unused for the first group, whose
    /// key is the fence's).
    type ForgedGroup = (u16, Vec<Vec<u8>>, Vec<u8>);

    /// `forge` with the page cut into the groups given: a directory that
    /// lies is built here and nowhere else.
    fn forge_groups(path: &Path, p: usize, first_rank: u32, groups: &[ForgedGroup]) {
        let bytes = std::fs::read(path).unwrap();
        let t = Trailer::decode(&bytes[bytes.len() - ZTRAILER..]).unwrap();
        let payloads: Vec<(Vec<u8>, Vec<u8>)> = groups
            .iter()
            .map(|(_, images, _)| {
                let group: Vec<u8> = images.concat();
                (lz4_flex::block::compress(&group), group)
            })
            .collect();
        let pg: Vec<PageGroup<'_>> = groups
            .iter()
            .zip(&payloads)
            .map(|((count, _, key), (payload, group))| PageGroup {
                count: *count,
                payload,
                ulen: group.len() as u32,
                stored: false,
                coded: false,
                key,
            })
            .collect();
        let mut page = [0u8; ZPAGE];
        encode_page(&mut page, first_rank, &pg, t.bind32()).expect("the forged page fits");
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
        fx.zfile();
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
        let check = |name: &str, zfile: &ZFile| {
            let mut declined = 0;
            for (k, vals, _) in &on_page {
                match z_values(&fx.memo, zfile, &fx.blob, k) {
                    Some(got) => assert_eq!(&got, vals, "{name}: {k:?} answered {got:?}"),
                    None => declined += 1,
                }
            }
            assert!(declined > 0, "{name}: nothing was refused");
            assert!(zfile.damaged(), "{name}: the form file stayed in service");
        };
        for (name, rank, cnt, imgs) in variants {
            std::fs::write(&fx.out, &good).unwrap();
            forge(&fx.out, p, rank, cnt, &imgs);
            let zfile = fx
                .attach()
                .expect("an unsampled page is not looked at by the attach");
            check(name, &zfile);
            assert_eq!(
                zfile.verified_pages(),
                0,
                "{name}: a refused page is verified"
            );
        }
        // The directory's lies, under a good CRC and the right records: the
        // key it names a group by is not the group's first record's, and
        // a record of the group before it is not below it. Each is the walk
        // of one group's to refuse, so a page that passed its other group's
        // first decode is refused all the same when the lying group is
        // read.
        let half = n / 2;
        let key_of = |img: &[u8]| crate::flatindex::record_key(img, 0).unwrap().to_vec();
        let cut = |key: Vec<u8>| {
            vec![
                (half as u16, images[..half].to_vec(), Vec::new()),
                ((n - half) as u16, images[half..].to_vec(), key),
            ]
        };
        for (name, key) in [
            (
                "the second group named by its second key",
                key_of(&images[half + 1]),
            ),
            (
                "the second group named by the first group's last key",
                key_of(&images[half - 1]),
            ),
        ] {
            std::fs::write(&fx.out, &good).unwrap();
            forge_groups(&fx.out, p, first_rank, &cut(key));
            let zfile = fx.attach().expect("unsampled");
            // The page's first decode walks both groups, whichever is
            // read first, so the first read of the page refuses either lie.
            check(name, &zfile);
        }
        // The control: the page cut into two groups named truly is read
        // whole and verified group by group.
        std::fs::write(&fx.out, &good).unwrap();
        forge_groups(&fx.out, p, first_rank, &cut(key_of(&images[half])));
        let zfile = fx.attach().expect("unsampled");
        for (k, vals, _) in &on_page {
            assert_eq!(z_values(&fx.memo, &zfile, &fx.blob, k).as_ref(), Some(vals));
        }
        assert!(!zfile.damaged());
        assert_eq!(zfile.verified_pages(), 1, "the page verified whole");
    }

    #[test]
    fn a_sampled_page_with_a_good_crc_and_the_wrong_count_is_refused_at_attach() {
        // The attach's sample compares each page it reads with the fence
        // entry that names it, since a CRC that passes is no proof of that.
        let fx = Fixture::new("sampled", suite(4000, 62), 256);
        fx.zfile();
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
        // it is set after every check has passed and not before: the form file
        // leaves service on the failure, which hides a bit set early from a
        // single reader, and a second reader that found the bit would serve
        // the page unchecked.
        let fx = Fixture::new("unverified", suite(4000, 63), 256);
        fx.zfile();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = unsampled(t.n_pages as usize, 5);
        let f = OpenOptions::new().write(true).open(&fx.out).unwrap();
        f.write_all_at(&[good[p * ZPAGE + 100] ^ 0x5a], (p * ZPAGE + 100) as u64)
            .unwrap();
        drop(f);
        let zfile = fx.attach().unwrap();
        let entry = zfile.fence.entry(p, zfile.nkeys as u32).unwrap();
        let key = fx.recs[entry.first_rank as usize + 1].0.clone();
        let mut got = 0;
        let r = zfile
            .read_all(&fx.memo, &fx.blob, &key, &mut |_: &[u8]| got += 1)
            .unwrap();
        assert_eq!((r, got), (ZRead::Fallback, 0));
        assert_eq!(zfile.verified_pages(), 0, "a page that failed is verified");
        assert_eq!(zfile.bad_pages(), 1);
        assert!(zfile.damaged());
        // The form file is out of service, and what it is asked verifies nothing.
        assert_eq!(zfile.count(&fx.memo, &fx.recs[0].0), ZRead::Out);
        assert_eq!(zfile.verified_pages(), 0);
    }

    #[test]
    fn a_verified_page_that_changes_underneath_is_never_answered_absent() {
        // Bit rot after the first decode: the page is read as verified, so
        // its CRC is not looked at again, and a group that no longer searches
        // is damage and not an absent key.
        let fx = Fixture::new("rot", suite(3000, 65), 256);
        fx.zfile();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = t.n_pages as usize / 2;
        // The memo off: with it on the group is served as it was decoded
        // while the memo holds it, whatever the page became.
        let zfile = fx.attach_nomemo().unwrap();
        let entry = zfile.fence.entry(p, zfile.nkeys as u32).unwrap();
        let (first_rank, count) = (entry.first_rank, entry.count as usize);
        let k = fx.recs[first_rank as usize + 3].clone();
        assert_eq!(
            z_values(&fx.memo, &zfile, &fx.blob, &k.0),
            Some(k.1.clone())
        );
        assert_eq!(zfile.verified_pages(), 1);
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
            match z_values(&fx.memo, &zfile, &fx.blob, key) {
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
        assert!(zfile.damaged() && zfile.bad_pages() == 1);
    }

    #[test]
    fn a_record_that_stops_parsing_after_its_page_was_verified_is_never_answered_absent() {
        // The search of a group reads a record's key and its length, which
        // for a compact record is the two bytes after the key and not the
        // header's other two; the record is parsed whole only once it is
        // found. A header the run contradicts is damage the search does not
        // see, and the page was verified before it came: the record that is
        // found and does not parse is the form file's damage, and a read that
        // took it for no record would answer a key it holds as missing.
        let fx = Fixture::new("rot-record", suite(3000, 68), 256);
        fx.zfile();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = t.n_pages as usize / 2;
        let zfile = fx.attach_nomemo().unwrap();
        let entry = zfile.fence.entry(p, zfile.nkeys as u32).unwrap();
        let (first_rank, count) = (entry.first_rank, entry.count as u16);
        let (key, vals, _) = fx.recs[first_rank as usize + 3].clone();
        assert_eq!(
            z_values(&fx.memo, &zfile, &fx.blob, &key),
            Some(vals.clone())
        );
        assert_eq!(zfile.verified_pages(), 1);
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
        let r = zfile
            .read_all(&fx.memo, &fx.blob, &key, &mut |_: &[u8]| got += 1)
            .unwrap();
        assert_eq!((r, got), (ZRead::Fallback, 0));
        assert!(zfile.damaged() && zfile.bad_pages() == 1);
        // The records before it in the group were answered while it held.
        assert_eq!(zfile.count(&fx.memo, &key), ZRead::Out);
    }

    #[test]
    fn a_second_install_is_refused() {
        let fx = Fixture::new("install", suite(2000, 67), 256);
        fx.zfile();
        let cell = ZCell::new();
        assert!(cell.get().is_none());
        assert!(cell.install(fx.attach().unwrap()));
        let first = cell.get().unwrap().id();
        assert!(
            !cell.install(fx.attach().unwrap()),
            "a cell holds one form file"
        );
        assert_eq!(cell.get().unwrap().id(), first);
    }

    #[test]
    fn the_trailer_sums_the_groups_and_the_file_is_the_length_the_build_returns() {
        let fx = Fixture::new("sums", suite(3000, 64), 256);
        let Outcome::Built(b) = fx.build() else {
            panic!("built")
        };
        let zfile = fx.attach().unwrap();
        let bytes = std::fs::read(&fx.out).unwrap();
        let mut sum = 0u64;
        for p in 0..zfile.pages() {
            sum += u64::from(
                page_view(&bytes[p * ZPAGE..(p + 1) * ZPAGE])
                    .unwrap()
                    .header
                    .ulen,
            );
        }
        assert_eq!(zfile.group_bytes(), sum);
        assert_eq!(b.total_ulen, sum);
        assert_eq!(zfile.file_bytes(), bytes.len() as u64);
        assert_eq!(b.file_bytes, bytes.len() as u64);
        assert_eq!(b.fill.pages as usize, zfile.pages());
    }

    // ---- what the build leaves behind ---------------------------------------

    /// The form file's temp name.
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
        // leaves neither the form file nor its temp name.
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
        assert!(r.is_err(), "records out of order built a form file: {r:?}");
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

    /// The ask ahead of a build's walk asks the kernel, not a note: a
    /// copy of a segment evicted from the page cache, asked for whole
    /// through a mapping of its own, is resident within a moment, and one
    /// not asked for stays cold.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_ask_ahead_brings_the_cold_pages_in() {
        let fx = Fixture::new("ahead", suite(3000, 9), 1 << 20);
        let page = crate::bytes::os_page_size();
        let cold = |name: &str| {
            let p = fx.dir.join(name);
            std::fs::copy(&fx.seg, &p).unwrap();
            evict(&p);
            let m = MmapBytes::open(&p).unwrap();
            let all = m.0.len().div_ceil(page);
            let res = m.resident_pages(0, m.0.len());
            assert!(
                res < all / 4,
                "{name}: {res} of {all} resident after the eviction"
            );
            (m, all)
        };
        let (asked, all) = cold("seg-asked");
        ask_ahead(asked.0.as_ptr() as usize, 0, asked.0.len());
        let (left, _) = cold("seg-left");
        let t = std::time::Instant::now();
        let mut res = asked.resident_pages(0, asked.0.len());
        while res * 4 < all * 3 && t.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
            res = asked.resident_pages(0, asked.0.len());
        }
        assert!(
            res * 4 >= all * 3,
            "{res} of {all} pages resident after the ask"
        );
        let untouched = left.resident_pages(0, left.0.len());
        assert!(
            untouched < all / 4,
            "{untouched} of {all} resident with nothing asked"
        );
    }

    /// A form file evicted from the page cache and warmed: every page
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

    /// Every window of the scan, through the form file and through the raw
    /// form: every start rank of the grid and every limit, the keys and
    /// values in order and the count returned, on the suite's shape and on
    /// the fixture with a record no page holds among the pages.
    #[test]
    fn a_scan_through_the_form_file_agrees_with_the_raw_walk_on_every_window() {
        let mut rng = Rng(77);
        let mut recs = suite(3000, 5);
        let big = b"0000000000001500-big";
        recs.insert(1500, (big.to_vec(), vec![rng.bytes(6000)], false));
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, recs) in [("scan-suite", suite(2500, 6)), ("scan-nopage", recs)] {
            let fx = Fixture::new(name, recs, 1 << 20);
            let zfile = fx.zfile();
            let n = fx.recs.len();
            let starts: Vec<usize> = (0..n).step_by(37).chain([n - 1, n, n + 5]).collect();
            let limits = [0usize, 1, 2, 3, 5, 17, 56, 57, 58, 114, 1000, usize::MAX];
            for &from in &starts {
                for &limit in &limits {
                    let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    let raw_n = fx
                        .blob
                        .scan_at(from, limit, |k, v| want.push((k.to_vec(), v.to_vec())))
                        .unwrap();
                    let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    let z = zfile
                        .scan_at(
                            &fx.memo,
                            &fx.blob,
                            from,
                            limit,
                            &mut |k: &[u8], v: &[u8]| got.push((k.to_vec(), v.to_vec())),
                        )
                        .unwrap();
                    assert_eq!(z.end, ScanEnd::Served, "{name}: from {from} limit {limit}");
                    assert_eq!(
                        z.seen, raw_n,
                        "{name}: the count, from {from} limit {limit}"
                    );
                    assert_eq!(got, want, "{name}: the stream, from {from} limit {limit}");
                }
            }
            // A walk across the record no page holds hands the raw form
            // exactly its run, once; the suite's shape never does.
            let mut sink = |_: &[u8], _: &[u8]| {};
            fx.memo.take_counts();
            let z = zfile
                .scan_at(&fx.memo, &fx.blob, 0, usize::MAX, &mut sink)
                .unwrap();
            let (hits, _) = fx.memo.take_counts();
            assert_eq!(z.nopage, u64::from(name == "scan-nopage"), "{name}");
            assert!(z.decoded + hits > 1, "{name}: {z:?}, {hits} from the memo");
            assert!(!zfile.damaged());
        }
    }

    /// The seek, the cut and the record by rank against the raw form, on
    /// the suite's shape and with a record no page holds among the pages:
    /// every probe key's seek rank and whether it is found; the cut of
    /// every window at every probe against a walk of the keys; the values
    /// at every rank, the one no page holds answered as such.
    #[test]
    fn the_seek_the_cut_and_the_values_by_rank_agree_with_the_raw_form() {
        let mut rng = Rng(77);
        let mut recs = suite(1200, 5);
        let big = b"0000000000000600-big";
        recs.insert(600, (big.to_vec(), vec![rng.bytes(6000)], false));
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, recs) in [("seek-suite", suite(1000, 6)), ("seek-nopage", recs)] {
            let fx = Fixture::new(name, recs, 1 << 20);
            let zfile = fx.zfile();
            let n = fx.recs.len();
            let keys: Vec<&[u8]> = fx.recs.iter().map(|r| r.0.as_slice()).collect();
            for key in probes(&fx.recs) {
                let want = fx.blob.seek(&key);
                let want_at = if keys.binary_search(&key.as_slice()).is_ok() {
                    KeyOrder::Equal
                } else {
                    KeyOrder::Greater
                };
                let got = zfile.seek(&fx.memo, &key);
                assert!(
                    matches!(got, ZSeek::Served { rank, at, .. } if rank == want && at == want_at),
                    "{name}: seek of {key:?}: {got:?}, wanted {want} {want_at:?}"
                );
                // The owner's span by the fence and the directory alone holds
                // the owner the seek names: the rank where the key is there,
                // the rank before it otherwise.
                let owner = if want_at == KeyOrder::Equal {
                    want
                } else {
                    want.saturating_sub(1)
                };
                let span = zfile.owner_span(&key);
                assert!(
                    span.is_some_and(|(lo, hi)| lo <= owner && owner < hi),
                    "{name}: span of {key:?}: {span:?}, the owner {owner}"
                );
                for (rank, end) in [(0usize, n), (0, 1), (n / 3, 2 * n / 3), (n - 1, n), (n, n)] {
                    let r = (rank..end)
                        .find(|&i| keys[i] >= key.as_slice())
                        .unwrap_or(end);
                    let at = if r < end && keys[r] == key.as_slice() {
                        KeyOrder::Equal
                    } else {
                        KeyOrder::Greater
                    };
                    let got = zfile.cut_in(&fx.memo, rank, end, &key);
                    assert!(
                        matches!(got, ZSeek::Served { rank, at: a, .. } if rank == r && a == at),
                        "{name}: cut of {key:?} in {rank}..{end}: {got:?}, wanted {r} {at:?}"
                    );
                }
            }
            for rank in (0..n + 2).step_by(7) {
                let mut want = Vec::new();
                let raw_n = fx.blob.values_at(rank, |v| want.push(v.to_vec())).unwrap();
                let mut got = Vec::new();
                let z = zfile
                    .values_at(&fx.memo, &fx.blob, rank, &mut |v: &[u8]| {
                        got.push(v.to_vec())
                    })
                    .unwrap();
                if fx.recs.get(rank).is_some_and(|r| r.0 == big) {
                    assert_eq!(z, ZRead::NoPage, "{name}: rank {rank}");
                    assert!(got.is_empty());
                } else {
                    assert!(
                        matches!(z, ZRead::Served { n, .. } if n == raw_n),
                        "{name}: rank {rank}: {z:?}, wanted {raw_n} values"
                    );
                    assert_eq!(got, want, "{name}: the values at {rank}");
                }
            }
            assert!(!zfile.damaged());
        }
    }

    /// The cursor walks every key and its values from every start as the
    /// raw form's `key_at` and `values_at` sequence has them, across groups
    /// and across the record no page holds, which it reads through the raw
    /// form; a seek lands where the raw form's does; and a page flipped
    /// under it makes the rest of its walk the raw form's from the first
    /// check that fails, with every key still right.
    #[test]
    fn the_cursor_walks_the_records_in_order_and_falls_to_the_raw_form_on_damage() {
        let mut rng = Rng(78);
        let mut recs = suite(1500, 9);
        let big = b"0000000000000750-big";
        recs.insert(750, (big.to_vec(), vec![rng.bytes(6000)], false));
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        // Under the block coding and under the raw tails read in place: the
        // seek into a group read in place is the seek over its heads, and
        // the first version sought it by the walk of its records, which such
        // a group answers no bytes for, and failed the page.
        for (name, recs, raw) in [
            ("cursor-suite", suite(1200, 10), false),
            ("cursor-nopage", recs.clone(), false),
            ("cursor-suite-raw", suite(1200, 10), true),
            ("cursor-nopage-raw", recs, true),
        ] {
            let fx = if raw {
                Fixture::with_shape(name, recs, 1 << 20, ZCODING_RECORD, true)
            } else {
                Fixture::new(name, recs, 1 << 20)
            };
            let zfile = fx.zfile();
            let n = fx.recs.len();
            let nopage = u64::from(name.starts_with("cursor-nopage"));
            for start in (0..n + 3).step_by(53) {
                let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, start);
                let mut rank = start;
                loop {
                    assert_eq!(c.rank(), rank, "{name}: from {start}");
                    let key = c.key().map(|k| k.to_vec());
                    assert_eq!(
                        key.as_deref(),
                        fx.blob.key_at(rank),
                        "{name}: the key at {rank}, from {start}"
                    );
                    if key.is_none() {
                        break;
                    }
                    let mut want = Vec::new();
                    fx.blob.values_at(rank, |v| want.push(v.to_vec())).unwrap();
                    let mut got = Vec::new();
                    c.values(&mut |v: &[u8]| got.push(v.to_vec())).unwrap();
                    assert_eq!(got, want, "{name}: the values at {rank}, from {start}");
                    c.advance();
                    rank += 1;
                }
                assert!(!c.fell_back(), "{name}: from {start}");
                if start == 0 {
                    assert_eq!(c.nopage, nopage, "{name}: the run no page holds, met once");
                    if raw {
                        assert!(
                            c.inplace > 0 && c.decoded == 0,
                            "{name}: the raw-tail groups read in place, none decoded"
                        );
                    } else {
                        assert!(c.decoded > 0, "{name}: groups decoded on the way");
                    }
                }
                // The seek to a key lands where the raw form's does, and so
                // does one to a key between records.
                let k = &fx.recs[start.min(n - 1)].0;
                let c = ZCursor::seek(&fx.memo, &zfile, &fx.blob, k).unwrap();
                assert_eq!((c.rank(), c.key()), (start.min(n - 1), Some(k.as_slice())));
                let between = [k.as_slice(), &[0u8]].concat();
                let c = ZCursor::seek(&fx.memo, &zfile, &fx.blob, &between).unwrap();
                assert_eq!(
                    c.rank(),
                    fx.blob.seek(&between),
                    "{name}: seek of {between:?}"
                );
                assert_eq!(c.key(), fx.blob.key_at(c.rank()));
            }
            let c = ZCursor::seek(&fx.memo, &zfile, &fx.blob, b"").unwrap();
            assert_eq!(c.rank(), 0);
            let c = ZCursor::seek(&fx.memo, &zfile, &fx.blob, &[0xffu8; 8]).unwrap();
            assert_eq!((c.rank(), c.key()), (n, None));
            assert!(!zfile.damaged());
            // A page flipped under the cursor: the first check that fails
            // makes the rest of the walk the raw form's, and the keys and
            // values stay right.
            let good = std::fs::read(&fx.out).unwrap();
            let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
            let p = unsampled(t.n_pages as usize, 3);
            let mut flipped = good.clone();
            flipped[p * ZPAGE + 100] ^= 0x40;
            std::fs::write(&fx.out, &flipped).unwrap();
            let zfile = fx
                .attach()
                .expect("the sample does not reach the flipped page");
            let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
            for rank in 0..n {
                assert_eq!(c.key(), fx.blob.key_at(rank), "{name}: flipped, at {rank}");
                let mut want = Vec::new();
                fx.blob.values_at(rank, |v| want.push(v.to_vec())).unwrap();
                let mut got = Vec::new();
                c.values(&mut |v: &[u8]| got.push(v.to_vec())).unwrap();
                assert_eq!(got, want, "{name}: flipped, the values at {rank}");
                c.advance();
            }
            assert_eq!((c.key(), c.rank()), (None, n));
            assert!(c.fell_back() && zfile.damaged(), "{name}: the flip was met");
            assert_eq!(zfile.seek(&fx.memo, b"0"), ZSeek::Out);
            assert!(ZCursor::seek(&fx.memo, &zfile, &fx.blob, b"0").is_err());
            std::fs::write(&fx.out, &good).unwrap();
        }
    }

    /// The bulk walk through the cursor (`walk_until`) hands over the keys
    /// and values from the position exactly as the raw form has them, up
    /// to the limit or the first key not below `until`, says how that key
    /// compares, and leaves the cursor on it, so that the overlay walk's
    /// turns -- walk to an unsealed key, read the record under the cursor,
    /// step past it, walk on -- are one walk with one open a group crossed;
    /// across the record no page holds, and under a flipped page, every
    /// key still right; and a long walk asks ahead once where the prefetch
    /// is on, a walk of one key never.
    /// A raw-tail coded group is read where it lies (`Group::Coded`): the
    /// cursor's walk from every start hands over every key and value the raw
    /// form has, in order, over the suite's shape and over every record
    /// shape, reading its groups in place and decoding nothing, with the
    /// memo asked nothing; and with the walk off (`ZFile::inplace`) the same
    /// cursor decodes every group it crosses, as the shape before did. The
    /// position's own reads (`key`, `values`, `advance`) agree with the raw
    /// form at every rank either way.
    #[test]
    fn the_cursor_reads_a_raw_tail_group_in_place_and_decodes_nothing() {
        for (name, recs, inline_max) in [
            ("inplace-suite", suite(1500, 23), 256),
            ("inplace-shapes", shapes(), 1 << 20),
        ] {
            let fx = Fixture::with_shape(name, recs, inline_max, ZCODING_RECORD, true);
            let n = fx.recs.len();
            for inplace in [true, false] {
                let mut zfile = fx.zfile();
                zfile.inplace = inplace;
                let what = format!("{name} in place {inplace}");
                // The file's groups by kind: the record coding's, which the
                // in-place walk reads where they lie, and the blocks the
                // builder falls back to where a record's head passes the
                // page -- the shapes' long keys -- which are decoded as
                // they were; a stored group is parsed in place either way.
                let (coded, blocks) = (0..zfile.pages())
                    .map(|p| {
                        let v = page_view(zfile.page_bytes(p).unwrap()).unwrap();
                        (0..v.groups()).fold((0usize, 0usize), |(c, b), j| {
                            let g = v.group(j).unwrap();
                            (
                                c + usize::from(g.coded),
                                b + usize::from(!g.coded && !g.stored),
                            )
                        })
                    })
                    .fold((0, 0), |(c, b), (dc, db)| (c + dc, b + db));
                assert!(coded > 0, "{what}: the form has coded groups");
                fx.memo.take_counts();
                // The bulk walk from several starts.
                for start in (0..n + 2).step_by(97) {
                    let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, start);
                    let (walked, order) = c
                        .walk_until(None, n + 5, &mut |k: &[u8], v: &[u8]| {
                            got.push((k.to_vec(), v.to_vec()))
                        })
                        .unwrap();
                    assert_eq!(order, KeyOrder::Greater, "{what}: from {start}");
                    assert_eq!(walked, n.saturating_sub(start), "{what}: from {start}");
                    let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    for rank in start..n {
                        let k = fx.blob.key_at(rank).unwrap().to_vec();
                        fx.blob
                            .values_at(rank, |v| want.push((k.clone(), v.to_vec())))
                            .unwrap();
                    }
                    assert_eq!(got, want, "{what}: the walk from {start}");
                    assert!(
                        !c.fell_back(),
                        "{what}: fell from {start}: decoded {} in place {} bad pages {} nopage {}",
                        c.decoded,
                        c.inplace,
                        zfile.bad_pages(),
                        c.nopage
                    );
                    if start == 0 {
                        // From the first record every group is opened once:
                        // the coded ones in place, or decoded with the walk
                        // off, and the blocks decoded either way.
                        let want = if inplace {
                            (coded, blocks)
                        } else {
                            (0, coded + blocks)
                        };
                        assert_eq!(
                            (c.inplace as usize, c.decoded as usize),
                            want,
                            "{what}: (in place, decoded) over {coded} coded and {blocks} block groups"
                        );
                    }
                }
                // The position's reads, rank by rank.
                let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
                for rank in 0..n {
                    assert_eq!(c.rank(), rank, "{what}");
                    assert_eq!(c.key(), fx.blob.key_at(rank), "{what}: the key at {rank}");
                    let mut want = Vec::new();
                    fx.blob.values_at(rank, |v| want.push(v.to_vec())).unwrap();
                    let mut got = Vec::new();
                    let count = c.values(&mut |v: &[u8]| got.push(v.to_vec())).unwrap();
                    assert_eq!(got, want, "{what}: the values at {rank}");
                    assert_eq!(count as usize, want.len(), "{what}: the count at {rank}");
                    c.advance();
                }
                assert_eq!(c.key(), None, "{what}: past the end");
                assert!(!c.fell_back(), "{what}");
                // The memo is asked for the groups decoded and for no group
                // read in place: nothing at all over the suite's shape with
                // the walk on, whose groups all code.
                let (hits, misses) = fx.memo.take_counts();
                assert_eq!(
                    hits + misses == 0,
                    inplace && blocks == 0,
                    "{what}: the memo asked {hits} hits {misses} misses over {blocks} block groups"
                );
                assert!(!zfile.damaged(), "{what}");
            }
        }
    }

    /// A raw-tail coded page whose CRC is good and whose record lies: a
    /// compact header naming a run longer than the tail that holds it. The
    /// heads pass the first touch's check, which reads no tail, and the
    /// in-place walk meets the lie where it parses the record: the page goes
    /// out of service and the rest of the walk is the raw form's, every key
    /// and value still right, as the decoded walk does on a record that
    /// does not parse.
    #[test]
    fn a_raw_tail_record_that_lies_about_its_run_falls_the_walk_to_the_raw_form() {
        use crate::db::zpages::{encode_page, encode_record_group, PageGroup, ZCAP};
        use std::os::unix::fs::FileExt;
        let fx = Fixture::with_shape("inplace-forged", suite(3000, 29), 256, ZCODING_RECORD, true);
        fx.zfile();
        let good = std::fs::read(&fx.out).unwrap();
        let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
        let p = unsampled(t.n_pages as usize, 7);
        let side0 = fx.attach().unwrap();
        let entry = side0.fence.entry(p, side0.nkeys as u32).unwrap();
        let (first_rank, count) = (entry.first_rank as usize, entry.count as usize);
        drop(side0);
        // The page's records as the writer laid them, re-encoded raw-tailed
        // with the third record's run length a byte long: the group's
        // bytes stay the same length, so the lie is in the record alone.
        let images: Vec<Vec<u8>> = (first_rank..first_rank + count)
            .map(|rank| {
                let (k, vals, _) = &fx.recs[rank];
                assert_eq!(vals.len(), 1, "the suite's records hold one value");
                record_image(k, &vals[0])
            })
            .collect();
        let mut imgs = images.clone();
        {
            // The compact header lies after the key: its first two bytes
            // are the run's length.
            let img = &mut imgs[2];
            let klen = u16::from_le_bytes([img[0], img[1]]) as usize;
            let e_at = (4 + klen).next_multiple_of(4);
            let len = u16::from_le_bytes([img[e_at], img[e_at + 1]]);
            img[e_at..e_at + 2].copy_from_slice(&(len + 64).to_le_bytes());
        }
        let ends: Vec<usize> = imgs
            .iter()
            .scan(0usize, |at, img| {
                *at += img.len();
                Some(*at)
            })
            .collect();
        let flat: Vec<u8> = imgs.concat();
        let mut payload = vec![0u8; ZCAP];
        let plen = encode_record_group(&flat, &ends, &mut payload, true).expect("the lie encodes");
        payload.truncate(plen);
        let pg = [PageGroup {
            count: count as u16,
            payload: &payload,
            ulen: flat.len() as u32,
            stored: false,
            coded: true,
            key: &[],
        }];
        let mut page = [0u8; ZPAGE];
        encode_page(&mut page, first_rank as u32, &pg, t.bind32()).expect("the forged page fits");
        let f = OpenOptions::new().write(true).open(&fx.out).unwrap();
        f.write_all_at(&page, (p * ZPAGE) as u64).unwrap();
        let zfile = fx.attach().unwrap();
        assert!(zfile.inplace);
        let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, first_rank.saturating_sub(5));
        c.walk_until(None, count + 10, &mut |k: &[u8], v: &[u8]| {
            got.push((k.to_vec(), v.to_vec()))
        })
        .unwrap();
        let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for rank in first_rank.saturating_sub(5)..first_rank.saturating_sub(5) + count + 10 {
            let k = fx.blob.key_at(rank).unwrap().to_vec();
            fx.blob
                .values_at(rank, |v| want.push((k.clone(), v.to_vec())))
                .unwrap();
        }
        assert_eq!(got, want, "every key and value right across the lie");
        assert!(c.fell_back(), "the walk fell to the raw form at the lie");
        assert!(zfile.damaged(), "the page is out of service");
        assert_eq!(zfile.bad_pages(), 1);
    }

    #[test]
    fn the_cursors_bulk_walk_stops_at_the_key_asked_and_hands_over_what_the_raw_form_has() {
        let mut rng = Rng(79);
        let mut recs = suite(1500, 11);
        let big = b"0000000000000750-big";
        recs.insert(750, (big.to_vec(), vec![rng.bytes(6000)], false));
        recs.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, recs) in [("bulk-suite", suite(1200, 12)), ("bulk-nopage", recs)] {
            let fx = Fixture::new(name, recs, 1 << 20);
            let zfile = fx.zfile();
            let n = fx.recs.len();
            let raw_seq = |from: usize, to: usize| -> Vec<(Vec<u8>, Vec<u8>)> {
                let mut v = Vec::new();
                for r in from..to.min(n) {
                    let k = fx.blob.key_at(r).unwrap().to_vec();
                    fx.blob
                        .values_at(r, |val| v.push((k.clone(), val.to_vec())))
                        .unwrap();
                }
                v
            };
            let key_at = |r: usize| fx.recs.get(r).map(|rec| rec.0.clone());
            for start in (0..n).step_by(47).chain([n]) {
                for ahead in [0usize, 1, 5, 40, 300] {
                    let target = start + ahead;
                    // `until`: the key `ahead` records on (stopped at it,
                    // `Equal`), the key just past it (stopped one further,
                    // `Greater`), and none (the partition's end).
                    let mut untils: Vec<(Option<Vec<u8>>, usize, KeyOrder)> =
                        vec![(None, n, KeyOrder::Greater)];
                    if let Some(k) = key_at(target) {
                        untils.push((Some(k.clone()), target, KeyOrder::Equal));
                        let between = [k.as_slice(), &[0u8]].concat();
                        untils.push((Some(between), target + 1, KeyOrder::Greater));
                    }
                    for (until, stop, at_want) in untils {
                        for limit in [1usize, 7, usize::MAX] {
                            let what = format!(
                                "{name}: from {start}, until {:?} (stop {stop}), limit {limit}",
                                until.as_deref().map(String::from_utf8_lossy)
                            );
                            let stop = stop.min(n);
                            let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, start);
                            let mut got = Vec::new();
                            let (got_n, at) = c
                                .walk_until(until.as_deref(), limit, &mut |k: &[u8], v: &[u8]| {
                                    got.push((k.to_vec(), v.to_vec()))
                                })
                                .unwrap();
                            let want_n = (stop - start).min(limit);
                            assert_eq!(got_n, want_n, "{what}: handed over");
                            assert_eq!(
                                got,
                                raw_seq(start, start + want_n),
                                "{what}: the keys and values"
                            );
                            let at_rank = start + want_n;
                            assert_eq!(c.rank(), at_rank, "{what}: the cursor's rank after");
                            assert_eq!(
                                c.key(),
                                fx.blob.key_at(at_rank),
                                "{what}: the key under it"
                            );
                            // At the limit nothing is compared, the end
                            // of the window being the caller's to see.
                            let want_at = if want_n >= limit {
                                KeyOrder::Greater
                            } else {
                                at_want
                            };
                            assert_eq!(at, want_at, "{what}: how the key compares");
                            if at == KeyOrder::Equal {
                                // The overlay walk's turn at an unsealed key
                                // the partition holds too: the record under
                                // the cursor, the step past it, the walk on.
                                let mut want = Vec::new();
                                fx.blob
                                    .values_at(at_rank, |v| want.push(v.to_vec()))
                                    .unwrap();
                                let mut vals = Vec::new();
                                c.values(&mut |v: &[u8]| vals.push(v.to_vec())).unwrap();
                                assert_eq!(vals, want, "{what}: the values under the cursor");
                                c.advance();
                                let mut rest = Vec::new();
                                let (m, at2) = c
                                    .walk_until(None, usize::MAX, &mut |k: &[u8], v: &[u8]| {
                                        rest.push((k.to_vec(), v.to_vec()))
                                    })
                                    .unwrap();
                                assert_eq!(
                                    (m, at2),
                                    (n - at_rank - 1, KeyOrder::Greater),
                                    "{what}: the rest"
                                );
                                assert_eq!(
                                    rest,
                                    raw_seq(at_rank + 1, n),
                                    "{what}: the rest's keys"
                                );
                                assert_eq!((c.key(), c.rank()), (None, n));
                            }
                            assert!(!c.fell_back(), "{what}");
                        }
                    }
                }
            }
            assert!(!zfile.damaged());
            // Asked ahead once a cursor where the prefetch is on, and never
            // for a walk its first page satisfies.
            let on = fx.attach_opts(false, true, true).unwrap();
            let mut sink = |_: &[u8], _: &[u8]| {};
            let mut c = ZCursor::at(&fx.memo, &on, &fx.blob, 0);
            c.walk_until(None, 300, &mut sink).unwrap();
            c.walk_until(None, 300, &mut sink).unwrap();
            assert_eq!(c.prefetches, 1, "{name}: one ask a cursor");
            let mut c = ZCursor::at(&fx.memo, &on, &fx.blob, 0);
            c.walk_until(None, 1, &mut sink).unwrap();
            assert_eq!(c.prefetches, 0, "{name}: nothing past the page wanted");
            let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
            c.walk_until(None, 300, &mut sink).unwrap();
            assert_eq!(c.prefetches, 0, "{name}: the option off");
            // A page flipped under the walk: the rest is the raw form's, and
            // every key and value is still right.
            let good = std::fs::read(&fx.out).unwrap();
            let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
            let p = unsampled(t.n_pages as usize, 3);
            let mut flipped = good.clone();
            flipped[p * ZPAGE + 100] ^= 0x40;
            std::fs::write(&fx.out, &flipped).unwrap();
            let zfile = fx
                .attach()
                .expect("the sample does not reach the flipped page");
            let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
            let mut got = Vec::new();
            let (m, at) = c
                .walk_until(None, usize::MAX, &mut |k: &[u8], v: &[u8]| {
                    got.push((k.to_vec(), v.to_vec()))
                })
                .unwrap();
            assert_eq!((m, at), (n, KeyOrder::Greater), "{name}: flipped");
            assert_eq!(got, raw_seq(0, n), "{name}: flipped, every key and value");
            assert!(c.fell_back() && zfile.damaged(), "{name}: the flip was met");
            std::fs::write(&fx.out, &good).unwrap();
        }
    }

    /// The scan's prefetch: a walk that wants more keys than its first
    /// page holds asks for the pages after it, once, as many as the keys
    /// still wanted over the mean a page and sixteen at most, and they are
    /// resident soon after while the same walk without the prefetch
    /// leaves them cold; a walk of one key and a walk from the last page
    /// ask for nothing.
    #[test]
    fn a_scan_asks_for_the_pages_it_will_cross_once_and_they_come_in_behind_it() {
        let fx = Fixture::new("prefetch", suite(6000, 12), 1 << 20);
        assert!(matches!(fx.build(), Outcome::Built(_)));
        // Evicted before the attach: a fault on a mapped file maps the
        // cached pages around it as well (the kernel's fault-around, sixteen
        // pages), which no eviction takes from a mapping, so a form file
        // attached while its file was resident stayed resident whatever
        // was advised. Attached advised random, as the store advises a
        // form file, so that the kernel's readahead does not bring in behind
        // a walk what the prefetch is measured by.
        evict(&fx.out);
        let on = fx.attach_opts(true, true, true).unwrap();
        let off = fx.attach_opts(true, true, false).unwrap();
        let pages = on.pages();
        assert!(pages > 3 * ZPREFETCH_PAGES, "{pages} pages");
        assert_eq!(
            crate::bytes::os_page_size(),
            ZPAGE,
            "a page of the form is a page of the machine"
        );
        // The attach's sample faulted sixteen pages in through the
        // mapping, which no eviction takes back; the pages read are the
        // others: (resident, counted) over a run.
        let sampled: Vec<usize> = (0..ZSAMPLE_PAGES)
            .map(|k| k * (pages - 1) / (ZSAMPLE_PAGES - 1))
            .collect();
        let resident = |from: usize, n: usize| -> (usize, usize) {
            let m = MmapBytes::open(&fx.out).unwrap();
            let (mut res, mut all) = (0, 0);
            for p in from..from + n {
                if sampled.contains(&p) {
                    continue;
                }
                all += 1;
                res += m.resident_pages(p * ZPAGE, ZPAGE);
            }
            (res, all)
        };
        let mut sink = |_: &[u8], _: &[u8]| {};
        let (res, all) = resident(1, ZPREFETCH_PAGES);
        assert!(
            all >= ZPREFETCH_PAGES - 5 && res == 0,
            "{res} of {all} resident after the eviction"
        );
        // Without the prefetch a walk of one key from the first page leaves
        // the pages after it as the eviction left them.
        let z = off.scan_at(&fx.memo, &fx.blob, 0, 1, &mut sink).unwrap();
        assert_eq!((z.seen, z.prefetches), (1, 0));
        assert_eq!(resident(1, ZPREFETCH_PAGES).0, 0, "cold behind the walk");
        // With it, a walk of one key asks for nothing, a walk from the last
        // page asks for nothing, and a long walk asks once for the pages it
        // will cross, which come in behind it.
        let z = on.scan_at(&fx.memo, &fx.blob, 0, 1, &mut sink).unwrap();
        assert_eq!(
            (z.seen, z.prefetches),
            (1, 0),
            "one key: nothing past its page"
        );
        let last_first = on
            .fence
            .entry(on.fence.len() - 1, on.nkeys as u32)
            .unwrap()
            .first_rank as usize;
        let z = on
            .scan_at(&fx.memo, &fx.blob, last_first, usize::MAX, &mut sink)
            .unwrap();
        assert_eq!(z.prefetches, 0, "from the last page: nothing after it");
        assert_eq!(resident(1, ZPREFETCH_PAGES).0, 0, "still cold");
        let mean = on.nkeys / pages;
        let first = on.fence.entry(0, on.nkeys as u32).unwrap().count as usize;
        let want = 5 * mean;
        let z = on.scan_at(&fx.memo, &fx.blob, 0, want, &mut sink).unwrap();
        assert_eq!((z.seen, z.prefetches), (want, 1), "{z:?}");
        let k = (want - first).div_ceil(mean).min(ZPREFETCH_PAGES);
        let t = std::time::Instant::now();
        while resident(1, k).0 < resident(1, k).1 && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (res, all) = resident(1, k);
        assert_eq!(res, all, "the {k} pages asked for came in");
        // The walk touched the pages it crossed, the ones asked for or the
        // one after; past those, nothing came in.
        assert_eq!(resident(1 + k + 2, 3).0, 0, "and no more than them");
    }

    #[test]
    fn a_record_no_page_holds_among_the_pages_is_one_entry_and_the_pages_go_on() {
        // The pages before the record are in the temp file when the fill
        // meets it; the entry is pushed and the pages after it follow, so
        // the form file is whole and the temp name is gone.
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
        let zfile = fx.attach().unwrap();
        agree(
            &fx.memo,
            &zfile,
            &fx.blob,
            &fx.recs
                .iter()
                .filter(|r| r.0 != big)
                .cloned()
                .collect::<Vec<_>>(),
        );
        assert_eq!(zfile.count(&fx.memo, big), ZRead::NoPage);
        // A key in the entry's range that is not the record's is absent
        // from the fence alone, and the record's own key is the raw form's.
        let above = [&big[..], &[0xffu8]].concat();
        assert_eq!(
            zfile.count(&fx.memo, &above),
            ZRead::Served {
                n: 0,
                decoded: false
            }
        );
        assert_eq!(z_values(&fx.memo, &zfile, &fx.blob, big), None);
        // The entry's page number is no page, and the pages after it are
        // numbered on from the one before.
        let e = zfile.fence.entry_of(big).unwrap();
        assert_eq!(zfile.fence.page(e), Some(ZNOPAGE));
        assert_eq!(
            zfile.fence.page(e + 1),
            zfile.fence.page(e - 1).map(|p| p + 1)
        );
    }

    #[test]
    fn a_build_over_a_stale_temp_file_is_a_whole_form_file() {
        // A build a crash cut off leaves its temp name, and the next build of
        // the same name writes over it: from the first byte, and to the
        // length the form file is, or the file renamed into place carries what
        // the crashed build had after its end.
        let fx = Fixture::new("stale", suite(3000, 66), 256);
        std::fs::write(tmp_of(&fx.out), vec![0xA5u8; 4 << 20]).unwrap();
        assert!(matches!(fx.build(), Outcome::Built(_)));
        let zfile = fx
            .attach()
            .expect("a form file built over a stale temp file attaches");
        agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
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
    fn a_form_file_is_the_same_bytes_whatever_the_piece_and_the_window() {
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
    fn a_page_the_form_file_cannot_lend_is_refused_by_a_read_and_not_answered_absent() {
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
        fx.zfile();
        let bytes = std::fs::read(&fx.out).unwrap();
        // A form file fresh from its attach: no page is verified yet, so a read
        // that meets a page meets it in its first decode.
        let mut zfile = fx.attach_nomemo().unwrap();
        // Which pages are stored: entropy makes most of them so, and a group
        // that LZ4 does shorten is decoded into a buffer of the reader's own,
        // which is aligned whatever the source is.
        let stored = |zfile: &ZFile, k: &[u8]| {
            let p = zfile.fence.page(zfile.fence.entry_of(k).unwrap()).unwrap() as usize;
            let v = page_view(&bytes[p * ZPAGE..(p + 1) * ZPAGE]).unwrap();
            v.group(v.group_of_key(k)).unwrap().stored
        };
        let on_stored = fx.recs.iter().filter(|r| stored(&zfile, &r.0)).count();
        assert!(
            on_stored > 100 && fx.recs.len() - on_stored > 100,
            "{on_stored} of {} records on stored pages: the test wants both kinds",
            fx.recs.len()
        );
        // Misaligned by every skew that is not a multiple of four: a read
        // and a count of a key on a stored page both decline, and the form file
        // stays in service, since a source that cannot lend is no damage to
        // the file; a key on any other page is served as it is.
        let refused = |zfile: &ZFile, what: &str| {
            let (mut refusals, mut answers) = (0, 0);
            for (k, vals, _) in fx.recs.iter().step_by(11) {
                let mut got = Vec::new();
                let r = zfile
                    .read_all(&fx.memo, &fx.blob, k, &mut |v: &[u8]| got.push(v.to_vec()))
                    .unwrap();
                if stored(zfile, k) {
                    assert_eq!(
                        (r, got.len()),
                        (ZRead::Fallback, 0),
                        "{what}: read of {k:?}"
                    );
                    assert_eq!(
                        zfile.count(&fx.memo, k),
                        ZRead::Fallback,
                        "{what}: count of {k:?}"
                    );
                    refusals += 1;
                } else {
                    assert_eq!(&got, vals, "{what}: read of {k:?}");
                    assert_eq!(served(zfile.count(&fx.memo, k)), vals.len() as u64);
                    answers += 1;
                }
            }
            assert!(
                refusals > 10 && answers > 0,
                "{refusals} refused, {answers} answered"
            );
            assert!(!zfile.damaged() && zfile.bad_pages() == 0, "{what}");
        };
        for extra in [1usize, 2, 3, 5, 6, 7] {
            zfile.skewed = Some(file_at(&bytes, extra));
            refused(&zfile, &format!("first decode, skew {extra}"));
        }
        for p in 0..zfile.pages() {
            // A page of stored groups alone was refused at its first
            // decode, before the bit; one with a compressed group beside
            // them may have been verified whole through that group, its
            // stored groups walked through a copy, and its stored groups
            // are refused all the same by the alignment the source lacks.
            let v = page_view(&bytes[p * ZPAGE..(p + 1) * ZPAGE]).unwrap();
            let all_stored = (0..v.groups()).all(|j| v.group(j).unwrap().stored);
            assert!(
                !(all_stored && bit(&zfile.verified, p)),
                "page {p} was refused and is verified"
            );
        }
        // Aligned, from a buffer of the test's own: every key is served, a
        // stored page in place with nothing decoded.
        zfile.skewed = Some(file_at(&bytes, 0));
        for (k, vals, _) in &fx.recs {
            let mut got = Vec::new();
            let r = zfile
                .read_all(&fx.memo, &fx.blob, k, &mut |v: &[u8]| got.push(v.to_vec()))
                .unwrap();
            let want = ZRead::Served {
                n: vals.len() as u64,
                decoded: !stored(&zfile, k),
            };
            assert_eq!(r, want);
            assert_eq!(&got, vals);
        }
        // The pages are verified now, and a read that meets one in the
        // verified path is refused the same way.
        for extra in [1usize, 3, 6] {
            zfile.skewed = Some(file_at(&bytes, extra));
            refused(&zfile, &format!("verified, skew {extra}"));
        }
        // A source that has no such page at all is the form file's damage.
        zfile.skewed = Some((Vec::new(), 0));
        let k = &fx.recs[fx.recs.len() / 2].0;
        assert_eq!(zfile.count(&fx.memo, k), ZRead::Fallback);
        assert!(zfile.damaged() && zfile.bad_pages() == 1);
        assert_eq!(zfile.count(&fx.memo, k), ZRead::Out);
    }

    #[test]
    fn records_the_form_holds_no_page_for_are_the_raw_forms_and_pages_are_found_by_number() {
        // A fence entry that names no page (`ZNOPAGE`) is a run of records the
        // form was built without, and a read of them is the raw form's: the
        // form file says so and does not answer absent. The entries after it
        // are not at their own index in the file: the page of entry `i` is
        // the number the fence gives, and a reader that took the entry's
        // index for it would open another entry's page and find the damage
        // check, rightly, unwilling.
        let fx = Fixture::new("nopage", suite(4000, 81), 256);
        fx.zfile();
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
        let dropped = page_view(&good[k * ZPAGE..(k + 1) * ZPAGE]).unwrap().header;
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
        let zfile = fx
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
            let r = zfile
                .read_all(&fx.memo, &fx.blob, key, &mut |v: &[u8]| {
                    got.push(v.to_vec())
                })
                .unwrap();
            if (lo..hi).contains(&i) {
                assert_eq!((r, got.len()), (ZRead::NoPage, 0), "record {i}");
                assert_eq!(
                    zfile.count(&fx.memo, key),
                    ZRead::NoPage,
                    "count of record {i}"
                );
                declined += 1;
            } else {
                assert_eq!(&got, vals, "record {i}");
                assert!(matches!(r, ZRead::Served { .. }), "record {i}: {r:?}");
                served += 1;
            }
        }
        assert_eq!(declined, hi - lo);
        assert!(served > 3 * declined, "{served} served");
        assert!(!zfile.damaged() && zfile.bad_pages() == 0);
        // A key that falls in the run and is no record is the filter's to
        // answer, before the fence is asked: absent, no page touched, or
        // the fence's answer where the filter admits it by chance. The
        // fence's own answer for such a key is the raw form's, since the
        // entry covers more than one record: asked with the filter set
        // aside.
        let inside = [fx.recs[lo].0.as_slice(), &[0u8]].concat();
        let answer = zfile.count(&fx.memo, &inside);
        assert!(
            answer
                == ZRead::Served {
                    n: 0,
                    decoded: false
                }
                || answer == ZRead::NoPage,
            "{answer:?}"
        );
        let mut zfile = zfile;
        zfile.filter = None;
        assert_eq!(
            zfile.count(&fx.memo, &inside),
            ZRead::NoPage,
            "the fence's own answer for a key in a run of several records"
        );
    }
}
