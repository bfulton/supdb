//! A store under a cipher read by the piece: the plaintext of its files
//! held in a cache of bounded size, decrypted where it is first read and
//! given back where it has not been read for a while.
//!
//! A store reads by lending from mappings, and under a cipher the caller
//! supplies (`Encryption::Cipher`, `db::vault`) the plaintext is the
//! process's to hold: the kernel cannot decrypt a page into its cache.
//! Here a file is address space its plaintext's length, of which a chunk
//! is present once something has asked for it, and the chunks nobody has
//! asked for lately are given back when the cache passes its budget. A store of any size opens by reading its files' headers, reads
//! at the rate of its misses, and holds the plaintext it was told it may.
//!
//! ## The pieces
//!
//! `Paged` is one file: a shared mapping of a memory file the plaintext's
//! length, a state a chunk, and the envelope (`supdb::crypt`) the chunks
//! are decrypted from. It is what stands behind a `bytes::MmapBytes` of
//! the paged kind, and `ensure` is the one way in: the readers of the
//! format ask their source for every range before they read it
//! (`Bytes::piecewise`, `flatindex::Pieces`), and for a paged file the ask
//! is a look at each chunk's state, one load where the chunk is present.
//!
//! `Pager` is the store's cache: what it holds against what it may, the
//! thread that gives chunks back, and the failure it remembers.
//!
//! ## What is given back, and when it is safe to
//!
//! A chunk is a stored page less its tag, so two chunks share a page of
//! plaintext and a page cannot be given back for one of them. What is
//! given back is a unit: the least run of chunks that is whole pages,
//! 256 of them, a megabyte. A unit with a chunk read since the evictor
//! last looked is passed over and its chunks marked unread, so a unit
//! goes when a whole pass has found nothing of it wanted.
//!
//! A reader holds what it was lent for the length of its operation, and
//! nothing says where in the mapping that is. So a unit is given back in
//! three steps. Its chunks are marked dying, after which a reader that
//! asks for one takes the unit back for itself (`rescue`) or, if the pages
//! are already going, waits for them to be gone and decrypts again. Then
//! the evictor waits out every reader that could have been lent a byte
//! before the mark: the store's reader table, by the epoch it bumps after
//! marking (`Grace`), and the threads that read with no slot in it, by the
//! era each stands at (`Hold`). Then the pages go. A reader pins before it
//! asks and the evictor marks before it reads the pins, both sequentially
//! consistent, so one of them sees the other.
//!
//! Addresses do not move, and a chunk decrypted again is the same bytes
//! at the same place: a reader that rests (`Hold::rest`) may keep a borrow
//! across it as long as it asks for the range again before it reads it.
//!
//! ## What can go wrong
//!
//! A chunk that fails its tag, or an envelope that is not what its header
//! says, is the store damaged or changed, found at the read that first
//! wants the bytes and not at the open. The read path's answers are `Option`s in
//! which a failed read and an absent key look the same, so the pager
//! remembers the failure (`poison`) and every operation of the store
//! answers with it from then on (`Pager::check`): a store under a cipher
//! does not answer a read its bytes failed.

use crate::bytes::Paging;
use crate::crypt::{self, Aead, Header, TAG};
use std::cell::Cell;
use std::fs::File;
use std::io::{Error, ErrorKind, Result};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::time::Duration;

type DynAead = dyn Aead + Send + Sync;

/// A chunk's states. `COLD` and `HOT` are a chunk present, lent to whoever
/// asks; `HOT` is one asked for since the evictor last looked. `DYING` is
/// a chunk present in a unit the evictor has marked: not lent until the
/// unit is taken back.
const EMPTY: u8 = 0;
const BUSY: u8 = 1;
const COLD: u8 = 2;
const HOT: u8 = 3;
const DYING: u8 = 4;

/// A unit's states: lent from, marked and waiting out its readers, and its
/// pages going.
const LIVE: u8 = 0;
const EVICTING: u8 = 1;
const FREEING: u8 = 2;

/// The stored chunk a store's envelopes are written in, and the plaintext
/// in one: `crypt`'s default, which is what `Vault::write_envelope` writes.
const CHUNK_SHIFT: u32 = crypt::DEFAULT_CHUNK_SHIFT;
pub(crate) const CHUNK: usize = (1 << CHUNK_SHIFT) - TAG;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn page_size() -> usize {
    crate::bytes::os_page_size().max(4096)
}

/// Chunks in a unit: the least count whose plaintext is whole pages. A
/// chunk is `2^12 - 16` bytes, so 256 of them are 255 pages of four
/// kilobytes, and on a machine of larger pages as many times that as the
/// page is larger.
fn unit_chunks() -> usize {
    256 * (page_size() / 4096)
}

// ------------------------------------------------------------------ lanes --

thread_local! {
    /// Pins this thread holds that an evictor waits out, of either kind: a
    /// hold on a pager (`Pager::hold`), or a slot of a store's reader table
    /// pinned (`debug_pinned`). Counted under debug assertions alone, for
    /// the check in `Paged::ensure` that nothing is lent to a thread with
    /// neither.
    static PINS: Cell<u32> = const { Cell::new(0) };
}

/// A thread's reads of a store's paged files protected for as long as this
/// lives: what a thread with no slot in the store's reader table holds
/// while it reads them -- a merge over its inputs, an open over its
/// segments. It stands at the era the pager was in when it was taken, and
/// the evictor, having marked what it means to give back, moves the era on
/// and waits for every hold to be at the new one or gone.
pub(crate) struct Hold {
    pager: Arc<Pager>,
    at: Arc<AtomicU64>,
    /// The thread's own: the debug count of pins is the thread's.
    _thread: std::marker::PhantomData<*const ()>,
}

impl Hold {
    /// The hold at the era as it stands: a store, and the era read again,
    /// so an era the evictor moved between the two is not the one left
    /// standing. Sequentially consistent, as the evictor's move and its
    /// reads of the holds are: whichever of the two came second sees the
    /// first.
    fn pin(&self) {
        loop {
            let e = self.pager.era.load(Ordering::SeqCst);
            self.at.store(e, Ordering::SeqCst);
            if self.pager.era.load(Ordering::SeqCst) == e {
                return;
            }
        }
    }

