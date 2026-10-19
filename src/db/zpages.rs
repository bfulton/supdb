//! The z form's layout: a derived sidecar that holds a partition's records,
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
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | `crc`: CRC-32C over bytes `[4, 4096)`, padding included, xor the trailer's `bind32` |
//! | 4 | 4 | `tag`, `"ZPG1"`, never zero: a zeroed page cannot pass |
//! | 8 | 4 | `first_rank` |
//! | 12 | 2 | `count`, 1 to [`ZMAX_COUNT`] |
//! | 14 | 1 | `flags`: bit 0 stored (the payload is the group verbatim) |
//! | 15 | 1 | zero |
//! | 16 | 4 | `ulen`: bytes of the group, a multiple of 4, at most [`ZMAX_ULEN`] |
//! | 20 | 2 | `clen`: payload bytes, at most [`ZCAP`] |
//! | 22 | 2 | zero |
//! | 24 | `clen` | the payload: one LZ4 block, no size prefix and no frame, or the group |
//! | 24 + `clen` | the rest | zeros |
//!
//! The page carries `clen` because the decoder stops only when its input is
//! exhausted after a literal run: handed the page's trailing zeros it parses
//! them as further sequences. The CRC is xored with `bind32`, a hash of the
//! segment's identity, so a page copied in from another sidecar fails its
//! own check without a second field to read. It is checked before anything
//! is decoded, because LZ4 decodes many corrupt inputs into plausible bytes.
//!
//! A group is the concatenation of the bytes `parse_record` consumes for each
//! of its records, in rank order, each image a multiple of four bytes. One
//! decoder then serves every read, count and scan, and no second definition
//! of the record exists to drift.
//!
//! # Why it is a child of `db`
//!
//! The filter stored in region B is `BlockedBloom`'s words and the walk over
//! a group compares keys as the engine's other searches do (`key_cmp`); both
//! are private to `db`, and a child module reaches them without widening any
//! visibility. The items here are public for the reason `flatindex`'s are:
//! the tools in `bench/` price the layout they link, and a copy of it in an
//! example is the second definition. The module is hidden from the crate's
//! documentation until a reader of the sidecar exists, since a layout nothing
//! reads is not yet an interface anyone is promised.

use crate::block::crc32;
use crate::flatindex::{parse_record, rd_u32, record_key_next};
use std::cmp::Ordering;
use std::fmt;

/// The page: one fault, one page-cache page, and the unit of residency.
pub const ZPAGE: usize = 4096;
/// The page header's length.
pub const ZHDR: usize = 24;
/// Payload bytes a page holds.
pub const ZCAP: usize = ZPAGE - ZHDR;
/// The most records in a group: `count` is a u16, and the buffer a group
/// decodes into has to be bounded.
pub const ZMAX_COUNT: usize = 256;
/// The most bytes in a decoded group, and the size of the buffer one decodes
/// into. LZ4's window is 64 KiB, so a longer group would not compress better
/// for its length.
pub const ZMAX_ULEN: usize = 64 * 1024;
/// A fence entry's page number for records the form does not hold.
pub const ZNOPAGE: u32 = u32::MAX;
/// A page's tag, `"ZPG1"` as the bytes on disk: the digit moves whenever an
/// older reader would misread a page rather than refuse it.
pub const ZTAG: u32 = u32::from_le_bytes(*b"ZPG1");
/// The trailer's magic: the digit moves whenever an older reader would
/// misread the file rather than refuse it.
pub const ZMAGIC: [u8; 8] = *b"SUPDZPG1";
/// The trailer's version: what an older reader refuses without misreading.
pub const ZVERSION: u32 = 1;
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

/// Page flag: the payload is the group verbatim.
const F_STORED: u8 = 1;

// The page header's fields.
const P_CRC: usize = 0;
const P_TAG: usize = 4;
const P_FIRST: usize = 8;
const P_COUNT: usize = 12;
const P_FLAGS: usize = 14;
const P_ULEN: usize = 16;
const P_CLEN: usize = 20;

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
    /// A page's CRC does not match its bytes under this sidecar's `bind32`.
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
/// the zeros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageHeader {
    /// The rank of the group's first record.
    pub first_rank: u32,
    /// Records in the group, 1 to [`ZMAX_COUNT`].
    pub count: u16,
    /// The payload is the group verbatim, and `clen == ulen`.
    pub stored: bool,
    /// Bytes of the decoded group: a multiple of four, at most [`ZMAX_ULEN`].
    pub ulen: u32,
    /// Payload bytes, at most [`ZCAP`].
    pub clen: u16,
}

impl PageHeader {
    /// The values a page may carry, which writing and reading both hold it to.
    fn check(&self) -> Result<(), ZError> {
        if self.count == 0 || self.count as usize > ZMAX_COUNT {
            return Err(ZError::Header("count is not 1 to the most a group holds"));
        }
        if self.ulen == 0 || !self.ulen.is_multiple_of(4) || self.ulen as usize > ZMAX_ULEN {
            return Err(ZError::Header(
                "ulen is not a multiple of four in 4 to the most a group holds",
            ));
        }
        if self.clen == 0 || self.clen as usize > ZCAP {
            return Err(ZError::Header("clen is not 1 to the most a page holds"));
        }
        if self.stored && u32::from(self.clen) != self.ulen {
            return Err(ZError::Header("a stored page's clen is not its ulen"));
        }
        Ok(())
    }
}

/// The page's checksum: CRC-32C over everything after the CRC field, xor the
/// sidecar's `bind32`.
pub fn page_crc(page: &[u8; ZPAGE], bind32: u32) -> u32 {
    crc32(&page[P_TAG..]) ^ bind32
}

/// Write one page: the header, the payload, zeros, and the CRC last. The
/// header and the payload must agree, and the page is untouched when they do
/// not.
pub fn encode_page(
    page: &mut [u8; ZPAGE],
    h: &PageHeader,
    payload: &[u8],
    bind32: u32,
) -> Result<(), ZError> {
    h.check()?;
    if payload.len() != h.clen as usize {
        return Err(ZError::Header("the payload is not clen bytes"));
    }
    page[P_TAG..P_TAG + 4].copy_from_slice(&ZTAG.to_le_bytes());
    page[P_FIRST..P_FIRST + 4].copy_from_slice(&h.first_rank.to_le_bytes());
    page[P_COUNT..P_COUNT + 2].copy_from_slice(&h.count.to_le_bytes());
    page[P_FLAGS] = if h.stored { F_STORED } else { 0 };
    page[P_FLAGS + 1] = 0;
    page[P_ULEN..P_ULEN + 4].copy_from_slice(&h.ulen.to_le_bytes());
    page[P_CLEN..P_CLEN + 2].copy_from_slice(&h.clen.to_le_bytes());
    page[P_CLEN + 2..ZHDR].fill(0);
    page[ZHDR..ZHDR + payload.len()].copy_from_slice(payload);
    page[ZHDR + payload.len()..].fill(0);
    let crc = page_crc(page, bind32);
    page[P_CRC..P_CRC + 4].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}

/// A page's header and payload, after every check that needs no fence and no
/// decoder: the length, the CRC (first, so nothing below trusts an
/// unchecked byte), the tag, the flags, the zeros, the header's values and
/// the zeros after the payload.
pub fn parse_page(page: &[u8], bind32: u32) -> Result<(PageHeader, &[u8]), ZError> {
    let page: &[u8; ZPAGE] = page.try_into().map_err(|_| ZError::Length)?;
    let word = |at: usize| u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]]);
    let half = |at: usize| u16::from_le_bytes([page[at], page[at + 1]]);
    if word(P_CRC) != page_crc(page, bind32) {
        return Err(ZError::PageCrc);
    }
    if word(P_TAG) != ZTAG {
        return Err(ZError::Tag);
    }
    if page[P_FLAGS] & !F_STORED != 0 {
        return Err(ZError::Header(
            "a flag bit this reader does not know is set",
        ));
    }
    if page[P_FLAGS + 1] != 0 || half(P_CLEN + 2) != 0 {
        return Err(ZError::Header("a reserved byte is not zero"));
    }
    let h = PageHeader {
        first_rank: word(P_FIRST),
        count: half(P_COUNT),
        stored: page[P_FLAGS] & F_STORED != 0,
        ulen: word(P_ULEN),
        clen: half(P_CLEN),
    };
    h.check()?;
    let end = ZHDR + h.clen as usize;
    if page[end..].iter().any(|&b| b != 0) {
        return Err(ZError::Padding);
    }
    Ok((h, &page[ZHDR..end]))
}

