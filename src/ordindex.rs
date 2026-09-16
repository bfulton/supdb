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
//! the two to it.
//!
//! When every key has one length, and that length is no more than the
//! prefix plus eight bytes, a head IS its key, and the header records the
//! length: a seek then never reads a record. Equal-length suffixes pad
//! identically, so distinct keys have distinct heads; a query no longer
//! than that length which ties a head is that key or a proper prefix of
//! it, so not below it; a longer query that ties one has that key as a
//! proper prefix, so is above it. The record read the tie search made was
//! 170 ns of a 390 ns seek at 300k keys, and the suite's keys, sixteen
//! digits behind a shared prefix, never needed it. Keys of two lengths can
//! share a head -- `k-1` and `k-1\0` pad to the same eight bytes -- and
//! keys longer than the prefix plus eight can tie on their first eight, so
//! either mix keeps the tie search. The word sits in a header slot older
//! files hold as zero and older readers never read, so the magic stays.
//!
//! It is consulted where `Db::scan` walks the partitions in bulk -- no
//! level-0 piece, the unsealed keys laid over the walk -- because that is
//! the only walk that starts with one seek per partition rather than a
//! merge. A store with a level-0 piece, one being written faster than it
//! seals, does not use it at all. That is worth knowing before attributing
//! a scan change to it. Nothing about the value bytes is duplicated: measured,
//! separating values from keys is a wash on a scan that reads them, and the
//! whole win is on the seek and on walks that do not.
//!
//! **Storing more than this does not make a scan faster, and it was tried.**
//! Two larger levels were built and measured in the engine over 300k keys,
//! six reps: one adding a reference a key and a value so the walk parses no
//! record, and one adding a contiguous copy of the keys and values. Against
//! this file's 1.56 file bytes a stored byte, 416,502 ops/s loading and a
//! 100-key scan at 2,340ns reading lengths and 1,966ns reading values:
//!
//! - references: 1.74 B/B, 385,575 ops/s, 1,237ns (1.89x) and 2,218ns (0.89x)
//! - a full copy: 2.74 B/B, 257,318 ops/s, 1,270ns (1.84x) and 2,166ns (0.91x)
//!
//! Both are FASTER only for a caller that never reads a value byte and
//! slower for one that does, and the copy is no faster than the references
//! for three times the disk. A separate lean record stream -- one sequential
//! stream holding exactly what a scan emits -- was measured too, at 0.99x on
//! one value a key and 1.22x on eight.
//!
//! The reason is general enough to save the next attempt. An over-store
//! cannot win by locality, because it does not replace the records: point
//! reads, counts, deletes and the browser reader all need them, so the
//! segment stays mapped and the seek still touches it, and the second
//! structure only adds to the working set it was meant to beat. A walk over
//! references touches the key references, the value references AND the
//! record bytes, where the record walk touches a directory and the records;
//! it is ahead only while the third stream stays cold, which is exactly when
//! the caller reads no values. The suite's own scan callback sums lengths
//! and reads none, so an option like that moves the figure and not the
//! engine. Anything that beats this has to REPLACE the record rather than
//! accompany it.
//!
//! **A replacement was then built, and it wins warm and loses cold.** A
//! columnar shape -- hash to rank, then separate key, run-bound and value
//! columns -- serves every operation without the records, so it is a
//! replacement rather than a companion. Over 1.5M keys at eight values,
//! half point reads and half 100-key scans, it is 1.84x the record segment
//! with everything resident. Under a memory cap it is 0.5x, because the two
//! shapes are fast for opposite reasons: warm, a scan pays to PARSE, and the
//! columns parse nothing; cold, a scan pays for the streams it TOUCHES, and
//! a record run co-locates the keys and values one window needs where the
//! columns spread them over six regions, each its own chance to miss.
//!
//! Carrying both and routing each operation to the better shape is worse
//! than either, and the way it fails is the part to remember. Each shape's
//! cliff sits at its own footprint, so two shapes move the cliff to their
//! sum: the record segment holds its rate down to a cap of 400 MB against a
//! 378 MB file, and the pair collapses by 400x at that cap. Skew does not
//! save it, because the failure is not in the hot set but in the tail --
//! with 99% of operations inside a tenth of the keys, a hot union of about
//! 77 MB inside a 600 MB cap, the pair still reads 8.9 GB off the device in
//! eight seconds, its footprint twelve times over, while either shape alone
//! reads its file once and then nothing. Nor is a little mixing safe: with
//! the workload entirely scans the record file is never touched, no union
//! forms and the second shape is exactly free, and one point read in two
//! hundred costs 7x. So a second shape may be selected per WORKLOAD and
//! never per operation, extra storage is free only while the union of what
//! a workload touches stays resident, and the thing worth building is
//! neither of these two but one shape that parses like the columns and
//! touches like the records.
//!
//! **That shape was then built, and it dominates both.** Cut the key order
//! into chunks of a few dozen keys and lay every column one scan window
//! needs inside the chunk, contiguously, addressed by `u16` offsets
//! relative to the chunk: nothing parses, and chunks sit in key order so a
//! walk across them is a walk across adjacent bytes. Relative offsets are
//! also what make it the SMALLEST of the three -- two bytes where the flat
//! columns need four -- at 353 MB against the records' 378 and the flat
//! columns' 393 on the same 1.5M keys at eight values.
//!
//! On the mixed workload it is about 1.4x the records warm and 1.2x to 1.8x
//! under a memory cap, and it beats the flat columns everywhere, by 1.1x
//! warm and up to 3.6x capped. The decomposition says why, and it is the
//! whole point: warm, the win is entirely the scan (about 1.7x) and a point
//! read is a shade SLOWER than a record's, because a hash probe and a chunk
//! indirection cost more than a directory probe into an inline run. Cold it
//! wins both, on the fewest faults an operation of the three -- fewer even
//! than the records, whose walk still reads a directory beside them. It is
//! a replacement rather than a companion, so the residency trap above does
//! not apply to it at all.
//!
//! The one axis it lost was the point read, and the fix was to move the
//! offsets rather than to remove them. Splitting a chunk into columns puts
//! a key's key-offset, run-bound, value-offset and bytes in four regions,
//! so a point read takes four scattered lines where a record takes one
//! span: measured, 0.64x the record's rate. Inlining a length before each
//! value gives the point read its one span and gets it back to about parity
//! -- and costs a sixth of the scan, because a length is a SERIAL chain
//! where an offset is not: value j+1 cannot be located until value j's
//! length has been read, while an array of offsets is loads the processor
//! issues at once.
//!
//! Keeping the offsets but putting them INSIDE the entry -- `u16` ends,
//! relative to the entry, ahead of the key and its values -- has both:
//! every value's position is still an independent load, and a point read is
//! still one span. Against the record segment it is then about 1.04x on
//! point reads, 1.3x on scans, 1.2x on the mixed workload warm and 1.4x
//! under a cap, in a file smaller than the records. It beats the split
//! layout on the mixed workload at both ends and trails it only on scans
//! alone, which is the trade to make: the entry-relative offsets cost the
//! same bytes as the chunk-wide arrays they replace, and make the entry
//! relocatable as well.
//!
//! What is measured here is shape against shape with damage detection off
//! on every arm and no compression anywhere. Both are unpriced, and neither
//! is free: a chunk's bytes are lent to the caller, and a decompressed
//! chunk has no bytes to lend.
//!
//! It is not free on the write path, and the suite is where that shows. Its
//! eight bytes a key are eight bytes a key the seal writes and syncs, and a
//! durable load is device bound, so the load axis pays them at any size
//! where no rewrite is being eliminated at the same time. Removing its
//! fsync does not get them back -- measured, that is not where the cost is;
//! the bytes are. Price a change to this file on `load` as well as `scan`.
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
use crate::bytes::{Bytes, MmapBytes};
use std::io::{Error, ErrorKind, Result};
use std::path::Path;

