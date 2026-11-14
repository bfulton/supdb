//! A live store under a cipher the caller supplies (`Options::cipher`).
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
//! and with the cipher in the process that memory is the process's: each of
//! the store's files is a memory file (`memfd_create`), written and mapped
//! exactly as a file on disk is, and what reaches the directory is that
//! file's envelope (`crate::crypt`), written once the file is finished. A
//! read runs the code it runs over a plain store, over a mapping of memory
//! in place of a mapping of the page cache; the cipher runs once over a file
//! when it is written and once when the store is opened. So a store under a
//! cipher is held in memory whole -- it has to fit, the kernel cannot
//! reclaim it, and an open decrypts all of it -- which is the trade against
//! the kernel's way and why there are two. Each file is a descriptor held
//! for the file's life, a segment and its ordered index two, so a large
//! store wants the process's limit on open files above twice its segments.
//!
//! # The door
//!
//! The engine names a file by a path in its directory and reaches it
//! through the functions here (`create`, `open`, `map`, `rename`,
//! `remove_file`, `names`, ...), each of which is the standard library's
//! call for a plain store. A store under a cipher is given a directory that
//! cannot exist: its marker file, `supdb.cipher`, with a name after it. The
//! functions here find the store by that directory and act on its memory
//! files; a file operation that did not come through them fails with "not a
//! directory" and writes nothing, which is what keeps a path the door was
//! not put in front of from writing plaintext beside the ciphertext.
//!
//! # What is in the directory
//!
//! - `supdb.cipher`: the marker. The store's id, the caller's name for the
//!   key, and the store's own key wrapped under the caller's cipher. A
//!   store without one is not under a cipher, and one with it opens under
//!   no other.
//! - `x-<token>`: a segment, as an envelope under a key drawn for that
//!   file; `x-<token>.o`, its ordered index, under the same key. The names
//!   say nothing: a segment's own name carries its fences, sixteen bytes of
//!   two keys, and lives in the manifest.
//! - `manifest`, `manifest.alt`: the slots, each sealed under the store's
//!   key, naming each live segment with its token and its wrapped key.
//! - `wal-<id>`: the log, each commit a record sealed under a key drawn for
//!   that file and wrapped in its header.
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
//! Any byte changed in a segment, an index, a manifest or a log record is
//! found, and none is ever read as data. A segment that fails its check
//! fails the open. An ordered index that fails is composed again from its
//! segment. A manifest slot that fails is passed over for the other slot,
//! as a torn one is, and the store opens as the state that slot names or
//! is refused where the log no longer reaches back to it. A log record
//! that fails is where the log ends, as a torn tail is. So what a changed
//! byte can do is what a crash can do, and no more: the store opens as a
//! state it once acknowledged, or does not open.
//!
//! What is not refused is the store taken back to an earlier state of
//! itself on purpose: an old copy of the whole directory, or the log cut at
//! a record's end. Nothing in a directory can say that a newer one existed.
//! And what is not hidden: how many files the store has and how large each
//! is, the size of each commit in the log, and the caller's name for its
//! key.
//!
//! The plaintext is in memory, and memory can be written to swap and to a
//! core file; a host where that matters encrypts its swap and keeps its
//! core files where it keeps its keys.
//!
//! # What a store under a cipher runs without
//!
//! Ordered ingest's run, the compact form and the recycled log, whatever
//! the options say (`under_cipher` in the engine): each rests on a file the
//! engine makes durable by its own fsync, and that file would be a memory
//! file here. An ordered load therefore goes through the log.

use crate::bytes::{Bytes, MmapBytes};
use crate::crypt::{self, Aead, EncryptOptions, NONCE, TAG};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Result, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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
/// let db = Db::create(dir, Options { cipher: Some(cipher), ..Options::default() })?;
/// ```
#[derive(Clone)]
pub struct StoreCipher {
    master: Arc<DynAead>,
    data: Arc<Maker>,
    key_id: u64,
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
        }
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
            "the store is kept under a cipher and none was given (Options::cipher)",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- files --

