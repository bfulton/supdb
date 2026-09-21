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
    /// EXPERIMENT: the share of the store's bytes the memtable may reach
    /// before a commit seals, as a cap on `seal_threshold` rather than a
    /// floor under it; zero is no cap.
    ///
    /// `seal_bytes` and `seal_grows` are both floors, so on a store
    /// smaller than the floor the memtable can hold the whole of it and
    /// never seal: a hundred thousand keys are six megabytes against a
    /// 32 MiB seal, so the lag sweep's every point sits unsealed. That is
    /// the axis where this engine is not competitive -- drained it reads
    /// 1.35x-1.64x of LMDB and with a tenth of the store unmerged
    /// 0.24x-0.35x -- and a B-tree has no such axis because it pays for
    /// every write where the write happens. The cap moves that cost back
    /// to the writer, which is what it buys and what it costs, so the
    /// load and the lag sweep are priced together or not at all.
    ///
    /// It engages only once something has been sealed, since a store of
    /// no bytes has no share to take, so a first load runs uncapped and
    /// the load axis does not move: measured, `device_bytes_per_byte` is
    /// identical with it and without.
    ///
    /// Ten percent, floored at `SEAL_CAP_FLOOR`, is what the sweep left.
    /// At a hundred thousand keys it reads ycsb-E at 1.302x-1.421x of the
    /// uncapped arm (9/9 and 11/11, p<=0.004) and the fully unmerged lag
    /// point at 5.66x-5.96x, for ycsb-F at 0.834x-0.907x. That trade is
    /// one-sided on the ladder: ycsb-E is the only mix this engine loses
    /// to LMDB, at 0.74x-0.87x, and ycsb-F it wins by 2.84x-6.27x.
    pub seal_max_pct: usize,
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
    /// EXPERIMENT: aligned pieces over one partition's range at which they
    /// are merged into one piece on a thread of their own, so a read meets
    /// fewer pieces while the partition merge lags; zero, the default,
    /// leaves the pieces for the partition merge. `supdb-tier` prices it
    /// at three. What a read pays over an unmerged range is the pieces --
    /// a block's build at the sweep's fully-unmerged point is 74,000
    /// instructions at seven or eight pieces and 18,000 at two -- and the
    /// merge takes them from seven or eight to one or two there, but the
    /// point reads 1.11x-1.13x for it and nothing else on the ladder
    /// moves, and the pieces it makes count one each toward `l0_trigger`,
    /// so a range that keeps folding never reaches it and the partition
    /// merge waits for a flush while the merged piece is rewritten every
    /// few seals; `docs/engine.md` has the measurement and what a version
    /// worth turning on would count. A piece merge carries the tombstones
    /// its inputs held, since the partition below still holds what they
    /// mask, and no partition merge starts while one runs.
    pub tier_pieces: usize,
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
    /// PROTOTYPE: with the block cache, build ahead of the reader on a
    /// thread of its own. At the writer's first scan over a published
    /// state, a reader handle pins the store as of the last commit and
    /// builds every block a piece or an unsealed key overlays, sending
    /// each form back as it is built; the store installs the forms at
    /// its scans, while the state is the one they were built over, and
    /// splices in the keys written after the builder's commit, which its
    /// own read of the write log lists. The scans after find the blocks
    /// built. Once per state: an installed form is kept current by the
    /// settle of every write after. Under `scan_cache_bytes` the builder
    /// stops at the budget, and a store whose partitions hold fewer than
    /// `scan_cache_ahead_min_blocks` gets none. `false` is the arm
    /// without the thread.
    pub scan_cache_ahead: bool,
    /// PROTOTYPE: the partitions' blocks below which no builder starts.
    /// Measured on the probe's E, six rounds interleaved, the builder
    /// against none: at ten thousand keys, 160 blocks, 310k-351k against
    /// 598k-699k ops/s; at thirty thousand, 470 blocks, 588k-635k against
    /// 529k-772k; at a hundred thousand, 1,560 blocks, 702k-773k against
    /// 682k-758k. A test that wants the builder on a small store sets 0.
    pub scan_cache_ahead_min_blocks: usize,
    /// EXPERIMENT: the range-read structure written at ingest. The writer
    /// keeps a canonical form of every block an unsealed key overlays
    /// current at each commit, the builder ahead having made the first
    /// ones, and a reader at the last commit walks them as one structure
    /// and builds nothing; see `CanonicalForm`. `false` is the arm where
    /// every handle builds and settles its own.
    pub commit_forms: bool,
    /// Scans over a state, through handles the caller made, before the
    /// writer keeps the canonical forms current at its commits. The
    /// structure is built once and read by whoever holds a handle, so
    /// its cost is the writer's and its benefit divides among the
    /// readers: measured over two roster positions, forms read 1.280x
    /// and 1.441x of the arm without them on the threaded scan mix and
    /// 0.849x and 0.879x on ycsb-E, which is the writer scanning its own
    /// store. One such scan is the evidence that the first case is the
    /// one this store is in.
    ///
    /// One, and it is a trade rather than a win. `bench ab` over fifteen
    /// pairs at a hundred thousand keys: maintaining regardless reads
    /// 1.075x on the threaded scan mix, thirteen pairs of fifteen, p
    /// 0.007, and 0.859x on `scan-lag` at full lag, fourteen of fifteen,
    /// p 0.001, with ycsb-E a wash. One stands because the loss is the
    /// larger and comes with twice the memory: 3,099 forms and 2.8 MB
    /// held at the end of a pass against 1,537 and 1.3 MB.
    ///
    /// Reads take a form 24,225 times over a pass, on 6,000 of the
    /// 12,000 scans through a caller's handle, and the same in both
    /// arms -- which is not evidence that maintaining is idle, though it
    /// was read that way here once. Both arms maintain by the end of a
    /// pass; the gate moves when that starts, not whether. What settles
    /// it is `supdb-settle`, which keeps the settle and builds no form:
    /// takes fall to zero and the threaded scan mix to 0.585x on four
    /// threads, fifteen pairs of fifteen. The forms built at a commit
    /// are the forms a read takes, and they are worth 1.71x on that
    /// workload against not building them at all.
    ///
    /// Zero now, where it was one. One waits for a handle the caller made
    /// to have scanned, on the reasoning that the forms lose where the
    /// writer reads its own store; measured against the seal cap that is
    /// no longer true. Eleven pairs each: at ten thousand keys the lag
    /// sweep reads 1.369x at a hundredth unmerged, 3.352x at a tenth and
    /// 5.643x at all of it (10/11 and 11/11, p<=0.012), and at a hundred
    /// thousand 1.398x at a hundredth (11/11, p=0.001), with every mix
    /// and the load within noise and the forms' bytes unchanged there.
    /// Before the cap the same flip lost 0.869x at the fully unmerged
    /// point and cost 2.18x those bytes: capping the seal leaves less
    /// unsealed, so the fill it pays for is smaller and the reads that
    /// use it are the same. The cost that is left is memory on a small
    /// store -- 162 KB of forms against 1.52 MB at ten thousand keys --
    /// and `scan_cache_bytes` is the bound for a caller who minds.
    pub forms_from_reader_scans: usize,
    /// Whether the writer, having settled its batch into the blocks it
    /// landed in, also builds a form for every block an unsealed key
    /// overlays and publishes what it touched. Off keeps the settle and
    /// the install of the builder ahead's forms and stops there.
    ///
    /// It was added to test a wrong guess -- that the forms built here
    /// are not the ones read, since adoption did not move between the
    /// gated arm and the ungated one -- and it refuted it: off, a read
    /// takes no form at all (24,225 to none) and the threaded scan mix
    /// falls to 0.585x on four threads and 0.638x on two, fifteen pairs
    /// of fifteen. It is kept because it prices the whole mechanism in
    /// one option: what the forms at a commit are worth is 1.71x there,
    /// against 1.172x on `scan-lag` at full lag and 1.130x on ycsb-E for
    /// not paying for them.
    pub commit_forms_build: bool,
    /// Publish the scan snapshot in the state, where every handle adopts
    /// it instead of sorting the unsealed keys again. Off is the shape
    /// before it, kept as the comparison arm: measured against it over
    /// two roster positions, the threaded scan mix read 1.145x and
    /// 1.212x and nothing else moved, so this is on.
    pub share_snapshot: bool,
    /// EXPERIMENT: the most live keys a published snapshot may lack
    /// before a handle sorts its own instead. A published snapshot stops
    /// where the handle that built it stopped, and the slots past that
    /// stay in the adopting handle's added list, where each is carried
    /// into the block it overlays: adopting saves the sort and buys that
    /// list. See the sweep in the pull request for where the two cross.
    pub snapshot_adopt_behind: usize,
    /// EXPERIMENT: the scan snapshot carries each unsealed key's chain,
    /// copied in key order when the snapshot is built, so a read of an
    /// overlaid key streams the copy instead of chasing the memtable's
    /// entry, chain and value; and a block those runs cover -- three
    /// quarters of its keys or more, counting every source's run over
    /// it -- is walked on a read's first touch rather than copied,
    /// since every partition record under such a run is masked by its
    /// update's tombstone and the copy would be a copy of the run, and
    /// copied on its second, since a block read over and over wants
    /// the copy.
    /// `docs/engine.md` has the pricing: on the lag sweep's last point at
    /// ten thousand keys the handle passes read 8.0 µs a scan against
    /// 9.7 without, level at thirty thousand where the pieces' keys are
    /// read per window either way, and the writer's own pass reads
    /// slower by the copy, which lands in the first scan after a burst
    /// -- at ten thousand keys 1.5 ms over ten thousand chains, half the
    /// pass. Off, and `supdb-runs` prices it; `snapshot_keeper` pays the
    /// copy at the commit instead, on a thread of the store's own, and
    /// is the write-time structure this is the read half of.
    pub snapshot_runs: bool,
    /// EXPERIMENT: the run keeper, a thread of the store's own at idle
    /// priority that keeps the published scan snapshot current to the
    /// commits. Polling them once a scan has happened over the store,
    /// it appends the batches' keys and runs to the snapshot's shared
    /// arena and publishes the extended version, so the first scan
    /// after a burst adopts a snapshot that has the burst instead of
    /// sorting it and copying its runs -- the copy that landed in the
    /// writer's first scan under `snapshot_runs` alone, or inline at
    /// each commit where the forms are maintained there. It carries the
    /// snapshot across a seal, the live entries becoming frozen ones at
    /// the freeze and leaving when the seal lands, and across a merge
    /// unchanged. Off, and `supdb-keeper` prices it beside `supdb-runs`;
    /// `docs/engine.md` has the figures.
    pub snapshot_keeper: bool,
    /// EXPERIMENT: how far past the last scan over the store the keeper
    /// follows the writes, as a percentage of the store's sealed keys,
    /// before it waits for a scan: its copy is a copy of every value
    /// written, and a load with no read in sight is not worth doubling
    /// in memory. Zero follows every write.
    pub snapshot_keeper_recent_pct: usize,
    /// EXPERIMENT: entries reads must have taken from a block before the
    /// store holds it a second way, as a merged copy beside the cheap
    /// form, so that the reads ask for the shape rather than the writes
    /// implying it. Zero holds every block one way, which is the cache
    /// as it was, with `CACHE_DENSE` choosing that way from the
    /// overlay's size. The number is a measured crossover and nothing
    /// else: walking a copy saves about twenty-two cycles an entry over
    /// walking deltas over the partition, and building one costs three
    /// to five thousand, so a block repays a copy after a few hundred
    /// entries of reading. A write to a block drops its copy and halves
    /// its count, which is the hysteresis: a block the writes own never
    /// holds one, and a block the reads own has it back after half the
    /// entries.
    pub promote_entries: usize,
    /// EXPERIMENT: unsealed entries written since the last builder that
    /// start another from a commit, rather than waiting for a scan to
    /// ask. The builder organises what the writes left -- a snapshot of
    /// the unsealed keys and a form for every block they overlay -- and
    /// that work is what the first read after a write burst pays for
    /// today: measured on the lag sweep at a hundred thousand keys, a
    /// scan pass over a store with every key rewritten costs 33 µs a
    /// scan the first time and 1.3 µs after, and the fault count goes
    /// from five to five thousand with it. Zero waits for the scan,
    /// which is where the builder started before. A builder already
    /// running is left alone.
    pub build_ahead_on_commit: usize,
    /// EXPERIMENT: a builder started at the first commit after a publish
    /// -- a seal, a join, a merge -- whether or not a scan has asked,
    /// and its forms installed at the commits they arrive at rather than
    /// at the first scan after; a commit a builder has posted forms to
    /// installs them and settles its batch as a scan-preceded commit
    /// does. Built for the fully-unmerged lag point, where a publish
    /// empties the table and the first reads after the burst build every
    /// block they walk: nine hundred blocks at 13-20 µs each across a
    /// thousand scans at a hundred thousand keys, a tenth of LMDB.
    ///
    /// Off, because the burst outruns the builder. That point seals a
    /// dozen times and merges between, every publish restarts the
    /// builder over the whole overlay, and the commits it posts to file
    /// their batches: 63,000 of the burst's 100,000 writes were filed at
    /// the commits, the writes ran 2.7x slower, and the scans still met
    /// an empty table, since the last seal restarted the builder just
    /// before them -- the point read 1.16x at a hundred thousand keys
    /// and 0.92x at three hundred thousand. In the mixes the builder
    /// restarted at each of F's seals reads ycsb-E at 0.84x-0.87x and F
    /// at 0.80x-0.97x. `supdb-aheadpub` prices it. The count above,
    /// which restarts by writes, is the other trigger and is also off.
    pub build_ahead_on_publish: bool,
    /// EXPERIMENT: the canonical forms published only while a handle the
    /// caller made is live to take them, and all at once when one is
    /// claimed. A publish is an `Arc` clone of the writer's own form, so
    /// every patch after it copies the block first, and a zipfian batch
    /// of five thousand writes touches about a thousand blocks; the
    /// suite's mixes and its lag sweep hold no handle, so every publish
    /// there was for nobody. With no handle live the writer settles as
    /// before and leaves the forms dirty; a claim files the backlog and
    /// publishes every dirty form, at the log's length when nothing is
    /// staged and at the next commit otherwise, and the table's position
    /// and completeness move only when it publishes, so a handle at an
    /// older commit still finds the forms of that commit. `false`
    /// publishes at every maintained commit as before; `forms_to_writer`
    /// publishes regardless, since the writer is then a taker too.
    /// Eleven pairs at a hundred thousand keys: the tenth-unmerged lag
    /// point at 1.11x (10/11, p=0.012), every mix within noise.
    pub forms_publish_lazily: bool,
    /// EXPERIMENT: the share of the store, as a percentage of the keys
    /// its partitions hold, that may be unsealed before a commit stops
    /// maintaining the forms; zero is no bound, which is what the
    /// maintenance had before this.
    ///
    /// The bound exists because maintaining regardless of who reads is
    /// worth 3.19x on the lag sweep at ten thousand keys with a tenth of
    /// the store unmerged, and costs 0.869x at a hundred thousand with
    /// *all* of it unmerged -- the one rung where it loses. At that depth
    /// every block is overlaid, every form is a merged copy rather than
    /// resolved deltas, and the scans are an order of magnitude slower
    /// anyway, so the forms are rebuilt wholesale for reads that cannot
    /// repay them. The store is also about to seal. Stopping is safe at
    /// any commit: `forms_at` stops advancing and a reader trusts a form
    /// only where it matches the log position it holds.
    pub forms_max_unsealed_pct: usize,
    /// EXPERIMENT: unfiled writes, as a share of the partitions' keys,
    /// past which a commit settles into the forms whether or not a scan
    /// has preceded it; zero never does.
    ///
    /// A run of writes with no read between them settles only its first
    /// batch, and the first read after the run files every batch since:
    /// on the lag sweep at a hundred thousand keys that is nine thousand
    /// writes filed by the first of a thousand scans, about 13 ms charged
    /// to the scans, and it is what the sweep measures at a tenth
    /// unmerged. A B-tree pays that at the write. Settling at every
    /// commit instead reads that point at 3.56x and ycsb-E at 1.14x
    /// (11/11, p=0.001) and costs ycsb-A 0.68x and ycsb-F 0.67x (0/11),
    /// because a commit in a write-heavy mix then rebuilds the sorted
    /// snapshot every time. A bound on the backlog caps what the first
    /// read pays without paying at every commit.
    ///
    /// A share and not a count: five thousand writes is a twentieth of a
    /// hundred thousand keys and there it reads the lag point at 1.90x
    /// (11/11, p=0.001) with every mix within noise, and it is a sixtieth
    /// of three hundred thousand, where the same count reads the lag
    /// point at 3.59x and ycsb-A at 0.66x and F at 0.80x (0/7), because
    /// the mixes there write five times over it. Five percent was the
    /// default while a settle copied every published block it touched.
    ///
    /// Two percent is the default now that a settle publishes nothing
    /// for readers that are not there. Against five, eleven pairs at a
    /// hundred thousand keys read the lag point at 2.31x (11/11,
    /// p=0.001) and ycsb-D at 0.86x (1/11, p=0.012), A, E and F within
    /// noise; seven at three hundred thousand read 2.11x (7/7, p=0.016)
    /// and D at 0.82x (0/7). D's loss is not a slower read: it is the
    /// one commit of its twenty-five where the backlog crosses the bound
    /// -- six thousand writes at three hundred thousand keys, most of
    /// them F's tail, 5 ms against 0.4 for every other commit -- while
    /// the reads on either side of it take what they took. The bound
    /// decides who files a burst's tail, the mix that commits when it
    /// crosses or the first read after it, and the point that reads
    /// after a tenth of the store rewritten was at half of LMDB either
    /// way this engine charged it to the reads.
    pub forms_settle_backlog_pct: usize,
    /// EXPERIMENT: the writes since the last scan over the store, as a
    /// share of the partitions' keys, within which the backlog bound
    /// above settles at all; zero, the default, settles by the bound
    /// whenever it is passed.
    ///
    /// Filing a write into a block form costs the same whoever does it,
    /// about 0.7 us at a hundred thousand keys, on the commit path or in
    /// the first scan after the burst; the bound only moves it. What
    /// makes the move a loss is that the forms belong to one memtable
    /// generation and a seal discards them: a burst with no scan between
    /// its commits and the next seal was filed for nobody. Over the
    /// suite's mixes at a hundred thousand keys, ycsb-A's settles under
    /// a bound of two percent built a thousand forms in four commits
    /// and ycsb-F's two seals threw every one away, while the lag
    /// sweep's same-sized burst is scanned a thousand times before any
    /// seal. This window was the bet that a scan within the last tenth
    /// of the store written says the reads are near, a share and not a
    /// count of commits since the same burst is nine commits at a
    /// hundred thousand keys and 270 at three million.
    ///
    /// Measured in the suite's shape at a hundred thousand keys, the bet
    /// does not pay where it was meant to. Both bursts are nine percent
    /// of the store written after a scan pass, so the window cannot
    /// tell A's from the sweep's at the commit that decides: A settles
    /// four times either way. What the window does is leave F's tail
    /// and D's inserts unfiled, which spares F about a tenth and D
    /// nothing and charges ycsb-E, the first reader after them, the
    /// filing of eight thousand writes it inherited: about a tenth of
    /// E. The lag sweep reads as the bound alone reads. Zero is the
    /// default; `supdb-recency` prices it. What the measurement says
    /// instead is that the loss is the seal's discard, not the bound's
    /// timing.
    pub forms_settle_recent_pct: usize,
    /// EXPERIMENT: the canonical forms carried across a seal. A form is
    /// the merged content of a partition's block and everything unsealed
    /// above it, and a seal changes none of that content: the memtable's
    /// values move into a piece. What a seal changes is the bookkeeping
    /// a form was resolved with -- the memtable's slots, the pieces'
    /// ranks, the snapshot's bounds -- and every publish makes a fresh,
    /// empty table and drops the writer's own tables with it, so
    /// everything filed since the last seal is filed for nobody. With
    /// this the writer files the backlog at the freeze, so the forms are
    /// current to the whole log, carries the state's pointers into the
    /// state the freeze publishes and again into the two the piece's
    /// join publishes, keeps its own tables' forms and remakes only
    /// their bookkeeping. A merge changes the partitions and starts
    /// afresh as before.
    ///
    /// Off, because it is correct and does not pay: eleven pairs at a
    /// hundred thousand keys read ycsb-F at 0.83x (0/11) and ycsb-E at
    /// 0.89x (1/11, p=0.012). It was built for two losses that looked
    /// like the seal's discard, and measured in the suite's shape
    /// neither was. ycsb-E inherits F's forms and reads slower: the
    /// builder ahead fills E's table on a spare core within its first
    /// millisecond either way, and E's own settles now patch carried
    /// forms, a copy per touched block, where over an emptied table they
    /// patched nothing. The fully-unmerged lag point reads with an empty
    /// table under both arms, three to seven pieces still standing at
    /// its scans: what empties the table there is the merges during the
    /// burst, whose rewritten partition no old form maps onto, and the
    /// writes there run 2.3x slower carrying forms the next merge drops.
    /// F pays the same way at its two seals. `supdb-carry` prices it,
    /// and the test holds it to the model through two seals and a
    /// merge.
    pub forms_carry: bool,
    /// EXPERIMENT: the writer's own handle takes the canonical forms it
    /// maintains, instead of building its own. Without this the
    /// maintenance is pure cost wherever the reads are the writer's: a
    /// form is taken only by a handle with a slot, and the writer's has
    /// none, so the suite's ycsb-E -- whose scans go through the writer
    /// -- pays for a structure it cannot read, and the arm that builds
    /// no form at commit reads 1.06x-1.23x of the one that does.
    ///
    /// What the slot stood for is two things, and neither needs it. The
    /// epoch: a form is freed past every pinned reader, and the writer
    /// is the only thread that frees one and frees none while it reads,
    /// which `canonical` already says. The watermark: a form is settled
    /// at the last commit and the writer's reads honour none, so a form
    /// is what the writer must read only while nothing has been written
    /// past that commit. That is a comparison rather than a rule, so the
    /// scan makes it and builds its own where it has staged writes.
    pub forms_to_writer: bool,
    /// EXPERIMENT: overlay keys in a block from which the writer's
    /// canonical form is a merged copy rather than resolved deltas; zero
    /// is `CACHE_DENSE`, the threshold a handle building for itself
    /// uses, and one is a copy of every overlaid block. The arm exists
    /// because a form built once and patched per write has different
    /// economics from one rebuilt on a read: `CACHE_DENSE` was measured
    /// against the rebuilding cache.
    pub form_dense_from: usize,
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
            seal_max_pct: 10,
            seal_grows: true,
            direct_ingest: true,
            segment: SegmentOptions::default(),
            l0_trigger: 4,
            tier_pieces: 0,
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
            scan_cache_ahead: true,
            scan_cache_ahead_min_blocks: 1024,
            commit_forms: true,
            forms_from_reader_scans: 0,
            commit_forms_build: true,
            share_snapshot: true,
            snapshot_adopt_behind: 0,
            snapshot_runs: false,
            snapshot_keeper: false,
            snapshot_keeper_recent_pct: 100,
            promote_entries: 0,
            build_ahead_on_commit: 0,
            build_ahead_on_publish: false,
            forms_publish_lazily: true,
            forms_max_unsealed_pct: 0,
            forms_settle_backlog_pct: 2,
            forms_settle_recent_pct: 0,
            forms_carry: false,
            forms_to_writer: false,
            form_dense_from: 0,
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
    /// taken once per table made, which clones the shared vector and
    /// reads it lock-free at every block build after: a lock per block
    /// build and piece bounced its word between a builder's thread and
    /// the scan's, twenty-two pieces a block at three million keys.
    ranks: std::sync::RwLock<Option<(u64, std::sync::Arc<Vec<u32>>)>>,
    /// Where this piece's positions fall against a partition's block
    /// boundaries, by the partition's blob id: a function of two sealed
    /// files, so taken once and shared by every table made over the
    /// pair. Every table -- the writer's at each state a seal or a merge
    /// published, the builder's, and each handle's at its first scan --
    /// walked the piece against the boundaries again: a handle's first
    /// scan after the mixes at three hundred thousand keys walked one
    /// piece for 360 us of a pass of 7 ms, and the writer walked every
    /// piece for each new one. Keyed by the partition's identity for the
    /// reason `ranks` is.
    bounds: std::sync::RwLock<BoundsById>,
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

/// The segment file's output, written in 2 MB pieces at 2 MB offsets.
///
/// The page cache sizes a folio by the write that creates it, at that
/// write's alignment: a file written in pieces of a few hundred kilobytes
/// is cached in folios of that size, and a mapping of it costs a page
/// table entry and a TLB entry per 4 KB page, where a file written in
/// 2 MB pieces at 2 MB offsets is cached in PMD-sized folios that the
/// mapping takes with one entry each. The difference is the address
/// translation a scan pays over a partition of 46 MB: 12% of the scan
/// through the blob, paired window by window against the same bytes
/// rewritten in one write, and nothing over a partition of 16 MB. The
/// WAL recycler met the same rule from the other side, a folio sized by
/// a write far larger than the commits after it (`CLAUDE.md`). A
/// `BufWriter` of a megabyte flushed wherever it filled. This buffers to
/// the next boundary of the file and writes the piece there whole; a
/// flush before the boundary writes what there is, and the direct
/// segment's commits do that by design.
struct AlignedWriter {
    file: File,
    buf: Vec<u8>,
    /// Bytes on the file so far: where the buffer's contents land.
    written: u64,
}

/// The piece: the PMD size on x86-64 and on arm64 with 4 KB pages, and a
/// harmless write size anywhere else.
const WRITE_PIECE: u64 = 2 << 20;

impl AlignedWriter {
    fn new(file: File) -> AlignedWriter {
        AlignedWriter {
            file,
            buf: Vec::with_capacity(WRITE_PIECE as usize),
            written: 0,
        }
    }

    fn get_ref(&self) -> &File {
        &self.file
    }

    fn into_inner(mut self) -> std::io::Result<File> {
        std::io::Write::flush(&mut self)?;
        Ok(self.file)
    }

    /// Bytes from the file's end to the next boundary: what the buffer
    /// holds before it is written.
    fn to_boundary(&self) -> usize {
        (WRITE_PIECE - self.written % WRITE_PIECE) as usize
    }

    fn write_buf(&mut self) -> std::io::Result<()> {
        std::io::Write::write_all(&mut self.file, &self.buf)?;
        self.written += self.buf.len() as u64;
        self.buf.clear();
        Ok(())
    }
}

impl std::io::Write for AlignedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut rest = data;
        while !rest.is_empty() {
            let room = self.to_boundary() - self.buf.len();
            let take = room.min(rest.len());
            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buf.len() == self.to_boundary() {
                self.write_buf()?;
            }
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            self.write_buf()?;
        }
        self.file.flush()
    }
}

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
    out: AlignedWriter,
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
        let mut out = AlignedWriter::new(file);
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

        let file = self.out.into_inner()?;
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
struct PieceWriter(Box<SegmentWriter>, crate::ordindex::Builder, bool);

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
        Ok(PieceWriter(
            Box::new(w),
            crate::ordindex::Builder::new(),
            false,
        ))
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
        self.2 |= tombstone;
        self.0.end_with(tombstone)
    }

    /// Whether any key ended with a tombstone: a piece with one cannot
    /// be a partition as it is, since the bottom level drops them.
    fn tombs(&self) -> bool {
        self.2
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
                bounds: std::sync::RwLock::new(Vec::new()),
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
                bounds: std::sync::RwLock::new(Vec::new()),
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
            bounds: std::sync::RwLock::new(Vec::new()),
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
    /// The write log's length and the entry count at the last commit:
    /// what a builder ahead of the reader takes its watermark with, so
    /// the writes it lacks are exactly the log from that length on and
    /// the entries its snapshot covers are exactly the committed ones.
    committed_log: AtomicUsize,
    committed_len: AtomicUsize,
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
            // Not zeroed: a byte is read only past a write that covered
            // it, and zeroing a fresh table's first blocks -- eleven
            // megabytes with the slabs -- cost its first write 3.7 ms,
            // which was 3x of ycsb-B at ten thousand keys.
            // SAFETY: a non-zero layout; the block is freed by `Drop`.
            let p = unsafe { std::alloc::alloc(layout) };
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
            // Not zeroed: an entry is read only below `len`, and each is
            // written whole before `len` covers it. See the arena.
            // SAFETY: a non-zero layout; the block is freed by `Drop`.
            p = unsafe { std::alloc::alloc(layout) } as *mut T;
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
    slots: Box<[Slot]>,
}

