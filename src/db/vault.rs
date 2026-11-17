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

use crate::bytes::{Bytes, MmapBytes};
use crate::crypt::{self, Aead, EncryptOptions, NONCE, TAG};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Result, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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
}

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
        }
    }

    /// The plaintext of its files the store may hold in memory, in bytes:
    /// what it has decrypted and not given back. A store reads through
    /// this cache,
    /// decrypting a chunk of a file where it is first read and giving the
    /// chunks least lately read back once the cache is past this, so a
    /// store of any size runs in it, at the rate of its misses. Unset, it
    /// is half of the memory the process may use (its control group's
    /// limit or the machine's, less what the process already holds), and
    /// sixty-four mebibytes at least; the other half is for the kernel's
    /// cache of the store's files, which is what a miss is read from.
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
        File::open("/dev/urandom")?.read_exact(buf)
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
        let mut f = File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(MARKER))?;
    File::open(dir)?.sync_all()
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
    mem: Option<Arc<File>>,
    page: Option<Arc<Paged>>,
    disk: Option<Disk>,
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
    mem: Arc<File>,
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
pub(crate) struct Mapped(Option<(Arc<Vault>, Arc<File>)>);

impl Drop for Mapped {
    fn drop(&mut self) {
        if let Some((v, mem)) = self.0.take() {
            v.unmapped(&mem);
        }
    }
}

/// The cache a store is given where its cipher names none: half of what
/// `zfile::read_budget` says the process may use, the other half being the
/// kernel's cache of the envelopes a miss is read from.
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

