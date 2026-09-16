//! The engine: a WAL, a memtable, and immutable segments.
//!
//! `docs/engine.md` is the design brief and every load-bearing decision
//! here cites a measurement. A durable commit is one framed append and one
//! fdatasync and nothing else, because that shape was measured at 1,191,125
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
//! fan that queries every source was priced at 90ns a segment, and every
//! keys-sized global router tried lost to routing by range, which is why the
//! routing is by range and not by key.
//!
//! Crash discipline, in order, so every window is survivable:
//! commit = WAL append + fdatasync (the batch is durable or its tail frame
//! fails its CRC and replay stops before it); seal = write the segment to a
//! temp name, fsync it, rename into place, fsync the directory, then reset
//! the WAL -- a crash between any two of those leaves either a WAL that
//! replays the whole memtable or a complete renamed segment plus a WAL
//! whose sealed prefix is skipped by sequence number.

use std::cell::UnsafeCell;
use std::cmp::Ordering;
use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};

use crate::block::{self, crc32, BlockBuilder, BlockLoc};
use crate::bytes::MmapBytes;
use crate::flatindex;
use crate::index::{Ext, Extents};
use crate::Blob;

/// Write a segment's ordered index and make it durable.
///
/// The caller renames the segment into place AFTER this returns, which is
/// the whole ordering: a segment that exists has an index, so a reader never
/// meets one without. A crash between the two leaves an index no segment
/// names, which the orphan sweep at open removes.
fn write_ord(dir: &Path, seg_name: &str, bytes: &[u8]) -> Result<()> {
    let name = Db::ord_name_for(seg_name).ok_or_else(|| err("segment name is malformed"))?;
    let tmp = dir.join(format!("{name}.tmp"));
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, dir.join(&name))?;
    Ok(())
}

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
/// It exists because the device was measured serving ~2,700 barriers a
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
/// this thread's pages; the commit phase was measured slowing whenever a
/// seal ran beside it, and this is the answer priced against that.
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

/// How a store advises the kernel about its segment mappings.
///
/// Once a store outgrows the page cache the kernel's default readahead is
/// the whole out-of-core cliff: cold point reads run 75.8x and 78.9x faster
/// under `MADV_RANDOM`, at 1.0x read amplification against 1800x -- the
/// default fetched 157 GB off the device to serve 89 MB anybody asked for.
/// It is a trade rather than a win, because an ordered scan wants exactly
/// the pages a point read does not and pays 2.3x to 2.5x for losing them.
///
/// A mapping's advice applies to the reader that set it and not to the file,
/// so a compaction streams its inputs under the kernel's default however
/// this is set: `compact` opens them through its own `MmapBytes`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ReadAdvice {
    /// The kernel's own readahead, for every access -- `MADV_NORMAL`, and
    /// what the engine did before there was a choice.
    ///
    /// Named for the `madvise` mode rather than called `Default`, because
    /// `ReadAdvice::default()` is `Adaptive`: a variant of that name would
    /// read as the type's default while meaning the opposite of it, and
    /// somebody reaching for one would silently get the other.
    Normal,
    /// `MADV_RANDOM` for the life of the store. For one whose working set
    /// outgrows memory and whose reads are points.
    Random,
    /// Follow the workload: `MADV_RANDOM` while the store is answering point
    /// reads, the kernel's default while it is scanning, switched on the
    /// first call of the other kind.
    ///
    /// The store does not have to infer which it is doing -- `read_all` and
    /// `scan` are different calls -- so the phase signal is free and exact.
    /// There is no threshold to tune: a sweep of one found that waiting for
    /// a second consecutive scan before switching falls to 33.2% and 30.8% of
    /// the better fixed advice on a workload with no phases, where switching
    /// on the first is 1.5x it. A switch is a `madvise` in microseconds and
    /// one cold scan in the wrong mode is milliseconds, so there is nothing
    /// to be gained by waiting.
    ///
    /// The default, because it wins where the advice matters and costs
    /// nothing where it does not. Out-of-core it is 4.3-4.4x the kernel's
    /// default and 6.5-6.6x a fixed `MADV_RANDOM` over a store of several
    /// segments, and 2.0-2.1x the better of the two on a workload with no
    /// phases. On a store that fits in memory, where it can win nothing and
    /// can only cost, it is a tie -- as it is on the canonical comparison
    /// this project quotes.
    #[default]
    Adaptive,
    /// Never leave `MADV_RANDOM`, and prefetch the value bytes a scan is
    /// about to walk before walking them.
    ///
    /// The other three tell the kernel what kind of access to expect and let
    /// it decide what to fetch. This one stops guessing: the reader is handed
    /// a `limit`, so it knows the span, plans the exact ranges its records
    /// name and asks for those. There is no phase to detect and no mode to
    /// switch, which makes it the simplest of the four rather than the most
    /// elaborate -- whether it is also the fastest is a question for the
    /// measurement.
    Prefetch,
}

impl ReadAdvice {
    /// The mode a store's mappings start in.
    ///
    /// `Adaptive` starts advised because it leaves that mode on the first
    /// scan and being wrong in the other direction is the expensive one:
    /// measured, a cold point read under the kernel's default ran at about
    /// a seventy-fifth of an advised one, against 2.4x for a scan under
    /// `MADV_RANDOM`.
    fn starts_random(self) -> bool {
        matches!(
            self,
            ReadAdvice::Random | ReadAdvice::Adaptive | ReadAdvice::Prefetch
        )
    }
}

#[derive(Clone)]
pub struct Options {
    pub sync: SyncPolicy,
    /// Memtable bytes that trigger a seal at the next commit. Sealing is off
    /// the commit path in cost accounting but runs on the committing thread
    /// in milestone 1; the brief's "Segment size" question owns this number.
    /// With `seal_grows` this is the floor: see there.
    pub seal_bytes: usize,
    /// Let the seal grow with the store, so a merge takes in at least a
    /// quarter of what it rewrites. A seal of a fixed size drops a slice
    /// into every range, and the slice shrinks as ranges multiply: on a
    /// store of thirty million keys a 32 MiB seal put 0.3 MB into each of
    /// 104 ranges, `l0_trigger` of them made 1.2 MB, and merging them
    /// rewrote a 64 MB partition -- fifty times the bytes -- while twenty
    /// more seals landed on the ranges the job covered. Level-0 reached 24
    /// pieces a range and every read checked 24 Blooms. With the seal at
    /// a sixteenth of the partitions' bytes, level-0 stayed under three,
    /// and D went from 35k to 533k ops/s, E from 14k to 334k. The divisor
    /// is `4 * l0_trigger`: the bytes a merge rewrites over the bytes it
    /// takes in, held to four. Off, the seal is `seal_bytes` and nothing
    /// else, the shape the suite's arms were measured in before this.
    pub seal_grows: bool,
    /// Ordered ingest goes straight into a segment. Keys arriving above the
    /// store's greatest, with values the record holds inline
    /// (`inline_bytes`), while the memtable is empty, fill an ordered
    /// memtable -- appended in key order and searched by binary search,
    /// never hashed -- and each commit streams them to a segment open for
    /// append: one fdatasync on it instead of the WAL, and no seal or
    /// partitioning pass after, since they are in their final order above
    /// every partition. At the seal threshold, or at the first write the
    /// run cannot take, the segment closes on the seal thread and joins as
    /// a piece for promotion by rename, as a seal's does. `false` is the
    /// path before it, every batch through the WAL and the seal, kept as
    /// the comparison arm.
    pub direct_ingest: bool,
    /// SegmentOptions for the segment writer. Fixed to `redo_log: false, shards: 1`
    /// regardless of what is passed, because a sealed segment is written
    /// once and never reopened for writing, and a 4 MiB redo arena in a
    /// write-once file is pure waste.
    pub segment: SegmentOptions,
    /// How many overlapping L0 segments to tolerate before a partitioning
    /// merge. The brief's open "partitioned compaction policy" question in
    /// one number; it was chosen by sweeping it.
    pub l0_trigger: usize,
    /// The measurement instrument: false keeps every segment in the
    /// unrouted L0 fan, which is milestone 3's behaviour exactly.
    pub compact: bool,
    /// Whether `flush` partitions what it sealed before returning.
    ///
    /// This is a read-for-write trade and it is a large one. Partitioning
    /// makes every later read touch exactly one segment instead of paying
    /// a Bloom check on each of several overlapping ones -- worth roughly
    /// 1.4x on the canonical read comparison -- but it is a second full
    /// pass over everything just sealed, inside whatever window the caller
    /// is timing. A writer that is keeping up with ingest and reads later
    /// wants it off, and the background compaction will get there on its
    /// own schedule.
    pub partition_on_flush: bool,
    /// Find the keys a merge writes by a k-way walk of the inputs' rank
    /// order (the default) rather than by collecting, sorting and probing
    /// them. The probe path is kept as the comparison arm.
    pub cursor_merge: bool,
    /// I/O priority of the seal and merge threads.
    pub background_io: BackgroundIo,
    /// Have the segment writer fdatasync every this many bytes as it
    /// streams blocks, so its dirty pages leave in slices rather than in
    /// one flush at the end. Zero syncs at the end only.
    pub seal_sync_every: usize,
    /// Promote pieces instead of merging them when nothing needs merging:
    /// a range's pieces whose keys all lie above its partition's last key,
    /// mutually disjoint, become partitions by rename, and the partition's
    /// fence closes below them. Nothing is rewritten. Ordered ingest -- a
    /// log -- is all promotion; uniform keys never qualify.
    pub promote: bool,
    /// How a flush drains level 0 once partitions exist: merge only the
    /// ranges that hold pieces, under the live fences (`true`), or
    /// re-partition everything from every key (`false`, the original), kept
    /// as the comparison arm.
    pub flush_ranges: bool,
    /// Runs of values up to this many bytes are stored inline in the index
    /// record rather than in a block, so a point read of such a key touches
    /// the hash slot and the record and nothing else. Zero disables.
    pub inline_bytes: usize,
    /// Target bytes per partition: how many partitions the first
    /// partitioning cuts, and how many keys one holds before a merge splits
    /// it. `None` uses `seal_bytes`, the coupling under which smaller seals
    /// were also making more partitions and paying for them on every read;
    /// `Some` decouples the two.
    pub partition_bytes: Option<usize>,
    /// Recycle retired WAL files instead of creating fresh ones, and
    /// pre-write the first to the seal size, so every block a commit's
    /// fdatasync touches is already allocated and written. On ext4 an
    /// fdatasync of an append that grows the file commits an inode change
    /// through the journal; an overwrite does not, and LMDB's commit is an
    /// overwrite.
    /// How reads advise the kernel about the segment mappings.
    ///
    /// See `ReadAdvice`. `Adaptive` unless changed.
    pub read_advice: ReadAdvice,
    /// Contiguous bytes a scan must expect to walk before the kernel's
    /// readahead is worth having, under `ReadAdvice::Adaptive`.
    ///
    /// `Adaptive` used to put the segments on the kernel's default for every
    /// scan, on the reasoning that a scan walks values in order and wants the
    /// pages ahead of it. That is right for a long scan and badly wrong for a
    /// short one: out of core, over a store at 1.33x the page cache, a scan of
    /// a hundred entries ran 9.4x FASTER under `MADV_RANDOM` than under the
    /// default, because each of its three hundred thousand seeks paid a
    /// readahead it never read. The same store scanned a hundred thousand
    /// entries at a time ran 3.6x SLOWER under `MADV_RANDOM`, where the
    /// readahead is exactly what the scan goes on to use.
    ///
    /// So the question is not the phase but the span, and `scan` is handed
    /// its limit before it touches a page -- the signal is free and exact,
    /// the way the phase signal is. Measured at thirty million keys under a
    /// four gibibyte cap, entries/s random against default: 11.6 KB 11.4x,
    /// 33 KB 4.2x, 113 KB 1.56x, then 339 KB 0.85x, 1.16 MB 0.64x, 11.6 MB
    /// 0.28x. The crossing is near 200 KB and the curve is flat across it --
    /// within 1.6x either way between 113 KB and 339 KB -- so the number
    /// below only has to land in that band, and the ends, where being wrong
    /// costs an order of magnitude, are nowhere near it.
    ///
    /// It is NOT the device's readahead window. That was the first guess and
    /// it is wrong by a factor of forty: this device reports 8,192 kB.
    pub scan_readahead_bytes: usize,
    pub recycle_wal: bool,
    /// The ordered scan's merge over unrouted sources. `true` is the merge
    /// that replaced the original: one cursor over the disjoint partitions
    /// in order rather than one per partition, each cursor's key resolved
    /// once per emitted key, and the unsealed snapshot carrying each key's
    /// memtable entry so the emit is a chain walk over a reused buffer
    /// instead of two hash probes and an allocation. `false` is the merge
    /// before it, kept as the comparison arm.
    pub scan_merge: bool,
    /// PROTOTYPE, on by default: keep merged copies of the partition
    /// blocks that scans read over unsealed keys, so a scan over them
    /// walks one sorted copy instead of merging. `false` is the merge on
    /// every scan, the shape before it, kept as the comparison arm. What
    /// the cache costs is memory, about a tenth of the store after a pass
    /// that reaches every block, unbounded unless `scan_cache_bytes` says
    /// otherwise; on every workload but the scan mixes it prices the same
    /// as the arm without it. A block is built on first
    /// read through the merge; a write to a key it owns is settled into it
    /// in place at the next scan, the key's run resolved as a build would;
    /// and every block is dropped whenever the segments change.
    pub scan_block_cache: bool,
    /// PROTOTYPE: the most bytes the block cache holds in built blocks,
    /// or 0 for no bound, the default. The cache holds what the scans
    /// touched: about a tenth of the store after a pass that reaches
    /// every block, at three million keys and at thirty, and a budget
    /// under that sheds blocks the next pass rebuilds -- at thirty
    /// million keys a sixteenth of the store cost ycsb-E a quarter of its
    /// rate and a sixty-fourth three fifths -- so the bound is the
    /// caller's to set from the memory it has. Past it, a build sheds the
    /// least recently touched of a few sampled blocks until under; a shed
    /// block is rebuilt from the partition, the pieces and the memtables
    /// when a scan next wants it, so the pieces on disk are what the
    /// cache overflows to.
    pub scan_cache_bytes: usize,
    /// PROTOTYPE, off by default: with the block cache, build ahead of
    /// the reader. After every publish a thread opens readers of its own
    /// over the published segments -- immutable files, readable while
    /// mapped -- and builds every block the pieces overlay from the
    /// partitions and the pieces alone, sending each form back as it is
    /// built; the store installs a form at its next scan, if the segments
    /// have not changed since, and splices the memtable's keys over the
    /// block in as it settles a write. The first scan over a block then
    /// finds it built.
    pub scan_cache_ahead: bool,
    /// How the ordered scan builds its sorted snapshot of the unsealed keys.
    /// `true` keeps the keys in one arena and sorts 24-byte records (a
    /// 16-byte key prefix and an index), touching the arena only on a shared
    /// prefix; `false` is the build before it -- a `Vec<u8>` per key, sorted
    /// through two heap pointers per compare -- kept as the comparison arm.
    /// The build runs on the first scan after a commit and cost 300 ns a key.
    pub scan_snapshot_arena: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            sync: SyncPolicy::Always,
            // 32 MB seals over 64 MB partitions: measured at 1.129x the
            // ingest of 64 MB seals at identical device bytes and identical
            // reads. Smaller still buys nothing and costs 1.5x the device
            // bytes.
            seal_bytes: 32 << 20,
            seal_grows: true,
            direct_ingest: true,
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
            read_advice: ReadAdvice::default(),
            scan_readahead_bytes: 256 << 10,
            scan_merge: true,
            scan_block_cache: true,
            scan_cache_bytes: 0,
            scan_cache_ahead: false,
            scan_snapshot_arena: true,
        }
    }
}

/// One WAL frame: `len u32 | crc u32 | seq u64 | klen uvarint | key | value`.
/// `len` covers everything after `crc`; `crc` covers the same bytes. The
/// value's length is `len` minus what precedes it, so values cost no second
/// length field.
const FRAME_HEADER: usize = 8;

/// A commit marker in a direct segment's record stream: a record head no
/// key produces -- zero key length and zero extents, where every key
/// writes one -- then this tag, the CRC and length of the records since
/// the previous marker, and the count of records before it. Readers
/// reach records by directory offset and never step on it; recovery
/// walks the stream to the last one whose three quantities agree with
/// the bytes before it, since a crash can land the page a marker is on
/// and not a page of the batch it closes.
const DIRECT_MARK: [u8; 4] = *b"SUPD";
const DIRECT_MARK_LEN: usize = 24;

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
    /// experiment can take it.
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
    /// in 4 KB pieces, 1.04x.
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
    /// The CRC is per batch, not per frame. A record frame's `crc`
    /// word is zero; the commit frame carries, as its payload, the CRC of
    /// every byte of the batch's record frames, and its own `crc` word
    /// covers its body as before. Replay applies a batch only at a commit
    /// frame whose both CRCs verify, so a damaged byte anywhere in a batch
    /// loses that batch and the ones after it -- exactly what a CRC per
    /// frame bought, at one CRC setup and finish per batch instead of per
    /// record: 92 of the 677 instructions a record cost.
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

/// A segment open for ordered ingest: its writer, the temp file it streams
/// to, its id, and how many of the ordered memtable's entries it holds --
/// the rest are the batch in progress.
struct Direct {
    w: PieceWriter,
    tmp: PathBuf,
    id: u64,
    committed: usize,
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
/// measured at 82.1% of a single store when it is the only routing there
/// is. Here it guards only the bounded L0 tail, because every keys-sized
/// global router tried lost to routing by range -- the partitioned levels
/// below are routed by fences that cost two comparisons.
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
    /// PROTOTYPE: for a level-0 piece aligned to a partition, each of its
    /// keys' rank in that partition, shifted left one, with the low bit
    /// set when the partition holds the key: where the key cuts a block's
    /// walk, found once when the piece is published instead of by a
    /// search over the block's records at every build. Keyed by the
    /// partition's blob id: a piece sealed while a merge of its range ran
    /// is kept across the merge's publish, under a new partition, and
    /// ranks taken against the old one are behind by every key the merge
    /// folded in below. A lock rather than a once-cell for that reason,
    /// taken once per block build and piece; any reader may fill it.
    ranks: std::sync::RwLock<Option<(u64, Vec<u32>)>>,
    level: u8,
    /// The WAL sequence the segment's name carries: what orders the
    /// level-0 pieces over one fence oldest to newest, which a read's
    /// tombstone rule and a merge's value order both rest on. Ordered by
    /// name alone, a piece sealed before the first partitioning, named
    /// `seg-`, came after a newer `pcs-` piece over the same empty fence,
    /// and a read took the older piece's tombstone as the newest source:
    /// a reader thread saw a version go backwards, and the writer's own
    /// reads would have too.
    seq: u64,
    lo: Vec<u8>,
    hi: Option<Vec<u8>>,
    bloom: Option<BlockedBloom>,
    /// The segment's ordered index, mapped. Not optional: it is written
    /// before its segment is renamed into place, so a segment a reader can
    /// see always has one, and a missing or damaged one fails the open
    /// rather than sending the seek back to `Blob::seek`. A fallback would
    /// be the slow path taken silently, which is the shape of every gate
    /// this repository has broken.
    ord: crate::ordindex::OrdIndex,
    /// Whether any extent here carries the tombstone flag. A read consults
    /// it before paying the newest-first pass that tombstones require.
    ///
    /// False for every partition, and `Seg::open` assumes that for a `par-`
    /// name rather than walking the keys to find out. A merge earns it by
    /// writing the bottom level and dropping them. A promotion does not --
    /// it renames the file as it stands -- but it does not need to: the
    /// partitions tile the key space, so a promoted piece is the only and
    /// therefore the oldest source over its own range, and a tombstone with
    /// nothing older beneath it masks nothing. What the flag would still
    /// have bought is the reclaim, which is why `promote_unpartitioned`
    /// leaves a lone piece holding one to the merge.
    tombs: bool,
}

/// The order the live segments hold: partitions first, disjoint and by
/// fence; then the level-0 pieces by fence, so the pieces over one range
/// are one run `pieces_over` can bracket, and within a fence oldest to
/// newest by the sequence their names carry, and by name last. A piece
/// sealed before the first partitioning has the empty fence, as the
/// pieces aligned to the first partition do, and it is the sequence that
/// puts it before them.
fn seg_order(a: &Seg, b: &Seg) -> Ordering {
    b.level
        .cmp(&a.level)
        .then_with(|| a.lo.cmp(&b.lo))
        .then_with(|| a.seq.cmp(&b.seq))
        .then_with(|| a.name.cmp(&b.name))
}

// ------------------------------------------------------- the segment writer --

/// Writes an immutable segment in one forward pass, for input that arrives
/// sorted by key with each key's values together.
///
/// `Store` is a general writer: a hash table to find keys again, a freelist
/// to place blocks, a pending arena, a reuse log, and a checkpoint that
/// publishes all of it. A seal and a merge need none of that -- their keys
/// come sorted, each key's values come once and together, and nothing is
/// ever read back or appended to -- and the general path was priced at
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
    /// Whether the records are hashed batch by batch for `mark`: on for
    /// a direct segment, off for a seal, which never marks.
    marks: bool,
    /// The CRC of the records since the last marker, and where they start.
    mark_crc: u32,
    mark_start: usize,
    /// LZ4 the blocks, as `Store` does when `SegmentOptions::compress` is set. A
    /// block above the chunk size is compressed chunk by chunk with its own
    /// directory, so a point read decompresses one chunk rather than the
    /// block; one that does not shrink is written verbatim. Inline runs live
    /// in the key section and are untouched either way.
    compress: bool,
    /// Per-chunk checksums for the blocks written verbatim, one row per
    /// block in the block table. Without them a run read fetches the whole
    /// block, which is what `blob::chunk_span` plans by.
    chunk_rows: Vec<[u32; block::MAX_CHUNK_CRCS]>,
    /// Bytes left free after the superblock page, into which `finish` puts
    /// the block table and a copy of the fence when they fit, so a host
    /// whose first probe covers the reserve opens in one round trip. Zero
    /// for none; laid down at the first key.
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
/// the pass and its whole section at `finish`, which measured as
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

/// Whether a run of `run_len` bytes goes into its record's tail rather
/// than a block, under an inline limit: the writer's one rule for it, and
/// the direct path's, which asks it of a batch's values at `append`,
/// since a run in a block is held until `finish` and is not durable at
/// a marker.
fn inlines(inline_max: usize, run_len: usize) -> bool {
    inline_max > 0 && run_len <= inline_max
}

impl SegmentWriter {
    /// Open `path` for a fresh segment with every per-file setting applied
    /// and the head reserve set, so nothing is left to remember.
    ///
    /// The four things a segment writer can be configured to do -- inline
    /// runs, compression, spread syncs, the head reserve -- all have to be set
    /// before the first key, and each is silent when forgotten: a plain
    /// segment where a compressed one was wanted, or a reserve of zero that
    /// costs the sparse reader a round trip and raises nothing. Setting them
    /// at construction is what makes forgetting one impossible.
    ///
    /// `reserve` is a number rather than a policy, because the caller may
    /// know it exactly. `reserve::for_lengths` computes it for input in hand,
    /// and its `Reserve` prices the hash-directory copy separately:
    ///
    /// ```no_run
    /// # use supdb::{SegmentOptions, SegmentWrite, SegmentWriter, reserve};
    /// # let (path, opts, write) = (std::path::Path::new("s.sup"), SegmentOptions::default(), SegmentWrite::default());
    /// # let lengths: Vec<(usize, usize)> = Vec::new();
    /// let r = reserve::for_lengths(&lengths, opts.block_size, write.inline_max).unwrap();
    /// // `r.bytes()` for a lookup that plans from the probe; `without_directory`
    /// // to save four bytes a key and let a lookup fetch the directory itself.
    /// let w = SegmentWriter::create_with(path, &opts, &write, r.bytes())?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn create_with(
        path: &Path,
        opts: &SegmentOptions,
        write: &SegmentWrite,
        reserve: usize,
    ) -> Result<SegmentWriter> {
        let mut w = SegmentWriter::create(path, opts)?;
        // Every field of `SegmentWrite`, and the reserve. If a setter is
        // added to this writer without a field beside it here, this stops
        // compiling, which is the point of the struct.
        let SegmentWrite {
            inline_max,
            compress,
            sync_every,
        } = *write;
        w.set_inline_max(inline_max);
        w.set_compress(compress);
        w.set_sync_every(sync_every);
        w.set_head_reserve(reserve);
        Ok(w)
    }

    /// Write a whole segment from input already in hand, sizing the head
    /// reserve exactly instead of guessing at it.
    ///
    /// The reserve has to be chosen before the first key is written, so a
    /// streaming caller can only guess -- and both ways of guessing wrong are
    /// invisible, costing a round trip or costing file. A caller that gathered
    /// its keys first does not have to: the lengths are enough to compute the
    /// reserve exactly, which is what `reserve::for_lengths` does and what
    /// this does for you.
    ///
    /// The reserve it computes holds the hash-directory copy, so a lookup
    /// plans its records from the probe. To trade that for four bytes a key,
    /// take `reserve::for_lengths(..).without_directory()` and stream through
    /// [`SegmentWriter::create_with`] instead.
    ///
    /// `items` must be sorted by key, as the streaming API requires. Returns
    /// the reserve it used, since a caller measuring segments wants to know.
    pub fn write_sorted(
        path: &Path,
        opts: &SegmentOptions,
        write: &SegmentWrite,
        generation: u64,
        items: &[(&[u8], &[&[u8]])],
    ) -> Result<usize> {
        let lengths: Vec<(usize, usize)> = items
            .iter()
            .map(|(k, vals)| {
                let lens: Vec<u32> = vals.iter().map(|v| v.len() as u32).collect();
                (k.len(), crate::reserve::run_len(&lens))
            })
            .collect();
        // Compression does not enter the reserve: blocks are cut on the
        // payload the builder staged, before anything compresses it, so the
        // block count -- and the table sized by it -- is the same either way.
        // What compression moves is where the key section lands, and the row
        // is already taken at its worst alignment.
        let reserve = crate::reserve::for_lengths(&lengths, opts.block_size, write.inline_max)
            .ok_or_else(|| err("segment writer: this input cannot be a segment"))?
            .bytes();

        let mut w = SegmentWriter::create_with(path, opts, write, reserve)?;
        for (k, vals) in items {
            w.begin(k)?;
            for v in *vals {
                w.value(v);
            }
            w.end()?;
        }
        w.finish(generation)?;
        Ok(reserve)
    }

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
            marks: false,
            mark_crc: 0,
            mark_start: 0,
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

    /// A commit marker after the records so far, in the records-first
    /// layout: what `recover_direct` cuts at. A batch of a direct segment
    /// ends with one, before its sync, so a crash leaves whole batches.
    pub fn mark(&mut self) -> Result<()> {
        if self.open_key.is_some() {
            return Err(err("segment writer: mark while a key is open"));
        }
        if self.layout()? != Layout::RecordsFirst {
            return Err(err(
                "segment writer: a marker needs the records-first layout",
            ));
        }
        if !self.marks {
            return Err(err("segment writer: mark on a writer not set to mark"));
        }
        let mut m = [0u8; DIRECT_MARK_LEN];
        m[4..8].copy_from_slice(&DIRECT_MARK);
        m[8..12].copy_from_slice(&self.mark_crc.to_le_bytes());
        m[12..16].copy_from_slice(&((self.recs_len - self.mark_start) as u32).to_le_bytes());
        m[16..24].copy_from_slice(&(self.spans.len() as u64).to_le_bytes());
        self.out.write_all(&m)?;
        self.pos += DIRECT_MARK_LEN as u64;
        self.recs_len += DIRECT_MARK_LEN;
        self.mark_crc = 0;
        self.mark_start = self.recs_len;
        Ok(())
    }

    /// Everything written so far made durable: a direct segment's commit.
    pub fn sync(&mut self) -> Result<()> {
        self.out.flush()?;
        self.out.get_ref().sync_data()
    }

    /// The keys and values a direct segment's stream holds up to its last
    /// commit marker, in order: what a crash left acknowledged. The stream
    /// starts after the superblock region and the section header, where
    /// `create` puts it with no head reserve; each key carries one inline
    /// extent, a fixed run of one value or a prefixed one. A record the
    /// stream cannot parse ends the
    /// walk, as does a marker whose CRC, length or count disagrees with
    /// the records before it; what follows the last good marker is a
    /// batch that never committed.
    pub fn recover_direct(path: &Path) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let buf = std::fs::read(path)?;
        let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut committed = 0usize;
        let mut p = crate::format::SUPER as usize + flatindex::HEADER;
        let mut batch = p;
        while p + 4 <= buf.len() {
            let klen = u16::from_le_bytes([buf[p], buf[p + 1]]);
            let n = u16::from_le_bytes([buf[p + 2], buf[p + 3]]);
            if klen == 0 && n == 0 {
                if p + DIRECT_MARK_LEN > buf.len() || buf[p + 4..p + 8] != DIRECT_MARK {
                    break;
                }
                let crc = u32::from_le_bytes(buf[p + 8..p + 12].try_into().expect("four"));
                let len = u32::from_le_bytes(buf[p + 12..p + 16].try_into().expect("four"));
                let count = u64::from_le_bytes(buf[p + 16..p + 24].try_into().expect("eight"));
                if len as usize != p - batch
                    || count as usize != out.len()
                    || crc != crc32(&buf[batch..p])
                {
                    break;
                }
                committed = out.len();
                p += DIRECT_MARK_LEN;
                batch = p;
                continue;
            }
            let Some((key, exts, tail, len)) = flatindex::parse_record(&buf, p) else {
                break;
            };
            let [e] = exts else { break };
            if !e.is_inline() || e.is_tombstone() {
                break;
            }
            let run =
                match tail.get(e.off as usize..(e.off as usize).saturating_add(e.len as usize)) {
                    Some(r) => r,
                    None => break,
                };
            let value = if e.count & Ext::FIXED != 0 {
                run
            } else {
                let mut q = 0usize;
                let Some(vlen) = get_uvarint(run, &mut q) else {
                    break;
                };
                match run.get(q..q.saturating_add(vlen as usize)) {
                    Some(v) => v,
                    None => break,
                }
            };
            out.push((key.to_vec(), value.to_vec()));
            p += len;
        }
        out.truncate(committed);
        Ok(out)
    }

    /// Store runs up to `bytes` long inline in the index record, and write
    /// the segment records-first so they stream. Zero keeps every run in a
    /// block and the blocks-first layout `Store` writes. Must be set before
    /// the first key.
    /// Hash the records batch by batch for `mark`. Must be set before
    /// the first key; a marker on a writer without it is an error.
    pub fn set_marks(&mut self, on: bool) {
        self.marks = on;
    }

    pub fn set_inline_max(&mut self, bytes: usize) {
        self.inline_max = bytes;
    }

    /// Compress the blocks. Off by default, because a segment written by the
    /// the seal is read back by its own merge and the seal path
    /// has never paid for compression; a segment written as an index to be
    /// downloaded is the other case, where a term index over posting deltas
    /// is materially smaller with it on. Must be set before the first key.
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
            // offsets are known; its bytes are reserved now so the
            // records start where `stream_trailer` says they do.
            self.out.write_all(&[0u8; flatindex::HEADER])?;
            self.pos += flatindex::HEADER as u64;
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
        let inline = inlines(self.inline_max, n);
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
                if self.marks {
                    self.mark_crc = block::crc32_resume(self.mark_crc, &self.rec_buf);
                }
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
        // instead of the block.
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