/// A reader's slot on a cache line of its own. The slots were adjacent
/// words, eight to a line, and every read pins and unpins its handle's
/// slot with a store, so four handles claimed in order stored to one
/// line from four cores on every read. Measured on a partitioned store
/// of ten thousand keys, uniform point reads through a handle per
/// thread, two binaries alternated over three rounds: one thread 8.8M
/// a second and four threads 8.9M with the slots adjacent, 9.0M and
/// 33.7M with each on a line; at thirty thousand, 7.2M and 6.9M against
/// 7.7M and 29.1M. 128 bytes covers a machine whose lines are that wide.
#[repr(align(128))]
struct Slot {
    epoch: AtomicU64,
    /// The handle's statistics, on this line rather than on the store's
    /// shared words: every scan through a handle bumped seven to
    /// thirteen shared counters and every form it took two more, and
    /// four handles on four cores bumping the same lines read the
    /// threaded scan mix at ten thousand keys at 0.46x of LMDB, 0.91x
    /// with the counters off. Written by the slot's handle alone, read
    /// by the accessors, which sum the slots.
    scans: AtomicU64,
    blockpath: AtomicU64,
    takes: AtomicU64,
    tried: AtomicU64,
    hit: AtomicU64,
    built: AtomicU64,
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            epoch: AtomicU64::new(0),
            scans: AtomicU64::new(0),
            blockpath: AtomicU64::new(0),
            takes: AtomicU64::new(0),
            tried: AtomicU64::new(0),
            hit: AtomicU64::new(0),
            built: AtomicU64::new(0),
        }
    }
}

const _: () = assert!(std::mem::align_of::<Slot>() >= 128 && std::mem::size_of::<Slot>() >= 128);

const READER_SLOTS: usize = 256;

