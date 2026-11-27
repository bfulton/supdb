//! A live store under a cipher the caller supplies (`Encryption::Cipher`).
//!
//! The other way a store is kept under a key is the kernel's
//! (`crate::dirkey`): there the page cache holds the plaintext and nothing
//! on a read path changes, on a filesystem that can encrypt. This one is the
//! engine's own, for a caller who brings the cipher: any filesystem, no
//! privilege, the caller's key wherever the caller keeps it, every byte on
//! the device authenticated as well as encrypted, and files that are
//! ciphertext wherever they are copied to.
//!
//! What it costs is where the plaintext lives. A store reads by lending
//! from mappings, so the plaintext has to be in memory something can map,
//! and with the cipher in the process that memory is the process's, where
//! a plain store's is the kernel's page cache. So a store under a cipher
//! keeps a cache of its own (`super::pager`): each file is address space
//! its plaintext's length, a chunk of which is decrypted from the file's
//! envelope (`crate::crypt`) where something first reads it and given back
//! once the cache is past its budget (`StoreCipher::cache_bytes`) and
//! nothing has read it lately. A store of any size runs in the cache it is
//! given, at the rate of its misses, as a plain store runs in the memory
//! the kernel has for it; an open reads each file's header and decrypts
//! nothing else; and a store holds no descriptor for a file once the file
//! has its envelope.
//!
//! What makes that possible is the readers asking their source for bytes
//! before they read them (`Bytes::piecewise`, `flatindex::Pieces`), which
//! a reader over a mapped file does not do: a file under a cipher is a
//! source that lends by the piece, and the engine names one reader type
//! for both.
//!
//! A file being written is a memory file, written by the code that writes
//! a plain store's, until it is finished and has its envelope
//! (`Vault::persist`); then it is the cache's (`Vault::settled`), which
//! takes the plaintext it was written as where it has room, so that a
//! store that fits its cache reads what it wrote without decrypting it,
//! and lets it go where it has not.
//!
//! What the cache's budget does not count is what a plain store holds in
//! the process too: the memtables, the block cache's forms, a scan's
//! snapshot. And the kernel's page cache holds the envelopes' ciphertext,
//! which is what a miss is read from, as it holds a plain store's files.
//!
//! # The door
//!
//! The engine names a file by a path in its directory and reaches it
//! through the functions here (`create`, `open`, `map`, `rename`,
//! `remove_file`, `names`, ...), each of which is the standard library's
//! call for a plain store. A store under a cipher is given a directory that
//! cannot exist: its marker file, `supdb.cipher`, with a name after it. The
//! functions here find the store by that directory and act on its files
//! as it holds them; a file operation that did not come through them fails
//! with "not a directory" and writes nothing, which is what keeps a path
//! the door was not put in front of from writing plaintext beside the
//! ciphertext.
//!
//! # What is in the directory
//!
//! - `supdb.cipher`: the marker. The store's id, the caller's name for the
//!   key, and the store's own key wrapped under the caller's cipher. A
//!   store without one is not under a cipher, and one with it opens under
//!   no other.
//! - `x-<token>`: a segment, as an envelope under a key drawn for that
//!   file; `x-<token>.o`, its ordered index, and `x-<token>.z`, its
//!   compact form where it has one (`zfile`), under the same key. The
//!   names say nothing: a segment's own name carries its fences, sixteen
//!   bytes of two keys, and lives in the manifest. No manifest names the
//!   two beside a segment: each is a function of its segment, checked
//!   against it where it is opened and made again where it is missing.
//! - `manifest`, `manifest.alt`: the slots, each sealed under the store's
//!   key, naming each live segment with its token and its wrapped key.
//! - `wal-<id>`: the log, each commit a record sealed under a key drawn for
//!   that file and wrapped in its header.
//! - `spare-<id>`: a log file retired and kept to be written again in
//!   place (`Options::recycle_wal`): its last life's records, or zeroes,
//!   under a key its next life does not have.
//! - `store.one`, in a store kept in one file (`Options::one_file`): the
//!   data file the envelopes are extents of, each under the name above
//!   that it would have had in the directory, in a table the vault holds
//!   (`Vault::one`, `onefile`). The sealed manifest lists each one's
//!   place behind the segments' names, tokens and keys, and an ordered
//!   index's and a form file's where a sync has covered them, which is
//!   how an open finds the two beside a segment there. An envelope is the
//!   bytes it is as a file, read through the same cache; what differs is
//!   that a retired one's room is written over and not freed, that a
//!   segment's sync is a sync of the data file and covers its index's
//!   envelope written ahead of it, and that no envelope has a name for
//!   the directory to make durable.
//!
//! # Keys and nonces
//!
//! The caller's cipher is used for one thing, wrapping the store's key in
//! the marker: once when the store is made and once at each open. That is
//! what lets it be slow, or somewhere else. Everything after is under
//! ciphers the caller's constructor makes from 32-byte keys the store
//! draws: the store's key wraps a key a file, and each file's bytes are
//! under its own. A nonce is never drawn at random from the whole of its
//! twelve bytes against a key that sees many of them: under a file's key it
//! is the envelope's chunk index or the log's record count behind eight
//! random bytes, and under the store's key a count behind eight random
//! bytes drawn at each open.
//!
//! # What an attacker who can write the directory can and cannot do
//!
//! Any byte changed in a segment, an index, a form file, a manifest or a
//! log record is found, and none is ever read as data. A file's bytes are checked where
//! they are decrypted, which is where they are first read: a header that
//! is not the file's is refused where the file is opened, and a chunk
//! that fails its tag at the read that wants it, which answers with an
//! error, as every operation of the store does from then on
//! (`pager::Pager::check`) -- the readers' answers are options in which
//! bytes that did not read and a key that is not there look alike, so the
//! failure is remembered and asked for at each operation's end, and a
//! merge that met one publishes nothing. A manifest slot that fails is
//! passed over for the other slot, as a torn one is, and the store opens
//! as the state that slot names or is refused where the log no longer
//! reaches back to it. A log record that fails is where the log ends, as
//! a torn tail is. So what a changed byte can do is what a crash can do,
//! or make the store refuse to be read, and no more: the store answers as
//! a state it once acknowledged, or does not answer.
//!
//! What is not refused is the store taken back to an earlier state of
//! itself on purpose: an old copy of the whole directory, or the log cut at
//! a record's end. Nothing in a directory can say that a newer one existed.
//! And what is not hidden: how many files the store has and how large each
//! is, the size of each commit in the log, and the caller's name for its
//! key.
//!
//! The plaintext the cache holds is in memory, and memory can be written
//! to swap and to a core file; a host where that matters encrypts its swap
//! and keeps its core files where it keeps its keys.
//!
//! # A file beside a segment, written again
//!
//! A segment's compact form is built after the segment, by a reader of
//! the segment's records through the store's cache, and may be built more
//! than once: in another coding, or after a page of it failed a check.
//! Its envelope is written under the segment's token and key
//! (`Vault::persist_form`) under a temp name and renamed over its own, so
//! that a reader's view of the form before keeps the file it was opened
//! on; an open learns of it where the engine first asks after it
//! (`companion`), since nothing names it but its place; and it leaves the
//! directory with its segment.
//!
//! # A file written and mapped at once
//!
//! Ordered ingest's run is a segment open for append that its table reads
//! through a mapping while it is written, and a plain store makes it
//! durable by syncing it at every commit. Here the run's file is a memory
//! file like any other being written, so its commits go to the log as
//! well, sealed as any commit's are, and the file gets its envelope once
//! the run closes. A file a writer has mapped for itself
//! ([`open_mapped`]) is held in memory for as long as the mapping stands
//! ([`Mapped`]): its envelope is written where the file is finished, and
//! the cache takes the file over, to give its pages back, only once the
//! last such mapping has gone.

// The door (scripts/door/clippy.toml, tests/data/door.txt): the door itself.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use crate::bytes::MmapBytes;
use crate::crypt::{self, Aead, EncryptOptions, NONCE, TAG};
use std::collections::HashMap;
use std::fs::{File as OsFile, OpenOptions};
use std::io::{Error, ErrorKind, Read, Result, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::onefile::{self, ExtFile, One};
pub use super::pager::PagerCounts;
use super::pager::{self, Back, Paged, Pager};
use crate::bytes::Paging;

/// The length of a key the store draws for itself or for a file.
pub const KEY: usize = 32;

/// A key wrapped under another: a nonce, the key's ciphertext, its tag.
pub(crate) const WRAPPED: usize = NONCE + KEY + TAG;

/// A file's token: what its envelope is named by in the directory.
pub(crate) type Token = [u8; 16];

/// What a manifest holds of each segment it names: the token its envelope
/// is under, and its key wrapped under the store's.
pub(crate) type FileKey = (Token, [u8; WRAPPED]);

type DynAead = dyn Aead + Send + Sync;
type Maker = dyn Fn(&[u8; KEY]) -> Box<DynAead> + Send + Sync;

/// The cipher a live store is kept under.
///
/// Two things, both the caller's. `master` is the cipher under the
/// caller's own key, and the store uses it for one thing: wrapping the key
/// it draws for itself, once at its creation and once at each open. `data`
/// makes a cipher of the same kind from a 32-byte key, and everything the
/// store writes is under ciphers it makes: the store's key, and a key
/// drawn for each file. So the caller's key encrypts 32 bytes a store and
/// may live wherever the caller keeps it, and no key meets more nonces
/// than its kind allows.
///
/// ```ignore
/// let cipher = StoreCipher::new(Gcm::new(&my_key), |k| Gcm::new(k)).key_id(7);
/// let opts = Options { encryption: Encryption::Cipher(cipher), ..Options::default() };
/// let db = Db::create(dir, opts)?;
/// ```
#[derive(Clone)]
pub struct StoreCipher {
    master: Arc<DynAead>,
    data: Arc<Maker>,
    key_id: u64,
    cache: Option<u64>,
    apart: bool,
    half: bool,
    beside: bool,
    drops_keep: bool,
    room: Option<Arc<RoomFn>>,
}

/// A room handed to a store in place of the platform's
/// (`StoreCipher::cache_room`): what its plaintext may hold, given what it
/// holds.
type RoomFn = dyn Fn(u64) -> Option<u64> + Send + Sync;

impl StoreCipher {
    /// A store's cipher from the cipher under the caller's key and the
    /// constructor of one from a key the store draws.
    pub fn new<M, A, D>(master: M, data: D) -> StoreCipher
    where
        M: Aead + Send + Sync + 'static,
        A: Aead + Send + Sync + 'static,
        D: Fn(&[u8; KEY]) -> A + Send + Sync + 'static,
    {
        StoreCipher {
            master: Arc::new(master),
            data: Arc::new(move |k| Box::new(data(k)) as Box<DynAead>),
            key_id: 0,
            cache: None,
            apart: false,
            half: false,
            beside: false,
            drops_keep: false,
            room: None,
        }
    }

    /// The plaintext of its files the store may hold in memory, in bytes:
    /// what it has decrypted and not given back. A store reads through
    /// this cache,
    /// decrypting a chunk of a file where it is first read and giving the
    /// chunks least lately read back once the cache is past this, so a
    /// store of any size runs in it, at the rate of its misses. Unset, it
    /// follows the room: the memory the process may use (its control
    /// group's limit or the machine's) less what the process itself
    /// holds, less a margin, read again as the store runs, shared between
    /// the stores of the process that follow it, and sixty-four mebibytes
    /// at least. Plaintext the store holds is memory the kernel cannot
    /// take back, where a store in the clear's page cache gives way to
    /// whatever the process allocates, so the store gives way itself: its
    /// cache shrinks as the process grows and grows as it shrinks. A size
    /// set here is held to whatever the process does beside it, and one
    /// near the limit under a process that grows is a process killed.
    ///
    /// The budget is of the files' plaintext, and it is where the cache
    /// gives back, not a line it never passes: a file being written is in
    /// memory whole until it is sealed into the directory, a read holds
    /// what it was lent until it ends or rests, and the store's own work
    /// waits for a cache past its budget before a caller's read does. The
    /// memtables and the block cache are not in it, as they are not in
    /// the page cache for a plain store (`Options::scan_cache_bytes`).
    pub fn cache_bytes(mut self, bytes: u64) -> StoreCipher {
        self.cache = Some(bytes);
        self
    }

    /// The cache's size as `cache_bytes` set it, or `None` where the cache
    /// follows the room: what decides a store's budget, where the budget
    /// itself is a reading of the room that moves with the process and is
    /// shared between the stores of the process that follow it.
    #[doc(hidden)]
    pub fn cache_bytes_set(&self) -> Option<u64> {
        self.cache
    }

    /// The cache as it was before it followed the room: half of what the
    /// process may use as its store opens, read once, the other half left
    /// to the kernel's cache of the store's files. The shape the room is
    /// priced against; nothing where `cache_bytes` is set.
    #[doc(hidden)]
    pub fn cache_half_at_open(mut self, half: bool) -> StoreCipher {
        self.half = half;
        self
    }

    /// A room in the platform's place, for a test: what the store's
    /// plaintext may hold, given what it holds, asked as the platform is
    /// asked. Nothing where `cache_bytes` is set.
    #[doc(hidden)]
    pub fn cache_room(
        mut self,
        room: impl Fn(u64) -> Option<u64> + Send + Sync + 'static,
    ) -> StoreCipher {
        self.room = Some(Arc::new(room));
        self
    }

    /// Whether a read that wants several chunks of a file at once leaves
    /// each chunk's ciphertext to be read from the device as that chunk is
    /// decrypted. As shipped, off: where two or more of the chunks are not
    /// in the cache, the store asks the kernel for the ciphertext of all
    /// of them before it decrypts the first, so chunks that lie side by
    /// side on the device come in one read (`CipherCounts`' cache counts
    /// them, `together`). On is the shape that is priced against.
    #[doc(hidden)]
    pub fn chunks_apart(mut self, apart: bool) -> StoreCipher {
        self.apart = apart;
        self
    }

    /// Whether a merge reads its inputs beside the store's cache: each
    /// chunk decrypted into a buffer of the merge's own, which its rests
    /// write again, a chunk the cache holds copied from it, and nothing of
    /// the inputs put into the cache or given back from it. As shipped,
    /// off: priced against the shape that reads them through the cache it
    /// read the pass after a full rewrite better and the passes after the
    /// load worse, the cache coming out of the load holding the merges'
    /// outputs where it had held their inputs (`ROADMAP.md`).
    #[doc(hidden)]
    pub fn merges_beside_cache(mut self, on: bool) -> StoreCipher {
        self.beside = on;
        self
    }

    /// Whether a drop of a file's pages (`Bytes::advise_dontneed`, the
    /// raw records of a partition its form file serves, a build's drop
    /// behind it) leaves the plaintext it covers in the store's cache, to
    /// be given back as the cache gives anything back, and drops the
    /// kernel's cache of the envelope alone. As shipped, off: the units
    /// wholly inside the range are given back at the evictor's next turn,
    /// whatever the budget, as a plain store's drop gives back its pages.
    /// On is the shape that is priced against.
    #[doc(hidden)]
    pub fn drops_keep_plaintext(mut self, keep: bool) -> StoreCipher {
        self.drops_keep = keep;
        self
    }

    /// Whether `drops_keep_plaintext` is set: a test's look at an arm.
    #[doc(hidden)]
    pub fn drops_keep_plaintext_set(&self) -> bool {
        self.drops_keep
    }

    /// Whether `merges_beside_cache` is set: a test's look at an arm.
    #[doc(hidden)]
    pub fn merges_beside_cache_set(&self) -> bool {
        self.beside
    }

    /// The caller's name for its key, written in the store's marker in the
    /// clear and read back by `key_id` without the key: how a caller with
    /// several keys finds the one a store wants. Zero unless set.
    pub fn key_id(mut self, id: u64) -> StoreCipher {
        self.key_id = id;
        self
    }
}

impl std::fmt::Debug for StoreCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StoreCipher {{ key_id: {} }}", self.key_id)
    }
}

/// A key the store drew, zeroed where it is dropped.
struct Key([u8; KEY]);

