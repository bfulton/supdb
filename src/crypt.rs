//! An encrypted object, read through the `Bytes` seam.
//!
//! A supdb file is published and read where the writer does not control the
//! storage: a bucket, a CDN, a copy on somebody's disk. This module wraps a
//! finished file in an envelope that hides its bytes and refuses any change
//! to them, and reads the envelope back as a byte source: `Encrypted` is a
//! `Bytes`, so `Blob::open(Encrypted::open(src, key)?)` and `SparseBlob` read
//! an encrypted object with nothing in the format or the readers changed.
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
//! header, 48 bytes        magic, version, chunk shift, plaintext length,
//!                         object id, key id
//! chunk 0                 ciphertext of plaintext [0, C)        + tag
//! chunk 1                 ciphertext of plaintext [C, 2C)       + tag
//! ...                     the last chunk holds what is left
//! ```
//!
//! `C` is `1 << chunk_shift` plaintext bytes, so a plaintext offset names its
//! chunk by a shift and a range of plaintext is a range of whole chunks: a
//! reader that fetches by range fetches `Header::stored_range` of what it was
//! going to read. An object of no bytes still has one chunk, of no bytes and
//! one tag, so that every envelope carries a tag over its header.
//!
//! Each chunk is encrypted on its own under nonce `id[..8] || chunk index`,
//! with the whole header as associated data. What that refuses:
//!
//! - a changed byte anywhere, in the header or in a chunk;
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
//! A read decrypts the chunks its range touches, whole, and copies out the
//! part asked for; the thread remembers the last chunk it decrypted, so a run
//! of small reads inside one chunk decrypts once. An encrypted source lends
//! nothing (`slice_at` answers `None`), so a `Blob` over one copies its index
//! sections out once at open, as it does over any source that cannot lend.
//! Plaintext is never handed out unverified: a chunk whose tag does not match
//! is an error and the caller's buffer is zeroed.

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
/// The chunk `EncryptOptions::default` writes: four kilobytes, the unit the
/// readers' own plans are made in, at four tenths of a percent of tags.
pub const DEFAULT_CHUNK_SHIFT: u32 = 12;

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

    /// A header for `plain_len` bytes of plaintext in chunks of
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
        (Header::LEN as u64)
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

    /// The plaintext bytes a chunk holds, all but the last.
    pub fn chunk_len(&self) -> usize {
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
        self.plain_len.div_ceil(1 << self.chunk_shift).max(1)
    }

    /// The stored object's length: the header, the plaintext's bytes and a
    /// tag a chunk.
    pub fn stored_len(&self) -> u64 {
        Header::LEN as u64 + self.plain_len + self.chunks() * TAG as u64
    }

    /// How many leading bytes of the stored object `Encrypted::open` reads:
    /// the header and the first chunk.
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
        let (first, last) = (off >> self.chunk_shift, (end - 1) >> self.chunk_shift);
        let lo = self.stored_off(first);
        let hi = self.stored_off(last) + (self.chunk_plain_len(last) + TAG) as u64;
        (lo, hi - lo)
    }

    /// Where a chunk's ciphertext starts in the stored object.
    fn stored_off(&self, chunk: u64) -> u64 {
        Header::LEN as u64 + chunk * (self.chunk_len() + TAG) as u64
    }

    /// A chunk's plaintext length: the chunk size, or what is left.
    fn chunk_plain_len(&self, chunk: u64) -> usize {
        let start = chunk << self.chunk_shift;
        (self.plain_len - start).min(self.chunk_len() as u64) as usize
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
    out.write_all(&aad)?;
    let mut buf = Vec::with_capacity(hdr.chunk_len() + TAG);
    for c in 0..hdr.chunks() {
        let n = hdr.chunk_plain_len(c);
        buf.clear();
        buf.resize(n, 0);
        src.read_at(c << hdr.chunk_shift, &mut buf)?;
        let tag = aead.encrypt(&hdr.nonce(c), &aad, &mut buf);
        buf.extend_from_slice(&tag);
        out.write_all(&buf)?;
    }
    Ok(hdr)
}

