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
//! A merge may read its inputs beside the cache instead (`Stream`,
//! `StoreCipher::merges_beside_cache`): each chunk decrypted into a buffer of
//! the merge's own, which the cache neither holds nor gives back, and
//! which the merge's rests write again. There a range asked for again
//! after a rest is the same bytes at another address, and what is read
//! after a rest is only what was asked for after it, which is how every
//! walk of a merge reads (`db::merge_ranks`).
//!
//! ## Who waits
//!
//! The evictor waits for readers, so a cache can run past its budget for
//! as long as some reader holds what it was lent, and two lines past the
//! budget say who waits for it in turn. Past the first (`crowded`, a
//! sixteenth of the budget over it) the store's own work does: a merge and
//! a builder of the block cache at their rests, each for a breath at most,
//! and a builder stops if the cache is crowded still. Past the second
//! (`swamped`, half the budget over it) a caller's read does, before it
//! begins. Both are drawn against what the cache could give back: a file
//! being written is in memory whole until it has its envelope, counted in
//! what the cache holds, and no wait is for it.
//!
//! A reader that waits for another thread, or for the device, says for the
//! length of the wait that it holds nothing (`db::Reader::unlent`): pinned
//! and silent, it would hold the evictor for a wait in which it reads
//! nothing.
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

// The door (scripts/door/clippy.toml, tests/data/door.txt): the cache's memory files, their
// mappings and their advice.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

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
    /// Whether the hold has stood for nothing since it was last asked
    /// (`idle`, `took_idle`).
    idled: Cell<bool>,
    /// How many times the holder has rested or idled: points at which it
    /// holds nothing it was lent, after which a view read in order under
    /// this hold writes its buffer again (`Stream`).
    rests: Arc<AtomicU64>,
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
        self.rests.fetch_add(1, Ordering::Relaxed);
        if self.pager.crowded() {
            self.at.store(0, Ordering::SeqCst);
            self.pager.breathe();
        }
        self.pin();
    }

    /// `wait` run with the hold standing for nothing, as it does in a
    /// rest's breath: for a step of the holder's that reads no file of
    /// the store and takes long -- a merge writing a finished output's
    /// envelope, fifty megabytes sealed, written and synced on the
    /// merge's own thread. With the hold left at its era the evictor
    /// waited the write out, a tenth of a second and more at every output
    /// of every merge, while the readers beside it filled the cache past
    /// its budget. Under debug assertions a read of a file inside what was
    /// called a wait panics (`debug_pinned`), as it does inside
    /// `db::Reader::unlent`. What the holder was lent before it is asked
    /// for again after, which `took_idle` is for.
    pub(crate) fn idle<T>(&self, wait: impl FnOnce() -> T) -> T {
        self.rests.fetch_add(1, Ordering::Relaxed);
        self.at.store(0, Ordering::SeqCst);
        debug_pinned(false);
        let r = wait();
        self.pin();
        debug_pinned(true);
        self.idled.set(true);
        r
    }

    /// Whether the hold has idled since this was last asked, for a walk
    /// that keeps what it was lent from one step to the next: an idle is
    /// a rest, after which it asks again.
    pub(crate) fn took_idle(&self) -> bool {
        self.idled.replace(false)
    }

    /// Whether the holder, a merge, reads its inputs beside the cache
    /// (`StoreCipher::merges_beside_cache`).
    pub(crate) fn beside(&self) -> bool {
        self.pager.beside.load(Ordering::Relaxed)
    }

    /// A view of the file whose envelope is `back`, read in order under
    /// this hold and kept out of the store's cache (`Stream`), taking the
    /// chunks the cache's view `cached` holds from it.
    pub(crate) fn stream(
        &self,
        back: Back,
        cached: Option<Arc<Paged>>,
        label: &str,
    ) -> Result<Arc<Stream>> {
        Stream::new(&self.pager, back, cached, self.rests.clone(), label)
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
    together: AtomicU64,
    rooms: AtomicU64,
    gave_way: AtomicU64,
    spared: AtomicU64,
    streamed: AtomicU64,
    dropped_units: AtomicU64,
    dropped_bytes: AtomicU64,
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
    /// Asks of several chunks, two or more of them not here, whose
    /// ciphertext was asked of the kernel in one piece before the first
    /// was decrypted (`StoreCipher::chunks_apart` off).
    pub together: u64,
    /// Readings of the room by a cache whose budget follows it, and the
    /// times the budget was lowered between two readings for memory the
    /// process had claimed since (`StoreCipher::cache_bytes` unset).
    pub rooms: u64,
    pub gave_way: u64,
    /// What the last reading of the room kept back from the budget for
    /// the cache's own overshoot, bytes.
    pub spared: u64,
    /// Chunks decrypted by views read in order beside the cache (a merge's
    /// inputs, `Stream`), which the cache neither holds nor counts.
    pub streamed: u64,
    /// Of `evicted_units` and `evicted_bytes`, the units a drop of their
    /// file's pages gave back (`Paged::forget`), and their plaintext.
    pub dropped_units: u64,
    pub dropped_bytes: u64,
}

/// What a cache whose budget follows the room asks of the platform
/// (`Pager::following`).
///
/// Plaintext in a memory file is memory the kernel cannot take back: a
/// group at its limit gives up its page cache to a process whose own
/// memory grows, and has nothing of this to give, so the process is
/// killed. A budget read once is therefore either small, half of the room
/// at the open as it was, or a store killed in its first burst of writes;
/// one that follows the room holds what there is and gives way as the
/// process grows.
pub(crate) trait Room: Send + Sync {
    /// The bytes this cache's plaintext may hold now, `held` being what it
    /// holds: the limit in effect less the process's own memory, less
    /// what it has mapped and not yet touched, less a margin, and where
    /// several stores of the process follow the room, this one's share.
    /// `None` where the platform does not say, which leaves the budget as
    /// it stands.
    fn read(&self, held: u64) -> Option<u64>;
    /// The least the cache is given, whatever the room: under it a store
    /// reads at the rate of its misses however little there is.
    fn floor(&self) -> u64;
    /// A count that rises with memory the process is about to take: the
    /// bytes of large buffers it has mapped. Read at every turn of the
    /// evictor, which is a load, where `read` is files; a rise since the
    /// last reading comes off the budget at once.
    fn claimed(&self) -> u64;
}

/// How often a following cache reads the room while it is within a line
/// of its budget, and how often while it is not: a reading is half a
/// dozen small files, a cache far under its budget has nothing to give
/// way with, and a store left open and idle should not be read fifty
/// times a second for it.
const ROOM_EVERY: Duration = Duration::from_millis(20);
const ROOM_SELDOM: Duration = Duration::from_millis(200);

/// How long a following cache's evictor waits with nothing to do while
/// the cache is near its budget, where `claimed` is what it is waiting to
/// see move: a seal's copy of its table is a hundred megabytes mapped and
/// filled in tens of milliseconds.
const ROOM_WATCH: Duration = Duration::from_millis(5);

/// How far past its budget a following cache may hold before the store's
/// own work waits, and before a caller's read does, at most: the lines of
/// a cache whose budget is the room are drawn inside the margin the room
/// leaves, where a fixed budget's are shares of the budget.
const ROOM_HARD: u64 = 16 << 20;
const ROOM_SWAMP: u64 = 32 << 20;

struct Follow {
    room: Box<dyn Room>,
    at: Mutex<Followed>,
}

/// Where a following cache stands with its room.
#[derive(Default)]
struct Followed {
    /// When the room was last read, and what was claimed then.
    read: Option<std::time::Instant>,
    claimed: u64,
    /// The most the cache has held past its budget, over the span in
    /// progress and the one before it (`ROOM_OVER_SPAN`): what comes off
    /// the room before it is the budget.
    over: (u64, u64),
    over_since: Option<std::time::Instant>,
    /// What the cache holds past its budget because the budget fell, not
    /// yet given back: no overshoot of the cache's, so it is taken out of
    /// what `over` remembers. Up by each fall of the budget, down by what
    /// the evictor has given back since (`evicted`), nothing once the
    /// cache is under its budget.
    debt: u64,
    evicted: u64,
}

/// How long an overshoot is remembered, once to twice this: a cache that
/// was past its budget a moment ago, by a file being written that it could
/// not give back or a reader it waited for, will be again. A tenth of a
/// second under the unit tests, which wait for it to be forgotten.
const ROOM_OVER_SPAN: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 2000 });

/// A store's cache of plaintext: its budget, what it holds, and the thread
/// that gives back.
pub(crate) struct Pager {
    /// Where the cache is `crowded`: its budget and the slack past it,
    /// worked out where the budget is set, since every read of the store
    /// asks.
    hard: AtomicU64,
    /// Where it is `swamped`.
    swamp: AtomicU64,
    /// The room the budget follows, where it follows one.
    follow: Option<Follow>,
    limit: AtomicU64,
    held: AtomicU64,
    /// Of `held`, what is in files with no envelope yet: a file just
    /// written, taken whole, of which nothing can be given back until
    /// there is somewhere to read it again from.
    unbacked: AtomicU64,
    peak: AtomicU64,
    /// `StoreCipher::chunks_apart`.
    apart: AtomicBool,
    /// `StoreCipher::merges_beside_cache`.
    beside: AtomicBool,
    /// `StoreCipher::drops_keep_plaintext`.
    drops_keep: AtomicBool,
    /// Units a drop has asked to be given back since the evictor last
    /// looked (`Paged::forget`), each flagged on its file.
    dropped: AtomicUsize,
    /// What the evictor gave back of files since dropped, by the first
    /// field of each one's name, bytes: the cache's trace adds it to the
    /// files still viewed (`Paged::given_bytes`).
    given_gone: Mutex<Vec<(String, u64)>>,
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
    /// Whether the evictor has been told since it last looked (`nudge`).
    told: AtomicBool,
    woken: Condvar,
    stop: AtomicBool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    counts: Counts,
}

impl Pager {
    /// A cache that may hold `limit` bytes of plaintext, with its evictor
    /// started.
    pub(crate) fn new(limit: u64) -> Arc<Pager> {
        Pager::make(limit, None)
    }

    /// A cache whose budget is what `room` says, read again as the process
    /// runs (`Room`), with its evictor started. A gibibyte until the
    /// platform first says.
    pub(crate) fn following(room: Box<dyn Room>) -> Arc<Pager> {
        let p = Pager::make(
            1 << 30,
            Some(Follow {
                room,
                at: Mutex::new(Followed::default()),
            }),
        );
        p.follow_room();
        p
    }