impl Key {
    fn draw() -> Result<Key> {
        let mut k = Key([0u8; KEY]);
        random(&mut k.0)?;
        Ok(k)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        for b in &mut self.0 {
            // SAFETY: a write through a live reference; volatile so that it
            // is not taken for a dead store.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

fn refused(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, msg)
}

/// Bytes from the operating system's generator.
fn random(buf: &mut [u8]) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let mut at = 0;
        while at < buf.len() {
            // SAFETY: the pointer and length are of the slice's unfilled
            // part.
            let n = unsafe { libc::getrandom(buf[at..].as_mut_ptr().cast(), buf.len() - at, 0) };
            if n < 0 {
                let e = Error::last_os_error();
                if e.kind() == ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            at += n as usize;
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        OsFile::open("/dev/urandom")?.read_exact(buf)
    }
}

/// Nonces under one key: eight random bytes and a count behind them, the
/// eight drawn again when the count is spent.
struct Nonces {
    prefix: [u8; 8],
    next: u32,
}

impl Nonces {
    fn new() -> Result<Nonces> {
        let mut prefix = [0u8; 8];
        random(&mut prefix)?;
        Ok(Nonces { prefix, next: 0 })
    }

    fn take(&mut self) -> Result<[u8; NONCE]> {
        let mut n = [0u8; NONCE];
        n[..8].copy_from_slice(&self.prefix);
        n[8..].copy_from_slice(&self.next.to_le_bytes());
        match self.next.checked_add(1) {
            Some(next) => self.next = next,
            None => {
                random(&mut self.prefix)?;
                self.next = 0;
            }
        }
        Ok(n)
    }
}

/// `plain` sealed under `aead`: `nonce | ciphertext | tag`.
fn seal(aead: &DynAead, nonce: [u8; NONCE], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(NONCE + plain.len() + TAG);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(plain);
    let tag = aead.encrypt(&nonce, aad, &mut out[NONCE..]);
    out.extend_from_slice(&tag);
    out
}

/// What `seal` sealed, or nothing where the tag does not hold.
fn unseal(aead: &DynAead, aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let body = sealed.len().checked_sub(NONCE + TAG)?;
    let nonce: [u8; NONCE] = sealed[..NONCE].try_into().unwrap();
    let tag: [u8; TAG] = sealed[NONCE + body..].try_into().unwrap();
    let mut plain = sealed[NONCE..NONCE + body].to_vec();
    if aead.decrypt(&nonce, aad, &mut plain, &tag) {
        Some(plain)
    } else {
        // Whatever the cipher left there is nobody's to see.
        plain.fill(0);
        None
    }
}

// ---------------------------------------------------------------- marker --

/// The marker's name, and the directory a store under a cipher is given:
/// a regular file, so nothing can be made under it.
pub(crate) const MARKER: &str = "supdb.cipher";
const MARKER_MAGIC: &[u8; 8] = b"supdbCS\0";
const MARKER_VERSION: u32 = 1;
/// `magic 8 | version u32 | zero u32 | key_id u64 | store id 16`, and then
/// the store's key wrapped under the caller's cipher with those bytes as
/// its associated data.
const MARKER_HEAD: usize = 40;
const MARKER_LEN: usize = MARKER_HEAD + WRAPPED;

fn marker_head(key_id: u64, store_id: &[u8; 16]) -> [u8; MARKER_HEAD] {
    let mut h = [0u8; MARKER_HEAD];
    h[..8].copy_from_slice(MARKER_MAGIC);
    h[8..12].copy_from_slice(&MARKER_VERSION.to_le_bytes());
    h[16..24].copy_from_slice(&key_id.to_le_bytes());
    h[24..40].copy_from_slice(store_id);
    h
}

/// A marker's key id, after its form: the magic, the version, the zero
/// word and the length.
fn marker_key_id(bytes: &[u8]) -> Result<u64> {
    if bytes.len() != MARKER_LEN || &bytes[..8] != MARKER_MAGIC {
        return Err(refused("not the marker of a store under a cipher"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != MARKER_VERSION || bytes[12..16] != [0u8; 4] {
        return Err(refused(
            "the marker of a store under a cipher is of a version this build does not read",
        ));
    }
    Ok(u64::from_le_bytes(bytes[16..24].try_into().unwrap()))
}

/// The caller's name for the key of the store in `dir`, read without the
/// key; `None` for a directory that holds no store under a cipher.
pub fn key_id(dir: &Path) -> Result<Option<u64>> {
    match std::fs::read(dir.join(MARKER)) {
        Ok(bytes) => marker_key_id(&bytes).map(Some),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The store in `dir` handed from one of the caller's keys to another.
///
/// The store's own key is unwrapped under `from` and wrapped again under
/// `to`, the marker is written again whole with `to`'s name for its key,
/// and nothing else in the directory is touched: the files are under keys
/// of their own, wrapped under the store's, which does not change. So the
/// caller's key is rotated in the time it takes to write a hundred bytes,
/// and from then on `from` opens nothing. It re-encrypts nothing: whoever
/// held `from` and a copy of the directory holds that copy still.
///
/// A store open under `from`, here or elsewhere, goes on as it was -- it
/// holds its own key already -- and opens under `to` the next time.
pub fn rewrap(dir: &Path, from: &StoreCipher, to: &StoreCipher) -> Result<()> {
    let bytes = std::fs::read(dir.join(MARKER))?;
    marker_key_id(&bytes)?;
    let key = unseal(&*from.master, &bytes[..MARKER_HEAD], &bytes[MARKER_HEAD..])
        .and_then(|k| Some(Key(k.as_slice().try_into().ok()?)))
        .ok_or_else(|| {
            Error::new(
                ErrorKind::PermissionDenied,
                "the cipher given does not open the store: the key is wrong, or the marker was \
                 changed",
            )
        })?;
    let store_id: [u8; 16] = bytes[24..MARKER_HEAD].try_into().unwrap();
    write_marker(dir, to, &store_id, &key)
}

/// The marker of a store under `cipher`, written whole and durable: under
/// a temporary name, synced, renamed into place, and the directory synced.
fn write_marker(dir: &Path, cipher: &StoreCipher, store_id: &[u8; 16], key: &Key) -> Result<()> {
    let head = marker_head(cipher.key_id, store_id);
    let mut nonce = [0u8; NONCE];
    random(&mut nonce)?;
    let mut bytes = head.to_vec();
    bytes.extend_from_slice(&seal(&*cipher.master, nonce, &head, &key.0));
    let tmp = dir.join(format!("{MARKER}.tmp"));
    {
        let mut f = OsFile::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(MARKER))?;
    OsFile::open(dir)?.sync_all()
}

/// A store opened with no cipher: refused where the directory is under
/// one, since its files would read as damage, or as nothing.
pub(crate) fn refuse_marked(dir: &Path) -> Result<()> {
    if dir.join(MARKER).exists() {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "the store is kept under a cipher and none was given (Encryption::Cipher)",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- files --

/// What an envelope holds: a segment, or one of the two files derived from
/// a segment and kept beside it under its token and its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Seg,
    /// The segment's ordered index, `x-<token>.o`.
    Ord,
    /// The segment's compact form (`zfile`), `x-<token>.z`.
    Form,
}

impl Kind {
    fn suffix(self) -> &'static str {
        match self {
            Kind::Seg => "",
            Kind::Ord => ".o",
            Kind::Form => ".z",
        }
    }
}

/// Where a kept file's envelope is, and the key it is under.
#[derive(Clone)]
struct Disk {
    token: Token,
    wrapped: [u8; WRAPPED],
    /// The segment the token names, or which of the files beside it.
    kind: Kind,
}

impl Disk {
    fn name(&self) -> String {
        disk_name(&self.token, self.kind)
    }

    fn same(&self, other: &Disk) -> bool {
        self.token == other.token && self.kind == other.kind
    }
}

fn disk_name(token: &Token, kind: Kind) -> String {
    let mut s = String::with_capacity(36);
    s.push_str("x-");
    for b in token {
        s.push_str(&format!("{b:02x}"));
    }
    s.push_str(kind.suffix());
    s
}

/// The name an envelope is written under before it has its own
/// (`Vault::persist_form`), which a crash leaves for the sweep.
const ENVELOPE_TMP: &str = ".tmp";

/// The token and kind an envelope's name carries, or nothing for a name
/// that is not one.
fn disk_parse(name: &str) -> Option<(Token, Kind)> {
    let rest = name.strip_prefix("x-")?;
    let (hex, kind) = if let Some(h) = rest.strip_suffix(".o") {
        (h, Kind::Ord)
    } else if let Some(h) = rest.strip_suffix(".z") {
        (h, Kind::Form)
    } else {
        (rest, Kind::Seg)
    };
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut token = [0u8; 16];
    for (i, t) in token.iter_mut().enumerate() {
        *t = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some((token, kind))
}

/// One of the store's files.
///
/// `mem` is the memory file a writer is writing or has written, and it is
/// held until the file has its envelope. `disk` is the envelope in the
/// directory, once written or found. `page` is the file as it is read
/// (`pager::Paged`): made at the first map, or when the envelope is
/// written, from the memory file where the cache has room for it and from
/// the envelope alone where it has not. A file with its envelope has no
/// memory file: its plaintext is the cache's, to hold and to give back.
#[derive(Clone, Default)]
struct Entry {
    mem: Option<Arc<OsFile>>,
    page: Option<Arc<Paged>>,
    disk: Option<Disk>,
}

/// The names of the envelopes kept beside the segment's under `token`,
/// which a store makes again where they are missing: its ordered index's
/// and its form file's.
fn riders(token: &[u8; 16]) -> [String; 2] {
    [disk_name(token, Kind::Ord), disk_name(token, Kind::Form)]
}

/// Whether `name` is a file whose bytes the engine seals itself, record by
/// record or slot by slot, and which therefore lives in the directory as
/// it is: the log, a retired log file kept to be the next one
/// (`Options::recycle_wal`), and the manifest. Every other name is kept
/// in memory and reaches the directory as an envelope, a name nobody
/// thought of included.
fn passes(name: &str) -> bool {
    name.starts_with("wal-") || name.starts_with("spare-") || name.starts_with("manifest")
}

/// What a store under a cipher counts of its own work.
#[derive(Default)]
pub(crate) struct Counts {
    /// Envelopes written, and the plaintext bytes in them.
    pub(crate) persisted: AtomicU64,
    pub(crate) persisted_bytes: AtomicU64,
    /// Files whose plaintext the cache took whole from their writer, and
    /// files it left to be read from their envelopes.
    pub(crate) adopted: AtomicU64,
    pub(crate) left: AtomicU64,
    /// Envelopes no manifest named, removed at the open.
    pub(crate) swept: AtomicU64,
    /// Segments whose envelope a manifest's own call wrote, no seal, run
    /// or merge having written it where it finished the file.
    pub(crate) late: AtomicU64,
    pub(crate) expected: AtomicU64,
}

/// A store under a cipher: its directory, its key, and its files.
pub(crate) struct Vault {
    /// The directory the envelopes, the log and the manifest are in.
    real: PathBuf,
    /// The directory the engine is given: the marker's path.
    decoy: PathBuf,
    /// The cipher under the store's own key.
    store: Box<DynAead>,
    data: Arc<Maker>,
    files: Mutex<HashMap<String, Entry>>,
    nonces: Mutex<Nonces>,
    /// One persist at a time: two would write one file twice.
    persisting: Mutex<()>,
    /// The memory files their writer has mapped for itself (`Mapped`).
    /// Taken after `files` where both are.
    mapped: Mutex<Vec<MappedFile>>,
    /// The cache the store's files are read through.
    pager: Arc<Pager>,
    /// Whether that cache follows the platform's room, which the stores
    /// of a process that do share (`PlatformRoom`).
    follows: bool,
    /// What the store's writers have said is coming, which the room its
    /// cache follows has as well (`Coming`).
    coming: Arc<Coming>,
    /// The table of the store's data file, where the store is kept in one
    /// (`Options::one_file`): every envelope is a file of that table and
    /// none is a file of the directory's (`keep_in`).
    one: std::sync::OnceLock<Arc<One>>,
    pub(crate) counts: Counts,
}

/// A memory file with mappings of its own outside the cache: an ordered
/// run's, which the run's table reads its committed records through
/// (`db::RunMap`). The cache's view of a file is a mapping of the same
/// memory, and what the cache gives back it takes out of that memory, so
/// while a mapping here stands the file is not the cache's to give back.
struct MappedFile {
    /// The memory file, kept open: its identity for as long as it is
    /// mapped.
    mem: Arc<OsFile>,
    /// The mappings over it.
    maps: usize,
    /// The view of it that waits for its envelope behind it, and the
    /// envelope: a file that got its envelope while it was mapped, to be
    /// the cache's to give back once the last mapping has gone.
    owed: Option<(Arc<Paged>, Disk)>,
}

/// What a mapping of a file being written holds on the file
/// (`open_mapped`): nothing for a plain store's. Under a cipher the file
/// is a memory file, which the store's cache will take for its own view
/// once the file has its envelope and give back by the megabyte, out of
/// the memory this mapping reads; dropped with the mapping, this is what
/// says the mapping has gone.
pub(crate) struct Mapped(Option<(Arc<Vault>, Arc<OsFile>)>);

impl Drop for Mapped {
    fn drop(&mut self) {
        if let Some((v, mem)) = self.0.take() {
            v.unmapped(&mem);
        }
    }
}

/// What a store's writers have said is on its way into its memory files
/// (`expect`): a file and the bytes it is to hold. A file being written is
/// memory the kernel cannot take back and no count of the cache has, and
/// the platform's reading sees it a page at a time, as it is written: a
/// merge's output, sixty megabytes in a fifth of a second, landed on a
/// cache steered to within its margin of the limit, with the merge's own
/// arrays and the output's form file beside it, and the store was killed
/// before two readings of its room had passed. Said where the file is
/// opened, the bytes still to come are out of the room from then, and the
/// cache gives way ahead of them.
#[derive(Default)]
struct Coming {
    files: Mutex<Vec<(std::sync::Weak<OsFile>, u64)>>,
    /// Every byte announced, ever: what the cache gives way to at once,
    /// between two readings of its room (`Room::claimed`).
    said: AtomicU64,
}

impl Coming {
    fn expect(&self, mem: &Arc<OsFile>, bytes: u64) {
        lock(&self.files).push((Arc::downgrade(mem), bytes));
        self.said.fetch_add(bytes, Ordering::Relaxed);
    }

    /// The bytes still to come: what each file was said to be, less what
    /// it holds, over the files still in a writer's hands. A file with
    /// its envelope is the cache's own, and one removed is nobody's:
    /// either way its memory file has gone from the table, and with the
    /// last writer's handle from here.
    fn bytes(&self) -> u64 {
        let mut c = lock(&self.files);
        c.retain(|(f, _)| f.strong_count() > 0);
        c.iter()
            .filter_map(|(f, want)| {
                let len = f.upgrade()?.metadata().ok()?.len();
                Some(want.saturating_sub(len))
            })
            .sum()
    }
}

/// The room a store's cache follows where its cipher names no size: the
/// platform's, or the one a test handed it (`StoreCipher::cache_room`),
/// less what the store's writers have said is coming (`Coming`).
struct PlatformRoom(Option<Arc<RoomFn>>, Arc<Coming>);

impl super::pager::Room for PlatformRoom {
    fn read(&self, held: u64) -> Option<u64> {
        let room = match &self.0 {
            Some(room) => room(held)?,
            None => PlatformRoom::platform(held)?,
        };
        // Less what this store's own writers have said is coming: each
        // store's off its own share.
        Some(room.saturating_sub(self.1.bytes()))
    }
    fn floor(&self) -> u64 {
        match self.0 {
            Some(_) => 0,
            None => ROOM_FLOOR,
        }
    }
    /// What the cache gives way to at once: this store's announcements,
    /// and beside the platform's room the buffers the process has mapped
    /// -- none of those with a room a test gives, since they are every
    /// store's in the process, and beside a room that is one store's alone
    /// they took its budget to nothing whenever another test mapped a
    /// table.
    fn claimed(&self) -> u64 {
        let said = self.1.said.load(Ordering::Relaxed);
        match self.0 {
            Some(_) => said,
            None => super::zfile::buffers_mapped().saturating_add(said),
        }
    }
}

impl PlatformRoom {
    /// The platform's room for a store that holds `held`.
    fn platform(held: u64) -> Option<u64> {
        // The platform's room is the process's, and every store of the
        // process that follows it takes an equal share of what is left of
        // it: what the store holds, and its share of how much more the
        // process may hold, or less by its share of what has to go.
        let (mut all, mut sharing) = (0u64, 0i64);
        for v in lock(&REGISTRY).iter().filter(|v| v.follows) {
            all += v.pager.held();
            sharing += 1;
        }
        // A store reads its room before it is registered: itself alone.
        let (all, sharing) = if sharing == 0 {
            (held, 1)
        } else {
            (all, sharing)
        };
        let slack = super::zfile::unreclaimable_slack(all)?;
        let room = i64::try_from(held)
            .unwrap_or(i64::MAX)
            .saturating_add(slack / sharing);
        if room_traced() {
            trace_room(slack);
        }
        Some(u64::try_from(room).unwrap_or(0))
    }
}

/// Whether the room's readings are traced (`SUPDB_PAGER_TRACE`), read once.
fn room_traced() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SUPDB_PAGER_TRACE").is_some())
}

/// `SUPDB_PAGER_TRACE=1`: what the plaintext a process holds under its
/// ciphers is made of, a `pagertrace` line on stderr from a cache's
/// reading of its room, ten a second at the most. The time on the clock
/// the suite's phase marks are on (`SUPDB_LAG_MARK`) and the slack the
/// room was drawn from, mebibytes; and for each store under a cipher what
/// its cache holds, what of that is files with no envelope yet, its
/// budget and the evictor's passes, stalls and waits, then its memory
/// files -- the ones being written, which no count of the cache has, and
/// the ones a reader has a view of -- with the largest by name. The
/// group's own memory by kind is `scripts/groupmem.py`'s, on the same
/// clock.
///
/// A process killed under a bound is read from this beside the group's
/// memory by kind (`scripts/groupmem.py`) and its threads' stacks at the
/// limit (`scripts/stallwatch.py`): the cache's own counts see nothing
/// of a file until it has a view or an envelope, and the evictor reads
/// its room at every turn, so a trace that stops while the store runs is
/// the evictor inside one pass's wait for a reader -- which is how the
/// close's pin was found (`Db::drop`).
fn trace_room(slack: i64) {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: the monotonic clock read into a timespec of this frame.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    let ms = ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000;
    let last = LAST.load(Ordering::Relaxed);
    if ms < last + 100
        || LAST
            .compare_exchange(last, ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let mib = |v: u64| v >> 20;
    let mut line = format!(
        "pagertrace {}.{:03} slack {}",
        ms / 1000,
        ms % 1000,
        slack >> 20
    );
    for v in lock(&REGISTRY).iter() {
        let c = v.pager.counts();
        line.push_str(&format!(
            " | held {} unbacked {} budget {} passes {} stalls {} waited_ms {}",
            mib(c.held),
            mib(v.pager.unbacked()),
            mib(c.budget),
            c.passes,
            c.stalls,
            c.waited_us / 1000
        ));
        // Each memory file once, by the name it was first met under.
        let mut seen: Vec<(*const OsFile, &str, u64, bool)> = Vec::new();
        let files = lock(&v.files);
        for (name, e) in files.iter() {
            let Some(mem) = &e.mem else { continue };
            let at = Arc::as_ptr(mem);
            if seen.iter().any(|s| s.0 == at) {
                continue;
            }
            let len = mem.metadata().map_or(0, |m| m.len());
            seen.push((at, name.as_str(), len, e.page.is_some()));
        }
        let sum = |viewed: bool| -> (usize, u64) {
            let of = seen.iter().filter(|s| s.3 == viewed);
            (of.clone().count(), of.map(|s| s.2).sum())
        };
        let (wn, wb) = sum(false);
        let (vn, vb) = sum(true);
        line.push_str(&format!(
            " | writing {wn} {} viewed {vn} {} mapped {} coming {}",
            mib(wb),
            mib(vb),
            lock(&v.mapped).len(),
            mib(v.coming.bytes())
        ));
        // The views, by the first field of the name each was met under,
        // for what the cache holds present of each kind of file.
        let views: Vec<(String, Arc<Paged>)> = files
            .iter()
            .filter_map(|(name, e)| {
                let field = name.split('-').next().unwrap_or(name).to_string();
                e.page.clone().map(|p| (field, p))
            })
            .collect();
        seen.sort_by_key(|s| std::cmp::Reverse(s.2));
        for (_, name, len, viewed) in seen.iter().take(6) {
            let short: String = name.chars().take(28).collect();
            line.push_str(&format!(
                " {short}{}={}",
                if *viewed { "*" } else { "" },
                mib(*len)
            ));
        }
        drop(seen);
        drop(files);
        // What the cache holds present by that field: whether the
        // plaintext held is what the reads ask for.
        let mut by: Vec<(String, u64, u64)> = Vec::new();
        for (field, p) in &views {
            let (held, given) = (p.present_bytes(), p.given_bytes());
            match by.iter_mut().find(|(f, _, _)| f == field) {
                Some((_, b, g)) => {
                    *b += held;
                    *g += given;
                }
                None => by.push((field.clone(), held, given)),
            }
        }
        for (field, given) in v.pager.given_gone() {
            match by.iter_mut().find(|(f, _, _)| *f == field) {
                Some((_, _, g)) => *g += given,
                None => by.push((field, 0, given)),
            }
        }
        by.sort();
        line.push_str(" | present");
        for (field, held, _) in &by {
            line.push_str(&format!(" {field} {}", mib(*held)));
        }
        // And what the evictor has given back of each kind over the
        // budget, the files since dropped among them: what the budget took.
        line.push_str(" | given");
        for (field, _, given) in &by {
            line.push_str(&format!(" {field} {}", mib(*given)));
        }
    }
    eprintln!("{line}");
}

/// The least a cache that follows the platform's room is given: sixty-four
/// mebibytes, as a cache of half the room at the open had at least.
const ROOM_FLOOR: u64 = 64 << 20;

/// The cache of a store whose cipher asks for the shape before the room
/// (`StoreCipher::cache_half_at_open`): half of what `zfile::read_budget`
/// says the process may use, the other half being the kernel's cache of
/// the envelopes a miss is read from.
fn default_cache() -> u64 {
    #[cfg(target_os = "linux")]
    if let Some(b) = super::zfile::read_budget() {
        return (b.budget / 2).max(64 << 20);
    }
    1 << 30
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The stores open in this process whose files the door finds somewhere
/// other than under their names in a directory: the stores under a cipher,
/// found by the directory each gave its engine (`REGISTRY`), and the stores
/// kept in one file, found by their own (`ONES`). The count is of both and
/// is read first, so a process with neither pays a load a file operation.
static VAULTS: AtomicUsize = AtomicUsize::new(0);
static REGISTRY: Mutex<Vec<Arc<Vault>>> = Mutex::new(Vec::new());
static ONES: Mutex<Vec<Arc<One>>> = Mutex::new(Vec::new());

/// A store kept in one file held open: the registration the door finds its
/// table by (`onefile`), for as long as the store is open.
pub(crate) struct OneHeld(Arc<One>);

impl OneHeld {
    pub(crate) fn one(&self) -> &Arc<One> {
        &self.0
    }
}

impl Drop for OneHeld {
    fn drop(&mut self) {
        let mut reg = lock(&ONES);
        if let Some(i) = reg.iter().position(|o| Arc::ptr_eq(o, &self.0)) {
            reg.swap_remove(i);
            VAULTS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// `one` registered as the table of the store in its directory, until the
/// answer is dropped.
pub(crate) fn hold_one(one: Arc<One>) -> OneHeld {
    lock(&ONES).push(one.clone());
    VAULTS.fetch_add(1, Ordering::AcqRel);
    OneHeld(one)
}

/// The table of the plain store kept in one file in `dir`, if there is
/// one: what the door routes a plain store's file operations by, and what
/// a plain store's writers ask before they do what only it wants of them
/// (a piece's room said ahead, an index written before its piece's sync).
/// A store under a cipher has none here, whichever way it is kept: its
/// files are its vault's, and its vault's table holds their envelopes
/// (`one_of`).
pub(crate) fn one_under(dir: &Path) -> Option<Arc<One>> {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    lock(&ONES).iter().find(|o| o.dir == dir).cloned()
}

/// The table of the store kept in one file in `dir`, plain or under a
/// cipher: a plain store's own, or the one a store under a cipher keeps
/// its envelopes in, found by the directory the engine names its files in
/// or by the store's own. For what is the table's whoever's files it
/// holds: the manifest on the device told to it, the segment work's look
/// at what no manifest lists, and a test's reading of its names and
/// places.
pub(crate) fn one_of(dir: &Path) -> Option<Arc<One>> {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    if let Some(o) = lock(&ONES).iter().find(|o| o.dir == dir) {
        return Some(o.clone());
    }
    lock(&REGISTRY)
        .iter()
        .find(|v| v.decoy == dir || v.real == dir)
        .and_then(|v| v.one().cloned())
}

/// A test's crash of the plain store kept in one file in `dir`
/// (`Db::crash`): its table taken out of the door's reach and killed
/// (`One::kill`), so the store opened in its place is the only one that
/// writes the file. A store under a cipher stays held open in the process
/// past the forget, its vault's registration being the forgotten store's,
/// and its table with it: the tests kill one by a copy of its directory.
pub(crate) fn kill_one(dir: &Path) {
    let gone = {
        let mut reg = lock(&ONES);
        reg.iter().position(|o| o.dir == dir).map(|i| {
            VAULTS.fetch_sub(1, Ordering::AcqRel);
            reg.swap_remove(i)
        })
    };
    if let Some(o) = gone {
        o.kill();
    }
}

/// How the store in a directory is kept, for the one caller that writes
/// what names its files (`db::manifest_image`): as files of the directory,
/// under a cipher, or in one file. One load for a store of files.
pub(crate) enum Stored {
    Files,
    Vault(Arc<Vault>),
    One(Arc<One>),
}

/// `Stored` for the store in `dir`.
#[inline]
pub(crate) fn stored(dir: &Path) -> Stored {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return Stored::Files;
    }
    stored_slow(dir)
}

#[cold]
fn stored_slow(dir: &Path) -> Stored {
    if let Some(o) = lock(&ONES).iter().find(|o| o.dir == dir) {
        return Stored::One(o.clone());
    }
    match lock(&REGISTRY).iter().find(|v| v.decoy == dir) {
        Some(v) => Stored::Vault(v.clone()),
        None => Stored::Files,
    }
}

/// What stands behind a path the engine names: nothing, for a file of a
/// directory's own; a store under a cipher; or the table of a store kept
/// in one file, which holds some of its directory's names and not others.
enum Behind {
    Plain,
    Vault(Arc<Vault>, String),
    One(Arc<One>, String),
}

/// `Behind` for `path`: one load where no such store is open.
#[inline]
fn behind(path: &Path) -> Behind {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return Behind::Plain;
    }
    behind_slow(path)
}

#[cold]
fn behind_slow(path: &Path) -> Behind {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Behind::Plain;
    };
    if let Some(o) = lock(&ONES).iter().find(|o| o.dir == dir) {
        return Behind::One(o.clone(), name.to_string());
    }
    match lock(&REGISTRY).iter().find(|v| v.decoy == dir) {
        Some(v) => Behind::Vault(v.clone(), name.to_string()),
        None => Behind::Plain,
    }
}

/// The store under a cipher whose engine was given `dir`, if `dir` is one.
pub(crate) fn under(dir: &Path) -> Option<Arc<Vault>> {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    lock(&REGISTRY).iter().find(|v| v.decoy == dir).cloned()
}

/// The store and the name for a path the engine made in its directory.
fn locate(path: &Path) -> Option<(Arc<Vault>, String)> {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return None;
    }
    let vault = under(path.parent()?)?;
    Some((vault, path.file_name()?.to_str()?.to_string()))
}

/// A store under a cipher held open: the registration its engine's file
/// operations find it by, given up at the drop.
pub(crate) struct Held(Arc<Vault>);

impl Held {
    /// The directory the engine is to name its files in.
    pub(crate) fn dir(&self) -> PathBuf {
        self.0.decoy.clone()
    }

    pub(crate) fn vault(&self) -> &Arc<Vault> {
        &self.0
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.pager.stop();
        self.0.pager.set_grace(None);
        let mut reg = lock(&REGISTRY);
        if let Some(i) = reg.iter().position(|v| Arc::ptr_eq(v, &self.0)) {
            reg.swap_remove(i);
            VAULTS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

use pager::memfile;

/// Another handle on a memory file with a position of its own, as a second
/// open of a file has: through the process's descriptor table, and where
/// that cannot be read, a duplicate, which shares the position.
fn reopen(mem: &OsFile, write: bool, append: bool) -> Result<OsFile> {
    use std::os::fd::AsRawFd;
    let path = format!("/proc/self/fd/{}", mem.as_raw_fd());
    let mut o = OpenOptions::new();
    o.read(true);
    if append {
        o.append(true);
    } else if write {
        o.write(true);
    }
    match o.open(path) {
        Ok(f) => Ok(f),
        Err(_) => mem.try_clone(),
    }
}

/// An envelope's file as it is written, each piece handed to the device as
/// soon as it is in the page cache (`sync_file_range`, a hint that waits
/// for nothing): the device writes the pieces behind while the cipher
/// works on the piece ahead, and the sync at the end finds most of the file
/// written. A plain segment's writer does the same for the same reason
/// (`SegmentOptions::early_writeback`); without it an envelope's sync was
/// the whole file's device time after the whole file's cipher time, end to
/// end.
struct Ahead {
    file: File,
    at: u64,
    hint: bool,
}

impl Write for Ahead {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.file.write_all_at(data, self.at)?;
        if self.hint {
            self.file.start_writeback(self.at, data.len() as u64);
        }
        self.at += data.len() as u64;
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Vault {
    fn new(real: &Path, store: Box<DynAead>, cipher: &StoreCipher) -> Result<Vault> {
        let coming = Arc::new(Coming::default());
        Ok(Vault {
            real: real.to_path_buf(),
            decoy: real.join(MARKER),
            store,
            data: cipher.data.clone(),
            files: Mutex::new(HashMap::new()),
            nonces: Mutex::new(Nonces::new()?),
            persisting: Mutex::new(()),
            mapped: Mutex::new(Vec::new()),
            follows: cipher.cache.is_none() && !cipher.half && cipher.room.is_none(),
            pager: {
                let pager = match (cipher.cache, cipher.half) {
                    (Some(bytes), _) => Pager::new(bytes),
                    (None, true) => Pager::new(default_cache()),
                    (None, false) => Pager::following(Box::new(PlatformRoom(
                        cipher.room.clone(),
                        coming.clone(),
                    ))),
                };
                pager.keep_apart(cipher.apart);
                pager.merge_beside(cipher.beside);
                pager.drops_keep(cipher.drops_keep);
                pager
            },
            coming,
            one: std::sync::OnceLock::new(),
            counts: Counts::default(),
        })
    }

    /// The table of the store's data file, where the store is kept in one.
    pub(crate) fn one(&self) -> Option<&Arc<One>> {
        self.one.get()
    }

    /// The directory the store's own files are in: the log, the manifest,
    /// the marker, and the data file of a store kept in one.
    pub(crate) fn real_dir(&self) -> &Path {
        &self.real
    }

    /// The store is kept in one file from here: its envelopes are made,
    /// read, renamed and removed as files of `one`, at the names they
    /// would have had in the directory. Said once, where the store is
    /// made or where the slot an open takes says so, before any envelope
    /// is written or read.
    pub(crate) fn keep_in(&self, one: Arc<One>) {
        let _ = self.one.set(one);
    }

    // -- where an envelope is kept ---------------------------------------

    /// An envelope made under `name`, to be written.
    fn env_create(&self, name: &str) -> Result<File> {
        match self.one() {
            Some(one) => one.make(name).and_then(File::ext),
            None => OsFile::create(self.real.join(name)).map(File::os),
        }
    }

    /// The envelope under `name` opened to be read as a file.
    fn env_open(&self, name: &str) -> Result<File> {
        match self.one() {
            Some(one) => one
                .reopen(name, false)
                .unwrap_or_else(|| Err(Error::from(ErrorKind::NotFound)))
                .and_then(File::ext),
            None => OsFile::open(self.real.join(name)).map(File::os),
        }
    }

    /// The envelope under `name` as the cache reads it, under `aead`.
    fn env_back(&self, name: &str, aead: Box<DynAead>) -> Result<Back> {
        match self.one() {
            Some(one) => {
                let (map, room, at) = one
                    .map_held(name)
                    .unwrap_or_else(|| Err(Error::from(ErrorKind::NotFound)))?;
                Back::over(map, one.data_path(), at, Some(room), aead)
            }
            None => Back::open(&self.real.join(name), aead),
        }
    }

    /// Whether the store holds an envelope under `name`.
    fn env_exists(&self, name: &str) -> bool {
        match self.one() {
            Some(one) => one.holds(name),
            None => self.real.join(name).exists(),
        }
    }

    /// The envelope under `name` removed, if there is one.
    fn env_remove(&self, name: &str) {
        match self.one() {
            Some(one) => {
                one.remove(name);
            }
            None => {
                let _ = std::fs::remove_file(self.real.join(name));
            }
        }
    }

    /// The envelope under `from` renamed `to`, over whatever was there.
    fn env_rename(&self, from: &str, to: &str) -> Result<()> {
        match self.one() {
            Some(one) => one
                .rename(from, to)
                .ok_or_else(|| Error::from(ErrorKind::NotFound)),
            None => std::fs::rename(self.real.join(from), self.real.join(to)),
        }
    }

    /// The cache the store's files are read through.
    pub(crate) fn pager(&self) -> &Arc<Pager> {
        &self.pager
    }

    fn register(self) -> Result<Held> {
        let mut reg = lock(&REGISTRY);
        if reg.iter().any(|v| v.decoy == self.decoy) {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "the store under a cipher is open in this process already",
            ));
        }
        let vault = Arc::new(self);
        reg.push(vault.clone());
        VAULTS.fetch_add(1, Ordering::AcqRel);
        Ok(Held(vault))
    }

    /// A store made under `cipher` in `real`, which holds nothing: the
    /// store's key drawn, wrapped and written in the marker, and the marker
    /// durable, before anything else is written.
    pub(crate) fn create(real: &Path, cipher: &StoreCipher) -> Result<Held> {
        if std::fs::read_dir(real)?.next().is_some() {
            return Err(refused(
                "a store under a cipher is made in an empty directory, and this one is not",
            ));
        }
        // Before the marker: a platform with no memory files makes no store.
        drop(memfile()?);
        let key = Key::draw()?;
        let mut store_id = [0u8; 16];
        random(&mut store_id)?;
        write_marker(real, cipher, &store_id, &key)?;
        Vault::new(real, (cipher.data)(&key.0), cipher)?.register()
    }

    /// The store in `real` opened under `cipher`: the marker read, and the
    /// store's key unwrapped, which is where a wrong key is refused.
    pub(crate) fn open(real: &Path, cipher: &StoreCipher) -> Result<Held> {
        // A platform with no memory files opens no store, as it makes none.
        drop(memfile()?);
        let bytes = match std::fs::read(real.join(MARKER)) {
            Ok(b) => b,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "a cipher was given to a store that is not kept under one: it would go on in \
                     the clear",
                ));
            }
            Err(e) => return Err(e),
        };
        marker_key_id(&bytes)?;
        let key = unseal(
            &*cipher.master,
            &bytes[..MARKER_HEAD],
            &bytes[MARKER_HEAD..],
        )
        .and_then(|k| Some(Key(k.as_slice().try_into().ok()?)))
        .ok_or_else(|| {
            Error::new(
                ErrorKind::PermissionDenied,
                "the cipher given does not open the store: the key is wrong, or the marker \
                     was changed",
            )
        })?;
        Vault::new(real, (cipher.data)(&key.0), cipher)?.register()
    }

    fn nonce(&self) -> Result<[u8; NONCE]> {
        lock(&self.nonces).take()
    }

    /// What the store has done with its cipher, and what it holds.
    pub(crate) fn counts(&self) -> CipherCounts {
        let files = lock(&self.files)
            .values()
            .filter(|e| e.mem.is_some())
            .count();
        self.counts.read(files as u64, self.pager.counts())
    }

    /// A key wrapped under the store's, bound to what it is the key of.
    fn wrap(&self, of: &[u8], key: &Key) -> Result<[u8; WRAPPED]> {
        Ok(seal(&*self.store, self.nonce()?, of, &key.0)
            .as_slice()
            .try_into()
            .unwrap())
    }

    fn unwrap(&self, of: &[u8], wrapped: &[u8]) -> Option<Key> {
        let k = unseal(&*self.store, of, wrapped)?;
        Some(Key(k.as_slice().try_into().ok()?))
    }

    fn file_aad(token: &Token) -> [u8; 20] {
        let mut a = [0u8; 20];
        a[..4].copy_from_slice(b"file");
        a[4..].copy_from_slice(token);
        a
    }

    // -- the door's kept side ------------------------------------------

    fn make(&self, name: &str) -> Result<OsFile> {
        let mem = memfile()?;
        let handle = mem.try_clone()?;
        // Over a name that exists, as a create truncates: the entry before
        // is forgotten, and an envelope it had is the sweep's at the next
        // open unless a persist writes over it first.
        lock(&self.files).insert(
            name.to_string(),
            Entry {
                mem: Some(Arc::new(mem)),
                page: None,
                disk: None,
            },
        );
        Ok(handle)
    }

    /// `name`, a file just made, is to hold `bytes` (`Coming`).
    fn expect(&self, name: &str, bytes: u64) {
        let mem = lock(&self.files).get(name).and_then(|e| e.mem.clone());
        if let Some(mem) = mem {
            self.coming.expect(&mem, bytes);
            Counts::add(&self.counts.expected, bytes);
        }
    }

    /// The envelope `disk` names, opened under its file's key.
    fn back_of(&self, disk: &Disk) -> Result<Back> {
        let key = self
            .unwrap(&Vault::file_aad(&disk.token), &disk.wrapped)
            .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
        self.env_back(&disk.name(), (self.data)(&key.0))
    }

    /// A view of the file beside the cache, read in order under `hold`
    /// (`pager::Stream`), where the file has its envelope; `None` where it
    /// has not, and is read through the cache.
    fn stream_of(&self, name: &str, hold: &pager::Hold) -> Result<Option<Arc<pager::Stream>>> {
        let (disk, cached) = {
            let files = lock(&self.files);
            let e = files
                .get(name)
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
            match (&e.disk, &e.mem, &e.page) {
                // In its writer's memory file still, or with no envelope:
                // the cache holds it whole, or will once it is viewed.
                (None, _, _) | (Some(_), Some(_), None) => return Ok(None),
                (Some(d), _, page) => (d.clone(), page.clone()),
            }
        };
        Ok(Some(hold.stream(self.back_of(&disk)?, cached, name)?))
    }

    /// The file as it is read: the cache's view of it, made here if it has
    /// none yet. A file still in its writer's memory file is taken whole,
    /// every chunk present, and is the cache's to give back once it has
    /// its envelope; a file known by its envelope alone starts with
    /// nothing present. Every name over the file takes the one view.
    fn page_of(&self, name: &str) -> Result<Arc<Paged>> {
        let mut files = lock(&self.files);
        let e = files
            .get(name)
            .cloned()
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
        if let Some(p) = e.page {
            return Ok(p);
        }
        let page = match (&e.mem, &e.disk) {
            (Some(mem), disk) => {
                let p = Paged::adopt(&self.pager, mem, mem.metadata()?.len(), name)?;
                if let Some(d) = disk {
                    p.back(self.back_of(d)?)?;
                }
                p
            }
            (None, Some(d)) => Paged::sparse(&self.pager, self.back_of(d)?, name)?,
            (None, None) => return Err(Error::from(ErrorKind::NotFound)),
        };
        let backed = page.backed();
        for o in files.values_mut() {
            let same = match (&e.mem, &e.disk) {
                (Some(mem), _) => o.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, mem)),
                (None, Some(d)) => o.mem.is_none() && o.disk.as_ref().is_some_and(|x| x.same(d)),
                (None, None) => false,
            };
            if same && o.page.is_none() {
                o.page = Some(page.clone());
                if backed {
                    // The memory file's descriptor: the view's mapping
                    // keeps the file, and nothing else reads it.
                    o.mem = None;
                }
            }
        }
        Ok(page)
    }

    fn len(&self, name: &str) -> Result<u64> {
        let disk = {
            let files = lock(&self.files);
            let e = files
                .get(name)
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
            if let Some(mem) = &e.mem {
                return Ok(mem.metadata()?.len());
            }
            if let Some(page) = &e.page {
                return Ok(page.size());
            }
            e.disk
                .clone()
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?
        };
        // From the envelope's header, which says how much plaintext it
        // holds: a length asked of a file nothing has read yet.
        let mut head = [0u8; crypt::Header::LEN];
        self.env_open(&disk.name())?.read_exact_at(&mut head, 0)?;
        Ok(crypt::Header::parse(&head)?.plain_len())
    }

    fn remove(&self, name: &str) -> Result<()> {
        let mut files = lock(&self.files);
        let e = files
            .remove(name)
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
        if let Some(d) = e.disk {
            // The envelope goes with the last name over it.
            let shared = files
                .values()
                .any(|o| o.disk.as_ref().is_some_and(|x| x.same(&d)));
            if !shared {
                self.env_remove(&d.name());
                if d.kind == Kind::Seg {
                    // The form file beside a segment goes with it where no
                    // name of the store's holds it: one written and never
                    // attached by this open of the store (`companion`).
                    let z = Disk {
                        kind: Kind::Form,
                        ..d
                    };
                    let held = files
                        .values()
                        .any(|o| o.disk.as_ref().is_some_and(|x| x.same(&z)));
                    if !held {
                        self.env_remove(&z.name());
                    }
                }
            }
        }
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let mut files = lock(&self.files);
        let e = files
            .remove(from)
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
        files.insert(to.to_string(), e);
        Ok(())
    }

    fn link(&self, from: &str, to: &str) -> Result<()> {
        let mut files = lock(&self.files);
        if files.contains_key(to) {
            return Err(Error::from(ErrorKind::AlreadyExists));
        }
        let e = files
            .get(from)
            .cloned()
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
        files.insert(to.to_string(), e);
        Ok(())
    }

    /// A file being written, opened to be mapped by its writer
    /// (`open_mapped`), and the memory file the mapping is counted
    /// against. One that has its envelope is refused, as `reopen_written`
    /// refuses it: it is read through the cache's view.
    fn map_written(&self, name: &str) -> Result<(OsFile, Arc<OsFile>)> {
        let files = lock(&self.files);
        let mem = files
            .get(name)
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?
            .mem
            .clone()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Unsupported,
                    "a file of a store under a cipher that has its envelope is read through \
                     its mapping",
                )
            })?;
        let file = reopen(&mem, false, false)?;
        let mut mapped = lock(&self.mapped);
        match mapped.iter_mut().find(|m| Arc::ptr_eq(&m.mem, &mem)) {
            Some(m) => m.maps += 1,
            None => mapped.push(MappedFile {
                mem: mem.clone(),
                maps: 1,
                owed: None,
            }),
        }
        Ok((file, mem))
    }

    /// A mapping of `mem` has gone (`Mapped`): with the last of them, the
    /// view that waited for its envelope gets it, and the file is the
    /// cache's to give back. A view that cannot be given its envelope
    /// stays whole in memory, which is what it was.
    fn unmapped(&self, mem: &Arc<OsFile>) {
        let _files = lock(&self.files);
        let mut mapped = lock(&self.mapped);
        let Some(i) = mapped.iter().position(|m| Arc::ptr_eq(&m.mem, mem)) else {
            return;
        };
        mapped[i].maps -= 1;
        if mapped[i].maps > 0 {
            return;
        }
        if let Some((page, disk)) = mapped.swap_remove(i).owed {
            if let Ok(back) = self.back_of(&disk) {
                let _ = page.back(back);
            }
        }
    }

    /// A kept file opened to be written again. One that has an envelope is
    /// refused: the envelope would no longer be the file, and nothing
    /// would say so.
    fn rewrite(&self, name: &str, append: bool) -> Result<OsFile> {
        let mem = {
            let files = lock(&self.files);
            let e = files
                .get(name)
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
            if e.disk.is_some() || e.page.is_some() {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "a file of a store under a cipher is written once: it has its envelope, or \
                     a reader",
                ));
            }
            e.mem
                .clone()
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?
        };
        reopen(&mem, true, append)
    }

