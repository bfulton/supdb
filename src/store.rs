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

const MAGIC: u64 = 0x5355_5044_4200_0001;

/// Two superblock slots live in the first sector-pair of the file, and a
/// checkpoint alternates between them.
///
/// The store previously wrote its index only at close, so a crash lost not
/// just recent appends but the entire file: without a footer nothing could be
/// read back at all. Alternating slots make a checkpoint atomic in the way
/// that matters -- a torn write can damage at most the slot being written,
/// and the other still describes a complete, older state. Recovery picks the
/// valid slot with the higher generation.
const SUPER: u64 = 4096;
const SLOT: u64 = 512;

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
}

impl Super {
    fn fields(&self) -> [u64; 13] {
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

    fn encode(&self) -> [u8; 120] {
        let mut out = [0u8; 120];
        for (i, v) in self.fields().iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        out[104..112].copy_from_slice(&MAGIC.to_le_bytes());
        out[112..120].copy_from_slice(&self.checksum().to_le_bytes());
        out
    }

    fn decode(buf: &[u8]) -> Option<Super> {
        if buf.len() < 120 {
            return None;
        }
        let f: Vec<u64> = (0..13)
            .map(|i| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap()))
            .collect();
        if u64::from_le_bytes(buf[104..112].try_into().unwrap()) != MAGIC {
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
        };
        if u64::from_le_bytes(buf[112..120].try_into().unwrap()) != s.checksum() {
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
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            mapped_blocks: true,
            scan_block_cache: true,
            seek_fence: true,
            chunk_verify: true,
            verify_checksums: true,
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
                Some(v) => v
                    .parse::<u32>()
                    .map(Sync::EveryN)
                    .unwrap_or(Sync::Always),
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
        let loc = self.blocks[e.block as usize];
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
        Ok(full[e.off as usize..(e.off + e.len) as usize].to_vec())
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
    buf: Vec<u8>,
    /// Offset of the most recently appended record within `buf`.
    last: u32,
    /// Extents this pending value replaces, to be released when it is sealed
    /// and only if the store is reclaiming.
    supersedes: Vec<Ext>,
    /// True when this came from put(): the sealed extent replaces the key's
    /// extents rather than being added to them.
    replaces: bool,
}

struct Shard {
    merges: u64,
    /// One table holding both a key's sealed extents and the value still
    /// buffered for it, so a put probes once rather than twice.
    keys: KeyTable<Pending>,
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
    shards: Vec<Mutex<Shard>>,
    appender: Mutex<Appender>,
    opts: Options,
    path: PathBuf,
}

impl Store {
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
            shards,
            appender: Mutex::new(Appender {
                table,
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
        let grew = {
            let e = sh.keys.get_or_insert(key);
            let p = e.pending.get_or_insert_with(Pending::default);
            let before = p.buf.len();
            p.last = before as u32;
            put_uvarint(&mut p.buf, value.len() as u64);
            p.buf.extend_from_slice(value);
            p.buf.len() - before
        };
        sh.pending_bytes += grew;

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
            if p.buf.len() >= self.opts.solo_threshold {
                // big enough to compress on its own; giving it a private block
                // means a read of this key decompresses only this key
                let id = {
                    let mut ap = self.appender.lock().unwrap();
                    ap.write_block(
                        &p.buf,
                        self.opts.compress,
                        true,
                        self.opts.solo_chunk_size,
                        self.opts.reclaim,
                    )?
                };
                let len = p.buf.len() as u32;
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
            if sh.builder.would_overflow(p.buf.len()) {
                self.flush_builder(sh)?;
            }
            let off = sh.builder.push(&p.buf);
            sh.members
                .push((idx, off, p.buf.len() as u32, p.last, p.replaces));
        }
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
        let (before, after) = {
            let e = sh.keys.get_or_insert(key);
            let p = e.pending.get_or_insert_with(Pending::default);
            let before = p.buf.len();
            p.buf.clear();
            put_uvarint(&mut p.buf, value.len() as u64);
            p.buf.extend_from_slice(value);
            p.last = 0;
            p.replaces = true;
            (before, p.buf.len())
        };
        sh.pending_bytes = sh.pending_bytes + after - before;
        // Same hazard as `delete`: a replacement supersedes every earlier
        // value, including one already staged in the block builder by an
        // inline seal. Left in place, `flush_builder` would push the
        // superseded extent onto the entry after this replacement lands.
        if let Some(idx) = sh.keys.index_of(key) {
            sh.members.retain(|m| m.0 != idx);
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
            let freed = e.pending.take().map(|p| p.buf.len()).unwrap_or(0);
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
            Some(p) => (p.buf.as_slice(), p.replaces),
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
        let in_place = self.checkpoint_in_place(&changed, nkeys)?;

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
                all.sort_unstable_by(|a, b| a.0.cmp(b.0));
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
                flatindex::encode(&all, gen, p, key_hash)
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
        let key_loc = match (in_place, ap.last_index) {
            (true, Some(loc)) => loc,
            _ if flat => {
                let mut payload = key_idx;
                if self.opts.write_index_slack {
                    payload.resize(key_reserve, 0);
                }
                write_section_raw(&mut ap, &payload, key_reserve, self.opts.reclaim)?
            }
            _ => write_section(&mut ap, &key_idx, self.opts.reclaim)?,
        };
        let blk_loc = if blk_flat {
            write_section_raw(&mut ap, &blk_idx, blk_idx.len(), self.opts.reclaim)?
        } else {
            write_section(&mut ap, &blk_idx, self.opts.reclaim)?
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
            ap.file.sync_data()?;
        }

        let gen = ap.generation + 1;
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
            ap.file.write_all_at(&sb.encode(), other)?;
        }
        if do_sync {
            ap.file.sync_data()?;
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
        if flat {
            if let Ok(map) = unsafe { MmapMut::map_mut(&ap.file) } {
                let (o, l) = (key_loc.off as usize, key_loc.stored as usize);
                let adopted = map
                    .get(o..o.saturating_add(l))
                    .and_then(FlatIndex::parse)
                    .map(|meta| (map, meta, key_loc.off, key_loc.stored as u64));
                if adopted.is_some() {
                    ap.live_key_off = Some(key_loc.off);
                }
                ap.live_index = adopted;
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
    fn checkpoint_in_place(&self, changed: &[(Vec<u8>, Extents)], nkeys: usize) -> Result<bool> {
        use std::sync::atomic::{AtomicU64, Ordering};
        let mut ap = self.appender.lock().unwrap();
        let Some((map, meta, sec_off, sec_len)) = ap.live_index.as_mut() else {
            return Ok(false);
        };
        if nkeys != meta.len() {
            // A key was added or removed: the hash and the sorted directory
            // both have to change, which is the rewrite this path avoids.
            return Ok(false);
        }
        let (off, len) = (*sec_off as usize, *sec_len as usize);
        let Some(sec) = map.get(off..off + len) else {
            return Ok(false);
        };

        // Work out the edits first, so nothing is written until every one of
        // them is known to fit.
        let mut edits: Vec<(usize, u64, usize, Vec<u8>)> = Vec::new();
        let mut probe =
            FlatIndex::parse(sec).ok_or_else(|| corrupt("live index no longer parses"))?;
        probe.set_bump(meta.bump());
        for (k, exts) in changed {
            let slice = exts.as_slice();
            let Some(slot_at) = meta.slot_of(sec, k, key_hash) else {
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
                bytes,
            ));
        }
        if edits.is_empty() {
            return Ok(true);
        }

        // Records first. Nothing points at them yet, so a crash here leaks
        // slack and loses nothing.
        for (at, _, _, bytes) in &edits {
            map[*at..*at + bytes.len()].copy_from_slice(bytes);
        }
        // Then the slots, one aligned store each. This is the publish.
        for (_, value, slot_at, _) in &edits {
            debug_assert_eq!(
                slot_at % 8,
                0,
                "a slot must be 8-byte aligned to publish atomically"
            );
            let cell = unsafe { &*(map.as_ptr().add(*slot_at) as *const AtomicU64) };
            cell.store(*value, Ordering::Release);
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
        crc: if block::checksums_on() {
            block::crc32(payload)
        } else {
            0
        },
    };
    ap.file.write_all_at(payload, off)?;
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

fn key_hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
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
        let a = Super::decode(&mmap[0..120]);
        let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + 120]);
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
            let a = Super::decode(&mmap[0..120]);
            let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + 120]);
            let newest = match (a, b) {
                (Some(x), Some(y)) => Some(if x.generation >= y.generation { x } else { y }),
                (Some(x), None) => Some(x),
                (None, Some(y)) => Some(y),
                (None, None) => None,
            };
            match newest {
                Some(sb) if sb.high_water as usize <= mmap.len() => {
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
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "no valid supdb checkpoint",
                    ))
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
        let a = Super::decode(&mmap[0..120]);
        let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + 120]);
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
            idx: Idx::Flat { meta, off, len },
            verified,
            blocks_src,
            opts: ReadOptions::default(),
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
            idx: Idx::Heap {
                entries,
                hash,
                mask,
            },
            verified,
            blocks_src,
            opts: ReadOptions::default(),
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
        self.idx.len()
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
        for r in 0..self.idx.len() {
            let Some((_, exts)) = self.idx.at(&self.mmap, r) else {
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
    fn lookup(&self, key: &[u8]) -> Option<&[Ext]> {
        self.idx.lookup(&self.mmap, key)
    }

    /// Position of the first key at or after `key`.
    pub fn seek(&self, key: &[u8]) -> usize {
        self.idx.seek(&self.mmap, key, self.opts.seek_fence)
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
        let end = (start + limit).min(self.idx.len());
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
        let flat = match &self.idx {
            Idx::Flat { meta, .. } => Some((meta, self.idx.section(&self.mmap))),
            _ => None,
        };

        for i in start..end {
            let got = match flat {
                Some((meta, sec)) => meta.at(sec, i),
                None => self.idx.at(&self.mmap, i),
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
