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
//! leak test holds the count to zero past every store's close, the
//! resident count beyond the allocator's use takes them out, and the suite
//! prints them beside the allocator's bytes in use.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A block of at least this many bytes is mapped when the option says so;
/// glibc's own threshold starts here, before its first large free raises
/// it.
pub(crate) const MAP_FROM: usize = 128 << 10;

/// Every block's alignment, mapped or not: a cache line.
const ALIGN: usize = 64;

static MAPPED: AtomicUsize = AtomicUsize::new(0);

/// The bytes of the blocks mapped and not yet unmapped, process-wide.
pub(crate) fn mapped_bytes() -> usize {
    MAPPED.load(Ordering::Relaxed)
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
}