    /// A kept file opened to be read as a file: one still in its writer's
    /// memory file. A file with its envelope is read through its mapping
    /// (`map`) and has no descriptor to hand out.
    fn reopen_written(&self, name: &str) -> Result<OsFile> {
        let mem = lock(&self.files)
            .get(name)
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?
            .mem
            .clone()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Unsupported,
                    "a file of a store under a cipher that has its envelope is read through \
                     its mapping",
                )
            })?;
        reopen(&mem, false, false)
    }

    /// The kept names, and the names in the directory the engine seals
    /// itself.
    fn names(&self) -> Result<Vec<String>> {
        let mut out: Vec<String> = lock(&self.files).keys().cloned().collect();
        for entry in std::fs::read_dir(&self.real)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if passes(&name) {
                out.push(name);
            }
        }
        Ok(out)
    }

    // -- envelopes -----------------------------------------------------

    /// `mem`'s bytes written to the directory as an envelope under `key`,
    /// and synced where `sync` says so.
    ///
    /// Through the page cache, each piece handed to the device behind the
    /// cipher (`Ahead`), and left there: the kernel's cache of the
    /// ciphertext is what a chunk is decrypted from when the store's own
    /// cache has given it back, and it is the kernel's to reclaim. The
    /// first version dropped it once it was synced, where the store's
    /// cache had taken the plaintext whole, and a store larger than its
    /// cache read every chunk it had given back from the device. Written
    /// past the cache instead (`O_DIRECT`, a megabyte at a time), the same
    /// envelope took half as long again: each piece waits for the device
    /// where the cached pieces go to it together.
    fn write_envelope(&self, mem: &OsFile, disk: &Disk, key: &Key, sync: bool) -> Result<u64> {
        self.write_envelope_at(mem, &disk.name(), key, sync)
    }

    /// `write_envelope` under a name of the caller's choosing. One that is
    /// not synced here rides the next sync of the store's data file where
    /// the store is kept in one (`File::ride`), which is nothing for a file
    /// of the directory's.
    fn write_envelope_at(&self, mem: &OsFile, name: &str, key: &Key, sync: bool) -> Result<u64> {
        let aead = (self.data)(&key.0);
        let len = mem.metadata()?.len();
        // Under a key of the file's own, which has no name of the caller's.
        let opts = EncryptOptions::default();
        let file = self.env_create(name)?;
        // In the store's data file the envelope's room is said before its
        // first byte: its length is its plaintext's and its chunks' tags,
        // known here. A file written there with no word of its size is
        // given its first write's room, and each time it outgrows what
        // stands free behind it -- which beside three merges and the
        // builds is most times -- it is moved, its bytes copied
        // (`onefile::ExtFile::placed`): a load of ten million keys sent
        // the device a third to a half more bytes than a store of files'
        // and its merges were still writing a gigabyte and a half inside
        // the scan pass after a full rewrite, a third of that pass's rate.
        if let Some(one) = self.one() {
            if let Ok(h) = crypt::Header::new(len, opts.chunk_shift, [0; 16], opts.key_id) {
                one.expect(name, h.stored_len());
            }
        }
        let mut out = Ahead {
            file,
            at: 0,
            hint: sync,
        };
        if len == 0 {
            crypt::encrypt(&crate::bytes::SliceBytes(&[]), &mut out, &*aead, &opts)?;
        } else {
            // SAFETY: a mapping of a memory file the store owns, whose
            // writer has finished with it.
            let src = MmapBytes::from_map(unsafe { memmap2::Mmap::map(mem)? });
            crypt::encrypt(&src, &mut out, &*aead, &opts)?;
        }
        let out = out.file;
        if sync {
            out.sync_all()?;
        } else {
            out.ride();
        }
        Counts::add(&self.counts.persisted, 1);
        Counts::add(&self.counts.persisted_bytes, len);
        Ok(len)
    }

    /// Whether the cache holds, or will take, the plaintext of the file in
    /// `mem` whole once it has its envelope: a view a reader has made of it
    /// already is one it keeps, and a file nobody has read yet is taken
    /// where the cache has room for it under its budget. A store that fits
    /// its cache therefore reads what it wrote without decrypting it, and
    /// one that does not leaves a merge's output to be read from its
    /// envelope, by the chunk, when something wants it.
    fn keeps(&self, mem: &Arc<OsFile>) -> bool {
        let viewed = lock(&self.files)
            .values()
            .any(|e| e.page.is_some() && e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, mem)));
        viewed || mem.metadata().is_ok_and(|m| self.pager.room_for(m.len()))
    }

    /// The file in `mem` has its envelope `disk`: from here it is the
    /// cache's. A view of it gets the envelope behind it, so its chunks
    /// may be given back; with `keep` and no view yet, one is made of the
    /// memory file whole; and either way every name over the file gives up
    /// the memory file's descriptor, which only the view's mapping needs.
    ///
    /// `files` is the table, locked by the caller since before it named
    /// the envelope in it, and that is the point: a memory file has one
    /// view. The first version took the lock again here, and a reader
    /// that mapped the file between the naming and this made its view
    /// from the memory file and the envelope both and took the memory
    /// file from every entry, so this found no entry over the memory file,
    /// made a second view of it for nobody, and the evictor, giving back a
    /// megabyte of that view in the instant before it was dropped, punched
    /// the memory file under the first: a segment's hash slots read as
    /// zeroes, its key as absent, and a delete in it was not seen. One
    /// seed in two hundred of the fuzz under a cache of sixteen kilobytes.
    fn settled(
        &self,
        files: &mut HashMap<String, Entry>,
        name: &str,
        mem: &Arc<OsFile>,
        disk: &Disk,
        keep: bool,
    ) -> Result<()> {
        let over = |e: &Entry| e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, mem));
        let viewed = files
            .values()
            .filter(|e| over(e))
            .find_map(|e| e.page.clone());
        // A file its writer still maps is not the cache's to give back
        // yet: its view, the one a reader made or the one made here,
        // waits for its envelope until the mapping has gone (`unmapped`).
        // Whole in memory meanwhile, as it was while it was written, and
        // read with nothing decrypted.
        let mut mapped = lock(&self.mapped);
        let mut held = mapped.iter_mut().find(|m| Arc::ptr_eq(&m.mem, mem));
        let mut give = |p: &Arc<Paged>| -> Result<()> {
            match held.as_mut() {
                Some(m) => m.owed = Some((p.clone(), disk.clone())),
                None => p.back(self.back_of(disk)?)?,
            }
            Ok(())
        };
        let page = match viewed {
            Some(p) => {
                give(&p)?;
                Some(p)
            }
            None if keep => {
                let p = Paged::adopt(&self.pager, mem, mem.metadata()?.len(), name)?;
                give(&p)?;
                Counts::add(&self.counts.adopted, 1);
                Some(p)
            }
            None => {
                Counts::add(&self.counts.left, 1);
                None
            }
        };
        for e in files.values_mut() {
            if over(e) {
                e.page = page.clone();
                e.mem = None;
            }
        }
        Ok(())
    }

    /// Every segment in `pairs` durable in the directory as an envelope,
    /// each with its ordered index beside it where the store holds one, and
    /// the token and wrapped key of each segment, in order: what a manifest
    /// that names them is written after and carries.
    ///
    /// A segment is synced and its index is not, as a plain store leaves
    /// them: an index is a function of its segment's keys, and an open that
    /// finds it missing or failing its check composes it again.
    ///
    /// `named` is a manifest's call, for which a name the store does not
    /// hold is an error: the manifest would name nothing. Without it the
    /// call is the one a seal or a merge makes where a plain store fsyncs,
    /// ahead of any manifest, and a name gone by then -- a piece a
    /// promotion has linked under another name and unlinked -- is passed
    /// over, since whichever manifest names the file writes it.
    ///
    /// `dir_sync` is the directory's own fsync after an envelope was
    /// made: off for a merge's piece written as it is finished
    /// (`persist_one`), whose merge syncs the directory once at its end.
    pub(crate) fn persist(
        &self,
        pairs: &[(String, Option<String>)],
        named: bool,
    ) -> Result<Vec<FileKey>> {
        self.persist_with(pairs, named, true)
    }

    fn persist_with(
        &self,
        pairs: &[(String, Option<String>)],
        named: bool,
        dir_sync: bool,
    ) -> Result<Vec<FileKey>> {
        let _one = lock(&self.persisting);
        let mut out = Vec::with_capacity(pairs.len());
        let mut made = false;
        for (seg, ord) in pairs {
            let Some(entry) = lock(&self.files).get(seg).cloned() else {
                if named {
                    return Err(Error::new(
                        ErrorKind::NotFound,
                        format!("a manifest names {seg}, which the store does not hold"),
                    ));
                }
                continue;
            };
            let disk = match entry.disk {
                Some(d) => d,
                None => {
                    let mem = entry.mem.ok_or_else(|| Error::from(ErrorKind::NotFound))?;
                    let key = Key::draw()?;
                    let mut token = [0u8; 16];
                    random(&mut token)?;
                    let disk = Disk {
                        token,
                        wrapped: self.wrap(&Vault::file_aad(&token), &key)?,
                        kind: Kind::Seg,
                    };
                    // In the store's data file the segment's ordered index
                    // is written ahead of it, with no sync of its own, and
                    // the segment's sync covers both: it is a sync of the
                    // data file, made through the segment's own handle,
                    // which answers its writes' errors as no other
                    // handle's sync does once one has reported them. One
                    // flush of the device a segment, where a sync of the
                    // segment and then one for its index were two; and
                    // the manifest this call is for lists the index, since
                    // a manifest lists what a sync has covered and an
                    // index it does not list is room the next open gives
                    // away. A plain store kept in one file writes its
                    // index ahead of its piece's sync for the same reason
                    // (`Emitter::leave_piece`).
                    let ahead = match (self.one(), ord) {
                        (Some(_), Some(ord)) => self.ord_envelope(ord, &disk, &key)?,
                        _ => None,
                    };
                    let keep = self.keeps(&mem);
                    #[cfg(all(test, target_os = "linux"))]
                    tests::persist_gate(&self.real);
                    // A write that fails here leaves the index's envelope
                    // under a token no segment has: no manifest lists it,
                    // and it is free room to the open after.
                    self.write_envelope(&mem, &disk, &key, true)?;
                    made = true;
                    if named {
                        Counts::add(&self.counts.late, 1);
                    }
                    let mut files = lock(&self.files);
                    // Every name over the same memory file: a link made
                    // before the envelope was.
                    let mut held = false;
                    for e in files.values_mut() {
                        if e.disk.is_none() && e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, &mem))
                        {
                            e.disk = Some(disk.clone());
                            held = true;
                        }
                    }
                    if held {
                        // Under the one lock with the naming above.
                        self.settled(&mut files, seg, &mem, &disk, keep)?;
                    } else {
                        // Removed while its envelope was written: nobody's.
                        self.env_remove(&disk.name());
                    }
                    drop(files);
                    if let (Some(ord), Some((omem, odisk, okeep))) = (ord, ahead) {
                        self.ord_settled(ord, &omem, &odisk, okeep)?;
                    }
                    disk
                }
            };
            // The index of a store of files, written behind its segment;
            // and in either store one composed after its segment had its
            // envelope, which an open that found none makes.
            if let Some(ord) = ord {
                let waits = matches!(
                    lock(&self.files).get(ord),
                    Some(Entry {
                        mem: Some(_),
                        disk: None,
                        ..
                    })
                );
                if waits {
                    let key = self
                        .unwrap(&Vault::file_aad(&disk.token), &disk.wrapped)
                        .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
                    if let Some((mem, odisk, keep)) = self.ord_envelope(ord, &disk, &key)? {
                        made = true;
                        self.ord_settled(ord, &mem, &odisk, keep)?;
                    }
                }
            }
            out.push((disk.token, disk.wrapped));
        }
        // The directory's own fsync, for the names of the envelopes made.
        // In the store's data file an envelope has no name of the
        // directory's: a segment's was synced through its own handle with
        // its index's ahead of it, and an index written behind its
        // segment's sync -- one an open composed again -- is covered by the
        // store's next sync of the data file and listed by the manifest
        // after it (`Maint::one_look`), as a plain store's is.
        if self.one().is_none() && made && dir_sync {
            OsFile::open(&self.real)?.sync_all()?;
        }
        Ok(out)
    }

    /// The ordered index `ord` of the segment under `disk`, written as an
    /// envelope under the segment's token and `key` with no sync of its
    /// own, where the store holds it in memory with none: the memory
    /// file, the envelope's place and whether the cache keeps the
    /// plaintext, for `ord_settled`. Nothing where the store holds no
    /// such file or it has its envelope.
    fn ord_envelope(
        &self,
        ord: &str,
        disk: &Disk,
        key: &Key,
    ) -> Result<Option<(Arc<OsFile>, Disk, bool)>> {
        let e = lock(&self.files).get(ord).cloned();
        let Some(Entry {
            mem: Some(mem),
            disk: None,
            ..
        }) = e
        else {
            return Ok(None);
        };
        let odisk = Disk {
            kind: Kind::Ord,
            ..disk.clone()
        };
        let keep = self.keeps(&mem);
        self.write_envelope(&mem, &odisk, key, false)?;
        Ok(Some((mem, odisk, keep)))
    }

    /// `ord`, whose envelope `ord_envelope` wrote, named by it and handed
    /// to the cache (`settled`), where the store still holds the memory
    /// file under that name.
    fn ord_settled(&self, ord: &str, mem: &Arc<OsFile>, odisk: &Disk, keep: bool) -> Result<()> {
        let mut files = lock(&self.files);
        let mut held = false;
        if let Some(e) = files.get_mut(ord) {
            if e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, mem)) {
                e.disk = Some(odisk.clone());
                held = true;
            }
        }
        if held {
            self.settled(&mut files, ord, mem, odisk, keep)?;
        }
        Ok(())
    }

    /// `seg`'s compact form, the file the store holds as `form`, written to
    /// the directory as an envelope beside the segment's, under the
    /// segment's token and key, and from there the cache's to give back
    /// and read again: `x-<token>.z`. Whether it was: a segment with no
    /// envelope yet has no token to write beside, and a form with no
    /// memory file has its envelope or is gone, each of which leaves the
    /// form as it is.
    ///
    /// The envelope is written under a temp name and renamed over the
    /// name, where a segment's is written in place: a segment's token is
    /// drawn for its one envelope, and a form file may be written again
    /// for the same segment -- in another coding, or after a page of the
    /// first failed a check -- while a reader still holds the view over
    /// the first, whose file a write in place would cut to nothing under
    /// it. No manifest names a form file: it is a function of its
    /// segment, found beside it at the next open (`companion`) and built
    /// again where it is missing or fails its checks, so it is synced only
    /// where the caller says and the directory never is. A second
    /// envelope under one file's key meets no nonce of the first, each
    /// envelope's nonces being behind an id drawn for it (`crypt`).
    ///
    /// `cold` is a form file nothing reads yet, one built ahead of the
    /// controller's buy beside a raw form that fits: its plaintext is not
    /// taken into the store's cache, where it would stand in the raw
    /// form's place, and the kernel's cache of its envelope is written
    /// back and dropped, as a plain store drops such a file's pages
    /// (`zfile::BuildPlan::drop_output`).
    fn persist_form(&self, seg: &str, form: &str, sync: bool, cold: bool) -> Result<bool> {
        // Outside the lock a segment's persist takes: a form file is one
        // builder's, and its envelope, megabytes on a thread at idle
        // priority, is not to stand in front of a seal's.
        let (sdisk, mem) = {
            let files = lock(&self.files);
            let Some(sdisk) = files
                .get(seg)
                .and_then(|e| e.disk.clone())
                .filter(|d| d.kind == Kind::Seg)
            else {
                return Ok(false);
            };
            match files.get(form) {
                Some(Entry {
                    mem: Some(mem),
                    disk: None,
                    ..
                }) => (sdisk, mem.clone()),
                _ => return Ok(false),
            }
        };
        let key = self
            .unwrap(&Vault::file_aad(&sdisk.token), &sdisk.wrapped)
            .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
        let zdisk = Disk {
            kind: Kind::Form,
            ..sdisk
        };
        let keep = !cold && self.keeps(&mem);
        let name = zdisk.name();
        let tmp = format!("{name}{ENVELOPE_TMP}");
        self.write_envelope_at(&mem, &tmp, &key, sync)?;
        if cold {
            if let Ok(f) = self.env_open(&tmp) {
                // Written back, waited for by range, which asks for no
                // flush of the device, and the clean pages dropped.
                let _ = f.written_back(0, 0);
                f.drop_all();
            }
        }
        self.env_rename(&tmp, &name).inspect_err(|_| {
            self.env_remove(&tmp);
        })?;
        let mut files = lock(&self.files);
        let held = match files.get_mut(form) {
            Some(e) if e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, &mem)) => {
                e.disk = Some(zdisk.clone());
                true
            }
            _ => false,
        };
        if held {
            // Under the one lock with the naming above, as a segment's is.
            self.settled(&mut files, form, &mem, &zdisk, keep)?;
        } else {
            // Removed while its envelope was written: nobody's.
            self.env_remove(&name);
        }
        Ok(held)
    }

    /// The form file beside `seg` known to the store as `form`, where the
    /// directory holds one and the store does not know it yet: an open
    /// reads the segments its manifest names and nothing says which of
    /// them has a form file but the file. Whether the store holds `form`
    /// now.
    fn companion(&self, seg: &str, form: &str) -> bool {
        let mut files = lock(&self.files);
        if files.contains_key(form) {
            return true;
        }
        let Some(sdisk) = files
            .get(seg)
            .and_then(|e| e.disk.clone())
            .filter(|d| d.kind == Kind::Seg)
        else {
            return false;
        };
        let zdisk = Disk {
            kind: Kind::Form,
            ..sdisk
        };
        if !self.env_exists(&zdisk.name()) {
            return false;
        }
        // The view another name over the same envelope has already.
        let page = files
            .values()
            .find(|e| e.page.is_some() && e.disk.as_ref().is_some_and(|x| x.same(&zdisk)))
            .and_then(|e| e.page.clone());
        files.insert(
            form.to_string(),
            Entry {
                mem: None,
                page,
                disk: Some(zdisk),
            },
        );
        true
    }

    /// A segment a manifest names, with the envelope it is in, and its
    /// ordered index where the directory holds one: known to the store
    /// from here, and decrypted when something first reads it.
    fn adopt(&self, seg: &str, token: Token, wrapped: [u8; WRAPPED], ord: Option<&str>) {
        let mut files = lock(&self.files);
        let disk = Disk {
            token,
            wrapped,
            kind: Kind::Seg,
        };
        // The view another name over the same envelope has already.
        let page_of = |files: &HashMap<String, Entry>, d: &Disk| {
            files
                .values()
                .find(|e| e.page.is_some() && e.disk.as_ref().is_some_and(|x| x.same(d)))
                .and_then(|e| e.page.clone())
        };
        if !files.contains_key(seg) {
            let page = page_of(&files, &disk);
            files.insert(
                seg.to_string(),
                Entry {
                    mem: None,
                    page,
                    disk: Some(disk.clone()),
                },
            );
        }
        if let Some(ord) = ord {
            let odisk = Disk {
                kind: Kind::Ord,
                ..disk
            };
            if !files.contains_key(ord) && self.env_exists(&odisk.name()) {
                let page = page_of(&files, &odisk);
                files.insert(
                    ord.to_string(),
                    Entry {
                        mem: None,
                        page,
                        disk: Some(odisk),
                    },
                );
            }
        }
    }

    /// `adopt` for every segment a manifest names, each with its ordered
    /// index's name, and the file its slot lists for it; a slot that lists
    /// more files or fewer than its manifest names is refused whole, since
    /// which file is whose is then nobody's to say.
    pub(crate) fn adopt_all(
        &self,
        segs: &[(String, Option<String>)],
        files: Vec<FileKey>,
    ) -> Result<()> {
        if segs.len() != files.len() {
            return Err(refused("manifest is truncated"));
        }
        for ((seg, ord), (token, wrapped)) in segs.iter().zip(files) {
            self.adopt(seg, token, wrapped, ord.as_deref());
        }
        Ok(())
    }

    /// Whether the directory holds a segment's envelope: for an open that
    /// read no slot, and so knows none of them.
    fn holds_segments(&self) -> Result<bool> {
        // A data file with anything past its head is a store's kept in
        // one file, whose envelopes only a slot lists the places of.
        if std::fs::metadata(self.real.join(onefile::NAME)).is_ok_and(|m| m.len() > onefile::HEAD) {
            return Ok(true);
        }
        for entry in std::fs::read_dir(&self.real)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if matches!(disk_parse(&name), Some((_, Kind::Seg))) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether `name`'s envelope -- the one a slot read named for it -- is
    /// in the directory.
    fn on_device(&self, name: &str) -> bool {
        lock(&self.files)
            .get(name)
            .and_then(|e| e.disk.clone())
            .is_some_and(|d| match self.one() {
                // Its place holding what the slot listed there
                // (`One::on_device`).
                Some(one) => one.on_device(&d.name()).unwrap_or(false),
                None => self.real.join(d.name()).exists(),
            })
    }

    /// What the manifest of sequence `seq` that names `files` carries
    /// behind its own bytes where the store is kept in one file: the
    /// table's section, the place of each segment's envelope under the
    /// name its token gives it and, where a sync has covered them, of its
    /// ordered index's and its form file's (`One::image`). `None` for a
    /// store whose envelopes are files of the directory.
    pub(crate) fn places(&self, seq: u64, files: &[FileKey]) -> Result<Option<Vec<u8>>> {
        let Some(one) = self.one() else {
            return Ok(None);
        };
        let segs: Vec<String> = files.iter().map(|(t, _)| disk_name(t, Kind::Seg)).collect();
        let with: Vec<String> = files.iter().flat_map(|(t, _)| riders(t)).collect();
        one.image(seq, &segs, &with).map(Some)
    }

    /// What the table may hold beside each of the segments the store has
    /// under `segs` that has its envelope: `places`' second list for
    /// them, for the segment work's look at what no manifest lists yet
    /// (`Maint::one_look`).
    pub(crate) fn beside<'a>(&self, segs: impl Iterator<Item = &'a str>) -> Vec<String> {
        let files = lock(&self.files);
        segs.filter_map(|s| {
            files
                .get(s)
                .and_then(|e| e.disk.as_ref())
                .filter(|d| d.kind == Kind::Seg)
                .map(|d| d.token)
        })
        .flat_map(|t| riders(&t))
        .collect()
    }

    /// The ordered index of each segment in `segs` known to the store
    /// where its envelope is a file of the store's data file: for an open
    /// of a store kept in one file, whose table is made what the slot
    /// taken lists only once that slot is chosen, after the slots were
    /// read and their segments made known (`adopt`), when no index was
    /// there to be found.
    pub(crate) fn adopt_ords(&self, segs: &[(String, Option<String>)]) {
        let mut files = lock(&self.files);
        for (seg, ord) in segs {
            let (Some(ord), Some(disk)) = (ord, files.get(seg).and_then(|e| e.disk.clone())) else {
                continue;
            };
            let odisk = Disk {
                kind: Kind::Ord,
                ..disk
            };
            if !files.contains_key(ord) && self.env_exists(&odisk.name()) {
                files.insert(
                    ord.clone(),
                    Entry {
                        mem: None,
                        page: None,
                        disk: Some(odisk),
                    },
                );
            }
        }
    }

    /// Every envelope in the directory that no file of the store is in,
    /// removed: one a crash came after and no manifest named, as a plain
    /// store sweeps a segment its manifest does not name. For the open,
    /// once the manifests have been read.
    ///
    /// And the marker's temp name, which a creation's crash or a rewrap
    /// leaves and which holds nothing a marker does not: here, where the
    /// open holds the store's lock and has checked its slots, and not in
    /// `open`, which runs before the lock, so that an open refused beside
    /// a live store writes nothing in its directory. A rewrap holds no lock
    /// (`rewrap`), so one that runs in another process while a store opens
    /// can have its temp name removed under it, and fails at its rename
    /// with the marker as it was.
    pub(crate) fn sweep(&self) -> Result<usize> {
        let _ = std::fs::remove_file(self.real.join(format!("{MARKER}.tmp")));
        // A data file beside a store that is not kept in one is nobody's:
        // a creation's that never wrote its manifest. One that holds
        // anything beside no slot was refused before this
        // (`holds_segments`).
        if self.one().is_none() {
            let _ = std::fs::remove_file(self.real.join(onefile::NAME));
        }
        let files = lock(&self.files);
        let mut swept = 0;
        for entry in std::fs::read_dir(&self.real)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("x-") && name.ends_with(ENVELOPE_TMP) {
                // An envelope a crash came while it was written under its
                // temp name (`persist_form`): nobody's.
                if std::fs::remove_file(self.real.join(&name)).is_ok() {
                    swept += 1;
                }
                continue;
            }
            let Some((token, kind)) = disk_parse(&name) else {
                continue;
            };
            // A form file is held by its segment as well as by its own
            // name: the store learns of it where it is first attached
            // (`companion`), which is after this.
            let held = files.values().any(|e| {
                e.disk.as_ref().is_some_and(|d| {
                    d.token == token
                        && (d.kind == kind || (kind == Kind::Form && d.kind == Kind::Seg))
                })
            });
            if !held && std::fs::remove_file(self.real.join(&name)).is_ok() {
                swept += 1;
            }
        }
        Counts::add(&self.counts.swept, swept as u64);
        Ok(swept)
    }

    // -- the manifest ----------------------------------------------------

    /// A manifest's bytes sealed for its slot, with the envelope and the
    /// wrapped key of each segment it names behind them:
    /// `SUPDBMAE\x01 | u64 seq | u32 len | nonce | ciphertext | tag`, the
    /// plaintext being `u32 len | manifest | (token | wrapped key)*` and
    /// the sequence, which is in the clear, bound by the tag.
    pub(crate) fn seal_manifest(
        &self,
        seq: u64,
        manifest: &[u8],
        files: &[FileKey],
    ) -> Result<Vec<u8>> {
        let mut body = Vec::with_capacity(4 + manifest.len() + files.len() * (16 + WRAPPED));
        body.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
        body.extend_from_slice(manifest);
        for (token, wrapped) in files {
            body.extend_from_slice(token);
            body.extend_from_slice(wrapped);
        }
        let sealed = seal(&*self.store, self.nonce()?, &slot_aad(seq), &body);
        let mut out = Vec::with_capacity(SLOT_HEAD + sealed.len());
        out.extend_from_slice(SLOT_MAGIC);
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&(sealed.len() as u32).to_le_bytes());
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// A slot's manifest and the files behind it, or the reason it is not
    /// one: a slot torn, changed, or sealed under another store's key.
    pub(crate) fn open_manifest(&self, slot: &[u8]) -> Result<(Vec<u8>, Vec<FileKey>)> {
        if slot.len() < SLOT_HEAD || &slot[..SLOT_MAGIC.len()] != SLOT_MAGIC {
            return Err(refused("manifest magic is wrong"));
        }
        let seq = u64::from_le_bytes(slot[9..17].try_into().unwrap());
        let len = u32::from_le_bytes(slot[17..21].try_into().unwrap()) as usize;
        let sealed = slot
            .get(SLOT_HEAD..SLOT_HEAD + len)
            .ok_or_else(|| refused("manifest is truncated"))?;
        let body = unseal(&*self.store, &slot_aad(seq), sealed)
            .ok_or_else(|| refused("manifest failed its check"))?;
        let short = || refused("manifest is truncated");
        let mlen = u32::from_le_bytes(body.get(..4).ok_or_else(short)?.try_into().unwrap());
        let manifest = body.get(4..4 + mlen as usize).ok_or_else(short)?;
        let (rest, over) = body[4 + mlen as usize..].as_chunks::<{ 16 + WRAPPED }>();
        if !over.is_empty() {
            return Err(short());
        }
        let files = rest
            .iter()
            .map(|c| (c[..16].try_into().unwrap(), c[16..].try_into().unwrap()))
            .collect();
        Ok((manifest.to_vec(), files))
    }

    // -- the log -------------------------------------------------------

    /// The cipher of a log file begun now, and the header bytes that hold
    /// its key: a key drawn for the file and wrapped under the store's.
    pub(crate) fn log_begin(&self, id: u64) -> Result<(LogCipher, [u8; WRAPPED])> {
        let key = Key::draw()?;
        let wrapped = self.wrap(&log_aad(id), &key)?;
        Ok((LogCipher::new((self.data)(&key.0), id)?, wrapped))
    }

    /// The cipher of a log file from the wrapped key in its header, or
    /// nothing where that is not this store's key for this file.
    pub(crate) fn log_resume(&self, id: u64, wrapped: &[u8]) -> Option<LogCipher> {
        let key = self.unwrap(&log_aad(id), wrapped)?;
        LogCipher::new((self.data)(&key.0), id).ok()
    }
}