    fn make(limit: u64, follow: Option<Follow>) -> Arc<Pager> {
        let p = Arc::new(Pager {
            hard: AtomicU64::new(0),
            swamp: AtomicU64::new(0),
            follow,
            told: AtomicBool::new(false),
            limit: AtomicU64::new(limit),
            held: AtomicU64::new(0),
            unbacked: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            apart: AtomicBool::new(false),
            beside: AtomicBool::new(false),
            drops_keep: AtomicBool::new(false),
            dropped: AtomicUsize::new(0),
            given_gone: Mutex::new(Vec::new()),
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
        p.set_budget(limit);
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

    /// The budget and the two lines past it. A fixed budget's lines are
    /// shares of it, a sixteenth and a half, and four and six units at
    /// least; a following cache's are those held inside the margin its
    /// room leaves (`ROOM_HARD`, `ROOM_SWAMP`), since half of a budget
    /// that is the room is past the limit the room was read from.
    fn set_budget(&self, limit: u64) {
        let unit = (unit_chunks() * CHUNK) as u64;
        let (hard, swamp) = ((limit / 16).max(4 * unit), (limit / 2).max(6 * unit));
        let (hard, swamp) = match self.follow {
            Some(_) => (
                hard.min(ROOM_HARD.max(4 * unit)),
                swamp.min(ROOM_SWAMP.max(6 * unit)),
            ),
            None => (hard, swamp),
        };
        self.limit.store(limit, Ordering::Relaxed);
        self.hard
            .store(limit.saturating_add(hard), Ordering::Relaxed);
        self.swamp
            .store(limit.saturating_add(swamp), Ordering::Relaxed);
    }

    /// The budget brought to the room, for a cache that follows one: read
    /// whole where the last reading is `ROOM_EVERY` old, and the room's
    /// floor at least; and between two readings lowered at once by
    /// whatever the process has claimed since the last (`Room::claimed`),
    /// which a reading does not see until the memory is touched. What the
    /// cache has lately held past its budget comes off the room first: a
    /// budget is where the cache gives back and not a line it holds under,
    /// a file being written is held whole and a reader is waited for, and
    /// a budget that is all of the room leaves that overshoot nowhere to
    /// be but past the limit. The evictor's, at every turn of it.
    fn follow_room(&self) {
        let Some(f) = &self.follow else {
            return;
        };
        let mut at = lock(&f.at);
        let now = std::time::Instant::now();
        let evicted = self.counts.evicted_bytes.load(Ordering::Relaxed);
        at.debt = at.debt.saturating_sub(evicted - at.evicted);
        at.evicted = evicted;
        if self.over() == 0 {
            at.debt = 0;
        }
        let over = self.over().saturating_sub(at.debt);
        if at
            .over_since
            .is_none_or(|t| now.duration_since(t) >= ROOM_OVER_SPAN)
        {
            at.over = (0, at.over.0);
            at.over_since = Some(now);
        }
        at.over.0 = at.over.0.max(over);
        let spare = at.over.0.max(at.over.1);
        let claimed = f.room.claimed();
        let floor = f.room.floor();
        let every = if self.near_budget() {
            ROOM_EVERY
        } else {
            ROOM_SELDOM
        };
        if at.read.is_none_or(|t| now.duration_since(t) >= every) {
            if let Some(room) = f.room.read(self.held()) {
                at.read = Some(now);
                at.claimed = claimed;
                Counts::bump(&self.counts.rooms);
                self.counts.spared.store(spare, Ordering::Relaxed);
                let (was, now) = (self.budget(), room.saturating_sub(spare).max(floor));
                at.debt += was.saturating_sub(now);
                self.set_budget(now);
                return;
            }
            // The platform said nothing: the budget stands, less what has
            // been claimed since it last spoke.
        }
        if claimed > at.claimed {
            let more = claimed - at.claimed;
            at.claimed = claimed;
            Counts::bump(&self.counts.gave_way);
            let (was, now) = (self.budget(), self.budget().saturating_sub(more).max(floor));
            at.debt += was.saturating_sub(now);
            self.set_budget(now);
        }
    }

    /// Whether the cache is within a line of its budget (`ROOM_SWAMP`).
    fn near_budget(&self) -> bool {
        self.held().saturating_add(ROOM_SWAMP) >= self.budget()
    }

    /// How long the evictor waits with nothing to give back: a following
    /// cache within a line of its budget watches what the process claims.
    fn idle_wait(&self) -> Duration {
        match self.follow {
            Some(_) if self.near_budget() => ROOM_WATCH,
            _ => Duration::from_millis(20),
        }
    }

    /// Hold this thread's reads of the store's files (`Hold`).
    pub(crate) fn hold(self: &Arc<Pager>) -> Hold {
        let at = Arc::new(AtomicU64::new(0));
        lock(&self.holds).push(at.clone());
        let h = Hold {
            pager: self.clone(),
            at,
            idled: Cell::new(false),
            rests: Arc::new(AtomicU64::new(0)),
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

    /// Of `held`, the bytes of files with no envelope yet, which nothing
    /// can give back (`could_give`).
    pub(crate) fn unbacked(&self) -> u64 {
        self.unbacked.load(Ordering::Relaxed)
    }

    /// Whether `bytes` more would fit under the budget.
    pub(crate) fn room_for(&self, bytes: u64) -> bool {
        self.held().saturating_add(bytes) <= self.budget()
    }

    /// Start the peak again from what the cache holds now: for a test
    /// that holds one phase of a store's life to a line, where an earlier
    /// phase was held to another (`Db::restart_cipher_peak`).
    pub(crate) fn restart_peak(&self) {
        self.peak.store(self.held(), Ordering::Relaxed);
    }

    /// Whether an ask of several chunks leaves each chunk's ciphertext to
    /// its own fault (`StoreCipher::chunks_apart`).
    pub(crate) fn keep_apart(&self, apart: bool) {
        self.apart.store(apart, Ordering::Relaxed);
    }

    /// Whether a merge reads its inputs beside the cache
    /// (`StoreCipher::merges_beside_cache`).
    pub(crate) fn merge_beside(&self, on: bool) {
        self.beside.store(on, Ordering::Relaxed);
    }

    /// What the evictor gave back of files since dropped, by the first
    /// field of each one's name (`given_gone`).
    pub(crate) fn given_gone(&self) -> Vec<(String, u64)> {
        lock(&self.given_gone).clone()
    }

    /// Whether a drop of a file's pages leaves its plaintext here
    /// (`StoreCipher::drops_keep_plaintext`).
    pub(crate) fn drops_keep(&self, keep: bool) {
        self.drops_keep.store(keep, Ordering::Relaxed);
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
            together: r(&c.together),
            rooms: r(&c.rooms),
            gave_way: r(&c.gave_way),
            spared: r(&c.spared),
            streamed: r(&c.streamed),
            dropped_units: r(&c.dropped_units),
            dropped_bytes: r(&c.dropped_bytes),
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

    /// Tell the evictor there is something to give back: once a look of
    /// its own, by the flag it clears before it looks. Every fill of a
    /// cache at its budget tells it, and the first version took the
    /// evictor's lock for each: three threads filling beside one another
    /// -- a scan, a merge, a builder -- slept on that lock a few thousand
    /// times a pass of ten thousand scans.
    fn nudge(&self) {
        if self.told.swap(true, Ordering::AcqRel) {
            return;
        }
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
    /// store can do without rests, and stops if it is crowded still
    /// (`Reader::build_rest`, `Db::same_blocks`).
    #[inline]
    pub(crate) fn crowded(&self) -> bool {
        self.could_give() > self.hard.load(Ordering::Relaxed)
    }

    /// What the cache holds that it could give back: all of it but the
    /// files with no envelope yet. The lines a reader waits at are drawn
    /// against this, since a wait is for the evictor and the evictor can
    /// do nothing about a file being written: counted whole, a seal's
    /// output in flight read as a cache past every line for as long as the
    /// seal took to sync, each merge beside it slept a breath at every
    /// rest, and the builders gave their forms up.
    #[inline]
    fn could_give(&self) -> u64 {
        self.held()
            .saturating_sub(self.unbacked.load(Ordering::Relaxed))
    }

    /// Whether the cache holds far more than it may: past its budget by
    /// half of it, or six units where that is more. `crowded` is where the
    /// store's own work waits -- a merge, a builder of the block cache --
    /// and this is where a caller's read does: a thread descheduled with a
    /// pin for one timeslice of a busy machine holds the evictor for ten
    /// milliseconds, the readers beside it fill past the first line in
    /// that time, and a read that waited there slept twenty milliseconds
    /// for an overshoot of megabytes.
    #[inline]
    pub(crate) fn swamped(&self) -> bool {
        self.could_give() > self.swamp.load(Ordering::Relaxed)
    }

    /// `breathe` for a caller's read: a wait for a cache that is `swamped`
    /// to be so no longer, or for a breath.
    pub(crate) fn breathe_swamped(&self) {
        if !self.swamped() {
            return;
        }
        Counts::bump(&self.counts.breaths);
        let began = std::time::Instant::now();
        while self.swamped() && began.elapsed() < BREATH {
            self.nudge();
            std::thread::sleep(Duration::from_micros(200));
        }
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
        self.breathe_unless(|| false);
    }

    /// `breathe` for a waiter someone may be waiting for in turn: the
    /// wait ends as well where `leave` says so, looked at between naps.
    pub(crate) fn breathe_unless(&self, leave: impl Fn() -> bool) {
        if !self.crowded() {
            return;
        }
        Counts::bump(&self.counts.breaths);
        let began = std::time::Instant::now();
        while self.crowded() && began.elapsed() < BREATH && !leave() {
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
            // Cleared before the look: a fill counted before this is in
            // what the look reads, and one after it tells again (`nudge`).
            p.told.store(false, Ordering::Release);
            p.follow_room();
            let idle = !p.pass();
            if idle {
                // Nothing to give back, or nothing that may be: wait to be
                // told, and look again before long in case the telling was
                // missed or a file has its envelope now.
                let owed = lock(&p.wake);
                let mut owed = if *owed {
                    owed
                } else {
                    p.woken
                        .wait_timeout(owed, p.idle_wait())
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                };
                *owed = false;
            }
        }
    }

    /// One pass: the units a drop asked for marked (`Paged::forget`), and
    /// where the cache is over its budget, units from the hand until what
    /// the marked hold covers what it is over by and a little more; the
    /// readers waited out once for all of them, and whichever were not
    /// taken back meanwhile given back. False when nothing was marked.
    fn pass(&self) -> bool {
        let (mut marked, dropped) = self.mark_dropped();
        let over = self.over();
        if over > 0 {
            Counts::bump(&self.counts.passes);
            // Under the budget by a margin, so that the next fill does not
            // start the next pass: a sixteenth of the budget, and a unit
            // at least. The first margin was eight units at least, eight
            // megabytes, which under a budget of two was every pass giving
            // back everything the store held.
            let budget = self.budget();
            let goal = over.saturating_add((budget / 16).max((unit_chunks() * CHUNK) as u64));
            if dropped < goal {
                marked.extend(self.mark(goal - dropped));
            }
            if marked.is_empty() {
                Counts::bump(&self.counts.stalls);
                return false;
            }
        } else if marked.is_empty() {
            return false;
        }
        self.give(&marked);
        true
    }

    /// The units a drop asked to be given back (`Paged::forget`), whatever
    /// the budget, each marked unless a chunk of it was read since the
    /// drop, and the plaintext they hold.
    fn mark_dropped(&self) -> (Vec<(Arc<Paged>, usize)>, u64) {
        let mut marked = Vec::new();
        let mut bytes = 0u64;
        if self.dropped.swap(0, Ordering::AcqRel) == 0 {
            return (marked, bytes);
        }
        let files: Vec<Arc<Paged>> = lock(&self.files).iter().filter_map(Weak::upgrade).collect();
        for f in &files {
            for (u, asked) in f.drop_asked.iter().enumerate() {
                if asked.swap(false, Ordering::AcqRel) {
                    let got = f.mark(u);
                    if got > 0 {
                        bytes += got;
                        f.dropping[u].store(true, Ordering::Relaxed);
                        marked.push((f.clone(), u));
                    }
                }
            }
        }
        (marked, bytes)
    }

    /// The marked units' pages given back once every reader that could
    /// have been lent a byte of them has gone, unless a reader took a unit
    /// back meanwhile, those a drop asked for counted apart. Nothing is
    /// given back where the store is closing.
    fn give(&self, marked: &[(Arc<Paged>, usize)]) {
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
                for (f, u) in marked {
                    f.unmark(*u);
                    f.dropping[*u].store(false, Ordering::Relaxed);
                }
                return;
            }
            std::thread::sleep(nap);
            nap = (nap * 2).min(Duration::from_millis(2));
        }
        let waited = began.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.counts.waited_us.fetch_add(waited, Ordering::Relaxed);
        self.counts
            .longest_wait_us
            .fetch_max(waited, Ordering::Relaxed);
        for (f, u) in marked {
            let freed = f.free(*u);
            if freed > 0 {
                Counts::bump(&self.counts.evicted_units);
                self.counts
                    .evicted_bytes
                    .fetch_add(freed, Ordering::Relaxed);
                if f.dropping[*u].swap(false, Ordering::Relaxed) {
                    Counts::bump(&self.counts.dropped_units);
                    self.counts
                        .dropped_bytes
                        .fetch_add(freed, Ordering::Relaxed);
                } else {
                    f.given.fetch_add(freed, Ordering::Relaxed);
                }
            }
        }
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
    /// Where the envelope was opened, for a drop of the kernel's cache of
    /// it (`Paged::forget`), which a mapping's own advice does not reach;
    /// and where in that file it begins, which is its start for an
    /// envelope that is a file and its place for one kept in a store's
    /// data file (`onefile`).
    path: std::path::PathBuf,
    at: u64,
    /// What holds the envelope's place in a store's data file for as long
    /// as this reads it: dropped with it, and the room is the store's.
    _room: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl Back {
    /// The envelope at `path`, under `aead`: its header checked as an open
    /// of an envelope checks it, and nothing decrypted.
    pub(crate) fn open(path: &std::path::Path, aead: Box<DynAead>) -> Result<Back> {
        let file = File::open(path)?;
        // SAFETY: the contract `MmapBytes::open` takes: an envelope is
        // written once, before the store names it, and not again.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        Back::over(map, path, 0, None, aead)
    }

    /// The envelope `map` is a mapping of, under `aead`: the file at
    /// `path` from its byte `at` on, held where it lies by `room` for as
    /// long as this stands.
    pub(crate) fn over(
        map: memmap2::Mmap,
        path: &std::path::Path,
        at: u64,
        room: Option<Box<dyn std::any::Any + Send + Sync>>,
        aead: Box<DynAead>,
    ) -> Result<Back> {
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
            path: path.to_path_buf(),
            at,
            _room: room,
        })
    }

    pub(crate) fn plain_len(&self) -> u64 {
        self.hdr.plain_len()
    }

    /// Chunk `c`'s plaintext into `dst`, its length: `stored` is the
    /// chunk's ciphertext and tag as the envelope holds them. A chunk that
    /// fails its check leaves `dst` zeroed.
    fn open_chunk(&self, c: usize, stored: &[u8], dst: &mut [u8], label: &str) -> Result<()> {
        let n = dst.len();
        dst.copy_from_slice(&stored[..n]);
        let tag: [u8; TAG] = stored[n..].try_into().expect("a tag's length");
        if !self
            .aead
            .decrypt(&self.hdr.nonce(c as u64), &self.aad, dst, &tag)
        {
            // Whatever the cipher left there is not plaintext anyone may
            // see.
            dst.fill(0);
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "{label}: chunk {c} failed its check: the store's files were changed, or the \
                     key is not theirs"
                ),
            ));
        }
        Ok(())
    }

    /// Chunk `c`'s ciphertext and tag, `n` bytes of plaintext long, as the
    /// envelope holds them.
    fn stored(&self, c: usize, n: usize, label: &str) -> Result<&[u8]> {
        let at = self.hdr.stored_off(c as u64) as usize;
        self.map.get(at..at + n + TAG).ok_or_else(|| {
            Error::new(
                ErrorKind::UnexpectedEof,
                format!("{label}: its envelope is shorter than its header says"),
            )
        })
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
    /// The units a drop of the file's pages has asked the evictor to give
    /// back (`Paged::forget`, `Pager::mark_dropped`).
    drop_asked: Box<[AtomicBool]>,
    /// The units marked for a drop and not yet given back, for the count
    /// of what drops gave back (`PagerCounts::dropped_units`).
    dropping: Box<[AtomicBool]>,
    back: OnceLock<Back>,
    /// The view advised for reads in order (`Paging::advise` with `random`
    /// false), which its envelope is given where it has one and when it
    /// comes where it has not: an envelope opens advised for reads far
    /// apart (`Back::open`), and a segment a seal lands readable is a
    /// memory file when its open advises it (`db::Seg::open_full`).
    around: AtomicBool,
    /// What the file is called, for an error.
    label: String,
    /// A file the store can make again, a form file (`zpg-<..>.zpg`, kept as
    /// `x-<token>.z`, by either name): a chunk
    /// of it that fails its tag is that file's failure and not the store's,
    /// the read answered as a range the file cannot lend, and the form's
    /// reader takes the file out of service and reads the partition raw
    /// (`ZFile::page_bytes`). Every other file's failure poisons the store.
    regenerable: bool,
    /// Chunks present, for the drop's count; kept by the fills and the
    /// frees.
    present: AtomicUsize,
    /// Plaintext the evictor has given back of this file over the budget,
    /// a drop's apart, bytes: the cache's trace sums it by kind of file
    /// (`SUPDB_PAGER_TRACE`).
    given: AtomicU64,
    /// A test's hold on a reader between its taking a unit back and its
    /// saying a chunk of it is lent (`pause_after_rescue`): 1 asked, 2
    /// held.
    #[cfg(test)]
    pause: AtomicU8,
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

/// `SUPDB_PAGER_TRACE=1`: a `pagerback` line on stderr where a view is
/// given its envelope, the file's name beside the envelope's and the
/// envelope's place in the file it lies in, so that a fault laid to an
/// envelope by its address is laid to the file it holds: by the
/// envelope's name where it is a file of the directory, and by its place
/// where every envelope is a range of one data file.
fn trace_back(label: &str, back: &Back) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("SUPDB_PAGER_TRACE").is_some()) {
        let env = back
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        eprintln!(
            "pagerback {label} {} at {} len {}",
            env.unwrap_or_default(),
            back.at,
            back.map.len()
        );
    }
}

/// A memory file: anonymous, the process's, gone with its last handle and
/// its last mapping.
/// The page cache's clean pages of `len` bytes of the file at `path` from
/// `from` dropped (`POSIX_FADV_DONTNEED`): Linux's advice, and nothing
/// elsewhere, where no store under a cipher is made (`memfile`).
#[cfg(target_os = "linux")]
fn drop_cached(path: &std::path::Path, from: usize, len: usize) {
    if let Ok(f) = File::open(path) {
        use std::os::fd::AsRawFd;
        // SAFETY: an advice over a descriptor this call holds.
        unsafe {
            libc::posix_fadvise(
                f.as_raw_fd(),
                from as libc::off_t,
                len as libc::off_t,
                libc::POSIX_FADV_DONTNEED,
            );
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn drop_cached(_path: &std::path::Path, _from: usize, _len: usize) {}

/// The `len` bytes of a read-only mapping at `p` mapped into this
/// process's page tables (`MADV_POPULATE_READ`), and whether the kernel
/// did so: Linux's advice, and nothing elsewhere.
///
/// # Safety
/// `p` is the start of a page of a mapping of at least `len` bytes that
/// lives through the call; the advice changes no byte.
#[cfg(target_os = "linux")]
unsafe fn populate_read(p: *const u8, len: usize) -> bool {
    // SAFETY: the caller's, above.
    unsafe { libc::madvise(p.cast_mut().cast(), len, libc::MADV_POPULATE_READ) == 0 }
}

/// # Safety
/// As the Linux one's.
#[cfg(not(target_os = "linux"))]
unsafe fn populate_read(_p: *const u8, _len: usize) -> bool {
    false
}

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
            drop_asked: (0..chunks.div_ceil(unit_chunks))
                .map(|_| AtomicBool::new(false))
                .collect(),
            dropping: (0..chunks.div_ceil(unit_chunks))
                .map(|_| AtomicBool::new(false))
                .collect(),
            back: OnceLock::new(),
            around: AtomicBool::new(false),
            label: label.to_string(),
            regenerable: label.ends_with(".zpg") || label.ends_with(".z"),
            present: AtomicUsize::new(if first == EMPTY { 0 } else { chunks }),
            given: AtomicU64::new(0),
            #[cfg(test)]
            pause: AtomicU8::new(0),
            #[cfg(debug_assertions)]
            viewed,
        });
        if first != EMPTY {
            pager.counts.adopted_bytes.fetch_add(len, Ordering::Relaxed);
            pager.unbacked.fetch_add(len, Ordering::Relaxed);
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
        trace_back(label, &back);
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
        trace_back(&self.label, &back);
        if self.back.set(back).is_ok() {
            self.pager.unbacked.fetch_sub(self.len, Ordering::Relaxed);
        }
        // The advice the view was given before it had an envelope. Read
        // after the set, as `advise` writes the word before it reads the
        // envelope, so one of the two applies it.
        if self.around.load(Ordering::SeqCst) {
            self.advise_back(false);
        }
        self.pager.nudge();
        Ok(())
    }

    pub(crate) fn backed(&self) -> bool {
        self.back.get().is_some()
    }

    /// The kernel told how the envelope is read, where the file has one:
    /// far apart, or in order, when the kernel's own read-ahead brings the
    /// next chunks' ciphertext in beside the one asked for.
    fn advise_back(&self, random: bool) {
        if let Some(b) = self.back.get() {
            let _ = b.map.advise(if random {
                memmap2::Advice::Random
            } else {
                memmap2::Advice::Normal
            });
        }
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
        if guard_on() {
            let ps = page_size();
            let a = (c * CHUNK) / ps * ps;
            let b = ((c * CHUNK + n).div_ceil(ps) * ps).min(self.mapped);
            // SAFETY: the pages this chunk lies in, of this mapping.
            unsafe {
                libc::mprotect(
                    self.base.add(a).cast(),
                    b - a,
                    libc::PROT_READ | libc::PROT_WRITE,
                )
            };
        }
        let dst = unsafe { std::slice::from_raw_parts_mut(self.base.add(c * CHUNK), n) };
        back.open_chunk(c, stored, dst, &self.label)
    }

    /// The ciphertext of chunks `c..=b` asked of the kernel in one piece,
    /// where two or more of them are not here: `c` is the first chunk of
    /// an ask of several that is not lent as it stands. The envelope is
    /// mapped for reads far apart, so each chunk's fill faults its own
    /// page of ciphertext and waits for the device by itself, and an ask
    /// that lies across two chunks, which is every page of a form file
    /// and one record in thirty, waited twice for two pages that lie side
    /// by side on the device. Asked for first, they come in one read.
    /// Nothing for an ask with one chunk gone, whose fault is the whole of
    /// its read, and nothing where the store keeps its chunks apart
    /// (`StoreCipher::chunks_apart`). A hint, read with no claim on the
    /// chunks: one that another thread fills meanwhile was asked for
    /// nothing.
    #[cold]
    #[inline(never)]
    fn ask_together(&self, c: usize, b: usize) {
        if self.pager.apart.load(Ordering::Relaxed) {
            return;
        }
        let Some((first, last)) = self.gone_span(c, b) else {
            return;
        };
        let Some(back) = self.back.get() else {
            return;
        };
        let ps = page_size();
        let from = back.hdr.stored_off(first as u64) as usize / ps * ps;
        let to = (back.hdr.stored_off(last as u64) as usize + self.chunk_len(last) + TAG)
            .min(back.map.len());
        if to <= from {
            return;
        }
        // SAFETY: an advice over pages of the envelope's mapping.
        unsafe {
            libc::madvise(
                back.map.as_ptr().add(from).cast_mut().cast(),
                to - from,
                libc::MADV_WILLNEED,
            );
        }
        self.pager.counts.together.fetch_add(1, Ordering::Relaxed);
    }

    /// The first and the last of chunks `c..=b` that are not here, where
    /// two or more are not: what `ask_together` asks for.
    fn gone_span(&self, c: usize, b: usize) -> Option<(usize, usize)> {
        let mut gone = (c..=b).filter(|&i| self.state[i].load(Ordering::Relaxed) == EMPTY);
        let first = gone.next()?;
        let last = gone.next_back()?;
        Some((first, last))
    }

    /// Chunk `c` copied into `dst`, its length, where the cache holds it:
    /// for a reader under a hold that keeps its own copy (`Stream`), which
    /// leaves the chunk's recency as it found it. A chunk seen held is
    /// one the evictor had not marked when the holder last stood at its
    /// era, so the evictor waits for the holder's next rest before it
    /// gives the chunk back, and the copy is made before then.
    fn copy_present(&self, c: usize, dst: &mut [u8]) -> bool {
        if !matches!(
            self.state.get(c).map(|s| s.load(Ordering::SeqCst)),
            Some(HOT | COLD)
        ) {
            return false;
        }
        // SAFETY: a chunk present, inside the mapping, `dst.len()` its
        // length, and not given back before the holder rests (above).
        dst.copy_from_slice(unsafe {
            std::slice::from_raw_parts(self.base.add(c * CHUNK), dst.len())
        });
        true
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
                    #[cfg(test)]
                    self.pause_after_rescue();
                    // The unit is lent from again, and this chunk with it
                    // -- if the unit is still the one taken back. Between
                    // the two a thread may lose its core for as long as it
                    // takes the evictor to end the pass this rescue was
                    // from and mark the unit again, this chunk dying in it
                    // as the mark found it. Said to be lent then, it is a
                    // chunk every reader after takes on that word alone
                    // (`ensure`), in a unit the evictor gives back as soon
                    // as the readers before its mark have gone, and those
                    // do not include the reader that has just been lent
                    // it. So the unit is looked at again once the chunk
                    // says so, and where it is marked the chunk is put
                    // back and the unit taken back from that mark too,
                    // which this thread is in time for: it has been pinned
                    // since before the mark, and the mark waits for it.
                    if state
                        .compare_exchange(DYING, HOT, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                        && unit.load(Ordering::SeqCst) != LIVE
                    {
                        let _ =
                            state.compare_exchange(HOT, DYING, Ordering::SeqCst, Ordering::SeqCst);
                    }
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
                            if !self.regenerable {
                                self.pager.poison(&e.to_string());
                            }
                            return false;
                        }
                    }
                }
                // Another thread is decrypting it: microseconds.
                _ => std::thread::yield_now(),
            }
        }
    }

    /// A test's: a reader that has taken a unit back held there, before it
    /// says its chunk is lent, until the test lets it go.
    #[cfg(test)]
    fn pause_after_rescue(&self) {
        if self
            .pause
            .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            while self.pause.load(Ordering::SeqCst) == 2 {
                std::thread::yield_now();
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
            if guard_on() {
                // SAFETY: the pages just given back, of this mapping.
                unsafe { libc::mprotect(self.base.add(a).cast(), b - a, libc::PROT_NONE) };
            }
        }
        let mut bytes = 0u64;
        let mut chunks = 0usize;
        for (i, s) in self.state[lo..hi].iter().enumerate() {
            let was = s.swap(EMPTY, Ordering::SeqCst);
            // Dying, as the mark left it: a chunk lent in a unit given
            // back is one a reader after the mark was reading.
            debug_assert!(
                !matches!(was, COLD | HOT),
                "{}: chunk {} was lent in a unit given back",
                self.label,
                lo + i
            );
            if matches!(was, DYING | COLD | HOT) {
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

    /// The units wholly inside `len` bytes of plaintext from `off`, of a
    /// file with its envelope, asked of the evictor to be given back at its
    /// next turn whatever the budget, as a plain store's drop gives back
    /// its pages (`StoreCipher::drops_keep_plaintext` leaves them): each
    /// unit's chunks marked unread, so that a read after the drop, which
    /// marks its chunk read again, keeps the unit, and the unit flagged for
    /// the evictor (`Pager::mark_dropped`). A unit is a megabyte, so a
    /// range under one asks for nothing, and a range's ends inside a unit
    /// leave that unit, as the kernel's drop leaves a folio a range does
    /// not cover whole.
    fn drop_units(&self, off: u64, len: usize) {
        if self.pager.drops_keep.load(Ordering::Relaxed)
            || len == 0
            || off >= self.len
            || self.back.get().is_none()
        {
            return;
        }
        let ub = (self.unit_chunks * CHUNK) as u64;
        let end = off.saturating_add(len as u64).min(self.len);
        let mut asked = 0usize;
        for u in off.div_ceil(ub) as usize..self.unit.len() {
            if ((u as u64 + 1) * ub).min(self.len) > end {
                break;
            }
            let lo = u * self.unit_chunks;
            let hi = (lo + self.unit_chunks).min(self.state.len());
            let mut present = false;
            for s in &self.state[lo..hi] {
                match s.load(Ordering::Relaxed) {
                    HOT => {
                        let _ = s.compare_exchange(HOT, COLD, Ordering::SeqCst, Ordering::SeqCst);
                        present = true;
                    }
                    COLD => present = true,
                    _ => {}
                }
            }
            if present && !self.drop_asked[u].swap(true, Ordering::AcqRel) {
                asked += 1;
            }
        }
        if asked > 0 {
            self.pager.dropped.fetch_add(asked, Ordering::AcqRel);
            self.pager.nudge();
        }
    }

    /// What the evictor has given back of this file over the budget,
    /// bytes, a drop's apart.
    pub(crate) fn given_bytes(&self) -> u64 {
        self.given.load(Ordering::Relaxed)
    }

    /// The plaintext this file holds present, bytes: its chunks' count,
    /// the last being short.
    pub(crate) fn present_bytes(&self) -> u64 {
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
        let given = self.given_bytes();
        if given > 0 {
            let field = self.label.split('-').next().unwrap_or(&self.label);
            let mut gone = lock(&self.pager.given_gone);
            match gone.iter_mut().find(|(f, _)| f == field) {
                Some((_, g)) => *g += given,
                None => gone.push((field.to_string(), given)),
            }
        }
        if !self.backed() {
            // Taken whole and gone before it had an envelope.
            self.pager.unbacked.fetch_sub(self.len, Ordering::Relaxed);
        }
        if self.mapped > 0 {
            // SAFETY: the mapping made in `make`, and nothing borrows from
            // a file whose last holder is going.
            unsafe { libc::munmap(self.base.cast(), self.mapped) };
        }
    }
}

impl Paging for Paged {
    fn base_address(&self, _: crate::bytes::BaseAddressToken) -> *const u8 {
        self.base
    }

    fn size(&self) -> u64 {
        self.len
    }

    #[inline]
    fn place(&self, off: u64, len: usize) -> Option<*const u8> {
        // Inside the mapping: the caller holds the range to the file's
        // length.
        self.ensure(off, len)
            .then(|| self.base.wrapping_add(off as usize).cast_const())
    }

    #[inline]
    fn place_piece(&self, off: u64) -> Option<(*const u8, usize)> {
        match self.piece_from(off) {
            0 => None,
            n => Some((self.base.wrapping_add(off as usize).cast_const(), n)),
        }
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
        let mut first = true;
        (a..=b).all(|c| {
            if self.state[c].load(Ordering::SeqCst) == HOT {
                return true;
            }
            if std::mem::take(&mut first) && c < b {
                self.ask_together(c, b);
            }
            self.slow(c)
        })
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
    /// that is not present comes from (`Paged::advise_back`), and the
    /// advice kept for an envelope the file has not yet.
    fn advise(&self, random: bool) {
        self.around.store(!random, Ordering::SeqCst);
        self.advise_back(random);
    }

    /// The chunks of the range a read would not wait on the device for:
    /// present here, or their ciphertext in the kernel's cache of the
    /// envelope, by one `mincore` over the envelope's bytes for the range.
    /// A hint, read with no pin: a chunk present now may be given back
    /// before anyone reads it.
    fn resident(&self, off: u64, len: usize) -> f64 {
        if len == 0 || off >= self.len {
            return 0.0;
        }
        let a = (off / CHUNK as u64) as usize;
        let b = (((off + len as u64).min(self.len) - 1) / CHUNK as u64) as usize;
        let cached = self.back.get().map(|back| {
            let ps = page_size();
            let from = back.hdr.stored_off(a as u64) as usize / ps * ps;
            let to = (back.hdr.stored_off(b as u64) as usize + self.chunk_len(b) + TAG)
                .min(back.map.len());
            let mut vec = vec![0u8; to.saturating_sub(from).div_ceil(ps)];
            // SAFETY: whole pages of the envelope's mapping, which lives
            // as long as this view, and a byte of `vec` for each.
            let rc = if to > from {
                unsafe {
                    libc::mincore(
                        back.map.as_ptr().add(from).cast_mut().cast(),
                        to - from,
                        vec.as_mut_ptr().cast(),
                    )
                }
            } else {
                -1
            };
            (rc == 0, from, vec)
        });
        let mut have = 0usize;
        for c in a..=b {
            let s = self.state[c].load(Ordering::Relaxed);
            if s == HOT || s == COLD {
                have += 1;
                continue;
            }
            if let (Some((true, from, vec)), Some(back)) = (&cached, self.back.get()) {
                let ps = page_size();
                let at = back.hdr.stored_off(c as u64) as usize;
                let end = at + self.chunk_len(c) + TAG;
                let pages = (at / ps * ps..end).step_by(ps);
                if pages
                    .map(|p| (p - from) / ps)
                    .all(|i| vec.get(i).is_some_and(|b| b & 1 != 0))
                {
                    have += 1;
                }
            }
        }
        have as f64 / (b - a + 1) as f64
    }

    /// The range given up: the units wholly inside it asked of the
    /// evictor (`drop_units`), and the kernel's cache of the envelope's
    /// bytes for it, this process's entries for them dropped and then the
    /// file's clean pages, which a drop skips while a mapping holds them,
    /// whole pages inside the range's ciphertext alone.
    fn forget(&self, off: u64, len: usize) {
        self.drop_units(off, len);
        let Some((back, mut from, mut to)) = self.stored_pages(off, len, true) else {
            return;
        };
        // The whole file is dropped whole, its header's page with it: the
        // page cache holds a file in folios the size of the writes that
        // made them, a megabyte for an envelope, and a drop takes only the
        // folios wholly inside its range, so a range a page short of the
        // file left the first megabyte of it.
        if off == 0 && len as u64 >= self.len {
            (from, to) = (0, back.map.len());
        }
        // SAFETY: an advice over whole pages of a read-only shared mapping
        // of a file this view holds: no byte changes, and a chunk read
        // after it is read from the file again.
        unsafe {
            libc::madvise(
                back.map.as_ptr().add(from).cast_mut().cast(),
                to - from,
                libc::MADV_DONTNEED,
            );
        }
        drop_cached(&back.path, back.at as usize + from, to - from);
    }

    /// The envelope's bytes for the range asked of the kernel, in the
    /// pieces it reads whole (`bytes::WILLNEED_PIECE`): the read a chunk
    /// not present here will make of the file, started before the reader
    /// gets there. The envelope is mapped for far-apart reads, so nothing
    /// reads ahead of a walk but this.
    fn willneed(&self, off: u64, len: usize) -> usize {
        let Some((back, from, to)) = self.stored_pages(off, len, false) else {
            return 0;
        };
        // Nothing for a range every chunk of which is here already.
        let a = (off / CHUNK as u64) as usize;
        let b = (((off + len as u64).min(self.len) - 1) / CHUNK as u64) as usize;
        if (a..=b).all(|c| matches!(self.state[c].load(Ordering::Relaxed), HOT | COLD)) {
            return 0;
        }
        let mut at = from;
        while at < to {
            let n = (to - at).min(crate::bytes::WILLNEED_PIECE);
            // SAFETY: an advice over pages of the envelope's mapping.
            unsafe {
                libc::madvise(
                    back.map.as_ptr().add(at).cast_mut().cast(),
                    n,
                    libc::MADV_WILLNEED,
                );
            }
            at += n;
        }
        to - from
    }

    /// The envelope's pages for the chunks of the range that are not
    /// present here, mapped into this process's page tables
    /// (`MADV_POPULATE_READ`): the pages a fill of such a chunk reads its
    /// ciphertext through, each a fault where the kernel's cache holds it
    /// unmapped. A run of chunks at a time, so nothing of a chunk
    /// decrypted already is read back from the device. A hint, read with
    /// no pin, as `resident` is.
    fn populate(&self, off: u64, len: usize) -> bool {
        let Some(back) = self.back.get() else {
            return false;
        };
        if len == 0 || off >= self.len {
            return false;
        }
        let ps = page_size();
        let here = |c: usize| matches!(self.state[c].load(Ordering::Relaxed), HOT | COLD);
        let a = (off / CHUNK as u64) as usize;
        let b = (((off + len as u64).min(self.len) - 1) / CHUNK as u64) as usize;
        let (mut c, mut any) = (a, false);
        while c <= b {
            if here(c) {
                c += 1;
                continue;
            }
            let first = c;
            while c <= b && !here(c) {
                c += 1;
            }
            let lo = back.hdr.stored_off(first as u64) as usize / ps * ps;
            let hi = (back.hdr.stored_off(c as u64 - 1) as usize + self.chunk_len(c - 1) + TAG)
                .min(back.map.len());
            if lo < hi {
                // SAFETY: pages of the envelope's read-only mapping, which
                // lives as long as this view.
                any |= unsafe { populate_read(back.map.as_ptr().add(lo), hi - lo) };
            }
        }
        any
    }

    /// The chunks of the range a read reaches with no fault of the page
    /// tables': present here, or every page their ciphertext lies in
    /// mapped in the envelope's mapping, by `/proc/self/pagemap`. `None`
    /// where a chunk is not present and the kernel will not say.
    fn mapped(&self, off: u64, len: usize) -> Option<f64> {
        if len == 0 || off >= self.len {
            return None;
        }
        let ps = page_size();
        let a = (off / CHUNK as u64) as usize;
        let b = (((off + len as u64).min(self.len) - 1) / CHUNK as u64) as usize;
        let back = self.back.get();
        let behind = back.and_then(|back| {
            let from = back.hdr.stored_off(a as u64) as usize / ps * ps;
            let to = (back.hdr.stored_off(b as u64) as usize + self.chunk_len(b) + TAG)
                .min(back.map.len());
            let base = back.map.as_ptr() as usize;
            crate::bytes::pagemap_present(base + from, base + to).map(|bits| (from, bits))
        });
        let mut have = 0usize;
        for c in a..=b {
            if matches!(self.state[c].load(Ordering::Relaxed), HOT | COLD) {
                have += 1;
                continue;
            }
            let (Some(back), Some((from, bits))) = (back, &behind) else {
                return None;
            };
            let at = back.hdr.stored_off(c as u64) as usize;
            let end = (at + self.chunk_len(c) + TAG).min(back.map.len());
            if (at / ps * ps..end)
                .step_by(ps)
                .all(|p| bits.get((p - from) / ps).is_some_and(|&b| b))
            {
                have += 1;
            }
        }
        Some(have as f64 / (b - a + 1) as f64)
    }
}

/// A file of a store under a cipher read once and in order by one thread
/// under a hold, a merge over its inputs: its chunks decrypted into a
/// buffer of the view's own and lent from there, and nothing of it in the
/// store's cache.
///
/// Read through the cache, a merge filled a fresh page of a memory file
/// for every chunk of its inputs, a fault on that page and one on the
/// envelope's, and the cache gave the units back behind it, a megabyte at
/// a time across every core's page tables, while the scans beside it lost
/// what the merge's reads had put out: in the lag sweep's full rewrite at
/// ten million keys under 1.25 GiB the fills were two thirds of what the
/// cipher added to a merge, and seven tenths of a fill was faults. A
/// compaction of a store that keeps a cache of blocks reads past the cache
/// for the same reason. Here the buffer's pages are touched once and
/// written again at every rest of the holder: a lend stands until the
/// holder next rests or idles (`Hold::rest`, `Hold::idle`), where by the
/// hold's contract it holds nothing it was lent, and the first ask after
/// that writes the buffer from its start. So unlike the cache's, a range
/// asked for again after a rest is the same bytes at another address, and
/// a walk read under a stream reads only what it asked for since its last
/// rest -- which every walk of a merge does, asking for each input's key
/// again at the step after a rest (`db::merge_ranks`).
///
/// One thread's: the hold it was made under is, and a stream is used by
/// that thread alone (checked under debug assertions).
pub(crate) struct Stream {
    pager: Arc<Pager>,
    back: Back,
    /// The cache's view of the file, where it has one: a chunk it holds is
    /// copied from it and not decrypted again, which for a file just
    /// written is every chunk, its plaintext still whole in the cache and
    /// its envelope, under a bound, out of the kernel's.
    cached: Option<Arc<Paged>>,
    len: u64,
    /// Address space the file's length mapped with no access, which is
    /// what `base_address` answers: nothing reads a stream through it.
    base: *mut u8,
    mapped: usize,
    /// The holder's rests (`Hold::rests`).
    rests: Arc<AtomicU64>,
    ring: std::cell::UnsafeCell<Ring>,
    label: String,
    #[cfg(debug_assertions)]
    owner: usize,
}

/// This thread, as a stream's debug check of its reader tells threads
/// apart: the address of a value of the thread's own.
#[cfg(debug_assertions)]
fn this_thread() -> usize {
    thread_local!(static MARK: u8 = const { 0 });
    MARK.with(|m| std::ptr::from_ref(m) as usize)
}

// SAFETY: a stream is read by the one thread whose hold made it (checked
// under debug assertions); `base` is address space nothing reads and the
// ring's buffers are the stream's own.
unsafe impl Send for Stream {}
// SAFETY: as above; `Paging` asks for `Sync` because the cache's views are
// shared, and a stream is not.
unsafe impl Sync for Stream {}

/// What a stream has decrypted over its holder's last two stretches
/// between rests: buffers it writes in order, and the runs of chunks in
/// them, each a run of the file's chunks one after another. A buffer
/// written in the stretch before the last one is free to write again, and
/// a run is lent from while it is in this stretch or the one before: what
/// the walk asks for again after a rest is where the last stretch left it,
/// the chunk each input's walk stands on, and is found there rather than
/// decrypted again. A rest is a promise that nothing lent before it is
/// read, so a buffer of the stretch before is not written while a lend of
/// it could be wanted, and nothing of it is read that was not lent again.
struct Ring {
    /// The holder's rests as the ring last saw them: the stretch it is in.
    rest: u64,
    /// Each buffer: where, how long, how much of it is written, and the
    /// last stretch it was written in.
    bufs: Vec<Buf>,
    /// The buffer being written.
    buf: usize,
    /// The runs most lately written: the first chunk, the chunk after the
    /// last, where the first lies, its buffer and the last stretch it grew
    /// in.
    runs: [Run; STREAM_RUNS],
    newest: usize,
}

#[derive(Clone, Copy)]
struct Buf {
    at: *mut u8,
    cap: usize,
    used: usize,
    rest: u64,
}

#[derive(Clone, Copy)]
struct Run {
    first: u64,
    end: u64,
    at: *const u8,
    buf: usize,
    rest: u64,
}

impl Run {
    const NONE: Run = Run {
        first: 0,
        end: 0,
        at: std::ptr::null(),
        buf: 0,
        rest: 0,
    };

    /// Lent from in stretch `rest`: written in it or the one before.
    fn lends(&self, rest: u64) -> bool {
        !self.at.is_null() && self.rest + 1 >= rest
    }
}

/// The runs a stream remembers: an input of a merge is asked for at a key,
/// its extents and its values, a few places a step, and the runs since the
/// last rest are few.
const STREAM_RUNS: usize = 8;

/// A stream's buffer: what a merge reads of one input between two rests is
/// a few chunks, and a range longer than this has a buffer of its own.
const STREAM_BUF: usize = 64 << 10;

impl Stream {
    fn new(
        pager: &Arc<Pager>,
        back: Back,
        cached: Option<Arc<Paged>>,
        rests: Arc<AtomicU64>,
        label: &str,
    ) -> Result<Arc<Stream>> {
        let len = back.plain_len();
        let size = usize::try_from(len)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "a file longer than an address"))?;
        let page = page_size();
        let mapped = size.div_ceil(page) * page;
        let base = if mapped == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            // SAFETY: a new mapping of address space the kernel places,
            // with no access and no memory behind it.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    mapped,
                    libc::PROT_NONE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                    -1,
                    0,
                )
            };
            if p == libc::MAP_FAILED {
                return Err(Error::last_os_error());
            }
            p.cast()
        };
        Ok(Arc::new(Stream {
            pager: pager.clone(),
            back,
            cached,
            len,
            base,
            mapped,
            ring: std::cell::UnsafeCell::new(Ring {
                rest: rests.load(Ordering::Relaxed),
                bufs: Vec::new(),
                buf: 0,
                runs: [Run::NONE; STREAM_RUNS],
                newest: 0,
            }),
            rests,
            label: label.to_string(),
            #[cfg(debug_assertions)]
            owner: this_thread(),
        }))
    }

    fn chunk_len(&self, c: u64) -> usize {
        (self.len - c * CHUNK as u64).min(CHUNK as u64) as usize
    }

    /// Chunks `a..=b` lent from where the ring holds them, decrypting what
    /// it does not hold since the holder's last rest.
    fn chunks(&self, a: u64, b: u64) -> Option<*const u8> {
        #[cfg(debug_assertions)]
        assert_eq!(
            this_thread(),
            self.owner,
            "{}: a stream read by a thread other than its holder's",
            self.label
        );
        // SAFETY: the ring is this thread's alone (above), and no borrow of
        // it outlives this call: what is lent is the buffers' bytes, which
        // the ring's own fields do not alias.
        let ring = unsafe { &mut *self.ring.get() };
        let rest = self.rests.load(Ordering::Relaxed);
        if rest != ring.rest {
            ring.restart(rest);
        }
        for i in 0..STREAM_RUNS {
            let r = ring.runs[i];
            if r.lends(rest) && r.first <= a && b < r.end {
                // Lent in this stretch: its buffer is kept through the
                // next, and the run with it.
                if r.rest < rest {
                    ring.runs[i].rest = rest;
                    ring.bufs[r.buf].rest = rest;
                }
                return Some(r.at.wrapping_add((a - r.first) as usize * CHUNK));
            }
        }
        // The run written last, where the ask goes on from its end or from
        // inside it, and nothing has been written after it in its buffer:
        // the walk reads in order, and the run grows.
        let r = ring.runs[ring.newest];
        let grows = r.lends(rest)
            && r.first <= a
            && a <= r.end
            && b >= r.end
            && ring.buf == r.buf
            && ring.bufs.get(r.buf).is_some_and(|s| {
                r.at.wrapping_add((r.end - r.first) as usize * CHUNK)
                    == s.at.wrapping_add(s.used).cast_const()
                    && s.used + (b + 1 - r.end) as usize * CHUNK <= s.cap
            });
        let (from, dst) = if grows {
            (r.end, ring.take((b + 1 - r.end) as usize * CHUNK))
        } else {
            (a, ring.take((b + 1 - a) as usize * CHUNK))
        };
        let mut put = dst;
        for c in from..=b {
            let n = self.chunk_len(c);
            // SAFETY: `n` bytes of a buffer the ring has just handed out,
            // which nothing lent in this stretch or the one before
            // overlaps.
            let into = unsafe { std::slice::from_raw_parts_mut(put, n) };
            // A chunk a run holds is copied from it: a record across two
            // chunks asks for both, and the run that holds the first is
            // seldom the one that can grow, an input's directory being
            // read beside its records.
            if let Some(r) = ring
                .runs
                .iter()
                .find(|r| r.lends(rest) && r.first <= c && c < r.end)
            {
                // SAFETY: `n` bytes of a run lent in this stretch or the
                // one before, which no buffer handed out since overlaps.
                into.copy_from_slice(unsafe {
                    std::slice::from_raw_parts(r.at.wrapping_add((c - r.first) as usize * CHUNK), n)
                });
                put = put.wrapping_add(CHUNK);
                continue;
            }
            if self
                .cached
                .as_ref()
                .is_some_and(|p| p.copy_present(c as usize, into))
            {
                put = put.wrapping_add(CHUNK);
                continue;
            }
            let got = self
                .back
                .stored(c as usize, n, &self.label)
                .and_then(|s| self.back.open_chunk(c as usize, s, into, &self.label));
            if let Err(e) = got {
                ring.runs = [Run::NONE; STREAM_RUNS];
                self.pager.poison(&e.to_string());
                return None;
            }
            Counts::bump(&self.pager.counts.streamed);
            put = put.wrapping_add(CHUNK);
        }
        if grows {
            let r = &mut ring.runs[ring.newest];
            r.end = b + 1;
            r.rest = rest;
            Some(r.at.wrapping_add((a - r.first) as usize * CHUNK))
        } else {
            ring.newest = (ring.newest + 1) % STREAM_RUNS;
            ring.runs[ring.newest] = Run {
                first: a,
                end: b + 1,
                at: dst.cast_const(),
                buf: ring.buf,
                rest,
            };
            Some(dst.cast_const())
        }
    }
}

