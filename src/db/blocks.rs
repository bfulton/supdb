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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A block of at least this many bytes is mapped when the option says so;
/// glibc's own threshold starts here, before its first large free raises
/// it.
pub(crate) const MAP_FROM: usize = 128 << 10;

/// Every block's alignment, mapped or not: a cache line.
const ALIGN: usize = 64;

static MAPPED: AtomicUsize = AtomicUsize::new(0);

/// The `BigVec` buffers that became mappings, process-wide, ever: what a
/// test holds `Options::jobs_mapped` to having done.
static BIG_MAPS: AtomicUsize = AtomicUsize::new(0);

/// The count of `BigVec` buffers that have become mappings.
pub(crate) fn big_maps() -> usize {
    BIG_MAPS.load(Ordering::Relaxed)
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
pub(crate) unsafe fn regrow(p: *mut u8, old: usize, new: usize, used: usize, mapped: bool) -> *mut u8 {
    debug_assert!(new > old && used <= old, "a block grown");
    #[cfg(target_os = "linux")]
    {
        if maps(old, mapped) && maps(new, mapped) {
            // SAFETY: `p` is a mapping of `old` bytes made by `map` or by
            // this; the kernel moves it where it cannot grow in place.
            let q = unsafe {
                libc::mremap(p as *mut libc::c_void, old, new, libc::MREMAP_MAYMOVE)
            };
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

/// A job's large array: a vector of plain values that grows as `Vec`
/// does and whose buffer, from `MAP_FROM` bytes on and where `mapped`
/// says so, is a mapping of the engine's own, unmapped at its drop
/// (`Options::jobs_mapped`). A seal's sort, a segment's writer, an ordered
/// index's builder and a merge's key list each hold tens of bytes a key
/// for as long as the job runs and nothing after it, on a thread that
/// ends with the job: from the allocator they stay in that thread's
/// arena, whose top glibc keeps up to twice the largest chunk the process
/// has mapped and freed, resident and nobody's, tens of megabytes an
/// arena under a memory bound that has page cache to hold with them
/// (`docs/engine.md`, which form a workload calls for). Below `MAP_FROM`
/// the buffer is the allocator's whatever the option, as a block's is. A
/// mapping grows by `mremap`, its pages moved and never copied, and what
/// of it was never written is resident nowhere.
pub(crate) struct BigVec<T: Copy> {
    p: std::ptr::NonNull<T>,
    len: usize,
    cap: usize,
    mapped: bool,
}

// SAFETY: a `BigVec` owns its buffer of plain values, as a `Vec` does.
unsafe impl<T: Copy + Send> Send for BigVec<T> {}
// SAFETY: as above; a shared borrow reads only.
unsafe impl<T: Copy + Sync> Sync for BigVec<T> {}

impl<T: Copy> BigVec<T> {
    /// A value with a size, aligned within a block's alignment.
    const FITS: () = assert!(std::mem::size_of::<T>() > 0 && std::mem::align_of::<T>() <= ALIGN);

    /// The smallest buffer made, in bytes: a cache line.
    const MIN_BYTES: usize = ALIGN;

    /// No buffer yet; `mapped` is whether a large one will be a mapping.
    pub(crate) fn new(mapped: bool) -> BigVec<T> {
        let () = Self::FITS;
        BigVec {
            p: std::ptr::NonNull::dangling(),
            len: 0,
            cap: 0,
            mapped,
        }
    }

    /// Room for `n` values, none of it touched.
    pub(crate) fn with_capacity(n: usize, mapped: bool) -> BigVec<T> {
        let mut v = BigVec::new(mapped);
        v.reserve(n);
        v
    }

    /// The values the buffer has room for.
    pub(crate) fn capacity(&self) -> usize {
        self.cap
    }

    /// Whether the buffer is a mapping as it stands.
    pub(crate) fn is_mapped(&self) -> bool {
        self.cap > 0 && maps(self.cap * std::mem::size_of::<T>(), self.mapped)
    }

    /// Room for `more` values past the length.
    pub(crate) fn reserve(&mut self, more: usize) {
        let want = self.len.checked_add(more).expect("BigVec: capacity overflow");
        if want > self.cap {
            self.grow(want);
        }
    }

    /// The buffer made to hold at least `want` values, and at least twice
    /// what it held, so pushes are amortised as a `Vec`'s are.
    #[cold]
    fn grow(&mut self, want: usize) {
        let size = std::mem::size_of::<T>();
        let cap = want
            .max(self.cap.saturating_mul(2))
            .max(Self::MIN_BYTES.div_ceil(size));
        let bytes = cap.checked_mul(size).expect("BigVec: capacity overflow");
        if maps(bytes, self.mapped) && !maps(self.cap * size, self.mapped) {
            BIG_MAPS.fetch_add(1, Ordering::Relaxed);
        }
        let p = if self.cap == 0 {
            alloc(bytes, false, self.mapped)
        } else {
            // SAFETY: the buffer is `self.cap * size` bytes from `alloc`
            // or `regrow` under this `mapped`, its first `self.len`
            // values written, and `self.p` is replaced below.
            unsafe {
                regrow(
                    self.p.as_ptr() as *mut u8,
                    self.cap * size,
                    bytes,
                    self.len * size,
                    self.mapped,
                )
            }
        };
        self.p = std::ptr::NonNull::new(p as *mut T).expect("a block is never null");
        self.cap = cap;
    }

    #[inline]
    pub(crate) fn push(&mut self, v: T) {
        if self.len == self.cap {
            self.grow(self.len + 1);
        }
        // SAFETY: `self.len < self.cap`, inside the buffer.
        unsafe { self.p.as_ptr().add(self.len).write(v) };
        self.len += 1;
    }

    #[inline]
    pub(crate) fn extend_from_slice(&mut self, s: &[T]) {
        self.reserve(s.len());
        // SAFETY: room for `s.len()` values past the length was just
        // reserved, and `s` borrows nothing of this buffer mutably.
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), self.p.as_ptr().add(self.len), s.len());
        }
        self.len += s.len();
    }

    /// `n` values long: cut short, or filled out with `v`.
    pub(crate) fn resize(&mut self, n: usize, v: T) {
        if n > self.len {
            self.reserve(n - self.len);
            for i in self.len..n {
                // SAFETY: `i < n <= self.cap`.
                unsafe { self.p.as_ptr().add(i).write(v) };
            }
        }
        self.len = n;
    }

    /// No values, the buffer kept.
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }

    /// At most `n` values.
    pub(crate) fn truncate(&mut self, n: usize) {
        self.len = self.len.min(n);
    }
}