/// Where a kept file's envelope is, and the key it is under.
#[derive(Clone)]
struct Disk {
    token: Token,
    wrapped: [u8; WRAPPED],
    /// The ordered index beside the segment the token names.
    ord: bool,
}

impl Disk {
    fn name(&self) -> String {
        disk_name(&self.token, self.ord)
    }

    fn same(&self, other: &Disk) -> bool {
        self.token == other.token && self.ord == other.ord
    }
}

fn disk_name(token: &Token, ord: bool) -> String {
    let mut s = String::with_capacity(36);
    s.push_str("x-");
    for b in token {
        s.push_str(&format!("{b:02x}"));
    }
    if ord {
        s.push_str(".o");
    }
    s
}

/// The token and kind an envelope's name carries, or nothing for a name
/// that is not one.
fn disk_parse(name: &str) -> Option<(Token, bool)> {
    let rest = name.strip_prefix("x-")?;
    let (hex, ord) = match rest.strip_suffix(".o") {
        Some(h) => (h, true),
        None => (rest, false),
    };
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut token = [0u8; 16];
    for (i, t) in token.iter_mut().enumerate() {
        *t = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some((token, ord))
}

/// One of the store's files: its plaintext in a memory file, once written
/// or read; its envelope in the directory, once written or found.
#[derive(Clone, Default)]
struct Entry {
    mem: Option<Arc<File>>,
    disk: Option<Disk>,
}

/// Whether `name` is a file whose bytes the engine seals itself, record by
/// record or slot by slot, and which therefore lives in the directory as
/// it is: the log and the manifest. Every other name is kept in memory
/// and reaches the directory as an envelope, a name nobody thought of
/// included.
fn passes(name: &str) -> bool {
    name.starts_with("wal-") || name.starts_with("manifest")
}

/// What a store under a cipher counts of its own work.
#[derive(Default)]
pub(crate) struct Counts {
    /// Envelopes written, and the plaintext bytes in them.
    pub(crate) persisted: AtomicU64,
    pub(crate) persisted_bytes: AtomicU64,
    /// Envelopes decrypted into memory, and the plaintext bytes of them.
    pub(crate) loaded: AtomicU64,
    pub(crate) loaded_bytes: AtomicU64,
    /// Envelopes no manifest named, removed at the open.
    pub(crate) swept: AtomicU64,
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
    pub(crate) counts: Counts,
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
        let mut reg = lock(&REGISTRY);
        if let Some(i) = reg.iter().position(|v| Arc::ptr_eq(v, &self.0)) {
            reg.swap_remove(i);
            VAULTS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// A memory file: anonymous, the process's, gone with its last handle.
#[cfg(target_os = "linux")]
fn memfile() -> Result<File> {
    use std::os::fd::FromRawFd;
    // SAFETY: a NUL-terminated name and a flag; the descriptor returned is
    // owned by nothing else.
    let fd = unsafe { libc::memfd_create(c"supdb".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        let e = Error::last_os_error();
        // A descriptor a file, held for the file's life: what a store of
        // many segments runs out of first, and says so.
        if matches!(e.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE)) {
            return Err(Error::new(
                e.kind(),
                "a store under a cipher holds a descriptor for each of its files, and the \
                 process has no more: raise its limit on open files",
            ));
        }
        return Err(e);
    }
    // SAFETY: a descriptor just made and owned here.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
fn memfile() -> Result<File> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "a store under a cipher keeps its files in memory files, which this platform has not",
    ))
}

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

/// The pages of a file the process will not read again, out of the page
/// cache: an envelope just written or just decrypted, whose plaintext is
/// what the store holds.
fn drop_cache(file: &File) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: advice on a descriptor this function borrows.
        unsafe {
            libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = file;
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
            counts: Counts::default(),
        })
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
        self.counts.read(files as u64)
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
                disk: None,
            },
        );
        Ok(handle)
    }

    /// The file's plaintext in memory: as written, or decrypted now from
    /// its envelope, whole, every chunk checked.
    fn load(&self, name: &str) -> Result<Arc<File>> {
        let disk = {
            let files = lock(&self.files);
            let e = files
                .get(name)
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
            if let Some(mem) = &e.mem {
                return Ok(mem.clone());
            }
            e.disk
                .clone()
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?
        };
        let key = self
            .unwrap(&Vault::file_aad(&disk.token), &disk.wrapped)
            .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
        let aead = (self.data)(&key.0);
        let path = self.real.join(disk.name());
        let mut mem = memfile()?;
        let hdr = {
            let src = MmapBytes::open(&path)?;
            let hdr = crypt::decrypt(&src, &mut mem, &*aead)?;
            src.advise_dontneed();
            hdr
        };
        if let Ok(f) = File::open(&path) {
            drop_cache(&f);
        }
        Counts::add(&self.counts.loaded, 1);
        Counts::add(&self.counts.loaded_bytes, hdr.plain_len());
        let mem = Arc::new(mem);
        let mut files = lock(&self.files);
        // Every name the envelope is under takes the one copy: a segment a
        // promotion linked is two names over one file.
        for e in files.values_mut() {
            if e.mem.is_none() && e.disk.as_ref().is_some_and(|d| d.same(&disk)) {
                e.mem = Some(mem.clone());
            }
        }
        Ok(files.get(name).and_then(|e| e.mem.clone()).unwrap_or(mem))
    }

    /// The files in `names` read into memory ahead of their first use, as
    /// many at once as the machine has cores for, up to eight: for the
    /// open, which reads every segment it names and would decrypt them one
    /// after another on its own thread, at a core's rate where the store's
    /// size over that rate is the whole of the open. A file that fails here
    /// fails again where it is opened, by whoever can say what its failure
    /// means -- a segment's is the open's, an ordered index's is a rebuild
    /// -- so nothing is answered.
    pub(crate) fn preload(&self, names: &[String]) {
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(8)
            .min(names.len());
        if threads < 2 {
            return;
        }
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    while let Some(name) = names.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let _ = self.load(name);
                    }
                });
            }
        });
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

    /// A kept file opened to be written again. One that has an envelope is
    /// refused: the envelope would no longer be the file, and nothing
    /// would say so.
    fn rewrite(&self, name: &str, append: bool) -> Result<File> {
        let mem = {
            let files = lock(&self.files);
            let e = files
                .get(name)
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
            if e.disk.is_some() {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "a file of a store under a cipher is written once: it has its envelope",
                ));
            }
            e.mem
                .clone()
                .ok_or_else(|| Error::from(ErrorKind::NotFound))?
        };
        reopen(&mem, true, append)
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
    /// cipher (`Ahead`), and the cache's copy of the ciphertext dropped
    /// once it is synced: the plaintext is what the store reads, and the
    /// ciphertext is read again at the next open and not before. Written
    /// past the cache instead (`O_DIRECT`, a megabyte at a time), the same
    /// envelope took half as long again: each piece waits for the device
    /// where the cached pieces go to it together.
    fn write_envelope(&self, mem: &File, disk: &Disk, key: &Key, sync: bool) -> Result<u64> {
        let aead = (self.data)(&key.0);
        let len = mem.metadata()?.len();
        // Under a key of the file's own, which has no name of the caller's.
        let opts = EncryptOptions::default();
        let mut out = Ahead {
            file: File::create(self.real.join(disk.name()))?,
            at: 0,
            hint: sync,
        };
        if len == 0 {
            crypt::encrypt(&crate::bytes::SliceBytes(&[]), &mut out, &*aead, &opts)?;
        } else {
            // SAFETY: a mapping of a memory file the store owns, whose
            // writer has finished with it.
            let src = MmapBytes(unsafe { memmap2::Mmap::map(mem)? });
            crypt::encrypt(&src, &mut out, &*aead, &opts)?;
        }
        let out = out.file;
        if sync {
            out.sync_all()?;
            drop_cache(&out);
        }
        Counts::add(&self.counts.persisted, 1);
        Counts::add(&self.counts.persisted_bytes, len);
        Ok(len)
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
    pub(crate) fn persist(
        &self,
        pairs: &[(String, Option<String>)],
        named: bool,
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
                        ord: false,
                    };
                    self.write_envelope(&mem, &disk, &key, true)?;
                    made = true;
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
                    if !held {
                        // Removed while its envelope was written: nobody's.
                        let _ = std::fs::remove_file(self.real.join(disk.name()));
                    }
                    disk
                }
            };
            if let Some(ord) = ord {
                let e = lock(&self.files).get(ord).cloned();
                if let Some(Entry {
                    mem: Some(mem),
                    disk: None,
                }) = e
                {
                    let key = self
                        .unwrap(&Vault::file_aad(&disk.token), &disk.wrapped)
                        .ok_or_else(|| refused("a file's key does not unwrap under the store's"))?;
                    let odisk = Disk {
                        ord: true,
                        ..disk.clone()
                    };
                    self.write_envelope(&mem, &odisk, &key, false)?;
                    made = true;
                    let mut files = lock(&self.files);
                    if let Some(e) = files.get_mut(ord) {
                        if e.mem.as_ref().is_some_and(|m| Arc::ptr_eq(m, &mem)) {
                            e.disk = Some(odisk);
                        }
                    }
                }
            }
            out.push((disk.token, disk.wrapped));
        }
        if made {
            File::open(&self.real)?.sync_all()?;
        }
        Ok(out)
    }

    /// A segment a manifest names, with the envelope it is in, and its
    /// ordered index where the directory holds one: known to the store
    /// from here, and decrypted when something first reads it.
    fn adopt(&self, seg: &str, token: Token, wrapped: [u8; WRAPPED], ord: Option<&str>) {
        let mut files = lock(&self.files);
        let disk = Disk {
            token,
            wrapped,
            ord: false,
        };
        // The copy another name over the same envelope has read already.
        let mem_of = |files: &HashMap<String, Entry>, d: &Disk| {
            files
                .values()
                .find(|e| e.mem.is_some() && e.disk.as_ref().is_some_and(|x| x.same(d)))
                .and_then(|e| e.mem.clone())
        };
        if !files.contains_key(seg) {
            let mem = mem_of(&files, &disk);
            files.insert(
                seg.to_string(),
                Entry {
                    mem,
                    disk: Some(disk.clone()),
                },
            );
        }
        if let Some(ord) = ord {
            let odisk = Disk { ord: true, ..disk };
            if !files.contains_key(ord) && self.real.join(odisk.name()).exists() {
                let mem = mem_of(&files, &odisk);
                files.insert(
                    ord.to_string(),
                    Entry {
                        mem,
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

    /// Every envelope in the directory that no file of the store is in,
    /// removed: one a crash came after and no manifest named, as a plain
    /// store sweeps a segment its manifest does not name. For the open,
    /// once the manifests have been read.
    pub(crate) fn sweep(&self) -> Result<usize> {
        let files = lock(&self.files);
        let mut swept = 0;
        for entry in std::fs::read_dir(&self.real)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            let Some((token, ord)) = disk_parse(&name) else {
                continue;
            };
            let held = files.values().any(|e| {
                e.disk
                    .as_ref()
                    .is_some_and(|d| d.token == token && d.ord == ord)
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

    fn read(&self, files: u64) -> CipherCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        CipherCounts {
            files,
            persisted: get(&self.persisted),
            persisted_bytes: get(&self.persisted_bytes),
            loaded: get(&self.loaded),
            loaded_bytes: get(&self.loaded_bytes),
            swept: get(&self.swept),
        }
    }
}

/// What a store under a cipher has done with it since it was opened
/// (`Db::cipher_counts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CipherCounts {
    /// The store's files held in memory now, each a memory file with a
    /// descriptor of its own: a segment and its ordered index are two.
    pub files: u64,
    /// Files written to the directory as envelopes, and the plaintext
    /// bytes sealed into them.
    pub persisted: u64,
    pub persisted_bytes: u64,
    /// Envelopes decrypted into memory, and the plaintext bytes of them.
    pub loaded: u64,
    pub loaded_bytes: u64,
    /// Envelopes no manifest named, removed at the open.
    pub swept: u64,
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
        Some((v, name)) => reopen(&*v.load(&name)?, false, false),
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
            let mem = v.load(&name)?;
            let mut buf = vec![0u8; mem.metadata()?.len() as usize];
            mem.read_exact_at(&mut buf, 0)?;
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

/// The file mapped for reading, as `MmapBytes::open` maps it.
pub(crate) fn map(path: impl AsRef<Path>) -> Result<MmapBytes> {
    let path = path.as_ref();
    match locate(path) {
        None => MmapBytes::open(path),
        Some((v, name)) if passes(&name) => MmapBytes::open(&v.real.join(name)),
        Some((v, name)) => {
            let mem = v.load(&name)?;
            // SAFETY: the contract `MmapBytes::open` takes: the file is
            // written once and not again while a reader holds it, and a
            // memory file is nobody's but the store's.
            let map = unsafe { memmap2::Mmap::map(&*mem)? };
            // Every page of it mapped here, by whoever opens the file: the
            // pages are in memory already, a memory file's are small, and
            // a first touch through an empty table maps sixteen of them a
            // fault where a plain file's megabyte folios map one a fault.
            // The scans after a load took 220 faults where a plain store's
            // took 40, a fifth of a pass of a thousand scans.
            #[cfg(target_os = "linux")]
            let _ = map.advise(memmap2::Advice::PopulateRead);
            Ok(MmapBytes(map))
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
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

    fn cipher(key: u8) -> StoreCipher {
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
        assert_eq!(&m.0[..], &body[..]);
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
                disk_name(&token, false),
                disk_name(&token, true)
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
        assert!(real.join(disk_name(&token, false)).exists());
        assert_eq!(read(d.join("par-1-wide.sup")).unwrap(), body);
        remove_file(d.join("par-1-wide.sup")).unwrap();
        assert!(!real.join(disk_name(&token, false)).exists());
        assert!(remove_file(d.join("par-1-wide.sup")).is_err());
        remove_file(d.join("ord-1.oidx")).unwrap();
        assert!(!real.join(disk_name(&token, true)).exists());
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
        assert!(!real.join(disk_name(&orphan, false)).exists());
        assert_eq!(len(d.join("seg-1.sup")).unwrap(), body.len() as u64);
        assert_eq!(
            v.counts.loaded.load(Ordering::Relaxed),
            0,
            "a length decrypts nothing"
        );
        assert_eq!(&map(d.join("seg-1.sup")).unwrap().0[..], &body[..]);
        assert_eq!(read(d.join("par-1.sup")).unwrap(), body);
        assert_eq!(
            v.counts.loaded.load(Ordering::Relaxed),
            1,
            "two names, one copy"
        );
        assert_eq!(read(d.join("ord-1.oidx")).unwrap(), b"index");
        assert!(open_write(d.join("seg-1.sup")).is_err(), "written once");
        // The name another slot had it under goes, and the envelope stays
        // for the name that is left.
        remove_file(d.join("par-1.sup")).unwrap();
        assert!(real.join(disk_name(&token, false)).exists());
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
        let path = real.join(disk_name(&token, false));
        let good = std::fs::read(&path).unwrap();
        for at in (0..good.len()).step_by(97) {
            let mut bad = good.clone();
            bad[at] ^= 1;
            std::fs::write(&path, &bad).unwrap();
            let held = Vault::open(&real, &cipher(1)).unwrap();
            held.vault().adopt("seg-1.sup", token, wrapped, None);
            assert!(map(held.dir().join("seg-1.sup")).is_err(), "byte {at}");
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