/// The stores under a cipher open in this process, found by the directory
/// each gave its engine. The count is read first, so a process with none
/// pays a load a file operation.
static VAULTS: AtomicUsize = AtomicUsize::new(0);
static REGISTRY: Mutex<Vec<Arc<Vault>>> = Mutex::new(Vec::new());

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
fn reopen(mem: &File, write: bool, append: bool) -> Result<File> {
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
        self.file.write_all(data)?;
        #[cfg(target_os = "linux")]
        if self.hint {
            use std::os::fd::AsRawFd;
            // SAFETY: a hint over a range of a file this owns; the call
            // reads and writes no memory of ours.
            unsafe {
                libc::sync_file_range(
                    self.file.as_raw_fd(),
                    self.at as libc::off64_t,
                    data.len() as libc::off64_t,
                    libc::SYNC_FILE_RANGE_WRITE,
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = self.hint;
        self.at += data.len() as u64;
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Vault {
    fn new(real: &Path, store: Box<DynAead>, cipher: &StoreCipher) -> Result<Vault> {
        Ok(Vault {
            real: real.to_path_buf(),
            decoy: real.join(MARKER),
            store,
            data: cipher.data.clone(),
            files: Mutex::new(HashMap::new()),
            nonces: Mutex::new(Nonces::new()?),
            persisting: Mutex::new(()),
            mapped: Mutex::new(Vec::new()),
            pager: Pager::new(cipher.cache.unwrap_or_else(default_cache)),
            counts: Counts::default(),
        })
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
        // A temp name a creation's crash left, which holds nothing a
        // marker does not.
        let _ = std::fs::remove_file(real.join(format!("{MARKER}.tmp")));
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

    fn make(&self, name: &str) -> Result<File> {
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

    /// The envelope `disk` names, opened under its file's key.
    fn back_of(&self, disk: &Disk) -> Result<Back> {
        let key = self
            .unwrap(&Vault::file_aad(&disk.token), &disk.wrapped)
            .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
        Back::open(&self.real.join(disk.name()), (self.data)(&key.0))
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
        File::open(self.real.join(disk.name()))?.read_exact(&mut head)?;
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
                let _ = std::fs::remove_file(self.real.join(d.name()));
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
                        let _ = std::fs::remove_file(self.real.join(z.name()));
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
    fn map_written(&self, name: &str) -> Result<(File, Arc<File>)> {
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
    fn unmapped(&self, mem: &Arc<File>) {
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
    fn rewrite(&self, name: &str, append: bool) -> Result<File> {
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
    fn reopen_written(&self, name: &str) -> Result<File> {
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
    fn write_envelope(&self, mem: &File, disk: &Disk, key: &Key, sync: bool) -> Result<u64> {
        self.write_envelope_at(mem, &self.real.join(disk.name()), key, sync)
    }

    /// `write_envelope` to a path of the caller's choosing.
    fn write_envelope_at(&self, mem: &File, path: &Path, key: &Key, sync: bool) -> Result<u64> {
        let aead = (self.data)(&key.0);
        let len = mem.metadata()?.len();
        // Under a key of the file's own, which has no name of the caller's.
        let opts = EncryptOptions::default();
        let mut out = Ahead {
            file: File::create(path)?,
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
    fn keeps(&self, mem: &Arc<File>) -> bool {
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
        mem: &Arc<File>,
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
                    let keep = self.keeps(&mem);
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
                        let _ = std::fs::remove_file(self.real.join(disk.name()));
                    }
                    drop(files);
                    disk
                }
            };
            if let Some(ord) = ord {
                let e = lock(&self.files).get(ord).cloned();
                if let Some(Entry {
                    mem: Some(mem),
                    disk: None,
                    ..
                }) = e
                {
                    let key = self
                        .unwrap(&Vault::file_aad(&disk.token), &disk.wrapped)
                        .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
                    let odisk = Disk {
                        kind: Kind::Ord,
                        ..disk.clone()
                    };
                    let keep = self.keeps(&mem);
                    self.write_envelope(&mem, &odisk, &key, false)?;
                    made = true;
                    let mut files = lock(&self.files);
                    let mut held = false;
                    if let Some(e) = files.get_mut(ord) {
                        if e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, &mem)) {
                            e.disk = Some(odisk.clone());
                            held = true;
                        }
                    }
                    if held {
                        self.settled(&mut files, ord, &mem, &odisk, keep)?;
                    }
                    drop(files);
                }
            }
            out.push((disk.token, disk.wrapped));
        }
        if made && dir_sync {
            File::open(&self.real)?.sync_all()?;
        }
        Ok(out)
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
        let path = self.real.join(zdisk.name());
        let tmp = self.real.join(format!("{}{ENVELOPE_TMP}", zdisk.name()));
        self.write_envelope_at(&mem, &tmp, &key, sync)?;
        if cold {
            if let Ok(f) = File::open(&tmp) {
                drop_cached(&f);
            }
        }
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
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
            let _ = std::fs::remove_file(&path);
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
        if !self.real.join(zdisk.name()).exists() {
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
            if !files.contains_key(ord) && self.real.join(odisk.name()).exists() {
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
            .is_some_and(|d| self.real.join(d.name()).exists())
    }

    /// Every envelope in the directory that no file of the store is in,
    /// removed: one a crash came after and no manifest named, as a plain
    /// store sweeps a segment its manifest does not name. For the open,
    /// once the manifests have been read.
    pub(crate) fn sweep(&self) -> Result<usize> {
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
        let rest = &body[4 + mlen as usize..];
        if rest.len() % (16 + WRAPPED) != 0 {
            return Err(short());
        }
        let files = rest
            .chunks_exact(16 + WRAPPED)
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

/// `File::create`: the file made, or cut to nothing, and open for writing.
pub(crate) fn create(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    match locate(path) {
        None => File::create(path),
        Some((v, name)) if passes(&name) => File::create(v.real.join(name)),
        Some((v, name)) => v.make(&name),
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
    match locate(path) {
        None => plain(path),
        Some((v, name)) if passes(&name) => plain(&v.real.join(name)),
        Some((v, name)) => v.make(&name),
    }
}

/// `File::open`: a file that exists, for reading.
pub(crate) fn open(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    match locate(path) {
        None => File::open(path),
        Some((v, name)) if passes(&name) => File::open(v.real.join(name)),
        Some((v, name)) => v.reopen_written(&name),
    }
}

/// A file that exists, opened for writing where the caller says, cut
/// nowhere.
pub(crate) fn open_write(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| OpenOptions::new().write(true).open(p);
    match locate(path) {
        None => plain(path),
        Some((v, name)) if passes(&name) => plain(&v.real.join(name)),
        Some((v, name)) => v.rewrite(&name, false),
    }
}

/// A file opened to be read and appended to, made if it is not there.
pub(crate) fn open_append(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| {
        OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .open(p)
    };
    match locate(path) {
        None => plain(path),
        Some((v, name)) if passes(&name) => plain(&v.real.join(name)),
        Some((v, name)) => match v.rewrite(&name, true) {
            Err(e) if e.kind() == ErrorKind::NotFound => v.make(&name),
            r => r,
        },
    }
}

/// A file opened to be written in place, made if it is not there and cut
/// nowhere if it is: a manifest slot.
pub(crate) fn open_slot(path: impl AsRef<Path>) -> Result<File> {
    let path = path.as_ref();
    let plain = |p: &Path| {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(p)
    };
    match locate(path) {
        None => plain(path),
        Some((v, name)) if passes(&name) => plain(&v.real.join(name)),
        Some((v, name)) => match v.rewrite(&name, false) {
            Err(e) if e.kind() == ErrorKind::NotFound => v.make(&name),
            r => r,
        },
    }
}

/// `std::fs::rename`.
pub(crate) fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    match (locate(from), locate(to)) {
        (None, None) => std::fs::rename(from, to),
        (Some((v, a)), Some((w, b))) if Arc::ptr_eq(&v, &w) && passes(&a) == passes(&b) => {
            if passes(&a) {
                std::fs::rename(v.real.join(a), v.real.join(b))
            } else {
                v.rename(&a, &b)
            }
        }
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
    match locate(path) {
        None => Ok((File::open(path)?, Mapped(None))),
        Some((v, name)) if passes(&name) => Ok((File::open(v.real.join(name))?, Mapped(None))),
        Some((v, name)) => {
            let (file, mem) = v.map_written(&name)?;
            Ok((file, Mapped(Some((v, mem)))))
        }
    }
}

/// `std::fs::hard_link`.
pub(crate) fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    match (locate(from), locate(to)) {
        (None, None) => std::fs::hard_link(from, to),
        (Some((v, a)), Some((w, b))) if Arc::ptr_eq(&v, &w) && passes(&a) == passes(&b) => {
            if passes(&a) {
                std::fs::hard_link(v.real.join(a), v.real.join(b))
            } else {
                v.link(&a, &b)
            }
        }
        _ => Err(Error::new(
            ErrorKind::Unsupported,
            "a link between a store under a cipher and somewhere else",
        )),
    }
}

/// `std::fs::remove_file`.
pub(crate) fn remove_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    match locate(path) {
        None => std::fs::remove_file(path),
        Some((v, name)) if passes(&name) => std::fs::remove_file(v.real.join(name)),
        Some((v, name)) => v.remove(&name),
    }
}

/// `Path::exists`.
pub(crate) fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match locate(path) {
        None => path.exists(),
        Some((v, name)) if passes(&name) => v.real.join(name).exists(),
        Some((v, name)) => lock(&v.files).contains_key(&name),
    }
}

/// Whether `path`'s bytes are in the directory: the file, or under a
/// cipher the envelope a slot read named for it.
pub(crate) fn on_device(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match locate(path) {
        None => path.exists(),
        Some((v, name)) if passes(&name) => v.real.join(name).exists(),
        Some((v, name)) => v.on_device(&name),
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
    match locate(path) {
        None => path.is_file(),
        Some((v, name)) if passes(&name) => v.real.join(name).is_file(),
        Some((v, name)) => lock(&v.files).contains_key(&name),
    }
}

/// The file's length, as `std::fs::metadata` has it.
pub(crate) fn len(path: impl AsRef<Path>) -> Result<u64> {
    let path = path.as_ref();
    match locate(path) {
        None => Ok(std::fs::metadata(path)?.len()),
        Some((v, name)) if passes(&name) => Ok(std::fs::metadata(v.real.join(name))?.len()),
        Some((v, name)) => v.len(&name),
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
            Ok(out)
        }
    }
}

/// `std::fs::read`: the file's bytes.
pub(crate) fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    match locate(path) {
        None => std::fs::read(path),
        Some((v, name)) if passes(&name) => std::fs::read(v.real.join(name)),
        Some((v, name)) => {
            let page = v.page_of(&name)?;
            let _held = v.pager.hold();
            let src = MmapBytes::paged(page);
            let mut buf = vec![0u8; src.len() as usize];
            src.read_at(0, &mut buf)?;
            v.pager.check()?;
            Ok(buf)
        }
    }
}

/// The directory's entries made durable: an fsync of the directory.
pub(crate) fn sync_dir(dir: impl AsRef<Path>) -> Result<()> {
    let dir = dir.as_ref();
    match under(dir) {
        Some(v) => File::open(&v.real)?.sync_all(),
        None => File::open(dir)?.sync_all(),
    }
}

/// Where in the filesystem a file the engine names lies: the path itself,
/// or for a store under a cipher the file's path in the store's own
/// directory, which only the log and the manifest have -- a kept file's
/// plaintext is nowhere in the filesystem, and the path answered for one
/// is where nothing is.
pub(crate) fn real(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    match locate(path) {
        Some((v, name)) if passes(&name) => v.real.join(name),
        _ => path.to_path_buf(),
    }
}

/// A directory's own handle, for a caller that syncs it later.
pub(crate) fn open_dir(dir: impl AsRef<Path>) -> Result<File> {
    let dir = dir.as_ref();
    match under(dir) {
        Some(v) => File::open(&v.real),
        None => File::open(dir),
    }
}

/// The segments in `pairs`, each with its ordered index's name, made
/// durable where their fsync is not what makes them so: for a store under
/// a cipher, their envelopes written and synced now (`Vault::persist`),
/// on the thread that would have paid the fsync. Nothing for a plain
/// store, whose caller's fsync is the whole of it.
pub(crate) fn persist(dir: impl AsRef<Path>, pairs: &[(String, Option<String>)]) -> Result<()> {
    match under(dir.as_ref()) {
        Some(v) => v.persist(pairs, false).map(|_| ()),
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

/// A file's pages written back and dropped from the kernel's cache: the
/// writeback waited for by range, which asks for no flush of the device,
/// then the clean pages dropped.
fn drop_cached(file: &File) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: a writeback and an advice over a descriptor this call
        // borrows; neither changes a byte of the file.
        unsafe {
            libc::sync_file_range(
                file.as_raw_fd(),
                0,
                0,
                libc::SYNC_FILE_RANGE_WAIT_BEFORE
                    | libc::SYNC_FILE_RANGE_WRITE
                    | libc::SYNC_FILE_RANGE_WAIT_AFTER,
            );
            libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = file;
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

/// The file mapped for reading, as `MmapBytes::open` maps it. A kept
/// file's mapping is a paged one (`pager::Paged`): address space the
/// file's length, of which a range is read by asking for it, under a pin
/// of the store's reader table or a hold (`hold`).
pub(crate) fn map(path: impl AsRef<Path>) -> Result<MmapBytes> {
    let path = path.as_ref();
    match locate(path) {
        None => MmapBytes::open(path),
        Some((v, name)) if passes(&name) => MmapBytes::open(&v.real.join(name)),
        Some((v, name)) => Ok(MmapBytes::paged(v.page_of(&name)?)),
    }
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
        assert!(
            m.as_slice().is_empty(),
            "and lends nothing that was not asked for"
        );
        {
            let _held = held.vault().pager().hold();
            assert_eq!(m.slice_at(0, body.len()).unwrap(), &body[..]);
            assert_eq!(m.slice_at(77, 5000).unwrap(), &body[77..5077]);
            assert!(m.slice_at(1, body.len()).is_none(), "past the end");
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
        persist(&d, &pair("pcs-9.sup", None)).unwrap();
        persist(&real, &pair("pcs-9.sup", None)).unwrap();
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
        assert!(File::create(d.join("seg-1.sup")).is_err());
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

    /// The engine reaches its files through the door and no other way: its
    /// source, up to its own tests, names none of the standard library's
    /// calls the door stands for. A call that slipped past would fail under
    /// a cipher where a test reached it, and write beside the door where
    /// none did; this is the check for the second.
    #[test]
    fn the_engine_names_no_file_but_through_the_door() {
        let engine = include_str!("../db.rs");
        let tests = engine
            .rfind("\n#[cfg(test)]\nmod streaming_writes")
            .unwrap();
        let barred = [
            "File::open(",
            "File::create(",
            "OpenOptions",
            "fs::rename(",
            "fs::hard_link(",
            "fs::remove_file(",
            "fs::read_dir(",
            "fs::metadata(",
            "fs::read(",
            "fs::write(",
            "fs::copy(",
            "MmapBytes::open(",
            ".exists()",
        ];
        for (i, line) in engine[..tests].lines().enumerate() {
            let code = line.split("//").next().unwrap();
            for b in barred {
                assert!(!code.contains(b), "db.rs:{}: {b} past the door", i + 1);
            }
        }
        let index = include_str!("../ordindex.rs");
        let open = index.find("pub fn open(path: &Path, keys: usize)").unwrap();
        assert!(index[open..open + 200].contains("crate::db::vault::map(path)"));
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