/// A page's header and payload for a page this process has already passed
/// through [`parse_page`] and a first decode: the length and the header's
/// values are checked and nothing else, because the CRC is a pass over the
/// page and a reader's every read of a page it has verified would pay it
/// again. The tag, the flags, the zeros and the padding are the first
/// decode's; a page that has not had one is read with [`parse_page`].
pub fn page_view(page: &[u8]) -> Result<(PageHeader, &[u8]), ZError> {
    let page: &[u8; ZPAGE] = page.try_into().map_err(|_| ZError::Length)?;
    let word = |at: usize| u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]]);
    let half = |at: usize| u16::from_le_bytes([page[at], page[at + 1]]);
    let h = PageHeader {
        first_rank: word(P_FIRST),
        count: half(P_COUNT),
        stored: page[P_FLAGS] & F_STORED != 0,
        ulen: word(P_ULEN),
        clen: half(P_CLEN),
    };
    h.check()?;
    Ok((h, &page[ZHDR..ZHDR + h.clen as usize]))
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
pub struct GroupBuf {
    words: Vec<u64>,
}

impl GroupBuf {
    /// An empty buffer, which the first group decoded into it sizes.
    pub fn new() -> GroupBuf {
        GroupBuf { words: Vec::new() }
    }

    /// The first `n` bytes of the buffer, grown to hold them. Not zeroed
    /// between uses: a decode writes every byte of the group it returns, and
    /// nothing beyond it is ever lent. `n` is at most [`ZMAX_ULEN`], which
    /// the header's check has seen to.
    fn bytes_for(&mut self, n: usize) -> &mut [u8] {
        let words = n.div_ceil(8);
        if self.words.len() < words {
            // Whole pages, so that groups a few bytes apart in size grow the
            // buffer once and not once each. The old contents are not kept:
            // nothing is lent from a buffer between two decodes.
            self.words = vec![0; words.next_multiple_of(ZPAGE / 8)];
        }
        // SAFETY: `words` owns at least `n` initialised bytes, `u8` has no
        // alignment to violate, and the borrow of `self` is the only access
        // while the slice lives.
        unsafe { std::slice::from_raw_parts_mut(self.words.as_mut_ptr() as *mut u8, n) }
    }
}

impl GroupBuf {
    /// The first `n` bytes of the buffer as the last decode left them: what
    /// a cursor that keeps its group across calls reads the group through,
    /// since a decode's own borrow ends with the call that made it. `n` is
    /// at most what the last `bytes_for` was asked for.
    pub fn bytes(&self, n: usize) -> &[u8] {
        assert!(n <= self.words.len() * 8, "a group longer than its buffer");
        // SAFETY: `words` owns at least `n` initialised bytes and `u8` has
        // no alignment to violate.
        unsafe { std::slice::from_raw_parts(self.words.as_ptr() as *const u8, n) }
    }
}

impl Default for GroupBuf {
    fn default() -> GroupBuf {
        GroupBuf::new()
    }
}