impl Ring {
    /// The holder has rested: the stretch it was in is now the one before,
    /// lent from still, and what was written before it is free.
    fn restart(&mut self, rest: u64) {
        self.rest = rest;
        if guard_on() {
            // What a stale borrow of a free buffer would read, made plain:
            // not plaintext.
            for s in &self.bufs {
                if s.rest + 1 < rest {
                    // SAFETY: a buffer of the ring's, `cap` bytes long,
                    // that nothing lent in the last two stretches lies in.
                    unsafe { std::ptr::write_bytes(s.at, 0xA5, s.cap) };
                }
            }
        }
    }

    /// `n` bytes of buffer nothing lent in this stretch or the one before
    /// lies in: the rest of the buffer being written, a free one, or a new
    /// one. The buffer it is in is marked as written in this stretch.
    fn take(&mut self, n: usize) -> *mut u8 {
        let rest = self.rest;
        if let Some(s) = self.bufs.get_mut(self.buf) {
            if s.rest + 1 >= rest && s.used + n <= s.cap {
                let at = s.at.wrapping_add(s.used);
                s.used += n;
                s.rest = rest;
                return at;
            }
        }
        if let Some((i, s)) = self
            .bufs
            .iter_mut()
            .enumerate()
            .find(|(_, s)| s.rest + 1 < rest && s.cap >= n)
        {
            s.used = n;
            s.rest = rest;
            self.buf = i;
            return s.at;
        }
        let cap = n.max(STREAM_BUF);
        let at = Box::into_raw(vec![0u8; cap].into_boxed_slice()).cast::<u8>();
        self.bufs.push(Buf {
            at,
            cap,
            used: n,
            rest,
        });
        self.buf = self.bufs.len() - 1;
        at
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        for s in &self.bufs {
            // SAFETY: each a boxed slice of `cap` bytes made in `take`, and
            // the stream that lent from it is going.
            drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(s.at, s.cap)) });
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.mapped > 0 {
            // SAFETY: the address space made in `new`, read by nobody.
            unsafe { libc::munmap(self.base.cast(), self.mapped) };
        }
    }
}

