//! The engine: a WAL, a memtable, and immutable segments.
//!
//! `docs/engine.md` is the design brief and every load-bearing decision
//! here cites a measurement. A durable commit is one framed append and one
//! fdatasync and nothing else, because f39 measured that shape at 1,191,125
//! ops/s with all engine work removed, and the engine this replaced ran
//! 5.85x below it on per-point work this design deletes. Sealed segments are
//! the format `Blob` reads, so everything measured about that read path
//! carries over, browser reader included. There is no checkpoint: sealing is
//! off the commit path, and a store killed before its first seal opens from
//! the WAL alone, which is the brief's P-E.
//!
//! A batch commits atomically behind a commit frame and `Txn` builds one.
//! Deletes are tombstones that the merge collects. Segments are compacted
//! into key ranges, so a read routes by fence to one partition plus a
//! bounded L0 tail that a per-segment Bloom filter guards: the unfiltered
//! fan that queries every source was priced at 90ns a segment by f38, and
//! f41 refuted every keys-sized global router, which is why the routing is
//! by range and not by key.
//!
//! Crash discipline, in order, so every window is survivable:
//! commit = WAL append + fdatasync (the batch is durable or its tail frame
//! fails its CRC and replay stops before it); seal = write the segment to a
//! temp name, fsync it, rename into place, fsync the directory, then reset
//! the WAL -- a crash between any two of those leaves either a WAL that
//! replays the whole memtable or a complete renamed segment plus a WAL
//! whose sealed prefix is skipped by sequence number.

use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::block::{self, crc32, BlockBuilder, BlockLoc};
use crate::bytes::MmapBytes;
use crate::flatindex;
use crate::index::{Ext, Extents};
use crate::Blob;

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
pub struct Options {
    pub sync: SyncPolicy,
    /// Memtable bytes that trigger a seal at the next commit. Sealing is off
    /// the commit path in cost accounting but runs on the committing thread
    /// in milestone 1; the brief's "Segment size" question owns this number.
    pub seal_bytes: usize,
    /// SegmentOptions for the segment writer. Fixed to `redo_log: false, shards: 1`
    /// regardless of what is passed, because a sealed segment is written
    /// once and never reopened for writing -- the logshed finding that a
    /// 4 MiB redo arena in a write-once file is pure waste.
    pub segment: SegmentOptions,
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
    /// Promote pieces instead of merging them when nothing needs merging:
    /// a range's pieces whose keys all lie above its partition's last key,
    /// mutually disjoint, become partitions by rename, and the partition's
    /// fence closes below them. Nothing is rewritten. Ordered ingest -- a
    /// log -- is all promotion; uniform keys never qualify (f55,
    /// promote-plan.md).
    pub promote: bool,
    /// How a flush drains level 0 once partitions exist: merge only the
    /// ranges that hold pieces, under the live fences (`true`), or
    /// re-partition everything from every key (`false`, the original). f54
    /// prices the difference (merge-plan.md).
    pub flush_ranges: bool,
    /// Runs of values up to this many bytes are stored inline in the index
    /// record rather than in a block, so a point read of such a key touches
    /// the hash slot and the record and nothing else. Zero disables; f53
    /// prices it (inline-plan.md).
    pub inline_bytes: usize,
    /// Target bytes per partition: how many partitions the first
    /// partitioning cuts, and how many keys one holds before a merge splits
    /// it. `None` uses `seal_bytes`, which is how f52 found that smaller
    /// seals were also making more partitions and paying for them on every
    /// read; `Some` decouples the two.
    pub partition_bytes: Option<usize>,
    /// Recycle retired WAL files instead of creating fresh ones, and
    /// pre-write the first to the seal size, so every block a commit's
    /// fdatasync touches is already allocated and written. On ext4 an
    /// fdatasync of an append that grows the file commits an inode change
    /// through the journal; an overwrite does not, and LMDB's commit is an
    /// overwrite. f57 prices it (walreuse-plan.md).
    /// Tell the kernel that reads of a segment are random.
    ///
    /// `MADV_RANDOM` on every segment mapping the reader opens. Off by
    /// default, because it is a trade rather than a win and neither side of it
    /// is small: `f65-madvise` measured cold point reads 75.8x and 78.9x
    /// faster advised, at 1800x read amplification against 1.0x -- the default
    /// fetched 157 GB off the device to serve 89 MB anybody asked for -- and
    /// the ordered scan 2.3x to 2.5x *slower*, because a scan wanted every page
    /// readahead would have fetched (`F65.1`, `F65.2`, `F65.3`).
    ///
    /// So set it for a store whose working set outgrows memory and whose reads
    /// are points; leave it off for one that scans.
    ///
    /// It applies to the reader's segment mappings only, and that holds even
    /// for a segment compaction later consumes: `madvise` is a property of a
    /// mapping rather than of a file, and `compact` opens its inputs through
    /// its own `MmapBytes`. So a compaction streams under the kernel's default
    /// readahead however this is set, which is the side of the trade it
    /// wants.
    pub advise_random: bool,
    pub recycle_wal: bool,
    /// The ordered scan's merge over unrouted sources. `true` is the merge
    /// f61 priced and f62 replaced: one cursor over the disjoint partitions
    /// in order rather than one per partition, each cursor's key resolved
    /// once per emitted key, and the unsealed snapshot carrying each key's
    /// memtable entry so the emit is a chain walk over a reused buffer
    /// instead of two hash probes and an allocation. `false` is the merge
    /// before it, kept as the comparison arm (scanmerge-plan.md).
    pub scan_merge: bool,
    /// How the ordered scan builds its sorted snapshot of the unsealed keys.
    /// `true` keeps the keys in one arena and sorts 24-byte records (a
    /// 16-byte key prefix and an index), touching the arena only on a shared
    /// prefix; `false` is the build before it -- a `Vec<u8>` per key, sorted
    /// through two heap pointers per compare -- kept as the comparison arm.
    /// The build runs on the first scan after a commit and cost 300 ns a key
    /// (scansnap-plan.md).
    pub scan_snapshot_arena: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            sync: SyncPolicy::Always,
            // 32 MB seals over 64 MB partitions: f52 measured 1.129x the
            // ingest of 64 MB seals at identical device bytes and identical
            // reads (F52.5, F52.6). Smaller still buys nothing and costs
            // 1.5x the device bytes.
            seal_bytes: 32 << 20,
            segment: SegmentOptions::default(),
            l0_trigger: 4,
            compact: true,
            partition_on_flush: true,
            cursor_merge: true,
            background_io: BackgroundIo::Normal,
            seal_sync_every: 0,
            inline_bytes: 256,
            partition_bytes: Some(64 << 20),
            flush_ranges: true,
            promote: true,
            recycle_wal: false,
            advise_random: false,
            scan_merge: true,
            scan_snapshot_arena: true,
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
    /// Bytes handed to the file so far, and how many of them were behind a
    /// barrier at the last `sync`. The difference is exactly what a power
    /// loss may take, and `Db::wal_durable` reports it so a crash
    /// experiment can take it (c4-crash).
    written: u64,
    synced: u64,
    /// Mixed from the file's id and xored into every frame's CRC, so a
    /// frame left in a recycled file by its previous life -- written under
    /// another id -- fails its check and replay stops at the true tail.
    seed: u32,
}

/// The WAL file starts with this, so a file from before frames carried a
/// kind byte, before CRCs were seeded by file id, or before the CRC moved
/// from the frame to the batch, is refused by name rather than replayed as
/// something else.
const WAL_MAGIC: &[u8; 8] = b"SUPDBWL\x04";

/// The per-file CRC seed. Any mix that separates neighbouring ids will do;
/// this is splitmix64's finalizer.
fn wal_seed(id: u64) -> u32 {
    let mut z = id.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}
/// Frame kinds. A batch is the frames between commit frames, and replay
/// applies a batch only once its commit frame has been read intact.
const WAL_PUT: u8 = 0;
const WAL_DEL: u8 = 1;
const WAL_COMMIT: u8 = 2;

