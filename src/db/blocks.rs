//! Where the engine's large blocks come from: a memtable's arena blocks,
//! its entry slabs and its hash index, and a scan snapshot's arena blocks.
//!
//! By `Options::blocks_mapped` a block of `MAP_FROM` bytes or more is
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
//! the allocator whatever the option, since a mapping costs a system call
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

/// A block of at least this many bytes is mapped when the option says so;
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
fn resident_of(p: *mut u8, len: usize) -> usize {
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
    mapped: bool,
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
    pub(crate) fn new(site: Site, mapped: bool) -> BigVec<T> {
        let () = Self::FITS;
        BigVec {
            heap: Vec::new(),
            map: std::ptr::null_mut(),
            len: 0,
            cap: 0,
            mapped,
            site,
        }
    }

    /// Room for `n` values, none of it touched: a mapping from the start
    /// where that is large and `mapped`.
    pub(crate) fn with_capacity(n: usize, site: Site, mapped: bool) -> BigVec<T> {
        let mut v = BigVec::new(site, mapped);
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
        let p = alloc(bytes, false, true) as *mut T;
        // SAFETY: `p` is `cap` values just mapped, `cap` at least the
        // `Vec`'s length by every caller's choice of it, and apart from
        // the `Vec`'s buffer.
        unsafe { std::ptr::copy_nonoverlapping(self.heap.as_ptr(), p, self.heap.len()) };
        self.len = self.heap.len();
        self.cap = cap;
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
        // SAFETY: the mapping is `self.cap * size` bytes from `alloc` or
        // `regrow`, mapped, its first `self.len` values written, and
        // `self.map` is replaced below.
        let p = unsafe {
            regrow(
                self.map as *mut u8,
                self.cap * size,
                bytes,
                self.len * size,
                true,
            )
        };
        self.map = p as *mut T;
        self.cap = cap;
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
            // SAFETY: the mapping is `self.cap` values from `alloc` or
            // `regrow`, mapped, and nothing reads it after.
            unsafe {
                free(
                    self.map as *mut u8,
                    self.cap * std::mem::size_of::<T>(),
                    true,
                )
            };
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            resident_of(p, len),
            0,
            "an unmapped range is resident nowhere"
        );
    }
}