impl Paging for Stream {
    fn base_address(&self, _: crate::bytes::BaseAddressToken) -> *const u8 {
        // Address space with no access: a stream lends by `place` alone.
        self.base.cast_const()
    }

    fn size(&self) -> u64 {
        self.len
    }

    /// Nothing is made present where the mapping is: a stream lends from
    /// its ring, by `place`.
    fn ensure(&self, _: u64, _: usize) -> bool {
        false
    }

    fn place(&self, off: u64, len: usize) -> Option<*const u8> {
        if len == 0 {
            return Some(self.base.cast_const());
        }
        let end = off.checked_add(len as u64).filter(|&e| e <= self.len)?;
        let (a, b) = (off / CHUNK as u64, (end - 1) / CHUNK as u64);
        let at = self.chunks(a, b)?;
        Some(at.wrapping_add((off - a * CHUNK as u64) as usize))
    }

    fn place_piece(&self, off: u64) -> Option<(*const u8, usize)> {
        if off >= self.len {
            return None;
        }
        let c = off / CHUNK as u64;
        let at = self.chunks(c, c)?;
        let n = (((c + 1) * CHUNK as u64).min(self.len) - off) as usize;
        Some((at.wrapping_add((off - c * CHUNK as u64) as usize), n))
    }

    fn piece_end(&self, off: u64) -> u64 {
        ((off / CHUNK as u64 + 1) * CHUNK as u64).min(self.len)
    }

