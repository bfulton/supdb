//! Where the engine's large blocks come from: a memtable's arena blocks,
//! its entry slabs and its hash index, and a scan snapshot's arena blocks.
//!
//! A table's or a snapshot's block of `MAP_FROM` bytes or more is
//! mapped from the kernel (`mmap`, anonymous and private) and unmapped at
//! its drop, so what a store frees goes back at once. From the global
//! allocator instead, a freed block stays in the allocator's heap for the
//! next allocation of its size: glibc raises its mmap threshold to the
//! size of the first large chunk it frees, so every arena block after the
//! first came from the heap and stayed there when its table dropped, and
//! across the lag sweep at ten million keys the process held 150-620 MB
//! beyond the bytes in use -- page cache the store cannot have under a
//! memory bound, and memory the compact form's budget subtracts -- where
//! glibc told to map every allocation of a megabyte or more held 0-135.
//! A mapped block's pages are fresh at their first touch, where a heap's
//! retained pages were touched before; a block below `MAP_FROM` comes from
//! the allocator whatever the source, since a mapping costs a system call
//! and a fault a page that a small block does not earn.
//!
//! The bytes mapped and not yet unmapped are counted (`mapped_bytes`): the
//! leak test holds the count to zero past every store's close, and the
//! suite prints them beside the allocator's bytes in use. The blocks alive
//! are kept by address as well (`LIVE`), so the resident count beyond the
//! allocator's use can take out what of them the kernel has resident
//! (`mapped_resident_bytes`, by `mincore`) and not the blocks whole: a
//! mapped page never touched is resident nowhere, and a store's tables in
//! flight map tens of megabytes they have barely written -- subtracted
//! whole, they read a dead arena of thirty-two megabytes as seven, and the
//! trim that counts by the resident bytes never fired on it.

// The door (scripts/door/clippy.toml, tests/data/door.txt): the engine's anonymous blocks.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A block of at least this many bytes is mapped where its source maps;
/// glibc's own threshold starts here, before its first large free raises
/// it.
pub(crate) const MAP_FROM: usize = 128 << 10;

/// Every block's alignment, mapped or not: a cache line.
const ALIGN: usize = 64;

static MAPPED: AtomicUsize = AtomicUsize::new(0);

/// Which of a job's arrays a `BigVec` is: the count of those that became
/// mappings is kept by it, so a test holds `Options::jobs_mapped` to
/// having reached each array it names and not only some.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Site {
    /// A seal's sort of its table: the records it sorts, the sort's
    /// scratch, and the order it answers.
    SortRecs,
    SortScratch,
    SortOrder,
    /// A segment writer's copy of every key, the keys' spans in it, the
    /// records' offsets and the keys' hashes.
    WriterKeys,
    WriterSpans,
    WriterOffs,
    WriterHashes,
    /// The trailer a finish composes: the directory and the hash slots.
    Trailer,
    /// An ordered index builder's copy of the keys and where each starts.
    IndexKeys,
    IndexStarts,
    /// A merge's key list: the keys cut short and where each starts.
    ListKeys,
    ListOffs,
    /// A unit test's.
    #[cfg(test)]
    Test,
}

impl Site {
    /// The sites a store's jobs have, in the order `big_maps` answers.
    pub(crate) const ALL: [(Site, &'static str); 12] = [
        (Site::SortRecs, "sort records"),
        (Site::SortScratch, "sort scratch"),
        (Site::SortOrder, "sort order"),
        (Site::WriterKeys, "writer keys"),
        (Site::WriterSpans, "writer spans"),
        (Site::WriterOffs, "writer offsets"),
        (Site::WriterHashes, "writer hashes"),
        (Site::Trailer, "trailer"),
        (Site::IndexKeys, "index keys"),
        (Site::IndexStarts, "index starts"),
        (Site::ListKeys, "list keys"),
        (Site::ListOffs, "list offsets"),
    ];
}

/// The `BigVec` buffers that became mappings, by site, process-wide,
/// ever: what a test holds `Options::jobs_mapped` to having done.
static BIG_MAPS: [AtomicUsize; 13] = [const { AtomicUsize::new(0) }; 13];

/// The count of `BigVec` buffers that have become mappings, by site, as
/// `Site::ALL` names them.
pub(crate) fn big_maps() -> Vec<(&'static str, usize)> {
    Site::ALL
        .iter()
        .map(|&(s, name)| (name, BIG_MAPS[s as usize].load(Ordering::Relaxed)))
        .collect()
}

/// The mapped blocks alive, each by its address and length, for the
/// resident count. A block comes and goes once per megabytes written, so
/// the lock is nobody's wait.
static LIVE: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn live() -> std::sync::MutexGuard<'static, Vec<(usize, usize)>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The bytes of the blocks mapped and not yet unmapped, process-wide.
pub(crate) fn mapped_bytes() -> usize {
    MAPPED.load(Ordering::Relaxed)
}

/// The bytes of the mapped blocks alive that the kernel has resident, in
/// whole pages, read from the kernel for each block (`resident_of`): what
/// the resident count beyond the allocator's use takes out, since these
/// are anonymous bytes the allocator knows nothing of. A block that goes
/// between the listing and its read counts nothing.
#[cfg(target_os = "linux")]
pub(crate) fn mapped_resident_bytes() -> usize {
    let blocks: Vec<(usize, usize)> = live().clone();
    blocks
        .iter()
        .map(|&(p, len)| resident_of(p as *mut u8, len))
        .sum()
}

/// Where the kernel cannot be asked, every mapped byte is taken as
/// resident, which is the count before this one.
#[cfg(not(target_os = "linux"))]
pub(crate) fn mapped_resident_bytes() -> usize {
    mapped_bytes()
}

/// The bytes of the `len` at `p` that are resident, in whole pages, by
/// `mincore`; none where the range is not a mapping any more.
#[cfg(target_os = "linux")]
pub(crate) fn resident_of(p: *mut u8, len: usize) -> usize {
    // SAFETY: a constant's read.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as usize;
    let pages = len.div_ceil(page);
    let mut vec = vec![0u8; pages];
    // SAFETY: `mincore` reads the mapping's state and writes one byte a
    // page into `vec`, which has a byte for every page of `len`; a range
    // that is no mapping fails and writes nothing.
    let r = unsafe { libc::mincore(p as *mut libc::c_void, len, vec.as_mut_ptr()) };
    if r != 0 {
        return 0;
    }
    vec.iter().filter(|&&b| b & 1 != 0).count() * page
}

/// Whether a block of `len` bytes is mapped under `mapped`: the one rule
/// `alloc` and `free` share, so a block is freed the way it was made.
#[inline]
fn maps(len: usize, mapped: bool) -> bool {
    mapped && len >= MAP_FROM
}

/// `len` bytes, aligned to a cache line, zeroed where `zeroed` (a mapping
/// is zero already), from a mapping where `maps(len, mapped)` and from the
/// global allocator otherwise. Never null: a store out of memory is not a
/// store that can go on. Freed by `free` with the same `len` and `mapped`.
pub(crate) fn alloc(len: usize, zeroed: bool, mapped: bool) -> *mut u8 {
    assert!(len > 0, "a block of no bytes");
    if maps(len, mapped) {
        let p = map(len);
        MAPPED.fetch_add(len, Ordering::Relaxed);
        live().push((p as usize, len));
        return p;
    }
    let layout = std::alloc::Layout::from_size_align(len, ALIGN).expect("block layout");
    // SAFETY: a non-zero layout.
    let p = unsafe {
        if zeroed {
            std::alloc::alloc_zeroed(layout)
        } else {
            std::alloc::alloc(layout)
        }
    };
    assert!(!p.is_null(), "block: out of memory");
    p
}

/// The pages of the `len` bytes at `p`, a fresh mapping, populated for
/// writing: each mapped to a page of its own, so that a load before the
/// first store finds the page and not the kernel's shared zero page,
/// whose replacement at that store is a copy-on-write fault and a flush of
/// the TLB on every CPU the process runs on, an IPI beside a thread on
/// another core. `MADV_POPULATE_WRITE` does it in one call where the
/// kernel has it, and a store of a zero on each page does it where not;
/// the bytes stay zero either way.
///
/// # Safety
/// `[p, p + len)` is a writable mapping of zeroed bytes nobody else reads
/// or writes yet.
pub(crate) unsafe fn populate_write(p: *mut u8, len: usize) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: the caller's range; the advice changes no byte's value.
        let r = unsafe { libc::madvise(p as *mut libc::c_void, len, libc::MADV_POPULATE_WRITE) };
        if r == 0 {
            return;
        }
    }
    let mut off = 0;
    while off < len {
        // SAFETY: inside the caller's range, a zero over a zero.
        unsafe { std::ptr::write_volatile(p.add(off), 0) };
        off += 4096;
    }
}

/// A block from `alloc(len, _, mapped)` given back.
///
/// # Safety
/// `p` came from `alloc` with this `len` and `mapped`, and nothing reads
/// it after.
pub(crate) unsafe fn free(p: *mut u8, len: usize, mapped: bool) {
    if maps(len, mapped) {
        {
            let mut l = live();
            if let Some(i) = l.iter().position(|&(a, n)| a == p as usize && n == len) {
                l.swap_remove(i);
            }
        }
        // SAFETY: mapped by `map` with this length.
        unsafe { unmap(p, len) };
        MAPPED.fetch_sub(len, Ordering::Relaxed);
        return;
    }
    let layout = std::alloc::Layout::from_size_align(len, ALIGN).expect("block layout");
    // SAFETY: allocated by `alloc` with this layout.
    unsafe { std::alloc::dealloc(p, layout) };
}

/// Where a structure's blocks come from and go back to: mapped from the
/// kernel or not, and for a mapped block the
/// store's pool first, where it has one. A
/// table's arenas, slabs and index each keep one, so a block goes back to
/// the pool it could have come from, whichever thread drops the table.
#[derive(Clone)]
pub(crate) struct Source {
    pub(crate) mapped: bool,
    pool: Option<std::sync::Arc<BlockPool>>,
    /// A zeroed block mapped fresh is populated for writing before it is
    /// handed out (`Options::index_populated`, `populate_write`).
    populate: bool,
    /// A mapped block of `HUGE` bytes or more is mapped on huge pages
    /// (`map_huge`, `Options::shared_huge`), and neither taken from the
    /// pool nor given to it, whose blocks of its length are not.
    huge: bool,
}

