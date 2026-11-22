//! Where a reader's bytes come from.
//!
//! The read path used to take `&Mmap` and hand back `&[Ext]` borrowed straight
//! out of it. That borrow is the entire point of `flatindex` -- an index that
//! is *addressed* where it lies rather than decoded -- and it is the thing a
//! byte-source abstraction is most likely to destroy, because the obvious
//! trait ("give me these bytes in this buffer") forces a copy on every path
//! including the one that had none.
//!
//! So the trait has two halves. `read_at` is the one every source can answer
//! and is what a browser has: a synchronous positional read into a caller's
//! buffer. `slice_at` is the one a source backed by memory can answer, and it
//! lends the bytes instead of copying them. A reader over a mapping takes the
//! second path for every access and copies nothing; a reader over an OPFS
//! file handle takes the first, and pays a copy per section rather than per
//! lookup, because it reads whole sections once at open.
//!
//! ## Why this is synchronous
//!
//! `flatindex::lookup` returns a borrow into the index section. A borrow
//! cannot survive an `await`, and an async trait method would infect every
//! caller up to the top. The browser side resolves this outside Rust: JS
//! downloads the object into the Origin Private File System once,
//! asynchronously, and thereafter `FileSystemSyncAccessHandle.read(buf, {at})`
//! is a synchronous random read. That costs one full download per index,
//! which was measured and accepted, and requires the reader to run
//! in a Web Worker, since sync access handles are not available on the main
//! thread. Both were acceptable, so the API needed no shape change.
//!
//! ## Endianness
//!
//! Every scalar in the file is written little-endian, but the zero-copy read
//! path reinterprets an extent array as `&[Ext]`, which is native-endian. The
//! two agree only on a little-endian machine, so `Blob::open` refuses a
//! big-endian target explicitly rather than misreading a file there. Every
//! browser is little-endian and so is every machine the suite has run on, so
//! this costs nothing and is checked rather than assumed.

// The door (scripts/door/clippy.toml, tests/data/door.txt): the mapped byte source and its advice.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::io::{Error, ErrorKind, Result};

/// The read path's token, which `Bytes::lend_at` takes and only `blob` makes.
use crate::blob::lend_seal::Token as LendToken;
/// The stride form's token, which `MmapBytes::lend_whole_map` takes and
/// only `db::zfile` makes.
#[cfg(not(target_family = "wasm"))]
use crate::db::StrideLendToken;
/// The ordered index's token, which `MmapBytes::lend_whole_map` takes and
/// only `ordindex` makes.
#[cfg(not(target_family = "wasm"))]
use crate::ordindex::OrdLendToken;

/// The readers of a mapping as it lies, one token each; a trait in a
/// private module, so no module but this one can give it to a type, and
/// `MmapBytes::lend_whole_map` takes a token of these readers and of
/// nobody else.
#[cfg(not(target_family = "wasm"))]
mod whole_map_readers {
    pub trait Reader {}
}
#[cfg(not(target_family = "wasm"))]
impl whole_map_readers::Reader for OrdLendToken {}
#[cfg(not(target_family = "wasm"))]
impl whole_map_readers::Reader for StrideLendToken {}

/// A synchronous, random-access source of bytes.
///
/// Implementors must be consistent for the life of the reader: supdb's file
/// format is written once and sealed, and a reader that saw the header change
/// underneath it would be reading two different files.
pub trait Bytes {
    /// Total length of the object.
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fill `dst` from `off`. Short reads are an error, not a partial fill:
    /// every caller here has already decided how many bytes it needs from a
    /// length in the file, so a short read means the file is not what it says.
    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()>;