/// The group a page holds, decoded: LZ4 into `buf`, or for a stored page the
/// payload itself where it is aligned for `parse_record` and a copy in `buf`
/// where it is not. Either way the bytes returned are aligned, so a record
/// of any shape parses out of them.
///
/// The decoder is `lz4_flex`'s `block::decompress_into` called directly, not
/// the engine's `crate::block::decompress_into`, which discards the length it
/// decoded: the buffer is reused without zeroing, so a payload that decodes
/// to fewer bytes than `ulen` would leave stale bytes inside the group, and
/// the length is required to equal `ulen`. The caller has parsed the page
/// ([`parse_page`]) first, so the CRC has passed.
pub fn decode_group<'a>(
    h: &PageHeader,
    payload: &'a [u8],
    buf: &'a mut GroupBuf,
) -> Result<&'a [u8], ZError> {
    h.check()?;
    if payload.len() != usize::from(h.clen) {
        return Err(ZError::Header("the payload is not clen bytes"));
    }
    let ulen = h.ulen as usize;
    if h.stored {
        if (payload.as_ptr() as usize).is_multiple_of(std::mem::align_of::<crate::index::Ext>()) {
            return Ok(payload);
        }
        let out = buf.bytes_for(ulen);
        out.copy_from_slice(payload);
        return Ok(out);
    }
    let out = buf.bytes_for(ulen);
    let n = lz4_flex::block::decompress_into(payload, out).map_err(|_| ZError::Decode)?;
    if n != ulen {
        return Err(ZError::DecodedLen);
    }
    Ok(out)
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
/// nothing after them, keys strictly ascending, and the first the fence's
/// where one is given. `parse_record` judges each record, so a compact header
/// its run contradicts, an extent array that is not aligned and a record cut
/// short are all refused. This is the walk a page's first decode makes, once
/// per page and process; [`group_seek`] is the walk a read makes after it.
pub fn walk_group(group: &[u8], count: usize, first_key: Option<&[u8]>) -> Result<(), ZError> {
    let mut off = 0usize;
    let mut prev: Option<&[u8]> = None;
    for i in 0..count {
        let (key, _, _, len) =
            parse_record(group, off).ok_or(ZError::Walk("a record does not parse"))?;
        if i == 0 && first_key.is_some_and(|f| f != key) {
            return Err(ZError::Walk("the first key is not the fence's"));
        }
        if prev.is_some_and(|p| p >= key) {
            return Err(ZError::Walk("keys do not ascend"));
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

/// A page `fill_group` built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    /// Records in the group, from the rank asked for.
    pub count: usize,
    /// Bytes of the decoded group.
    pub ulen: usize,
    /// Payload bytes in the page.
    pub clen: usize,
    /// The payload is the group verbatim.
    pub stored: bool,
    /// Compress calls this page cost.
    pub calls: u32,
}

/// What `fill_group` made of the records from a rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// A page is in the caller's buffer, decoded and compared with the
    /// images it was built from.
    Page(Group),
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

/// What `fill_group` has done since its scratch was made. Counters only:
/// nothing here changes what a call returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillStats {
    /// Pages built.
    pub pages: u64,
    /// Records in them.
    pub records: u64,
    /// Pages whose payload is the group verbatim.
    pub stored: u64,
    /// Records declined.
    pub nopages: u64,
    /// Compress calls.
    pub calls: u64,
    /// Turns taken, each an attempt to compress a group within a call's
    /// budget: the calls' compress calls but for the one a fallback adds.
    pub turns: u64,
    /// Growth rounds that fit.
    pub growths: u64,
    /// Calls that spent every turn without a fit and fell back to the first
    /// record alone. The arithmetic of the turns says none does.
    pub fallbacks: u64,
    /// Payload bytes in the pages built.
    pub payload_bytes: u64,
    /// Decoded bytes in the pages built.
    pub group_bytes: u64,
}

/// The buffers `fill_group` works in, kept between calls so that a page
/// costs no allocation, and the one knob.
pub struct FillScratch {
    /// The images of the group being tried, end to end, and where each ends.
    imgs: Vec<u8>,
    ends: Vec<usize>,
    /// Compressor output, two buffers because a group that grew and did not
    /// fit must not have overwritten the one that did.
    out: [Vec<u8>; 2],
    /// Where the page just built is decoded to be compared.
    verify: GroupBuf,
    /// Growth rounds after a first fit. [`ZGROW_ROUNDS`] until a measurement
    /// of fill against compress calls says otherwise.
    pub grow_rounds: u32,
    /// What has been done with this scratch.
    pub stats: FillStats,
}

impl FillScratch {
    /// A scratch with every buffer at its largest.
    pub fn new() -> FillScratch {
        // `compress_into` refuses an output shorter than the worst case for
        // its input, whatever the data would compress to, so the buffers
        // are sized for the largest group and a group is judged by the
        // length it compressed to.
        let out = lz4_flex::block::get_maximum_output_size(ZMAX_ULEN);
        FillScratch {
            imgs: Vec::with_capacity(ZMAX_ULEN),
            ends: Vec::with_capacity(ZMAX_COUNT),
            out: [vec![0; out], vec![0; out]],
            verify: GroupBuf::new(),
            grow_rounds: ZGROW_ROUNDS,
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

    /// The staged group compressed into buffer `buf`, and what the length
    /// says to do with it. `None` if the compressor failed, which a buffer
    /// sized for the largest group makes unreachable.
    fn classify(&mut self, ulen: usize, buf: usize) -> Option<Class> {
        let n = lz4_flex::block::compress_into(&self.imgs[..ulen], &mut self.out[buf]).ok()?;
        self.stats.calls += 1;
        Some(if n >= ulen && ulen <= ZCAP {
            // LZ4 did not help and the group fits as it is: its own expansion
            // is never a reason to shrink a group or to give a record up.
            Class::Stored
        } else if n > ZCAP {
            Class::TooBig(n)
        } else {
            Class::Lz4(n)
        })
    }

    fn decline(&mut self, why: NoPage) -> Fill {
        self.stats.nopages += 1;
        Fill::NoPage(why)
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
    Lz4(usize),
    TooBig(usize),
}

#[derive(Clone, Copy)]
enum Body {
    Stored,
    Lz4 { buf: usize, clen: usize },
}

/// A group that fits a page: its records, its decoded bytes and its body.
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

/// Size a group from `rank` so that its compressed bytes fill one page, and
/// build the page into `page`.
///
/// `hint` is where to start: the previous page's record count, which is
/// close to this page's, so a page costs about one compress call when the
/// count fits again and two when it needs a trim or a growth. Zero starts
/// from one record, and a growth by the room that one record leaves reaches
/// the count its own bytes allow, not the count a group of many compresses
/// to: a call with no predecessor builds a page well short of full, and
/// whoever sizes a partition from such calls has to chain a few first. The
/// loop predicts, then verifies: the group of `m` records is compressed, and
/// a group that fits is grown (by the records the slack holds, at the
/// group's own bytes a record, [`FillScratch::grow_rounds`] times) while one
/// that does not is trimmed in proportion to the overshoot for the first
/// four attempts and halved after, so that the count is one by the twelfth
/// attempt at the latest and a record that cannot fit alone is declined, not
/// retried.
///
/// The function is total. No input makes it panic, form a group of no
/// records or loop past its turns: the first record at `rank` is either in
/// a page, or declined as [`NoPage`] for a reason, or `rank` is past the end
/// ([`Fill::End`]). Each of these once was a way to fail:
///
/// * The shrink of a group that overshot, `m * ZCAP / len`, is zero whenever
///   `m * ZCAP < len` -- two records of 4,500 compressed bytes, or seven of
///   5,100 -- and a group of no records divided by its own count. `m` is
///   never below one here.
/// * A lone record of 4,056 to 4,072 bytes that does not compress has an LZ4
///   output over a page although the record fits one. A group whose LZ4
///   output is no smaller than its images is stored verbatim if the images
///   fit, and LZ4's expansion is never a reason to shrink a group or to
///   give a record up.
/// * `compress_into` cannot say whether a group fits a page: it refuses any
///   output buffer shorter than the worst case for its input, whatever the
///   data would compress to. The group is compressed into a buffer for the
///   largest group and its length is compared with [`ZCAP`].
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
pub fn fill_group<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    hint: usize,
    bind32: u32,
    scratch: &mut FillScratch,
    page: &mut [u8; ZPAGE],
) -> Fill {
    fill_with_turns(src, rank, hint, bind32, scratch, page, ZFILL_TURNS)
}

/// `fill_group` with the turn budget a parameter, so that a test can spend
/// it and reach the path after it.
fn fill_with_turns<I: Images + ?Sized>(
    src: &I,
    rank: usize,
    hint: usize,
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
    s.imgs.clear();
    s.ends.clear();
    // The most records the group can hold: the cap, the records left, and
    // the first record that is not an image.
    let mut limit = (n - rank).min(ZMAX_COUNT);
    let mut m = hint.clamp(1, limit);
    let mut turn = 0u32;
    // The output buffer the next compress writes, and the other one holds
    // the group that fit.
    let mut cur = 0usize;
    let mut calls = 0u32;
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
                    return s.decline(NoPage::Unplaceable);
                }
                limit = j;
                m = m.min(j);
                continue;
            }
            Stage::Over => {
                if s.ends.len() == 1 {
                    return s.decline(NoPage::Oversize);
                }
                m = (m / 2).max(1);
                continue;
            }
        }
        turn += 1;
        s.stats.turns += 1;
        let ulen = s.imgs.len();
        let Some(class) = s.classify(ulen, cur) else {
            return Fill::Mismatch;
        };
        calls += 1;
        match class {
            Class::Stored => {
                found = Some(Fit {
                    m,
                    ulen,
                    body: Body::Stored,
                });
                break;
            }
            Class::Lz4(clen) => {
                found = Some(Fit {
                    m,
                    ulen,
                    body: Body::Lz4 { buf: cur, clen },
                });
                cur ^= 1;
                break;
            }
            Class::TooBig(c) => {
                if m == 1 {
                    return s.decline(NoPage::Incompressible);
                }
                m = if turn <= 4 {
                    // In proportion to the overshoot, and at least one.
                    let cut = ((c - ZCAP) * m).div_ceil(c).max(1);
                    m.saturating_sub(cut).max(1)
                } else {
                    (m / 2).max(1)
                };
            }
        }
    }

    let mut fit = match found {
        Some(f) => f,
        None => {
            // Every turn spent without a fit. A turn is an attempt, the first
            // four cut the count in proportion (by at least one) and the rest
            // halve it, so from the most a group holds the count is one at
            // the twelfth and the twelfth decides: this path is not reached
            // with the turns the layout gives. It is the path a smaller
            // budget reaches, and what it does is the first record alone,
            // once, so that a spent budget is a page of one and not a hole.
            s.stats.fallbacks += 1;
            match s.stage(src, rank, 1) {
                Stage::Ok => {}
                Stage::Over => return s.decline(NoPage::Oversize),
                Stage::Bad(_) => return s.decline(NoPage::Unplaceable),
            }
            let ulen = s.imgs.len();
            let Some(class) = s.classify(ulen, cur) else {
                return Fill::Mismatch;
            };
            calls += 1;
            match class {
                Class::Stored => Fit {
                    m: 1,
                    ulen,
                    body: Body::Stored,
                },
                Class::Lz4(clen) => {
                    let f = Fit {
                        m: 1,
                        ulen,
                        body: Body::Lz4 { buf: cur, clen },
                    };
                    cur ^= 1;
                    f
                }
                Class::TooBig(_) => return s.decline(NoPage::Incompressible),
            }
        }
    };

    // Growth: a group that fit with a record's room to spare is grown by the
    // records the slack holds, at the bytes a record has taken so far; past
    // the cap, to the most records that fit under it. A growth that does not
    // fit returns the group that did: it never replaces a group with a
    // smaller one.
    let mut rounds = s.grow_rounds;
    while rounds > 0 && turn < turns {
        let body = fit.body_len();
        let slack = ZCAP - body;
        let per = (body / fit.m).max(1);
        if fit.m >= limit || slack < per {
            break;
        }
        let mut m2 = (fit.m + slack / per).min(limit);
        match s.stage(src, rank, m2) {
            Stage::Ok => {}
            Stage::Bad(j) => {
                limit = j;
                continue;
            }
            Stage::Over => {
                // The grown group passes the cap: grow to the most records
                // that fit under it, if that is more than the group has. The
                // staged bytes are the records so far, so the count is a
                // search of where they end and not another try.
                m2 = s.ends.partition_point(|&e| e <= ZMAX_ULEN);
                if m2 <= fit.m || !matches!(s.stage(src, rank, m2), Stage::Ok) {
                    break;
                }
            }
        }
        turn += 1;
        s.stats.turns += 1;
        let ulen = s.imgs.len();
        let Some(class) = s.classify(ulen, cur) else {
            return Fill::Mismatch;
        };
        calls += 1;
        fit = match class {
            Class::Stored => Fit {
                m: m2,
                ulen,
                body: Body::Stored,
            },
            Class::Lz4(clen) => {
                let f = Fit {
                    m: m2,
                    ulen,
                    body: Body::Lz4 { buf: cur, clen },
                };
                cur ^= 1;
                f
            }
            Class::TooBig(_) => break,
        };
        rounds -= 1;
        s.stats.growths += 1;
    }

    seal_page(s, fit, first_rank, calls, bind32, page)
}

/// The page of a group that fit: written into `page`, then read back as a
/// reader reads a page and compared with the images the group was built
/// from, and only then counted and returned.
///
/// This is the whole of the builder's guard against a page whose CRC is good
/// and whose bytes are wrong -- the CRC is computed over whatever was
/// written, so it cannot see a payload that is not the group -- and it is
/// split from the search for a fit so that a test can hand it a fit whose
/// payload has been damaged since it was judged: no input to `fill_group`
/// reaches the comparison's failure, because the compressor is correct.
fn seal_page(
    s: &mut FillScratch,
    fit: Fit,
    first_rank: u32,
    calls: u32,
    bind32: u32,
    page: &mut [u8; ZPAGE],
) -> Fill {
    let payload: &[u8] = match fit.body {
        Body::Stored => &s.imgs[..fit.ulen],
        Body::Lz4 { buf, clen } => &s.out[buf][..clen],
    };
    let hdr = PageHeader {
        first_rank,
        count: fit.m as u16,
        stored: matches!(fit.body, Body::Stored),
        ulen: fit.ulen as u32,
        clen: payload.len() as u16,
    };
    if encode_page(page, &hdr, payload, bind32).is_err() {
        return Fill::Mismatch;
    }
    if !page_decodes_to(page, bind32, &hdr, &s.imgs[..fit.ulen], &mut s.verify) {
        return Fill::Mismatch;
    }
    s.stats.pages += 1;
    s.stats.records += fit.m as u64;
    s.stats.stored += u64::from(hdr.stored);
    s.stats.payload_bytes += u64::from(hdr.clen);
    s.stats.group_bytes += u64::from(hdr.ulen);
    Fill::Page(Group {
        count: fit.m,
        ulen: fit.ulen,
        clen: usize::from(hdr.clen),
        stored: hdr.stored,
        calls,
    })
}