impl Source {
    pub(crate) fn new(mapped: bool, pool: Option<std::sync::Arc<BlockPool>>) -> Source {
        Source {
            mapped,
            pool,
            populate: false,
            huge: false,
        }
    }

    /// This source with its mapped blocks of `HUGE` bytes or more mapped on
    /// huge pages, or not (`Options::shared_huge`).
    pub(crate) fn huge(mut self, on: bool) -> Source {
        self.huge = on;
        self
    }

    /// Whether a block of `len` bytes from this source is mapped on huge
    /// pages.
    fn maps_huge(&self, len: usize) -> bool {
        self.huge && maps(len, self.mapped) && len >= HUGE
    }

    /// This source with its zeroed mapped blocks populated for writing or
    /// not (`Options::index_populated`); a clone keeps the setting, so an
    /// index's next doubling takes its source's.
    pub(crate) fn populated(mut self, on: bool) -> Source {
        self.populate = on;
        self
    }

    /// Whether this source populates its zeroed mapped blocks.
    #[cfg(test)]
    pub(crate) fn populates(&self) -> bool {
        self.populate
    }

    /// `alloc(len, zeroed, mapped)`, a pooled block of the length first:
    /// its pages touched already, and zeroed here where asked, since a
    /// pooled block holds what its last table wrote.
    pub(crate) fn alloc(&self, len: usize, zeroed: bool) -> *mut u8 {
        if self.maps_huge(len) {
            let p = alloc_huge(len);
            if zeroed && self.populate {
                // SAFETY: the `len` bytes just mapped, nobody's yet.
                unsafe { populate_write(p, len) };
            }
            return p;
        }
        if maps(len, self.mapped) {
            if let Some(pool) = &self.pool {
                if let Some(p) = pool.take(len) {
                    if zeroed {
                        // SAFETY: a block of `len` bytes the pool held, no
                        // reader's since it was given back.
                        unsafe { std::ptr::write_bytes(p, 0, len) };
                    }
                    return p;
                }
                pool.missed(len);
            }
            let p = alloc(len, zeroed, true);
            if zeroed && self.populate {
                // SAFETY: the `len` bytes just mapped, nobody's yet.
                unsafe { populate_write(p, len) };
            }
            return p;
        }
        alloc(len, zeroed, self.mapped)
    }

    /// `free(p, len, mapped)`, to the pool where it has room.
    ///
    /// # Safety
    /// As `free`: `p` came from this source's `alloc` with this `len`, and
    /// nothing reads it after.
    pub(crate) unsafe fn free(&self, p: *mut u8, len: usize) {
        if maps(len, self.mapped) && !self.maps_huge(len) {
            if let Some(pool) = &self.pool {
                if pool.give(p, len) {
                    return;
                }
            }
        }
        // SAFETY: the caller's.
        unsafe { free(p, len, self.mapped) };
    }
}

/// A store's mapped blocks given back by the tables it dropped, kept for
/// its next tables' blocks of the same lengths instead of unmapped.
/// A table's blocks are mapped fresh, and
/// every page of them is a fault at its first touch -- a microsecond and
/// more on this guest, half of an update's time where the writer faulted
/// once every dozen updates -- and the table after it asks for the same
/// lengths: arena blocks of four megabytes, slab blocks, an index that
/// doubles from a thousand slots. The pool keeps at most `cap` bytes, the
/// rest unmapped as before, and none while it is off (`set_on`): what it
/// holds is resident memory the store's other tenants, the page cache's
/// among them, cannot have, so the store turns it off out of core. Its
/// blocks stay mapped and counted (`mapped_bytes`, `LIVE`), and go back to
/// the kernel when it is turned off or dropped, which is the last table's
/// drop or the store's, whichever is later.
pub(crate) struct BlockPool {
    held: Mutex<Vec<(usize, usize)>>,
    bytes: AtomicUsize,
    cap: usize,
    on: std::sync::atomic::AtomicBool,
    /// Blocks given out of the pool, and given to it, over its life.
    takes: AtomicUsize,
    gives: AtomicUsize,
    /// Blocks its tables asked for that it held none of the length of, so
    /// mapped fresh, and their bytes, over its life: the blocks whose
    /// pages are each a fault at the first touch.
    misses: AtomicUsize,
    missed_bytes: AtomicUsize,
}

impl BlockPool {
    pub(crate) fn new(cap: usize, on: bool) -> BlockPool {
        BlockPool {
            held: Mutex::new(Vec::new()),
            bytes: AtomicUsize::new(0),
            cap,
            on: std::sync::atomic::AtomicBool::new(on),
            takes: AtomicUsize::new(0),
            gives: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
            missed_bytes: AtomicUsize::new(0),
        }
    }

    /// A block of `len` bytes asked for and not held, mapped fresh.
    fn missed(&self, len: usize) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.missed_bytes.fetch_add(len, Ordering::Relaxed);
    }

    /// The blocks asked for that the pool held none of the length of, and
    /// their bytes, over its life.
    pub(crate) fn misses(&self) -> (usize, usize) {
        (
            self.misses.load(Ordering::Relaxed),
            self.missed_bytes.load(Ordering::Relaxed),
        )
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Vec<(usize, usize)>> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A held block of exactly `len` bytes, the most recently given first,
    /// whose pages are the likeliest still in the core's caches.
    fn take(&self, len: usize) -> Option<*mut u8> {
        let mut held = self.held();
        let i = held.iter().rposition(|&(_, n)| n == len)?;
        let (p, _) = held.remove(i);
        self.bytes.fetch_sub(len, Ordering::Relaxed);
        self.takes.fetch_add(1, Ordering::Relaxed);
        Some(p as *mut u8)
    }

    /// `p`, a mapped block of `len` bytes, kept where the pool is on and
    /// has room for it; false, and nothing kept, otherwise.
    fn give(&self, p: *mut u8, len: usize) -> bool {
        if !self.on.load(Ordering::Relaxed) {
            return false;
        }
        let mut held = self.held();
        if self.bytes.load(Ordering::Relaxed) + len > self.cap {
            return false;
        }
        held.push((p as usize, len));
        self.bytes.fetch_add(len, Ordering::Relaxed);
        self.gives.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The pool on or off: off, it keeps nothing more and gives back to
    /// the kernel what it holds.
    pub(crate) fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
        if !on {
            self.release();
        }
    }

    /// Whether the pool keeps blocks now.
    pub(crate) fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Every held block unmapped.
    fn release(&self) {
        let blocks: Vec<(usize, usize)> = std::mem::take(&mut *self.held());
        for (p, len) in blocks {
            self.bytes.fetch_sub(len, Ordering::Relaxed);
            // SAFETY: a block a source mapped with this length and gave
            // here, read by nobody since.
            unsafe { free(p as *mut u8, len, true) };
        }
    }

    /// The bytes held, and the blocks given out and given back over the
    /// pool's life.
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (
            self.bytes.load(Ordering::Relaxed),
            self.takes.load(Ordering::Relaxed),
            self.gives.load(Ordering::Relaxed),
        )
    }
}

impl Drop for BlockPool {
    fn drop(&mut self) {
        self.release();
    }
}

// SAFETY: the held blocks are addresses, owned by the pool and handed to
// one owner at a time under its lock.
unsafe impl Send for BlockPool {}
unsafe impl Sync for BlockPool {}

/// A block from `alloc(old, _, mapped)` made `new` bytes long, its first
/// `used` bytes kept and the rest unwritten: a mapping moved by the kernel
/// where both lengths are mappings (`mremap`, the pages changing address
/// and never copied), and otherwise a new block, the bytes copied and the
/// old one freed. Never null, as `alloc` is.
///
/// # Safety
/// `p` came from `alloc` or `regrow` with `old` and this `mapped`, `used`
/// is no more than `old`, and nothing reads `p` after.
pub(crate) unsafe fn regrow(
    p: *mut u8,
    old: usize,
    new: usize,
    used: usize,
    mapped: bool,
) -> *mut u8 {
    debug_assert!(new > old && used <= old, "a block grown");
    #[cfg(target_os = "linux")]
    {
        if maps(old, mapped) && maps(new, mapped) {
            // SAFETY: `p` is a mapping of `old` bytes made by `map` or by
            // this; the kernel moves it where it cannot grow in place.
            let q = unsafe { libc::mremap(p as *mut libc::c_void, old, new, libc::MREMAP_MAYMOVE) };
            assert!(q != libc::MAP_FAILED, "block: out of memory (mremap)");
            {
                let mut l = live();
                if let Some(i) = l.iter().position(|&(a, n)| a == p as usize && n == old) {
                    l[i] = (q as usize, new);
                }
            }
            MAPPED.fetch_add(new - old, Ordering::Relaxed);
            return q as *mut u8;
        }
    }
    let q = alloc(new, false, mapped);
    // SAFETY: `used` bytes of `p` are the caller's to read, `q` is `new`
    // bytes just allocated and apart from `p`, and `p` is freed as it was
    // made.
    unsafe {
        std::ptr::copy_nonoverlapping(p, q, used);
        free(p, old, mapped);
    }
    q
}

/// Where a job's large array gets its buffer once the buffer is large
/// (`BigVec`): from the allocator; from a mapping of its own, unmapped at
/// its drop (`Options::jobs_mapped`); or from a mapping taken from the
/// pool and given back to it (`Options::jobs_pooled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Maps {
    Heap,
    Fresh,
    Pooled,
}

impl Maps {
    /// The mode two options name: mapped at all, and pooled where mapped.
    pub(crate) fn of(mapped: bool, pooled: bool) -> Maps {
        match (mapped, pooled) {
            (false, _) => Maps::Heap,
            (true, false) => Maps::Fresh,
            (true, true) => Maps::Pooled,
        }
    }
}

impl From<bool> for Maps {
    /// A mapping of its own where `mapped`, the allocator's otherwise.
    fn from(mapped: bool) -> Maps {
        Maps::of(mapped, false)
    }
}

/// The longest buffer the pool keeps: glibc's own largest threshold for
/// serving a chunk from a mapping of its own, unmapped at its free
/// (`DEFAULT_MMAP_THRESHOLD_MAX`, thirty-two megabytes on a 64-bit
/// machine). An array longer than this the allocator maps and unmaps at
/// every job, and so does the pool; one shorter the allocator keeps warm
/// in an arena, and the pool keeps it warm in its place.
pub(crate) const KEEP_MAX: usize = 32 << 20;

