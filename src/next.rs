//! The next engine, milestone 1: a WAL is the only mutable thing.
//!
//! `docs/next-engine.md` is the design brief and every load-bearing decision
//! here cites a measurement. A durable commit is one framed append and one
//! fdatasync and nothing else, because f39 measured that shape at 1,191,125
//! ops/s with all engine work removed (F39.1) and today's engine 5.85x below
//! it on work this design deletes (F39.3). Sealed segments are byte-for-byte
//! today's store format, written by the existing `Store` writer and read by
//! `Blob` (`Options { redo_log: false, shards: 1 }` -- the logshed
//! configuration), so everything measured about that read path carries over,
//! browser reader included. There is no checkpoint: sealing is off the
//! commit path, and a store killed before its first seal opens from the WAL
//! alone, which is the brief's P-E and the flip of C3.4.
//!
//! Milestone 1 deliberately leaves out: deletes, scans over segments,
//! per-segment Blooms and range-partitioned compaction (the routing story
//! F40/F41 settled -- the read path here queries every source, which is the
//! unfiltered fan and is priced at 90ns per segment by F38.1), sharded
//! writers (P-D), and group commit. Each arrives with its own experiment.
//!
//! Crash discipline, in order, so every window is survivable:
//! commit = WAL append + fdatasync (the batch is durable or its tail frame
//! fails its CRC and replay stops before it); seal = write the segment to a
//! temp name, fsync it, rename into place, fsync the directory, then reset
//! the WAL -- a crash between any two of those leaves either a WAL that
//! replays the whole memtable or a complete renamed segment plus a WAL
//! whose sealed prefix is skipped by sequence number.

use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Write};
use std::path::{Path, PathBuf};

use crate::block::crc32;
use crate::bytes::MmapBytes;
use crate::{Blob, Options, Store};

fn err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn get_uvarint(buf: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*p)?;
        *p += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

#[derive(Clone)]
pub struct NextOptions {
    /// Memtable bytes that trigger a seal at the next commit. Sealing is off
    /// the commit path in cost accounting but runs on the committing thread
    /// in milestone 1; the brief's "Segment size" question owns this number.
    pub seal_bytes: usize,
    /// Options for the segment writer. Fixed to `redo_log: false, shards: 1`
    /// regardless of what is passed, because a sealed segment is written
    /// once and never reopened for writing -- the logshed finding that a
    /// 4 MiB redo arena in a write-once file is pure waste.
    pub segment: Options,
    /// How many overlapping L0 segments to tolerate before a partitioning
    /// merge. The brief's open "partitioned compaction policy" question in
    /// one number; f43 sweeps it.
    pub l0_trigger: usize,
    /// The measurement instrument: false keeps every segment in the
    /// unrouted L0 fan, which is milestone 3's behaviour exactly.
    pub compact: bool,
}

impl Default for NextOptions {
    fn default() -> NextOptions {
        NextOptions {
            seal_bytes: 64 << 20,
            segment: Options::default(),
            l0_trigger: 4,
            compact: true,
        }
    }
}

/// One WAL frame: `len u32 | crc u32 | seq u64 | klen uvarint | key | value`.
/// `len` covers everything after `crc`; `crc` covers the same bytes. The
/// value's length is `len` minus what precedes it, so values cost no second
/// length field.
const FRAME_HEADER: usize = 8;

struct Wal {
    file: File,
    path: PathBuf,
    /// Sequence of the next record to be written.
    seq: u64,
    /// Buffered frames since the last commit.
    pending: Vec<u8>,
}

impl Wal {
    fn create(path: &Path) -> Result<Wal> {
        let file = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
        Ok(Wal { file, path: path.to_path_buf(), seq: 0, pending: Vec::new() })
    }

    fn append(&mut self, key: &[u8], value: &[u8]) {
        let body_at = self.pending.len() + FRAME_HEADER;
        self.pending.extend_from_slice(&[0u8; FRAME_HEADER]);
        self.pending.extend_from_slice(&self.seq.to_le_bytes());
        put_uvarint(&mut self.pending, key.len() as u64);
        self.pending.extend_from_slice(key);
        self.pending.extend_from_slice(value);
        let body_len = (self.pending.len() - body_at) as u32;
        let crc = crc32(&self.pending[body_at..]);
        self.pending[body_at - 8..body_at - 4].copy_from_slice(&body_len.to_le_bytes());
        self.pending[body_at - 4..body_at].copy_from_slice(&crc.to_le_bytes());
        self.seq += 1;
    }

    /// The durable point: one write, one fdatasync. F39.1 is the budget this
    /// function is held to.
    fn commit(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.pending)?;
        self.file.sync_data()?;
        self.pending.clear();
        Ok(())
    }

    /// Replay a WAL into `apply`, stopping cleanly at a torn tail: a frame
    /// whose length runs past the buffer or whose CRC fails is the crash
    /// point, not corruption to report -- everything durable precedes it.
    /// Records with seq below `from` were sealed and are skipped.
    fn replay(path: &Path, from: u64, mut apply: impl FnMut(&[u8], &[u8])) -> Result<u64> {
        let mut buf = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut buf)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(from),
            Err(e) => return Err(e),
        }
        let mut p = 0usize;
        let mut next_seq = from;
        while buf.len() - p >= FRAME_HEADER {
            let len = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[p + 4..p + 8].try_into().unwrap());
            let body_at = p + FRAME_HEADER;
            let Some(end) = body_at.checked_add(len) else { break };
            if end > buf.len() || len < 8 {
                break;
            }
            let body = &buf[body_at..end];
            if crc32(body) != crc {
                break;
            }
            let seq = u64::from_le_bytes(body[..8].try_into().unwrap());
            let mut q = 8usize;
            let Some(klen) = get_uvarint(body, &mut q) else {
                return Err(err("wal frame key length is malformed"));
            };
            let kend = q
                .checked_add(klen as usize)
                .filter(|&e| e <= body.len())
                .ok_or_else(|| err("wal frame key runs past its frame"))?;
            if seq >= from {
                if seq != next_seq {
                    return Err(err("wal sequence gap: a durable record is missing"));
                }
                apply(&body[q..kend], &body[kend..]);
                next_seq = seq + 1;
            }
            p = end;
        }
        Ok(next_seq)
    }
}

/// A conservative key fence, encoded into a segment's file name.
///
/// Exactness is not required and truncation is not a bug: a fence may only
/// be *widened*, never narrowed, because a wide fence costs an unnecessary
/// probe while a narrow one loses a key. So the low bound is a 16-byte
/// prefix of the true minimum (a prefix sorts at or before the key it came
/// from) and the high bound is a 16-byte prefix of the true maximum with
/// the last byte carried up (which sorts strictly after every key sharing
/// that prefix). Keys of any length therefore fit in a bounded file name.
const FENCE_MAX: usize = 16;

fn fence_lo(min_key: &[u8]) -> Vec<u8> {
    min_key[..min_key.len().min(FENCE_MAX)].to_vec()
}

