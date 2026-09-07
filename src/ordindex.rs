//! The ordered index: what a scan is allowed to store to go faster.
//!
//! A segment's key section is a hash table with the records in key order
//! behind a directory. A scan over it pays twice: an ordered seek that
//! binary-searches the records, each carrying its key, its extents and --
//! with inline runs -- its values, so the search strides through 140 bytes
//! to compare sixteen; and then a walk that parses one of those records a
//! key. Measured, a hand-written parse of the same record shape is within
//! 9% of the engine's, so the walk's cost is the shape and not the code.
//!
//! Three levels, and the trade between them was measured **in the engine**,
//! which is not what a prototype of it predicted. Bytes are what a level adds
//! to the segment; the scans are 100 keys over 290k, six reps:
//!
//! | level  | file B/B | load ops/s | a scan reading lengths | a scan reading values |
//! |--------|----------|------------|------------------------|-----------------------|
//! | `Heads`|     1.56 |    416,502 |          2,340ns 1.00x |         1,966ns 1.00x |
//! | `Refs` |     1.74 |    385,575 |          1,237ns 1.89x |         2,218ns 0.89x |
//! | `Copy` |     2.74 |    257,318 |          1,270ns 1.84x |         2,166ns 0.91x |
//!
//! Read the last column before choosing. **References help only a caller
//! that does not read the value bytes**, and cost about a tenth of one that
//! does. A walk over references touches three streams -- the key references,
//! the value references, and the segment -- where the record walk touches
//! two, and it only comes out ahead when the third is never touched at all.
//! A prototype put `Refs` at 1.50x on a value-reading scan; it held its
//! reference columns in process memory and addressed them without bounds
//! checks, and neither is true of a mapped file.
//!
//! So `Heads` is the default. It is the only level that costs nothing and
//! loses nothing, and the shape `Refs` wins -- a scan that wants keys and
//! counts and not bytes -- is worth asking for explicitly rather than
//! turning on for everyone. The suite's own scan callback sums lengths and
//! never reads a value, so defaulting to `Refs` would move that figure 1.89x
//! while making a real reader 11% slower, which is a way of measuring
//! nothing.
//!
//! `Copy` is dominated as it stands and is kept because the option was asked
//! for: it scans no faster than `Refs` and costs 76% of the segment and a
//! third of the load. Its measured advantage lived in a fixed-stride walk
//! that needs no references at all, which is available only where every key
//! and value is the same width; this walk reads references into the copy, so
//! all the copy buys is locality, and locality was not the cost.
//!
//! **Heads** is one head a key: the eight bytes after the segment's common
//! prefix, big-endian, so a probe is an aligned `u64` compare. The prefix is
//! what makes eight enough -- the suite's keys are sixteen zero-padded digits
//! whose first ten bytes are identical, so a raw prefix separates nothing.
//! Where heads tie, the segment's own keys resolve it, over the run of equal
//! heads only. Two cheaper ideas were measured and refused: a denser fence
//! does not help, because a binary search costs log2(nkeys) probes however
//! the levels are cut, and a model over the keys is worse than the binary
//! search it would replace, because the cost is cold lines and not compares.
//!
//! **Refs** adds, per key and per value, where its bytes lie in the segment.
//! The walk then hands back slices without parsing anything. The value bytes
//! are not copied: measured, copying them is a wash on a scan that reads
//! them, and the whole of `Copy`'s remaining gain is locality.
//!
//! **Copy** adds the keys and values themselves, contiguous in key order,
//! and points the references at them instead. It cannot be built over a
//! segment whose values do not lie in the mapping -- a compressed block is
//! decompressed into a buffer -- and neither can `Refs`; such a segment gets
//! `Heads`, which is always available, and `level()` says which it has.
//!
//! Entry `i` is the segment's rank `i` -- both are key ordered -- so a seek
//! here answers exactly what `Blob::seek` answers and a walk emits exactly
//! what `Blob::scan_at` emits. `tests/db.rs` holds both to that.
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
use crate::Blob;
use std::io::{Error, ErrorKind, Result};
use std::path::Path;