impl Counts {
    fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }

    fn read(&self, files: u64, cache: pager::PagerCounts) -> CipherCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        CipherCounts {
            files,
            persisted: get(&self.persisted),
            persisted_bytes: get(&self.persisted_bytes),
            adopted: get(&self.adopted),
            left: get(&self.left),
            swept: get(&self.swept),
            late: get(&self.late),
            expected: get(&self.expected),
            cache,
        }
    }
}

/// What a store under a cipher has done with it since it was opened
/// (`Db::cipher_counts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CipherCounts {
    /// The store's files in a writer's hands now: a file being written, or
    /// written and not yet in the directory as its envelope. Each is a
    /// memory file with a descriptor of its own until it is.
    pub files: u64,
    /// Files written to the directory as envelopes, and the plaintext
    /// bytes sealed into them.
    pub persisted: u64,
    pub persisted_bytes: u64,
    /// Files whose plaintext the cache took whole from their writer, there
    /// being room for it, and files it left to be read from their
    /// envelopes.
    pub adopted: u64,
    pub left: u64,
    /// Envelopes no manifest named, removed at the open.
    pub swept: u64,
    /// Segments whose envelope was written by the manifest that names
    /// them, on the thread every landing goes through: a file is sealed
    /// where it is finished, by the seal, the run or the merge that wrote
    /// it, and this counts the ones that were not -- a piece renamed
    /// while its writer was on its way to seal it.
    pub late: u64,
    /// The bytes the store's writers announced before they wrote them: a
    /// merge's outputs and a form file's build, each said where its file
    /// was opened, for the cache to give way ahead of.
    pub expected: u64,
    /// The cache the store reads through: what it may hold and holds, and
    /// what it has decrypted and given back.
    pub cache: PagerCounts,
}

const SLOT_MAGIC: &[u8; 9] = b"SUPDBMAE\x01";
const SLOT_HEAD: usize = 9 + 8 + 4;

fn slot_aad(seq: u64) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[..8].copy_from_slice(b"manifest");
    a[8..].copy_from_slice(&seq.to_le_bytes());
    a
}

fn log_aad(id: u64) -> [u8; 12] {
    let mut a = [0u8; 12];
    a[..4].copy_from_slice(b"log\0");
    a[4..].copy_from_slice(&id.to_le_bytes());
    a
}

/// What precedes a record's ciphertext in a log: its length and its nonce.
pub(crate) const RECORD_HEAD: usize = 4 + NONCE;

/// A log file's cipher: the file's own key, and the nonces of the records
/// this open of it writes -- eight bytes drawn at the open and a count, so
/// a file reopened after a crash never meets a nonce its torn tail used.
pub(crate) struct LogCipher {
    aead: Box<DynAead>,
    id: u64,
    nonces: Nonces,
}

impl LogCipher {
    fn new(aead: Box<DynAead>, id: u64) -> Result<LogCipher> {
        Ok(LogCipher {
            aead,
            id,
            nonces: Nonces::new()?,
        })
    }

    /// What a record's tag covers beside its bytes: the file, where in it
    /// the record lies, and its length -- a record moved, repeated or cut
    /// is not the record.
    fn aad(&self, at: u64, len: u32) -> [u8; 20] {
        let mut a = [0u8; 20];
        a[..8].copy_from_slice(&self.id.to_le_bytes());
        a[8..16].copy_from_slice(&at.to_le_bytes());
        a[16..].copy_from_slice(&len.to_le_bytes());
        a
    }

    /// `plain` as the record that lands at `at` in the file, into `out`:
    /// `len u32 | nonce | ciphertext | tag`.
    pub(crate) fn seal_into(&mut self, at: u64, plain: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let len = u32::try_from(plain.len())
            .map_err(|_| refused("a commit too large for one log record"))?;
        let nonce = self.nonces.take()?;
        out.clear();
        out.reserve(RECORD_HEAD + plain.len() + TAG);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(plain);
        let tag = self
            .aead
            .encrypt(&nonce, &self.aad(at, len), &mut out[RECORD_HEAD..]);
        out.extend_from_slice(&tag);
        Ok(())
    }