/// Whether `page`, read the way a reader reads one -- its CRC, its header,
/// its payload decoded -- carries the header `hdr` and decodes to exactly
/// `want`. What was written is read back, not what was meant to be written.
fn page_decodes_to(
    page: &[u8; ZPAGE],
    bind32: u32,
    hdr: &PageHeader,
    want: &[u8],
    verify: &mut GroupBuf,
) -> bool {
    let Ok((h, body)) = parse_page(page, bind32) else {
        return false;
    };
    h == *hdr && decode_group(&h, body, verify).is_ok_and(|g| g == want)
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
        Ok(())
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
    pub fn entry_of(&self, key: &[u8]) -> Option<usize> {
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

    /// Heap bytes the fence holds: the four tables of region B, as they sit
    /// in memory.
    pub fn heap_bytes(&self) -> usize {
        4 * (self.first_rank.len() + self.page.len() + self.key_off.len()) + self.keys.len()
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
                .chunks_exact(4)
                .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
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

/// The hash a sidecar's pages are bound by: CRC-32C over the segment's id,
/// the sequence its name covers, its key count and the CRC of its key
/// section's checksum row, twenty-eight bytes little-endian. Every page's
/// CRC is xored with it, so a page that is another sidecar's fails its own
/// check at no cost.
pub fn bind32(seg_id: u64, seg_end_seq: u64, nkeys: u64, seg_row_crc: u32) -> u32 {
    let mut b = [0u8; 28];
    b[0..8].copy_from_slice(&seg_id.to_le_bytes());
    b[8..16].copy_from_slice(&seg_end_seq.to_le_bytes());
    b[16..24].copy_from_slice(&nkeys.to_le_bytes());
    b[24..28].copy_from_slice(&seg_row_crc.to_le_bytes());
    crc32(&b)
}

/// The bytes of the filter a sidecar stores for a partition of `nkeys` keys:
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
pub fn check_entry(h: &PageHeader, entry: &FenceEntry<'_>) -> Result<(), ZError> {
    if h.first_rank != entry.first_rank || u32::from(h.count) != entry.count {
        return Err(ZError::Fence);
    }
    Ok(())
}

/// That a decoded group is what its page's header and its fence entry say:
/// `count` records, whole, in key order, the first the entry's key
/// ([`walk_group`]). See [`check_entry`] for why it is a function of its own.
pub fn check_group(group: &[u8], h: &PageHeader, entry: &FenceEntry<'_>) -> Result<(), ZError> {
    walk_group(group, usize::from(h.count), Some(entry.key))
}

/// Everything a first decode of a page checks, in the order that trusts the
/// least first: the page's CRC, tag and header; its `first_rank` and `count`
/// against its fence entry ([`check_entry`]); the payload decoded to exactly
/// `ulen`; and the structural walk -- `count` records, whole, ascending, the
/// first the entry's key ([`check_group`]). The builder runs this on every
/// page it writes, as the page will be read, so that a defect that would
/// fail every first decode cannot pass the build and then loop the rebuild;
/// the reader's first decode is these checks in this order with the one
/// difference its mapping makes (`db::zside::first_decode`).
pub fn verify_page(
    page: &[u8],
    bind32: u32,
    entry: &FenceEntry<'_>,
    buf: &mut GroupBuf,
) -> Result<PageHeader, ZError> {
    let (h, payload) = parse_page(page, bind32)?;
    check_entry(&h, entry)?;
    let group = decode_group(&h, payload, buf)?;
    check_group(group, &h, entry)?;
    Ok(h)
}

// The trailer's fields.
const T_MAGIC: usize = 0;
const T_VERSION: usize = 8;
const T_PAGE_SIZE: usize = 12;
const T_N_PAGES: usize = 16;
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
/// Where the zeros begin, and where the trailer's own CRC does.
const T_ZERO: usize = 212;
const T_CRC: usize = ZTRAILER - 4;

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

/// The trailer, the file's last page: what the sidecar holds, the segment it
/// derives from as that segment says of itself, and the filter's
/// parameters. The magic, the version, the page size, the page's capacity
/// and the most records in a group are constants of the layout and are not
/// fields here; `bind32` and the CRC are derived.
///
/// A sidecar holds values, so it is bound to its segment by content and not
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
    /// The hash this sidecar's pages are bound by ([`bind32`]).
    pub fn bind32(&self) -> u32 {
        bind32(self.seg_id, self.seg_end_seq, self.nkeys, self.seg_row_crc)
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
        b[T_MAGIC..T_MAGIC + 8].copy_from_slice(&ZMAGIC);
        put32(&mut b, T_VERSION, ZVERSION);
        put32(&mut b, T_PAGE_SIZE, ZPAGE as u32);
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
        let crc = crc32(&b[..T_CRC]);
        put32(&mut b, T_CRC, crc);
        b
    }

    /// The trailer out of a page, after every check that needs nothing of the
    /// file: the length, the magic, the version, the page size, the CRC, the
    /// constants, the flags, the zeros, `bind32` against the fields it hashes,
    /// and that the fields do not contradict each other. The cheap identifying
    /// checks come first, so a file that is not a sidecar at all is said to
    /// be that, and the CRC says the rest was torn.
    pub fn decode(bytes: &[u8]) -> Result<Trailer, ZError> {
        if bytes.len() != ZTRAILER {
            return Err(ZError::Length);
        }
        if bytes[T_MAGIC..T_MAGIC + 8] != ZMAGIC {
            return Err(ZError::Magic);
        }
        if get32(bytes, T_VERSION) != ZVERSION {
            return Err(ZError::Version);
        }
        if get32(bytes, T_PAGE_SIZE) != ZPAGE as u32 {
            return Err(ZError::PageSize);
        }
        if get32(bytes, T_CRC) != crc32(&bytes[..T_CRC]) {
            return Err(ZError::TrailerCrc);
        }
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

    /// A page built by hand from a group, so the layout's tests do not stand
    /// on `fill_group`.
    fn manual_page(
        group: &[u8],
        count: u16,
        first_rank: u32,
        bind: u32,
        stored: bool,
    ) -> [u8; ZPAGE] {
        let mut page = [0u8; ZPAGE];
        let payload = if stored {
            group.to_vec()
        } else {
            lz4_flex::block::compress(group)
        };
        let h = PageHeader {
            first_rank,
            count,
            stored,
            ulen: group.len() as u32,
            clen: payload.len() as u16,
        };
        encode_page(&mut page, &h, &payload, bind).expect("the fixture page encodes");
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

    // ---- constants ------------------------------------------------------

    #[test]
    fn the_constants_are_the_layouts() {
        assert_eq!(ZCAP, 4072);
        assert_eq!(ZHDR + ZCAP, ZPAGE);
        assert_eq!(&ZTAG.to_le_bytes(), b"ZPG1");
        assert_ne!(ZTAG, 0, "a zeroed page must not carry a valid tag");
        assert_eq!(ZNOPAGE, 0xFFFF_FFFF);
        assert_eq!(ZMAX_COUNT, 256);
        assert_eq!(ZMAX_ULEN, 65_536);
        assert_eq!(&ZMAGIC, b"SUPDZPG1");
        assert_eq!((ZTRAILER, T_CRC, T_ZERO), (4096, 4092, 212));
        // The header's fields tile the first 24 bytes, in the order written.
        assert_eq!(
            [P_CRC, P_TAG, P_FIRST, P_COUNT, P_FLAGS, P_ULEN, P_CLEN],
            [0, 4, 8, 12, 14, 16, 20]
        );
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
            let (h, payload) = parse_page(&page, 0xDEAD_BEEF).expect("a good page parses");
            assert_eq!(h.first_rank, 77);
            assert_eq!(h.count as usize, images.len());
            assert_eq!(h.stored, stored);
            assert_eq!(h.ulen as usize, group.len());
            assert_eq!(payload.len(), h.clen as usize);
            let mut buf = GroupBuf::new();
            let back = decode_group(&h, payload, &mut buf).expect("a good page decodes");
            assert_eq!(back, &group[..]);
            assert!(
                (back.as_ptr() as usize).is_multiple_of(4),
                "a decoded group is aligned for extents"
            );
            walk_group(back, images.len(), None).expect("the group walks");
        }
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
            assert!(payload.len() <= ZCAP, "{n}: the group fits a page");
            let h = PageHeader {
                first_rank: 0,
                count: 1,
                stored: false,
                ulen: group.len() as u32,
                clen: payload.len() as u16,
            };
            let back = decode_group(&h, &payload, &mut buf).unwrap();
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
        let (h, payload) = parse_page(&bytes[..ZPAGE], 5).unwrap();
        let g = decode_group(&h, payload, &mut buf).unwrap();
        assert_eq!(g.as_ptr(), payload.as_ptr(), "aligned: lent from the page");
        assert_eq!(g, &group[..]);
        bytes.copy_within(0..ZPAGE, 1);
        let (h, payload) = parse_page(&bytes[1..1 + ZPAGE], 5).unwrap();
        assert!(!(payload.as_ptr() as usize).is_multiple_of(4));
        let g = decode_group(&h, payload, &mut buf).unwrap();
        assert_ne!(g.as_ptr(), payload.as_ptr(), "misaligned: copied");
        assert!((g.as_ptr() as usize).is_multiple_of(4));
        assert_eq!(g, &group[..]);
        // And the records parse out of it, which is what a misaligned
        // buffer would have turned into "absent".
        walk_group(g, images.len(), None).expect("a copied group walks");
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
            assert_eq!(&page[4..8], b"ZPG1");
            assert_eq!(le32(&page, 8), 1234);
            assert_eq!(
                u16::from_le_bytes([page[12], page[13]]) as usize,
                images.len()
            );
            assert_eq!(page[14], u8::from(stored));
            assert_eq!(page[15], 0);
            assert_eq!(le32(&page, 16) as usize, group.len());
            let clen = u16::from_le_bytes([page[20], page[21]]) as usize;
            assert_eq!(&page[22..24], &[0, 0]);
            if stored {
                assert_eq!(clen, group.len());
                assert_eq!(&page[24..24 + clen], &group[..]);
            } else {
                let mut out = vec![0u8; group.len()];
                let n = lz4_flex::block::decompress_into(&page[24..24 + clen], &mut out).unwrap();
                assert_eq!(&out[..n], &group[..]);
            }
            assert!(
                page[24 + clen..].iter().all(|&b| b == 0),
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
        let clen = u16::from_le_bytes([good[20], good[21]]) as usize;
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
        // The flags, and the bytes that are zero.
        for at in [14usize, 15, 22, 23] {
            let e = refused(edit(&|p| p[at] |= 2), 7);
            assert!(header(e), "byte {at}: {e:?}");
        }
        // The counts and lengths.
        let set16 =
            |at: usize, v: u16| edit(&move |p| p[at..at + 2].copy_from_slice(&v.to_le_bytes()));
        let set32 =
            |at: usize, v: u32| edit(&move |p| p[at..at + 4].copy_from_slice(&v.to_le_bytes()));
        for count in [0u16, 257, u16::MAX] {
            let e = refused(set16(12, count), 7);
            assert!(header(e), "count {count}: {e:?}");
        }
        for ulen in [0u32, 6, group.len() as u32 + 1, 65_540, u32::MAX] {
            let e = refused(set32(16, ulen), 7);
            assert!(header(e), "ulen {ulen}: {e:?}");
        }
        for len in [0u16, 4073, u16::MAX] {
            let e = refused(set16(20, len), 7);
            assert!(header(e), "clen {len}: {e:?}");
        }
        // A compressed page marked stored: its clen is not its ulen.
        let e = refused(edit(&|p| p[14] = 1), 7);
        assert!(header(e), "{e:?}");
        // Bytes after the payload.
        assert_eq!(
            refused(edit(&|p| p[24 + clen] = 1), 7),
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
        // The payload: a ulen the payload does not decode to, either way.
        let (h, payload) = parse_page(&good, 7).unwrap();
        let mut buf = GroupBuf::new();
        let longer = PageHeader {
            ulen: h.ulen + 4,
            ..h
        };
        assert_eq!(
            decode_group(&longer, payload, &mut buf).err(),
            Some(ZError::DecodedLen)
        );
        let shorter = PageHeader {
            ulen: h.ulen - 4,
            ..h
        };
        assert_eq!(
            decode_group(&shorter, payload, &mut buf).err(),
            Some(ZError::Decode)
        );
        // A payload that is not the page's clen bytes is not the page's.
        let e = decode_group(&h, &payload[..payload.len() / 2], &mut buf).err();
        assert!(matches!(e, Some(ZError::Header(_))), "{e:?}");
        // And one cut short with its header corrected is not a block.
        let half = PageHeader {
            clen: (payload.len() / 2) as u16,
            ..h
        };
        let e = decode_group(&half, &payload[..payload.len() / 2], &mut buf).err();
        assert!(
            matches!(e, Some(ZError::Decode | ZError::DecodedLen)),
            "{e:?}"
        );
    }

    #[test]
    fn a_group_is_not_decoded_under_a_header_no_page_may_carry() {
        // `decode_group` is public and takes a header, so it holds the header
        // to what a page may carry itself: an unchecked length past the
        // buffer is a panic, and a stored header whose lengths disagree is a
        // slice of the wrong size.
        let images = small_groups().remove(0).0;
        let good = manual_page(&images.concat(), images.len() as u16, 3, 7, false);
        let (h, payload) = parse_page(&good, 7).unwrap();
        let mut buf = GroupBuf::new();
        assert!(decode_group(&h, payload, &mut buf).is_ok(), "the control");
        for bad in [
            PageHeader {
                ulen: ZMAX_ULEN as u32 + 4,
                ..h
            },
            PageHeader {
                ulen: u32::MAX,
                ..h
            },
            PageHeader {
                ulen: h.ulen + 2,
                ..h
            },
            PageHeader { ulen: 0, ..h },
            PageHeader { count: 0, ..h },
            PageHeader {
                count: ZMAX_COUNT as u16 + 1,
                ..h
            },
            PageHeader { clen: 0, ..h },
            PageHeader {
                clen: ZCAP as u16 + 1,
                ..h
            },
            PageHeader { stored: true, ..h },
        ] {
            let got = decode_group(&bad, payload, &mut buf).err();
            assert!(matches!(got, Some(ZError::Header(_))), "{bad:?}: {got:?}");
        }
    }

    #[test]
    fn a_page_of_another_sidecar_fails_its_own_check() {
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
    fn a_pages_view_is_its_header_and_payload_and_checks_only_those() {
        for (group, stored) in small_groups() {
            let group = group.concat();
            let page = manual_page(&group, 3, 17, 0xBEEF, stored);
            let (h, payload) = parse_page(&page, 0xBEEF).unwrap();
            // The view agrees with the full parse where the page is whole.
            assert_eq!(page_view(&page), Ok((h, payload)));
            // It does not look at the CRC, the tag or the padding: those are
            // the first decode's, and a read of a verified page must not pay
            // for them again. A damaged page's view is the first decode's to
            // refuse, which is why a page is viewed only once it has passed.
            let mut damaged = page;
            damaged[ZPAGE - 1] ^= 0xFF;
            assert_eq!(parse_page(&damaged, 0xBEEF).err(), Some(ZError::PageCrc));
            assert_eq!(page_view(&damaged), Ok((h, payload)));
            // What it does check is what it hands back is bounded: the
            // header's values, and the page's length.
            let mut wild = page;
            wild[P_CLEN..P_CLEN + 2].copy_from_slice(&u16::MAX.to_le_bytes());
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
        // An empty fence key, the first, is at or below every key, the empty
        // one included, and a long key is compared as the engine compares it.
        let mut g = ZFence::new();
        g.push(0, 0, b"").unwrap();
        g.push(5, 1, &[7u8; 40]).unwrap();
        assert_eq!(g.entry_of(b""), Some(0));
        assert_eq!(g.entry_of(&[3u8]), Some(0));
        assert_eq!(g.entry_of(b"a"), Some(1));
        assert_eq!(g.entry_of(&[7u8; 40]), Some(1));
        assert_eq!(g.entry_of(&[7u8; 39]), Some(0));
        assert_eq!(g.entry_of(&[7u8; 41]), Some(1));
        assert_eq!(ZFence::new().entry_of(b"k"), None);
        // The fence's heap: four words an entry, one more for the offsets,
        // and the keys.
        assert_eq!(f.heap_bytes(), 4 * (5 + 5 + 6) + 1 + 1 + 2 + 1 + 2);
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
    fn a_page_is_refused_a_header_it_would_not_write() {
        let mut page = [0xAAu8; ZPAGE];
        let h = PageHeader {
            first_rank: 0,
            count: 1,
            stored: false,
            ulen: 8,
            clen: 4,
        };
        for bad in [
            PageHeader { count: 0, ..h },
            PageHeader { count: 257, ..h },
            PageHeader { ulen: 0, ..h },
            PageHeader { ulen: 6, ..h },
            PageHeader { ulen: 65_540, ..h },
            PageHeader { clen: 0, ..h },
            PageHeader { clen: 4073, ..h },
            PageHeader { stored: true, ..h },
        ] {
            assert!(
                encode_page(&mut page, &bad, &[1, 2, 3, 4], 0).is_err(),
                "{bad:?}"
            );
        }
        // A payload that is not clen bytes.
        assert!(encode_page(&mut page, &h, &[1, 2, 3], 0).is_err());
        assert!(
            page.iter().all(|&b| b == 0xAA),
            "a refused page is left alone"
        );
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
        assert_eq!(&b[0..8], b"SUPDZPG1");
        assert_eq!(le32(&b, 8), 1, "version");
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
        // A version or a page size the CRC covers is refused before it.
        let mut b = base;
        b[8] = 2;
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
            walk_group(g, n, Some(&recs[0].0)).expect("a whole group walks");
            walk_group(g, n, None).expect("and without a fence key");
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
                walk_group(al.bytes(), count, first)
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

    // ---- fill_group -----------------------------------------------------

    struct Built {
        /// (first rank, the group, the page)
        pages: Vec<(usize, Group, [u8; ZPAGE])>,
        declined: Vec<(usize, NoPage)>,
    }

    /// `fill_group` over every record in rank order, the previous page's
    /// count the next call's hint, as a builder drives it; or one hint for
    /// every call.
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
        let (mut rank, mut hint) = (0usize, start);
        let mut out = Built {
            pages: Vec::new(),
            declined: Vec::new(),
        };
        while rank < n {
            let mut page = [0u8; ZPAGE];
            let turns = s.stats.turns;
            match fill_group(src, rank, fixed_hint.unwrap_or(hint), bind, s, &mut page) {
                Fill::Page(g) => {
                    assert!(g.count >= 1 && rank + g.count <= n, "rank {rank}: {g:?}");
                    assert!(
                        g.count <= ZMAX_COUNT && g.ulen <= ZMAX_ULEN && g.clen <= ZCAP,
                        "{g:?}"
                    );
                    out.pages.push((rank, g, page));
                    rank += g.count;
                    hint = g.count;
                }
                Fill::NoPage(why) => {
                    out.declined.push((rank, why));
                    rank += 1;
                }
                other => panic!("rank {rank} of {n}: {other:?}"),
            }
            assert!(
                s.stats.turns - turns <= u64::from(ZFILL_TURNS),
                "rank {rank}: {} turns",
                s.stats.turns - turns
            );
        }
        out
    }

    /// What a first decode must refuse of a page that is not its entry's:
    /// the page against its own entry with one quantity off, and its
    /// neighbours against it. Every page of a sidecar shares `bind32` and
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
        // comparison in the walk can see it.
        let mut keys = vec![[entry.key, &[0u8][..]].concat()];
        if let Some((last, rest)) = entry.key.split_last() {
            keys.push(rest.to_vec());
            keys.push([rest, &[last ^ 1][..]].concat());
        }
        for k in &keys {
            let e = FenceEntry { key: k, ..*entry };
            let got = refused(page, &e);
            assert!(matches!(got, Some(ZError::Walk(_))), "{e:?}: {got:?}");
        }
        // A page of this sidecar at another entry's place: it passes its own
        // CRC under this `bind32`, and only the comparison refuses it.
        for other in neighbours {
            assert!(parse_page(*other, bind).is_ok(), "a good page of the build");
            assert_eq!(refused(other, entry), Some(ZError::Fence), "{entry:?}");
        }
    }

    /// What must hold of a build whatever the records: they are covered
    /// once each by a page or a decline; every page decodes to its records'
    /// images, is stored exactly where LZ4 did not help and the group fits,
    /// and carries the LZ4 block of its group; a record is declined only for
    /// a reason that holds of it alone. With `records`, the images are
    /// records, every page passes the check a reader's first decode makes at
    /// its own fence entry, and is refused at an entry that is not its own.
    fn check_build<I: Images + ?Sized>(src: &I, b: &Built, bind: u32, records: bool) {
        let n = src.records();
        let mut covered = vec![0u32; n];
        let mut buf = GroupBuf::new();
        for (i, (rank, g, page)) in b.pages.iter().enumerate() {
            for c in &mut covered[*rank..*rank + g.count] {
                *c += 1;
            }
            let group: Vec<u8> = (*rank..*rank + g.count)
                .flat_map(|r| src.image(r).iter().copied())
                .collect();
            assert_eq!(group.len(), g.ulen, "rank {rank}");
            let (h, payload) = parse_page(page, bind).expect("a page fill_group built parses");
            assert_eq!(
                (
                    h.first_rank as usize,
                    h.count as usize,
                    h.ulen as usize,
                    h.clen as usize,
                    h.stored
                ),
                (*rank, g.count, g.ulen, g.clen, g.stored)
            );
            assert_eq!(
                decode_group(&h, payload, &mut buf).unwrap(),
                &group[..],
                "rank {rank}"
            );
            let c = lz4_flex::block::compress(&group);
            assert_eq!(
                g.stored,
                c.len() >= g.ulen && g.ulen <= ZCAP,
                "rank {rank}: lz4 {} of {}",
                c.len(),
                g.ulen
            );
            if g.stored {
                assert_eq!(payload, &group[..]);
            } else {
                assert_eq!(
                    payload,
                    &c[..],
                    "the block is the compressor's output for the group"
                );
            }
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
                    im.len() > ZCAP && lz4_flex::block::compress(im).len() > ZCAP,
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

    /// The summary a build's scratch gives.
    fn per_page(s: &FillScratch) -> (f64, f64, f64) {
        let p = s.stats.pages as f64;
        (
            s.stats.records as f64 / p,
            s.stats.payload_bytes as f64 / (p * ZCAP as f64),
            s.stats.calls as f64 / p,
        )
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
            // A page of another sidecar fails its CRC before the entry is
            // looked at, whatever the entry says.
            assert_eq!(
                verify_page(page, bind ^ 1, &entry, &mut buf).err(),
                Some(ZError::PageCrc)
            );
        }
    }

    #[test]
    fn fill_group_fills_pages_with_records_shaped_like_the_suites() {
        let images = suite_images(20_000, 1);
        assert!(
            images.iter().all(|i| i.len() == 124),
            "the suite's record is 124 bytes"
        );
        let mut s = FillScratch::new();
        let b = build_all(&images, &mut s, 99, None);
        check_build(&images, &b, 99, true);
        assert!(b.declined.is_empty());
        assert_eq!(s.stats.pages as usize, b.pages.len());
        assert_eq!(s.stats.records, 20_000);
        assert_eq!(s.stats.stored, 0);
        assert_eq!(s.stats.nopages + s.stats.fallbacks, 0);
        assert_eq!(s.stats.group_bytes, 20_000 * 124);
        let (rpp, fill, calls) = per_page(&s);
        eprintln!("suite shape: {rpp:.2} records a page, fill {fill:.4}, {calls:.2} calls a page");
        // Guard rails, not figures: the form is not worth building under 36
        // records a page, a page left a quarter empty wastes the lever, and a
        // page that takes four compress calls has lost the previous count.
        assert!(rpp >= 36.0, "{rpp} records a page is under the stop rule");
        assert!(fill > 0.9, "fill {fill}");
        assert!(calls < 4.0, "{calls} compress calls a page");
    }

    #[test]
    fn a_stored_page_of_images_that_do_not_compress_holds_thirty_two_of_124_bytes() {
        // 124-byte images that are entropy throughout: LZ4 saves nothing, so
        // the group is stored, 32 of them, 3,968 bytes, parsed in place.
        let mut rng = Rng::new(31);
        let images: Vec<Vec<u8>> = (0..200).map(|_| rng.bytes(124)).collect();
        let mut s = FillScratch::new();
        let b = build_all(&images, &mut s, 1, None);
        check_build(&images, &b, 1, false);
        for (_, g, _) in &b.pages[..b.pages.len() - 1] {
            assert!(g.stored, "{g:?}");
            assert_eq!((g.count, g.ulen, g.clen), (32, 3968, 3968));
        }
        assert_eq!(s.stats.stored, s.stats.pages);
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
    fn fill_group_is_total_on_adversarial_input() {
        let mut rng = Rng::new(41);
        let bind = 5;
        let run = |images: &[Vec<u8>], hint: Option<usize>| {
            let mut s = FillScratch::new();
            let b = build_all(images, &mut s, bind, hint);
            check_build(images, &b, bind, false);
            assert_eq!(s.stats.fallbacks, 0, "the turns suffice");
            (b, s)
        };
        let small = |n: usize| -> Vec<Vec<u8>> { vec![vec![0x55u8; 124]; n] };
        let declined = |b: &Built| -> Vec<(usize, NoPage)> { b.declined.clone() };

        // No records, and a rank past the ones there are.
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let none: Vec<Vec<u8>> = Vec::new();
        assert_eq!(fill_group(&none, 0, 0, bind, &mut s, &mut page), Fill::End);
        let three = small(3);
        for rank in [3usize, 4, 1000, usize::MAX] {
            assert_eq!(
                fill_group(&three, rank, 0, bind, &mut s, &mut page),
                Fill::End,
                "{rank}"
            );
        }
        assert_eq!(s.stats, FillStats::default(), "an end costs nothing");

        // The smallest images: a header's worth, and stored.
        let (b, _) = run(&[vec![1, 2, 3, 4]], None);
        assert!(b.pages[0].1.stored && b.pages[0].1.count == 1);
        let (b, _) = run(&[vec![9; 8]], Some(usize::MAX));
        assert_eq!(b.pages.len(), 1);

        // A record larger than a page that compresses is a page of one; among
        // neighbours it is still one record, and they are pages.
        let mut mixed = small(10);
        mixed.push(vec![b'a'; 5000]);
        mixed.extend(small(10));
        let (b, _) = run(&mixed, None);
        assert!(b.declined.is_empty());
        assert!(b
            .pages
            .iter()
            .any(|(r, g, _)| *r <= 10 && *r + g.count > 10));

        // One that does not compress is read from the raw segment, and only
        // it is.
        let mut mixed = small(10);
        mixed.push(rng.bytes(5000));
        mixed.extend(small(10));
        let (b, _) = run(&mixed, None);
        assert_eq!(declined(&b), vec![(10, NoPage::Incompressible)]);

        // A lone incompressible record that fits a page is stored, to the
        // last word, where its LZ4 output would not.
        for len in (4056..=4072).step_by(4) {
            let (b, _) = run(&[rng.bytes(len)], None);
            assert!(b.declined.is_empty(), "{len} bytes fit a page as they are");
            let (_, g, _) = &b.pages[0];
            assert!(g.stored && g.clen == len && g.count == 1, "{len}: {g:?}");
        }
        for len in [4076usize, 4080, 4084] {
            let (b, _) = run(&[rng.bytes(len)], None);
            assert_eq!(declined(&b), vec![(0, NoPage::Incompressible)], "{len}");
        }

        // The shrink that once reached zero: two records of 4,500
        // incompressible bytes, and seven of 5,100. Every record alone is
        // declined, and nothing divides by a count of none.
        let (b, s) = run(&[rng.bytes(4500), rng.bytes(4500)], Some(2));
        assert_eq!(
            declined(&b),
            vec![(0, NoPage::Incompressible), (1, NoPage::Incompressible)]
        );
        assert!(b.pages.is_empty());
        assert!(s.stats.turns <= 4 * u64::from(ZFILL_TURNS));
        let seven: Vec<Vec<u8>> = (0..7).map(|_| rng.bytes(5100)).collect();
        let (b, _) = run(&seven, Some(7));
        assert_eq!(b.declined.len(), 7);
        let (b, _) = run(&seven, Some(usize::MAX));
        assert_eq!(b.declined.len(), 7);

        // Over the most a group may decode to: declined without a compress
        // call, though it would compress; at the most, a page.
        let (b, s) = run(
            &[small(1).remove(0), vec![b'z'; 65_540], small(1).remove(0)],
            None,
        );
        assert_eq!(declined(&b), vec![(1, NoPage::Oversize)]);
        assert_eq!(b.pages.len(), 2);
        assert!(s.stats.calls <= 6, "{} calls", s.stats.calls);
        let (b, _) = run(&[vec![b'z'; ZMAX_ULEN]], None);
        assert!(b.declined.is_empty() && b.pages[0].1.ulen == ZMAX_ULEN);
        let (b, _) = run(&[rng.bytes(70_000)], None);
        assert_eq!(declined(&b), vec![(0, NoPage::Oversize)]);

        // A group the cap binds: records of 300 identical bytes would hold
        // many more than fit 64 KiB.
        let (b, _) = run(&vec![vec![7u8; 300]; 600], None);
        assert!(b.declined.is_empty());
        let held = ZMAX_ULEN / 300;
        assert!(
            b.pages[..b.pages.len() - 1]
                .iter()
                .all(|(_, g, _)| g.count == held && g.ulen <= ZMAX_ULEN),
            "{:?}",
            b.pages.iter().map(|p| p.1).collect::<Vec<_>>()
        );

        // A group the count binds: identical records are 256 to a page.
        let (b, _) = run(&small(2000), None);
        assert!(b.pages[..b.pages.len() - 1]
            .iter()
            .all(|(_, g, _)| g.count == ZMAX_COUNT));
        assert_eq!(b.pages.iter().map(|p| p.1.count).sum::<usize>(), 2000);

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
            run(&varied, hint);
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
        let (b, _) = run(&bad, None);
        let want = [0usize, 2, 5, 6, 7, 9];
        assert_eq!(
            b.declined,
            want.iter()
                .map(|&r| (r, NoPage::Unplaceable))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_call_with_no_predecessor_builds_a_smaller_page_than_one_that_has_one() {
        let images = suite_images(4000, 8);
        let mut s = FillScratch::new();
        let mut page = [0u8; ZPAGE];
        let Fill::Page(cold) = fill_group(&images, 2000, 0, 1, &mut s, &mut page) else {
            panic!()
        };
        // Chained from there, the counts climb to the steady state.
        let (mut rank, mut hint, mut last) = (2000 + cold.count, cold.count, cold);
        for _ in 0..8 {
            let Fill::Page(g) = fill_group(&images, rank, hint, 1, &mut s, &mut page) else {
                panic!()
            };
            (rank, hint, last) = (rank + g.count, g.count, g);
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
    }

    #[test]
    fn a_group_that_ends_exactly_at_capacity_is_a_page() {
        let mut rng = Rng::new(43);
        let mut s = FillScratch::new();
        let bind = 3;
        // Stored: thirty-one of 124 bytes and one of 228 make 4,072.
        let mut images: Vec<Vec<u8>> = (0..31).map(|_| rng.bytes(124)).collect();
        images.push(rng.bytes(228));
        for hint in [0usize, 1, 32, 100] {
            let mut page = [0u8; ZPAGE];
            let f = fill_group(&images, 0, hint, bind, &mut s, &mut page);
            let Fill::Page(g) = f else { panic!("{f:?}") };
            assert!(
                g.stored && g.count == 32 && g.clen == ZCAP,
                "hint {hint}: {g:?}"
            );
        }
        // One word more is over: the last record leaves the group.
        let mut over = images.clone();
        over[31] = rng.bytes(232);
        let mut page = [0u8; ZPAGE];
        let Fill::Page(g) = fill_group(&over, 0, 32, bind, &mut s, &mut page) else {
            panic!()
        };
        assert!(g.count < 32 && g.clen <= ZCAP, "{g:?}");

        // Compressed: some group of the suite's records and a tail of
        // entropy compresses to exactly a page. Found, not assumed: the
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
                    if c == ZCAP {
                        found = Some((suite[..base].to_vec(), tail));
                        break 'search;
                    }
                    if c > ZCAP {
                        break;
                    }
                }
            }
        }
        let (mut images, tail) = found.expect("some group compresses to exactly a page");
        images.push(tail);
        let mut page = [0u8; ZPAGE];
        let f = fill_group(&images, 0, images.len(), bind, &mut s, &mut page);
        let Fill::Page(g) = f else { panic!("{f:?}") };
        assert!(
            !g.stored && g.clen == ZCAP && g.count == images.len(),
            "{g:?}"
        );
        assert_eq!(parse_page(&page, bind).unwrap().0.clen as usize, ZCAP);
        // And one more word is over, so the group gives a record up.
        let mut longer = images.clone();
        longer.push(vec![0xA5, 0x5A, 0xA5, 0x5A]);
        let mut page2 = [0u8; ZPAGE];
        let f = fill_group(&longer, 0, longer.len(), bind, &mut s, &mut page2);
        let Fill::Page(g2) = f else { panic!("{f:?}") };
        assert!(g2.clen <= ZCAP && g2.count <= images.len() + 1, "{g2:?}");
        // Whatever was built decodes to its records.
        let b = build_all(&longer, &mut s, bind, None);
        check_build(&longer, &b, bind, false);
    }

    #[test]
    fn fill_group_is_deterministic() {
        let images = suite_images(5000, 2);
        let fingerprint = |b: &Built| -> Vec<(usize, Group, u32)> {
            b.pages.iter().map(|(r, g, p)| (*r, *g, crc32(p))).collect()
        };
        let mut fresh = FillScratch::new();
        let a = build_all(&images, &mut fresh, 7, None);
        let want = fingerprint(&a);
        // Again, with a scratch the first build dirtied.
        let b = build_all(&images, &mut fresh, 7, None);
        assert_eq!(fingerprint(&b), want, "a reused scratch");
        // A scratch dirtied by something else entirely: entropy, big records,
        // records that are declined.
        let mut rng = Rng::new(3);
        let mut dirty = FillScratch::new();
        let junk: Vec<Vec<u8>> = (0..400)
            .map(|i| rng.bytes(4 * (1 + (i * 37) % 1500)))
            .chain(std::iter::once(vec![b'q'; 70_000]))
            .collect();
        build_all(&junk, &mut dirty, 1, None);
        let c = build_all(&images, &mut dirty, 7, None);
        assert_eq!(fingerprint(&c), want, "a scratch dirtied by other records");
        // Another thread.
        let moved = std::thread::spawn(move || {
            let mut s = FillScratch::new();
            let d = build_all(&images, &mut s, 7, None);
            fingerprint(&d)
        })
        .join()
        .unwrap();
        assert_eq!(moved, want, "another thread");
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
        // A hint far past what fits, and a budget of one turn: the first
        // group is too big, the turn is spent, and the call falls back to the
        // first record alone.
        for turns in [1u32, 2, 3, 5, 8] {
            let mut s = FillScratch::new();
            let mut rank = 0;
            let mut pages = 0;
            while rank < images.len() {
                let mut page = [0u8; ZPAGE];
                let f = fill_with_turns(&images, rank, 200, 8, &mut s, &mut page, turns);
                let Fill::Page(g) = f else {
                    panic!("turns {turns} rank {rank}: {f:?}")
                };
                // The records are the images', and the page parses.
                let group: Vec<u8> = images[rank..rank + g.count].concat();
                let (h, payload) = parse_page(&page, 8).unwrap();
                let mut buf = GroupBuf::new();
                assert_eq!(decode_group(&h, payload, &mut buf).unwrap(), &group[..]);
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
        // records the other tests build: the arithmetic of the turns says it
        // cannot be.
        let mut s = FillScratch::new();
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
        for hint in [0usize, 1, 7, 255, 256, usize::MAX] {
            build_all(&hard, &mut s, 1, Some(hint));
        }
        build_all(&images, &mut s, 1, None);
        assert_eq!(s.stats.fallbacks, 0);
        assert!(s.stats.turns > 0);
    }

    #[test]
    fn growth_rounds_trade_compress_calls_for_fill() {
        let images = suite_images(30_000, 6);
        let mut rows = Vec::new();
        for rounds in 0..=3u32 {
            let mut s = FillScratch::new();
            s.grow_rounds = rounds;
            let b = build_from(&images, &mut s, 1, None, 60);
            check_build(&images, &b, 1, true);
            let (rpp, fill, calls) = per_page(&s);
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

    /// A group of the suite's records staged and compressed as `fill_group`
    /// leaves it at the moment it makes the page, with the scratch it is in.
    fn fit_of(images: &[Vec<u8>], m: usize) -> (FillScratch, Fit) {
        let mut s = FillScratch::new();
        assert!(matches!(s.stage(images, 0, m), Stage::Ok));
        let ulen = s.imgs.len();
        let Some(Class::Lz4(clen)) = s.classify(ulen, 0) else {
            panic!("{m} of the suite's records compress into a page")
        };
        let body = Body::Lz4 { buf: 0, clen };
        (s, Fit { m, ulen, body })
    }

    #[test]
    fn a_page_that_does_not_decode_to_its_images_is_not_returned() {
        let images = suite_images(60, 15);
        let (bind, want) = (31, images[..30].concat());
        // The page `seal_page` makes of a fit, and what a refused one leaves:
        // no page counted, whatever the compress calls before it cost.
        let seal = |s: &mut FillScratch, fit: Fit| -> Fill {
            let before = s.stats;
            let got = seal_page(s, fit, 0, 1, bind, &mut [0u8; ZPAGE]);
            if got == Fill::Mismatch {
                assert_eq!(s.stats, before, "a refused page is not counted");
            }
            got
        };
        // The control: the group as it was compressed is a page, counted once.
        let (mut s, fit) = fit_of(&images, 30);
        let mut page = [0u8; ZPAGE];
        let Fill::Page(g) = seal_page(&mut s, fit, 0, 1, bind, &mut page) else {
            panic!("the group as compressed is a page")
        };
        assert_eq!((g.count, s.stats.pages, s.stats.records), (30, 1, 30));
        let (h, payload) = parse_page(&page, bind).unwrap();
        let mut buf = GroupBuf::new();
        assert_eq!(decode_group(&h, payload, &mut buf).unwrap(), &want[..]);

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
            s.out[0][at] ^= 0x20;
            let mut back = vec![0u8; fit.ulen];
            let decoded = lz4_flex::block::decompress_into(&s.out[0][..clen], &mut back);
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

        // A header whose length is not the block's, shorter and longer.
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

        // A fit no header can describe, a payload over a page, is no page.
        let (mut s, fit) = fit_of(&images, 30);
        let huge = Fit {
            body: Body::Lz4 {
                buf: 0,
                clen: ZCAP + 1,
            },
            ..fit
        };
        assert_eq!(seal(&mut s, huge), Fill::Mismatch);
    }

    #[test]
    fn a_page_is_compared_as_a_reader_reads_it() {
        let images = small_groups().remove(0).0;
        let (group, bind) = (images.concat(), 5);
        let page = manual_page(&group, images.len() as u16, 9, bind, false);
        let (hdr, _) = parse_page(&page, bind).unwrap();
        let mut v = GroupBuf::new();
        assert!(page_decodes_to(&page, bind, &hdr, &group, &mut v));
        // Not the group it was asked for.
        let mut other = group.clone();
        other[3] ^= 1;
        assert!(!page_decodes_to(&page, bind, &hdr, &other, &mut v));
        assert!(!page_decodes_to(
            &page,
            bind,
            &hdr,
            &group[..group.len() - 4],
            &mut v
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
                !page_decodes_to(&page, bind, &bad, &group, &mut v),
                "{bad:?}"
            );
        }
        // Not a page this sidecar reads: another bind32, or a byte changed.
        assert!(!page_decodes_to(&page, bind ^ 1, &hdr, &group, &mut v));
        let mut torn = page;
        torn[100] ^= 1;
        assert!(!page_decodes_to(&torn, bind, &hdr, &group, &mut v));
        // A page whose CRC is good and whose payload is not the group.
        let mut signed = page;
        signed[ZHDR + 3] ^= 0x40;
        resign_page(&mut signed, bind);
        assert!(parse_page(&signed, bind).is_ok(), "its CRC is good");
        assert!(!page_decodes_to(&signed, bind, &hdr, &group, &mut v));
    }

    #[test]
    fn a_compressor_that_fails_fails_the_build_and_returns_no_page() {
        let images = suite_images(200, 16);
        let mut page = [0u8; ZPAGE];
        let call =
            |s: &mut FillScratch, page: &mut [u8; ZPAGE]| fill_group(&images, 0, 10, 1, s, page);
        // The control: with both buffers whole, the call is a page.
        let mut s = FillScratch::new();
        assert!(matches!(call(&mut s, &mut page), Fill::Page(_)));
        // The first compress of a call, into a buffer too small for the
        // worst case, which `compress_into` refuses whatever the data is.
        let mut s = FillScratch::new();
        s.out[0].truncate(8);
        assert_eq!(call(&mut s, &mut page), Fill::Mismatch);
        assert_eq!(s.stats.pages, 0);
        // A growth's compress: the fit is made in one buffer, the growth in
        // the other.
        let mut s = FillScratch::new();
        s.out[1].truncate(8);
        assert_eq!(call(&mut s, &mut page), Fill::Mismatch);
        assert_eq!(s.stats.pages, 0);
    }

    // ---- a whole sidecar ------------------------------------------------

    #[test]
    fn a_whole_sidecar_round_trips_through_its_file() {
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
        let (mut rank, mut hint) = (0usize, 0usize);
        let (mut file, mut fence) = (Vec::new(), ZFence::new());
        let (mut pages, mut total_ulen) = (0u32, 0u64);
        let mut run: Option<usize> = None;
        while rank < n {
            let mut page = [0u8; ZPAGE];
            match fill_group(&images, rank, hint, bind, &mut s, &mut page) {
                Fill::Page(g) => {
                    if let Some(at) = run.take() {
                        fence.push(at as u32, ZNOPAGE, &recs[at].0).unwrap();
                    }
                    fence.push(rank as u32, pages, &recs[rank].0).unwrap();
                    file.extend_from_slice(&page);
                    pages += 1;
                    total_ulen += g.ulen as u64;
                    rank += g.count;
                    hint = g.count;
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
                let (h, payload) = parse_page(&file[at..at + ZPAGE], t.bind32()).unwrap();
                let g = decode_group(&h, payload, &mut buf).unwrap();
                let p = group_seek(g, usize::from(h.count), &probe).unwrap();
                let at_rank = e.first_rank as usize + p.index;
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
            "BlockedBloom's bits are not the ones this pin was taken from: a sidecar \
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
}