/// The mapped buffers jobs have given back and none has taken again
/// (`Options::jobs_pooled`). A job's arrays from the allocator stay warm
/// in the arena of the thread that freed them, which is what makes the
/// next job's cheap and what a memory bound cannot afford: the arena
/// keeps them whether the store has room or not, and nothing a library
/// can call gives the top of another thread's arena back. A mapping of
/// the job's own goes back at the job's end and comes fresh at the next,
/// a fault and a zeroed page for every page of every job. The pool is the
/// arena's warmth with the engine's hand on it: a buffer given back is
/// kept under the array it was (`Site`), the next job's array of that
/// kind takes it, the last job's being the best word there is on how
/// long this one's will grow, and a store that reads short of memory
/// empties the pool and holds it off (`hold`), so that what the jobs
/// before kept is the page cache's from then on. It keeps of each array
/// no more than the jobs have held of it at once, and no buffer longer
/// than the allocator would have kept (`KEEP_MAX`). A pooled buffer is
/// still a mapping alive, counted in `mapped_bytes` and listed in `LIVE`
/// until it is unmapped.
pub(crate) struct Pool {
    /// The buffers kept, each by its address, its length and the array
    /// it was.
    kept: Mutex<Vec<(usize, usize, Site)>>,
    /// The bytes kept, of every array.
    bytes: AtomicUsize,
    /// By array, as `BIG_MAPS` is: the bytes in jobs' hands, and the most
    /// those have been at once, which is what the pool and the jobs
    /// together may hold of that array.
    lent: [AtomicUsize; 13],
    peak: [AtomicUsize; 13],
    /// The stores holding the pool off.
    holds: AtomicUsize,
    /// Buffers taken from it, buffers kept by it, and buffers it
    /// unmapped, ever; and the buffers taken, by array.
    counts: [AtomicUsize; 3],
    takes: [AtomicUsize; 13],
}

/// The process's pool: every store's jobs give to it and take from it.
pub(crate) static POOL: Pool = Pool::new();

impl Pool {
    pub(crate) const fn new() -> Pool {
        Pool {
            kept: Mutex::new(Vec::new()),
            bytes: AtomicUsize::new(0),
            lent: [const { AtomicUsize::new(0) }; 13],
            peak: [const { AtomicUsize::new(0) }; 13],
            holds: AtomicUsize::new(0),
            counts: [const { AtomicUsize::new(0) }; 3],
            takes: [const { AtomicUsize::new(0) }; 13],
        }
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Vec<(usize, usize, Site)>> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Buffers taken from the pool, kept by it and unmapped by it, ever,
    /// and the bytes it holds now.
    pub(crate) fn counts(&self) -> [usize; 4] {
        [
            self.counts[0].load(Ordering::Relaxed),
            self.counts[1].load(Ordering::Relaxed),
            self.counts[2].load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        ]
    }

    /// The buffers taken from the pool, by the array that took each, as
    /// `Site::ALL` names them: what a test holds `Options::jobs_pooled`
    /// to having reached each array and not only some.
    pub(crate) fn takes(&self) -> Vec<(&'static str, usize)> {
        Site::ALL
            .iter()
            .map(|&(s, name)| (name, self.takes[s as usize].load(Ordering::Relaxed)))
            .collect()
    }

    /// A kept buffer of `site`'s, taken out: its address and its length.
    /// The smallest of at least `bytes`, which may be more than was
    /// asked; and where none is that long, the largest where
    /// `or_largest`, for a vector with no buffer yet to grow, its pages
    /// kept, and none otherwise, a vector with a buffer of its own
    /// growing that one.
    fn take(&self, site: Site, bytes: usize, or_largest: bool) -> Option<(*mut u8, usize)> {
        let mut kept = self.kept();
        let (mut fit, mut largest): (Option<usize>, Option<usize>) = (None, None);
        for (i, &(_, len, s)) in kept.iter().enumerate() {
            if s != site {
                continue;
            }
            if len >= bytes && fit.is_none_or(|j| len < kept[j].1) {
                fit = Some(i);
            }
            if largest.is_none_or(|j| len > kept[j].1) {
                largest = Some(i);
            }
        }
        let i = fit.or(if or_largest { largest } else { None })?;
        let (p, len, _) = kept.swap_remove(i);
        self.bytes.fetch_sub(len, Ordering::Relaxed);
        self.counts[0].fetch_add(1, Ordering::Relaxed);
        self.takes[site as usize].fetch_add(1, Ordering::Relaxed);
        Some((p as *mut u8, len))
    }

    /// `bytes` more of `site`'s in a job's hands: a buffer taken or
    /// mapped for it, or one of its buffers grown by that much.
    fn lend(&self, site: Site, bytes: usize) {
        let lent = self.lent[site as usize].fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak[site as usize].fetch_max(lent, Ordering::Relaxed);
    }

    /// A job's mapped buffer given back: kept for the next job's array
    /// of its kind unless a store holds the pool off or it is longer
    /// than the pool keeps (`KEEP_MAX`), and what the pool keeps of that
    /// array then brought down to the most the jobs have held of it at
    /// once less what they hold now, its smallest buffers unmapped
    /// first, so that the pool and the jobs together keep no more of an
    /// array than the jobs alone once did.
    ///
    /// # Safety
    /// `p` is a mapping of `len` bytes from `alloc` or `regrow`, counted
    /// by `lend` under `site` to that length, and nothing reads it after.
    unsafe fn give(&self, site: Site, p: *mut u8, len: usize) {
        let lent = self.lent[site as usize].fetch_sub(len, Ordering::Relaxed) - len;
        let mut out: Vec<(usize, usize)> = Vec::new();
        {
            let mut kept = self.kept();
            // Read under the lock `hold` counts itself under: a give that
            // reads the pool free is in it before the hold's drain, and
            // none is kept behind one.
            if self.holds.load(Ordering::Acquire) != 0 || len > KEEP_MAX {
                out.push((p as usize, len));
            } else {
                kept.push((p as usize, len, site));
                self.bytes.fetch_add(len, Ordering::Relaxed);
                self.counts[1].fetch_add(1, Ordering::Relaxed);
                let room = self.peak[site as usize]
                    .load(Ordering::Relaxed)
                    .saturating_sub(lent);
                let mut of_site: usize = kept
                    .iter()
                    .filter(|&&(_, _, s)| s == site)
                    .map(|&(_, l, _)| l)
                    .sum();
                while of_site > room {
                    let Some(i) = kept
                        .iter()
                        .enumerate()
                        .filter(|(_, &(_, _, s))| s == site)
                        .min_by_key(|(_, &(_, l, _))| l)
                        .map(|(i, _)| i)
                    else {
                        break;
                    };
                    let (a, l, _) = kept.swap_remove(i);
                    self.bytes.fetch_sub(l, Ordering::Relaxed);
                    of_site -= l;
                    out.push((a, l));
                }
            }
        }
        for (a, l) in out {
            self.counts[2].fetch_add(1, Ordering::Relaxed);
            // SAFETY: a mapping of `l` bytes from `alloc` or `regrow`,
            // out of the pool or never in it, and nobody's.
            unsafe { free(a as *mut u8, l, true) };
        }
    }

    /// Every kept buffer unmapped: the bytes given back to the kernel.
    pub(crate) fn drain(&self) -> usize {
        let all: Vec<(usize, usize, Site)> = std::mem::take(&mut *self.kept());
        let mut bytes = 0;
        for (a, l, _) in all {
            self.bytes.fetch_sub(l, Ordering::Relaxed);
            self.counts[2].fetch_add(1, Ordering::Relaxed);
            bytes += l;
            // SAFETY: a mapping of `l` bytes given to the pool, taken out
            // of it under its lock, and nobody's.
            unsafe { free(a as *mut u8, l, true) };
        }
        bytes
    }

    /// A store's hold on the pool, taken where it reads short of memory
    /// and let go where it no longer does or closes: while any store
    /// holds it nothing is kept, and what was kept is unmapped at the
    /// hold.
    pub(crate) fn hold(&self, on: bool) {
        if !on {
            self.holds.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        {
            let _kept = self.kept();
            self.holds.fetch_add(1, Ordering::AcqRel);
        }
        self.drain();
    }
}

/// A job's large array: a vector of plain values that is a `Vec` and
/// nothing else until, where `mapped` says so, its buffer would be
/// `MAP_FROM` bytes or more, and from there a mapping of the engine's
/// own, unmapped at its drop (`Options::jobs_mapped`). A seal's sort, a
/// segment's writer, an ordered index's builder and a merge's key list
/// each hold tens of bytes a key for as long as the job runs and nothing
/// after it, on a thread that ends with the job: from the allocator they
/// stay in that thread's arena, whose top glibc keeps up to twice the
/// largest chunk the process has mapped and freed, resident and
/// nobody's, tens of megabytes an arena under a memory bound that has
/// page cache to hold with them (`docs/engine.md`, which form a workload
/// calls for). With `mapped` off it is the `Vec` it was, the allocator
/// asked for exactly what a `Vec` asks. A mapping grows by `mremap`, its
/// pages moved and never copied, and what of it was never written is
/// resident nowhere.
pub(crate) struct BigVec<T: Copy> {
    /// The buffer while it is the allocator's; empty, with no buffer,
    /// once `map` holds the values.
    heap: Vec<T>,
    /// The buffer once it is a mapping, of `cap` values with the first
    /// `len` written; null until then.
    map: *mut T,
    len: usize,
    cap: usize,
    /// The mapping's length in bytes: `cap` values and, where the buffer
    /// came from a pool, whatever is left of the buffer past them.
    bytes: usize,
    mapped: bool,
    /// The pool the mapping is taken from and given back to
    /// (`Maps::Pooled`); none, and it is this vector's own.
    pool: Option<&'static Pool>,
    site: Site,
}

// SAFETY: a `BigVec` owns its buffer of plain values, as a `Vec` does.
unsafe impl<T: Copy + Send> Send for BigVec<T> {}
// SAFETY: as above; a shared borrow reads only.
unsafe impl<T: Copy + Sync> Sync for BigVec<T> {}

impl<T: Copy> BigVec<T> {
    /// A value with a size, aligned within a block's alignment.
    const FITS: () = assert!(std::mem::size_of::<T>() > 0 && std::mem::align_of::<T>() <= ALIGN);

    /// No buffer yet; `mapped` is whether a large one will be a mapping,
    /// and `site` which of a job's arrays this is.
    pub(crate) fn new(site: Site, maps: impl Into<Maps>) -> BigVec<T> {
        let maps = maps.into();
        Self::new_in(
            site,
            maps != Maps::Heap,
            (maps == Maps::Pooled).then_some(&POOL),
        )
    }

    /// `new`, with the pool named: the process's for a job, and a test's
    /// own for a test of the vector against one.
    fn new_in(site: Site, mapped: bool, pool: Option<&'static Pool>) -> BigVec<T> {
        let () = Self::FITS;
        BigVec {
            heap: Vec::new(),
            map: std::ptr::null_mut(),
            len: 0,
            cap: 0,
            bytes: 0,
            mapped,
            pool,
            site,
        }
    }

    /// Room for `n` values, none of it touched: a mapping from the start
    /// where that is large and `mapped`.
    pub(crate) fn with_capacity(n: usize, site: Site, maps: impl Into<Maps>) -> BigVec<T> {
        let mut v = BigVec::new(site, maps);
        if !v.become_mapping(n) {
            v.heap = Vec::with_capacity(n);
        }
        v
    }

    /// The values the buffer has room for.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        if self.map.is_null() {
            self.heap.capacity()
        } else {
            self.cap
        }
    }