impl<T: Copy> std::ops::Deref for BigVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        // SAFETY: the first `self.len` values are written; with no buffer
        // the pointer is dangling and aligned and the length zero.
        unsafe { std::slice::from_raw_parts(self.p.as_ptr(), self.len) }
    }
}

impl<T: Copy> std::ops::DerefMut for BigVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: as `deref`, and the borrow is exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.p.as_ptr(), self.len) }
    }
}

impl<T: Copy> Drop for BigVec<T> {
    fn drop(&mut self) {
        if self.cap > 0 {
            // SAFETY: the buffer is `self.cap` values from `alloc` or
            // `regrow` under this `mapped`, and nothing reads it after.
            unsafe {
                free(
                    self.p.as_ptr() as *mut u8,
                    self.cap * std::mem::size_of::<T>(),
                    self.mapped,
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
    /// every growth: from no buffer, through the allocator's, across
    /// `MAP_FROM` into a mapping and through the mapping's own growths,
    /// under either setting; and it is a mapping exactly from `MAP_FROM`
    /// bytes on where the setting says so.
    #[test]
    fn a_bigvec_is_a_vec_through_every_growth() {
        for mapped in [true, false] {
            let mut b: BigVec<(u64, u64, u32)> = BigVec::new(mapped);
            let mut v: Vec<(u64, u64, u32)> = Vec::new();
            assert!(b.is_empty() && !b.is_mapped() && b.capacity() == 0);
            assert_eq!(&b[..], &v[..]);
            let mut was_heap = false;
            let mut grew_mapped = 0;
            let mut last_cap = 0;
            for i in 0..40_000u64 {
                b.push((i, !i, i as u32));
                v.push((i, !i, i as u32));
                if b.capacity() != last_cap {
                    if b.is_mapped() && last_cap * 24 >= MAP_FROM {
                        grew_mapped += 1;
                    }
                    was_heap |= !b.is_mapped();
                    last_cap = b.capacity();
                    assert_eq!(&b[..], &v[..], "after a growth to {last_cap}");
                }
            }
            assert_eq!(&b[..], &v[..]);
            assert_eq!(b.is_mapped(), mapped, "960 KB of values");
            assert_eq!(
                live()
                    .iter()
                    .any(|&(a, n)| a == b.p.as_ptr() as usize && n == b.capacity() * 24),
                mapped,
                "a mapped buffer is listed for the resident count"
            );
            assert_eq!(
                b.is_mapped(),
                maps(b.capacity() * 24, mapped),
                "a mapping from MAP_FROM on"
            );
            assert!(was_heap, "the first buffers are the allocator's");
            if mapped {
                assert!(grew_mapped >= 2, "a mapping grown as one: {grew_mapped}");
            }
            b.extend_from_slice(&v[..1000]);
            v.extend_from_within(..1000);
            assert_eq!(&b[..], &v[..]);
            b.resize(50_000, (7, 7, 7));
            v.resize(50_000, (7, 7, 7));
            assert_eq!(&b[..], &v[..]);
            b[3].0 = 99;
            v[3].0 = 99;
            b.truncate(10);
            v.truncate(10);
            assert_eq!(&b[..], &v[..]);
            b.truncate(20);
            assert_eq!(b.len(), 10, "truncate never lengthens");
            b.resize(5, (0, 0, 0));
            assert_eq!(b.len(), 5);
            let cap = b.capacity();
            b.clear();
            assert!(b.is_empty() && b.capacity() == cap, "clear keeps the buffer");
        }
        // Room asked for up front is one buffer of that size, a mapping
        // from the start where it is large, and nothing of it is touched.
        let big: BigVec<u64> = BigVec::with_capacity(MAP_FROM / 8, true);
        assert!(big.is_mapped() && big.capacity() == MAP_FROM / 8 && big.is_empty());
        let small: BigVec<u8> = BigVec::with_capacity(100, true);
        assert!(!small.is_mapped() && small.capacity() >= 100);
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
                t[..old].iter().enumerate().all(|(i, &b)| b == (i % 251) as u8),
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
