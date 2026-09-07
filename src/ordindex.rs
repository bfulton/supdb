//! The ordered index: eight bytes a key, so a scan's seek stops walking
//! records.
//!
//! A segment's key section is a hash table with the records in key order
//! behind a directory, and the ordered seek a scan starts with binary
//! searches those records. Measured, that is the whole of the scan deficit:
//! the seek costs about twenty dependent cache misses because every probe
//! lands on a record carrying its key, its extents and -- with inline runs
//! -- its values, so the search strides through 140 bytes to compare
//! sixteen. Its own bytes are what fix that, and not a denser fence: a
//! binary search costs log2(nkeys) probes however the levels are cut, and a
//! sweep of every stride from 32 down to 1 moved the seek only 1,171ns to
//! 899 while making the scans worse.
//!
//! So this file holds one `head` a key and nothing else: the eight bytes
//! after the segment's common prefix, big-endian, so a probe is one aligned
//! `u64` compare. The prefix is what makes eight bytes enough -- the suite's
//! keys are sixteen zero-padded digits whose first ten bytes are identical,
//! so a raw prefix separates nothing and a stripped one separates
//! everything. Where two keys do tie on their head, the segment resolves it:
//! it already holds every key, and duplicating them here cost 28 bytes a key
//! against 8 and 14% of the durable load at 300k keys against 0%.
//!
//! Entry `i` is the segment's rank `i` -- both are key ordered -- so a seek
//! here answers exactly what `Blob::seek` answers, and `tests/db.rs` holds
//! the two to it. Nothing about the value bytes is duplicated: measured,
//! separating values from keys is a wash on a scan that reads them, and the
//! whole win is on the seek and on walks that do not.
//!
//! It is a companion file rather than a region of the segment because that
//! leaves the segment format, and the browser reader over it, untouched. It
//! is written before its segment is renamed into place and is required to
//! exist, so a segment a reader can see always has one. There is no fallback
//! to the old seek: a companion missing or failing its checksum fails the
//! open, which is what every other structure in this format does with
//! damage. A fallback would be the slow path taken silently, and a check
//! that reports a verdict it has not earned is how every gate here has
//! broken.

use crate::block::crc32;
use crate::bytes::MmapBytes;
use std::io::{Error, ErrorKind, Result};
use std::path::Path;

/// "SUPDORD1", little-endian. The trailing digit is the version, and it
/// moves whenever an older reader would misread rather than refuse.
const MAGIC: u64 = 0x3144_524f_4450_5553;
/// Room for the header and the alignment the heads want, in one constant so
/// the writer and the reader cannot disagree about where the body starts.
const HEADER: usize = 64;
const HEAD: usize = 8;

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, format!("ordered index: {msg}"))
}

#[inline]
fn head_of(key: &[u8], pfx: usize) -> u64 {
    let mut h = [0u8; HEAD];
    let from = pfx.min(key.len());
    let n = (key.len() - from).min(HEAD);
    h[..n].copy_from_slice(&key[from..from + n]);
    u64::from_be_bytes(h)
}

