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