/// A half-open key range `[lo, hi)`, `None` above meaning unbounded. The
/// live partitions tile the key space with these, and every merge output
/// is named by one.
type Fence = (Vec<u8>, Option<Vec<u8>>);

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// One 64-byte block per query, four probe bits inside it: the structure
/// f40 measured at 82.1% of a single store when it is the only routing
/// there is (F40.1). Here it guards only the bounded L0 tail, because
/// F41.1/F41.2 refuted every keys-sized global router -- the partitioned
/// levels below are routed by fences that cost two comparisons.
pub(crate) struct BlockedBloom {
    blocks: Vec<[u64; 8]>,
}

impl BlockedBloom {
    fn with_capacity(n: usize) -> BlockedBloom {
        BlockedBloom { blocks: vec![[0u64; 8]; (n * 10).div_ceil(512).max(1)] }
    }

    fn hash(key: &[u8]) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for &b in key {
            h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
        h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        h ^ (h >> 31)
    }

    fn slots(&self, key: &[u8]) -> (usize, [(usize, u64); 4]) {
        let h = BlockedBloom::hash(key);
        let bi = (h >> 32) as usize % self.blocks.len();
        let mut probes = [(0usize, 0u64); 4];
        let mut x = h;
        for p in &mut probes {
            x = x.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(1);
            let bit = (x >> 55) as usize & 511;
            *p = (bit >> 6, 1u64 << (bit & 63));
        }
        (bi, probes)
    }

    fn insert(&mut self, key: &[u8]) {
        let (bi, probes) = self.slots(key);
        for (w, m) in probes {
            self.blocks[bi][w] |= m;
        }
    }

    #[inline]
    fn maybe_contains(&self, key: &[u8]) -> bool {
        let (bi, probes) = self.slots(key);
        let b = &self.blocks[bi];
        probes.iter().all(|&(w, m)| b[w] & m != 0)
    }
}

/// A merge in flight: the input names it will retire, and the thread
/// producing the outputs that replace them.
type Compaction = (Vec<String>, std::thread::JoinHandle<Result<Vec<String>>>);

/// A live segment: the mapped store, where it sits in the level structure,
/// and whatever routing it carries. L0 segments come straight from a seal,
/// overlap each other freely, and are gated by a Bloom; L1 segments come
/// from a partitioning merge, are disjoint, and are gated by their fence.
struct Seg {
    blob: Blob<MmapBytes>,
    name: String,
    level: u8,
    lo: Vec<u8>,
    hi: Option<Vec<u8>>,
    bloom: Option<BlockedBloom>,
}

impl Seg {
    /// Cheap ordered key walk: O(extents), no block touched -- the property
    /// `scan_counts_fixed` exists for. The width argument is irrelevant
    /// here because only the keys are wanted.
    fn for_each_key(blob: &Blob<MmapBytes>, mut f: impl FnMut(&[u8])) -> Result<()> {
        blob.scan_counts_fixed(b"", usize::MAX, 8, |k, _| {
            f(k);
            true
        })
        .map_err(|e| err(&format!("segment key walk: {e}")))?;
        Ok(())
    }

    fn open(dir: &Path, name: &str) -> Result<Seg> {
        let blob = Blob::open(MmapBytes::open(&dir.join(name))?)
            .map_err(|e| err(&format!("segment {name}: {e}")))?;
        // `pcs-` is a range-ALIGNED L0 piece: a seal split at the live
        // partition boundaries, so it carries a fence like a partition and
        // overlaps only the pieces of its own range. That alignment is what
        // makes a merge O(range) instead of O(store).
        if let Some(rest) = name.strip_prefix("pcs-").and_then(|r| r.strip_suffix(".sup")) {
            let f: Vec<&str> = rest.split('-').collect();
            if f.len() != 4 {
                return Err(err("aligned piece name is malformed"));
            }
            let lo = unhex(f[2]).ok_or_else(|| err("segment fence is malformed"))?;
            let hi = if f[3].is_empty() {
                None
            } else {
                Some(unhex(f[3]).ok_or_else(|| err("segment fence is malformed"))?)
            };
            let mut bloom = BlockedBloom::with_capacity(blob.keys());
            Seg::for_each_key(&blob, |k| bloom.insert(k))?;
            return Ok(Seg {
                blob,
                name: name.to_string(),
                level: 0,
                lo,
                hi,
                bloom: Some(bloom),
            });
        }
        if let Some(rest) = name.strip_prefix("par-").and_then(|r| r.strip_suffix(".sup")) {
            // par-<id>-<endseq>-<lo hex>-<hi hex>: fences route this one,
            // so nothing is walked at open. The unbounded high fence is the
            // empty string.
            let f: Vec<&str> = rest.split('-').collect();
            if f.len() != 4 {
                return Err(err("partitioned segment name is malformed"));
            }
            let lo = unhex(f[2]).ok_or_else(|| err("segment fence is malformed"))?;
            let hi = if f[3].is_empty() {
                None
            } else {
                Some(unhex(f[3]).ok_or_else(|| err("segment fence is malformed"))?)
            };
            return Ok(Seg { blob, name: name.to_string(), level: 1, lo, hi, bloom: None });
        }
        // L0: build the Bloom by walking the segment's keys. That walk is
        // O(keys) and it is affordable for exactly one reason -- L0 is
        // bounded at `l0_trigger` segments of at most `seal_bytes` each, so
        // this cost is bounded where the level below it is not.
        let mut bloom = BlockedBloom::with_capacity(blob.keys());
        Seg::for_each_key(&blob, |k| bloom.insert(k))?;
        Ok(Seg {
            blob,
            name: name.to_string(),
            level: 0,
            lo: Vec::new(),
            hi: None,
            bloom: Some(bloom),
        })
    }

    /// Could this segment hold `key`? A fence answers exactly; a Bloom
    /// answers with false positives and never a false negative.
    #[inline]
    fn may_hold(&self, key: &[u8]) -> bool {
        if key < self.lo.as_slice() {
            return false;
        }
        if self.hi.as_ref().is_some_and(|h| key >= h.as_slice()) {
            return false;
        }
        self.bloom.as_ref().is_none_or(|b| b.maybe_contains(key))
    }

    /// Could this segment hold anything at or after `from`?
    #[inline]
    fn may_reach(&self, from: &[u8]) -> bool {
        self.hi.as_ref().is_none_or(|h| from < h.as_slice())
    }
}

/// The memtable, built so that an append allocates nothing per key or per
/// value: f42's decomposition priced the HashMap<Box<[u8]>, Vec> version at
/// 456k ops/s of the gap to the floor, more than the seal itself (F42.3).
/// Keys live in one bump arena; values live in another as per-key backward
/// chains (each chunk records the previous chunk's offset, and a read or
/// seal walks the chain and reverses it); the table is open-addressed with
/// linear probing at load <= 0.5, resized by rehash of the fixed-size
/// entries only -- key and value bytes never move.
struct MemTable {
    entries: Vec<MemEntry>,
    mask: usize,
    len: usize,
    keys: Vec<u8>,
    vals: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct MemEntry {
    hash: u64,
    key_off: u32,
    key_len: u32,
    /// Offset+1 of this key's newest chunk in `vals`; 0 = vacant slot.
    head: u64,
    count: u64,
}

const NO_CHUNK: u64 = u64::MAX;

fn mem_hash(key: &[u8]) -> u64 {
    // FNV-1a, then a splitmix finish; the std SipHash was part of what f42
    // priced.
    let mut h = 0xcbf29ce484222325u64;
    for &b in key {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h | 1 // 0 marks a vacant slot
}

impl MemTable {
    fn new() -> MemTable {
        MemTable {
            entries: vec![MemEntry::default(); 1024],
            mask: 1023,
            len: 0,
            keys: Vec::new(),
            vals: Vec::new(),
        }
    }

    fn key_of<'a>(keys: &'a [u8], e: &MemEntry) -> &'a [u8] {
        &keys[e.key_off as usize..(e.key_off + e.key_len) as usize]
    }