    fn piece_from(&self, off: u64) -> usize {
        self.place_piece(off).map_or(0, |(_, n)| n)
    }

    fn advise(&self, random: bool) {
        let _ = self.back.map.advise(if random {
            memmap2::Advice::Random
        } else {
            memmap2::Advice::Normal
        });
    }

    /// Nothing of a stream stays: a read finds in memory only the
    /// envelope's pages the kernel holds, which a stream does not ask
    /// after.
    fn resident(&self, _: u64, _: usize) -> f64 {
        0.0
    }

    fn forget(&self, off: u64, len: usize) {
        let Some((from, to)) = self.stored_span(off, len) else {
            return;
        };
        // SAFETY: an advice over whole pages of a read-only shared mapping
        // of a file this view holds: no byte changes.
        unsafe {
            libc::madvise(
                self.back.map.as_ptr().add(from).cast_mut().cast(),
                to - from,
                libc::MADV_DONTNEED,
            );
        }
    }

    fn willneed(&self, off: u64, len: usize) -> usize {
        let Some((from, to)) = self.stored_span(off, len) else {
            return 0;
        };
        let mut at = from;
        while at < to {
            let n = (to - at).min(crate::bytes::WILLNEED_PIECE);
            // SAFETY: an advice over pages of the envelope's mapping.
            unsafe {
                libc::madvise(
                    self.back.map.as_ptr().add(at).cast_mut().cast(),
                    n,
                    libc::MADV_WILLNEED,
                );
            }
            at += n;
        }
        to - from
    }

    fn populate(&self, _: u64, _: usize) -> bool {
        false
    }

    fn mapped(&self, _: u64, _: usize) -> Option<f64> {
        None
    }
}

impl Stream {
    /// The envelope's whole pages for the chunks of `len` bytes at `off`.
    fn stored_span(&self, off: u64, len: usize) -> Option<(usize, usize)> {
        if len == 0 || off >= self.len {
            return None;
        }
        let ps = page_size();
        let a = off / CHUNK as u64;
        let b = ((off + len as u64).min(self.len) - 1) / CHUNK as u64;
        let from = self.back.hdr.stored_off(a) as usize / ps * ps;
        let to = (self.back.hdr.stored_off(b) as usize + self.chunk_len(b) + TAG)
            .min(self.back.map.len());
        (from < to).then_some((from, to))
    }
}

impl Paged {
    /// The envelope and the pages of it that hold the chunks of `len`
    /// bytes at `off`: the pages wholly inside those chunks' stored bytes
    /// (`inside`), which is what may be dropped, or every page a byte of
    /// them lies in, which is what a read of them touches.
    fn stored_pages(&self, off: u64, len: usize, inside: bool) -> Option<(&Back, usize, usize)> {
        let back = self.back.get()?;
        if len == 0 || off >= self.len {
            return None;
        }
        let ps = page_size();
        let a = (off / CHUNK as u64) as usize;
        let b = (((off + len as u64).min(self.len) - 1) / CHUNK as u64) as usize;
        let lo = back.hdr.stored_off(a as u64) as usize;
        let hi =
            (back.hdr.stored_off(b as u64) as usize + self.chunk_len(b) + TAG).min(back.map.len());
        let (from, to) = if inside {
            (lo.next_multiple_of(ps), hi / ps * ps)
        } else {
            (lo / ps * ps, hi.next_multiple_of(ps).min(back.map.len()))
        };
        (from < to).then_some((back, from, to))
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

/// Whether pages given back are made unreadable until they are filled
/// again: under debug assertions, with `SUPDB_PAGER_GUARD` set. A page
/// given back reads as zeroes, so a read through a borrow that outlived
/// its pages is a key that is not there or a record of nothing, found far
/// from where it was made; with this it is a fault at the read, and
/// `gdb -batch -ex run -ex bt` over the test names the reader. A chunk
/// shares a page with its neighbour, so a page is readable from the first
/// of its chunks filled, and a read of the other half of it is not seen.
/// It is how the chunk lent out of a unit marked twice was found
/// (`Paged::slow`): one seed in three hundred of the fuzz under a cache of
/// sixteen kilobytes, four seeds at a time, and a fault in the first run
/// with this set.
#[cfg(debug_assertions)]
fn guard_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SUPDB_PAGER_GUARD").is_some())
}

#[cfg(not(debug_assertions))]
const fn guard_on() -> bool {
    false
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

    /// A stream of `path`'s envelope under `hold`, as a merge reads one.
    fn stream_of(hold: &Hold, path: &std::path::Path, key: u64) -> crate::bytes::MmapBytes {
        let back = Back::open(path, Box::new(Toy(key))).unwrap();
        crate::bytes::MmapBytes::paged(hold.stream(back, None, "test").unwrap())
    }

    /// A stream lends the plaintext of any range of its file -- inside a
    /// chunk, across chunks, longer than its buffer, empty, a piece -- and
    /// refuses a range past the end, and the store's cache neither holds
    /// nor counts any of it.
    #[test]
    fn a_stream_lends_any_range_and_holds_nothing_in_the_cache() {
        use crate::bytes::Bytes;
        let dir = scratch("stream-ranges");
        let bytes = plain(3 * STREAM_BUF + 12_345, 5);
        let path = dir.join("e");
        envelope(&path, &bytes, 4);
        let pager = Pager::new(u64::MAX);
        let hold = pager.hold();
        let m = stream_of(&hold, &path, 4);
        let len = bytes.len() as u64;
        assert_eq!(m.len(), len);
        let mut s = 99u64;
        let mut next = |n: u64| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        };
        let mut ranges: Vec<(u64, usize)> = vec![
            (0, 0),
            (0, 1),
            (CHUNK as u64 - 3, 7),
            (len - 1, 1),
            (5 * CHUNK as u64, 2 * STREAM_BUF),
            (0, bytes.len()),
        ];
        for _ in 0..400 {
            let off = next(len);
            let n = next((len - off).min(3 * CHUNK as u64) + 1) as usize;
            ranges.push((off, n));
        }
        for (i, &(off, n)) in ranges.iter().enumerate() {
            if i % 37 == 0 {
                hold.rest();
            }
            let got = m.slice_at(off, n).expect("a range inside the file");
            assert_eq!(got, &bytes[off as usize..off as usize + n], "{off}+{n}");
        }
        assert!(m.slice_at(len - 1, 2).is_none(), "past the end");
        assert!(m.slice_at(len, 1).is_none(), "past the end");
        let p = m.piece_at(2 * CHUNK as u64 + 10).expect("a piece");
        assert_eq!(p, &bytes[2 * CHUNK + 10..3 * CHUNK]);
        assert_eq!(pager.held(), 0, "nothing in the cache");
        assert!(
            pager.counts().streamed > 0,
            "the chunks counted as streamed"
        );
        assert_eq!(pager.counts().fills, 0, "and not as the cache's fills");
        assert!(pager.check().is_ok());
    }