    /// The records of a log file's bytes from `from` on, decrypted and
    /// appended to `plain` one after another until the first that is not
    /// whole or fails its check, which is where the log ends; and for each,
    /// where its plaintext ends in `plain` and where it ends in the file.
    /// A file's unwritten tail reads as zeroes, a record of no bytes whose
    /// tag is zeroes, and it is the tag that refuses it: nothing here asks
    /// what a length is.
    pub(crate) fn open_records(
        &self,
        file: &[u8],
        from: usize,
        plain: &mut Vec<u8>,
    ) -> Vec<(usize, u64)> {
        let mut ends = Vec::new();
        let mut p = from;
        while file.len() - p >= RECORD_HEAD + TAG {
            let len = u32::from_le_bytes(file[p..p + 4].try_into().unwrap());
            let body = p + RECORD_HEAD;
            let Some(end) = body
                .checked_add(len as usize)
                .and_then(|e| e.checked_add(TAG))
                .filter(|&e| e <= file.len())
            else {
                break;
            };
            let nonce: [u8; NONCE] = file[p + 4..body].try_into().unwrap();
            let tag: [u8; TAG] = file[end - TAG..end].try_into().unwrap();
            let at = plain.len();
            plain.extend_from_slice(&file[body..end - TAG]);
            if !self
                .aead
                .decrypt(&nonce, &self.aad(p as u64, len), &mut plain[at..], &tag)
            {
                plain.truncate(at);
                break;
            }
            ends.push((plain.len(), end as u64));
            p = end;
        }
        ends
    }
}

// ------------------------------------------------------------- the door --

/// A file as the door hands it to the engine: every open and create below
/// answers one, and what the engine does with a file it does through this
/// type's methods, each the standard library's call or the system call the
/// engine made on the descriptor itself before the type stood here
/// (`pwritev`, `sync_file_range`, `posix_fadvise`, a second description by
/// `/proc/self/fd`, `kcmp`). So the engine names no descriptor, and a file
/// that is not a file of the directory's own has one place to be told from
/// one that is: a plain store's file and a memory file of a store under a
/// cipher are the standard library's file under it, and a file of a store
/// kept in one file is an extent of that file (`onefile::ExtFile`), every
/// offset the engine names the file's own.
///
/// The two are told apart by one test a call, of `ext`, and a caller that
/// makes a test of its own before every call -- a segment's writer, whose
/// every write and sync asks one byte first -- carries the answer there
/// and calls the directory's file with no test (`plain`): `os` is at one
/// place in both kinds, and under a file of a data file it is a
/// descriptor of nothing (`nowhere`), which takes no write and syncs
/// nothing, so a call that should have asked fails where it is made, and
/// never writes the data file at an offset that is another file's.
pub(crate) struct File {
    os: OsFile,
    /// Dropped by `File`'s own drop, out of line, where there is one: a
    /// file of the directory's own is closed on one test, and its drop
    /// stays small enough to be written into whoever drops it.
    ext: std::mem::ManuallyDrop<Option<Box<ExtFile>>>,
}

impl Drop for File {
    #[inline]
    fn drop(&mut self) {
        if self.ext.is_some() {
            self.drop_ext();
        }
    }
}

/// Which of the two a file is, as each of its methods asks.
enum Kept<'a> {
    Os(&'a OsFile),
    Ext(&'a ExtFile),
}

/// A descriptor of nothing, read-only on `/dev/null`: what stands at
/// `File::os` under a file of a store's data file. A write through it is
/// refused, a sync of it is an error, a read of it is the end of a file,
/// and an advice or a writeback over it is of no page.
fn nowhere() -> Result<OsFile> {
    static NOWHERE: std::sync::OnceLock<OsFile> = std::sync::OnceLock::new();
    let f = match NOWHERE.get() {
        Some(f) => f,
        None => {
            let f = OsFile::open("/dev/null")?;
            NOWHERE.get_or_init(|| f)
        }
    };
    f.try_clone()
}

/// One `pwritev` of `iov` at `at` on `f`: what the call returned.
#[inline(always)]
fn os_pwritev(f: &OsFile, iov: &[std::io::IoSlice<'_>], at: u64) -> isize {
    use std::os::fd::AsRawFd;
    // SAFETY: `IoSlice` is ABI-compatible with `iovec` on unix, and every
    // slice it names is borrowed for the call.
    unsafe {
        libc::pwritev(
            f.as_raw_fd(),
            iov.as_ptr() as *const libc::iovec,
            iov.len().min(1024) as libc::c_int,
            at as libc::off_t,
        )
    }
}

/// A file as one of the directory's own, with no look at which it is
/// (`File::plain`): the calls a writer makes beside a test of its own.
pub(crate) struct Plain<'a>(&'a OsFile);

impl Plain<'_> {
    /// `File::pwritev`.
    #[inline(always)]
    pub(crate) fn pwritev(&self, iov: &[std::io::IoSlice<'_>], at: u64) -> isize {
        os_pwritev(self.0, iov, at)
    }

    /// `File::write_all_at`.
    #[inline]
    pub(crate) fn write_all_at(&self, buf: &[u8], at: u64) -> Result<()> {
        std::os::unix::fs::FileExt::write_all_at(self.0, buf, at)
    }

    /// `Write::write_all`.
    #[inline]
    pub(crate) fn write_all(&self, buf: &[u8]) -> Result<()> {
        let mut f = self.0;
        f.write_all(buf)
    }

    /// `File::sync_data`.
    #[inline]
    pub(crate) fn sync_data(&self) -> Result<()> {
        self.0.sync_data()
    }

    /// `File::start_writeback`.
    #[inline]
    pub(crate) fn start_writeback(&self, at: u64, len: u64) {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: a writeback over a range of an open file; the call
            // reads and writes no memory of ours.
            let _ = unsafe {
                libc::sync_file_range(
                    self.0.as_raw_fd(),
                    at as libc::off64_t,
                    len as libc::off64_t,
                    libc::SYNC_FILE_RANGE_WRITE,
                )
            };
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (at, len);
    }
}

impl File {
    #[inline]
    fn os(f: OsFile) -> File {
        File {
            os: f,
            ext: std::mem::ManuallyDrop::new(None),
        }
    }

    fn ext(f: ExtFile) -> Result<File> {
        Ok(File {
            os: nowhere()?,
            ext: std::mem::ManuallyDrop::new(Some(Box::new(f))),
        })
    }

    #[cold]
    #[inline(never)]
    fn drop_ext(&mut self) {
        // SAFETY: dropped once, here, from the file's own drop.
        unsafe { std::mem::ManuallyDrop::drop(&mut self.ext) }
    }

    #[inline(always)]
    fn kept(&self) -> Kept<'_> {
        match &*self.ext {
            None => Kept::Os(&self.os),
            Some(x) => Kept::Ext(x),
        }
    }

    /// The file as one of the directory's own, with no look at which it
    /// is: for a caller that has asked (`is_placed`) and carries the
    /// answer on a test it makes anyway. Under a file of a store's data
    /// file this is a descriptor of nothing, and every call on it fails
    /// or does nothing.
    #[inline(always)]
    pub(crate) fn plain(&self) -> Plain<'_> {
        Plain(&self.os)
    }

    /// Whether this is a file of a store's data file, which `plain`'s
    /// calls are not for.
    #[inline]
    pub(crate) fn is_placed(&self) -> bool {
        self.ext.is_some()
    }

    /// A file of a store's data file hands each of its writes to the
    /// device as it lands (`start_writeback` over the bytes written)
    /// where `on`: what a segment's writer does for a file of its own
    /// after each piece, said once here since such a file's writes are
    /// made off the writer's fast path. Nothing for a file of the
    /// directory's own.
    pub(crate) fn hint_writes(&self, on: bool) {
        if let Kept::Ext(x) = self.kept() {
            x.hint_writes(on);
        }
    }

    /// The standard library's file under this one, for the callers that
    /// hand it to something that takes one: a mapping made of it
    /// (`memmap2`). Each is of a file the directory holds under its own
    /// name -- the log, a run of ordered ingest -- and a file of a store's
    /// data file, which has none of its own, is refused: its mapping is
    /// `map`'s.
    #[inline]
    pub(crate) fn as_os(&self) -> Result<&OsFile> {
        match self.kept() {
            Kept::Os(f) => Ok(f),
            Kept::Ext(_) => Err(Error::other(
                "a file of the store's data file has no file of its own",
            )),
        }
    }

    /// The file mapped whole for reading, as `vault::map` maps it by its
    /// path: a mapping of the file under this handle, or of its extent.
    pub(crate) fn map(&self) -> Result<MmapBytes> {
        match self.kept() {
            // SAFETY: the contract `MmapBytes::open` takes -- the file is
            // not cut under the mapping.
            Kept::Os(f) => Ok(MmapBytes::from_map(unsafe { memmap2::Mmap::map(f)? })),
            Kept::Ext(x) => x.map(),
        }
    }

    /// `File::sync_data`.
    #[inline]
    pub(crate) fn sync_data(&self) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => f.sync_data(),
            Kept::Ext(x) => x.sync(),
        }
    }

    /// `File::sync_all`.
    #[inline]
    pub(crate) fn sync_all(&self) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => f.sync_all(),
            Kept::Ext(x) => x.sync(),
        }
    }

    /// This file's bytes are all written and it asks for no sync of its
    /// own: for a file a store regenerates, written beside one whose sync
    /// follows. Nothing for a file of its own, which a store finds again by
    /// its name and checks; a file of a store's data file is found only
    /// through a manifest, which names what a sync has covered, so it asks
    /// to be covered by the next sync of any file of the store
    /// (`ExtFile::ride`).
    #[inline]
    pub(crate) fn ride(&self) {
        if let Kept::Ext(x) = self.kept() {
            x.ride();
        }
    }

    /// `File::set_len`.
    #[inline]
    pub(crate) fn set_len(&self, len: u64) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => f.set_len(len),
            Kept::Ext(x) => x.set_len(len),
        }
    }

    /// The file's length: `File::metadata`'s.
    #[inline]
    pub(crate) fn len(&self) -> Result<u64> {
        match self.kept() {
            Kept::Os(f) => Ok(f.metadata()?.len()),
            Kept::Ext(x) => Ok(x.len()),
        }
    }

    /// `File::try_clone`: a second handle on the same open file
    /// description, its position shared.
    #[inline]
    pub(crate) fn try_clone(&self) -> Result<File> {
        match self.kept() {
            Kept::Os(f) => f.try_clone().map(File::os),
            Kept::Ext(x) => x.try_clone().and_then(File::ext),
        }
    }

    /// `FileExt::read_exact_at`.
    #[inline]
    pub(crate) fn read_exact_at(&self, buf: &mut [u8], at: u64) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => std::os::unix::fs::FileExt::read_exact_at(f, buf, at),
            Kept::Ext(x) => x.read_exact_at(buf, at),
        }
    }

    /// `FileExt::write_all_at`.
    #[inline]
    pub(crate) fn write_all_at(&self, buf: &[u8], at: u64) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => std::os::unix::fs::FileExt::write_all_at(f, buf, at),
            Kept::Ext(x) => x.write_all_at(buf, at),
        }
    }

    /// One `pwritev` of `iov` at `at`: what the call returned, the bytes
    /// written or a negative with the error in `errno`. The loop over a
    /// short count is the caller's (`db::pwritev_loop`).
    #[inline(always)]
    pub(crate) fn pwritev(&self, iov: &[std::io::IoSlice<'_>], at: u64) -> isize {
        match self.kept() {
            Kept::Os(f) => os_pwritev(f, iov, at),
            Kept::Ext(x) => x.pwritev(iov, at),
        }
    }

    /// The descriptor a call on this file is made through and the range
    /// `[at, at + len)` of the file as that descriptor has it: the file's
    /// own and the range as given, a length of nothing reaching the file's
    /// end; or the data file's and the range within the file's extent, a
    /// length of nothing being the extent's and never the data file's.
    /// `None` where the range is nothing of the file's.
    #[cfg(target_os = "linux")]
    #[inline]
    fn fd_range(&self, at: u64, len: u64) -> Option<(libc::c_int, u64, u64)> {
        use std::os::fd::AsRawFd;
        match self.kept() {
            Kept::Os(f) => Some((f.as_raw_fd(), at, len)),
            Kept::Ext(x) => {
                let (at, len) = x.range(at, len);
                (len != 0).then(|| (x.description().as_raw_fd(), at, len))
            }
        }
    }

    /// The pages of `len` bytes at `at` handed to the device, waiting for
    /// nothing: `sync_file_range` with the write flag on Linux, where it
    /// is a hint the later fsync is shorter for; nothing on another
    /// platform, and nothing on an error, since the fsync that follows
    /// writes whatever this did not.
    #[inline]
    pub(crate) fn start_writeback(&self, at: u64, len: u64) {
        #[cfg(target_os = "linux")]
        let _ = self.sync_range(at, len, libc::SYNC_FILE_RANGE_WRITE);
        #[cfg(not(target_os = "linux"))]
        let _ = (at, len);
    }

    /// `sync_file_range` over `len` bytes at `at` with `flags`, a length
    /// of nothing reaching the file's end: what the call returned. Linux's
    /// alone.
    #[cfg(target_os = "linux")]
    #[inline]
    pub(crate) fn sync_range(&self, at: u64, len: u64, flags: libc::c_uint) -> libc::c_int {
        let Some((fd, at, len)) = self.fd_range(at, len) else {
            return 0;
        };
        // SAFETY: a writeback over a range of an open file; the call
        // reads and writes no memory of ours.
        unsafe { libc::sync_file_range(fd, at as libc::off64_t, len as libc::off64_t, flags) }
    }

    /// `posix_fadvise` over `len` bytes at `at`, a length of nothing
    /// reaching the file's end: what the call returned, an error's number
    /// and not `errno`. Linux's alone.
    #[cfg(target_os = "linux")]
    #[inline]
    pub(crate) fn advise(&self, at: u64, len: u64, advice: libc::c_int) -> libc::c_int {
        let Some((fd, at, len)) = self.fd_range(at, len) else {
            return 0;
        };
        let (Ok(at), Ok(len)) = (libc::off_t::try_from(at), libc::off_t::try_from(len)) else {
            return libc::EINVAL;
        };
        // SAFETY: an advice over a range of an open file; nothing is
        // written.
        unsafe { libc::posix_fadvise(fd, at, len, advice) }
    }

    /// `POSIX_FADV_DONTNEED` over `len` bytes at `at`, a length of nothing
    /// reaching the file's end: the clean pages of that range dropped from
    /// the page cache, a mapped page skipped, which is why
    /// `Bytes::advise_dontneed_range` goes first, and a dirty one written
    /// back first and dropped by a later reclaim. Nothing off Linux.
    #[inline]
    pub(crate) fn drop_range(&self, at: u64, len: u64) {
        #[cfg(target_os = "linux")]
        let _ = self.advise(at, len, libc::POSIX_FADV_DONTNEED);
        #[cfg(not(target_os = "linux"))]
        let _ = (at, len);
    }

    /// `drop_range` over the whole file.
    #[inline]
    pub(crate) fn drop_all(&self) {
        self.drop_range(0, 0)
    }

    /// `POSIX_FADV_WILLNEED` over `len` bytes at `at`: the reads started
    /// and none waited for, a page the cache holds already not read again.
    /// Linux's alone, as its one caller is (`zfile::ask_file`).
    #[cfg(target_os = "linux")]
    #[inline]
    pub(crate) fn ask_range(&self, at: u64, len: u64) {
        let _ = self.advise(at, len, libc::POSIX_FADV_WILLNEED);
    }

    /// `[at, at + len)` written back and waited for, a length of nothing
    /// reaching the file's end, and the writeback's error read:
    /// `sync_file_range` with both waits, which reports a writeback error
    /// the descriptor has not yet seen as an fsync does, once, and asks
    /// the filesystem for no journal commit and the device for no flush,
    /// as an fsync would. Off Linux, where nothing waits for a range, the
    /// file's sync.
    pub(crate) fn written_back(&self, at: u64, len: u64) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let rc = self.sync_range(
                at,
                len,
                libc::SYNC_FILE_RANGE_WAIT_BEFORE
                    | libc::SYNC_FILE_RANGE_WRITE
                    | libc::SYNC_FILE_RANGE_WAIT_AFTER,
            );
            if rc != 0 {
                return Err(Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (at, len);
            self.sync_data()
        }
    }

    /// Another open file description of the same file, for a thread that
    /// syncs it beside this one's holder and has to be told of a failure
    /// of its own (`db::HelperShared`): through `/proc/self/fd/<fd>` on
    /// Linux, a path built on the stack; a duplicate of the descriptor
    /// elsewhere. Of a file of a store's data file, another open of the
    /// data file.
    pub(crate) fn reopen(&self) -> Result<File> {
        let f = match self.kept() {
            Kept::Os(f) => f,
            Kept::Ext(x) => return x.reopen().and_then(File::ext),
        };
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let mut path = *b"/proc/self/fd/\0\0\0\0\0\0\0\0\0\0\0\0";
            let mut digits = [0u8; 10];
            let (mut fd, mut n) = (f.as_raw_fd() as u32, 0);
            loop {
                digits[n] = b'0' + (fd % 10) as u8;
                n += 1;
                fd /= 10;
                if fd == 0 {
                    break;
                }
            }
            for i in 0..n {
                path[14 + i] = digits[n - 1 - i];
            }
            // SAFETY: `path` is a NUL-terminated string on the stack; the
            // descriptor returned is ours alone.
            let fd = unsafe {
                libc::open(
                    path.as_ptr() as *const libc::c_char,
                    libc::O_RDONLY | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(Error::last_os_error());
            }
            // SAFETY: a descriptor just opened, owned by nothing else.
            Ok(File::os(unsafe { OsFile::from_raw_fd(fd) }))
        }
        #[cfg(not(target_os = "linux"))]
        {
            f.try_clone().map(File::os)
        }
    }

    /// The descriptor the file's calls are made through, its number.
    #[inline]
    fn raw(&self) -> libc::c_int {
        use std::os::fd::AsRawFd;
        match self.kept() {
            Kept::Os(f) => f.as_raw_fd(),
            Kept::Ext(x) => x.description().as_raw_fd(),
        }
    }

    /// Whether this and `other` are one open file description (`kcmp`'s
    /// `KCMP_FILE`).
    pub(crate) fn same_description(&self, other: &File) -> bool {
        #[cfg(target_os = "linux")]
        {
            const KCMP_FILE: libc::c_int = 0;
            let pid = std::process::id() as libc::pid_t;
            // SAFETY: compares two of this process's descriptors; no
            // memory.
            let r = unsafe {
                libc::syscall(
                    libc::SYS_kcmp,
                    pid,
                    pid,
                    KCMP_FILE,
                    self.raw() as libc::c_ulong,
                    other.raw() as libc::c_ulong,
                )
            };
            assert!(r >= 0, "kcmp: {}", Error::last_os_error());
            r == 0
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.raw() == other.raw()
        }
    }

    /// The file's inode number, for a test's seam that names which file a
    /// failure was made for; for a file of a store's data file, a number
    /// as much its own.
    pub(crate) fn ino(&self) -> Result<u64> {
        use std::os::unix::fs::MetadataExt;
        match self.kept() {
            Kept::Os(f) => Ok(f.metadata()?.ino()),
            Kept::Ext(x) => Ok(x.id()),
        }
    }

    /// `File::try_lock`: an exclusive `flock`, taken or refused at once. A
    /// store's `LOCK` alone (`open_lock`).
    #[inline]
    pub(crate) fn try_lock(&self) -> std::result::Result<(), std::fs::TryLockError> {
        match self.kept() {
            Kept::Os(f) => f.try_lock(),
            Kept::Ext(_) => Err(std::fs::TryLockError::Error(Error::other(
                "a lock on a file of the store's data file",
            ))),
        }
    }

    /// `File::unlock`.
    #[inline]
    pub(crate) fn unlock(&self) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => f.unlock(),
            Kept::Ext(_) => Ok(()),
        }
    }
}

/// `Read::read_to_end` over a file of a store's data file: the rest of it,
/// read in one piece, its length known.
fn ext_to_end(x: &ExtFile, buf: &mut Vec<u8>) -> Result<usize> {
    let at = x.seek(std::io::SeekFrom::Current(0))?;
    let n = x.len().saturating_sub(at) as usize;
    let had = buf.len();
    buf.resize(had + n, 0);
    x.read_exact_at(&mut buf[had..], at)?;
    x.seek(std::io::SeekFrom::Start(at + n as u64))?;
    Ok(n)
}

// The standard library's file answers `read_to_end` and its kin with the
// file's length in hand, where a default built on `read` grows a buffer by
// guesses: each is handed on as it is, so a read through the door makes
// the calls a read of the file made.
impl Read for &File {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.kept() {
            Kept::Os(f) => (&*f).read(buf),
            Kept::Ext(x) => x.read(buf),
        }
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [std::io::IoSliceMut<'_>]) -> Result<usize> {
        match self.kept() {
            Kept::Os(f) => (&*f).read_vectored(bufs),
            Kept::Ext(x) => match bufs.iter_mut().find(|b| !b.is_empty()) {
                Some(b) => x.read(b),
                None => Ok(0),
            },
        }
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        match self.kept() {
            Kept::Os(f) => (&*f).read_to_end(buf),
            Kept::Ext(x) => ext_to_end(x, buf),
        }
    }

    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        match self.kept() {
            Kept::Os(f) => (&*f).read_to_string(buf),
            Kept::Ext(x) => {
                let mut bytes = Vec::new();
                let n = ext_to_end(x, &mut bytes)?;
                buf.push_str(
                    std::str::from_utf8(&bytes)
                        .map_err(|_| Error::new(ErrorKind::InvalidData, "not UTF-8"))?,
                );
                Ok(n)
            }
        }
    }
}

impl Read for File {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (&*self).read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [std::io::IoSliceMut<'_>]) -> Result<usize> {
        (&*self).read_vectored(bufs)
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        (&*self).read_to_end(buf)
    }

    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        (&*self).read_to_string(buf)
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => {
                let mut f = f;
                f.read_exact(buf)
            }
            Kept::Ext(x) => {
                let mut rest = buf;
                while !rest.is_empty() {
                    match x.read(rest)? {
                        0 => {
                            return Err(Error::new(
                                ErrorKind::UnexpectedEof,
                                "failed to fill whole buffer",
                            ))
                        }
                        n => rest = &mut rest[n..],
                    }
                }
                Ok(())
            }
        }
    }
}

impl Write for &File {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        match self.kept() {
            Kept::Os(f) => (&*f).write(buf),
            Kept::Ext(x) => x.write(buf),
        }
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> Result<usize> {
        match self.kept() {
            Kept::Os(f) => (&*f).write_vectored(bufs),
            Kept::Ext(x) => match bufs.iter().find(|b| !b.is_empty()) {
                Some(b) => x.write(b),
                None => Ok(0),
            },
        }
    }

    #[inline]
    fn flush(&mut self) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => (&*f).flush(),
            Kept::Ext(_) => Ok(()),
        }
    }
}

impl Write for File {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (&*self).write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> Result<usize> {
        (&*self).write_vectored(bufs)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        match self.kept() {
            Kept::Os(f) => {
                let mut f = f;
                f.write_all(buf)
            }
            Kept::Ext(_) => {
                let mut rest = buf;
                while !rest.is_empty() {
                    let n = (&*self).write(rest)?;
                    rest = &rest[n..];
                }
                Ok(())
            }
        }
    }

