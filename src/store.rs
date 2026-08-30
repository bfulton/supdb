use crate::block::{self, BlockBuilder, BlockCache, BlockLoc};
use crate::flatindex::{self, FlatIndex};
use crate::freelist::{capacity_for, FreeList};
use crate::index::{get_uvarint, put_uvarint, Ext, Extents};
use crate::keytable::KeyTable;
use crate::readers;

use memmap2::{Mmap, MmapMut};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{Result, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const MAGIC: u64 = 0x5355_5044_4200_0003;

/// Two superblock slots live in the first sector-pair of the file, and a
/// checkpoint alternates between them.
///
/// The store previously wrote its index only at close, so a crash lost not
/// just recent appends but the entire file: without a footer nothing could be
/// read back at all. Alternating slots make a checkpoint atomic in the way
/// that matters -- a torn write can damage at most the slot being written,
/// and the other still describes a complete, older state. Recovery picks the
/// valid slot with the higher generation.
pub(crate) const SUPER: u64 = 4096;
pub(crate) const SLOT: u64 = 512;
/// Encoded size of a superblock: the fields, then the magic, then the
/// checksum. Named because eight call sites used to slice the literal, and
/// adding two fields to `Super` left every one of them reading a prefix that
/// no longer contained the checksum -- a format change that presented itself
/// as "no valid supdb checkpoint" on a healthy file.
pub(crate) const SUPER_BYTES: usize = 144;

#[derive(Clone, Copy, Default, Debug)]
struct Super {
    generation: u64,
    /// The oldest generation whose history is still intact.
    ///
    /// Reclaiming a superseded extent hands its space back for reuse, which
    /// silently invalidates every earlier index that still points at it.
    /// Recording where that happened lets a read of an older state fail
    /// loudly instead of returning whatever now occupies those bytes -- the
    /// one outcome worse than refusing.
    history_from: u64,
    /// Wall-clock milliseconds when this checkpoint was taken.
    ///
    /// The generation is the authoritative identity of a state -- exact,
    /// monotonic, independent of any clock. The timestamp is how people
    /// actually want to ask the question, and it is advisory: a clock can be
    /// stepped backwards, so it is recorded as max(previous, now) to keep the
    /// chain searchable in time order as well as in generation order.
    timestamp: u64,
    key_off: u64,
    key_stored: u64,
    key_uncompressed: u64,
    blk_off: u64,
    blk_stored: u64,
    blk_uncompressed: u64,
    reuse_off: u64,
    reuse_stored: u64,
    reuse_uncompressed: u64,
    high_water: u64,
    /// The redo log: where the records written since the last full index
    /// rewrite begin, and how many bytes of them there are.
    ///
    /// A checkpoint has always done two jobs -- make writes durable, and make
    /// them findable by a fresh reader -- and only the second needs the index.
    /// f27 measured what conflating them costs: inserting under `Sync::Always`
    /// runs at 42,079 ops/s against 173,446 for updating the same number of
    /// keys with the same number of checkpoints, because any insertion sends
    /// `checkpoint_in_place` to the full-rewrite path. The log makes a
    /// durability point proportional to what changed; the index rewrite
    /// becomes something that happens occasionally, to bound replay.
    log_off: u64,
    log_len: u64,
    /// Generation of the last checkpoint that updated the key index -- a full
    /// rewrite or an in-place edit, never a logged point.
    ///
    /// This is what makes log replay safe to order. A logged record is newer
    /// than the index if and only if its stamped generation exceeds this;
    /// without the comparison, replay applied whatever the arena held over
    /// whatever the index said, and the repro was ugly: log a batch, delete
    /// it, let the tombstones fit the in-place slack, crash -- and all forty
    /// deleted keys came back on reopen.
    index_gen: u64,
}

impl Super {
    fn fields(&self) -> [u64; 16] {
        [
            self.generation,
            self.history_from,
            self.timestamp,
            self.key_off,
            self.key_stored,
            self.key_uncompressed,
            self.blk_off,
            self.blk_stored,
            self.blk_uncompressed,
            self.reuse_off,
            self.reuse_stored,
            self.reuse_uncompressed,
            self.high_water,
            self.log_off,
            self.log_len,
            self.index_gen,
        ]
    }

    /// FNV-1a over the fields and the magic. Enough to reject a torn or never
    /// written slot; this guards against truncation, not tampering.
    fn checksum(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in self.fields().iter().chain(std::iter::once(&MAGIC)) {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        }
        h
    }

    fn encode(&self) -> [u8; SUPER_BYTES] {
        let mut out = [0u8; SUPER_BYTES];
        for (i, v) in self.fields().iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        // Native-endian, deliberately, and it is the only field written that
        // way. Every scalar here is little-endian on the wire, but the paths
        // that make this format fast are not: `flatindex` hands back `&[Ext]`
        // borrowed straight out of the mapping and `BlockRec` is reinterpreted
        // in place, both native-endian. So a file is self-consistent only on a
        // machine of the same byte order as the one that wrote it, and until
        // now nothing recorded which that was -- a big-endian-written file
        // would have been read, not refused.
        //
        // Writing the magic with `to_ne_bytes` makes it a byte-order mark at
        // no cost: on a little-endian machine the bytes are identical to what
        // `to_le_bytes` produced, so every file already written stays valid,
        // and a reader of the other order sees the magic byte-swapped and
        // stops. `wrong_endian` turns that into an error that says so.
        out[128..136].copy_from_slice(&MAGIC.to_ne_bytes());
        out[136..144].copy_from_slice(&self.checksum().to_le_bytes());
        out
    }

    /// True when this slot holds a superblock written by a machine of the
    /// opposite byte order: the magic is present but swapped.
    ///
    /// Distinguished from damage so the error can say which. "No readable
    /// superblock" on a perfectly intact file written on the other kind of
    /// machine is a diagnosis that would cost somebody a day.
    fn wrong_endian(buf: &[u8]) -> bool {
        buf.len() >= SUPER_BYTES
            && u64::from_ne_bytes(buf[128..136].try_into().unwrap()) == MAGIC.swap_bytes()
    }

    fn decode(buf: &[u8]) -> Option<Super> {
        if buf.len() < SUPER_BYTES {
            return None;
        }
        let f: Vec<u64> = (0..16)
            .map(|i| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap()))
            .collect();
        if u64::from_ne_bytes(buf[128..136].try_into().unwrap()) != MAGIC {
            return None;
        }
        let s = Super {
            generation: f[0],
            history_from: f[1],
            timestamp: f[2],
            key_off: f[3],
            key_stored: f[4],
            key_uncompressed: f[5],
            blk_off: f[6],
            blk_stored: f[7],
            blk_uncompressed: f[8],
            reuse_off: f[9],
            reuse_stored: f[10],
            reuse_uncompressed: f[11],
            high_water: f[12],
            log_off: f[13],
            log_len: f[14],
            index_gen: f[15],
        };
        if u64::from_le_bytes(buf[136..144].try_into().unwrap()) != s.checksum() {
            return None;
        }
        Some(s)
    }
}

/// When a checkpoint reaches the device.
///
/// Publishing and persisting are different things, and conflating them is
/// where the cost hides. A reader maps the same file, so a write is visible to
/// it as soon as it is in the page cache, and a process crash leaves the file
/// intact regardless. fsync buys ordering against *power loss*: it stops the
/// superblock landing before the sections it points at.
///
/// Every option here is safe against a process crash. They differ only in how
/// much recent work a power cut may take with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    /// fsync every checkpoint. Durable at the cost of two device flushes per
    /// publish, which on the read-your-writes shape is 31x -- 415 ops/s
    /// against 12,985 (`f13-sync`). The default, because a store that loses
    /// acknowledged writes on power loss should be something a caller asked
    /// for rather than something they got.
    Always,
    /// fsync at most once every `n` checkpoints.
    ///
    /// The window is bounded in *checkpoints*, not in time or bytes, because
    /// that is the unit the caller controls: publish ten times between syncs
    /// and at most ten publishes are at risk.
    EveryN(u32),
    /// fsync at most once every interval.
    ///
    /// The bound a caller usually means by "I can lose a second". Note that it
    /// bounds the gap *between syncs*, not the age of the newest unsynced
    /// write, so the exposure is up to one interval plus one checkpoint.
    Interval(std::time::Duration),
    /// Never automatically. `Store::sync` and `Store::close` still flush, so
    /// this is "durable when I say so", not "never durable".
    Never,
}

/// What the kernel is told about how a mapping will be read.
///
/// The engine passed no advice at all, which means `MADV_NORMAL`: the kernel
/// faults in a cluster of pages around every miss on the assumption that a
/// read is the start of a sequential run. For a random point read that is
/// 100 bytes wanted and a readahead window fetched, and when the store no
/// longer fits in memory the pages it fetched speculatively evict the ones
/// something was still using. F1.2 measures the end of that road: 338,681
/// reads/s resident against 370 out-of-core, a 916x collapse.
///
/// `Random` turns readahead off for the mapping. `Sequential` asks for more of
/// it, which is what a scan wants and a point read does not -- the two are
/// opposite, which is why this is a choice and not a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readahead {
    /// Choose from the file's size against the memory available to cache it.
    Auto,
    /// Whatever the kernel does by default. What the engine did until now.
    Default,
    /// No readahead: fetch the page that faulted and nothing around it.
    Random,
    /// More readahead than the default.
    Sequential,
}

/// Memory this process may actually use to cache a file, in bytes.
///
/// `MemAvailable` is the host's answer and is wrong inside a container: the
/// cgroup limit is what the page cache will be reclaimed against, and it can
/// be a fraction of the host's. Both are consulted and the smaller wins.
///
/// v1 and v2 are both read because the layout differs and either can be the
/// one in force. A missing or absent limit is not an error -- it means no cap,
/// and the host figure stands.
#[cfg(target_os = "macos")]
fn sysctl_memsize() -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}

#[cfg(not(target_os = "macos"))]
fn sysctl_memsize() -> Option<u64> {
    None
}

fn available_memory() -> Option<u64> {
    fn field(path: &str, key: &str) -> Option<u64> {
        std::fs::read_to_string(path).ok()?.lines().find_map(|l| {
            let rest = l.strip_prefix(key)?;
            rest.split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kb| kb * 1024)
        })
    }
    fn limit(path: &str) -> Option<u64> {
        let v = std::fs::read_to_string(path).ok()?;
        let v = v.trim();
        if v == "max" {
            return None;
        }
        // A cgroup with no limit stores a number near u64::MAX rather than
        // saying so, and treating that as a cap would advise Random for every
        // store on earth.
        v.parse::<u64>().ok().filter(|b| *b < u64::MAX / 2)
    }
    // Which cgroup this process is in, for the v2 layout.
    let v2 = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("0::").map(|p| p.to_string()))
        })
        .and_then(|p| limit(&format!("/sys/fs/cgroup{p}/memory.max")));
    // The *memory* controller's line, not whichever line came first. Taking
    // field three of line one reads the systemd hierarchy's path and then
    // looks for a memory limit that was never going to be there, so every
    // store looks like it fits and `Auto` never advises anything.
    let v1 = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| {
                    l.split(':')
                        .nth(1)
                        .is_some_and(|c| c.split(',').any(|n| n == "memory"))
                })
                .and_then(|l| l.split(':').nth(2))
                .map(|p| p.to_string())
        })
        .and_then(|p| limit(&format!("/sys/fs/cgroup/memory{p}/memory.limit_in_bytes")))
        .or_else(|| limit("/sys/fs/cgroup/memory/memory.limit_in_bytes"));
    // `/proc` is Linux's. Without a reading, `Auto` has no basis for
    // overriding the kernel and leaves readahead alone -- which is safe, and
    // silently disables the whole feature on macOS. `hw.memsize` is the total
    // rather than what is available, so it errs towards a larger denominator
    // and therefore towards the default advice: the conservative direction.
    let host = field("/proc/meminfo", "MemAvailable:")
        .or_else(|| field("/proc/meminfo", "MemTotal:"))
        .or_else(sysctl_memsize);
    [host, v1, v2].into_iter().flatten().min()
}

/// Above this ratio of file size to available memory, `Auto` advises Random.
///
/// Measured, and the first value I picked was wrong by a factor of two in the
/// expensive direction. `f24-autoreadahead` sweeps the ratio with all three
/// advices interleaved at every point, at 1M keys:
///
///   ratio   default   random
///    0.25    16,172    4,920    default 3.3x ahead
///    1.00    15,024    4,792    default 3.1x ahead
///    1.50       709    4,813    default 6.8x behind
///    3.00       167    4,803    default 29x behind
///
/// The crossover is a cliff between 1.0 and 1.5, not a slope, and readahead
/// keeps paying right up to it -- a store that merely *approaches* the memory
/// available to it is still one whose speculative pages get used. A threshold
/// of 0.5, which looked conservative, advised Random across the whole 0.5-1.0
/// band where the default is three times faster.
///
/// So: advise Random when the store does not fit, and not before -- but not
/// at exactly one. A cgroup rounds its limit down to a page multiple, so a
/// store sized to its memory comes out a couple of kilobytes over and lands on
/// the wrong side of the cliff; at ratio 1.00 the sweep duly caught `Auto`
/// choosing Random where the default is 3.1x faster. A store sized to fit is
/// the common case, not an edge one, so the threshold sits clear of it. At
/// 1.25 the measurement is already 2,682 against 4,887 and firmly Random's.
const AUTO_RANDOM_ABOVE: f64 = 1.1;

/// Initial capacity of a shard's pending arena, before it grows.
///
/// Large enough that a batch does not spend its life doubling from nothing,
/// small enough that 64 shards cost a few megabytes rather than the whole
/// buffer budget. See F25.3.
const ARENA_START: usize = 128 * 1024;

impl Readahead {
    /// What this advice means for a file of `bytes`, with `Auto` resolved.
    fn resolve(self, bytes: u64) -> Readahead {
        match self {
            Readahead::Auto => match available_memory() {
                Some(avail) if bytes as f64 > avail as f64 * AUTO_RANDOM_ABOVE => Readahead::Random,
                // No reading on how much memory there is means no basis for
                // overriding the kernel, so leave it alone.
                _ => Readahead::Default,
            },
            other => other,
        }
    }

    fn apply(self, m: &Mmap) {
        let a = match self {
            // Already resolved by the caller; nothing to apply.
            Readahead::Auto | Readahead::Default => return,
            Readahead::Random => memmap2::Advice::Random,
            Readahead::Sequential => memmap2::Advice::Sequential,
        };
        // Advisory by definition: a kernel that declines still serves reads.
        let _ = m.advise(a);
    }
}

/// How a reader represents the block table.
///
/// A store's block table can be read in place out of the mapping or decoded
/// into a private `Vec` at open. Mapped is the default, and is what lets many
/// readers of one store share it instead of each holding a copy. This exists
/// so both can run in one process: comparing them across two runs is what this
/// repository has already learned not to do.
#[derive(Clone, Copy, Debug)]
pub struct ReadOptions {
    pub mapped_blocks: bool,
    /// Hold the current block across a scan's entries instead of resolving it
    /// for each one.
    ///
    /// A scan walks keys in order and consecutive keys usually share a block,
    /// so resolving it per entry -- twice, until this commit -- re-reads the
    /// block table, re-bounds-checks the mapping and re-tests a checksum bit
    /// for an answer that has not changed. Off is the old behaviour, kept so
    /// `f15-scancache` can measure the difference in one process.
    pub scan_block_cache: bool,
    /// Narrow an ordered seek with the index's fence before searching records.
    ///
    /// A seek without it binary-searches the record region directly: about
    /// twenty probes for a million keys, each at a scattered offset in a 36MB
    /// region. Off is the old behaviour, kept so `f18-fence` can price it in
    /// one process over one file.
    pub seek_fence: bool,
    /// Verify only the chunks an extent touches, when the block carries
    /// per-chunk checksums, instead of the whole block.
    ///
    /// Off is the old behaviour, kept so `f20-chunkcrc` can price it over one
    /// file.
    pub chunk_verify: bool,
    /// Verify a plain block's checksum the first time this reader touches it.
    ///
    /// A reader checks each block once and remembers that, so a point-read
    /// workload amortises it to nothing -- which is what f8-checksums measured
    /// and why the cost looked free. A scan over a fresh reader touches every
    /// block for the first time and pays for all of them, and `f19-coldscan`
    /// is that case.
    ///
    /// This is per reader rather than per process, unlike `Options::checksums`
    /// which is a global set at store creation (review 2.7c). A reader can
    /// therefore decline to verify a file that has checksums in it, which is
    /// the guarantee LMDB does not offer at all and is not charged for.
    pub verify_checksums: bool,
    /// What to tell the kernel about this reader's mapping.
    pub readahead: Readahead,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            mapped_blocks: true,
            scan_block_cache: true,
            seek_fence: true,
            chunk_verify: true,
            verify_checksums: true,
            // Out of the box, because the choice depends on a ratio the caller
            // usually does not know and the cost of getting it wrong is 30x.
            readahead: Readahead::Auto,
        }
    }
}

/// When space belonging to a superseded value may be handed out again.
///
/// Releasing a block only makes its space available; reuse is what actually
/// destroys the older state that still points there. These are the points on
/// that spectrum, from cheapest to safest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reclaim {
    /// Reuse immediately. Cheapest and smallest, and correct only when nothing
    /// reads the store while it is written -- a reader that has opened but not
    /// finished can have its data written over. Measured here, this failed 41%
    /// of reads under a concurrent reader.
    Now,
    /// Reuse once no reader that could see it remains. Readers publish the
    /// generation they hold in a shared table and the writer stays behind the
    /// oldest of them, so this costs only the space live readers are actually
    /// pinning. The default: it is exact rather than estimated.
    AfterReads,
    /// Reuse once this many checkpoints have passed. An approximation of
    /// AfterReads for readers that cannot register -- another process on a
    /// filesystem without shared memory semantics, say. A reader slower than
    /// the delay still fails loudly rather than reading the wrong bytes.
    AfterDelay(u64),
    /// Accumulate freed space and reuse none of it until the store is closed,
    /// where defragmentation and truncation reclaim it in one pass. For a
    /// writer that wants no reuse hazard at all while it runs and does not
    /// need the space back until it stops.
    OnClose,
    /// Never reuse, and never release. Earlier states stay readable, so this
    /// is what read-as-of requires; the file keeps every version ever written.
    Never,
}

