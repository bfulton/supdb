//! An encrypted object, read through the `Bytes` seam.
//!
//! A supdb file is published and read where the writer does not control the
//! storage: a bucket, a CDN, a copy on somebody's disk. This module wraps a
//! finished file in an envelope that hides its bytes and refuses any change
//! to them, and reads the envelope back as a byte source: `Encrypted` is a
//! `Bytes`, so `Blob::open(Encrypted::open(src, key)?)` and `SparseBlob` read
//! an encrypted object with nothing in the format or the readers changed.
//!
//! This is for a file that is finished and goes somewhere. A live store
//! takes a key another way, `Options::dir_key`: the kernel encrypts its
//! directory, and its reads stay borrows from the page cache.
//!
//! The cipher is the caller's. The crate ships none: `Aead` is the hook, any
//! authenticated cipher with a twelve-byte nonce and a sixteen-byte tag fits
//! it (AES-GCM, ChaCha20-Poly1305), and `examples/crypt.rs` is the adapter
//! for one. The envelope's layout, its nonces and what each tag covers are
//! defined here, because that is the part a caller gets wrong alone.
//!
//! ## The envelope
//!
//! ```text
//! 0      header, 48 bytes     magic, version, chunk shift, plaintext length,
//!                             object id, key id
//! 48     zeroes to 4096
//! 4096   chunk 0              ciphertext of plaintext [0, P), then its tag
//! + S    chunk 1              ciphertext of plaintext [P, 2P), then its tag
//! ...                         the last chunk holds what is left
//! ```
//!
//! `S` is `1 << chunk_shift` stored bytes, a chunk's ciphertext and its tag
//! together, and `P`, the plaintext in a chunk, is `S` less the tag. At the
//! default shift a stored chunk is a page and starts on one, so a read of
//! one chunk faults one page of the stored object, as the same read of the
//! plain file would: laid out the other way, whole pages of plaintext with a
//! tag after each, every chunk would lie across two. A range of plaintext is
//! a range of whole chunks, so a reader that fetches by range fetches
//! `Header::stored_range` of what it was going to read, one contiguous span.
//! An object of no bytes still has one chunk, of no bytes and one tag, so
//! that every envelope carries a tag over its header.
//!
//! Each chunk is encrypted on its own under nonce `id[..8] || chunk index`,
//! with the 48-byte header as associated data. What that refuses:
//!
//! - a changed byte anywhere, in the header or in a chunk (the zeroes after
//!   the header are under no tag, and are checked to be zeroes);
//! - a chunk moved to another position, since the index is in its nonce;
//! - a chunk of another object, since the object's id is in both;
//! - an object cut short or grown, since the plaintext length is in the
//!   header every tag covers, and the stored length must be what the header
//!   implies.
//!
//! What it does not refuse is an older envelope served whole in place of a
//! newer one. Nothing inside one object says which is the newest; a caller
//! that needs that names the object by its version.
//!
//! ## Keys and ids
//!
//! A nonce must never repeat under one key, and the first eight bytes of the
//! object's id are what keep two objects' nonces apart. `encrypt` draws the
//! id from the operating system. At random, one key stays within the usual
//! bound for random nonces (a collision chance of two to the minus
//! thirty-two) for some ninety thousand objects; a caller with more rotates
//! keys, or derives a key per object from the sixteen-byte id, which
//! `Header::read` gives before any key is needed. `key_id` is the caller's
//! word for which key an object is under, stored in the clear and covered by
//! every tag.
//!
//! ## What a read costs
//!
//! By default a reader keeps what it decrypts. A chunk is decrypted at the
//! first read that touches it, into an anonymous mapping the plaintext's
//! length, and from then on it is lent as a mapping lends (`slice_at`): a
//! `Blob` over the reader borrows its index out of it and copies nothing,
//! and a read of a chunk already kept costs what the same read of the plain
//! file costs. The price is the first touch, which runs at the cipher's
//! rate and the kernel's for a fresh page, and the memory: the plaintext
//! touched, up to the whole object, held until the reader drops and not the
//! kernel's to reclaim. The stored object's own pages are, as any mapped
//! file's.
//!
//! `ReadOptions::keep_bytes` bounds that. Past the bound, and at zero, a
//! read decrypts the chunks its range touches and copies out the part
//! asked for, with the thread's last few chunks remembered, so that a walk
//! decrypts each chunk it crosses once: memory stays flat, and a point
//! read of a value in a block costs a chunk or two's decryption, which is
//! why the default chunk is the smallest that is a page. That is the
//! reader for an object larger than memory.
//!
//! The cipher is most of a first touch, and implementations of one cipher
//! differ several times over: `examples/crypt_cost.rs` prints the rate of
//! the one it links and what every read costs beside the same read with no
//! envelope.
//!
//! Plaintext is never handed out unverified: a chunk whose tag does not
//! match is an error, the caller's buffer is zeroed, and the chunk is not
//! kept.

use crate::bytes::{short, Bytes};
use std::cell::Cell;
use std::io::{Error, ErrorKind, Result, Write};
use std::sync::atomic::{AtomicU64, Ordering};

/// The nonce an `Aead` takes, in bytes.
pub const NONCE: usize = 12;
/// The tag an `Aead` makes, in bytes.
pub const TAG: usize = 16;

/// The smallest and the largest chunk a header may name, as shifts: half a
/// kilobyte to sixteen megabytes. The upper bound is what a hostile header
/// can make a reader allocate.
pub const MIN_CHUNK_SHIFT: u32 = 9;
pub const MAX_CHUNK_SHIFT: u32 = 24;
/// The chunk `EncryptOptions::default` writes: four kilobytes stored, a
/// page, so that a read of one chunk faults one page of the stored object
/// and a reader that keeps nothing decrypts no more than a page for a read
/// inside one, at four tenths of a percent of tags.
pub const DEFAULT_CHUNK_SHIFT: u32 = 12;

/// Where the first chunk starts in the stored object. The header is the
/// first `Header::LEN` bytes and zeroes follow it to here, so that with a
/// chunk of a page or more every chunk starts on a page of the file.
pub const DATA_START: u64 = 4096;

/// The piece `encrypt` hands its writer at a time.
const WRITE_PIECE: usize = 1 << 20;

const MAGIC: [u8; 8] = *b"supdbAE\0";
const VERSION: u32 = 1;

fn refused(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, format!("supdb: {msg}"))
}

/// An authenticated cipher, keyed: the caller's.
///
/// One instance is one key. Both calls work in place, and the tag travels
/// beside the buffer rather than inside it.
pub trait Aead {
    /// Encrypt `buf` in place under `nonce`, bind `aad` to it, and return
    /// the tag over both.
    fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG];

    /// Verify `tag` over `aad` and `buf` and decrypt `buf` in place. `false`
    /// when the tag does not match, with `buf` left in any state: the caller
    /// here discards it.
    #[must_use]
    fn decrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8], tag: &[u8; TAG]) -> bool;
}

impl<A: Aead + ?Sized> Aead for &A {
    fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG] {
        (**self).encrypt(nonce, aad, buf)
    }
    fn decrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8], tag: &[u8; TAG]) -> bool {
        (**self).decrypt(nonce, aad, buf, tag)
    }
}

/// An envelope's header: everything about it that is not ciphertext.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    chunk_shift: u32,
    plain_len: u64,
    id: [u8; 16],
    key_id: u64,
}

impl Header {
    /// The header's length in the stored object, and the first bytes a
    /// reader needs of one.
    pub const LEN: usize = 48;

    /// A header for `plain_len` bytes of plaintext in stored chunks of
    /// `1 << chunk_shift`. Refuses a shift outside the bounds and a length
    /// whose chunks a thirty-two-bit index cannot count.
    pub fn new(plain_len: u64, chunk_shift: u32, id: [u8; 16], key_id: u64) -> Result<Header> {
        if !(MIN_CHUNK_SHIFT..=MAX_CHUNK_SHIFT).contains(&chunk_shift) {
            return Err(refused(&format!(
                "an encrypted object's chunk shift is {chunk_shift}, outside \
                 {MIN_CHUNK_SHIFT}..={MAX_CHUNK_SHIFT}"
            )));
        }
        let h = Header {
            chunk_shift,
            plain_len,
            id,
            key_id,
        };
        // The index is the nonce's last four bytes, and the stored length
        // has to be a number.
        let too_long = || {
            refused(&format!(
                "{plain_len} bytes is more than an encrypted object of chunk shift \
                 {chunk_shift} can hold"
            ))
        };
        if h.chunks() > 1 << 32 {
            return Err(too_long());
        }
        DATA_START
            .checked_add(plain_len)
            .and_then(|n| n.checked_add(h.chunks() * TAG as u64))
            .ok_or_else(too_long)?;
        Ok(h)
    }

    /// The header the first `Header::LEN` bytes of a stored object hold.
    pub fn parse(bytes: &[u8]) -> Result<Header> {
        let Some(b) = bytes.get(..Header::LEN) else {
            return Err(refused("too short to be an encrypted object"));
        };
        if b[..8] != MAGIC {
            return Err(refused("not an encrypted object: the magic does not match"));
        }
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
        let version = u32_at(8);
        if version != VERSION {
            return Err(refused(&format!(
                "an encrypted object of version {version}; this reader reads version {VERSION}"
            )));
        }
        Header::new(
            u64_at(16),
            u32_at(12),
            b[24..40].try_into().unwrap(),
            u64_at(40),
        )
    }

    /// The header of the object `src` holds, read without a key: what a
    /// caller that keeps several keys, or derives one per object, reads
    /// first.
    pub fn read<B: Bytes + ?Sized>(src: &B) -> Result<Header> {
        let mut b = [0u8; Header::LEN];
        if src.len() < Header::LEN as u64 {
            return Err(refused("too short to be an encrypted object"));
        }
        src.read_at(0, &mut b)?;
        Header::parse(&b)
    }

