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

use std::io::{Error, ErrorKind, Result};

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
    /// (`flatindex::Pieces`), so for a mapped file it is the one range
    /// check it always was.
    #[inline]
    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {
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
#[cfg(not(target_family = "wasm"))]
#[doc(hidden)]
pub trait Paging: Send + Sync {
    /// The mapping's first byte; it does not move.
    fn base(&self) -> *const u8;
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
}

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
            ptr: pager.base(),
            lend: 0,
            len: pager.size(),
            own: Own::Paged(pager),
        }
    }

    /// The mapping as it lies: all of a mapped file, and nothing of a
    /// paged mapping, whose bytes are asked for through `slice_at`.
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `lend` bytes at `ptr` are a mapped file's, kept by `own`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.lend as usize) }
    }

    /// The mapped file, where this is one.
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

    /// How many of the system pages that `len` bytes at `off` lie in the
    /// kernel holds in memory now, by `mincore`: what a warm of the file can
    /// skip. The range is clamped to the mapping, and every page a byte of
    /// it lies in counts, the first one from its start; zero for a range
    /// that is empty or past the mapping, and where the kernel will not say.
    pub fn resident_pages(&self, off: usize, len: usize) -> usize {
        let page = os_page_size();
        let end = off.saturating_add(len).min(self.lend as usize);
        if end <= off {
            return 0;
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
            return 0;
        }
        vec.iter().filter(|&&b| b & 1 != 0).count()
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
    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {
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
        // advice's: its lendable length is zero, so the range is empty.
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
    }
    fn advise_willneed(&self, off: u64, len: usize) -> usize {
        let Some(map) = self.map() else {
            return 0;
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
    /// hundred bytes, all of it just written and so in the page cache.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn resident_pages_counts_every_page_a_range_touches_and_none_past_the_end() {
        let page = os_page_size();
        let len = 3 * page + 100;
        let path =
            std::env::temp_dir().join(format!("supdb-next-resident-pages-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; len]).unwrap();
        let m = MmapBytes::open(&path).unwrap();
        let sum: u64 = (0..len)
            .step_by(page)
            .map(|o| u64::from(m.as_slice()[o]))
            .sum();
        assert_eq!(sum, 4 * 7, "every page touched");
        assert_eq!(m.resident_pages(0, len), 4);
        assert_eq!(
            m.resident_pages(page - 1, 2),
            2,
            "two bytes across a page boundary"
        );
        assert_eq!(m.resident_pages(page, page), 1, "one page exactly");
        assert_eq!(
            m.resident_pages(len - 1, 100),
            1,
            "clamped to the last page"
        );
        assert_eq!(m.resident_pages(len, 5), 0, "past the mapping");
        assert_eq!(m.resident_pages(len + page, 5), 0, "well past the mapping");
        assert_eq!(m.resident_pages(5, 0), 0, "an empty range");
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