impl Reclaim {
    /// Whether superseded extents are handed back at all.
    fn releases(&self) -> bool {
        !matches!(self, Reclaim::Never)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Target size of a compression block. Bigger compresses better and costs
    /// more to decompress on a point read; this is the size/read dial.
    pub block_size: usize,
    /// Ceiling on bytes held in unsealed per-key buffers, across all shards.
    /// A shallow workload has more keys in flight than fit, so this is what
    /// decides how large extents get before they are forced out.
    pub buffer_bytes: usize,
    pub compress: bool,
    /// Write the flat key index's slack region instead of leaving it a hole.
    ///
    /// The index reserves half its record region again as slack so that an
    /// in-place checkpoint has somewhere to put a lengthened record. Nothing
    /// reads that region until an update writes into it, and a file that has
    /// never been written there reads back zeroes either way -- so the bytes
    /// can be reserved without being sent to the disk. At 1M keys the slack is
    /// 18MB of a 63MB index. Off is the default; on is the old behaviour, kept
    /// so `f16-slack` can price it in one process.
    /// Verify checksums on the writer's own read path.
    ///
    /// `Reader` has always verified and `Store::read_all` never did, so the
    /// same store answered with two different guarantees depending on which
    /// handle you held, and the mixed YCSB workloads -- where Supdb leads by
    /// 3.6x to 20x -- all ran on the unchecked one (C1.3). RocksDB verifies
    /// every block it loads by default and so does a `Reader` here, so on is
    /// the default and matching it is the point.
    ///
    /// Off is the faster setting, and it is a real one: LMDB has no checksums
    /// at all, so a caller who wants that trade can have it and `f21-writerverify`
    /// prices it.
    pub verify_reads: bool,
    pub write_index_slack: bool,
    /// Copy every key onto the heap before sorting them for a full checkpoint.
    ///
    /// The keys already lie contiguously in each shard's arena, so a rewrite
    /// can borrow them and sort slices into a few large buffers. Copying means
    /// one allocation per key -- a million 16-byte mallocs on a bulk load --
    /// and a sort whose every comparison chases a pointer into a separate
    /// allocation. Off is the default; on is the old behaviour, kept so
    /// `f17-gather` can price it in one process.
    pub checkpoint_copies_keys: bool,
    /// Independent write shards.
    ///
    /// More of them means each key table is smaller and its working set is
    /// likelier to stay in cache -- the write path's cost grows with the
    /// number of distinct keys, so this is the cheapest lever on that slope.
    /// It also spreads lock contention across writers.
    pub shards: usize,
    pub cache_blocks: usize,
    /// Merge a key's extents once it has this many, inline on the append that
    /// crosses the line.
    ///
    /// There is no background thread and no stop-the-world pass: a batch
    /// compaction measured 18.7 seconds, which is a stall whether or not a
    /// separate thread runs it. Doing the merge on the writer that caused the
    /// fragmentation keeps the cost proportional to the damage and spreads it
    /// across the run. A key that never fragments is never rewritten, and the
    /// data is rewritten about log_threshold(N) times over its life rather
    /// than on a levelled schedule.
    ///
    /// Contiguity on ingest is bought with buffer memory, and the exchange
    /// rate is steep: at a 64 MB buffer a deep key fragments into ~21 extents
    /// and warm reads fall to 1,168/s, against 38,509/s at 1 GB. Since no
    /// buffer can cover a dataset larger than memory, fragmentation has to be
    /// repaired rather than prevented.
    ///
    /// Doing it per key and only past a threshold is what keeps it cheap: the
    /// work scales with how badly a key fragmented, not with total data
    /// volume, so a key that never fragmented is never rewritten. That is the
    /// distinction from LSM levelled compaction, which rewrites everything on
    /// a schedule regardless of need.
    pub merge_threshold: usize,
    pub reclaim: Reclaim,
    /// An extent at least this large gets a block to itself.
    ///
    /// Packing exists to give the compressor a window when a key holds only a
    /// kilobyte. A key that already holds tens of kilobytes needs no help, and
    /// packing it costs dearly on reads: sharing 64 KB blocks meant
    /// decompressing 128 KB to extract 47 KB, which measured as 9,377 warm
    /// reads/s against Uppend's 57,247. Above this size a key's run stands
    /// alone and a read touches nothing but that key's bytes.
    pub solo_threshold: usize,
    pub chunk_size: usize,
    /// Compute and verify block checksums.
    ///
    /// On by default: without it a bit flip, a torn write or a reused slot
    /// returns silently wrong data, because LZ4 decodes many corrupted inputs
    /// into plausible bytes. The knob exists so the cost can be measured
    /// honestly -- both arms in one process, interleaved -- rather than by
    /// comparing two runs taken hours apart, which measures the machine as
    /// much as the code.
    pub checksums: bool,
    /// Buffer pending values in one arena per shard instead of one `Vec` per
    /// key.
    ///
    /// On by default. `EXT.10` had Supdb losing bulk ingest 1.85x to an LMDB
    /// that was not syncing either, and the profile said why: 1.37x the
    /// instructions but 21x the last-level misses per key, because every
    /// buffered value used to get its own allocation. f25-arena prices the
    /// change with both arms interleaved.
    pub pending_arena: bool,
    /// Seal a shard from `put` when its buffer fills, as `append` does.
    ///
    /// On by default. Off is the old behaviour, kept so f26 can price it:
    /// `put` ignored `buffer_bytes` and buffered a whole workload until
    /// `flush`, which no benchmark here reached the size to notice.
    pub seal_on_put: bool,
    /// Make a durable checkpoint proportional to what changed, by writing a
    /// redo log instead of rewriting the key index.
    ///
    /// Off by default, and the default is deliberate: a logged checkpoint is
    /// durable and is replayed by `Store::open`, but a `Reader` opened before
    /// the next full rewrite does not see it. That is a real narrowing of what
    /// `checkpoint` has always promised -- durable *and* visible to anyone --
    /// so it is opt-in until the reader replays too, and F28.2 records the
    /// limitation rather than leaving it to be discovered.
    ///
    /// f27 is why this exists: inserting under `Sync::Always` runs at 42,079
    /// ops/s against 173,446 for updating the same keys with the same
    /// checkpoint count, because any insertion sends `checkpoint_in_place` to
    /// the full-rewrite path.
    /// Reserve directory room so a later checkpoint can add keys in place
    /// instead of rewriting the whole index section.
    ///
    /// Off by default because it is not free: the directory is double-buffered
    /// so an insertion can be published with one aligned store, which costs
    /// about 4 bytes per key on an index that is about 57, and Supdb already
    /// loses the size axis (EXT.6). f30 prices both halves.
    ///
    /// The records and the hash already had room -- records carry half again
    /// in slack, and the hash runs at half load -- so the directory was the
    /// only reason `checkpoint_in_place` declined every insertion, which f27
    /// measured at 4.122x on a workload that only inserts.
    /// Sort and encode the key index across threads instead of on the
    /// checkpointing one.
    ///
    /// On by default. f34 measures the build at 1.930x with all three parts
    /// threaded -- the sort splits and merges, the record loop splits because
    /// `rec_offs` is a prefix sum, and the hash claims slots with
    /// compare-exchange. It is about 8% of a bulk load, which this host cannot
    /// resolve end to end, so the phase is what is gated and the contribution
    /// is arithmetic.
    pub parallel_index: bool,
    pub index_inserts: bool,
    pub redo_log: bool,
    /// Bytes reserved for the redo log at each full rewrite. When it fills,
    /// the next checkpoint rewrites the index and starts a fresh one, which is
    /// what bounds replay.
    pub log_bytes: usize,
    /// Write the key index in a shape a reader can use where it lies, instead
    /// of one it has to decode into the heap first.
    ///
    /// Buys the two things the heap index cannot give: an open that does not
    /// grow with the key count, and an index shared between reader processes
    /// rather than duplicated in each. Costs file size, because a section read
    /// in place cannot be compressed. Both arms exist so the trade is measured
    /// rather than argued -- see `f11-flatindex`.
    pub flat_index: bool,
    /// fsync on checkpoint.
    ///
    /// Publishing does not need it. Readers map the same file, so a write is
    /// visible to them as soon as it is in the page cache, and a process crash
    /// leaves the file intact either way. What fsync buys is *ordering* across
    /// a power loss: it stops the superblock landing before the sections it
    /// points at.
    ///
    /// Recovery can catch that instead of preventing it. Every section is
    /// CRC'd, the superblock alternates between two slots, and `Reader::open`
    /// already takes the newest one that validates and falls back to the older
    /// otherwise. A superblock pointing at bytes that never landed fails its
    /// checksum and the previous checkpoint is used.
    ///
    /// The cost of turning this off is losing checkpoints on power loss, not
    /// corruption. LMDB's MDB_NOSYNC and RocksDB without WAL sync make the
    /// same trade. Left on by default because it is the safe direction and
    /// f13 measures what it costs.
    /// When a checkpoint reaches the device. See `Sync`.
    ///
    /// `Sync::Always` by default. `Store::publish` is the per-call escape
    /// hatch when a caller wants visibility without durability for one
    /// checkpoint, and `Store::sync` takes a durability point on demand.
    pub sync: Sync,
    /// Chunk size for solo blocks. A deep key's whole run is decompressed on a
    /// full read regardless of chunking, so warm reads there are flat across
    /// this value while compression is not: the larger window is close to free
    /// on space. Only read_last pays for it.
    pub solo_chunk_size: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            block_size: 64 * 1024,
            buffer_bytes: 512 * 1024 * 1024,
            // Off. f12 measures what it costs at 5M keys, both arms
            // interleaved: reads 3.6x, scans 30.1x, writes 3.8x -- to save
            // 1.04x on disk. The payload there is half-random by
            // construction, so a more compressible workload would save more
            // space; the time costs do not depend on how well the data
            // compresses, because decompression is per byte either way.
            //
            // Still available for a deployment that is disk-bound rather than
            // latency-bound, which is why this is an option and not a
            // deletion.
            compress: false,
            verify_reads: true,
            write_index_slack: false,
            checkpoint_copies_keys: false,
            shards: 64,
            cache_blocks: 4096,
            solo_threshold: 16 * 1024,
            chunk_size: 1024,
            solo_chunk_size: block::CHUNK,
            merge_threshold: 4,
            checksums: true,
            pending_arena: true,
            seal_on_put: true,
            parallel_index: true,
            index_inserts: true,
            redo_log: true,
            log_bytes: 4 << 20,
            // On, now that the space cost is bounded.
            //
            // It was off while a checkpoint appended a whole index section
            // that nothing reclaimed -- 61 bytes per key against the varint
            // format's 8.8, forever -- because that trades a bounded cost for
            // an unbounded one. Sections are now released once no reader can
            // reach them, and f11 measures the steady-state growth at 0 B/key
            // per checkpoint on both arms, so the precondition is met.
            //
            // What is left is a one-off: at 5M keys the file goes from 394 MB
            // to 683 MB, +73.5%, because a section read in place cannot be
            // compressed. That buys an open of 0.29ms against 738ms (2537x),
            // reads at 1.25x, and an index of 67 B/key that is file-backed and
            // shared between reader processes rather than 186 B/key duplicated
            // in each. Space is the axis this engine has to spare.
            // On. It was defaulted off for one commit after c2-oracle found
            // it serving wrong data under Reclaim::AfterReads -- 187
            // mismatches and 2,896 read errors. The cause was a double
            // release: two mechanisms both freed a superseded key section, so
            // the free list held one range twice and handed it to two blocks.
            // A data block landed exactly on the live index. Fixed by making
            // "released exactly once" a property of `index_history` rather
            // than of two call sites agreeing; the oracle is clean on all
            // three reclaim policies.
            flat_index: std::env::var("SUPDB_FLAT_INDEX")
                .map(|v| v != "0")
                .unwrap_or(true),
            sync: match std::env::var("SUPDB_SYNC").ok().as_deref() {
                Some("0") => Sync::Never,
                Some(v) => v.parse::<u32>().map(Sync::EveryN).unwrap_or(Sync::Always),
                None => Sync::Always,
            },
            reclaim: Reclaim::AfterReads,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub blocks: u64,
    pub bytes_written: u64,
    pub index_bytes: u64,
    pub keys: u64,
    pub merges: u64,
    pub free_bytes: u64,
    pub reused: u64,
    pub reused_bytes: u64,
}

// ---------------------------------------------------------------- writing --

struct Appender {
    file: File,
    off: u64,
    blocks: Vec<BlockLoc>,
    /// Per-chunk checksums, one fixed-size row per block, parallel to
    /// `blocks`. Block ids are indices that are only ever appended, so the two
    /// stay in step by construction.
    chunk_crcs: Vec<[u32; block::MAX_CHUNK_CRCS]>,
    /// Which chunks this writer has already checked, one bit each. A block is
    /// checked on first touch and not again, which is what a `Reader` does and
    /// what RocksDB does when it loads a block into its cache.
    verified: Vec<std::sync::atomic::AtomicU64>,
    /// Extents still referencing each block. A block nobody references is
    /// garbage and its space can be handed back.
    live: Vec<u32>,
    free: FreeList,
    /// The reader table, in the reserved first page.
    table: MmapMut,
    /// The redo log arena: where it starts, how big it is, how much is used.
    ///
    /// `used` is not persisted anywhere. The arena is written zeroed, every
    /// record carries its length and a CRC, and replay stops at the first zero
    /// length or bad checksum -- so the log describes its own extent and a
    /// durability point costs the records plus one fsync, with no superblock
    /// write at all. A crash mid-record leaves a partial tail that the CRC
    /// rejects, and everything before it is intact, which is the whole reason
    /// a log is written this way rather than with a length field to update.
    log: Option<(u64, u64, u64)>,
    /// Generation of the last index-updating checkpoint. See Super::index_gen.
    index_gen: u64,
    /// The arena the *previous* generation's superblock still names.
    ///
    /// A rewrite makes the current arena redundant, but not immediately
    /// reusable: the superblock being replaced still points at it, and a
    /// reader that opened on that generation is still entitled to replay it.
    /// Releasing it in the same checkpoint hands its bytes straight back to
    /// the next section -- `place_section` returned the identical offset -- so
    /// it is held one generation, which is the same hysteresis
    /// `index_history` gives a superseded key section.
    prev_log: Option<(u64, u64)>,
    /// A read mapping of the file, for serving a sealed extent in place.
    ///
    /// `read_extent` preads the extent's whole block -- 64KiB -- into a fresh
    /// Vec and copies the extent out of it, which is the right shape for the
    /// occasional read during a merge and the wrong one for a read path. A
    /// `Reader` never did that; it slices the extent straight out of its
    /// mapping. Serving `read_all` the old way cost 20x on a read-only
    /// workload against serving it this way.
    ///
    /// Only bytes below `off` are served from here. The file is truncated to
    /// `off` when a section is reclaimed, and reading a mapped page past
    /// end-of-file is a SIGBUS rather than an error.
    map: Option<Mmap>,
    generation: u64,
    timestamp: u64,
    reuse_log: Vec<(u64, u32, u64)>,
    last_index: Option<BlockLoc>,
    /// The index sections of recent checkpoints, newest last, so superseded
    /// ones can be handed back instead of accumulating.
    ///
    /// Every checkpoint writes three sections -- keys, blocks, reuse log --
    /// and nothing ever released them, so a store that checkpoints often grew
    /// without bound in checkpoint count rather than in data. That was
    /// tolerable while the key section was compressed varints; it stopped
    /// being tolerable at the flat format's seven times the size.
    /// Newest last. The key section is optional: an in-place checkpoint
    /// republishes the *same* key section, which must not be released while
    /// it is still the live one. Recording it again -- or worse, recording
    /// some other section in its place -- hands a live index to the free list,
    /// and the next section written lands on top of it. That presents as
    /// "extent names a block that does not exist" from a reader, which is a
    /// trampled index rather than a bad extent.
    /// Each section is `Some` until it has been released, and the entry is
    /// dropped only once all three are `None`.
    ///
    /// It used to hold them unconditionally and `remove(0)` the whole entry
    /// while skipping the key section if it was still live -- which forgot
    /// that section, so a second mechanism existed to catch the leak, and the
    /// two of them released the same range twice. The free list then held one
    /// range twice and handed it to two different blocks: c2-oracle saw 187
    /// mismatches and 2,896 read errors, and a data block landed exactly on
    /// the live key index at 209408..238080. Marking each section as it goes
    /// is what makes "released exactly once" a property of the structure
    /// rather than of two call sites agreeing.
    index_history: Vec<(u64, Option<BlockLoc>, Option<BlockLoc>, Option<BlockLoc>)>,
    /// Checkpoints published since the last flush to the device, and when that
    /// flush happened. What `Sync::EveryN` and `Sync::Interval` count.
    since_sync: u32,
    last_sync: std::time::Instant,
    /// Set by a checkpoint that did not sync, cleared by one that did. Lets
    /// `close` tell "nothing to flush" from "work is at risk".
    unsynced: bool,
    /// The published key index, mapped writable, so a checkpoint that only
    /// changes existing keys can publish each one with a single aligned store
    /// instead of rewriting the section.
    ///
    /// `pwrite` would not do: POSIX says nothing about a write racing a
    /// reader's mapping of the same bytes, and the whole guarantee here is
    /// that a reader sees the old slot or the new one and never half of
    /// either. An aligned 8-byte store to a shared mapping is that guarantee.
    live_index: Option<(MmapMut, FlatIndex, u64, u64)>,
    /// File offset of the key section currently being updated in place.
    ///
    /// It was written by an ordinary full checkpoint, so it sits in
    /// `index_history` marked releasable like any other -- and once the reuse
    /// floor passed that generation it *was* released, while still live, and
    /// the next section written landed on top of it. Six in-place checkpoints
    /// were enough. Cleared when a full checkpoint supersedes it, which is the
    /// only moment it stops being live.
    live_key_off: Option<u64>,
    /// Oldest generation whose index sections are still intact. Reported in
    /// the superblock, and what `open_as_of` refuses to read past.
    history_from: u64,
}

impl Appender {
    fn retain(&mut self, id: u32) {
        self.live[id as usize] += 1;
    }

    /// Drop one reference; hand the block's space back when the last one goes.
    ///
    /// The saturating guard this used to carry -- decrement only if positive,
    /// then free if the count is zero -- did not prevent a double release, it
    /// guaranteed one. A second call on an already-freed block skipped the
    /// decrement and then fell straight into the free-list push, offering the
    /// same slot twice. `take_below` duly handed it out twice, and the
    /// differential oracle caught the result: three block ids describing the
    /// identical byte range, one of them still holding 71 live references,
    /// and the writer's own merge path unable to decode a block it had
    /// written.
    ///
    /// Now an unbalanced release is refused rather than absorbed, and asserts
    /// in debug builds so the caller that over-released is the thing that
    /// fails, not a decode three thousand operations later.
    fn release(&mut self, id: u32) -> Result<()> {
        let i = id as usize;
        if self.live[i] == 0 {
            debug_assert!(false, "release of block {id} with no live references");
            return Ok(());
        }
        self.live[i] -= 1;
        if self.live[i] == 0 {
            let loc = self.blocks[i];
            self.free.release(loc.off, loc.cap, self.generation);
        }
        Ok(())
    }

    /// Make sure the read mapping covers at least `need` bytes.
    ///
    /// Remapping is rare: the map is only short when the file has grown since
    /// it was made, and it is made over the whole file each time.
    fn ensure_map(&mut self, need: u64) -> Result<()> {
        let have = self.map.as_ref().map_or(0, |m| m.len() as u64);
        if have >= need && self.map.is_some() {
            return Ok(());
        }
        // A shared read mapping, so writes made through the file descriptor
        // are visible without a remap. Only the length has to be chased.
        self.map = Some(unsafe { Mmap::map(&self.file)? });
        Ok(())
    }

    /// One extent's bytes, borrowed from the mapping when the block is stored
    /// verbatim and decoded into `scratch` when it is not.
    ///
    /// The mapping is used only for a block that lies wholly below the
    /// high-water mark and wholly inside the map, so a file that was trimmed
    /// after the map was made cannot turn a read into a SIGBUS.
    /// Verify the chunks an extent touches, against the checksums this writer
    /// computed when it wrote the block.
    ///
    /// The `Reader` reads its checksums out of the block table in the file;
    /// the writer already has them in memory, so this needs no section and no
    /// parse. Chunks are checked once each and remembered, so the steady state
    /// is the same either way and only the first touch of a block costs.
    fn verify_extent(&self, block: u32, raw: &[u8], a: usize, b: usize) -> Result<()> {
        use std::sync::atomic::Ordering;
        let Some(row) = self.chunk_crcs.get(block as usize) else {
            return Ok(());
        };
        if a >= b || b > raw.len() {
            return Ok(());
        }
        for j in (a / block::CHUNK)..=((b - 1) / block::CHUNK) {
            let (lo, hi) = (j * block::CHUNK, ((j + 1) * block::CHUNK).min(raw.len()));
            if lo >= hi || j >= block::MAX_CHUNK_CRCS {
                return Ok(());
            }
            let slot = block as usize * block::MAX_CHUNK_CRCS + j;
            let Some(cell) = self.verified.get(slot / 64) else {
                return Ok(());
            };
            let bit = 1u64 << (slot % 64);
            if cell.load(Ordering::Relaxed) & bit != 0 {
                continue;
            }
            if block::crc32(&raw[lo..hi]) != row[j] {
                return Err(corrupt("block checksum mismatch"));
            }
            cell.fetch_or(bit, Ordering::Relaxed);
        }
        Ok(())
    }

    fn extent_bytes<'a>(
        &'a self,
        e: Ext,
        scratch: &'a mut Vec<u8>,
        verify: bool,
    ) -> Result<&'a [u8]> {
        let loc = *self
            .blocks
            .get(e.block as usize)
            .ok_or_else(|| corrupt("extent names a block that does not exist"))?;
        let end = loc.off + loc.stored as u64;
        let a = e.off as usize;
        let b = a
            .checked_add(e.len as usize)
            .ok_or_else(|| corrupt("extent length overflows"))?;
        if loc.is_plain() && b <= loc.uncompressed as usize && end <= self.off {
            if let Some(m) = &self.map {
                if end as usize <= m.len() {
                    let base = loc.off as usize;
                    let raw = &m[base..base + loc.stored as usize];
                    if verify && loc.chunk_crc && block::checksums_on() {
                        self.verify_extent(e.block, raw, a, b)?;
                    }
                    return Ok(&raw[a..b]);
                }
            }
        }
        *scratch = self.read_extent(e)?;
        Ok(scratch.as_slice())
    }

    /// Read one extent back out, decompressing its block if needed.
    fn read_extent(&self, e: Ext) -> Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        // Checked, because `e` comes out of an index that a corruption
        // experiment damages. `Reader` has had `loc_of` for this since a
        // damaged file took the process down through `scan`; the writer's copy
        // indexed straight into the table and had never been asked, because
        // nothing could reopen a store to ask it.
        let loc = *self
            .blocks
            .get(e.block as usize)
            .ok_or_else(|| corrupt("extent names a block that does not exist"))?;
        let mut raw = vec![0u8; loc.stored as usize];
        self.file.read_exact_at(&mut raw, loc.off)?;
        // a chunked block is a chunk directory plus per-chunk streams, not a
        // single lz4 stream, so it has to be decoded as such
        let full = if loc.is_plain() {
            raw
        } else if loc.chunked {
            let mut out = vec![0u8; loc.uncompressed as usize];
            block::read_chunked_range(
                &raw,
                loc.uncompressed as usize,
                0,
                loc.uncompressed as usize,
                &mut out,
            )?;
            out
        } else {
            block::decompress(&raw, loc.uncompressed as usize)?
        };
        let (a, b) = (e.off as usize, (e.off + e.len) as usize);
        full.get(a..b)
            .map(|s| s.to_vec())
            .ok_or_else(|| corrupt("extent runs past its block"))
    }
}

impl Appender {
    /// Write one block and return its id. Compression is attempted and kept
    /// only if it actually shrinks the block; a block that does not compress
    /// is stored verbatim so that reads of it can be served straight from the
    /// mapping with no copy at all.
    /// The generation below which freed space may be reused.
    ///
    /// A live reader's generation is authoritative: nothing it can still see
    /// is handed out, however long it takes. The grace window only applies
    /// when no reader is registered, covering the moment between a reader
    /// deciding to open and getting its slot.
    fn reuse_floor(&self, policy: Reclaim) -> u64 {
        let live = || {
            let t = unsafe { readers::slots(self.table.as_ptr()) };
            readers::oldest(t)
        };
        match policy {
            Reclaim::Now => u64::MAX,
            // Never hand out anything a live reader can still see. With none
            // registered the floor is still the current generation, not past
            // it: merging frees blocks the newest checkpoint points at, and a
            // reader may have opened on that checkpoint a moment ago and not
            // yet claimed its slot.
            Reclaim::AfterReads => live().unwrap_or(self.generation),
            Reclaim::AfterDelay(n) => {
                let by_delay = self.generation.saturating_sub(n);
                live().map_or(by_delay, |o| o.min(by_delay))
            }
            // nothing is reusable while the store runs
            Reclaim::OnClose | Reclaim::Never => 0,
        }
    }

    fn write_block(
        &mut self,
        payload: &[u8],
        compress: bool,
        solo: bool,
        chunk: usize,
        policy: Reclaim,
    ) -> Result<u32> {
        // Every compressed block is chunked, packed ones included. A packed
        // block holds many keys' short runs, so reading one key out of a
        // 64 KiB block compressed as a unit meant decompressing 64 KiB to
        // reach 960 bytes -- 68x read amplification, and the reason the wide
        // shape read at 29,706/s against RocksDB's 123,153. Chunking bounds
        // that to the one chunk the extent lands in.
        let chunked = compress && payload.len() > chunk;
        let stored: Option<Vec<u8>> = if chunked {
            let c = block::write_chunked_sz(payload, chunk);
            if c.len() < payload.len() {
                Some(c)
            } else {
                None
            }
        } else if compress {
            block::compress(payload)
        } else {
            None
        };
        let chunked = chunked && stored.is_some();
        let bytes: &[u8] = stored.as_deref().unwrap_or(payload);
        use std::os::unix::fs::FileExt;
        let len = bytes.len() as u32;
        let floor = self.reuse_floor(policy);
        let (off, cap) = match self.free.take_below(len, floor) {
            // A slot freed earlier, now reused. This is the moment history is
            // actually lost -- releasing a block only made its space
            // available, and until something is written over it every earlier
            // index that pointed there is still correct. Recording the range
            // and the generation lets a later read of an older state fail only
            // if it actually touches these bytes.
            Some(slot) => {
                self.reuse_log.push((slot.0, slot.1, self.generation));
                slot
            }
            None => {
                let cap = capacity_for(len);
                let off = self.off;
                self.off += cap as u64;
                (off, cap)
            }
        };
        // A block stored verbatim gets per-chunk checksums beside it, so a
        // reader can verify the 4KiB its extent lands in rather than all
        // 64KiB. A compressed block already carries per-chunk checksums in its
        // own directory, and a block too large to chunk this way falls back to
        // the whole-block checksum.
        let chunks = if block::checksums_on() && !chunked && stored.is_none() {
            block::chunk_crcs(bytes)
        } else {
            None
        };
        let loc = BlockLoc {
            off,
            stored: len,
            uncompressed: payload.len() as u32,
            cap,
            chunked,
            solo,
            chunk_crc: chunks.is_some(),
            crc: if block::checksums_on() {
                block::crc32(bytes)
            } else {
                0
            },
        };
        wled(&WL_BLOCKS, bytes.len());
        self.file.write_all_at(bytes, off)?;
        self.blocks.push(loc);
        self.chunk_crcs
            .push(chunks.unwrap_or([0u32; block::MAX_CHUNK_CRCS]));
        let want = (self.blocks.len() * block::MAX_CHUNK_CRCS).div_ceil(64);
        while self.verified.len() < want {
            self.verified.push(std::sync::atomic::AtomicU64::new(0));
        }
        self.live.push(0);
        Ok((self.blocks.len() - 1) as u32)
    }
}

/// A key's extent while it is still being built.
#[derive(Default)]
struct Pending {
    /// The buffered value, when `Options::pending_arena` is off.
    ///
    /// One heap allocation per key, and dhat measured what that costs: 50,305
    /// live blocks against LMDB's 2,077 on a 50k-key load, and 9.4 last-level
    /// misses per key against LMDB's 0.45. A load scatters its writes across
    /// tens of megabytes of small objects, five times the last-level cache,
    /// touched in hash order. See docs/profiling.md.
    buf: Vec<u8>,
    /// Where this key's buffered run lives in the shard arena, when
    /// `Options::pending_arena` is on. `len` is zero exactly when the arena is
    /// not in use for this entry, since a record always carries at least its
    /// own length varint.
    off: u32,
    len: u32,
    /// Offset of the most recently appended record within `buf`.
    last: u32,
    /// Extents this pending value replaces, to be released when it is sealed
    /// and only if the store is reclaiming.
    supersedes: Vec<Ext>,
    /// True when this came from put(): the sealed extent replaces the key's
    /// extents rather than being added to them.
    replaces: bool,
}

impl Pending {
    /// The buffered bytes, from whichever place holds them.
    #[inline]
    fn bytes<'a>(&'a self, arena: &'a [u8]) -> &'a [u8] {
        if self.len > 0 {
            &arena[self.off as usize..][..self.len as usize]
        } else {
            &self.buf
        }
    }
    #[inline]
    fn nbytes(&self) -> usize {
        if self.len > 0 {
            self.len as usize
        } else {
            self.buf.len()
        }
    }
}

struct Shard {
    merges: u64,
    /// One table holding both a key's sealed extents and the value still
    /// buffered for it, so a put probes once rather than twice.
    keys: KeyTable<Pending>,
    /// Buffered values for every key in this shard, appended end to end.
    ///
    /// The point is the write *pattern*, not the allocation count: values land
    /// consecutively and stream out of cache instead of scattering across one
    /// malloc block per key. A replaced value leaves its old bytes behind as
    /// garbage until the next seal clears the whole arena, which is why
    /// `pending_bytes` tracks the arena length rather than the live total --
    /// the seal threshold should fire on memory actually held.
    arena: Vec<u8>,
    pending_bytes: usize,
    builder: BlockBuilder,
    /// Extents already placed in the current block, awaiting its id.
    members: Vec<(u32, u32, u32, u32, bool)>,
    /// Keys whose extents changed since the last checkpoint, by table index.
    ///
    /// Without this a checkpoint has to ask every shard for every key just to
    /// find the handful that moved, which is O(key count) before the work
    /// starts -- and that was the whole cost the in-place path exists to
    /// avoid. Cleared when a checkpoint publishes them.
    dirty: Vec<u32>,
}