    /// A point where this thread reads nothing it was lent before: the
    /// hold moves to the era as it stands, so what the evictor marked
    /// before now is no longer waited for on this thread's account. The
    /// caller asks again for whatever it means to read after; a range
    /// asked for again is the same bytes at the same address.
    ///
    /// And where the cache is well past its budget, a wait for it to come
    /// back (`Pager::breathe`), holding nothing: a merge reads its inputs
    /// in order at the cipher's rate, hundreds of megabytes a second, and
    /// whenever the evictor waited fifty milliseconds for some reader the
    /// merges beside it had read the store into the cache.
    pub(crate) fn rest(&self) {
        if self.pager.crowded() {
            self.at.store(0, Ordering::SeqCst);
            self.pager.breathe();
        }
        self.pin();
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        debug_pinned(false);
        self.at.store(0, Ordering::SeqCst);
        lock(&self.pager.holds).retain(|a| !Arc::ptr_eq(a, &self.at));
    }
}

/// Count a pin of the other kind taken or given up, under debug assertions:
/// `db::Readers::pin` and `unpin`.
#[inline]
pub(crate) fn debug_pinned(taken: bool) {
    #[cfg(debug_assertions)]
    {
        let _ = PINS.try_with(|p| {
            p.set(if taken {
                p.get() + 1
            } else {
                p.get().saturating_sub(1)
            })
        });
    }
    #[cfg(not(debug_assertions))]
    let _ = taken;
}

/// The store's reader table, as the evictor needs it: a new epoch, and the
/// oldest epoch a reader is pinned at (`db::Readers`).
pub(crate) trait Grace: Send + Sync {
    /// Move the epoch on and answer the new one: a reader that pins from
    /// here on pins at it or later.
    fn bump(&self) -> u64;
    /// The oldest epoch a reader is pinned at, `u64::MAX` when none is.
    fn oldest(&self) -> u64;
}

// ------------------------------------------------------------------ pager --

/// What a pager counts of its own work.
#[derive(Default)]
struct Counts {
    fills: AtomicU64,
    evicted_units: AtomicU64,
    evicted_bytes: AtomicU64,
    rescues: AtomicU64,
    waits: AtomicU64,
    passes: AtomicU64,
    stalls: AtomicU64,
    waited_us: AtomicU64,
    longest_wait_us: AtomicU64,
    breaths: AtomicU64,
    adopted_bytes: AtomicU64,
}

/// The longest a reader waits for a crowded cache (`Pager::breathe`).
const BREATH: Duration = Duration::from_millis(20);

/// A pager's counts, read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PagerCounts {
    /// The plaintext the cache may hold, bytes, and what it holds now.
    pub budget: u64,
    pub held: u64,
    /// The most it has held.
    pub peak: u64,
    /// Chunks decrypted.
    pub fills: u64,
    /// Plaintext taken whole from files just written, bytes: what the
    /// cache held without decrypting it.
    pub adopted_bytes: u64,
    /// Units given back, and the plaintext in them.
    pub evicted_units: u64,
    pub evicted_bytes: u64,
    /// Units marked and taken back by a reader before their pages went.
    pub rescues: u64,
    /// Reads that waited for a unit's pages to be gone.
    pub waits: u64,
    /// Passes of the evictor, and those that found nothing to give back
    /// with the cache over its budget.
    pub passes: u64,
    pub stalls: u64,
    /// What the evictor's passes waited for the readers before them, in
    /// microseconds: all of it, and the longest one wait. A read holds
    /// what it was lent until it ends or rests, so a long wait is one
    /// long read, and the cache holds what was read during it.
    pub waited_us: u64,
    pub longest_wait_us: u64,
    /// Waits of a reader for the cache to come back from well past its
    /// budget, each twenty milliseconds at most (`StoreCipher::cache_bytes`).
    pub breaths: u64,
}

/// A store's cache of plaintext: its budget, what it holds, and the thread
/// that gives back.
pub(crate) struct Pager {
    /// Where the cache is `crowded`: its budget and the slack past it,
    /// worked out once, since every read of the store asks.
    hard: u64,
    limit: AtomicU64,
    held: AtomicU64,
    peak: AtomicU64,
    files: Mutex<Vec<Weak<Paged>>>,
    /// The era the holds stand in (`Hold`), and the holds.
    era: AtomicU64,
    holds: Mutex<Vec<Arc<AtomicU64>>>,
    /// Where the evictor's pass stands: a file's place in `files` and a
    /// unit of it.
    hand: Mutex<(usize, usize)>,
    grace: RwLock<Option<Arc<dyn Grace>>>,
    poisoned: AtomicBool,
    poison: Mutex<Option<String>>,
    /// The evictor's wait: true while a wake is owed.
    wake: Mutex<bool>,
    woken: Condvar,
    stop: AtomicBool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    counts: Counts,
}

impl Pager {
    /// A cache that may hold `limit` bytes of plaintext, with its evictor
    /// started.
    pub(crate) fn new(limit: u64) -> Arc<Pager> {
        let unit = (unit_chunks() * CHUNK) as u64;
        let p = Arc::new(Pager {
            hard: limit.saturating_add((limit / 16).max(4 * unit)),
            limit: AtomicU64::new(limit),
            held: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            files: Mutex::new(Vec::new()),
            era: AtomicU64::new(1),
            holds: Mutex::new(Vec::new()),
            hand: Mutex::new((0, 0)),
            grace: RwLock::new(None),
            poisoned: AtomicBool::new(false),
            poison: Mutex::new(None),
            wake: Mutex::new(false),
            woken: Condvar::new(),
            stop: AtomicBool::new(false),
            thread: Mutex::new(None),
            counts: Counts::default(),
        });
        let weak = Arc::downgrade(&p);
        let t = std::thread::Builder::new()
            .name("supdb-pager".into())
            .spawn(move || Pager::run(weak));
        // A thread the system will not make is a cache that gives nothing
        // back, which is the store held in memory: said where it is asked.
        if let Ok(t) = t {
            *lock(&p.thread) = Some(t);
        }
        p
    }

    /// Hold this thread's reads of the store's files (`Hold`).
    pub(crate) fn hold(self: &Arc<Pager>) -> Hold {
        let at = Arc::new(AtomicU64::new(0));
        lock(&self.holds).push(at.clone());
        let h = Hold {
            pager: self.clone(),
            at,
            _thread: std::marker::PhantomData,
        };
        h.pin();
        debug_pinned(true);
        h
    }

    /// Whether every hold is at `era` or past it.
    fn holds_past(&self, era: u64) -> bool {
        lock(&self.holds).iter().all(|a| {
            let at = a.load(Ordering::SeqCst);
            at == 0 || at >= era
        })
    }

    /// The store's reader table, once the store has one.
    pub(crate) fn set_grace(&self, g: Option<Arc<dyn Grace>>) {
        *self.grace.write().unwrap_or_else(|e| e.into_inner()) = g;
    }

