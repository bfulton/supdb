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

use crate::block::{self, crc32, BlockBuilder, BlockLoc};
use crate::bytes::MmapBytes;
use crate::flatindex;
use crate::index::{Ext, Extents};
use crate::{Blob, Options};

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

/// When a commit reaches the device. The WAL is WRITTEN on every commit
/// under every policy; this decides only the barrier. `EveryN(n)` bounds
/// loss at n batches: on a crash, replay stops at the first frame that is
/// torn or missing and the sequence-gap check refuses anything past a
/// hole, so an unsynced tail is lost whole and never served in part.
///
/// It exists because f47 measured this device serving ~2,700 barriers a
/// second however they are issued -- sharding cannot scale past 1.6x --
/// so on a barrier-bound device the lever is fewer barriers per record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPolicy {
    /// One fdatasync per commit. Durable per batch, LMDB's boundary.
    Always,
    /// One fdatasync per `n` commits, and always at seal, flush and close.
    EveryN(u32),
}

/// I/O priority for the seal and merge threads. `Idle` asks the block layer
/// to serve everything else -- the commit path's barrier above all -- before
/// this thread's pages; f49 found the commit phase slowing whenever a seal
/// ran beside it, and f51 prices this as the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundIo {
    Normal,
    Idle,
}

/// Lower the calling thread's I/O priority to the idle class. Linux only;
/// elsewhere a no-op. A failure is ignored on purpose: a scheduler that does
/// not honour classes leaves the writes where they were, which is a fact
/// about the host that the measurement records rather than an error.
fn idle_io_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        // IOPRIO_WHO_PROCESS = 1, who = 0 is this thread, class IDLE = 3
        // sits in bits 13 and up of the priority value.
        let _ = libc::syscall(
            libc::SYS_ioprio_set,
            1 as libc::c_int,
            0 as libc::c_int,
            (3 << 13) as libc::c_int,
        );
    }
}

#[derive(Clone)]
pub struct NextOptions {
    pub sync: SyncPolicy,
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
    /// Whether `flush` partitions what it sealed before returning.
    ///
    /// This is a read-for-write trade and it is a large one. Partitioning
    /// makes every later read touch exactly one segment instead of paying
    /// a Bloom check on each of several overlapping ones -- worth roughly
    /// 1.4x on EXT.23 -- but it is a second full pass over everything just
    /// sealed, inside whatever window the caller is timing. A writer that
    /// is keeping up with ingest and reads later wants it off, and the
    /// background compaction will get there on its own schedule.
    pub partition_on_flush: bool,
    /// Write segments with `SegmentWriter` (the default) or through the
    /// general `Store` path it replaced. The general path is kept as the
    /// comparison arm because the rule for pricing an engine change is both
    /// arms interleaved in one process -- f49 does that -- and a number
    /// against an old run would mostly have been the machine.
    pub bulk_writer: bool,
    /// Find the keys a merge writes by a k-way walk of the inputs' rank
    /// order (the default) rather than by collecting, sorting and probing
    /// them. The probe path is kept as f49's comparison arm.
    pub cursor_merge: bool,
    /// I/O priority of the seal and merge threads (f51).
    pub background_io: BackgroundIo,
    /// Have the segment writer fdatasync every this many bytes as it
    /// streams blocks, so its dirty pages leave in slices rather than in
    /// one flush at the end. Zero syncs at the end only (f51).
    pub seal_sync_every: usize,
    /// Target bytes per partition: how many partitions the first
    /// partitioning cuts, and how many keys one holds before a merge splits
    /// it. `None` uses `seal_bytes`, which is how f52 found that smaller
    /// seals were also making more partitions and paying for them on every
    /// read; `Some` decouples the two.
    pub partition_bytes: Option<usize>,
}