    /// Borrow `off..off+len` if these bytes are contiguous in memory.
    ///
    /// `None` is always a correct answer and means "copy it with `read_at`".
    /// A source that can answer this makes the whole read path zero-copy, and
    /// that is not an optimisation detail -- `flatindex` exists to win on
    /// exactly this axis, and a trait that forced a copy on the native path
    /// would be a regression rather than a refactor.
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let _ = (off, len);
        None
    }

    /// A section lent whole, for a reader that borrows it once and
    /// addresses into it: `slice_at` for a source that lends whole, and
    /// nothing for one that lends by the piece, where the borrow would be
    /// the section made present whole. A reader asks this of its key
    /// section at every lookup and goes by the piece where it is refused
    /// (`flatindex::Pieces`), so for a mapped file it is a range check and
    /// nothing else.
    ///
    /// Sealed: the token is the read path's (`LendToken`,
    /// `blob::lend_seal`), whose module alone makes one, so the read path's
    /// dispatch is the one caller of this, by the compiler, and no source
    /// outside the crate can write the method to override it -- a reader
    /// written elsewhere asks with `slice_at`, which every kind of source
    /// answers. The token is a
    /// zero-sized value and costs the call nothing.
    #[inline]
    fn lend_at(&self, off: u64, len: usize, _: LendToken) -> Option<&[u8]> {
        if self.piecewise() {
            return None;
        }
        self.slice_at(off, len)
    }

    /// Whether this source lends by the piece: `slice_at` answers, and what
    /// it lends is made present by the asking, so a reader asks for the
    /// bytes it is about to read and for no more. A reader over a source
    /// that lends whole borrows a section once and addresses into it; over
    /// one that lends by the piece that borrow would ask for the section
    /// whole at every lookup, which for a source that decrypts what it is
    /// asked for is the section decrypted. `Blob` holds such a source's
    /// sections by offset and reads them through `flatindex::Pieces`.
    fn piecewise(&self) -> bool {
        false
    }

    /// Whether every byte this source lends has passed a check of the
    /// source's own -- an authenticating cipher's tag over the chunk it
    /// lies in -- before it is lent. `Blob::open` reads this: over a source
    /// that lends by the piece it skips the key section's checksum row
    /// where the answer is yes, and verifies the row a piece at a time
    /// where it is no. The ordered index (`OrdIndex::open`) skips its
    /// checksum over any source that lends by the piece without asking
    /// this, since its one such source is a paged mapping (`vault::map`),
    /// which answers yes. `false` is always a correct answer.
    fn authenticated(&self) -> bool {
        false
    }

    /// Whether a reader that has not been told has to ask `piecewise` of a
    /// source of this type. Every type says yes but the mapped file, whose
    /// kind that lends by the piece is made by the engine alone
    /// (`MmapBytes::paged`) and read by it through the entry that finds
    /// the kind out for nothing (`Blob::scan_either`): asked at
    /// `Blob::scan_at`, the question was three instructions a scan of a
    /// mapped file that no caller's mapped file can answer yes to.
    const ASK_PIECES: bool = true;

    /// Where the piece holding `off` ends, for a source that lends by the
    /// piece: a reader that does not know how long the thing at `off` is
    /// asks to here first, and for more only when that fell short. The
    /// default is a page's worth.
    fn piece_end(&self, off: u64) -> u64 {
        off.saturating_add(4096).min(self.len())
    }

    /// The bytes from `off` to the end of the piece holding it, lent: what
    /// `slice_at(off, piece_end(off) - off)` lends, in one ask of a source
    /// for which the two are one look at one piece.
    fn piece_at(&self, off: u64) -> Option<&[u8]> {
        let end = self.piece_end(off);
        self.slice_at(off, usize::try_from(end.checked_sub(off)?).ok()?)
    }

    /// Hint that access will be random.
    ///
    /// A no-op where the target has no mapping to advise, which is every
    /// non-native source -- not an error. There is nothing to tell an OPFS
    /// file handle about page-fault patterns.
    fn advise_random(&self) {}

    /// This source's pages unmapped from the process (`MADV_DONTNEED`), so
    /// that a drop of the file's page cache after it can take them: a cache
    /// drop skips a page a mapping holds. The bytes do not change; a later
    /// touch faults the page back from the cache or the file. A no-op where
    /// there is no mapping.
    fn advise_dontneed(&self) {}

    /// Fetch `len` bytes at `off` in the background, now.
    ///
    /// `MADV_WILLNEED` on a mapped file: not a hint about a pattern but an
    /// explicit asynchronous read of exactly those bytes, which is the one
    /// thing a reader can tell the kernel that the kernel cannot work out --
    /// where the span it is about to walk ends.
    ///
    /// Returns the span actually submitted rather than nothing, because an
    /// implementation may clamp: a caller counting what it *asked* for would
    /// report bytes that were never submitted, and this reader's whole
    /// argument is that its numbers say what happened. Zero on any source
    /// with no mapping behind it.
    fn advise_willneed(&self, _off: u64, _len: usize) -> usize {
        0
    }

    /// Undo `advise_random`: back to the kernel's default readahead.
    ///
    /// The pair exists so a reader can follow its workload rather than pick a
    /// side once; whether that is worth doing is a measurement, not a given. A
    /// no-op wherever `advise_random` is one.
    fn advise_normal(&self) {}

    /// Whether the page holding `off` is in the page cache now (`mincore`),
    /// `None` where the source cannot say: a mapping with no page cache
    /// behind it, or no mapping at all. The one question a reader asks
    /// before `advise_willneed`: an ask over pages already resident walks
    /// them in the kernel for nothing, ten to twenty microseconds a scan
    /// whose pages a settle had just pulled in, where a scan that hits is
    /// five.
    fn resident_at(&self, _off: u64) -> Option<bool> {
        None
    }

    /// The share of `len` bytes at `off` a read would find in memory now,
    /// from 0 to 1, `None` where the source cannot say: a mapping with no
    /// page cache behind it, no mapping at all, or, for a mapped file, a
    /// kernel that will not answer; a paged mapping always answers. A
    /// mapped file answers by `mincore` over the pages the range
    /// lies in, clamped to the file, each page a byte of it lies in
    /// counting, the first from its start; a paged mapping by what its
    /// pager holds present or the kernel holds of the file behind it
    /// (`Paging::resident`). Zero for a range that is empty or past the
    /// end. The question a whole range asks, where `resident_at` asks it
    /// of a page: whether a warm of a file can skip a span, or a partition
    /// is read from memory or the device.
    fn resident_share(&self, _off: u64, _len: usize) -> Option<f64> {
        None
    }

    /// The pages holding `len` bytes at `off` unmapped from the process
    /// (`MADV_DONTNEED` over the whole pages inside the range), so that a
    /// drop of the file's page cache behind them can take them: a merge
    /// streams its inputs once and never again, and the pages it has read
    /// would otherwise stand in the cache as the newest, pushing out what
    /// the readers beside it are using. The bytes do not change; a later
    /// touch faults the page back. A no-op where there is no mapping.
    fn advise_dontneed_range(&self, _off: u64, _len: usize) {}
}