    /// The header as it is stored, which is also what every tag covers.
    pub fn encode(&self) -> [u8; Header::LEN] {
        let mut b = [0u8; Header::LEN];
        b[..8].copy_from_slice(&MAGIC);
        b[8..12].copy_from_slice(&VERSION.to_le_bytes());
        b[12..16].copy_from_slice(&self.chunk_shift.to_le_bytes());
        b[16..24].copy_from_slice(&self.plain_len.to_le_bytes());
        b[24..40].copy_from_slice(&self.id);
        b[40..48].copy_from_slice(&self.key_id.to_le_bytes());
        b
    }

    /// The plaintext's length.
    pub fn plain_len(&self) -> u64 {
        self.plain_len
    }

    /// The plaintext bytes a chunk holds, all but the last: the stored
    /// chunk less its tag.
    pub fn chunk_len(&self) -> usize {
        (1 << self.chunk_shift) - TAG
    }

    /// A chunk's bytes in the stored object, ciphertext and tag: all but
    /// the last chunk's.
    pub fn stored_chunk(&self) -> usize {
        1 << self.chunk_shift
    }

    pub fn chunk_shift(&self) -> u32 {
        self.chunk_shift
    }

    /// The object's id: random, unique to this envelope, and the salt for a
    /// caller that derives a key per object.
    pub fn id(&self) -> &[u8; 16] {
        &self.id
    }

    /// The caller's word for which key the object is under.
    pub fn key_id(&self) -> u64 {
        self.key_id
    }

    /// How many chunks the object has: one at least.
    pub fn chunks(&self) -> u64 {
        self.plain_len.div_ceil(self.chunk_len() as u64).max(1)
    }

    /// The stored object's length: the header's page, the plaintext's bytes
    /// and a tag a chunk.
    pub fn stored_len(&self) -> u64 {
        DATA_START + self.plain_len + self.chunks() * TAG as u64
    }

    /// How many leading bytes of the stored object `Encrypted::open` reads:
    /// the header's page and the first chunk.
    pub fn open_probe(&self) -> u64 {
        self.stored_off(0) + (self.chunk_plain_len(0) + TAG) as u64
    }

    /// The stored bytes a read of `len` plaintext bytes at `off` touches, as
    /// an offset and a length: the whole chunks the range lies in, tags
    /// included. Nothing for an empty range or one past the plaintext.
    pub fn stored_range(&self, off: u64, len: u64) -> (u64, u64) {
        let Some(end) = off.checked_add(len).filter(|&e| e <= self.plain_len) else {
            return (0, 0);
        };
        if len == 0 {
            return (0, 0);
        }
        let per = self.chunk_len() as u64;
        let (first, last) = (off / per, (end - 1) / per);
        let lo = self.stored_off(first);
        let hi = self.stored_off(last) + (self.chunk_plain_len(last) + TAG) as u64;
        (lo, hi - lo)
    }

    /// Where a chunk's ciphertext starts in the stored object.
    fn stored_off(&self, chunk: u64) -> u64 {
        DATA_START + (chunk << self.chunk_shift)
    }

    /// Where a chunk's plaintext starts in the plaintext.
    fn plain_off(&self, chunk: u64) -> u64 {
        chunk * self.chunk_len() as u64
    }

    /// A chunk's plaintext length: the chunk size, or what is left.
    fn chunk_plain_len(&self, chunk: u64) -> usize {
        (self.plain_len - self.plain_off(chunk)).min(self.chunk_len() as u64) as usize
    }

    /// A chunk's nonce: the id's first eight bytes, then the chunk's index.
    fn nonce(&self, chunk: u64) -> [u8; NONCE] {
        let mut n = [0u8; NONCE];
        n[..8].copy_from_slice(&self.id[..8]);
        // `new` held the chunk count to what four bytes index.
        n[8..].copy_from_slice(&(chunk as u32).to_le_bytes());
        n
    }
}

/// How `encrypt` lays an envelope out.
#[derive(Clone, Copy, Debug)]
pub struct EncryptOptions {
    /// Plaintext bytes a chunk, as a shift. A smaller chunk is less to
    /// decrypt for a small read and more tags.
    pub chunk_shift: u32,
    /// The caller's word for the key in use, stored in the header.
    pub key_id: u64,
}

impl Default for EncryptOptions {
    fn default() -> Self {
        EncryptOptions {
            chunk_shift: DEFAULT_CHUNK_SHIFT,
            key_id: 0,
        }
    }
}

/// Write `src` to `out` as an encrypted object under `aead`, with an id
/// drawn from the operating system, and answer the header written.
///
/// `out` receives the envelope from its first byte to its last, in order.
/// Publishing it is the caller's: written under a temporary name and renamed
/// into place, as any object a reader may already hold open is replaced.
#[cfg(unix)]
pub fn encrypt<S, W, A>(src: &S, out: &mut W, aead: &A, opts: &EncryptOptions) -> Result<Header>
where
    S: Bytes + ?Sized,
    W: Write + ?Sized,
    A: Aead + ?Sized,
{
    use std::io::Read;
    let mut id = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut id)?;
    encrypt_with_id(src, out, aead, opts, id)
}

/// `encrypt` with the id given: for a target with no random source of its
/// own, and for a test that needs the same bytes twice.
///
/// An id must never be used for two objects under one key: the nonces are
/// drawn from it, and a repeated nonce gives both objects away.
pub fn encrypt_with_id<S, W, A>(
    src: &S,
    out: &mut W,
    aead: &A,
    opts: &EncryptOptions,
    id: [u8; 16],
) -> Result<Header>
where
    S: Bytes + ?Sized,
    W: Write + ?Sized,
    A: Aead + ?Sized,
{
    let hdr = Header::new(src.len(), opts.chunk_shift, id, opts.key_id)?;
    let aad = hdr.encode();
    // Written a megabyte at a time, each piece at a megabyte's offset in a
    // file written from its start: the page cache sizes its folios by the
    // write that makes them, and the envelope is read far more than it is
    // written.
    let mut buf = Vec::with_capacity(WRITE_PIECE + hdr.stored_chunk());
    buf.extend_from_slice(&aad);
    buf.resize(DATA_START as usize, 0);
    for c in 0..hdr.chunks() {
        let (at, n) = (buf.len(), hdr.chunk_plain_len(c));
        buf.resize(at + n, 0);
        src.read_at(hdr.plain_off(c), &mut buf[at..])?;
        let tag = aead.encrypt(&hdr.nonce(c), &aad, &mut buf[at..]);
        buf.extend_from_slice(&tag);
        let whole = buf.len() / WRITE_PIECE * WRITE_PIECE;
        for piece in buf[..whole].chunks(WRITE_PIECE) {
            out.write_all(piece)?;
        }
        buf.drain(..whole);
    }
    out.write_all(&buf)?;
    Ok(hdr)
}

/// The thread's buffers for the chunks a reader does not keep: a chunk's
/// stored bytes, read whole from a source that cannot lend them, and the
/// last few chunks decrypted, each with whose it is. The thread's rather
/// than the reader's, since a reader is shared between threads, and `Cell`s
/// so that a reader over a reader finds an empty buffer rather than a
/// borrow in use.
struct Scratch {
    stored: Cell<Vec<u8>>,
    memo: Cell<Vec<Remembered>>,
}

/// A decrypted chunk the thread remembers: which reader's, which chunk,
/// and its plaintext. No reader's while it is being filled or was refused.
#[derive(Default)]
struct Remembered {
    of: Option<(u64, u64)>,
    plain: Vec<u8>,
}

/// The most chunks a thread remembers, and the most plaintext they may come
/// to: a chunk larger than the second is remembered alone.
///
/// One was not enough. A value in a block is read as the piece of the block
/// it lies in, and that piece lies across two chunks more often than in
/// one, so a walk of a block's values asked for the same two chunks turn
/// about and decrypted both at every value: a scan through a reader that
/// keeps nothing ran at a twenty-sixth of the rate of one that keeps
/// (`supdb-segmentblocks-enc` against `-encbounded` in the suite). A chunk
/// takes the place its index names among these, so a walk's two chunks have
/// a place each and the chunks behind it are put out as it moves on.
const REMEMBERED: usize = 8;
const REMEMBERED_BYTES: usize = 64 << 10;

thread_local! {
    static SCRATCH: Scratch = const {
        Scratch {
            stored: Cell::new(Vec::new()),
            memo: Cell::new(Vec::new()),
        }
    };
}

/// Reader ids, one per open.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// How a reader holds what it decrypts.
#[derive(Clone, Copy, Debug)]
pub struct ReadOptions {
    /// The most plaintext the reader keeps, in bytes. A chunk is decrypted
    /// at the first read that touches it and kept, until this much is held;
    /// a kept chunk is lent as a mapping lends, and never decrypted again.
    /// Past the bound, and at zero, a read decrypts what it touches each
    /// time and the reader's memory does not grow.
    ///
    /// Without a bound, the default, a reader comes to hold as much
    /// plaintext as its reads have touched, up to the whole object, until
    /// it is dropped. Nothing is kept on a wasm target, whatever this says.
    pub keep_bytes: u64,
    /// Ask the kernel for the fresh pages of a long range in one call
    /// (`MADV_POPULATE_WRITE`, Linux) before its chunks are decrypted into
    /// them, where each chunk's first write would otherwise fault its page
    /// in alone. On by default: with a fast cipher the pages are most of a
    /// first touch, and it took a quarter off the open of a `Blob` over an
    /// envelope, which is its index sections' first touch, with no read
    /// after it moved (`examples/crypt_cost.rs`, `kept+populate`). Only a
    /// range of several chunks is asked for, and no more of it than the
    /// reader has room to keep, so a read of one chunk and a reader at its
    /// bound take no page they will not write.
    pub populate: bool,
    /// REFUTED here, and off: huge pages under the kept plaintext
    /// (`MADV_HUGEPAGE`, Linux), one fault for two megabytes. The open and
    /// the first pass of point reads both took four tenths longer with it
    /// (`kept+huge` in the same probe), the kernel zeroing two megabytes
    /// for a chunk of four kilobytes, and memory is then taken two
    /// megabytes at a time whatever `keep_bytes` counts. Kept as the arm
    /// for a host that prices a page otherwise.
    pub huge_pages: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            keep_bytes: u64::MAX,
            populate: true,
            huge_pages: false,
        }
    }
}