    fn grow(&mut self) {
        let cap = self.entries.len() * 2;
        let mut entries = vec![MemEntry::default(); cap];
        let mask = cap - 1;
        for e in self.entries.iter().filter(|e| e.hash != 0) {
            let mut i = (e.hash as usize) & mask;
            while entries[i].hash != 0 {
                i = (i + 1) & mask;
            }
            entries[i] = *e;
        }
        self.entries = entries;
        self.mask = mask;
    }

    fn append(&mut self, key: &[u8], value: &[u8]) {
        if (self.len + 1) * 2 > self.entries.len() {
            self.grow();
        }
        let hash = mem_hash(key);
        let mut i = (hash as usize) & self.mask;
        loop {
            let e = self.entries[i];
            if e.hash == 0 {
                let key_off = self.keys.len() as u32;
                self.keys.extend_from_slice(key);
                let head = self.push_chunk(NO_CHUNK, value);
                self.entries[i] = MemEntry {
                    hash,
                    key_off,
                    key_len: key.len() as u32,
                    head: head + 1,
                    count: 1,
                };
                self.len += 1;
                return;
            }
            if e.hash == hash && MemTable::key_of(&self.keys, &e) == key {
                let head = self.push_chunk(e.head - 1, value);
                self.entries[i].head = head + 1;
                self.entries[i].count += 1;
                return;
            }
            i = (i + 1) & self.mask;
        }
    }

    fn push_chunk(&mut self, prev: u64, value: &[u8]) -> u64 {
        let off = self.vals.len() as u64;
        self.vals.extend_from_slice(&prev.to_le_bytes());
        put_uvarint(&mut self.vals, value.len() as u64);
        self.vals.extend_from_slice(value);
        off
    }

    /// Chunk offsets for one entry, oldest first.
    fn chain(&self, e: &MemEntry) -> Vec<usize> {
        let mut offs = Vec::with_capacity(e.count as usize);
        let mut at = e.head - 1;
        while at != NO_CHUNK {
            offs.push(at as usize);
            at = u64::from_le_bytes(self.vals[at as usize..at as usize + 8].try_into().unwrap());
        }
        offs.reverse();
        offs
    }

    fn value_at(&self, off: usize) -> &[u8] {
        let mut p = off + 8;
        let len = get_uvarint(&self.vals, &mut p).expect("memtable framing") as usize;
        &self.vals[p..p + len]
    }