/// The thread's last decrypted chunk, and whose it is: which reader and
/// which chunk. The thread's rather than the reader's, since a reader is
/// shared between threads, and a `Cell` so that a reader over a reader finds
/// an empty buffer rather than a borrow in use.
struct Scratch {
    buf: Cell<Vec<u8>>,
    at: Cell<Option<(u64, u64)>>,
}

thread_local! {
    static SCRATCH: Scratch = const {
        Scratch {
            buf: Cell::new(Vec::new()),
            at: Cell::new(None),
        }
    };
}

/// Reader ids, one per open.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An encrypted object read as its plaintext: a `Bytes` over the stored
/// object `src`, under the key `aead` holds.
pub struct Encrypted<B: Bytes, A: Aead> {
    src: B,
    aead: A,
    hdr: Header,
    aad: [u8; Header::LEN],
    id: u64,
}

impl<B: Bytes, A: Aead> Encrypted<B, A> {
    /// Open the object `src` holds. Reads the header and the first chunk
    /// (`Header::open_probe` bytes) and verifies that chunk, so a wrong key,
    /// a changed header and a length that is not the header's are refused
    /// here and not at some later read.
    pub fn open(src: B, aead: A) -> Result<Encrypted<B, A>> {
        let hdr = Header::read(&src)?;
        if src.len() != hdr.stored_len() {
            return Err(refused(&format!(
                "an encrypted object of {} bytes whose header describes {}: cut short, or grown",
                src.len(),
                hdr.stored_len()
            )));
        }
        let e = Encrypted {
            aad: hdr.encode(),
            src,
            aead,
            hdr,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        };
        e.with_chunk(0, |_| ())?;
        Ok(e)
    }

    pub fn header(&self) -> &Header {
        &self.hdr
    }

    /// The stored object this reads through.
    pub fn source(&self) -> &B {
        &self.src
    }

    /// Chunk `c`'s plaintext, decrypted into the thread's buffer unless it
    /// is what the buffer already holds.
    fn with_chunk<R>(&self, c: u64, f: impl FnOnce(&[u8]) -> R) -> Result<R> {
        SCRATCH.with(|s| {
            let mut buf = s.buf.take();
            // Nobody's while it is being filled: an error below leaves it so.
            let held = s.at.replace(None) == Some((self.id, c));
            let r = if held {
                Ok(())
            } else {
                self.open_chunk(c, &mut buf)
            };
            let r = r.map(|()| {
                s.at.set(Some((self.id, c)));
                f(&buf)
            });
            s.buf.set(buf);
            r
        })
    }