    /// Whether the buffer is a mapping as it stands.
    #[cfg(test)]
    pub(crate) fn is_mapped(&self) -> bool {
        !self.map.is_null()
    }

    /// The allocator's buffer left for a mapping of `cap` values, where
    /// `mapped` says so and that many are `MAP_FROM` bytes or more:
    /// whether it was. The values move once, here; the mapping's own
    /// growths after this copy nothing.
    #[cold]
    fn become_mapping(&mut self, cap: usize) -> bool {
        debug_assert!(self.map.is_null());
        let size = std::mem::size_of::<T>();
        let bytes = cap.checked_mul(size).expect("BigVec: capacity overflow");
        if !maps(bytes, self.mapped) {
            return false;
        }
        // The buffer the last such array left in the pool, where there
        // is a pool and it has one, and a mapping of its own otherwise.
        // A pooled buffer may be longer than was asked, and every value
        // of it is this vector's room; one shorter is grown, its pages
        // kept.
        let (p, bytes) = match self.pool.and_then(|pool| pool.take(self.site, bytes, true)) {
            Some((p, len)) if len >= bytes => (p as *mut T, len),
            // SAFETY: `p` is a mapping of `len` bytes out of the pool,
            // nobody's, none of it this vector's values yet.
            Some((p, len)) => (unsafe { regrow(p, len, bytes, 0, true) } as *mut T, bytes),
            None => (alloc(bytes, false, true) as *mut T, bytes),
        };
        if let Some(pool) = self.pool {
            pool.lend(self.site, bytes);
        }
        let cap = bytes / size;
        // SAFETY: `p` is `cap` values mapped, `cap` at least the `Vec`'s
        // length by every caller's choice of it, and apart from the
        // `Vec`'s buffer.
        unsafe { std::ptr::copy_nonoverlapping(self.heap.as_ptr(), p, self.heap.len()) };
        self.len = self.heap.len();
        self.cap = cap;
        self.bytes = bytes;
        self.map = p;
        self.heap = Vec::new();
        BIG_MAPS[self.site as usize].fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Before the `Vec` grows to hold `more` values past its length: the
    /// mapping in its place where the buffer it would grow to is large,
    /// sized as the `Vec` would size it, twice what it held or what is
    /// asked. Whether the values are in a mapping now.
    #[cold]
    fn outgrow(&mut self, more: usize) -> bool {
        let want = self
            .heap
            .len()
            .checked_add(more)
            .expect("BigVec: capacity overflow");
        self.become_mapping(want.max(self.heap.capacity().saturating_mul(2)))
    }

    /// The mapping grown to hold at least `want` values, and at least
    /// twice what it held, so pushes are amortised as a `Vec`'s are.
    #[cold]
    fn grow_map(&mut self, want: usize) {
        let size = std::mem::size_of::<T>();
        let cap = want.max(self.cap.saturating_mul(2));
        let bytes = cap.checked_mul(size).expect("BigVec: capacity overflow");
        // A larger buffer of this array's from the pool where it has
        // one, the values copied into it and this one given back: the
        // pool's buffers are the lengths the jobs before grew to, so a
        // vector that grows as one of theirs did ends in the buffer
        // theirs ended in.
        if let Some(pool) = self.pool {
            if let Some((q, len)) = pool.take(self.site, bytes, false) {
                pool.lend(self.site, len);
                // SAFETY: `q` is `len` bytes out of the pool, at least
                // `bytes` and so past the `self.len` values copied, and
                // apart from this mapping, which is given back whole.
                unsafe {
                    std::ptr::copy_nonoverlapping(self.map, q as *mut T, self.len);
                    pool.give(self.site, self.map as *mut u8, self.bytes);
                }
                self.map = q as *mut T;
                self.cap = len / size;
                self.bytes = len;
                return;
            }
        }
        // SAFETY: the mapping is `self.bytes` bytes from `alloc`,
        // `regrow` or a pool, mapped, its first `self.len` values
        // written, and `self.map` is replaced below.
        let p = unsafe {
            regrow(
                self.map as *mut u8,
                self.bytes,
                bytes,
                self.len * size,
                true,
            )
        };
        if let Some(pool) = self.pool {
            pool.lend(self.site, bytes - self.bytes);
        }
        self.map = p as *mut T;
        self.cap = cap;
        self.bytes = bytes;
    }

    /// Room for `more` values past the length.
    pub(crate) fn reserve(&mut self, more: usize) {
        if self.map.is_null() {
            let full = self.heap.capacity() - self.heap.len() < more;
            if !(self.mapped && full && self.outgrow(more)) {
                self.heap.reserve(more);
                return;
            }
        }
        let want = self
            .len
            .checked_add(more)
            .expect("BigVec: capacity overflow");
        if want > self.cap {
            self.grow_map(want);
        }
    }

    #[inline]
    pub(crate) fn push(&mut self, v: T) {
        if self.map.is_null() {
            let full = self.heap.len() == self.heap.capacity();
            if !(self.mapped && full && self.outgrow(1)) {
                self.heap.push(v);
                return;
            }
        }
        if self.len == self.cap {
            self.grow_map(self.len + 1);
        }
        // SAFETY: `self.len < self.cap`, inside the mapping.
        unsafe { self.map.add(self.len).write(v) };
        self.len += 1;
    }

    #[inline]
    pub(crate) fn extend_from_slice(&mut self, s: &[T]) {
        if self.map.is_null() {
            let full = self.heap.capacity() - self.heap.len() < s.len();
            if !(self.mapped && full && self.outgrow(s.len())) {
                self.heap.extend_from_slice(s);
                return;
            }
        }
        if self.cap - self.len < s.len() {
            self.grow_map(self.len + s.len());
        }
        // SAFETY: room for `s.len()` values past the length, and `s`
        // borrows nothing of this buffer mutably.
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.map.add(self.len), s.len()) };
        self.len += s.len();
    }

    /// `n` values long: cut short, or filled out with `v`.
    pub(crate) fn resize(&mut self, n: usize, v: T) {
        if self.map.is_null() {
            let full = n > self.heap.capacity();
            if !(self.mapped && full && self.outgrow(n - self.heap.len())) {
                self.heap.resize(n, v);
                return;
            }
        }
        if n > self.len {
            if n > self.cap {
                self.grow_map(n);
            }
            for i in self.len..n {
                // SAFETY: `i < n <= self.cap`.
                unsafe { self.map.add(i).write(v) };
            }
        }
        self.len = n;
    }

    /// No values, the buffer kept.
    pub(crate) fn clear(&mut self) {
        self.heap.clear();
        self.len = 0;
    }
}

impl<T: Copy> std::ops::Deref for BigVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        if self.map.is_null() {
            return &self.heap;
        }
        // SAFETY: the mapping's first `self.len` values are written.
        unsafe { std::slice::from_raw_parts(self.map, self.len) }
    }
}

impl<T: Copy> std::ops::DerefMut for BigVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        if self.map.is_null() {
            return &mut self.heap;
        }
        // SAFETY: as `deref`, and the borrow is exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.map, self.len) }
    }
}

impl<T: Copy> Drop for BigVec<T> {
    fn drop(&mut self) {
        if !self.map.is_null() {
            // SAFETY: the mapping is `self.bytes` bytes from `alloc`,
            // `regrow` or a pool, mapped, and nothing reads it after.
            unsafe {
                match self.pool {
                    Some(pool) => pool.give(self.site, self.map as *mut u8, self.bytes),
                    None => free(self.map as *mut u8, self.bytes, true),
                }
            }
        }
    }
}

/// What a sort or a fill asks of the vector it is handed, so one body
/// serves a `Vec` and a `BigVec` (`radix_by_prefix`).
pub(crate) trait Plain<T: Copy>: std::ops::DerefMut<Target = [T]> {
    fn clear(&mut self);
    fn resize(&mut self, n: usize, v: T);
}

impl<T: Copy> Plain<T> for Vec<T> {
    fn clear(&mut self) {
        Vec::clear(self)
    }
    fn resize(&mut self, n: usize, v: T) {
        Vec::resize(self, n, v)
    }
}

impl<T: Copy> Plain<T> for BigVec<T> {
    fn clear(&mut self) {
        BigVec::clear(self)
    }
    fn resize(&mut self, n: usize, v: T) {
        BigVec::resize(self, n, v)
    }
}

/// The cells slabs have handed out, process-wide, ever: what a test holds
/// `Options::zpages_memo_slab` to having done.
static SLAB_CELLS: AtomicUsize = AtomicUsize::new(0);

/// The count of cells slabs have handed out.
pub(crate) fn slab_cells() -> usize {
    SLAB_CELLS.load(Ordering::Relaxed)
}

/// Cells of one size in one block, handed out and taken back: where a
/// reader handle's memo keeps its decoded groups
/// (`Options::zpages_memo_slab`). From the allocator each group's buffer
/// is a chunk of its own in whole pages, twice the group, and a handle
/// dropped leaves them all in its thread's arena, which glibc keeps the
/// top of: eighteen megabytes a handle for a memo budgeted at eight,
/// resident after the thread that read through it has gone. A slab is
/// one block of `cell * cells` bytes, a mapping where that is
/// `MAP_FROM` or more, its pages touched as its cells are first written
/// and all of it unmapped when the slab and the last cell of it have
/// dropped.
pub(crate) struct Slab {
    inner: std::sync::Arc<SlabInner>,
}

struct SlabInner {
    p: *mut u8,
    bytes: usize,
    cell: usize,
    cells: usize,
    state: Mutex<SlabState>,
}