impl Default for NextOptions {
    fn default() -> NextOptions {
        NextOptions {
            sync: SyncPolicy::Always,
            // 32 MB seals over 64 MB partitions: f52 measured 1.129x the
            // ingest of 64 MB seals at identical device bytes and identical
            // reads (F52.5, F52.6). Smaller still buys nothing and costs
            // 1.5x the device bytes.
            seal_bytes: 32 << 20,
            segment: Options::default(),
            l0_trigger: 4,
            compact: true,
            partition_on_flush: true,
            bulk_writer: true,
            cursor_merge: true,
            background_io: BackgroundIo::Normal,
            seal_sync_every: 0,
            partition_bytes: Some(64 << 20),
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

/// The WAL file starts with this, so a file from before frames carried a
/// kind byte is refused by name rather than replayed as something else.
const WAL_MAGIC: &[u8; 8] = b"SUPDBWL\x02";
/// Frame kinds. A batch is the frames between commit frames, and replay
/// applies a batch only once its commit frame has been read intact.
const WAL_PUT: u8 = 0;
const WAL_DEL: u8 = 1;
const WAL_COMMIT: u8 = 2;

impl Wal {
    fn create(path: &Path) -> Result<Wal> {
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
        file.write_all(WAL_MAGIC)?;
        Ok(Wal { file, path: path.to_path_buf(), seq: 0, pending: Vec::new() })
    }

    /// Reopen a WAL for appending at `seq`, after replay has truncated it to
    /// its last commit frame. A file that does not exist yet gets its header
    /// so the next replay finds one.
    fn open_append(path: &Path, seq: u64) -> Result<Wal> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        if file.metadata()?.len() == 0 {
            file.write_all(WAL_MAGIC)?;
        }
        Ok(Wal { file, path: path.to_path_buf(), seq, pending: Vec::new() })
    }

    /// One frame: `len u32 | crc u32 | seq u64 | kind u8 | payload`, where a
    /// put's payload is `klen uvarint | key | value`, a delete's is the key
    /// alone and a commit's is empty. `len` covers everything after `crc`;
    /// `crc` covers the same bytes.
    fn frame(&mut self, kind: u8, key: &[u8], value: &[u8]) {
        let body_at = self.pending.len() + FRAME_HEADER;
        self.pending.extend_from_slice(&[0u8; FRAME_HEADER]);
        self.pending.extend_from_slice(&self.seq.to_le_bytes());
        self.pending.push(kind);
        if kind != WAL_COMMIT {
            put_uvarint(&mut self.pending, key.len() as u64);
            self.pending.extend_from_slice(key);
            if kind == WAL_PUT {
                self.pending.extend_from_slice(value);
            }
        }
        let body_len = (self.pending.len() - body_at) as u32;
        let crc = crc32(&self.pending[body_at..]);
        self.pending[body_at - 8..body_at - 4].copy_from_slice(&body_len.to_le_bytes());
        self.pending[body_at - 4..body_at].copy_from_slice(&crc.to_le_bytes());
        self.seq += 1;
    }

    fn append(&mut self, key: &[u8], value: &[u8]) {
        self.frame(WAL_PUT, key, value);
    }

    fn delete(&mut self, key: &[u8]) {
        self.frame(WAL_DEL, key, &[]);
    }

    /// Close the batch: a commit frame after its records, so replay applies
    /// them all or none of them. Nothing pending, nothing to close.
    fn mark_commit(&mut self) {
        if !self.pending.is_empty() {
            self.frame(WAL_COMMIT, &[], &[]);
        }
    }

    fn commit(&mut self) -> Result<()> {
        self.mark_commit();
        self.write()?;
        self.sync()
    }

    fn write(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.pending)?;
        self.pending.clear();
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.file.sync_data()
    }

    /// Replay committed batches: `apply(kind, key, value)` for every record
    /// of every batch whose commit frame was read intact, in order. Frames
    /// after the last commit frame are a batch that never committed -- torn,
    /// or written and never synced -- and are not applied.
    ///
    /// Returns the next sequence number and the length of the file up to and
    /// including the last commit frame. The caller truncates the live WAL to
    /// that length before appending, because a partial batch left in place
    /// would sit in front of the next batch's commit frame and be adopted by
    /// it on the following replay.
    fn replay(
        path: &Path,
        from: u64,
        mut apply: impl FnMut(u8, &[u8], &[u8]),
    ) -> Result<(u64, u64)> {
        let mut buf = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut buf)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((from, 0)),
            Err(e) => return Err(e),
        }
        if buf.is_empty() {
            return Ok((from, 0));
        }
        if buf.len() < WAL_MAGIC.len() || &buf[..WAL_MAGIC.len()] != WAL_MAGIC {
            return Err(err("not a next-engine WAL: the header is missing or from an older format"));
        }
        let mut p = WAL_MAGIC.len();
        let mut next_seq = from;
        let mut committed_seq = from;
        let mut valid_len = p as u64;
        // The batch being read: kind, and where its key and value lie in
        // `buf`, so nothing is copied until the commit frame says to apply.
        let mut batch: Vec<(u8, usize, usize, usize)> = Vec::new();
        while buf.len() - p >= FRAME_HEADER {
            let len = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[p + 4..p + 8].try_into().unwrap());
            let body_at = p + FRAME_HEADER;
            let Some(end) = body_at.checked_add(len) else { break };
            if end > buf.len() || len < 9 {
                break;
            }
            let body = &buf[body_at..end];
            if crc32(body) != crc {
                break;
            }
            let seq = u64::from_le_bytes(body[..8].try_into().unwrap());
            let kind = body[8];
            if seq >= from {
                if seq != next_seq {
                    return Err(err("wal sequence gap: a durable record is missing"));
                }
                next_seq = seq + 1;
            }
            match kind {
                WAL_COMMIT => {
                    for &(k, ks, ke, ve) in &batch {
                        apply(k, &buf[ks..ke], &buf[ke..ve]);
                    }
                    batch.clear();
                    committed_seq = next_seq;
                    valid_len = end as u64;
                }
                WAL_PUT | WAL_DEL => {
                    let mut q = 9usize;
                    let Some(klen) = get_uvarint(body, &mut q) else {
                        return Err(err("wal frame key length is malformed"));
                    };
                    let kend = q
                        .checked_add(klen as usize)
                        .filter(|&e| e <= body.len())
                        .ok_or_else(|| err("wal frame key runs past its frame"))?;
                    if kind == WAL_DEL && kend != body.len() {
                        return Err(err("wal delete frame carries a value"));
                    }
                    if seq >= from {
                        batch.push((kind, body_at + q, body_at + kend, end));
                    }
                }
                _ => return Err(err("wal frame kind is unknown")),
            }
            p = end;
        }
        // Whatever `batch` still holds never committed: lost whole.
        Ok((committed_seq, valid_len))
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
    /// Whether any extent here carries the tombstone flag. A read consults
    /// it before paying the newest-first pass that tombstones require;
    /// partitions are always false, because a merge writes the bottom level
    /// and drops them.
    tombs: bool,
}

// ------------------------------------------------------- the segment writer --

/// Writes an immutable segment in one forward pass, for input that arrives
/// sorted by key with each key's values together.
///
/// `Store` is a general writer: a hash table to find keys again, a freelist
/// to place blocks, a pending arena, a reuse log, and a checkpoint that
/// publishes all of it. A seal and a merge need none of that -- their keys
/// come sorted, each key's values come once and together, and nothing is
/// ever read back or appended to -- and f46 priced the general path at
/// 2.04x the floor for exactly that input (F46.1). This is the writer that
/// floor described: values are packed into blocks in the order they arrive,
/// each key gets one extent, and the end of the pass writes the block table,
/// the key section and both superblock slots. It emits the format `Store`
/// writes and `Blob` reads, and `tests/segwriter.rs` holds the two writers
/// to agreement on every read, `store::Reader` included.
///
/// A second writer of a format is a liability of the same kind as a second
/// reader: its failure mode is a file that opens and answers differently.
/// So the superblock is not re-derived here but copied field for field from
/// `store::Super::encode`, the record encoding is `index::put_uvarint`
/// because that is the reader's inverse, and the test corrupts a block to
/// prove the checksum recorded is the one the reader checks.
pub struct SegmentWriter {
    out: std::io::BufWriter<File>,
    /// File offset the next block lands at. Data starts after the header
    /// region, which holds the two superblock slots and is written last.
    pos: u64,
    builder: BlockBuilder,
    block_size: usize,
    blocks: Vec<BlockLoc>,
    /// Every key written, concatenated, with each key's span. Flat rather
    /// than a `Vec<Vec<u8>>` because a segment has a million keys and the
    /// index build wants them all at once; the extent beside each span is
    /// the one record `flatindex::encode` reads.
    key_arena: Vec<u8>,
    spans: Vec<(usize, usize)>,
    exts: Vec<Extents>,
    /// The key currently open, its run of length-prefixed records, and the
    /// offset of the newest record's prefix inside the run -- what
    /// `Ext::last` carries so that reading the newest value is O(1).
    open_key: Option<(usize, usize)>,
    run: Vec<u8>,
    last: usize,
    records: u32,
    parallel_index: bool,
    /// fdatasync every this many block bytes; zero for the end only.
    sync_every: u64,
    since_sync: u64,
}

/// One superblock slot, field for field what `store::Super::encode` writes:
/// sixteen little-endian u64 fields, the magic in native order as the
/// byte-order mark, and the FNV-1a of the fields and the magic.
fn superblock(fields: &[u64; 16]) -> [u8; crate::store::SUPER_BYTES] {
    let mut out = [0u8; crate::store::SUPER_BYTES];
    for (i, v) in fields.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    out[128..136].copy_from_slice(&crate::store::MAGIC.to_ne_bytes());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in fields.iter().chain(std::iter::once(&crate::store::MAGIC)) {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    }
    out[136..144].copy_from_slice(&h.to_le_bytes());
    out
}

impl SegmentWriter {
    /// Open `path` for a fresh segment. `opts` supplies the block size, the
    /// checksum switch and whether the index build may use threads; the
    /// rest of `Options` describes machinery this writer does not have.
    pub fn create(path: &Path, opts: &Options) -> Result<SegmentWriter> {
        // The checksum switch is process-wide and `Store::create` sets it
        // from the same option; a writer that recorded none while readers
        // expected them would fail every block it wrote.
        block::CHECKSUMS.store(opts.checksums, std::sync::atomic::Ordering::Relaxed);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
        // The header region stays zero until `finish`, so a segment that
        // was never finished is a file no reader accepts rather than a
        // segment with some of its keys.
        out.write_all(&[0u8; crate::store::SUPER as usize])?;
        let block_size = opts.block_size.max(1);
        Ok(SegmentWriter {
            out,
            pos: crate::store::SUPER,
            builder: BlockBuilder::new(block_size),
            block_size,
            blocks: Vec::new(),
            key_arena: Vec::new(),
            spans: Vec::new(),
            exts: Vec::new(),
            open_key: None,
            run: Vec::new(),
            last: 0,
            records: 0,
            parallel_index: opts.parallel_index,
            sync_every: 0,
            since_sync: 0,
        })
    }