    /// Read chunk `c` from the source into `buf` and decrypt it there,
    /// leaving `buf` the chunk's plaintext, or empty and an error.
    fn open_chunk(&self, c: u64, buf: &mut Vec<u8>) -> Result<()> {
        let n = self.hdr.chunk_plain_len(c);
        let at = self.hdr.stored_off(c);
        buf.clear();
        buf.resize(n + TAG, 0);
        match self.src.slice_at(at, n + TAG) {
            Some(stored) => buf.copy_from_slice(stored),
            None => self.src.read_at(at, buf).inspect_err(|_| buf.clear())?,
        }
        let (body, tag) = buf.split_at_mut(n);
        let tag: [u8; TAG] = (&*tag).try_into().unwrap();
        if !self.aead.decrypt(&self.hdr.nonce(c), &self.aad, body, &tag) {
            // Whatever the cipher left there is not plaintext anyone may see.
            buf.fill(0);
            buf.clear();
            return Err(refused(&format!(
                "chunk {c} of an encrypted object failed its check: the key is wrong, or the \
                 object was changed"
            )));
        }
        buf.truncate(n);
        Ok(())
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
            let c = at >> self.hdr.chunk_shift;
            let lo = (at - (c << self.hdr.chunk_shift)) as usize;
            let n = (self.hdr.chunk_plain_len(c) - lo).min(dst.len() - done);
            let part = &mut dst[done..done + n];
            if let Err(e) = self.with_chunk(c, |plain| part.copy_from_slice(&plain[lo..lo + n])) {
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

    // `slice_at` is the default: an encrypted source has no plaintext to
    // lend. The advice that names no offset goes to the stored object, whose
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

    /// A stand-in cipher, for the envelope's own arithmetic: a keystream and
    /// a tag that are each a function of the key, the nonce, the associated
    /// data and the bytes, so that a tag made under one of them fails under
    /// any other. Not a cipher anyone may use.
    struct Toy(u64);

    fn mix(mut h: u64, bytes: &[u8]) -> u64 {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h ^ (h >> 29)
    }

    impl Toy {
        fn stream(&self, nonce: &[u8; NONCE], buf: &mut [u8]) {
            let mut s = mix(self.0 | 1, nonce);
            for b in buf {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *b ^= s as u8;
            }
        }
        fn tag(&self, nonce: &[u8; NONCE], aad: &[u8], body: &[u8]) -> [u8; TAG] {
            let a = mix(mix(mix(self.0, nonce), aad), body);
            let b = mix(a, &(aad.len() as u64).to_le_bytes());
            let mut t = [0u8; TAG];
            t[..8].copy_from_slice(&a.to_le_bytes());
            t[8..].copy_from_slice(&b.to_le_bytes());
            t
        }
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
            let ok = self.tag(nonce, aad, buf) == *tag;
            // Decrypts whatever the verdict, as the trait allows: the reader
            // is what keeps a failed chunk's bytes from the caller.
            self.stream(nonce, buf);
            ok
        }
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
        encrypt_with_id(&SliceBytes(p), &mut out, &Toy(key), &opts, [id; 16]).unwrap();
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
        assert_eq!((h.plain_len(), h.chunk_len(), h.key_id()), (5000, 512, 77));
        assert_eq!(h.id(), &[3; 16]);
        assert_eq!(h.chunks(), 10, "nine whole chunks and a part of one");
        assert_eq!(h.stored_len(), 48 + 5000 + 10 * 16);
        assert_eq!(h.open_probe(), 48 + 512 + 16);
        // No bytes is one chunk of none, and the probe is the whole of it.
        let empty = Header::new(0, 12, [0; 16], 0).unwrap();
        assert_eq!((empty.chunks(), empty.stored_len()), (1, 48 + 16));
        assert_eq!(empty.open_probe(), empty.stored_len());
        // A length on a chunk boundary has no part chunk after it.
        assert_eq!(Header::new(1024, 9, [0; 16], 0).unwrap().chunks(), 2);
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
        assert!(Header::new((1u64 << 41) + 1, 9, [0; 16], 0).is_err());
        assert!(Header::new(1u64 << 41, 9, [0; 16], 0).is_ok());
        // Every header of forty-eight bytes parses to an answer.
        let mut seed = 1u64;
        for _ in 0..20_000 {
            let mut b = good;
            for x in b.iter_mut().skip(8) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *x = (seed >> 33) as u8;
            }
            if let Ok(h) = Header::parse(&b) {
                assert!(h.stored_len() >= Header::LEN as u64 + TAG as u64);
            }
        }
    }