/// "SUPDORD1", little-endian. The trailing digit is the version, and it
/// moves whenever an older reader would misread rather than refuse.
const MAGIC: u64 = 0x3144_524f_4450_5553;
/// Room for the header and the alignment the heads want, in one constant so
/// the writer and the reader cannot disagree about where the body starts.
const HEADER: usize = 64;
const HEAD: usize = 8;
/// Heads between two samples of the top level: a run of 512 bytes, eight
/// cache lines the prefetcher walks, so a seek's cold probes fall in one
/// run instead of across the file.
const TOP_STRIDE: usize = 64;
/// Header word: one more than the keys' common length when every key has
/// one length and it is at most `pfx + HEAD`, so a head is a whole key and
/// a seek needs no record; 0 otherwise, and in every file from before the
/// word existed.
const UNIFORM_AT: usize = 32;

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
        let len0 = self.starts.get(1).map_or(0, |&b| b as usize);
        let uniform = self.n > 0
            && len0 <= pfx + HEAD
            && (0..self.n).all(|i| (self.starts[i + 1] - self.starts[i]) as usize == len0);
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
        w(
            &mut out,
            UNIFORM_AT,
            if uniform { len0 as u64 + 1 } else { 0 },
        );
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
    /// What was last asked of the kernel for this mapping, so a check can
    /// compare the record against the policy that should have set it. The
    /// mapping itself cannot be interrogated portably, and the failure worth
    /// catching is not the kernel ignoring `madvise` -- it is this call site
    /// going away again.
    advised: std::sync::atomic::AtomicBool,
    n: usize,
    pfx: usize,
    /// The keys' one length, when they have one and it is at most
    /// `pfx + HEAD`: a head is then a whole key.
    uniform_len: Option<usize>,
    /// The common prefix, learned from the segment's first key once at
    /// open, so the seek's prefix check reads no record. Absent until
    /// `learn_prefix`; the seek then reads the first key itself.
    prefix: Option<Vec<u8>>,
    /// Every `TOP_STRIDE`th head, built at the first seek and held in
    /// memory: a sixty-fourth of the file, hot after a few seeks, so the
    /// search's probes into the mapping are confined to one run of heads.
    /// Without it a seek over a partition of seven hundred thousand keys
    /// was twenty probes, the lower ten of them cache misses into a 5 MB
    /// file, 0.4 us of a 2.6 us scan at thirty million keys.
    top: std::sync::OnceLock<Vec<u64>>,
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
        // The word is absent, or a length the prefix and the head bound.
        // Anything else is damage the checksum missed, not a length with
        // extra meaning.
        let uniform_len = match rd(UNIFORM_AT) {
            0 => None,
            w => {
                let l = (w - 1) as usize;
                if l < pfx || l > pfx + HEAD {
                    return Err(bad("uniform length is outside the prefix and the head"));
                }
                Some(l)
            }
        };
        // The heads have to fill the file exactly. A header saying otherwise
        // is damage, and indexing past it later would be a panic rather than
        // an error.
        let want = n
            .checked_mul(HEAD)
            .ok_or_else(|| bad("key count overflows"))?;
        if heads_at != HEADER || heads_at.checked_add(want) != Some(body) {
            return Err(bad("heads do not fill the file"));
        }
        Ok(OrdIndex {
            map,
            n,
            pfx,
            uniform_len,
            prefix: None,
            advised: std::sync::atomic::AtomicBool::new(false),
            top: std::sync::OnceLock::new(),
        })
    }

    /// Every key of one length no more than the prefix plus eight bytes,
    /// so a seek reads no record.
    pub fn uniform(&self) -> bool {
        self.uniform_len.is_some()
    }

    /// Learn the common prefix from the segment's first key, which every
    /// key starts with, so the seek's prefix check is a compare against
    /// these bytes and not a record read. A first key shorter than the
    /// prefix is damage; what is learned is what there is, and the seek's
    /// compare is bounded by it either way.
    pub fn learn_prefix(&mut self, first_key: &[u8]) {
        let m = self.pfx.min(first_key.len());
        self.prefix = Some(first_key[..m].to_vec());
    }

    /// `MADV_RANDOM` for the heads.
    ///
    /// Every access to them is `seek`'s binary search, so this mapping is
    /// random in BOTH of the store's phases -- unlike the segment, whose
    /// advice follows the workload and which `Db::advise` flips. It was
    /// advised by nothing at all until a measurement went looking for a
    /// scan that collapsed once the store outgrew memory: under the
    /// kernel's default readahead a probe fetched a window to consume its
    /// eight bytes, and out of core that cost 12% of scan throughput.
    pub fn advise_random(&self) {
        self.map.advise_random();
        self.advised
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether `advise_random` was called on this mapping.
    pub fn advised(&self) -> bool {
        self.advised.load(std::sync::atomic::Ordering::Relaxed)
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

    /// `seek` within ranks `lo..hi`: the first rank there whose key is
    /// not below `key`, or `hi` when none is. The search runs over the
    /// range's heads alone -- a block's sixty-four are eight lines, hot
    /// after the block's first build -- and reads a key only to break a
    /// tie, as `seek` does. A block's build asked this of the records
    /// before, a gallop and a binary search of parsed keys for each of
    /// the block's overlay keys.
    pub fn seek_in<'a>(
        &self,
        lo: usize,
        hi: usize,
        key: &[u8],
        key_at: impl Fn(usize) -> Option<&'a [u8]>,
    ) -> usize {
        let hi = hi.min(self.n);
        if lo >= hi {
            return hi;
        }
        // A query without the common prefix is below or above every key,
        // as `seek` decides it; below is `lo` here, above is `hi`.
        if self.pfx > 0 {
            let m = self.pfx.min(key.len());
            let learned = self.prefix.as_deref();
            let first = match learned {
                Some(p) => p,
                None => match key_at(0) {
                    Some(k) => k,
                    None => return lo,
                },
            };
            let m = m.min(first.len());
            match key[..m].cmp(&first[..m]) {
                std::cmp::Ordering::Less => return lo,
                std::cmp::Ordering::Greater => return hi,
                std::cmp::Ordering::Equal if m < self.pfx => return lo,
                std::cmp::Ordering::Equal => {}
            }
        }
        let h = head_of(key, self.pfx);
        let (mut a, mut b) = (lo, hi);
        while a < b {
            let m = (a + b) / 2;
            if self.head(m) < h {
                a = m + 1;
            } else {
                b = m;
            }
        }
        if a >= hi || self.head(a) != h {
            return a;
        }
        if let Some(len) = self.uniform_len {
            return if key.len() <= len {
                a
            } else {
                self.run_end(h, a).min(hi)
            };
        }
        let (mut a, mut b) = (a, self.run_end(h, a).min(hi));
        while a < b {
            let m = (a + b) / 2;
            match key_at(m) {
                Some(k) if k < key => a = m + 1,
                _ => b = m,
            }
        }
        a
    }

    /// The end of the run of heads equal to `h` that starts at `from`.
    ///
    /// Galloping, not a binary search over the whole array. A run is one
    /// entry long whenever the head separates the keys, which is the usual
    /// case and is every case in the suite -- and a seek for a key that is
    /// PRESENT always reaches here, because its head necessarily matches.
    /// Searching the whole array for the end of a run of one made every such
    /// seek pay the binary search twice: measured, a seek that should cost
    /// one search over the heads plus one key comparison was costing about
    /// three times that.
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
        if self.n == 0 {
            return 0;
        }
        // The heads order only keys that carry the common prefix. A query
        // that does not is above or below every key, and its own first
        // bytes against the prefix say which; its bytes after the prefix,
        // which the heads are, say nothing. The prefix is read off the first
        // key, which every key starts with. A first key the segment will not
        // resolve sorts the query below everything, widening the answer the
        // way the record search does with damage.
        if self.pfx > 0 {
            let m = self.pfx.min(key.len());
            let learned = self.prefix.as_deref();
            let first = match learned {
                Some(p) => p,
                None => match key_at(0) {
                    Some(k) => k,
                    None => return 0,
                },
            };
            let m = m.min(first.len());
            match key[..m].cmp(&first[..m]) {
                std::cmp::Ordering::Less => return 0,
                std::cmp::Ordering::Greater => return self.n,
                std::cmp::Ordering::Equal if m < self.pfx => return 0,
                std::cmp::Ordering::Equal => {}
            }
        }
        let h = head_of(key, self.pfx);
        // The samples below the query: the answer lies past the last of
        // them and no further than the next, so the search over the heads
        // runs within one stride.
        let top = self.top.get_or_init(|| {
            (0..self.n)
                .step_by(TOP_STRIDE)
                .map(|i| self.head(i))
                .collect()
        });
        let t = top.partition_point(|&s| s < h);
        // No sample below the query is the first head at or above it. Past
        // the last sample below it, the search runs to the next sample and
        // lands on it when every head between is below.
        let (mut lo, mut hi) = if t == 0 {
            (0, 0)
        } else {
            ((t - 1) * TOP_STRIDE + 1, (t * TOP_STRIDE).min(self.n))
        };
        // The stride's eight lines together: the search below probes
        // three of them one after another, each a miss on a cold
        // partition, and issued at once they cost one.
        if lo < hi {
            let at = HEADER + lo * HEAD;
            crate::db::prefetch_lines(self.map.0[at..].as_ptr(), (hi - lo) * HEAD);
        }
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
        if let Some(len) = self.uniform_len {
            // A head is a whole key. A query no longer than the keys that
            // ties one is that key or a proper prefix of it, so not below
            // it; a longer query has the key with this head as a proper
            // prefix, so is above it.
            return if key.len() <= len {
                lo
            } else {
                self.run_end(h, lo)
            };
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

    /// A query that does not carry the segment's common prefix: above every
    /// key or below every key by its first bytes, and its bytes after the
    /// prefix, which the heads are, say nothing about which. The seek once
    /// took them anyway, and a scan from past the last key of a partition
    /// whose keys share a prefix walked that partition from its first key.
    #[test]
    fn a_query_outside_the_common_prefix_seeks_to_an_end() {
        let owned: Vec<Vec<u8>> = (552u32..600)
            .step_by(3)
            .map(|i| format!("key-{i:05}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let p = write(&build(&keys), "offprefix");
        let idx = OrdIndex::open(&p, keys.len()).expect("opens");
        for probe in [
            "zzz",       // above, shorter than the prefix
            "kez",       // above, differs inside the prefix
            "key-006",   // above, differs at the prefix's last byte
            "a",         // below, shorter than the prefix
            "abc-99zzz", // below, with bytes after the prefix that read high
            "key-00",    // below: a proper prefix of the common prefix
            "key-005",   // the prefix itself: below the first key
            "key-00552", // the first key
            "key-00598", // above the last key, carrying the prefix
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

    /// Sixteen-digit keys behind a twelve-byte prefix have one length,
    /// four short of the prefix plus eight, so the index records it and a
    /// seek reads no record: the resolver here panics if asked. Present
    /// keys, keys between, shorter and longer queries, and queries outside
    /// the prefix, against the lower bound over the keys.
    #[test]
    fn a_uniform_index_seeks_without_reading_a_record() {
        let owned: Vec<Vec<u8>> = (0u32..3_000)
            .map(|i| format!("{i:016}").into_bytes())
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let p = write(&build(&keys), "uniform");
        let mut idx = OrdIndex::open(&p, keys.len()).expect("opens");
        assert!(idx.uniform(), "every key has one length within prefix + 8");
        idx.learn_prefix(keys[0]);
        let never = |r: usize| -> Option<&'static [u8]> { panic!("read record {r}") };
        let mut probes: Vec<Vec<u8>> = owned.clone();
        for i in [0u32, 1, 7, 1_005, 2_999] {
            let k = format!("{i:016}");
            probes.push(format!("{k}x").into_bytes()); // longer: above the key
            probes.push(format!("{k}\0").into_bytes()); // longer by a zero: still above
            probes.push(k.as_bytes()[..15].to_vec()); // shorter: a prefix, so not above
            probes.push(k.as_bytes()[..12].to_vec());
        }
        probes.push(b"0000000000005000".to_vec()); // between, past the end
        probes.push(b"".to_vec());
        probes.push(b"9".to_vec());
        probes.push(b"00000000000".to_vec()); // a proper prefix of the prefix
        for q in &probes {
            assert_eq!(
                idx.seek(q, never),
                lower_bound(&keys, q),
                "probe {:?}",
                String::from_utf8_lossy(q)
            );
        }
        let _ = std::fs::remove_file(&p);
    }

    /// Keys shorter than the prefix plus eight pad their heads with zeros,
    /// so `k-1`, `k-1\0` and `k-1\0\0` share one head while being three
    /// keys in order. The flag must stay off for such a file and the tie
    /// search must still answer exactly, resolver and all.
    #[test]
    fn mixed_lengths_keep_the_tie_search() {
        let mut owned: Vec<Vec<u8>> = Vec::new();
        for i in 0u32..50 {
            let base = format!("k-{i:02}").into_bytes();
            owned.push(base.clone());
            let mut z1 = base.clone();
            z1.push(0);
            owned.push(z1);
            let mut z2 = base.clone();
            z2.extend_from_slice(&[0, 0]);
            owned.push(z2);
            let mut full = base.clone();
            full.extend_from_slice(b"000000"); // exactly prefix + 8
            owned.push(full);
        }
        owned.sort();
        owned.dedup();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let p = write(&build(&keys), "mixed");
        let idx = OrdIndex::open(&p, keys.len()).expect("opens");
        assert!(!idx.uniform(), "lengths differ, so a head is not a key");
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(
                idx.seek(k, resolver(&keys)),
                lower_bound(&keys, k),
                "present {i}"
            );
        }
        for q in [
            &b"k-07\0"[..],
            b"k-07\0\0\0",
            b"k-07x",
            b"k-0",
            b"k-99",
            b"k-070000000",
        ] {
            assert_eq!(
                idx.seek(q, resolver(&keys)),
                lower_bound(&keys, q),
                "probe {:?}",
                String::from_utf8_lossy(q)
            );
        }
        let _ = std::fs::remove_file(&p);
    }

    /// One length, but longer than the prefix plus eight: keys can tie on
    /// their first eight suffix bytes and differ after, so the index must
    /// not claim a head is a key.
    #[test]
    fn equal_lengths_past_the_head_keep_the_tie_search() {
        let owned: Vec<Vec<u8>> = (0u32..40)
            .flat_map(|i| (0u32..5).map(move |j| format!("pre{i:02}000000{j:02}").into_bytes()))
            .collect();
        let keys: Vec<&[u8]> = owned.iter().map(|k| k.as_slice()).collect();
        let p = write(&build(&keys), "toolong");
        let idx = OrdIndex::open(&p, keys.len()).expect("opens");
        assert!(
            !idx.uniform(),
            "fifteen-byte keys behind a three-byte prefix tie on their heads"
        );
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(
                idx.seek(k, resolver(&keys)),
                lower_bound(&keys, k),
                "present {i}"
            );
        }
        for q in [
            &b"pre0700000003"[..],
            b"pre07000000",
            b"pre0700000004x",
            b"pre99",
        ] {
            assert_eq!(
                idx.seek(q, resolver(&keys)),
                lower_bound(&keys, q),
                "probe {:?}",
                String::from_utf8_lossy(q)
            );
        }
        let _ = std::fs::remove_file(&p);
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