    fn get(&self, key: &[u8]) -> Option<&MemEntry> {
        let hash = mem_hash(key);
        let mut i = (hash as usize) & self.mask;
        loop {
            let e = &self.entries[i];
            if e.hash == 0 {
                return None;
            }
            if e.hash == hash && MemTable::key_of(&self.keys, e) == key {
                return Some(e);
            }
            i = (i + 1) & self.mask;
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The live segment set, named atomically.
///
/// A compaction writes new files and retires old ones, and a crash between
/// those two acts would otherwise leave both on disk -- every merged record
/// readable twice. The manifest is the swap point: it is written to a temp
/// name, fsynced, renamed over the old one and the directory fsynced, so a
/// reopen sees exactly one of the two sets. Segment files not named by it
/// are orphans from an interrupted job and are deleted at open.
///
/// `SUPDBMAN\x01 | u32 body_len | u32 crc | body`, body being the covered
/// WAL sequence and then each live segment's name.
const MANIFEST_MAGIC: &[u8; 9] = b"SUPDBMAN\x01";

fn manifest_write(dir: &Path, covered_seq: u64, names: &[String]) -> Result<()> {
    let mut body = Vec::new();
    body.extend_from_slice(&covered_seq.to_le_bytes());
    body.extend_from_slice(&(names.len() as u32).to_le_bytes());
    for n in names {
        body.extend_from_slice(&(n.len() as u16).to_le_bytes());
        body.extend_from_slice(n.as_bytes());
    }
    let mut out = Vec::with_capacity(body.len() + 17);
    out.extend_from_slice(MANIFEST_MAGIC);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32(&body).to_le_bytes());
    out.extend_from_slice(&body);

    let tmp = dir.join("manifest.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join("manifest"))?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

/// `None` when no manifest exists -- a store that has never sealed, or one
/// written before manifests. A manifest that fails its CRC is a torn write
/// of the file that is supposed to be atomic, so it is refused rather than
/// guessed at.
fn manifest_read(dir: &Path) -> Result<Option<(u64, Vec<String>)>> {
    let mut buf = Vec::new();
    match File::open(dir.join("manifest")) {
        Ok(mut f) => f.read_to_end(&mut buf)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if buf.len() < 17 || &buf[..9] != MANIFEST_MAGIC {
        return Err(err("manifest magic is wrong"));
    }
    let len = u32::from_le_bytes(buf[9..13].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(buf[13..17].try_into().unwrap());
    let body = buf.get(17..17 + len).ok_or_else(|| err("manifest is truncated"))?;
    if crc32(body) != crc {
        return Err(err("manifest failed its checksum"));
    }
    let covered = u64::from_le_bytes(body[..8].try_into().unwrap());
    let n = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
    let mut names = Vec::with_capacity(n);
    let mut p = 12usize;
    for _ in 0..n {
        let l = u16::from_le_bytes(
            body.get(p..p + 2).ok_or_else(|| err("manifest is truncated"))?.try_into().unwrap(),
        ) as usize;
        p += 2;
        let raw = body.get(p..p + l).ok_or_else(|| err("manifest is truncated"))?;
        names.push(String::from_utf8(raw.to_vec()).map_err(|_| err("manifest name is not utf8"))?);
        p += l;
    }
    Ok(Some((covered, names)))
}

/// The partitioning merge, run on a background thread.
///
/// Every input's keys are walked cheaply, unioned and sorted, then split
/// into `parts` contiguous ranges; each range is written as one segment
/// whose fence is its own boundaries, so the result is disjoint and routes
/// by two comparisons. Values for a key are appended input by input in age
/// order, which is what keeps a multivalue key's append order intact across
/// a merge.
///
/// The union key list is materialised rather than streamed, because `Blob`
/// hands out keys through a callback and not an iterator. It is the one
/// place this milestone spends memory proportional to the store; a
/// streaming k-way merge is the fix if it ever matters.
/// Everything a merge needs to know, gathered so the job takes one
/// argument instead of eight.
struct MergePlan {
    dir: PathBuf,
    inputs: Vec<String>,
    first_id: u64,
    end_seq: u64,
    parts: usize,
    fences: Option<Vec<Fence>>,
    max_keys: usize,
    opts: Options,
}

fn compact_job(plan: MergePlan) -> Result<Vec<String>> {
    let MergePlan { dir, inputs, first_id, end_seq, parts, fences, max_keys, opts } = plan;
    compact_run(dir, inputs, first_id, end_seq, parts, fences, max_keys, opts)
}

#[allow(clippy::too_many_arguments)]
fn compact_run(
    dir: PathBuf,
    inputs: Vec<String>,
    first_id: u64,
    end_seq: u64,
    parts: usize,
    // `Some` names the exact output fences: one segment per fence, keys
    // assigned by it. That is how boundaries stay STABLE once they exist
    // -- a merge that re-derived them would misalign every piece sealed
    // under the old ones, forcing another full merge, and that loop is
    // what kept device bytes at the full-rewrite level. `None` is the
    // first partitioning only, which has no boundaries to preserve.
    fences: Option<Vec<Fence>>,
    // Keys a partition may hold before the merge splits it in two. Stable
    // boundaries keep pieces aligned, but a boundary set that NEVER grows
    // leaves partitions growing with the store: at 1M keys the store had
    // settled into 8 partitions of 14.5MB each, and a bigger partition is
    // a bigger index section and more misses per probe. A range splits
    // when it outgrows this, which disturbs only its own pieces and only
    // until the next seal.
    max_keys: usize,
    opts: Options,
) -> Result<Vec<String>> {
    let _ = &parts;
    let mut blobs = Vec::with_capacity(inputs.len());
    for name in &inputs {
        blobs.push(
            Blob::open(MmapBytes::open(&dir.join(name))?)
                .map_err(|e| err(&format!("compact input {name}: {e}")))?,
        );
    }
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for b in &blobs {
        Seg::for_each_key(b, |k| keys.push(k.to_vec()))?;
    }
    keys.sort_unstable();
    keys.dedup();
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let parts = match &fences {
        Some(f) => f.len().max(1),
        None => parts.max(1).min(keys.len()),
    };
    let _ = parts;
    let per = keys.len().div_ceil(parts);

    // The partition set must TILE the key space: every key routes to
    // exactly one partition, with no gap between one partition's high
    // fence and the next one's low fence. Deriving each fence separately
    // from its own chunk does not do that -- `fence_hi(last of chunk i)`
    // and `fence_lo(first of chunk i+1)` are different values, and a key
    // landing between them is sealed into a range whose fence then denies
    // it on the read path. Silent value loss, found at 1M keys by an
    // experiment's own assertion after the contract tests (whose stores
    // are too small to make a gap) passed clean.
    //
    // So a boundary is ONE value, shared by the partitions on either side,
    // and the keys are sliced BY the boundaries rather than the boundaries
    // derived from the slices.
    let mut bounds: Vec<Vec<u8>> = Vec::new();
    if fences.is_none() {
        for i in 1..parts {
            let b = fence_lo(&keys[(i * per).min(keys.len() - 1)]);
            if bounds.last().is_none_or(|p| p != &b) {
                bounds.push(b);
            }
        }
    }
    // Slice by whichever boundaries govern: the given fences when the
    // caller has them, the derived ones when it does not.
    let mut slices: Vec<(usize, usize)> = Vec::new();
    let mut given: Vec<Fence> = Vec::new();
    match &fences {
        Some(fs) => {
            for (lo, hi) in fs {
                let from = keys.partition_point(|k| k.as_slice() < lo.as_slice());
                let to = match hi {
                    Some(h) => keys.partition_point(|k| k.as_slice() < h.as_slice()),
                    None => keys.len(),
                };
                let to = to.max(from);
                let n = (to - from).div_ceil(max_keys.max(1)).max(1);
                let per_sub = (to - from).div_ceil(n);
                for i in 0..n {
                    let sf = from + i * per_sub;
                    let st = (sf + per_sub).min(to);
                    if sf >= st {
                        continue;
                    }
                    // Sub-fences tile the fence they came from: the first
                    // keeps its low bound, the last its high bound, and the
                    // joins are single shared values.
                    let sub_lo =
                        if i == 0 { lo.clone() } else { fence_lo(&keys[sf]) };
                    let sub_hi = if st == to {
                        hi.clone()
                    } else {
                        Some(fence_lo(&keys[st]))
                    };
                    slices.push((sf, st));
                    given.push((sub_lo, sub_hi));
                }
            }
        }
        None => {
            let mut at = 0usize;
            for b in &bounds {
                let end = keys.partition_point(|k| k.as_slice() < b.as_slice());
                if end > at {
                    slices.push((at, end));
                    at = end;
                }
            }
            slices.push((at, keys.len()));
        }
    }

    let mut out = Vec::with_capacity(slices.len());
    for (pi, &(from, to)) in slices.iter().enumerate() {
        let chunk = &keys[from..to];
        if chunk.is_empty() {
            continue;
        }
        let id = first_id + pi as u64;
        let (lo, hi) = match &fences {
            Some(_) => given[pi].clone(),
            // Unbounded at both ends of the set, and every interior fence
            // is the boundary shared with the neighbour.
            None => (
                if pi == 0 { Vec::new() } else { bounds[pi - 1].clone() },
                if pi + 1 == slices.len() { None } else { Some(bounds[pi].clone()) },
            ),
        };
        let name = format!(
            "par-{id:08}-{end_seq:016}-{}-{}.sup",
            hex(&lo),
            hi.as_deref().map(hex).unwrap_or_default()
        );
        let tmp = dir.join(format!("compact-{id:08}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        {
            let store = Store::create(&tmp, opts.clone())
                .map_err(|e| err(&format!("compact create: {e}")))?;
            for key in chunk {
                // Insurance against the class of bug that produced this
                // line: a merge told to write a fence must contain what it
                // writes, or the read path will deny it and no test will
                // say so.
                if key.as_slice() < lo.as_slice()
                    || hi.as_ref().is_some_and(|h| key.as_slice() >= h.as_slice())
                {
                    return Err(err("compaction would write a key outside its fence"));
                }
                for b in &blobs {
                    b.read_all(key, |v| {
                        // `Store::append` cannot fail for a value already in
                        // a segment, and the callback cannot return.
                        let _ = store.append(key, v);
                    })
                    .map_err(|e| err(&format!("compact read: {e}")))?;
                }
            }
            store.checkpoint().map_err(|e| err(&format!("compact checkpoint: {e}")))?;
            store.close().map_err(|e| err(&format!("compact close: {e}")))?;
        }
        std::fs::rename(&tmp, dir.join(&name))?;
        out.push(name);
    }
    File::open(&dir)?.sync_all()?;
    Ok(out)
}

pub struct Db {
    dir: PathBuf,
    opts: NextOptions,
    wal: Wal,
    wal_id: u64,
    mem: MemTable,
    mem_bytes: usize,
    /// Live segments. Partitioned (L1) first and disjoint, then L0 oldest
    /// to newest: a key's values come back in append order because a merge
    /// preserves it and everything L0 holds is newer than everything L1
    /// holds.
    segs: Vec<Seg>,
    next_seg: u64,
    /// WAL files whose records no segment has been *named* as covering
    /// yet. One rule governs every one of them: a WAL may be deleted only
    /// after the manifest names a segment that covers its records. The
    /// model oracle found this twice in one afternoon -- the seal thread
    /// deleting the rotated WAL on rename, before the publish that made its
    /// segment reachable, and `open` deleting older WALs once it had
    /// replayed them into a memtable that lives only in memory. Both were
    /// the same mistake: treating "the data is somewhere" as "the data is
    /// durable somewhere a reopen can find".
    retiring_wals: Vec<PathBuf>,
    /// The WAL sequence every live segment covers between them. Kept as a
    /// monotone field rather than derived from segment names: a compaction
    /// renames the whole live set, and deriving the bound from the names it
    /// happens to produce let it move BACKWARDS -- caught by the model
    /// oracle as "wal sequence gap: a durable record is missing" on the
    /// reopen after a merge. A durability bound may only ever rise.
    covered_seq: u64,
    /// A partitioning merge in flight. Its inputs stay live and readable
    /// until the manifest names its outputs instead, which is what makes
    /// the swap atomic across a crash.
    compacting: Option<Compaction>,
    /// A seal in flight: the frozen memtable stays readable (it is newer
    /// than every segment and older than `mem`) while a thread writes it
    /// out; `join_seal` collects the finished segment.
    frozen: Option<std::sync::Arc<MemTable>>,
    sealing: Option<std::thread::JoinHandle<Result<Vec<String>>>>,
    /// Sorted keys of the unsealed sources (memtable + frozen), built lazily
    /// by `scan` and reused until a write or a seal changes what is
    /// unsealed. Without this, every scan walked the whole memtable: the
    /// ext-kv scan phase spent 15 minutes a rep in that walk, twice -- once
    /// through the live table and once through the frozen one.
    scan_keys: std::cell::RefCell<Option<(u64, Vec<Vec<u8>>)>>,
}

impl Db {
    /// WAL files are numbered and rotate at each seal: the sealing thread
    /// owns the old file and deletes it once its segment is renamed into
    /// place, while commits continue into the next file. Replay walks them
    /// in id order; sequence numbers are continuous across the boundary.
    fn wal_path(dir: &Path, id: u64) -> PathBuf {
        dir.join(format!("wal-{id:08}"))
    }

    /// The end-of-covered-sequence rides the file name so the rename that
    /// publishes a segment also publishes, atomically, which WAL records it
    /// covers. A crash between the rename and the WAL reset then leaves a
    /// WAL whose covered prefix is skipped by sequence on replay instead of
    /// replayed into duplicates.
    fn seg_name(n: u64, end_seq: u64) -> String {
        format!("seg-{n:08}-{end_seq:016}.sup")
    }

    /// Both `seg-` and `par-` names carry id then covered end-sequence in
    /// their first two fields, so one parser serves the manifest, the
    /// orphan sweep and the replay bound.
    fn name_field(name: &str, i: usize) -> Option<u64> {
        let rest = name
            .strip_prefix("seg-")
            .or_else(|| name.strip_prefix("par-"))
            .or_else(|| name.strip_prefix("pcs-"))?;
        rest.strip_suffix(".sup")?.split('-').nth(i)?.parse().ok()
    }

    fn name_id(name: &str) -> Option<u64> {
        Db::name_field(name, 0)
    }

    fn name_end_seq(name: &str) -> Option<u64> {
        Db::name_field(name, 1)
    }

    /// The live set, in the order the manifest should record it.
    fn live_names(&self) -> Vec<String> {
        self.segs.iter().map(|s| s.name.clone()).collect()
    }

    fn segment_opts(opts: &NextOptions) -> Options {
        Options { redo_log: false, shards: 1, ..opts.segment.clone() }
    }

    pub fn create(dir: &Path, opts: NextOptions) -> Result<Db> {
        std::fs::create_dir_all(dir)?;
        let wal = Wal::create(&Db::wal_path(dir, 0))?;
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal,
            wal_id: 0,
            mem: MemTable::new(),
            mem_bytes: 0,
            segs: Vec::new(),
            next_seg: 0,
            frozen: None,
            sealing: None,
            compacting: None,
            retiring_wals: Vec::new(),
            covered_seq: 0,
            scan_keys: std::cell::RefCell::new(None),
        })
    }

    /// Open from the directory alone. Segments are complete by construction
    /// (they were renamed into place after their fsync); the WAL replays
    /// whatever outlived the last seal, torn tail tolerated. A directory
    /// with no segments and only a WAL is a store killed before its first
    /// seal, and it opens -- the brief's P-E.
    pub fn open(dir: &Path, opts: NextOptions) -> Result<Db> {
        // The manifest is the truth when it exists. Without one -- a store
        // killed before its first seal -- the directory is scanned, which
        // is also how a store written before manifests still opens.
        let mut on_disk: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if (name.starts_with("seg-")
                || name.starts_with("par-")
                || name.starts_with("pcs-"))
                && name.ends_with(".sup")
            {
                on_disk.push(name);
            }
        }
        let (sealed, live) = match manifest_read(dir)? {
            Some((covered, names)) => (covered, names),
            None => {
                let mut names: Vec<String> =
                    on_disk.iter().filter(|n| n.starts_with("seg-")).cloned().collect();
                names.sort_unstable();
                let covered = names
                    .last()
                    .and_then(|n| Db::name_end_seq(n))
                    .unwrap_or(0);
                (covered, names)
            }
        };
        // Orphans: files a crash left behind from a merge or a seal whose
        // manifest never landed. The manifest says what is live, so
        // anything else is unreachable and is removed rather than kept.
        for name in &on_disk {
            if !live.contains(name) {
                let _ = std::fs::remove_file(dir.join(name));
            }
        }
        let mut segs = Vec::with_capacity(live.len());
        for name in &live {
            segs.push(Seg::open(dir, name)?);
        }
        segs.sort_by(|a, b| {
            b.level.cmp(&a.level).then_with(|| a.lo.cmp(&b.lo)).then_with(|| a.name.cmp(&b.name))
        });
        let seg_ids: Vec<(u64, u64)> = live
            .iter()
            .filter_map(|n| Some((Db::name_id(n)?, Db::name_end_seq(n)?)))
            .collect();
        let mut wal_ids: Vec<u64> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_prefix("wal-") {
                wal_ids.push(id.parse().map_err(|_| err("wal file name is malformed"))?);
            }
        }
        wal_ids.sort_unstable();
        let mut mem = MemTable::new();
        let mut mem_bytes = 0usize;
        let mut from = sealed;
        for &id in &wal_ids {
            from = Wal::replay(&Db::wal_path(dir, id), from, |k, v| {
                mem.append(k, v);
                mem_bytes += k.len() + v.len();
            })?;
        }
        // Older WALs are kept, not swept: their records are in the
        // memtable and the memtable is not durable. They retire at the
        // next seal, when a named segment covers them.
        let retiring: Vec<PathBuf> = wal_ids
            .iter()
            .rev()
            .skip(1)
            .map(|&id| Db::wal_path(dir, id))
            .collect();
        let wal_id = wal_ids.last().copied().unwrap_or(0);
        let wal_path = Db::wal_path(dir, wal_id);
        let file = OpenOptions::new().create(true).append(true).open(&wal_path)?;
        let next_seg = seg_ids.iter().map(|&(n, _)| n + 1).max().unwrap_or(0);
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal: Wal { file, path: wal_path, seq: from, pending: Vec::new() },
            wal_id,
            mem,
            mem_bytes,
            segs,
            next_seg,
            frozen: None,
            sealing: None,
            compacting: None,
            retiring_wals: retiring,
            covered_seq: sealed,
            scan_keys: std::cell::RefCell::new(None),
        })
    }

    /// Buffered until `commit`; visible to this handle's reads immediately,
    /// which is the read-your-writes contract `Store::read_all` set.
    pub fn append(&mut self, key: &[u8], value: &[u8]) {
        self.wal.append(key, value);
        self.mem.append(key, value);
        self.mem_bytes += key.len() + value.len();
    }

    /// The durability point: WAL append + fdatasync. If the memtable has
    /// crossed the seal threshold, seal after the commit -- after, so the
    /// batch's durability never waits on a segment write.
    pub fn commit(&mut self) -> Result<()> {
        self.wal.commit()?;
        if self.sealing.as_ref().is_some_and(|h| h.is_finished()) {
            self.join_seal()?;
        }
        if self.mem_bytes >= self.opts.seal_bytes {
            self.seal()?;
        }
        Ok(())
    }

    /// Freeze the memtable, rotate the WAL, and hand the frozen table to a
    /// thread that writes it as one immutable segment in today's store
    /// format -- fsync, rename into place (the name carrying the covered
    /// end-sequence), fsync the directory, delete the rotated-out WAL.
    /// Commits continue into the new WAL while it runs; at most one seal is
    /// in flight, so a second trigger joins the first (backpressure).
    pub fn seal(&mut self) -> Result<()> {
        self.wal.commit()?;
        if self.mem.is_empty() {
            return Ok(());
        }
        self.join_seal()?;
        let frozen = std::sync::Arc::new(std::mem::replace(&mut self.mem, MemTable::new()));
        self.mem_bytes = 0;
        let old_wal = std::mem::replace(
            &mut self.wal,
            Wal::create(&Db::wal_path(&self.dir, self.wal_id + 1))?,
        );
        self.wal_id += 1;
        self.wal.seq = old_wal.seq;
        // The live partition fences, if any. A seal splits the memtable at
        // them and writes one piece per range, so every piece overlaps only
        // its own range and a later merge touches one partition instead of
        // the whole store. Before the first partitioning there are none and
        // the seal writes a single full-range segment.
        let fences: Vec<Fence> = self
            .segs
            .iter()
            .filter(|s| s.level > 0)
            .map(|s| (s.lo.clone(), s.hi.clone()))
            .collect();
        let first_id = self.next_seg;
        self.next_seg += fences.len().max(1) as u64;
        let dir = self.dir.clone();
        let opts = Db::segment_opts(&self.opts);
        let end_seq = old_wal.seq;
        self.retiring_wals.push(old_wal.path.clone());
        drop(old_wal);
        let mem = frozen.clone();
        self.frozen = Some(frozen);
        self.sealing = Some(std::thread::spawn(move || {
            // In KEY order, not hash order. A segment written in the
            // memtable's iteration order scatters each key's values across
            // blocks by hash, so an ordered scan walks the file randomly;
            // written sorted, a scan walks it forwards. This is W1.3's
            // finding in the new engine -- how the roll writes decides what
            // the read costs -- and the sort is affordable because a seal is
            // off the commit path. The same sort is what makes splitting at
            // the fences a matter of slicing.
            let mut order: Vec<&MemEntry> =
                mem.entries.iter().filter(|e| e.hash != 0).collect();
            order.sort_unstable_by_key(|e| MemTable::key_of(&mem.keys, e));

            let ranges: Vec<Fence> = if fences.is_empty() {
                vec![(Vec::new(), None)]
            } else {
                fences
            };
            let mut names = Vec::new();
            let mut at = 0usize;
            for (ri, (lo, hi)) in ranges.iter().enumerate() {
                let start = at;
                while at < order.len() {
                    let k = MemTable::key_of(&mem.keys, order[at]);
                    if hi.as_ref().is_some_and(|h| k >= h.as_slice()) {
                        break;
                    }
                    at += 1;
                }
                if at == start {
                    continue;
                }
                let id = first_id + ri as u64;
                let tmp = dir.join(format!("seal-{id:08}.tmp"));
                let _ = std::fs::remove_file(&tmp);
                {
                    let store = Store::create(&tmp, opts.clone())
                        .map_err(|e| err(&format!("seal create: {e}")))?;
                    for e in &order[start..at] {
                        let key = MemTable::key_of(&mem.keys, e);
                        for off in mem.chain(e) {
                            store
                                .append(key, mem.value_at(off))
                                .map_err(|e| err(&format!("seal append: {e}")))?;
                        }
                    }
                    store.checkpoint().map_err(|e| err(&format!("seal checkpoint: {e}")))?;
                    store.close().map_err(|e| err(&format!("seal close: {e}")))?;
                }
                let name = if ranges.len() == 1 && lo.is_empty() && hi.is_none() {
                    Db::seg_name(id, end_seq)
                } else {
                    format!(
                        "pcs-{id:08}-{end_seq:016}-{}-{}.sup",
                        hex(lo),
                        hi.as_deref().map(hex).unwrap_or_default()
                    )
                };
                std::fs::rename(&tmp, dir.join(&name))?;
                names.push(name);
            }
            File::open(&dir)?.sync_all()?;
            Ok(names)
        }));
        Ok(())
    }

    /// Commit, seal, and wait for the segment: the full drain, for a caller
    /// entering a read-heavy phase. `seal` alone leaves the frozen memtable
    /// readable until an eventual join, which is right for a writer that
    /// keeps committing and wrong for one that stops: the ext-kv adapter
    /// sealed without draining and every scan walked the 550k-key frozen
    /// table for the rest of the phase -- the same artifact the seal was
    /// supposed to remove, back through the side door.
    pub fn flush(&mut self) -> Result<()> {
        self.wal.commit()?;
        self.seal()?;
        self.join_seal()?;
        self.join_compact()
    }

    /// Collect a finished (or in-flight) seal: join the thread, open its
    /// segment, retire the frozen memtable.
    fn join_seal(&mut self) -> Result<()> {
        let Some(handle) = self.sealing.take() else {
            return Ok(());
        };
        let names = handle.join().map_err(|_| err("seal thread panicked"))??;
        for name in &names {
            self.covered_seq = self.covered_seq.max(Db::name_end_seq(name).unwrap_or(0));
            self.segs.push(Seg::open(&self.dir, name)?);
        }
        self.sort_segs();
        self.frozen = None;
        self.publish()?;
        for old in std::mem::take(&mut self.retiring_wals) {
            let _ = std::fs::remove_file(old);
        }
        if self.opts.compact {
            self.maybe_compact()?;
        }
        Ok(())
    }

    /// Partitions first in key order, then L0 by (range, age). `read_all`
    /// binary-searches the first group and walks a contiguous run of the
    /// second, so both depend on this order.
    fn sort_segs(&mut self) {
        self.segs.sort_by(|a, b| {
            b.level
                .cmp(&a.level)
                .then_with(|| a.lo.cmp(&b.lo))
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    /// A merge is due when any one range has accumulated `l0_trigger`
    /// aligned pieces -- or, before the first partitioning, when that many
    /// full-range segments have piled up.
    fn maybe_compact(&mut self) -> Result<()> {
        // Collect a finished merge BEFORE deciding. Its outputs are the
        // partitions the decision depends on, and deciding first meant
        // deciding against a store that still looked unpartitioned: every
        // merge then took the full re-partitioning path and the
        // incremental one never ran once in a whole load.
        if self.compacting.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            self.join_compact()?;
        }
        let parts: Vec<Fence> = self
            .segs
            .iter()
            .filter(|s| s.level > 0)
            .map(|s| (s.lo.clone(), s.hi.clone()))
            .collect();
        if parts.is_empty() {
            if self.l0_len() >= self.opts.l0_trigger {
                return self.start_compact(None);
            }
            return Ok(());
        }
        // A seal that finished DURING the initial partitioning wrote a
        // full-range segment against the fences that existed when it
        // started, so it now spans several partitions. Folding it into one
        // range would write keys outside that range's fence and the read
        // path would then deny them -- silent loss, which is exactly how
        // this was found. Anything not aligned to a live range forces a
        // full re-partitioning instead.
        if let Some(wide) = self
            .segs
            .iter()
            .find(|s| s.level == 0 && !parts.iter().any(|f| (s.lo.clone(), s.hi.clone()) == *f))
        {
            // It spans several ranges: merge it into every partition it
            // overlaps, each output keeping its own existing fence. The
            // boundaries do not move, so nothing else becomes misaligned.
            let (wlo, whi) = (wide.lo.clone(), wide.hi.clone());
            let touched: Vec<Fence> = parts
                .into_iter()
                .filter(|(lo, hi)| {
                    let below = hi.as_ref().is_some_and(|h| &wlo >= h);
                    let above = whi.as_ref().is_some_and(|h| h <= lo);
                    !below && !above
                })
                .collect();
            return self.start_compact(Some(touched));
        }
        // EVERY range that is over its bound, in one job -- not just the
        // worst. A per-range merge has to run once per range where the
        // whole-store merge ran once, so picking a single range per seal
        // starves it: with sixteen ranges and one merge in flight, pieces
        // accumulated faster than they were consumed and a read ended up
        // walking ten of them. That starvation cost more than the
        // whole-store rewrite it replaced (EXT.23 0.846x -> 0.561x), which
        // is the measurement that produced this loop.
        let due: Vec<Fence> = parts
            .into_iter()
            .filter(|f| {
                self.segs
                    .iter()
                    .filter(|s| s.level == 0 && s.lo == f.0 && s.hi == f.1)
                    .count()
                    >= self.opts.l0_trigger
            })
            .collect();
        if !due.is_empty() {
            return self.start_compact(Some(due));
        }
        Ok(())
    }

    fn l0_len(&self) -> usize {
        self.segs.iter().filter(|s| s.level == 0).count()
    }

    /// Name the live set durably. Everything before this call is a file on
    /// disk that nothing reaches; everything after it is the store.
    fn publish(&mut self) -> Result<()> {
        manifest_write(&self.dir, self.covered_seq, &self.live_names())
    }

    /// Merge the L0 tail and every partition it overlaps into a new
    /// disjoint set. Inputs stay live until `join_compact` publishes the
    /// outputs, so a reader during the merge sees the old set and a crash
    /// during it leaves the old set.
    /// `fence: None` is the initial partitioning -- everything live, split
    /// into partitions by size. `Some(range)` is the incremental merge: one
    /// partition and the pieces aligned to it, rewritten as one partition
    /// with the same fence. The second reads and writes O(range) where the
    /// first is O(store), which is what F43.4 and F44.1 both convicted.
    fn start_compact(&mut self, fences: Option<Vec<Fence>>) -> Result<()> {
        if let Some((_, h)) = &self.compacting {
            if !h.is_finished() {
                // A merge is still running. Deferring rather than blocking
                // keeps it off the commit path; the range it would have
                // merged waits for the next seal.
                return Ok(());
            }
            self.join_compact()?;
        }
        let inputs: Vec<String> = match &fences {
            None => self.live_names(),
            Some(fs) => self
                .segs
                .iter()
                .filter(|s| {
                    // Everything the output fences will cover: the
                    // partitions being rewritten, and any level-0 segment
                    // whose own range overlaps one of them.
                    fs.iter().any(|(lo, hi)| {
                        let below = hi.as_ref().is_some_and(|h| &s.lo >= h);
                        let above = s.hi.as_ref().is_some_and(|h| h <= lo);
                        !below && !above
                    })
                })
                .map(|s| s.name.clone())
                .collect(),
        };
        if inputs.is_empty() {
            return Ok(());
        }
        let parts = match &fences {
            Some(f) => f.len(),
            None => {
                let b: u64 = inputs
                    .iter()
                    .filter_map(|n| std::fs::metadata(self.dir.join(n)).ok())
                    .map(|m| m.len())
                    .sum();
                (b as usize).div_ceil(self.opts.seal_bytes.max(1)).max(1)
            }
        };
        let bytes: u64 = inputs
            .iter()
            .filter_map(|n| std::fs::metadata(self.dir.join(n)).ok())
            .map(|m| m.len())
            .sum();
        let live_keys: usize = self
            .segs
            .iter()
            .filter(|s| inputs.contains(&s.name))
            .map(|s| s.blob.keys())
            .sum();
        let per_key = (bytes as f64 / live_keys.max(1) as f64).max(1.0);
        let max_keys = ((self.opts.seal_bytes as f64 / per_key) as usize).max(1_000);
        let end_seq = self.covered_seq;
        let first_id = self.next_seg;
        // A split can turn one fence into several, so ids are reserved
        // generously; gaps in the sequence cost nothing.
        self.next_seg += (parts * 4).max(8) as u64;
        let dir = self.dir.clone();
        let opts = Db::segment_opts(&self.opts);
        let job_inputs = inputs.clone();
        let handle = std::thread::spawn(move || {
            compact_job(MergePlan {
                dir,
                inputs: job_inputs,
                first_id,
                end_seq,
                parts,
                fences,
                max_keys,
                opts,
            })
        });
        self.compacting = Some((inputs, handle));
        Ok(())
    }

    /// Collect a merge: swap its outputs in, name them in the manifest --
    /// the atomic instant -- and only then delete the inputs.
    fn join_compact(&mut self) -> Result<()> {
        let Some((inputs, handle)) = self.compacting.take() else {
            return Ok(());
        };
        let outputs = handle.join().map_err(|_| err("compaction thread panicked"))??;
        let mut kept: Vec<Seg> = Vec::new();
        for seg in self.segs.drain(..) {
            if !inputs.contains(&seg.name) {
                kept.push(seg);
            }
        }
        let mut merged = Vec::with_capacity(outputs.len());
        for name in &outputs {
            merged.push(Seg::open(&self.dir, name)?);
        }
        // Partitions first (older, disjoint), then whatever L0 arrived
        // while the merge ran, oldest to newest.
        merged.extend(kept);
        self.segs = merged;
        self.sort_segs();
        self.publish()?;
        for name in &inputs {
            let _ = std::fs::remove_file(self.dir.join(name));
        }
        Ok(())
    }

    /// Every value for `key`, in append order: partitions first, then L0
    /// oldest to newest, then the frozen memtable, then the live one.
    ///
    /// `may_hold` is the routing F38-F41 settled. A partition answers from
    /// its fence in two comparisons and no memory beyond the `Seg`; an L0
    /// segment answers from a Bloom in one cache line. Neither can produce
    /// a false negative, so a skipped segment is a segment that provably
    /// holds nothing for this key.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let mut n = 0u64;
        // Partitions sort first and are disjoint, so at most one can hold
        // the key and a binary search finds it: the promise F40/F41 bought
        // was two comparisons, not one per partition. Checking every fence
        // linearly would put the partition count back into the read cost,
        // which is the cost partitioning exists to remove.
        let np = self.segs.partition_point(|s| s.level > 0);
        let at = self.segs[..np]
            .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        if let Some(seg) = self.segs[..np].get(at) {
            if seg.may_hold(key) {
                n += seg
                    .blob
                    .read_all(key, &mut f)
                    .map_err(|e| err(&format!("segment read: {e}")))?;
            }
        }
        // L0 is walked linearly, not binary-searched. A search on the high
        // fence needs that fence to be monotone across the run, and one
        // leftover full-range piece (hi unbounded, sorting first) breaks
        // that -- the search then returns the start of the run and every
        // read walks the whole tail, which cost EXT.23 0.846x -> 0.561x
        // before the level dump showed a perfectly healthy 8 partitions and
        // 9 pieces. The tail is bounded by policy, and two comparisons
        // against a fence are cheap enough that bounded is enough.
        for seg in &self.segs[np..] {
            if !seg.may_hold(key) {
                continue;
            }
            n += seg
                .blob
                .read_all(key, &mut f)
                .map_err(|e| err(&format!("segment read: {e}")))?;
        }
        if let Some(fr) = &self.frozen {
            if let Some(e) = fr.get(key) {
                for off in fr.chain(e) {
                    f(fr.value_at(off));
                }
                n += e.count;
            }
        }
        if let Some(e) = self.mem.get(key) {
            for off in self.mem.chain(e) {
                f(self.mem.value_at(off));
            }
            n += e.count;
        }
        Ok(n)
    }

    /// Ordered scan from `from`, at most `limit` distinct keys, values in
    /// append order within each key. Milestone 3's merge is the unrouted fan
    /// on the scan axis: every segment contributes up to `limit` candidate
    /// keys via its own index walk, the memtables contribute theirs, and the
    /// union is sorted and re-read through `read_all`. Range-partitioned
    /// compaction is what will make this cost one segment instead of all of
    /// them; until then this is priced as what it is.
    pub fn scan<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        mut f: F,
    ) -> Result<usize> {
        let gen = self.wal.seq ^ (self.next_seg << 48) ^ ((self.frozen.is_some() as u64) << 63);
        {
            let mut cache = self.scan_keys.borrow_mut();
            let stale = cache.as_ref().is_none_or(|(g, _)| *g != gen);
            if stale {
                let mut all: Vec<Vec<u8>> = Vec::with_capacity(self.mem.len);
                let mut take = |mem: &MemTable| {
                    for e in mem.entries.iter().filter(|e| e.hash != 0) {
                        all.push(MemTable::key_of(&mem.keys, e).to_vec());
                    }
                };
                if let Some(fr) = &self.frozen {
                    take(fr);
                }
                take(&self.mem);
                all.sort_unstable();
                all.dedup();
                *cache = Some((gen, all));
            }
        }
        let cache = self.scan_keys.borrow();
        let unsealed = &cache.as_ref().expect("scan snapshot").1;
        let mut mi = unsealed.partition_point(|k| k.as_slice() < from);

        // A k-way merge over rank cursors, allocating nothing per key.
        //
        // The version before this one materialised every candidate key from
        // every source, sorted them and re-read each one: three copies and
        // a sort per key, which cost more than the reads. `Blob::key_at`
        // borrows out of the mapped index and the unsealed snapshot is
        // already sorted, so the merge can run on borrowed keys and emit
        // values straight from the position it is already holding.
        let mut cursors: Vec<(&Seg, usize)> = self
            .segs
            .iter()
            .filter(|s| s.may_reach(from))
            .map(|s| {
                let start = if s.lo.as_slice() > from { s.lo.as_slice() } else { from };
                (s, s.blob.seek(start))
            })
            .collect();

        let mut seen = 0usize;
        while seen < limit {
            // The next key is the smallest any source is holding.
            let mut next: Option<&[u8]> = None;
            for (seg, rank) in &cursors {
                if let Some(k) = seg.blob.key_at(*rank) {
                    if next.is_none_or(|n| k < n) {
                        next = Some(k);
                    }
                }
            }
            if let Some(k) = unsealed.get(mi) {
                if next.is_none_or(|n| k.as_slice() < n) {
                    next = Some(k.as_slice());
                }
            }
            let Some(key) = next else { break };

            // Emit in append order -- partitions, then L0 oldest to
            // newest, then the frozen memtable, then the live one -- and
            // advance every cursor that was sitting on this key.
            for (seg, rank) in cursors.iter_mut() {
                if seg.blob.key_at(*rank) == Some(key) {
                    seg.blob
                        .values_at(*rank, |v| f(key, v))
                        .map_err(|e| err(&format!("segment scan read: {e}")))?;
                    *rank += 1;
                }
            }
            if unsealed.get(mi).map(|k| k.as_slice()) == Some(key) {
                if let Some(fr) = &self.frozen {
                    if let Some(e) = fr.get(key) {
                        for off in fr.chain(e) {
                            f(key, fr.value_at(off));
                        }
                    }
                }
                if let Some(e) = self.mem.get(key) {
                    for off in self.mem.chain(e) {
                        f(key, self.mem.value_at(off));
                    }
                }
                mi += 1;
            }
            seen += 1;
        }
        Ok(seen)
    }

    pub fn segments(&self) -> usize {
        self.segs.len() + usize::from(self.sealing.is_some())
    }

    /// Live segment count by level: (partitioned, L0). The compaction
    /// experiment reports both, because "how many segments does a read
    /// touch" is the whole question.
    pub fn levels(&self) -> (usize, usize) {
        (self.segs.len() - self.l0_len(), self.l0_len())
    }

    /// Commit what is pending, seal the rest. Close is a convenience, not a
    /// durability point -- the WAL already made everything durable.
    pub fn close(mut self) -> Result<()> {
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<()> {
        self.flush()
    }
}

/// Dropping a `Db` without `close` emulates a crash in tests, but the seal
/// thread is not a crash casualty in-process: it would keep mutating the
/// directory under whoever reopens it. Join it; the WAL it rotated out
/// still covers everything either way, so crash semantics are unchanged.
impl Drop for Db {
    fn drop(&mut self) {
        if let Some(h) = self.sealing.take() {
            let _ = h.join();
        }
        if let Some((_, h)) = self.compacting.take() {
            let _ = h.join();
        }
    }
}