/// Read `len` bytes at `off`, borrowing when the source allows it.
///
/// The one helper every caller wants: it takes the zero-copy path when there
/// is one and falls back to filling `scratch` when there is not, without the
/// caller branching on which.
pub fn take<'a, B: Bytes + ?Sized>(
    src: &'a B,
    off: u64,
    len: usize,
    scratch: &'a mut Vec<u8>,
) -> Result<&'a [u8]> {
    if src.slice_at(off, len).is_some() {
        // Re-borrowed rather than returned from the `if`, because the borrow
        // checker cannot see that the `Some` arm does not also need `scratch`.
        return Ok(src.slice_at(off, len).unwrap());
    }
    scratch.clear();
    scratch.resize(len, 0);
    src.read_at(off, scratch)?;
    Ok(&scratch[..])
}

pub(crate) fn short(off: u64, want: usize, have: u64) -> Error {
    Error::new(
        ErrorKind::UnexpectedEof,
        format!("wanted {want} bytes at {off} of a {have}-byte object"),
    )
}

// ------------------------------------------------------------------ memory --

/// Bytes already in memory: a downloaded object, a test fixture, a `Vec` the
/// host handed over. Zero-copy, because there is nothing to copy from.
pub struct SliceBytes<'a>(pub &'a [u8]);

impl Bytes for SliceBytes<'_> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
        let end = off
            .checked_add(dst.len() as u64)
            .ok_or_else(|| short(off, dst.len(), self.len()))?;
        if end > self.len() {
            return Err(short(off, dst.len(), self.len()));
        }
        dst.copy_from_slice(&self.0[off as usize..end as usize]);
        Ok(())
    }
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.len() {
            return None;
        }
        self.0.get(off as usize..end as usize)
    }
}

/// The same, owning its bytes.
pub struct VecBytes(pub Vec<u8>);

impl Bytes for VecBytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
        SliceBytes(&self.0).read_at(off, dst)
    }
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.len() {
            return None;
        }
        self.0.get(off as usize..end as usize)
    }
}

// ------------------------------------------------------------------ mapping --

/// A memory mapping. The native source, and the only one that is zero-copy
/// without first being read into memory.
///
/// Two kinds, behind one type so that a reader over either is one reader.
/// A mapped file lends any range of itself with a bounds check and nothing
/// else. A paged mapping (`Paging`) is address space the length of a file
/// whose bytes are made present by the asking: it lends nothing unasked --
/// its lendable length is zero, so every `slice_at` fails the bounds check
/// a mapped file's passes and goes round by `slice_slow`, which asks the
/// pager for the range and then lends it. The check a mapped file pays is
/// the one it paid when this type was a mapped file and nothing else.
#[cfg(not(target_family = "wasm"))]
pub struct MmapBytes {
    ptr: *const u8,
    /// The bytes lent on the bounds check alone: all of a mapped file's,
    /// none of a paged mapping's.
    lend: u64,
    len: u64,
    own: Own,
}