/// A chunk's place in the kept plaintext: nothing there, one thread filling
/// it, or filled for good.
#[cfg(not(target_family = "wasm"))]
const EMPTY: u8 = 0;
#[cfg(not(target_family = "wasm"))]
const BUSY: u8 = 1;
#[cfg(not(target_family = "wasm"))]
const READY: u8 = 2;

/// How many ranges of kept chunks a reader remembers whole, and the fewest
/// chunks worth remembering: a shorter range is as cheap to walk.
#[cfg(not(target_family = "wasm"))]
const KNOWN: usize = 4;
#[cfg(not(target_family = "wasm"))]
const KNOWN_MIN: u64 = 8;
#[cfg(not(target_family = "wasm"))]
const UNKNOWN: u64 = u64::MAX;

/// The plaintext a reader keeps: an anonymous mapping the object's length,
/// which costs nothing until a chunk is written into it, and each chunk's
/// state.
///
/// A chunk is written once, by the one thread that moved it from `EMPTY` to
/// `BUSY`, and is `READY` only after; nothing is lent or copied out of a
/// chunk that is not `READY`, and a `READY` chunk is never written again.
/// That is the whole of what makes lending from it sound.
#[cfg(not(target_family = "wasm"))]
struct Kept {
    map: memmap2::MmapRaw,
    state: Box<[std::sync::atomic::AtomicU8]>,
    /// Plaintext bytes held, and the most that may be.
    bytes: AtomicU64,
    limit: u64,
    /// `ReadOptions::populate`.
    populate: bool,
    /// Chunks `READY`, and whether that is all of them.
    ready: AtomicU64,
    whole: std::sync::atomic::AtomicBool,
    /// Ranges of chunks known `READY`, first and last packed in a word: a
    /// section a `Blob` borrows is asked for again at every lookup, and a
    /// walk of its chunks' states each time would cost what the borrow
    /// saves.
    known: [AtomicU64; KNOWN],
    /// For the tests: chunk states looked at by a borrow, chunks claimed for
    /// a fill, and the look at the bound before a claim switched off.
    #[cfg(test)]
    walked: AtomicU64,
    #[cfg(test)]
    claims: AtomicU64,
    #[cfg(test)]
    no_look: std::sync::atomic::AtomicBool,
}