    pub(crate) fn budget(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    pub(crate) fn held(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }

    /// Whether `bytes` more would fit under the budget.
    pub(crate) fn room_for(&self, bytes: u64) -> bool {
        self.held().saturating_add(bytes) <= self.budget()
    }

    pub(crate) fn counts(&self) -> PagerCounts {
        let c = &self.counts;
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        PagerCounts {
            budget: self.budget(),
            held: self.held(),
            peak: r(&self.peak),
            fills: r(&c.fills),
            adopted_bytes: r(&c.adopted_bytes),
            evicted_units: r(&c.evicted_units),
            evicted_bytes: r(&c.evicted_bytes),
            rescues: r(&c.rescues),
            waits: r(&c.waits),
            passes: r(&c.passes),
            stalls: r(&c.stalls),
            waited_us: r(&c.waited_us),
            longest_wait_us: r(&c.longest_wait_us),
            breaths: r(&c.breaths),
        }
    }

    /// The evictor stopped and joined: for the store's close. What the
    /// cache holds stays lent until its files go.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.nudge();
        let t = lock(&self.thread).take();
        if let Some(t) = t {
            let _ = t.join();
        }
    }

    /// Remember a read that failed: the first is kept, and every
    /// operation of the store answers with it from here on (`check`).
    fn poison(&self, what: &str) {
        let mut p = lock(&self.poison);
        if p.is_none() {
            *p = Some(what.to_string());
        }
        self.poisoned.store(true, Ordering::SeqCst);
    }

    /// The failure a read met, if one has.
    #[inline]
    pub(crate) fn check(&self) -> Result<()> {
        if !self.poisoned.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.failed()
    }

    #[cold]
    fn failed(&self) -> Result<()> {
        let what = lock(&self.poison).clone().unwrap_or_default();
        Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "a file of the store under its cipher did not read, and nothing read from the \
                 store since is to be trusted: {what}"
            ),
        ))
    }

    fn add(&self, bytes: u64) {
        let now = self.held.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if now > self.peak.load(Ordering::Relaxed) {
            self.peak.fetch_max(now, Ordering::Relaxed);
        }
        if now > self.limit.load(Ordering::Relaxed) {
            self.nudge();
        }
    }

    fn sub(&self, bytes: u64) {
        self.held.fetch_sub(bytes, Ordering::Relaxed);
    }

    fn nudge(&self) {
        let mut owed = lock(&self.wake);
        if !*owed {
            *owed = true;
            self.woken.notify_one();
        }
    }

    fn over(&self) -> u64 {
        self.held().saturating_sub(self.budget())
    }

    /// Whether the cache holds well more than it may: past its budget by a
    /// sixteenth of it, or four units where that is more. Past here
    /// whoever reads through it waits a little for the evictor before
    /// reading on (`breathe`), and whoever reads in bulk for something the
    /// store can do without stops (`Reader::abandon_asked`,
    /// `Db::same_blocks`).
    #[inline]
    pub(crate) fn crowded(&self) -> bool {
        self.held() > self.hard
    }

    /// A reader's wait, holding nothing it was lent, for a cache that is
    /// `crowded` to come back: until it is not, or `BREATH` has passed.
    /// The wait has an end because the evictor may be waiting for a reader
    /// that will not rest -- one with a snapshot's worth of reading to do
    /// in a single call that nobody taught to rest, or a file with no
    /// envelope yet to be read again from -- and a store that stopped
    /// there would be worse than one that held more than it was told. What
    /// the bound buys is that a cache nobody can empty grows by what each
    /// reader reads between two waits, a breath apart, and not by what the
    /// cipher can decrypt.
    pub(crate) fn breathe(&self) {
        if !self.crowded() {
            return;
        }
        Counts::bump(&self.counts.breaths);
        let began = std::time::Instant::now();
        while self.crowded() && began.elapsed() < BREATH {
            self.nudge();
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    // -- the evictor ---------------------------------------------------

    fn run(me: Weak<Pager>) {
        loop {
            let Some(p) = me.upgrade() else {
                return;
            };
            if p.stop.load(Ordering::SeqCst) {
                return;
            }
            let idle = p.over() == 0 || !p.pass();
            if idle {
                // Nothing to give back, or nothing that may be: wait to be
                // told, and look again before long in case the telling was
                // missed or a file has its envelope now.
                let owed = lock(&p.wake);
                let mut owed = if *owed {
                    owed
                } else {
                    p.woken
                        .wait_timeout(owed, Duration::from_millis(20))
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                };
                *owed = false;
            }
        }
    }

    /// One pass: mark units until what they hold covers what the cache is
    /// over by and a little more, wait out the readers, and give back
    /// whichever were not taken back meanwhile. False when nothing could
    /// be marked.
    fn pass(&self) -> bool {
        Counts::bump(&self.counts.passes);
        // Under the budget by a margin, so that the next fill does not
        // start the next pass: a sixteenth of the budget, and a unit at
        // least. The first margin was eight units at least, eight
        // megabytes, which under a budget of two was every pass giving
        // back everything the store held.
        let budget = self.budget();
        let goal = self
            .over()
            .saturating_add((budget / 16).max((unit_chunks() * CHUNK) as u64));
        let marked = self.mark(goal);
        if marked.is_empty() {
            Counts::bump(&self.counts.stalls);
            return false;
        }
        // Every reader that could have been lent a byte of a marked unit
        // pinned before now. The epoch and the era move after the marks,
        // and the pins are read after they move.
        let era = self.era.fetch_add(1, Ordering::SeqCst) + 1;
        let grace = self.grace.read().unwrap_or_else(|e| e.into_inner()).clone();
        let tag = grace.as_ref().map(|g| g.bump());
        let mut nap = Duration::from_micros(20);
        let began = std::time::Instant::now();
        loop {
            let slots = match (&grace, tag) {
                (Some(g), Some(tag)) => g.oldest() >= tag,
                _ => true,
            };
            if slots && self.holds_past(era) {
                break;
            }
            if self.stop.load(Ordering::SeqCst) {
                // The store is closing under readers still pinned: nothing
                // is given back, and the marks are lifted.
                for (f, u) in &marked {
                    f.unmark(*u);
                }
                return true;
            }
            std::thread::sleep(nap);
            nap = (nap * 2).min(Duration::from_millis(2));
        }
        let waited = began.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.counts.waited_us.fetch_add(waited, Ordering::Relaxed);
        self.counts
            .longest_wait_us
            .fetch_max(waited, Ordering::Relaxed);
        for (f, u) in &marked {
            let freed = f.free(*u);
            if freed > 0 {
                Counts::bump(&self.counts.evicted_units);
                self.counts
                    .evicted_bytes
                    .fetch_add(freed, Ordering::Relaxed);
            }
        }
        true
    }

    /// Mark units for a pass, from where the hand stands and at most twice
    /// round: a unit with a chunk read since the hand last passed it has
    /// its chunks marked unread and is passed over, and a unit with none
    /// is marked, until the marked hold `goal` bytes.
    fn mark(&self, goal: u64) -> Vec<(Arc<Paged>, usize)> {
        let files: Vec<Arc<Paged>> = {
            let mut list = lock(&self.files);
            list.retain(|w| w.strong_count() > 0);
            list.iter().filter_map(Weak::upgrade).collect()
        };
        let mut out = Vec::new();
        if files.is_empty() {
            return out;
        }
        let total: usize = files.iter().map(|f| f.unit.len()).sum();
        let mut hand = lock(&self.hand);
        let (mut fi, mut ui) = *hand;
        if fi >= files.len() {
            (fi, ui) = (0, 0);
        }
        let mut bytes = 0u64;
        let mut seen = 0usize;
        while bytes < goal && seen < 2 * total {
            let f = &files[fi];
            if ui >= f.unit.len() {
                fi = (fi + 1) % files.len();
                ui = 0;
                continue;
            }
            seen += 1;
            if f.back.get().is_some() {
                let got = f.mark(ui);
                if got > 0 {
                    bytes += got;
                    out.push((f.clone(), ui));
                }
            }
            ui += 1;
        }
        *hand = (fi, ui);
        out
    }
}

impl Counts {
    fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------ files --

/// A file's envelope: where its ciphertext lies, and the cipher under the
/// file's key.
pub(crate) struct Back {
    map: memmap2::Mmap,
    hdr: Header,
    aad: [u8; Header::LEN],
    aead: Box<DynAead>,
}

impl Back {
    /// The envelope at `path`, under `aead`: its header checked as an open
    /// of an envelope checks it, and nothing decrypted.
    pub(crate) fn open(path: &std::path::Path, aead: Box<DynAead>) -> Result<Back> {
        let file = File::open(path)?;
        // SAFETY: the contract `MmapBytes::open` takes: an envelope is
        // written once, before the store names it, and not again.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        let hdr = crypt::checked_header(&crate::bytes::SliceBytes(&map))?;
        if hdr.chunk_shift() != CHUNK_SHIFT {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "an envelope of another chunk size than a store's files are written in",
            ));
        }
        // A read is of one chunk and then, as likely as not, of one far
        // from it: the kernel's read-ahead would be pages nobody asked for.
        let _ = map.advise(memmap2::Advice::Random);
        Ok(Back {
            aad: hdr.encode(),
            map,
            hdr,
            aead,
        })
    }

    pub(crate) fn plain_len(&self) -> u64 {
        self.hdr.plain_len()
    }
}