impl Wal {
    fn create(path: &Path, id: u64) -> Result<Wal> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        file.write_all(WAL_MAGIC)?;
        Ok(Wal {
            file,
            path: path.to_path_buf(),
            seq: 0,
            pending: Vec::new(),
            written: WAL_MAGIC.len() as u64,
            synced: 0,
            seed: wal_seed(id),
        })
    }

    /// Write zeros out to `bytes` so the blocks behind the coming appends
    /// are allocated and written before any commit needs them, then put
    /// the cursor back behind the header. Zeros read as a frame of length
    /// zero, which replay refuses, so a crash before the first commit
    /// leaves an empty WAL and not a strange one.
    ///
    /// In page-sized writes, and that is load-bearing: the page cache
    /// sizes a folio by the write that creates it, and a byte dirtied in
    /// a 1 MB folio writes back the whole megabyte. Pre-written in 1 MB
    /// pieces, every later 100 KB commit cost 11x its bytes at the device;
    /// in 4 KB pieces, 1.04x (f57's first run, and walreuse-plan.md).
    fn prefill(&mut self, bytes: u64) -> Result<()> {
        let zeros = vec![0u8; 4096];
        let mut at = self.written;
        while at < bytes {
            let n = zeros.len().min((bytes - at) as usize);
            self.file.write_all(&zeros[..n])?;
            at += n as u64;
        }
        self.file.sync_all()?;
        self.file.seek(SeekFrom::Start(self.written))?;
        Ok(())
    }

    /// Take a retired file as the new WAL: rename it into place and write a
    /// fresh header over its old one. Everything after the header is the
    /// previous life's frames, written under another id, and is overwritten
    /// as this life appends; replay stops at the first frame whose CRC does
    /// not verify under this id.
    fn recycle(spare: &Path, path: &Path, id: u64) -> Result<Wal> {
        std::fs::rename(spare, path)?;
        let mut file = OpenOptions::new().write(true).open(path)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(WAL_MAGIC)?;
        Ok(Wal {
            file,
            path: path.to_path_buf(),
            seq: 0,
            pending: Vec::new(),
            written: WAL_MAGIC.len() as u64,
            synced: 0,
            seed: wal_seed(id),
        })
    }

    /// Reopen a WAL for appending at `seq`, after replay has truncated it to
    /// its last commit frame. A file that does not exist yet gets its header
    /// so the next replay finds one.
    fn open_append(path: &Path, id: u64, seq: u64) -> Result<Wal> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut written = file.metadata()?.len();
        if written == 0 {
            file.write_all(WAL_MAGIC)?;
            written = WAL_MAGIC.len() as u64;
        }
        // What replay kept was read back from the device, so it counts as
        // synced; a header written just now does not until the first
        // barrier.
        let synced = if written == WAL_MAGIC.len() as u64 {
            0
        } else {
            written
        };
        Ok(Wal {
            file,
            path: path.to_path_buf(),
            seq,
            pending: Vec::new(),
            written,
            synced,
            seed: wal_seed(id),
        })
    }

    /// One frame: `len u32 | crc u32 | seq u64 | kind u8 | payload`, where a
    /// put's payload is `klen uvarint | key | value`, a delete's is the key
    /// alone and a commit's is the batch CRC. `len` covers everything after
    /// `crc`.
    ///
    /// The CRC is per batch, not per frame (f59). A record frame's `crc`
    /// word is zero; the commit frame carries, as its payload, the CRC of
    /// every byte of the batch's record frames, and its own `crc` word
    /// covers its body as before. Replay applies a batch only at a commit
    /// frame whose both CRCs verify, so a damaged byte anywhere in a batch
    /// loses that batch and the ones after it -- exactly what a CRC per
    /// frame bought, at one CRC setup and finish per batch instead of per
    /// record: 92 of the 677 instructions a record cost (f58).
    fn frame(&mut self, kind: u8, key: &[u8], value: &[u8]) {
        // `pending` holds exactly this batch's record frames: `write`
        // empties it at every commit.
        let batch_crc = if kind == WAL_COMMIT {
            crc32(&self.pending) ^ self.seed
        } else {
            0
        };
        let body_at = self.pending.len() + FRAME_HEADER;
        self.pending.extend_from_slice(&[0u8; FRAME_HEADER]);
        self.pending.extend_from_slice(&self.seq.to_le_bytes());
        self.pending.push(kind);
        if kind == WAL_COMMIT {
            self.pending.extend_from_slice(&batch_crc.to_le_bytes());
        } else {
            put_uvarint(&mut self.pending, key.len() as u64);
            self.pending.extend_from_slice(key);
            if kind == WAL_PUT {
                self.pending.extend_from_slice(value);
            }
        }
        let body_len = (self.pending.len() - body_at) as u32;
        let crc = if kind == WAL_COMMIT {
            crc32(&self.pending[body_at..]) ^ self.seed
        } else {
            0
        };
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
        self.written += self.pending.len() as u64;
        self.pending.clear();
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.file.sync_data()?;
        self.synced = self.written;
        Ok(())
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
        id: u64,
        from: u64,
        mut apply: impl FnMut(u8, &[u8], &[u8]),
    ) -> Result<(u64, u64)> {
        let seed = wal_seed(id);
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
        // A file shorter than the header but a prefix of it is a header a
        // power loss tore before its first barrier -- a WAL with nothing in
        // it, not a foreign one. The caller truncates it and the header is
        // rewritten.
        if buf.len() < WAL_MAGIC.len() {
            if WAL_MAGIC.starts_with(&buf) {
                return Ok((from, 0));
            }
            return Err(err(
                "not a supdb WAL: the header is missing or from an older format",
            ));
        }
        if &buf[..WAL_MAGIC.len()] != WAL_MAGIC {
            return Err(err(
                "not a supdb WAL: the header is missing or from an older format",
            ));
        }
        let mut p = WAL_MAGIC.len();
        let mut next_seq = from;
        let mut committed_seq = from;
        let mut valid_len = p as u64;
        // The batch being read: kind, sequence, and where its key and value
        // lie in `buf`, so nothing is copied and nothing is checked until
        // the commit frame says the whole batch is intact.
        let mut batch: Vec<(u8, u64, usize, usize, usize)> = Vec::new();
        let mut batch_start = p;
        while buf.len() - p >= FRAME_HEADER {
            let len = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[p + 4..p + 8].try_into().unwrap());
            let body_at = p + FRAME_HEADER;
            let Some(end) = body_at.checked_add(len) else {
                break;
            };
            if end > buf.len() || len < 9 {
                break;
            }
            let body = &buf[body_at..end];
            let seq = u64::from_le_bytes(body[..8].try_into().unwrap());
            let kind = body[8];
            match kind {
                WAL_COMMIT => {
                    // The commit frame's own CRC, then the batch's. Either
                    // failing means the batch never made it whole, and the
                    // walk ends here -- what a torn tail always meant.
                    if crc32(body) ^ seed != crc || body.len() != 13 {
                        break;
                    }
                    let want = u32::from_le_bytes(body[9..13].try_into().unwrap());
                    if crc32(&buf[batch_start..p]) ^ seed != want {
                        break;
                    }
                    // Intact. Now the sequence has to be continuous, which is
                    // a statement about durability rather than damage: a gap
                    // in a verified batch is a record the writer lost, and
                    // that is an error, not a torn tail.
                    for &(_, s, _, _, _) in &batch {
                        if s >= from {
                            if s != next_seq {
                                return Err(err("wal sequence gap: a durable record is missing"));
                            }
                            next_seq = s + 1;
                        }
                    }
                    if seq >= from {
                        if seq != next_seq {
                            return Err(err("wal sequence gap: a durable record is missing"));
                        }
                        next_seq = seq + 1;
                    }
                    for &(k, s, ks, ke, ve) in &batch {
                        if s >= from {
                            apply(k, &buf[ks..ke], &buf[ke..ve]);
                        }
                    }
                    batch.clear();
                    committed_seq = next_seq;
                    valid_len = end as u64;
                    batch_start = end;
                }
                WAL_PUT | WAL_DEL => {
                    // A record frame is checked by its batch, so anything
                    // malformed here is a batch that will not verify: end the
                    // walk rather than report damage the commit frame would
                    // have caught. Its `crc` word is zero by construction.
                    if crc != 0 {
                        break;
                    }
                    let mut q = 9usize;
                    let Some(klen) = get_uvarint(body, &mut q) else {
                        break;
                    };
                    let Some(kend) = q.checked_add(klen as usize).filter(|&e| e <= body.len())
                    else {
                        break;
                    };
                    if kind == WAL_DEL && kend != body.len() {
                        break;
                    }
                    batch.push((kind, seq, body_at + q, body_at + kend, end));
                }
                _ => break,
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
/// there is. Here it guards only the bounded L0 tail, because f41
/// refuted every keys-sized global router -- the partitioned
/// levels below are routed by fences that cost two comparisons.
pub(crate) struct BlockedBloom {
    blocks: Vec<[u64; 8]>,
}

impl BlockedBloom {
    fn with_capacity(n: usize) -> BlockedBloom {
        BlockedBloom {
            blocks: vec![[0u64; 8]; (n * 10).div_ceil(512).max(1)],
        }
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
/// 2.04x the floor for exactly that input. This is the writer that
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
    /// File offset the next write lands at. Data starts after the header
    /// region, which holds the two superblock slots and is written last.
    pos: u64,
    builder: BlockBuilder,
    block_size: usize,
    blocks: Vec<BlockLoc>,
    /// Every key written, concatenated, with each key's span. Flat rather
    /// than a `Vec<Vec<u8>>` because a segment has a million keys; the
    /// extent beside each span is the one record the index carries.
    key_arena: Vec<u8>,
    spans: Vec<(usize, usize)>,
    exts: Vec<Extents>,
    /// The key currently open, its run of length-prefixed records, and the
    /// offset of the newest record's prefix inside the run -- what
    /// `Ext::last` carries so that reading the newest value is O(1).
    open_key: Option<(usize, usize)>,
    run: Vec<u8>,
    /// The open key's values as they arrive, and their lengths; encoded
    /// into `run` at `end`.
    raw: Vec<u8>,
    lens: Vec<u32>,
    last: usize,
    records: u32,
    parallel_index: bool,
    /// fdatasync every this many block bytes; zero for the end only.
    sync_every: u64,
    since_sync: u64,
    /// Runs up to this many bytes go into the record's tail instead of a
    /// block (`Ext::INLINE`); zero keeps every run in blocks.
    inline_max: usize,
    /// LZ4 the blocks, as `Store` does when `SegmentOptions::compress` is set. A
    /// block above the chunk size is compressed chunk by chunk with its own
    /// directory, so a point read decompresses one chunk rather than the
    /// block; one that does not shrink is written verbatim. Inline runs live
    /// in the key section and are untouched either way
    /// (segcompress-plan.md, R7.4).
    compress: bool,
    /// Per-chunk checksums for the blocks written verbatim, one row per
    /// block in the block table. Without them a run read fetches the whole
    /// block, which is what `blob::chunk_span` plans by.
    chunk_rows: Vec<[u32; block::MAX_CHUNK_CRCS]>,
    /// Bytes left free after the superblock page, into which `finish` puts
    /// the block table and a copy of the fence when they fit, so a host
    /// whose first probe covers the reserve opens in one round trip
    /// (waves-plan.md, R7.1). Zero for none; laid down at the first key.
    head_reserve: usize,
    reserve_off: u64,
    /// The inline runs, concatenated, with each key's span in it (empty for
    /// a key whose run went to a block). Blocks-first mode only; the
    /// records-first mode streams each tail out inside its record.
    tails: Vec<u8>,
    tail_spans: Vec<(usize, usize)>,
    /// Which of the two layouts this segment is being written in. Decided
    /// at the first key from `inline_max`, because the first bytes differ.
    mode: Option<Layout>,
    /// Records-first mode: the records streamed so far, their offsets, and
    /// each key's hash for the trailer's slots.
    recs_len: usize,
    rec_offs: Vec<u32>,
    hashes: Vec<u64>,
    rec_buf: Vec<u8>,
    /// Records-first mode: block bytes held until the section is complete,
    /// with their table rows (offsets filled in when they are written).
    pending_blocks: Vec<Vec<u8>>,
}

/// The two layouts the writer produces. Same format, same readers, one
/// difference: what streams during the pass.
///
/// `BlocksFirst` is what `Store` also writes -- data blocks as they fill,
/// then the block table and the key section built whole at the end. It is
/// right when values live in blocks, because the blocks stream and the
/// kernel writes them back behind the pass.
///
/// `RecordsFirst` is for inline runs: the key section comes first in the
/// file and its records stream as keys arrive, the few block-backed runs
/// are held and written after it, and the hash slots, directory and fences
/// go after the records. Without it an inline segment wrote nothing during
/// the pass and its whole section at `finish`, and f53 measured that as
/// 0.807x on ingest for the same bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    BlocksFirst,
    RecordsFirst,
}

/// One superblock slot, field for field what `store::Super::encode` writes:
/// sixteen little-endian u64 fields, the magic in native order as the
/// byte-order mark, and the FNV-1a of the fields and the magic.
fn superblock(fields: &[u64; 16]) -> [u8; crate::format::SUPER_BYTES] {
    let mut out = [0u8; crate::format::SUPER_BYTES];
    for (i, v) in fields.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    out[128..136].copy_from_slice(&crate::format::MAGIC.to_ne_bytes());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in fields.iter().chain(std::iter::once(&crate::format::MAGIC)) {
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
    /// rest of `SegmentOptions` describes machinery this writer does not have.
    pub fn create(path: &Path, opts: &SegmentOptions) -> Result<SegmentWriter> {
        // The checksum switch is process-wide and `Store::create` sets it
        // from the same option; a writer that recorded none while readers
        // expected them would fail every block it wrote.
        block::CHECKSUMS.store(opts.checksums, std::sync::atomic::Ordering::Relaxed);
        // Read as well as write: `finish` reads the streamed records back to
        // compute the key section's checksum row.
        let file = OpenOptions::new()
            .read(true)
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
        // The header region stays zero until `finish`, so a segment that
        // was never finished is a file no reader accepts rather than a
        // segment with some of its keys.
        out.write_all(&[0u8; crate::format::SUPER as usize])?;
        let block_size = opts.block_size.max(1);
        Ok(SegmentWriter {
            out,
            pos: crate::format::SUPER,
            builder: BlockBuilder::new(block_size),
            block_size,
            blocks: Vec::new(),
            key_arena: Vec::new(),
            spans: Vec::new(),
            exts: Vec::new(),
            open_key: None,
            run: Vec::new(),
            raw: Vec::new(),
            lens: Vec::new(),
            last: 0,
            records: 0,
            parallel_index: opts.parallel_index,
            sync_every: 0,
            since_sync: 0,
            inline_max: 0,
            compress: false,
            chunk_rows: Vec::new(),
            head_reserve: 0,
            reserve_off: 0,
            tails: Vec::new(),
            tail_spans: Vec::new(),
            mode: None,
            recs_len: 0,
            rec_offs: Vec::new(),
            hashes: Vec::new(),
            rec_buf: Vec::new(),
            pending_blocks: Vec::new(),
        })
    }

    /// Spread the writer's syncs: fdatasync every `bytes` of blocks written
    /// instead of once at `finish`. Zero restores the single sync.
    pub fn set_sync_every(&mut self, bytes: usize) {
        self.sync_every = bytes as u64;
    }

    /// Store runs up to `bytes` long inline in the index record, and write
    /// the segment records-first so they stream. Zero keeps every run in a
    /// block and the blocks-first layout `Store` writes. Must be set before
    /// the first key.
    pub fn set_inline_max(&mut self, bytes: usize) {
        self.inline_max = bytes;
    }

    /// Compress the blocks. Off by default, because a segment written by the
    /// the seal is read back by its own merge and the seal path
    /// has never paid for compression; a segment written as an index to be
    /// downloaded is the other case, and logshed's day index is 30% smaller
    /// with it on. Must be set before the first key.
    pub fn set_compress(&mut self, on: bool) {
        self.compress = on;
    }

    /// Leave `bytes` free after the superblock page for the block table and
    /// a copy of the fence, so a sparse open whose first probe is that
    /// generous needs no second round trip. Must be set before the first
    /// key. Costs `bytes` of file whether or not they fill.
    pub fn set_head_reserve(&mut self, bytes: usize) {
        self.head_reserve = bytes;
    }

    /// Where the key section starts: after the superblock page and the
    /// head reserve, if any.
    fn key_start(&self) -> u64 {
        crate::format::SUPER
            + if self.reserve_off != 0 {
                self.head_reserve as u64
            } else {
                0
            }
    }

    fn layout(&mut self) -> Result<Layout> {
        if let Some(m) = self.mode {
            return Ok(m);
        }
        let m = if self.inline_max > 0 {
            Layout::RecordsFirst
        } else {
            Layout::BlocksFirst
        };
        if self.head_reserve > 0 && self.reserve_off == 0 {
            self.reserve_off = self.pos;
            let mut left = self.head_reserve;
            let zeros = [0u8; 4096];
            while left > 0 {
                let n = left.min(zeros.len());
                self.out.write_all(&zeros[..n])?;
                left -= n;
            }
            self.pos += self.head_reserve as u64;
        }
        if m == Layout::RecordsFirst {
            // The section header is written last, once the trailer's
            // offsets are known; its 192 bytes are reserved now so the
            // records start where `stream_trailer` says they do.
            self.out.write_all(&[0u8; 192])?;
            self.pos += 192;
        }
        self.mode = Some(m);
        Ok(m)
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
                return Err(err(
                    "segment writer: keys must arrive in strictly increasing order",
                ));
            }
        }
        self.layout()?;
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
        // Raw bytes and a length: the encoding is chosen at `end`, when the
        // whole run is in hand and it is known whether every value shares
        // one width (fixed, no prefixes) or not (prefixed).
        self.records += 1;
        self.lens.push(v.len() as u32);
        self.raw.extend_from_slice(v);
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
        let (last, flag) = crate::index::encode_run(&self.raw, &self.lens, &mut self.run);
        self.last = last as usize;
        self.raw.clear();
        self.lens.clear();
        let n = self.run.len();
        if n > u32::MAX as usize {
            return Err(err(
                "segment writer: a key's values exceed 4 GiB in one segment",
            ));
        }
        if self.records >= Ext::FIXED {
            return Err(err(
                "segment writer: a key's values exceed the extent's count",
            ));
        }
        let count = self.records | flag | if tombstone { Ext::TOMBSTONE } else { 0 };
        let layout = self.layout()?;
        let inline = self.inline_max > 0 && n <= self.inline_max;
        let ext = if inline {
            // Into the record: a read of this key never touches a block.
            // `off` is within this key's tail, and a key has one run here.
            Ext {
                block: Ext::INLINE,
                off: 0,
                len: n as u32,
                last: self.last as u32,
                count,
            }
        } else {
            // A run that does not fit beside what is staged starts a new
            // block; a run larger than a whole block takes an empty builder
            // and is a block by itself, so a key's values stay contiguous --
            // the same rule `Store` applies through the same `BlockBuilder`.
            if self.builder.would_overflow(n) {
                self.flush_block()?;
            }
            let off = self.builder.push(&self.run);
            let ext = Ext {
                block: self.blocks.len() as u32,
                off,
                len: n as u32,
                last: self.last as u32,
                count,
            };
            if self.builder.len() >= self.block_size {
                self.flush_block()?;
            }
            ext
        };
        match layout {
            Layout::RecordsFirst => {
                let key = &self.key_arena[start..start + len];
                let tail: &[u8] = if inline { &self.run } else { &[] };
                self.rec_buf.clear();
                let wrote = flatindex::stream_record(&mut self.rec_buf, key, &[ext], tail)
                    .ok_or_else(|| err("segment writer: record exceeds the flat index's limits"))?;
                self.out.write_all(&self.rec_buf)?;
                self.pos += wrote as u64;
                self.rec_offs.push(self.recs_len as u32);
                self.recs_len += wrote;
                self.hashes.push(flatindex::key_hash(key));
                if self.recs_len > flatindex::MAX_RECS {
                    return Err(err(
                        "segment writer: key section exceeds the flat index's limits",
                    ));
                }
            }
            Layout::BlocksFirst => {
                if inline {
                    let ts = self.tails.len();
                    self.tails.extend_from_slice(&self.run);
                    self.tail_spans.push((ts, n));
                } else {
                    self.tail_spans.push((0, 0));
                }
                self.exts.push(Extents::One(ext));
            }
        }
        self.spans.push((start, len));
        Ok(())
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.builder.is_empty() {
            return Ok(());
        }
        let payload = self.builder.take();
        // The same three cases `Store::write_block` has: chunked when the
        // payload is worth chunking and the result is smaller, compressed
        // whole when it is not chunkable, verbatim when compression does not
        // pay. A chunked block carries its own per-chunk checksums in its
        // directory; a verbatim one gets a row beside it in the block table,
        // which is what lets a reader fetch the chunks an extent spans
        // instead of the block (segcompress-plan.md).
        let uncompressed = payload.len() as u32;
        let chunked = self.compress && payload.len() > block::CHUNK;
        let stored: Option<Vec<u8>> = if chunked {
            let c = block::write_chunked_sz(&payload, block::CHUNK);
            if c.len() < payload.len() {
                Some(c)
            } else {
                None
            }
        } else if self.compress {
            block::compress(&payload)
        } else {
            None
        };
        let chunked = chunked && stored.is_some();
        let bytes = stored.unwrap_or(payload);
        let len = bytes.len() as u32;
        let row = if block::checksums_on() && len == uncompressed {
            block::chunk_crcs(&bytes)
        } else {
            None
        };
        let crc = if block::checksums_on() {
            crc32(&bytes)
        } else {
            0
        };
        self.blocks.push(BlockLoc {
            off: self.pos,
            stored: len,
            uncompressed,
            cap: len,
            chunked,
            solo: false,
            chunk_crc: row.is_some(),
            crc,
        });
        self.chunk_rows
            .push(row.unwrap_or([0u32; block::MAX_CHUNK_CRCS]));
        if self.mode == Some(Layout::RecordsFirst) {
            // Held until the section is complete; `off` is set when it is
            // written, and nothing reads the row before then.
            self.pending_blocks.push(bytes);
            return Ok(());
        }
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

    /// Write what is left -- the key section or its trailer, the held
    /// blocks, the block table, the superblock -- and fsync. `generation`
    /// is what the segment reports as its checkpoint identity; a segment
    /// is written once, so 1 is the usual answer.
    pub fn finish(mut self, generation: u64) -> Result<()> {
        if self.open_key.is_some() {
            return Err(err("segment writer: finish with a key still open"));
        }
        let layout = self.layout()?;
        // A segment with no keys is allowed: a partition whose every key was
        // deleted still has to exist, or the fences stop tiling the key
        // space and a later seal would route keys into a neighbour's range.
        self.flush_block()?;

        let (key_off, key_len, header): (u64, usize, Option<Vec<u8>>) = match layout {
            Layout::RecordsFirst => {
                let key_off = self.key_start();
                let (header, trailer, total) = {
                    let arena = &self.key_arena;
                    let spans = &self.spans;
                    let key_at = |i: usize| -> &[u8] {
                        let (s, l) = spans[i];
                        &arena[s..s + l]
                    };
                    flatindex::stream_trailer(
                        self.recs_len,
                        &self.rec_offs,
                        &key_at,
                        &self.hashes,
                        generation,
                    )
                    .ok_or_else(|| {
                        err("segment writer: key section exceeds the flat index's limits")
                    })?
                };
                self.out.write_all(&trailer)?;
                self.pos += trailer.len() as u64;
                debug_assert_eq!(self.pos, key_off + total as u64);
                // The checksum row: named in the header, computed over the
                // header as it will be written plus the records already on
                // disk, one piece at a time, and appended after the trailer.
                let mut header = header;
                flatindex::set_checksum_words(&mut header, total);
                self.out.flush()?;
                let row = {
                    use std::os::unix::fs::FileExt;
                    let file = self.out.get_ref();
                    let mut buf = vec![0u8; 1usize << flatindex::PIECE_SHIFT];
                    let mut row = Vec::with_capacity(flatindex::checksum_row_len(
                        total,
                        flatindex::PIECE_SHIFT,
                        key_off,
                    ));
                    for (at, end) in flatindex::pieces(total, flatindex::PIECE_SHIFT, key_off) {
                        let n = end - at;
                        let from_header = header.len().saturating_sub(at).min(n);
                        if from_header > 0 {
                            buf[..from_header].copy_from_slice(&header[at..at + from_header]);
                        }
                        if n > from_header {
                            file.read_exact_at(
                                &mut buf[from_header..n],
                                key_off + (at + from_header) as u64,
                            )?;
                        }
                        row.extend_from_slice(&block::crc32(&buf[..n]).to_le_bytes());
                    }
                    row
                };
                self.out.write_all(&row)?;
                self.pos += row.len() as u64;
                let total = total + row.len();
                // Now the blocks that were held, each row taking its offset
                // as it lands.
                let held = std::mem::take(&mut self.pending_blocks);
                for (i, bytes) in held.into_iter().enumerate() {
                    self.blocks[i].off = self.pos;
                    self.out.write_all(&bytes)?;
                    self.pos += bytes.len() as u64;
                }
                (key_off, total, Some(header))
            }
            Layout::BlocksFirst => (0, 0, None),
        };

        let table = flatindex::encode_blocks(&self.blocks, &self.chunk_rows);
        // The table goes into the head reserve when there is one and it
        // fits with room for the fence copy; else at the end, as before.
        let table_in_reserve = self.reserve_off != 0 && table.len() + 8 <= self.head_reserve;
        let blk_off = if table_in_reserve {
            self.reserve_off
        } else {
            self.pad_to(8)?;
            let at = self.pos;
            self.out.write_all(&table)?;
            self.pos += table.len() as u64;
            at
        };

        let (key_off, key_len, header_bytes) = if layout == Layout::BlocksFirst {
            let (section, reserve) = {
                let all: Vec<(&[u8], &Extents)> = self
                    .spans
                    .iter()
                    .zip(&self.exts)
                    .map(|(&(s, l), e)| (&self.key_arena[s..s + l], e))
                    .collect();
                let tails: Vec<&[u8]> = self
                    .tail_spans
                    .iter()
                    .map(|&(s, l)| &self.tails[s..s + l])
                    .collect();
                // No insert room and no record slack: a segment is never
                // edited in place, and the half-again the flat index
                // reserves for that is 20 B a key of file it would never use.
                flatindex::encode_inline(
                    &all,
                    &tails,
                    generation,
                    None,
                    flatindex::key_hash,
                    0,
                    false,
                    self.parallel_index,
                )
                .ok_or_else(|| err("segment writer: key section exceeds the flat index's limits"))?
            };
            // A segment reserves no slack, so the section is complete and
            // takes its checksum row here, over pieces laid on the object's
            // pages from where the section will start.
            self.pad_to(8)?;
            let key_off = self.pos;
            let section = if reserve <= section.len() {
                flatindex::with_checksums(section, key_off)
            } else {
                section
            };
            self.out.write_all(&section)?;
            let key_len = reserve.max(section.len());
            if key_len > section.len() {
                self.out.write_all(&vec![0u8; key_len - section.len()])?;
            }
            self.pos += key_len as u64;
            let mut hb = [0u8; flatindex::HEADER_BYTES];
            hb.copy_from_slice(&section[..flatindex::HEADER_BYTES]);
            (key_off, key_len, hb)
        } else {
            let mut hb = [0u8; flatindex::HEADER_BYTES];
            hb.copy_from_slice(header.as_deref().expect("records-first header"));
            (key_off, key_len, hb)
        };

        let file = self.out.into_inner().map_err(|e| e.into_error())?;
        use std::os::unix::fs::FileExt;
        if let Some(h) = &header {
            file.write_all_at(h, key_off)?;
        }

        // The superblock extension: the header copy and every offset a
        // sparse open needs, so it plans itself from the first probe; and
        // the reserve's contents -- table, then the fence copy when it fits.
        let hdr = flatindex::Header::parse(&header_bytes)
            .ok_or_else(|| err("segment writer: the header it wrote does not parse"))?;
        let (foff, flen) = flatindex::fence_span(&hdr, key_len);
        let row_len = if hdr.crc_off != 0 {
            flatindex::checksum_row_len(hdr.crc_off, hdr.piece_shift, key_off)
        } else {
            0
        };
        let mut fence_copy = None;
        let mut row_copy = None;
        let mut dir_copy = None;
        if table_in_reserve {
            file.write_all_at(&table, self.reserve_off)?;
            let end = self.reserve_off + self.head_reserve as u64;
            let mut at = (self.reserve_off + table.len() as u64).div_ceil(8) * 8;
            // The checksum row first -- verification needs it before the
            // fence -- then the fence, each when it fits.
            if row_len > 0 && at + row_len as u64 <= end {
                let mut row = vec![0u8; row_len];
                file.read_exact_at(&mut row, key_off + hdr.crc_off as u64)?;
                file.write_all_at(&row, at)?;
                row_copy = Some(at);
                at = (at + row_len as u64).div_ceil(8) * 8;
            }
            if flen > 0 && at + flen as u64 <= end {
                let mut fence = vec![0u8; flen];
                file.read_exact_at(&mut fence, key_off + foff as u64)?;
                file.write_all_at(&fence, at)?;
                fence_copy = Some((at, flen as u64, block::crc32(&fence)));
                at = (at + flen as u64).div_ceil(8) * 8;
            }
            // And the directory, so a directory-resident open is one wave
            // too when the reserve is sized for it.
            let dlen = hdr.nkeys * 4;
            if dlen > 0 && at + dlen as u64 <= end {
                let mut d = vec![0u8; dlen];
                file.read_exact_at(&mut d, key_off + hdr.dir_off as u64)?;
                file.write_all_at(&d, at)?;
                dir_copy = Some((at, block::crc32(&d)));
            }
        }
        let ext = crate::blob::SuperExt {
            fence: (key_off + foff as u64, flen as u64),
            dir: (key_off + hdr.dir_off as u64, hdr.nkeys as u64 * 4),
            hash: (key_off + hdr.hash_off as u64, hdr.hash_cap as u64 * 8),
            row: (key_off + hdr.crc_off as u64, row_len as u64),
            table_copy: if table_in_reserve {
                Some((blk_off, table.len() as u64))
            } else {
                None
            },
            fence_copy,
            row_copy,
            dir_copy,
            header: header_bytes,
        };

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
        let mut page = vec![0u8; crate::format::SUPER as usize];
        page[..sb.len()].copy_from_slice(&sb);
        page[crate::format::SLOT as usize..crate::format::SLOT as usize + sb.len()]
            .copy_from_slice(&sb);
        let x = crate::blob::encode_super_ext(&ext, generation);
        page[1024..1024 + x.len()].copy_from_slice(&x);
        file.write_all_at(&page, 0)?;
        file.sync_all()?;
        Ok(())
    }
}

/// How a segment file is written.
///
/// Three settings, which is what is left of a struct that once carried
/// twenty-four: the rest described a writer that no longer exists -- its
/// buffer, its shards, its redo log, its freelist, its checkpoint policy --
/// and nothing read them. `Options::segment` carries one of these to the
/// writer for every piece the engine seals or merges.
///
/// Compression is deliberately not here. It is a property of one file rather
/// than of the engine's configuration, so `SegmentWriter::set_compress` takes
/// it; a field on this struct would be read by nothing and would silently do
/// nothing, which is how `tests/ranges.rs` came to check the plain path while
/// claiming to check the compressed one.
#[derive(Clone, Debug)]
pub struct SegmentOptions {
    /// Target size of a compression block. Bigger compresses better and costs
    /// more to decompress on a point read; this is the size/read dial.
    pub block_size: usize,
    /// Compute and verify block checksums.
    ///
    /// On by default: without it a bit flip, a torn write or a reused slot
    /// returns silently wrong data, because LZ4 decodes many corrupted inputs
    /// into plausible bytes. The knob exists so the cost can be measured
    /// honestly -- both arms in one process, interleaved -- rather than by
    /// comparing two runs taken hours apart, which measures the machine as
    /// much as the code (f8-checksums).
    pub checksums: bool,
    /// Sort and encode the key index across threads rather than on one.
    ///
    /// On by default. The sort splits and merges, the record loop splits
    /// because `rec_offs` is a prefix sum, and the hash claims slots with
    /// compare-exchange.
    pub parallel_index: bool,
}

impl Default for SegmentOptions {
    fn default() -> SegmentOptions {
        SegmentOptions {
            block_size: 64 * 1024,
            checksums: true,
            parallel_index: true,
        }
    }
}

/// How a piece gets written.
///
/// This was an enum: `SegmentWriter`, or the general `Store` path it
/// replaced, kept behind an option so f49 could interleave
/// the two in one process and price the change honestly. That comparison is
/// settled and the old path is gone, so what is left is a thin shim that
/// keeps `flush` and `merge` reading as a sequence of begin/value/end calls.
struct PieceWriter(Box<SegmentWriter>);

impl PieceWriter {
    fn create(
        path: &Path,
        opts: &SegmentOptions,
        sync_every: usize,
        inline_max: usize,
    ) -> Result<PieceWriter> {
        let mut w = SegmentWriter::create(path, opts)?;
        w.set_sync_every(sync_every);
        w.set_inline_max(inline_max);
        Ok(PieceWriter(Box::new(w)))
    }

    fn begin(&mut self, k: &[u8]) -> Result<()> {
        self.0.begin(k)
    }

    /// Infallible at the call so it can sit inside a read callback.
    fn value(&mut self, v: &[u8]) {
        self.0.value(v)
    }

    fn end_with(&mut self, tombstone: bool) -> Result<()> {
        self.0.end_with(tombstone)
    }

    fn finish(self) -> Result<()> {
        (*self.0).finish(1)
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

    fn open(dir: &Path, name: &str, advise_random: bool) -> Result<Seg> {
        let src = MmapBytes::open(&dir.join(name)).map_err(|e| {
            // A manifest naming a segment that is not on disk is a damaged
            // store, not a missing file, and saying so is the difference
            // between a diagnosis and an ENOENT.
            err(&format!(
                "the manifest names segment {name}, which is not in the store: {e}"
            ))
        })?;
        let blob = Blob::open(src).map_err(|e| err(&format!("segment {name}: {e}")))?;
        if advise_random {
            blob.advise_random();
        }
        // `pcs-` is a range-ALIGNED L0 piece: a seal split at the live
        // partition boundaries, so it carries a fence like a partition and
        // overlaps only the pieces of its own range. That alignment is what
        // makes a merge O(range) instead of O(store).
        if let Some(rest) = name
            .strip_prefix("pcs-")
            .and_then(|r| r.strip_suffix(".sup"))
        {
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
        if let Some(rest) = name
            .strip_prefix("par-")
            .and_then(|r| r.strip_suffix(".sup"))
        {
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

    /// `live_chain` into a caller's buffer, oldest first, allocating nothing
    /// after the buffer has grown once; returns whether a tombstone cut it.
    fn live_offs_into(&self, e: &MemEntry, out: &mut Vec<usize>) -> bool {
        out.clear();
        let mut at = e.head - 1;
        let mut tomb = false;
        while at != NO_CHUNK {
            if self.is_tomb(at as usize) {
                tomb = true;
                break;
            }
            out.push(at as usize);
            at = u64::from_le_bytes(self.vals[at as usize..at as usize + 8].try_into().unwrap());
        }
        out.reverse();
        tomb
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
    let body = buf
        .get(17..17 + len)
        .ok_or_else(|| err("manifest is truncated"))?;
    if crc32(body) != crc {
        return Err(err("manifest failed its checksum"));
    }
    let covered = u64::from_le_bytes(body[..8].try_into().unwrap());
    let n = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
    let mut names = Vec::with_capacity(n);
    let mut p = 12usize;
    for _ in 0..n {
        let l = u16::from_le_bytes(
            body.get(p..p + 2)
                .ok_or_else(|| err("manifest is truncated"))?
                .try_into()
                .unwrap(),
        ) as usize;
        p += 2;
        let raw = body
            .get(p..p + l)
            .ok_or_else(|| err("manifest is truncated"))?;
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
    opts: SegmentOptions,
    cursors: bool,
    background_io: BackgroundIo,
    sync_every: usize,
    inline_max: usize,
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
        KeyList {
            bytes: Vec::new(),
            offs: vec![0],
        }
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
    opts: &'a SegmentOptions,
    sync_every: usize,
    inline_max: usize,
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
                PieceWriter::create(&tmp, self.opts, self.sync_every, self.inline_max)
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
            w.finish()
                .map_err(|e| err(&format!("compact finish: {e}")))?;
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
        cursors,
        background_io,
        sync_every,
        inline_max,
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
                    let sub_lo = if i == 0 {
                        lo.clone()
                    } else {
                        fence_lo(keys.get(sf))
                    };
                    let sub_hi = if st == to {
                        hi.clone()
                    } else {
                        Some(fence_lo(keys.get(st)))
                    };
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
                if pi == 0 {
                    Vec::new()
                } else {
                    bounds[pi - 1].clone()
                },
                if pi + 1 == slices.len() {
                    None
                } else {
                    Some(bounds[pi].clone())
                },
            ),
        };
        let name = format!(
            "par-{id:08}-{end_seq:016}-{}-{}.sup",
            hex(&lo),
            hi.as_deref().map(hex).unwrap_or_default()
        );
        let tmp = dir.join(format!("compact-{id:08}.tmp"));
        pieces.push(Piece {
            from,
            to,
            lo,
            hi,
            name,
            tmp,
        });
    }

    // Pass two: values, in rank order, into one piece per slice.
    let mut em = Emitter {
        dir: &dir,
        opts: &opts,
        sync_every,
        inline_max,
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
            let mut found: Vec<(usize, &[Ext], &[u8])> = Vec::with_capacity(blobs.len());
            let mut start = 0usize;
            let mut live = 0u64;
            for (i, b) in blobs.iter().enumerate() {
                if let Some((exts, tail)) = b.lookup_full(k) {
                    if exts.iter().any(|e| e.is_tombstone()) {
                        start = found.len();
                        live = 0;
                    }
                    live += exts.iter().map(|e| u64::from(e.records())).sum::<u64>();
                    found.push((i, exts, tail));
                }
            }
            if live == 0 {
                em.skip()?;
                continue;
            }
            em.key(k, |w| {
                for &(i, exts, tail) in &found[start..] {
                    blobs[i]
                        .read_exts(exts, tail, |v| w.value(v))
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

/// Where the commit thread's seal time goes. `phase_ns().1` is the sum of
/// everything `join_seal` does; this says how much of it was waiting for a
/// seal thread that had not finished when the next seal came due
/// (`join_wait_ns`, over `blocked_joins` such joins), how much was the
/// final drain a `flush` performs (`drain_wait_ns`), and how much was
/// publishing the manifest with its two barriers (`publish_ns`). `joins`
/// counts seals joined.
#[derive(Clone, Copy, Debug, Default)]
pub struct SealWaits {
    pub join_wait_ns: u64,
    pub drain_wait_ns: u64,
    pub publish_ns: u64,
    pub blocked_joins: u64,
    pub joins: u64,
}

pub struct Db {
    dir: PathBuf,
    opts: Options,
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
    /// The seal phase decomposed: how long the commit thread blocked on a
    /// seal still running mid-load, how long the final drain took, how long
    /// publishing (the manifest and its barriers) took, and how often a
    /// join found the seal unfinished. f60 asks which of these the 14% of
    /// the durable load in `phase_ns[1]` is (sealwait-plan.md).
    seal_wait: SealWaits,
    /// Set by `flush` while it waits for the last seal, so that wait is
    /// booked as the drain and not as backpressure.
    draining: bool,
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
    /// Retired WAL files kept for the next rotation (`recycle_wal`), under
    /// `spare-` names so `open` never replays them. One is enough: a
    /// retiring WAL is released at `join_seal`, and a seal joins the one
    /// before it before rotating.
    spare_wals: Vec<PathBuf>,
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
    scan_keys: std::cell::RefCell<Option<(u64, Snapshot)>>,
}

/// One key of the unsealed snapshot a scan merges: where the key sits in
/// the snapshot's arena, and where it sits in the live and the frozen
/// memtable (`u32::MAX` for absent), so emitting it is an indexed chain
/// walk and not a hash probe per table.
#[derive(Clone, Copy)]
struct SnapKey {
    off: u32,
    len: u32,
    mem: u32,
    frozen: u32,
}

/// The sorted keys of the unsealed sources, built lazily by `Db::scan` and
/// kept until the next commit or seal. Keys live in one arena rather than
/// one allocation each, which is what makes the build a sort of small
/// records instead of a pointer chase (scansnap-plan.md).
#[derive(Default)]
struct Snapshot {
    keys: Vec<u8>,
    ents: Vec<SnapKey>,
}

impl Snapshot {
    fn len(&self) -> usize {
        self.ents.len()
    }
    fn get(&self, i: usize) -> Option<(&[u8], &SnapKey)> {
        self.ents
            .get(i)
            .map(|e| (&self.keys[e.off as usize..(e.off + e.len) as usize], e))
    }
    /// First index whose key is not below `from`.
    fn seek(&self, from: &[u8]) -> usize {
        self.ents
            .partition_point(|e| &self.keys[e.off as usize..(e.off + e.len) as usize] < from)
    }
    /// Merge a run of entries sorted by key into `ents`, folding a key
    /// present in both tables into one entry carrying both indices.
    fn push_sorted(&mut self, e: SnapKey) {
        if let Some(last) = self.ents.last_mut() {
            let same = self.keys[last.off as usize..(last.off + last.len) as usize]
                == self.keys[e.off as usize..(e.off + e.len) as usize];
            if same {
                if e.mem != u32::MAX {
                    last.mem = e.mem;
                }
                if e.frozen != u32::MAX {
                    last.frozen = e.frozen;
                }
                return;
            }
        }
        self.ents.push(e);
    }
}

/// LSD radix sort of `(key, a, b)` triples by the key, two 16-bit passes.
/// Stable, O(n), and what puts a hash table's entries back into the order
/// their keys were appended so the copy that follows is sequential.
fn radix_by_first(v: &mut Vec<(u32, u32, u32)>, scratch: &mut Vec<(u32, u32, u32)>) {
    scratch.clear();
    scratch.resize(v.len(), (0, 0, 0));
    for shift in [0u32, 16] {
        let mut counts = vec![0usize; 1 << 16];
        for &(k, _, _) in v.iter() {
            counts[((k >> shift) & 0xFFFF) as usize] += 1;
        }
        let mut sum = 0usize;
        for c in counts.iter_mut() {
            let n = *c;
            *c = sum;
            sum += n;
        }
        for &t in v.iter() {
            let b = ((t.0 >> shift) & 0xFFFF) as usize;
            scratch[counts[b]] = t;
            counts[b] += 1;
        }
        std::mem::swap(v, scratch);
    }
}

/// The first sixteen bytes of a key as two big-endian words, zero-padded,
/// so that comparing the words compares the keys wherever they differ
/// inside that prefix.
fn key_prefix(k: &[u8]) -> (u64, u64) {
    let mut b = [0u8; 16];
    let n = k.len().min(16);
    b[..n].copy_from_slice(&k[..n]);
    (
        u64::from_be_bytes(b[..8].try_into().unwrap()),
        u64::from_be_bytes(b[8..].try_into().unwrap()),
    )
}

impl Db {
    /// WAL files are numbered and rotate at each seal: the sealing thread
    /// owns the old file and deletes it once its segment is renamed into
    /// place, while commits continue into the next file. Replay walks them
    /// in id order; sequence numbers are continuous across the boundary.
    fn wal_path(dir: &Path, id: u64) -> PathBuf {
        dir.join(format!("wal-{id:08}"))
    }

    fn spare_path(dir: &Path, id: u64) -> PathBuf {
        dir.join(format!("spare-{id:08}"))
    }

    /// The new live WAL for a rotation: a recycled retiree when the pool
    /// has one, else a fresh file.
    fn next_wal(&mut self, id: u64) -> Result<Wal> {
        let path = Db::wal_path(&self.dir, id);
        if self.opts.recycle_wal {
            if let Some(spare) = self.spare_wals.pop() {
                return Wal::recycle(&spare, &path, id);
            }
            let mut wal = Wal::create(&path, id)?;
            wal.prefill(self.opts.seal_bytes as u64)?;
            return Ok(wal);
        }
        Wal::create(&path, id)
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

    fn segment_opts(opts: &Options) -> SegmentOptions {
        opts.segment.clone()
    }

    pub fn create(dir: &Path, opts: Options) -> Result<Db> {
        std::fs::create_dir_all(dir)?;
        let mut wal = Wal::create(&Db::wal_path(dir, 0), 0)?;
        let mut spare_wals = Vec::new();
        if opts.recycle_wal {
            // The live file and one spare, both written through, so the
            // first rotation recycles too and no rotation ever pays the
            // pre-write on the commit path.
            wal.prefill(opts.seal_bytes as u64)?;
            let spare = Db::spare_path(dir, 0);
            Wal::create(&spare, 0)?.prefill(opts.seal_bytes as u64)?;
            spare_wals.push(spare);
            File::open(dir)?.sync_all()?;
        }
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
            spare_wals,
            covered_seq: 0,
            scan_keys: std::cell::RefCell::new(None),
            seal_wait: SealWaits::default(),
            draining: false,
        })
    }

    /// Open from the directory alone. Segments are complete by construction
    /// (they were renamed into place after their fsync); the WAL replays
    /// whatever outlived the last seal, torn tail tolerated. A directory
    /// with no segments and only a WAL is a store killed before its first
    /// seal, and it opens -- the brief's P-E.
    pub fn open(dir: &Path, opts: Options) -> Result<Db> {
        // The manifest is the truth when it exists. Without one -- a store
        // killed before its first seal -- the directory is scanned, which
        // is also how a store written before manifests still opens.
        let mut on_disk: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if (name.starts_with("seg-") || name.starts_with("par-") || name.starts_with("pcs-"))
                && name.ends_with(".sup")
            {
                on_disk.push(name);
            }
        }
        let (sealed, live) = match manifest_read(dir)? {
            Some((covered, names)) => (covered, names),
            None => {
                let mut names: Vec<String> = on_disk
                    .iter()
                    .filter(|n| n.starts_with("seg-"))
                    .cloned()
                    .collect();
                names.sort_unstable();
                let covered = names.last().and_then(|n| Db::name_end_seq(n)).unwrap_or(0);
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
            segs.push(Seg::open(dir, name, opts.advise_random)?);
        }
        segs.sort_by(|a, b| {
            b.level
                .cmp(&a.level)
                .then_with(|| a.lo.cmp(&b.lo))
                .then_with(|| a.name.cmp(&b.name))
        });
        let seg_ids: Vec<(u64, u64)> = live
            .iter()
            .filter_map(|n| Some((Db::name_id(n)?, Db::name_end_seq(n)?)))
            .collect();
        let mut wal_ids: Vec<u64> = Vec::new();
        let mut spare_wals: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_prefix("wal-") {
                wal_ids.push(id.parse().map_err(|_| err("wal file name is malformed"))?);
            } else if name.starts_with("spare-") {
                // A retired WAL kept for recycling. It holds nothing a
                // segment does not, so it is either the pool or garbage.
                if opts.recycle_wal && spare_wals.is_empty() {
                    spare_wals.push(dir.join(name.as_ref()));
                } else {
                    let _ = std::fs::remove_file(dir.join(name.as_ref()));
                }
            }
        }
        wal_ids.sort_unstable();
        let mut mem = MemTable::new();
        let mut mem_bytes = 0usize;
        let mut from = sealed;
        let mut valid_len = 0u64;
        for &id in &wal_ids {
            let (next, valid) = Wal::replay(&Db::wal_path(dir, id), id, from, |kind, k, v| {
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
        if let Ok(md) = std::fs::metadata(&wal_path) {
            if md.len() > valid_len {
                let f = OpenOptions::new().write(true).open(&wal_path)?;
                f.set_len(valid_len)?;
                f.sync_data()?;
            }
        }
        let wal = Wal::open_append(&wal_path, wal_id, from)?;
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
            spare_wals,
            covered_seq: sealed,
            scan_keys: std::cell::RefCell::new(None),
            seal_wait: SealWaits::default(),
            draining: false,
        })
    }

    /// Buffered until `commit`; visible to this handle's reads immediately,
    /// which is the read-your-writes contract `Store::read_all` set.
    pub fn append(&mut self, key: &[u8], value: &[u8]) {
        self.wal.append(key, value);
        self.mem.append(key, value);
        self.mem_bytes += key.len() + value.len();
    }

    /// Replace a key's values with one new value: a delete and an append in
    /// the same batch, so a read after the commit sees the new value alone
    /// and a crash sees both or neither. This is the update the external
    /// suite's YCSB phase means, and what `Store::put` and every
    /// single-value engine there do; `append` is the other verb, and using
    /// it for an update piled every Zipfian rewrite onto its key until each
    /// read walked the pile (ycsb-plan.md).
    pub fn put(&mut self, key: &[u8], value: &[u8]) {
        self.delete(key);
        self.append(key, value);
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
        Txn {
            db: self,
            ops: Vec::new(),
        }
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
        let new_wal = self.next_wal(self.wal_id + 1)?;
        let old_wal = std::mem::replace(&mut self.wal, new_wal);
        // The new file's directory entry is made durable now, not at the
        // end of the seal: commits into it are acknowledged from here on,
        // and an fdatasync of the file does not promise the entry that
        // names it. One directory barrier per seal, off the per-commit path.
        File::open(&self.dir)?.sync_all()?;
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
        let background_io = self.opts.background_io;
        let sync_every = self.opts.seal_sync_every;
        let inline_max = self.opts.inline_bytes;
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
            // written sorted, a scan walks it forwards. This is what the
            // retired line-order arm of w1-daysize found, in the new engine -- how the roll writes decides what
            // the read costs -- and the sort is affordable because a seal is
            // off the commit path. The same sort is what makes splitting at
            // the fences a matter of slicing.
            let mut order: Vec<&MemEntry> = mem.entries.iter().filter(|e| e.hash != 0).collect();
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
                    let mut w = PieceWriter::create(&tmp, &opts, sync_every, inline_max)
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

    /// Wait for whatever seal and merge are in flight, starting nothing new
    /// (a joined seal may still trigger a merge when compaction is on and
    /// the level-0 count says so). For an experiment that wants a store in a
    /// known shape before it measures.
    pub fn settle(&mut self) -> Result<()> {
        self.join_seal()?;
        self.join_compact()
    }

    /// Make everything written durable and seal nothing: the WAL's pending
    /// frames written and fsynced, the memtable left where it is. What a
    /// caller wants when it has stopped writing for now and will read the
    /// tail out of memory; `flush` is the other answer, and f60 priced the
    /// difference at 11% of a canonical load window (sealwait-plan.md).
    pub fn sync(&mut self) -> Result<()> {
        self.wal.commit()?;
        self.unsynced = 0;
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
        self.draining = true;
        let sealed = self.seal().and_then(|_| self.join_seal());
        self.draining = false;
        sealed?;
        self.join_compact()?;
        // Leave the store routed. A flush is a caller saying it has
        // stopped writing, and what it leaves behind otherwise is a set of
        // OVERLAPPING full-range segments -- each one costing every
        // subsequent read a Bloom check, because nothing tells them apart.
        // Partitioning them costs one merge now and makes every later read
        // touch exactly one segment, which is the arrangement the read
        // lead was measured in.
        if !(self.opts.compact && self.opts.partition_on_flush) {
            return Ok(());
        }
        // With `flush_ranges`, each round merges only the ranges that hold
        // pieces, under the live fences -- one piece is enough to be due
        // here, where the background trigger waits for several -- so a
        // flush after an ordered or skewed load rewrites the partitions it
        // touched and not the store. Without it, or before the first
        // partitioning, everything is re-partitioned from every key.
        let mut rounds = 0usize;
        while self.segs.iter().any(|s| s.level == 0) {
            let plan = if self.opts.flush_ranges {
                self.merge_due(1)
            } else {
                None
            };
            match plan {
                Some(fences) if !fences.is_empty() => {
                    let fences = if self.opts.promote {
                        self.promote_ranges(fences)?
                    } else {
                        fences
                    };
                    if !fences.is_empty() {
                        self.start_compact(Some(fences))?;
                    }
                }
                Some(_) => {}
                None => {
                    if !(self.opts.promote && self.promote_unpartitioned()?) {
                        self.start_compact(None)?;
                    }
                }
            }
            self.join_compact()?;
            rounds += 1;
            if rounds > 64 {
                return Err(err("flush: level 0 did not drain in 64 merge rounds"));
            }
        }
        Ok(())
    }

    /// The fences a range merge should rewrite now, or `None` when the store
    /// is not partitioned yet (the first partitioning takes every key). A
    /// piece that is not aligned to a live range -- sealed during the first
    /// partitioning against fences that no longer exist -- selects every
    /// range it overlaps; otherwise a range is selected when it holds at
    /// least `threshold` pieces. `maybe_compact` uses the trigger as the
    /// threshold; a flush uses one.
    fn merge_due(&self, threshold: usize) -> Option<Vec<Fence>> {
        let parts: Vec<Fence> = self
            .segs
            .iter()
            .filter(|s| s.level > 0)
            .map(|s| (s.lo.clone(), s.hi.clone()))
            .collect();
        if parts.is_empty() {
            return None;
        }
        if let Some(wide) = self
            .segs
            .iter()
            .find(|s| s.level == 0 && !parts.iter().any(|f| (s.lo.clone(), s.hi.clone()) == *f))
        {
            let (wlo, whi) = (wide.lo.clone(), wide.hi.clone());
            return Some(
                parts
                    .into_iter()
                    .filter(|(lo, hi)| {
                        let below = hi.as_ref().is_some_and(|h| &wlo >= h);
                        let above = whi.as_ref().is_some_and(|h| h <= lo);
                        !below && !above
                    })
                    .collect(),
            );
        }
        Some(
            parts
                .into_iter()
                .filter(|f| {
                    self.segs
                        .iter()
                        .filter(|s| s.level == 0 && s.lo == f.0 && s.hi == f.1)
                        .count()
                        >= threshold
                })
                .collect(),
        )
    }

    /// Collect a finished (or in-flight) seal: join the thread, open its
    /// segment, retire the frozen memtable.
    fn join_seal(&mut self) -> Result<()> {
        let Some(handle) = self.sealing.take() else {
            return Ok(());
        };
        let t = std::time::Instant::now();
        let blocked = !handle.is_finished();
        let names = handle.join().map_err(|_| err("seal thread panicked"))??;
        let waited = t.elapsed().as_nanos() as u64;
        self.seal_wait.joins += 1;
        if self.draining {
            self.seal_wait.drain_wait_ns += waited;
        } else if blocked {
            self.seal_wait.join_wait_ns += waited;
            self.seal_wait.blocked_joins += 1;
        }
        for name in &names {
            self.covered_seq = self.covered_seq.max(Db::name_end_seq(name).unwrap_or(0));
            self.segs
                .push(Seg::open(&self.dir, name, self.opts.advise_random)?);
        }
        self.sort_segs();
        self.frozen = None;
        let tp = std::time::Instant::now();
        self.publish()?;
        self.seal_wait.publish_ns += tp.elapsed().as_nanos() as u64;
        for old in std::mem::take(&mut self.retiring_wals) {
            if self.opts.recycle_wal && self.spare_wals.is_empty() {
                let id = old
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("wal-"))
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
                let spare = Db::spare_path(&self.dir, id);
                if std::fs::rename(&old, &spare).is_ok() {
                    self.spare_wals.push(spare);
                    continue;
                }
            }
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
        if self
            .compacting
            .as_ref()
            .is_some_and(|(_, h)| h.is_finished())
        {
            self.join_compact()?;
        }
        // One selection rule for both schedulers, so they cannot drift: a
        // range is due when it holds `l0_trigger` pieces, a piece not
        // aligned to the live ranges selects every range it overlaps, and
        // before the first partitioning the trigger counts every piece.
        match self.merge_due(self.opts.l0_trigger) {
            None => {
                if self.l0_len() >= self.opts.l0_trigger {
                    if self.opts.promote && self.promote_unpartitioned()? {
                        return Ok(());
                    }
                    return self.start_compact(None);
                }
                Ok(())
            }
            Some(due) if !due.is_empty() => {
                let due = if self.opts.promote {
                    self.promote_ranges(due)?
                } else {
                    due
                };
                if due.is_empty() {
                    return Ok(());
                }
                self.start_compact(Some(due))
            }
            Some(_) => Ok(()),
        }
    }

    /// Try to promote the aligned pieces of each of `due`'s ranges instead
    /// of merging them; return the ranges that still need a merge.
    ///
    /// A range qualifies when its partition's last key lies below every
    /// piece's first key and the pieces are disjoint in key order. Then the
    /// partition keeps its data and its fence closes at the first piece's
    /// first key, and each piece becomes a partition running to the next
    /// piece's first key, the last inheriting the range's upper fence. A
    /// piece and a partition are the same file from the same writer; only
    /// the name and the level differ, so this is hard links, one manifest
    /// write, and the old names unlinked -- in that order, so a crash on
    /// either side of the manifest leaves exactly one complete set for the
    /// orphan sweep to reconcile.
    fn promote_ranges(&mut self, due: Vec<Fence>) -> Result<Vec<Fence>> {
        let mut rest = Vec::new();
        for f in due {
            let part = self
                .segs
                .iter()
                .position(|s| s.level > 0 && s.lo == f.0 && s.hi == f.1);
            let mut pieces: Vec<usize> = self
                .segs
                .iter()
                .enumerate()
                .filter(|(_, s)| s.level == 0 && s.lo == f.0 && s.hi == f.1)
                .map(|(i, _)| i)
                .collect();
            let Some(pi) = part else {
                rest.push(f);
                continue;
            };
            // The partition's last key, or nothing if it is empty.
            let floor: Option<Vec<u8>> = {
                let b = &self.segs[pi].blob;
                if b.keys() == 0 {
                    None
                } else {
                    b.key_at(b.keys() - 1).map(|k| k.to_vec())
                }
            };
            match self.promotion_chain(&f, floor, &mut pieces) {
                Some(bounds) => {
                    // Piece i takes (bounds[i], bounds[i+1]); the partition
                    // keeps its low fence and closes at bounds[0].
                    let mut renames: Vec<(usize, Fence)> = Vec::with_capacity(pieces.len() + 1);
                    renames.push((pi, (f.0.clone(), Some(bounds[0].clone()))));
                    for (j, &si) in pieces.iter().enumerate() {
                        let hi = if j + 1 < pieces.len() {
                            Some(bounds[j + 1].clone())
                        } else {
                            f.1.clone()
                        };
                        renames.push((si, (bounds[j].clone(), hi)));
                    }
                    self.apply_promotion(renames)?;
                }
                None => rest.push(f),
            }
        }
        Ok(rest)
    }

    /// Before the first partitioning: if the full-range segments are
    /// disjoint in key order, they become the first partitions as they
    /// are, tiling the space from the bottom.
    fn promote_unpartitioned(&mut self) -> Result<bool> {
        let mut pieces: Vec<usize> = self
            .segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.level == 0)
            .map(|(i, _)| i)
            .collect();
        if pieces.len() < 2 {
            return Ok(false);
        }
        let whole: Fence = (Vec::new(), None);
        let Some(bounds) = self.promotion_chain(&whole, None, &mut pieces) else {
            return Ok(false);
        };
        let mut renames: Vec<(usize, Fence)> = Vec::with_capacity(pieces.len());
        for (j, &si) in pieces.iter().enumerate() {
            let lo = if j == 0 {
                Vec::new()
            } else {
                bounds[j].clone()
            };
            let hi = if j + 1 < pieces.len() {
                Some(bounds[j + 1].clone())
            } else {
                None
            };
            renames.push((si, (lo, hi)));
        }
        self.apply_promotion(renames)?;
        Ok(true)
    }

    /// Order `pieces` by first key and check the chain: every piece's first
    /// key, taken as a fence, must lie strictly above what came before it
    /// (the partition's last key, then the previous piece's last key) and
    /// inside the range. Returns each piece's fence boundary, or `None` when
    /// something overlaps and a merge is what is needed.
    fn promotion_chain(
        &self,
        range: &Fence,
        floor: Option<Vec<u8>>,
        pieces: &mut [usize],
    ) -> Option<Vec<Vec<u8>>> {
        if pieces.is_empty() {
            return None;
        }
        let first_of = |si: usize| -> Option<Vec<u8>> {
            let b = &self.segs[si].blob;
            if b.keys() == 0 {
                None
            } else {
                b.key_at(0).map(|k| k.to_vec())
            }
        };
        let last_of = |si: usize| -> Option<Vec<u8>> {
            let b = &self.segs[si].blob;
            if b.keys() == 0 {
                None
            } else {
                b.key_at(b.keys() - 1).map(|k| k.to_vec())
            }
        };
        // An empty piece has nothing to promote; leave it to the merge.
        if pieces.iter().any(|&si| self.segs[si].blob.keys() == 0) {
            return None;
        }
        pieces.sort_by_key(|&si| first_of(si));
        let mut bounds = Vec::with_capacity(pieces.len());
        let mut prev_last: Option<Vec<u8>> = floor;
        for &si in pieces.iter() {
            let first = first_of(si)?;
            let b = fence_lo(&first);
            // Strictly above everything before it, and inside the range.
            if let Some(pl) = &prev_last {
                if *pl >= b {
                    return None;
                }
            }
            if b < range.0 || range.1.as_ref().is_some_and(|h| &b >= h) {
                return None;
            }
            bounds.push(b);
            prev_last = last_of(si);
        }
        Some(bounds)
    }

    /// Give each segment its new fence and level by hard link, publish,
    /// then unlink the old names.
    fn apply_promotion(&mut self, renames: Vec<(usize, Fence)>) -> Result<()> {
        let mut old_names = Vec::with_capacity(renames.len());
        for (si, (lo, hi)) in renames {
            let old = self.segs[si].name.clone();
            // Keep the id and covered-sequence fields verbatim; only the
            // prefix and the fences change.
            let stem = old.trim_end_matches(".sup");
            let fields: Vec<&str> = stem.split('-').collect();
            if fields.len() < 3 {
                return Err(err("promotion: segment name is malformed"));
            }
            let new = format!(
                "par-{}-{}-{}-{}.sup",
                fields[1],
                fields[2],
                hex(&lo),
                hi.as_deref().map(hex).unwrap_or_default()
            );
            if new == old {
                continue;
            }
            std::fs::hard_link(self.dir.join(&old), self.dir.join(&new))?;
            let seg = &mut self.segs[si];
            seg.name = new;
            seg.lo = lo;
            seg.hi = hi;
            seg.level = 1;
            seg.bloom = None;
            old_names.push(old);
        }
        File::open(&self.dir)?.sync_all()?;
        self.sort_segs();
        self.publish()?;
        for old in old_names {
            let _ = std::fs::remove_file(self.dir.join(old));
        }
        Ok(())
    }
    // The starvation lesson that shaped `merge_due`: EVERY range that is
    // over its bound merges in one job, not just the worst. A per-range
    // merge has to run once per range where the whole-store merge ran once,
    // so picking a single range per seal starved it -- with sixteen ranges
    // and one merge in flight, pieces accumulated faster than they were
    // consumed and a read ended up walking ten of them. That starvation
    // cost more than the whole-store rewrite it replaced (EXT.23 0.846x ->
    // 0.561x), which is the measurement that produced the rule.

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
        let pb = self
            .opts
            .partition_bytes
            .unwrap_or(self.opts.seal_bytes)
            .max(1);
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
        let cursors = self.opts.cursor_merge;
        let background_io = self.opts.background_io;
        let sync_every = self.opts.seal_sync_every;
        let inline_max = self.opts.inline_bytes;
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
                cursors,
                background_io,
                sync_every,
                inline_max,
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
        let outputs = handle
            .join()
            .map_err(|_| err("compaction thread panicked"))??;
        let mut kept: Vec<Seg> = Vec::new();
        for seg in self.segs.drain(..) {
            if !inputs.contains(&seg.name) {
                kept.push(seg);
            }
        }
        let mut merged = Vec::with_capacity(outputs.len());
        for name in &outputs {
            merged.push(Seg::open(&self.dir, name, self.opts.advise_random)?);
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
        let at =
            self.segs[..np].partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
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

    /// The sorted snapshot of every unsealed key (frozen table first, then
    /// live), one entry per key. Two builds behind `scan_snapshot_arena`;
    /// both walk the hash tables once and both end in the same `Snapshot`.
    fn build_snapshot(&self) -> Snapshot {
        let n = self.mem.len + self.frozen.as_ref().map_or(0, |f| f.len);
        let mut snap = Snapshot {
            keys: Vec::with_capacity(
                self.mem.keys.len() + self.frozen.as_ref().map_or(0, |f| f.keys.len()),
            ),
            ents: Vec::with_capacity(n),
        };
        if self.opts.scan_snapshot_arena {
            // Arena build. The hash table is walked in slot order, which
            // visits the key bytes in random order -- one cache miss a key,
            // and at 428k keys that walk, not the sort, was most of the
            // build. So the walk records (key offset, slot) without touching
            // a key, a radix pass puts them in arena order, and the copy
            // into the snapshot's arena is sequential. Then sort (prefix,
            // prefix, index) records, touching the arena only on a tie.
            let mut recs: Vec<(u64, u64, u32)> = Vec::with_capacity(n);
            let mut pending: Vec<SnapKey> = Vec::with_capacity(n);
            let mut order: Vec<(u32, u32, u32)> = Vec::with_capacity(n);
            let mut scratch: Vec<(u32, u32, u32)> = Vec::with_capacity(n);
            let mut take = |mem: &MemTable, live: bool| {
                // (key offset, key length, slot): the copy below needs no
                // slot access, since a slot in key order is a random one.
                order.clear();
                order.extend(
                    mem.entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.hash != 0)
                        .map(|(i, e)| (e.key_off, e.key_len, i as u32)),
                );
                radix_by_first(&mut order, &mut scratch);
                for &(off, len, i) in &order {
                    let k = &mem.keys[off as usize..(off + len) as usize];
                    let (a, b) = key_prefix(k);
                    recs.push((a, b, pending.len() as u32));
                    pending.push(SnapKey {
                        off: snap.keys.len() as u32,
                        len: k.len() as u32,
                        mem: if live { i } else { u32::MAX },
                        frozen: if live { u32::MAX } else { i },
                    });
                    snap.keys.extend_from_slice(k);
                }
            };
            if let Some(fr) = &self.frozen {
                take(fr, false);
            }
            take(&self.mem, true);
            let keys = &snap.keys;
            let key_of = |e: &SnapKey| &keys[e.off as usize..(e.off + e.len) as usize];
            recs.sort_unstable_by(|x, y| {
                (x.0, x.1).cmp(&(y.0, y.1)).then_with(|| {
                    key_of(&pending[x.2 as usize])
                        .cmp(key_of(&pending[y.2 as usize]))
                        // Frozen entries were pushed first; on a tie the
                        // live one must come later so the fold sees it.
                        .then(x.2.cmp(&y.2))
                })
            });
            for r in recs {
                snap.push_sorted(pending[r.2 as usize]);
            }
        } else {
            // The build before it: one allocation per key, sorted through
            // the pointers, then copied into the arena the merge expects.
            struct Old {
                key: Vec<u8>,
                mem: u32,
                frozen: u32,
            }
            let mut all: Vec<Old> = Vec::with_capacity(n);
            let mut take = |mem: &MemTable, live: bool| {
                for (i, e) in mem.entries.iter().enumerate().filter(|(_, e)| e.hash != 0) {
                    all.push(Old {
                        key: MemTable::key_of(&mem.keys, e).to_vec(),
                        mem: if live { i as u32 } else { u32::MAX },
                        frozen: if live { u32::MAX } else { i as u32 },
                    });
                }
            };
            if let Some(fr) = &self.frozen {
                take(fr, false);
            }
            take(&self.mem, true);
            all.sort_by(|a, b| a.key.cmp(&b.key));
            for o in all {
                let off = snap.keys.len() as u32;
                snap.keys.extend_from_slice(&o.key);
                snap.push_sorted(SnapKey {
                    off,
                    len: o.key.len() as u32,
                    mem: o.mem,
                    frozen: o.frozen,
                });
            }
        }
        snap
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
                *cache = Some((gen, self.build_snapshot()));
            }
        }
        let cache = self.scan_keys.borrow();
        let unsealed = &cache.as_ref().expect("scan snapshot").1;
        let mut mi = unsealed.seek(from);

        // When nothing overlaps -- no unsealed keys in range, no L0 -- the
        // partitions ARE the answer in key order, and each one can be
        // walked by `Blob::scan`, which resolves each key once. The merge
        // below costs five or six index lookups an entry (a key_at per
        // cursor to find the minimum, another to emit, and a third inside
        // `values_at`) where this costs one, and after a routed flush this
        // is the shape the store is in. An earlier version had this path,
        // a refactor dropped it, and the scan axis paid for it.
        if mi >= unsealed.len() && !self.segs.iter().any(|s| s.level == 0) {
            let mut parts: Vec<&Seg> = self.segs.iter().filter(|s| s.may_reach(from)).collect();
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

        if self.opts.scan_merge {
            return self.scan_merged(from, limit, mi, unsealed, f);
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
                let start = if s.lo.as_slice() > from {
                    s.lo.as_slice()
                } else {
                    from
                };
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
            if let Some((k, _)) = unsealed.get(mi) {
                if next.is_none_or(|n| k < n) {
                    next = Some(k);
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
            let in_unsealed = unsealed.get(mi).map(|(k, _)| k) == Some(key);
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

    /// The merge over unrouted sources, f62's arm: one cursor walking the
    /// disjoint partitions in order, one cursor per level-0 segment, and the
    /// unsealed snapshot with each key's entries in hand. Every cursor's key
    /// is resolved once per emitted key. Sources are ordered oldest to
    /// newest -- the partition, level 0 oldest first, the frozen memtable,
    /// the live one -- and a tombstone in the newest source that holds the
    /// key cuts everything older, as in `read_all`.
    fn scan_merged<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        mut mi: usize,
        unsealed: &Snapshot,
        mut f: F,
    ) -> Result<usize> {
        let np = self.segs.partition_point(|s| s.level > 0);
        let parts = &self.segs[..np];
        // The partition cursor: the first partition whose fence can reach
        // `from`, then each following one from its first key.
        let mut pi = parts.partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= from));
        let mut prank = 0usize;
        let mut pkey: Option<&[u8]> = None;
        while pi < np {
            let s = &parts[pi];
            let start = if s.lo.as_slice() > from {
                s.lo.as_slice()
            } else {
                from
            };
            prank = s.blob.seek(start);
            pkey = s.blob.key_at(prank);
            if pkey.is_some() {
                break;
            }
            pi += 1;
        }
        struct Cur<'a> {
            seg: &'a Seg,
            rank: usize,
            key: Option<&'a [u8]>,
        }
        let mut l0: Vec<Cur> = self.segs[np..]
            .iter()
            .filter(|s| s.may_reach(from))
            .map(|s| {
                let start = if s.lo.as_slice() > from {
                    s.lo.as_slice()
                } else {
                    from
                };
                let rank = s.blob.seek(start);
                Cur {
                    seg: s,
                    rank,
                    key: s.blob.key_at(rank),
                }
            })
            .collect();
        let nc = l0.len();
        let tombs = self.has_tombstones();
        let mut scratch: Vec<usize> = Vec::new();
        let mut seen = 0usize;
        while seen < limit {
            let mut next: Option<&[u8]> = pkey;
            for c in &l0 {
                if let Some(k) = c.key {
                    if next.is_none_or(|n| k < n) {
                        next = Some(k);
                    }
                }
            }
            let snap = unsealed.get(mi);
            if let Some((k, _)) = snap {
                if next.is_none_or(|n| k < n) {
                    next = Some(k);
                }
            }
            let Some(key) = next else { break };
            let in_unsealed = snap.is_some_and(|(k, _)| k == key);
            let snap = snap.map(|(_, sk)| sk);

            // Source indices: partition 0, level 0 at 1..=nc, frozen nc+1,
            // live nc+2. `start` is the oldest source whose values are live.
            let mut start = 0usize;
            if tombs {
                if let Some(sk) = snap.filter(|_| in_unsealed) {
                    if sk.mem != u32::MAX && self.mem.has_tomb(&self.mem.entries[sk.mem as usize]) {
                        start = nc + 2;
                    } else if sk.frozen != u32::MAX
                        && self
                            .frozen
                            .as_ref()
                            .is_some_and(|fr| fr.has_tomb(&fr.entries[sk.frozen as usize]))
                    {
                        start = nc + 1;
                    }
                }
                if start == 0 {
                    for (j, c) in l0.iter().enumerate().rev() {
                        if c.seg.tombs && c.key == Some(key) {
                            if let Some((_, exts)) = c.seg.blob.exts_at(c.rank) {
                                if exts.iter().any(|e| e.is_tombstone()) {
                                    start = j + 1;
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if pkey == Some(key) {
                if start == 0 {
                    parts[pi]
                        .blob
                        .values_at(prank, |v| f(key, v))
                        .map_err(|e| err(&format!("segment scan read: {e}")))?;
                }
                prank += 1;
                pkey = parts[pi].blob.key_at(prank);
                while pkey.is_none() && pi + 1 < np {
                    pi += 1;
                    prank = 0;
                    pkey = parts[pi].blob.key_at(0);
                }
            }
            for (j, c) in l0.iter_mut().enumerate() {
                if c.key == Some(key) {
                    if j + 1 >= start {
                        c.seg
                            .blob
                            .values_at(c.rank, |v| f(key, v))
                            .map_err(|e| err(&format!("segment scan read: {e}")))?;
                    }
                    c.rank += 1;
                    c.key = c.seg.blob.key_at(c.rank);
                }
            }
            if let Some(sk) = snap.filter(|_| in_unsealed) {
                if sk.frozen != u32::MAX && nc + 1 >= start {
                    if let Some(fr) = &self.frozen {
                        let e = &fr.entries[sk.frozen as usize];
                        fr.live_offs_into(e, &mut scratch);
                        for &off in &scratch {
                            f(key, fr.value_at(off));
                        }
                    }
                }
                if sk.mem != u32::MAX && nc + 2 >= start {
                    let e = &self.mem.entries[sk.mem as usize];
                    self.mem.live_offs_into(e, &mut scratch);
                    for &off in &scratch {
                        f(key, self.mem.value_at(off));
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
        let at =
            self.segs[..np].partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
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
                n += seg
                    .blob
                    .count(key)
                    .map_err(|e| err(&format!("segment count: {e}")))?;
            }
        }
        for (i, seg) in l0.iter().enumerate() {
            if 1 + i < start || !seg.may_hold(key) {
                continue;
            }
            n += seg
                .blob
                .count(key)
                .map_err(|e| err(&format!("segment count: {e}")))?;
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

    /// The seal phase decomposed; see `SealWaits`.
    pub fn seal_waits(&self) -> SealWaits {
        self.seal_wait
    }

    /// Keys held by the unsealed sources: the live memtable and, while a
    /// seal is in flight, the frozen one. A key in both counts twice.
    pub fn unsealed_keys(&self) -> usize {
        self.mem.len + self.frozen.as_ref().map_or(0, |f| f.len)
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

    /// Whether a seal and a merge are running right now. A crash experiment
    /// records the state it died in with these (c4-crash).
    pub fn in_flight(&self) -> (bool, bool) {
        (self.sealing.is_some(), self.compacting.is_some())
    }

    /// The live WAL: its path, the bytes of it behind a barrier, and the
    /// bytes written to it. Everything between the two is what a power loss
    /// may take; `c4-crash` takes a random amount of it, because a process
    /// kill alone leaves the page cache intact and cannot tell `EveryN` from
    /// `Always`.
    pub fn wal_durable(&self) -> (PathBuf, u64, u64) {
        (self.wal.path.clone(), self.wal.synced, self.wal.written)
    }

    /// Commit what is pending, seal the rest. Close is a convenience, not a
    /// durability point -- the WAL already made everything durable.
    pub fn close(mut self) -> Result<()> {
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<()> {
        self.flush()?;
        // Nothing will rotate into a spare again.
        for spare in std::mem::take(&mut self.spare_wals) {
            let _ = std::fs::remove_file(spare);
        }
        Ok(())
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