    /// Spread the writer's syncs: fdatasync every `bytes` of blocks written
    /// instead of once at `finish`. Zero restores the single sync.
    pub fn set_sync_every(&mut self, bytes: usize) {
        self.sync_every = bytes as u64;
    }

    /// Start a key. Keys must arrive in strictly increasing order; the
    /// writer refuses anything else rather than build an index whose
    /// directory disagrees with its records.
    pub fn begin(&mut self, key: &[u8]) -> Result<()> {
        if self.open_key.is_some() {
            return Err(err("segment writer: begin while a key is open"));
        }
        if key.len() > u16::MAX as usize {
            return Err(err("segment writer: key longer than 65,535 bytes"));
        }
        if let Some(&(s, l)) = self.spans.last() {
            if key <= &self.key_arena[s..s + l] {
                return Err(err("segment writer: keys must arrive in strictly increasing order"));
            }
        }
        let start = self.key_arena.len();
        self.key_arena.extend_from_slice(key);
        self.open_key = Some((start, key.len()));
        self.run.clear();
        self.last = 0;
        self.records = 0;
        Ok(())
    }

    /// One value of the open key, in append order.
    pub fn value(&mut self, v: &[u8]) {
        debug_assert!(self.open_key.is_some(), "value without begin");
        self.last = self.run.len();
        self.records += 1;
        crate::index::put_uvarint(&mut self.run, v.len() as u64);
        self.run.extend_from_slice(v);
    }

    /// Close the open key: place its run in a block and record the extent.
    pub fn end(&mut self) -> Result<()> {
        self.end_with(false)
    }

    /// `end`, with the extent flagged as a tombstone: this run supersedes
    /// every older value of the key, in every older segment.
    pub fn end_with(&mut self, tombstone: bool) -> Result<()> {
        let (start, len) = self
            .open_key
            .take()
            .ok_or_else(|| err("segment writer: end without begin"))?;
        let n = self.run.len();
        if n > u32::MAX as usize {
            return Err(err("segment writer: a key's values exceed 4 GiB in one segment"));
        }
        // A run that does not fit beside what is staged starts a new block;
        // a run larger than a whole block takes an empty builder and is a
        // block by itself, so a key's values stay contiguous -- the same
        // rule `Store` applies through the same `BlockBuilder`.
        if self.builder.would_overflow(n) {
            self.flush_block()?;
        }
        let off = self.builder.push(&self.run);
        let ext = Ext {
            block: self.blocks.len() as u32,
            off,
            len: n as u32,
            last: self.last as u32,
            count: self.records | if tombstone { Ext::TOMBSTONE } else { 0 },
        };
        if self.builder.len() >= self.block_size {
            self.flush_block()?;
        }
        self.spans.push((start, len));
        self.exts.push(Extents::One(ext));
        Ok(())
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.builder.is_empty() {
            return Ok(());
        }
        let bytes = self.builder.take();
        let len = bytes.len() as u32;
        let crc = if block::checksums_on() { crc32(&bytes) } else { 0 };
        self.blocks.push(BlockLoc {
            off: self.pos,
            stored: len,
            uncompressed: len,
            cap: len,
            chunked: false,
            solo: false,
            chunk_crc: false,
            crc,
        });
        self.out.write_all(&bytes)?;
        self.pos += bytes.len() as u64;
        if self.sync_every > 0 {
            self.since_sync += bytes.len() as u64;
            if self.since_sync >= self.sync_every {
                self.out.flush()?;
                self.out.get_ref().sync_data()?;
                self.since_sync = 0;
            }
        }
        Ok(())
    }

    /// Sections are aligned in the FILE, not just within themselves: the
    /// index hands back `&[Ext]` borrowed from the mapping at its absolute
    /// address, and `store::write_section_raw` carries the story of the
    /// lookups that returned nothing when that was forgotten.
    fn pad_to(&mut self, align: u64) -> Result<()> {
        let rem = self.pos % align;
        if rem != 0 {
            let pad = (align - rem) as usize;
            self.out.write_all(&vec![0u8; pad])?;
            self.pos += pad as u64;
        }
        Ok(())
    }

    /// Keys written so far.
    pub fn keys(&self) -> usize {
        self.spans.len()
    }

    /// Write the block table, the key section and the superblock, and
    /// fsync. `generation` is what the segment reports as its checkpoint
    /// identity; a segment is written once, so 1 is the usual answer.
    pub fn finish(mut self, generation: u64) -> Result<()> {
        if self.open_key.is_some() {
            return Err(err("segment writer: finish with a key still open"));
        }
        // A segment with no keys is allowed: a partition whose every key was
        // deleted still has to exist, or the fences stop tiling the key
        // space and a later seal would route keys into a neighbour's range.
        self.flush_block()?;

        let rows = vec![[0u32; block::MAX_CHUNK_CRCS]; self.blocks.len()];
        let table = flatindex::encode_blocks(&self.blocks, &rows);
        self.pad_to(8)?;
        let blk_off = self.pos;
        self.out.write_all(&table)?;
        self.pos += table.len() as u64;

        let (section, reserve) = {
            let all: Vec<(&[u8], &Extents)> = self
                .spans
                .iter()
                .zip(&self.exts)
                .map(|(&(s, l), e)| (&self.key_arena[s..s + l], e))
                .collect();
            flatindex::encode(
                &all,
                generation,
                None,
                flatindex::key_hash,
                0,
                self.parallel_index,
            )
            .ok_or_else(|| err("segment writer: key section exceeds the flat index's limits"))?
        };
        self.pad_to(8)?;
        let key_off = self.pos;
        self.out.write_all(&section)?;
        let key_len = reserve.max(section.len());
        if key_len > section.len() {
            self.out.write_all(&vec![0u8; key_len - section.len()])?;
        }
        self.pos += key_len as u64;

        let file = self.out.into_inner().map_err(|e| e.into_error())?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // generation, history_from, timestamp, key section (off, stored,
        // uncompressed), block table (same three), reuse log (none),
        // high_water, redo log (none), index_gen.
        let fields: [u64; 16] = [
            generation,
            generation,
            ts,
            key_off,
            key_len as u64,
            key_len as u64,
            blk_off,
            table.len() as u64,
            table.len() as u64,
            0,
            0,
            0,
            self.pos,
            0,
            0,
            generation,
        ];
        let sb = superblock(&fields);
        use std::os::unix::fs::FileExt;
        file.write_all_at(&sb, 0)?;
        file.write_all_at(&sb, crate::store::SLOT)?;
        file.sync_all()?;
        Ok(())
    }
}