    /// What a stream lends stands until its holder rests or idles, however
    /// much is asked for meanwhile, and a walk in order decrypts each chunk
    /// once. After a rest the walk asks again for where it stood, which is
    /// lent from the stretch before without decrypting it again and kept
    /// another stretch for being found; what nothing asks for through two
    /// rests is written over, from the first buffer, and a chunk of it is
    /// decrypted again. An idle is a rest.
    #[test]
    fn a_stream_keeps_its_lends_until_a_rest_and_the_stretch_before_until_the_next() {
        use crate::bytes::Bytes;
        let dir = scratch("stream-rests");
        let bytes = plain(4 * STREAM_BUF, 6);
        let path = dir.join("e");
        envelope(&path, &bytes, 2);
        let pager = Pager::new(u64::MAX);
        let hold = pager.hold();
        let m = stream_of(&hold, &path, 2);
        let chunks = bytes.len().div_ceil(CHUNK) as u64;
        // A walk in order, in asks that cross chunks, every lend kept.
        let mut kept: Vec<(usize, &[u8])> = Vec::new();
        let mut off = 0usize;
        while off < bytes.len() {
            let n = 300.min(bytes.len() - off);
            kept.push((off, m.slice_at(off as u64, n).unwrap()));
            off += n;
        }
        for &(off, got) in &kept {
            assert_eq!(got, &bytes[off..off + got.len()], "a lend at {off} kept");
        }
        // Each chunk once, but where a run reaches the end of a buffer: the
        // ask across it starts the run again in the next, with the chunk
        // the last one ended on.
        let walked = pager.counts().streamed;
        let buffers = (chunks as usize * CHUNK).div_ceil(STREAM_BUF) as u64;
        assert!(
            (chunks..chunks + buffers).contains(&walked),
            "{walked} chunks decrypted for {chunks} in {buffers} buffers"
        );
        let first = kept[0].1.as_ptr();
        let (last, tail) = *kept.last().unwrap();
        let tail = tail.as_ptr();
        drop(kept);
        let count = || pager.counts().streamed;
        // A rest: where the walk stood is lent again as it lies, and found
        // so it is kept through the next rest too.
        hold.rest();
        let again = m.slice_at(last as u64, bytes.len() - last).unwrap();
        assert_eq!(again, &bytes[last..]);
        assert_eq!(again.as_ptr(), tail, "the stretch before lent from");
        assert_eq!(count(), walked, "and nothing decrypted again");
        hold.rest();
        let again = m.slice_at(last as u64, bytes.len() - last).unwrap();
        assert_eq!(again.as_ptr(), tail, "a run found is kept another stretch");
        assert_eq!(count(), walked);
        // Two rests with nothing asked: the first buffer is free, and is
        // written again.
        hold.rest();
        hold.rest();
        let fresh = m.slice_at(0, 300).unwrap();
        assert_eq!(fresh, &bytes[..300]);
        assert_eq!(fresh.as_ptr(), first, "the first buffer written again");
        assert_eq!(count(), walked + 1);
        // An idle is a rest: the stretch before is lent from, and then not.
        hold.idle(|| ());
        assert_eq!(m.slice_at(0, 300).unwrap(), &bytes[..300]);
        assert_eq!(count(), walked + 1, "lent from the stretch before an idle");
        hold.idle(|| ());
        hold.idle(|| ());
        assert_eq!(m.slice_at(0, 300).unwrap(), &bytes[..300]);
        assert_eq!(count(), walked + 2, "decrypted again two idles on");
        // And between two rests nothing is decrypted twice.
        let _ = m.slice_at(100, 200).unwrap();
        assert_eq!(count(), walked + 2);
    }

    /// A stream's ring writes no buffer written in this stretch or the one
    /// before, which is what a walk may still be lent after a rest, and
    /// writes again one written two rests ago.
    #[test]
    fn a_rings_buffer_is_written_again_two_rests_after_it_was_written() {
        let mut ring = Ring {
            rest: 0,
            bufs: Vec::new(),
            buf: 0,
            runs: [Run::NONE; STREAM_RUNS],
            newest: 0,
        };
        let a = ring.take(STREAM_BUF);
        let b = ring.take(STREAM_BUF);
        assert_ne!(a, b);
        ring.restart(1);
        let c = ring.take(STREAM_BUF);
        assert!(
            c != a && c != b,
            "a buffer of the stretch before written over"
        );
        ring.restart(2);
        let d = ring.take(STREAM_BUF);
        assert!(d == a || d == b, "a buffer two rests old not written again");
        let e = ring.take(STREAM_BUF);
        assert!(e == a || e == b, "the other one either");
        assert_ne!(d, e);
        assert_eq!(ring.bufs.len(), 3);
    }

    /// An input of a merge is read in two places at once, its directory
    /// and its records, so the run a record across two chunks starts in is
    /// seldom the one written last: the chunk it already holds is copied
    /// from it, and no chunk the walk touches is decrypted twice.
    #[test]
    fn a_stream_read_in_two_places_decrypts_each_chunk_once() {
        use crate::bytes::Bytes;
        let dir = scratch("stream-two");
        let bytes = plain(40 * CHUNK, 8);
        let path = dir.join("e");
        envelope(&path, &bytes, 5);
        let pager = Pager::new(u64::MAX);
        let hold = pager.hold();
        let m = stream_of(&hold, &path, 5);
        let dir_at = 36 * CHUNK;
        let mut touched = std::collections::BTreeSet::new();
        let mut rec = 0usize;
        let mut i = 0usize;
        while rec + 300 < 30 * CHUNK {
            let d = dir_at + 8 * i;
            assert_eq!(m.slice_at(d as u64, 8).unwrap(), &bytes[d..d + 8]);
            assert_eq!(m.slice_at(rec as u64, 300).unwrap(), &bytes[rec..rec + 300]);
            for at in [d, d + 7, rec, rec + 299] {
                touched.insert(at / CHUNK);
            }
            rec += 300;
            i += 1;
        }
        assert_eq!(
            pager.counts().streamed,
            touched.len() as u64,
            "each chunk touched decrypted once"
        );
    }

    /// A stream over a file the cache holds part of takes those chunks from
    /// the cache's view, as they lie and without touching their recency,
    /// and decrypts the rest.
    #[test]
    fn a_stream_takes_what_the_cache_holds_from_it() {
        use crate::bytes::Bytes;
        let dir = scratch("stream-cached");
        let bytes = plain(30 * CHUNK, 9);
        let path = dir.join("e");
        envelope(&path, &bytes, 6);
        let pager = Pager::new(u64::MAX);
        let hold = pager.hold();
        let view = sparse(&pager, &path, 6);
        for c in [3usize, 4, 10, 29] {
            read(&view, (c * CHUNK) as u64, 10).unwrap();
        }
        let fills = pager.counts().fills;
        let back = Back::open(&path, Box::new(Toy(6))).unwrap();
        let m = crate::bytes::MmapBytes::paged(hold.stream(back, Some(view.clone()), "t").unwrap());
        let mut off = 0usize;
        while off < bytes.len() {
            let n = 500.min(bytes.len() - off);
            assert_eq!(m.slice_at(off as u64, n).unwrap(), &bytes[off..off + n]);
            off += n;
        }
        assert_eq!(pager.counts().streamed, 30 - 4, "the cache's four copied");
        assert_eq!(pager.counts().fills, fills, "and nothing filled into it");
    }

    /// A chunk a stream reads that fails its check is the store's failure,
    /// as a fill's is: the read answers nothing and every read of the
    /// store after answers the failure (`Pager::check`).
    #[test]
    fn a_chunk_that_fails_its_check_through_a_stream_poisons_the_store() {
        use crate::bytes::Bytes;
        let dir = scratch("stream-damage");
        let bytes = plain(10 * CHUNK, 7);
        let path = dir.join("e");
        envelope(&path, &bytes, 3);
        let mut env = std::fs::read(&path).unwrap();
        let hdr = Header::parse(&env[..Header::LEN]).unwrap();
        let at = hdr.stored_off(4) as usize + 9;
        env[at] ^= 1;
        std::fs::write(&path, &env).unwrap();
        let pager = Pager::new(u64::MAX);
        let hold = pager.hold();
        let m = stream_of(&hold, &path, 3);
        assert_eq!(m.slice_at(0, 10).unwrap(), &bytes[..10], "a chunk whole");
        assert!(pager.check().is_ok());
        assert!(
            m.slice_at(4 * CHUNK as u64 + 1, 5).is_none(),
            "the chunk changed"
        );
        assert!(pager.check().is_err(), "the store refuses from then on");
    }