    #[inline]
    fn flush(&mut self) -> Result<()> {
        (&*self).flush()
    }
}

impl std::io::Seek for &File {
    #[inline]
    fn seek(&mut self, pos: std::io::SeekFrom) -> Result<u64> {
        match self.kept() {
            Kept::Os(f) => (&*f).seek(pos),
            Kept::Ext(x) => x.seek(pos),
        }
    }
}

impl std::io::Seek for File {
    #[inline]
    fn seek(&mut self, pos: std::io::SeekFrom) -> Result<u64> {
        (&*self).seek(pos)
    }
}

impl std::fmt::Debug for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kept() {
            Kept::Os(o) => o.fmt(f),
            Kept::Ext(x) => write!(f, "File(extent {:#x})", x.id()),
        }
    }
}

/// `File::create`: the file made, or cut to nothing, and open for writing.
pub(crate) fn create(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let f = match behind(path) {
        Behind::Plain => OsFile::create(path),
        Behind::Vault(v, name) if passes(&name) => OsFile::create(v.real.join(name)),
        Behind::Vault(v, name) => v.make(&name),
        Behind::One(o, name) if onefile::takes(&name) => return o.make(&name).and_then(File::ext),
        Behind::One(..) => OsFile::create(path),
    };
    f.map(File::os)
}

/// The file at `path`, just made, is to hold `bytes`: said to the store's
/// cache where the store is under a cipher, which gives way ahead of a
/// file it would otherwise learn of a page at a time (`Coming`). Nothing
/// for a plain store, whose files are the page cache's.
pub(crate) fn expect(path: impl AsRef<Path>, bytes: u64) {
    match behind(path.as_ref()) {
        Behind::Plain => {}
        Behind::Vault(v, name) => {
            if !passes(&name) {
                v.expect(&name, bytes);
            }
        }
        Behind::One(o, name) => o.expect(&name, bytes),
    }
}

/// `create`, readable as well: what a writer that reads its own bytes back
/// opens.
pub(crate) fn create_rw(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| {
        OpenOptions::new()
            .read(true)
            .create(true)
            .write(true)
            .truncate(true)
            .open(p)
    };
    let f = match behind(path) {
        Behind::Plain => plain(path),
        Behind::Vault(v, name) if passes(&name) => plain(&v.real.join(name)),
        Behind::Vault(v, name) => v.make(&name),
        Behind::One(o, name) if onefile::takes(&name) => return o.make(&name).and_then(File::ext),
        Behind::One(..) => plain(path),
    };
    f.map(File::os)
}

/// `File::open`: a file that exists, for reading.
pub(crate) fn open(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let f = match behind(path) {
        Behind::Plain => OsFile::open(path),
        Behind::Vault(v, name) if passes(&name) => OsFile::open(v.real.join(name)),
        Behind::Vault(v, name) => v.reopen_written(&name),
        Behind::One(o, name) => match o.reopen(&name, false) {
            Some(r) => return r.and_then(File::ext),
            None => OsFile::open(path),
        },
    };
    f.map(File::os)
}

/// A file that exists, opened for writing where the caller says, cut
/// nowhere.
pub(crate) fn open_write(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| OpenOptions::new().write(true).open(p);
    let f = match behind(path) {
        Behind::Plain => plain(path),
        Behind::Vault(v, name) if passes(&name) => plain(&v.real.join(name)),
        Behind::Vault(v, name) => v.rewrite(&name, false),
        Behind::One(o, name) => match o.reopen(&name, true) {
            Some(r) => return r.and_then(File::ext),
            None => plain(path),
        },
    };
    f.map(File::os)
}

/// A file that exists, opened to be read and written where the caller
/// says, cut nowhere: a run's file an open resumes (`SegmentWriter::resume_run`).
pub(crate) fn open_rw(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| OpenOptions::new().read(true).write(true).open(p);
    let f = match behind(path) {
        Behind::Plain => plain(path),
        Behind::Vault(v, name) if passes(&name) => plain(&v.real.join(name)),
        Behind::Vault(v, name) => v.rewrite(&name, false),
        Behind::One(o, name) => match o.reopen(&name, true) {
            Some(r) => return r.and_then(File::ext),
            None => plain(path),
        },
    };
    f.map(File::os)
}

/// A file opened to be read and written at the offset its caller seeks
/// to, made if it is not there and cut nowhere: a log file reopened for
/// its next frames (`Wal::open_append`). Not opened to append, since an
/// append-mode write goes to the file's end whatever the cursor says.
pub(crate) fn open_append(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(p)
    };
    let f = match behind(path) {
        Behind::Plain => plain(path),
        Behind::Vault(v, name) if passes(&name) => plain(&v.real.join(name)),
        Behind::Vault(v, name) => match v.rewrite(&name, true) {
            Err(e) if e.kind() == ErrorKind::NotFound => v.make(&name),
            r => r,
        },
        Behind::One(o, name) => match o.reopen(&name, true) {
            Some(r) => return r.and_then(File::ext),
            None => plain(path),
        },
    };
    f.map(File::os)
}

/// A store's lock file (`db::StoreLock`), made if it is not there and cut
/// nowhere: in the store's own directory whatever the store is kept
/// under, a file of no bytes that only the kernel's lock on it means
/// anything, and nothing the vault's door stands in front of.
pub(crate) fn open_lock(path: impl AsRef<Path>) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map(File::os)
}

/// A file opened to be written in place, made if it is not there and cut
/// nowhere if it is: a manifest slot.
pub(crate) fn open_slot(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    if IO_SEAMS.load(Ordering::Relaxed) & SEAM_TRACE != 0 {
        io_trace("open_slot", &real(path));
    }
    let plain = |p: &Path| {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(p)
    };
    let f = match behind(path) {
        Behind::Plain => plain(path),
        Behind::Vault(v, name) if passes(&name) => plain(&v.real.join(name)),
        Behind::Vault(v, name) => match v.rewrite(&name, false) {
            Err(e) if e.kind() == ErrorKind::NotFound => v.make(&name),
            r => r,
        },
        Behind::One(o, name) => match o.reopen(&name, true) {
            Some(r) => return r.and_then(File::ext),
            None => plain(path),
        },
    };
    f.map(File::os)
}

/// A manifest slot's bytes made durable as the page cache holds them: an
/// fdatasync of `f`, the file at `path` as the open read it, recorded once
/// it has returned. `failed` is a test's: the error the fdatasync answers
/// in its place, so a failure is never recorded.
pub(crate) fn sync_slot(
    f: &File,
    path: impl AsRef<Path>,
    failed: Option<std::io::Error>,
) -> Result<()> {
    match failed {
        Some(e) => Err(e),
        // A slot is a file of the directory's own.
        None => f.plain().sync_data(),
    }?;
    note_file("sync_slot", path, f);
    Ok(())
}

/// `std::fs::read`: the file's bytes. The tests' only.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    use crate::bytes::Bytes;
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => std::fs::read(path),
        Behind::Vault(v, name) if passes(&name) => std::fs::read(v.real.join(name)),
        Behind::Vault(v, name) => {
            let page = v.page_of(&name)?;
            let _held = v.pager.hold();
            let src = MmapBytes::paged(page);
            let mut buf = vec![0u8; src.len() as usize];
            src.read_at(0, &mut buf)?;
            v.pager.check()?;
            Ok(buf)
        }
        Behind::One(o, name) => match o.reopen(&name, false) {
            Some(f) => {
                let f = f?;
                let mut buf = vec![0u8; f.len() as usize];
                f.read_exact_at(&mut buf, 0)?;
                Ok(buf)
            }
            None => std::fs::read(path),
        },
    }
}

/// `std::fs::copy`, for a copy out of a store in the clear (a checkpoint's
/// files where a link is refused); a store under a cipher has no file to
/// copy as it lies, so a path under one is refused.
pub(crate) fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<u64> {
    let (from, to) = (from.as_ref(), to.as_ref());
    match (behind(from), behind(to)) {
        (Behind::Plain, Behind::Plain) => std::fs::copy(from, to),
        // A file of a store's data file is copied out of it, into a file
        // of its own (`One::copy_out`); any other file of that store's
        // directory is the directory's.
        (Behind::One(o, name), Behind::Plain) => match o.copy_out(&name, to) {
            Some(r) => r,
            None => std::fs::copy(from, to),
        },
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            "a copy of a file of a store under a cipher",
        )),
    }
}

/// `std::fs::metadata` of a directory, answered as whether it is there:
/// the error where it is not.
pub(crate) fn dir_exists(dir: impl AsRef<Path>) -> Result<()> {
    let dir = dir.as_ref();
    match locate(dir) {
        None => std::fs::metadata(dir).map(|_| ()),
        Some(_) => Err(Error::new(
            ErrorKind::Unsupported,
            "a directory inside a store under a cipher",
        )),
    }
}

/// The names a guard stands beside (`guard`), process-wide as the door
/// is: how many, read beside every removal and rename, and the names.
static GUARDED: AtomicUsize = AtomicUsize::new(0);
static GUARDS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The guard of `path`: its name with `.guard` after it, in the same
/// directory, so a store under a cipher passes it to the directory where
/// it passes the file (`passes`).
pub(crate) fn guard_path(path: &Path) -> PathBuf {
    let mut g = path.as_os_str().to_os_string();
    g.push(GUARD_SUFFIX);
    PathBuf::from(g)
}

/// The suffix a guard's name carries.
pub(crate) const GUARD_SUFFIX: &str = ".guard";

/// A second directory entry made for the file at `path`, a hard link
/// under its guard's name, after a directory fsync that was to make
/// `path`'s own entry durable failed (`Db::dir_barrier`): Linux may drop
/// what a failed sync was to write and answer a later one `Ok`, so the
/// entry the failure was for is never trusted again in this boot, and a
/// new operation on the directory is made instead, which the caller's
/// next fsync of the directory covers as any operation after it. The
/// guard stands until the file goes: its removal or its rename through
/// the door unlinks the guard first (`unguard`), so a guard never names
/// the inode under another name, and an open that finds a guard whose
/// file is not there gives the file its name back from it.
pub(crate) fn guard(path: &Path) -> Result<()> {
    let g = guard_path(path);
    match hard_link(path, &g) {
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        r => r?,
    }
    adopt_guard(path);
    Ok(())
}

/// A guard an open found beside its file, kept as `guard` keeps one.
pub(crate) fn adopt_guard(path: &Path) {
    let mut gs = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
    if !gs.iter().any(|p| p == path) {
        gs.push(path.to_path_buf());
        GUARDED.store(gs.len(), Ordering::Relaxed);
    }
}

/// The guards of the files in `dir` forgotten, unlinked nowhere: a store's
/// close, whose files the next open finds the guards of again.
pub(crate) fn forget_guards(dir: &Path) {
    let mut gs = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
    gs.retain(|p| p.parent() != Some(dir));
    GUARDED.store(gs.len(), Ordering::Relaxed);
}

/// Whether `path` stands guarded, for a test.
pub(crate) fn guarded(path: &Path) -> bool {
    GUARDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|p| p == path)
}

/// The guard of `path` unlinked before `path` is removed or renamed, and
/// forgotten: first, so that a crash between the two leaves the file
/// with no guard -- a name an open reads as it always did -- and never a
/// guard whose file has gone, which an open would give the name back.
#[cold]
#[inline(never)]
fn unguard(path: &Path) -> Result<()> {
    let mut gs = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(i) = gs.iter().position(|p| p == path) else {
        return Ok(());
    };
    let g = guard_path(path);
    let r = match locate(&g) {
        None => std::fs::remove_file(&g),
        Some((v, name)) if passes(&name) => std::fs::remove_file(v.real.join(name)),
        Some((v, name)) => v.remove(&name),
    };
    match r {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    gs.swap_remove(i);
    GUARDED.store(gs.len(), Ordering::Relaxed);
    Ok(())
}

/// `std::fs::rename`.
pub(crate) fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    if GUARDED.load(Ordering::Relaxed) != 0 {
        unguard(from)?;
    }
    match (behind(from), behind(to)) {
        (Behind::Plain, Behind::Plain) => std::fs::rename(from, to),
        (Behind::Vault(v, a), Behind::Vault(w, b))
            if Arc::ptr_eq(&v, &w) && passes(&a) == passes(&b) =>
        {
            if passes(&a) {
                std::fs::rename(v.real.join(a), v.real.join(b))
            } else {
                v.rename(&a, &b)
            }
        }
        // In a store kept in one file: a file of the table is renamed in
        // the table, and a file of the directory's own in the directory,
        // where a name of the table it takes is the table's no longer.
        (Behind::One(o, a), Behind::One(w, b)) if Arc::ptr_eq(&o, &w) => {
            if o.rename(&a, &b).is_some() {
                return Ok(());
            }
            std::fs::rename(from, to)?;
            let _ = o.remove(&b);
            Ok(())
        }
        (Behind::One(o, a), Behind::Plain) if !o.holds(&a) => std::fs::rename(from, to),
        (Behind::Plain, Behind::One(..)) => std::fs::rename(from, to),
        (Behind::One(..), _) => Err(Error::new(
            ErrorKind::Unsupported,
            "a rename of a file of the store's data file out of the store",
        )),
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            "a rename between a store under a cipher and somewhere else",
        )),
    }
}

/// A file being written, opened to be mapped by its writer, and what the
/// mapping holds on it: the file as `open` gives it, and for a store under
/// a cipher the hold that keeps the store's cache from giving the file's
/// memory back under the mapping (`Mapped`).
pub(crate) fn open_mapped(path: impl AsRef<Path>) -> Result<(File, Mapped)> {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => Ok((File::os(OsFile::open(path)?), Mapped(None))),
        Behind::Vault(v, name) if passes(&name) => {
            Ok((File::os(OsFile::open(v.real.join(name))?), Mapped(None)))
        }
        Behind::Vault(v, name) => {
            let (file, mem) = v.map_written(&name)?;
            Ok((File::os(file), Mapped(Some((v, mem)))))
        }
        Behind::One(o, name) => match o.reopen(&name, false) {
            Some(f) => Ok((File::ext(f?)?, Mapped(None))),
            None => Ok((File::os(OsFile::open(path)?), Mapped(None))),
        },
    }
}

/// `std::fs::hard_link`.
pub(crate) fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    match (behind(from), behind(to)) {
        (Behind::Plain, Behind::Plain) => std::fs::hard_link(from, to),
        (Behind::Vault(v, a), Behind::Vault(w, b))
            if Arc::ptr_eq(&v, &w) && passes(&a) == passes(&b) =>
        {
            if passes(&a) {
                std::fs::hard_link(v.real.join(a), v.real.join(b))
            } else {
                v.link(&a, &b)
            }
        }
        // In a store kept in one file: a file of the table gains a name
        // in the table, a file of the directory's own a link in it.
        (Behind::One(o, a), Behind::One(w, b)) if Arc::ptr_eq(&o, &w) => match o.link(&a, &b) {
            Some(r) => r,
            None => std::fs::hard_link(from, to),
        },
        (Behind::One(o, a), Behind::Plain) if !o.holds(&a) => std::fs::hard_link(from, to),
        (Behind::Plain, Behind::One(..)) => std::fs::hard_link(from, to),
        // A link out of the data file is refused, as a link across
        // filesystems is: a caller that wants the file elsewhere copies
        // it (`copy`).
        (Behind::One(..), _) => Err(Error::new(
            ErrorKind::Unsupported,
            "a link to a file of the store's data file from outside the store",
        )),
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            "a link between a store under a cipher and somewhere else",
        )),
    }
}

/// `std::fs::remove_file`.
pub(crate) fn remove_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if GUARDED.load(Ordering::Relaxed) != 0 {
        unguard(path)?;
    }
    #[cfg(all(test, target_os = "linux"))]
    if tests::fails_after_unguard(path) {
        return Err(Error::other(
            "a test's failure between the guard's unlink and the file's",
        ));
    }
    match behind(path) {
        Behind::Plain => std::fs::remove_file(path),
        Behind::Vault(v, name) if passes(&name) => std::fs::remove_file(v.real.join(name)),
        Behind::Vault(v, name) => v.remove(&name),
        Behind::One(o, name) => match o.remove(&name) {
            Some(()) => Ok(()),
            None => std::fs::remove_file(path),
        },
    }
}

/// `Path::exists`.
pub(crate) fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => path.exists(),
        Behind::Vault(v, name) if passes(&name) => v.real.join(name).exists(),
        Behind::Vault(v, name) => lock(&v.files).contains_key(&name),
        Behind::One(o, name) => o.holds(&name) || path.exists(),
    }
}

/// Whether `path`'s bytes are in the directory: the file, or under a
/// cipher the envelope a slot read named for it.
pub(crate) fn on_device(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    note("on_device", path);
    match behind(path) {
        Behind::Plain => path.exists(),
        Behind::Vault(v, name) if passes(&name) => v.real.join(name).exists(),
        Behind::Vault(v, name) => v.on_device(&name),
        Behind::One(o, name) => o.on_device(&name).unwrap_or_else(|| path.exists()),
    }
}

/// Whether the store at `dir`, under a cipher, holds any segment's
/// envelope; never for a store in the clear.
pub(crate) fn holds_envelopes(dir: impl AsRef<Path>) -> Result<bool> {
    match under(dir.as_ref()) {
        Some(v) => v.holds_segments(),
        None => Ok(false),
    }
}

/// `Path::is_file`.
pub(crate) fn is_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => path.is_file(),
        Behind::Vault(v, name) if passes(&name) => v.real.join(name).is_file(),
        Behind::Vault(v, name) => lock(&v.files).contains_key(&name),
        Behind::One(o, name) => o.holds(&name) || path.is_file(),
    }
}

/// The file's length, as `std::fs::metadata` has it.
pub(crate) fn len(path: impl AsRef<Path>) -> Result<u64> {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => Ok(std::fs::metadata(path)?.len()),
        Behind::Vault(v, name) if passes(&name) => Ok(std::fs::metadata(v.real.join(name))?.len()),
        Behind::Vault(v, name) => v.len(&name),
        Behind::One(o, name) => match o.len(&name) {
            Some(n) => Ok(n),
            None => Ok(std::fs::metadata(path)?.len()),
        },
    }
}

/// The names of the files in a store's directory, as `std::fs::read_dir`
/// lists them.
pub(crate) fn names(dir: impl AsRef<Path>) -> Result<Vec<String>> {
    let dir = dir.as_ref();
    match under(dir) {
        Some(v) => v.names(),
        None => {
            let mut out = Vec::new();
            for entry in std::fs::read_dir(dir)? {
                out.push(entry?.file_name().to_string_lossy().into_owned());
            }
            // A store kept in one file: the names its table holds beside
            // the directory's own.
            if let Some(o) = one_under(dir) {
                out.extend(o.names());
            }
            Ok(out)
        }
    }
}

/// The directory's entries made durable: an fsync of the directory.
pub(crate) fn sync_dir(dir: impl AsRef<Path>) -> Result<()> {
    sync_dir_as(dir, DirSync::Other)
}

/// Which directory fsync a call to `sync_dir_as` is, for a test's hook on
/// it (`hook_dir_sync`) and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DirSync {
    /// Any but the next.
    Other,
    /// A rename-shape manifest publish's, after its rename
    /// (`manifest_write`).
    Publish,
}

/// `sync_dir`, saying which fsync it is. The test seams ride the one word
/// the call loads anyway: none set, it is the fsync and nothing else.
#[inline]
pub(crate) fn sync_dir_as(dir: impl AsRef<Path>, kind: DirSync) -> Result<()> {
    let dir = dir.as_ref();
    let seams = IO_SEAMS.load(Ordering::Relaxed);
    if seams != 0 {
        return sync_dir_seamed(dir, kind, seams);
    }
    match under(dir) {
        Some(v) => OsFile::open(&v.real)?.sync_all()?,
        None => OsFile::open(dir)?.sync_all()?,
    }
    Ok(())
}

/// `sync_dir_as` with a test's seam set: the hook on `dir`, where one is,
/// before the fsync, which it may fail in its place; the fsync recorded
/// once it has returned, where `trace_io` is on.
#[cold]
#[inline(never)]
fn sync_dir_seamed(dir: &Path, kind: DirSync, seams: u8) -> Result<()> {
    if seams & SEAM_HOOK != 0 {
        let hook = {
            let mut hooks = DIR_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
            hooks.retain(|(_, h)| h(None).is_some());
            if hooks.is_empty() {
                IO_SEAMS.fetch_and(!SEAM_HOOK, Ordering::Relaxed);
            }
            hooks.iter().find(|(d, _)| d == dir).map(|(_, h)| h.clone())
        };
        if let Some(h) = hook {
            if let Some(r) = h(Some(kind)) {
                r?;
            }
        }
    }
    match under(dir) {
        Some(v) => OsFile::open(&v.real)?.sync_all()?,
        None => OsFile::open(dir)?.sync_all()?,
    }
    if seams & SEAM_TRACE != 0 {
        io_trace("sync_dir", dir);
    }
    Ok(())
}

/// A test's hook on a directory's fsyncs: asked with `Some(kind)` before
/// each fsync of the directory it is set on, whose error the call answers
/// in the fsync's place, and with `None` to say whether it is still
/// wanted; `None` back from either is a hook whose store has gone.
pub(crate) type DirHook = Arc<dyn Fn(Option<DirSync>) -> Option<Result<()>> + Send + Sync>;

/// The hooks set, by the directory each is on, as the engine names it.
static DIR_HOOKS: Mutex<Vec<(PathBuf, DirHook)>> = Mutex::new(Vec::new());

/// A test's: `hook` set on the fsyncs of `dir`, in place of any set there
/// before; `None` takes it off.
pub(crate) fn hook_dir_sync(dir: &Path, hook: Option<DirHook>) {
    let mut hooks = DIR_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
    hooks.retain(|(d, h)| d != dir && h(None).is_some());
    if let Some(h) = hook {
        hooks.push((dir.to_path_buf(), h));
    }
    if hooks.is_empty() {
        IO_SEAMS.fetch_and(!SEAM_HOOK, Ordering::Relaxed);
    } else {
        IO_SEAMS.fetch_or(SEAM_HOOK, Ordering::Relaxed);
    }
}

/// A file's bytes made durable: an fsync of the file, opened for it. The
/// open's, for a pending segment it adopts and that segment's ordered
/// index, before the slot that names them durable.
pub(crate) fn sync_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    open(path)?.sync_all()?;
    if IO_SEAMS.load(Ordering::Relaxed) & SEAM_TRACE != 0 {
        io_trace("sync_file", path);
    }
    Ok(())
}

/// A test's seams, process-wide: whether directory fsyncs, the open's file
/// fsyncs (`sync_file`, `sync_slot`) and slot opens are recorded
/// (`SEAM_TRACE`, `trace_io`), and whether a hook is set on some
/// directory's fsyncs (`SEAM_HOOK`, `hook_dir_sync`); read beside the
/// syscall each makes, one load whatever is set.
static IO_SEAMS: AtomicU8 = AtomicU8::new(0);
const SEAM_TRACE: u8 = 1;
const SEAM_HOOK: u8 = 2;