/// The cells never yet handed out begin at `made`; `free` are the ones
/// handed back.
struct SlabState {
    made: usize,
    free: Vec<u32>,
}

// SAFETY: the block is the slab's own, and its cells are disjoint ranges
// of it, each handed to one holder at a time under the lock.
unsafe impl Send for SlabInner {}
// SAFETY: as above; the lock is what the handing out goes through.
unsafe impl Sync for SlabInner {}

impl SlabInner {
    fn state(&self) -> std::sync::MutexGuard<'_, SlabState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Slab {
    /// `cells` cells of `cell` bytes, the size rounded up to a cache line
    /// so that every cell is aligned to one; zeroed, nothing touched.
    pub(crate) fn new(cell: usize, cells: usize) -> Slab {
        let cell = cell.max(1).next_multiple_of(ALIGN);
        let cells = cells.max(1);
        let bytes = cell.checked_mul(cells).expect("slab: size overflow");
        Slab {
            inner: std::sync::Arc::new(SlabInner {
                p: alloc(bytes, true, true),
                bytes,
                cell,
                cells,
                state: Mutex::new(SlabState {
                    made: 0,
                    free: Vec::new(),
                }),
            }),
        }
    }

    /// A cell: one handed back before, else the next never handed out,
    /// else none, every cell being out.
    pub(crate) fn take(&self) -> Option<SlabCell> {
        let i = {
            let mut s = self.inner.state();
            match s.free.pop() {
                Some(i) => i,
                None if s.made < self.inner.cells => {
                    s.made += 1;
                    (s.made - 1) as u32
                }
                None => return None,
            }
        };
        SLAB_CELLS.fetch_add(1, Ordering::Relaxed);
        Some(SlabCell {
            slab: self.inner.clone(),
            i,
        })
    }
}

impl Drop for SlabInner {
    fn drop(&mut self) {
        // SAFETY: from `alloc` with this length, mapped, and every cell
        // of it has dropped, each holding this alive until it did.
        unsafe { free(self.p, self.bytes, true) };
    }
}

/// One cell of a slab, its holder's until it drops, which hands it back.
pub(crate) struct SlabCell {
    slab: std::sync::Arc<SlabInner>,
    i: u32,
}

impl SlabCell {
    /// The cell's bytes, a whole number of cache lines.
    pub(crate) fn len(&self) -> usize {
        self.slab.cell
    }

    /// Where the cell begins, aligned to a cache line: `len` bytes, zero
    /// at the cell's first use and whatever its last holder left after.
    pub(crate) fn as_ptr(&self) -> *mut u8 {
        // SAFETY: `i < cells`, so the offset is inside the block.
        unsafe { self.slab.p.add(self.i as usize * self.slab.cell) }
    }
}

impl Drop for SlabCell {
    fn drop(&mut self) {
        self.slab.state().free.push(self.i);
    }
}

impl crate::flatindex::TrailerBuf for BigVec<u8> {
    fn resize(&mut self, n: usize, v: u8) {
        BigVec::resize(self, n, v)
    }
    fn extend_from_slice(&mut self, s: &[u8]) {
        BigVec::extend_from_slice(self, s)
    }
}

#[cfg(unix)]
fn map(len: usize) -> *mut u8 {
    // SAFETY: an anonymous private mapping of `len` bytes, no file, no
    // address asked for; the kernel rounds the length up to its pages
    // and `unmap` gives it the same length.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert!(p != libc::MAP_FAILED, "block: out of memory (mmap)");
    p as *mut u8
}

/// A transparent huge page's bytes, the alignment `map_huge` maps at.
pub(crate) const HUGE: usize = 2 << 20;

/// `len` bytes mapped as `map` maps them, at an address a multiple of
/// `HUGE` and advised `MADV_HUGEPAGE`, so that the kernel can back every
/// whole `HUGE` of it with one page, which a mapping at an address the
/// kernel chose leaves to the extents that happen to be aligned. Mapped
/// `HUGE` longer and the excess on either side unmapped, so the mapping is
/// exactly `[p, p + len)` and `unmap` gives it back as `map`'s. The advice
/// is advice: a kernel without huge pages leaves the pages small.
#[cfg(unix)]
fn map_huge(len: usize) -> *mut u8 {
    // SAFETY: an anonymous private mapping of `len + HUGE` bytes, no file,
    // no address asked for.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len + HUGE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert!(p != libc::MAP_FAILED, "block: out of memory (mmap)");
    let (a, head, tail) = huge_trim(p as usize);
    // SAFETY: the parts of the mapping just made outside `[a, a + len)`,
    // which nothing has seen, and the advice over the rest.
    unsafe {
        if head > 0 {
            libc::munmap(p, head);
        }
        if tail > 0 {
            libc::munmap((a + len) as *mut libc::c_void, tail);
        }
        #[cfg(target_os = "linux")]
        libc::madvise(a as *mut libc::c_void, len, libc::MADV_HUGEPAGE);
    }
    a as *mut u8
}

/// Of a mapping `HUGE` bytes longer than the block it is cut for at `p`, the
/// block's start at the first multiple of `HUGE`, and the bytes before the
/// block and after it to unmap. A kernel from 6.7 on puts an anonymous
/// mapping of `HUGE` or more at such a multiple already, and one before it
/// anywhere a page lands.
fn huge_trim(p: usize) -> (usize, usize, usize) {
    let a = p.next_multiple_of(HUGE);
    let head = a - p;
    (a, head, HUGE - head)
}

#[cfg(not(unix))]
fn map_huge(len: usize) -> *mut u8 {
    map(len)
}

/// `alloc(len, true, true)`'s mapping by `map_huge`, counted as `alloc`
/// counts a mapping and freed by `free` as one.
fn alloc_huge(len: usize) -> *mut u8 {
    let p = map_huge(len);
    MAPPED.fetch_add(len, Ordering::Relaxed);
    live().push((p as usize, len));
    p
}

#[cfg(unix)]
unsafe fn unmap(p: *mut u8, len: usize) {
    // SAFETY: the caller's, see `free`.
    let r = unsafe { libc::munmap(p as *mut libc::c_void, len) };
    debug_assert_eq!(r, 0, "block: munmap failed");
}

#[cfg(not(unix))]
fn map(len: usize) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(len, ALIGN).expect("block layout");
    // SAFETY: a non-zero layout.
    let p = unsafe { std::alloc::alloc_zeroed(layout) };
    assert!(!p.is_null(), "block: out of memory");
    p
}

#[cfg(not(unix))]
unsafe fn unmap(p: *mut u8, len: usize) {
    let layout = std::alloc::Layout::from_size_align(len, ALIGN).expect("block layout");
    // SAFETY: the caller's, see `free`.
    unsafe { std::alloc::dealloc(p, layout) };
}

