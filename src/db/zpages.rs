//! The z form's layout: a derived form file that holds a partition's records,
//! verbatim, in groups that each compress into one 4 KiB page.
//!
//! The module is the one definition of it -- the page, the trailer, the fence
//! entries, the group that fills a page and the walk over a decoded group --
//! so that the builder, the reader and the tools that price the form all use
//! the same arithmetic, not a copy of it. A second definition of this layout
//! is a second reader, and a second reader fails by answering differently,
//! not by crashing.
//!
//! # The file
//!
//! All integers are little-endian. The length is a multiple of 4,096.
//!
//! | region | offset | contents |
//! |---|---|---|
//! | A, pages | `4096 * p` | `P` pages ([`ZPAGE`] bytes each) |
//! | B, resident part | `res_off = 4096 * P` | `res_len` bytes, zero-padded to a multiple of 4,096: the fence entries, then the filter |
//! | C, trailer | `len - 4096` | [`Trailer`] |
//!
//! The resident part is at the tail and the trailer is the last page, so a
//! builder streams pages from offset zero without knowing how many there
//! will be and never patches an earlier byte: a patch dirties its whole
//! folio again, and `/proc/self/io` counts the folio again in full.
//!
//! Region B is, in order: `first_rank[E]` as u32; `page_no[E]` as u32
//! ([`ZNOPAGE`] for records the form does not hold); `key_off[E + 1]` as
//! u32, the first zero and the last the length of the key blob; the key blob
//! (entry `i`'s key is `blob[key_off[i]..key_off[i + 1]]`); zeros to a
//! multiple of 8; and the filter, whole 64-byte blocks of eight u64 words.
//! Entry `i` covers the ranks `[first_rank[i], first_rank[i + 1])`, the last
//! up to `nkeys`. An entry with a page names the page that holds exactly
//! those records; an entry with [`ZNOPAGE`] names records the form does not
//! hold, which are read from the raw segment.
//!
//! # The page
//!
//! A page is a container of one to [`ZMAX_GROUPS`] groups, each compressed
//! on its own, so that a point read decodes the group its record is in and
//! not the page: a directory after the header says where each group's
//! payload lies, how many records it holds and, for every group but the
//! first, the key of its first record, by which a read picks the group
//! without decoding one.
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | `crc`: CRC-32C over bytes `[4, 4096)`, padding included, xor the trailer's `bind32` |
//! | 4 | 4 | `tag`, `"ZPG2"`, never zero: a zeroed page cannot pass |
//! | 8 | 4 | `first_rank` |
//! | 12 | 2 | `count`: records in the page, 1 to [`ZMAX_COUNT`] |
//! | 14 | 2 | `stored`: bit `j` set where group `j`'s payload is its images verbatim |
//! | 16 | 4 | `ulen`: bytes of the decoded groups, summed |
//! | 20 | 2 | `used`: bytes after the header the page uses, at most [`ZCAP`] |
//! | 22 | 1 | `groups`, 1 to [`ZMAX_GROUPS`] |
//! | 23 | 1 | zero |
//! | 24 | 8 × `groups` | the directory: per group `first` (u16, the index in the page of its first record), `coff` (u16, its payload's offset from the page's start, a multiple of 4), `clen` (u16, payload bytes), `ulen4` (u16, decoded bytes over four) |
//! | after | varies | the keys of groups 1..: per group `klen` (u16) and the key |
//! | `coff` | `clen` | each payload at its `coff`, the first at the keys' end rounded up to a multiple of 4 and each after at the end of the one before rounded the same, zeros between: one LZ4 block, no size prefix and no frame, or the group |
//! | 24 + `used` | the rest | zeros |
//!
//! The groups' firsts ascend from zero and their keys ascend; the directory
//! holds no key for group 0, whose first key the fence carries. A group's
//! records are 4 to [`ZMAX_ULEN`] decoded bytes; the sum over the page is
//! `ulen`. The directory carries each `clen` because the decoder stops only
//! when its input is exhausted after a literal run: handed the padding
//! after a payload it parses it as further sequences. The CRC is xored with
//! `bind32`, a hash of the segment's identity, so a page copied in from
//! another form file fails its own check without a second field to read.
//! It is checked before anything is decoded, because LZ4 decodes many
//! corrupt inputs into plausible bytes.
//!
//! A group is the concatenation of the bytes `parse_record` consumes for each
//! of its records, in rank order, each image a multiple of four bytes. One
//! decoder then serves every read, count and scan, and no second definition
//! of the record exists to drift. The groups are cut at
//! [`FillScratch::group_bytes`] decoded bytes, four to a page at the suite's
//! shape, which costs the form six percent of its bytes for a point read's
//! decode a quarter of the page's (`bench/examples/zdecode.rs`); a target of
//! zero is the page as one group.
//!
//! # The stride form
//!
//! A form file's other body, for a partition whose keys are an exact run of
//! numbers rendered at one width -- every number from the first key's to
//! the last's present, each key the prefix the two share and a field of
//! decimal digits or big-endian bytes ([`KeyCodec`]) -- and whose every
//! record is one inline fixed value of one width `W`. Rank `r`'s key is the
//! rendering of `base + r` and its value lies at `r * W`, so the file holds
//! the values alone and a point read is a key's parse and one or two pages,
//! with nothing searched.
//!
//! | region | offset | contents |
//! |---|---|---|
//! | V, values | 0 | rank `r`'s value at `r * W`, unpadded; zeros from `n * W` to the page's end |
//! | R, CRC row | `res_off = 4096 * V` | a u32 a value page, page `p`'s [`stride_page_crc`], zeros to the page's end |
//! | trailer | `len - 4096` | [`StrideTrailer`], magic [`STRIDE_MAGIC`] |
//!
//! [`stride_layout`] is the arithmetic of the three. A slot is at most a
//! page and spans at most two; the slots are not padded to a divisor of
//! the page, which for a hundred-byte value is 128 bytes a slot and the
//! form's bytes more than a quarter again, so a read whose slot straddles
//! checks two pages. The trailer is the block form's length and head --
//! the magic, the version, the page size and the CRC at one set of places,
//! written and checked by one pair of functions -- with another magic, so
//! either kind is told from the other at the last page's first eight
//! bytes, and a reader of one refuses the other there.
//!
//! # Why it is a child of `db`
//!
//! The filter stored in region B is `BlockedBloom`'s words and the walk over
//! a group compares keys as the engine's other searches do (`key_cmp`); both
//! are private to `db`, and a child module reaches them without widening any
//! visibility. The items here are public for the reason `flatindex`'s are:
//! the tools in `bench/` price the layout they link, and a copy of it in an
//! example is the second definition. The module is hidden from the crate's
//! documentation until a reader of the form file exists, since a layout nothing
//! reads is not yet an interface anyone is promised.

use super::key_cmp;
use crate::block::crc32;
use crate::flatindex::{parse_record, rd_u32, record_key, record_key_next};
use std::cmp::Ordering;
use std::fmt;

/// The page: one fault, one page-cache page, and the unit of residency.
pub const ZPAGE: usize = 4096;
/// The page header's length.
pub const ZHDR: usize = 24;
/// Payload bytes a page holds.
pub const ZCAP: usize = ZPAGE - ZHDR;
/// The most records in a page: `count` is a u16, and a group's record
/// index in the page is one too.
pub const ZMAX_COUNT: usize = 256;
/// The most bytes in a decoded group, and the size of the buffer one decodes
/// into. LZ4's window is 64 KiB, so a longer group would not compress better
/// for its length.
pub const ZMAX_ULEN: usize = 64 * 1024;
/// The most groups in a page: the header's `stored` mask is a u16.
pub const ZMAX_GROUPS: usize = 16;
/// A directory entry's length.
pub const ZDIR_ENTRY: usize = 8;
/// The decoded bytes a group is cut at by default (`FillScratch::group_bytes`):
/// what a point read decodes for one record. Four groups a page at the
/// suite's shape, 6% of the form's bytes for a decode a quarter the whole
/// page's; zero cuts nothing and a page is one group.
pub const ZGROUP_BYTES: usize = 2048;
/// A fence entry's page number for records the form does not hold.
pub const ZNOPAGE: u32 = u32::MAX;
/// A page's tag, `"ZPG2"` as the bytes on disk: the digit moves whenever an
/// older reader would misread a page rather than refuse it. It moved when
/// the page became a container of groups: a `ZPG1` reader takes the
/// directory for the payload.
pub const ZTAG: u32 = u32::from_le_bytes(*b"ZPG2");
/// The trailer's magic: the digit moves whenever an older reader would
/// misread the file rather than refuse it; it moves with the page's tag,
/// so a file of the other layout is refused at its trailer and rebuilt.
pub const ZMAGIC: [u8; 8] = *b"SUPDZPG2";
/// The trailer's version: what an older reader refuses without misreading.
/// It moved to 2 with the record coding ([`ZCODING_RECORD`]): the page's
/// layout is the same and a `ZPG2` reader refuses a coded group at its
/// decoded length, whose high bit says the coding, but a refusal at a page
/// is read as damage and a refusal at the trailer as a file to rebuild,
/// which a derived file is.
pub const ZVERSION: u32 = 2;
/// The trailer's length, a page.
pub const ZTRAILER: usize = ZPAGE;
/// The trailer flag that says a filter is stored in the resident part.
pub const ZT_FILTER: u32 = 1;
/// How many attempts to compress a group `fill_group` may make to find one
/// that fits a page, and then to grow it.
pub const ZFILL_TURNS: u32 = 12;
/// How many times `fill_group` grows a group that fit with room to spare,
/// by default: the knob that trades compress calls a page for fill.
pub const ZGROW_ROUNDS: u32 = 1;

// The page header's fields.
const P_CRC: usize = 0;
const P_TAG: usize = 4;
const P_FIRST: usize = 8;
const P_COUNT: usize = 12;
const P_STORED: usize = 14;
const P_ULEN: usize = 16;
const P_USED: usize = 20;
const P_GROUPS: usize = 22;
const P_ZERO: usize = 23;
// A directory entry's fields, from the entry's start.
const D_FIRST: usize = 0;
const D_COFF: usize = 2;
const D_CLEN: usize = 4;
const D_ULEN4: usize = 6;

/// What a check of the layout found. One variant for each check the layout
/// makes, so a test can say which one a damaged byte reaches and a caller
/// can count them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZError {
    /// A page, a trailer or a region is not the length it must be.
    Length,
    /// The trailer does not begin with the magic.
    Magic,
    /// The trailer's version is not one this reader reads.
    Version,
    /// The trailer's page size is not [`ZPAGE`].
    PageSize,
    /// The trailer's CRC does not match its bytes.
    TrailerCrc,
    /// The trailer's fields contradict each other or this layout.
    Trailer(&'static str),
    /// The resident part's CRC does not match its bytes.
    ResidentCrc,
    /// The resident part does not parse as the fence entries and the filter
    /// the trailer says it holds.
    Resident(&'static str),
    /// The fence entries do not tile the partition.
    Tiling(&'static str),
    /// A page's CRC does not match its bytes under this form file's `bind32`.
    PageCrc,
    /// A page's tag is not [`ZTAG`].
    Tag,
    /// A page's header holds a value no page may carry.
    Header(&'static str),
    /// Bytes after a page's payload are not zero.
    Padding,
    /// The LZ4 decoder refused a page's payload.
    Decode,
    /// The payload decoded to a length other than `ulen`.
    DecodedLen,
    /// A page's `first_rank` or `count` is not its fence entry's.
    Fence,
    /// A decoded group is not `count` records, whole, in key order.
    Walk(&'static str),
}

impl fmt::Display for ZError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZError::Length => write!(f, "z form: a page, trailer or region has the wrong length"),
            ZError::Magic => write!(f, "z form: the trailer's magic is wrong"),
            ZError::Version => write!(
                f,
                "z form: the trailer's version is not one this reader reads"
            ),
            ZError::PageSize => write!(f, "z form: the trailer's page size is not {ZPAGE}"),
            ZError::TrailerCrc => write!(f, "z form: the trailer's checksum does not match"),
            ZError::Trailer(why) => write!(f, "z form: the trailer is inconsistent: {why}"),
            ZError::ResidentCrc => write!(f, "z form: the resident part's checksum does not match"),
            ZError::Resident(why) => write!(f, "z form: the resident part is malformed: {why}"),
            ZError::Tiling(why) => {
                write!(f, "z form: the fence does not tile the partition: {why}")
            }
            ZError::PageCrc => write!(f, "z form: a page's checksum does not match"),
            ZError::Tag => write!(f, "z form: a page's tag is wrong"),
            ZError::Header(why) => write!(f, "z form: a page's header is invalid: {why}"),
            ZError::Padding => write!(f, "z form: a page's padding is not zero"),
            ZError::Decode => write!(f, "z form: a page's payload does not decode"),
            ZError::DecodedLen => {
                write!(f, "z form: a page decoded to a length other than its own")
            }
            ZError::Fence => write!(f, "z form: a page disagrees with its fence entry"),
            ZError::Walk(why) => write!(f, "z form: a decoded group is malformed: {why}"),
        }
    }
}

impl std::error::Error for ZError {}

impl From<ZError> for std::io::Error {
    fn from(e: ZError) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    }
}

/// A page's header, less the fields that are constants: the tag, the CRC and
/// the zero byte. What is page-wide; the groups are the directory's
/// ([`PageView`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageHeader {
    /// The rank of the page's first record.
    pub first_rank: u32,
    /// Records in the page, every group's, 1 to [`ZMAX_COUNT`].
    pub count: u16,
    /// Bit `j`: group `j`'s payload is its images verbatim.
    pub stored: u16,
    /// Bytes of the decoded groups, summed.
    pub ulen: u32,
    /// Bytes after the header the page uses: the directory, the keys and
    /// the payloads, at most [`ZCAP`]; zeros after.
    pub used: u16,
    /// Groups in the page, 1 to [`ZMAX_GROUPS`].
    pub groups: u8,
}

impl PageHeader {
    /// The values a page may carry, which writing and reading both hold it
    /// to; the directory's are [`PageView`]'s to hold.
    fn check(&self) -> Result<(), ZError> {
        if self.count == 0 || self.count as usize > ZMAX_COUNT {
            return Err(ZError::Header("count is not 1 to the most a page holds"));
        }
        if self.groups == 0 || self.groups as usize > ZMAX_GROUPS {
            return Err(ZError::Header("groups is not 1 to the most a page holds"));
        }
        if u32::from(self.groups) < u16::BITS && self.stored >> self.groups != 0 {
            return Err(ZError::Header("a stored bit names no group"));
        }
        if self.ulen == 0 || !self.ulen.is_multiple_of(4) {
            return Err(ZError::Header("ulen is not a multiple of four"));
        }
        if self.used as usize > ZCAP || (self.used as usize) < ZDIR_ENTRY * self.groups as usize {
            return Err(ZError::Header("used is not the directory to the cap"));
        }
        Ok(())
    }

    /// Whether group `j` is stored verbatim.
    pub fn is_stored(&self, j: usize) -> bool {
        j < 16 && self.stored & (1 << j) != 0
    }
}

/// One group of a page as the directory names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupEntry {
    /// The index in the page of the group's first record.
    pub first: u16,
    /// Records in the group: to the next group's first, or the page's
    /// count.
    pub count: u16,
    /// The payload's offset from the page's start, a multiple of four.
    pub coff: u16,
    /// Payload bytes.
    pub clen: u16,
    /// Bytes of the decoded group, a multiple of four, 4 to [`ZMAX_ULEN`].
    pub ulen: u32,
    /// The payload is the group verbatim.
    pub stored: bool,
    /// The payload is the record coding ([`ZCODING_RECORD`]): its records
    /// one by one, each decodable alone. Else, not stored, one LZ4 block.
    /// The high bit of the entry's decoded-length word, which the length
    /// never reaches.
    pub coded: bool,
}

/// The directory word's bit that says a group is record-coded.
const D_CODED: u16 = 0x8000;

/// A page parsed: its header, and the directory the groups' payloads and
/// keys are found by, over the page's own bytes. Made by [`parse_page`]
/// (every check) or [`page_view`] (the header's and the directory's values,
/// for a page a first decode has passed).
#[derive(Clone, Copy)]
pub struct PageView<'a> {
    /// The page-wide fields.
    pub header: PageHeader,
    page: &'a [u8; ZPAGE],
    /// Where the keys of groups 1.. begin, after the directory.
    keys_at: usize,
    /// Where each group's key lies in the page, `key_at[j]..key_end[j]`,
    /// for `j` in 1..groups; group 0's is the fence's.
    key_at: [u16; ZMAX_GROUPS],
    key_end: [u16; ZMAX_GROUPS],
}

impl<'a> PageView<'a> {
    /// Groups in the page.
    pub fn groups(&self) -> usize {
        usize::from(self.header.groups)
    }

    fn entry_raw(&self, j: usize) -> (u16, u16, u16, u16) {
        let at = ZHDR + j * ZDIR_ENTRY;
        let half = |o: usize| u16::from_le_bytes([self.page[at + o], self.page[at + o + 1]]);
        (half(D_FIRST), half(D_COFF), half(D_CLEN), half(D_ULEN4))
    }

    /// Group `j` as the directory names it, `None` past the last.
    pub fn group(&self, j: usize) -> Option<GroupEntry> {
        if j >= self.groups() {
            return None;
        }
        let (first, coff, clen, ulen4) = self.entry_raw(j);
        let next = if j + 1 < self.groups() {
            self.entry_raw(j + 1).0
        } else {
            self.header.count
        };
        // Firsts ascend in a directory the checked view passed; a trusted
        // view ([`view_trusted`]) has not asked, and a group whose first is
        // past the next's is no group.
        let count = next.checked_sub(first)?;
        Some(GroupEntry {
            first,
            count,
            coff,
            clen,
            ulen: u32::from(ulen4 & !D_CODED) * 4,
            stored: self.header.is_stored(j),
            coded: ulen4 & D_CODED != 0,
        })
    }

    /// Group `j`'s payload, where it lies in the page.
    pub fn payload(&self, j: usize) -> Option<&'a [u8]> {
        let g = self.group(j)?;
        self.page
            .get(usize::from(g.coff)..usize::from(g.coff) + usize::from(g.clen))
    }

    /// Group `j`'s payload with the rest of the page after it: the bytes a
    /// decoder may read past the payload's end inside the page, and the
    /// payload's length.
    pub fn payload_and_rest(&self, j: usize) -> Option<(&'a [u8], usize)> {
        let g = self.group(j)?;
        let rest = self.page.get(usize::from(g.coff)..)?;
        let len = usize::from(g.clen);
        (len <= rest.len()).then_some((rest, len))
    }

    /// The key of group `j`'s first record for `j` in 1..groups, where it
    /// lies in the page; `None` for group 0, whose key is the fence's, and
    /// past the last group.
    pub fn key(&self, j: usize) -> Option<&'a [u8]> {
        if j == 0 || j >= self.groups() {
            return None;
        }
        self.page
            .get(usize::from(self.key_at[j])..usize::from(self.key_end[j]))
    }

    /// The group that holds the record at index `i` of the page: the last
    /// whose first is at or below it. `i` below the count.
    pub fn group_of_index(&self, i: usize) -> usize {
        let mut j = 0;
        while j + 1 < self.groups() && usize::from(self.entry_raw(j + 1).0) <= i {
            j += 1;
        }
        j
    }

    /// The group `key` would be in: the last whose first key is at or
    /// below it, group 0 for a key below group 1's. A key below the page's
    /// own first key is the fence's to refuse; here it is group 0's.
    pub fn group_of_key(&self, key: &[u8]) -> usize {
        let mut j = 0;
        while j + 1 < self.groups()
            && self
                .key(j + 1)
                .is_some_and(|k| key_cmp(k, key) != Ordering::Greater)
        {
            j += 1;
        }
        j
    }
}

/// The page's checksum: CRC-32C over everything after the CRC field, xor the
/// form file's `bind32`.
pub fn page_crc(page: &[u8; ZPAGE], bind32: u32) -> u32 {
    crc32(&page[P_TAG..]) ^ bind32
}

/// One group to write into a page.
#[derive(Clone, Copy, Debug)]
pub struct PageGroup<'a> {
    /// Records in the group, at least one.
    pub count: u16,
    /// The payload: one LZ4 block, the record coding where `coded`, or the
    /// group verbatim where `stored`.
    pub payload: &'a [u8],
    /// Bytes of the decoded group.
    pub ulen: u32,
    pub stored: bool,
    pub coded: bool,
    /// The key of the group's first record. Group 0's is the fence's and
    /// is not written; the rest go into the directory.
    pub key: &'a [u8],
}

/// Where the payloads of groups with these compressed and key lengths
/// would lie: each group's `coff`, and the bytes the page would use after
/// the header. `None` where the keys alone pass the cap or a group is past
/// the most a page holds; whether the payloads fit is the caller's `used`
/// against [`ZCAP`].
pub fn page_layout(groups: &[(usize, usize)]) -> Option<(Vec<usize>, usize)> {
    if groups.is_empty() || groups.len() > ZMAX_GROUPS {
        return None;
    }
    let keys: usize = groups.iter().skip(1).map(|&(_, klen)| 2 + klen).sum();
    let mut at = (ZHDR + ZDIR_ENTRY * groups.len() + keys).next_multiple_of(4);
    if at > ZPAGE {
        return None;
    }
    let mut coffs = Vec::with_capacity(groups.len());
    for &(clen, _) in groups {
        coffs.push(at);
        at = (at + clen).next_multiple_of(4);
    }
    // The last group's padding is not used bytes: zeros after the payload.
    let last = coffs[coffs.len() - 1] + groups[groups.len() - 1].0;
    Some((coffs, last - ZHDR))
}

/// Write one page: the header, the directory, the keys of groups 1.., the
/// payloads each at a multiple of four, zeros, and the CRC last. The groups
/// must agree with themselves and fit; the page is untouched when they do
/// not.
pub fn encode_page(
    page: &mut [u8; ZPAGE],
    first_rank: u32,
    groups: &[PageGroup<'_>],
    bind32: u32,
) -> Result<PageHeader, ZError> {
    let bad = |why: &'static str| Err(ZError::Header(why));
    if groups.is_empty() || groups.len() > ZMAX_GROUPS {
        return bad("groups is not 1 to the most a page holds");
    }
    let mut count = 0usize;
    let mut ulen = 0u64;
    let mut stored = 0u16;
    for (j, g) in groups.iter().enumerate() {
        if g.count == 0 {
            return bad("a group of no records");
        }
        if g.ulen == 0 || !g.ulen.is_multiple_of(4) || g.ulen as usize > ZMAX_ULEN {
            return bad("a group's ulen is not a multiple of four in 4 to the most a group holds");
        }
        if g.payload.is_empty() || g.payload.len() > ZCAP {
            return bad("a group's payload is not 1 to the most a page holds");
        }
        if g.stored && g.payload.len() != g.ulen as usize {
            return bad("a stored group's payload is not its ulen");
        }
        if g.stored && g.coded {
            return bad("a stored group is not coded");
        }
        if j > 0 {
            if g.key.is_empty() || g.key.len() > u16::MAX as usize {
                return bad("a group's key is empty or over a u16");
            }
            if key_cmp(groups[j - 1].key, g.key) != Ordering::Less {
                return bad("the groups' keys do not ascend");
            }
        }
        count += usize::from(g.count);
        ulen += u64::from(g.ulen);
        stored |= u16::from(g.stored) << j;
    }
    if count > ZMAX_COUNT {
        return bad("count is not 1 to the most a page holds");
    }
    let shape: Vec<(usize, usize)> = groups
        .iter()
        .enumerate()
        .map(|(j, g)| (g.payload.len(), if j == 0 { 0 } else { g.key.len() }))
        .collect();
    let Some((coffs, used)) = page_layout(&shape) else {
        return bad("the groups do not fit the page");
    };
    if used > ZCAP {
        return bad("the groups do not fit the page");
    }
    let h = PageHeader {
        first_rank,
        count: count as u16,
        stored,
        ulen: ulen as u32,
        used: used as u16,
        groups: groups.len() as u8,
    };
    h.check()?;
    page.fill(0);
    page[P_TAG..P_TAG + 4].copy_from_slice(&ZTAG.to_le_bytes());
    page[P_FIRST..P_FIRST + 4].copy_from_slice(&h.first_rank.to_le_bytes());
    page[P_COUNT..P_COUNT + 2].copy_from_slice(&h.count.to_le_bytes());
    page[P_STORED..P_STORED + 2].copy_from_slice(&h.stored.to_le_bytes());
    page[P_ULEN..P_ULEN + 4].copy_from_slice(&h.ulen.to_le_bytes());
    page[P_USED..P_USED + 2].copy_from_slice(&h.used.to_le_bytes());
    page[P_GROUPS] = h.groups;
    page[P_ZERO] = 0;
    let mut first = 0u16;
    let mut keys_at = ZHDR + ZDIR_ENTRY * groups.len();
    for (j, g) in groups.iter().enumerate() {
        let at = ZHDR + j * ZDIR_ENTRY;
        page[at + D_FIRST..at + D_FIRST + 2].copy_from_slice(&first.to_le_bytes());
        page[at + D_COFF..at + D_COFF + 2].copy_from_slice(&(coffs[j] as u16).to_le_bytes());
        page[at + D_CLEN..at + D_CLEN + 2].copy_from_slice(&(g.payload.len() as u16).to_le_bytes());
        let ulen4 = (g.ulen / 4) as u16 | if g.coded { D_CODED } else { 0 };
        page[at + D_ULEN4..at + D_ULEN4 + 2].copy_from_slice(&ulen4.to_le_bytes());
        first += g.count;
        if j > 0 {
            page[keys_at..keys_at + 2].copy_from_slice(&(g.key.len() as u16).to_le_bytes());
            page[keys_at + 2..keys_at + 2 + g.key.len()].copy_from_slice(g.key);
            keys_at += 2 + g.key.len();
        }
        page[coffs[j]..coffs[j] + g.payload.len()].copy_from_slice(g.payload);
    }
    let crc = page_crc(page, bind32);
    page[P_CRC..P_CRC + 4].copy_from_slice(&crc.to_le_bytes());
    Ok(h)
}

/// The header's and the directory's values, checked against each other and
/// the page's bounds: what a view needs to hand out a payload or a key
/// safely. Nothing of the CRC, the tag or the zeros, which are
/// [`parse_page`]'s.
fn view_checked(page: &[u8]) -> Result<PageView<'_>, ZError> {
    view_with(page, true)
}

/// A verified page's view: the header's bounds and where each group's key
/// lies, and none of the cross-checks [`view_checked`] makes between the
/// directory's values -- the keys ascending, the firsts ascending below the
/// count, each payload where the one before ends, the lengths summing to
/// the header's. A page this process has passed through [`parse_page`] and
/// a first decode holds a directory those checks accepted, and every slice
/// a view hands out is bounds-checked where it is taken, so what the checks
/// buy a verified page is a repeat of what was checked once, at every group
/// open: a fifth of a scan's lookup work beside the decode, measured. A
/// directory a flipped byte changed after the verification is caught where
/// the checked view would catch it only by chance too -- at the decode, whose
/// length the payload no longer fits, or at the record parse -- and the page
/// leaves service there as it would. `Options::zpages_trusted_views`.
pub fn view_trusted(page: &[u8]) -> Result<PageView<'_>, ZError> {
    view_with(page, false)
}

/// The view, with or without the directory's cross-checks.
fn view_with(page: &[u8], checked: bool) -> Result<PageView<'_>, ZError> {
    let page: &[u8; ZPAGE] = page.try_into().map_err(|_| ZError::Length)?;
    let word = |at: usize| u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]]);
    let half = |at: usize| u16::from_le_bytes([page[at], page[at + 1]]);
    let h = PageHeader {
        first_rank: word(P_FIRST),
        count: half(P_COUNT),
        stored: half(P_STORED),
        ulen: word(P_ULEN),
        used: half(P_USED),
        groups: page[P_GROUPS],
    };
    h.check()?;
    let bad = |why: &'static str| Err(ZError::Header(why));
    let groups = usize::from(h.groups);
    let end = ZHDR + usize::from(h.used);
    let mut v = PageView {
        header: h,
        page,
        keys_at: ZHDR + ZDIR_ENTRY * groups,
        key_at: [0; ZMAX_GROUPS],
        key_end: [0; ZMAX_GROUPS],
    };
    // The keys of groups 1.., end to end after the directory.
    let mut at = v.keys_at;
    for j in 1..groups {
        if at + 2 > end {
            return bad("a group's key lies past the used bytes");
        }
        let klen = usize::from(half(at));
        if klen == 0 || at + 2 + klen > end {
            return bad("a group's key is empty or lies past the used bytes");
        }
        v.key_at[j] = (at + 2) as u16;
        v.key_end[j] = (at + 2 + klen) as u16;
        if checked
            && j > 1
            && key_cmp(v.key(j - 1).unwrap_or(&[]), &page[at + 2..at + 2 + klen]) != Ordering::Less
        {
            return bad("the groups' keys do not ascend");
        }
        at += 2 + klen;
    }
    if !checked {
        return Ok(v);
    }
    // The entries: firsts from zero ascending below the count, payloads
    // each at a multiple of four straight after the one before, the
    // last ending at the used bytes, each decoded length in range and a
    // stored group's payload its decoded length, the lengths summing to
    // the header's.
    let mut expect_coff = at.next_multiple_of(4);
    let mut ulen = 0u64;
    for j in 0..groups {
        let (first, coff, clen, ulen4) = v.entry_raw(j);
        let want_first = if j == 0 {
            0
        } else {
            let prev = v.entry_raw(j - 1).0;
            if first <= prev {
                return bad("the groups' firsts do not ascend");
            }
            first
        };
        if first != want_first || first >= h.count {
            return bad("a group's first is not in order below the count");
        }
        if usize::from(coff) != expect_coff || clen == 0 {
            return bad("a group's payload is not where the one before ends");
        }
        let gulen = u32::from(ulen4 & !D_CODED) * 4;
        if gulen == 0 || gulen as usize > ZMAX_ULEN {
            return bad("a group's ulen is not 4 to the most a group holds");
        }
        if h.is_stored(j) && u32::from(clen) != gulen {
            return bad("a stored group's clen is not its ulen");
        }
        if h.is_stored(j) && ulen4 & D_CODED != 0 {
            return bad("a stored group is not coded");
        }
        let pend = usize::from(coff) + usize::from(clen);
        if pend > end {
            return bad("a group's payload lies past the used bytes");
        }
        if j + 1 == groups && pend != end {
            return bad("the last payload does not end at the used bytes");
        }
        expect_coff = pend.next_multiple_of(4);
        ulen += u64::from(gulen);
    }
    if ulen != u64::from(h.ulen) {
        return bad("ulen is not the groups' sum");
    }
    Ok(v)
}

/// A page's view, after every check that needs no fence and no decoder:
/// the length, the CRC (first, so nothing below trusts an unchecked byte),
/// the tag, the zero byte, the header's and the directory's values, and
/// the zeros: the padding between the keys and the first payload, between
/// the payloads, and after the last.
pub fn parse_page(page: &[u8], bind32: u32) -> Result<PageView<'_>, ZError> {
    let p: &[u8; ZPAGE] = page.try_into().map_err(|_| ZError::Length)?;
    let word = |at: usize| u32::from_le_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]]);
    if word(P_CRC) != page_crc(p, bind32) {
        return Err(ZError::PageCrc);
    }
    if word(P_TAG) != ZTAG {
        return Err(ZError::Tag);
    }
    if p[P_ZERO] != 0 {
        return Err(ZError::Header("a reserved byte is not zero"));
    }
    let v = view_checked(page)?;
    let end = ZHDR + usize::from(v.header.used);
    if p[end..].iter().any(|&b| b != 0) {
        return Err(ZError::Padding);
    }
    // The bytes no field names: after the keys to the first payload, and
    // between payloads.
    let mut gap_from = if v.groups() > 1 {
        usize::from(v.key_end[v.groups() - 1])
    } else {
        v.keys_at
    };
    for j in 0..v.groups() {
        let g = v.group(j).ok_or(ZError::Length)?;
        if p[gap_from..usize::from(g.coff)].iter().any(|&b| b != 0) {
            return Err(ZError::Padding);
        }
        gap_from = usize::from(g.coff) + usize::from(g.clen);
    }
    Ok(v)
}

/// A page's view for a page this process has already passed through
/// [`parse_page`] and a first decode: the length and the header's and
/// directory's values are checked and nothing else, because the CRC is a
/// pass over the page and a reader's every read of a page it has verified
/// would pay it again. The tag, the zeros and the padding are the first
/// decode's; a page that has not had one is read with [`parse_page`].
pub fn page_view(page: &[u8]) -> Result<PageView<'_>, ZError> {
    view_checked(page)
}

/// A buffer a group decodes into: words, not bytes, so that it is aligned
/// for the extent arrays `parse_record` borrows in place. A `Vec<u8>` is not
/// promised to be, and a record read from a misaligned buffer is `None`,
/// which a probe loop reads as "absent".
///
/// It is as large as the largest group decoded into it so far, in whole
/// pages, and no larger: a thread that reads a covered partition keeps one
/// for its life, and a callback that reads again makes one of its own, so a
/// buffer made at [`ZMAX_ULEN`] was 64 KiB zeroed for every thread and every
/// nested read to hold groups that are a few kilobytes.
///
/// A reader handle's memo may keep its groups in the cells of a slab
/// instead (`in_cell`, `Options::zpages_memo_slab`): a cell is the bytes
/// while a group fits it, and a group that does not sends the buffer back
/// to words of its own, the cell handed back.
pub struct GroupBuf {
    words: Vec<u64>,
    cell: Option<super::blocks::SlabCell>,
}

impl GroupBuf {
    /// An empty buffer, which the first group decoded into it sizes.
    pub fn new() -> GroupBuf {
        GroupBuf {
            words: Vec::new(),
            cell: None,
        }
    }

    /// A buffer whose bytes are `cell`'s, for the groups that fit it.
    pub(crate) fn in_cell(cell: super::blocks::SlabCell) -> GroupBuf {
        GroupBuf {
            words: Vec::new(),
            cell: Some(cell),
        }
    }

    /// Where the buffer's bytes begin, aligned for words: the cell's
    /// where it holds one, else the words'.
    #[inline]
    fn base(&self) -> *const u8 {
        match &self.cell {
            Some(c) => c.as_ptr(),
            None => self.words.as_ptr() as *const u8,
        }
    }

    /// `base`, to write through.
    #[inline]
    fn base_mut(&mut self) -> *mut u8 {
        match &self.cell {
            Some(c) => c.as_ptr(),
            None => self.words.as_mut_ptr() as *mut u8,
        }
    }

    /// The first `n` bytes of the buffer, grown to hold them. Not zeroed
    /// between uses: a decode writes every byte of the group it returns, and
    /// nothing beyond it is ever lent. `n` is at most [`ZMAX_ULEN`], which
    /// the header's check has seen to.
    fn bytes_for(&mut self, n: usize) -> &mut [u8] {
        if self.capacity_bytes() < n {
            // Whole pages, so that groups a few bytes apart in size grow the
            // buffer once and not once each. The old contents are not kept:
            // nothing is lent from a buffer between two decodes. A cell
            // does not grow: the group goes to words and the cell back.
            self.cell = None;
            self.words = vec![0; n.div_ceil(8).next_multiple_of(ZPAGE / 8)];
        }
        // SAFETY: the buffer owns at least `n` initialised bytes -- its
        // words, or its cell, zero at first and written since -- `u8` has
        // no alignment to violate, and the borrow of `self` is the only
        // access while the slice lives.
        unsafe { std::slice::from_raw_parts_mut(self.base_mut(), n) }
    }
}

impl GroupBuf {
    /// The first `n` bytes and up to `slack` more, as many as the buffer's
    /// whole pages hold past `n`: room for a decoder's wide copies past a
    /// group's end that grows the buffer no further than the group does.
    fn bytes_with_slack(&mut self, n: usize, slack: usize) -> &mut [u8] {
        self.bytes_for(n);
        let room = self.capacity_bytes().min(n + slack);
        // SAFETY: `bytes_for` left the buffer owning at least `room`
        // initialised bytes, `u8` has no alignment to violate, and the
        // borrow of `self` is the only access while the slice lives.
        unsafe { std::slice::from_raw_parts_mut(self.base_mut(), room) }
    }

    /// Its first [`STRIDE_KEY_MAX`] bytes as the last write left them, the
    /// buffer grown to hold them where it is shorter: a stride cursor's key,
    /// rendered and stepped in place in the buffer the cursor holds, which a
    /// stride file, with no group to decode, leaves to it. Only the first
    /// call grows a buffer, so a key rendered into it is there at the next.
    pub fn stride_key(&mut self) -> &mut [u8; STRIDE_KEY_MAX] {
        if self.capacity_bytes() < STRIDE_KEY_MAX {
            self.grow_for_stride_key();
        }
        // SAFETY: the buffer owns at least `STRIDE_KEY_MAX` initialised
        // bytes, grown above where it did not, `u8` has no alignment to
        // violate, and the borrow of `self` is the only access while the
        // array lives.
        unsafe { &mut *(self.base_mut() as *mut [u8; STRIDE_KEY_MAX]) }
    }

    /// `stride_key`'s growth, once a thread's buffer, out of line.
    #[cold]
    #[inline(never)]
    fn grow_for_stride_key(&mut self) {
        self.bytes_for(STRIDE_KEY_MAX);
    }

    /// The bytes the buffer holds room for: what the last growth made it,
    /// in whole pages, and zero before the first decode into it; or its
    /// cell's bytes, where it holds one.
    pub fn capacity_bytes(&self) -> usize {
        match &self.cell {
            Some(c) => c.len(),
            None => self.words.len() * 8,
        }
    }

    /// Whether a slab's cell holds the bytes (`in_cell`).
    #[cfg(test)]
    pub(crate) fn is_cell(&self) -> bool {
        self.cell.is_some()
    }

    /// The first `n` bytes of the buffer as the last decode left them: what
    /// a cursor that keeps its group across calls reads the group through,
    /// since a decode's own borrow ends with the call that made it. `n` is
    /// at most what the last `bytes_for` was asked for.
    pub fn bytes(&self, n: usize) -> &[u8] {
        assert!(n <= self.capacity_bytes(), "a group longer than its buffer");
        // SAFETY: the buffer owns at least `n` initialised bytes and `u8`
        // has no alignment to violate.
        unsafe { std::slice::from_raw_parts(self.base(), n) }
    }
}

impl Default for GroupBuf {
    fn default() -> GroupBuf {
        GroupBuf::new()
    }
}

/// Group `j` of a page, decoded: LZ4 into `buf` (record by record for a
/// group of the record coding, [`decode_record_group`]), or for a stored
/// group the payload itself where it is aligned for `parse_record` and a
/// copy in `buf` where it is not. Either way the bytes returned are aligned, so a record
/// of any shape parses out of them.
///
/// The decoder is `lz4_flex`'s `block::decompress_into` called directly, not
/// the engine's `crate::block::decompress_into`, which discards the length it
/// decoded: the buffer is reused without zeroing, so a payload that decodes
/// to fewer bytes than `ulen` would leave stale bytes inside the group, and
/// the length is required to equal `ulen`. The caller has parsed the page
/// ([`parse_page`]) first, so the CRC has passed.
pub fn decode_group<'a>(
    view: &PageView<'a>,
    j: usize,
    buf: &'a mut GroupBuf,
) -> Result<&'a [u8], ZError> {
    decode_group_with(view, j, buf, false)
}

/// The LZ4 decoder a block-coded group is decoded by: `lz4_flex`'s, the
/// engine's own fast loop (`Options::zpages_own_decoder`), or the engine's
/// own with its flat loop ahead of the fast one
/// (`Options::zpages_flat_decode`, `zlz4::decompress_in_flat`). The same
/// bytes from every block and the same refusals of every damage, from all
/// three. A `bool` is the choice before the flat loop: the engine's own
/// decoder or `lz4_flex`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dec {
    #[default]
    Flex,
    Own,
    Flat,
    /// The flat loop given the room it runs to the group's end in
    /// (`zlz4::FLAT_SLACK`, `Options::zpages_flat_slack`).
    FlatEnd,
}

impl Dec {
    /// The decoder the two options name: `lz4_flex` without the engine's
    /// own, and the flat loop only with it.
    pub fn of(own: bool, flat: bool) -> Dec {
        match (own, flat) {
            (false, _) => Dec::Flex,
            (true, false) => Dec::Own,
            (true, true) => Dec::Flat,
        }
    }

    /// Whether the engine's own decoder is this one, flat or not.
    pub fn own(self) -> bool {
        self != Dec::Flex
    }

    /// Whether the flat loop runs.
    pub fn flat(self) -> bool {
        matches!(self, Dec::Flat | Dec::FlatEnd)
    }

    /// The flat loop given the room it runs to the group's end in, where
    /// `on` and the flat loop runs; this decoder otherwise.
    pub fn with_slack(self, on: bool) -> Dec {
        if on && self == Dec::Flat {
            Dec::FlatEnd
        } else {
            self
        }
    }

    /// The room past a group's decoded length its buffer is lent with.
    pub fn slack(self) -> usize {
        if self == Dec::FlatEnd {
            super::zlz4::FLAT_SLACK
        } else {
            super::zlz4::DECODE_SLACK
        }
    }
}

impl From<bool> for Dec {
    fn from(own: bool) -> Dec {
        Dec::of(own, false)
    }
}

/// [`decode_group`] with the block coding's groups decoded by the decoder
/// `dec` names (`Dec`): the same bytes from every block either encoder
/// writes, and the same refusals of every damage the page's checks let
/// through.
pub fn decode_group_with<'a>(
    view: &PageView<'a>,
    j: usize,
    buf: &'a mut GroupBuf,
    dec: impl Into<Dec>,
) -> Result<&'a [u8], ZError> {
    let dec = dec.into();
    let g = view
        .group(j)
        .ok_or(ZError::Header("no such group in the page"))?;
    let payload = view.payload(j).ok_or(ZError::Length)?;
    let ulen = g.ulen as usize;
    if ulen == 0 || ulen > ZMAX_ULEN {
        return Err(ZError::Header(
            "a group's ulen is not 4 to the most a group holds",
        ));
    }
    if g.stored {
        if (payload.as_ptr() as usize).is_multiple_of(std::mem::align_of::<crate::index::Ext>()) {
            return Ok(payload);
        }
        let out = buf.bytes_for(ulen);
        out.copy_from_slice(payload);
        return Ok(out);
    }
    if g.coded {
        return decode_record_group(payload, usize::from(g.count), ulen, buf);
    }
    if dec.own() {
        // The engine's own decoder, into the group's bytes and the slack
        // its fast loop runs to the end in (`zlz4::DECODE_SLACK`), as much
        // of it as the buffer's whole pages hold past the group. A
        // payload that decodes past the group is not a block of it, as
        // `lz4_flex` refuses one into an output of the group's length, and
        // one that decodes short is the length that disagrees: the same
        // refusals either decoder gives.
        let (rest, len) = view.payload_and_rest(j).ok_or(ZError::Length)?;
        let out = buf.bytes_with_slack(ulen, dec.slack());
        let n = if dec.flat() {
            super::zlz4::decompress_in_flat(rest, len, out)
        } else {
            super::zlz4::decompress_in(rest, len, out)
        }
        .map_err(|_| ZError::Decode)?;
        if n > ulen {
            return Err(ZError::Decode);
        }
        if n < ulen {
            return Err(ZError::DecodedLen);
        }
        return Ok(&out[..ulen]);
    }
    let out = buf.bytes_for(ulen);
    let n = lz4_flex::block::decompress_into(payload, out).map_err(|_| ZError::Decode)?;
    if n != ulen {
        return Err(ZError::DecodedLen);
    }
    Ok(out)
}

/// The output a point read's partial decode asks its decoder for past what
/// it has ([`seek_group_partly`]): two of the suite's records, so that a
/// call brings a record or two and the walk asks again once a record or
/// so. Smaller, and the calls' own cost is a share of a short decode;
/// larger, and the read decodes that much past its record.
pub const PART_STEP: usize = 256;

/// A block-coded group being decoded a part at a time for a walk
/// ([`open_group_partly`], [`decode_group_more`]): where its decoder
/// stands, the payload with the rest of its page behind it, and the bytes
/// the whole group decodes to.
#[derive(Clone, Copy, Debug)]
pub struct PartGroup<'a> {
    st: super::zlz4::Part,
    rest: &'a [u8],
    len: usize,
    ulen: usize,
    /// The flat loop runs (`Dec::Flat`), and the room past the group the
    /// buffer is lent with (`Dec::slack`), the same at every call.
    flat: bool,
    slack: usize,
}

impl PartGroup<'_> {
    /// The group's bytes decoded so far, never the decoder's overrun past
    /// them.
    pub fn have(&self) -> usize {
        self.st.op.min(self.ulen)
    }

    /// The bytes the whole group decodes to.
    pub fn ulen(&self) -> usize {
        self.ulen
    }

    /// Whether the decode has reached the group's end.
    pub fn done(&self) -> bool {
        self.st.done
    }
}

/// Group `j` of a page this process has verified, begun: decoded into
/// `buf` from its first byte to `stop` bytes, a sequence past it at most,
/// or to its end where that comes first, and handed back as what
/// [`decode_group_more`] goes on from. `Ok(None)` for a group this does
/// not serve, which the caller opens whole: one that is stored or
/// record-coded.
///
/// For a walk's own decode of a group it will not read to the end
/// (`Options::zpages_walk_part`): a scan's last group is read half way at
/// the mean, the decoder is sequential and its cost is its tokens. The
/// page is one whose every group a first decode has walked whole, as
/// [`seek_group_partly`]'s is, and nothing is verified again; what a
/// decode that stops short does not see is said there. The bytes of `buf`
/// past [`PartGroup::have`] are not the group's.
pub fn open_group_partly<'a>(
    view: &PageView<'a>,
    j: usize,
    buf: &mut GroupBuf,
    stop: usize,
) -> Result<Option<PartGroup<'a>>, ZError> {
    open_group_partly_with(view, j, buf, stop, Dec::Own)
}

/// [`open_group_partly`] with the flat loop where `dec` names it.
pub fn open_group_partly_with<'a>(
    view: &PageView<'a>,
    j: usize,
    buf: &mut GroupBuf,
    stop: usize,
    dec: Dec,
) -> Result<Option<PartGroup<'a>>, ZError> {
    let g = view
        .group(j)
        .ok_or(ZError::Header("no such group in the page"))?;
    if g.stored || g.coded {
        return Ok(None);
    }
    let ulen = g.ulen as usize;
    if ulen == 0 || ulen > ZMAX_ULEN {
        return Err(ZError::Header(
            "a group's ulen is not 4 to the most a group holds",
        ));
    }
    let (rest, len) = view.payload_and_rest(j).ok_or(ZError::Length)?;
    let mut part = PartGroup {
        st: super::zlz4::Part::default(),
        rest,
        len,
        ulen,
        flat: dec.flat(),
        slack: dec.slack(),
    };
    decode_group_more(&mut part, buf, stop)?;
    Ok(Some(part))
}

/// The decode `part` began taken on to `stop` bytes or the group's end,
/// in `buf`, the buffer it began in, which nothing has written since: the
/// bytes of the group decoded after it. A call always decodes a sequence
/// more of a group not yet at its end, whatever `stop`. An `Err` is
/// damage, as [`seek_group_partly`]'s is: a payload that does not decode,
/// decodes past the group, or ends short of it.
pub fn decode_group_more(
    part: &mut PartGroup<'_>,
    buf: &mut GroupBuf,
    stop: usize,
) -> Result<usize, ZError> {
    if part.st.done {
        return Ok(part.have());
    }
    let out = buf.bytes_with_slack(part.ulen, part.slack);
    let stop = stop.max(part.st.op);
    if part.flat {
        super::zlz4::decompress_more_flat(part.rest, part.len, out, &mut part.st, stop)
    } else {
        super::zlz4::decompress_more(part.rest, part.len, out, &mut part.st, stop)
    }
    .map_err(|_| ZError::Decode)?;
    if part.st.op > part.ulen {
        return Err(ZError::Decode);
    }
    if part.st.done && part.st.op < part.ulen {
        return Err(ZError::DecodedLen);
    }
    Ok(part.have())
}

/// Where a key falls in a group decoded only as far as the walk for it
/// read ([`seek_group_partly`]): the record's offset, or `None` for a key
/// the group does not hold; the bytes of the group decoded, which hold the
/// record whole; and the bytes the whole group decodes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartSeek {
    pub off: Option<usize>,
    pub len: usize,
    pub of: usize,
}

/// [`group_seek`] over group `j` of a page this process has verified, the
/// group decoded into `buf` a part at a time and only as far as the walk
/// reads: the records are walked from the first as [`group_seek`] walks
/// them, and where the next record does not lie whole in what is decoded
/// the decoder is asked for [`PART_STEP`] bytes more
/// (`zlz4::decompress_more`), until the key is met, passed, or the group
/// ends. `Ok(None)` for a group this does not serve, which the caller opens
/// whole: one that is stored or record-coded.
///
/// For a point read's first touch of a group (`Options::zpages_point_part`):
/// the read wants one record, the decoder is sequential and its cost is its
/// tokens, so a read of a record in the middle of its group pays for half
/// of it. The page is one whose every group a first decode has walked
/// whole (`first_checks`), so the prefix decoded here is a prefix of bytes
/// already held to the page's CRC and the walk's checks; nothing is
/// verified again, and a caller must not use this on a page that has not
/// been. The bytes of `buf` past the length answered are not the group's.
///
/// An `Err` is damage, as [`group_seek`]'s is: a payload that does not
/// decode, decodes past the group, ends short of it, or a record that does
/// not parse with the whole group decoded. A walk that stops before the
/// group's end does not see what a whole decode learns there, a decoded
/// length that disagrees with the directory's: bytes that change under a
/// verified page are no more this path's to find than the whole decode's,
/// which reads the page's CRC once, at the page's first decode.
pub fn seek_group_partly(
    view: &PageView<'_>,
    j: usize,
    buf: &mut GroupBuf,
    key: &[u8],
) -> Result<Option<PartSeek>, ZError> {
    seek_group_partly_with(view, j, buf, key, Dec::Own)
}

/// [`seek_group_partly`] with the flat loop where `dec` names it.
pub fn seek_group_partly_with(
    view: &PageView<'_>,
    j: usize,
    buf: &mut GroupBuf,
    key: &[u8],
    dec: Dec,
) -> Result<Option<PartSeek>, ZError> {
    const CUT: ZError = ZError::Walk("a record does not parse");
    let g = view
        .group(j)
        .ok_or(ZError::Header("no such group in the page"))?;
    if g.stored || g.coded {
        return Ok(None);
    }
    let ulen = g.ulen as usize;
    if ulen == 0 || ulen > ZMAX_ULEN {
        return Err(ZError::Header(
            "a group's ulen is not 4 to the most a group holds",
        ));
    }
    let (rest, len) = view.payload_and_rest(j).ok_or(ZError::Length)?;
    let out = buf.bytes_with_slack(ulen, dec.slack());
    let count = usize::from(g.count);
    let mut st = super::zlz4::Part::default();
    let mut off = 0usize;
    for _ in 0..count {
        let (k, next) = loop {
            // The group's bytes decoded so far, never the decoder's
            // overrun past them.
            let have = st.op.min(ulen);
            if let Some(r) = record_key_next(&out[..have], off) {
                break r;
            }
            if st.done {
                return Err(CUT);
            }
            let stop = st.op.max(off) + PART_STEP;
            if dec.flat() {
                super::zlz4::decompress_more_flat(rest, len, out, &mut st, stop)
            } else {
                super::zlz4::decompress_more(rest, len, out, &mut st, stop)
            }
            .map_err(|_| ZError::Decode)?;
            if st.op > ulen {
                return Err(ZError::Decode);
            }
            if st.done && st.op < ulen {
                return Err(ZError::DecodedLen);
            }
        };
        // Nothing sorts below the empty key, so the first record is its
        // answer, found where that record's key is empty too, and no
        // compare is made, as `group_seek` makes none.
        let ord = if !key.is_empty() {
            key_cmp(k, key)
        } else if k.is_empty() {
            Ordering::Equal
        } else {
            Ordering::Greater
        };
        let at = match ord {
            Ordering::Less => {
                off = next;
                continue;
            }
            Ordering::Equal => Some(off),
            Ordering::Greater => None,
        };
        return Ok(Some(PartSeek {
            off: at,
            len: st.op.min(ulen),
            of: ulen,
        }));
    }
    Ok(Some(PartSeek {
        off: None,
        len: st.op.min(ulen),
        of: ulen,
    }))
}

/// Which LZ4 decoder a build links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lz4Decoder {
    /// The decoder the engine's manifest asks for: `lz4_flex` with its
    /// default features off.
    Unsafe,
    /// The decoder the crate's default features select.
    Safe,
    /// Neither answered as the version this was written against does.
    Unknown,
}

/// Which decoder this build links, found by running it. A read's largest
/// fixed cost is the decode of a page, and the two decoders do not cost the
/// same; cargo unifies a dependency's features across the whole build, so a
/// tool whose own dependencies ask for the crate's defaults (a dev-dependency
/// of the suite did) links the engine to the other decoder, and a timing of
/// the decode taken there is a timing of a codec the engine does not ship.
/// The two check a malformed block in the other order -- a match at an offset
/// there is no data for, into an output with no room for it -- and say which
/// they are. Their encoders emit the same bytes, so no output tells them
/// apart, and only the decoder is found here; a build that asks for the safe
/// decoder (`safe-decode`) asks for the safe encoder (`safe-encode`) beside
/// it.
pub fn lz4_decoder() -> Lz4Decoder {
    use lz4_flex::block::DecompressError;
    let block = [0x10u8, b'a', 0x05, 0x00];
    let mut out = [0u8; 1];
    match lz4_flex::block::decompress_into(&block, &mut out) {
        Err(DecompressError::OffsetOutOfBounds) => Lz4Decoder::Unsafe,
        Err(DecompressError::OutputTooSmall { .. }) => Lz4Decoder::Safe,
        _ => Lz4Decoder::Unknown,
    }
}

/// The fingerprint of the encoder every size this layout's tools print
/// depends on: the bytes in, the bytes out and the CRC-32C of the bytes out
/// of [`lz4_encoder_pin`]'s fixed input, as `lz4_flex` 0.11.6 makes them.
/// A release of the crate that compresses differently moves every
/// records-a-page figure.
///
/// It is a computation and not a version because the build that prints a
/// size is not the build that tests the engine: the suite is its own
/// workspace with a lock of its own, so a version read from the engine's
/// lock says nothing of the encoder a tool linked, and a version bump that
/// leaves the output alone is not a change to any size. Whoever finds the
/// fingerprint moved takes the sizing again (`bench/examples/zsize.rs`)
/// before the new bytes are believed, and then moves the constant.
pub const LZ4_ENCODER_PIN: (usize, usize, u32) = (3720, 2024, 0xE1D3_F522);

/// What the encoder this build links makes of a fixed input, to be compared
/// with [`LZ4_ENCODER_PIN`]: (bytes in, bytes out, CRC-32C of the bytes out).
pub fn lz4_encoder_pin() -> (usize, usize, u32) {
    let input = encoder_pin_input();
    let out = lz4_flex::block::compress(&input);
    (input.len(), out.len(), crc32(&out))
}

/// Thirty of the suite's records, compact, end to end: a counter for a
/// sixteen-digit key and a value of a hundred bytes, half a repeated phrase
/// at a phase drawn for each and half entropy from a fixed stream. The
/// entropy is the module's tests' own generator (an xorshift with a
/// multiplier) and a test holds the two to the same bytes, so that the pin
/// is taken over images a test can look at.
fn encoder_pin_input() -> Vec<u8> {
    let mut state = 5u64.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut word = move || {
        let mut x = state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
    let mut out = Vec::new();
    for i in 0..30 {
        let key = format!("{i:016}");
        let phase = (word() % motif.len() as u64) as usize;
        let mut value: Vec<u8> = (0..50).map(|j| motif[(phase + j) % motif.len()]).collect();
        value.extend((0..50).map(|_| (word() >> 24) as u8));
        let ext = crate::index::Ext {
            block: crate::index::Ext::INLINE,
            off: 0,
            len: value.len() as u32,
            last: 0,
            count: 1 | crate::index::Ext::FIXED,
        };
        crate::flatindex::stream_record_as(&mut out, key.as_bytes(), &[ext], &value, true)
            .expect("a record of the suite's shape is a record");
    }
    out
}

/// A decoded group walked end to end: `count` records laid end to end with
/// nothing after them, keys strictly ascending, the first the key given
/// where one is (the fence's for a page's first group, the directory's for
/// the rest), and every key below `below` where one is given (the next
/// group's first key). `parse_record` judges each record, so a compact
/// header its run contradicts, an extent array that is not aligned and a
/// record cut short are all refused. This is the walk a group's first decode
/// makes, once per group and process; [`group_seek`] is the walk a read
/// makes after it.
pub fn walk_group(
    group: &[u8],
    count: usize,
    first_key: Option<&[u8]>,
    below: Option<&[u8]>,
) -> Result<(), ZError> {
    let mut off = 0usize;
    let mut prev: Option<&[u8]> = None;
    for i in 0..count {
        let (key, _, _, len) =
            parse_record(group, off).ok_or(ZError::Walk("a record does not parse"))?;
        if i == 0 && first_key.is_some_and(|f| f != key) {
            return Err(ZError::Walk("the first key is not the one named for it"));
        }
        if prev.is_some_and(|p| p >= key) {
            return Err(ZError::Walk("keys do not ascend"));
        }
        if below.is_some_and(|b| key_cmp(key, b) != Ordering::Less) {
            return Err(ZError::Walk("a key is not below the next group's first"));
        }
        prev = Some(key);
        off = off
            .checked_add(len)
            .ok_or(ZError::Walk("a record's length overflows"))?;
    }
    if off != group.len() {
        return Err(ZError::Walk("the group is not exactly count records"));
    }
    Ok(())
}

/// Where a key falls in a decoded group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupPos {
    /// Records whose key is below the key sought: the index of the first
    /// record at or above it, or `count` when every record is below.
    pub index: usize,
    /// The offset of that record in the group, or the offset the walk ended at
    /// when there is none.
    pub off: usize,
    /// The record at `index` has the key sought.
    pub found: bool,
}

/// The walk that searches a group: from offset zero, each record's key and
/// the offset of the next from `record_key_next`, no extent rebuilt, keys
/// compared as the engine's other searches compare them.
///
/// An `Err` is a group that is not whole -- a record that does not parse
/// before the position is reached -- which the first decode's [`walk_group`]
/// has already refused, so a search over a verified group cannot return one.
/// It is damage and never an absent key: an absent key is
/// `Ok(GroupPos { found: false, .. })`. A caller that reads an `Err` as
/// "not here" answers wrongly with nothing raised, the silent miss; it
/// counts the damage and reads the raw form.
pub fn group_seek(group: &[u8], count: usize, key: &[u8]) -> Result<GroupPos, ZError> {
    const CUT: ZError = ZError::Walk("a record does not parse");
    // Nothing sorts below the empty key, so the answer is the first record's
    // and no compare is made: a compare of a zero-length slice is a call like
    // any other, and an empty key held in a `Vec` points at no page.
    if key.is_empty() {
        let found = count > 0 && record_key_next(group, 0).ok_or(CUT)?.0.is_empty();
        return Ok(GroupPos {
            index: 0,
            off: 0,
            found,
        });
    }
    let mut off = 0usize;
    for index in 0..count {
        let (k, next) = record_key_next(group, off).ok_or(CUT)?;
        match super::key_cmp(k, key) {
            Ordering::Less => off = next,
            Ordering::Equal => {
                return Ok(GroupPos {
                    index,
                    off,
                    found: true,
                })
            }
            Ordering::Greater => {
                return Ok(GroupPos {
                    index,
                    off,
                    found: false,
                })
            }
        }
    }
    Ok(GroupPos {
        index: count,
        off,
        found: false,
    })
}

/// A group's coding, in the high bit of its directory entry's decoded
/// length ([`GroupEntry::coded`]), and the fill's preference
/// (`FillScratch::coding`): how a group the `stored` mask does not name
/// holds its records.
///
/// `ZCODING_BLOCK`: a group's payload is one LZ4 block of its images laid
/// end to end, and a read decodes the whole group for one record.
/// `ZCODING_RECORD`: the payload holds the records one by one, each
/// decodable alone -- an offset table, each record's head front-coded
/// against the first record's and its tail LZ4-compressed against the first
/// record's tail, which is stored raw -- so a point read decodes one record
/// and a scan decodes the records it emits ([`encode_record_group`]).
pub const ZCODING_BLOCK: u8 = 0;
pub const ZCODING_RECORD: u8 = 1;

/// The record coding's fields are LEB128 varints: one byte to 127, two to
/// 16,383, and so on; nothing in a group needs more than four.
fn put_var(out: &mut [u8], at: &mut usize, mut v: usize) -> Option<()> {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let slot = out.get_mut(*at)?;
        *at += 1;
        if v == 0 {
            *slot = b;
            return Some(());
        }
        *slot = b | 0x80;
    }
}

fn get_var(b: &[u8], at: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    let mut shift = 0u32;
    loop {
        let x = *b.get(*at)?;
        *at += 1;
        v |= usize::from(x & 0x7f) << shift;
        if x & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
}

/// A record image's head: the two header halves and the key, `4 + klen`
/// bytes; the tail begins at the next multiple of four.
fn image_head_len(img: &[u8]) -> Option<usize> {
    let klen = usize::from(u16::from_le_bytes([*img.first()?, *img.get(1)?]));
    let head = 4 + klen;
    (head <= img.len()).then_some(head)
}

/// The record coding of a group: `ends.len()` images laid end to end in
/// `imgs`, image `i` ending at `ends[i]`, written into `out` as
///
/// ```text
/// [u16 off_i; n]                      where record i begins in the payload
/// record i:
///   varint plen, varint slen, suffix   head_i = head_0[..plen] ++ suffix
///   varint ulen_i                      the image's length
///   varint clen_i                      the tail's stored length
///   tail_i                             image_i from its aligned tail start:
///                                      LZ4 against tail_0, or raw when
///                                      clen_i is the tail's length
/// ```
///
/// Record 0 is stored whole: its head as the suffix of a zero prefix and its
/// tail raw, since it is the dictionary. A tail LZ4 does not shrink is stored
/// raw. The payload's length, or `None` where the coding cannot hold the
/// group: an image that is not one, a record that would begin past a u16,
/// or an output too small, which `FillScratch::classify` reads as a group
/// too big for the page.
pub fn encode_record_group(
    imgs: &[u8],
    ends: &[usize],
    out: &mut [u8],
    raw_tails: bool,
) -> Option<usize> {
    let n = ends.len();
    if n == 0 || n > ZMAX_COUNT || out.len() < 2 * n {
        return None;
    }
    let img0 = imgs.get(..ends[0])?;
    let head0 = image_head_len(img0)?;
    let tail0 = img0.get(head0.next_multiple_of(4)..)?;
    let mut at = 2 * n;
    let mut start = 0usize;
    for (i, &end) in ends.iter().enumerate() {
        let img = imgs.get(start..end)?;
        if img.is_empty() || !img.len().is_multiple_of(4) {
            return None;
        }
        if at > usize::from(u16::MAX) {
            return None;
        }
        out[2 * i..2 * i + 2].copy_from_slice(&(at as u16).to_le_bytes());
        let head = image_head_len(img)?;
        let plen = if i == 0 {
            0
        } else {
            img[..head]
                .iter()
                .zip(&img0[..head0])
                .take_while(|(a, b)| a == b)
                .count()
        };
        put_var(out, &mut at, plen)?;
        put_var(out, &mut at, head - plen)?;
        out.get_mut(at..at + head - plen)?
            .copy_from_slice(&img[plen..head]);
        at += head - plen;
        put_var(out, &mut at, img.len())?;
        let tail = &img[head.next_multiple_of(4)..];
        // The compressed tail, where it is shorter; raw otherwise. The
        // length field goes before the tail, so the compression is made
        // into the room after where the field at its longest would end.
        let field_max = 3;
        let room = out.get_mut(at + field_max..)?;
        let clen = if i == 0 || tail.len() < 8 || raw_tails {
            tail.len()
        } else {
            match lz4_flex::block::compress_into_with_dict(tail, room, tail0) {
                Ok(c) if c < tail.len() => c,
                _ => tail.len(),
            }
        };
        if clen < tail.len() {
            // Written into the room past the field at its longest: move it
            // down to right after the field as written.
            let mut field_at = at;
            put_var(out, &mut field_at, clen)?;
            out.copy_within(at + field_max..at + field_max + clen, field_at);
            at = field_at + clen;
        } else {
            put_var(out, &mut at, clen)?;
            out.get_mut(at..at + clen)?.copy_from_slice(tail);
            at += clen;
        }
        start = end;
    }
    Some(at)
}

/// Record `i`'s fields in a record-coded payload, located and checked:
/// where its head's suffix, its tail and its lengths are.
pub struct CodedRec<'a> {
    pub plen: usize,
    pub suffix: &'a [u8],
    pub ulen: usize,
    pub tail: &'a [u8],
}

impl<'a> CodedRec<'a> {
    pub fn head_len(&self) -> usize {
        self.plen + self.suffix.len()
    }

    /// Where the tail begins in the image.
    pub fn tail_at(&self) -> usize {
        self.head_len().next_multiple_of(4)
    }

    /// Whether the tail is stored raw: the image's bytes from its tail
    /// start, verbatim, which a reader may parse where they lie
    /// (`encode_record_group` under `raw_tails`, and every record's whose
    /// tail LZ4 did not shrink).
    pub fn tail_is_raw(&self) -> bool {
        self.tail.len() == self.ulen - self.tail_at()
    }
}

pub const REC_CUT: ZError = ZError::Walk("a coded record does not parse");

pub fn coded_rec(payload: &[u8], count: usize, i: usize) -> Result<CodedRec<'_>, ZError> {
    if i >= count || count > ZMAX_COUNT || payload.len() < 2 * count {
        return Err(REC_CUT);
    }
    let mut at = usize::from(u16::from_le_bytes([payload[2 * i], payload[2 * i + 1]]));
    if at < 2 * count {
        return Err(REC_CUT);
    }
    let plen = get_var(payload, &mut at).ok_or(REC_CUT)?;
    let slen = get_var(payload, &mut at).ok_or(REC_CUT)?;
    let suffix = payload.get(at..at + slen).ok_or(REC_CUT)?;
    at += slen;
    let ulen = get_var(payload, &mut at).ok_or(REC_CUT)?;
    let clen = get_var(payload, &mut at).ok_or(REC_CUT)?;
    let tail = payload.get(at..at + clen).ok_or(REC_CUT)?;
    let r = CodedRec {
        plen,
        suffix,
        ulen,
        tail,
    };
    // A head holds its two halves at least; the image is whole words, at
    // most a group; the tail fits where the head ends.
    if r.head_len() < 4
        || ulen == 0
        || !ulen.is_multiple_of(4)
        || ulen > ZMAX_ULEN
        || r.tail_at() > ulen
        || clen > ulen - r.tail_at()
    {
        return Err(REC_CUT);
    }
    if i == 0 && (plen != 0 || clen != ulen - r.tail_at()) {
        return Err(ZError::Walk(
            "a coded group's first record is not stored whole",
        ));
    }
    Ok(r)
}

/// Record 0 of a record-coded payload: the head every other record's is
/// coded against and the tail they are compressed against.
pub fn coded_first(payload: &[u8], count: usize) -> Result<(&[u8], &[u8]), ZError> {
    let r0 = coded_rec(payload, count, 0)?;
    Ok((r0.suffix, r0.tail))
}

/// Record `i` of a record-coded group written into `out`, which is the
/// image's length: the head from the first record's and the suffix, zeros to
/// the tail, the tail decoded against the first record's.
fn decode_record_into(
    payload: &[u8],
    count: usize,
    i: usize,
    head0: &[u8],
    tail0: &[u8],
    r: &CodedRec<'_>,
    out: &mut [u8],
) -> Result<(), ZError> {
    debug_assert_eq!(out.len(), r.ulen);
    let _ = (payload, count, i);
    if r.plen > head0.len() {
        return Err(ZError::Walk(
            "a coded record's prefix is longer than the first head",
        ));
    }
    let head = r.head_len();
    out[..r.plen].copy_from_slice(&head0[..r.plen]);
    out[r.plen..head].copy_from_slice(r.suffix);
    let tail_at = r.tail_at();
    out[head..tail_at].fill(0);
    let want = r.ulen - tail_at;
    if r.tail.len() == want {
        out[tail_at..].copy_from_slice(r.tail);
        return Ok(());
    }
    let n = lz4_flex::block::decompress_into_with_dict(r.tail, &mut out[tail_at..], tail0)
        .map_err(|_| ZError::Decode)?;
    if n != want {
        return Err(ZError::DecodedLen);
    }
    Ok(())
}

/// Record `i` of a record-coded group of `count` records, decoded into
/// `buf`: the image, aligned, as `parse_record` reads one.
pub fn decode_record<'a>(
    payload: &[u8],
    count: usize,
    i: usize,
    buf: &'a mut GroupBuf,
) -> Result<&'a [u8], ZError> {
    let (head0, tail0) = coded_first(payload, count)?;
    let r = coded_rec(payload, count, i)?;
    let out = buf.bytes_for(r.ulen);
    decode_record_into(payload, count, i, head0, tail0, &r, out)?;
    Ok(out)
}

/// A record-coded group decoded whole into `buf`: the images end to end,
/// `ulen` bytes, as the block coding's decode lays them, so everything that
/// reads a decoded group reads this one.
pub fn decode_record_group<'a>(
    payload: &[u8],
    count: usize,
    ulen: usize,
    buf: &'a mut GroupBuf,
) -> Result<&'a [u8], ZError> {
    let (head0, tail0) = coded_first(payload, count)?;
    let out = buf.bytes_for(ulen);
    let mut at = 0usize;
    for i in 0..count {
        let r = coded_rec(payload, count, i)?;
        let end = at.checked_add(r.ulen).ok_or(REC_CUT)?;
        let slot = out.get_mut(at..end).ok_or(ZError::DecodedLen)?;
        decode_record_into(payload, count, i, head0, tail0, &r, slot)?;
        at = end;
    }
    if at != ulen {
        return Err(ZError::DecodedLen);
    }
    Ok(out)
}

/// A coded group's key `i` assembled into `into`: the first record's head
/// to the prefix length, the suffix, and the key is what follows the four
/// header bytes. `false`, with `into` cleared, for a prefix longer than the
/// first record's head, which is damage: a forged payload once panicked the
/// check here instead of failing it.
pub fn coded_key_into(head0: &[u8], r: &CodedRec<'_>, into: &mut Vec<u8>) -> bool {
    into.clear();
    if r.plen > head0.len() {
        return false;
    }
    into.extend_from_slice(&head0[..r.plen]);
    into.extend_from_slice(r.suffix);
    into.drain(..4);
    true
}

/// A coded record's four header bytes -- the key's length and the extent
/// count or the compact mark, as `flatindex::parse_record` reads them from
/// an image -- taken from the first record's head to the prefix length and
/// the suffix after it, which is where the record's head lies without
/// assembling it. `None` for a prefix longer than the first head.
pub fn coded_header(head0: &[u8], r: &CodedRec<'_>) -> Option<[u8; 4]> {
    if r.plen > head0.len() || r.head_len() < 4 {
        return None;
    }
    let mut h = [0u8; 4];
    for (k, b) in h.iter_mut().enumerate() {
        *b = if k < r.plen {
            head0[k]
        } else {
            r.suffix[k - r.plen]
        };
    }
    Some(h)
}

/// That a record-coded group `j` is what its page's directory and its
/// fence entry say, read from its heads and nothing decoded: every record's
/// fields in range (`coded_rec`), the image lengths summing to the group's,
/// the keys strictly ascending, the first the entry's key for group 0 and
/// the directory's for the rest, every one below the next group's first.
/// What [`check_group`] checks of a decoded group, less the tails, whose
/// decode is checked at the read that wants them -- a tail that decodes to
/// the wrong length or not at all is damage there as a record that does not
/// parse is in a decoded group -- so a page's first touch under this coding
/// walks heads where the block coding decodes every group of the page.
pub fn check_coded_group(
    payload: &[u8],
    view: &PageView<'_>,
    j: usize,
    entry: &FenceEntry<'_>,
) -> Result<(), ZError> {
    let g = view
        .group(j)
        .ok_or(ZError::Header("no such group in the page"))?;
    let count = usize::from(g.count);
    let first = if j == 0 { Some(entry.key) } else { view.key(j) };
    let below = view.key(j + 1);
    let (head0, _) = coded_first(payload, count)?;
    let mut prev: Vec<u8> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut ulen = 0usize;
    for i in 0..count {
        let r = coded_rec(payload, count, i)?;
        ulen = ulen.checked_add(r.ulen).ok_or(REC_CUT)?;
        if !coded_key_into(head0, &r, &mut cur) {
            return Err(ZError::Walk(
                "a coded record's prefix is longer than the first head",
            ));
        }
        if i == 0 && first.is_some_and(|f| f != cur.as_slice()) {
            return Err(ZError::Walk("the first key is not the one named for it"));
        }
        if i > 0 && key_cmp(&prev, &cur) != Ordering::Less {
            return Err(ZError::Walk("keys do not ascend"));
        }
        if below.is_some_and(|b| key_cmp(&cur, b) != Ordering::Less) {
            return Err(ZError::Walk("a key is not below the next group's first"));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    if ulen != g.ulen as usize {
        return Err(ZError::DecodedLen);
    }
    Ok(())
}

/// The key of a coded record, `head_i[4..]`, compared with `key` without
/// assembling it: the part the shared prefix holds, then the suffix's. The
/// prefix is within the first head: the caller has checked it, as
/// `check_coded_group` has for every record of a verified group.
pub fn cmp_coded_key(head0: &[u8], r: &CodedRec<'_>, key: &[u8]) -> Ordering {
    let a = &head0[r.plen.min(4)..r.plen];
    let b = &r.suffix[4usize.saturating_sub(r.plen)..];
    // key_i = a ++ b against key, lexicographically.
    let n = a.len().min(key.len());
    match a[..n].cmp(&key[..n]) {
        Ordering::Equal => {}
        o => return o,
    }
    if key.len() < a.len() {
        return Ordering::Greater;
    }
    b.cmp(&key[n..])
}

/// Where a key falls in a record-coded group, as [`group_seek`] answers for
/// a decoded one, with `off` the record's offset in the payload. The heads
/// are read in order and no record is decoded.
pub fn record_group_seek(payload: &[u8], count: usize, key: &[u8]) -> Result<GroupPos, ZError> {
    let (head0, _) = coded_first(payload, count)?;
    for index in 0..count {
        let r = coded_rec(payload, count, index)?;
        let off = usize::from(u16::from_le_bytes([
            payload[2 * index],
            payload[2 * index + 1],
        ]));
        match cmp_coded_key(head0, &r, key) {
            Ordering::Less => {}
            Ordering::Equal => {
                return Ok(GroupPos {
                    index,
                    off,
                    found: true,
                })
            }
            Ordering::Greater => {
                return Ok(GroupPos {
                    index,
                    off,
                    found: false,
                })
            }
        }
    }
    Ok(GroupPos {
        index: count,
        off: payload.len(),
        found: false,
    })
}

/// Where `fill_group` reads record images from: the records of one
/// partition, in rank order.
pub trait Images {
    /// How many records there are.
    fn records(&self) -> usize;
    /// The image of record `rank`, less than `records()`: the bytes
    /// `parse_record` consumes for it, padding included. An empty slice says
    /// the record could not be read.
    fn image(&self, rank: usize) -> &[u8];
}

impl Images for [Vec<u8>] {
    fn records(&self) -> usize {
        self.len()
    }
    fn image(&self, rank: usize) -> &[u8] {
        self.get(rank).map_or(&[], |v| v.as_slice())
    }
}

impl Images for Vec<Vec<u8>> {
    fn records(&self) -> usize {
        self.len()
    }
    fn image(&self, rank: usize) -> &[u8] {
        self.get(rank).map_or(&[], |v| v.as_slice())
    }
}

/// The records of a segment as images, read where the directory says they
/// lie. `recs` and `dir` are the segment's record region and directory
/// (`FlatIndex::regions`) and `nkeys` its key count.
///
/// The directory is the authority for where a record lies and never the
/// difference of two directory words: a segment closed from a direct run
/// keeps its commit markers inside the record region, counted in every later
/// word, and read as a record a marker is a four-byte "record" followed by
/// garbage.
pub struct SegImages<'a> {
    recs: &'a [u8],
    dir: &'a [u8],
    nkeys: usize,
}

impl<'a> SegImages<'a> {
    /// The images of the segment whose record region and directory these are.
    pub fn new(recs: &'a [u8], dir: &'a [u8], nkeys: usize) -> SegImages<'a> {
        SegImages { recs, dir, nkeys }
    }
}

impl Images for SegImages<'_> {
    fn records(&self) -> usize {
        self.nkeys
    }
    fn image(&self, rank: usize) -> &[u8] {
        let Some(off) = rank.checked_mul(4).and_then(|at| rd_u32(self.dir, at)) else {
            return &[];
        };
        let off = off as usize;
        match record_key_next(self.recs, off) {
            Some((_, next)) => self.recs.get(off..next).unwrap_or(&[]),
            None => &[],
        }
    }
}

/// Why a record is not held by a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPage {
    /// The record's image is over [`ZMAX_ULEN`]: no group could hold it.
    Oversize,
    /// The record alone does not fit a page once compressed, and stored it is
    /// over a page too.
    Incompressible,
    /// The record is not an image a group can hold: it is empty, or not a
    /// multiple of four bytes, or its rank does not fit a u32.
    Unplaceable,
}

/// A page `fill_page` built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filled {
    /// Records in the page, from the rank asked for.
    pub count: usize,
    /// Bytes of the decoded groups, summed.
    pub ulen: usize,
    /// Payload bytes in the page, summed over the groups.
    pub clen: usize,
    /// Bytes after the header the page uses: the directory, the keys and
    /// the payloads.
    pub used: usize,
    /// Groups in the page.
    pub groups: usize,
    /// Groups whose payload is the images verbatim.
    pub stored: usize,
    /// Compress calls this page cost.
    pub calls: u32,
}

/// What `fill_page` made of the records from a rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// A page is in the caller's buffer, decoded and compared with the
    /// images it was built from.
    Page(Filled),
    /// The record at the rank is not held by a page, and is read from the
    /// raw segment. One record: the next call starts at the next rank.
    NoPage(NoPage),
    /// There is no record at the rank.
    End,
    /// The page built did not decode to the images it was built from, or the
    /// compressor failed: a bug in the builder or the codec, which fails the
    /// build rather than ship a page whose CRC is good and whose bytes are
    /// wrong.
    Mismatch,
}

/// What `fill_page` has done since its scratch was made. Counters only:
/// nothing here changes what a call returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillStats {
    /// Pages built.
    pub pages: u64,
    /// Records in them.
    pub records: u64,
    /// Groups in them.
    pub groups: u64,
    /// Groups whose payload is the images verbatim.
    pub stored: u64,
    /// Records declined.
    pub nopages: u64,
    /// Compress calls.
    pub calls: u64,
    /// Turns taken, each an attempt to compress a group within a group's
    /// budget: the calls' compress calls but for the ones a fallback adds.
    pub turns: u64,
    /// Growth rounds that fit.
    pub growths: u64,
    /// Groups that spent every turn without a fit and fell back to the
    /// first record alone. The arithmetic of the turns says none does.
    pub fallbacks: u64,
    /// Payload bytes in the pages built.
    pub payload_bytes: u64,
    /// Bytes after the header the pages use: the payloads and what the
    /// directories and keys cost.
    pub used_bytes: u64,
    /// Decoded bytes in the pages built.
    pub group_bytes: u64,
}

/// A group accepted into the page being built: its records, its images'
/// span in `page_imgs`, its payload's span in `payloads`, and whether the
/// payload is the images.
#[derive(Clone, Copy)]
struct Accepted {
    count: usize,
    img_at: usize,
    ulen: usize,
    pay_at: usize,
    clen: usize,
    stored: bool,
    coded: bool,
}

/// The buffers `fill_page` works in, kept between calls so that a page
/// costs no allocation, and the knobs.
pub struct FillScratch {
    /// The images of the group being tried, end to end, and where each ends.
    imgs: Vec<u8>,
    ends: Vec<usize>,
    /// Compressor output for the group being tried.
    out: Vec<u8>,
    /// The groups accepted into the page so far: their images end to end,
    /// their payloads end to end, and each one's spans.
    page_imgs: Vec<u8>,
    payloads: Vec<u8>,
    accepted: Vec<Accepted>,
    /// A fit's compressed bytes kept aside at the end of `payloads` while
    /// a growth is tried over the output buffer: where they are and how
    /// many.
    held: Option<(usize, usize)>,
    /// Where the page just built is decoded to be compared.
    verify: GroupBuf,
    /// Growth rounds after a first fit, in the one-group shape.
    /// [`ZGROW_ROUNDS`] until a measurement of fill against compress calls
    /// says otherwise.
    pub grow_rounds: u32,
    /// The decoded bytes a group is cut at: a group takes records from its
    /// first until the next would pass it, and the page holds as many
    /// groups as fit, the last trimmed to the room. Zero is one group a
    /// page, grown to fill it. [`ZGROUP_BYTES`] to begin with.
    pub group_bytes: usize,
    /// The previous page's record count, where the one-group shape starts
    /// its first attempt; zero before the first page. A caller may set it,
    /// as the sizing tool's sweep does to start a run near its steady
    /// state.
    pub hint: usize,
    /// The coding tried first for a group that compresses:
    /// [`ZCODING_BLOCK`], the options' coding and the shape this module's
    /// figures were taken on, unless a caller says [`ZCODING_RECORD`] (the
    /// arm `Options::zpages_record_coding`, or the selector's pick of the
    /// raw-tail form under `ZPages::Auto`, `zselect::choose_coding`), under
    /// which a record alone that only the block holds is the block's
    /// (`compress`).
    pub coding: u8,
    /// Under the record coding, every tail stored raw and LZ4 never tried on
    /// one (`Options::zpages_raw_tails`): the heads front-coded, the values
    /// the records' own bytes, a decode a copy a record.
    pub raw_tails: bool,
    /// The block coding's groups written by the parsing encoder
    /// (`Options::zpages_parse`, `super::zlz4`) and not `lz4_flex`'s greedy
    /// one: the same format in fewer sequences.
    pub parse: bool,
    /// Under `parse`, the parse's search: zero the shipped lazy parse
    /// (`zlz4::Effort::SHIPPED`), else a greedy parse over that many of
    /// each position's chain (`Options::zpages_parse_chains`).
    pub parse_chains: u32,
    /// The pages the fill writes read back through the engine's own decoder
    /// (`Options::zpages_own_decoder`), as the readers of the store that
    /// builds them will read them.
    pub own_decode: bool,
    /// That encoder's tables, made at its first group.
    parser: Option<Box<super::zlz4::Encoder>>,
    /// What has been done with this scratch.
    pub stats: FillStats,
}

impl FillScratch {
    /// A scratch with every buffer at its largest.
    pub fn new() -> FillScratch {
        // `compress_into` refuses an output shorter than the worst case for
        // its input, whatever the data would compress to, so the buffer is
        // sized for the largest group and a group is judged by the length
        // it compressed to.
        // The record coding's worst case is over the block's: an offset a
        // record, its fields, and LZ4's fixed overhead once a tail.
        let out = lz4_flex::block::get_maximum_output_size(ZMAX_ULEN) + 32 * ZMAX_COUNT;
        FillScratch {
            imgs: Vec::with_capacity(ZMAX_ULEN),
            ends: Vec::with_capacity(ZMAX_COUNT),
            out: vec![0; out],
            page_imgs: Vec::with_capacity(ZMAX_ULEN),
            payloads: Vec::with_capacity(ZPAGE),
            accepted: Vec::with_capacity(ZMAX_GROUPS),
            held: None,
            verify: GroupBuf::new(),
            grow_rounds: ZGROW_ROUNDS,
            group_bytes: ZGROUP_BYTES,
            hint: 0,
            coding: ZCODING_BLOCK,
            raw_tails: false,
            parse: false,
            parse_chains: 0,
            own_decode: false,
            parser: None,
            stats: FillStats::default(),
        }
    }

    /// Bring the staged images to `m` records, from `rank`. Stops when the
    /// staged bytes pass [`ZMAX_ULEN`], or at a record that is not an image.
    fn stage<I: Images + ?Sized>(&mut self, src: &I, rank: usize, m: usize) -> Stage {
        if self.ends.len() > m {
            self.ends.truncate(m);
            self.imgs.truncate(self.ends.last().copied().unwrap_or(0));
        }
        if self.imgs.len() > ZMAX_ULEN {
            return Stage::Over;
        }
        while self.ends.len() < m {
            let j = self.ends.len();
            let img = src.image(rank + j);
            if img.is_empty() || !img.len().is_multiple_of(4) {
                return Stage::Bad(j);
            }
            self.imgs.extend_from_slice(img);
            self.ends.push(self.imgs.len());
            if self.imgs.len() > ZMAX_ULEN {
                return Stage::Over;
            }
        }
        Stage::Ok
    }

    /// Stage the records from `rank`, at most `limit` of them, while the
    /// staged bytes stay within `target` -- the first whatever its size,
    /// up to [`ZMAX_ULEN`]. Stops at a record that is not an image.
    fn stage_to<I: Images + ?Sized>(
        &mut self,
        src: &I,
        rank: usize,
        limit: usize,
        target: usize,
    ) -> Stage {
        self.imgs.clear();
        self.ends.clear();
        while self.ends.len() < limit {
            let j = self.ends.len();
            let img = src.image(rank + j);
            if img.is_empty() || !img.len().is_multiple_of(4) {
                return Stage::Bad(j);
            }
            if j > 0 && self.imgs.len() + img.len() > target {
                break;
            }
            self.imgs.extend_from_slice(img);
            self.ends.push(self.imgs.len());
            if self.imgs.len() > ZMAX_ULEN {
                return Stage::Over;
            }
        }
        Stage::Ok
    }

    /// The staged group compressed, and what the length says to do with it
    /// against `room`, the payload bytes the page has left. `None` if the
    /// compressor failed, which a buffer sized for the largest group makes
    /// unreachable.
    fn classify(&mut self, ulen: usize, room: usize) -> Option<Class> {
        let (n, coded) = self.compress(ulen, room)?;
        self.stats.calls += 1;
        Some(if n >= ulen && ulen <= room {
            // LZ4 did not help and the group fits as it is: its own expansion
            // is never a reason to shrink a group or to give a record up.
            Class::Stored
        } else if n > room {
            Class::TooBig(n)
        } else {
            Class::Lz4(n, coded)
        })
    }

    /// The staged group's payload into `out`: the record coding where it
    /// is the preference, and one LZ4 block otherwise -- or for a record
    /// alone whose heads pass the room coded, since a long key LZ4 shrinks
    /// and a head is never compressed; a group of more records that passes
    /// the room is trimmed by the caller and tried coded again. The bytes
    /// and which coding they are. A group the record coding cannot hold in
    /// the buffer (its offsets past a u16) is reported over the room, with
    /// nothing of it in `out`, which a `TooBig` never reads.
    fn compress(&mut self, ulen: usize, room: usize) -> Option<(usize, bool)> {
        if self.coding == ZCODING_RECORD {
            let n = encode_record_group(
                &self.imgs[..ulen],
                &self.ends,
                &mut self.out,
                self.raw_tails,
            )
            .unwrap_or(ulen + ulen / 8 + 64 * self.ends.len().max(1));
            if n <= room || ulen <= room || self.ends.len() > 1 {
                return Some((n, true));
            }
        }
        let n = if self.parse {
            self.parser
                .get_or_insert_with(|| Box::new(super::zlz4::Encoder::new()))
                .compress_into(
                    &self.imgs[..ulen],
                    &mut self.out,
                    super::zlz4::Effort::of_chains(self.parse_chains),
                )?
        } else {
            lz4_flex::block::compress_into(&self.imgs[..ulen], &mut self.out).ok()?
        };
        Some((n, false))
    }

    fn decline(&mut self, why: NoPage) -> Fill {
        self.stats.nopages += 1;
        Fill::NoPage(why)
    }

    /// Nothing accepted yet: the start of a page.
    fn reset_page(&mut self) {
        self.page_imgs.clear();
        self.payloads.clear();
        self.accepted.clear();
    }

    /// The compressed and key lengths of the groups accepted, with one more
    /// group of `clen` payload bytes and a `klen`-byte key: what
    /// [`page_layout`] shapes a page from.
    fn shape_with(&self, clen: usize, klen: usize) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self
            .accepted
            .iter()
            .enumerate()
            .map(|(j, a)| (a.clen, if j == 0 { 0 } else { self.key_len(j) }))
            .collect();
        v.push((clen, if v.is_empty() { 0 } else { klen }));
        v
    }

    /// The payload bytes the page has left for one more group whose key is
    /// `klen` bytes: the cap less what the layout takes with it at no
    /// payload; `None` where there is none.
    fn room_for(&self, klen: usize) -> Option<usize> {
        let (_, used) = page_layout(&self.shape_with(0, klen))?;
        ZCAP.checked_sub(used)
    }

    /// Accepted group `j`'s key: its first record's.
    fn key_of(&self, j: usize) -> &[u8] {
        let a = self.accepted[j];
        record_key(&self.page_imgs[a.img_at..a.img_at + a.ulen], 0).unwrap_or(&[])
    }

    fn key_len(&self, j: usize) -> usize {
        self.key_of(j).len()
    }

    /// The group that fit, moved from the staging buffers into the page's.
    fn accept(&mut self, fit: Fit) {
        let img_at = self.page_imgs.len();
        self.page_imgs.extend_from_slice(&self.imgs[..fit.ulen]);
        let pay_at = self.payloads.len();
        let (clen, stored, coded) = match fit.body {
            Body::Stored => {
                self.payloads.extend_from_slice(&self.imgs[..fit.ulen]);
                (fit.ulen, true, false)
            }
            Body::Lz4 { clen, coded } => {
                self.payloads.extend_from_slice(&self.out[..clen]);
                (clen, false, coded)
            }
        };
        self.accepted.push(Accepted {
            count: fit.m,
            img_at,
            ulen: fit.ulen,
            pay_at,
            clen,
            stored,
            coded,
        });
    }
}

impl Default for FillScratch {
    fn default() -> FillScratch {
        FillScratch::new()
    }
}

enum Stage {
    Ok,
    /// The staged bytes are over [`ZMAX_ULEN`].
    Over,
    /// The record at this index of the group is not an image.
    Bad(usize),
}

enum Class {
    Stored,
    /// Compressed to this many bytes, under the record coding where `bool`.
    Lz4(usize, bool),
    TooBig(usize),
}

#[derive(Clone, Copy)]
enum Body {
    Stored,
    Lz4 { clen: usize, coded: bool },
}

/// A group that fits the page's room: its records, its decoded bytes and
/// its body, the compressed bytes in the scratch's output buffer.
#[derive(Clone, Copy)]
struct Fit {
    m: usize,
    ulen: usize,
    body: Body,
}

impl Fit {
    fn body_len(&self) -> usize {
        match self.body {
            Body::Stored => self.ulen,
            Body::Lz4 { clen, .. } => clen,
        }
    }
}

/// Why a group was not fitted into the page's room.
enum NoFit {
    /// The record alone does not fit the room, or is no image, or is over
    /// the largest group: the first group of a page declines it, a later
    /// group closes the page before it and leaves it to the next page's
    /// first group to decline.
    Decline(NoPage),
    /// The compressor failed.
    Mismatch,
}

/// Build the page at `rank`: groups of records from there, each cut at
/// [`FillScratch::group_bytes`] decoded bytes and compressed on its own, as
/// many as fit the page with the last trimmed to the room; or, at a target
/// of zero, one group sized so that its compressed bytes fill the page.
///
/// The one-group shape predicts, then verifies: the previous page's count
/// is where it starts (zero starts from one record, and a growth by the
/// room that one record leaves reaches the count its own bytes allow, not
/// the count a group of many compresses to, so whoever sizes a partition
/// from such calls has to chain a few first); the group of `m` records is
/// compressed, and a group that fits is grown (by the records the slack
/// holds, at the group's own bytes a record, [`FillScratch::grow_rounds`]
/// times) while one that does not is trimmed in proportion to the
/// overshoot for the first four attempts and halved after. The cut shape
/// stages the records up to the target, compresses them once, and trims
/// the same way only where the room is short. Either way the count is one
/// by the twelfth attempt at the latest ([`ZFILL_TURNS`], a group's
/// budget), and a record that cannot fit alone is declined, not retried.
///
/// The function is total. No input makes it panic, form a page of no
/// records or loop past its turns: the first record at `rank` is either in
/// a page, or declined as [`NoPage`] for a reason, or `rank` is past the end
/// ([`Fill::End`]). Each of these once was a way to fail:
///
/// * The shrink of a group that overshot, `m * room / len`, is zero whenever
///   `m * room < len` -- two records of 4,500 compressed bytes, or seven of
///   5,100 -- and a group of no records divided by its own count. `m` is
///   never below one here.
/// * A lone record of 4,056 to 4,072 bytes that does not compress has an LZ4
///   output over a page although the record fits one. A group whose LZ4
///   output is no smaller than its images is stored verbatim if the images
///   fit, and LZ4's expansion is never a reason to shrink a group or to
///   give a record up.
/// * `compress_into` cannot say whether a group fits: it refuses any
///   output buffer shorter than the worst case for its input, whatever the
///   data would compress to. The group is compressed into a buffer for the
///   largest group and its length is compared with the room.
///
/// A record with a block-backed extent is copied verbatim, extent and all,
/// and stays exact; its value still lives in the raw segment's block region.
/// A record whose image is over [`ZMAX_ULEN`], or does not fit a page even
/// alone, is declined and read from the raw segment.
///
/// The page built is decoded and compared with the images it was built from
/// before it is returned, because a bug here would otherwise ship a page
/// whose CRC is good and whose bytes are wrong, which no checksum downstream
/// can see.
pub fn fill_page<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    bind32: u32,
    scratch: &mut FillScratch,
    page: &mut [u8; ZPAGE],
) -> Fill {
    fill_with_turns(src, rank, bind32, scratch, page, ZFILL_TURNS)
}

/// `fill_page` with the turn budget a parameter, so that a test can spend
/// it and reach the path after it.
fn fill_with_turns<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    bind32: u32,
    s: &mut FillScratch,
    page: &mut [u8; ZPAGE],
    turns: u32,
) -> Fill {
    let n = src.records();
    if rank >= n {
        return Fill::End;
    }
    let Ok(first_rank) = u32::try_from(rank) else {
        return s.decline(NoPage::Unplaceable);
    };
    s.reset_page();
    let mut calls = 0u32;
    let mut at = rank;
    let mut idx = 0usize;
    loop {
        let g = s.accepted.len();
        if g == ZMAX_GROUPS || at >= n || idx >= ZMAX_COUNT {
            break;
        }
        // The next group's key, for the room its directory entry leaves:
        // the first record's. A record that is no image is the first
        // group's to decline and a later group's to close the page before;
        // so is an image the directory cannot name, one with no key to
        // parse or a key not above the group before it (never a record of
        // a partition, whose keys ascend), since the first group of a page
        // is named by the fence and needs none.
        let img = src.image(at);
        if img.is_empty() || !img.len().is_multiple_of(4) {
            if g == 0 {
                return s.decline(NoPage::Unplaceable);
            }
            break;
        }
        let klen = if g == 0 {
            0
        } else {
            match record_key(img, 0) {
                Some(k) if key_cmp(s.key_of(g - 1), k) == Ordering::Less => k.len(),
                _ => break,
            }
        };
        let Some(room) = s.room_for(klen) else { break };
        let limit = (n - at).min(ZMAX_COUNT - idx);
        let fit = if s.group_bytes == 0 {
            fit_one_group(src, at, limit, room, s, turns, &mut calls)
        } else {
            fit_cut_group(src, at, limit, room, s, turns, &mut calls)
        };
        match fit {
            Ok(fit) => {
                at += fit.m;
                idx += fit.m;
                s.accept(fit);
            }
            Err(NoFit::Mismatch) => return Fill::Mismatch,
            Err(NoFit::Decline(why)) => {
                if g == 0 {
                    return s.decline(why);
                }
                break;
            }
        }
        if s.group_bytes == 0 {
            break;
        }
    }
    seal_page(s, first_rank, calls, bind32, page)
}

/// The cut shape's group at `rank`: the records up to the target, trimmed
/// to the room where its compressed bytes pass it.
fn fit_cut_group<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    limit: usize,
    room: usize,
    s: &mut FillScratch,
    turns: u32,
    calls: &mut u32,
) -> Result<Fit, NoFit> {
    let target = s.group_bytes.min(ZMAX_ULEN);
    let mut m = match s.stage_to(src, rank, limit, target) {
        Stage::Ok => s.ends.len(),
        Stage::Bad(0) => return Err(NoFit::Decline(NoPage::Unplaceable)),
        // Up to the record that is no image.
        Stage::Bad(j) => j,
        Stage::Over => {
            if s.ends.len() == 1 {
                return Err(NoFit::Decline(NoPage::Oversize));
            }
            (s.ends.len() / 2).max(1)
        }
    };
    // The page's last group is the one the room cuts, and the groups
    // before it say what a record compresses to here: a group the ratio
    // says will not fit is staged shorter before it is compressed, so the
    // room costs a trim and not a compress call and a trim.
    let (records, bytes) = s
        .accepted
        .iter()
        .fold((0usize, 0usize), |(r, b), a| (r + a.count, b + a.clen));
    if records > 0 && m > 1 {
        let per = (bytes / records).max(1);
        if m * per > room {
            m = (room / per).clamp(1, m);
        }
    }
    let mut turn = 0u32;
    while turn < turns {
        match s.stage(src, rank, m) {
            Stage::Ok => {}
            Stage::Bad(j) => {
                if j == 0 {
                    return Err(NoFit::Decline(NoPage::Unplaceable));
                }
                m = m.min(j);
                continue;
            }
            Stage::Over => {
                if s.ends.len() == 1 {
                    return Err(NoFit::Decline(NoPage::Oversize));
                }
                m = (m / 2).max(1);
                continue;
            }
        }
        turn += 1;
        s.stats.turns += 1;
        let ulen = s.imgs.len();
        let Some(class) = s.classify(ulen, room) else {
            return Err(NoFit::Mismatch);
        };
        *calls += 1;
        match class {
            Class::Stored => {
                return Ok(Fit {
                    m,
                    ulen,
                    body: Body::Stored,
                })
            }
            Class::Lz4(clen, coded) => {
                return Ok(Fit {
                    m,
                    ulen,
                    body: Body::Lz4 { clen, coded },
                })
            }
            Class::TooBig(c) => {
                if m == 1 {
                    return Err(NoFit::Decline(NoPage::Incompressible));
                }
                m = trimmed(m, c, room, turn);
            }
        }
    }
    fallback(src, rank, room, s, calls)
}

/// The count a group that compressed to `c` bytes over `room` is cut to:
/// in proportion to the overshoot, and by at least one, for the first four
/// turns; halved after, so that it is one by the twelfth.
fn trimmed(m: usize, c: usize, room: usize, turn: u32) -> usize {
    if turn <= 4 {
        let cut = ((c - room) * m).div_ceil(c).max(1);
        m.saturating_sub(cut).max(1)
    } else {
        (m / 2).max(1)
    }
}

/// Every turn spent without a fit. A turn is an attempt, the first four
/// cut the count in proportion (by at least one) and the rest halve it,
/// so from the most a group holds the count is one at the twelfth and the
/// twelfth decides: this path is not reached with the turns the layout
/// gives. It is the path a smaller budget reaches, and what it does is the
/// first record alone, once, so that a spent budget is a group of one and
/// not a hole.
fn fallback<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    room: usize,
    s: &mut FillScratch,
    calls: &mut u32,
) -> Result<Fit, NoFit> {
    s.stats.fallbacks += 1;
    match s.stage(src, rank, 1) {
        Stage::Ok => {}
        Stage::Over => return Err(NoFit::Decline(NoPage::Oversize)),
        Stage::Bad(_) => return Err(NoFit::Decline(NoPage::Unplaceable)),
    }
    let ulen = s.imgs.len();
    let Some(class) = s.classify(ulen, room) else {
        return Err(NoFit::Mismatch);
    };
    *calls += 1;
    match class {
        Class::Stored => Ok(Fit {
            m: 1,
            ulen,
            body: Body::Stored,
        }),
        Class::Lz4(clen, coded) => Ok(Fit {
            m: 1,
            ulen,
            body: Body::Lz4 { clen, coded },
        }),
        Class::TooBig(_) => Err(NoFit::Decline(NoPage::Incompressible)),
    }
}

/// The one-group shape's group at `rank`: sized from the scratch's hint so
/// that its compressed bytes fill the room, grown by the slack.
fn fit_one_group<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    limit: usize,
    room: usize,
    s: &mut FillScratch,
    turns: u32,
    calls: &mut u32,
) -> Result<Fit, NoFit> {
    s.imgs.clear();
    s.ends.clear();
    // The most records the group can hold: the cap, the records left, and
    // the first record that is not an image.
    let mut limit = limit;
    let mut m = s.hint.clamp(1, limit);
    let mut turn = 0u32;
    let mut found: Option<Fit> = None;
    while turn < turns {
        // Staging is not a turn: only an attempt to compress is. It can
        // send the loop round again at most nine times, once for a record
        // that is not an image (after which no count passes it) and each
        // time for a group over the cap (which halves the count).
        match s.stage(src, rank, m) {
            Stage::Ok => {}
            Stage::Bad(j) => {
                if j == 0 {
                    return Err(NoFit::Decline(NoPage::Unplaceable));
                }
                limit = j;
                m = m.min(j);
                continue;
            }
            Stage::Over => {
                if s.ends.len() == 1 {
                    return Err(NoFit::Decline(NoPage::Oversize));
                }
                m = (m / 2).max(1);
                continue;
            }
        }
        turn += 1;
        s.stats.turns += 1;
        let ulen = s.imgs.len();
        let Some(class) = s.classify(ulen, room) else {
            return Err(NoFit::Mismatch);
        };
        *calls += 1;
        match class {
            Class::Stored => {
                found = Some(Fit {
                    m,
                    ulen,
                    body: Body::Stored,
                });
                break;
            }
            Class::Lz4(clen, coded) => {
                found = Some(Fit {
                    m,
                    ulen,
                    body: Body::Lz4 { clen, coded },
                });
                break;
            }
            Class::TooBig(c) => {
                if m == 1 {
                    return Err(NoFit::Decline(NoPage::Incompressible));
                }
                m = trimmed(m, c, room, turn);
            }
        }
    }
    let mut fit = match found {
        Some(f) => f,
        None => fallback(src, rank, room, s, calls)?,
    };
    // Growth: a group that fit with a record's room to spare is grown by the
    // records the slack holds, at the bytes a record has taken so far; past
    // the cap, to the most records that fit under it. A growth that does not
    // fit returns the group that did: it never replaces a group with a
    // smaller one. The group that fit is kept in the page's buffers while a
    // larger one is tried, since the compressor's output is one buffer.
    let mut rounds = s.grow_rounds;
    while rounds > 0 && turn < turns {
        let body = fit.body_len();
        let slack = room - body;
        let per = (body / fit.m).max(1);
        if fit.m >= limit || slack < per {
            break;
        }
        let mut m2 = (fit.m + slack / per).min(limit);
        s.hold(fit);
        match s.stage(src, rank, m2) {
            Stage::Ok => {}
            Stage::Bad(j) => {
                limit = j;
                s.unhold(fit);
                continue;
            }
            Stage::Over => {
                // The grown group passes the cap: grow to the most records
                // that fit under it, if that is more than the group has. The
                // staged bytes are the records so far, so the count is a
                // search of where they end and not another try.
                m2 = s.ends.partition_point(|&e| e <= ZMAX_ULEN);
                if m2 <= fit.m || !matches!(s.stage(src, rank, m2), Stage::Ok) {
                    s.unhold(fit);
                    break;
                }
            }
        }
        turn += 1;
        s.stats.turns += 1;
        let ulen = s.imgs.len();
        let Some(class) = s.classify(ulen, room) else {
            return Err(NoFit::Mismatch);
        };
        *calls += 1;
        match class {
            Class::Stored => {
                fit = Fit {
                    m: m2,
                    ulen,
                    body: Body::Stored,
                };
                s.drop_held();
            }
            Class::Lz4(clen, coded) => {
                fit = Fit {
                    m: m2,
                    ulen,
                    body: Body::Lz4 { clen, coded },
                };
                s.drop_held();
            }
            Class::TooBig(_) => {
                s.unhold(fit);
                break;
            }
        }
        rounds -= 1;
        s.stats.growths += 1;
    }
    s.hint = fit.m;
    Ok(fit)
}

impl FillScratch {
    /// A fit's compressed bytes kept aside while a larger group is tried
    /// over the same output buffer: at the end of the page's payloads,
    /// where `unhold` restores them to the output buffer and `drop_held`
    /// forgets them.
    fn hold(&mut self, fit: Fit) {
        if let Body::Lz4 { clen, .. } = fit.body {
            let at = self.payloads.len();
            self.payloads.extend_from_slice(&self.out[..clen]);
            self.held = Some((at, clen));
        }
    }

    fn unhold(&mut self, fit: Fit) {
        if let (Body::Lz4 { clen, .. }, Some((at, n))) = (fit.body, self.held.take()) {
            debug_assert_eq!(clen, n);
            self.out[..n].copy_from_slice(&self.payloads[at..at + n]);
            self.payloads.truncate(at);
        }
        // The images of the fit are staged again by the caller's next
        // `stage`, which truncates to the count.
        let keep = self.ends.get(fit.m - 1).copied().unwrap_or(0);
        self.ends.truncate(fit.m);
        self.imgs.truncate(keep);
    }

    fn drop_held(&mut self) {
        if let Some((at, _)) = self.held.take() {
            self.payloads.truncate(at);
        }
    }
}

/// The page of the groups that fit: written into `page`, then read back as
/// a reader reads a page and compared with the images the groups were built
/// from, and only then counted and returned.
///
/// This is the whole of the builder's guard against a page whose CRC is good
/// and whose bytes are wrong -- the CRC is computed over whatever was
/// written -- so the comparison is of the page as a reader decodes it, not
/// of the buffers it was composed from.
fn seal_page(
    s: &mut FillScratch,
    first_rank: u32,
    calls: u32,
    bind32: u32,
    page: &mut [u8; ZPAGE],
) -> Fill {
    let groups: Vec<PageGroup<'_>> = s
        .accepted
        .iter()
        .enumerate()
        .map(|(j, a)| PageGroup {
            count: a.count as u16,
            payload: &s.payloads[a.pay_at..a.pay_at + a.clen],
            ulen: a.ulen as u32,
            stored: a.stored,
            coded: a.coded,
            key: s.key_of(j),
        })
        .collect();
    let Ok(h) = encode_page(page, first_rank, &groups, bind32) else {
        return Fill::Mismatch;
    };
    if !page_decodes_to(page, bind32, &h, &s.page_imgs, &mut s.verify, s.own_decode) {
        return Fill::Mismatch;
    }
    let built = Filled {
        count: usize::from(h.count),
        ulen: h.ulen as usize,
        clen: s.accepted.iter().map(|a| a.clen).sum(),
        used: usize::from(h.used),
        groups: s.accepted.len(),
        stored: s.accepted.iter().filter(|a| a.stored).count(),
        calls,
    };
    s.stats.pages += 1;
    s.stats.records += built.count as u64;
    s.stats.groups += built.groups as u64;
    s.stats.stored += built.stored as u64;
    s.stats.payload_bytes += built.clen as u64;
    s.stats.used_bytes += built.used as u64;
    s.stats.group_bytes += built.ulen as u64;
    Fill::Page(built)
}

/// Whether `page`, read the way a reader reads one -- its CRC, its header
/// and directory, each group's payload decoded -- carries the header `hdr`
/// and decodes, group after group, to exactly `want`. What was written is
/// read back, not what was meant to be written.
fn page_decodes_to(
    page: &[u8; ZPAGE],
    bind32: u32,
    hdr: &PageHeader,
    want: &[u8],
    verify: &mut GroupBuf,
    own: bool,
) -> bool {
    let Ok(view) = parse_page(page, bind32) else {
        return false;
    };
    if view.header != *hdr {
        return false;
    }
    let mut at = 0usize;
    for j in 0..view.groups() {
        let Ok(g) = decode_group_with(&view, j, verify, own) else {
            return false;
        };
        if want.get(at..at + g.len()) != Some(g) {
            return false;
        }
        at += g.len();
    }
    at == want.len()
}

/// One fence entry as a page's first decode wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceEntry<'a> {
    /// Where the entry sits in the fence.
    pub index: usize,
    /// The rank of the first record it covers.
    pub first_rank: u32,
    /// How many records it covers: the next entry's first rank, or the
    /// segment's key count for the last, less this entry's.
    pub count: u32,
    /// The page that holds them, or [`ZNOPAGE`].
    pub page: u32,
    /// The key of the first record it covers.
    pub key: &'a [u8],
}

/// How a form file's fence is searched, decided at the attach: over the
/// keys, a binary search with a line of the offsets and a line of the key
/// blob a probe (the shape before); through a static B-tree of the
/// entries' words, eight a node and a cache line a level
/// (`Options::zpages_fence_words`, [`ZFence::index_words`]); or through the
/// binary tree over the words (`Options::zpages_fence_tree`,
/// [`ZFence::index`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceSearch {
    Keys,
    Words,
    Tree,
}

/// The fence: for each entry the rank and the key of the first record it
/// covers and the page that holds them, in rank order. Heap, not mapped: it
/// is resident from attach and never evicted, and a read searches it for the
/// page before it faults one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZFence {
    first_rank: Vec<u32>,
    page: Vec<u32>,
    /// `key_off[i]..key_off[i + 1]` is entry `i`'s key in `keys`.
    key_off: Vec<u32>,
    keys: Vec<u8>,
    /// The search tree over the entries' words ([`ZFence::index`]): the
    /// nodes of a balanced binary tree over the entries in key order, laid
    /// out breadth first -- node 1 the root, node `i`'s children `2i` and
    /// `2i + 1` -- so that a search reads one word a level and asks for the
    /// line that holds a node's great-grandchildren three levels before it
    /// reads one of them. Empty until `index` builds it, and emptied by a
    /// `push`.
    probe: Vec<ProbeLine>,
    /// Each node's entry: `probe_pos[i]` for node `i`.
    probe_pos: Vec<u32>,
    /// The entries' words in a static B-tree of eight a node
    /// ([`ZFence::index_words`]): node `k`'s words at `words[8k..8k + 8]`,
    /// its nine children the nodes `9k + 1..=9k + 9`, the words in key
    /// order through the tree, padded with `u64::MAX` past the last entry.
    /// Empty until `index_words` builds it, and emptied by a `push` or by
    /// `index`; `words_h` is the tree's height.
    words: Vec<u64>,
    words_h: u8,
    /// The bytes every non-empty key shares, and so where an entry's word
    /// begins in its key.
    shared: Vec<u8>,
}

/// Eight nodes of a fence's search tree, a line of them: node `i` sits at
/// `probe[i / 8].0[i % 8]`, so node `8i` begins a line and the line at index
/// `i` holds the eight great-grandchildren of node `i`.
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProbeLine([u64; 8]);

/// An entry's word: the eight bytes of `key` after the `shared` bytes every
/// non-empty key begins with, padded with zeros, as a big-endian integer.
/// The words of the entries ascend with their keys -- a tail that is a
/// prefix of a longer one pads to a smaller or equal word -- and two entries
/// tied on the word are told apart by the whole key.
fn fence_word(key: &[u8], shared: usize) -> u64 {
    let tail = key.get(shared..).unwrap_or(&[]);
    let mut w = [0u8; 8];
    let n = tail.len().min(8);
    w[..n].copy_from_slice(&tail[..n]);
    u64::from_be_bytes(w)
}

impl Default for ZFence {
    fn default() -> ZFence {
        ZFence::new()
    }
}

/// What the trailer records of the resident part it describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentFacts {
    /// Region B's length before its padding.
    pub res_len: u64,
    /// The key blob's length.
    pub keys_len: u64,
    /// The filter's offset in region B, or zero where there is none.
    pub filter_off: u64,
    /// Whole 64-byte blocks of filter, or zero.
    pub filter_blocks: u64,
    /// CRC-32C of region B's first `res_len` bytes.
    pub res_crc: u32,
}

impl ZFence {
    /// A fence of no entries.
    pub fn new() -> ZFence {
        ZFence {
            first_rank: Vec::new(),
            page: Vec::new(),
            key_off: vec![0],
            keys: Vec::new(),
            probe: Vec::new(),
            probe_pos: Vec::new(),
            words: Vec::new(),
            words_h: 0,
            shared: Vec::new(),
        }
    }

    /// Add an entry at the end. Its place is the caller's: the tiling is
    /// checked where a fence is read, not where it is built.
    pub fn push(&mut self, first_rank: u32, page: u32, key: &[u8]) -> Result<(), ZError> {
        let end = self
            .keys
            .len()
            .checked_add(key.len())
            .and_then(|e| u32::try_from(e).ok())
            .ok_or(ZError::Resident("the key blob passes four gibibytes"))?;
        self.first_rank.push(first_rank);
        self.page.push(page);
        self.keys.extend_from_slice(key);
        self.key_off.push(end);
        // The tree was of the entries before this one.
        self.probe.clear();
        self.probe_pos.clear();
        self.words.clear();
        self.words_h = 0;
        self.shared.clear();
        Ok(())
    }

    /// The search tree over the entries' words built, for `entry_of` to
    /// read where it stands; without it the search is over the keys, a pair
    /// of lines a probe. A reader's fence is indexed once at the attach
    /// under `Options::zpages_fence_tree`. The words are the eight bytes after
    /// the bytes every non-empty key shares, which are the bytes the first
    /// non-empty key and the last share, since the keys ascend; the empty
    /// key, the open lower fence, has no bytes to share and its word is
    /// zero, below every other.
    pub fn index(&mut self) {
        let n = self.len();
        self.probe.clear();
        self.probe_pos.clear();
        self.words.clear();
        self.words_h = 0;
        self.shared.clear();
        if n == 0 {
            return;
        }
        self.find_shared();
        let shared = self.shared.len();
        self.probe = vec![ProbeLine([0; 8]); (n + 1).div_ceil(8)];
        self.probe_pos = vec![0; n + 1];
        // The nodes filled in key order: an in-order walk of the tree, the
        // walk's depth the tree's height.
        fn fill(f: &mut ZFence, node: usize, n: usize, next: &mut usize, shared: usize) {
            if node > n {
                return;
            }
            fill(f, 2 * node, n, next, shared);
            let e = *next;
            *next += 1;
            let w = fence_word(f.key(e).expect("an entry has its key"), shared);
            f.probe[node / 8].0[node % 8] = w;
            f.probe_pos[node] = e as u32;
            fill(f, 2 * node + 1, n, next, shared);
        }
        let mut next = 0usize;
        fill(self, 1, n, &mut next, shared);
        debug_assert_eq!(next, n, "every entry has its node");
    }

    /// The bytes every non-empty key shares, found: the bytes the first
    /// non-empty key and the last share, since the keys ascend. For `index`
    /// and `index_words`.
    fn find_shared(&mut self) {
        let n = self.len();
        let first = (0..n).find(|&i| self.key(i).is_some_and(|k| !k.is_empty()));
        if let Some(f) = first {
            let a = self.key(f).expect("an entry has its key");
            let b = self.key(n - 1).expect("an entry has its key");
            let shared = a.iter().zip(b).take_while(|(x, y)| x == y).count();
            self.shared = a[..shared].to_vec();
        }
    }

    /// The entries' words in a static B-tree, for `entry_of` to search a
    /// cache line a level (`Options::zpages_fence_words`): eight words a
    /// node, nine children, the words as `index` takes them laid through
    /// the tree in key order -- node `k`'s children are nodes `9k + 1` to
    /// `9k + 9`, a child's whole subtree below the node word after it --
    /// and the slots past the last entry `u64::MAX`. The height is the
    /// least with room for every entry: four levels hold 6,560, which is a
    /// partition's fence at the suite's shape, where a binary search over
    /// the same words is thirteen dependent loads. Takes the binary tree
    /// down, as a push does: a fence is searched one way.
    pub fn index_words(&mut self) {
        self.probe.clear();
        self.probe_pos.clear();
        self.words.clear();
        self.words_h = 0;
        self.shared.clear();
        let n = self.len();
        if n == 0 {
            return;
        }
        self.find_shared();
        let shared = self.shared.len();
        let mut h = 1usize;
        let mut slots = 8usize;
        while slots < n {
            h += 1;
            slots = slots * 9 + 8;
        }
        let mut words = vec![u64::MAX; slots];
        // The in-order walk: each child's subtree before the node's word
        // after it, so the words ascend through the tree; a slot the walk
        // reaches past the last entry keeps the padding.
        #[allow(clippy::too_many_arguments)]
        fn fill(
            f: &ZFence,
            words: &mut [u64],
            k: usize,
            depth: usize,
            h: usize,
            next: &mut usize,
            n: usize,
            shared: usize,
        ) {
            if depth == h {
                return;
            }
            for j in 0..8 {
                fill(f, words, 9 * k + 1 + j, depth + 1, h, next, n, shared);
                if *next < n {
                    words[8 * k + j] =
                        fence_word(f.key(*next).expect("an entry has its key"), shared);
                }
                *next += 1;
            }
            fill(f, words, 9 * k + 9, depth + 1, h, next, n, shared);
        }
        let mut next = 0usize;
        fill(self, &mut words, 0, 0, h, &mut next, n, shared);
        debug_assert_eq!(next, slots, "every slot has its turn");
        self.words = words;
        self.words_h = h as u8;
    }

    /// Whether the word tree stands (`index_words`).
    pub fn worded(&self) -> bool {
        !self.words.is_empty()
    }

    /// The first entry whose word is above `w`, by the word tree: a line a
    /// level, each level's eight words compared at once for how many are
    /// at or below `w`, which is how many of the node's words and whole
    /// child subtrees lie below -- the rank gathers those -- and names the
    /// child to take. Four dependent loads for a partition's fence where a
    /// binary search over the same words makes thirteen, and the cost of
    /// the search was the latency of its chain: laid flat and searched
    /// with no branch on the data, the words took the same share of E's,
    /// C's and D's threads as the search over the keys (`docs/engine.md`).
    /// The padding past the last entry is `u64::MAX`, at or below nothing
    /// but a word of `u64::MAX`, whose answer is the length anyway.
    /// `len()` for none. With it, whether an entry ties `w` on the word:
    /// the greatest word at or below `w` is the last of the node words
    /// counted at some level, so a tie is seen in the descent, and where
    /// there is none the entry before the first above is at or below `key`
    /// by its word alone -- `entry_of` then reads no key at all, where it
    /// read the entry's key to find the same thing: a line of the offsets
    /// and a line of the key blob a search, whatever the search over the
    /// words had saved.
    fn above_by_word_wide(&self, w: u64) -> (usize, bool) {
        let ws = &self.words;
        let h = usize::from(self.words_h);
        let mut k = 0usize;
        let mut rank = 0usize;
        let mut tie = false;
        // A node word at or below `w` counts itself and the child subtree
        // before it: 9^(h - 1 - depth) entries, less one for the subtree
        // that is not there below a leaf.
        let mut per = 9usize.pow(h.saturating_sub(1) as u32);
        for _ in 0..h {
            let node = &ws[8 * k..8 * k + 8];
            let i = node.iter().filter(|&&x| x <= w).count();
            tie |= i > 0 && node[i - 1] == w;
            rank += i * per;
            k = 9 * k + 1 + i;
            per /= 9;
        }
        (rank.min(self.len()), tie)
    }

    /// Whether the tree stands (`index`).
    pub fn indexed(&self) -> bool {
        !self.probe.is_empty()
    }

    /// The first entry whose word is above `w`, by the tree, or `len()` for
    /// none: a level a step, the line a step three levels down will read
    /// asked for first, and the answer the node of the last step that went
    /// left.
    fn above_by_word(&self, w: u64) -> usize {
        let n = self.len();
        let mut i = 1usize;
        while i <= n {
            if let Some(line) = self.probe.get(i) {
                super::prefetch_lines(line.0.as_ptr() as *const u8, 64);
            }
            let node = self.probe[i / 8].0[i % 8];
            i = 2 * i + usize::from(node <= w);
        }
        let j = i >> (i.trailing_ones() + 1);
        if j == 0 {
            n
        } else {
            self.probe_pos[j] as usize
        }
    }

    /// Whether the first entry's key is empty -- the open lower fence, at or
    /// below every key -- as the answer for a key below every non-empty
    /// entry's.
    fn below_every_nonempty(&self) -> Option<usize> {
        (self.key(0) == Some(&[][..])).then_some(0)
    }

    /// How many entries.
    pub fn len(&self) -> usize {
        self.first_rank.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.first_rank.is_empty()
    }

    /// The key blob's length.
    pub fn keys_len(&self) -> usize {
        self.keys.len()
    }

    /// Entry `i`'s first rank.
    pub fn first_rank(&self, i: usize) -> Option<u32> {
        self.first_rank.get(i).copied()
    }

    /// The entry whose records rank `rank` falls among: the greatest whose
    /// first rank is at or below it, or `None` for an empty fence or a rank
    /// below the first entry's, which no fence this engine writes has (the
    /// tiling check holds the first entry at rank zero).
    pub fn entry_at_rank(&self, rank: u32) -> Option<usize> {
        self.first_rank
            .partition_point(|&r| r <= rank)
            .checked_sub(1)
    }

    /// Entry `i`'s page, or [`ZNOPAGE`].
    pub fn page(&self, i: usize) -> Option<u32> {
        self.page.get(i).copied()
    }

    /// Entry `i`'s key.
    pub fn key(&self, i: usize) -> Option<&[u8]> {
        let a = *self.key_off.get(i)? as usize;
        let b = *self.key_off.get(i + 1)? as usize;
        self.keys.get(a..b)
    }

    /// Entry `i` as a page's first decode wants it, for a segment of `nkeys`
    /// keys.
    pub fn entry(&self, i: usize, nkeys: u32) -> Option<FenceEntry<'_>> {
        let first_rank = self.first_rank(i)?;
        let next = self.first_rank(i + 1).unwrap_or(nkeys);
        Some(FenceEntry {
            index: i,
            first_rank,
            count: next.checked_sub(first_rank)?,
            page: self.page(i)?,
            key: self.key(i)?,
        })
    }

    /// The entry whose records `key` falls among: the greatest whose key is
    /// at or below `key`, or `None` where `key` sorts below the first
    /// entry's, which is a key the partition does not hold. A search over
    /// the fence's keys with the engine's key compare, in the heap and
    /// touching no page.
    ///
    /// An empty fence key is not handed to a byte compare, nor is an empty
    /// `key`: a slice held in a `Vec` that never allocated points at no
    /// page, and the library's compare reads through its first operand
    /// before it honours a zero length -- a failed page walk a call, 81 ns
    /// measured. An empty key sorts below every other, so neither needs
    /// one.
    ///
    /// Over a fence with its word tree (`index_words`,
    /// `Options::zpages_fence_words`) or indexed (`index`,
    /// `Options::zpages_fence_tree`) the search is by the entries' words,
    /// the word tree a line a level and the binary tree a word a level: a
    /// key that differs
    /// from the shared bytes is below or above every non-empty entry, a key
    /// shorter than them that agrees as far as it goes is below them all (a
    /// prefix sorts first), and a key that carries them is searched by its
    /// word, with the entries tied on the word compared whole from the top
    /// of the tie. The tree reads one line a level, asked for three levels
    /// ahead, where the search over the keys reads a line of the offsets
    /// and a line of the key blob a probe; it is the arm, since a
    /// partitioned store's fences are small enough to sit in a cache and
    /// the search over them was never miss-bound (the option's doc).
    pub fn entry_of(&self, key: &[u8]) -> Option<usize> {
        if self.probe.is_empty() && self.words.is_empty() {
            return self.entry_of_search(key);
        }
        if key.is_empty() {
            return self.below_every_nonempty();
        }
        let n = self.len();
        let shared = self.shared.len();
        let head = &key[..key.len().min(shared)];
        match head.cmp(&self.shared[..head.len()]) {
            Ordering::Less => return self.below_every_nonempty(),
            Ordering::Greater => return Some(n - 1),
            Ordering::Equal if key.len() < shared => return self.below_every_nonempty(),
            Ordering::Equal => {}
        }
        let w = fence_word(key, shared);
        let (mut e, tie) = if self.words.is_empty() {
            (self.above_by_word(w), true)
        } else {
            self.above_by_word_wide(w)
        };
        // No entry tied on the word: the one before the first above is at
        // or below by its word, and no key is read.
        if !tie {
            return e.checked_sub(1);
        }
        // Down from the first entry above by word: an entry below by word
        // is at or below without a compare, the empty key is at or below
        // everything, and an entry tied on the word is compared whole.
        while e > 0 {
            e -= 1;
            let k = self.key(e).expect("an entry has its key");
            if k.is_empty()
                || fence_word(k, shared) < w
                || super::key_cmp(k, key) != Ordering::Greater
            {
                return Some(e);
            }
        }
        None
    }

    /// [`entry_of`](ZFence::entry_of) by a binary search over the keys: the
    /// shape before the tree, the search of a fence not indexed, and the
    /// oracle the tree is held to.
    fn entry_of_search(&self, key: &[u8]) -> Option<usize> {
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // Every entry below `len` has a key: `push` writes the offset
            // with the key and `decode_resident` refuses offsets that do
            // not run from zero to the blob's length. A miss here is a bug
            // in the type, and a panic is the better failure than a key
            // answered as one below the first.
            let k = self.key(mid).expect("an entry has its key");
            let at_or_below = if k.is_empty() {
                true
            } else if key.is_empty() {
                false
            } else {
                super::key_cmp(k, key) != Ordering::Greater
            };
            if at_or_below {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.checked_sub(1)
    }

    /// [`entry_of`](ZFence::entry_of) for a key at or above the one entry
    /// `from` was found for -- a batch of keys in key order, each resolved
    /// from the entry before -- so a run of keys that share an entry costs a
    /// compare or two each where the search from the top costs the fence's
    /// depth in cold lines (`CLAUDE.md`, a batch resolved in arrival
    /// order): `from` itself where the key is still at or above its key and
    /// below the next entry's, else a gallop forward from `from` and a
    /// search over the span the gallop bracketed. A key below entry `from`'s,
    /// or a `from` past the last entry, is searched from the top. The same
    /// answer as `entry_of` for every key and every `from`.
    pub fn entry_of_from(&self, key: &[u8], from: usize) -> Option<usize> {
        let n = self.len();
        if from >= n || key.is_empty() {
            return self.entry_of(key);
        }
        // Whether entry `i`'s key is at or below `key`, `key` non-empty.
        let at_or_below = |i: usize| -> bool {
            let k = self.key(i).expect("an entry has its key");
            k.is_empty() || super::key_cmp(k, key) != Ordering::Greater
        };
        if !at_or_below(from) {
            return self.entry_of(key);
        }
        // `lo` is at or below the key; `hi` is the next candidate, probed
        // at doubling strides until one is above the key or the fence ends.
        let mut lo = from;
        let mut hi = from + 1;
        let mut step = 1usize;
        while hi < n && at_or_below(hi) {
            lo = hi;
            hi = hi.saturating_add(step).min(n);
            step = step.saturating_mul(2);
        }
        // The first entry above the key lies in `lo + 1..=hi`.
        let (mut a, mut b) = (lo + 1, hi);
        while a < b {
            let mid = a + (b - a) / 2;
            if at_or_below(mid) {
                a = mid + 1;
            } else {
                b = mid;
            }
        }
        Some(a - 1)
    }

    /// Heap bytes the fence holds: the four tables of region B, as they sit
    /// in memory, and the search tree where it stands.
    pub fn heap_bytes(&self) -> usize {
        4 * (self.first_rank.len() + self.page.len() + self.key_off.len())
            + self.keys.len()
            + std::mem::size_of::<ProbeLine>() * self.probe.len()
            + 4 * self.probe_pos.len()
            + 8 * self.words.len()
            + self.shared.len()
    }

    /// Region B, padded with zeros to a multiple of [`ZPAGE`], and what the
    /// trailer records of it. `filter` is the filter's blocks as bytes,
    /// little-endian words, whole 64-byte blocks, or empty for none.
    pub fn encode_resident(&self, filter: &[u8]) -> Result<(Vec<u8>, ResidentFacts), ZError> {
        if !filter.len().is_multiple_of(64) {
            return Err(ZError::Resident("the filter is not whole blocks"));
        }
        let e = self.len();
        let mut out = Vec::with_capacity(12 * e + 4 + self.keys.len() + filter.len() + ZPAGE);
        for w in self
            .first_rank
            .iter()
            .chain(&self.page)
            .chain(&self.key_off)
        {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&self.keys);
        let (filter_off, filter_blocks) = if filter.is_empty() {
            (0, 0)
        } else {
            out.resize(out.len().next_multiple_of(8), 0);
            let at = out.len();
            out.extend_from_slice(filter);
            (at as u64, (filter.len() / 64) as u64)
        };
        let res_len = out.len();
        let res_crc = crc32(&out);
        out.resize(res_len.next_multiple_of(ZPAGE), 0);
        Ok((
            out,
            ResidentFacts {
                res_len: res_len as u64,
                keys_len: self.keys.len() as u64,
                filter_off,
                filter_blocks,
                res_crc,
            },
        ))
    }

    /// The fence, and the filter's bytes, out of region B as stored: at least
    /// `t.res_len` bytes, padding allowed after. The checks, in order: the
    /// CRC the trailer holds; the regions the trailer's fields name, and
    /// that they end where the trailer says; the offsets of the key blob;
    /// and that the entries tile the partition ([`ZFence::check_tiling`]),
    /// which needs nothing but this region and is the one check of the fence
    /// that a CRC that passes does not make: a builder that wrote the entries
    /// wrong wrote them with a good CRC.
    pub fn decode_resident<'a>(res: &'a [u8], t: &Trailer) -> Result<(ZFence, &'a [u8]), ZError> {
        let bad = ZError::Resident;
        let res_len = usize::try_from(t.res_len).map_err(|_| ZError::Length)?;
        let region = res.get(..res_len).ok_or(ZError::Length)?;
        if crc32(region) != t.res_crc {
            return Err(ZError::ResidentCrc);
        }
        let e = usize::try_from(t.n_entries)
            .ok()
            .filter(|&e| u32::try_from(e).is_ok())
            .ok_or(bad("more entries than a u32 counts"))?;
        let keys_len = usize::try_from(t.keys_len)
            .ok()
            .filter(|&k| u32::try_from(k).is_ok())
            .ok_or(bad("a key blob over four gibibytes"))?;
        let b_end = e
            .checked_mul(12)
            .and_then(|b| b.checked_add(4))
            .and_then(|b| b.checked_add(keys_len))
            .ok_or(bad("the entries overflow"))?;
        let filter: &[u8] = if t.filter {
            let at = b_end.next_multiple_of(8);
            let blocks =
                usize::try_from(t.filter_blocks).map_err(|_| bad("the filter overflows"))?;
            let len = blocks.checked_mul(64).ok_or(bad("the filter overflows"))?;
            if blocks == 0 || t.filter_off != at as u64 || at.checked_add(len) != Some(res_len) {
                return Err(bad("the filter is not where the entries end"));
            }
            if region[b_end..at].iter().any(|&b| b != 0) {
                return Err(bad("the padding before the filter is not zero"));
            }
            &region[at..]
        } else {
            if t.filter_off != 0 || t.filter_blocks != 0 || res_len != b_end {
                return Err(bad("the region is not the entries alone"));
            }
            &[]
        };
        let words = |from: usize, n: usize| -> Vec<u32> {
            region[from..from + 4 * n]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w))
                .collect()
        };
        let key_off = words(8 * e, e + 1);
        if key_off[0] != 0
            || key_off[e] as usize != keys_len
            || key_off.windows(2).any(|w| w[0] > w[1])
        {
            return Err(bad(
                "the key offsets do not run from zero to the blob's length",
            ));
        }
        let fence = ZFence {
            first_rank: words(0, e),
            page: words(4 * e, e),
            key_off,
            keys: region[12 * e + 4..b_end].to_vec(),
            probe: Vec::new(),
            probe_pos: Vec::new(),
            words: Vec::new(),
            words_h: 0,
            shared: Vec::new(),
        };
        fence.check_tiling(t.nkeys, t.n_pages)?;
        Ok((fence, filter))
    }

    /// That the entries tile a partition of `nkeys` keys held by `n_pages`
    /// pages: the first begins at rank zero; first ranks and keys strictly
    /// increase and every first rank is a key's; every page number is below
    /// the pages, strictly increasing over the entries that have one, so each
    /// page is used once; and the pages are exactly the entries that have
    /// one.
    pub fn check_tiling(&self, nkeys: u64, n_pages: u64) -> Result<(), ZError> {
        let bad = ZError::Tiling;
        let e = self.len();
        if nkeys == 0 {
            return if e == 0 && n_pages == 0 {
                Ok(())
            } else {
                Err(bad("a partition of no keys has entries"))
            };
        }
        if e == 0 {
            return Err(bad("a partition of keys has no entries"));
        }
        if self.first_rank[0] != 0 {
            return Err(bad("the first entry does not begin at rank zero"));
        }
        if self.first_rank.windows(2).any(|w| w[0] >= w[1]) {
            return Err(bad("first ranks do not increase"));
        }
        if u64::from(self.first_rank[e - 1]) >= nkeys {
            return Err(bad("an entry begins past the last key"));
        }
        for i in 1..e {
            if self.key(i - 1) >= self.key(i) {
                return Err(bad("keys do not increase"));
            }
        }
        let mut last: Option<u32> = None;
        let mut held = 0u64;
        for &p in &self.page {
            if p == ZNOPAGE {
                continue;
            }
            if u64::from(p) >= n_pages {
                return Err(bad("a page number is past the pages"));
            }
            if last.is_some_and(|l| p <= l) {
                return Err(bad("page numbers do not increase"));
            }
            last = Some(p);
            held += 1;
        }
        if held != n_pages {
            return Err(bad("the pages are not exactly the entries that have one"));
        }
        Ok(())
    }
}

/// The hash a form file's pages are bound by: CRC-32C over the segment's id,
/// the sequence its name covers, its key count and the CRC of its key
/// section's checksum row, twenty-eight bytes little-endian. Every page's
/// CRC is xored with it, so a page that is another form file's fails its own
/// check at no cost.
pub fn bind32(seg_id: u64, seg_end_seq: u64, nkeys: u64, seg_row_crc: u32) -> u32 {
    let mut b = [0u8; 28];
    b[0..8].copy_from_slice(&seg_id.to_le_bytes());
    b[8..16].copy_from_slice(&seg_end_seq.to_le_bytes());
    b[16..24].copy_from_slice(&nkeys.to_le_bytes());
    b[24..28].copy_from_slice(&seg_row_crc.to_le_bytes());
    crc32(&b)
}

/// The bytes of the filter a form file stores for a partition of `nkeys` keys:
/// the blocks `BlockedBloom` allocates for that many keys, which is the size
/// of the filter at build and at attach alike. A tool that prices the
/// resident part asks here and does not restate the filter's bits a key and
/// its block, so that its model follows a change to the filter.
pub fn filter_bytes(nkeys: usize) -> usize {
    super::BlockedBloom::blocks_for(nkeys) * std::mem::size_of::<[u64; 8]>()
}

/// That a page's header is its fence entry's: the rank of its first record
/// and the records it holds. A page that passes its CRC and is some other
/// page's, or is the right page rewritten, is told apart here and by
/// nothing earlier.
///
/// One of the two checks a first decode makes beyond the page's own
/// ([`check_group`] is the other), and the reader's first decode, the
/// builder's verification of what it wrote ([`verify_page`]) and an attach's
/// look at the pages it samples all call it, so that the three cannot come to
/// mean different things by it.
pub fn check_entry(view: &PageView<'_>, entry: &FenceEntry<'_>) -> Result<(), ZError> {
    let h = &view.header;
    if h.first_rank != entry.first_rank || u32::from(h.count) != entry.count {
        return Err(ZError::Fence);
    }
    // The directory's keys ascend among themselves (the view's check);
    // the fence's key, the first group's, is below them.
    if view
        .key(1)
        .is_some_and(|k| key_cmp(entry.key, k) != Ordering::Less)
    {
        return Err(ZError::Fence);
    }
    Ok(())
}

/// That a decoded group `j` is what its page's directory and its fence
/// entry say: the group's records, whole, in key order, the first the
/// entry's key for group 0 and the directory's for the rest, every key
/// below the next group's first ([`walk_group`]). See [`check_entry`] for
/// why it is a function of its own.
pub fn check_group(
    group: &[u8],
    view: &PageView<'_>,
    j: usize,
    entry: &FenceEntry<'_>,
) -> Result<(), ZError> {
    let g = view
        .group(j)
        .ok_or(ZError::Header("no such group in the page"))?;
    let first = if j == 0 { Some(entry.key) } else { view.key(j) };
    walk_group(group, usize::from(g.count), first, view.key(j + 1))
}

/// Everything a first decode of a page checks, in the order that trusts the
/// least first: the page's CRC, tag, header and directory; its `first_rank`
/// and `count` against its fence entry ([`check_entry`]); and for each
/// group the payload decoded to exactly its `ulen` and the structural walk
/// -- its records, whole, ascending, the first the key named for it and
/// every one below the next group's ([`check_group`]). The builder runs this
/// on every page it writes, as the page will be read, so that a defect that
/// would fail every first decode cannot pass the build and then loop the
/// rebuild; the reader's first decode of a group is these checks in this
/// order over the page and that group, with the one difference its mapping
/// makes (`db::zfile::first_decode`).
pub fn verify_page(
    page: &[u8],
    bind32: u32,
    entry: &FenceEntry<'_>,
    buf: &mut GroupBuf,
) -> Result<PageHeader, ZError> {
    verify_page_with(page, bind32, entry, buf, false)
}

/// [`verify_page`] through the decoder the readers will take
/// ([`decode_group_with`]).
pub fn verify_page_with(
    page: &[u8],
    bind32: u32,
    entry: &FenceEntry<'_>,
    buf: &mut GroupBuf,
    own: bool,
) -> Result<PageHeader, ZError> {
    let view = parse_page(page, bind32)?;
    check_entry(&view, entry)?;
    for j in 0..view.groups() {
        let group = decode_group_with(&view, j, buf, own)?;
        check_group(group, &view, j, entry)?;
    }
    Ok(view.header)
}

// Every trailer's head and its last word, whichever body it describes: the
// magic naming the body, that body's layout version, the page size, and
// the CRC of every byte before the CRC. Written by `seal_trailer` and
// checked by `open_trailer` alone, since a reader of either kind refusing
// the other at the magic rests on the two kinds keeping these at one set
// of places, and an attach that dispatches on the magic reads it there.
const T_MAGIC: usize = 0;
const T_VERSION: usize = 8;
const T_PAGE_SIZE: usize = 12;
const T_CRC: usize = ZTRAILER - 4;
/// The head's length: where a body's fields begin.
const T_HEAD: usize = 16;

// The block form's fields.
const T_N_PAGES: usize = T_HEAD;
const T_N_ENTRIES: usize = 24;
const T_NKEYS: usize = 32;
const T_RES_OFF: usize = 40;
const T_RES_LEN: usize = 48;
const T_KEYS_LEN: usize = 56;
const T_FILTER_OFF: usize = 64;
const T_FILTER_BLOCKS: usize = 72;
const T_RES_CRC: usize = 80;
const T_FLAGS: usize = 84;
const T_TOTAL_ULEN: usize = 88;
const T_SEG_RECS_LEN: usize = 96;
const T_SEG_CRC_OFF: usize = 104;
const T_SEG_KEY_OFF: usize = 112;
const T_SEG_HASH_CAP: usize = 120;
const T_SEG_FILE_LEN: usize = 128;
const T_SEG_TIMESTAMP: usize = 136;
const T_SEG_GENERATION: usize = 144;
const T_SEG_ROW_CRC: usize = 152;
const T_REC_VERSION: usize = 156;
const T_REC_MAGIC: usize = 160;
const T_PAGE_CAP: usize = 168;
const T_MAX_COUNT: usize = 172;
const T_SEG_ID: usize = 176;
const T_SEG_END_SEQ: usize = 184;
const T_BIND32: usize = 192;
const T_BLOOM_PROBES: usize = 196;
const T_BLOOM_SIG: usize = 200;
const T_BLOOM_BITS: usize = 208;
/// Where the zeros begin, which run to the CRC.
const T_ZERO: usize = 212;

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn get32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn get64(b: &[u8], at: usize) -> u64 {
    let mut w = [0u8; 8];
    w.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(w)
}

/// A trailer's head and CRC written over `b`, whose body's fields are in
/// place: the magic naming the body and its version, the page size, and
/// then the CRC, last since it covers the head as well.
fn seal_trailer(b: &mut [u8; ZTRAILER], magic: &[u8; 8], version: u32) {
    b[T_MAGIC..T_MAGIC + 8].copy_from_slice(magic);
    put32(b, T_VERSION, version);
    put32(b, T_PAGE_SIZE, ZPAGE as u32);
    let crc = crc32(&b[..T_CRC]);
    put32(b, T_CRC, crc);
}

/// The magic of the trailer page `bytes`, which says which body the file
/// holds ([`ZMAGIC`], [`STRIDE_MAGIC`]): read by an attach before any
/// other byte of the page, at the one place every kind's head puts it.
/// `None` for a page shorter than a trailer.
pub fn trailer_magic(bytes: &[u8]) -> Option<[u8; 8]> {
    if bytes.len() != ZTRAILER {
        return None;
    }
    bytes[T_MAGIC..T_MAGIC + 8].try_into().ok()
}

/// The CRC word the trailer page `bytes` ends in, every kind's at one place
/// ([`seal_trailer`]): over every byte of the page before it, the trailer's
/// own fields and the row's CRC among them for a stride trailer, so two
/// trailers whose words agree are, to a CRC's strength, one trailer. `None`
/// for a page shorter than a trailer.
pub fn trailer_crc(bytes: &[u8]) -> Option<u32> {
    (bytes.len() == ZTRAILER).then(|| get32(bytes, T_CRC))
}

/// The checks a trailer of the body `magic` names makes before it reads a
/// field of the body, in order: the length, the magic, the version, the
/// page size, the CRC. The cheap identifying checks come first, so a page
/// that is no trailer of this kind -- another kind's among them -- is said
/// to be that, and the CRC says the rest was torn.
fn open_trailer(bytes: &[u8], magic: &[u8; 8], version: u32) -> Result<(), ZError> {
    if bytes.len() != ZTRAILER {
        return Err(ZError::Length);
    }
    if bytes[T_MAGIC..T_MAGIC + 8] != *magic {
        return Err(ZError::Magic);
    }
    if get32(bytes, T_VERSION) != version {
        return Err(ZError::Version);
    }
    if get32(bytes, T_PAGE_SIZE) != ZPAGE as u32 {
        return Err(ZError::PageSize);
    }
    if get32(bytes, T_CRC) != crc32(&bytes[..T_CRC]) {
        return Err(ZError::TrailerCrc);
    }
    Ok(())
}

/// The trailer, the file's last page: what the form file holds, the segment it
/// derives from as that segment says of itself, and the filter's
/// parameters. The magic, the version, the page size, the page's capacity
/// and the most records in a group are constants of the layout and are not
/// fields here; `bind32` and the CRC are derived.
///
/// A form file holds values, so it is bound to its segment by content and not
/// by a name: the engine hands segment ids out again after a crash, and the
/// ordered index binds to its segment by key count alone, which is enough for
/// heads and not for a file that carries values. The segment's key-section
/// checksum row is the strong part -- four bytes per 16 KiB are a fingerprint
/// of every byte the records live in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trailer {
    /// Pages in region A.
    pub n_pages: u64,
    /// Fence entries.
    pub n_entries: u64,
    /// The segment's key count.
    pub nkeys: u64,
    /// Region B's offset, which is `4096 * n_pages`.
    pub res_off: u64,
    /// Region B's length before its padding.
    pub res_len: u64,
    /// The key blob's length.
    pub keys_len: u64,
    /// The filter's offset in region B, or zero where there is none.
    pub filter_off: u64,
    /// Whole 64-byte blocks of filter, or zero.
    pub filter_blocks: u64,
    /// CRC-32C of region B's first `res_len` bytes.
    pub res_crc: u32,
    /// A filter is stored in region B.
    pub filter: bool,
    /// The decoded bytes of every group, summed, for the ratio.
    pub total_ulen: u64,
    /// The segment's record region length.
    pub seg_recs_len: u64,
    /// The segment's key section: where its checksum row is.
    pub seg_crc_off: u64,
    /// The segment's key section offset.
    pub seg_key_off: u64,
    /// The segment's hash capacity.
    pub seg_hash_cap: u64,
    /// The segment's file length.
    pub seg_file_len: u64,
    /// The segment's superblock timestamp.
    pub seg_timestamp: u64,
    /// The segment's superblock generation.
    pub seg_generation: u64,
    /// CRC-32C of the segment's key-section checksum row.
    pub seg_row_crc: u32,
    /// The record format's version the records copied are in.
    pub rec_version: u32,
    /// The record format's magic.
    pub rec_magic: u64,
    /// The segment's name fields: its id, and the sequence it covers.
    pub seg_id: u64,
    pub seg_end_seq: u64,
    /// The filter's probes, the signature of the code that fills it, and its
    /// bits a key. The signature is the one that decides whether the words
    /// stored are the words this engine would compute.
    pub bloom_probes: u32,
    pub bloom_sig: u64,
    pub bloom_bits_per_key: u32,
}

impl Trailer {
    /// The hash this form file's pages are bound by ([`bind32`]).
    pub fn bind32(&self) -> u32 {
        self.seg_ident().bind32()
    }

    /// The segment this form file says it derives from, in the shape both
    /// kinds of trailer record it, so that one comparison holds either to
    /// the live segment (`zfile::same_segment`).
    pub fn seg_ident(&self) -> SegIdent {
        SegIdent {
            seg_id: self.seg_id,
            seg_end_seq: self.seg_end_seq,
            nkeys: self.nkeys,
            recs_len: self.seg_recs_len,
            crc_off: self.seg_crc_off,
            key_off: self.seg_key_off,
            hash_cap: self.seg_hash_cap,
            file_len: self.seg_file_len,
            timestamp: self.seg_timestamp,
            generation: self.seg_generation,
            row_crc: self.seg_row_crc,
            rec_version: self.rec_version,
            rec_magic: self.rec_magic,
        }
    }

    /// The segment's fields set from `s`, the inverse of
    /// [`Trailer::seg_ident`]: what a writer records of the segment it
    /// derives from, from the one account of it both kinds of trailer are
    /// filled from (`zfile::seg_ident_of`), so that no writer spells the
    /// segment's fields out again. The key count is the segment's.
    pub fn set_seg(&mut self, s: &SegIdent) {
        self.seg_id = s.seg_id;
        self.seg_end_seq = s.seg_end_seq;
        self.nkeys = s.nkeys;
        self.seg_recs_len = s.recs_len;
        self.seg_crc_off = s.crc_off;
        self.seg_key_off = s.key_off;
        self.seg_hash_cap = s.hash_cap;
        self.seg_file_len = s.file_len;
        self.seg_timestamp = s.timestamp;
        self.seg_generation = s.generation;
        self.seg_row_crc = s.row_crc;
        self.rec_version = s.rec_version;
        self.rec_magic = s.rec_magic;
    }

    /// Whether the stored filter is the one this engine would compute: its
    /// signature, probes and bits a key are the running engine's. A filter
    /// answers "absent" for a key it does not hold, so one whose bits are
    /// not this engine's would answer it for keys that are present, and
    /// nothing downstream would see it.
    pub fn filter_matches_engine(&self) -> bool {
        self.filter
            && self.bloom_sig == super::BLOOM_SIG
            && self.bloom_probes == super::BLOOM_PROBES
            && self.bloom_bits_per_key == super::BLOOM_BITS_PER_KEY
    }

    /// The length the file must have: region A, region B padded, the trailer.
    /// `None` where the fields overflow.
    pub fn expected_file_len(&self) -> Option<u64> {
        let padded = self.res_len.checked_next_multiple_of(ZPAGE as u64)?;
        self.res_off
            .checked_add(padded)?
            .checked_add(ZTRAILER as u64)
    }

    /// The trailer's bytes.
    pub fn encode(&self) -> [u8; ZTRAILER] {
        let mut b = [0u8; ZTRAILER];
        put64(&mut b, T_N_PAGES, self.n_pages);
        put64(&mut b, T_N_ENTRIES, self.n_entries);
        put64(&mut b, T_NKEYS, self.nkeys);
        put64(&mut b, T_RES_OFF, self.res_off);
        put64(&mut b, T_RES_LEN, self.res_len);
        put64(&mut b, T_KEYS_LEN, self.keys_len);
        put64(&mut b, T_FILTER_OFF, self.filter_off);
        put64(&mut b, T_FILTER_BLOCKS, self.filter_blocks);
        put32(&mut b, T_RES_CRC, self.res_crc);
        put32(&mut b, T_FLAGS, if self.filter { ZT_FILTER } else { 0 });
        put64(&mut b, T_TOTAL_ULEN, self.total_ulen);
        put64(&mut b, T_SEG_RECS_LEN, self.seg_recs_len);
        put64(&mut b, T_SEG_CRC_OFF, self.seg_crc_off);
        put64(&mut b, T_SEG_KEY_OFF, self.seg_key_off);
        put64(&mut b, T_SEG_HASH_CAP, self.seg_hash_cap);
        put64(&mut b, T_SEG_FILE_LEN, self.seg_file_len);
        put64(&mut b, T_SEG_TIMESTAMP, self.seg_timestamp);
        put64(&mut b, T_SEG_GENERATION, self.seg_generation);
        put32(&mut b, T_SEG_ROW_CRC, self.seg_row_crc);
        put32(&mut b, T_REC_VERSION, self.rec_version);
        put64(&mut b, T_REC_MAGIC, self.rec_magic);
        put32(&mut b, T_PAGE_CAP, ZCAP as u32);
        put32(&mut b, T_MAX_COUNT, ZMAX_COUNT as u32);
        put64(&mut b, T_SEG_ID, self.seg_id);
        put64(&mut b, T_SEG_END_SEQ, self.seg_end_seq);
        put32(&mut b, T_BIND32, self.bind32());
        put32(&mut b, T_BLOOM_PROBES, self.bloom_probes);
        put64(&mut b, T_BLOOM_SIG, self.bloom_sig);
        put32(&mut b, T_BLOOM_BITS, self.bloom_bits_per_key);
        seal_trailer(&mut b, &ZMAGIC, ZVERSION);
        b
    }

    /// The trailer out of a page, after every check that needs nothing of the
    /// file: the head's -- the length, the magic, the version, the page size,
    /// the CRC ([`open_trailer`]) -- and then the constants, the flags, the
    /// zeros, `bind32` against the fields it hashes, and that the fields do
    /// not contradict each other.
    pub fn decode(bytes: &[u8]) -> Result<Trailer, ZError> {
        open_trailer(bytes, &ZMAGIC, ZVERSION)?;
        let bad = ZError::Trailer;
        if get32(bytes, T_PAGE_CAP) != ZCAP as u32 || get32(bytes, T_MAX_COUNT) != ZMAX_COUNT as u32
        {
            return Err(bad(
                "the page capacity or the group size is not this layout's",
            ));
        }
        let flags = get32(bytes, T_FLAGS);
        if flags & !ZT_FILTER != 0 {
            return Err(bad("a flag bit this reader does not know is set"));
        }
        if bytes[T_ZERO..T_CRC].iter().any(|&b| b != 0) {
            return Err(bad("a reserved byte is not zero"));
        }
        let t = Trailer {
            n_pages: get64(bytes, T_N_PAGES),
            n_entries: get64(bytes, T_N_ENTRIES),
            nkeys: get64(bytes, T_NKEYS),
            res_off: get64(bytes, T_RES_OFF),
            res_len: get64(bytes, T_RES_LEN),
            keys_len: get64(bytes, T_KEYS_LEN),
            filter_off: get64(bytes, T_FILTER_OFF),
            filter_blocks: get64(bytes, T_FILTER_BLOCKS),
            res_crc: get32(bytes, T_RES_CRC),
            filter: flags & ZT_FILTER != 0,
            total_ulen: get64(bytes, T_TOTAL_ULEN),
            seg_recs_len: get64(bytes, T_SEG_RECS_LEN),
            seg_crc_off: get64(bytes, T_SEG_CRC_OFF),
            seg_key_off: get64(bytes, T_SEG_KEY_OFF),
            seg_hash_cap: get64(bytes, T_SEG_HASH_CAP),
            seg_file_len: get64(bytes, T_SEG_FILE_LEN),
            seg_timestamp: get64(bytes, T_SEG_TIMESTAMP),
            seg_generation: get64(bytes, T_SEG_GENERATION),
            seg_row_crc: get32(bytes, T_SEG_ROW_CRC),
            rec_version: get32(bytes, T_REC_VERSION),
            rec_magic: get64(bytes, T_REC_MAGIC),
            seg_id: get64(bytes, T_SEG_ID),
            seg_end_seq: get64(bytes, T_SEG_END_SEQ),
            bloom_probes: get32(bytes, T_BLOOM_PROBES),
            bloom_sig: get64(bytes, T_BLOOM_SIG),
            bloom_bits_per_key: get32(bytes, T_BLOOM_BITS),
        };
        if get32(bytes, T_BIND32) != t.bind32() {
            return Err(bad(
                "bind32 is not the hash of the segment's identity fields",
            ));
        }
        if t.n_pages.checked_mul(ZPAGE as u64) != Some(t.res_off) {
            return Err(bad("res_off is not the pages' length"));
        }
        if t.n_pages > t.n_entries || t.n_entries > t.nkeys {
            return Err(bad("entries and pages do not nest in the keys"));
        }
        if t.n_entries > u64::from(u32::MAX) || t.keys_len > u64::from(u32::MAX) {
            return Err(bad("the fence passes what its u32 offsets count"));
        }
        if t.filter != (t.filter_blocks > 0) {
            return Err(bad("the filter flag and the filter's blocks disagree"));
        }
        if t.filter && (t.bloom_probes == 0 || t.bloom_bits_per_key == 0) {
            return Err(bad("a filter with no probes or no bits"));
        }
        Ok(t)
    }
}

/// The segment a form file derives from, as the segment says of itself:
/// what either kind of trailer records of it and what the attach holds to
/// the live segment's own account (`zfile::same_segment`), field by field.
/// One shape for both kinds, so that the comparison and its refusals are
/// written once; a second copy of it for the stride form would be a second
/// definition of what "this segment's file" means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SegIdent {
    /// The segment's name fields: its id, and the sequence it covers.
    pub seg_id: u64,
    pub seg_end_seq: u64,
    /// Its key count.
    pub nkeys: u64,
    /// Its record region's length.
    pub recs_len: u64,
    /// Where its key section's checksum row is.
    pub crc_off: u64,
    /// Its key section's offset.
    pub key_off: u64,
    /// Its hash capacity.
    pub hash_cap: u64,
    /// Its file's length.
    pub file_len: u64,
    /// Its superblock's timestamp and generation.
    pub timestamp: u64,
    pub generation: u64,
    /// CRC-32C of its key section's checksum row.
    pub row_crc: u32,
    /// The record format's version and the file format's magic its records
    /// are laid out by.
    pub rec_version: u32,
    pub rec_magic: u64,
}

// A `SegIdent`'s fields in a stride trailer, from its start.
const SI_SEG_ID: usize = 0;
const SI_SEG_END_SEQ: usize = 8;
const SI_NKEYS: usize = 16;
const SI_RECS_LEN: usize = 24;
const SI_CRC_OFF: usize = 32;
const SI_KEY_OFF: usize = 40;
const SI_HASH_CAP: usize = 48;
const SI_FILE_LEN: usize = 56;
const SI_TIMESTAMP: usize = 64;
const SI_GENERATION: usize = 72;
const SI_ROW_CRC: usize = 80;
const SI_REC_VERSION: usize = 84;
const SI_REC_MAGIC: usize = 88;
const SI_LEN: usize = 96;

impl SegIdent {
    /// The hash a form file's pages are bound by ([`bind32`]) over the
    /// fields it takes.
    pub fn bind32(&self) -> u32 {
        bind32(self.seg_id, self.seg_end_seq, self.nkeys, self.row_crc)
    }

    fn put(&self, b: &mut [u8]) {
        put64(b, SI_SEG_ID, self.seg_id);
        put64(b, SI_SEG_END_SEQ, self.seg_end_seq);
        put64(b, SI_NKEYS, self.nkeys);
        put64(b, SI_RECS_LEN, self.recs_len);
        put64(b, SI_CRC_OFF, self.crc_off);
        put64(b, SI_KEY_OFF, self.key_off);
        put64(b, SI_HASH_CAP, self.hash_cap);
        put64(b, SI_FILE_LEN, self.file_len);
        put64(b, SI_TIMESTAMP, self.timestamp);
        put64(b, SI_GENERATION, self.generation);
        put32(b, SI_ROW_CRC, self.row_crc);
        put32(b, SI_REC_VERSION, self.rec_version);
        put64(b, SI_REC_MAGIC, self.rec_magic);
    }

    fn get(b: &[u8]) -> SegIdent {
        SegIdent {
            seg_id: get64(b, SI_SEG_ID),
            seg_end_seq: get64(b, SI_SEG_END_SEQ),
            nkeys: get64(b, SI_NKEYS),
            recs_len: get64(b, SI_RECS_LEN),
            crc_off: get64(b, SI_CRC_OFF),
            key_off: get64(b, SI_KEY_OFF),
            hash_cap: get64(b, SI_HASH_CAP),
            file_len: get64(b, SI_FILE_LEN),
            timestamp: get64(b, SI_TIMESTAMP),
            generation: get64(b, SI_GENERATION),
            row_crc: get32(b, SI_ROW_CRC),
            rec_version: get32(b, SI_REC_VERSION),
            rec_magic: get64(b, SI_REC_MAGIC),
        }
    }
}

// ---- the stride form ------------------------------------------------------

/// The stride trailer's magic: which body the file's last page describes,
/// read before anything else of it. Not [`ZMAGIC`] moved, since the block
/// form's layout is unchanged: a reader of the block form alone takes this
/// file's trailer at the same length and refuses it at the magic, as
/// damage, and rebuilds -- refused, never misread, which is all a derived
/// file needs of an old reader. The digit moves when a stride reader would
/// misread a newer stride file rather than refuse it.
pub const STRIDE_MAGIC: [u8; 8] = *b"SUPDZST1";
/// The stride trailer's version: what an older stride reader refuses
/// without misreading.
pub const STRIDE_VERSION: u32 = 1;
/// The longest key a stride form takes, and the buffer a key renders into
/// on the stack of every read that renders one. The prefix is at most a
/// byte shorter, the field being at least a byte, and the trailer keeps
/// this many bytes for it.
pub const STRIDE_KEY_MAX: usize = 64;
/// The most decimal digits a field holds: every number of nineteen digits
/// is below 2^64, and not every one of twenty is.
pub const STRIDE_DECIMAL_MAX: usize = 19;
/// The most big-endian bytes a field holds: a u64's.
pub const STRIDE_BE_MAX: usize = 8;
/// The most hex digits a field holds: a u64's.
pub const STRIDE_HEX_MAX: usize = 16;
/// The version of a stride trailer whose file holds its keys' numbers
/// beside the values (the sparse form): what a reader of the first version
/// refuses without misreading, as it refuses any version it does not know.
/// A file whose keys are a whole run is written at [`STRIDE_VERSION`]
/// still, byte for byte what it was.
pub const STRIDE_VERSION_SPARSE: u32 = 2;
/// The ranks between two of a sparse stride form's samples: every
/// sixty-fourth number is a sample, so the run a sample opens is 512 bytes
/// of numbers, eight cache lines a search asks for together -- the ordered
/// index's stride, for its reason (`ordindex::TOP_STRIDE`).
pub const STRIDE_SAMPLE: u64 = 64;
/// The widest value a stride form holds: a page, so that a slot spans at
/// most two pages and a read checks at most two of the row's words.
pub const STRIDE_W_MAX: u32 = ZPAGE as u32;
/// The multiplier a value page's index is mixed into its word of the row
/// by, the golden ratio's 32-bit word. A block page names its first rank in
/// its header and the fence holds it to that; a value page names nothing of
/// its own, so its word is bound to its index as well as to the file, and a
/// word moved with its page to another index -- a row and its pages
/// shifted together -- fails there.
pub const STRIDE_PAGE_MIX: u32 = 0x9E37_79B9;

/// Value page `p`'s word in a stride form's CRC row: CRC-32C of the page,
/// xor the file's [`bind32`], xor the index mixed by [`STRIDE_PAGE_MIX`].
/// The one definition: the builder fills the row by it and every read
/// checks a page by it. An index is a u32 because a stride trailer's key
/// count is ([`StrideTrailer::decode`]) and a value is at most a page, so
/// there are no more value pages than keys.
pub fn stride_page_crc(page: &[u8], bind: u32, p: u32) -> u32 {
    crc32(page) ^ bind ^ p.wrapping_mul(STRIDE_PAGE_MIX)
}

/// The CRC a stride trailer records of the row: CRC-32C of the row's
/// words, `res_len` bytes and not their padding, xor the file's
/// [`bind32`], so a row copied whole from another form file fails it.
pub fn stride_row_crc(row: &[u8], bind: u32) -> u32 {
    crc32(row) ^ bind
}

/// A stride form's sizes ([`stride_layout`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrideLayout {
    /// Pages of values, `V`.
    pub v_pages: u64,
    /// The CRC row's offset, `4096 * V`.
    pub res_off: u64,
    /// The CRC row's bytes, a word a value page.
    pub res_len: u64,
    /// The row's pages, padded to a whole page.
    pub res_pages: u64,
    /// The file's length: the values, the row padded, the trailer.
    pub file_len: u64,
    /// Pages of the keys' numbers, `K`, after the values: none in a form
    /// whose keys are a whole run, which holds no number.
    pub k_pages: u64,
    /// The samples of the numbers, every [`STRIDE_SAMPLE`]th: none in a
    /// form whose keys are a whole run.
    pub samples: u64,
}

impl StrideLayout {
    /// The pages the row has a word for: the values' and the numbers'.
    pub fn row_pages(&self) -> u64 {
        self.v_pages + self.k_pages
    }

    /// The row's own bytes, a word a page it covers; in a sparse form the
    /// samples follow it at the next multiple of eight
    /// ([`StrideLayout::samples_off`]).
    pub fn row_len(&self) -> u64 {
        self.row_pages() * 4
    }

    /// Where the numbers begin, the values' end.
    pub fn k_off(&self) -> u64 {
        self.v_pages * ZPAGE as u64
    }

    /// Where the samples begin in the file: the row's end, to a multiple
    /// of eight.
    pub fn samples_off(&self) -> u64 {
        self.res_off + self.row_len().next_multiple_of(8)
    }
}

/// The sizes of a stride form of `n` values of `w` bytes each: the one
/// definition of them, which the builder writes by, the trailer is held
/// to, and a precheck prices a conversion with. `None` where they overflow.
pub fn stride_layout(n: u64, w: u32) -> Option<StrideLayout> {
    let page = ZPAGE as u64;
    let v_pages = n.checked_mul(u64::from(w))?.div_ceil(page);
    let res_off = v_pages.checked_mul(page)?;
    let res_len = v_pages.checked_mul(4)?;
    let res_pages = res_len.div_ceil(page);
    let file_len = v_pages
        .checked_add(res_pages)?
        .checked_add(1)?
        .checked_mul(page)?;
    Some(StrideLayout {
        v_pages,
        res_off,
        res_len,
        res_pages,
        file_len,
        k_pages: 0,
        samples: 0,
    })
}

/// The sizes of a sparse stride form of `n` values of `w` bytes each: the
/// values as [`stride_layout`] has them, then the keys' numbers, a u64 a
/// rank, to a whole page; then the resident part, the row with a word for
/// every value page and every number page, zeros to a multiple of eight,
/// and the samples, a u64 for every [`STRIDE_SAMPLE`]th rank, to a whole
/// page; then the trailer. `res_len` is the row, its padding and the
/// samples, which the trailer's CRC covers together. `None` where they
/// overflow.
pub fn stride_layout_sparse(n: u64, w: u32) -> Option<StrideLayout> {
    let page = ZPAGE as u64;
    let v_pages = n.checked_mul(u64::from(w))?.div_ceil(page);
    let k_pages = n.checked_mul(8)?.div_ceil(page);
    let samples = n.div_ceil(STRIDE_SAMPLE);
    let row_pages = v_pages.checked_add(k_pages)?;
    let res_off = row_pages.checked_mul(page)?;
    let res_len = row_pages
        .checked_mul(4)?
        .next_multiple_of(8)
        .checked_add(samples.checked_mul(8)?)?;
    let res_pages = res_len.div_ceil(page);
    let file_len = row_pages
        .checked_add(res_pages)?
        .checked_add(1)?
        .checked_mul(page)?;
    Some(StrideLayout {
        v_pages,
        res_off,
        res_len,
        res_pages,
        file_len,
        k_pages,
        samples,
    })
}

/// Where the straight line through a sparse form's first and last number
/// puts number `x` among its `m` samples: `0` at `lo`, `m` at `hi`, and
/// in proportion between, `x` held inside the two. The one definition of
/// the guess a search starts from, which the builder measures the error of
/// against every sample ([`StrideTrailer::sparse`]) and a reader searches
/// within that error of.
#[inline]
pub fn stride_line(lo: u64, hi: u64, m: u64, x: u64) -> u64 {
    let x = x.clamp(lo, hi);
    let span = hi - lo;
    if span == 0 {
        return 0;
    }
    ((u128::from(x - lo) * u128::from(m)) / u128::from(span)) as u64
}

/// How a stride form's key field holds its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// Zero-padded ASCII decimal digits.
    Decimal,
    /// Big-endian unsigned bytes.
    BigEndian,
    /// Zero-padded ASCII hex digits, the letters lowercase.
    HexLower,
    /// Zero-padded ASCII hex digits, the letters uppercase.
    HexUpper,
}

/// The kinds a detection tries, narrowest alphabet first: a field of
/// decimal digits is one of hex digits too, and of bytes, and the first
/// kind that holds a run is the one whose numbers are closest together.
pub const KEY_KINDS: [KeyKind; 4] = [
    KeyKind::Decimal,
    KeyKind::HexLower,
    KeyKind::HexUpper,
    KeyKind::BigEndian,
];
/// The kinds every stride reader knows, which a detection is held to
/// where the sparse form is not allowed: the hex kinds came with it
/// ([`KeyCodec::detect_any`]), and a whole run of hex keys written where
/// it is allowed is a first-version file of a kind a reader from before
/// refuses by its code.
pub const KEY_KINDS_DENSE: [KeyKind; 2] = [KeyKind::Decimal, KeyKind::BigEndian];

impl KeyKind {
    /// The byte a stride trailer records.
    pub fn code(self) -> u8 {
        match self {
            KeyKind::Decimal => 1,
            KeyKind::BigEndian => 2,
            KeyKind::HexLower => 3,
            KeyKind::HexUpper => 4,
        }
    }

    /// The kind a trailer's byte names, if any.
    pub fn from_code(c: u8) -> Option<KeyKind> {
        match c {
            1 => Some(KeyKind::Decimal),
            2 => Some(KeyKind::BigEndian),
            3 => Some(KeyKind::HexLower),
            4 => Some(KeyKind::HexUpper),
            _ => None,
        }
    }

    /// The widest field of this kind.
    pub fn d_max(self) -> usize {
        match self {
            KeyKind::Decimal => STRIDE_DECIMAL_MAX,
            KeyKind::BigEndian => STRIDE_BE_MAX,
            KeyKind::HexLower | KeyKind::HexUpper => STRIDE_HEX_MAX,
        }
    }

    /// The values a byte of the field takes.
    fn radix(self) -> u64 {
        match self {
            KeyKind::Decimal => 10,
            KeyKind::BigEndian => 256,
            KeyKind::HexLower | KeyKind::HexUpper => 16,
        }
    }

    /// The first of the six letters a hex kind's digits past nine are.
    fn hex_a(self) -> u8 {
        if self == KeyKind::HexUpper {
            b'A'
        } else {
            b'a'
        }
    }

    /// The field byte after `b` in the kind's order, or `None` where `b`
    /// is its last and the field carries: the step a rendered key takes
    /// to the next number's ([`KeyCodec::increment`]).
    fn next_byte(self, b: u8) -> Option<u8> {
        match self {
            KeyKind::Decimal => (b != b'9').then(|| b + 1),
            KeyKind::BigEndian => b.checked_add(1),
            KeyKind::HexLower | KeyKind::HexUpper => {
                let a = self.hex_a();
                if b == b'9' {
                    Some(a)
                } else if b == a + 5 {
                    None
                } else {
                    Some(b + 1)
                }
            }
        }
    }

    /// The field byte of value zero, which a field that carries wraps to.
    fn zero_byte(self) -> u8 {
        match self {
            KeyKind::BigEndian => 0,
            _ => b'0',
        }
    }

    /// For a byte `b` of a key, how many of this kind's field bytes are
    /// below it, and whether it is one: a field byte's value is the first.
    fn rank_byte(self, b: u8) -> (u64, bool) {
        match self {
            KeyKind::Decimal if b < b'0' => (0, false),
            KeyKind::Decimal if b > b'9' => (10, false),
            KeyKind::Decimal => (u64::from(b - b'0'), true),
            KeyKind::BigEndian => (u64::from(b), true),
            KeyKind::HexLower | KeyKind::HexUpper => {
                let a = self.hex_a();
                if b < b'0' {
                    (0, false)
                } else if b <= b'9' {
                    (u64::from(b - b'0'), true)
                } else if b < a {
                    (10, false)
                } else if b <= a + 5 {
                    (10 + u64::from(b - a), true)
                } else {
                    (16, false)
                }
            }
        }
    }
}

/// The sixteen hex digits of `x` as two words of eight, the first eight in
/// `.0`, each word's bytes from its top the digits in order; the letters
/// from `a` (`b'a'` or `b'A'`). Each half's eight nibbles are spread a byte
/// apiece by three shifts and masks, then every byte made its digit at
/// once: a byte of ten or more carries into its bit four when six is added,
/// and that bit times the letters' distance from the digits is what a
/// letter adds.
#[inline]
fn hex16_words(x: u64, a: u8) -> (u64, u64) {
    #[inline]
    fn spread(v: u32) -> u64 {
        let mut y = u64::from(v);
        y = (y | (y << 16)) & 0x0000_FFFF_0000_FFFF;
        y = (y | (y << 8)) & 0x00FF_00FF_00FF_00FF;
        (y | (y << 4)) & 0x0F0F_0F0F_0F0F_0F0F
    }
    #[inline]
    fn ascii(y: u64, a: u8) -> u64 {
        let ten = ((y + 0x0606_0606_0606_0606) >> 4) & 0x0101_0101_0101_0101;
        y + 0x3030_3030_3030_3030 + ten * u64::from(a - b'0' - 10)
    }
    (
        ascii(spread((x >> 32) as u32), a),
        ascii(spread(x as u32), a),
    )
}

/// The eight decimal digits of `v`, below 10^8, as a word whose bytes from
/// its top are the digits in order: the number halved into two of four
/// digits, each into two of two and each of those into two of one, every
/// divide a multiply and a shift that is exact over the range its lane
/// holds (`n * 5243 >> 19` for `n / 100` below ten thousand, `n * 103 >>
/// 10` for `n / 10` below a hundred), the lanes of one word so the three
/// steps are three multiplies. The lanes come out with the first digit in
/// the lowest byte, and the word is turned end for end.
#[inline]
fn dec8(v: u32) -> u64 {
    debug_assert!(v < 100_000_000);
    let v = u64::from(v);
    let hi = v / 10_000;
    let mut x = hi | ((v - hi * 10_000) << 32);
    let q = ((x * 5243) >> 19) & 0x0000_007F_0000_007F;
    x = q | ((x - q * 100) << 16);
    let q = ((x * 103) >> 10) & 0x000F_000F_000F_000F;
    x = q | ((x - q * 10) << 8);
    (x + 0x3030_3030_3030_3030).swap_bytes()
}

/// The sixteen decimal digits of `x`, below 10^16, as two words of eight,
/// as `hex16_words` has a number's hex digits.
#[inline]
fn dec16_words(x: u64) -> (u64, u64) {
    debug_assert!(x < 10_000_000_000_000_000);
    let hi = x / 100_000_000;
    (dec8(hi as u32), dec8((x - hi * 100_000_000) as u32))
}

/// The last `field.len()` of sixteen digits held as two words
/// (`hex16_words`, `dec16_words`), written over `field`, at most sixteen
/// bytes: each store a whole word from a register -- two that overlap for
/// a field of eight to fifteen digits, its first eight bytes cut from the
/// two words and its last eight the second word -- so that nothing loads a
/// digit back before the key is read. A field under eight digits is the
/// second word's last bytes.
#[inline]
fn put_digits(field: &mut [u8], hi: u64, lo: u64) {
    let d = field.len();
    debug_assert!(d <= 16);
    if d >= 16 {
        field[..8].copy_from_slice(&hi.to_be_bytes());
        field[8..16].copy_from_slice(&lo.to_be_bytes());
    } else if d >= 8 {
        let both = (u128::from(hi) << 64) | u128::from(lo);
        let first = (both >> (8 * (d - 8))) as u64;
        field[..8].copy_from_slice(&first.to_be_bytes());
        field[d - 8..].copy_from_slice(&lo.to_be_bytes());
    } else {
        field.copy_from_slice(&lo.to_be_bytes()[8 - d..]);
    }
}

/// The sixteen digits of the two words, in order: a test's reading of them.
#[cfg(test)]
fn digits16((hi, lo): (u64, u64)) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&hi.to_be_bytes());
    out[8..].copy_from_slice(&lo.to_be_bytes());
    out
}

/// Why two keys are not the ends of a run a stride form renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyShapeNo {
    /// They are not of one length of one to [`STRIDE_KEY_MAX`] bytes: the
    /// empty key renders from no field.
    Length,
    /// The field past the prefix they share is of no kind asked: neither
    /// decimal digits, at most [`STRIDE_DECIMAL_MAX`] of them, nor at most
    /// [`STRIDE_BE_MAX`] bytes, nor, where the hex kinds are asked, at
    /// most [`STRIDE_HEX_MAX`] hex digits of one case.
    Field,
    /// No kind counts exactly the run's keys between them.
    Holes,
}

/// The keys of a stride form: a prefix `P` the run shares and a field of
/// `D` bytes holding a number, zero-padded decimal or big-endian, so that
/// every key is `L = P + D` bytes and rank `r`'s key is the rendering of
/// `base + r`. Both kinds render at a fixed width, so rendering is strictly
/// increasing under the byte order the engine's keys sort by (`key_cmp`),
/// and a key's rank, a seek's answer and a scan's next key are arithmetic.
///
/// Its fields are private and its constructors check them, so a codec in
/// hand renders every number from `base` to [`KeyCodec::top`]: a codec
/// that could not would answer a read with a key it cannot write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyCodec {
    kind: KeyKind,
    d: u8,
    p: u8,
    prefix: [u8; STRIDE_KEY_MAX],
    base: u64,
}

impl KeyCodec {
    /// A codec of `kind` over `prefix` and a field of `d` bytes, whose run
    /// starts at `base`; the reason where they make none.
    pub fn new(
        kind: KeyKind,
        prefix: &[u8],
        d: usize,
        base: u64,
    ) -> std::result::Result<KeyCodec, &'static str> {
        if d == 0 || d > kind.d_max() {
            return Err("the field's width is not one to the most its kind holds");
        }
        if prefix.len() + d > STRIDE_KEY_MAX {
            return Err("the key is longer than a stride form takes");
        }
        let mut c = KeyCodec {
            kind,
            d: d as u8,
            p: prefix.len() as u8,
            prefix: [0; STRIDE_KEY_MAX],
            base,
        };
        c.prefix[..prefix.len()].copy_from_slice(prefix);
        if base > c.top() {
            return Err("the first key's number is past what the field holds");
        }
        Ok(c)
    }

    /// The codec under which `first` and `last` are the ends of a run of
    /// exactly `n` keys, each rendered from its number: the prefix they
    /// share, cut a byte short of the key so the field has one, and the
    /// rest the field. Decimal is tried where both fields are digits and
    /// big-endian where the field fits a u64, in that order, and a kind is
    /// taken only where it counts `n` keys from the first to the last; a
    /// key between the two shares the prefix, since bytewise order puts
    /// every key that does not outside them. What this proves is the run's
    /// count: that the keys between are the renderings is the build's
    /// walk to check, a key at a time.
    pub fn detect(first: &[u8], last: &[u8], n: u64) -> std::result::Result<KeyCodec, KeyShapeNo> {
        match KeyCodec::detect_any(first, last, n, &KEY_KINDS_DENSE, &[])? {
            (c, true) => Ok(c),
            (_, false) => Err(KeyShapeNo::Holes),
        }
    }

    /// `detect` over the kinds asked, in their order, for a run that may
    /// have gaps: the codec under which `first` and `last` are the ends of
    /// a run of `n` keys each rendered from its number, and whether the
    /// run is whole -- the numbers between the two ends exactly `n`, so
    /// rank `r`'s is `base + r` -- or has gaps, more numbers between them
    /// than keys, where a form must hold each key's number
    /// ([`StrideTrailer::sparse`]). A kind that counts exactly `n` is
    /// taken before any that counts more, whatever their order: a whole
    /// run is one, and the form that holds no number is the smaller. A
    /// kind that counts fewer than `n` holds no `n` distinct keys between
    /// the ends and is no kind of this run.
    ///
    /// A kind is tried where the field is no wider than it holds and
    /// every field byte of both ends and of every key of `more` is one of
    /// its. The ends alone do not say which kind a run with gaps is: two
    /// keys of hex digits are, once in a while, both of decimal digits,
    /// and the narrower kind would then be taken for a run it does not
    /// hold, which the build's walk finds at the first key with a letter
    /// and gives the whole build up for. `more` is keys from inside the
    /// run that the caller read for this, and with a few of them the
    /// narrower kind is taken for a wider run one time in the alphabets'
    /// ratio to the power of every field byte read. What is proved is the
    /// count's bound and the read keys' shape: every other key is the
    /// build's walk to check.
    pub fn detect_any(
        first: &[u8],
        last: &[u8],
        n: u64,
        kinds: &[KeyKind],
        more: &[&[u8]],
    ) -> std::result::Result<(KeyCodec, bool), KeyShapeNo> {
        let l = first.len();
        if l == 0 || l > STRIDE_KEY_MAX || last.len() != l || more.iter().any(|k| k.len() != l) {
            return Err(KeyShapeNo::Length);
        }
        let shared = first.iter().zip(last).take_while(|(a, b)| a == b).count();
        let p = shared.min(l - 1);
        let d = l - p;
        let mut shaped = false;
        let mut gapped = None;
        for &kind in kinds {
            let of_kind = |k: &[u8]| k[p..].iter().all(|&b| kind.rank_byte(b).1);
            if d > kind.d_max()
                || !(of_kind(first) && of_kind(last))
                || !more.iter().all(|k| k[..p] == first[..p] && of_kind(k))
            {
                continue;
            }
            shaped = true;
            // Neither can refuse past the test above, and a refusal here
            // would be a kind turned away for a reason no key earns: every
            // byte of the field is the kind's and the field is no wider
            // than the kind holds, so it parses within a u64; the field is
            // at least a byte, the prefix being cut short of the key, the
            // key at most the longest, and a field's number at most its top.
            let base = KeyCodec::field_of(kind, &first[p..])
                .expect("a field of the kind's bytes, no wider than it holds");
            let c = KeyCodec::new(kind, &first[..p], d, base)
                .expect("a field of one byte to the kind's widest, in a key the form takes");
            // The numbers from the first to the last, which is one more
            // than a u64 holds for the whole of an eight-byte field.
            let count = c
                .decode(last)
                .and_then(|x| x.checked_sub(base))
                .map(|k| u128::from(k) + 1);
            match count {
                Some(k) if k == u128::from(n) => return Ok((c, true)),
                Some(k) if k > u128::from(n) && gapped.is_none() => gapped = Some(c),
                _ => {}
            }
        }
        match gapped {
            Some(c) => Ok((c, false)),
            None if shaped => Err(KeyShapeNo::Holes),
            None => Err(KeyShapeNo::Field),
        }
    }

    /// The kind of the field.
    pub fn kind(&self) -> KeyKind {
        self.kind
    }

    /// The field's width, `D`.
    pub fn d(&self) -> usize {
        usize::from(self.d)
    }

    /// The key's length, `L`.
    pub fn key_len(&self) -> usize {
        usize::from(self.p) + usize::from(self.d)
    }

    /// The prefix every key shares, `P` bytes.
    pub fn prefix(&self) -> &[u8] {
        &self.prefix[..usize::from(self.p)]
    }

    /// The first key's number.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The largest number the field holds.
    pub fn top(&self) -> u64 {
        match self.kind {
            KeyKind::Decimal => 10u64.pow(u32::from(self.d)) - 1,
            KeyKind::BigEndian => u64::MAX >> (8 * (STRIDE_BE_MAX - self.d())),
            KeyKind::HexLower | KeyKind::HexUpper => u64::MAX >> (4 * (STRIDE_HEX_MAX - self.d())),
        }
    }

    /// Whether a run of `n` keys from `base` is a run at all and renders
    /// whole: `n` at least one, and `base + n - 1` within the field.
    pub fn holds(&self, n: u64) -> bool {
        n.checked_sub(1)
            .and_then(|k| self.base.checked_add(k))
            .is_some_and(|last| last <= self.top())
    }

    /// The number a field of `kind` holds, where every byte is one of its.
    fn field_of(kind: KeyKind, field: &[u8]) -> Option<u64> {
        let radix = kind.radix();
        let mut x = 0u64;
        for &b in field {
            let (v, ok) = kind.rank_byte(b);
            if !ok {
                return None;
            }
            x = x.checked_mul(radix)?.checked_add(v)?;
        }
        Some(x)
    }

    /// The number `key` renders, where it is this codec's: the key's
    /// length, the prefix and every field byte one of the kind's; `None`
    /// for any other key. Not its rank: a rank is the number less `base`.
    pub fn decode(&self, key: &[u8]) -> Option<u64> {
        let p = usize::from(self.p);
        if key.len() != self.key_len() || key[..p] != self.prefix[..p] {
            return None;
        }
        KeyCodec::field_of(self.kind, &key[p..])
    }

    /// Number `x`'s key, rendered into `buf`, at most [`KeyCodec::top`].
    pub fn render<'a>(&self, x: u64, buf: &'a mut [u8; STRIDE_KEY_MAX]) -> &'a [u8] {
        debug_assert!(x <= self.top(), "{x} is past what the field holds");
        let (p, l) = (usize::from(self.p), self.key_len());
        buf[..p].copy_from_slice(&self.prefix[..p]);
        let field = &mut buf[p..l];
        match self.kind {
            KeyKind::Decimal => {
                let mut v = x;
                for b in field.iter_mut().rev() {
                    *b = b'0' + (v % 10) as u8;
                    v /= 10;
                }
            }
            KeyKind::BigEndian => {
                field.copy_from_slice(&x.to_be_bytes()[STRIDE_BE_MAX - self.d()..])
            }
            KeyKind::HexLower | KeyKind::HexUpper => {
                let a = self.kind.hex_a();
                let mut v = x;
                for b in field.iter_mut().rev() {
                    let digit = (v & 15) as u8;
                    *b = if digit < 10 {
                        b'0' + digit
                    } else {
                        a + digit - 10
                    };
                    v >>= 4;
                }
            }
        }
        &buf[..l]
    }

    /// Number `x`'s field written over `field`, the `D` bytes after the
    /// prefix of a key of this codec: what a walk of a sparse form steps a
    /// key by, the prefix standing from the key before
    /// (`Options::zpages_stride_step_direct`). The bytes [`render`] writes
    /// after the prefix, and nothing else of them is shared with it: the
    /// digits are made a word at a time (`hex16_words`, `dec16_words`) and
    /// stored from the two words where they lie (`put_digits`), no byte of
    /// them through a buffer, where `render`'s loop stores a byte a digit
    /// and its caller then loads the key whole from those bytes, a load no
    /// store forwards. A decimal field wider than sixteen digits goes by
    /// `render`'s loop.
    ///
    /// [`render`]: KeyCodec::render
    #[inline]
    pub fn render_field(&self, x: u64, field: &mut [u8]) {
        debug_assert!(x <= self.top(), "{x} is past what the field holds");
        debug_assert_eq!(field.len(), self.d(), "not this codec's field");
        let d = field.len();
        match self.kind {
            KeyKind::HexLower | KeyKind::HexUpper if d <= 16 => {
                let (hi, lo) = hex16_words(x, self.kind.hex_a());
                put_digits(field, hi, lo);
            }
            KeyKind::Decimal if d <= 16 => {
                let (hi, lo) = dec16_words(x);
                put_digits(field, hi, lo);
            }
            KeyKind::Decimal => {
                let mut v = x;
                for b in field.iter_mut().rev() {
                    *b = b'0' + (v % 10) as u8;
                    v /= 10;
                }
            }
            KeyKind::BigEndian if d <= STRIDE_BE_MAX => {
                field.copy_from_slice(&x.to_be_bytes()[STRIDE_BE_MAX - d..]);
            }
            // No codec has such a field (`KeyCodec::new` holds the width to
            // its kind's most): written as zeros rather than read past.
            _ => field.fill(self.kind.zero_byte()),
        }
    }

    /// The prefix's length, `P`.
    pub fn p(&self) -> usize {
        usize::from(self.p)
    }

    /// A rendered key moved to the next number's, in place, as a scan
    /// steps a rank: the field's last byte up by one, carried left, the
    /// prefix untouched. False where the field was [`KeyCodec::top`] and
    /// wrapped to zero, which a walk inside a run that [`holds`] never
    /// reaches.
    ///
    /// [`holds`]: KeyCodec::holds
    pub fn increment(&self, key: &mut [u8]) -> bool {
        debug_assert_eq!(key.len(), self.key_len(), "not a rendered key");
        let zero = self.kind.zero_byte();
        for b in key[usize::from(self.p)..].iter_mut().rev() {
            match self.kind.next_byte(*b) {
                Some(next) => {
                    *b = next;
                    return true;
                }
                None => *b = zero,
            }
        }
        false
    }

    /// Where `key` falls among the field's numbers: the least number whose
    /// rendering is at or after `key` in byte order, and whether that
    /// rendering is `key` itself -- a key the codec renders is its own
    /// number, and any other key stands just under the first rendering
    /// above it, the count of the renderings below it. One past the
    /// field's last number where every rendering is below `key`, which is
    /// why it is a u128. What a search of a run with gaps looks for
    /// (`lower_bound` is this less `base`, for a run with none).
    pub fn at_or_after(&self, key: &[u8]) -> (u128, bool) {
        match self.decode(key) {
            Some(x) => (u128::from(x), true),
            None => (self.below(key), false),
        }
    }

    /// Where `key` falls in the run of `n` keys from `base`: the rank of
    /// the first key at or after it, `Equal` where that key is `key` and
    /// `Greater` otherwise, `n` with `Greater` past them all -- the answer
    /// a binary search of the rendered keys under `key_cmp` gives, by
    /// arithmetic. Any key is asked: a seek's key is the caller's and need
    /// not be shaped.
    pub fn lower_bound(&self, key: &[u8], n: u64) -> (u64, Ordering) {
        if let Some(x) = self.decode(key) {
            return match x.checked_sub(self.base) {
                None => (0, Ordering::Greater),
                Some(r) if r >= n => (n, Ordering::Greater),
                Some(r) => (r, Ordering::Equal),
            };
        }
        let below = self.below(key);
        let r = below
            .saturating_sub(u128::from(self.base))
            .min(u128::from(n));
        (r as u64, Ordering::Greater)
    }

    /// How many numbers of the field render to a key below `key` in byte
    /// order: every one where `key` is above the prefix, none where it is
    /// below it or the prefix alone, and otherwise the field's renderings
    /// below `key`'s tail, counted byte by byte from the left -- at each
    /// byte the renderings that agree before it and take a smaller byte
    /// there, until the tail leaves the kind's bytes or runs out. A u128,
    /// since the big-endian field of eight bytes holds 2^64 numbers.
    fn below(&self, key: &[u8]) -> u128 {
        let (p, d) = (usize::from(self.p), self.d());
        let radix = u128::from(self.kind.radix());
        let all = radix.pow(d as u32);
        let k = key.len().min(p);
        match key[..k].cmp(&self.prefix[..k]) {
            Ordering::Less => return 0,
            Ordering::Greater => return all,
            Ordering::Equal => {}
        }
        if key.len() <= p {
            return 0;
        }
        let tail = &key[p..];
        let mut count = 0u128;
        let mut weight = all;
        for (i, &b) in tail.iter().enumerate() {
            if i == d {
                // The tail holds a whole field and more: the rendering of
                // its field is a proper prefix of it, so below it too.
                return count + 1;
            }
            weight /= radix;
            let (smaller, ok) = self.kind.rank_byte(b);
            count += u128::from(smaller) * weight;
            if !ok {
                return count;
            }
        }
        // The tail ran out at or before the field's end: every rendering
        // that agrees with it so far is as long or longer, so none is
        // below it.
        count
    }
}

/// A stride form's trailer, the file's last page: the keys' codec and the
/// values' width, the row's CRC and the segment the file derives from. The
/// magic, the version and the page size are constants of the layout;
/// `V`, `res_off`, `res_len` and the file's length are [`stride_layout`]'s
/// of the key count and the width and are written and checked, not held;
/// `bind32` and the CRC are derived.
///
/// A sparse form's trailer ([`StrideTrailer::sparse`]) is the second
/// version: the same fields at the same places, and after the prefix's
/// room the numbers' pages, the last key's number, the line's error and
/// the sample stride, where the first version holds zeros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrideTrailer {
    /// The keys' codec, the first key's number with it.
    pub codec: KeyCodec,
    /// Every value's width, `W`.
    pub w: u32,
    /// The resident part's CRC ([`stride_row_crc`]): the row's, and in a
    /// sparse form the row's, its padding's and the samples' together.
    pub res_crc: u32,
    /// The segment, whose key count is the run's.
    pub seg: SegIdent,
    /// What a form whose keys are numbers with gaps between them holds of
    /// its keys; `None` where they are a whole run, rank `r`'s number
    /// `base + r` and nothing held.
    pub sparse: Option<StrideSparse>,
}

/// A sparse stride form's keys: the file holds each rank's number after
/// the values, a little-endian u64 a rank ascending, and every
/// [`STRIDE_SAMPLE`]th of them again in its resident part, the samples a
/// reader keeps in memory. A key's rank is searched for: its number's
/// place among the samples is guessed by the straight line through the
/// first number and the last ([`stride_line`]), the samples within the
/// line's error of the guess are searched for the last one at or below the
/// number, and the run of numbers that sample opens is searched for the
/// number itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrideSparse {
    /// The last key's number, the line's other end; the first is the
    /// codec's `base`.
    pub last: u64,
    /// The most samples the line puts any of the form's numbers from the
    /// last sample at or below it, which the builder measured over every
    /// number ([`stride_eps`]): a search of the samples from that many
    /// before the guess to that many after it misses none. A count the
    /// builder made and no reader can check short of reading every number,
    /// so a reader takes it for where to look first and never for where
    /// the number is: a window that does not bracket the number is left
    /// for a search of every sample, and a wrong count costs that search
    /// and no answer.
    pub eps: u32,
}

/// The error of the line over `numbers`, a form's keys' numbers in rank
/// order: the most samples [`stride_line`] puts any of them from the last
/// sample at or below it. A number between two samples is guessed between
/// their guesses, the line being monotone, so the samples' own errors
/// bound it to one more: the measure is taken at each sample and at the
/// number just before the next, the two ends of every run.
pub fn stride_eps(numbers: &[u64]) -> u32 {
    let (Some(&lo), Some(&hi)) = (numbers.first(), numbers.last()) else {
        return 0;
    };
    let m = (numbers.len() as u64).div_ceil(STRIDE_SAMPLE);
    let step = STRIDE_SAMPLE as usize;
    let mut worst = 0u64;
    for (i, run) in numbers.chunks(step).enumerate() {
        let i = i as u64;
        for &x in [run[0], run[run.len() - 1]].iter() {
            worst = worst.max(stride_line(lo, hi, m, x).abs_diff(i));
        }
    }
    worst.min(u64::from(u32::MAX)) as u32
}

// A stride trailer's fields, past the head every trailer has (`T_MAGIC`
// to `T_HEAD`, and `T_CRC`).
const S_CODEC: usize = T_HEAD;
const S_D: usize = 17;
const S_L: usize = 18;
const S_P: usize = 19;
const S_W: usize = 20;
const S_BASE: usize = 24;
const S_V: usize = 32;
const S_RES_OFF: usize = 40;
const S_RES_LEN: usize = 48;
const S_RES_CRC: usize = 56;
const S_BIND32: usize = 60;
const S_SEG: usize = 64;
const S_PREFIX: usize = S_SEG + SI_LEN;
/// Where the zeros after the prefix's room begin, which run to the CRC in
/// a form whose keys are a whole run.
const S_ZERO: usize = S_PREFIX + STRIDE_KEY_MAX;
// A sparse form's fields, where the first version holds zeros.
const S_K: usize = S_ZERO;
const S_LAST: usize = S_ZERO + 8;
const S_EPS: usize = S_ZERO + 16;
const S_SAMPLE: usize = S_ZERO + 20;
/// Where a sparse trailer's zeros begin.
const S_ZERO_SPARSE: usize = S_ZERO + 24;

impl StrideTrailer {
    /// The segment the file says it derives from.
    pub fn seg_ident(&self) -> SegIdent {
        self.seg
    }

    /// The hash the file's row is bound by ([`bind32`]).
    pub fn bind32(&self) -> u32 {
        self.seg.bind32()
    }

    /// The file's sizes, after the checks its fields need of each other: a
    /// key count of one to what a u32 page index counts, a run the codec
    /// renders whole, and a width of one to [`STRIDE_W_MAX`].
    pub fn layout(&self) -> Result<StrideLayout, ZError> {
        let bad = ZError::Trailer;
        let n = self.seg.nkeys;
        if n == 0 || n > u64::from(u32::MAX) {
            return Err(bad("the key count is not one to what a page index counts"));
        }
        if self.w == 0 || self.w > STRIDE_W_MAX {
            return Err(bad("the value width is not one to a page"));
        }
        let Some(sp) = self.sparse else {
            if !self.codec.holds(n) {
                return Err(bad("the run passes what its key field holds"));
            }
            // Inside those bounds the values are under 2^44 bytes, and
            // nothing of the layout overflows: a refusal here would be one
            // no file could earn.
            return Ok(
                stride_layout(n, self.w).expect("a u32 of keys of a page each overflows no u64")
            );
        };
        // A sparse form's numbers ascend from the codec's base to the last
        // inside the field, and `n` distinct numbers need as many between
        // the two; the line is never wrong by more samples than there are.
        let base = self.codec.base();
        if sp.last > self.codec.top() || sp.last < base {
            return Err(bad(
                "the last key's number is not in the field past the first",
            ));
        }
        if u128::from(sp.last - base) + 1 < u128::from(n) {
            return Err(bad("the numbers between the ends are fewer than the keys"));
        }
        if u64::from(sp.eps) > n.div_ceil(STRIDE_SAMPLE) {
            return Err(bad("the line's error is more samples than the form has"));
        }
        Ok(stride_layout_sparse(n, self.w)
            .expect("a u32 of keys of a page and a word each overflows no u64"))
    }

    /// The length the file must have.
    pub fn expected_file_len(&self) -> Option<u64> {
        self.layout().ok().map(|l| l.file_len)
    }

    /// The trailer's bytes, or the check its fields fail.
    pub fn encode(&self) -> Result<[u8; ZTRAILER], ZError> {
        let lay = self.layout()?;
        let c = &self.codec;
        let mut b = [0u8; ZTRAILER];
        b[S_CODEC] = c.kind.code();
        b[S_D] = c.d;
        b[S_L] = c.key_len() as u8;
        b[S_P] = c.p;
        put32(&mut b, S_W, self.w);
        put64(&mut b, S_BASE, c.base);
        put64(&mut b, S_V, lay.v_pages);
        put64(&mut b, S_RES_OFF, lay.res_off);
        put64(&mut b, S_RES_LEN, lay.res_len);
        put32(&mut b, S_RES_CRC, self.res_crc);
        put32(&mut b, S_BIND32, self.bind32());
        self.seg.put(&mut b[S_SEG..S_PREFIX]);
        b[S_PREFIX..S_PREFIX + c.prefix().len()].copy_from_slice(c.prefix());
        let version = match self.sparse {
            None => STRIDE_VERSION,
            Some(sp) => {
                put64(&mut b, S_K, lay.k_pages);
                put64(&mut b, S_LAST, sp.last);
                put32(&mut b, S_EPS, sp.eps);
                put32(&mut b, S_SAMPLE, STRIDE_SAMPLE as u32);
                STRIDE_VERSION_SPARSE
            }
        };
        seal_trailer(&mut b, &STRIDE_MAGIC, version);
        Ok(b)
    }

    /// The trailer out of a page, after every check that needs nothing of
    /// the file: the head's first, as every trailer makes them
    /// ([`open_trailer`]), and then the fields against each other: the
    /// codec's kind, the key's length, the prefix and the field making it,
    /// the field's width, the run in the field, the width, the sizes
    /// against [`stride_layout`]'s, the zeros, and `bind32` against the
    /// fields it hashes.
    pub fn decode(bytes: &[u8]) -> Result<StrideTrailer, ZError> {
        // Either version this reader knows, by the word the head holds it
        // in; a page too short to hold the word fails the head's length.
        let sparse = bytes.len() == ZTRAILER && get32(bytes, T_VERSION) == STRIDE_VERSION_SPARSE;
        open_trailer(
            bytes,
            &STRIDE_MAGIC,
            if sparse {
                STRIDE_VERSION_SPARSE
            } else {
                STRIDE_VERSION
            },
        )?;
        let bad = ZError::Trailer;
        let kind = KeyKind::from_code(bytes[S_CODEC])
            .ok_or(bad("the key codec is not one this reader knows"))?;
        let (d, l, p) = (
            usize::from(bytes[S_D]),
            usize::from(bytes[S_L]),
            usize::from(bytes[S_P]),
        );
        if l == 0 || l > STRIDE_KEY_MAX {
            return Err(bad(
                "the key length is not one to the most a stride form takes",
            ));
        }
        if p + d != l {
            return Err(bad("the prefix and the field are not the key's length"));
        }
        let codec = KeyCodec::new(
            kind,
            &bytes[S_PREFIX..S_PREFIX + p],
            d,
            get64(bytes, S_BASE),
        )
        .map_err(bad)?;
        let t = StrideTrailer {
            codec,
            w: get32(bytes, S_W),
            res_crc: get32(bytes, S_RES_CRC),
            seg: SegIdent::get(&bytes[S_SEG..S_PREFIX]),
            sparse: sparse.then(|| StrideSparse {
                last: get64(bytes, S_LAST),
                eps: get32(bytes, S_EPS),
            }),
        };
        let lay = t.layout()?;
        if sparse {
            if get64(bytes, S_K) != lay.k_pages {
                return Err(bad("K is not the numbers' pages"));
            }
            if u64::from(get32(bytes, S_SAMPLE)) != STRIDE_SAMPLE {
                return Err(bad("the sample stride is not this reader's"));
            }
        }
        if get64(bytes, S_V) != lay.v_pages {
            return Err(bad("V is not the values' pages"));
        }
        if get64(bytes, S_RES_OFF) != lay.res_off {
            return Err(bad("res_off is not the values' length"));
        }
        if get64(bytes, S_RES_LEN) != lay.res_len {
            return Err(bad("res_len is not the row's length"));
        }
        if bytes[S_PREFIX + p..S_ZERO].iter().any(|&b| b != 0) {
            return Err(bad("a byte past the prefix is not zero"));
        }
        let zeros = if sparse { S_ZERO_SPARSE } else { S_ZERO };
        if bytes[zeros..T_CRC].iter().any(|&b| b != 0) {
            return Err(bad("a reserved byte is not zero"));
        }
        if get32(bytes, S_BIND32) != t.bind32() {
            return Err(bad(
                "bind32 is not the hash of the segment's identity fields",
            ));
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatindex::stream_record_as;
    use crate::index::{put_uvarint, Ext};

    // ---- fixtures -------------------------------------------------------

    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }
        fn word(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
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

    /// Bytes at an address aligned for extents, as a decoded group is.
    struct Al {
        words: Vec<u64>,
        len: usize,
    }

    impl Al {
        fn new(bytes: &[u8]) -> Al {
            let mut words = vec![0u64; bytes.len().div_ceil(8).max(1)];
            // SAFETY: `words` owns at least `bytes.len()` initialised bytes.
            let dst = unsafe {
                std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, words.len() * 8)
            };
            dst[..bytes.len()].copy_from_slice(bytes);
            Al {
                words,
                len: bytes.len(),
            }
        }
        fn bytes(&self) -> &[u8] {
            // SAFETY: as above, shared.
            unsafe { std::slice::from_raw_parts(self.words.as_ptr() as *const u8, self.len) }
        }
    }

    /// CRC-32C by the definition, a bit at a time: the second definition
    /// the engine's table and hardware paths are held to.
    fn crc32c(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x82F6_3B78
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn le32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn le64(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }

    fn varint(v: usize) -> Vec<u8> {
        let mut o = Vec::new();
        put_uvarint(&mut o, v as u64);
        o
    }

    fn inline(len: usize, last: usize, count: u32) -> Ext {
        Ext {
            block: Ext::INLINE,
            off: 0,
            len: len as u32,
            last: last as u32,
            count,
        }
    }

    fn image(key: &[u8], exts: &[Ext], tail: &[u8], compact: bool) -> Vec<u8> {
        let mut v = Vec::new();
        stream_record_as(&mut v, key, exts, tail, compact).expect("a fixture record is a record");
        v
    }

    /// The suite's record: a key with one fixed-width value, compact.
    fn one_value(key: &[u8], value: &[u8]) -> Vec<u8> {
        image(key, &[inline(value.len(), 0, 1 | Ext::FIXED)], value, true)
    }

    /// A record of any shape the format has, keyed by `key`.
    fn random_image(rng: &mut Rng, key: &[u8]) -> Vec<u8> {
        match rng.below(8) {
            0 => {
                let l = rng.below(300);
                let v = rng.bytes(l);
                one_value(key, &v)
            }
            1 => {
                let n = 2 + rng.below(18);
                let w = 1 + rng.below(39);
                let run = rng.bytes(n * w);
                let e = inline(run.len(), (n - 1) * w, n as u32 | Ext::FIXED);
                image(key, &[e], &run, true)
            }
            2 => {
                let n = 2 + rng.below(18);
                let mut run = Vec::new();
                let mut last = 0;
                for i in 0..n {
                    let l = rng.below(60);
                    if i == n - 1 {
                        last = run.len();
                    }
                    run.extend(varint(l));
                    run.extend(rng.bytes(l));
                }
                let e = inline(run.len(), last, n as u32);
                image(key, &[e], &run, true)
            }
            3 => image(key, &[inline(0, 0, Ext::TOMBSTONE)], &[], true),
            4 => {
                let l = rng.below(300);
                let v = rng.bytes(l);
                image(key, &[inline(v.len(), 0, 1 | Ext::FIXED)], &v, false)
            }
            5 => {
                let k = 2 + rng.below(3);
                let mut tail = Vec::new();
                let mut exts = Vec::new();
                for _ in 0..k {
                    if rng.below(3) == 0 {
                        exts.push(Ext {
                            block: rng.below(1000) as u32,
                            off: rng.below(65_536) as u32,
                            len: 1 + rng.below(200) as u32,
                            last: 0,
                            count: 1,
                        });
                    } else {
                        let l = 1 + rng.below(50);
                        exts.push(Ext {
                            block: Ext::INLINE,
                            off: tail.len() as u32,
                            len: l as u32,
                            last: 0,
                            count: 1,
                        });
                        tail.extend(rng.bytes(l));
                    }
                }
                image(key, &exts, &tail, false)
            }
            6 => {
                let e = Ext {
                    block: rng.below(50) as u32,
                    off: rng.below(65_536) as u32,
                    len: 1 + rng.below(4000) as u32,
                    last: 0,
                    count: 1,
                };
                image(key, &[e], &[], true)
            }
            _ => image(key, &[], &[], false),
        }
    }

    /// `n` records of every shape with distinct ascending keys, as
    /// (key, image).
    fn sorted_records(rng: &mut Rng, n: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut keys = std::collections::BTreeSet::new();
        while keys.len() < n {
            let l = rng.below(40);
            keys.insert(rng.bytes(l));
        }
        keys.into_iter()
            .map(|k| {
                let img = random_image(rng, &k);
                (k, img)
            })
            .collect()
    }

    /// The suite's shape: sixteen decimal digits and a hundred bytes of which
    /// half is a repeated phrase and half entropy, at a random phase.
    fn suite_images(n: usize, seed: u64) -> Vec<Vec<u8>> {
        let mut rng = Rng::new(seed);
        let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
        (0..n)
            .map(|i| {
                let key = format!("{i:016}");
                let phase = rng.below(motif.len());
                let mut v: Vec<u8> = (0..50).map(|j| motif[(phase + j) % motif.len()]).collect();
                v.extend(rng.bytes(50));
                one_value(key.as_bytes(), &v)
            })
            .collect()
    }

    /// A page built by hand from one group, so the layout's tests do not
    /// stand on `fill_page`.
    fn manual_page(
        group: &[u8],
        count: u16,
        first_rank: u32,
        bind: u32,
        stored: bool,
    ) -> [u8; ZPAGE] {
        manual_pages(&[(group, count, stored, &[])], first_rank, bind)
    }

    /// A page built by hand from several groups: each its bytes, its record
    /// count, whether it is stored verbatim, and the key the directory
    /// names it by (unused for the first).
    fn manual_pages(
        groups: &[(&[u8], u16, bool, &[u8])],
        first_rank: u32,
        bind: u32,
    ) -> [u8; ZPAGE] {
        let mut page = [0u8; ZPAGE];
        let payloads: Vec<Vec<u8>> = groups
            .iter()
            .map(|&(group, _, stored, _)| {
                if stored {
                    group.to_vec()
                } else {
                    lz4_flex::block::compress(group)
                }
            })
            .collect();
        let pg: Vec<PageGroup<'_>> = groups
            .iter()
            .zip(&payloads)
            .map(|(&(group, count, stored, key), payload)| PageGroup {
                count,
                payload,
                ulen: group.len() as u32,
                stored,
                coded: false,
                key,
            })
            .collect();
        encode_page(&mut page, first_rank, &pg, bind).expect("the fixture page encodes");
        page
    }

    fn resign_page(page: &mut [u8; ZPAGE], bind: u32) {
        let crc = page_crc(page, bind);
        page[0..4].copy_from_slice(&crc.to_le_bytes());
    }

    fn resign_trailer(b: &mut [u8; ZTRAILER]) {
        let crc = crc32c(&b[..T_CRC]);
        b[T_CRC..].copy_from_slice(&crc.to_le_bytes());
    }

    /// A group of records that compresses (the suite's) and one that does
    /// not (entropy), each a few hundred bytes.
    fn small_groups() -> Vec<(Vec<Vec<u8>>, bool)> {
        let mut rng = Rng::new(11);
        let compressible = suite_images(24, 3);
        let entropy: Vec<Vec<u8>> = (0..8)
            .map(|i| one_value(format!("{i:016}").as_bytes(), &rng.bytes(100)))
            .collect();
        vec![(compressible, false), (entropy, true)]
    }

    // ---- the record coding ---------------------------------------------

    /// Groups of every shape the module's tests know, each as its images in
    /// key order: the suite's, entropy that does not compress, the empty
    /// key first, long keys, and records of every extent shape.
    fn coding_groups() -> Vec<Vec<Vec<u8>>> {
        let mut rng = Rng::new(29);
        let mut v: Vec<Vec<Vec<u8>>> = small_groups().into_iter().map(|(g, _)| g).collect();
        v.push(suite_images(16, 5));
        v.push(suite_images(1, 6));
        let mut empty_first = vec![one_value(b"", b"v")];
        empty_first.extend(suite_images(5, 7));
        v.push(empty_first);
        let long: Vec<Vec<u8>> = (0..6u8)
            .map(|i| one_value(&[0x61 + i].repeat(300 + 7 * usize::from(i)), &rng.bytes(40)))
            .collect();
        v.push(long);
        let recs = sorted_records(&mut rng, 20);
        let shapes: Vec<Vec<u8>> = recs
            .iter()
            .map(|(k, _)| random_image(&mut rng, k))
            .collect();
        v.push(shapes);
        v
    }

    fn coded(imgs: &[Vec<u8>]) -> (Vec<u8>, Vec<usize>, Vec<u8>) {
        let mut all = Vec::new();
        let mut ends = Vec::new();
        for im in imgs {
            all.extend_from_slice(im);
            ends.push(all.len());
        }
        let mut out = vec![0u8; 2 * ZMAX_ULEN];
        let n = encode_record_group(&all, &ends, &mut out, false).expect("the group codes");
        out.truncate(n);
        (all, ends, out)
    }

    #[test]
    fn the_record_coding_round_trips_and_seeks_as_the_decoded_group() {
        for imgs in coding_groups() {
            let (all, ends, payload) = coded(&imgs);
            let count = imgs.len();
            let mut buf = GroupBuf::new();
            let whole = decode_record_group(&payload, count, all.len(), &mut buf).unwrap();
            assert_eq!(whole, &all[..], "the group decoded whole");
            for (i, im) in imgs.iter().enumerate() {
                let mut b = GroupBuf::new();
                assert_eq!(
                    decode_record(&payload, count, i, &mut b).unwrap(),
                    &im[..],
                    "record {i}"
                );
            }
            // Every key, the key after each (a zero appended), the empty
            // key, one below every digit and one past every key.
            let keys: Vec<&[u8]> = imgs.iter().map(|im| record_key(im, 0).unwrap()).collect();
            let mut probes: Vec<Vec<u8>> = Vec::new();
            for k in &keys {
                probes.push(k.to_vec());
                let mut up = k.to_vec();
                up.push(0);
                probes.push(up);
            }
            probes.push(Vec::new());
            probes.push(vec![0x00]);
            probes.push(vec![0xff; 400]);
            for p in &probes {
                let want = group_seek(&all, count, p).unwrap();
                let got = record_group_seek(&payload, count, p).unwrap();
                assert_eq!(
                    (got.index, got.found),
                    (want.index, want.found),
                    "probe {p:?}"
                );
                if got.index < count {
                    let off =
                        u16::from_le_bytes([payload[2 * got.index], payload[2 * got.index + 1]]);
                    assert_eq!(
                        got.off,
                        usize::from(off),
                        "off names the record in the payload"
                    );
                } else {
                    assert_eq!(got.off, payload.len());
                }
            }
            // No room is a refusal, never a short payload.
            let mut small = vec![0u8; 12];
            assert!(encode_record_group(&all, &ends, &mut small, false).is_none());
        }
    }

    #[test]
    fn the_record_coding_is_near_the_block_codings_size_on_the_suites_records() {
        let imgs = suite_images(16, 21);
        let (all, _, payload) = coded(&imgs);
        let block = lz4_flex::block::compress(&all);
        assert!(
            payload.len() <= block.len() * 6 / 5,
            "coded {} against the block's {}",
            payload.len(),
            block.len()
        );
        assert!(
            payload.len() < all.len(),
            "the coding compresses the suite's records"
        );
    }

    /// Where record `i`'s single-byte fields lie in a payload of the suite's
    /// records: plen, slen, the ulen and the clen after the suffix.
    fn fields_of(payload: &[u8], i: usize) -> (usize, usize, usize, usize) {
        let off = usize::from(u16::from_le_bytes([payload[2 * i], payload[2 * i + 1]]));
        let slen = usize::from(payload[off + 1]);
        let ulen_at = off + 2 + slen;
        (off, off + 1, ulen_at, ulen_at + 1)
    }

    #[test]
    fn a_coded_group_that_lies_is_refused() {
        let imgs = suite_images(8, 9);
        let (all, _, payload) = coded(&imgs);
        let count = imgs.len();
        let refused = |p: &[u8], what: &str| {
            let mut b = GroupBuf::new();
            assert!(
                decode_record_group(p, count, all.len(), &mut b).is_err(),
                "{what}: whole"
            );
        };
        // An offset past the payload, and one into the table.
        let mut p = payload.clone();
        p[6..8].copy_from_slice(&0xfff0u16.to_le_bytes());
        refused(&p, "an offset past the payload");
        let mut b = GroupBuf::new();
        assert!(decode_record(&p, count, 3, &mut b).is_err());
        assert!(record_group_seek(&p, count, b"0000000000000003").is_err());
        let mut p = payload.clone();
        p[6..8].copy_from_slice(&2u16.to_le_bytes());
        refused(&p, "an offset into the table");
        // A prefix longer than the first head.
        let mut p = payload.clone();
        let (plen_at, _, _, _) = fields_of(&payload, 2);
        p[plen_at] = 60;
        refused(&p, "a prefix past the first head");
        let mut b = GroupBuf::new();
        assert!(decode_record(&p, count, 2, &mut b).is_err());
        // An image length that is not whole words, and one the tail passes.
        let mut p = payload.clone();
        let (_, _, ulen_at, _) = fields_of(&payload, 1);
        p[ulen_at] += 1;
        refused(&p, "ulen not a multiple of four");
        let mut p = payload.clone();
        p[ulen_at] = 4;
        refused(&p, "a tail past its image");
        // A compressed tail cut short decodes to the wrong length or not at
        // all; the first record coded against anything is not the
        // dictionary.
        let mut p = payload.clone();
        let (_, _, _, clen_at) = fields_of(&payload, 1);
        assert!(
            usize::from(p[clen_at]) < imgs[1].len(),
            "record 1 is compressed"
        );
        p[clen_at] -= 1;
        refused(&p, "a tail cut short");
        let mut p = payload.clone();
        let (plen_at, _, _, _) = fields_of(&payload, 0);
        p[plen_at] = 1;
        refused(&p, "the first record not stored whole");
        // What was not touched still decodes.
        let mut b = GroupBuf::new();
        assert_eq!(
            decode_record_group(&payload, count, all.len(), &mut b).unwrap(),
            &all[..]
        );
    }

    /// The head walk refuses what the decoded walk refuses, from the heads:
    /// a first key that is not the one named, keys out of order, a key at or
    /// past the next group's first, and lengths that do not sum to the
    /// group's; and passes the group as written.
    #[test]
    fn a_coded_groups_heads_are_checked_as_its_decoded_records_would_be() {
        let imgs = suite_images(30, 51);
        let (a, b) = imgs.split_at(15);
        let (all_a, _, pay_a) = coded(a);
        let (_, _, pay_b) = coded(b);
        let key_a = record_key(&a[0], 0).unwrap().to_vec();
        let key_b = record_key(&b[0], 0).unwrap().to_vec();
        let bind = 0x3131_0003;
        let groups = [
            PageGroup {
                count: 15,
                payload: &pay_a,
                ulen: all_a.len() as u32,
                stored: false,
                coded: true,
                key: b"",
            },
            PageGroup {
                count: 15,
                payload: &pay_b,
                ulen: all_a.len() as u32,
                stored: false,
                coded: true,
                key: &key_b,
            },
        ];
        let mut page = [0u8; ZPAGE];
        encode_page(&mut page, 0, &groups, bind).unwrap();
        let view = parse_page(&page, bind).unwrap();
        let entry = FenceEntry {
            index: 0,
            first_rank: 0,
            count: 30,
            page: 0,
            key: &key_a,
        };
        for j in 0..2 {
            assert_eq!(
                check_coded_group(view.payload(j).unwrap(), &view, j, &entry),
                Ok(())
            );
            let mut buf = GroupBuf::new();
            let dec = decode_group(&view, j, &mut buf).unwrap();
            assert_eq!(check_group(dec, &view, j, &entry), Ok(()));
        }
        // The entry names another first key: group 0 refused, group 1 not.
        let wrong = FenceEntry {
            key: &key_b,
            ..entry
        };
        assert!(check_coded_group(view.payload(0).unwrap(), &view, 0, &wrong).is_err());
        assert_eq!(
            check_coded_group(view.payload(1).unwrap(), &view, 1, &wrong),
            Ok(())
        );
        // Record 3's suffix made to sort below record 2's: not ascending.
        let (_, _, _, _) = fields_of(&pay_a, 3);
        let mut p = pay_a.clone();
        let off = usize::from(u16::from_le_bytes([p[6], p[7]]));
        let slen = usize::from(p[off + 1]);
        p[off + 2 + slen - 1] = b'0';
        let r = check_coded_group(&p, &view, 0, &entry);
        assert!(matches!(r, Err(ZError::Walk(_))), "{r:?}");
        // Group 0's last key raised past group 1's first: not below the next.
        let mut p = pay_a.clone();
        let off = usize::from(u16::from_le_bytes([p[28], p[29]]));
        let slen = usize::from(p[off + 1]);
        p[off + 2 + slen - 1] = b'9';
        p[off + 2 + slen - 2] = b'9';
        let r = check_coded_group(&p, &view, 0, &entry);
        assert!(matches!(r, Err(ZError::Walk(_))), "{r:?}");
        // An image length changed: the sum is not the group's.
        let mut p = pay_a.clone();
        let (_, _, ulen_at, _) = fields_of(&pay_a, 5);
        p[ulen_at] += 4;
        assert_eq!(
            check_coded_group(&p, &view, 0, &entry),
            Err(ZError::DecodedLen)
        );
    }

    #[test]
    fn a_page_of_the_record_coding_is_read_as_one_of_the_block_coding() {
        let imgs = suite_images(40, 31);
        let (all, _, payload) = coded(&imgs);
        let count = imgs.len() as u16;
        let bind = 0x5151_0001;
        let block = lz4_flex::block::compress(&all);
        for coding in [ZCODING_RECORD, ZCODING_BLOCK] {
            let pay: &[u8] = if coding == ZCODING_RECORD {
                &payload
            } else {
                &block
            };
            let coded = coding == ZCODING_RECORD;
            let groups = [PageGroup {
                count,
                payload: pay,
                ulen: all.len() as u32,
                stored: false,
                coded,
                key: b"",
            }];
            let mut page = [0u8; ZPAGE];
            encode_page(&mut page, 0, &groups, bind).unwrap();
            let view = parse_page(&page, bind).unwrap();
            let g = view.group(0).unwrap();
            assert_eq!((g.coded, g.ulen as usize), (coded, all.len()));
            let mut buf = GroupBuf::new();
            assert_eq!(decode_group(&view, 0, &mut buf).unwrap(), &all[..]);
            assert!(walk_group(&all, imgs.len(), record_key(&imgs[0], 0), None).is_ok());
        }
        // A stored group is never coded: refused by the encoder, and by the
        // view when the bit is set on one after the fact. A smaller group,
        // since stored it must fit the page as it is.
        let small = suite_images(16, 32);
        let (all, _, _) = coded(&small);
        let count = small.len() as u16;
        let stored = [PageGroup {
            count,
            payload: &all,
            ulen: all.len() as u32,
            stored: true,
            coded: true,
            key: b"",
        }];
        let mut page = [0u8; ZPAGE];
        assert!(encode_page(&mut page, 0, &stored, bind).is_err());
        let stored = [PageGroup {
            coded: false,
            ..stored[0]
        }];
        encode_page(&mut page, 0, &stored, bind).unwrap();
        let at = ZHDR + D_ULEN4;
        let word = u16::from_le_bytes([page[at], page[at + 1]]) | D_CODED;
        page[at..at + 2].copy_from_slice(&word.to_le_bytes());
        resign_page(&mut page, bind);
        assert!(matches!(parse_page(&page, bind), Err(ZError::Header(_))));
    }

    #[test]
    fn the_fill_under_the_record_coding_builds_pages_the_reader_decodes() {
        let imgs = suite_images(500, 41);
        let mut s = FillScratch::new();
        s.coding = ZCODING_RECORD;
        let bind = 0x7777_0002;
        let mut rank = 0usize;
        let mut page = [0u8; ZPAGE];
        let mut buf = GroupBuf::new();
        let mut seen = 0usize;
        let mut stored_groups = 0usize;
        let mut groups = 0usize;
        while rank < imgs.len() {
            let Fill::Page(built) = fill_page(&imgs, rank, bind, &mut s, &mut page) else {
                panic!("a page at rank {rank}");
            };
            groups += built.groups;
            let view = parse_page(&page, bind).unwrap();
            let mut at = rank;
            for j in 0..view.groups() {
                let g = view.group(j).unwrap();
                assert_eq!(
                    g.coded, !g.stored,
                    "a compressed group of the suite's is coded"
                );
                let n = usize::from(g.count);
                let dec = decode_group(&view, j, &mut buf).unwrap();
                let mut want = Vec::new();
                for im in &imgs[at..at + n] {
                    want.extend_from_slice(im);
                }
                assert_eq!(dec, &want[..], "page at {rank}, group {j}");
                // A point read of every record of a coded group decodes it
                // alone; a group the coding did not shrink is stored verbatim
                // under either coding, and is sought as a decoded group.
                let payload = view.payload(j).unwrap();
                for (k, im) in imgs[at..at + n].iter().enumerate() {
                    let key = record_key(im, 0).unwrap();
                    if g.stored {
                        stored_groups += 1;
                        let pos = group_seek(dec, n, key).unwrap();
                        assert!(pos.found && pos.index == k);
                        continue;
                    }
                    let pos = record_group_seek(payload, n, key)
                        .unwrap_or_else(|e| panic!("seek in a coded group: {e:?}"));
                    assert!(
                        pos.found && pos.index == k,
                        "record {k} of {n} at rank {at}: {pos:?}, key {:?}",
                        String::from_utf8_lossy(key)
                    );
                    let mut b = GroupBuf::new();
                    assert_eq!(decode_record(payload, n, k, &mut b).unwrap(), &im[..]);
                }
                at += n;
            }
            seen += built.count;
            rank += built.count;
        }
        assert_eq!(seen, imgs.len());
        // The suite's records compress under the coding: the groups stored
        // verbatim are the odd one the fill could not grow, not the rule.
        assert!(
            stored_groups * 4 < groups,
            "{stored_groups} of {groups} groups stored verbatim"
        );
    }

    // ---- constants ------------------------------------------------------

    #[test]
    fn the_constants_are_the_layouts() {
        assert_eq!(ZCAP, 4072);
        assert_eq!(ZHDR + ZCAP, ZPAGE);
        assert_eq!(&ZTAG.to_le_bytes(), b"ZPG2");
        assert_ne!(ZTAG, 0, "a zeroed page must not carry a valid tag");
        assert_eq!(ZNOPAGE, 0xFFFF_FFFF);
        assert_eq!(ZMAX_COUNT, 256);
        assert_eq!(ZMAX_ULEN, 65_536);
        assert_eq!((ZMAX_GROUPS, ZDIR_ENTRY, ZGROUP_BYTES), (16, 8, 2048));
        assert_eq!(&ZMAGIC, b"SUPDZPG2");
        assert_eq!((ZTRAILER, T_CRC, T_ZERO), (4096, 4092, 212));
        // The header's fields tile the first 24 bytes, in the order written,
        // and a directory entry's its eight.
        assert_eq!(
            [P_CRC, P_TAG, P_FIRST, P_COUNT, P_STORED, P_ULEN, P_USED, P_GROUPS, P_ZERO],
            [0, 4, 8, 12, 14, 16, 20, 22, 23]
        );
        assert_eq!([D_FIRST, D_COFF, D_CLEN, D_ULEN4], [0, 2, 4, 6]);
        // The CRC check value of the Castagnoli polynomial, and the engine's
        // CRC held to the definition above.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32(b"123456789"), 0xE306_9283);
        let mut rng = Rng::new(1);
        for n in [0usize, 1, 3, 7, 8, 9, 63, 64, 65, 4092, 4096, 5000] {
            let d = rng.bytes(n);
            assert_eq!(crc32(&d), crc32c(&d), "{n} bytes");
        }
    }

    // ---- the page -------------------------------------------------------

    #[test]
    fn a_page_round_trips_stored_and_compressed() {
        for (images, stored) in small_groups() {
            let group = images.concat();
            let page = manual_page(&group, images.len() as u16, 77, 0xDEAD_BEEF, stored);
            let v = parse_page(&page, 0xDEAD_BEEF).expect("a good page parses");
            let h = v.header;
            assert_eq!(h.first_rank, 77);
            assert_eq!(h.count as usize, images.len());
            assert_eq!((h.groups, h.stored), (1, u16::from(stored)));
            assert_eq!(h.ulen as usize, group.len());
            let g = v.group(0).unwrap();
            assert_eq!(
                (g.first, g.count as usize, g.stored),
                (0, images.len(), stored)
            );
            assert_eq!(g.ulen as usize, group.len());
            assert_eq!(v.payload(0).unwrap().len(), g.clen as usize);
            assert_eq!(usize::from(h.used), ZDIR_ENTRY + usize::from(g.clen));
            assert!(v.group(1).is_none() && v.key(0).is_none() && v.key(1).is_none());
            let mut buf = GroupBuf::new();
            let back = decode_group(&v, 0, &mut buf).expect("a good page decodes");
            assert_eq!(back, &group[..]);
            assert!(
                (back.as_ptr() as usize).is_multiple_of(4),
                "a decoded group is aligned for extents"
            );
            walk_group(back, images.len(), None, None).expect("the group walks");
            assert!(matches!(
                decode_group(&v, 1, &mut buf),
                Err(ZError::Header(_))
            ));
        }
    }

    #[test]
    fn a_page_of_several_groups_round_trips_and_is_found_by_key_and_index() {
        // The suite's records cut into four groups, one of them stored, with
        // the keys the directory names each by: every group decodes to its
        // records on its own, the directory finds a key's group and an
        // index's group, and the layout is what the table says.
        let images = suite_images(40, 5);
        let cuts = [0usize, 7, 20, 28, 40];
        let groups: Vec<Vec<u8>> = cuts
            .windows(2)
            .map(|w| images[w[0]..w[1]].concat())
            .collect();
        let keys: Vec<Vec<u8>> = cuts[..4]
            .iter()
            .map(|&i| record_key(&images[i], 0).unwrap().to_vec())
            .collect();
        let spec: Vec<(&[u8], u16, bool, &[u8])> = (0..4)
            .map(|j| {
                (
                    groups[j].as_slice(),
                    (cuts[j + 1] - cuts[j]) as u16,
                    j == 2,
                    keys[j].as_slice(),
                )
            })
            .collect();
        let page = manual_pages(&spec, 100, 9);
        let v = parse_page(&page, 9).expect("parses");
        assert_eq!(
            (v.groups(), v.header.count, v.header.first_rank),
            (4, 40, 100)
        );
        assert_eq!(v.header.stored, 1 << 2);
        assert_eq!(v.header.ulen as usize, images.concat().len());
        let mut buf = GroupBuf::new();
        let mut at = ZHDR + 4 * ZDIR_ENTRY;
        for j in 0..4 {
            let g = v.group(j).unwrap();
            assert_eq!(
                (g.first as usize, g.count as usize),
                (cuts[j], cuts[j + 1] - cuts[j])
            );
            assert_eq!((g.stored, g.ulen as usize), (j == 2, groups[j].len()));
            assert_eq!(
                decode_group(&v, j, &mut buf).unwrap(),
                &groups[j][..],
                "group {j}"
            );
            if j > 0 {
                assert_eq!(v.key(j), Some(keys[j].as_slice()));
                // The key where the table says: a length and the bytes.
                assert_eq!(
                    u16::from_le_bytes([page[at], page[at + 1]]) as usize,
                    keys[j].len()
                );
                assert_eq!(&page[at + 2..at + 2 + keys[j].len()], &keys[j][..]);
                at += 2 + keys[j].len();
            }
        }
        // The payloads from the keys' end rounded up to four, each after
        // the last rounded the same, the last ending at the used bytes.
        let mut coff = at.next_multiple_of(4);
        for j in 0..4 {
            let g = v.group(j).unwrap();
            assert_eq!(usize::from(g.coff), coff, "group {j}'s payload");
            coff = (coff + usize::from(g.clen)).next_multiple_of(4);
        }
        let g3 = v.group(3).unwrap();
        assert_eq!(
            ZHDR + usize::from(v.header.used),
            usize::from(g3.coff) + usize::from(g3.clen)
        );
        // Found by index and by key: every record of every group, and the
        // keys between.
        for (i, image) in images.iter().enumerate().take(40) {
            let j = cuts.iter().rposition(|&c| c <= i).unwrap();
            assert_eq!(v.group_of_index(i), j, "index {i}");
            let k = record_key(image, 0).unwrap();
            assert_eq!(v.group_of_key(k), j, "key of {i}");
            let mut above = k.to_vec();
            above.push(0);
            assert_eq!(v.group_of_key(&above), j, "a key just above {i}'s");
        }
        assert_eq!(
            v.group_of_key(b""),
            0,
            "a key below the page's first is group 0's to refuse"
        );
        assert_eq!(v.group_of_key(&[0xff; 20]), 3);
        // And the whole page verifies at its entry, group by group.
        let entry = FenceEntry {
            index: 0,
            first_rank: 100,
            count: 40,
            page: 0,
            key: &keys[0],
        };
        assert_eq!(
            verify_page(&page, 9, &entry, &mut buf).map(|h| h.groups),
            Ok(4)
        );
    }

    /// A buffer in a slab's cell (`GroupBuf::in_cell`) is the cell's bytes
    /// for every group that fits it, the decoder's room past the group
    /// inside the cell and no further; a group that does not fit sends the
    /// buffer to words of its own in whole pages, and the cell goes back to
    /// its slab.
    #[test]
    fn a_group_buffer_in_a_cell_leaves_it_for_a_group_that_does_not_fit() {
        let slab = super::super::blocks::Slab::new(2240, 2);
        let mut buf = GroupBuf::in_cell(slab.take().unwrap());
        assert!(buf.is_cell() && buf.capacity_bytes() == 2240);
        let at = buf.bytes_for(2048).as_ptr() as usize;
        assert_eq!(at % 8, 0, "aligned for the extent arrays");
        buf.bytes_for(2048).fill(3);
        assert!(buf.is_cell(), "a group that fits stays in the cell");
        assert!(buf.bytes(2048).iter().all(|&b| b == 3));
        assert_eq!(buf.bytes(2048).as_ptr() as usize, at);
        assert_eq!(
            buf.bytes_with_slack(2048, 192).len(),
            2240,
            "the decoder's room, inside the cell"
        );
        assert_eq!(
            buf.bytes_with_slack(2100, 192).len(),
            2240,
            "and no further than the cell"
        );
        assert_eq!(buf.stride_key().as_ptr() as usize, at);
        // A group past the cell: words, in whole pages, the cell back.
        let other = slab.take().expect("the slab's second cell");
        assert!(slab.take().is_none(), "both cells out");
        assert_eq!(buf.bytes_for(3000).len(), 3000);
        assert!(!buf.is_cell() && buf.capacity_bytes() == ZPAGE);
        let back = slab.take().expect("the cell handed back");
        assert_eq!(back.as_ptr() as usize, at);
        assert_ne!(other.as_ptr() as usize, at);
        // A buffer of words is as it was: no cell, sized by its groups.
        let mut plain = GroupBuf::new();
        assert!(!plain.is_cell() && plain.capacity_bytes() == 0);
        plain.bytes_for(10);
        assert!(!plain.is_cell() && plain.capacity_bytes() == ZPAGE);
    }

    #[test]
    fn a_decode_buffer_is_as_large_as_the_largest_group_it_has_held_in_whole_pages() {
        // A thread keeps one for its life and a callback that reads again
        // makes one of its own, so a buffer made at the largest group's size
        // was 64 KiB zeroed for each, to hold groups of a few kilobytes.
        let mut buf = GroupBuf::new();
        assert!(buf.words.is_empty(), "an empty buffer holds nothing");
        for (n, words) in [
            (0usize, 0usize),
            (100, ZPAGE / 8),
            (ZPAGE, ZPAGE / 8),
            (ZPAGE + 4, 2 * ZPAGE / 8),
            (500, 2 * ZPAGE / 8),
            (ZMAX_ULEN, ZMAX_ULEN / 8),
            (4, ZMAX_ULEN / 8),
        ] {
            let got = buf.bytes_for(n);
            assert_eq!(got.len(), n);
            assert!(
                (got.as_ptr() as usize).is_multiple_of(8),
                "a group decodes to where extents can be borrowed"
            );
            assert_eq!(buf.words.len(), words, "after a group of {n} bytes");
        }
        // Groups of every size one after another decode to their bytes,
        // whichever way the buffer has grown: small, large, small.
        let mut rng = Rng(0xB0F);
        let mut buf = GroupBuf::new();
        for n in [64usize, 3000, 40, 20_000, 4000, ZMAX_ULEN, 8, 3000] {
            // Entropy for the sizes a page holds, a pattern for the ones it
            // can only hold compressed.
            let group: Vec<u8> = if n <= 4000 {
                rng.bytes(n)
            } else {
                (0..n).map(|i| (i / 7 % 251) as u8).collect()
            };
            let payload = lz4_flex::block::compress(&group);
            assert!(
                payload.len() <= ZCAP - ZDIR_ENTRY,
                "{n}: the group fits a page"
            );
            let page = manual_page(&group, 1, 0, 0, false);
            let v = parse_page(&page, 0).unwrap();
            let back = decode_group(&v, 0, &mut buf).unwrap();
            assert_eq!(back, &group[..], "{n}");
        }
        assert_eq!(
            buf.words.len(),
            ZMAX_ULEN / 8,
            "grown to the largest, not past it"
        );
    }

    #[test]
    fn a_stored_page_is_parsed_in_place_where_aligned_and_copied_where_not() {
        let images = small_groups().remove(1).0;
        let group = images.concat();
        let page = manual_page(&group, images.len() as u16, 0, 5, true);
        let mut buf = GroupBuf::new();
        // The page at the start of an aligned buffer, then one byte in.
        let mut region = vec![0u64; ZPAGE / 8 + 1];
        // SAFETY: `region` owns `len * 8` initialised bytes.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(region.as_mut_ptr() as *mut u8, region.len() * 8)
        };
        bytes[..ZPAGE].copy_from_slice(&page);
        let v = parse_page(&bytes[..ZPAGE], 5).unwrap();
        let payload = v.payload(0).unwrap();
        let g = decode_group(&v, 0, &mut buf).unwrap();
        assert_eq!(g.as_ptr(), payload.as_ptr(), "aligned: lent from the page");
        assert_eq!(g, &group[..]);
        bytes.copy_within(0..ZPAGE, 1);
        let v = parse_page(&bytes[1..1 + ZPAGE], 5).unwrap();
        let payload = v.payload(0).unwrap();
        assert!(!(payload.as_ptr() as usize).is_multiple_of(4));
        let g = decode_group(&v, 0, &mut buf).unwrap();
        assert_ne!(g.as_ptr(), payload.as_ptr(), "misaligned: copied");
        assert!((g.as_ptr() as usize).is_multiple_of(4));
        assert_eq!(g, &group[..]);
        // And the records parse out of it, which is what a misaligned
        // buffer would have turned into "absent".
        walk_group(g, images.len(), None, None).expect("a copied group walks");
    }

    #[test]
    fn a_page_is_read_by_an_independent_decoder() {
        for (images, stored) in small_groups() {
            let group = images.concat();
            let bind = 0x0BAD_F00D;
            let page = manual_page(&group, images.len() as u16, 1234, bind, stored);
            // Field by field, at the offsets the layout's table gives.
            assert_eq!(
                le32(&page, 0),
                crc32c(&page[4..]) ^ bind,
                "crc covers [4, 4096) xor bind32"
            );
            assert_eq!(&page[4..8], b"ZPG2");
            assert_eq!(le32(&page, 8), 1234);
            let le16 = |at: usize| u16::from_le_bytes([page[at], page[at + 1]]) as usize;
            assert_eq!(le16(12), images.len());
            assert_eq!(le16(14), usize::from(stored), "the stored mask");
            assert_eq!(le32(&page, 16) as usize, group.len());
            let used = le16(20);
            assert_eq!((page[22], page[23]), (1, 0), "one group, then zero");
            // The one directory entry: first 0, the payload at 32 (the
            // directory's end, a multiple of four), its length, ulen / 4.
            assert_eq!(le16(24), 0);
            assert_eq!(le16(26), 32);
            let clen = le16(28);
            assert_eq!(le16(30) * 4, group.len());
            assert_eq!(used, 8 + clen);
            if stored {
                assert_eq!(clen, group.len());
                assert_eq!(&page[32..32 + clen], &group[..]);
            } else {
                let mut out = vec![0u8; group.len()];
                let n = lz4_flex::block::decompress_into(&page[32..32 + clen], &mut out).unwrap();
                assert_eq!(&out[..n], &group[..]);
            }
            assert!(
                page[32 + clen..].iter().all(|&b| b == 0),
                "zeros after the payload"
            );
        }
    }

    #[test]
    fn a_flipped_byte_anywhere_in_a_page_is_refused_by_its_crc() {
        for (images, stored) in small_groups() {
            let group = images.concat();
            let page = manual_page(&group, images.len() as u16, 9, 42, stored);
            for at in 0..ZPAGE {
                for flip in [0x01u8, 0x80, 0xFF] {
                    let mut bad = page;
                    bad[at] ^= flip;
                    assert_eq!(
                        parse_page(&bad, 42).err(),
                        Some(ZError::PageCrc),
                        "byte {at} ^ {flip:#x}, stored {stored}"
                    );
                }
            }
        }
    }

    /// What `parse_page` finds wrong with a page, owned, so a page built for
    /// the call can be a temporary.
    fn refused(p: [u8; ZPAGE], bind: u32) -> Option<ZError> {
        parse_page(&p, bind).err()
    }

    #[test]
    fn damage_a_crc_cannot_see_is_refused_by_the_checks_behind_it() {
        let images = small_groups().remove(0).0;
        let group = images.concat();
        let good = manual_page(&group, images.len() as u16, 3, 7, false);
        let clen = usize::from(parse_page(&good, 7).unwrap().group(0).unwrap().clen);
        // A copy of the good page edited and signed again, so that only the
        // checks behind the CRC can see the edit.
        let edit = |f: &dyn Fn(&mut [u8; ZPAGE])| {
            let mut p = good;
            f(&mut p);
            resign_page(&mut p, 7);
            p
        };
        let header = |e: Option<ZError>| matches!(e, Some(ZError::Header(_)));
        assert_eq!(refused(edit(&|_| {}), 7), None);
        // The tag: zero, and one bit off.
        assert_eq!(refused(edit(&|p| p[4..8].fill(0)), 7), Some(ZError::Tag));
        assert_eq!(refused(edit(&|p| p[4] ^= 1), 7), Some(ZError::Tag));
        // The byte that is zero.
        let e = refused(edit(&|p| p[23] |= 2), 7);
        assert!(header(e), "byte 23: {e:?}");
        // The counts and lengths.
        let set16 =
            |at: usize, v: u16| edit(&move |p| p[at..at + 2].copy_from_slice(&v.to_le_bytes()));
        let set32 =
            |at: usize, v: u32| edit(&move |p| p[at..at + 4].copy_from_slice(&v.to_le_bytes()));
        for count in [0u16, 257, u16::MAX] {
            let e = refused(set16(12, count), 7);
            assert!(header(e), "count {count}: {e:?}");
        }
        for ulen in [
            0u32,
            6,
            group.len() as u32 + 1,
            group.len() as u32 + 4,
            65_540,
            u32::MAX,
        ] {
            let e = refused(set32(16, ulen), 7);
            assert!(header(e), "ulen {ulen}: {e:?}");
        }
        for used in [0u16, 7, 4073, u16::MAX] {
            let e = refused(set16(20, used), 7);
            assert!(header(e), "used {used}: {e:?}");
        }
        for groups in [0u8, 2, 17, u8::MAX] {
            let e = refused(edit(&move |p| p[22] = groups), 7);
            assert!(header(e), "groups {groups}: {e:?}");
        }
        // A compressed group marked stored: its clen is not its ulen; and a
        // stored bit for a group the page does not have.
        let e = refused(set16(14, 1), 7);
        assert!(header(e), "{e:?}");
        let e = refused(set16(14, 2), 7);
        assert!(header(e), "{e:?}");
        // The directory: a first that is not zero, a payload not where the
        // keys end, no payload, a ulen of nothing, a ulen past the cap.
        let e = refused(set16(24, 1), 7);
        assert!(header(e), "first: {e:?}");
        for coff in [0u16, 28, 36, 4000] {
            let e = refused(set16(26, coff), 7);
            assert!(header(e), "coff {coff}: {e:?}");
        }
        let e = refused(set16(28, 0), 7);
        assert!(header(e), "clen 0: {e:?}");
        let e = refused(set16(28, clen as u16 + 4), 7);
        assert!(header(e), "clen past the used bytes: {e:?}");
        let e = refused(set16(30, 0), 7);
        assert!(header(e), "ulen4 0: {e:?}");
        let e = refused(set16(30, (ZMAX_ULEN / 4 + 1) as u16), 7);
        assert!(header(e), "ulen4 past the cap: {e:?}");
        // Bytes after the payload.
        assert_eq!(
            refused(edit(&|p| p[32 + clen] = 1), 7),
            Some(ZError::Padding)
        );
        assert_eq!(
            refused(edit(&|p| p[ZPAGE - 1] = 1), 7),
            Some(ZError::Padding)
        );
        // A wrong length is no page.
        assert_eq!(
            parse_page(&good[..ZPAGE - 1], 7).err(),
            Some(ZError::Length)
        );
        assert_eq!(parse_page(&[], 7).err(), Some(ZError::Length));
        // The payload: a ulen the payload does not decode to, either way,
        // the header's sum kept in step so the directory passes.
        let mut buf = GroupBuf::new();
        let ulen4 = (group.len() / 4) as u16;
        let longer = edit(&|p| {
            p[30..32].copy_from_slice(&(ulen4 + 1).to_le_bytes());
            p[16..20].copy_from_slice(&(group.len() as u32 + 4).to_le_bytes());
        });
        let v = parse_page(&longer, 7).expect("the directory agrees with the header");
        assert_eq!(
            decode_group(&v, 0, &mut buf).err(),
            Some(ZError::DecodedLen)
        );
        let shorter = edit(&|p| {
            p[30..32].copy_from_slice(&(ulen4 - 1).to_le_bytes());
            p[16..20].copy_from_slice(&(group.len() as u32 - 4).to_le_bytes());
        });
        let v = parse_page(&shorter, 7).unwrap();
        assert_eq!(decode_group(&v, 0, &mut buf).err(), Some(ZError::Decode));
        // A payload cut short, with the lengths corrected, is not a block.
        let half = edit(&|p| {
            p[28..30].copy_from_slice(&((clen / 2) as u16).to_le_bytes());
            p[20..22].copy_from_slice(&((8 + clen / 2) as u16).to_le_bytes());
            p[32 + clen / 2..].fill(0);
        });
        let v = parse_page(&half, 7).unwrap();
        let e = decode_group(&v, 0, &mut buf).err();
        assert!(
            matches!(e, Some(ZError::Decode | ZError::DecodedLen)),
            "{e:?}"
        );
        // A page of two groups: the keys between the directory and the
        // payloads, and the padding around them, are checked too. The
        // second key empty, past the used bytes, not above the first
        // group's; the second payload not at the first's end rounded up;
        // the padding after the keys and between the payloads not zero.
        let two = suite_images(20, 6);
        let (g0, g1) = (two[..12].concat(), two[12..].concat());
        let k1 = record_key(&two[12], 0).unwrap();
        let page2 = manual_pages(&[(&g0, 12, false, &[]), (&g1, 8, false, k1)], 0, 7);
        let v2 = parse_page(&page2, 7).unwrap();
        let (e0, e1) = (v2.group(0).unwrap(), v2.group(1).unwrap());
        let keys_at = ZHDR + 2 * ZDIR_ENTRY;
        assert_eq!(&page2[keys_at + 2..keys_at + 2 + k1.len()], k1);
        let edit2 = |f: &dyn Fn(&mut [u8; ZPAGE])| {
            let mut p = page2;
            f(&mut p);
            resign_page(&mut p, 7);
            p
        };
        type Lie = Box<dyn Fn(&mut [u8; ZPAGE])>;
        let lies: Vec<(&str, Lie)> = vec![
            (
                "an empty second key",
                Box::new(move |p| p[keys_at..keys_at + 2].fill(0)),
            ),
            (
                "a second key past the used bytes",
                Box::new(move |p| p[keys_at..keys_at + 2].copy_from_slice(&4000u16.to_le_bytes())),
            ),
            (
                "the second first not above the first",
                Box::new(|p| p[ZHDR + ZDIR_ENTRY..ZHDR + ZDIR_ENTRY + 2].fill(0)),
            ),
            (
                "the second payload not at the first's end",
                Box::new(move |p| {
                    let at = ZHDR + ZDIR_ENTRY + D_COFF;
                    p[at..at + 2].copy_from_slice(&(e1.coff + 4).to_le_bytes());
                }),
            ),
        ];
        for (what, f) in &lies {
            let e = refused(edit2(f.as_ref()), 7);
            assert!(header(e), "{what}: {e:?}");
        }
        let pad_after_keys = keys_at + 2 + k1.len();
        if pad_after_keys < usize::from(e0.coff) {
            assert_eq!(
                refused(edit2(&|p| p[pad_after_keys] = 1), 7),
                Some(ZError::Padding),
                "the padding after the keys"
            );
        }
        let between = usize::from(e0.coff) + usize::from(e0.clen);
        if between < usize::from(e1.coff) {
            assert_eq!(
                refused(edit2(&|p| p[between] = 1), 7),
                Some(ZError::Padding),
                "the padding between the payloads"
            );
        }
        // The view, which trusts the CRC, hands the keys out as they lie.
        assert_eq!(page_view(&page2).unwrap().key(1), Some(k1));
    }

    #[test]
    fn a_view_refuses_a_header_or_a_directory_no_page_may_carry() {
        // `page_view` is public and reads a page a first decode has passed,
        // so it holds the header and the directory to what a page may carry
        // itself: an unchecked length past the buffer is a panic, and a
        // stored group whose lengths disagree is a slice of the wrong size.
        let images = small_groups().remove(0).0;
        let good = manual_page(&images.concat(), images.len() as u16, 3, 7, false);
        let mut buf = GroupBuf::new();
        let v = page_view(&good).unwrap();
        assert!(decode_group(&v, 0, &mut buf).is_ok(), "the control");
        let set16 = |at: usize, val: u16| {
            let mut p = good;
            p[at..at + 2].copy_from_slice(&val.to_le_bytes());
            p
        };
        let set32 = |at: usize, val: u32| {
            let mut p = good;
            p[at..at + 4].copy_from_slice(&val.to_le_bytes());
            p
        };
        for (what, bad) in [
            ("ulen past the cap", set32(16, ZMAX_ULEN as u32 + 4)),
            ("ulen huge", set32(16, u32::MAX)),
            ("ulen not a multiple of four", set32(16, v.header.ulen + 2)),
            ("ulen zero", set32(16, 0)),
            ("count zero", set16(12, 0)),
            ("count past the most", set16(12, ZMAX_COUNT as u16 + 1)),
            ("used zero", set16(20, 0)),
            ("used past the cap", set16(20, ZCAP as u16 + 1)),
            ("stored, but the lengths disagree", set16(14, 1)),
            ("a directory entry's first off", set16(24, 5)),
            ("a payload nowhere", set16(26, 4090)),
            ("no payload", set16(28, 0)),
            ("a group of no bytes", set16(30, 0)),
        ] {
            let got = page_view(&bad).err();
            assert!(matches!(got, Some(ZError::Header(_))), "{what}: {got:?}");
        }
        let mut groups = good;
        groups[22] = 0;
        assert!(matches!(page_view(&groups).err(), Some(ZError::Header(_))));
    }
    #[test]
    fn a_page_of_another_form_file_fails_its_own_check() {
        let images = small_groups().remove(0).0;
        let page = manual_page(&images.concat(), images.len() as u16, 0, 1, false);
        assert!(parse_page(&page, 1).is_ok());
        for other in [0u32, 2, 3, 0x8000_0001, u32::MAX] {
            assert_eq!(
                parse_page(&page, other).err(),
                Some(ZError::PageCrc),
                "bind32 {other}"
            );
        }
    }

    #[test]
    fn a_pages_view_is_its_header_and_directory_and_checks_only_those() {
        for (group, stored) in small_groups() {
            let group = group.concat();
            let page = manual_page(&group, 3, 17, 0xBEEF, stored);
            let full = parse_page(&page, 0xBEEF).unwrap();
            // The view agrees with the full parse where the page is whole.
            let v = page_view(&page).unwrap();
            assert_eq!(
                (v.header, v.group(0), v.payload(0)),
                (full.header, full.group(0), full.payload(0))
            );
            // It does not look at the CRC, the tag or the padding: those are
            // the first decode's, and a read of a verified page must not pay
            // for them again. A damaged page's view is the first decode's to
            // refuse, which is why a page is viewed only once it has passed.
            let mut damaged = page;
            damaged[ZPAGE - 1] ^= 0xFF;
            assert_eq!(parse_page(&damaged, 0xBEEF).err(), Some(ZError::PageCrc));
            let dv = page_view(&damaged).unwrap();
            assert_eq!((dv.header, dv.payload(0)), (full.header, full.payload(0)));
            // What it does check is what it hands back is bounded: the
            // header's and the directory's values, and the page's length.
            let mut wild = page;
            wild[P_USED..P_USED + 2].copy_from_slice(&u16::MAX.to_le_bytes());
            assert!(matches!(page_view(&wild), Err(ZError::Header(_))));
            let mut zero_count = page;
            zero_count[P_COUNT..P_COUNT + 2].copy_from_slice(&0u16.to_le_bytes());
            assert!(matches!(page_view(&zero_count), Err(ZError::Header(_))));
            assert_eq!(page_view(&page[..ZPAGE - 1]).err(), Some(ZError::Length));
            assert_eq!(page_view(&[]).err(), Some(ZError::Length));
        }
    }

    #[test]
    fn a_fence_finds_the_entry_a_key_falls_among_and_none_below_the_first() {
        let keys: [&[u8]; 5] = [b"b", b"d", b"dd", b"f", b"ff"];
        let mut f = ZFence::new();
        for (i, k) in keys.iter().enumerate() {
            f.push(i as u32 * 10, i as u32, k).unwrap();
        }
        // The same answers from the search over the keys and from the tree.
        let check = |f: &ZFence| {
            assert_eq!(
                f.entry_of(b""),
                None,
                "the empty key sorts below every other"
            );
            assert_eq!(f.entry_of(b"a"), None);
            for (i, k) in keys.iter().enumerate() {
                assert_eq!(f.entry_of(k), Some(i), "an entry's own key");
            }
            assert_eq!(f.entry_of(b"c"), Some(0));
            assert_eq!(f.entry_of(b"d\x00"), Some(1));
            assert_eq!(
                f.entry_of(b"dc"),
                Some(1),
                "a key among an entry's extensions"
            );
            assert_eq!(f.entry_of(b"de"), Some(2));
            assert_eq!(f.entry_of(b"e"), Some(2));
            assert_eq!(f.entry_of(b"zzzz"), Some(4));
        };
        assert!(!f.indexed());
        check(&f);
        let mut fi = f.clone();
        fi.index();
        assert!(fi.indexed());
        check(&fi);
        // An empty fence key, the first, is at or below every key, the empty
        // one included, and a long key is compared as the engine compares it.
        let mut g = ZFence::new();
        g.push(0, 0, b"").unwrap();
        g.push(5, 1, &[7u8; 40]).unwrap();
        let check = |g: &ZFence| {
            assert_eq!(g.entry_of(b""), Some(0));
            assert_eq!(g.entry_of(&[3u8]), Some(0));
            assert_eq!(g.entry_of(b"a"), Some(1));
            assert_eq!(g.entry_of(&[7u8; 40]), Some(1));
            assert_eq!(g.entry_of(&[7u8; 39]), Some(0));
            assert_eq!(g.entry_of(&[7u8; 41]), Some(1));
        };
        check(&g);
        g.index();
        check(&g);
        // The shared bytes are the one non-empty key's, so its word is zero
        // like the empty key's, and the tie is told apart by the whole key.
        assert_eq!(g.shared, vec![7u8; 40]);
        assert_eq!(ZFence::new().entry_of(b"k"), None);
        let mut e = ZFence::new();
        e.index();
        assert!(!e.indexed());
        assert_eq!(e.entry_of(b"k"), None);
        // A push after the index takes the tree down: it was of the entries
        // before.
        g.push(9, 2, &[9u8; 3]).unwrap();
        assert!(!g.indexed());
        assert_eq!(g.entry_of(&[8u8]), Some(1));
        // The fence's heap: four words an entry, one more for the offsets,
        // and the keys; indexed, a line of eight nodes and a word a node
        // beside them, and the shared bytes (none here).
        assert_eq!(f.heap_bytes(), 4 * (5 + 5 + 6) + 1 + 1 + 2 + 1 + 2);
        assert_eq!(fi.heap_bytes(), f.heap_bytes() + 64 + 4 * 6);
    }

    /// The tree's search (`index`) and the word tree's (`index_words`)
    /// answer as the search over the keys for
    /// every shape of fence -- the suite's digit keys with their long
    /// shared prefix, an empty first key, random keys of every length,
    /// keys tied on the word and told apart beyond it, a key that is
    /// exactly the shared bytes, one entry, none -- and every kind of key:
    /// each entry's, its neighbours a byte either side, its extensions and
    /// truncations, the shared bytes themselves and keys below and above
    /// them all, and random ones.
    #[test]
    fn the_trees_search_answers_as_the_search_over_the_keys() {
        let mut rng = Rng(0xF3A1);
        let digits = |n: u32, step: u32| -> Vec<Vec<u8>> {
            (0..n)
                .map(|i| format!("{:016}", i * step + 3).into_bytes())
                .collect()
        };
        let mut fences: Vec<Vec<Vec<u8>>> = Vec::new();
        fences.push(digits(3000, 7));
        // The word tree's heights change at 8, 80, 728 and 6,560 entries:
        // a fence at each side of each.
        for n in [8, 9, 80, 81, 728, 729, 6560, 6561] {
            fences.push(digits(n, 3));
        }
        let mut with_empty = digits(500, 13);
        with_empty[0].clear();
        fences.push(with_empty);
        let mut random: Vec<Vec<u8>> = (0..2000)
            .map(|_| {
                let len = 1 + rng.below(24);
                (0..len).map(|_| rng.word() as u8).collect()
            })
            .collect();
        random.sort();
        random.dedup();
        fences.push(random);
        // Every word the same: the whole fence one tie, told apart by the
        // tail.
        let mut tied: Vec<Vec<u8>> = (0..300)
            .map(|i| {
                let mut k = b"shared-prefix-ABCDEFGH".to_vec();
                k.extend_from_slice(format!("{i:03}").as_bytes());
                k
            })
            .collect();
        tied.sort();
        fences.push(tied);
        // Words from a small alphabet, so some tie and most do not.
        let mut mixed: Vec<Vec<u8>> = (0..1500)
            .map(|_| {
                let mut k = b"pre/".to_vec();
                for _ in 0..8 {
                    k.push(b'a' + rng.below(3) as u8);
                }
                for _ in 0..rng.below(4) {
                    k.push(rng.word() as u8);
                }
                k
            })
            .collect();
        mixed.sort();
        mixed.dedup();
        fences.push(mixed);
        fences.push(vec![Vec::new()]);
        fences.push(vec![b"only".to_vec()]);
        fences.push(vec![b"ab".to_vec(), b"abc".to_vec(), b"abd".to_vec()]);
        fences.push(vec![Vec::new(), b"ab".to_vec(), b"abc".to_vec()]);
        for keys in &fences {
            let mut f = ZFence::new();
            for (i, k) in keys.iter().enumerate() {
                f.push(i as u32 * 10, i as u32, k).unwrap();
            }
            let mut fw = f.clone();
            f.index();
            assert!(f.indexed());
            fw.index_words();
            assert_eq!(fw.worded(), !keys.is_empty());
            assert!(!fw.indexed(), "a fence is searched one way");
            if !keys.is_empty() {
                // The least height with room for every entry, and the
                // tree's slots exactly that height's.
                let h = usize::from(fw.words_h);
                let slots = |h: u32| 9usize.pow(h) - 1;
                assert!(h >= 1 && keys.len() <= slots(h as u32));
                assert!(h == 1 || keys.len() > slots(h as u32 - 1));
                assert_eq!(fw.words.len(), slots(h as u32));
            }
            assert_eq!(
                fw.shared, f.shared,
                "the words' shared bytes are the tree's"
            );
            let shared = f.shared.clone();
            let mut probes: Vec<Vec<u8>> = vec![
                Vec::new(),
                vec![0],
                vec![255; 30],
                shared.clone(),
                [shared.as_slice(), &[0]].concat(),
                [shared.as_slice(), &[255]].concat(),
            ];
            if !shared.is_empty() {
                probes.push(shared[..shared.len() - 1].to_vec());
                let mut below = shared.clone();
                *below.last_mut().unwrap() = below.last().unwrap().wrapping_sub(1);
                probes.push(below);
                let mut above = shared.clone();
                *above.last_mut().unwrap() = above.last().unwrap().wrapping_add(1);
                probes.push(above);
            }
            for k in keys {
                probes.push(k.clone());
                if let Some(last) = k.last() {
                    let mut below = k.clone();
                    *below.last_mut().unwrap() = last.wrapping_sub(1);
                    probes.push(below);
                    let mut above = k.clone();
                    *above.last_mut().unwrap() = last.wrapping_add(1);
                    probes.push(above);
                    probes.push(k[..k.len() - 1].to_vec());
                }
                probes.push([k.as_slice(), &[0]].concat());
                probes.push([k.as_slice(), &[255]].concat());
            }
            for _ in 0..300 {
                let len = rng.below(31);
                probes.push((0..len).map(|_| rng.word() as u8).collect());
                let mut k = shared.clone();
                for _ in 0..rng.below(12) {
                    k.push(rng.word() as u8);
                }
                probes.push(k);
            }
            for key in &probes {
                assert_eq!(
                    f.entry_of(key),
                    f.entry_of_search(key),
                    "fence of {} keys (shared {:?}), key {key:?}",
                    keys.len(),
                    String::from_utf8_lossy(&shared)
                );
                assert_eq!(
                    fw.entry_of(key),
                    f.entry_of_search(key),
                    "word tree: fence of {} keys (shared {:?}), key {key:?}",
                    keys.len(),
                    String::from_utf8_lossy(&shared)
                );
            }
            // A push after the words takes them down, as it takes the tree.
            let mut fp = fw.clone();
            fp.push(u32::MAX - 1, u32::MAX - 1, &[255u8; 32]).unwrap();
            assert!(!fp.worded());
            assert_eq!(fp.entry_of(b"\x01"), fp.entry_of_search(b"\x01"));
        }
    }

    /// The search from a hint (`entry_of_from`) answers as the search from
    /// the top for every key and every hint: the hint itself, the entries
    /// either side of the answer, the ends, and hints past the fence; over
    /// a fence whose first key is empty too.
    #[test]
    fn the_hinted_entry_search_answers_as_the_search_from_the_top() {
        let mut fences = Vec::new();
        let mut f = ZFence::new();
        let keys: Vec<Vec<u8>> = (0..300u32)
            .map(|i| format!("{:08}", i * 7 + 3).into_bytes())
            .collect();
        for (i, k) in keys.iter().enumerate() {
            f.push(i as u32 * 10, i as u32, k).unwrap();
        }
        fences.push((f, keys.clone()));
        let mut g = ZFence::new();
        g.push(0, 0, b"").unwrap();
        for (i, k) in keys.iter().enumerate().skip(1) {
            g.push(i as u32 * 10, i as u32, k).unwrap();
        }
        fences.push((g, keys));
        for (f, keys) in &fences {
            let n = f.len();
            let mut probes: Vec<Vec<u8>> = vec![Vec::new(), b"0".to_vec(), b"9".repeat(12)];
            for k in keys {
                probes.push(k.clone());
                if let Some(last) = k.last() {
                    let mut below = k.clone();
                    *below.last_mut().unwrap() = last.wrapping_sub(1);
                    probes.push(below);
                }
                let mut above = k.clone();
                above.push(0);
                probes.push(above);
            }
            for key in &probes {
                let want = f.entry_of(key);
                let mut froms = vec![0usize, 1, n / 2, n - 1, n, n + 7, usize::MAX];
                if let Some(e) = want {
                    froms.extend([
                        e,
                        e.saturating_sub(1),
                        e.saturating_sub(5),
                        (e + 1).min(n),
                        (e + 3).min(n),
                    ]);
                }
                for from in froms {
                    assert_eq!(f.entry_of_from(key, from), want, "key {key:?} from {from}");
                }
            }
        }
        assert_eq!(ZFence::new().entry_of_from(b"k", 0), None);
    }

    #[test]
    fn a_zeroed_page_cannot_pass() {
        let zero = [0u8; ZPAGE];
        for bind in [0u32, 1, 0xFFFF_FFFF, 0x1234_5678] {
            assert!(parse_page(&zero, bind).is_err(), "bind32 {bind:#x}");
        }
        // Even where the CRC happens to be the zero page's, the tag is not.
        let coincidence = crc32(&zero[4..]);
        assert_eq!(parse_page(&zero, coincidence).err(), Some(ZError::Tag));
    }

    #[test]
    fn a_page_is_refused_groups_it_would_not_write() {
        let mut page = [0xAAu8; ZPAGE];
        let payload = [1u8, 2, 3, 4];
        let big = [0u8; ZCAP + 1];
        let g = PageGroup {
            count: 1,
            payload: &payload,
            ulen: 8,
            stored: false,
            coded: false,
            key: b"a",
        };
        fn one<'a>(bad: PageGroup<'a>) -> Vec<PageGroup<'a>> {
            vec![bad]
        }
        let refusals: Vec<(&str, Vec<PageGroup<'_>>)> = vec![
            ("no groups", Vec::new()),
            ("a group of no records", one(PageGroup { count: 0, ..g })),
            ("a ulen of nothing", one(PageGroup { ulen: 0, ..g })),
            (
                "a ulen not a multiple of four",
                one(PageGroup { ulen: 6, ..g }),
            ),
            ("a ulen past the cap", one(PageGroup { ulen: 65_540, ..g })),
            ("no payload", one(PageGroup { payload: &[], ..g })),
            (
                "a payload past the page",
                one(PageGroup { payload: &big, ..g }),
            ),
            (
                "stored, lengths disagreeing",
                one(PageGroup { stored: true, ..g }),
            ),
            (
                "the records past the most a page holds",
                vec![
                    PageGroup { count: 200, ..g },
                    PageGroup {
                        count: 57,
                        key: b"b",
                        ..g
                    },
                ],
            ),
            ("keys not ascending", vec![g, PageGroup { key: b"a", ..g }]),
            (
                "an empty key after the first",
                vec![g, PageGroup { key: b"", ..g }],
            ),
            (
                "more groups than a page holds",
                (0..17).map(|_| g).collect(),
            ),
        ];
        for (what, groups) in &refusals {
            assert!(encode_page(&mut page, 0, groups, 0).is_err(), "{what}");
        }
        // Groups whose payloads do not fit the page together.
        let half = vec![0u8; ZCAP / 2];
        let twice = vec![
            PageGroup {
                payload: &half,
                ..g
            },
            PageGroup {
                payload: &half,
                key: b"b",
                ..g
            },
        ];
        assert!(encode_page(&mut page, 0, &twice, 0).is_err());
        assert!(
            page.iter().all(|&b| b == 0xAA),
            "a refused page is left alone"
        );
        // A stored group with the lengths agreeing is written.
        let stored = PageGroup {
            stored: true,
            coded: false,
            ulen: 4,
            ..g
        };
        assert!(encode_page(&mut page, 0, &[stored], 0).is_ok());
    }

    // ---- the trailer ----------------------------------------------------

    fn sample_trailer() -> Trailer {
        Trailer {
            n_pages: 3,
            n_entries: 5,
            nkeys: 200,
            res_off: 3 * ZPAGE as u64,
            res_len: 123,
            keys_len: 40,
            filter_off: 0,
            filter_blocks: 0,
            res_crc: 0xCAFE_F00D,
            filter: false,
            total_ulen: 9_999,
            seg_recs_len: 24_800,
            seg_crc_off: 30_000,
            seg_key_off: 192,
            seg_hash_cap: 512,
            seg_file_len: 40_960,
            seg_timestamp: 1_760_000_000,
            seg_generation: 1,
            seg_row_crc: 0x1357_9BDF,
            rec_version: 3,
            rec_magic: 0x5355_5044_4200_0007,
            seg_id: 17,
            seg_end_seq: 4_000,
            bloom_probes: 0,
            bloom_sig: 0,
            bloom_bits_per_key: 0,
        }
    }

    fn engine_filter(t: Trailer, blocks: u64) -> Trailer {
        Trailer {
            filter: true,
            filter_blocks: blocks,
            bloom_probes: super::super::BLOOM_PROBES,
            bloom_sig: super::super::BLOOM_SIG,
            bloom_bits_per_key: super::super::BLOOM_BITS_PER_KEY,
            ..t
        }
    }

    #[test]
    fn the_trailer_round_trips() {
        let plain = sample_trailer();
        let filtered = engine_filter(
            Trailer {
                keys_len: 40,
                filter_off: 48,
                res_len: 48 + 128,
                ..sample_trailer()
            },
            2,
        );
        for t in [plain, filtered] {
            let bytes = t.encode();
            assert_eq!(Trailer::decode(&bytes), Ok(t.clone()));
            assert_eq!(
                t.expected_file_len(),
                Some(t.res_off + t.res_len.next_multiple_of(4096) + 4096)
            );
            assert_eq!(t.expected_file_len().unwrap() % ZPAGE as u64, 0);
        }
        assert!(
            !sample_trailer().filter_matches_engine(),
            "no filter, no match"
        );
        assert!(engine_filter(sample_trailer(), 1).filter_matches_engine());
        let mut other = engine_filter(sample_trailer(), 1);
        other.bloom_sig += 1;
        assert!(!other.filter_matches_engine());
        other = engine_filter(sample_trailer(), 1);
        other.bloom_probes += 1;
        assert!(!other.filter_matches_engine());
        other = engine_filter(sample_trailer(), 1);
        other.bloom_bits_per_key += 1;
        assert!(!other.filter_matches_engine());
        // Fields that overflow the length are no length.
        let huge = Trailer {
            res_off: u64::MAX - 10,
            ..sample_trailer()
        };
        assert_eq!(huge.expected_file_len(), None);
    }

    /// A trailer's segment set from an account of it reads back as that
    /// account and leaves every other field as it was: the setter is the
    /// getter's inverse field by field, each field's value distinct from
    /// every other's so that two fields' places swapped read back wrong.
    #[test]
    fn a_trailers_segment_set_reads_back_as_set() {
        let s = SegIdent {
            seg_id: 1,
            seg_end_seq: 2,
            nkeys: 3,
            recs_len: 4,
            crc_off: 5,
            key_off: 6,
            hash_cap: 7,
            file_len: 8,
            timestamp: 9,
            generation: 10,
            row_crc: 11,
            rec_version: 12,
            rec_magic: 13,
        };
        let mut t = sample_trailer();
        t.set_seg(&s);
        assert_eq!(t.seg_ident(), s);
        let mut back = t.clone();
        back.set_seg(&sample_trailer().seg_ident());
        assert_eq!(back, sample_trailer(), "nothing but the segment moved");
    }

    #[test]
    fn bind32_hashes_the_identity_fields_in_order() {
        let mut by_hand = Vec::new();
        by_hand.extend_from_slice(&17u64.to_le_bytes());
        by_hand.extend_from_slice(&4_000u64.to_le_bytes());
        by_hand.extend_from_slice(&200u64.to_le_bytes());
        by_hand.extend_from_slice(&0x1357_9BDFu32.to_le_bytes());
        assert_eq!(by_hand.len(), 28);
        assert_eq!(bind32(17, 4_000, 200, 0x1357_9BDF), crc32c(&by_hand));
        assert_eq!(sample_trailer().bind32(), crc32c(&by_hand));
        // Each field moves it.
        let base = bind32(17, 4_000, 200, 0x1357_9BDF);
        assert_ne!(base, bind32(18, 4_000, 200, 0x1357_9BDF));
        assert_ne!(base, bind32(17, 4_001, 200, 0x1357_9BDF));
        assert_ne!(base, bind32(17, 4_000, 201, 0x1357_9BDF));
        assert_ne!(base, bind32(17, 4_000, 200, 0x1357_9BDE));
        // And the order is part of it: the same numbers in another slot.
        assert_ne!(bind32(1, 2, 3, 4), bind32(2, 1, 3, 4));
    }

    #[test]
    fn the_trailer_is_read_by_an_independent_decoder() {
        let t = engine_filter(
            Trailer {
                filter_off: 48,
                res_len: 176,
                ..sample_trailer()
            },
            2,
        );
        let b = t.encode();
        assert_eq!(&b[0..8], b"SUPDZPG2");
        assert_eq!(le32(&b, 8), 2, "version");
        assert_eq!(le32(&b, 12), 4096, "page size");
        for (at, want) in [
            (16, t.n_pages),
            (24, t.n_entries),
            (32, t.nkeys),
            (40, t.res_off),
            (48, t.res_len),
            (56, t.keys_len),
            (64, t.filter_off),
            (72, t.filter_blocks),
            (88, t.total_ulen),
            (96, t.seg_recs_len),
            (104, t.seg_crc_off),
            (112, t.seg_key_off),
            (120, t.seg_hash_cap),
            (128, t.seg_file_len),
            (136, t.seg_timestamp),
            (144, t.seg_generation),
            (160, t.rec_magic),
            (176, t.seg_id),
            (184, t.seg_end_seq),
            (200, t.bloom_sig),
        ] {
            assert_eq!(le64(&b, at), want, "the u64 at {at}");
        }
        for (at, want) in [
            (80, t.res_crc),
            (84, 1),
            (152, t.seg_row_crc),
            (156, t.rec_version),
            (168, 4072),
            (172, 256),
            (192, t.bind32()),
            (196, t.bloom_probes),
            (208, t.bloom_bits_per_key),
        ] {
            assert_eq!(le32(&b, at), want, "the u32 at {at}");
        }
        assert!(b[212..4092].iter().all(|&x| x == 0), "reserved zeros");
        assert_eq!(
            le32(&b, 4092),
            crc32c(&b[..4092]),
            "trailer_crc covers [0, 4092)"
        );
        // bind32 is the CRC of the 28 identity bytes, wherever they sit.
        let mut ident = Vec::new();
        ident.extend_from_slice(&b[176..184]);
        ident.extend_from_slice(&b[184..192]);
        ident.extend_from_slice(&b[32..40]);
        ident.extend_from_slice(&b[152..156]);
        assert_eq!(le32(&b, 192), crc32c(&ident));
    }

    #[test]
    fn a_flipped_byte_anywhere_in_a_trailer_is_refused() {
        let b = engine_filter(
            Trailer {
                filter_off: 48,
                res_len: 176,
                ..sample_trailer()
            },
            2,
        )
        .encode();
        for at in 0..ZTRAILER {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut bad = b;
                bad[at] ^= flip;
                let want = match at {
                    0..=7 => ZError::Magic,
                    8..=11 => ZError::Version,
                    12..=15 => ZError::PageSize,
                    _ => ZError::TrailerCrc,
                };
                assert_eq!(Trailer::decode(&bad), Err(want), "byte {at} ^ {flip:#x}");
            }
        }
        assert_eq!(Trailer::decode(&b[..ZTRAILER - 1]), Err(ZError::Length));
        assert_eq!(Trailer::decode(&[]), Err(ZError::Length));
        assert_eq!(Trailer::decode(&[0u8; ZTRAILER]), Err(ZError::Magic));
    }

    #[test]
    fn damage_the_trailers_crc_cannot_see_is_refused_by_the_checks_behind_it() {
        let good = engine_filter(
            Trailer {
                filter_off: 48,
                res_len: 176,
                ..sample_trailer()
            },
            2,
        );
        let base = good.encode();
        assert!(Trailer::decode(&base).is_ok());
        let bad = |f: &dyn Fn(&mut [u8; ZTRAILER])| {
            let mut b = base;
            f(&mut b);
            resign_trailer(&mut b);
            Trailer::decode(&b).err()
        };
        let inconsistent = |e: Option<ZError>| matches!(e, Some(ZError::Trailer(_)));
        // The constants of the layout.
        assert!(inconsistent(bad(&|b| b[168] ^= 1)), "page_cap");
        assert!(inconsistent(bad(&|b| b[172] ^= 1)), "max_count");
        // The flags: a bit this reader does not know.
        assert!(inconsistent(bad(&|b| b[84] |= 2)), "flags");
        // The zeros.
        for at in [212usize, 213, 2000, 4091] {
            assert!(inconsistent(bad(&|b| b[at] = 1)), "reserved byte {at}");
        }
        // bind32 against the fields it hashes.
        assert!(inconsistent(bad(&|b| b[192] ^= 1)), "bind32");
        assert!(inconsistent(bad(&|b| b[176] ^= 1)), "seg_id without bind32");
        assert!(
            inconsistent(bad(&|b| b[184] ^= 1)),
            "seg_end_seq without bind32"
        );
        assert!(inconsistent(bad(&|b| b[32] ^= 1)), "nkeys without bind32");
        assert!(
            inconsistent(bad(&|b| b[152] ^= 1)),
            "row_crc without bind32"
        );
        // The pages' length, and the nesting of pages in entries in keys.
        assert!(inconsistent(bad(&|b| b[40] ^= 1)), "res_off");
        assert!(
            inconsistent(bad(&|b| {
                put64(b, T_N_PAGES, 9);
                put64(b, T_RES_OFF, 9 * ZPAGE as u64);
            })),
            "more pages than entries"
        );
        assert!(
            inconsistent(bad(&|b| b[24] = 255)),
            "more entries than keys"
        );
        // The u32 offsets of the fence.
        assert!(
            inconsistent(bad(&|b| b[60] = 1)),
            "a key blob over four gibibytes"
        );
        // The filter flag and its blocks.
        assert!(inconsistent(bad(&|b| b[84] = 0)), "blocks without the flag");
        assert!(
            inconsistent(bad(&|b| b[72..80].fill(0))),
            "the flag without blocks"
        );
        assert!(
            inconsistent(bad(&|b| b[196] = 0)),
            "a filter with no probes"
        );
        assert!(inconsistent(bad(&|b| b[208] = 0)), "a filter with no bits");
        // A version or a page size the CRC covers is refused before it: a
        // version this reader does not know, and the one before, whose
        // files it rebuilds.
        let mut b = base;
        b[8] = 3;
        assert_eq!(Trailer::decode(&b), Err(ZError::Version));
        let mut b = base;
        b[8] = 1;
        assert_eq!(Trailer::decode(&b), Err(ZError::Version));
        let mut b = base;
        b[12..16].copy_from_slice(&16_384u32.to_le_bytes());
        assert_eq!(Trailer::decode(&b), Err(ZError::PageSize));
    }

    // ---- the resident part ----------------------------------------------

    /// A fence of `e` entries with ascending keys, every third holding no
    /// page, over a partition with room for a record or more past the last.
    fn sample_fence(rng: &mut Rng, e: usize) -> (ZFence, u64, u64) {
        let mut keys = std::collections::BTreeSet::new();
        while keys.len() < e {
            let l = rng.below(30);
            keys.insert(rng.bytes(l));
        }
        let mut fence = ZFence::new();
        let (mut rank, mut page) = (0u32, 0u32);
        for (i, k) in keys.iter().enumerate() {
            let p = if i % 3 == 2 {
                ZNOPAGE
            } else {
                page += 1;
                page - 1
            };
            fence.push(rank, p, k).unwrap();
            rank += 1 + rng.below(60) as u32;
        }
        (fence, u64::from(rank), u64::from(page))
    }

    fn trailer_for(fence: &ZFence, filter: &[u8], nkeys: u64, n_pages: u64) -> (Trailer, Vec<u8>) {
        let (res, f) = fence.encode_resident(filter).unwrap();
        let t = Trailer {
            n_pages,
            n_entries: fence.len() as u64,
            nkeys,
            res_off: n_pages * ZPAGE as u64,
            res_len: f.res_len,
            keys_len: f.keys_len,
            filter_off: f.filter_off,
            filter_blocks: f.filter_blocks,
            res_crc: f.res_crc,
            ..sample_trailer()
        };
        let t = if filter.is_empty() {
            t
        } else {
            engine_filter(t, f.filter_blocks)
        };
        (t, res)
    }

    #[test]
    fn the_entry_a_rank_falls_in_is_the_greatest_at_or_below_it() {
        let mut f = ZFence::new();
        assert_eq!(f.entry_at_rank(0), None, "an empty fence");
        for (rank, key) in [(0u32, &b"a"[..]), (10, b"k"), (20, b"u")] {
            f.push(rank, rank / 10, key).unwrap();
        }
        for (rank, want) in [(0u32, 0usize), (9, 0), (10, 1), (19, 1), (20, 2), (1000, 2)] {
            assert_eq!(f.entry_at_rank(rank), Some(want), "rank {rank}");
        }
    }

    #[test]
    fn the_resident_part_round_trips_with_and_without_a_filter() {
        let mut rng = Rng::new(5);
        for (e, blocks) in [
            (1usize, 0usize),
            (1, 1),
            (2, 0),
            (7, 3),
            (100, 0),
            (100, 17),
            (1000, 5),
        ] {
            let (fence, nkeys, n_pages) = sample_fence(&mut rng, e);
            let filter = rng.bytes(blocks * 64);
            let (t, res) = trailer_for(&fence, &filter, nkeys, n_pages);
            assert!(res.len().is_multiple_of(ZPAGE) && res.len() >= t.res_len as usize);
            assert_eq!(res.len() as u64, t.res_len.next_multiple_of(ZPAGE as u64));
            let (back, f) = ZFence::decode_resident(&res, &t).expect("a good region decodes");
            assert_eq!(back, fence, "{e} entries, {blocks} blocks");
            assert_eq!(f, &filter[..]);
            // The trailer that describes it is itself a good trailer.
            assert_eq!(Trailer::decode(&t.encode()), Ok(t.clone()));
            // The region without its padding decodes alike.
            let (again, _) = ZFence::decode_resident(&res[..t.res_len as usize], &t).unwrap();
            assert_eq!(again, fence);
            // Entries as a page's first decode wants them.
            for i in 0..fence.len() {
                let en = fence.entry(i, nkeys as u32).unwrap();
                assert_eq!(
                    (en.index, en.first_rank, en.page),
                    (i, fence.first_rank(i).unwrap(), fence.page(i).unwrap())
                );
                assert_eq!(en.key, fence.key(i).unwrap());
                let next = fence.first_rank(i + 1).unwrap_or(nkeys as u32);
                assert_eq!(en.count, next - en.first_rank);
            }
            assert!(fence.entry(fence.len(), nkeys as u32).is_none());
        }
        // No entries, no keys: the empty partition.
        let empty = ZFence::new();
        let (t, res) = trailer_for(&empty, &[], 0, 0);
        assert_eq!(ZFence::decode_resident(&res, &t).unwrap().0, empty);
        assert!(empty.is_empty() && empty.keys_len() == 0);
        // A filter that is not whole blocks is no filter.
        assert!(matches!(
            empty.encode_resident(&[0u8; 63]),
            Err(ZError::Resident(_))
        ));
    }

    #[test]
    fn the_resident_part_is_read_by_an_independent_decoder() {
        let mut rng = Rng::new(6);
        let (fence, nkeys, n_pages) = sample_fence(&mut rng, 40);
        let filter = rng.bytes(3 * 64);
        let (t, res) = trailer_for(&fence, &filter, nkeys, n_pages);
        let e = fence.len();
        let r = |at: usize| le32(&res, at);
        // B1, B2, B3 back to back, then the blob, then zeros to a multiple
        // of eight, then the filter.
        for i in 0..e {
            assert_eq!(r(4 * i), fence.first_rank(i).unwrap(), "first_rank[{i}]");
            assert_eq!(r(4 * e + 4 * i), fence.page(i).unwrap(), "page_no[{i}]");
        }
        assert_eq!(r(8 * e), 0, "key_off[0]");
        let blob = 12 * e + 4;
        assert_eq!(
            r(8 * e + 4 * e) as u64,
            t.keys_len,
            "key_off[E] is the blob's length"
        );
        for i in 0..e {
            let (a, b) = (r(8 * e + 4 * i) as usize, r(8 * e + 4 * (i + 1)) as usize);
            assert_eq!(&res[blob + a..blob + b], fence.key(i).unwrap(), "key {i}");
        }
        let end = blob + t.keys_len as usize;
        let at = end.next_multiple_of(8);
        assert!(res[end..at].iter().all(|&b| b == 0));
        assert_eq!(t.filter_off as usize, at);
        assert_eq!(&res[at..at + filter.len()], &filter[..]);
        assert_eq!(t.res_len as usize, at + filter.len());
        assert_eq!(t.res_crc, crc32c(&res[..t.res_len as usize]));
        assert!(res[t.res_len as usize..].iter().all(|&b| b == 0));
    }

    #[test]
    fn a_flipped_byte_anywhere_in_the_resident_part_is_refused() {
        let mut rng = Rng::new(8);
        for blocks in [0usize, 2] {
            let (fence, nkeys, n_pages) = sample_fence(&mut rng, 25);
            let (t, res) = trailer_for(&fence, &rng.bytes(blocks * 64), nkeys, n_pages);
            for at in 0..t.res_len as usize {
                for flip in [0x01u8, 0xFF] {
                    let mut bad = res.clone();
                    bad[at] ^= flip;
                    assert_eq!(
                        ZFence::decode_resident(&bad, &t).err(),
                        Some(ZError::ResidentCrc),
                        "byte {at} ^ {flip:#x}, {blocks} blocks"
                    );
                }
            }
            // A region shorter than the trailer says is no region.
            assert_eq!(
                ZFence::decode_resident(&res[..t.res_len as usize - 1], &t).err(),
                Some(ZError::Length)
            );
        }
    }

    /// A region edited, and the trailer that describes the edited bytes.
    fn resident_edited(
        fence: &ZFence,
        filter: &[u8],
        nkeys: u64,
        n_pages: u64,
        edit: &dyn Fn(&mut Vec<u8>, &mut Trailer),
    ) -> Result<(), ZError> {
        let (mut t, mut res) = trailer_for(fence, filter, nkeys, n_pages);
        edit(&mut res, &mut t);
        t.res_crc = crc32c(&res[..t.res_len as usize]);
        ZFence::decode_resident(&res, &t).map(|_| ())
    }

    #[test]
    fn a_resident_part_that_does_not_parse_is_refused_though_its_crc_passes() {
        let mut rng = Rng::new(9);
        let (fence, nkeys, n_pages) = sample_fence(&mut rng, 12);
        let filter = rng.bytes(2 * 64);
        let e = fence.len();
        let resident = |e: Result<(), ZError>| matches!(e, Err(ZError::Resident(_)));
        let run = |edit: &dyn Fn(&mut Vec<u8>, &mut Trailer)| {
            resident_edited(&fence, &filter, nkeys, n_pages, edit)
        };
        assert_eq!(run(&|_, _| {}), Ok(()));
        // key_off: not from zero, not to the blob's length, not ascending.
        assert!(resident(run(&|r, _| r[8 * e] = 1)), "key_off[0]");
        assert!(resident(run(&|r, _| r[8 * e + 4 * e] ^= 1)), "key_off[E]");
        assert!(
            resident(run(&|r, _| {
                // key_off[1] past everything after it.
                r[8 * e + 4..8 * e + 8].copy_from_slice(&u32::MAX.to_le_bytes());
            })),
            "key_off not ascending"
        );
        // The filter: not where the entries end, not the length the region has.
        assert!(resident(run(&|_, t| t.filter_off += 8)), "filter_off");
        assert!(resident(run(&|_, t| t.filter_off -= 8)), "filter_off");
        assert!(resident(run(&|_, t| t.filter_blocks += 1)), "filter_blocks");
        assert!(resident(run(&|_, t| t.res_len -= 8)), "res_len");
        assert!(resident(run(&|_, t| t.keys_len += 1)), "keys_len");
        // No filter, but the trailer says there is a tail.
        let none = |edit: &dyn Fn(&mut Vec<u8>, &mut Trailer)| {
            resident_edited(&fence, &[], nkeys, n_pages, edit)
        };
        assert_eq!(none(&|_, _| {}), Ok(()));
        assert!(resident(none(&|_, t| t.filter_off = 8)));
        assert!(resident(none(&|_, t| t.filter_blocks = 1)));
        assert!(resident(none(&|_, t| t.res_len -= 1)));
    }

    #[test]
    fn the_padding_before_the_filter_is_zeros() {
        // A fence whose key blob does not end on a multiple of eight, so that
        // there is padding to be wrong: built to, not drawn, since a test that
        // asserts only where the draw left padding passes by skipping itself.
        let mut fence = ZFence::new();
        for (i, key) in [&b"a"[..], b"bc", b"def"].into_iter().enumerate() {
            fence.push(10 * i as u32, i as u32, key).unwrap();
        }
        let (nkeys, n_pages) = (30, 3);
        let filter = Rng::new(10).bytes(2 * 64);
        let blob_end = 12 * fence.len() + 4 + fence.keys_len();
        let pad = blob_end.next_multiple_of(8) - blob_end;
        assert!(pad > 1, "{pad} bytes of padding to put a one in");
        let run = |edit: &dyn Fn(&mut Vec<u8>, &mut Trailer)| {
            resident_edited(&fence, &filter, nkeys, n_pages, edit)
        };
        assert_eq!(run(&|_, _| {}), Ok(()), "the padding as written");
        // The padding is read: a one in the first byte of it, in the last,
        // and in all of it. The resident part's CRC is made good again, so
        // the CRC cannot be what refuses them.
        for at in [blob_end, blob_end + pad - 1] {
            let got = run(&|r, _| r[at] = 1);
            assert!(
                matches!(got, Err(ZError::Resident(_))),
                "byte {at}: {got:?}"
            );
        }
        let got = run(&|r, _| r[blob_end..blob_end + pad].fill(0xFF));
        assert!(matches!(got, Err(ZError::Resident(_))), "{got:?}");
        // The check is on the padding and not on the region: the byte before
        // it is a key's, and a key that changed is a different key, not a
        // refusal.
        assert_eq!(run(&|r, _| r[blob_end - 1] ^= 1), Ok(()));
    }

    #[test]
    fn a_fence_that_does_not_tile_is_refused() {
        let mk = |entries: &[(u32, u32, &[u8])]| {
            let mut f = ZFence::new();
            for &(r, p, k) in entries {
                f.push(r, p, k).unwrap();
            }
            f
        };
        let good = [(0u32, 0u32, &b"a"[..]), (10, 1, b"c"), (20, ZNOPAGE, b"e")];
        let fence = mk(&good);
        assert_eq!(fence.check_tiling(30, 2), Ok(()));
        let tiling = |e: Result<(), ZError>| matches!(e, Err(ZError::Tiling(_)));
        // Where the first entry begins.
        assert!(tiling(
            mk(&[(1, 0, b"a"), (10, 1, b"c")]).check_tiling(30, 2)
        ));
        // First ranks: equal, decreasing, and past the last key.
        assert!(tiling(
            mk(&[(0, 0, b"a"), (10, 1, b"c"), (10, 2, b"e")]).check_tiling(30, 3)
        ));
        assert!(tiling(
            mk(&[(0, 0, b"a"), (10, 1, b"c"), (9, 2, b"e")]).check_tiling(30, 3)
        ));
        assert!(
            tiling(fence.check_tiling(20, 2)),
            "the last entry begins at nkeys"
        );
        assert!(tiling(fence.check_tiling(5, 2)));
        // Keys: equal and descending.
        assert!(tiling(
            mk(&[(0, 0, b"a"), (10, 1, b"a")]).check_tiling(30, 2)
        ));
        assert!(tiling(
            mk(&[(0, 0, b"b"), (10, 1, b"a")]).check_tiling(30, 2)
        ));
        // Pages: past the pages, descending, repeated, and too few or too many.
        assert!(tiling(fence.check_tiling(30, 1)), "page 1 with one page");
        assert!(tiling(
            mk(&[(0, 1, b"a"), (10, 0, b"c")]).check_tiling(30, 2)
        ));
        assert!(tiling(
            mk(&[(0, 0, b"a"), (10, 0, b"c")]).check_tiling(30, 2)
        ));
        assert!(tiling(fence.check_tiling(30, 3)), "a page no entry names");
        assert!(tiling(mk(&[(0, ZNOPAGE, b"a")]).check_tiling(30, 1)));
        // Entries over no keys, and none over some.
        assert!(tiling(fence.check_tiling(0, 0)));
        assert!(tiling(ZFence::new().check_tiling(5, 0)));
        assert_eq!(ZFence::new().check_tiling(0, 0), Ok(()));
        assert!(tiling(ZFence::new().check_tiling(0, 1)));
        // A region whose entries do not tile is refused on its way in, with
        // a CRC that passes: the CRC cannot see a builder that wrote them
        // wrong.
        let bad = mk(&[(0, 0, b"b"), (10, 1, b"a")]);
        let (t, res) = trailer_for(&bad, &[], 30, 2);
        assert!(tiling(ZFence::decode_resident(&res, &t).map(|_| ())));
    }

    // ---- records and groups ---------------------------------------------

    #[test]
    fn record_key_next_is_parse_records_length_for_every_shape() {
        let mut rng = Rng::new(7);
        let mut shapes = [0usize; 3];
        for i in 0..100_000 {
            let kl = if i % 50 == 0 {
                rng.below(400)
            } else {
                rng.below(40)
            };
            let key = rng.bytes(kl);
            let img = random_image(&mut rng, &key);
            let al = Al::new(&img);
            let buf = al.bytes();
            let (k, exts, _tail, len) = parse_record(buf, 0).expect("a fixture record parses");
            assert_eq!(len, img.len(), "an image is the bytes its record spans");
            assert_eq!(k, &key[..]);
            assert_eq!(record_key_next(buf, 0), Some((k, len)), "record {i}");
            let shape = if img[3] & 0x80 != 0 {
                0
            } else if exts.len() == 1 {
                1
            } else {
                2
            };
            shapes[shape] += 1;
        }
        // Compact records, full records of one extent, and records of any
        // other count were all walked.
        assert!(shapes.iter().all(|&n| n > 1000), "{shapes:?}");
    }

    #[test]
    fn record_key_next_walks_records_laid_end_to_end() {
        let mut rng = Rng::new(17);
        for round in 0..2000 {
            let n = 1 + rng.below(12);
            let recs = sorted_records(&mut rng, n);
            // Records behind a prefix of padding words, as a group is not but
            // a region may be.
            let lead = 4 * rng.below(5);
            let mut bytes = vec![0u8; lead];
            for (_, im) in &recs {
                bytes.extend_from_slice(im);
            }
            let al = Al::new(&bytes);
            let buf = al.bytes();
            let (mut a, mut b) = (lead, lead);
            for (i, (key, im)) in recs.iter().enumerate() {
                let (k, next) = record_key_next(buf, a).expect("a record in a buffer of records");
                let (pk, _, _, plen) = parse_record(buf, b).expect("parse_record agrees");
                assert_eq!((k, next - a), (pk, plen), "round {round} record {i}");
                assert_eq!(k, &key[..]);
                assert_eq!(next - a, im.len());
                a = next;
                b += plen;
            }
            assert_eq!(a, buf.len());
            // The end of the buffer holds no record.
            assert_eq!(record_key_next(buf, buf.len()), None);
        }
    }

    #[test]
    fn record_key_next_never_leaves_its_buffer() {
        let mut rng = Rng::new(19);
        for _ in 0..300 {
            let kl = rng.below(30);
            let key = rng.bytes(kl);
            let img = random_image(&mut rng, &key);
            // Cut anywhere short of the whole record, padding included: no
            // answer, rather than an offset past the buffer.
            for cut in 0..img.len() {
                let al = Al::new(&img[..cut]);
                assert_eq!(
                    record_key_next(al.bytes(), 0),
                    None,
                    "cut at {cut} of {}",
                    img.len()
                );
            }
        }
        // Garbage: no panic, whatever the offset, and an answer stays inside.
        for round in 0..3000 {
            let l = rng.below(80);
            let junk = rng.bytes(l);
            let al = Al::new(&junk);
            let buf = al.bytes();
            let offs = [
                0,
                1,
                2,
                3,
                4,
                8,
                buf.len(),
                buf.len() + 1,
                usize::MAX - 1,
                usize::MAX,
            ];
            for off in offs {
                if let Some((k, next)) = record_key_next(buf, off) {
                    assert!(next > off && next <= buf.len(), "round {round} off {off}");
                    assert_eq!(Some(k), crate::flatindex::record_key(buf, off));
                }
            }
        }
        // A compact record whose header its run contradicts has a key and an
        // end here, and no values in `parse_record`: the reader that wants
        // the values is the one that finds it.
        let mut odd = Vec::new();
        odd.extend_from_slice(&1u16.to_le_bytes());
        odd.extend_from_slice(&crate::flatindex::COMPACT.to_le_bytes());
        odd.extend_from_slice(&[b'k', 0, 0, 0]);
        odd.extend_from_slice(&8u16.to_le_bytes());
        odd.extend_from_slice(&(crate::flatindex::C_FIXED | 3).to_le_bytes());
        odd.extend_from_slice(&[7u8; 8]);
        assert_eq!(odd.len(), crate::flatindex::record_len_compact(1, 8));
        let al = Al::new(&odd);
        assert_eq!(record_key_next(al.bytes(), 0), Some((&b"k"[..], odd.len())));
        assert!(
            parse_record(al.bytes(), 0).is_none(),
            "eight bytes are not three values of one width"
        );
        // A compact bit beside a count is damage, here as in `record_key`.
        let mut both = odd.clone();
        both[2..4].copy_from_slice(&(crate::flatindex::COMPACT | 5).to_le_bytes());
        let al = Al::new(&both);
        assert_eq!(record_key_next(al.bytes(), 0), None);
    }

    #[test]
    fn a_group_walks_and_seeks_like_its_records() {
        let mut rng = Rng::new(21);
        for round in 0..300 {
            let n = 1 + rng.below(40);
            let recs = sorted_records(&mut rng, n);
            let group: Vec<u8> = recs.iter().flat_map(|(_, i)| i.iter().copied()).collect();
            let al = Al::new(&group);
            let g = al.bytes();
            walk_group(g, n, Some(&recs[0].0), None).expect("a whole group walks");
            walk_group(g, n, None, None).expect("and without a fence key");
            // Every key below a bound above the last, and not below the
            // last itself or anything under it.
            let mut above = recs[n - 1].0.clone();
            above.push(0);
            walk_group(g, n, None, Some(&above)).expect("every key below the bound");
            assert!(matches!(
                walk_group(g, n, None, Some(&recs[n - 1].0)),
                Err(ZError::Walk(_))
            ));
            assert!(matches!(
                walk_group(g, n, None, Some(b"")),
                Err(ZError::Walk(_))
            ));
            let lens: Vec<usize> = recs.iter().map(|(_, im)| im.len()).collect();
            let model = |key: &[u8]| {
                let idx = recs.partition_point(|(k, _)| k.as_slice() < key);
                let off: usize = lens[..idx].iter().sum();
                (idx, off, idx < n && recs[idx].0 == key)
            };
            let mut probes: Vec<Vec<u8>> = vec![
                Vec::new(),
                vec![0xFF; 8],
                vec![0],
                rng.bytes(5),
                rng.bytes(25),
            ];
            for (k, _) in &recs {
                probes.push(k.clone());
                let mut above = k.clone();
                above.push(0);
                probes.push(above);
                if let Some((last, rest)) = k.split_last() {
                    let mut below = rest.to_vec();
                    below.push(last.wrapping_sub(1));
                    below.push(0xFF);
                    probes.push(below);
                    probes.push(rest.to_vec());
                }
            }
            for key in &probes {
                let (idx, off, found) = model(key);
                let p = group_seek(g, n, key).expect("a whole group seeks");
                assert_eq!(
                    (p.index, p.off, p.found),
                    (idx, off, found),
                    "round {round} key {key:?}"
                );
            }
        }
    }

    #[test]
    fn the_empty_key_is_sought_at_the_front_of_every_group() {
        // Nothing sorts below it: it is found when the first record has it,
        // and is the position before the first record when not.
        let with = [one_value(b"", b"v"), one_value(b"a", b"w")].concat();
        let without = [one_value(b"a", b"v"), one_value(b"b", b"w")].concat();
        for (group, found) in [(&with, true), (&without, false)] {
            let al = Al::new(group);
            let p = group_seek(al.bytes(), 2, b"").expect("a whole group seeks");
            assert_eq!((p.index, p.off, p.found), (0, 0, found));
        }
        let al = Al::new(&[]);
        let p = group_seek(al.bytes(), 0, b"").expect("no records is a group too");
        assert_eq!((p.index, p.off, p.found), (0, 0, false));
        // A group cut short is still refused for the empty key: an error,
        // and not the absence of the key.
        let al = Al::new(&with[..6]);
        let got = group_seek(al.bytes(), 2, b"");
        assert!(matches!(got, Err(ZError::Walk(_))), "{got:?}");
    }

    #[test]
    fn a_group_that_is_not_the_records_it_says_is_refused() {
        let mut rng = Rng::new(23);
        for _ in 0..200 {
            let n = 3 + rng.below(20);
            let recs = sorted_records(&mut rng, n);
            let imgs: Vec<&Vec<u8>> = recs.iter().map(|(_, i)| i).collect();
            let join = |order: &[usize]| -> Vec<u8> {
                order
                    .iter()
                    .flat_map(|&i| imgs[i].iter().copied())
                    .collect()
            };
            let natural: Vec<usize> = (0..n).collect();
            let group = join(&natural);
            let walks = |g: &[u8], count: usize, first: Option<&[u8]>| {
                let al = Al::new(g);
                walk_group(al.bytes(), count, first, None)
            };
            assert_eq!(walks(&group, n, Some(&recs[0].0)), Ok(()));
            // One more record than there are, and one fewer.
            assert!(matches!(walks(&group, n + 1, None), Err(ZError::Walk(_))));
            assert!(matches!(walks(&group, n - 1, None), Err(ZError::Walk(_))));
            // Cut short, and padded past its end.
            for cut in 1..=4 {
                assert!(
                    matches!(
                        walks(&group[..group.len() - cut], n, None),
                        Err(ZError::Walk(_))
                    ),
                    "cut {cut}"
                );
            }
            let mut longer = group.clone();
            longer.extend_from_slice(&[0, 0, 0, 0]);
            assert!(matches!(walks(&longer, n, None), Err(ZError::Walk(_))));
            // Two records swapped, and one twice.
            let mut swapped = natural.clone();
            swapped.swap(1, 2);
            assert!(matches!(
                walks(&join(&swapped), n, None),
                Err(ZError::Walk(_))
            ));
            let mut twice = natural.clone();
            twice[2] = 1;
            assert!(matches!(
                walks(&join(&twice), n, None),
                Err(ZError::Walk(_))
            ));
            // The fence's key is not the first.
            assert!(matches!(
                walks(&group, n, Some(&recs[1].0)),
                Err(ZError::Walk(_))
            ));
            // A search over a group cut short says so rather than guessing: an
            // error, which a reader must count and not read as "absent", and
            // which `found: false` is not.
            let cut = &group[..group.len() - 1];
            let alc = Al::new(cut);
            let past = vec![0xFFu8; 64];
            let got = group_seek(alc.bytes(), n, &past);
            assert!(matches!(got, Err(ZError::Walk(_))), "{got:?}");
        }
    }

    // ---- fill_page ------------------------------------------------------

    struct Built {
        /// (first rank, the page's summary, the page)
        pages: Vec<(usize, Filled, [u8; ZPAGE])>,
        declined: Vec<(usize, NoPage)>,
    }

    /// `fill_page` over every record in rank order, as a builder drives it,
    /// the previous page's count the one-group shape's hint; or one hint
    /// for every call.
    fn build_all<I: Images + ?Sized>(
        src: &I,
        s: &mut FillScratch,
        bind: u32,
        fixed_hint: Option<usize>,
    ) -> Built {
        build_from(src, s, bind, fixed_hint, 0)
    }

    /// `build_all` with the first call's hint given: a builder that never
    /// grows a group has to start from a count that is close.
    fn build_from<I: Images + ?Sized>(
        src: &I,
        s: &mut FillScratch,
        bind: u32,
        fixed_hint: Option<usize>,
        start: usize,
    ) -> Built {
        let n = src.records();
        let mut rank = 0usize;
        s.hint = start;
        let mut out = Built {
            pages: Vec::new(),
            declined: Vec::new(),
        };
        while rank < n {
            let mut page = [0u8; ZPAGE];
            let turns = s.stats.turns;
            if let Some(h) = fixed_hint {
                s.hint = h;
            }
            match fill_page(src, rank, bind, s, &mut page) {
                Fill::Page(g) => {
                    assert!(g.count >= 1 && rank + g.count <= n, "rank {rank}: {g:?}");
                    assert!(
                        g.count <= ZMAX_COUNT && g.used <= ZCAP && g.clen <= g.used,
                        "{g:?}"
                    );
                    assert!(g.groups >= 1 && g.groups <= ZMAX_GROUPS && g.stored <= g.groups);
                    out.pages.push((rank, g, page));
                    rank += g.count;
                }
                Fill::NoPage(why) => {
                    out.declined.push((rank, why));
                    rank += 1;
                }
                other => panic!("rank {rank} of {n}: {other:?}"),
            }
            assert!(
                s.stats.turns - turns <= u64::from(ZFILL_TURNS) * ZMAX_GROUPS as u64,
                "rank {rank}: {} turns",
                s.stats.turns - turns
            );
        }
        out
    }

    /// What a first decode must refuse of a page that is not its entry's:
    /// the page against its own entry with one quantity off, and its
    /// neighbours against it. Every page of a form file shares `bind32` and
    /// passes its own CRC, so the comparison of the page's `first_rank` and
    /// `count` with the entry's, and of its first key with the entry's, is
    /// all that stands between a page read at the wrong place -- a page
    /// number off by one, a fence entry from before a rebuild -- and the
    /// records it holds.
    fn check_fence_binds(
        page: &[u8; ZPAGE],
        neighbours: &[&[u8; ZPAGE]],
        entry: &FenceEntry<'_>,
        bind: u32,
        buf: &mut GroupBuf,
    ) {
        let mut refused =
            |page: &[u8; ZPAGE], e: &FenceEntry<'_>| verify_page(page, bind, e, buf).err();
        // The entry's rank and its count, each off by one either way: the
        // comparison that names the disagreement, not the walk behind it.
        for (first_rank, count) in [
            (entry.first_rank.wrapping_add(1), entry.count),
            (entry.first_rank.wrapping_sub(1), entry.count),
            (entry.first_rank, entry.count + 1),
            (entry.first_rank, entry.count - 1),
        ] {
            let e = FenceEntry {
                first_rank,
                count,
                ..*entry
            };
            assert_eq!(refused(page, &e), Some(ZError::Fence), "{e:?}");
        }
        // The entry's key: longer, shorter, and one byte off. The page's
        // rank and count agree with the entry here, so only the first key's
        // comparison in the walk can see it -- or, on a page of several
        // groups, the comparison of the entry's key with the second group's,
        // where the longer key sorts at or past it.
        let mut keys = vec![[entry.key, &[0u8][..]].concat()];
        if let Some((last, rest)) = entry.key.split_last() {
            keys.push(rest.to_vec());
            keys.push([rest, &[last ^ 1][..]].concat());
        }
        for k in &keys {
            let e = FenceEntry { key: k, ..*entry };
            let got = refused(page, &e);
            assert!(
                matches!(got, Some(ZError::Walk(_) | ZError::Fence)),
                "{e:?}: {got:?}"
            );
        }
        // A page of this form file at another entry's place: it passes its own
        // CRC under this `bind32`, and only the comparison refuses it.
        for other in neighbours {
            assert!(parse_page(*other, bind).is_ok(), "a good page of the build");
            assert_eq!(refused(other, entry), Some(ZError::Fence), "{entry:?}");
        }
    }

    /// What must hold of a build whatever the records: they are covered
    /// once each by a page or a decline; every page decodes, group after
    /// group, to its records' images; every group is stored exactly where
    /// LZ4 did not help, and carries the LZ4 block of its records
    /// otherwise; every group but the last of a page holds what the target
    /// allows and no more; the directory names each group after the first
    /// by its first record's key; and a record is declined only for a
    /// reason that holds of it alone. With `records`, the images are
    /// records, every page passes the check a reader's first decode makes
    /// at its own fence entry, and is refused at an entry that is not its
    /// own.
    fn check_build<I: Images + ?Sized>(src: &I, b: &Built, bind: u32, records: bool) {
        check_build_at(src, b, bind, records, ZGROUP_BYTES)
    }

    fn check_build_at<I: Images + ?Sized>(
        src: &I,
        b: &Built,
        bind: u32,
        records: bool,
        target: usize,
    ) {
        let n = src.records();
        let mut covered = vec![0u32; n];
        let mut buf = GroupBuf::new();
        let mut own_buf = GroupBuf::new();
        for (i, (rank, g, page)) in b.pages.iter().enumerate() {
            for c in &mut covered[*rank..*rank + g.count] {
                *c += 1;
            }
            let v = parse_page(page, bind).expect("a page fill_page built parses");
            let h = v.header;
            assert_eq!(
                (
                    h.first_rank as usize,
                    h.count as usize,
                    h.ulen as usize,
                    usize::from(h.used),
                    usize::from(h.groups),
                    h.stored.count_ones() as usize
                ),
                (*rank, g.count, g.ulen, g.used, g.groups, g.stored),
                "rank {rank}"
            );
            let (mut at, mut clen) = (*rank, 0usize);
            for j in 0..v.groups() {
                let ge = v.group(j).unwrap();
                let count = usize::from(ge.count);
                let group: Vec<u8> = (at..at + count)
                    .flat_map(|r| src.image(r).iter().copied())
                    .collect();
                assert_eq!(group.len(), ge.ulen as usize, "rank {rank} group {j}");
                assert_eq!(
                    decode_group(&v, j, &mut buf).unwrap(),
                    &group[..],
                    "rank {rank} group {j}"
                );
                // And through the engine's own decoder, the page's last group
                // with no more of its page after it than the fill left.
                assert_eq!(
                    decode_group_with(&v, j, &mut own_buf, true).unwrap(),
                    &group[..],
                    "rank {rank} group {j}, the own decoder"
                );
                let c = lz4_flex::block::compress(&group);
                let payload = v.payload(j).unwrap();
                if ge.stored {
                    assert!(
                        c.len() >= group.len(),
                        "rank {rank} group {j}: stored though LZ4 helped"
                    );
                    assert_eq!(payload, &group[..]);
                } else {
                    assert_eq!(
                        payload,
                        &c[..],
                        "rank {rank} group {j}: the block is the compressor's output"
                    );
                }
                // Under a target every group but the page's last holds the
                // records the target allows and no more: the next record
                // would have passed it, or the count or the page's groups
                // bound it.
                if target > 0 && j + 1 < v.groups() {
                    let next = src.image(at + count).len();
                    assert!(
                        group.len() <= target.min(ZMAX_ULEN) || count == 1,
                        "rank {rank} group {j}: {} bytes over the target",
                        group.len()
                    );
                    assert!(
                        group.len() + next > target || count == ZMAX_COUNT,
                        "rank {rank} group {j}: the next record would have fit"
                    );
                }
                if records && j > 0 {
                    let key = crate::flatindex::record_key(src.image(at), 0).unwrap();
                    assert_eq!(v.key(j), Some(key), "rank {rank} group {j}'s key");
                }
                at += count;
                clen += usize::from(ge.clen);
            }
            assert_eq!((at, clen), (*rank + g.count, g.clen));
            if records {
                let key = crate::flatindex::record_key(src.image(*rank), 0).unwrap();
                let entry = FenceEntry {
                    index: 0,
                    first_rank: *rank as u32,
                    count: g.count as u32,
                    page: 0,
                    key,
                };
                assert_eq!(
                    verify_page(page, bind, &entry, &mut buf).map(|h| h.count as usize),
                    Ok(g.count)
                );
                // And refuses what is not this page's entry, with the pages on
                // either side of it for the entries a page is read at by mistake.
                let near: Vec<&[u8; ZPAGE]> = [i.checked_sub(1), Some(i + 1)]
                    .into_iter()
                    .flatten()
                    .filter_map(|j| b.pages.get(j).map(|p| &p.2))
                    .collect();
                check_fence_binds(page, &near, &entry, bind, &mut buf);
            }
        }
        for &(r, why) in &b.declined {
            covered[r] += 1;
            let im = src.image(r);
            match why {
                NoPage::Oversize => assert!(im.len() > ZMAX_ULEN, "rank {r}"),
                NoPage::Incompressible => assert!(
                    im.len() > ZCAP - ZDIR_ENTRY
                        && lz4_flex::block::compress(im).len() > ZCAP - ZDIR_ENTRY,
                    "rank {r}: {} bytes, lz4 {}",
                    im.len(),
                    lz4_flex::block::compress(im).len()
                ),
                NoPage::Unplaceable => {
                    assert!(im.is_empty() || !im.len().is_multiple_of(4), "rank {r}")
                }
            }
        }
        assert!(
            covered.iter().all(|&c| c == 1),
            "every record once: {covered:?}"
        );
    }

    /// The summary a build's scratch gives: records a page, the page's
    /// used bytes over the cap, compress calls a page, groups a page.
    fn per_page(s: &FillScratch) -> (f64, f64, f64, f64) {
        let p = s.stats.pages as f64;
        (
            s.stats.records as f64 / p,
            s.stats.used_bytes as f64 / (p * ZCAP as f64),
            s.stats.calls as f64 / p,
            s.stats.groups as f64 / p,
        )
    }

    /// A scratch of the one-group shape: the page as one group, grown to
    /// fill it, which the tests of the hint, the growth and the turns are
    /// about.
    fn one_group() -> FillScratch {
        let mut s = FillScratch::new();
        s.group_bytes = 0;
        s
    }

    #[test]
    fn a_page_is_refused_at_an_entry_that_is_not_its_own() {
        let images = suite_images(2500, 14);
        let bind = bind32(1, 2, images.len() as u64, 3);
        let mut s = FillScratch::new();
        let b = build_all(&images, &mut s, bind, None);
        assert!(b.pages.len() >= 20, "{} pages", b.pages.len());
        let mut buf = GroupBuf::new();
        for (i, (rank, g, page)) in b.pages.iter().enumerate() {
            let key = crate::flatindex::record_key(&images[*rank], 0).unwrap();
            let entry = FenceEntry {
                index: i,
                first_rank: *rank as u32,
                count: g.count as u32,
                page: i as u32,
                key,
            };
            let ok = verify_page(page, bind, &entry, &mut buf).map(|h| usize::from(h.count));
            assert_eq!(ok, Ok(g.count), "page {i} at its own entry");
            let near: Vec<&[u8; ZPAGE]> = [i.checked_sub(1), Some(i + 1)]
                .into_iter()
                .flatten()
                .filter_map(|j| b.pages.get(j).map(|p| &p.2))
                .collect();
            check_fence_binds(page, &near, &entry, bind, &mut buf);
            // A page of another form file fails its CRC before the entry is
            // looked at, whatever the entry says.
            assert_eq!(
                verify_page(page, bind ^ 1, &entry, &mut buf).err(),
                Some(ZError::PageCrc)
            );
        }
    }

    #[test]
    fn fill_page_fills_pages_with_records_shaped_like_the_suites() {
        let images = suite_images(20_000, 1);
        assert!(
            images.iter().all(|i| i.len() == 124),
            "the suite's record is 124 bytes"
        );
        // The cut shape, as shipped: four groups a page or so, each the
        // records two kibibytes hold, the directory and the keys the cost.
        let mut s = FillScratch::new();
        let b = build_all(&images, &mut s, 99, None);
        check_build(&images, &b, 99, true);
        assert!(b.declined.is_empty());
        assert_eq!(s.stats.pages as usize, b.pages.len());
        assert_eq!(s.stats.records, 20_000);
        assert_eq!(s.stats.stored, 0);
        assert_eq!(s.stats.nopages + s.stats.fallbacks, 0);
        assert_eq!(s.stats.group_bytes, 20_000 * 124);
        let (rpp, fill, calls, gpp) = per_page(&s);
        eprintln!("suite shape, cut: {rpp:.2} records a page, fill {fill:.4}, {calls:.2} calls a page, {gpp:.2} groups a page");
        // Guard rails, not figures: the form is not worth building under 36
        // records a page, a page left a quarter empty wastes the lever, a
        // group is a compress call and a page that takes two more has lost
        // its way, and the groups are what the target makes of 124-byte
        // records: sixteen a group, three or four and a tail to a page.
        assert!(rpp >= 36.0, "{rpp} records a page is under the stop rule");
        assert!(fill > 0.9, "fill {fill}");
        assert!(
            calls < gpp + 2.0,
            "{calls} compress calls a page for {gpp} groups"
        );
        assert!((3.0..=5.0).contains(&gpp), "{gpp} groups a page");
        // The one-group shape, the page as it was before the cut: the hint
        // keeps the compress calls near one a page.
        let mut one = one_group();
        let b1 = build_all(&images, &mut one, 99, None);
        check_build_at(&images, &b1, 99, true, 0);
        assert!(b1.pages.iter().all(|(_, g, _)| g.groups == 1));
        let (rpp1, fill1, calls1, _) = per_page(&one);
        eprintln!("suite shape, one group: {rpp1:.2} records a page, fill {fill1:.4}, {calls1:.2} calls a page");
        assert!(
            rpp1 >= 36.0 && fill1 > 0.9 && calls1 < 4.0,
            "{rpp1} {fill1} {calls1}"
        );
        // The cut's price in pages: the probe priced four groups at six
        // percent of the form's bytes, and the fill here says the same
        // within a few percent of it.
        let cost = b.pages.len() as f64 / b1.pages.len() as f64;
        eprintln!("the cut costs {cost:.3} of the one-group pages");
        assert!(
            (1.02..=1.12).contains(&cost),
            "the cut's cost in pages: {cost}"
        );
    }

    #[test]
    fn a_stored_page_of_images_that_do_not_compress_holds_thirty_two_of_124_bytes() {
        // 124-byte images that are entropy throughout: LZ4 saves nothing, so
        // the groups are stored, 32 records to a page, 3,968 bytes, parsed
        // in place. Such images are no records, and the directory cannot
        // name a group by one, so under the cut a page holds one group of
        // them, sixteen.
        let mut rng = Rng::new(31);
        let images: Vec<Vec<u8>> = (0..200).map(|_| rng.bytes(124)).collect();
        for (shape, mut s, count, groups) in [
            ("cut", FillScratch::new(), 16, 1),
            ("one group", one_group(), 32, 1),
        ] {
            let b = build_all(&images, &mut s, 1, None);
            check_build_at(&images, &b, 1, false, s.group_bytes);
            for (_, g, _) in &b.pages[..b.pages.len() - 1] {
                assert_eq!((g.stored, g.groups), (groups, groups), "{shape}: {g:?}");
                assert_eq!(
                    (g.count, g.ulen, g.clen),
                    (count, count * 124, count * 124),
                    "{shape}: {g:?}"
                );
            }
            assert_eq!(s.stats.stored, s.stats.groups, "{shape}");
        }
        // The suite's record with a random value is not such an image: its
        // framing and its sequential key still compress, and the page is
        // LZ4's, with a few more records than a stored page holds.
        let framed: Vec<Vec<u8>> = (0..400)
            .map(|i| one_value(format!("{i:016}").as_bytes(), &rng.bytes(100)))
            .collect();
        let mut s = FillScratch::new();
        let b = build_all(&framed, &mut s, 1, None);
        check_build(&framed, &b, 1, true);
        assert_eq!(s.stats.stored, 0);
        assert!(per_page(&s).0 > 32.0, "{:?}", per_page(&s));
    }

    #[test]
    fn fill_page_is_total_on_adversarial_input() {
        let mut rng = Rng::new(41);
        let bind = 5;
        // Under the cut shape unless the case is the one-group shape's.
        let run = |images: &[Vec<u8>], hint: Option<usize>, target: usize| {
            let mut s = FillScratch::new();
            s.group_bytes = target;
            let b = build_all(images, &mut s, bind, hint);
            check_build_at(images, &b, bind, false, target);
            assert_eq!(s.stats.fallbacks, 0, "the turns suffice");
            (b, s)
        };
        // Records of 124 bytes with ascending keys and one value: what
        // the directory can name a group by.
        let small = |n: usize| -> Vec<Vec<u8>> {
            (0..n)
                .map(|i| one_value(format!("{i:016}").as_bytes(), &[0x55u8; 100]))
                .collect()
        };
        assert_eq!(small(1)[0].len(), 124);
        let declined = |b: &Built| -> Vec<(usize, NoPage)> { b.declined.clone() };
        let room1 = ZCAP - ZDIR_ENTRY;

        for target in [ZGROUP_BYTES, 0] {
            // No records, and a rank past the ones there are.
            let mut s = FillScratch::new();
            s.group_bytes = target;
            let mut page = [0u8; ZPAGE];
            let none: Vec<Vec<u8>> = Vec::new();
            assert_eq!(fill_page(&none, 0, bind, &mut s, &mut page), Fill::End);
            let three = small(3);
            for rank in [3usize, 4, 1000, usize::MAX] {
                assert_eq!(
                    fill_page(&three, rank, bind, &mut s, &mut page),
                    Fill::End,
                    "{rank}"
                );
            }
            assert_eq!(s.stats, FillStats::default(), "an end costs nothing");

            // The smallest images: a header's worth, and stored.
            let (b, _) = run(&[vec![1, 2, 3, 4]], None, target);
            assert!(b.pages[0].1.stored == 1 && b.pages[0].1.count == 1);
            let (b, _) = run(&[vec![9; 8]], Some(usize::MAX), target);
            assert_eq!(b.pages.len(), 1);

            // A record larger than a page that compresses is a group of
            // one; among neighbours it is still one record, and they are
            // pages, on the same page as it where they fit.
            let mut mixed = small(10);
            mixed.push(one_value(b"0000000000000010", &[b'a'; 4900]));
            mixed.extend(small(10));
            let (b, _) = run(&mixed, None, target);
            assert!(b.declined.is_empty());
            assert!(b
                .pages
                .iter()
                .any(|(r, g, _)| *r <= 10 && *r + g.count > 10));

            // One that does not compress is read from the raw segment, and
            // only it is.
            let mut mixed = small(10);
            mixed.push(one_value(b"0000000000000010", &rng.bytes(4900)));
            mixed.extend(small(10));
            let (b, _) = run(&mixed, None, target);
            assert_eq!(declined(&b), vec![(10, NoPage::Incompressible)]);

            // A lone incompressible record that fits a page's room -- the
            // cap less its one directory entry -- is stored, to the last
            // word, where its LZ4 output would not.
            for len in (4052..=room1).step_by(4) {
                let (b, _) = run(&[rng.bytes(len)], None, target);
                assert!(b.declined.is_empty(), "{len} bytes fit a page as they are");
                let (_, g, _) = &b.pages[0];
                assert!(
                    g.stored == 1 && g.clen == len && g.count == 1,
                    "{len}: {g:?}"
                );
            }
            for len in [room1 + 4, room1 + 8, 4080, 4084] {
                let (b, _) = run(&[rng.bytes(len)], None, target);
                assert_eq!(declined(&b), vec![(0, NoPage::Incompressible)], "{len}");
            }

            // The shrink that once reached zero: two records of 4,500
            // incompressible bytes, and seven of 5,100. Every record alone is
            // declined, and nothing divides by a count of none.
            let (b, s) = run(&[rng.bytes(4500), rng.bytes(4500)], Some(2), target);
            assert_eq!(
                declined(&b),
                vec![(0, NoPage::Incompressible), (1, NoPage::Incompressible)]
            );
            assert!(b.pages.is_empty());
            assert!(s.stats.turns <= 4 * u64::from(ZFILL_TURNS));
            let seven: Vec<Vec<u8>> = (0..7).map(|_| rng.bytes(5100)).collect();
            let (b, _) = run(&seven, Some(7), target);
            assert_eq!(b.declined.len(), 7);
            let (b, _) = run(&seven, Some(usize::MAX), target);
            assert_eq!(b.declined.len(), 7);

            // Over the most a group may decode to: declined without a
            // compress call, though it would compress; at the most, a page.
            let (b, s) = run(
                &[small(1).remove(0), vec![b'z'; 65_540], small(1).remove(0)],
                None,
                target,
            );
            assert_eq!(declined(&b), vec![(1, NoPage::Oversize)]);
            assert_eq!(b.pages.len(), 2);
            assert!(s.stats.calls <= 6, "{} calls", s.stats.calls);
            let (b, _) = run(&[vec![b'z'; ZMAX_ULEN]], None, target);
            assert!(b.declined.is_empty() && b.pages[0].1.ulen == ZMAX_ULEN);
            let (b, _) = run(&[rng.bytes(70_000)], None, target);
            assert_eq!(declined(&b), vec![(0, NoPage::Oversize)]);

            // A group the count binds: identical images are 256 to a page
            // in the one-group shape, whose first group needs no key;
            // under the cut, records of one value, whose keys compress on
            // their own in each group, and the room binds first.
            if target == 0 {
                let (b, _) = run(&vec![vec![0x55u8; 124]; 2000], None, 0);
                assert!(b.pages[..b.pages.len() - 1]
                    .iter()
                    .all(|(_, g, _)| g.count == ZMAX_COUNT));
                assert_eq!(b.pages.iter().map(|p| p.1.count).sum::<usize>(), 2000);
            } else {
                let (b, _) = run(&small(2000), None, target);
                assert!(b.pages[..b.pages.len() - 1]
                    .iter()
                    .all(|(_, g, _)| g.count <= ZMAX_COUNT && g.count >= 100));
                assert_eq!(b.pages.iter().map(|p| p.1.count).sum::<usize>(), 2000);
            }

            // Records of every length and every degree of compressibility.
            let varied: Vec<Vec<u8>> = (0..3000)
                .map(|_| {
                    let len = 4 * (1 + rng.below(500));
                    match rng.below(4) {
                        0 => rng.bytes(len),
                        1 => vec![0x55; len],
                        2 => (0..len).map(|i| (i % 7) as u8).collect(),
                        _ => {
                            let mut v = rng.bytes(len / 4 * 2);
                            v.resize(len, b'x');
                            v
                        }
                    }
                })
                .collect();
            for hint in [
                None,
                Some(0),
                Some(1),
                Some(255),
                Some(256),
                Some(257),
                Some(usize::MAX),
            ] {
                run(&varied, hint, target);
            }

            // Things that are not images: empty, or not a multiple of four.
            let g = small(1).remove(0);
            let bad = vec![
                vec![],
                g.clone(),
                vec![1u8; 6],
                g.clone(),
                g.clone(),
                vec![],
                vec![],
                vec![2u8; 3],
                g.clone(),
                vec![],
            ];
            let (b, _) = run(&bad, None, target);
            let want = [0usize, 2, 5, 6, 7, 9];
            assert_eq!(
                b.declined,
                want.iter()
                    .map(|&r| (r, NoPage::Unplaceable))
                    .collect::<Vec<_>>()
            );
        }

        // A group the cap binds: records of 300 identical bytes would hold
        // many more than fit 64 KiB, in the one-group shape; under the cut
        // the groups a page holds bind first, sixteen of six records.
        let (b, _) = run(&vec![vec![7u8; 300]; 600], None, 0);
        assert!(b.declined.is_empty());
        let held = ZMAX_ULEN / 300;
        assert!(
            b.pages[..b.pages.len() - 1]
                .iter()
                .all(|(_, g, _)| g.count == held && g.ulen <= ZMAX_ULEN),
            "{:?}",
            b.pages.iter().map(|p| p.1).collect::<Vec<_>>()
        );
        let framed: Vec<Vec<u8>> = (0..600)
            .map(|i| one_value(format!("{i:016}").as_bytes(), &[7u8; 276]))
            .collect();
        assert_eq!(framed[0].len(), 300);
        let (b, _) = run(&framed, None, ZGROUP_BYTES);
        assert!(b.declined.is_empty());
        assert!(
            b.pages[..b.pages.len() - 1]
                .iter()
                .all(|(_, g, _)| g.groups == ZMAX_GROUPS && g.count == 16 * 6),
            "{:?}",
            b.pages.iter().map(|p| p.1).collect::<Vec<_>>()
        );

        // Targets of every size: under a record, between, past the cap.
        let images = suite_images(3000, 44);
        for target in [1usize, 100, 124, 125, 500, 4000, ZMAX_ULEN, usize::MAX] {
            let (b, s) = run(&images, None, target);
            assert!(b.declined.is_empty(), "target {target}");
            let (rpp, _, _, gpp) = per_page(&s);
            assert!(rpp >= 1.0 && gpp >= 1.0, "target {target}: {rpp} {gpp}");
        }
    }

    #[test]
    fn the_cut_shape_names_its_groups_and_trims_the_last_to_the_room() {
        // Every group but a page's last holds what the target allows and no
        // more; the last holds what the room allows; the directory names
        // every group after the first by its first record's key; and the
        // page's used bytes are the directory, the keys and the payloads.
        let images = suite_images(6000, 17);
        for target in [512usize, 1024, ZGROUP_BYTES, 4096] {
            let mut s = FillScratch::new();
            s.group_bytes = target;
            let b = build_all(&images, &mut s, 3, None);
            check_build_at(&images, &b, 3, true, target);
            let per = target / 124;
            let (mut full, mut trimmed) = (0usize, 0usize);
            for (rank, g, page) in &b.pages {
                let v = parse_page(page, 3).unwrap();
                let mut keys_len = 0usize;
                for j in 0..v.groups() {
                    let ge = v.group(j).unwrap();
                    if j + 1 < v.groups() {
                        assert_eq!(
                            usize::from(ge.count),
                            per,
                            "rank {rank} group {j}: the target's count"
                        );
                        full += 1;
                    } else {
                        assert!(usize::from(ge.count) <= per);
                        trimmed += usize::from(usize::from(ge.count) < per);
                    }
                    if j > 0 {
                        keys_len += 2 + v.key(j).unwrap().len();
                    }
                }
                let layout = (ZHDR + ZDIR_ENTRY * v.groups() + keys_len).next_multiple_of(4) - ZHDR;
                assert!(g.used >= layout + g.clen && g.used <= layout + g.clen + 3 * g.groups);
                // The page is full to within a group's trim: the next record
                // would not have fit the room, or the groups bound it.
                assert!(
                    g.used + 130 > ZCAP
                        || g.groups == ZMAX_GROUPS
                        || *rank + g.count == images.len(),
                    "rank {rank}: {g:?} left room"
                );
            }
            assert!(full > 0, "target {target}");
            eprintln!(
                "target {target}: {} pages, {full} full groups, {trimmed} trimmed",
                b.pages.len()
            );
        }
    }

    #[test]
    fn a_call_with_no_predecessor_builds_a_smaller_page_than_one_that_has_one() {
        // The one-group shape: its first attempt is the hint, and a hint of
        // nothing starts from one record.
        let images = suite_images(4000, 8);
        let mut s = one_group();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(cold) = fill_page(&images, 2000, 1, &mut s, &mut page) else {
            panic!()
        };
        // Chained from there, the counts climb to the steady state.
        let (mut rank, mut last) = (2000 + cold.count, cold);
        for _ in 0..8 {
            let Fill::Page(g) = fill_page(&images, rank, 1, &mut s, &mut page) else {
                panic!()
            };
            (rank, last) = (rank + g.count, g);
        }
        eprintln!(
            "a cold page holds {} records, filled {:.3}; a chained one {}, filled {:.3}",
            cold.count,
            cold.clen as f64 / ZCAP as f64,
            last.count,
            last.clen as f64 / ZCAP as f64
        );
        assert!(
            cold.count * 10 < last.count * 8,
            "a cold call is not the steady state: {} against {}",
            cold.count,
            last.count
        );
        assert!(cold.clen < last.clen);
        // The cut shape has no hint: its first page is its steady state.
        let mut c = FillScratch::new();
        let Fill::Page(first) = fill_page(&images, 2000, 1, &mut c, &mut page) else {
            panic!()
        };
        let Fill::Page(next) = fill_page(&images, 2000 + first.count, 1, &mut c, &mut page) else {
            panic!()
        };
        assert!(
            first.count * 10 >= next.count * 9,
            "{first:?} against {next:?}"
        );
    }

    #[test]
    fn a_group_that_ends_exactly_at_capacity_is_a_page() {
        let mut rng = Rng::new(43);
        let mut s = one_group();
        let bind = 3;
        // The one-group page's room: the cap less its directory entry.
        let room1 = ZCAP - ZDIR_ENTRY;
        // Stored: thirty-one of 124 bytes and one of 220 make 4,064.
        let mut images: Vec<Vec<u8>> = (0..31).map(|_| rng.bytes(124)).collect();
        images.push(rng.bytes(room1 - 31 * 124));
        for hint in [0usize, 1, 32, 100] {
            let mut page = [0u8; ZPAGE];
            s.hint = hint;
            let f = fill_page(&images, 0, bind, &mut s, &mut page);
            let Fill::Page(g) = f else { panic!("{f:?}") };
            assert!(
                g.stored == 1 && g.count == 32 && g.clen == room1 && g.used == ZCAP,
                "hint {hint}: {g:?}"
            );
        }
        // One word more is over: the last record leaves the group.
        let mut over = images.clone();
        over[31] = rng.bytes(room1 - 31 * 124 + 4);
        let mut page = [0u8; ZPAGE];
        s.hint = 32;
        let Fill::Page(g) = fill_page(&over, 0, bind, &mut s, &mut page) else {
            panic!()
        };
        assert!(g.count < 32 && g.used <= ZCAP, "{g:?}");

        // Compressed: some group of the suite's records and a tail of
        // entropy compresses to exactly the room. Found, not assumed: the
        // length of the block moves with the tail by four, and by five where
        // a length byte joins, so a base may skip it.
        let mut found = None;
        'search: for seed in 0..200 {
            let suite = suite_images(80, 9 + seed);
            for base in 36..60 {
                let head: Vec<u8> = suite[..base].concat();
                for words in 0..300 {
                    let tail = rng.bytes(4 * words + 4);
                    let mut group = head.clone();
                    group.extend_from_slice(&tail);
                    let c = lz4_flex::block::compress(&group).len();
                    if c == room1 {
                        found = Some((suite[..base].to_vec(), tail));
                        break 'search;
                    }
                    if c > room1 {
                        break;
                    }
                }
            }
        }
        let (mut images, tail) = found.expect("some group compresses to exactly the room");
        images.push(tail);
        let mut page = [0u8; ZPAGE];
        s.hint = images.len();
        let f = fill_page(&images, 0, bind, &mut s, &mut page);
        let Fill::Page(g) = f else { panic!("{f:?}") };
        assert!(
            g.stored == 0 && g.clen == room1 && g.count == images.len(),
            "{g:?}"
        );
        assert_eq!(
            usize::from(parse_page(&page, bind).unwrap().header.used),
            ZCAP
        );
        // And one more word is over, so the group gives a record up.
        let mut longer = images.clone();
        longer.push(vec![0xA5, 0x5A, 0xA5, 0x5A]);
        let mut page2 = [0u8; ZPAGE];
        s.hint = longer.len();
        let f = fill_page(&longer, 0, bind, &mut s, &mut page2);
        let Fill::Page(g2) = f else { panic!("{f:?}") };
        assert!(g2.used <= ZCAP && g2.count <= images.len() + 1, "{g2:?}");
        // Whatever was built decodes to its records.
        let b = build_all(&longer, &mut s, bind, None);
        check_build_at(&longer, &b, bind, false, 0);
    }

    #[test]
    fn fill_page_is_deterministic() {
        let images = suite_images(5000, 2);
        let fingerprint = |b: &Built| -> Vec<(usize, Filled, u32)> {
            b.pages.iter().map(|(r, g, p)| (*r, *g, crc32(p))).collect()
        };
        for target in [ZGROUP_BYTES, 0] {
            let mut fresh = FillScratch::new();
            fresh.group_bytes = target;
            let a = build_all(&images, &mut fresh, 7, None);
            let want = fingerprint(&a);
            // Again, with a scratch the first build dirtied.
            let b = build_all(&images, &mut fresh, 7, None);
            assert_eq!(fingerprint(&b), want, "a reused scratch");
            // A scratch dirtied by something else entirely: entropy, big
            // records, records that are declined.
            let mut rng = Rng::new(3);
            let mut dirty = FillScratch::new();
            dirty.group_bytes = target;
            let junk: Vec<Vec<u8>> = (0..400)
                .map(|i| rng.bytes(4 * (1 + (i * 37) % 1500)))
                .chain(std::iter::once(vec![b'q'; 70_000]))
                .collect();
            build_all(&junk, &mut dirty, 1, None);
            let c = build_all(&images, &mut dirty, 7, None);
            assert_eq!(fingerprint(&c), want, "a scratch dirtied by other records");
            // Another thread.
            let moved = {
                let images = images.clone();
                std::thread::spawn(move || {
                    let mut s = FillScratch::new();
                    s.group_bytes = target;
                    let d = build_all(&images, &mut s, 7, None);
                    fingerprint(&d)
                })
                .join()
                .unwrap()
            };
            assert_eq!(moved, want, "another thread");
        }
        // The bind32 is in the CRC and nowhere else in the page.
        let images = suite_images(200, 2);
        let x = build_all(&images, &mut FillScratch::new(), 1, None);
        let y = build_all(&images, &mut FillScratch::new(), 2, None);
        for ((_, g1, p1), (_, g2, p2)) in x.pages.iter().zip(&y.pages) {
            assert_eq!(g1, g2);
            assert_eq!(p1[4..], p2[4..]);
            assert_ne!(p1[..4], p2[..4]);
        }
    }

    #[test]
    fn the_turns_are_spent_at_most_and_a_spent_budget_still_yields_a_page() {
        let images = suite_images(3000, 4);
        // The one-group shape with a hint far past what fits on every call,
        // and a budget of one turn: the first group is too big, the turn is
        // spent, and the call falls back to the first record alone.
        for turns in [1u32, 2, 3, 5, 8] {
            let mut s = one_group();
            let mut rank = 0;
            let mut pages = 0;
            while rank < images.len() {
                let mut page = [0u8; ZPAGE];
                s.hint = 200;
                let f = fill_with_turns(&images, rank, 8, &mut s, &mut page, turns);
                let Fill::Page(g) = f else {
                    panic!("turns {turns} rank {rank}: {f:?}")
                };
                // The records are the images', and the page parses.
                let group: Vec<u8> = images[rank..rank + g.count].concat();
                let v = parse_page(&page, 8).unwrap();
                let mut buf = GroupBuf::new();
                assert_eq!(decode_group(&v, 0, &mut buf).unwrap(), &group[..]);
                rank += g.count;
                pages += 1;
            }
            assert!(pages > 0);
            if turns == 1 {
                assert!(
                    s.stats.fallbacks > 0,
                    "a budget of one turn must reach the fallback"
                );
            }
            assert!(
                s.stats.turns
                    <= u64::from(turns) * s.stats.pages + s.stats.nopages * u64::from(turns)
            );
        }
        // With the production budget the fallback is not reached on any
        // records the other tests build, under either shape: the arithmetic
        // of the turns says it cannot be.
        let mut rng = Rng::new(55);
        let hard: Vec<Vec<u8>> = (0..800)
            .map(|_| {
                let len = 4 * (1 + rng.below(1200));
                if rng.below(3) == 0 {
                    rng.bytes(len)
                } else {
                    vec![0x33; len]
                }
            })
            .collect();
        for target in [0usize, ZGROUP_BYTES] {
            let mut s = FillScratch::new();
            s.group_bytes = target;
            for hint in [0usize, 1, 7, 255, 256, usize::MAX] {
                build_all(&hard, &mut s, 1, Some(hint));
            }
            build_all(&images, &mut s, 1, None);
            assert_eq!(s.stats.fallbacks, 0, "target {target}");
            assert!(s.stats.turns > 0);
        }
    }

    #[test]
    fn growth_rounds_trade_compress_calls_for_fill() {
        let images = suite_images(30_000, 6);
        let mut rows = Vec::new();
        for rounds in 0..=3u32 {
            let mut s = one_group();
            s.grow_rounds = rounds;
            let b = build_from(&images, &mut s, 1, None, 60);
            check_build_at(&images, &b, 1, true, 0);
            let (rpp, fill, calls, _) = per_page(&s);
            eprintln!("grow_rounds {rounds}: {rpp:.2} records a page, fill {fill:.4}, {calls:.3} calls a page");
            rows.push((rpp, fill, calls));
        }
        for w in rows.windows(2) {
            assert!(
                w[1].2 + 1e-9 >= w[0].2,
                "more rounds, no fewer calls: {rows:?}"
            );
            assert!(
                w[1].1 + 0.01 >= w[0].1,
                "more rounds, no emptier pages: {rows:?}"
            );
        }
        assert!(
            rows[1].1 > rows[0].1,
            "the first growth round pays: {rows:?}"
        );
    }

    // ---- the builder's guard against a page that is wrong -----------------

    /// A group of the suite's records staged and compressed as `fill_page`
    /// leaves it at the moment it accepts the group, with the scratch it is
    /// in: the compressed bytes in the output buffer, not yet accepted.
    fn fit_of(images: &[Vec<u8>], m: usize) -> (FillScratch, Fit) {
        let mut s = FillScratch::new();
        s.reset_page();
        assert!(matches!(s.stage(images, 0, m), Stage::Ok));
        let ulen = s.imgs.len();
        let Some(Class::Lz4(clen, coded)) = s.classify(ulen, ZCAP) else {
            panic!("{m} of the suite's records compress into a page")
        };
        let body = Body::Lz4 { clen, coded };
        (s, Fit { m, ulen, body })
    }

    #[test]
    fn a_page_that_does_not_decode_to_its_images_is_not_returned() {
        let images = suite_images(60, 15);
        let (bind, want) = (31, images[..30].concat());
        // The page `seal_page` makes of a fit accepted, and what a refused
        // one leaves: no page counted, whatever the compress calls before it
        // cost.
        let seal = |s: &mut FillScratch, fit: Fit| -> Fill {
            let before = s.stats;
            s.accept(fit);
            let got = seal_page(s, 0, 1, bind, &mut [0u8; ZPAGE]);
            if got == Fill::Mismatch {
                assert_eq!(s.stats, before, "a refused page is not counted");
            }
            got
        };
        // The control: the group as it was compressed is a page, counted once.
        let (mut s, fit) = fit_of(&images, 30);
        let mut page = [0u8; ZPAGE];
        s.accept(fit);
        let Fill::Page(g) = seal_page(&mut s, 0, 1, bind, &mut page) else {
            panic!("the group as compressed is a page")
        };
        assert_eq!((g.count, s.stats.pages, s.stats.records), (30, 1, 30));
        let v = parse_page(&page, bind).unwrap();
        let mut buf = GroupBuf::new();
        assert_eq!(decode_group(&v, 0, &mut buf).unwrap(), &want[..]);

        // The compressor's block damaged after the group was judged to fit,
        // at every byte. The CRC is computed over what was written, so the
        // page is signed either way and only the comparison can refuse it:
        // it must agree, byte by byte, with an oracle that decodes the
        // damaged block itself.
        let Body::Lz4 { clen, .. } = fit.body else {
            unreachable!()
        };
        let mut harmful = 0;
        for at in 0..clen {
            let (mut s, fit) = fit_of(&images, 30);
            s.out[at] ^= 0x20;
            let mut back = vec![0u8; fit.ulen];
            let decoded = lz4_flex::block::decompress_into(&s.out[..clen], &mut back);
            let intact = decoded.ok() == Some(fit.ulen) && back == want;
            let got = seal(&mut s, fit);
            if intact {
                assert!(
                    matches!(got, Fill::Page(_)),
                    "byte {at}: the group is intact"
                );
            } else {
                harmful += 1;
                assert_eq!(got, Fill::Mismatch, "byte {at}");
            }
        }
        assert!(harmful * 10 > clen * 9, "{harmful} of {clen} bytes matter");

        // The images changed after the compress: the page decodes to the
        // group that was compressed, which is no longer them.
        let (mut s, fit) = fit_of(&images, 30);
        s.imgs[7] ^= 1;
        assert_eq!(seal(&mut s, fit), Fill::Mismatch);

        // A directory whose length is not the block's, shorter and longer.
        let (mut s, fit) = fit_of(&images, 30);
        let shorter = Fit {
            ulen: fit.ulen - 4,
            ..fit
        };
        assert_eq!(seal(&mut s, shorter), Fill::Mismatch);
        let (mut s, fit) = fit_of(&images, 30);
        assert!(matches!(s.stage(&images, 0, 31), Stage::Ok));
        let longer = Fit {
            ulen: s.imgs.len(),
            ..fit
        };
        assert_eq!(seal(&mut s, longer), Fill::Mismatch);

        // A fit no page can describe, a payload over a page, is no page.
        let (mut s, fit) = fit_of(&images, 30);
        let huge = Fit {
            body: Body::Lz4 {
                clen: ZCAP + 1,
                coded: false,
            },
            ..fit
        };
        assert_eq!(seal(&mut s, huge), Fill::Mismatch);

        // Two groups, the second's images changed after its compress: the
        // page decodes group by group, and the second refuses it.
        let (mut s, fit) = fit_of(&images, 30);
        s.accept(fit);
        assert!(matches!(s.stage(&images, 30, 20), Stage::Ok));
        let ulen = s.imgs.len();
        let Some(Class::Lz4(clen, coded)) = s.classify(ulen, ZCAP) else {
            panic!()
        };
        s.imgs[ulen - 3] ^= 1;
        s.accept(Fit {
            m: 20,
            ulen,
            body: Body::Lz4 { clen, coded },
        });
        assert_eq!(seal_page(&mut s, 0, 2, bind, &mut page), Fill::Mismatch);
    }

    #[test]
    fn a_page_is_compared_as_a_reader_reads_it() {
        let images = small_groups().remove(0).0;
        let (group, bind) = (images.concat(), 5);
        let page = manual_page(&group, images.len() as u16, 9, bind, false);
        let hdr = parse_page(&page, bind).unwrap().header;
        let mut v = GroupBuf::new();
        assert!(page_decodes_to(&page, bind, &hdr, &group, &mut v, false));
        // Not the group it was asked for.
        let mut other = group.clone();
        other[3] ^= 1;
        assert!(!page_decodes_to(&page, bind, &hdr, &other, &mut v, false));
        assert!(!page_decodes_to(
            &page,
            bind,
            &hdr,
            &group[..group.len() - 4],
            &mut v,
            false
        ));
        // Not the header it was asked for.
        for bad in [
            PageHeader {
                first_rank: 10,
                ..hdr
            },
            PageHeader {
                count: hdr.count - 1,
                ..hdr
            },
        ] {
            assert!(
                !page_decodes_to(&page, bind, &bad, &group, &mut v, false),
                "{bad:?}"
            );
        }
        // Not a page this form file reads: another bind32, or a byte changed.
        assert!(!page_decodes_to(
            &page,
            bind ^ 1,
            &hdr,
            &group,
            &mut v,
            false
        ));
        let mut torn = page;
        torn[100] ^= 1;
        assert!(!page_decodes_to(&torn, bind, &hdr, &group, &mut v, false));
        // A page whose CRC is good and whose payload is not the group: the
        // payload begins after the one directory entry, at 32.
        let mut signed = page;
        signed[32 + 3] ^= 0x40;
        resign_page(&mut signed, bind);
        assert!(parse_page(&signed, bind).is_ok(), "its CRC is good");
        assert!(!page_decodes_to(&signed, bind, &hdr, &group, &mut v, false));
        // A page of two groups compares group by group: the second's payload
        // changed under a good CRC is refused.
        let two = suite_images(20, 6);
        let (g0, g1) = (two[..12].concat(), two[12..].concat());
        let k1 = record_key(&two[12], 0).unwrap();
        let page2 = manual_pages(&[(&g0, 12, false, &[]), (&g1, 8, false, k1)], 0, bind);
        let v2 = parse_page(&page2, bind).unwrap();
        let (hdr2, coff1) = (v2.header, usize::from(v2.group(1).unwrap().coff));
        let both = [g0.clone(), g1.clone()].concat();
        assert!(page_decodes_to(&page2, bind, &hdr2, &both, &mut v, false));
        let mut signed2 = page2;
        signed2[coff1 + 2] ^= 0x40;
        resign_page(&mut signed2, bind);
        assert!(parse_page(&signed2, bind).is_ok());
        assert!(!page_decodes_to(
            &signed2, bind, &hdr2, &both, &mut v, false
        ));
    }

    #[test]
    fn a_compressor_that_fails_fails_the_build_and_returns_no_page() {
        let images = suite_images(200, 16);
        let mut page = [0u8; ZPAGE];
        for target in [ZGROUP_BYTES, 0] {
            let call = |s: &mut FillScratch, page: &mut [u8; ZPAGE]| {
                s.group_bytes = target;
                s.hint = 10;
                fill_page(&images, 0, 1, s, page)
            };
            // The control: with the buffer whole, the call is a page.
            let mut s = FillScratch::new();
            assert!(matches!(call(&mut s, &mut page), Fill::Page(_)));
            // The first compress of a call, into a buffer too small for the
            // worst case, which `compress_into` refuses whatever the data is.
            let mut s = FillScratch::new();
            s.out.truncate(8);
            assert_eq!(call(&mut s, &mut page), Fill::Mismatch);
            assert_eq!(s.stats.pages, 0);
        }
    }

    // ---- a group sought a part at a time ----------------------------------

    /// The suite's records in pages as a builder fills them, decoded by the
    /// flat loop (`Dec::Flat`, and `Dec::FlatEnd` with the room it runs to
    /// the group's end in) whole, a part at a time and sought a part at a
    /// time: every byte, length, offset and stop the fast loop's
    /// (`Dec::Own`) gives the same calls.
    #[test]
    fn a_group_decoded_by_the_flat_loop_is_the_fast_loops_whole_in_parts_and_sought() {
        let images = suite_images(3000, 29);
        let bind = 11;
        let mut s = FillScratch::new();
        let (mut rank, mut pages) = (0usize, Vec::new());
        while rank < images.len() {
            let mut page = [0u8; ZPAGE];
            match fill_page(&images, rank, bind, &mut s, &mut page) {
                Fill::Page(g) => {
                    pages.push((rank, page));
                    rank += g.count;
                }
                other => panic!("rank {rank}: {other:?}"),
            }
        }
        let (mut fast, mut flat) = (GroupBuf::new(), GroupBuf::new());
        let (mut fast_part, mut flat_part) = (GroupBuf::new(), GroupBuf::new());
        let mut groups = 0usize;
        for (first, page) in &pages {
            let v = parse_page(page, bind).unwrap();
            let mut at = *first;
            for j in 0..v.groups() {
                let count = usize::from(v.group(j).unwrap().count);
                let g = decode_group_with(&v, j, &mut fast, Dec::Own)
                    .unwrap()
                    .to_vec();
                // The flat loop lent the fast loop's slack and the room it
                // runs to the group's end in (`Dec::FlatEnd`).
                for dec in [Dec::Flat, Dec::FlatEnd] {
                    let f = decode_group_with(&v, j, &mut flat, dec).unwrap();
                    assert!(
                        f == &g[..],
                        "page at {first}, group {j}, {dec:?}: the whole decode"
                    );
                    for probe in [
                        format!("{at:016}").into_bytes(),
                        format!("{:016}", at + count / 2).into_bytes(),
                        format!("{:016}x", at + count - 1).into_bytes(),
                        b"z".to_vec(),
                    ] {
                        let a = seek_group_partly_with(&v, j, &mut fast_part, &probe, Dec::Own)
                            .unwrap()
                            .unwrap();
                        let b = seek_group_partly_with(&v, j, &mut flat_part, &probe, dec)
                            .unwrap()
                            .unwrap();
                        assert_eq!((a.off, a.len, a.of), (b.off, b.len, b.of), "a seek");
                        assert!(flat_part.bytes(b.len) == &g[..b.len], "the seek's prefix");
                    }
                    for step in [1usize, 64, 300] {
                        let mut p = open_group_partly_with(&v, j, &mut flat_part, 0, dec)
                            .unwrap()
                            .expect("a block-coded group");
                        let mut before = p.have();
                        while !p.done() {
                            let got =
                                decode_group_more(&mut p, &mut flat_part, before + step).unwrap();
                            assert!(got > before || p.done(), "a call goes on");
                            assert!(flat_part.bytes(got) == &g[..got], "step {step}: a prefix");
                            before = got;
                        }
                        assert_eq!(p.have(), g.len());
                    }
                }
                at += count;
                groups += 1;
            }
        }
        assert!(groups > 20, "{groups} groups");
    }

    /// The suite's records in pages as a builder fills them, every group one
    /// LZ4 block: each key of a group, a key just past it and one above the
    /// whole group, sought by the whole decode's walk and by the partial
    /// one. The same record at the same offset, the bytes decoded a prefix
    /// of the whole decode's that holds the record whole, and over every
    /// key of a group about half of it decoded.
    #[test]
    fn a_groups_partial_seek_is_the_whole_seek_and_stops_near_its_record() {
        let images = suite_images(3000, 23);
        let bind = 7;
        let mut s = FillScratch::new();
        let (mut rank, mut pages) = (0usize, Vec::new());
        while rank < images.len() {
            let mut page = [0u8; ZPAGE];
            match fill_page(&images, rank, bind, &mut s, &mut page) {
                Fill::Page(g) => {
                    pages.push((rank, page));
                    rank += g.count;
                }
                other => panic!("rank {rank}: {other:?}"),
            }
        }
        let (mut whole, mut part) = (GroupBuf::new(), GroupBuf::new());
        let (mut seeks, mut short, mut bytes, mut of) = (0usize, 0usize, 0usize, 0usize);
        let mut groups = 0usize;
        for (first, page) in &pages {
            let v = parse_page(page, bind).unwrap();
            let mut at = *first;
            for j in 0..v.groups() {
                let ge = v.group(j).unwrap();
                let count = usize::from(ge.count);
                assert!(!ge.stored && !ge.coded, "the suite's records compress");
                groups += 1;
                let g = decode_group_with(&v, j, &mut whole, true).unwrap();
                let mut probes: Vec<Vec<u8>> = Vec::new();
                for r in at..at + count {
                    let key = format!("{r:016}").into_bytes();
                    probes.push([key.as_slice(), b"x"].concat());
                    probes.push(key);
                }
                for probe in &probes {
                    let want = group_seek(g, count, probe).unwrap();
                    let got = seek_group_partly(&v, j, &mut part, probe)
                        .unwrap()
                        .expect("a block-coded group");
                    assert_eq!(got.off, want.found.then_some(want.off));
                    assert_eq!(got.of, g.len());
                    assert!(got.len <= got.of);
                    assert!(
                        part.bytes(got.len) == &g[..got.len],
                        "a prefix of the group"
                    );
                    if let Some(off) = got.off {
                        let end = record_key_next(g, off).unwrap().1;
                        assert!(end <= got.len, "the record whole in what was decoded");
                    }
                    seeks += 1;
                    short += usize::from(got.len < got.of);
                    bytes += got.len;
                    of += got.of;
                }
                // A key above every key of the group walks it to its end,
                // and one below its first stops at the first record.
                let above = seek_group_partly(&v, j, &mut part, b"z").unwrap().unwrap();
                assert_eq!((above.off, above.len), (None, g.len()));
                assert!(
                    part.bytes(g.len()) == g,
                    "the whole group, a part at a time"
                );
                let below = seek_group_partly(&v, j, &mut part, b"!").unwrap().unwrap();
                assert_eq!(below.off, None);
                assert!(below.len <= (2 * PART_STEP).min(g.len()), "{}", below.len);
                // The empty key is answered by the first record with no
                // compare, as the whole walk answers it: absent here,
                // where no key is empty.
                let empty = seek_group_partly(&v, j, &mut part, b"").unwrap().unwrap();
                let want = group_seek(g, count, b"").unwrap();
                assert_eq!(empty.off, want.found.then_some(want.off));
                assert!(empty.len <= (2 * PART_STEP).min(g.len()));
                at += count;
            }
        }
        assert!(
            groups > 100 && seeks > 5000,
            "{groups} groups, {seeks} seeks"
        );
        assert!(
            bytes * 100 < of * 80 && short * 2 > seeks,
            "{bytes} of {of} bytes decoded, {short} of {seeks} seeks short of the group"
        );
    }

    /// Bytes that changed under a verified page: every byte of a group's
    /// payload flipped in turn and keys sought through the damage. The
    /// partial seek returns and never panics, and where the whole decode
    /// still decodes the payload, what the partial one decoded is a prefix
    /// of the same bytes -- one decoder over one input -- so it answers
    /// what the whole walk answers from them. What it does not see is what
    /// a whole decode learns at the block's end, a length that disagrees:
    /// neither path reads the page's CRC again, and a page is held to it
    /// once, at its first decode.
    #[test]
    fn a_partial_seek_over_a_damaged_payload_reads_a_prefix_of_the_whole_decodes_bytes() {
        let images = suite_images(40, 5);
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(_) = fill_page(&images, 0, 3, &mut s, &mut page) else {
            panic!("a page")
        };
        let v = parse_page(&page, 3).unwrap();
        let ge = v.group(0).unwrap();
        assert!(!ge.stored && !ge.coded);
        let count = usize::from(ge.count);
        let (coff, clen) = (usize::from(ge.coff), usize::from(ge.clen));
        let probes: Vec<Vec<u8>> = vec![
            format!("{:016}", 0).into_bytes(),
            format!("{:016}", count / 2).into_bytes(),
            format!("{:016}", count - 1).into_bytes(),
            b"z".to_vec(),
        ];
        let (mut whole, mut part) = (GroupBuf::new(), GroupBuf::new());
        let (mut refused, mut prefixes, mut whole_refused) = (0usize, 0usize, 0usize);
        for i in coff..coff + clen {
            for bit in [1u8, 0x10, 0x80] {
                let mut bad = page;
                bad[i] ^= bit;
                // The damaged page's view, its CRC not asked.
                let Ok(bv) = page_view(&bad) else { continue };
                let w = decode_group_with(&bv, 0, &mut whole, true).map(|g| g.to_vec());
                whole_refused += usize::from(w.is_err());
                for probe in &probes {
                    match seek_group_partly(&bv, 0, &mut part, probe) {
                        Err(_) => refused += 1,
                        Ok(None) => panic!("a block-coded group"),
                        Ok(Some(p)) => {
                            assert!(p.len <= p.of && p.of == ge.ulen as usize);
                            if let Ok(g) = &w {
                                assert!(
                                    part.bytes(p.len) == &g[..p.len],
                                    "byte {i} bit {bit:#x}: not the whole decode's prefix"
                                );
                                if let Ok(want) = group_seek(g, count, probe) {
                                    assert_eq!(p.off, want.found.then_some(want.off));
                                }
                                prefixes += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            refused > 0 && prefixes > 0 && whole_refused > 0,
            "{refused} refused, {prefixes} prefixes held, {whole_refused} whole decodes refused"
        );
    }

    /// A directory that names a group four bytes shorter than its payload
    /// decodes to, the payload as it was. The whole decode refuses the
    /// length at the block's end. A partial seek whose decode runs that far
    /// refuses it too, and its own check is the only thing that does: the
    /// decoder's bound is the stated length and its slack, which the four
    /// bytes are inside, and the record sought lies whole in the stated
    /// length. The check a verified page cannot reach, held here by the
    /// forged directory; short records, so that the step for the last whole
    /// record takes the decode to the block's end.
    #[test]
    fn a_partial_seek_refuses_a_payload_that_decodes_past_its_groups_length() {
        let images: Vec<Vec<u8>> = (0..40)
            .map(|i| one_value(format!("{i:016}").as_bytes(), b"foxfoxfo"))
            .collect();
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(_) = fill_page(&images, 0, 3, &mut s, &mut page) else {
            panic!("a page")
        };
        let v = parse_page(&page, 3).unwrap();
        let ge = v.group(0).unwrap();
        assert!(!ge.stored && !ge.coded, "a block-coded group: {ge:?}");
        let count = usize::from(ge.count);
        assert!(count >= 3);
        let mut whole = GroupBuf::new();
        let g = decode_group_with(&v, 0, &mut whole, true).unwrap().to_vec();
        // The record before the last: its start and its key.
        let (mut off, mut key) = (0usize, Vec::new());
        for i in 0..count - 1 {
            let (k, next) = record_key_next(&g, off).unwrap();
            if i == count - 2 {
                key = k.to_vec();
            } else {
                off = next;
            }
        }
        // Through the page as written the seek answers the record, and its
        // decode has run to the group's end by then.
        let mut part = GroupBuf::new();
        let got = seek_group_partly(&v, 0, &mut part, &key).unwrap().unwrap();
        assert_eq!((got.off, got.len), (Some(off), g.len()));
        // The group's length in the directory, and the header's sum of the
        // groups' lengths by as much, which the view holds the directory to.
        let cut = 4usize;
        assert!(cut < super::super::zlz4::DECODE_SLACK);
        let mut bad = page;
        let at = ZHDR + D_ULEN4;
        bad[at..at + 2].copy_from_slice(&(((g.len() - cut) / 4) as u16).to_le_bytes());
        let sum = u32::from_le_bytes(bad[P_ULEN..P_ULEN + 4].try_into().unwrap());
        bad[P_ULEN..P_ULEN + 4].copy_from_slice(&(sum - cut as u32).to_le_bytes());
        let bv = page_view(&bad).expect("a view of the forged page");
        assert_eq!(bv.group(0).unwrap().ulen as usize, g.len() - cut);
        assert!(
            decode_group_with(&bv, 0, &mut whole, true).is_err(),
            "the whole decode refuses the length"
        );
        assert!(
            seek_group_partly(&bv, 0, &mut part, &key).is_err(),
            "a decode past the stated length is refused, not answered from"
        );
    }

    // ---- a group decoded a part at a time for a walk ----------------------

    /// The suite's records in pages as a builder fills them: every group
    /// begun at a spread of stops and taken on in steps of several sizes.
    /// What is decoded is at every step a prefix of the whole decode's
    /// bytes, past what was asked or the whole group, a call goes on by a
    /// sequence at the least, and the last leaves the whole group and says
    /// so. A group begun for nothing stops well short of its end.
    #[test]
    fn a_group_decoded_a_part_at_a_time_is_a_prefix_of_the_whole_at_every_step() {
        let images = suite_images(3000, 37);
        let bind = 9;
        let mut s = FillScratch::new();
        let (mut rank, mut pages) = (0usize, Vec::new());
        while rank < images.len() {
            let mut page = [0u8; ZPAGE];
            match fill_page(&images, rank, bind, &mut s, &mut page) {
                Fill::Page(g) => {
                    pages.push(page);
                    rank += g.count;
                }
                other => panic!("rank {rank}: {other:?}"),
            }
        }
        let (mut whole, mut part) = (GroupBuf::new(), GroupBuf::new());
        let (mut groups, mut steps, mut begun0, mut of0) = (0usize, 0usize, 0usize, 0usize);
        for page in &pages {
            let v = parse_page(page, bind).unwrap();
            for j in 0..v.groups() {
                let ge = v.group(j).unwrap();
                assert!(!ge.stored && !ge.coded, "the suite's records compress");
                groups += 1;
                let g = decode_group_with(&v, j, &mut whole, true).unwrap().to_vec();
                let n = g.len();
                for (stop, step) in [
                    (0, 1),
                    (1, 64),
                    (n / 4, PART_STEP),
                    (n / 2, 7),
                    (n - 1, 1),
                    (n, 1),
                    (usize::MAX, 1),
                ] {
                    let mut p = open_group_partly(&v, j, &mut part, stop)
                        .unwrap()
                        .expect("a block-coded group");
                    assert_eq!(p.ulen(), n);
                    assert!(p.done() || p.have() > stop, "stop {stop}: {}", p.have());
                    assert_eq!(p.done(), p.have() == n, "stop {stop}");
                    assert!(part.bytes(p.have()) == &g[..p.have()], "a prefix");
                    if stop == 0 {
                        begun0 += p.have();
                        of0 += n;
                    }
                    while !p.done() {
                        let before = p.have();
                        let got = decode_group_more(&mut p, &mut part, before + step).unwrap();
                        assert_eq!(got, p.have());
                        assert!(got > before, "a call goes on");
                        assert!(p.done() || got > before + step, "past what was asked");
                        assert!(part.bytes(got) == &g[..got], "a prefix still");
                        steps += 1;
                    }
                    assert!(part.bytes(n) == g.as_slice(), "the whole group at the end");
                    // A group at its end is left as it is.
                    assert_eq!(decode_group_more(&mut p, &mut part, usize::MAX).unwrap(), n);
                    assert!(part.bytes(n) == g.as_slice());
                }
            }
        }
        assert!(
            groups > 100 && steps > 5000,
            "{groups} groups, {steps} steps"
        );
        assert!(
            begun0 * 4 < of0,
            "{begun0} of {of0} bytes decoded by a begin that asked for none"
        );
    }

    /// A group that is stored or record-coded is not begun, and the caller
    /// opens it as it lies; a group the page does not have is an error.
    #[test]
    fn a_stored_or_record_coded_group_is_not_decoded_a_part_at_a_time() {
        let mut part = GroupBuf::new();
        let mut rng = Rng::new(31);
        let noise: Vec<Vec<u8>> = (0..40).map(|_| rng.bytes(124)).collect();
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(_) = fill_page(&noise, 0, 3, &mut s, &mut page) else {
            panic!("a page")
        };
        let v = parse_page(&page, 3).unwrap();
        assert!(v.group(0).unwrap().stored);
        assert!(open_group_partly(&v, 0, &mut part, 64).unwrap().is_none());
        assert!(open_group_partly(&v, v.groups(), &mut part, 64).is_err());

        let images = suite_images(200, 5);
        let mut s = FillScratch::new();
        s.coding = ZCODING_RECORD;
        let Fill::Page(_) = fill_page(&images, 0, 3, &mut s, &mut page) else {
            panic!("a page")
        };
        let v = parse_page(&page, 3).unwrap();
        let mut coded = 0usize;
        for j in 0..v.groups() {
            if v.group(j).unwrap().coded {
                coded += 1;
                assert!(open_group_partly(&v, j, &mut part, 64).unwrap().is_none());
            }
        }
        assert!(coded > 0, "a record-coded group in the page");
    }

    /// Bytes that changed under a verified page, as the partial seek's test
    /// has them: every byte of a group's payload flipped in turn and the
    /// group decoded a part at a time through the damage. It returns and
    /// never panics, what it holds at every step is no longer than the
    /// group, it reaches the group's end only where the whole decode
    /// decodes the payload, and where that does, every step is a prefix of
    /// the same bytes.
    #[test]
    fn a_damaged_payload_decoded_a_part_at_a_time_is_the_whole_decodes_prefix_or_refused() {
        let images = suite_images(40, 5);
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(_) = fill_page(&images, 0, 3, &mut s, &mut page) else {
            panic!("a page")
        };
        let v = parse_page(&page, 3).unwrap();
        let ge = v.group(0).unwrap();
        assert!(!ge.stored && !ge.coded);
        let ulen = ge.ulen as usize;
        let (coff, clen) = (usize::from(ge.coff), usize::from(ge.clen));
        let (mut whole, mut part) = (GroupBuf::new(), GroupBuf::new());
        let (mut refused, mut prefixes, mut ended) = (0usize, 0usize, 0usize);
        for i in coff..coff + clen {
            for bit in [1u8, 0x10, 0x80] {
                let mut bad = page;
                bad[i] ^= bit;
                let Ok(bv) = page_view(&bad) else { continue };
                let w = decode_group_with(&bv, 0, &mut whole, true).map(|g| g.to_vec());
                for (stop, step) in [(0usize, 1usize), (ulen / 2, PART_STEP)] {
                    let mut p = match open_group_partly(&bv, 0, &mut part, stop) {
                        Err(_) => {
                            refused += 1;
                            continue;
                        }
                        Ok(None) => panic!("a block-coded group"),
                        Ok(Some(p)) => p,
                    };
                    loop {
                        assert!(p.have() <= ulen && p.ulen() == ulen);
                        if let Ok(g) = &w {
                            assert!(
                                part.bytes(p.have()) == &g[..p.have()],
                                "byte {i} bit {bit:#x}: not the whole decode's prefix"
                            );
                            prefixes += 1;
                        }
                        if p.done() {
                            assert!(
                                w.is_ok(),
                                "byte {i} bit {bit:#x}: an end the whole decode refuses"
                            );
                            assert_eq!(p.have(), ulen);
                            ended += 1;
                            break;
                        }
                        let before = p.have();
                        match decode_group_more(&mut p, &mut part, before + step) {
                            Err(_) => {
                                refused += 1;
                                break;
                            }
                            Ok(got) => assert!(got > before, "a call goes on"),
                        }
                    }
                }
            }
        }
        assert!(
            refused > 0 && prefixes > 0 && ended > 0,
            "{refused} refused, {prefixes} prefixes held, {ended} ended"
        );
    }

    /// The directory forged four bytes short of what the payload decodes
    /// to, as the partial seek's test forges it: a decode a part at a time
    /// that runs to the block's end refuses the length there, in one call
    /// or in steps, and never hands the group over as whole.
    #[test]
    fn a_part_at_a_time_decode_refuses_a_payload_that_decodes_past_its_groups_length() {
        let images: Vec<Vec<u8>> = (0..40)
            .map(|i| one_value(format!("{i:016}").as_bytes(), b"foxfoxfo"))
            .collect();
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(_) = fill_page(&images, 0, 3, &mut s, &mut page) else {
            panic!("a page")
        };
        let v = parse_page(&page, 3).unwrap();
        let ge = v.group(0).unwrap();
        assert!(!ge.stored && !ge.coded, "a block-coded group: {ge:?}");
        let mut whole = GroupBuf::new();
        let n = decode_group_with(&v, 0, &mut whole, true).unwrap().len();
        let cut = 4usize;
        assert!(cut < super::super::zlz4::DECODE_SLACK);
        let mut bad = page;
        let at = ZHDR + D_ULEN4;
        bad[at..at + 2].copy_from_slice(&(((n - cut) / 4) as u16).to_le_bytes());
        let sum = u32::from_le_bytes(bad[P_ULEN..P_ULEN + 4].try_into().unwrap());
        bad[P_ULEN..P_ULEN + 4].copy_from_slice(&(sum - cut as u32).to_le_bytes());
        let bv = page_view(&bad).expect("a view of the forged page");
        assert_eq!(bv.group(0).unwrap().ulen as usize, n - cut);
        let mut part = GroupBuf::new();
        assert!(
            open_group_partly(&bv, 0, &mut part, usize::MAX).is_err(),
            "the whole of it in one call"
        );
        let mut p = open_group_partly(&bv, 0, &mut part, 0)
            .expect("a begin short of the end")
            .expect("a block-coded group");
        let refused = loop {
            assert!(!p.done(), "never whole");
            let before = p.have();
            match decode_group_more(&mut p, &mut part, before + 16) {
                Err(_) => break true,
                Ok(got) => assert!(got > before && got <= n - cut),
            }
        };
        assert!(refused);
    }

    // ---- a whole form file ------------------------------------------------

    #[test]
    fn a_whole_form_file_round_trips_through_its_file() {
        use super::super::{BlockedBloom, BLOOM_BITS_PER_KEY, BLOOM_PROBES, BLOOM_SIG};
        let mut rng = Rng::new(71);
        // Records of every shape, and three whose keys are too big for a page
        // to hold once compressed.
        let mut keys = std::collections::BTreeSet::new();
        while keys.len() < 1500 {
            let l = rng.below(40);
            keys.insert(rng.bytes(l));
        }
        for _ in 0..3 {
            keys.insert(rng.bytes(5000));
        }
        let recs: Vec<(Vec<u8>, Vec<u8>)> = keys
            .into_iter()
            .map(|k| {
                let im = random_image(&mut rng, &k);
                (k, im)
            })
            .collect();
        let images: Vec<Vec<u8>> = recs.iter().map(|(_, i)| i.clone()).collect();
        let n = recs.len();
        let bind = bind32(5, 6, n as u64, 7);

        // The pages, as a builder writes them, and the fence entries they make:
        // one for each page, and one for each run of records no page holds.
        let mut s = FillScratch::new();
        let mut rank = 0usize;
        let (mut file, mut fence) = (Vec::new(), ZFence::new());
        let (mut pages, mut total_ulen) = (0u32, 0u64);
        let mut run: Option<usize> = None;
        while rank < n {
            let mut page = [0u8; ZPAGE];
            match fill_page(&images, rank, bind, &mut s, &mut page) {
                Fill::Page(g) => {
                    if let Some(at) = run.take() {
                        fence.push(at as u32, ZNOPAGE, &recs[at].0).unwrap();
                    }
                    fence.push(rank as u32, pages, &recs[rank].0).unwrap();
                    file.extend_from_slice(&page);
                    pages += 1;
                    total_ulen += g.ulen as u64;
                    rank += g.count;
                }
                Fill::NoPage(_) => {
                    run.get_or_insert(rank);
                    rank += 1;
                }
                other => panic!("rank {rank}: {other:?}"),
            }
        }
        if let Some(at) = run.take() {
            fence.push(at as u32, ZNOPAGE, &recs[at].0).unwrap();
        }
        let mut bloom = BlockedBloom::with_capacity(n);
        for (k, _) in &recs {
            bloom.insert(k);
        }
        let filter = bloom.words();
        let (res, f) = fence.encode_resident(&filter).unwrap();
        let t = Trailer {
            n_pages: u64::from(pages),
            n_entries: fence.len() as u64,
            nkeys: n as u64,
            res_off: file.len() as u64,
            res_len: f.res_len,
            keys_len: f.keys_len,
            filter_off: f.filter_off,
            filter_blocks: f.filter_blocks,
            res_crc: f.res_crc,
            filter: true,
            total_ulen,
            seg_id: 5,
            seg_end_seq: 6,
            seg_row_crc: 7,
            bloom_probes: BLOOM_PROBES,
            bloom_sig: BLOOM_SIG,
            bloom_bits_per_key: BLOOM_BITS_PER_KEY,
            ..sample_trailer()
        };
        assert_eq!(t.bind32(), bind);
        file.extend_from_slice(&res);
        file.extend_from_slice(&t.encode());

        // The file, read the way an attach and a read will.
        assert!(file.len().is_multiple_of(ZPAGE));
        let t = Trailer::decode(&file[file.len() - ZTRAILER..]).expect("the trailer");
        assert_eq!(t.expected_file_len(), Some(file.len() as u64));
        let (fence, filter) =
            ZFence::decode_resident(&file[t.res_off as usize..], &t).expect("the resident part");
        assert!(t.filter_matches_engine());
        let bloom = BlockedBloom::from_words(filter).expect("whole blocks");
        for (k, _) in &recs {
            assert!(bloom.maybe_contains(k), "the stored filter holds every key");
        }
        // Every entry that has a page passes a reader's first decode.
        let mut buf = GroupBuf::new();
        let (mut paged, mut raw) = (0usize, 0usize);
        for i in 0..fence.len() {
            let e = fence.entry(i, n as u32).unwrap();
            if e.page == ZNOPAGE {
                raw += e.count as usize;
                continue;
            }
            let at = e.page as usize * ZPAGE;
            let h = verify_page(&file[at..at + ZPAGE], t.bind32(), &e, &mut buf).expect("a page");
            paged += usize::from(h.count);
        }
        assert_eq!(paged + raw, n);
        assert!(
            (3..30).contains(&raw),
            "{raw} records are read from the raw segment"
        );
        // Every key is found through the fence and the group's walk, at its
        // rank; and a key between two is not.
        let entry_of = |key: &[u8]| {
            let (mut lo, mut hi) = (0usize, fence.len());
            while lo < hi {
                let mid = (lo + hi) / 2;
                if fence.key(mid).unwrap() <= key {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo.checked_sub(1)
        };
        let mut raw_seen = 0;
        let mut part_buf = GroupBuf::new();
        let (mut part_seeks, mut part_bytes, mut part_of, mut part_short) =
            (0usize, 0usize, 0usize, 0usize);
        for (rank, (k, _)) in recs.iter().enumerate() {
            for (probe, want_found) in [(k.clone(), true), ([k.as_slice(), &[0u8]].concat(), false)]
            {
                let Some(i) = entry_of(&probe) else {
                    panic!("every probe is at or above the first key")
                };
                let e = fence.entry(i, n as u32).unwrap();
                if e.page == ZNOPAGE {
                    raw_seen += 1;
                    continue;
                }
                let at = e.page as usize * ZPAGE;
                let v = parse_page(&file[at..at + ZPAGE], t.bind32()).unwrap();
                let j = v.group_of_key(&probe);
                let ge = v.group(j).unwrap();
                let g = decode_group(&v, j, &mut buf).unwrap();
                let p = group_seek(g, usize::from(ge.count), &probe).unwrap();
                // The same seek with the group decoded only as far as the
                // walk reads (`seek_group_partly`): the same record at the
                // same offset, whole in what was decoded and byte for byte
                // the whole decode's; a stored group is not its to serve.
                match seek_group_partly(&v, j, &mut part_buf, &probe).unwrap() {
                    Some(s) => {
                        assert!(!ge.stored && !ge.coded, "rank {rank}");
                        assert_eq!(s.of, ge.ulen as usize, "rank {rank}");
                        assert_eq!(s.off, p.found.then_some(p.off), "rank {rank}");
                        assert!(s.len <= s.of, "rank {rank}");
                        assert_eq!(
                            part_buf.bytes(s.len),
                            &g[..s.len],
                            "rank {rank}: the prefix"
                        );
                        if let Some(off) = s.off {
                            let end = record_key_next(g, off).unwrap().1;
                            assert!(end <= s.len, "rank {rank}: the record whole");
                        }
                        part_seeks += 1;
                        part_bytes += s.len;
                        part_of += s.of;
                        part_short += usize::from(s.len < s.of);
                    }
                    None => assert!(ge.stored || ge.coded, "rank {rank}"),
                }
                let at_rank = e.first_rank as usize + usize::from(ge.first) + p.index;
                if want_found {
                    assert!(p.found && at_rank == rank, "rank {rank}");
                } else {
                    // The next key may be the probe itself only by chance of the draw.
                    assert_eq!(
                        p.found,
                        recs.get(rank + 1).is_some_and(|(n, _)| *n == probe)
                    );
                    assert_eq!(at_rank, rank + 1);
                }
            }
        }
        assert!(
            raw_seen >= raw,
            "the raw records were reached through NOPAGE entries"
        );
        // These records are mostly random bytes, so few of their groups
        // are blocks; the ones that are were each held to the whole walk
        // above, and the suite's shape has a test of its own.
        assert!(part_bytes <= part_of && part_short <= part_seeks);
        // A group the page does not have is refused.
        let e = (0..fence.len())
            .map(|i| fence.entry(i, n as u32).unwrap())
            .find(|e| e.page != ZNOPAGE)
            .unwrap();
        let at = e.page as usize * ZPAGE;
        let v = parse_page(&file[at..at + ZPAGE], t.bind32()).unwrap();
        assert!(seek_group_partly(&v, v.groups(), &mut part_buf, b"k").is_err());
    }

    // ---- pins -----------------------------------------------------------

    /// A filter over 4,096 fixed keys, its words as bytes.
    fn pinned_filter() -> (usize, Vec<u8>) {
        use super::super::BlockedBloom;
        let mut f = BlockedBloom::with_capacity(4096);
        let keys: Vec<Vec<u8>> = (0..4096u32)
            .map(|i| format!("{i:016}").into_bytes())
            .collect();
        for k in &keys {
            f.insert(k);
        }
        for k in &keys {
            assert!(f.maybe_contains(k), "a filter has no false negatives");
        }
        (f.blocks.len(), f.words())
    }

    #[test]
    fn the_bloom_layout_is_pinned() {
        let (blocks, words) = pinned_filter();
        assert_eq!(
            (super::super::BLOOM_SIG, blocks, words.len(), crc32(&words)),
            (1, 80, 5120, 0x2EA5_76A7),
            "BlockedBloom's bits are not the ones this pin was taken from: a form file \
             built before the change would attach with every other check passing and \
             answer \"absent\" for keys it holds. Bump BLOOM_SIG (db.rs) and ZVERSION, \
             then take the new CRC of the words from this message"
        );
    }

    #[test]
    fn the_blooms_parameters_are_the_structures() {
        use super::super::{BlockedBloom, BLOOM_BITS_PER_KEY, BLOOM_PROBES};
        let f = BlockedBloom::with_capacity(512);
        assert_eq!(f.blocks.len(), 512 * BLOOM_BITS_PER_KEY as usize / 512);
        assert_eq!(f.slots(b"any key").1.len(), BLOOM_PROBES as usize);
    }

    #[test]
    fn the_filters_size_is_the_one_the_filter_allocates() {
        use super::super::BlockedBloom;
        // `filter_bytes` is what a tool asks for the stored filter's size: it
        // is the filter's own allocation, at the counts a block boundary
        // falls between, and for none.
        for n in [0usize, 1, 50, 51, 52, 511, 512, 513, 4096, 100_000] {
            let f = BlockedBloom::with_capacity(n);
            assert_eq!(filter_bytes(n), f.blocks.len() * 64, "{n} keys");
            assert!(filter_bytes(n).is_multiple_of(64) && filter_bytes(n) >= 64);
        }
        // The suite's ten million keys: ten bits a key, in whole blocks.
        assert_eq!(filter_bytes(10_000_000), 195_313 * 64);
    }

    #[test]
    fn the_lz4_codec_is_the_one_the_sizes_were_measured_with() {
        // The decoder the engine's manifest asks for. A build that links the
        // other -- a dependency that asks for the crate's defaults -- fails
        // here, and its timings of a decode are not the engine's.
        assert_eq!(
            lz4_decoder(),
            Lz4Decoder::Unsafe,
            "lz4_flex is not built with default-features off: some dependency of this build \
             turned on safe-decode"
        );
        // The encoder: the bytes it makes of a fixed input are what the
        // pages' sizes, and so every records-a-page figure, depend on. A new
        // version of the crate that encodes differently moves them, and the
        // sizing has to be taken again before the new bytes are believed.
        // The pin is a computation the sizing tool runs in its own build as
        // well (`lz4_encoder_pin`), over images a test can look at.
        let input = suite_images(30, 5).concat();
        assert_eq!(
            encoder_pin_input(),
            input,
            "the pin's input is the suite's records"
        );
        let c = lz4_flex::block::compress(&input);
        assert_eq!((input.len(), c.len(), crc32(&c)), LZ4_ENCODER_PIN);
        assert_eq!(
            lz4_encoder_pin(),
            LZ4_ENCODER_PIN,
            "lz4_flex encodes this input differently: re-run bench/examples/zsize.rs and \
             take the new pin from this message"
        );
        // compress_into a buffer for the largest group is the same encoder.
        let mut big = vec![0u8; lz4_flex::block::get_maximum_output_size(ZMAX_ULEN)];
        let n = lz4_flex::block::compress_into(&input, &mut big).unwrap();
        assert_eq!(&big[..n], &c[..]);
        // The decoder on a block written by hand: three literals, a match of
        // eight bytes at an offset of three, one last literal.
        let block = [0x34u8, b'a', b'b', b'c', 0x03, 0x00, 0x10, b'Z'];
        let mut out = [0u8; 12];
        assert_eq!(
            lz4_flex::block::decompress_into(&block, &mut out).unwrap(),
            12
        );
        assert_eq!(&out, b"abcabcabcabZ");
        // And it stops where its input does, not where its output does: the
        // page carries clen for this.
        let mut zeros = block.to_vec();
        zeros.extend_from_slice(&[0, 0, 0, 0]);
        let mut out = [0u8; 12];
        let r = lz4_flex::block::decompress_into(&zeros, &mut out);
        assert!(
            r.is_err() || r.unwrap() != 12,
            "trailing zeros are further sequences"
        );
    }

    // ---- images out of real segments ------------------------------------

    fn scratch(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("supdb-next-zpages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The records of the segment at `path` as images, checked against the
    /// writer's own encoder: for every rank, the bytes the directory points
    /// at are the bytes `stream_record` makes of the record `exts_at_full`
    /// reads back. Returns how many records, how many of them are compact,
    /// and how many bytes of the record region lie between records.
    fn check_segment_images(path: &std::path::Path, compact: bool) -> (usize, usize, usize) {
        use crate::bytes::Bytes;
        let src = crate::MmapBytes::open(path).unwrap();
        let blob = crate::Blob::open(crate::MmapBytes::open(path).unwrap()).unwrap();
        let off = blob.index_offset();
        let sec = src.slice_at(off, (src.len() - off) as usize).unwrap();
        let idx = crate::flatindex::FlatIndex::parse(sec).expect("a flat index");
        let (recs, dir) = idx.regions(sec).unwrap();
        let n = blob.keys();
        let images = SegImages::new(recs, dir, n);
        assert_eq!(images.records(), n);
        let (mut compacts, mut gap, mut end) = (0usize, 0usize, None::<usize>);
        for r in 0..n {
            let stored = images.image(r);
            let (key, exts, tail) = blob.exts_at_full(r).expect("a record reads back");
            let mut made = Vec::new();
            crate::flatindex::stream_record_as(&mut made, key, &exts, tail, compact)
                .expect("a record the engine read is one it can write");
            assert_eq!(
                stored,
                &made[..],
                "rank {r}: the stored record is the streamed one"
            );
            compacts += usize::from(stored[3] & 0x80 != 0);
            let at = crate::flatindex::rd_u32(dir, 4 * r).unwrap() as usize;
            if let Some(e) = end {
                gap += at.saturating_sub(e);
                assert!(at >= e, "records ascend with rank in a segment");
            }
            end = Some(at + stored.len());
        }
        (n, compacts, gap)
    }

    fn fill_segment(path: &std::path::Path) -> (usize, usize) {
        let src = crate::MmapBytes::open(path).unwrap();
        let blob = crate::Blob::open(crate::MmapBytes::open(path).unwrap()).unwrap();
        use crate::bytes::Bytes;
        let off = blob.index_offset();
        let sec = src.slice_at(off, (src.len() - off) as usize).unwrap();
        let idx = crate::flatindex::FlatIndex::parse(sec).unwrap();
        let (recs, dir) = idx.regions(sec).unwrap();
        let images = SegImages::new(recs, dir, blob.keys());
        let mut s = FillScratch::new();
        let b = build_all(&images, &mut s, 77, None);
        check_build(&images, &b, 77, true);
        (b.pages.len(), b.declined.len())
    }

    #[test]
    fn a_segments_images_are_the_streamed_records() {
        let dir = scratch("seg-images");
        let mut rng = Rng::new(61);
        let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
        // Every shape: the suite's one fixed-width value, several of one
        // width, several of many, a run too long to be inline, and a value
        // of no bytes.
        let items: Vec<(Vec<u8>, Vec<Vec<u8>>)> = (0..3000usize)
            .map(|i| {
                let key = format!("key-{i:08}").into_bytes();
                let mut value = |n: usize| -> Vec<u8> {
                    let ph = rng.below(motif.len());
                    let mut v: Vec<u8> =
                        (0..n / 2).map(|j| motif[(ph + j) % motif.len()]).collect();
                    v.extend(rng.bytes(n - n / 2));
                    v
                };
                let vals = match i {
                    _ if i % 101 == 100 => vec![Vec::new()],
                    _ if i % 97 == 96 => vec![value(1000)],
                    _ if i % 11 == 10 => (0..3).map(|_| value(40)).collect(),
                    _ if i % 7 == 6 => (0..2 + i % 4).map(|k| value(10 + 13 * k)).collect(),
                    _ => vec![value(100)],
                };
                (key, vals)
            })
            .collect();
        let refs: Vec<(&[u8], Vec<&[u8]>)> = items
            .iter()
            .map(|(k, v)| (k.as_slice(), v.iter().map(|x| x.as_slice()).collect()))
            .collect();
        let items_ref: Vec<(&[u8], &[&[u8]])> =
            refs.iter().map(|(k, v)| (*k, v.as_slice())).collect();
        for compact in [true, false] {
            let path = dir.join(format!("seg-{compact}.sup"));
            let opts = crate::SegmentOptions {
                compact_records: compact,
                ..crate::SegmentOptions::default()
            };
            let write = crate::SegmentWrite {
                inline_max: 256,
                compress: false,
                sync_every: 0,
            };
            crate::SegmentWriter::write_sorted(&path, &opts, &write, 1, &items_ref).unwrap();
            let (n, compacts, gap) = check_segment_images(&path, compact);
            assert_eq!(n, items.len());
            assert_eq!(
                gap, 0,
                "a segment from the writer has no bytes between its records"
            );
            if compact {
                assert!(compacts > n / 2, "{compacts} of {n} compact");
            } else {
                assert_eq!(compacts, 0);
            }
            let (pages, declined) = fill_segment(&path);
            assert!(
                pages > 10 && declined == 0,
                "{pages} pages, {declined} declined"
            );
            // The suite's record is 124 bytes, and the images say so.
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the stride form ------------------------------------------------

    /// Every kind at every width it holds, each over no prefix, a byte of
    /// one, and the longest prefix the key's length leaves room for.
    fn every_codec_shape() -> Vec<KeyCodec> {
        let mut v = Vec::new();
        for kind in KEY_KINDS {
            for d in 1..=kind.d_max() {
                let long = vec![b'p'; STRIDE_KEY_MAX - d];
                for prefix in [&b""[..], b"k", &long] {
                    v.push(KeyCodec::new(kind, prefix, d, 0).expect("a codec of a width it holds"));
                }
            }
        }
        v
    }

    /// Numbers across a field: its ends and their neighbours, the carries
    /// of its first bytes, and a scatter, ascending and distinct.
    fn numbers_of(c: &KeyCodec, rng: &mut Rng) -> Vec<u64> {
        let top = c.top();
        let radix = c.kind().radix();
        let mut v = vec![0, 1, 2, top / 2, top - 1, top];
        let mut w = 1u64;
        for _ in 0..c.d() {
            v.extend([w.saturating_sub(1), w, w.saturating_add(1)]);
            w = w.saturating_mul(radix);
        }
        v.extend((0..64).map(|_| rng.word() % top.saturating_add(1).max(1)));
        v.retain(|&x| x <= top);
        v.sort_unstable();
        v.dedup();
        v
    }

    /// `render_field` writes the bytes `render` writes after the prefix
    /// (`Options::zpages_stride_step_direct`): for every kind at every
    /// width, over the numbers across its field, written over the field of
    /// another number's key with the prefix standing. And the digits by the
    /// word are the digits by the digit: every number of eight decimal
    /// digits, since the three multiplies share a word and a carry between
    /// its lanes would show for some pair of halves and no single one;
    /// sixteen digits either side of each power of ten and a scatter; and
    /// hex with every nibble at every place and a scatter, in both cases.
    #[test]
    fn a_field_rendered_by_the_word_is_the_field_rendered_by_the_digit() {
        let mut rng = Rng::new(0x51de);
        let (mut buf, mut other) = ([0u8; STRIDE_KEY_MAX], [0u8; STRIDE_KEY_MAX]);
        for c in every_codec_shape() {
            let xs = numbers_of(&c, &mut rng);
            for (i, &x) in xs.iter().enumerate() {
                let want = c.render(x, &mut buf).to_vec();
                // Over the key of another number of the run, so a byte the
                // field's writer left alone is a byte of the wrong key.
                let from = xs[(i + xs.len() / 2 + 1) % xs.len()];
                let mut key = c.render(from, &mut other).to_vec();
                c.render_field(x, &mut key[c.p()..]);
                assert_eq!(key, want, "{c:?} {x} over {from}");
                assert_eq!(c.p(), c.prefix().len(), "{c:?}");
            }
        }
        // Every eight digits, against a counter stepped in place.
        let mut digits = *b"00000000";
        for v in 0..100_000_000u32 {
            assert_eq!(dec8(v).to_be_bytes(), digits, "{v}");
            for b in digits.iter_mut().rev() {
                if *b == b'9' {
                    *b = b'0';
                } else {
                    *b += 1;
                    break;
                }
            }
        }
        let by_digit = |x: u64| -> [u8; 16] {
            let mut out = [b'0'; 16];
            let mut v = x;
            for b in out.iter_mut().rev() {
                *b = b'0' + (v % 10) as u8;
                v /= 10;
            }
            out
        };
        let top = 10_000_000_000_000_000u64;
        let mut xs = vec![0, 1, top - 1, top / 2];
        let mut w = 1u64;
        while w < top {
            xs.extend([w - 1, w, w + 1, w * 9, w * 9 + 8]);
            w *= 10;
        }
        xs.extend((0..100_000).map(|_| rng.word() % top));
        for x in xs.into_iter().filter(|&x| x < top) {
            assert_eq!(digits16(dec16_words(x)), by_digit(x), "{x}");
        }
        let hex_by_digit = |x: u64, a: u8| -> [u8; 16] {
            let mut out = [b'0'; 16];
            let mut v = x;
            for b in out.iter_mut().rev() {
                let d = (v & 15) as u8;
                *b = if d < 10 { b'0' + d } else { a + d - 10 };
                v >>= 4;
            }
            out
        };
        let mut xs = vec![0, u64::MAX, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];
        for place in 0..16 {
            for d in 0..16u64 {
                // The nibble among zeros and among fifteens.
                xs.push(d << (4 * place));
                xs.push(!(15u64 << (4 * place)) | (d << (4 * place)));
            }
        }
        xs.extend((0..100_000).map(|_| rng.word()));
        for x in xs {
            for a in *b"aA" {
                assert_eq!(
                    digits16(hex16_words(x, a)),
                    hex_by_digit(x, a),
                    "{x:#x} {}",
                    a as char
                );
            }
        }
    }

    #[test]
    fn a_key_codec_renders_decodes_and_steps_in_key_order() {
        let mut rng = Rng::new(43);
        let mut buf = [0u8; STRIDE_KEY_MAX];
        let mut next = [0u8; STRIDE_KEY_MAX];
        for c in every_codec_shape() {
            let xs = numbers_of(&c, &mut rng);
            let mut before: Option<Vec<u8>> = None;
            for &x in &xs {
                let key = c.render(x, &mut buf).to_vec();
                assert_eq!(key.len(), c.key_len(), "{c:?} {x}");
                assert_eq!(&key[..c.prefix().len()], c.prefix());
                assert_eq!(c.decode(&key), Some(x), "{c:?} {x}");
                // Strictly increasing under the engine's key order.
                if let Some(b) = &before {
                    assert_eq!(key_cmp(b, &key), Ordering::Less, "{c:?} at {x}");
                }
                before = Some(key.clone());
                // A step is the next number's rendering, until the top,
                // which wraps and says so.
                let mut stepped = key.clone();
                let fit = c.increment(&mut stepped);
                if x < c.top() {
                    assert!(fit, "{c:?} {x}");
                    assert_eq!(stepped, c.render(x + 1, &mut next), "{c:?} {x}");
                } else {
                    assert!(!fit, "{c:?} top");
                    assert_eq!(stepped, c.render(0, &mut next), "{c:?} wraps");
                }
                // Refusals: the wrong length, either way.
                assert_eq!(c.decode(&key[..key.len() - 1]), None, "{c:?} {x} short");
                for b in [0u8, b'0', 0xff] {
                    let mut long = key.clone();
                    long.push(b);
                    assert_eq!(c.decode(&long), None, "{c:?} {x} long");
                }
                // A prefix byte other than the prefix's.
                for i in 0..c.prefix().len() {
                    let mut k = key.clone();
                    k[i] ^= 1;
                    assert_eq!(c.decode(&k), None, "{c:?} {x} prefix byte {i}");
                }
                // A byte at each place of the field that is none of the
                // kind's: either side of the digits and of each case's
                // letters, the other case's letters, the ends of a byte.
                for i in c.prefix().len()..key.len() {
                    for b in EDGE_BYTES {
                        if c.kind().rank_byte(b).1 {
                            continue;
                        }
                        let mut k = key.clone();
                        k[i] = b;
                        assert_eq!(c.decode(&k), None, "{c:?} {x} field byte {i} = {b}");
                    }
                }
            }
            // The empty key is no codec's, and a key of 0xff bytes is the
            // top of a big-endian field with no prefix and nobody else's.
            assert_eq!(c.decode(b""), None);
            for len in 1..=STRIDE_KEY_MAX + 1 {
                let ff = vec![0xffu8; len];
                let want =
                    (c.kind() == KeyKind::BigEndian && c.prefix().is_empty() && len == c.d())
                        .then_some(c.top());
                assert_eq!(c.decode(&ff), want, "{c:?} {len} bytes of 0xff");
            }
        }
        // The decoder takes exactly the keys the codec renders, and
        // rendering gives each back byte for byte: every field of one and
        // of two bytes, of each kind, behind a prefix. That is what leaves
        // the build walk's second quantity, a key the number renders
        // (`zfile::stride_key_is`), a reason no key earns under this
        // decoder.
        for kind in KEY_KINDS {
            for d in 1..=2usize {
                let c = KeyCodec::new(kind, b"k", d, 0).unwrap();
                let mut taken = 0u64;
                for f in 0..1u64 << (8 * d) {
                    let key = [b"k".as_slice(), &f.to_be_bytes()[8 - d..]].concat();
                    if let Some(x) = c.decode(&key) {
                        taken += 1;
                        assert_eq!(c.render(x, &mut buf), &key[..], "{c:?} {key:?}");
                    }
                }
                assert_eq!(taken, c.top() + 1, "{c:?}: every number's key, once");
            }
        }
        // The widest decimal field holds every nineteen-digit number and
        // nothing of twenty.
        let c = KeyCodec::new(KeyKind::Decimal, b"", STRIDE_DECIMAL_MAX, 0).unwrap();
        let nines = vec![b'9'; STRIDE_DECIMAL_MAX];
        assert_eq!(c.decode(&nines), Some(c.top()));
        assert_eq!(c.render(c.top(), &mut buf), &nines[..]);
        assert_eq!(c.decode(&[b'9'; STRIDE_DECIMAL_MAX + 1]), None);
        assert_eq!(c.decode(&[b'1'; STRIDE_DECIMAL_MAX + 1]), None);
        assert_eq!(
            u128::from(c.top()) + 1,
            10u128.pow(STRIDE_DECIMAL_MAX as u32)
        );
        let c = KeyCodec::new(KeyKind::BigEndian, b"", STRIDE_BE_MAX, 0).unwrap();
        assert_eq!(c.top(), u64::MAX);
        // The widest hex field is a u64 in sixteen digits of its case, and
        // each case refuses the other's letters.
        for (kind, top, other) in [
            (KeyKind::HexLower, b"ffffffffffffffff", b"FFFFFFFFFFFFFFFF"),
            (KeyKind::HexUpper, b"FFFFFFFFFFFFFFFF", b"ffffffffffffffff"),
        ] {
            let c = KeyCodec::new(kind, b"", STRIDE_HEX_MAX, 0).unwrap();
            assert_eq!(c.top(), u64::MAX);
            assert_eq!(c.decode(top), Some(u64::MAX));
            assert_eq!(c.render(u64::MAX, &mut buf), &top[..]);
            assert_eq!(c.decode(other), None);
            assert_eq!(
                c.decode(b"0123456789abcdef").is_some(),
                kind == KeyKind::HexLower
            );
            assert_eq!(
                c.decode(b"0123456789ABCDEF").is_some(),
                kind == KeyKind::HexUpper
            );
            assert_eq!(
                KeyCodec::new(kind, b"", 3, 0).unwrap().top(),
                0xfff,
                "{kind:?}: three digits"
            );
        }
        // The constructor refuses what it cannot render.
        for kind in KEY_KINDS {
            let width = "the field's width is not one to the most its kind holds";
            assert_eq!(KeyCodec::new(kind, b"", 0, 0).err(), Some(width));
            assert_eq!(
                KeyCodec::new(kind, b"", kind.d_max() + 1, 0).err(),
                Some(width)
            );
            let long = vec![b'p'; STRIDE_KEY_MAX];
            assert_eq!(
                KeyCodec::new(kind, &long, 1, 0).err(),
                Some("the key is longer than a stride form takes")
            );
            assert!(KeyCodec::new(kind, &long[1..], 1, 0).is_ok());
            let c = KeyCodec::new(kind, b"k", 2, 0).unwrap();
            assert!(KeyCodec::new(kind, b"k", 2, c.top()).is_ok());
            assert_eq!(
                KeyCodec::new(kind, b"k", 2, c.top() + 1).err(),
                Some("the first key's number is past what the field holds")
            );
        }
        // A run held whole is one of at least a key and not past the top.
        let c = KeyCodec::new(KeyKind::Decimal, b"", 2, 90).unwrap();
        assert!(!c.holds(0));
        assert!(c.holds(1) && c.holds(10));
        assert!(!c.holds(11));
        assert!(!c.holds(u64::MAX));
        let c = KeyCodec::new(KeyKind::BigEndian, b"", STRIDE_BE_MAX, u64::MAX).unwrap();
        assert!(c.holds(1) && !c.holds(2));
    }

    /// Bytes at the edges of every kind's alphabet: either side of the
    /// digits and of each case's six letters, the letters' ends, and the
    /// ends of a byte.
    const EDGE_BYTES: [u8; 14] = [
        0,
        b'0' - 1,
        b'0',
        b'9',
        b'9' + 1,
        b'A' - 1,
        b'A',
        b'F',
        b'F' + 1,
        b'a' - 1,
        b'a',
        b'f',
        b'f' + 1,
        0xff,
    ];

    /// The rank of the first rendered key at or after `q`, and whether it is
    /// `q`, by a binary search under the engine's key order: what
    /// `lower_bound` computes, written the other way.
    fn search(keys: &[Vec<u8>], q: &[u8]) -> (u64, Ordering) {
        let r = keys.partition_point(|k| key_cmp(k, q) == Ordering::Less);
        let at = if keys.get(r).is_some_and(|k| k == q) {
            Ordering::Equal
        } else {
            Ordering::Greater
        };
        (r as u64, at)
    }

    #[test]
    fn lower_bound_is_the_binary_search_of_the_rendered_keys() {
        let dec = KeyKind::Decimal;
        let be = KeyKind::BigEndian;
        let (hexl, hexu) = (KeyKind::HexLower, KeyKind::HexUpper);
        let top = |kind, d| KeyCodec::new(kind, b"", d, 0).unwrap().top();
        let runs: Vec<(KeyKind, &[u8], usize, u64, u64)> = vec![
            // A run of one; a run over a carry of the field's first digit.
            (dec, b"", 3, 0, 1),
            (dec, b"k", 4, 990, 37),
            // The suite's: a long prefix of zeros, a field of the rest.
            (dec, b"0000000000", 6, 0, 2000),
            // A run to the field's top, and one over a whole field.
            (
                dec,
                b"",
                STRIDE_DECIMAL_MAX,
                top(dec, STRIDE_DECIMAL_MAX) - 40,
                41,
            ),
            (dec, b"", 1, 0, 10),
            (be, b"", 1, 0, 256),
            (be, b"\x00\x01", 2, 250, 600),
            (be, b"\xff", 1, 3, 200),
            (be, b"", STRIDE_BE_MAX, u64::MAX - 30, 31),
            // Prefixed runs that end at their field's top, where a key
            // above the prefix is past the run by exactly every number
            // the field holds and the clamp to `n` hides nothing: a run
            // whose last key is the field's last, as "k0990" to "k0999"
            // makes, and as the last partition of a store of sixteen-digit
            // keys whose prefix holds a digit makes. A run that ends short
            // of the top answers `n` for any count at or past its end.
            (dec, b"k", 1, 0, 10),
            (dec, b"k099", 1, 0, 10),
            (dec, b"0000000009", 6, 999_000, 1000),
            (be, b"\x80", 1, 3, 253),
            (be, b"\x00\x01", 2, 65_000, 536),
            // Hex runs of each case: over the step from the digits to the
            // letters, over a carry, to a field's top, behind a prefix of
            // letters of the other case, and the widest field's last keys.
            (hexl, b"", 2, 0, 256),
            (hexl, b"k", 3, 0x9f8, 40),
            (hexu, b"", 2, 0x08, 0x20),
            (hexu, b"abc", 4, 0xff00, 256),
            (hexl, b"ABC", 1, 0, 16),
            (hexl, b"", STRIDE_HEX_MAX, u64::MAX - 30, 31),
        ];
        let mut buf = [0u8; STRIDE_KEY_MAX];
        for &(kind, prefix, d, base, n) in &runs {
            let c = KeyCodec::new(kind, prefix, d, base).unwrap();
            assert!(c.holds(n), "{c:?} {n}");
            let keys: Vec<Vec<u8>> = (0..n)
                .map(|r| c.render(base + r, &mut buf).to_vec())
                .collect();
            let mut qs: Vec<Vec<u8>> = vec![
                Vec::new(),
                prefix.to_vec(),
                prefix[..prefix.len().saturating_sub(1)].to_vec(),
            ];
            for len in 1..=c.key_len() + 1 {
                for b in EDGE_BYTES {
                    qs.push(vec![b; len]);
                }
            }
            for i in 0..prefix.len() {
                for delta in [1u8, 0xff] {
                    let mut q = keys[0].clone();
                    q[i] = q[i].wrapping_add(delta);
                    qs.push(q);
                }
            }
            // The run's keys with their neighbours inside the field, each
            // as it is, a byte short, a byte long, and with each place
            // turned to a byte on either side of the digits and to the
            // ends of a byte.
            let lo = base.saturating_sub(3);
            let hi = (base + (n - 1)).saturating_add(3).min(c.top());
            for x in lo..=hi {
                let k = c.render(x, &mut buf).to_vec();
                qs.push(k[..k.len() - 1].to_vec());
                for b in EDGE_BYTES {
                    let mut q = k.clone();
                    q.push(b);
                    qs.push(q);
                }
                for i in 0..k.len() {
                    for b in EDGE_BYTES {
                        let mut q = k.clone();
                        q[i] = b;
                        qs.push(q);
                    }
                }
                qs.push(k);
            }
            for q in &qs {
                assert_eq!(
                    c.lower_bound(q, n),
                    search(&keys, q),
                    "{c:?} n {n}: {q:02x?}"
                );
                // The same place among the field's numbers, which a run
                // with gaps is searched by: for a run with none it is the
                // rank past the base, wherever it falls inside the run.
                let (at, exact) = c.at_or_after(q);
                let (r, order) = search(&keys, q);
                if r > 0 && r < n {
                    assert_eq!(at, u128::from(base + r), "{c:?}: {q:02x?}");
                    assert_eq!(exact, order == Ordering::Equal, "{c:?}: {q:02x?}");
                } else if r == 0 {
                    assert!(at <= u128::from(base), "{c:?}: {q:02x?} below the run");
                } else {
                    assert!(
                        at >= u128::from(base) + u128::from(n),
                        "{c:?}: {q:02x?} past it"
                    );
                }
            }
            // The run's own keys, each found where it is.
            for (r, k) in keys.iter().enumerate() {
                assert_eq!(c.lower_bound(k, n), (r as u64, Ordering::Equal));
            }
        }
    }

    #[test]
    fn detect_takes_exactly_a_run_its_codec_counts() {
        let dec = |s: &str| s.as_bytes().to_vec();
        let mut buf = [0u8; STRIDE_KEY_MAX];
        // A run of one: the prefix cut a byte short of the key, so the
        // field has one, whatever byte it is.
        let c = KeyCodec::detect(b"k0042", b"k0042", 1).unwrap();
        assert_eq!(
            (c.kind(), c.prefix(), c.d(), c.base()),
            (KeyKind::Decimal, &b"k004"[..], 1, 2)
        );
        let c = KeyCodec::detect(b"key\xfe", b"key\xfe", 1).unwrap();
        assert_eq!(
            (c.kind(), c.prefix(), c.d(), c.base()),
            (KeyKind::BigEndian, &b"key"[..], 1, 0xfe)
        );
        // A carry across what the ends share: no prefix at all.
        let c = KeyCodec::detect(b"0999", b"1002", 4).unwrap();
        assert_eq!(
            (c.kind(), c.prefix(), c.d(), c.base()),
            (KeyKind::Decimal, &b""[..], 4, 999)
        );
        for (r, want) in ["0999", "1000", "1001", "1002"].iter().enumerate() {
            assert_eq!(c.render(c.base() + r as u64, &mut buf), want.as_bytes());
        }
        // The suite's keys: sixteen digits, the run's ten leading zeros the
        // prefix.
        let n = 345_000u64;
        let c = KeyCodec::detect(
            &dec(&format!("{:016}", 0)),
            &dec(&format!("{:016}", n - 1)),
            n,
        )
        .unwrap();
        assert_eq!(
            (c.kind(), c.prefix(), c.d(), c.base()),
            (KeyKind::Decimal, &b"0000000000"[..], 6, 0)
        );
        let c = KeyCodec::detect(b"user0000", b"user0999", 1000).unwrap();
        assert_eq!(
            (c.kind(), c.prefix(), c.d()),
            (KeyKind::Decimal, &b"user0"[..], 3)
        );
        // Big-endian fields, and the widest.
        let c = KeyCodec::detect(&[0, 0, 1, 0], &[0, 0, 2, 255], 512).unwrap();
        assert_eq!(
            (c.kind(), c.prefix(), c.d(), c.base()),
            (KeyKind::BigEndian, &[0u8, 0][..], 2, 256)
        );
        let (first, last) = ((1u64 << 56).to_be_bytes(), (2u64 << 56).to_be_bytes());
        let c = KeyCodec::detect(&first, &last, (1 << 56) + 1).unwrap();
        assert_eq!(
            (c.kind(), c.d(), c.base()),
            (KeyKind::BigEndian, STRIDE_BE_MAX, 1 << 56)
        );
        // Digits at both ends that decimal does not count: the bytes of the
        // run between them are big-endian's.
        let span = u64::from(u16::from_be_bytes(*b"20") - u16::from_be_bytes(*b"19")) + 1;
        let c = KeyCodec::detect(b"19", b"20", span).unwrap();
        assert_eq!(c.kind(), KeyKind::BigEndian);
        assert_eq!(
            KeyCodec::detect(b"19", b"20", 2).unwrap().kind(),
            KeyKind::Decimal
        );
        // Holes, a run counted backwards, a run of nothing.
        assert_eq!(
            KeyCodec::detect(b"k0000", b"k0999", 999),
            Err(KeyShapeNo::Holes)
        );
        assert_eq!(
            KeyCodec::detect(b"k0000", b"k0999", 1001),
            Err(KeyShapeNo::Holes)
        );
        assert!(KeyCodec::detect(b"k0000", b"k0999", 1000).is_ok());
        assert_eq!(
            KeyCodec::detect(b"k0999", b"k0000", 1000),
            Err(KeyShapeNo::Holes)
        );
        assert_eq!(
            KeyCodec::detect(b"k0042", b"k0042", 0),
            Err(KeyShapeNo::Holes)
        );
        // Every eight-byte number, from zeros to 0xff bytes, is a run one
        // longer than any count: no `n` is it, and a count that wrapped
        // would take it for a run of none.
        for n in [0, 1, u64::MAX] {
            assert_eq!(
                KeyCodec::detect(&[0; STRIDE_BE_MAX], &[0xff; STRIDE_BE_MAX], n),
                Err(KeyShapeNo::Holes),
                "{n}"
            );
        }
        // The same field from a later first key is a run a count holds,
        // to the field's top.
        let first = 0x0101_0101_0101_0101u64;
        let c = KeyCodec::detect(
            &first.to_be_bytes(),
            &[0xff; STRIDE_BE_MAX],
            u64::MAX - first + 1,
        );
        assert_eq!(
            c.map(|c| (c.kind(), c.d(), c.base())),
            Ok((KeyKind::BigEndian, STRIDE_BE_MAX, first))
        );
        // Two lengths, the empty key, a key past the longest.
        assert_eq!(
            KeyCodec::detect(b"k000", b"k0000", 2),
            Err(KeyShapeNo::Length)
        );
        assert_eq!(KeyCodec::detect(b"", b"", 1), Err(KeyShapeNo::Length));
        let longest = vec![b'p'; STRIDE_KEY_MAX];
        assert!(KeyCodec::detect(&longest, &longest, 1).is_ok());
        let past = vec![b'p'; STRIDE_KEY_MAX + 1];
        assert_eq!(KeyCodec::detect(&past, &past, 1), Err(KeyShapeNo::Length));
        // A field wider than either kind holds: twenty digits, and nine
        // bytes that are not digits.
        let nineteen = (
            dec(&format!("1{}", "0".repeat(18))),
            vec![b'9'; STRIDE_DECIMAL_MAX],
        );
        let c = KeyCodec::detect(&nineteen.0, &nineteen.1, 9 * 10u64.pow(18)).unwrap();
        assert_eq!((c.kind(), c.d()), (KeyKind::Decimal, STRIDE_DECIMAL_MAX));
        let twenty = (
            dec(&format!("1{}", "0".repeat(19))),
            vec![b'9'; STRIDE_DECIMAL_MAX + 1],
        );
        assert_eq!(
            KeyCodec::detect(&twenty.0, &twenty.1, 2),
            Err(KeyShapeNo::Field)
        );
        assert_eq!(
            KeyCodec::detect(&[0; STRIDE_BE_MAX + 1], &[1; STRIDE_BE_MAX + 1], 2),
            Err(KeyShapeNo::Field)
        );
        // Decimal only where both ends are digits: a first key of digits
        // beside a last key of none is no decimal run, and too wide for
        // big-endian.
        assert_eq!(
            KeyCodec::detect(b"123456789", &[0xff; 9], 2),
            Err(KeyShapeNo::Field)
        );
    }

    /// `at_or_after` against the definition, over every number of a small
    /// field of each kind: the least number whose rendering is at or after
    /// the key, one past the field's last where none is, and exact only
    /// for a rendering.
    #[test]
    fn at_or_after_is_the_least_number_rendered_at_or_after_the_key() {
        let mut buf = [0u8; STRIDE_KEY_MAX];
        for kind in KEY_KINDS {
            for (prefix, d) in [(&b""[..], 1usize), (b"k", 2), (b"ab", 1)] {
                let c = KeyCodec::new(kind, prefix, d, 0).unwrap();
                let keys: Vec<Vec<u8>> = (0..=c.top())
                    .map(|x| c.render(x, &mut buf).to_vec())
                    .collect();
                let mut qs: Vec<Vec<u8>> = vec![Vec::new(), prefix.to_vec()];
                for len in 1..=c.key_len() + 1 {
                    for b in EDGE_BYTES {
                        qs.push(vec![b; len]);
                        let mut q = prefix.to_vec();
                        q.resize(len.max(prefix.len()), b);
                        qs.push(q);
                    }
                }
                for k in &keys {
                    qs.push(k.clone());
                    qs.push(k[..k.len() - 1].to_vec());
                    for b in EDGE_BYTES {
                        let mut q = k.clone();
                        q.push(b);
                        qs.push(q);
                        let mut q = k.clone();
                        *q.last_mut().unwrap() = b;
                        qs.push(q);
                    }
                }
                for q in &qs {
                    let (r, order) = search(&keys, q);
                    assert_eq!(
                        c.at_or_after(q),
                        (u128::from(r), order == Ordering::Equal),
                        "{c:?}: {q:02x?}"
                    );
                }
            }
        }
    }

    #[test]
    fn detect_any_takes_a_run_with_gaps_under_the_narrowest_kind_that_holds_it() {
        let all = &KEY_KINDS[..];
        let shape = |r: std::result::Result<(KeyCodec, bool), KeyShapeNo>| {
            r.map(|(c, whole)| (c.kind(), c.prefix().to_vec(), c.d(), c.base(), whole))
        };
        // A whole run under every kind asked is what `detect` finds, and
        // a whole run of hex keys is one where the hex kinds are asked and
        // holes where they are not.
        assert_eq!(
            shape(KeyCodec::detect_any(b"k0000", b"k0999", 1000, all, &[])),
            Ok((KeyKind::Decimal, b"k0".to_vec(), 3, 0, true))
        );
        assert_eq!(
            shape(KeyCodec::detect_any(b"k00f0", b"k010f", 32, all, &[])),
            Ok((KeyKind::HexLower, b"k0".to_vec(), 3, 0x0f0, true))
        );
        assert_eq!(
            shape(KeyCodec::detect_any(b"k00F0", b"k010F", 32, all, &[])),
            Ok((KeyKind::HexUpper, b"k0".to_vec(), 3, 0x0f0, true))
        );
        // Where they are not asked the same two keys are the ends of a run
        // of bytes with gaps, which `detect`, a whole run's, calls holes.
        assert_eq!(
            shape(KeyCodec::detect_any(
                b"k00f0",
                b"k010f",
                32,
                &KEY_KINDS_DENSE,
                &[]
            )),
            Ok((KeyKind::BigEndian, b"k0".to_vec(), 3, 0x30_6630, false))
        );
        assert_eq!(
            KeyCodec::detect(b"k00f0", b"k010f", 32),
            Err(KeyShapeNo::Holes)
        );
        // Fewer keys than numbers between the ends: a run with gaps, under
        // the first kind that holds the ends.
        assert_eq!(
            shape(KeyCodec::detect_any(b"k0000", b"k0999", 999, all, &[])),
            Ok((KeyKind::Decimal, b"k0".to_vec(), 3, 0, false))
        );
        assert_eq!(
            shape(KeyCodec::detect_any(b"1a00", b"ff00", 1000, all, &[])),
            Ok((KeyKind::HexLower, Vec::new(), 4, 0x1a00, false))
        );
        // More keys than numbers is no run of the kind: under decimal the
        // ends hold a thousand numbers, under hex 0x99a, under bytes more
        // still, and each is taken in its turn as the count passes the
        // kind before it.
        for (n, kind) in [
            (1000u64, KeyKind::Decimal),
            (1001, KeyKind::HexLower),
            (0x99a, KeyKind::HexLower),
            (0x99b, KeyKind::BigEndian),
        ] {
            let got = KeyCodec::detect_any(b"k0000", b"k0999", n, all, &[]).unwrap();
            assert_eq!(got.0.kind(), kind, "{n} keys");
        }
        // A kind that counts the run exactly is taken before a narrower
        // one that counts more: "19" to "20" is two decimal numbers, eight
        // hex numbers, and the whole run of the bytes between them.
        let span = u64::from(u16::from_be_bytes(*b"20") - u16::from_be_bytes(*b"19")) + 1;
        assert_eq!(
            shape(KeyCodec::detect_any(b"19", b"20", span, all, &[])),
            Ok((KeyKind::BigEndian, Vec::new(), 2, 0x3139, true))
        );
        assert_eq!(
            shape(KeyCodec::detect_any(b"19", b"20", 8, all, &[])),
            Ok((KeyKind::HexLower, Vec::new(), 2, 0x19, true))
        );
        assert_eq!(
            shape(KeyCodec::detect_any(b"19", b"20", 5, all, &[])),
            Ok((KeyKind::HexLower, Vec::new(), 2, 0x19, false))
        );
        // And whatever the order the kinds are asked in: with the bytes
        // before the digits, the bytes count a run with gaps first and the
        // digits the whole run after, and the whole run is the answer.
        assert_eq!(
            shape(KeyCodec::detect_any(
                b"19",
                b"20",
                2,
                &[KeyKind::BigEndian, KeyKind::Decimal],
                &[]
            )),
            Ok((KeyKind::Decimal, Vec::new(), 2, 19, true))
        );
        // Keys read from inside the run say which kind the ends cannot:
        // ends of digits around a key with a letter are a hex run, of
        // either case by the letter, and around a key with a byte of
        // neither they are a run of bytes or, too wide for one, of no kind.
        let ends = (&b"1000000000"[..], &b"9000000000"[..]);
        let kind_with = |more: &[&[u8]]| {
            KeyCodec::detect_any(ends.0, ends.1, 100, all, more).map(|(c, whole)| (c.kind(), whole))
        };
        assert_eq!(kind_with(&[]), Ok((KeyKind::Decimal, false)));
        assert_eq!(kind_with(&[b"2000000000"]), Ok((KeyKind::Decimal, false)));
        assert_eq!(kind_with(&[b"2000a00000"]), Ok((KeyKind::HexLower, false)));
        assert_eq!(kind_with(&[b"2000A00000"]), Ok((KeyKind::HexUpper, false)));
        assert_eq!(
            kind_with(&[b"2000a00000", b"3000A00000"]),
            Err(KeyShapeNo::Field)
        );
        assert_eq!(kind_with(&[b"2000_00000"]), Err(KeyShapeNo::Field));
        let short = KeyCodec::detect_any(b"10", b"90", 5, all, &[b"2_"]);
        assert_eq!(
            short.map(|(c, whole)| (c.kind(), whole)),
            Ok((KeyKind::BigEndian, false))
        );
        // A key of another length, or outside the prefix the ends share,
        // is no key of the run.
        assert_eq!(
            KeyCodec::detect_any(b"k10", b"k90", 5, all, &[b"k2"]).map(|_| ()),
            Err(KeyShapeNo::Length)
        );
        assert_eq!(
            KeyCodec::detect_any(b"k10", b"k90", 5, all, &[b"j20"]).map(|_| ()),
            Err(KeyShapeNo::Field)
        );
        // A run counted backwards and a run of nothing are holes under
        // every kind; every eight-byte number is a run with gaps for any
        // count, where it was no run at all for a form that held none.
        assert_eq!(
            KeyCodec::detect_any(b"k0999", b"k0000", 10, all, &[]).map(|_| ()),
            Err(KeyShapeNo::Holes)
        );
        assert_eq!(
            shape(KeyCodec::detect_any(b"k0042", b"k0042", 0, all, &[])),
            Ok((KeyKind::Decimal, b"k004".to_vec(), 1, 2, false))
        );
        let whole = KeyCodec::detect_any(&[0; STRIDE_BE_MAX], &[0xff; STRIDE_BE_MAX], 7, all, &[]);
        assert_eq!(
            shape(whole),
            Ok((KeyKind::BigEndian, Vec::new(), STRIDE_BE_MAX, 0, false))
        );
    }

    #[test]
    fn the_sparse_stride_layout_is_its_regions_end_to_end() {
        let page = ZPAGE as u64;
        for n in [
            1u64,
            2,
            63,
            64,
            65,
            511,
            512,
            513,
            345_000,
            10_000_000,
            u64::from(u32::MAX),
        ] {
            for w in [1u32, 99, 100, 4095, STRIDE_W_MAX] {
                let l = stride_layout_sparse(n, w).expect("no overflow inside a trailer's ranges");
                let dense = stride_layout(n, w).unwrap();
                assert_eq!(
                    l.v_pages, dense.v_pages,
                    "{n} {w}: the values are the dense form's"
                );
                assert_eq!(l.k_off(), l.v_pages * page);
                assert_eq!(l.k_pages, (n * 8).div_ceil(page), "{n}");
                assert!(l.k_pages * page >= n * 8 && (l.k_pages - 1) * page < n * 8);
                assert_eq!(l.samples, n.div_ceil(STRIDE_SAMPLE));
                assert!((l.samples - 1) * STRIDE_SAMPLE < n && l.samples * STRIDE_SAMPLE >= n);
                assert_eq!(l.row_pages(), l.v_pages + l.k_pages);
                assert_eq!(l.res_off, l.row_pages() * page);
                assert_eq!(l.row_len(), 4 * l.row_pages());
                assert_eq!(l.samples_off() % 8, 0);
                assert!(l.samples_off() >= l.res_off + l.row_len());
                assert!(l.samples_off() < l.res_off + l.row_len() + 8);
                assert_eq!(l.res_off + l.res_len, l.samples_off() + 8 * l.samples);
                assert_eq!(l.res_pages, l.res_len.div_ceil(page));
                assert_eq!(l.file_len, (l.row_pages() + l.res_pages + 1) * page);
                // A form that holds no number has no page or sample of
                // them, and its row is its values' alone.
                assert_eq!((dense.k_pages, dense.samples), (0, 0));
                assert_eq!(dense.row_len(), dense.res_len);
            }
        }
        assert_eq!(stride_layout_sparse(u64::MAX, 2), None);
        assert_eq!(stride_layout_sparse(u64::MAX / 4, 1), None);
    }

    /// The line and its error, against the definition: over numbers of
    /// several spreads, every number's guess is within the measured error
    /// of the last sample at or below it, some number's is exactly that
    /// far, and any value between two numbers is within one more.
    #[test]
    fn the_lines_error_bounds_every_numbers_guess_and_is_reached() {
        let mut rng = Rng::new(0x51_7EED);
        let step = STRIDE_SAMPLE as usize;
        let mut spreads: Vec<(&str, Vec<u64>)> = Vec::new();
        for n in [1usize, 2, 63, 64, 65, 1000, 20_000] {
            // A whole run, where the line is exact to a sample.
            spreads.push(("a whole run", (0..n as u64).map(|i| 7 + i).collect()));
            // Uniform draws.
            let mut v: Vec<u64> = (0..n).map(|_| rng.word()).collect();
            v.sort_unstable();
            v.dedup();
            spreads.push(("uniform", v));
            // All but the last in one corner: the line as wrong as it gets.
            let mut v: Vec<u64> = (0..n as u64).collect();
            *v.last_mut().unwrap() = u64::MAX;
            spreads.push(("a corner", v));
            // Squares: dense at the low end, thin at the high.
            let mut v: Vec<u64> = (0..n as u64).map(|i| i * i * 1000).collect();
            v.dedup();
            spreads.push(("squares", v));
        }
        for (what, numbers) in spreads {
            let n = numbers.len();
            let (lo, hi) = (numbers[0], numbers[n - 1]);
            let m = (n as u64).div_ceil(STRIDE_SAMPLE);
            let eps = u64::from(stride_eps(&numbers));
            let mut worst = 0;
            let mut before = 0;
            for (r, &x) in numbers.iter().enumerate() {
                let g = stride_line(lo, hi, m, x);
                assert!(
                    g <= m,
                    "{what} {n}: the guess is among the samples or one past"
                );
                assert!(g >= before, "{what} {n}: the line never falls");
                before = g;
                let s = (r / step) as u64;
                assert!(
                    g.abs_diff(s) <= eps,
                    "{what} {n}: rank {r} guessed {g}, error {eps}"
                );
                worst = worst.max(g.abs_diff(s));
                // A value just past a number, short of the next: its last
                // sample is this number's.
                if numbers.get(r + 1).is_some_and(|&next| next > x + 1) {
                    let g = stride_line(lo, hi, m, x + 1);
                    assert!(g.abs_diff(s) <= eps + 1, "{what} {n}: past rank {r}");
                }
            }
            assert_eq!(worst, eps, "{what} {n}: the error is one some number has");
            // Outside the two ends the line is held at them.
            assert_eq!(stride_line(lo, hi, m, lo.saturating_sub(1)), 0);
            assert_eq!(
                stride_line(lo, hi, m, hi.saturating_add(1)),
                if hi > lo { m } else { 0 }
            );
            if what == "a whole run" {
                assert!(eps <= 1, "{n}: a whole run is on the line: {eps}");
            }
            if what == "a corner" && n >= 2 * step {
                assert!(
                    eps >= m - 1,
                    "{n}: a corner is off it by nearly every sample: {eps}"
                );
            }
        }
        assert_eq!(stride_eps(&[]), 0);
    }

    #[test]
    fn a_stride_page_is_bound_to_its_file_and_its_index() {
        let mut rng = Rng::new(47);
        let page = rng.bytes(ZPAGE);
        let bind = 0x0BAD_CAFE;
        for p in [0u32, 1, 2, 239, u32::MAX] {
            assert_eq!(
                stride_page_crc(&page, bind, p),
                crc32c(&page) ^ bind ^ p.wrapping_mul(STRIDE_PAGE_MIX),
                "page {p}"
            );
        }
        // Another file's bind, or another index, and the word is another.
        assert_ne!(
            stride_page_crc(&page, bind, 7),
            stride_page_crc(&page, bind ^ 1, 7)
        );
        // No two indices give one page's bytes the same word: the mix is
        // odd, so it moves every index to another word.
        assert_eq!(STRIDE_PAGE_MIX % 2, 1);
        let zero = [0u8; ZPAGE];
        let mut words: Vec<u32> = (0..4096).map(|p| stride_page_crc(&zero, bind, p)).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), 4096, "a zero page's words at 4096 indices");
        // The row's own CRC is bound to the file too.
        let row = rng.bytes(4 * 239);
        assert_eq!(stride_row_crc(&row, bind), crc32c(&row) ^ bind);
        assert_ne!(stride_row_crc(&row, bind), stride_row_crc(&row, bind ^ 1));
    }

    #[test]
    fn the_stride_layout_is_its_regions_end_to_end() {
        let page = ZPAGE as u128;
        // Whole pages and one more for a part, by quotient and remainder.
        let pages = |bytes: u128| bytes / page + u128::from(!bytes.is_multiple_of(page));
        for n in [0u64, 1, 40, 41, 345_000, 10_000_000, u64::from(u32::MAX)] {
            for w in [1u32, 99, 100, 4095, STRIDE_W_MAX] {
                let l = stride_layout(n, w).expect("no overflow inside a trailer's ranges");
                let values = u128::from(n) * u128::from(w);
                let v = pages(values);
                assert_eq!(u128::from(l.v_pages), v, "{n} {w}");
                assert!(u128::from(l.v_pages) * page >= values);
                assert!(values == 0 || (u128::from(l.v_pages) - 1) * page < values);
                assert_eq!(u128::from(l.res_off), v * page);
                assert_eq!(u128::from(l.res_len), 4 * v);
                assert_eq!(u128::from(l.res_pages), pages(4 * v));
                assert_eq!(
                    u128::from(l.file_len),
                    (v + u128::from(l.res_pages) + 1) * page,
                    "{n} {w}"
                );
                assert_eq!(l.file_len % ZPAGE as u64, 0);
            }
        }
        assert_eq!(stride_layout(u64::MAX, 2), None);
        assert_eq!(stride_layout(u64::MAX / 2, STRIDE_W_MAX), None);
    }

    fn sample_stride_trailer() -> StrideTrailer {
        StrideTrailer {
            codec: KeyCodec::new(KeyKind::Decimal, b"0000000000", 6, 0).unwrap(),
            w: 100,
            res_crc: 0xF00D_CAFE,
            seg: sample_trailer().seg_ident(),
            sparse: None,
        }
    }

    #[test]
    fn the_stride_trailer_round_trips() {
        let plain = sample_stride_trailer();
        let mut shapes = vec![plain];
        shapes.push(StrideTrailer {
            codec: KeyCodec::new(KeyKind::BigEndian, b"", STRIDE_BE_MAX, 1 << 40).unwrap(),
            w: 1,
            ..plain
        });
        let long = vec![0xa5u8; STRIDE_KEY_MAX - 1];
        shapes.push(StrideTrailer {
            codec: KeyCodec::new(KeyKind::BigEndian, &long, 1, 0).unwrap(),
            w: STRIDE_W_MAX,
            seg: SegIdent {
                nkeys: 256,
                ..plain.seg
            },
            ..plain
        });
        shapes.push(StrideTrailer {
            seg: SegIdent {
                nkeys: 1,
                ..plain.seg
            },
            ..plain
        });
        // The most keys a page index counts, a value a key.
        shapes.push(StrideTrailer {
            codec: KeyCodec::new(KeyKind::BigEndian, b"", 4, 0).unwrap(),
            w: 1,
            seg: SegIdent {
                nkeys: u64::from(u32::MAX),
                ..plain.seg
            },
            ..plain
        });
        for t in shapes {
            let b = t.encode().expect("a trailer of consistent fields");
            assert_eq!(StrideTrailer::decode(&b), Ok(t), "{t:?}");
            let l = stride_layout(t.seg.nkeys, t.w).unwrap();
            assert_eq!(t.layout(), Ok(l));
            assert_eq!(t.expected_file_len(), Some(l.file_len));
            assert_eq!(t.seg_ident(), t.seg);
            assert_eq!(
                t.bind32(),
                bind32(t.seg.seg_id, t.seg.seg_end_seq, t.seg.nkeys, t.seg.row_crc)
            );
        }
        // The block form's trailer names the same segment by the same
        // fields.
        let b = sample_trailer();
        assert_eq!(b.seg_ident().bind32(), b.bind32());
        // Fields that make no file are refused before a byte is written.
        let zero = StrideTrailer { w: 0, ..plain };
        assert!(matches!(zero.encode(), Err(ZError::Trailer(_))));
        assert_eq!(zero.expected_file_len(), None);
        let none = StrideTrailer {
            seg: SegIdent {
                nkeys: 0,
                ..plain.seg
            },
            ..plain
        };
        assert!(matches!(none.encode(), Err(ZError::Trailer(_))));
    }

    #[test]
    fn the_stride_trailer_is_read_by_an_independent_decoder() {
        let t = sample_stride_trailer();
        let b = t.encode().unwrap();
        let c = &t.codec;
        let s = &t.seg;
        let n = s.nkeys;
        let v = (n * u64::from(t.w)).div_ceil(4096);
        assert_eq!(&b[0..8], b"SUPDZST1");
        assert_eq!(le32(&b, 8), 1, "version");
        assert_eq!(le32(&b, 12), 4096, "page size");
        assert_eq!(b[16], 1, "the decimal codec");
        assert_eq!(usize::from(b[17]), c.d());
        assert_eq!(usize::from(b[18]), c.key_len());
        assert_eq!(usize::from(b[19]), c.prefix().len());
        assert_eq!(le32(&b, 20), t.w);
        for (at, want) in [
            (24, c.base()),
            (32, v),
            (40, 4096 * v),
            (48, 4 * v),
            (64, s.seg_id),
            (72, s.seg_end_seq),
            (80, s.nkeys),
            (88, s.recs_len),
            (96, s.crc_off),
            (104, s.key_off),
            (112, s.hash_cap),
            (120, s.file_len),
            (128, s.timestamp),
            (136, s.generation),
            (152, s.rec_magic),
        ] {
            assert_eq!(le64(&b, at), want, "the u64 at {at}");
        }
        for (at, want) in [(56, t.res_crc), (144, s.row_crc), (148, s.rec_version)] {
            assert_eq!(le32(&b, at), want, "the u32 at {at}");
        }
        let p = c.prefix().len();
        assert_eq!(&b[160..160 + p], c.prefix());
        assert!(
            b[160 + p..4092].iter().all(|&x| x == 0),
            "zeros past the prefix"
        );
        assert_eq!(
            le32(&b, 4092),
            crc32c(&b[..4092]),
            "the CRC covers [0, 4092)"
        );
        // bind32 is the CRC of the 28 identity bytes, in the block form's
        // order.
        let mut ident = Vec::new();
        ident.extend_from_slice(&b[64..72]);
        ident.extend_from_slice(&b[72..80]);
        ident.extend_from_slice(&b[80..88]);
        ident.extend_from_slice(&b[144..148]);
        assert_eq!(le32(&b, 60), crc32c(&ident));
        let big = StrideTrailer {
            codec: KeyCodec::new(KeyKind::BigEndian, b"", 2, 0).unwrap(),
            ..t
        };
        assert_eq!(big.encode().unwrap()[16], 2, "the big-endian codec");
    }

    #[test]
    fn a_flipped_byte_anywhere_in_a_stride_trailer_is_refused() {
        let b = sample_stride_trailer().encode().unwrap();
        for at in 0..ZTRAILER {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut bad = b;
                bad[at] ^= flip;
                let want = match at {
                    0..=7 => ZError::Magic,
                    8..=11 => ZError::Version,
                    12..=15 => ZError::PageSize,
                    _ => ZError::TrailerCrc,
                };
                assert_eq!(
                    StrideTrailer::decode(&bad),
                    Err(want),
                    "byte {at} ^ {flip:#x}"
                );
            }
        }
        assert_eq!(
            StrideTrailer::decode(&b[..ZTRAILER - 1]),
            Err(ZError::Length)
        );
        assert_eq!(StrideTrailer::decode(&[]), Err(ZError::Length));
        assert_eq!(StrideTrailer::decode(&[0u8; ZTRAILER]), Err(ZError::Magic));
        // Each kind's decoder refuses the other's trailer at the magic: what
        // an engine that reads only the block form does with a stride file,
        // and the other way.
        assert_eq!(Trailer::decode(&b), Err(ZError::Magic));
        assert_eq!(
            StrideTrailer::decode(&sample_trailer().encode()),
            Err(ZError::Magic)
        );
    }

    #[test]
    fn damage_the_stride_trailers_crc_cannot_see_is_refused_by_the_checks_behind_it() {
        let base = sample_stride_trailer().encode().unwrap();
        assert!(StrideTrailer::decode(&base).is_ok());
        let bad = |f: &dyn Fn(&mut [u8; ZTRAILER])| {
            let mut b = base;
            f(&mut b);
            resign_trailer(&mut b);
            StrideTrailer::decode(&b).err()
        };
        let why = |e: Option<ZError>| match e {
            Some(ZError::Trailer(w)) => w,
            other => panic!("not an inconsistent trailer: {other:?}"),
        };
        let p = sample_stride_trailer().codec.prefix().len();
        let codec = "the key codec is not one this reader knows";
        assert_eq!(why(bad(&|b| b[S_CODEC] = 0)), codec);
        assert_eq!(why(bad(&|b| b[S_CODEC] = 5)), codec);
        assert_eq!(why(bad(&|b| b[S_CODEC] = 0xff)), codec);
        // The hex kinds' codes are kinds: the same field read as hex
        // digits is a run its six digits hold, of another codec.
        for (code, kind) in [(3, KeyKind::HexLower), (4, KeyKind::HexUpper)] {
            let mut b = base;
            b[S_CODEC] = code;
            resign_trailer(&mut b);
            assert_eq!(StrideTrailer::decode(&b).map(|t| t.codec.kind()), Ok(kind));
        }
        let length = "the key length is not one to the most a stride form takes";
        assert_eq!(why(bad(&|b| b[S_L] = 0)), length);
        assert_eq!(why(bad(&|b| b[S_L] = STRIDE_KEY_MAX as u8 + 1)), length);
        let parts = "the prefix and the field are not the key's length";
        assert_eq!(why(bad(&|b| b[S_P] += 1)), parts);
        assert_eq!(why(bad(&|b| b[S_D] -= 1)), parts);
        // A field wider than its kind, with the length made to agree; and a
        // field of nothing, all of the key the prefix.
        let width = "the field's width is not one to the most its kind holds";
        assert_eq!(
            why(bad(&|b| {
                b[S_D] = STRIDE_DECIMAL_MAX as u8 + 1;
                b[S_L] = b[S_P] + b[S_D];
            })),
            width
        );
        assert_eq!(
            why(bad(&|b| {
                b[S_CODEC] = KeyKind::BigEndian.code();
                b[S_D] = STRIDE_BE_MAX as u8 + 1;
                b[S_L] = b[S_P] + b[S_D];
            })),
            width
        );
        assert_eq!(
            why(bad(&|b| {
                b[S_D] = 0;
                b[S_L] = b[S_P];
            })),
            width
        );
        // The run in the field: its first number past it, and its last.
        let top = sample_stride_trailer().codec.top();
        assert_eq!(
            why(bad(&|b| put64(b, S_BASE, top + 1))),
            "the first key's number is past what the field holds"
        );
        assert_eq!(
            why(bad(&|b| put64(b, S_BASE, top))),
            "the run passes what its key field holds"
        );
        // The key count a page index counts.
        let count = "the key count is not one to what a page index counts";
        assert_eq!(why(bad(&|b| put64(b, S_SEG + SI_NKEYS, 0))), count);
        assert_eq!(
            why(bad(&|b| put64(
                b,
                S_SEG + SI_NKEYS,
                u64::from(u32::MAX) + 1
            ))),
            count
        );
        // The width.
        let w = "the value width is not one to a page";
        assert_eq!(why(bad(&|b| put32(b, S_W, 0))), w);
        assert_eq!(why(bad(&|b| put32(b, S_W, STRIDE_W_MAX + 1))), w);
        // The sizes, each against the layout's.
        assert_eq!(why(bad(&|b| b[S_V] ^= 1)), "V is not the values' pages");
        assert_eq!(
            why(bad(&|b| b[S_RES_OFF + 1] ^= 0x10)),
            "res_off is not the values' length"
        );
        assert_eq!(
            why(bad(&|b| b[S_RES_LEN] ^= 4)),
            "res_len is not the row's length"
        );
        // The zeros: past the prefix, and after the prefix's room.
        let past = "a byte past the prefix is not zero";
        for at in [S_PREFIX + p, S_ZERO - 1] {
            assert_eq!(why(bad(&|b| b[at] = 1)), past, "byte {at}");
        }
        for at in [S_ZERO, 2000, T_CRC - 1] {
            assert_eq!(
                why(bad(&|b| b[at] = 1)),
                "a reserved byte is not zero",
                "byte {at}"
            );
        }
        // bind32 against the fields it hashes, each moved without it.
        let bind = "bind32 is not the hash of the segment's identity fields";
        assert_eq!(why(bad(&|b| b[S_BIND32] ^= 1)), bind);
        for (field, at) in [
            ("seg_id", S_SEG + SI_SEG_ID),
            ("seg_end_seq", S_SEG + SI_SEG_END_SEQ),
            ("nkeys", S_SEG + SI_NKEYS),
            ("row_crc", S_SEG + SI_ROW_CRC),
        ] {
            assert_eq!(why(bad(&|b| b[at] ^= 1)), bind, "{field} without bind32");
        }
        // The fields only the attach can judge are the segment's own
        // account, and decode takes them as written.
        let mut b = base;
        b[S_SEG + SI_RECS_LEN] ^= 4;
        resign_trailer(&mut b);
        let t = StrideTrailer::decode(&b).unwrap();
        assert_eq!(t.seg.recs_len, sample_stride_trailer().seg.recs_len ^ 4);
        // A version or a page size the CRC covers is refused before it.
        for v in [0u32, STRIDE_VERSION_SPARSE + 1] {
            let mut b = base;
            put32(&mut b, T_VERSION, v);
            assert_eq!(
                StrideTrailer::decode(&b),
                Err(ZError::Version),
                "version {v}"
            );
        }
        // The sparse form's version over a trailer that is not one: torn
        // as it stands, and with its CRC made good a sparse trailer whose
        // fields, zeros where a whole run's are, name no numbers.
        let mut b = base;
        put32(&mut b, T_VERSION, STRIDE_VERSION_SPARSE);
        assert_eq!(StrideTrailer::decode(&b), Err(ZError::TrailerCrc));
        resign_trailer(&mut b);
        assert_eq!(
            why(StrideTrailer::decode(&b).err()),
            "the numbers between the ends are fewer than the keys"
        );
        let mut b = base;
        put32(&mut b, T_PAGE_SIZE, 16_384);
        assert_eq!(StrideTrailer::decode(&b), Err(ZError::PageSize));
    }

    /// A sparse trailer over the sample's segment: sixteen hex digits
    /// behind no prefix, the first number and the last a spread apart.
    fn sample_sparse_trailer() -> StrideTrailer {
        let plain = sample_stride_trailer();
        StrideTrailer {
            codec: KeyCodec::new(KeyKind::HexLower, b"", STRIDE_HEX_MAX, 0x1000).unwrap(),
            sparse: Some(StrideSparse {
                last: u64::MAX - 5,
                eps: 3,
            }),
            ..plain
        }
    }

    #[test]
    fn the_sparse_stride_trailer_round_trips_and_is_read_by_an_independent_decoder() {
        let t = sample_sparse_trailer();
        let n = t.seg.nkeys;
        let mut shapes = vec![t];
        // A run of one key: its number both ends, a line with no error.
        shapes.push(StrideTrailer {
            seg: SegIdent { nkeys: 1, ..t.seg },
            sparse: Some(StrideSparse {
                last: 0x1000,
                eps: 0,
            }),
            ..t
        });
        // Exactly as many numbers as keys: a whole run held as a sparse
        // one, which the layout takes and no builder writes.
        shapes.push(StrideTrailer {
            sparse: Some(StrideSparse {
                last: 0x1000 + n - 1,
                eps: 1,
            }),
            ..t
        });
        // Every kind holds one; the error as large as the samples.
        for kind in KEY_KINDS {
            shapes.push(StrideTrailer {
                codec: KeyCodec::new(kind, b"id-", 5, 7).unwrap(),
                sparse: Some(StrideSparse {
                    last: KeyCodec::new(kind, b"id-", 5, 7).unwrap().top(),
                    eps: n.div_ceil(STRIDE_SAMPLE) as u32,
                }),
                seg: SegIdent {
                    nkeys: n.min(1000),
                    ..t.seg
                },
                ..t
            });
        }
        for t in shapes {
            let b = t.encode().expect("a sparse trailer of consistent fields");
            assert_eq!(StrideTrailer::decode(&b), Ok(t), "{t:?}");
            let l = stride_layout_sparse(t.seg.nkeys, t.w).unwrap();
            assert_eq!(t.layout(), Ok(l));
            assert_eq!(t.expected_file_len(), Some(l.file_len));
            // The same fields without the numbers are the smaller file,
            // where they make one.
            let dense = StrideTrailer { sparse: None, ..t };
            if let Ok(d) = dense.layout() {
                assert!(d.file_len < l.file_len);
            }
        }
        // The bytes, by offsets written out: the head's version, the
        // fields a whole run's trailer has at their places, and the four
        // of a sparse one where that trailer holds zeros.
        let b = t.encode().unwrap();
        let v = (n * u64::from(t.w)).div_ceil(4096);
        let k = (n * 8).div_ceil(4096);
        let m = n.div_ceil(64);
        assert_eq!(&b[0..8], b"SUPDZST1");
        assert_eq!(le32(&b, 8), 2, "the sparse version");
        assert_eq!(b[16], 3, "the lowercase hex codec");
        assert_eq!((b[17], b[18], b[19]), (16, 16, 0));
        assert_eq!(le32(&b, 20), t.w);
        assert_eq!(le64(&b, 24), 0x1000, "the first number");
        assert_eq!(le64(&b, 32), v);
        assert_eq!(
            le64(&b, 40),
            4096 * (v + k),
            "the resident part after the numbers"
        );
        assert_eq!(le64(&b, 48), (4 * (v + k)).next_multiple_of(8) + 8 * m);
        assert_eq!(le32(&b, 56), t.res_crc);
        assert_eq!(le64(&b, 224), k, "the numbers' pages");
        assert_eq!(le64(&b, 232), u64::MAX - 5, "the last number");
        assert_eq!(le32(&b, 240), 3, "the line's error");
        assert_eq!(le32(&b, 244), 64, "the sample stride");
        assert!(
            b[248..4092].iter().all(|&x| x == 0),
            "zeros past the fields"
        );
        assert_eq!(le32(&b, 4092), crc32c(&b[..4092]));
        assert_eq!(
            sample_sparse_trailer()
                .encode()
                .map(|b| StrideTrailer::decode(&b).unwrap().sparse),
            Ok(t.sparse)
        );
        // A whole run's trailer is the first version's bytes still.
        let dense = sample_stride_trailer().encode().unwrap();
        assert_eq!(le32(&dense, 8), 1);
        assert!(dense[224..4092].iter().all(|&x| x == 0));
        // The hex kinds' codes.
        let upper = StrideTrailer {
            codec: KeyCodec::new(KeyKind::HexUpper, b"", 4, 0).unwrap(),
            sparse: Some(StrideSparse {
                last: 0xffff,
                eps: 0,
            }),
            seg: SegIdent { nkeys: 9, ..t.seg },
            ..t
        };
        assert_eq!(upper.encode().unwrap()[16], 4, "the uppercase hex codec");
    }

    #[test]
    fn a_sparse_stride_trailers_fields_are_held_to_each_other() {
        let t = sample_sparse_trailer();
        let base = t.encode().unwrap();
        assert!(StrideTrailer::decode(&base).is_ok());
        // A flipped byte anywhere is the head's or the CRC's to refuse.
        for at in (0..ZTRAILER).step_by(7).chain([224, 232, 240, 244, 247]) {
            let mut bad = base;
            bad[at] ^= 0x40;
            let want = match at {
                0..=7 => ZError::Magic,
                8..=11 => ZError::Version,
                12..=15 => ZError::PageSize,
                _ => ZError::TrailerCrc,
            };
            assert_eq!(StrideTrailer::decode(&bad), Err(want), "byte {at}");
        }
        let bad = |f: &dyn Fn(&mut [u8; ZTRAILER])| {
            let mut b = base;
            f(&mut b);
            resign_trailer(&mut b);
            StrideTrailer::decode(&b).err()
        };
        let why = |e: Option<ZError>| match e {
            Some(ZError::Trailer(w)) => w,
            other => panic!("not an inconsistent trailer: {other:?}"),
        };
        let n = t.seg.nkeys;
        // The last number: before the first, and with fewer numbers
        // between the two than the form has keys; one fewer is refused and
        // exactly as many is taken.
        let ends = "the last key's number is not in the field past the first";
        assert_eq!(why(bad(&|b| put64(b, S_LAST, 0x0fff))), ends);
        let few = "the numbers between the ends are fewer than the keys";
        assert_eq!(why(bad(&|b| put64(b, S_LAST, 0x1000 + n - 2))), few);
        assert_eq!(bad(&|b| put64(b, S_LAST, 0x1000 + n - 1)), None);
        // Past the field, where the field is narrower than a u64.
        let narrow = StrideTrailer {
            codec: KeyCodec::new(KeyKind::HexLower, b"k", 6, 0).unwrap(),
            sparse: Some(StrideSparse {
                last: 0xff_ffff,
                eps: 0,
            }),
            ..t
        };
        assert!(narrow.encode().is_ok());
        let past = StrideTrailer {
            sparse: Some(StrideSparse {
                last: 0x100_0000,
                eps: 0,
            }),
            ..narrow
        };
        assert_eq!(why(past.encode().err()), ends);
        // The line's error: as many samples as the form has is taken, one
        // more is not.
        let m = n.div_ceil(STRIDE_SAMPLE) as u32;
        assert_eq!(bad(&|b| put32(b, S_EPS, m)), None);
        assert_eq!(
            why(bad(&|b| put32(b, S_EPS, m + 1))),
            "the line's error is more samples than the form has"
        );
        // The numbers' pages and the sample stride against this reader's.
        assert_eq!(why(bad(&|b| b[S_K] ^= 1)), "K is not the numbers' pages");
        assert_eq!(
            why(bad(&|b| put32(b, S_SAMPLE, 32))),
            "the sample stride is not this reader's"
        );
        // The sizes are the sparse layout's: a whole run's are refused.
        let dense = stride_layout(n, t.w).unwrap();
        assert_eq!(
            why(bad(&|b| put64(b, S_RES_OFF, dense.res_off))),
            "res_off is not the values' length"
        );
        assert_eq!(
            why(bad(&|b| put64(b, S_RES_LEN, dense.res_len))),
            "res_len is not the row's length"
        );
        // The zeros after the four fields.
        for at in [S_ZERO_SPARSE, 2000, T_CRC - 1] {
            assert_eq!(
                why(bad(&|b| b[at] = 1)),
                "a reserved byte is not zero",
                "byte {at}"
            );
        }
        // The first version over a sparse trailer's bytes: the four fields
        // are bytes where a whole run's trailer holds zeros.
        let mut b = base;
        put32(&mut b, T_VERSION, STRIDE_VERSION);
        resign_trailer(&mut b);
        assert!(matches!(StrideTrailer::decode(&b), Err(ZError::Trailer(_))));
        // A reader that knows the first version alone refuses the second
        // at the version, before the CRC: the head's order.
        assert_eq!(
            open_trailer(&base, &STRIDE_MAGIC, STRIDE_VERSION),
            Err(ZError::Version)
        );
    }
}