/// A test's: the directories `trace_io` records for, each with what was
/// recorded for it in order -- the calls, and beside them the descriptors
/// `note_file` saw -- so a record holds only the calls on a directory a
/// test is reading, and goes when that test says it has read.
type IoRecord = (
    PathBuf,
    Vec<(&'static str, PathBuf)>,
    Vec<(&'static str, PathBuf, i32, u64)>,
);
static IO_TRACE: Mutex<Vec<IoRecord>> = Mutex::new(Vec::new());

/// A test's: `what` of the file at `path` recorded in the trace where it
/// is on (`trace_io`), by the file's path in the filesystem; one load
/// otherwise. For the open's own reads -- a manifest slot opened to be
/// read (`read_slot`), a named segment looked for (`on_device`), a segment
/// mapped to hold an unnamed one's key (`open_holder`), a log file
/// replayed (`replay`) -- which a test counts.
pub(crate) fn note(what: &'static str, path: impl AsRef<Path>) {
    if IO_SEAMS.load(Ordering::Relaxed) & SEAM_TRACE != 0 {
        io_trace(what, &real(path));
    }
}

/// A test's: `what` of the file at `path` recorded as `note` records it,
/// and beside it in a record of its own which descriptor it was made
/// through -- the descriptor's number and its open file description's
/// offset, which a second open of the same file does not share -- so a
/// test can hold two calls to one descriptor: a slot file read
/// (`read_slot`, once its bytes are read) and the fdatasync that makes it
/// durable (`sync_slot`). One load where tracing is off.
pub(crate) fn note_file(what: &'static str, path: impl AsRef<Path>, f: &File) {
    if IO_SEAMS.load(Ordering::Relaxed) & SEAM_TRACE != 0 {
        use std::io::Seek;
        use std::os::fd::AsRawFd;
        let path = real(path);
        let (fd, at) = match f.kept() {
            Kept::Os(o) => (
                o.as_raw_fd(),
                (&mut &*o).stream_position().unwrap_or(u64::MAX),
            ),
            Kept::Ext(x) => (x.description().as_raw_fd(), u64::MAX),
        };
        io_trace(what, &path);
        for r in IO_TRACE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .filter(|r| path.starts_with(&r.0))
        {
            r.2.push((what, path.clone(), fd, at));
        }
    }
}

/// A test's: what `note_file` recorded beside the trace for `dir`, in
/// order: the call, the file's path in the filesystem, the descriptor's
/// number and its offset.
pub(crate) fn traced_descriptors(dir: &Path) -> Vec<(&'static str, PathBuf, i32, u64)> {
    IO_TRACE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|r| r.0 == dir)
        .map(|r| r.2.clone())
        .unwrap_or_default()
}

fn io_trace(what: &'static str, path: &Path) {
    for r in IO_TRACE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter_mut()
        .filter(|r| path.starts_with(&r.0))
    {
        r.1.push((what, path.to_path_buf()));
    }
}

/// A test's: with `on`, record from now for `dir` -- the directory itself
/// and every file under it -- every directory fsync that returned, every
/// file fsync through `sync_file` or `sync_slot` that returned, every
/// open of a manifest slot for writing and the open's own reads (`note`),
/// each by the file's path in the filesystem (`real`), in the order they
/// were made; the record for `dir` so far. Off, the record for `dir`
/// returned and dropped, and nothing more recorded for it; nothing is
/// recorded once no directory is asked for. Each record holds the calls
/// on one test's directory between that test's first ask and its last.
pub(crate) fn trace_io(dir: &Path, on: bool) -> Vec<(&'static str, PathBuf)> {
    let mut all = IO_TRACE.lock().unwrap_or_else(|e| e.into_inner());
    let at = all.iter().position(|r| r.0 == dir);
    let out = match (at, on) {
        (Some(i), true) => all[i].1.clone(),
        (Some(i), false) => all.swap_remove(i).1,
        (None, true) => {
            all.push((dir.to_path_buf(), Vec::new(), Vec::new()));
            Vec::new()
        }
        (None, false) => Vec::new(),
    };
    if all.is_empty() {
        IO_SEAMS.fetch_and(!SEAM_TRACE, Ordering::Relaxed);
    } else {
        IO_SEAMS.fetch_or(SEAM_TRACE, Ordering::Relaxed);
    }
    out
}

/// Where in the filesystem a file the engine names lies: the path itself,
/// or for a store under a cipher the file's path in the store's own
/// directory, which only the log and the manifest have -- a kept file's
/// plaintext is nowhere in the filesystem, and the path answered for one
/// is where nothing is.
pub(crate) fn real(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    match behind(path) {
        Behind::Vault(v, name) if passes(&name) => v.real.join(name),
        _ => path.to_path_buf(),
    }
}

/// A directory's own handle, for a caller that syncs it later.
pub(crate) fn open_dir(dir: impl AsRef<Path>) -> Result<File> {
    let dir = dir.as_ref();
    let f = match under(dir) {
        Some(v) => OsFile::open(&v.real),
        None => OsFile::open(dir),
    };
    f.map(File::os)
}

/// The segments in `pairs`, each with its ordered index's name, made
/// durable where their fsync is not what makes them so: for a store under
/// a cipher, their envelopes written and synced now (`Vault::persist`),
/// on the thread that would have paid the fsync. Nothing for a plain
/// store, whose caller's fsync is the whole of it: `pairs` is asked for
/// only under a cipher, so a plain store makes no list of them.
pub(crate) fn persist(
    dir: impl AsRef<Path>,
    pairs: impl FnOnce() -> Vec<(String, Option<String>)>,
) -> Result<()> {
    match under(dir.as_ref()) {
        Some(v) => v.persist(&pairs(), false).map(|_| ()),
        None => Ok(()),
    }
}

/// `persist` for one segment a merge has just finished, by its name, with
/// its ordered index beside it; the directory's sync is left to the
/// merge's own `persist` of all its outputs at its end, which finds the
/// envelopes made and syncs the directory once.
pub(crate) fn persist_one(dir: impl AsRef<Path>, seg: &str) -> Result<()> {
    match under(dir.as_ref()) {
        Some(v) => {
            let ord = crate::db::Db::ord_name_for(seg);
            v.persist_with(&[(seg.to_string(), ord)], false, false)
                .map(|_| ())
        }
        None => Ok(()),
    }
}

/// `seg`'s compact form, written as `form`, given its envelope beside the
/// segment's (`Vault::persist_form`), synced where `sync` says: for a
/// store under a cipher, where a form file is otherwise a memory file the
/// store's cache can give nothing back of and the close loses. Nothing
/// for a plain store, whose form file is in its directory as it is
/// written.
pub(crate) fn persist_form(
    dir: impl AsRef<Path>,
    seg: &str,
    form: &str,
    sync: bool,
    cold: bool,
) -> Result<()> {
    match under(dir.as_ref()) {
        Some(v) => v.persist_form(seg, form, sync, cold).map(|_| ()),
        None => Ok(()),
    }
}

/// The form file beside `seg` made known to the store as `form`, where a
/// store under a cipher holds one in its directory from an earlier open
/// (`Vault::companion`). Nothing for a plain store, whose form file is
/// found by its name.
pub(crate) fn companion(dir: impl AsRef<Path>, seg: &str, form: &str) {
    if let Some(v) = under(dir.as_ref()) {
        v.companion(seg, form);
    }
}

/// Whether `path` names a file a store under a cipher keeps: one that is
/// read through the store's cache by the piece and has no descriptor to
/// read it by, where a plain store's file, the log and the manifest have.
pub(crate) fn kept(path: impl AsRef<Path>) -> bool {
    matches!(locate(path.as_ref()), Some((_, name)) if !passes(&name))
}

/// Whether a store that is told what a file is to hold (`expect`) is in
/// `dir`: a store under a cipher, or one kept in one file. A caller that
/// would have to add its inputs up to say asks first, so a plain store
/// adds nothing up.
pub(crate) fn announces(dir: impl AsRef<Path>) -> bool {
    if VAULTS.load(Ordering::Acquire) == 0 {
        return false;
    }
    let dir = dir.as_ref();
    one_under(dir).is_some() || under(dir).is_some()
}

/// The file at `path`, whose bytes are all written, asks for no sync of
/// its own (`File::ride`): nothing for a file of a directory's own, and
/// for a file of a store's data file the mark that the next sync of the
/// store covers it. By its path, for a caller that has closed the file.
pub(crate) fn ride(path: impl AsRef<Path>) {
    if let Behind::One(o, name) = behind(path.as_ref()) {
        o.ride(&name);
    }
}

/// `Path::is_file` for a file of a store in the clear: its own file in its
/// directory, or a file of its data file. Never for a store under a
/// cipher, whose engine names a directory that is not there: the callers
/// are the controller's looks for a form file an earlier build left,
/// which under a cipher find none and read what they would have read off
/// the file from a sample (`zfile::file_facts`).
pub(crate) fn clear_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => path.is_file(),
        Behind::One(o, name) => o.holds(&name) || path.is_file(),
        Behind::Vault(..) => false,
    }
}

/// `MmapBytes::open` for a file of a store in the clear, as `clear_file`
/// answers for one: its own file mapped, or its extent of the store's
/// data file. Refused for a store under a cipher, as the directory its
/// engine names refuses it.
pub(crate) fn map_clear(path: impl AsRef<Path>) -> Result<MmapBytes> {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => MmapBytes::open(path),
        Behind::One(o, name) => match o.map(&name) {
            Some(m) => m,
            None => MmapBytes::open(path),
        },
        Behind::Vault(..) => Err(Error::new(
            ErrorKind::NotFound,
            "a file of a store under a cipher mapped as it lies",
        )),
    }
}

/// The file mapped for reading, as `MmapBytes::open` maps it. A kept
/// file's mapping is a paged one (`pager::Paged`): address space the
/// file's length, of which a range is read by asking for it, under a pin
/// of the store's reader table or a hold (`hold`).
pub(crate) fn map(path: impl AsRef<Path>) -> Result<MmapBytes> {
    let path = path.as_ref();
    match behind(path) {
        Behind::Plain => MmapBytes::open(path),
        Behind::Vault(v, name) if passes(&name) => MmapBytes::open(&v.real.join(name)),
        Behind::Vault(v, name) => Ok(MmapBytes::paged(v.page_of(&name)?)),
        Behind::One(o, name) => match o.map(&name) {
            Some(m) => m,
            None => MmapBytes::open(path),
        },
    }
}

/// A file of the store in `dir` read once and in order under `hold`, beside
/// the store's cache (`pager::Stream`): a merge's input. A file with no
/// envelope yet is read through the cache, which holds it whole, and a
/// plain store's file is its mapping, as `map` answers.
pub(crate) fn stream(path: impl AsRef<Path>, hold: &pager::Hold) -> Result<MmapBytes> {
    let path = path.as_ref();
    if let Some((v, name)) = locate(path) {
        if !passes(&name) {
            if let Some(s) = v.stream_of(&name, hold)? {
                return Ok(MmapBytes::paged(s));
            }
        }
    }
    map(path)
}

/// Whether every file of the store in `dir` has read so far: the failure a
/// read of a store under a cipher met, if one has (`Pager::check`), and
/// nothing for a plain store, whose reads fail where they are made.
pub(crate) fn check(dir: impl AsRef<Path>) -> Result<()> {
    match under(dir.as_ref()) {
        Some(v) => v.pager.check(),
        None => Ok(()),
    }
}