/// The flags the kernel keeps for the mapping that holds `p`, from
/// `/proc/self/smaps`: `hg` where it was advised onto huge pages.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn vm_flags(p: *const u8) -> String {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    let mut inside = false;
    for line in smaps.lines() {
        if let Some((range, _)) = line.split_once(' ') {
            if let Some((lo, hi)) = range.split_once('-') {
                if let (Ok(lo), Ok(hi)) =
                    (usize::from_str_radix(lo, 16), usize::from_str_radix(hi, 16))
                {
                    inside = (lo..hi).contains(&(p as usize));
                    continue;
                }
            }
        }
        if inside {
            if let Some(f) = line.strip_prefix("VmFlags:") {
                return f.to_string();
            }
        }
    }
    panic!("no mapping holds {p:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that populates hands out a zeroed block it maps with every
    /// page resident and zero, the pages made by writes before anything
    /// reads them; one that does not hands it out with no page touched, as
    /// a mapping is, so a probe's load maps the zero page; and a block not
    /// asked zeroed, or under `MAP_FROM`, is no business of the population.
    /// Read off `mincore`, which counts a page the mapping has a page for.
    /// Not under qemu-user (`supdb_emulated`), which answers
    /// `madvise(MADV_POPULATE_WRITE)` with success and passes nothing to
    /// the kernel: a populated block of 512 KiB read every page resident
    /// natively and none emulated. The native ARM leg runs it.
    #[cfg(all(target_os = "linux", not(supdb_emulated)))]
    #[test]
    fn a_populating_source_maps_a_zeroed_block_written_and_no_other() {
        let len = 4 * MAP_FROM;
        for (populate, zeroed, resident) in [(true, true, len), (false, true, 0), (true, false, 0)]
        {
            let src = Source::new(true, None).populated(populate);
            assert_eq!(src.clone().populates(), populate, "a clone keeps it");
            let p = src.alloc(len, zeroed);
            assert_eq!(
                resident_of(p, len),
                resident,
                "populate {populate}, zeroed {zeroed}"
            );
            if zeroed {
                // SAFETY: `len` bytes just allocated.
                let s = unsafe { std::slice::from_raw_parts(p, len) };
                assert!(s.iter().all(|&b| b == 0), "populate {populate}: not zeroed");
            }
            // SAFETY: from this source's `alloc` with this length.
            unsafe { src.free(p, len) };
        }
        let small = Source::new(true, None).populated(true);
        let p = small.alloc(MAP_FROM / 2, true);
        // SAFETY: the bytes just allocated, from the heap under `MAP_FROM`.
        let s = unsafe { std::slice::from_raw_parts(p, MAP_FROM / 2) };
        assert!(s.iter().all(|&b| b == 0), "a heap block zeroed");
        // SAFETY: as above.
        unsafe { small.free(p, MAP_FROM / 2) };
    }

    /// The mapping a huge block is cut from is trimmed to the first multiple
    /// of `HUGE` in it, whatever address the kernel gave: the bytes kept
    /// start there, and the bytes before and after them are the excess,
    /// none past the mapping's end.
    #[test]
    fn a_huge_mapping_is_cut_at_the_first_multiple_of_a_huge_page() {
        let len = 2 * HUGE;
        for p in [0x4000_0000usize, 0x4000_1000, 0x401f_f000, 0x4010_0000] {
            let (a, head, tail) = huge_trim(p);
            assert_eq!(a % HUGE, 0, "{p:#x}");
            assert!(a >= p && a < p + HUGE, "{p:#x}: the first multiple");
            assert_eq!(head, a - p, "{p:#x}");
            assert_eq!(head + len + tail, len + HUGE, "{p:#x}: the whole mapping");
        }
    }

    /// A huge source maps a block of `HUGE` bytes or more at an address a
    /// multiple of `HUGE`, advised onto huge pages, writable to its last
    /// byte, and neither takes such a block from its pool nor gives one to
    /// it; a block under `HUGE` from it, and every block of a source that is
    /// not huge, are as they were. Where the kernel has no huge pages the
    /// advice is refused and the flag absent, and under qemu-user
    /// (`supdb_emulated`) the advice is answered and passed to no kernel,
    /// so the flag is read only where a kernel keeps the setting; where
    /// the block is cut and what the pool holds are the source's own and
    /// are held everywhere.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_huge_source_maps_its_large_blocks_aligned_and_advised_and_pools_none() {
        let thp = !cfg!(supdb_emulated)
            && std::path::Path::new("/sys/kernel/mm/transparent_hugepage/enabled").exists();
        let len = 2 * HUGE;
        let pool = std::sync::Arc::new(BlockPool::new(4 * len, true));
        let huge = Source::new(true, Some(pool.clone())).huge(true);
        let plain = Source::new(true, Some(pool.clone()));
        // A plain block given to the pool, which a huge source must not take.
        let q = plain.alloc(len, false);
        // SAFETY: from this source's `alloc` with this length.
        unsafe { plain.free(q, len) };
        assert_eq!(pool.held().len(), 1, "a plain block pooled");
        let p = huge.alloc(len, false);
        assert_ne!(p, q, "a huge block taken from the pool");
        assert_eq!(p as usize % HUGE, 0, "a huge block aligned");
        if thp {
            assert!(vm_flags(p).split_whitespace().any(|f| f == "hg"), "advised");
        }
        // SAFETY: `len` bytes just mapped.
        let s = unsafe { std::slice::from_raw_parts_mut(p, len) };
        s[0] = 1;
        s[len - 1] = 2;
        assert_eq!((s[0], s[len - 1]), (1, 2));
        // SAFETY: from this source's `alloc` with this length.
        unsafe { huge.free(p, len) };
        assert_eq!(pool.held().len(), 1, "a huge block given to the pool");
        // A block under `HUGE` from the huge source is the pool's as any.
        let small = huge.alloc(HUGE / 2, false);
        if thp {
            assert!(
                !vm_flags(small).split_whitespace().any(|f| f == "hg"),
                "small advised"
            );
        }
        // SAFETY: as above.
        unsafe { huge.free(small, HUGE / 2) };
        assert_eq!(pool.held().len(), 2, "a small block pooled");
        let r = plain.alloc(len, false);
        assert_eq!(r, q, "the plain block, from the pool");
        if thp {
            assert!(
                !vm_flags(r).split_whitespace().any(|f| f == "hg"),
                "plain advised"
            );
        }
        // SAFETY: as above.
        unsafe { plain.free(r, len) };
    }

    /// A block of either kind is cache-line aligned, zero where asked,
    /// writable through, and given back; a block under `MAP_FROM` is never
    /// a mapping, however the option reads. The counter is the process's,
    /// moved by every store the binary's other tests run beside this one,
    /// so it is held in the single-test binary (`tests/mapped.rs`) and not
    /// here.
    #[test]
    fn blocks_of_both_kinds_are_aligned_zeroed_and_freed() {
        for &(len, zeroed, mapped) in &[
            (MAP_FROM, true, true),
            (MAP_FROM + 4096, false, true),
            (MAP_FROM - 64, true, true),
            (64usize, true, false),
            (MAP_FROM, false, false),
        ] {
            let p = alloc(len, zeroed, mapped);
            assert_eq!(p as usize % ALIGN, 0, "{len} {zeroed} {mapped}");
            // SAFETY: `len` bytes just allocated.
            let s = unsafe { std::slice::from_raw_parts_mut(p, len) };
            if zeroed {
                assert!(s.iter().all(|&b| b == 0), "{len} {mapped}: not zeroed");
            }
            s[0] = 1;
            s[len - 1] = 2;
            assert_eq!((s[0], s[len - 1]), (1, 2));
            // SAFETY: from `alloc` with these arguments, read no more.
            unsafe { free(p, len, mapped) };
        }
        assert!(maps(MAP_FROM, true));
        assert!(!maps(MAP_FROM - 1, true));
        assert!(!maps(MAP_FROM, false));
    }

    /// A pool keeps what it is given under the array it was and lends
    /// that array's smallest buffer that holds what is asked, or its
    /// largest to a vector that has none yet, and nothing of another
    /// array's; of each array it keeps, with what is lent of it, no more
    /// than the most that was ever lent of it at once, the smallest
    /// going first, whatever the other arrays hold; held off it keeps
    /// nothing and gives back what it had; let go it keeps again; and a
    /// buffer longer than the allocator would have kept it never keeps.
    /// A pool of the test's own: the process's is every store's.
    #[test]
    fn a_pool_lends_a_sites_smallest_fit_keeps_to_its_peak_and_empties_at_a_hold() {
        let pool = Pool::new();
        let m = MAP_FROM;
        let (t, other) = (Site::Test, Site::SortRecs);
        let lent = |site: Site, n: usize| {
            let p = alloc(n, false, true);
            pool.lend(site, n);
            p
        };
        // Three lent at once, which is the array's peak, and all kept.
        let (a, b, c) = (lent(t, m), lent(t, 2 * m), lent(t, 4 * m));
        // SAFETY: here and below, each buffer is a mapping of the length
        // it is given back with, lent to that length under the array it
        // is given back as, and read no more.
        unsafe {
            pool.give(t, a, m);
            pool.give(t, b, 2 * m);
            pool.give(t, c, 4 * m);
        }
        assert_eq!(pool.counts(), [0, 3, 0, 7 * m], "all three kept");
        // Another array's vector takes none of them, to grow or not.
        assert!(pool.take(other, 1, true).is_none(), "another array's");
        // The smallest that holds what is asked; and where none does,
        // none for a vector that has a buffer to grow.
        assert!(
            pool.take(t, 4 * m + 1, false).is_none(),
            "nothing that long"
        );
        assert_eq!(pool.counts(), [0, 3, 0, 7 * m], "and nothing taken");
        assert_eq!(
            pool.take(t, m + 1, false),
            Some((b, 2 * m)),
            "the smaller of two"
        );
        pool.lend(t, 2 * m);
        assert_eq!(pool.take(t, 1, true), Some((a, m)), "the smallest of all");
        pool.lend(t, m);
        assert_eq!(pool.counts(), [2, 3, 0, 4 * m]);
        // Three lent of a peak of seven: room for four, which the one
        // kept fills, so a fourth buffer given back goes at once, being
        // the smallest, and the larger one stays.
        let d = lent(t, 2 * m);
        unsafe { pool.give(t, d, 2 * m) };
        assert_eq!(pool.counts(), [2, 4, 1, 4 * m], "kept to the peak");
        // The largest, where none is as long as is asked and the vector
        // has no buffer yet.
        assert_eq!(
            pool.take(t, 8 * m, true),
            Some((c, 4 * m)),
            "the largest, to grow"
        );
        pool.lend(t, 4 * m);
        // Another array's buffer is kept by that array's room: this one
        // is all lent, at its peak, and has none.
        let x = lent(other, 2 * m);
        unsafe { pool.give(other, x, 2 * m) };
        assert_eq!(pool.counts(), [3, 5, 1, 2 * m], "each array its own room");
        assert!(
            pool.take(t, 1, true).is_none(),
            "and none of it this array's"
        );
        assert_eq!(
            pool.take(other, 1, false),
            Some((x, 2 * m)),
            "its own array's"
        );
        pool.lend(other, 2 * m);
        unsafe {
            pool.give(other, x, 2 * m);
            pool.give(t, a, m);
            pool.give(t, b, 2 * m);
            pool.give(t, c, 4 * m);
        }
        assert_eq!(pool.counts(), [4, 9, 1, 9 * m]);
        let takes = pool.takes();
        assert_eq!(takes.len(), Site::ALL.len());
        assert_eq!(
            takes.iter().find(|(n, _)| *n == "sort records").unwrap().1,
            1,
            "the takes counted by the array that took"
        );
        assert_eq!(takes.iter().map(|&(_, n)| n).sum::<usize>(), 1);
        // Held off: emptied, and a buffer given back is not kept.
        pool.hold(true);
        assert_eq!(pool.counts(), [4, 9, 5, 0], "emptied at the hold");
        assert!(pool.take(t, 1, true).is_none());
        let e = lent(t, m);
        unsafe { pool.give(t, e, m) };
        assert_eq!(pool.counts(), [4, 9, 6, 0], "nothing kept behind a hold");
        // A second store's hold, and the first let go: still held.
        pool.hold(true);
        pool.hold(false);
        let f = lent(t, m);
        unsafe { pool.give(t, f, m) };
        assert_eq!(pool.counts(), [4, 9, 7, 0], "held while any store holds");
        // Let go: kept again, and drained for the bytes it held.
        pool.hold(false);
        let g = lent(t, m);
        unsafe { pool.give(t, g, m) };
        assert_eq!(pool.counts(), [4, 10, 7, m]);
        assert_eq!(pool.drain(), m);
        assert_eq!(pool.counts(), [4, 10, 8, 0]);
        assert_eq!(pool.drain(), 0, "nothing left to give back");
        // A buffer longer than the allocator would have kept is unmapped
        // as it comes back, and one as long as that is kept.
        let long = lent(t, KEEP_MAX + m);
        unsafe { pool.give(t, long, KEEP_MAX + m) };
        assert_eq!(pool.counts(), [4, 10, 9, 0], "too long to keep");
        let most = lent(t, KEEP_MAX);
        unsafe { pool.give(t, most, KEEP_MAX) };
        assert_eq!(pool.counts(), [4, 11, 9, KEEP_MAX], "the longest kept");
        assert_eq!(pool.drain(), KEEP_MAX);
        // An array past its room gives up its own buffers, the smallest
        // first, and none of another array's, however small that is.
        let third = Site::SortOrder;
        let small = lent(other, m);
        unsafe { pool.give(other, small, m) };
        let (p1, p2) = (lent(third, 2 * m), lent(third, 4 * m));
        unsafe {
            pool.give(third, p1, 2 * m);
            pool.give(third, p2, 4 * m);
        }
        assert_eq!(pool.counts(), [4, 14, 10, 7 * m]);
        let p3 = lent(third, 4 * m);
        unsafe { pool.give(third, p3, 4 * m) };
        assert_eq!(
            pool.counts(),
            [4, 15, 12, 5 * m],
            "two of its own went, the smaller and one of the larger"
        );
        assert_eq!(
            pool.take(other, 1, false),
            Some((small, m)),
            "the other array's smaller buffer stayed"
        );
        pool.lend(other, m);
        unsafe { pool.give(other, small, m) };
        assert_eq!(pool.drain(), 5 * m);
    }

    /// A vector with a pool has its buffer from it and gives it back: the
    /// next vector of its kind fed as the first was ends in the buffer
    /// the first ended in, having moved into it where it outgrew a
    /// smaller one the pool lent it first, and holds what it was fed
    /// whatever the buffer held before; one that asks more than any kept
    /// buffer holds takes the largest and grows it; and a vector of
    /// another kind, or with no pool, takes nothing.
    #[test]
    fn a_pooled_bigvec_takes_gives_back_and_grows_into_what_the_pool_keeps() {
        let pool: &'static Pool = Box::leak(Box::new(Pool::new()));
        type V = (u64, u64, u32);
        let size = std::mem::size_of::<V>();
        let n = 40_000u64;
        // Two alive at once: one of ten thousand values from the start,
        // and one grown a push at a time to forty thousand.
        let (small, large) = {
            let mut s: BigVec<V> = BigVec::new_in(Site::Test, true, Some(pool));
            s.reserve(10_000);
            s.push((1, 2, 3));
            let mut l: BigVec<V> = BigVec::new_in(Site::Test, true, Some(pool));
            for i in 0..n {
                l.push((i, !i, i as u32));
            }
            assert!(s.is_mapped() && l.is_mapped());
            assert_eq!(
                pool.counts(),
                [0, 0, 0, 0],
                "nothing to take, nothing given yet"
            );
            (
                (s.as_ptr() as usize, s.capacity()),
                (l.as_ptr() as usize, l.capacity()),
            )
        };
        assert_eq!(small.1, 10_000);
        assert!(large.1 >= 2 * small.1, "the grown one the larger by twice");
        let kept = (small.1 + large.1) * size;
        assert_eq!(pool.counts(), [0, 2, 0, kept], "both kept at their drop");
        // The next vector: a `Vec` until its buffer would be a mapping,
        // then the smaller kept buffer, which holds what it asks first,
        // then the larger, where it outgrows that one.
        let mut b: BigVec<V> = BigVec::new_in(Site::Test, true, Some(pool));
        let mut v: Vec<V> = Vec::new();
        for i in 0..n {
            b.push((!i, i, 7));
            v.push((!i, i, 7));
            if i == 9_000 {
                assert_eq!(
                    (b.as_ptr() as usize, b.capacity()),
                    small,
                    "in the smaller buffer first"
                );
                assert_eq!(pool.counts(), [1, 2, 0, large.1 * size]);
            }
        }
        assert_eq!(
            (b.as_ptr() as usize, b.capacity()),
            large,
            "then the larger"
        );
        assert_eq!(&b[..], &v[..], "what it was fed, whatever the buffer held");
        assert_eq!(
            pool.counts(),
            [2, 3, 0, small.1 * size],
            "taken twice, and the smaller given back at the move"
        );
        drop(b);
        assert_eq!(pool.counts(), [2, 4, 0, kept]);
        // A vector that asks more than either holds: the largest, grown.
        let more = 2 * large.1;
        let mut big: BigVec<V> = BigVec::new_in(Site::Test, true, Some(pool));
        big.reserve(more);
        assert!(big.is_mapped());
        assert_eq!(big.capacity(), more);
        assert_eq!(
            pool.counts(),
            [3, 4, 0, small.1 * size],
            "the larger taken to grow"
        );
        for i in 0..more as u64 {
            big.push((i, 1, 2));
        }
        assert_eq!(big.capacity(), more, "and long enough as grown");
        assert!(big.iter().enumerate().all(|(i, &x)| x == (i as u64, 1, 2)));
        // A vector of another kind takes nothing of the pool's.
        let mut other: BigVec<V> = BigVec::new_in(Site::SortRecs, true, Some(pool));
        for i in 0..n {
            other.push((i, i, 1));
        }
        assert!(other.is_mapped());
        assert_eq!(pool.counts(), [3, 4, 0, small.1 * size], "another array's");
        // Given back: the other kind's is kept, the first of its kind;
        // and of this kind the grown one, the most ever lent of it at
        // once, is kept and the small one goes for it.
        drop(other);
        assert_eq!(
            pool.counts(),
            [3, 5, 0, (small.1 + large.1) * size],
            "the other array's kept by its own room"
        );
        drop(big);
        let at_most = (more + large.1) * size;
        assert_eq!(
            pool.counts(),
            [3, 6, 1, at_most],
            "the smallest of its kind went"
        );
        // With no pool the same vector's buffer is its own, and goes.
        let mut own: BigVec<V> = BigVec::new_in(Site::Test, true, None);
        for i in 0..n {
            own.push((i, i, 1));
        }
        assert!(own.is_mapped());
        drop(own);
        assert_eq!(
            pool.counts(),
            [3, 6, 1, at_most],
            "nothing of it the pool's"
        );
        assert_eq!(pool.drain(), at_most);
    }

    /// A pool gives back a block it holds only for a request of the same
    /// length, the most recently given first, with the bytes its last owner
    /// wrote unless the request asks for zeros, and the same address; it
    /// holds no more than its cap, the block past it unmapped as without a
    /// pool; off, it keeps nothing and has given back what it held; and a
    /// block under `MAP_FROM` or of a source that does not map never
    /// reaches it. Its counts are its own, so they are held exactly here,
    /// where the process's mapped bytes are not (`tests/mapped.rs`).
    #[test]
    fn a_pool_gives_a_block_back_to_its_length_and_keeps_its_cap() {
        const LEN: usize = 4 * MAP_FROM;
        let pool = std::sync::Arc::new(BlockPool::new(2 * LEN + MAP_FROM, true));
        let src = Source::new(true, Some(pool.clone()));
        let a = src.alloc(LEN, false);
        let b = src.alloc(LEN, false);
        let c = src.alloc(MAP_FROM, false);
        // SAFETY: blocks of these lengths, just allocated.
        unsafe {
            a.write(7);
            b.write(8);
            c.write(9);
        }
        assert_eq!(pool.counts(), (0, 0, 0), "nothing given yet");
        assert_eq!(
            pool.misses(),
            (3, 2 * LEN + MAP_FROM),
            "an empty pool's three blocks mapped fresh"
        );
        // SAFETY: from this source with these lengths, read no more here.
        unsafe {
            src.free(a, LEN);
            src.free(b, LEN);
            src.free(c, MAP_FROM);
        }
        assert_eq!(pool.counts(), (2 * LEN + MAP_FROM, 0, 3), "all three held");
        // A fourth over the cap: unmapped, not held.
        let d = src.alloc(MAP_FROM, false);
        assert_eq!(pool.counts().1, 1, "the third block given back out");
        assert_eq!(pool.misses().0, 3, "a block taken is no miss");
        assert_eq!(d, c, "the same block");
        // SAFETY: as above.
        assert_eq!(unsafe { d.read() }, 9, "what its last owner wrote");
        let e = alloc(MAP_FROM, false, true);
        // SAFETY: as above.
        unsafe {
            src.free(d, MAP_FROM);
            src.free(e, MAP_FROM);
        }
        assert_eq!(
            pool.counts(),
            (2 * LEN + MAP_FROM, 1, 4),
            "the pool full, the block past its cap unmapped"
        );
        // The most recent of a length first, and zeroed where asked.
        let f = src.alloc(LEN, true);
        assert_eq!(f, b, "the block given last");
        // SAFETY: a block of `LEN` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(f, LEN) };
        assert!(bytes.iter().all(|&x| x == 0), "zeroed where asked");
        // No block of a length it does not hold, longer or shorter: a
        // longer block given out would be freed at the shorter length.
        let g = src.alloc(2 * LEN, false);
        assert_eq!(pool.counts().1, 2, "no block of twice the length");
        let short = src.alloc(2 * MAP_FROM, false);
        assert_eq!(pool.counts().1, 2, "no block of a shorter length");
        // SAFETY: as above.
        unsafe { src.free(short, 2 * MAP_FROM) };
        assert_eq!(pool.counts().2, 5, "the short block held at its own length");
        let again = src.alloc(2 * MAP_FROM, false);
        assert_eq!(again, short, "and given back at it");
        assert_eq!(
            pool.misses(),
            (5, 2 * LEN + MAP_FROM + 2 * LEN + 2 * MAP_FROM),
            "the lengths it held none of, mapped fresh, and none it held"
        );
        // SAFETY: as above.
        unsafe { src.free(again, 2 * MAP_FROM) };
        // A block too small to map, and a source that does not map.
        let small = Source::new(true, Some(pool.clone()));
        let h = small.alloc(64, false);
        let unmapped = Source::new(false, Some(pool.clone()));
        let i = unmapped.alloc(LEN, false);
        assert_eq!(pool.misses().0, 5, "a block the pool never sees is no miss");
        let held = pool.counts();
        // SAFETY: as above.
        unsafe {
            small.free(h, 64);
            unmapped.free(i, LEN);
        }
        assert_eq!(pool.counts(), held, "neither reaches the pool");
        // Off: nothing kept, and what it held gone back.
        pool.set_on(false);
        assert!(!pool.is_on());
        assert_eq!(pool.counts().0, 0, "the held blocks unmapped");
        // SAFETY: as above.
        unsafe {
            src.free(f, LEN);
            src.free(g, 2 * LEN);
        }
        assert_eq!(pool.counts().0, 0, "nothing kept while off");
        pool.set_on(true);
        let j = src.alloc(LEN, false);
        // SAFETY: as above.
        unsafe { src.free(j, LEN) };
        assert_eq!(pool.counts().0, LEN, "on again, held again");
        drop(src);
        drop(pool);
    }

    /// A `BigVec` holds what a `Vec` fed the same calls holds, through
    /// every growth: a `Vec` itself until its buffer would be `MAP_FROM`
    /// bytes, then a mapping through the mapping's own growths where the
    /// setting says so, and a `Vec` to the end where it does not.
    #[test]
    fn a_bigvec_is_a_vec_through_every_growth() {
        for mapped in [true, false] {
            let mut b: BigVec<(u64, u64, u32)> = BigVec::new(Site::Test, mapped);
            let mut v: Vec<(u64, u64, u32)> = Vec::new();
            assert!(b.is_empty() && !b.is_mapped() && b.capacity() == 0);
            assert_eq!(&b[..], &v[..]);
            let mut was_heap = false;
            let mut grew_mapped = 0;
            for i in 0..40_000u64 {
                let before = (b.capacity(), b.is_mapped());
                b.push((i, !i, i as u32));
                v.push((i, !i, i as u32));
                if b.capacity() != before.0 {
                    if before.1 {
                        grew_mapped += 1;
                    }
                    was_heap |= !b.is_mapped();
                    assert_eq!(&b[..], &v[..], "after a growth to {}", b.capacity());
                    assert_eq!(
                        b.is_mapped(),
                        maps(b.capacity() * 24, mapped),
                        "a mapping from MAP_FROM on, and only there"
                    );
                    if !b.is_mapped() {
                        assert_eq!(b.capacity(), v.capacity(), "a `Vec` while it is one");
                    }
                }
            }
            assert_eq!(&b[..], &v[..]);
            assert_eq!(b.is_mapped(), mapped, "960 KB of values");
            assert_eq!(
                live()
                    .iter()
                    .any(|&(a, n)| a == b.as_ptr() as usize && n == b.capacity() * 24),
                mapped,
                "a mapped buffer is listed for the resident count"
            );
            assert!(was_heap, "the first buffers are the allocator's");
            if mapped {
                // Grown as one, and by doubling: three growths from the
                // eight thousand values it was mapped with to these, where
                // a growth a push would be thirty thousand moves.
                assert!(
                    (2..=4).contains(&grew_mapped),
                    "a mapping grown as one, by doubling: {grew_mapped}"
                );
            }
            b.extend_from_slice(&v[..1000]);
            v.extend_from_within(..1000);
            assert_eq!(&b[..], &v[..]);
            b.resize(120_000, (7, 7, 7));
            v.resize(120_000, (7, 7, 7));
            assert_eq!(&b[..], &v[..], "a resize past the room");
            b.reserve(200_000);
            assert!(b.capacity() >= 320_000 && b.len() == 120_000);
            assert_eq!(&b[..], &v[..], "a reserve keeps the values");
            b[3].0 = 99;
            v[3].0 = 99;
            b.resize(10, (0, 0, 0));
            v.resize(10, (0, 0, 0));
            assert_eq!(&b[..], &v[..], "a resize that cuts short");
            let cap = b.capacity();
            b.clear();
            assert!(
                b.is_empty() && b.capacity() == cap,
                "clear keeps the buffer"
            );
            b.push((1, 2, 3));
            assert_eq!(&b[..], &[(1, 2, 3)]);
        }
        // Room asked for up front is one buffer of that size, a mapping
        // from the start where it is large, and nothing of it is touched;
        // and each way into a mapping from a `Vec` -- a slice appended, a
        // resize, a reserve -- carries the values across.
        let big: BigVec<u64> = BigVec::with_capacity(MAP_FROM / 8, Site::Test, true);
        assert!(big.is_mapped() && big.capacity() == MAP_FROM / 8 && big.is_empty());
        let small: BigVec<u8> = BigVec::with_capacity(100, Site::Test, true);
        assert!(!small.is_mapped() && small.capacity() >= 100);
        let seed: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let mut by_slice: BigVec<u8> = BigVec::new(Site::Test, true);
        by_slice.extend_from_slice(&seed);
        assert!(!by_slice.is_mapped());
        by_slice.extend_from_slice(&vec![9u8; MAP_FROM]);
        assert!(by_slice.is_mapped() && by_slice.len() == 1000 + MAP_FROM);
        assert!(by_slice[..1000] == seed[..] && by_slice[1000..].iter().all(|&x| x == 9));
        let mut by_resize: BigVec<u8> = BigVec::new(Site::Test, true);
        by_resize.extend_from_slice(&seed);
        by_resize.resize(MAP_FROM, 5);
        assert!(by_resize.is_mapped() && by_resize.len() == MAP_FROM);
        assert!(by_resize[..1000] == seed[..] && by_resize[1000..].iter().all(|&x| x == 5));
        let mut by_reserve: BigVec<u8> = BigVec::new(Site::Test, true);
        by_reserve.extend_from_slice(&seed);
        by_reserve.reserve(MAP_FROM);
        assert!(by_reserve.is_mapped() && by_reserve.capacity() >= 1000 + MAP_FROM);
        assert!(by_reserve[..] == seed[..]);
    }

    /// A slab hands out as many cells as it has and no more, each its own
    /// range of the block, aligned and as long as asked or longer; a cell
    /// handed back is the next handed out, with what its holder left in
    /// it; and the block outlives the slab for as long as a cell is held.
    #[test]
    fn a_slabs_cells_are_disjoint_and_come_back() {
        let slab = Slab::new(2240, 300);
        let mut cells: Vec<SlabCell> = (0..300).map(|_| slab.take().unwrap()).collect();
        assert!(slab.take().is_none(), "every cell is out");
        let mut at: Vec<usize> = cells.iter().map(|c| c.as_ptr() as usize).collect();
        at.sort_unstable();
        assert!(at.iter().all(|a| a % ALIGN == 0), "aligned to a cache line");
        assert!(
            cells
                .iter()
                .all(|c| c.len() >= 2240 && c.len() % ALIGN == 0),
            "as long as asked, in whole cache lines"
        );
        assert!(
            at.windows(2).all(|w| w[1] - w[0] >= cells[0].len()),
            "no cell overlaps the next"
        );
        for (n, c) in cells.iter().enumerate() {
            // SAFETY: the cell's own `len` bytes, held by this test alone.
            let s = unsafe { std::slice::from_raw_parts_mut(c.as_ptr(), c.len()) };
            assert!(s.iter().all(|&b| b == 0), "zero at its first use");
            s.fill(n as u8);
        }
        for (n, c) in cells.iter().enumerate() {
            // SAFETY: as above.
            let s = unsafe { std::slice::from_raw_parts(c.as_ptr(), c.len()) };
            assert!(s.iter().all(|&b| b == n as u8), "cell {n} kept its bytes");
        }
        let back = cells.pop().unwrap();
        let (was, fill) = (back.as_ptr() as usize, 299usize as u8);
        drop(back);
        let again = slab.take().expect("the cell handed back");
        assert_eq!(again.as_ptr() as usize, was, "handed out again");
        // SAFETY: the cell's own bytes.
        assert_eq!(unsafe { *again.as_ptr() }, fill, "as its holder left it");
        assert!(slab.take().is_none());
        // The slab gone, a held cell still reads and writes its bytes.
        drop(slab);
        // SAFETY: the cell's own bytes, the block alive while it is held.
        unsafe { again.as_ptr().write(7) };
        assert_eq!(unsafe { *again.as_ptr() }, 7);
        drop(again);
        drop(cells);
        // A slab under `MAP_FROM` is the allocator's, and works the same.
        let small = Slab::new(100, 3);
        let a = small.take().unwrap();
        let b = small.take().unwrap();
        assert!(a.len() == 128 && b.as_ptr() as usize - a.as_ptr() as usize == 128);
    }

    /// A block grown keeps its bytes, mapping to mapping, allocator to
    /// mapping and allocator to allocator.
    #[test]
    fn a_block_grown_keeps_its_bytes() {
        // Lengths no other test's blocks have, since the registry is the
        // process's and an address a block left is the next one's to take.
        for &(old, new, mapped) in &[
            (MAP_FROM + 3 * 4096, 4 * MAP_FROM + 5 * 4096, true),
            (4096usize + 64, MAP_FROM + 7 * 4096, true),
            (4096 + 128, 8192 + 64, true),
            (MAP_FROM + 9 * 4096, 2 * MAP_FROM + 11 * 4096, false),
        ] {
            let p = alloc(old, false, mapped);
            // SAFETY: `old` bytes just allocated.
            let s = unsafe { std::slice::from_raw_parts_mut(p, old) };
            for (i, b) in s.iter_mut().enumerate() {
                *b = (i % 251) as u8;
            }
            // SAFETY: from `alloc` with these arguments, `p` read no more.
            let q = unsafe { regrow(p, old, new, old, mapped) };
            assert_eq!(q as usize % ALIGN, 0);
            // The registry the resident count walks names the block as it
            // is now, and no longer as it was.
            assert_eq!(
                live().iter().any(|&(a, n)| a == q as usize && n == new),
                maps(new, mapped),
                "{old} -> {new} {mapped}: the grown block listed"
            );
            assert!(
                !live().iter().any(|&(a, n)| a == p as usize && n == old),
                "{old} -> {new} {mapped}: the old block still listed"
            );
            // SAFETY: `new` bytes, the first `old` written.
            let t = unsafe { std::slice::from_raw_parts_mut(q, new) };
            assert!(
                t[..old]
                    .iter()
                    .enumerate()
                    .all(|(i, &b)| b == (i % 251) as u8),
                "{old} -> {new} {mapped}"
            );
            t[new - 1] = 1;
            // SAFETY: from `regrow` with this length, read no more.
            unsafe { free(q, new, mapped) };
        }
    }

    /// A mapped block's resident bytes are the pages it has touched, read
    /// from the kernel: none fresh, at least the touched ones after, and
    /// never more than the block; and the live blocks' sum counts it while
    /// it lives. The sum is the process's, moved by every other test's
    /// blocks, so it is held to no less than this block's and no more.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_mapped_blocks_resident_bytes_are_the_pages_it_touched() {
        let len = 64 * 4096;
        let p = alloc(len, false, true);
        assert_eq!(resident_of(p, len), 0, "fresh pages are resident nowhere");
        // SAFETY: `len` bytes just allocated.
        let s = unsafe { std::slice::from_raw_parts_mut(p, len) };
        for i in 0..16 {
            s[i * 4096] = 1;
        }
        let r = resident_of(p, len);
        assert!(
            (16 * 4096..=len).contains(&r),
            "sixteen pages touched: {r} resident"
        );
        assert!(
            mapped_resident_bytes() >= r,
            "the live blocks' sum holds it"
        );
        // SAFETY: from `alloc` with these arguments, read no more.
        unsafe { free(p, len, true) };
        // A range no mapping holds is resident nowhere: the page at address
        // zero, which the kernel gives no mapping (`vm.mmap_min_addr`). Not
        // the range just freed, which another thread of this binary may be
        // given by its next mapping before the count is read -- seventeen
        // pages of someone else's were, once in a gate's stride group.
        assert_eq!(
            resident_of(std::ptr::null_mut(), 4096),
            0,
            "an unmapped range is resident nowhere"
        );
    }
}