#[cfg(not(target_family = "wasm"))]
impl Kept {
    fn new(hdr: &Header, opts: &ReadOptions) -> Option<Kept> {
        let (limit, len) = (opts.keep_bytes, usize::try_from(hdr.plain_len).ok()?);
        if limit == 0 || len == 0 {
            return None;
        }
        // A mapping the kernel will not make, an object past what it will
        // promise, is a reader that keeps nothing and not one that fails.
        let map: memmap2::MmapRaw = memmap2::MmapOptions::new().len(len).map_anon().ok()?.into();
        #[cfg(target_os = "linux")]
        if opts.huge_pages {
            // SAFETY: advice over the whole of a mapping this value owns;
            // it changes how the kernel backs the pages and no byte of one.
            unsafe { libc::madvise(map.as_mut_ptr().cast(), len, libc::MADV_HUGEPAGE) };
        }
        Some(Kept {
            map,
            populate: opts.populate,
            state: (0..hdr.chunks())
                .map(|_| std::sync::atomic::AtomicU8::new(EMPTY))
                .collect(),
            bytes: AtomicU64::new(0),
            limit,
            ready: AtomicU64::new(0),
            whole: std::sync::atomic::AtomicBool::new(false),
            known: std::array::from_fn(|_| AtomicU64::new(UNKNOWN)),
            #[cfg(test)]
            walked: AtomicU64::new(0),
            #[cfg(test)]
            claims: AtomicU64::new(0),
            #[cfg(test)]
            no_look: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Whether every chunk from `first` to `last` is known kept, without a
    /// look at any of them.
    fn knows(&self, first: u64, last: u64) -> bool {
        // An empty place reads as the one chunk at the top of what an index
        // counts, which no range asked about here lies in.
        self.known.iter().any(|k| {
            let k = k.load(Ordering::Acquire);
            k >> 32 <= first && last <= k & 0xffff_ffff
        })
    }

    /// Remember that every chunk from `first` to `last` is kept, in place of
    /// the shortest range remembered if this one is longer: the long ranges
    /// are the sections, and a run of values must not put one out.
    fn note(&self, first: u64, last: u64) {
        let chunks = last - first + 1;
        if last >= u64::from(u32::MAX) {
            return;
        }
        let span = |k: u64| match k {
            UNKNOWN => 0,
            k => (k & 0xffff_ffff) - (k >> 32) + 1,
        };
        let shortest = self
            .known
            .iter()
            .min_by_key(|k| span(k.load(Ordering::Relaxed)))
            .expect("a range to replace");
        if span(shortest.load(Ordering::Relaxed)) < chunks {
            shortest.store(first << 32 | last, Ordering::Release);
        }
    }

    /// The pages under plaintext `from..to` made present in one call, so
    /// that the chunks about to be decrypted into them fault on none.
    fn populate(&self, from: u64, to: u64) {
        #[cfg(target_os = "linux")]
        if from < to {
            let page = crate::bytes::os_page_size();
            let (a, b) = (from as usize / page * page, to as usize);
            // SAFETY: a range inside this mapping, which lives as long as
            // `self`. The advice faults the pages in as a write would and
            // writes nothing: a chunk already kept reads the same after.
            unsafe {
                libc::madvise(
                    self.map.as_mut_ptr().add(a).cast(),
                    b - a,
                    libc::MADV_POPULATE_WRITE,
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (from, to);
    }

    /// `len` kept bytes at `off`.
    ///
    /// # Safety
    ///
    /// Every chunk the range lies in is `READY`, as an `Acquire` load of
    /// its state or of a range in `known` said.
    unsafe fn lend(&self, off: u64, len: usize) -> &[u8] {
        // SAFETY: the range is inside the mapping, which lives as long as
        // `self`; its chunks are `READY`, so nothing writes them again.
        unsafe { std::slice::from_raw_parts(self.map.as_ptr().add(off as usize), len) }
    }
}

/// A chunk being filled: back to `EMPTY`, with its bytes given back to the
/// bound, unless the fill ends well. A cipher that panics must not leave a
/// chunk every other thread waits on.
#[cfg(not(target_family = "wasm"))]
struct Filling<'a> {
    kept: &'a Kept,
    chunk: usize,
    bytes: u64,
}

#[cfg(not(target_family = "wasm"))]
impl Drop for Filling<'_> {
    fn drop(&mut self) {
        self.kept.bytes.fetch_sub(self.bytes, Ordering::Relaxed);
        self.kept.state[self.chunk].store(EMPTY, Ordering::Release);
    }
}

/// Division by a chunk's plaintext length, which the tag leaves short of a
/// power of two, as a multiply and a shift: a read finds its chunk at every
/// call, and a divide there is a tenth of a lookup.
///
/// For a divisor `d` of at most `2^l` and any `n` below `2^56`, with
/// `k = 56 + l` and `m = floor(2^k / d) + 1`, `floor(n * m / 2^k)` is
/// `floor(n / d)`: `m * d` overshoots `2^k` by at most `d`, so the product
/// overshoots `n / d` by at most `n / 2^k`, which is under `1 / d` since
/// `n * d < 2^k`, and that much cannot carry a quotient to the next whole
/// number. A header's chunk count and chunk length keep every offset under
/// `2^56`.
#[derive(Clone, Copy)]
struct Per {
    d: u64,
    m: u64,
    k: u32,
}

impl Per {
    fn new(d: u64) -> Per {
        let k = 56 + (64 - (d - 1).leading_zeros());
        Per {
            d,
            m: ((1u128 << k) / u128::from(d)) as u64 + 1,
            k,
        }
    }

    /// `n / d` and `n % d`, for `n` below `2^56`.
    #[inline]
    fn of(&self, n: u64) -> (u64, usize) {
        let q = ((u128::from(n) * u128::from(self.m)) >> self.k) as u64;
        (q, (n - q * self.d) as usize)
    }
}

/// An encrypted object read as its plaintext: a `Bytes` over the stored
/// object `src`, under the key `aead` holds.
pub struct Encrypted<B: Bytes, A: Aead> {
    src: B,
    aead: A,
    hdr: Header,
    aad: [u8; Header::LEN],
    per: Per,
    id: u64,
    #[cfg(not(target_family = "wasm"))]
    kept: Option<Kept>,
}

impl<B: Bytes, A: Aead> Encrypted<B, A> {
    /// Open the object `src` holds, keeping every chunk it decrypts
    /// (`ReadOptions::default`).
    pub fn open(src: B, aead: A) -> Result<Encrypted<B, A>> {
        Encrypted::open_with(src, aead, &ReadOptions::default())
    }

    /// Open the object `src` holds. Reads the header and the first chunk
    /// (`Header::open_probe` bytes) and verifies that chunk, so a wrong key,
    /// a changed header and a length that is not the header's are refused
    /// here and not at some later read.
    pub fn open_with(src: B, aead: A, opts: &ReadOptions) -> Result<Encrypted<B, A>> {
        // The header's page in one read: the header, and the zeroes after
        // it, which no tag covers and a reader therefore looks at.
        let mut head = [0u8; DATA_START as usize];
        if src.len() < DATA_START {
            return Err(refused("too short to be an encrypted object"));
        }
        src.read_at(0, &mut head)?;
        let hdr = Header::parse(&head)?;
        if head[Header::LEN..].iter().any(|&b| b != 0) {
            return Err(refused(
                "an encrypted object with something other than zeroes after its header",
            ));
        }
        if src.len() != hdr.stored_len() {
            return Err(refused(&format!(
                "an encrypted object of {} bytes whose header describes {}: cut short, or grown",
                src.len(),
                hdr.stored_len()
            )));
        }
        #[cfg(target_family = "wasm")]
        let _ = opts;
        let e = Encrypted {
            aad: hdr.encode(),
            per: Per::new(hdr.chunk_len() as u64),
            #[cfg(not(target_family = "wasm"))]
            kept: Kept::new(&hdr, opts),
            src,
            aead,
            hdr,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        };
        if !e.keep(0)? {
            e.with_chunk(0, |_| ())?;
        }
        Ok(e)
    }

    pub fn header(&self) -> &Header {
        &self.hdr
    }

    /// The stored object this reads through.
    pub fn source(&self) -> &B {
        &self.src
    }

    /// The plaintext the reader holds now, in bytes.
    pub fn kept_bytes(&self) -> u64 {
        #[cfg(not(target_family = "wasm"))]
        if let Some(k) = &self.kept {
            return k.bytes.load(Ordering::Relaxed);
        }
        0
    }

    /// Read chunk `c` from the source into `dst`, which is the chunk's
    /// length, and decrypt it there: the chunk's plaintext, or zeroes and an
    /// error.
    fn open_chunk(&self, c: u64, dst: &mut [u8]) -> Result<()> {
        let (n, at) = (dst.len(), self.hdr.stored_off(c));
        let mut tag = [0u8; TAG];
        let mut take = |stored: &[u8]| {
            dst.copy_from_slice(&stored[..n]);
            tag.copy_from_slice(&stored[n..]);
        };
        match self.src.slice_at(at, n + TAG) {
            Some(stored) => take(stored),
            // One read for the chunk and its tag, and a copy: a source that
            // cannot lend is one whose reads are the cost.
            None => SCRATCH.with(|s| {
                let mut buf = s.stored.take();
                buf.clear();
                buf.resize(n + TAG, 0);
                let r = self.src.read_at(at, &mut buf).map(|()| take(&buf));
                s.stored.set(buf);
                r
            })?,
        }
        if !self.aead.decrypt(&self.hdr.nonce(c), &self.aad, dst, &tag) {
            // Whatever the cipher left there is not plaintext anyone may see.
            dst.fill(0);
            return Err(refused(&format!(
                "chunk {c} of an encrypted object failed its check: the key is wrong, or the \
                 object was changed"
            )));
        }
        Ok(())
    }

    /// Chunk `c`'s plaintext for a reader that does not keep it: decrypted
    /// into the place the thread remembers it in, unless it is there
    /// already.
    fn with_chunk<R>(&self, c: u64, f: impl FnOnce(&[u8]) -> R) -> Result<R> {
        SCRATCH.with(|s| {
            let mut memo = s.memo.take();
            if memo.len() < REMEMBERED {
                memo.resize_with(REMEMBERED, Remembered::default);
            }
            let places = (REMEMBERED_BYTES / self.hdr.chunk_len()).clamp(1, REMEMBERED);
            let slot = &mut memo[(c % places as u64) as usize];
            // Nobody's while it is being filled: an error below leaves it so.
            let held = slot.of.take() == Some((self.id, c));
            let r = if held {
                Ok(())
            } else {
                slot.plain.clear();
                slot.plain.resize(self.hdr.chunk_plain_len(c), 0);
                self.open_chunk(c, &mut slot.plain)
                    .inspect_err(|_| slot.plain.clear())
            };
            let r = r.map(|()| {
                slot.of = Some((self.id, c));
                f(&slot.plain)
            });
            s.memo.set(memo);
            r
        })
    }

    /// Whether chunk `c` is kept, decrypting it into its place if the reader
    /// keeps chunks, has room for this one and nobody has yet. `false` is a
    /// chunk the caller reads the other way.
    #[cfg(not(target_family = "wasm"))]
    fn keep(&self, c: u64) -> Result<bool> {
        let Some(k) = &self.kept else {
            return Ok(false);
        };
        let state = &k.state[c as usize];
        let n = self.hdr.chunk_plain_len(c);
        loop {
            match state.load(Ordering::Acquire) {
                READY => return Ok(true),
                EMPTY => {
                    // A look before the claim, so that a reader at its bound
                    // does not take and give back every chunk it reads.
                    #[cfg(test)]
                    let looks = !k.no_look.load(Ordering::Relaxed);
                    #[cfg(not(test))]
                    let looks = true;
                    if looks && k.bytes.load(Ordering::Relaxed).saturating_add(n as u64) > k.limit {
                        return Ok(false);
                    }
                    if state
                        .compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Relaxed)
                        .is_err()
                    {
                        continue;
                    }
                    #[cfg(test)]
                    k.claims.fetch_add(1, Ordering::Relaxed);
                    let filling = Filling {
                        kept: k,
                        chunk: c as usize,
                        bytes: n as u64,
                    };
                    if k.bytes.fetch_add(n as u64, Ordering::Relaxed) + n as u64 > k.limit {
                        return Ok(false);
                    }
                    // SAFETY: this thread moved the chunk from `EMPTY` to
                    // `BUSY`, so no other writes it and none reads it until
                    // it is `READY`; its bytes are inside the mapping.
                    let dst = unsafe {
                        std::slice::from_raw_parts_mut(
                            k.map.as_mut_ptr().add(self.hdr.plain_off(c) as usize),
                            n,
                        )
                    };
                    self.open_chunk(c, dst)?;
                    std::mem::forget(filling);
                    state.store(READY, Ordering::Release);
                    // `AcqRel`: each count takes the one before it, so whoever
                    // reads `whole` has every fill behind it.
                    if k.ready.fetch_add(1, Ordering::AcqRel) + 1 == k.state.len() as u64 {
                        k.whole.store(true, Ordering::Release);
                    }
                    return Ok(true);
                }
                // Another thread is decrypting it: microseconds.
                _ => std::thread::yield_now(),
            }
        }
    }

    #[cfg(target_family = "wasm")]
    fn keep(&self, _c: u64) -> Result<bool> {
        Ok(false)
    }
}

impl<B: Bytes, A: Aead> Bytes for Encrypted<B, A> {
    fn len(&self) -> u64 {
        self.hdr.plain_len
    }

    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
        let end = off
            .checked_add(dst.len() as u64)
            .filter(|&e| e <= self.hdr.plain_len)
            .ok_or_else(|| short(off, dst.len(), self.hdr.plain_len))?;
        let (mut at, mut done) = (off, 0usize);
        while at < end {
            let (c, lo) = self.per.of(at);
            let whole = self.hdr.chunk_plain_len(c);
            let n = (whole - lo).min(dst.len() - done);
            let part = &mut dst[done..done + n];
            let r = match self.keep(c) {
                #[cfg(not(target_family = "wasm"))]
                Ok(true) => {
                    let k = self.kept.as_ref().expect("a kept chunk");
                    // SAFETY: `keep` answered that the chunk is `READY`.
                    part.copy_from_slice(unsafe { k.lend(at, n) });
                    Ok(())
                }
                #[cfg(target_family = "wasm")]
                Ok(true) => unreachable!("nothing is kept here"),
                // A whole chunk goes straight into the caller's buffer.
                Ok(false) if n == whole => self.open_chunk(c, part),
                Ok(false) => self.with_chunk(c, |plain| part.copy_from_slice(&plain[lo..lo + n])),
                Err(e) => Err(e),
            };
            if let Err(e) = r {
                // The chunks before this one verified, and a caller that
                // ignores the error must not find a partial answer.
                dst.fill(0);
                return Err(e);
            }
            at += n as u64;
            done += n;
        }
        Ok(())
    }

    /// Lent where every chunk of the range is kept, each decrypted here if
    /// it was not yet. `None` for a reader that keeps nothing, for a range
    /// past what it may keep, and for a chunk that fails its check: the
    /// `read_at` a caller falls back to says which.
    #[cfg(not(target_family = "wasm"))]
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let k = self.kept.as_ref()?;
        let end = off
            .checked_add(len as u64)
            .filter(|&e| e <= self.hdr.plain_len)?;
        if len == 0 {
            return Some(&[]);
        }
        // With every chunk kept, which is where a reader of a whole object
        // ends up, a borrow is this one load and no arithmetic.
        if !k.whole.load(Ordering::Acquire) {
            let (first, last) = (self.per.of(off).0, self.per.of(end - 1).0);
            // A short range is as cheap to walk as to look up, and is
            // neither looked up nor remembered.
            let long = last - first + 1 >= KNOWN_MIN;
            if !(long && k.knows(first, last)) {
                // The pages from the first chunk not yet kept to the
                // range's end, in one call, once.
                let mut fresh = long && k.populate;
                for c in first..=last {
                    #[cfg(test)]
                    k.walked.fetch_add(1, Ordering::Relaxed);
                    if k.state[c as usize].load(Ordering::Acquire) != READY {
                        if std::mem::take(&mut fresh) {
                            // No further than the reader has room to keep.
                            let from = self.hdr.plain_off(c);
                            let room = k.limit.saturating_sub(k.bytes.load(Ordering::Relaxed));
                            k.populate(from, end.min(from.saturating_add(room)));
                        }
                        if !self.keep(c).ok()? {
                            return None;
                        }
                    }
                }
                if long {
                    k.note(first, last);
                }
            }
        }
        // SAFETY: every chunk of the range is `READY`: its state said so,
        // or `knows` of the range, or `whole` of them all.
        Some(unsafe { k.lend(off, len) })
    }

    // The advice that names no offset goes to the stored object, whose
    // pages are the ones a read faults; the calls that name one are left as
    // a source with no mapping answers them.
    fn advise_random(&self) {
        self.src.advise_random();
    }
    fn advise_normal(&self) {
        self.src.advise_normal();
    }
    fn advise_dontneed(&self) {
        self.src.advise_dontneed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::SliceBytes;
    use std::sync::atomic::AtomicU64;

    /// A stand-in cipher, for the envelope's own arithmetic: a keystream and
    /// a tag that are each a function of the key, the nonce, the associated
    /// data and the bytes, so that a tag made under one of them fails under
    /// any other. It counts what it decrypts, and panics at a call it is
    /// told to. Not a cipher anyone may use.
    struct Toy {
        key: u64,
        decrypts: AtomicU64,
        panic_at: AtomicU64,
    }

    impl Toy {
        fn new(key: u64) -> Toy {
            Toy {
                key,
                decrypts: AtomicU64::new(0),
                panic_at: AtomicU64::new(u64::MAX),
            }
        }
        fn stream(&self, nonce: &[u8; NONCE], buf: &mut [u8]) {
            let mut s = mix(self.key | 1, nonce);
            for b in buf {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *b ^= s as u8;
            }
        }
        fn tag(&self, nonce: &[u8; NONCE], aad: &[u8], body: &[u8]) -> [u8; TAG] {
            let a = mix(mix(mix(self.key, nonce), aad), body);
            let b = mix(a, &(aad.len() as u64).to_le_bytes());
            let mut t = [0u8; TAG];
            t[..8].copy_from_slice(&a.to_le_bytes());
            t[8..].copy_from_slice(&b.to_le_bytes());
            t
        }
    }

    fn mix(mut h: u64, bytes: &[u8]) -> u64 {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h ^ (h >> 29)
    }

    impl Aead for Toy {
        fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG] {
            self.stream(nonce, buf);
            self.tag(nonce, aad, buf)
        }
        fn decrypt(
            &self,
            nonce: &[u8; NONCE],
            aad: &[u8],
            buf: &mut [u8],
            tag: &[u8; TAG],
        ) -> bool {
            let call = self.decrypts.fetch_add(1, Ordering::Relaxed);
            assert!(
                call != self.panic_at.load(Ordering::Relaxed),
                "the cipher was told to"
            );
            let ok = self.tag(nonce, aad, buf) == *tag;
            // Decrypts whatever the verdict, as the trait allows: the reader
            // is what keeps a failed chunk's bytes from the caller.
            self.stream(nonce, buf);
            ok
        }
    }

    /// The geometry the tests are written in, a chunk shift of nine: a
    /// stored chunk, the plaintext in one, and where the chunks start.
    const S: usize = 512;
    const C: usize = S - TAG;
    const D: usize = DATA_START as usize;

    /// The ways a reader holds what it decrypts: all of it, none of it, and
    /// two chunks with a little over.
    const KEEPS: [u64; 3] = [u64::MAX, 0, 2 * C as u64 + 108];

    fn open<B: Bytes>(src: B, key: u64, keep: u64) -> Result<Encrypted<B, Toy>> {
        let opts = ReadOptions {
            keep_bytes: keep,
            ..ReadOptions::default()
        };
        Encrypted::open_with(src, Toy::new(key), &opts)
    }

    fn plain(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    fn envelope(p: &[u8], shift: u32, id: u8, key: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let opts = EncryptOptions {
            chunk_shift: shift,
            key_id: 9,
        };
        encrypt_with_id(&SliceBytes(p), &mut out, &Toy::new(key), &opts, [id; 16]).unwrap();
        out
    }

    fn read_whole<B: Bytes>(e: &B) -> Result<Vec<u8>> {
        let mut v = vec![0xEEu8; e.len() as usize];
        e.read_at(0, &mut v)?;
        Ok(v)
    }

    #[test]
    fn a_header_survives_its_encoding_and_says_what_it_holds() {
        let h = Header::new(5000, 9, [3; 16], 77).unwrap();
        assert_eq!(Header::parse(&h.encode()).unwrap(), h);
        assert_eq!((h.plain_len(), h.chunk_len(), h.key_id()), (5000, 496, 77));
        assert_eq!(
            h.stored_chunk(),
            512,
            "a chunk and its tag are the shift's bytes"
        );
        assert_eq!(h.id(), &[3; 16]);
        assert_eq!(h.chunks(), 11, "ten whole chunks and a part of one");
        assert_eq!(h.stored_len(), 4096 + 5000 + 11 * 16);
        assert_eq!(h.open_probe(), 4096 + 512, "the header's page and a chunk");
        // At a page or more a chunk, every chunk starts on a page.
        let paged = Header::new(100_000, 12, [0; 16], 0).unwrap();
        assert_eq!(paged.chunk_len(), 4080);
        assert!((0..paged.chunks()).all(|c| paged.stored_off(c).is_multiple_of(4096)));
        assert_eq!(
            paged.stored_range(4079, 2),
            (4096, 8192),
            "two chunks, two pages"
        );
        // No bytes is one chunk of none, and the probe is the whole of it.
        let empty = Header::new(0, 12, [0; 16], 0).unwrap();
        assert_eq!((empty.chunks(), empty.stored_len()), (1, 4096 + 16));
        assert_eq!(empty.open_probe(), empty.stored_len());
        // A length on a chunk boundary has no part chunk after it.
        assert_eq!(Header::new(992, 9, [0; 16], 0).unwrap().chunks(), 2);
    }

    #[test]
    fn a_header_that_cannot_be_an_envelope_is_refused_and_never_panics() {
        let good = Header::new(5000, 12, [1; 16], 0).unwrap().encode();
        assert!(Header::parse(&good).is_ok());
        assert!(Header::parse(&good[..47]).is_err(), "a byte short");
        for (at, what) in [(0, "magic"), (8, "version"), (12, "shift")] {
            let mut b = good;
            b[at] ^= 0x40;
            assert!(Header::parse(&b).is_err(), "{what}");
        }
        for shift in [0, MIN_CHUNK_SHIFT - 1, MAX_CHUNK_SHIFT + 1, u32::MAX] {
            assert!(Header::new(1, shift, [0; 16], 0).is_err(), "shift {shift}");
        }
        // More chunks than the nonce's four bytes count, and a stored length
        // past what a u64 holds.
        assert!(Header::new(u64::MAX, MIN_CHUNK_SHIFT, [0; 16], 0).is_err());
        assert!(Header::new(u64::MAX, MAX_CHUNK_SHIFT, [0; 16], 0).is_err());
        assert!(Header::new((496u64 << 32) + 1, 9, [0; 16], 0).is_err());
        assert!(Header::new(496u64 << 32, 9, [0; 16], 0).is_ok());
        // Every header of forty-eight bytes parses to an answer.
        let mut seed = 1u64;
        for _ in 0..20_000 {
            let mut b = good;
            for x in b.iter_mut().skip(8) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *x = (seed >> 33) as u8;
            }
            if let Ok(h) = Header::parse(&b) {
                assert!(h.stored_len() >= DATA_START + TAG as u64);
            }
        }
    }

    /// The multiply that stands for a divide is the divide, for every chunk
    /// length a header can name and every offset one can hold.
    #[test]
    fn a_chunk_is_found_without_a_divide_and_is_the_one_a_divide_finds() {
        let mut seed = 7u64;
        for shift in MIN_CHUNK_SHIFT..=MAX_CHUNK_SHIFT {
            let d = (1u64 << shift) - TAG as u64;
            let per = Per::new(d);
            let top = (1u64 << 56) - 1;
            let check = |n: u64| assert_eq!(per.of(n), (n / d, (n % d) as usize), "{n} by {d}");
            let last = top / d * d;
            for n in [
                0,
                1,
                d - 1,
                d,
                d + 1,
                2 * d - 1,
                2 * d,
                top - 1,
                top,
                last,
                last - 1,
            ] {
                check(n);
            }
            for _ in 0..20_000 {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let q = (seed >> 8) % (top / d);
                for n in [q * d, q * d + d - 1, (seed >> 8) & top] {
                    check(n);
                }
            }
        }
    }

    #[test]
    fn every_range_reads_as_the_plaintext_does() {
        for keep in KEEPS {
            for shift in [9, 10, 12] {
                let c = (1usize << shift) - TAG;
                for n in [0, 1, c - 1, c, c + 1, 3 * c, 3 * c + 7] {
                    let p = plain(n);
                    let stored = envelope(&p, shift, 5, 11);
                    assert_eq!(
                        stored.len() as u64,
                        Header::parse(&stored).unwrap().stored_len()
                    );
                    let e = open(SliceBytes(&stored), 11, keep).unwrap();
                    let what = format!("shift {shift}, {n} bytes, keeping {keep}");
                    assert_eq!(e.len(), n as u64);
                    let edges = [
                        0,
                        1,
                        c - 1,
                        c,
                        c + 1,
                        2 * c - 1,
                        2 * c,
                        n.saturating_sub(1),
                        n,
                    ];
                    for &a in edges.iter().filter(|&&a| a <= n) {
                        for &b in edges.iter().filter(|&&b| b >= a && b <= n) {
                            let mut got = vec![0u8; b - a];
                            e.read_at(a as u64, &mut got).unwrap();
                            assert_eq!(got, p[a..b], "{a}..{b}, {what}");
                            // Lent, it is the same bytes; and a reader that
                            // keeps nothing lends nothing.
                            match e.slice_at(a as u64, b - a) {
                                Some(lent) => assert_eq!(lent, &p[a..b], "lent {a}..{b}, {what}"),
                                None => assert!(keep != u64::MAX || n == 0, "{a}..{b}, {what}"),
                            }
                            assert!(keep != 0 || e.slice_at(a as u64, b - a).is_none());
                        }
                    }
                    assert_eq!(read_whole(&e).unwrap(), p, "whole, {what}");
                    assert!(e.kept_bytes() <= keep.min(n as u64), "{what}");
                    let mut one = [0u8; 1];
                    assert!(e.read_at(n as u64, &mut one).is_err(), "past the end");
                    assert!(
                        e.read_at(u64::MAX, &mut one).is_err(),
                        "an offset that wraps"
                    );
                    assert!(e.slice_at(n as u64, 1).is_none(), "lent past the end");
                    assert!(
                        e.slice_at(u64::MAX, 2).is_none(),
                        "lent at an offset that wraps"
                    );
                }
            }
        }
    }

    /// Every bit of a stored object is one the open or the read refuses,
    /// the zeroes after the header among them, however the reader holds
    /// what it reads; and nothing of a changed object is lent.
    #[test]
    fn a_changed_bit_anywhere_is_refused() {
        let p = plain(3 * C + 100);
        let stored = envelope(&p, 9, 5, 11);
        for keep in KEEPS {
            let whole =
                |bytes: &[u8]| open(SliceBytes(bytes), 11, keep).and_then(|e| read_whole(&e));
            assert_eq!(whole(&stored).unwrap(), p, "the control reads");
            for at in 0..stored.len() {
                for bit in [0x01, 0x80] {
                    let mut bad = stored.clone();
                    bad[at] ^= bit;
                    assert!(
                        whole(&bad).is_err(),
                        "byte {at} bit {bit:#x} read as if unchanged"
                    );
                    if let Ok(e) = open(SliceBytes(&bad), 11, keep) {
                        assert!(e.slice_at(0, p.len()).is_none(), "byte {at} lent");
                    }
                }
            }
        }
    }

    #[test]
    fn a_chunk_moved_or_borrowed_is_refused() {
        let p = plain(4 * C);
        let stored = envelope(&p, 9, 5, 11);
        let chunk = |i: usize| D + i * S..D + (i + 1) * S;
        let (third, fourth) = (2 * C as u64, 3 * C as u64);
        for keep in KEEPS {
            let read = |bytes: &[u8], off: u64| {
                let e = open(SliceBytes(bytes), 11, keep)?;
                assert!(e.slice_at(off, 8).is_none() || bytes == stored, "lent");
                let mut b = [0u8; 8];
                e.read_at(off, &mut b)
            };
            assert!(
                read(&stored, third).is_ok() && read(&stored, fourth).is_ok(),
                "the control"
            );

            // Two chunks of one object, each in the other's place.
            let mut swapped = stored.clone();
            swapped[chunk(2)].copy_from_slice(&stored[chunk(3)]);
            swapped[chunk(3)].copy_from_slice(&stored[chunk(2)]);
            assert!(read(&swapped, third).is_err() && read(&swapped, fourth).is_err());

            // The same chunk of another object under the same key: another id.
            let other = envelope(&p, 9, 6, 11);
            let mut spliced = stored.clone();
            spliced[chunk(2)].copy_from_slice(&other[chunk(2)]);
            assert!(read(&spliced, third).is_err());

            // The same id over a plaintext one byte longer: the same nonces,
            // and a header every tag was made over that says another length.
            let longer = envelope(&plain(4 * C + 1), 9, 5, 11);
            let mut grown = stored.clone();
            grown[chunk(2)].copy_from_slice(&longer[chunk(2)]);
            assert!(read(&grown, third).is_err());

            // Another key id is another header.
            let mut renamed = stored.clone();
            renamed[40] ^= 1;
            assert!(read(&renamed, 0).is_err());
        }
    }

    #[test]
    fn an_object_cut_short_or_grown_is_refused_at_open() {
        let p = plain(4 * C);
        let stored = envelope(&p, 9, 5, 11);
        for keep in KEEPS {
            let open = |bytes: &[u8]| open(SliceBytes(bytes), 11, keep).map(|_| ());
            assert!(open(&stored).is_ok(), "the control");
            assert!(open(&stored[..stored.len() - 1]).is_err(), "a byte short");
            assert!(open(&stored[..stored.len() - S]).is_err(), "a chunk short");
            assert!(open(&stored[..40]).is_err(), "less than a header");
            assert!(open(&stored[..D]).is_err(), "a header and no chunk");
            assert!(open(&[]).is_err(), "nothing");
            let mut longer = stored.clone();
            longer.push(0);
            assert!(open(&longer).is_err(), "a byte long");

            // A chunk short with the header's length rewritten to agree: the
            // stored length is right, and the first chunk's tag is over the
            // header as it was.
            let mut cut = stored[..stored.len() - S].to_vec();
            cut[16..24].copy_from_slice(&(3 * C as u64).to_le_bytes());
            assert_eq!(cut.len() as u64, Header::parse(&cut).unwrap().stored_len());
            assert!(open(&cut).is_err());

            // An envelope of no bytes is not one a header alone can forge.
            let empty = envelope(&[], 9, 5, 11);
            assert!(open(&empty).is_ok(), "the control");
            let mut forged = stored[..D + TAG].to_vec();
            forged[16..24].copy_from_slice(&0u64.to_le_bytes());
            assert!(open(&forged).is_err());
        }
    }

    #[test]
    fn the_wrong_key_is_refused_at_open() {
        let stored = envelope(&plain(2000), 9, 5, 11);
        for keep in KEEPS {
            assert!(open(SliceBytes(&stored), 11, keep).is_ok());
            assert!(open(SliceBytes(&stored), 12, keep).is_err());
        }
    }

    /// A source that answers reads and lends nothing, counts them, and can
    /// be made to change a byte after the open.
    struct Copying {
        data: Vec<u8>,
        flip: Cell<Option<usize>>,
        reads: Cell<usize>,
    }

    impl Copying {
        fn new(data: Vec<u8>) -> Copying {
            Copying {
                data,
                flip: Cell::new(None),
                reads: Cell::new(0),
            }
        }
    }

    impl Bytes for Copying {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
            self.reads.set(self.reads.get() + 1);
            SliceBytes(&self.data).read_at(off, dst)?;
            if let Some(at) = self.flip.get() {
                if let Some(i) = at.checked_sub(off as usize).filter(|&i| i < dst.len()) {
                    dst[i] ^= 1;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn a_failed_read_hands_back_zeroes_and_not_a_part_of_the_answer() {
        let p = plain(4 * C);
        let third = 2 * C as u64;
        for keep in KEEPS {
            let e = open(Copying::new(envelope(&p, 9, 5, 11)), 11, keep).unwrap();
            // A byte of the third chunk's ciphertext changes under the
            // reader: the two chunks before it verify, and none of them is
            // handed back.
            e.source().flip.set(Some(D + 2 * S + 5));
            let mut got = vec![0xEEu8; p.len()];
            assert!(e.read_at(0, &mut got).is_err());
            assert!(
                got.iter().all(|&b| b == 0),
                "a refused read left bytes behind"
            );
            assert!(e.slice_at(third, 10).is_none(), "a refused chunk lent");
            // And the chunk refused is not remembered as read, or kept, and
            // what the cipher made of it is not left where a kept chunk goes.
            let mut one = [0xEEu8; 1];
            assert!(e.read_at(third + 9, &mut one).is_err());
            assert_eq!(one, [0]);
            if let Some(k) = &e.kept {
                // SAFETY: the mapping is the plaintext's length, and nothing
                // is writing the chunk: the read that tried has returned.
                let left = unsafe { std::slice::from_raw_parts(k.map.as_ptr().add(2 * C), C) };
                assert!(
                    left.iter().all(|&b| b == 0),
                    "a refused chunk's bytes left in place"
                );
            }
            let held = e.kept_bytes();
            assert!(held <= third, "a refused chunk counted as kept: {held}");
            e.source().flip.set(None);
            e.read_at(third + 9, &mut one).unwrap();
            assert_eq!(one[0], p[2 * C + 9]);
            assert_eq!(
                read_whole(&e).unwrap(),
                p,
                "and the object reads once it is itself"
            );
        }
    }

    #[test]
    fn a_reader_that_keeps_nothing_decrypts_a_run_of_reads_in_one_chunk_once() {
        let p = plain(4 * C);
        let e = open(Copying::new(envelope(&p, 9, 5, 11)), 11, 0).unwrap();
        let opened = e.source().reads.get();
        assert_eq!(opened, 2, "the header's page and the first chunk");
        let mut b = [0u8; 16];
        for off in [0u64, 100, 480] {
            e.read_at(off, &mut b).unwrap();
            assert_eq!(b, p[off as usize..off as usize + 16]);
        }
        assert_eq!(
            e.source().reads.get(),
            opened,
            "the open's chunk served them"
        );
        for off in [600u64, 700, 900] {
            e.read_at(off, &mut b).unwrap();
        }
        assert_eq!(e.source().reads.get(), opened + 1, "one read for three");
        assert_eq!(e.aead.decrypts.load(Ordering::Relaxed), 2);
        // A walk of pieces that lie across the two chunks, which is how a
        // block's values are read, decrypts neither again.
        for off in [480u64, 490, 485, 100, 495, 600, 488] {
            e.read_at(off, &mut b).unwrap();
            assert_eq!(b, p[off as usize..off as usize + 16]);
        }
        assert_eq!(
            e.aead.decrypts.load(Ordering::Relaxed),
            2,
            "two chunks, each once"
        );
        // A whole chunk goes to the caller's buffer and is not remembered:
        // the part of it read next is decrypted again.
        let mut whole = [0u8; C];
        e.read_at(2 * C as u64, &mut whole).unwrap();
        assert_eq!(whole, p[2 * C..3 * C]);
        e.read_at(2 * C as u64 + 6, &mut b).unwrap();
        assert_eq!(e.aead.decrypts.load(Ordering::Relaxed), 4);
        assert_eq!(e.kept_bytes(), 0);
    }

    /// The thread's remembered chunk is one reader's: two objects read turn
    /// about at the same chunk never answer each other's bytes.
    #[test]
    fn two_readers_on_one_thread_keep_their_own_chunks() {
        let (pa, pb) = (
            plain(2000),
            plain(2000).iter().map(|b| !b).collect::<Vec<u8>>(),
        );
        let (sa, sb) = (envelope(&pa, 9, 5, 11), envelope(&pb, 9, 6, 11));
        for keep in KEEPS {
            let a = open(SliceBytes(&sa), 11, keep).unwrap();
            let b = open(SliceBytes(&sb), 11, keep).unwrap();
            let mut got = [0u8; 32];
            for _ in 0..3 {
                for off in [10u64, 700, 1500] {
                    a.read_at(off, &mut got).unwrap();
                    assert_eq!(got, pa[off as usize..off as usize + 32]);
                    b.read_at(off, &mut got).unwrap();
                    assert_eq!(got, pb[off as usize..off as usize + 32]);
                }
            }
        }
    }

    /// A reader over a reader: the inner one's chunk displaces the outer's
    /// in the thread's buffers while the outer is filling them.
    #[test]
    fn an_envelope_inside_an_envelope_reads() {
        let p = plain(5000);
        let inner = envelope(&p, 9, 5, 11);
        let outer = envelope(&inner, 10, 6, 12);
        let part = KEEPS[2];
        for (keep_outer, keep_inner) in [(0, 0), (0, u64::MAX), (u64::MAX, 0), (part, part)] {
            let e = open(
                open(SliceBytes(&outer), 12, keep_outer).unwrap(),
                11,
                keep_inner,
            )
            .unwrap();
            let mut got = [0u8; 100];
            for off in [0u64, 500, 1000, 4900, 30, 2047] {
                e.read_at(off, &mut got).unwrap();
                assert_eq!(got, p[off as usize..off as usize + 100], "at {off}");
            }
            assert_eq!(read_whole(&e).unwrap(), p);
        }
    }

    /// A read touches the stored bytes `stored_range` names and no others,
    /// and an open the probe.
    #[test]
    fn a_read_touches_the_stored_range_it_was_planned_as() {
        struct Recording<'a>(&'a [u8], std::cell::RefCell<Vec<(u64, u64)>>);
        impl Bytes for Recording<'_> {
            fn len(&self) -> u64 {
                self.0.len() as u64
            }
            fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
                self.1.borrow_mut().push((off, dst.len() as u64));
                SliceBytes(self.0).read_at(off, dst)
            }
        }
        let p = plain(9 * C + 40);
        let stored = envelope(&p, 9, 5, 11);
        let h = Header::parse(&stored).unwrap();
        let span = |log: &[(u64, u64)]| {
            let lo = log.iter().map(|r| r.0).min().unwrap();
            (lo, log.iter().map(|r| r.0 + r.1).max().unwrap() - lo)
        };
        let c = C as u64;
        for keep in [0, u64::MAX] {
            for (off, len) in [
                (600, 10),
                (900, 200),
                (c, 8 * c + 40),
                (9 * c - 5, 45),
                (4 * c, c),
            ] {
                // A reader of its own for each, so that nothing is held
                // from the read before but the first chunk.
                let e = open(Recording(&stored, Default::default()), 11, keep).unwrap();
                let opened = std::mem::take(&mut *e.source().1.borrow_mut());
                assert_eq!(span(&opened), (0, h.open_probe()));
                e.read_at(off, &mut vec![0u8; len as usize]).unwrap();
                let log = std::mem::take(&mut *e.source().1.borrow_mut());
                assert_eq!(span(&log), h.stored_range(off, len), "{len} bytes at {off}");
                assert_eq!(
                    log.iter().map(|r| r.1).sum::<u64>(),
                    h.stored_range(off, len).1,
                    "each stored byte once"
                );
                assert_eq!(
                    log.len() as u64,
                    (off + len - 1) / c - off / c + 1,
                    "a read a chunk"
                );
            }
        }
        let (d, s) = (D as u64, S as u64);
        assert_eq!(h.stored_range(0, c), (d, s));
        assert_eq!(h.stored_range(c - 1, 2), (d, 2 * s));
        assert_eq!(h.stored_range(9 * c, 40), (d + 9 * s, 56), "the part chunk");
        assert_eq!(h.stored_range(10, 0), (0, 0), "nothing asked");
        assert_eq!(h.stored_range(9 * c + 32, 9), (0, 0), "past the end");
        assert_eq!(h.stored_range(u64::MAX, 2), (0, 0), "an offset that wraps");
    }

    /// The stored object shows nothing of what it holds, and two envelopes
    /// of one plaintext share nothing but their header's first words.
    #[test]
    fn the_stored_bytes_are_not_the_plaintext() {
        let p = vec![0xABu8; 4096];
        let a = envelope(&p, 9, 5, 11);
        let run = |bytes: &[u8]| {
            bytes
                .windows(8)
                .filter(|w| w.iter().all(|&b| b == 0xAB))
                .count()
        };
        assert_eq!(
            run(&a[D..]),
            0,
            "a run of the plaintext's byte in the ciphertext"
        );
        // The ciphertext alone, without the tags: those differ by the
        // header they cover whatever the nonce is.
        let b = envelope(&p, 9, 6, 11);
        assert_ne!(a[D..D + C], b[D..D + C], "another id is another keystream");
        assert_ne!(a[D..D + C], a[D + S..D + S + C], "and another chunk too");
        assert!(
            a[Header::LEN..D].iter().all(|&x| x == 0),
            "zeroes to the first chunk"
        );
    }

    #[cfg(unix)]
    #[test]
    fn two_envelopes_of_one_plaintext_have_ids_of_their_own() {
        let p = plain(1000);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let opts = EncryptOptions::default();
        let ha = encrypt(&SliceBytes(&p), &mut a, &Toy::new(1), &opts).unwrap();
        let hb = encrypt(&SliceBytes(&p), &mut b, &Toy::new(1), &opts).unwrap();
        assert_ne!(ha.id(), hb.id());
        assert_ne!(ha.id(), &[0u8; 16], "an id nobody drew");
        assert_eq!(ha.chunk_shift(), DEFAULT_CHUNK_SHIFT);
        assert_eq!(read_whole(&open(SliceBytes(&a), 1, 0).unwrap()).unwrap(), p);
    }

    /// The writer hands its sink a megabyte at a time, each at a megabyte's
    /// offset, whatever the chunk: the envelope is the same bytes either way.
    #[test]
    fn the_writer_writes_whole_megabytes() {
        struct Pieces(Vec<usize>, Vec<u8>);
        impl Write for Pieces {
            fn write(&mut self, buf: &[u8]) -> Result<usize> {
                self.0.push(buf.len());
                self.1.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> Result<()> {
                Ok(())
            }
        }
        let p = plain((5 << 19) + 77);
        for shift in [9, 12, 21] {
            let mut out = Pieces(Vec::new(), Vec::new());
            let opts = EncryptOptions {
                chunk_shift: shift,
                key_id: 0,
            };
            let h =
                encrypt_with_id(&SliceBytes(&p), &mut out, &Toy::new(3), &opts, [8; 16]).unwrap();
            assert_eq!(out.1.len() as u64, h.stored_len());
            let (last, whole) = out.0.split_last().unwrap();
            assert!(
                whole.len() >= 2 && whole.iter().all(|&n| n == 1 << 20),
                "shift {shift}: {:?}",
                out.0
            );
            assert!(*last < 1 << 20);
            assert_eq!(
                read_whole(&open(SliceBytes(&out.1), 3, 0).unwrap()).unwrap(),
                p
            );
        }
    }

    // --------------------------------------------------- what a reader keeps --

    /// A kept chunk is decrypted once, read from the source once, and lent
    /// from the same place every time after.
    #[test]
    fn a_kept_chunk_is_decrypted_once_and_lent_from_then_on() {
        let p = plain(6 * C + 40);
        let e = open(Copying::new(envelope(&p, 9, 5, 11)), 11, u64::MAX).unwrap();
        let count = |e: &Encrypted<Copying, Toy>| {
            (
                e.aead.decrypts.load(Ordering::Relaxed),
                e.source().reads.get() as u64,
            )
        };
        assert_eq!(
            count(&e),
            (1, 2),
            "the first chunk, and the header's page before it"
        );
        assert_eq!(e.kept_bytes(), C as u64);

        let a = e.slice_at(700, 900).expect("lent").as_ptr();
        assert_eq!(e.slice_at(700, 900).unwrap(), &p[700..1600]);
        assert_eq!(count(&e), (4, 5), "chunks one to three, once each");
        assert_eq!(e.kept_bytes(), 4 * C as u64);
        let mut got = vec![0u8; 900];
        for _ in 0..3 {
            e.read_at(700, &mut got).unwrap();
            assert_eq!(got, p[700..1600]);
            assert_eq!(
                e.slice_at(700, 900).unwrap().as_ptr(),
                a,
                "lent from the same place"
            );
            assert_eq!(e.slice_at(10, 20).unwrap(), &p[10..30], "the open's chunk");
        }
        assert_eq!(count(&e), (4, 5), "nothing decrypted or read again");

        assert_eq!(read_whole(&e).unwrap(), p);
        assert_eq!(e.slice_at(0, p.len()).unwrap(), &p[..]);
        assert_eq!(count(&e), (7, 8), "the three chunks left");
        assert_eq!(
            e.kept_bytes(),
            p.len() as u64,
            "the whole plaintext, and no more"
        );
        assert_eq!(
            e.slice_at(p.len() as u64, 0),
            Some(&[][..]),
            "no bytes at the end"
        );
    }

    /// Under a bound a reader keeps the chunks touched first, while they
    /// fit, and reads the rest as a reader that keeps nothing does.
    #[test]
    fn a_bound_keeps_the_chunks_that_fit_and_reads_the_rest() {
        let p = plain(3 * C + 40);
        let c = C as u64;
        // Room for two whole chunks and sixty bytes.
        let stored = envelope(&p, 9, 5, 11);
        let e = open(SliceBytes(&stored), 11, 2 * c + 60).unwrap();
        assert_eq!(e.kept_bytes(), c, "the open's chunk");
        assert_eq!(
            e.slice_at(2 * c + 6, 100).unwrap(),
            &p[2 * C + 6..2 * C + 106],
            "the third fits"
        );
        assert_eq!(e.kept_bytes(), 2 * c);
        // The second does not, and neither does a range it is part of.
        assert!(e.slice_at(c + 100, 10).is_none());
        assert!(e.slice_at(0, 2 * C + 100).is_none());
        assert_eq!(e.kept_bytes(), 2 * c, "a chunk refused room took none");
        let before = e.aead.decrypts.load(Ordering::Relaxed);
        let mut got = [0u8; 10];
        for _ in 0..2 {
            let mut whole = [0u8; C];
            e.read_at(c, &mut whole).unwrap();
            assert_eq!(whole, p[C..2 * C]);
            e.read_at(100, &mut got).unwrap();
            assert_eq!(got, p[100..110], "a kept chunk between");
        }
        assert_eq!(
            e.aead.decrypts.load(Ordering::Relaxed),
            before + 2,
            "decrypted at each read"
        );
        // A chunk there is no room for is not so much as claimed: the bound
        // is looked at first, so a reader at its bound takes nothing a
        // reader beside it would have to wait on.
        let k = e.kept.as_ref().unwrap();
        assert_eq!(
            k.claims.load(Ordering::Relaxed),
            2,
            "the two chunks kept, and no other"
        );
        // And with that look away, the claim itself holds the bound: the
        // chunk is given back, and nothing is counted or lent.
        k.no_look.store(true, Ordering::Relaxed);
        assert!(e.slice_at(c + 100, 10).is_none());
        e.read_at(c + 100, &mut got).unwrap();
        assert_eq!(got, p[C + 100..C + 110]);
        assert_eq!(
            k.claims.load(Ordering::Relaxed),
            4,
            "claimed twice, and given back"
        );
        assert_eq!(e.kept_bytes(), 2 * c);
        k.no_look.store(false, Ordering::Relaxed);
        // The last chunk is forty bytes, and those fit.
        assert_eq!(e.slice_at(3 * c, 40).unwrap(), &p[3 * C..]);
        assert_eq!(e.kept_bytes(), 2 * c + 40);
        assert_eq!(read_whole(&e).unwrap(), p);
        assert_eq!(e.kept_bytes(), 2 * c + 40, "and nothing past the bound");
    }

    /// Threads that reach a chunk together: one decrypts it, the others
    /// wait, and all of them read the same bytes.
    #[test]
    fn threads_decrypt_each_chunk_once_between_them() {
        let p = plain(64 * C + 9);
        let stored = envelope(&p, 9, 5, 11);
        for _ in 0..20 {
            let e = open(SliceBytes(&stored), 11, u64::MAX).unwrap();
            let start = std::sync::Barrier::new(8);
            std::thread::scope(|s| {
                for t in 0..8usize {
                    let (e, p, start) = (&e, &p, &start);
                    s.spawn(move || {
                        start.wait();
                        for i in 0..65usize {
                            // Each thread in an order of its own.
                            let c = (i * (2 * t + 1) + t) % 65;
                            let (a, b) = (c * C, ((c + 1) * C).min(p.len()));
                            if t % 2 == 0 {
                                assert_eq!(e.slice_at(a as u64, b - a).unwrap(), &p[a..b]);
                            } else {
                                let mut got = vec![0u8; b - a];
                                e.read_at(a as u64, &mut got).unwrap();
                                assert_eq!(got, p[a..b]);
                            }
                        }
                    });
                }
            });
            assert_eq!(
                e.aead.decrypts.load(Ordering::Relaxed),
                65,
                "a chunk decrypted twice"
            );
            assert_eq!(e.kept_bytes(), p.len() as u64);
            assert_eq!(
                e.slice_at(0, p.len()).unwrap(),
                &p[..],
                "and the whole of it is lent"
            );
        }
    }

    /// A cipher that panics inside a chunk leaves the chunk as it was
    /// before: nobody waits on it for good, and its room is given back.
    #[test]
    fn a_cipher_that_panics_leaves_its_chunk_to_the_next_reader() {
        let p = plain(4 * C);
        let stored = envelope(&p, 9, 5, 11);
        let e = open(SliceBytes(&stored), 11, KEEPS[2]).unwrap();
        e.aead.panic_at.store(1, Ordering::Relaxed);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            e.slice_at(600, 10).is_some()
        }));
        assert!(r.is_err(), "the cipher was to panic");
        assert_eq!(e.kept_bytes(), C as u64, "the chunk's room came back");
        e.aead.panic_at.store(u64::MAX, Ordering::Relaxed);
        assert_eq!(
            e.slice_at(600, 10).unwrap(),
            &p[600..610],
            "and the chunk is not left taken"
        );
        assert_eq!(e.kept_bytes(), 2 * C as u64);
    }

    /// The pages asked for ahead of a long range are no more than the
    /// reader may keep: a reader at its bound takes none it will not write.
    /// And neither way of asking for pages changes an answer.
    #[cfg(target_os = "linux")]
    #[test]
    fn pages_asked_for_ahead_stay_inside_the_bound() {
        let p = plain(1 << 20);
        let stored = envelope(&p, 12, 5, 11);
        // The kept mapping's pages the kernel holds now.
        let resident = |e: &Encrypted<SliceBytes, Toy>| {
            let k = e.kept.as_ref().unwrap();
            let mut vec = vec![0u8; k.map.len().div_ceil(4096)];
            // SAFETY: the mapping lives as long as the reader, and `vec`
            // has a byte for each of its pages.
            let rc =
                unsafe { libc::mincore(k.map.as_mut_ptr().cast(), k.map.len(), vec.as_mut_ptr()) };
            assert_eq!(rc, 0);
            vec.iter().filter(|&&b| b & 1 != 0).count()
        };
        let bound = ReadOptions {
            keep_bytes: 16 * 4080,
            ..ReadOptions::default()
        };
        assert!(bound.populate, "the default asks");
        let e = Encrypted::open_with(SliceBytes(&stored), Toy::new(11), &bound).unwrap();
        assert!(
            e.slice_at(0, p.len()).is_none(),
            "the whole object is past the bound"
        );
        assert_eq!(e.kept_bytes(), 16 * 4080);
        assert!(
            resident(&e) <= 17,
            "pages taken past the bound: {}",
            resident(&e)
        );
        assert_eq!(read_whole(&e).unwrap(), p);

        for (populate, huge_pages) in [(true, false), (false, false), (false, true), (true, true)] {
            let opts = ReadOptions {
                populate,
                huge_pages,
                ..ReadOptions::default()
            };
            let e = Encrypted::open_with(SliceBytes(&stored), Toy::new(11), &opts).unwrap();
            assert_eq!(e.slice_at(4096, 300_000).unwrap(), &p[4096..304_096]);
            assert_eq!(e.slice_at(0, p.len()).unwrap(), &p[..]);
            assert_eq!(e.kept_bytes(), p.len() as u64);
        }
    }

    /// A long range lent once is known whole after, and asked for again it
    /// costs no look at its chunks: a section a reader borrows is asked for
    /// at every lookup.
    #[test]
    fn a_section_lent_once_is_not_walked_again() {
        let p = plain(100 * C);
        let stored = envelope(&p, 9, 5, 11);
        let e = open(SliceBytes(&stored), 11, u64::MAX).unwrap();
        let walked = |e: &Encrypted<SliceBytes, Toy>| {
            e.kept.as_ref().unwrap().walked.load(Ordering::Relaxed)
        };
        let lend = |a: usize, b: usize| {
            assert_eq!(e.slice_at(a as u64, b - a).unwrap(), &p[a..b], "{a}..{b}")
        };

        lend(10 * C, 40 * C);
        assert_eq!(walked(&e), 30, "thirty chunks, each looked at");
        for _ in 0..3 {
            lend(10 * C, 40 * C);
            lend(12 * C + 3, 30 * C - 9);
        }
        assert_eq!(walked(&e), 30, "known whole, and a part of it too");
        // A range that runs one chunk past what is known is walked, and is
        // what it should be: what is known is no more than what was looked
        // at.
        lend(32 * C, 41 * C);
        assert_eq!(walked(&e), 39);
        lend(32 * C, 48 * C);
        assert_eq!(walked(&e), 55);
        // A short range is walked each time and takes no place.
        let places = |e: &Encrypted<SliceBytes, Toy>| {
            let known = &e.kept.as_ref().unwrap().known;
            known
                .iter()
                .filter(|k| k.load(Ordering::Relaxed) != UNKNOWN)
                .count()
        };
        assert_eq!(places(&e), 3);
        for _ in 0..2 {
            lend(60 * C, 63 * C);
        }
        assert_eq!(walked(&e), 61);
        assert_eq!(places(&e), 3, "a short range remembered");
        // One more long range fills the places; a fifth, shorter than all
        // of them, puts none of them out, and each is still known.
        lend(64 * C, 77 * C);
        let filled = walked(&e);
        assert_eq!((filled, places(&e)), (61 + 13, 4));
        lend(90 * C, 98 * C);
        lend(90 * C, 98 * C);
        assert_eq!(walked(&e), filled + 16, "the shortest took no place");
        lend(10 * C, 40 * C);
        lend(32 * C, 41 * C);
        lend(32 * C, 48 * C);
        lend(64 * C, 77 * C);
        assert_eq!(walked(&e), filled + 16);
        // A longer one takes the shortest's place, and only that one.
        lend(78 * C, 90 * C);
        lend(78 * C, 90 * C);
        lend(10 * C, 40 * C);
        lend(32 * C, 48 * C);
        lend(64 * C, 77 * C);
        assert_eq!(walked(&e), filled + 16 + 12);
        lend(32 * C, 41 * C);
        assert_eq!(
            walked(&e),
            filled + 16 + 12,
            "inside a longer one still known"
        );
        // With every chunk kept, nothing is walked at all.
        lend(0, p.len());
        let all = walked(&e);
        assert_eq!(all, filled + 16 + 12 + 100, "the whole object, once");
        lend(60 * C, 63 * C);
        lend(1, p.len() - 1);
        lend(90 * C, 99 * C);
        assert_eq!(walked(&e), all);
    }
}