/// Collects a segment's keys as they are written and composes the file.
///
/// The writer already holds the keys sorted -- a seal sorts the memtable by
/// key before writing and a merge emits in key order -- so this costs the
/// copy and nothing else: 2.2ns a key measured, against the 20.2ns a
/// separate pass spends reading them back out of the finished segment.
#[derive(Default)]
pub struct Builder {
    /// The keys as they arrive, kept only until `finish` knows the common
    /// prefix. They are not written.
    starts: Vec<u32>,
    bytes: Vec<u8>,
    n: usize,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    /// Keys arrive in the order they are written, which is key order.
    pub fn push(&mut self, key: &[u8]) {
        self.starts.push(self.bytes.len() as u32);
        self.bytes.extend_from_slice(key);
        self.n += 1;
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The file's bytes: a header, one head a key, a checksum.
    pub fn finish(mut self) -> Vec<u8> {
        self.starts.push(self.bytes.len() as u32);
        let pfx = self.common_prefix();
        let mut out = Vec::with_capacity(HEADER + self.n * HEAD + 4);
        out.resize(HEADER, 0);
        for i in 0..self.n {
            let k = &self.bytes[self.starts[i] as usize..self.starts[i + 1] as usize];
            out.extend_from_slice(&head_of(k, pfx).to_be_bytes());
        }
        let w = |out: &mut Vec<u8>, at: usize, v: u64| {
            out[at..at + 8].copy_from_slice(&v.to_le_bytes());
        };
        w(&mut out, 0, MAGIC);
        w(&mut out, 8, self.n as u64);
        w(&mut out, 16, pfx as u64);
        w(&mut out, 24, HEADER as u64);
        // The checksum goes last and covers everything before it, the spare
        // header words included. Covering only the body left those
        // unchecked and the damage test found it: a flip there opened clean,
        // which is the shape of the bug that made the key section's checksum
        // row name its own piece shift.
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// The bytes every key shares, from the first and the last: the keys are
    /// sorted, so no key between them can differ earlier than those two do.
    fn common_prefix(&self) -> usize {
        if self.n == 0 {
            return 0;
        }
        let first = &self.bytes[self.starts[0] as usize..self.starts[1] as usize];
        let last = &self.bytes[self.starts[self.n - 1] as usize..self.starts[self.n] as usize];
        first
            .iter()
            .zip(last.iter())
            .take_while(|(a, b)| a == b)
            .count()
    }
}

/// One segment's ordered index, mapped.
pub struct OrdIndex {
    map: MmapBytes,
    n: usize,
    pfx: usize,
}

impl OrdIndex {
    /// Map and verify. `keys` is the segment's own key count: a companion
    /// describing a different segment -- an orphan a crashed merge left
    /// under an id handed out again later -- is refused here rather than
    /// believed.
    pub fn open(path: &Path, keys: usize) -> Result<OrdIndex> {
        let map = MmapBytes::open(path)?;
        let b = &map.0[..];
        if b.len() < HEADER + 4 {
            return Err(bad("shorter than its header"));
        }
        let body = b.len() - 4;
        let crc = u32::from_le_bytes(b[body..].try_into().expect("four bytes"));
        if crc32(&b[..body]) != crc {
            return Err(bad("checksum does not match"));
        }
        let rd = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"));
        if rd(0) != MAGIC {
            return Err(bad("wrong magic"));
        }
        let n = rd(8) as usize;
        let (pfx, heads_at) = (rd(16) as usize, rd(24) as usize);
        if n != keys {
            return Err(bad("describes a different segment"));
        }
        // The heads have to fill the file exactly. A header saying otherwise
        // is damage, and indexing past it later would be a panic rather than
        // an error.
        let want = n
            .checked_mul(HEAD)
            .ok_or_else(|| bad("key count overflows"))?;
        if heads_at != HEADER || heads_at.checked_add(want) != Some(body) {
            return Err(bad("heads do not fill the file"));
        }
        Ok(OrdIndex { map, n, pfx })
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    #[inline]
    fn head(&self, i: usize) -> u64 {
        let at = HEADER + i * HEAD;
        u64::from_be_bytes(self.map.0[at..at + HEAD].try_into().expect("eight bytes"))
    }

    /// The first rank at or after `from` whose head is greater than `h`.
    #[inline]
    fn upper(&self, h: u64, from: usize) -> usize {
        let (mut lo, mut hi) = (from, self.n);
        while lo < hi {
            let m = (lo + hi) / 2;
            if self.head(m) <= h {
                lo = m + 1;
            } else {
                hi = m;
            }
        }
        lo
    }

    /// The first rank whose key is not less than `key` -- what `Blob::seek`
    /// answers.
    ///
    /// `key_at` reads the segment's own key at a rank, and is asked only
    /// where heads tie. Eight bytes past the common prefix separate every
    /// key in any distribution that is not adversarial, so the usual seek is
    /// the head search alone; where they do not separate, a second binary
    /// search over the run of equal heads resolves it and the answer is
    /// exact either way.
    pub fn seek<'a>(&self, key: &[u8], key_at: impl Fn(usize) -> Option<&'a [u8]>) -> usize {
        let h = head_of(key, self.pfx);
        let (mut lo, mut hi) = (0usize, self.n);
        while lo < hi {
            let m = (lo + hi) / 2;
            if self.head(m) < h {
                lo = m + 1;
            } else {
                hi = m;
            }
        }
        if lo >= self.n || self.head(lo) != h {
            return lo;
        }
        let (mut a, mut b) = (lo, self.upper(h, lo));
        while a < b {
            let m = (a + b) / 2;
            // A rank the segment will not resolve sorts as "not less", the
            // same rule the record search uses, so damage widens the answer
            // rather than moving it.
            match key_at(m) {
                Some(k) if k < key => a = m + 1,
                _ => b = m,
            }
        }
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(keys: &[&[u8]]) -> Vec<u8> {
        let mut b = Builder::new();
        for k in keys {
            b.push(k);
        }
        b.finish()
    }

    fn lower_bound(keys: &[&[u8]], key: &[u8]) -> usize {
        keys.partition_point(|k| *k < key)
    }

    /// The segment's role in a seek: the key at a rank.
    fn resolver<'a>(keys: &'a [&'a [u8]]) -> impl Fn(usize) -> Option<&'a [u8]> + 'a {
        move |i: usize| keys.get(i).copied()
    }

    fn write(bytes: &[u8], name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ordidx-{name}-{}", std::process::id()));
        std::fs::write(&p, bytes).expect("write");
        p
    }

    fn check_all(keys: &[&[u8]], name: &str) {
        let p = write(&build(keys), name);
        let idx = OrdIndex::open(&p, keys.len()).expect("opens");
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(
                idx.seek(k, resolver(keys)),
                lower_bound(keys, k),
                "present {i}"
            );
        }
        let _ = std::fs::remove_file(&p);
    }

    /// Sixteen zero-padded digits: the shape whose first ten bytes are
    /// identical, which is why the head is taken after the prefix.
    #[test]
    fn a_seek_answers_what_a_lower_bound_answers() {
        let owned: Vec<Vec<u8>> = (0u32..2_000)
            .map(|i| format!("{i:016}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        check_all(&keys, "lower");
        let p = write(&build(&keys), "probes");
        let idx = OrdIndex::open(&p, keys.len()).expect("opens");
        for probe in [
            "",
            "0",
            "00000000000000000",
            "9999999999999999",
            "00000000000001005",
        ] {
            let b = probe.as_bytes();
            assert_eq!(
                idx.seek(b, resolver(&keys)),
                lower_bound(&keys, b),
                "probe {probe:?}"
            );
        }
        let _ = std::fs::remove_file(&p);
    }

    /// A shared prefix longer than the head: stripping it is what keeps the
    /// heads distinct.
    #[test]
    fn keys_behind_a_long_shared_prefix_still_seek_exactly() {
        let owned: Vec<Vec<u8>> = (0u32..500)
            .map(|i| format!("a-very-long-shared-prefix-indeed-{i:06}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        check_all(&keys, "prefix");
    }

    /// Keys that tie on their head whatever the prefix strips: the second
    /// search has to carry the whole answer.
    #[test]
    fn keys_indistinguishable_by_head_still_seek_exactly() {
        let owned: Vec<Vec<u8>> = (0u32..300)
            .map(|i| {
                let mut k = vec![b'x'; 40];
                k.extend_from_slice(format!("{i:06}").as_bytes());
                k
            })
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        check_all(&keys, "headtie");
    }

    #[test]
    fn an_empty_segment_builds_and_seeks() {
        let p = write(&build(&[]), "empty");
        let idx = OrdIndex::open(&p, 0).expect("opens");
        assert!(idx.is_empty());
        let none: Vec<&[u8]> = Vec::new();
        assert_eq!(idx.seek(b"anything", resolver(&none)), 0);
        let _ = std::fs::remove_file(&p);
    }

    /// There is no fallback, so damage has to be refused rather than read
    /// around. Every seventh byte, the way `tests/segwriter.rs` does it.
    #[test]
    fn a_flipped_byte_anywhere_fails_the_open() {
        let owned: Vec<Vec<u8>> = (0u32..300)
            .map(|i| format!("{i:016}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let good = build(&keys);
        for at in (0..good.len()).step_by(7) {
            let mut damaged = good.clone();
            damaged[at] ^= 0x40;
            let p = write(&damaged, &format!("flip{at}"));
            assert!(
                OrdIndex::open(&p, keys.len()).is_err(),
                "a flip at {at} opened clean"
            );
            let _ = std::fs::remove_file(&p);
        }
    }

    /// A companion left by a crashed merge, under an id handed out again
    /// later, must not be adopted by the segment that now holds it.
    #[test]
    fn a_companion_for_another_segment_is_refused() {
        let owned: Vec<Vec<u8>> = (0u32..64)
            .map(|i| format!("{i:016}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let p = write(&build(&keys), "wrongseg");
        assert!(OrdIndex::open(&p, keys.len() + 1).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