pub struct Store {
    /// Whether anything written is not yet in the published index.
    ///
    /// The first version of `has_unpublished` asked all 64 shards, taking and
    /// releasing 64 mutexes to answer one bit. `Store::scan` calls it on every
    /// scan, and it put 883ns on the fixed cost of one -- the whole of what
    /// `Store::scan` was built to save, spent finding out whether it needed to
    /// do anything. State that knows whether it is stale is the point; asking
    /// sixty-four locks is polling with extra steps.
    ///
    /// Cleared before a checkpoint gathers, so a write that lands during one
    /// sets it again rather than being lost.
    unpublished: std::sync::atomic::AtomicBool,
    /// Set for the lifetime of `close()`. A closing store takes the full
    /// rewrite unconditionally -- an in-place or logged final checkpoint would
    /// leave state in structures that only exist to defer work there will
    /// never be another chance to do -- and it ends with no log arena, because
    /// a store nothing will append to again has no use for 4 MB of reserved
    /// zeroes that every download of the file pays for.
    closing: std::sync::atomic::AtomicBool,
    shards: Vec<Mutex<Shard>>,
    appender: Mutex<Appender>,
    opts: Options,
    path: PathBuf,
}

impl Store {
    /// Reopen an existing store for writing.
    ///
    /// `create` truncates, and until now there was no other way in: a store
    /// could be written once and then only ever read. The architecture review
    /// calls that critical and lists it first, `CLAUDE.md` heads its
    /// known-failing list with it, and the comparison suite docks a feature
    /// point for it -- and it had no claim and no experiment, because the
    /// limitation made itself untestable.
    ///
    /// What has to be rebuilt is everything the appender keeps that the file
    /// does not spell out. The block table and its per-chunk checksums are
    /// read back from their section. The key tables are refilled from the
    /// published index, resharded by whatever `Options::shards` says now
    /// rather than what it said then, since the hash is over the key and
    /// nothing on disk depends on the split. Reference counts are not
    /// persisted at all and are recounted from the extents that survive,
    /// which is also how the free list gets seeded: a block that nothing
    /// points at is space this store may hand out again.
    ///
    /// Two things are deliberately given up rather than guessed at.
    ///
    /// `history_from` is set to the generation being opened, so snapshots
    /// older than the reopen are refused rather than served. The reuse log
    /// records which byte ranges were handed out again and in which
    /// generation; it is what lets a reader at an older generation fail
    /// loudly instead of reading whatever now occupies those bytes. Carrying
    /// a log across a reopen and then appending to it is the kind of
    /// bookkeeping that is wrong once and silently wrong forever, so this
    /// declares the break instead.
    ///
    /// The in-place checkpoint state starts empty, so the first checkpoint
    /// after a reopen rewrites the index rather than editing it. That costs
    /// one full checkpoint and cannot be wrong.
    pub fn open(path: &Path, opts: Options) -> Result<Store> {
        block::CHECKSUMS.store(opts.checksums, std::sync::atomic::Ordering::Relaxed);

        // Everything about the format, the two superblock slots and their
        // validation is already correct in `Reader::open`; duplicating it here
        // would mean two decoders that have to agree forever.
        let (generation, timestamp, blocks, chunk_crcs, nkeys, entries) = {
            let r = Reader::open(path)?;
            let blocks = r.all_blocks()?;
            let crcs = r.all_chunk_crcs();
            let n = r.keys();
            let mut entries: Vec<(Vec<u8>, Extents)> = Vec::with_capacity(n);
            for rank in 0..n {
                let Some((k, exts)) = r.entry_at(rank) else {
                    return Err(corrupt("published index is shorter than it says"));
                };
                let mut e = Extents::None;
                for x in exts {
                    e.push(*x);
                }
                entries.push((k.to_vec(), e));
            }
            let (g, t) = r.version();
            (g, t, blocks, crcs, n, entries)
        };

        // The high-water mark is the appender's cursor and the reader has no
        // use for it, so it comes from the superblock directly.
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let sb = {
            let map = unsafe { Mmap::map(&file)? };
            if map.len() < SUPER as usize {
                return Err(corrupt("file is shorter than its superblock"));
            }
            let a = Super::decode(&map[0..SUPER_BYTES]);
            let b = Super::decode(&map[SLOT as usize..SLOT as usize + SUPER_BYTES]);
            match (a, b) {
                (Some(x), Some(y)) if y.generation > x.generation => y,
                (Some(x), _) => x,
                (None, Some(y)) => y,
                (None, None) => {
                    let foreign = Super::wrong_endian(&map[0..SUPER_BYTES])
                        || Super::wrong_endian(&map[SLOT as usize..SLOT as usize + SUPER_BYTES]);
                    return Err(corrupt(if foreign {
                        "this store was written on a machine of the opposite byte order; \
                         Supdb addresses its index in place, so a file is only self-consistent \
                         on the byte order that wrote it"
                    } else {
                        "no readable superblock"
                    }));
                }
            }
        };
        let high_water = sb.high_water;
        // Replay the redo log over the published index.
        //
        // The records are the writes that were made durable without rewriting
        // the index, so they are newer than everything in it by construction,
        // and a key appearing in both takes its logged extents. Applied in
        // order, because a key may appear more than once and the last record
        // is the current one.
        let (logged, log_used): (Vec<(Vec<u8>, Extents)>, u64) = if sb.log_len == 0 {
            (Vec::new(), 0)
        } else {
            let map = unsafe { Mmap::map(&file)? };
            let (o, l) = (sb.log_off as usize, sb.log_len as usize);
            match map.get(o..o.saturating_add(l)) {
                Some(arena) => {
                    let (recs, used) = log_replay(arena);
                    // Only records newer than the index. An in-place
                    // checkpoint after a logged one leaves older records in
                    // the arena with nothing else marking them stale, and
                    // replaying them resurrected forty deleted keys in the
                    // reproducer. The stamp is the arbiter; the walk offset
                    // still covers every intact record so appends resume in
                    // the right place.
                    let keep: Vec<(Vec<u8>, Extents)> = recs
                        .into_iter()
                        .filter(|(g, _, _)| *g > sb.index_gen)
                        .map(|(_, k, e)| (k, e))
                        .collect();
                    (keep, used)
                }
                // A log the file is too short to contain is corruption, not an
                // empty log: pretending it is empty would silently drop writes
                // that were acknowledged as durable.
                None => return Err(corrupt("redo log lies outside the file")),
            }
        };
        // Where the published index lives, so `scan` has an order to walk and
        // the next checkpoint has a predecessor to point at. Its reserved
        // capacity is not recorded anywhere, so `stored` stands in: releasing
        // it later then frees less than was taken, which leaks padding. The
        // other rounding would free bytes belonging to whatever follows.
        let last_index = (sb.key_stored > 0).then(|| BlockLoc {
            off: sb.key_off,
            stored: sb.key_stored as u32,
            uncompressed: sb.key_uncompressed as u32,
            cap: sb.key_stored as u32,
            chunked: false,
            solo: false,
            chunk_crc: false,
            crc: 0,
        });
        if high_water < SUPER || high_water > file.metadata()?.len() {
            return Err(corrupt("superblock's high-water mark is outside the file"));
        }

        let shards: Vec<Mutex<Shard>> = (0..opts.shards)
            .map(|_| {
                Mutex::new(Shard {
                    merges: 0,
                    keys: KeyTable::new(),
                    pending_bytes: 0,
                    builder: BlockBuilder::new(opts.block_size),
                    arena: Vec::new(),
                    members: Vec::new(),
                    dirty: Vec::new(),
                })
            })
            .collect();

        // Refcounts are not on disk. Every surviving extent is one reference,
        // and a block nothing references is free space this store may reuse --
        // which is where the free list comes from too, since it is not
        // persisted either.
        let mut live = vec![0u32; blocks.len()];
        let table = unsafe { MmapMut::map_mut(&file)? };
        let store = Store {
            unpublished: std::sync::atomic::AtomicBool::new(false),
            closing: std::sync::atomic::AtomicBool::new(false),
            shards,
            appender: Mutex::new(Appender {
                table,
            log: None,
            index_gen: 0,
            prev_log: None,
                map: None,
                file,
                off: high_water,
                blocks,
                chunk_crcs,
                verified: Vec::new(),
                live: Vec::new(),
                free: FreeList::new(),
                generation,
                timestamp,
                reuse_log: Vec::new(),
                last_index,
                index_history: Vec::new(),
                since_sync: 0,
                last_sync: std::time::Instant::now(),
                unsynced: false,
                // History before this reopen is declared broken rather than
                // guessed at; see the note above.
                history_from: generation,
                live_index: None,
                live_key_off: None,
            }),
            opts,
            path: path.to_path_buf(),
        };

        // Logged records last, so a key present in both takes the newer
        // extents. `put`-style replacement is what a log record means: it
        // carries the key's whole extent list as of that checkpoint, not a
        // delta against it.
        let replayed: Vec<Vec<u8>> = logged.iter().map(|(k, _)| k.clone()).collect();
        // Not chained with `logged`: the reader those entries came from now
        // replays the log itself, so they already carry it. Applying it twice
        // was harmless for extents, which replace, and wrong for the count.
        for (key, exts) in entries {
            let si = store.shard_of(&key);
            let mut sh = store.shards[si].lock().unwrap();
            sh.keys.get_or_insert(&key).extents = exts;
        }
        // A replayed record is durable and *not* published: it is in no index
        // section, so `scan`, which walks the published order, cannot see it.
        // Marking the keys dirty and the store unpublished says exactly that,
        // and the next publish or checkpoint writes them into the index.
        //
        // Without this the two read paths disagreed after a reopen --
        // `read_all` answered from the shards and found every replayed key,
        // while `scan` walked the index and saw only what predated the log.
        // A test that checked just `read_all` would have passed.
        if !replayed.is_empty() {
            for key in &replayed {
                let si = store.shard_of(key);
                let mut sh = store.shards[si].lock().unwrap();
                if let Some(idx) = sh.keys.index_of(key) {
                    sh.dirty.push(idx);
                }
            }
            store
                .unpublished
                .store(true, std::sync::atomic::Ordering::Release);
        }
        // Refcounts are taken from the merged result, not from each list as it
        // arrives. A key present in both the index and the log would otherwise
        // be counted twice -- once for extents the log has already superseded
        // -- and those blocks would never be reclaimed. Over-counting leaks
        // rather than corrupts, which is why it would have gone unnoticed.
        for sh in &store.shards {
            let sh = sh.lock().unwrap();
            for (_, e) in sh.keys.iter() {
                for x in e.extents.as_slice() {
                    match live.get_mut(x.block as usize) {
                        Some(c) => *c += 1,
                        None => {
                            return Err(corrupt("index names a block the table does not have"))
                        }
                    }
                }
            }
        }
        {
            let mut ap = store.appender.lock().unwrap();
            // Resume the arena where replay stopped, so a reopened store keeps
            // logging instead of falling back to a full rewrite on its next
            // durable checkpoint. `log_used` is where the walk ended, which is
            // the first byte not covered by an intact record -- so a torn tail
            // from a crash is overwritten rather than kept.
            if sb.log_len > 0 && store.opts.redo_log {
                ap.log = Some((sb.log_off, sb.log_len, log_used));
            }
            ap.index_gen = sb.index_gen;
            {
            }
            let nblocks = ap.blocks.len();
            ap.verified = (0..(nblocks * block::MAX_CHUNK_CRCS).div_ceil(64))
                .map(|_| std::sync::atomic::AtomicU64::new(0))
                .collect();
            // Free space is the complement of what is occupied, not the union
            // of what looks dead. A block id is never reused, so `blocks`
            // keeps entries for blocks released long ago -- and their offsets
            // may since have been handed to a section or another block.
            // Releasing every zero-refcount entry hands out space that is no
            // longer its own, which is how this first presented: twenty keys
            // of a thousand read back as a checksum mismatch under
            // Reclaim::Now, after `close` trimmed a file whose free list
            // claimed the tail was empty. This repository has met that shape
            // before, in the double release that gave one range to two blocks.
            let mut used: Vec<(u64, u64)> = Vec::with_capacity(ap.blocks.len() + 3);
            for (i, c) in live.iter().enumerate() {
                if *c > 0 {
                    let loc = ap.blocks[i];
                    used.push((loc.off, loc.off + loc.cap as u64));
                }
            }
            // The three sections the superblock points at are occupied too,
            // and none of them is a block. Their reserved capacity is not
            // recorded, so `stored` stands in: the padding past it is not read
            // by anyone, so handing it out cannot corrupt what is.
            // The log arena is the fourth. Leaving it out is how a reopened
            // store hands its own live redo log to the next allocation: the
            // comment above describes that symptom exactly, and this is a
            // second way to reach it.
            if sb.log_len > 0 {
                used.push((sb.log_off, sb.log_off + sb.log_len));
            }
            for (off, len) in [
                (sb.key_off, sb.key_stored),
                (sb.blk_off, sb.blk_stored),
                (sb.reuse_off, sb.reuse_stored),
            ] {
                if off > 0 && len > 0 {
                    used.push((off, off + len));
                }
            }
            used.sort_unstable();
            let mut at = SUPER;
            for (lo, hi) in used {
                if lo > at {
                    ap.free.release(at, (lo - at) as u32, generation);
                }
                at = at.max(hi);
            }
            if at < high_water {
                ap.free.release(at, (high_water - at) as u32, generation);
            }
            ap.live = live;
        }
        debug_assert_eq!(
            store
                .shards
                .iter()
                .map(|s| s.lock().unwrap().keys.len())
                .sum::<usize>(),
            nkeys
        );
        Ok(store)
    }

    pub fn create(path: &Path, opts: Options) -> Result<Store> {
        // opened for reading too: compaction reads written extents back
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        block::CHECKSUMS.store(opts.checksums, std::sync::atomic::Ordering::Relaxed);
        let shards = (0..opts.shards)
            .map(|_| {
                Mutex::new(Shard {
                    merges: 0,
                    keys: KeyTable::new(),
                    pending_bytes: 0,
                    builder: BlockBuilder::new(opts.block_size),
                    arena: Vec::new(),
                    members: Vec::new(),
                    dirty: Vec::new(),
                })
            })
            .collect();
        // the first page must exist before it can be mapped
        file.set_len(SUPER)?;
        let table = unsafe { MmapMut::map_mut(&file)? };
        Ok(Store {
            unpublished: std::sync::atomic::AtomicBool::new(false),
            closing: std::sync::atomic::AtomicBool::new(false),
            shards,
            appender: Mutex::new(Appender {
                table,
            log: None,
            index_gen: 0,
            prev_log: None,
                map: None,

                file,
                // the first page is reserved for the two superblock slots
                off: SUPER,
                blocks: Vec::new(),
                chunk_crcs: Vec::new(),
                verified: Vec::new(),
                live: Vec::new(),
                free: FreeList::new(),
                generation: 0,
                timestamp: 0,
                reuse_log: Vec::new(),
                last_index: None,
                index_history: Vec::new(),
                since_sync: 0,
                last_sync: std::time::Instant::now(),
                unsynced: false,
                history_from: 0,
                live_index: None,
                live_key_off: None,
            }),
            opts,
            path: path.to_path_buf(),
        })
    }

    fn shard_of(&self, key: &[u8]) -> usize {
        // FxHash-style multiply-xor; keys here are short and highly similar,
        // so a cheap mixer that still moves the low bits is what is wanted.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in key {
            h ^= b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        ((h >> 32) as usize) % self.shards.len()
    }

    /// Append one value to a key. This is the path that has to stay fast --
    /// it is the axis the whole design is built to win.
    pub fn append(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.mark_unpublished();
        let si = self.shard_of(key);
        let mut sh = self.shards[si].lock().unwrap();
        let arena_on = self.opts.pending_arena;
        let budget = self.opts.buffer_bytes / self.shards.len();
        {
            let Shard {
                keys,
                arena,
                pending_bytes,
                ..
            } = &mut *sh;
            let e = keys.get_or_insert(key);
            let p = e.pending.get_or_insert_with(Pending::default);
            if arena_on {
                if arena.capacity() == 0 {
                    arena.reserve(budget);
                }
                // A key's run has to stay contiguous, and another key may have
                // appended since this one last did. When that has happened,
                // copy the run to the tail first; when it has not -- which is
                // every append in a run of appends to the same key -- extend
                // in place and copy nothing.
                if p.len > 0 && (p.off + p.len) as usize != arena.len() {
                    let (from, n) = (p.off as usize, p.len as usize);
                    let moved = arena.len() as u32;
                    arena.extend_from_within(from..from + n);
                    p.off = moved;
                }
                if p.len == 0 {
                    p.off = arena.len() as u32;
                }
                p.last = p.len;
                put_uvarint(arena, value.len() as u64);
                arena.extend_from_slice(value);
                p.len = arena.len() as u32 - p.off;
                *pending_bytes = arena.len();
            } else {
                let before = p.buf.len();
                p.last = before as u32;
                put_uvarint(&mut p.buf, value.len() as u64);
                p.buf.extend_from_slice(value);
                *pending_bytes += p.buf.len() - before;
            }
        }

        if sh.pending_bytes >= self.opts.buffer_bytes / self.shards.len() {
            self.seal_shard(&mut sh)?;
        }
        Ok(())
    }

    /// Seal every buffered extent in this shard.
    ///
    /// The batch is sorted by key on the way out. It is already being copied,
    /// so ordering it costs almost nothing here and yields sorted runs --
    /// which is what an ordered scan would need, and what cannot be recovered
    /// later without rewriting the data.
    fn seal_shard(&self, sh: &mut Shard) -> Result<()> {
        if sh.pending_bytes == 0 {
            return Ok(());
        }
        let mut batch: Vec<(u32, Pending)> = sh.keys.take_pending();
        sh.pending_bytes = 0;
        // Taken out rather than borrowed so the loop can hold `&arena` while
        // it mutates `builder` and `members`. The capacity comes back at the
        // end; an error path loses the buffered bytes, which is already true
        // of `batch` and `pending_bytes` above.
        let arena = std::mem::take(&mut sh.arena);
        // sorted by the keys the indices point at, with no key copied
        sh.keys.sort_by_key(&mut batch);

        for (idx, p) in batch {
            // Whatever this extent replaces stops being referenced now. For a
            // replacement the superseded extents are whatever the index still
            // holds for the key, resolved here rather than on the hot path.
            //
            // The index reference has to be dropped in the same breath as the
            // release. This previously released the blocks and left
            // `entry.extents` pointing at them until `flush_builder` got round
            // to overwriting it -- a window in which the entry named blocks
            // whose refcount had already reached zero and whose space was back
            // in the free list. A subsequent non-replacing append would then
            // `push` onto that stale list rather than replacing it, and the
            // writer's own merge path would try to decode a block that had
            // since been handed to somebody else. The differential oracle
            // found it as three block ids describing one byte range.
            //
            // Clearing is unconditional: `replaces` means the previous values
            // are gone whether or not this store is reclaiming their space.
            let superseded: Vec<Ext> = if p.replaces {
                let e = sh.keys.entry_at(idx);
                let old = e.extents.as_slice().to_vec();
                e.extents = Extents::None;
                old
            } else {
                // Never populated: an append supersedes nothing.
                p.supersedes.clone()
            };
            if self.opts.reclaim.releases() && !superseded.is_empty() {
                let mut ap = self.appender.lock().unwrap();
                for e in &superseded {
                    ap.release(e.block)?;
                }
            }
            let pbytes = p.bytes(&arena);
            if pbytes.len() >= self.opts.solo_threshold {
                // big enough to compress on its own; giving it a private block
                // means a read of this key decompresses only this key
                let id = {
                    let mut ap = self.appender.lock().unwrap();
                    ap.write_block(
                        pbytes,
                        self.opts.compress,
                        true,
                        self.opts.solo_chunk_size,
                        self.opts.reclaim,
                    )?
                };
                let len = pbytes.len() as u32;
                self.appender.lock().unwrap().retain(id);
                let ext = Ext {
                    block: id,
                    off: 0,
                    len,
                    last: p.last,
                };
                {
                    let entry = sh.keys.entry_at(idx);
                    if p.replaces {
                        entry.extents = Extents::One(ext);
                    } else {
                        entry.extents.push(ext);
                    }
                }
                sh.dirty.push(idx);
                self.merge_key(sh, idx)?;
                continue;
            }
            if sh.builder.would_overflow(pbytes.len()) {
                self.flush_builder(sh)?;
            }
            let off = sh.builder.push(pbytes);
            sh.members
                .push((idx, off, pbytes.len() as u32, p.last, p.replaces));
        }
        // Keep the capacity: clearing is what makes the next batch of writes
        // land in already-reserved memory instead of growing again.
        sh.arena = arena;
        sh.arena.clear();
        Ok(())
    }

    /// Write the current block and only then record where each extent landed:
    /// a block's id is not known until it is placed in the file.
    fn flush_builder(&self, sh: &mut Shard) -> Result<()> {
        if sh.builder.is_empty() {
            return Ok(());
        }
        let payload = sh.builder.take();
        let id = {
            let mut ap = self.appender.lock().unwrap();
            ap.write_block(
                &payload,
                self.opts.compress,
                false,
                self.opts.chunk_size,
                self.opts.reclaim,
            )?
        };
        {
            let mut ap = self.appender.lock().unwrap();
            for _ in 0..sh.members.len() {
                ap.retain(id);
            }
        }
        let touched: Vec<u32> = sh
            .members
            .drain(..)
            .map(|(idx, off, len, last, replaces)| {
                let ext = Ext {
                    block: id,
                    off,
                    len,
                    last,
                };
                let entry = sh.keys.entry_at(idx);
                if replaces {
                    entry.extents = Extents::One(ext);
                } else {
                    entry.extents.push(ext);
                }
                idx
            })
            .collect();
        sh.dirty.extend_from_slice(&touched);
        for idx in touched {
            self.merge_key(sh, idx)?;
        }
        Ok(())
    }

