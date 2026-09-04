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
//! is a synchronous random read. That costs one full download per index --
//! which `w1-daysize` is the measurement of -- and requires the reader to run
//! in a Web Worker, since sync access handles are not available on the main
//! thread. Both were acceptable, so the API needed no shape change.
//!
//! ## Endianness
//!
//! Every scalar in the file is written little-endian, but the zero-copy read
//! path reinterprets an extent array as `&[Ext]`, which is native-endian. The
//! two agree only on a little-endian machine, so `Blob::open` refuses a
//! big-endian target explicitly rather than misreading a file there. Every
//! browser is little-endian and so is every machine in `results/`, so this
//! costs nothing and is checked rather than assumed.

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

    /// Hint that access will be random.
    ///
    /// A no-op where the target has no mapping to advise, which is every
    /// non-native source -- not an error. There is nothing to tell an OPFS
    /// file handle about page-fault patterns.
    fn advise_random(&self) {}
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
#[cfg(not(target_family = "wasm"))]
pub struct MmapBytes(pub memmap2::Mmap);

#[cfg(not(target_family = "wasm"))]
impl MmapBytes {
    pub fn open(path: &std::path::Path) -> Result<MmapBytes> {
        let file = std::fs::File::open(path)?;
        // SAFETY: same contract `Reader::open` takes -- the file must not be
        // truncated underneath the mapping. supdb only ever appends and the
        // logshed shape seals the object before any reader sees it.
        Ok(MmapBytes(unsafe { memmap2::Mmap::map(&file)? }))
    }
}

#[cfg(not(target_family = "wasm"))]
impl Bytes for MmapBytes {
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
    fn advise_random(&self) {
        let _ = self.0.advise(memmap2::Advice::Random);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