// Two kinds and no third: `MmapBytes::map` answers `None` for every kind
// but `Map`, and `Bytes::authenticated` reads that `None` as "a paged
// mapping, which lends only what its pager has checked". A third kind that
// answered `None` there would be read as authenticated whether or not it
// checks anything, so a third kind is given its own answer in both.
#[cfg(not(target_family = "wasm"))]
enum Own {
    Map(memmap2::Mmap),
    Paged(std::sync::Arc<dyn Paging>),
}

// SAFETY: the pointer is into a mapping `own` keeps for the value's life,
// read-only through this type; a mapped file is `Send + Sync` and so is a
// pager.
#[cfg(not(target_family = "wasm"))]
unsafe impl Send for MmapBytes {}
#[cfg(not(target_family = "wasm"))]
unsafe impl Sync for MmapBytes {}

/// What stands behind a paged mapping: address space `len` bytes long at
/// `base`, of which a range is readable once `ensure` has answered for it
/// and for as long as the caller's protection holds (`db::pager`).
///
/// The engine's: crate-private, and its address asked for only with a token
/// this module alone makes (`BaseAddressToken`), so the one reader of it is
/// `MmapBytes::paged`, which lends nothing of it unasked.
///
/// A paged mapping answers `Bytes::authenticated` with yes, and the readers
/// over it skip the checks of their own that would ask for a section whole
/// (the key section's checksum row, the ordered index's checksum). So an
/// implementor answers `ensure` and `piece_from` only for bytes it has
/// checked: the pager does, by each chunk's tag. A stand-in that checks
/// nothing, as a test's does, leaves those bytes checked by nobody.
#[cfg(not(target_family = "wasm"))]
pub(crate) trait Paging: Send + Sync {
    /// The mapping's first byte; it does not move.
    fn base_address(&self, _: BaseAddressToken) -> *const u8;
    /// The file's length in bytes.
    fn size(&self) -> u64;
    /// `len` bytes at `off` made present; false where they cannot be (a
    /// range past the end, bytes that fail their check, a read that
    /// failed), in which case nothing of the range may be read.
    fn ensure(&self, off: u64, len: usize) -> bool;
    /// Where the piece holding `off` ends.
    fn piece_end(&self, off: u64) -> u64;
    /// The piece holding `off` made present from `off` to its end, and how
    /// many bytes that is; zero where it cannot be, or `off` is past the
    /// end.
    fn piece_from(&self, off: u64) -> usize;
    /// How what stands behind the mapping is about to be read: far apart
    /// (`random`), or in order.
    fn advise(&self, random: bool);
    /// The share of `len` bytes at `off` that a read would find in memory:
    /// present in the mapping, or in the kernel's cache of what stands
    /// behind it.
    fn resident(&self, off: u64, len: usize) -> f64;
    /// The kernel's cache of what stands behind `len` bytes at `off` given
    /// up, for bytes nothing is expected to read again soon; what the
    /// mapping holds present is its owner's to give back.
    fn forget(&self, off: u64, len: usize);
    /// What stands behind `len` bytes at `off` asked of the kernel now, in
    /// the background, and the bytes asked for: nothing for bytes the
    /// mapping holds present.
    fn willneed(&self, off: u64, len: usize) -> usize;
}

/// The token `Paging::base_address` takes: made in this module alone (its
/// field is private), so the address behind a paged mapping is asked for by
/// `MmapBytes::paged` and by nobody else.
#[cfg(not(target_family = "wasm"))]
pub(crate) struct BaseAddressToken(());

/// The most of a mapping one `MADV_WILLNEED` asks for. Linux reads a
/// will-need range ahead only up to the larger of the device's largest
/// request and the file's readahead window, and drops the rest of the
/// range without a word (`force_page_cache_ra`): one ask over 104 evicted
/// pages brought in all of them where the device takes 4 MiB a request and
/// 64 where it takes 256 KiB, the ARM runner's. The window is 128 KiB unless
/// it was lowered, so an ask cut into pieces of that is read whole.
#[cfg(not(target_family = "wasm"))]
pub const WILLNEED_PIECE: usize = 128 << 10;