impl Readers {
    fn new() -> Readers {
        Readers {
            epoch: AtomicU64::new(1),
            slots: (0..READER_SLOTS).map(|_| Slot::new()).collect(),
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
            let v = s.epoch.load(AtomicOrdering::SeqCst);
            v == 0 || v >= epoch
        })
    }

    /// A slot for a reader handle's life, or none when every slot is
    /// taken.
    fn claim(&self) -> Option<usize> {
        (0..READER_SLOTS).find(|&i| {
            self.slots[i]
                .epoch
                .compare_exchange(0, u64::MAX, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
                .is_ok()
        })
    }

    fn release(&self, slot: usize) {
        self.slots[slot].epoch.store(0, AtomicOrdering::SeqCst);
    }

    /// One handle statistic, summed over the slots.
    fn stat(&self, pick: impl Fn(&Slot) -> &AtomicU64) -> u64 {
        self.slots
            .iter()
            .map(|s| pick(s).load(AtomicOrdering::Relaxed))
            .sum()
    }

    /// Pin the current epoch in `slot`: a load, a store, and the load
    /// again, so an epoch the writer bumped between the two is not the one
    /// left pinned. Between operations a claimed slot holds `u64::MAX`,
    /// which no retirement is ever older than.
    fn pin(&self, slot: usize) {
        loop {
            let e = self.epoch.load(AtomicOrdering::SeqCst);
            self.slots[slot].epoch.store(e, AtomicOrdering::SeqCst);
            if self.epoch.load(AtomicOrdering::SeqCst) == e {
                return;
            }
        }
    }

    /// Release suffices: the writer's `none_before` wants the unpin to
    /// come after the reads it ends, and nothing here waits on it.
    fn unpin(&self, slot: usize) {
        self.slots[slot]
            .epoch
            .store(u64::MAX, AtomicOrdering::Release);
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
            committed_log: AtomicUsize::new(0),
            committed_len: AtomicUsize::new(0),
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
            committed_log: AtomicUsize::new(0),
            committed_len: AtomicUsize::new(0),
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

    fn value_bytes(&self) -> usize {
        self.vals.used()
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
        self.committed_log
            .store(self.log.len(), AtomicOrdering::Release);
        self.committed_len
            .store(self.entries.len(), AtomicOrdering::Release);
        self.committed
            .store(self.vals.tail() as u64, AtomicOrdering::Release);
    }

    /// The watermark a reader honours to see committed chunks only.
    fn committed(&self) -> u64 {
        self.committed.load(AtomicOrdering::Acquire)
    }

    /// The write log's length at the last commit.
    fn committed_log(&self) -> usize {
        self.committed_log.load(AtomicOrdering::Acquire)
    }

    /// The entry count at the last commit.
    fn committed_len(&self) -> usize {
        self.committed_len.load(AtomicOrdering::Acquire)
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
        // A partition merge writes the bottom level, so no output extent
        // carries the tombstone flag: there is nothing older left for it
        // to mask. A piece merge carries it, since the partition below
        // still holds what it masks.
        self.key_with(k, false, pull)
    }

    fn key_with(
        &mut self,
        k: &[u8],
        tombstone: bool,
        pull: impl FnOnce(&mut PieceWriter) -> Result<()>,
    ) -> Result<()> {
        let to = self.enter(Some(k))?;
        let w = self.w.as_mut().ok_or_else(|| err("merge piece not open"))?;
        w.begin(k)?;
        pull(w)?;
        w.end_with(tombstone)?;
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

/// A piece merge: the aligned pieces over one partition's range, oldest
/// first, into one piece over the same range.
struct TierPlan {
    dir: PathBuf,
    inputs: Vec<String>,
    id: u64,
    end_seq: u64,
    lo: Vec<u8>,
    hi: Option<Vec<u8>>,
    opts: SegmentOptions,
    background_io: BackgroundIo,
    sync_every: usize,
    inline_max: usize,
}

/// The piece merge's body: the inputs' keys walked once for their count
/// and once for their values, as the partition merge walks them, into one
/// piece named by a fresh id and the newest input's covered sequence, so
/// it sorts among the range's pieces where that input did. For each key
/// the inputs older than its newest flagged extent are dropped as the
/// partition merge drops them, and the flag is carried when any input
/// held it: the partition below, and pieces older than the inputs, still
/// hold what it masks. A key whose inputs hold nothing live and a
/// tombstone is written as the tombstone alone, which is what a seal
/// writes for a deleted key.
fn tier_run(plan: TierPlan) -> Result<Vec<String>> {
    let TierPlan {
        dir,
        inputs,
        id,
        end_seq,
        lo,
        hi,
        opts,
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
            .map_err(|e| err(&format!("tier input {name}: {e}")))?,
        );
    }
    let mut total = 0usize;
    merge_ranks(&blobs, |_, _| {
        total += 1;
        Ok(())
    })?;
    if total == 0 {
        return Ok(Vec::new());
    }
    let name = format!(
        "pcs-{id:08}-{end_seq:016}-{}-{}.sup",
        hex(&lo),
        hi.as_deref().map(hex).unwrap_or_default()
    );
    let tmp = dir.join(format!("tier-{id:08}.tmp"));
    let mut em = Emitter {
        dir: &dir,
        opts: &opts,
        sync_every,
        inline_max,
        pieces: vec![Piece {
            from: 0,
            to: total,
            lo,
            hi,
            name,
            tmp,
        }],
        pi: 0,
        r: 0,
        w: None,
        out: Vec::new(),
    };
    merge_ranks(&blobs, |k, tied| {
        let mut start = 0usize;
        let mut live = 0u64;
        let mut tomb = false;
        for (j, &(i, rank)) in tied.iter().enumerate() {
            let Some((_, exts)) = blobs[i].exts_at(rank) else {
                return Err(err("segment key walk: a rank the index does not have"));
            };
            if exts.iter().any(|e| e.is_tombstone()) {
                start = j;
                live = 0;
                tomb = true;
            }
            live += exts.iter().map(|e| u64::from(e.records())).sum::<u64>();
        }
        if live == 0 && !tomb {
            return em.skip();
        }
        em.key_with(k, tomb, |w| {
            for &(i, rank) in &tied[start..] {
                blobs[i]
                    .values_at(rank, |v| w.value(v))
                    .map_err(|e| err(&format!("tier read: {e}")))?;
            }
            Ok(())
        })
    })?;
    let out = em.finish(total)?;
    File::open(&dir)?.sync_all()?;
    Ok(out)
}

impl Db {
    /// Start a piece merge where a partition's range holds `tier_pieces`
    /// aligned pieces that no partition merge holds as inputs, one range
    /// at a time; collect a finished one first, since the pieces it
    /// replaced are what the decision counts.
    fn maybe_tier(&mut self) -> Result<()> {
        let n = self.opts.tier_pieces;
        if n == 0 {
            return Ok(());
        }
        if let Some((_, h)) = &self.tiering {
            if !h.is_finished() {
                return Ok(());
            }
            self.join_tier()?;
        }
        let busy: Vec<String> = self
            .compacting
            .as_ref()
            .map(|(i, _)| i.clone())
            .unwrap_or_default();
        let parts: Vec<Fence> = self
            .segs()
            .iter()
            .filter(|s| s.level > 0)
            .map(|s| (s.lo.clone(), s.hi.clone()))
            .collect();
        for (lo, hi) in parts {
            let pieces: Vec<String> = self
                .segs()
                .iter()
                .filter(|s| s.level == 0 && s.lo == lo && s.hi == hi && !busy.contains(&s.name))
                .map(|s| s.name.clone())
                .collect();
            if pieces.len() >= n {
                return self.start_tier(lo, hi, pieces);
            }
        }
        Ok(())
    }

    /// `inputs` are the range's pieces oldest first, as the live set
    /// orders them; the output takes the newest one's covered sequence.
    fn start_tier(&mut self, lo: Vec<u8>, hi: Option<Vec<u8>>, inputs: Vec<String>) -> Result<()> {
        let end_seq = inputs
            .iter()
            .filter_map(|n| Db::name_end_seq(n))
            .max()
            .unwrap_or(self.covered_seq);
        let id = self.next_seg;
        self.next_seg += 1;
        let plan = TierPlan {
            dir: self.dir.clone(),
            inputs: inputs.clone(),
            id,
            end_seq,
            lo,
            hi,
            opts: Db::segment_opts(&self.opts),
            background_io: self.opts.background_io,
            sync_every: self.opts.seal_sync_every,
            inline_max: self.opts.inline_bytes,
        };
        let handle = std::thread::spawn(move || tier_run(plan));
        self.tiering = Some((inputs, handle));
        Ok(())
    }

    /// Collect a piece merge as a partition merge is collected: the
    /// output in for the inputs, the manifest naming it, then the inputs
    /// deleted; a crash on either side of the manifest leaves one
    /// complete set and an orphan the open sweeps.
    fn join_tier(&mut self) -> Result<()> {
        let Some((inputs, handle)) = self.tiering.take() else {
            return Ok(());
        };
        let outputs = handle.join().map_err(|_| err("tier thread panicked"))??;
        let mut merged: Vec<std::sync::Arc<Seg>> = self
            .segs()
            .iter()
            .filter(|seg| !inputs.contains(&seg.name))
            .cloned()
            .collect();
        for name in &outputs {
            merged.push(std::sync::Arc::new(Seg::open(
                &self.dir,
                name,
                self.advice_random(),
                self.opts.read_advice != ReadAdvice::Normal,
                self.opts.segment.checksums,
            )?));
        }
        self.publish_segs_with(merged, true);
        if self.opts.scan_block_cache {
            self.build_ctx().rank_pieces()?;
        }
        self.publish()?;
        for name in &inputs {
            self.retire_seg(name);
        }
        Ok(())
    }
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
    /// Manifests written: every publish of the live set.
    pub publishes: u64,
}

/// PROTOTYPE: records of one partition block a scan reads over unsealed
/// keys. Both the block's own records and the unsealed keys that fall in
/// its key range, each key once with its live values, in order, so a scan
/// over the block walks this and merges nothing.
#[derive(Default, Clone)]
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
        select_lower_bound(self.ents.len(), |i| self.key(&self.ents[i]) < from)
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
        // Four lines a step: the loop's compare and add per line were
        // twice the prefetch itself, over the sixty-four lines of a
        // block walk's span, on every block a scan crosses.
        // SAFETY: a prefetch is a hint that faults on no address, and
        // every address here is inside the buffer.
        let mut off = 0usize;
        while off + 256 <= bytes {
            unsafe {
                _mm_prefetch(ptr.add(off) as *const i8, _MM_HINT_T0);
                _mm_prefetch(ptr.add(off + 64) as *const i8, _MM_HINT_T0);
                _mm_prefetch(ptr.add(off + 128) as *const i8, _MM_HINT_T0);
                _mm_prefetch(ptr.add(off + 192) as *const i8, _MM_HINT_T0);
            }
            off += 256;
        }
        while off < bytes {
            unsafe { _mm_prefetch(ptr.add(off) as *const i8, _MM_HINT_T0) };
            off += 64;
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (ptr, bytes);
}

/// The first index in `0..n` for which `below` is false, or `n`, given
/// `below` true for a prefix: `partition_point`, with each step a mask
/// and not a branch. Written as a select the compiler turned it back
/// into a branch, and a mask it cannot; measured on the probe's E at a
/// hundred thousand keys and three hundred, six rounds interleaved, it
/// moved nothing the machine's own predictor was not already hiding,
/// and it stays because the ordered index's doc says the top search
/// selects, which it now does.
#[inline(always)]
pub(crate) fn select_lower_bound(n: usize, below: impl Fn(usize) -> bool) -> usize {
    let mut lo = 0usize;
    let mut len = n;
    while len > 1 {
        let half = len / 2;
        lo += half & usize::from(below(lo + half - 1)).wrapping_neg();
        len -= half;
    }
    lo + usize::from(len == 1 && below(lo))
}

/// PROTOTYPE: the lines a walk of block `b` from rank `from` for `n`
/// entries touches first, by the block's form: a copy's or a sparse
/// block's buffers, and the partition's records a sparse or clean walk
/// streams. Nothing is waited for, and a block not yet built has nothing
/// to fetch.
fn prefetch_block(blob: &Blob<MmapBytes>, form: &Cached, from: usize, n: usize) {
    match form {
        Cached::Block(blk) => blk.prefetch(),
        Cached::Sparse(sb) => {
            sb.prefetch();
            blob.prefetch_ranks(from, n);
        }
        Cached::Clean => blob.prefetch_ranks(from, n),
        Cached::Wide(_) => {}
    }
}

/// PROTOTYPE: what the cache knows about a block.
#[derive(Clone)]
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
#[derive(Default, Clone)]
struct WideBlock {
    sorted: Vec<(u32, u32)>,
    seen: usize,
    /// PROTOTYPE: wide by the covered rule rather than by its count of
    /// keys, so a second touch copies it; and the touches so far.
    covered: bool,
    walks: u32,
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
#[derive(Default, Clone)]
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
#[derive(Clone, Copy)]
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
    /// The snapshot whose runs the keys' `SnapKey`s name, and the slots
    /// written since its copy, whose runs are not read.
    snap: &'a Snapshot,
    stale: &'a std::collections::HashSet<u32>,
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
    /// Each block's form, shared with every handle that took it from the
    /// forms table (`SharedForms`) and made this handle's own by a
    /// copy-on-write the first time a settle touches it.
    slots: Vec<Option<std::sync::Arc<Cached>>>,
    /// EXPERIMENT: the same block held a second way, as a merged copy,
    /// for as long as the reads pay for it. A read walks this when it is
    /// here and the cheap form when it is not, so the block is two
    /// shapes at once and the choice is the read's; see
    /// `Options::promote_entries`.
    dense: Vec<Option<std::sync::Arc<Cached>>>,
    /// EXPERIMENT: entries reads have taken from each block since it was
    /// last written, which is what pays for a copy.
    reads: Vec<u32>,
    /// The scan count when each block was last walked.
    touched: Vec<u32>,
    /// Each block's index in `Db::built`, or `u32::MAX` when unlisted.
    listed: Vec<u32>,
    /// How many keys the per-block lists hold, so a partition with no
    /// piece meeting it, no snapshot key in its range and nothing filed
    /// is known to be clean throughout without a look at any block.
    filed: usize,
    /// The partition's last key, so an install can tell that keys written
    /// past it fall in the last block alone without reading the index.
    last_key: Vec<u8>,
    /// Per level-0 piece meeting the partition's range: its index in the
    /// level, and the first rank not below each block's lower bound, one
    /// more for the partition's upper fence, so block `b` holds the
    /// piece's ranks `at[b]..at[b + 1]`.
    pieces: PieceBounds,
    /// Per entry of `pieces`, the piece's ranks against this partition,
    /// taken when the table was made, or none when it has none against
    /// it and the cut is searched for instead.
    piece_ranks: Vec<Option<std::sync::Arc<Vec<u32>>>>,
    /// The same over the snapshot of unsealed keys, for the snapshot
    /// `snap_gen` names; walked again when a rebuild replaces it. Taken
    /// at the first build that needs it and not when the table is made:
    /// a handle over complete canonical forms builds nothing, and the
    /// walk of the run against every boundary was the whole of its
    /// first scan's setup -- 16 us at ten thousand keys, 47 at thirty,
    /// a tenth of the threaded scan pass after the mixes at both.
    snap_at: std::cell::OnceCell<std::sync::Arc<SnapBounds>>,
    /// The snapshot's positions at the partition's two fences, which is
    /// what `clean_throughout` asks of the bounds: two searches.
    snap_span: (u32, u32),
    snap_gen: u64,
    /// Live slots created since that snapshot, under the block their key
    /// falls in, in creation order, each with the cut the write's seek
    /// found.
    added: Vec<Vec<(u32, u32)>>,
    /// EXPERIMENT: blocks built or patched since the last publish.
    dirty: Vec<bool>,
}

impl BlockTable {
    /// PROTOTYPE: nothing above the partition anywhere in its range: no
    /// piece meets it, the snapshot's run over it is empty, and nothing
    /// was filed since. A scan then walks the partition's records as the
    /// bulk walk does, with no block touched.
    fn clean_throughout(&self) -> bool {
        self.pieces.is_empty() && self.filed == 0 && self.snap_span.0 == self.snap_span.1
    }

    /// The snapshot's bounds, walked now if this table has not yet.
    fn snap_at(&self, seg: &Seg, unsealed: &Snapshot) -> Result<&SnapBounds> {
        if let Some(at) = self.snap_at.get() {
            return Ok(at);
        }
        let at = BuildCtx::snap_bounds(seg, self.slots.len(), unsealed)?;
        let _ = self.snap_at.set(at);
        Ok(self.snap_at.get().expect("just set"))
    }

    /// The bounds dropped for a snapshot that replaced the one they
    /// were walked over: the span taken again, the bounds at the next
    /// build.
    fn resnap(&mut self, seg: &Seg, unsealed: &Snapshot, gen: u64) {
        self.snap_at = std::cell::OnceCell::new();
        self.snap_span = BuildCtx::snap_span(seg, unsealed);
        self.snap_gen = gen;
    }
}

/// PROTOTYPE: where a sorted source's positions fall against a
/// partition's block boundaries: the first position not below each
/// block's lower bound, and last the first not below the partition's
/// upper fence, so block `b` holds positions `at[b]..at[b + 1]`. The two
/// fences are answered by `seek`; between them the source is walked with
/// `advance`, the first position at or past the given one not below a
/// boundary key, reading each boundary key of the partition once. An open
/// fence is the source's start or end, never a compare against an empty
/// slice.
fn block_bounds_of(
    seg: &Seg,
    nblocks: usize,
    len: usize,
    seek: impl Fn(&[u8]) -> usize,
    advance: impl Fn(usize, &[u8]) -> usize,
) -> Result<Vec<u32>> {
    let mut at = Vec::with_capacity(nblocks + 1);
    let mut i = if seg.lo.is_empty() { 0 } else { seek(&seg.lo) };
    at.push(i as u32);
    let mut kbuf = Vec::new();
    for b in 1..nblocks {
        // Past the source's end no bound moves, and the boundary key is
        // read only to move one: reading it anyway was a cold line per
        // block for a source with no keys, which is what a scan's first
        // table over a store just flushed makes over the snapshot of
        // unsealed keys -- 500 us of the first scan at 300k, a tenth of
        // the suite's scan pass, and the builder ahead read the same
        // lines again on its thread.
        if i < len {
            let rank = b * CACHE_BLOCK;
            // The boundary key from the ordered index's top level where a
            // head is a whole key, and from the record where it is not:
            // the record is a cold line per block for every source the
            // table maps, the top level is in memory.
            let bound = match seg.ord.whole_key_at(rank, &mut kbuf) {
                Some(k) => k,
                None => seg
                    .blob
                    .key_at(rank)
                    .ok_or_else(|| err("block cache: a rank did not resolve"))?,
            };
            i = advance(i, bound).min(len);
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
    /// Whether any live segment holds a tombstone, found once here: a
    /// read asked it of every segment, forty-one pointer chases a read
    /// at thirty million keys.
    segs_tombs: bool,
    /// EXPERIMENT: the canonical block forms of this state, a slot per
    /// block of every partition, null where none is installed; see
    /// `CanonicalForm`.
    forms: Vec<Box<[AtomicPtr<CanonicalForm>]>>,
    /// EXPERIMENT: whether the forms were carried into the state that
    /// replaced this one, which owns them from then on: a retired state
    /// whose forms moved frees none of them.
    forms_moved: std::sync::atomic::AtomicBool,
    /// EXPERIMENT: this state's sorted unsealed keys, built by whichever
    /// handle wanted them first and adopted by the rest. A handle that
    /// builds its own sorts every unsealed key: at a hundred thousand
    /// keys each updated once, 9 ms, and with the organiser running, the
    /// builder's thread and the reading thread sorted the same 63,242
    /// keys inside one pass. The pointer is an `Arc` this state owns, so
    /// a handle adopts by cloning it under its pin; a swap retires the
    /// reference the state gives up past every reader pinned before it,
    /// since a handle can be between the load and the clone. It only
    /// ever moves forward, to a snapshot over more of the memtable.
    snap: AtomicPtr<Snapshot>,
    /// Scans over this state by handles the caller made -- not the
    /// writer's own and not the builder's. The regime the canonical
    /// forms are maintained under: the structure costs the writer at
    /// every commit and is read by whoever holds a handle, so it pays
    /// where other handles read this state and loses where the writer
    /// reads its own store. Asking whether a handle is live at the
    /// instant of a commit is the wrong question and was tried -- a
    /// workload that writes in one phase and reads through handles in
    /// another answers no at every commit, and the threaded scan mix
    /// measured 1.537x for the arm that maintains regardless. A scan is
    /// the evidence, and it is kept for the state's life, so a seal or a
    /// merge would ask again -- so a publish carries it forward. It does
    /// not need to decay: `maintain_forms` already returns where nothing
    /// has been scanned since the last commit, so a store whose readers
    /// have gone quiet stops paying without this having to forget. Kept
    /// per state and not carried, the threaded scan mix at three hundred
    /// thousand keys measured 1.49x for the arm that maintains
    /// regardless, because a seal there lands often enough that the
    /// writer commits several times before a handle reads again.
    reader_scans: AtomicU64,
    /// EXPERIMENT: the write log position every canonical form is
    /// current to, `usize::MAX` while the table is not maintained.
    forms_at: AtomicUsize,
    /// EXPERIMENT: whether every block an unsealed key overlays has a
    /// canonical form, so a null slot is a clean block.
    forms_complete: std::sync::atomic::AtomicBool,
    /// EXPERIMENT: scans any handle has made over this state on the
    /// block path. The regime the canonical forms are maintained for is
    /// a store being range-read *now*: the writer maintains them at a
    /// commit only when a scan has happened since the last one, so a
    /// run of writes nobody reads between pays nothing and a mix that
    /// scans between its writes pays at each. Counted by the scan and
    /// read by the writer's commit, so the regime is the store's
    /// behaviour and not a setting. Whether a store was scanned at all
    /// was the first rule, and it kept maintenance on through eight
    /// thousand updates with no scan among them: the suite's A and F
    /// paid 30% and 43% for forms nothing read.
    scans: AtomicU64,
    forms_bytes: AtomicUsize,
}

/// EXPERIMENT: a block form the writer keeps current at every commit,
/// for every reader: the range-read structure written at ingest. A
/// reader at the table's position walks it as one structure and builds
/// nothing; a reader elsewhere builds its own as before. The writer
/// never patches a published form in place: it patches its own copy
/// and swaps that in, retiring the old one against the reader epoch.
struct CanonicalForm {
    form: std::sync::Arc<Cached>,
    bytes: usize,
}

impl Drop for State {
    fn drop(&mut self) {
        // A state is freed past every reader pinned before its
        // retirement, so nothing walks its forms now. Carried into the
        // next state, they are its to free.
        if self.forms_moved.load(AtomicOrdering::Relaxed) {
            self.forms.clear();
        }
        for slot in self.forms.iter().flat_map(|f| f.iter()) {
            let p = slot.load(AtomicOrdering::Relaxed);
            if !p.is_null() {
                // SAFETY: installed by the writer, owned by the state since.
                drop(unsafe { Box::from_raw(p) });
            }
        }
        let p = self.snap.load(AtomicOrdering::Relaxed);
        if !p.is_null() {
            // SAFETY: published into this state, which has owned the
            // reference since and is freed past every reader.
            drop(unsafe { std::sync::Arc::from_raw(p as *const Snapshot) });
        }
    }
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
    /// EXPERIMENT: canonical forms the writer replaced, each with the
    /// epoch it was replaced at, freed once no reader is pinned at or
    /// before that epoch.
    retired_forms: std::sync::Mutex<Vec<(u64, RetiredForm)>>,
    /// EXPERIMENT: snapshots a publish replaced, freed past every pinned
    /// reader as the forms beside them are. Any handle may push here, so
    /// unlike the forms' list this one is contended.
    retired_snaps: std::sync::Mutex<Vec<(u64, RetiredSnap)>>,
    /// EXPERIMENT: scan snapshots built over this store's life, which is
    /// what publishing one is meant to bring down; for a test.
    snap_builds: AtomicU64,
    /// Reads that took a canonical form, over this store's life. The
    /// count beside it on `State` is that state's and is zero again at
    /// every publish, so reading it at the end of a pass says what
    /// happened since the last seal and nothing about the pass: it read
    /// zero where a pass had taken forms all along, and this exists so a
    /// measurement cannot make that mistake.
    form_takes: AtomicU64,
    /// Scans that reached the test for walking the forms, and the ones
    /// that passed it. A mechanism that holds 1,562 forms and is taken
    /// by no read is not slow, it is off, and these say which clause
    /// turns it off.
    canon_tried: AtomicU64,
    canon_hit: AtomicU64,
    rd_scans: AtomicU64,
    rd_blocks: AtomicU64,
    /// Scans over the store through any handle, for the writer to tell
    /// how long ago the last one was; the state's own count restarts at
    /// every publish.
    scans_life: AtomicU64,
    /// Handles the caller made and still holds: who the forms are
    /// published for.
    live_handles: AtomicUsize,
    /// Blocks materialised, by a handle the caller made and by the
    /// engine's own. These were added to size what sharing the block
    /// tables between handles would be worth, the last structure thought
    /// to be built per handle, and the answer is nothing: over a pass at
    /// a hundred thousand keys a caller's handles materialise none at
    /// all, and every one of the 2,801 is the writer's or the builder's.
    /// A reader takes a canonical form or walks a clean block; the
    /// per-handle rebuild went away when the forms went into the state.
    /// They stay because that is worth knowing again after any change
    /// to what a read may adopt.
    blk_reader: AtomicU64,
    blk_engine: AtomicU64,
    /// EXPERIMENT: snapshots carried forward by merging a batch into the
    /// run rather than sorting everything again; the count that should
    /// rise where `snap_builds` stops.
    snap_extends: AtomicU64,
    /// PROTOTYPE: the run keeper's thread, for a publish and a settle
    /// to wake; a commit and a scan wake nothing, since the keeper
    /// polls for those -- see `Reader::keep`. The flush sequence a
    /// settle raises and the keeper answers once the snapshot is
    /// current; and what the keeper has published, for a test: versions
    /// extended, and versions carried across a publish.
    keeper_thread: std::sync::OnceLock<std::thread::Thread>,
    keeper_seq: AtomicU64,
    keeper_done: AtomicU64,
    snap_kept: AtomicU64,
    snap_carried: AtomicU64,
    /// PROTOTYPE: handles that took the published snapshot over their
    /// own under the keeper's rule; for a test.
    snap_switched: AtomicU64,
}

/// EXPERIMENT: a replaced canonical form on its way to being freed. A
/// raw pointer, since a reader that loaded it before the replacement may
/// still be walking it; `Send` because the writer frees it from its own
/// thread once every such reader has left.
struct RetiredForm(*mut CanonicalForm);

/// EXPERIMENT: a snapshot a publish replaced, as `RetiredForm`.
struct RetiredSnap(*const Snapshot);
// SAFETY: see the type's doc; the pointee is never touched through this
// wrapper except to free it past every pinned reader.
unsafe impl Send for RetiredForm {}
// SAFETY: as `RetiredForm`; the pointee is a `Snapshot`, which is `Send`
// and `Sync`, and only the sweep touches it after the swap.
unsafe impl Send for RetiredSnap {}

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
    /// Whether this handle is one the caller asked for, and so one the
    /// maintenance regime counts; the writer's own and the builder's are
    /// the engine's and are not.
    counted: bool,
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
    scan_keys: std::cell::RefCell<Option<(u64, std::sync::Arc<Snapshot>)>>,
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
    pending: std::cell::RefCell<Vec<(u32, u32, bool, u32)>>,
    /// A settled write's resolved run, built here and spliced into the
    /// block: one buffer for every write of a settle, where a `Vec` built
    /// from empty per write was a malloc, a realloc and a free apiece,
    /// two fifths of a settle's instructions.
    settle_run: std::cell::RefCell<Vec<u8>>,
    /// EXPERIMENT: whether any table holds a block built or patched
    /// since the last publish; the writer's own handle alone sets it.
    dirty_any: std::cell::Cell<bool>,
    /// EXPERIMENT: whether every block an unsealed key overlays has a
    /// form in this handle's tables, which the state's `forms_complete`
    /// takes at each publish.
    tables_complete: std::cell::Cell<bool>,
    /// EXPERIMENT: a handle was claimed while writes were staged, so
    /// the next commit publishes whether or not a scan preceded it.
    publish_due: std::cell::Cell<bool>,
    /// EXPERIMENT: the state's scan count at the last commit this handle
    /// maintained the canonical forms at; a commit with the count
    /// unmoved maintains nothing.
    scans_seen: std::cell::Cell<u64>,
    /// The commit -- generation and committed log length -- this handle
    /// last signalled a scan at, so it signals once per commit.
    signalled: std::cell::Cell<(u64, usize)>,
    /// EXPERIMENT: for `forms_settle_recent_pct`: the store's scan count
    /// as this handle last saw it at a commit, the writes it has counted
    /// over the store's life (the log's growth, summed across
    /// generations, from the generation and length it last counted at),
    /// and the count at the last scan it saw.
    scans_life_seen: std::cell::Cell<u64>,
    writes_seen: std::cell::Cell<u64>,
    writes_at_scan: std::cell::Cell<u64>,
    log_counted: std::cell::Cell<(u64, usize)>,
    /// EXPERIMENT: what the reads chose, for a run to report:
    /// promotions, drops by a write, walks over a copy, walks over the
    /// cheap form, and the copies' bytes.
    choices: std::cell::Cell<[u64; 5]>,
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
    /// PROTOTYPE: live slots written since the scan snapshot's runs were
    /// copied, by the write log from the snapshot's `log_at`: a key here
    /// is read from its chain, not its run. See `Snapshot::vals`.
    snap_stale: std::cell::RefCell<std::collections::HashSet<u32>>,
    /// How far into the memtable's write log this handle has looked, and
    /// the generation it looked in: a new generation is a new memtable
    /// or a new segment set, and the log is read from the start again.
    log_seen: std::cell::Cell<usize>,
    log_gen: std::cell::Cell<u64>,
    /// How far into the write log this handle may look: the log's length
    /// at the commit whose watermark it reads under, taken before the
    /// watermark so it never runs ahead of it, or unbounded under `Dirty`
    /// and for the writer's own handle. A handle that read the log to
    /// its end settled a key's uncommitted write under the committed
    /// watermark, leaving the value out as it must, and when the commit
    /// landed the log had not moved, so the block kept the old run for
    /// as long as it was cached; the point read beside it, through the
    /// memtable, answered the new value.
    log_bound: std::cell::Cell<usize>,
    /// How many of the live memtable's entries the scan snapshot covers:
    /// a key the log says was created at a number below it is in the
    /// snapshot already, not a key to file.
    snap_entries: std::cell::Cell<usize>,
    /// PROTOTYPE: the builder ahead of the reader, while one is running
    /// or has forms still to install. The writer's handle alone has one.
    ahead: std::cell::RefCell<Option<Ahead>>,
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
    /// EXPERIMENT: the memtable's entry count when the last builder was
    /// started from a commit, so the next starts a burst later and not a
    /// batch later, and the state's generation then, since a publish
    /// stops the builder and retires what it had organised.
    built_ahead_len: usize,
    built_ahead_gen: u64,
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
    /// A piece merge in flight: its inputs' names and its thread. Its
    /// inputs and a partition merge's are disjoint, each excluding the
    /// other's at its start, so the two run beside each other.
    tiering: Option<Compaction>,
    sealing: Option<std::thread::JoinHandle<Result<Vec<String>>>>,
    /// PROTOTYPE: the run keeper, once the first commit has started it.
    keeper: Option<Keeper>,
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
            if self.counted {
                self.shared
                    .live_handles
                    .fetch_sub(1, AtomicOrdering::Relaxed);
            }
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
    /// The key's live and frozen chains copied into `Snapshot::vals` when
    /// the snapshot was built, or `NO_RUN`: a filed key carries none, and
    /// a key written again since the copy is read from its chain.
    lrun: u32,
    frun: u32,
}

/// No copied run for a snapshot key.
const NO_RUN: u32 = u32::MAX;

/// An overlay assembled from a chain alone names no snapshot; this one
/// stands in, and nothing reads a run from it.
static NO_SNAPSHOT: std::sync::LazyLock<Snapshot> = std::sync::LazyLock::new(Snapshot::default);

/// PROTOTYPE: the arena a snapshot's key bytes and copied runs live in,
/// shared by every version of one snapshot and appended to by whichever
/// thread extends or files into one: the run keeper on its own thread, a
/// handle carrying a published snapshot forward at a scan, a handle
/// filing its own keys. A version names bytes by offset, and an offset
/// once written never moves or changes, so a version is a set of entries
/// over an arena that outlives it -- which is what lets an extension
/// append the batch alone where the version before it copied every key
/// and every run of the base into a fresh pair of vectors first, 1.4 MB
/// at ten thousand keys, for every batch.
///
/// Blocks double from a base sized at the build, so the block an offset
/// falls in is arithmetic and not a search. A reservation is one
/// fetch-add on the tail, retried past a block's end, and the first
/// reservation into a block allocates it: no thread waits on another,
/// which matters because the keeper runs at idle priority and a lock it
/// held while descheduled would hold a scan at that priority too. Bytes
/// are read only through an entry published after they were written.
struct SnapArena {
    /// Block `b` holds `1 << (base_shift + b)` bytes and starts at
    /// offset `((1 << b) - 1) << base_shift`.
    base_shift: u32,
    blocks: [AtomicPtr<u8>; SNAP_BLOCKS],
    tail: AtomicUsize,
}

const SNAP_BLOCKS: usize = 32;
/// The smallest base block, for a snapshot over a handful of keys.
const SNAP_BASE_SHIFT: u32 = 12;

impl Default for SnapArena {
    fn default() -> SnapArena {
        SnapArena::with_capacity(0)
    }
}

impl SnapArena {
    /// An arena whose first block holds `bytes`, so a build of that many
    /// allocates once.
    fn with_capacity(bytes: usize) -> SnapArena {
        let base_shift = bytes
            .next_power_of_two()
            .trailing_zeros()
            .max(SNAP_BASE_SHIFT);
        SnapArena {
            base_shift,
            blocks: std::array::from_fn(|_| AtomicPtr::new(std::ptr::null_mut())),
            tail: AtomicUsize::new(0),
        }
    }

    /// The block an offset falls in, and the offset within it.
    #[inline]
    fn locate(&self, off: usize) -> (usize, usize) {
        let q = (off >> self.base_shift) + 1;
        let b = (usize::BITS - 1 - q.leading_zeros()) as usize;
        let start = ((1usize << b) - 1) << self.base_shift;
        (b, off - start)
    }

    #[inline]
    fn cap(&self, b: usize) -> usize {
        1usize << (self.base_shift + b as u32)
    }

    /// `n` contiguous bytes, at the offset returned. Any thread.
    fn reserve(&self, n: usize) -> u32 {
        loop {
            let off = self.tail.fetch_add(n, AtomicOrdering::Relaxed);
            assert!(
                off + n <= u32::MAX as usize,
                "snapshot arena: more bytes than it addresses"
            );
            let (b, w) = self.locate(off);
            let cap = self.cap(b);
            if w + n > cap {
                // Past this block's end: what is left of the block is
                // left, and the next try lands in the one after.
                continue;
            }
            assert!(b < SNAP_BLOCKS, "snapshot arena: more blocks than it has");
            if self.blocks[b].load(AtomicOrdering::Acquire).is_null() {
                let layout =
                    std::alloc::Layout::from_size_align(cap, 64).expect("arena block layout");
                // Not zeroed: a byte is read only past a write that
                // covered it. SAFETY: a non-zero layout; the block is
                // freed by `Drop`, or below when another thread's won.
                let p = unsafe { std::alloc::alloc(layout) };
                assert!(!p.is_null(), "snapshot arena: out of memory");
                if self.blocks[b]
                    .compare_exchange(
                        std::ptr::null_mut(),
                        p,
                        AtomicOrdering::AcqRel,
                        AtomicOrdering::Acquire,
                    )
                    .is_err()
                {
                    // SAFETY: allocated just above with this layout, and
                    // published nowhere.
                    unsafe { std::alloc::dealloc(p, layout) };
                }
            }
            return off as u32;
        }
    }

    fn block(&self, off: usize, len: usize) -> (*mut u8, usize) {
        let (b, w) = self.locate(off);
        let p = self.blocks[b].load(AtomicOrdering::Acquire);
        assert!(
            !p.is_null(),
            "snapshot arena: an offset past what was written"
        );
        assert!(
            w + len <= self.cap(b),
            "snapshot arena: a read past its block"
        );
        (p, w)
    }

    /// `bytes` into a reserved range at `off`.
    fn write(&self, off: u32, bytes: &[u8]) {
        let (p, w) = self.block(off as usize, bytes.len());
        // SAFETY: inside the block, a range this thread reserved and no
        // reader can reach until an entry naming it is published.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(w), bytes.len()) };
    }

    fn append(&self, bytes: &[u8]) -> u32 {
        let off = self.reserve(bytes.len());
        self.write(off, bytes);
        off
    }

    /// Published bytes at `off`.
    #[inline]
    fn slice(&self, off: u32, len: u32) -> &[u8] {
        let (p, w) = self.block(off as usize, len as usize);
        // SAFETY: inside a block that lives as long as the arena, bytes
        // written before the entry that names them was published.
        unsafe { std::slice::from_raw_parts(p.add(w), len as usize) }
    }

    /// Bytes reserved so far, the left-over tails of blocks included.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.tail.load(AtomicOrdering::Relaxed)
    }

    /// PROTOTYPE: the whole chain of `e` appended, oldest chunk first,
    /// as a count and then each chunk's memtable offset, its length and,
    /// for a value, its bytes, in one contiguous reservation. Returns
    /// where the run starts.
    fn copy_run(&self, mem: &MemTable, e: &MemEntry, scratch: &mut Vec<(u64, u32)>) -> u32 {
        scratch.clear();
        let mut at = MemTable::head(e);
        let mut bytes = 4usize;
        while at != NO_CHUNK {
            let len = mem.chunk_len(at);
            bytes += 12 + if len == TOMB_LEN { 0 } else { len as usize };
            scratch.push((at, len));
            at = mem.chunk_prev(at);
        }
        let off = self.reserve(bytes);
        let mut p = off;
        self.write(p, &(scratch.len() as u32).to_le_bytes());
        p += 4;
        for &(at, len) in scratch.iter().rev() {
            self.write(p, &at.to_le_bytes());
            self.write(p + 8, &len.to_le_bytes());
            p += 12;
            if len != TOMB_LEN {
                self.write(p, mem.value_at(at as usize));
                p += len;
            }
        }
        off
    }

    /// The bytes a run at `off` takes.
    fn run_len(&self, off: u32) -> u32 {
        let n = self.word(off);
        let mut p = off + 4;
        for _ in 0..n {
            let len = self.word(p + 8);
            p += 12 + if len == TOMB_LEN { 0 } else { len };
        }
        p - off
    }

    fn word(&self, off: u32) -> u32 {
        u32::from_le_bytes(self.slice(off, 4).try_into().expect("four bytes"))
    }

    fn wide(&self, off: u32) -> u64 {
        u64::from_le_bytes(self.slice(off, 8).try_into().expect("eight bytes"))
    }
}

impl Drop for SnapArena {
    fn drop(&mut self) {
        for (b, slot) in self.blocks.iter().enumerate() {
            let p = slot.load(AtomicOrdering::Relaxed);
            if !p.is_null() {
                let layout = std::alloc::Layout::from_size_align(self.cap(b), 64)
                    .expect("arena block layout");
                // SAFETY: allocated by `reserve` with this layout.
                unsafe { std::alloc::dealloc(p, layout) };
            }
        }
    }
}

/// The sorted keys of the unsealed sources, built lazily by `Db::scan` and
/// kept until the next commit or seal. Keys live in one arena rather than
/// one allocation each, which is what makes the build a sort of small
/// records instead of a pointer chase.
#[derive(Clone, Default)]
struct Snapshot {
    /// The keys' bytes, and the runs. PROTOTYPE, the runs: each key's
    /// whole chain from the live table and from the frozen one, copied
    /// here when the snapshot is built or extended -- oldest chunk
    /// first, each with the arena offset it has in the memtable and its
    /// length, a tombstone by `TOMB_LEN` -- so a read of an overlaid key
    /// streams the copy instead of chasing the entry, the chain and the
    /// value through the memtable's own layout, two or three dependent
    /// misses a key that were a third of a block's build and of a wide
    /// walk on the store the lag sweep's last point leaves. A reader
    /// under a watermark honours it here as it does on the chain, by
    /// the chunk offsets; a key written again after the copy is found
    /// through the handle's stale set, fed from the write log past
    /// `log_at`, and read from the chain as before.
    arena: std::sync::Arc<SnapArena>,
    /// PROTOTYPE: whether the runs were copied at all; a filed key has
    /// none either way.
    runs: bool,
    ents: Vec<SnapKey>,
    /// The write log's length when the runs were copied: every log entry
    /// from here on may have moved a chain past its copy.
    log_at: usize,
    /// The live memtable prefix `ents` was built over, which with the
    /// frozen table is everything it holds. A handle that adopts this
    /// snapshot rather than building its own reads it to know which of
    /// its logged slots the snapshot already has.
    live_len: usize,
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
    /// Where the main run's positions fall against a partition's block
    /// boundaries, by the partition's blob id. The main run is fixed
    /// once built -- a filing goes to the side runs -- so the bounds
    /// are a function of the run and the partition, taken once and
    /// shared: by the writer's table and every handle that adopts the
    /// snapshot, and across the copy a filing makes of a shared one,
    /// whose run is the same. A handle's first scan after the mixes
    /// walked the run against every boundary for 280 us at three
    /// hundred thousand keys.
    bounds: std::sync::Arc<std::sync::RwLock<SnapById>>,
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
        self.arena.slice(e.off, e.len)
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
                        lrun: if e.mem != u32::MAX { e.lrun } else { be.lrun },
                        frozen: if e.frozen != u32::MAX {
                            e.frozen
                        } else {
                            be.frozen
                        },
                        frun: if e.frozen != u32::MAX {
                            e.frun
                        } else {
                            be.frun
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
            batch.push(SnapKey {
                off: self.arena.append(key),
                len: key.len() as u32,
                mem: slot,
                frozen: u32::MAX,
                lrun: NO_RUN,
                frun: NO_RUN,
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
    /// This snapshot with the live slots from where it stops up to `to`
    /// folded in: their keys and runs appended to the shared arena, that
    /// batch sorted on its own, the two runs merged by a search and a
    /// block copy per batch key, and every run written again since this
    /// snapshot's copy -- the write log from `log_at` to the mark taken
    /// here names their slots -- copied again from its chain as it
    /// stands now. A committed batch cannot change, so its order is
    /// settled once and the run it joins never has to be sorted again --
    /// the build it replaces sorts every unsealed key afresh, which at
    /// 63,242 of them is 9 ms, and the organiser paid that fifteen times
    /// over one burst of writes. Only for a run with nothing filed into
    /// it: a handle's side runs are that handle's own and are not what
    /// gets published.
    fn extend(&self, mem: &MemTable, to: usize, runs: bool) -> Snapshot {
        let from = self.live_len;
        // The log's length first, then the chains: a write that lands
        // between is logged past this mark and read from its chain.
        let log_at = mem.log_len();
        let arena = self.arena.clone();
        let mut rscratch: Vec<(u64, u32)> = Vec::new();
        let mut batch: Vec<SnapKey> = Vec::with_capacity(to - from);
        for i in from..to {
            let e = mem.entry(i);
            let key = mem.key_of(e);
            let lrun = if runs {
                arena.copy_run(mem, e, &mut rscratch)
            } else {
                NO_RUN
            };
            batch.push(SnapKey {
                off: arena.append(key),
                len: key.len() as u32,
                mem: i as u32,
                frozen: u32::MAX,
                lrun,
                frun: NO_RUN,
            });
        }
        let key_at = |e: &SnapKey| arena.slice(e.off, e.len);
        batch.sort_unstable_by(|a, b| key_at(a).cmp(key_at(b)));
        let mut out = Snapshot {
            arena: self.arena.clone(),
            runs,
            ents: Vec::with_capacity(self.ents.len() + batch.len()),
            log_at,
            live_len: to,
            side: Vec::new(),
            fresh: Vec::new(),
            filed: 0,
            bounds: Default::default(),
        };
        // The run first where the keys are equal, as the build pushes
        // the frozen table's entries before the live ones: `push_sorted`
        // folds the batch's slot onto the run's entry, whose bytes are
        // the same. Only two can meet: the run holds one entry a key and
        // the memtable gives a key one slot, so a slot past `from` is a
        // key the run has from the frozen table alone. The run between
        // two batch keys is sorted and folded already, and is copied
        // whole: the merge that compared every pair was a compare an
        // entry for a batch of a hundred.
        let mut i = 0usize;
        for b in &batch {
            let key = key_at(b);
            let at = i + self.ents[i..].partition_point(|a| self.key_of(a) <= key);
            out.ents.extend_from_slice(&self.ents[i..at]);
            i = at;
            out.push_sorted(*b);
        }
        out.ents.extend_from_slice(&self.ents[i..]);
        // A run written again since this snapshot copied it, found by
        // its key; a slot in the batch was copied above. The set is the
        // log's, not a handle's: a handle's stale set is relative to the
        // snapshot it holds, and the base here may be one it adopted.
        if runs {
            let mut again: Vec<u32> = (self.log_at..log_at)
                .map(|i| mem.log_at(i).0 as u32)
                .filter(|&id| (id as usize) < from)
                .collect();
            again.sort_unstable();
            again.dedup();
            for id in again {
                let e = mem.entry(id as usize);
                let key = mem.key_of(e);
                let at = out.ents.partition_point(|a| key_at(a) < key);
                if out
                    .ents
                    .get(at)
                    .is_some_and(|a| a.mem == id && a.lrun != NO_RUN)
                {
                    out.ents[at].lrun = arena.copy_run(mem, e, &mut rscratch);
                }
            }
        }
        out
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
            .map(|e| (self.arena.slice(e.off, e.len), e))
    }
    /// The first index at or past `from` whose key is not below `key`, or
    /// the length: a table's walk over the main run against a partition's
    /// block boundaries, a few keys per boundary.
    fn advance_below(&self, from: usize, key: &[u8]) -> usize {
        let mut i = from;
        while let Some(e) = self.ents.get(i) {
            if self.arena.slice(e.off, e.len) >= key {
                break;
            }
            i += 1;
        }
        i
    }
    /// First index whose key is not below `from`.
    ///
    /// The ends first: a start below every unsealed key -- every scan of a
    /// store whose inserts land past its loaded range, the YCSB shape -- is
    /// answered by one compare instead of a binary search over the snapshot,
    /// and a start above them all by two. A start inside the range pays
    /// those two compares on top of the search.
    fn seek(&self, from: &[u8]) -> usize {
        let key = |e: &SnapKey| self.arena.slice(e.off, e.len);
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
            let same = self.arena.slice(last.off, last.len) == self.arena.slice(e.off, e.len);
            if same {
                if e.mem != u32::MAX {
                    last.mem = e.mem;
                    last.lrun = e.lrun;
                }
                if e.frozen != u32::MAX {
                    last.frozen = e.frozen;
                    last.frun = e.frun;
                }
                return;
            }
        }
        self.ents.push(e);
    }

    fn copy_run(&self, mem: &MemTable, e: &MemEntry, scratch: &mut Vec<(u64, u32)>) -> u32 {
        self.arena.copy_run(mem, e, scratch)
    }

    /// PROTOTYPE: the run at `off` as a read under `wm` sees it, the way
    /// `MemTable::live_offs_into` sees a chain: chunks at or past the
    /// mark are not there, and everything older than the newest visible
    /// tombstone is dead. Returns the arena range of the live values'
    /// records and whether a visible tombstone was met.
    fn run_visible(&self, off: u32, wm: u64) -> (std::ops::Range<u32>, bool) {
        let v = &self.arena;
        let n = v.word(off);
        let mut p = off + 4;
        let mut start = p;
        let mut tomb = false;
        for _ in 0..n {
            let at = v.wide(p);
            if at >= wm {
                break;
            }
            let len = v.word(p + 8);
            p += 12;
            if len == TOMB_LEN {
                tomb = true;
                start = p;
            } else {
                p += len;
            }
        }
        (start..p, tomb)
    }

    fn run_values<F: FnMut(&[u8])>(&self, off: u32, wm: u64, mut f: F) {
        let (range, _) = self.run_visible(off, wm);
        let v = &self.arena;
        let mut p = range.start;
        while p < range.end {
            let len = v.word(p + 8);
            p += 12;
            f(v.slice(p, len));
            p += len;
        }
    }

    fn run_has_tomb(&self, off: u32, wm: u64) -> bool {
        self.run_visible(off, wm).1
    }
}

/// LSD radix sort of `(key, a, b)` triples by the key, a byte a pass over
/// the bytes the largest key has, with the counters on the stack. Stable,
/// O(n), and what puts a hash table's entries back into the order their
/// keys were appended so the copy that follows is sequential. It was two
/// 16-bit passes, each over a histogram of 65,536 words zeroed for the
/// call: 512 KB of fresh pages twice, whose faults were the first scan
/// of every store, 400 us at ten thousand keys and at a hundred thousand,
/// over a memtable that held nothing.
fn radix_by_first(v: &mut Vec<(u32, u32, u32)>, scratch: &mut Vec<(u32, u32, u32)>) {
    let max = v.iter().map(|t| t.0).max().unwrap_or(0);
    scratch.clear();
    scratch.resize(v.len(), (0, 0, 0));
    let mut shift = 0u32;
    while shift < 32 && (max >> shift) != 0 {
        let mut counts = [0usize; 256];
        for &(k, _, _) in v.iter() {
            counts[((k >> shift) & 0xFF) as usize] += 1;
        }
        let mut sum = 0usize;
        for c in counts.iter_mut() {
            let n = *c;
            *c = sum;
            sum += n;
        }
        for &t in v.iter() {
            let b = ((t.0 >> shift) & 0xFF) as usize;
            scratch[counts[b]] = t;
            counts[b] += 1;
        }
        std::mem::swap(v, scratch);
        shift += 8;
    }
}

#[cfg(test)]
mod radix {
    /// The radix pass orders by the first word and keeps the input order
    /// among equal words, over keys of every width up to the full four
    /// bytes, and over nothing.
    #[test]
    fn the_radix_pass_orders_as_a_stable_sort_by_the_first_word_does() {
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for (n, bits) in [
            (0, 32),
            (1, 0),
            (7, 3),
            (1000, 8),
            (5000, 12),
            (20000, 20),
            (3000, 32),
        ] {
            let mask = if bits == 32 {
                u32::MAX
            } else {
                (1u32 << bits) - 1
            };
            let mut v: Vec<(u32, u32, u32)> = (0..n)
                .map(|i| ((rng() as u32) & mask, i as u32, rng() as u32))
                .collect();
            let mut want = v.clone();
            want.sort_by_key(|t| t.0);
            let mut scratch = Vec::new();
            super::radix_by_first(&mut v, &mut scratch);
            assert_eq!(v, want, "{n} keys of {bits} bits");
        }
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
                    let mem = &unsafe { &*p }.mem;
                    // The log's length first: a commit stores it before
                    // the watermark, so a length read first belongs to
                    // the watermark's commit or an earlier one.
                    self.log_bound.set(mem.committed_log());
                    self.wm.set(mem.committed());
                }
                Isolation::Dirty => {
                    self.shared.readers.pin(slot);
                    let p = self.shared.state.load(AtomicOrdering::Acquire);
                    self.held.store(p, AtomicOrdering::Relaxed);
                    self.log_bound.set(usize::MAX);
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
        let mem = &unsafe { &*p }.mem;
        self.log_bound.set(mem.committed_log());
        self.wm.set(mem.committed());
        self.isolation.set(Isolation::Snapshot);
    }

    /// PROTOTYPE: hold the state of generation `gen` under watermark `wm`,
    /// both named by the writer for its builder ahead, as `snapshot`
    /// holds the latest; false, and nothing held, when the state has moved
    /// on already.
    fn pin_at(&self, gen: u64, wm: u64, log: usize) -> bool {
        let Some(slot) = self.slot else { return false };
        if self.isolation.get() == Isolation::Snapshot {
            self.release();
        }
        self.shared.readers.pin(slot);
        let p = self.shared.state.load(AtomicOrdering::Acquire);
        // SAFETY: pinned above, so not freed under this handle.
        if unsafe { &*p }.gen != gen {
            self.shared.readers.unpin(slot);
            return false;
        }
        self.held.store(p, AtomicOrdering::Relaxed);
        self.log_bound.set(log);
        self.wm.set(wm);
        self.isolation.set(Isolation::Snapshot);
        true
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
        self.state().has_tombstones()
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
        self.state().pieces_over(np, at)
    }
}

impl State {
    fn has_tombstones(&self) -> bool {
        self.segs_tombs
            || self.mem.tombs() > 0
            || self.frozen.as_ref().is_some_and(|f| f.tombs() > 0)
    }

    /// The level-0 pieces a read of a key in partition `at` consults.
    fn pieces_over(&self, np: usize, at: usize) -> &[std::sync::Arc<Seg>] {
        let l0 = &self.segs[np..];
        if !self.l0_aligned || at >= np {
            return l0;
        }
        // The first partition's lower fence is empty, and so is that of
        // every piece aligned to it; see `below_lo` for why an empty fence
        // is never handed to a compare.
        let lo = self.segs[at].lo.as_slice();
        let before =
            |s: &std::sync::Arc<Seg>| !lo.is_empty() && (s.lo.is_empty() || s.lo.as_slice() < lo);
        let same = |s: &std::sync::Arc<Seg>| {
            s.lo.is_empty() == lo.is_empty() && (lo.is_empty() || s.lo.as_slice() == lo)
        };
        let from = l0.partition_point(before);
        let to = from + l0[from..].partition_point(same);
        &l0[from..to]
    }
}

/// EXPERIMENT: the smallest a `seal_max_pct` cap may make the seal, and
/// the variable the sweep turned on rather than the share.
///
/// At ten thousand keys the store is about 600 KB, so any floor below it
/// binds. With the floor at 64 KiB a tenth of the store is 64 KiB, the
/// memtable seals on nearly every commit, and ycsb-F reads 0.571x of the
/// uncapped arm (0/15, p=0.000); at 256 KiB, ycsb-E reads 0.792x and the
/// threaded scan mix 0.673x (0/9 and 1/9, p<=0.039). At a megabyte the
/// rung is clean -- every mix within noise -- and keeps a 1.755x on the
/// fully unmerged lag point, while a hundred thousand keys, whose store
/// clears the floor, keeps the whole win. So the floor is what makes the
/// cap a no-op on a store too small to have a lag problem.
const SEAL_CAP_FLOOR: usize = 1 << 20;

impl Reader {
    /// The memtable bytes at which the next commit seals: `seal_bytes`, or
    /// with `seal_grows` the larger of that and the partitions' bytes over
    /// four times `l0_trigger`.
    pub fn seal_threshold(&self) -> usize {
        let base = if !self.opts.seal_grows {
            self.opts.seal_bytes
        } else {
            let grown = self.state().store_bytes / (4 * self.opts.l0_trigger.max(1)) as u64;
            self.opts
                .seal_bytes
                .max(usize::try_from(grown).unwrap_or(usize::MAX))
        };
        if self.opts.seal_max_pct == 0 {
            return base;
        }
        let store = usize::try_from(self.state().store_bytes).unwrap_or(usize::MAX);
        // Nothing sealed yet is nothing to take a share of, and the floor
        // keeps a store of a few kilobytes off a seal a commit.
        match store / 100 * self.opts.seal_max_pct {
            0 => base,
            cap => base.min(cap.max(SEAL_CAP_FLOOR)),
        }
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
        // The state once, and the memtable's hash and slot line only when
        // it has entries: a read over a store just flushed hashed the key
        // and fetched a line of an empty table, and took the state through
        // an accessor at every step, thirty nanoseconds on a read of two
        // hundred at three hundred thousand keys.
        let st = self.state();
        let (segs, mem) = (&st.segs, &*st.mem);
        let mem_empty = mem.is_empty();
        let hash = if mem_empty { 0 } else { mem.prefetch(key) };
        let np = segs.partition_point(|s| s.level > 0);
        let at = segs[..np].partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let part = segs[..np].get(at).filter(|s| s.may_hold(key));
        let l0 = st.pieces_over(np, at);
        // Sources oldest to newest: the partition (0), the level-0 pieces
        // (1..), the frozen memtable, the live one. `start` is the source
        // live values begin at: 0 unless a newer source holds a tombstone
        // for this key. Only a store with tombstones in it checks, and the
        // check is what a delete costs a read -- a second probe on the
        // sources that hold the key.
        let (fr_ix, mem_ix) = (1 + l0.len(), 2 + l0.len());
        let mut start = 0usize;
        if st.has_tombstones() {
            if !mem_empty {
                if let Some(e) = mem.get_with(hash, key) {
                    if mem.has_tomb(e, self.wm()) {
                        start = mem_ix;
                    }
                }
            }
            if start == 0 {
                if let Some(fr) = &st.frozen {
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
            if let Some(fr) = &st.frozen {
                if let Some(e) = fr.get(key) {
                    let (offs, _) = fr.live_chain(e, SEE_ALL);
                    n += offs.len() as u64;
                    for off in offs {
                        f(fr.value_at(off));
                    }
                }
            }
        }
        if mem_ix >= start && !mem_empty {
            if let Some(e) = mem.get_with(hash, key) {
                let (offs, _) = mem.live_chain(e, self.wm());
                n += offs.len() as u64;
                for off in offs {
                    f(mem.value_at(off));
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
        self.shared
            .snap_builds
            .fetch_add(1, AtomicOrdering::Relaxed);
        let n = live_len + self.frozen().as_ref().map_or(0, |f| f.len());
        let runs = self.opts.snapshot_runs;
        // The arena's first block sized to what the build appends: the
        // keys, and with the runs the values and sixteen bytes a chunk.
        let mut bytes =
            self.mem().key_bytes() + self.frozen().as_ref().map_or(0, |f| f.key_bytes());
        if runs {
            bytes += self.mem().value_bytes()
                + self.frozen().as_ref().map_or(0, |f| f.value_bytes())
                + 16 * n;
        }
        let mut snap = Snapshot {
            live_len,
            // The log's length before any chain is read: see `extend`.
            log_at: self.mem().log_len(),
            arena: std::sync::Arc::new(SnapArena::with_capacity(bytes)),
            runs,
            ents: Vec::with_capacity(n),
            side: Vec::new(),
            fresh: Vec::new(),
            filed: 0,
            bounds: Default::default(),
        };
        let mut rscratch: Vec<(u64, u32)> = Vec::new();
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
                    let run = if runs {
                        snap.copy_run(mem, mem.entry(i as usize), &mut rscratch)
                    } else {
                        NO_RUN
                    };
                    pending.push(SnapKey {
                        off: snap.arena.append(k),
                        len: k.len() as u32,
                        mem: if live { i } else { u32::MAX },
                        frozen: if live { u32::MAX } else { i },
                        lrun: if live { run } else { NO_RUN },
                        frun: if live { NO_RUN } else { run },
                    });
                }
            };
            if let Some(fr) = self.frozen() {
                take(fr, false);
            }
            take(self.mem(), true);
            let arena = &snap.arena;
            let key_of = |e: &SnapKey| arena.slice(e.off, e.len);
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
                run: u32,
            }
            let mut all: Vec<Old> = Vec::with_capacity(n);
            let mut take = |mem: &MemTable, live: bool| {
                let upto = if live { live_len } else { mem.len() };
                for (i, e) in (0..upto).map(|i| (i, mem.entry(i))) {
                    all.push(Old {
                        key: mem.key_of(e).to_vec(),
                        mem: if live { i as u32 } else { u32::MAX },
                        frozen: if live { u32::MAX } else { i as u32 },
                        run: if runs {
                            snap.copy_run(mem, e, &mut rscratch)
                        } else {
                            NO_RUN
                        },
                    });
                }
            };
            if let Some(fr) = self.frozen() {
                take(fr, false);
            }
            take(self.mem(), true);
            all.sort_by(|a, b| a.key.cmp(&b.key));
            for o in all {
                snap.push_sorted(SnapKey {
                    off: snap.arena.append(&o.key),
                    len: o.key.len() as u32,
                    mem: o.mem,
                    frozen: o.frozen,
                    lrun: if o.mem != u32::MAX { o.run } else { NO_RUN },
                    frun: if o.frozen != u32::MAX { o.run } else { NO_RUN },
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
        if let (true, Some(slot)) = (self.counted, self.slot) {
            let s = &self.shared.readers.slots[slot];
            s.scans.fetch_add(1, AtomicOrdering::Relaxed);
            if use_cache {
                s.blockpath.fetch_add(1, AtomicOrdering::Relaxed);
            }
        }
        // Nothing written or published since the last scan: nothing to
        // settle, and the snapshot that stood then stands. Every scan paid
        // the checks below, four cell borrows and a snapshot's length,
        // seventy nanoseconds of a scan of a microsecond at 300k keys, on
        // a mix that writes once in twenty operations.
        let moved = self.sync_log() || self.scan_keys.borrow().is_none();
        if use_cache && moved {
            self.settle_pending()?;
        }
        self.refresh_snapshot(gen, use_cache, moved);
        let cache = self.scan_keys.borrow();
        let unsealed: &Snapshot = &cache.as_ref().expect("scan snapshot").1;
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
    fn sync_log(&self) -> bool {
        let st = self.state();
        let mut moved = false;
        if self.log_gen.get() != st.gen {
            moved = true;
            self.log_gen.set(st.gen);
            self.log_seen.set(0);
            // A new state counts its own scans from zero, so the count
            // this handle last maintained at belongs to the old one.
            self.scans_seen.set(0);
            self.snap_entries.set(0);
            *self.scan_keys.borrow_mut() = None;
            self.snap_added.borrow_mut().clear();
            self.snap_stale.borrow_mut().clear();
            self.pending.borrow_mut().clear();
            self.drop_blocks();
            if self.tables.borrow().len() != st.segs.len() {
                *self.tables.borrow_mut() = Db::tables_for(st.segs.len());
            }
        }
        let mem = &st.mem;
        let n = mem.log_len().min(self.log_bound.get());
        let seen = self.log_seen.get();
        if seen >= n {
            return moved;
        }
        let file = self.cache_used.get();
        // A handle with no snapshot and no tables has nothing to file: the
        // snapshot it builds or adopts next covers every entry there is,
        // and the keys created since it is exactly what this loop lists.
        // Read in full, the log's two thousand entries at ten thousand
        // keys were twenty microseconds of a handle's first scan.
        if !file && self.scan_keys.borrow().is_none() && self.ahead.borrow().is_none() {
            self.log_seen.set(n);
            return true;
        }
        let covered = self.snap_entries.get();
        let mut added = self.snap_added.borrow_mut();
        let mut pending = self.pending.borrow_mut();
        let ahead = self.ahead.borrow();
        let mut since = ahead
            .as_ref()
            .filter(|a| !a.done.get())
            .map(|a| (a.from, a.since.borrow_mut()));
        // Every write logged past the snapshot's copy may have moved a
        // chain past its run: the slot is read from the chain from here.
        let run_at = self
            .scan_keys
            .borrow()
            .as_ref()
            .map_or(usize::MAX, |(_, s)| s.log_at);
        let mut stale = self.snap_stale.borrow_mut();
        for i in seen..n {
            let (id, new) = mem.log_at(i);
            if i >= run_at {
                stale.insert(id as u32);
            }
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
                    _ => pending.push((e.key_off, e.key_len, new, id as u32)),
                }
            }
            if let Some((from, since)) = since.as_mut() {
                if i >= *from {
                    let e = mem.entry(id);
                    since.file(mem, e.key_off, e.key_len);
                }
            }
        }
        self.log_seen.set(n);
        true
    }

    fn drop_blocks(&self) {
        for t in self.tables.borrow().iter() {
            *t.borrow_mut() = None;
        }
        self.dirty_any.set(false);
        self.tables_complete.set(false);
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
        let mut ahead = self.ahead.borrow_mut();
        let Some(a) = ahead.as_mut() else {
            return Ok(());
        };
        if a.done.get() || !a.posted.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return Ok(());
        }
        let np = self.segs().partition_point(|s| s.level > 0);
        let l0 = &self.segs()[np..];
        let mut since = a.since.borrow_mut();
        since.settle();
        let mut done = false;
        loop {
            let built = match a.rx.try_recv() {
                Ok(b) => b,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            };
            // Every publish stops the builder before the state it built
            // over changes, so a form of another generation cannot arrive
            // here today; the check guards a path that publishes without
            // one.
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
                table.resnap(seg, unsealed, self.snap_gen.get());
            }
            table.snap_at(seg, unsealed)?;
            for (b, bytes, form) in built.forms {
                let b = b as usize;
                if b >= table.slots.len()
                    || table.slots[b].is_some()
                    || BuildCtx::overlay_count(table, b) > WIDE
                {
                    continue;
                }
                // The form as built, current to the builder's commit, is
                // every handle's at that position; this handle's copy is
                // spliced below, which makes it its own.
                let form = std::sync::Arc::new(form);
                table.slots[b] = Some(form);
                self.list_built_bytes(pi, b, table, bytes);
                self.shed(pi, b, table);
                // The keys written since the builder's watermark that fall
                // in the block, spliced in as a settle splices a write: the
                // block's range runs from its first key, or the partition's
                // lower fence for the first block, to the next block's
                // first key, or the upper fence for the last. Keys past the
                // partition's last key fall in its last block alone, which
                // is where every key a mix inserts past the end goes, and
                // any other block learns so by one compare.
                if since.is_empty() {
                    continue;
                }
                let past_all = b + 1 < table.slots.len()
                    && !table.last_key.is_empty()
                    && since.first().is_some_and(|k| k > table.last_key.as_slice());
                if past_all {
                    continue;
                }
                let lo = if b == 0 {
                    (!seg.lo.is_empty()).then_some(seg.lo.as_slice())
                } else {
                    seg.blob.key_at(b * CACHE_BLOCK)
                };
                let hi = seg.blob.key_at((b + 1) * CACHE_BLOCK).or(seg.hi.as_deref());
                if since.meets(lo, hi) {
                    for i in since.range(lo, hi) {
                        let k = since.at(i);
                        let cut = BuildCtx::owner_of(seg, k).1;
                        // The list carries keys and not slots, so this
                        // path probes for the one the settle carries.
                        let slot = self.mem().slot_of(k).map_or(u32::MAX, |i| i as u32);
                        self.patch_block(pi, b, table, k, cut, slot)?;
                    }
                }
            }
        }
        if done {
            // Every form the builder made is installed or dropped, and the
            // thread is joined; the builder stays, as the mark that this
            // state has had one and the length its forms covered, and the
            // keys since are nobody's to splice.
            *since = Since::default();
            drop(since);
            a.done.set(true);
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
        Ok(())
    }

    /// EXPERIMENT: an empty canonical forms table over `segs`: a slot per
    /// block of every partition, none for a piece.
    fn forms_for(segs: &[std::sync::Arc<Seg>]) -> Vec<Box<[AtomicPtr<CanonicalForm>]>> {
        segs.iter()
            .take_while(|s| s.level > 0)
            .map(|s| {
                (0..s.blob.keys().div_ceil(CACHE_BLOCK))
                    .map(|_| AtomicPtr::new(std::ptr::null_mut()))
                    .collect()
            })
            .collect()
    }

    /// EXPERIMENT: the canonical form of block `b` of partition `p`, for a
    /// reader at the table's position; none where the slot is empty.
    /// EXPERIMENT: the log position a canonical form must have been
    /// settled at for this handle to read it, or none where the handle
    /// may not read one at all. A handle with a slot holds the position
    /// of the commit its watermark names. The writer's own honours no
    /// watermark, so it holds the log's whole length: equal to
    /// `forms_at`, nothing has been written past the commit the forms
    /// were settled at and they are exactly what it must read; short of
    /// it, it has staged writes they do not carry and it builds its own.
    /// `forms_at` before the first maintenance is `usize::MAX`, which no
    /// log length equals, so an unmaintained table is never taken.
    fn forms_bound(&self) -> Option<usize> {
        match self.slot {
            Some(_) => Some(self.log_bound.get()),
            None if self.opts.forms_to_writer => Some(self.mem().log_len()),
            None => None,
        }
    }

    fn canonical(&self, p: usize, b: usize) -> Option<&Cached> {
        let st = self.state();
        let e = st.forms.get(p)?.get(b)?.load(AtomicOrdering::Acquire);
        if e.is_null() {
            return None;
        }
        // SAFETY: a canonical form is freed only with its state or, once
        // replaced, past every reader pinned at or before the
        // replacement, and this handle is pinned for the operation or is
        // the writer's own, which frees nothing while it reads.
        let e = unsafe { &*e };
        Some(&e.form)
    }

    /// The forms a scan took, counted once at its end: counted at each
    /// take it was an atomic add per block on a scan of two or three.
    fn count_takes(&self, n: u64) {
        if n == 0 {
            return;
        }
        match self.slot {
            Some(slot) => {
                self.shared.readers.slots[slot]
                    .takes
                    .fetch_add(n, AtomicOrdering::Relaxed);
            }
            None => {
                self.shared.form_takes.fetch_add(n, AtomicOrdering::Relaxed);
            }
        }
    }

    /// EXPERIMENT: the writer installs `form` as block `b` of partition
    /// `p`'s canonical form, retiring the one it replaces.
    /// EXPERIMENT: this state's published snapshot, cloned. Called under
    /// a pin, which is what holds the pointee across the clone: a swap
    /// beside it retires the old reference rather than dropping it.
    /// EXPERIMENT: a snapshot of the live memtable up to `to`, over this
    /// state, got the cheapest way that is correct: carried forward from
    /// `have` when it is a run of the same state that stops short of
    /// `to`, and sorted from nothing when it is not. Whatever comes back
    /// is offered to the state.
    fn snapshot_to(
        &self,
        to: usize,
        have: Option<std::sync::Arc<Snapshot>>,
    ) -> std::sync::Arc<Snapshot> {
        let carry = have.filter(|s| s.live_len <= to && s.side.is_empty() && s.fresh.is_empty());
        let snap = match carry {
            Some(s) if s.live_len == to => return s,
            Some(s) => {
                self.shared
                    .snap_extends
                    .fetch_add(1, AtomicOrdering::Relaxed);
                std::sync::Arc::new(s.extend(self.mem(), to, self.opts.snapshot_runs))
            }
            None => std::sync::Arc::new(self.build_snapshot(to)),
        };
        self.publish_snapshot(&snap);
        snap
    }

    fn adopt_snapshot(&self) -> Option<std::sync::Arc<Snapshot>> {
        if !self.opts.share_snapshot {
            return None;
        }
        let p = self.state().snap.load(AtomicOrdering::Acquire);
        if p.is_null() {
            return None;
        }
        // SAFETY: published into the state this handle has pinned, and a
        // swap frees what it replaces only past every reader pinned then.
        Some(unsafe {
            std::sync::Arc::increment_strong_count(p);
            std::sync::Arc::from_raw(p as *const Snapshot)
        })
    }

    /// EXPERIMENT: offer a snapshot to the state for the handles that
    /// come after. Kept only when it covers more of the live memtable
    /// than the one there, or as much with its runs copied at a later
    /// log position, so the pointer only moves forward and two handles
    /// racing cannot leave the shorter one published.
    fn publish_snapshot(&self, snap: &std::sync::Arc<Snapshot>) {
        if !self.opts.share_snapshot {
            return;
        }
        let st = self.state();
        let mut cur = st.snap.load(AtomicOrdering::Acquire);
        loop {
            if !cur.is_null() {
                // SAFETY: as in `adopt_snapshot`.
                let c = unsafe { &*cur };
                if (c.live_len, c.log_at) >= (snap.live_len, snap.log_at) {
                    return;
                }
            }
            let new = std::sync::Arc::into_raw(snap.clone()) as *mut Snapshot;
            match st.snap.compare_exchange_weak(
                cur,
                new,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            ) {
                Ok(_) => {
                    if !cur.is_null() {
                        let epoch = self.shared.readers.epoch.load(AtomicOrdering::SeqCst);
                        self.shared
                            .retired_snaps
                            .lock()
                            .expect("the retired snapshots")
                            .push((epoch, RetiredSnap(cur)));
                        // The epoch moved past the retirement, so a
                        // handle that pins from here on is not one that
                        // could hold the replaced snapshot, and the
                        // sweep can free it once the ones before leave.
                        // Left to the writer's own bumps, the keeper's
                        // versions between two seals were freed by none
                        // of them and held every run they had copied.
                        self.shared.readers.bump();
                        self.sweep_retired_snaps();
                    }
                    return;
                }
                Err(now) => {
                    // SAFETY: ours, and no other handle was given it.
                    drop(unsafe { std::sync::Arc::from_raw(new as *const Snapshot) });
                    cur = now;
                }
            }
        }
    }

    /// EXPERIMENT: the snapshots a publish replaced, freed past every
    /// reader pinned before the swap, as the canonical forms are. Called
    /// by whichever handle retired one rather than by the writer alone:
    /// a store between seals never reaches the writer's sweep, and a
    /// retired snapshot holds the key bytes of everything unsealed. A
    /// sweep before every reader has left simply leaves them for the
    /// next.
    fn sweep_retired_snaps(&self) {
        let mut retired = self
            .shared
            .retired_snaps
            .lock()
            .expect("the retired snapshots");
        if retired.is_empty() {
            return;
        }
        let readers = &self.shared.readers;
        retired.retain(|(t, s)| {
            if readers.none_before(t + 1) {
                // SAFETY: replaced in the state, and every handle that
                // could be between the load and the clone has left.
                drop(unsafe { std::sync::Arc::from_raw(s.0) });
                false
            } else {
                true
            }
        });
    }

    /// One block materialised, charged to whoever built it.
    fn count_built(&self) {
        match (self.counted, self.slot) {
            (true, Some(slot)) => {
                self.shared.readers.slots[slot]
                    .built
                    .fetch_add(1, AtomicOrdering::Relaxed);
            }
            _ => {
                self.shared.blk_engine.fetch_add(1, AtomicOrdering::Relaxed);
            }
        }
    }

    /// EXPERIMENT: the mark, in a block's canonical slot, that the block
    /// is every reader's own to build: what the writer publishes for a
    /// block it holds wide or holds no form for, since a reader takes a
    /// published form as the block and, with the table complete, an
    /// empty slot as clean. Before this a block gone wide kept whatever
    /// form was published before it went wide -- the writer drops its
    /// own and builds it wide, a form it never publishes -- and a handle
    /// taking the forms read the last block of a store short of every
    /// key inserted past the end since: three hundred of five hundred in
    /// the test that found it. A wide form is what a reader already
    /// treats as "build your own", so the mark is an empty one.
    fn publish_marker(&self, p: usize, b: usize) {
        let st = self.state();
        let Some(slot) = st.forms.get(p).and_then(|f| f.get(b)) else {
            return;
        };
        let cur = slot.load(AtomicOrdering::Acquire);
        // SAFETY: as in `canonical`.
        if !cur.is_null() && matches!(*unsafe { &*cur }.form, Cached::Wide(_)) {
            return;
        }
        let marker = std::sync::Arc::new(Cached::Wide(WideBlock {
            sorted: Vec::new(),
            seen: 0,
            covered: false,
            walks: 0,
        }));
        self.publish_form(p, b, &marker);
    }

    /// EXPERIMENT: whether the forms are published now: always when
    /// `force`d or published eagerly, else only for a handle the caller
    /// holds; see `Options::forms_publish_lazily`.
    fn publishing(&self, force: bool) -> bool {
        force
            || !self.opts.forms_publish_lazily
            || self.opts.forms_to_writer
            || self.shared.live_handles.load(AtomicOrdering::Relaxed) > 0
    }

    /// EXPERIMENT: every block this handle built or patched since the
    /// last publish, published: its form, or the mark for a block it
    /// holds wide or no longer holds. Returns whether the table is
    /// current to this handle's tables afterwards, which it is not when
    /// nobody is publishing for and the blocks are left dirty.
    fn publish_dirty(&self, force: bool) -> bool {
        if !self.publishing(force) {
            return false;
        }
        if !self.opts.commit_forms_build || !self.dirty_any.get() {
            return true;
        }
        let tables = self.tables.borrow();
        for (p, cell) in tables.iter().enumerate() {
            let mut held = cell.borrow_mut();
            let Some(t) = held.as_mut() else { continue };
            for b in 0..t.dirty.len() {
                if !t.dirty[b] {
                    continue;
                }
                t.dirty[b] = false;
                match t.slots[b].as_ref() {
                    Some(form) if !matches!(**form, Cached::Wide(_)) => {
                        self.publish_form(p, b, form)
                    }
                    _ => self.publish_marker(p, b),
                }
            }
        }
        self.dirty_any.set(false);
        true
    }

    /// EXPERIMENT: the forms published for a handle just claimed, when
    /// they were left dirty for want of one: the backlog filed so they
    /// are current to the whole log, and every dirty block published at
    /// the log's length. Only when nothing is staged -- the writer's own
    /// scans settle staged writes into its tables, and a handle must not
    /// find those -- and otherwise at the next commit.
    fn publish_for_handle(&self) {
        if self.slot.is_some() || !self.opts.scan_block_cache {
            return;
        }
        let st = self.state();
        let mem = self.mem();
        if mem.log_len() != mem.committed_log() {
            if self.opts.commit_forms && self.dirty_any.get() {
                self.publish_due.set(true);
            }
            return;
        }
        // The log read and the batch settled first, which is the order a
        // snapshot may move in; then the snapshot brought current and
        // published, so the handle adopts it.
        let moved = self.sync_log();
        if moved && self.settle_pending().is_err() {
            return;
        }
        if self.segs().first().is_some_and(|s| s.level > 0) {
            self.refresh_snapshot_to(st.gen, true, moved, true);
        }
        if !self.opts.commit_forms || !self.dirty_any.get() || self.log_gen.get() != st.gen {
            return;
        }
        if self.publish_dirty(true) {
            st.forms_complete
                .store(self.tables_complete.get(), AtomicOrdering::Release);
            st.forms_at
                .store(mem.committed_log(), AtomicOrdering::Release);
        }
    }

    fn publish_form(&self, p: usize, b: usize, form: &std::sync::Arc<Cached>) {
        let st = self.state();
        let Some(slot) = st.forms.get(p).and_then(|f| f.get(b)) else {
            return;
        };
        let bytes = form.bytes();
        let new = Box::into_raw(Box::new(CanonicalForm {
            form: form.clone(),
            bytes,
        }));
        let old = slot.swap(new, AtomicOrdering::AcqRel);
        st.forms_bytes.fetch_add(bytes, AtomicOrdering::Relaxed);
        if !old.is_null() {
            // SAFETY: as in `canonical`; the pointee is read only to
            // count its bytes out, and freed past every pinned reader.
            let old_bytes = unsafe { &*old }.bytes;
            st.forms_bytes.fetch_sub(old_bytes, AtomicOrdering::Relaxed);
            let epoch = self.shared.readers.epoch.load(AtomicOrdering::SeqCst);
            self.shared
                .retired_forms
                .lock()
                .expect("the retired forms")
                .push((epoch, RetiredForm(old)));
        }
    }

    /// EXPERIMENT: where a key's block stands: its partition, its block,
    /// whether the block is held as a copy too, the entries reads have
    /// taken from it since it was written, and the cheap form's shape.
    pub fn block_state(&self, key: &[u8]) -> Option<(usize, usize, bool, u32, &'static str)> {
        let _e = self.enter();
        let np = self.segs().partition_point(|s| s.level > 0);
        let at = self.segs()[..np]
            .partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let seg = self.segs()[..np].get(at)?;
        let (b, _) = BuildCtx::owner_of(seg, key);
        let tables = self.tables.borrow();
        let held = tables.get(at)?.borrow();
        let t = held.as_ref()?;
        if b >= t.slots.len() {
            return None;
        }
        let kind = match t.slots[b].as_deref() {
            None => "unbuilt",
            Some(Cached::Clean) => "clean",
            Some(Cached::Sparse(_)) => "sparse",
            Some(Cached::Block(_)) => "copy",
            Some(Cached::Wide(_)) => "wide",
        };
        Some((at, b, t.dense[b].is_some(), t.reads[b], kind))
    }

    /// EXPERIMENT: what the reads chose: blocks promoted to a copy
    /// beside their cheap form, copies dropped by a write, block walks
    /// over a copy, block walks over the cheap form, and the bytes the
    /// copies hold now.
    pub fn form_choices(&self) -> [u64; 5] {
        self.choices.get()
    }

    /// EXPERIMENT: the canonical forms table's size: forms held, their
    /// bytes, how many walks took one, and whether the table is complete.
    pub fn canonical_forms(&self) -> (usize, usize, usize, bool) {
        let st = self.state();
        let forms = st
            .forms
            .iter()
            .flat_map(|f| f.iter())
            .filter(|s| !s.load(AtomicOrdering::Relaxed).is_null())
            .count();
        (
            forms,
            st.forms_bytes.load(AtomicOrdering::Relaxed),
            (self.shared.form_takes.load(AtomicOrdering::Relaxed)
                + self.shared.readers.stat(|s| &s.takes)) as usize,
            st.forms_complete.load(AtomicOrdering::Relaxed),
        )
    }

    /// PROTOTYPE: the build context over this store's own state.
    fn build_ctx(&self) -> BuildCtx<'_> {
        BuildCtx {
            wm: self.wm(),
            segs: self.segs(),
            mem: self.mem(),
            frozen: self.frozen().map(|f| f.as_ref()),
            tombs: self.has_tombstones(),
            dense_from: if self.opts.commit_forms && self.slot.is_none() {
                match self.opts.form_dense_from {
                    0 => CACHE_DENSE,
                    n => n,
                }
            } else {
                CACHE_DENSE
            },
            stale: self.snap_stale.borrow(),
            copy_dense: false,
        }
    }

    /// The scan snapshot, current: what the scan preamble held inline
    /// before a commit needed the same. `gen` is the state's, `moved`
    /// whether the log or the state has moved since this handle looked.
    fn refresh_snapshot(&self, gen: u64, use_cache: bool, moved: bool) {
        self.refresh_snapshot_to(gen, use_cache, moved, false)
    }

    /// `refresh_snapshot`, and with `current` a snapshot short of the live
    /// memtable is stale whatever the rule below says: what the writer
    /// does at a handle's claim, so the handle adopts a snapshot instead
    /// of building one. The writer's own rule keeps a snapshot until the
    /// keys since outnumber it, filing them by block meanwhile, and that
    /// is right for the writer, whose tables carry those keys; a handle
    /// has no such tables and could only build. At ten thousand keys the
    /// published snapshot was the empty one the drained scan pass built,
    /// and every handle claimed after the mixes built its own from
    /// nothing: 70 µs of a first scan of 120, in a pass of a hundred
    /// scans that take two each.
    fn refresh_snapshot_to(&self, gen: u64, use_cache: bool, moved: bool, current: bool) {
        if !moved && !current {
            return;
        }
        let mut cache = self.scan_keys.borrow_mut();
        if current {
            let short = cache
                .as_ref()
                .is_none_or(|(g, s)| *g != gen || s.live_len < self.mem().len());
            if !short {
                return;
            }
        }
        // What a snapshot may lack before it is worth building again:
        // the keys created since it was built, against the keys it has.
        let behind = |held: usize, added: usize| {
            if use_cache {
                added > held.max(4096)
            } else {
                added > (held / 8).max(4096)
            }
        };
        let mut stale = current || cache.as_ref().is_none_or(|(g, _)| *g != gen);
        if !stale {
            let held = cache.as_ref().map_or(0, |(_, s)| s.len());
            let added = self.snap_added.borrow().len();
            stale = behind(held, added);
        }
        // PROTOTYPE: the published snapshot ahead of this handle's own
        // by a share of what the handle holds -- the keeper has absorbed
        // that many writes this one still reads from their chains, or
        // holds in its added list -- is taken instead: the switch costs
        // a merge of the run and the bounds walked again, and the share
        // is where that is repaid by the reads.
        if !stale && self.opts.snapshot_keeper {
            if let Some((_, mine)) = cache.as_ref() {
                let p = self.state().snap.load(AtomicOrdering::Acquire);
                if !p.is_null() {
                    // SAFETY: as in `adopt_snapshot`.
                    let published = unsafe { &*p };
                    let ahead = published.log_at.saturating_sub(mine.log_at);
                    stale = ahead >= (mine.len() / 8).max(1024);
                    if stale {
                        self.shared
                            .snap_switched
                            .fetch_add(1, AtomicOrdering::Relaxed);
                    }
                }
            }
        }
        if !stale && !use_cache {
            let (_, snap) = cache.as_mut().expect("not stale");
            let added = self.snap_added.borrow();
            if added.len() > snap.filed {
                // A snapshot this handle adopted is shared, and filing
                // its own keys into it is the handle's business alone:
                // the copy is taken here and not by every reader.
                let snap = std::sync::Arc::make_mut(snap);
                snap.file(self.mem(), &added[snap.filed..]);
                snap.filed = added.len();
            }
        }
        if stale {
            let live_len = self.mem().len();
            // The state's, when it covers enough of the memtable, and
            // this handle's own otherwise. Adopting is the whole of the
            // saving: the sort is of every unsealed key, and a handle
            // that scans a store someone else is already scanning would
            // otherwise repeat it in full.
            // What this handle holds already, when it is a run over this
            // same state: carrying that forward is a merge of the batch
            // written since, where building is a sort of everything.
            let have = cache
                .as_ref()
                .filter(|(g, _)| *g == gen)
                .map(|(_, s)| s.clone());
            let snap = match self.adopt_snapshot() {
                Some(s)
                    if live_len.saturating_sub(s.live_len) <= self.opts.snapshot_adopt_behind =>
                {
                    s
                }
                // Short of current: carry the longer of the two runs
                // forward rather than sort everything again. Taking it
                // as it stands would leave the keys past it in the added
                // list; sorting afresh throws away a run that is nearly
                // all of the answer -- at a hundred thousand keys each
                // updated once, the state's run stopped 1,480 keys short
                // and the build that replaced it cost 9.5 ms where the
                // merge costs 2.
                published => {
                    let base = match (have, published) {
                        (Some(a), Some(b)) if b.live_len > a.live_len => Some(b),
                        (Some(a), _) => Some(a),
                        (None, b) => b,
                    };
                    self.snapshot_to(live_len, base)
                }
            };
            // The keys the snapshot has are not added keys; an adopted
            // one may stop short of the memtable's end, and the slots
            // past where it stops stay in the list for the tables.
            self.snap_entries.set(snap.live_len);
            self.snap_added
                .borrow_mut()
                .retain(|&slot| slot as usize >= snap.live_len);
            // The stale set is the log from the runs' copy to where this
            // handle has read it; what it reads on is added as it goes.
            {
                let mem = self.mem();
                let mut stale = self.snap_stale.borrow_mut();
                stale.clear();
                for i in snap.log_at..self.log_seen.get().min(mem.log_len()) {
                    stale.insert(mem.log_at(i).0 as u32);
                }
            }
            *cache = Some((gen, snap));
            // Every key created since the old snapshot is in the new one:
            // the lists that held them are emptied, and the bounds each
            // table walked are walked again on its next touch.
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
                        if matches!(t.slots[b].as_deref(), Some(Cached::Wide(_))) {
                            self.unlist(p, b, t);
                        }
                    }
                }
            }
        }
    }

    /// EXPERIMENT: the writer's commit keeps the canonical forms current.
    /// The builder's forms are installed and the batch's keys settled
    /// into the writer's own copies, and every copy touched since the
    /// last commit is swapped into the state's table; each form is then
    /// current to this commit, so a reader under `Latest` walks it
    /// instead of building one. Every write the copies hold is committed
    /// by the time they are published, which is what makes them a
    /// reader's to walk at all.
    ///
    /// The table is complete once the builder ahead is done and the keys
    /// written while it ran have forms: from then on a block with no
    /// form is clean, and the writer builds one for any block a write
    /// lands in. Under a cache budget it is never complete, since a form
    /// may be shed.
    fn maintain_forms(&self) -> Result<()> {
        if !(self.opts.commit_forms && self.opts.scan_block_cache) {
            return Ok(());
        }
        // The regime, asked of the store rather than set for it: the
        // forms cost the writer at every commit and are read by whoever
        // holds a handle, so they pay where several do and lose where the
        // writer reads its own store. Stopping is safe at any commit --
        // `forms_at` stops advancing, and a reader trusts the forms only
        // where it matches the log position the reader holds, so it
        // builds its own from the next one.

        let st = self.state();
        if st.reader_scans.load(AtomicOrdering::Relaxed) < self.opts.forms_from_reader_scans as u64
        {
            return Ok(());
        }
        if st.forms.is_empty() || self.segs().first().is_none_or(|s| s.level == 0) {
            return Ok(());
        }
        // Too much of the store unsealed to be worth organising: see
        // `Options::forms_max_unsealed_pct`. The unsealed count is the
        // memtables' own, and the store's is what the partitions hold,
        // which is a load a segment and there are few.
        if self.opts.forms_max_unsealed_pct > 0 {
            let unsealed = st.mem.committed_len() + st.frozen.as_ref().map_or(0, |f| f.len());
            let keys: usize = self.segs().iter().map(|s| s.blob.keys()).sum();
            if keys > 0 && unsealed * 100 > keys * self.opts.forms_max_unsealed_pct {
                return Ok(());
            }
        }
        // Nothing to maintain where nothing has been scanned since the
        // last commit: the structure is for range reads, and a run of
        // writes with no read between them pays nothing for it. What is
        // published stays where it was, so a reader past it builds its
        // own, and the blocks touched meanwhile are published by the
        // first commit a scan precedes.
        let scans = st.scans.load(AtomicOrdering::Relaxed);
        // The writes not yet filed into the forms: everything the log holds
        // past the position this handle last read it to.
        let log_len = self.mem().log_len();
        let backlog = log_len.saturating_sub(self.log_seen.get());
        let keys: usize = self.segs().iter().map(|s| s.blob.keys()).sum();
        // The bound is a share of the store's keys and not a count, for
        // the reason the seal cap's floor is: a count that is free at one
        // rung is five settles at the next.
        let bound = if self.opts.forms_settle_backlog_pct == 0 {
            usize::MAX
        } else {
            (keys / 100 * self.opts.forms_settle_backlog_pct).max(1)
        };
        // The writes since the last scan over the store, through whichever
        // handle: the log's growth since this handle last counted it, the
        // whole log where the generation has changed since.
        let (gen_counted, len_counted) = self.log_counted.get();
        let grew = if gen_counted == st.gen {
            log_len.saturating_sub(len_counted)
        } else {
            log_len
        };
        self.log_counted.set((st.gen, log_len));
        let writes = self.writes_seen.get() + grew as u64;
        self.writes_seen.set(writes);
        let life = self.shared.scans_life.load(AtomicOrdering::Relaxed);
        if life != self.scans_life_seen.get() {
            self.scans_life_seen.set(life);
            self.writes_at_scan.set(writes);
        }
        // A burst is settled by its backlog only near a scan, see
        // `Options::forms_settle_recent_pct`: far from one, what it would
        // file is more likely discarded at the next seal than read.
        let recent = self.opts.forms_settle_recent_pct == 0
            || writes - self.writes_at_scan.get()
                <= (keys / 100 * self.opts.forms_settle_recent_pct) as u64;
        // Forms the builder has posted are installed now, and the batch
        // settled with them, so the first read after finds them in
        // place; see `Options::build_ahead_on_publish`.
        let posted =
            self.ahead.borrow().as_ref().is_some_and(|a| {
                !a.done.get() && a.posted.load(std::sync::atomic::Ordering::Acquire)
            });
        let due = scans != self.scans_seen.get()
            || (backlog >= bound && recent)
            || posted
            || self.publish_due.replace(false);
        if !due {
            return Ok(());
        }
        self.scans_seen.set(scans);
        self.cache_used.set(true);
        let gen = st.gen;
        let moved = self.sync_log() || self.scan_keys.borrow().is_none();
        // Settle before the snapshot moves, which is the order the scan
        // path takes and the reason it was never wrong. A pending write
        // carries the flag `sync_log` gave it -- created since the
        // snapshot, or in it already -- and that flag is about the
        // snapshot standing when the log was read. Settled after a
        // refresh that carried the snapshot past those keys, every one of
        // them is filed into the block it lands in as new while the
        // snapshot holds it too, and the overlay merge meets the same
        // live slot twice: `a live key was created twice`, with slot 1000
        // on both sides of it and a snapshot 8,000 entries long. This was
        // here before the snapshot was ever carried forward -- a rebuild
        // moves it just as far -- and it never fired because nothing
        // maintained the forms by default, so nothing reached this line.
        if moved {
            self.settle_pending()?;
        }
        self.refresh_snapshot(gen, true, moved);
        {
            let cache = self.scan_keys.borrow();
            let unsealed: &Snapshot = &cache.as_ref().expect("scan snapshot").1;
            self.install_ahead(unsealed)?;
        }
        // A form for every overlaid block, once per state: from here on
        // the writer builds one for any block a write lands in, so a
        // block without one is clean. Under a cache budget a form may be
        // shed, so the table is never called complete.
        //
        // The fill itself belongs to the builder ahead, on a core the
        // writer is not using: filling it from the commit put the whole
        // overlay of the mixes before E into E's own window, and E read
        // 0.70x-0.81x of the arm without forms where a table already
        // filled read 1.01x-1.33x. The builder declines a store too
        // small for one, and there the commit fills it, which at ten
        // thousand keys is a few hundred blocks.
        if self.opts.commit_forms_build
            && self.opts.scan_cache_bytes == 0
            && !self.tables_complete.get()
        {
            if self.opts.scan_cache_ahead && self.ahead.borrow().is_none() {
                self.start_ahead();
            }
            let filling = self.ahead.borrow().as_ref().is_some_and(|a| !a.done.get());
            if !filling {
                let cache = self.scan_keys.borrow();
                let unsealed: &Snapshot = &cache.as_ref().expect("scan snapshot").1;
                self.complete_forms(unsealed)?;
                drop(cache);
                self.tables_complete.set(true);
            }
        }
        // The table's position and completeness move with a publish and
        // only then: a handle at an older commit finds the forms of that
        // commit until someone is here to take newer ones.
        if self.publish_dirty(false) {
            st.forms_complete
                .store(self.tables_complete.get(), AtomicOrdering::Release);
            st.forms_at
                .store(self.mem().committed_log(), AtomicOrdering::Release);
        }
        Ok(())
    }

    /// EXPERIMENT: a form for every block any unsealed key overlays,
    /// after which a block with none is clean and the table is
    /// complete. The tables know where the overlay falls already --
    /// each piece's and the snapshot's block bounds, and the keys filed
    /// since -- so this counts rather than walks keys, and builds only
    /// the blocks with a count and no form. It is the work the builder
    /// ahead does on a spare core, done here for the store the builder
    /// declined (too small) or has not reached.
    fn complete_forms(&self, unsealed: &Snapshot) -> Result<()> {
        let np = self.segs().partition_point(|s| s.level > 0);
        let l0 = &self.segs()[np..];
        let ctx = self.build_ctx();
        ctx.rank_pieces()?;
        for pi in 0..np {
            let seg = &self.segs()[pi];
            if seg.blob.keys() == 0 {
                continue;
            }
            let tables = self.tables.borrow();
            let mut held = tables[pi].borrow_mut();
            if held.is_none() {
                *held = Some(self.make_table(seg, l0, unsealed)?);
            }
            let table = held.as_mut().expect("just made");
            if table.snap_gen != self.snap_gen.get() {
                table.resnap(seg, unsealed, self.snap_gen.get());
            }
            if table.clean_throughout() {
                continue;
            }
            table.snap_at(seg, unsealed)?;
            let src = Sources { seg, l0 };
            let ctx = BuildCtx {
                copy_dense: true,
                ..self.build_ctx()
            };
            for b in 0..table.slots.len() {
                if table.slots[b].is_some() || BuildCtx::overlay_count(table, b) == 0 {
                    continue;
                }
                let built = std::sync::Arc::new(ctx.materialize(src, table, b, unsealed)?);
                self.count_built();
                let bytes = built.bytes();
                table.slots[b] = Some(built);
                self.list_built_bytes(pi, b, table, bytes);
            }
        }
        Ok(())
    }

    fn list_built(&self, p: usize, b: usize, table: &mut BlockTable) {
        let bytes = table.slots[b].as_ref().map_or(0, |c| c.bytes());
        self.list_built_bytes(p, b, table, bytes);
    }

    /// `list_built` for a form whose size is known already.
    fn list_built_bytes(&self, p: usize, b: usize, table: &mut BlockTable, bytes: usize) {
        if self.opts.commit_forms && self.slot.is_none() {
            table.dirty[b] = true;
            self.dirty_any.set(true);
        }
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
        // Whatever was published for the block is not this handle's to
        // keep current any more: a reader builds it for itself.
        if self.opts.commit_forms && self.slot.is_none() {
            self.publish_marker(p, b);
        }
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
        // Grouped by key with the write that created it first: that one
        // files the key into its block's list and the rest are skipped as
        // duplicates, so the order is a correctness order. One integer key,
        // since a tuple compared through a closure was a quarter of a settle.
        pending.sort_unstable_by_key(|&(off, _, new, _)| ((off as u64) << 1) | u64::from(!new));
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

    fn settle_each(&self, pending: &[(u32, u32, bool, u32)]) -> Result<()> {
        let np = self.segs().partition_point(|s| s.level > 0);
        // Resolved first, applied in the partition's order. A write's
        // position is one seek, but applying it reads the partition's
        // record at that position and the block's own buffers, and in
        // the log's order those are one cold line each per write: about
        // half of a settle waited on `Blob::key_at`. Sorted by position
        // the reads run through the partition once, the way a B-tree
        // applies a batch.
        let mut last = u32::MAX;
        let mut resolved: Vec<(u32, u32, u32, u32, u32, bool, u32)> =
            Vec::with_capacity(pending.len());
        for &(off, len, new, slot) in pending {
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
            let (b, cut) = BuildCtx::owner_of(seg, key);
            resolved.push((at as u32, b as u32, cut, off, len, new, slot));
        }
        resolved.sort_unstable_by_key(|r| (u64::from(r.0) << 32) | u64::from(r.2));
        for &(at, b, cut, off, len, new, slot) in &resolved {
            let (at, b) = (at as usize, b as usize);
            let key = self.mem().key_at(off, len);
            let tables = self.tables.borrow();
            let mut held = tables[at].borrow_mut();
            let Some(table) = held.as_mut() else {
                continue;
            };
            if b < table.slots.len() {
                // The copy this block was also held as is the writes'
                // to drop: patching two forms for one write is what the
                // promotion is paid out of, and the halved count is
                // what brings the copy back for a block the reads still
                // own.
                if let Some(old) = table.dense[b].take() {
                    self.cache_bytes
                        .set(self.cache_bytes.get().saturating_sub(old.bytes()));
                    let mut c = self.choices.get();
                    c[1] += 1;
                    c[4] = c[4].saturating_sub(old.bytes() as u64);
                    self.choices.set(c);
                }
                table.reads[b] /= 2;
                // A write into a block the writer holds no form for
                // leaves the table incomplete rather than building one:
                // a reader then builds that block itself, as it does
                // without the forms at all. Building it here instead,
                // to keep every empty slot meaning clean, made the
                // writer build a form for every block any write landed
                // in, which over a uniform update mix is the whole
                // store, where a skewed scan reads a fraction of it: E
                // read 0.73x-0.75x of the arm without forms.
                if table.slots[b].is_none() && self.opts.commit_forms && self.slot.is_none() {
                    debug_assert!(
                        self.canonical(at, b)
                            .is_none_or(|f| matches!(f, Cached::Wide(_))),
                        "a block the writer holds no form for has none published as the block"
                    );
                    self.tables_complete.set(false);
                }
                self.patch_block(at, b, table, key, cut, slot)?;
            }
            if new {
                if let Some(list) = table.added.get_mut(b) {
                    list.push((slot, cut));
                    table.filed += 1;
                }
            }
            // Only a build chooses the wide form, and a patched block is
            // never rebuilt: past the bound, the block is dropped so the
            // next scan builds it wide, as the drop-and-rebuild did.
            if b < table.slots.len()
                && BuildCtx::overlay_count(table, b) > WIDE
                && !matches!(table.slots[b].as_deref(), Some(Cached::Wide(_)))
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
        slot: u32,
    ) -> Result<()> {
        if matches!(table.slots[b].as_deref(), None | Some(Cached::Wide(_))) {
            return Ok(());
        }
        if self.opts.commit_forms && self.slot.is_none() {
            table.dirty[b] = true;
            self.dirty_any.set(true);
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
            mem: slot,
            frozen: self.frozen().as_ref().map_or(u32::MAX, |fr| slot_in(fr)),
            lrun: NO_RUN,
            frun: NO_RUN,
        };
        let ctx = self.build_ctx();
        let ov = Overlay {
            over: vec![Over {
                key,
                sk: Some(sk),
                cut,
                pieces: 0..held.len() as u32,
            }],
            held,
            snap: &NO_SNAPSHOT,
            stale: &ctx.stale,
        };
        let (c, at_eq) = BuildCtx::cut_known(cut, lo, hi);
        let same = c < hi && at_eq == Ordering::Equal;
        let mut em = Emit {
            tombs: self.has_tombstones(),
            scratch: Vec::new(),
        };
        let mut run_scratch = self.settle_run.borrow_mut();
        let run: &mut Vec<u8> = &mut run_scratch;
        run.clear();
        ctx.emit_over(
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
        let was_clean = matches!(table.slots[b].as_deref(), Some(Cached::Clean));
        let mut bloated = false;
        match std::sync::Arc::make_mut(table.slots[b].as_mut().expect("checked above")) {
            Cached::Block(blk) => {
                let i = blk.lower_bound(key);
                let at_run = blk.vals.len() as u32;
                blk.vals.extend_from_slice(&run[..]);
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
                sb.vals.extend_from_slice(&run[..]);
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
                    vals: std::mem::take(run),
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
        // From here the writes are filed into the tables, whoever made
        // this one. The scan path and the install set this beside their
        // call and the commit's fill did not, and `sync_log` resets it
        // when the generation moves -- after `maintain_forms` had set it
        // -- so a writer whose first table over a state was the commit's
        // fill, because a handle's scan and not its own asked for the
        // maintenance, filed nothing into its forms for the rest of the
        // state: its scans answered a key's values short of every write
        // since, and so did the forms it published.
        self.cache_used.set(true);
        let nblocks = seg.blob.keys().div_ceil(CACHE_BLOCK);
        let ctx = self.build_ctx();
        ctx.rank_pieces()?;
        let (pieces, piece_ranks) = ctx.table_bounds(seg, l0)?;
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
            dense: (0..nblocks).map(|_| None).collect(),
            reads: vec![0; nblocks],
            touched: vec![0; nblocks],
            listed: vec![u32::MAX; nblocks],
            last_key: seg
                .blob
                .keys()
                .checked_sub(1)
                .and_then(|r| seg.blob.key_at(r))
                .map_or_else(Vec::new, <[u8]>::to_vec),
            pieces,
            piece_ranks,
            snap_at: std::cell::OnceCell::new(),
            snap_span: BuildCtx::snap_span(seg, unsealed),
            snap_gen: self.snap_gen.get(),
            added,
            filed,
            dirty: vec![false; nblocks],
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
        if self.opts.scan_cache_ahead && self.ahead.borrow().is_none() {
            self.start_ahead();
        }
        self.install_ahead(unsealed)?;
        // EXPERIMENT: a reader handle whose watermark is the commit the
        // canonical forms were maintained at walks them and builds
        // nothing; with the table complete, a block with no form is
        // clean. The writer's own handle holds those forms already, with
        // whatever it has staged since settled into them.
        let st = self.state();
        // The regime's signal: a scan happened over this state since the
        // last commit. The writer's own handle says so at every scan; a
        // handle says so once per commit -- the count is compared, not
        // read -- so four handles scanning do not bump one line a
        // thousand times a millisecond.
        if self.opts.commit_forms || self.opts.snapshot_keeper {
            let now = (st.gen, self.mem().committed_log());
            let signal = self.slot.is_none() || self.signalled.replace(now) != now;
            if signal {
                st.scans.fetch_add(1, AtomicOrdering::Relaxed);
                self.shared.scans_life.fetch_add(1, AtomicOrdering::Relaxed);
                if self.counted {
                    st.reader_scans.fetch_add(1, AtomicOrdering::Relaxed);
                }
            }
        }
        let canonical = self.opts.commit_forms
            && self
                .forms_bound()
                .is_some_and(|n| st.forms_at.load(AtomicOrdering::Acquire) == n);
        if self.opts.commit_forms && self.forms_bound().is_some() {
            match self.slot {
                Some(slot) => {
                    let s = &self.shared.readers.slots[slot];
                    s.tried.fetch_add(1, AtomicOrdering::Relaxed);
                    if canonical {
                        s.hit.fetch_add(1, AtomicOrdering::Relaxed);
                    }
                }
                None => {
                    self.shared
                        .canon_tried
                        .fetch_add(1, AtomicOrdering::Relaxed);
                    if canonical {
                        self.shared.canon_hit.fetch_add(1, AtomicOrdering::Relaxed);
                    }
                }
            }
        }
        let complete = canonical && st.forms_complete.load(AtomicOrdering::Acquire);
        // One context for the scan: its tombstone flag is a walk over every
        // segment, which a context per block paid on every sparse walk.
        let ctx = self.build_ctx();
        // EXPERIMENT: the per-block bookkeeping is dead under the default
        // options -- the shed that reads `touched` returns at once with
        // no budget, and `dense` is written only by a promotion -- and
        // each of those is an array as long as the partition's blocks in
        // an allocation of its own, so reading one is a cold line of its
        // own on a scan that streams past the cache.
        let bookkeep = self.opts.scan_cache_bytes > 0;
        let promoting = self.opts.promote_entries > 0;
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
            let (rank, exact) = seg.ord.seek_exact(cursor, |r| seg.blob.key_at(r));
            let same =
                rank < keys && exact.unwrap_or_else(|| seg.blob.key_at(rank) == Some(cursor));
            let owner = if same { rank } else { rank.saturating_sub(1) };
            let nblocks = keys.div_ceil(CACHE_BLOCK);
            let tables = self.tables.borrow();
            let mut held = tables[pi].borrow_mut();
            if held.is_none() {
                *held = Some(self.make_table(seg, l0, unsealed)?);
                self.cache_used.set(true);
            }
            let table = held.as_mut().expect("just made");
            if table.snap_gen != self.snap_gen.get() {
                table.resnap(seg, unsealed, self.snap_gen.get());
            }
            if table.clean_throughout() && !canonical {
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
            // Whether the block this iteration walks was fetched by the
            // one before, as the next block it would cross into: the
            // same span from the same rank, issued twice.
            let mut fetched = false;
            let mut takes = 0u64;
            while seen < limit && b < nblocks {
                let lo = b * CACHE_BLOCK;
                let hi = ((b + 1) * CACHE_BLOCK).min(keys);
                let start = if first { rank.max(lo) } else { lo };
                let from_key: &[u8] = if first { cursor } else { b"" };
                // A wide form is the handle's own to walk against its own
                // list of filed keys, so it is never taken from the
                // table; a reader that meets one builds as before.
                let canon: Option<&Cached> =
                    match canonical.then(|| self.canonical(pi, b)).flatten() {
                        Some(Cached::Wide(_)) => None,
                        Some(f) => {
                            takes += 1;
                            Some(f)
                        }
                        None if complete => Some(&Cached::Clean),
                        None => None,
                    };
                if canon.is_none() && table.slots[b].is_none() {
                    let built = std::sync::Arc::new(ctx.materialize(src, table, b, unsealed)?);
                    self.count_built();
                    let bytes = built.bytes();
                    table.slots[b] = Some(built);
                    self.list_built_bytes(pi, b, table, bytes);
                    self.shed(pi, b, table);
                }
                if bookkeep && canon.is_none() {
                    table.touched[b] = tick;
                }
                // A block wide by the covered rule is walked on its first
                // touch and copied on its second: the rule saves the copy
                // where a block is met once, which is the pass after a
                // burst, and a copy of the run is what a block read again
                // and again wants -- ycsb-E at a hundred thousand keys
                // read 0.58x with every touch assembling the window.
                let mut recopy = false;
                if let (None, Some(Cached::Wide(w))) =
                    (canon, table.slots[b].as_mut().map(std::sync::Arc::make_mut))
                {
                    w.walks = w.walks.saturating_add(1);
                    recopy = w.covered && w.walks >= 2;
                }
                if recopy {
                    self.unlist(pi, b, table);
                    let built = std::sync::Arc::new(
                        BuildCtx {
                            dense_from: 1,
                            copy_dense: true,
                            ..self.build_ctx()
                        }
                        .materialize(src, table, b, unsealed)?,
                    );
                    self.count_built();
                    let bytes = built.bytes();
                    table.slots[b] = Some(built);
                    self.list_built_bytes(pi, b, table, bytes);
                    self.shed(pi, b, table);
                }
                if let (None, Some(Cached::Wide(w))) =
                    (canon, table.slots[b].as_mut().map(std::sync::Arc::make_mut))
                {
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
                // The choice, per read and per block: the copy when this
                // block is also held as one, the cheap form when it is
                // not. Both are current -- a write drops the copy and
                // patches the cheap form -- so the pick costs one load
                // and a branch.
                let cheap: &Cached = match canon {
                    Some(f) => f,
                    None => table.slots[b].as_deref().expect("just built"),
                };
                let dense: Option<&Cached> = if promoting {
                    table.dense[b].as_deref()
                } else {
                    None
                };
                let form: &Cached = dense.unwrap_or(cheap);
                let mut c = self.choices.get();
                c[if dense.is_some() { 2 } else { 3 }] += 1;
                self.choices.set(c);
                let took_from = seen;
                if !fetched {
                    prefetch_block(&seg.blob, form, start, ahead);
                }
                fetched = false;
                if hi - start < ahead && b + 1 < nblocks {
                    let next = match canonical.then(|| self.canonical(pi, b + 1)).flatten() {
                        Some(f) => {
                            takes += 1;
                            Some(f)
                        }
                        None if complete => Some(&Cached::Clean),
                        None => table.slots[b + 1].as_deref(),
                    };
                    if let Some(next) = next {
                        prefetch_block(&seg.blob, next, hi, ahead - (hi - start));
                        fetched = true;
                    }
                }
                match form {
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
                        let clean_at = |b: usize| {
                            if canonical {
                                complete && st.forms[pi][b].load(AtomicOrdering::Acquire).is_null()
                            } else {
                                matches!(table.slots[b].as_deref(), Some(Cached::Clean))
                            }
                        };
                        while b + 1 < nblocks && run_hi - start < limit - seen && clean_at(b + 1) {
                            b += 1;
                            // The run took the block the next-block
                            // fetch was for; the block after it was not
                            // fetched.
                            fetched = false;
                            if bookkeep && !canonical {
                                table.touched[b] = tick;
                            }
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
                // What this read took from the block is what pays for a
                // copy of it; past the crossover the store holds it both
                // ways from here on. Only a block walked as deltas has
                // anything to gain: a clean one is already the bulk
                // walk's, and a dense cheap form is a copy already.
                let promote = self.opts.promote_entries;
                if promote > 0 && table.dense[b].is_none() {
                    let took = (seen - took_from) as u32;
                    table.reads[b] = table.reads[b].saturating_add(took);
                    // A walked block is copied only below the wide bound:
                    // past it the block holds more than a copy should.
                    let repays = match table.slots[b].as_deref() {
                        Some(Cached::Sparse(_)) => true,
                        Some(Cached::Wide(_)) => BuildCtx::overlay_count(table, b) <= WIDE,
                        _ => false,
                    };
                    if table.reads[b] as usize >= promote && repays {
                        let copied = BuildCtx {
                            dense_from: 1,
                            copy_dense: true,
                            ..self.build_ctx()
                        }
                        .materialize(src, table, b, unsealed)?;
                        if matches!(copied, Cached::Block(_)) {
                            let mut c = self.choices.get();
                            c[0] += 1;
                            c[4] += copied.bytes() as u64;
                            self.choices.set(c);
                            self.cache_bytes
                                .set(self.cache_bytes.get() + copied.bytes());
                            table.dense[b] = Some(std::sync::Arc::new(copied));
                        }
                    }
                }
                b += 1;
                first = false;
            }
            self.count_takes(takes);
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
                match &**c {
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
                        .filter(|c| matches!(&***c, Cached::Wide(_)))
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
                    (uk, &sk),
                    same.then_some((seg, rank)),
                    unsealed,
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
            self.emit_unsealed(&mut f, &mut scratch, tombs, (uk, &sk), None, unsealed)?;
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
        entry: (&[u8], &SnapKey),
        part: Option<(&Seg, usize)>,
        snap: &Snapshot,
    ) -> Result<()> {
        let (key, sk) = entry;
        let stale = self.snap_stale.borrow();
        let live_run = sk.mem != u32::MAX && sk.lrun != NO_RUN && !stale.contains(&sk.mem);
        let frozen_run = sk.frozen != u32::MAX && sk.frun != NO_RUN;
        let mut start = 0usize;
        if tombs {
            let live_tomb = if sk.mem == u32::MAX {
                false
            } else if live_run {
                snap.run_has_tomb(sk.lrun, self.wm())
            } else {
                self.mem()
                    .has_tomb(self.mem().entry(sk.mem as usize), self.wm())
            };
            let frozen_tomb = || {
                if sk.frozen == u32::MAX {
                    false
                } else if frozen_run {
                    snap.run_has_tomb(sk.frun, SEE_ALL)
                } else {
                    self.frozen()
                        .as_ref()
                        .is_some_and(|fr| fr.has_tomb(fr.entry(sk.frozen as usize), SEE_ALL))
                }
            };
            if live_tomb {
                start = 2;
            } else if frozen_tomb() {
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
            if frozen_run {
                snap.run_values(sk.frun, SEE_ALL, |v| f(key, v));
            } else if let Some(fr) = self.frozen() {
                let e = fr.entry(sk.frozen as usize);
                fr.live_offs_into(e, scratch, SEE_ALL);
                for &off in scratch.iter() {
                    f(key, fr.value_at(off));
                }
            }
        }
        if sk.mem != u32::MAX {
            if live_run {
                snap.run_values(sk.lrun, self.wm(), |v| f(key, v));
            } else {
                let e = self.mem().entry(sk.mem as usize);
                self.mem().live_offs_into(e, scratch, self.wm());
                for &off in scratch.iter() {
                    f(key, self.mem().value_at(off));
                }
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
        let st = self.state();
        let (segs, mem) = (&st.segs, &*st.mem);
        let mem_empty = mem.is_empty();
        let hash = if mem_empty { 0 } else { mem.prefetch(key) };
        let np = segs.partition_point(|s| s.level > 0);
        let at = segs[..np].partition_point(|s| s.hi.as_ref().is_some_and(|h| h.as_slice() <= key));
        let part = segs[..np].get(at).filter(|s| s.may_hold(key));
        let l0 = st.pieces_over(np, at);
        let (fr_ix, mem_ix) = (1 + l0.len(), 2 + l0.len());
        let mut start = 0usize;
        if st.has_tombstones() {
            if !mem_empty {
                if let Some(e) = mem.get_with(hash, key) {
                    if mem.has_tomb(e, self.wm()) {
                        start = mem_ix;
                    }
                }
            }
            if start == 0 {
                if let Some(fr) = &st.frozen {
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
            if let Some(fr) = &st.frozen {
                if let Some(e) = fr.get(key) {
                    n += e.count.load(AtomicOrdering::Relaxed);
                }
            }
        }
        if mem_ix >= start && !mem_empty {
            if let Some(e) = mem.get_with(hash, key) {
                n += mem.live_chain(e, self.wm()).0.len() as u64;
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
    #[doc(hidden)]
    pub fn state_gen(&self) -> u64 {
        self.state().gen
    }
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

    /// The name a full-range piece takes as the store's first partition:
    /// what `apply_promotion` would link it under, with the empty fences.
    fn first_partition_name(n: u64, end_seq: u64) -> String {
        format!("par-{n:08}-{end_seq:016}--.sup")
    }

    /// Whether a seal that drains may write the store's first partition
    /// directly: the store holds no segment, so a full-range piece that
    /// carries no tombstone and fits a partition is what the flush's
    /// promotion would link it as, under a second publish. Naming it so
    /// in the seal saves that publish: at ten thousand keys the drain was
    /// a third of the load, and the promotion's link, second open,
    /// directory sync, manifest sync and directory sync a quarter of the
    /// drain.
    fn seals_first_partition(&self) -> bool {
        self.draining
            && self.opts.compact
            && self.opts.partition_on_flush
            && self.opts.promote
            && self.segs().is_empty()
    }

    /// The largest file a partition may be, in bytes.
    fn partition_limit(&self) -> u64 {
        self.opts
            .partition_bytes
            .unwrap_or(self.opts.seal_bytes)
            .max(1) as u64
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
        let segs_tombs = false;
        let state = State {
            forms: Reader::forms_for(&segs),
            forms_moved: std::sync::atomic::AtomicBool::new(false),
            snap: AtomicPtr::new(std::ptr::null_mut()),
            reader_scans: AtomicU64::new(0),
            forms_at: AtomicUsize::new(usize::MAX),
            forms_complete: std::sync::atomic::AtomicBool::new(false),
            scans: AtomicU64::new(0),
            forms_bytes: AtomicUsize::new(0),
            segs,
            mem: std::sync::Arc::new(MemTable::new()),
            frozen: None,
            gen: 1,
            mean_key_bytes,
            store_bytes,
            l0_aligned,
            segs_tombs,
        };
        let shared = std::sync::Arc::new(Shared {
            state: AtomicPtr::new(Box::into_raw(Box::new(state))),
            readers: Readers::new(),
            retired: std::sync::Mutex::new(Vec::new()),
            advice_random: std::sync::atomic::AtomicBool::new(starts_random),
            retired_forms: std::sync::Mutex::new(Vec::new()),
            retired_snaps: std::sync::Mutex::new(Vec::new()),
            snap_builds: AtomicU64::new(0),
            form_takes: AtomicU64::new(0),
            canon_tried: AtomicU64::new(0),
            canon_hit: AtomicU64::new(0),
            rd_scans: AtomicU64::new(0),
            rd_blocks: AtomicU64::new(0),
            scans_life: AtomicU64::new(0),
            live_handles: AtomicUsize::new(0),
            blk_reader: AtomicU64::new(0),
            blk_engine: AtomicU64::new(0),
            snap_extends: AtomicU64::new(0),
            keeper_thread: std::sync::OnceLock::new(),
            keeper_seq: AtomicU64::new(0),
            keeper_done: AtomicU64::new(0),
            snap_kept: AtomicU64::new(0),
            snap_carried: AtomicU64::new(0),
            snap_switched: AtomicU64::new(0),
        });
        let r = Reader {
            shared,
            counted: false,
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
            settle_run: std::cell::RefCell::new(Vec::new()),
            built: std::cell::RefCell::new(Vec::new()),
            dirty_any: std::cell::Cell::new(false),
            tables_complete: std::cell::Cell::new(false),
            publish_due: std::cell::Cell::new(false),
            scans_seen: std::cell::Cell::new(0),
            signalled: std::cell::Cell::new((0, usize::MAX)),
            scans_life_seen: std::cell::Cell::new(0),
            writes_seen: std::cell::Cell::new(0),
            writes_at_scan: std::cell::Cell::new(0),
            log_counted: std::cell::Cell::new((0, 0)),
            choices: std::cell::Cell::new([0; 5]),
            snap_gen: std::cell::Cell::new(0),
            snap_added: std::cell::RefCell::new(Vec::new()),
            snap_stale: std::cell::RefCell::new(std::collections::HashSet::new()),
            log_seen: std::cell::Cell::new(0),
            log_gen: std::cell::Cell::new(0),
            log_bound: std::cell::Cell::new(usize::MAX),
            snap_entries: std::cell::Cell::new(0),
            ahead: std::cell::RefCell::new(None),
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
            built_ahead_len: 0,
            built_ahead_gen: 0,
            pending_err: None,
            next_seg: 0,
            sealing: None,
            keeper: None,
            compacting: None,
            tiering: None,
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
        // What replayed is what the WAL had committed: a reader under
        // `Latest` sees it from the open on, not from the first commit
        // after. The builder ahead of the reader found it missing, reading
        // a reopened store at its watermark.
        mem.commit();
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
        let segs_tombs = segs.iter().any(|s| s.tombs);
        let max_key = Db::max_key_of(&segs, &mem);
        let ntables = segs.len();
        let state = State {
            forms: Reader::forms_for(&segs),
            forms_moved: std::sync::atomic::AtomicBool::new(false),
            snap: AtomicPtr::new(std::ptr::null_mut()),
            reader_scans: AtomicU64::new(0),
            forms_at: AtomicUsize::new(usize::MAX),
            forms_complete: std::sync::atomic::AtomicBool::new(false),
            scans: AtomicU64::new(0),
            forms_bytes: AtomicUsize::new(0),
            segs,
            mem: std::sync::Arc::new(mem),
            frozen: None,
            gen: 1,
            mean_key_bytes,
            store_bytes,
            l0_aligned,
            segs_tombs,
        };
        let shared = std::sync::Arc::new(Shared {
            state: AtomicPtr::new(Box::into_raw(Box::new(state))),
            readers,
            retired: std::sync::Mutex::new(Vec::new()),
            advice_random: std::sync::atomic::AtomicBool::new(starts_random),
            retired_forms: std::sync::Mutex::new(Vec::new()),
            retired_snaps: std::sync::Mutex::new(Vec::new()),
            snap_builds: AtomicU64::new(0),
            form_takes: AtomicU64::new(0),
            canon_tried: AtomicU64::new(0),
            canon_hit: AtomicU64::new(0),
            rd_scans: AtomicU64::new(0),
            rd_blocks: AtomicU64::new(0),
            scans_life: AtomicU64::new(0),
            live_handles: AtomicUsize::new(0),
            blk_reader: AtomicU64::new(0),
            blk_engine: AtomicU64::new(0),
            snap_extends: AtomicU64::new(0),
            keeper_thread: std::sync::OnceLock::new(),
            keeper_seq: AtomicU64::new(0),
            keeper_done: AtomicU64::new(0),
            snap_kept: AtomicU64::new(0),
            snap_carried: AtomicU64::new(0),
            snap_switched: AtomicU64::new(0),
        });
        let r = Reader {
            shared,
            counted: false,
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
            settle_run: std::cell::RefCell::new(Vec::new()),
            built: std::cell::RefCell::new(Vec::new()),
            dirty_any: std::cell::Cell::new(false),
            tables_complete: std::cell::Cell::new(false),
            publish_due: std::cell::Cell::new(false),
            scans_seen: std::cell::Cell::new(0),
            signalled: std::cell::Cell::new((0, usize::MAX)),
            scans_life_seen: std::cell::Cell::new(0),
            writes_seen: std::cell::Cell::new(0),
            writes_at_scan: std::cell::Cell::new(0),
            log_counted: std::cell::Cell::new((0, 0)),
            choices: std::cell::Cell::new([0; 5]),
            snap_gen: std::cell::Cell::new(0),
            snap_added: std::cell::RefCell::new(Vec::new()),
            snap_stale: std::cell::RefCell::new(std::collections::HashSet::new()),
            log_seen: std::cell::Cell::new(0),
            log_gen: std::cell::Cell::new(0),
            log_bound: std::cell::Cell::new(usize::MAX),
            snap_entries: std::cell::Cell::new(0),
            ahead: std::cell::RefCell::new(None),
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
            built_ahead_len: 0,
            built_ahead_gen: 0,
            pending_err: None,
            next_seg,
            sealing: None,
            keeper: None,
            compacting: None,
            tiering: None,
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
        self.maintain_forms()?;
        self.build_ahead_if_due();
        self.start_keeper();
        self.sweep_retired_forms();
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
        let first_partition = self.seals_first_partition();
        let limit = self.partition_limit();
        let (id, seq) = (d.id, end_seq);
        self.retiring_tmps.push(tmp.clone());
        self.sealing = Some(std::thread::spawn(move || {
            let tombs = w.tombs();
            let ord = w
                .finish()
                .map_err(|e| err(&format!("direct finish: {e}")))?;
            let name = if first_partition && !tombs && std::fs::metadata(&tmp)?.len() <= limit {
                Db::first_partition_name(id, seq)
            } else {
                name
            };
            write_ord(&dir, &name, &ord)?;
            std::fs::hard_link(&tmp, dir.join(&name))?;
            // The directory's entries are made durable by the publish that
            // names the segment in the manifest, before the WAL retires; a
            // sync here as well was one more device round trip a drain
            // waited for.
            Ok(vec![name])
        }));
        Ok(())
    }

    /// Freeze the memtable, rotate the WAL, and hand the frozen table to a
    /// thread that writes it as one immutable segment in today's store
    /// format -- fsync, rename into place (the name carrying the covered
    /// end-sequence); the join publishes the manifest and syncs the
    /// directory, then the rotated-out WAL is retired.
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
        let first_partition = self.seals_first_partition();
        let limit = self.partition_limit();
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
                let tombs;
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
                    tombs = w.tombs();
                    ord = w.finish().map_err(|e| err(&format!("seal finish: {e}")))?;
                }
                let whole = ranges.len() == 1 && lo.is_empty() && hi.is_none();
                let name = if whole
                    && first_partition
                    && !tombs
                    && std::fs::metadata(&tmp)?.len() <= limit
                {
                    Db::first_partition_name(id, end_seq)
                } else if whole {
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
            // The directory's entries are made durable by the publish that
            // names the segments in the manifest, before the WAL retires.
            Ok(names)
        }));
        Ok(())
    }

    /// EXPERIMENT: a builder started from a commit once the writes have
    /// piled up, so the organisation the first read would otherwise
    /// build -- the snapshot of the unsealed keys and a form per block
    /// they overlay -- is made on a core the writer is not using, before
    /// a read asks. A builder still running is left to finish; the
    /// count is the memtable's, so a seal's fresh table starts the next
    /// one over.
    fn build_ahead_if_due(&mut self) {
        let due = self.opts.build_ahead_on_commit;
        let on_publish = self.opts.build_ahead_on_publish;
        if (due == 0 && !on_publish) || !self.opts.scan_block_cache {
            return;
        }
        let running = self
            .ahead
            .borrow()
            .as_ref()
            .is_some_and(|a| a.handle.as_ref().is_some_and(|h| !h.is_finished()));
        if running {
            return;
        }
        let len = self.mem().len();
        if len < self.built_ahead_len {
            // A seal: the memtable is new and the count starts over.
            self.built_ahead_len = 0;
        }
        // A publish stops the builder and retires the forms it made, so
        // the state that replaces it has none and the writes it carries
        // are nobody's: a generation this handle has not organised is
        // due whatever the memtable has taken since. Without this the
        // builder started at a commit, the seal's own publish killed it,
        // and no later commit was a burst away from the last -- a store
        // with a hundred pieces reached its reads with one block built.
        let gen = self.state().gen;
        let fresh = gen != self.built_ahead_gen;
        if fresh && !on_publish && self.built_ahead_gen != 0 {
            // Restarted by the count alone: the publish only resets it.
            self.built_ahead_gen = gen;
            self.built_ahead_len = len;
            return;
        }
        if !fresh && (due == 0 || len - self.built_ahead_len < due) {
            return;
        }
        self.built_ahead_gen = gen;
        self.built_ahead_len = len;
        self.start_ahead();
    }

    /// Wait for whatever seal and merge are in flight, starting nothing new
    /// (a joined seal may still trigger a merge when compaction is on and
    /// the level-0 count says so). For an experiment that wants a store in a
    /// known shape before it measures.
    pub fn settle(&mut self) -> Result<()> {
        self.join_seal()?;
        self.join_compact()?;
        self.join_tier()?;
        self.join_ahead();
        self.settle_keeper();
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
        self.join_tier()?;
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
            self.join_tier()?;
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
            self.maybe_tier()?;
        }
        Ok(())
    }

    /// Partitions first in key order, then L0 by (range, age). `read_all`
    /// binary-searches the first group and walks a contiguous run of the
    /// second, so both depend on this order.
    /// The segment set `segs`, sorted, its derived quantities refreshed,
    /// published as the state: the writer's one way to change the
    /// segments. Every block table is dropped with it.
    fn publish_segs(&mut self, segs: Vec<std::sync::Arc<Seg>>) {
        self.publish_segs_with(segs, false)
    }

    /// `publish_segs`, and with `tier` the publish of a piece merge: the
    /// partitions, the memtables and every key's values are what they
    /// were, only the files some of them sit in have changed, so the forms
    /// and the writer's tables are carried across it whatever
    /// `forms_carry` says, and the pieces' bounds alone are walked again.
    fn publish_segs_with(&mut self, mut segs: Vec<std::sync::Arc<Seg>>, tier: bool) {
        segs.sort_by(|a, b| seg_order(a, b));
        let mean_key_bytes = Db::mean_key_bytes_of(&segs);
        let store_bytes = Db::store_bytes_of(&self.dir, &segs);
        let l0_aligned = Db::l0_aligned_of(&segs);
        let segs_tombs = segs.iter().any(|s| s.tombs);
        let cur = self.state();
        let next = State {
            forms: Reader::forms_for(&segs),
            forms_moved: std::sync::atomic::AtomicBool::new(false),
            snap: AtomicPtr::new(std::ptr::null_mut()),
            reader_scans: AtomicU64::new(cur.reader_scans.load(AtomicOrdering::Relaxed)),
            forms_at: AtomicUsize::new(usize::MAX),
            forms_complete: std::sync::atomic::AtomicBool::new(false),
            scans: AtomicU64::new(0),
            forms_bytes: AtomicUsize::new(0),
            segs,
            mem: cur.mem.clone(),
            frozen: cur.frozen.clone(),
            gen: cur.gen + 1,
            mean_key_bytes,
            store_bytes,
            l0_aligned,
            segs_tombs,
        };
        let mut next = next;
        if !self.carry_forms(&mut next, tier) {
            self.drop_blocks();
            *self.tables.borrow_mut() = Db::tables_for(next.segs.len());
        }
        self.publish_and_organise(next);
    }

    /// `next` becomes the state; the one before it is retired at the
    /// epoch this bumps and freed once no reader is pinned before it.
    /// EXPERIMENT: after a publish, which stopped the builder and
    /// retired its forms, organise the state that replaced it.
    fn publish_and_organise(&mut self, next: State) {
        self.publish_state(next);
        self.build_ahead_if_due();
    }

    fn publish_state(&mut self, next: State) {
        // The builder holds the state it builds over, and its forms are
        // of that state: stopped before the swap, so its handle is gone
        // before the state it pinned is retired.
        self.stop_ahead();
        let p = Box::into_raw(Box::new(next));
        let old = self.shared.state.swap(p, AtomicOrdering::AcqRel);
        let tag = self.shared.readers.bump();
        let mut retired = self.shared.retired.lock().expect("the retired list");
        // SAFETY: published by this writer, owned by it until freed.
        retired.push((tag, unsafe { Box::from_raw(old) }));
        let readers = &self.shared.readers;
        retired.retain(|(t, _)| !readers.none_before(*t));
        drop(retired);
        self.sweep_retired_forms();
        self.wake_keeper();
    }

    /// EXPERIMENT: free the replaced canonical forms no pinned reader can
    /// still be walking: those retired at an epoch every pinned slot is
    /// past. The epoch is bumped first, so a form retired under the
    /// current epoch can be freed once its readers leave.
    fn sweep_retired_forms(&self) {
        self.sweep_retired_snaps();
        let mut retired = self.shared.retired_forms.lock().expect("the retired forms");
        if retired.is_empty() {
            return;
        }
        self.shared.readers.bump();
        let readers = &self.shared.readers;
        retired.retain(|(t, f)| {
            if readers.none_before(t + 1) {
                // SAFETY: replaced by the writer, unreachable since, and
                // every reader that could hold it has left.
                drop(unsafe { Box::from_raw(f.0) });
                false
            } else {
                true
            }
        });
    }

    /// EXPERIMENT: scan snapshots built over this store's life. Sharing
    /// one is meant to hold this at one per state however many handles
    /// read it, which is what a test asks.
    pub fn snapshot_builds(&self) -> u64 {
        self.shared.snap_builds.load(AtomicOrdering::Relaxed)
    }

    /// EXPERIMENT: snapshots carried forward by a merge rather than a
    /// sort of everything; see `Snapshot::extend`.
    pub fn snapshot_extends(&self) -> u64 {
        self.shared.snap_extends.load(AtomicOrdering::Relaxed)
    }

    /// EXPERIMENT: reads that took a canonical form over this store's
    /// life, as against `canonical_forms`'s third field, which is the
    /// current state's alone and zero again after every publish.
    pub fn form_takes(&self) -> u64 {
        self.shared.form_takes.load(AtomicOrdering::Relaxed)
            + self.shared.readers.stat(|s| &s.takes)
    }

    /// EXPERIMENT: scans through a caller's handle that reached the test
    /// for walking the forms, and the ones where the forms were current
    /// to the log position the handle holds.
    pub fn canonical_tries(&self) -> (u64, u64) {
        (
            self.shared.canon_tried.load(AtomicOrdering::Relaxed)
                + self.shared.readers.stat(|s| &s.tried),
            self.shared.canon_hit.load(AtomicOrdering::Relaxed)
                + self.shared.readers.stat(|s| &s.hit),
        )
    }

    /// EXPERIMENT: scans through a caller's handle, and those of them
    /// that took the block path at all.
    /// EXPERIMENT: blocks materialised by a caller's handle and by the
    /// engine's own; see `Shared::blk_reader`.
    pub fn blocks_built(&self) -> (u64, u64) {
        (
            self.shared.blk_reader.load(AtomicOrdering::Relaxed)
                + self.shared.readers.stat(|s| &s.built),
            self.shared.blk_engine.load(AtomicOrdering::Relaxed),
        )
    }

    pub fn reader_scans(&self) -> (u64, u64) {
        (
            self.shared.rd_scans.load(AtomicOrdering::Relaxed)
                + self.shared.readers.stat(|s| &s.scans),
            self.shared.rd_blocks.load(AtomicOrdering::Relaxed)
                + self.shared.readers.stat(|s| &s.blockpath),
        )
    }

    /// EXPERIMENT: the log position the canonical forms are settled to,
    /// or `usize::MAX` before the first maintenance: a commit that
    /// maintained them moves it to the commit's own position, one that
    /// did not leaves it where it was.
    pub fn forms_position(&self) -> usize {
        self.state().forms_at.load(AtomicOrdering::Acquire)
    }

    /// PROTOTYPE: how many states a publish replaced are still held for a
    /// reader, for a test to hold the reader table to its word.
    pub fn retired_states(&self) -> usize {
        self.shared.retired.lock().expect("the retired list").len()
    }
}

impl Reader {
    /// A handle that reads this store from any thread, under `Latest`
    /// isolation to begin with, with caches of its own and a slot in the
    /// reader table for its life. It is `Send` and not `Sync`: one
    /// thread reads through it at a time, and a thread that wants its
    /// own asks for its own. Fails when every slot is taken.
    pub fn reader(&self) -> Result<Reader> {
        self.new_reader(true)
    }

    /// A handle for the engine's own use -- the builder ahead's -- which
    /// claims a slot like any other but is not one of the caller's, so it
    /// cannot be what turns the maintenance regime on for itself.
    fn new_reader(&self, counted: bool) -> Result<Reader> {
        let slot = self
            .shared
            .readers
            .claim()
            .ok_or_else(|| err("reader table: every slot is taken"))?;
        if counted {
            self.shared
                .live_handles
                .fetch_add(1, AtomicOrdering::Relaxed);
            self.publish_for_handle();
        }
        Ok(Reader {
            shared: self.shared.clone(),
            counted,
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
            settle_run: std::cell::RefCell::new(Vec::new()),
            built: std::cell::RefCell::new(Vec::new()),
            dirty_any: std::cell::Cell::new(false),
            tables_complete: std::cell::Cell::new(false),
            publish_due: std::cell::Cell::new(false),
            scans_seen: std::cell::Cell::new(0),
            signalled: std::cell::Cell::new((0, usize::MAX)),
            scans_life_seen: std::cell::Cell::new(0),
            writes_seen: std::cell::Cell::new(0),
            writes_at_scan: std::cell::Cell::new(0),
            log_counted: std::cell::Cell::new((0, 0)),
            choices: std::cell::Cell::new([0; 5]),
            snap_gen: std::cell::Cell::new(0),
            snap_added: std::cell::RefCell::new(Vec::new()),
            snap_stale: std::cell::RefCell::new(std::collections::HashSet::new()),
            log_seen: std::cell::Cell::new(0),
            log_gen: std::cell::Cell::new(0),
            log_bound: std::cell::Cell::new(usize::MAX),
            snap_entries: std::cell::Cell::new(0),
            ahead: std::cell::RefCell::new(None),
        })
    }
}

impl Db {
    /// The state with `mem` as the live memtable.
    fn set_mem(&mut self, mem: std::sync::Arc<MemTable>) {
        let cur = self.state();
        let next = State {
            forms: Reader::forms_for(&cur.segs),
            forms_moved: std::sync::atomic::AtomicBool::new(false),
            snap: AtomicPtr::new(std::ptr::null_mut()),
            reader_scans: AtomicU64::new(cur.reader_scans.load(AtomicOrdering::Relaxed)),
            forms_at: AtomicUsize::new(usize::MAX),
            forms_complete: std::sync::atomic::AtomicBool::new(false),
            scans: AtomicU64::new(0),
            forms_bytes: AtomicUsize::new(0),
            segs: cur.segs.clone(),
            mem,
            frozen: cur.frozen.clone(),
            gen: cur.gen + 1,
            mean_key_bytes: cur.mean_key_bytes,
            store_bytes: cur.store_bytes,
            l0_aligned: cur.l0_aligned,
            segs_tombs: cur.segs_tombs,
        };
        self.publish_and_organise(next);
    }

    /// The state with `frozen` as the frozen memtable.
    fn set_frozen(&mut self, frozen: Option<std::sync::Arc<MemTable>>) {
        let cur = self.state();
        let next = State {
            forms: Reader::forms_for(&cur.segs),
            forms_moved: std::sync::atomic::AtomicBool::new(false),
            snap: AtomicPtr::new(std::ptr::null_mut()),
            reader_scans: AtomicU64::new(cur.reader_scans.load(AtomicOrdering::Relaxed)),
            forms_at: AtomicUsize::new(usize::MAX),
            forms_complete: std::sync::atomic::AtomicBool::new(false),
            scans: AtomicU64::new(0),
            forms_bytes: AtomicUsize::new(0),
            segs: cur.segs.clone(),
            mem: cur.mem.clone(),
            frozen,
            gen: cur.gen + 1,
            mean_key_bytes: cur.mean_key_bytes,
            store_bytes: cur.store_bytes,
            l0_aligned: cur.l0_aligned,
            segs_tombs: cur.segs_tombs,
        };
        let mut next = next;
        if !self.carry_forms(&mut next, false) {
            self.drop_blocks();
        }
        self.publish_and_organise(next);
    }

    /// EXPERIMENT: the canonical forms and the writer's own tables
    /// carried into `next`, a state over the same partitions; see
    /// `Options::forms_carry`. The backlog is filed and published first,
    /// so every form is current to the whole log, and the state's
    /// pointers move to `next`, which owns them from its publish; the
    /// writer's tables keep their forms, drop what named the old
    /// memtable's slots -- the wide forms, the copies, the lists of keys
    /// filed since the snapshot -- and walk the pieces' bounds again
    /// against `next`'s pieces. Returns whether it was done; when not,
    /// the caller starts the table afresh and drops the writer's own, as
    /// every publish did before.
    /// With `tier`, the carry a piece merge's publish makes whatever
    /// `forms_carry` says: the memtable is the one it was, so the keys
    /// filed since the snapshot, the snapshot and this handle's log
    /// position all stand, and only the pieces' bounds are walked again.
    fn carry_forms(&mut self, next: &mut State, tier: bool) -> bool {
        if !((self.opts.forms_carry || tier)
            && self.opts.commit_forms
            && self.opts.scan_block_cache)
        {
            return false;
        }
        let cur = self.state();
        let np = cur.segs.partition_point(|s| s.level > 0);
        if np == 0 || cur.forms.len() < np {
            return false;
        }
        let same = next.segs.partition_point(|s| s.level > 0) == np
            && cur.segs[..np]
                .iter()
                .zip(&next.segs[..np])
                .all(|(a, b)| std::sync::Arc::ptr_eq(a, b));
        if !same {
            return false;
        }
        // The forms were published from this handle's tables: none, or
        // tables of another generation, and nothing current is here to
        // carry.
        if !self.cache_used.get() || self.log_gen.get() != cur.gen {
            return false;
        }
        // Everything logged, filed and published.
        self.sync_log();
        if self.settle_pending().is_err() {
            return false;
        }
        self.publish_dirty(true);
        // The writer's tables, remade for `next`: forms kept, the rest
        // walked again. A failure here leaves nothing carried.
        let l0 = &next.segs[np..];
        let ctx = self.build_ctx();
        let mut bounds = Vec::with_capacity(np);
        for seg in &next.segs[..np] {
            match ctx.table_bounds(seg, l0) {
                Ok(b) => bounds.push(b),
                Err(_) => return false,
            }
        }
        self.tables
            .borrow_mut()
            .resize_with(next.segs.len(), || std::cell::RefCell::new(None));
        for (p, (pieces, piece_ranks)) in bounds.into_iter().enumerate() {
            let tables = self.tables.borrow();
            let mut held = tables[p].borrow_mut();
            let Some(t) = held.as_mut() else { continue };
            for b in 0..t.slots.len() {
                if matches!(t.slots[b].as_deref(), Some(Cached::Wide(_))) {
                    self.unlist(p, b, t);
                }
                if let Some(old) = t.dense[b].take() {
                    self.cache_bytes
                        .set(self.cache_bytes.get().saturating_sub(old.bytes()));
                    let mut c = self.choices.get();
                    c[4] = c[4].saturating_sub(old.bytes() as u64);
                    self.choices.set(c);
                }
            }
            t.pieces = pieces;
            t.piece_ranks = piece_ranks;
            if tier {
                continue;
            }
            t.snap_at = std::cell::OnceCell::new();
            t.snap_span = (0, 0);
            t.snap_gen = u64::MAX;
            t.reads.fill(0);
            t.touched.fill(0);
            for list in &mut t.added {
                list.clear();
            }
            t.filed = 0;
        }
        // The pointers, `next`'s from its publish; `cur` frees none.
        let mut bytes = 0usize;
        for p in 0..np {
            for (b, slot) in cur.forms[p].iter().enumerate() {
                let ptr = slot.load(AtomicOrdering::Acquire);
                if ptr.is_null() {
                    continue;
                }
                // SAFETY: as in `canonical`; owned by `next` from here.
                bytes += unsafe { &*ptr }.bytes;
                next.forms[p][b].store(ptr, AtomicOrdering::Relaxed);
            }
        }
        cur.forms_moved.store(true, AtomicOrdering::Release);
        next.forms_at = AtomicUsize::new(0);
        next.forms_complete = std::sync::atomic::AtomicBool::new(self.tables_complete.get());
        next.forms_bytes = AtomicUsize::new(bytes);
        // This handle's log and snapshot bookkeeping, for `next`: across a
        // seal the log is read from its start and the snapshot built
        // again; across a piece merge the memtable is the same one, so
        // the position, the snapshot and the lists stand and only the
        // generation they are keyed by moves.
        self.log_gen.set(next.gen);
        if tier {
            if let Some((g, _)) = self.scan_keys.borrow_mut().as_mut() {
                *g = next.gen;
            }
            return true;
        }
        self.log_seen.set(0);
        self.scans_seen.set(0);
        self.snap_entries.set(0);
        *self.scan_keys.borrow_mut() = None;
        self.snap_added.borrow_mut().clear();
        self.pending.borrow_mut().clear();
        true
    }

    /// The live memtable frozen and a fresh one live, in one publish;
    /// the frozen one returned for the seal.
    fn freeze(&mut self) -> std::sync::Arc<MemTable> {
        let cur = self.state();
        let frozen = cur.mem.clone();
        let next = State {
            forms: Reader::forms_for(&cur.segs),
            forms_moved: std::sync::atomic::AtomicBool::new(false),
            snap: AtomicPtr::new(std::ptr::null_mut()),
            reader_scans: AtomicU64::new(cur.reader_scans.load(AtomicOrdering::Relaxed)),
            forms_at: AtomicUsize::new(usize::MAX),
            forms_complete: std::sync::atomic::AtomicBool::new(false),
            scans: AtomicU64::new(0),
            forms_bytes: AtomicUsize::new(0),
            segs: cur.segs.clone(),
            mem: std::sync::Arc::new(MemTable::new()),
            frozen: Some(frozen.clone()),
            gen: cur.gen + 1,
            mean_key_bytes: cur.mean_key_bytes,
            store_bytes: cur.store_bytes,
            l0_aligned: cur.l0_aligned,
            segs_tombs: cur.segs_tombs,
        };
        let mut next = next;
        if !self.carry_forms(&mut next, false) {
            // The scan snapshot names the live memtable's slots, and the
            // live memtable is new: a write's bookkeeping renumbers the
            // snapshot at every rehash, and the fresh table's first
            // rehash has a thousand slots where the snapshot names
            // hundreds of thousands. It stood stale until the next scan
            // rebuilt it, and six hundred inserts between a seal and
            // that scan were enough to index past the map.
            *self.scan_keys.borrow_mut() = None;
            self.snap_added.borrow_mut().clear();
            self.snap_stale.borrow_mut().clear();
            self.drop_blocks();
        }
        self.publish_and_organise(next);
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
        // Level 0 is newer than the level below it, every piece of it, so
        // a partition merge may take only a prefix of a range's pieces by
        // age: the pieces present when it starts. A piece merge in flight
        // holds some of them, and a partition merge started beside it took
        // the piece sealed after them and folded it under them -- the
        // oracle read a key's values out of order, and lost a deleted
        // key's older values to a tombstone the merge had dropped. So no
        // partition merge starts while a piece merge runs; a finished one
        // is collected first, since the piece it made is what the count
        // below sees. The other way round is safe: a partition merge's
        // inputs are the range's oldest pieces, so the pieces a merge
        // beside it takes are all newer.
        match &self.tiering {
            Some((_, h)) if h.is_finished() => self.join_tier()?,
            Some(_) => return Ok(()),
            None => {}
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
        Ok(())
    }

    /// Name the live set durably. Everything before this call is a file on
    /// disk that nothing reaches; everything after it is the store.
    fn publish(&mut self) -> Result<()> {
        self.seal_wait.publishes += 1;
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
        // No piece merge runs when a partition merge starts, so the pieces
        // taken here are every piece of the ranges, the oldest included.
        debug_assert!(
            self.tiering.is_none(),
            "a partition merge started beside a piece merge"
        );
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
}

impl Reader {
    /// PROTOTYPE: the builder started over the state as of the last
    /// commit, by the writer's own handle at its first scan on the block
    /// path over a state: a reader handle of its own, pinned to this
    /// generation under the commit's watermark, the write log's length at
    /// the commit as the first write its forms lack. Once, per state: an
    /// installed form is kept current by the settle of every write after,
    /// so a second builder over the same state has nothing to add, and
    /// one that ran anyway cost the scans beside it a quarter at three
    /// million keys.
    fn start_ahead(&self) {
        if self.slot.is_some() {
            return;
        }
        self.stop_ahead();
        if self.segs().first().is_none_or(|s| s.level == 0) {
            return;
        }
        // A store too small for a builder: its run beside the scans costs
        // them more than the builds it saves; the option's doc has the
        // measurement.
        let blocks: usize = self
            .segs()
            .iter()
            .take_while(|s| s.level > 0)
            .map(|s| s.blob.keys().div_ceil(CACHE_BLOCK))
            .sum();
        if blocks < self.opts.scan_cache_ahead_min_blocks {
            return;
        }
        let gen = self.state().gen;
        // Nothing to build: with no piece and no unsealed key every block
        // is clean, and the job would spawn a thread to find so -- 60 us
        // of the first scan over a store just flushed, a third of that
        // scan at three hundred thousand keys, and the same walk over
        // every partition's blocks on the thread. The state's one run is
        // spent all the same, as a builder that found nothing would
        // leave it: the writes after this scan are the commit's to
        // settle, not a builder's, and a builder started at the first
        // scan with something unsealed instead ran beside the scans of
        // the lag sweep's first point, which had never had one.
        if self.segs().iter().all(|s| s.level > 0)
            && self.mem().is_empty()
            && self.frozen().is_none()
        {
            let (_tx, rx) = std::sync::mpsc::channel();
            *self.ahead.borrow_mut() = Some(Ahead {
                handle: None,
                rx,
                stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
                from: self.mem().committed_log(),
                since: std::cell::RefCell::new(Since::default()),
                done: std::cell::Cell::new(true),
                posted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            return;
        }
        let Ok(r) = self.new_reader(false) else {
            return;
        };
        let mem = self.mem();
        // The log's length first and the watermark last, the order a
        // commit stores them in reversed, so the length and the count
        // belong to the watermark's commit or an earlier one: a length
        // from a later commit would leave the keys logged between the
        // two outside the builder's forms and outside the splice.
        let from = mem.committed_log();
        let live_len = mem.committed_len();
        let wm = mem.committed();
        // Writes logged past the commit that this handle has read already
        // -- a batch staged and not committed, which the writer's own
        // scans see -- are filed now; the log is read on from where it
        // was, so nothing is filed twice.
        let mut since = Since::default();
        if self.log_gen.get() == gen {
            for i in from..self.log_seen.get() {
                let (id, _) = mem.log_at(i);
                let e = mem.entry(id);
                since.file(mem, e.key_off, e.key_len);
            }
        }
        // The blocks this handle holds a form for: the builder has nothing
        // to build for them, whether or not they are published.
        let held: Vec<Vec<bool>> = self
            .tables
            .borrow()
            .iter()
            .map(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|t| t.slots.iter().map(Option::is_some).collect())
                    .unwrap_or_default()
            })
            .collect();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let posted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let post = posted.clone();
        let handle = std::thread::spawn(move || {
            let at = AtCommit {
                gen,
                wm,
                log: from,
                len: live_len,
            };
            let _ = r.build_ahead_job(at, &flag, &tx, &post, &held);
            drop(tx);
            post.store(true, std::sync::atomic::Ordering::Release);
        });
        *self.ahead.borrow_mut() = Some(Ahead {
            handle: Some(handle),
            rx,
            stop,
            from,
            since: std::cell::RefCell::new(since),
            done: std::cell::Cell::new(false),
            posted,
        });
    }

    /// PROTOTYPE: the builder told to stop and joined, its forms dropped.
    fn stop_ahead(&self) {
        if let Some(mut a) = self.ahead.borrow_mut().take() {
            a.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// PROTOTYPE: the builder waited for, its forms kept for the next
    /// scan to install. What `settle` does, for a test or an experiment
    /// that wants the cache as the builder leaves it.
    fn join_ahead(&self) {
        if let Some(a) = self.ahead.borrow_mut().as_mut() {
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }
}

impl Db {
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
type PieceBounds = Vec<(usize, std::sync::Arc<Vec<u32>>)>;
/// PROTOTYPE: a piece's run over one block, as a build merges it: the
/// piece's index in the level, the run of its ranks, and its ranks
/// against the partition when it has them.
type PieceRun<'a> = (usize, std::ops::Range<usize>, Option<&'a [u32]>);
/// Where a sorted source's positions fall against a partition's block
/// boundaries, by the partition's blob id: what a piece keeps against
/// each partition it meets and a snapshot keeps for its main run.
type BoundsById = Vec<(u64, std::sync::Arc<Vec<u32>>)>;

/// PROTOTYPE: a snapshot's main run against one partition: where the
/// run's positions fall against the partition's block boundaries, and
/// where each key of the run cuts the partition's walk, encoded as
/// `owner_of` encodes a cut. Both are functions of the run and the
/// partition, taken once in one forward walk along the partition's index
/// heads and kept with the snapshot under the partition's blob id. The
/// cuts were searched for at every build instead, a binary search over
/// the block's heads per snapshot key, six mispredicted branches a key
/// by the simulator's count and a quarter of a build's.
struct SnapBounds {
    at: Vec<u32>,
    cuts: Vec<u32>,
}
type SnapById = Vec<(u64, std::sync::Arc<SnapBounds>)>;

/// PROTOTYPE: per entry of a `PieceBounds`, the piece's ranks against the
/// partition, as `BlockTable::piece_ranks` holds them.
type PieceRanks = Vec<Option<std::sync::Arc<Vec<u32>>>>;

/// PROTOTYPE: a builder ahead of the reader in flight: its thread while
/// it runs, the forms it sends, the flag that stops it, the write log's
/// length its forms cover, and the keys written from there on, for the
/// install to splice into them.
struct Ahead {
    handle: Option<std::thread::JoinHandle<()>>,
    rx: std::sync::mpsc::Receiver<Built>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The write log's length at the commit the builder took its
    /// watermark from: every write logged from here on is one its forms
    /// lack, filed into `since` as the log is read, until every form is
    /// installed or dropped and `done` says so.
    from: usize,
    since: std::cell::RefCell<Since>,
    done: std::cell::Cell<bool>,
    /// Raised by the builder after each form it sends and once more when
    /// it ends, taken down by the install that drains the channel: a scan
    /// with nothing posted asks the channel nothing. Polling the channel
    /// at every scan cost the scans a tenth at three hundred thousand
    /// keys, most of it after the builder was done.
    posted: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// PROTOTYPE: the keys written since a builder ahead took its watermark,
/// each as (key offset, key length) in the live memtable's arena, a run
/// of writes to one key filed once, in key order once settled so an
/// install finds a block's run of them by two searches. Its memory is
/// the writes between two builders, and it leaves with the builder.
struct Since {
    /// The keys' bytes, copied as they are filed: a search over the
    /// memtable's arena for each compare read twenty-two cold lines an
    /// install, a microsecond, where the copies are a few thousand bytes
    /// that stay in cache.
    keys: Vec<u8>,
    /// (offset, length) into `keys`, in key order once settled.
    sorted: Vec<(u32, u32)>,
    fresh: Vec<(u32, u32)>,
    /// The memtable offset last filed, so a run of writes to one key
    /// files it once.
    last_off: u32,
}

impl Default for Since {
    fn default() -> Since {
        Since {
            keys: Vec::new(),
            sorted: Vec::new(),
            fresh: Vec::new(),
            last_off: u32::MAX,
        }
    }
}

impl Since {
    fn file(&mut self, mem: &MemTable, off: u32, len: u32) {
        if off == self.last_off {
            return;
        }
        self.last_off = off;
        let at = self.keys.len() as u32;
        self.keys.extend_from_slice(mem.key_at(off, len));
        self.fresh.push((at, len));
    }
    fn key(&self, e: (u32, u32)) -> &[u8] {
        &self.keys[e.0 as usize..(e.0 + e.1) as usize]
    }
    /// The keys filed since the last settle, sorted and folded into the
    /// sorted run, one entry per key.
    fn settle(&mut self) {
        if self.fresh.is_empty() {
            return;
        }
        let mut fresh = std::mem::take(&mut self.fresh);
        fresh.sort_unstable_by(|&a, &b| self.key(a).cmp(self.key(b)));
        fresh.dedup_by(|&mut a, &mut b| self.key(a) == self.key(b));
        let sorted = std::mem::take(&mut self.sorted);
        let mut out = Vec::with_capacity(sorted.len() + fresh.len());
        let (mut i, mut j) = (0, 0);
        while i < sorted.len() && j < fresh.len() {
            match self.key(sorted[i]).cmp(self.key(fresh[j])) {
                Ordering::Less => {
                    out.push(sorted[i]);
                    i += 1;
                }
                Ordering::Greater => {
                    out.push(fresh[j]);
                    j += 1;
                }
                Ordering::Equal => {
                    out.push(fresh[j]);
                    i += 1;
                    j += 1;
                }
            }
        }
        out.extend_from_slice(&sorted[i..]);
        out.extend_from_slice(&fresh[j..]);
        self.sorted = out;
    }
    fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }
    fn first(&self) -> Option<&[u8]> {
        self.sorted.first().map(|&e| self.key(e))
    }
    /// Whether any settled key could fall in `[lo, hi)`: one compare at
    /// each end before a search, since nearly every block's range holds
    /// none of the keys written since.
    fn meets(&self, lo: Option<&[u8]>, hi: Option<&[u8]>) -> bool {
        match (self.sorted.first(), self.sorted.last()) {
            (Some(&f), Some(&l)) => {
                hi.is_none_or(|hi| self.key(f) < hi) && lo.is_none_or(|lo| self.key(l) >= lo)
            }
            _ => false,
        }
    }
    /// The settled keys in `[lo, hi)`, unbounded on a side given as
    /// `None`, as a range of indexes into the sorted run.
    fn range(&self, lo: Option<&[u8]>, hi: Option<&[u8]>) -> std::ops::Range<usize> {
        let a = lo.map_or(0, |lo| self.sorted.partition_point(|&e| self.key(e) < lo));
        let b = hi.map_or(self.sorted.len(), |hi| {
            self.sorted.partition_point(|&e| self.key(e) < hi)
        });
        a..b.max(a)
    }
    fn at(&self, i: usize) -> &[u8] {
        self.key(self.sorted[i])
    }
}

/// PROTOTYPE: one form built ahead: the state's generation it was built
/// at, the partition by name, the block, and the form.
struct Built {
    gen: u64,
    name: String,
    /// (block, bytes, form), up to `AHEAD_BATCH` of one partition's: a
    /// message per form cost the install 150 ns to receive each and a
    /// name each, and the form's size is read here so the install does
    /// not touch the form's buffers, which another core wrote.
    forms: Vec<(u32, usize, Cached)>,
}

/// PROTOTYPE: forms the builder sends in one message.
const AHEAD_BATCH: usize = 64;

/// PROTOTYPE: a commit as the builder ahead holds it: the state's
/// generation, the watermark, the write log's length and the entry
/// count, read in the order the commit stores them reversed.
struct AtCommit {
    gen: u64,
    wm: u64,
    log: usize,
    len: usize,
}

impl Reader {
    /// PROTOTYPE: the builder ahead of the reader, on a handle of its
    /// own: the state of generation `gen` held under the last commit's
    /// watermark `wm`, a snapshot of the `live_len` entries that commit
    /// covers, and every block a piece or an unsealed key overlays built
    /// from the partitions, the pieces and the memtables, each form sent
    /// back as it is built. Stops when told, when the state has moved on
    /// before it held it, or when the store has dropped the channel's
    /// other end. A block with no overlay is clean and needs no form;
    /// one past the wide bound is the store's to walk.
    fn build_ahead_job(
        &self,
        at: AtCommit,
        stop: &std::sync::atomic::AtomicBool,
        tx: &std::sync::mpsc::Sender<Built>,
        posted: &std::sync::atomic::AtomicBool,
        held: &[Vec<bool>],
    ) -> Result<()> {
        let AtCommit { gen, wm, log, len } = at;
        if !self.pin_at(gen, wm, log) {
            return Ok(());
        }
        // The builder needs the keys up to `len` exactly, so it takes a
        // published snapshot only to carry it forward: one that stops
        // short is merged with the batch since, and one past `len` holds
        // keys this commit does not cover and is no use. It publishes
        // what it ends with, which is the point of it -- this work is on
        // a core the reads are not using, and a handle that reads the
        // state after takes these keys instead of sorting them again.
        let unsealed = self.snapshot_to(len, self.adopt_snapshot().filter(|s| s.live_len <= len));
        let ctx = BuildCtx {
            copy_dense: true,
            ..self.build_ctx()
        };
        // Under a budget, the forms queued for the install are bounded
        // by it too: the store sheds past the budget only as it installs.
        let budget = self.opts.scan_cache_bytes;
        let mut sent = 0usize;
        ctx.rank_pieces()?;
        let np = self.segs().partition_point(|s| s.level > 0);
        let l0 = &self.segs()[np..];
        for (pi, seg) in self.segs()[..np].iter().enumerate() {
            let nblocks = seg.blob.keys().div_ceil(CACHE_BLOCK);
            let (pieces, piece_ranks) = ctx.table_bounds(seg, l0)?;
            let table = BlockTable {
                slots: (0..nblocks).map(|_| None).collect(),
                dense: (0..nblocks).map(|_| None).collect(),
                reads: vec![0; nblocks],
                touched: vec![0; nblocks],
                listed: vec![u32::MAX; nblocks],
                last_key: Vec::new(),
                pieces,
                piece_ranks,
                snap_at: std::cell::OnceCell::new(),
                snap_span: BuildCtx::snap_span(seg, &unsealed),
                snap_gen: 0,
                added: (0..nblocks).map(|_| Vec::new()).collect(),
                filed: 0,
                dirty: vec![false; nblocks],
            };
            if table.clean_throughout() {
                continue;
            }
            table.snap_at(seg, &unsealed)?;
            let src = Sources { seg, l0 };
            let mut forms: Vec<(u32, usize, Cached)> = Vec::with_capacity(AHEAD_BATCH);
            for b in 0..=nblocks {
                let full = forms.len() == AHEAD_BATCH || (b == nblocks && !forms.is_empty());
                if full {
                    let built = Built {
                        gen,
                        name: seg.name.clone(),
                        forms: std::mem::replace(&mut forms, Vec::with_capacity(AHEAD_BATCH)),
                    };
                    if tx.send(built).is_err() || (budget > 0 && sent > budget) {
                        return Ok(());
                    }
                    posted.store(true, std::sync::atomic::Ordering::Release);
                }
                if b == nblocks {
                    break;
                }
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }
                // A block the writer holds a form for, or one published
                // already, has nothing to build for.
                let published = self
                    .state()
                    .forms
                    .get(pi)
                    .and_then(|f| f.get(b))
                    .is_some_and(|s| !s.load(AtomicOrdering::Acquire).is_null());
                if published
                    || held
                        .get(pi)
                        .and_then(|h| h.get(b))
                        .copied()
                        .unwrap_or(false)
                {
                    continue;
                }
                let n = BuildCtx::overlay_count(&table, b);
                if n == 0 || n > WIDE {
                    continue;
                }
                let form = ctx.materialize(src, &table, b, &unsealed)?;
                if matches!(form, Cached::Clean | Cached::Wide(_)) {
                    continue;
                }
                let bytes = form.bytes();
                sent += bytes;
                forms.push((b as u32, bytes, form));
            }
        }
        Ok(())
    }
}

/// PROTOTYPE: the run keeper in flight: its thread, and the flag that
/// stops it.
struct Keeper {
    handle: Option<std::thread::JoinHandle<()>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// How long the keeper sleeps between looks at the commits, from the
/// shortest after a tick that did something, doubling while nothing is
/// due, to the longest, which is also how often a dormant keeper looks
/// for a scan.
const KEEPER_POLL_MIN: std::time::Duration = std::time::Duration::from_micros(100);
const KEEPER_POLL_MAX: std::time::Duration = std::time::Duration::from_millis(20);
/// Retired snapshot versions still held by a pinned reader past which
/// the keeper stops publishing; see `keep_tick`.
const KEEPER_RETIRED_MAX: usize = 8;
/// The fewest writes an extension is made for, below a sixteenth of
/// the version's entries; see `keep_tick`.
const KEEPER_MIN_BATCH: usize = 256;

/// What the keeper holds between ticks: the version it last published,
/// the generation and the memtables it is over, the committed log
/// position it was extended to, and the store's scan count as it last
/// saw it with the commit that count was seen at.
#[derive(Default)]
struct Kept {
    snap: Option<std::sync::Arc<Snapshot>>,
    gen: u64,
    live: Option<std::sync::Arc<MemTable>>,
    frozen: Option<std::sync::Arc<MemTable>>,
    seen_log: usize,
    scans_seen: u64,
    scan_at: (u64, usize),
}

/// What a keeper's tick asks for next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// Look again at once: something was done and more may have landed.
    More,
    /// Nothing until a commit.
    Commit,
    /// Nothing until a scan over the store, or a publish.
    Scan,
}

/// Put the calling thread in the idle scheduling class, which runs only
/// when nothing else wants the core. Linux only; elsewhere a no-op, and
/// a failure is ignored on purpose, as `idle_io_priority` ignores its.
fn idle_cpu_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let param: libc::sched_param = std::mem::zeroed();
        let _ = libc::sched_setscheduler(0, libc::SCHED_IDLE, &param);
    }
}

impl Reader {
    /// PROTOTYPE: the run keeper's loop, on a handle of its own. Each
    /// tick pins the state, carries the snapshot across a publish if one
    /// happened, extends it to the last commit once enough has landed
    /// and publishes the result; then the keeper sleeps and looks again,
    /// for as short as `KEEPER_POLL_MIN` after a tick that did
    /// something and doubling while nothing is due, to
    /// `KEEPER_POLL_MAX`, which is also how often a dormant keeper looks
    /// for a scan. Nothing on the commit path or the read path wakes it:
    /// the version before this one was unparked by every commit, and
    /// the wake is a futex call and an interrupt to an idle core, on
    /// this guest a VM exit paid by the thread that woke it -- ycsb-A's
    /// commits at a hundred thousand keys read 12-23% slower for it. A
    /// publish and a settle wake it at once, and a settle's tick brings
    /// the snapshot current whatever the regime or the batch bound says,
    /// and answers on `keeper_done`.
    fn keep(&self, stop: &std::sync::atomic::AtomicBool) {
        let _ = self.shared.keeper_thread.set(std::thread::current());
        let mut k = Kept::default();
        let mut idle = KEEPER_POLL_MIN;
        loop {
            if stop.load(AtomicOrdering::SeqCst) {
                break;
            }
            let flush = self.shared.keeper_seq.load(AtomicOrdering::SeqCst);
            let forced = flush != self.shared.keeper_done.load(AtomicOrdering::Relaxed);
            let want = self.keep_tick(&mut k, forced);
            if forced && want != Want::More {
                self.shared.keeper_done.store(flush, AtomicOrdering::SeqCst);
            }
            match want {
                Want::More => idle = KEEPER_POLL_MIN,
                Want::Commit => {
                    std::thread::park_timeout(idle);
                    idle = (idle * 2).min(KEEPER_POLL_MAX);
                }
                Want::Scan => std::thread::park_timeout(KEEPER_POLL_MAX),
            }
        }
    }

    /// One tick of the keeper: see `keep`.
    fn keep_tick(&self, k: &mut Kept, forced: bool) -> Want {
        let Some(slot) = self.slot else {
            return Want::Scan;
        };
        if self.isolation.get() == Isolation::Snapshot {
            self.release();
        }
        self.shared.readers.pin(slot);
        let p = self.shared.state.load(AtomicOrdering::Acquire);
        self.held.store(p, AtomicOrdering::Relaxed);
        self.isolation.set(Isolation::Snapshot);
        // SAFETY: pinned above, so not freed under this handle.
        let st = unsafe { &*p };
        let mem = &st.mem;
        // A publish since the last tick: the snapshot carried across it
        // where the memtables allow, and dropped otherwise.
        if st.gen != k.gen {
            let carried = k.snap.take().and_then(|s| self.carry_snapshot(s, k, st));
            k.gen = st.gen;
            k.live = Some(mem.clone());
            k.frozen = st.frozen.clone();
            // The carried version's mark, so the next tick extends only
            // for the commits past it; none, where nothing carried.
            k.seen_log = carried.as_ref().map_or(0, |s| s.log_at);
            if let Some(s) = &carried {
                self.shared
                    .snap_carried
                    .fetch_add(1, AtomicOrdering::Relaxed);
                self.publish_snapshot(s);
            }
            k.snap = carried;
        }
        // The regime: nothing before the first scan over the store, and
        // past the bound of writes since the last one, nothing until the
        // next. The scan's position is taken as the commit it was seen
        // at, which is at or past where it happened.
        let life = self.shared.scans_life.load(AtomicOrdering::SeqCst);
        if life != k.scans_seen {
            k.scans_seen = life;
            k.scan_at = (st.gen, mem.committed_log());
        }
        if !forced {
            let pct = self.opts.snapshot_keeper_recent_pct;
            let far = life == 0
                || (pct > 0 && {
                    let keys: usize = st.segs.iter().map(|s| s.blob.keys()).sum();
                    let since = if k.scan_at.0 == st.gen {
                        mem.committed_log().saturating_sub(k.scan_at.1)
                    } else {
                        mem.committed_log()
                    };
                    since > keys / 100 * pct
                });
            if far {
                self.release();
                return Want::Scan;
            }
        }
        // The log's position first and the count after it, the order a
        // commit stores them in, so the position belongs to the count's
        // commit or an earlier one and the next commit moves it.
        let log = mem.committed_log();
        let len = mem.committed_len();
        // Current, or behind by less than a sixteenth of what the version
        // holds: an extension merges the whole entry run, so one per
        // commit over a mix of small batches is a copy of the run per
        // hundred writes -- ycsb-A at a hundred thousand keys read 0.82x
        // beside the runs with the keeper streaming that much -- and
        // what a scan then finds behind is a sixteenth's copy, which it
        // makes for itself. A settle brings it current whatever the lag.
        let behind = k.snap.as_ref().map_or(usize::MAX, |s| {
            len.saturating_sub(s.live_len) + log.saturating_sub(k.seen_log)
        });
        let held = k.snap.as_ref().map_or(0, |s| s.len());
        let current = behind == 0 || (!forced && behind < (held / 16).max(KEEPER_MIN_BATCH));
        if current {
            self.release();
            return Want::Commit;
        }
        // The base: the keeper's own version, or the published one when
        // a scan carried it further and it stops at or before this
        // commit -- the writer's own reads cover what it has staged, and
        // a version past the commit is no base for one at it.
        let published = self
            .adopt_snapshot()
            .filter(|s| s.live_len <= len && s.side.is_empty() && s.fresh.is_empty());
        let base = match (k.snap.take(), published) {
            (Some(a), Some(b)) if (b.live_len, b.log_at) > (a.live_len, a.log_at) => Some(b),
            (Some(a), _) => Some(a),
            (None, b) => b,
        };
        let next = match base {
            Some(s) => {
                self.shared
                    .snap_extends
                    .fetch_add(1, AtomicOrdering::Relaxed);
                std::sync::Arc::new(s.extend(mem, len, self.opts.snapshot_runs))
            }
            None => std::sync::Arc::new(self.build_snapshot(len)),
        };
        self.shared.snap_kept.fetch_add(1, AtomicOrdering::Relaxed);
        // A version is published only while the ones it replaced are
        // being freed: a handle pinned under `Snapshot` across a burst
        // holds every version retired since it pinned, a copy of the run
        // per commit, and the keeper's own version is dropped here at
        // the next tick instead. Whoever adopts the last published one
        // carries it forward, so nothing is wrong, only later.
        let retired = self
            .shared
            .retired_snaps
            .lock()
            .expect("the retired snapshots")
            .len();
        if retired < KEEPER_RETIRED_MAX {
            self.publish_snapshot(&next);
        }
        k.snap = Some(next);
        k.seen_log = log;
        self.release();
        Want::More
    }

    /// PROTOTYPE: the keeper's snapshot, over the memtables `k` names,
    /// carried into the state `st`: unchanged where the memtables are
    /// the same, which is a merge; at a freeze, brought current to the
    /// old live table's end -- a frozen table is complete -- and its live
    /// entries made frozen ones, the run's order and so its bounds the
    /// same; when the seal lands, its frozen entries dropped and the
    /// live keys and runs moved to an arena of their own, so the old
    /// arena and the values it copied go with the frozen table. Nothing
    /// else carries.
    fn carry_snapshot(
        &self,
        s: std::sync::Arc<Snapshot>,
        k: &Kept,
        st: &State,
    ) -> Option<std::sync::Arc<Snapshot>> {
        let live = k.live.as_ref()?;
        let same_mem = std::sync::Arc::ptr_eq(&st.mem, live);
        let same_frozen = match (&st.frozen, &k.frozen) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same_mem && same_frozen {
            return Some(s);
        }
        if same_mem && st.frozen.is_none() && k.frozen.is_some() {
            let live_ents = s.ents.iter().filter(|e| e.mem != u32::MAX);
            let bytes: usize = live_ents
                .clone()
                .map(|e| {
                    e.len as usize
                        + if e.lrun != NO_RUN {
                            s.arena.run_len(e.lrun) as usize
                        } else {
                            0
                        }
                })
                .sum();
            let mut out = Snapshot {
                arena: std::sync::Arc::new(SnapArena::with_capacity(bytes)),
                runs: s.runs,
                ents: Vec::with_capacity(live_ents.clone().count()),
                log_at: s.log_at,
                live_len: s.live_len,
                side: Vec::new(),
                fresh: Vec::new(),
                filed: 0,
                bounds: Default::default(),
            };
            for e in live_ents {
                let lrun = if e.lrun != NO_RUN {
                    out.arena
                        .append(s.arena.slice(e.lrun, s.arena.run_len(e.lrun)))
                } else {
                    NO_RUN
                };
                out.ents.push(SnapKey {
                    off: out.arena.append(s.key_of(e)),
                    len: e.len,
                    mem: e.mem,
                    frozen: u32::MAX,
                    lrun,
                    frun: NO_RUN,
                });
            }
            return Some(std::sync::Arc::new(out));
        }
        let froze = k.frozen.is_none()
            && st
                .frozen
                .as_ref()
                .is_some_and(|f| std::sync::Arc::ptr_eq(f, live));
        if froze {
            let s = if s.live_len < live.len() || s.log_at < live.log_len() {
                self.shared
                    .snap_extends
                    .fetch_add(1, AtomicOrdering::Relaxed);
                std::sync::Arc::new(s.extend(live, live.len(), s.runs))
            } else {
                s
            };
            let mut out = Snapshot {
                arena: s.arena.clone(),
                runs: s.runs,
                ents: Vec::with_capacity(s.ents.len()),
                log_at: 0,
                live_len: 0,
                side: Vec::new(),
                fresh: Vec::new(),
                filed: 0,
                bounds: s.bounds.clone(),
            };
            for e in &s.ents {
                debug_assert_eq!(e.frozen, u32::MAX, "a freeze finds no frozen table");
                out.ents.push(SnapKey {
                    off: e.off,
                    len: e.len,
                    mem: u32::MAX,
                    frozen: e.mem,
                    lrun: NO_RUN,
                    frun: e.lrun,
                });
            }
            return Some(std::sync::Arc::new(out));
        }
        None
    }
}

impl Db {
    /// PROTOTYPE: the run keeper started, on a handle of its own, if the
    /// option asks and it is not running.
    fn start_keeper(&mut self) {
        if self.keeper.is_some() || !self.opts.snapshot_keeper {
            return;
        }
        let Ok(r) = self.new_reader(false) else {
            return;
        };
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("supdb-keeper".into())
            .spawn(move || {
                idle_cpu_priority();
                r.keep(&flag);
            });
        if let Ok(handle) = spawned {
            self.keeper = Some(Keeper {
                handle: Some(handle),
                stop,
            });
        }
    }

    /// The keeper woken by a publish, so the snapshot is carried into
    /// the new state before its first scan where it can be; a commit
    /// wakes nothing, see `Reader::keep`.
    fn wake_keeper(&self) {
        if self.keeper.is_none() {
            return;
        }
        if let Some(t) = self.shared.keeper_thread.get() {
            t.unpark();
        }
    }

    /// PROTOTYPE: the keeper's snapshot brought current to the last
    /// commit and published, whatever its regime says, and waited for.
    /// What `settle` does; for a test or an experiment that wants the
    /// published snapshot as the keeper leaves it.
    #[doc(hidden)]
    pub fn settle_keeper(&mut self) {
        let Some(k) = self.keeper.as_ref() else {
            return;
        };
        if k.handle.as_ref().is_none_or(|h| h.is_finished()) {
            return;
        }
        let seq = self.shared.keeper_seq.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        while self.shared.keeper_done.load(AtomicOrdering::SeqCst) < seq {
            if let Some(t) = self.shared.keeper_thread.get() {
                t.unpark();
            }
            if k.handle.as_ref().is_none_or(|h| h.is_finished()) {
                return;
            }
            // A sleep and not a yield: the keeper runs at idle priority,
            // and a spinning waiter is what keeps it off the core.
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
    }

    fn stop_keeper(&mut self) {
        if let Some(mut k) = self.keeper.take() {
            k.stop.store(true, AtomicOrdering::SeqCst);
            if let Some(t) = self.shared.keeper_thread.get() {
                t.unpark();
            }
            if let Some(h) = k.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// PROTOTYPE: what the keeper has published over this store's life:
    /// versions extended or built, and versions carried across a
    /// publish. For a test.
    #[doc(hidden)]
    pub fn snapshot_switches(&self) -> u64 {
        self.shared.snap_switched.load(AtomicOrdering::Relaxed)
    }

    #[doc(hidden)]
    pub fn snapshot_kept(&self) -> (u64, u64) {
        (
            self.shared.snap_kept.load(AtomicOrdering::Relaxed),
            self.shared.snap_carried.load(AtomicOrdering::Relaxed),
        )
    }
}

/// PROTOTYPE: what building a cached block reads, and nothing the cache
/// keeps: the segments, the live and the frozen memtable, and whether any
/// source holds a tombstone. A handle makes one over the state it holds
/// for every build and every settle; the builder ahead of the reader is a
/// handle holding the state as of a commit, and the store splices in the
/// keys written after that commit when it installs a block.
struct BuildCtx<'s> {
    /// The watermark the live memtable's chains are walked under.
    wm: u64,
    segs: &'s [std::sync::Arc<Seg>],
    mem: &'s MemTable,
    frozen: Option<&'s MemTable>,
    tombs: bool,
    /// Overlay keys from which a block is built as a merged copy rather
    /// than as resolved deltas: `CACHE_DENSE`, or one for the writer
    /// under canonical forms, whose readers walk a form as the one
    /// structure over the block and never merge it with a source.
    dense_from: usize,
    /// The handle's stale set, held for the build: see `Snapshot::vals`.
    stale: std::cell::Ref<'s, std::collections::HashSet<u32>>,
    /// PROTOTYPE: whether a block the unsealed run covers is copied now
    /// or walked. A read's first touch walks it: every partition record
    /// under a run that covers the block is masked by its update's
    /// tombstone, so a copy of such a block is a copy of the run, and
    /// the walk streams the run itself. The writer's fill at a commit,
    /// the builder ahead and a promotion copy, since they run where no
    /// read waits or for a block whose reads have repaid it.
    copy_dense: bool,
}

impl<'s> BuildCtx<'s> {
    /// PROTOTYPE: where a partition's block boundaries fall in every
    /// level-0 piece meeting its range, and in the snapshot of unsealed
    /// keys: the two source maps a table starts from.
    fn table_bounds(
        &self,
        seg: &Seg,
        l0: &[std::sync::Arc<Seg>],
    ) -> Result<(PieceBounds, PieceRanks)> {
        let nblocks = seg.blob.keys().div_ceil(CACHE_BLOCK);
        let against = seg.blob.id();
        let mut pieces = Vec::new();
        let mut ranks = Vec::new();
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
            let cached = p
                .bounds
                .read()
                .expect("a piece's bounds")
                .iter()
                .find(|(id, _)| *id == against)
                .map(|(_, at)| at.clone());
            let at = match cached {
                Some(at) => at,
                None => {
                    let at = std::sync::Arc::new(block_bounds_of(
                        seg,
                        nblocks,
                        p.blob.keys(),
                        |k| p.ord.seek(p.cursor_from(k), |i| p.blob.key_at(i)),
                        |i, bound| p.ord.advance_below(i, bound, |r| p.blob.key_at(r)),
                    )?);
                    p.bounds
                        .write()
                        .expect("a piece's bounds")
                        .push((against, at.clone()));
                    at
                }
            };
            pieces.push((j, at));
            // Ranks against another partition are no ranks.
            ranks.push(
                p.ranks
                    .read()
                    .expect("a piece's ranks")
                    .as_ref()
                    .filter(|(id, _)| *id == against)
                    .map(|(_, v)| v.clone()),
            );
        }
        Ok((pieces, ranks))
    }
    /// The snapshot's positions at a partition's fences: what the bounds
    /// hold at their two ends, without the walk between.
    fn snap_span(seg: &Seg, unsealed: &Snapshot) -> (u32, u32) {
        let lo = if seg.lo.is_empty() {
            0
        } else {
            unsealed.seek(&seg.lo)
        };
        let hi = match &seg.hi {
            Some(h) => unsealed.seek(h).max(lo),
            None => unsealed.len(),
        };
        (lo as u32, hi as u32)
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
            let ranks = std::sync::Arc::new(BuildCtx::ranks_over(part, p)?);
            *p.ranks.write().expect("a piece's ranks") = Some((id, ranks));
        }
        Ok(())
    }
    /// PROTOTYPE: where the snapshot's keys fall against the partition's
    /// block boundaries.
    fn snap_bounds(
        seg: &Seg,
        nblocks: usize,
        unsealed: &Snapshot,
    ) -> Result<std::sync::Arc<SnapBounds>> {
        let against = seg.blob.id();
        let cached = unsealed
            .bounds
            .read()
            .expect("a snapshot's bounds")
            .iter()
            .find(|(id, _)| *id == against)
            .map(|(_, at)| at.clone());
        if let Some(at) = cached {
            return Ok(at);
        }
        let at = block_bounds_of(
            seg,
            nblocks,
            unsealed.len(),
            |k| unsealed.seek(k),
            |i, bound| unsealed.advance_below(i, bound),
        )?;
        let (lo, hi) = (
            at.first().copied().unwrap_or(0) as usize,
            at.last().copied().unwrap_or(0) as usize,
        );
        let cuts = Self::snap_cuts(seg, unsealed, lo, hi)?;
        let sb = std::sync::Arc::new(SnapBounds { at, cuts });
        unsealed
            .bounds
            .write()
            .expect("a snapshot's bounds")
            .push((against, sb.clone()));
        Ok(sb)
    }
    /// PROTOTYPE: where each key of the snapshot's run over `lo..hi` cuts
    /// the partition's walk, from one forward walk along the index heads:
    /// the run is sorted, so each key's rank is at or past the last one's.
    /// Entries outside the range carry no cut, and a build meeting one
    /// searches as it did.
    fn snap_cuts(seg: &Seg, unsealed: &Snapshot, lo: usize, hi: usize) -> Result<Vec<u32>> {
        let n = unsealed.len();
        let mut cuts = vec![u32::MAX; n];
        let keys = seg.blob.keys();
        let mut r = 0usize;
        let mut kbuf = Vec::new();
        let hi = hi.min(n);
        for (i, cut) in cuts.iter_mut().enumerate().take(hi).skip(lo) {
            let (k, _) = unsealed
                .get(i)
                .ok_or_else(|| err("block cache: a snapshot bound did not resolve"))?;
            r = seg.ord.advance_below(r, k, |j| seg.blob.key_at(j));
            let same = r < keys
                && match seg.ord.whole_key_at(r, &mut kbuf) {
                    Some(w) => w == k,
                    None => seg.blob.key_at(r) == Some(k),
                };
            *cut = ((r as u32) << 1) | same as u32;
        }
        Ok(cuts)
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
        cuts: &[u32],
    ) -> Result<Vec<Over<'a>>> {
        let mut out: Vec<Over> = Vec::with_capacity(snap.len() + filed.len());
        for i in snap {
            let (k, sk) = unsealed
                .get(i)
                .ok_or_else(|| err("block cache: a snapshot bound did not resolve"))?;
            out.push(Over {
                key: k,
                sk: Some(*sk),
                cut: cuts.get(i).copied().unwrap_or(u32::MAX),
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
                    lrun: NO_RUN,
                    frun: NO_RUN,
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
                            sk: Some(SnapKey {
                                mem: ys.mem,
                                lrun: NO_RUN,
                                ..xs
                            }),
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
        pieces: &[PieceRun<'_>],
        snap: &'a Snapshot,
    ) -> Result<Overlay<'a>> {
        let stale: &'a std::collections::HashSet<u32> = &self.stale;
        let mut held: Vec<(&[u8], usize, usize, u32)> = Vec::new();
        for (j, run, ranks) in pieces {
            let p = &src.l0[*j];
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
            return Ok(Overlay {
                over: mem,
                held,
                snap,
                stale,
            });
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
        Ok(Overlay {
            over,
            held,
            snap,
            stale,
        })
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
        let sb = table.snap_at(src.seg, unsealed)?;
        let snap = sb.at[b] as usize..sb.at[b + 1] as usize;
        let mem = self.overlay_mem(unsealed, snap, &table.added[b], false, &sb.cuts)?;
        let pieces: Vec<PieceRun<'_>> = table
            .pieces
            .iter()
            .zip(&table.piece_ranks)
            .map(|((j, at), ranks)| {
                (
                    *j,
                    at[b] as usize..at[b + 1] as usize,
                    ranks.as_ref().map(|v| v.as_slice()),
                )
            })
            .collect();
        self.overlay_runs(src, mem, &pieces, unsealed)
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
        // A table without its bounds has built nothing, and the one
        // caller that asks before a build unlists, which on an unbuilt
        // block is nothing.
        let snap = table
            .snap_at
            .get()
            .map_or(0, |sb| (sb.at[b + 1] - sb.at[b]) as usize);
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
        let sb = table.snap_at(src.seg, unsealed)?;
        let (s0, s1) = (sb.at[b] as usize, sb.at[b + 1] as usize);
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
        let mem = self.overlay_mem(unsealed, snap, filed, true, &sb.cuts)?;
        let pieces: Vec<PieceRun<'_>> = table
            .pieces
            .iter()
            .zip(&table.piece_ranks)
            .map(|((j, at), ranks)| {
                let p = &src.l0[*j];
                let (r0, r1) = (at[b] as usize, at[b + 1] as usize);
                let r = p
                    .ord
                    .seek(p.cursor_from(cursor), |i| p.blob.key_at(i))
                    .clamp(r0, r1);
                (
                    *j,
                    r..r1.min(r.saturating_add(limit)),
                    ranks.as_ref().map(|v| v.as_slice()),
                )
            })
            .collect();
        self.overlay_runs(src, mem, &pieces, unsealed)
    }
    /// PROTOTYPE: the oldest source whose values for an overlay key are
    /// live: 0 with no tombstone in the way, else one past the newest
    /// source holding one. Sources are numbered oldest to newest -- the
    /// partition 0, level-0 pieces 1 through their count, the frozen
    /// memtable, the live one.
    fn oldest_live(
        &self,
        em: &Emit,
        ov: &Overlay,
        o: &Over,
        held: &[(&[u8], usize, usize, u32)],
        src: Sources,
    ) -> usize {
        let nc = src.l0.len();
        let mut start = 0usize;
        if em.tombs {
            if let Some(sk) = o.sk {
                let live_tomb = if sk.mem == u32::MAX {
                    false
                } else if sk.lrun != NO_RUN && !ov.stale.contains(&sk.mem) {
                    ov.snap.run_has_tomb(sk.lrun, self.wm)
                } else {
                    self.mem.has_tomb(self.mem.entry(sk.mem as usize), self.wm)
                };
                let frozen_tomb = || {
                    if sk.frozen == u32::MAX {
                        false
                    } else if sk.frun != NO_RUN {
                        ov.snap.run_has_tomb(sk.frun, SEE_ALL)
                    } else {
                        self.frozen
                            .as_ref()
                            .is_some_and(|fr| fr.has_tomb(fr.entry(sk.frozen as usize), SEE_ALL))
                    }
                };
                if live_tomb {
                    start = nc + 2;
                } else if frozen_tomb() {
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
        let start = self.oldest_live(em, ov, o, held, src);
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
                if sk.frun != NO_RUN {
                    ov.snap.run_values(sk.frun, SEE_ALL, |v| f(key, v));
                } else if let Some(fr) = self.frozen {
                    let e = fr.entry(sk.frozen as usize);
                    fr.live_offs_into(e, &mut em.scratch, SEE_ALL);
                    for &off in em.scratch.iter() {
                        f(key, fr.value_at(off));
                    }
                }
            }
            if sk.mem != u32::MAX {
                if sk.lrun != NO_RUN && !ov.stale.contains(&sk.mem) {
                    ov.snap.run_values(sk.lrun, self.wm, |v| f(key, v));
                } else {
                    let e = self.mem.entry(sk.mem as usize);
                    self.mem.live_offs_into(e, &mut em.scratch, self.wm);
                    for &off in em.scratch.iter() {
                        f(key, self.mem.value_at(off));
                    }
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
        let sb = table.snap_at(src.seg, unsealed)?;
        let floor = BuildCtx::overlay_count(table, b);
        // The run covers the block when its entries over it are most of
        // its keys: three quarters, a fraction the walk-against-copy
        // pricing in `docs/engine.md` puts between.
        let over: usize = (sb.at[b + 1] - sb.at[b]) as usize
            + table
                .pieces
                .iter()
                .map(|(_, at)| (at[b + 1] - at[b]) as usize)
                .sum::<usize>();
        let covered = !self.copy_dense && unsealed.runs && over * 4 >= (hi - lo) * 3;
        if floor > WIDE || covered {
            return Ok(Cached::Wide(WideBlock {
                sorted: self.sorted_filed(&table.added[b]),
                seen: table.added[b].len(),
                covered: floor <= WIDE,
                walks: 0,
            }));
        }
        let ov = self.overlay_all(src, table, b, unsealed)?;
        if ov.over.is_empty() {
            return Ok(Cached::Clean);
        }
        if ov.over.len() < self.dense_from {
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
        // Every block after a scan's first is walked from its start, and
        // the search below answered zero for each of them at the cost of
        // a search.
        let di = if cursor.is_empty() {
            0
        } else {
            select_lower_bound(blk.ents.len(), |i| blk.key(&blk.ents[i]) < cursor)
        };
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
        // A key read from its run streams the snapshot's arena; only the
        // chains are chased.
        let chased = |sk: &SnapKey| -> (u32, u32) {
            (
                if sk.lrun != NO_RUN && !ov.stale.contains(&sk.mem) {
                    u32::MAX
                } else {
                    sk.mem
                },
                if sk.frun != NO_RUN {
                    u32::MAX
                } else {
                    sk.frozen
                },
            )
        };
        for o in &ov.over {
            let Some(sk) = o.sk else { continue };
            let (m, fz) = chased(&sk);
            for (t, slot) in [(tables[0], m), (tables[1], fz)] {
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
            let (m, fz) = chased(&sk);
            for (t, slot) in [(tables[0], m), (tables[1], fz)] {
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
        self.stop_keeper();
        self.stop_ahead();
        if let Some(h) = self.sealing.take() {
            let _ = h.join();
        }
        if let Some((_, h)) = self.compacting.take() {
            let _ = h.join();
        }
        if let Some((_, h)) = self.tiering.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod snap_arena {
    use super::*;

    #[test]
    fn offsets_cross_blocks_and_two_threads_append_without_overlap() {
        let a = std::sync::Arc::new(SnapArena::with_capacity(100));
        assert_eq!(a.base_shift, SNAP_BASE_SHIFT);
        // Block edges: a reservation that would cross one moves whole
        // into the next block, and every offset locates back to it.
        let mut got: Vec<(u32, Vec<u8>)> = Vec::new();
        for i in 0..2000u32 {
            let bytes: Vec<u8> = (0..(i % 37 + 1) as u8)
                .map(|b| b.wrapping_mul(i as u8))
                .collect();
            got.push((a.append(&bytes), bytes));
        }
        for (off, bytes) in &got {
            assert_eq!(a.slice(*off, bytes.len() as u32), &bytes[..]);
        }
        assert!(
            a.len() > (1 << SNAP_BASE_SHIFT),
            "the base block was outgrown"
        );
        let (b, w) = a.locate((1 << SNAP_BASE_SHIFT) + 5);
        assert_eq!((b, w), (1, 5));
        let (b, w) = a.locate(3 << SNAP_BASE_SHIFT);
        assert_eq!((b, w), (2, 0));
        // Two threads appending at once: nothing overlaps, everything
        // reads back.
        let mut hs = Vec::new();
        for t in 0..2u8 {
            let a = a.clone();
            hs.push(std::thread::spawn(move || {
                let mut mine = Vec::new();
                for i in 0..20_000u32 {
                    let bytes = [t, (i & 0xff) as u8, (i >> 8) as u8, 7];
                    mine.push((a.append(&bytes), bytes));
                }
                mine
            }));
        }
        for h in hs {
            for (off, bytes) in h.join().unwrap() {
                assert_eq!(a.slice(off, 4), &bytes[..]);
            }
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