/// The per-file settings a segment is written with: one field for every one
/// of `SegmentWriter`'s setters, and nothing else.
///
/// That invariant is the point. Every setter here must be called before the
/// first key, and forgetting one is silent -- so
/// [`SegmentWriter::create_with`] takes this struct and applies all of it,
/// and a new setter that does not appear here is a compile error there rather
/// than a quiet default. Nothing lives in this struct that `create_with` does
/// not apply, which is why the choice of what the head reserve holds is not
/// here: `create_with` is given the reserve as a number.
///
/// These are separate from [`SegmentOptions`] on purpose, and the separation
/// is the same one that struct's own note draws: `SegmentOptions` is the
/// engine's configuration, carried to the writer for every piece it seals,
/// while these describe one file. A term index built to be downloaded wants
/// compression and inline runs; the segments the seal writes and its own
/// merge reads back want neither.
#[derive(Clone, Debug, Default)]
pub struct SegmentWrite {
    /// Runs up to this many bytes go inline in the index record, and the
    /// segment is written records-first so they stream. Zero keeps every run
    /// in a block and writes the blocks-first layout `Store` writes.
    pub inline_max: usize,
    /// LZ4 the blocks. Off by default, as it is on the writer.
    pub compress: bool,
    /// fdatasync every this many bytes of blocks rather than once at the end.
    /// Zero for the single sync.
    pub sync_every: usize,
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
    /// much as the code.
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
/// replaced, kept behind an option so a measurement could interleave the
/// two in one process and price the change honestly. That comparison is
/// settled and the old path is gone, so what is left is a thin shim that
/// keeps `flush` and `merge` reading as a sequence of begin/value/end calls.
struct PieceWriter(Box<SegmentWriter>, crate::ordindex::Builder);

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
        Ok(PieceWriter(Box::new(w), crate::ordindex::Builder::new()))
    }

    fn begin(&mut self, k: &[u8]) -> Result<()> {
        // The ordered index is composed here because here is where the keys
        // already are, sorted: 2.2ns a key against the 20.2ns a later pass
        // spends reading them back out of the finished segment.
        self.1.push(k);
        self.0.begin(k)
    }

    /// Infallible at the call so it can sit inside a read callback.
    fn value(&mut self, v: &[u8]) {
        self.0.value(v)
    }

    fn end_with(&mut self, tombstone: bool) -> Result<()> {
        self.0.end_with(tombstone)
    }

    fn set_marks(&mut self, on: bool) {
        self.0.set_marks(on)
    }

    fn mark(&mut self) -> Result<()> {
        self.0.mark()
    }

    fn sync(&mut self) -> Result<()> {
        self.0.sync()
    }

    /// The segment, then its ordered index's bytes for the caller to write
    /// beside it. Returning them rather than writing them keeps the naming
    /// with the two callers that know the segment's final name.
    fn finish(self) -> Result<Vec<u8>> {
        (*self.0).finish(1)?;
        Ok(self.1.finish())
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

    /// `random` is the mode the *store* is in right now, not the option: a
    /// segment from a seal or a merge has to join the mode its store is
    /// already in. Passing the option here instead is silent -- reads stay
    /// correct and only the advice goes stale -- which is why `advice_random`
    /// is there to be checked against the mappings rather than trusted.
    /// `verify` is the store's own checksum option. It has to be passed
    /// because the switch the writer used is process-wide and a reader
    /// process need never have written a segment: read it off a global and
    /// a store written with checksums off is refused by the engine that
    /// wrote it, on every run whose values reached a block.
    fn open(dir: &Path, name: &str, random: bool, advise_ord: bool, verify: bool) -> Result<Seg> {
        let src = MmapBytes::open(&dir.join(name)).map_err(|e| {
            // A manifest naming a segment that is not on disk is a damaged
            // store, not a missing file, and saying so is the difference
            // between a diagnosis and an ENOENT.
            err(&format!(
                "the manifest names segment {name}, which is not in the store: {e}"
            ))
        })?;
        let blob = Blob::open_with(
            src,
            crate::blob::BlobOptions {
                verify_checksums: verify,
                ..Default::default()
            },
        )
        .map_err(|e| err(&format!("segment {name}: {e}")))?;
        if random {
            blob.advise_random();
        }
        let oname = Db::ord_name_for(name).ok_or_else(|| err("segment name is malformed"))?;
        let mut ord = crate::ordindex::OrdIndex::open(&dir.join(&oname), blob.keys())
            .map_err(|e| err(&format!("segment {name}: {e}")))?;
        // The common prefix, once, off the first key, so a seek's prefix
        // check reads no record.
        if let Some(first) = blob.key_at(0) {
            ord.learn_prefix(first);
        }
        // Taken from the POLICY, not from the phase the store is in. The
        // segment's advice follows the workload and `Db::advise` flips it;
        // the companion is binary-searched in either phase, so a segment
        // opened mid-scan -- by a seal or a merge, which pass
        // `advice_random.get()` -- would otherwise get an unadvised index
        // for the life of the store.
        if advise_ord {
            ord.advise_random();
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
                ranks: std::sync::RwLock::new(None),
                level: 0,
                seq: Db::name_end_seq(name).unwrap_or(0),
                lo,
                hi,
                bloom: Some(bloom),
                ord,
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
                ranks: std::sync::RwLock::new(None),
                level: 1,
                seq: Db::name_end_seq(name).unwrap_or(0),
                lo,
                hi,
                bloom: None,
                ord,
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
            ranks: std::sync::RwLock::new(None),
            level: 0,
            seq: Db::name_end_seq(name).unwrap_or(0),
            lo: Vec::new(),
            hi: None,
            bloom: Some(bloom),
            ord,
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

    /// Whether `key` sorts below this segment's lower fence. An open fence
    /// is the empty vector, and nothing is below it -- which the compare
    /// would also answer, at a price: `Vec::new()` holds a dangling
    /// non-null pointer, and glibc's memcmp reads through its left operand
    /// before it honours a zero length, so the compare costs a failed page
    /// walk every call, 81 ns measured against 2 ns on a real pointer.
    /// Every read that reaches the first partition or a level-0 piece was
    /// paying it, and so was every scan that started there.
    #[inline]
    fn below_lo(&self, key: &[u8]) -> bool {
        !self.lo.is_empty() && key < self.lo.as_slice()
    }

    /// The scan's cursor into this segment: `from`, or the lower fence if
    /// that is higher. See `below_lo` for why the empty fence is not
    /// compared.
    #[inline]
    fn cursor_from<'a>(&'a self, from: &'a [u8]) -> &'a [u8] {
        if !self.lo.is_empty() && self.lo.as_slice() > from {
            self.lo.as_slice()
        } else {
            from
        }
    }

    /// Could this segment hold `key`? A fence answers exactly; a Bloom
    /// answers with false positives and never a false negative.
    #[inline]
    fn may_hold(&self, key: &[u8]) -> bool {
        if self.below_lo(key) {
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
/// value: a decomposition priced the HashMap<Box<[u8]>, Vec> version at
/// 456k ops/s of the gap to the floor, more than the seal itself. Keys
/// live in one arena; values live in another as per-key backward chains
/// (each chunk records the previous chunk's offset, and a read or seal
/// walks the chain and reverses it).
///
/// Built for one writer and any number of readers with no lock between
/// them. Nothing in it moves once published: the arenas are blocks that
/// are never reallocated, the entries a slab of the same kind, numbered
/// in the order they were made, and the hash index -- a table of entry
/// numbers -- is rebuilt whole when it fills and published by one
/// pointer store, the table it replaces kept until every reader that
/// could hold it has moved on. A reader reaches an entry through the
/// index or through a number it was handed, and everything the entry
/// names was written before the store that published it: the key's
/// bytes before the index slot, a chunk's bytes before the head that
/// names it. A chunk is `[prev: u64][len: u32][value]`, the length fixed
/// so the header is one read of twelve bytes that were all written
/// before the head moved.
///
/// The value arena's offset is also the version: chunks are appended in
/// time order, so `committed`, the arena's tail at the last commit,
/// divides every chain into an uncommitted prefix and a committed rest,
/// and a reader that honours it walks past the prefix. The store's own
/// reads pass no watermark and see their own uncommitted writes, the
/// contract `read_all` set; a reader handle chooses.
///
/// The writer is the store, made single by `&mut Db`; the writer-only
/// fields are cells it alone touches, and that is the whole of what the
/// `Sync` below asserts.
struct MemTable {
    /// Every entry in the order it was made -- key order, when
    /// `ordered`, since ordered ingest pushes each key above the last and
    /// a lookup there is a binary search, never a probe.
    entries: Slab<MemEntry>,
    /// Entry numbers by hash, or null when `ordered`.
    index: AtomicPtr<Index>,
    ordered: bool,
    keys: ByteArena,
    vals: ByteArena,
    /// Tombstone chunks pushed so far. Non-zero is what tells a read that
    /// this memtable can end a key's older values; zero lets it skip the
    /// check entirely.
    tombs: AtomicUsize,
    /// The value arena's tail at the last commit: a chunk at or past it
    /// is a write no commit has covered.
    committed: AtomicU64,
    /// Indexes a rebuild replaced, each with the epoch it was retired at,
    /// freed once no reader is pinned before that epoch. Writer-only.
    retired: UnsafeCell<Vec<(u64, Box<Index>)>>,
    /// Every write, in order: the entry's number, with the high bit set
    /// when the write made the entry. What a handle reads to learn the
    /// keys written since it last looked -- for the scan snapshot and
    /// for settling into cached blocks -- without the writer keeping a
    /// list for it.
    log: Slab<u32>,
}

// SAFETY: the writer-only cells (`retired`, and the arenas' and the
// slab's tails) are touched by the one writer `&mut Db` makes, and
// everything a reader follows is published with a release store after
// the bytes it names were written; see the type's doc.
unsafe impl Sync for MemTable {}
unsafe impl Send for MemTable {}

/// A chain chunk whose length word is this is a tombstone: it holds no
/// value, and nothing older than it -- in this chain or in any older
/// source -- is live.
const TOMB_LEN: u32 = u32::MAX;
/// A chunk: the previous chunk's offset, the value's length, the value.
const CHUNK_HDR: usize = 12;

struct MemEntry {
    hash: u64,
    key_off: u32,
    key_len: u32,
    /// Offset+1 of this key's newest chunk in `vals`, stored after the
    /// chunk's bytes; 0 = no chunk yet, which no published entry has.
    head: AtomicU64,
    count: AtomicU64,
}

const NO_CHUNK: u64 = u64::MAX;

/// No watermark: every chunk in a chain is visible, committed or not.
const SEE_ALL: u64 = u64::MAX;

fn mem_hash(key: &[u8]) -> u64 {
    // FNV-1a, then a splitmix finish; the std SipHash was part of what the
    // memtable's decomposition priced.
    let mut h = 0xcbf29ce484222325u64;
    for &b in key {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h | 1 // 0 marks a vacant slot
}

/// The hash index: per slot the hash's high half over the entry number
/// plus one, zero vacant, open addressing with linear probing at load
/// <= 0.5. A slot is stored once, with a release, and never moved:
/// growth is a new table. The tag is what keeps a probe from reading an
/// entry it will not match -- the entry is a line of its own, and at
/// thirty million keys the slot and the entry are both misses.
struct Index {
    slots: Box<[AtomicU64]>,
    mask: usize,
}

impl Index {
    fn with_slots(n: usize) -> Index {
        debug_assert!(n.is_power_of_two());
        Index {
            slots: (0..n).map(|_| AtomicU64::new(0)).collect(),
            mask: n - 1,
        }
    }

    fn word(hash: u64, id: usize) -> u64 {
        (hash & !0xffff_ffff) | (id as u64 + 1)
    }
}

/// Bytes in blocks that never move. A block is `ARENA_BLOCK` bytes, or
/// a value's own size when the value is larger; a reservation never
/// straddles two, so the tail steps to a fresh block when one will not
/// hold it. Offsets are logical, block number times the block size plus
/// the place inside, so an offset finds its block with a shift.
const ARENA_SHIFT: u32 = 22;
const ARENA_BLOCK: usize = 1 << ARENA_SHIFT;
/// Blocks an arena can address: sixteen gigabytes of the plain size.
const ARENA_BLOCKS: usize = 1 << 12;

struct ByteArena {
    blocks: Box<[AtomicPtr<u8>]>,
    caps: Box<[AtomicUsize]>,
    /// Writer-only.
    tail: UnsafeCell<ArenaTail>,
    used: AtomicUsize,
}

#[derive(Default)]
struct ArenaTail {
    at: usize,
    block_end: usize,
    next_block: usize,
}

impl ByteArena {
    fn new() -> ByteArena {
        ByteArena {
            blocks: (0..ARENA_BLOCKS)
                .map(|_| AtomicPtr::new(std::ptr::null_mut()))
                .collect(),
            caps: (0..ARENA_BLOCKS).map(|_| AtomicUsize::new(0)).collect(),
            tail: UnsafeCell::new(ArenaTail::default()),
            used: AtomicUsize::new(0),
        }
    }

    /// Writer: `n` contiguous bytes, at the offset returned.
    fn reserve(&self, n: usize) -> usize {
        // SAFETY: writer-only, see the memtable's doc.
        let t = unsafe { &mut *self.tail.get() };
        if t.at + n > t.block_end {
            let b = t.next_block;
            assert!(
                b < ARENA_BLOCKS,
                "memtable arena: more bytes than it addresses"
            );
            let cap = ARENA_BLOCK.max((n + 63) & !63);
            let layout = std::alloc::Layout::from_size_align(cap, 64).expect("arena block layout");
            // SAFETY: a non-zero layout; the block is freed by `Drop`.
            let p = unsafe { std::alloc::alloc_zeroed(layout) };
            assert!(!p.is_null(), "memtable arena: out of memory");
            self.caps[b].store(cap, AtomicOrdering::Release);
            self.blocks[b].store(p, AtomicOrdering::Release);
            t.at = b << ARENA_SHIFT;
            t.block_end = t.at + cap;
            t.next_block = b + cap.div_ceil(ARENA_BLOCK);
        }
        let off = t.at;
        t.at += n;
        self.used.fetch_add(n, AtomicOrdering::Relaxed);
        off
    }

    /// Writer: the tail, where the next reservation starts.
    fn tail(&self) -> usize {
        // SAFETY: writer-only.
        unsafe { (*self.tail.get()).at }
    }

    fn block(&self, off: usize, len: usize) -> (*mut u8, usize) {
        let b = off >> ARENA_SHIFT;
        let w = off & (ARENA_BLOCK - 1);
        let p = self.blocks[b].load(AtomicOrdering::Acquire);
        assert!(
            !p.is_null(),
            "memtable arena: an offset past what was published"
        );
        assert!(
            w + len <= self.caps[b].load(AtomicOrdering::Acquire),
            "memtable arena: a read past its block"
        );
        (p, w)
    }

    /// Writer: `bytes` into a reserved range at `off`.
    fn write(&self, off: usize, bytes: &[u8]) {
        let (p, w) = self.block(off, bytes.len());
        // SAFETY: inside the block, a range this writer reserved and no
        // reader can reach until it is published.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(w), bytes.len()) };
    }

    /// Anyone: published bytes at `off`.
    fn slice(&self, off: usize, len: usize) -> &[u8] {
        let (p, w) = self.block(off, len);
        // SAFETY: inside a block that lives as long as the arena, bytes
        // the writer wrote before publishing the offset that names them.
        unsafe { std::slice::from_raw_parts(p.add(w), len) }
    }

    /// A hint to fetch the lines at `off`, clipped to the block; nothing
    /// is read.
    fn prefetch(&self, off: usize, len: usize) {
        let b = off >> ARENA_SHIFT;
        let w = off & (ARENA_BLOCK - 1);
        let p = self.blocks[b].load(AtomicOrdering::Acquire);
        if p.is_null() {
            return;
        }
        let cap = self.caps[b].load(AtomicOrdering::Acquire);
        if w < cap {
            // SAFETY: a hint over addresses inside the block.
            prefetch_lines(unsafe { p.add(w) }, len.min(cap - w));
        }
    }

    fn used(&self) -> usize {
        self.used.load(AtomicOrdering::Relaxed)
    }
}

impl Drop for ByteArena {
    fn drop(&mut self) {
        for (b, cap) in self.blocks.iter().zip(self.caps.iter()) {
            let p = b.load(AtomicOrdering::Relaxed);
            if !p.is_null() {
                let cap = cap.load(AtomicOrdering::Relaxed);
                let layout =
                    std::alloc::Layout::from_size_align(cap, 64).expect("arena block layout");
                // SAFETY: allocated by `reserve` with this layout.
                unsafe { std::alloc::dealloc(p, layout) };
            }
        }
    }
}

/// Entries in blocks that never move, numbered in push order. A block
/// holds `SLAB_BLOCK` entries; a push writes the entry and then publishes
/// the length, so a reader that loaded the length may read every entry
/// below it, and one handed a number by an index slot may read that one.
const SLAB_SHIFT: u32 = 16;
const SLAB_BLOCK: usize = 1 << SLAB_SHIFT;
const SLAB_BLOCKS: usize = 1 << 12;

struct Slab<T> {
    blocks: Box<[AtomicPtr<T>]>,
    len: AtomicUsize,
}

impl<T> Slab<T> {
    fn new() -> Slab<T> {
        Slab {
            blocks: (0..SLAB_BLOCKS)
                .map(|_| AtomicPtr::new(std::ptr::null_mut()))
                .collect(),
            len: AtomicUsize::new(0),
        }
    }

    fn len(&self) -> usize {
        self.len.load(AtomicOrdering::Acquire)
    }

    /// Writer: `v` at the next number, published.
    fn push(&self, v: T) -> usize {
        let id = self.len.load(AtomicOrdering::Relaxed);
        let (b, w) = (id >> SLAB_SHIFT, id & (SLAB_BLOCK - 1));
        assert!(
            b < SLAB_BLOCKS,
            "memtable: more entries than the slab addresses"
        );
        let mut p = self.blocks[b].load(AtomicOrdering::Acquire);
        if p.is_null() {
            let layout = std::alloc::Layout::array::<T>(SLAB_BLOCK).expect("slab block layout");
            // SAFETY: a non-zero layout; the block is freed by `Drop`.
            p = unsafe { std::alloc::alloc_zeroed(layout) } as *mut T;
            assert!(!p.is_null(), "memtable: out of memory");
            self.blocks[b].store(p, AtomicOrdering::Release);
        }
        // SAFETY: inside the block, at a number no reader has been given.
        unsafe { std::ptr::write(p.add(w), v) };
        self.len.store(id + 1, AtomicOrdering::Release);
        id
    }

    /// A published entry.
    fn get(&self, id: usize) -> &T {
        debug_assert!(
            id < self.len(),
            "memtable: an entry number past the published length"
        );
        let p = self.blocks[id >> SLAB_SHIFT].load(AtomicOrdering::Acquire);
        assert!(
            !p.is_null(),
            "memtable: an entry number past what was published"
        );
        // SAFETY: a published entry in a block that lives as long as the
        // slab.
        unsafe { &*p.add(id & (SLAB_BLOCK - 1)) }
    }

    /// Writer: forget the entries from `n` on. Only for a table that will
    /// never be pushed to again -- the ordered table leaving a direct run,
    /// frozen right after -- since a reader handed a number past `n`
    /// still reads the entry there, and a push would overwrite it.
    fn truncate(&self, n: usize) {
        debug_assert!(n <= self.len());
        self.len.store(n, AtomicOrdering::Release);
    }
}

impl<T> Drop for Slab<T> {
    fn drop(&mut self) {
        // The entries are plain data with nothing to drop: this slab holds
        // `MemEntry` only.
        for b in self.blocks.iter() {
            let p = b.load(AtomicOrdering::Relaxed);
            if !p.is_null() {
                let layout = std::alloc::Layout::array::<T>(SLAB_BLOCK).expect("slab block layout");
                // SAFETY: allocated by `push` with this layout.
                unsafe { std::alloc::dealloc(p as *mut u8, layout) };
            }
        }
    }
}

/// The reader table: LMDB's shape. A reader handle owns a slot for its
/// life and pins by storing the epoch it observed there; the writer
/// bumps the epoch when it publishes a structure that replaces another,
/// tags the replaced one with the new epoch, and frees it once no slot
/// holds an older epoch, so a reader that pinned before the publish keeps
/// what it may still be walking. Readers store and the writer scans;
/// neither locks, and neither ever waits for the other -- a reader that
/// holds a pin only holds memory.
pub(crate) struct Readers {
    epoch: AtomicU64,
    slots: Box<[AtomicU64]>,
}

const READER_SLOTS: usize = 256;

impl Readers {
    fn new() -> Readers {
        Readers {
            epoch: AtomicU64::new(1),
            slots: (0..READER_SLOTS).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// The writer: a new epoch, after publishing what it replaces.
    fn bump(&self) -> u64 {
        self.epoch.fetch_add(1, AtomicOrdering::SeqCst) + 1
    }

    /// Whether every pinned reader pinned at or after `epoch`, so nothing
    /// retired at `epoch` can still be walked.
    fn none_before(&self, epoch: u64) -> bool {
        self.slots.iter().all(|s| {
            let v = s.load(AtomicOrdering::SeqCst);
            v == 0 || v >= epoch
        })
    }

    /// A slot for a reader handle's life, or none when every slot is
    /// taken.
    fn claim(&self) -> Option<usize> {
        (0..READER_SLOTS).find(|&i| {
            self.slots[i]
                .compare_exchange(0, u64::MAX, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
                .is_ok()
        })
    }

    fn release(&self, slot: usize) {
        self.slots[slot].store(0, AtomicOrdering::SeqCst);
    }

    /// Pin the current epoch in `slot`: a load, a store, and the load
    /// again, so an epoch the writer bumped between the two is not the one
    /// left pinned. Between operations a claimed slot holds `u64::MAX`,
    /// which no retirement is ever older than.
    fn pin(&self, slot: usize) {
        loop {
            let e = self.epoch.load(AtomicOrdering::SeqCst);
            self.slots[slot].store(e, AtomicOrdering::SeqCst);
            if self.epoch.load(AtomicOrdering::SeqCst) == e {
                return;
            }
        }
    }

    fn unpin(&self, slot: usize) {
        self.slots[slot].store(u64::MAX, AtomicOrdering::SeqCst);
    }
}

impl MemTable {
    fn new() -> MemTable {
        MemTable {
            entries: Slab::new(),
            index: AtomicPtr::new(Box::into_raw(Box::new(Index::with_slots(1024)))),
            ordered: false,
            keys: ByteArena::new(),
            vals: ByteArena::new(),
            tombs: AtomicUsize::new(0),
            committed: AtomicU64::new(0),
            retired: UnsafeCell::new(Vec::new()),
            log: Slab::new(),
        }
    }

    /// A table for keys that arrive in order: each above the last.
    fn new_ordered() -> MemTable {
        MemTable {
            entries: Slab::new(),
            index: AtomicPtr::new(std::ptr::null_mut()),
            ordered: true,
            keys: ByteArena::new(),
            vals: ByteArena::new(),
            tombs: AtomicUsize::new(0),
            committed: AtomicU64::new(0),
            retired: UnsafeCell::new(Vec::new()),
            log: Slab::new(),
        }
    }

    /// The writes so far, for a handle's `log_at`.
    fn log_len(&self) -> usize {
        self.log.len()
    }

    /// The `i`th write: the entry it wrote and whether it made it.
    fn log_at(&self, i: usize) -> (usize, bool) {
        let w = *self.log.get(i);
        ((w & 0x7fff_ffff) as usize, w >> 31 != 0)
    }

    /// Writer: the write logged, after the entry it names is published.
    fn log_write(&self, id: usize, new: bool) {
        self.log.push(id as u32 | ((new as u32) << 31));
    }

    fn index(&self) -> Option<&Index> {
        let p = self.index.load(AtomicOrdering::Acquire);
        // SAFETY: an index the writer published, freed only past every
        // reader that could hold it.
        (!p.is_null()).then(|| unsafe { &*p })
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entry(&self, id: usize) -> &MemEntry {
        self.entries.get(id)
    }

    fn key_of(&self, e: &MemEntry) -> &[u8] {
        self.keys.slice(e.key_off as usize, e.key_len as usize)
    }

    fn key_at(&self, off: u32, len: u32) -> &[u8] {
        self.keys.slice(off as usize, len as usize)
    }

    fn key_bytes(&self) -> usize {
        self.keys.used()
    }

    fn tombs(&self) -> usize {
        self.tombs.load(AtomicOrdering::Relaxed)
    }

    /// The offset of the entry's newest chunk, or `NO_CHUNK`.
    fn head(e: &MemEntry) -> u64 {
        match e.head.load(AtomicOrdering::Acquire) {
            0 => NO_CHUNK,
            h => h - 1,
        }
    }

    /// Writer: the entries from `n` on forgotten; see `Slab::truncate`.
    fn truncate_entries(&self, n: usize) {
        self.entries.truncate(n);
    }

    /// Writer: what is written is committed.
    fn commit(&self) {
        self.committed
            .store(self.vals.tail() as u64, AtomicOrdering::Release);
    }

    /// The watermark a reader honours to see committed chunks only.
    fn committed(&self) -> u64 {
        self.committed.load(AtomicOrdering::Acquire)
    }

    /// Writer: the entry for `key`, made if the table lacks it -- its key
    /// copied, its first chunk pushed with no predecessor -- or found,
    /// with a chunk pushed onto its chain. `value` is `None` for a
    /// tombstone. `rd` is for an index rebuild's retirement.
    fn write(&self, hash: u64, key: &[u8], value: Option<&[u8]>, rd: &Readers) {
        if self.ordered {
            debug_assert!(
                value.is_some(),
                "a delete never reaches an ordered memtable"
            );
            debug_assert!(
                self.is_empty() || self.key_of(self.entry(self.len() - 1)) < key,
                "an ordered memtable takes each key above its last"
            );
            let id = self.new_entry(hash, key, value);
            self.log_write(id, true);
            return;
        }
        if let Some(id) = self.probe(hash, key) {
            let e = self.entry(id);
            let prev = MemTable::head(e);
            let off = match value {
                Some(v) => self.push_chunk(prev, v),
                None => self.push_tomb(prev),
            };
            e.head.store(off + 1, AtomicOrdering::Release);
            match value {
                Some(_) => {
                    e.count.fetch_add(1, AtomicOrdering::Relaxed);
                }
                None => e.count.store(0, AtomicOrdering::Relaxed),
            }
            self.log_write(id, false);
            return;
        }
        let id = self.new_entry(hash, key, value);
        self.index_insert(hash, id, rd);
        self.log_write(id, true);
    }

    fn append(&self, hash: u64, key: &[u8], value: &[u8], rd: &Readers) {
        self.write(hash, key, Some(value), rd)
    }

    /// A tombstone and then `value` on `key`'s chain, one probe for both:
    /// what a put is, and half the misses of a delete then an append at
    /// thirty million keys.
    fn put(&self, hash: u64, key: &[u8], value: &[u8], rd: &Readers) {
        assert!(!self.ordered, "a put never reaches an ordered memtable");
        if let Some(id) = self.probe(hash, key) {
            let e = self.entry(id);
            let tomb = self.push_tomb(MemTable::head(e));
            let off = self.push_chunk(tomb, value);
            e.head.store(off + 1, AtomicOrdering::Release);
            e.count.store(1, AtomicOrdering::Relaxed);
            self.log_write(id, false);
            return;
        }
        let id = self.new_entry(hash, key, None);
        let e = self.entry(id);
        let off = self.push_chunk(MemTable::head(e), value);
        e.head.store(off + 1, AtomicOrdering::Release);
        e.count.store(1, AtomicOrdering::Relaxed);
        self.index_insert(hash, id, rd);
        self.log_write(id, true);
    }

    /// End every value of `key` before this point: a tombstone chunk at the
    /// head of the chain, and the live count back to zero. A key never seen
    /// before gets an entry too, because the tombstone has older sources to
    /// mask even when this memtable holds nothing of its own.
    fn delete(&self, hash: u64, key: &[u8], rd: &Readers) {
        assert!(
            !self.ordered,
            "a delete never reaches an ordered memtable: the store leaves order first"
        );
        self.write(hash, key, None, rd)
    }

    /// Writer: a new entry, its key and first chunk written, published by
    /// the slab's length; the index is the caller's.
    fn new_entry(&self, hash: u64, key: &[u8], value: Option<&[u8]>) -> usize {
        let key_off = self.keys.reserve(key.len());
        self.keys.write(key_off, key);
        let (off, count) = match value {
            Some(v) => (self.push_chunk(NO_CHUNK, v), 1),
            None => (self.push_tomb(NO_CHUNK), 0),
        };
        self.entries.push(MemEntry {
            hash,
            key_off: u32::try_from(key_off).expect("a memtable's keys stay under four gigabytes"),
            key_len: key.len() as u32,
            head: AtomicU64::new(off + 1),
            count: AtomicU64::new(count),
        })
    }

    /// Writer: entry `id` into the index at `hash`, after a rebuild when
    /// the table is at half load.
    fn index_insert(&self, hash: u64, id: usize, rd: &Readers) {
        let idx = self.index().expect("a hashed table has an index");
        let idx = if self.len() * 2 > idx.slots.len() {
            self.rebuild_index(idx, rd)
        } else {
            idx
        };
        let mut i = (hash as usize) & idx.mask;
        while idx.slots[i].load(AtomicOrdering::Relaxed) != 0 {
            i = (i + 1) & idx.mask;
        }
        idx.slots[i].store(Index::word(hash, id), AtomicOrdering::Release);
    }

    /// Writer: a table twice the size with every entry reinserted,
    /// published, the old one retired at the epoch the publish bumps.
    fn rebuild_index(&self, old: &Index, rd: &Readers) -> &Index {
        let cap = old.slots.len() * 2;
        let fresh = Index::with_slots(cap);
        for id in 0..self.len() {
            let hash = self.entry(id).hash;
            let mut i = (hash as usize) & fresh.mask;
            while fresh.slots[i].load(AtomicOrdering::Relaxed) != 0 {
                i = (i + 1) & fresh.mask;
            }
            fresh.slots[i].store(Index::word(hash, id), AtomicOrdering::Relaxed);
        }
        let fresh = Box::into_raw(Box::new(fresh));
        let old = self.index.swap(fresh, AtomicOrdering::AcqRel);
        let tag = rd.bump();
        // SAFETY: writer-only; `old` was published by this table and is
        // owned by it until freed here or in `Drop`.
        unsafe {
            (*self.retired.get()).push((tag, Box::from_raw(old)));
        }
        self.reclaim(rd);
        // SAFETY: just published, freed only past every reader.
        unsafe { &*fresh }
    }

    /// Writer: free the retired indexes no reader can still hold.
    fn reclaim(&self, rd: &Readers) {
        // SAFETY: writer-only.
        let retired = unsafe { &mut *self.retired.get() };
        retired.retain(|(tag, _)| !rd.none_before(*tag));
    }

    fn push_chunk(&self, prev: u64, value: &[u8]) -> u64 {
        let off = self.vals.reserve(CHUNK_HDR + value.len());
        self.vals.write(off, &prev.to_le_bytes());
        self.vals
            .write(off + 8, &(value.len() as u32).to_le_bytes());
        self.vals.write(off + CHUNK_HDR, value);
        off as u64
    }

    fn push_tomb(&self, prev: u64) -> u64 {
        let off = self.vals.reserve(CHUNK_HDR);
        self.vals.write(off, &prev.to_le_bytes());
        self.vals.write(off + 8, &TOMB_LEN.to_le_bytes());
        self.tombs.fetch_add(1, AtomicOrdering::Relaxed);
        off as u64
    }

    fn chunk_prev(&self, off: u64) -> u64 {
        u64::from_le_bytes(
            self.vals
                .slice(off as usize, 8)
                .try_into()
                .expect("eight bytes"),
        )
    }

    fn chunk_len(&self, off: u64) -> u32 {
        u32::from_le_bytes(
            self.vals
                .slice(off as usize + 8, 4)
                .try_into()
                .expect("four bytes"),
        )
    }

    fn is_tomb(&self, off: u64) -> bool {
        self.chunk_len(off) == TOMB_LEN
    }

    /// The key's live values, oldest first, and whether a tombstone ends
    /// the chain -- in which case everything older, here and in every older
    /// source, is dead. Chunks at or past `wm` are skipped: a reader's
    /// watermark, or `SEE_ALL`.
    fn live_chain(&self, e: &MemEntry, wm: u64) -> (Vec<usize>, bool) {
        let mut offs = Vec::with_capacity(e.count.load(AtomicOrdering::Relaxed) as usize);
        let tomb = self.live_offs_into(e, &mut offs, wm);
        (offs, tomb)
    }

    /// `live_chain` into a caller's buffer, oldest first, allocating nothing
    /// after the buffer has grown once; returns whether a tombstone cut it.
    fn live_offs_into(&self, e: &MemEntry, out: &mut Vec<usize>, wm: u64) -> bool {
        out.clear();
        let mut at = MemTable::head(e);
        let mut tomb = false;
        while at != NO_CHUNK {
            if at >= wm {
                at = self.chunk_prev(at);
                continue;
            }
            if self.is_tomb(at) {
                tomb = true;
                break;
            }
            out.push(at as usize);
            at = self.chunk_prev(at);
        }
        out.reverse();
        tomb
    }

    /// Whether a tombstone sits anywhere in the key's chain below `wm`.
    fn has_tomb(&self, e: &MemEntry, wm: u64) -> bool {
        let mut at = MemTable::head(e);
        while at != NO_CHUNK {
            if at < wm && self.is_tomb(at) {
                return true;
            }
            at = self.chunk_prev(at);
        }
        false
    }

    fn value_at(&self, off: usize) -> &[u8] {
        let len = self.chunk_len(off as u64);
        debug_assert_ne!(len, TOMB_LEN, "a tombstone has no value");
        self.vals.slice(off + CHUNK_HDR, len as usize)
    }

    fn get(&self, key: &[u8]) -> Option<&MemEntry> {
        self.slot_of(key).map(|i| self.entry(i))
    }

    /// `get` with the hash `prefetch` returned.
    fn get_with(&self, hash: u64, key: &[u8]) -> Option<&MemEntry> {
        if self.ordered {
            return self.get(key);
        }
        self.probe(hash, key).map(|i| self.entry(i))
    }

    /// The entry number holding `key`, if the table has it.
    fn slot_of(&self, key: &[u8]) -> Option<usize> {
        if self.ordered {
            let n = self.len();
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let m = lo + (hi - lo) / 2;
                match self.key_of(self.entry(m)).cmp(key) {
                    Ordering::Less => lo = m + 1,
                    Ordering::Greater => hi = m,
                    Ordering::Equal => return Some(m),
                }
            }
            return None;
        }
        self.probe(mem_hash(key), key)
    }

    /// A hint to fetch the slot line `key` probes first, and its hash for
    /// the probe: issued at the top of a write or a read, the store's
    /// bookkeeping before the probe -- the WAL frame, the fences -- runs
    /// while the line comes in. At thirty million keys the slot and the
    /// entry behind it are both misses, and this hides the first.
    fn prefetch(&self, key: &[u8]) -> u64 {
        let hash = mem_hash(key);
        if let Some(idx) = self.index() {
            let i = (hash as usize) & idx.mask;
            prefetch_lines(idx.slots[i..].as_ptr() as *const u8, 64);
        }
        hash
    }

    fn probe(&self, hash: u64, key: &[u8]) -> Option<usize> {
        let idx = self.index()?;
        let mut i = (hash as usize) & idx.mask;
        loop {
            let s = idx.slots[i].load(AtomicOrdering::Acquire);
            if s == 0 {
                return None;
            }
            if s >> 32 == hash >> 32 {
                let id = ((s & 0xffff_ffff) - 1) as usize;
                let e = self.entry(id);
                if e.hash == hash && self.key_of(e) == key {
                    return Some(id);
                }
            }
            i = (i + 1) & idx.mask;
        }
    }
}

impl Drop for MemTable {
    fn drop(&mut self) {
        let p = self.index.load(AtomicOrdering::Relaxed);
        if !p.is_null() {
            // SAFETY: published by this table, owned by it.
            drop(unsafe { Box::from_raw(p) });
        }
        // The retired indexes drop with the cell.
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
/// emitter serves both ways of finding keys, so the two arms compared
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
            if (!p.lo.is_empty() && k < p.lo.as_slice())
                || p.hi.as_ref().is_some_and(|h| k >= h.as_slice())
            {
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
            let ord = w
                .finish()
                .map_err(|e| err(&format!("compact finish: {e}")))?;
            let p = &self.pieces[self.pi];
            write_ord(self.dir, &p.name, &ord)?;
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
            Blob::open_with(
                MmapBytes::open(&dir.join(name))?,
                crate::blob::BlobOptions {
                    verify_checksums: opts.checksums,
                    ..Default::default()
                },
            )
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
                // An open fence is the empty vector, and nothing is below
                // it; see `Seg::below_lo` for why it is not compared.
                let from = if lo.is_empty() {
                    0
                } else {
                    keys.partition_point(|k| k < lo.as_slice())
                };
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

/// PROTOTYPE: records of one partition block a scan reads over unsealed
/// keys. Both the block's own records and the unsealed keys that fall in
/// its key range, each key once with its live values, in order, so a scan
/// over the block walks this and merges nothing.
#[derive(Default)]
struct CachedBlock {
    keys: Vec<u8>,
    vals: Vec<u8>,
    /// key offset, key length, run offset, run length; the run is values
    /// each behind a u32 length.
    ents: Vec<[u32; 4]>,
}

impl CachedBlock {
    fn begin(&mut self, key: &[u8]) {
        let e = [
            self.keys.len() as u32,
            key.len() as u32,
            self.vals.len() as u32,
            0,
        ];
        self.keys.extend_from_slice(key);
        self.ents.push(e);
    }
    fn push(&mut self, v: &[u8]) {
        self.vals.extend_from_slice(&(v.len() as u32).to_le_bytes());
        self.vals.extend_from_slice(v);
    }
    fn end(&mut self) {
        if let Some(e) = self.ents.last_mut() {
            e[3] = self.vals.len() as u32 - e[2];
        }
    }
    fn key(&self, e: &[u32; 4]) -> &[u8] {
        &self.keys[e[0] as usize..(e[0] + e[1]) as usize]
    }
    /// First entry whose key is not below `from`.
    fn lower_bound(&self, from: &[u8]) -> usize {
        self.ents.partition_point(|e| self.key(e) < from)
    }
    /// PROTOTYPE: pull the entries and the keys toward the core, and the
    /// first lines of the values, ahead of a walk. After a pass over
    /// hundreds of megabytes of blocks a copy's three buffers are cold,
    /// and the lower bound over its entries is six dependent misses into
    /// two of them; issued together they cost one, and the values stream
    /// behind the walk once its first lines are in.
    fn prefetch(&self) {
        prefetch_lines(self.ents.as_ptr() as *const u8, self.ents.len() * 16);
        prefetch_lines(self.keys.as_ptr(), self.keys.len());
        prefetch_lines(self.vals.as_ptr(), self.vals.len().min(1024));
    }
    fn each_value(&self, e: &[u32; 4], mut f: impl FnMut(&[u8])) {
        let run = &self.vals[e[2] as usize..(e[2] + e[3]) as usize];
        let mut p = 0usize;
        while p < run.len() {
            let n = u32::from_le_bytes(run[p..p + 4].try_into().unwrap()) as usize;
            f(&run[p + 4..p + 4 + n]);
            p += 4 + n;
        }
    }
}

/// PROTOTYPE: a hint to the core to fetch the lines of a buffer, none of
/// which it waits for; on any other architecture, nothing.
#[inline]
pub(crate) fn prefetch_lines(ptr: *const u8, bytes: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
        let mut off = 0usize;
        while off < bytes {
            // SAFETY: a prefetch is a hint that faults on no address, and
            // every address here is inside the buffer.
            unsafe { _mm_prefetch(ptr.add(off) as *const i8, _MM_HINT_T0) };
            off += 64;
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (ptr, bytes);
}

/// PROTOTYPE: the lines a walk of block `b` from rank `from` for `n`
/// entries touches first, by the block's form: a copy's or a sparse
/// block's buffers, and the partition's records a sparse or clean walk
/// streams. Nothing is waited for, and a block not yet built has nothing
/// to fetch.
fn prefetch_block(blob: &Blob<MmapBytes>, table: &BlockTable, b: usize, from: usize, n: usize) {
    match table.slots[b].as_ref() {
        Some(Cached::Block(blk)) => blk.prefetch(),
        Some(Cached::Sparse(sb)) => {
            sb.prefetch();
            blob.prefetch_ranks(from, n);
        }
        Some(Cached::Clean) => blob.prefetch_ranks(from, n),
        _ => {}
    }
}

/// PROTOTYPE: what the cache knows about a block.
enum Cached {
    /// No unsealed key falls in it: the partition's own records are the
    /// answer, walked directly.
    Clean,
    /// A few overlay keys fall in it, each resolved once: the rank it
    /// cuts the partition's walk at, whether that rank is the key itself,
    /// and its values, copied. The block is walked in the partition with
    /// these slipped in at their cuts, so the walk restarts only where a
    /// key falls and reads nothing from the pieces or the memtable. The
    /// values were references into their sources once: at thirty million
    /// keys each emission was a read into a piece's cold page, half a
    /// microsecond, and a sparse walk cost 2.5x a copy's.
    Sparse(SparseBlock),
    /// Dense with unsealed keys: one merged copy, walked without a merge.
    Block(CachedBlock),
    /// PROTOTYPE: too many keys above the partition to hold as either --
    /// the last block of the last partition, whose range is open above
    /// and collects every key inserted past the end. Its sources are
    /// sorted already, so a scan seeks each of them to its start and
    /// merges only the entries it walks; the block keeps just the filed
    /// keys in key order, and a write never drops it, since it holds no
    /// value. Before this the block was a copy of every such key, rebuilt
    /// after every inserting commit: 7 MB and 12 ms a rebuild by the end
    /// of ycsb-E on a store of three million keys.
    Wide(WideBlock),
}

/// PROTOTYPE: a wide block's filed keys in key order, and how many of
/// the block's filed entries that order covers; the rest are merged in
/// at the next walk.
#[derive(Default)]
struct WideBlock {
    sorted: Vec<(u32, u32)>,
    seen: usize,
}

/// PROTOTYPE: keys above the partition, as `overlay_count` counts them,
/// past which a block is walked as a merge instead of built.
const WIDE: usize = 4 * CACHE_BLOCK;

/// PROTOTYPE: a sparse block's keys above the partition, resolved once:
/// each key's bytes, where it cuts the walk, and its values as (source,
/// position) pairs read from the source at walk time -- the partition or
/// a piece by rank, a memtable by value offset. The values were copied
/// into the block before; measured against references on one store of
/// three million keys, the copies bought a sparse block nothing and cost
/// a seventh of the cache. The dense copy keeps its bytes: there,
/// references cost the hot region most of a microsecond a scan. A walk
/// over the block touches three allocations however many keys it has.
#[derive(Default)]
struct SparseBlock {
    keys: Vec<u8>,
    ents: Vec<DeltaEnt>,
    /// Every delta's values, each behind a u32 length, as `CachedBlock`
    /// holds its runs.
    vals: Vec<u8>,
}

/// PROTOTYPE: one key of a sparse block: where its key sits in the
/// block's keys, the first rank of the partition's records not below it,
/// whether that rank is the key itself -- whose values are then in the
/// run already and which the walk steps over -- and its run in `vals`.
struct DeltaEnt {
    key: (u32, u32),
    cut: u32,
    same: bool,
    run: (u32, u32),
}

impl SparseBlock {
    /// PROTOTYPE: the block's three buffers toward the core ahead of a
    /// walk, as a copy's; the lower bound over the deltas is the same
    /// dependent misses into two of them.
    fn prefetch(&self) {
        prefetch_lines(
            self.ents.as_ptr() as *const u8,
            self.ents.len() * std::mem::size_of::<DeltaEnt>(),
        );
        prefetch_lines(self.keys.as_ptr(), self.keys.len());
        prefetch_lines(self.vals.as_ptr(), self.vals.len().min(1024));
    }
    fn key(&self, e: &DeltaEnt) -> &[u8] {
        &self.keys[e.key.0 as usize..(e.key.0 + e.key.1) as usize]
    }
    fn each_value<F: FnMut(&[u8])>(&self, e: &DeltaEnt, mut f: F) {
        let run = &self.vals[e.run.0 as usize..(e.run.0 + e.run.1) as usize];
        let mut p = 0usize;
        while p + 4 <= run.len() {
            let n = u32::from_le_bytes(run[p..p + 4].try_into().expect("four")) as usize;
            p += 4;
            f(&run[p..p + n]);
            p += n;
        }
    }
}

impl Cached {
    /// PROTOTYPE: what the block holds beyond its slot, for the budget.
    fn bytes(&self) -> usize {
        match self {
            Cached::Clean => 0,
            Cached::Sparse(b) => b.keys.len() + b.ents.len() * 24 + b.vals.len(),
            Cached::Block(b) => b.keys.len() + b.vals.len() + b.ents.len() * 16,
            Cached::Wide(w) => w.sorted.len() * 8,
        }
    }
}

/// PROTOTYPE: overlay keys in a block from which a merged copy pays;
/// below it the block is resolved deltas over the partition's own walk.
/// Measured on ycsb-E at 300k keys: copying every block with an unsealed
/// key held 31 MB and ran slower than copying only the blocks with eight
/// or more, since a block the Zipfian tail touches once or twice never
/// repays its copy, and copying by touch count instead was slower still.
/// At thirty million, on the store A, F and D leave -- 82 pieces, most
/// blocks with a few overlay keys and a tenth with eight or more --
/// sixteen built E's first pass 6-13% faster than eight at half the
/// memory and tied its second; sixty-four lost 10% on the first and a
/// quarter on the second, a walk cut every few keys costing more than
/// the copy it saves. Rounds interleaved, one machine.
const CACHE_DENSE: usize = 16;

/// PROTOTYPE: records a block spans. A scan of the suite's length touches
/// one or two.
const CACHE_BLOCK: usize = 64;

/// PROTOTYPE: one key of a block's overlay, the sources above the
/// partition that hold it: the memtables (`sk`, the snapshot's entry) and
/// level-0 pieces (`pieces`: the piece's index among the store's level-0
/// segments, oldest first, and the key's rank in it).
struct Over<'a> {
    key: &'a [u8],
    sk: Option<SnapKey>,
    /// The key's rank in the partition, shifted left one with the low bit
    /// for an equal partition key, or `u32::MAX` when no source carried
    /// it and the build searches.
    cut: u32,
    /// The key's entries in `Overlay::held`, contiguous: piece index and
    /// rank, oldest piece first.
    pieces: std::ops::Range<u32>,
}

/// PROTOTYPE: a block's keys above the partition, in order, and the piece
/// entries they refer to, in one list so a key allocates nothing.
struct Overlay<'a> {
    over: Vec<Over<'a>>,
    /// Key, piece index, rank in the piece, and the key's cut from the
    /// piece's ranks or `u32::MAX`.
    held: Vec<(&'a [u8], usize, usize, u32)>,
}

/// PROTOTYPE: what emitting an overlay key needs beyond the key: whether
/// any source holds a tombstone, and a scratch buffer for memtable chains.
struct Emit {
    tombs: bool,
    scratch: Vec<usize>,
}

/// PROTOTYPE: what a block is built from: its partition and the store's
/// level-0 pieces, oldest first.
#[derive(Clone, Copy)]
struct Sources<'a> {
    seg: &'a Seg,
    l0: &'a [std::sync::Arc<Seg>],
}

/// PROTOTYPE: a partition's block cache, with what every block's build
/// needs found once for the whole partition instead of by seeks per
/// block: where the level-0 pieces' keys and the snapshot's fall against
/// the block boundaries, and the keys created since the snapshot, filed
/// by block as they are written.
struct BlockTable {
    slots: Vec<Option<Cached>>,
    /// The scan count when each block was last walked.
    touched: Vec<u32>,
    /// Each block's index in `Db::built`, or `u32::MAX` when unlisted.
    listed: Vec<u32>,
    /// How many keys the per-block lists hold, so a partition with no
    /// piece meeting it, no snapshot key in its range and nothing filed
    /// is known to be clean throughout without a look at any block.
    filed: usize,
    /// Per level-0 piece meeting the partition's range: its index in the
    /// level, and the first rank not below each block's lower bound, one
    /// more for the partition's upper fence, so block `b` holds the
    /// piece's ranks `at[b]..at[b + 1]`.
    pieces: Vec<(usize, Vec<u32>)>,
    /// The same over the snapshot of unsealed keys, for the snapshot
    /// `snap_gen` names; walked again when a rebuild replaces it.
    snap_at: Vec<u32>,
    snap_gen: u64,
    /// Live slots created since that snapshot, under the block their key
    /// falls in, in creation order, each with the cut the write's seek
    /// found.
    added: Vec<Vec<(u32, u32)>>,
}

impl BlockTable {
    /// PROTOTYPE: nothing above the partition anywhere in its range: no
    /// piece meets it, the snapshot's run over it is empty, and nothing
    /// was filed since. A scan then walks the partition's records as the
    /// bulk walk does, with no block touched.
    fn clean_throughout(&self) -> bool {
        self.pieces.is_empty() && self.filed == 0 && self.snap_at.first() == self.snap_at.last()
    }
}

/// PROTOTYPE: where a sorted source's positions fall against a
/// partition's block boundaries: the first position not below each
/// block's lower bound, and last the first not below the partition's
/// upper fence, so block `b` holds positions `at[b]..at[b + 1]`. The two
/// fences are answered by `seek`; between them the source is walked with
/// `below`, reading each boundary key of the partition once. An open
/// fence is the source's start or end, never a compare against an empty
/// slice.
fn block_bounds_of(
    seg: &Seg,
    nblocks: usize,
    len: usize,
    seek: impl Fn(&[u8]) -> usize,
    below: impl Fn(usize, &[u8]) -> bool,
) -> Result<Vec<u32>> {
    let mut at = Vec::with_capacity(nblocks + 1);
    let mut i = if seg.lo.is_empty() { 0 } else { seek(&seg.lo) };
    at.push(i as u32);
    for b in 1..nblocks {
        let bound = seg
            .blob
            .key_at(b * CACHE_BLOCK)
            .ok_or_else(|| err("block cache: a rank did not resolve"))?;
        while i < len && below(i, bound) {
            i += 1;
        }
        at.push(i as u32);
    }
    let end = match &seg.hi {
        Some(h) => seek(h).max(i),
        None => len,
    };
    at.push(end as u32);
    Ok(at)
}

/// What a reader reads: the store as of a publish. The writer builds a
/// new one at every seal, join, merge or freeze and swaps it in whole,
/// so a reader that loaded the old one keeps it, unchanged, until its
/// operation ends; the old one is freed past every pinned reader.
struct State {
    /// Live segments. Partitioned (L1) first and disjoint, then L0 oldest
    /// to newest: a key's values come back in append order because a merge
    /// preserves it and everything L0 holds is newer than everything L1
    /// holds.
    segs: Vec<std::sync::Arc<Seg>>,
    mem: std::sync::Arc<MemTable>,
    /// A seal in flight: the frozen memtable stays readable (it is newer
    /// than every segment and older than `mem`) while a thread writes it
    /// out; `join_seal` collects the finished segment.
    frozen: Option<std::sync::Arc<MemTable>>,
    /// Bumped at every publish: what a scan snapshot and a block table
    /// are keyed by, so either is rebuilt when the segments or the
    /// memtables change under it.
    gen: u64,
    /// Mean bytes a key costs across the live segments, kept rather than
    /// recomputed because `scan` asks it on every call and a fold over the
    /// segments there measured 5% of an in-core scan. Refreshed by
    /// `sort_segs`, which every mutation of `segs` already ends with.
    mean_key_bytes: usize,
    /// The partitions' bytes on disk, refreshed with the segment set: what
    /// a merge rewrites, and so what the seal is sized against.
    store_bytes: u64,
    /// Whether every level-0 piece is aligned to a partition -- its fence
    /// one partition's -- so a read finds the pieces over its key by
    /// binary search instead of a walk over all of them; see
    /// `pieces_over`. Refreshed by `sort_segs`.
    l0_aligned: bool,
}

/// What every handle on one store shares: the state, the reader table
/// the writer frees against, and the mappings' advice mode.
struct Shared {
    state: AtomicPtr<State>,
    /// The reader table: slots for reader handles, and the epoch the
    /// writer bumps at each publish.
    readers: Readers,
    /// States a publish replaced, each with the epoch it was retired at,
    /// freed once no reader is pinned before it. The writer's, behind a
    /// lock only because the shared struct must be `Sync`; it is never
    /// contended.
    retired: std::sync::Mutex<Vec<(u64, Box<State>)>>,
    /// Which mode the segment mappings are in: `true` is `MADV_RANDOM`.
    /// One flag for the whole store rather than one per segment, because
    /// the phase is a property of what the caller is doing and not of
    /// which file answers; the mappings are shared, so the flag is.
    advice_random: std::sync::atomic::AtomicBool,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let p = self.state.load(AtomicOrdering::Relaxed);
        if !p.is_null() {
            // SAFETY: published by the writer, owned here.
            drop(unsafe { Box::from_raw(p) });
        }
    }
}

/// What a reader handle sees of the writer's work: the reader's choice,
/// and one mechanism. Chunks are appended to the memtable's value arena
/// in time order, so the arena's tail at the last commit is a watermark
/// that divides every key's chain into an uncommitted prefix and a
/// committed rest, and a read walks past what it does not honour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Isolation {
    /// Each read sees every commit before it, and nothing of the batch
    /// the writer is still staging. The default.
    #[default]
    Latest,
    /// Each read sees the store as it was at `Reader::snapshot`, whatever
    /// the writer commits, seals or merges after, until `Reader::release`.
    Snapshot,
    /// Each read sees the writer's staged batch too: what the store's own
    /// reads see, the read-your-writes contract `Db::read_all` set.
    Dirty,
}

/// A handle that reads the store: the read API, over the state the
/// writer publishes, with caches of its own -- the scan snapshot and the
/// block tables -- so that no two handles share anything but what the
/// writer published. A `Db` is one of these plus the writer; another
/// comes from `Db::reader`, is `Send`, and reads from whatever thread
/// holds it while the writer keeps writing, with no lock between them.
pub struct Reader {
    shared: std::sync::Arc<Shared>,
    /// This handle's slot in the reader table, or none for the writer's
    /// own handle, under which nothing is ever freed.
    slot: Option<usize>,
    isolation: std::cell::Cell<Isolation>,
    /// The state pinned by `snapshot`, or null: what `state` answers
    /// instead of the writer's latest while it is held. An atomic only
    /// so the handle is `Send`; one thread touches it.
    held: AtomicPtr<State>,
    /// The watermark the operation in progress reads the live memtable
    /// under: the last commit's under `Latest`, the snapshot's under
    /// `Snapshot`, none under `Dirty` and for the writer's own handle.
    wm: std::cell::Cell<u64>,
    opts: Options,
    /// PROTOTYPE: the block cache's table for each segment, by position
    /// in `segs`, made on first use and dropped whenever the segments
    /// change. One load finds a block; nothing is hashed. This handle's
    /// own: a reader handle has tables of its own over the same segments.
    tables: std::cell::RefCell<Vec<std::cell::RefCell<Option<BlockTable>>>>,
    /// Sorted keys of the unsealed sources (memtable + frozen), built lazily
    /// by `scan` and reused until a write or a seal changes what is
    /// unsealed. Without this, every scan walked the whole memtable: the
    /// ext-kv scan phase spent 15 minutes a rep in that walk, twice -- once
    /// through the live table and once through the frozen one.
    scan_keys: std::cell::RefCell<Option<(u64, Snapshot)>>,
    /// PROTOTYPE: whether any partition holds a cached block, so a write
    /// with nothing cached skips the lookup that would drop one.
    cache_used: std::cell::Cell<bool>,
    /// PROTOTYPE: bytes the built blocks hold, kept exact at every build,
    /// eviction and drop, for the budget.
    cache_bytes: std::cell::Cell<usize>,
    /// PROTOTYPE: counts scans on the block path; a block records the
    /// count when walked, and the budget sheds the block with the oldest.
    scan_tick: std::cell::Cell<u32>,
    /// PROTOTYPE: the state of the sampler that picks blocks to shed.
    shed_seed: std::cell::Cell<u64>,
    /// PROTOTYPE: keys written since the last scan on the block path, as
    /// (key offset, key length, created) in the live memtable's arena,
    /// with a run of writes to one key recorded once. A write used to seek
    /// the key in its partition to drop the block it lands in; the suite's
    /// ycsb-A, which follows a scan phase, paid that seek twice an update
    /// and lost a fifth. The next scan drops and files the distinct keys
    /// at once, and a mix that never scans never pays.
    pending: std::cell::RefCell<Vec<(u32, u32, bool)>>,
    /// PROTOTYPE: every built block holding bytes, as (partition index,
    /// block), so the sampler draws from blocks and never from empty
    /// slots. Sampling slots was tried: with a tenth of them built, a
    /// round of eight misses ended the shedding and one large block left
    /// the cache twice its budget.
    built: std::cell::RefCell<Vec<(u32, u32)>>,
    /// PROTOTYPE: the live memtable rehashed since the snapshot was built,
    /// so its slot indices are stale and the next scan rebuilds.
    /// PROTOTYPE: counts the snapshot's rebuilds, so a table can tell
    /// whether its bounds were walked over the snapshot that stands.
    snap_gen: std::cell::Cell<u64>,
    /// PROTOTYPE: slots of the keys the live memtable gained since the
    /// scan snapshot was built, sorted by key, so a commit does not force
    /// a rebuild: materialization merges these with the snapshot's keys.
    snap_added: std::cell::RefCell<Vec<u32>>,
    /// How far into the memtable's write log this handle has looked, and
    /// the generation it looked in: a new generation is a new memtable
    /// or a new segment set, and the log is read from the start again.
    log_seen: std::cell::Cell<usize>,
    log_gen: std::cell::Cell<u64>,
    /// How many of the live memtable's entries the scan snapshot covers:
    /// a key the log says was created at a number below it is in the
    /// snapshot already, not a key to file.
    snap_entries: std::cell::Cell<usize>,
    /// PROTOTYPE: the builder ahead of the reader, while one is running
    /// or has forms still to install.
    ahead: Option<Ahead>,
}

pub struct Db {
    r: Reader,
    dir: PathBuf,
    wal: Wal,
    wal_id: u64,
    mem_bytes: usize,
    /// The segment ordered ingest streams into, while one is open. The
    /// memtable is ordered exactly while a run is forming or open.
    direct: Option<Direct>,
    /// The store's greatest key, or empty for none: what a key has to be
    /// above to go direct.
    max_key: Vec<u8>,
    /// Scratch for the run a value would encode as, measured at `append`
    /// against the inline limit.
    run_scratch: Vec<u8>,
    /// Direct segments' temp names, unlinked once the manifest names the
    /// segment; until then the temp name is what recovery reads.
    retiring_tmps: Vec<PathBuf>,
    /// An error from leaving order mid-batch, which `append` and `delete`
    /// cannot return: the next `commit` does.
    pending_err: Option<std::io::Error>,
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
    /// join found the seal unfinished, so it can be said which of these the
    /// 14% of the durable load in `phase_ns[1]` is.
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
    sealing: Option<std::thread::JoinHandle<Result<Vec<String>>>>,
}

/// A read in progress on a handle; dropping it ends the read.
struct Entered<'a> {
    r: &'a Reader,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        self.r.leave();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            self.shared.readers.release(slot);
        }
    }
}

impl std::ops::Deref for Db {
    type Target = Reader;
    fn deref(&self) -> &Reader {
        &self.r
    }
}

impl std::ops::DerefMut for Db {
    fn deref_mut(&mut self) -> &mut Reader {
        &mut self.r
    }
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
/// records instead of a pointer chase.
#[derive(Default)]
struct Snapshot {
    keys: Vec<u8>,
    ents: Vec<SnapKey>,
    /// Keys created since the build, on the merge paths: filed here from
    /// `snap_added` at each scan, in key order, so the snapshot outlives
    /// a write. A rebuild after every write batch was the whole cost of
    /// ycsb-E at thirty million keys without the block cache: 2,500
    /// rebuilds over two million unsealed keys. `fresh` takes each
    /// filing and is folded into `side` past a few hundred, so a filing
    /// moves a few hundred entries and not every key filed so far.
    side: Vec<SnapKey>,
    fresh: Vec<SnapKey>,
    /// How many of `snap_added` are filed.
    filed: usize,
}

/// Entries a filing may leave in `fresh` before it is folded into `side`.
const SNAP_FRESH: usize = 256;

/// A scan's position in a snapshot: one index per run, and the key at
/// the front is the least of the three runs' heads, folded when the main
/// run and a side run hold it -- a key in the frozen table written again
/// live after the build, whose live slot only the side run knows.
#[derive(Clone, Copy)]
struct SnapCursor {
    i: usize,
    j: usize,
    k: usize,
}

impl Snapshot {
    fn len(&self) -> usize {
        self.ents.len()
    }
    fn key_of(&self, e: &SnapKey) -> &[u8] {
        &self.keys[e.off as usize..(e.off + e.len) as usize]
    }
    fn seek_in(&self, run: &[SnapKey], from: &[u8]) -> usize {
        run.partition_point(|e| self.key_of(e) < from)
    }
    fn cursor(&self, from: &[u8]) -> SnapCursor {
        SnapCursor {
            i: self.seek(from),
            j: self.seek_in(&self.side, from),
            k: self.seek_in(&self.fresh, from),
        }
    }
    /// The key at the cursor and its entry, folded across the runs.
    fn peek(&self, c: SnapCursor) -> Option<(&[u8], SnapKey)> {
        let heads = [self.ents.get(c.i), self.side.get(c.j), self.fresh.get(c.k)];
        let mut best: Option<(&[u8], SnapKey)> = None;
        for e in heads.into_iter().flatten() {
            let k = self.key_of(e);
            best = match best {
                None => Some((k, *e)),
                Some((bk, _)) if k < bk => Some((k, *e)),
                Some((bk, be)) if k == bk => Some((
                    bk,
                    SnapKey {
                        mem: if e.mem != u32::MAX { e.mem } else { be.mem },
                        frozen: if e.frozen != u32::MAX {
                            e.frozen
                        } else {
                            be.frozen
                        },
                        ..be
                    },
                )),
                other => other,
            };
        }
        best
    }
    /// Past the key at the cursor, in every run that holds it.
    fn advance(&self, c: &mut SnapCursor) {
        let Some((key, _)) = self.peek(*c) else {
            return;
        };
        let key: &[u8] = key;
        if self.ents.get(c.i).is_some_and(|e| self.key_of(e) == key) {
            c.i += 1;
        }
        if self.side.get(c.j).is_some_and(|e| self.key_of(e) == key) {
            c.j += 1;
        }
        if self.fresh.get(c.k).is_some_and(|e| self.key_of(e) == key) {
            c.k += 1;
        }
    }
    /// File the live memtable's slots `slots`, created since the build,
    /// as a sorted run: their keys copied into the arena, the batch
    /// sorted, merged into `fresh`, and `fresh` folded into `side` once
    /// it holds more than `SNAP_FRESH`.
    fn file(&mut self, mem: &MemTable, slots: &[u32]) {
        let mut batch: Vec<SnapKey> = Vec::with_capacity(slots.len());
        for &slot in slots {
            let e = mem.entry(slot as usize);
            let key = mem.key_of(e);
            let off = self.keys.len() as u32;
            self.keys.extend_from_slice(key);
            batch.push(SnapKey {
                off,
                len: key.len() as u32,
                mem: slot,
                frozen: u32::MAX,
            });
        }
        batch.sort_by(|a, b| self.key_of(a).cmp(self.key_of(b)));
        let fresh = std::mem::take(&mut self.fresh);
        self.fresh = self.merged(fresh, batch);
        if self.fresh.len() > SNAP_FRESH {
            let (side, fresh) = (
                std::mem::take(&mut self.side),
                std::mem::take(&mut self.fresh),
            );
            self.side = self.merged(side, fresh);
        }
    }
    fn merged(&self, a: Vec<SnapKey>, b: Vec<SnapKey>) -> Vec<SnapKey> {
        let mut out = Vec::with_capacity(a.len() + b.len());
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() && j < b.len() {
            if self.key_of(&a[i]) <= self.key_of(&b[j]) {
                out.push(a[i]);
                i += 1;
            } else {
                out.push(b[j]);
                j += 1;
            }
        }
        out.extend_from_slice(&a[i..]);
        out.extend_from_slice(&b[j..]);
        out
    }
    fn get(&self, i: usize) -> Option<(&[u8], &SnapKey)> {
        self.ents
            .get(i)
            .map(|e| (&self.keys[e.off as usize..(e.off + e.len) as usize], e))
    }
    /// First index whose key is not below `from`.
    ///
    /// The ends first: a start below every unsealed key -- every scan of a
    /// store whose inserts land past its loaded range, the YCSB shape -- is
    /// answered by one compare instead of a binary search over the snapshot,
    /// and a start above them all by two. A start inside the range pays
    /// those two compares on top of the search.
    fn seek(&self, from: &[u8]) -> usize {
        let key = |e: &SnapKey| &self.keys[e.off as usize..(e.off + e.len) as usize];
        match self.ents.first() {
            None => return 0,
            Some(e) if from <= key(e) => return 0,
            _ => {}
        }
        if self.ents.last().is_some_and(|e| from > key(e)) {
            return self.ents.len();
        }
        self.ents.partition_point(|e| key(e) < from)
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

impl Reader {
    /// The state the writer last published. Valid for as long as the
    /// borrow of this handle: the writer frees a state only past every
    /// pinned reader, and never under its own handle.
    fn state(&self) -> &State {
        let held = self.held.load(AtomicOrdering::Relaxed);
        let p = if held.is_null() {
            self.shared.state.load(AtomicOrdering::Acquire)
        } else {
            held
        };
        // SAFETY: see above; the pointer is never null after `open`, and a
        // held one is pinned.
        unsafe { &*p }
    }

    /// The watermark the operation in progress walks the live memtable
    /// under; see `Isolation`.
    fn wm(&self) -> u64 {
        self.wm.get()
    }

    /// The start of a read: under `Latest` and `Dirty` the handle's slot
    /// pins the epoch, so nothing the read walks is freed under it, the
    /// state is taken once and held for the read, so the writer's next
    /// publish cannot change the segments or the memtables under it
    /// midway, and the watermark is taken; under `Snapshot` all three
    /// were fixed at the snapshot. The writer's own handle pins nothing
    /// and holds nothing: it publishes nothing while a borrow of itself
    /// is out. The guard ends the read.
    fn enter(&self) -> Entered<'_> {
        if let Some(slot) = self.slot {
            match self.isolation.get() {
                Isolation::Snapshot => {}
                Isolation::Latest => {
                    self.shared.readers.pin(slot);
                    let p = self.shared.state.load(AtomicOrdering::Acquire);
                    self.held.store(p, AtomicOrdering::Relaxed);
                    // SAFETY: pinned above, so not freed under this handle.
                    self.wm.set(unsafe { &*p }.mem.committed());
                }
                Isolation::Dirty => {
                    self.shared.readers.pin(slot);
                    let p = self.shared.state.load(AtomicOrdering::Acquire);
                    self.held.store(p, AtomicOrdering::Relaxed);
                    self.wm.set(SEE_ALL);
                }
            }
        }
        Entered { r: self }
    }

    fn leave(&self) {
        if let Some(slot) = self.slot {
            if self.isolation.get() != Isolation::Snapshot {
                self.held
                    .store(std::ptr::null_mut(), AtomicOrdering::Relaxed);
                self.shared.readers.unpin(slot);
            }
        }
    }

    /// What this handle's reads see of the writer's work; `Latest` to
    /// begin with. Setting it releases a snapshot held.
    pub fn set_isolation(&self, isolation: Isolation) {
        if self.slot.is_none() {
            // The writer's own handle reads its own writes, always.
            return;
        }
        if self.isolation.get() == Isolation::Snapshot {
            self.release();
        }
        self.isolation.set(isolation);
    }

    pub fn isolation(&self) -> Isolation {
        self.isolation.get()
    }

    /// Hold the store as it is now -- the segments, the memtables and
    /// the last commit -- for every read until `release`. A snapshot
    /// held keeps what the writer replaces after it in memory and holds
    /// nothing else: the writer never waits for it.
    pub fn snapshot(&self) {
        let Some(slot) = self.slot else { return };
        if self.isolation.get() == Isolation::Snapshot {
            return;
        }
        self.shared.readers.pin(slot);
        let p = self.shared.state.load(AtomicOrdering::Acquire);
        self.held.store(p, AtomicOrdering::Relaxed);
        // SAFETY: pinned above, so not freed under this handle.
        self.wm.set(unsafe { &*p }.mem.committed());
        self.isolation.set(Isolation::Snapshot);
    }

    /// Let the snapshot go: reads see the latest commit again.
    pub fn release(&self) {
        let Some(slot) = self.slot else { return };
        if self.isolation.get() != Isolation::Snapshot {
            return;
        }
        self.held
            .store(std::ptr::null_mut(), AtomicOrdering::Relaxed);
        self.isolation.set(Isolation::Latest);
        self.shared.readers.unpin(slot);
    }

    fn segs(&self) -> &[std::sync::Arc<Seg>] {
        &self.state().segs
    }

    fn mem(&self) -> &std::sync::Arc<MemTable> {
        &self.state().mem
    }

    fn frozen(&self) -> Option<&std::sync::Arc<MemTable>> {
        self.state().frozen.as_ref()
    }

    fn set_advice_random(&self, random: bool) {
        self.shared
            .advice_random
            .store(random, AtomicOrdering::Relaxed);
    }
}

impl Reader {
    /// The live set, in the order the manifest should record it.
    fn live_names(&self) -> Vec<String> {
        self.segs().iter().map(|s| s.name.clone()).collect()
    }

    /// Whether any source can end a key's older values. False for a store
    /// nothing was ever deleted from, which lets every read skip the
    /// newest-first pass tombstones require.
    fn has_tombstones(&self) -> bool {
        self.mem().tombs() > 0
            || self.frozen().as_ref().is_some_and(|f| f.tombs() > 0)
            || self.segs().iter().any(|s| s.tombs)
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
            .segs()
            .iter()
            .filter(|s| s.level > 0)
            .map(|s| (s.lo.clone(), s.hi.clone()))
            .collect();
        if parts.is_empty() {
            return None;
        }
        if let Some(wide) = self
            .segs()
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
                    self.segs()
                        .iter()
                        .filter(|s| s.level == 0 && s.lo == f.0 && s.hi == f.1)
                        .count()
                        >= threshold
                })
                .collect(),
        )
    }

    /// The first of the partitions `segs[..np]` that may hold a key at or
    /// after `from`: they tile the key space in order, so it is the first
    /// whose upper fence is above `from`, and every partition after it may
    /// reach too. Found by galloping from the front and then a binary
    /// search over the bracket. A scan filtered every partition by its
    /// fence, a compare per partition below its start; a binary search
    /// over all of them was measured next and was slower on ycsb-E, whose
    /// Zipfian starts fall in the first few partitions, where the walk
    /// was one to three predictable compares and the search seven
    /// mispredicting ones. The gallop is one compare for a start in the
    /// first partition and logarithmic for a far one.
    fn first_reaching(&self, np: usize, from: &[u8]) -> usize {
        let parts = &self.segs()[..np];
        let below = |i: usize| parts[i].hi.as_ref().is_some_and(|h| h.as_slice() <= from);
        let (mut lo, mut step) = (0usize, 1usize);
        while step <= np && below(step - 1) {
            lo = step;
            step *= 2;
        }
        let end = step.min(np);
        lo + parts[lo..end].partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= from))
    }

    /// The level-0 pieces a read of a key in partition `at` consults. When
    /// every piece is aligned to a partition they are the run over `at`
    /// alone: the pieces sort by lower fence and then by name, so one
    /// range's pieces are consecutive and oldest first, and two binary
    /// searches bound the run. Otherwise all of them, each answering from
    /// its own fence, as every read did before: a walk over every piece
    /// in the store, two fence compares each, that grew with the range
    /// count times the pieces over a range.
    fn pieces_over(&self, np: usize, at: usize) -> &[std::sync::Arc<Seg>] {
        let l0 = &self.segs()[np..];
        if !self.state().l0_aligned || at >= np {
            return l0;
        }
        // The first partition's lower fence is empty, and so is that of
        // every piece aligned to it; see `below_lo` for why an empty fence
        // is never handed to a compare.
        let lo = self.segs()[at].lo.as_slice();
        let before =
            |s: &std::sync::Arc<Seg>| !lo.is_empty() && (s.lo.is_empty() || s.lo.as_slice() < lo);
        let same = |s: &std::sync::Arc<Seg>| {
            s.lo.is_empty() == lo.is_empty() && (lo.is_empty() || s.lo.as_slice() == lo)
        };
        let from = l0.partition_point(before);
        let to = from + l0[from..].partition_point(same);
        &l0[from..to]
    }

    /// The memtable bytes at which the next commit seals: `seal_bytes`, or
    /// with `seal_grows` the larger of that and the partitions' bytes over
    /// four times `l0_trigger`.
    pub fn seal_threshold(&self) -> usize {
        if !self.opts.seal_grows {
            return self.opts.seal_bytes;
        }
        let grown = self.state().store_bytes / (4 * self.opts.l0_trigger.max(1)) as u64;
        self.opts
            .seal_bytes
            .max(usize::try_from(grown).unwrap_or(usize::MAX))
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
            let b = &self.segs()[si].blob;
            if b.keys() == 0 {
                None
            } else {
                b.key_at(0).map(|k| k.to_vec())
            }
        };
        let last_of = |si: usize| -> Option<Vec<u8>> {
            let b = &self.segs()[si].blob;
            if b.keys() == 0 {
                None
            } else {
                b.key_at(b.keys() - 1).map(|k| k.to_vec())
            }
        };
        // An empty piece has nothing to promote; leave it to the merge.
        if pieces.iter().any(|&si| self.segs()[si].blob.keys() == 0) {
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

    // The starvation lesson that shaped `merge_due`: EVERY range that is
    // over its bound merges in one job, not just the worst. A per-range
    // merge has to run once per range where the whole-store merge ran once,
    // so picking a single range per seal starved it -- with sixteen ranges
    // and one merge in flight, pieces accumulated faster than they were
    // consumed and a read ended up walking ten of them. That starvation
    // cost more than the whole-store rewrite it replaced (the canonical read
    // comparison went from 0.846x to 0.561x), which is the measurement that
    // produced the rule.

    fn l0_len(&self) -> usize {
        self.segs().iter().filter(|s| s.level == 0).count()
    }

    /// Every value for `key`, in append order: partitions first, then L0
    /// oldest to newest, then the frozen memtable, then the live one.
    ///
    /// `may_hold` is the routing F38-F41 settled. A partition answers from
    /// its fence in two comparisons and no memory beyond the `Seg`; an L0
    /// segment answers from a Bloom in one cache line. Neither can produce
    /// a false negative, so a skipped segment is a segment that provably
    /// holds nothing for this key.
    /// Put the segment mappings in `random` if they are not already there.
    ///
    /// A no-op unless the mode actually changes, so the steady state costs a
    /// `Cell` load and a compare. On a change it is one `madvise` per live
    /// segment -- a cost priced over a store of several segments, since the
    /// earlier measurement was over a single mapping.
    /// Will this scan walk enough contiguous bytes for readahead to pay?
    ///
    /// The span is the limit times what a key costs on disk, taken from the
    /// segments themselves rather than assumed: `index_bytes / keys` over the
    /// live set. That is the whole section per key -- records, directory and
    /// hash -- where a scan walks only the records, so it reads high by about
    /// a quarter on the shape measured. It does not need to be tight. The
    /// crossing it is compared against is flat for a factor of three either
    /// side, and a quarter is well inside that.
    fn scan_wants_readahead(&self, limit: usize) -> bool {
        let mean = self.state().mean_key_bytes;
        if mean == 0 {
            // Nothing sealed: the scan is answered from the memtable and
            // touches no mapping, so the advice is moot. Say no and leave the
            // segments as they are rather than switching them for nothing.
            return false;
        }
        limit.saturating_mul(mean) >= self.opts.scan_readahead_bytes
    }

    fn advise(&self, random: bool) {
        if self.opts.read_advice != ReadAdvice::Adaptive || self.advice_random() == random {
            return;
        }
        self.set_advice_random(random);
        // Segments only. A segment's pages are walked in whichever way the
        // workload is walking them, so the advice follows the phase; the
        // ordered companion is reached only by a binary search and is left
        // on `MADV_RANDOM` in both.
        for s in self.segs() {
            if random {
                s.blob.advise_random();
            } else {
                s.blob.advise_normal();
            }
        }
    }

    /// Which mode the segment mappings are in: `true` is `MADV_RANDOM`.
    ///
    /// The store's own record of what it last asked for, which is what a
    /// check needs to compare against the mappings themselves -- the
    /// interesting failure is the two disagreeing.
    pub fn advice_random(&self) -> bool {
        self.shared.advice_random.load(AtomicOrdering::Relaxed)
    }

    /// Whether reads route to the pieces over their range, for a test to
    /// know which path it is on: `false` whenever a piece spans ranges.
    pub fn pieces_aligned(&self) -> bool {
        let _entered = self.enter();
        self.state().l0_aligned
    }

    /// How many segments' ordered companions were advised `MADV_RANDOM`, and
    /// how many there are.
    ///
    /// The companion is the mapping the advice policy missed. `advise` walks
    /// the segments and a companion is not one of them, so it sat on the
    /// kernel's default readahead however the store was configured, and
    /// nothing anywhere said so -- the only symptom was a scan that lost 12%
    /// out of core. A check needs to be able to ask.
    pub fn ords_advised(&self) -> (usize, usize) {
        let _entered = self.enter();
        (
            self.segs().iter().filter(|s| s.ord.advised()).count(),
            self.segs().len(),
        )
    }

    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let _entered = self.enter();
        self.advise(true);
        let hash = self.mem().prefetch(key);
        let np = self.segs().partition_point(|s| s.level > 0);
        let at = self.segs()[..np]
            .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let part = self.segs()[..np].get(at).filter(|s| s.may_hold(key));
        let l0 = self.pieces_over(np, at);
        // Sources oldest to newest: the partition (0), the level-0 pieces
        // (1..), the frozen memtable, the live one. `start` is the source
        // live values begin at: 0 unless a newer source holds a tombstone
        // for this key. Only a store with tombstones in it checks, and the
        // check is what a delete costs a read -- a second probe on the
        // sources that hold the key.
        let (fr_ix, mem_ix) = (1 + l0.len(), 2 + l0.len());
        let mut start = 0usize;
        if self.has_tombstones() {
            if !self.mem().is_empty() {
                if let Some(e) = self.mem().get_with(hash, key) {
                    if self.mem().has_tomb(e, self.wm()) {
                        start = mem_ix;
                    }
                }
            }
            if start == 0 {
                if let Some(fr) = self.frozen() {
                    if let Some(e) = fr.get(key) {
                        if fr.has_tomb(e, SEE_ALL) {
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
            if let Some(fr) = self.frozen() {
                if let Some(e) = fr.get(key) {
                    let (offs, _) = fr.live_chain(e, SEE_ALL);
                    n += offs.len() as u64;
                    for off in offs {
                        f(fr.value_at(off));
                    }
                }
            }
        }
        if mem_ix >= start && !self.mem().is_empty() {
            if let Some(e) = self.mem().get_with(hash, key) {
                let (offs, _) = self.mem().live_chain(e, self.wm());
                n += offs.len() as u64;
                for off in offs {
                    f(self.mem().value_at(off));
                }
            }
        }
        Ok(n)
    }

    /// The sorted snapshot of every unsealed key (frozen table first, then
    /// live), one entry per key. Two builds behind `scan_snapshot_arena`;
    /// both walk the hash tables once and both end in the same `Snapshot`.
    /// The snapshot over the frozen table and the live one's first
    /// `live_len` entries: the count is the caller's, taken once, since
    /// the writer may be appending while this builds, and what the
    /// snapshot covers is what the log replay must not file again.
    fn build_snapshot(&self, live_len: usize) -> Snapshot {
        let n = live_len + self.frozen().as_ref().map_or(0, |f| f.len());
        let mut snap = Snapshot {
            keys: Vec::with_capacity(
                self.mem().key_bytes() + self.frozen().as_ref().map_or(0, |f| f.key_bytes()),
            ),
            ents: Vec::with_capacity(n),
            ..Default::default()
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
                let upto = if live { live_len } else { mem.len() };
                order.extend((0..upto).map(|i| {
                    let e = mem.entry(i);
                    (e.key_off, e.key_len, i as u32)
                }));
                radix_by_first(&mut order, &mut scratch);
                for &(off, len, i) in &order {
                    let k = mem.key_at(off, len);
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
            if let Some(fr) = self.frozen() {
                take(fr, false);
            }
            take(self.mem(), true);
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
                let upto = if live { live_len } else { mem.len() };
                for (i, e) in (0..upto).map(|i| (i, mem.entry(i))) {
                    all.push(Old {
                        key: mem.key_of(e).to_vec(),
                        mem: if live { i as u32 } else { u32::MAX },
                        frozen: if live { u32::MAX } else { i as u32 },
                    });
                }
            };
            if let Some(fr) = self.frozen() {
                take(fr, false);
            }
            take(self.mem(), true);
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
        let _entered = self.enter();
        self.advise(!self.scan_wants_readahead(limit));
        // `Prefetch` never changes mode, so `advise` above is a no-op for it.
        // Instead the segments are told exactly which value bytes this scan
        // will walk, before it walks them. Errors are dropped: a plan that
        // cannot be built is a scan that fetches the way it always did, not a
        // scan that fails.
        if self.opts.read_advice == ReadAdvice::Prefetch {
            for seg in self.segs() {
                let _ = seg.blob.prefetch_scan(from, limit);
            }
        }
        let gen = self.state().gen;
        // The snapshot outlives writes on both paths. With the block
        // cache, the keys created since it was built are filed by block
        // and merged in when a block builds, until they outnumber the
        // snapshot; filed by block, what grows with their count is only
        // the walk a new table makes over them. On the merge paths they
        // are filed into the snapshot's side runs at each scan, until they
        // reach an eighth of it, since a scan merges those runs over its
        // whole length. Before this a write was a rebuild on those paths,
        // and at thirty million keys ycsb-E made 2,500 of them over two
        // million unsealed keys. The cache wants partitions to hang blocks
        // on: before the first partitioning it stands aside.
        let use_cache =
            self.opts.scan_block_cache && self.segs().first().is_some_and(|s| s.level > 0);
        self.sync_log();
        if use_cache {
            self.settle_pending()?;
        }
        {
            let mut cache = self.scan_keys.borrow_mut();
            let mut stale = cache.as_ref().is_none_or(|(g, _)| *g != gen);
            if !stale {
                let held = cache.as_ref().map_or(0, |(_, s)| s.len());
                let added = self.snap_added.borrow().len();
                stale = if use_cache {
                    added > held.max(4096)
                } else {
                    added > (held / 8).max(4096)
                };
            }
            if !stale && !use_cache {
                let (_, snap) = cache.as_mut().expect("not stale");
                let added = self.snap_added.borrow();
                if added.len() > snap.filed {
                    snap.file(self.mem(), &added[snap.filed..]);
                    snap.filed = added.len();
                }
            }
            if stale {
                let live_len = self.mem().len();
                self.snap_entries.set(live_len);
                *cache = Some((gen, self.build_snapshot(live_len)));
                self.snap_added.borrow_mut().clear();
                // Every key created since the old snapshot is in the new
                // one: the lists that held them are emptied, and the
                // bounds each table walked are walked again on its next
                // touch.
                self.snap_gen.set(self.snap_gen.get().wrapping_add(1));
                let np = self.segs().partition_point(|s| s.level > 0);
                let tables = self.tables.borrow();
                for p in 0..np {
                    if let Some(t) = tables[p].borrow_mut().as_mut() {
                        for list in &mut t.added {
                            list.clear();
                        }
                        t.filed = 0;
                        for b in 0..t.slots.len() {
                            if matches!(t.slots[b], Some(Cached::Wide(_))) {
                                self.unlist(p, b, t);
                            }
                        }
                    }
                }
            }
        }
        let cache = self.scan_keys.borrow();
        let unsealed = &cache.as_ref().expect("scan snapshot").1;
        // The block path finds the unsealed keys a block needs when it
        // builds the block, from the block's own bounds, and never from
        // this cursor. Seeking it anyway was a binary search over every
        // unsealed key on every scan, with its lower levels cold: on one
        // store of three million keys, half a microsecond of a far scan's
        // three, for a number nothing read.
        if use_cache {
            return self.scan_blocks(from, limit, unsealed, f);
        }
        let mut mc = unsealed.cursor(from);

        // With no level-0 piece the partitions tile the key space in order
        // and the unsealed keys are one sorted array, so the partitions can
        // be walked in bulk by `Blob::scan_at`, which resolves each key
        // once, with the unsealed keys laid over the walk where they fall.
        // The merge below costs five or six index lookups an entry (a
        // key_at per cursor to find the minimum, another to emit, and a
        // third inside `values_at`) where this costs one, and after a
        // routed flush this is the shape the store is in. An earlier
        // version had this path, a refactor dropped it, and the scan axis
        // paid for it.
        if !self.segs().iter().any(|s| s.level == 0) {
            return self.scan_partitions(from, limit, mc, unsealed, f);
        }

        if self.opts.scan_merge {
            return self.scan_merged(from, limit, mc, unsealed, f);
        }

        // A k-way merge over rank cursors, allocating nothing per key.
        //
        // The version before this one materialised every candidate key from
        // every source, sorted them and re-read each one: three copies and
        // a sort per key, which cost more than the reads. `Blob::key_at`
        // borrows out of the mapped index and the unsealed snapshot is
        // already sorted, so the merge can run on borrowed keys and emit
        // values straight from the position it is already holding.
        let mut cursors: Vec<(&std::sync::Arc<Seg>, usize)> = self
            .segs()
            .iter()
            .filter(|s| s.may_reach(from))
            .map(|s| (s, s.ord.seek(s.cursor_from(from), |r| s.blob.key_at(r))))
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
            if let Some((k, _)) = unsealed.peek(mc) {
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
            let in_unsealed = unsealed.peek(mc).map(|(k, _)| k) == Some(key);
            let mut start = 0usize;
            if tombs {
                if in_unsealed {
                    if self
                        .mem()
                        .get(key)
                        .is_some_and(|e| self.mem().has_tomb(e, self.wm()))
                    {
                        start = nc + 1;
                    } else if self
                        .frozen()
                        .as_ref()
                        .and_then(|fr| fr.get(key).map(|e| fr.has_tomb(e, SEE_ALL)))
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
                    if let Some(fr) = self.frozen() {
                        if let Some(e) = fr.get(key) {
                            for off in fr.live_chain(e, SEE_ALL).0 {
                                f(key, fr.value_at(off));
                            }
                        }
                    }
                }
                if nc + 1 >= start {
                    if let Some(e) = self.mem().get(key) {
                        for off in self.mem().live_chain(e, self.wm()).0 {
                            f(key, self.mem().value_at(off));
                        }
                    }
                }
                unsealed.advance(&mut mc);
            }
            seen += 1;
        }
        Ok(seen)
    }

    /// PROTOTYPE: every cached block dropped, and the flag that says a
    /// write need not look.
    /// PROTOTYPE: the writes since this handle last looked, from the
    /// memtable's log: the keys created since the snapshot, for filing,
    /// and every write's key, for settling into the block it landed in
    /// when the cache is in use. A new generation -- a seal, a join, a
    /// merge, a fresh memtable -- makes this handle's tables and snapshot
    /// stale, so they are dropped and the log is read from its start.
    fn sync_log(&self) {
        let st = self.state();
        if self.log_gen.get() != st.gen {
            self.log_gen.set(st.gen);
            self.log_seen.set(0);
            self.snap_entries.set(0);
            *self.scan_keys.borrow_mut() = None;
            self.snap_added.borrow_mut().clear();
            self.pending.borrow_mut().clear();
            self.drop_blocks();
            if self.tables.borrow().len() != st.segs.len() {
                *self.tables.borrow_mut() = Db::tables_for(st.segs.len());
            }
        }
        let mem = &st.mem;
        let n = mem.log_len();
        let seen = self.log_seen.get();
        if seen == n {
            return;
        }
        let file = self.cache_used.get();
        let covered = self.snap_entries.get();
        let mut added = self.snap_added.borrow_mut();
        let mut pending = self.pending.borrow_mut();
        for i in seen..n {
            let (id, new) = mem.log_at(i);
            // Created since the snapshot: a key the snapshot holds already
            // was created below its count, however late the log says so.
            let new = new && id >= covered;
            if new {
                added.push(id as u32);
            }
            if file {
                let e = mem.entry(id);
                match pending.last_mut() {
                    Some(last) if last.0 == e.key_off => last.2 |= new,
                    _ => pending.push((e.key_off, e.key_len, new)),
                }
            }
        }
        self.log_seen.set(n);
    }

    fn drop_blocks(&self) {
        for t in self.tables.borrow().iter() {
            *t.borrow_mut() = None;
        }
        self.cache_used.set(false);
        self.cache_bytes.set(0);
        self.built.borrow_mut().clear();
        self.pending.borrow_mut().clear();
    }

    /// PROTOTYPE: the forms the builder ahead has sent, installed: each
    /// into its partition's table, made here if the partition has none,
    /// if the segment set is still the one it was built at and the block
    /// is unbuilt and not past the wide bound; then the memtable's keys
    /// over the block spliced in -- the snapshot's run over it and the
    /// keys filed since, which is what a build here would have merged --
    /// so the installed block equals one built here.
    fn install_ahead(&self, unsealed: &Snapshot) -> Result<()> {
        let Some(a) = &self.ahead else {
            return Ok(());
        };
        let np = self.segs().partition_point(|s| s.level > 0);
        let l0 = &self.segs()[np..];
        while let Ok(built) = a.rx.try_recv() {
            // Every publish stops the builder before the set it built over
            // changes, so a form of another generation cannot arrive here
            // today; the check guards a path that sorts the segments
            // without one.
            if built.gen != self.state().gen {
                continue;
            }
            let Some(pi) = self.segs()[..np].iter().position(|s| s.name == built.name) else {
                continue;
            };
            let seg = &self.segs()[pi];
            let tables = self.tables.borrow();
            let mut held = tables[pi].borrow_mut();
            if held.is_none() {
                *held = Some(self.make_table(seg, l0, unsealed)?);
                self.cache_used.set(true);
            }
            let table = held.as_mut().expect("just made");
            if table.snap_gen != self.snap_gen.get() {
                table.snap_at = BuildCtx::snap_bounds(seg, table.slots.len(), unsealed)?;
                table.snap_gen = self.snap_gen.get();
            }
            let b = built.b as usize;
            if b >= table.slots.len()
                || table.slots[b].is_some()
                || BuildCtx::overlay_count(table, b) > WIDE
            {
                continue;
            }
            table.slots[b] = Some(built.form);
            self.list_built(pi, b, table);
            self.shed(pi, b, table);
            let (lo, hi) = (table.snap_at[b] as usize, table.snap_at[b + 1] as usize);
            let mut keys: Vec<(Vec<u8>, u32)> = Vec::with_capacity(hi - lo + table.added[b].len());
            for i in lo..hi {
                if let Some((k, _)) = unsealed.get(i) {
                    keys.push((k.to_vec(), u32::MAX));
                }
            }
            for &(slot, cut) in &table.added[b] {
                let k = self.mem().key_of(self.mem().entry(slot as usize));
                keys.push((k.to_vec(), cut));
            }
            for (k, cut) in keys {
                let cut = if cut == u32::MAX {
                    BuildCtx::owner_of(seg, &k).1
                } else {
                    cut
                };
                self.patch_block(pi, b, table, &k, cut)?;
            }
        }
        Ok(())
    }

    /// PROTOTYPE: the build context over this store's own state.
    fn build_ctx(&self) -> BuildCtx<'_> {
        BuildCtx {
            wm: self.wm(),
            segs: self.segs(),
            mem: self.mem(),
            frozen: self.frozen().map(|f| f.as_ref()),
            tombs: self.has_tombstones(),
        }
    }

    fn list_built(&self, p: usize, b: usize, table: &mut BlockTable) {
        let bytes = table.slots[b].as_ref().map_or(0, |c| c.bytes());
        self.cache_bytes.set(self.cache_bytes.get() + bytes);
        if bytes > 0 {
            let mut built = self.built.borrow_mut();
            table.listed[b] = built.len() as u32;
            built.push((p as u32, b as u32));
        }
    }

    /// PROTOTYPE: a block leaves the cache: its bytes leave the count and
    /// its place in the list goes to the last listed block, whose table
    /// is `table` when it is the one in hand and borrowed otherwise.
    fn unlist(&self, p: usize, b: usize, table: &mut BlockTable) -> usize {
        let Some(c) = table.slots[b].take() else {
            return 0;
        };
        let bytes = c.bytes();
        self.cache_bytes.set(self.cache_bytes.get() - bytes);
        let at = std::mem::replace(&mut table.listed[b], u32::MAX);
        if at != u32::MAX {
            let mut built = self.built.borrow_mut();
            let last = built.pop().expect("a listed block is in the list");
            if (at as usize) < built.len() {
                built[at as usize] = last;
                let (lp, lb) = (last.0 as usize, last.1 as usize);
                if lp == p {
                    table.listed[lb] = at;
                } else if let Some(t) = self.tables.borrow()[lp].borrow_mut().as_mut() {
                    t.listed[lb] = at;
                }
            }
        }
        bytes
    }

    /// PROTOTYPE: bring the cache under its budget by shedding built
    /// blocks: of eight drawn at random from the list of built blocks,
    /// the one walked longest ago, again until under. `cur` is the
    /// partition whose table the caller holds, reached through `table`
    /// rather than borrowed again, and `keep` the block it is about to
    /// walk, never shed: a budget below one block holds that block.
    fn shed(&self, cur: usize, keep: usize, table: &mut BlockTable) {
        let budget = self.opts.scan_cache_bytes;
        if budget == 0 {
            return;
        }
        let tick = self.scan_tick.get();
        while self.cache_bytes.get() > budget {
            let mut best: Option<(usize, usize, u32)> = None;
            {
                let built = self.built.borrow();
                if built.is_empty() {
                    break;
                }
                for _ in 0..8 {
                    let mut x = self.shed_seed.get();
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    self.shed_seed.set(x);
                    let (p, b) = built[(x as usize) % built.len()];
                    let (p, b) = (p as usize, b as usize);
                    if p == cur && b == keep {
                        continue;
                    }
                    let touched = if p == cur {
                        table.touched[b]
                    } else {
                        match self.tables.borrow()[p].borrow().as_ref() {
                            Some(t) => t.touched[b],
                            None => continue,
                        }
                    };
                    let age = tick.wrapping_sub(touched);
                    if best.is_none_or(|(_, _, a)| age > a) {
                        best = Some((p, b, age));
                    }
                }
            }
            let Some((p, b, _)) = best else { break };
            if p == cur {
                self.unlist(p, b, table);
            } else {
                // The other table's borrow must end before `unlist`
                // borrows a third table to fix the moved entry, so the
                // block is taken out through a short borrow of its own.
                let tables = self.tables.borrow();
                let mut held = tables[p].borrow_mut();
                let Some(t) = held.as_mut() else { break };
                if t.slots[b].is_none() {
                    break;
                }
                let c = t.slots[b].take().expect("checked");
                let bytes = c.bytes();
                self.cache_bytes.set(self.cache_bytes.get() - bytes);
                let at = std::mem::replace(&mut t.listed[b], u32::MAX);
                drop(held);
                if at != u32::MAX {
                    let mut built = self.built.borrow_mut();
                    let last = built.pop().expect("a listed block is in the list");
                    if (at as usize) < built.len() {
                        built[at as usize] = last;
                        let (lp, lb) = (last.0 as usize, last.1 as usize);
                        if lp == cur {
                            table.listed[lb] = at;
                        } else if let Some(t) = self.tables.borrow()[lp].borrow_mut().as_mut() {
                            t.listed[lb] = at;
                        }
                    }
                }
            }
        }
    }

    /// PROTOTYPE: the writes since the last scan on the block path, settled:
    /// each distinct key's block dropped from the cache, and a key created
    /// since the snapshot filed under its block when its partition has a
    /// table. The keys are sorted by arena offset so a key written many
    /// times costs one seek, the created one's record first so the flag
    /// survives the fold.
    fn settle_pending(&self) -> Result<()> {
        let mut pending = std::mem::take(&mut *self.pending.borrow_mut());
        if pending.is_empty() {
            return Ok(());
        }
        pending.sort_unstable_by_key(|&(off, _, new)| (off, !new));
        // A write that could not be settled leaves the block it landed in
        // stale, so an error drops every table rather than leave one.
        let settled = self.settle_each(&pending);
        if settled.is_err() {
            self.drop_blocks();
        }
        pending.clear();
        *self.pending.borrow_mut() = pending;
        settled
    }

    fn settle_each(&self, pending: &[(u32, u32, bool)]) -> Result<()> {
        let np = self.segs().partition_point(|s| s.level > 0);
        let mut last = u32::MAX;
        for &(off, len, new) in pending {
            if off == last {
                continue;
            }
            last = off;
            let key = self.mem().key_at(off, len);
            let at = self.segs()[..np]
                .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
            let Some(seg) = self.segs()[..np].get(at) else {
                continue;
            };
            if seg.blob.keys() == 0 {
                continue;
            }
            let tables = self.tables.borrow();
            let mut held = tables[at].borrow_mut();
            let Some(table) = held.as_mut() else {
                continue;
            };
            let (b, cut) = BuildCtx::owner_of(seg, key);
            if b < table.slots.len() {
                self.patch_block(at, b, table, key, cut)?;
            }
            if new {
                if let (Some(slot), Some(list)) = (self.mem().slot_of(key), table.added.get_mut(b))
                {
                    list.push((slot as u32, cut));
                    table.filed += 1;
                }
            }
            // Only a build chooses the wide form, and a patched block is
            // never rebuilt: past the bound, the block is dropped so the
            // next scan builds it wide, as the drop-and-rebuild did.
            if b < table.slots.len()
                && BuildCtx::overlay_count(table, b) > WIDE
                && !matches!(table.slots[b], Some(Cached::Wide(_)))
            {
                self.unlist(at, b, table);
            }
        }
        Ok(())
    }

    /// PROTOTYPE: a write settled into the block it landed in, in place:
    /// the key's run as a build would resolve it -- the partition's values
    /// for an equal key, then the pieces' and the memtables', a tombstone
    /// masking everything older -- spliced into whatever form the block
    /// holds, a clean block becoming sparse. Before this the block was
    /// dropped and rebuilt at the next scan that crossed it, a copy at 68
    /// µs at thirty million keys, so a mix that updates and scans the same
    /// keys rebuilt its hot blocks per write. A replaced run's bytes stay
    /// in the block until they outweigh the live ones, when the block is
    /// dropped instead. A wide block holds no values and is left alone; an
    /// unbuilt one has nothing to patch.
    fn patch_block(
        &self,
        at: usize,
        b: usize,
        table: &mut BlockTable,
        key: &[u8],
        cut: u32,
    ) -> Result<()> {
        if matches!(table.slots[b], None | Some(Cached::Wide(_))) {
            return Ok(());
        }
        let np = self.segs().partition_point(|s| s.level > 0);
        let seg = &self.segs()[at];
        let l0 = &self.segs()[np..];
        let src = Sources { seg, l0 };
        let keys = seg.blob.keys();
        let lo = b * CACHE_BLOCK;
        let hi = ((b + 1) * CACHE_BLOCK).min(keys);
        let mut held: Vec<(&[u8], usize, usize, u32)> = Vec::new();
        for &(j, _) in &table.pieces {
            let p = &l0[j];
            let r = p.ord.seek(key, |i| p.blob.key_at(i));
            if r < p.blob.keys() && p.blob.key_at(r) == Some(key) {
                held.push((key, j, r, u32::MAX));
            }
        }
        let slot_in = |t: &MemTable| t.slot_of(key).map_or(u32::MAX, |i| i as u32);
        let sk = SnapKey {
            off: 0,
            len: 0,
            mem: slot_in(self.mem()),
            frozen: self.frozen().as_ref().map_or(u32::MAX, |fr| slot_in(fr)),
        };
        let ov = Overlay {
            over: vec![Over {
                key,
                sk: Some(sk),
                cut,
                pieces: 0..held.len() as u32,
            }],
            held,
        };
        let (c, at_eq) = BuildCtx::cut_known(cut, lo, hi);
        let same = c < hi && at_eq == Ordering::Equal;
        let mut em = Emit {
            tombs: self.has_tombstones(),
            scratch: Vec::new(),
        };
        let mut run: Vec<u8> = Vec::new();
        self.build_ctx().emit_over(
            &mut |_, v: &[u8]| {
                run.extend_from_slice(&(v.len() as u32).to_le_bytes());
                run.extend_from_slice(v);
            },
            &mut em,
            &ov,
            0,
            same.then_some(c),
            src,
        )?;
        let before = table.slots[b].as_ref().map_or(0, |c| c.bytes());
        let was_clean = matches!(table.slots[b], Some(Cached::Clean));
        let mut bloated = false;
        match table.slots[b].as_mut().expect("checked above") {
            Cached::Block(blk) => {
                let i = blk.lower_bound(key);
                let at_run = blk.vals.len() as u32;
                blk.vals.extend_from_slice(&run);
                if i < blk.ents.len() && blk.key(&blk.ents[i]) == key {
                    blk.ents[i][2] = at_run;
                    blk.ents[i][3] = run.len() as u32;
                } else {
                    let key_at = blk.keys.len() as u32;
                    blk.keys.extend_from_slice(key);
                    blk.ents
                        .insert(i, [key_at, key.len() as u32, at_run, run.len() as u32]);
                }
                let live: usize = blk.ents.iter().map(|e| e[3] as usize).sum();
                bloated = blk.vals.len() > 2 * live.max(4096);
            }
            Cached::Sparse(sb) => {
                let i = sb.ents.partition_point(|e| sb.key(e) < key);
                let at_run = sb.vals.len() as u32;
                sb.vals.extend_from_slice(&run);
                if i < sb.ents.len() && sb.key(&sb.ents[i]) == key {
                    sb.ents[i].run = (at_run, run.len() as u32);
                } else {
                    let key_at = sb.keys.len() as u32;
                    sb.keys.extend_from_slice(key);
                    sb.ents.insert(
                        i,
                        DeltaEnt {
                            key: (key_at, key.len() as u32),
                            cut: c as u32,
                            same,
                            run: (at_run, run.len() as u32),
                        },
                    );
                }
                let live: usize = sb.ents.iter().map(|e| e.run.1 as usize).sum();
                bloated = sb.vals.len() > 2 * live.max(4096);
            }
            slot @ Cached::Clean => {
                *slot = Cached::Sparse(SparseBlock {
                    keys: key.to_vec(),
                    ents: vec![DeltaEnt {
                        key: (0, key.len() as u32),
                        cut: c as u32,
                        same,
                        run: (0, run.len() as u32),
                    }],
                    vals: run,
                });
            }
            Cached::Wide(_) => unreachable!("a wide block is left alone above"),
        }
        if was_clean {
            self.list_built(at, b, table);
        } else {
            let after = table.slots[b].as_ref().map_or(0, |c| c.bytes());
            self.cache_bytes
                .set(self.cache_bytes.get() + after - before);
        }
        if bloated {
            self.unlist(at, b, table);
        }
        Ok(())
    }

    /// PROTOTYPE: bookkeeping after a memtable write, when the block cache
    /// is on: a rehash renumbers every slot the snapshot and the lists
    /// hold, a created key joins the list of keys since the snapshot, and
    /// while any partition has a table the key joins the writes the next
    /// scan settles.
    /// PROTOTYPE: a partition's table, made on its first touch: every
    /// level-0 piece meeting its range walked once against its block
    /// boundaries, the snapshot walked once, and the keys created since
    /// the snapshot filed by block. Before this every block's build
    /// seeked each of those sources from scratch: on one store of three
    /// million keys, three microseconds of a seven microsecond build.
    fn make_table(
        &self,
        seg: &Seg,
        l0: &[std::sync::Arc<Seg>],
        unsealed: &Snapshot,
    ) -> Result<BlockTable> {
        let nblocks = seg.blob.keys().div_ceil(CACHE_BLOCK);
        let ctx = self.build_ctx();
        ctx.rank_pieces()?;
        let (pieces, snap_at) = ctx.table_bounds(seg, l0, unsealed)?;
        let mut added: Vec<Vec<(u32, u32)>> = (0..nblocks).map(|_| Vec::new()).collect();
        if nblocks > 0 {
            for &slot in self.snap_added.borrow().iter() {
                let key = self.mem().key_of(self.mem().entry(slot as usize));
                if seg.below_lo(key) || seg.hi.as_ref().is_some_and(|h| key >= h.as_slice()) {
                    continue;
                }
                let (b, cut) = BuildCtx::owner_of(seg, key);
                added[b].push((slot, cut));
            }
        }
        let filed = added.iter().map(Vec::len).sum();
        Ok(BlockTable {
            slots: (0..nblocks).map(|_| None).collect(),
            touched: vec![0; nblocks],
            listed: vec![u32::MAX; nblocks],
            pieces,
            snap_at,
            snap_gen: self.snap_gen.get(),
            added,
            filed,
        })
    }

    /// PROTOTYPE: the scan over partitions and everything above them,
    /// block by block: a cached copy is walked, a clean block is walked in
    /// the partition, a sparse one with its deltas, and a block not yet
    /// seen is built first.
    fn scan_blocks<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        unsealed: &Snapshot,
        mut f: F,
    ) -> Result<usize> {
        let np = self.segs().partition_point(|s| s.level > 0);
        let l0 = &self.segs()[np..];
        let tick = self.scan_tick.get().wrapping_add(1);
        self.scan_tick.set(tick);
        self.install_ahead(unsealed)?;
        // One context for the scan: its tombstone flag is a walk over every
        // segment, which a context per block paid on every sparse walk.
        let ctx = self.build_ctx();
        let mut seen = 0usize;
        let mut cursor: &[u8] = from;
        // The partitions tile the key space in order, so the first that
        // may reach the start is found by binary search and every one
        // after it may; filtering each in turn was a fence compare per
        // partition below the start, on every scan.
        let first = self.first_reaching(np, from);
        for (pi, seg) in self.segs()[..np].iter().enumerate().skip(first) {
            if seen >= limit {
                break;
            }
            let src = Sources { seg, l0 };
            cursor = seg.cursor_from(cursor);
            let keys = seg.blob.keys();
            if keys == 0 {
                match &seg.hi {
                    Some(h) => {
                        cursor = h.as_slice();
                        continue;
                    }
                    None => break,
                }
            }
            let rank = seg.ord.seek(cursor, |r| seg.blob.key_at(r));
            let owner = if rank < keys && seg.blob.key_at(rank) == Some(cursor) {
                rank
            } else {
                rank.saturating_sub(1)
            };
            let nblocks = keys.div_ceil(CACHE_BLOCK);
            let tables = self.tables.borrow();
            let mut held = tables[pi].borrow_mut();
            if held.is_none() {
                *held = Some(self.make_table(seg, l0, unsealed)?);
                self.cache_used.set(true);
            }
            let table = held.as_mut().expect("just made");
            if table.snap_gen != self.snap_gen.get() {
                table.snap_at = BuildCtx::snap_bounds(seg, nblocks, unsealed)?;
                table.snap_gen = self.snap_gen.get();
            }
            if table.clean_throughout() {
                // The suite's scan workload, and any store between a flush
                // and its next write: one walk from the seek, as the bulk
                // walk makes it. Measured through the blocks it was a
                // tenth slower, in first touches and bookkeeping.
                if rank < keys {
                    let want = (keys - rank).min(limit - seen);
                    let got = seg
                        .blob
                        .scan_at(rank, want, &mut f)
                        .map_err(|e| err(&format!("segment scan: {e}")))?;
                    if got < want {
                        return Err(err(
                            "segment scan: a partition's walk stopped short of its key count",
                        ));
                    }
                    seen += got;
                }
                match &seg.hi {
                    Some(h) => {
                        cursor = h.as_slice();
                        continue;
                    }
                    None => break,
                }
            }
            let mut b = owner / CACHE_BLOCK;
            let mut first = true;
            while seen < limit && b < nblocks {
                let lo = b * CACHE_BLOCK;
                let hi = ((b + 1) * CACHE_BLOCK).min(keys);
                let start = if first { rank.max(lo) } else { lo };
                let from_key: &[u8] = if first { cursor } else { b"" };
                if table.slots[b].is_none() {
                    let built = ctx.materialize(src, table, b, unsealed)?;
                    table.slots[b] = Some(built);
                    self.list_built(pi, b, table);
                    self.shed(pi, b, table);
                }
                table.touched[b] = tick;
                if let Some(Cached::Wide(w)) = table.slots[b].as_mut() {
                    // Filed since the order was made: sorted and merged in.
                    let filed = &table.added[b];
                    if filed.len() > w.seen {
                        let fresh = ctx.sorted_filed(&filed[w.seen..]);
                        let key_of = |slot: u32| self.mem().key_of(self.mem().entry(slot as usize));
                        let mut merged = Vec::with_capacity(w.sorted.len() + fresh.len());
                        let (mut i, mut j) = (0usize, 0usize);
                        while i < w.sorted.len() || j < fresh.len() {
                            let take_old = j >= fresh.len()
                                || (i < w.sorted.len()
                                    && key_of(w.sorted[i].0) <= key_of(fresh[j].0));
                            if take_old {
                                merged.push(w.sorted[i]);
                                i += 1;
                            } else {
                                merged.push(fresh[j]);
                                j += 1;
                            }
                        }
                        let grew = (merged.len() - w.sorted.len()) * 8;
                        w.sorted = merged;
                        w.seen = filed.len();
                        self.cache_bytes.set(self.cache_bytes.get() + grew);
                    }
                }
                // This block's cold lines, and the next block's when the
                // scan will cross into it, fetched while this one walks.
                let ahead = limit - seen;
                prefetch_block(&seg.blob, table, b, start, ahead);
                if hi - start < ahead && b + 1 < nblocks {
                    prefetch_block(&seg.blob, table, b + 1, hi, ahead - (hi - start));
                }
                match table.slots[b].as_ref().expect("just built") {
                    Cached::Sparse(deltas) => {
                        seen += ctx.walk_deltas(
                            src,
                            start..hi,
                            deltas,
                            from_key,
                            limit - seen,
                            &mut f,
                        )?;
                    }
                    Cached::Clean => {
                        // Every clean block built after this one joins the
                        // run: on a store with nothing unsealed, a scan of
                        // a hundred entries is one walk, as the bulk walk
                        // makes it, not three block-sized ones.
                        let mut run_hi = hi;
                        while b + 1 < nblocks
                            && run_hi - start < limit - seen
                            && matches!(table.slots[b + 1], Some(Cached::Clean))
                        {
                            b += 1;
                            table.touched[b] = tick;
                            run_hi = ((b + 1) * CACHE_BLOCK).min(keys);
                        }
                        if start < run_hi {
                            let want = (run_hi - start).min(limit - seen);
                            let got = seg
                                .blob
                                .scan_at(start, want, &mut f)
                                .map_err(|e| err(&format!("segment scan: {e}")))?;
                            if got < want {
                                return Err(err("segment scan: a partition's walk stopped short of its key count"));
                            }
                            seen += got;
                        }
                    }
                    Cached::Block(blk) => {
                        let i = if first { blk.lower_bound(cursor) } else { 0 };
                        for e in &blk.ents[i..] {
                            if seen >= limit {
                                break;
                            }
                            let k = blk.key(e);
                            blk.each_value(e, |v| f(k, v));
                            seen += 1;
                        }
                    }
                    Cached::Wide(w) => {
                        let window = (from_key, limit - seen);
                        let ov = ctx.overlay_window(src, table, b, unsealed, w, window)?;
                        seen +=
                            ctx.walk_block(src, start..hi, &ov, limit - seen, |_k| {}, &mut f)?;
                    }
                }
                b += 1;
                first = false;
            }
            match &seg.hi {
                Some(h) => cursor = h.as_slice(),
                None => break,
            }
        }
        Ok(seen)
    }

    /// PROTOTYPE: the cache's size, for a measurement: blocks held and
    /// bytes of keys and values in them.
    pub fn block_cache_size(&self) -> (usize, usize) {
        let (mut clean, mut sparse, mut copies, mut wide, mut bytes) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        for t in self.tables.borrow().iter() {
            for c in t.borrow().iter().flat_map(|t| t.slots.iter()).flatten() {
                match c {
                    Cached::Clean => clean += 1,
                    Cached::Sparse(b) => {
                        sparse += 1;
                        bytes += b.keys.len() + b.ents.len() * 24 + b.vals.len();
                    }
                    Cached::Block(b) => {
                        copies += 1;
                        bytes += b.keys.len() + b.vals.len() + b.ents.len() * 16;
                    }
                    Cached::Wide(w) => {
                        wide += 1;
                        bytes += w.sorted.len() * 8;
                    }
                }
            }
        }
        eprintln!(
            "  cache: {clean} clean, {sparse} sparse, {copies} copies, {wide} wide; {bytes} B walked, {} B counted",
            self.cache_bytes.get()
        );
        (clean + sparse + copies + wide, bytes)
    }

    /// PROTOTYPE: the bytes the cache counts itself holding, for a test
    /// to hold against a walk of it.
    pub fn block_cache_bytes(&self) -> usize {
        self.cache_bytes.get()
    }

    /// PROTOTYPE: the most bytes any one built block holds, the slack a
    /// budget allows since the block in hand is never shed.
    pub fn block_cache_largest(&self) -> usize {
        self.tables
            .borrow()
            .iter()
            .map(|t| {
                t.borrow().as_ref().map_or(0, |t| {
                    t.slots
                        .iter()
                        .flatten()
                        .map(|c| c.bytes())
                        .max()
                        .unwrap_or(0)
                })
            })
            .max()
            .unwrap_or(0)
    }

    /// PROTOTYPE: how many blocks the cache holds as wide, for a test to
    /// hold the count that makes one to its definition.
    pub fn block_cache_wide(&self) -> usize {
        self.tables
            .borrow()
            .iter()
            .map(|t| {
                t.borrow().as_ref().map_or(0, |t| {
                    t.slots
                        .iter()
                        .flatten()
                        .filter(|c| matches!(c, Cached::Wide(_)))
                        .count()
                })
            })
            .sum()
    }

    /// The partitions walked in bulk, with the unsealed keys laid over them.
    ///
    /// The caller has checked there is no level-0 piece, so the sources are
    /// the partitions, which tile the key space in order, and the two
    /// memtables, whose keys `unsealed` holds as one sorted array. Each
    /// partition is walked by `Blob::scan_at` -- one record decode an entry
    /// -- up to the next unsealed key that falls inside the walk. That key
    /// is then emitted as `scan_merged` would emit it, and the walk resumes
    /// after it. Unsealed keys beyond the last partition come out at the
    /// end, in order.
    ///
    /// Before this, the bulk walk ran only when no unsealed key was at or
    /// after `from`. YCSB's inserts land past the end of the loaded range,
    /// so after the first one every scan failed that test for keys it never
    /// reached and paid the merge: on one store in one process, a 5% insert
    /// past the end with no seal cost 100-entry scans 2.8x, and a flush gave
    /// it back.
    ///
    /// Whether the next unsealed key cuts a walk is decided by one key read,
    /// the last key the walk would reach, and only when it does is its rank
    /// found -- by a binary search over the walk's window, not the ordered
    /// index over the whole partition. A scan that meets no unsealed key
    /// pays one key read a partition for the question.
    fn scan_partitions<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        mut mc: SnapCursor,
        unsealed: &Snapshot,
        mut f: F,
    ) -> Result<usize> {
        // `sort_segs` orders by level descending then by `lo`, and this
        // runs only when every segment is a partition, so they are already
        // in key order here. Collecting them into a `Vec` to sort them
        // again repeated work the store had done -- and the collect, the
        // sort and the three `Vec` clones around the cursor measured 391ns
        // of a 648ns seek, six times what the ordered index saved. A scan
        // is a read; it allocates nothing until a memtable chain is walked.
        debug_assert!(
            self.segs()
                .windows(2)
                .all(|w| w[0].level != w[1].level || w[0].lo <= w[1].lo),
            "partitions are not in key order, so this walk would skip one"
        );
        // Whether any source holds a tombstone is asked of every segment,
        // and only an emitted unsealed key needs the answer, so a scan that
        // meets none never asks.
        let mut tombs: Option<bool> = None;
        let mut scratch: Vec<usize> = Vec::new();
        let mut seen = 0usize;
        let mut cursor: &[u8] = from;
        // Every segment is a partition here, and the first that may reach
        // the start is found by binary search; see `first_reaching`.
        let first = self.first_reaching(self.segs().len(), from);
        for seg in &self.segs()[first..] {
            if seen >= limit {
                break;
            }
            cursor = seg.cursor_from(cursor);
            let keys = seg.blob.keys();
            // The ordered index answers the seek this partition starts
            // with; the walk after it is the reader's own. That split is
            // the whole point of the index -- the seek was the entire
            // measured deficit and the walk was already competitive.
            let mut rank = seg.ord.seek(cursor, |r| seg.blob.key_at(r));
            while seen < limit {
                // The walk reaches the limit or the partition's end, cut
                // where the next unsealed key falls inside it. A rank that
                // does not resolve sorts as "not less", the rule the seek
                // uses, so damage widens the cut rather than moving it.
                let end = rank.saturating_add(limit - seen).min(keys);
                let next = unsealed.peek(mc);
                let (bound, at_bound) = match next {
                    Some((uk, _)) if end > rank => BuildCtx::cut_at(seg, rank, end, uk),
                    _ => (end, Ordering::Greater),
                };
                if bound > rank {
                    let got = seg
                        .blob
                        .scan_at(rank, bound - rank, &mut f)
                        .map_err(|e| err(&format!("segment scan: {e}")))?;
                    if got < bound - rank {
                        // A rank below the key count that the walk could
                        // not resolve. The seek above would have widened
                        // past it; the walk cannot, and saying nothing
                        // would drop every key after it.
                        return Err(err(
                            "segment scan: a partition's walk stopped short of its key count",
                        ));
                    }
                    seen += got;
                    rank += got;
                    if seen >= limit {
                        break;
                    }
                }
                // The walk stands at the unsealed key's cut, or at the end
                // of this partition's keys. The partition's own key is
                // below its fence by construction, so only another key is
                // asked whether it belongs past the fence, to the next
                // partition, or past the last partition's last key, to the
                // tail below.
                let Some((uk, sk)) = next else { break };
                let same = rank < keys && at_bound == Ordering::Equal;
                if !same {
                    if seg.hi.as_ref().is_some_and(|h| uk >= h.as_slice()) {
                        break;
                    }
                    if rank >= keys && seg.hi.is_none() {
                        break;
                    }
                }
                let tombs = *tombs.get_or_insert_with(|| self.has_tombstones());
                self.emit_unsealed(
                    &mut f,
                    &mut scratch,
                    tombs,
                    uk,
                    &sk,
                    same.then_some((seg, rank)),
                )?;
                if same {
                    rank += 1;
                }
                unsealed.advance(&mut mc);
                seen += 1;
            }
            match &seg.hi {
                Some(h) => cursor = h.as_slice(),
                None => break,
            }
        }
        // Unsealed keys after every partition, in order.
        while seen < limit {
            let Some((uk, sk)) = unsealed.peek(mc) else {
                break;
            };
            let tombs = *tombs.get_or_insert_with(|| self.has_tombstones());
            self.emit_unsealed(&mut f, &mut scratch, tombs, uk, &sk, None)?;
            unsealed.advance(&mut mc);
            seen += 1;
        }
        Ok(seen)
    }

    /// One unsealed key, emitted as `scan_merged` emits it with no level-0
    /// piece in the way: the partition's values first when `part` names an
    /// equal key, then the frozen memtable's, then the live one's, each
    /// older source cut by a tombstone in a newer. Sources are numbered
    /// partition 0, frozen 1, live 2; `start` is the oldest one whose
    /// values are live.
    fn emit_unsealed<F: FnMut(&[u8], &[u8])>(
        &self,
        f: &mut F,
        scratch: &mut Vec<usize>,
        tombs: bool,
        key: &[u8],
        sk: &SnapKey,
        part: Option<(&Seg, usize)>,
    ) -> Result<()> {
        let mut start = 0usize;
        if tombs {
            if sk.mem != u32::MAX
                && self
                    .mem()
                    .has_tomb(self.mem().entry(sk.mem as usize), self.wm())
            {
                start = 2;
            } else if sk.frozen != u32::MAX
                && self
                    .frozen()
                    .as_ref()
                    .is_some_and(|fr| fr.has_tomb(fr.entry(sk.frozen as usize), SEE_ALL))
            {
                start = 1;
            }
        }
        if start == 0 {
            if let Some((seg, rank)) = part {
                seg.blob
                    .values_at(rank, |v| f(key, v))
                    .map_err(|e| err(&format!("segment scan read: {e}")))?;
            }
        }
        if sk.frozen != u32::MAX && start <= 1 {
            if let Some(fr) = self.frozen() {
                let e = fr.entry(sk.frozen as usize);
                fr.live_offs_into(e, scratch, SEE_ALL);
                for &off in scratch.iter() {
                    f(key, fr.value_at(off));
                }
            }
        }
        if sk.mem != u32::MAX {
            let e = self.mem().entry(sk.mem as usize);
            self.mem().live_offs_into(e, scratch, self.wm());
            for &off in scratch.iter() {
                f(key, self.mem().value_at(off));
            }
        }
        Ok(())
    }

    /// The merge over unrouted sources, the `scan_merge` arm: one cursor walking the
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
        mut mc: SnapCursor,
        unsealed: &Snapshot,
        mut f: F,
    ) -> Result<usize> {
        let np = self.segs().partition_point(|s| s.level > 0);
        let parts = &self.segs()[..np];
        struct Cur<'a> {
            seg: &'a Seg,
            rank: usize,
            key: Option<&'a [u8]>,
        }
        // The level-0 cursors. With every piece aligned to a partition
        // they are the pieces over the range the walk is in, seeked to
        // its start there and re-seeked when it crosses into the next
        // range; a range is left once its partition and its pieces are
        // both exhausted, since a piece can hold keys above its
        // partition's last. Otherwise, every piece that may reach the
        // start, seeked once, and the walk of every key over every one:
        // what every scan did, and paid a cursor per piece in the store
        // at the seek and a compare per piece per key.
        let routed = self.state().l0_aligned;
        let seek_l0 = |pi: usize, from: &[u8]| -> Vec<Cur> {
            let pieces = if routed {
                self.pieces_over(np, pi)
            } else {
                &self.segs()[np..]
            };
            pieces
                .iter()
                .filter(|s| s.may_reach(from))
                .map(|s| {
                    let rank = s.ord.seek(s.cursor_from(from), |r| s.blob.key_at(r));
                    Cur {
                        seg: s,
                        rank,
                        key: s.blob.key_at(rank),
                    }
                })
                .collect()
        };
        // The partition cursor: the first partition whose fence can reach
        // `from`, then each following one from its first key.
        let mut pi = self.first_reaching(np, from);
        let mut prank = 0usize;
        let mut pkey: Option<&[u8]> = None;
        while pi < np {
            let s = &parts[pi];
            prank = s.ord.seek(s.cursor_from(from), |r| s.blob.key_at(r));
            pkey = s.blob.key_at(prank);
            if pkey.is_some() || routed {
                break;
            }
            pi += 1;
        }
        let mut l0: Vec<Cur> = seek_l0(pi.min(np), from);
        let tombs = self.has_tombstones();
        let mut scratch: Vec<usize> = Vec::new();
        let mut seen = 0usize;
        while seen < limit {
            if routed {
                while pkey.is_none() && l0.iter().all(|c| c.key.is_none()) && pi + 1 < np {
                    pi += 1;
                    prank = 0;
                    pkey = parts[pi].blob.key_at(0);
                    l0 = seek_l0(pi, parts[pi].lo.as_slice());
                }
            }
            let nc = l0.len();
            let mut next: Option<&[u8]> = pkey;
            for c in &l0 {
                if let Some(k) = c.key {
                    if next.is_none_or(|n| k < n) {
                        next = Some(k);
                    }
                }
            }
            let snap = unsealed.peek(mc);
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
                    if sk.mem != u32::MAX
                        && self
                            .mem()
                            .has_tomb(self.mem().entry(sk.mem as usize), self.wm())
                    {
                        start = nc + 2;
                    } else if sk.frozen != u32::MAX
                        && self
                            .frozen()
                            .as_ref()
                            .is_some_and(|fr| fr.has_tomb(fr.entry(sk.frozen as usize), SEE_ALL))
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
                while !routed && pkey.is_none() && pi + 1 < np {
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
                    if let Some(fr) = self.frozen() {
                        let e = fr.entry(sk.frozen as usize);
                        fr.live_offs_into(e, &mut scratch, SEE_ALL);
                        for &off in &scratch {
                            f(key, fr.value_at(off));
                        }
                    }
                }
                if sk.mem != u32::MAX && nc + 2 >= start {
                    let e = self.mem().entry(sk.mem as usize);
                    self.mem().live_offs_into(e, &mut scratch, self.wm());
                    for &off in &scratch {
                        f(key, self.mem().value_at(off));
                    }
                }
                unsealed.advance(&mut mc);
            }
            seen += 1;
        }
        Ok(seen)
    }

    /// Values of `key` across every source. O(extents) per segment touched:
    /// each extent carries its record count (`Ext::count`, format v5), so no
    /// block is read. The memtable keeps a live count per key.
    pub fn count(&self, key: &[u8]) -> Result<u64> {
        let _entered = self.enter();
        // A count resolves one key, so it is a point read for advice
        // purposes even though it returns no bytes (`F28`: 94 ns, a lookup).
        self.advise(true);
        let hash = self.mem().prefetch(key);
        let np = self.segs().partition_point(|s| s.level > 0);
        let at = self.segs()[..np]
            .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let part = self.segs()[..np].get(at).filter(|s| s.may_hold(key));
        let l0 = self.pieces_over(np, at);
        let (fr_ix, mem_ix) = (1 + l0.len(), 2 + l0.len());
        let mut start = 0usize;
        if self.has_tombstones() {
            if !self.mem().is_empty() {
                if let Some(e) = self.mem().get_with(hash, key) {
                    if self.mem().has_tomb(e, self.wm()) {
                        start = mem_ix;
                    }
                }
            }
            if start == 0 {
                if let Some(fr) = self.frozen() {
                    if let Some(e) = fr.get(key) {
                        if fr.has_tomb(e, SEE_ALL) {
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
            if let Some(fr) = self.frozen() {
                if let Some(e) = fr.get(key) {
                    n += e.count.load(AtomicOrdering::Relaxed);
                }
            }
        }
        if mem_ix >= start && !self.mem().is_empty() {
            if let Some(e) = self.mem().get_with(hash, key) {
                n += self.mem().live_chain(e, self.wm()).0.len() as u64;
            }
        }
        Ok(n)
    }

    /// Keys held by the unsealed sources: the live memtable and, while a
    /// seal is in flight, the frozen one. A key in both counts twice.
    pub fn unsealed_keys(&self) -> usize {
        let _entered = self.enter();
        self.mem().len() + self.frozen().as_ref().map_or(0, |f| f.len())
    }

    /// Live segment count by level: (partitioned, L0). The compaction
    /// experiment reports both, because "how many segments does a read
    /// touch" is the whole question.
    pub fn levels(&self) -> (usize, usize) {
        let _entered = self.enter();
        (self.segs().len() - self.l0_len(), self.l0_len())
    }
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

    /// A segment's ordered index is named by the two fields a promotion
    /// keeps -- the id and the covered end-sequence -- and not by the
    /// segment's file name, which a promotion rewrites. So a promotion has
    /// nothing to do here at all.
    fn ord_name(id: u64, end_seq: u64) -> String {
        format!("ord-{id:08}-{end_seq:016}.oidx")
    }

    fn ord_name_for(seg: &str) -> Option<String> {
        Some(Db::ord_name(Db::name_id(seg)?, Db::name_end_seq(seg)?))
    }

    fn name_id(name: &str) -> Option<u64> {
        Db::name_field(name, 0)
    }

    fn name_end_seq(name: &str) -> Option<u64> {
        Db::name_field(name, 1)
    }

    /// Unlink a retired segment, and the ordered index named after it when
    /// no live segment still claims that index.
    ///
    /// The index has to go here rather than wait for the sweep at open. That
    /// sweep is the backstop for a crash window; a merge is not a crash
    /// window, it is the steady state, so a process that merges for hours
    /// leaked one index per input it retired and nothing reclaimed them
    /// until the store was reopened -- a run at a hundred million keys was
    /// found with 53,596 files in one store directory, most of them indexes
    /// whose segments were long gone.
    ///
    /// The liveness check is not defensive. A promotion renames a segment
    /// and keeps the id and end-sequence its index is named by, so the
    /// retired name and the live one address the SAME index file; unlinking
    /// it by name alone took the index of a segment that was still open, and
    /// seven tests said so.
    fn retire_seg(&self, name: &str) {
        let _ = std::fs::remove_file(self.dir.join(name));
        let Some(ord) = Db::ord_name_for(name) else {
            return;
        };
        let claimed = self
            .segs()
            .iter()
            .any(|s| Db::ord_name_for(&s.name).as_deref() == Some(ord.as_str()));
        if !claimed {
            let _ = std::fs::remove_file(self.dir.join(&ord));
        }
    }

    fn segment_opts(opts: &Options) -> SegmentOptions {
        opts.segment.clone()
    }

    pub fn create(dir: &Path, opts: Options) -> Result<Db> {
        let starts_random = opts.read_advice.starts_random();
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
        let segs: Vec<std::sync::Arc<Seg>> = Vec::new();
        let mean_key_bytes = 0;
        let store_bytes = 0;
        let l0_aligned = false;
        let state = State {
            segs,
            mem: std::sync::Arc::new(MemTable::new()),
            frozen: None,
            gen: 1,
            mean_key_bytes,
            store_bytes,
            l0_aligned,
        };
        let shared = std::sync::Arc::new(Shared {
            state: AtomicPtr::new(Box::into_raw(Box::new(state))),
            readers: Readers::new(),
            retired: std::sync::Mutex::new(Vec::new()),
            advice_random: std::sync::atomic::AtomicBool::new(starts_random),
        });
        let r = Reader {
            shared,
            slot: None,
            isolation: std::cell::Cell::new(Isolation::Dirty),
            held: AtomicPtr::new(std::ptr::null_mut()),
            wm: std::cell::Cell::new(SEE_ALL),
            opts,
            tables: std::cell::RefCell::new(Vec::new()),
            scan_keys: std::cell::RefCell::new(None),
            cache_used: std::cell::Cell::new(false),
            cache_bytes: std::cell::Cell::new(0),
            scan_tick: std::cell::Cell::new(0),
            shed_seed: std::cell::Cell::new(0x9E37_79B9_7F4A_7C15),
            pending: std::cell::RefCell::new(Vec::new()),
            built: std::cell::RefCell::new(Vec::new()),
            snap_gen: std::cell::Cell::new(0),
            snap_added: std::cell::RefCell::new(Vec::new()),
            log_seen: std::cell::Cell::new(0),
            log_gen: std::cell::Cell::new(0),
            snap_entries: std::cell::Cell::new(0),
            ahead: None,
        };
        Ok(Db {
            r,
            dir: dir.to_path_buf(),
            wal,
            wal_id: 0,
            mem_bytes: 0,
            direct: None,
            max_key: Vec::new(),
            run_scratch: Vec::new(),
            retiring_tmps: Vec::new(),
            pending_err: None,
            next_seg: 0,
            sealing: None,
            compacting: None,
            unsynced: 0,
            phase_ns: [0; 3],
            retiring_wals: Vec::new(),
            spare_wals,
            covered_seq: 0,
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
        let (sealed, mut live) = match manifest_read(dir)? {
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
        // A direct segment a crash left open: its records up to the last
        // commit marker are the batches that were acknowledged, rewritten
        // as a piece over the last range -- the whole range before the
        // first partitioning -- and named live for the merge to promote.
        // A temp file whose id the manifest already names is a close that
        // published and never unlinked it.
        let mut direct_tmps: Vec<(u64, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if let Some(id) = name
                .strip_prefix("direct-")
                .and_then(|r| r.strip_suffix(".tmp"))
                .and_then(|r| r.parse::<u64>().ok())
            {
                direct_tmps.push((id, dir.join(&name)));
            }
        }
        for (id, tmp) in direct_tmps {
            if live.iter().any(|n| Db::name_id(n) == Some(id)) {
                let _ = std::fs::remove_file(&tmp);
                continue;
            }
            let recs = SegmentWriter::recover_direct(&tmp)?;
            if recs.is_empty() {
                let _ = std::fs::remove_file(&tmp);
                continue;
            }
            let last_lo: Option<Vec<u8>> = live
                .iter()
                .filter(|n| n.starts_with("par-"))
                .filter_map(|n| {
                    let f: Vec<&str> = n.trim_end_matches(".sup").split('-').collect();
                    (f.len() == 4 && f[3].is_empty())
                        .then(|| unhex(f[2]))
                        .flatten()
                })
                .next();
            let name = match &last_lo {
                Some(lo) => format!("pcs-{id:08}-{sealed:016}-{}-.sup", hex(lo)),
                None => Db::seg_name(id, sealed),
            };
            let rebuilt = dir.join(format!("seal-{id:08}.tmp"));
            let _ = std::fs::remove_file(&rebuilt);
            let ord = {
                let mut w =
                    PieceWriter::create(&rebuilt, &Db::segment_opts(&opts), 0, opts.inline_bytes)?;
                for (k, v) in &recs {
                    w.begin(k)?;
                    w.value(v);
                    w.end_with(false)?;
                }
                w.finish()?
            };
            write_ord(dir, &name, &ord)?;
            std::fs::rename(&rebuilt, dir.join(&name))?;
            File::open(dir)?.sync_all()?;
            live.push(name);
            manifest_write(dir, sealed, &live)?;
            let _ = std::fs::remove_file(&tmp);
        }
        // The same for ordered indexes, which outlive their segment by a
        // crash window at either end: written before a seal's rename, and
        // still there after a merge unlinks its inputs. A promotion renames
        // a segment but keeps the id and end-sequence its index is named by,
        // so the live set below still claims it.
        let live_ord: std::collections::HashSet<String> =
            live.iter().filter_map(|n| Db::ord_name_for(n)).collect();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("ord-") && !live_ord.contains(&name) {
                let _ = std::fs::remove_file(dir.join(name));
            }
        }
        // Where the store starts. `Random` starts advised and never moves;
        let starts_random = opts.read_advice.starts_random();
        let mut segs = Vec::with_capacity(live.len());
        for name in &live {
            segs.push(Seg::open(
                dir,
                name,
                starts_random,
                opts.read_advice != ReadAdvice::Normal,
                opts.segment.checksums,
            )?);
        }
        segs.sort_by(seg_order);
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
        let mem = MemTable::new();
        let readers = Readers::new();
        let mut mem_bytes = 0usize;
        let mut from = sealed;
        let mut valid_len = 0u64;
        for &id in &wal_ids {
            let (next, valid) = Wal::replay(&Db::wal_path(dir, id), id, from, |kind, k, v| {
                if kind == WAL_DEL {
                    mem.delete(mem_hash(k), k, &readers);
                    mem_bytes += k.len() + 16;
                } else {
                    mem.append(mem_hash(k), k, v, &readers);
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
        let segs: Vec<std::sync::Arc<Seg>> = segs.into_iter().map(std::sync::Arc::new).collect();
        let mean_key_bytes = Db::mean_key_bytes_of(&segs);
        let store_bytes = Db::store_bytes_of(dir, &segs);
        let l0_aligned = Db::l0_aligned_of(&segs);
        let max_key = Db::max_key_of(&segs, &mem);
        let ntables = segs.len();
        let state = State {
            segs,
            mem: std::sync::Arc::new(mem),
            frozen: None,
            gen: 1,
            mean_key_bytes,
            store_bytes,
            l0_aligned,
        };
        let shared = std::sync::Arc::new(Shared {
            state: AtomicPtr::new(Box::into_raw(Box::new(state))),
            readers,
            retired: std::sync::Mutex::new(Vec::new()),
            advice_random: std::sync::atomic::AtomicBool::new(starts_random),
        });
        let r = Reader {
            shared,
            slot: None,
            isolation: std::cell::Cell::new(Isolation::Dirty),
            held: AtomicPtr::new(std::ptr::null_mut()),
            wm: std::cell::Cell::new(SEE_ALL),
            opts,
            tables: std::cell::RefCell::new(Db::tables_for(ntables)),
            scan_keys: std::cell::RefCell::new(None),
            cache_used: std::cell::Cell::new(false),
            cache_bytes: std::cell::Cell::new(0),
            scan_tick: std::cell::Cell::new(0),
            shed_seed: std::cell::Cell::new(0x9E37_79B9_7F4A_7C15),
            pending: std::cell::RefCell::new(Vec::new()),
            built: std::cell::RefCell::new(Vec::new()),
            snap_gen: std::cell::Cell::new(0),
            snap_added: std::cell::RefCell::new(Vec::new()),
            log_seen: std::cell::Cell::new(0),
            log_gen: std::cell::Cell::new(0),
            snap_entries: std::cell::Cell::new(0),
            ahead: None,
        };
        Ok(Db {
            r,
            dir: dir.to_path_buf(),
            wal,
            wal_id,
            mem_bytes,
            direct: None,
            max_key,
            run_scratch: Vec::new(),
            retiring_tmps: Vec::new(),
            pending_err: None,
            next_seg,
            sealing: None,
            compacting: None,
            unsynced: 0,
            phase_ns: [0; 3],
            retiring_wals: retiring,
            spare_wals,
            covered_seq: sealed,
            seal_wait: SealWaits::default(),
            draining: false,
        })
    }

    /// Buffered until `commit`; visible to this handle's reads immediately,
    /// which is the read-your-writes contract `Store::read_all` set.
    pub fn append(&mut self, key: &[u8], value: &[u8]) {
        if self.mem().ordered || self.direct_can_open() {
            if self.goes_direct(key, value) {
                if !self.mem().ordered {
                    self.set_mem(std::sync::Arc::new(MemTable::new_ordered()));
                }
                self.max_key.clear();
                self.max_key.extend_from_slice(key);
                self.mem()
                    .append(mem_hash(key), key, value, &self.shared.readers);
                self.mem_bytes += key.len() + value.len();
                return;
            }
            if self.mem().ordered {
                self.leave_direct();
            }
        }
        let hash = self.mem().prefetch(key);
        if key > self.max_key.as_slice() {
            self.max_key.clear();
            self.max_key.extend_from_slice(key);
        }
        self.wal.append(key, value);
        self.mem().append(hash, key, value, &self.shared.readers);
        self.mem_bytes += key.len() + value.len();
    }

    /// Whether the next key could start a direct run: ordered ingest on,
    /// the memtable empty, nothing of this batch staged before it. A seal
    /// in flight is no bar -- the run's keys lie above the frozen table's,
    /// and the run's own close joins the seal first.
    fn direct_can_open(&self) -> bool {
        self.opts.direct_ingest && self.mem().is_empty() && self.wal.pending.is_empty()
    }

    /// Whether a write goes into the direct run: a key above the store's
    /// greatest -- not the empty key, which is below every other and is
    /// the shape a direct segment's marker takes -- with a value the
    /// record holds inline, by the writer's own encoder, since a run in a
    /// block is held until the segment closes and is not durable at a
    /// marker.
    fn goes_direct(&mut self, key: &[u8], value: &[u8]) -> bool {
        if key.is_empty() || key <= self.max_key.as_slice() {
            return false;
        }
        crate::index::encode_run(value, &[value.len() as u32], &mut self.run_scratch);
        inlines(self.opts.inline_bytes, self.run_scratch.len())
    }

    /// The batch has written something the run cannot take. The run's
    /// committed entries close as a segment; the batch's own -- appended
    /// since the last commit, so nothing on disk has them -- move to a
    /// fresh hashed memtable with the WAL frames they never had, in the
    /// order they came, and the write that ended the run follows them.
    /// `append` and `delete` cannot fail, so an error closing the run
    /// waits for the next `commit`.
    fn leave_direct(&mut self) {
        let committed = self.direct.as_ref().map_or(0, |d| d.committed);
        let tail: Vec<(Vec<u8>, Vec<u8>)> = (committed..self.mem().len())
            .map(|i| self.mem().entry(i))
            .map(|e| {
                (
                    self.mem().key_of(e).to_vec(),
                    self.mem().value_at(MemTable::head(e) as usize).to_vec(),
                )
            })
            .collect();
        self.mem().truncate_entries(committed);
        if let Err(e) = self.close_direct() {
            self.pending_err = Some(e);
        }
        for (k, v) in tail {
            self.wal.append(&k, &v);
            self.mem()
                .append(mem_hash(&k), &k, &v, &self.shared.readers);
            self.mem_bytes += k.len() + v.len();
        }
    }

    /// The greatest key any source holds: the partitions' and pieces' last
    /// keys and the memtable's, for a store that opens with all of them.
    fn max_key_of(segs: &[std::sync::Arc<Seg>], mem: &MemTable) -> Vec<u8> {
        let mut best: Vec<u8> = Vec::new();
        for s in segs {
            let b = &s.blob;
            if b.keys() > 0 {
                if let Some(k) = b.key_at(b.keys() - 1) {
                    if k > best.as_slice() {
                        best = k.to_vec();
                    }
                }
            }
        }
        for e in (0..mem.len()).map(|i| mem.entry(i)) {
            let k = mem.key_of(e);
            if k > best.as_slice() {
                best = k.to_vec();
            }
        }
        best
    }

    /// Replace a key's values with one new value: a delete and an append in
    /// the same batch, so a read after the commit sees the new value alone
    /// and a crash sees both or neither. This is the update YCSB means, and
    /// what `Store::put` and every single-value engine do; `append` is the
    /// other verb, and using it for an update piled every Zipfian rewrite
    /// onto its key until each read walked the pile.
    pub fn put(&mut self, key: &[u8], value: &[u8]) {
        if self.mem().ordered {
            self.leave_direct();
        }
        if key > self.max_key.as_slice() {
            self.max_key.clear();
            self.max_key.extend_from_slice(key);
        }
        let hash = self.mem().prefetch(key);
        self.wal.delete(key);
        self.wal.append(key, value);
        self.mem().put(hash, key, value, &self.shared.readers);
        self.mem_bytes += key.len() + 16 + key.len() + value.len();
    }

    /// End every value of `key` written before this point; later appends
    /// start fresh. Durable at the next `commit`, exactly like an append,
    /// and reclaimed by the next merge that reaches the key.
    pub fn delete(&mut self, key: &[u8]) {
        if self.mem().ordered {
            self.leave_direct();
        }
        let hash = self.mem().prefetch(key);
        self.wal.delete(key);
        self.mem().delete(hash, key, &self.shared.readers);
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

    /// The durability point: WAL append + fdatasync -- or, under ordered
    /// ingest, the batch streamed to the direct segment and that synced.
    /// If the memtable has crossed the seal threshold, seal after the
    /// commit -- after, so the batch's durability never waits on a segment
    /// write.
    pub fn commit(&mut self) -> Result<()> {
        if let Some(e) = self.pending_err.take() {
            return Err(e);
        }
        let t = std::time::Instant::now();
        if self.mem().ordered {
            self.commit_direct()?;
        } else {
            self.wal.mark_commit();
            self.wal.write()?;
            self.mem().commit();
            self.unsynced += 1;
            let due = match self.opts.sync {
                SyncPolicy::Always => true,
                SyncPolicy::EveryN(n) => self.unsynced >= n.max(1),
            };
            if due {
                self.wal.sync()?;
                self.unsynced = 0;
            }
        }
        self.phase_ns[0] += t.elapsed().as_nanos() as u64;
        if self.sealing.as_ref().is_some_and(|h| h.is_finished()) {
            self.join_seal()?;
        }
        // A direct run closes when a seal would: it joins whole, as a
        // seal's piece does by promotion, so the two paths leave one shape.
        if self.mem_bytes >= self.seal_threshold() {
            self.seal()?;
        }
        Ok(())
    }

    /// What is staged made durable where it is going: the batch's records
    /// to the direct segment under an ordered memtable, the WAL's pending
    /// frames otherwise. What `sync` and `flush` do before anything else.
    fn commit_staged(&mut self) -> Result<()> {
        if let Some(e) = self.pending_err.take() {
            return Err(e);
        }
        if self.mem().ordered {
            self.commit_direct()
        } else {
            self.wal.commit()?;
            self.mem().commit();
            Ok(())
        }
    }

    /// The batch -- the ordered memtable's entries since the last commit
    /// -- written to the direct segment, opened here on the first: each
    /// key and its one value, then a commit marker, then the sync the
    /// policy asks for. The segment is their log; the WAL never sees them.
    fn commit_direct(&mut self) -> Result<()> {
        if self.direct.as_ref().map_or(0, |d| d.committed) == self.mem().len() {
            return Ok(());
        }
        if self.direct.is_none() {
            let id = self.next_seg;
            self.next_seg += 1;
            let tmp = self.dir.join(format!("direct-{id:08}.tmp"));
            let _ = std::fs::remove_file(&tmp);
            let opts = Db::segment_opts(&self.opts);
            let mut w = PieceWriter::create(&tmp, &opts, 0, self.opts.inline_bytes)?;
            w.set_marks(true);
            self.direct = Some(Direct {
                w,
                tmp,
                id,
                committed: 0,
            });
        }
        let mem = self.r.mem().clone();
        let d = self.direct.as_mut().expect("opened above");
        for i in d.committed..mem.len() {
            let e = mem.entry(i);
            debug_assert_eq!(
                e.count.load(AtomicOrdering::Relaxed),
                1,
                "an ordered entry has the one value it was appended with"
            );
            d.w.begin(mem.key_of(e))?;
            d.w.value(mem.value_at(MemTable::head(e) as usize));
            d.w.end_with(false)?;
        }
        d.w.mark()?;
        d.committed = mem.len();
        mem.commit();
        self.unsynced += 1;
        let due = match self.r.opts.sync {
            SyncPolicy::Always => true,
            SyncPolicy::EveryN(n) => self.unsynced >= n.max(1),
        };
        if due {
            d.w.sync()?;
            self.unsynced = 0;
        }
        Ok(())
    }

    /// The direct run closed: its segment finished, indexed and linked
    /// under a piece's name on the seal thread, with the ordered memtable
    /// frozen for reads until the join publishes it, as a seal's is. It
    /// joins as a piece over the last range -- above every partition's
    /// last key by construction -- for promotion to take by rename when
    /// the range is due, as it takes a seal's piece: the seal's rule is
    /// the only one. The temp name stays linked until the manifest names
    /// the segment, since a name the manifest lacks is swept at open and
    /// the temp name is what recovery reads. Nothing rotates: the run has
    /// no WAL frames, and its name covers the sequence so far so that the
    /// frames of what follows replay.
    fn close_direct(&mut self) -> Result<()> {
        *self.scan_keys.borrow_mut() = None;
        self.snap_added.borrow_mut().clear();
        self.drop_blocks();
        self.mem_bytes = 0;
        let Some(d) = self.direct.take() else {
            // A run forming with nothing committed: no file, nothing to close.
            self.set_mem(std::sync::Arc::new(MemTable::new()));
            return Ok(());
        };
        self.join_seal()?;
        debug_assert_eq!(
            self.mem().len(),
            d.committed,
            "a run closes between batches"
        );
        self.freeze();
        let end_seq = self.wal.seq;
        let np = self.segs().partition_point(|s| s.level > 0);
        let name = if np == 0 {
            Db::seg_name(d.id, end_seq)
        } else {
            format!(
                "pcs-{:08}-{:016}-{}-.sup",
                d.id,
                end_seq,
                hex(&self.segs()[np - 1].lo)
            )
        };
        let dir = self.dir.clone();
        let tmp = d.tmp;
        let w = d.w;
        self.retiring_tmps.push(tmp.clone());
        self.sealing = Some(std::thread::spawn(move || {
            let ord = w
                .finish()
                .map_err(|e| err(&format!("direct finish: {e}")))?;
            write_ord(&dir, &name, &ord)?;
            std::fs::hard_link(&tmp, dir.join(&name))?;
            File::open(&dir)?.sync_all()?;
            Ok(vec![name])
        }));
        Ok(())
    }

    /// Freeze the memtable, rotate the WAL, and hand the frozen table to a
    /// thread that writes it as one immutable segment in today's store
    /// format -- fsync, rename into place (the name carrying the covered
    /// end-sequence), fsync the directory, delete the rotated-out WAL.
    /// Commits continue into the new WAL while it runs; at most one seal is
    /// in flight, so a second trigger joins the first (backpressure).
    pub fn seal(&mut self) -> Result<()> {
        if let Some(e) = self.pending_err.take() {
            return Err(e);
        }
        if self.mem().ordered {
            // The ordered memtable is the direct segment: committing what
            // is staged and closing it is the seal.
            self.commit_direct()?;
            return self.close_direct();
        }
        self.wal.commit()?;
        self.mem().commit();
        self.unsynced = 0;
        if self.mem().is_empty() {
            return Ok(());
        }
        self.join_seal()?;
        let frozen = self.freeze();
        // The scan snapshot names the live memtable's slots, and the live
        // memtable is new: a write's bookkeeping renumbers the snapshot at
        // every rehash, and the fresh table's first rehash has a thousand
        // slots where the snapshot names hundreds of thousands. It stood
        // stale until the next scan rebuilt it, and six hundred inserts
        // between a seal and that scan were enough to index past the map.
        *self.scan_keys.borrow_mut() = None;
        self.snap_added.borrow_mut().clear();
        self.drop_blocks();
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
            .segs()
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
        self.sealing = Some(std::thread::spawn(move || {
            if background_io == BackgroundIo::Idle {
                idle_io_priority();
            }
            // In KEY order, not hash order. A segment written in the
            // memtable's iteration order scatters each key's values across
            // blocks by hash, so an ordered scan walks the file randomly;
            // written sorted, a scan walks it forwards. This is what the
            // retired line-order arm of the day-size measurement found, in
            // the new engine -- how the roll writes decides what the read
            // costs -- and the sort is affordable because a seal is off the
            // commit path. The same sort is what makes splitting at the
            // fences a matter of slicing.
            let mut order: Vec<&MemEntry> = (0..mem.len()).map(|i| mem.entry(i)).collect();
            order.sort_unstable_by_key(|e| mem.key_of(e));

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
                    let k = mem.key_of(order[at]);
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
                let ord;
                {
                    let mut w = PieceWriter::create(&tmp, &opts, sync_every, inline_max)
                        .map_err(|e| err(&format!("seal create: {e}")))?;
                    for e in &order[start..at] {
                        let key = mem.key_of(e);
                        // Only what is live after the newest tombstone, and
                        // the flag if there was one: the segment carries the
                        // delete forward for the sources older than it.
                        let (offs, tomb) = mem.live_chain(e, SEE_ALL);
                        w.begin(key)?;
                        for off in offs {
                            w.value(mem.value_at(off));
                        }
                        w.end_with(tomb)?;
                    }
                    ord = w.finish().map_err(|e| err(&format!("seal finish: {e}")))?;
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
                // Before the segment's own rename, so the segment never
                // exists without it.
                write_ord(&dir, &name, &ord)?;
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
        self.join_compact()?;
        self.join_ahead();
        Ok(())
    }

    /// Make everything written durable and seal nothing: the WAL's pending
    /// frames written and fsynced, the memtable left where it is. What a
    /// caller wants when it has stopped writing for now and will read the
    /// tail out of memory; `flush` is the other answer, and the difference
    /// was priced at 11% of a canonical load window.
    pub fn sync(&mut self) -> Result<()> {
        self.commit_staged()?;
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
        self.commit_staged()?;
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
        while self.segs().iter().any(|s| s.level == 0) {
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
        let mut segs = self.segs().to_vec();
        for name in &names {
            self.covered_seq = self.covered_seq.max(Db::name_end_seq(name).unwrap_or(0));
            segs.push(std::sync::Arc::new(Seg::open(
                &self.dir,
                name,
                self.advice_random(),
                self.opts.read_advice != ReadAdvice::Normal,
                self.opts.segment.checksums,
            )?));
        }
        self.publish_segs(segs);
        self.set_frozen(None);
        if self.opts.scan_block_cache {
            self.build_ctx().rank_pieces()?;
        }
        let tp = std::time::Instant::now();
        self.publish()?;
        self.seal_wait.publish_ns += tp.elapsed().as_nanos() as u64;
        self.build_ahead();
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
        for tmp in std::mem::take(&mut self.retiring_tmps) {
            let _ = std::fs::remove_file(tmp);
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
    /// The segment set `segs`, sorted, its derived quantities refreshed,
    /// published as the state: the writer's one way to change the
    /// segments. Every block table is dropped with it.
    fn publish_segs(&mut self, mut segs: Vec<std::sync::Arc<Seg>>) {
        self.drop_blocks();
        segs.sort_by(|a, b| seg_order(a, b));
        *self.tables.borrow_mut() = Db::tables_for(segs.len());
        let mean_key_bytes = Db::mean_key_bytes_of(&segs);
        let store_bytes = Db::store_bytes_of(&self.dir, &segs);
        let l0_aligned = Db::l0_aligned_of(&segs);
        let cur = self.state();
        let next = State {
            segs,
            mem: cur.mem.clone(),
            frozen: cur.frozen.clone(),
            gen: cur.gen + 1,
            mean_key_bytes,
            store_bytes,
            l0_aligned,
        };
        self.publish_state(next);
    }

    /// `next` becomes the state; the one before it is retired at the
    /// epoch this bumps and freed once no reader is pinned before it.
    fn publish_state(&mut self, next: State) {
        let p = Box::into_raw(Box::new(next));
        let old = self.shared.state.swap(p, AtomicOrdering::AcqRel);
        let tag = self.shared.readers.bump();
        let mut retired = self.shared.retired.lock().expect("the retired list");
        // SAFETY: published by this writer, owned by it until freed.
        retired.push((tag, unsafe { Box::from_raw(old) }));
        let readers = &self.shared.readers;
        retired.retain(|(t, _)| !readers.none_before(*t));
    }

    /// PROTOTYPE: how many states a publish replaced are still held for a
    /// reader, for a test to hold the reader table to its word.
    pub fn retired_states(&self) -> usize {
        self.shared.retired.lock().expect("the retired list").len()
    }

    /// A handle that reads this store from any thread, under `Latest`
    /// isolation to begin with, with caches of its own and a slot in the
    /// reader table for its life. It is `Send` and not `Sync`: one
    /// thread reads through it at a time, and a thread that wants its
    /// own asks for its own. Fails when every slot is taken.
    pub fn reader(&self) -> Result<Reader> {
        let slot = self
            .shared
            .readers
            .claim()
            .ok_or_else(|| err("reader table: every slot is taken"))?;
        Ok(Reader {
            shared: self.shared.clone(),
            slot: Some(slot),
            isolation: std::cell::Cell::new(Isolation::Latest),
            held: AtomicPtr::new(std::ptr::null_mut()),
            wm: std::cell::Cell::new(SEE_ALL),
            opts: self.opts.clone(),
            tables: std::cell::RefCell::new(Vec::new()),
            scan_keys: std::cell::RefCell::new(None),
            cache_used: std::cell::Cell::new(false),
            cache_bytes: std::cell::Cell::new(0),
            scan_tick: std::cell::Cell::new(0),
            shed_seed: std::cell::Cell::new(0x9E37_79B9_7F4A_7C15),
            pending: std::cell::RefCell::new(Vec::new()),
            built: std::cell::RefCell::new(Vec::new()),
            snap_gen: std::cell::Cell::new(0),
            snap_added: std::cell::RefCell::new(Vec::new()),
            log_seen: std::cell::Cell::new(0),
            log_gen: std::cell::Cell::new(0),
            snap_entries: std::cell::Cell::new(0),
            ahead: None,
        })
    }

    /// The state with `mem` as the live memtable.
    fn set_mem(&mut self, mem: std::sync::Arc<MemTable>) {
        let cur = self.state();
        let next = State {
            segs: cur.segs.clone(),
            mem,
            frozen: cur.frozen.clone(),
            gen: cur.gen + 1,
            mean_key_bytes: cur.mean_key_bytes,
            store_bytes: cur.store_bytes,
            l0_aligned: cur.l0_aligned,
        };
        self.publish_state(next);
    }

    /// The state with `frozen` as the frozen memtable.
    fn set_frozen(&mut self, frozen: Option<std::sync::Arc<MemTable>>) {
        let cur = self.state();
        let next = State {
            segs: cur.segs.clone(),
            mem: cur.mem.clone(),
            frozen,
            gen: cur.gen + 1,
            mean_key_bytes: cur.mean_key_bytes,
            store_bytes: cur.store_bytes,
            l0_aligned: cur.l0_aligned,
        };
        self.publish_state(next);
    }

    /// The live memtable frozen and a fresh one live, in one publish;
    /// the frozen one returned for the seal.
    fn freeze(&mut self) -> std::sync::Arc<MemTable> {
        let cur = self.state();
        let frozen = cur.mem.clone();
        let next = State {
            segs: cur.segs.clone(),
            mem: std::sync::Arc::new(MemTable::new()),
            frozen: Some(frozen.clone()),
            gen: cur.gen + 1,
            mean_key_bytes: cur.mean_key_bytes,
            store_bytes: cur.store_bytes,
            l0_aligned: cur.l0_aligned,
        };
        self.publish_state(next);
        frozen
    }

    /// Whether every level-0 piece's fence is some partition's, over a
    /// segment list `sort_segs` has ordered. Nothing to align to is not
    /// aligned: before the first partitioning every piece spans the whole
    /// key space.
    fn l0_aligned_of(segs: &[std::sync::Arc<Seg>]) -> bool {
        let np = segs.partition_point(|s| s.level > 0);
        let (parts, l0) = segs.split_at(np);
        !parts.is_empty()
            && l0
                .iter()
                .all(|s| parts.iter().any(|p| p.lo == s.lo && p.hi == s.hi))
    }

    /// The partitions' bytes on disk. A free function over the segments,
    /// like `mean_key_bytes_of`, because `open` needs it before there is a
    /// `Db` to ask.
    fn store_bytes_of(dir: &Path, segs: &[std::sync::Arc<Seg>]) -> u64 {
        segs.iter()
            .filter(|s| s.level > 0)
            .filter_map(|s| std::fs::metadata(dir.join(&s.name)).ok())
            .map(|m| m.len())
            .sum()
    }

    /// What a key costs on disk, averaged over the live segments.
    ///
    /// Every mutation of `segs` ends in `sort_segs`, so refreshing here is
    /// what keeps this from going stale. It is the whole index section per
    /// key -- records, directory and hash -- where a scan walks only the
    /// records, so it reads high by about a quarter on the shape it was
    /// measured against. That is inside the tolerance: what it feeds is a
    /// comparison against a crossing that is flat for a factor of three
    /// either side.
    /// Free function over the segments, because `open` needs the number
    /// before there is a `Db` to ask. It sorts its segments itself rather
    /// than through `sort_segs`, so an opened store had a zero here and took
    /// the short-scan path for every scan however long -- which the create
    /// path's test could not see.
    fn mean_key_bytes_of(segs: &[std::sync::Arc<Seg>]) -> usize {
        let (bytes, keys) = segs.iter().fold((0usize, 0usize), |(b, k), s| {
            (b + s.blob.index_bytes(), k + s.blob.keys())
        });
        if keys == 0 {
            0
        } else {
            bytes / keys
        }
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
                .segs()
                .iter()
                .position(|s| s.level > 0 && s.lo == f.0 && s.hi == f.1);
            let mut pieces: Vec<usize> = self
                .segs()
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
                let b = &self.segs()[pi].blob;
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
    ///
    /// One piece qualifies too, and refusing it was expensive. A flush that
    /// leaves a single full-range piece -- every store up to about one seal,
    /// which is every rung of the suite's `quick` ladder below 300k keys --
    /// fell through to a merge that read that piece back and wrote it out
    /// again as one partition covering the same range. Measured at 100k keys:
    /// a quarter of the load window, and 4.06 device bytes a stored byte
    /// against 2.63 without it, where 300k keys -- two pieces, so promotion
    /// already fired -- spent nothing and wrote 2.69.
    ///
    /// Two conditions keep the promoted store the shape the merge would have
    /// left. The piece must fit a partition, or a merge would have cut it
    /// into several and promotion would not be the same store; and it must
    /// carry no tombstone, because a merge writes the bottom level and drops
    /// them, and a promotion keeps the file exactly as it is.
    fn promote_unpartitioned(&mut self) -> Result<bool> {
        let mut pieces: Vec<usize> = self
            .segs()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.level == 0)
            .map(|(i, _)| i)
            .collect();
        if pieces.is_empty() {
            return Ok(false);
        }
        if pieces.len() == 1 {
            let s = &self.segs()[pieces[0]];
            if s.tombs {
                return Ok(false);
            }
            let pb = self
                .opts
                .partition_bytes
                .unwrap_or(self.opts.seal_bytes)
                .max(1) as u64;
            match std::fs::metadata(self.dir.join(&s.name)) {
                Ok(m) if m.len() <= pb => {}
                _ => return Ok(false),
            }
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

    /// Give each segment its new fence and level by hard link, publish,
    /// then unlink the old names.
    fn apply_promotion(&mut self, renames: Vec<(usize, Fence)>) -> Result<()> {
        let mut old_names = Vec::with_capacity(renames.len());
        let mut segs = self.segs().to_vec();
        for (si, (lo, hi)) in renames {
            let old = segs[si].name.clone();
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
            // A segment is immutable once open, so the promoted one is
            // opened again under its new name: the partition's fences and
            // level come from the name.
            segs[si] = std::sync::Arc::new(Seg::open(
                &self.dir,
                &new,
                self.advice_random(),
                self.opts.read_advice != ReadAdvice::Normal,
                self.opts.segment.checksums,
            )?);
            old_names.push(old);
        }
        File::open(&self.dir)?.sync_all()?;
        self.publish_segs(segs);
        self.publish()?;
        for old in old_names {
            self.retire_seg(&old);
        }
        self.build_ahead();
        Ok(())
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
    /// first is O(store), which is what the merge measurements convicted.
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
                .segs()
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
            .segs()
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
        let kept: Vec<std::sync::Arc<Seg>> = self
            .segs()
            .iter()
            .filter(|seg| !inputs.contains(&seg.name))
            .cloned()
            .collect();
        let mut merged = Vec::with_capacity(outputs.len() + kept.len());
        for name in &outputs {
            merged.push(std::sync::Arc::new(Seg::open(
                &self.dir,
                name,
                self.advice_random(),
                self.opts.read_advice != ReadAdvice::Normal,
                self.opts.segment.checksums,
            )?));
        }
        // Partitions first (older, disjoint), then whatever L0 arrived
        // while the merge ran, oldest to newest.
        merged.extend(kept);
        self.publish_segs(merged);
        if self.opts.scan_block_cache {
            self.build_ctx().rank_pieces()?;
        }
        self.publish()?;
        self.build_ahead();
        for name in &inputs {
            self.retire_seg(name);
        }
        self.phase_ns[2] += t.elapsed().as_nanos() as u64;
        Ok(())
    }

    /// One empty table cell per segment.
    fn tables_for(n: usize) -> Vec<std::cell::RefCell<Option<BlockTable>>> {
        (0..n).map(|_| std::cell::RefCell::new(None)).collect()
    }

    /// PROTOTYPE: a built block takes its place in the list the sampler
    /// draws from, and the count grows by what it holds.
    /// PROTOTYPE: start the builder ahead of the reader over the segment
    /// set just published, stopping one still running for an older set.
    /// Nothing to build without a piece: every block of a partition no
    /// piece overlays is clean, and a form for it is nothing.
    fn build_ahead(&mut self) {
        if !(self.opts.scan_block_cache && self.opts.scan_cache_ahead) {
            return;
        }
        self.stop_ahead();
        if !self.segs().iter().any(|s| s.level == 0) {
            return;
        }
        let names: Vec<String> = self.segs().iter().map(|s| s.name.clone()).collect();
        let gen = self.state().gen;
        let dir = self.dir.clone();
        let random = self.advice_random();
        let advise_ord = self.opts.read_advice != ReadAdvice::Normal;
        let verify = self.opts.segment.checksums;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let _ = build_ahead_job(&dir, &names, gen, random, advise_ord, verify, &flag, &tx);
        });
        self.ahead = Some(Ahead {
            handle: Some(handle),
            rx,
            stop,
        });
    }

    /// PROTOTYPE: the builder told to stop and joined, its forms dropped.
    fn stop_ahead(&mut self) {
        if let Some(mut a) = self.ahead.take() {
            a.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// PROTOTYPE: the builder waited for, its forms kept for the next
    /// scan to install. What `settle` does, for a test or an experiment
    /// that wants the cache as the builder leaves it.
    fn join_ahead(&mut self) {
        if let Some(a) = self.ahead.as_mut() {
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }

    pub fn phase_ns(&self) -> (u64, u64, u64) {
        (self.phase_ns[0], self.phase_ns[1], self.phase_ns[2])
    }

    /// The seal phase decomposed; see `SealWaits`.
    pub fn seal_waits(&self) -> SealWaits {
        self.seal_wait
    }

    pub fn segments(&self) -> usize {
        self.segs().len() + usize::from(self.sealing.is_some())
    }

    /// Whether a seal and a merge are running right now. A crash experiment
    /// records the state it died in with these.
    pub fn in_flight(&self) -> (bool, bool) {
        (self.sealing.is_some(), self.compacting.is_some())
    }

    /// The live WAL: its path, the bytes of it behind a barrier, and the
    /// bytes written to it. Everything between the two is what a power loss
    /// may take; a crash experiment takes a random amount of it, because a process
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

/// PROTOTYPE: per level-0 piece meeting a partition's range, its index in
/// the level and the first rank not below each block's lower bound, as
/// `BlockTable::pieces` holds them.
type PieceBounds = Vec<(usize, Vec<u32>)>;

/// PROTOTYPE: a builder ahead of the reader in flight: its thread while
/// it runs, the forms it sends, and the flag that stops it.
struct Ahead {
    handle: Option<std::thread::JoinHandle<()>>,
    rx: std::sync::mpsc::Receiver<Built>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// PROTOTYPE: one form built ahead: the segment set's generation it was
/// built at, the partition by name, the block, and the form.
struct Built {
    gen: u64,
    name: String,
    b: u32,
    form: Cached,
}

/// PROTOTYPE: the builder ahead of the reader: readers of its own over
/// the published segments, every block the pieces overlay built from the
/// partitions and the pieces with an empty memtable, each form sent back
/// as it is built. Stops when told, or when the store has dropped the
/// channel's other end. A block with no overlay is clean and needs no
/// form; one past the wide bound is the store's to build.
#[allow(clippy::too_many_arguments)]
fn build_ahead_job(
    dir: &Path,
    names: &[String],
    gen: u64,
    random: bool,
    advise_ord: bool,
    verify: bool,
    stop: &std::sync::atomic::AtomicBool,
    tx: &std::sync::mpsc::Sender<Built>,
) -> Result<()> {
    let mut segs: Vec<std::sync::Arc<Seg>> = Vec::with_capacity(names.len());
    for n in names {
        segs.push(std::sync::Arc::new(Seg::open(
            dir, n, random, advise_ord, verify,
        )?));
    }
    segs.sort_by(|a, b| seg_order(a, b));
    let mem = MemTable::new();
    let unsealed = Snapshot::default();
    let ctx = BuildCtx {
        wm: SEE_ALL,
        segs: &segs,
        mem: &mem,
        frozen: None,
        tombs: segs.iter().any(|s| s.tombs),
    };
    ctx.rank_pieces()?;
    let np = segs.partition_point(|s| s.level > 0);
    let l0 = &segs[np..];
    for seg in &segs[..np] {
        let nblocks = seg.blob.keys().div_ceil(CACHE_BLOCK);
        let (pieces, snap_at) = ctx.table_bounds(seg, l0, &unsealed)?;
        if pieces.is_empty() {
            continue;
        }
        let table = BlockTable {
            slots: (0..nblocks).map(|_| None).collect(),
            touched: vec![0; nblocks],
            listed: vec![u32::MAX; nblocks],
            pieces,
            snap_at,
            snap_gen: 0,
            added: (0..nblocks).map(|_| Vec::new()).collect(),
            filed: 0,
        };
        let src = Sources { seg, l0 };
        for b in 0..nblocks {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(());
            }
            let n = BuildCtx::overlay_count(&table, b);
            if n == 0 || n > WIDE {
                continue;
            }
            let form = ctx.materialize(src, &table, b, &unsealed)?;
            // Copies only: a sparse form costs about as much to build at
            // the scan as the memtable's keys cost to splice into one
            // built here, measured at thirty million with a million and a
            // half unsealed keys at E's start, and a clean block is
            // nothing. A copy is the build that pays back.
            if !matches!(form, Cached::Block(_)) {
                continue;
            }
            let built = Built {
                gen,
                name: seg.name.clone(),
                b: b as u32,
                form,
            };
            if tx.send(built).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// PROTOTYPE: what building a cached block reads, and nothing the cache
/// keeps: the segments, the live and the frozen memtable, and whether any
/// source holds a tombstone. `Db` makes one over its own state for every
/// build and every settle; a builder ahead of the reader makes one over
/// readers of its own on the published segments and an empty memtable,
/// since the files are immutable and stay readable while mapped, and the
/// store splices the memtable's keys in when the block is installed.
struct BuildCtx<'s> {
    /// The watermark the live memtable's chains are walked under.
    wm: u64,
    segs: &'s [std::sync::Arc<Seg>],
    mem: &'s MemTable,
    frozen: Option<&'s MemTable>,
    tombs: bool,
}

impl<'s> BuildCtx<'s> {
    /// PROTOTYPE: where a partition's block boundaries fall in every
    /// level-0 piece meeting its range, and in the snapshot of unsealed
    /// keys: the two source maps a table starts from.
    fn table_bounds(
        &self,
        seg: &Seg,
        l0: &[std::sync::Arc<Seg>],
        unsealed: &Snapshot,
    ) -> Result<(PieceBounds, Vec<u32>)> {
        let nblocks = seg.blob.keys().div_ceil(CACHE_BLOCK);
        let mut pieces = Vec::new();
        for (j, p) in l0.iter().enumerate() {
            if !seg.lo.is_empty()
                && p.hi
                    .as_ref()
                    .is_some_and(|h| h.as_slice() <= seg.lo.as_slice())
            {
                continue;
            }
            if seg
                .hi
                .as_ref()
                .is_some_and(|h| !p.lo.is_empty() && p.lo.as_slice() >= h.as_slice())
            {
                continue;
            }
            let at = block_bounds_of(
                seg,
                nblocks,
                p.blob.keys(),
                |k| p.ord.seek(p.cursor_from(k), |i| p.blob.key_at(i)),
                |i, bound| p.blob.key_at(i).is_some_and(|k| k < bound),
            )?;
            pieces.push((j, at));
        }
        let snap_at = Self::snap_bounds(seg, nblocks, unsealed)?;
        Ok((pieces, snap_at))
    }

    /// PROTOTYPE: the block whose key range holds `key`.
    fn owner_of(seg: &Seg, key: &[u8]) -> (usize, u32) {
        let keys = seg.blob.keys();
        let rank = seg.ord.seek(key, |r| seg.blob.key_at(r));
        let same = rank < keys && seg.blob.key_at(rank) == Some(key);
        let owner = if same { rank } else { rank.saturating_sub(1) };
        (owner / CACHE_BLOCK, ((rank as u32) << 1) | same as u32)
    }
    /// PROTOTYPE: a cut carried with a key, as `cut_at` would answer it for
    /// a walk standing at `rank` and ending at `end`.
    fn cut_known(cut: u32, rank: usize, end: usize) -> (usize, Ordering) {
        let c = (cut >> 1) as usize;
        debug_assert!(c >= rank, "an overlay key's cut is behind the walk");
        let c = c.max(rank);
        if c >= end {
            (end, Ordering::Greater)
        } else if cut & 1 == 1 {
            (c, Ordering::Equal)
        } else {
            (c, Ordering::Greater)
        }
    }
    /// PROTOTYPE: each key of `piece` ranked in `part`, the partition it is
    /// aligned to, from one forward walk: a gallop from the last rank, then
    /// a binary search inside the gallop's span, so a run of keys the
    /// partition also holds costs two reads a key.
    fn ranks_over(part: &Seg, piece: &Seg) -> Result<Vec<u32>> {
        let n = piece.blob.keys();
        let pk = part.blob.keys();
        let mut out = Vec::with_capacity(n);
        let mut r = 0usize;
        for i in 0..n {
            let key = piece
                .blob
                .key_at(i)
                .ok_or_else(|| err("block cache: a rank did not resolve"))?;
            let below = |j: usize| part.blob.key_at(j).is_some_and(|k| k < key);
            if r < pk && below(r) {
                let mut lo = r;
                let mut step = 1usize;
                let hi = loop {
                    let probe = lo + step;
                    if probe >= pk {
                        break pk;
                    }
                    if below(probe) {
                        lo = probe;
                        step *= 2;
                    } else {
                        break probe;
                    }
                };
                let (mut a, mut b) = (lo + 1, hi);
                while a < b {
                    let m = a + (b - a) / 2;
                    if below(m) {
                        a = m + 1;
                    } else {
                        b = m;
                    }
                }
                r = a;
            }
            let same = r < pk && part.blob.key_at(r) == Some(key);
            out.push(((r as u32) << 1) | same as u32);
        }
        Ok(out)
    }
    /// PROTOTYPE: rank the keys of every piece aligned to a partition
    /// that has no ranks against that partition yet. Called when a seal
    /// or a merge publishes, so the work is off the read path, and by a
    /// table's making for pieces that were opened from disk.
    fn rank_pieces(&self) -> Result<()> {
        let np = self.segs.partition_point(|s| s.level > 0);
        let (parts, l0) = self.segs.split_at(np);
        for p in l0 {
            let Some(part) = parts.iter().find(|q| q.lo == p.lo && q.hi == p.hi) else {
                continue;
            };
            let id = part.blob.id();
            if p.ranks
                .read()
                .expect("a piece's ranks")
                .as_ref()
                .is_some_and(|(against, _)| *against == id)
            {
                continue;
            }
            let ranks = BuildCtx::ranks_over(part, p)?;
            *p.ranks.write().expect("a piece's ranks") = Some((id, ranks));
        }
        Ok(())
    }
    /// PROTOTYPE: where the snapshot's keys fall against the partition's
    /// block boundaries.
    fn snap_bounds(seg: &Seg, nblocks: usize, unsealed: &Snapshot) -> Result<Vec<u32>> {
        block_bounds_of(
            seg,
            nblocks,
            unsealed.len(),
            |k| unsealed.seek(k),
            |i, bound| unsealed.get(i).is_some_and(|(k, _)| k < bound),
        )
    }
    /// PROTOTYPE: the memtables' keys of a block, in order: a run of the
    /// snapshot and the filed keys, merged. The filed keys are put in key
    /// order here unless `sorted` says they are, with their keys resolved
    /// once; sorting the slots through a key read per compare cost a
    /// dense block's build measurably.
    fn overlay_mem<'a>(
        &'a self,
        unsealed: &'a Snapshot,
        snap: std::ops::Range<usize>,
        filed: &[(u32, u32)],
        sorted: bool,
    ) -> Result<Vec<Over<'a>>> {
        let mut out: Vec<Over> = Vec::with_capacity(snap.len() + filed.len());
        for i in snap {
            let (k, sk) = unsealed
                .get(i)
                .ok_or_else(|| err("block cache: a snapshot bound did not resolve"))?;
            out.push(Over {
                key: k,
                sk: Some(*sk),
                cut: u32::MAX,
                pieces: 0..0,
            });
        }
        if filed.is_empty() {
            return Ok(out);
        }
        let key_of = |slot: u32| self.mem.key_of(self.mem.entry(slot as usize));
        let mut fresh: Vec<Over> = filed
            .iter()
            .map(|&(i, cut)| Over {
                key: key_of(i),
                sk: Some(SnapKey {
                    off: 0,
                    len: 0,
                    mem: i,
                    frozen: u32::MAX,
                }),
                cut,
                pieces: 0..0,
            })
            .collect();
        if !sorted {
            fresh.sort_by(|x, y| x.key.cmp(y.key));
        }
        if out.is_empty() {
            return Ok(fresh);
        }
        // A key in both is one key: the snapshot's entry names its frozen
        // slot, the side list's the live slot created since, and the live
        // one's tombstone must cut the frozen values. Merged as two entries
        // they came out as two keys, the tombstone cutting nothing, and the
        // test that scans between a seal and a live delete found it.
        let mut merged = Vec::with_capacity(out.len() + fresh.len());
        let (mut out, mut fresh) = (out.into_iter().peekable(), fresh.into_iter().peekable());
        loop {
            match (out.peek(), fresh.peek()) {
                (None, None) => break,
                (Some(_), None) => merged.push(out.next().unwrap()),
                (None, Some(_)) => merged.push(fresh.next().unwrap()),
                (Some(x), Some(y)) => match x.key.cmp(y.key) {
                    Ordering::Less => merged.push(out.next().unwrap()),
                    Ordering::Greater => merged.push(fresh.next().unwrap()),
                    Ordering::Equal => {
                        let (x, y) = (out.next().unwrap(), fresh.next().unwrap());
                        let (xs, ys) = (x.sk.expect("snapshot key"), y.sk.expect("side-list key"));
                        debug_assert_eq!(xs.mem, u32::MAX, "a live key was created twice");
                        merged.push(Over {
                            key: x.key,
                            sk: Some(SnapKey { mem: ys.mem, ..xs }),
                            cut: y.cut,
                            pieces: 0..0,
                        });
                    }
                },
            }
        }
        Ok(merged)
    }
    /// PROTOTYPE: the filed entries of a block in key order.
    fn sorted_filed(&self, filed: &[(u32, u32)]) -> Vec<(u32, u32)> {
        let key_of = |slot: u32| self.mem.key_of(self.mem.entry(slot as usize));
        let mut v = filed.to_vec();
        v.sort_by(|x, y| key_of(x.0).cmp(key_of(y.0)));
        v
    }
    /// PROTOTYPE: the memtables' keys and the pieces' over given runs,
    /// folded by key with the pieces holding it oldest first.
    fn overlay_runs<'a>(
        &'a self,
        src: Sources<'a>,
        mem: Vec<Over<'a>>,
        pieces: &[(usize, std::ops::Range<usize>)],
    ) -> Result<Overlay<'a>> {
        let mut held: Vec<(&[u8], usize, usize, u32)> = Vec::new();
        let against = src.seg.blob.id();
        for (j, run) in pieces {
            let p = &src.l0[*j];
            // Ranks against another partition are no ranks: the cut is
            // searched for instead.
            let against_it = p.ranks.read().expect("a piece's ranks");
            let ranks = against_it
                .as_ref()
                .filter(|(id, _)| *id == against)
                .map(|(_, v)| v);
            for r in run.clone() {
                let k = p
                    .blob
                    .key_at(r)
                    .ok_or_else(|| err("block cache: a rank did not resolve"))?;
                let cut = ranks.map_or(u32::MAX, |v| v[r]);
                held.push((k, *j, r, cut));
            }
        }
        if held.is_empty() {
            return Ok(Overlay { over: mem, held });
        }
        held.sort_by(|x, y| x.0.cmp(y.0).then(x.1.cmp(&y.1)));
        let mut over: Vec<Over> = Vec::with_capacity(mem.len() + held.len());
        let mut mem = mem.into_iter().peekable();
        let mut h = 0usize;
        while mem.peek().is_some() || h < held.len() {
            let take_mem = h >= held.len() || mem.peek().is_some_and(|m| m.key < held[h].0);
            if take_mem {
                over.push(mem.next().unwrap());
                continue;
            }
            let k = held[h].0;
            let from = h;
            let mut cut = u32::MAX;
            while h < held.len() && held[h].0 == k {
                if cut == u32::MAX {
                    cut = held[h].3;
                }
                h += 1;
            }
            let sk = if mem.peek().is_some_and(|m| m.key == k) {
                let m = mem.next().unwrap();
                if cut == u32::MAX {
                    cut = m.cut;
                }
                m.sk
            } else {
                None
            };
            over.push(Over {
                key: k,
                sk,
                cut,
                pieces: from as u32..h as u32,
            });
        }
        Ok(Overlay { over, held })
    }
    /// PROTOTYPE: every key above the partition in block `b`, from the
    /// table's bounds; nothing is seeked.
    fn overlay_all<'a>(
        &'a self,
        src: Sources<'a>,
        table: &BlockTable,
        b: usize,
        unsealed: &'a Snapshot,
    ) -> Result<Overlay<'a>> {
        let snap = table.snap_at[b] as usize..table.snap_at[b + 1] as usize;
        let mem = self.overlay_mem(unsealed, snap, &table.added[b], false)?;
        let pieces: Vec<(usize, std::ops::Range<usize>)> = table
            .pieces
            .iter()
            .map(|(j, at)| (*j, at[b] as usize..at[b + 1] as usize))
            .collect();
        self.overlay_runs(src, mem, &pieces)
    }
    /// PROTOTYPE: a floor under how many keys above the partition block
    /// `b` has, from the table's bounds alone: its largest source's run
    /// over it. Summing the runs was tried first, and a key updated in
    /// every seal is in every piece, so the sum counted it once per
    /// piece: with seven pieces over a range it called every hot block
    /// wide, and a wide block's walk merges every source for every scan
    /// through it. Any one source holds distinct keys, so its run is a
    /// floor; a block can hold more than the floor, spread thin across
    /// its sources, and the merge at `l0_trigger` bounds how thin.
    fn overlay_count(table: &BlockTable, b: usize) -> usize {
        let snap = (table.snap_at[b + 1] - table.snap_at[b]) as usize;
        let pieces = table
            .pieces
            .iter()
            .map(|(_, at)| (at[b + 1] - at[b]) as usize);
        snap.max(table.added[b].len())
            .max(pieces.max().unwrap_or(0))
    }
    /// PROTOTYPE: a wide block's keys above the partition from `cursor`
    /// on, at most `limit` from each source: each source seeked to the
    /// cursor inside the block's run of it, and the runs merged as a
    /// build merges whole ones. What a scan of a wide block walks, and
    /// all it ever assembles of the block.
    fn overlay_window<'a>(
        &'a self,
        src: Sources<'a>,
        table: &BlockTable,
        b: usize,
        unsealed: &'a Snapshot,
        wide: &WideBlock,
        window: (&[u8], usize),
    ) -> Result<Overlay<'a>> {
        let (cursor, limit) = window;
        let (s0, s1) = (table.snap_at[b] as usize, table.snap_at[b + 1] as usize);
        let key_at = |i: usize| unsealed.get(i).map(|(k, _)| k);
        let mut lo = s0;
        let mut hi = s1;
        while lo < hi {
            let m = lo + (hi - lo) / 2;
            if key_at(m).is_some_and(|k| k < cursor) {
                lo = m + 1;
            } else {
                hi = m;
            }
        }
        let snap = lo..s1.min(lo.saturating_add(limit));
        let key_of = |slot: u32| self.mem.key_of(self.mem.entry(slot as usize));
        let f0 = wide.sorted.partition_point(|&(i, _)| key_of(i) < cursor);
        let filed = &wide.sorted[f0..wide.sorted.len().min(f0.saturating_add(limit))];
        let mem = self.overlay_mem(unsealed, snap, filed, true)?;
        let pieces: Vec<(usize, std::ops::Range<usize>)> = table
            .pieces
            .iter()
            .map(|(j, at)| {
                let p = &src.l0[*j];
                let (r0, r1) = (at[b] as usize, at[b + 1] as usize);
                let r = p
                    .ord
                    .seek(p.cursor_from(cursor), |i| p.blob.key_at(i))
                    .clamp(r0, r1);
                (*j, r..r1.min(r.saturating_add(limit)))
            })
            .collect();
        self.overlay_runs(src, mem, &pieces)
    }
    /// PROTOTYPE: the oldest source whose values for an overlay key are
    /// live: 0 with no tombstone in the way, else one past the newest
    /// source holding one. Sources are numbered oldest to newest -- the
    /// partition 0, level-0 pieces 1 through their count, the frozen
    /// memtable, the live one.
    fn oldest_live(
        &self,
        em: &Emit,
        o: &Over,
        held: &[(&[u8], usize, usize, u32)],
        src: Sources,
    ) -> usize {
        let nc = src.l0.len();
        let mut start = 0usize;
        if em.tombs {
            if let Some(sk) = o.sk {
                if sk.mem != u32::MAX && self.mem.has_tomb(self.mem.entry(sk.mem as usize), self.wm)
                {
                    start = nc + 2;
                } else if sk.frozen != u32::MAX
                    && self
                        .frozen
                        .as_ref()
                        .is_some_and(|fr| fr.has_tomb(fr.entry(sk.frozen as usize), SEE_ALL))
                {
                    start = nc + 1;
                }
            }
            if start == 0 {
                for &(_, j, rank, _) in held.iter().rev() {
                    let p = &src.l0[j];
                    if !p.tombs {
                        continue;
                    }
                    if let Some((_, exts)) = p.blob.exts_at(rank) {
                        if exts.iter().any(|e| e.is_tombstone()) {
                            start = j + 1;
                            break;
                        }
                    }
                }
            }
        }
        start
    }
    /// PROTOTYPE: one overlay key emitted as `scan_merged` emits it.
    /// Sources are numbered oldest to newest -- the partition 0, level-0
    /// pieces 1 through their count, the frozen memtable, the live one --
    /// and the newest that holds a tombstone for the key cuts every older
    /// one. `part_rank` names the partition's own record for the key, if
    /// it has one.
    fn emit_over<F: FnMut(&[u8], &[u8])>(
        &self,
        f: &mut F,
        em: &mut Emit,
        ov: &Overlay,
        oi: usize,
        part_rank: Option<usize>,
        src: Sources,
    ) -> Result<()> {
        let o = &ov.over[oi];
        let held = &ov.held[o.pieces.start as usize..o.pieces.end as usize];
        let nc = src.l0.len();
        let key = o.key;
        let read = |e: std::io::Error| err(&format!("block cache read: {e}"));
        let start = self.oldest_live(em, o, held, src);
        if start == 0 {
            if let Some(r) = part_rank {
                src.seg.blob.values_at(r, |v| f(key, v)).map_err(read)?;
            }
        }
        for &(_, j, rank, _) in held {
            if j + 1 >= start {
                src.l0[j]
                    .blob
                    .values_at(rank, |v| f(key, v))
                    .map_err(read)?;
            }
        }
        if let Some(sk) = o.sk {
            if sk.frozen != u32::MAX && nc + 1 >= start {
                if let Some(fr) = self.frozen {
                    let e = fr.entry(sk.frozen as usize);
                    fr.live_offs_into(e, &mut em.scratch, SEE_ALL);
                    for &off in em.scratch.iter() {
                        f(key, fr.value_at(off));
                    }
                }
            }
            if sk.mem != u32::MAX {
                let e = self.mem.entry(sk.mem as usize);
                self.mem.live_offs_into(e, &mut em.scratch, self.wm);
                for &off in em.scratch.iter() {
                    f(key, self.mem.value_at(off));
                }
            }
        }
        Ok(())
    }
    /// PROTOTYPE: the walk over the partition's ranks with the overlay's
    /// keys laid over: the walk `scan_partitions` makes, on a block, with
    /// every source. `on_key` sees every key once before its values,
    /// tombstone-only keys included, which is what a copy needs and a
    /// caller's closure does not.
    fn walk_block<F: FnMut(&[u8], &[u8]), K: FnMut(&[u8])>(
        &self,
        src: Sources,
        ranks: std::ops::Range<usize>,
        ov: &Overlay,
        limit: usize,
        mut on_key: K,
        mut f: F,
    ) -> Result<usize> {
        let mut oi = 0usize;
        let seg = src.seg;
        let hi = ranks.end;
        let mut seen = 0usize;
        let mut rank = ranks.start;
        let mut em: Option<Emit> = None;
        while seen < limit {
            let end = rank.saturating_add(limit - seen).min(hi);
            let next = ov.over.get(oi);
            let (bound, at_bound) = match next {
                Some(o) if o.cut != u32::MAX => BuildCtx::cut_known(o.cut, rank, end),
                Some(o) if end > rank => BuildCtx::cut_at(seg, rank, end, o.key),
                _ => (end, Ordering::Greater),
            };
            if bound > rank {
                let got = seg
                    .blob
                    .scan_at(rank, bound - rank, |k, v| {
                        on_key(k);
                        f(k, v)
                    })
                    .map_err(|e| err(&format!("segment scan: {e}")))?;
                if got < bound - rank {
                    return Err(err(
                        "segment scan: a partition's walk stopped short of its key count",
                    ));
                }
                seen += got;
                rank += got;
                if seen >= limit {
                    break;
                }
            }
            let Some(o) = next else { break };
            let same = rank < hi && at_bound == Ordering::Equal;
            let em = em.get_or_insert_with(|| Emit {
                tombs: self.tombs,
                scratch: Vec::new(),
            });
            on_key(o.key);
            self.emit_over(&mut f, em, ov, oi, same.then_some(rank), src)?;
            if same {
                rank += 1;
            }
            oi += 1;
            seen += 1;
        }
        Ok(seen)
    }
    /// PROTOTYPE: the cached form of block `b`, built on its first touch:
    /// `Clean` with no key above the partition in its range, resolved
    /// deltas with a few, and a merged copy when it is dense with them.
    ///
    /// Building on the first touch was measured against walking the block
    /// through its overlay once and building on the second: on one store
    /// of three million keys, five of every six blocks E touched it
    /// touched again, and each of those paid the overlay twice. At thirty
    /// million, where a copy's build is sixty microseconds of cold reads,
    /// the same variant left a sixth of the blocks E touched unbuilt after
    /// its pass and gained three to four percent on that pass, nothing on
    /// the next, which built every one of them: inside the spread, for a
    /// form more and a walk twice. Fetching every overlay key's memtable
    /// entry and newest chunk ahead of the build, in two sweeps so the
    /// misses overlap, was measured the same way and moved nothing: the
    /// build's cost is not those misses. Timed apart at thirty million, a
    /// copy's build is the pieces' key runs merged, the copy's three
    /// buffers allocated -- fresh heap pages faulted in as the cache
    /// grows, a quarter of the build -- eleven record walks between the
    /// overlay's keys, and twenty keys' values read from their sources,
    /// each a few microseconds and none the most of it.
    fn materialize(
        &self,
        src: Sources,
        table: &BlockTable,
        b: usize,
        unsealed: &Snapshot,
    ) -> Result<Cached> {
        let keys = src.seg.blob.keys();
        let lo = b * CACHE_BLOCK;
        let hi = ((b + 1) * CACHE_BLOCK).min(keys);
        if BuildCtx::overlay_count(table, b) > WIDE {
            return Ok(Cached::Wide(WideBlock {
                sorted: self.sorted_filed(&table.added[b]),
                seen: table.added[b].len(),
            }));
        }
        let ov = self.overlay_all(src, table, b, unsealed)?;
        if ov.over.is_empty() {
            return Ok(Cached::Clean);
        }
        if ov.over.len() < CACHE_DENSE {
            return Ok(Cached::Sparse(self.deltas_for(src, lo..hi, &ov)?));
        }
        Ok(Cached::Block(self.copy_block(src, lo..hi, &ov)?))
    }
    /// PROTOTYPE: the overlay's keys resolved against the partition once:
    /// where each cuts the walk, and its values from every source that
    /// holds it, the partition's own for an equal key first, packed into
    /// one block.
    fn deltas_for(
        &self,
        src: Sources,
        ranks: std::ops::Range<usize>,
        ov: &Overlay,
    ) -> Result<SparseBlock> {
        let seg = src.seg;
        let hi = ranks.end;
        let mut em = Emit {
            tombs: self.tombs,
            scratch: Vec::new(),
        };
        self.prefetch_overlay(ov);
        // Sized once from the key count: a buffer grown by doubling from
        // empty reallocates several times for a block of a few keys.
        let mut blk = SparseBlock {
            keys: Vec::with_capacity(ov.over.len() * 24),
            ents: Vec::with_capacity(ov.over.len()),
            vals: Vec::with_capacity(ov.over.len() * 128),
        };
        let mut rank = ranks.start;
        for (oi, o) in ov.over.iter().enumerate() {
            let (cut, at) = if o.cut != u32::MAX {
                BuildCtx::cut_known(o.cut, rank, hi)
            } else if rank < hi {
                BuildCtx::cut_at(seg, rank, hi, o.key)
            } else {
                (hi, Ordering::Greater)
            };
            let same = cut < hi && at == Ordering::Equal;
            let key_at = blk.keys.len() as u32;
            blk.keys.extend_from_slice(o.key);
            let at = blk.vals.len() as u32;
            let vals = &mut blk.vals;
            self.emit_over(
                &mut |_, v: &[u8]| {
                    vals.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    vals.extend_from_slice(v);
                },
                &mut em,
                ov,
                oi,
                same.then_some(cut),
                src,
            )?;
            blk.ents.push(DeltaEnt {
                key: (key_at, o.key.len() as u32),
                cut: cut as u32,
                same,
                run: (at, blk.vals.len() as u32 - at),
            });
            rank = cut + same as usize;
        }
        Ok(blk)
    }
    /// PROTOTYPE: the walk over ranks `from..hi` with the block's resolved
    /// keys slipped in at their cuts; only keys not below `cursor` are
    /// emitted, which is every one after the first block.
    fn walk_deltas<F: FnMut(&[u8], &[u8])>(
        &self,
        src: Sources,
        ranks: std::ops::Range<usize>,
        blk: &SparseBlock,
        cursor: &[u8],
        limit: usize,
        mut f: F,
    ) -> Result<usize> {
        let seg = src.seg;
        let hi = ranks.end;
        let mut seen = 0usize;
        let mut rank = ranks.start;
        let di = blk.ents.partition_point(|e| blk.key(e) < cursor);
        for e in &blk.ents[di..] {
            let cut = (e.cut as usize).max(rank);
            if cut > rank && seen < limit {
                let want = (cut - rank).min(limit - seen);
                let got = seg
                    .blob
                    .scan_at(rank, want, &mut f)
                    .map_err(|e| err(&format!("segment scan: {e}")))?;
                if got < want {
                    return Err(err(
                        "segment scan: a partition's walk stopped short of its key count",
                    ));
                }
                seen += got;
                rank += got;
            }
            if seen >= limit {
                return Ok(seen);
            }
            let k = blk.key(e);
            blk.each_value(e, |v| f(k, v));
            seen += 1;
            if e.same {
                rank += 1;
            }
        }
        if seen < limit && rank < hi {
            let want = (hi - rank).min(limit - seen);
            let got = seg
                .blob
                .scan_at(rank, want, &mut f)
                .map_err(|e| err(&format!("segment scan: {e}")))?;
            if got < want {
                return Err(err(
                    "segment scan: a partition's walk stopped short of its key count",
                ));
            }
            seen += got;
        }
        Ok(seen)
    }
    /// PROTOTYPE: the memtable lines an overlay's emits will miss, fetched
    /// in two sweeps so the misses overlap instead of following one
    /// another key by key: every key's entry, then the chunk at its head
    /// with the line before it, where the tombstone a put leaves sits.
    /// At three hundred thousand keys, with nothing sealed since the
    /// mixes, every overlay value is in the memtable, and a copy's build
    /// spent 15 of its 23 us emitting 33 keys: two misses a key into a
    /// 5 MB arena.
    fn prefetch_overlay(&self, ov: &Overlay) {
        let tables: [Option<&MemTable>; 2] = [Some(self.mem), self.frozen];
        for o in &ov.over {
            let Some(sk) = o.sk else { continue };
            for (t, slot) in [(tables[0], sk.mem), (tables[1], sk.frozen)] {
                if let Some(t) = t {
                    if slot != u32::MAX && (slot as usize) < t.len() {
                        let e = t.entry(slot as usize);
                        prefetch_lines(
                            e as *const MemEntry as *const u8,
                            std::mem::size_of::<MemEntry>(),
                        );
                    }
                }
            }
        }
        for o in &ov.over {
            let Some(sk) = o.sk else { continue };
            for (t, slot) in [(tables[0], sk.mem), (tables[1], sk.frozen)] {
                if let Some(t) = t {
                    if slot != u32::MAX && (slot as usize) < t.len() {
                        let head = MemTable::head(t.entry(slot as usize));
                        if head != NO_CHUNK {
                            let at = head as usize;
                            t.vals.prefetch(at.saturating_sub(64), 192);
                        }
                    }
                }
            }
        }
    }
    /// PROTOTYPE: the merged copy of the ranks with the overlay laid over
    /// them.
    fn copy_block(
        &self,
        src: Sources,
        ranks: std::ops::Range<usize>,
        ov: &Overlay,
    ) -> Result<CachedBlock> {
        // Every key once, then its values: the walk's `on_key` opens an
        // entry on a key change, and a key without values still gets one.
        // The two closures never run at once; the cell is for the borrow
        // checker.
        self.prefetch_overlay(ov);
        let n = ranks.len() + ov.over.len();
        let blk = std::cell::RefCell::new(CachedBlock {
            keys: Vec::with_capacity(n * 16),
            vals: Vec::with_capacity(n * 128),
            ents: Vec::with_capacity(n),
        });
        let mut current: Vec<u8> = Vec::with_capacity(32);
        let mut open = false;
        self.walk_block(
            src,
            ranks,
            ov,
            usize::MAX,
            |k| {
                if !open || current.as_slice() != k {
                    let mut b = blk.borrow_mut();
                    if open {
                        b.end();
                    }
                    b.begin(k);
                    current.clear();
                    current.extend_from_slice(k);
                    open = true;
                }
            },
            |_k, v| blk.borrow_mut().push(v),
        )?;
        let mut blk = blk.into_inner();
        if open {
            blk.end();
        }
        Ok(blk)
    }
    /// The first rank in `rank..end` whose key is not below `uk`, or `end`,
    /// with how that rank's key compares to `uk` -- `Equal` for the key
    /// itself, `Greater` for a key past it or a rank that does not resolve,
    /// which sorts as "not less" the way the seek's damage rule has it.
    ///
    /// Searched over the ordered index's heads for the window -- sixty-four
    /// eight-byte heads in eight lines, hot after the block's first build
    /// -- with one key read after, for the comparison the emit needs.
    /// Before this the search read the records: the rank itself first,
    /// then the window's last key, then a gallop from the rank and a
    /// binary search over the gap it landed in, a parsed key at every
    /// probe; at 300k keys a sparse block's build was 3 us, most of it
    /// here, and a fifth of E's first pass.
    fn cut_at(seg: &Seg, rank: usize, end: usize, uk: &[u8]) -> (usize, Ordering) {
        let r = seg.ord.seek_in(rank, end, uk, |i| seg.blob.key_at(i));
        if r >= end {
            return (end, Ordering::Greater);
        }
        let at = seg.blob.key_at(r).map_or(Ordering::Greater, |k| k.cmp(uk));
        (r, at)
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        self.stop_ahead();
        if let Some(h) = self.sealing.take() {
            let _ = h.join();
        }
        if let Some((_, h)) = self.compacting.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod threading {
    /// A reader handle crosses threads, and the state it reads is shared
    /// between them: the compiler holds both, which is what keeps a cell
    /// out of a segment or a memtable.
    #[test]
    fn the_state_is_shared_and_the_handle_is_sent() {
        fn shared<T: Send + Sync>() {}
        fn sent<T: Send>() {}
        shared::<super::State>();
        shared::<super::Shared>();
        sent::<super::Reader>();
    }
}