#[cfg(not(target_family = "wasm"))]
impl MmapBytes {
    pub fn open(path: &std::path::Path) -> Result<MmapBytes> {
        let file = std::fs::File::open(path)?;
        // SAFETY: same contract `Reader::open` takes -- the file must not be
        // truncated underneath the mapping. supdb only ever appends and the
        // write-once shape this serves seals the object before any reader
        // sees it.
        Ok(MmapBytes::from_map(unsafe { memmap2::Mmap::map(&file)? }))
    }

    /// A mapping already made, under the contract `open` takes.
    pub fn from_map(map: memmap2::Mmap) -> MmapBytes {
        MmapBytes {
            ptr: map.as_ptr(),
            lend: map.len() as u64,
            len: map.len() as u64,
            own: Own::Map(map),
        }
    }

    /// A paged mapping: nothing of it is lent but through `pager`.
    #[doc(hidden)]
    pub(crate) fn paged(pager: std::sync::Arc<dyn Paging>) -> MmapBytes {
        MmapBytes {
            ptr: pager.base_address(BaseAddressToken(())),
            lend: 0,
            len: pager.size(),
            own: Own::Paged(pager),
        }
    }

    /// The mapping as it lies: all of a mapped file, and nothing of a
    /// paged mapping, whose bytes are asked for through `slice_at`. Sealed:
    /// the token is a reader's that may read a mapping so
    /// (`whole_map_readers::Reader`, implemented here alone): the ordered
    /// index, which checks its checksum over its heads and reads them where
    /// they lie (`ordindex::lend_seal`), and the stride form, which reads
    /// its value pages at their offsets (`db::zfile::lend_seal`), each
    /// token made in its own module alone, so the compiler refuses this call in any other. Any
    /// other reader asks with `slice_at`. The token is a zero-sized value
    /// and costs the call nothing.
    #[inline]
    pub(crate) fn lend_whole_map<T: whole_map_readers::Reader>(&self, _: T) -> &[u8] {
        // SAFETY: `lend` bytes at `ptr` are a mapped file's, kept by `own`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.lend as usize) }
    }

    /// The first byte of each system page of `len` bytes at `off` read,
    /// and their wrapping sum, for a warm that makes the pages present
    /// before the reads that want them: a mapped file's pages, clamped to
    /// it; nothing of a paged mapping, whose pages are made present only by
    /// asking for them, so a touch here is none (its lendable length is
    /// zero).
    pub(crate) fn touch(&self, off: u64, len: usize) -> u64 {
        let page = os_page_size() as u64;
        let end = off.saturating_add(len as u64).min(self.lend);
        let mut sum = 0u64;
        let mut at = off;
        while at < end {
            // SAFETY: `at` is inside the `lend` bytes of a mapped file,
            // which `own` keeps for this value's life.
            sum = sum.wrapping_add(u64::from(unsafe { *self.ptr.add(at as usize) }));
            at += page;
        }
        sum
    }

    /// How many bytes from the start the mapping lends whole: a test's
    /// look, which holds a paged mapping to none whatever its bytes are.
    #[cfg(test)]
    pub(crate) fn lends(&self) -> u64 {
        self.lend
    }

    /// Where the mapping lies in the process, for a test that reads the
    /// kernel's page table over it.
    #[cfg(all(test, target_os = "linux", not(supdb_emulated)))]
    pub(crate) fn addr(&self) -> usize {
        self.ptr as usize
    }

    /// The mapped file, where this is one. `None` for every other kind,
    /// which `authenticated` reads as a paged mapping's: a kind added to
    /// `Own` answers here and there on its own terms.
    fn map(&self) -> Option<&memmap2::Mmap> {
        match &self.own {
            Own::Map(m) => Some(m),
            Own::Paged(_) => None,
        }
    }