/// "SUPDORD3", little-endian. The trailing digit is the version, and it
/// moves whenever an older reader would misread rather than refuse.
const MAGIC: u64 = 0x3344_524f_4450_5553;
/// Room for the header and the alignment the heads want, in one constant so
/// the writer and the reader cannot disagree about where the body starts.
const HEADER: usize = 128;
const HEAD: usize = 8;
/// A reference is a u32 offset and a u32 length.
const REF: usize = 8;

/// How much a segment's ordered index stores, and so how fast a scan over it
/// is. See the module documentation for what each level costs and buys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScanIndex {
    /// Heads only: a fast seek, and a walk that parses records. The
    /// default, because it is the only level that costs nothing.
    #[default]
    Heads,
    /// Heads and a reference a key and a value. Faster only for a caller
    /// that does not read the value bytes.
    Refs,
    /// Heads, references, and a contiguous copy of the keys and values.
    Copy,
}

impl ScanIndex {
    fn code(self) -> u64 {
        match self {
            ScanIndex::Heads => 0,
            ScanIndex::Refs => 1,
            ScanIndex::Copy => 2,
        }
    }
    fn from_code(v: u64) -> Option<ScanIndex> {
        Some(match v {
            0 => ScanIndex::Heads,
            1 => ScanIndex::Refs,
            2 => ScanIndex::Copy,
            _ => return None,
        })
    }
}

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