/// One file of a store under a cipher, read by the piece.
pub(crate) struct Paged {
    pager: Arc<Pager>,
    base: *mut u8,
    /// Bytes mapped: the plaintext's length rounded up to pages, zero for
    /// a file of no bytes, which has no mapping.
    mapped: usize,
    len: u64,
    unit_chunks: usize,
    state: Box<[AtomicU8]>,
    unit: Box<[AtomicU8]>,
    back: OnceLock<Back>,
    /// What the file is called, for an error.
    label: String,
    /// Chunks present, for the drop's count; kept by the fills and the
    /// frees.
    present: AtomicUsize,
    /// The memory file this is the one view of (`VIEWED`).
    #[cfg(debug_assertions)]
    viewed: Option<u64>,
}

/// The memory files a view was made of and not yet dropped, by inode,
/// under debug assertions: a memory file has one view (`Vault::settled`),
/// since a page given back through one mapping of it is gone from every
/// mapping of it, with nothing in the other's states to say so.
#[cfg(debug_assertions)]
static VIEWED: Mutex<std::collections::BTreeSet<u64>> =
    Mutex::new(std::collections::BTreeSet::new());

// SAFETY: the mapping is this value's for its life; a chunk of it is
// written by the one thread that moved the chunk to `BUSY` and read only
// once it is present, and its pages are given back only past every reader
// that could hold a borrow (the module's docs).
unsafe impl Send for Paged {}
unsafe impl Sync for Paged {}

/// A memory file: anonymous, the process's, gone with its last handle and
/// its last mapping.
#[cfg(target_os = "linux")]
pub(crate) fn memfile() -> Result<File> {
    use std::os::fd::FromRawFd;
    // SAFETY: a NUL-terminated name and a flag; the descriptor returned is
    // owned by nothing else.
    let fd = unsafe { libc::memfd_create(c"supdb".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(Error::last_os_error());
    }
    // SAFETY: a descriptor just made and owned here.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn memfile() -> Result<File> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "a store under a cipher keeps its plaintext in memory files, which this platform has not",
    ))
}