    /// `slice_at` for a range the bounds check did not pass: past the end
    /// of a mapped file, or any range of a paged mapping, which is asked
    /// of its pager here. Out of line and cold, so a mapped file's
    /// `slice_at` is its compare and its slice.
    #[cold]
    #[inline(never)]
    fn slice_slow(&self, off: u64, len: usize) -> Option<&[u8]> {
        let Own::Paged(p) = &self.own else {
            return None;
        };
        if off.checked_add(len as u64)? > self.len || !p.ensure(off, len) {
            return None;
        }
        // SAFETY: inside the mapping, and made present by `ensure`.
        Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), len) })
    }

    /// How many of the system pages that `len` bytes at `off` of a mapped
    /// file lie in the kernel holds in memory now, by `mincore`, and how
    /// many pages that is: the range clamped to the mapping, every page a
    /// byte of it lies in counting, the first from its start; `(0, 0)` for
    /// a range that is empty or past the mapping, and `None` where the
    /// kernel will not say. `Bytes::resident_share` for a mapped file.
    fn mapped_resident(&self, off: usize, len: usize) -> Option<(usize, usize)> {
        let page = os_page_size();
        let end = off.saturating_add(len).min(self.lend as usize);
        if end <= off {
            return Some((0, 0));
        }
        let start = off / page * page;
        let span = end - start;
        let mut vec = vec![0u8; span.div_ceil(page)];
        // SAFETY: the range lies inside this mapping, which lives for the
        // call, and `vec` has a byte for every page of it.
        let rc = unsafe {
            libc::mincore(
                self.ptr.add(start).cast_mut().cast(),
                span,
                vec.as_mut_ptr().cast(),
            )
        };
        if rc != 0 {
            return None;
        }
        Some((vec.iter().filter(|&&b| b & 1 != 0).count(), vec.len()))
    }
}

/// The system's page size.
#[cfg(not(target_family = "wasm"))]
pub fn os_page_size() -> usize {
    // SAFETY: a query with no side effect.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(n).unwrap_or(4096).max(1)
}