fn put(out: &mut [u8], at: usize, v: u64) {
    out[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Collects a segment's keys as they are written and composes the heads.
///
/// The writer already holds the keys sorted -- a seal sorts the memtable by
/// key before writing and a merge emits in key order -- so `Heads` costs the
/// copy and nothing else: 2.2ns a key measured, against the 20.2ns a
/// separate pass spends reading them back out of the finished segment. The
/// other two levels need where the bytes landed, which only the finished
/// segment knows, so they are built by `from_segment`.
#[derive(Default)]
pub struct Builder {
    starts: Vec<u32>,
    bytes: Vec<u8>,
    n: usize,
}

impl Builder {
    /// The sentinel `finish` would append, so `from_segment` can read the
    /// keys back without duplicating the arithmetic.
    fn seal_starts(&mut self) {
        self.starts.push(self.bytes.len() as u32);
    }
    fn key(&self, i: usize) -> &[u8] {
        &self.bytes[self.starts[i] as usize..self.starts[i + 1] as usize]
    }
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

    /// A `Heads` index: a header, one head a key, a checksum.
    pub fn finish(mut self) -> Vec<u8> {
        self.seal_starts();
        let pfx = self.common_prefix();
        let heads: Vec<u64> = (0..self.n).map(|i| head_of(self.key(i), pfx)).collect();
        compose(self.n, 0, pfx, ScanIndex::Heads, &heads, &[], &[], &[], &[])
    }
}

/// Lay the regions out and close with a checksum over every byte before it.
///
/// Covering only the body left the spare header words unchecked and the
/// damage test found it: a flip there opened clean, which is the shape of the
/// bug that made the key section's checksum row name its own piece shift.
#[allow(clippy::too_many_arguments)]
fn compose(
    n: usize,
    nvalues: usize,
    pfx: usize,
    level: ScanIndex,
    heads: &[u64],
    kref: &[(u32, u32)],
    vstart: &[u32],
    vref: &[(u32, u32)],
    copy: &[u8],
) -> Vec<u8> {
    let heads_at = HEADER;
    let kref_at = heads_at + heads.len() * HEAD;
    let vstart_at = kref_at + kref.len() * REF;
    let vref_at = vstart_at + vstart.len() * 4;
    let copy_at = vref_at + vref.len() * REF;
    let mut out = Vec::with_capacity(copy_at + copy.len() + 4);
    out.resize(HEADER, 0);
    for h in heads {
        out.extend_from_slice(&h.to_be_bytes());
    }
    for (o, l) in kref {
        out.extend_from_slice(&o.to_le_bytes());
        out.extend_from_slice(&l.to_le_bytes());
    }
    for v in vstart {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for (o, l) in vref {
        out.extend_from_slice(&o.to_le_bytes());
        out.extend_from_slice(&l.to_le_bytes());
    }
    out.extend_from_slice(copy);
    put(&mut out, 0, MAGIC);
    put(&mut out, 8, n as u64);
    put(&mut out, 16, nvalues as u64);
    put(&mut out, 24, pfx as u64);
    put(&mut out, 32, level.code());
    put(&mut out, 40, heads_at as u64);
    // The level decides which regions exist, not whether one happens to be
    // empty: a segment every key of which was deleted has references and no
    // values, and writing a zero for that region made the reader reject a
    // file it had just written. The delete tests found it.
    let refs = level != ScanIndex::Heads;
    put(&mut out, 48, if refs { kref_at } else { 0 } as u64);
    put(&mut out, 56, if refs { vstart_at } else { 0 } as u64);
    put(&mut out, 64, if refs { vref_at } else { 0 } as u64);
    put(
        &mut out,
        72,
        if level == ScanIndex::Copy && !copy.is_empty() {
            copy_at
        } else {
            0
        } as u64,
    );
    put(&mut out, 80, copy.len() as u64);
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// Build `level` over a finished segment.
///
/// `Refs` and `Copy` need where every key and value landed, which only the
/// written segment knows, so this walks it. That walk is the price of those
/// levels on the write path -- 20.2ns a key measured, against the 2.2ns
/// `Heads` pays for having the keys already in hand.
///
/// Returns a `Heads` index when the segment cannot support references: the
/// values of a compressed block are decompressed into a buffer rather than
/// lent from the mapping, and a reference to a buffer would name nothing.
/// The level in the header says which was built, so a reader never assumes.
pub fn from_segment(blob: &Blob<MmapBytes>, level: ScanIndex) -> Result<Vec<u8>> {
    let n = blob.keys();
    let mut b = Builder::new();
    for r in 0..n {
        b.push(blob.key_at(r).ok_or_else(|| bad("segment lost a rank"))?);
    }
    if level == ScanIndex::Heads || n == 0 {
        return Ok(b.finish());
    }
    let Some(all) = blob.mapped() else {
        return Ok(b.finish());
    };
    let base = all.as_ptr() as usize;
    let end = base + all.len();
    let within = |s: &[u8]| -> Option<(u32, u32)> {
        let p = s.as_ptr() as usize;
        if p < base || p + s.len() > end {
            return None;
        }
        Some(((p - base) as u32, s.len() as u32))
    };
    if all.len() > u32::MAX as usize {
        return Ok(b.finish());
    }
    let mut kref: Vec<(u32, u32)> = Vec::with_capacity(n);
    let mut vstart: Vec<u32> = Vec::with_capacity(n + 1);
    let mut vref: Vec<(u32, u32)> = Vec::new();
    let mut lent = true;
    for r in 0..n {
        let k = blob.key_at(r).ok_or_else(|| bad("segment lost a rank"))?;
        let Some(kr) = within(k) else {
            lent = false;
            break;
        };
        kref.push(kr);
        vstart.push(vref.len() as u32);
        let mut ok = true;
        blob.values_at(r, |v| match within(v) {
            Some(vr) => vref.push(vr),
            None => ok = false,
        })?;
        if !ok {
            lent = false;
            break;
        }
    }
    if !lent {
        return Ok(b.finish());
    }
    vstart.push(vref.len() as u32);
    b.seal_starts();
    let pfx = b.common_prefix();
    let heads: Vec<u64> = (0..n).map(|i| head_of(b.key(i), pfx)).collect();
    if level == ScanIndex::Refs {
        return Ok(compose(
            n,
            vref.len(),
            pfx,
            ScanIndex::Refs,
            &heads,
            &kref,
            &vstart,
            &vref,
            &[],
        ));
    }
    // Copy: the same references, pointing into a contiguous copy of the
    // bytes instead of into the segment.
    let mut copy: Vec<u8> = Vec::with_capacity(
        kref.iter().map(|(_, l)| *l as usize).sum::<usize>()
            + vref.iter().map(|(_, l)| *l as usize).sum::<usize>(),
    );
    let mut ck: Vec<(u32, u32)> = Vec::with_capacity(n);
    for (o, l) in &kref {
        ck.push((copy.len() as u32, *l));
        copy.extend_from_slice(&all[*o as usize..(*o + *l) as usize]);
    }
    let mut cv: Vec<(u32, u32)> = Vec::with_capacity(vref.len());
    for (o, l) in &vref {
        cv.push((copy.len() as u32, *l));
        copy.extend_from_slice(&all[*o as usize..(*o + *l) as usize]);
    }
    Ok(compose(
        n,
        cv.len(),
        pfx,
        ScanIndex::Copy,
        &heads,
        &ck,
        &vstart,
        &cv,
        &copy,
    ))
}

/// One segment's ordered index, mapped.
pub struct OrdIndex {
    map: MmapBytes,
    n: usize,
    nvalues: usize,
    pfx: usize,
    level: ScanIndex,
    kref_at: usize,
    vstart_at: usize,
    vref_at: usize,
    copy_at: usize,
    copy_len: usize,
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
        let nvalues = rd(16) as usize;
        let pfx = rd(24) as usize;
        let level = ScanIndex::from_code(rd(32)).ok_or_else(|| bad("unknown level"))?;
        let heads_at = rd(40) as usize;
        let (kref_at, vstart_at, vref_at, copy_at, copy_len) = (
            rd(48) as usize,
            rd(56) as usize,
            rd(64) as usize,
            rd(72) as usize,
            rd(80) as usize,
        );
        if n != keys {
            return Err(bad("describes a different segment"));
        }
        // The regions have to tile the file exactly. A header saying
        // otherwise is damage, and indexing past it later would be a panic
        // rather than an error.
        let sz = |a: usize, b: usize| a.checked_mul(b).ok_or_else(|| bad("a region overflows"));
        let mut at = HEADER;
        if heads_at != at {
            return Err(bad("heads are not where the header says"));
        }
        at += sz(n, HEAD)?;
        let want = |present: bool, off: usize, at: usize| -> bool {
            if present {
                off == at
            } else {
                off == 0
            }
        };
        let refs = level != ScanIndex::Heads;
        if !want(refs, kref_at, at) {
            return Err(bad("the key references are not where the header says"));
        }
        if refs {
            at += sz(n, REF)?;
        }
        if !want(refs, vstart_at, at) {
            return Err(bad("the run bounds are not where the header says"));
        }
        if refs {
            at += sz(n + 1, 4)?;
        }
        if !want(refs, vref_at, at) {
            return Err(bad("the value references are not where the header says"));
        }
        if refs {
            at += sz(nvalues, REF)?;
        }
        let has_copy = level == ScanIndex::Copy && copy_len > 0;
        if !want(has_copy, copy_at, at) {
            return Err(bad("the copy is not where the header says"));
        }
        at += copy_len;
        if at != body {
            return Err(bad("the regions do not fill the file"));
        }
        Ok(OrdIndex {
            map,
            n,
            nvalues,
            pfx,
            level,
            kref_at,
            vstart_at,
            vref_at,
            copy_at,
            copy_len,
        })
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// What this one holds, which the file says rather than the option: a
    /// segment whose values are not lent gets `Heads` whatever was asked.
    pub fn level(&self) -> ScanIndex {
        self.level
    }

    #[inline]
    fn head(&self, i: usize) -> u64 {
        let at = HEADER + i * HEAD;
        u64::from_be_bytes(self.map.0[at..at + HEAD].try_into().expect("eight bytes"))
    }

    #[inline]
    fn at_ref(&self, region: usize, i: usize) -> (usize, usize) {
        let a = region + i * REF;
        let b = &self.map.0;
        (
            u32::from_le_bytes(b[a..a + 4].try_into().expect("four bytes")) as usize,
            u32::from_le_bytes(b[a + 4..a + 8].try_into().expect("four bytes")) as usize,
        )
    }

    #[inline]
    fn run(&self, i: usize) -> (usize, usize) {
        let b = &self.map.0;
        let a = self.vstart_at + i * 4;
        (
            u32::from_le_bytes(b[a..a + 4].try_into().expect("four bytes")) as usize,
            u32::from_le_bytes(b[a + 4..a + 8].try_into().expect("four bytes")) as usize,
        )
    }

    /// The end of the run of heads equal to `h` that starts at `from`.
    ///
    /// Galloping, not a binary search over the whole array. A run is one
    /// entry long whenever the head separates the keys, which is the usual
    /// case -- and a seek for a key that is PRESENT always reaches here,
    /// because its head necessarily matches. Searching the whole array for
    /// the end of a run of one made every such seek pay the binary search
    /// twice.
    #[inline]
    fn run_end(&self, h: u64, from: usize) -> usize {
        let mut lo = from;
        let mut step = 1usize;
        loop {
            let p = from.saturating_add(step);
            if p >= self.n || self.head(p) != h {
                break;
            }
            lo = p;
            step *= 2;
        }
        let mut hi = from.saturating_add(step).min(self.n);
        let mut a = lo + 1;
        while a < hi {
            let m = (a + hi) / 2;
            if self.head(m) == h {
                a = m + 1;
            } else {
                hi = m;
            }
        }
        a
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
        let (mut a, mut b) = (lo, self.run_end(h, lo));
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

    /// Walk `limit` keys from `from`, emitting a key and a value the way
    /// `Blob::scan_at` does. `segment` is the segment's mapped bytes, which
    /// `Refs` points into and `Copy` does not need.
    ///
    /// Returns the keys emitted, or `None` when this index has no references
    /// and the caller must walk the records instead.
    pub fn walk<F: FnMut(&[u8], &[u8])>(
        &self,
        segment: Option<&[u8]>,
        from: usize,
        limit: usize,
        mut f: F,
    ) -> Option<usize> {
        let base: &[u8] = match self.level {
            ScanIndex::Heads => return None,
            ScanIndex::Copy => self.map.0.get(self.copy_at..self.copy_at + self.copy_len)?,
            ScanIndex::Refs => segment?,
        };
        let mut seen = 0usize;
        let mut i = from;
        while seen < limit && i < self.n {
            let (ko, kl) = self.at_ref(self.kref_at, i);
            let k = base.get(ko..ko + kl)?;
            let (a, b) = self.run(i);
            for j in a..b.min(self.nvalues) {
                let (vo, vl) = self.at_ref(self.vref_at, j);
                f(k, base.get(vo..vo + vl)?);
            }
            seen += 1;
            i += 1;
        }
        Some(seen)
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
        assert_eq!(idx.level(), ScanIndex::Heads);
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

    /// A `Heads` index has no references, so a walk over it has no answer
    /// and the caller has to use the records. Saying so is what keeps the
    /// two levels from quietly disagreeing.
    #[test]
    fn a_heads_index_declines_to_walk() {
        let owned: Vec<Vec<u8>> = (0u32..64)
            .map(|i| format!("{i:016}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let p = write(&build(&keys), "nowalk");
        let idx = OrdIndex::open(&p, keys.len()).expect("opens");
        assert!(idx.walk(None, 0, 10, |_, _| {}).is_none());
        let _ = std::fs::remove_file(&p);
    }
}
