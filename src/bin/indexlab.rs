//! Index layout laboratory.
//!
//! The falsification suite measures the engine as it is. This measures a
//! *proposed replacement* for its weakest part, before anyone writes a merge
//! path for it.
//!
//! The claim under test is narrow and specific: that the reader index's
//! problem is **layout, not algorithmic complexity**. At 10M keys a point
//! lookup costs ~1.67us -- roughly five thousand cycles for work that should
//! take a few hundred -- and the index occupies 131 bytes per key to record a
//! 16-byte key and a 16-byte extent. Neither number is explained by a
//! complexity class. Both are explained by scattered heap allocations.
//!
//! Four layouts, same keys, same extents, same machine:
//!
//!   heap-hash   what the reader does today: Vec<(Vec<u8>, Extents)> plus an
//!               open-addressed hash of (tag, index). One allocation per key.
//!   btree       a bulk-loaded, page-based B+tree in a flat buffer, the shape
//!               LMDB uses. Loaded at 100% fill, which is generous: a mutated
//!               B+tree runs nearer 65-70%.
//!   packed      sorted keys, prefix-compressed between restart points, extents
//!               varint-packed, with a restart array carrying an eight-byte key
//!               prefix so most of the binary search never touches the blob.
//!   packed+radix  the same, with a radix table over the top bits of the key
//!               prefix replacing most of the binary search. This is the radix
//!               layer of RadixSpline without the spline -- the part that is
//!               simple enough to be honest about.
//!
//! Three key shapes, because a structure that indexes by key *value* rather
//! than by comparison is only as good as its assumption about the
//! distribution. This project has already been burned once by exactly that:
//! FxHash cost a factor of ten on fixed-width decimal keys. `clustered` is the
//! shape that punishes the radix table, and it is here so the failure mode
//! shows up rather than being discovered later.

use std::path::PathBuf;
use std::time::Instant;
use supdb::bench::{compare, env, Finding, Profile, Record, Rng, Trial, Verdict, J};
use supdb::jobj;

// ------------------------------------------------------------------ types --

/// Mirrors `index::Ext`: block, offset, length, last-record offset.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Ext4 {
    block: u32,
    off: u32,
    len: u32,
    last: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// db_bench's shape: sixteen zero-padded decimal digits. Dense and smooth.
    Decimal16,
    /// Sixteen hex characters of a random u64. Uniform over the key space.
    RandomHex,
    /// Dense islands separated by large gaps -- the shape that defeats a radix
    /// table, because most of its buckets are empty and a few hold everything.
    Clustered,
}

impl Shape {
    fn parse(s: &str) -> Option<Shape> {
        match s {
            "decimal16" => Some(Shape::Decimal16),
            "randomhex" => Some(Shape::RandomHex),
            "clustered" => Some(Shape::Clustered),
            _ => None,
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            Shape::Decimal16 => "decimal16",
            Shape::RandomHex => "randomhex",
            Shape::Clustered => "clustered",
        }
    }
}