#[cfg(not(target_family = "wasm"))]
impl Bytes for MmapBytes {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
        let got = self
            .slice_at(off, dst.len())
            .ok_or_else(|| short(off, dst.len(), self.len))?;
        dst.copy_from_slice(got);
        Ok(())
    }
    #[inline]
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.lend {
            return self.slice_slow(off, len);
        }
        // SAFETY: `off..end` is inside the `lend` bytes of a mapped file,
        // which `own` keeps for this value's life.
        Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), len) })
    }
    /// A paged mapping lends no byte without being asked for it, and its
    /// lendable length is zero: the range check a mapped file passes is
    /// the refusal, with no test of which kind this is.
    #[inline]
    fn lend_at(&self, off: u64, len: usize, _: LendToken) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.lend {
            return None;
        }
        // SAFETY: as `slice_at`.
        Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), len) })
    }
    fn piecewise(&self) -> bool {
        matches!(self.own, Own::Paged(_))
    }
    /// A paged mapping lends what its pager has decrypted, and the pager
    /// lends no chunk whose tag it has not checked; a mapped file checks
    /// nothing.
    fn authenticated(&self) -> bool {
        self.map().is_none()
    }
    const ASK_PIECES: bool = false;
    fn piece_end(&self, off: u64) -> u64 {
        match &self.own {
            Own::Map(_) => off.saturating_add(4096).min(self.len),
            Own::Paged(p) => p.piece_end(off).min(self.len),
        }
    }
    fn piece_at(&self, off: u64) -> Option<&[u8]> {
        match &self.own {
            Own::Map(_) => {
                let end = off.saturating_add(4096).min(self.len);
                self.slice_at(off, usize::try_from(end.checked_sub(off)?).ok()?)
            }
            Own::Paged(p) => match p.piece_from(off) {
                0 => None,
                // SAFETY: inside the mapping, and made present by
                // `piece_from`.
                n => Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), n) }),
            },
        }
    }
    fn advise_random(&self) {
        match &self.own {
            Own::Map(m) => {
                let _ = m.advise(memmap2::Advice::Random);
            }
            Own::Paged(p) => p.advise(true),
        }
    }
    fn advise_normal(&self) {
        match &self.own {
            Own::Map(m) => {
                let _ = m.advise(memmap2::Advice::Normal);
            }
            Own::Paged(p) => p.advise(false),
        }
    }
    #[cfg(target_os = "linux")]
    fn advise_dontneed_range(&self, off: u64, len: usize) {
        // A paged mapping's pages are its pager's to give back, never an
        // advice's: its lendable length is zero, so the range below is
        // empty, and what a drop reaches is the kernel's cache of the file
        // behind it.
        if let Own::Paged(p) = &self.own {
            p.forget(off, len);
            return;
        }
        let Ok(off) = usize::try_from(off) else {
            return;
        };
        let end = off.saturating_add(len).min(self.lend as usize);
        // SAFETY: the page size is a constant of the process; the range is
        // whole pages of this read-only shared mapping, aligned in, and the
        // advice drops the page-table entries for them and nothing else.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let Some(page) = usize::try_from(page).ok().filter(|p| p.is_power_of_two()) else {
            return;
        };
        let a = off.next_multiple_of(page);
        let b = end & !(page - 1);
        if a >= b {
            return;
        }
        let base = self.ptr as usize;
        unsafe {
            libc::madvise((base + a) as *mut libc::c_void, b - a, libc::MADV_DONTNEED);
        }
    }
    #[cfg(target_os = "linux")]
    fn resident_at(&self, off: u64) -> Option<bool> {
        if let Own::Paged(p) = &self.own {
            return (off < self.len).then(|| p.resident(off, 1) > 0.0);
        }
        let off = usize::try_from(off).ok()?;
        if off >= self.lend as usize {
            return None;
        }
        // SAFETY: the page size is a constant of the process, and the
        // address handed to `mincore` is one page of this mapping, aligned
        // down to the page and within it.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page = usize::try_from(page).ok().filter(|p| p.is_power_of_two())?;
        let base = self.ptr as usize;
        let at = (base + off) & !(page - 1);
        let mut vec = [0u8; 1];
        let rc = unsafe { libc::mincore(at as *mut libc::c_void, page, vec.as_mut_ptr()) };
        (rc == 0).then_some(vec[0] & 1 == 1)
    }
    fn resident_share(&self, off: u64, len: usize) -> Option<f64> {
        let Own::Paged(p) = &self.own else {
            let (have, pages) = self.mapped_resident(usize::try_from(off).ok()?, len)?;
            return Some(if pages == 0 {
                0.0
            } else {
                have as f64 / pages as f64
            });
        };
        Some(p.resident(off, len))
    }
    fn advise_dontneed(&self) {
        #[cfg(target_os = "linux")]
        if self.lend > 0 {
            // SAFETY: an advice over the whole of a read-only shared file
            // mapping this value owns: it drops this process's page-table
            // entries for the range and changes no byte, and a later touch
            // faults the page back from the file, so a borrow into the
            // mapping reads the same bytes after it as before. A paged
            // mapping lends nothing here and is not advised.
            unsafe {
                libc::madvise(
                    self.ptr as *mut libc::c_void,
                    self.lend as usize,
                    libc::MADV_DONTNEED,
                );
            }
        }
        // A paged mapping's own pages are its pager's to give back; what
        // the kernel caches of the file behind it is what a drop can
        // reach.
        if let Own::Paged(p) = &self.own {
            p.forget(0, self.len as usize);
        }
    }
    fn advise_willneed(&self, off: u64, len: usize) -> usize {
        let Some(map) = self.map() else {
            // A paged mapping: the file behind it asked for, where the
            // bytes are not present already.
            return match &self.own {
                Own::Paged(p) => p.willneed(off, len),
                Own::Map(_) => 0,
            };
        };
        // Clamped rather than trusted: the offsets come from a plan built out
        // of a file's own block table, and a damaged one must not hand the
        // kernel a range past the end of the mapping. `try_from` rather than
        // `as` on the offset because a wrapped one would name a valid page
        // that is the wrong page, which clamping cannot catch -- a length
        // only ever comes out short, an offset comes out wrong.
        let Ok(start) = usize::try_from(off) else {
            return 0;
        };
        let start = start.min(map.len());
        let span = len.min(map.len() - start);
        // In pieces, each read whole (`WILLNEED_PIECE`); what is reported
        // is what was submitted, up to the first piece the kernel refused.
        let mut done = 0;
        while done < span {
            let n = WILLNEED_PIECE.min(span - done);
            if map
                .advise_range(memmap2::Advice::WillNeed, start + done, n)
                .is_err()
            {
                break;
            }
            done += n;
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every page a byte of the range lies in counts, from the first one's
    /// start, and nothing past the mapping does: a file of three pages and a
    /// hundred bytes, all of it just written and so in the page cache. The
    /// share is of those pages, and none for a range with none.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn residency_counts_every_page_a_range_touches_and_none_past_the_end() {
        let page = os_page_size();
        let len = 3 * page + 100;
        let path =
            std::env::temp_dir().join(format!("supdb-next-resident-pages-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; len]).unwrap();
        let m = MmapBytes::open(&path).unwrap();
        let sum: u64 = (0..len)
            .step_by(page)
            .map(|o| u64::from(m.slice_at(o as u64, 1).unwrap()[0]))
            .sum();
        assert_eq!(sum, 4 * 7, "every page touched");
        let pages = |off: usize, n: usize| m.mapped_resident(off, n).unwrap();
        assert_eq!(pages(0, len), (4, 4));
        assert_eq!(
            pages(page - 1, 2),
            (2, 2),
            "two bytes across a page boundary"
        );
        assert_eq!(pages(page, page), (1, 1), "one page exactly");
        assert_eq!(pages(len - 1, 100), (1, 1), "clamped to the last page");
        assert_eq!(pages(len, 5), (0, 0), "past the mapping");
        assert_eq!(pages(len + page, 5), (0, 0), "well past the mapping");
        assert_eq!(pages(5, 0), (0, 0), "an empty range");
        assert_eq!(m.resident_share(0, len), Some(1.0));
        assert_eq!(m.resident_share(page as u64 - 1, 2), Some(1.0));
        assert_eq!(
            m.resident_share(len as u64, 5),
            Some(0.0),
            "past the mapping"
        );
        assert_eq!(m.resident_share(5, 0), Some(0.0), "an empty range");
        drop(m);
        let _ = std::fs::remove_file(&path);
    }

    /// A touch reads the first byte of every page of the range from the
    /// range's start, the last page of the file too where it is short, and
    /// nothing past the mapping: a file of five pages and a hundred bytes
    /// whose pages begin with 1, 2, 3, 4, 5 and 7 and hold zeroes after.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn a_touch_reads_each_page_of_its_range_once_and_nothing_past_the_file() {
        let page = os_page_size();
        let len = 5 * page + 100;
        let mut body = vec![0u8; len];
        for (i, first) in [1u8, 2, 3, 4, 5, 7].into_iter().enumerate() {
            body[i * page] = first;
        }
        // Bytes inside every whole page that a touch reads none of: a
        // stride shorter than a page, or one that starts past a page's
        // first byte, reads one of them.
        for i in 0..5 {
            for at in [1, page / 4, page / 2, page - 1] {
                body[i * page + at] = 100;
            }
        }
        let path = std::env::temp_dir().join(format!("supdb-next-touch-{}", std::process::id()));
        std::fs::write(&path, &body).unwrap();
        let m = MmapBytes::open(&path).unwrap();
        assert_eq!(m.touch(0, len), 22, "every page, the short last one too");
        assert_eq!(m.touch(0, len + 3 * page), 22, "clamped to the file");
        assert_eq!(m.touch(0, 1), 1, "the range's first page");
        assert_eq!(m.touch(page as u64, 2 * page), 2 + 3, "two pages exactly");
        assert_eq!(m.touch(5 * page as u64, 1), 7, "the last page alone");
        assert_eq!(m.touch(len as u64, page), 0, "past the file");
        assert_eq!(m.touch(3, 0), 0, "an empty range");
        drop(m);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn slice_source_lends_and_refuses_past_the_end() {
        let data: Vec<u8> = (0..64u8).collect();
        let s = SliceBytes(&data);
        assert_eq!(s.len(), 64);
        assert_eq!(s.slice_at(8, 4), Some(&data[8..12]));
        assert_eq!(s.slice_at(60, 8), None);
        assert_eq!(s.slice_at(u64::MAX, 8), None);
        let mut buf = [0u8; 4];
        s.read_at(8, &mut buf).unwrap();
        assert_eq!(buf, [8, 9, 10, 11]);
        assert!(s.read_at(62, &mut buf).is_err());
    }

    /// A source with no memory behind it: the shape an OPFS handle has.
    struct Copying(Vec<u8>);
    impl Bytes for Copying {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<()> {
            SliceBytes(&self.0).read_at(off, dst)
        }
    }

    #[test]
    fn take_borrows_from_memory_and_copies_otherwise() {
        let data: Vec<u8> = (0..64u8).collect();
        let mut scratch = Vec::new();
        let src = SliceBytes(&data);
        let got = take(&src, 8, 4, &mut scratch).unwrap();
        assert_eq!(got, &[8, 9, 10, 11]);
        // Nothing was copied: the borrow came out of the source.
        assert!(scratch.is_empty());

        let c = Copying(data.clone());
        let mut scratch = Vec::new();
        let got = take(&c, 8, 4, &mut scratch).unwrap();
        assert_eq!(got, &[8, 9, 10, 11]);
    }

    #[test]
    fn a_copying_source_still_refuses_a_short_read() {
        let c = Copying(vec![0u8; 16]);
        let mut scratch = Vec::new();
        assert!(take(&c, 12, 8, &mut scratch).is_err());
    }
}