/// The two ways a piece gets written. `Bulk` is the shipping path; `General`
/// is the `Store` path it replaced, kept behind `NextOptions::bulk_writer` as
/// the comparison arm f49 interleaves against it. Both see the same calls in
/// the same order, so the only thing that differs is the writer.
enum PieceWriter {
    Bulk(Box<SegmentWriter>),
    General {
        store: Box<crate::Store>,
        key: Vec<u8>,
        failed: Option<std::io::Error>,
    },
}

impl PieceWriter {
    fn create(path: &Path, opts: &Options, bulk: bool, sync_every: usize) -> Result<PieceWriter> {
        if bulk {
            let mut w = SegmentWriter::create(path, opts)?;
            w.set_sync_every(sync_every);
            Ok(PieceWriter::Bulk(Box::new(w)))
        } else {
            Ok(PieceWriter::General {
                store: Box::new(crate::Store::create(path, opts.clone())?),
                key: Vec::new(),
                failed: None,
            })
        }
    }

    fn begin(&mut self, k: &[u8]) -> Result<()> {
        match self {
            PieceWriter::Bulk(w) => w.begin(k),
            PieceWriter::General { key, .. } => {
                key.clear();
                key.extend_from_slice(k);
                Ok(())
            }
        }
    }

    /// Infallible at the call so it can sit inside a read callback; the
    /// general path parks a failure and `end` reports it.
    fn value(&mut self, v: &[u8]) {
        match self {
            PieceWriter::Bulk(w) => w.value(v),
            PieceWriter::General { store, key, failed } => {
                if failed.is_none() {
                    if let Err(e) = store.append(key, v) {
                        *failed = Some(e);
                    }
                }
            }
        }
    }

    fn end_with(&mut self, tombstone: bool) -> Result<()> {
        match self {
            PieceWriter::Bulk(w) => w.end_with(tombstone),
            PieceWriter::General { failed, .. } => {
                if tombstone {
                    return Err(err("the general writer cannot express a delete"));
                }
                match failed.take() {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
        }
    }

    fn finish(self) -> Result<()> {
        match self {
            PieceWriter::Bulk(w) => (*w).finish(1),
            PieceWriter::General { store, .. } => {
                store.checkpoint()?;
                store.close()?;
                Ok(())
            }
        }
    }
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
        let src = MmapBytes::open(&dir.join(name)).map_err(|e| {
            // A manifest naming a segment that is not on disk is a damaged
            // store, not a missing file, and saying so is the difference
            // between a diagnosis and an ENOENT.
            err(&format!("the manifest names segment {name}, which is not in the store: {e}"))
        })?;
        let blob = Blob::open(src).map_err(|e| err(&format!("segment {name}: {e}")))?;
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
            let (bloom, tombs) = Seg::bloom_and_tombs(&blob)?;
            return Ok(Seg {
                blob,
                name: name.to_string(),
                level: 0,
                lo,
                hi,
                bloom: Some(bloom),
                tombs,
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
            return Ok(Seg {
                blob,
                name: name.to_string(),
                level: 1,
                lo,
                hi,
                bloom: None,
                tombs: false,
            });
        }
        // L0: build the Bloom by walking the segment's keys. That walk is
        // O(keys) and it is affordable for exactly one reason -- L0 is
        // bounded at `l0_trigger` segments of at most `seal_bytes` each, so
        // this cost is bounded where the level below it is not.
        let (bloom, tombs) = Seg::bloom_and_tombs(&blob)?;
        Ok(Seg {
            blob,
            name: name.to_string(),
            level: 0,
            lo: Vec::new(),
            hi: None,
            bloom: Some(bloom),
            tombs,
        })
    }

    /// The Bloom for a level-0 piece and whether any of its extents carries
    /// the tombstone flag: one walk of the key section for both, which a
    /// piece pays for the Bloom anyway.
    fn bloom_and_tombs(blob: &Blob<MmapBytes>) -> Result<(BlockedBloom, bool)> {
        let mut bloom = BlockedBloom::with_capacity(blob.keys());
        let mut tombs = false;
        for rank in 0..blob.keys() {
            let (k, exts) = blob
                .exts_at(rank)
                .ok_or_else(|| err("segment key walk: a rank the index does not have"))?;
            bloom.insert(k);
            tombs |= exts.iter().any(|e| e.is_tombstone());
        }
        Ok((bloom, tombs))
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
    /// Tombstone chunks pushed so far. Non-zero is what tells a read that
    /// this memtable can end a key's older values; zero lets it skip the
    /// check entirely.
    tombs: usize,
}

/// A chain chunk whose length prefix is this is a tombstone: it holds no
/// value, and nothing older than it -- in this chain or in any older
/// source -- is live.
const TOMB_LEN: u64 = u64::MAX;

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
            tombs: 0,
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
    /// End every value of `key` before this point: a tombstone chunk at the
    /// head of the chain, and the live count back to zero. A key never seen
    /// before gets an entry too, because the tombstone has older sources to
    /// mask even when this memtable holds nothing of its own.
    fn delete(&mut self, key: &[u8]) {
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
                let head = self.push_tomb(NO_CHUNK);
                self.entries[i] = MemEntry {
                    hash,
                    key_off,
                    key_len: key.len() as u32,
                    head: head + 1,
                    count: 0,
                };
                self.len += 1;
                self.tombs += 1;
                return;
            }
            if e.hash == hash && MemTable::key_of(&self.keys, &e) == key {
                let head = self.push_tomb(e.head - 1);
                self.entries[i].head = head + 1;
                self.entries[i].count = 0;
                self.tombs += 1;
                return;
            }
            i = (i + 1) & self.mask;
        }
    }

    fn push_tomb(&mut self, prev: u64) -> u64 {
        let off = self.vals.len() as u64;
        self.vals.extend_from_slice(&prev.to_le_bytes());
        put_uvarint(&mut self.vals, TOMB_LEN);
        off
    }

    fn is_tomb(&self, at: usize) -> bool {
        let mut p = at + 8;
        get_uvarint(&self.vals, &mut p) == Some(TOMB_LEN)
    }

    /// The key's live values, oldest first, and whether a tombstone ends
    /// the chain -- in which case everything older, here and in every older
    /// source, is dead.
    fn live_chain(&self, e: &MemEntry) -> (Vec<usize>, bool) {
        let mut offs = Vec::with_capacity(e.count as usize);
        let mut at = e.head - 1;
        while at != NO_CHUNK {
            if self.is_tomb(at as usize) {
                offs.reverse();
                return (offs, true);
            }
            offs.push(at as usize);
            at = u64::from_le_bytes(self.vals[at as usize..at as usize + 8].try_into().unwrap());
        }
        offs.reverse();
        (offs, false)
    }