impl Paged {
    fn make(
        pager: &Arc<Pager>,
        mem: &File,
        len: u64,
        label: &str,
        first: u8,
        #[cfg(debug_assertions)] viewed: Option<u64>,
    ) -> Result<Arc<Paged>> {
        let size = usize::try_from(len)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "a file longer than an address"))?;
        let page = page_size();
        let mapped = size.div_ceil(page) * page;
        let base = if mapped == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            map_shared(mem, mapped)?
        };
        let chunks = size.div_ceil(CHUNK);
        let unit_chunks = unit_chunks();
        let p = Arc::new(Paged {
            pager: pager.clone(),
            base,
            mapped,
            len,
            unit_chunks,
            state: (0..chunks).map(|_| AtomicU8::new(first)).collect(),
            unit: (0..chunks.div_ceil(unit_chunks))
                .map(|_| AtomicU8::new(LIVE))
                .collect(),
            back: OnceLock::new(),
            label: label.to_string(),
            present: AtomicUsize::new(if first == EMPTY { 0 } else { chunks }),
            #[cfg(debug_assertions)]
            viewed,
        });
        if first != EMPTY {
            pager.counts.adopted_bytes.fetch_add(len, Ordering::Relaxed);
            pager.add(len);
        }
        lock(&pager.files).push(Arc::downgrade(&p));
        Ok(p)
    }

    /// A file just written, whole in its memory file `mem`: every chunk
    /// present, and nothing of it given back until it has its envelope
    /// (`back`).
    pub(crate) fn adopt(
        pager: &Arc<Pager>,
        mem: &File,
        len: u64,
        label: &str,
    ) -> Result<Arc<Paged>> {
        #[cfg(debug_assertions)]
        let viewed = {
            use std::os::unix::fs::MetadataExt;
            let ino = mem.metadata()?.ino();
            assert!(
                lock(&VIEWED).insert(ino),
                "{label}: a second view of one memory file: giving back a page of either \
                 takes it from both"
            );
            ino
        };
        let p = Paged::make(
            pager,
            mem,
            len,
            label,
            COLD,
            #[cfg(debug_assertions)]
            Some(viewed),
        )?;
        if p.mapped > 0 {
            // Every page of it mapped now: the pages are in memory already,
            // a memory file's are small, and a first touch through an empty
            // table maps sixteen of them a fault where a plain file's
            // megabyte folios map one a fault (`CLAUDE.md`, the scan pass
            // after a load).
            #[cfg(target_os = "linux")]
            // SAFETY: advice over the whole of a mapping this value owns.
            unsafe {
                libc::madvise(p.base.cast(), p.mapped, libc::MADV_POPULATE_READ);
            }
        }
        Ok(p)
    }

    /// A file known by its envelope alone: nothing of it present, and each
    /// chunk decrypted where it is first asked for.
    pub(crate) fn sparse(pager: &Arc<Pager>, back: Back, label: &str) -> Result<Arc<Paged>> {
        let len = back.plain_len();
        let mem = memfile()?;
        mem.set_len(len)?;
        let p = Paged::make(
            pager,
            &mem,
            len,
            label,
            EMPTY,
            #[cfg(debug_assertions)]
            None,
        )?;
        let _ = p.back.set(back);
        Ok(p)
    }

    /// The file's envelope, written: from here its chunks may be given
    /// back, since there is somewhere to read them again from.
    pub(crate) fn back(&self, back: Back) -> Result<()> {
        if back.plain_len() != self.len {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "{}: an envelope of {} bytes for a file of {}",
                    self.label,
                    back.plain_len(),
                    self.len
                ),
            ));
        }
        let _ = self.back.set(back);
        self.pager.nudge();
        Ok(())
    }

    pub(crate) fn backed(&self) -> bool {
        self.back.get().is_some()
    }

    fn chunk_len(&self, c: usize) -> usize {
        (self.len - (c * CHUNK) as u64).min(CHUNK as u64) as usize
    }

    /// Chunk `c` decrypted into its place. The caller moved it to `BUSY`.
    fn fill(&self, c: usize) -> Result<()> {
        let back = self.back.get().ok_or_else(|| {
            Error::new(
                ErrorKind::NotFound,
                format!(
                    "{}: a chunk is gone and the file has no envelope",
                    self.label
                ),
            )
        })?;
        let n = self.chunk_len(c);
        let at = back.hdr.stored_off(c as u64) as usize;
        let stored = back.map.get(at..at + n + TAG).ok_or_else(|| {
            Error::new(
                ErrorKind::UnexpectedEof,
                format!(
                    "{}: its envelope is shorter than its header says",
                    self.label
                ),
            )
        })?;
        // SAFETY: the chunk is `BUSY` by this thread's hand, so nobody
        // else writes it and nobody reads it; its bytes are inside the
        // mapping.
        let dst = unsafe { std::slice::from_raw_parts_mut(self.base.add(c * CHUNK), n) };
        dst.copy_from_slice(&stored[..n]);
        let tag: [u8; TAG] = stored[n..].try_into().expect("a tag's length");
        if !back
            .aead
            .decrypt(&back.hdr.nonce(c as u64), &back.aad, dst, &tag)
        {
            // Whatever the cipher left there is not plaintext anyone may
            // see.
            dst.fill(0);
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "{}: chunk {c} failed its check: the store's files were changed, or the key \
                     is not theirs",
                    self.label
                ),
            ));
        }
        Ok(())
    }

    /// `ensure` for a chunk that is not `HOT`.
    #[inline(never)]
    fn slow(&self, c: usize) -> bool {
        let state = &self.state[c];
        let unit = &self.unit[c / self.unit_chunks];
        loop {
            match state.load(Ordering::SeqCst) {
                HOT => return true,
                COLD => {
                    // Read since the hand last passed. Lost to the
                    // evictor's mark, the chunk is dying and the next
                    // turn says so.
                    if state
                        .compare_exchange(COLD, HOT, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        return true;
                    }
                }
                DYING => {
                    if !self.rescue(unit) {
                        self.gone(unit);
                        continue;
                    }
                    // The unit is lent from again, and this chunk with it.
                    let _ = state.compare_exchange(DYING, HOT, Ordering::SeqCst, Ordering::SeqCst);
                }
                EMPTY => {
                    if state
                        .compare_exchange(EMPTY, BUSY, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                    {
                        continue;
                    }
                    // A fill must not land in pages on their way out: the
                    // unit is taken back first, or waited for -- with the
                    // chunk put back before the wait, since the evictor
                    // waits in turn for no chunk of the unit to be `BUSY`
                    // before it lets the pages go.
                    if !self.rescue(unit) {
                        state.store(EMPTY, Ordering::SeqCst);
                        self.gone(unit);
                        continue;
                    }
                    match self.fill(c) {
                        Ok(()) => {
                            self.present.fetch_add(1, Ordering::Relaxed);
                            state.store(COLD, Ordering::SeqCst);
                            Counts::bump(&self.pager.counts.fills);
                            self.pager.add(self.chunk_len(c) as u64);
                            return true;
                        }
                        Err(e) => {
                            state.store(EMPTY, Ordering::SeqCst);
                            self.pager.poison(&e.to_string());
                            return false;
                        }
                    }
                }
                // Another thread is decrypting it: microseconds.
                _ => std::thread::yield_now(),
            }
        }
    }

    /// The unit lent from: true where it is `LIVE`, having been so or
    /// having been taken back from the evictor's mark here; false where
    /// its pages are going, and the caller waits for that (`gone`) holding
    /// no chunk of the unit and then looks at its chunk again.
    fn rescue(&self, unit: &AtomicU8) -> bool {
        match unit.compare_exchange(EVICTING, LIVE, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                Counts::bump(&self.pager.counts.rescues);
                true
            }
            Err(LIVE) => true,
            Err(_) => false,
        }
    }

    /// Wait for a unit whose pages are going to be lent from again.
    fn gone(&self, unit: &AtomicU8) {
        Counts::bump(&self.pager.counts.waits);
        while unit.load(Ordering::SeqCst) == FREEING {
            std::thread::yield_now();
        }
    }

    /// The evictor's look at unit `u`: its chunks read since the last look
    /// marked unread and nothing else, or, with none read, the unit marked
    /// and every chunk present in it dying. The bytes marked.
    fn mark(&self, u: usize) -> u64 {
        let lo = u * self.unit_chunks;
        let hi = (lo + self.unit_chunks).min(self.state.len());
        let (mut hot, mut present) = (false, false);
        for s in &self.state[lo..hi] {
            match s.load(Ordering::Relaxed) {
                HOT => {
                    hot = true;
                    let _ = s.compare_exchange(HOT, COLD, Ordering::SeqCst, Ordering::SeqCst);
                }
                COLD | DYING => present = true,
                _ => {}
            }
        }
        if hot || !present {
            return 0;
        }
        if self.unit[u]
            .compare_exchange(LIVE, EVICTING, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return 0;
        }
        let mut bytes = 0u64;
        for (i, s) in self.state[lo..hi].iter().enumerate() {
            loop {
                match s.load(Ordering::SeqCst) {
                    EMPTY => break,
                    DYING => {
                        bytes += self.chunk_len(lo + i) as u64;
                        break;
                    }
                    // A fill that began before the mark: it finishes, and
                    // its reader pinned before the mark too.
                    BUSY => std::thread::yield_now(),
                    was => {
                        if s.compare_exchange(was, DYING, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                        {
                            bytes += self.chunk_len(lo + i) as u64;
                            break;
                        }
                    }
                }
            }
        }
        bytes
    }

    /// A mark lifted with nothing given back.
    fn unmark(&self, u: usize) {
        let _ = self.unit[u].compare_exchange(EVICTING, LIVE, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Unit `u`'s pages given back, unless a reader took the unit back
    /// since it was marked. The bytes given back.
    fn free(&self, u: usize) -> u64 {
        if self.unit[u]
            .compare_exchange(EVICTING, FREEING, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return 0;
        }
        let lo = u * self.unit_chunks;
        let hi = (lo + self.unit_chunks).min(self.state.len());
        // A reader that moved a chunk to `BUSY` and then found the unit
        // going puts it back and waits; nothing fills here.
        for s in &self.state[lo..hi] {
            while s.load(Ordering::SeqCst) == BUSY {
                std::thread::yield_now();
            }
        }
        let a = lo * CHUNK;
        let b = (hi * CHUNK).min(self.mapped).div_ceil(page_size()) * page_size();
        let b = b.min(self.mapped);
        if a < b {
            // SAFETY: whole pages of a shared mapping of a memory file this
            // value owns; every reader that could hold a borrow into them
            // has been waited out, and none is lent from them until the
            // unit is `LIVE` again. The pages and the file's store behind
            // them go, and read as zeroes until filled again.
            unsafe { give_back(self.base.add(a), b - a) };
        }
        let mut bytes = 0u64;
        let mut chunks = 0usize;
        for (i, s) in self.state[lo..hi].iter().enumerate() {
            // Dying, as the mark left it, or revived by a reader that took
            // the unit back from an earlier mark, was waited out by this
            // one, and has gone: present either way, and gone now.
            if matches!(s.swap(EMPTY, Ordering::SeqCst), DYING | COLD | HOT) {
                bytes += self.chunk_len(lo + i) as u64;
                chunks += 1;
            }
        }
        self.present.fetch_sub(chunks, Ordering::Relaxed);
        self.pager.sub(bytes);
        self.unit[u].store(LIVE, Ordering::SeqCst);
        bytes
    }

    /// Chunks present now.
    #[cfg(test)]
    pub(crate) fn present_chunks(&self) -> usize {
        self.present.load(Ordering::Relaxed)
    }

    /// The plaintext this file holds present, bytes: its chunks' count,
    /// the last being short.
    fn present_bytes(&self) -> u64 {
        let mut bytes = 0u64;
        for (c, s) in self.state.iter().enumerate() {
            if matches!(s.load(Ordering::Relaxed), COLD | HOT | DYING) {
                bytes += self.chunk_len(c) as u64;
            }
        }
        bytes
    }
}

impl Drop for Paged {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        if let Some(ino) = self.viewed {
            lock(&VIEWED).remove(&ino);
        }
        self.pager.sub(self.present_bytes());
        if self.mapped > 0 {
            // SAFETY: the mapping made in `make`, and nothing borrows from
            // a file whose last holder is going.
            unsafe { libc::munmap(self.base.cast(), self.mapped) };
        }
    }
}

impl Paging for Paged {
    fn base(&self) -> *const u8 {
        self.base
    }

    fn size(&self) -> u64 {
        self.len
    }

    #[inline]
    fn ensure(&self, off: u64, len: usize) -> bool {
        if len == 0 {
            return true;
        }
        debug_assert!(
            PINS.with(|p| p.get()) > 0,
            "{}: read with nothing pinned: the evictor would not wait for this thread",
            self.label
        );
        // A chunk's length is a constant, so the divide is a multiply the
        // compiler writes. The loads are sequentially consistent, against
        // the pin before them and the evictor's mark: a chunk seen `HOT`
        // here was not marked when this thread pinned.
        let a = (off / CHUNK as u64) as usize;
        // Most asks are of a few bytes inside one chunk: one divide, and
        // the second only for an ask that crosses.
        if (off - (a * CHUNK) as u64) + len as u64 <= CHUNK as u64 {
            return match self.state.get(a) {
                Some(s) => s.load(Ordering::SeqCst) == HOT || self.slow(a),
                None => false,
            };
        }
        let b = ((off + len as u64 - 1) / CHUNK as u64) as usize;
        if b >= self.state.len() {
            return false;
        }
        (a..=b).all(|c| self.state[c].load(Ordering::SeqCst) == HOT || self.slow(c))
    }

    fn piece_end(&self, off: u64) -> u64 {
        ((off / CHUNK as u64 + 1) * CHUNK as u64).min(self.len)
    }

    #[inline]
    fn piece_from(&self, off: u64) -> usize {
        if off >= self.len {
            return 0;
        }
        let c = (off / CHUNK as u64) as usize;
        let ready = match self.state.get(c) {
            Some(s) => s.load(Ordering::SeqCst) == HOT || self.slow(c),
            None => false,
        };
        if !ready {
            return 0;
        }
        (((c as u64 + 1) * CHUNK as u64).min(self.len) - off) as usize
    }

    /// The kernel told how the envelope is read, which is where a chunk
    /// that is not present comes from: far apart, or in order, when the
    /// kernel's own read-ahead brings the next chunks' ciphertext in
    /// beside the one asked for.
    fn advise(&self, random: bool) {
        if let Some(b) = self.back.get() {
            let _ = b.map.advise(if random {
                memmap2::Advice::Random
            } else {
                memmap2::Advice::Normal
            });
        }
    }
}

/// `len` bytes of `mem` mapped shared and writable.
fn map_shared(mem: &File, len: usize) -> Result<*mut u8> {
    use std::os::fd::AsRawFd;
    // SAFETY: a new mapping of a descriptor this call borrows, at an
    // address the kernel chooses.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            mem.as_raw_fd(),
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return Err(Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    // SAFETY: advice over the mapping just made. A huge page under a file
    // of which one chunk is present would be two megabytes held for four
    // kilobytes wanted, and the count of what the cache holds would be of
    // nothing.
    unsafe {
        libc::madvise(p, len, libc::MADV_NOHUGEPAGE);
    }
    Ok(p.cast())
}

/// The pages at `at` and their store given back.
///
/// # Safety
/// `at..at + len` is whole pages of a shared, writable mapping of a memory
/// file, and nothing reads them until they are filled again.
unsafe fn give_back(at: *mut u8, len: usize) {
    #[cfg(target_os = "linux")]
    // SAFETY: the caller's.
    unsafe {
        libc::madvise(at.cast(), len, libc::MADV_REMOVE);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (at, len);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::crypt::{EncryptOptions, NONCE};
    use std::io::Write;

    /// A cipher for the tests: a keystream and a tag that are functions of
    /// the key, the nonce, the associated data and the bytes, and nothing
    /// anybody should use.
    struct Toy(u64);

    fn mix(mut h: u64, bytes: &[u8]) -> u64 {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    impl Toy {
        fn stream(&self, nonce: &[u8; NONCE], buf: &mut [u8]) {
            let mut s = mix(self.0 ^ 0x9e37_79b9_7f4a_7c15, nonce);
            for b in buf {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *b ^= s as u8;
            }
        }
        fn tag(&self, nonce: &[u8; NONCE], aad: &[u8], ct: &[u8]) -> [u8; TAG] {
            let a = mix(mix(mix(self.0, nonce), aad), ct);
            let b = mix(a ^ 0xa5a5, ct);
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
            if &self.tag(nonce, aad, buf) != tag {
                return false;
            }
            self.stream(nonce, buf);
            true
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("supdb-next-pager-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plain(n: usize, salt: u64) -> Vec<u8> {
        let mut s = 0x1234_5678_9abc_def0u64 ^ salt;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s as u8
            })
            .collect()
    }

    /// An envelope of `bytes` at `path` under key `key`.
    fn envelope(path: &std::path::Path, bytes: &[u8], key: u64) {
        let mut out = File::create(path).unwrap();
        crypt::encrypt(
            &crate::bytes::SliceBytes(bytes),
            &mut out,
            &Toy(key),
            &EncryptOptions::default(),
        )
        .unwrap();
        out.flush().unwrap();
    }

    fn sparse(pager: &Arc<Pager>, path: &std::path::Path, key: u64) -> Arc<Paged> {
        let back = Back::open(path, Box::new(Toy(key))).unwrap();
        Paged::sparse(pager, back, "test").unwrap()
    }

    fn read(p: &Paged, off: u64, len: usize) -> Option<Vec<u8>> {
        p.ensure(off, len).then(|| {
            // SAFETY: made present by `ensure`, under the caller's hold.
            unsafe { std::slice::from_raw_parts(p.base.add(off as usize), len) }.to_vec()
        })
    }

    #[test]
    fn a_unit_is_whole_pages() {
        let bytes = unit_chunks() * CHUNK;
        assert_eq!(bytes % page_size(), 0);
        assert_eq!(CHUNK, 4080);
    }

    /// Every range reads as the plaintext does, a chunk is decrypted once,
    /// and nothing is present that was not asked for.
    #[test]
    fn a_range_asked_for_is_the_plaintext_and_nothing_else_is_decrypted() {
        let dir = scratch("ranges");
        let bytes = plain(3 * 1024 * 1024 + 123, 1);
        envelope(&dir.join("e"), &bytes, 7);
        let pager = Pager::new(u64::MAX);
        let p = sparse(&pager, &dir.join("e"), 7);
        let _h = pager.hold();
        assert_eq!(p.present_chunks(), 0, "an open decrypts nothing");
        for (off, len) in [
            (0u64, 1usize),
            (4079, 2),
            (4080, 4080),
            (1_000_000, 9000),
            (bytes.len() as u64 - 5, 5),
            (123_456, 0),
        ] {
            let got = read(&p, off, len).unwrap();
            assert_eq!(got, &bytes[off as usize..off as usize + len], "{off}+{len}");
        }
        // The first two chunks, three at the megabyte, and the last.
        assert_eq!(p.present_chunks(), 6);
        assert_eq!(pager.counts().fills, 6);
        assert_eq!(read(&p, 4079, 2).unwrap(), &bytes[4079..4081]);
        assert_eq!(
            pager.counts().fills,
            6,
            "a chunk present is not decrypted again"
        );
        assert_eq!(pager.held(), 5 * 4080 + (bytes.len() % 4080) as u64);
        drop(p);
        assert_eq!(pager.held(), 0, "a file gone gives back what it held");
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cache over its budget gives units back and reads them again as
    /// the same bytes; what it holds comes back under the budget.
    #[test]
    fn a_cache_over_its_budget_gives_back_and_reads_again() {
        let dir = scratch("evict");
        let bytes = plain(24 * 1024 * 1024, 2);
        envelope(&dir.join("e"), &bytes, 9);
        let unit = (unit_chunks() * CHUNK) as u64;
        let pager = Pager::new(4 * unit);
        let p = sparse(&pager, &dir.join("e"), 9);
        let step = 64 * 1024;
        for round in 0..3 {
            let mut off = 0usize;
            while off < bytes.len() {
                let _h = pager.hold();
                let len = step.min(bytes.len() - off);
                let got = read(&p, off as u64, len).unwrap();
                assert_eq!(got, &bytes[off..off + len], "round {round} at {off}");
                off += len;
            }
        }
        let c = pager.counts();
        assert!(c.evicted_units > 0, "nothing was given back: {c:?}");
        // Settled: nobody holds, and the evictor has had its turn.
        let t = std::time::Instant::now();
        while pager.held() > pager.budget() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pager.held() <= pager.budget(), "{:?}", pager.counts());
        assert!(
            c.peak < pager.budget() + 16 * unit,
            "the cache ran far past its budget: {c:?}"
        );
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A thread holding its lane keeps what it was lent: the evictor marks
    /// and waits, and gives nothing back until the hold is gone or rested.
    #[test]
    fn nothing_is_given_back_under_a_hold() {
        let dir = scratch("hold");
        let bytes = plain(8 * 1024 * 1024, 3);
        envelope(&dir.join("e"), &bytes, 11);
        let unit = (unit_chunks() * CHUNK) as u64;
        let pager = Pager::new(unit);
        let p = sparse(&pager, &dir.join("e"), 11);
        let h = pager.hold();
        // Far past the budget, under one hold.
        let got = read(&p, 0, bytes.len()).unwrap();
        assert_eq!(got, bytes);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            pager.counts().evicted_units,
            0,
            "a unit went while its reader held"
        );
        // SAFETY: lent above and held since.
        let lent = unsafe { std::slice::from_raw_parts(p.base, bytes.len()) };
        assert_eq!(lent, &bytes[..], "the bytes lent are still the bytes");
        drop(h);
        let t = std::time::Instant::now();
        while pager.held() > pager.budget() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pager.held() <= pager.budget(), "{:?}", pager.counts());
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rest lets the evictor past a hold, and a range asked for again
    /// after it is the same bytes at the same address.
    #[test]
    fn a_rest_gives_way_and_a_range_asked_again_is_the_same_bytes() {
        let dir = scratch("rest");
        let bytes = plain(16 * 1024 * 1024, 4);
        envelope(&dir.join("e"), &bytes, 13);
        let unit = (unit_chunks() * CHUNK) as u64;
        let pager = Pager::new(2 * unit);
        let p = sparse(&pager, &dir.join("e"), 13);
        let h = pager.hold();
        assert!(p.ensure(0, 100));
        // SAFETY: lent by the line above.
        let first = unsafe { std::slice::from_raw_parts(p.base, 100) };
        assert_eq!(first, &bytes[..100]);
        let mut off = 0usize;
        while off < bytes.len() {
            let len = (256 * 1024).min(bytes.len() - off);
            assert_eq!(read(&p, off as u64, len).unwrap(), &bytes[off..off + len]);
            off += len;
            h.rest();
        }
        let t = std::time::Instant::now();
        while pager.counts().evicted_units == 0 && t.elapsed() < Duration::from_secs(10) {
            h.rest();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pager.counts().evicted_units > 0, "{:?}", pager.counts());
        h.rest();
        assert!(p.ensure(0, 100));
        assert_eq!(
            first,
            &bytes[..100],
            "asked for again, the borrow reads as it did"
        );
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Threads reading at random under a budget a tenth of the file, each
    /// holding for a read: every read is the plaintext, whatever the
    /// evictor did between.
    #[test]
    fn threads_read_the_plaintext_while_the_cache_turns_over() {
        let dir = scratch("threads");
        let bytes = Arc::new(plain(40 * 1024 * 1024, 5));
        envelope(&dir.join("e"), &bytes, 17);
        let unit = (unit_chunks() * CHUNK) as u64;
        let pager = Pager::new(4 * unit);
        let p = sparse(&pager, &dir.join("e"), 17);
        let threads: Vec<_> = (0..4u64)
            .map(|t| {
                let (p, bytes, pager) = (p.clone(), bytes.clone(), pager.clone());
                std::thread::spawn(move || {
                    let mut s = 0x9e37_79b9u64 ^ (t << 32) | 1;
                    for _ in 0..20_000 {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        let off = (s % (bytes.len() as u64 - 9000)) as usize;
                        let len = 1 + (s >> 40) as usize % 9000;
                        let _h = pager.hold();
                        let got = read(&p, off as u64, len).expect("a read");
                        assert_eq!(got, &bytes[off..off + len], "at {off}+{len}");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let c = pager.counts();
        assert!(c.evicted_units > 0, "{c:?}");
        assert!(pager.check().is_ok());
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A chunk changed on the device fails its read and is remembered: the
    /// read answers nothing, zeroes stand where the plaintext would, and
    /// the store's every operation answers with the failure after.
    #[test]
    fn a_changed_chunk_fails_its_read_and_poisons_the_store() {
        let dir = scratch("poison");
        let bytes = plain(64 * 1024, 6);
        envelope(&dir.join("e"), &bytes, 19);
        let mut raw = std::fs::read(dir.join("e")).unwrap();
        // In the third chunk's ciphertext.
        raw[crypt::DATA_START as usize + 2 * 4096 + 100] ^= 1;
        std::fs::write(dir.join("e"), &raw).unwrap();
        let pager = Pager::new(u64::MAX);
        let p = sparse(&pager, &dir.join("e"), 19);
        let _h = pager.hold();
        assert_eq!(read(&p, 0, 4080).unwrap(), &bytes[..4080]);
        assert!(pager.check().is_ok());
        assert!(
            read(&p, 2 * 4080 + 5, 10).is_none(),
            "the changed chunk read"
        );
        let e = pager.check().unwrap_err().to_string();
        assert!(e.contains("chunk 2 failed its check"), "{e}");
        // SAFETY: inside the mapping; the chunk is not lent, and this
        // looks at what was left where it would be.
        let left = unsafe { std::slice::from_raw_parts(p.base.add(2 * 4080), 4080) };
        assert!(
            left.iter().all(|&b| b == 0),
            "what the cipher left is not zeroes"
        );
        assert!(
            read(&p, 4080, 10).is_some(),
            "the chunks beside it still read"
        );
        assert!(
            pager.check().is_err(),
            "a failure once is a failure from then on"
        );
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file adopted from its memory file is whole from the start, holds
    /// its bytes against the budget, and is given back only once it has an
    /// envelope to be read again from.
    #[test]
    fn a_file_adopted_is_whole_and_stays_until_it_has_its_envelope() {
        let dir = scratch("adopt");
        let bytes = plain(6 * 1024 * 1024, 8);
        let unit = (unit_chunks() * CHUNK) as u64;
        let pager = Pager::new(unit);
        let mut mem = memfile().unwrap();
        mem.write_all(&bytes).unwrap();
        let p = Paged::adopt(&pager, &mem, bytes.len() as u64, "adopted").unwrap();
        drop(mem);
        assert_eq!(pager.held(), bytes.len() as u64);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            pager.counts().evicted_units,
            0,
            "given back with no envelope"
        );
        {
            let _h = pager.hold();
            assert_eq!(read(&p, 0, bytes.len()).unwrap(), bytes);
        }
        envelope(&dir.join("e"), &bytes, 23);
        p.back(Back::open(&dir.join("e"), Box::new(Toy(23))).unwrap())
            .unwrap();
        let t = std::time::Instant::now();
        while pager.held() > pager.budget() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pager.held() <= pager.budget(), "{:?}", pager.counts());
        {
            let _h = pager.hold();
            assert_eq!(
                read(&p, 0, bytes.len()).unwrap(),
                bytes,
                "read again from the envelope"
            );
        }
        let short = plain(100, 9);
        envelope(&dir.join("s"), &short, 23);
        assert!(
            p.back(Back::open(&dir.join("s"), Box::new(Toy(23))).unwrap())
                .is_err(),
            "an envelope of another length was taken"
        );
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
