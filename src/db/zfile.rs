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
    coded_key_into, coded_rec, decode_group_more, decode_group_with, decode_record, fill_page,
    filter_bytes, group_seek, open_group_partly_with, page_view, parse_page, record_group_seek,
    seek_group_partly_with, verify_page_with, view_trusted, walk_group, Dec, FenceEntry,
    FenceSearch, Fill, FillScratch, FillStats, GroupBuf, Images, KeyCodec, KeyShapeNo, NoPage,
    PageView, PartGroup, SegIdent, SegImages, StrideLayout, StrideTrailer, Trailer, ZError, ZFence,
    PART_STEP, STRIDE_KEY_MAX, STRIDE_MAGIC, STRIDE_W_MAX, ZCODING_BLOCK, ZCODING_RECORD,
    ZGROUP_BYTES, ZMAX_GROUPS, ZNOPAGE, ZPAGE, ZTRAILER,
};
use super::zpages::{
    stride_eps, stride_line, stride_page_crc, stride_row_crc, StrideSparse, KEY_KINDS,
    STRIDE_SAMPLE,
};
use super::zpages::{trailer_crc, trailer_magic};
use crate::blob::{Blob, BlobOptions, ZSource};
use crate::bytes::{Bytes, MmapBytes};
use crate::flatindex::{
    compact_fixed_at, compact_fixed_tomb_at, parse_record, parse_record_tail, rd_u32, record_key,
    record_key_next,
};
use crate::index::Ext;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering as KeyOrder;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Result, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicU8, Ordering};
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

/// Pages whose checksums an attach reads: the first, the last and the ones
/// evenly spaced between, sixteen faults a file, once.
const ZSAMPLE_PAGES: usize = 16;

/// The pages an attach samples of a file of `n` pages, ascending: every
/// page where there are no more than `ZSAMPLE_PAGES`, and otherwise the
/// first, the last and evenly spaced pages between, `ZSAMPLE_PAGES` of
/// them -- past that many pages the step between two is at least one, so
/// no page is named twice. The one choice of a sample both bodies' attaches
/// make (`ZFile::check_sample`, `ZFile::attach_stride`), and the one the
/// tests that forge a page around the sample read (`db::zpage_samples`).
pub(super) fn sample_pages(n: usize) -> Vec<usize> {
    if n <= ZSAMPLE_PAGES {
        (0..n).collect()
    } else {
        (0..ZSAMPLE_PAGES)
            .map(|k| k * (n - 1) / (ZSAMPLE_PAGES - 1))
            .collect()
    }
}

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

/// Whether the segments named `a` and `b` address one form file: the same
/// two fields (`zpg_fields`), never the same name. Every check of a build
/// in flight asks this, since a promotion re-opens a partition under a new
/// name with the fields kept, and two builds of one form file share its
/// temporary name: asked by name, a build held at its rename is not in
/// flight for the re-opened partition, so a second build of the file starts
/// beside it, its open truncates the temporary file the first may be
/// reading back, and the word the first leaves stands for the second's file.
pub(super) fn same_zpg(a: &str, b: &str) -> bool {
    zpg_fields(a).is_some_and(|f| zpg_fields(b) == Some(f))
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
    /// The next group's lines asked for at an open
    /// (`Options::zpages_bulk_ahead`).
    pub aheads: u64,
    /// The entries the walk took by stepping the fence, and the groups it
    /// opened without asking the memo again (`Options::zpages_bulk_steps`).
    pub stepped: u64,
    pub absent: u64,
    /// Whether the walk's first entry was the one the handle's last seek
    /// found (`ZMemo::seek_at`).
    pub from_seek: bool,
    pub end: ScanEnd,
}

impl ZScan {
    /// A walk that has visited nothing and is served so far.
    fn start() -> ZScan {
        ZScan {
            seen: 0,
            decoded: 0,
            inplace: 0,
            nopage: 0,
            prefetches: 0,
            aheads: 0,
            stepped: 0,
            absent: 0,
            from_seek: false,
            end: ScanEnd::Served,
        }
    }
}

/// What the bulk walk's decode in part of the group it ends inside did
/// (`ZFile::walk_last_part`): the records it walked, or declined for the
/// whole decode to take, or failed with the form file out of service.
enum PartWalk {
    Walked(usize),
    Declined,
    Failed,
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
///
/// The order of the variants is the tag's, and it is chosen so that each
/// question the block paths ask of the tag is one compare: `Stride` and
/// `Raw` are the tag's first two, so the step's question (`advance`) is a tag
/// under the second; the block groups the four after them and `None` last,
/// so the block groups' range (`resume`, `walk_inner`, `next_group`) is a
/// subtraction and a compare. Two variants matched as two arms are two
/// compares and two branches, which a stride arm beside the raw form's would
/// cost every block record and every block walk; and each test leaves the
/// variants it gathers to a function out of line, so that the compiler does
/// not split the one test back into a compare a variant. The block groups
/// are not at the tag's top: there `resume`'s two tests compile to one
/// branchless sequence, longer than the branch it replaces.
enum Group<'a> {
    /// A stride file's position, with the file's body: the position is the
    /// cursor's rank and nothing else, its key the codec's rendering of the
    /// rank's number in the cursor's buffer (`GroupBuf::stride_key`), made
    /// where the cursor is placed and stepped in place as it moves
    /// (`StrideBody::step_key`),
    /// and its value the slot at the rank, read after the slot's pages pass
    /// their checks (`StrideBody::values_at`). Nothing is decoded or opened.
    Stride(&'a StrideBody),
    Raw,
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
    None,
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

/// A fixed run's values handed to `f` with the key, as `Blob::scan_at`'s
/// fast path hands them: the one value without a division, the rest as
/// slices of one width, which `compact_fixed_at` has held the run to.
#[inline]
fn emit_fixed<F: FnMut(&[u8], &[u8])>(k: &[u8], run: &[u8], records: usize, f: &mut F) {
    if records == 1 {
        f(k, run);
        return;
    }
    let w = run.len() / records;
    let mut p = 0usize;
    while let Some(v) = run.get(p..p + w) {
        f(k, v);
        p += w;
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
/// The bytes of the next entry's page a cursor asks for at the last group
/// of a page (`Options::zpages_group_ahead`): the header, the directory and
/// the start of the first group, sixteen lines.
const ZGROUP_AHEAD: usize = 1024;

pub(super) struct ZCursor<'a> {
    memo: &'a ZMemo,
    zfile: &'a ZFile,
    /// The raw form through the form file's door (`SegBlob::for_zfile`),
    /// read by `ZCursor::raw` and nothing else, which counts the reads a
    /// stride file's cursor makes of it.
    door: &'a Blob<MmapBytes>,
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
    /// The view of page `pview_page`, held across the page's groups under
    /// `ZFile::cursor_view`; the open's own otherwise.
    pview: Option<PageView<'a>>,
    pview_page: usize,
    /// Groups decoded, groups read in place without a decode, runs of
    /// records no page holds met and prefetch asks made, for the handle's
    /// counters; the page views built and the groups asked for ahead.
    pub(super) decoded: u64,
    pub(super) inplace: u64,
    pub(super) nopage: u64,
    pub(super) prefetches: u64,
    pub(super) views: u64,
    pub(super) aheads: u64,
    /// The fence entries the cursor stepped to from the one before, no
    /// search of the fence made (`Options::zpages_cursor_steps`).
    pub(super) steps: u64,
    /// The next open is the walk's, decoded into `buf` under
    /// `ZFile::walk_owns`; taken at the open.
    owns_next: bool,
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
    /// The group in `buf` is decoded only so far (`ZMemo::walk_part`): what
    /// its decoder goes on from, and `Group::Buf` the length decoded. `None`
    /// for a group held whole, which every group is but a walk's own decode
    /// of one it wanted less than the whole of.
    part: Option<PartGroup<'a>>,
    /// The records the walk in progress wants of a group it opens;
    /// `usize::MAX` outside a walk, where a group is opened whole.
    want: usize,
    /// The records the reader expects to read from this cursor after the
    /// run in progress (`walk_run_ahead`): counted into what a group is
    /// wanted for, so that a run cut at a block's edge or at an unsealed
    /// key does not open in part a group the scan goes on through.
    ahead: usize,
}

impl<'a> ZCursor<'a> {
    fn new(memo: &'a ZMemo, zfile: &'a ZFile, raw: &'a Blob<MmapBytes>) -> ZCursor<'a> {
        ZCursor {
            memo,
            zfile,
            door: raw,
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
            pview: None,
            pview_page: usize::MAX,
            decoded: 0,
            inplace: 0,
            nopage: 0,
            prefetches: 0,
            views: 0,
            aheads: 0,
            steps: 0,
            owns_next: false,
            asked: false,
            fell: false,
            last: Vec::new(),
            last_ok: false,
            part: None,
            want: usize::MAX,
            ahead: 0,
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

    /// `at`, for a reader that will walk `want` records from the rank and
    /// may read no further: the group the rank falls in is opened as a
    /// walk's is, decoded only as far as the rank and those records where
    /// that is short of the group (`ZMemo::walk_part`). A run of a few
    /// records placed in the middle of a group -- a scan's last, in a
    /// block walked after one that was not walked through the form -- was
    /// a group decoded whole for them.
    pub(super) fn at_wanting(
        memo: &'a ZMemo,
        zfile: &'a ZFile,
        raw: &'a Blob<MmapBytes>,
        rank: usize,
        want: usize,
    ) -> ZCursor<'a> {
        let mut c = ZCursor::new(memo, zfile, raw);
        c.rank = rank;
        c.want = want;
        c.load();
        c.want = usize::MAX;
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
        if zfile.gate_out() {
            // A stride file's seek is the codec's arithmetic over the run,
            // the path `ZFile::seek` takes (`StrideBody::lower_bound`): no
            // page.
            return match zfile.stride().filter(|_| !zfile.damaged()) {
                Some(body) => ZCursor::stride_seek(memo, zfile, raw, body, key),
                None => Err(ZSeek::Out),
            };
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

    /// The raw form, for the cursor's every read of it: a run no page
    /// holds, a value that lies in a block, the walk after a fall. Over a
    /// stride file, where none of the first two exists, each read is
    /// counted on the file under debug assertions (`ZFile::raw_reads`): the
    /// door is not `Seg::raw`, and a stride position that answered from
    /// here where it had the slot was a read of the pages the form
    /// replaces that no count saw.
    #[inline]
    fn raw(&self) -> &'a Blob<MmapBytes> {
        #[cfg(debug_assertions)]
        if self.zfile.is_stride() {
            self.zfile.raw_reads.fetch_add(1, Ordering::Relaxed);
        }
        self.door
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
            Group::None | Group::Raw | Group::Coded(..) | Group::Stride(_) => &[],
        }
    }

    /// The walk from here to the partition's end is the raw form's.
    fn fall(&mut self) {
        self.fell = true;
        self.part = None;
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
        self.part = None;
        // Whether a decode goes into the memo: not for the walk's next
        // group under `ZFile::walk_owns` (`owns_next`, set by `next_group`
        // for this one open).
        let memoise = !std::mem::take(&mut self.owns_next);
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
        // A walk's first touch of a group (`Options::zpages_scan_admit`):
        // decoded into this cursor's own buffer, nothing put in the memo,
        // which takes a group at its second touch while the first's tag
        // stands (`Memo::admit`), as it does for a point read's. A scan
        // that reads a group once paid the memo's buffer, written
        // thousands of decodes ago, its `Arc` and the group it displaced,
        // for nothing; scans that start where others did find their groups
        // still held, since the groups read once no longer turn the slots
        // over. Where the walk's later groups are its own anyway
        // (`ZFile::walk_owns`) this decides only the first.
        let mut memoise = memoise;
        if memoise && self.memo.scan_admit && self.zfile.memo {
            if let Ok(index) = u8::try_from(j) {
                let (id, hashed) = (self.zfile.id, self.zfile.memo_hashed);
                if !self.memo.holds(id, entry.page, index, hashed)
                    && !self.memo.admit(id, entry.page, index)
                {
                    memoise = false;
                }
            }
        }
        // Where the group lies once opened: a stored group that is aligned
        // is parsed where it lies in the mapping, anything else is decoded
        // into the buffer, whose borrow by the open ends with the open. The
        // page's view, where the open needs one: under `ZFile::cursor_view`
        // held across the page's groups, built once a page and not once an
        // open, and not at all for a group the memo holds, which is asked
        // first; else this open's own, built before the memo is asked, the
        // shape before.
        let opened = if self.zfile.cursor_view {
            if self.pview_page != self.page {
                self.pview = None;
            }
            self.pview_page = self.page;
            match self.zfile.memo_first_hit(self.memo, entry, j) {
                Err(()) => None,
                Ok(Some(hit)) => Some(hit),
                Ok(None) => {
                    if self.pview.is_none() {
                        self.pview = self
                            .zfile
                            .page_bytes(self.page)
                            .and_then(|page| self.zfile.view(self.page, page).ok());
                        self.views += 1;
                    }
                    if !memoise {
                        if let Some(opened) = self.open_part(entry, j, first) {
                            return opened;
                        }
                    }
                    self.zfile.open_group_with(
                        self.memo,
                        entry,
                        j,
                        &mut self.buf,
                        self.pview.as_ref(),
                        self.zfile.inplace,
                        true,
                        false,
                        memoise,
                    )
                }
            }
        } else {
            self.pview = self
                .zfile
                .page_bytes(self.page)
                .and_then(|page| self.zfile.view(self.page, page).ok());
            self.pview_page = self.page;
            self.views += 1;
            self.zfile.open_group(
                self.memo,
                entry,
                j,
                &mut self.buf,
                self.pview.as_ref(),
                self.zfile.inplace,
                memoise,
            )
        };
        let Some((opened, gfirst, count, decoded)) = opened else {
            self.fall();
            return false;
        };
        let view = self.pview.as_ref();
        self.gstart = first.saturating_add(gfirst).min(self.zfile.nkeys);
        self.end = self.gstart.saturating_add(count).min(self.page_end);
        if decoded {
            self.decoded += 1;
        }
        // The memo's group is held by its `Rc`; a borrowed one is the
        // buffer's or the page's, and its borrow ends here.
        // A coded group read in place is the page's payload, taken from
        // the view, whose borrow is the mapping's and not the buffer's.
        let mut inplace = 0u64;
        let placed = match opened {
            Opened::Memo(g, n) => Some(Group::Memo(g, n)),
            Opened::Coded(_) => view.and_then(|v| v.payload(j)).map(|payload| {
                inplace += 1;
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
        self.inplace += inplace;
        let Some(group) = placed else {
            self.zfile.fail(self.page);
            self.fall();
            return false;
        };
        self.group = group;
        if self.zfile.group_ahead {
            self.ask_group_ahead(j);
        }
        true
    }

    /// The walk's own decode of group `j`, of which it wants fewer records
    /// than the group holds, begun and taken only as far as their share of
    /// the group's bytes and a record more (`ZMemo::walk_part`,
    /// `zpages::open_group_partly`); the walk decodes on where a record it
    /// reaches is not there yet (`more`). `None` where this open is not
    /// one for it, and the caller opens the group whole: the option off,
    /// the walk wanting the whole group or not a walk at all, a page not
    /// yet verified, a group stored or record-coded or held by the memo, or
    /// the other decoder in use. `Some(false)` for a payload that does not
    /// decode, the page failed and the cursor fallen.
    fn open_part(&mut self, entry: &FenceEntry<'_>, j: usize, first: usize) -> Option<bool> {
        if !self.memo.walk_part
            || self.want == usize::MAX
            || !self.zfile.own_decoder
            || !bit(&self.zfile.verified, self.page)
        {
            return None;
        }
        let view = self.pview.as_ref()?;
        let g = view.group(j)?;
        let (gfirst, count, ulen) = (usize::from(g.first), usize::from(g.count), g.ulen as usize);
        if g.stored || g.coded {
            return None;
        }
        // The records of the group the walk reads: the ones before the
        // position, which a sequential decoder decodes on its way, and the
        // ones the walk wants from it.
        let gstart = first.saturating_add(gfirst).min(self.zfile.nkeys);
        let reads = self.rank.saturating_sub(gstart).saturating_add(self.want);
        if reads >= count {
            return None;
        }
        // A group the memo holds is the memo's to hand over: asked already
        // where the memo is asked first (`ZFile::memo_first_hit`).
        if self.zfile.memo && !self.zfile.memo_first {
            if let Ok(index) = u8::try_from(j) {
                let (id, hashed) = (self.zfile.id, self.zfile.memo_hashed);
                if self.memo.holds(id, entry.page, index, hashed) {
                    return None;
                }
            }
        }
        let stop = (reads + 1).saturating_mul(ulen) / count;
        let tick = self.zfile.decode_tick();
        match open_group_partly_with(view, j, &mut self.buf, stop, self.zfile.dec()) {
            Ok(Some(part)) => {
                self.zfile.count_decode(tick);
                if self.zfile.memo {
                    self.memo.count_owned();
                }
                self.memo.count_walk_part(part.have(), part.ulen());
                self.gstart = gstart;
                self.end = self.gstart.saturating_add(count).min(self.page_end);
                self.decoded += 1;
                self.group = Group::Buf(part.have());
                self.part = (!part.done()).then_some(part);
                if self.zfile.group_ahead {
                    self.ask_group_ahead(j);
                }
                Some(true)
            }
            Ok(None) => None,
            Err(_) => {
                self.zfile.fail(self.page);
                self.fall();
                Some(false)
            }
        }
    }

    /// The cursor's group, decoded in part, decoded on to `stop` bytes or
    /// its end: true where it went on. False for a group held whole
    /// already, and for a payload that does not decode on, which leaves the
    /// group as far as it was decoded and no longer in part, so that the
    /// record its reader wanted fails its parse and is the damage it is.
    fn more(&mut self, stop: usize) -> bool {
        let Some(part) = self.part.as_mut() else {
            return false;
        };
        let before = part.have();
        match decode_group_more(part, &mut self.buf, stop) {
            Ok(n) => {
                let done = part.done();
                self.memo.count_walk_more(n - before);
                self.group = Group::Buf(n);
                if done {
                    self.part = None;
                }
                true
            }
            Err(_) => {
                self.part = None;
                false
            }
        }
    }

    /// The cursor's group decoded to its end where it was decoded in part:
    /// for the readers that walk a group by its bytes and not a run of
    /// records at a time.
    fn whole(&mut self) {
        while self.part.is_some() && self.more(usize::MAX) {}
    }

    /// The lines of the group the walk reaches after group `j` asked for
    /// while this one is walked (`ZFile::group_ahead`): the next group of
    /// the page by the directory, else the first `ZGROUP_AHEAD` bytes of the
    /// next entry's page -- its header, its directory and the start of its
    /// first group. A group's walk is lead enough to bring them.
    fn ask_group_ahead(&mut self, j: usize) {
        // A group the memo held opened with no view, and the next group's
        // lines are named by the directory: the view is built here, the
        // page's header line warm or not, and held for the groups after;
        // without it the ask fell to the next page's lines while the walk
        // had groups of this page left.
        if self.pview.is_none() {
            self.pview = self
                .zfile
                .page_bytes(self.page)
                .and_then(|page| self.zfile.view(self.page, page).ok());
            self.pview_page = self.page;
            self.views += 1;
        }
        let lines = match self.pview.as_ref().and_then(|v| v.payload(j + 1)) {
            Some(payload) => Some((payload.as_ptr(), payload.len())),
            None => self
                .zfile
                .fence
                .page(self.eidx + 1)
                .filter(|&p| p != ZNOPAGE)
                .and_then(|p| self.zfile.page_bytes(p as usize))
                .map(|page| (page.as_ptr(), ZGROUP_AHEAD.min(page.len()))),
        };
        if let Some((ptr, len)) = lines {
            super::prefetch_lines(ptr, len.min(ZPAGE));
            self.aheads += 1;
        }
    }

    /// The group of page `p` that holds the page's record `i`, by the
    /// directory: through the held view under `ZFile::cursor_view`, built
    /// here where the page is new to the cursor so that the open after
    /// reads the same one; else by a view of the file's own, the shape
    /// before. `None` where the page does not parse, the page out of
    /// service.
    fn group_index(&mut self, p: usize, i: usize) -> Option<usize> {
        if !self.zfile.cursor_view {
            return self.zfile.group_of_index(p, i);
        }
        if self.pview_page != p {
            self.pview = None;
        }
        self.pview_page = p;
        if self.pview.is_none() {
            self.pview = self
                .zfile
                .page_bytes(p)
                .filter(|_| p < self.zfile.n_pages)
                .and_then(|page| self.zfile.view(p, page).ok());
            self.views += 1;
        }
        match self.pview.as_ref() {
            Some(v) => Some(v.group_of_index(i)),
            None => {
                self.zfile.fail(p);
                None
            }
        }
    }

    /// The position at the end of a group: the next group of the same
    /// page where there is one, opened at its first record, else the
    /// entry the rank falls in.
    fn next_group(&mut self) {
        // A stride position's end is the run's (`stride_load`), so it comes
        // here only past the run, to the load that makes it the end; asked
        // as the block groups' range of the tag (`Group`), one test for the
        // three that are not.
        if self.rank < self.page_end
            && matches!(
                self.group,
                Group::Page(_) | Group::Buf(_) | Group::Memo(..) | Group::Coded(..)
            )
        {
            if let Some(entry) = self.zfile.fence.entry(self.eidx, self.zfile.nkeys as u32) {
                // The walk's next group: decoded into this cursor's own
                // buffer under `ZFile::walk_owns`, the memo asked and not
                // written.
                self.owns_next = self.zfile.walk_owns;
                if self.open_at(&entry, self.gidx + 1) && !matches!(self.group, Group::Raw) {
                    self.place();
                }
                return;
            }
        }
        // The next page's first group is the walk's as much as the next
        // group of this page.
        self.owns_next = self.zfile.walk_owns;
        // The entry after the one held is the rank's wherever the walk ran
        // off the end of an entry's records, which is every time it comes
        // here from inside a block file's run: taken by its index, and held
        // to the rank, where `load` searches the fence's ranks for it
        // (`Options::zpages_cursor_steps`). A walk of a hundred records
        // crosses two or three pages, and each crossing was a binary search
        // of the partition's whole fence for the entry one past the one in
        // hand. The search stands for every other position: past the last
        // key, a cursor fallen or a file whose gate is out, and an entry
        // that does not hold the rank -- an empty one, or a cursor whose
        // entry is not the one before the rank's.
        let stepped = self.memo.cursor_steps
            && self.rank < self.zfile.nkeys
            && !self.fell
            && !self.zfile.gate_out()
            && match self
                .zfile
                .fence
                .entry(self.eidx + 1, self.zfile.nkeys as u32)
            {
                Some(entry)
                    if self.rank >= entry.first_rank as usize
                        && self.rank - (entry.first_rank as usize) < entry.count as usize =>
                {
                    self.off = 0;
                    self.part = None;
                    self.steps += 1;
                    self.load_in(entry);
                    true
                }
                _ => false,
            };
        if !stepped {
            self.load();
        }
        self.owns_next = false;
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
        // A group decoded in part is decoded on until the record under the
        // position lies whole in it, a step at a time, as the point read's
        // partial seek goes on.
        while self.part.is_some() && record_key_next(self.group(), self.off).is_none() {
            if !self.more(self.off + PART_STEP) {
                break;
            }
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
        self.part = None;
        if self.rank >= self.zfile.nkeys {
            self.group = Group::None;
            self.end = self.zfile.nkeys;
            return;
        }
        if self.fell || self.zfile.gate_out() {
            match self
                .zfile
                .stride()
                .filter(|_| !self.fell && !self.zfile.damaged())
            {
                Some(body) => self.stride_load(body),
                None => self.fall(),
            }
            return;
        }
        let Some(entry) = self.zfile.entry_at_rank(self.rank) else {
            self.fall();
            return;
        };
        self.load_in(entry);
    }

    /// `load` from the entry `self.rank` falls in: the page's group by the
    /// directory, the group opened and the walk to the rank's record. The
    /// entry is the caller's to have found, by the fence's search (`load`)
    /// or by a step from the entry before (`next_group`), and holds the
    /// rank either way.
    fn load_in(&mut self, entry: FenceEntry<'a>) {
        debug_assert!(
            self.rank >= entry.first_rank as usize
                && self.rank - (entry.first_rank as usize) < entry.count as usize,
            "the entry holds the rank"
        );
        let j = if entry.page == ZNOPAGE {
            0
        } else {
            let i = self.rank - entry.first_rank as usize;
            match self.group_index(entry.page as usize, i) {
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
            // A group decoded in part is decoded on where the record to
            // step over does not lie whole in it yet.
            while self.part.is_some() && record_key_next(self.group(), self.off).is_none() {
                if !self.more(self.off + PART_STEP) {
                    break;
                }
            }
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
            Group::Raw => self.raw().key_at(self.rank),
            Group::Coded(..) => Some(self.keybuf.as_slice()),
            Group::Stride(_) => {
                let (at, len) = self.key_at;
                self.buf.bytes(at + len).get(at..at + len)
            }
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
            Group::Raw => self.raw().values_at(self.rank, &mut *f),
            Group::Stride(body) => self.stride_values(body, f),
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
                        self.raw(),
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
                    self.raw().values_at(self.rank, &mut *f)
                }
            }
            Group::Page(_) | Group::Buf(_) | Group::Memo(..) => {
                match parse_record(self.group(), self.off) {
                    Some((_, exts, tail, _)) => self.raw().read_exts(&exts, tail, &mut *f),
                    None => {
                        // The first decode walked every record of the group
                        // whole, so this is a group changed under the mapping:
                        // the page out of service and the raw form's values.
                        self.zfile.fail(self.page);
                        self.raw().values_at(self.rank, &mut *f)
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
            return;
        }
        // One test of the tag for a block group's record: `Stride` and `Raw`
        // are the tag's first two (`Group`), and the step out of line tells
        // them apart, so a block record's step pays no test of either.
        if matches!(self.group, Group::Stride(_) | Group::Raw) {
            self.step_off_block();
        } else {
            self.off = self.next;
            self.place();
        }
    }

    /// `advance`'s step where the group is not a block group: a stride
    /// position's key moved to the next rank's, a run the raw form reads
    /// nothing past the rank. Out of line, so that the step's one test of the
    /// tag is not split into a test a variant; a record of a run the raw
    /// form reads pays the call where it is stepped by `advance`, the merge
    /// path's step of a fallen cursor, and the walk over such a run steps by
    /// `raw_step` and pays none.
    #[inline(never)]
    fn step_off_block(&mut self) {
        if let Group::Stride(body) = self.group {
            self.stride_step(body);
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
    ///
    /// The reader expects to read `ahead` records more from this cursor
    /// after the run: a scan's runs end at a block's edge or at an
    /// unsealed key with the scan's limit still ahead. A group the run
    /// ends in is decoded as far as the run and those records reach, which
    /// for every group but the scan's last is the whole of it, in one call
    /// (`Options::zpages_walk_part`). The first shape wanted the run alone,
    /// and a scan's groups were begun in part at every such cut and taken
    /// on at the next run: 1.2 groups a scan begun in part in the scrambled
    /// E mix at ten million keys, at three quarters of their bytes.
    pub(super) fn walk_run_ahead<F: FnMut(&[u8], &[u8])>(
        &mut self,
        until: Option<&[u8]>,
        limit: usize,
        ahead: usize,
        f: &mut F,
    ) -> Result<(usize, KeyOrder)> {
        self.ahead = ahead;
        let walked = self.walk_impl(until, limit, f, true);
        self.ahead = 0;
        walked
    }

    fn walk_impl<F: FnMut(&[u8], &[u8])>(
        &mut self,
        until: Option<&[u8]>,
        limit: usize,
        f: &mut F,
        lazy_end: bool,
    ) -> Result<(usize, KeyOrder)> {
        // What the walk wants of a group it opens is what is left of its
        // limit, set before every open it makes; outside a walk an open
        // wants the whole group.
        self.want = limit.saturating_add(self.ahead);
        let walked = self.walk_inner(until, limit, f, lazy_end);
        self.want = usize::MAX;
        walked
    }

    fn walk_inner<F: FnMut(&[u8], &[u8])>(
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
            self.want = (limit - n).saturating_add(self.ahead);
            // The end, a stride position and a run the raw form reads, asked
            // as one test, a group outside the block groups' range of the tag
            // (`Group`): two tests here, or a test of the stride position's
            // own at the walk's head, would be a compare and a branch more on
            // every block walk.
            if !matches!(
                self.group,
                Group::Page(_) | Group::Buf(_) | Group::Memo(..) | Group::Coded(..)
            ) {
                let (walked, done) = self.walk_off_block(until, limit - n, f)?;
                n += walked;
                if let Some(order) = done {
                    return Ok((n, order));
                }
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
                let raw = self.raw();
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
                    self.want = (limit - n).saturating_add(self.ahead);
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
            // A group decoded in part: a record that does not parse is one
            // the decode has not reached, and the group is decoded on.
            let partial = self.part.is_some();
            let mut short = false;
            {
                let group = self.group();
                while n < limit && rank < end {
                    // The common record in place; any other shape through
                    // the parse.
                    if let Some((k, run, records, next)) = compact_fixed_at(group, off) {
                        if let Some(u) = until {
                            match k.cmp(u) {
                                KeyOrder::Less => {}
                                order => {
                                    stop = Some(order);
                                    break;
                                }
                            }
                        }
                        emit_fixed(k, run, records, f);
                        off = next;
                        rank += 1;
                        n += 1;
                        continue;
                    }
                    let Some((k, exts, tail, len)) = parse_record(group, off) else {
                        if partial {
                            short = true;
                        } else {
                            damaged = true;
                        }
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
                    self.raw().read_exts(&exts, tail, &mut |v: &[u8]| f(k, v))?;
                    off += len;
                    rank += 1;
                    n += 1;
                }
            }
            self.rank = rank;
            if short {
                // The records decoded are walked. The group is decoded on by
                // the share of its bytes the records still wanted of it
                // take, and a record more, or to its end where the walk
                // wants the rest; and walked from where it stands.
                self.off = off;
                let (left, wanted) = (end - rank, (limit - n).saturating_add(self.ahead));
                let to = match &self.part {
                    Some(part) if wanted < left => {
                        let count = (self.end - self.gstart).max(1);
                        let share = (wanted + 1).saturating_mul(part.ulen()) / count;
                        // A step at the least, where the records left are
                        // longer than the group's mean.
                        (off + share).max(part.have() + PART_STEP)
                    }
                    _ => usize::MAX,
                };
                self.more(to);
                continue;
            }
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
                    // `walk_run_ahead`'s end: the run ended at the group's last
                    // record, and the next group is the next read's to
                    // open (`resume`).
                    self.off = off;
                    return Ok((n, KeyOrder::Greater));
                }
                self.want = (limit - n).saturating_add(self.ahead);
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

    /// `walk_inner`'s walk where the group is not a block group, for at most
    /// `limit` records: how many it handed over, and the answer where the walk
    /// is done -- the end; a stride position's walk to the limit, `until` or
    /// the run's end (`stride_walk`); a run the raw form reads, a key at a time
    /// while the group stays that run, stopped by `until` or the raw form's end
    /// -- or `None` where the walk goes on in whatever group the cursor holds
    /// now: the limit reached, a block group opened past the run, or a stride
    /// position fallen. Out of line, so that the walk's one test of the tag is
    /// not split into a test a variant.
    #[inline(never)]
    fn walk_off_block<F: FnMut(&[u8], &[u8])>(
        &mut self,
        until: Option<&[u8]>,
        limit: usize,
        f: &mut F,
    ) -> Result<(usize, Option<KeyOrder>)> {
        match self.group {
            Group::None => Ok((0, Some(KeyOrder::Greater))),
            Group::Stride(body) => Ok(self.stride_walk(body, until, limit, f)),
            _ => {
                let mut n = 0usize;
                while n < limit && matches!(self.group, Group::Raw) {
                    self.want = (limit - n).saturating_add(self.ahead);
                    let Some(k) = self.raw().key_at(self.rank) else {
                        return Ok((n, Some(KeyOrder::Greater)));
                    };
                    if let Some(u) = until {
                        match k.cmp(u) {
                            KeyOrder::Less => {}
                            order => return Ok((n, Some(order))),
                        }
                    }
                    self.raw().values_at(self.rank, &mut |v: &[u8]| f(k, v))?;
                    n += 1;
                    self.raw_step();
                }
                Ok((n, None))
            }
        }
    }

    /// `advance` for a run the raw form reads, which is all of it there: the
    /// rank on, and the next group at the run's end. Inline, for the walk
    /// over such a run, a record a step, which `advance` would send through
    /// its step out of line (`step_off_block`).
    #[inline]
    fn raw_step(&mut self) {
        debug_assert!(matches!(self.group, Group::Raw));
        self.last_ok = false;
        self.rank += 1;
        if self.rank >= self.end {
            self.next_group();
        }
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
        if self.fell || self.zfile.gate_out() {
            // A stride file's cut is the seek's arithmetic, from any
            // position, forwards or back: nothing held to keep and no
            // record read.
            return match self
                .zfile
                .stride()
                .filter(|_| !self.fell && !self.zfile.damaged())
            {
                Some(body) => self.stride_skip_to(body, until),
                None => {
                    self.fall();
                    None
                }
            };
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
        debug_assert!(
            !matches!(self.group, Group::Stride(_)),
            "a stride position is moved to a key by the seek's arithmetic"
        );
        self.whole();
        loop {
            match self.group {
                Group::None => return KeyOrder::Greater,
                Group::Raw => {
                    if self.fell {
                        return KeyOrder::Greater;
                    }
                    let Some(k) = self.raw().key_at(self.rank) else {
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

// ---- the cursor's stride position -------------------------------------------
//
// A cursor over a stride file stands at a rank and holds nothing else
// (`Group::Stride`): its key is the rank's rendering, made where the cursor
// is placed and stepped in place as it moves, its value the rank's slot read
// after the slot's pages pass, and a seek, a move to a key and a walk's
// stop at `until` are the codec's arithmetic over the run
// (`StrideBody::lower_bound`), the path `ZFile::seek` takes. So every
// placing -- `at`, `at_wanting`, `unplaced` then `skip_to`, either way,
// `seek` -- is arithmetic, `resume` has no group to open, and the decode,
// view and ahead counts stay at nothing; the prefetch count moves only
// under the scan prefetch's gate, as the block walk's does. Each arm honours
// the file's being out of service before anything else, as the block paths
// do where they place (`load`, `seek`, `skip_to`), and so does each step and
// each value read at the position on its own, since a stride position holds
// no group to read on from: the rest is the raw form's from the first look
// that finds the file out. A walk in progress (`stride_walk`) looks once, at
// its start, and reads on, each page by its own check, as the block walk
// reads on in the group it holds when another handle takes the file out of
// service under it. A page whose check fails takes the file out of service
// (`ZFile::fail`), with nothing of the slot handed over. A walk that meets it
// stops there and falls, the rest of the walk the raw form's. A value read
// (`stride_values`) answers the raw form's value and moves nothing, as the
// block cursor answers a group that fails under it: it reads through `&self`,
// so the cursor falls at its next step or placing (`stride_step`, `load`,
// `skip_to`), each of which finds the file out first.
// The cursor's placings meet the stride body where the file's methods do, on
// the gate's branch (`ZCursor::seek`, `load`, `skip_to`), and each arm here is
// out of line, so that the block cursor's code is its own.
// The values it hands over are counted on the handle's slot line
// (`StrideCounts::cursor`, `StrideTally`), and never as a decode. They are
// the file's heat only where the store converts between the forms
// (`ZFile::stride_heat`, `count_stride_touch`), counted there, here and in
// the file's own read arms (above `ZFile::is_stride`) together, as the
// groups a block file would have opened; elsewhere its walks and values add
// nothing to `ZFile::opens16`, and nothing anywhere to a census by form:
// under the block cache this cursor is a covered scan's path, and a stride
// partition read through it reads cold to the hot-raw policy, which stands
// down beside the form.

impl<'a> ZCursor<'a> {
    /// The position at the rank in a stride file: its key rendered into
    /// the cursor's buffer (`StrideBody::key_at_into`), or the end past the
    /// run.
    #[inline(never)]
    fn stride_load(&mut self, body: &'a StrideBody) {
        self.off = 0;
        self.part = None;
        self.end = self.zfile.nkeys;
        self.page_end = self.zfile.nkeys;
        let (zfile, memo, rank) = (self.zfile, self.memo, self.rank as u64);
        let file = zfile.stride_bytes();
        let len = match body.key_at_into(file, rank, self.stride_key(), &mut |p| {
            zfile.stride_page(body, memo, p)
        }) {
            Ok(k) => k.map(<[u8]>::len),
            // A sparse form's page of numbers failed its check, which took
            // the file out of service: the rest is the raw form's.
            Err(_) => {
                self.fall();
                return;
            }
        };
        match len {
            Some(len) => {
                self.key_at = (0, len);
                self.group = Group::Stride(body);
            }
            None => self.group = Group::None,
        }
    }

    /// `seek` over a stride file in service: the cursor at the first key at
    /// or after `key`, by the seek's arithmetic, or, for a sparse form, its
    /// search of the numbers; `Fallback` where a page of them failed its
    /// check, the seek the raw form's.
    #[inline(never)]
    fn stride_seek(
        memo: &'a ZMemo,
        zfile: &'a ZFile,
        raw: &'a Blob<MmapBytes>,
        body: &'a StrideBody,
        key: &[u8],
    ) -> std::result::Result<ZCursor<'a>, ZSeek> {
        let mut c = ZCursor::new(memo, zfile, raw);
        let file = zfile.stride_bytes();
        let Ok((rank, _)) = body.lower_bound(file, key, &mut |p| zfile.stride_page(body, memo, p))
        else {
            return Err(ZSeek::Fallback);
        };
        c.rank = rank as usize;
        c.stride_load(body);
        if c.fell {
            return Err(ZSeek::Fallback);
        }
        Ok(c)
    }

    /// `skip_to` at a stride position: the cut of `until` by the seek's
    /// arithmetic, from any position, the cursor placed there.
    #[inline(never)]
    fn stride_skip_to(&mut self, body: &'a StrideBody, until: &[u8]) -> Option<KeyOrder> {
        let (zfile, memo) = (self.zfile, self.memo);
        let file = zfile.stride_bytes();
        let Ok((rank, at)) =
            body.lower_bound(file, until, &mut |p| zfile.stride_page(body, memo, p))
        else {
            // A sparse form's page of numbers failed its check: the cut is
            // the raw form's, as it is from a cursor fallen any other way.
            self.fall();
            return None;
        };
        self.rank = rank as usize;
        self.stride_load(body);
        (!self.fell).then_some(at)
    }

    /// `advance`'s step at a stride position, inside the run: the key moved
    /// to the next rank's in place, or the rest the raw form's where the
    /// file is out of service.
    #[inline(never)]
    fn stride_step(&mut self, body: &'a StrideBody) {
        if self.zfile.damaged() {
            self.fall();
            return;
        }
        // `advance` moved the rank before the step: the key is the key of
        // the rank the cursor now stands at.
        let (zfile, memo, rank, len) = (self.zfile, self.memo, self.rank as u64, self.key_at.1);
        let file = zfile.stride_bytes();
        let direct = memo.stride_direct;
        let stepped = body.step_key(
            file,
            rank,
            &mut self.stride_key()[..len],
            direct,
            &mut |p| zfile.stride_page(body, memo, p),
        );
        match stepped {
            Err(_) => self.fall(),
            Ok(by_field) => memo.count_stride_direct(u64::from(by_field)),
        }
    }

    /// The stride position's key, in the group buffer the cursor holds
    /// (`GroupBuf::stride_key`): the thread's, so a placing allocates
    /// nothing past the thread's first, and no field of the cursor's own,
    /// since every cursor -- a block file's, every cursor a store without
    /// the form places -- is made and moved by value, and a key buffer in
    /// the cursor was bytes each of those copied for a form it never read.
    fn stride_key(&mut self) -> &mut [u8; STRIDE_KEY_MAX] {
        self.buf.stride_key()
    }

    /// `values` at a stride position: the slot's one value after its
    /// pages' checks, counted; the raw form's values where the file is out
    /// of service or a page fails, the page and the file taken out first.
    #[inline(never)]
    fn stride_values<F: FnMut(&[u8])>(&self, body: &StrideBody, f: &mut F) -> Result<u64> {
        let zfile = self.zfile;
        if zfile.damaged() {
            return self.raw().values_at(self.rank, &mut *f);
        }
        let memo = self.memo;
        let file = zfile.stride_bytes();
        match body.values_at(file, self.rank as u64, &mut |p| {
            zfile.stride_page(body, memo, p)
        }) {
            StrideAt::Value(v) => {
                memo.count_stride_cursor(1);
                zfile.count_stride_touch_at(self.rank);
                f(v);
                Ok(1)
            }
            // A page whose check failed, which took it and the file out of
            // service (`ZFile::stride_page`); never `Absent` inside the
            // run, where the position is. The raw form's values.
            StrideAt::Bad(_) | StrideAt::Absent => self.raw().values_at(self.rank, &mut *f),
        }
    }

    /// The bulk walk at a stride position (`walk_until`, `walk_run_ahead`):
    /// the ranks from the position to the first of the limit, `until`'s
    /// rank -- taken once, by the seek's arithmetic -- and the run's end,
    /// each key stepped in place and each value handed over after its
    /// slot's pages pass; the cursor stands where it stopped. How many it
    /// handed over, and how the key it stands at compares with `until` as
    /// the block walk answers it: `Equal` for `until` itself, `Greater` for
    /// a key past it and at the limit or the end, where nothing is
    /// compared. `None` in place of the answer where the file was out of
    /// service or a page failed: the cursor has fallen, standing at the
    /// first rank not handed over, and the rest is the raw form's.
    #[inline(never)]
    fn stride_walk<F: FnMut(&[u8], &[u8])>(
        &mut self,
        body: &'a StrideBody,
        until: Option<&[u8]>,
        limit: usize,
        f: &mut F,
    ) -> (usize, Option<KeyOrder>) {
        let zfile = self.zfile;
        if zfile.damaged() {
            self.fall();
            return (0, None);
        }
        let (rank, end) = (self.rank, self.end);
        let cap = rank.saturating_add(limit).min(end);
        let (ur, uat) = match until {
            Some(u) => {
                let memo = self.memo;
                let cut = body.lower_bound(zfile.stride_bytes(), u, &mut |p| {
                    zfile.stride_page(body, memo, p)
                });
                let Ok((r, at)) = cut else {
                    // A sparse form's page of numbers failed its check
                    // before a key was handed over.
                    self.fall();
                    return (0, None);
                };
                (r as usize, at)
            }
            None => (end, KeyOrder::Greater),
        };
        // `until` stops the walk where its rank is short of the limit and
        // the end; a rank at or before the position stops it before a key.
        let by_until = ur < cap;
        let to = if by_until { ur } else { cap };
        if !self.asked {
            self.asked = true;
            if zfile
                .stride_ask_ahead(body, rank as u64, (cap - rank) as u64)
                .is_some()
            {
                self.prefetches += 1;
            }
        }
        let (memo, file, len) = (self.memo, zfile.stride_bytes(), self.key_at.1);
        let direct = memo.stride_direct;
        let key = &mut self.stride_key()[..len];
        let mut r = rank;
        let mut fell = false;
        // The keys stepped by their field, for the handle's count.
        let mut steps = 0u64;
        while r < to {
            match body.values_at(file, r as u64, &mut |p| zfile.stride_page(body, memo, p)) {
                StrideAt::Value(v) => f(key, v),
                // A page taken out of service by its check
                // (`ZFile::stride_page`); never `Absent`, every rank of the
                // walk being in the run.
                StrideAt::Bad(_) | StrideAt::Absent => {
                    fell = true;
                    break;
                }
            }
            r += 1;
            if r < end {
                match body.step_key(file, r as u64, key, direct, &mut |p| {
                    zfile.stride_page(body, memo, p)
                }) {
                    Ok(by_field) => steps += u64::from(by_field),
                    // The key of the rank the walk now stands at is a
                    // sparse form's page of numbers that failed: fallen
                    // there.
                    Err(_) => {
                        fell = true;
                        break;
                    }
                }
            }
        }
        let walked = r - rank;
        self.rank = r;
        if walked > 0 {
            memo.count_stride_cursor(walked as u64);
            zfile.count_stride_touch(walked as u64);
        }
        memo.count_stride_direct(steps);
        if fell {
            self.fall();
            return (walked, None);
        }
        if r >= end {
            self.load();
            return (walked, Some(KeyOrder::Greater));
        }
        let order = if by_until && ur >= rank {
            uat
        } else {
            KeyOrder::Greater
        };
        (walked, Some(order))
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
    own: Dec,
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
    own: Dec,
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
    verified_group_in(&view, j, buf, Dec::Flex)
}

/// `verified_group` over a view the caller took of the page: the group the
/// directory names decoded into `buf`, or parsed where it lies.
pub(super) fn verified_group_in<'a>(
    view: &PageView<'a>,
    j: usize,
    buf: &'a mut GroupBuf,
    own: Dec,
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
/// The cells a memo's slab holds past its slots and its free buffers
/// (`Options::zpages_memo_slab`): for the groups its slots have let go
/// that a read in flight still holds, a cursor's and a nested read's.
const ZMEMO_HELD: usize = 12;

/// The cell a memo's slab keeps a group in (`Options::zpages_memo_slab`),
/// for groups cut at `group_bytes` decoded bytes: the cut, which no group
/// of more than one record passes, and the room past its end the decoder's
/// flat loop runs into (`zlz4::FLAT_SLACK`). None where a page is one
/// group, which a cell that size would be most of.
pub(super) fn memo_cell_bytes(group_bytes: usize) -> Option<usize> {
    (group_bytes > 0 && group_bytes < ZPAGE).then(|| group_bytes + super::zlz4::FLAT_SLACK)
}

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
    /// alone.
    free: Vec<GroupBuf>,
    /// The cell a decoded group is kept in under
    /// `Options::zpages_memo_slab`, in bytes, or zero for buffers of the
    /// allocator's; and the slab of them, made at the first decode.
    slab_cell: usize,
    slab: Option<super::blocks::Slab>,
    hits: u64,
    misses: u64,
    /// E2's footprint (design 4.7): while the handle reads the compact form
    /// under the controller, one bit a page it touched, by form file, and
    /// the distinct pages so far; off and empty otherwise. A list and not
    /// a map: a store has a few dozen form files and a walk opens group
    /// after group of one, so the open looks at the file it touched last
    /// before it looks at the rest, where a map hashed the file's identity
    /// at every open -- a fiftieth of the threaded scan pass under the
    /// bound, by the profile of its threads.
    track: bool,
    touched: Vec<(u64, Vec<u64>)>,
    touched_last: usize,
    distinct: u64,
    /// The groups point reads decoded in part lately, a tag a slot
    /// (`Memo::admit`): made at the first ask.
    admit: Vec<u32>,
    /// Where the keys read through the memo lie (`Memo::key_at`): made at
    /// the first put.
    keys: Vec<KeyAt>,
    /// The point reads' counts since the last take (`ZMemo::take_point`):
    /// groups decoded in part, the bytes decoded of them and the bytes they
    /// hold, groups taken at a second touch, and reads the key table sent
    /// straight to their record.
    point: PointCounts,
}

/// The point reads' counts a handle's memo keeps (`ZMemo::take_point`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PointCounts {
    /// Groups a point read decoded only as far as its key
    /// (`Options::zpages_point_part`).
    pub(super) parts: u64,
    /// The bytes those decodes wrote, and the bytes the groups hold.
    pub(super) part_bytes: u64,
    pub(super) part_of: u64,
    /// Groups the memo took at their second touch.
    pub(super) admits: u64,
    /// Point reads the key table answered with their record's place
    /// (`Options::zpages_point_keys`).
    pub(super) key_hits: u64,
    /// Groups a walk decoded into its cursor's own buffer, each a decode
    /// the memo would have taken (`Options::zpages_scan_admit`,
    /// `Options::zpages_walk_owns_decodes`).
    pub(super) owned: u64,
    /// Of those, groups a walk decoded only as far as it read
    /// (`Options::zpages_walk_part`), the bytes those decodes wrote, and
    /// the bytes the groups hold.
    pub(super) walk_parts: u64,
    pub(super) walk_part_bytes: u64,
    pub(super) walk_part_of: u64,
    /// The times a group begun in part was decoded further: none where the
    /// first stop covered what the walk went on to read.
    pub(super) walk_mores: u64,
}

/// What only a stride file counts on a handle, kept off the block form's
/// counts (`PointCounts`), which every dispatch copies, zeroes and tests
/// whole: a word there is a word every block file's dispatch pays for
/// whether a stride file exists or not. A store's handle counts them
/// straight onto its slot's line (`StrideTally`, `ZMemo::tally_into`), so
/// no dispatch takes or tests anything of them -- a flag the stride arm
/// raised, tested at every dispatch's take, was two instructions on every
/// block file's read; a memo with no tally, a test's, keeps them here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct StrideCounts {
    /// Value pages checked against their word of the row at their first
    /// touch in this process (`ZFile::stride_page`): none for a file a
    /// build of this process made and the attach trusted
    /// (`StrideAttach::Trust`), one a page touched for a file found on disk.
    pub(super) checks: u64,
    /// Point reads a stride file answered (`ZFile::stride_read_all`).
    pub(super) reads: u64,
    /// Values a cursor's stride position handed over (`Group::Stride`): its
    /// walks' records and the values read at the position -- the block
    /// cache's reads of a stride partition, the merge path's walks over it
    /// and the covered walk of a scan over the partitions alone
    /// (`Reader::scan_partitions`), each through the cursor.
    pub(super) cursor: u64,
    /// Keys a walk of a sparse form stepped by writing the next key's field
    /// (`Options::zpages_stride_step_direct`): none without the option, and
    /// none over a whole run, whose keys step by one in place.
    pub(super) direct: u64,
}

/// `StrideCounts` on a handle's slot line in the reader table, which the
/// handle's thread alone writes, with a plain load and store, and the
/// store's counts sum (`Db::zpage_counts`).
#[derive(Debug)]
pub(super) struct StrideTally {
    pub(super) checks: AtomicU64,
    pub(super) reads: AtomicU64,
    pub(super) cursor: AtomicU64,
    pub(super) direct: AtomicU64,
}

impl StrideTally {
    pub(super) const fn new() -> StrideTally {
        StrideTally {
            checks: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            cursor: AtomicU64::new(0),
            direct: AtomicU64::new(0),
        }
    }

    fn add(c: &AtomicU64, n: u64) {
        c.store(c.load(Ordering::Relaxed) + n, Ordering::Relaxed);
    }
}

/// A memo's pointer to its handle's `StrideTally` (`ZMemo::tally_into`).
#[derive(Clone, Copy)]
struct TallyPtr(*const StrideTally);

// SAFETY: the pointer names atomics, which any thread may load and store,
// in the reader table of a `Shared` the memo's handle holds an `Arc` of,
// so it is valid wherever the handle, and the memo with it, is sent.
unsafe impl Send for TallyPtr {}

/// The slots of the tags of groups decoded in part (`Memo::admit`): sixty-
/// four kilobytes a handle, in the core's second-level cache beside the
/// read path, and enough that a group read again within a few tens of
/// thousands of reads finds its tag standing.
const ZADMIT_SLOTS: usize = 1 << 14;

/// The key table's slots (`Memo::key_at`), sixteen bytes each: a megabyte
/// a handle that reads the compact form by key.
const ZKEYS_SLOTS: usize = 1 << 16;

/// Where a key read through the memo lies: the form file's attach
/// identity, the page, the group in it and the record's offset in the
/// decoded group, with a byte of the key's hash to tell most other keys of
/// the slot apart before the memo is asked. `zfile` zero is an empty slot,
/// since an identity is never zero.
#[derive(Clone, Copy, Default)]
struct KeyAt {
    zfile: u64,
    page: u32,
    off: u16,
    index: u8,
    tag: u8,
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
            slab_cell: 0,
            slab: None,
            hits: 0,
            misses: 0,
            track: false,
            touched: Vec::new(),
            touched_last: 0,
            distinct: 0,
            admit: Vec::new(),
            keys: Vec::new(),
            point: PointCounts::default(),
        }
    }

    /// The Fibonacci mix of a group's number and its form file's identity
    /// that the hashed slot is cut from (`Memo::slot`), whole: the tag
    /// filter cuts its slot and its tag from other bits of it.
    fn group_hash(zfile: u64, page: u32, index: u8) -> u64 {
        let group = u64::from(page)
            .wrapping_mul(ZMAX_GROUPS as u64)
            .wrapping_add(u64::from(index));
        group
            .wrapping_add(zfile.wrapping_mul(0xD6E8_FEB8_6659_FD93))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }

    /// Whether the memo holds `(zfile, page, index)`, with nothing counted
    /// and nothing cloned: a point read's look before it decides how to
    /// open a group it may not want whole.
    fn holds(&self, zfile: u64, page: u32, index: u8, hashed: bool) -> bool {
        if self.slots.is_empty() {
            return false;
        }
        matches!(
            &self.slots[self.slot(zfile, page, index, hashed)],
            Some(e) if e.zfile == zfile && e.page == page && e.index == index
        )
    }

    /// Whether a point read that finds `(zfile, page, index)` out of the
    /// memo decodes it whole for the memo: `true` at the group's second
    /// such ask while its tag stands, `false` at the first, which leaves
    /// the tag. A group read once is decoded as far as its key into the
    /// read's own buffer and displaces nothing; one read again is worth
    /// its whole decode and a slot. A tag is thirty-one bits of the
    /// group's hash and a slot holds one, so another group's tag standing
    /// there is taken for this group's once in two thousand million asks,
    /// which admits a group a touch early, and a group whose slot another
    /// overwrote between its two touches is asked a third time.
    fn admit(&mut self, zfile: u64, page: u32, index: u8) -> bool {
        if self.admit.is_empty() {
            self.admit = vec![0u32; ZADMIT_SLOTS];
        }
        let h = Memo::group_hash(zfile, page, index);
        let i = (h >> 50) as usize & (ZADMIT_SLOTS - 1);
        let tag = ((h >> 18) as u32) | 1;
        if self.admit[i] == tag {
            self.admit[i] = 0;
            self.point.admits += 1;
            true
        } else {
            self.admit[i] = tag;
            false
        }
    }

    /// The key table's slot and tag for a key's hash.
    fn key_slot(h: u64) -> (usize, u8) {
        ((h >> 40) as usize & (ZKEYS_SLOTS - 1), (h >> 56) as u8)
    }

    /// Where the key of hash `h` was last read in form file `zfile`, where
    /// the table has it: the page, the group and the record's offset in
    /// the decoded group. A place, not an answer: the caller asks the memo
    /// for the group and compares the record's key there, so a slot
    /// another key of the same tag took sends a read to a record that is
    /// not its own and nothing more, and the place named is always a
    /// record's start in the group named, since a put names nothing else
    /// and a group's bytes do not change under an identity.
    fn key_at(&self, zfile: u64, h: u64) -> Option<(u32, u8, usize)> {
        if self.keys.is_empty() {
            return None;
        }
        let (i, tag) = Memo::key_slot(h);
        let k = &self.keys[i];
        (k.zfile == zfile && k.tag == tag).then_some((k.page, k.index, usize::from(k.off)))
    }

    /// The key of hash `h` read at `off` in the memoised group
    /// `(zfile, page, index)`, for the next read of it; an offset past
    /// what a slot holds is left out.
    fn key_put(&mut self, zfile: u64, h: u64, page: u32, index: u8, off: usize) {
        let Ok(off) = u16::try_from(off) else {
            return;
        };
        if self.keys.is_empty() {
            self.keys = vec![KeyAt::default(); ZKEYS_SLOTS];
        }
        let (i, tag) = Memo::key_slot(h);
        self.keys[i] = KeyAt {
            zfile,
            page,
            off,
            index,
            tag,
        };
    }

    /// A page touched, for the footprint; nothing while it is not kept.
    fn touch(&mut self, zfile: u64, page: u32, n_pages: usize) {
        if !self.track {
            return;
        }
        let words = n_pages.div_ceil(64).max(1);
        let at = match self.touched.get(self.touched_last) {
            Some((z, _)) if *z == zfile => self.touched_last,
            _ => match self.touched.iter().position(|(z, _)| *z == zfile) {
                Some(i) => i,
                None => {
                    self.touched.push((zfile, vec![0u64; words]));
                    self.touched.len() - 1
                }
            },
        };
        self.touched_last = at;
        let map = &mut self.touched[at].1;
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

    /// A buffer for a decode: a displaced entry's, or a new one -- a cell
    /// of the memo's slab where it keeps one (`Options::zpages_memo_slab`),
    /// which holds a cell for every slot and the few buffers in flight
    /// beside them, and a buffer of the allocator's where every cell is
    /// out, a read still holding the groups the slots let go.
    fn take_free(&mut self) -> GroupBuf {
        if let Some(b) = self.free.pop() {
            return b;
        }
        if self.slab_cell == 0 {
            return GroupBuf::default();
        }
        let (cell, cells) = (self.slab_cell, self.n + ZMEMO_FREE + ZMEMO_HELD);
        let slab = self
            .slab
            .get_or_insert_with(|| super::blocks::Slab::new(cell, cells));
        slab.take().map(GroupBuf::in_cell).unwrap_or_default()
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
        ((Memo::group_hash(zfile, page, index) >> 32) as usize) % self.n
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
    /// A point read's first touch of a block-coded group decodes it in
    /// part and the memo takes a group at its second
    /// (`Options::zpages_point_part`); the key table is kept and asked
    /// (`Options::zpages_point_keys`). The handle's, set where it is made.
    point_part: bool,
    point_keys: bool,
    /// A point read asks the form file's filter before its fence
    /// (`Options::zpages_point_filter`); on in a memo made without it.
    point_filter: bool,
    /// The filter's block asked for at the read's start and the fence
    /// searched before the filter is (`Options::zpages_filter_ahead`).
    filter_ahead: bool,
    /// A walk's first touch of a group is decoded into the cursor's own
    /// buffer and the memo takes it at its second
    /// (`Options::zpages_scan_admit`).
    scan_admit: bool,
    /// A walk's own decode of a group it wants less than the whole of goes
    /// only as far as the walk reads (`Options::zpages_walk_part`).
    walk_part: bool,
    /// The bulk walk's (`ZFile::scan_at`) first touch of a group is decoded
    /// into the thread's buffer and the memo takes it at its second, the
    /// cursor's rule (`Options::zpages_bulk_admit`); and it asks for the
    /// next group's lines at each open (`Options::zpages_bulk_ahead`).
    bulk_admit: bool,
    bulk_ahead: bool,
    /// The bulk walk's own decode of the group it ends inside goes only as
    /// far as it reads (`Options::zpages_bulk_part`).
    bulk_part: bool,
    /// The bulk walk's ask is made for the next group of the same page as
    /// well as for the next page's head (`Options::zpages_bulk_ahead_within`).
    bulk_within: bool,
    /// The bulk walk steps the fence to the next entry where it ran to the
    /// end of one, and asks the memo once a group (`Options::zpages_bulk_steps`).
    bulk_steps: bool,
    /// A cursor at the end of a fence entry's records takes the entry after
    /// it where the rank is its first, and searches the fence by rank only
    /// where it is not (`Options::zpages_cursor_steps`).
    cursor_steps: bool,
    /// A sparse stride form's walk steps a key by writing the next number's
    /// field over the key's (`Options::zpages_stride_step_direct`).
    stride_direct: bool,
    /// The form file and the fence entry this handle's last seek found
    /// (`ZFile::seek`), which a scan's walk starts in: under
    /// `Options::zpages_bulk_steps` the walk takes it where it covers the
    /// rank it starts at, and searches the fence otherwise.
    seek_at: Cell<Option<(u64, usize)>>,
    /// What a stride file counted since the last take (`StrideCounts`),
    /// for a memo with no tally.
    stride: Cell<StrideCounts>,
    /// The handle's slot line the stride counts go to (`tally_into`); null
    /// for a memo of no handle's, which keeps them in `stride`.
    tally: Cell<TallyPtr>,
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
            point_part: false,
            point_keys: false,
            point_filter: true,
            filter_ahead: false,
            scan_admit: false,
            walk_part: false,
            bulk_admit: false,
            bulk_ahead: false,
            bulk_part: false,
            bulk_within: false,
            bulk_steps: false,
            cursor_steps: false,
            stride_direct: false,
            seek_at: Cell::new(None),
            stride: Cell::new(StrideCounts::default()),
            tally: Cell::new(TallyPtr(std::ptr::null())),
        }
    }

    /// The memo keeping its decoded groups in the cells of a slab of its
    /// own, `cell` bytes each (`Options::zpages_memo_slab`,
    /// `memo_cell_bytes`); none leaves them the allocator's.
    pub(super) fn slab(self, cell: Option<usize>) -> ZMemo {
        self.memo.borrow_mut().slab_cell = cell.unwrap_or(0);
        self
    }

    /// The memo with a walk's admission set (`Options::zpages_scan_admit`),
    /// as `point` sets the point reads' options.
    pub(super) fn scans(mut self, admit: bool, part: bool) -> ZMemo {
        self.scan_admit = admit;
        self.walk_part = part;
        self
    }

    /// The memo with the bulk walk's three options set
    /// (`Options::zpages_bulk_admit`, `Options::zpages_bulk_ahead`,
    /// `Options::zpages_bulk_part`); a memo made without it has them off,
    /// the shape before them.
    pub(super) fn bulk(mut self, admit: bool, ahead: bool, part: bool) -> ZMemo {
        self.bulk_admit = admit;
        self.bulk_ahead = ahead;
        self.bulk_part = part;
        self
    }

    /// The memo with the bulk walk's ask made within a page as well
    /// (`Options::zpages_bulk_ahead_within`).
    pub(super) fn bulk_within(mut self, within: bool) -> ZMemo {
        self.bulk_within = within;
        self
    }

    /// The memo with the bulk walk's fence steps and single memo ask set
    /// (`Options::zpages_bulk_steps`).
    pub(super) fn bulk_steps(mut self, steps: bool) -> ZMemo {
        self.bulk_steps = steps;
        self
    }

    /// The memo with the cursor's step to the next fence entry set
    /// (`Options::zpages_cursor_steps`).
    pub(super) fn cursor_steps(mut self, steps: bool) -> ZMemo {
        self.cursor_steps = steps;
        self
    }

    /// The memo with a sparse stride walk's direct key step set
    /// (`Options::zpages_stride_step_direct`).
    pub(super) fn stride_direct(mut self, direct: bool) -> ZMemo {
        self.stride_direct = direct;
        self
    }

    /// The memo with the point reads' two options set
    /// (`Options::zpages_point_part`, `Options::zpages_point_keys`): a
    /// store's handles are made so, and a memo made without it has both
    /// off, the shape before them.
    pub(super) fn point(mut self, part: bool, keys: bool) -> ZMemo {
        self.point_part = part;
        self.point_keys = keys;
        self
    }

    /// The memo with the point reads' filter set
    /// (`Options::zpages_point_filter`).
    pub(super) fn point_filter(mut self, on: bool) -> ZMemo {
        self.point_filter = on;
        self
    }

    /// The memo with the filter asked ahead of the fence
    /// (`Options::zpages_filter_ahead`).
    pub(super) fn filter_ahead(mut self, on: bool) -> ZMemo {
        self.filter_ahead = on;
        self
    }

    /// The hits, the misses and the point reads' counts since the last
    /// take, in one borrow: the handle's take at each dispatch
    /// (`Reader::zmemo_take`).
    pub(super) fn take_all(&self) -> (u64, u64, PointCounts) {
        let mut m = self.memo.borrow_mut();
        let r = (m.hits, m.misses, m.point);
        m.hits = 0;
        m.misses = 0;
        m.point = PointCounts::default();
        r
    }

    /// The point reads' counts since the last take.
    #[cfg(test)]
    pub(super) fn take_point(&self) -> PointCounts {
        std::mem::take(&mut self.memo.borrow_mut().point)
    }

    fn holds(&self, zfile: u64, page: u32, index: u8, hashed: bool) -> bool {
        self.memo.borrow().holds(zfile, page, index, hashed)
    }

    fn admit(&self, zfile: u64, page: u32, index: u8) -> bool {
        self.memo.borrow_mut().admit(zfile, page, index)
    }

    fn count_owned(&self) {
        self.memo.borrow_mut().point.owned += 1;
    }

    /// The stride counts go to `t` from here on.
    ///
    /// # Safety
    ///
    /// `t` outlives this memo: a handle's slot in the reader table of the
    /// store whose `Shared` the handle holds, with the memo one of the
    /// handle's fields.
    pub(super) unsafe fn tally_into(&self, t: &StrideTally) {
        self.tally.set(TallyPtr(t));
    }

    /// One stride count, by `n`: onto the handle's slot line where the
    /// memo has one, into `stride` otherwise.
    fn count_stride(
        &self,
        n: u64,
        pick: fn(&StrideTally) -> &AtomicU64,
        cell: fn(&mut StrideCounts) -> &mut u64,
    ) {
        let t = self.tally.get().0;
        if t.is_null() {
            let mut c = self.stride.get();
            *cell(&mut c) += n;
            self.stride.set(c);
        } else {
            // SAFETY: `tally_into`'s contract: the slot outlives the memo.
            StrideTally::add(pick(unsafe { &*t }), n);
        }
    }

    fn count_stride_check(&self) {
        self.count_stride(1, |t| &t.checks, |c| &mut c.checks);
    }

    fn count_stride_read(&self) {
        self.count_stride(1, |t| &t.reads, |c| &mut c.reads);
    }

    fn count_stride_cursor(&self, n: u64) {
        self.count_stride(n, |t| &t.cursor, |c| &mut c.cursor);
    }

    /// `n` keys of a sparse form stepped by the field
    /// (`Options::zpages_stride_step_direct`); nothing counted for none.
    fn count_stride_direct(&self, n: u64) {
        if n > 0 {
            self.count_stride(n, |t| &t.direct, |c| &mut c.direct);
        }
    }

    /// What a stride file counted on a memo of no handle's since the last
    /// take, taken.
    #[cfg(test)]
    pub(super) fn take_stride(&self) -> StrideCounts {
        self.stride.take()
    }

    /// A walk's group begun in part: `len` of its `of` bytes decoded.
    fn count_walk_part(&self, len: usize, of: usize) {
        let mut m = self.memo.borrow_mut();
        m.point.walk_parts += 1;
        m.point.walk_part_bytes += len as u64;
        m.point.walk_part_of += of as u64;
    }

    /// `len` bytes more of a group begun in part decoded.
    fn count_walk_more(&self, len: usize) {
        let mut m = self.memo.borrow_mut();
        m.point.walk_part_bytes += len as u64;
        m.point.walk_mores += 1;
    }

    fn count_part(&self, len: usize, of: usize) {
        let mut m = self.memo.borrow_mut();
        m.point.parts += 1;
        m.point.part_bytes += len as u64;
        m.point.part_of += of as u64;
    }

    fn key_at(&self, zfile: u64, h: u64) -> Option<(u32, u8, usize)> {
        self.memo.borrow().key_at(zfile, h)
    }

    /// The memo's group for `(zfile, page, index)` and its bytes, nothing
    /// counted: the key table's read, which counts its own hit once the
    /// record there is the key's.
    fn peek(
        &self,
        zfile: u64,
        page: u32,
        index: u8,
        hashed: bool,
    ) -> Option<(Arc<GroupBuf>, usize)> {
        let m = self.memo.borrow();
        if m.slots.is_empty() {
            return None;
        }
        match &m.slots[m.slot(zfile, page, index, hashed)] {
            Some(e) if e.zfile == zfile && e.page == page && e.index == index => {
                Some((Arc::clone(&e.group), e.len))
            }
            _ => None,
        }
    }

    fn key_put(&self, zfile: u64, h: u64, page: u32, index: u8, off: usize) {
        self.memo.borrow_mut().key_put(zfile, h, page, index, off)
    }

    /// A read the key table sent to its record: a hit of the memo's, and
    /// of the table's.
    fn count_key_hit(&self) {
        let mut m = self.memo.borrow_mut();
        m.hits += 1;
        m.point.key_hits += 1;
    }

    /// A test's: a memo of `slots` slots under `cap` bytes.
    #[cfg(test)]
    fn with(slots: usize, cap: usize) -> ZMemo {
        ZMemo::of(Memo::new(slots, cap))
    }

    /// The hits and misses since the last take, for the handle's counters
    /// (`Reader::zpage_counts`: `memo_hits`, `memo_misses`).
    #[cfg(test)]
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
            m.touched = Vec::new();
            m.touched_last = 0;
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

/// `ZFile::gate`'s bits: a check failed somewhere in the file.
const GATE_DAMAGED: u8 = 1;
/// `ZFile::gate`'s bits: the file holds the stride body.
const GATE_STRIDE: u8 = 2;

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
    /// The pages `verified` and `bad` hold a bit for: `n_pages`, and with
    /// them a sparse stride form's pages of its keys' numbers
    /// (`StrideBody::checked_pages`).
    checked: usize,
    /// The hash every page's CRC is xored with, recomputed from the live
    /// segment and not taken from the file. A stride file's is its
    /// trailer's (`StrideTrailer::bind32`), the trailer having been held to
    /// the live segment before (`same_segment`), and the body keeps it too,
    /// since its own checks read it there (`StrideBody::page_ok`): this one
    /// is read by the block helpers on every page they check, each of which
    /// never sees a stride file, so it stays a plain field beside the body
    /// rather than a branch inside it.
    bind: u32,
    /// The block coding's groups decoded by the engine's own decoder
    /// (`Options::zpages_own_decoder`), set by the store after the attach
    /// as `inplace` is; `lz4_flex`'s where not.
    pub(super) own_decoder: bool,
    /// The engine's own decoder runs its flat loop ahead of its fast one
    /// (`Options::zpages_flat_decode`), set beside `own_decoder`; and runs
    /// it to the group's end (`Options::zpages_flat_slack`).
    pub(super) flat_decoder: bool,
    pub(super) flat_slack: bool,
    /// One bit per page: the first-decode checks passed in this process,
    /// over every group of it.
    verified: Box<[AtomicU64]>,
    /// One bit per page: a check failed.
    bad: Box<[AtomicU64]>,
    /// Whether the block form's paths serve this file, in the one byte each
    /// of them loads first: nothing set for a block file in service;
    /// `GATE_DAMAGED` once any check failed, the file out of service and the
    /// partition's reads the raw form's until it is dropped; `GATE_STRIDE`
    /// from the attach of a stride file, which no block path reads. Each read
    /// method's stride arm sits on the branch its block path takes for a file
    /// it does not serve (`ZFile::gate_out`), so a block file's read loads
    /// and tests the byte as it loaded and tested the damaged flag alone,
    /// and the stride body adds nothing to it -- an arm at the head of each
    /// method, a load of the body's tag and a branch, cost the bulk scan
    /// through the block cache a sixtieth of its instructions in the codegen
    /// around it, and a point read and a count their branch and its spills.
    gate: AtomicU8,
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
    /// The ranks a block form's group of this stride file's partition would
    /// hold, `ZGROUP_BYTES` of values, as a shift: the power of two at or
    /// under the count, so that a walk by the key counts by a mask and
    /// divides nothing; zero for a block file. What a stride file's reads
    /// are counted by (`count_stride_touch`), so that its heat is in the
    /// unit a block file's decodes are: the groups the block form would
    /// have opened for the same reads.
    touch_shift: u8,
    /// Whether a stride file's reads are counted as its heat
    /// (`count_stride_touch`): set by the policy that reads the heat, at
    /// its first look at the file (`ask_stride_heat`,
    /// `Options::zpages_stride_convert`), and by nothing else, so that a
    /// stride file's read pays the count only where a policy reads it and
    /// no attach of one has to be told. Written once and read by every
    /// read, which shares the line as readers.
    stride_heat: std::sync::atomic::AtomicBool,
    /// Reads of the raw form a cursor over this file made through its door
    /// while the file is a stride file, under debug assertions
    /// (`ZCursor::raw`). A stride file holds every value in a slot and
    /// none in a block, so every read through the door there is a record
    /// read -- a fall's, or a position that answered from the raw form
    /// where it had the slot -- and the door is not `Seg::raw`, whose
    /// touches are the only other count: a cursor that served the raw
    /// form's records passed every test that held the store to no touch.
    /// `Reader::raw_touches` sums it beside them.
    raw_reads: AtomicU64,
    /// The counted decodes are timed (`Options::zpages_recode`).
    pub(super) time_decodes: bool,
    /// The sampled pages' groups are record-coded ([`ZCODING_RECORD`]):
    /// the coding this form file was built in, read from its own
    /// directories at the attach.
    coded: bool,
    /// This attach's identity, which the memo keys its groups by.
    id: u64,
    /// The body this attach read (`ZBody`): the block trailer it accepted,
    /// or the stride form's body, read by the stride arm each read method
    /// begins with (`ZFile::stride`) and by no block-only helper, each of
    /// which holds that the file is not a stride file.
    body: ZBody,
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
    /// Whether the cursor holds a page's view across its groups and asks
    /// the memo before any view (`Options::zpages_cursor_view`), and
    /// whether it asks for the next group's lines at an open
    /// (`Options::zpages_group_ahead`, `ZCursor::ask_group_ahead`).
    pub(super) cursor_view: bool,
    pub(super) group_ahead: bool,
    /// Whether a cursor's walk decodes the groups after its first into its
    /// own buffer, the memo asked and not written
    /// (`Options::zpages_walk_owns_decodes`).
    pub(super) walk_owns: bool,
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

/// What an attach takes of a stride file under a partition's name
/// (`Options::zpages_stride`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StrideAttach {
    /// Refused as another engine's file (`Reject::Identity`), as an engine
    /// from before the form refuses it at the trailer's magic: the store does
    /// not take the stride form, and the partition is built again in the
    /// block coding.
    Refuse,
    /// Attached after its checks, each value page checked against its word
    /// of the row at its first touch: a file found on disk.
    Check,
    /// Attached with every value page marked verified where the file under
    /// the name has a trailer of this CRC word -- the one a build of this
    /// process wrote and read back every rank of from the device
    /// (`Built::left_seal`), the row's CRC in it -- and as under `Check`
    /// where it has another.
    Trust(u32),
}

/// What an attach read of a form file, either body's, for `ZFile::assemble`.
struct Assembly {
    map: MmapBytes,
    file: Option<std::fs::File>,
    advise: bool,
    fence: ZFence,
    filter: Option<super::BlockedBloom>,
    nkeys: usize,
    n_pages: usize,
    bind: u32,
    body: ZBody,
}

/// The body a form file holds, one of the two, as its attach read it: the
/// block body's trailer, whose fence and filter are the file's own fields;
/// or the stride form's body (`zpages::StrideTrailer`), its trailer, sizes
/// and row, beside which the fence is empty, there is no filter, and the
/// file's page count and bitmaps are over the value pages. One field for
/// the two, so that a stride file holds no block trailer at a default and a
/// block file no stride body.
enum ZBody {
    Block(Trailer),
    Stride(Box<StrideBody>),
}

/// A stride file's body as its attach accepted it, and whether its pages
/// are trusted (`StrideAttach::Trust`).
struct StrideAttached {
    body: StrideBody,
    trusted: bool,
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
    /// The last page's magic says which body the file holds, read after the
    /// advice and before anything else of the file: a stride file
    /// (`zpages::STRIDE_MAGIC`) is refused as another engine's where
    /// `stride` refuses the form and is otherwise the stride form's attach
    /// (`ZFile::attach_stride`); every other file is the block form's,
    /// whose trailer's decode refuses one of neither kind at its magic.
    ///
    /// `advise` maps the file `MADV_RANDOM`, before its own sampled reads.
    /// `fence` is how the fence is searched: over its keys, over its words
    /// laid flat (`ZFence::index_words`, `Options::zpages_fence_words`) or
    /// through the tree over them (`ZFence::index`,
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
        fence: FenceSearch,
        memo_ahead: bool,
        stride: &StrideAttach,
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
        let last = map
            .slice_at(len - ZTRAILER as u64, ZTRAILER)
            .ok_or_else(|| damage(ZError::Length))?;
        // The last page's magic, before anything else of it is read: which
        // body the file holds. The block form's goes on below as it always
        // has; anything that is neither kind's is the block trailer's to
        // refuse, as damage, at its magic.
        if trailer_magic(last) == Some(STRIDE_MAGIC) {
            if *stride == StrideAttach::Refuse {
                return Err(Reject::Identity(
                    "a stride form file, which the store does not take".into(),
                ));
            }
            let StrideAttached {
                body,
                trusted: trusted_all,
            } = ZFile::attach_stride(&map, blob, id, end_seq, stride)?;
            let n_pages = body.pages();
            let nkeys = body.keys() as usize;
            let zfile = ZFile::assemble(
                Assembly {
                    map,
                    file,
                    advise,
                    fence: ZFence::new(),
                    filter: None,
                    nkeys,
                    n_pages,
                    bind: body.trailer().bind32(),
                    body: ZBody::Stride(Box::new(body)),
                },
                memo,
                prefetch,
                trusted,
                memo_first,
                memo_ahead,
            );
            if trusted_all {
                for p in 0..zfile.checked {
                    set_bit(&zfile.verified, p);
                }
            }
            return Ok(zfile);
        }
        let t = Trailer::decode(last).map_err(damage)?;
        if t.expected_file_len() != Some(len) {
            return Err(damage(ZError::Trailer(
                "the file is not the length its trailer says",
            )));
        }
        let facts = blob.z_source_facts().ok_or_else(|| {
            Reject::Identity("the segment has no checksum row to be bound to".into())
        })?;
        same_segment(&t.seg_ident(), &facts, id, end_seq)
            .map_err(|why| Reject::Identity(why.into()))?;
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
        let fence_search = fence;
        let (mut fence, filter) = ZFence::decode_resident(res, &t).map_err(damage)?;
        match fence_search {
            FenceSearch::Keys => {}
            FenceSearch::Words => fence.index_words(),
            FenceSearch::Tree => fence.index(),
        }
        let filter = if t.filter {
            Some(engine_filter(&t, filter, &fence, nkeys)?)
        } else {
            None
        };
        let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
        let mut zfile = ZFile::assemble(
            Assembly {
                map,
                file,
                advise,
                fence,
                filter,
                nkeys,
                n_pages,
                bind,
                body: ZBody::Block(t),
            },
            memo,
            prefetch,
            trusted,
            memo_first,
            memo_ahead,
        );
        zfile.coded = zfile.check_sample().map_err(damage)?;
        Ok(zfile)
    }

    /// An attached form file of either body from what its attach read, the
    /// one place its fields are listed: every block-coding and memo setting
    /// at what the store's options and the store's sets after the attach
    /// start from, nothing verified, nothing counted.
    fn assemble(
        a: Assembly,
        memo: MemoShape,
        prefetch: bool,
        trusted: bool,
        memo_first: bool,
        memo_ahead: bool,
    ) -> ZFile {
        // A bit a page a read checks at its first touch: the pages that
        // hold the records, and with them a sparse stride form's pages of
        // its keys' numbers.
        let checked = match &a.body {
            ZBody::Block(_) => a.n_pages,
            ZBody::Stride(body) => body.checked_pages(),
        };
        let words = checked.div_ceil(64);
        ZFile {
            map: a.map,
            file: a.file,
            advised: a.advise,
            fence: a.fence,
            filter: a.filter,
            nkeys: a.nkeys,
            n_pages: a.n_pages,
            checked,
            bind: a.bind,
            own_decoder: false,
            flat_decoder: false,
            flat_slack: false,
            verified: (0..words).map(|_| AtomicU64::new(0)).collect(),
            bad: (0..words).map(|_| AtomicU64::new(0)).collect(),
            gate: AtomicU8::new(match a.body {
                ZBody::Block(_) => 0,
                ZBody::Stride(_) => GATE_STRIDE,
            }),
            decodes16: AtomicU64::new(0),
            decode_ns16: AtomicU64::new(0),
            opens16: AtomicU64::new(0),
            touch_shift: match &a.body {
                ZBody::Block(_) => 0,
                ZBody::Stride(b) => (super::zpages::ZGROUP_BYTES as u32 / b.trailer().w.max(1))
                    .max(1)
                    .ilog2() as u8,
            },
            raw_reads: AtomicU64::new(0),
            stride_heat: std::sync::atomic::AtomicBool::new(false),
            time_decodes: false,
            coded: false,
            id: NEXT_ZSIDE_ID.fetch_add(1, Ordering::Relaxed),
            body: a.body,
            memo: memo != MemoShape::Off,
            memo_hashed: memo == MemoShape::Hashed,
            trusted,
            memo_first,
            inplace: true,
            cursor_view: false,
            group_ahead: false,
            walk_owns: false,
            memo_ahead,
            prefetch,
            #[cfg(test)]
            skewed: None,
        }
    }

    /// The stride form's attach, from the file's mapping, its length whole
    /// pages and its last page the stride trailer's magic: the checks, in
    /// order, each a refusal of its own kind -- the trailer and the length
    /// its sizes give (damage); the segment it names the live one
    /// (`same_segment`, identity); the row's CRC and the zeros after it
    /// (damage), the row then copied into the heap (`StrideBody::open` and
    /// `StrideBody::with_row`); the first and the last key the codec renders
    /// the live segment's own first and last keys (identity); and the first,
    /// the last and fourteen evenly spaced value pages each against its
    /// word of the row (damage) and the slot each holds the start of --
    /// where none starts on it, the slot its first byte is in -- against the
    /// live segment's record at that rank, key and value (identity). That
    /// last is the one check behind the checksum a file of values alone can
    /// make: a page forged with a good CRC is refused here where it is
    /// sampled and served where it is not, the block form's limit too. A
    /// value page beyond the sample is checked against its word at its
    /// first touch (`ZFile::stride_page`).
    ///
    /// Where `stride` trusts a trailer and the file's is that trailer, the
    /// file is the one a build of this process wrote and read back, every
    /// rank of it against its records, before its name: the row is the
    /// build's by the CRC the trailer carries, and the endpoints and the
    /// sample, which the read-back covered, are not read again -- they
    /// would be a raw page each of records the install is about to give
    /// back.
    fn attach_stride(
        map: &MmapBytes,
        blob: &Blob<MmapBytes>,
        id: u64,
        end_seq: u64,
        stride: &StrideAttach,
    ) -> std::result::Result<StrideAttached, Reject> {
        let damage = |e: ZError| Reject::Damage(e.to_string());
        let identity = |why: &str| Reject::Identity(why.into());
        let bytes: &[u8] = &map.0;
        let (trailer, layout) = StrideBody::open(bytes).map_err(damage)?;
        let facts = blob
            .z_source_facts()
            .ok_or_else(|| identity("the segment has no checksum row to be bound to"))?;
        same_segment(&trailer.seg_ident(), &facts, id, end_seq).map_err(identity)?;
        let body = StrideBody::with_row(bytes, trailer, layout).map_err(damage)?;
        if let StrideAttach::Trust(seal) = *stride {
            if trailer_crc(&bytes[bytes.len() - ZTRAILER..]) == Some(seal) {
                return Ok(StrideAttached {
                    body,
                    trusted: true,
                });
            }
        }
        let n = body.keys();
        let mut kbuf = [0u8; STRIDE_KEY_MAX];
        // A rank's key as the form has it: rendered from the run's first
        // number for a whole run, and for a sparse form from the number its
        // page holds, the page checked first and its failure damage.
        let key_of = |rank: u64, kbuf: &mut [u8; STRIDE_KEY_MAX]| {
            body.key_at_into(bytes, rank, kbuf, &mut |q| body.page_ok(bytes, q))
                .map(|k| k.map(<[u8]>::to_vec))
                .map_err(|_| damage(ZError::PageCrc))
        };
        // Both sides answer at either end: the layout refuses a run of no
        // keys and `same_segment` held the count to the segment's, so the
        // compare is of two keys and never of two absences.
        for (rank, which) in [(0, "first"), (n - 1, "last")] {
            if key_of(rank, &mut kbuf)?.as_deref() != blob.key_at(rank as usize) {
                return Err(Reject::Identity(format!(
                    "the stride form's {which} key is not the segment's"
                )));
            }
        }
        let mut raw = Vec::new();
        for p in sample_pages(body.pages()) {
            let p = p as u32;
            let rank = body.rank_on_page(p);
            // Every page the slot spans, the sampled one among them, against
            // its word of the row before the slot is read.
            let value = match body.values_at(bytes, rank, &mut |q| body.page_ok(bytes, q)) {
                StrideAt::Value(value) => value,
                StrideAt::Absent | StrideAt::Bad(_) => return Err(damage(ZError::PageCrc)),
            };
            raw.clear();
            let got = blob
                .values_at(rank as usize, &mut |v: &[u8]| raw.push(v.to_vec()))
                .map_err(|e| Reject::Damage(format!("the segment's record reads: {e}")))?;
            if got != 1 || raw[0] != value {
                return Err(identity(
                    "a sampled value page's slot is not the segment's value at its rank",
                ));
            }
            let key = key_of(rank, &mut kbuf)?;
            if key.as_deref() != blob.key_at(rank as usize) {
                return Err(identity(
                    "a sampled value page's slot is not the segment's key at its rank",
                ));
            }
        }
        Ok(StrideAttached {
            body,
            trusted: false,
        })
    }

    /// The first, the last and fourteen evenly spaced pages through
    /// `parse_page` -- the CRC under this form file's `bind32`, the tag, the
    /// header, the directory, the padding -- and their rank and count
    /// against the fence entry that names each. Every page where there are no more than
    /// sixteen. These are not first decodes: nothing is decoded and no page
    /// is marked verified. Whether the first group of a sampled page is
    /// record-coded, which is the coding the file was built in.
    fn check_sample(&self) -> std::result::Result<bool, ZError> {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        let want = sample_pages(self.n_pages);
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
        if p < self.checked {
            set_bit(&self.bad, p);
        }
        self.gate.fetch_or(GATE_DAMAGED, Ordering::Relaxed);
    }

    /// The file's gate (`ZFile::gate`): zero for a block file in service.
    #[inline]
    fn gate_word(&self) -> u8 {
        self.gate.load(Ordering::Relaxed)
    }

    /// Whether a block path leaves this file: a stride file, or a file out
    /// of service. The one test each block path makes before anything else,
    /// whose branch the stride arms sit on.
    #[inline]
    fn gate_out(&self) -> bool {
        self.gate_word() != 0
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
    #[allow(clippy::too_many_arguments)]
    fn open_group<'a>(
        &'a self,
        memo: &ZMemo,
        entry: &FenceEntry<'_>,
        j: usize,
        buf: &'a mut GroupBuf,
        view: Option<&PageView<'a>>,
        inplace: bool,
        memoise: bool,
    ) -> Option<(Opened<'a>, usize, usize, bool)> {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        self.open_group_with(memo, entry, j, buf, view, inplace, false, false, memoise)
    }

    /// The memo's group for group `j` of `entry`'s page, asked before any
    /// view is built where the page is one this process has verified
    /// (`Options::zpages_memo_first`), the page touched as an open touches
    /// it: `Ok(Some)` on a hit, `Ok(None)` where the memo does not hold it
    /// or is not asked first, and the caller opens through
    /// `open_group_with` with `after_memo`; `Err` where the page or the
    /// group is none the file has, the page failed. For the cursor under
    /// `Options::zpages_cursor_view`, whose view is held and built only for
    /// an open that reads the page.
    fn memo_first_hit<'a>(
        &'a self,
        memo: &ZMemo,
        entry: &FenceEntry<'_>,
        j: usize,
    ) -> std::result::Result<Option<(Opened<'a>, usize, usize, bool)>, ()> {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        let p = entry.page as usize;
        if self.page_bytes(p).filter(|_| p < self.n_pages).is_none() {
            self.fail(p);
            return Err(());
        }
        memo.touch(self.id, entry.page, self.n_pages);
        let Ok(index) = u8::try_from(j) else {
            self.fail(p);
            return Err(());
        };
        if self.memo_first && self.memo && bit(&self.verified, p) {
            if let Some((g, n, first, count)) =
                memo.get(self.id, entry.page, index, self.memo_hashed)
            {
                return Ok(Some((
                    Opened::Memo(g, n),
                    usize::from(first),
                    usize::from(count),
                    false,
                )));
            }
        }
        Ok(None)
    }

    /// `open_group`; with `after_memo`, for a caller that asked the memo
    /// first itself (`memo_first_hit`) and touched the page, neither again;
    /// with `absent`, for a caller that has just found the memo without the
    /// group (`ZMemo::holds`), the memo not asked for it again.
    #[allow(clippy::too_many_arguments)]
    fn open_group_with<'a>(
        &'a self,
        memo: &ZMemo,
        entry: &FenceEntry<'_>,
        j: usize,
        buf: &'a mut GroupBuf,
        view: Option<&PageView<'a>>,
        inplace: bool,
        after_memo: bool,
        absent: bool,
        memoise: bool,
    ) -> Option<(Opened<'a>, usize, usize, bool)> {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        let p = entry.page as usize;
        let Some(page) = self.page_bytes(p).filter(|_| p < self.n_pages) else {
            self.fail(p);
            return None;
        };
        if !after_memo {
            memo.touch(self.id, entry.page, self.n_pages);
        }
        let Ok(index) = u8::try_from(j) else {
            self.fail(p);
            return None;
        };
        let was_verified = bit(&self.verified, p);
        if !after_memo && !absent && self.memo_first && self.memo && was_verified {
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
                        match first_checks(page, self.bind, entry, None, self.dec()) {
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
            if was_verified && !self.memo_first && !absent {
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
            if !memoise {
                // The walk's own decode (`walk_owns`): into the caller's
                // buffer below, nothing put in the memo.
                // Counted on the handle's memo and not on the form file:
                // a count every reader thread bumps at every such decode
                // is a line they all write, and the threaded scan pass
                // read a tenth slower for it once a walk's first touches
                // were its own (`Options::zpages_scan_admit`).
                memo.count_owned();
            } else {
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
                        verified_group_in(view, j, &mut fresh, self.dec())
                    } else {
                        first_decode(page, self.bind, entry, j, &mut fresh, self.dec())
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
        }
        let tick = self.decode_tick();
        let opened = if was_verified {
            verified_group_in(view, j, buf, self.dec())
        } else {
            first_decode(page, self.bind, entry, j, buf, self.dec())
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
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        if self.trusted && bit(&self.verified, p) {
            view_trusted(page)
        } else {
            page_view(page)
        }
    }

    /// Page `p`'s view, where it has bytes and they parse; `None` takes the
    /// page out of service.
    fn view_of(&self, p: usize) -> Option<PageView<'_>> {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
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
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
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
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
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
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
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
        if self.gate_out() {
            return Locate::Out;
        }
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
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
            self.open_group(memo, &entry, j, buf, Some(&view), false, true)
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
        if self.gate_out() {
            return Locate::Out;
        }
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        // The key table's shortcut (`Options::zpages_point_keys`): a key
        // this handle read through the memo before is sent to where its
        // record lay -- the group asked of the memo, the record's key
        // compared where it lies -- with no filter, fence, page or walk of
        // the group. The table names a place and the compare is the
        // answer: a slot another key took, or a group the memo has let go,
        // is the search below. The memo holds groups of verified pages
        // alone, and a group's bytes do not change under an identity, so a
        // place put for this identity is a record's start for good.
        let keyed = memo.point_keys && self.memo && !key.is_empty();
        // The filter's block asked for now and the fence searched while it
        // comes in (`Options::zpages_filter_ahead`): the probe was a line
        // the core's cache did not hold for a cold key, waited for alone.
        let filter = self.filter.as_ref().filter(|_| memo.point_filter);
        let ahead = memo.filter_ahead && filter.is_some();
        let hashed = keyed || ahead;
        let kh = if hashed {
            super::BlockedBloom::hash(key)
        } else {
            0
        };
        if ahead {
            if let Some(f) = filter {
                f.prefetch_hash(kh);
            }
        }
        if keyed {
            if let Some((page, index, off)) = memo.key_at(self.id, kh) {
                if let Some((g, n)) = memo.peek(self.id, page, index, self.memo_hashed) {
                    if record_key(g.bytes(n), off) == Some(key) {
                        memo.touch(self.id, page, self.n_pages);
                        memo.count_key_hit();
                        return Locate::Found {
                            opened: Opened::Memo(g, n),
                            off,
                            page: page as usize,
                            decoded: false,
                        };
                    }
                }
            }
        }
        // A key the filter does not hold is not in the partition: answered
        // from the heap, no page touched. Asked ahead, the fence is searched
        // first and the filter after it, the same answers in another order.
        let filtered_out = |f: &super::BlockedBloom| {
            if hashed {
                !f.maybe_contains_hash(kh)
            } else {
                !f.maybe_contains(key)
            }
        };
        if !ahead && filter.is_some_and(filtered_out) {
            return Locate::Absent { decoded: false };
        }
        // A key below the first key is not in the partition.
        let Some(e) = self.fence.entry_of(key) else {
            return Locate::Absent { decoded: false };
        };
        if ahead && filter.is_some_and(filtered_out) {
            return Locate::Absent { decoded: false };
        }
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
                match first_checks(page, self.bind, &entry, None, self.dec()) {
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
        // A point read's first touch of a block-coded group
        // (`Options::zpages_point_part`): the group is decoded into the
        // read's own buffer only as far as the walk for the key reads
        // (`seek_group_partly`), and nothing is put in the memo. A read
        // wants one record and the decoder is sequential, so a whole decode
        // was the rest of the group for nothing, into a buffer of the memo
        // last touched thousands of decodes ago, displacing a group that
        // may have been read again. The memo takes a group at its second
        // touch (`Memo::admit`), whole, as before. Only on a page this
        // process has verified, where the prefix is of bytes a first decode
        // held to every check; a first touch of a page is the open below.
        if memo.point_part && self.memo && self.own_decoder && bit(&self.verified, p) {
            let Ok(index) = u8::try_from(j) else {
                self.fail(p);
                return Locate::Fallback;
            };
            if !memo.holds(self.id, entry.page, index, self.memo_hashed)
                && !memo.admit(self.id, entry.page, index)
            {
                let tick = self.decode_tick();
                match seek_group_partly_with(&view, j, buf, key, self.dec()) {
                    Ok(Some(s)) => {
                        memo.touch(self.id, entry.page, self.n_pages);
                        self.count_decode(tick);
                        memo.count_part(s.len, s.of);
                        return match s.off {
                            Some(off) => {
                                let b: &'a GroupBuf = buf;
                                Locate::Found {
                                    opened: Opened::Borrowed(b.bytes(s.len)),
                                    off,
                                    page: p,
                                    decoded: true,
                                }
                            }
                            None => Locate::Absent { decoded: true },
                        };
                    }
                    // A stored group: opened where it lies.
                    Ok(None) => {}
                    Err(_) => {
                        self.fail(p);
                        return Locate::Fallback;
                    }
                }
            }
        }
        let Some((opened, _, count, decoded)) =
            self.open_group(memo, &entry, j, buf, Some(&view), false, true)
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
        // The key's place for its next read, where the group is the
        // memo's now -- a hit, or a decode the memo took.
        if keyed && matches!(opened, Opened::Memo(..)) {
            if let Ok(index) = u8::try_from(j) {
                memo.key_put(self.id, kh, entry.page, index, pos.off);
            }
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
            Locate::Out => Ok(match self.stride() {
                Some(body) => self.stride_read_all(body, memo, key, f),
                None => ZRead::Out,
            }),
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

    /// How many values `key` has in this partition: the dispatch's two
    /// steps (`Reader::part_count_z`), `count_block` and `count_out` on its
    /// answer for a file the block path does not serve, as a test asks them
    /// of one file.
    #[cfg(test)]
    pub(super) fn count(&self, memo: &ZMemo, key: &[u8]) -> ZRead {
        match self.count_block(memo, key) {
            ZRead::Out => self.count_out(key),
            r => r,
        }
    }

    /// `count` as the block path answers it, `Out` for a file it does not
    /// serve (`ZFile::gate_out`), whose answer is `count_out`'s: the
    /// dispatch's own count (`Reader::part_count_z`), which takes `count_out`
    /// on its arm for `Out`. Every answer here is one the dispatch's match
    /// knows the variant of where it is made, as it was before the stride
    /// body: with the stride arm's call among them, the inlined count's
    /// answer was tested at the match where it had been threaded through.
    #[inline]
    pub(super) fn count_block(&self, memo: &ZMemo, key: &[u8]) -> ZRead {
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
        if self.gate_out() {
            return Ok(match self.stride() {
                Some(body) => self.stride_scan(body, memo, from, limit, f),
                None => ZScan {
                    end: ScanEnd::Out,
                    ..ZScan::start()
                },
            });
        }
        self.scan_at_block(memo, raw, from, limit, f)
    }

    /// `scan_at` of a block file in service, out of line: inlined into the
    /// dispatch, the walk's code was the dispatch's to lay out, and a call
    /// beside it moved it -- the bulk scan through the block cache gave a
    /// sixtieth of its instructions to bounds checks the walk alone elides.
    #[inline(never)]
    fn scan_at_block<F: FnMut(&[u8], &[u8])>(
        &self,
        memo: &ZMemo,
        raw: &Blob<MmapBytes>,
        from: usize,
        limit: usize,
        f: &mut F,
    ) -> Result<ZScan> {
        let mut out = ZScan::start();
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
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        let nkeys = self.nkeys;
        let mut rank = from;
        let mut asked = false;
        // The key of a record read in place, assembled (`Opened::Coded`).
        let mut key: Vec<u8> = Vec::new();
        // The entry after the one the walk ran to the end of, under
        // `Options::zpages_bulk_steps`: the next entry's first rank is the
        // rank the walk stands at, so the fence is stepped and not searched.
        // The walk's first entry is the one this handle's last seek found
        // where it is this file's: a scan seeks its start and then walks
        // from the rank the seek found.
        let mut next_e: Option<usize> = memo
            .seek_at
            .get()
            .filter(|&(id, _)| memo.bulk_steps && id == self.id)
            .map(|(_, e)| e);
        while out.seen < limit && rank < nkeys {
            // The entry the rank falls in, and how far its records go: a
            // fence that does not cover the rank is damage, since the
            // tiling check held every rank below the key count to an
            // entry. A stepped or sought entry that does not cover the rank
            // is left for the search, whose answer one that covers it is:
            // the next entry begins past the rank, so it is the last whose
            // first rank is at or below it.
            let stepped = next_e
                .take()
                .and_then(|e| self.fence.entry(e, self.nkeys as u32))
                .filter(|en| {
                    (en.first_rank as usize) <= rank
                        && rank < en.first_rank as usize + en.count as usize
                });
            let entry = match stepped {
                Some(en) => {
                    if out.seen == 0 && rank == from {
                        out.from_seek = true;
                    } else {
                        out.stepped += 1;
                    }
                    Some(en)
                }
                None => u32::try_from(rank)
                    .ok()
                    .and_then(|r| self.fence.entry_at_rank(r))
                    .and_then(|e| self.fence.entry(e, self.nkeys as u32)),
            };
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
                if memo.bulk_steps && rank == end {
                    next_e = Some(entry.index + 1);
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
                // The walk's first touch of a group (`Options::zpages_bulk_admit`):
                // decoded into the thread's buffer, which the walk below
                // reads before the next open overwrites it, and put nowhere;
                // the memo takes a group at its second touch while the
                // first's tag stands, as it does a cursor's and a point
                // read's. A decode into the memo wrote a buffer last used
                // thousands of decodes ago, line by line, allocated the
                // group's `Arc` and freed the one it displaced, for a group
                // a scan over a store larger than the memo reads once.
                let mut held = false;
                let memoise = !memo.bulk_admit
                    || !self.memo
                    || u8::try_from(j).map_or(true, |index| {
                        held = memo.holds(self.id, entry.page, index, self.memo_hashed);
                        held || memo.admit(self.id, entry.page, index)
                    });
                // Found absent just now: the open asks the memo nothing
                // more for it (`Options::zpages_bulk_steps`).
                let absent = memo.bulk_steps && memo.bulk_admit && self.memo && !held;
                if absent {
                    out.absent += 1;
                }
                // The group the walk ends inside, its own, decoded only as
                // far as the walk reads (`Options::zpages_bulk_part`), as a
                // cursor's last group is (`zpages_walk_part`).
                if !memoise && memo.bulk_part {
                    match self.walk_last_part(
                        memo,
                        raw,
                        &view,
                        p,
                        j,
                        first,
                        rank,
                        limit - out.seen,
                        buf,
                        f,
                    )? {
                        PartWalk::Walked(n) => {
                            out.decoded += 1;
                            out.seen += n;
                            rank += n;
                            j += 1;
                            continue;
                        }
                        PartWalk::Declined => {}
                        PartWalk::Failed => {
                            out.end = ScanEnd::Fallback;
                            return Ok(());
                        }
                    }
                }
                let Some((opened, gfirst, count, decoded)) = self.open_group_with(
                    memo,
                    &entry,
                    j,
                    buf,
                    Some(&view),
                    self.inplace,
                    false,
                    absent,
                    memoise,
                ) else {
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
                // The next page's head, where the walk goes on past this
                // page's last group (`Options::zpages_bulk_ahead`): this
                // group's walk is the lead that brings it, as a cursor's is.
                // The next group of the same page follows this one's bytes,
                // which the core's own prefetcher streams, and is asked for
                // only under `Options::zpages_bulk_ahead_within`.
                if memo.bulk_ahead
                    && limit - out.seen > want
                    && self.ask_next_group(&view, &entry, j, memo.bulk_within)
                {
                    out.aheads += 1;
                }
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
                    // The common record in place, as the raw walk reads it;
                    // any other shape through the parse.
                    match compact_fixed_at(group, off) {
                        Some((k, run, records, next)) => {
                            emit_fixed(k, run, records, f);
                            off = next;
                        }
                        None => {
                            let Some((k, exts, tail, len)) = parse_record(group, off) else {
                                self.fail(p);
                                out.end = ScanEnd::Fallback;
                                return Ok(());
                            };
                            raw.read_exts(&exts, tail, &mut |v: &[u8]| f(k, v))?;
                            off += len;
                        }
                    }
                    out.seen += 1;
                    rank += 1;
                }
                j += 1;
            }
            if memo.bulk_steps && rank == end {
                next_e = Some(entry.index + 1);
            }
        }
        Ok(())
    }

    /// The bulk walk's own decode of group `j` of page `p`, whose view is
    /// `view`, where the walk ends inside the group (`Options::zpages_bulk_part`):
    /// decoded only as far as the records the walk reads, by the bytes the
    /// group's records average, and on a step at a time where the next
    /// record does not lie whole in what is decoded yet, as a cursor's last
    /// group is (`ZCursor::open_part`). `rank` is where the walk stands in
    /// the entry whose first rank is `first`, and `want` how many records
    /// it still wants. `Walked` with the records handed to `f`; `Declined`
    /// where the walk reads the group to its end, the group is stored or
    /// coded, the page is one this process has not verified (the page's
    /// first decode is the whole of it, every group checked), or the
    /// decoder is not the engine's own; `Failed`, the form file out of
    /// service, where a record does not parse in the whole group.
    #[allow(clippy::too_many_arguments)]
    fn walk_last_part<F: FnMut(&[u8], &[u8])>(
        &self,
        memo: &ZMemo,
        raw: &Blob<MmapBytes>,
        view: &PageView<'_>,
        p: usize,
        j: usize,
        first: usize,
        rank: usize,
        want: usize,
        buf: &mut GroupBuf,
        f: &mut F,
    ) -> Result<PartWalk> {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        if !self.own_decoder || !bit(&self.verified, p) {
            return Ok(PartWalk::Declined);
        }
        let Some(g) = view.group(j).filter(|g| !g.stored && !g.coded) else {
            return Ok(PartWalk::Declined);
        };
        let (gfirst, count, ulen) = (usize::from(g.first), usize::from(g.count), g.ulen as usize);
        let gstart = first.saturating_add(gfirst);
        if rank < gstart || count == 0 {
            return Ok(PartWalk::Declined);
        }
        let skip = rank - gstart;
        let reads = skip.saturating_add(want);
        if reads >= count {
            return Ok(PartWalk::Declined);
        }
        let stop = (reads + 1).saturating_mul(ulen) / count;
        let tick = self.decode_tick();
        let mut part = match open_group_partly_with(view, j, buf, stop, self.dec()) {
            Ok(Some(part)) => part,
            Ok(None) => return Ok(PartWalk::Declined),
            Err(_) => {
                self.fail(p);
                return Ok(PartWalk::Failed);
            }
        };
        self.count_decode(tick);
        if self.memo {
            memo.count_owned();
        }
        memo.count_walk_part(part.have(), part.ulen());
        let mut off = 0usize;
        for i in 0..reads {
            // The record under the offset decoded whole first: a step at a
            // time past what the guess decoded, and the group's end is the
            // last step, where a record that does not lie whole is damage.
            while !part.done() && record_key_next(buf.bytes(part.have()), off).is_none() {
                let before = part.have();
                match decode_group_more(&mut part, buf, off + PART_STEP) {
                    Ok(n) => memo.count_walk_more(n - before),
                    Err(_) => {
                        self.fail(p);
                        return Ok(PartWalk::Failed);
                    }
                }
            }
            let group = buf.bytes(part.have());
            let next = if i < skip {
                record_key_next(group, off).map(|(_, next)| next)
            } else {
                match compact_fixed_at(group, off) {
                    Some((k, run, records, next)) => {
                        emit_fixed(k, run, records, f);
                        Some(next)
                    }
                    None => match parse_record(group, off) {
                        Some((k, exts, tail, len)) => {
                            raw.read_exts(&exts, tail, &mut |v: &[u8]| f(k, v))?;
                            Some(off + len)
                        }
                        None => None,
                    },
                }
            };
            let Some(next) = next else {
                self.fail(p);
                return Ok(PartWalk::Failed);
            };
            off = next;
        }
        Ok(PartWalk::Walked(want))
    }

    /// The lines of the group a bulk walk reaches after group `j` of
    /// `entry`'s page, asked for while group `j` is walked
    /// (`Options::zpages_bulk_ahead`): the first `ZGROUP_AHEAD` bytes of the
    /// next entry's page -- its header, its directory and the start of its
    /// first group -- where `j` is the page's last group, as a cursor asks
    /// (`ZCursor::ask_group_ahead`); and the next group of the page by the
    /// view's directory where `within`. Whether it asked: not for a group
    /// of the same page unless `within`, not past the last entry, nor for
    /// records no page holds.
    fn ask_next_group(
        &self,
        view: &PageView<'_>,
        entry: &FenceEntry<'_>,
        j: usize,
        within: bool,
    ) -> bool {
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
        let lines = match view.payload(j + 1) {
            Some(payload) if within => Some((payload.as_ptr(), payload.len())),
            Some(_) => None,
            None => self
                .fence
                .page(entry.index + 1)
                .filter(|&p| p != ZNOPAGE)
                .and_then(|p| self.page_bytes(p as usize))
                .map(|page| (page.as_ptr(), ZGROUP_AHEAD.min(page.len()))),
        };
        match lines {
            Some((ptr, len)) => {
                super::prefetch_lines(ptr, len.min(ZPAGE));
                true
            }
            None => false,
        }
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
        debug_assert!(!self.is_stride(), "a block-only helper over a stride file");
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
            Locate::Out => Ok(match self.stride() {
                Some(body) => self.stride_values_at(body, memo, rank, f),
                None => ZRead::Out,
            }),
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
        if !self.gate_out() && rank < self.nkeys {
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
            Locate::Out => match self.stride() {
                Some(body) => self.stride_key_at_into(body, memo, rank, out),
                None => ZRead::Out,
            },
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
        if self.gate_out() {
            return match self.stride() {
                Some(body) => self.stride_seek(body, memo, key),
                None => ZSeek::Out,
            };
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
        if memo.bulk_steps {
            memo.seek_at.set(Some((self.id, e)));
        }
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
                    match first_checks(page, self.bind, &entry, None, self.dec()) {
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
        let r = match self.open_group(memo, &entry, j, &mut buf, Some(&view), false, true) {
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
        if self.gate_out() {
            return self
                .stride()
                .and_then(|body| self.stride_owner_span(body, key));
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
        self.gate_word() & GATE_DAMAGED != 0
    }

    /// Pages in the file that hold its records: the block form's region A,
    /// and for a stride file its value pages, `V` -- the row and the trailer
    /// apart, as region B and the trailer are. What the store sums for the
    /// form's pages (`Db::zpage_counts`, `Db::zpage_form_files`) and E2's
    /// footprint divides the keys by (`Maint::z_footprint`), where a stride
    /// file's pages hold the values alone.
    pub(super) fn pages(&self) -> usize {
        self.n_pages
    }

    /// Whether the mapping was advised random at attach.
    pub(super) fn advised(&self) -> bool {
        self.advised
    }

    /// Pages whose first-decode checks have passed in this process, and
    /// pages a check has failed: of a stride file, its value pages checked
    /// against the row, or all of them where the attach trusted the file
    /// (`StrideAttach::Trust`).
    pub(super) fn verified_pages(&self) -> u64 {
        popcount(&self.verified)
    }

    pub(super) fn bad_pages(&self) -> u64 {
        popcount(&self.bad)
    }

    /// The share of the file's pages the page cache holds now (`mincore`
    /// over the mapping): whether a build that reads the form reads memory
    /// or the device. Over the whole file whichever body it holds, so a
    /// stride file's row and trailer count as region B and the trailer do.
    pub(super) fn pages_resident(&self) -> f64 {
        let len = self.map.len();
        match self.map.slice_at(0, len as usize) {
            Some(bytes) => super::resident_share(bytes),
            None => 0.0,
        }
    }

    /// Heap bytes the form file keeps resident: the fence and the filter, and
    /// for a stride file its body's row and trailer (`StrideBody::heap_bytes`)
    /// beside an empty fence. What the store sums for the forms' heap
    /// (`Db::zpage_counts`).
    pub(super) fn resident_bytes(&self) -> usize {
        self.fence.heap_bytes()
            + self.filter.as_ref().map_or(0, |f| f.blocks.len() * 64)
            + self.stride().map_or(0, StrideBody::heap_bytes)
    }

    /// A group's decode begun: whether it is the one in sixteen this
    /// thread counts (`ZFile::decodes16`), with its start where this form
    /// file times the decodes it counts (`ZFile::time_decodes`).
    /// The decoder this file's block-coded groups are decoded by.
    fn dec(&self) -> Dec {
        Dec::of(self.own_decoder, self.flat_decoder).with_slack(self.flat_slack)
    }

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

    /// This stride file's reads counted as its heat from here on
    /// (`ZFile::stride_heat`): the policy's first look at the file. The
    /// reads before it were not counted, and a look takes no rate from a
    /// file it has not seen before.
    pub(super) fn ask_stride_heat(&self) {
        if !self.stride_heat.load(Ordering::Relaxed) {
            self.stride_heat.store(true, Ordering::Relaxed);
        }
    }

    /// A stride file's read of `ranks` values in a run, counted where the
    /// store converts between the forms (`ZFile::stride_heat`) as the
    /// groups the block form would have opened for it -- one, and one more
    /// for each group's ranks of the run (`touch_shift`) -- once in sixteen
    /// per thread on the word a raw-tailed file counts its opens in
    /// (`ZFile::opens16`): the partition's heat while the stride form
    /// serves it (`Options::zpages_stride_convert`), which a policy holds
    /// against the block files' decodes. Elsewhere a read pays the flag's
    /// load and a branch.
    #[inline]
    fn count_stride_touch(&self, ranks: u64) {
        if !self.stride_heat.load(Ordering::Relaxed) {
            return;
        }
        self.count_stride_touch_due(ranks);
    }

    #[inline(never)]
    fn count_stride_touch_due(&self, ranks: u64) {
        thread_local! {
            static TICK: Cell<u32> = const { Cell::new(0) };
        }
        let due = TICK.with(|t| {
            let v = t.get().wrapping_add(1);
            t.set(v);
            v % 16 == 0
        });
        if due {
            let groups = 1 + (ranks.saturating_sub(1) >> self.touch_shift);
            self.opens16.fetch_add(groups, Ordering::Relaxed);
        }
    }

    /// `count_stride_touch` for a cursor's read of the one value at `rank`,
    /// which a walk by the key makes of every rank it crosses: counted at
    /// the ranks a block group would begin at, so that a walk of a hundred
    /// keys is the groups it crosses and not a hundred. A lone read by
    /// rank, as a block's build makes of a rank here and there, is counted
    /// only where its rank begins a group, one time in the group's ranks:
    /// such reads are the walk's and not the reads the heat is for.
    #[inline]
    fn count_stride_touch_at(&self, rank: usize) {
        if !self.stride_heat.load(Ordering::Relaxed) {
            return;
        }
        if rank & ((1usize << self.touch_shift) - 1) == 0 {
            self.count_stride_touch_due(1);
        }
    }

    /// The form file is the record coding's (`ZFile::coded`); false for a
    /// stride file, which is neither of the block body's codings
    /// (`ZFile::kind`).
    pub(super) fn coded(&self) -> bool {
        self.coded
    }

    /// The file's bytes, whichever body: what the room and the hot-raw
    /// policy charge a served partition (`Db::settle_room_bytes`,
    /// `Maint::z_hot_look`), the page cache a file takes.
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

    /// The whole file asked for in the background, in the pieces the
    /// kernel reads whole (`MmapBytes::advise_willneed`): the form file a
    /// re-code put back in service, whose pages its drop took. The bytes
    /// submitted.
    pub(super) fn prefetch_all(&self) -> u64 {
        self.map.advise_willneed(0, self.map.len() as usize) as u64
    }

    /// The decoded bytes of every group the file holds, as its trailer says;
    /// for a stride file its values' bytes, `n * W` (`StrideBody::value_bytes`):
    /// what each body holds of the records, which the store sums for the
    /// form's ratio to them (`Db::zpage_counts`).
    pub(super) fn group_bytes(&self) -> u64 {
        match &self.body {
            ZBody::Block(t) => t.total_ulen,
            ZBody::Stride(b) => b.value_bytes(),
        }
    }

    /// This attach's number: no other form file attached in this process has
    /// it.
    pub(super) fn id(&self) -> u64 {
        self.id
    }

    /// The block trailer this attach accepted; `None` for a stride file,
    /// whose trailer is its body's (`StrideBody::trailer`).
    #[cfg(test)]
    pub(super) fn ident(&self) -> Option<&Trailer> {
        match &self.body {
            ZBody::Block(t) => Some(t),
            ZBody::Stride(_) => None,
        }
    }
}

// ---- the stride arms ---------------------------------------------------------
//
// A stride file answers each read method of a form file here, from the
// branch that method's block path takes for a file it does not serve: every
// block path loads and tests the file's gate first (`ZFile::gate_out`), the
// byte that once held the damaged flag alone and holds the stride bit as well,
// set at a stride file's attach, and the arm, out of line, sits on that
// branch beside the answer for a file out of service. A block file's read so
// runs the block path's own code and pays nothing for the stride body: the
// same load and test it made for the damaged flag. The arms first sat at the
// head of each method, a load of the body's tag, which no block path reads,
// and a branch, and the block form paid for them on every read and count and,
// in the bulk scan through the block cache, in the codegen of the walk
// inlined beside the call. Each arm honours the file's being out of service
// first (`Out`), as the block form's paths do before anything else, since a
// check that failed on one page takes the whole file out of service and a
// read of another page of it is the raw form's from then on; a page whose
// check fails takes the file out of service, and the read that met it answers
// `Fallback` with nothing handed over, so that the raw form answers it whole. A value is handed to the caller after every page of
// its slot has passed. A key the codec does not render, or one outside the
// run, is absent: the build proved every key of the partition the rendering
// of its rank, and the attach the two ends, so absence is a fact of the run
// and never a check that failed.
//
// The reads count where a block file's served read counts (the dispatch's
// form-read counters, `zreads` and the rest) and never as a decode or a group
// opened, since none is made: a stride file's heat and its census by form are
// not counted here. What only a stride file counts goes straight onto the
// handle's slot line (`StrideTally`), which no dispatch takes or tests.

impl ZFile {
    /// Whether this is a stride file (`ZFile::stride`): the one way a
    /// caller asks which body a form file holds, with `kind`.
    #[inline]
    pub(super) fn is_stride(&self) -> bool {
        matches!(self.body, ZBody::Stride(_))
    }

    /// The stride body, where this is a stride file: what each read method's
    /// arm looks at on the branch its block path takes for a file it does not
    /// serve (`ZFile::gate_out`).
    #[inline]
    pub(super) fn stride(&self) -> Option<&StrideBody> {
        match &self.body {
            ZBody::Stride(b) => Some(b),
            ZBody::Block(_) => None,
        }
    }

    /// The body and coding this form file holds, as its attach read it: the
    /// stride body, or the block body whose sampled groups are record-coded
    /// (`ZFile::coded`, the coding the selector's raw-tail pick names) or
    /// not. A record coding with compressed tails, which only
    /// `Options::zpages_record_coding` makes, reads as `RawTails` here, as
    /// `coded` has it: the attach reads the record flag and not the tails.
    pub(super) fn kind(&self) -> FileCoding {
        if self.is_stride() {
            FileCoding::Stride
        } else if self.coded {
            FileCoding::RawTails
        } else {
            FileCoding::Block
        }
    }

    /// The cursors' reads of the raw form over this stride file
    /// (`ZFile::raw_reads`): none in a build without debug assertions, and
    /// none over a block file, where the door reads a run no page holds and
    /// a value that lies in a block on purpose.
    pub(super) fn raw_reads(&self) -> u64 {
        self.raw_reads.load(Ordering::Relaxed)
    }

    /// The mapping whole, which a stride file's value pages are read from
    /// at their offsets.
    fn stride_bytes(&self) -> &[u8] {
        &self.map.0
    }

    /// Value page `p` of a stride file, checked against its word of the row
    /// (`StrideBody::page_ok`) where this process has not verified it --
    /// the check counted on the reading handle's memo, the page marked
    /// verified where it passes, the page and the file out of service where
    /// it fails. Whether the page may be read.
    fn stride_page(&self, body: &StrideBody, memo: &ZMemo, p: u32) -> bool {
        if bit(&self.verified, p as usize) {
            return true;
        }
        memo.count_stride_check();
        self.stride_page_check(body, p)
    }

    /// `stride_page` for a read with no handle's memo to count the check
    /// on (`ZFile::count_out`, `ZFile::owner_span_from`): a page of a
    /// sparse form's numbers, the one page such a read touches. The check
    /// is the same and so is the bit it leaves; what goes uncounted is a
    /// check a page, once a process.
    fn stride_page_uncounted(&self, body: &StrideBody, p: u32) -> bool {
        bit(&self.verified, p as usize) || self.stride_page_check(body, p)
    }

    /// Page `p` against its word of the row and, for a page of numbers,
    /// its samples (`StrideBody::page_ok`): marked verified where it
    /// passes, and it and the file out of service where it fails.
    #[inline(never)]
    fn stride_page_check(&self, body: &StrideBody, p: u32) -> bool {
        let i = p as usize;
        if body.page_ok(self.stride_bytes(), p) {
            set_bit(&self.verified, i);
            true
        } else {
            self.fail(i);
            false
        }
    }

    /// `read_all` of a stride file: the key's one value, its slot's pages
    /// checked first, or none for a key that is not the run's.
    #[inline(never)]
    fn stride_read_all<F: FnMut(&[u8])>(
        &self,
        body: &StrideBody,
        memo: &ZMemo,
        key: &[u8],
        f: &mut F,
    ) -> ZRead {
        if self.damaged() {
            return ZRead::Out;
        }
        let file = self.stride_bytes();
        // A page that fails was taken out of service by its check.
        match body.read_all(
            file,
            key,
            &mut |p| self.stride_page(body, memo, p),
            &mut |v| f(v),
        ) {
            StrideRead::Served(n) => {
                memo.count_stride_read();
                self.count_stride_touch(1);
                ZRead::Served { n, decoded: false }
            }
            StrideRead::Bad(_) => ZRead::Fallback,
        }
    }

    /// `count` for a file the block path does not serve (`ZFile::gate_out`):
    /// a stride file's, or `Out` for a file out of service.
    #[inline(never)]
    pub(super) fn count_out(&self, key: &[u8]) -> ZRead {
        match self.stride() {
            Some(body) => self.stride_count(body, key),
            None => ZRead::Out,
        }
    }

    /// `count` of a stride file: one for a key of the run, none for any
    /// other -- from the key alone for a whole run, no page touched, and by
    /// the search of a sparse form's numbers, whose page failing its check
    /// is a fallback.
    fn stride_count(&self, body: &StrideBody, key: &[u8]) -> ZRead {
        if self.damaged() {
            return ZRead::Out;
        }
        let file = self.stride_bytes();
        match body.rank_of(file, key, &mut |p| self.stride_page_uncounted(body, p)) {
            Ok(rank) => ZRead::Served {
                n: u64::from(rank.is_some()),
                decoded: false,
            },
            Err(_) => ZRead::Fallback,
        }
    }

    /// `scan_at` of a stride file: from rank `from`, at most `limit` ranks
    /// and never past the run, each key rendered once and stepped in place
    /// (`KeyCodec::increment`) and each value's pages checked as the walk
    /// reaches them. A page that fails ends the walk where it stands with
    /// `ScanEnd::Fallback` and the ranks handed over in `seen`, for the raw
    /// form to walk the rest from there. Under `Options::zpages_scan_prefetch`
    /// the pages the walk will cross after its first are asked for at once
    /// (`ZFile::stride_ask_ahead`), the block walk's rule.
    #[inline(never)]
    fn stride_scan<F: FnMut(&[u8], &[u8])>(
        &self,
        body: &StrideBody,
        memo: &ZMemo,
        from: usize,
        limit: usize,
        f: &mut F,
    ) -> ZScan {
        let mut out = ZScan::start();
        if self.damaged() {
            out.end = ScanEnd::Out;
            return out;
        }
        let n = body.keys();
        let from = from as u64;
        if from >= n || limit == 0 {
            return out;
        }
        let want = (limit as u64).min(n - from);
        if self.stride_ask_ahead(body, from, want).is_some() {
            out.prefetches += 1;
        }
        let len = body.codec().key_len();
        let mut kbuf = [0u8; STRIDE_KEY_MAX];
        let file = self.stride_bytes();
        // The first rank's key rendered and each after it stepped to; a
        // sparse form reads both from its pages of numbers, and one that
        // fails ends the walk as a value page that fails does, before the
        // rank whose key it held.
        if !matches!(
            body.key_at_into(file, from, &mut kbuf, &mut |p| self
                .stride_page(body, memo, p)),
            Ok(Some(_))
        ) {
            out.end = ScanEnd::Fallback;
            return out;
        }
        let direct = memo.stride_direct;
        // The keys stepped by their field, for the handle's count.
        let mut steps = 0u64;
        for rank in from..from + want {
            if rank > from {
                match body.step_key(file, rank, &mut kbuf[..len], direct, &mut |p| {
                    self.stride_page(body, memo, p)
                }) {
                    Ok(by_field) => steps += u64::from(by_field),
                    Err(_) => {
                        memo.count_stride_direct(steps);
                        out.end = ScanEnd::Fallback;
                        return out;
                    }
                }
            }
            match body.values_at(file, rank, &mut |p| self.stride_page(body, memo, p)) {
                StrideAt::Value(v) => f(&kbuf[..len], v),
                // A page taken out of service by its check; never `Absent`,
                // every rank of the walk being below the key count.
                StrideAt::Bad(_) | StrideAt::Absent => {
                    memo.count_stride_direct(steps);
                    out.end = ScanEnd::Fallback;
                    return out;
                }
            }
            out.seen += 1;
        }
        self.count_stride_touch(out.seen as u64);
        memo.count_stride_direct(steps);
        out
    }

    /// The pages a stride walk of `want` ranks from `from` will cross after
    /// its first, asked for at once, at most `ZPREFETCH_PAGES` of them
    /// (`Options::zpages_scan_prefetch`, the block walk's `ask_ahead`): the
    /// slots' span is arithmetic, so the ask is exact. What it asked, as the
    /// first page and the count: nothing with the option off, or for a walk
    /// inside one page.
    ///
    /// Behind the block walk's gate, which is off by default, so under
    /// `ZPages::Always` and `ZStride::Forced`, whose install drops the raw
    /// pages (`Options::zpages_drop_raw`), a stride walk faults its value
    /// pages one at a time. Whether a stride walk asks with the gate off is
    /// the open question this leaves to the forced pairing, which decides it
    /// from the faults a scan takes by form.
    fn stride_ask_ahead(&self, body: &StrideBody, from: u64, want: u64) -> Option<(u32, usize)> {
        if !self.prefetch || want == 0 {
            return None;
        }
        let (first, last) = body.span_pages(from, want);
        let k = ((last - first) as usize).min(ZPREFETCH_PAGES);
        if k == 0 {
            return None;
        }
        let ask = first + 1;
        self.map
            .advise_willneed(u64::from(ask) * ZPAGE as u64, k * ZPAGE);
        Some((ask, k))
    }

    /// `values_at` of a stride file: rank `rank`'s one value after its
    /// slot's pages, none past the run.
    #[inline(never)]
    fn stride_values_at<F: FnMut(&[u8])>(
        &self,
        body: &StrideBody,
        memo: &ZMemo,
        rank: usize,
        f: &mut F,
    ) -> ZRead {
        if self.damaged() {
            return ZRead::Out;
        }
        let file = self.stride_bytes();
        match body.values_at(file, rank as u64, &mut |p| self.stride_page(body, memo, p)) {
            StrideAt::Value(v) => {
                self.count_stride_touch_at(rank);
                f(v);
                ZRead::Served {
                    n: 1,
                    decoded: false,
                }
            }
            StrideAt::Absent => ZRead::Served {
                n: 0,
                decoded: false,
            },
            // Taken out of service by its check.
            StrideAt::Bad(_) => ZRead::Fallback,
        }
    }

    /// `key_at_into` of a stride file: rank `rank`'s key rendered, none past
    /// the run; no page is touched for a whole run, and a sparse form's
    /// page of numbers is, a fallback where it fails its check.
    #[inline(never)]
    fn stride_key_at_into(
        &self,
        body: &StrideBody,
        memo: &ZMemo,
        rank: usize,
        out: &mut Vec<u8>,
    ) -> ZRead {
        if self.damaged() {
            return ZRead::Out;
        }
        let mut kbuf = [0u8; STRIDE_KEY_MAX];
        let file = self.stride_bytes();
        match body.key_at_into(file, rank as u64, &mut kbuf, &mut |p| {
            self.stride_page(body, memo, p)
        }) {
            Ok(Some(k)) => {
                out.clear();
                out.extend_from_slice(k);
                ZRead::Served {
                    n: 1,
                    decoded: false,
                }
            }
            Ok(None) => ZRead::Served {
                n: 0,
                decoded: false,
            },
            Err(_) => ZRead::Fallback,
        }
    }

    /// `seek` of a stride file: the rank of the first key at or after `key`
    /// and how it compares, by the codec's arithmetic over the run
    /// (`KeyCodec::lower_bound`) -- a key the codec renders by its number,
    /// any other by counting the renderings below it -- with no page touched.
    /// `cut_in` is written over `seek` and answers through it.
    #[inline(never)]
    fn stride_seek(&self, body: &StrideBody, memo: &ZMemo, key: &[u8]) -> ZSeek {
        if self.damaged() {
            return ZSeek::Out;
        }
        let file = self.stride_bytes();
        match body.lower_bound(file, key, &mut |p| self.stride_page(body, memo, p)) {
            Ok((rank, at)) => ZSeek::Served {
                rank: rank as usize,
                at,
                decoded: false,
            },
            // A sparse form's page of numbers failed its check.
            Err(_) => ZSeek::Fallback,
        }
    }

    /// `owner_span_from` of a stride file, exact where the block form names
    /// a group: the key's own rank where it is the run's, the rank before
    /// the first key past it where it is not -- the rank whose block a block
    /// cache files the key under (`Reader::cache_owner_of`) -- and rank
    /// zero for a key below the first; one rank in every case, so the
    /// caller never resolves it again. The hint is the fence's, and a
    /// stride file has none to step.
    #[inline(never)]
    fn stride_owner_span(&self, body: &StrideBody, key: &[u8]) -> Option<(usize, usize)> {
        if self.damaged() {
            return None;
        }
        let file = self.stride_bytes();
        // A sparse form's page of numbers that fails its check leaves the
        // key no span the form can name, as a file out of service does.
        let (rank, at) = body
            .lower_bound(file, key, &mut |p| self.stride_page_uncounted(body, p))
            .ok()?;
        let r = rank as usize;
        Some(if at == KeyOrder::Equal {
            (r, r + 1)
        } else if r == 0 {
            (0, 1)
        } else {
            (r - 1, r)
        })
    }
}

/// That the segment a form file's trailer records is the live segment:
/// every quantity it records equals what the segment recomputed of itself.
/// The first mismatch is named, the name's fields and the content's
/// fingerprint first: two segments written a second apart differ in their
/// timestamps whatever else they share, and a refusal that named the clock
/// would say nothing of whether the records were the same. Either kind of
/// trailer is asked through its `seg_ident`, so the two kinds are held to
/// one comparison. The binding hash is not compared: it is a function of
/// the name's fields, the key count and the row's CRC, each held equal
/// here, and each trailer's decode holds the word it stores to those
/// fields, so a compare of it could refuse nothing the ones before it pass.
fn same_segment(
    t: &SegIdent,
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
    if t.row_crc != f.row_crc {
        return Err("the key section's checksum row differs");
    }
    if t.recs_len != f.recs_len {
        return Err("the record region's length differs");
    }
    if t.crc_off != f.crc_off {
        return Err("the checksum row's place differs");
    }
    if t.key_off != f.key_off {
        return Err("the key section's offset differs");
    }
    if t.hash_cap != f.hash_cap {
        return Err("the hash capacity differs");
    }
    if t.file_len != f.file_len {
        return Err("the segment's length differs");
    }
    if t.timestamp != f.timestamp {
        return Err("the segment's timestamp differs");
    }
    if t.generation != f.generation {
        return Err("the segment's generation differs");
    }
    if t.rec_version != f.rec_version {
        return Err("the record format's version differs");
    }
    if t.rec_magic != f.rec_magic {
        return Err("the file format's magic differs");
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
    /// Block-coded partitions built again by the parsing encoder while
    /// served (`Options::zpages_reparse`), and swapped in.
    pub reparses: AtomicU64,
    /// The re-code policy's looks, one a period it read the heat at.
    pub recode_looks: AtomicU64,
    /// The hot-raw policy's (`Options::zpages_hot_raw`): partitions released
    /// to the raw form, partitions whose form file it attached again, and
    /// its looks, one a period it read the heat at.
    pub hot_raw: AtomicU64,
    pub hot_back: AtomicU64,
    pub hot_looks: AtomicU64,
    /// Its asks for a partition's raw pages ahead of a release, and the
    /// asks dropped: the pages not there when the wait ran out, or the
    /// room gone.
    pub hot_asks: AtomicU64,
    pub hot_dropped: AtomicU64,
    /// Its asks for a raw partition's form file pages ahead of the way back
    /// (`Options::zpages_hot_back_ask`), and the backs taken at the wait's
    /// end with the pages short of the share.
    pub hot_back_asks: AtomicU64,
    pub hot_back_waited: AtomicU64,
    /// The form files asked for whole where scans came back after a
    /// stretch without one (`Options::zpages_rewarm_scans`): the looks
    /// that asked, and the files.
    pub rewarms: AtomicU64,
    pub rewarm_files: AtomicU64,
    /// A settle's waits for the form files the store asked the kernel for
    /// (`Options::zpages_settle_joins_asks`): the waits, their looks at the
    /// files' residency, the asks made again, and their milliseconds.
    pub ask_joins: AtomicU64,
    pub ask_join_polls: AtomicU64,
    pub ask_join_reasks: AtomicU64,
    pub ask_join_ms: AtomicU64,
    /// The rewarms that trimmed the allocator first
    /// (`Options::zpages_rewarm_trims`), and the resident free bytes the
    /// trims gave back.
    pub rewarm_trims: AtomicU64,
    pub rewarm_trim_bytes: AtomicU64,
    /// The rewarms that asked for the raw form's files
    /// (`Options::zpages_rewarm_raw`), the files asked, the bytes of them
    /// the page cache did not hold, and the files left for want of room.
    pub raw_rewarms: AtomicU64,
    pub raw_rewarm_files: AtomicU64,
    pub raw_rewarm_bytes: AtomicU64,
    pub raw_rewarm_short: AtomicU64,
    /// The settles that asked for the raw form's files and waited for them,
    /// and the milliseconds they waited.
    pub raw_settles: AtomicU64,
    pub raw_settle_ms: AtomicU64,
    /// The segments such a settle mapped after its ask, and the
    /// milliseconds the mapping took.
    pub raw_maps: AtomicU64,
    pub raw_map_ms: AtomicU64,
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
    /// Builds started on the builder's thread and taken in, whatever came
    /// of them (`ZCounts::build_collected`).
    pub build_collected: AtomicU64,
    /// Warms started on the builder's thread and taken in, and the
    /// builder's jobs whose thread has returned, builds and warms alike
    /// (`ZCounts::job_ends`).
    pub warm_starts: AtomicU64,
    pub warm_collected: AtomicU64,
    pub job_ends: AtomicU64,
    /// Builds collected dormant, ahead of the controller's decision
    /// (`ZCounts::prebuilt`).
    pub prebuilt: AtomicU64,
    /// Builds at a merge's output finish (`Options::zpages_build_at_finish`):
    /// started, built, failed, and the merge's waits for a slot.
    pub finish_starts: AtomicU64,
    pub finish_builds: AtomicU64,
    pub finish_failed: AtomicU64,
    pub finish_waits: AtomicU64,
    /// Builds ahead started in the second slot, beside another build
    /// ahead (`ZCounts::ahead_paired`).
    pub ahead_paired: AtomicU64,
    /// Partitions a build ahead at landing found with their records out of
    /// the page cache and left to the controller's build
    /// (`ZCounts::ahead_cold`).
    pub ahead_cold: AtomicU64,
    /// Form files built ahead in the coding the controller's pick then
    /// refused, given back unattached and built again
    /// (`ZCounts::ahead_recoded`).
    pub ahead_recoded: AtomicU64,
    /// Partitions built ahead whose records and form file were swapped in
    /// the page cache as the store passed its budget
    /// (`ZCounts::ahead_swapped`).
    pub ahead_swapped: AtomicU64,
    /// Form files asked back at their collect, their build having dropped
    /// them as it wrote them, and the bytes asked for
    /// (`ZCounts::dropped_asks`).
    pub dropped_asks: AtomicU64,
    pub dropped_ask_bytes: AtomicU64,
    /// Conversions whose file its build dropped and the collect asked back
    /// (`ZCounts::convert_dropped`).
    pub convert_dropped: AtomicU64,
    /// The builder's turns taken at a job's end between two steps
    /// (`ZCounts::job_end_drives`).
    pub job_end_drives: AtomicU64,
    /// Partitions whose raw pages the crossing asked for
    /// (`ZCounts::cross_raw_asks`).
    pub cross_raw_asks: AtomicU64,
    /// Crossings made at a buy (`ZCounts::buy_crosses`).
    pub buy_crosses: AtomicU64,
    /// Ordered indexes dropped a second time, and their bytes resident
    /// before and after (`ZCounts::raw_redrops`).
    pub raw_redrops: AtomicU64,
    pub raw_redrop_before: AtomicU64,
    pub raw_redrop_after: AtomicU64,
    /// Form files synced once named (`ZCounts::build_synced`).
    pub build_synced: AtomicU64,
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
    /// The ticks that stepped the selector and have ended, their builder's
    /// turn and their policies' looks done (`ZCounts::tick_ends`).
    pub tick_ends: AtomicU64,
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
    /// Readings refreshed from the last sample's costs and a count of the
    /// form files on disk, in place of a sample (`Maint::z_sample_refresh`).
    pub sample_refreshed: AtomicU64,
    /// Readings made from the form files on disk at the crossing, in place
    /// of a fill sample (`Maint::z_reading_from_files`,
    /// `Options::zpages_reading_from_files`).
    pub sample_from_files: AtomicU64,
    /// The price of a fault the last reading carried, nanoseconds, zero
    /// where its timed decodes took none: their wait over their faults, or
    /// the seam's price where it sets one (`ZInject::set_fault_cost`).
    pub sample_t_f_ns: AtomicU64,
    /// The sampler's estimate of the form's bytes over the partitions it
    /// counted, at its last reading (`Sample::w_z` summed).
    pub sample_w_z: AtomicU64,
    /// Where the selector chooses the coding (`Options::zpages_coding_auto`):
    /// the raw-tail form's bytes at the same reading, the codings chosen
    /// (block, then raw tails) and the builds started under the raw tails.
    pub sample_w_z_raw: AtomicU64,
    pub coding_picks: [AtomicU64; 2],
    pub builds_raw_tails: AtomicU64,
    /// A raw-tail pick taken back where the partitions' records grew past
    /// the fill share after it (`Maint::z_repick`), and the live
    /// partitions' form files on record raw-tailed at each take-back, which
    /// the files' cover then leaves out (`Maint::z_files_cover`).
    pub coding_repicks: AtomicU64,
    pub finish_refused: AtomicU64,
    /// Raw-tail picks taken as the block coding for want of a reader
    /// (`Maint::z_pick_raw_ok`).
    pub coding_unread: AtomicU64,
    /// Builds held to the room that asked nothing of the stride form
    /// (`Options::zpages_stride_room`).
    pub stride_no_room: AtomicU64,
    /// The conversion between the block and the stride forms
    /// (`Options::zpages_stride_convert`): partitions converted into the
    /// stride form, out of it, and the policy's looks.
    pub stride_converts: AtomicU64,
    pub stride_converts_back: AtomicU64,
    pub stride_convert_looks: AtomicU64,
    /// The rents the conversion's last look left, summed, microseconds
    /// (`ZCounts::stride_convert_rent_us`): a level, stored and not added.
    pub stride_convert_rent_us: AtomicU64,
    /// The forms' bytes as the conversion's last look found them and the
    /// fill it held a move in to (`ZCounts::stride_convert_held`,
    /// `stride_convert_fill`): levels, stored and not added.
    pub stride_convert_held: AtomicU64,
    pub stride_convert_fill: AtomicU64,
    /// The partitions whose raw pages were dropped at their form file's
    /// install (`Options::zpages_drop_raw`).
    pub raw_dropped: AtomicU64,
    /// Of those, the ones dropped at the gate's turn to the form rather
    /// than at their attach (`Options::zpages_drop_at_install`).
    pub raw_dropped_at_install: AtomicU64,
    /// The builds' wall, their threads' CPU and their major faults
    /// (`Built::wall_us`).
    pub build_wall_us: AtomicU64,
    pub build_cpu_us: AtomicU64,
    pub build_majflt: AtomicU64,
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
    /// handles touched, summed, and the raw bound in bytes, and of it the
    /// raw footprints counted whole (`ZCounts::footprint_held`).
    pub footprint_pages: AtomicU64,
    pub footprint_bytes: AtomicU64,
    pub footprint_held: AtomicU64,
    /// The model's two populations as the tick last summed them over the
    /// partitions, bytes: the hash regions and the record regions.
    pub model_hash: AtomicU64,
    pub model_records: AtomicU64,
    /// Form files the controller retired: taken out of their cells through
    /// the epoch and given dead names for the idle sweep.
    pub retired: AtomicU64,
    /// Stride files built and counted where a build is (`ZEvents::built`),
    /// and the builds that asked for the stride form and found the
    /// partition does not qualify, by reason (`StrideNo::code` less one):
    /// the block file built in its place under `StrideAsk::OrBlock`, or no
    /// file under `StrideAsk::Only` (`ZEvents::stride_declined`). A run in
    /// a block and a partition under the floor are the opening's declines
    /// and counted here only under `StrideAsk::Only`, a conversion's ask:
    /// under `OrBlock` they are the block form's declines too, no file is
    /// built, and they are counted once, by the block form's reason
    /// (`ZEvents::declined`).
    pub stride_builds: AtomicU64,
    pub stride_declines: [AtomicU64; STRIDE_REASONS],
    /// The stride files among `stride_builds` built in the sparse form, the
    /// keys' numbers held beside the values
    /// (`Options::zpages_stride_sparse`). What a store's other partitions
    /// are priced by before they have a file of their own: a store whose
    /// builds have been sparse ones is taken to be of such keys
    /// (`Maint::z_stride_bytes`).
    pub stride_sparse_builds: AtomicU64,
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
        ZEvents::add(&self.build_wall_us, b.wall_us);
        ZEvents::add(&self.build_cpu_us, b.cpu_us);
        ZEvents::add(&self.build_majflt, b.majflt);
        ZEvents::add(&self.build_synced, u64::from(b.sync_us > 0));
        if b.coding == FileCoding::Stride {
            ZEvents::add(&self.stride_builds, 1);
        }
        if b.stride_sparse {
            ZEvents::add(&self.stride_sparse_builds, 1);
        }
        if let Some(why) = b.stride_no {
            self.stride_declined(why);
        }
    }

    /// A build that asked for the stride form and found the partition does
    /// not qualify, counted by reason.
    pub(super) fn stride_declined(&self, why: StrideNo) {
        ZEvents::add(&self.stride_declines[usize::from(why.code() - 1)], 1);
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

/// The code `Seg::z_declined` keeps for a partition the hot-raw policy serves
/// raw while its form file waits on disk (`Options::zpages_hot_raw`): the
/// partition is hot enough that its raw pages are worth their room in the
/// page cache, and reads of it go through the raw form and are counted for
/// the policy's heat (`Seg::hot_read`). Not a reason to build: the policy
/// takes the code back when it attaches the file again, and a settle or a
/// retire clears it.
pub(super) const DECLINED_HOT_RAW: u8 = 9;

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
    /// The build's wall, the building thread's CPU over it, and the
    /// thread's major faults -- the raw records it read back from the
    /// device -- in microseconds and a count (`timed_build`).
    pub wall_us: u64,
    pub cpu_us: u64,
    pub majflt: u64,
    /// The file's sync once named, microseconds (`BuildPlan::sync`); zero
    /// where it was not asked for.
    pub sync_us: u64,
    /// The body and coding the file holds.
    pub coding: FileCoding,
    /// Why the stride form is not what was built, where the plan asked for
    /// it or the block body (`StrideAsk::OrBlock`): the block file is this
    /// build's, and the reason the segment's for its life
    /// (`Outcome::note_stride`).
    pub stride_no: Option<StrideNo>,
    /// The CRC word of the trailer of the stride file this build wrote and
    /// read back, every rank of it, before its name (`check_stride`,
    /// `zpages::trailer_crc`): what an attach of the file holds the file
    /// under the name to before it trusts its pages (`StrideAttach::Trust`),
    /// given to an attach only where the read-back read the device
    /// (`stride_device`, `Built::left_seal`). The trailer carries the row's
    /// CRC, so a file whose trailer's word is this one has this build's
    /// trailer and row, both to a CRC's strength. `None` for a block file.
    pub stride_seal: Option<u32>,
    /// Whether the read-back read every byte of the stride file from the
    /// device: none of its pages in the page cache as the read-back began,
    /// by the kernel's count (`mincore`), which is what a build that writes
    /// its output back and drops it as it goes leaves once its last piece
    /// is dropped too (`BuildPlan::drop_output`). A build that keeps its
    /// pages read back the page cache, after reading its writeback's error
    /// (`read_writeback`): the device acknowledged every page, which says
    /// that the device holds what the cache does as far as the kernel
    /// knows, and not that a read of the device returns it -- a write the
    /// device misplaced or stored wrong is seen only by a read of it.
    pub stride_device: bool,
    /// A stride file that holds its keys' numbers: the sparse form, of a
    /// partition whose keys are numbers with gaps between them.
    pub stride_sparse: bool,
    /// Whether the build wrote its output back and dropped it from the page
    /// cache as it went (`BuildPlan::drop_output`): the file is on the
    /// device alone, and whoever serves it at once asks for its pages
    /// first (`Options::zpages_ask_dropped`).
    pub dropped: bool,
}

impl Built {
    /// The word an attach of the stride file this build made trusts it by
    /// (`StrideAttach::Trust`), the attach that follows the build and one
    /// that first installs a file left on disk (`Shared::z_left_seals`)
    /// alike: the build's word where its read-back read the device, none
    /// otherwise. An attach marks a trusted page verified for the file's
    /// life, and whatever page the cache gives up after it is read from the
    /// device's copy, at once as later, so only a read-back of the device
    /// has seen what such an attach serves. A file with no word is checked
    /// as one found on disk is, each value page at its first touch, and
    /// that check is of the copy the touch reads and stands for the file's
    /// life: a page first touched while the cache holds it is checked in
    /// the cache's copy, which its build's writeback was acknowledged for
    /// (`read_writeback`), and one first touched after the cache gave it up
    /// in the device's.
    pub fn left_seal(&self) -> Option<u32> {
        self.stride_seal.filter(|_| self.stride_device)
    }
}

/// What a form file a build wrote holds: the block body in one of the
/// selector's two codings (`zselect::Coding`) -- the compressed coding the
/// options give, or the record coding's groups with their tails stored raw
/// -- or the stride body. The body, and not the segment work's record of
/// the files' codings (`Maint::z_ahead_coding`), which keeps the plan's
/// flag (`z_coded_record`): the two part where a plan asks for raw tails
/// under the block coding, a block body the record calls raw-tailed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileCoding {
    Block,
    RawTails,
    Stride,
}

/// Whether a build asks for the stride form (`zpages::StrideTrailer`): a
/// partition whose keys are an exact run of numbers at one width and whose
/// every record is one inline fixed value, held as its values alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StrideAsk {
    /// The block body, in the plan's coding.
    No,
    /// The stride form or nothing: a conversion of a partition a block file
    /// serves, which serves on where the partition does not qualify
    /// (`Outcome::NotStride`), the floor's and the blocks' declines among
    /// them, since a decline of the block form's would leave a served
    /// partition marked as one no build may touch.
    Only,
    /// The stride form where the partition qualifies, and the block body in
    /// the same call where it does not, with the reason on what was built
    /// (`Built::stride_no`); a decline of the block form is the build's
    /// answer, the partition being one no form file covers.
    OrBlock,
}

/// Key ranges a plan carries to its builder's thread, each a first key and
/// the key it ends before, empty for no end (`BuildPlan::stride_spans`).
pub(super) type KeySpans = std::sync::Arc<[(Vec<u8>, Vec<u8>)]>;

/// Whether `key` lies in one of `spans`, each from its first key up to and
/// not including its second, or without end where that is empty
/// (`BuildPlan::stride_spans`).
pub(super) fn in_spans(spans: &[(Vec<u8>, Vec<u8>)], key: &[u8]) -> bool {
    spans
        .iter()
        .any(|(lo, hi)| key >= lo.as_slice() && (hi.is_empty() || key < hi.as_slice()))
}

/// Why a partition is not held in the stride form, which a segment keeps
/// for its life (`Seg::z_stride_no`, `StrideNo::code`) beside and never in
/// `Seg::z_declined`: every code there but a refusal's and a failure's
/// stops all builds of the partition, and a partition the stride form
/// declines is one the block form serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StrideNo {
    /// A key is not of the run's shape: the empty key, one longer than a
    /// stride form takes, keys of two lengths, a byte no field of the
    /// run's kind holds, a field wider than its kind holds, or no key.
    NotShaped,
    /// The run's numbers are not its keys' count: a number missing between
    /// its ends, or a key out of its place.
    Holes,
    /// A record is not one inline value: two or more (appends, a merge's
    /// concatenation), a record that does not parse.
    NotOneValue,
    /// A value's width is not the run's, or not one to a page: two widths,
    /// or one value of no bytes (a prefixed run).
    Width,
    /// A record's run carries the tombstone flag: a promoted piece's, which
    /// keeps the flags a seal wrote, and one with nothing under it.
    Tomb,
    /// A run lies in a block (`Decline::BlockBacked`).
    BlockBacked,
    /// The record region is under the floor (`Decline::Small`).
    Small,
    /// The file written did not read back as its records through the body
    /// a reader serves it by (`check_stride`).
    SelfCheck,
    /// A run with gaps the sparse form would take, not asked at this build
    /// because its plan leaves the ask to a conversion
    /// (`BuildPlan::stride_sparse_later`). Not a refusal: the one reason a
    /// conversion still asks of the partition, and at no rent where the
    /// forms with it moved fit the fill whole (`Maint::z_convert_look`).
    Deferred,
}

/// The reasons a partition is not held in the stride form, `StrideNo::code`
/// running from one to this.
pub(super) const STRIDE_REASONS: usize = 9;

impl StrideNo {
    /// The code `Seg::z_stride_no` keeps; zero is none.
    pub(super) fn code(self) -> u8 {
        match self {
            StrideNo::NotShaped => 1,
            StrideNo::Holes => 2,
            StrideNo::NotOneValue => 3,
            StrideNo::Width => 4,
            StrideNo::Tomb => 5,
            StrideNo::BlockBacked => 6,
            StrideNo::Small => 7,
            StrideNo::SelfCheck => 8,
            StrideNo::Deferred => 9,
        }
    }

    /// Whether a partition whose segment keeps `code` may be asked for the
    /// stride form by a conversion: one with no reason against it, and one
    /// whose ask a build left for later.
    pub(super) fn askable(code: u8) -> bool {
        code == 0 || code == StrideNo::Deferred.code()
    }
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
    /// A build that asked for the stride form alone (`StrideAsk::Only`)
    /// found the partition does not qualify, or what it wrote did not read
    /// back; no file left, and the form the partition has serves on.
    NotStride(StrideNo),
}

impl Outcome {
    /// What this outcome says of the stride form, kept in `cell`
    /// (`Seg::z_stride_no`): the reason it was not built, where this names
    /// one, and none where a stride file was. Every other outcome -- a block
    /// file built where nothing asked for the stride form, a decline, a
    /// cancel -- leaves the cell as it was. A decline or a cancel of the
    /// block body under `StrideAsk::OrBlock` after the stride walk declined
    /// late is one of them and carries no reason, though the walk looked:
    /// a cancelled build's rebuild walks again and finds it, and a partition
    /// the block form declines is one no form file serves, which a
    /// conversion -- of a partition a block file serves -- never asks of.
    pub(super) fn note_stride(&self, cell: &AtomicU8) {
        if let Some(code) = self.stride_code() {
            cell.store(code, Ordering::Relaxed);
        }
    }

    /// The code `note_stride` keeps, or `None` where it leaves the cell as
    /// it was: for a build whose segment is not live yet to keep it in, a
    /// build at a merge's output finish, whose code waits by name for the
    /// attach that installs its file (`Maint::z_attach_on_disk`).
    pub(super) fn stride_code(&self) -> Option<u8> {
        match self {
            Outcome::NotStride(why) => Some(why.code()),
            Outcome::Built(b) => match (b.coding, b.stride_no) {
                (FileCoding::Stride, _) => Some(0),
                (_, Some(why)) => Some(why.code()),
                (_, None) => None,
            },
            Outcome::Declined(_) | Outcome::Cancelled => None,
        }
    }
}

/// Where a build's pieces go: in order, whole, one call a piece, at the
/// piece's own offset. The file for a build, a recorder for a test.
trait PieceSink {
    fn put(&mut self, at: u64, piece: &[u8]) -> Result<()>;
}

/// The build's sink: a piece written to the file whole, and handed to the
/// device with a hint at once, nothing waited for -- or, where the file is
/// to stay out of the page cache (`BuildPlan::drop_output`), the piece
/// before it waited for and dropped, so that no more than two pieces of the
/// file are cached at once: a build ahead beside a raw form that fits, whose
/// dirty pages pushed the raw form's out until the file was dropped whole
/// at its end, and the raw form's refaults read as the group's pressure.
struct FileSink<'a> {
    file: &'a File,
    drop: bool,
    prev: Option<(u64, u64)>,
}

impl<'a> FileSink<'a> {
    fn new(file: &'a File, drop: bool) -> FileSink<'a> {
        FileSink {
            file,
            drop,
            prev: None,
        }
    }
}

impl PieceSink for FileSink<'_> {
    fn put(&mut self, at: u64, piece: &[u8]) -> Result<()> {
        let mut f = self.file;
        f.write_all(piece)?;
        super::start_writeback(self.file, at, piece.len() as u64);
        if self.drop {
            if let Some((a, n)) = self.prev {
                drop_written_range(self.file, a, n)?;
            }
            self.prev = Some((at, piece.len() as u64));
        }
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

/// A file's own pages written back and dropped from the page cache, the
/// whole of it (`drop_written_range`).
fn drop_written(file: &File) -> Result<()> {
    drop_written_range(file, 0, 0)
}

/// `[at, at + n)` of a file written back and dropped from the page cache,
/// zero bytes being to the file's end: the writeback waited for and its
/// error read (`written_back_range`), then the clean pages dropped. The
/// error is this call's to answer: the wait consumes it for the descriptor,
/// so no later wait or sync of the file sees it, and a page whose writeback
/// failed and that the drop skipped -- one a mapping holds -- reads right
/// from the cache and wrong from the device.
#[cfg(target_os = "linux")]
fn drop_written_range(file: &File, at: u64, n: u64) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    written_back_range(file, at, n)?;
    // SAFETY: an advice over a descriptor this call holds open; it changes
    // no byte of the file.
    unsafe {
        libc::posix_fadvise(
            file.as_raw_fd(),
            at as libc::off64_t,
            n as libc::off64_t,
            libc::POSIX_FADV_DONTNEED,
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn drop_written_range(_file: &File, _at: u64, _n: u64) -> Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// A test's: the next writeback wait on this thread answers an error,
    /// once, as a write the device failed answers the first wait that reads
    /// it and no wait after (`fail_next_writeback`).
    static WRITEBACK_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A test's: the next wait for a writeback on this thread
/// (`written_back_range`) answers an error, once -- what no input of a test
/// can make.
#[cfg(test)]
fn fail_next_writeback(on: bool) {
    WRITEBACK_FAILS.with(|f| f.set(on));
}

/// `[at, at + n)` of a file's writeback started and waited for and its
/// error read, zero bytes being to the file's end: `sync_file_range` with
/// both waits, which reports a writeback error the descriptor has not yet
/// seen as an fsync does, once, and asks the filesystem for no journal
/// commit and the device for no flush, as an fsync would. Off Linux, where
/// nothing waits for a range, the file's sync. A test's seam answers an
/// error once (`fail_next_writeback`).
fn written_back_range(file: &File, at: u64, n: u64) -> Result<()> {
    #[cfg(test)]
    if WRITEBACK_FAILS.with(|f| f.replace(false)) {
        return Err(std::io::Error::other(
            "the form file's writeback was made to fail",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: a writeback over a descriptor this call holds open; it
        // reads and writes no memory of ours.
        let rc = unsafe {
            libc::sync_file_range(
                file.as_raw_fd(),
                at as libc::off64_t,
                n as libc::off64_t,
                libc::SYNC_FILE_RANGE_WAIT_BEFORE
                    | libc::SYNC_FILE_RANGE_WRITE
                    | libc::SYNC_FILE_RANGE_WAIT_AFTER,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (at, n);
        file.sync_data()
    }
}

/// A stride file's writeback waited for and its error read before its
/// read-back (`write_stride`): every page of it on the device as the cache
/// holds it, as far as the kernel knows, or the build fails. A read-back
/// that reads the cache checks the cache's copy, and so does a page's check
/// at its first touch while the cache still holds the page, which stands
/// for the file's life: the copy the reads take once the cache gives the
/// page up is the device's, and a writeback error nobody read leaves that
/// copy wrong under a check of the right one. A build that drops its
/// output reads the error of each piece's writeback but the last's as it
/// drops the piece (`drop_written_range`), and this wait reads the last's.
fn read_writeback(file: &File) -> Result<()> {
    written_back_range(file, 0, 0)
}

/// A partition built ahead turned, in the page cache, into what a build
/// past the budget leaves (`Options::zpages_land_drop_past`): the
/// segment's record region, `[at, at + len)` of `seg`, dropped
/// (`POSIX_FADV_DONTNEED`, which skips a page a mapping holds), and the
/// form file at `form` asked for (`POSIX_FADV_WILLNEED`) in the pieces the
/// kernel reads whole (`crate::bytes::WILLNEED_PIECE`): asked in windows of
/// this machine's 4 MiB requests, a device of 256 KiB requests read 64 pages
/// of each. The bytes asked for; an error opening the form file asks for
/// none.
#[cfg(target_os = "linux")]
pub(super) fn swap_ahead(seg: &File, at: u64, len: u64, form: &Path) -> u64 {
    use std::os::unix::io::AsRawFd;
    let page = ZPAGE as u64;
    let (a, b) = (at.next_multiple_of(page), (at + len) & !(page - 1));
    // SAFETY: advices over file descriptors this call holds open; neither
    // changes a byte of either file.
    unsafe {
        if a < b {
            libc::posix_fadvise(
                seg.as_raw_fd(),
                a as libc::off_t,
                (b - a) as libc::off_t,
                libc::POSIX_FADV_DONTNEED,
            );
        }
    }
    ask_file(form)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn swap_ahead(_seg: &File, _at: u64, _len: u64, _form: &Path) -> u64 {
    0
}

/// The file at `form` asked of the kernel whole (`POSIX_FADV_WILLNEED`), in
/// the pieces the kernel reads whole (`crate::bytes::WILLNEED_PIECE`): the
/// bytes asked for, none where the file does not open. The ask starts the
/// reads and waits for none of them, and a page the cache holds already is
/// not read again.
#[cfg(target_os = "linux")]
pub(super) fn ask_file(form: &Path) -> u64 {
    use std::os::unix::io::AsRawFd;
    let Ok(f) = File::open(form) else {
        return 0;
    };
    let n = f.metadata().map_or(0, |m| m.len());
    let mut off = 0u64;
    while off < n {
        let w = (n - off).min(crate::bytes::WILLNEED_PIECE as u64);
        // SAFETY: an advice over a file descriptor this call holds open;
        // it changes no byte of the file.
        unsafe {
            libc::posix_fadvise(
                f.as_raw_fd(),
                off as libc::off_t,
                w as libc::off_t,
                libc::POSIX_FADV_WILLNEED,
            );
        }
        off += w;
    }
    n
}

#[cfg(not(target_os = "linux"))]
pub(super) fn ask_file(_form: &Path) -> u64 {
    0
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
/// down to its page, and the window is asked in pieces the kernel reads
/// whole (`crate::bytes::WILLNEED_PIECE`): asked at once, a window past the
/// device's largest request was read only up to it. The call is advice and
/// its failure is nobody's.
#[cfg(target_os = "linux")]
fn ask_ahead(base: usize, from: usize, to: usize) {
    let mut a = from & !(ZPAGE - 1);
    while a < to {
        let n = crate::bytes::WILLNEED_PIECE.min(to - a);
        // SAFETY: a read-only shared mapping of a file, advised over a range
        // inside it; the kernel reads pages in and changes nothing else.
        unsafe {
            libc::madvise((base + a) as *mut libc::c_void, n, libc::MADV_WILLNEED);
        }
        a += n;
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
    /// The coding of the groups that compress (`FillScratch::coding`):
    /// `ZCODING_BLOCK` as the options give it (`Options::z_coding`), and
    /// `ZCODING_RECORD` under the arm `Options::zpages_record_coding` or
    /// where the selector picks the raw-tail form under `ZPages::Auto`
    /// (`zselect::choose_coding`).
    pub coding: u8,
    /// Every group stored verbatim, LZ4 never tried
    /// (`Options::zpages_store_all`, `FillScratch::store_all`).
    pub store_all: bool,
    /// Under the record coding, every tail stored raw
    /// (`Options::zpages_raw_tails`, `FillScratch::raw_tails`).
    pub raw_tails: bool,
    /// The block coding's groups written by the parsing encoder
    /// (`Options::zpages_parse`, `FillScratch::parse`), its search
    /// (`Options::zpages_parse_chains`, `FillScratch::parse_chains`).
    pub parse: bool,
    pub parse_chains: u32,
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
    /// Whether the tests' crash has ended the store the build is of
    /// (`Db::crash`): a build held before its rename in such a store
    /// leaves its file under the temp name, as a crash there does.
    pub life: Option<std::sync::Arc<super::Life>>,
    /// The form file's own pages written back and dropped from the page
    /// cache once it is written (`drop_written`): a build ahead the
    /// controller has not bought, beside a raw form that fits its budget,
    /// whose reads want the cache the file would take.
    pub drop_output: bool,
    /// The form file synced (`fdatasync`) once it has its name, on the
    /// build's own thread (`Options::zpages_build_sync`): its pages reach
    /// the device by `sync_file_range`, which asks for no flush of the
    /// device's write-back cache, and the next fsync anyone issues pays
    /// that flush. The sync asks for that flush alone, and its error is the
    /// file's: a build that drops its output read every writeback error as
    /// it dropped each piece (`drop_written_range`), so a flush that fails
    /// is one a crash may tear the file over, and a file a crash tore is a
    /// file found on disk, checked page by page as its reads first touch it
    /// and built again where a page is refused.
    pub sync: bool,
    /// Whether the build asks for the stride form, and what it builds
    /// where the partition does not qualify.
    pub stride: StrideAsk,
    /// The key at and above which a partition is not asked for the stride
    /// form whatever `stride` says (`Options::zpages_stride_below`): one
    /// whose first key is at or above it is built in the block form as a
    /// plan that asks nothing builds it, with no reason kept, since none was
    /// found -- the partition was not asked. `None` asks every partition.
    pub stride_below: Option<std::sync::Arc<[u8]>>,
    /// The key ranges a partition is asked for the stride form in, each a
    /// first key and the key it ends before, empty for no end: a partition
    /// whose first key lies in none is built in the block form as a plan
    /// that asks nothing builds it, with no reason kept. The ranges the
    /// stride form held as a merge began, so that each output is built in
    /// the form the partition it rewrites had (`ZStride::Chosen`,
    /// `Maint::z_stride_ask_finish`). `None` asks every partition.
    pub stride_spans: Option<KeySpans>,
    /// Whether a build that asks for the stride form takes a partition
    /// whose keys are numbers with gaps between them, in the sparse form,
    /// and the hex kinds of key with it (`Options::zpages_stride_sparse`,
    /// `stride_precheck`); off, such a partition is no run, as it was
    /// before the form held any.
    pub stride_sparse: bool,
    /// The sparse form is the store's to take and this build does not ask
    /// for it: a build made before any reader has come, of a store still
    /// in its first writing, whose size when it is read no build of it
    /// knows (`Maint::z_sparse_now`). A partition the sparse form would
    /// take is then built in the block form with `StrideNo::Deferred` for
    /// its reason, which a conversion asks again once reads have come.
    /// Nothing where `stride_sparse` is set.
    pub stride_sparse_later: bool,
    /// A test's: the stride form's file damaged once it is written and
    /// before it is read back, so that a check the reads go through is
    /// shown to refuse what it is for.
    #[cfg(test)]
    pub damage: Option<fn(&Path)>,
    /// A test's: the kernel's count of the stride file's pages it holds as
    /// the read-back begins answers nothing (`read_back_residency`), as a
    /// `mincore` that fails answers, which a mapping a test holds cannot
    /// make.
    #[cfg(test)]
    pub residency_unknown: bool,
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
            parse_chains: 0,
            own_decode: false,
            cancel: None,
            hold: None,
            life: None,
            drop_output: false,
            sync: false,
            stride: StrideAsk::No,
            stride_below: None,
            stride_spans: None,
            stride_sparse: false,
            stride_sparse_later: false,
            #[cfg(test)]
            damage: None,
            #[cfg(test)]
            residency_unknown: false,
        }
    }

    /// One of the opening's declines, as this plan answers it: a
    /// conversion's (`StrideAsk::Only`) is the stride form's, since the
    /// block file it would replace serves on and a block decline would mark
    /// the partition as one no build may touch.
    fn declined(&self, d: Decline, why: StrideNo) -> Outcome {
        match self.stride {
            StrideAsk::Only => Outcome::NotStride(why),
            StrideAsk::No | StrideAsk::OrBlock => Outcome::Declined(d),
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Acquire))
    }

    /// The test's hold before the rename waited out, or `true` where the
    /// store was crashed under it.
    fn wait_rename(&self) -> bool {
        self.hold
            .as_ref()
            .is_some_and(|h| super::zbuild_hold_wait(h, super::ZHOLD_RENAME, self.life.as_deref()))
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
/// has once a writer has opened it, a decline after pages were written
/// among them; a crash leaves it for the sweep at open. A decline before
/// it -- the opening's, and the stride form's two ends asked for alone --
/// touches no name.
///
/// Where the plan asks for the stride form (`StrideAsk`), its writer
/// (`write_stride`) takes the page walk's place over the same opening, and
/// both end in the one tail that names a file (`name_built`).
pub(super) fn build(
    seg_path: &Path,
    out: &Path,
    id: u64,
    end_seq: u64,
    plan: &BuildPlan,
) -> Result<Outcome> {
    timed_build(|| build_seen(seg_path, out, id, end_seq, plan, &mut |_, _| {}))
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
    timed_build(|| build_file_seen(file, out, id, end_seq, plan, &mut |_, _| {}))
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
        return Ok(plan.declined(Decline::Small(recs_len), StrideNo::Small));
    }
    if blob.blocks() > 0 {
        return Ok(plan.declined(Decline::BlockBacked, StrideNo::BlockBacked));
    }
    let nkeys = blob.keys();
    let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
    let tmp = {
        let mut s = out.as_os_str().to_owned();
        s.push(".tmp");
        std::path::PathBuf::from(s)
    };
    let block = |seen: &mut dyn FnMut(usize, usize)| {
        write_pages(
            &file, base, recs, dir, nkeys, bind, &tmp, &facts, id, end_seq, plan, seen,
        )
    };
    // A partition the plan keeps in the block form by its keys' place is
    // not asked (`BuildPlan::stride_below`).
    let ask = match plan.stride_below.as_deref() {
        Some(cut) if blob.key_at(0).is_some_and(|first| first >= cut) => StrideAsk::No,
        _ => plan.stride,
    };
    // Nor one whose first key lies in no range the plan asks in
    // (`BuildPlan::stride_spans`).
    let ask = match plan.stride_spans.as_deref() {
        Some(spans) if !blob.key_at(0).is_some_and(|first| in_spans(spans, first)) => StrideAsk::No,
        _ => ask,
    };
    let made = match ask {
        StrideAsk::No => block(seen),
        ask => {
            let images = SegImages::new(recs, dir, nkeys);
            let checked = stride_precheck(&images, plan.stride_sparse).map_err(|why| {
                // A run with gaps this plan leaves for a conversion to ask:
                // named as that, and not as the holes a plan that takes no
                // such run finds in it.
                let later = plan.stride_sparse_later
                    && !plan.stride_sparse
                    && stride_precheck(&images, true).is_ok_and(|f| f.sparse_last.is_some());
                if later {
                    StrideNo::Deferred
                } else {
                    why
                }
            });
            let walked = match checked {
                Ok(sf) => write_stride(
                    &file, base, recs, dir, nkeys, bind, &tmp, &facts, id, end_seq, &sf, plan, seen,
                ),
                // The two ends decide before the temporary name is this
                // build's: asked for the stride form alone, their decline
                // is the opening's and leaves the name alone, since a way
                // out removes only a name its build opened, and the name is
                // the partition's, which another build of it may hold.
                Err(why) if ask == StrideAsk::Only => return Ok(Outcome::NotStride(why)),
                Err(why) => Ok(Outcome::NotStride(why)),
            };
            match walked {
                // A late decline: the block body is built in the same
                // call, over records the walk left warm, under the same
                // temporary name, which its writer opens truncated.
                Ok(Outcome::NotStride(why)) if ask == StrideAsk::OrBlock => {
                    block(seen).map(|o| match o {
                        Outcome::Built(b) => Outcome::Built(Built {
                            stride_no: Some(why),
                            ..b
                        }),
                        other => other,
                    })
                }
                other => other,
            }
        }
    };
    match made {
        Ok(Outcome::Built(b)) => name_built(&tmp, out, plan, b),
        other => {
            let _ = std::fs::remove_file(&tmp);
            other
        }
    }
}

/// A file either body's writer built, given its name: the cancel's last
/// look, the test's hold, the rename, and the sync where the plan asks for
/// it -- the one tail both writers end in, so that what a name promises is
/// promised once.
fn name_built(tmp: &Path, out: &Path, plan: &BuildPlan, mut b: Built) -> Result<Outcome> {
    // The cancel's last look, before the file gets its name; a build past
    // it renames whatever landed meanwhile, and the collect finds the
    // target gone and the file an orphan.
    if plan.cancelled() {
        let _ = std::fs::remove_file(tmp);
        return Ok(Outcome::Cancelled);
    }
    if plan.wait_rename() {
        // Crashed under the hold: the file stays under its temp name, for
        // the open after to find as a crash before the rename leaves it.
        return Ok(Outcome::Cancelled);
    }
    std::fs::rename(tmp, out).inspect_err(|_| {
        let _ = std::fs::remove_file(tmp);
    })?;
    if plan.sync {
        let t = std::time::Instant::now();
        if let Ok(f) = File::open(out) {
            let _ = f.sync_data();
        }
        b.sync_us = (t.elapsed().as_micros() as u64).max(1);
    }
    Ok(Outcome::Built(b))
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
    let mut w = Pieces::new(FileSink::new(&out, plan.drop_output), plan.piece);
    let images = SegImages::new(recs, dir, nkeys);
    let mut scratch = FillScratch::new();
    scratch.group_bytes = plan.group_bytes;
    scratch.coding = plan.coding;
    scratch.store_all = plan.store_all;
    scratch.raw_tails = plan.raw_tails;
    scratch.parse = plan.parse;
    scratch.parse_chains = plan.parse_chains;
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
    let mut t = Trailer {
        n_pages: u64::from(pages),
        n_entries: fence.len() as u64,
        res_off: u64::from(pages) * ZPAGE as u64,
        res_len: rf.res_len,
        keys_len: rf.keys_len,
        filter_off: rf.filter_off,
        filter_blocks: rf.filter_blocks,
        res_crc: rf.res_crc,
        filter: true,
        total_ulen,
        bloom_probes: super::BLOOM_PROBES,
        bloom_sig: super::BLOOM_SIG,
        bloom_bits_per_key: super::BLOOM_BITS_PER_KEY,
        ..Trailer::default()
    };
    t.set_seg(&seg_ident_of(facts, id, end_seq));
    w.push(&t.encode())?;
    w.flush()?;
    if plan.drop_output {
        drop_written(&out)?;
    }
    let file_bytes = w.at;
    debug_assert_eq!(Some(file_bytes), t.expected_file_len());
    Ok(Outcome::Built(Built {
        fill: scratch.stats,
        file_bytes,
        total_ulen,
        asked: asked_bytes,
        wall_us: 0,
        cpu_us: 0,
        majflt: 0,
        sync_us: 0,
        coding: if plan.coding == ZCODING_RECORD && plan.raw_tails {
            FileCoding::RawTails
        } else {
            FileCoding::Block
        },
        stride_no: None,
        stride_seal: None,
        stride_device: false,
        stride_sparse: false,
        dropped: plan.drop_output,
    }))
}

/// The segment a form file of the segment `facts` describes derives from,
/// under the name fields `id` and `end_seq`: what a writer records of the
/// segment in either kind of trailer, and what the attach holds a trailer
/// to (`same_segment`). The one mapping of the live segment's account into
/// the trailers' shape, which both writers fill their trailers from
/// (`Trailer::set_seg`, `StrideTrailer::seg`).
fn seg_ident_of(facts: &ZSource, id: u64, end_seq: u64) -> SegIdent {
    SegIdent {
        seg_id: id,
        seg_end_seq: end_seq,
        nkeys: facts.nkeys,
        recs_len: facts.recs_len,
        crc_off: facts.crc_off,
        key_off: facts.key_off,
        hash_cap: facts.hash_cap,
        file_len: facts.file_len,
        timestamp: facts.timestamp,
        generation: facts.generation,
        row_crc: facts.row_crc,
        rec_version: facts.rec_version,
        rec_magic: facts.rec_magic,
    }
}

/// What the stride form's precheck found of a partition from its two ends
/// (`stride_precheck`): the keys' codec, the values' width, the file's
/// sizes, and, for a run with gaps, the last key's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StrideFacts {
    pub codec: KeyCodec,
    pub w: u32,
    pub layout: StrideLayout,
    /// The last key's number where the keys are numbers with gaps between
    /// them and the form to write is the sparse one; `None` for a whole
    /// run.
    pub sparse_last: Option<u64>,
}

/// The keys from inside a partition a precheck reads to say which kind of
/// number a run with gaps is of (`KeyCodec::detect_any`): at most this many,
/// evenly spaced between the ends.
const STRIDE_KIND_KEYS: usize = 8;

/// Whether a partition's two ends, ranks 0 and `n - 1`, make a run the
/// stride form holds, and its sizes where they do: both records one inline
/// fixed value of one width of one to a page with no flag
/// (`stride_record`), and the two keys of one length under a codec by which
/// they bound exactly the partition's count of numbers (`KeyCodec::detect`).
/// It proves the run's count and the ends' shape, nothing between them,
/// which is the build's walk to check rank by rank; what it gives is the
/// file's sizes, the trailer's own (`StrideTrailer::layout`), exact before
/// anything past the two ends is read.
///
/// With `sparse`, ends that bound more numbers than the partition has keys
/// are a run with gaps, which the sparse form holds
/// (`KeyCodec::detect_any`, over every kind): asked only where the ends
/// make no whole run, so a partition the form took before is taken as it
/// was, from its two ends. Which kind its numbers are of the ends alone do
/// not say, so a few records from inside it are read for their keys
/// (`STRIDE_KIND_KEYS`), each held to `stride_record` as the walk will
/// hold it; the sizes are then the sparse layout's, exact as a whole
/// run's are.
pub(super) fn stride_precheck(
    images: &impl Images,
    sparse: bool,
) -> std::result::Result<StrideFacts, StrideNo> {
    let n = images.records();
    let last = n.checked_sub(1).ok_or(StrideNo::NotShaped)?;
    let (k0, v0) = stride_record(images.image(0))?;
    let (kn, vn) = stride_record(images.image(last))?;
    let w = u32::try_from(v0.len())
        .ok()
        .filter(|w| (1..=STRIDE_W_MAX).contains(w))
        .ok_or(StrideNo::Width)?;
    if vn.len() != v0.len() {
        return Err(StrideNo::Width);
    }
    let no = |e| match e {
        KeyShapeNo::Holes => StrideNo::Holes,
        KeyShapeNo::Length | KeyShapeNo::Field => StrideNo::NotShaped,
    };
    let (codec, sparse_last) = match KeyCodec::detect(k0, kn, n as u64) {
        Ok(codec) => (codec, None),
        Err(e) if !sparse => return Err(no(e)),
        Err(_) => {
            let mut inside = [&b""[..]; STRIDE_KIND_KEYS];
            let mut read = 0;
            for i in 1..=STRIDE_KIND_KEYS {
                let r = i * n / (STRIDE_KIND_KEYS + 1);
                if r == 0 || r >= last || (i > 1 && r == (i - 1) * n / (STRIDE_KIND_KEYS + 1)) {
                    continue;
                }
                inside[read] = stride_record(images.image(r))?.0;
                read += 1;
            }
            match KeyCodec::detect_any(k0, kn, n as u64, &KEY_KINDS, &inside[..read]).map_err(no)? {
                (codec, true) => (codec, None),
                (codec, false) => {
                    // The last key is one the codec just counted to.
                    let last = codec.decode(kn).ok_or(StrideNo::NotShaped)?;
                    (codec, Some(last))
                }
            }
        }
    };
    // The trailer's own sizes and the refusals they make, so that what a
    // precheck prices is what the writer can write: a key count past what
    // a page index counts is no run the form holds. A sparse form's sizes
    // need none of the line's error, which the walk measures.
    let layout = StrideTrailer {
        codec,
        w,
        res_crc: 0,
        seg: SegIdent {
            nkeys: n as u64,
            ..SegIdent::default()
        },
        sparse: sparse_last.map(|last| StrideSparse { last, eps: 0 }),
    }
    .layout()
    .map_err(|_| StrideNo::NotShaped)?;
    Ok(StrideFacts {
        codec,
        w,
        layout,
        sparse_last,
    })
}

/// A record as the stride form holds it -- its key and its one value --
/// where it is one inline fixed value with no flag; why not, otherwise.
/// The compact record is read where it lies (`compact_fixed_tomb_at`), its
/// header's tombstone flag tested here, since the walk that reads it so
/// reads past the flag; any other is parsed (`parse_record`), the full
/// record `SegmentOptions::compact_records` off writes among them, and must
/// give one extent, inline, of one fixed record and no flag. The flag comes
/// first: a promoted piece keeps the flags its seal wrote, a put's among
/// them, and a run with nothing under it is a tombstone and no value.
fn stride_record(image: &[u8]) -> std::result::Result<(&[u8], &[u8]), StrideNo> {
    if let Some(f) = compact_fixed_tomb_at(image, 0) {
        if f.tomb {
            return Err(StrideNo::Tomb);
        }
        if f.records != 1 {
            return Err(StrideNo::NotOneValue);
        }
        return Ok((f.key, f.run));
    }
    let (key, exts, tail, _) = parse_record(image, 0).ok_or(StrideNo::NotOneValue)?;
    let &[e] = &*exts else {
        return Err(StrideNo::NotOneValue);
    };
    if e.is_tombstone() {
        return Err(StrideNo::Tomb);
    }
    if !e.is_inline() || e.records() != 1 {
        return Err(StrideNo::NotOneValue);
    }
    // One value written prefixed is one of no bytes: a fixed run's width
    // is never zero.
    if !e.is_fixed() {
        return Err(StrideNo::Width);
    }
    let at = e.off as usize;
    let value = tail
        .get(at..at + e.len as usize)
        .ok_or(StrideNo::NotOneValue)?;
    Ok((key, value))
}

/// A value page into the file and its word into the row, the word taken
/// over the page as it goes out (`stride_page_crc`).
fn stride_page_out(
    w: &mut Pieces<FileSink<'_>>,
    row: &mut Vec<u8>,
    page: &[u8; ZPAGE],
    bind: u32,
    p: &mut u32,
) -> Result<()> {
    row.extend_from_slice(&stride_page_crc(page, bind, *p).to_le_bytes());
    w.push(page)?;
    *p += 1;
    Ok(())
}

/// Whether `key` is the key of number `x` in the run `codec` renders, as
/// the build's walk asks it of every rank: both quantities of the run's
/// rule, the number `decoded` read from the key the run's `x` and `x`'s
/// rendering the key byte for byte, since a number alone taken from a key
/// it does not render would put a value under a key no read asks for. The
/// shipped decoder takes a key only where every byte is its field's or its
/// prefix's (`KeyCodec::decode`), which rendering undoes, so the second
/// quantity names its reason only behind a decoder that takes keys it does
/// not render -- a field read past its width, a prefix compared short --
/// and the number is this function's input so that a test can hand it the
/// lax decoder's.
fn stride_key_is(
    codec: &KeyCodec,
    key: &[u8],
    x: u64,
    decoded: Option<u64>,
    kbuf: &mut [u8; STRIDE_KEY_MAX],
) -> std::result::Result<(), StrideNo> {
    match decoded {
        None => Err(StrideNo::NotShaped),
        Some(y) if y != x => Err(StrideNo::Holes),
        Some(_) if codec.render(x, kbuf) != key => Err(StrideNo::NotShaped),
        Some(_) => Ok(()),
    }
}

/// The stride form of the partition whose records and directory these are,
/// written to `tmp` (`StrideAsk`), from the opening `write_pages` has and in
/// its shape: the file in whole pieces at their own alignment, the raw
/// records asked for ahead of the walk a window at a time, the cancel
/// looked at once a window, and each value page's word of the row taken as
/// the page completes; then the row and the trailer.
///
/// The partition has qualified by its two ends (`stride_precheck`, the
/// opening's, whose facts `sf` are) and qualifies here rank by rank, each
/// read through the directory and never by stride, since a segment closed
/// from a direct run keeps its commit markers inside the record region:
/// every record one inline fixed value of the run's width with no flag
/// (`stride_record`), and every key its rank's (`stride_key_is`). A rank
/// that fails ends the walk with its reason.
///
/// The file is then read back, every rank, through the body a reader serves
/// it by (`check_stride`), before it has a name, against the records it
/// was written from. Those are warm: the walk asked for them and left them,
/// and the check is the build's last read of them, so the drop behind the
/// walk is the check's, a window at a time -- dropped behind the first walk,
/// the check would read every record from the device a second time. A walk
/// that declines under `StrideAsk::Only` drops what it read, since nothing
/// reads it next; under `OrBlock` the block walk reads it next and drops it
/// as that walk does.
///
/// The output is not warm in the same way under `BuildPlan::drop_output`:
/// its sink writes back and drops each piece as the next goes out, and the
/// last piece is written back and dropped before the read-back, so the
/// read-back reads the whole file from the device -- the copy a later
/// attach reads, which is what lets the file left on disk keep its word
/// (`Built::left_seal`) -- and the drop after it drops what that read
/// brought in. That read is the price of a stride build where the store
/// fits, for whoever asks for one there to weigh; holding the pieces to the
/// read-back would keep the whole file dirty beside the raw form instead,
/// the shape the per-piece drop was made to end.
#[allow(clippy::too_many_arguments)]
fn write_stride(
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
    sf: &StrideFacts,
    plan: &BuildPlan,
    seen: &mut dyn FnMut(usize, usize),
) -> Result<Outcome> {
    let images = SegImages::new(recs, dir, nkeys);
    let recs_at = recs.as_ptr() as usize - base;
    let dir_at = dir.as_ptr() as usize - base;
    let mut forget = |from: usize, to: usize| {
        if plan.drop_behind && from < to {
            seen(from, to);
            drop_behind(base, file, from, to);
        }
    };
    // Where rank `r`'s record begins in the region, the region's end past
    // the last: the directory says, as it says to the block walk.
    let rec_at = |r: usize| {
        if r < nkeys {
            rd_u32(dir, r * 4).map_or(recs.len(), |o| o as usize)
        } else {
            recs.len()
        }
        .min(recs.len())
    };
    let out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp)?;
    let mut w = Pieces::new(FileSink::new(&out, plan.drop_output), plan.piece);
    let lay = sf.layout;
    let codec = sf.codec;
    let width = sf.w as usize;
    let mut row: Vec<u8> = Vec::with_capacity(usize::try_from(lay.res_len).unwrap_or(0));
    let mut page = [0u8; ZPAGE];
    let (mut fill, mut p) = (0usize, 0u32);
    let (mut asked, mut asked_bytes, mut looked) = (0usize, 0u64, 0usize);
    let mut kbuf = [0u8; STRIDE_KEY_MAX];
    // A sparse form's numbers, a word a rank, kept for the pages of them
    // the file holds after its values, for the samples and for the line's
    // error, which is a measure over all of them; nothing for a whole run.
    let mut numbers: Vec<u64> =
        Vec::with_capacity(if sf.sparse_last.is_some() { nkeys } else { 0 });
    for r in 0..nkeys {
        let at = rec_at(r);
        if r == 0 || at - looked >= plan.window {
            if plan.cancelled() {
                return Ok(Outcome::Cancelled);
            }
            looked = at;
        }
        if plan.ahead > 0 && asked < recs.len() && at + plan.ahead / 2 >= asked {
            let to = asked.saturating_add(plan.ahead).min(recs.len());
            ask_ahead(base, recs_at + asked, recs_at + to);
            asked_bytes += (to - asked) as u64;
            asked = to;
        }
        let value = stride_record(images.image(r)).and_then(|(key, value)| {
            if value.len() != width {
                return Err(StrideNo::Width);
            }
            let decoded = codec.decode(key);
            let x = match sf.sparse_last {
                // Inside the run the precheck proved: `base + r` is within
                // what the field holds.
                None => codec.base() + r as u64,
                // A run with gaps: the key's own number, which the rule
                // below holds its rendering to. The numbers ascend as the
                // keys do, a rendering being in the keys' order
                // (`KeyCodec`), from the codec's base, the first key's; a
                // file whose numbers did not would be refused by its own
                // pages at the read-back (`StrideBody::page_ok`).
                Some(_) => {
                    let x = decoded.ok_or(StrideNo::NotShaped)?;
                    debug_assert!(
                        numbers
                            .last()
                            .map_or(x == codec.base(), |&before| before < x),
                        "rank {r}'s number is the base or above the one before"
                    );
                    x
                }
            };
            stride_key_is(&codec, key, x, decoded, &mut kbuf)?;
            if sf.sparse_last.is_some() {
                numbers.push(x);
            }
            Ok(value)
        });
        let mut rest = match value {
            Ok(v) => v,
            Err(why) => {
                if plan.stride == StrideAsk::Only {
                    forget(recs_at, recs_at + rec_at(r + 1));
                    forget(dir_at, dir_at + dir.len());
                }
                return Ok(Outcome::NotStride(why));
            }
        };
        while !rest.is_empty() {
            let take = (ZPAGE - fill).min(rest.len());
            page[fill..fill + take].copy_from_slice(&rest[..take]);
            fill += take;
            rest = &rest[take..];
            if fill == ZPAGE {
                stride_page_out(&mut w, &mut row, &page, bind, &mut p)?;
                fill = 0;
            }
        }
    }
    if fill > 0 {
        page[fill..].fill(0);
        stride_page_out(&mut w, &mut row, &page, bind, &mut p)?;
    }
    debug_assert_eq!(
        u64::from(p),
        lay.v_pages,
        "the value pages are the layout's"
    );
    // A sparse form's numbers, a page of them at a time after the values,
    // each page's word into the row as a value page's goes; then, after
    // the row, the samples.
    // The last number is the walk's own last, which the precheck's read
    // of the last key is where both read one key.
    debug_assert_eq!(sf.sparse_last, numbers.last().copied());
    let sparse = numbers.last().map(|&last| StrideSparse {
        last,
        eps: stride_eps(&numbers),
    });
    if sparse.is_some() {
        for chunk in numbers.chunks(ZPAGE / 8) {
            page.fill(0);
            for (slot, x) in page.as_chunks_mut::<8>().0.iter_mut().zip(chunk) {
                *slot = x.to_le_bytes();
            }
            stride_page_out(&mut w, &mut row, &page, bind, &mut p)?;
        }
        debug_assert_eq!(
            u64::from(p),
            lay.row_pages(),
            "the pages of numbers are the layout's"
        );
        row.resize(row.len().next_multiple_of(8), 0);
        for x in numbers.iter().step_by(STRIDE_SAMPLE as usize) {
            row.extend_from_slice(&x.to_le_bytes());
        }
    }
    debug_assert_eq!(
        row.len() as u64,
        lay.res_len,
        "the resident part is the layout's"
    );
    let res_crc = stride_row_crc(&row, bind);
    w.push(&row)?;
    let pad = usize::try_from(lay.res_pages * ZPAGE as u64 - lay.res_len).unwrap_or(0);
    w.push(&vec![0u8; pad])?;
    let t = StrideTrailer {
        codec,
        w: sf.w,
        res_crc,
        seg: seg_ident_of(facts, id, end_seq),
        sparse,
    };
    let sealed = t
        .encode()
        .map_err(|e| super::err(&format!("z build: the stride trailer: {e}")))?;
    w.push(&sealed)?;
    w.flush()?;
    let file_bytes = w.at;
    debug_assert_eq!(file_bytes, lay.file_len, "the file is the layout's");
    #[cfg(test)]
    if let Some(damage) = plan.damage {
        damage(tmp);
    }
    read_writeback(&out)?;
    if plan.drop_output {
        drop_written(&out)?;
    }
    let mut dropped = 0usize;
    let mut looked = 0usize;
    let checked = check_stride(tmp, &images, facts, id, end_seq, plan, &mut |r| {
        let at = rec_at(r);
        if r == 0 || at - looked >= plan.window {
            if plan.cancelled() {
                return false;
            }
            looked = at;
        }
        if at - dropped >= plan.window {
            forget(recs_at + dropped, recs_at + at);
            dropped = at;
        }
        true
    })?;
    let device = match checked {
        StrideCheck::Refused(Outcome::Cancelled) => return Ok(Outcome::Cancelled),
        StrideCheck::Refused(o) if plan.stride == StrideAsk::OrBlock => return Ok(o),
        StrideCheck::Refused(o) => {
            forget(recs_at + dropped, recs_at + recs.len());
            forget(dir_at, dir_at + dir.len());
            return Ok(o);
        }
        StrideCheck::Read { device } => device,
    };
    forget(recs_at + dropped, recs_at + recs.len());
    forget(dir_at, dir_at + dir.len());
    if plan.drop_output {
        drop_written(&out)?;
    }
    Ok(Outcome::Built(Built {
        fill: FillStats {
            pages: lay.row_pages(),
            records: nkeys as u64,
            ..FillStats::default()
        },
        file_bytes,
        total_ulen: nkeys as u64 * u64::from(sf.w),
        asked: asked_bytes,
        wall_us: 0,
        cpu_us: 0,
        majflt: 0,
        sync_us: 0,
        coding: FileCoding::Stride,
        stride_no: None,
        stride_seal: trailer_crc(&sealed),
        stride_device: device,
        stride_sparse: sparse.is_some(),
        dropped: plan.drop_output,
    }))
}

/// The kernel's count of the pages of the mapped stride file it holds as
/// the read-back begins (`MmapBytes::resident_count`), or `None` where it
/// would not say: a count refused is no count of none, and only a count of
/// none says the read-back reads the device (`check_stride`). A test's seam
/// answers `None` (`BuildPlan::residency_unknown`).
fn read_back_residency(map: &MmapBytes, plan: &BuildPlan) -> Option<usize> {
    #[cfg(test)]
    if plan.residency_unknown {
        return None;
    }
    let _ = plan;
    map.resident_count(0, map.0.len())
}

/// What the read-back of a stride file found (`check_stride`).
enum StrideCheck {
    /// Every rank read back; `device` where none of the file's pages was in
    /// the page cache as the read-back began, by `mincore`, so that every
    /// byte it read came from the device (`Built::stride_device`).
    Read { device: bool },
    /// Not read back, and what the build answers.
    Refused(Outcome),
}

/// Whether the stride form at `tmp` reads back as the records `images` it
/// was written from, every rank, through the body a reader serves it by:
/// the body taken from the file's bytes alone (`StrideBody::from_bytes`),
/// the segment it names the live one (`same_segment`), and at each rank the
/// two quantities a file holds of it -- the value its slot holds, every
/// page checked against the row once as a reader's verified bits check it,
/// and the key its trailer's codec renders there. `each` is told of a rank
/// before it is read and stops the check where it answers false. `None`
/// where the file reads back; else what the build answers.
///
/// A key's read (`StrideBody::read_all`) is not a third leg: it is the
/// codec's inverse of the render the key leg holds, then the value leg's
/// read, so with both passing it fails only where the codec decodes a key
/// to another number than the one it renders it from, which no file can
/// make and the codec's own test holds. As a leg it would refuse nothing
/// the two do not and refuse what each of them is for, a guard no file
/// reaches that leaves either of the two deletable with every damage still
/// refused. A sparse form's is its search too, and no leg for the same
/// reason: the key leg reads every page of numbers through the check that
/// holds its numbers ascending and its samples the samples
/// (`StrideBody::page_ok`), and over pages so held the search is exact
/// whatever the trailer says the line's error is (`StrideBody::search`).
///
/// Whether the read-back read the device is the kernel's count of the
/// file's pages it holds before the first byte is read, never the plan's
/// word that the pieces were dropped: a drop is advice, and a page it
/// skips -- dirty, locked, mapped -- is read back from the cache with
/// nothing said.
#[allow(clippy::too_many_arguments)]
fn check_stride(
    tmp: &Path,
    images: &SegImages,
    facts: &ZSource,
    id: u64,
    end_seq: u64,
    plan: &BuildPlan,
    each: &mut dyn FnMut(usize) -> bool,
) -> Result<StrideCheck> {
    let refused = StrideCheck::Refused(Outcome::NotStride(StrideNo::SelfCheck));
    let map = MmapBytes::open(tmp)?;
    let device = read_back_residency(&map, plan) == Some(0);
    let bytes: &[u8] = &map.0;
    let Ok(body) = StrideBody::from_bytes(bytes) else {
        return Ok(refused);
    };
    if same_segment(&body.trailer().seg_ident(), facts, id, end_seq).is_err() {
        return Ok(refused);
    }
    let mut verified = vec![false; body.checked_pages()];
    let mut kbuf = [0u8; STRIDE_KEY_MAX];
    for r in 0..images.records() {
        if !each(r) {
            return Ok(StrideCheck::Refused(Outcome::Cancelled));
        }
        let Ok((key, value)) = stride_record(images.image(r)) else {
            return Ok(refused);
        };
        let mut good = |p: u32| {
            let i = p as usize;
            if !verified[i] {
                verified[i] = body.page_ok(bytes, p);
            }
            verified[i]
        };
        let rank = r as u64;
        if body.values_at(bytes, rank, &mut good) != StrideAt::Value(value) {
            return Ok(refused);
        }
        if body.key_at_into(bytes, rank, &mut kbuf, &mut good) != Ok(Some(key)) {
            return Ok(refused);
        }
    }
    Ok(StrideCheck::Read { device })
}

// ---- the stride form's body -------------------------------------------------

/// A stride form's body as a reader holds it: the trailer it accepted, the
/// sizes that trailer gives, the hash its pages are bound by, and the CRC
/// row in the heap, a word a value page. The one reader of the body: the
/// build reads every rank of a file back through it before the file has a
/// name (`check_stride`), and a form file that holds one serves its reads
/// by it (`ZFile::stride`). The point read by key (`StrideBody::read_all`)
/// is that serving's alone: the read-back holds the two quantities it is
/// made of, not their composition.
pub(super) struct StrideBody {
    trailer: StrideTrailer,
    layout: StrideLayout,
    bind: u32,
    row: Box<[u32]>,
    /// A sparse form's samples, every `STRIDE_SAMPLE`th of its keys'
    /// numbers, in the heap: what a key's search reads before any page.
    /// None in a form whose keys are a whole run.
    samples: Box<[u64]>,
}

/// What a stride body answers of a rank.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StrideAt<'f> {
    /// Its value, every page of its slot checked.
    Value(&'f [u8]),
    /// No rank of the run: a fact of the run, never a check that failed.
    Absent,
    /// Value page `p` failed its check, and nothing was answered.
    Bad(u32),
}

/// What a stride body answers of a key's read: the values handed over, one
/// or none, or the page that failed its check before any was.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StrideRead {
    Served(u64),
    Bad(u32),
}

impl StrideBody {
    /// The body of the stride form whose file's bytes these are, after the
    /// checks that need nothing else: `open`'s, then `with_row`'s. Not the
    /// segment's identity, which is the live segment's to hold the trailer
    /// to (`same_segment`), and no value page, each of which is checked
    /// against the row where a read first touches it (`StrideBody::page_ok`).
    pub(super) fn from_bytes(file: &[u8]) -> std::result::Result<StrideBody, ZError> {
        let (trailer, layout) = StrideBody::open(file)?;
        StrideBody::with_row(file, trailer, layout)
    }

    /// The trailer of the stride form whose file's bytes these are and its
    /// sizes: a length of whole pages, the trailer (`StrideTrailer::decode`),
    /// and the file the length its sizes say. Nothing of the row, which an
    /// attach reads only once the trailer names the live segment, since a
    /// row is checked under the hash of the segment its trailer names.
    pub(super) fn open(file: &[u8]) -> std::result::Result<(StrideTrailer, StrideLayout), ZError> {
        let len = file.len();
        if len < ZTRAILER || !len.is_multiple_of(ZPAGE) {
            return Err(ZError::Length);
        }
        let trailer = StrideTrailer::decode(&file[len - ZTRAILER..])?;
        let layout = trailer.layout()?;
        if layout.file_len != len as u64 {
            return Err(ZError::Length);
        }
        Ok((trailer, layout))
    }

    /// The body over `file`, whose trailer and sizes `open` gave: the
    /// resident part's CRC and the zeros after it, and the row copied into
    /// the heap, a word a page it covers. A sparse form's samples follow
    /// the row under the same CRC and are copied too, after the checks the
    /// trailer alone lets a reader make of them: the first is the first
    /// key's number, they ascend, and the last is no more than the last
    /// key's -- what makes a search of them a search of an ordered list,
    /// whatever the numbers' pages turn out to hold when they are read
    /// (`StrideBody::page_ok`).
    pub(super) fn with_row(
        file: &[u8],
        trailer: StrideTrailer,
        layout: StrideLayout,
    ) -> std::result::Result<StrideBody, ZError> {
        let bind = trailer.bind32();
        // Inside a file of the layout's length: the resident part and its
        // padding lie before the trailer.
        let at = layout.res_off as usize;
        let res = &file[at..at + layout.res_len as usize];
        if stride_row_crc(res, bind) != trailer.res_crc {
            return Err(ZError::ResidentCrc);
        }
        let pad = &file[at + res.len()..at + (layout.res_pages as usize) * ZPAGE];
        if pad.iter().any(|&b| b != 0) {
            return Err(ZError::Resident("a byte after the row is not zero"));
        }
        let row_len = layout.row_len() as usize;
        let row = res[..row_len]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| u32::from_le_bytes(*w))
            .collect();
        let samples: Box<[u64]> = match trailer.sparse {
            None => Box::default(),
            Some(sp) => {
                let from = (layout.samples_off() - layout.res_off) as usize;
                if res[row_len..from].iter().any(|&b| b != 0) {
                    return Err(ZError::Resident(
                        "a byte between the row and the samples is not zero",
                    ));
                }
                let samples: Box<[u64]> = res[from..]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|w| u64::from_le_bytes(*w))
                    .collect();
                if samples.first() != Some(&trailer.codec.base())
                    || samples.windows(2).any(|w| w[0] >= w[1])
                    || samples.last().is_some_and(|&x| x > sp.last)
                {
                    return Err(ZError::Resident(
                        "the samples are not the first number ascending to the last",
                    ));
                }
                samples
            }
        };
        Ok(StrideBody {
            trailer,
            layout,
            bind,
            row,
            samples,
        })
    }

    /// The trailer this body was read from.
    pub(super) fn trailer(&self) -> &StrideTrailer {
        &self.trailer
    }

    /// The run's keys, `n`.
    pub(super) fn keys(&self) -> u64 {
        self.trailer.seg.nkeys
    }

    /// The value pages, `V`.
    pub(super) fn pages(&self) -> usize {
        self.layout.v_pages as usize
    }

    /// The pages the row has a word for, which a reader keeps a verified
    /// bit each for: the value pages, and a sparse form's pages of numbers
    /// after them.
    pub(super) fn checked_pages(&self) -> usize {
        self.layout.row_pages() as usize
    }

    /// What a sparse form holds of its keys, or `None` for a form whose
    /// keys are a whole run: a test's look at the trailer's word.
    #[cfg(test)]
    pub(super) fn sparse(&self) -> Option<StrideSparse> {
        self.trailer.sparse
    }

    /// The bytes a read by key touches a page of beside the value's: a
    /// sparse form's numbers, none in a form whose keys are a whole run.
    pub(super) fn number_bytes(&self) -> u64 {
        if self.trailer.sparse.is_some() {
            self.keys() * 8
        } else {
            0
        }
    }

    /// The values' bytes, `n * W`: what the file holds of the records, the
    /// stride form's counterpart of the block form's decoded group bytes.
    pub(super) fn value_bytes(&self) -> u64 {
        self.keys() * u64::from(self.trailer.w)
    }

    /// The heap a reader holds of the body: the row, a word a value page,
    /// and the body itself, the trailer among it.
    pub(super) fn heap_bytes(&self) -> usize {
        self.row.len() * 4 + self.samples.len() * 8 + std::mem::size_of::<StrideBody>()
    }

    /// The rank whose slot value page `p` holds the start of, or, where no
    /// slot starts on it, the slot its first byte is in: a rank whose value
    /// reads a byte of the page, for `p` below `V`. A slot is no wider than
    /// a page (`STRIDE_W_MAX`), so the first slot to start at or after a
    /// page's first byte starts inside the page, and the one page that can
    /// hold no start is the last, its bytes the tail of a slot begun on the
    /// page before.
    pub(super) fn rank_on_page(&self, p: u32) -> u64 {
        let w = u64::from(self.trailer.w);
        let at = u64::from(p) * ZPAGE as u64;
        let first = at.div_ceil(w);
        if first < self.keys() {
            first
        } else {
            at / w
        }
    }

    /// Where `key` falls in the run: the rank of the first key at or after
    /// it and how that key compares, the key count with `Greater` past
    /// them all. By arithmetic for a whole run (`KeyCodec::lower_bound`),
    /// no page touched; by a search of a sparse form's numbers for the
    /// key's place among the field's (`KeyCodec::at_or_after`,
    /// `StrideBody::search`), whose pages are held to `good` first, the
    /// page that failed the answer where one does.
    pub(super) fn lower_bound(
        &self,
        file: &[u8],
        key: &[u8],
        good: &mut dyn FnMut(u32) -> bool,
    ) -> std::result::Result<(u64, KeyOrder), u32> {
        let n = self.keys();
        if self.trailer.sparse.is_none() {
            return Ok(self.trailer.codec.lower_bound(key, n));
        }
        let (at, exact) = self.trailer.codec.at_or_after(key);
        // Past every number the field holds: past every key.
        let Ok(x) = u64::try_from(at) else {
            return Ok((n, KeyOrder::Greater));
        };
        let (rank, found) = self.search(file, x, good)?;
        Ok((
            rank,
            if exact && found {
                KeyOrder::Equal
            } else {
                KeyOrder::Greater
            },
        ))
    }

    /// The codec the run's keys are rendered by.
    pub(super) fn codec(&self) -> &KeyCodec {
        &self.trailer.codec
    }

    /// Whether page `p` of `file` is the page the row says, and, for a
    /// page of a sparse form's numbers, a page a search may read: its
    /// numbers ascend, each rank of it that is a sample is that sample --
    /// the first of all the codec's base with them, which the samples
    /// begin at (`StrideBody::with_row`) -- the form's last number is the
    /// trailer's, and nothing follows the last. The CRC alone would let a
    /// file whose samples and numbers disagree, each under a good
    /// checksum, send a key's search to a run the key is not in and answer
    /// it absent; held to each other at a page's first touch, a search
    /// that reads a run reads the one its samples name.
    pub(super) fn page_ok(&self, file: &[u8], p: u32) -> bool {
        let at = p as usize * ZPAGE;
        let page = match (file.get(at..at + ZPAGE), self.row.get(p as usize)) {
            (Some(page), Some(&word)) if stride_page_crc(page, self.bind, p) == word => page,
            _ => return false,
        };
        let Some(sp) = self.trailer.sparse else {
            return true;
        };
        let Some(k) = u64::from(p).checked_sub(self.layout.v_pages) else {
            return true;
        };
        let per = (ZPAGE / 8) as u64;
        let (first, n) = (k * per, self.keys());
        let held = per.min(n.saturating_sub(first)) as usize;
        let words = page.as_chunks::<8>().0;
        let number = |i: usize| u64::from_le_bytes(words[i]);
        if held == 0 || (1..held).any(|i| number(i - 1) >= number(i)) {
            return false;
        }
        if words[held..].iter().any(|w| *w != [0u8; 8]) {
            return false;
        }
        if first + held as u64 == n && number(held - 1) != sp.last {
            return false;
        }
        let step = STRIDE_SAMPLE as usize;
        (0..held)
            .step_by(step)
            .all(|i| self.samples.get((first as usize + i) / step) == Some(&number(i)))
    }

    /// Rank `rank`'s number in a sparse form's `file`, its page held to
    /// `good` first: `Err` with the page where that fails.
    #[inline]
    fn number_at(
        &self,
        file: &[u8],
        rank: u64,
        good: &mut dyn FnMut(u32) -> bool,
    ) -> std::result::Result<u64, u32> {
        let at = self.layout.k_off() + rank * 8;
        let p = (at / ZPAGE as u64) as u32;
        if !good(p) {
            return Err(p);
        }
        let at = at as usize;
        Ok(u64::from_le_bytes(
            file[at..at + 8].try_into().expect("eight bytes of a page"),
        ))
    }

    /// Where number `x` falls among a sparse form's numbers: the rank of
    /// the first at or after it, and whether that number is `x`; the key
    /// count past them all. The samples say which run of `STRIDE_SAMPLE`
    /// numbers to read -- the last sample at or below `x`, looked for
    /// within the line's error of where the line puts `x`
    /// (`stride_line`, `StrideSparse::eps`) and, where the samples there do
    /// not bracket `x`, among them all, so the error the trailer records
    /// bounds a search's cost and never its answer -- and the run is one
    /// page's, asked for whole and searched. A run is inside one page of
    /// numbers, a page holding a whole count of them, and the page's check
    /// held its numbers ascending and its samples the samples
    /// (`StrideBody::page_ok`); so a number found is the key's, and one not
    /// found below a later number of the run is absent. Where `x` is above
    /// the whole run, the next run's first number bounds it, which the
    /// samples say is above `x`: that number is read where it lies, on
    /// this page or the next, so that the bound too is the file's numbers'
    /// and not the samples' word alone.
    fn search(
        &self,
        file: &[u8],
        x: u64,
        good: &mut dyn FnMut(u32) -> bool,
    ) -> std::result::Result<(u64, bool), u32> {
        let n = self.keys();
        let (Some(sp), base) = (self.trailer.sparse, self.trailer.codec.base()) else {
            return Ok((n, false));
        };
        if x <= base {
            return Ok((0, x == base));
        }
        if x > sp.last {
            return Ok((n, false));
        }
        let samples = &self.samples[..];
        let m = samples.len();
        let reach = sp.eps as usize + 1;
        let g = stride_line(base, sp.last, m as u64, x) as usize;
        let (a, z) = (g.saturating_sub(reach).min(m - 1), (g + reach + 1).min(m));
        let mut s = a + samples[a..z].partition_point(|&v| v <= x);
        // The window brackets `x` where the sample before the answer is at
        // or below it and the one at the answer is above it or the end.
        if (s == a && a > 0) || (s == z && z < m && samples[z] <= x) {
            s = samples.partition_point(|&v| v <= x);
        }
        // `x` is above the first sample, the base.
        let s = (s - 1) as u64;
        let lo = s * STRIDE_SAMPLE;
        let len = STRIDE_SAMPLE.min(n - lo) as usize;
        let at = (self.layout.k_off() + lo * 8) as usize;
        let p = (at / ZPAGE) as u32;
        if !good(p) {
            return Err(p);
        }
        let run = file[at..at + len * 8].as_chunks::<8>().0;
        prefetch_run(run);
        let i = run.partition_point(|w| u64::from_le_bytes(*w) < x);
        if i < len {
            return Ok((lo + i as u64, u64::from_le_bytes(run[i]) == x));
        }
        // Above the whole run, which is not the last: `x` is no more than
        // the last number, and the last run holds it. The next run's first
        // number is the bound.
        let next = lo + len as u64;
        if next >= n || self.number_at(file, next, good)? <= x {
            // A run the samples sent `x` to that the numbers do not hold it
            // in: the file's two accounts of its keys disagree.
            return Err(p);
        }
        Ok((next, false))
    }

    /// The rank `key` is at, or `None` for a key the form does not hold:
    /// for a whole run its number, where it is a key the codec renders,
    /// less the run's first, inside the run -- nothing else is a key of
    /// it, the build having proved every key shaped and the run whole, and
    /// no page is touched; for a sparse form the rank its number is found
    /// at (`StrideBody::search`), `Err` with the page of numbers that
    /// failed `good`.
    pub(super) fn rank_of(
        &self,
        file: &[u8],
        key: &[u8],
        good: &mut dyn FnMut(u32) -> bool,
    ) -> std::result::Result<Option<u64>, u32> {
        let c = &self.trailer.codec;
        let Some(x) = c.decode(key) else {
            return Ok(None);
        };
        if self.trailer.sparse.is_none() {
            return Ok(x.checked_sub(c.base()).filter(|&r| r < self.keys()));
        }
        Ok(match self.search(file, x, good)? {
            (rank, true) => Some(rank),
            (_, false) => None,
        })
    }

    /// Rank `rank`'s key, rendered into `buf`, or `None` past the run: its
    /// number the run's first and the rank for a whole run, no page
    /// touched, and the number a sparse form holds at the rank, `Err`
    /// with its page where that fails `good`.
    pub(super) fn key_at_into<'b>(
        &self,
        file: &[u8],
        rank: u64,
        buf: &'b mut [u8; STRIDE_KEY_MAX],
        good: &mut dyn FnMut(u32) -> bool,
    ) -> std::result::Result<Option<&'b [u8]>, u32> {
        if rank >= self.keys() {
            return Ok(None);
        }
        let c = &self.trailer.codec;
        let x = if self.trailer.sparse.is_some() {
            self.number_at(file, rank, good)?
        } else {
            c.base() + rank
        };
        Ok(Some(c.render(x, buf)))
    }

    /// A rank's key, rendered, moved to the key of `next`, the rank after
    /// it and inside the run: the one step a walk makes from a key it
    /// rendered once, the file's (`ZFile::stride_scan`) and a cursor's
    /// (`ZCursor::stride_walk`). In place for a whole run
    /// (`KeyCodec::increment`), whose next number the field holds, the run
    /// being one the codec holds whole (`KeyCodec::holds`); rendered from
    /// the number a sparse form holds at `next`, `Err` with its page where
    /// that fails `good` and the key left as it was. Whether the step wrote
    /// the field alone (`direct`, a sparse form's under
    /// `Options::zpages_stride_step_direct`), which its callers count.
    ///
    /// Inline, as the number's read is and with the value's (`values_at`):
    /// a walk calls each once a record with the same check of a page
    /// (`good`), and out of line that check is a call through a pointer a
    /// record where inlined it is a bit's test.
    #[inline]
    pub(super) fn step_key(
        &self,
        file: &[u8],
        next: u64,
        key: &mut [u8],
        direct: bool,
        good: &mut dyn FnMut(u32) -> bool,
    ) -> std::result::Result<bool, u32> {
        if self.trailer.sparse.is_some() {
            let x = self.number_at(file, next, good)?;
            let c = &self.trailer.codec;
            // The field alone, written where it lies: the prefix is the
            // key before's, which every key of the run shares
            // (`Options::zpages_stride_step_direct`).
            if direct && key.len() == c.key_len() {
                c.render_field(x, &mut key[c.p()..]);
                return Ok(true);
            }
            let mut buf = [0u8; STRIDE_KEY_MAX];
            key.copy_from_slice(c.render(x, &mut buf));
            return Ok(false);
        }
        let stepped = self.trailer.codec.increment(key);
        debug_assert!(
            stepped,
            "a walk inside the run never passes the field's top"
        );
        Ok(false)
    }

    /// Rank `rank`'s value in `file`, every page of its slot -- one, or two
    /// where it straddles -- held to `good` first: a reader's verified bit,
    /// or its check (`StrideBody::page_ok`) whose answer it keeps. `file` is
    /// the file the body was opened over, whose length the open held to the
    /// layout's (`StrideBody::open`), every value page inside it: a slot of a
    /// rank of the run lies in it, and is indexed where it lies.
    #[inline]
    pub(super) fn values_at<'f>(
        &self,
        file: &'f [u8],
        rank: u64,
        good: &mut dyn FnMut(u32) -> bool,
    ) -> StrideAt<'f> {
        if rank >= self.keys() {
            return StrideAt::Absent;
        }
        let (first, last) = self.span_pages(rank, 1);
        for p in first..=last {
            if !good(p) {
                return StrideAt::Bad(p);
            }
        }
        let w = u64::from(self.trailer.w);
        let at = rank * w;
        StrideAt::Value(&file[at as usize..(at + w) as usize])
    }

    /// The first and the last value page the slots of `count` ranks from
    /// `from` span, `count` at least one: the one arithmetic from slots to
    /// pages, which a value's read checks each page of and a walk asks the
    /// pages of ahead (`ZFile::stride_ask_ahead`).
    pub(super) fn span_pages(&self, from: u64, count: u64) -> (u32, u32) {
        debug_assert!(count > 0, "a span of no slot");
        let (w, page) = (u64::from(self.trailer.w), ZPAGE as u64);
        let (at, end) = (from * w, (from + count) * w);
        ((at / page) as u32, ((end - 1) / page) as u32)
    }

    /// `key`'s value in `file` handed to `f`, after its pages' checks: one
    /// for a key of the run, none for any other, and nothing handed over
    /// where a page failed.
    pub(super) fn read_all(
        &self,
        file: &[u8],
        key: &[u8],
        good: &mut dyn FnMut(u32) -> bool,
        f: &mut dyn FnMut(&[u8]),
    ) -> StrideRead {
        let rank = match self.rank_of(file, key, good) {
            Ok(Some(rank)) => rank,
            Ok(None) => return StrideRead::Served(0),
            Err(p) => return StrideRead::Bad(p),
        };
        match self.values_at(file, rank, good) {
            StrideAt::Value(v) => {
                f(v);
                StrideRead::Served(1)
            }
            StrideAt::Absent => StrideRead::Served(0),
            StrideAt::Bad(p) => StrideRead::Bad(p),
        }
    }
}

/// A run of a sparse stride form's numbers asked of the cache together,
/// before its search: eight lines at the sample stride, one wait where a
/// search's probes would each wait in turn (`prefetch_lines`).
#[inline]
fn prefetch_run(run: &[[u8; 8]]) {
    super::prefetch_lines(run.as_ptr().cast(), run.len() * 8);
}

/// A build run on this thread with its wall, the thread's CPU and its major
/// faults over it written into what it built, which is what says whether a
/// build under the readers' faults waited on the device or ran.
fn timed_build(f: impl FnOnce() -> Result<Outcome>) -> Result<Outcome> {
    let t = std::time::Instant::now();
    let (cpu0, flt0) = (super::thread_cpu_ns(), thread_major_faults());
    let out = f()?;
    Ok(match out {
        Outcome::Built(mut b) => {
            b.wall_us = t.elapsed().as_micros() as u64;
            b.cpu_us = super::thread_cpu_ns().saturating_sub(cpu0) / 1000;
            b.majflt = thread_major_faults()
                .zip(flt0)
                .map_or(0, |(a, b)| a.saturating_sub(b));
            Outcome::Built(b)
        }
        other => other,
    })
}

// ---- the controller's platform side ---------------------------------------

use super::zselect::Budget;
// The readings `read_budget` parses, which exist on Linux alone.
#[cfg(target_os = "linux")]
use super::zselect::{
    parse_cgroup_v1_path, parse_cgroup_v2_path, parse_limit, parse_meminfo_available,
    parse_meminfo_total, parse_mount, parse_stat_v1, parse_stat_v2, parse_status_rss_anon,
    V1_NO_LIMIT,
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
    /// The share of a partition's records the page cache holds, as the
    /// bits of an `f64`, which a build ahead at landing reads in place of
    /// `mincore`'s (`Options::zpages_land_pct`); `u64::MAX` for the
    /// kernel's. A test's files are small and fresh, and resident.
    pub records_resident: AtomicU64,
    /// The group's unused memory less the margin, as `Budget::slack` reads
    /// it, for the hot-raw policy's room (`Options::zpages_hot_raw_slack`);
    /// `u64::MAX` for none, which leaves the policy its fill line.
    pub slack: AtomicU64,
    /// Looks at the asked form files' residency that a settle's wait for
    /// them reads as nothing resident yet
    /// (`Options::zpages_settle_joins_asks`), counted down by the looks: a
    /// test's files are a mebibyte and resident before the first look, so
    /// the wait is held to waiting, and to giving up, through this.
    pub asked_holds: AtomicU64,
    /// The first file a settle's ask for the raw form's files names is
    /// asked this many milliseconds late, from a thread of its own: a file
    /// the kernel reads in after the others; 0 for none.
    pub late_ask_ms: AtomicU64,
    /// What a conversion's build costs, nanoseconds, in place of the
    /// builds' measured wall a byte with its displacement
    /// (`Options::zpages_stride_convert`); 0 for the measured one. A move
    /// is made once its rent reaches the price, and a test's builds are
    /// of a few hundred kilobytes at whatever the host gives them, so the
    /// look a move falls at is the host's without this.
    pub convert_price_ns: AtomicU64,
    /// The wall of an operation the census stamps, nanoseconds, in place of
    /// the one it took (`Options::zpages_census_sample`); 0 for the measured
    /// one. The census scales its stamps' wall to the window's operations,
    /// one in sixteen of them stamped, so one stamped read the host stalled
    /// for two milliseconds read as thirty in a window of three thousand,
    /// and the test that holds the scaling to the wall every stamp gives
    /// failed under the gate's load, once in several runs.
    pub op_wall_ns: AtomicU64,
}

impl Default for ZInject {
    fn default() -> ZInject {
        ZInject {
            budget: AtomicU64::new(0),
            refault_rate: AtomicU64::new(u64::MAX),
            clock_ns: AtomicU64::new(0),
            faults: AtomicU64::new(0),
            fault_cost_ns: AtomicU64::new(0),
            op_wall_ns: AtomicU64::new(0),
            decode_cost_ns: AtomicU64::new(0),
            records_resident: AtomicU64::new(u64::MAX),
            slack: AtomicU64::new(u64::MAX),
            asked_holds: AtomicU64::new(0),
            late_ask_ms: AtomicU64::new(0),
            convert_price_ns: AtomicU64::new(0),
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

    pub fn set_slack(&self, bytes: u64) {
        self.slack.store(bytes, Ordering::Release);
    }

    /// The next `looks` looks of a settle's wait for the asked form files
    /// read nothing resident.
    pub fn hold_asked(&self, looks: u64) {
        self.asked_holds.store(looks, Ordering::Release);
    }

    /// A settle's ask for the raw form's files names its first file
    /// `ms` milliseconds late.
    pub fn ask_first_late(&self, ms: u64) {
        self.late_ask_ms.store(ms, Ordering::Release);
    }

    /// How late the first raw file is asked for, if at all.
    pub(super) fn late_ask(&self) -> Option<Duration> {
        match self.late_ask_ms.load(Ordering::Acquire) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }

    /// One held look taken, if any is left.
    pub(super) fn take_asked_hold(&self) -> bool {
        self.asked_holds
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
            .is_ok()
    }

    pub fn slack(&self) -> Option<u64> {
        match self.slack.load(Ordering::Acquire) {
            u64::MAX => None,
            b => Some(b),
        }
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
    /// The seam's wall of a stamped operation, or `None` for the measured.
    pub fn op_wall(&self) -> Option<Duration> {
        match self.op_wall_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

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

    /// The price of a conversion's build, in place of the measured one
    /// (`Options::zpages_stride_convert`).
    pub fn set_convert_price(&self, d: Duration) {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1);
        self.convert_price_ns.store(ns, Ordering::Release);
    }

    /// The injected price of a conversion's build, if one is set.
    pub fn convert_price(&self) -> Option<Duration> {
        match self.convert_price_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// The injected decode of a page, if one is set.
    pub fn decode_cost(&self) -> Option<Duration> {
        match self.decode_cost_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// The share of every partition's records the builds ahead at landing
    /// read as resident, in place of the kernel's.
    pub fn set_records_resident(&self, share: f64) {
        self.records_resident
            .store(share.to_bits(), Ordering::Release);
    }

    /// The injected share of the records resident, if one is set.
    pub fn records_resident(&self) -> Option<f64> {
        match self.records_resident.load(Ordering::Acquire) {
            u64::MAX => None,
            bits => Some(f64::from_bits(bits)),
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
    pub parse_chains: u32,
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
            parse_chains: 0,
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
        scratch.parse_chains = self.parse_chains;
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

/// What a form file on disk says of itself, for the controller's reading
/// at the crossing (`Maint::z_reading_from_files`): its length, its pages,
/// its resident part -- region B with the trailer, what an attach maps and
/// keeps; for a stride file its value pages, and its row with the trailer.
/// Read from the trailer alone, one page of the file, advised random
/// first where `advise` says, as the attach advises its mapping; `None` where the file is
/// missing, does not open, is not a form file of this segment (another
/// generation's, by `same_segment`) or is not the length its trailer says,
/// which leaves the partition to the fill sample. The last page's magic says
/// which body the file holds, as it says at the attach (`ZFile::attach`): a
/// stride file under a store that refuses the form (`stride`,
/// `StrideAttach::Refuse`) is a file the attach will not take, `None`; one
/// the store takes is on disk as a block file is. Read as a block trailer
/// alone, every stride file was missing to the reading while the files'
/// cover (`Maint::z_files_cover`) counted it on disk, and the reading put
/// the block files' bytes to the keys over the stride partitions' in their
/// place.
pub(super) struct FileFacts {
    pub bytes: u64,
    pub pages: u64,
    pub resident: u64,
    /// The file holds the stride body: no group to time a decode of
    /// (`time_file_decodes`), and a read of it decodes nothing.
    pub stride: bool,
}

pub(super) fn file_facts(
    path: &Path,
    blob: &Blob<MmapBytes>,
    id: u64,
    end_seq: u64,
    advise: bool,
    stride: &StrideAttach,
) -> Option<FileFacts> {
    let map = MmapBytes::open(path).ok()?;
    // The advice before the first read, as the attach's: under the kernel's
    // default the trailer's page pulls a readahead window in around it.
    if advise {
        map.advise_random();
    }
    let len = map.len();
    if len < ZTRAILER as u64 || !len.is_multiple_of(ZPAGE as u64) {
        return None;
    }
    let last = map.slice_at(len - ZTRAILER as u64, ZTRAILER)?;
    if trailer_magic(last) == Some(STRIDE_MAGIC) {
        if *stride == StrideAttach::Refuse {
            return None;
        }
        let (t, lay) = StrideBody::open(&map.0).ok()?;
        same_segment(&t.seg_ident(), &blob.z_source_facts()?, id, end_seq).ok()?;
        return Some(FileFacts {
            bytes: len,
            pages: lay.v_pages,
            resident: len - lay.res_off,
            stride: true,
        });
    }
    let t = Trailer::decode(last).ok()?;
    if t.expected_file_len() != Some(len) {
        return None;
    }
    let facts = blob.z_source_facts()?;
    same_segment(&t.seg_ident(), &facts, id, end_seq).ok()?;
    Some(FileFacts {
        bytes: len,
        pages: t.n_pages,
        resident: len.saturating_sub(t.res_off),
        stride: false,
    })
}

/// The decodes' and walks' thread CPU over up to `pages` evenly spaced
/// pages of the form file at `path`, every group of each decoded and walked
/// as a read decodes and walks one, with the groups and records they held
/// and the faults and the wait the touches took: what a read through the
/// form pays a group, timed on the file as built, where a fill sample times
/// it on the pages it fills. `None` where the file does not open, is not
/// this segment's, or a page fails its own parse, decode or walk, and for a
/// stride file, which holds no group to time: the block trailer's decode
/// refuses its last page's magic (`Trailer::decode`). The reading times no
/// stride file (`FileFacts::stride`).
pub(super) struct DecodeTiming {
    pub decode_ns: u64,
    pub groups: u64,
    pub zgroups: u64,
    pub records: u64,
    pub wait_ns: u64,
    pub faults: u64,
}

pub(super) fn time_file_decodes(
    path: &Path,
    blob: &Blob<MmapBytes>,
    id: u64,
    end_seq: u64,
    pages: usize,
    own_decode: bool,
    advise: bool,
) -> Option<DecodeTiming> {
    let map = MmapBytes::open(path).ok()?;
    if advise {
        map.advise_random();
    }
    let len = map.len();
    if len < ZTRAILER as u64 || !len.is_multiple_of(ZPAGE as u64) {
        return None;
    }
    let last = map.slice_at(len - ZTRAILER as u64, ZTRAILER)?;
    let t = Trailer::decode(last).ok()?;
    let facts = blob.z_source_facts()?;
    same_segment(&t.seg_ident(), &facts, id, end_seq).ok()?;
    let bind = bind32(id, end_seq, facts.nkeys, facts.row_crc);
    let n_pages = usize::try_from(t.n_pages).ok()?;
    if n_pages == 0 || pages == 0 {
        return None;
    }
    let take = pages.min(n_pages);
    let mut want: Vec<usize> = (0..take)
        .map(|k| {
            if take == n_pages {
                k
            } else {
                k * (n_pages - 1) / (take - 1).max(1)
            }
        })
        .collect();
    want.dedup();
    let mut buf = GroupBuf::new();
    let mut d = DecodeTiming {
        decode_ns: 0,
        groups: 0,
        zgroups: 0,
        records: 0,
        wait_ns: 0,
        faults: 0,
    };
    let flt0 = thread_major_faults().unwrap_or(0);
    let cpu0 = super::thread_cpu_ns();
    let wall0 = std::time::Instant::now();
    for p in want {
        let page = map.slice_at((p as u64) * ZPAGE as u64, ZPAGE)?;
        let view = parse_page(page, bind).ok()?;
        let c0 = super::thread_cpu_ns();
        for j in 0..view.groups() {
            let group = decode_group_with(&view, j, &mut buf, own_decode).ok()?;
            let count = view.group(j).map_or(0, |g| usize::from(g.count));
            walk_group(group, count, None, None).ok()?;
            d.records += count as u64;
        }
        d.decode_ns = d
            .decode_ns
            .saturating_add(super::thread_cpu_ns().saturating_sub(c0));
        d.groups += 1;
        d.zgroups += view.groups() as u64;
    }
    d.faults = thread_major_faults().unwrap_or(0).saturating_sub(flt0);
    let cpu = super::thread_cpu_ns().saturating_sub(cpu0);
    d.wait_ns = (wall0.elapsed().as_nanos().min(u64::MAX as u128) as u64).saturating_sub(cpu);
    Some(d)
}

/// The platform's reading: the v1 memory controller's group where the
/// process has one (this host is a hybrid whose v1 line wins), else the
/// unified hierarchy's group with the least of `memory.max` and
/// `memory.high` over the path to its mount, else the machine's total and
/// the process's own anonymous memory; and `None` where even `/proc/meminfo`
/// cannot be read. Whichever it is, the budget is held under the machine's
/// `MemAvailable` less the margin (`Budget::within_available`), since
/// neither a group's limit nor the machine's total sees the other processes
/// beside the store. Off Linux there is no reading.
#[cfg(target_os = "linux")]
pub fn read_budget() -> Option<Budget> {
    budget_from(&|p: &Path| std::fs::read_to_string(p).ok())
}

/// `read_budget` over the files `read` gives, so a test can hand it a
/// machine this host is not.
#[cfg(target_os = "linux")]
fn budget_from(read: &dyn Fn(&Path) -> Option<String>) -> Option<Budget> {
    let meminfo = read(Path::new("/proc/meminfo"))?;
    let total = parse_meminfo_total(&meminfo)?;
    let available = parse_meminfo_available(&meminfo);
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
                return Some(
                    Budget::of(limit, st.rss.saturating_add(st.rss_huge), st.refaults)
                        .with_cache(st.cache)
                        .within_available(available),
                );
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
                return Some(
                    Budget::of(limit, st.anon, st.refaults)
                        .with_cache(st.file)
                        .within_available(available),
                );
            }
        }
    }
    let anon = read(Path::new("/proc/self/status"))
        .and_then(|t| parse_status_rss_anon(&t))
        .unwrap_or(0);
    Some(Budget::of(total, anon, None).within_available(available))
}

#[cfg(not(target_os = "linux"))]
pub fn read_budget() -> Option<Budget> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::zpages::ZCODING_RECORD;

    /// A machine of 16 GiB with `available` free of it, the process in a
    /// v1 group limited to `v1` or a v2 group limited to `v2` where given,
    /// and in neither otherwise; 100 MiB anonymous in every case.
    #[cfg(target_os = "linux")]
    fn machine(
        available: Option<u64>,
        v1: Option<u64>,
        v2: Option<u64>,
    ) -> impl Fn(&Path) -> Option<String> {
        move |p: &Path| {
            let anon = 100u64 << 20;
            let text = match p.to_str()? {
                "/proc/meminfo" => {
                    let mut t = format!("MemTotal: {} kB\nMemFree: 1 kB\n", 16u64 << 20);
                    if let Some(a) = available {
                        t += &format!("MemAvailable: {} kB\n", a >> 10);
                    }
                    t
                }
                "/proc/self/cgroup" => match (v1, v2) {
                    (Some(_), _) => "4:memory:/g\n0::/\n".to_string(),
                    (None, Some(_)) => "0::/g\n".to_string(),
                    (None, None) => "0::/\n".to_string(),
                },
                "/proc/mounts" => match (v1, v2) {
                    (Some(_), _) => "cgroup /cg cgroup rw,memory 0 0\n".to_string(),
                    (None, Some(_)) => "cgroup2 /cg2 cgroup2 rw 0 0\n".to_string(),
                    (None, None) => String::new(),
                },
                "/cg/g/memory.stat" => format!(
                    "rss {anon}\nrss_huge 0\nhierarchical_memory_limit {}\n",
                    v1?
                ),
                "/cg2/g/memory.stat" => {
                    v2?;
                    format!("anon {anon}\n")
                }
                "/cg2/g/memory.max" => format!("{}\n", v2?),
                "/proc/self/status" => format!("RssAnon: {} kB\n", anon >> 10),
                _ => return None,
            };
            Some(text)
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn every_reading_is_held_under_what_the_machine_has_available() {
        use crate::db::zselect::margin;
        let gib = 1u64 << 30;
        let anon = 100u64 << 20;
        // Each path's budget with room to spare, and then on a machine
        // whose other processes leave the store less than its limit.
        let cases = [
            ("v1", Some(4 * gib), None, 4 * gib),
            ("v2", None, Some(4 * gib), 4 * gib),
            ("host", None, None, 16 * gib),
        ];
        for (name, v1, v2, limit) in cases {
            let roomy = budget_from(&machine(Some(16 * gib), v1, v2)).expect(name);
            assert_eq!(roomy.limit, limit, "{name}");
            assert_eq!(roomy.budget, limit - anon - margin(limit), "{name}");
            let unsaid = budget_from(&machine(None, v1, v2)).expect(name);
            assert_eq!(unsaid, roomy, "{name}");
            let crowded = budget_from(&machine(Some(2 * gib), v1, v2)).expect(name);
            assert_eq!(crowded.budget, 2 * gib - margin(limit), "{name}");
            assert_eq!(
                (crowded.limit, crowded.anon),
                (roomy.limit, roomy.anon),
                "{name}"
            );
        }
    }

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

    /// The facts a form file on disk gives of itself are the build's --
    /// its bytes, its pages, its coding -- and the decodes timed on it count
    /// the groups and records of the pages they took; a file of another
    /// segment, by id, gives neither, and a name no file is at gives none.
    #[test]
    fn a_form_files_facts_are_the_builds_and_its_decodes_time_on_the_file() {
        let fx = Fixture::new("file-facts", suite(20_000, 0x5A5), 1 << 16);
        let Outcome::Built(b) = fx.build() else {
            panic!("the fixture builds")
        };
        let f = file_facts(&fx.out, &fx.blob, ID, END, true, &StrideAttach::Check)
            .expect("the file's facts");
        assert_eq!(f.bytes, b.file_bytes);
        assert_eq!(f.pages, b.fill.pages);
        assert!(!f.stride, "a block file");
        assert!(
            f.resident > 0 && f.resident < f.bytes,
            "{}/{}",
            f.resident,
            f.bytes
        );
        let s = sample(&fx.blob, ID, END, 0, FillShape::default()).unwrap();
        assert!(s.resident_est > 0, "{s:?}");
        let ratio = f.resident as f64 / s.resident_est as f64;
        assert!(
            (0.5..2.0).contains(&ratio),
            "the file's resident part {} against the sample's estimate {}",
            f.resident,
            s.resident_est
        );
        let d = time_file_decodes(&fx.out, &fx.blob, ID, END, 8, true, true).expect("timed");
        assert_eq!(d.groups, 8.min(b.fill.pages), "pages taken");
        assert!(
            d.zgroups >= d.groups && d.records >= d.zgroups,
            "{}/{}/{}",
            d.groups,
            d.zgroups,
            d.records
        );
        assert!(d.records <= 20_000);
        assert!(d.decode_ns > 0, "timed");
        // One page: the first; and more pages than the file has: every one.
        let one = time_file_decodes(&fx.out, &fx.blob, ID, END, 1, true, true).expect("timed");
        assert_eq!(one.groups, 1);
        let all =
            time_file_decodes(&fx.out, &fx.blob, ID, END, usize::MAX, true, true).expect("timed");
        assert_eq!(all.groups, b.fill.pages);
        assert_eq!(all.records, 20_000, "every record of every page");
        // Another segment's id: not this partition's file.
        assert!(file_facts(&fx.out, &fx.blob, ID + 1, END, true, &StrideAttach::Check).is_none());
        assert!(time_file_decodes(&fx.out, &fx.blob, ID + 1, END, 4, true, true).is_none());
        // No file.
        let missing = fx.out.with_extension("missing");
        assert!(file_facts(&missing, &fx.blob, ID, END, true, &StrideAttach::Check).is_none());
        assert!(time_file_decodes(&missing, &fx.blob, ID, END, 4, true, true).is_none());
    }

    /// A stride file's facts are its own trailer's, read by the last page's
    /// magic as the attach reads it: its length, its value pages and its
    /// row with the trailer as the resident part, and its body, for a store
    /// that takes the form; none for one that refuses it, as its attach
    /// refuses the file; none for another segment. Read as a block trailer,
    /// the file was missing to the controller's reading while the files'
    /// cover counted it on disk. It holds no group, and a decode timing of it
    /// is none, the block trailer's decode refusing its magic.
    #[test]
    fn a_stride_files_facts_are_its_trailers() {
        let fx = stride_fixture("stride-facts", 3000, 100, 0x3f);
        stride_built(&fx);
        let bytes = std::fs::read(&fx.out).unwrap();
        let (_, lay) = StrideBody::open(&bytes).unwrap();
        for stride in [StrideAttach::Check, StrideAttach::Trust(0)] {
            let f = file_facts(&fx.out, &fx.blob, ID, END, true, &stride).expect("its facts");
            assert_eq!(
                (f.bytes, f.pages, f.resident, f.stride),
                (
                    bytes.len() as u64,
                    lay.v_pages,
                    lay.file_len - lay.res_off,
                    true
                ),
                "{stride:?}"
            );
            assert!(file_facts(&fx.out, &fx.blob, ID + 1, END, true, &stride).is_none());
        }
        assert!(file_facts(&fx.out, &fx.blob, ID, END, true, &StrideAttach::Refuse).is_none());
        assert!(time_file_decodes(&fx.out, &fx.blob, ID, END, 8, true, true).is_none());
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
    use crate::db::zpages::{KeyKind, ZHDR};
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
        write_segment_with(path, recs, inline_max, &SegmentOptions::default());
    }

    /// `write_segment` under the writer's options as given.
    fn write_segment_with(path: &Path, recs: &[Rec], inline_max: usize, opts: &SegmentOptions) {
        let mut w = SegmentWriter::create(path, opts).unwrap();
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
            Fixture::written(name, recs, coding, raw_tails, |seg, recs| {
                write_segment(seg, recs, inline_max)
            })
        }

        /// A fixture over the segment `write` makes of `recs`.
        fn written(
            name: &str,
            recs: Vec<Rec>,
            coding: u8,
            raw_tails: bool,
            write: impl FnOnce(&Path, &[Rec]),
        ) -> Fixture {
            let dir = dir(name);
            let seg = dir.join("par-00000007-0000000000000099--.sup");
            write(&seg, &recs);
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
                &self.out,
                &self.blob,
                ID,
                END,
                false,
                true,
                false,
                true,
                false,
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
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
                &self.out,
                &self.blob,
                ID,
                END,
                advise,
                memo,
                prefetch,
                true,
                false,
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
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
                &self.out,
                &self.blob,
                ID,
                END,
                false,
                true,
                false,
                true,
                false,
                if fence_tree {
                    FenceSearch::Tree
                } else {
                    FenceSearch::Keys
                },
                memo_ahead,
                &StrideAttach::Check,
            )
        }

        /// Attached with the memo off: for a test of the decode path,
        /// whose second read of a page must decode it as the first did --
        /// a page changed under the mapping, a page a buffer cannot lend,
        /// a stored page beside a compressed one. With the memo on, a
        /// page's group is served as it was decoded while it holds.
        fn attach_nomemo(&self) -> std::result::Result<ZFile, Reject> {
            ZFile::attach(
                &self.out,
                &self.blob,
                ID,
                END,
                false,
                false,
                false,
                true,
                false,
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
            )
        }

        /// Attached as the store attaches a file under `stride`
        /// (`StrideAttach`), the rest as `attach` has it.
        fn attach_stride(&self, stride: StrideAttach) -> std::result::Result<ZFile, Reject> {
            ZFile::attach(
                &self.out,
                &self.blob,
                ID,
                END,
                false,
                true,
                false,
                true,
                false,
                FenceSearch::Keys,
                true,
                &stride,
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
    /// (`walk_run_ahead`) opens the next group for nobody: the memo is asked for
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
            .walk_run_ahead(None, count, 0, &mut |k: &[u8], _: &[u8]| {
                seen.push(k.to_vec())
            })
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
            .walk_run_ahead(None, 1, 0, &mut |k: &[u8], _: &[u8]| seen.push(k.to_vec()))
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
        cur.walk_run_ahead(None, count, 0, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        cur.advance();
        assert_eq!(cur.rank(), count + 1);
        assert_eq!(cur.key(), Some(fx.recs[count + 1].0.as_slice()));
        // And a key sought from one is sought from the next group's first.
        let mut cur = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
        cur.walk_run_ahead(None, count, 0, &mut |_: &[u8], _: &[u8]| {})
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
        let sample = sample_pages(pages);
        let sampled = |p: usize| sample.contains(&p);
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
        let sample = sample_pages(pages);
        let sampled = |p: usize| sample.contains(&p);
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
        let (group, _, count, decoded) =
            first_decode(p, bind, &entry, 0, &mut buf, Dec::Flex).unwrap();
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
                first_decode(p, bind, &entry, 0, &mut buf, Dec::Flex).unwrap_err(),
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

    /// The block-coded groups of a form file, and how many hold more than
    /// one record.
    fn block_groups(zfile: &ZFile) -> (u64, u64) {
        let (mut blocks, mut multi) = (0u64, 0u64);
        for p in 0..zfile.pages() {
            let v = page_view(zfile.page_bytes(p).unwrap()).unwrap();
            for j in 0..v.groups() {
                let g = v.group(j).unwrap();
                if !g.stored && !g.coded {
                    blocks += 1;
                    multi += u64::from(g.count > 1);
                }
            }
        }
        (blocks, multi)
    }

    /// The point read's partial decode (`Options::zpages_point_part`): on
    /// verified pages a group's first touch is decoded only as far as its
    /// key and put nowhere, its second is the whole decode the memo takes,
    /// and every probe answers as the segment answers it through both and
    /// through the hits after. Off -- the memo's flag, or the engine's own
    /// decoder -- no group is decoded in part.
    #[test]
    fn a_first_touched_group_is_decoded_in_part_and_the_memo_takes_it_at_its_second_touch() {
        for (name, recs, inline) in [
            ("part-suite", suite(3000, 11), 256),
            ("part-shapes", shapes(), 1 << 20),
        ] {
            let fx = Fixture::new(name, recs, inline);
            let mut zfile = fx.zfile();
            zfile.own_decoder = true;
            // Every page verified first, through a memo of its own: the
            // partial decode is for a page this process has verified.
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            assert_eq!(fx.memo.take_point(), PointCounts::default(), "{name}");
            let (blocks, _) = block_groups(&zfile);
            let suite = name == "part-suite";
            assert!(
                !suite || blocks > 100,
                "{name}: {blocks} block-coded groups"
            );

            let memo = ZMemo::new().point(true, false);
            agree(&memo, &zfile, &fx.blob, &fx.recs);
            let (hits, misses) = memo.take_counts();
            let p = memo.take_point();
            assert!(
                p.parts >= blocks,
                "{name}: every block-coded group's first touch is a partial decode: {p:?} of {blocks}"
            );
            assert!(p.part_bytes <= p.part_of, "{name}: {p:?}");
            assert!(
                misses <= p.admits,
                "{name}: the memo takes a group only at an admission: {misses} puts, {p:?}"
            );
            assert!(blocks == 0 || hits > 0, "{name}: {hits} hits");
            if suite {
                // Two groups in one slot of the memo displace each other,
                // and the displaced one's next touch is a first again: a
                // few groups in a few hundred, so the counts are held to
                // a tenth.
                assert_eq!(misses, p.admits, "{name}: an admission a put");
                assert!(
                    misses >= blocks * 9 / 10 && misses <= blocks + blocks / 10,
                    "{name}: every group read again is taken once: {misses} of {blocks}"
                );
                assert!(hits > misses, "{name}: and answers the reads after: {hits}");
                assert!(
                    p.part_bytes * 100 < p.part_of * 90,
                    "{name}: a partial decode stops short of its group: {p:?}"
                );
            }
            // Again: the groups read twice are the memo's now, and but for
            // the displaced few nothing is decoded at all.
            agree(&memo, &zfile, &fx.blob, &fx.recs);
            if suite {
                let (_, misses) = memo.take_counts();
                let parts = memo.take_point().parts;
                assert!(
                    misses + parts <= blocks / 10,
                    "{name}: hits: {misses} puts and {parts} partial decodes of {blocks} groups"
                );
            }
            assert!(!zfile.damaged() && zfile.bad_pages() == 0, "{name}");

            // The memo's flag off, or the engine's decoder off: whole
            // decodes, as before.
            let plain = ZMemo::new();
            agree(&plain, &zfile, &fx.blob, &fx.recs);
            assert_eq!(plain.take_point(), PointCounts::default(), "{name}");
            zfile.own_decoder = false;
            let other = ZMemo::new().point(true, false);
            agree(&other, &zfile, &fx.blob, &fx.recs);
            assert_eq!(
                other.take_point().parts,
                0,
                "{name}: lz4_flex decodes whole"
            );
        }
    }

    /// A page not yet verified is opened by its first decode, whole and
    /// with every check, whatever the memo's flag says: the partial decode
    /// begins at the page's second read.
    #[test]
    fn a_pages_first_read_is_its_whole_first_decode_and_a_partial_one_follows() {
        let fx = Fixture::new("part-first", suite(3000, 5), 256);
        let mut zfile = fx.zfile();
        zfile.own_decoder = true;
        let memo = ZMemo::new().point(true, false);
        let key = |i: usize| fx.recs[i].0.clone();
        // The first read of the file: its page is not verified.
        assert_eq!(zfile.verified_pages(), 0);
        assert_eq!(
            z_values(&memo, &zfile, &fx.blob, &key(0)),
            Some(raw_values(&fx.blob, &key(0)))
        );
        assert_eq!(zfile.verified_pages(), 1);
        assert_eq!(memo.take_point().parts, 0, "the first decode is whole");
        assert_eq!(memo.take_counts(), (0, 1), "and the memo's");
        // A key of another group of the page, one of several records: the
        // page is verified now, the group is not the memo's, and its first
        // touch is a partial decode.
        let v = page_view(zfile.page_bytes(0).unwrap()).unwrap();
        let g = (1..v.groups())
            .map(|j| v.group(j).unwrap())
            .find(|g| g.count >= 6)
            .expect("a second group of several records in the first page");
        assert!(!g.stored && !g.coded);
        let i = usize::from(g.first);
        assert_eq!(
            z_values(&memo, &zfile, &fx.blob, &key(i)),
            Some(raw_values(&fx.blob, &key(i)))
        );
        let p = memo.take_point();
        assert_eq!((p.parts, p.admits), (1, 0), "{p:?}");
        assert!(
            p.part_bytes < p.part_of,
            "the group's first record of several: {p:?}"
        );
        assert_eq!(
            memo.take_counts(),
            (0, 0),
            "nothing asked of the memo's slots"
        );
        // Its second touch is the whole decode, the memo's; its third a hit.
        assert!(z_values(&memo, &zfile, &fx.blob, &key(i + 1)).is_some());
        let p = memo.take_point();
        assert_eq!((p.parts, p.admits), (0, 1), "{p:?}");
        assert_eq!(memo.take_counts(), (0, 1));
        assert!(z_values(&memo, &zfile, &fx.blob, &key(i)).is_some());
        assert_eq!(memo.take_counts(), (1, 0));
        assert_eq!(memo.take_point(), PointCounts::default());
    }

    /// A walk's admission (`Options::zpages_scan_admit`): a cursor's first
    /// touch of a group is decoded into its own buffer and counted the
    /// walk's own, its second is the memo's, and its third a hit; every
    /// walk reads every key in order. Off, every open is the memo's.
    #[test]
    fn a_walks_first_touch_of_a_group_is_its_own_decode_and_its_second_the_memos() {
        let fx = Fixture::new("scan-admit", suite(3000, 29), 256);
        let zfile = fx.zfile();
        let (blocks, _) = block_groups(&zfile);
        assert!(blocks > 100, "{blocks} block-coded groups");
        let keys: Vec<Vec<u8>> = fx.recs.iter().map(|r| r.0.clone()).collect();
        let walk = |memo: &ZMemo| {
            let mut seen = Vec::new();
            let mut cur = ZCursor::at(memo, &zfile, &fx.blob, 0);
            let (n, _) = cur
                .walk_until(None, usize::MAX, &mut |k: &[u8], _: &[u8]| {
                    seen.push(k.to_vec())
                })
                .unwrap();
            assert!(!cur.fell_back());
            assert_eq!(n, keys.len());
            assert!(seen == keys, "every key in order");
        };
        let memo = ZMemo::new().scans(true, false);
        // The first walk: every page's first decode and every group after
        // it, into the cursor's buffer; nothing put.
        walk(&memo);
        assert_eq!(
            memo.take_counts(),
            (0, 0),
            "nothing asked for is the memo's yet"
        );
        let p = memo.take_point();
        assert_eq!(p.owned, blocks, "a decode a group, the walk's own");
        assert_eq!(p.admits, 0);
        // The second: every group's second touch, admitted and put.
        walk(&memo);
        let (hits, misses) = memo.take_counts();
        let p = memo.take_point();
        assert_eq!(misses, p.admits, "an admission a put");
        assert!(
            hits == 0 && misses >= blocks * 9 / 10 && misses <= blocks,
            "the groups' second touches: {hits} hits, {misses} puts of {blocks}"
        );
        assert_eq!(
            misses + p.owned,
            blocks,
            "every group decoded once, one way or the other"
        );
        // The third: the memo's, but for the few two groups in one slot
        // displaced.
        walk(&memo);
        let (hits, misses) = memo.take_counts();
        let p = memo.take_point();
        assert!(
            hits >= blocks * 9 / 10 && misses + p.owned <= blocks / 10,
            "{hits} hits, {misses} puts and {} own decodes of {blocks}",
            p.owned
        );
        // Off: every open of a walk is a put, then a hit.
        let plain = ZMemo::new();
        walk(&plain);
        assert_eq!(plain.take_counts(), (0, blocks));
        assert_eq!(
            plain.take_point(),
            PointCounts::default(),
            "none the walk's own"
        );
    }

    /// One read of a form file as a block's reader makes it, for the test
    /// below: a cursor placed at `start`, a run of `limit`, and then the
    /// position read every way -- its rank, the key and the values under
    /// it, a short run more, a step, and a key sought a few records on.
    /// What was read, the ranks it stood at, and the memo's counts as the
    /// first run left them.
    #[allow(clippy::type_complexity)]
    fn read_every_way(
        memo: &ZMemo,
        zfile: &ZFile,
        blob: &Blob<MmapBytes>,
        recs: &[Rec],
        start: usize,
        limit: usize,
        run: bool,
    ) -> (Vec<(Vec<u8>, Vec<u8>)>, Vec<usize>, PointCounts) {
        let mut seen: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut marks = Vec::new();
        let mut cur = ZCursor::at_wanting(memo, zfile, blob, start, limit);
        let (got, _) = {
            let mut f = |k: &[u8], v: &[u8]| seen.push((k.to_vec(), v.to_vec()));
            if run {
                cur.walk_run_ahead(None, limit, 0, &mut f).unwrap()
            } else {
                cur.walk_until(None, limit, &mut f).unwrap()
            }
        };
        let counts = memo.take_point();
        marks.extend([got, cur.rank()]);
        cur.resume();
        if let Some(k) = cur.key().map(|k| k.to_vec()) {
            let mut vals = Vec::new();
            cur.values(&mut |v: &[u8]| vals.extend_from_slice(v))
                .unwrap();
            seen.push((k, vals));
        }
        let (more, _) = cur
            .walk_run_ahead(None, 3, 0, &mut |k: &[u8], v: &[u8]| {
                seen.push((k.to_vec(), v.to_vec()))
            })
            .unwrap();
        marks.extend([more, cur.rank()]);
        cur.advance();
        marks.push(cur.rank());
        if let Some(k) = cur.key() {
            seen.push((k.to_vec(), Vec::new()));
        }
        let target = (cur.rank() + 5).min(recs.len() - 1);
        let at = cur.skip_to(&recs[target].0);
        marks.extend([
            cur.rank(),
            match at {
                Some(KeyOrder::Equal) => 1,
                Some(_) => 2,
                None => 0,
            },
        ]);
        assert!(!cur.fell_back(), "start {start} limit {limit}");
        (seen, marks, counts)
    }

    /// A walk's partial decode (`Options::zpages_walk_part`): a walk that
    /// wants fewer records of a group than it holds begins the group's
    /// decode and takes it only as far as it reads, and every read after --
    /// another run, a step, the key and the values under the position, a
    /// key sought -- finds what a whole decode finds. From starts over the
    /// whole file and limits from one record to several groups, by either
    /// walk, the read in part hands over what the whole one hands over and
    /// stands where it stands; the groups begun in part are decoded short
    /// of their length, the ones walked to their ends whole; and off --
    /// the memo's flag, a walk with no decode of its own, or the other
    /// decoder -- nothing is decoded in part.
    #[test]
    fn a_walk_decodes_a_group_it_does_not_finish_only_as_far_as_it_reads() {
        let fx = Fixture::new("walk-part", suite(1200, 43), 256);
        let mut zfile = fx.zfile();
        // As the store sets them: the engine's own decoder, and the page's
        // view held across its groups, which the partial open reads.
        zfile.own_decoder = true;
        zfile.cursor_view = true;
        let n = fx.recs.len();
        // Every page verified first, through a memo of its own: the partial
        // decode is for a page this process has verified.
        {
            let warm = ZMemo::new();
            let mut cur = ZCursor::at(&warm, &zfile, &fx.blob, 0);
            let (got, _) = cur
                .walk_until(None, usize::MAX, &mut |_: &[u8], _: &[u8]| {})
                .unwrap();
            assert_eq!(got, n);
        }
        let (mut reads, mut parts, mut bytes, mut of, mut owned) = (0u64, 0u64, 0u64, 0u64, 0u64);
        for start in (0..n).step_by(7) {
            for limit in [1usize, 2, 3, 5, 9, 14, 23, 40, 77] {
                for run in [true, false] {
                    let part = ZMemo::new().scans(true, true);
                    let whole = ZMemo::new().scans(true, false);
                    let a = read_every_way(&part, &zfile, &fx.blob, &fx.recs, start, limit, run);
                    let b = read_every_way(&whole, &zfile, &fx.blob, &fx.recs, start, limit, run);
                    assert!(
                        a.0 == b.0 && a.1 == b.1,
                        "start {start} limit {limit} run {run}: {:?} against {:?}",
                        a.1,
                        b.1
                    );
                    let got = a.1[0];
                    assert_eq!(got, limit.min(n - start));
                    for (i, (k, v)) in a.0[..got].iter().enumerate() {
                        let r = &fx.recs[start + i];
                        assert!(
                            k == &r.0 && v == &r.1[0],
                            "start {start} limit {limit}: record {i}"
                        );
                    }
                    assert_eq!(b.2.walk_parts, 0, "off, none in part");
                    assert!(a.2.walk_parts <= a.2.owned, "{:?}", a.2);
                    assert!(a.2.walk_part_bytes <= a.2.walk_part_of, "{:?}", a.2);
                    reads += 1;
                    parts += a.2.walk_parts;
                    bytes += a.2.walk_part_bytes;
                    of += a.2.walk_part_of;
                    owned += a.2.owned;
                }
            }
        }
        assert!(
            parts * 4 > reads,
            "{parts} groups begun in part over {reads} reads"
        );
        assert!(
            bytes * 100 < of * 80,
            "{bytes} of {of} bytes decoded of the groups begun in part"
        );
        assert!(
            owned > parts,
            "the groups walked to their ends decoded whole: {owned} owned, {parts} in part"
        );
        // One walk taken apart: a cursor placed at a page's first record,
        // not as a walk, opens its group whole, and a run two records
        // into the next group begins that one and no other.
        let v = page_view(zfile.page_bytes(0).unwrap()).unwrap();
        let (c0, g1) = (usize::from(v.group(0).unwrap().count), v.group(1).unwrap());
        assert!(g1.count > 8 && !g1.stored && !g1.coded, "{g1:?}");
        let memo = ZMemo::new().scans(true, true);
        let mut cur = ZCursor::at(&memo, &zfile, &fx.blob, 0);
        assert_eq!(
            memo.take_point().walk_parts,
            0,
            "a placement wants the group"
        );
        let (got, _) = cur
            .walk_run_ahead(None, c0 + 2, 0, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(got, c0 + 2);
        let p = memo.take_point();
        assert_eq!((p.walk_parts, p.owned), (1, 1), "{p:?}");
        assert_eq!(p.walk_part_of, u64::from(g1.ulen));
        assert!(
            p.walk_part_bytes * 2 < p.walk_part_of,
            "three records' worth of {}: {p:?}",
            g1.count
        );
        drop(cur);
        // The same run by a reader with the rest of its scan ahead: the
        // group is wanted whole, and is opened so.
        let memo = ZMemo::new().scans(true, true);
        let mut cur = ZCursor::at(&memo, &zfile, &fx.blob, 0);
        memo.take_point();
        let (got, _) = cur
            .walk_run_ahead(None, c0 + 2, 100, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(got, c0 + 2);
        let p = memo.take_point();
        assert_eq!((p.walk_parts, p.owned), (0, 1), "{p:?}");
        drop(cur);
        // A run that ends at a group's last record leaves the next group
        // unopened, and the run after it, wanting two records, opens it
        // as it resumes: in part, by that run's count.
        let memo = ZMemo::new().scans(true, true);
        let mut cur = ZCursor::at(&memo, &zfile, &fx.blob, 0);
        memo.take_point();
        let (got, _) = cur
            .walk_run_ahead(None, c0, 0, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(got, c0);
        let p = memo.take_point();
        assert_eq!((p.walk_parts, p.owned), (0, 0), "nothing opened: {p:?}");
        let (got, _) = cur
            .walk_run_ahead(None, 2, 0, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(got, 2);
        let p = memo.take_point();
        assert_eq!((p.walk_parts, p.owned, p.walk_mores), (1, 1, 0), "{p:?}");
        assert!(
            p.walk_part_bytes * 2 < p.walk_part_of,
            "three records' worth of {}: {p:?}",
            g1.count
        );
        drop(cur);
        // A placement inside a group, told its walk wants two records:
        // the group is begun in part as far as the three records skipped
        // and those two, in one step, and the walk of the two decodes
        // nothing more.
        let memo = ZMemo::new().scans(true, true);
        let mut cur = ZCursor::at_wanting(&memo, &zfile, &fx.blob, c0 + 3, 2);
        let p = memo.take_point();
        assert_eq!((p.walk_parts, p.owned, p.walk_mores), (1, 1, 0), "{p:?}");
        let rec = u64::from(g1.ulen) / u64::from(g1.count);
        assert!(
            p.walk_part_bytes >= 5 * rec && p.walk_part_bytes <= 8 * rec,
            "five records' worth of {rec} bytes each and a decoder's run past them: {p:?}"
        );
        let (got, _) = cur
            .walk_run_ahead(None, 2, 0, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(got, 2);
        let p = memo.take_point();
        assert_eq!(
            (p.walk_parts, p.walk_mores, p.walk_part_bytes),
            (0, 0, 0),
            "the two were decoded at the placement: {p:?}"
        );
        drop(cur);
        // A walk with no decode of its own has none to cut short.
        let plain = ZMemo::new().scans(false, true);
        let c = read_every_way(&plain, &zfile, &fx.blob, &fx.recs, 3, 20, true);
        assert_eq!((c.2.walk_parts, c.2.owned), (0, 0), "{:?}", c.2);
        // The other decoder decodes whole, and so does a cursor that holds
        // no view.
        zfile.own_decoder = false;
        let other = ZMemo::new().scans(true, true);
        let c = read_every_way(&other, &zfile, &fx.blob, &fx.recs, 3, 20, true);
        assert!(c.2.owned > 0 && c.2.walk_parts == 0, "{:?}", c.2);
        zfile.own_decoder = true;
        zfile.cursor_view = false;
        let viewless = ZMemo::new().scans(true, true);
        let c = read_every_way(&viewless, &zfile, &fx.blob, &fx.recs, 3, 20, true);
        assert!(c.2.owned > 0 && c.2.walk_parts == 0, "{:?}", c.2);
    }

    /// Records of uneven sizes: every ninth and tenth carries a value thirty
    /// times the others', inline, so a group's bytes are not its records'
    /// count times a mean.
    fn uneven(n: usize) -> Vec<Rec> {
        (0..n)
            .map(|i| {
                let len = if i % 9 < 2 { 360 } else { 12 };
                (
                    format!("{i:016}").into_bytes(),
                    vec![vec![b'a' + (i % 23) as u8; len]],
                    false,
                )
            })
            .collect()
    }

    /// The walk's partial decode sizes its first stop by the group's mean
    /// record, and over records of uneven sizes the stop falls short: the
    /// decode is taken on where a record runs past what stands -- at a
    /// placement that skips into the group, in a walk, under a step -- and
    /// a group left in part is never the state the next group is read
    /// through. Random sequences of runs with and without a scan ahead of
    /// them, walks, steps, the key and values under the position and keys
    /// sought, from random starts, by a cursor that decodes in part and by
    /// one that decodes whole: the two hand over the same entries and
    /// stand at the same ranks after every step, and neither falls back.
    #[test]
    fn a_walk_in_part_over_uneven_records_reads_what_a_whole_decode_reads() {
        let fx = Fixture::new("walk-part-uneven", uneven(900), 512);
        let mut zfile = fx.zfile();
        zfile.own_decoder = true;
        zfile.cursor_view = true;
        let n = fx.recs.len();
        {
            let warm = ZMemo::new();
            let mut cur = ZCursor::at(&warm, &zfile, &fx.blob, 0);
            let (got, _) = cur
                .walk_until(None, usize::MAX, &mut |_: &[u8], _: &[u8]| {})
                .unwrap();
            assert_eq!(got, n);
        }
        let mut rng = Rng(0x05ee_d150);
        let (mut parts, mut mores, mut placed_mores) = (0u64, 0u64, 0u64);
        for round in 0..1500 {
            let start = rng.below(n);
            let want = [1usize, 2, 3, 5, 8, 13, 30][rng.below(7)];
            let part = ZMemo::new().scans(true, true);
            let whole = ZMemo::new().scans(true, false);
            let mut a = ZCursor::at_wanting(&part, &zfile, &fx.blob, start, want);
            let mut b = ZCursor::at_wanting(&whole, &zfile, &fx.blob, start, want);
            placed_mores += part.take_point().walk_mores;
            let mut steps = Vec::new();
            for _ in 0..7 {
                if a.rank() >= n {
                    break;
                }
                let op = rng.below(6);
                let (limit, ahead, d) = (
                    [1usize, 2, 3, 5, 8, 13, 30][rng.below(7)],
                    [0usize, 0, 100][rng.below(3)],
                    rng.below(12),
                );
                steps.push((op, limit, ahead, d));
                let mut out: [Vec<(Vec<u8>, Vec<u8>)>; 2] = [Vec::new(), Vec::new()];
                let mut at = [0usize; 2];
                for (i, cur) in [&mut a, &mut b].into_iter().enumerate() {
                    let seen = &mut out[i];
                    match op {
                        0 | 1 => {
                            cur.walk_run_ahead(None, limit, ahead, &mut |k: &[u8], v: &[u8]| {
                                seen.push((k.to_vec(), v.to_vec()))
                            })
                            .unwrap();
                        }
                        2 => {
                            cur.walk_until(None, limit, &mut |k: &[u8], v: &[u8]| {
                                seen.push((k.to_vec(), v.to_vec()))
                            })
                            .unwrap();
                        }
                        3 => {
                            cur.resume();
                            if let Some(k) = cur.key().map(|k| k.to_vec()) {
                                let mut vals = Vec::new();
                                cur.values(&mut |v: &[u8]| vals.extend_from_slice(v))
                                    .unwrap();
                                seen.push((k, vals));
                            }
                        }
                        4 => {
                            cur.resume();
                            cur.advance();
                        }
                        _ => {
                            cur.resume();
                            let target = (cur.rank() + d).min(n - 1);
                            cur.skip_to(&fx.recs[target].0);
                        }
                    }
                    at[i] = cur.rank();
                    assert!(!cur.fell_back(), "round {round}, start {start}: {steps:?}");
                }
                assert!(
                    out[0] == out[1] && at[0] == at[1],
                    "round {round}, start {start} wanting {want}: {steps:?}: ranks {at:?}"
                );
            }
            let p = part.take_point();
            parts += p.walk_parts;
            mores += p.walk_mores;
            assert_eq!(whole.take_point().walk_parts, 0);
        }
        // The premises: groups were begun in part, taken on in walks, and
        // taken on at a placement that skipped into one.
        assert!(
            parts > 500 && mores > 100 && placed_mores > 10,
            "{parts} begun in part, {mores} taken on, {placed_mores} at a placement"
        );
    }

    /// The bulk walk's own decode of the group it ends inside goes only as
    /// far as it reads, and on a step at a time where a record runs past
    /// the guess (`Options::zpages_bulk_part`): over records of uneven
    /// sizes, scans from random starts of random lengths through a memo
    /// that decodes in part and one that decodes whole hand over the same
    /// entries and neither falls back, and the first begins groups in part
    /// and takes some of them on.
    #[test]
    fn a_bulk_walk_in_part_over_uneven_records_reads_what_a_whole_decode_reads() {
        let fx = Fixture::new("bulk-part-uneven", uneven(900), 512);
        let mut zfile = fx.zfile();
        zfile.own_decoder = true;
        let n = fx.recs.len();
        {
            // Every page verified by its first decode, which a decode in
            // part waits for.
            let warm = ZMemo::new();
            let got = zfile
                .scan_at(&warm, &fx.blob, 0, usize::MAX, &mut |_: &[u8], _: &[u8]| {})
                .unwrap();
            assert_eq!(got.seen, n);
        }
        let mut rng = Rng(0x0b01_c5a1);
        let (mut parts, mut mores) = (0u64, 0u64);
        for round in 0..1500 {
            let start = rng.below(n);
            let want = [1usize, 2, 3, 5, 8, 13, 30, 100][rng.below(8)];
            let part = ZMemo::new().bulk(true, false, true);
            let whole = ZMemo::new().bulk(true, false, false);
            let mut out: [Vec<(Vec<u8>, Vec<u8>)>; 2] = [Vec::new(), Vec::new()];
            for (i, memo) in [&part, &whole].into_iter().enumerate() {
                let seen = &mut out[i];
                let r = zfile
                    .scan_at(memo, &fx.blob, start, want, &mut |k: &[u8], v: &[u8]| {
                        seen.push((k.to_vec(), v.to_vec()))
                    })
                    .unwrap();
                assert_eq!(
                    (r.end, r.seen),
                    (ScanEnd::Served, want.min(n - start)),
                    "round {round}: start {start} wanting {want}"
                );
            }
            assert!(
                out[0] == out[1],
                "round {round}: start {start} wanting {want}"
            );
            let p = part.take_point();
            parts += p.walk_parts;
            mores += p.walk_mores;
            assert_eq!(whole.take_point().walk_parts, 0);
        }
        assert!(
            parts > 500 && mores > 50,
            "{parts} begun in part, {mores} taken on"
        );
    }

    /// A page this process has not verified is not decoded in part: its
    /// first decode is the whole one, which holds the page to its checks.
    /// A byte of the last group of a page the attach does not sample is
    /// flipped, and a walk crosses into the page wanting three records of
    /// its first group, which a decode in part would hand over from a page
    /// nothing had checked: the page fails its first decode instead, the
    /// form file leaves service, and the walk's records are the raw form's,
    /// every one right.
    #[test]
    fn a_walk_does_not_decode_a_page_in_part_before_its_first_decode() {
        let fx = Fixture::new("walk-part-unverified", suite(4000, 17), 256);
        assert!(matches!(fx.build(), Outcome::Built(_)));
        let nkeys = fx.recs.len();
        let (at, first_on_p) = {
            let zfile = fx.attach().unwrap();
            let pages = zfile.pages();
            assert!(
                pages > 3 * ZSAMPLE_PAGES,
                "pages enough to leave some unsampled"
            );
            let sample = sample_pages(pages);
            let sampled = |p: usize| sample.contains(&p);
            let unsampled: Vec<usize> = (1..pages - 1).filter(|&p| !sampled(p)).collect();
            let p = unsampled[unsampled.len() / 2];
            let v = page_view(zfile.page_bytes(p).unwrap()).unwrap();
            assert!(v.groups() > 1, "a page of several groups");
            let (first, last) = (v.group(0).unwrap(), v.group(v.groups() - 1).unwrap());
            assert!(
                !first.stored && !first.coded && first.count > 4,
                "{first:?}"
            );
            assert!(!last.stored && !last.coded, "{last:?}");
            let first_on_p = (0..zfile.fence.len())
                .find_map(|e| {
                    zfile
                        .fence
                        .entry(e, nkeys as u32)
                        .filter(|en| en.page as usize == p)
                        .map(|en| en.first_rank as usize)
                })
                .expect("a page the fence names");
            (
                p * ZPAGE + usize::from(last.coff) + usize::from(last.clen) / 2,
                first_on_p,
            )
        };
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fx.out)
            .unwrap();
        let mut b = [0u8; 1];
        f.read_at(&mut b, at as u64).unwrap();
        f.write_at(&[b[0] ^ 0x5a], at as u64).unwrap();
        drop(f);
        let mut zfile = fx
            .attach()
            .expect("a damaged page the sample misses attaches");
        zfile.own_decoder = true;
        zfile.cursor_view = true;
        assert!(first_on_p > 2 && first_on_p + 3 < nkeys);
        let memo = ZMemo::new().scans(true, true);
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut cur = ZCursor::at(&memo, &zfile, &fx.blob, first_on_p - 2);
        assert!(
            !cur.fell_back() && !zfile.damaged(),
            "the page before is sound"
        );
        let (got, _) = cur
            .walk_run_ahead(None, 5, 0, &mut |k: &[u8], _: &[u8]| seen.push(k.to_vec()))
            .unwrap();
        assert_eq!(got, 5);
        let want: Vec<Vec<u8>> = fx.recs[first_on_p - 2..first_on_p + 3]
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(
            seen == want,
            "every record, through the raw form past the damage"
        );
        assert!(
            cur.fell_back() && zfile.damaged(),
            "the page failed its first decode"
        );
        assert_eq!(memo.take_point().walk_parts, 0, "nothing decoded in part");
    }

    /// The bulk walk does not decode in part a page this process has not
    /// verified (`Options::zpages_bulk_part`), as the cursor does not: a
    /// byte of the last group of a page the attach does not sample is
    /// flipped, and a bulk walk crosses into the page wanting three records
    /// of its first group, which a decode in part would hand over from a
    /// page nothing had checked. The page fails its first decode instead,
    /// the walk stops at the page with the records before it handed over
    /// and right, the form file leaves service, and nothing was decoded in
    /// part.
    #[test]
    fn a_bulk_walk_does_not_decode_a_page_in_part_before_its_first_decode() {
        let fx = Fixture::new("bulk-part-unverified", suite(4000, 17), 256);
        assert!(matches!(fx.build(), Outcome::Built(_)));
        let nkeys = fx.recs.len();
        let (at, first_on_p) = {
            let zfile = fx.attach().unwrap();
            let pages = zfile.pages();
            assert!(
                pages > 3 * ZSAMPLE_PAGES,
                "pages enough to leave some unsampled"
            );
            let sample = sample_pages(pages);
            let sampled = |p: usize| sample.contains(&p);
            let unsampled: Vec<usize> = (1..pages - 1).filter(|&p| !sampled(p)).collect();
            let p = unsampled[unsampled.len() / 2];
            let v = page_view(zfile.page_bytes(p).unwrap()).unwrap();
            assert!(v.groups() > 1, "a page of several groups");
            let (first, last) = (v.group(0).unwrap(), v.group(v.groups() - 1).unwrap());
            assert!(
                !first.stored && !first.coded && first.count > 4,
                "{first:?}"
            );
            assert!(!last.stored && !last.coded, "{last:?}");
            let first_on_p = (0..zfile.fence.len())
                .find_map(|e| {
                    zfile
                        .fence
                        .entry(e, nkeys as u32)
                        .filter(|en| en.page as usize == p)
                        .map(|en| en.first_rank as usize)
                })
                .expect("a page the fence names");
            (
                p * ZPAGE + usize::from(last.coff) + usize::from(last.clen) / 2,
                first_on_p,
            )
        };
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fx.out)
            .unwrap();
        let mut b = [0u8; 1];
        f.read_at(&mut b, at as u64).unwrap();
        f.write_at(&[b[0] ^ 0x5a], at as u64).unwrap();
        drop(f);
        let mut zfile = fx
            .attach()
            .expect("a damaged page the sample misses attaches");
        zfile.own_decoder = true;
        assert!(first_on_p > 2 && first_on_p + 3 < nkeys);
        let memo = ZMemo::new().bulk(true, false, true);
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let r = zfile
            .scan_at(
                &memo,
                &fx.blob,
                first_on_p - 2,
                5,
                &mut |k: &[u8], _: &[u8]| seen.push(k.to_vec()),
            )
            .unwrap();
        let before: Vec<Vec<u8>> = fx.recs[first_on_p - 2..first_on_p]
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert_eq!(
            (r.end, r.seen, &seen),
            (ScanEnd::Fallback, 2, &before),
            "the walk stops at the damaged page with the records before it"
        );
        assert!(zfile.damaged(), "the page failed its first decode");
        assert_eq!(memo.take_point().walk_parts, 0, "nothing decoded in part");
    }

    /// The tag filter: a group's first ask is refused and leaves its tag,
    /// its second while the tag stands is admitted and clears it, and a
    /// group whose slot another took between its two asks starts again.
    #[test]
    fn the_memo_admits_a_group_at_its_second_ask_while_its_tag_stands() {
        let mut m = Memo::new(8, 1 << 20);
        assert!(!m.admit(1, 0, 0));
        assert!(m.admit(1, 0, 0));
        assert_eq!(m.point.admits, 1);
        assert!(!m.admit(1, 0, 0), "the tag went with the admission");
        // Another form file's same group, and the page's next group, are
        // tags of their own.
        assert!(!m.admit(2, 0, 0) && !m.admit(1, 0, 1));
        assert!(m.admit(2, 0, 0) && m.admit(1, 0, 1));
        // A group in the same slot overwrites the tag, found over the
        // pages: the first group's next ask is a first again.
        let slot = |z: u64, p: u32| (Memo::group_hash(z, p, 0) >> 50) as usize & (ZADMIT_SLOTS - 1);
        let other = (1u32..)
            .find(|&p| slot(7, p) == slot(7, 0))
            .expect("two pages in one slot");
        assert!(!m.admit(7, 0, 0));
        assert!(!m.admit(7, other, 0), "the slot's other group");
        assert!(!m.admit(7, 0, 0), "its tag was overwritten");
        assert!(m.admit(7, 0, 0));
        assert_eq!(m.point.admits, 4);
        assert_eq!(m.admit.len(), ZADMIT_SLOTS);
    }

    /// The key table (`Options::zpages_point_keys`): a key read through
    /// the memo is sent to its record at its next read, with the memo's
    /// group asked and the key compared there; a slot that names another
    /// record, a group the memo let go, and another attach of the same
    /// file are each the search, and every answer is the segment's.
    #[test]
    fn a_key_read_through_the_memo_is_sent_to_its_record_and_a_stale_place_is_the_search() {
        let fx = Fixture::new("point-keys", suite(3000, 17), 256);
        let mut zfile = fx.zfile();
        zfile.own_decoder = true;
        let keys: Vec<Vec<u8>> = fx.recs.iter().map(|r| r.0.clone()).collect();
        let read_all = |memo: &ZMemo, z: &ZFile| {
            for k in &keys {
                assert_eq!(
                    z_values(memo, z, &fx.blob, k),
                    Some(raw_values(&fx.blob, k)),
                    "read of {k:?}"
                );
                assert_eq!(served(z.count(memo, k)), fx.blob.count(k).unwrap());
            }
        };
        // The table alone, every miss a whole decode the memo takes: the
        // first read of a key finds its record by the search and leaves its
        // place, and the count beside it and every read after are sent
        // there.
        let memo = ZMemo::new().point(false, true);
        read_all(&memo, &zfile);
        let n = keys.len() as u64;
        let first = memo.take_point();
        assert!(
            first.key_hits >= n * 9 / 10 && first.key_hits <= n,
            "the count after each read: {first:?} of {n}"
        );
        read_all(&memo, &zfile);
        let again = memo.take_point();
        assert!(
            again.key_hits >= 2 * n * 9 / 10,
            "both reads of nearly every key: {again:?} of {n}"
        );
        // Probes around the keys are no key's record: the table sends
        // none of them anywhere it answers from.
        agree(&memo, &zfile, &fx.blob, &fx.recs);

        // A place that names another key's record, forged: the compare
        // there refuses it and the search answers.
        let (k0, k1) = (&keys[10], &keys[11]);
        let h0 = crate::db::BlockedBloom::hash(k0);
        let h1 = crate::db::BlockedBloom::hash(k1);
        let at1 = memo.key_at(zfile.id, h1).expect("a key just read");
        memo.key_put(zfile.id, h0, at1.0, at1.1, at1.2);
        memo.take_point();
        assert_eq!(
            z_values(&memo, &zfile, &fx.blob, k0),
            Some(raw_values(&fx.blob, k0))
        );
        assert_eq!(memo.take_point().key_hits, 0, "another key's record");
        // The search left the key's own place again.
        assert!(z_values(&memo, &zfile, &fx.blob, k0).is_some());
        assert_eq!(memo.take_point().key_hits, 1);
        // An offset that is no record's start in the group: refused the
        // same way.
        memo.key_put(zfile.id, h0, at1.0, at1.1, at1.2 + 2);
        assert_eq!(
            z_values(&memo, &zfile, &fx.blob, k0),
            Some(raw_values(&fx.blob, k0))
        );
        assert_eq!(memo.take_point().key_hits, 0);

        // A memo of one slot: a key's place outlives the group it names,
        // and a place whose group the memo let go is the search.
        let small = ZMemo::with(1, 1 << 20).point(false, true);
        let (ka, kb) = (&keys[0], &keys[keys.len() - 1]);
        assert!(z_values(&small, &zfile, &fx.blob, ka).is_some());
        assert!(z_values(&small, &zfile, &fx.blob, kb).is_some());
        assert!(
            small
                .key_at(zfile.id, crate::db::BlockedBloom::hash(ka))
                .is_some(),
            "the first key's place stands"
        );
        small.take_point();
        assert_eq!(
            z_values(&small, &zfile, &fx.blob, ka),
            Some(raw_values(&fx.blob, ka))
        );
        assert_eq!(small.take_point().key_hits, 0, "its group was displaced");
        read_all(&small, &zfile);
        read_all(&small, &zfile);

        // Another attach of the same file is another identity: none of the
        // places the first left is its.
        let mut second = fx.attach().unwrap();
        second.own_decoder = true;
        assert_ne!(second.id, zfile.id);
        memo.take_point();
        for k in keys.iter().step_by(13) {
            assert!(memo
                .key_at(second.id, crate::db::BlockedBloom::hash(k))
                .is_none());
        }
        // Both options together answer every probe, twice over.
        let both = ZMemo::new().point(true, true);
        agree(&both, &zfile, &fx.blob, &fx.recs);
        agree(&both, &zfile, &fx.blob, &fx.recs);
        let p = both.take_point();
        assert!(p.parts > 0 && p.admits > 0 && p.key_hits > 0, "{p:?}");
        assert!(!zfile.damaged() && zfile.bad_pages() == 0);
    }

    /// With a slab (`Options::zpages_memo_slab`) every buffer the memo
    /// makes for a decode is one of its cells, sized to a group and the
    /// decoder's room: a displaced group's cell is the next decode's, a
    /// group a read still holds keeps its cell and its bytes while the
    /// slot takes another, a memo with every cell out hands a buffer of
    /// the allocator's, and a group held past the memo's drop still reads.
    /// Without one, no buffer is a cell.
    #[test]
    fn a_memo_with_a_slab_keeps_its_groups_in_cells() {
        let cell = memo_cell_bytes(ZGROUP_BYTES).expect("a cell for the shipped cut");
        assert_eq!(cell, ZGROUP_BYTES + crate::db::zlz4::FLAT_SLACK);
        assert_eq!(memo_cell_bytes(0), None, "a page as one group keeps none");
        assert_eq!(memo_cell_bytes(ZPAGE), None);
        let mut m = Memo::new(4, 3 * 2000);
        m.slab_cell = cell;
        // A decode: a buffer taken, marked, and put as one group of a
        // page. Unhashed, a page's groups take a slot each.
        let put = |m: &mut Memo, index: u8, mark: u8| -> usize {
            let mut b = m.take_free();
            assert!(b.is_cell(), "group {index}: a cell");
            assert!(b.capacity_bytes() >= cell && b.capacity_bytes() < ZPAGE);
            b.stride_key()[0] = mark;
            let at = b.bytes(1).as_ptr() as usize;
            m.put(1, 0, index, false, Arc::new(b), 2000, 0, 1);
            at
        };
        let mark = |m: &mut Memo, index: u8| m.get(1, 0, index, false).map(|(g, ..)| g.bytes(1)[0]);
        let at: Vec<usize> = (0..3u8).map(|i| put(&mut m, i, 10 + i)).collect();
        assert!(at[0] != at[1] && at[1] != at[2] && at[0] != at[2]);
        assert_eq!(
            (mark(&mut m, 0), mark(&mut m, 1), mark(&mut m, 2)),
            (Some(10), Some(11), Some(12))
        );
        // A fourth is past the cap: the sweep lets one go, and its cell is
        // the next decode's.
        let fourth = put(&mut m, 3, 13);
        assert!(!at.contains(&fourth), "three held, a fourth cell");
        assert_eq!(m.free.len(), 1, "the displaced group's buffer kept");
        let gone = (0..3u8).find(|&i| m.get(1, 0, i, false).is_none()).unwrap();
        let fifth = put(&mut m, 3, 14);
        assert_eq!(fifth, at[gone as usize], "the displaced cell, used again");
        assert_eq!(mark(&mut m, 3), Some(14));
        // A read holds a group while its slot takes another: the group
        // keeps its cell and its bytes.
        let held = m.get(1, 0, 3, false).unwrap().0;
        let sixth = put(&mut m, 3, 15);
        assert_ne!(sixth, fifth, "the held group's cell is not handed out");
        assert_eq!((held.bytes(1)[0], mark(&mut m, 3)), (14, Some(15)));
        // Every cell out: the slots', the free buffers' and the held
        // ones'. The next buffer is the allocator's, and none after it is
        // a cell until one comes back.
        let mut out = Vec::new();
        loop {
            let b = m.take_free();
            if !b.is_cell() {
                break;
            }
            out.push(b);
            assert!(
                out.len() <= 4 + ZMEMO_FREE + ZMEMO_HELD,
                "more cells than the slab has"
            );
        }
        assert!(
            out.len() >= ZMEMO_HELD,
            "{} cells beside the slots'",
            out.len()
        );
        out.pop();
        assert!(m.take_free().is_cell(), "a cell handed back is handed out");
        drop(out);
        // The memo gone, the held group still reads from its cell.
        drop(m);
        assert_eq!(held.bytes(1)[0], 14);
        // No slab, no cell.
        let mut plain = Memo::new(4, 3 * 2000);
        assert!(!plain.take_free().is_cell());
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
            &fx.out,
            &fx.blob,
            ID,
            END,
            false,
            false,
            false,
            true,
            false,
            FenceSearch::Keys,
            true,
            &StrideAttach::Check,
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
                &fx.out,
                &fx.blob,
                ID,
                END,
                false,
                true,
                false,
                trusted,
                memo_first,
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
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
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
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
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
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

    /// Either kind of trailer's account of its segment, held to the live
    /// segment's by the one comparison, with each field wrong in turn named
    /// by its own refusal: the block form's through `Trailer::seg_ident`, and
    /// the stride form's as its bytes decode, so that a field of either
    /// mapped to another's place is a refusal of the wrong words.
    #[test]
    fn same_segment_names_each_field_of_either_kinds_ident() {
        use crate::db::zpages::{KeyCodec, KeyKind, StrideTrailer};
        let f = ZSource {
            nkeys: 200,
            recs_len: 24_800,
            crc_off: 30_000,
            key_off: 192,
            hash_cap: 512,
            file_len: 40_960,
            timestamp: 1_760_000_000,
            generation: 1,
            row_crc: 0x1357_9BDF,
            rec_version: 3,
            rec_magic: 0x5355_5044_4200_0007,
        };
        let (id, end_seq) = (17, 4_000);
        let block = Trailer {
            nkeys: f.nkeys,
            seg_recs_len: f.recs_len,
            seg_crc_off: f.crc_off,
            seg_key_off: f.key_off,
            seg_hash_cap: f.hash_cap,
            seg_file_len: f.file_len,
            seg_timestamp: f.timestamp,
            seg_generation: f.generation,
            seg_row_crc: f.row_crc,
            rec_version: f.rec_version,
            rec_magic: f.rec_magic,
            seg_id: id,
            seg_end_seq: end_seq,
            ..Trailer::default()
        };
        let stride_of = |seg: SegIdent| {
            let t = StrideTrailer {
                codec: KeyCodec::new(KeyKind::Decimal, b"k", 4, 0).unwrap(),
                w: 100,
                res_crc: 0,
                seg,
                sparse: None,
            };
            StrideTrailer::decode(&t.encode().unwrap())
                .unwrap()
                .seg_ident()
        };
        assert_eq!(stride_of(block.seg_ident()), block.seg_ident());
        assert_eq!(same_segment(&block.seg_ident(), &f, id, end_seq), Ok(()));
        assert_eq!(
            same_segment(&stride_of(block.seg_ident()), &f, id, end_seq),
            Ok(())
        );
        // The live segment named otherwise.
        assert_eq!(
            same_segment(&block.seg_ident(), &f, id + 1, end_seq),
            Err("the form file names another segment")
        );
        assert_eq!(
            same_segment(&block.seg_ident(), &f, id, end_seq + 1),
            Err("the form file names another segment")
        );
        type Edit = (fn(&mut Trailer), fn(&mut SegIdent), &'static str);
        let edits: Vec<(&str, Edit)> = vec![
            (
                "seg_id",
                (
                    |t| t.seg_id += 1,
                    |s| s.seg_id += 1,
                    "the form file names another segment",
                ),
            ),
            (
                "seg_end_seq",
                (
                    |t| t.seg_end_seq += 1,
                    |s| s.seg_end_seq += 1,
                    "the form file names another segment",
                ),
            ),
            (
                "nkeys",
                (|t| t.nkeys += 1, |s| s.nkeys += 1, "the key count differs"),
            ),
            (
                "row_crc",
                (
                    |t| t.seg_row_crc ^= 1,
                    |s| s.row_crc ^= 1,
                    "the key section's checksum row differs",
                ),
            ),
            (
                "recs_len",
                (
                    |t| t.seg_recs_len += 4,
                    |s| s.recs_len += 4,
                    "the record region's length differs",
                ),
            ),
            (
                "crc_off",
                (
                    |t| t.seg_crc_off += 4,
                    |s| s.crc_off += 4,
                    "the checksum row's place differs",
                ),
            ),
            (
                "key_off",
                (
                    |t| t.seg_key_off += 4096,
                    |s| s.key_off += 4096,
                    "the key section's offset differs",
                ),
            ),
            (
                "hash_cap",
                (
                    |t| t.seg_hash_cap *= 2,
                    |s| s.hash_cap *= 2,
                    "the hash capacity differs",
                ),
            ),
            (
                "file_len",
                (
                    |t| t.seg_file_len += 4096,
                    |s| s.file_len += 4096,
                    "the segment's length differs",
                ),
            ),
            (
                "timestamp",
                (
                    |t| t.seg_timestamp += 1,
                    |s| s.timestamp += 1,
                    "the segment's timestamp differs",
                ),
            ),
            (
                "generation",
                (
                    |t| t.seg_generation += 1,
                    |s| s.generation += 1,
                    "the segment's generation differs",
                ),
            ),
            (
                "rec_version",
                (
                    |t| t.rec_version += 1,
                    |s| s.rec_version += 1,
                    "the record format's version differs",
                ),
            ),
            (
                "rec_magic",
                (
                    |t| t.rec_magic ^= 1,
                    |s| s.rec_magic ^= 1,
                    "the file format's magic differs",
                ),
            ),
        ];
        for (field, (on_block, on_ident, why)) in &edits {
            let mut t = block.clone();
            on_block(&mut t);
            assert_eq!(
                same_segment(&t.seg_ident(), &f, id, end_seq),
                Err(*why),
                "the block form's trailer with {field} changed"
            );
            let mut s = block.seg_ident();
            on_ident(&mut s);
            assert_eq!(
                same_segment(&stride_of(s), &f, id, end_seq),
                Err(*why),
                "the stride form's trailer with {field} changed"
            );
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
        assert_eq!(zfile.ident(), Some(&t0));
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
        let sampled = sample_pages(pages);
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
        let sampled = sample_pages(pages);
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
    /// not asked for stays cold. Not under qemu-user (`supdb_emulated`),
    /// which answers `madvise(MADV_WILLNEED)` with success and passes
    /// nothing to the kernel: a file of 1,024 evicted pages advised so read
    /// 1,024 resident natively and none emulated. The native ARM leg runs it.
    #[cfg(all(target_os = "linux", not(supdb_emulated)))]
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
            // The walk under each of its shapes: the memo taking every
            // decode, the first touch the walk's own, and the fence stepped
            // with the memo asked once (`Options::zpages_bulk_steps`), with
            // the first touch the walk's own and without.
            let memos = [
                ("memo", ZMemo::new()),
                ("bulk", ZMemo::new().bulk(true, true, true)),
                (
                    "steps",
                    ZMemo::new().bulk(true, true, true).bulk_steps(true),
                ),
                ("steps-memo", ZMemo::new().bulk_steps(true)),
            ];
            for (shape, memo) in &memos {
                let (mut stepped, mut absent) = (0u64, 0u64);
                for &from in &starts {
                    for &limit in &limits {
                        let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                        let raw_n = fx
                            .blob
                            .scan_at(from, limit, |k, v| want.push((k.to_vec(), v.to_vec())))
                            .unwrap();
                        let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                        let z = zfile
                            .scan_at(memo, &fx.blob, from, limit, &mut |k: &[u8], v: &[u8]| {
                                got.push((k.to_vec(), v.to_vec()))
                            })
                            .unwrap();
                        stepped += z.stepped;
                        absent += z.absent;
                        assert_eq!(
                            z.end,
                            ScanEnd::Served,
                            "{name} {shape}: from {from} limit {limit}"
                        );
                        assert_eq!(
                            z.seen, raw_n,
                            "{name} {shape}: the count, from {from} limit {limit}"
                        );
                        assert_eq!(
                            got, want,
                            "{name} {shape}: the stream, from {from} limit {limit}"
                        );
                    }
                }
                // The steps and the single ask fire in the shapes they are
                // for and in no other.
                let steps = shape.starts_with("steps");
                assert_eq!(
                    stepped > 0,
                    steps,
                    "{name} {shape}: {stepped} entries stepped"
                );
                assert_eq!(
                    absent > 0,
                    *shape == "steps",
                    "{name} {shape}: {absent} opens asked once"
                );
                // A seek and the walk from the rank it found, as a scan
                // makes them: the walk starts in the entry the seek found,
                // searching nothing, under the steps; the stream is the
                // raw walk's under every shape.
                let (mut seeded, mut walks) = (0u64, 0u64);
                for i in (0..n).step_by(53) {
                    let key = fx.recs[i].0.clone();
                    let ZSeek::Served { rank, .. } = zfile.seek(memo, &key) else {
                        panic!("{name} {shape}: the seek of record {i}");
                    };
                    let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    fx.blob
                        .scan_at(rank, 9, |k, v| want.push((k.to_vec(), v.to_vec())))
                        .unwrap();
                    let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                    let z = zfile
                        .scan_at(memo, &fx.blob, rank, 9, &mut |k: &[u8], v: &[u8]| {
                            got.push((k.to_vec(), v.to_vec()))
                        })
                        .unwrap();
                    assert_eq!(
                        got, want,
                        "{name} {shape}: the walk after the seek of record {i}"
                    );
                    seeded += u64::from(z.from_seek);
                    walks += 1;
                }
                assert_eq!(
                    seeded,
                    if steps { walks } else { 0 },
                    "{name} {shape}: {seeded} walks of {walks} began where the seek did"
                );
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
    /// ask for nothing. Not under qemu-user (`supdb_emulated`), which passes
    /// neither the attach's `MADV_RANDOM` nor the ask's `MADV_WILLNEED` to
    /// the kernel, as `an_ask_ahead_brings_the_cold_pages_in` says.
    #[cfg(all(target_os = "linux", not(supdb_emulated)))]
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
        let sampled = sample_pages(pages);
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

    // ---- the stride form's builder ----------------------------------------

    /// `n` keys of `k` and six digits from zero, each with one value of `w`
    /// bytes drawn for it.
    fn stride_recs(n: usize, w: usize, seed: u64) -> Vec<Rec> {
        stride_recs_from(0, n, w, seed)
    }

    /// `stride_recs` from the number `start`.
    fn stride_recs_from(start: usize, n: usize, w: usize, seed: u64) -> Vec<Rec> {
        let mut rng = Rng(seed | 1);
        (start..start + n)
            .map(|i| (format!("k{i:06}").into_bytes(), vec![rng.bytes(w)], false))
            .collect()
    }

    /// A plan asking `ask` of the stride form in pieces of three pages and
    /// windows of four, so that a fixture a test can afford crosses every
    /// boundary a partition's build does.
    fn stride_plan(ask: StrideAsk) -> BuildPlan {
        BuildPlan {
            stride: ask,
            piece: 3 * ZPAGE,
            window: 4 * ZPAGE,
            ..BuildPlan::new(0, true)
        }
    }

    /// The precheck over a fixture's records, as a build makes it where
    /// its plan takes no run with gaps.
    fn precheck_of(fx: &Fixture) -> std::result::Result<StrideFacts, StrideNo> {
        let (recs, dir) = fx.blob.record_regions().unwrap();
        stride_precheck(&SegImages::new(recs, dir, fx.blob.keys()), false)
    }

    /// The precheck as a build makes it where its plan takes one
    /// (`BuildPlan::stride_sparse`).
    fn precheck_sparse_of(fx: &Fixture) -> std::result::Result<StrideFacts, StrideNo> {
        let (recs, dir) = fx.blob.record_regions().unwrap();
        stride_precheck(&SegImages::new(recs, dir, fx.blob.keys()), true)
    }

    /// `stride_plan` taking a run with gaps, in the sparse form.
    fn sparse_plan(ask: StrideAsk) -> BuildPlan {
        BuildPlan {
            stride_sparse: true,
            ..stride_plan(ask)
        }
    }

    /// `n` keys of `k` and six digits whose numbers ascend from `start` by
    /// steps of one to seven, so that runs of neighbours and gaps of every
    /// width under eight both occur, each with one value of `w` bytes.
    fn gapped_recs(start: usize, n: usize, w: usize, seed: u64) -> Vec<Rec> {
        let mut rng = Rng(seed | 1);
        let mut x = start;
        (0..n)
            .map(|i| {
                if i > 0 {
                    x += 1 + rng.below(7);
                }
                (format!("k{x:06}").into_bytes(), vec![rng.bytes(w)], false)
            })
            .collect()
    }

    /// `n` keys of sixteen hex digits, the letters of one case, of distinct
    /// random 64-bit ids in their order, each with one value of `w` bytes:
    /// what a store of random ids holds in a partition.
    fn id_recs(n: usize, w: usize, seed: u64, upper: bool) -> Vec<Rec> {
        let mut rng = Rng(seed | 1);
        let mut ids: Vec<u64> = (0..n).map(|_| rng.word()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|id| {
                let key = if upper {
                    format!("{id:016X}")
                } else {
                    format!("{id:016x}")
                };
                (key.into_bytes(), vec![rng.bytes(w)], false)
            })
            .collect()
    }

    /// The numbers a stride file holds its keys by, in rank order, as the
    /// fixture's keys decode under the file's codec.
    fn numbers_of(body: &StrideBody, fx: &Fixture) -> Vec<u64> {
        fx.recs
            .iter()
            .map(|(k, _, _)| body.codec().decode(k).expect("a key of the run"))
            .collect()
    }

    /// The stride file a fixture built, held to the fixture's records and to
    /// the raw form through the body a reader serves it by: the segment it
    /// names the live one; every rank's value, key and key's read; nothing
    /// past the run; keys beside the run's absent; and the zeros from the
    /// values' end to their last page's.
    fn stride_agrees(fx: &Fixture) -> StrideBody {
        let bytes = std::fs::read(&fx.out).unwrap();
        let body = StrideBody::from_bytes(&bytes).expect("the stride file reads");
        let facts = fx.blob.z_source_facts().unwrap();
        assert_eq!(
            same_segment(&body.trailer().seg_ident(), &facts, ID, END),
            Ok(())
        );
        assert_eq!(body.keys(), fx.recs.len() as u64);
        let mut good = |p: u32| body.page_ok(&bytes, p);
        let mut kbuf = [0u8; STRIDE_KEY_MAX];
        for (r, (key, vals, _)) in fx.recs.iter().enumerate() {
            let rank = r as u64;
            assert_eq!(vals.len(), 1, "rank {r}");
            assert_eq!(
                body.values_at(&bytes, rank, &mut good),
                StrideAt::Value(&vals[0][..]),
                "rank {r}"
            );
            assert_eq!(
                body.key_at_into(&bytes, rank, &mut kbuf, &mut good),
                Ok(Some(&key[..])),
                "rank {r}"
            );
            assert_eq!(
                body.rank_of(&bytes, key, &mut good),
                Ok(Some(rank)),
                "rank {r}"
            );
            assert_eq!(
                body.lower_bound(&bytes, key, &mut good),
                Ok((rank, KeyOrder::Equal)),
                "rank {r}"
            );
            let mut got = Vec::new();
            assert_eq!(
                body.read_all(&bytes, key, &mut good, &mut |v| got.push(v.to_vec())),
                StrideRead::Served(1),
                "rank {r}"
            );
            assert_eq!(&got, vals, "rank {r}");
            assert_eq!(&raw_values(&fx.blob, key), vals, "the raw form's, rank {r}");
        }
        let n = body.keys();
        assert_eq!(body.values_at(&bytes, n, &mut good), StrideAt::Absent);
        assert_eq!(body.key_at_into(&bytes, n, &mut kbuf, &mut good), Ok(None));
        let (first, last) = (&fx.recs[0].0, &fx.recs[fx.recs.len() - 1].0);
        let codec = body.trailer().codec;
        let mut beside = vec![
            [first.as_slice(), b"0"].concat(),
            first[..first.len() - 1].to_vec(),
            [last.as_slice(), b"\0"].concat(),
            Vec::new(),
        ];
        if let Some(below) = codec.base().checked_sub(1) {
            beside.push(codec.render(below, &mut kbuf).to_vec());
        }
        // The number after the last key's, where the field holds one; and,
        // for a run with gaps, every number in a gap next to a key's, each
        // a key the codec renders and the form does not hold.
        let numbers = numbers_of(&body, fx);
        let top = numbers[numbers.len() - 1];
        assert_eq!(
            body.sparse().map(|sp| sp.last),
            body.sparse().map(|_| top),
            "a sparse form's last number is its last key's"
        );
        if body.sparse().is_none() {
            assert_eq!(
                top,
                codec.base() + n - 1,
                "a whole run's numbers are its ranks"
            );
        }
        if top < codec.top() {
            beside.push(codec.render(top + 1, &mut kbuf).to_vec());
        }
        for pair in numbers.windows(2) {
            if pair[1] - pair[0] > 1 {
                beside.push(codec.render(pair[0] + 1, &mut kbuf).to_vec());
                beside.push(codec.render(pair[1] - 1, &mut kbuf).to_vec());
            }
        }
        let sorted: Vec<&[u8]> = fx.recs.iter().map(|r| r.0.as_slice()).collect();
        for k in &beside {
            assert_eq!(body.rank_of(&bytes, k, &mut good), Ok(None), "{k:?}");
            assert_eq!(
                body.lower_bound(&bytes, k, &mut good),
                Ok((
                    sorted.partition_point(|s| *s < k.as_slice()) as u64,
                    KeyOrder::Greater
                )),
                "{k:?}"
            );
            assert_eq!(
                body.read_all(&bytes, k, &mut good, &mut |_| panic!("a value for {k:?}")),
                StrideRead::Served(0),
                "{k:?}"
            );
            assert!(raw_values(&fx.blob, k).is_empty(), "{k:?}");
        }
        let end = fx.recs.len() * body.trailer().w as usize;
        assert!(
            bytes[end..body.pages() * ZPAGE].iter().all(|&b| b == 0),
            "the values' last page is zeros past them"
        );
        body
    }

    /// The codes a segment keeps the reasons by are distinct and none of
    /// them zero, which is no reason.
    #[test]
    fn every_stride_reason_has_a_code_of_its_own() {
        use StrideNo::*;
        let all = [
            NotShaped,
            Holes,
            NotOneValue,
            Width,
            Tomb,
            BlockBacked,
            Small,
            SelfCheck,
            Deferred,
        ];
        let codes: std::collections::HashSet<u8> = all.iter().map(|w| w.code()).collect();
        assert_eq!(codes.len(), all.len());
        assert!(!codes.contains(&0));
        // One to `STRIDE_REASONS`, the slots the decline counts are kept in
        // (`ZEvents::stride_declines`).
        assert_eq!(all.len(), STRIDE_REASONS);
        assert!(codes
            .iter()
            .all(|&c| (1..=STRIDE_REASONS as u8).contains(&c)));
        // A conversion asks of a partition with no reason and of one whose
        // ask was left for it, and of no other.
        for why in all {
            assert_eq!(StrideNo::askable(why.code()), why == Deferred, "{why:?}");
        }
        assert!(StrideNo::askable(0));
    }

    /// Each reason is counted in its own slot of the declines, in the order
    /// the store's counts document (`ZCounts::stride_declines`): written
    /// out here, and not read from the code that maps them, so that a slot
    /// moved is a slot the reader of the counts would misname.
    #[test]
    fn each_stride_decline_is_counted_in_the_slot_its_reason_names() {
        use StrideNo::*;
        let slots = [
            (NotShaped, 0),
            (Holes, 1),
            (NotOneValue, 2),
            (Width, 3),
            (Tomb, 4),
            (BlockBacked, 5),
            (Small, 6),
            (SelfCheck, 7),
            (Deferred, 8),
        ];
        for (why, slot) in slots {
            let ev = ZEvents::default();
            ev.stride_declined(why);
            let got: Vec<u64> = ev
                .stride_declines
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .collect();
            let mut want = vec![0u64; STRIDE_REASONS];
            want[slot] = 1;
            assert_eq!(got, want, "{why:?}");
        }
    }

    /// A block build names the body it wrote by the plan's two quantities
    /// together: raw tails are a body of their own under the record coding
    /// alone, and under the block coding the flag builds the block body --
    /// the plan the segment work's record calls raw-tailed all the same
    /// (`z_coded_record`). Each file attaches in the coding it was built in
    /// and serves the raw form's answers.
    #[test]
    fn a_block_build_names_the_body_its_plan_makes() {
        for (coding, raw_tails, body) in [
            (ZCODING_BLOCK, false, FileCoding::Block),
            (ZCODING_BLOCK, true, FileCoding::Block),
            (ZCODING_RECORD, false, FileCoding::Block),
            (ZCODING_RECORD, true, FileCoding::RawTails),
        ] {
            let name = format!("coding-{coding}-{raw_tails}");
            let fx = Fixture::with_shape(&name, suite(1200, 31), 256, coding, raw_tails);
            let made = fx.build();
            let Outcome::Built(b) = made else {
                panic!("{name}: {made:?}")
            };
            assert_eq!((b.coding, b.stride_no), (body, None), "{name}");
            let zfile = fx.attach().expect("the block file attaches");
            assert_eq!(zfile.coded(), coding == ZCODING_RECORD, "{name}");
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
        }
    }

    /// Partitions of numbered keys of one length with one fixed value each
    /// -- values of one byte, of a hundred that straddle pages, of a page
    /// less one byte and of a page, a run of one, and a big-endian field --
    /// are built in the stride form under either ask, the precheck's sizes
    /// the file's to the byte, no temporary name left, the reason a segment
    /// keeps cleared, and every rank read back through the reader's body.
    #[test]
    fn a_run_of_numbered_keys_and_one_fixed_value_each_is_built_in_the_stride_form() {
        let mut rng = Rng(0x5be);
        let be: Vec<Rec> = (0..500u64)
            .map(|i| {
                let key = [b"be".as_slice(), &(0x0102_0304_0506_0000 + i).to_be_bytes()].concat();
                (key, vec![rng.bytes(100)], false)
            })
            .collect();
        let cases = [
            ("w1", stride_recs(5000, 1, 1)),
            ("w100", stride_recs(3000, 100, 2)),
            ("w4095", stride_recs(40, 4095, 3)),
            ("w4096", stride_recs(40, 4096, 4)),
            ("one", stride_recs(1, 100, 5)),
            ("be", be),
        ];
        for (name, recs) in cases {
            let fx = Fixture::new(&format!("stride-{name}"), recs, 1 << 16);
            let facts = precheck_of(&fx).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            for ask in [StrideAsk::Only, StrideAsk::OrBlock] {
                let _ = std::fs::remove_file(&fx.out);
                let cell = AtomicU8::new(StrideNo::SelfCheck.code());
                let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(ask)).unwrap();
                made.note_stride(&cell);
                let Outcome::Built(b) = made else {
                    panic!("{name} {ask:?}: {made:?}")
                };
                assert_eq!(
                    (b.coding, b.stride_no),
                    (FileCoding::Stride, None),
                    "{name}"
                );
                assert_eq!(
                    b.file_bytes, facts.layout.file_len,
                    "{name}: the precheck's sizes are the file's"
                );
                assert_eq!(std::fs::metadata(&fx.out).unwrap().len(), b.file_bytes);
                assert!(!tmp_of(&fx.out).exists(), "{name}");
                assert_eq!(cell.load(Ordering::Relaxed), 0, "{name}: no reason");
                let body = stride_agrees(&fx);
                assert_eq!(
                    (body.trailer().codec, body.trailer().w),
                    (facts.codec, facts.w),
                    "{name}"
                );
            }
        }
    }

    /// A fixture's sparse stride file, built under `StrideAsk::OrBlock` by a
    /// plan that takes a run with gaps.
    fn sparse_built(fx: &Fixture) -> Built {
        let _ = std::fs::remove_file(&fx.out);
        let made = build(&fx.seg, &fx.out, ID, END, &sparse_plan(StrideAsk::OrBlock)).unwrap();
        match made {
            Outcome::Built(b) if b.coding == FileCoding::Stride => b,
            other => panic!("not a stride file: {other:?}"),
        }
    }

    /// Runs with gaps, of each kind of key and at each edge of the form's
    /// own layout: decimal numbers with holes, random ids of sixteen hex
    /// digits of either case, a big-endian field, values of one byte, of a
    /// hundred and of a page, two keys, and counts on either side of a
    /// sample's sixty-four numbers and of a page's five hundred and twelve.
    fn gapped_cases() -> Vec<(&'static str, Vec<Rec>, KeyKind)> {
        let mut rng = Rng(0x5a55);
        let mut x = 0x0102_0304_0506_0000u64;
        let be: Vec<Rec> = (0..900)
            .map(|_| {
                x += 1 + rng.word() % 300;
                let key = [b"be".as_slice(), &x.to_be_bytes()].concat();
                (key, vec![rng.bytes(100)], false)
            })
            .collect();
        let two = vec![
            (b"k000001".to_vec(), vec![rng.bytes(100)], false),
            (b"k000008".to_vec(), vec![rng.bytes(100)], false),
        ];
        let (dec, hexl, hexu) = (KeyKind::Decimal, KeyKind::HexLower, KeyKind::HexUpper);
        vec![
            ("gap-w100", gapped_recs(3, 3000, 100, 1), dec),
            ("gap-w1", gapped_recs(0, 5000, 1, 2), dec),
            ("gap-w4096", gapped_recs(7, 40, 4096, 3), dec),
            ("two", two, dec),
            ("ids", id_recs(3000, 100, 4, false), hexl),
            ("IDS", id_recs(700, 100, 5, true), hexu),
            ("ids-63", id_recs(63, 100, 6, false), hexl),
            ("ids-64", id_recs(64, 100, 7, false), hexl),
            ("ids-65", id_recs(65, 100, 8, false), hexl),
            ("ids-511", id_recs(511, 100, 9, false), hexl),
            ("ids-512", id_recs(512, 100, 10, false), hexl),
            ("ids-513", id_recs(513, 100, 11, false), hexl),
            ("be-gaps", be, KeyKind::BigEndian),
        ]
    }

    /// A partition whose keys are numbers with gaps between them is no run
    /// to a plan that takes none -- the reason named and nothing on disk
    /// asked for the stride form alone, the block file with the reason
    /// beside it asked for either -- and is built in the sparse stride form
    /// by a plan that takes one, under either ask: the precheck's sizes the
    /// file's to the byte, the codec the kind its keys are of, the numbers
    /// the file holds its keys' in rank order, the samples every
    /// sixty-fourth of them, the line's error the measure over them, and
    /// every rank read back through the reader's body and through the file a
    /// store attaches.
    #[test]
    fn a_run_with_gaps_is_built_in_the_sparse_form_only_where_the_plan_takes_one() {
        for (name, recs, kind) in gapped_cases() {
            let fx = Fixture::new(&format!("sparse-{name}"), recs, 1 << 16);
            // A plan that takes no run with gaps.
            let why = precheck_of(&fx).expect_err("a run with gaps is no whole run");
            assert!(
                matches!(why, StrideNo::Holes | StrideNo::NotShaped),
                "{name}: {why:?}"
            );
            let _ = std::fs::remove_file(&fx.out);
            let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::Only)).unwrap();
            assert!(
                matches!(made, Outcome::NotStride(w) if w == why),
                "{name}: {made:?}"
            );
            assert!(!fx.out.exists() && !tmp_of(&fx.out).exists(), "{name}");
            let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::OrBlock)).unwrap();
            match made {
                Outcome::Built(b) => assert_eq!(
                    (b.coding, b.stride_no),
                    (FileCoding::Block, Some(why)),
                    "{name}"
                ),
                // The block form's own decline of a partition that small.
                Outcome::Declined(_) => assert!(fx.recs.len() < 100, "{name}"),
                other => panic!("{name}: {other:?}"),
            }
            if fx.out.exists() {
                let zfile = fx.attach().expect("the block file attaches");
                assert!(!zfile.is_stride(), "{name}");
                agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            }
            // A plan that takes one.
            let facts = precheck_sparse_of(&fx).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(facts.codec.kind(), kind, "{name}");
            let last_key = &fx.recs[fx.recs.len() - 1].0;
            assert_eq!(facts.sparse_last, facts.codec.decode(last_key), "{name}");
            assert!(facts.sparse_last.is_some(), "{name}");
            for ask in [StrideAsk::Only, StrideAsk::OrBlock] {
                let _ = std::fs::remove_file(&fx.out);
                let cell = AtomicU8::new(StrideNo::SelfCheck.code());
                let made = build(&fx.seg, &fx.out, ID, END, &sparse_plan(ask)).unwrap();
                made.note_stride(&cell);
                let Outcome::Built(b) = made else {
                    panic!("{name} {ask:?}: {made:?}")
                };
                assert_eq!(
                    (b.coding, b.stride_no),
                    (FileCoding::Stride, None),
                    "{name}"
                );
                assert_eq!(
                    b.file_bytes, facts.layout.file_len,
                    "{name}: the precheck's sizes are the file's"
                );
                assert_eq!(std::fs::metadata(&fx.out).unwrap().len(), b.file_bytes);
                assert!(!tmp_of(&fx.out).exists(), "{name}");
                assert_eq!(cell.load(Ordering::Relaxed), 0, "{name}: no reason");
                let body = stride_agrees(&fx);
                assert_eq!(
                    (body.trailer().codec, body.trailer().w),
                    (facts.codec, facts.w),
                    "{name}"
                );
                // The file's own account of its keys, from its bytes: the
                // numbers after the values, the samples after the row.
                let bytes = std::fs::read(&fx.out).unwrap();
                let numbers = numbers_of(&body, &fx);
                let lay = facts.layout;
                let words = |at: u64, n: usize| -> Vec<u64> {
                    let (words, _) = bytes[at as usize..at as usize + 8 * n].as_chunks::<8>();
                    words.iter().map(|w| u64::from_le_bytes(*w)).collect()
                };
                assert_eq!(words(lay.k_off(), numbers.len()), numbers, "{name}");
                assert!(
                    bytes[lay.k_off() as usize + 8 * numbers.len()..lay.res_off as usize]
                        .iter()
                        .all(|&b| b == 0),
                    "{name}: zeros past the last number"
                );
                let samples: Vec<u64> = numbers.iter().step_by(64).copied().collect();
                assert_eq!(lay.samples, samples.len() as u64, "{name}");
                assert_eq!(words(lay.samples_off(), samples.len()), samples, "{name}");
                assert_eq!(
                    body.sparse(),
                    Some(StrideSparse {
                        last: numbers[numbers.len() - 1],
                        eps: stride_eps(&numbers),
                    }),
                    "{name}"
                );
                assert_eq!(body.number_bytes(), 8 * numbers.len() as u64, "{name}");
                assert_eq!(
                    body.checked_pages() as u64,
                    lay.v_pages + lay.k_pages,
                    "{name}"
                );
                // And through the file a store attaches.
                let zfile = fx.attach().unwrap_or_else(|e| panic!("{name}: {e:?}"));
                assert!(zfile.is_stride(), "{name}");
                agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
                assert!(!zfile.damaged() && zfile.bad_pages() == 0, "{name}");
            }
            // A whole run is built as it was by a plan that takes a run
            // with gaps too: no number held.
        }
        let fx = Fixture::new("sparse-whole", stride_recs(700, 100, 12), 1 << 16);
        assert_eq!(precheck_sparse_of(&fx), precheck_of(&fx));
        let whole = sparse_built(&fx);
        let sparse_bytes = std::fs::read(&fx.out).unwrap();
        assert!(stride_agrees(&fx).sparse().is_none());
        let plain = stride_built(&fx);
        assert_eq!(whole.file_bytes, plain.file_bytes);
        assert_eq!(std::fs::read(&fx.out).unwrap(), sparse_bytes);
    }

    /// A plan that leaves the sparse form for a conversion to ask
    /// (`BuildPlan::stride_sparse_later`) builds a run with gaps in the
    /// block form and names it deferred -- the one reason a conversion
    /// asks past (`StrideNo::askable`) -- under either ask, nothing on disk
    /// where the stride form alone was asked. It names nothing so of a
    /// partition the sparse form would not take either, whose reason is
    /// the one it has; it builds a whole run in the stride form as any
    /// plan does; and where the plan takes the sparse form now the later
    /// says nothing.
    #[test]
    fn a_plan_that_leaves_the_sparse_form_for_later_names_a_run_with_gaps_deferred() {
        let later = |ask| BuildPlan {
            stride_sparse_later: true,
            ..stride_plan(ask)
        };
        for (name, recs, _) in gapped_cases() {
            if recs.len() < 100 {
                continue;
            }
            let fx = Fixture::new(&format!("sparse-later-{name}"), recs, 1 << 16);
            let _ = std::fs::remove_file(&fx.out);
            let cell = AtomicU8::new(0);
            let made = build(&fx.seg, &fx.out, ID, END, &later(StrideAsk::Only)).unwrap();
            made.note_stride(&cell);
            assert!(
                matches!(made, Outcome::NotStride(StrideNo::Deferred)),
                "{name}: {made:?}"
            );
            assert!(!fx.out.exists() && !tmp_of(&fx.out).exists(), "{name}");
            assert_eq!(cell.load(Ordering::Relaxed), StrideNo::Deferred.code());
            let cell = AtomicU8::new(0);
            let made = build(&fx.seg, &fx.out, ID, END, &later(StrideAsk::OrBlock)).unwrap();
            made.note_stride(&cell);
            let Outcome::Built(b) = made else {
                panic!("{name}: {made:?}")
            };
            assert_eq!(
                (b.coding, b.stride_no, b.stride_sparse),
                (FileCoding::Block, Some(StrideNo::Deferred), false),
                "{name}"
            );
            assert!(StrideNo::askable(cell.load(Ordering::Relaxed)), "{name}");
            let zfile = fx.attach().expect("the block file attaches");
            assert!(!zfile.is_stride(), "{name}");
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
            // Taken now, the later says nothing.
            let _ = std::fs::remove_file(&fx.out);
            let both = BuildPlan {
                stride_sparse_later: true,
                ..sparse_plan(StrideAsk::OrBlock)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &both).unwrap();
            assert!(
                matches!(
                    made,
                    Outcome::Built(Built {
                        coding: FileCoding::Stride,
                        stride_sparse: true,
                        stride_no: None,
                        ..
                    })
                ),
                "{name}: {made:?}"
            );
        }
        // A partition the sparse form would not take: a record of two
        // values among a run with gaps keeps its own reason, and a key of
        // another length keeps the shape's.
        let mut two = gapped_recs(3, 600, 100, 0x71);
        let extra = two[300].1[0].clone();
        two[300].1.push(extra);
        let mut odd = gapped_recs(3, 600, 100, 0x72);
        odd[300].0.push(b'x');
        for (name, recs, why) in [
            ("two-values", two, StrideNo::NotOneValue),
            ("odd-key", odd, StrideNo::NotShaped),
        ] {
            let fx = Fixture::new(&format!("sparse-later-{name}"), recs, 1 << 16);
            let _ = std::fs::remove_file(&fx.out);
            let made = build(&fx.seg, &fx.out, ID, END, &later(StrideAsk::OrBlock)).unwrap();
            let Outcome::Built(b) = made else {
                panic!("{name}: {made:?}")
            };
            // The two ends are a run with gaps, so the ask is left: what
            // the walk would find is the conversion's to find.
            assert_eq!(
                (b.coding, b.stride_no),
                (FileCoding::Block, Some(StrideNo::Deferred)),
                "{name}"
            );
            // And the conversion's ask finds it.
            let _ = std::fs::remove_file(&fx.out);
            let made = build(&fx.seg, &fx.out, ID, END, &sparse_plan(StrideAsk::Only)).unwrap();
            assert!(
                matches!(made, Outcome::NotStride(w) if w == why),
                "{name}: {made:?}"
            );
        }
        // A run whose ends are no run of any kind keeps its reason: keys of
        // two lengths at the two ends.
        let mut ends = gapped_recs(3, 600, 100, 0x73);
        ends.last_mut().unwrap().0.push(b'9');
        let fx = Fixture::new("sparse-later-ends", ends, 1 << 16);
        let _ = std::fs::remove_file(&fx.out);
        let made = build(&fx.seg, &fx.out, ID, END, &later(StrideAsk::Only)).unwrap();
        assert!(
            matches!(made, Outcome::NotStride(StrideNo::NotShaped)),
            "{made:?}"
        );
        // A whole run is built in the stride form, later or not.
        let fx = Fixture::new("sparse-later-whole", stride_recs(700, 100, 0x74), 1 << 16);
        let _ = std::fs::remove_file(&fx.out);
        let made = build(&fx.seg, &fx.out, ID, END, &later(StrideAsk::OrBlock)).unwrap();
        assert!(
            matches!(
                made,
                Outcome::Built(Built {
                    coding: FileCoding::Stride,
                    stride_sparse: false,
                    stride_no: None,
                    ..
                })
            ),
            "{made:?}"
        );
    }

    /// Each state a partition can be in that the stride form does not hold,
    /// built: asked for the stride form alone, the reason named, nothing on
    /// disk, the reason kept where the segment keeps it, and the precheck
    /// deciding the states its two ends show; asked for it or the block
    /// form, the block file built in the same call and serving the raw
    /// form's answers with the reason beside it, or the block form's own
    /// decline for the two states that decline it too.
    #[test]
    fn the_stride_form_declines_each_state_for_its_reason_and_leaves_nothing() {
        use StrideNo::*;
        let n = 600;
        let mid = n / 2;
        let base = stride_recs(n, 100, 0x57);
        let with = |f: &dyn Fn(&mut Vec<Rec>)| {
            let mut r = base.clone();
            f(&mut r);
            r
        };
        let long: Vec<Rec> = base
            .iter()
            .enumerate()
            .map(|(i, (_, v, t))| {
                let key = format!("{}{i:07}", "p".repeat(STRIDE_KEY_MAX - 6)).into_bytes();
                (key, v.clone(), *t)
            })
            .collect();
        assert_eq!(long[0].0.len(), STRIDE_KEY_MAX + 1);
        // Both ends' values one wider than the widest the form takes, of one
        // width, so that the width and not two widths declines them; a
        // byte repeated, so that the block form's pages hold them
        // compressed.
        let wide = with(&|r| {
            for i in [0, n - 1] {
                r[i].1[0] = vec![0x5a; STRIDE_W_MAX as usize + 1];
            }
        });
        // (name, records, floor, inline bytes, reason, decided by the ends)
        type Case = (&'static str, Vec<Rec>, usize, usize, StrideNo, Option<bool>);
        let cases: Vec<Case> = vec![
            (
                "two-values",
                with(&|r| {
                    let v = r[mid].1[0].clone();
                    r[mid].1.push(v)
                }),
                0,
                1 << 16,
                NotOneValue,
                Some(false),
            ),
            (
                "two-values-first",
                with(&|r| {
                    let v = r[0].1[0].clone();
                    r[0].1.push(v)
                }),
                0,
                1 << 16,
                NotOneValue,
                Some(true),
            ),
            (
                "two-widths",
                with(&|r| r[mid].1[0].push(7)),
                0,
                1 << 16,
                Width,
                Some(false),
            ),
            (
                "two-widths-last",
                with(&|r| {
                    r[n - 1].1[0].pop();
                }),
                0,
                1 << 16,
                Width,
                Some(true),
            ),
            (
                "no-bytes",
                with(&|r| r[mid].1[0].clear()),
                0,
                1 << 16,
                Width,
                Some(false),
            ),
            (
                // A prefixed run's one value of no bytes is its length's
                // byte: a run of them all is no run of one-byte values.
                "no-bytes-all",
                with(&|r| r.iter_mut().for_each(|x| x.1[0].clear())),
                0,
                1 << 16,
                Width,
                Some(true),
            ),
            (
                "flagged",
                with(&|r| r[mid].2 = true),
                0,
                1 << 16,
                Tomb,
                Some(false),
            ),
            (
                "tomb-alone",
                with(&|r| {
                    r[mid].1.clear();
                    r[mid].2 = true
                }),
                0,
                1 << 16,
                Tomb,
                Some(false),
            ),
            (
                "two-lengths",
                with(&|r| r[mid].0.push(b'x')),
                0,
                1 << 16,
                NotShaped,
                Some(false),
            ),
            (
                // Below a digit, so that the key sorts in its place.
                "non-digit",
                with(&|r| *r[mid].0.last_mut().unwrap() = b'/'),
                0,
                1 << 16,
                NotShaped,
                Some(false),
            ),
            (
                "hole",
                with(&|r| {
                    r.remove(mid);
                }),
                0,
                1 << 16,
                Holes,
                Some(true),
            ),
            (
                "empty-key",
                with(&|r| r[0].0.clear()),
                0,
                1 << 16,
                NotShaped,
                Some(true),
            ),
            ("long-keys", long, 0, 1 << 16, NotShaped, Some(true)),
            ("wide", wide, 0, 1 << 16, Width, Some(true)),
            (
                "block-backed",
                with(&|r| r[mid].1[0] = vec![7u8; 9000]),
                0,
                256,
                BlockBacked,
                None,
            ),
            ("small", base.clone(), usize::MAX, 1 << 16, Small, None),
        ];
        for (name, recs, floor, inline, why, by_ends) in cases {
            let mut sorted = recs.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(sorted, recs, "{name}: the fixture's keys are in order");
            let fx = Fixture::new(&format!("stride-no-{name}"), recs, inline);
            match by_ends {
                Some(true) => assert_eq!(precheck_of(&fx), Err(why), "{name}"),
                Some(false) => assert!(precheck_of(&fx).is_ok(), "{name}: past the ends"),
                None => {}
            }
            let plan = |ask| BuildPlan {
                floor,
                ..stride_plan(ask)
            };
            // A decline the opening or the two ends decide comes before
            // the temporary name is the build's: a name it never opened --
            // another build's of the partition, under the same name -- is
            // left as it was. A decline the walk decides removes the name
            // the walk opened.
            let early = by_ends != Some(false);
            let other = b"another build's bytes";
            if early {
                std::fs::write(tmp_of(&fx.out), other).unwrap();
            }
            let cell = AtomicU8::new(0);
            let made = build(&fx.seg, &fx.out, ID, END, &plan(StrideAsk::Only)).unwrap();
            made.note_stride(&cell);
            assert!(
                matches!(made, Outcome::NotStride(w) if w == why),
                "{name}: {made:?}"
            );
            assert!(!fx.out.exists(), "{name}: no file");
            if early {
                assert_eq!(
                    std::fs::read(tmp_of(&fx.out)).unwrap(),
                    other,
                    "{name}: a name the build never opened left as it was"
                );
                std::fs::remove_file(tmp_of(&fx.out)).unwrap();
            }
            assert!(!tmp_of(&fx.out).exists(), "{name}: no temporary name");
            assert_eq!(cell.load(Ordering::Relaxed), why.code(), "{name}");
            let cell = AtomicU8::new(0);
            let made = build(&fx.seg, &fx.out, ID, END, &plan(StrideAsk::OrBlock)).unwrap();
            made.note_stride(&cell);
            assert!(!tmp_of(&fx.out).exists(), "{name}: no temporary name");
            match why {
                Small | BlockBacked => {
                    assert!(
                        matches!(
                            made,
                            Outcome::Declined(Decline::Small(_) | Decline::BlockBacked)
                        ),
                        "{name}: {made:?}"
                    );
                    assert!(!fx.out.exists(), "{name}: no file");
                    assert_eq!(cell.load(Ordering::Relaxed), 0, "{name}");
                }
                _ => {
                    let Outcome::Built(b) = made else {
                        panic!("{name}: {made:?}")
                    };
                    assert_eq!((b.coding, b.stride_no), (FileCoding::Block, Some(why)));
                    assert_eq!(cell.load(Ordering::Relaxed), why.code(), "{name}");
                    let zfile = fx.attach().expect("the block file attaches");
                    agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
                }
            }
        }
    }

    /// A build that gave no answer for the stride form leaves the reason a
    /// segment keeps as it was: a block file built where nothing asked for
    /// the stride form, a floor's decline under the block ask and under the
    /// stride form or the block form, and a cancel under each ask. The
    /// segment work notes every build's outcome (`Seg::z_stride_no`), so a
    /// reason one build found is the next block build's to keep, not to
    /// clear.
    #[test]
    fn a_build_that_gives_no_answer_for_the_stride_form_leaves_its_reason_as_it_was() {
        let fx = Fixture::new("stride-note", stride_recs(600, 100, 0x60), 1 << 16);
        assert!(
            precheck_of(&fx).is_ok(),
            "a partition the stride form would take, so that nothing declines it but the plan"
        );
        let kept = StrideNo::Holes.code();
        let cancelled = |ask| BuildPlan {
            cancel: Some(Arc::new(AtomicBool::new(true))),
            ..stride_plan(ask)
        };
        let floor = |ask| BuildPlan {
            floor: usize::MAX,
            ..stride_plan(ask)
        };
        type Want = fn(&Outcome) -> bool;
        let cases: [(&str, BuildPlan, Want); 6] = [
            ("block", stride_plan(StrideAsk::No), |o| {
                matches!(
                    o,
                    Outcome::Built(Built {
                        coding: FileCoding::Block,
                        stride_no: None,
                        ..
                    })
                )
            }),
            ("floor, block", floor(StrideAsk::No), |o| {
                matches!(o, Outcome::Declined(Decline::Small(_)))
            }),
            ("floor, stride or block", floor(StrideAsk::OrBlock), |o| {
                matches!(o, Outcome::Declined(Decline::Small(_)))
            }),
            ("cancel, block", cancelled(StrideAsk::No), |o| {
                matches!(o, Outcome::Cancelled)
            }),
            (
                "cancel, stride or block",
                cancelled(StrideAsk::OrBlock),
                |o| matches!(o, Outcome::Cancelled),
            ),
            ("cancel, stride alone", cancelled(StrideAsk::Only), |o| {
                matches!(o, Outcome::Cancelled)
            }),
        ];
        for (name, plan, want) in cases {
            let _ = std::fs::remove_file(&fx.out);
            let cell = AtomicU8::new(kept);
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            assert!(want(&made), "{name}: {made:?}");
            made.note_stride(&cell);
            assert_eq!(cell.load(Ordering::Relaxed), kept, "{name}");
        }
    }

    /// A plan that keeps the partitions at and above a key in the block
    /// form (`BuildPlan::stride_below`) asks the stride form of a partition
    /// whose first key is below the key, and nothing of one whose first key
    /// is the key or above it: the block body with no reason, as a plan
    /// that asks nothing builds it, the reason a segment keeps left as it
    /// was. Under either ask, a conversion's too.
    #[test]
    fn a_partition_at_or_above_the_plans_key_is_not_asked_for_the_stride_form() {
        let fx = Fixture::new("stride-below", stride_recs(600, 100, 0x61), 1 << 16);
        assert!(
            precheck_of(&fx).is_ok(),
            "a partition the stride form would take, so that nothing keeps it out but the key"
        );
        let first = fx.blob.key_at(0).unwrap().to_vec();
        // The run's second key, above the first; and the first key less its
        // last byte, which sorts below it.
        let second = fx.blob.key_at(1).unwrap().to_vec();
        let under = first[..first.len() - 1].to_vec();
        let kept = StrideNo::Holes.code();
        for ask in [StrideAsk::OrBlock, StrideAsk::Only] {
            for (name, cut, stride) in [
                ("the key above the first", &second, true),
                ("the key the first", &first, false),
                ("the key below the first", &under, false),
            ] {
                let what = format!("{ask:?}, {name}");
                let _ = std::fs::remove_file(&fx.out);
                let plan = BuildPlan {
                    stride_below: Some(cut.as_slice().into()),
                    ..stride_plan(ask)
                };
                let cell = AtomicU8::new(kept);
                let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
                made.note_stride(&cell);
                let Outcome::Built(b) = made else {
                    panic!("{what}: {made:?}")
                };
                assert_eq!(b.stride_no, None, "{what}: no reason either way");
                if stride {
                    assert_eq!(b.coding, FileCoding::Stride, "{what}");
                    assert_eq!(cell.load(Ordering::Relaxed), 0, "{what}: held");
                } else {
                    assert_eq!(b.coding, FileCoding::Block, "{what}");
                    assert_eq!(cell.load(Ordering::Relaxed), kept, "{what}: not asked");
                    let zfile = fx.attach().expect("the block file attaches");
                    agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
                }
            }
        }
    }

    /// The walk's key check takes both quantities of the run's rule as its
    /// inputs and names each one's reason: no number, a number not the
    /// rank's, and the rank's number read from a key that does not render
    /// it. The shipped decoder gives no such number -- it refuses each key
    /// below -- so the number is handed in as a lax decoder would read it:
    /// a field read past its width, a prefix compared short, a byte past
    /// the key's length ignored.
    #[test]
    fn a_key_is_its_ranks_only_where_its_number_is_and_the_number_renders_it() {
        use crate::db::zpages::KeyKind;
        let codec = KeyCodec::new(KeyKind::Decimal, b"k", 6, 100).unwrap();
        let mut kbuf = [0u8; STRIDE_KEY_MAX];
        let x = 300;
        let key = codec.render(x, &mut kbuf).to_vec();
        assert_eq!(key, b"k000300");
        let is = |key: &[u8], x: u64, decoded: Option<u64>| {
            stride_key_is(&codec, key, x, decoded, &mut [0u8; STRIDE_KEY_MAX])
        };
        assert_eq!(is(&key, x, codec.decode(&key)), Ok(()));
        assert_eq!(
            is(&key, x + 1, codec.decode(&key)),
            Err(StrideNo::Holes),
            "the next rank's number"
        );
        let unshaped = b"k00030x";
        assert_eq!(codec.decode(unshaped), None);
        assert_eq!(
            is(unshaped, x, codec.decode(unshaped)),
            Err(StrideNo::NotShaped)
        );
        for lax in [&b"k0000300"[..], b"j000300", b"k000300\0"] {
            assert_eq!(
                codec.decode(lax),
                None,
                "the shipped decoder refuses {lax:?}"
            );
            assert_eq!(
                is(lax, x, Some(x)),
                Err(StrideNo::NotShaped),
                "{lax:?} does not render {x}"
            );
        }
    }

    /// A segment whose key section was right at its open and whose record
    /// region a key was then changed in -- rank three hundred's made rank
    /// four hundred's, so that the two ends still count the run -- is
    /// declined at the rank it fails: a number the walk does not expect is
    /// a hole, whatever the precheck said of the ends.
    #[test]
    fn a_key_out_of_its_place_is_a_hole_at_its_rank() {
        let recs = stride_recs(600, 100, 0x58);
        let fx = Fixture::new("stride-out-of-place", recs, 1 << 16);
        let map = MmapBytes::open(&fx.seg).unwrap();
        let base = map.slice_at(0, 1).unwrap().as_ptr() as usize;
        let blob = Blob::open(map).unwrap();
        let (recs, dir) = blob.record_regions().unwrap();
        let at = rd_u32(dir, 300 * 4).unwrap() as usize;
        let key = record_key(&recs[at..], 0).unwrap();
        assert_eq!(key, b"k000300");
        let off = key.as_ptr() as usize - base;
        drop(blob);
        let f = OpenOptions::new().write(true).open(&fx.seg).unwrap();
        f.write_at(b"k000400", off as u64).unwrap();
        drop(f);
        assert!(precheck_of(&fx).is_ok(), "the ends still count the run");
        let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::Only)).unwrap();
        assert!(
            matches!(made, Outcome::NotStride(StrideNo::Holes)),
            "{made:?}"
        );
        assert!(!fx.out.exists() && !tmp_of(&fx.out).exists());
    }

    /// The full record (`SegmentOptions::compact_records` off) carries its
    /// extent and is read by `parse_record`, not in place: a run of such
    /// records qualifies through that parse, and that parse names a flag,
    /// two values and a value of no bytes as the compact path names them.
    #[test]
    fn the_full_record_qualifies_through_its_parse() {
        use StrideNo::*;
        let opts = SegmentOptions {
            compact_records: false,
            ..SegmentOptions::default()
        };
        let full = |name: &str, recs: Vec<Rec>| {
            Fixture::written(name, recs, ZCODING_BLOCK, false, |seg, recs| {
                write_segment_with(seg, recs, 1 << 16, &opts)
            })
        };
        let n = 400;
        let fx = full("stride-full", stride_recs(n, 100, 0x59));
        let (recs, dir) = fx.blob.record_regions().unwrap();
        let images = SegImages::new(recs, dir, n);
        for r in 0..n {
            assert!(
                compact_fixed_tomb_at(images.image(r), 0).is_none(),
                "rank {r} is a full record"
            );
        }
        let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::Only)).unwrap();
        assert!(
            matches!(
                made,
                Outcome::Built(Built {
                    coding: FileCoding::Stride,
                    ..
                })
            ),
            "{made:?}"
        );
        stride_agrees(&fx);
        let mid = n / 2;
        type Edit = fn(&mut Rec);
        let states: [(&str, Edit, StrideNo); 4] = [
            ("flagged", |r| r.2 = true, Tomb),
            (
                "tomb-alone",
                |r| {
                    r.1.clear();
                    r.2 = true
                },
                Tomb,
            ),
            (
                "two-values",
                |r| {
                    let v = r.1[0].clone();
                    r.1.push(v)
                },
                NotOneValue,
            ),
            ("no-bytes", |r| r.1[0].clear(), Width),
        ];
        for (name, edit, why) in states {
            let mut recs = stride_recs(n, 100, 0x59);
            edit(&mut recs[mid]);
            let fx = full(&format!("stride-full-{name}"), recs);
            let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::Only)).unwrap();
            assert!(
                matches!(made, Outcome::NotStride(w) if w == why),
                "{name}: {made:?}"
            );
        }
    }

    /// A segment closed from a direct run keeps a commit marker after each
    /// batch inside its record region, so its records are not end to end:
    /// the walk reads each through the directory and the run qualifies,
    /// where a walk by stride would read the markers as records.
    #[test]
    fn a_direct_runs_segment_with_its_markers_qualifies() {
        let n = 500;
        let fx = Fixture::written(
            "stride-marked",
            stride_recs(n, 100, 0x5a),
            ZCODING_BLOCK,
            false,
            |seg, recs| {
                let mut w = SegmentWriter::create(seg, &SegmentOptions::default()).unwrap();
                w.set_inline_max(1 << 16);
                w.set_marks(true);
                for (i, (k, vals, tomb)) in recs.iter().enumerate() {
                    w.begin(k).unwrap();
                    for v in vals {
                        w.value(v);
                    }
                    w.end_with(*tomb).unwrap();
                    if i % 7 == 6 {
                        w.mark().unwrap();
                    }
                }
                w.mark().unwrap();
                w.finish(1).unwrap();
            },
        );
        let (recs, dir) = fx.blob.record_regions().unwrap();
        let images = SegImages::new(recs, dir, n);
        let at = |r: usize| rd_u32(dir, r * 4).unwrap() as usize;
        let gaps = (0..n - 1)
            .filter(|&r| at(r) + images.image(r).len() != at(r + 1))
            .count();
        assert_eq!(gaps, (n - 1) / 7, "a marker after every seventh record");
        let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::Only)).unwrap();
        assert!(
            matches!(
                made,
                Outcome::Built(Built {
                    coding: FileCoding::Stride,
                    ..
                })
            ),
            "{made:?}"
        );
        stride_agrees(&fx);
    }

    /// The stride file is the same bytes whatever piece it is written in and
    /// whatever window it asks and drops by, and the drop behind it -- the
    /// read-back's, the build's last read of the records -- covers the
    /// records once, in order, a window at a time, and the directory last.
    #[test]
    fn a_stride_file_is_the_same_bytes_whatever_the_piece_and_the_window() {
        let fx = Fixture::new("stride-plans", stride_recs(4000, 100, 0x5b), 1 << 16);
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
                ahead: window,
                stride: StrideAsk::Only,
                ..BuildPlan::new(0, drop_behind)
            };
            let mut seen: Vec<(usize, usize)> = Vec::new();
            let r = build_seen(&fx.seg, &fx.out, ID, END, &plan, &mut |a, b| {
                seen.push((a, b))
            })
            .unwrap();
            assert!(
                matches!(r, Outcome::Built(Built { coding: FileCoding::Stride, asked, .. }) if asked > 0),
                "{r:?}"
            );
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
            let (dirs, recs_seen) = seen.split_last().expect("something was dropped");
            assert_eq!(*dirs, (dir_at, dir_end), "the directory is dropped last");
            let mut at = recs_at;
            for (i, &(a, b)) in recs_seen.iter().enumerate() {
                assert_eq!(a, at, "range {i} begins where the one before ended");
                assert!(b > a, "range {i} is empty");
                if i + 1 < recs_seen.len() {
                    assert!(b - a >= window, "range {i} is {} bytes", b - a);
                }
                at = b;
            }
            assert_eq!(at, recs_end, "the records are dropped to their end");
            assert_eq!(
                recs_seen.len() > 1,
                window <= 64 << 10,
                "a window under the region drops in turns, one over it once"
            );
        }
        stride_agrees(&fx);
    }

    thread_local! {
        /// The cancel a test's damage seam sets, the build running on the
        /// test's own thread: the seam is a function and holds nothing.
        static CANCEL_AT_SEAM: std::cell::RefCell<Option<Arc<AtomicBool>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// A stride build whose cancel is set before it begins stops at the
    /// write walk's first look, writing nothing past it -- the seam after
    /// the write is never reached -- and one whose cancel is set between
    /// the write and the read-back stops at the read-back's first look,
    /// before the read-back gives back a record: a cancel is a merge taking
    /// the partition as an input, which reads those records next, and the
    /// rename's own last look would answer the same outcome after a
    /// read-back that ran whole. Either leaves nothing on disk.
    #[test]
    fn a_cancelled_stride_build_stops_at_its_next_look_and_leaves_nothing() {
        let fx = Fixture::new("stride-cancel", stride_recs(300, 100, 0x5c), 1 << 16);
        for ask in [StrideAsk::Only, StrideAsk::OrBlock] {
            let plan = BuildPlan {
                cancel: Some(Arc::new(AtomicBool::new(true))),
                damage: Some(|_| panic!("the write walk ran past its cancel")),
                ..stride_plan(ask)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            assert!(matches!(made, Outcome::Cancelled), "{ask:?}: {made:?}");
            assert!(!fx.out.exists() && !tmp_of(&fx.out).exists(), "{ask:?}");
            let cancel = Arc::new(AtomicBool::new(false));
            CANCEL_AT_SEAM.with(|c| *c.borrow_mut() = Some(cancel.clone()));
            let plan = BuildPlan {
                cancel: Some(cancel),
                damage: Some(|_| {
                    CANCEL_AT_SEAM.with(|c| {
                        c.borrow()
                            .as_ref()
                            .expect("the test's cancel")
                            .store(true, Ordering::Release)
                    })
                }),
                ..stride_plan(ask)
            };
            let mut seen: Vec<(usize, usize)> = Vec::new();
            let made = build_seen(&fx.seg, &fx.out, ID, END, &plan, &mut |a, b| {
                seen.push((a, b))
            })
            .unwrap();
            assert!(matches!(made, Outcome::Cancelled), "{ask:?}: {made:?}");
            assert!(seen.is_empty(), "{ask:?}: the read-back gave back {seen:?}");
            assert!(!fx.out.exists() && !tmp_of(&fx.out).exists(), "{ask:?}");
        }
    }

    /// A walk that declines with the stride form asked for alone gives back
    /// what it read -- the records to the end of the one it declined at,
    /// and the directory -- since the block file it would have replaced
    /// serves on and nothing reads them next; asked for the stride form or
    /// the block form, it leaves them to the block walk, which drops them
    /// as its own walk does, from the region's start.
    #[test]
    fn a_stride_walk_that_declines_alone_gives_back_what_it_read() {
        let n = 2000;
        let mid = n / 2;
        let mut recs = stride_recs(n, 100, 0x5f);
        let v = recs[mid].1[0].clone();
        recs[mid].1.push(v);
        let fx = Fixture::new("stride-no-drop", recs, 1 << 16);
        let map = MmapBytes::open(&fx.seg).unwrap();
        let base = map.slice_at(0, 1).unwrap().as_ptr() as usize;
        let blob = Blob::open(map).unwrap();
        let (recs, dir) = blob.record_regions().unwrap();
        let recs_at = recs.as_ptr() as usize - base;
        let past = recs_at + rd_u32(dir, (mid + 1) * 4).unwrap() as usize;
        let dir_at = dir.as_ptr() as usize - base;
        let dir_end = dir_at + dir.len();
        drop(blob);
        let mut seen: Vec<(usize, usize)> = Vec::new();
        let made = build_seen(
            &fx.seg,
            &fx.out,
            ID,
            END,
            &stride_plan(StrideAsk::Only),
            &mut |a, b| seen.push((a, b)),
        )
        .unwrap();
        assert!(
            matches!(made, Outcome::NotStride(StrideNo::NotOneValue)),
            "{made:?}"
        );
        assert_eq!(seen, vec![(recs_at, past), (dir_at, dir_end)]);
        let mut seen: Vec<(usize, usize)> = Vec::new();
        let made = build_seen(
            &fx.seg,
            &fx.out,
            ID,
            END,
            &stride_plan(StrideAsk::OrBlock),
            &mut |a, b| seen.push((a, b)),
        )
        .unwrap();
        assert!(matches!(made, Outcome::Built(_)), "{made:?}");
        assert_eq!(seen.first().map(|r| r.0), Some(recs_at), "{seen:?}");
        assert_eq!(seen.last(), Some(&(dir_at, dir_end)), "{seen:?}");
        assert!(
            seen.windows(2).all(|w| w[0].1 <= w[1].0),
            "the block walk drops each range once: {seen:?}"
        );
    }

    /// The stride file at `path` with `edit` applied to its bytes and its
    /// trailer, and then made whole again under every checksum `edit` did
    /// not break on purpose: page `reword`'s word of the row taken again
    /// where one is named, the row's CRC taken again, and the trailer sealed
    /// again -- so that what `edit` changed is reached only by a check
    /// behind the checksums.
    fn forge_stride(
        path: &Path,
        reword: Option<u32>,
        edit: impl FnOnce(&mut [u8], &mut StrideTrailer, &StrideLayout),
    ) {
        let mut b = std::fs::read(path).unwrap();
        let len = b.len();
        let mut t = StrideTrailer::decode(&b[len - ZTRAILER..]).unwrap();
        let lay = t.layout().unwrap();
        edit(&mut b, &mut t, &lay);
        let bind = t.bind32();
        let res = lay.res_off as usize;
        if let Some(p) = reword {
            let at = p as usize * ZPAGE;
            let word = stride_page_crc(&b[at..at + ZPAGE], bind, p);
            let w_at = res + 4 * p as usize;
            b[w_at..w_at + 4].copy_from_slice(&word.to_le_bytes());
        }
        t.res_crc = stride_row_crc(&b[res..res + lay.res_len as usize], bind);
        b[len - ZTRAILER..].copy_from_slice(&t.encode().unwrap());
        std::fs::write(path, &b).unwrap();
    }

    /// The read-back before the name refuses a file the reader's body would
    /// serve wrong, each damage made between the write and the read-back
    /// and forged under good checksums where a checksum would catch it
    /// first: a value byte, a word of the row, the trailer's first number
    /// and the trailer's segment, each forged one refused by one of the
    /// read-back's checks alone -- the value's, the value's page against the
    /// row, the key's, the segment's -- so that any one of them deleted
    /// leaves its damage served. Asked for the stride form alone, nothing
    /// is named and nothing left; asked for it or the block form, the block
    /// file is built with the reason beside it. The seam that damages
    /// nothing builds.
    #[test]
    fn the_read_back_refuses_what_the_reader_would_serve_wrong() {
        let fx = Fixture::new("stride-check", stride_recs(1500, 100, 0x5d), 1 << 16);
        type Damage = (&'static str, fn(&Path));
        let damages: [Damage; 7] = [
            ("value byte", |p| {
                let f = OpenOptions::new().write(true).read(true).open(p).unwrap();
                let mut b = [0u8; 1];
                f.read_exact_at(&mut b, 7).unwrap();
                f.write_at(&[b[0] ^ 0x10], 7).unwrap();
            }),
            ("value byte, forged", |p| {
                forge_stride(p, Some(0), |b, _, _| b[7] ^= 0x10)
            }),
            ("row word", |p| {
                let mut b = std::fs::read(p).unwrap();
                let t = StrideTrailer::decode(&b[b.len() - ZTRAILER..]).unwrap();
                b[t.layout().unwrap().res_off as usize + 12] ^= 1;
                std::fs::write(p, &b).unwrap();
            }),
            ("row word, forged", |p| {
                forge_stride(p, None, |b, _, l| b[l.res_off as usize + 12] ^= 1)
            }),
            ("first number, forged", |p| {
                forge_stride(p, None, |_, t, _| {
                    let c = t.codec;
                    t.codec = KeyCodec::new(c.kind(), c.prefix(), c.d(), c.base() + 1).unwrap()
                })
            }),
            ("segment, forged", |p| {
                forge_stride(p, None, |_, t, _| t.seg.timestamp += 1)
            }),
            ("a page's zeros, forged", |p| {
                forge_stride(p, Some(36), |b, t, l| {
                    // Rank 1500's would-be slot in the last value page,
                    // which no read touches.
                    let end = 1500 * t.w as usize;
                    assert!(end < l.v_pages as usize * ZPAGE);
                    assert_eq!(end / ZPAGE, 36);
                    b[end] = 1;
                })
            }),
        ];
        for (name, damage) in damages {
            let _ = std::fs::remove_file(&fx.out);
            let cell = AtomicU8::new(0);
            let plan = BuildPlan {
                damage: Some(damage),
                ..stride_plan(StrideAsk::Only)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            made.note_stride(&cell);
            if name == "a page's zeros, forged" {
                // Bytes no read reaches read back as the records: the
                // read-back is of what a reader serves, and the zeros are
                // the writer's, held by `stride_agrees`.
                assert!(matches!(made, Outcome::Built(_)), "{name}: {made:?}");
                continue;
            }
            assert!(
                matches!(made, Outcome::NotStride(StrideNo::SelfCheck)),
                "{name}: {made:?}"
            );
            assert!(!fx.out.exists(), "{name}: nothing named");
            assert!(!tmp_of(&fx.out).exists(), "{name}: nothing left");
            assert_eq!(cell.load(Ordering::Relaxed), StrideNo::SelfCheck.code());
            let plan = BuildPlan {
                damage: Some(damage),
                ..stride_plan(StrideAsk::OrBlock)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            let Outcome::Built(b) = made else {
                panic!("{name}: {made:?}")
            };
            assert_eq!(
                (b.coding, b.stride_no),
                (FileCoding::Block, Some(StrideNo::SelfCheck)),
                "{name}"
            );
            assert!(!tmp_of(&fx.out).exists(), "{name}: nothing left");
            let zfile = fx.attach().expect("the block file attaches");
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
        }
        let _ = std::fs::remove_file(&fx.out);
        let plan = BuildPlan {
            damage: Some(|_| {}),
            ..stride_plan(StrideAsk::Only)
        };
        let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
        assert!(
            matches!(
                made,
                Outcome::Built(Built {
                    coding: FileCoding::Stride,
                    ..
                })
            ),
            "{made:?}"
        );
        stride_agrees(&fx);
    }

    /// `n` keys of sixteen lowercase hex digits whose numbers lie in a
    /// corner of the field but for the last hundred: the first of them a
    /// few apart from zero up and the rest spread over the top half, so
    /// that the straight line through the first number and the last is
    /// wrong for nearly every one by most of the samples -- the spread a
    /// search's guess is worst for.
    fn cornered_recs(n: usize, seed: u64) -> Vec<Rec> {
        let mut rng = Rng(seed | 1);
        let low = n - 100;
        (0..n)
            .map(|i| {
                let x = if i < low {
                    7 + 3 * i as u64
                } else {
                    (1 << 63) + ((i - low) as u64) * ((1 << 56) + 1)
                };
                (
                    format!("{x:016x}").into_bytes(),
                    vec![rng.bytes(100)],
                    false,
                )
            })
            .collect()
    }

    /// The read-back before the name holds a sparse file's account of its
    /// keys -- the numbers after the values, the samples after the row, the
    /// two ends in the trailer -- to the keys it was written from, each
    /// damage forged under good checksums so that only a check behind them
    /// reaches it: a number changed and still in order, which the key at
    /// its rank says; a number out of order, a sample that is not its
    /// number, a byte past the last number, a last number that is not the
    /// trailer's, each of which the page's first touch says
    /// (`StrideBody::page_ok`); and samples out of order or not from the
    /// first number, which the body refuses whole. The line's error is not
    /// among them: understated to nothing or overstated to every sample it
    /// costs a search more and answers every key the same, over a spread
    /// the line is as wrong for as it gets, which is what lets a reader
    /// take the trailer's word for it.
    #[test]
    fn the_read_back_holds_a_sparse_files_numbers_and_samples_to_its_keys() {
        let fx = Fixture::new("sparse-check", cornered_recs(1500, 0x5d), 1 << 16);
        let facts = precheck_sparse_of(&fx).unwrap();
        let lay = facts.layout;
        // Three pages of numbers after thirty-seven of values, and a
        // sample at every sixty-fourth rank.
        assert_eq!((lay.v_pages, lay.k_pages, lay.samples), (37, 3, 24));
        type Damage = (&'static str, fn(&Path), bool);
        // Rank 517 is on the second page of numbers, page 38 of the file.
        fn number_at(b: &[u8], l: &StrideLayout, rank: usize) -> u64 {
            let at = l.k_off() as usize + 8 * rank;
            u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
        }
        fn set_number(b: &mut [u8], l: &StrideLayout, rank: usize, x: u64) {
            let at = l.k_off() as usize + 8 * rank;
            b[at..at + 8].copy_from_slice(&x.to_le_bytes());
        }
        fn sample_at(b: &[u8], l: &StrideLayout, i: usize) -> u64 {
            let at = l.samples_off() as usize + 8 * i;
            u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
        }
        fn set_sample(b: &mut [u8], l: &StrideLayout, i: usize, x: u64) {
            let at = l.samples_off() as usize + 8 * i;
            b[at..at + 8].copy_from_slice(&x.to_le_bytes());
        }
        let damages: [Damage; 11] = [
            (
                "a number, in order",
                |p| {
                    forge_stride(p, Some(38), |b, _, l| {
                        let x = number_at(b, l, 517);
                        assert!(x + 1 < number_at(b, l, 518));
                        set_number(b, l, 517, x + 1)
                    })
                },
                true,
            ),
            (
                "a number, out of order",
                |p| {
                    forge_stride(p, Some(38), |b, _, l| {
                        let next = number_at(b, l, 518);
                        set_number(b, l, 517, next + 1)
                    })
                },
                true,
            ),
            (
                "a number that is a sample",
                |p| {
                    forge_stride(p, Some(38), |b, _, l| {
                        // Rank 576, the ninth sample: moved inside its gap.
                        let x = number_at(b, l, 576);
                        set_number(b, l, 576, x + 1)
                    })
                },
                true,
            ),
            (
                "a sample",
                |p| {
                    forge_stride(p, None, |b, _, l| {
                        let x = sample_at(b, l, 3);
                        assert!(x + 1 < sample_at(b, l, 4));
                        set_sample(b, l, 3, x + 1)
                    })
                },
                true,
            ),
            (
                "the samples out of order",
                |p| {
                    forge_stride(p, None, |b, _, l| {
                        let (x, y) = (sample_at(b, l, 3), sample_at(b, l, 4));
                        set_sample(b, l, 3, y);
                        set_sample(b, l, 4, x);
                    })
                },
                true,
            ),
            (
                "the first sample",
                |p| {
                    forge_stride(p, None, |b, _, l| {
                        let x = sample_at(b, l, 0);
                        set_sample(b, l, 0, x - 1)
                    })
                },
                true,
            ),
            (
                "a byte past the last number",
                |p| {
                    forge_stride(p, Some(39), |b, _, l| {
                        let at = l.k_off() as usize + 8 * 1500;
                        assert!(at < l.res_off as usize);
                        b[at] = 1;
                    })
                },
                true,
            ),
            (
                "the last number in the trailer",
                |p| {
                    forge_stride(p, None, |_, t, _| {
                        let sp = t.sparse.unwrap();
                        t.sparse = Some(StrideSparse {
                            last: sp.last + 1,
                            ..sp
                        })
                    })
                },
                true,
            ),
            (
                "a number page's word of the row",
                |p| {
                    // The row's last word, the last page of numbers', with
                    // the resident part's CRC made good over it: the page
                    // no longer the one its word says.
                    forge_stride(p, None, |b, _, l| {
                        b[(l.res_off + l.row_len()) as usize - 1] ^= 1
                    })
                },
                true,
            ),
            (
                "the line's error, none",
                |p| {
                    forge_stride(p, None, |_, t, _| {
                        let sp = t.sparse.unwrap();
                        assert!(sp.eps > 4, "a spread the line is wrong for: {}", sp.eps);
                        t.sparse = Some(StrideSparse { eps: 0, ..sp })
                    })
                },
                false,
            ),
            (
                "the line's error, every sample",
                |p| {
                    forge_stride(p, None, |_, t, _| {
                        let sp = t.sparse.unwrap();
                        t.sparse = Some(StrideSparse { eps: 24, ..sp })
                    })
                },
                false,
            ),
        ];
        for (name, damage, refused) in damages {
            let _ = std::fs::remove_file(&fx.out);
            let cell = AtomicU8::new(0);
            let plan = BuildPlan {
                damage: Some(damage),
                ..sparse_plan(StrideAsk::Only)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            made.note_stride(&cell);
            if !refused {
                assert!(matches!(made, Outcome::Built(_)), "{name}: {made:?}");
                // Every key found, every key beside the run's absent, and
                // every seek the keys' own, by the body and by the file a
                // store attaches.
                let body = stride_agrees(&fx);
                assert!(body.sparse().is_some(), "{name}");
                let zfile = fx.attach().unwrap_or_else(|e| panic!("{name}: {e:?}"));
                agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
                assert!(!zfile.damaged(), "{name}");
                continue;
            }
            assert!(
                matches!(made, Outcome::NotStride(StrideNo::SelfCheck)),
                "{name}: {made:?}"
            );
            assert!(!fx.out.exists(), "{name}: nothing named");
            assert!(!tmp_of(&fx.out).exists(), "{name}: nothing left");
            assert_eq!(cell.load(Ordering::Relaxed), StrideNo::SelfCheck.code());
            let plan = BuildPlan {
                damage: Some(damage),
                ..sparse_plan(StrideAsk::OrBlock)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            let Outcome::Built(b) = made else {
                panic!("{name}: {made:?}")
            };
            assert_eq!(
                (b.coding, b.stride_no),
                (FileCoding::Block, Some(StrideNo::SelfCheck)),
                "{name}"
            );
            let zfile = fx.attach().expect("the block file attaches");
            agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
        }
    }

    /// Each thing a sparse file's samples and its pages of numbers are held
    /// to, alone: a file forged under good checksums so that one clause and
    /// no other is what it fails. The samples, where the body is taken from
    /// the bytes: not from the first key's number, not ascending -- two out
    /// of order, two equal -- and past the last key's number, and a byte
    /// between the row and the samples where the row's words end off a
    /// multiple of eight. A page of numbers, at its check, with every other
    /// page passing: two numbers out of order, two equal, a number that is
    /// a sample moved and still in order, a byte past the last number, and
    /// a last number that is not the trailer's. The read-back refuses most
    /// of these by another route as well, the key at a rank
    /// (`the_read_back_holds_a_sparse_files_numbers_and_samples_to_its_keys`);
    /// a reader of a file found on disk has only these.
    #[test]
    fn a_sparse_files_samples_and_pages_of_numbers_are_held_clause_by_clause() {
        let fx = Fixture::new("sparse-clauses", id_recs(1500, 100, 0x61, false), 1 << 16);
        assert_eq!(fx.recs.len(), 1500, "distinct ids");
        sparse_built(&fx);
        let good = std::fs::read(&fx.out).unwrap();
        let body = StrideBody::from_bytes(&good).unwrap();
        let (v, pages) = (body.pages() as u32, body.checked_pages() as u32);
        assert_eq!(pages, v + 3, "three pages of numbers");
        assert!((0..pages).all(|p| body.page_ok(&good, p)));
        let last = body.sparse().unwrap().last;
        type Edit = fn(&mut [u8], &mut StrideTrailer, &StrideLayout);
        let forged = |reword: Option<u32>, edit: Edit| -> Vec<u8> {
            std::fs::write(&fx.out, &good).unwrap();
            forge_stride(&fx.out, reword, edit);
            std::fs::read(&fx.out).unwrap()
        };
        fn word(b: &[u8], at: u64) -> u64 {
            u64::from_le_bytes(b[at as usize..at as usize + 8].try_into().unwrap())
        }
        fn put(b: &mut [u8], at: u64, x: u64) {
            b[at as usize..at as usize + 8].copy_from_slice(&x.to_le_bytes());
        }
        // The samples.
        let samples: [(&str, Edit); 4] = [
            ("not from the first number", |b, _, l| {
                let at = l.samples_off();
                let x = word(b, at);
                put(b, at, x - 1)
            }),
            ("two out of order", |b, _, l| {
                let at = l.samples_off() + 8 * 3;
                let (x, y) = (word(b, at), word(b, at + 8));
                put(b, at, y);
                put(b, at + 8, x);
            }),
            ("two equal", |b, _, l| {
                let at = l.samples_off() + 8 * 3;
                let x = word(b, at);
                put(b, at + 8, x)
            }),
            ("past the last number", |b, t, l| {
                let at = l.samples_off() + 8 * (l.samples - 1);
                put(b, at, t.sparse.unwrap().last + 1)
            }),
        ];
        let no = ZError::Resident("the samples are not the first number ascending to the last");
        for (name, edit) in samples {
            let b = forged(None, edit);
            assert_eq!(StrideBody::from_bytes(&b).err(), Some(no), "{name}");
        }
        // The pages of numbers: `v`, `v + 1` and `v + 2`.
        let pages_of: [(&str, u32, Option<u32>, Edit); 6] = [
            ("two out of order", 1, Some(1), |b, _, l| {
                let at = l.k_off() + 8 * 517;
                let (x, y) = (word(b, at), word(b, at + 8));
                put(b, at, y);
                put(b, at + 8, x);
            }),
            ("two equal", 1, Some(1), |b, _, l| {
                let at = l.k_off() + 8 * 517;
                let x = word(b, at);
                put(b, at + 8, x)
            }),
            ("a sample's number moved", 1, Some(1), |b, _, l| {
                let at = l.k_off() + 8 * 576;
                let x = word(b, at);
                assert!(x + 1 < word(b, at + 8));
                put(b, at, x + 1)
            }),
            ("a byte past the last number", 2, Some(2), |b, _, l| {
                let at = l.k_off() + 8 * 1500;
                assert!(at < l.res_off);
                b[at as usize + 3] = 1
            }),
            ("the last number moved", 2, Some(2), |b, _, l| {
                let at = l.k_off() + 8 * 1499;
                let x = word(b, at);
                put(b, at, x + 1)
            }),
            ("the trailer's last number moved", 2, None, |_, t, _| {
                let sp = t.sparse.unwrap();
                t.sparse = Some(StrideSparse {
                    last: sp.last + 1,
                    ..sp
                })
            }),
        ];
        assert!(last < u64::MAX);
        for (name, k, reword, edit) in pages_of {
            let b = forged(reword.map(|p| v + p), edit);
            let body = StrideBody::from_bytes(&b)
                .unwrap_or_else(|e| panic!("{name}: every checksum is good: {e:?}"));
            for p in 0..pages {
                assert_eq!(body.page_ok(&b, p), p != v + k, "{name}: page {p}");
            }
        }
        // A byte between the row and the samples: a file whose row's words
        // end off a multiple of eight, thirty-two pages of values and three
        // of numbers.
        let odd = Fixture::new("sparse-odd-row", id_recs(1300, 100, 0x62, false), 1 << 16);
        sparse_built(&odd);
        let lay = StrideBody::from_bytes(&std::fs::read(&odd.out).unwrap())
            .unwrap()
            .trailer()
            .layout()
            .unwrap();
        assert_eq!(lay.row_len() % 8, 4, "{lay:?}");
        forge_stride(&odd.out, None, |b, _, l| {
            b[(l.res_off + l.row_len()) as usize + 1] = 1
        });
        assert_eq!(
            StrideBody::from_bytes(&std::fs::read(&odd.out).unwrap()).err(),
            Some(ZError::Resident(
                "a byte between the row and the samples is not zero"
            ))
        );
    }

    /// A sample that says a run begins later than it does, in a file whose
    /// every checksum is good, sends the keys from the run's true first
    /// number up to the sample's to the run before, which does not hold
    /// them. The search does not answer them absent: a key above a whole
    /// run is held to the next run's first number where it lies, and that
    /// page's first touch finds the sample is not its number. The run
    /// before, on a page of its own, answers as it did; the key the sample
    /// lies about and a key in the gap below it are a page that failed, the
    /// raw form's to answer. Forged at a page's first number, the one
    /// sample whose run is on a page the misled search does not read.
    #[test]
    fn a_sample_that_lies_under_good_checksums_is_refused_and_never_an_absence() {
        let fx = Fixture::new(
            "sparse-sample-lie",
            id_recs(1500, 100, 0x5f, false),
            1 << 16,
        );
        assert_eq!(fx.recs.len(), 1500, "distinct ids");
        sparse_built(&fx);
        // Sample 8 is rank 512, the first number of the second page of
        // numbers: raised by one, still below the next sample.
        forge_stride(&fx.out, None, |b, _, l| {
            let at = l.samples_off() as usize + 8 * 8;
            let x = u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
            b[at..at + 8].copy_from_slice(&(x + 1).to_le_bytes());
        });
        let bytes = std::fs::read(&fx.out).unwrap();
        let body = StrideBody::from_bytes(&bytes).expect("every checksum is good");
        let v = body.pages() as u32;
        let mut touched = Vec::new();
        let mut good = |p: u32| {
            touched.push(p);
            body.page_ok(&bytes, p)
        };
        // The last key of the first page of numbers, in the run before the
        // lie: found, on its own page.
        assert_eq!(
            body.rank_of(&bytes, &fx.recs[511].0, &mut good),
            Ok(Some(511))
        );
        // The key at rank 512: the samples send it to the run before, the
        // run does not hold it, and the next run's first number is on the
        // page whose sample lies.
        assert_eq!(body.rank_of(&bytes, &fx.recs[512].0, &mut good), Err(v + 1));
        // A key in the gap between the two runs, which the form does not
        // hold: not absent on the samples' word, but the same page's
        // failure.
        let codec = *body.codec();
        let mut kbuf = [0u8; STRIDE_KEY_MAX];
        let between = codec.decode(&fx.recs[511].0).unwrap() + 1;
        assert!(between < codec.decode(&fx.recs[512].0).unwrap());
        let gap = codec.render(between, &mut kbuf).to_vec();
        assert_eq!(body.rank_of(&bytes, &gap, &mut good), Err(v + 1));
        assert_eq!(body.lower_bound(&bytes, &gap, &mut good), Err(v + 1));
        // A key further into the second page's first run is sent there, and
        // the page fails as itself.
        assert_eq!(body.rank_of(&bytes, &fx.recs[530].0, &mut good), Err(v + 1));
        assert!(
            touched.contains(&v) && touched.contains(&(v + 1)),
            "{touched:?}"
        );
        // Through a file a store attaches the lie is damage wherever the
        // attach's sample reads the page, and a fallback to the raw form
        // where it does not: every key the raw form's answer, none absent.
        match fx.attach() {
            Err(Reject::Damage(_)) => {}
            Err(e) => panic!("{e:?}"),
            Ok(zfile) => {
                agree(&fx.memo, &zfile, &fx.blob, &fx.recs);
                assert!(zfile.damaged(), "the page's failure took the file out");
            }
        }
    }

    /// The body refuses bytes its trailer does not describe, each check
    /// reached with the ones before it passing: a length of no whole pages;
    /// a page more than the trailer's sizes; a word of the row the row's
    /// CRC does not cover; a byte after the row; a byte of the trailer. A
    /// page checks against its word, and a slot that straddles two pages
    /// is held to both.
    #[test]
    fn a_stride_body_refuses_bytes_its_trailer_does_not_describe() {
        let fx = Fixture::new("stride-body", stride_recs(1500, 100, 0x5e), 1 << 16);
        assert!(matches!(
            build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::Only)).unwrap(),
            Outcome::Built(_)
        ));
        let bytes = std::fs::read(&fx.out).unwrap();
        let body = StrideBody::from_bytes(&bytes).unwrap();
        let lay = body.trailer().layout().unwrap();
        let (len, res) = (bytes.len(), lay.res_off as usize);
        assert!(
            lay.res_len < lay.res_pages * ZPAGE as u64,
            "the row has padding"
        );
        let short = &bytes[..len - 1];
        assert_eq!(StrideBody::from_bytes(short).err(), Some(ZError::Length));
        let mut longer = bytes[..len - ZTRAILER].to_vec();
        longer.extend_from_slice(&[0u8; ZPAGE]);
        longer.extend_from_slice(&bytes[len - ZTRAILER..]);
        assert_eq!(StrideBody::from_bytes(&longer).err(), Some(ZError::Length));
        let mut row = bytes.clone();
        row[res + 5] ^= 1;
        assert_eq!(
            StrideBody::from_bytes(&row).err(),
            Some(ZError::ResidentCrc)
        );
        let mut pad = bytes.clone();
        pad[res + lay.res_len as usize] = 1;
        assert!(
            matches!(StrideBody::from_bytes(&pad), Err(ZError::Resident(_))),
            "a byte after the row"
        );
        let mut trailer = bytes.clone();
        trailer[len - ZTRAILER + 30] ^= 1;
        assert_eq!(
            StrideBody::from_bytes(&trailer).err(),
            Some(ZError::TrailerCrc)
        );
        // A value byte: its page fails its word, the others pass theirs.
        let mut value = bytes.clone();
        value[ZPAGE + 3] ^= 1;
        for p in 0..body.pages() as u32 {
            assert_eq!(body.page_ok(&value, p), p != 1, "page {p}");
        }
        // A slot across a page's end, at a hundred bytes a value: the
        // first page's failure and the second's are each the read's.
        let w = body.trailer().w as u64;
        let rank = (0..body.keys())
            .find(|r| (r * w) / ZPAGE as u64 != (r * w + w - 1) / ZPAGE as u64)
            .expect("a slot that straddles");
        let first = (rank * w / ZPAGE as u64) as u32;
        for bad in [first, first + 1] {
            assert_eq!(
                body.values_at(&bytes, rank, &mut |p| p != bad),
                StrideAt::Bad(bad),
                "page {bad} of rank {rank}'s two"
            );
        }
    }

    /// The sample both bodies' attaches take (`sample_pages`), held to its
    /// doc by what it must be and not by a second copy of its arithmetic,
    /// which the tests that forge or flip a page around the sample call
    /// instead: every page of a file of at most `ZSAMPLE_PAGES`, and of a
    /// longer one `ZSAMPLE_PAGES` pages ascending, the first and the last
    /// among them, each step at least a page and no step more than a page
    /// longer than another -- the even spacing, which leaves no stretch of
    /// the file unsampled for longer than its share.
    #[test]
    fn an_attach_samples_the_first_and_the_last_page_and_evenly_between() {
        let sizes = (1..=4 * ZSAMPLE_PAGES + 3).chain([239, 1000, 10_007, 1 << 20]);
        for n in sizes {
            let s = sample_pages(n);
            if n <= ZSAMPLE_PAGES {
                assert_eq!(s, (0..n).collect::<Vec<_>>(), "{n} pages: every one");
                continue;
            }
            assert_eq!(s.len(), ZSAMPLE_PAGES, "{n} pages: {s:?}");
            assert_eq!((s[0], s[ZSAMPLE_PAGES - 1]), (0, n - 1), "{n}: {s:?}");
            let steps: Vec<usize> = s.windows(2).map(|w| w[1] - w[0]).collect();
            let (lo, hi) = (steps.iter().min().unwrap(), steps.iter().max().unwrap());
            assert!(*lo >= 1 && hi - lo <= 1, "{n} pages: steps {steps:?}");
        }
    }

    // ---- the stride form's reader -------------------------------------------

    /// A walk's emits folded, key and value each with its length, so that
    /// two walks of millions of records compare without holding them.
    fn fold(h: &mut u64, k: &[u8], v: &[u8]) {
        for part in [k, v] {
            *h = h
                .wrapping_mul(0x100_0000_01b3)
                .wrapping_add(part.len() as u64)
                .wrapping_mul(0x100_0000_01b3)
                .wrapping_add(u64::from(crate::block::crc32(part)));
        }
    }

    /// A fixture's stride file, built under `StrideAsk::OrBlock` as a store
    /// asks under `ZStride::Forced`, and the build's outcome; the file under
    /// the fixture's name.
    fn stride_built(fx: &Fixture) -> Built {
        let _ = std::fs::remove_file(&fx.out);
        let made = build(&fx.seg, &fx.out, ID, END, &stride_plan(StrideAsk::OrBlock)).unwrap();
        match made {
            Outcome::Built(b) if b.coding == FileCoding::Stride => b,
            other => panic!("not a stride file: {other:?}"),
        }
    }

    /// A partition of numbered keys whose values of one width lie across
    /// page ends, from a fixture of its own.
    fn stride_fixture(name: &str, n: usize, w: usize, seed: u64) -> Fixture {
        Fixture::new(name, stride_recs(n, w, seed), 1 << 16)
    }

    /// Every read a form file answers, of a stride file, against the
    /// segment's own reader, the `Blob` the raw form is: the values and the
    /// key at every rank and past the run; the read, the count, the seek
    /// and the cut of every key and of keys beside the run's; the owner's
    /// span, exactly the owner, for every one of them; and the walk from
    /// every rank at limits of none, one, a block less one, a block, a
    /// block and one, and the run. Values of one byte, of a hundred that
    /// straddle pages, of three thousand, most of whose slots straddle a
    /// page end, of a page less one, which leaves the last page with no slot
    /// starting on it, and of a page, a run of one, and a big-endian field;
    /// and runs whose first number is not zero, so that every rank's key is
    /// the base's arithmetic and not the rank's -- from 997, whose field
    /// carries at 0999 and at 1999, a run of twenty from 990 whose field is
    /// the four digits that carry, and a big-endian run whose low byte
    /// carries into the next. And two runs that end at their field's top,
    /// decimal at 9999 and big-endian at 0xffff, where the number after the
    /// last is one the field cannot render: a walk from the run's end, a
    /// seek, a cut and an owner past the last key each answer there without
    /// rendering it. The key by rank goes into a buffer holding another
    /// key's bytes, which a served key replaces and an answer of none
    /// leaves as it was, the contract the block cache keeps a buffer by
    /// (`Reader::cache_key_at`). The widest the builder takes is a page, and
    /// one wider is a decline (`the_stride_form_declines_each_state_...`).
    #[test]
    fn every_read_of_a_stride_file_answers_as_the_segment_does() {
        let mut rng = Rng(0x5be1);
        let be_from = |start: u64, n: u64, rng: &mut Rng| -> Vec<Rec> {
            (start..start + n)
                .map(|i| {
                    let key = [b"be".as_slice(), &i.to_be_bytes()].concat();
                    (key, vec![rng.bytes(100)], false)
                })
                .collect()
        };
        let be = be_from(0x0102_0304_0506_0000, 500, &mut rng);
        let be_carry = be_from(0x0102_0304_0506_00f0, 300, &mut rng);
        // The last key ...ffff: the run's common prefix is the six bytes
        // before the field, so the field is two bytes and its top is it.
        let be_top = be_from(0x0102_0304_0506_fe80, 384, &mut rng);
        let mut cases = vec![
            ("w1", stride_recs(5000, 1, 11), false),
            ("w100", stride_recs(3000, 100, 12), false),
            ("w3000", stride_recs(30, 3000, 13), false),
            ("w4095", stride_recs(40, 4095, 14), false),
            ("w4096", stride_recs(40, 4096, 15), false),
            ("one", stride_recs(1, 100, 16), false),
            ("be", be, false),
            ("from997", stride_recs_from(997, 1200, 100, 17), false),
            ("carry", stride_recs_from(990, 20, 100, 18), false),
            ("be-carry", be_carry, false),
            // k998500 to k999999: the common prefix `k99`, a field of four
            // digits from 8500, and the last key its top.
            ("top", stride_recs_from(998_500, 1500, 100, 19), false),
            ("be-top", be_top, false),
        ];
        // The same reads of sparse files: runs with gaps of each kind of
        // key (`gapped_cases`), and one whose last key is its field's top,
        // k999999 behind the prefix the run shares.
        for (name, recs, _) in gapped_cases() {
            cases.push((name, recs, true));
        }
        let mut to_top = gapped_recs(997_000, 500, 100, 20);
        to_top.last_mut().unwrap().0 = b"k999999".to_vec();
        cases.push(("gap-top", to_top, true));
        for (name, recs, sparse) in cases {
            let fx = Fixture::new(&format!("stride-api-{name}"), recs, 1 << 16);
            let built = if sparse {
                sparse_built(&fx)
            } else {
                stride_built(&fx)
            };
            let zfile = fx.attach().unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let body = zfile.stride().expect("a stride body");
            assert_eq!(body.sparse().is_some(), sparse, "{name}");
            // A handle's memo whose sparse walks step a key by its field
            // (`Options::zpages_stride_step_direct`), beside the fixture's,
            // which renders each whole.
            let direct = ZMemo::new().stride_direct(true);
            let n = fx.recs.len();
            let mut first = [0u8; STRIDE_KEY_MAX];
            assert_eq!(
                body.codec().render(body.codec().base(), &mut first),
                fx.recs[0].0.as_slice(),
                "{name}: the base renders the first key"
            );
            let w = fx.recs[0].1[0].len() as u64;
            assert!(zfile.is_stride() && !zfile.coded(), "{name}");
            assert_eq!(zfile.kind(), FileCoding::Stride, "{name}");
            let ends_at_top = name.ends_with("top");
            let numbers = numbers_of(body, &fx);
            assert_eq!(
                numbers[n - 1] == body.codec().top(),
                ends_at_top,
                "{name}: the run ends at its field's top only where it says"
            );
            assert_eq!(zfile.pages(), body.pages(), "{name}");
            assert_eq!(zfile.group_bytes(), n as u64 * w, "{name}");
            assert_eq!(zfile.file_bytes(), built.file_bytes, "{name}");
            // The empty fence's one key offset, no filter, a word of the row
            // a page the row covers, a sparse form's samples, and the body
            // itself.
            let samples = if sparse { n.div_ceil(64) } else { 0 };
            assert_eq!(
                zfile.resident_bytes(),
                4 + 4 * body.checked_pages() + 8 * samples + std::mem::size_of::<StrideBody>(),
                "{name}: an empty fence and no filter beside the row"
            );
            assert_eq!(body.checked_pages() > body.pages(), sparse, "{name}");
            // By rank: the values and the key, and nothing past the run. The
            // key into a buffer that holds bytes already: a served key is
            // the key alone, and none leaves the bytes.
            let junk = b"a key read before, and longer than any here".to_vec();
            let mut kbuf = Vec::new();
            for rank in 0..n + 2 {
                let mut want = Vec::new();
                let raw_n = fx.blob.values_at(rank, |v| want.push(v.to_vec())).unwrap();
                let mut got = Vec::new();
                let z = zfile
                    .values_at(&fx.memo, &fx.blob, rank, &mut |v: &[u8]| {
                        got.push(v.to_vec())
                    })
                    .unwrap();
                assert_eq!(
                    z,
                    ZRead::Served {
                        n: raw_n,
                        decoded: false
                    },
                    "{name}: rank {rank}"
                );
                assert_eq!(got, want, "{name}: the values at {rank}");
                kbuf.clone_from(&junk);
                let z = zfile.key_at_into(&fx.memo, rank, &mut kbuf);
                match fx.blob.key_at(rank) {
                    Some(k) => {
                        assert_eq!(
                            z,
                            ZRead::Served {
                                n: 1,
                                decoded: false
                            },
                            "{name}: {rank}"
                        );
                        assert_eq!(kbuf, k, "{name}: the key at {rank}");
                    }
                    None => {
                        assert_eq!(
                            z,
                            ZRead::Served {
                                n: 0,
                                decoded: false
                            },
                            "{name}"
                        );
                        assert_eq!(kbuf, junk, "{name}: none past the run leaves the buffer");
                    }
                }
            }
            // By key: the run's keys and keys beside them, shaped and not.
            let codec = body.trailer().codec;
            let mut keys = probes(&fx.recs);
            let mut r = [0u8; STRIDE_KEY_MAX];
            // Numbers beside the run's and inside its gaps: below the
            // first, past the last, the field's two ends, and either side
            // of every key's number.
            let mut beside = vec![
                codec.base().checked_sub(1),
                codec.base().checked_add(n as u64),
                codec.base().checked_add(n as u64 + 7),
                Some(codec.top()),
                Some(0),
            ];
            for &x in &numbers {
                beside.extend([x.checked_sub(1), x.checked_add(1)]);
            }
            for x in beside.into_iter().flatten().filter(|&x| x <= codec.top()) {
                keys.push(codec.render(x, &mut r).to_vec());
            }
            let sorted: Vec<&[u8]> = fx.recs.iter().map(|r| r.0.as_slice()).collect();
            for key in &keys {
                let want = raw_values(&fx.blob, key);
                assert_eq!(
                    z_values(&fx.memo, &zfile, &fx.blob, key).as_ref(),
                    Some(&want),
                    "{name}: read of {key:?}"
                );
                assert_eq!(
                    served(zfile.count(&fx.memo, key)),
                    fx.blob.count(key).unwrap(),
                    "{name}: count of {key:?}"
                );
                let rank = fx.blob.seek(key);
                let at = if sorted.binary_search(&key.as_slice()).is_ok() {
                    KeyOrder::Equal
                } else {
                    KeyOrder::Greater
                };
                assert_eq!(
                    zfile.seek(&fx.memo, key),
                    ZSeek::Served {
                        rank,
                        at,
                        decoded: false
                    },
                    "{name}: seek of {key:?}"
                );
                let owner = if at == KeyOrder::Equal {
                    rank
                } else {
                    rank.saturating_sub(1)
                };
                let mut hint = usize::MAX;
                assert_eq!(
                    zfile.owner_span_from(key, &mut hint),
                    Some((owner, owner + 1)),
                    "{name}: the owner of {key:?}"
                );
                for (lo, hi) in [(0usize, n), (0, 1), (n / 3, 2 * n / 3), (n - 1, n), (n, n)] {
                    let r = (lo..hi)
                        .find(|&i| sorted[i] >= key.as_slice())
                        .unwrap_or(hi);
                    let a = if r < hi && sorted[r] == key.as_slice() {
                        KeyOrder::Equal
                    } else {
                        KeyOrder::Greater
                    };
                    assert_eq!(
                        zfile.cut_in(&fx.memo, lo, hi, key),
                        ZSeek::Served {
                            rank: r,
                            at: a,
                            decoded: false
                        },
                        "{name}: cut of {key:?} in {lo}..{hi}"
                    );
                }
            }
            // The walk from every rank, past the end too.
            for from in 0..=n + 1 {
                for limit in [0, 1, 63, 64, 65, n] {
                    let (mut want, mut got) = (0u64, 0u64);
                    let raw_seen = fx
                        .blob
                        .scan_at(from, limit, |k, v| fold(&mut want, k, v))
                        .unwrap();
                    let z = zfile
                        .scan_at(
                            &fx.memo,
                            &fx.blob,
                            from,
                            limit,
                            &mut |k: &[u8], v: &[u8]| fold(&mut got, k, v),
                        )
                        .unwrap();
                    assert_eq!(
                        (z.seen, z.end, got),
                        (raw_seen, ScanEnd::Served, want),
                        "{name}: a walk of {limit} from {from}"
                    );
                    assert_eq!((z.decoded, z.inplace, z.nopage), (0, 0, 0), "{name}");
                    // The same walk of a sparse file with each key stepped
                    // by its field (`Options::zpages_stride_step_direct`),
                    // through the file and through a cursor's bulk walk: the
                    // keys and the values the raw form's, as above. A whole
                    // run steps no key by its number, so it has no such walk.
                    if sparse && (from % 5 == 0 || from + 2 >= n) {
                        let mut dgot = 0u64;
                        let z = zfile
                            .scan_at(&direct, &fx.blob, from, limit, &mut |k: &[u8], v: &[u8]| {
                                fold(&mut dgot, k, v)
                            })
                            .unwrap();
                        assert_eq!(
                            (z.seen, z.end, dgot),
                            (raw_seen, ScanEnd::Served, want),
                            "{name}: a walk of {limit} from {from}, stepped by the field"
                        );
                        let mut cgot = 0u64;
                        let mut c = ZCursor::at(&direct, &zfile, &fx.blob, from);
                        let (walked, _) = c
                            .walk_until(None, limit, &mut |k: &[u8], v: &[u8]| {
                                fold(&mut cgot, k, v)
                            })
                            .unwrap();
                        assert_eq!(
                            (walked, cgot),
                            (raw_seen, want),
                            "{name}: a cursor's walk of {limit} from {from}, stepped by the field"
                        );
                        assert!(!c.fell_back(), "{name}");
                    }
                }
            }
            // And a cursor stepped a rank at a time under the option, the
            // key at each rank the raw form's and none past the run: a step
            // by the field for every rank after the first, counted, where
            // the walks above counted theirs and the fixture's memo, which
            // has the option off, counted none. A whole run counts none
            // under either memo.
            let walked_direct = direct.take_stride().direct;
            assert_eq!(
                walked_direct > 0,
                sparse && n > 1,
                "{name}: keys stepped by the field in the walks: {walked_direct}"
            );
            let mut c = ZCursor::at(&direct, &zfile, &fx.blob, 0);
            for rank in 0..n {
                assert_eq!(
                    c.key(),
                    fx.blob.key_at(rank),
                    "{name}: stepped by the field to {rank}"
                );
                c.advance();
            }
            assert_eq!(c.key(), None, "{name}: past the run");
            assert!(!c.fell_back(), "{name}");
            drop(c);
            assert_eq!(
                direct.take_stride().direct,
                if sparse { n as u64 - 1 } else { 0 },
                "{name}: a step by the field for every rank after the first"
            );
            assert_eq!(
                fx.memo.take_stride().direct,
                0,
                "{name}: none stepped by the field without the option"
            );
            assert!(!zfile.damaged() && zfile.bad_pages() == 0, "{name}");
            assert_eq!(
                zfile.verified_pages(),
                body.checked_pages() as u64,
                "{name}"
            );
        }
    }

    /// A stride walk asks for the pages it will cross after its first where
    /// the scan's prefetch is on, and only then; one that stays inside its
    /// first page asks nothing. The ask is held to its pages, from the
    /// slots' bytes and not from the body's span: the page after the walk's
    /// first and on, at most `ZPREFETCH_PAGES` of them, the cap reached
    /// exactly and passed by one page.
    #[test]
    fn a_stride_walk_asks_for_the_pages_it_will_cross_under_the_scans_prefetch() {
        let fx = stride_fixture("stride-ahead", 3000, 100, 0x5a);
        stride_built(&fx);
        let attach = |prefetch: bool| {
            ZFile::attach(
                &fx.out,
                &fx.blob,
                ID,
                END,
                false,
                true,
                prefetch,
                true,
                false,
                FenceSearch::Keys,
                true,
                &StrideAttach::Check,
            )
            .unwrap()
        };
        // The pages a walk asks for: from the page after the one its first
        // slot starts on to the one its last slot ends on, capped.
        let w = 100u64;
        let want_ask = |from: u64, want: u64| -> Option<(u32, usize)> {
            let first = (from * w / ZPAGE as u64) as usize;
            let last = (((from + want) * w - 1) / ZPAGE as u64) as usize;
            (last > first).then(|| ((first + 1) as u32, (last - first).min(ZPREFETCH_PAGES)))
        };
        // A walk whose last slot ends on the page `ZPREFETCH_PAGES` after
        // its first, and one on the page after that.
        let at_cap = (ZPREFETCH_PAGES * ZPAGE) as u64 / w + 1;
        let past_cap = ((ZPREFETCH_PAGES + 1) * ZPAGE) as u64 / w + 1;
        assert_eq!(want_ask(0, at_cap), Some((1, ZPREFETCH_PAGES)));
        assert_eq!(want_ask(0, at_cap - 1), Some((1, ZPREFETCH_PAGES - 1)));
        assert_eq!(want_ask(0, past_cap), Some((1, ZPREFETCH_PAGES)));
        {
            let (on, off) = (attach(true), attach(false));
            let body = on.stride().unwrap();
            for (from, want) in [
                (0, 3000),
                (100, 200),
                (0, 41),
                (0, 40),
                (0, 30),
                (0, at_cap),
                (0, at_cap - 1),
                (0, past_cap),
                (7, past_cap),
                (2900, 100),
            ] {
                assert_eq!(
                    on.stride_ask_ahead(body, from, want),
                    want_ask(from, want),
                    "a walk of {want} from {from}"
                );
                assert_eq!(
                    off.stride_ask_ahead(body, from, want),
                    None,
                    "the option off"
                );
            }
        }
        let walk = |prefetch: bool, from: usize, limit: usize| {
            let z = attach(prefetch);
            z.scan_at(
                &fx.memo,
                &fx.blob,
                from,
                limit,
                &mut |_: &[u8], _: &[u8]| {},
            )
            .unwrap()
        };
        assert_eq!(walk(true, 0, 3000).prefetches, 1);
        assert_eq!(
            walk(true, 100, 200).prefetches,
            1,
            "a walk across page ends"
        );
        // Ranks 0 to 40, bytes to 4,100: the first page and four bytes of
        // the second, one page to ask for.
        assert_eq!(
            walk(true, 0, 41).prefetches,
            1,
            "a walk across one page end"
        );
        assert_eq!(
            walk(true, 0, 40).prefetches,
            0,
            "a walk to its first page's last slot"
        );
        assert_eq!(
            walk(true, 0, 30).prefetches,
            0,
            "a walk inside its first page"
        );
        assert_eq!(walk(false, 0, 3000).prefetches, 0, "the option off");
    }

    /// A mapping of a stride file's temp name made at the build's damage
    /// seam, one value page touched through it, held until the build has
    /// returned: a page a mapping holds is one `POSIX_FADV_DONTNEED` skips,
    /// so the drop that a plan asks for leaves it in the cache.
    static HELD_MAPPING: std::sync::Mutex<Option<MmapBytes>> = std::sync::Mutex::new(None);

    fn hold_a_page_mapped(tmp: &Path) {
        let map = MmapBytes::open(tmp).unwrap();
        std::hint::black_box(map.0[ZPAGE]);
        *HELD_MAPPING.lock().unwrap() = Some(map);
    }

    /// A stride build is trusted -- at the attach that follows it and at a
    /// later attach of the file it leaves on disk alike, by the one word it
    /// leaves (`Built::left_seal`, `Options::z_stride_attach_built`) -- only
    /// where its read-back read the device, by the kernel's count of the
    /// file's pages in the cache as the read-back began, never by the plan's
    /// word that it dropped them. A build that keeps its pages read back the
    /// cache, which says nothing of the copy the reads after any attach take
    /// from the device once the cache gives a page up: checked page by page.
    /// A build that drops its output (`BuildPlan::drop_output`, the builds
    /// ahead and at a merge's finish under the selector's line) drops its
    /// last piece too before the read-back, the file wholly out of the cache
    /// where the drop acts, on Linux; a drop that leaves a page resident --
    /// one a mapping holds, here made through the damage seam, and every
    /// page of a drop off Linux, where the drop does nothing -- is a
    /// read-back of the cache all the same. With the trust off every build
    /// is checked, and with the form off every stride file is refused.
    #[test]
    fn a_stride_build_is_trusted_only_where_its_read_back_read_the_device() {
        use crate::db::{Options, ZStride};
        let fx = stride_fixture("stride-dropseal", 3000, 100, 0x5d);
        let opts = |stride: ZStride, trust: bool| Options {
            zpages_stride: stride,
            zpages_stride_trust_built: trust,
            ..Options::default()
        };
        for (drop_output, hold) in [(false, false), (true, false), (true, true)] {
            let _ = std::fs::remove_file(&fx.out);
            let plan = BuildPlan {
                drop_output,
                damage: hold.then_some(hold_a_page_mapped as fn(&Path)),
                ..stride_plan(StrideAsk::OrBlock)
            };
            let made = build(&fx.seg, &fx.out, ID, END, &plan).unwrap();
            let held = HELD_MAPPING.lock().unwrap().take();
            assert_eq!(held.is_some(), hold, "the seam ran where it was asked");
            drop(held);
            let b = match made {
                Outcome::Built(b) if b.coding == FileCoding::Stride => b,
                other => panic!("not a stride file: {other:?}"),
            };
            let bytes = std::fs::read(&fx.out).unwrap();
            let word = trailer_crc(&bytes[bytes.len() - ZTRAILER..]);
            assert!(word.is_some());
            let device = drop_output && !hold && cfg!(target_os = "linux");
            let what = format!("drop_output {drop_output}, a page held {hold}");
            assert_eq!(
                (b.stride_seal, b.stride_device, b.left_seal()),
                (word, device, if device { word } else { None }),
                "{what}"
            );
            assert_eq!(
                opts(ZStride::Forced, true).z_stride_attach_built(&b),
                match word.filter(|_| device) {
                    Some(w) => StrideAttach::Trust(w),
                    None => StrideAttach::Check,
                },
                "{what}: the attach that follows the build"
            );
            assert_eq!(
                opts(ZStride::Forced, false).z_stride_attach_built(&b),
                StrideAttach::Check,
                "{what}: the trust off"
            );
            assert_eq!(
                opts(ZStride::Off, true).z_stride_attach_built(&b),
                StrideAttach::Refuse,
                "{what}: the form off"
            );
        }
    }

    /// A build reads its own writeback's error where it waits for the
    /// writeback, and an error fails the build with nothing named and the
    /// temporary name gone. A stride build reads it before its read-back
    /// (`read_writeback`): a read-back of the cache checks the cache's copy,
    /// which a write the device failed leaves right over a wrong one on the
    /// device, the copy every read takes once the cache gives the page up.
    /// A build that drops its output, of either body, reads it as it drops
    /// each piece (`drop_written_range`), and that wait is the only one that
    /// sees it: the kernel reports a writeback error once to a descriptor,
    /// and the seam answers the first wait after it is armed and no other
    /// (`fail_next_writeback`), as the kernel would. A block build that keeps
    /// its pages waits for nothing, and builds with the seam armed. Without
    /// the seam every plan builds its file.
    #[test]
    fn a_build_whose_writeback_fails_names_nothing() {
        let fx = stride_fixture("stride-wbfail", 3000, 100, 0x5e);
        let linux = cfg!(target_os = "linux");
        for (ask, drop_output, fails) in [
            (StrideAsk::OrBlock, false, true),
            (StrideAsk::OrBlock, true, true),
            (StrideAsk::No, true, linux),
            (StrideAsk::No, false, false),
        ] {
            let what = format!("{ask:?}, drop_output {drop_output}");
            let _ = std::fs::remove_file(&fx.out);
            let plan = BuildPlan {
                drop_output,
                ..stride_plan(ask)
            };
            fail_next_writeback(true);
            let made = build(&fx.seg, &fx.out, ID, END, &plan);
            fail_next_writeback(false);
            if fails {
                assert!(made.is_err(), "{what}: {made:?}");
                assert!(!fx.out.exists(), "{what}: nothing named");
            } else {
                assert!(
                    matches!(made, Ok(Outcome::Built(_))),
                    "{what}: no wait, no error: {made:?}"
                );
            }
            assert!(
                !fx.out.with_extension("zpg.tmp").exists(),
                "{what}: the temporary name gone"
            );
            let _ = std::fs::remove_file(&fx.out);
            let want = if ask == StrideAsk::No {
                FileCoding::Block
            } else {
                FileCoding::Stride
            };
            match build(&fx.seg, &fx.out, ID, END, &plan).unwrap() {
                Outcome::Built(b) if b.coding == want => {}
                other => panic!("{what}: not the file asked for: {other:?}"),
            }
        }
    }

    /// The read-back's word that it read the device is the kernel's count
    /// of none of the file's pages cached, and a count the kernel will not
    /// give is no count of none: a build that drops its output and whose
    /// count is refused leaves no word (`read_back_residency`). The seam
    /// answers the refusal, which a `mincore` over a live mapping never
    /// makes; the same plan without it leaves its word on Linux.
    #[test]
    fn a_stride_build_whose_residency_is_unknown_leaves_no_word() {
        let fx = stride_fixture("stride-nores", 3000, 100, 0x5f);
        for unknown in [true, false] {
            let _ = std::fs::remove_file(&fx.out);
            let plan = BuildPlan {
                drop_output: true,
                residency_unknown: unknown,
                ..stride_plan(StrideAsk::OrBlock)
            };
            let b = match build(&fx.seg, &fx.out, ID, END, &plan).unwrap() {
                Outcome::Built(b) if b.coding == FileCoding::Stride => b,
                other => panic!("not a stride file: {other:?}"),
            };
            let device = !unknown && cfg!(target_os = "linux");
            assert!(b.stride_seal.is_some(), "unknown {unknown}");
            assert_eq!(
                (b.stride_device, b.left_seal().is_some()),
                (device, device),
                "the count refused {unknown}"
            );
        }
    }

    /// A stride file's value pages are checked against the row at their
    /// first touch in this process and never again, the check counted on
    /// the reading memo; a file the attach trusts as the one a build of this
    /// process wrote and read back is checked nowhere -- every page verified
    /// from the attach -- and a trailer to trust that is not the file's is
    /// the file found on disk, checked page by page. The trust's limit, held
    /// as a test: a page changed in place under the mapping after a trusted
    /// attach is served as it reads, where the checked attach refuses it.
    #[test]
    fn a_stride_files_pages_are_checked_at_first_touch_unless_the_build_is_trusted() {
        let fx = stride_fixture("stride-trust", 3000, 100, 0x7e);
        let built = stride_built(&fx);
        let t = built.stride_seal.expect("the build's trailer's word");
        let read_every_rank = |z: &ZFile| {
            for rank in 0..fx.recs.len() {
                let mut got = Vec::new();
                let r = z
                    .values_at(&fx.memo, &fx.blob, rank, &mut |v: &[u8]| {
                        got.push(v.to_vec())
                    })
                    .unwrap();
                assert_eq!(
                    r,
                    ZRead::Served {
                        n: 1,
                        decoded: false
                    },
                    "rank {rank}"
                );
                assert_eq!(got, fx.recs[rank].1, "rank {rank}");
            }
            fx.memo.take_stride().checks
        };
        let _ = fx.memo.take_stride();
        let checked = fx.attach_stride(StrideAttach::Check).unwrap();
        let pages = checked.pages() as u64;
        assert!(pages > 16, "{pages} pages");
        assert_eq!(checked.verified_pages(), 0, "none verified before a read");
        assert_eq!(read_every_rank(&checked), pages, "each page once");
        assert_eq!(checked.verified_pages(), pages);
        assert_eq!(read_every_rank(&checked), 0, "and never again");
        let trusted = fx.attach_stride(StrideAttach::Trust(t)).unwrap();
        assert_eq!(trusted.verified_pages(), pages, "verified from the attach");
        assert_eq!(read_every_rank(&trusted), 0, "nothing checked");
        let not_it = fx.attach_stride(StrideAttach::Trust(t ^ 1)).unwrap();
        assert_eq!(
            not_it.verified_pages(),
            0,
            "another trailer is not this file's"
        );
        assert_eq!(read_every_rank(&not_it), pages);
        assert!(matches!(
            fx.attach_stride(StrideAttach::Refuse),
            Err(Reject::Identity(_))
        ));
        // A value byte of page 1 flipped under the mappings.
        let rank = (ZPAGE + 50) / 100;
        {
            use std::os::unix::fs::FileExt;
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fx.out)
                .unwrap();
            let mut b = [0u8; 1];
            f.read_exact_at(&mut b, (ZPAGE + 50) as u64).unwrap();
            f.write_all_at(&[b[0] ^ 0x40], (ZPAGE + 50) as u64).unwrap();
        }
        let trusted = fx.attach_stride(StrideAttach::Trust(t)).unwrap();
        let mut got = Vec::new();
        let r = trusted
            .values_at(&fx.memo, &fx.blob, rank, &mut |v: &[u8]| {
                got.push(v.to_vec())
            })
            .unwrap();
        assert_eq!(
            r,
            ZRead::Served {
                n: 1,
                decoded: false
            }
        );
        assert_ne!(got, fx.recs[rank].1, "the trusted file's page as it reads");
        let checked = fx.attach_stride(StrideAttach::Check).unwrap();
        let r = checked
            .values_at(&fx.memo, &fx.blob, rank, &mut |_: &[u8]| panic!("a value"))
            .unwrap();
        assert_eq!(r, ZRead::Fallback, "the checked file refuses it");
    }

    /// What only a stride file counts goes onto the handle's slot line
    /// (`StrideTally`, `ZMemo::tally_into`) and nowhere a dispatch takes it
    /// from: every read of a block file counts nothing there; a stride
    /// file's point read counts the read and its pages' checks, a walk its
    /// checks alone, a count nothing, since a count touches no page. A memo
    /// with no tally, a test's, keeps the same counts itself. With the counts
    /// taken off the memo behind a flag every dispatch tested, a block
    /// file's read paid the flag's test.
    #[test]
    fn only_a_stride_files_reads_count_onto_the_handles_tally() {
        let fx = stride_fixture("stride-tally", 3000, 100, 0x7f);
        let _ = std::fs::remove_file(&fx.out);
        assert!(matches!(fx.build(), Outcome::Built(_)));
        let block = fx.attach().unwrap();
        assert!(!block.is_stride());
        let tally = StrideTally::new();
        let memo = ZMemo::new();
        // SAFETY: `tally` outlives `memo`, declared before it.
        unsafe { memo.tally_into(&tally) };
        let counts = |t: &StrideTally| StrideCounts {
            checks: t.checks.load(Ordering::Relaxed),
            reads: t.reads.load(Ordering::Relaxed),
            cursor: t.cursor.load(Ordering::Relaxed),
            direct: t.direct.load(Ordering::Relaxed),
        };
        let none = |what: &str| {
            assert_eq!(counts(&tally), StrideCounts::default(), "{what}");
            assert_eq!(memo.take_stride(), StrideCounts::default(), "{what}");
        };
        for (rank, (key, _, _)) in fx.recs.iter().enumerate() {
            block
                .read_all(&memo, &fx.blob, key, &mut |_: &[u8]| {})
                .unwrap();
            none("a block file's read");
            let _ = block.count(&memo, key);
            let _ = block.seek(&memo, key);
            block
                .values_at(&memo, &fx.blob, rank, &mut |_: &[u8]| {})
                .unwrap();
            let mut k = Vec::new();
            let _ = block.key_at_into(&memo, rank, &mut k);
            none("a block file's count, seek, values and key");
        }
        block
            .scan_at(
                &memo,
                &fx.blob,
                0,
                fx.recs.len(),
                &mut |_: &[u8], _: &[u8]| {},
            )
            .unwrap();
        none("a block file's walk");
        drop(block);
        stride_built(&fx);
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let _ = z.count(&memo, &fx.recs[0].0);
        none("a stride file's count");
        // Rank 40's slot straddles pages 0 and 1: two checks and the read,
        // on the tally and not on the memo.
        z.read_all(&memo, &fx.blob, &fx.recs[40].0, &mut |_: &[u8]| {})
            .unwrap();
        let read = StrideCounts {
            checks: 2,
            reads: 1,
            cursor: 0,
            direct: 0,
        };
        assert_eq!(counts(&tally), read, "the stride read on the tally");
        assert_eq!(
            memo.take_stride(),
            StrideCounts::default(),
            "and not the memo's"
        );
        z.read_all(&memo, &fx.blob, &fx.recs[40].0, &mut |_: &[u8]| {})
            .unwrap();
        assert_eq!(
            counts(&tally),
            StrideCounts { reads: 2, ..read },
            "the pages verified, the read counted again"
        );
        z.scan_at(&memo, &fx.blob, 0, 82, &mut |_: &[u8], _: &[u8]| {})
            .unwrap();
        assert_eq!(
            counts(&tally),
            StrideCounts {
                checks: 3,
                reads: 2,
                cursor: 0,
                direct: 0,
            },
            "ranks 0 to 81 end on page 2, the one page not checked before"
        );
        // A memo of no handle's: the same counts on the memo.
        drop(z);
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        z.read_all(&fx.memo, &fx.blob, &fx.recs[40].0, &mut |_: &[u8]| {})
            .unwrap();
        assert_eq!(
            fx.memo.take_stride(),
            read,
            "a memo with no tally keeps them"
        );
        assert_eq!(fx.memo.take_stride(), StrideCounts::default(), "taken");
    }

    /// A stride file's reads are its heat where the store asks for it
    /// (`ZFile::stride_heat`, `Options::zpages_stride_convert`),
    /// counted once in sixteen on the word a raw-tailed file counts its
    /// opens in, as the groups a block file would have opened for them: a
    /// point read one; a walk of a hundred ranks of a hundred bytes seven,
    /// a group holding the sixteen ranks that are the power of two under
    /// the twenty `ZGROUP_BYTES` hold; and a read of one value by its rank,
    /// which a walk by the key makes of every rank, one at each sixteenth
    /// rank and none between. A file not asked for its heat counts
    /// nothing, timing its decodes or not, and a count of a key, which
    /// touches no page, counts nothing either way.
    #[test]
    fn a_stride_files_reads_are_counted_as_the_groups_a_block_file_would_open() {
        let fx = stride_fixture("stride-touch", 3000, 100, 0x70c);
        stride_built(&fx);
        let memo = ZMemo::new();
        let opens = |z: &ZFile| z.opens16.load(Ordering::Relaxed);
        // Not timed: nothing counted.
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        assert_eq!(z.touch_shift, 4, "sixteen ranks a group");
        for i in 0..64 {
            z.read_all(&memo, &fx.blob, &fx.recs[i].0, &mut |_: &[u8]| {})
                .unwrap();
            z.scan_at(&memo, &fx.blob, i, 100, &mut |_: &[u8], _: &[u8]| {})
                .unwrap();
        }
        assert_eq!(opens(&z), 0, "a file that times nothing counts nothing");
        drop(z);
        let mut z = fx.attach_stride(StrideAttach::Check).unwrap();
        // The decodes timed and the heat not asked for: nothing still, the
        // count being the conversion's and no other policy's.
        z.time_decodes = true;
        z.read_all(&memo, &fx.blob, &fx.recs[0].0, &mut |_: &[u8]| {})
            .unwrap();
        for i in 0..64 {
            z.scan_at(&memo, &fx.blob, i, 100, &mut |_: &[u8], _: &[u8]| {})
                .unwrap();
        }
        assert_eq!(opens(&z), 0, "timed decodes alone count no stride read");
        drop(z);
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        z.ask_stride_heat();
        // Point reads: the thread's every sixteenth, one each.
        for i in 0..160 {
            z.read_all(&memo, &fx.blob, &fx.recs[i].0, &mut |_: &[u8]| {})
                .unwrap();
            let _ = z.count(&memo, &fx.recs[i].0);
        }
        assert_eq!(opens(&z), 10, "a hundred and sixty reads, ten counted");
        // Walks of a hundred ranks: one in sixteen, seven groups each.
        for i in 0..32 {
            z.scan_at(&memo, &fx.blob, i, 100, &mut |_: &[u8], _: &[u8]| {})
                .unwrap();
        }
        assert_eq!(opens(&z), 10 + 2 * 7, "thirty-two walks, two counted");
        // A value by its rank: the ranks a group begins at, one in sixteen
        // of those.
        let before = opens(&z);
        for rank in 0..16 * 16 * 2 {
            z.values_at(&memo, &fx.blob, rank, &mut |_: &[u8]| {})
                .unwrap();
        }
        assert_eq!(
            opens(&z),
            before + 2,
            "thirty-two group starts in five hundred and twelve ranks, two counted"
        );
    }

    /// The key ranges a plan asks for the stride form in
    /// (`BuildPlan::stride_spans`): a first key inside a range is asked,
    /// one at a range's end or before its start is not, a range with no end
    /// takes every key from its start, and a partition outside every range
    /// is built in the block form with no reason kept, where the same plan
    /// with its range around the partition builds the stride form.
    #[test]
    fn a_plan_asks_for_the_stride_form_in_its_key_ranges_alone() {
        let span = |lo: &[u8], hi: &[u8]| (lo.to_vec(), hi.to_vec());
        let spans = [span(b"b", b"d"), span(b"m", b"")];
        for (key, inside) in [
            (b"a".as_slice(), false),
            (b"b", true),
            (b"c", true),
            (b"czz", true),
            (b"d", false),
            (b"l", false),
            (b"m", true),
            (b"zzzz", true),
            (b"", false),
        ] {
            assert_eq!(in_spans(&spans, key), inside, "{key:?}");
        }
        assert!(!in_spans(&[], b"a"), "no range, no key");
        assert!(in_spans(&[span(b"", b"")], b""), "the whole key space");
        let fx = stride_fixture("stride-spans", 3000, 100, 0x70d);
        let first = fx.recs[0].0.clone();
        let mut past = first.clone();
        past.push(0);
        for (spans, stride) in [
            (vec![span(&first, b"")], true),
            (vec![span(b"", &past)], true),
            (vec![span(&past, b"")], false),
            (vec![span(b"", &first)], false),
            (vec![], false),
        ] {
            let _ = std::fs::remove_file(&fx.out);
            let plan = BuildPlan {
                stride_spans: Some(spans.clone().into()),
                ..stride_plan(StrideAsk::OrBlock)
            };
            let Outcome::Built(b) = build(&fx.seg, &fx.out, ID, END, &plan).unwrap() else {
                panic!("built: {spans:?}");
            };
            assert_eq!(b.coding == FileCoding::Stride, stride, "{spans:?}");
            assert_eq!(b.stride_no, None, "no reason either way: {spans:?}");
        }
    }

    /// Once a value page fails its check the stride file is out of service,
    /// and every read of it after -- of any page, the intact ones too -- is
    /// the raw form's: each read method answers `Out` before it reads
    /// anything. A walk across the page that fails stops where it stands,
    /// every rank before the page handed over and nothing after.
    #[test]
    fn a_stride_file_a_page_check_failed_in_answers_nothing_from_any_page() {
        let fx = stride_fixture("stride-out", 3000, 100, 0x0b);
        stride_built(&fx);
        let bad_page = 3usize;
        {
            use std::os::unix::fs::FileExt;
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fx.out)
                .unwrap();
            let at = (bad_page * ZPAGE + 7) as u64;
            let mut b = [0u8; 1];
            f.read_exact_at(&mut b, at).unwrap();
            f.write_all_at(&[b[0] ^ 1], at).unwrap();
        }
        // A walk over the damaged page, from a file attached fresh: every
        // page before it checked and passed, and its own check, which
        // failed, counted with them.
        let _ = fx.memo.take_point();
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let first_bad = bad_page * ZPAGE / 100;
        let mut got = 0u64;
        let r = z
            .scan_at(&fx.memo, &fx.blob, 0, 3000, &mut |k: &[u8], v: &[u8]| {
                fold(&mut got, k, v)
            })
            .unwrap();
        let mut want = 0u64;
        fx.blob
            .scan_at(0, first_bad, |k, v| fold(&mut want, k, v))
            .unwrap();
        assert_eq!((r.end, r.seen, got), (ScanEnd::Fallback, first_bad, want));
        assert!(z.damaged() && z.bad_pages() == 1);
        assert_eq!(
            fx.memo.take_stride().checks,
            bad_page as u64 + 1,
            "the pages before the bad one and the bad one's failed check"
        );
        // Point reads: the damaged page falls back, and then the intact first
        // page's ranks are out, by every method.
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let on_bad = &fx.recs[first_bad + 1].0;
        let r = z
            .read_all(&fx.memo, &fx.blob, on_bad, &mut |_: &[u8]| {
                panic!("a value")
            })
            .unwrap();
        assert_eq!(r, ZRead::Fallback);
        assert!(z.damaged() && z.bad_pages() == 1);
        assert_eq!(
            (fx.memo.take_point(), fx.memo.take_stride()),
            (
                PointCounts::default(),
                StrideCounts {
                    checks: 1,
                    reads: 0,
                    cursor: 0,
                    direct: 0,
                }
            ),
            "the failed check counted, and no read served"
        );
        let intact = &fx.recs[1].0;
        let none = &mut |_: &[u8]| panic!("a value from a file out of service");
        assert_eq!(
            z.read_all(&fx.memo, &fx.blob, intact, none).unwrap(),
            ZRead::Out
        );
        assert_eq!(z.count(&fx.memo, intact), ZRead::Out);
        assert_eq!(
            z.values_at(&fx.memo, &fx.blob, 1, none).unwrap(),
            ZRead::Out
        );
        let mut k = Vec::new();
        assert_eq!(z.key_at_into(&fx.memo, 1, &mut k), ZRead::Out);
        assert_eq!(z.seek(&fx.memo, intact), ZSeek::Out);
        assert_eq!(z.cut_in(&fx.memo, 0, 10, intact), ZSeek::Out);
        let mut hint = usize::MAX;
        assert_eq!(z.owner_span_from(intact, &mut hint), None);
        let walk = z
            .scan_at(&fx.memo, &fx.blob, 0, 10, &mut |_: &[u8], _: &[u8]| {
                panic!("a record from a file out of service")
            })
            .unwrap();
        assert_eq!((walk.end, walk.seen), (ScanEnd::Out, 0));
    }

    /// A cursor's counts that a stride position never moves: it decodes,
    /// reads in place, meets a run no page holds, views and asks ahead of
    /// nothing, and asks the pages of a walk only under the scan's prefetch.
    fn stride_cursor_quiet(c: &ZCursor<'_>, what: &str) {
        assert_eq!(
            (
                c.decoded,
                c.inplace,
                c.nopage,
                c.views,
                c.aheads,
                c.prefetches
            ),
            (0, 0, 0, 0, 0, 0),
            "{what}: a stride position decodes, opens and asks nothing"
        );
    }

    /// A cursor over a stride file stands where the block cursor over the
    /// same segment and the segment's own reader stand, at every placing
    /// and every move. `at` and `at_wanting` from ranks across the run and
    /// past it, stepped a rank at a time with the key and the values at
    /// each; `seek` of every key of the run, of keys beside them and of
    /// keys no rendering is, every one against the raw seek; `unplaced` and
    /// `skip_to` over the same keys in key order and in reverse, against
    /// the block cursor's moves and the file's cut, with the key the move
    /// stands on -- none past the run, where a stride position left at the
    /// run's last rank would lend that rank's key -- and the values under
    /// the position where the key is the run's, as a block's build reads
    /// the record an overlay key replaces; and the bulk walk, `walk_until`
    /// and `walk_run_ahead`, from ranks across the run, with `until` a key
    /// of the run (the walk stops on it, `Equal`), a key between two that
    /// the codec does not render (`Greater`, one rank on), the rendering
    /// past the run where the field holds it and none, keys of the run and
    /// between two below the position (nothing handed over, `Greater`: the
    /// key under the cursor is compared, never `until`'s own answer
    /// taken), at limits that stop before `until` and after it -- where the
    /// walk stops on `until`, the overlay walk's turn: the values under the
    /// cursor, the step, the walk on to the end. Over runs of one width a
    /// page holds many of and one a page holds one of, one key, a
    /// big-endian field, and a run that ends at its field's top, whose next
    /// number no key renders. The stride position decodes, views and asks
    /// ahead of nothing, the memo counts every value it handed over
    /// (`StrideCounts::cursor`), and no block cursor makes the stride
    /// position's key buffer.
    #[test]
    fn a_cursor_over_a_stride_file_stands_where_the_block_cursor_and_the_raw_form_do() {
        let mut rng = Rng(0x5c0a);
        let be: Vec<Rec> = (0..600u64)
            .map(|i| {
                let key = [b"be".as_slice(), &(0x0102_0304_0506_00f0 + i).to_be_bytes()].concat();
                (key, vec![rng.bytes(100)], false)
            })
            .collect();
        let cases = [
            ("w100", stride_recs(1200, 100, 31)),
            ("w1", stride_recs(700, 1, 32)),
            ("w3000", stride_recs(30, 3000, 33)),
            ("one", stride_recs(1, 100, 34)),
            ("be", be),
            ("top", stride_recs_from(999_400, 600, 100, 35)),
        ];
        for (name, recs) in cases {
            let fx = Fixture::new(&format!("stride-cursor-{name}"), recs, 1 << 16);
            // The block body of the segment, attached and kept mapped, then
            // the stride file built under the same name.
            let block = match fx.build() {
                Outcome::Built(_) => fx.attach().unwrap(),
                other => panic!("{name}: the block body: {other:?}"),
            };
            assert!(!block.is_stride(), "{name}");
            stride_built(&fx);
            let zfile = fx.attach().unwrap();
            let body = zfile.stride().expect("a stride body");
            let n = fx.recs.len();
            let _ = fx.memo.take_stride();
            let mut handed = 0u64;
            let raw_vals = |r: usize| -> Vec<Vec<u8>> {
                let mut v = Vec::new();
                fx.blob.values_at(r, |x| v.push(x.to_vec())).unwrap();
                v
            };
            let raw_seq = |from: usize, to: usize| -> Vec<(Vec<u8>, Vec<u8>)> {
                let mut v = Vec::new();
                for r in from..to.min(n) {
                    let k = fx.blob.key_at(r).unwrap().to_vec();
                    fx.blob
                        .values_at(r, |x| v.push((k.clone(), x.to_vec())))
                        .unwrap();
                }
                v
            };
            let mut starts: Vec<usize> =
                vec![0, 1, 2, 39, 40, 41, 63, 64, 65, n / 2, n - 1, n, n + 1];
            starts.retain(|&s| s <= n + 1);
            starts.sort_unstable();
            starts.dedup();
            // Placed at a rank, and stepped a rank at a time, with the key
            // and the values at each.
            for &start in &starts {
                for want in [None, Some(1usize), Some(5), Some(200)] {
                    let what = format!("{name}: at {start} wanting {want:?}");
                    let place = |z| match want {
                        None => ZCursor::at(&fx.memo, z, &fx.blob, start),
                        Some(w) => ZCursor::at_wanting(&fx.memo, z, &fx.blob, start, w),
                    };
                    let (mut c, mut b) = (place(&zfile), place(&block));
                    let steps = n.saturating_sub(start).min(want.unwrap_or(130) + 2);
                    for rank in start..=start + steps {
                        assert_eq!(c.rank(), rank, "{what}");
                        assert_eq!(c.key(), fx.blob.key_at(rank), "{what}: key at {rank}");
                        assert_eq!(c.key(), b.key(), "{what}: the block cursor's, at {rank}");
                        let (mut got, mut bgot) = (Vec::new(), Vec::new());
                        let m = c.values(&mut |v: &[u8]| got.push(v.to_vec())).unwrap();
                        b.values(&mut |v: &[u8]| bgot.push(v.to_vec())).unwrap();
                        assert_eq!(got, raw_vals(rank), "{what}: values at {rank}");
                        assert_eq!(got, bgot, "{what}: the block cursor's, at {rank}");
                        handed += m;
                        if c.key().is_none() {
                            break;
                        }
                        c.advance();
                        b.advance();
                    }
                    assert!(!c.fell_back() && !b.fell_back(), "{what}");
                    stride_cursor_quiet(&c, &what);
                }
            }
            // By key: the run's keys, keys beside them, and renderings
            // below and past the run.
            let codec = body.trailer().codec;
            let mut keys = probes(&fx.recs);
            let mut kb = [0u8; STRIDE_KEY_MAX];
            for x in [
                codec.base().checked_sub(1),
                codec.base().checked_add(n as u64),
                Some(codec.top()),
                Some(0),
            ]
            .into_iter()
            .flatten()
            .filter(|&x| x <= codec.top())
            {
                keys.push(codec.render(x, &mut kb).to_vec());
            }
            keys.sort();
            keys.dedup();
            for k in &keys {
                let c = ZCursor::seek(&fx.memo, &zfile, &fx.blob, k).unwrap();
                let b = ZCursor::seek(&fx.memo, &block, &fx.blob, k).unwrap();
                let rank = fx.blob.seek(k);
                assert_eq!(c.rank().min(n), rank, "{name}: seek of {k:?}");
                assert_eq!(c.key(), fx.blob.key_at(rank), "{name}: seek of {k:?}");
                assert_eq!(
                    (c.rank().min(n), c.key()),
                    (b.rank().min(n), b.key()),
                    "{name}: the block cursor's seek of {k:?}"
                );
                stride_cursor_quiet(&c, name);
            }
            // Moved to each key, in key order and back, from one cursor:
            // the cut the file's seek gives, and the block cursor's; the
            // values under the position where the key is the run's.
            for order in ["forwards", "backwards"] {
                let mut c = ZCursor::unplaced(&fx.memo, &zfile, &fx.blob);
                let mut b = ZCursor::unplaced(&fx.memo, &block, &fx.blob);
                let walk: Vec<&Vec<u8>> = if order == "forwards" {
                    keys.iter().collect()
                } else {
                    keys.iter().rev().collect()
                };
                for k in walk {
                    let at = c.skip_to(k).expect("the file answers");
                    let bat = b.skip_to(k).expect("the block file answers");
                    let cut = match zfile.cut_in(&fx.memo, 0, n, k) {
                        ZSeek::Served { rank, at, .. } => (rank, at),
                        other => panic!("{other:?}"),
                    };
                    assert_eq!((c.rank().min(n), at), cut, "{name}, {order}: {k:?}");
                    assert_eq!(
                        (c.rank().min(n), at),
                        (b.rank().min(n), bat),
                        "{name}, {order}: the block cursor's move to {k:?}"
                    );
                    // The key under the position, past the run as well,
                    // where the move leaves the end and no rank's key.
                    assert_eq!(
                        c.key(),
                        fx.blob.key_at(c.rank()),
                        "{name}, {order}: the key the move to {k:?} stands on"
                    );
                    assert_eq!(
                        c.key(),
                        b.key(),
                        "{name}, {order}: the block cursor's key after the move to {k:?}"
                    );
                    if at == KeyOrder::Equal {
                        let mut got = Vec::new();
                        handed += c.values(&mut |v: &[u8]| got.push(v.to_vec())).unwrap();
                        assert_eq!(got, raw_vals(c.rank()), "{name}, {order}: {k:?}");
                    }
                }
                assert!(!c.fell_back(), "{name}, {order}");
                stride_cursor_quiet(&c, name);
            }
            // The bulk walk.
            for &start in &starts {
                for ahead in [0usize, 1, 5, 70] {
                    let target = start + ahead;
                    let mut untils: Vec<(Option<Vec<u8>>, usize, KeyOrder)> =
                        vec![(None, n, KeyOrder::Greater)];
                    if let Some(k) = fx.blob.key_at(target) {
                        untils.push((Some(k.to_vec()), target, KeyOrder::Equal));
                        let between = [k, &[0u8]].concat();
                        assert_eq!(codec.decode(&between), None, "not a rendering");
                        untils.push((Some(between), target + 1, KeyOrder::Greater));
                    }
                    if codec.holds(n as u64 + 1) {
                        let past = codec.render(codec.base() + n as u64, &mut kb).to_vec();
                        untils.push((Some(past), n, KeyOrder::Greater));
                    }
                    // `until` below the position, a key of the run and one
                    // between two: nothing handed over, and the key the
                    // cursor stands on past it, as the block walk compares
                    // it, never `until`'s own answer.
                    let mut below = vec![0, start.saturating_sub(2), start.saturating_sub(1)];
                    below.retain(|&r| r < start && r < n);
                    below.dedup();
                    for r in below {
                        let k = fx.blob.key_at(r).unwrap();
                        untils.push((Some(k.to_vec()), r, KeyOrder::Greater));
                        untils.push((Some([k, &[0u8]].concat()), r + 1, KeyOrder::Greater));
                    }
                    for (until, stop, at_want) in untils {
                        for limit in [1usize, 7, 64, usize::MAX] {
                            for run_ahead in [None, Some(0usize), Some(100)] {
                                let what = format!(
                                    "{name}: from {start}, until {:?} (stop {stop}), limit {limit}, ahead {run_ahead:?}",
                                    until.as_deref().map(String::from_utf8_lossy)
                                );
                                let stop = stop.min(n);
                                let mut c = ZCursor::at(&fx.memo, &zfile, &fx.blob, start);
                                let mut b = ZCursor::at(&fx.memo, &block, &fx.blob, start);
                                let (mut got, mut bgot) = (Vec::new(), Vec::new());
                                let walk =
                                    |c: &mut ZCursor<'_>, out: &mut Vec<(Vec<u8>, Vec<u8>)>| {
                                        let mut f =
                                            |k: &[u8], v: &[u8]| out.push((k.to_vec(), v.to_vec()));
                                        let r = match run_ahead {
                                            None => c.walk_until(until.as_deref(), limit, &mut f),
                                            Some(a) => {
                                                c.walk_run_ahead(until.as_deref(), limit, a, &mut f)
                                            }
                                        };
                                        let r = r.unwrap();
                                        c.resume();
                                        r
                                    };
                                let (m, at) = walk(&mut c, &mut got);
                                let (bm, bat) = walk(&mut b, &mut bgot);
                                handed += m as u64;
                                let want_n = stop.saturating_sub(start).min(limit);
                                let want_at = if want_n >= limit {
                                    KeyOrder::Greater
                                } else {
                                    at_want
                                };
                                assert_eq!((m, at), (want_n, want_at), "{what}");
                                assert_eq!((m, at), (bm, bat), "{what}: the block cursor's");
                                assert_eq!(got, raw_seq(start, start + want_n), "{what}");
                                assert_eq!(got, bgot, "{what}: the block cursor's");
                                let at_rank = start + want_n;
                                assert_eq!(
                                    c.rank().min(n),
                                    at_rank.min(n),
                                    "{what}: the rank after"
                                );
                                assert_eq!(
                                    c.key(),
                                    fx.blob.key_at(at_rank),
                                    "{what}: the key under it"
                                );
                                assert_eq!(
                                    c.key(),
                                    b.key(),
                                    "{what}: the block cursor's key after"
                                );
                                if at == KeyOrder::Equal {
                                    let mut vals = Vec::new();
                                    handed +=
                                        c.values(&mut |v: &[u8]| vals.push(v.to_vec())).unwrap();
                                    assert_eq!(vals, raw_vals(at_rank), "{what}: under the cursor");
                                    c.advance();
                                    let mut rest = Vec::new();
                                    let (r, at2) = c
                                        .walk_until(None, usize::MAX, &mut |k: &[u8], v: &[u8]| {
                                            rest.push((k.to_vec(), v.to_vec()))
                                        })
                                        .unwrap();
                                    handed += r as u64;
                                    assert_eq!(
                                        (r, at2),
                                        (n - at_rank - 1, KeyOrder::Greater),
                                        "{what}: the rest"
                                    );
                                    assert_eq!(
                                        rest,
                                        raw_seq(at_rank + 1, n),
                                        "{what}: the rest's records"
                                    );
                                    assert_eq!(
                                        (c.key(), c.rank()),
                                        (None, n),
                                        "{what}: at the end"
                                    );
                                }
                                assert!(!c.fell_back(), "{what}");
                                stride_cursor_quiet(&c, &what);
                            }
                        }
                    }
                }
            }
            assert_eq!(
                fx.memo.take_stride().cursor,
                handed,
                "{name}: every value the stride position handed over, counted"
            );
            assert!(!zfile.damaged() && zfile.bad_pages() == 0, "{name}");
            // Every key and value above was the stride position's: nothing
            // went through the cursor's door to the raw form, every read of
            // which is a record read over a stride file. The block cursor
            // read every value through it, of a block body, which the count
            // leaves out (`ZFile::raw_reads`).
            assert_eq!(
                (zfile.raw_reads(), block.raw_reads()),
                (0, 0),
                "{name}: the raw form read through the cursor's door"
            );
        }
    }

    /// The stride position under the scan's prefetch: a walk that will
    /// cross pages past its first asks for them, once a cursor, as the
    /// block walk does (`ZFile::stride_ask_ahead`); one inside its first
    /// page and one with the prefetch off ask nothing; and a walk whose
    /// limit passes the run's end, unbounded or not, asks for the slots to
    /// the end and none past it.
    #[test]
    fn a_stride_cursors_walk_asks_ahead_only_under_the_scans_prefetch() {
        let fx = stride_fixture("stride-cursor-ahead", 3000, 100, 0x5a1);
        stride_built(&fx);
        let on = fx.attach_opts(false, true, true).unwrap();
        let off = fx.attach().unwrap();
        let mut sink = |_: &[u8], _: &[u8]| {};
        let mut c = ZCursor::at(&fx.memo, &on, &fx.blob, 0);
        c.walk_until(None, 300, &mut sink).unwrap();
        c.walk_until(None, 300, &mut sink).unwrap();
        assert_eq!(c.prefetches, 1, "one ask a cursor");
        let mut c = ZCursor::at(&fx.memo, &on, &fx.blob, 0);
        c.walk_until(None, 40, &mut sink).unwrap();
        assert_eq!(c.prefetches, 0, "ranks 0 to 39 lie in the first page");
        let mut c = ZCursor::at(&fx.memo, &off, &fx.blob, 0);
        c.walk_until(None, 300, &mut sink).unwrap();
        assert_eq!(c.prefetches, 0, "the option off");
        // A walk near the run's end whose limit passes it, unbounded and
        // not: the ask is of the slots to the run's end and no further,
        // whatever the limit -- from the last rank whose slot starts before
        // the last value page, that page alone, and from the first whose
        // slot starts on it, nothing. A want taken as the limit asks pages
        // past the run, and an unbounded one overflows the span's byte
        // arithmetic.
        let n = fx.recs.len();
        let body = on.stride().unwrap();
        let w = body.trailer().w as usize;
        let value_pages = (n * w).div_ceil(ZPAGE);
        let on_last = ((value_pages - 1) * ZPAGE).div_ceil(w);
        assert!(on_last < n, "a run whose last value page holds slots");
        assert_eq!(
            on.stride_ask_ahead(body, (on_last - 1) as u64, (n - on_last + 1) as u64),
            Some(((value_pages - 1) as u32, 1)),
            "the run's last value page"
        );
        assert_eq!(
            on.stride_ask_ahead(body, on_last as u64, (n - on_last) as u64),
            None
        );
        for (from, asks) in [(on_last, 0u64), (on_last - 1, 1)] {
            for limit in [n, usize::MAX] {
                let mut c = ZCursor::at(&fx.memo, &on, &fx.blob, from);
                let walked = c.walk_until(None, limit, &mut sink).unwrap();
                assert_eq!(walked, (n - from, KeyOrder::Greater), "from {from}");
                assert_eq!(
                    c.prefetches, asks,
                    "from {from} for {limit}: the ask stays inside the value pages"
                );
            }
        }
    }

    /// A cursor over a stride file honours the file's being out of service
    /// before anything else at every step, as the block cursor does at its
    /// every placing: a walk across a page that fails its check stops the
    /// stride position at the first rank the page holds, every rank before
    /// it handed over from the file and every rank from it on the raw
    /// form's, with the file out of service; a cursor standing on an intact
    /// page when another read takes the file out answers its values from
    /// the raw form, falls at its next step and walks the rest raw, with
    /// nothing more counted as the stride position's; and a cursor placed
    /// after is fallen from the start, a seek is refused and a move to a
    /// key answers none. Every key and value right throughout.
    #[test]
    fn a_stride_cursor_answers_the_raw_form_once_its_file_is_out_of_service() {
        let fx = stride_fixture("stride-cursor-out", 3000, 100, 0x0c);
        stride_built(&fx);
        let n = fx.recs.len();
        let bad_page = 3usize;
        {
            use std::os::unix::fs::FileExt;
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fx.out)
                .unwrap();
            let at = (bad_page * ZPAGE + 7) as u64;
            let mut b = [0u8; 1];
            f.read_exact_at(&mut b, at).unwrap();
            f.write_all_at(&[b[0] ^ 1], at).unwrap();
        }
        let first_bad = bad_page * ZPAGE / 100;
        let raw_seq = |from: usize, to: usize| -> Vec<(Vec<u8>, Vec<u8>)> {
            let mut v = Vec::new();
            for r in from..to.min(n) {
                let k = fx.blob.key_at(r).unwrap().to_vec();
                fx.blob
                    .values_at(r, |x| v.push((k.clone(), x.to_vec())))
                    .unwrap();
            }
            v
        };
        // A walk across the damaged page, from a file attached fresh.
        let _ = fx.memo.take_stride();
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let mut c = ZCursor::at(&fx.memo, &z, &fx.blob, 0);
        let mut got = Vec::new();
        let (m, at) = c
            .walk_until(None, usize::MAX, &mut |k: &[u8], v: &[u8]| {
                got.push((k.to_vec(), v.to_vec()))
            })
            .unwrap();
        assert_eq!((m, at), (n, KeyOrder::Greater));
        assert_eq!(got, raw_seq(0, n), "every key and value, across the fall");
        assert!(c.fell_back() && z.damaged() && z.bad_pages() == 1);
        assert_eq!(
            fx.memo.take_stride().cursor,
            first_bad as u64,
            "the ranks before the bad page from the file, none from it on"
        );
        // The control for the count `Reader::raw_touches` sums: the ranks
        // from the bad page on were read through the cursor's door, each
        // at least once, and a build without debug assertions counts none.
        assert!(
            if cfg!(debug_assertions) {
                z.raw_reads() >= (n - first_bad) as u64
            } else {
                z.raw_reads() == 0
            },
            "the fall's reads of the raw form counted: {}",
            z.raw_reads()
        );
        // A cursor standing on the damaged page: its value read fails the
        // page's check and answers the raw form's value, never none, with
        // nothing of the slot handed over or counted, and the file out of
        // service.
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let c = ZCursor::at(&fx.memo, &z, &fx.blob, first_bad);
        assert!(!c.fell_back() && !z.damaged());
        let mut v = Vec::new();
        assert_eq!(c.values(&mut |x: &[u8]| v.push(x.to_vec())).unwrap(), 1);
        assert_eq!(
            v,
            vec![fx.recs[first_bad].1[0].clone()],
            "the raw form's value"
        );
        assert!(z.damaged() && z.bad_pages() == 1);
        assert_eq!(fx.memo.take_stride().cursor, 0, "nothing from the slot");
        assert_eq!(
            z.raw_reads(),
            u64::from(cfg!(debug_assertions)),
            "the one value read through the door, counted"
        );
        // A cursor standing on an intact page when a point read takes the
        // file out of service.
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let mut c = ZCursor::at(&fx.memo, &z, &fx.blob, 1);
        let mut v = Vec::new();
        c.values(&mut |x: &[u8]| v.push(x.to_vec())).unwrap();
        assert_eq!(v, vec![fx.recs[1].1[0].clone()]);
        assert_eq!(fx.memo.take_stride().cursor, 1, "served before the failure");
        let r = z
            .read_all(
                &fx.memo,
                &fx.blob,
                &fx.recs[first_bad + 1].0,
                &mut |_: &[u8]| panic!("a value"),
            )
            .unwrap();
        assert_eq!(r, ZRead::Fallback);
        assert!(z.damaged());
        let _ = fx.memo.take_stride();
        let mut v = Vec::new();
        assert_eq!(c.values(&mut |x: &[u8]| v.push(x.to_vec())).unwrap(), 1);
        assert_eq!(v, vec![fx.recs[1].1[0].clone()], "the raw form's value");
        assert!(!c.fell_back(), "a value read moves nothing");
        c.advance();
        assert!(c.fell_back(), "fallen at its next step");
        assert_eq!(c.key(), fx.blob.key_at(2));
        let mut rest = Vec::new();
        let (m, _) = c
            .walk_until(None, 100, &mut |k: &[u8], v: &[u8]| {
                rest.push((k.to_vec(), v.to_vec()))
            })
            .unwrap();
        assert_eq!((m, rest), (100, raw_seq(2, 102)));
        assert_eq!(
            fx.memo.take_stride().cursor,
            0,
            "nothing from a file out of service"
        );
        // A walk begun on a cursor placed before the failure falls before
        // its first record.
        let z = fx.attach_stride(StrideAttach::Check).unwrap();
        let mut c = ZCursor::at(&fx.memo, &z, &fx.blob, 10);
        let _ = z
            .read_all(
                &fx.memo,
                &fx.blob,
                &fx.recs[first_bad + 1].0,
                &mut |_: &[u8]| panic!("a value"),
            )
            .unwrap();
        assert!(z.damaged());
        let _ = fx.memo.take_stride();
        let mut got = Vec::new();
        let (m, at) = c
            .walk_until(Some(&fx.recs[20].0), 50, &mut |k: &[u8], v: &[u8]| {
                got.push((k.to_vec(), v.to_vec()))
            })
            .unwrap();
        assert_eq!((m, at), (10, KeyOrder::Equal));
        assert_eq!(got, raw_seq(10, 20));
        assert!(c.fell_back());
        assert_eq!(fx.memo.take_stride().cursor, 0, "nothing from the file");
        // Placed after: fallen from the start, a seek refused, a move none.
        let c = ZCursor::at(&fx.memo, &z, &fx.blob, 5);
        assert!(c.fell_back());
        assert_eq!(c.key(), fx.blob.key_at(5));
        assert!(matches!(
            ZCursor::seek(&fx.memo, &z, &fx.blob, &fx.recs[5].0),
            Err(ZSeek::Out)
        ));
        let mut c = ZCursor::unplaced(&fx.memo, &z, &fx.blob);
        assert_eq!(c.skip_to(&fx.recs[5].0), None);
        assert!(c.fell_back());
    }

    /// Every byte of a stride file's value pages, its row's page and its
    /// trailer flipped, one at a time, under a fresh attach: the attach
    /// refuses the file, as damage, where the flip is in a page it samples
    /// or in the row or the trailer; elsewhere every read of the page's
    /// ranks answers the raw form's value or falls back, never another
    /// value and never absence, the slot that straddles the page's end among
    /// them, and a fallback takes the file out of service.
    #[test]
    fn every_byte_of_a_stride_file_flipped_is_never_an_answer() {
        let fx = stride_fixture("stride-flips", 820, 100, 0xf1);
        stride_built(&fx);
        let good = std::fs::read(&fx.out).unwrap();
        let body = StrideBody::from_bytes(&good).unwrap();
        let v = body.pages();
        assert!(v > ZSAMPLE_PAGES, "{v} pages, some of them unsampled");
        // The pages the attach reads: each sampled page, and the page beside
        // it that the slot it checks there spans (`StrideBody::rank_on_page`).
        let w = 100usize;
        let mut sampled: Vec<usize> = Vec::new();
        for p in sample_pages(v) {
            let r = body.rank_on_page(p as u32) as usize;
            sampled.extend([r * w / ZPAGE, (r * w + w - 1) / ZPAGE]);
        }
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fx.out)
            .unwrap();
        use std::os::unix::fs::FileExt;
        let (mut refused, mut fell, mut straddled) = (0usize, 0usize, 0usize);
        for (at, &byte) in good.iter().enumerate() {
            f.write_all_at(&[byte ^ 0x5a], at as u64).unwrap();
            let p = at / ZPAGE;
            match fx.attach() {
                Err(Reject::Damage(_)) => {
                    assert!(
                        p >= v || sampled.contains(&p),
                        "byte {at}: refused, page {p}"
                    );
                    refused += 1;
                }
                Err(e) => panic!("byte {at}: {e:?}"),
                Ok(z) => {
                    assert!(
                        p < v && !sampled.contains(&p),
                        "byte {at}: page {p} attached"
                    );
                    // The ranks whose slots touch the page.
                    let lo = p * ZPAGE / w;
                    let hi = (((p + 1) * ZPAGE).div_ceil(w)).min(fx.recs.len());
                    let mut failed = false;
                    for rank in lo..hi {
                        let mut got = Vec::new();
                        let r = z
                            .values_at(&fx.memo, &fx.blob, rank, &mut |x: &[u8]| {
                                got.push(x.to_vec())
                            })
                            .unwrap();
                        match r {
                            ZRead::Served { n: 1, .. } => {
                                assert_eq!(got, fx.recs[rank].1, "byte {at}: rank {rank}")
                            }
                            ZRead::Fallback | ZRead::Out => {
                                assert!(got.is_empty());
                                failed = true;
                                if rank * w / ZPAGE != (rank * w + w - 1) / ZPAGE {
                                    straddled += 1;
                                }
                            }
                            other => panic!("byte {at}: rank {rank}: {other:?}"),
                        }
                    }
                    // A flip in the padding past the last value is in no
                    // slot: no read sees it.
                    let in_slot = at < fx.recs.len() * w;
                    assert_eq!(
                        failed, in_slot,
                        "byte {at}: a fallback where the flip is read"
                    );
                    assert_eq!(z.damaged(), in_slot, "byte {at}");
                    fell += usize::from(in_slot);
                }
            }
            f.write_all_at(&[byte], at as u64).unwrap();
        }
        assert!(
            refused > 0 && fell > 0 && straddled > 0,
            "{refused} {fell} {straddled}"
        );
        assert!(fx.attach().is_ok(), "the file restored attaches");
    }

    /// The attach's checks behind the checksums, each held to firing by a
    /// file forged with good CRCs, and the identity's: a sampled page's slot
    /// not the segment's value is an identity refusal; a first number off by
    /// one, its row and trailer whole, is one at the first key; a page no
    /// attach samples, forged so, is served as it reads, which is the
    /// sample's limit; a stride file of another segment -- another name in
    /// its trailer, or the same name over other records, as a segment id
    /// handed out again after a crash -- is refused; and bound under good
    /// checksums to a segment that agrees with it everywhere but in one
    /// place, the file is refused by the one check for that place: a key at
    /// a sampled rank, a second value in every record, and a codec that
    /// renders the first key right and the last wrong over a file whose
    /// sample is its first rank alone.
    #[test]
    fn a_stride_file_forged_under_good_checksums_is_refused_where_the_attach_looks() {
        use crate::db::zpages::KeyKind;
        let fx = stride_fixture("stride-forge", 3000, 100, 0xf0);
        stride_built(&fx);
        let good = std::fs::read(&fx.out).unwrap();
        let v = StrideBody::from_bytes(&good).unwrap().pages();
        let sampled = sample_pages(v);
        let identity = |what: &str| match fx.attach() {
            Err(Reject::Identity(why)) => why,
            Err(other) => panic!("{what}: {other:?}"),
            Ok(_) => panic!("{what}: attached"),
        };
        // A sampled page's slot forged.
        let p = sampled[3];
        let b = StrideBody::from_bytes(&good).unwrap();
        let rank = b.rank_on_page(p as u32) as usize;
        forge_stride(&fx.out, Some(p as u32), |bytes, _, _| {
            bytes[rank * 100 + 1] ^= 1
        });
        let why = identity("a sampled page forged");
        assert!(why.contains("value at its rank"), "{why}");
        std::fs::write(&fx.out, &good).unwrap();
        // The first number off by one.
        forge_stride(&fx.out, None, |_, t, _| {
            let c = t.codec;
            t.codec = KeyCodec::new(c.kind(), c.prefix(), c.d(), c.base() + 1).unwrap();
        });
        let why = identity("the first number");
        assert!(why.contains("first key"), "{why}");
        std::fs::write(&fx.out, &good).unwrap();
        // A page no attach samples, forged: served as it reads.
        let p = (1..v).find(|p| !sampled.contains(p)).unwrap();
        let rank = p * ZPAGE / 100 + 1;
        assert_eq!(rank * 100 / ZPAGE, p, "a slot inside the page");
        forge_stride(&fx.out, Some(p as u32), |bytes, _, _| {
            bytes[rank * 100 + 2] ^= 1
        });
        let z = fx.attach().expect("an unsampled forged page attaches");
        let mut got = Vec::new();
        let r = z
            .values_at(&fx.memo, &fx.blob, rank, &mut |x: &[u8]| {
                got.push(x.to_vec())
            })
            .unwrap();
        assert_eq!(
            r,
            ZRead::Served {
                n: 1,
                decoded: false
            }
        );
        assert_ne!(got, fx.recs[rank].1, "served as forged: the sample's limit");
        std::fs::write(&fx.out, &good).unwrap();
        assert!(fx.attach().is_ok());
        // Another segment's file under this name: another id in its trailer,
        // and this id over other records.
        let _ = std::fs::remove_file(&fx.out);
        build(&fx.seg, &fx.out, ID + 1, END, &stride_plan(StrideAsk::Only)).unwrap();
        let why = identity("another name");
        assert!(why.contains("another segment"), "{why}");
        let other = stride_fixture("stride-forge-other", 2000, 100, 0xf2);
        stride_built(&other);
        std::fs::copy(&other.out, &fx.out).unwrap();
        let why = identity("this name over other records");
        assert!(why.contains("differs"), "{why}");
        std::fs::write(&fx.out, &good).unwrap();
        assert!(fx.attach().is_ok());
        // A segment of the same count, ends and values, whose key at a
        // sampled rank is not the rendering of its number -- a key between
        // its neighbours that the build's walk would decline -- and this
        // file under its name, its trailer naming that segment and its row
        // bound to it: every checksum good, the ends the segment's, and the
        // sampled slot's value the segment's, so its key is what refuses.
        let p = sampled[3];
        let rank = b.rank_on_page(p as u32) as usize;
        let mut recs = fx.recs.clone();
        recs[rank].0 = [recs[rank - 1].0.as_slice(), b"x"].concat();
        let odd = Fixture::new("stride-forge-key", recs, 1 << 16);
        std::fs::write(&odd.out, rebind_stride(&good, &odd)).unwrap();
        let why = identity_of(&odd, "a sampled slot's key");
        assert!(why.contains("key at its rank"), "{why}");
        // A segment of the same keys whose every record holds two values,
        // the first the one this file holds, and this file bound to it: the
        // ends are its keys and each sampled slot its first value, so what
        // refuses is the record's count -- a file of one value a key would
        // answer one where the raw form answers two.
        let recs: Vec<Rec> = fx
            .recs
            .iter()
            .map(|(k, vs, t)| (k.clone(), vec![vs[0].clone(), b"second".to_vec()], *t))
            .collect();
        let two = Fixture::new("stride-forge-two", recs, 1 << 16);
        std::fs::write(&two.out, rebind_stride(&good, &two)).unwrap();
        let why = identity_of(&two, "two values a record");
        assert!(why.contains("value at its rank"), "{why}");
        // A file of one value page, so that its sample is rank 0 alone,
        // under a codec of the other kind that renders the first key as the
        // segment has it and every key after it otherwise: the prefix
        // `k0000` and two big-endian bytes from 0x3030, "00". Only the last
        // key's compare refuses it; attached, a read of the run's eleventh
        // key, which the codec renders `k00000:`, would answer nothing where
        // the raw form answers its value.
        let one = stride_fixture("stride-forge-last", 40, 100, 0xf3);
        stride_built(&one);
        let bytes = std::fs::read(&one.out).unwrap();
        assert_eq!(StrideBody::from_bytes(&bytes).unwrap().pages(), 1);
        forge_stride(&one.out, None, |_, t, _| {
            let be = KeyCodec::new(KeyKind::BigEndian, b"k0000", 2, 0x3030).unwrap();
            let mut r = [0u8; STRIDE_KEY_MAX];
            assert_eq!(be.render(be.base(), &mut r), one.recs[0].0.as_slice());
            assert_ne!(be.render(be.base() + 39, &mut r), one.recs[39].0.as_slice());
            t.codec = be;
        });
        let why = identity_of(&one, "the last key");
        assert!(why.contains("last key"), "{why}");
    }

    /// The reason `fx`'s attach refuses its file as another segment's or
    /// another engine's, or a panic naming `what` where it refuses it
    /// otherwise or attaches it.
    fn identity_of(fx: &Fixture, what: &str) -> String {
        match fx.attach() {
            Err(Reject::Identity(why)) => why,
            Err(other) => panic!("{what}: {other:?}"),
            Ok(_) => panic!("{what}: attached"),
        }
    }

    /// The stride file whose bytes are `good` bound to `to`'s segment
    /// under every checksum: its trailer naming that segment, each value
    /// page's word of the row taken again under the hash that binds, the
    /// row's CRC taken again and the trailer sealed again -- a file whose
    /// checksums are all good for a segment it was not built from, so that
    /// only the attach's checks of the segment's records against the file
    /// can refuse it.
    fn rebind_stride(good: &[u8], to: &Fixture) -> Vec<u8> {
        let facts = to.blob.z_source_facts().unwrap();
        let mut bytes = good.to_vec();
        let len = bytes.len();
        let mut t = StrideTrailer::decode(&bytes[len - ZTRAILER..]).unwrap();
        let lay = t.layout().unwrap();
        t.seg = seg_ident_of(&facts, ID, END);
        let bind = t.bind32();
        let res = lay.res_off as usize;
        for q in 0..lay.v_pages as usize {
            let word = stride_page_crc(&bytes[q * ZPAGE..(q + 1) * ZPAGE], bind, q as u32);
            bytes[res + 4 * q..res + 4 * q + 4].copy_from_slice(&word.to_le_bytes());
        }
        t.res_crc = stride_row_crc(&bytes[res..res + lay.res_len as usize], bind);
        bytes[len - ZTRAILER..].copy_from_slice(&t.encode().unwrap());
        bytes
    }

    /// A stride file's seek, through the file and through a cursor placed by
    /// it, is the binary search of its keys under the engine's key order
    /// (`key_cmp`), for keys of every shape about runs of either kind: the
    /// run's own, its neighbours, keys a byte shorter and longer, a
    /// non-digit at each place, the empty key, keys of 0xff and random
    /// bytes. The codec's arithmetic alone is the grid in `zpages`
    /// (`lower_bound_is_the_binary_search_of_the_rendered_keys`); this holds
    /// the file's body over a built and attached file to it -- the run's
    /// count, the codec the build detected from the keys, which is not the
    /// one the keys were rendered by where a prefix runs into the field, and
    /// the dispatch -- none of which the codec's own grid reaches.
    #[test]
    fn a_stride_files_seek_is_the_binary_search_of_its_keys() {
        use crate::db::key_cmp;
        use crate::db::zpages::KeyKind;
        let mut rng = Rng(0x5e);
        let runs: Vec<(&str, KeyCodec, u64)> = vec![
            (
                "k6",
                KeyCodec::new(KeyKind::Decimal, b"k", 6, 0).unwrap(),
                3000,
            ),
            (
                "k5",
                KeyCodec::new(KeyKind::Decimal, b"k0", 5, 997).unwrap(),
                41,
            ),
            (
                "d3",
                KeyCodec::new(KeyKind::Decimal, b"", 3, 950).unwrap(),
                50,
            ),
            (
                "be2",
                KeyCodec::new(KeyKind::BigEndian, b"be", 2, 0xfe00).unwrap(),
                512,
            ),
            (
                "be1",
                KeyCodec::new(KeyKind::BigEndian, b"", 1, 7).unwrap(),
                9,
            ),
        ];
        for (name, codec, n) in runs {
            let mut buf = [0u8; STRIDE_KEY_MAX];
            let keys: Vec<Vec<u8>> = (0..n)
                .map(|r| codec.render(codec.base() + r, &mut buf).to_vec())
                .collect();
            let recs: Vec<Rec> = keys
                .iter()
                .map(|k| (k.clone(), vec![rng.bytes(8)], false))
                .collect();
            let fx = Fixture::new(&format!("stride-seek-{name}"), recs, 1 << 16);
            stride_built(&fx);
            let zfile = fx.attach().unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert!(zfile.is_stride(), "{name}");
            let mut probes: Vec<Vec<u8>> = vec![Vec::new(), vec![0xff; 12], vec![0], b"k".to_vec()];
            for k in keys
                .iter()
                .step_by(((n / 40) as usize).max(1))
                .chain(keys.last())
            {
                probes.extend(around_key(k));
                for i in 0..k.len() {
                    for b in [b'/', b':', 0, 0xff] {
                        let mut x = k.clone();
                        x[i] = b;
                        probes.push(x);
                    }
                }
            }
            for _ in 0..200 {
                let len = rng.below(10);
                probes.push(rng.bytes(len));
            }
            for p in &probes {
                let want = keys.partition_point(|k| key_cmp(k, p) == KeyOrder::Less);
                let at = if keys.get(want).is_some_and(|k| k == p) {
                    KeyOrder::Equal
                } else {
                    KeyOrder::Greater
                };
                assert_eq!(
                    zfile.seek(&fx.memo, p),
                    ZSeek::Served {
                        rank: want,
                        at,
                        decoded: false
                    },
                    "{name}: the file's seek of {p:?}"
                );
                let c = ZCursor::seek(&fx.memo, &zfile, &fx.blob, p).expect("the file answers");
                assert_eq!(c.rank(), want, "{name}: a cursor's seek of {p:?}");
                assert_eq!(
                    c.key(),
                    keys.get(want).map(Vec::as_slice),
                    "{name}: the key under it"
                );
            }
        }
    }

    /// Keys around `k`: it, a byte either side of its last, a proper prefix
    /// of it, and two keys that extend it.
    fn around_key(k: &[u8]) -> Vec<Vec<u8>> {
        let mut v = vec![
            k.to_vec(),
            [k, &[0u8][..]].concat(),
            [k, &[0xffu8][..]].concat(),
        ];
        if let Some((&last, head)) = k.split_last() {
            v.push(head.to_vec());
            v.push([head, &[last.wrapping_sub(1)][..]].concat());
            v.push([head, &[last.wrapping_add(1)][..]].concat());
        }
        v
    }
}