    /// Whether a tombstone sits anywhere in the key's chain.
    fn has_tomb(&self, e: &MemEntry) -> bool {
        let mut at = e.head - 1;
        while at != NO_CHUNK {
            if self.is_tomb(at as usize) {
                return true;
            }
            at = u64::from_le_bytes(self.vals[at as usize..at as usize + 8].try_into().unwrap());
        }
        false
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
    bulk: bool,
    cursors: bool,
    background_io: BackgroundIo,
    sync_every: usize,
}

fn compact_job(plan: MergePlan) -> Result<Vec<String>> {
    compact_run(plan)
}

/// Distinct keys of a merge, in order, in one allocation -- pass one of the
/// merge. The slicing addresses keys by rank exactly as the sorted vector
/// this replaced did, without a million allocations, a sort or a dedup.
struct KeyList {
    bytes: Vec<u8>,
    offs: Vec<usize>,
}

impl KeyList {
    fn new() -> KeyList {
        KeyList { bytes: Vec::new(), offs: vec![0] }
    }

    fn push(&mut self, k: &[u8]) {
        self.bytes.extend_from_slice(k);
        self.offs.push(self.bytes.len());
    }

    fn len(&self) -> usize {
        self.offs.len() - 1
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, i: usize) -> &[u8] {
        &self.bytes[self.offs[i]..self.offs[i + 1]]
    }

    /// First rank whose key fails `pred`, for a `pred` that is true on a
    /// prefix of the list.
    fn partition_point(&self, pred: impl Fn(&[u8]) -> bool) -> usize {
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(self.get(mid)) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }
}

/// K-way walk of the inputs in rank order: `f` sees each distinct key once,
/// with `(input, rank)` for every input that holds it, oldest input first.
/// Inputs are ordered oldest to newest, so draining those cursors in the
/// order given returns a key's values in append order -- the order the probe
/// path got by asking each input in turn. Each input's key section is read
/// forwards, once, and nothing is hashed.
fn merge_ranks(
    blobs: &[Blob<MmapBytes>],
    mut f: impl FnMut(&[u8], &[(usize, usize)]) -> Result<()>,
) -> Result<()> {
    let n: Vec<usize> = blobs.iter().map(|b| b.keys()).collect();
    let mut rank = vec![0usize; blobs.len()];
    let mut tied: Vec<(usize, usize)> = Vec::with_capacity(blobs.len());
    loop {
        let mut min: Option<&[u8]> = None;
        tied.clear();
        for (i, b) in blobs.iter().enumerate() {
            if rank[i] >= n[i] {
                continue;
            }
            let k = b
                .key_at(rank[i])
                .ok_or_else(|| err("segment key walk: a rank the index does not have"))?;
            match min {
                None => {
                    min = Some(k);
                    tied.push((i, rank[i]));
                }
                Some(m) => match k.cmp(m) {
                    std::cmp::Ordering::Less => {
                        min = Some(k);
                        tied.clear();
                        tied.push((i, rank[i]));
                    }
                    std::cmp::Ordering::Equal => tied.push((i, rank[i])),
                    std::cmp::Ordering::Greater => {}
                },
            }
        }
        let Some(k) = min else {
            return Ok(());
        };
        f(k, &tied)?;
        for &(i, _) in &tied {
            rank[i] += 1;
        }
    }
}

/// One output of a merge: the ranks it holds, the fence it must contain,
/// and its names on disk.
struct Piece {
    from: usize,
    to: usize,
    lo: Vec<u8>,
    hi: Option<Vec<u8>>,
    name: String,
    tmp: PathBuf,
}

/// Pass two of a merge: keys arrive in rank order and go into the piece
/// their rank belongs to, with a writer opened at each piece's first rank
/// and finished and renamed at its last. The same
/// emitter serves both ways of finding keys, so the arms f49 compares
/// differ only in that.
struct Emitter<'a> {
    dir: &'a Path,
    opts: &'a Options,
    bulk: bool,
    sync_every: usize,
    pieces: Vec<Piece>,
    pi: usize,
    r: usize,
    w: Option<PieceWriter>,
    out: Vec<String>,
}

impl Emitter<'_> {
    /// Validate that the visited rank belongs to the current piece and that
    /// `k`, if given, lies inside its fence; open the piece's writer at its
    /// first rank. Returns the piece's last rank.
    fn enter(&mut self, k: Option<&[u8]>) -> Result<usize> {
        let p = self
            .pieces
            .get(self.pi)
            .ok_or_else(|| err("merge visited a key past its last piece"))?;
        // The slices tile the ranks; a key that belongs to no piece is a
        // key that would have been dropped, silently, on the way to disk.
        if self.r < p.from || self.r >= p.to {
            return Err(err("merge slices leave a key unassigned"));
        }
        // Insurance against the class of bug that produced this line: a
        // merge told to write a fence must contain what it writes, or the
        // read path will deny it and no test will say so.
        if let Some(k) = k {
            if k < p.lo.as_slice() || p.hi.as_ref().is_some_and(|h| k >= h.as_slice()) {
                return Err(err("compaction would write a key outside its fence"));
            }
        }
        let (from, to, tmp) = (p.from, p.to, p.tmp.clone());
        if self.r == from {
            let _ = std::fs::remove_file(&tmp);
            self.w = Some(
                PieceWriter::create(&tmp, self.opts, self.bulk, self.sync_every)
                    .map_err(|e| err(&format!("compact create: {e}")))?,
            );
        }
        Ok(to)
    }

    /// Advance past the visited rank; finish, rename and publish the piece
    /// at its last one.
    fn leave(&mut self, to: usize) -> Result<()> {
        self.r += 1;
        if self.r == to {
            let w = self.w.take().ok_or_else(|| err("merge piece not open"))?;
            w.finish().map_err(|e| err(&format!("compact finish: {e}")))?;
            let p = &self.pieces[self.pi];
            std::fs::rename(&p.tmp, self.dir.join(&p.name))?;
            self.out.push(p.name.clone());
            self.pi += 1;
        }
        Ok(())
    }

    fn key(&mut self, k: &[u8], pull: impl FnOnce(&mut PieceWriter) -> Result<()>) -> Result<()> {
        let to = self.enter(Some(k))?;
        let w = self.w.as_mut().ok_or_else(|| err("merge piece not open"))?;
        w.begin(k)?;
        pull(w)?;
        // Merges write the bottom level, so no output extent carries the
        // tombstone flag: there is nothing older left for it to mask.
        w.end_with(false)?;
        self.leave(to)
    }

    /// A rank whose key has nothing live. It still belongs to a piece, and
    /// the piece is still opened and finished around it, so the fence
    /// tiling survives even a partition whose every key was deleted.
    fn skip(&mut self) -> Result<()> {
        let to = self.enter(None)?;
        self.leave(to)
    }

    fn finish(self, total: usize) -> Result<Vec<String>> {
        if self.r != total || self.pi != self.pieces.len() {
            return Err(err("merge ended with a piece still open"));
        }
        Ok(self.out)
    }
}