    /// Replace a key's values with a single new value.
    ///
    /// The new version is appended and the index is repointed at it; the old
    /// extents are released only under Retain::Reclaim. Nothing is ever
    /// overwritten in place, so an earlier checkpoint that still points at the
    /// old extents keeps reading the old value.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.mark_unpublished();
        let si = self.shard_of(key);
        let mut sh = self.shards[si].lock().unwrap();
        // Buffer it like an append rather than writing a block of its own.
        // Giving each replaced value a private block cost a whole size class
        // per value: a hundred bytes reserved four kilobytes, and writing a
        // million of them produced a 4.6 GB file for 100 MB of data. Small
        // values have to share a block, whether they arrive by append or by
        // replacement.
        // What this replaces is not looked up here. Finding it costs a hash
        // and a probe on the hot path, and the index is already in hand at
        // seal time, where the new extent has to be recorded anyway. Putting a
        // value therefore touches one map, not two.
        let arena_on = self.opts.pending_arena;
        let budget = self.opts.buffer_bytes / self.shards.len();
        let put_idx;
        {
            // Disjoint field borrows: the entry lives in `keys` and the bytes
            // go in `arena`, so both are needed at once. Probing twice to
            // avoid that would undo the single-probe property this path was
            // built for.
            let Shard {
                keys,
                arena,
                pending_bytes,
                ..
            } = &mut *sh;
            let idx = keys.slot_or_insert(key);
            put_idx = idx;
            let e = keys.entry_at(idx);
            let p = e.pending.get_or_insert_with(Pending::default);
            let before = p.nbytes();
            if arena_on {
                // Start at a size that covers a batch or two and let it grow
                // from there.
                //
                // This used to reserve the shard's whole `buffer_bytes` share
                // on first use, on the theory that growth memcpys explained
                // the 21% rise in last-level misses the arena caused. They did
                // not: reserving removed every one of those copies and the
                // misses did not move. So the reservation was answering a
                // question it turned out not to be the answer to, while
                // costing 2.2x resident memory (F25.3) for an 8% append-path
                // gain that flips one run in five (F25.2).
                if arena.capacity() == 0 {
                    arena.reserve(ARENA_START.min(budget));
                }
                // A replacement abandons the old run where it lies. The bytes
                // stay until the next seal clears the arena; reclaiming them
                // here would mean moving everything after them.
                let off = arena.len() as u32;
                put_uvarint(arena, value.len() as u64);
                arena.extend_from_slice(value);
                p.off = off;
                p.len = arena.len() as u32 - off;
                p.buf = Vec::new();
            } else {
                p.buf.clear();
                put_uvarint(&mut p.buf, value.len() as u64);
                p.buf.extend_from_slice(value);
                p.len = 0;
            }
            p.last = 0;
            p.replaces = true;
            let after = p.nbytes();
            if arena_on {
                *pending_bytes = arena.len();
            } else {
                *pending_bytes = *pending_bytes + after - before;
            }
        }
        // Same hazard as `delete`: a replacement supersedes every earlier
        // value, including one already staged in the block builder by an
        // inline seal. Left in place, `flush_builder` would push the
        // superseded extent onto the entry after this replacement lands.
        // The index came from the probe above rather than a second one.
        if !sh.members.is_empty() {
            sh.members.retain(|m| m.0 != put_idx);
        }
        // Honour the buffer the caller asked for. `append` has always sealed
        // here and `put` never did, so a put-only workload -- which is every
        // load phase in the external suite -- ignored `buffer_bytes` entirely
        // and grew until `flush`. At the sizes measured that is invisible,
        // because 1M values of 100 bytes is 105MB against a 256MB budget and
        // the threshold is never reached. It is not invisible at ten times the
        // keys, where the buffer the caller set is the difference between
        // streaming and running out of memory.
        //
        // The comment above is the reason this is safe to add rather than a
        // new hazard: `put` already had to cope with an extent staged by an
        // inline seal, because `append` could cause one.
        if self.opts.seal_on_put
            && sh.pending_bytes >= self.opts.buffer_bytes / self.shards.len()
        {
            self.seal_shard(&mut sh)?;
        }
        Ok(())
    }

    /// Delete a key by leaving a tombstone.
    ///
    /// The entry stays in the index with no extents, which is what
    /// distinguishes a deleted key from one that was never written -- and what
    /// lets an older snapshot still find the value while the current
    /// generation reports it gone.
    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.mark_unpublished();
        let si = self.shard_of(key);
        let mut sh = self.shards[si].lock().unwrap();
        let (freed, old) = {
            let e = sh.keys.get_or_insert(key);
            let freed = e.pending.take().map(|p| p.nbytes()).unwrap_or(0);
            let old = e.extents.as_slice().to_vec();
            e.extents = Extents::None;
            (freed, old)
        };
        sh.pending_bytes -= freed;
        // Clearing `extents` is not enough on its own. An earlier `append` may
        // have filled the shard buffer and triggered an inline `seal_shard`,
        // which stages the extent in the block builder and records it in
        // `members` -- but the block has no id yet, so nothing has been
        // written to `extents` for `delete` to clear. When `flush_builder`
        // later assigns the id it pushes the staged extent onto the entry and
        // the deleted key comes back with every value it had.
        //
        // The staged bytes stay in the block; they are simply no longer
        // referenced. `flush_builder` retains the block once per surviving
        // member, so the refcount stays correct.
        if let Some(idx) = sh.keys.index_of(key) {
            sh.members.retain(|m| m.0 != idx);
            // A tombstone is a change like any other, and it was not recorded
            // as one. `checkpoint_in_place` and the redo log both publish only
            // what `dirty` names, so a delete they were asked to carry was
            // simply dropped and the key stayed readable at its old extents.
            //
            // It was invisible while any insertion forced a full rewrite,
            // because a rewrite reads every key from the shards and sees the
            // tombstone directly. `Options::index_inserts` removes that
            // rewrite, which is what exposed it -- the bug is older than
            // either flag.
            sh.dirty.push(idx);
        }
        if self.opts.reclaim.releases() {
            let mut ap = self.appender.lock().unwrap();
            for x in &old {
                ap.release(x.block)?;
            }
        }
        Ok(())
    }

    /// Move live blocks down into freed space, then truncate.
    ///
    /// A block is named by its id, and the only place a file offset appears is
    /// the block index, so relocating one is a copy plus a single entry
    /// update -- no key ever has to be touched, however many of them point at
    /// it. That is what makes reclaiming interior space practical without any
    /// filesystem cooperation, and why hole punching is not needed: it worked
    /// only on some platforms and left the file sparse and scattered.
    ///
    /// Bounded by `max_moves` so it can be run in slices. Stopping early is
    /// always safe: every state it passes through is one the block index
    /// already describes.
    pub fn defragment(&self, max_moves: usize) -> Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut ap = self.appender.lock().unwrap();
        let mut moved = 0usize;
        while moved < max_moves {
            let holes = ap.free.coalesced();
            let Some(&(hole_off, hole_len)) = holes.first() else {
                break;
            };
            // the highest-offset live block that fits in this hole
            let mut best: Option<(usize, u64)> = None;
            for (i, b) in ap.blocks.iter().enumerate() {
                if ap.live[i] == 0 || b.off <= hole_off {
                    continue;
                }
                if (b.cap as u64) <= hole_len {
                    match best {
                        Some((_, off)) if off >= b.off => {}
                        _ => best = Some((i, b.off)),
                    }
                }
            }
            let Some((id, _)) = best else { break };
            let loc = ap.blocks[id];
            let mut buf = vec![0u8; loc.stored as usize];
            ap.file.read_exact_at(&mut buf, loc.off)?;
            wled(&WL_DEFRAG, buf.len());
            ap.file.write_all_at(&buf, hole_off)?;
            // one entry changes; every extent pointing here is untouched
            ap.blocks[id].off = hole_off;
            let gen = ap.generation;
            ap.free.release(loc.off, loc.cap, gen);
            ap.free.take_at(hole_off, loc.cap);
            moved += 1;
        }
        // give back whatever is now trailing
        let holes = ap.free.coalesced();
        let mut end = ap.off;
        for &(off, len) in holes.iter().rev() {
            if off + len == end {
                end = off;
            } else {
                break;
            }
        }
        if end < ap.off {
            ap.off = end;
            let _ = ap.file.set_len(end);
        }
        Ok(moved)
    }

    /// Merge one key's extents into a single contiguous run, in place.
    ///
    /// Called from the seal path, so the writer that fragmented the key pays
    /// for it. The old extents are released, which punches their blocks out
    /// once nothing references them.
    fn merge_key(&self, sh: &mut Shard, idx: u32) -> Result<()> {
        let exts = {
            let e = &sh.keys.entry_at(idx).extents;
            if e.as_slice().len() < self.opts.merge_threshold {
                return Ok(());
            }
            e.as_slice().to_vec()
        };
        let mut buf = Vec::new();
        let mut last = 0u32;
        let mut ap = self.appender.lock().unwrap();
        for e in &exts {
            let bytes = ap.read_extent(*e)?;
            last = (buf.len() as u32) + e.last;
            buf.extend_from_slice(&bytes);
        }
        let len = buf.len() as u32;
        let id = ap.write_block(
            &buf,
            self.opts.compress,
            true,
            self.opts.solo_chunk_size,
            self.opts.reclaim,
        )?;
        ap.retain(id);
        // Only when reclaiming. Merging relocates a key's values; the originals
        // are unreachable from the current index either way, but an older
        // checkpoint still points at them and a reader may be holding it.
        // Releasing here unconditionally is what let Retain::Snapshots hand
        // out space it had promised to keep.
        if self.opts.reclaim.releases() {
            for e in &exts {
                ap.release(e.block)?;
            }
        }
        drop(ap);
        sh.dirty.push(idx);
        sh.keys.entry_at(idx).extents = Extents::One(Ext {
            block: id,
            off: 0,
            len,
            last,
        });
        sh.merges += 1;
        Ok(())
    }

    /// Make everything written so far recoverable.
    ///
    /// Data first, then the slot that points at it, with a barrier between:
    /// a checkpoint that reached the slot but not the data would describe
    /// blocks that do not exist. Blocks written after the last checkpoint are
    /// simply unreachable after a crash -- leaked space, not corruption.
    /// Read a key's values without publishing anything.
    ///
    /// The gap this closes is structural, not incremental. `Store` had no read
    /// method, so seeing your own write meant `checkpoint()` plus a fresh
    /// `Reader` -- and until recently that pair cost an index rewrite, two
    /// device flushes and an O(key count) open. LMDB needs none of it: a write
    /// is visible to the handle that made it, immediately. That difference is
    /// the whole of `EXT.3`, and the reason the mixed YCSB workloads sit two
    /// orders of magnitude behind while read-only sits ahead.
    ///
    /// A key's values can be in three places at once, and this reads all
    /// three, oldest first:
    ///
    ///   1. **Sealed extents**, in blocks already written to the file.
    ///   2. **Staged bytes**, pushed into the block builder but not yet
    ///      written, so their block id does not exist yet.
    ///   3. **Pending bytes**, buffered against the key and not yet sealed.
    ///
    /// A `put` complicates the order: it marks its pending value `replaces`,
    /// and the sealed extents it supersedes are not cleared until the seal
    /// happens. So a pending replacement hides everything before it, and a
    /// staged replacement hides everything sealed. Getting that wrong would
    /// resurrect deleted values, which is a bug this repository has already
    /// had once and keeps a reproducer for.
    ///
    /// Returns the number of values emitted.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let si = self.shard_of(key);
        // Shared, not `mut`: every borrow below is a read, and taking the
        // entry mutably forced this path to clone the extent list, the staged
        // list and the pending buffer on every call.
        let sh = self.shards[si].lock().unwrap();
        let Some(idx) = sh.keys.index_of(key) else {
            return Ok(0);
        };
        let e = sh.keys.entry(idx);
        let sealed = e.extents.as_slice();
        let (pending_buf, pending_replaces) = match &e.pending {
            Some(p) => (p.bytes(&sh.arena), p.replaces),
            None => (&[][..], false),
        };
        // The last staged member for this key that replaces what came before.
        // Everything staged before it is hidden, and its presence hides the
        // sealed extents entirely.
        let last_replace = sh
            .members
            .iter()
            .rposition(|(i, _, _, _, r)| *i == idx && *r);
        let staged_replaces = last_replace.is_some();

        let mut n = 0u64;
        let mut emit_count = |bytes: &[u8], f: &mut F| -> Result<()> {
            let mut p = 0usize;
            while p < bytes.len() {
                let len = get_uvarint(bytes, &mut p) as usize;
                let end = p
                    .checked_add(len)
                    .ok_or_else(|| corrupt("record length overflows"))?;
                let Some(rec) = bytes.get(p..end) else {
                    return Err(corrupt("record runs past the end of its extent"));
                };
                f(rec);
                n += 1;
                p = end;
            }
            Ok(())
        };

        // Oldest first: sealed, then staged, then pending. A replacement
        // anywhere later hides everything earlier.
        if !pending_replaces && !staged_replaces && !sealed.is_empty() {
            let mut ap = self.appender.lock().unwrap();
            // Map far enough for the furthest block these extents name, once,
            // rather than testing the length per extent.
            let need = sealed
                .iter()
                .filter_map(|e| ap.blocks.get(e.block as usize))
                .map(|l| l.off + l.stored as u64)
                .max()
                .unwrap_or(0);
            ap.ensure_map(need)?;
            let ap = &*ap;
            let mut scratch = Vec::new();
            for e in sealed {
                // `extent_bytes` narrows to the extent's own bytes; slicing by
                // `off` again reads from the wrong place, which is how this
                // first reported "extent runs past its block".
                let bytes = ap.extent_bytes(*e, &mut scratch, self.opts.verify_reads)?;
                emit_count(bytes, &mut f)?;
            }
        }
        if !pending_replaces {
            let from = last_replace.unwrap_or(0);
            for (pos, (i, off, len, _, _)) in sh.members.iter().enumerate() {
                if *i != idx || pos < from {
                    continue;
                }
                let (a, b) = (*off as usize, (*off + *len) as usize);
                let slice = sh
                    .builder
                    .staged()
                    .get(a..b)
                    .ok_or_else(|| corrupt("staged extent runs past the builder"))?;
                emit_count(slice, &mut f)?;
            }
        }
        emit_count(pending_buf, &mut f)?;
        Ok(n)
    }

    /// Publish and persist: make everything written so far visible to new
    /// readers, and flush it to the device according to `Options::sync`.
    pub fn checkpoint(&self) -> Result<u64> {
        self.checkpoint_inner(self.opts.sync)
    }

    /// Publish without persisting: make writes visible to new readers, and let
    /// `Options::sync` decide nothing -- this call never flushes.
    ///
    /// The distinction is the point. Readers map the same file, so visibility
    /// costs nothing beyond the page cache, and a process crash cannot lose
    /// what this published. Only a power cut can, and only back to the last
    /// flush. On the read-your-writes shape that is worth 31x (`f13-sync`),
    /// which is the whole of the gap between Supdb and LMDB on the mixed YCSB
    /// workloads.
    ///
    /// Pair it with `sync` at whatever boundary the caller actually cares
    /// about -- a batch, a second, a user-visible acknowledgement.

    /// Is there anything written that a reader could not see?
    ///
    /// The external adapter carried a `dirty` bool for this, set by hand in
    /// `write_batch` and `sync` and cleared in `refresh`. A caller-maintained
    /// flag is a footgun -- add a write path, forget the line, and reads go
    /// stale silently -- and it was only there because the engine would not
    /// answer the question. `pending_bytes`, `members` and `dirty` are the
    /// answer and the store has always had them.
    pub fn has_unpublished(&self) -> bool {
        self.unpublished.load(std::sync::atomic::Ordering::Acquire)
    }

    #[inline]
    fn mark_unpublished(&self) {
        use std::sync::atomic::Ordering;
        // Load first: the common case is already-set, and a load of a shared
        // line is cheap where a store to one is not.
        if !self.unpublished.load(Ordering::Relaxed) {
            self.unpublished.store(true, Ordering::Release);
        }
    }

    /// The published generation. A reader at the same one is current.
    pub fn generation(&self) -> u64 {
        self.appender.lock().unwrap().generation
    }

    /// Walk keys in order from `from`, without opening a `Reader`.
    ///
    /// The writer already holds everything an ordered walk needs: the
    /// published index is a mapped section it can parse in place, and the
    /// extents it names resolve through the same mapping and the same verified
    /// bitset that `read_all` warms. A `Reader` duplicates both -- its own
    /// mapping of the same file, its own bitset starting empty -- so a scan
    /// through one pays to re-verify a store this process just wrote.
    ///
    /// That is most of why the kv suite reports Supdb scanning at 0.77x of
    /// LMDB while an interleaved sweep has it ahead at every length from 100
    /// up: the suite reads through the writer, then scans through a cold
    /// reader, while LMDB does both through one warm handle. Measured solo and
    /// warm, Supdb walks at 11.72 ns/entry against LMDB's 13.03.
    ///
    /// Publishes first if there is anything unpublished, because ordered
    /// access is over the index and the index is what publishing writes. A
    /// scan of a store with nothing outstanding costs nothing extra.
    pub fn scan<F: FnMut(&[u8], &[u8])>(
        &self,
        from: Option<&[u8]>,
        limit: usize,
        mut f: F,
    ) -> Result<u64> {
        if self.has_unpublished() {
            self.publish()?;
        }
        let mut ap = self.appender.lock().unwrap();
        let Some(loc) = ap.last_index else {
            // Nothing has ever been published, so there is no order to walk.
            return Ok(0);
        };
        let end = loc.off + loc.stored as u64;
        ap.ensure_map(end)?;
        let ap = &*ap;
        let Some(map) = &ap.map else {
            return Err(corrupt("index section is not mapped"));
        };
        let Some(sec) = map.get(loc.off as usize..end as usize) else {
            return Err(corrupt("index section runs past the mapping"));
        };
        // Only the flat format can be read in place. The varint one would have
        // to be decoded into a private copy, which is the cost this avoids.
        let Some(idx) = flatindex::FlatIndex::parse(sec) else {
            return Err(corrupt("published index is not readable in place"));
        };

        let start = from.map_or(0, |k| idx.seek_with(sec, k, true));
        let stop = start.saturating_add(limit).min(idx.len());
        let mut n = 0u64;
        let mut scratch: Vec<u8> = Vec::new();
        for rank in start..stop {
            let Some((key, exts)) = idx.at(sec, rank) else {
                continue;
            };
            for e in exts {
                let bytes = ap.extent_bytes(*e, &mut scratch, self.opts.verify_reads)?;
                let mut p = 0usize;
                while p < bytes.len() {
                    let len = get_uvarint(bytes, &mut p) as usize;
                    let end = p
                        .checked_add(len)
                        .ok_or_else(|| corrupt("record length overflows"))?;
                    let Some(rec) = bytes.get(p..end) else {
                        return Err(corrupt("record runs past the end of its extent"));
                    };
                    f(key, rec);
                    n += 1;
                    p = end;
                }
            }
        }
        Ok(n)
    }

    pub fn publish(&self) -> Result<u64> {
        self.checkpoint_inner(Sync::Never)
    }

    /// Flush everything published so far to the device.
    ///
    /// Cheap and a no-op when nothing has been published since the last flush,
    /// so calling it on a timer or per batch costs nothing when idle.
    pub fn sync(&self) -> Result<()> {
        let mut ap = self.appender.lock().unwrap();
        if !ap.unsynced {
            return Ok(());
        }
        ap.file.sync_data()?;
        ap.since_sync = 0;
        ap.last_sync = std::time::Instant::now();
        ap.unsynced = false;
        Ok(())
    }

    /// The one implementation. `policy` is the caller's, not necessarily the
    /// store's: `publish` passes `Never` for a single call.
    fn checkpoint_inner(&self, policy: Sync) -> Result<u64> {
        use std::os::unix::fs::FileExt;
        self.flush()?;
        // Gather only what moved. Asking every shard for every key just to
        // find the handful that changed is O(key count) before any work
        // starts, and that was the whole cost the in-place path exists to
        // avoid -- it made an incremental checkpoint of 100 updates cost the
        // same 24ms as a full one.
        // Cleared before the gather, not after: a write that lands while this
        // is running must leave the flag set rather than be forgotten.
        self.unpublished
            .store(false, std::sync::atomic::Ordering::Release);
        let mut changed: Vec<(Vec<u8>, Extents)> = Vec::new();
        let mut nkeys = 0usize;
        for sh in &self.shards {
            let mut sh = sh.lock().unwrap();
            nkeys += sh.keys.len();
            for idx in std::mem::take(&mut sh.dirty) {
                let key = sh.keys.key_at(idx).to_vec();
                let exts = sh.keys.entry_at(idx).extents.clone();
                changed.push((key, exts));
            }
        }
        let closing = self.closing.load(std::sync::atomic::Ordering::Acquire);
        // In-place stays legal while closing -- it lands state durably in the
        // mapped index, which is exactly where a final checkpoint wants it,
        // and forcing the full rewrite instead was measured to inflate a tiny
        // store by two extra quarantined section generations. Only the LOG
        // path is forbidden below: a closing store is about to release its
        // arena, and records appended to a structure being released are lost.
        let in_place_edit = self.checkpoint_in_place(&changed, nkeys)?;
        // Only a durability point may use the log. `publish` passes
        // `Sync::Never` and wants *visibility*, which the log does not give: a
        // logged record is durable and replayed by `Store::open`, and a
        // `Reader` opened before the next full rewrite does not see it. Taking
        // this path for a publish would make a scan miss writes that the
        // writer had already been told were published.
        let logged = !closing
            && !in_place_edit
            && !matches!(policy, Sync::Never)
            && self.checkpoint_to_log(&changed)?;
        if std::env::var_os("SUPDB_CKPT_TRACE").is_some() {
            eprintln!("ckpt path: in_place={in_place_edit} logged={logged} changed={}", changed.len());
        }
        // Downstream the two mean the same thing: do not rewrite the index.
        let in_place = in_place_edit || logged;
        if logged {
            // A logged checkpoint made the writes durable and did not publish
            // them: they are in no index section. Saying otherwise is what
            // made `scan` walk a stale index and report one key where the
            // store held sixteen, and the writes were on disk the whole time.
            //
            // The dirty marks go back for the same reason. `checkpoint_inner`
            // takes them before it knows which path it will take, and the next
            // in-place attempt needs them to find what still has not been
            // written into the index.
            self.unpublished
                .store(true, std::sync::atomic::Ordering::Release);
            for (k, _) in &changed {
                let si = self.shard_of(k);
                let mut sh = self.shards[si].lock().unwrap();
                if let Some(idx) = sh.keys.index_of(k) {
                    sh.dirty.push(idx);
                }
            }
        }

        // Only the rewrite needs every key, and only then is the sort worth
        // paying for.
        //
        // Borrowed, not copied. This used to build a `Vec<(Vec<u8>, Extents)>`,
        // which is one heap allocation per key -- a million 16-byte mallocs on
        // a bulk load -- and then sorted a million pointers into scattered
        // allocations, so every comparison was a cache miss on a fresh
        // cacheline. The keys already lie contiguously in each shard's arena,
        // so holding every shard's lock for the gather lets the sort compare
        // slices into a handful of large buffers instead.
        //
        // All shards, then the appender: the same order `read_all` takes them
        // in, and no other path takes two shard locks at once.
        let copies = self.opts.checkpoint_copies_keys;
        let guards: Vec<_> = if in_place || copies {
            Vec::new()
        } else {
            self.shards.iter().map(|s| s.lock().unwrap()).collect()
        };
        let mut owned: Vec<(Vec<u8>, Extents)> = Vec::new();
        let mut all: Vec<(&[u8], &Extents)> = Vec::new();
        if !in_place {
            if copies {
                for sh in &self.shards {
                    let sh = sh.lock().unwrap();
                    owned.extend(sh.keys.iter().map(|(k, e)| (k.to_vec(), e.extents.clone())));
                }
                owned.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                all.extend(owned.iter().map(|(k, e)| (k.as_slice(), e)));
            } else {
                all.reserve(nkeys);
                for sh in &guards {
                    all.extend(sh.keys.iter().map(|(k, e)| (k, &e.extents)));
                }
                let t = std::time::Instant::now();
                if self.opts.parallel_index {
                    sort_keys_parallel(&mut all);
                } else {
                    all.sort_unstable_by(|a, b| a.0.cmp(b.0));
                }
                ckpt_phase("sort", t, Some(all.len()));
            }
        }

        // The fast path: every key already in the published index, and enough
        // slack to hold the records that changed. Then a checkpoint is a few
        // record writes and one aligned store each, and nothing is rewritten.
        //
        // This is what makes checkpoint cost track the change rather than the
        // key count. YCSB's run phase never inserts, so it is entirely this
        // `flat` stays true only if the flat encoder actually produced a
        // section. It declines on inputs it cannot address -- a key over 64KiB,
        // or a record region past 4GiB -- and falling back to the varint
        // encoder is correct there rather than an error.
        let (key_idx, key_reserve, flat) = if in_place {
            (Vec::new(), 0usize, true)
        } else {
            let ap = self.appender.lock().unwrap();
            let prev = ap.last_index.map(|loc| (loc, ap.generation, ap.timestamp));
            let gen = ap.generation + 1;
            let encoded = if self.opts.flat_index {
                let p = prev.map(|(loc, pgen, pts)| {
                    (
                        pgen,
                        pts,
                        loc.off,
                        loc.stored as u64,
                        loc.uncompressed as u64,
                    )
                });
                // Half again, matching the record slack: enough that a store
                // growing steadily keeps publishing in place, and bounded so
                // the reserved directory cannot dwarf what it indexes.
                let slack = if self.opts.index_inserts {
                    (all.len() / 2).max(16)
                } else {
                    0
                };
                let t = std::time::Instant::now();
            let r = flatindex::encode(&all, gen, p, key_hash, slack, self.opts.parallel_index);
            ckpt_phase("encode", t, Some(all.len()));
            r
            } else {
                None
            };
            match encoded {
                Some((v, reserve)) => (v, reserve, true),
                None => {
                    let v = encode_key_index(&all, gen, prev);
                    let n = v.len();
                    (v, n, false)
                }
            }
        };
        let mut ap = self.appender.lock().unwrap();
        if closing {
            // A store nothing will append to again has no use for a log
            // arena, and the arena is 4 MB of reserved zeroes that every
            // download of the file pays for -- measured as the difference
            // between a 580 KB and a 4.8 MB fixed cost per day-index segment.
            // Dropped HERE, before sections are placed, for two reasons: the
            // final sections can then land in the reclaimed space instead of
            // beyond it, and the tail case can be trimmed directly -- the
            // release-into-the-free-list version of this failed silently,
            // because close's trim reads free.coalesced(), which rightly
            // excludes slots still in generation quarantine, and a slot
            // released during close is the youngest slot there is.
            let mut drops: Vec<(u64, u64)> = Vec::new();
            if let Some((o, c, _)) = ap.log.take() {
                drops.push((o, c));
            }
            if let Some((o, c)) = ap.prev_log.take() {
                drops.push((o, c));
            }
            drops.sort_unstable();
            while let Some(&(o, c)) = drops.last() {
                if o + c == ap.off {
                    ap.off = o;
                    let _ = ap.file.set_len(o);
                    drops.pop();
                } else {
                    break;
                }
            }
            let gen_now = ap.generation + 1;
            for (o, c) in drops {
                ap.free.release(o, c as u32, gen_now);
            }
        }
        // Flat and uncompressed, for the same reason as the key index: it is
        // read on every open, and decoding it measured 34% of all
        // instructions in a checkpoint-heavy workload.
        let blk_flat = self.opts.flat_index;
        let blk_idx = if blk_flat {
            flatindex::encode_blocks(&ap.blocks, &ap.chunk_crcs)
        } else {
            encode_block_index(&ap.blocks)
        };
        // Uncompressed, deliberately: a section that is decompressed on open
        // is a section copied into every reader's heap, which is the cost this
        // format exists to remove.
        // An in-place checkpoint leaves the key section exactly where it was.
        let t_write = std::time::Instant::now();
        let key_loc = match (in_place, ap.last_index) {
            (true, Some(loc)) => loc,
            _ if flat => {
                let mut payload = key_idx;
                if self.opts.write_index_slack {
                    payload.resize(key_reserve, 0);
                }
                {
                    let l = write_section_raw(&mut ap, &payload, key_reserve, self.opts.reclaim)?;
                    wled(&WL_KEYSEC, l.stored as usize);
                    l
                }
            }
            _ => {
                let l = write_section(&mut ap, &key_idx, self.opts.reclaim)?;
                wled(&WL_KEYSEC, l.stored as usize);
                l
            }
        };
        let blk_loc = if blk_flat {
            {
                let l = write_section_raw(&mut ap, &blk_idx, blk_idx.len(), self.opts.reclaim)?;
                wled(&WL_BLKSEC, l.stored as usize);
                l
            }
        } else {
            {
                let l = write_section(&mut ap, &blk_idx, self.opts.reclaim)?;
                wled(&WL_BLKSEC, l.stored as usize);
                l
            }
        };
        // Drop entries older than any reader could still be relying on. A
        // reader outside the grace window is already unsafe and is told so, so
        // keeping the record forever only made every checkpoint bigger and
        // slower than the last.
        // keep only what a reader could still be checking against; below the
        // reuse floor the record can never change an answer
        let horizon = ap
            .reuse_floor(self.opts.reclaim)
            .min(ap.generation)
            .saturating_sub(1);
        ap.reuse_log.retain(|(_, _, gen)| *gen >= horizon);
        let reuse = encode_reuse_log(&ap.reuse_log);
        let reuse_loc = write_section(&mut ap, &reuse, self.opts.reclaim)?;
        wled(&WL_REUSE, reuse_loc.stored as usize);
        // Before the superblock, so a power loss cannot leave one pointing at
        // sections that never landed. Skipped when the caller has accepted
        // losing checkpoints on power loss; the CRCs and the alternating
        // superblock slots turn that into "fall back to the previous
        // checkpoint" rather than into corruption.
        //
        // Decided once and used for both flushes: syncing before the
        // superblock but not after would leave a superblock that can land
        // ahead of the sections it points at, which is the ordering the sync
        // exists to provide. Either both or neither.
        let do_sync = match policy {
            Sync::Always => true,
            Sync::Never => false,
            Sync::EveryN(n) => ap.since_sync + 1 >= n.max(1),
            Sync::Interval(d) => ap.last_sync.elapsed() >= d,
        };
        if do_sync {
            let t = std::time::Instant::now();
            ap.file.sync_data()?;
            ckpt_phase("  sync-data(1st)", t, None);
        }

        let gen = ap.generation + 1;
        // A logged point leaves the index as it was; everything else -- full
        // rewrite, in-place edit, a closing fold -- makes the index current.
        if !logged {
            ap.index_gen = gen;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Hand back the sections of checkpoints no reader can still be using.
        //
        // The newest is always kept, and so is anything at or above the reuse
        // floor -- which under Reclaim::Never and Reclaim::OnClose is zero, so
        // this does nothing and history stays complete. Where it does release,
        // `history_from` records how far back the chain is still intact, which
        // is what `open_as_of` already refuses to read past.
        // The key section is only added to the history when it is a new one;
        // an in-place checkpoint reuses the section already recorded, and
        // releasing it would hand a live index to the free list.
        ap.index_history.push((
            gen,
            if in_place { None } else { Some(key_loc) },
            Some(blk_loc),
            Some(reuse_loc),
        ));
        let floor = ap.reuse_floor(self.opts.reclaim);
        let mut i = 0;
        while i < ap.index_history.len() && ap.index_history.len() > 1 {
            if ap.index_history[i].0 >= floor {
                break;
            }
            let live = ap.live_key_off;
            let g = ap.index_history[i].0;
            // Take each section out of the entry as it is released, so nothing
            // can release it a second time. A key section that is still live
            // stays recorded rather than being forgotten here.
            let mut freed = Vec::new();
            {
                let e = &mut ap.index_history[i];
                if e.1.map(|l| l.off) != live {
                    freed.extend(e.1.take());
                }
                freed.extend(e.2.take());
                freed.extend(e.3.take());
            }
            for loc in freed {
                ap.free.release(loc.off, loc.cap, gen);
            }
            ap.history_from = ap.history_from.max(g + 1);
            let empty = {
                let e = &ap.index_history[i];
                e.1.is_none() && e.2.is_none() && e.3.is_none()
            };
            if empty {
                ap.index_history.remove(i);
            } else {
                i += 1;
            }
        }
        let history_from = ap.history_from;

        // A full rewrite makes every logged record redundant -- each one is
        // now in the index this superblock names -- so the old arena is
        // abandoned and a fresh one allocated. Written zeroed, because replay
        // stops at the first zero length and reclaimed space is not
        // necessarily zero.
        //
        // In-place and logged checkpoints keep the arena they have: they did
        // not rewrite the index, so the records still matter.
        let log_arena: Option<(u64, u64)> = if closing {
            // Taken and trimmed at the top of this function, before sections
            // were placed. Nothing to rotate and nothing to record.
            None
        } else if !self.opts.redo_log {
            None
        } else if in_place {
            ap.log.map(|(o, c, _)| (o, c))
        } else {
            // One generation behind: the arena this rewrite supersedes is
            // still named by the superblock being replaced, so what goes back
            // now is the one before it.
            if let Some((older_off, older_cap)) = ap.prev_log.take() {
                ap.free.release(older_off, older_cap as u32, gen);
            }
            ap.prev_log = ap.log.map(|(o, c, _)| (o, c));
            let want = self.opts.log_bytes.max(LOG_HDR);
            // Reserve the arena, write four bytes into it.
            //
            // Zeroing the whole thing is what a self-describing log seems to
            // ask for, and it is not: replay stops at the *first* zero length,
            // so an empty arena needs one zero word at its head and nothing
            // else. Writing 4MB of zeros per rewrite instead cost more than
            // the index rewrite it was there to avoid -- the logged arm wrote
            // 30.8MB against 15.3 for rewriting, and ran at the same speed.
            let loc = write_section_raw(&mut ap, &0u32.to_le_bytes(), want, self.opts.reclaim)?;
            wled(&WL_LOG, 4);
            // `cap` is the space actually reserved, which the allocator rounds
            // up from what was asked for, and it is what has to be recorded --
            // both so replay knows the extent and so the free-list
            // reconstruction in `open` covers all of it. Recording the
            // requested size instead left the rounded-up tail looking free,
            // and the next allocation was handed bytes belonging to a live
            // log: a block checksum mismatch several sessions later. The
            // comment on that loop describes the same symptom by another
            // route, and this is a third.
            let cap = loc.cap as u64;
            // The arena is reserved, not written: only its first word goes
            // down, because replay stops at the first zero length. But the
            // *file* still has to cover it, because `high_water` moved past
            // it and a reader refuses a superblock whose high-water mark is
            // outside the mapping -- it falls back to an older slot, and the
            // older slot decodes as damage. Extending leaves a hole, which
            // reads as the zeroes replay wants and costs no blocks.
            //
            // This is the cost of the earlier optimisation that stopped
            // writing 4MB of zeros per rewrite: that write was doing two jobs
            // and only one of them was visible.
            let end = loc.off + cap;
            if ap.file.metadata().map(|m| m.len()).unwrap_or(0) < end {
                ap.file.set_len(end)?;
            }
            ap.log = Some((loc.off, cap, 0));
            Some((loc.off, cap))
        };

        let sb = Super {
            generation: gen,
            history_from,
            timestamp: now.max(ap.timestamp),
            key_off: key_loc.off,
            key_stored: key_loc.stored as u64,
            key_uncompressed: key_loc.uncompressed as u64,
            blk_off: blk_loc.off,
            blk_stored: blk_loc.stored as u64,
            blk_uncompressed: blk_loc.uncompressed as u64,
            reuse_off: reuse_loc.off,
            reuse_stored: reuse_loc.stored as u64,
            reuse_uncompressed: reuse_loc.uncompressed as u64,
            high_water: ap.off,
            index_gen: ap.index_gen,
            log_off: log_arena.map(|(o, _)| o).unwrap_or(0),
            log_len: log_arena.map(|(_, c)| c).unwrap_or(0),
        };
        // The superblock's high water mark is the append cursor, and the
        // cursor runs ahead of the bytes actually written: a block advances it
        // by its size class while writing only its length, and a section
        // placed in a reclaimed hole does not advance it at all. A reader
        // whose mapping is shorter than the mark refuses to use that
        // superblock, and until now quietly fell back to the older slot --
        // which is how a mutated index came to be paired with a stale block
        // table.
        // Extend only. `set_len` shrinks as readily as it grows, and the
        // cursor can legitimately sit behind the end of the file -- a section
        // written into a reclaimed hole near the end does not move it -- so an
        // unconditional call truncates live data.
        if ap.file.metadata().map(|m| m.len()).unwrap_or(0) < ap.off {
            let _ = ap.file.set_len(ap.off);
        }

        let at = if gen % 2 == 0 { 0 } else { SLOT };
        wled(&WL_SUPER, SUPER_BYTES);
        ap.file.write_all_at(&sb.encode(), at)?;
        // An in-place checkpoint mutates the key section rather than writing a
        // new one, and the two superblock slots exist on the assumption that
        // sections are immutable: the older slot names the *same* key section,
        // which now holds records referring to blocks its own older block
        // table does not list. A reader that falls back to it -- which
        // Reader::open does whenever the newest slot's high water mark is past
        // the end of its mapping -- gets a new index against an old block
        // table, and reports an extent naming a block that does not exist.
        //
        // So the older slot is overwritten too. A crash between the two writes
        // leaves the newer generation in one slot and the older in the other,
        // which is the situation the alternation was already designed for.
        if in_place {
            let other = if at == 0 { SLOT } else { 0 };
            wled(&WL_SUPER, SUPER_BYTES);
            ap.file.write_all_at(&sb.encode(), other)?;
        }
        ckpt_phase(
            "write-sections",
            t_write,
            Some(CKPT_BYTES.swap(0, std::sync::atomic::Ordering::Relaxed) as usize),
        );
        let t_fsync = std::time::Instant::now();
        if do_sync {
            ap.file.sync_data()?;
            ckpt_phase("fsync", t_fsync, None);
            ap.since_sync = 0;
            ap.last_sync = std::time::Instant::now();
            ap.unsynced = false;
        } else {
            ap.since_sync = ap.since_sync.saturating_add(1);
            ap.unsynced = true;
        }
        ap.generation = gen;
        ap.timestamp = sb.timestamp;
        ap.last_index = Some(key_loc);

        // Adopt the section just written, so the next checkpoint can update it
        // in place instead of rewriting it. Best effort: if it will not map or
        // will not parse, the next checkpoint simply takes the slow path.
        if in_place {
            return Ok(gen);
        }
        // The section this replaces is no longer live and may be reclaimed.
        let superseded = ap.live_key_off.take();
        if let Some(off) = superseded {
            if off != key_loc.off {
                // Take it out of the history as it is released. Leaving it
                // there let the pruning loop release the same range a second
                // time once `live_key_off` had moved on.
                if let Some(pos) = ap
                    .index_history
                    .iter()
                    .position(|(_, k, _, _)| k.map(|l| l.off) == Some(off))
                {
                    if let Some(loc) = ap.index_history[pos].1.take() {
                        ap.free.release(loc.off, loc.cap, gen);
                    }
                    let e = &ap.index_history[pos];
                    if e.1.is_none() && e.2.is_none() && e.3.is_none() {
                        ap.index_history.remove(pos);
                    }
                }
            }
        }
        ap.live_index = None;
        // Whatever section the superblock names is the live one, whether or
        // not it is the flat format and whether or not it could be adopted for
        // in-place editing. This used to be set only inside the `flat` branch
        // and only when adoption succeeded, which was survivable while every
        // checkpoint rewrote the key section -- the previous one really was
        // superseded, so releasing it was right.
        //
        // The redo log breaks that assumption: a logged checkpoint publishes
        // no key section, so the previous one stays live across generations.
        // With `live_key_off` left at None the pruning loop saw a key section
        // that did not match "the live one" and released it, and the next
        // block table was placed on top of the index every reader was using.
        // It surfaced as lz4 failing to decompress a section that was never
        // written there.
        ap.live_key_off = Some(key_loc.off);
        if flat {
            if let Ok(map) = unsafe { MmapMut::map_mut(&ap.file) } {
                let (o, l) = (key_loc.off as usize, key_loc.stored as usize);
                ap.live_index = map
                    .get(o..o.saturating_add(l))
                    .and_then(FlatIndex::parse)
                    .map(|meta| (map, meta, key_loc.off, key_loc.stored as u64));
            }
        }
        Ok(gen)
    }

    /// Publish only what changed, or decline.
    ///
    /// Declines -- returning Ok(None) so the caller does a full checkpoint --
    /// when there is no published index yet, when a key is new, or when the
    /// slack cannot hold the updated records. Declining is always safe; the
    /// full path reclaims every superseded record as it rebuilds.
    ///
    /// Note what a reader observes: records are written before the slot that
    /// points at them, and each slot is published by one aligned 8-byte store,
    /// so a reader sees a key's old extents or its new ones. It does *not*
    /// see a snapshot frozen at open -- a reader open across an incremental
    /// checkpoint will observe the new values. For a store whose extents only
    /// ever grow that is more data, not wrong data, and it is the trade that
    /// buys an O(changed) checkpoint.
    /// Append what changed to the redo log and make it durable, without
    /// touching the index.
    ///
    /// This is the split f27 argued for. A checkpoint has always done two jobs
    /// -- make writes durable, and make them findable by a fresh reader -- and
    /// only the second needs the index rewritten. Inserting under
    /// `Sync::Always` ran at 42,079 ops/s against 173,446 for updating the
    /// same keys with the same checkpoint count, because any insertion sends
    /// `checkpoint_in_place` to the full-rewrite path. Here an insertion costs
    /// its own record.
    ///
    /// Returns false when there is no arena or it is full, and the caller then
    /// rewrites the index, which is what bounds replay.
    fn checkpoint_to_log(&self, changed: &[(Vec<u8>, Extents)]) -> Result<bool> {
        use std::os::unix::fs::FileExt;
        if !self.opts.redo_log {
            return Ok(false);
        }
        let mut ap = self.appender.lock().unwrap();
        let Some((off, cap, used)) = ap.log else {
            return Ok(false);
        };
        let gen = ap.generation + 1;
        let mut buf = Vec::new();
        for (k, exts) in changed {
            let Some(rec) = log_encode(k, exts.as_slice(), gen) else {
                return Ok(false);
            };
            buf.extend_from_slice(&rec);
        }
        // The terminator matters: a previous generation may have left records
        // beyond this one, and replay must stop here rather than resurrect
        // them. The arena is zeroed when allocated, so this only has to be
        // written when something follows -- but writing it always is one word
        // and removes the case analysis.
        if used + buf.len() as u64 + LOG_HDR as u64 > cap {
            return Ok(false);
        }
        buf.extend_from_slice(&0u32.to_le_bytes());
        wled(&WL_LOG, buf.len());
        ap.file.write_all_at(&buf, off + used)?;
        // The records are the durability point. Nothing else is updated --
        // not the superblock, not the index -- because the arena describes its
        // own extent: replay stops at the first zero length or bad CRC.
        ap.file.sync_data()?;
        ap.unsynced = false;
        ap.since_sync = 0;
        ap.last_sync = std::time::Instant::now();
        // The terminator is not counted, so the next append overwrites it.
        ap.log = Some((off, cap, used + buf.len() as u64 - 4));
        Ok(true)
    }

    fn checkpoint_in_place(&self, changed: &[(Vec<u8>, Extents)], nkeys: usize) -> Result<bool> {
        use std::sync::atomic::{AtomicU64, Ordering};
        let mut ap = self.appender.lock().unwrap();
        let Some((map, meta, sec_off, sec_len)) = ap.live_index.as_mut() else {
            return Ok(false);
        };
        // A key added since the last rewrite is only a problem for the
        // directory. Records carry half again in slack and the hash runs at
        // half load, so both can take a new key where they lie; the directory
        // is a sorted array and an insertion shifts everything after it, which
        // is not a change a reader may catch half-done.
        //
        // When the section was built with room (`Options::index_inserts`), the
        // new directory is written into the inactive buffer and published with
        // one aligned store, exactly as a record is written into the slack and
        // published with one store of its hash slot. Without that room this
        // still declines, which is what it always did -- and what f27 priced
        // at 4.122x on a workload that only inserts.
        let inserting = nkeys.saturating_sub(meta.len());
        let spare = meta.spare_dir();
        if nkeys < meta.len() {
            return Ok(false);
        }
        if inserting > 0 {
            match spare {
                Some((_, cap)) if nkeys <= cap => {}
                _ => return Ok(false),
            }
        }
        let (off, len) = (*sec_off as usize, *sec_len as usize);
        let Some(sec) = map.get(off..off + len) else {
            return Ok(false);
        };

        // Work out the edits first, so nothing is written until every one of
        // them is known to fit.
        // (record offset in section, hash slot value, hash slot offset,
        //  directory entry offset, record bytes)
        #[allow(clippy::type_complexity)]
        let mut edits: Vec<(usize, u64, usize, usize, u32, Vec<u8>)> = Vec::new();
        let mut probe =
            FlatIndex::parse(sec).ok_or_else(|| corrupt("live index no longer parses"))?;
        probe.set_bump(meta.bump());
        // Keys the index does not have yet, with the rank each belongs at and
        // the record offset it will get. Empty on the update-only path, which
        // is left exactly as it was.
        let mut inserts: Vec<(usize, u32)> = Vec::new();
        for (k, exts) in changed {
            let slice = exts.as_slice();
            let present = meta.slot_of(sec, k, key_hash);
            if present.is_none() && inserting == 0 {
                // Counted as an update but not in the index: something is
                // inconsistent, and guessing is not the answer.
                return Ok(false);
            }
            if let Some(slot_at) = present {
                // The directory is the other way to reach a record, and it has
                // to be republished with the hash or the two disagree: a point
                // lookup would return the new value and a scan the old one.
                // That is what used to happen, silently, for every key an
                // in-place checkpoint touched.
                let Some(dir_at) = meta.dir_slot_of(sec, k) else {
                    return Ok(false);
                };
                match meta.lookup(sec, k, key_hash) {
                    Some(cur) if cur == slice => continue,
                    _ => {}
                }
                let Some(bytes) = FlatIndex::encode_record(k, slice) else {
                    return Ok(false);
                };
                let Some((at, rel)) = probe.reserve(bytes.len()) else {
                    return Ok(false);
                };
                edits.push((
                    off + at,
                    FlatIndex::slot_value(k, rel, key_hash),
                    off + slot_at,
                    off + dir_at,
                    rel,
                    bytes,
                ));
            } else {
                // New key: claim the slot a lookup would stop at, take record
                // space from the slack, and remember where it belongs in key
                // order so the next directory can be spliced rather than
                // rebuilt from the records.
                let Some(slot_at) = meta.slot_for_insert(sec, k, key_hash) else {
                    return Ok(false);
                };
                let Some(bytes) = FlatIndex::encode_record(k, slice) else {
                    return Ok(false);
                };
                let Some((at, rel)) = probe.reserve(bytes.len()) else {
                    return Ok(false);
                };
                inserts.push((meta.rank_for(sec, k), rel));
                edits.push((
                    off + at,
                    FlatIndex::slot_value(k, rel, key_hash),
                    off + slot_at,
                    // No directory entry to overwrite; the splice places it.
                    usize::MAX,
                    rel,
                    bytes,
                ));
            }
        }
        // With insertions, the whole directory moves: the old one is spliced
        // into the inactive buffer with the new entries at their ranks, and
        // published by a single store of `dir_state`. Copying is why the
        // buffer is doubled -- a reader is walking the live one throughout,
        // and a sorted array cannot be grown in place without a window in
        // which it is neither the old order nor the new.
        //
        // A splice, not a rebuild: `rank_for` binary-searches each new key, so
        // this moves 4 bytes per key rather than re-reading every record to
        // re-derive the order.
        let published_dir = if inserts.is_empty() {
            None
        } else {
            let Some((spare_at, cap)) = spare else {
                return Ok(false);
            };
            let old_n = meta.len();
            let Some(old) = meta.dir_entries(sec).map(|d| d.to_vec()) else {
                return Ok(false);
            };
            let mut ins = inserts.clone();
            // By rank, and stably, so two keys landing at the same rank keep
            // the order `changed` had them in.
            ins.sort_by_key(|(rank, _)| *rank);
            if old_n + ins.len() > cap {
                return Ok(false);
            }
            let mut next: Vec<u8> = Vec::with_capacity((old_n + ins.len()) * 4);
            let mut at = 0usize;
            for (rank, rel) in &ins {
                let upto = (*rank).min(old_n);
                if upto * 4 > old.len() || at > upto {
                    return Ok(false);
                }
                next.extend_from_slice(&old[at * 4..upto * 4]);
                next.extend_from_slice(&rel.to_le_bytes());
                at = upto;
            }
            next.extend_from_slice(&old[at * 4..]);
            let want = (old_n + ins.len()) * 4;
            if next.len() != want || spare_at + want > len {
                return Ok(false);
            }
            Some((spare_at, old_n + ins.len(), next))
        };
        if edits.is_empty() {
            return Ok(true);
        }

        // Records first. Nothing points at them yet, so a crash here leaks
        // slack and loses nothing.
        for (at, _, _, _, _, bytes) in &edits {
            map[*at..*at + bytes.len()].copy_from_slice(bytes);
        }
        // The directory entry is published before the hash slot, and both
        // point at a record that is already written. A reader taking either
        // route mid-update gets the old record or the new one, never a
        // mismatch between them, because neither offset is ever partially
        // written: a directory entry is one aligned 4-byte store.
        for (_, _, _, dir_at, rel, _) in &edits {
            if *dir_at == usize::MAX {
                continue;
            }
            map[*dir_at..*dir_at + 4].copy_from_slice(&rel.to_le_bytes());
        }
        // The spliced directory goes down next, into the buffer nobody is
        // reading. Still nothing points at it.
        if let Some((spare_at, _, next)) = &published_dir {
            map[off + spare_at..off + spare_at + next.len()].copy_from_slice(next);
        }
        // Then the slots, one aligned store each. This is the publish.
        for (_, value, slot_at, _, _, _) in &edits {
            debug_assert_eq!(
                slot_at % 8,
                0,
                "a slot must be 8-byte aligned to publish atomically"
            );
            let cell = unsafe { &*(map.as_ptr().add(*slot_at) as *const AtomicU64) };
            cell.store(*value, Ordering::Release);
        }
        // Last, the word that says which directory is live and how many keys
        // it holds. One aligned store, so a reader sees the old directory with
        // the old count or the new with the new -- never a count that outruns
        // the entries behind it.
        if let Some((spare_at, n, _)) = published_dir {
            let at = off + FlatIndex::DIR_STATE_AT;
            debug_assert_eq!(at % 8, 0, "the publish word must be 8-byte aligned");
            let cell = unsafe { &*(map.as_ptr().add(at) as *const AtomicU64) };
            cell.store(
                FlatIndex::dir_state(spare_at, n),
                std::sync::atomic::Ordering::Release,
            );
        }
        // Finally the bump cursor, so a later open knows where the slack
        // starts. After the records, never before.
        let bump = probe.bump();
        let cur = unsafe { &*(map.as_ptr().add(off + FlatIndex::BUMP_AT) as *const AtomicU64) };
        cur.store(bump as u64, Ordering::Release);
        meta.set_bump(bump);

        // Deliberately no msync here. `MmapMut::flush` syncs the whole
        // mapping, which is the whole file, so an incremental checkpoint of a
        // hundred records paid for flushing every page of a multi-gigabyte
        // store -- the cost grew with the store rather than with the change,
        // which is the thing this path exists to stop. The `sync_data` at the
        // end of the checkpoint already forces pages dirtied through the
        // mapping, so durability is unchanged.
        Ok(true)
    }

    pub fn flush(&self) -> Result<()> {
        for s in &self.shards {
            let mut sh = s.lock().unwrap();
            self.seal_shard(&mut sh)?;
            self.flush_builder(&mut sh)?;
        }
        Ok(())
    }

    /// Serialize both indexes and the footer. Keys are written in sorted order
    /// so the index itself compresses (adjacent keys share prefixes) and so a
    /// future ordered scan can binary-search it directly.
    pub fn close(self) -> Result<Stats> {
        self.closing.store(true, std::sync::atomic::Ordering::Release);
        let keys: u64 = {
            let mut n = 0u64;
            for sh in &self.shards {
                n += sh.lock().unwrap().keys.len() as u64;
            }
            n
        };
        // checkpoint first so the store is recoverable even if trimming fails.
        // Always durable, whatever the policy: a clean shutdown that leaves
        // acknowledged writes on the wrong side of a power cut is not a
        // policy, it is a bug. Sync::Never means "durable when I say so", and
        // closing is saying so.
        self.checkpoint_inner(Sync::Always)?;
        {
            let mut ap = self.appender.lock().unwrap();
            let free = ap.free.coalesced();
            let mut end = ap.off;
            for &(off, len) in free.iter().rev() {
                if off + len == end {
                    end = off;
                } else {
                    break;
                }
            }
            if end < ap.off {
                ap.off = end;
                let _ = ap.file.set_len(end);
            }
        }
        // trimming moved the high-water mark, so record the final state
        self.checkpoint_inner(Sync::Always)?;
        // Shards first. `read_all` takes a shard and then the appender, so
        // taking them the other way round here is a deadlock waiting for two
        // threads to want it at once.
        let merges: u64 = self.shards.iter().map(|s| s.lock().unwrap().merges).sum();
        let ap = self.appender.lock().unwrap();
        Ok(Stats {
            blocks: ap.blocks.len() as u64,
            bytes_written: ap.off,
            index_bytes: 0,
            keys,
            merges,
            free_bytes: ap.free.free_bytes(),
            reused: ap.free.reused().0,
            reused_bytes: ap.free.reused().1,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn encode_key_index(
    all: &[(&[u8], &Extents)],
    generation: u64,
    prev: Option<(BlockLoc, u64, u64)>,
) -> Vec<u8> {
    let mut out = Vec::new();
    // chain header: this generation always, then where the previous index
    // lives. The first checkpoint has no predecessor but still has an
    // identity, so the generation is written unconditionally.
    put_uvarint(&mut out, generation);
    match prev {
        Some((loc, pgen, pts)) => {
            put_uvarint(&mut out, pgen);
            put_uvarint(&mut out, pts);
            put_uvarint(&mut out, loc.off);
            put_uvarint(&mut out, loc.stored as u64);
            put_uvarint(&mut out, loc.uncompressed as u64);
        }
        None => {
            for _ in 0..5 {
                put_uvarint(&mut out, 0);
            }
        }
    }
    put_uvarint(&mut out, all.len() as u64);
    for (k, exts) in all {
        put_uvarint(&mut out, k.len() as u64);
        out.extend_from_slice(k);
        let slice = exts.as_slice();
        put_uvarint(&mut out, slice.len() as u64);
        for e in slice {
            put_uvarint(&mut out, e.block as u64);
            put_uvarint(&mut out, e.off as u64);
            put_uvarint(&mut out, e.len as u64);
            put_uvarint(&mut out, e.last as u64);
        }
    }
    out
}

fn encode_reuse_log(log: &[(u64, u32, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    put_uvarint(&mut out, log.len() as u64);
    for (off, cap, gen) in log {
        put_uvarint(&mut out, *off);
        put_uvarint(&mut out, *cap as u64);
        put_uvarint(&mut out, *gen);
    }
    out
}

fn decode_reuse_log(buf: &[u8]) -> Vec<(u64, u32, u64)> {
    let mut p = 0usize;
    let n = get_uvarint(buf, &mut p) as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let off = get_uvarint(buf, &mut p);
        let cap = get_uvarint(buf, &mut p) as u32;
        let gen = get_uvarint(buf, &mut p);
        out.push((off, cap, gen));
    }
    out
}

fn encode_block_index(blocks: &[BlockLoc]) -> Vec<u8> {
    let mut out = Vec::new();
    put_uvarint(&mut out, blocks.len() as u64);
    for b in blocks {
        put_uvarint(&mut out, b.off);
        put_uvarint(&mut out, b.stored as u64);
        put_uvarint(&mut out, b.uncompressed as u64);
        put_uvarint(&mut out, b.cap as u64);
        put_uvarint(&mut out, b.crc as u64);
        out.push((b.solo as u8) | ((b.chunked as u8) << 1));
    }
    out
}

/// Write a section verbatim, so a reader can use it in the mapping.
///
/// `stored == uncompressed` is what marks a section as readable in place, and
/// `read_section` already treats those as equal rather than decompressing, so
/// nothing else has to learn about this.
/// Place a section, reusing freed space when a big enough hole exists.
///
/// Mirrors how blocks are allocated, including the reuse-log entry: releasing
/// a section only makes its space available, and an older index that pointed
/// there stays correct until something is actually written over it. Recording
/// the range and the generation is what lets a later read of an older state
/// fail only if it really touches those bytes.
///
/// `align` is 8 for a section meant to be read in place. A hole that would not
/// land aligned is left alone rather than fudged: the alternative is carving
/// the slot up and handing a different range back to the free list than the
/// one taken from it.
fn place_section(ap: &mut Appender, len: u32, policy: Reclaim, align: u64) -> (u64, u32) {
    let floor = ap.reuse_floor(policy);
    if let Some((off, cap)) = ap.free.take_below(len, floor) {
        if align <= 1 || off % align == 0 {
            ap.reuse_log.push((off, cap, ap.generation));
            return (off, cap);
        }
        // Put it straight back: this section cannot use it.
        ap.free.release(off, cap, ap.generation);
    }
    if align > 1 {
        ap.off = (ap.off + align - 1) & !(align - 1);
    }
    // Rounded to a size class, like blocks, so a section that grows by a byte
    // between checkpoints still fits the hole the last one left. Exact fits
    // were tried and reclaim collapsed: the varint key section changes size
    // slightly every checkpoint and never fitted its predecessor's hole.
    //
    // Rounding means `ap.off` runs past the last byte written, and `ap.off` is
    // the superblock's high water mark, so the file has to actually be that
    // long -- otherwise a reader sees a superblock describing more file than
    // exists and retries until it gives up with "the writer kept moving ahead
    // of the mapping".
    let cap = capacity_for(len);
    let off = ap.off;
    ap.off += cap as u64;
    if ap.file.metadata().map(|m| m.len()).unwrap_or(0) < ap.off {
        let _ = ap.file.set_len(ap.off);
    }
    (off, cap)
}

/// Write a section verbatim, reserving `reserve` bytes for it.
///
/// `reserve` may exceed `payload.len()`. The flat key index ends in a run of
/// slack that only in-place updates ever write into: reserving it without
/// writing it leaves a hole, which reads back as the same zeroes and costs no
/// bandwidth. At 1M keys that slack is half the record region.
///
/// The checksum covers the bytes actually written. A section whose tail is
/// rewritten in place by design has no stable whole-section checksum to take,
/// which is why nothing verifies one for this section.
/// Header on every redo-log record: payload length, then a CRC of it.
const LOG_HDR: usize = 8;

/// One redo-log record: `[u32 len][u32 crc32c][payload]`.
///
/// The payload is exactly what `FlatIndex::encode_record` produces, so the log
/// and the index agree on how a key and its extents are spelled and there is
/// only one encoder to keep correct.
fn log_encode(key: &[u8], exts: &[Ext], gen: u64) -> Option<Vec<u8>> {
    let rec = FlatIndex::encode_record(key, exts)?;
    // The generation is inside the CRC'd payload, so a torn stamp fails the
    // frame the same way torn data does.
    let mut payload = Vec::with_capacity(8 + rec.len());
    payload.extend_from_slice(&gen.to_le_bytes());
    payload.extend_from_slice(&rec);
    let mut out = Vec::with_capacity(LOG_HDR + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&block::crc32(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    Some(out)
}

/// Walk a log arena, stopping at the first record that is not intact.
///
/// The arena is written zeroed, so a zero length is the end. A torn tail from
/// a crash mid-append fails its CRC and ends the walk there, with every record
/// before it still good -- which is the property that makes this a log rather
/// than a file that has to be rewritten to be extended.
fn log_replay(arena: &[u8]) -> (Vec<(u64, Vec<u8>, Extents)>, u64) {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + LOG_HDR <= arena.len() {
        let len = u32::from_le_bytes(arena[at..at + 4].try_into().unwrap()) as usize;
        if len == 0 || at + LOG_HDR + len > arena.len() {
            break;
        }
        let want = u32::from_le_bytes(arena[at + 4..at + 8].try_into().unwrap());
        let payload = &arena[at + LOG_HDR..at + LOG_HDR + len];
        if block::crc32(payload) != want {
            break;
        }
        let parsed = payload.get(0..8).and_then(|g| {
            let gen = u64::from_le_bytes(g.try_into().ok()?);
            let (k, exts) = FlatIndex::decode_record(&payload[8..])?;
            let mut e = Extents::None;
            for x in exts {
                e.push(x);
            }
            Some((gen, k, e))
        });
        match parsed {
            Some(r) => out.push(r),
            None => break,
        }
        at += LOG_HDR + len;
    }
    (out, at as u64)
}

/// Where a checkpoint's time goes, accumulated in nanoseconds.
///
/// Recorded rather than printed, because the answer was not what two profiles
/// said it was. cachegrind put 62x LMDB's last-level misses per key on an
/// ordered load, which reads like a scattered write path; `cg_annotate` put 1%
/// of them in the hash probe. Timing the phases said the checkpoint dominates.
/// Timing inside the checkpoint said the sort and the encode are a third of
/// it -- and then a control said a bare 57MB write costs 0.087s on this
/// machine against the 0.406s the phase was taking, which is what finally
/// pointed at the `sync_data` sitting in the middle of it.
///
/// `SUPDB_CKPT_PHASES=1` also prints them, which is how they were found.
#[derive(Default)]
pub struct Phases {
    pub sort_ns: u64,
    pub encode_ns: u64,
    pub crc_ns: u64,
    pub pwrite_ns: u64,
    pub fsync_ns: u64,
    pub bytes: u64,
}

static P_SORT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static P_ENCODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static P_CRC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static P_PWRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static P_FSYNC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CKPT_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Explicit bytes handed to write_all_at, attributed by file region.
///
/// This exists to convict a residual. f29 measured 523.3 MB reaching the
/// device for 23 MB of data with the value log on, and nobody owned the
/// difference; the design panel's first ruling was that no durability claim
/// moves until that number decomposes term by term. The ledger counts what
/// the engine wrote on purpose; the gap between its sum and /proc/self/io
/// write_bytes is what reached the device by other routes -- mmap-dirtied
/// index pages flushed under fsync, and filesystem metadata -- which is
/// itself one of the terms under suspicion.
#[derive(Default, Debug, Clone, Copy)]
pub struct WriteLedger {
    pub log: u64,
    pub blocks: u64,
    pub key_section: u64,
    pub block_table: u64,
    pub reuse: u64,
    pub superblock: u64,
    pub defrag: u64,
}

static WL_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WL_BLOCKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WL_KEYSEC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WL_BLKSEC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WL_REUSE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WL_SUPER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WL_DEFRAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[inline]
fn wled(cell: &std::sync::atomic::AtomicU64, n: usize) {
    cell.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
}

/// Read the counters and zero them, so a caller measures one span.
pub fn take_write_ledger() -> WriteLedger {
    use std::sync::atomic::Ordering::Relaxed;
    WriteLedger {
        log: WL_LOG.swap(0, Relaxed),
        blocks: WL_BLOCKS.swap(0, Relaxed),
        key_section: WL_KEYSEC.swap(0, Relaxed),
        block_table: WL_BLKSEC.swap(0, Relaxed),
        reuse: WL_REUSE.swap(0, Relaxed),
        superblock: WL_SUPER.swap(0, Relaxed),
        defrag: WL_DEFRAG.swap(0, Relaxed),
    }
}

impl WriteLedger {
    pub fn total(&self) -> u64 {
        self.log + self.blocks + self.key_section + self.block_table + self.reuse
            + self.superblock + self.defrag
    }
}

/// Read the counters and zero them, so a caller measures one span.
pub fn take_phases() -> Phases {
    use std::sync::atomic::Ordering::Relaxed;
    Phases {
        sort_ns: P_SORT.swap(0, Relaxed),
        encode_ns: P_ENCODE.swap(0, Relaxed),
        crc_ns: P_CRC.swap(0, Relaxed),
        pwrite_ns: P_PWRITE.swap(0, Relaxed),
        fsync_ns: P_FSYNC.swap(0, Relaxed),
        bytes: CKPT_BYTES.swap(0, Relaxed),
    }
}

/// Sub-phases of `flatindex::encode`, printed under the same env var.
pub(crate) fn enc_phase(what: &str, t: std::time::Instant) {
    if std::env::var_os("SUPDB_CKPT_PHASES").is_some() {
        eprintln!("      enc:{what} {:.4}s", t.elapsed().as_secs_f64());
    }
}

#[inline]
fn ckpt_phase(what: &str, t: std::time::Instant, extra: Option<usize>) {
    use std::sync::atomic::Ordering::Relaxed;
    let d = t.elapsed();
    let ns = d.as_nanos() as u64;
    match what {
        "sort" => P_SORT.fetch_add(ns, Relaxed),
        "encode" => P_ENCODE.fetch_add(ns, Relaxed),
        "  crc" => P_CRC.fetch_add(ns, Relaxed),
        "  pwrite" => P_PWRITE.fetch_add(ns, Relaxed),
        "  sync-data(1st)" | "fsync" => P_FSYNC.fetch_add(ns, Relaxed),
        _ => 0,
    };
    if std::env::var_os("SUPDB_CKPT_PHASES").is_none() {
        return;
    }
    let secs = d.as_secs_f64();
    match extra {
        Some(n) => eprintln!("  {what} {secs:.4}s ({n})"),
        None => eprintln!("  {what} {secs:.4}s"),
    }
}

/// Sort the gathered keys across threads, then merge the runs.
///
/// A comparison here is a `memcmp` of two keys, so the sort is comparison
/// bound rather than move bound, which is the shape that parallelises well.
/// The merge is sequential and linear, and with four runs it is cheap next to
/// the sort it replaces.
///
/// Scoped threads because the items borrow from shard guards the caller is
/// holding: nothing outlives this call, so nothing has to be copied to be
/// sent.
fn sort_keys_parallel(all: &mut Vec<(&[u8], &Extents)>) {
    let n = all.len();
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
        .min(8);
    // Below this the threads cost more than the sort. Measured rather than
    // guessed would be better; this is a floor to keep small checkpoints off
    // the thread path entirely, and small checkpoints are the common case.
    if threads < 2 || n < 64 * 1024 {
        all.sort_unstable_by(|a, b| a.0.cmp(b.0));
        return;
    }
    let chunk = n.div_ceil(threads);
    {
        let mut rest: &mut [(&[u8], &Extents)] = all.as_mut_slice();
        let mut parts: Vec<&mut [(&[u8], &Extents)]> = Vec::with_capacity(threads);
        while !rest.is_empty() {
            let take = chunk.min(rest.len());
            let (a, b) = rest.split_at_mut(take);
            parts.push(a);
            rest = b;
        }
        std::thread::scope(|s| {
            for p in parts {
                s.spawn(move || p.sort_unstable_by(|a, b| a.0.cmp(b.0)));
            }
        });
    }
    // Merge the sorted runs. Repeated pairwise merging rather than a heap:
    // with at most eight runs the heap's bookkeeping costs more than the
    // extra passes, and the passes are sequential memory.
    let mut bounds: Vec<(usize, usize)> = (0..n)
        .step_by(chunk)
        .map(|a| (a, (a + chunk).min(n)))
        .collect();
    let mut src: Vec<(&[u8], &Extents)> = std::mem::take(all);
    let mut dst: Vec<(&[u8], &Extents)> = Vec::with_capacity(n);
    while bounds.len() > 1 {
        dst.clear();
        let mut next = Vec::with_capacity(bounds.len().div_ceil(2));
        let mut i = 0;
        while i < bounds.len() {
            let (a0, a1) = bounds[i];
            if i + 1 == bounds.len() {
                let start = dst.len();
                dst.extend_from_slice(&src[a0..a1]);
                next.push((start, dst.len()));
                break;
            }
            let (b0, b1) = bounds[i + 1];
            let start = dst.len();
            let (mut x, mut y) = (a0, b0);
            while x < a1 && y < b1 {
                if src[x].0 <= src[y].0 {
                    dst.push(src[x]);
                    x += 1;
                } else {
                    dst.push(src[y]);
                    y += 1;
                }
            }
            dst.extend_from_slice(&src[x..a1]);
            dst.extend_from_slice(&src[y..b1]);
            next.push((start, dst.len()));
            i += 2;
        }
        std::mem::swap(&mut src, &mut dst);
        bounds = next;
    }
    *all = src;
}

fn write_section_raw(
    ap: &mut Appender,
    payload: &[u8],
    reserve: usize,
    policy: Reclaim,
) -> Result<BlockLoc> {
    use std::os::unix::fs::FileExt;
    // Aligned in the *file*, not just within itself.
    //
    // A mapped index hands back `&[Ext]` borrowed from the mapping, which
    // requires those extents to be aligned at their absolute address. Laying
    // records out 4-aligned relative to the section start is not enough: the
    // section begins wherever the appender happened to be, so the same layout
    // was aligned or not depending on how many bytes preceded it. That
    // presented as every lookup and every scan returning nothing while
    // `keys()` stayed correct -- the header parsed, the records did not --
    // and it tracked checkpoint count rather than key count, which is what
    // made it look like a scale bug.
    let len = reserve.max(payload.len()) as u32;
    let (off, cap) = place_section(ap, len, policy, 8);
    let loc = BlockLoc {
        off,
        stored: len,
        uncompressed: len,
        cap,
        chunked: false,
        solo: false,
        // Sections are not read through the block path, so per-chunk
        // checksums would never be consulted.
        chunk_crc: false,
        crc: {
            let t = std::time::Instant::now();
            let c = if block::checksums_on() {
                block::crc32(payload)
            } else {
                0
            };
            ckpt_phase("  crc", t, Some(payload.len()));
            c
        },
    };
    let t_w = std::time::Instant::now();
    ap.file.write_all_at(payload, off)?;
    ckpt_phase("  pwrite", t_w, Some(payload.len()));
    CKPT_BYTES.fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
    Ok(loc)
}

fn write_section(ap: &mut Appender, payload: &[u8], policy: Reclaim) -> Result<BlockLoc> {
    let stored = block::compress(payload);
    let bytes: &[u8] = stored.as_deref().unwrap_or(payload);
    use std::os::unix::fs::FileExt;
    let (off, cap) = place_section(ap, bytes.len() as u32, policy, 1);
    let loc = BlockLoc {
        off,
        stored: bytes.len() as u32,
        uncompressed: payload.len() as u32,
        cap,
        chunked: false,
        solo: false,
        chunk_crc: false,
        crc: if block::checksums_on() {
            block::crc32(bytes)
        } else {
            0
        },
    };
    ap.file.write_all_at(bytes, off)?;
    Ok(loc)
}

// ---------------------------------------------------------------- reading --

enum BlockRef<'a> {
    Mapped(&'a [u8]),
    Owned(Arc<Vec<u8>>),
}

impl BlockRef<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            BlockRef::Mapped(s) => s,
            BlockRef::Owned(v) => v.as_slice(),
        }
    }
}

thread_local! {
    static SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

/// The error a decoder returns when the bytes it was given are not the bytes
/// that were written.
fn corrupt(what: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "record framing is not valid ({what}); the stored bytes differ from what was \
             written, and nothing outside the superblock is checksummed so this is the first \
             point at which that can be noticed"
        ),
    )
}

/// Hand each record in an extent to the visitor; returns bytes emitted.
///
/// Every record length is checked against the bytes actually remaining. This
/// used to slice `extent[p..p + n]` on a length read straight out of the
/// buffer, so a damaged extent panicked the calling process instead of
/// returning an error.
fn emit<F: FnMut(&[u8])>(extent: &[u8], f: &mut F) -> Result<u64> {
    let mut p = 0usize;
    let mut total = 0u64;
    while p < extent.len() {
        let n = get_uvarint(extent, &mut p) as usize;
        let end = p
            .checked_add(n)
            .ok_or_else(|| corrupt("record length overflows"))?;
        if end > extent.len() {
            return Err(corrupt("record runs past the end of its extent"));
        }
        f(&extent[p..end]);
        total += n as u64;
        p = end;
    }
    Ok(total)
}

pub struct Reader {
    mmap: Mmap,
    /// Redo-log records this reader replayed, merged into the published order.
    /// Empty for a store that has no outstanding log, which is every store
    /// written without `Options::redo_log` and every one whose last checkpoint
    /// rewrote the index.
    overlay: Overlay,
    /// Where this reader's keys live: decoded onto the heap, or left in the
    /// mapping and addressed where they lie.
    idx: Idx,
    /// One bit per block: whether this reader has already verified its
    /// checksum.
    ///
    /// An uncompressed block is read straight out of the mapping, and the
    /// checksum covers the whole block, so verifying on every read costs
    /// O(block size) per value returned -- 64 KiB of CRC to hand back 100
    /// bytes. It measured 7985 ns per entry on an ordered scan against 26 with
    /// checking off: a 307x penalty, and the reason compression looked like it
    /// made reads *faster*. The compressed path never showed it because
    /// chunking makes it verify a kilobyte at a time.
    ///
    /// A block a reader can see cannot be rewritten underneath it -- that is
    /// what the generation claim buys -- so once verified it stays verified
    /// for this reader's lifetime. Atomic because a `Reader` is shared by
    /// reference; the race is benign, two threads may verify the same block
    /// once each.
    verified: Vec<std::sync::atomic::AtomicU64>,
    /// Where this reader's block table lives: decoded onto the heap, or left
    /// in the mapping like the key index.
    blocks_src: BlocksSrc,
    opts: ReadOptions,
    /// What `Readahead::Auto` resolved to, so a caller can see the choice
    /// rather than infer it.
    advice: Readahead,
    cache: BlockCache,
    /// Slot held in the reader table, released when this reader is dropped.
    slot: Option<usize>,
    /// The reserved page, mapped writable so the slot can be published.
    table: Option<MmapMut>,
    generation: u64,
    timestamp: u64,
    history_from: u64,
    /// Byte ranges overwritten after this reader's generation, sorted by
    /// offset. Empty for a reader of the current state, which is why a normal
    /// read pays nothing for this.
    overwritten: Vec<(u64, u64)>,
    /// (generation, timestamp, offset, stored, uncompressed) of the previous
    /// checkpoint's index.
    prev: Option<(u64, u64, u64, u64, u64)>,
}

/// The two block-table arms.
///
/// `Owned` is the varint format decoded into a `Vec` at open -- five varints
/// and a flag byte per block, which callgrind measured at 34% of all
/// instructions in a checkpoint-heavy workload, because block count grows with
/// overwrite churn and every open pays it again. `Mapped` reads an entry where
/// it lies.
enum BlocksSrc {
    Owned(Vec<BlockLoc>),
    Mapped {
        meta: flatindex::MappedBlocks,
        off: usize,
        len: usize,
    },
}

/// The two index arms.
///
/// `Heap` is the shipped one: every key copied into its own `Vec`, every
/// extent decoded, then a hash built over the result. `Flat` is the same
/// information addressed where the mapping already holds it.
///
/// Both answer the same four questions, and every caller in `Reader` goes
/// through them, so the arms cannot drift apart in behaviour -- only in cost.
enum Idx {
    Heap {
        entries: Vec<(Vec<u8>, Extents)>,
        hash: Vec<(u8, u32)>,
        mask: usize,
    },
    Flat {
        meta: FlatIndex,
        /// Byte range of the section inside the mapping.
        off: usize,
        len: usize,
    },
}

/// Redo-log records a reader has replayed, spliced into the published order.
///
/// This is what lets `redo_log` be the engine rather than an option. A logged
/// checkpoint makes writes durable without publishing them, so until this
/// existed a `Reader` opened before the next full rewrite could not see them
/// -- F29.2 measured that as 0 of 500 durable keys visible, and it is the only
/// reason the log was off by default.
///
/// The published index is untouched: it is mapped, shared between processes
/// and read in place, and none of that survives being edited per reader. So
/// the records are held beside it and merged on the way out. The log is
/// bounded by `Options::log_bytes`, so the overlay is bounded too -- this is
/// not a second copy of the index.
/// Which side of the merge a rank came from.
enum Where {
    Published(usize),
    Overlay(usize),
}

#[derive(Default)]
struct Overlay {
    /// Sorted by key, one entry per key, last record winning.
    entries: Vec<(Vec<u8>, Extents)>,
    /// Each insertion as (published rank it belongs before, merged rank it
    /// occupies, index into `entries`), sorted. Overlay entries whose key
    /// already exists in the published index are not here: they replace
    /// extents in place and move no rank.
    ///
    /// The merged rank is stored rather than derived. The k-th insertion sits
    /// at `at + k`, and computing k inside a `partition_point` predicate as
    /// "how many sorted before `at`" gives two insertions at the *same*
    /// published rank the same answer, so they resolve to one merged rank and
    /// the other is unreachable. That presented as "published index is shorter
    /// than it says" on reopen.
    inserts: Vec<(usize, usize, usize)>,
}

impl Overlay {
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Extra ranks this overlay adds to the published order.
    fn extra(&self) -> usize {
        self.inserts.len()
    }

    fn find(&self, key: &[u8]) -> Option<&Extents> {
        self.entries
            .binary_search_by(|(k, _)| k.as_slice().cmp(key))
            .ok()
            .map(|i| &self.entries[i].1)
    }

    /// How many insertions sort before published rank `r`.
    fn before(&self, r: usize) -> usize {
        self.inserts.partition_point(|(at, _, _)| *at < r)
    }

    /// Resolve a merged rank to either a published rank or an overlay entry.
    ///
    /// The k-th insertion, which belongs before published rank `at`, occupies
    /// merged rank `at + k`: everything before it is `at` published entries
    /// plus the `k` insertions that sorted earlier.
    fn resolve(&self, merged: usize) -> Where {
        let k = self.inserts.partition_point(|&(_, at_merged, _)| at_merged <= merged);
        if k > 0 {
            let (_, at_merged, which) = self.inserts[k - 1];
            if at_merged == merged {
                return Where::Overlay(which);
            }
        }
        Where::Published(merged - k)
    }
}

impl Idx {
    fn len(&self) -> usize {
        match self {
            Idx::Heap { entries, .. } => entries.len(),
            Idx::Flat { meta, .. } => meta.len(),
        }
    }

    #[inline]
    fn section<'a>(&self, mmap: &'a Mmap) -> &'a [u8] {
        match self {
            Idx::Flat { off, len, .. } => &mmap[*off..*off + *len],
            Idx::Heap { .. } => &[],
        }
    }

    #[inline]
    fn lookup<'a>(&'a self, mmap: &'a Mmap, key: &[u8]) -> Option<&'a [Ext]> {
        match self {
            Idx::Heap {
                entries,
                hash,
                mask,
            } => {
                let h = key_hash(key);
                let tag = ((h >> 56) as u8) | 1;
                let mut slot = (h as usize) & mask;
                loop {
                    let (t, i) = hash[slot];
                    if i == u32::MAX {
                        return None;
                    }
                    if t == tag {
                        let e = &entries[i as usize];
                        if e.0.as_slice() == key {
                            return Some(e.1.as_slice());
                        }
                    }
                    slot = (slot + 1) & mask;
                }
            }
            Idx::Flat { meta, .. } => meta.lookup(self.section(mmap), key, key_hash),
        }
    }

    fn seek(&self, mmap: &Mmap, key: &[u8], fence: bool) -> usize {
        match self {
            Idx::Heap { entries, .. } => {
                match entries.binary_search_by(|(k, _)| k.as_slice().cmp(key)) {
                    Ok(i) | Err(i) => i,
                }
            }
            Idx::Flat { meta, .. } => meta.seek_with(self.section(mmap), key, fence),
        }
    }

    #[inline]
    fn at<'a>(&'a self, mmap: &'a Mmap, rank: usize) -> Option<(&'a [u8], &'a [Ext])> {
        match self {
            Idx::Heap { entries, .. } => {
                entries.get(rank).map(|(k, e)| (k.as_slice(), e.as_slice()))
            }
            Idx::Flat { meta, .. } => meta.at(self.section(mmap), rank),
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let (Some(slot), Some(table)) = (self.slot, self.table.as_ref()) {
            let t = unsafe { readers::slots(table.as_ptr()) };
            readers::release(t, slot);
        }
    }
}

// Moved to `flatindex`, unchanged, so that `blob.rs` -- which is the same
// reader over a byte source that is not a mapping, and which must compile
// without this file -- cannot disagree with the writer about it. Delegating
// rather than duplicating is the point.
//
// This is the only line of the engine the byte-source work touched, and it
// costs nothing: with LTO on, the `.text` of `target/release/supbench` is
// byte-identical before and after (sha256 2a4a12ce...). CLAUDE.md requires a
// change to be measured with both arms interleaved in one process, and there
// is nothing to interleave here -- the two arms are the same machine code, so
// the compiler's output is the measurement.
#[inline]
fn key_hash(key: &[u8]) -> u64 {
    flatindex::key_hash(key)
}

impl Reader {
    /// Open at the last complete checkpoint.
    ///
    /// Both slots are read and the valid one with the higher generation wins,
    /// so a crash during a checkpoint falls back to the previous state rather
    /// than failing to open. Anything written after that checkpoint is
    /// unreachable: leaked space, never corruption.
    /// Open the store as it stood at a given checkpoint generation.
    ///
    /// Walks the chain of index sections backwards. Only meaningful under
    /// Retain::Snapshots -- with Reclaim the older index is still there but the
    /// blocks it points at may have been handed to the free list and rewritten.
    pub fn open_as_of(path: &Path, generation: u64) -> Result<Reader> {
        let mut r = Reader::open(path)?;
        if generation < r.history_from {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "cannot read as of generation {generation}: history is intact only from \
                     generation {} onwards, because a mutation under Retain::Reclaim released \
                     space that earlier states referred to. Use Retain::Snapshots to keep it.",
                    r.history_from
                ),
            ));
        }
        while r.generation > generation {
            let Some((_pgen, pts, off, stored, uncompressed)) = r.prev else {
                break;
            };
            r = Reader::open_at(path, off, stored as usize, uncompressed as usize, pts)?;
        }
        r.load_overwritten(path)?;
        Ok(r)
    }

    /// Open the store as it stood at a wall-clock time (milliseconds since the
    /// epoch): the newest checkpoint taken at or before it.
    ///
    /// The generation is the exact handle; this is the convenient one. If the
    /// requested time predates every checkpoint, the oldest reachable one is
    /// returned rather than an error, since that is the earliest state that
    /// exists.
    pub fn open_as_of_time(path: &Path, millis: u64) -> Result<Reader> {
        let mut r = Reader::open(path)?;
        let floor = r.history_from;
        while r.timestamp > millis {
            let Some((pgen, pts, off, stored, uncompressed)) = r.prev else {
                break;
            };
            if pgen < floor {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "cannot read as of time {millis}: it falls before generation {floor}, \
                         the oldest state whose history survived a mutation under \
                         Retain::Reclaim. Use Retain::Snapshots to keep it."
                    ),
                ));
            }
            r = Reader::open_at(path, off, stored as usize, uncompressed as usize, pts)?;
        }
        r.load_overwritten(path)?;
        Ok(r)
    }

    /// Collect the byte ranges rewritten after this reader's generation.
    /// How many byte ranges are known to have been overwritten since this
    /// reader's generation. Zero means every value it can see is intact.
    pub fn overwritten_ranges(&self) -> usize {
        self.overwritten.len()
    }

    fn load_overwritten(&mut self, path: &Path) -> Result<()> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let a = Super::decode(&mmap[0..SUPER_BYTES]);
        let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + SUPER_BYTES]);
        let sb = match (a, b) {
            (Some(x), Some(y)) => {
                if x.generation >= y.generation {
                    x
                } else {
                    y
                }
            }
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => return Ok(()),
        };
        if sb.reuse_off == 0 {
            return Ok(());
        }
        let raw = read_section(
            &mmap,
            sb.reuse_off,
            sb.reuse_stored as usize,
            sb.reuse_uncompressed as usize,
        )?;
        let mut v: Vec<(u64, u64)> = decode_reuse_log(&raw)
            .into_iter()
            // a slot reused during generation N was overwritten after
            // generation N was checkpointed, so it is unsafe for readers at N
            // and earlier
            .filter(|(_, _, gen)| *gen >= self.generation)
            .map(|(off, cap, _)| (off, cap as u64))
            .collect();
        v.sort_unstable();
        self.overwritten = v;
        Ok(())
    }

    /// This reader's checkpoint identity: (generation, milliseconds).
    pub fn version(&self) -> (u64, u64) {
        (self.generation, self.timestamp)
    }

    /// True if this byte range was written over after this reader's generation.
    fn is_overwritten(&self, off: u64, len: u64) -> bool {
        if self.overwritten.is_empty() {
            return false;
        }
        let i = self.overwritten.partition_point(|(o, _)| *o < off + len);
        self.overwritten[..i].iter().any(|(o, l)| o + l > off)
    }

    /// Resolve an extent's block, checking the id against the block table.
    ///
    /// The decoded index validated every block id while it was decoding, so
    /// every read path could index `self.blocks` directly and did. A mapped
    /// index does no such pass -- that pass is exactly the O(key count) open
    /// being removed -- so the check has to happen here instead, and it did
    /// not: `scan` indexed the block table with a number straight out of a
    /// damaged file. The corruption suite found it at 4% of trials, as a
    /// panic in the calling process rather than an error return, which for an
    /// embedded library means somebody else's application aborting.
    #[inline]
    fn loc_of(&self, block: u32) -> Result<BlockLoc> {
        let got = match &self.blocks_src {
            BlocksSrc::Owned(v) => v.get(block as usize).copied(),
            BlocksSrc::Mapped { meta, off, len } => self
                .mmap
                .get(*off..off.saturating_add(*len))
                .and_then(|sec| meta.get(sec, block as usize)),
        };
        got.ok_or_else(|| {
            corrupt(&format!(
                "extent names block {block} but the table has {}",
                self.nblocks()
            ))
        })
    }

    #[inline]
    fn nblocks(&self) -> usize {
        match &self.blocks_src {
            BlocksSrc::Owned(v) => v.len(),
            BlocksSrc::Mapped { meta, .. } => meta.len(),
        }
    }

    /// The stored bytes of a block, checked against the mapping.
    #[inline]
    fn raw_of(&self, loc: BlockLoc) -> Result<&[u8]> {
        let end = (loc.off as usize).checked_add(loc.stored as usize);
        match end {
            Some(end) => self.mmap.get(loc.off as usize..end).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "block spans {}..{end} but the mapping is {} bytes",
                        loc.off,
                        self.mmap.len()
                    ),
                )
            }),
            None => Err(corrupt("block offset overflows")),
        }
    }

    fn check_extent(&self, e: Ext) -> Result<()> {
        let loc = self.loc_of(e.block)?;
        self.check_extent_loc(loc)
    }

    /// The same check, for a caller that already resolved the block.
    ///
    /// A scan resolved every extent's block twice -- once here and once for
    /// the bytes -- which is one bounds-checked read of the block table per
    /// entry that nothing needed.
    fn check_extent_loc(&self, loc: BlockLoc) -> Result<()> {
        if self.overwritten.is_empty() {
            return Ok(());
        }
        if self.is_overwritten(loc.off, loc.stored as u64) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "this value was stored at bytes {}..{} of the file, which have since been \
                     written over by a later block: the store is running under Retain::Reclaim, \
                     which reuses the space a superseded value occupied. Other keys in this \
                     snapshot are still readable. Use Retain::Snapshots to keep history intact.",
                    loc.off,
                    loc.off + loc.stored as u64
                ),
            ));
        }
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Reader> {
        Reader::open_with(path, ReadOptions::default())
    }

    /// Open with the block table forced to one representation or the other.
    ///
    /// The mapped table exists so that readers of the same store share it
    /// rather than each decoding a private copy. Whether that costs anything
    /// on the read path is a question only an interleaved measurement can
    /// answer, and it cannot be interleaved without a runtime choice --
    /// `f14-blocktable` is that measurement.
    pub fn open_with(path: &Path, opts: ReadOptions) -> Result<Reader> {
        // The superblock lives in the first page and so is always visible, but
        // it is written last and points at data that may lie beyond the length
        // this mapping captured -- a writer checkpointing between the open and
        // the map is enough. Remap until the mapping covers what the
        // superblock describes; each attempt sees a state at least as new, so
        // this converges rather than spinning.
        for attempt in 0.. {
            let file = File::open(path)?;
            let mmap = unsafe { Mmap::map(&file)? };
            if mmap.len() < SUPER as usize {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "file too short to hold a superblock",
                ));
            }
            let a = Super::decode(&mmap[0..SUPER_BYTES]);
            let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + SUPER_BYTES]);
            let newest = match (a, b) {
                (Some(x), Some(y)) => Some(if x.generation >= y.generation { x } else { y }),
                (Some(x), None) => Some(x),
                (None, Some(y)) => Some(y),
                (None, None) => None,
            };
            match newest {
                Some(sb) if {
                    if std::env::var_os("SUPDB_ARENA_TRACE").is_some() {
                        eprintln!("reader: hw={} maplen={} log_off={} log_len={}", sb.high_water, mmap.len(), sb.log_off, sb.log_len);
                    }
                    sb.high_water as usize <= mmap.len()
                } => {
                    // Claim the slot before parsing anything. Registration has
                    // to precede the slow part, or the writer can reuse the
                    // space this reader is about to walk while it is still
                    // decoding the index.
                    let claim = Self::claim(path, sb.generation);
                    let mut r = Self::open_mapped(mmap, sb, opts)?;
                    if let Some((slot, table)) = claim {
                        r.slot = Some(slot);
                        r.table = Some(table);
                    }
                    return Ok(r);
                }
                // an older slot may still be fully covered by this mapping
                Some(_) => {
                    if let Some(older) = match (a, b) {
                        (Some(x), Some(y)) => Some(if x.generation < y.generation { x } else { y }),
                        _ => None,
                    } {
                        if older.high_water as usize <= mmap.len() {
                            return Self::open_mapped(mmap, older, opts);
                        }
                    }
                }
                None => {
                    // An intact file of the other byte order reads as damage
                    // unless it is named, and "no valid supdb checkpoint" on a
                    // healthy file is a diagnosis that would cost somebody a
                    // day.
                    let foreign = mmap.len() >= SUPER as usize
                        && (Super::wrong_endian(&mmap[0..SUPER_BYTES])
                            || Super::wrong_endian(
                                &mmap[SLOT as usize..SLOT as usize + SUPER_BYTES],
                            ));
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        if foreign {
                            format!(
                                "this store was written on a {} machine and this is a {} one. \
                             Supdb's index is addressed in place -- `&[Ext]` is borrowed \
                             straight out of the mapping -- so a file is only self-consistent \
                             on the byte order that wrote it, and it is refused rather than \
                             misread",
                            if cfg!(target_endian = "little") {
                                "big-endian"
                            } else {
                                "little-endian"
                            },
                            if cfg!(target_endian = "little") {
                                "little-endian"
                            } else {
                                "big-endian"
                            }
                            )
                        } else {
                            "no valid supdb checkpoint".to_string()
                        },
                    ));
                }
            }
            if attempt >= 16 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "the writer kept moving ahead of the mapping",
                ));
            }
            std::thread::yield_now();
        }
        unreachable!()
    }

    fn open_mapped(mmap: Mmap, sb: Super, opts: ReadOptions) -> Result<Reader> {
        // Before anything is read out of it, so the first fault already knows.
        // Resolved against the mapping's own length, which is the file's.
        let advice = opts.readahead.resolve(mmap.len() as u64);
        advice.apply(&mmap);
        // Same treatment as the key index: used where it lies when it is the
        // flat format, decoded otherwise.
        let blocks_src = {
            let off = sb.blk_off as usize;
            let len = sb.blk_stored as usize;
            // A flat section is written uncompressed so it can be read where
            // it lies; anything compressed is the varint format by
            // construction. Which format it is decides how it is *decoded*;
            // `mapped_blocks` decides only whether the decode is skipped.
            let flat = sb.blk_stored == sb.blk_uncompressed
                && mmap
                    .get(off..off.saturating_add(len))
                    .is_some_and(flatindex::is_block_section);
            match flat {
                true => {
                    let sec = mmap
                        .get(off..off.saturating_add(len))
                        .ok_or_else(|| corrupt("block table runs past the mapping"))?;
                    if opts.mapped_blocks {
                        match flatindex::MappedBlocks::parse(sec) {
                            Some(meta) => BlocksSrc::Mapped { meta, off, len },
                            // Unaligned in this mapping, or an entry size this
                            // build disagrees with. Copying it out still reads
                            // it correctly; guessing the other format does not.
                            None => BlocksSrc::Owned(
                                flatindex::decode_blocks(sec)
                                    .ok_or_else(|| corrupt("block table is not readable"))?,
                            ),
                        }
                    } else {
                        BlocksSrc::Owned(
                            flatindex::decode_blocks(sec)
                                .ok_or_else(|| corrupt("block table is not readable"))?,
                        )
                    }
                }
                false => BlocksSrc::Owned(Self::decode_blocks(&read_section(
                    &mmap,
                    sb.blk_off,
                    sb.blk_stored as usize,
                    sb.blk_uncompressed as usize,
                )?)?),
            }
        };
        // A flat section is stored verbatim, so it is only a candidate when
        // nothing was compressed away, and only if the header validates.
        // Anything else is the varint format and gets decoded as before.
        if sb.key_stored == sb.key_uncompressed {
            let off = sb.key_off as usize;
            let len = sb.key_stored as usize;
            if let Some(sec) = mmap.get(off..off.saturating_add(len)) {
                if let Some(meta) = FlatIndex::parse(sec) {
                    let mut r = Self::build_flat(
                        mmap,
                        meta,
                        off,
                        len,
                        blocks_src,
                        sb.timestamp,
                        sb.history_from,
                    )?;
                    r.opts = opts;
                    r.advice = advice;
                    // After `opts`, because the replay seeks with the fence
                    // setting this reader was opened with.
                    r.attach_log(sb.log_off, sb.log_len, sb.index_gen);
                    return Ok(r);
                }
            }
        }
        let key_idx = read_section(
            &mmap,
            sb.key_off,
            sb.key_stored as usize,
            sb.key_uncompressed as usize,
        )?;
        let mut r = Self::build(mmap, key_idx, blocks_src, sb.timestamp, sb.history_from)?;
        r.opts = opts;
        r.advice = advice;
        r.attach_log(sb.log_off, sb.log_len, sb.index_gen);
        Ok(r)
    }

    /// Publish the generation being read so the writer will not hand out the
    /// space behind it. Best effort: if the table is full or unwritable the
    /// reader still works, and falls back on the grace window for safety.
    fn claim(path: &Path, generation: u64) -> Option<(usize, MmapMut)> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .ok()?;
        let table = unsafe { MmapMut::map_mut(&file) }.ok()?;
        if table.len() < readers::TABLE_OFF + readers::TABLE_BYTES {
            return None;
        }
        let slot = {
            let t = unsafe { readers::slots(table.as_ptr()) };
            readers::acquire(t, generation.max(1))?
        };
        Some((slot, table))
    }

    /// Load a specific index section, for reading as of an older generation.
    fn open_at(
        path: &Path,
        off: u64,
        stored: usize,
        uncompressed: usize,
        ts: u64,
    ) -> Result<Reader> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let a = Super::decode(&mmap[0..SUPER_BYTES]);
        let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + SUPER_BYTES]);
        let sb = match (a, b) {
            (Some(x), Some(y)) => {
                if x.generation >= y.generation {
                    x
                } else {
                    y
                }
            }
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "no checkpoint",
                ))
            }
        };
        let key_idx = read_section(&mmap, off, stored, uncompressed)?;
        let blk_idx = read_section(
            &mmap,
            sb.blk_off,
            sb.blk_stored as usize,
            sb.blk_uncompressed as usize,
        )?;
        let blocks_src = match flatindex::MappedBlocks::parse(&blk_idx) {
            // This path owns its section rather than borrowing the mapping, so
            // the flat form is converted. Time travel, not the hot path.
            Some(m) => BlocksSrc::Owned(
                (0..m.len())
                    .map(|i| {
                        m.get(&blk_idx, i)
                            .ok_or_else(|| corrupt("block table truncated"))
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            None => BlocksSrc::Owned(Self::decode_blocks(&blk_idx)?),
        };
        Self::build(mmap, key_idx, blocks_src, ts, sb.history_from)
    }

    /// Decode the block table. Shared by both index arms: the block index is
    /// small and is not what either arm is arguing about.
    fn decode_blocks(blk_idx: &[u8]) -> Result<Vec<BlockLoc>> {
        let mut p = 0usize;
        let nblocks = get_uvarint(blk_idx, &mut p) as usize;
        if nblocks > blk_idx.len() {
            return Err(corrupt("block count exceeds the size of the block index"));
        }
        let mut blocks = Vec::with_capacity(nblocks.min(1 << 20));
        for _ in 0..nblocks {
            let off = get_uvarint(blk_idx, &mut p);
            let stored = get_uvarint(blk_idx, &mut p) as u32;
            let uncompressed = get_uvarint(blk_idx, &mut p) as u32;
            let cap = get_uvarint(blk_idx, &mut p) as u32;
            let crc = get_uvarint(blk_idx, &mut p) as u32;
            if p >= blk_idx.len() {
                return Err(corrupt("block index is truncated"));
            }
            let flags = blk_idx[p];
            p += 1;
            blocks.push(BlockLoc {
                off,
                stored,
                uncompressed,
                cap,
                solo: flags & 1 != 0,
                chunked: flags & 2 != 0,
                // The varint format predates per-chunk checksums and has
                // nowhere to put them.
                chunk_crc: false,
                crc,
            });
        }
        Ok(blocks)
    }

    /// Open against a section used where it lies.
    ///
    /// Note what is absent: no loop over keys, no allocation per key, no hash
    /// built. That is the entire difference, and it is the whole of F2.1 and
    /// most of F7.2. Extents are validated against the block table lazily, at
    /// the point a lookup returns one, rather than eagerly here -- validating
    /// eagerly would reintroduce exactly the per-key pass being removed.
    #[allow(clippy::too_many_arguments)]
    fn build_flat(
        mmap: Mmap,
        meta: FlatIndex,
        off: usize,
        len: usize,
        blocks_src: BlocksSrc,
        ts: u64,
        history_from: u64,
    ) -> Result<Reader> {
        let nblocks = match &blocks_src {
            BlocksSrc::Owned(v) => v.len(),
            BlocksSrc::Mapped { meta, .. } => meta.len(),
        };
        let generation = meta.generation;
        let prev = meta.prev;
        let verified = (0..(nblocks * block::MAX_CHUNK_CRCS).div_ceil(64))
            .map(|_| std::sync::atomic::AtomicU64::new(0))
            .collect();
        Ok(Reader {
            mmap,
            overlay: Overlay::default(),
            idx: Idx::Flat { meta, off, len },
            verified,
            blocks_src,
            opts: ReadOptions::default(),
            advice: Readahead::Default,
            cache: BlockCache::new(4096),
            slot: None,
            table: None,
            generation,
            timestamp: ts,
            history_from,
            overwritten: Vec::new(),
            prev,
        })
    }

    fn build(
        mmap: Mmap,
        key_idx: Vec<u8>,
        blocks_src: BlocksSrc,
        ts: u64,
        history_from: u64,
    ) -> Result<Reader> {
        let nblocks = match &blocks_src {
            BlocksSrc::Owned(v) => v.len(),
            BlocksSrc::Mapped { meta, .. } => meta.len(),
        };

        let mut p = 0usize;
        let _gen_read = get_uvarint(&key_idx, &mut p);
        let prev_gen = get_uvarint(&key_idx, &mut p);
        let prev_ts = get_uvarint(&key_idx, &mut p);
        let prev_off = get_uvarint(&key_idx, &mut p);
        let prev_stored = get_uvarint(&key_idx, &mut p);
        let prev_uncompressed = get_uvarint(&key_idx, &mut p);
        let prev = if prev_off > 0 {
            Some((prev_gen, prev_ts, prev_off, prev_stored, prev_uncompressed))
        } else {
            None
        };
        let nkeys = get_uvarint(&key_idx, &mut p) as usize;
        // Every length below is read out of the file, so every one of them is
        // checked against the bytes actually present. Unchecked, a damaged
        // index section panics the calling process -- which for a library
        // embedded in somebody else's address space means their application
        // aborts. `with_capacity` is bounded for the same reason: a corrupt
        // key count would otherwise try to reserve gigabytes before the first
        // length is even looked at.
        if nkeys > key_idx.len() {
            return Err(corrupt("key count exceeds the size of the index"));
        }
        let mut entries: Vec<(Vec<u8>, Extents)> = Vec::with_capacity(nkeys.min(1 << 20));
        for _ in 0..nkeys {
            let klen = get_uvarint(&key_idx, &mut p) as usize;
            let kend = p
                .checked_add(klen)
                .ok_or_else(|| corrupt("key length overflows"))?;
            if kend > key_idx.len() {
                return Err(corrupt("key runs past the end of the index"));
            }
            let key = key_idx[p..kend].to_vec();
            p = kend;
            let n = get_uvarint(&key_idx, &mut p) as usize;
            if n > key_idx.len() {
                return Err(corrupt("extent count exceeds the size of the index"));
            }
            let mut exts = Extents::None;
            for _ in 0..n {
                let block = get_uvarint(&key_idx, &mut p) as u32;
                let o = get_uvarint(&key_idx, &mut p) as u32;
                let l = get_uvarint(&key_idx, &mut p) as u32;
                let last = get_uvarint(&key_idx, &mut p) as u32;
                // Validate the block id once, here, rather than at each of the
                // four read paths that index `self.blocks` with it. A damaged
                // index otherwise names a block that does not exist and every
                // one of those paths panics the calling process.
                if block as usize >= nblocks {
                    return Err(corrupt("extent names a block that does not exist"));
                }
                exts.push(Ext {
                    block,
                    off: o,
                    len: l,
                    last,
                });
            }
            entries.push((key, exts));
        }
        // written sorted, but a checkpoint taken mid-run can interleave, so
        // make the ordering an invariant rather than an assumption
        if !entries.windows(2).all(|w| w[0].0 <= w[1].0) {
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        }

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
            // 0 marks an empty tag, so fold it away
            hash[slot] = (((h >> 56) as u8) | 1, i as u32);
        }

        let cache = BlockCache::new(4096);
        let verified = (0..(nblocks * block::MAX_CHUNK_CRCS).div_ceil(64))
            .map(|_| std::sync::atomic::AtomicU64::new(0))
            .collect();
        Ok(Reader {
            mmap,
            overlay: Overlay::default(),
            idx: Idx::Heap {
                entries,
                hash,
                mask,
            },
            verified,
            blocks_src,
            opts: ReadOptions::default(),
            advice: Readahead::Default,
            cache,
            slot: None,
            table: None,
            generation: _gen_read,
            timestamp: ts,
            history_from,
            overwritten: Vec::new(),
            prev,
        })
    }

    /// Verify an uncompressed block's checksum, at most once per reader.
    #[inline]
    /// Verify only the chunks an extent actually touches.
    ///
    /// `write_block` chunks a *compressed* block so a point read decompresses
    /// one chunk rather than 64KiB -- 68x read amplification was the reason.
    /// The checksum had the same shape and nobody said so: a plain block was
    /// hashed in full to hand back a hundred bytes, and f19-coldscan priced
    /// that at 0.715x on a cold scan. Chunks are verified once each and
    /// remembered, so this converges to the same steady state either way; what
    /// changes is the first touch.
    ///
    /// Every reason to doubt the per-chunk path -- a block that carries none,
    /// a table that cannot be read, a range outside the block -- falls back to
    /// verifying the whole block, never to skipping the check.
    fn verify_range(&self, id: u32, loc: BlockLoc, raw: &[u8], lo: usize, hi: usize) -> Result<()> {
        if !self.opts.verify_checksums || !block::checksums_on() {
            return Ok(());
        }
        if !self.opts.chunk_verify || !loc.chunk_crc || hi > raw.len() || lo >= hi {
            return self.verify_plain(id, raw, loc.crc);
        }
        let (meta, sec) = match &self.blocks_src {
            BlocksSrc::Mapped { meta, off, len } => {
                match self.mmap.get(*off..off.saturating_add(*len)) {
                    Some(sec) => (meta, sec),
                    None => return self.verify_plain(id, raw, loc.crc),
                }
            }
            // The varint table has nowhere to keep them.
            BlocksSrc::Owned(_) => return self.verify_plain(id, raw, loc.crc),
        };
        use std::sync::atomic::Ordering;
        for j in (lo / block::CHUNK)..=((hi - 1) / block::CHUNK) {
            let a = j * block::CHUNK;
            let b = ((j + 1) * block::CHUNK).min(raw.len());
            let (Some(want), true) = (meta.chunk_crc(sec, id as usize, j), a < b) else {
                return self.verify_plain(id, raw, loc.crc);
            };
            let slot = id as usize * block::MAX_CHUNK_CRCS + j;
            let (w, bit) = (slot / 64, 1u64 << (slot % 64));
            let Some(cell) = self.verified.get(w) else {
                return self.verify_plain(id, raw, loc.crc);
            };
            if cell.load(Ordering::Relaxed) & bit != 0 {
                continue;
            }
            if block::crc32(&raw[a..b]) != want {
                return Err(corrupt("block checksum mismatch"));
            }
            cell.fetch_or(bit, Ordering::Relaxed);
        }
        Ok(())
    }

    fn verify_plain(&self, id: u32, raw: &[u8], want: u32) -> Result<()> {
        if !self.opts.verify_checksums || !block::checksums_on() {
            return Ok(());
        }
        use std::sync::atomic::Ordering;
        // Slot zero of this block's chunk row, so the two schemes cannot mark
        // each other's bits.
        let slot = id as usize * block::MAX_CHUNK_CRCS;
        let (w, bit) = (slot / 64, 1u64 << (slot % 64));
        let Some(cell) = self.verified.get(w) else {
            // No room to remember: check every time rather than skip.
            return if block::crc32(raw) == want {
                Ok(())
            } else {
                Err(corrupt("block checksum mismatch"))
            };
        };
        if cell.load(Ordering::Relaxed) & bit != 0 {
            return Ok(());
        }
        if block::crc32(raw) != want {
            return Err(corrupt("block checksum mismatch"));
        }
        cell.fetch_or(bit, Ordering::Relaxed);
        Ok(())
    }

    fn block(&self, id: u32) -> Result<BlockRef<'_>> {
        let loc = self.loc_of(id)?;
        let raw = self.raw_of(loc)?;
        if loc.is_plain() {
            self.verify_plain(id, raw, loc.crc)?;
            // never compressed, so hand out the mapping itself
            return Ok(BlockRef::Mapped(raw));
        }
        if let Some(hit) = self.cache.get(id) {
            return Ok(BlockRef::Owned(hit));
        }
        // Compressed as a single stream, so no per-chunk checksum applies.
        self.verify_plain(id, raw, loc.crc)?;
        let out = Arc::new(block::decompress(raw, loc.uncompressed as usize)?);
        self.cache.put(id, Arc::clone(&out));
        Ok(BlockRef::Owned(out))
    }

    /// Visit every value of a key in append order. Values are handed out as
    /// slices of the block, so a read allocates nothing per value.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let Some(exts) = self.lookup(key) else {
            return Ok(0);
        };
        let mut total = 0u64;
        for e in exts {
            self.check_extent(*e)?;
            let loc = self.loc_of(e.block)?;
            let end = loc.off as usize + loc.stored as usize;
            if end > self.mmap.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "block {} spans {}..{} but the mapping is {} bytes: the file grew after \
                         it was mapped",
                        e.block,
                        loc.off,
                        end,
                        self.mmap.len()
                    ),
                ));
            }
            let raw = &self.mmap[loc.off as usize..end];
            if loc.is_plain() {
                // Stored verbatim, so there is no chunk directory to carry a
                // checksum; the block table carries them instead.
                let (a, b) = (e.off as usize, (e.off + e.len) as usize);
                if b > raw.len() {
                    return Err(corrupt("extent runs past its block"));
                }
                self.verify_range(e.block, loc, raw, a, b)?;
                total += emit(&raw[a..b], &mut f)?;
            } else if loc.chunked || loc.solo {
                if std::env::var_os("SUPDB_DEBUG").is_some() && loc.stored as usize > raw.len() {
                    eprintln!(
                        "# block {} off={} stored={} chunked={} solo={} mmap={}",
                        e.block,
                        loc.off,
                        loc.stored,
                        loc.chunked,
                        loc.solo,
                        self.mmap.len()
                    );
                }
                // one key's run: decompress into scratch, retain nothing
                total += SCRATCH.with(|s| -> Result<u64> {
                    let mut buf = s.borrow_mut();
                    let un = loc.uncompressed as usize;
                    if buf.len() < un {
                        buf.resize(un, 0);
                    }
                    let (a, b) = (e.off as usize, (e.off + e.len) as usize);
                    let r = if loc.chunked {
                        block::read_chunked_range(raw, un, a, b, &mut buf[..un])
                    } else {
                        self.verify_plain(e.block, raw, loc.crc)
                            .and_then(|_| block::decompress_into(raw, &mut buf, un))
                    };
                    if let Err(err) = r {
                        if std::env::var_os("SUPDB_DEBUG").is_some() {
                            let head: Vec<u8> = raw.iter().take(12).copied().collect();
                            eprintln!(
                                "# gen={} blk={} off={} stored={} uncomp={} cap={} chunked={} solo={} \
                                 ext=({}..{}) mmap={} nblocks={} head={:?}",
                                self.generation, e.block, loc.off, loc.stored, loc.uncompressed, loc.cap,
                                loc.chunked, loc.solo, a, b, self.mmap.len(),
                                self.nblocks(), head
                            );
                        }
                        return Err(err);
                    }
                    if b > un {
                        return Err(corrupt("extent runs past its block"));
                    }
                    emit(&buf[a..b], &mut f)
                })?;
            } else {
                let b = self.block(e.block)?;
                let sl = b.as_slice();
                let (a, z) = (e.off as usize, (e.off + e.len) as usize);
                if z > sl.len() {
                    return Err(corrupt("extent runs past its block"));
                }
                total += emit(&sl[a..z], &mut f)?;
            }
        }
        Ok(total)
    }

    /// Length of the record starting at `at` within the extent.
    ///
    /// A point read of a solo block still has to decompress the run to reach
    /// one record; scratch keeps it from allocating as well.
    fn record_len_at(&self, e: Ext, at: u32) -> Result<i32> {
        self.check_extent(e)?;
        let loc = self.loc_of(e.block)?;
        let raw = self.raw_of(loc)?;
        if loc.is_plain() {
            // A record length is a varint of at most ten bytes, so this needs
            // the one chunk it lands in rather than the block.
            let p0 = (e.off + at) as usize;
            self.verify_range(e.block, loc, raw, p0, (p0 + 10).min(raw.len()))?;
            let mut p = p0;
            return Ok(get_uvarint(raw, &mut p) as i32);
        }
        if loc.chunked || loc.solo {
            return SCRATCH.with(|s| -> Result<i32> {
                let mut buf = s.borrow_mut();
                let un = loc.uncompressed as usize;
                if buf.len() < un {
                    buf.resize(un, 0);
                }
                let at = (e.off + at) as usize;
                if loc.chunked {
                    // only the chunks holding this one record
                    block::read_chunked_range(raw, un, at, (at + 16).min(un), &mut buf[..un])?;
                } else {
                    self.verify_plain(e.block, raw, loc.crc)?;
                    block::decompress_into(raw, &mut buf, un)?;
                }
                let mut p = at;
                Ok(get_uvarint(&buf, &mut p) as i32)
            });
        }
        let b = self.block(e.block)?;
        let mut p = (e.off + at) as usize;
        Ok(get_uvarint(b.as_slice(), &mut p) as i32)
    }

    pub fn read_first(&self, key: &[u8]) -> Result<i32> {
        let Some(exts) = self.lookup(key) else {
            return Ok(-1);
        };
        let Some(e) = exts.first() else { return Ok(-1) };
        self.record_len_at(*e, 0)
    }

    pub fn read_last(&self, key: &[u8]) -> Result<i32> {
        let Some(exts) = self.lookup(key) else {
            return Ok(-1);
        };
        let Some(e) = exts.last() else { return Ok(-1) };
        self.record_len_at(*e, e.last)
    }

    pub fn keys(&self) -> usize {
        self.merged_len()
    }

    /// Bytes the key index occupies, as stored.
    ///
    /// Diagnostic, and the honest way to price the index arms against each
    /// other: resident-set size after a read pass includes block and cache
    /// pages common to both arms, which dilutes the difference being measured.
    /// Zero for a decoded index, whose cost is in the heap rather than in a
    /// section -- ask `keys()` and the format for that.
    pub fn index_bytes(&self) -> usize {
        match &self.idx {
            Idx::Flat { len, .. } => *len,
            Idx::Heap { .. } => 0,
        }
    }

    /// Byte ranges holding live block payload, as (offset, stored length).
    ///
    /// Diagnostic. A corruption experiment that picks byte offsets uniformly
    /// mostly lands in size-class padding or in an index section, so a "how
    /// much damage goes unnoticed" figure taken that way says more about the
    /// file's layout than about the engine's integrity checking. This lets a
    /// caller aim at bytes that actually carry data. An fsck-style tool would
    /// want the same thing.
    pub fn block_extents(&self) -> Vec<(u64, u64)> {
        // Only blocks some key still points at. A superseded block whose space
        // has not yet been reused is still in the file and still in the block
        // table, but nothing reads it -- damage there is undetectable and
        // correctly so, and counting it as undetected corruption would
        // overstate the gap a second time.
        let mut referenced = vec![false; self.nblocks()];
        // Merged, so a block that only a logged record points at counts as
        // referenced. It is live -- a reader can reach it -- and calling it
        // unreferenced would let c1 treat damage there as undetectable.
        for r in 0..self.merged_len() {
            let Some((_, exts)) = self.merged_at(r) else {
                continue;
            };
            for e in exts {
                if let Some(slot) = referenced.get_mut(e.block as usize) {
                    *slot = true;
                }
            }
        }
        referenced
            .into_iter()
            .enumerate()
            .filter(|(_, live)| *live)
            .filter_map(|(i, _)| self.loc_of(i as u32).ok())
            .map(|b| (b.off, b.stored as u64))
            .collect()
    }

    #[inline]
    /// Replay the redo log over the published index.
    ///
    /// Called once, at open, after the index exists -- the insert positions
    /// are ranks *in that index*, so they cannot be computed before it. A
    /// record whose key is already published replaces its extents and moves
    /// nothing; one whose key is new takes a rank, and every published rank at
    /// or after it shifts by one.
    fn attach_log(&mut self, off: u64, len: u64, index_gen: u64) {
        if len == 0 {
            return;
        }
        let Some(arena) = self
            .mmap
            .get(off as usize..(off as usize).saturating_add(len as usize))
        else {
            // A log outside the mapping is corruption, and a reader that
            // silently ignored it would serve a state the writer was told was
            // durable. `Store::open` refuses outright; a reader has no error
            // channel here, so it keeps the overlay empty and the caller sees
            // the last published state -- which is stale, not wrong.
            return;
        };
        let (records, _) = log_replay(arena);
        // Same stamp filter as Store::open: a record at or below the index
        // generation describes state the index has since superseded.
        let records: Vec<(Vec<u8>, Extents)> = records
            .into_iter()
            .filter(|(g, _, _)| *g > index_gen)
            .map(|(_, k, e)| (k, e))
            .collect();
        if records.is_empty() {
            return;
        }
        // Last record for a key wins, and the result is sorted so a lookup is
        // a binary search and the merge is a walk.
        let mut entries = records;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| {
            if a.0 == b.0 {
                // `dedup_by` keeps `b` and drops `a`, and `a` is the later of
                // the pair, so move its extents across.
                b.1 = std::mem::replace(&mut a.1, Extents::None);
                true
            } else {
                false
            }
        });
        let mut inserts: Vec<(usize, usize, usize)> = Vec::new();
        for (i, (k, _)) in entries.iter().enumerate() {
            if self.idx.lookup(&self.mmap, k).is_none() {
                let at = self.idx.seek(&self.mmap, k, self.opts.seek_fence);
                inserts.push((at, 0, i));
            }
        }
        // `entries` is sorted by key, so the insertions are already in key
        // order; sorting by published rank keeps ties in that order.
        inserts.sort_by_key(|(at, _, _)| *at);
        for (k, e) in inserts.iter_mut().enumerate() {
            e.1 = e.0 + k;
        }
        self.overlay = Overlay { entries, inserts };
    }

    /// Published keys plus the log's insertions.
    fn merged_len(&self) -> usize {
        self.idx.len() + self.overlay.extra()
    }

    /// A key's extents, the log's version winning where it has one.
    fn merged_lookup(&self, key: &[u8]) -> Option<&[Ext]> {
        if let Some(e) = self.overlay.find(key) {
            // An empty extent list is a delete the log carried, and it has to
            // read as absent rather than falling through to the published
            // index, which still has the key.
            let s = e.as_slice();
            return if s.is_empty() { None } else { Some(s) };
        }
        self.idx.lookup(&self.mmap, key)
    }

    fn merged_at(&self, rank: usize) -> Option<(&[u8], &[Ext])> {
        match self.overlay.resolve(rank) {
            Where::Overlay(i) => {
                let (k, e) = &self.overlay.entries[i];
                Some((k.as_slice(), e.as_slice()))
            }
            Where::Published(r) => {
                let (k, e) = self.idx.at(&self.mmap, r)?;
                // The published rank is still the right key; its extents may
                // have been superseded by a record the log carries.
                match self.overlay.find(k) {
                    Some(over) => Some((k, over.as_slice())),
                    None => Some((k, e)),
                }
            }
        }
    }

    fn merged_seek(&self, key: &[u8]) -> usize {
        let r = self.idx.seek(&self.mmap, key, self.opts.seek_fence);
        r + self.overlay.before(r)
    }

    fn lookup(&self, key: &[u8]) -> Option<&[Ext]> {
        self.merged_lookup(key)
    }

    /// Every block this reader can see, in id order.
    ///
    /// For `Store::open`, which has to rebuild the appender's block table and
    /// would otherwise duplicate the format handling, the superblock slot
    /// selection and the bounds checking that getting here already did.
    pub(crate) fn all_blocks(&self) -> Result<Vec<BlockLoc>> {
        (0..self.nblocks() as u32).map(|i| self.loc_of(i)).collect()
    }

    /// Per-chunk checksums for every block, in id order. Zeroed rows for
    /// blocks that carry none.
    pub(crate) fn all_chunk_crcs(&self) -> Vec<[u32; block::MAX_CHUNK_CRCS]> {
        let n = self.nblocks();
        let mut out = vec![[0u32; block::MAX_CHUNK_CRCS]; n];
        if let BlocksSrc::Mapped { meta, off, len } = &self.blocks_src {
            if let Some(sec) = self.mmap.get(*off..off.saturating_add(*len)) {
                for (i, row) in out.iter_mut().enumerate() {
                    for (j, c) in row.iter_mut().enumerate() {
                        *c = meta.chunk_crc(sec, i, j).unwrap_or(0);
                    }
                }
            }
        }
        out
    }

    /// Every key and its extents, in rank order.
    pub(crate) fn entry_at(&self, rank: usize) -> Option<(&[u8], &[Ext])> {
        self.merged_at(rank)
    }

    /// The readahead advice actually in force, with `Auto` resolved.
    pub fn advice(&self) -> Readahead {
        self.advice
    }

    /// Position of the first key at or after `key`.
    pub fn seek(&self, key: &[u8]) -> usize {
        self.merged_seek(key)
    }

    /// Visit keys in order from `from`, handing each key's values to the
    /// visitor. Returns the number of values emitted.
    ///
    /// A seal batch is sorted by key before it is packed, so a block holds a
    /// contiguous run of keys and a scan in key order returns to the same
    /// block hundreds of times before moving on -- around eight hundred keys
    /// per block at the sizes measured here. Holding the last block decoded
    /// turns that into no work at all; decompressing per key threw it away.
    ///
    /// A scan also needs no lookup: the entry is already in hand, so neither
    /// the hash nor the key copy that read_all performs is required.
    pub fn scan<F: FnMut(&[u8], &[u8])>(
        &self,
        from: Option<&[u8]>,
        limit: usize,
        mut f: F,
    ) -> Result<u64> {
        let start = from.map(|k| self.seek(k)).unwrap_or(0);
        let end = (start + limit).min(self.merged_len());
        let mut n = 0u64;
        let mut cached: u32 = u32::MAX;
        let mut buf: Vec<u8> = Vec::new();
        // which chunks of the cached block have been decoded
        let mut have: Vec<u64> = Vec::new();

        // The block the previous entry used. A scan walks keys in order and
        // consecutive keys usually sit in the same block, so resolving it
        // again means re-reading the block table, re-bounds-checking the
        // mapping and re-testing a checksum bit to learn what is already
        // known. It was resolved twice per entry -- `check_extent` did it and
        // then the read did it again.
        let mut held: Option<(u32, BlockLoc, &[u8])> = None;
        // The chunk the previous entry verified. Per-chunk verification is
        // checked per *extent* where whole-block verification is checked once
        // per block-resolve and amortised over every entry in it, which is how
        // a change that hashes less ended up slower. A 4KiB chunk holds about
        // thirty-five of these entries, and consecutive entries in a scan are
        // in the same one.
        let mut verified_chunk: (u32, usize) = (u32::MAX, usize::MAX);
        // The index section, sliced once. `Idx::at` takes the whole mapping and
        // re-slices it to the section on every rank, which is a bounds check
        // and two additions per entry to arrive at the same bytes.
        // The shortcut is only sound when there is nothing to merge: it goes
        // straight to the mapped section by rank, which is exactly what the
        // overlay reorders and supersedes. With a log outstanding the scan
        // takes the merged path, which is slower and correct.
        let flat = match &self.idx {
            Idx::Flat { meta, .. } if self.overlay.is_empty() => {
                Some((meta, self.idx.section(&self.mmap)))
            }
            _ => None,
        };

        for i in start..end {
            let got = match flat {
                Some((meta, sec)) => meta.at(sec, i),
                None => self.merged_at(i),
            };
            let Some((key, exts)) = got else {
                continue;
            };
            for e in exts {
                let (loc, raw) = match held {
                    Some((b, loc, raw)) if self.opts.scan_block_cache && b == e.block => (loc, raw),
                    _ => {
                        let loc = self.loc_of(e.block)?;
                        self.check_extent_loc(loc)?;
                        let raw = self.raw_of(loc)?;
                        held = Some((e.block, loc, raw));
                        (loc, raw)
                    }
                };
                let extent: &[u8] = if loc.is_plain() {
                    // Verified per extent rather than per block: holding the
                    // block across entries is what makes the scan fast, and
                    // checksumming all of it on the first entry is what made
                    // the first pass slow.
                    let (a, b) = (e.off as usize, (e.off + e.len) as usize);
                    let Some(bytes) = raw.get(a..b) else {
                        return Err(corrupt("extent runs past its block"));
                    };
                    let (c0, c1) = (a / block::CHUNK, b.saturating_sub(1) / block::CHUNK);
                    if c0 != c1 || verified_chunk != (e.block, c0) {
                        self.verify_range(e.block, loc, raw, a, b)?;
                        if c0 == c1 {
                            verified_chunk = (e.block, c0);
                        }
                    }
                    bytes
                } else {
                    let un = loc.uncompressed as usize;
                    if cached != e.block {
                        if buf.len() < un {
                            buf.resize(un, 0);
                        }
                        have.clear();
                        have.resize(un / 64 + 2, 0);
                        if !loc.chunked {
                            self.verify_plain(e.block, raw, loc.crc)?;
                            block::decompress_into(raw, &mut buf, un)?;
                        }
                        cached = e.block;
                    }
                    if loc.chunked {
                        // only the chunks this key needs and does not have
                        block::read_chunks_into(
                            raw,
                            un,
                            e.off as usize,
                            (e.off + e.len) as usize,
                            &mut buf[..un],
                            &mut have,
                        )?;
                    }
                    &buf[e.off as usize..(e.off + e.len) as usize]
                };
                let mut p = 0usize;
                while p < extent.len() {
                    let len = get_uvarint(extent, &mut p) as usize;
                    let end = p
                        .checked_add(len)
                        .ok_or_else(|| corrupt("record length overflows"))?;
                    // `get` is the bounds check, so the explicit one it used to
                    // do first was the same test run twice per record.
                    let Some(rec) = extent.get(p..end) else {
                        return Err(corrupt("record runs past the end of its extent"));
                    };
                    f(key, rec);
                    n += 1;
                    p = end;
                }
            }
        }
        Ok(n)
    }
}

fn read_section(mmap: &Mmap, off: u64, stored: usize, uncompressed: usize) -> Result<Vec<u8>> {
    // A writer may have extended the file since this mapping was made, so a
    // section can legitimately lie beyond its end. Report it rather than
    // indexing past the slice.
    let end = off as usize + stored;
    if end > mmap.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "section at {off}..{end} lies past the {} byte mapping",
                mmap.len()
            ),
        ));
    }
    let raw = &mmap[off as usize..end];
    if stored == uncompressed {
        Ok(raw.to_vec())
    } else {
        block::decompress(raw, uncompressed)
    }
}

impl Store {
    pub fn reopen(path: &Path) -> Result<Reader> {
        Reader::open(path)
    }
}

#[allow(dead_code)]
fn unused_seek(f: &mut File) -> Result<u64> {
    f.seek(SeekFrom::Current(0))
}