/// Distinct keys, sorted. Sorted because every candidate but the hash needs
/// order, and because the engine already sorts each seal batch.
fn make_keys(shape: Shape, n: usize) -> Vec<Vec<u8>> {
    let mut rng = Rng::new(0x1DEA);
    let mut keys: Vec<Vec<u8>> = Vec::with_capacity(n);
    match shape {
        Shape::Decimal16 => {
            for i in 0..n {
                keys.push(format!("{:016}", i).into_bytes());
            }
        }
        Shape::RandomHex => {
            let mut seen = std::collections::HashSet::with_capacity(n * 2);
            while keys.len() < n {
                let v = rng.next();
                if seen.insert(v) {
                    keys.push(format!("{:016x}", v).into_bytes());
                }
            }
        }
        Shape::Clustered => {
            // 64 dense islands spread across the space. Within an island keys
            // are consecutive; between islands the gap is enormous.
            let islands = 64u64;
            let per = (n as u64).div_ceil(islands);
            let mut seen = std::collections::HashSet::with_capacity(n * 2);
            for i in 0..islands {
                let base = (i.wrapping_mul(0x9E37_79B9_7F4A_7C15)) >> 8 << 20;
                for j in 0..per {
                    if keys.len() >= n {
                        break;
                    }
                    let v = base.wrapping_add(j);
                    if seen.insert(v) {
                        keys.push(format!("{:016x}", v).into_bytes());
                    }
                }
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn make_exts(n: usize) -> Vec<Ext4> {
    (0..n)
        .map(|i| Ext4 {
            block: (i / 400) as u32,
            off: ((i % 400) * 160) as u32,
            len: 160,
            last: 140,
        })
        .collect()
}

/// First eight bytes of a key as a big-endian u64, zero-padded.
///
/// Order-preserving on the prefix, which is what lets the restart array and
/// the radix table filter without touching the key bytes.
#[inline]
fn prefix8(key: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    let n = key.len().min(8);
    b[..n].copy_from_slice(&key[..n]);
    u64::from_be_bytes(b)
}

/// Records the byte ranges a lookup actually reads, so the number of distinct
/// cache lines and pages it touches can be counted directly.
///
/// This exists because the machine has no usable PMU: it is a Firecracker
/// guest, and `perf` reports every hardware counter as `<not supported>`. The
/// cache-miss model can still be tested, but its *inputs* have to be measured
/// rather than its outputs -- count the distinct lines a lookup touches, and
/// compare that against the latency it costs.
#[derive(Default)]
struct Trace {
    lines: std::collections::HashSet<usize>,
    pages: std::collections::HashSet<usize>,
    reads: usize,
}

impl Trace {
    #[inline]
    fn touch(&mut self, addr: usize, len: usize) {
        self.reads += 1;
        let mut a = addr & !63;
        let end = addr + len.max(1);
        while a < end {
            self.lines.insert(a >> 6);
            self.pages.insert(a >> 12);
            a += 64;
        }
    }
}

trait Layout {
    /// Reported alongside every measurement, so a row can never be attributed
    /// to the wrong structure.
    fn name(&self) -> &'static str;
    /// Bytes the structure occupies, by its own accounting. The measured RSS
    /// figure is taken separately in a child process, because this one cannot
    /// see the allocator's overhead and that overhead is most of the story.
    fn logical_bytes(&self) -> usize;
    fn lookup(&self, key: &[u8]) -> Option<Ext4>;
    /// Visit `n` records in key order from sorted position `start`, returning
    /// an accumulator so the work cannot be optimised away. Ordered scan is a
    /// category in its own right and a layout that wins point lookups by
    /// giving up order has not won anything.
    ///
    /// Every implementation must touch the key as well as the extent. The
    /// engine's `scan` hands `(key, value)` to a visitor, so it dereferences
    /// the key on every entry; a scan benchmark that reads only an inline
    /// field measures something the engine never does, and flatters exactly
    /// the layout that keys behind a pointer.
    fn scan_from(&self, start: usize, n: usize) -> u64;
    /// Mirror of `lookup` that records what it reads. Implemented only for the
    /// layouts under investigation; the default reports nothing so the others
    /// are visibly absent rather than silently zero.
    fn trace_lookup(&self, _key: &[u8], _t: &mut Trace) -> bool {
        false
    }
}

// ------------------------------------------------------------- heap-hash --

/// What `Reader::build` does today.
struct HeapHash {
    entries: Vec<(Vec<u8>, Ext4)>,
    hash: Vec<(u8, u32)>,
    mask: usize,
}

fn key_hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

impl HeapHash {
    fn build(keys: &[Vec<u8>], exts: &[Ext4]) -> HeapHash {
        let entries: Vec<(Vec<u8>, Ext4)> =
            keys.iter().cloned().zip(exts.iter().copied()).collect();
        let mut cap = 1usize;
        while cap < entries.len() * 2 {
            cap <<= 1;
        }
        cap = cap.max(16);
        let mask = cap - 1;
        let mut hash = vec![(0u8, u32::MAX); cap];
        for (i, (k, _)) in entries.iter().enumerate() {
            let h = key_hash(k);
            let mut slot = (h as usize) & mask;
            while hash[slot].1 != u32::MAX {
                slot = (slot + 1) & mask;
            }
            hash[slot] = (((h >> 56) as u8) | 1, i as u32);
        }
        HeapHash {
            entries,
            hash,
            mask,
        }
    }
}

impl Layout for HeapHash {
    fn name(&self) -> &'static str {
        "heap-hash"
    }
    fn logical_bytes(&self) -> usize {
        // Vec slots, plus each key's own heap allocation. Allocator headers and
        // size-class rounding are invisible here and are exactly why the
        // measured RSS runs well above this.
        self.entries.capacity() * std::mem::size_of::<(Vec<u8>, Ext4)>()
            + self
                .entries
                .iter()
                .map(|(k, _)| k.capacity())
                .sum::<usize>()
            + self.hash.capacity() * std::mem::size_of::<(u8, u32)>()
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        loop {
            let (t, i) = self.hash[slot];
            if i == u32::MAX {
                return None;
            }
            if t == tag {
                let e = &self.entries[i as usize];
                if e.0.as_slice() == key {
                    return Some(e.1);
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn trace_lookup(&self, key: &[u8], t: &mut Trace) -> bool {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        let hbase = self.hash.as_ptr() as usize;
        let ebase = self.entries.as_ptr() as usize;
        let esz = std::mem::size_of::<(Vec<u8>, Ext4)>();
        loop {
            t.touch(hbase + slot * 8, 8);
            let (tg, i) = self.hash[slot];
            if i == u32::MAX {
                return true;
            }
            if tg == tag {
                // The entry record, then the key it points at: two separate
                // regions, which is the whole point of the comparison.
                t.touch(ebase + i as usize * esz, esz);
                let e = &self.entries[i as usize];
                t.touch(e.0.as_ptr() as usize, e.0.len());
                if e.0.as_slice() == key {
                    return true;
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        let end = (start + n).min(self.entries.len());
        let mut acc = 0u64;
        for (k, e) in &self.entries[start.min(end)..end] {
            // The pointer chase the engine's scan actually pays.
            acc = acc.wrapping_add(e.block as u64).wrapping_add(k[0] as u64);
        }
        acc
    }
}

// ------------------------------------------------------------ varint I/O --

fn put_uv(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

#[inline]
fn get_uv(buf: &[u8], p: &mut usize) -> u64 {
    let mut v = 0u64;
    let mut shift = 0u32;
    while *p < buf.len() {
        let b = buf[*p];
        *p += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b < 0x80 {
            return v;
        }
        shift += 7;
        if shift >= 64 {
            break;
        }
    }
    v
}

/// Keys between restarts share a prefix with their predecessor.
const RESTART_EVERY: usize = 16;

// ---------------------------------------------------------------- packed --

/// Sorted keys, prefix-compressed between restarts, extents varint-packed.
///
/// The restart array carries an eight-byte key prefix alongside the blob
/// offset, so the binary search runs over a compact array of 12-byte entries
/// and only reads the blob when two prefixes tie. For ten million keys that
/// array is 7.5 MB against a 2 GB heap structure -- the difference between a
/// search that mostly hits cache and one that mostly does not.
struct Packed {
    /// Bytes common to every key, skipped before deriving the eight-byte
    /// prefix. Without this, sixteen-digit decimal keys all share a leading
    /// '0' and the radix table collapses to a single bucket -- the same shape
    /// that cost this project a factor of ten when it tried FxHash.
    lcp: usize,
    blob: Vec<u8>,
    /// (first eight bytes of the restart's key, byte offset into blob)
    restarts: Vec<(u64, u32)>,
    /// Radix table over the top bits of the prefix, when enabled.
    radix: Vec<u32>,
    radix_bits: u32,
    named: &'static str,
}

impl Packed {
    fn describe(&self) -> &'static str {
        self.named
    }
}

impl Packed {
    fn build(keys: &[Vec<u8>], exts: &[Ext4], radix_bits: u32) -> Packed {
        let lcp = match (keys.first(), keys.last()) {
            (Some(a), Some(b)) => a.iter().zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        };
        let mut blob = Vec::with_capacity(keys.len() * 16);
        let mut restarts = Vec::with_capacity(keys.len() / RESTART_EVERY + 1);
        let mut prev: &[u8] = &[];
        for (i, k) in keys.iter().enumerate() {
            let restart = i % RESTART_EVERY == 0;
            if restart {
                restarts.push((prefix8(&k[lcp.min(k.len())..]), blob.len() as u32));
                prev = &[];
            }
            let shared = if restart {
                0
            } else {
                k.iter().zip(prev).take_while(|(a, b)| a == b).count()
            };
            put_uv(&mut blob, shared as u64);
            put_uv(&mut blob, (k.len() - shared) as u64);
            blob.extend_from_slice(&k[shared..]);
            let e = exts[i];
            put_uv(&mut blob, e.block as u64);
            put_uv(&mut blob, e.off as u64);
            put_uv(&mut blob, e.len as u64);
            put_uv(&mut blob, e.last as u64);
            prev = k;
        }

        let mut radix = Vec::new();
        if radix_bits > 0 {
            let buckets = 1usize << radix_bits;
            radix = vec![u32::MAX; buckets + 1];
            for (j, (p, _)) in restarts.iter().enumerate() {
                let b = (p >> (64 - radix_bits)) as usize;
                if radix[b] == u32::MAX {
                    radix[b] = j as u32;
                }
            }
            // Fill gaps backwards so an empty bucket points at the next
            // populated restart; the last sentinel is the end.
            radix[buckets] = restarts.len() as u32;
            for b in (0..buckets).rev() {
                if radix[b] == u32::MAX {
                    radix[b] = radix[b + 1];
                }
            }
        }
        blob.shrink_to_fit();
        restarts.shrink_to_fit();
        Packed {
            lcp,
            blob,
            restarts,
            radix,
            radix_bits,
            named: if radix_bits > 0 {
                "packed+radix"
            } else {
                "packed"
            },
        }
    }

    /// Full key at a restart, needed only to break prefix ties.
    fn restart_key(&self, i: usize) -> &[u8] {
        let mut p = self.restarts[i].1 as usize;
        let _shared = get_uv(&self.blob, &mut p);
        let len = get_uv(&self.blob, &mut p) as usize;
        &self.blob[p..(p + len).min(self.blob.len())]
    }

    /// The last restart whose key is <= `key`.
    fn seek_restart(&self, key: &[u8]) -> Option<usize> {
        let k8 = prefix8(&key[self.lcp.min(key.len())..]);
        let (mut lo, mut hi) = if self.radix_bits > 0 {
            let b = (k8 >> (64 - self.radix_bits)) as usize;
            // The bucket's own start can be past our key, so begin one back.
            let start = self.radix[b].saturating_sub(1) as usize;
            let end = (self.radix[b + 1] as usize + 1).min(self.restarts.len());
            (start, end)
        } else {
            (0usize, self.restarts.len())
        };
        if self.restarts.is_empty() {
            return None;
        }
        // Invariant: everything below `lo` is <= key. Compare on the packed
        // prefix first; only a tie costs a blob read.
        let mut best: Option<usize> = None;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let p = self.restarts[mid].0;
            let ord = if p != k8 {
                p.cmp(&k8)
            } else {
                self.restart_key(mid).cmp(key)
            };
            if ord == std::cmp::Ordering::Greater {
                hi = mid;
            } else {
                best = Some(mid);
                lo = mid + 1;
            }
        }
        // With a radix table the window started mid-array; if nothing in it
        // qualified, the answer is below the window.
        if best.is_none() && self.radix_bits > 0 {
            let mut lo = 0usize;
            let mut hi = self.restarts.len();
            while lo < hi {
                let mid = (lo + hi) / 2;
                let p = self.restarts[mid].0;
                let ord = if p != k8 {
                    p.cmp(&k8)
                } else {
                    self.restart_key(mid).cmp(key)
                };
                if ord == std::cmp::Ordering::Greater {
                    hi = mid;
                } else {
                    best = Some(mid);
                    lo = mid + 1;
                }
            }
        }
        best
    }
}

impl Packed {
    /// The record at sorted position `rank`, if its key matches.
    fn at_rank(&self, rank: usize, key: &[u8]) -> Option<Ext4> {
        let r = rank / RESTART_EVERY;
        let skip = rank % RESTART_EVERY;
        let mut p = *self.restarts.get(r).map(|(_, o)| o)? as usize;
        let mut cur = [0u8; 128];
        let mut cur_len;
        for step in 0..=skip {
            let shared = get_uv(&self.blob, &mut p) as usize;
            let suffix = get_uv(&self.blob, &mut p) as usize;
            if shared + suffix > cur.len() || p + suffix > self.blob.len() {
                return None;
            }
            cur[shared..shared + suffix].copy_from_slice(&self.blob[p..p + suffix]);
            cur_len = shared + suffix;
            p += suffix;
            let e = Ext4 {
                block: get_uv(&self.blob, &mut p) as u32,
                off: get_uv(&self.blob, &mut p) as u32,
                len: get_uv(&self.blob, &mut p) as u32,
                last: get_uv(&self.blob, &mut p) as u32,
            };
            if step == skip {
                return if cur[..cur_len] == *key {
                    Some(e)
                } else {
                    None
                };
            }
        }
        None
    }
}

impl Layout for Packed {
    fn name(&self) -> &'static str {
        self.describe()
    }
    fn logical_bytes(&self) -> usize {
        self.blob.capacity()
            + self.restarts.capacity() * std::mem::size_of::<(u64, u32)>()
            + self.radix.capacity() * 4
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let start = self.seek_restart(key)?;
        let mut p = self.restarts[start].1 as usize;
        let end = self
            .restarts
            .get(start + 1)
            .map(|(_, o)| *o as usize)
            .unwrap_or(self.blob.len());
        // A Vec here allocated once per lookup and dominated the measurement.
        let mut cur = [0u8; 128];
        let mut cur_len;
        while p < end {
            let shared = get_uv(&self.blob, &mut p) as usize;
            let suffix = get_uv(&self.blob, &mut p) as usize;
            if p + suffix > self.blob.len() {
                return None;
            }
            if shared + suffix > cur.len() {
                return None;
            }
            cur[shared..shared + suffix].copy_from_slice(&self.blob[p..p + suffix]);
            cur_len = shared + suffix;
            p += suffix;
            let e = Ext4 {
                block: get_uv(&self.blob, &mut p) as u32,
                off: get_uv(&self.blob, &mut p) as u32,
                len: get_uv(&self.blob, &mut p) as u32,
                last: get_uv(&self.blob, &mut p) as u32,
            };
            match cur[..cur_len].cmp(key) {
                std::cmp::Ordering::Equal => return Some(e),
                std::cmp::Ordering::Greater => return None,
                std::cmp::Ordering::Less => {}
            }
        }
        None
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        let r = start / RESTART_EVERY;
        let Some((_, off)) = self.restarts.get(r) else {
            return 0;
        };
        let mut p = *off as usize;
        let mut acc = 0u64;
        let mut seen = 0usize;
        let mut idx = r * RESTART_EVERY;
        while p < self.blob.len() && seen < n {
            let _shared = get_uv(&self.blob, &mut p) as usize;
            let suffix = get_uv(&self.blob, &mut p) as usize;
            let kb = if suffix > 0 { self.blob[p] as u64 } else { 0 };
            p += suffix;
            let b = get_uv(&self.blob, &mut p).wrapping_add(kb);
            let _ = get_uv(&self.blob, &mut p);
            let _ = get_uv(&self.blob, &mut p);
            let _ = get_uv(&self.blob, &mut p);
            if idx >= start {
                acc = acc.wrapping_add(b);
                seen += 1;
            }
            idx += 1;
        }
        acc
    }
}

// ---------------------------------------------------- hash over a packed blob --

/// The current hash, with the heap `Vec<(Vec<u8>, Extents)>` behind it replaced
/// by the packed blob.
///
/// This is the layout that isolates the hypothesis. The reader's hash is
/// already a flat array of eight-byte slots; what sits behind it is one heap
/// allocation per key plus a fat entry record. If the lookup cost is dominated
/// by chasing that pointer rather than by the probe, then keeping the probe and
/// replacing only what it points at should recover most of the speed at a
/// fraction of the space.
struct HashPacked {
    packed: Packed,
    /// (tag, sorted rank). Flat, mmap-able, and shared -- no per-key allocation.
    hash: Vec<(u8, u32)>,
    mask: usize,
}

impl HashPacked {
    fn build(keys: &[Vec<u8>], exts: &[Ext4]) -> HashPacked {
        let packed = Packed::build(keys, exts, 0);
        let mut cap = 1usize;
        while cap < keys.len() * 2 {
            cap <<= 1;
        }
        cap = cap.max(16);
        let mask = cap - 1;
        let mut hash = vec![(0u8, u32::MAX); cap];
        for (i, k) in keys.iter().enumerate() {
            let h = key_hash(k);
            let mut slot = (h as usize) & mask;
            while hash[slot].1 != u32::MAX {
                slot = (slot + 1) & mask;
            }
            hash[slot] = (((h >> 56) as u8) | 1, i as u32);
        }
        HashPacked { packed, hash, mask }
    }
}

impl Layout for HashPacked {
    fn name(&self) -> &'static str {
        "hash+packed"
    }
    fn logical_bytes(&self) -> usize {
        self.packed.logical_bytes() + self.hash.capacity() * std::mem::size_of::<(u8, u32)>()
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        loop {
            let (t, rank) = self.hash[slot];
            if rank == u32::MAX {
                return None;
            }
            if t == tag {
                // The rank is the key's position in sorted order, so the
                // restart that covers it is rank / RESTART_EVERY. Decode from
                // there and verify the full key -- a tag match is not proof,
                // and this project has already argued that a fingerprint match
                // must never be treated as one.
                if let Some(e) = self.packed.at_rank(rank as usize, key) {
                    return Some(e);
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        self.packed.scan_from(start, n)
    }
}

// ------------------------------------------------- hash over flat records --

/// Flat hash of (tag, byte offset) over self-contained records.
///
/// `hash+packed` showed where the cost actually sits: its misses are as fast as
/// the current hash (an empty slot short-circuits) but its hits are not,
/// because reaching sorted position `rank` means decoding up to sixteen
/// prefix-compressed records from the preceding restart. Prefix compression is
/// the index's own read-amplification dial, and it is turned the wrong way for
/// point lookups.
///
/// So: give up prefix compression, keep everything else. Records are
/// self-contained, the hash points straight at one, and a lookup is a probe
/// plus a single contiguous read. A sparse restart array remains for ordered
/// scans, which are the only thing prefix compression was buying.
struct HashFlat {
    blob: Vec<u8>,
    hash: Vec<(u8, u32)>,
    mask: usize,
    /// Every 16th record, for ordered scans.
    restarts: Vec<u32>,
}

impl HashFlat {
    fn build(keys: &[Vec<u8>], exts: &[Ext4]) -> HashFlat {
        let mut blob = Vec::with_capacity(keys.len() * 24);
        let mut offs = Vec::with_capacity(keys.len());
        let mut restarts = Vec::with_capacity(keys.len() / RESTART_EVERY + 1);
        for (i, k) in keys.iter().enumerate() {
            if i % RESTART_EVERY == 0 {
                restarts.push(blob.len() as u32);
            }
            offs.push(blob.len() as u32);
            blob.extend_from_slice(&(k.len() as u16).to_le_bytes());
            blob.extend_from_slice(k);
            let e = exts[i];
            put_uv(&mut blob, e.block as u64);
            put_uv(&mut blob, e.off as u64);
            put_uv(&mut blob, e.len as u64);
            put_uv(&mut blob, e.last as u64);
        }
        blob.shrink_to_fit();
        restarts.shrink_to_fit();
        let mut cap = 1usize;
        while cap < keys.len() * 2 {
            cap <<= 1;
        }
        cap = cap.max(16);
        let mask = cap - 1;
        let mut hash = vec![(0u8, u32::MAX); cap];
        for (i, k) in keys.iter().enumerate() {
            let h = key_hash(k);
            let mut slot = (h as usize) & mask;
            while hash[slot].1 != u32::MAX {
                slot = (slot + 1) & mask;
            }
            hash[slot] = (((h >> 56) as u8) | 1, offs[i]);
        }
        HashFlat {
            blob,
            hash,
            mask,
            restarts,
        }
    }
}

impl Layout for HashFlat {
    fn name(&self) -> &'static str {
        "hash+flat"
    }
    fn logical_bytes(&self) -> usize {
        self.blob.capacity()
            + self.hash.capacity() * std::mem::size_of::<(u8, u32)>()
            + self.restarts.capacity() * 4
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        loop {
            let (t, off) = self.hash[slot];
            if off == u32::MAX {
                return None;
            }
            if t == tag {
                let mut p = off as usize;
                let kl = u16::from_le_bytes(self.blob[p..p + 2].try_into().unwrap()) as usize;
                p += 2;
                if &self.blob[p..p + kl] == key {
                    p += kl;
                    return Some(Ext4 {
                        block: get_uv(&self.blob, &mut p) as u32,
                        off: get_uv(&self.blob, &mut p) as u32,
                        len: get_uv(&self.blob, &mut p) as u32,
                        last: get_uv(&self.blob, &mut p) as u32,
                    });
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn trace_lookup(&self, key: &[u8], t: &mut Trace) -> bool {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        let hbase = self.hash.as_ptr() as usize;
        let bbase = self.blob.as_ptr() as usize;
        loop {
            t.touch(hbase + slot * 8, 8);
            let (tg, off) = self.hash[slot];
            if off == u32::MAX {
                return true;
            }
            if tg == tag {
                let p = off as usize;
                let kl = u16::from_le_bytes(self.blob[p..p + 2].try_into().unwrap()) as usize;
                // One contiguous record: length, key, extent varints.
                t.touch(bbase + p, 2 + kl + 24);
                if self.blob[p + 2..p + 2 + kl] == *key {
                    return true;
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        let r = start / RESTART_EVERY;
        let Some(off) = self.restarts.get(r) else {
            return 0;
        };
        let mut p = *off as usize;
        let mut acc = 0u64;
        let mut seen = 0usize;
        let mut idx = r * RESTART_EVERY;
        while p < self.blob.len() && seen < n {
            let kl = u16::from_le_bytes(self.blob[p..p + 2].try_into().unwrap()) as usize;
            let kb = if kl > 0 { self.blob[p + 2] as u64 } else { 0 };
            p += 2 + kl;
            let b = get_uv(&self.blob, &mut p).wrapping_add(kb);
            let _ = get_uv(&self.blob, &mut p);
            let _ = get_uv(&self.blob, &mut p);
            let _ = get_uv(&self.blob, &mut p);
            if idx >= start {
                acc = acc.wrapping_add(b);
                seen += 1;
            }
            idx += 1;
        }
        acc
    }
}

// ------------------------------- hash over flat records, fixed-width extents --

/// `hash+flat` with the extent stored as four little-endian u32s instead of
/// four varints.
///
/// One variable, to test one hypothesis. `hash+flat` touches fewer cache lines
/// and fewer pages than the current heap layout (2.67 and 2.01 against 3.53 and
/// 3.01, measured) and is nonetheless 29% slower. Subtracting the miss path,
/// which is identical for both, heap-hash's two extra memory accesses cost
/// 123ns while hash+flat's single access costs 224ns -- so the extra time is
/// not being spent waiting for memory. Varint decoding is a serial, branchy
/// loop of eight or so iterations with data-dependent exits, and it is the only
/// other thing on that path. Sixteen fixed bytes instead removes it.
struct HashFlatFixed {
    blob: Vec<u8>,
    hash: Vec<(u8, u32)>,
    mask: usize,
    restarts: Vec<u32>,
}

impl HashFlatFixed {
    fn build(keys: &[Vec<u8>], exts: &[Ext4]) -> HashFlatFixed {
        let mut blob = Vec::with_capacity(keys.len() * 34);
        let mut offs = Vec::with_capacity(keys.len());
        let mut restarts = Vec::with_capacity(keys.len() / RESTART_EVERY + 1);
        for (i, k) in keys.iter().enumerate() {
            if i % RESTART_EVERY == 0 {
                restarts.push(blob.len() as u32);
            }
            offs.push(blob.len() as u32);
            blob.extend_from_slice(&(k.len() as u16).to_le_bytes());
            blob.extend_from_slice(k);
            let e = exts[i];
            for v in [e.block, e.off, e.len, e.last] {
                blob.extend_from_slice(&v.to_le_bytes());
            }
        }
        blob.shrink_to_fit();
        restarts.shrink_to_fit();
        let mut cap = 1usize;
        while cap < keys.len() * 2 {
            cap <<= 1;
        }
        cap = cap.max(16);
        let mask = cap - 1;
        let mut hash = vec![(0u8, u32::MAX); cap];
        for (i, k) in keys.iter().enumerate() {
            let h = key_hash(k);
            let mut slot = (h as usize) & mask;
            while hash[slot].1 != u32::MAX {
                slot = (slot + 1) & mask;
            }
            hash[slot] = (((h >> 56) as u8) | 1, offs[i]);
        }
        HashFlatFixed {
            blob,
            hash,
            mask,
            restarts,
        }
    }
}

impl Layout for HashFlatFixed {
    fn name(&self) -> &'static str {
        "hash+flatfixed"
    }
    fn logical_bytes(&self) -> usize {
        self.blob.capacity()
            + self.hash.capacity() * std::mem::size_of::<(u8, u32)>()
            + self.restarts.capacity() * 4
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        loop {
            let (t, off) = self.hash[slot];
            if off == u32::MAX {
                return None;
            }
            if t == tag {
                let mut p = off as usize;
                let kl = u16::from_le_bytes(self.blob[p..p + 2].try_into().unwrap()) as usize;
                p += 2;
                if self.blob[p..p + kl] == *key {
                    p += kl;
                    let mut v = [0u32; 4];
                    for slot in v.iter_mut() {
                        *slot = u32::from_le_bytes(self.blob[p..p + 4].try_into().unwrap());
                        p += 4;
                    }
                    return Some(Ext4 {
                        block: v[0],
                        off: v[1],
                        len: v[2],
                        last: v[3],
                    });
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        let r = start / RESTART_EVERY;
        let Some(off) = self.restarts.get(r) else {
            return 0;
        };
        let mut p = *off as usize;
        let mut acc = 0u64;
        let mut seen = 0usize;
        let mut idx = r * RESTART_EVERY;
        while p < self.blob.len() && seen < n {
            let kl = u16::from_le_bytes(self.blob[p..p + 2].try_into().unwrap()) as usize;
            let kb = if kl > 0 { self.blob[p + 2] as u64 } else { 0 };
            p += 2 + kl;
            let b = u32::from_le_bytes(self.blob[p..p + 4].try_into().unwrap()) as u64;
            p += 16;
            if idx >= start {
                acc = acc.wrapping_add(b).wrapping_add(kb);
                seen += 1;
            }
            idx += 1;
        }
        acc
    }
    fn trace_lookup(&self, key: &[u8], t: &mut Trace) -> bool {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        let hbase = self.hash.as_ptr() as usize;
        let bbase = self.blob.as_ptr() as usize;
        loop {
            t.touch(hbase + slot * 8, 8);
            let (tg, off) = self.hash[slot];
            if off == u32::MAX {
                return true;
            }
            if tg == tag {
                let p = off as usize;
                let kl = u16::from_le_bytes(self.blob[p..p + 2].try_into().unwrap()) as usize;
                t.touch(bbase + p, 2 + kl + 16);
                if self.blob[p + 2..p + 2 + kl] == *key {
                    return true;
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
}

// ------------------------------------------------- paged, prefix per page --

/// Records grouped into fixed-count pages, each page storing its keys' common
/// prefix once and a slot directory for O(1) access to any record in it.
///
/// This is the composite the frontier argues for. `packed` is small because it
/// prefix-compresses each key against its predecessor, but that makes a record
/// undecodable without replaying the ones before it -- which is why
/// `hash+packed` misses fast and hits slowly. Sharing the prefix at *page*
/// granularity instead keeps most of the space saving while leaving every
/// record independently decodable, so one structure can serve a point lookup
/// and an ordered scan without either paying for the other.
///
/// The hash stores a rank, not a byte offset, so the page directory can move
/// pages without touching it.
const PER_PAGE: usize = 128;

/// The page-organised record blob, shared by every layout that uses one.
///
/// Split out because `MphPaged` used to embed a whole `HashPaged` and so
/// carried -- and was charged for -- the sixteen bytes per key of hash table
/// it exists to replace. The measured size was 44.7 B/key when the structure
/// actually in use was nearer 26.
struct PagedBlob {
    /// Records per page. Derived from the detected cache line rather than
    /// compiled in, so the same binary is sized correctly on a machine with
    /// 128-byte lines as on one with 64.
    per_page: usize,
    /// Extents written as four fixed 32-bit words rather than four varints.
    ///
    /// `hash+flatfixed` established that varint decoding, not cache misses,
    /// was the whole of the gap between the heap index and the mmap-able one:
    /// cachegrind put `hash+flat` and `hash+flatfixed` within 1.5% of each
    /// other on misses while they were 180ns apart. The paged layouts decode
    /// four varints per hit on exactly the same path, so both arms are kept
    /// here and measured interleaved -- the space this costs is the thing
    /// being traded, and a claim about it has to come from one process.
    fixed: bool,
    pages: Vec<u8>,
    /// Byte offset of each page. Pages are packed end to end, so nothing is
    /// wasted on alignment.
    page_dir: Vec<u32>,
    len: usize,
}

struct HashPaged {
    blob: PagedBlob,
    hash: Vec<(u8, u32)>,
    mask: usize,
    named: &'static str,
}

impl PagedBlob {
    fn build(keys: &[Vec<u8>], exts: &[Ext4]) -> PagedBlob {
        Self::build_with(keys, exts, per_page_setting(), false)
    }

    fn build_fixed(keys: &[Vec<u8>], exts: &[Ext4]) -> PagedBlob {
        Self::build_with(keys, exts, per_page_setting(), true)
    }

    fn build_with(keys: &[Vec<u8>], exts: &[Ext4], per_page: usize, fixed: bool) -> PagedBlob {
        let mut pages: Vec<u8> = Vec::with_capacity(keys.len() * 16);
        let mut page_dir = Vec::with_capacity(keys.len() / PER_PAGE + 1);
        for chunk_start in (0..keys.len()).step_by(per_page) {
            let end = (chunk_start + per_page).min(keys.len());
            let group = &keys[chunk_start..end];
            let base = pages.len();
            page_dir.push(base as u32);
            let prefix = match (group.first(), group.last()) {
                (Some(a), Some(b)) => a.iter().zip(b).take_while(|(x, y)| x == y).count(),
                _ => 0,
            };
            let count = group.len();
            pages.extend_from_slice(&(prefix as u16).to_le_bytes());
            pages.extend_from_slice(&(count as u16).to_le_bytes());
            pages.extend_from_slice(&group[0][..prefix]);
            let slot_base = pages.len();
            pages.resize(slot_base + 2 * count, 0);
            for (j, k) in group.iter().enumerate() {
                let off = (pages.len() - base) as u16;
                pages[slot_base + 2 * j..slot_base + 2 * j + 2].copy_from_slice(&off.to_le_bytes());
                let suffix = &k[prefix..];
                pages.extend_from_slice(&(suffix.len() as u16).to_le_bytes());
                pages.extend_from_slice(suffix);
                let e = exts[chunk_start + j];
                if fixed {
                    for v in [e.block, e.off, e.len, e.last] {
                        pages.extend_from_slice(&v.to_le_bytes());
                    }
                } else {
                    put_uv(&mut pages, e.block as u64);
                    put_uv(&mut pages, e.off as u64);
                    put_uv(&mut pages, e.len as u64);
                    put_uv(&mut pages, e.last as u64);
                }
            }
        }
        pages.shrink_to_fit();
        page_dir.shrink_to_fit();

        PagedBlob {
            per_page,
            fixed,
            pages,
            page_dir,
            len: keys.len(),
        }
    }

    fn bytes(&self) -> usize {
        self.pages.capacity() + self.page_dir.capacity() * 4
    }

    /// (key prefix, slot offset within the page) for a record by rank.
    #[inline]
    fn locate(&self, rank: usize) -> Option<(usize, usize, usize)> {
        let page = rank / self.per_page;
        let slot = rank % self.per_page;
        let base = *self.page_dir.get(page)? as usize;
        let prefix = u16::from_le_bytes(self.pages[base..base + 2].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(self.pages[base + 2..base + 4].try_into().unwrap()) as usize;
        if slot >= count {
            return None;
        }
        let slot_base = base + 4 + prefix;
        let off = u16::from_le_bytes(
            self.pages[slot_base + 2 * slot..slot_base + 2 * slot + 2]
                .try_into()
                .unwrap(),
        ) as usize;
        Some((base, prefix, base + off))
    }

    /// Decode the record at `at`, returning its extent only if the key matches.
    #[inline]
    fn matches(&self, base: usize, prefix: usize, at: usize, key: &[u8]) -> Option<Ext4> {
        let sl = u16::from_le_bytes(self.pages[at..at + 2].try_into().unwrap()) as usize;
        let mut p = at + 2;
        if key.len() != prefix + sl
            || key[..prefix] != self.pages[base + 4..base + 4 + prefix]
            || key[prefix..] != self.pages[p..p + sl]
        {
            return None;
        }
        p += sl;
        if self.fixed {
            let mut v = [0u32; 4];
            for w in v.iter_mut() {
                *w = u32::from_le_bytes(self.pages[p..p + 4].try_into().unwrap());
                p += 4;
            }
            return Some(Ext4 {
                block: v[0],
                off: v[1],
                len: v[2],
                last: v[3],
            });
        }
        Some(Ext4 {
            block: get_uv(&self.pages, &mut p) as u32,
            off: get_uv(&self.pages, &mut p) as u32,
            len: get_uv(&self.pages, &mut p) as u32,
            last: get_uv(&self.pages, &mut p) as u32,
        })
    }

    fn scan(&self, start: usize, n: usize) -> u64 {
        let mut acc = 0u64;
        let mut rank = start;
        let end = (start + n).min(self.len);
        while rank < end {
            let Some((_, _, at)) = self.locate(rank) else {
                break;
            };
            let sl = u16::from_le_bytes(self.pages[at..at + 2].try_into().unwrap()) as usize;
            let kb = if sl > 0 { self.pages[at + 2] as u64 } else { 0 };
            let mut p = at + 2 + sl;
            let first = if self.fixed {
                u32::from_le_bytes(self.pages[p..p + 4].try_into().unwrap()) as u64
            } else {
                get_uv(&self.pages, &mut p)
            };
            acc = acc.wrapping_add(first).wrapping_add(kb);
            rank += 1;
        }
        acc
    }
}

impl HashPaged {
    fn build(keys: &[Vec<u8>], exts: &[Ext4], fixed: bool) -> HashPaged {
        let blob = if fixed {
            PagedBlob::build_fixed(keys, exts)
        } else {
            PagedBlob::build(keys, exts)
        };
        let mut cap = 1usize;
        while cap < keys.len() * 2 {
            cap <<= 1;
        }
        cap = cap.max(16);
        let mask = cap - 1;
        let mut hash = vec![(0u8, u32::MAX); cap];
        for (i, k) in keys.iter().enumerate() {
            let h = key_hash(k);
            let mut slot = (h as usize) & mask;
            while hash[slot].1 != u32::MAX {
                slot = (slot + 1) & mask;
            }
            hash[slot] = (((h >> 56) as u8) | 1, i as u32);
        }
        HashPaged {
            blob,
            hash,
            mask,
            named: if fixed {
                "hash+pagedfixed"
            } else {
                "hash+paged"
            },
        }
    }
}

impl Layout for HashPaged {
    fn name(&self) -> &'static str {
        self.named
    }
    fn logical_bytes(&self) -> usize {
        self.blob.bytes() + self.hash.capacity() * std::mem::size_of::<(u8, u32)>()
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        loop {
            let (t, rank) = self.hash[slot];
            if rank == u32::MAX {
                return None;
            }
            if t == tag {
                if let Some((base, prefix, at)) = self.blob.locate(rank as usize) {
                    if let Some(e) = self.blob.matches(base, prefix, at, key) {
                        return Some(e);
                    }
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn trace_lookup(&self, key: &[u8], t: &mut Trace) -> bool {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.mask;
        let hbase = self.hash.as_ptr() as usize;
        let pbase = self.blob.pages.as_ptr() as usize;
        let dbase = self.blob.page_dir.as_ptr() as usize;
        loop {
            t.touch(hbase + slot * 8, 8);
            let (tg, rank) = self.hash[slot];
            if rank == u32::MAX {
                return true;
            }
            if tg == tag {
                let page = rank as usize / self.blob.per_page;
                t.touch(dbase + page * 4, 4);
                if let Some((base, prefix, at)) = self.blob.locate(rank as usize) {
                    // Page header and shared prefix, the slot directory entry,
                    // then the record itself.
                    t.touch(pbase + base, 4 + prefix);
                    let slot_in = rank as usize % self.blob.per_page;
                    t.touch(pbase + base + 4 + prefix + 2 * slot_in, 2);
                    t.touch(pbase + at, 2 + 32);
                    if self.blob.matches(base, prefix, at, key).is_some() {
                        return true;
                    }
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        self.blob.scan(start, n)
    }
}

// --------------------------------------------- minimal perfect hash (BBHash) --

#[inline]
fn key_hash_seeded(key: &[u8], seed: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 32)
}

/// One level of a BBHash minimal perfect hash: a bit array plus a rank index.
struct MphLevel {
    bits: Vec<u64>,
    /// Set bits before each 512-bit block, so rank is one lookup plus a few
    /// popcounts rather than a scan.
    block_rank: Vec<u32>,
    nbits: usize,
    /// Keys placed by all earlier levels.
    base: u32,
}

impl MphLevel {
    #[inline]
    fn is_set(&self, p: usize) -> bool {
        self.bits[p / 64] & (1u64 << (p % 64)) != 0
    }
    #[inline]
    fn rank(&self, p: usize) -> u32 {
        let block = p / 512;
        let mut r = self.block_rank[block];
        for w in (block * 8)..(p / 64) {
            r += self.bits[w].count_ones();
        }
        r + (self.bits[p / 64] & ((1u64 << (p % 64)) - 1)).count_ones()
    }
}

/// A minimal perfect hash: N distinct keys onto exactly the slots 0..N, with
/// no table of slots and no collisions.
///
/// Built offline from the key set, which is what makes it collision-free by
/// construction. It costs a few bits per key instead of the sixteen bytes a
/// real hash table needs -- but it is static, so it belongs to a base that is
/// rebuilt at merge, and it returns a meaningless slot for any key that was
/// not in the set. That second property is the whole reason it wants a filter
/// beside it: a lookup for an absent key has no cheap way to fail.
struct Mphf {
    levels: Vec<MphLevel>,
    /// Keys that never landed alone, after the level cap. Expected to be tiny.
    fallback: std::collections::HashMap<Vec<u8>, u32>,
    n: usize,
}

impl Mphf {
    fn build(keys: &[Vec<u8>]) -> Mphf {
        const GAMMA: f64 = 2.0;
        const MAX_LEVELS: usize = 12;
        let mut levels: Vec<MphLevel> = Vec::new();
        let mut remaining: Vec<u32> = (0..keys.len() as u32).collect();
        let mut base = 0u32;

        for level in 0..MAX_LEVELS {
            if remaining.is_empty() {
                break;
            }
            let nbits = (((remaining.len() as f64) * GAMMA) as usize)
                .max(64)
                .next_multiple_of(512);
            // Saturating occupancy: 0, 1, or "more than one".
            let mut occ = vec![0u8; nbits];
            for &i in &remaining {
                let p = (key_hash_seeded(&keys[i as usize], level as u64) % nbits as u64) as usize;
                if occ[p] < 2 {
                    occ[p] += 1;
                }
            }
            let mut bits = vec![0u64; nbits / 64];
            for (p, o) in occ.iter().enumerate() {
                if *o == 1 {
                    bits[p / 64] |= 1u64 << (p % 64);
                }
            }
            let mut block_rank = Vec::with_capacity(nbits / 512 + 1);
            let mut running = 0u32;
            for b in 0..(nbits / 512) {
                block_rank.push(running);
                for w in &bits[b * 8..(b + 1) * 8] {
                    running += w.count_ones();
                }
            }
            block_rank.push(running);
            let placed = running;

            let next: Vec<u32> = remaining
                .iter()
                .copied()
                .filter(|&i| {
                    let p =
                        (key_hash_seeded(&keys[i as usize], level as u64) % nbits as u64) as usize;
                    occ[p] != 1
                })
                .collect();
            levels.push(MphLevel {
                bits,
                block_rank,
                nbits,
                base,
            });
            base += placed;
            remaining = next;
        }

        // Anything still unplaced gets an explicit slot after the levels.
        let mut fallback = std::collections::HashMap::new();
        for (j, &i) in remaining.iter().enumerate() {
            fallback.insert(keys[i as usize].clone(), base + j as u32);
        }
        Mphf {
            levels,
            fallback,
            n: keys.len(),
        }
    }

    fn bytes(&self) -> usize {
        self.levels
            .iter()
            .map(|l| l.bits.capacity() * 8 + l.block_rank.capacity() * 4)
            .sum::<usize>()
            + self.fallback.len() * 40
    }

    #[inline]
    fn slot(&self, key: &[u8]) -> Option<u32> {
        for (level, l) in self.levels.iter().enumerate() {
            let p = (key_hash_seeded(key, level as u64) % l.nbits as u64) as usize;
            if l.is_set(p) {
                return Some(l.base + l.rank(p));
            }
        }
        self.fallback.get(key).copied()
    }
}

// ------------------------------------------------------- blocked bloom filter --

/// A blocked Bloom filter: every probe for a key lands in one 512-bit block,
/// so a query costs a single cache miss rather than k scattered ones.
///
/// Chosen over a ribbon filter deliberately. Ribbon is roughly 30% more
/// space-efficient at the same false-positive rate, but the category being
/// bought here is *miss latency*, and on that axis both are one cache line.
/// The extra construction machinery would not change the measurement.
struct BlockedBloom {
    blocks: Vec<u64>,
    nblocks: usize,
}

const BLOOM_BITS_PER_KEY: usize = 12;
const BLOOM_PROBES: usize = 6;

impl BlockedBloom {
    fn build(keys: &[Vec<u8>]) -> BlockedBloom {
        let nblocks = ((keys.len() * BLOOM_BITS_PER_KEY) / 512)
            .max(1)
            .next_power_of_two();
        let mut blocks = vec![0u64; nblocks * 8];
        let f = BlockedBloom {
            blocks: Vec::new(),
            nblocks,
        };
        let mut out = vec![0u64; nblocks * 8];
        for k in keys {
            let h = key_hash_seeded(k, 0xB100);
            let b = f.block_of(h);
            let mut x = h;
            for _ in 0..BLOOM_PROBES {
                x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
                let bit = (x >> 40) as usize % 512;
                out[b * 8 + bit / 64] |= 1u64 << (bit % 64);
            }
        }
        blocks.copy_from_slice(&out);
        BlockedBloom { blocks, nblocks }
    }

    #[inline]
    fn block_of(&self, h: u64) -> usize {
        ((h >> 32) as usize) & (self.nblocks - 1)
    }

    #[inline]
    fn maybe_contains(&self, key: &[u8]) -> bool {
        let h = key_hash_seeded(key, 0xB100);
        let b = self.block_of(h);
        let mut x = h;
        for _ in 0..BLOOM_PROBES {
            x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
            let bit = (x >> 40) as usize % 512;
            if self.blocks[b * 8 + bit / 64] & (1u64 << (bit % 64)) == 0 {
                return false;
            }
        }
        true
    }

    fn bytes(&self) -> usize {
        self.blocks.capacity() * 8
    }
}

// ------------------------------------------------- MPH over the paged blob --

/// The composite the frontier pointed at: a minimal perfect hash instead of a
/// hash table, over the same page-organised records, with an optional filter
/// to give absent keys somewhere cheap to fail.
struct MphPaged {
    blob: PagedBlob,
    mph: Mphf,
    /// MPH slots are arbitrary; the blob is in sorted order. Four bytes per key
    /// buys the translation, and is still a quarter of what the hash cost.
    rank_of_slot: Vec<u32>,
    bloom: Option<BlockedBloom>,
    named: &'static str,
}

impl MphPaged {
    fn build(keys: &[Vec<u8>], exts: &[Ext4], with_bloom: bool, fixed: bool) -> MphPaged {
        let blob = if fixed {
            PagedBlob::build_fixed(keys, exts)
        } else {
            PagedBlob::build(keys, exts)
        };
        let mph = Mphf::build(keys);
        let mut rank_of_slot = vec![u32::MAX; mph.n];
        for (rank, k) in keys.iter().enumerate() {
            let slot = mph.slot(k).expect("every key must have a slot") as usize;
            rank_of_slot[slot] = rank as u32;
        }
        MphPaged {
            blob,
            mph,
            rank_of_slot,
            bloom: if with_bloom {
                Some(BlockedBloom::build(keys))
            } else {
                None
            },
            named: match (with_bloom, fixed) {
                (true, false) => "mph+bloom+paged",
                (true, true) => "mph+bloom+pagedfixed",
                (false, false) => "mph+paged",
                (false, true) => "mph+pagedfixed",
            },
        }
    }
}

impl Layout for MphPaged {
    fn name(&self) -> &'static str {
        self.named
    }
    fn logical_bytes(&self) -> usize {
        self.blob.bytes()
            + self.mph.bytes()
            + self.rank_of_slot.capacity() * 4
            + self.bloom.as_ref().map(|b| b.bytes()).unwrap_or(0)
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        if let Some(b) = &self.bloom {
            if !b.maybe_contains(key) {
                return None;
            }
        }
        let slot = self.mph.slot(key)? as usize;
        let rank = *self.rank_of_slot.get(slot)? as usize;
        let (base, prefix, at) = self.blob.locate(rank)?;
        // The MPH returns a slot for keys it never saw, so the full key must be
        // verified. A fingerprint would not do: this project has already argued
        // that a 32-bit match is not proof at six million keys.
        self.blob.matches(base, prefix, at, key)
    }
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        self.blob.scan(start, n)
    }
}

// ----------------------------------------------------------------- btree --

/// A bulk-loaded, page-based B+tree in one flat buffer.
///
/// Separators in branch pages are full keys, as LMDB's are: routing on a
/// truncated prefix would send two keys that share eight bytes to different
/// leaves and silently lose one. Loaded at 100% fill, which flatters it --
/// a B+tree that has been mutated sits nearer 65-70%, so its real bytes per
/// key are worse than measured here.
struct BTree {
    pages: Vec<u8>,
    page_size: usize,
    root: usize,
    /// Levels from root to leaf; reported so a page-size change that alters
    /// the tree's shape is visible rather than inferred from timings.
    height: usize,
    /// Leaves are emitted before branches, so an ordered scan is a walk of
    /// [0, leaf_end) rather than a traversal.
    leaf_end: usize,
}

impl BTree {
    /// Bulk-load. Each page carries a slot directory of u16 entry offsets at
    /// its tail, so a lookup binary-searches within the page instead of
    /// walking it. A linear walk -- which an earlier version of this did --
    /// scans about 170 entries per 4 KiB leaf and measures the harness, not
    /// the structure.
    ///
    /// Page: [u8 kind][u16 count] entries... then, at the page tail,
    /// `count` u16 offsets relative to the page base.
    fn build(keys: &[Vec<u8>], exts: &[Ext4], page_size: usize) -> BTree {
        let mut pages: Vec<u8> = Vec::new();

        let finish = |pages: &mut Vec<u8>, base: usize, slots: &[u16], count: u16| {
            pages[base + 1..base + 3].copy_from_slice(&count.to_le_bytes());
            // Pad up to where the directory starts, then write it.
            let dir = base + page_size - 2 * slots.len();
            pages.resize(dir, 0);
            for off in slots {
                pages.extend_from_slice(&off.to_le_bytes());
            }
            debug_assert_eq!(pages.len() - base, page_size);
        };

        let mut leaf_seps: Vec<(Vec<u8>, u32)> = Vec::new();
        let mut i = 0usize;
        while i < keys.len() {
            let base = pages.len();
            pages.push(0);
            pages.extend_from_slice(&0u16.to_le_bytes());
            let mut slots: Vec<u16> = Vec::new();
            let first = keys[i].clone();
            while i < keys.len() {
                let need = 2 + keys[i].len() + 16;
                // body so far + this entry + directory including this slot
                if (pages.len() - base) + need + 2 * (slots.len() + 1) > page_size
                    && !slots.is_empty()
                {
                    break;
                }
                slots.push((pages.len() - base) as u16);
                pages.extend_from_slice(&(keys[i].len() as u16).to_le_bytes());
                pages.extend_from_slice(&keys[i]);
                let e = exts[i];
                for v in [e.block, e.off, e.len, e.last] {
                    pages.extend_from_slice(&v.to_le_bytes());
                }
                i += 1;
            }
            let count = slots.len() as u16;
            finish(&mut pages, base, &slots, count);
            leaf_seps.push((first, base as u32));
        }

        let leaf_end = pages.len();
        let mut level = leaf_seps;
        let mut height = 1usize;
        let mut root = level[0].1 as usize;
        while level.len() > 1 {
            let mut up: Vec<(Vec<u8>, u32)> = Vec::new();
            let mut j = 0usize;
            while j < level.len() {
                let base = pages.len();
                pages.push(1);
                pages.extend_from_slice(&0u16.to_le_bytes());
                let mut slots: Vec<u16> = Vec::new();
                let first = level[j].0.clone();
                while j < level.len() {
                    let need = 2 + level[j].0.len() + 4;
                    if (pages.len() - base) + need + 2 * (slots.len() + 1) > page_size
                        && !slots.is_empty()
                    {
                        break;
                    }
                    slots.push((pages.len() - base) as u16);
                    pages.extend_from_slice(&(level[j].0.len() as u16).to_le_bytes());
                    pages.extend_from_slice(&level[j].0);
                    pages.extend_from_slice(&level[j].1.to_le_bytes());
                    j += 1;
                }
                let count = slots.len() as u16;
                finish(&mut pages, base, &slots, count);
                up.push((first, base as u32));
            }
            level = up;
            height += 1;
            root = level[0].1 as usize;
        }
        pages.shrink_to_fit();
        BTree {
            pages,
            page_size,
            root,
            height,
            leaf_end,
        }
    }

    #[inline]
    fn slot(&self, base: usize, count: usize, i: usize) -> usize {
        let dir = base + self.page_size - 2 * count;
        base + u16::from_le_bytes(self.pages[dir + 2 * i..dir + 2 * i + 2].try_into().unwrap())
            as usize
    }

    #[inline]
    fn key_at(&self, at: usize) -> &[u8] {
        let kl = u16::from_le_bytes(self.pages[at..at + 2].try_into().unwrap()) as usize;
        &self.pages[at + 2..at + 2 + kl]
    }
}

impl Layout for BTree {
    fn name(&self) -> &'static str {
        "btree"
    }
    fn logical_bytes(&self) -> usize {
        self.pages.capacity()
    }
    fn lookup(&self, key: &[u8]) -> Option<Ext4> {
        let mut at = self.root;
        loop {
            let kind = self.pages[at];
            let count = u16::from_le_bytes(self.pages[at + 1..at + 3].try_into().unwrap()) as usize;
            if count == 0 {
                return None;
            }
            if kind == 1 {
                // Last separator <= key.
                let (mut lo, mut hi) = (0usize, count);
                let mut chosen: Option<usize> = None;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let e = self.slot(at, count, mid);
                    if self.key_at(e) <= key {
                        chosen = Some(mid);
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let m = chosen?;
                let e = self.slot(at, count, m);
                let kl = u16::from_le_bytes(self.pages[e..e + 2].try_into().unwrap()) as usize;
                let cp = e + 2 + kl;
                at = u32::from_le_bytes(self.pages[cp..cp + 4].try_into().unwrap()) as usize;
            } else {
                let (mut lo, mut hi) = (0usize, count);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let e = self.slot(at, count, mid);
                    match self.key_at(e).cmp(key) {
                        std::cmp::Ordering::Less => lo = mid + 1,
                        std::cmp::Ordering::Greater => hi = mid,
                        std::cmp::Ordering::Equal => {
                            let kl = u16::from_le_bytes(self.pages[e..e + 2].try_into().unwrap())
                                as usize;
                            let mut p = e + 2 + kl;
                            let mut v = [0u32; 4];
                            for slot in v.iter_mut() {
                                *slot =
                                    u32::from_le_bytes(self.pages[p..p + 4].try_into().unwrap());
                                p += 4;
                            }
                            return Some(Ext4 {
                                block: v[0],
                                off: v[1],
                                len: v[2],
                                last: v[3],
                            });
                        }
                    }
                }
                return None;
            }
        }
    }
    /// NOTE: this walks leaves from the beginning rather than descending to
    /// `start`, so its cost includes an O(start) prefix walk. That is a
    /// property of this harness, not of B+trees -- a real implementation seeks
    /// to the leaf first. The figure is reported for completeness and must not
    /// be read as a B+tree scan rate. The tree is dominated on the other two
    /// axes regardless, which is why this was not worth fixing.
    fn scan_from(&self, start: usize, n: usize) -> u64 {
        let mut acc = 0u64;
        let (mut seen, mut idx) = (0usize, 0usize);
        let mut base = 0usize;
        while base < self.leaf_end && seen < n {
            let count =
                u16::from_le_bytes(self.pages[base + 1..base + 3].try_into().unwrap()) as usize;
            for i in 0..count {
                if seen >= n {
                    break;
                }
                if idx >= start {
                    let e = self.slot(base, count, i);
                    let kl = u16::from_le_bytes(self.pages[e..e + 2].try_into().unwrap()) as usize;
                    let p = e + 2 + kl;
                    acc = acc.wrapping_add(u32::from_le_bytes(
                        self.pages[p..p + 4].try_into().unwrap(),
                    ) as u64);
                    seen += 1;
                }
                idx += 1;
            }
            base += self.page_size;
        }
        acc
    }
}

// ------------------------------------------------------------------ main --

struct Args(Vec<String>);
impl Args {
    fn get(&self, n: &str) -> Option<&str> {
        self.0
            .iter()
            .position(|a| a == n)
            .and_then(|i| self.0.get(i + 1))
            .map(|s| s.as_str())
    }
    fn num(&self, n: &str, d: usize) -> usize {
        self.get(n).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

/// Records per page: derived from the machine unless overridden, which is how
/// the sweep searches the parameter the derivation is meant to predict.
fn per_page_setting() -> usize {
    std::env::var("SUPDB_PER_PAGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| supdb::bench::Machine::detect().records_per_page())
}

fn build_layout(which: &str, keys: &[Vec<u8>], exts: &[Ext4]) -> Box<dyn Layout> {
    match which {
        "heap-hash" => Box::new(HeapHash::build(keys, exts)),
        "btree" => Box::new(BTree::build(keys, exts, 4096)),
        "packed" => Box::new(Packed::build(keys, exts, 0)),
        "packed+radix" => Box::new(Packed::build(keys, exts, 18)),
        "hash+packed" => Box::new(HashPacked::build(keys, exts)),
        "hash+flat" => Box::new(HashFlat::build(keys, exts)),
        "hash+flatfixed" => Box::new(HashFlatFixed::build(keys, exts)),
        "hash+paged" => Box::new(HashPaged::build(keys, exts, false)),
        "hash+pagedfixed" => Box::new(HashPaged::build(keys, exts, true)),
        "mph+paged" => Box::new(MphPaged::build(keys, exts, false, false)),
        "mph+pagedfixed" => Box::new(MphPaged::build(keys, exts, false, true)),
        "mph+bloom+paged" => Box::new(MphPaged::build(keys, exts, true, false)),
        other => panic!("unknown layout {other}"),
    }
}

const LAYOUTS: [&str; 12] = [
    "heap-hash",
    "btree",
    "packed",
    "packed+radix",
    "hash+packed",
    "hash+flat",
    "hash+flatfixed",
    "hash+paged",
    "hash+pagedfixed",
    "mph+paged",
    "mph+pagedfixed",
    "mph+bloom+paged",
];

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let args = Args(argv.clone());
    if argv.get(1).map(|s| s.as_str()) == Some("child") {
        return child(&args);
    }
    if argv.get(1).map(|s| s.as_str()) == Some("trace") {
        return trace_mode(&args);
    }
    if argv.get(1).map(|s| s.as_str()) == Some("probe") {
        return probe_mode(&args);
    }
    if argv.get(1).map(|s| s.as_str()) == Some("sweep") {
        return sweep_mode(&args);
    }
    if argv.get(1).map(|s| s.as_str()) == Some("machine") {
        return machine_mode();
    }
    if argv.get(1).map(|s| s.as_str()) == Some("pair") {
        return pair_mode(&args);
    }
    let profile = Profile::parse(args.get("--profile").unwrap_or("dev")).unwrap_or(Profile::Dev);
    let out = PathBuf::from(args.get("--out").unwrap_or("results"));
    let scales: Vec<usize> = match profile {
        Profile::Ci => vec![100_000],
        Profile::Dev => vec![100_000, 1_000_000],
        Profile::Full => vec![100_000, 1_000_000, 10_000_000],
    };
    let shapes: Vec<Shape> = match args.get("--shape") {
        Some(s) => vec![Shape::parse(s).expect("shape")],
        None => vec![Shape::Decimal16, Shape::RandomHex, Shape::Clustered],
    };
    let lookups = args.num("--lookups", profile.pick(200_000, 500_000, 2_000_000)) as u64;

    let mut rec = Record::new("f9-index-layout", profile);
    rec.param(
        "key_counts",
        J::arr(scales.iter().map(|s| J::u(*s as u64)).collect()),
    )
    .param(
        "shapes",
        J::arr(shapes.iter().map(|s| J::s(s.as_str())).collect()),
    )
    .param(
        "layouts",
        J::arr(LAYOUTS.iter().map(|l| J::s(*l)).collect()),
    )
    .param("lookups_per_measurement", J::u(lookups))
    .param("restart_every", J::u(RESTART_EVERY as u64))
    .param("btree_page_size", J::u(4096))
    .param("radix_bits", J::u(18));

    let exe = std::env::current_exe().expect("exe");
    let mut rows = Vec::new();
    // Held for the significance gate: the comparison that decides the design.
    let mut hash_samples = std::collections::HashMap::new();
    let mut packed_samples = std::collections::HashMap::new();

    for &shape in &shapes {
        for &n in &scales {
            let keys = make_keys(shape, n);
            let exts = make_exts(keys.len());
            eprintln!("# {} x {} keys", shape.as_str(), keys.len());

            // Correctness before speed. A layout that is fast and wrong is not
            // a candidate, and a lookup benchmark that silently misses every
            // key is very fast indeed.
            for which in LAYOUTS {
                let l = build_layout(which, &keys, &exts);
                for i in (0..keys.len()).step_by((keys.len() / 997).max(1)) {
                    assert_eq!(
                        l.lookup(&keys[i]),
                        Some(exts[i]),
                        "{which}/{}: wrong value for key {i}",
                        shape.as_str()
                    );
                }
                let mut absent = keys[keys.len() / 2].clone();
                *absent.last_mut().unwrap() = b'~';
                assert_eq!(l.lookup(&absent), None, "{which}: found an absent key");
            }

            for which in LAYOUTS {
                let l = build_layout(which, &keys, &exts);
                assert_eq!(l.name(), which, "layout reported the wrong name");
                let logical = l.logical_bytes();
                let height = if which == "btree" {
                    BTree::build(&keys, &exts, 4096).height as u64
                } else {
                    0
                };

                let t = Instant::now();
                let _ = build_layout(which, &keys, &exts);
                let build_ms = t.elapsed().as_secs_f64() * 1000.0;

                // Hits and misses measured separately: a miss short-circuits on
                // an empty hash slot but must walk a whole page in a sorted
                // structure, so averaging them hides the difference.
                let trial = Trial::new(profile.reps());
                let s = trial.run(2, |ci, _| {
                    let mut r = Rng::new(0xF9);
                    let t = Instant::now();
                    let mut found = 0u64;
                    for _ in 0..lookups {
                        let i = (r.next() as usize) % keys.len();
                        let hit = if ci == 0 {
                            l.lookup(&keys[i])
                        } else {
                            let mut k = keys[i].clone();
                            *k.last_mut().unwrap() = b'~';
                            l.lookup(&k)
                        };
                        if std::hint::black_box(hit).is_some() {
                            found += 1;
                        }
                    }
                    std::hint::black_box(found);
                    t.elapsed().as_secs_f64() * 1e9 / lookups as f64
                });

                // Ordered scan: the other category. A layout that wins point
                // lookups by abandoning order has not won anything, and the
                // engine's `scan` is a first-class API.
                let scan_len = 1000usize.min(keys.len());
                let scan = Trial::new(profile.reps().min(5)).run(1, |_, _| {
                    let mut r = Rng::new(0x5CA5);
                    let rounds = 200usize;
                    let t = Instant::now();
                    let mut acc = 0u64;
                    for _ in 0..rounds {
                        let from = (r.next() as usize) % (keys.len() - scan_len).max(1);
                        acc = acc.wrapping_add(l.scan_from(from, scan_len));
                    }
                    std::hint::black_box(acc);
                    t.elapsed().as_secs_f64() * 1e9 / (rounds * scan_len) as f64
                });
                let scan_ns = scan[0].median();

                // Resident size, measured in a child so the allocator's
                // overhead is counted rather than estimated.
                let o = std::process::Command::new(&exe)
                    .args([
                        "child",
                        "--layout",
                        which,
                        "--keys",
                        &keys.len().to_string(),
                        "--shape",
                        shape.as_str(),
                    ])
                    .output()?;
                let txt = String::from_utf8_lossy(&o.stdout).to_string();
                let rss: f64 = txt
                    .split("rss_bytes=")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);

                let hit_ns = s[0].median();
                let miss_ns = s[1].median();
                if which == "heap-hash" {
                    hash_samples.insert((shape.as_str(), n), s[0].clone());
                }
                if which == "hash+flat" {
                    packed_samples.insert((shape.as_str(), n), s[0].clone());
                }
                println!(
                    "  {:<13} {:>10} keys  hit {:>7.1} ns  miss {:>7.1} ns  scan {:>6.2} ns/e  {:>6.1} B/key",
                    which,
                    keys.len(),
                    hit_ns,
                    miss_ns,
                    scan_ns,
                    rss / keys.len() as f64
                );
                rows.push(jobj! {
                    "shape" => J::s(shape.as_str()),
                    "keys" => J::u(keys.len() as u64),
                    "layout" => J::s(which),
                    "hit_ns" => J::fp(hit_ns, 2),
                    "miss_ns" => J::fp(miss_ns, 2),
                    "logical_bytes_per_key" => J::fp(logical as f64 / keys.len() as f64, 2),
                    "resident_bytes_per_key" => J::fp(rss / keys.len() as f64, 2),
                    "scan_ns_per_entry" => J::fp(scan_ns, 3),
                    "build_ms" => J::fp(build_ms, 2),
                    "btree_height" => J::u(height),
                    "hit_samples" => s[0].to_json(),
                });
            }
        }
    }
    rec.series("measurements", J::arr(rows.clone()));

    // The claim the whole exercise exists to test.
    let biggest = scales.last().copied().unwrap_or(0);
    for &shape in &shapes {
        if let (Some(h), Some(p)) = (
            hash_samples.get(&(shape.as_str(), biggest)),
            packed_samples.get(&(shape.as_str(), biggest)),
        ) {
            rec.compare(
                &format!("hash_flat_vs_heap_hash_{}", shape.as_str()),
                compare(p, h, supdb::bench::MIN_EFFECT),
            );
        }
    }
    let get = |shape: &str, layout: &str, field: &str| -> Option<f64> {
        rows.iter()
            .find(|r| {
                r.path("shape").and_then(|v| v.as_str()) == Some(shape)
                    && r.path("layout").and_then(|v| v.as_str()) == Some(layout)
                    && r.path("keys").and_then(|v| v.as_u64()) == Some(biggest as u64)
            })
            .and_then(|r| r.num(field))
    };

    // These four were written before the first run and two of them tested the
    // wrong layout: they were calibrated for `packed`, which turned out not to
    // be the answer. Revised to ask what the frontier actually poses.
    if let (Some(hb), Some(fb)) = (
        get("decimal16", "heap-hash", "resident_bytes_per_key"),
        get("decimal16", "hash+flat", "resident_bytes_per_key"),
    ) {
        rec.finding(Finding::new(
            "F9.1",
            "an mmap-able layout exists that is at least 1.5x smaller than the current index",
            fb * 1.5 <= hb,
            format!("hash+flat {fb:.0} B/key against the current {hb:.0} B/key ({:.2}x smaller), and shared between processes rather than duplicated", hb / fb.max(1e-9)),
        ));
    }
    if let (Some(hl), Some(fl)) = (
        get("decimal16", "heap-hash", "hit_ns"),
        get("decimal16", "hash+flat", "hit_ns"),
    ) {
        rec.finding(Finding::new(
            "F9.2",
            "that layout looks up within 1.5x of the current heap hash",
            fl <= hl * 1.5,
            format!("hash+flat {fl:.0} ns against heap-hash {hl:.0} ns ({:.2}x). In the engine's read path the index is about a fifth of a point read, so this is roughly +5% end to end", fl / hl.max(1e-9)),
        ));
    }
    // The question the design review actually turned on.
    if let (Some(bl), Some(bb), Some(pl), Some(pb)) = (
        get("decimal16", "btree", "hit_ns"),
        get("decimal16", "btree", "resident_bytes_per_key"),
        get("decimal16", "packed", "hit_ns"),
        get("decimal16", "packed", "resident_bytes_per_key"),
    ) {
        let dominated = pl < bl && pb < bb;
        rec.finding(Finding::new(
            "F9.3",
            "a bulk-loaded B+tree is on the speed/space frontier",
            !dominated,
            format!("B+tree {bl:.0} ns / {bb:.0} B/key against a plain packed array at {pl:.0} ns / {pb:.0} B/key: the array is {} on both axes, so the tree is dominated. Loaded at 100% fill, which flatters it -- a mutated tree sits nearer 65-70%", if dominated { "better" } else { "not better" }),
        ));
    }
    if let (Some(hs), Some(ps)) = (
        get("decimal16", "heap-hash", "scan_ns_per_entry"),
        get("decimal16", "hash+paged", "scan_ns_per_entry"),
    ) {
        rec.finding(Finding::new(
            "F9.5",
            "the composite layout scans in order at least as fast as the current index",
            ps <= hs,
            format!(
                "hash+paged {ps:.2} ns/entry against heap-hash {hs:.2} ns/entry ({:.2}x)",
                ps / hs.max(1e-9)
            ),
        ));
    }
    // What the filter is actually for.
    if let (Some(bare), Some(filtered), Some(bs), Some(fs)) = (
        get("decimal16", "mph+paged", "miss_ns"),
        get("decimal16", "mph+bloom+paged", "miss_ns"),
        get("decimal16", "mph+paged", "resident_bytes_per_key"),
        get("decimal16", "mph+bloom+paged", "resident_bytes_per_key"),
    ) {
        rec.finding(Finding::new(
            "F9.6",
            "a blocked Bloom filter at least halves the cost of an absent-key lookup",
            filtered * 2.0 <= bare,
            format!("mph+paged {bare:.0} ns -> mph+bloom+paged {filtered:.0} ns ({:.2}x) for {:.1} B/key. A minimal perfect hash returns a slot for keys it never saw, so without a filter every miss pays a full record read to discover it was a miss", bare / filtered.max(1e-9), fs - bs),
        ));
    }
    // Whether the minimal perfect hash bought speed as well as space.
    if let (Some(hl), Some(ml), Some(hb), Some(mb)) = (
        get("decimal16", "heap-hash", "hit_ns"),
        get("decimal16", "mph+paged", "hit_ns"),
        get("decimal16", "heap-hash", "resident_bytes_per_key"),
        get("decimal16", "mph+paged", "resident_bytes_per_key"),
    ) {
        rec.finding(Finding::new(
            "F9.7",
            "a minimal perfect hash reaches packed-class space at hash-class speed",
            ml <= hl * 1.5,
            format!("mph+paged {mb:.0} B/key against heap-hash {hb:.0} ({:.1}x smaller) but {ml:.0} ns against {hl:.0} ({:.2}x slower). BBHash probes several level bit-arrays, each a random access into megabytes, plus a rank; that is more cache misses than one hash probe, not fewer", hb / mb.max(1e-9), ml / hl.max(1e-9)),
        ));
    }
    if let (Some(smooth), Some(rough)) = (
        get("decimal16", "packed+radix", "hit_ns"),
        get("clustered", "packed+radix", "hit_ns"),
    ) {
        rec.finding(Finding::new(
            "F9.4",
            "the radix table degrades gracefully on a clustered key distribution",
            rough <= smooth * 2.0,
            format!("clustered {rough:.0} ns against smooth {smooth:.0} ns ({:.2}x). Indexing by key value rather than by comparison is only as good as its assumption about the distribution, and the radix layer collapsed entirely on decimal keys until the shared prefix was stripped", rough / smooth.max(1e-9)),
        ));
    }
    rec.note(
        "The B+tree's scan figure includes an O(start) prefix walk because this harness does not \
         seek to the starting leaf. It is a harness artifact, not a property of B+trees, and is \
         reported only so the column is not silently blank",
    );
    rec.note(
        "This measures a proposed replacement, not the shipped engine. Layouts are built in \
         memory rather than mapped from a file, so the figures are an upper bound on what an \
         mmap-backed version achieves on a warm cache and say nothing about cold behaviour",
    );
    rec.print_summary();
    rec.write(&out)?;
    Ok(())
}

/// Build one layout and report resident bytes, so the allocator's overhead is
/// measured rather than estimated.
fn child(args: &Args) -> std::io::Result<()> {
    let which = args.get("--layout").expect("--layout");
    let n = args.num("--keys", 1000);
    let shape = Shape::parse(args.get("--shape").unwrap_or("decimal16")).expect("shape");
    let keys = make_keys(shape, n);
    let exts = make_exts(keys.len());
    // The inputs are already resident when `before` is taken, so the delta is
    // the structure. Subtracting them a second time -- which an earlier version
    // did -- drove the compact layouts to a reported zero bytes per key.
    let before = env::rss_bytes();
    let l = build_layout(which, &keys, &exts);
    let after = env::rss_bytes();
    std::hint::black_box(l.lookup(&keys[keys.len() / 2]));
    println!(
        "rss_bytes={} before={before} after={after}",
        after.saturating_sub(before)
    );
    Ok(())
}

/// Count the distinct cache lines and pages a lookup actually touches.
///
/// The machine is a Firecracker guest with no PMU -- `perf` reports every
/// hardware counter as `<not supported>` -- so the cache-miss model cannot be
/// checked against measured misses. It can still be checked against its
/// inputs: instrument the lookups, count distinct 64-byte lines and 4 KiB
/// pages, and see whether the layout that touches fewer is the one that runs
/// faster. When it is not, the model is missing a term.
fn trace_mode(args: &Args) -> std::io::Result<()> {
    let n = args.num("--keys", 10_000_000);
    let samples = args.num("--samples", 20_000);
    let shape = Shape::parse(args.get("--shape").unwrap_or("decimal16")).expect("shape");
    let keys = make_keys(shape, n);
    let exts = make_exts(keys.len());
    let traced = ["heap-hash", "hash+flat", "hash+flatfixed", "hash+paged"];

    println!(
        "# {} x {} keys, {} sampled lookups\n{:<13} {:>8} {:>9} {:>9} {:>9}",
        shape.as_str(),
        keys.len(),
        samples,
        "layout",
        "reads",
        "lines",
        "pages",
        "bytes/key"
    );
    for which in traced {
        let l = build_layout(which, &keys, &exts);
        let mut rng = Rng::new(0x71ACE);
        let (mut lines, mut pages, mut reads) = (0usize, 0usize, 0usize);
        for _ in 0..samples {
            let k = &keys[(rng.next() as usize) % keys.len()];
            let mut t = Trace::default();
            assert!(l.trace_lookup(k, &mut t), "{which} has no tracer");
            lines += t.lines.len();
            pages += t.pages.len();
            reads += t.reads;
        }
        let s = samples as f64;
        println!(
            "{:<13} {:>8.2} {:>9.2} {:>9.2} {:>9.1}",
            which,
            reads as f64 / s,
            lines as f64 / s,
            pages as f64 / s,
            l.logical_bytes() as f64 / keys.len() as f64
        );
    }
    println!(
        "\n# lines and pages are distinct 64-byte lines and 4 KiB pages touched per lookup.\n\
         # A page count above one means the lookup is exposed to TLB reach: an L2 TLB of\n\
         # ~1536 entries covers 6 MiB with 4 KiB pages, and these structures are hundreds."
    );
    Ok(())
}

/// Build one layout, perform a fixed number of lookups, exit.
///
/// Exists so an external simulator can measure a single layout in isolation.
/// Cachegrind reports misses for the whole process, so the process has to do
/// one thing. The build phase is unavoidably included; `--lookups 0` measures
/// the build alone, and subtracting gives the lookup cost.
fn probe_mode(args: &Args) -> std::io::Result<()> {
    let which = args.get("--layout").expect("--layout");
    let n = args.num("--keys", 1_000_000);
    let lookups = args.num("--lookups", 50_000);
    let shape = Shape::parse(args.get("--shape").unwrap_or("decimal16")).expect("shape");
    let keys = make_keys(shape, n);
    let exts = make_exts(keys.len());
    let l = build_layout(which, &keys, &exts);
    let mut rng = Rng::new(0x9803);
    let mut found = 0u64;
    for _ in 0..lookups {
        let k = &keys[(rng.next() as usize) % keys.len()];
        if std::hint::black_box(l.lookup(k)).is_some() {
            found += 1;
        }
    }
    eprintln!("# {which} {n} keys {lookups} lookups, {found} found");
    Ok(())
}

/// Search the records-per-page parameter, and check the derived value against
/// the empirical optimum.
///
/// This is the test of whether one implementation can be near-optimal
/// everywhere. `Machine::records_per_page` derives the constant from the cache
/// line size with a mechanism behind it; the sweep finds what is actually
/// best. If the derivation lands close to the optimum on every machine shape,
/// the unified approach holds and no per-architecture tuning table is needed.
/// If it does not, the gap says how much complexity is actually being bought.
/// Compare two layouts interleaved in one process.
///
/// The probe measures each layout under its own `Trial`, one after another.
/// That is enough for a table and not enough for a claim: CLAUDE.md requires
/// two arms of a change to be run interleaved, as `f8-checksums` does for
/// `Options::checksums`, precisely because sequential blocks let the machine
/// drift between them. The drift within one process over a few seconds is far
/// smaller than the drift between runs that once moved three unchanged
/// comparators by +20% to +43% -- but "far smaller" is not "measured", and a
/// difference is not a difference here until it clears `stats::compare`.
///
/// So this mode builds both layouts up front and round-robins the
/// configurations through a single `Trial`, which is the only shape that
/// licenses a comparison between them.
fn pair_mode(args: &Args) -> std::io::Result<()> {
    let profile = Profile::parse(args.get("--profile").unwrap_or("dev")).unwrap_or(Profile::Dev);
    let out = PathBuf::from(args.get("--out").unwrap_or("results"));
    let a_name = args.get("--a").expect("--a <layout>");
    let b_name = args.get("--b").expect("--b <layout>");
    let n = args.num("--keys", profile.pick(100_000, 1_000_000, 10_000_000));
    let lookups = args.num("--lookups", profile.pick(200_000, 500_000, 2_000_000)) as u64;
    let shape = Shape::parse(args.get("--shape").unwrap_or("decimal16")).expect("shape");

    let keys = make_keys(shape, n);
    let exts = make_exts(keys.len());
    let arms = [
        build_layout(a_name, &keys, &exts),
        build_layout(b_name, &keys, &exts),
    ];

    // Correctness first. An arm that is fast and wrong is not an arm.
    for (which, l) in [a_name, b_name].iter().zip(arms.iter()) {
        for i in (0..keys.len()).step_by((keys.len() / 997).max(1)) {
            assert_eq!(l.lookup(&keys[i]), Some(exts[i]), "{which}: wrong value");
        }
    }

    // Four configurations round-robined: {a,b} x {hit,miss}. The Trial
    // interleaves them, so a thermal or frequency excursion lands on both arms
    // rather than on whichever happened to run during it.
    let trial = Trial::new(profile.reps());
    let s = trial.run(4, |ci, _| {
        let l = &arms[ci / 2];
        let miss = ci % 2 == 1;
        let mut r = Rng::new(0xF9);
        let t = Instant::now();
        let mut found = 0u64;
        for _ in 0..lookups {
            let i = (r.next() as usize) % keys.len();
            let hit = if miss {
                let mut k = keys[i].clone();
                *k.last_mut().unwrap() = b'~';
                l.lookup(&k)
            } else {
                l.lookup(&keys[i])
            };
            if std::hint::black_box(hit).is_some() {
                found += 1;
            }
        }
        std::hint::black_box(found);
        t.elapsed().as_secs_f64() * 1e9 / lookups as f64
    });

    let scan_len = 1000usize.min(keys.len());
    let scan = Trial::new(profile.reps().min(5)).run(2, |ci, _| {
        let l = &arms[ci];
        let mut r = Rng::new(0x5CA5);
        let rounds = 200usize;
        let t = Instant::now();
        let mut acc = 0u64;
        for _ in 0..rounds {
            let from = (r.next() as usize) % (keys.len() - scan_len).max(1);
            acc = acc.wrapping_add(l.scan_from(from, scan_len));
        }
        std::hint::black_box(acc);
        t.elapsed().as_secs_f64() * 1e9 / (rounds * scan_len) as f64
    });

    let a_bytes = arms[0].logical_bytes() as f64 / keys.len() as f64;
    let b_bytes = arms[1].logical_bytes() as f64 / keys.len() as f64;

    // Named for the arms, not for the mode: two pairs written to the same
    // directory would otherwise collide on one filename, and results/ is the
    // source of truth rather than a scratch area.
    let slug = format!(
        "f10-pair-{}-vs-{}",
        a_name.replace('+', "-"),
        b_name.replace('+', "-")
    );
    let mut rec = Record::new(&slug, profile);
    rec.param("a", J::s(a_name))
        .param("b", J::s(b_name))
        .param("keys", J::u(keys.len() as u64))
        .param("shape", J::s(shape.as_str()))
        .param("lookups", J::u(lookups));
    let hit_cmp = compare(&s[0], &s[2], supdb::bench::MIN_EFFECT);
    let miss_cmp = compare(&s[1], &s[3], supdb::bench::MIN_EFFECT);
    let scan_cmp = compare(&scan[0], &scan[1], supdb::bench::MIN_EFFECT);
    let (hit_v, hit_r) = (hit_cmp.verdict, hit_cmp.ratio);
    let (miss_v, miss_r) = (miss_cmp.verdict, miss_cmp.ratio);
    let (scan_v, scan_r) = (scan_cmp.verdict, scan_cmp.ratio);
    rec.compare("b_hit_vs_a_hit", hit_cmp);
    rec.compare("b_miss_vs_a_miss", miss_cmp);
    rec.compare("b_scan_vs_a_scan", scan_cmp);

    // Three statements rather than three numbers, so `verify` has something to
    // hold the next run against.
    rec.finding(Finding::new(
        "P1",
        "the b arm's point lookup is faster by a margin that clears the gate",
        hit_v == Verdict::Greater,
        format!(
            "{a_name} {:.0} ns -> {b_name} {:.0} ns ({hit_r:.3}x), verdict {hit_v:?}",
            s[0].median(),
            s[2].median()
        ),
    ));
    rec.finding(Finding::new(
        "P2",
        "the b arm's ordered scan is faster by a margin that clears the gate",
        scan_v == Verdict::Greater,
        format!(
            "{a_name} {:.2} ns/entry -> {b_name} {:.2} ns/entry ({scan_r:.3}x), verdict {scan_v:?}",
            scan[0].median(),
            scan[1].median()
        ),
    ));
    // The mechanism check, and the one most likely to catch a harness error.
    // A miss fails at the key comparison and never reaches the extent, so an
    // encoding change sitting behind that comparison must not move it. If this
    // ever reports a difference, the benchmark is measuring something other
    // than what it says it is.
    rec.finding(Finding::new(
        "P3",
        "the b arm's absent-key lookup is unchanged, the encoding sitting past the key comparison",
        miss_v == Verdict::NoDifference,
        format!(
            "{a_name} {:.0} ns vs {b_name} {:.0} ns ({miss_r:.3}x), verdict {miss_v:?}",
            s[1].median(),
            s[3].median()
        ),
    ));
    rec.series(
        "arms",
        J::arr(vec![
            jobj! {
                "layout" => J::s(a_name),
                "hit_ns" => J::fp(s[0].median(), 2),
                "miss_ns" => J::fp(s[1].median(), 2),
                "scan_ns_per_entry" => J::fp(scan[0].median(), 3),
                "logical_bytes_per_key" => J::fp(a_bytes, 2),
                "hit_samples" => s[0].to_json(),
            },
            jobj! {
                "layout" => J::s(b_name),
                "hit_ns" => J::fp(s[2].median(), 2),
                "miss_ns" => J::fp(s[3].median(), 2),
                "scan_ns_per_entry" => J::fp(scan[1].median(), 3),
                "logical_bytes_per_key" => J::fp(b_bytes, 2),
                "hit_samples" => s[2].to_json(),
            },
        ]),
    );

    println!(
        "# {} vs {} -- {} x {} keys [{}]",
        a_name,
        b_name,
        shape.as_str(),
        keys.len(),
        profile.as_str()
    );
    for (name, hit, miss, sc, by) in [
        (
            a_name,
            s[0].median(),
            s[1].median(),
            scan[0].median(),
            a_bytes,
        ),
        (
            b_name,
            s[2].median(),
            s[3].median(),
            scan[1].median(),
            b_bytes,
        ),
    ] {
        println!(
            "  {name:<16} hit {hit:>7.1} ns  miss {miss:>7.1} ns  scan {sc:>6.2} ns/e  {by:>6.1} B/key"
        );
    }
    rec.print_summary();
    rec.write(&out)?;
    Ok(())
}

/// Print what the machine reports about itself and exit.
///
/// The sweep derives its candidate from `cache_line`, so a machine whose line
/// size was defaulted rather than read produces a derived constant that looks
/// like every other one in the record. This mode is the cheap check -- seconds
/// rather than a full sweep -- that detection worked before a measurement is
/// taken against it. Exits non-zero when the line size was not detected, so
/// CI on a new platform fails loudly instead of measuring a guess.
fn machine_mode() -> std::io::Result<()> {
    let m = supdb::bench::Machine::detect();
    println!("{}", m.to_json().render());
    if !m.cache_line_detected {
        eprintln!(
            "!! cache line size was not detected on this platform; 64 assumed.\n\
             !! Set SUPDB_CACHE_LINE, or teach Machine::detect how to read it here."
        );
        std::process::exit(1);
    }
    Ok(())
}

fn sweep_mode(args: &Args) -> std::io::Result<()> {
    let n = args.num("--keys", 2_000_000);
    let lookups = args.num("--lookups", 200_000) as u64;
    let shape = Shape::parse(args.get("--shape").unwrap_or("decimal16")).expect("shape");
    let machine = supdb::bench::Machine::detect();
    let derived = machine.records_per_page();
    if !machine.cache_line_detected {
        eprintln!(
            "!! cache line size could not be read on this platform; assuming 64 bytes.\n\
             !! Apple Silicon is 128 -- the derived constant will be wrong by a factor of two.\n\
             !! Re-run with SUPDB_CACHE_LINE=128 (check: sysctl -n hw.cachelinesize)."
        );
    }
    let candidates: Vec<usize> = vec![8, 16, 32, 64, 128, 256];

    let keys = make_keys(shape, n);
    let exts = make_exts(keys.len());
    println!(
        "# {} x {} keys, cache line {} B, page {} B -> derived records/page = {}",
        shape.as_str(),
        keys.len(),
        machine.cache_line,
        machine.page_size,
        derived
    );
    println!(
        "{:>12} {:>10} {:>10} {:>12}",
        "records/page", "hit ns", "scan ns/e", "B/key"
    );

    // Build every candidate first, then measure them round-robin. Measuring all
    // repetitions of one setting before moving to the next is blocked
    // execution, and it gave an unstable answer here: two runs of the earlier
    // version of this sweep disagreed about which setting was best, because
    // whatever drifts over a run was being attributed to the setting.
    let built: Vec<Box<dyn Layout>> = candidates
        .iter()
        .map(|&pp| {
            std::env::set_var("SUPDB_PER_PAGE", pp.to_string());
            let l = build_layout("hash+paged", &keys, &exts);
            // A sweep that silently breaks lookups finds a very fast wrong answer.
            for i in (0..keys.len()).step_by((keys.len() / 401).max(1)) {
                assert_eq!(
                    l.lookup(&keys[i]),
                    Some(exts[i]),
                    "per_page {pp} broke lookups"
                );
            }
            l
        })
        .collect();
    std::env::remove_var("SUPDB_PER_PAGE");

    let samples = Trial::new(7).run(candidates.len(), |ci, _| {
        let l = &built[ci];
        let mut r = Rng::new(0x5EE9);
        let t = Instant::now();
        for _ in 0..lookups {
            let k = &keys[(r.next() as usize) % keys.len()];
            std::hint::black_box(l.lookup(k));
        }
        t.elapsed().as_secs_f64() * 1e9 / lookups as f64
    });

    let di = candidates.iter().position(|&p| p == derived).unwrap_or(0);
    let mut best = di;
    for (i, s) in samples.iter().enumerate() {
        if s.median() < samples[best].median() {
            best = i;
        }
    }
    for (i, pp) in candidates.iter().enumerate() {
        let scan = {
            let t = Instant::now();
            let mut acc = 0u64;
            let mut r = Rng::new(0x5CA5);
            for _ in 0..200 {
                let from = (r.next() as usize) % (keys.len() - 1000);
                acc = acc.wrapping_add(built[i].scan_from(from, 1000));
            }
            std::hint::black_box(acc);
            t.elapsed().as_secs_f64() * 1e9 / 200_000.0
        };
        let mark = if *pp == derived { "  <- derived" } else { "" };
        println!(
            "{:>12} {:>10.1} {:>10.2} {:>12.1} {:>7.1}%{}",
            pp,
            samples[i].median(),
            scan,
            built[i].logical_bytes() as f64 / keys.len() as f64,
            samples[i].rel_iqr() * 100.0,
            mark
        );
    }

    // The gate, not a bare ratio: if the best setting is not significantly
    // different from the derived one, the derivation is fine and the spread is
    // noise.
    let c = compare(&samples[best], &samples[di], supdb::bench::MIN_EFFECT);
    println!(
        "\n# derived {} at {:.1} ns; best measured {} at {:.1} ns",
        derived,
        samples[di].median(),
        candidates[best],
        samples[best].median()
    );
    println!(
        "# {}",
        c.summary(
            &format!("per_page={}", candidates[best]),
            &format!("derived={derived}")
        )
    );
    if c.verdict == supdb::bench::Verdict::NoDifference {
        println!("# The derivation is not distinguishable from the best setting on this machine.");
    } else {
        println!(
            "# The derivation is {:.0}% off the best setting here and needs revisiting.",
            (samples[di].median() / samples[best].median() - 1.0) * 100.0
        );
    }
    println!(
        "# The unified approach holds on this machine if that penalty is small. Run the same\n\
         # sweep on a machine with a different cache line to find out whether one derivation\n\
         # covers both, or whether a per-shape table is actually being bought."
    );
    Ok(())
}