/// A hold on the reads of the store in `dir` by this thread, for a thread
/// that reads the store's files with no slot of its reader table pinned:
/// nothing for a plain store, whose mappings are the kernel's to page.
pub(crate) fn hold(dir: impl AsRef<Path>) -> Option<pager::Hold> {
    under(dir.as_ref()).map(|v| v.pager.hold())
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;
    use crate::bytes::Bytes;

    /// A test's: the next `remove_file` of this path fails between its
    /// guard's unlink and its own, the state a crash between the two
    /// leaves. By path, so a test beside it removing its own files is not
    /// the one that fails.
    static FAIL_AFTER_UNGUARD: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

    /// A test's: a store's directory whose next segment envelope waits,
    /// just before it is written, until the test lets it go; whether one
    /// has got there; and whether it has been let go. One store at a time.
    static PERSIST_GATE: std::sync::Mutex<Option<(PathBuf, bool, bool)>> =
        std::sync::Mutex::new(None);
    static PERSIST_GATE_MOVED: std::sync::Condvar = std::sync::Condvar::new();

    /// The gate's wait, in the writer of a segment's envelope
    /// (`Vault::persist_with`): nothing for a store no test has gated.
    pub(crate) fn persist_gate(real: &Path) {
        let mut g = PERSIST_GATE.lock().unwrap_or_else(|e| e.into_inner());
        let shut =
            |g: &Option<(PathBuf, bool, bool)>| g.as_ref().is_some_and(|g| g.0 == real && !g.2);
        if !shut(&g) {
            return;
        }
        if let Some(g) = g.as_mut() {
            g.1 = true;
        }
        PERSIST_GATE_MOVED.notify_all();
        while shut(&g) {
            g = PERSIST_GATE_MOVED
                .wait(g)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The store at `real` gated: its next segment envelope waits, until
    /// the guard goes. A guard and not a call to open it, so that a test
    /// that fails with a writer at the gate lets the writer go as it
    /// unwinds: the first version opened the gate by hand after its
    /// assertions, and a failed assertion left a merge standing there and
    /// the store's drop waiting for it, a test that hung where it should
    /// have failed.
    pub(crate) fn gate_persists(real: &Path) -> PersistGate {
        *PERSIST_GATE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((real.to_path_buf(), false, false));
        PersistGate(real.to_path_buf())
    }

    pub(crate) struct PersistGate(PathBuf);

    impl Drop for PersistGate {
        fn drop(&mut self) {
            open_persist_gate(&self.0);
        }
    }

    /// Whether an envelope of the gated store is waiting at the gate.
    pub(crate) fn persist_gate_reached(real: &Path) -> bool {
        PERSIST_GATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|g| g.0 == real && g.1)
    }

    /// The gate opened for good: the waiting envelope and every one after.
    fn open_persist_gate(real: &Path) {
        let mut g = PERSIST_GATE.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|g| g.0 == real) {
            *g = None;
        }
        PERSIST_GATE_MOVED.notify_all();
    }

    pub(crate) fn fails_after_unguard(path: &Path) -> bool {
        let mut f = FAIL_AFTER_UNGUARD.lock().unwrap_or_else(|e| e.into_inner());
        if f.as_deref() == Some(path) {
            *f = None;
            return true;
        }
        false
    }

    /// A guarded file's removal unlinks the guard first: stopped between
    /// the two, the file stands with no guard, a name an open reads as it
    /// always did, and never a guard whose file has gone, which an open
    /// gives the name back to.
    #[test]
    fn a_guard_goes_before_its_file() {
        let d = std::env::temp_dir().join(format!("supdb-vault-unguard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("direct-00000007.tmp");
        std::fs::write(&f, b"run").unwrap();
        guard(&f).unwrap();
        let g = guard_path(&f);
        assert!(
            g.exists() && guarded(&f),
            "premise: the file stands guarded"
        );
        *FAIL_AFTER_UNGUARD.lock().unwrap() = Some(f.clone());
        assert!(
            remove_file(&f).is_err(),
            "the removal stopped between its unlinks"
        );
        assert!(f.exists(), "the file stands");
        assert!(
            !g.exists() && !guarded(&f),
            "and its guard has gone before it"
        );
        remove_file(&f).unwrap();
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A cipher for the tests: a keystream and a tag that are functions of
    /// the key, the nonce, the associated data and the bytes, and nothing
    /// anybody should use.
    struct Toy(u64);

    fn mix(mut h: u64, bytes: &[u8]) -> u64 {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h ^ (h >> 29)
    }

    impl Toy {
        fn of(key: &[u8; KEY]) -> Toy {
            Toy(mix(0x9E37_79B9_7F4A_7C15, key))
        }
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
            self.stream(nonce, buf);
            ok
        }
    }

    /// A store's cipher over the stand-in, for this module's tests and for
    /// the engine's own (`db::cipher_looks`).
    pub(crate) fn cipher(key: u8) -> StoreCipher {
        StoreCipher::new(Toy::of(&[key; KEY]), Toy::of).key_id(u64::from(key))
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("supdb-next-vault-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(path: &Path, bytes: &[u8]) {
        create(path).unwrap().write_all(bytes).unwrap();
    }

    /// The door's looks for a file by its path find one a store's data
    /// file keeps (`onefile::One`): what the controller asks of a form
    /// file an earlier build left before it maps it, and what the forced
    /// builds ask before they build one (`is_file`, `clear_file`,
    /// `map_clear`).
    #[test]
    fn a_file_of_a_onefile_store_is_found_by_the_looks_by_path() {
        use crate::bytes::Bytes as _;
        let d = dir("one-looks");
        let one = One::create(&d).unwrap();
        let held = hold_one(one.clone());
        let data: Vec<u8> = (0..70_000u32).map(|i| (i * 31 + 7) as u8).collect();
        let f = one.make("kept").unwrap();
        f.write_all_at(&data, 0).unwrap();
        f.sync().unwrap();
        drop(f);
        let path = d.join("kept");
        assert!(!path.exists(), "no file of the directory's own");
        assert!(is_file(&path), "a file of the data file is there");
        assert!(clear_file(&path), "and is one of a store in the clear");
        let other = d.join("never");
        assert!(!is_file(&other) && !clear_file(&other));
        let m = map_clear(&path).unwrap();
        assert_eq!(m.slice_at(0, data.len()).unwrap(), &data[..]);
        drop(m);
        drop(held);
    }

    fn pair(seg: &str, ord: Option<&str>) -> Vec<(String, Option<String>)> {
        vec![(seg.to_string(), ord.map(str::to_string))]
    }

    /// Every file in the directory, by name, with its bytes.
    fn on_disk(real: &Path) -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(real)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        v.sort();
        v
    }

    fn holds(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// A form file written again for the same segment leaves the view over
    /// the one before reading what it read: the second envelope is a file
    /// of its own, renamed over the first's name, and the first's bytes
    /// stay under the mapping a reader holds. Written in place, the first
    /// view would read the second's bytes under the first's header, and
    /// none of them passes its tag. And the name the first is kept under
    /// meanwhile takes the envelope with it only when it is the last.
    #[test]
    fn a_form_file_written_again_leaves_the_view_over_the_first_reading_the_first() {
        let real = dir("formagain");
        // A cache with no room: a file is read from its envelope, by the
        // chunk, when something wants it.
        let held = Vault::create(&real, &cipher(4).cache_bytes(4096)).unwrap();
        let d = held.dir();
        let v = held.vault();
        put(&d.join("par-1.sup"), &b"a segment ".repeat(100));
        v.persist(&pair("par-1.sup", None), false).unwrap();
        let first = b"the first coding of the form, ".repeat(2000);
        let second = b"another coding of it, and a longer file, ".repeat(3000);
        put(&d.join("zpg-1.zpg"), &first);
        assert!(v
            .persist_form("par-1.sup", "zpg-1.zpg", false, false)
            .unwrap());
        let forms = |real: &Path| {
            on_disk(real)
                .into_iter()
                .filter(|(n, _)| n.ends_with(".z"))
                .count()
        };
        assert_eq!(forms(&real), 1);
        // A reader's view of the first, nothing of it read yet.
        let old = map(d.join("zpg-1.zpg")).unwrap();
        rename(d.join("zpg-1.zpg"), d.join("dead-1")).unwrap();
        put(&d.join("zpg-1.zpg"), &second);
        assert!(v
            .persist_form("par-1.sup", "zpg-1.zpg", false, false)
            .unwrap());
        assert_eq!(forms(&real), 1, "one envelope a segment's form");
        {
            let _held = v.pager().hold();
            let mut got = vec![0u8; first.len()];
            old.read_at(0, &mut got).unwrap();
            assert!(got == first, "the first view read other bytes");
        }
        v.pager().check().unwrap();
        assert!(read(d.join("zpg-1.zpg")).unwrap() == second);
        // The first's name goes, and the envelope under the name they
        // share is the second's and stays.
        drop(old);
        remove_file(d.join("dead-1")).unwrap();
        assert_eq!(
            forms(&real),
            1,
            "the second's envelope went with the first's name"
        );
        assert!(read(d.join("zpg-1.zpg")).unwrap() == second);
        // And with the last name, it goes.
        remove_file(d.join("zpg-1.zpg")).unwrap();
        assert_eq!(forms(&real), 0);
    }

    /// A room gives way at once to what its store announces: the bytes
    /// said, every one ever, are what the cache's budget is lowered by
    /// between two readings (`Room::claimed`), a test's room carrying its
    /// own store's alone and the platform's the buffers the process has
    /// mapped as well; and a reading is less what is still to come.
    #[test]
    fn a_room_carries_what_its_store_announces() {
        use super::super::pager::Room;
        let coming = Arc::new(Coming::default());
        let mem = Arc::new(memfile().unwrap());
        let fixed: Arc<RoomFn> = Arc::new(|_| Some(1 << 30));
        let room = PlatformRoom(Some(fixed), coming.clone());
        assert_eq!(room.claimed(), 0);
        assert_eq!(room.read(0), Some(1 << 30));
        coming.expect(&mem, 5 << 20);
        assert_eq!(room.claimed(), 5 << 20);
        assert_eq!(room.read(0), Some((1 << 30) - (5 << 20)));
        // Beside the platform's room: over a room with nothing announced
        // by exactly what was, read where the buffers stood still.
        let (plat, bare) = (
            PlatformRoom(None, coming.clone()),
            PlatformRoom(None, Arc::default()),
        );
        let over = loop {
            let before = bare.claimed();
            let with = plat.claimed();
            if bare.claimed() == before {
                break with - before;
            }
        };
        assert_eq!(over, 5 << 20);
        // The file gone, nothing is coming, and what was said was said.
        drop(mem);
        assert_eq!(coming.bytes(), 0);
        assert_eq!(room.read(0), Some(1 << 30));
        assert_eq!(room.claimed(), 5 << 20);
    }

    /// A file a writer announces (`expect`) is out of the room its store's
    /// cache follows from where it is announced: the bytes it is still to
    /// hold, which shrink as it is written and are nothing once it holds
    /// what was said, has its envelope, or is gone. The room here is a
    /// test's, one figure, and the cache holds nothing, so its budget is
    /// the room less what is coming.
    #[test]
    fn a_file_announced_is_out_of_the_room_until_it_is_written() {
        const MIB: u64 = 1 << 20;
        let real = dir("coming");
        let room = 64 * MIB;
        let held = Vault::create(&real, &cipher(3).cache_room(move |_| Some(room))).unwrap();
        let (v, d) = (held.vault().clone(), held.dir());
        let budget = |want: u64, what: &str| {
            let t = std::time::Instant::now();
            while v.pager().budget() != want {
                assert!(
                    t.elapsed() < std::time::Duration::from_secs(10),
                    "{what}: a budget of {} for {want}, {} coming",
                    v.pager().budget(),
                    v.coming.bytes()
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        budget(room, "the room alone");
        // Written in two parts, the second past what was said.
        let mut f = create(d.join("compact-1.tmp")).unwrap();
        expect(d.join("compact-1.tmp"), 20 * MIB);
        assert_eq!(v.counts().expected, 20 * MIB);
        budget(room - 20 * MIB, "less the file announced");
        f.write_all(&vec![7u8; 8 << 20]).unwrap();
        budget(room - 12 * MIB, "less what of it is still to come");
        f.write_all(&vec![7u8; 16 << 20]).unwrap();
        budget(room, "a file written past what was said");
        drop(f);
        remove_file(d.join("compact-1.tmp")).unwrap();
        // Removed short of what was said.
        let f = create(d.join("compact-2.tmp")).unwrap();
        expect(d.join("compact-2.tmp"), 10 * MIB);
        budget(room - 10 * MIB, "less a second file");
        drop(f);
        remove_file(d.join("compact-2.tmp")).unwrap();
        budget(room, "a file removed");
        // Given its envelope short of what was said: the cache's own from
        // there, counted in what it holds.
        put(&d.join("par-3-key.sup"), &vec![9u8; 2 << 20]);
        expect(d.join("par-3-key.sup"), 9 * MIB);
        budget(room - 7 * MIB, "less a third");
        persist(&d, || vec![("par-3-key.sup".to_string(), None)]).unwrap();
        budget(room, "a file with its envelope");
        // A name the store does not hold, and a file that passes to the
        // directory under its own name, are announced to nobody.
        expect(d.join("compact-9.tmp"), 5 * MIB);
        expect(d.join("manifest"), 5 * MIB);
        assert_eq!(v.counts().expected, 39 * MIB);
        assert_eq!(v.coming.bytes(), 0);
        drop(held);
        let _ = std::fs::remove_dir_all(&real);
    }

    /// A kept file is written, read, mapped, renamed, linked and removed
    /// through the door as a file on disk is, and none of it reaches the
    /// directory: only a persist does, as an envelope whose name and bytes
    /// say nothing of the file's.
    #[test]
    fn a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope() {
        let real = dir("kept");
        let held = Vault::create(&real, &cipher(1)).unwrap();
        let d = held.dir();
        let body = b"the fence key-00750 and a value nobody may read".repeat(300);
        put(&d.join("seal-1.tmp"), &body);
        assert!(exists(d.join("seal-1.tmp")));
        assert_eq!(len(d.join("seal-1.tmp")).unwrap(), body.len() as u64);
        rename(d.join("seal-1.tmp"), d.join("par-1-key-00750.sup")).unwrap();
        assert!(!exists(d.join("seal-1.tmp")));
        assert_eq!(read(d.join("par-1-key-00750.sup")).unwrap(), body);
        let m = map(d.join("par-1-key-00750.sup")).unwrap();
        assert!(m.piecewise(), "a kept file is read by the piece");
        assert_eq!(m.lends(), 0, "a paged mapping lends nothing whole");
        // The touches below read a byte at each page's start; a sum of zero
        // says none was read only while those bytes are not zero.
        let page = crate::bytes::os_page_size();
        assert!(
            body.iter().step_by(page).all(|&b| b != 0),
            "the body has a byte that is not zero at every page's start"
        );
        assert_eq!(
            m.touch(0, body.len()),
            0,
            "and lends nothing that was not asked for"
        );
        {
            let _held = held.vault().pager().hold();
            assert_eq!(m.slice_at(0, body.len()).unwrap(), &body[..]);
            assert_eq!(m.slice_at(77, 5000).unwrap(), &body[77..5077]);
            assert!(m.slice_at(1, body.len()).is_none(), "past the end");
            assert_eq!(
                m.touch(0, body.len()),
                0,
                "a paged mapping is not touched, its pages present or not"
            );
        }
        let mut again = Vec::new();
        open(d.join("par-1-key-00750.sup"))
            .unwrap()
            .read_to_end(&mut again)
            .unwrap();
        assert_eq!(again, body);
        assert_eq!(
            on_disk(&real)
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec![MARKER],
            "nothing of a kept file is in the directory before a persist"
        );
        put(&d.join("ord-1.oidx"), b"an index over key-00750");
        let files = held
            .vault()
            .persist(&pair("par-1-key-00750.sup", Some("ord-1.oidx")), true)
            .unwrap();
        let (token, _) = files[0];
        let names: Vec<String> = on_disk(&real).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            vec![
                MARKER.to_string(),
                disk_name(&token, Kind::Seg),
                disk_name(&token, Kind::Ord)
            ]
        );
        for (name, bytes) in on_disk(&real) {
            assert!(!name.contains("00750"), "{name} carries the fence");
            assert!(!holds(&bytes, b"key-00750"), "{name} holds a key");
            assert!(!holds(&bytes, b"nobody may read"), "{name} holds a value");
        }
        // A name the store does not hold: an error for a manifest, which
        // would name nothing, and passed over for a seal's own call, whose
        // piece a promotion may have renamed under it. And the door's call
        // is nothing at all for a directory that is not a store's.
        assert!(held
            .vault()
            .persist(&pair("pcs-9.sup", None), true)
            .is_err());
        assert!(held
            .vault()
            .persist(&pair("pcs-9.sup", None), false)
            .unwrap()
            .is_empty());
        persist(&d, || pair("pcs-9.sup", None)).unwrap();
        persist(&real, || pair("pcs-9.sup", None)).unwrap();
        // A second persist writes nothing: the envelope is the file.
        let before = on_disk(&real);
        assert_eq!(
            held.vault()
                .persist(&pair("par-1-key-00750.sup", Some("ord-1.oidx")), true)
                .unwrap()[0]
                .0,
            token
        );
        assert_eq!(on_disk(&real), before);
        // A link shares the envelope, and the envelope goes with the last
        // name over it.
        hard_link(d.join("par-1-key-00750.sup"), d.join("par-1-wide.sup")).unwrap();
        assert!(hard_link(d.join("par-1-key-00750.sup"), d.join("par-1-wide.sup")).is_err());
        remove_file(d.join("par-1-key-00750.sup")).unwrap();
        assert!(real.join(disk_name(&token, Kind::Seg)).exists());
        assert_eq!(read(d.join("par-1-wide.sup")).unwrap(), body);
        remove_file(d.join("par-1-wide.sup")).unwrap();
        assert!(!real.join(disk_name(&token, Kind::Seg)).exists());
        assert!(remove_file(d.join("par-1-wide.sup")).is_err());
        remove_file(d.join("ord-1.oidx")).unwrap();
        assert!(!real.join(disk_name(&token, Kind::Ord)).exists());
    }

    /// A file operation that did not come through the door finds no
    /// directory to act in, and writes nothing.
    #[test]
    fn a_path_the_door_is_not_in_front_of_writes_nothing() {
        let real = dir("decoy");
        let held = Vault::create(&real, &cipher(1)).unwrap();
        let d = held.dir();
        assert!(OsFile::create(d.join("seg-1.sup")).is_err());
        assert!(std::fs::write(d.join("seg-1.sup"), b"plaintext").is_err());
        assert!(std::fs::read_dir(&d).is_err());
        assert_eq!(on_disk(&real).len(), 1, "the marker alone");
        // A name nobody classed is kept, not passed to the directory.
        put(&d.join("something-new"), b"plaintext nobody planned for");
        assert_eq!(on_disk(&real).len(), 1);
        drop(held);
        // And with the store closed the door is the standard library's
        // again, which finds the same nothing.
        assert!(create(d.join("seg-1.sup")).is_err());
    }

    /// The log and the manifest are the directory's own files, under their
    /// own names: the engine seals what it writes into them.
    #[test]
    fn the_log_and_the_manifest_pass_to_the_directory() {
        let real = dir("pass");
        let held = Vault::create(&real, &cipher(1)).unwrap();
        let d = held.dir();
        put(&d.join("wal-00000000"), b"sealed by the engine");
        put(&d.join("manifest.tmp"), b"sealed too");
        rename(d.join("manifest.tmp"), d.join("manifest")).unwrap();
        assert_eq!(
            std::fs::read(real.join("wal-00000000")).unwrap(),
            b"sealed by the engine"
        );
        assert_eq!(std::fs::read(real.join("manifest")).unwrap(), b"sealed too");
        assert_eq!(len(d.join("manifest")).unwrap(), 10);
        assert_eq!(super::real(d.join("manifest")), real.join("manifest"));
        assert_eq!(super::real(d.join("seg-1.sup")), d.join("seg-1.sup"));
        assert_eq!(super::real(real.join("manifest")), real.join("manifest"));
        let mut listed = names(&d).unwrap();
        listed.sort();
        assert_eq!(listed, vec!["manifest", "wal-00000000"]);
        assert!(rename(d.join("manifest"), d.join("seg-1.sup")).is_err());
        sync_dir(&d).unwrap();
        remove_file(d.join("wal-00000000")).unwrap();
        assert!(!exists(d.join("wal-00000000")));
    }

    /// A store reopened finds a file a manifest named by its token and its
    /// wrapped key, reads it back as it was written, and sweeps an envelope
    /// no manifest named; a file with an envelope is not written again.
    #[test]
    fn a_reopened_store_reads_what_was_named_and_sweeps_what_was_not() {
        let real = dir("reopen");
        let body: Vec<u8> = (0..300_000u32).map(|i| (i * 7 + i / 251) as u8).collect();
        let (token, wrapped, orphan) = {
            let held = Vault::create(&real, &cipher(1)).unwrap();
            let d = held.dir();
            put(&d.join("seg-1.sup"), &body);
            put(&d.join("ord-1.oidx"), b"index");
            put(&d.join("seg-2.sup"), b"sealed and never named");
            let v = held.vault();
            let (token, wrapped) = v
                .persist(&pair("seg-1.sup", Some("ord-1.oidx")), true)
                .unwrap()[0];
            let (orphan, _) = v.persist(&pair("seg-2.sup", None), true).unwrap()[0];
            assert_eq!(v.counts.persisted.load(Ordering::Relaxed), 3);
            (token, wrapped, orphan)
        };
        let held = Vault::open(&real, &cipher(1)).unwrap();
        let d = held.dir();
        let v = held.vault();
        assert!(
            names(&d).unwrap().is_empty(),
            "nothing is known before a manifest"
        );
        v.adopt("seg-1.sup", token, wrapped, Some("ord-1.oidx"));
        v.adopt("par-1.sup", token, wrapped, None);
        assert_eq!(v.sweep().unwrap(), 1);
        assert!(!real.join(disk_name(&orphan, Kind::Seg)).exists());
        assert_eq!(len(d.join("seg-1.sup")).unwrap(), body.len() as u64);
        let m = map(d.join("seg-1.sup")).unwrap();
        assert_eq!(
            v.pager().counts().fills,
            0,
            "a length and a map decrypt nothing"
        );
        {
            let _held = v.pager().hold();
            assert_eq!(m.slice_at(100_000, 10).unwrap(), &body[100_000..100_010]);
        }
        assert_eq!(v.pager().counts().fills, 1, "a read decrypts its chunk");
        assert_eq!(read(d.join("seg-1.sup")).unwrap(), body);
        let chunks = body.len().div_ceil(pager::CHUNK) as u64;
        assert_eq!(v.pager().counts().fills, chunks);
        assert_eq!(read(d.join("par-1.sup")).unwrap(), body);
        assert_eq!(
            v.pager().counts().fills,
            chunks,
            "two names, one copy: nothing is decrypted twice"
        );
        assert!(
            open(d.join("seg-1.sup")).is_err(),
            "a file with its envelope has no descriptor to hand out"
        );
        assert_eq!(read(d.join("ord-1.oidx")).unwrap(), b"index");
        assert!(open_write(d.join("seg-1.sup")).is_err(), "written once");
        // The name another slot had it under goes, and the envelope stays
        // for the name that is left.
        remove_file(d.join("par-1.sup")).unwrap();
        assert!(real.join(disk_name(&token, Kind::Seg)).exists());
    }

    /// A byte changed anywhere in an envelope, or a wrapped key that is
    /// another file's, refuses the file.
    #[test]
    fn a_changed_envelope_is_refused() {
        let real = dir("damage");
        let body = vec![7u8; 9000];
        let (token, wrapped, other) = {
            let held = Vault::create(&real, &cipher(1)).unwrap();
            let d = held.dir();
            put(&d.join("seg-1.sup"), &body);
            put(&d.join("seg-2.sup"), &body);
            let v = held.vault();
            let a = v.persist(&pair("seg-1.sup", None), true).unwrap()[0];
            let b = v.persist(&pair("seg-2.sup", None), true).unwrap()[0];
            (a.0, a.1, b.1)
        };
        let path = real.join(disk_name(&token, Kind::Seg));
        let good = std::fs::read(&path).unwrap();
        for at in (0..good.len()).step_by(97) {
            let mut bad = good.clone();
            bad[at] ^= 1;
            std::fs::write(&path, &bad).unwrap();
            let held = Vault::open(&real, &cipher(1)).unwrap();
            held.vault().adopt("seg-1.sup", token, wrapped, None);
            // A header that is not one is refused where the file is
            // mapped, and a chunk that fails its tag where it is read;
            // either way the file's bytes are not answered.
            assert!(read(held.dir().join("seg-1.sup")).is_err(), "byte {at}");
        }
        std::fs::write(&path, &good).unwrap();
        let held = Vault::open(&real, &cipher(1)).unwrap();
        held.vault().adopt("seg-1.sup", token, other, None);
        assert!(
            map(held.dir().join("seg-1.sup")).is_err(),
            "another file's key"
        );
        held.vault().adopt("seg-9.sup", token, wrapped, None);
        assert_eq!(read(held.dir().join("seg-9.sup")).unwrap(), body);
        // An ordered index the directory does not hold is not a file of
        // the store: its segment's open finds none and composes it.
        held.vault()
            .adopt("seg-8.sup", token, wrapped, Some("ord-8.oidx"));
        assert!(exists(held.dir().join("seg-8.sup")));
        assert!(!exists(held.dir().join("ord-8.oidx")));
    }

    /// The marker is where a store is refused: a wrong key, a changed
    /// byte, a store that has none, a directory that holds something, a
    /// store open already; and it gives the key's name without the key.
    #[test]
    fn the_marker_refuses_what_is_not_the_store() {
        let real = dir("marker");
        assert_eq!(key_id(&real).unwrap(), None);
        assert!(
            Vault::open(&real, &cipher(1)).is_err(),
            "no store under a cipher here"
        );
        refuse_marked(&real).unwrap();
        {
            let held = Vault::create(&real, &cipher(1)).unwrap();
            assert!(Vault::open(&real, &cipher(1)).is_err(), "open already");
            drop(held);
        }
        assert_eq!(key_id(&real).unwrap(), Some(1));
        assert!(refuse_marked(&real).is_err());
        assert!(Vault::create(&real, &cipher(1)).is_err(), "not empty");
        assert!(Vault::open(&real, &cipher(2)).is_err(), "a wrong key");
        drop(Vault::open(&real, &cipher(1)).unwrap());
        let good = std::fs::read(real.join(MARKER)).unwrap();
        assert_eq!(good.len(), MARKER_LEN);
        for at in 0..good.len() {
            let mut bad = good.clone();
            bad[at] ^= 1;
            std::fs::write(real.join(MARKER), &bad).unwrap();
            assert!(Vault::open(&real, &cipher(1)).is_err(), "byte {at}");
        }
        std::fs::write(real.join(MARKER), &good[..good.len() - 1]).unwrap();
        assert!(Vault::open(&real, &cipher(1)).is_err(), "cut short");
        assert!(key_id(&real).is_err());
        // The marker's form is checked by what reads it without the key
        // too, where no tag stands behind the check: its name, its version,
        // the word that is zero, its length.
        assert_eq!(marker_key_id(&good).unwrap(), 1);
        for at in [0, 7, 8, 11, 12, 15] {
            let mut bad = good.clone();
            bad[at] ^= 1;
            assert!(marker_key_id(&bad).is_err(), "byte {at}");
        }
        let mut long = good.clone();
        long.push(0);
        assert!(marker_key_id(&long).is_err());
        let mut other = good.clone();
        other[16] ^= 1;
        assert_eq!(
            marker_key_id(&other).unwrap(),
            0,
            "the key's name is in the clear"
        );
    }

    /// The caller's key rotated: the store opens under the new one and no
    /// longer under the old, with the new key's name, and what it wrote
    /// under the old reads back, since no file's key moved.
    #[test]
    fn a_rewrapped_store_opens_under_the_new_key_alone() {
        let real = dir("rewrap");
        let (token, wrapped) = {
            let held = Vault::create(&real, &cipher(1)).unwrap();
            put(
                &held.dir().join("seg-1.sup"),
                b"written under the first key",
            );
            held.vault()
                .persist(&pair("seg-1.sup", None), true)
                .unwrap()[0]
        };
        assert!(
            rewrap(&real, &cipher(2), &cipher(3)).is_err(),
            "not the store's key"
        );
        assert_eq!(key_id(&real).unwrap(), Some(1));
        rewrap(&real, &cipher(1), &cipher(2)).unwrap();
        assert_eq!(key_id(&real).unwrap(), Some(2));
        assert!(
            Vault::open(&real, &cipher(1)).is_err(),
            "the old key opens nothing"
        );
        assert!(rewrap(&real, &cipher(1), &cipher(2)).is_err());
        let held = Vault::open(&real, &cipher(2)).unwrap();
        held.vault().adopt("seg-1.sup", token, wrapped, None);
        assert_eq!(
            read(held.dir().join("seg-1.sup")).unwrap(),
            b"written under the first key"
        );
        assert_eq!(
            on_disk(&real)
                .iter()
                .filter(|(n, _)| n.contains("tmp"))
                .count(),
            0
        );
    }

    /// A manifest sealed opens to what was sealed, and a byte changed, a
    /// sequence changed, a slot cut or another store's slot does not.
    #[test]
    fn a_sealed_manifest_opens_whole_or_not_at_all() {
        let real = dir("slot");
        let held = Vault::create(&real, &cipher(1)).unwrap();
        let v = held.vault();
        let files = vec![([3u8; 16], [4u8; WRAPPED]), ([5u8; 16], [6u8; WRAPPED])];
        let slot = v
            .seal_manifest(9, b"the manifest's own bytes", &files)
            .unwrap();
        assert!(!holds(&slot, b"manifest's own"));
        let (m, f) = v.open_manifest(&slot).unwrap();
        assert_eq!(
            (m.as_slice(), &f),
            (&b"the manifest's own bytes"[..], &files)
        );
        // Behind a longer slot's tail, as a slot written in place leaves it.
        let mut long = slot.clone();
        long.extend_from_slice(&[0xAB; 40]);
        assert!(v.open_manifest(&long).is_ok());
        for at in 0..slot.len() {
            let mut bad = slot.clone();
            bad[at] ^= 1;
            assert!(v.open_manifest(&bad).is_err(), "byte {at}");
        }
        assert!(v.open_manifest(&slot[..slot.len() - 1]).is_err());
        assert!(v.open_manifest(&[]).is_err());
        let empty = v.seal_manifest(0, b"", &[]).unwrap();
        assert_eq!(v.open_manifest(&empty).unwrap(), (Vec::new(), Vec::new()));
        // A slot sealed under the store's own key whose files are not whole
        // files: what no tag refuses, and the parser does.
        let mut body = 3u32.to_le_bytes().to_vec();
        body.extend_from_slice(b"abc");
        body.extend_from_slice(&[9u8; 16 + WRAPPED + 5]);
        let sealed = seal(&*v.store, v.nonce().unwrap(), &slot_aad(4), &body);
        let mut ragged = SLOT_MAGIC.to_vec();
        ragged.extend_from_slice(&4u64.to_le_bytes());
        ragged.extend_from_slice(&(sealed.len() as u32).to_le_bytes());
        ragged.extend_from_slice(&sealed);
        assert!(v.open_manifest(&ragged).is_err(), "a ragged tail");
        // And a slot's files held to its manifest's names, one for one.
        let segs = pair("seg-1.sup", None);
        assert!(v.adopt_all(&segs, Vec::new()).is_err());
        assert!(v.adopt_all(&segs, files.clone()).is_err());
        assert!(
            names(held.dir()).unwrap().is_empty(),
            "a refused slot adopts nothing"
        );
        v.adopt_all(&segs, files[..1].to_vec()).unwrap();
        assert_eq!(names(held.dir()).unwrap(), vec!["seg-1.sup"]);
        // Two seals of the same bytes are not the same bytes.
        assert_ne!(
            v.seal_manifest(9, b"the manifest's own bytes", &files)
                .unwrap(),
            slot
        );
        let other_real = dir("slot-other");
        let other = Vault::create(&other_real, &cipher(1)).unwrap();
        assert!(
            other.vault().open_manifest(&slot).is_err(),
            "another store's key"
        );
    }

    /// A log's records open in order to what was sealed, and stop at the
    /// first that was changed, moved, cut, or is another file's.
    #[test]
    fn a_log_opens_to_its_records_and_ends_at_the_first_that_fails() {
        let real = dir("log");
        let held = Vault::create(&real, &cipher(1)).unwrap();
        let v = held.vault();
        let (mut log, wrapped) = v.log_begin(4).unwrap();
        let batches: [&[u8]; 3] = [b"first batch", b"the second, longer batch", b"3"];
        let mut file = vec![0u8; 68];
        let mut rec = Vec::new();
        let mut ends = Vec::new();
        for b in batches {
            log.seal_into(file.len() as u64, b, &mut rec).unwrap();
            file.extend_from_slice(&rec);
            ends.push(file.len() as u64);
        }
        assert!(!holds(&file, b"second"));
        let log = v.log_resume(4, &wrapped).unwrap();
        let mut plain = b"HEAD".to_vec();
        let got = log.open_records(&file, 68, &mut plain);
        assert_eq!(plain, b"HEADfirst batchthe second, longer batch3");
        assert_eq!(got, vec![(15, ends[0]), (39, ends[1]), (40, ends[2])]);
        // Any byte of the second record changed: the first alone.
        for at in ends[0] as usize..ends[1] as usize {
            let mut bad = file.clone();
            bad[at] ^= 1;
            let mut plain = Vec::new();
            let got = log.open_records(&bad, 68, &mut plain);
            assert_eq!(
                (plain.as_slice(), got.len()),
                (&b"first batch"[..], 1),
                "byte {at}"
            );
        }
        // Cut anywhere: the records whole before the cut.
        for cut in 68..file.len() {
            let mut plain = Vec::new();
            let got = log.open_records(&file[..cut], 68, &mut plain);
            assert_eq!(
                got.len(),
                ends.iter().filter(|&&e| e as usize <= cut).count()
            );
        }
        // The second record in the first's place is not the first.
        let mut moved = file[..68].to_vec();
        moved.extend_from_slice(&file[ends[0] as usize..ends[1] as usize]);
        assert!(log.open_records(&moved, 68, &mut Vec::new()).is_empty());
        // Another file's cipher, and another file's wrapped key.
        assert!(v.log_resume(5, &wrapped).is_none());
        let (other, _) = v.log_begin(4).unwrap();
        assert!(other.open_records(&file, 68, &mut Vec::new()).is_empty());
        // Zeroes, as a file's unwritten tail reads, are not a record.
        let mut zeros = file.clone();
        zeros.extend_from_slice(&[0u8; 64]);
        assert_eq!(log.open_records(&zeros, 68, &mut Vec::new()).len(), 3);
    }

    /// Nonces under one key never repeat, across the count's end too.
    #[test]
    fn nonces_do_not_repeat() {
        let mut n = Nonces::new().unwrap();
        let (a, b) = (n.take().unwrap(), n.take().unwrap());
        assert_ne!(a, b);
        assert_eq!(a[..8], b[..8]);
        n.next = u32::MAX;
        let last = n.take().unwrap();
        let after = n.take().unwrap();
        assert_eq!(last[8..], u32::MAX.to_le_bytes());
        assert_eq!(after[8..], 0u32.to_le_bytes());
        assert_ne!(
            last[..8],
            after[..8],
            "a spent count draws its eight bytes again"
        );
    }
}