fn compact_run(plan: MergePlan) -> Result<Vec<String>> {
    let MergePlan {
        dir,
        inputs,
        first_id,
        end_seq,
        parts,
        fences,
        max_keys,
        opts,
        bulk,
        cursors,
        background_io,
        sync_every,
    } = plan;
    if background_io == BackgroundIo::Idle {
        idle_io_priority();
    }
    let mut blobs = Vec::with_capacity(inputs.len());
    for name in &inputs {
        blobs.push(
            Blob::open(MmapBytes::open(&dir.join(name))?)
                .map_err(|e| err(&format!("compact input {name}: {e}")))?,
        );
    }

    // Pass one: every distinct key once, in order.
    let mut keys = KeyList::new();
    if cursors {
        merge_ranks(&blobs, |k, _| {
            keys.push(k);
            Ok(())
        })?;
    } else {
        let mut all: Vec<Vec<u8>> = Vec::new();
        for b in &blobs {
            Seg::for_each_key(b, |k| all.push(k.to_vec()))?;
        }
        all.sort_unstable();
        all.dedup();
        for k in &all {
            keys.push(k);
        }
    }
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let parts = match &fences {
        Some(f) => f.len().max(1),
        None => parts.max(1).min(keys.len()),
    };
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
            let b = fence_lo(keys.get((i * per).min(keys.len() - 1)));
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
                let from = keys.partition_point(|k| k < lo.as_slice());
                let to = match hi {
                    Some(h) => keys.partition_point(|k| k < h.as_slice()),
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
                    let sub_lo = if i == 0 { lo.clone() } else { fence_lo(keys.get(sf)) };
                    let sub_hi = if st == to { hi.clone() } else { Some(fence_lo(keys.get(st))) };
                    slices.push((sf, st));
                    given.push((sub_lo, sub_hi));
                }
            }
        }
        None => {
            let mut at = 0usize;
            for b in &bounds {
                let end = keys.partition_point(|k| k < b.as_slice());
                if end > at {
                    slices.push((at, end));
                    at = end;
                }
            }
            slices.push((at, keys.len()));
        }
    }

    let mut pieces = Vec::with_capacity(slices.len());
    for (pi, &(from, to)) in slices.iter().enumerate() {
        if from >= to {
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
        pieces.push(Piece { from, to, lo, hi, name, tmp });
    }

    // Pass two: values, in rank order, into one piece per slice.
    let mut em = Emitter {
        dir: &dir,
        opts: &opts,
        bulk,
        sync_every,
        pieces,
        pi: 0,
        r: 0,
        w: None,
        out: Vec::new(),
    };
    // Tombstones end here. Every merge writes the bottom level, so for each
    // key the inputs older than its newest flagged extent are dropped, the
    // flag itself is not carried, and a key with nothing live is left out
    // -- which is how a delete gets its bytes back.
    if cursors {
        merge_ranks(&blobs, |k, tied| {
            let mut start = 0usize;
            let mut live = 0u64;
            for (j, &(i, rank)) in tied.iter().enumerate() {
                let Some((_, exts)) = blobs[i].exts_at(rank) else {
                    return Err(err("segment key walk: a rank the index does not have"));
                };
                if exts.iter().any(|e| e.is_tombstone()) {
                    start = j;
                    live = 0;
                }
                live += exts.iter().map(|e| u64::from(e.records())).sum::<u64>();
            }
            if live == 0 {
                return em.skip();
            }
            em.key(k, |w| {
                for &(i, rank) in &tied[start..] {
                    blobs[i]
                        .values_at(rank, |v| w.value(v))
                        .map_err(|e| err(&format!("compact read: {e}")))?;
                }
                Ok(())
            })
        })?;
    } else {
        for r in 0..keys.len() {
            let k = keys.get(r);
            let mut found: Vec<(usize, &[Ext])> = Vec::with_capacity(blobs.len());
            let mut start = 0usize;
            let mut live = 0u64;
            for (i, b) in blobs.iter().enumerate() {
                if let Some(exts) = b.lookup(k) {
                    if exts.iter().any(|e| e.is_tombstone()) {
                        start = found.len();
                        live = 0;
                    }
                    live += exts.iter().map(|e| u64::from(e.records())).sum::<u64>();
                    found.push((i, exts));
                }
            }
            if live == 0 {
                em.skip()?;
                continue;
            }
            em.key(k, |w| {
                for &(i, exts) in &found[start..] {
                    blobs[i]
                        .read_exts(exts, |v| w.value(v))
                        .map_err(|e| err(&format!("compact read: {e}")))?;
                }
                Ok(())
            })?;
        }
    }
    let out = em.finish(keys.len())?;
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
    /// Commits written since the last barrier, for `SyncPolicy::EveryN`.
    unsynced: u32,
    /// Nanoseconds spent in each phase of a load, accumulated so an
    /// experiment can attribute the durable-load cost instead of inferring
    /// it. `commit` is the WAL append and its fdatasync -- the only work on
    /// the commit path; `seal` is writing a memtable out as a segment;
    /// `merge` is compaction, counted where the caller waits for it.
    phase_ns: [u64; 3],
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
            unsynced: 0,
            phase_ns: [0; 3],
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
        let mut valid_len = 0u64;
        for &id in &wal_ids {
            let (next, valid) = Wal::replay(&Db::wal_path(dir, id), from, |kind, k, v| {
                if kind == WAL_DEL {
                    mem.delete(k);
                    mem_bytes += k.len() + 16;
                } else {
                    mem.append(k, v);
                    mem_bytes += k.len() + v.len();
                }
            })?;
            from = next;
            valid_len = valid;
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
        // A batch that never committed is cut off before anything is
        // appended behind it: left in place, its records would sit in front
        // of the next batch's commit frame and be adopted by it.
        if valid_len > 0 {
            let f = OpenOptions::new().write(true).open(&wal_path)?;
            if f.metadata()?.len() > valid_len {
                f.set_len(valid_len)?;
                f.sync_data()?;
            }
        }
        let wal = Wal::open_append(&wal_path, from)?;
        let next_seg = seg_ids.iter().map(|&(n, _)| n + 1).max().unwrap_or(0);
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal,
            wal_id,
            mem,
            mem_bytes,
            segs,
            next_seg,
            frozen: None,
            sealing: None,
            compacting: None,
            unsynced: 0,
            phase_ns: [0; 3],
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

    /// End every value of `key` written before this point; later appends
    /// start fresh. Durable at the next `commit`, exactly like an append,
    /// and reclaimed by the next merge that reaches the key.
    pub fn delete(&mut self, key: &[u8]) {
        self.wal.delete(key);
        self.mem.delete(key);
        self.mem_bytes += key.len() + 16;
    }

    /// Start a transaction: puts and deletes staged until `Txn::commit`
    /// applies them as one batch. It borrows the store mutably, so no other
    /// write can interleave with it and no read can observe it half-applied.
    pub fn begin(&mut self) -> Txn<'_> {
        Txn { db: self, ops: Vec::new() }
    }

    /// Whether any source can end a key's older values. False for a store
    /// nothing was ever deleted from, which lets every read skip the
    /// newest-first pass tombstones require.
    fn has_tombstones(&self) -> bool {
        self.mem.tombs > 0
            || self.frozen.as_ref().is_some_and(|f| f.tombs > 0)
            || self.segs.iter().any(|s| s.tombs)
    }

    /// The durability point: WAL append + fdatasync. If the memtable has
    /// crossed the seal threshold, seal after the commit -- after, so the
    /// batch's durability never waits on a segment write.
    pub fn commit(&mut self) -> Result<()> {
        let t = std::time::Instant::now();
        self.wal.mark_commit();
        self.wal.write()?;
        self.unsynced += 1;
        let due = match self.opts.sync {
            SyncPolicy::Always => true,
            SyncPolicy::EveryN(n) => self.unsynced >= n.max(1),
        };
        if due {
            self.wal.sync()?;
            self.unsynced = 0;
        }
        self.phase_ns[0] += t.elapsed().as_nanos() as u64;
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
        self.unsynced = 0;
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
        let bulk = self.opts.bulk_writer;
        let background_io = self.opts.background_io;
        let sync_every = self.opts.seal_sync_every;
        let end_seq = old_wal.seq;
        self.retiring_wals.push(old_wal.path.clone());
        drop(old_wal);
        let mem = frozen.clone();
        self.frozen = Some(frozen);
        self.sealing = Some(std::thread::spawn(move || {
            if background_io == BackgroundIo::Idle {
                idle_io_priority();
            }
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
                    let mut w = PieceWriter::create(&tmp, &opts, bulk, sync_every)
                        .map_err(|e| err(&format!("seal create: {e}")))?;
                    for e in &order[start..at] {
                        let key = MemTable::key_of(&mem.keys, e);
                        // Only what is live after the newest tombstone, and
                        // the flag if there was one: the segment carries the
                        // delete forward for the sources older than it.
                        let (offs, tomb) = mem.live_chain(e);
                        w.begin(key)?;
                        for off in offs {
                            w.value(mem.value_at(off));
                        }
                        w.end_with(tomb)?;
                    }
                    w.finish().map_err(|e| err(&format!("seal finish: {e}")))?;
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
        self.unsynced = 0;
        self.seal()?;
        self.join_seal()?;
        self.join_compact()?;
        // Leave the store routed. A flush is a caller saying it has
        // stopped writing, and what it leaves behind otherwise is a set of
        // OVERLAPPING full-range segments -- each one costing every
        // subsequent read a Bloom check, because nothing tells them apart.
        // Partitioning them costs one merge now and makes every later read
        // touch exactly one segment, which is the arrangement the read
        // lead was measured in.
        if self.opts.compact
            && self.opts.partition_on_flush
            && self.segs.iter().any(|s| s.level == 0)
        {
            self.start_compact(None)?;
            self.join_compact()?;
        }
        Ok(())
    }

    /// Collect a finished (or in-flight) seal: join the thread, open its
    /// segment, retire the frozen memtable.
    fn join_seal(&mut self) -> Result<()> {
        let Some(handle) = self.sealing.take() else {
            return Ok(());
        };
        let t = std::time::Instant::now();
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
        self.phase_ns[1] += t.elapsed().as_nanos() as u64;
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
        let pb = self.opts.partition_bytes.unwrap_or(self.opts.seal_bytes).max(1);
        let parts = match &fences {
            Some(f) => f.len(),
            None => {
                let b: u64 = inputs
                    .iter()
                    .filter_map(|n| std::fs::metadata(self.dir.join(n)).ok())
                    .map(|m| m.len())
                    .sum();
                (b as usize).div_ceil(pb).max(1)
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
        let max_keys = ((pb as f64 / per_key) as usize).max(1_000);
        let end_seq = self.covered_seq;
        let first_id = self.next_seg;
        // A split can turn one fence into several, so ids are reserved
        // generously; gaps in the sequence cost nothing.
        self.next_seg += (parts * 4).max(8) as u64;
        let dir = self.dir.clone();
        let opts = Db::segment_opts(&self.opts);
        let bulk = self.opts.bulk_writer;
        let cursors = self.opts.cursor_merge;
        let background_io = self.opts.background_io;
        let sync_every = self.opts.seal_sync_every;
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
                bulk,
                cursors,
                background_io,
                sync_every,
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
        let t = std::time::Instant::now();
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
        self.phase_ns[2] += t.elapsed().as_nanos() as u64;
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
        let np = self.segs.partition_point(|s| s.level > 0);
        let at = self.segs[..np]
            .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let part = self.segs[..np].get(at).filter(|s| s.may_hold(key));
        let l0 = &self.segs[np..];
        // Sources oldest to newest: the partition (0), the level-0 pieces
        // (1..), the frozen memtable, the live one. `start` is the source
        // live values begin at: 0 unless a newer source holds a tombstone
        // for this key. Only a store with tombstones in it checks, and the
        // check is what a delete costs a read -- a second probe on the
        // sources that hold the key.
        let (fr_ix, mem_ix) = (1 + l0.len(), 2 + l0.len());
        let mut start = 0usize;
        if self.has_tombstones() {
            if !self.mem.is_empty() {
                if let Some(e) = self.mem.get(key) {
                    if self.mem.has_tomb(e) {
                        start = mem_ix;
                    }
                }
            }
            if start == 0 {
                if let Some(fr) = &self.frozen {
                    if let Some(e) = fr.get(key) {
                        if fr.has_tomb(e) {
                            start = fr_ix;
                        }
                    }
                }
            }
            if start == 0 {
                for (i, seg) in l0.iter().enumerate().rev() {
                    if !seg.tombs || !seg.may_hold(key) {
                        continue;
                    }
                    if let Some(exts) = seg.blob.lookup(key) {
                        if exts.iter().any(|e| e.is_tombstone()) {
                            start = 1 + i;
                            break;
                        }
                    }
                }
            }
        }
        let mut n = 0u64;
        if start == 0 {
            if let Some(seg) = part {
                n += seg
                    .blob
                    .read_all(key, &mut f)
                    .map_err(|e| err(&format!("segment read: {e}")))?;
            }
        }
        for (i, seg) in l0.iter().enumerate() {
            if 1 + i < start || !seg.may_hold(key) {
                continue;
            }
            n += seg
                .blob
                .read_all(key, &mut f)
                .map_err(|e| err(&format!("segment read: {e}")))?;
        }
        if fr_ix >= start {
            if let Some(fr) = &self.frozen {
                if let Some(e) = fr.get(key) {
                    let (offs, _) = fr.live_chain(e);
                    n += offs.len() as u64;
                    for off in offs {
                        f(fr.value_at(off));
                    }
                }
            }
        }
        if mem_ix >= start && !self.mem.is_empty() {
            if let Some(e) = self.mem.get(key) {
                let (offs, _) = self.mem.live_chain(e);
                n += offs.len() as u64;
                for off in offs {
                    f(self.mem.value_at(off));
                }
            }
        }
        Ok(n)
    }

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

        // When nothing overlaps -- no unsealed keys in range, no L0 -- the
        // partitions ARE the answer in key order, and each one can be
        // walked by `Blob::scan`, which resolves each key once. The merge
        // below costs five or six index lookups an entry (a key_at per
        // cursor to find the minimum, another to emit, and a third inside
        // `values_at`) where this costs one, and after a routed flush this
        // is the shape the store is in. An earlier version had this path,
        // a refactor dropped it, and the scan axis paid for it.
        if mi >= unsealed.len() && !self.segs.iter().any(|s| s.level == 0) {
            let mut parts: Vec<&Seg> =
                self.segs.iter().filter(|s| s.may_reach(from)).collect();
            parts.sort_by(|a, b| a.lo.cmp(&b.lo));
            let mut seen = 0usize;
            let mut cursor: Vec<u8> = from.to_vec();
            for seg in parts {
                if seen >= limit {
                    break;
                }
                if seg.lo.as_slice() > cursor.as_slice() {
                    cursor = seg.lo.clone();
                }
                seen += seg
                    .blob
                    .scan(&cursor, limit - seen, &mut f)
                    .map_err(|e| err(&format!("segment scan: {e}")))?;
                match &seg.hi {
                    Some(h) => cursor = h.clone(),
                    None => break,
                }
            }
            return Ok(seen);
        }

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

        let tombs = self.has_tombstones();
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
            // Sources are ordered oldest to newest -- the cursors, then the
            // frozen memtable, then the live one -- so the newest source with
            // a tombstone for this key is a cut, and live values start there.
            let nc = cursors.len();
            let in_unsealed = unsealed.get(mi).map(|k| k.as_slice()) == Some(key);
            let mut start = 0usize;
            if tombs {
                if in_unsealed {
                    if self.mem.get(key).is_some_and(|e| self.mem.has_tomb(e)) {
                        start = nc + 1;
                    } else if self
                        .frozen
                        .as_ref()
                        .and_then(|fr| fr.get(key).map(|e| fr.has_tomb(e)))
                        .unwrap_or(false)
                    {
                        start = nc;
                    }
                }
                if start == 0 {
                    for (j, (seg, rank)) in cursors.iter().enumerate().rev() {
                        if seg.tombs && seg.blob.key_at(*rank) == Some(key) {
                            if let Some((_, exts)) = seg.blob.exts_at(*rank) {
                                if exts.iter().any(|e| e.is_tombstone()) {
                                    start = j;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            for (j, (seg, rank)) in cursors.iter_mut().enumerate() {
                if seg.blob.key_at(*rank) == Some(key) {
                    if j >= start {
                        seg.blob
                            .values_at(*rank, |v| f(key, v))
                            .map_err(|e| err(&format!("segment scan read: {e}")))?;
                    }
                    *rank += 1;
                }
            }
            if in_unsealed {
                if nc >= start {
                    if let Some(fr) = &self.frozen {
                        if let Some(e) = fr.get(key) {
                            for off in fr.live_chain(e).0 {
                                f(key, fr.value_at(off));
                            }
                        }
                    }
                }
                if nc + 1 >= start {
                    if let Some(e) = self.mem.get(key) {
                        for off in self.mem.live_chain(e).0 {
                            f(key, self.mem.value_at(off));
                        }
                    }
                }
                mi += 1;
            }
            seen += 1;
        }
        Ok(seen)
    }

    /// Values of `key` across every source. O(extents) per segment touched:
    /// each extent carries its record count (`Ext::count`, format v5), so no
    /// block is read. The memtable keeps a live count per key.
    pub fn count(&self, key: &[u8]) -> Result<u64> {
        let np = self.segs.partition_point(|s| s.level > 0);
        let at = self.segs[..np]
            .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let part = self.segs[..np].get(at).filter(|s| s.may_hold(key));
        let l0 = &self.segs[np..];
        let (fr_ix, mem_ix) = (1 + l0.len(), 2 + l0.len());
        let mut start = 0usize;
        if self.has_tombstones() {
            if !self.mem.is_empty() {
                if let Some(e) = self.mem.get(key) {
                    if self.mem.has_tomb(e) {
                        start = mem_ix;
                    }
                }
            }
            if start == 0 {
                if let Some(fr) = &self.frozen {
                    if let Some(e) = fr.get(key) {
                        if fr.has_tomb(e) {
                            start = fr_ix;
                        }
                    }
                }
            }
            if start == 0 {
                for (i, seg) in l0.iter().enumerate().rev() {
                    if !seg.tombs || !seg.may_hold(key) {
                        continue;
                    }
                    if let Some(exts) = seg.blob.lookup(key) {
                        if exts.iter().any(|e| e.is_tombstone()) {
                            start = 1 + i;
                            break;
                        }
                    }
                }
            }
        }
        let mut n = 0u64;
        if start == 0 {
            if let Some(seg) = part {
                n += seg.blob.count(key).map_err(|e| err(&format!("segment count: {e}")))?;
            }
        }
        for (i, seg) in l0.iter().enumerate() {
            if 1 + i < start || !seg.may_hold(key) {
                continue;
            }
            n += seg.blob.count(key).map_err(|e| err(&format!("segment count: {e}")))?;
        }
        if fr_ix >= start {
            if let Some(fr) = &self.frozen {
                if let Some(e) = fr.get(key) {
                    n += e.count;
                }
            }
        }
        if mem_ix >= start && !self.mem.is_empty() {
            if let Some(e) = self.mem.get(key) {
                n += e.count;
            }
        }
        Ok(n)
    }

    pub fn phase_ns(&self) -> (u64, u64, u64) {
        (self.phase_ns[0], self.phase_ns[1], self.phase_ns[2])
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
/// A transaction: puts and deletes staged in memory and applied at `commit`
/// as one WAL batch behind one commit frame and one barrier, so a crash
/// leaves all of them or none of them (`Wal::replay`). Reads through it see
/// the store as of `begin` plus its own staged writes, in order. Dropping it
/// without `commit` is `abort`: nothing has reached the WAL or the memtable,
/// so there is nothing to undo -- which is what staging buys, for one copy
/// of each value, against an undo log over a hash table that may have grown
/// under the transaction.
///
/// The plain `append` + `commit` path stays for callers that do not need
/// rollback; it is atomic too, by the same commit frame.
pub struct Txn<'a> {
    db: &'a mut Db,
    ops: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

impl Txn<'_> {
    pub fn append(&mut self, key: &[u8], value: &[u8]) {
        self.ops.push((key.to_vec(), Some(value.to_vec())));
    }

    pub fn delete(&mut self, key: &[u8]) {
        self.ops.push((key.to_vec(), None));
    }

    /// Staged operations so far.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The key's values as this transaction sees them: the store's, then its
    /// own staged operations applied in order. A key the transaction has not
    /// touched reads straight through.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        if !self.ops.iter().any(|(k, _)| k.as_slice() == key) {
            return self.db.read_all(key, f);
        }
        let mut vals: Vec<Vec<u8>> = Vec::new();
        self.db.read_all(key, |v| vals.push(v.to_vec()))?;
        for (k, v) in &self.ops {
            if k.as_slice() == key {
                match v {
                    Some(v) => vals.push(v.clone()),
                    None => vals.clear(),
                }
            }
        }
        for v in &vals {
            f(v);
        }
        Ok(vals.len() as u64)
    }

    pub fn count(&self, key: &[u8]) -> Result<u64> {
        let mut n = self.db.count(key)?;
        for (k, v) in &self.ops {
            if k.as_slice() == key {
                match v {
                    Some(_) => n += 1,
                    None => n = 0,
                }
            }
        }
        Ok(n)
    }

    /// Apply every staged operation and commit them as one batch.
    pub fn commit(self) -> Result<()> {
        let Txn { db, ops } = self;
        for (k, v) in ops {
            match v {
                Some(v) => db.append(&k, &v),
                None => db.delete(&k),
            }
        }
        db.commit()
    }

    /// Discard every staged operation. Dropping the transaction does the
    /// same; this is the name for doing it on purpose.
    pub fn abort(self) {}
}

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