    /// A view says what of a range is in memory, here or in the kernel's
    /// cache of the envelope, gives the kernel's up, and asks it back: the
    /// three things the engine asks of a mapped file's pages, asked of the
    /// file a paged mapping stands in front of. Not under qemu-user
    /// (`supdb_emulated`), which answers the ask with success and passes
    /// nothing to the kernel, so the file dropped never comes back there;
    /// the native ARM leg runs it.
    #[cfg(not(supdb_emulated))]
    #[test]
    fn a_view_says_what_is_in_memory_and_drops_and_asks_for_what_stands_behind_it() {
        let dir = scratch("resident");
        let bytes = plain(4 * 1024 * 1024, 3);
        let path = dir.join("e");
        envelope(&path, &bytes, 9);
        // Clean pages are what a drop takes.
        File::open(&path).unwrap().sync_all().unwrap();
        let pager = Pager::new(u64::MAX);
        // The kernel's side of a drop: what a drop gives back of the
        // plaintext held here is the next test's.
        pager.drops_keep(true);
        let p = sparse(&pager, &path, 9);
        let len = bytes.len();
        assert_eq!(p.resident(0, 0), 0.0);
        let just_written = p.resident(0, len);
        assert!(just_written > 0.99, "a file just written: {just_written}");
        p.forget(0, len);
        let dropped = p.resident(0, len);
        assert!(dropped < 0.05, "a forget left {dropped} of the file");
        // A chunk here is in memory whatever the kernel holds of it.
        {
            let _h = pager.hold();
            read(&p, 1_000_000, 10).unwrap();
        }
        p.forget(0, len);
        assert_eq!(p.resident(1_000_000, 10), 1.0, "a chunk present");
        assert_eq!(p.resident(2_000_000, 10), 0.0, "a chunk nobody holds");
        assert_eq!(p.willneed(1_000_000, 10), 0, "nothing asked for it");
        // A part dropped leaves the rest.
        let asked = p.willneed(0, len);
        assert!(asked >= len, "the whole file asked for: {asked}");
        let t = std::time::Instant::now();
        while p.resident(0, len) < 0.99 {
            assert!(t.elapsed().as_secs() < 20, "the file did not come back");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        p.forget(2_000_000, 1_000_000);
        assert!(p.resident(2_100_000, 800_000) < 0.05, "the range dropped");
        assert!(p.resident(0, 1_900_000) > 0.99, "and nothing before it");
        assert!(p.resident(3_100_000, 900_000) > 0.99, "or after it");
        // And as the engine asks, through the mapping a reader holds: the
        // advice a mapped file takes for its own pages reaches the file
        // behind a paged one.
        use crate::bytes::Bytes;
        let m = crate::bytes::MmapBytes::paged(p.clone());
        assert_eq!(m.resident_at(2_500_000), Some(false));
        assert_eq!(m.resident_at(100_000), Some(true));
        assert_eq!(m.resident_at(len as u64), None, "past the end");
        assert!(m.advise_willneed(2_000_000, 1_000_000) >= 1_000_000);
        // The ask is advice and its read is on the kernel's time: the page
        // in the middle of what was asked is back before the last of it, so
        // the whole file's share is waited for as that page is. Read once
        // behind the page alone, the share was short in one run of six.
        let t = std::time::Instant::now();
        while m.resident_at(2_500_000) != Some(true)
            || m.resident_share(0, len).map(|s| s > 0.99) != Some(true)
        {
            assert!(t.elapsed().as_secs() < 20, "the range did not come back");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        m.advise_dontneed_range(2_000_000, 1_000_000);
        assert_eq!(m.resident_at(2_500_000), Some(false));
        m.advise_dontneed();
        assert_eq!(m.resident_at(100_000), Some(false), "the whole file");
        assert_eq!(m.resident_at(1_000_000), Some(true), "but the chunk here");
    }

    /// A drop of a range gives back the units wholly inside it, whatever
    /// the budget, as a plain store's drop gives back its pages: a unit a
    /// range covers in part stays, as does one read after the drop, and
    /// the shape that leaves the plaintext (`StoreCipher::
    /// drops_keep_plaintext`) gives nothing back. The evictor is stopped,
    /// so that each give-back is the test's own call.
    #[test]
    fn a_drop_gives_back_the_units_it_covers_and_not_one_read_after_it() {
        let dir = scratch("dropunits");
        let bytes = plain(4 * 1024 * 1024, 4);
        let path = dir.join("e");
        envelope(&path, &bytes, 9);
        let pager = Pager::new(u64::MAX);
        pager.stop();
        let p = sparse(&pager, &path, 9);
        let len = bytes.len();
        {
            let _h = pager.hold();
            assert_eq!(read(&p, 0, len).as_deref(), Some(&bytes[..]));
        }
        let ub = p.unit_chunks * CHUNK;
        assert_eq!(p.unit.len(), len.div_ceil(ub), "five units, the last short");
        let present = |u: usize| {
            let lo = u * p.unit_chunks;
            let hi = (lo + p.unit_chunks).min(p.state.len());
            p.state[lo..hi]
                .iter()
                .filter(|s| matches!(s.load(Ordering::SeqCst), COLD | HOT))
                .count()
        };
        let whole = |u: usize| ((u + 1) * p.unit_chunks).min(p.state.len()) - u * p.unit_chunks;
        // Units 1 and 2 lie wholly inside; 0 and 3 in part.
        let (off, n) = (ub as u64 - 10, 2 * ub + 20);
        p.forget(off, n);
        assert_eq!(pager.dropped.load(Ordering::SeqCst), 2, "two units asked");
        // A read after the drop keeps its unit.
        {
            let _h = pager.hold();
            let at = 2 * ub + 100;
            assert_eq!(
                read(&p, at as u64, 10).as_deref(),
                Some(&bytes[at..at + 10])
            );
        }
        pager.pass();
        let c = pager.counts();
        assert_eq!(c.dropped_units, 1, "{c:?}");
        assert_eq!(c.dropped_bytes, ub as u64, "{c:?}");
        assert_eq!(present(1), 0, "the unit dropped is given back");
        for u in [0, 2, 3, 4] {
            assert_eq!(present(u), whole(u), "unit {u} stays");
        }
        assert_eq!(pager.held(), (len - ub) as u64, "what is held is counted");
        // Read back, the plaintext is the file's.
        {
            let _h = pager.hold();
            assert_eq!(read(&p, 0, len).as_deref(), Some(&bytes[..]));
        }
        // The whole file: every unit.
        p.forget(0, len);
        assert_eq!(pager.dropped.load(Ordering::SeqCst), 5, "every unit asked");
        pager.pass();
        assert_eq!(pager.held(), 0, "the file given back whole");
        assert_eq!(pager.counts().dropped_units, 6);
        // The shape before: the drop leaves the plaintext here.
        {
            let _h = pager.hold();
            assert_eq!(read(&p, 0, len).as_deref(), Some(&bytes[..]));
        }
        pager.drops_keep(true);
        p.forget(0, len);
        assert_eq!(pager.dropped.load(Ordering::SeqCst), 0, "nothing asked");
        pager.pass();
        assert_eq!(pager.held(), len as u64, "nothing given back");
    }

    /// A paged mapping and a mapped file over the same plaintext say alike
    /// what of a range is in memory, once both are: the one question the
    /// engine asks a source about a span of its pages, asked of either
    /// kind (`Bytes::resident_share`). A paged mapping answered "none" to
    /// the page count a warm of a form file asked, every page of it
    /// present. Not under qemu-user
    /// (`supdb_emulated`), whose ask passes nothing to the kernel.
    #[cfg(not(supdb_emulated))]
    #[test]
    fn a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident() {
        let dir = scratch("alike");
        let bytes = plain(1 << 20, 5);
        let (sealed, flat) = (dir.join("e"), dir.join("f"));
        envelope(&sealed, &bytes, 9);
        std::fs::write(&flat, &bytes).unwrap();
        let pager = Pager::new(u64::MAX);
        let p = sparse(&pager, &sealed, 9);
        let len = bytes.len();
        let _ = p.willneed(0, len);
        let t = std::time::Instant::now();
        while p.resident(0, len) < 1.0 {
            assert!(t.elapsed().as_secs() < 20, "the envelope did not come in");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        use crate::bytes::Bytes;
        let paged = crate::bytes::MmapBytes::paged(p.clone());
        let mapped = crate::bytes::MmapBytes::open(&flat).unwrap();
        let page = crate::bytes::os_page_size();
        for (off, n) in [(0, len), (page, 2 * page), (len / 2 + 7, 100), (len - 1, 1)] {
            assert_eq!(
                paged.resident_share(off as u64, n),
                mapped.resident_share(off as u64, n),
                "{n} bytes at {off}"
            );
            assert_eq!(paged.resident_share(off as u64, n), Some(1.0));
        }
        assert_eq!(
            paged.resident_share(len as u64, 5),
            Some(0.0),
            "past the end"
        );
        assert_eq!(
            mapped.resident_share(len as u64, 5),
            Some(0.0),
            "past the end"
        );
        use std::os::unix::io::AsRawFd;
        // Each out of the cache whole and its first quarter asked back: a
        // range is answered by its own place, whole in or whole out, by
        // either kind. (Whole: the cache holds a file in folios the size of
        // the writes that made it, and a drop of part of one leaves it.)
        for f in [&sealed, &flat] {
            std::fs::File::open(f).unwrap().sync_all().unwrap();
        }
        p.forget(0, len);
        mapped.advise_dontneed();
        let f = std::fs::File::open(&flat).unwrap();
        // SAFETY: a hint on an open descriptor.
        let rc = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        assert_eq!(rc, 0, "fadvise");
        assert_eq!(paged.resident_share(0, len), Some(0.0), "paged: out");
        assert_eq!(mapped.resident_share(0, len), Some(0.0), "mapped: out");
        let _ = p.willneed(0, len / 4);
        mapped.advise_willneed(0, len / 4);
        let t = std::time::Instant::now();
        while paged.resident_share(0, len / 4) < Some(1.0)
            || mapped.resident_share(0, len / 4) < Some(1.0)
        {
            assert!(t.elapsed().as_secs() < 20, "the quarter did not come in");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for (off, n, want) in [
            (0, page, 1.0),
            (page, 2 * page, 1.0),
            (len / 4 * 3, 2 * page, 0.0),
            (len / 4 * 3 + 5000, 1, 0.0),
        ] {
            assert_eq!(
                paged.resident_share(off as u64, n),
                Some(want),
                "paged: {n} at {off}"
            );
            assert_eq!(
                mapped.resident_share(off as u64, n),
                Some(want),
                "mapped: {n} at {off}"
            );
        }
    }

    /// What a populate maps is the ciphertext a fill would fault on
    /// (`Paging::populate`, `Paging::mapped`): an envelope in the kernel's
    /// cache and out of this process's page tables reads as resident and
    /// as not mapped; a populate of a range maps the envelope's pages for
    /// its chunks and decrypts nothing, and maps nothing far from it; a
    /// chunk present here reads as mapped whatever the tables hold of its
    /// ciphertext, and a populate of it asks the kernel for nothing. And
    /// as the engine asks, through the mapping a reader holds. Not under
    /// qemu-user (`supdb_emulated`), whose ask passes nothing to the
    /// kernel.
    #[cfg(all(target_os = "linux", not(supdb_emulated)))]
    #[test]
    fn a_populate_maps_the_ciphertext_of_what_is_not_present_and_decrypts_nothing() {
        let dir = scratch("populate");
        let bytes = plain(8 * 1024 * 1024, 3);
        let path = dir.join("e");
        envelope(&path, &bytes, 9);
        File::open(&path).unwrap().sync_all().unwrap();
        let pager = Pager::new(u64::MAX);
        // The kernel's side of a drop, as the test before.
        pager.drops_keep(true);
        let p = sparse(&pager, &path, 9);
        let len = bytes.len();
        // Out of the tables and the cache, then back into the cache alone.
        p.forget(0, len);
        let _ = p.willneed(0, len);
        let t = std::time::Instant::now();
        while p.resident(0, len) < 0.99 {
            assert!(t.elapsed().as_secs() < 20, "the envelope did not come in");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(p.mapped(0, len), Some(0.0), "resident and not mapped");
        assert_eq!(p.mapped(0, 0), None, "an empty range");
        assert_eq!(p.mapped(len as u64, 5), None, "past the end");
        let fills = pager.counts().fills;
        assert!(p.populate(2_000_000, 1_000_000), "the kernel took it");
        assert_eq!(p.mapped(2_000_000, 1_000_000), Some(1.0), "the range");
        assert_eq!(p.mapped(6_000_000, 2_000_000), Some(0.0), "and nothing far");
        assert_eq!(p.mapped(0, 500_000), Some(0.0), "on either side");
        assert_eq!(pager.counts().fills, fills, "nothing decrypted");
        assert!(!p.populate(0, 0), "an empty range");
        assert!(!p.populate(len as u64, 5), "past the end");
        // A chunk here is reached with no fault whatever the tables hold
        // of its ciphertext, and nothing is asked for it.
        {
            let _h = pager.hold();
            read(&p, 7_000_000, 10).unwrap();
        }
        p.forget(0, len);
        assert_eq!(p.mapped(7_000_000, 10), Some(1.0), "a chunk present");
        assert_eq!(p.mapped(6_500_000, 10), Some(0.0), "a chunk nobody holds");
        assert!(!p.populate(7_000_000, 10), "nothing mapped for it");
        assert_eq!(p.resident(6_500_000, 10), 0.0, "and nothing read back");
        // As the engine asks, through the mapping a reader holds.
        use crate::bytes::Bytes;
        let m = crate::bytes::MmapBytes::paged(p.clone());
        let _ = m.advise_willneed(0, len);
        let t = std::time::Instant::now();
        while m.resident_share(0, len).map(|s| s > 0.99) != Some(true) {
            assert!(t.elapsed().as_secs() < 20, "the envelope did not come back");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(m.mapped_share(3_000_000, 1_000_000), Some(0.0));
        assert!(m.populate_read(3_000_000, 1_000_000));
        assert_eq!(m.mapped_share(3_000_000, 1_000_000), Some(1.0));
        assert_eq!(m.mapped_share(len as u64, 5), None, "past the end");
        assert_eq!(pager.counts().fills, fills + 1, "one chunk read, no other");
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
        // What the budget took is the file's to say, for the trace, and a
        // file that goes hands its count to the cache by the first field
        // of its name. Read with the evictor stopped: a pass goes on past
        // the budget to its margin and holds every file for its length
        // (`Pager::mark`), so beside one the file's count was a unit short
        // of what its drop handed over, fourteen runs of a hundred and
        // fifty with six at once, and the drop itself was the evictor's to
        // finish, the count not there when it was read, once in a gate.
        pager.stop();
        let c = pager.counts();
        assert_eq!(p.given_bytes(), c.evicted_bytes - c.dropped_bytes, "{c:?}");
        assert!(p.given_bytes() > 0);
        assert!(pager.given_gone().is_empty());
        let given = p.given_bytes();
        drop(p);
        assert_eq!(pager.given_gone(), vec![("test".to_string(), given)]);
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

    /// A hold idles for a wait and stands again after it (`Hold::idle`):
    /// through the wait the cache gives back what the holder read before,
    /// the holder is told it idled, once, and from the wait's end the
    /// evictor waits for it as it did. A hold left at nothing after its
    /// wait read on for a walk's next rest with nothing waited for on its
    /// account, which no test of a merge caught: a unit just read is
    /// passed over once by the evictor's hand.
    #[test]
    fn a_hold_idles_for_a_wait_and_stands_again_after_it() {
        let dir = scratch("idle");
        let bytes = plain(8 * 1024 * 1024, 5);
        envelope(&dir.join("e"), &bytes, 14);
        let unit = (unit_chunks() * CHUNK) as u64;
        let pager = Pager::new(unit);
        let p = sparse(&pager, &dir.join("e"), 14);
        let h = pager.hold();
        // Far past the budget, under the hold: nothing goes.
        assert_eq!(read(&p, 0, bytes.len()).unwrap(), bytes);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(pager.counts().evicted_units, 0);
        assert!(!h.took_idle(), "a hold that has not idled");
        // Through the wait the hold stands for nothing, and the cache
        // comes back to its budget.
        let (at, held) = h.idle(|| {
            let t = std::time::Instant::now();
            while pager.held() > pager.budget() && t.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
            }
            (h.at.load(Ordering::SeqCst), pager.held())
        });
        assert_eq!(at, 0, "the hold through its wait");
        assert!(held <= pager.budget(), "{:?}", pager.counts());
        assert!(h.took_idle(), "the holder is told");
        assert!(!h.took_idle(), "once");
        // From the wait's end it holds again: at the era as it stands,
        // and what it reads now is not given back under it.
        assert_eq!(
            h.at.load(Ordering::SeqCst),
            pager.era.load(Ordering::SeqCst),
            "the hold after its wait"
        );
        let before = pager.counts().evicted_units;
        assert_eq!(read(&p, 0, bytes.len()).unwrap(), bytes);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            pager.counts().evicted_units,
            before,
            "a unit went while its reader held again"
        );
        // SAFETY: lent above and held since.
        let lent = unsafe { std::slice::from_raw_parts(p.base, bytes.len()) };
        assert_eq!(lent, &bytes[..], "the bytes lent are still the bytes");
        drop(h);
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

    /// A room a test moves: what it says, whether it says anything, and
    /// what the process has claimed.
    struct TestRoom {
        room: Arc<AtomicU64>,
        quiet: Arc<AtomicBool>,
        claimed: Arc<AtomicU64>,
    }

    impl Room for TestRoom {
        fn read(&self, _held: u64) -> Option<u64> {
            (!self.quiet.load(Ordering::SeqCst)).then(|| self.room.load(Ordering::SeqCst))
        }
        fn floor(&self) -> u64 {
            0
        }
        fn claimed(&self) -> u64 {
            self.claimed.load(Ordering::SeqCst)
        }
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let t = std::time::Instant::now();
        while !done() {
            assert!(t.elapsed() < Duration::from_secs(10), "never: {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A cache that follows its room holds what the room says as the room
    /// moves, with nobody reading: it gives back when the room falls,
    /// holds more when it rises, and between two readings gives way at
    /// once to what the process claims, which a reading sees only once the
    /// memory is touched. A fall of the room is no overshoot of the
    /// cache's: once it has given back, its budget is the room. Its lines
    /// stand inside the room's margin, where a fixed budget's are shares
    /// of it.
    #[test]
    fn a_cache_that_follows_its_room_gives_way_as_the_room_does() {
        let dir = scratch("room");
        let bytes = plain(24 * 1024 * 1024, 7);
        envelope(&dir.join("e"), &bytes, 3);
        let unit = (unit_chunks() * CHUNK) as u64;
        let room = Arc::new(AtomicU64::new(16 * unit));
        let quiet = Arc::new(AtomicBool::new(false));
        let claimed = Arc::new(AtomicU64::new(0));
        let pager = Pager::following(Box::new(TestRoom {
            room: room.clone(),
            quiet: quiet.clone(),
            claimed: claimed.clone(),
        }));
        assert_eq!(pager.budget(), 16 * unit, "the room as the cache is made");
        let p = sparse(&pager, &dir.join("e"), 3);
        let read_through = || {
            let step = 64 * 1024;
            let mut off = 0usize;
            while off < bytes.len() {
                let _h = pager.hold();
                let len = step.min(bytes.len() - off);
                assert_eq!(
                    read(&p, off as u64, len).unwrap(),
                    &bytes[off..off + len],
                    "at {off}"
                );
                off += len;
            }
        };
        // Settled: under its budget, and whatever it overshot while it was
        // read through forgotten, so the budget is the room.
        let settled = |at: u64| {
            until("the budget at its room", || pager.budget() == at);
            until("the cache under its room", || pager.held() <= at);
        };
        read_through();
        settled(16 * unit);
        assert!(
            pager.held() > 8 * unit,
            "premise: more held than the room is about to be: {}",
            pager.held()
        );
        // The room falls: the cache gives back to it, nobody having read,
        // and what it held past the new budget meanwhile was no overshoot.
        let before = pager.counts();
        room.store(8 * unit, Ordering::SeqCst);
        settled(8 * unit);
        let after = pager.counts();
        assert!(after.evicted_units > before.evicted_units);
        assert_eq!(after.spared, 0, "a fall of the room kept back: {after:?}");
        // What the process claims comes off at once and stays off until
        // the platform is read again.
        quiet.store(true, Ordering::SeqCst);
        std::thread::sleep(2 * ROOM_SELDOM);
        claimed.fetch_add(3 * unit, Ordering::SeqCst);
        until("the budget less what was claimed", || {
            pager.budget() == 5 * unit
        });
        until("the cache under it", || pager.held() <= 5 * unit);
        assert!(pager.counts().gave_way >= 1, "{:?}", pager.counts());
        std::thread::sleep(2 * ROOM_SELDOM);
        assert_eq!(
            pager.budget(),
            5 * unit,
            "nothing said by the platform, nothing given back to the cache"
        );
        // A claim that falls is nothing given back either: what was
        // unmapped is the platform's to say.
        claimed.fetch_sub(3 * unit, Ordering::SeqCst);
        std::thread::sleep(2 * ROOM_SELDOM);
        assert_eq!(pager.budget(), 5 * unit);
        // The room rises and the platform says so: the cache holds more.
        room.store(20 * unit, Ordering::SeqCst);
        quiet.store(false, Ordering::SeqCst);
        until("the budget up with the room", || pager.budget() > 12 * unit);
        read_through();
        settled(20 * unit);
        assert!(pager.held() > 12 * unit, "held {}", pager.held());
        assert!(pager.counts().rooms >= 3, "{:?}", pager.counts());
        // The lines: inside the margin for a cache whose budget is the
        // room, shares of the budget for one whose budget is fixed.
        room.store(1 << 34, Ordering::SeqCst);
        until("the lines of a room of sixteen gibibytes", || {
            let b = pager.budget();
            b == 1 << 34
                && pager.hard.load(Ordering::Relaxed) == b + ROOM_HARD.max(4 * unit)
                && pager.swamp.load(Ordering::Relaxed) == b + ROOM_SWAMP.max(6 * unit)
        });
        let fixed = Pager::new(1 << 34);
        assert_eq!(fixed.hard.load(Ordering::Relaxed), (1u64 << 34) + (1 << 30));
        assert_eq!(
            fixed.swamp.load(Ordering::Relaxed),
            (1u64 << 34) + (1 << 33)
        );
        fixed.stop();
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whether the live mapping of `path` this process holds is advised
    /// for reads far apart, by the kernel's own word (`rr` in its
    /// `VmFlags`); `None` where nothing maps it.
    #[cfg(target_os = "linux")]
    fn mapped_random(path: &std::path::Path) -> Option<bool> {
        let want = path.to_string_lossy().into_owned();
        let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
        let mut ours = false;
        for line in smaps.lines() {
            if let Some(flags) = line.strip_prefix("VmFlags:") {
                if ours {
                    return Some(flags.split_whitespace().any(|f| f == "rr"));
                }
            } else if line.split_whitespace().nth(5) == Some(want.as_str()) {
                ours = true;
            }
        }
        None
    }

    /// An advice given a view before it has its envelope is the
    /// envelope's when it comes (`Paged::advise`, `Paged::back`): an
    /// envelope opens advised for reads far apart (`Back::open`), and a
    /// segment a seal lands readable as a memory file is advised to be
    /// read in order wherever its store is in a scan's phase, before the
    /// seal has written its envelope -- left random, the plain store's
    /// default being to read around a fault. And an advice given after the
    /// envelope is the envelope's at once, both ways.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_advice_given_before_the_envelope_is_the_envelopes_when_it_comes() {
        let dir = scratch("advice-before");
        let bytes = plain(8 * CHUNK, 5);
        let pager = Pager::new(u64::MAX);
        for (before, random) in [(Some(false), false), (Some(true), true), (None, true)] {
            let path = dir.join(format!("e-{before:?}"));
            let mut mem = memfile().unwrap();
            mem.write_all(&bytes).unwrap();
            let p = Paged::adopt(&pager, &mem, bytes.len() as u64, "file").unwrap();
            drop(mem);
            if let Some(r) = before {
                p.advise(r);
            }
            envelope(&path, &bytes, 31);
            p.back(Back::open(&path, Box::new(Toy(31))).unwrap())
                .unwrap();
            assert_eq!(
                mapped_random(&path),
                Some(random),
                "advised {before:?} before the envelope"
            );
            p.advise(!random);
            assert_eq!(mapped_random(&path), Some(!random), "advised after it");
            drop(p);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a following cache holds past its budget and cannot give back
    /// comes off the room before the room is its budget: a file with no
    /// envelope yet is held whole, and a budget that is all of the room
    /// would leave it past the limit the room was read from. Kept back by
    /// what it is and no more -- the budget's own fall is not counted
    /// again as overshoot, which would walk the budget to nothing -- and
    /// forgotten a span after the file has its envelope.
    #[test]
    fn a_following_cache_keeps_back_what_it_overshot() {
        let dir = scratch("spare");
        let unit = (unit_chunks() * CHUNK) as u64;
        let bytes = plain(12 * unit as usize, 4);
        let room = Arc::new(AtomicU64::new(8 * unit));
        let pager = Pager::following(Box::new(TestRoom {
            room: room.clone(),
            quiet: Arc::new(AtomicBool::new(false)),
            claimed: Arc::new(AtomicU64::new(0)),
        }));
        assert_eq!(pager.budget(), 8 * unit);
        let mut mem = memfile().unwrap();
        mem.write_all(&bytes).unwrap();
        let p = Paged::adopt(&pager, &mem, bytes.len() as u64, "unbacked").unwrap();
        drop(mem);
        assert_eq!(pager.held(), 12 * unit);
        // Four units past the room, none of it the cache's to give back.
        until("the overshoot kept back", || pager.budget() == 4 * unit);
        for _ in 0..10 {
            std::thread::sleep(ROOM_SELDOM / 2);
            assert_eq!(
                pager.budget(),
                4 * unit,
                "the room less what was overshot, and no less: {:?}",
                pager.counts()
            );
        }
        let c = pager.counts();
        assert_eq!((c.spared, c.evicted_units), (4 * unit, 0), "{c:?}");
        // With its envelope the file is given back, and the overshoot is
        // forgotten: the budget is the room again.
        envelope(&dir.join("e"), &bytes, 23);
        p.back(Back::open(&dir.join("e"), Box::new(Toy(23))).unwrap())
            .unwrap();
        until("the cache under its budget", || {
            pager.held() <= pager.budget()
        });
        until("the budget at its room", || pager.budget() == 8 * unit);
        assert_eq!(pager.counts().spared, 0);
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An ask that lies across chunks asks the kernel for the ciphertext
    /// of the ones that are gone in one piece where two or more are, and
    /// for nothing otherwise: an ask inside one chunk, an ask with one
    /// chunk gone, an ask whose chunks are all here, and any ask of a
    /// store that keeps its chunks apart. And what is read is the
    /// plaintext, whichever way the ciphertext came.
    #[test]
    fn an_ask_of_several_chunks_asks_for_their_ciphertext_together() {
        let dir = scratch("together");
        let bytes = plain(1024 * 1024, 5);
        envelope(&dir.join("e"), &bytes, 31);
        let pager = Pager::new(u64::MAX);
        let p = sparse(&pager, &dir.join("e"), 31);
        let _h = pager.hold();
        let together = || pager.counts().together;
        let at = |chunk: usize, back: usize| (chunk * CHUNK - back) as u64;
        let same = |off: u64, len: usize| {
            let got = read(&p, off, len).expect("a range of the file");
            assert_eq!(got, &bytes[off as usize..off as usize + len], "at {off}");
        };
        // Inside one chunk: its own fault is the whole of its read.
        same(10, 100);
        assert_eq!(together(), 0, "an ask inside one chunk");
        // Across two chunks, the first of them here.
        same(at(1, 50), 100);
        assert_eq!(together(), 0, "an ask with one chunk gone");
        // Across two chunks, both gone.
        same(at(4, 50), 100);
        assert_eq!(together(), 1, "an ask with two chunks gone");
        // Again: both are here, and nothing is asked of the kernel.
        same(at(4, 50), 100);
        assert_eq!(together(), 1, "an ask whose chunks are here");
        // Three chunks, the middle one here: the two gone are a span.
        same(at(11, 0), 10);
        assert_eq!(p.gone_span(10, 12), Some((10, 12)));
        same(at(10, 0), 3 * CHUNK);
        assert_eq!(together(), 2, "two gone about one that is here");
        assert_eq!(p.gone_span(10, 12), None, "none gone");
        // Three chunks, the middle one gone: no span, and nothing asked.
        same(at(20, 0), 10);
        same(at(22, 0), 10);
        assert_eq!(p.gone_span(20, 22), None, "one gone");
        same(at(20, 0), 3 * CHUNK);
        assert_eq!(together(), 2, "one gone between two that are here");
        // Kept apart, an ask with two gone asks for nothing together.
        pager.keep_apart(true);
        same(at(31, 50), 100);
        assert_eq!(together(), 2, "kept apart");
        pager.keep_apart(false);
        same(at(41, 50), 100);
        assert_eq!(together(), 3, "and together again");
        drop(p);
        pager.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A chunk is not lent out of a unit marked again under the rescue
    /// that lent it. A reader that finds its chunk dying takes the unit
    /// back and then says the chunk is lent, two steps, and a thread can
    /// lose its core between them for long enough that the evictor ends
    /// the pass the rescue was from and marks the unit in its next: the
    /// chunk then said to be lent stood in a unit about to be given back,
    /// and the next reader, pinned after that mark and so not waited for,
    /// took it on the word and read pages as they went. The interleaving
    /// by hand: a mark, a reader held after its rescue, the first pass's
    /// end, a second mark, the reader let go. The reader takes the unit
    /// back from the second mark too, so nothing of it is given back
    /// while its chunk is lent.
    #[test]
    fn a_chunk_is_not_lent_out_of_a_unit_marked_again_under_its_rescue() {
        let dir = scratch("remark");
        let bytes = plain(4 * 1024 * 1024, 9);
        envelope(&dir.join("e"), &bytes, 23);
        // A budget the file is far inside: the evictor marks nothing of
        // its own, and the marks here are the test's.
        let pager = Pager::new(1 << 40);
        let p = sparse(&pager, &dir.join("e"), 23);
        let h = pager.hold();
        assert!(p.ensure(0, 100));
        // A first look finds the chunk read and passes the unit over; the
        // second marks it.
        let t = std::time::Instant::now();
        while p.mark(0) == 0 {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "the unit is never marked"
            );
        }
        assert_eq!(p.unit[0].load(Ordering::SeqCst), EVICTING);
        assert_eq!(p.state[0].load(Ordering::SeqCst), DYING);
        p.pause.store(1, Ordering::SeqCst);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let _h = pager.hold();
                p.ensure(0, 100)
            });
            let t = std::time::Instant::now();
            while p.pause.load(Ordering::SeqCst) != 2 {
                assert!(
                    t.elapsed() < Duration::from_secs(10),
                    "the reader never took the unit back"
                );
                std::thread::yield_now();
            }
            // Taken back: the first pass gives nothing of it back.
            assert_eq!(p.unit[0].load(Ordering::SeqCst), LIVE);
            assert_eq!(p.free(0), 0, "a unit taken back was given back");
            // Marked again, its chunk dying still, as the reader left it.
            assert!(p.mark(0) > 0, "the unit was not marked a second time");
            assert_eq!(p.unit[0].load(Ordering::SeqCst), EVICTING);
            p.pause.store(0, Ordering::SeqCst);
            assert!(reader.join().unwrap());
        });
        assert_eq!(p.state[0].load(Ordering::SeqCst), HOT, "the chunk is lent");
        assert_eq!(
            p.unit[0].load(Ordering::SeqCst),
            LIVE,
            "a chunk is lent in a unit marked to go"
        );
        assert_eq!(p.free(0), 0, "a unit with a chunk lent was given back");
        assert_eq!(read(&p, 0, 100).unwrap(), &bytes[..100]);
        drop(h);
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