    #[test]
    fn every_range_reads_as_the_plaintext_does() {
        for shift in [9, 10, 12] {
            let c = 1usize << shift;
            for n in [0, 1, c - 1, c, c + 1, 3 * c, 3 * c + 7] {
                let p = plain(n);
                let stored = envelope(&p, shift, 5, 11);
                assert_eq!(
                    stored.len() as u64,
                    Header::parse(&stored).unwrap().stored_len()
                );
                let e = Encrypted::open(SliceBytes(&stored), Toy(11)).unwrap();
                assert_eq!(e.len(), n as u64);
                assert_eq!(read_whole(&e).unwrap(), p, "whole, shift {shift} len {n}");
                assert!(e.slice_at(0, n.min(1)).is_none(), "nothing to lend");
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
                        assert_eq!(got, p[a..b], "{a}..{b} of {n} at shift {shift}");
                    }
                }
                let mut one = [0u8; 1];
                assert!(e.read_at(n as u64, &mut one).is_err(), "past the end");
                assert!(
                    e.read_at(u64::MAX, &mut one).is_err(),
                    "an offset that wraps"
                );
            }
        }
    }

    /// Every bit of a stored object is one the open or the read refuses.
    #[test]
    fn a_changed_bit_anywhere_is_refused() {
        let p = plain(3 * 512 + 100);
        let stored = envelope(&p, 9, 5, 11);
        let whole =
            |bytes: &[u8]| Encrypted::open(SliceBytes(bytes), Toy(11)).and_then(|e| read_whole(&e));
        assert_eq!(whole(&stored).unwrap(), p, "the control reads");
        for at in 0..stored.len() {
            for bit in [0x01, 0x80] {
                let mut bad = stored.clone();
                bad[at] ^= bit;
                assert!(
                    whole(&bad).is_err(),
                    "byte {at} bit {bit:#x} read as if unchanged"
                );
            }
        }
    }

    #[test]
    fn a_chunk_moved_or_borrowed_is_refused() {
        let p = plain(4 * 512);
        let stored = envelope(&p, 9, 5, 11);
        let chunk = |i: usize| 48 + i * 528..48 + (i + 1) * 528;
        let read = |bytes: &[u8], off: u64| {
            let e = Encrypted::open(SliceBytes(bytes), Toy(11))?;
            let mut b = [0u8; 8];
            e.read_at(off, &mut b)
        };
        assert!(
            read(&stored, 1024).is_ok() && read(&stored, 1536).is_ok(),
            "the control"
        );

        // Two chunks of one object, each in the other's place.
        let mut swapped = stored.clone();
        swapped[chunk(2)].copy_from_slice(&stored[chunk(3)]);
        swapped[chunk(3)].copy_from_slice(&stored[chunk(2)]);
        assert!(read(&swapped, 1024).is_err() && read(&swapped, 1536).is_err());

        // The same chunk of another object under the same key: another id.
        let other = envelope(&p, 9, 6, 11);
        let mut spliced = stored.clone();
        spliced[chunk(2)].copy_from_slice(&other[chunk(2)]);
        assert!(read(&spliced, 1024).is_err());

        // The same id over a plaintext one byte longer: the same nonces, and
        // a header every tag was made over that says another length.
        let longer = envelope(&plain(4 * 512 + 1), 9, 5, 11);
        let mut grown = stored.clone();
        grown[chunk(2)].copy_from_slice(&longer[chunk(2)]);
        assert!(read(&grown, 1024).is_err());

        // Another key id is another header.
        let mut renamed = stored.clone();
        renamed[40] ^= 1;
        assert!(read(&renamed, 0).is_err());
    }

    #[test]
    fn an_object_cut_short_or_grown_is_refused_at_open() {
        let p = plain(4 * 512);
        let stored = envelope(&p, 9, 5, 11);
        let open = |bytes: &[u8]| Encrypted::open(SliceBytes(bytes), Toy(11)).map(|_| ());
        assert!(open(&stored).is_ok(), "the control");
        assert!(open(&stored[..stored.len() - 1]).is_err(), "a byte short");
        assert!(
            open(&stored[..stored.len() - 528]).is_err(),
            "a chunk short"
        );
        assert!(open(&stored[..40]).is_err(), "less than a header");
        assert!(open(&[]).is_err(), "nothing");
        let mut longer = stored.clone();
        longer.push(0);
        assert!(open(&longer).is_err(), "a byte long");

        // A chunk short with the header's length rewritten to agree: the
        // stored length is right, and the first chunk's tag is over the
        // header as it was.
        let mut cut = stored[..stored.len() - 528].to_vec();
        cut[16..24].copy_from_slice(&(3u64 * 512).to_le_bytes());
        assert_eq!(cut.len() as u64, Header::parse(&cut).unwrap().stored_len());
        assert!(open(&cut).is_err());

        // An envelope of no bytes is not one a header alone can forge.
        let empty = envelope(&[], 9, 5, 11);
        assert!(open(&empty).is_ok(), "the control");
        let mut forged = stored[..48 + 16].to_vec();
        forged[16..24].copy_from_slice(&0u64.to_le_bytes());
        assert!(open(&forged).is_err());
    }

    #[test]
    fn the_wrong_key_is_refused_at_open() {
        let stored = envelope(&plain(2000), 9, 5, 11);
        assert!(Encrypted::open(SliceBytes(&stored), Toy(11)).is_ok());
        assert!(Encrypted::open(SliceBytes(&stored), Toy(12)).is_err());
    }

    /// A source that answers reads and lends nothing, and can be made to
    /// change a byte after the open.
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
        let p = plain(4 * 512);
        let e = Encrypted::open(Copying::new(envelope(&p, 9, 5, 11)), Toy(11)).unwrap();
        assert_eq!(
            read_whole(&e).unwrap(),
            p,
            "the control, through a copying source"
        );
        // A byte of the third chunk's ciphertext changes under the reader:
        // the two chunks before it verify, and none of them is handed back.
        e.source().flip.set(Some(48 + 2 * 528 + 5));
        let mut got = vec![0xEEu8; p.len()];
        assert!(e.read_at(0, &mut got).is_err());
        assert!(
            got.iter().all(|&b| b == 0),
            "a refused read left bytes behind"
        );
        // And the chunk refused is not remembered as read.
        let mut one = [0xEEu8; 1];
        assert!(e.read_at(2 * 512 + 9, &mut one).is_err());
        assert_eq!(one, [0]);
        e.source().flip.set(None);
        e.read_at(2 * 512 + 9, &mut one).unwrap();
        assert_eq!(one[0], p[2 * 512 + 9]);
    }

    #[test]
    fn a_run_of_reads_inside_one_chunk_decrypts_it_once() {
        let p = plain(4 * 512);
        let e = Encrypted::open(Copying::new(envelope(&p, 9, 5, 11)), Toy(11)).unwrap();
        let opened = e.source().reads.get();
        assert_eq!(opened, 2, "the header and the first chunk");
        let mut b = [0u8; 16];
        for off in [0u64, 100, 496] {
            e.read_at(off, &mut b).unwrap();
            assert_eq!(b, p[off as usize..off as usize + 16]);
        }
        assert_eq!(
            e.source().reads.get(),
            opened,
            "the open's chunk served them"
        );
        for off in [600u64, 700, 1000] {
            e.read_at(off, &mut b).unwrap();
        }
        assert_eq!(e.source().reads.get(), opened + 1, "one read for three");
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
        let a = Encrypted::open(SliceBytes(&sa), Toy(11)).unwrap();
        let b = Encrypted::open(SliceBytes(&sb), Toy(11)).unwrap();
        let mut got = [0u8; 32];
        for _ in 0..3 {
            for off in [10u64, 700] {
                a.read_at(off, &mut got).unwrap();
                assert_eq!(got, pa[off as usize..off as usize + 32]);
                b.read_at(off, &mut got).unwrap();
                assert_eq!(got, pb[off as usize..off as usize + 32]);
            }
        }
    }

    /// A reader over a reader: the inner one's chunk displaces the outer's
    /// in the thread's buffer while the outer is filling it.
    #[test]
    fn an_envelope_inside_an_envelope_reads() {
        let p = plain(5000);
        let inner = envelope(&p, 9, 5, 11);
        let outer = envelope(&inner, 10, 6, 12);
        let e = Encrypted::open(
            Encrypted::open(SliceBytes(&outer), Toy(12)).unwrap(),
            Toy(11),
        )
        .unwrap();
        assert_eq!(read_whole(&e).unwrap(), p);
        let mut got = [0u8; 100];
        for off in [0u64, 500, 1000, 4900, 30, 2047] {
            e.read_at(off, &mut got).unwrap();
            assert_eq!(got, p[off as usize..off as usize + 100], "at {off}");
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
        let p = plain(9 * 512 + 40);
        let stored = envelope(&p, 9, 5, 11);
        let e = Encrypted::open(Recording(&stored, Default::default()), Toy(11)).unwrap();
        let h = *e.header();
        let span = |log: &[(u64, u64)]| {
            let lo = log.iter().map(|r| r.0).min().unwrap();
            (lo, log.iter().map(|r| r.0 + r.1).max().unwrap() - lo)
        };
        let opened = std::mem::take(&mut *e.source().1.borrow_mut());
        assert_eq!(span(&opened), (0, h.open_probe()));

        for (off, len) in [
            (600u64, 10u64),
            (1000, 100),
            (0, 4648),
            (4600, 48),
            (2048, 512),
        ] {
            // Another chunk first, so that none of this read is remembered.
            let far = if off < 2048 { 4000 } else { 0 };
            e.read_at(far, &mut [0u8; 1]).unwrap();
            e.source().1.borrow_mut().clear();
            e.read_at(off, &mut vec![0u8; len as usize]).unwrap();
            let log = std::mem::take(&mut *e.source().1.borrow_mut());
            assert_eq!(span(&log), h.stored_range(off, len), "{len} bytes at {off}");
            assert_eq!(
                log.iter().map(|r| r.1).sum::<u64>(),
                h.stored_range(off, len).1,
                "each stored byte once"
            );
        }
        assert_eq!(h.stored_range(0, 512), (48, 528));
        assert_eq!(h.stored_range(511, 2), (48, 2 * 528));
        assert_eq!(
            h.stored_range(4608, 40),
            (48 + 9 * 528, 56),
            "the part chunk"
        );
        assert_eq!(h.stored_range(10, 0), (0, 0), "nothing asked");
        assert_eq!(h.stored_range(4640, 9), (0, 0), "past the end");
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
            run(&a[48..]),
            0,
            "a run of the plaintext's byte in the ciphertext"
        );
        // The ciphertext alone, without the tags: those differ by the
        // header they cover whatever the nonce is.
        let b = envelope(&p, 9, 6, 11);
        assert_ne!(
            a[48..48 + 512],
            b[48..48 + 512],
            "another id is another keystream"
        );
        assert_ne!(
            a[48..48 + 512],
            a[48 + 528..48 + 528 + 512],
            "and another chunk too"
        );
    }

    #[cfg(unix)]
    #[test]
    fn two_envelopes_of_one_plaintext_have_ids_of_their_own() {
        let p = plain(1000);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let opts = EncryptOptions::default();
        let ha = encrypt(&SliceBytes(&p), &mut a, &Toy(1), &opts).unwrap();
        let hb = encrypt(&SliceBytes(&p), &mut b, &Toy(1), &opts).unwrap();
        assert_ne!(ha.id(), hb.id());
        assert_ne!(ha.id(), &[0u8; 16], "an id nobody drew");
        assert_eq!(ha.chunk_shift(), DEFAULT_CHUNK_SHIFT);
        assert_eq!(
            read_whole(&Encrypted::open(SliceBytes(&a), Toy(1)).unwrap()).unwrap(),
            p
        );
    }
}
