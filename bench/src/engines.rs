//! Adapters for the comparison field.
//!
//! Every engine here is a native Rust binding, so a measurement crosses no
//! language boundary. The design document's cross-language harness quantified
//! its JNI bias at <=11% of the append gap and controlled for it with a pure
//! Java engine, which is good practice -- but the bias is now avoidable
//! rather than merely bounded.
//!
//! Fairness rules, applied to every engine equally:
//!
//!   * **Batch size is shared.** A transactional engine committing once per
//!     operation is not being compared, it is being handicapped -- the exact
//!     defect the design document confesses to in its own LMDB adapter.
//!   * **No allocation per value on the read path** where the API allows it.
//!     That handicap was worth 2.3x on LMDB when it was removed.
//!   * **Guarantees are matched, not merely recorded.** This rule used to read
//!     "what each engine promises is recorded, not assumed", and recording is
//!     not controlling. The features table below said for months that Supdb
//!     does not commit durably and LMDB does; an early ordering compared their load
//!     throughput anyway and reported Supdb 1.33x ahead. Measured with the two
//!     committing on the same boundary, LMDB is about 19x faster. That is the
//!     same defect as the first rule -- "committing once per operation is not
//!     being compared, it is being handicapped" -- except that when the
//!     handicap fell on the comparator it was called a defect and fixed, and
//!     when it fell in Supdb's favour it was written into a table.
//!
//!     So every axis that can be equalized is, in the adapter, before a number
//!     is reported: `supdb-durable` checkpoints on LMDB's commit boundary, and
//!     `lmdb-nosync` gives up durability the way `supdb-ingest` does. The
//!     checksum axis equalizes downward, since LMDB has none to turn on -- and
//!     that one was costing Supdb 8.5% on every write number in the other
//!     direction, unequalized for just as long.
//!
//!     One axis cannot be equalized: LMDB cannot stop being transactional.
//!     That residual runs *against* LMDB -- it pays for atomic commit and
//!     isolation that Supdb does not provide -- so a matched comparison Supdb
//!     loses is a lower bound on the loss, and one it wins is not yet a win.
//!     `ordering_of` enforces the matching and names the residual; the table
//!     is a precondition now rather than a disclaimer.

use std::path::{Path, PathBuf};

/// What an engine actually guarantees. Reported beside every number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Features {
    pub durable_commit: bool,
    pub transactions: bool,
    pub checksums: bool,
    pub reopen_for_write: bool,
    pub read_your_writes: bool,
    pub ordered_scan: bool,
}

impl Features {
    /// Axes where two engines promise different things and could have been
    /// made to promise the same, restricted to those that bear on this metric.
    ///
    /// A non-empty answer means the pair is not comparable on that metric --
    /// not that the number is noisy, that it is not an ordering. Durability
    /// is an axis like the others: a guarantee group is defined by it, and
    /// for as long as this skipped it on the buffered group -- on the
    /// reasoning that a read does not care when a commit lands, which is
    /// true of a read and not of the group -- `supdb-ingest` sat in that
    /// group committing durably on every batch and the check reported the
    /// group matched.
    pub fn unmatched(&self, other: &Features) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.durable_commit != other.durable_commit {
            v.push("durable_commit");
        }
        if self.checksums != other.checksums {
            v.push("checksums");
        }
        if self.read_your_writes != other.read_your_writes {
            v.push("read_your_writes");
        }
        if self.ordered_scan != other.ordered_scan {
            v.push("ordered_scan");
        }
        if self.reopen_for_write != other.reopen_for_write {
            v.push("reopen_for_write");
        }
        v
    }

    /// The asymmetry that cannot be equalized, and which way it leans.
    ///
    /// LMDB cannot stop being transactional, so a pair matched on everything
    /// else still has one engine paying for atomic commit and isolation the
    /// other does not provide. That is not a reason to refuse the comparison;
    /// it is a reason to read it as a bound. `true` means *this* engine is the
    /// one getting the free ride.
    pub fn free_ride(&self, other: &Features) -> bool {
        other.transactions && !self.transactions
    }

    /// How many of the six an engine provides. A throughput comparison
    /// between engines with different scores is a comparison of promises as
    /// much as of implementations.
    pub fn score(&self) -> usize {
        [
            self.durable_commit,
            self.transactions,
            self.checksums,
            self.reopen_for_write,
            self.read_your_writes,
            self.ordered_scan,
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }
}

pub type Res<T> = Result<T, String>;

/// A batch assembled without an allocation per record: keys and values
/// copied once into two arenas that are reused across batches, and the
/// borrowed pairs built at the flush. What it replaces -- a `Vec` of owned
/// pairs -- allocated and freed two vectors per record, and cachegrind put
/// that at 640 instructions a record, a term every engine paid identically
/// and that therefore sat inside every load ratio the suite reported.
pub struct Batch {
    keys: Vec<u8>,
    vals: Vec<u8>,
    ends: Vec<(u32, u32)>,
}

impl Batch {
    pub fn with_capacity(records: usize, value_size: usize) -> Batch {
        Batch {
            keys: Vec::with_capacity(records * 16),
            vals: Vec::with_capacity(records * value_size),
            ends: Vec::with_capacity(records),
        }
    }

    pub fn push(&mut self, key: &[u8], value: &[u8]) {
        self.keys.extend_from_slice(key);
        self.vals.extend_from_slice(value);
        self.ends
            .push((self.keys.len() as u32, self.vals.len() as u32));
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Hand the batch to the engine and empty it, keeping the arenas. A
    /// no-op when empty. One vector of slice pairs per flush -- one
    /// allocation per batch, against two per record before.
    pub fn flush(&mut self, e: &mut dyn Engine) -> Res<()> {
        if self.ends.is_empty() {
            return Ok(());
        }
        let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(self.ends.len());
        let (mut ks, mut vs) = (0usize, 0usize);
        for &(ke, ve) in &self.ends {
            pairs.push((&self.keys[ks..ke as usize], &self.vals[vs..ve as usize]));
            ks = ke as usize;
            vs = ve as usize;
        }
        e.write_batch(&pairs)?;
        self.keys.clear();
        self.vals.clear();
        self.ends.clear();
        Ok(())
    }

    /// As `flush`, through `Engine::update_batch`: the keys may exist and
    /// the values replace.
    pub fn flush_updates(&mut self, e: &mut dyn Engine) -> Res<()> {
        if self.ends.is_empty() {
            return Ok(());
        }
        let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(self.ends.len());
        let (mut ks, mut vs) = (0usize, 0usize);
        for &(ke, ve) in &self.ends {
            pairs.push((&self.keys[ks..ke as usize], &self.vals[vs..ve as usize]));
            ks = ke as usize;
            vs = ve as usize;
        }
        e.update_batch(&pairs)?;
        self.keys.clear();
        self.vals.clear();
        self.ends.clear();
        Ok(())
    }
}

/// What `Engine::zpages_state` answers: the second form as the engine holds
/// it at the moment of asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZState {
    /// The form is forced on (`supdb::ZPages::Always`), as the engine's own
    /// options say and not as the arm's name does.
    pub forced: bool,
    /// The form is the controller's to choose (`supdb::ZPages::Auto`): what
    /// it reads through is its decision, and the rows carry it.
    pub auto: bool,
    /// Point reads of a partition answered from a form file, over the
    /// store's life.
    pub reads: u64,
    /// Reads and counts of a partition that went to the raw form instead,
    /// over the store's life: the partition had no form file in service, or a
    /// check of the form file failed on the read.
    pub raw: u64,
    /// The live partitions, those with a form file, and those with none for
    /// being under the floor.
    pub partitions: u64,
    pub form_files: u64,
    pub small: u64,
    /// Form files out of service: a check failed on one of their pages, and
    /// the partition is read raw from then on.
    pub damaged: u64,
    /// What the store's directory holds of the form: files named for a
    /// form file, and other files in the form's prefix (a build's temp name,
    /// or any name the engine's sweep would take). A forced store settled has
    /// one file for each form file and no other: a file nothing claims is disk
    /// the row's `bytes_on_disk_per_byte` counts.
    pub files: u64,
    pub stray: u64,
}

pub trait Engine {
    fn name(&self) -> &'static str;
    fn features(&self) -> Features;
    /// Write a batch and make it visible to this engine's own read path.
    ///
    /// Borrowed, not owned: the first form took `&[(Vec<u8>, Vec<u8>)]`,
    /// and measured, the two allocations, two copies and two frees that
    /// cost per record to be 640 instructions -- as much as the next
    /// engine's whole commit path -- paid alike by every adapter and so
    /// folded into every load ratio the suite reports. `Batch` builds one
    /// without allocating per record.
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()>;
    /// Write a batch whose keys may already exist, with the value replacing
    /// what was there: YCSB's update. The same as `write_batch` for every
    /// single-value engine, and for `Store`, whose `put` replaces; the next
    /// engine's `write_batch` appends, which is its load verb, and an update
    /// through it accumulated values on hot keys until reads walked them.
    fn update_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        self.write_batch(items)
    }
    /// Bytes returned for the key; 0 for a miss.
    fn get(&mut self, key: &[u8]) -> Res<usize>;
    /// Bytes visited scanning `n` entries from `from`.
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize>;
    /// Make everything written durable and readable.
    fn sync(&mut self) -> Res<()>;
    fn size_bytes(&self) -> u64;
    /// A reader for a thread of its own, made here on the thread that
    /// owns the engine and opened on the thread that reads through it.
    /// Two steps because the engines differ in which one binds to a
    /// thread: supdb's `Db::reader` claims a slot in the reader table
    /// here and hands back a `Send` handle; RocksDB's handle is shared,
    /// so the opener carries a reference to it; LMDB's read transaction
    /// is bound to the thread that begins it -- heed's `RoTxn` is not
    /// `Send` without the crate feature that opens the environment with
    /// `MDB_NOTLS` -- so the opener carries the environment and begins
    /// the transaction on the thread. The runner times the reads, never
    /// the opening.
    fn thread_reader(&self) -> Res<ReaderOpener>;

    /// What the engine did to get here, rather than how fast: the counts
    /// a mechanism is made of. Throughput over a whole pass is the noisy
    /// quantity -- a run of it cannot tell a 15% option apart from the
    /// machine -- and a count is exact, so `bench ab` reports these
    /// beside the timings and they are what a mechanism is judged on.
    /// Empty for an engine with nothing to say, which is every
    /// comparator: this is supdb's own instrumentation, not a fairness
    /// axis, and nothing compares an engine to another through it.
    fn counters(&self) -> Vec<(&'static str, f64)> {
        Vec::new()
    }

    /// What the engine holds in memory, by owner and by capacity: the
    /// bytes behind each structure that a bound on the process is spent
    /// in, which `counters` cannot say since it counts what the structures
    /// hold and not what they cost (`Reader::memory_census`). A name
    /// ending `_n` is a count and the rest are bytes. Walks every block
    /// the store has formed, so it is read at the lag sweep's points under
    /// `SUPDB_LAG_COUNTERS` and nowhere else. Empty for an engine with
    /// nothing to say.
    fn memory_census(&self) -> Vec<(String, u64)> {
        Vec::new()
    }

    /// Each form file's keys, megabytes and decodes, by partition in key
    /// order (`Db::zpage_heat`): where a mix's decodes land, printed after
    /// each mix under `SUPDB_MIX_HEAT`. Empty for an engine with no form.
    fn zpage_heat(&self) -> String {
        String::new()
    }

    /// The second form as the engine holds it, or `None` for an engine
    /// that has none. A pass asks between its timed phases whether a forced
    /// arm is what it says it is, and `counters` is not for that: it takes
    /// the writer's upkeep home, which would change the phase after it for
    /// every arm. This reads the reader table's slots and the live
    /// segments and takes nothing.
    fn zpages_state(&self) -> Option<ZState> {
        None
    }
}

/// What a reader thread reads through: the two reads of `Engine`, over a
/// handle the thread opened for itself and drops when it is done. Not
/// `Send`, because it never leaves that thread, which is what lets an
/// LMDB transaction sit in one.
pub trait ThreadReader {
    /// Bytes returned for the key; 0 for a miss.
    fn get(&mut self, key: &[u8]) -> Res<usize>;
    /// Bytes visited scanning `n` entries from `from`.
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize>;
}

/// Becomes a `ThreadReader` on the thread that calls it; see
/// `Engine::thread_reader`.
pub type ReaderOpener = Box<dyn FnOnce() -> Res<Box<dyn ThreadReader>> + Send>;

/// Every file under `p` at any depth, in the blocks the filesystem has
/// given them rather than their lengths: a file's length can run past what
/// was ever written to it, and a store that reserves a map it has not
/// filled would be charged for the reservation. Measured on the arms here
/// the two agree within a thousandth, so this is robustness against a
/// sparse file rather than a correction to one -- but `du` is what a user
/// checks, and `du` counts blocks.
fn dir_size(p: &Path) -> u64 {
    let mut seen = std::collections::HashSet::new();
    dir_size_into(p, &mut seen)
}

/// `dir_size` over one tree, each inode counted once however many names
/// it has: a direct run's segment is linked under its piece's name while
/// its temp name stays for recovery, and summing by name read the store
/// at twice its size for as long as both names stood.
fn dir_size_into(p: &Path, seen: &mut std::collections::HashSet<(u64, u64)>) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                total += dir_size_into(&e.path(), seen);
            } else if seen.insert((m.dev(), m.ino())) {
                total += m.blocks() * 512;
            }
        }
    } else if let Ok(m) = std::fs::metadata(p) {
        total = m.blocks() * 512;
    }
    total
}

// ------------------------------------------------------------------- next --

/// Supdb (`supdb::Db`): a WAL-only commit with sealed segments in
/// today's store format. Durable in the durable arms -- a commit is a WAL
/// append plus one fdatasync, which is LMDB's own boundary -- and buffered
/// in `supdb-ingest`, frames written per commit and fsynced at `sync`, the
/// boundary `lmdb-nosync` and `rocksdb-nosync` commit on. Scans pay the unrouted fan
/// (every segment contributes candidates) until range-partitioned compaction
/// lands; that cost is the arm's to show, not to hide.
pub struct Supdb {
    db: Option<supdb::Db>,
    path: PathBuf,
    /// False for the ingest-first arm: a flush stops partitioning what it
    /// sealed and leaves that to the background -- promotion at once, a
    /// merge for the rest (`Options::flush_schedules`). Both arms exist so
    /// the trade is measured in ONE interleaved run rather than compared
    /// across two, which is the whole reason this suite interleaves.
    partition: bool,
    /// Whether `sync` drains -- seals the last memtable and partitions what
    /// it sealed inside the load window -- or only makes the WAL durable and
    /// leaves the tail in memory, as RocksDB's `sync` does. Measured, the
    /// drain was 11% of the load window and the whole of the seal phase, so
    /// both shapes are arms.
    drain: bool,
    /// The block cache: scans over unsealed keys walk cached copies of the
    /// partition blocks they cross instead of merging every source, and a
    /// write is settled into the block it lands in. On in the default arm
    /// because the option is on by default; the arm that turns it off
    /// runs interleaved with it, which is the only way to price it, and
    /// with LMDB, whose in-place tree is what the cache is measured
    /// against on the scan mixes.
    block_cache: bool,
    /// A read advice pinned against the engine's own default, or `None` to
    /// take whatever the default is.
    ///
    /// `None` for the canonical arm, deliberately: the numbers this project
    /// quotes should describe what a user gets, so the arm follows the
    /// default rather than pinning a setting beside it. The contrast arm
    /// pins the kernel's plain readahead, and the two run interleaved --
    /// which is the only way to price this, since the three unchanged
    /// comparators in this suite once moved +20% to +43% between
    /// consecutive runs.
    advice: Option<supdb::ReadAdvice>,
    /// The block cache's budget in bytes, 0 for the engine's default of no
    /// bound. The default arm matches LMDB, whose cache is the page cache
    /// with no bound but the machine's; `supdb-cache256` matches
    /// `rocksdb-tuned`, whose block cache is 256 MB, so the pair is priced
    /// on the same memory.
    budget: usize,
    /// Whether a commit reaches the device before it returns: the axis the
    /// arm's guarantee row is about, set here rather than inherited. Every
    /// supdb arm took the engine's durable default for as long as none set
    /// it, `supdb-ingest` included, which sat in the buffered row.
    durable: bool,
    /// Whether the writer keeps the range-read structure current at each
    /// commit; `supdb-forms`.
    forms: bool,
    /// Whether the sorted unsealed keys are the state's, built once and
    /// carried forward, or each handle's own sorted afresh; `supdb-snap`.
    snap: bool,
    /// Whether a commit's maintenance stops after settling; `supdb-settle`.
    settle: bool,
    /// Whether the writer reads the forms it maintains; `supdb-wforms`.
    wforms: bool,
    /// The unsealed share past which a commit stops; `supdb-regime`.
    lagcap: usize,
    /// Whether the forms wait for a caller's handle; `supdb-lazyforms`.
    lazyforms: bool,
    /// The settle backlog this arm pins, or none for the engine's own;
    /// `supdb-nosettle` pins zero. `supdb-eager`.
    eager: Option<usize>,
    /// The recency window this arm pins, or none for the engine's own.
    /// `supdb-recency`.
    recent: Option<usize>,
    /// Whether the forms are carried across a seal; off is `supdb-nocarry`.
    carry: bool,
    /// Keys of one block in a settle's backlog from which the block is
    /// rebuilt rather than patched key by key. `supdb-rebuild`.
    rebuild: usize,
    /// The writer's scan snapshot carried across a publish rather than
    /// dropped. `supdb-snapcarry`.
    snapcarry: bool,
    /// A scan builds the snapshot only at a block that needs it rather
    /// than before it walks anything. `supdb-lazysnap`.
    lazysnap: bool,
    /// Every record written with its extent, as before the compact
    /// record. `supdb-fullrec`.
    fullrec: bool,
    /// A partition's copies dropped at a merge that rewrote it over the
    /// same keys, as before the rebase. `supdb-norebase`.
    norebase: bool,
    /// The seal sized by the partitions' file bytes rather than the key
    /// and value bytes they hold. `supdb-sealfile`.
    sealfile: bool,
    /// The settle bound taken on every segment's keys, level-0 pieces
    /// included, rather than the partitions'. `supdb-settleall`.
    settleall: bool,
    /// Who keeps the writer's upkeep, where not the engine's own choice:
    /// the writer itself, as before the thread. `supdb-inline`.
    upkeep: Option<supdb::Upkeep>,
    /// Every commit lends its batch to the upkeep thread, as before a
    /// commit with no fsync filed a burst's batch itself
    /// (`Options::upkeep_inline_bursts` off). `supdb-ingestlendbursts`,
    /// and `supdb-lendbursts` durable, where the commits sync and so
    /// lend either way.
    lendbursts: bool,
    /// A buffered commit near reads files any batch itself, however
    /// small (`Options::upkeep_inline_min` at one). `supdb-ingestinlineall`,
    /// and `supdb-inlineall` durable, where the commits sync and so lend
    /// either way.
    inlineall: bool,
    /// A buffered commit near reads leaves a small batch's settle to the
    /// next scan, as before (`Options::upkeep_settle_after_scans` off).
    /// `supdb-ingestlazysettle`, and `supdb-lazysettle` durable, where the
    /// commits sync and so lend either way.
    lazysettle: bool,
    /// A flush fsyncs the WAL before its seal whatever the policy, as
    /// before (`Options::drain_syncs_wal`). `supdb-ingestwalsync`, and
    /// `supdb-walsync` durable, where every commit synced already.
    walsync: bool,
    /// A drain's seal builds no snapshot copy and walks the table for
    /// its records (`Options::drain_snapshot` off), the refuted shape.
    /// `supdb-ingestnodrainsnap`, and `supdb-nodrainsnap` durable.
    nodrainsnap: bool,
    /// A seal writes every key through the writer's general path, as
    /// before the one-value path (`Options::seal_put_one` off).
    /// `supdb-ingestsealrun`, and `supdb-sealrun` durable.
    sealrun: bool,
    /// A flush's close composes the trailer on the writer's thread while
    /// a thread syncs the records (`Options::close_finish_inline`), the
    /// refuted shape. `supdb-ingestcloseinline`, and `supdb-closeinline`
    /// durable.
    closeinline: bool,
    /// The scan phases timed on every scan, as before the clock reads
    /// were gated (`Options::scan_timers`). `supdb-timers`, and
    /// `supdb-ingesttimers` buffered.
    timers: bool,
    /// A lend wakes the upkeep thread when it is parked past a short
    /// batch's wait (`Options::upkeep_wake_parked`). `supdb-wake`, and
    /// `supdb-ingestwake` buffered.
    wake: bool,
    /// The block walk issues no prefetches (`Options::scan_prefetch`
    /// off). `supdb-noprefetch`, and `supdb-ingestnoprefetch` buffered.
    noprefetch: bool,
    /// A scan's seek without the stride window's directory hint, and the
    /// record hint reading its span's two directory words
    /// (`Options::seek_prefetch_dir` off). `supdb-dirplain`, and
    /// `supdb-ingestdirplain` buffered.
    dirplain: bool,
    /// The commit's maintenance filing its backlog into the forms far
    /// from any scan (`Options::forms_settle_far`), the shape before a
    /// stretch nobody reads filed nothing. `supdb-settlefar`, and
    /// `supdb-ingestsettlefar` buffered.
    settlefar: bool,
    /// The block cache's forms counted, built and grown by what their
    /// buffers cost, with the budget kept where they grow, the retired
    /// snapshots and forms swept when an operation ends and the
    /// allocator's free pages given back (`Options::forms_to_fit` and the
    /// three options beside it). `supdb-tofit`, `supdb-ingesttofit`
    /// buffered, and `supdb-cache256tofit` beside the 256 MB budget.
    tofit: bool,
    /// With `tofit`, the trim that gives the allocator's free pages back
    /// off (`Options::forms_trim_bytes` zero): against `tofit` alone it
    /// prices what the trim returns. `supdb-cache256tofitnotrim`.
    notrim: bool,
    /// With `tofit`, the budget not kept after a patch
    /// (`Options::forms_fit_shed` off): against `tofit` alone it prices
    /// what keeping it where the forms grow returns.
    /// `supdb-cache256tofitnoshed`.
    noshed: bool,
    /// With `tofit`, the retired snapshots and forms not swept when an
    /// operation ends (`Options::forms_fit_sweep` off): against `tofit`
    /// alone it prices what the sweep returns.
    /// `supdb-cache256tofitnosweep`.
    nosweep: bool,
    /// A walk's prefetch reaching a kilobyte into the records rather than
    /// four (`Options::scan_prefetch_bytes`). `supdb-prefetch1k`, and
    /// `supdb-ingestprefetch1k` buffered.
    prefetch1k: bool,
    /// The manifest published by a temp file, a rename and a directory
    /// fsync, as before the two slots (`Options::manifest_slots` off).
    /// `supdb-manifestrename`, and `supdb-ingestmanifestrename` buffered.
    manifestrename: bool,
    /// A seal's manifest slot written at its readable landing, naming its
    /// segments pending (`Options::manifest_pending`). `supdb-manifestpending`,
    /// and `supdb-ingestmanifestpending` buffered.
    manifestpending: bool,
    /// A flush waiting for its manifest slot's fdatasync, the shape before
    /// `Options::flush_waits_manifest` was off. `supdb-waitmanifest`, and
    /// `supdb-ingestwaitmanifest` buffered.
    waitmanifest: bool,
    /// The writer's buffers populated a window ahead of their fill
    /// (`Options::prefault`). `supdb-populate`, and `supdb-ingestpopulate`
    /// buffered.
    populate: bool,
    /// Segments written in 256 KB pieces (`SegmentOptions::write_piece`),
    /// a quarter of the default's fresh pages for the piece buffer.
    /// `supdb-piece256`, and `supdb-ingestpiece256` buffered.
    piece256: bool,
    /// A direct run's records written by `begin`, `value` and `end`, as
    /// before `Options::direct_put_one`. `supdb-recordsteps`, and
    /// `supdb-ingestrecordsteps` buffered.
    recordsteps: bool,
    /// A direct run's table holding a copy of each value, re-encoded at
    /// the commit, the shape before `Options::direct_stream`.
    /// `supdb-copyvals`, and `supdb-ingestcopyvals` buffered.
    copyvals: bool,
    /// A direct run's segment writer keeping its own copy of every key
    /// for the key section, the ordered index and the Bloom, the shape
    /// before `Options::direct_derive_keys`. `supdb-keycopies`, and
    /// `supdb-ingestkeycopies` buffered.
    keycopies: bool,
    /// The finish's slot and Bloom fills placing each key as it is asked
    /// for, the shape before `SegmentOptions::lookahead`. `supdb-fillplain`,
    /// and `supdb-ingestfillplain` buffered.
    fillplain: bool,
    /// A flush's close with the one fsync at the end, the shape before
    /// `Options::close_presync`. `supdb-latesync`, and
    /// `supdb-ingestlatesync` buffered.
    latesync: bool,
    /// A direct run's record stream kept whole in the arena, the shape
    /// before `Options::direct_recycle`. `supdb-norecycle`, and
    /// `supdb-ingestnorecycle` buffered.
    norecycle: bool,
    /// A streamed table keeping a copy of every key, the shape before
    /// `Options::direct_keys_in_stream`. `supdb-keysarena`, and
    /// `supdb-ingestkeysarena` buffered.
    keysarena: bool,
    /// The upkeep thread's back-off as it was, climbing from its first
    /// park, the shape before `Options::upkeep_alert`. `supdb-noalert`,
    /// and `supdb-ingestnoalert` buffered.
    noalert: bool,
    /// An ordered table keeping a write log as a hash table does, the
    /// shape before `Options::direct_log_range`. `supdb-ordlog`, and
    /// `supdb-ingestordlog` buffered.
    ordlog: bool,
    /// An ordered table's entries twenty-four bytes each, the shape
    /// before `Options::direct_slim_entries`. `supdb-fatentries`, and
    /// `supdb-ingestfatentries` buffered.
    fatentries: bool,
    /// Aligned pieces over a range at which they are merged into one
    /// piece, or none for the engine's own, which leaves them for the
    /// partition merge. `supdb-tier`.
    tier: Option<usize>,
    /// The scan snapshot carries each unsealed key's chain, and a block
    /// the runs cover is walked rather than copied. `supdb-runs`.
    runs: bool,
    /// The run keeper beside the runs: a thread at idle priority keeps
    /// the published snapshot current from the commits. `supdb-keeper`.
    keeper: bool,
    /// Whether a publish starts the builder and a commit installs its
    /// forms. `supdb-aheadpub`.
    aheadpub: bool,
    /// Whether the forms are published at every maintained commit, as
    /// they were before they waited for a handle. `supdb-pubalways`.
    pubalways: bool,
    /// A flush that does not partition leaves its pieces as they are,
    /// as before `Options::flush_schedules`. `supdb-ingestleave`.
    leave: bool,
    /// The partitions' blocks below which no builder starts, or none for
    /// the engine's own. `supdb-ahead`.
    aheadmin: Option<usize>,
    /// The seal cap this arm pins, or none for the engine's own;
    /// `supdb-noseal` pins zero. `Option` and not a number, because the
    /// shipping arm must inherit a default rather than restate it.
    sealcap: Option<usize>,
    /// The merge trigger this arm pins, or none for the engine's own.
    /// `supdb-l0`.
    l0: Option<usize>,
    /// The writer's operations pin nothing and hold no state, as before
    /// `Options::writer_pins`. `supdb-nopin`.
    nopin: bool,
    /// The sweep fences for the pins and the pin does not
    /// (`Options::asym_pins`). `supdb-asympin`.
    asympin: bool,
    /// A point read takes a segment's extents by value, as before
    /// `Blob::with_lookup` (`Options::exts_by_value`). `supdb-extsval`.
    extsval: bool,
    /// The forms hold the pieces' fold alone and every walk overlays the
    /// snapshot (`Options::forms_pieces_only`). `supdb-piecesonly`, and
    /// `supdb-ingestpiecesonly` buffered.
    piecesonly: bool,
    /// The segment writer leaves every piece dirty and unsynced until the
    /// closing fsync, as before `SegmentOptions::early_writeback` and
    /// `sync_ahead`. `supdb-latewb`, and `supdb-ingestlatewb` buffered.
    latewb: bool,
    /// The segment work driven inline by the writer, as before
    /// `Options::publish_in_background`. `supdb-inlinemaint`.
    inlinemaint: bool,
    /// A scan and a settle search a copy for their key, as before
    /// `Options::copy_start_at_rank`. `supdb-copysearch`, and
    /// `supdb-ingestcopysearch` buffered.
    copysearch: bool,
    /// A dense sparse form converted by a build from the sources, as
    /// before `Options::convert_from_form`. `supdb-convertsources`, and
    /// `supdb-ingestconvertsources` buffered.
    convertsources: bool,
    /// A dense form's copy sized with a record's worth of room per record,
    /// as before `Options::copy_exact`. `supdb-copyslack`, and
    /// `supdb-ingestcopyslack` buffered.
    copyslack: bool,
    /// A drain landing its lone piece and promoting it under a second
    /// manifest, as before `Options::drain_names_partition`:
    /// `supdb-ingestdrainpromote`.
    drainpromote: bool,
    /// A manifest publish that renames over the old manifest and frees it
    /// on the path, as before `Options::manifest_spare`. `supdb-manifestfree`,
    /// and `supdb-ingestmanifestfree` buffered.
    manifestfree: bool,
    /// The ordered index fsynced where it is written, as before
    /// `Options::ord_durable`. `supdb-oidxsync`, and `supdb-ingestoidxsync`
    /// buffered.
    oidxsync: bool,
    /// A direct run's table holding references into its segment in place
    /// of its values (`Options::direct_table_refs`). `supdb-refvals`, and
    /// `supdb-ingestrefvals` buffered.
    refvals: bool,
    /// The key section's checksum row hashed at the close from the file,
    /// as before `SegmentOptions::row_on_write`. `supdb-rowatclose`, and
    /// `supdb-ingestrowatclose` buffered.
    rowatclose: bool,
    /// A piece's keys ranked over the partition's records, as before
    /// `Options::piece_ranks_by_heads`. `supdb-rankrecords`, and
    /// `supdb-ingestrankrecords` buffered.
    rankrecords: bool,
    /// A store with no partitions partitioned once it is read,
    /// `Options::adaptive_shape`. `supdb-ingestshape`.
    shape: bool,
    adaptcap: Option<usize>,
    adapttrig: bool,
    rotate: bool,
    defer: bool,
    firstfloor: bool,
    nofrozen: bool,
    idle: bool,
    sortover: bool,
    /// Segments written in 2 MB pieces, `SegmentOptions::write_piece`.
    /// `supdb-pmd`, and `supdb-ingestpmd` buffered.
    pmd: bool,
    /// The freeze settles the backlog and carries the tables itself,
    /// `Options::freeze_settles` on, as it did before the carry was left
    /// to the look. `supdb-settlefreeze`, and `supdb-ingestsettlefreeze`
    /// buffered.
    settlefreeze: bool,
    /// Dense forms' conversions capped at a fill, the builder ahead of a
    /// reader converting the rest (`Options::forms_convert_cap` at
    /// `CONVERT_CAP`, `Options::ahead_converts` on). `supdb-convertcap`,
    /// and `supdb-ingestconvertcap` buffered.
    convertcap: bool,
    /// The upkeep thread's pass bounded at `SHORT_PASS_PCT` of the
    /// store's keys (`Options::upkeep_pass_pct`). `supdb-shortpass`, and
    /// `supdb-ingestshortpass` buffered.
    shortpass: bool,
    /// The upkeep thread's fill converting every dense form it meets
    /// rather than yielding to a newer commit (`Options::forms_convert_yield`
    /// off), the shape before. `supdb-noyield`, and `supdb-ingestnoyield`
    /// buffered.
    noyield: bool,
    /// The fill yielding its conversions at any posted commit
    /// (`Options::forms_convert_behind` zero), the shape before it
    /// converted in the thread's slack. `supdb-postedyield`, and
    /// `supdb-ingestpostedyield` buffered.
    postedyield: bool,
    /// The fill's conversions yielding to a posted commit once the thread
    /// is a thousand writes behind (`Options::forms_convert_behind`), as
    /// the option's first default meant. `supdb-behindyield`, and
    /// `supdb-ingestbehindyield` buffered.
    behindyield: bool,
    /// The background work as the cost model in `docs/engine.md` orders
    /// it: no dense form rebuilt while nothing reads
    /// (`Options::forms_convert_unread` off). `supdb-bg`, and
    /// `supdb-ingestbg` buffered.
    bg: bool,
    /// `bg` with the thread's lag bounded and its passes batched
    /// (`Options::upkeep_lag_pct`, `Options::upkeep_batch_pct`):
    /// `supdb-bglag`, and `supdb-ingestbglag` buffered.
    bglag: bool,
    /// A point read of a partition through its ordered index rather than
    /// its hash (`Options::point_via_ord`). `supdb-nocacheordread`, the
    /// nocache policy alone: the form is for a store that does not fit.
    ordread: bool,
    /// Every partition's second form forced on (`Options::zpages` at
    /// `ZPages::Always`), and a point read or a count of a partition read
    /// from it: `supdb-nocachezpages`, the nocache policy alone, the form
    /// being for a store that does not fit.
    zpages: bool,
    /// The build of the second form leaves the raw records it passed in the
    /// page cache (`Options::zpages_drop_behind` off): the pair
    /// `supdb-nocachezpagesnodrop` against `supdb-nocachezpages` prices the
    /// drop, which is a share of the build's elapsed time.
    zpages_nodrop: bool,
    /// The warm of a form file after its attach off
    /// (`Options::zpages_warm` off): the pair `supdb-nocachezpagesnowarm`
    /// against `supdb-nocachezpages` prices the warm, which a restart with
    /// cold form files pays before its first read.
    zpages_nowarm: bool,
    /// The memo of decoded groups off (`Options::zpages_memo` off): the
    /// pair `supdb-nocachezpagesnomemo` against `supdb-nocachezpages`
    /// prices the memo, a decode saved on every read of a page the thread
    /// decoded before.
    zpages_nomemo: bool,
    /// A scan's seek of a covered partition by the ordered index's heads
    /// (`Options::zpages_seek_ord` on): the pair `supdb-nocachezpagesheads`
    /// against `supdb-nocachezpages` prices the seek by the form file's
    /// fence and one group against the one by the heads and the raw
    /// records.
    zpages_heads: bool,
    /// A scan's prefetch of the pages it will cross after its first
    /// (`Options::zpages_scan_prefetch` on): the pair
    /// `supdb-nocachezpagesprefetch` against `supdb-nocachezpages` prices
    /// it, out of core on the scan cells and in core where it is a syscall
    /// for nothing.
    zpages_prefetch: bool,
    /// The overlay walk of a covered partition cut by rank, as it was
    /// before the cursor (`Options::zpages_walk_cursor` off): the pair
    /// `supdb-nocachezpagesrankwalk` against `supdb-nocachezpages` prices
    /// the cursor's one open a group against the seek, the fresh walk and
    /// the read at every unsealed key, on the E mix and the scan passes.
    zpages_rankwalk: bool,
    /// Every build left to the settle of the form rather than started as
    /// its partition lands (`Options::zpages_eager` off):
    /// `supdb-nocachezpagessettle`, the shape the forced arm's rows were
    /// first taken with.
    zpages_settle: bool,
    /// The second form the controller's to choose (`Options::zpages` at
    /// `ZPages::Auto`): `supdb-autozpages` beside `supdb`, and
    /// `supdb-nocacheautozpages` beside `supdb-nocache`, each the base
    /// with the controller and nothing else, so the pair prices what the
    /// controller costs in memory, where it must build nothing, and what
    /// it buys out of core, where it must reach the form.
    zpages_auto: bool,
    /// The controller's bounded arm with the form's threads at the
    /// readers' I/O class (`Options::zpages_io_idle` off):
    /// `supdb-nocacheautozpagesfairio`, the pair against
    /// `supdb-nocacheautozpages` under a bound.
    zpages_fairio: bool,
    /// The controller's bounded arm with its builds asking nothing of the
    /// raw records ahead of their walks (`Options::zpages_build_ahead` at
    /// zero, the shape the window was priced against):
    /// `supdb-nocacheautozpagesnoahead`, the pair against
    /// `supdb-nocacheautozpages` under a bound.
    zpages_noahead: bool,
    /// The controller's bounded arm that buys its first form at SUSPECT on
    /// the model's prediction (`Options::zpages_build_eager`):
    /// `supdb-nocacheautozpageseager`, the pair against
    /// `supdb-nocacheautozpages` under a bound.
    zpages_buildeager: bool,
    /// The block cache beside the controller's form with the cache's reads
    /// of a partition through the door (`Options::zpages_block_cache` off),
    /// the shape whose walks evicted the form under a bound:
    /// `supdb-cache64tofitautozpagesraw`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zcacheraw: bool,
    /// The coding the selector's to choose (`Options::zpages_coding_auto`):
    /// `supdb-cache64tofitautozpagescodingauto` against
    /// `supdb-cache64tofitautozpages`.
    zcodingauto: bool,
    /// The combined arm with a block read this many entries' worth held
    /// as a copy in its slot under the form (`Options::promote_entries`
    /// with `zpages_promote_copies`), zero for neither:
    /// `supdb-cache64tofitautozpagespromote`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zpromote: usize,
    /// The combined arm with the form's page cut into groups at this many
    /// bytes of records (`Options::zpages_group_bytes`), `None` for the
    /// engine's default: `supdb-cache64tofitautozpagesonegroup` at zero,
    /// the page as one group, and `supdb-cache64tofitautozpagesgroup1k` at
    /// a kibibyte, each the pair against `supdb-cache64tofitautozpages`.
    zgroup: Option<usize>,
    /// The combined arm with the form files built ahead of the
    /// controller's decision once the store outgrows its budget
    /// (`Options::zpages_prebuild`): `supdb-cache64tofitautozpagesprebuild`,
    /// the pair against `supdb-cache64tofitautozpages`.
    zprebuild: bool,
    /// The combined arm with a block-cache scan's block taken from the
    /// fence and the directory and its rank sought only for a form walked
    /// by rank (`Options::zpages_seek_span`):
    /// `supdb-cache64tofitautozpagesspan`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zspan: bool,
    /// The combined arm with the memo's slot the group's number plus a
    /// multiple of the identity (`Options::zpages_memo_hash` off):
    /// `supdb-cache64tofitautozpagesmemolinear`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zmemolinear: bool,
    /// The combined arm with a settle resolving every written key's cut
    /// (`Options::zpages_settle_span` off):
    /// `supdb-cache64tofitautozpagesexactcut`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zexactcut: bool,
    /// Every view of a form file's page checked and the memo asked after it
    /// (`Options::zpages_trusted_views` off): the combined arm's
    /// `supdb-cache64tofitautozpagescheckedviews`, the pair against
    /// `supdb-cache64tofitautozpages`, and the forced arm's
    /// `supdb-nocachezpagescheckedviews`, the pair against
    /// `supdb-nocachezpages`, which prices the views by instruction in core.
    zcheckedviews: bool,
    /// The form file's groups holding their records one by one
    /// (`Options::zpages_record_coding` on): `supdb-cache64tofitautozpagesrecord`
    /// and the forced `supdb-nocachezpagesrecord`, each the pair against the
    /// arm without it.
    zrecord: bool,
    /// The form file's groups stored verbatim, no LZ4 (`Options::zpages_store_all`):
    /// `supdb-nocachezpagesstored`, and `supdb-cache64tofitzpagesstored`
    /// against `supdb-cache64tofitzpages`, the forced form with the block
    /// cache.
    zstored: bool,
    /// The record coding with every tail stored raw (`Options::zpages_raw_tails`
    /// with `zpages_record_coding`): `supdb-nocachezpagesrecordraw`, and
    /// `supdb-cache64tofitzpagesrecordraw` against `supdb-cache64tofitzpages`.
    zrawtails: bool,
    /// The allocator trimmed once after the form files' install
    /// (`Options::zpages_trim_install`); with the memo off and the raw
    /// tails, the fit check of the raw-tail form at five million keys under
    /// 640 MiB, `supdb-cache64tofitzpagesrecordrawlean` against
    /// `supdb-cache64tofitzpages`.
    ztrim: bool,
    /// The raw-tail form's groups decoded whole for a scan instead of read
    /// in place (`Options::zpages_inplace_walk` off), the shape before the
    /// in-place walk: `supdb-cache64tofitzpagesrecordrawdecode` against
    /// `supdb-cache64tofitzpagesrecordraw`.
    zdecodewalk: bool,
    /// The trim thread asked to look at every upkeep pass end
    /// (`Options::upkeep_trim_look`); with the lean arm's two levers, the
    /// second fit check of the raw-tail form,
    /// `supdb-cache64tofitzpagesrecordrawleantrim` against
    /// `supdb-cache64tofitzpages`.
    ztrimpass: bool,
    /// The trim thread counting its stride by the resident bytes beyond
    /// the allocator's use (`Options::trim_by_resident`), the stride eight
    /// megabytes; with the lean arm's levers and the look, the third fit
    /// check of the raw-tail form,
    /// `supdb-cache64tofitzpagesrecordrawleanresident` against
    /// `supdb-cache64tofitzpages`.
    ztrimresident: bool,
    /// The first this many partitions by key order raw-tailed and the rest
    /// block-coded (`Options::zpages_raw_tails_first`): the upper bound of a
    /// coding chosen per partition by its heat,
    /// `supdb-cache64tofitzpageshot4` against `supdb-cache64tofitzpages`.
    zhotfirst: usize,
    /// A partition's raw file and ordered index dropped from the page cache
    /// once its form file serves it (`Options::zpages_drop_raw`):
    /// `supdb-cache64tofitzpagesdrop` and `supdb-cache64tofitzpageshot4drop`
    /// against the same without.
    zdropraw: bool,
    /// Its partitions re-coded while served by what their decodes cost
    /// (`Options::zpages_recode`): `supdb-cache64tofitzpagesrecodedrop`
    /// against `supdb-cache64tofitzpagesdrop` and the key-order arms.
    zrecode: bool,
    /// The form file a re-code replaced kept beside the served one, so the
    /// partition's later re-codes are the two exchanged
    /// (`Options::zpages_recode_keep`):
    /// `supdb-cache64tofitzpagesrecodekeepdrop` against
    /// `supdb-cache64tofitzpagesrecodedrop`.
    zrecodekeep: bool,
    /// Every commit near reads settling its batch into the forms
    /// (`Options::forms_settle_every`): `supdb-cache64tofitzpageshot8dropsettleevery`
    /// against `supdb-cache64tofitzpageshot8drop`.
    settleevery: bool,
    /// And the commit's fill building a form for every block its settle
    /// filed into that held none (`Options::forms_settle_builds`):
    /// `supdb-cache64tofitzpageshot8dropsettlebuilds`.
    settlebuilds: bool,
    /// before any view (`Options::zpages_memo_first`):
    /// `supdb-cache64tofitautozpagesmemofirst`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zmemofirst: bool,
    /// The combined arm with the form file's fence searched through a tree
    /// over the entries' words (`Options::zpages_fence_tree` on):
    /// `supdb-cache64tofitautozpagesfencetree`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zfencetree: bool,
    /// The combined arm with a decode asking nothing ahead for the memo
    /// entry its put displaces (`Options::zpages_memo_ahead` off):
    /// `supdb-cache64tofitautozpagesmemonoahead`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zmemonoahead: bool,
    /// The combined arm with a scan's start key kept with its block
    /// (`Options::scan_start_cache` at 4,096 entries, a repeated start
    /// skipping the seek): `supdb-cache64tofitautozpagesstartcache`, the
    /// pair against `supdb-cache64tofitautozpages`, which seeks every start.
    zstartcache: bool,
    /// The combined arm with a block read through a cursor a run, a seek a
    /// cut and a read by rank a replaced record
    /// (`Options::zpages_block_cursor` off): `supdb-cache64tofitautozpagesperrun`,
    /// the pair against `supdb-cache64tofitautozpages`.
    zperrun: bool,
    /// The combined arm with the memo of decoded groups capped at this many
    /// mebibytes instead of the engine's eight (`Options::zpages_memo_bytes`):
    /// `supdb-cache64tofitautozpagesmemo4`, `...memo16`, `...memo32`, each
    /// the pair against `supdb-cache64tofitautozpages`. Zero is the engine's
    /// cap.
    zmemo_mib: usize,
    /// What the reader handles this store gave its threads answered
    /// through the ordered index, summed as each is dropped
    /// (`OrdTally`); only the arm that reads that way hands out handles
    /// that sum.
    ord_threads: std::sync::Arc<OrdTally>,
}

/// The ordered-index point reads of the reader handles a threaded pass
/// hands its threads. A handle's own count (`supdb::Reader::ord_point_reads`)
/// dies with it, and a pass drops its handles before the store's counters
/// are read, so the threaded cells of a pairing would print nothing of
/// whether they took the path the single-threaded cell did.
#[derive(Default)]
struct OrdTally {
    served: std::sync::atomic::AtomicU64,
    declined: std::sync::atomic::AtomicU64,
}

/// A thread's reader on the arm that reads through the ordered index: the
/// handle, and its counts handed to the store's tally when it is dropped,
/// which is after the thread's clock has stopped and not once per read.
struct TallyReader {
    r: supdb::Reader,
    tally: std::sync::Arc<OrdTally>,
}

impl Drop for TallyReader {
    fn drop(&mut self) {
        let (served, declined) = self.r.ord_point_reads();
        let relaxed = std::sync::atomic::Ordering::Relaxed;
        self.tally.served.fetch_add(served, relaxed);
        self.tally.declined.fetch_add(declined, relaxed);
    }
}

/// What an arm differs from `supdb` by. One struct rather than a row of
/// positional flags, which reached seven and could not take the eighth.
struct Policy {
    partition: bool,
    drain: bool,
    advice: Option<supdb::ReadAdvice>,
    block_cache: bool,
    durable: bool,
    budget: usize,
    forms: bool,
    snap: bool,
    /// Maintain at every commit but settle only: no form built for an
    /// overlaid block, nothing published. `supdb-settle`.
    settle: bool,
    /// The writer's own handle reads the forms it maintains rather than
    /// building its own. `supdb-wforms`.
    wforms: bool,
    /// The share of the store that may be unsealed before a commit
    /// stops maintaining; zero is no bound. `supdb-regime`.
    lagcap: usize,
    /// Whether the forms wait for a caller's handle to have scanned, as
    /// they did before the seal cap. `supdb-lazyforms`.
    lazyforms: bool,
    /// The settle backlog this arm pins, or none for the engine's own.
    eager: Option<usize>,
    /// The recency window this arm pins, or none for the engine's own.
    /// `supdb-recency`.
    recent: Option<usize>,
    /// Whether the forms are carried across a seal; off is `supdb-nocarry`.
    carry: bool,
    /// Keys of one block in a settle's backlog from which the block is
    /// rebuilt rather than patched key by key. `supdb-rebuild`.
    rebuild: usize,
    /// The writer's scan snapshot carried across a publish rather than
    /// dropped. `supdb-snapcarry`.
    snapcarry: bool,
    /// A scan builds the snapshot only at a block that needs it rather
    /// than before it walks anything. `supdb-lazysnap`.
    lazysnap: bool,
    /// Every record written with its extent, as before the compact
    /// record. `supdb-fullrec`.
    fullrec: bool,
    /// A partition's copies dropped at a merge that rewrote it over the
    /// same keys, as before the rebase. `supdb-norebase`.
    norebase: bool,
    /// The seal sized by the partitions' file bytes rather than the key
    /// and value bytes they hold. `supdb-sealfile`.
    sealfile: bool,
    /// The settle bound taken on every segment's keys, level-0 pieces
    /// included, rather than the partitions'. `supdb-settleall`.
    settleall: bool,
    /// Who keeps the writer's upkeep, where not the engine's own choice:
    /// the writer itself, as before the thread. `supdb-inline`.
    upkeep: Option<supdb::Upkeep>,
    /// Every commit lends its batch to the upkeep thread, as before a
    /// commit with no fsync filed a burst's batch itself
    /// (`Options::upkeep_inline_bursts` off). `supdb-ingestlendbursts`,
    /// and `supdb-lendbursts` durable, where the commits sync and so
    /// lend either way.
    lendbursts: bool,
    /// A buffered commit near reads files any batch itself, however
    /// small (`Options::upkeep_inline_min` at one). `supdb-ingestinlineall`,
    /// and `supdb-inlineall` durable, where the commits sync and so lend
    /// either way.
    inlineall: bool,
    /// A buffered commit near reads leaves a small batch's settle to the
    /// next scan, as before (`Options::upkeep_settle_after_scans` off).
    /// `supdb-ingestlazysettle`, and `supdb-lazysettle` durable, where the
    /// commits sync and so lend either way.
    lazysettle: bool,
    /// A flush fsyncs the WAL before its seal whatever the policy, as
    /// before (`Options::drain_syncs_wal`). `supdb-ingestwalsync`, and
    /// `supdb-walsync` durable, where every commit synced already.
    walsync: bool,
    /// A drain's seal builds no snapshot copy and walks the table for
    /// its records (`Options::drain_snapshot` off), the refuted shape.
    /// `supdb-ingestnodrainsnap`, and `supdb-nodrainsnap` durable.
    nodrainsnap: bool,
    /// A seal writes every key through the writer's general path, as
    /// before the one-value path (`Options::seal_put_one` off).
    /// `supdb-ingestsealrun`, and `supdb-sealrun` durable.
    sealrun: bool,
    /// A flush's close composes the trailer on the writer's thread while
    /// a thread syncs the records (`Options::close_finish_inline`), the
    /// refuted shape. `supdb-ingestcloseinline`, and `supdb-closeinline`
    /// durable.
    closeinline: bool,
    /// The scan phases timed on every scan, as before the clock reads
    /// were gated (`Options::scan_timers`). `supdb-timers`, and
    /// `supdb-ingesttimers` buffered.
    timers: bool,
    /// A lend wakes the upkeep thread when it is parked past a short
    /// batch's wait (`Options::upkeep_wake_parked`). `supdb-wake`, and
    /// `supdb-ingestwake` buffered.
    wake: bool,
    /// The block walk issues no prefetches (`Options::scan_prefetch`
    /// off). `supdb-noprefetch`, and `supdb-ingestnoprefetch` buffered.
    noprefetch: bool,
    /// A scan's seek without the stride window's directory hint, and the
    /// record hint reading its span's two directory words
    /// (`Options::seek_prefetch_dir` off). `supdb-dirplain`, and
    /// `supdb-ingestdirplain` buffered.
    dirplain: bool,
    /// The commit's maintenance filing its backlog into the forms far
    /// from any scan (`Options::forms_settle_far`), the shape before a
    /// stretch nobody reads filed nothing. `supdb-settlefar`, and
    /// `supdb-ingestsettlefar` buffered.
    settlefar: bool,
    /// The block cache's forms counted, built and grown by what their
    /// buffers cost, with the budget kept where they grow, the retired
    /// snapshots and forms swept when an operation ends and the
    /// allocator's free pages given back (`Options::forms_to_fit` and the
    /// three options beside it). `supdb-tofit`, `supdb-ingesttofit`
    /// buffered, and `supdb-cache256tofit` beside the 256 MB budget.
    tofit: bool,
    /// With `tofit`, the trim that gives the allocator's free pages back
    /// off (`Options::forms_trim_bytes` zero): against `tofit` alone it
    /// prices what the trim returns. `supdb-cache256tofitnotrim`.
    notrim: bool,
    /// With `tofit`, the budget not kept after a patch
    /// (`Options::forms_fit_shed` off): against `tofit` alone it prices
    /// what keeping it where the forms grow returns.
    /// `supdb-cache256tofitnoshed`.
    noshed: bool,
    /// With `tofit`, the retired snapshots and forms not swept when an
    /// operation ends (`Options::forms_fit_sweep` off): against `tofit`
    /// alone it prices what the sweep returns.
    /// `supdb-cache256tofitnosweep`.
    nosweep: bool,
    /// A walk's prefetch reaching a kilobyte into the records rather than
    /// four (`Options::scan_prefetch_bytes`). `supdb-prefetch1k`, and
    /// `supdb-ingestprefetch1k` buffered.
    prefetch1k: bool,
    /// The manifest published by a temp file, a rename and a directory
    /// fsync, as before the two slots (`Options::manifest_slots` off).
    /// `supdb-manifestrename`, and `supdb-ingestmanifestrename` buffered.
    manifestrename: bool,
    /// A seal's manifest slot written at its readable landing, naming its
    /// segments pending (`Options::manifest_pending`). `supdb-manifestpending`,
    /// and `supdb-ingestmanifestpending` buffered.
    manifestpending: bool,
    /// A flush waiting for its manifest slot's fdatasync, the shape before
    /// `Options::flush_waits_manifest` was off. `supdb-waitmanifest`, and
    /// `supdb-ingestwaitmanifest` buffered.
    waitmanifest: bool,
    /// The writer's buffers populated a window ahead of their fill
    /// (`Options::prefault`). `supdb-populate`, and `supdb-ingestpopulate`
    /// buffered.
    populate: bool,
    /// Segments written in 256 KB pieces (`SegmentOptions::write_piece`),
    /// a quarter of the default's fresh pages for the piece buffer.
    /// `supdb-piece256`, and `supdb-ingestpiece256` buffered.
    piece256: bool,
    /// A direct run's records written by `begin`, `value` and `end`, as
    /// before `Options::direct_put_one`. `supdb-recordsteps`, and
    /// `supdb-ingestrecordsteps` buffered.
    recordsteps: bool,
    /// A direct run's table holding a copy of each value, re-encoded at
    /// the commit, the shape before `Options::direct_stream`.
    /// `supdb-copyvals`, and `supdb-ingestcopyvals` buffered.
    copyvals: bool,
    /// A direct run's segment writer keeping its own copy of every key
    /// for the key section, the ordered index and the Bloom, the shape
    /// before `Options::direct_derive_keys`. `supdb-keycopies`, and
    /// `supdb-ingestkeycopies` buffered.
    keycopies: bool,
    /// The finish's slot and Bloom fills placing each key as it is asked
    /// for, the shape before `SegmentOptions::lookahead`. `supdb-fillplain`,
    /// and `supdb-ingestfillplain` buffered.
    fillplain: bool,
    /// A flush's close with the one fsync at the end, the shape before
    /// `Options::close_presync`. `supdb-latesync`, and
    /// `supdb-ingestlatesync` buffered.
    latesync: bool,
    /// A direct run's record stream kept whole in the arena, the shape
    /// before `Options::direct_recycle`. `supdb-norecycle`, and
    /// `supdb-ingestnorecycle` buffered.
    norecycle: bool,
    /// A streamed table keeping a copy of every key, the shape before
    /// `Options::direct_keys_in_stream`. `supdb-keysarena`, and
    /// `supdb-ingestkeysarena` buffered.
    keysarena: bool,
    /// The upkeep thread's back-off as it was, climbing from its first
    /// park, the shape before `Options::upkeep_alert`. `supdb-noalert`,
    /// and `supdb-ingestnoalert` buffered.
    noalert: bool,
    /// An ordered table keeping a write log as a hash table does, the
    /// shape before `Options::direct_log_range`. `supdb-ordlog`, and
    /// `supdb-ingestordlog` buffered.
    ordlog: bool,
    /// An ordered table's entries twenty-four bytes each, the shape
    /// before `Options::direct_slim_entries`. `supdb-fatentries`, and
    /// `supdb-ingestfatentries` buffered.
    fatentries: bool,
    /// Aligned pieces over a range at which they are merged into one
    /// piece, or none for the engine's own, which leaves them for the
    /// partition merge. `supdb-tier`.
    tier: Option<usize>,
    /// The scan snapshot carries each unsealed key's chain, and a block
    /// the runs cover is walked rather than copied. `supdb-runs`.
    runs: bool,
    /// The run keeper beside the runs: a thread at idle priority keeps
    /// the published snapshot current from the commits. `supdb-keeper`.
    keeper: bool,
    /// Whether a publish starts the builder and a commit installs its
    /// forms. `supdb-aheadpub`.
    aheadpub: bool,
    /// Whether the forms are published at every maintained commit, as
    /// they were before they waited for a handle. `supdb-pubalways`.
    pubalways: bool,
    /// A flush that does not partition leaves its pieces as they are,
    /// as before `Options::flush_schedules`. `supdb-ingestleave`.
    leave: bool,
    /// The builder's minimum blocks, or none for the engine's own.
    /// `supdb-ahead`.
    aheadmin: Option<usize>,
    /// The seal cap this arm pins, or none for the engine's own.
    /// `supdb-noseal`.
    sealcap: Option<usize>,
    /// The merge trigger this arm pins, or none for the engine's own.
    /// `supdb-l0`.
    l0: Option<usize>,
    /// The writer's operations pin nothing and hold no state, as before
    /// `Options::writer_pins`. `supdb-nopin`.
    nopin: bool,
    /// The sweep fences for the pins and the pin does not
    /// (`Options::asym_pins`). `supdb-asympin`.
    asympin: bool,
    /// A point read takes a segment's extents by value, as before
    /// `Blob::with_lookup` (`Options::exts_by_value`). `supdb-extsval`.
    extsval: bool,
    /// The forms hold the pieces' fold alone and every walk overlays the
    /// snapshot (`Options::forms_pieces_only`). `supdb-piecesonly`, and
    /// `supdb-ingestpiecesonly` buffered.
    piecesonly: bool,
    /// The segment writer leaves every piece dirty and unsynced until the
    /// closing fsync, as before `SegmentOptions::early_writeback` and
    /// `sync_ahead`. `supdb-latewb`, and `supdb-ingestlatewb` buffered.
    latewb: bool,
    /// The segment work driven inline by the writer, as before
    /// `Options::publish_in_background`. `supdb-inlinemaint`.
    inlinemaint: bool,
    /// A scan and a settle search a copy for their key, as before
    /// `Options::copy_start_at_rank`. `supdb-copysearch`, and
    /// `supdb-ingestcopysearch` buffered.
    copysearch: bool,
    /// A dense sparse form converted by a build from the sources, as
    /// before `Options::convert_from_form`. `supdb-convertsources`, and
    /// `supdb-ingestconvertsources` buffered.
    convertsources: bool,
    /// A dense form's copy sized with a record's worth of room per record,
    /// as before `Options::copy_exact`. `supdb-copyslack`, and
    /// `supdb-ingestcopyslack` buffered.
    copyslack: bool,
    /// A drain landing its lone piece and promoting it under a second
    /// manifest, as before `Options::drain_names_partition`:
    /// `supdb-ingestdrainpromote`.
    drainpromote: bool,
    /// A manifest publish that renames over the old manifest and frees it
    /// on the path, as before `Options::manifest_spare`. `supdb-manifestfree`,
    /// and `supdb-ingestmanifestfree` buffered.
    manifestfree: bool,
    /// The ordered index fsynced where it is written, as before
    /// `Options::ord_durable`. `supdb-oidxsync`, and `supdb-ingestoidxsync`
    /// buffered.
    oidxsync: bool,
    /// A direct run's table holding references into its segment in place
    /// of its values (`Options::direct_table_refs`). `supdb-refvals`, and
    /// `supdb-ingestrefvals` buffered.
    refvals: bool,
    /// The key section's checksum row hashed at the close from the file,
    /// as before `SegmentOptions::row_on_write`. `supdb-rowatclose`, and
    /// `supdb-ingestrowatclose` buffered.
    rowatclose: bool,
    /// A piece's keys ranked over the partition's records, as before
    /// `Options::piece_ranks_by_heads`. `supdb-rankrecords`, and
    /// `supdb-ingestrankrecords` buffered.
    rankrecords: bool,
    /// A store with no partitions partitioned once it is read,
    /// `Options::adaptive_shape`. `supdb-ingestshape`.
    shape: bool,
    /// The seal cap follows the reads, `Options::adaptive_cap`, to this
    /// share of the store at the full lag level: `supdb-adapt` and
    /// `supdb-ingestadapt` at 2, tighter than the cap; `supdb-adaptloose`
    /// and `supdb-ingestadaptloose` at 30, looser.
    adaptcap: Option<usize>,
    /// The merge trigger follows them too, `Options::adaptive_trigger`,
    /// with the cap. `supdb-adapttrig`, `supdb-ingestadapttrig`.
    adapttrig: bool,
    /// A seal syncs and rotates the WAL, the shape before seals kept it,
    /// `Options::seal_rotates_wal`. `supdb-rotate`, `supdb-shaperotate`.
    rotate: bool,
    /// A commit past the threshold keeps writing while a seal holds the
    /// slot, `Options::seal_defers`. `supdb-shapedefer`.
    defer: bool,
    /// A store with nothing sealed seals at the floor under the shape,
    /// `Options::seal_first_floor`. `supdb-shapefloor`.
    firstfloor: bool,
    /// Each reader of a frozen table sorts it for itself, as before the
    /// table kept its own snapshot: `Options::frozen_snaps` off.
    /// `supdb-nofrozen`, and `supdb-shapenofrozen` under the shape.
    nofrozen: bool,
    /// A commit under the threshold seals whenever no seal is writing
    /// (`Options::seal_idle`). `supdb-idle`, and `supdb-shapeidle` under
    /// the shape.
    idle: bool,
    /// A block over pieces sorts the pieces' keys whole rather than
    /// merging their runs: `Options::overlay_merge` off. `supdb-sortover`,
    /// and `supdb-shapesortover` under the shape.
    sortover: bool,
    /// Segments written in 2 MB pieces at 2 MB offsets, the page cache's
    /// PMD-sized folios, where the engine writes a megabyte:
    /// `SegmentOptions::write_piece`. `supdb-pmd`, and `supdb-ingestpmd`
    /// buffered.
    pmd: bool,
    /// The freeze takes the upkeep home, settles the backlog and carries
    /// the tables itself: `Options::freeze_settles` on.
    /// `supdb-settlefreeze`, and `supdb-ingestsettlefreeze` buffered.
    settlefreeze: bool,
    /// Dense forms' conversions capped at a fill, the builder converting
    /// the rest: `supdb-convertcap`, and `supdb-ingestconvertcap`
    /// buffered.
    convertcap: bool,
    /// The upkeep thread's pass bounded: `supdb-shortpass`, and
    /// `supdb-ingestshortpass` buffered.
    shortpass: bool,
    /// The upkeep thread's fill never yielding its conversions:
    /// `supdb-noyield`, and `supdb-ingestnoyield` buffered.
    noyield: bool,
    /// The fill yielding at any posted commit: `supdb-postedyield`, and
    /// `supdb-ingestpostedyield` buffered.
    postedyield: bool,
    /// The fill's conversions yielding to a posted commit once the thread
    /// is a thousand writes behind (`Options::forms_convert_behind`), as
    /// the option's first default meant. `supdb-behindyield`, and
    /// `supdb-ingestbehindyield` buffered.
    behindyield: bool,
    /// The background work as the cost model orders it: `supdb-bg`, and
    /// `supdb-ingestbg` buffered.
    bg: bool,
    /// `bg` with the thread's lag bounded and its passes batched
    /// (`Options::upkeep_lag_pct`, `Options::upkeep_batch_pct`):
    /// `supdb-bglag`, and `supdb-ingestbglag` buffered.
    bglag: bool,
    /// A point read of a partition through its ordered index
    /// (`Options::point_via_ord`): `supdb-nocacheordread`.
    ordread: bool,
    /// The second form forced on (`Options::zpages`):
    /// `supdb-nocachezpages`.
    zpages: bool,
    /// Its build keeps the raw records in the page cache
    /// (`Options::zpages_drop_behind` off): `supdb-nocachezpagesnodrop`.
    zpages_nodrop: bool,
    /// Its warm after the attach off (`Options::zpages_warm` off):
    /// `supdb-nocachezpagesnowarm`.
    zpages_nowarm: bool,
    /// Its memo of decoded groups off (`Options::zpages_memo` off):
    /// `supdb-nocachezpagesnomemo`.
    zpages_nomemo: bool,
    /// Its seeks by the ordered index's heads (`Options::zpages_seek_ord`
    /// on): `supdb-nocachezpagesheads`.
    zpages_heads: bool,
    /// Its scans asking ahead for their pages
    /// (`Options::zpages_scan_prefetch` on): `supdb-nocachezpagesprefetch`.
    zpages_prefetch: bool,
    /// Its overlay walk cut by rank (`Options::zpages_walk_cursor` off):
    /// `supdb-nocachezpagesrankwalk`.
    zpages_rankwalk: bool,
    /// Its builds left to the settle (`Options::zpages_eager` off):
    /// `supdb-nocachezpagessettle`.
    zpages_settle: bool,
    /// The form the controller's (`Options::zpages` at `ZPages::Auto`):
    /// `supdb-autozpages`, `supdb-nocacheautozpages`.
    zpages_auto: bool,
    /// The controller's bounded arm with the form's threads at the
    /// readers' I/O class (`Options::zpages_io_idle` off):
    /// `supdb-nocacheautozpagesfairio`, the pair against
    /// `supdb-nocacheautozpages` under a bound.
    zpages_fairio: bool,
    /// The controller's bounded arm with its builds asking nothing of the
    /// raw records ahead of their walks (`Options::zpages_build_ahead` at
    /// zero): `supdb-nocacheautozpagesnoahead`.
    zpages_noahead: bool,
    /// The controller's bounded arm with the eager entry
    /// (`Options::zpages_build_eager`): `supdb-nocacheautozpageseager`.
    zpages_buildeager: bool,
    /// The block cache's reads of a partition through the door under the
    /// controller's form (`Options::zpages_block_cache` off):
    /// `supdb-cache64tofitautozpagesraw`.
    zcacheraw: bool,
    /// The coding the selector's to choose (`Options::zpages_coding_auto`):
    /// `supdb-cache64tofitautozpagescodingauto` against
    /// `supdb-cache64tofitautozpages`.
    zcodingauto: bool,
    /// Blocks read this many entries' worth held as copies in their slots
    /// under the form (`Options::promote_entries` with
    /// `zpages_promote_copies`), zero for neither:
    /// `supdb-cache64tofitautozpagespromote`.
    zpromote: usize,
    /// The form's page cut into groups at this many bytes of records
    /// (`Options::zpages_group_bytes`), `None` for the engine's default:
    /// `supdb-cache64tofitautozpagesonegroup`, `...group1k`.
    zgroup: Option<usize>,
    /// The form files built ahead past the budget
    /// (`Options::zpages_prebuild`): `supdb-cache64tofitautozpagesprebuild`.
    zprebuild: bool,
    /// The scan's rank skipped for a copy (`Options::zpages_seek_span`):
    /// `supdb-cache64tofitautozpagesspan`.
    zspan: bool,
    /// The memo's slot unhashed (`Options::zpages_memo_hash` off):
    /// `supdb-cache64tofitautozpagesmemolinear`.
    zmemolinear: bool,
    /// Every filed cut resolved at the settle (`Options::zpages_settle_span`
    /// off): `supdb-cache64tofitautozpagesexactcut`.
    zexactcut: bool,
    /// Every view of a form file's page checked and the memo asked after it
    /// (`Options::zpages_trusted_views` off):
    /// `supdb-cache64tofitautozpagescheckedviews`.
    zcheckedviews: bool,
    /// The form file's groups holding their records one by one
    /// (`Options::zpages_record_coding` on): `supdb-cache64tofitautozpagesrecord`,
    /// `supdb-nocachezpagesrecord`.
    zrecord: bool,
    /// The form file's groups stored verbatim (`Options::zpages_store_all`):
    /// `supdb-nocachezpagesstored`, `supdb-cache64tofitzpagesstored`.
    zstored: bool,
    /// The record coding with every tail stored raw (`Options::zpages_raw_tails`):
    /// `supdb-nocachezpagesrecordraw`, `supdb-cache64tofitzpagesrecordraw`.
    zrawtails: bool,
    /// The allocator trimmed once after the form files' install
    /// (`Options::zpages_trim_install`): `supdb-cache64tofitzpagesrecordrawlean`.
    ztrim: bool,
    /// Its raw-tail groups decoded whole for a scan
    /// (`Options::zpages_inplace_walk` off): `supdb-cache64tofitzpagesrecordrawdecode`.
    zdecodewalk: bool,
    /// The trim thread asked to look at every upkeep pass end
    /// (`Options::upkeep_trim_look`): `supdb-cache64tofitzpagesrecordrawleantrim`.
    ztrimpass: bool,
    /// Its trim counting by the resident bytes (`Options::trim_by_resident`):
    /// `supdb-cache64tofitzpagesrecordrawleanresident`.
    ztrimresident: bool,
    /// Its first partitions raw-tailed (`Options::zpages_raw_tails_first`):
    /// `supdb-cache64tofitzpageshot4`.
    zhotfirst: usize,
    /// Its raw pages dropped behind the form (`Options::zpages_drop_raw`):
    /// `supdb-cache64tofitzpagesdrop`, `supdb-cache64tofitzpageshot4drop`.
    zdropraw: bool,
    /// Its partitions re-coded while served (`Options::zpages_recode`):
    /// `supdb-cache64tofitzpagesrecodedrop`.
    zrecode: bool,
    /// The replaced form file kept for the next re-code
    /// (`Options::zpages_recode_keep`): `supdb-cache64tofitzpagesrecodekeepdrop`.
    zrecodekeep: bool,
    /// Every commit near reads settles its batch
    /// (`Options::forms_settle_every`):
    /// `supdb-cache64tofitzpageshot8dropsettleevery`.
    settleevery: bool,
    /// Its settles' blocks built by the fill (`Options::forms_settle_builds`):
    /// `supdb-cache64tofitzpageshot8dropsettlebuilds`.
    settlebuilds: bool,
    /// (`Options::zpages_memo_first`): `supdb-cache64tofitautozpagesmemofirst`.
    zmemofirst: bool,
    /// The fence searched through its tree (`Options::zpages_fence_tree`):
    /// `supdb-cache64tofitautozpagesfencetree`.
    zfencetree: bool,
    /// Nothing asked ahead for the memo entry a decode's put displaces
    /// (`Options::zpages_memo_ahead` off): `supdb-cache64tofitautozpagesmemonoahead`.
    zmemonoahead: bool,
    /// A scan's start key kept with its block (`Options::scan_start_cache`
    /// at 4,096 entries): `supdb-cache64tofitautozpagesstartcache`.
    zstartcache: bool,
    /// A block read through a cursor a run, a seek a cut and a read by rank
    /// a replaced record (`Options::zpages_block_cursor` off):
    /// `supdb-cache64tofitautozpagesperrun`.
    zperrun: bool,
    /// The memo's cap in mebibytes, zero for the engine's
    /// (`Options::zpages_memo_bytes`): `supdb-cache64tofitautozpagesmemo4`,
    /// `...memo16`, `...memo32`.
    zmemo_mib: usize,
}

/// The conversions a fill makes at most in `supdb-convertcap`: the first
/// version's cap, about a pass's worth of conversions at a hundred
/// thousand keys.
const CONVERT_CAP: usize = 64;

/// The share of the store's keys a pass of `supdb-shortpass` files at
/// most: the square-root rule's pass size at the quick ladder's rungs
/// (`docs/engine.md`, the cost model).
const SHORT_PASS_PCT: usize = 2;

impl Default for Policy {
    /// `supdb`: the shipping configuration.
    fn default() -> Policy {
        Policy {
            partition: true,
            drain: true,
            advice: None,
            block_cache: true,
            durable: true,
            budget: 0,
            forms: false,
            snap: true,
            settle: false,
            wforms: false,
            lagcap: 0,
            sealcap: None,
            l0: None,
            nopin: false,
            asympin: false,
            extsval: false,
            piecesonly: false,
            latewb: false,
            inlinemaint: false,
            copysearch: false,
            convertsources: false,
            copyslack: false,
            drainpromote: false,
            manifestfree: false,
            oidxsync: false,
            refvals: false,
            rowatclose: false,
            lendbursts: false,
            inlineall: false,
            lazysettle: false,
            walsync: false,
            nodrainsnap: false,
            sealrun: false,
            closeinline: false,
            timers: false,
            wake: false,
            noprefetch: false,
            dirplain: false,
            settlefar: false,
            tofit: false,
            notrim: false,
            noshed: false,
            nosweep: false,
            prefetch1k: false,
            manifestrename: false,
            manifestpending: false,
            waitmanifest: false,
            populate: false,
            piece256: false,
            recordsteps: false,
            copyvals: false,
            keycopies: false,
            fillplain: false,
            latesync: false,
            norecycle: false,
            keysarena: false,
            noalert: false,
            ordlog: false,
            fatentries: false,
            rankrecords: false,
            shape: false,
            adaptcap: None,
            adapttrig: false,
            rotate: false,
            nofrozen: false,
            idle: false,
            sortover: false,
            pmd: false,
            settlefreeze: false,
            convertcap: false,
            shortpass: false,
            noyield: false,
            postedyield: false,
            behindyield: false,
            bg: false,
            bglag: false,
            ordread: false,
            zpages: false,
            zpages_nodrop: false,
            zpages_nowarm: false,
            zpages_nomemo: false,
            zpages_heads: false,
            zpages_prefetch: false,
            zpages_rankwalk: false,
            zpages_settle: false,
            zpages_auto: false,
            zpages_fairio: false,
            zpages_noahead: false,
            zpages_buildeager: false,
            zcacheraw: false,
            zcodingauto: false,
            zpromote: 0,
            zgroup: None,
            zprebuild: false,
            zspan: false,
            zmemolinear: false,
            zexactcut: false,
            zcheckedviews: false,
            zrecord: false,
            zstored: false,
            zrawtails: false,
            ztrim: false,
            zdecodewalk: false,
            ztrimpass: false,
            ztrimresident: false,
            zhotfirst: 0,
            zdropraw: false,
            zrecode: false,
            zrecodekeep: false,
            settleevery: false,
            settlebuilds: false,
            zmemofirst: false,
            zfencetree: false,
            zmemonoahead: false,
            zstartcache: false,
            zperrun: false,
            zmemo_mib: 0,
            defer: false,
            firstfloor: false,
            aheadmin: None,
            lazyforms: false,
            eager: None,
            recent: None,
            carry: true,
            rebuild: 0,
            snapcarry: false,
            lazysnap: false,
            fullrec: false,
            norebase: false,
            sealfile: false,
            settleall: false,
            upkeep: None,
            tier: None,
            runs: false,
            keeper: false,
            aheadpub: false,
            pubalways: false,
            leave: false,
        }
    }
}

impl Supdb {
    pub fn create(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(path, Policy::default())
    }

    pub fn create_ingest(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingest` whose `sync` is the durable write
    /// and nothing more -- the WAL's frames written and synced, the
    /// memtable left where it is -- with the seal and the partitioning
    /// left to the background.
    pub fn create_ingest_sync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with its flush leaving the pieces as they are, the
    /// shape before `Options::flush_schedules`, priced against the arm in
    /// one process.
    pub fn create_ingest_leave(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                leave: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the segment work driven inline by the writer,
    /// as before `Options::publish_in_background`.
    pub fn create_ingest_inline(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                inlinemaint: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingestsync` -- `sync` the durable write, with
    /// the tail of a store no partition was made for handed to a seal it
    /// does not wait for -- and the store partitioned by the segment work
    /// once anything reads it (`Options::adaptive_shape`).
    pub fn create_ingest_shape(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb` with the seal cap following the reads
    /// (`Options::adaptive_cap`): the cap as it stands while nothing
    /// reads over lag, tighter while something does.
    pub fn create_adapt(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                adaptcap: Some(2),
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-adapt` with the merge trigger following the
    /// reads as well (`Options::adaptive_trigger`).
    pub fn create_adapttrig(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                adapttrig: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingest` with the seal cap following the reads.
    pub fn create_ingest_adapt(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                adaptcap: Some(2),
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingestadapt` with the merge trigger following
    /// the reads as well.
    pub fn create_ingest_adapttrig(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                adapttrig: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose seals sync and rotate the WAL on the writer's
    /// thread (`Options::seal_rotates_wal`), the shape before seals kept
    /// it: the comparison arm for that change.
    pub fn create_rotate(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                rotate: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose readers of a frozen table each sort it for
    /// themselves (`Options::frozen_snaps` off), the shape before the
    /// table kept its own snapshot.
    pub fn create_nofrozen(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                nofrozen: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose blocks over pieces sort the pieces' keys whole
    /// (`Options::overlay_merge` off), the shape before the merge.
    pub fn create_sortover(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                sortover: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose segments are written in 2 MB pieces, the shape
    /// before `SegmentOptions::write_piece` was a megabyte.
    pub fn create_pmd(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                pmd: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose segments are written in 2 MB pieces, as
    /// `supdb-pmd`: the arm whose ordered load writes its segment on the
    /// commit path without a sync between pieces.
    pub fn create_ingest_pmd(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                pmd: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose freeze settles the backlog and carries the tables
    /// itself, as it did before the carry was left to the look.
    pub fn create_settlefreeze(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                settlefreeze: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose freeze settles and carries, as
    /// `supdb-settlefreeze`.
    pub fn create_ingest_settlefreeze(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                settlefreeze: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the dense forms' conversions capped at a fill and the
    /// builder ahead of a reader converting the rest.
    pub fn create_convertcap(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                convertcap: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` converting as `supdb-convertcap`.
    pub fn create_ingest_convertcap(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                convertcap: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose upkeep thread files at most a share of the store's
    /// keys in one pass and re-posts the rest.
    pub fn create_shortpass(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                shortpass: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the bounded pass, as `supdb-shortpass`.
    pub fn create_ingest_shortpass(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                shortpass: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose upkeep thread converts every dense form a fill
    /// meets, yielding to no commit: the shape before the yield.
    pub fn create_noyield(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                noyield: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose fill yields its conversions at any posted commit
    /// (`Options::forms_convert_behind` zero): against `supdb` it prices
    /// converting in the thread's slack through a burst.
    pub fn create_postedyield(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                postedyield: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose fill yields at any posted commit, as
    /// `supdb-postedyield`.
    pub fn create_ingest_postedyield(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                postedyield: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose fill never yields, as `supdb-noyield`.
    pub fn create_ingest_noyield(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                noyield: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose conversions yield to a posted commit once the thread
    /// is a thousand writes behind the writer (`Options::forms_convert_behind`):
    /// against `supdb` it prices the yield to the writer's wait alone.
    pub fn create_behindyield(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                behindyield: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` yielding a thousand writes behind, as
    /// `supdb-behindyield`.
    pub fn create_ingest_behindyield(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                behindyield: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the background work the cost model orders.
    pub fn create_bg(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                bg: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the background work the cost model orders.
    pub fn create_ingest_bg(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                bg: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-bg` with the thread's lag bounded and its passes batched.
    pub fn create_bglag(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                bglag: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingestbg` with the thread's lag bounded and its passes
    /// batched.
    pub fn create_ingest_bglag(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                bglag: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingestshape` whose blocks over pieces sort the pieces' keys
    /// whole, as `supdb-sortover`: the arm that keeps its pieces until a
    /// read shapes the store.
    pub fn create_shape_sortover(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                sortover: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingestshape` whose readers of a frozen table each sort it
    /// for themselves, as `supdb-nofrozen`: the arm a `sync` hands its
    /// table to a seal under, which the table's own snapshot is for.
    pub fn create_shape_nofrozen(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                nofrozen: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb` whose commits seal whenever no seal is writing
    /// (`Options::seal_idle`).
    pub fn create_idle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                idle: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingestshape` whose commits seal whenever no seal
    /// is writing (`Options::seal_idle`).
    pub fn create_shape_idle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                idle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingestshape` whose seals rotate the WAL, as `supdb-rotate`.
    pub fn create_shape_rotate(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                rotate: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingestshape` whose commits defer a seal the frozen
    /// slot cannot take yet (`Options::seal_defers`); the deferred seal
    /// takes whatever grew, up to twice the threshold, so the seal's size
    /// moves with the wait it removes.
    pub fn create_shape_defer(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                defer: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-shapedefer` whose first seal comes at the floor
    /// (`Options::seal_first_floor`), so its load leaves a partition.
    pub fn create_shape_floor(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                drain: false,
                durable: false,
                shape: true,
                defer: true,
                firstfloor: true,
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: the seal cap following the reads the other way: the
    /// cap as it stands while nothing reads over lag, three times it
    /// while something does, for the store whose reads a seal costs more
    /// than the lag it removes.
    pub fn create_adaptloose(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                adaptcap: Some(30),
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingest` with the seal cap loosening under
    /// reads, as `supdb-adaptloose` does.
    pub fn create_ingest_adaptloose(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                adaptcap: Some(30),
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb-ingest` with the seal cap off: its memtable
    /// seals at `seal_bytes` whatever the store holds, as it did while the
    /// store stayed one piece and the cap had nothing to take a share of.
    pub fn create_ingest_noseal(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                sealcap: Some(0),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the range-read structure maintained at every commit
    /// whether or not anyone holds a handle to read it, which is the
    /// shape before the regime was asked of the store: it prices the
    /// gate, since `supdb` maintains only once a caller's handle is live.
    /// The pair differs by one option and needs no matching.
    pub fn create_forms(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                forms: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` in every respect but where the sorted unsealed keys live:
    /// each handle's own, sorted from nothing whenever it goes stale,
    /// which is the shape before they were published in the state. The
    /// pair differs by one option and needs no matching.
    /// `supdb` maintaining at every commit as `supdb-forms` does, but
    /// settling only: no form built for an overlaid block and nothing
    /// published. Between the two it says whether that arm's gain on the
    /// threaded scan mix is the settling or the forms, which the counts
    /// say cannot be adoption.
    pub fn create_settle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                forms: true,
                settle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with no settle backlog at all: a write burst's filing waits
    /// for the first read after it, which is the shape before the bound.
    pub fn create_nosettle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                eager: Some(0),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the piece merge on at three pieces, so a range's
    /// pieces fold into one beside the partition merge: what prices it.
    pub fn create_runs(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                runs: true,
                ..Policy::default()
            },
        )
    }

    pub fn create_keeper(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                runs: true,
                keeper: true,
                ..Policy::default()
            },
        )
    }

    pub fn create_tier(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                tier: Some(3),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the canonical forms dropped at every seal, the shape
    /// before they were carried. Against `supdb` it prices the carry:
    /// what a table that survives the seal is worth to the reads after
    /// it, and what keeping it whole through a write-only burst costs
    /// the writes.
    /// `supdb` with a block rebuilt instead of patched once a settle's
    /// backlog holds six of its keys. Against `supdb` it prices a cost
    /// per key against a cost per block: six is near where the two met
    /// when the patch was timed, and the density the settles meet says
    /// how often the bound is reached at all.
    pub fn create_rebuild(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                rebuild: 6,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` dropping and rebuilding a block from six keys of
    /// it in a settle's backlog, as `supdb-rebuild`: the buffered arm's
    /// passes carry tens of keys a block where the durable arm's carry
    /// two, which is the density the bound was measured against.
    pub fn create_ingest_rebuild(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                rebuild: 6,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the writer's scan snapshot carried across a publish
    /// rather than dropped and sorted again at the next commit due.
    /// Against `supdb` it prices where that work lands: the pass after a
    /// burst, or the burst's own commits.
    pub fn create_snapcarry(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                snapcarry: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the scan snapshot built only at a block that needs
    /// it, rather than before a scan walks anything. Against `supdb` it
    /// prices the lazy snapshot: the first scan after a publish over forms
    /// the writer holds.
    pub fn create_lazysnap(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                lazysnap: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` writing every record with its extent. Against `supdb` it
    /// prices the compact record: sixteen bytes a key of file and what a
    /// scan saves in not reading them. The seal is sized by the data, so
    /// the cadence is not in it.
    pub fn create_fullrec(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                fullrec: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` sizing the seal by the partitions' file bytes, the rule
    /// before the store's payload was recorded. Against `supdb` it prices
    /// the calibration that carries the file rule's cadence over to the
    /// data; beside `supdb-fullrec` it is what the format priced before.
    pub fn create_sealfile(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                sealfile: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` taking the settle bound on every segment's keys, level-0
    /// pieces included, as before. Against `supdb` it prices the bound on
    /// the partitions': the lag sweep's pass after a burst, and the mixes'
    /// commits, which settle more often where pieces stand.
    pub fn create_settleall(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                settleall: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the writer keeping its own upkeep -- the forms and
    /// the snapshot a commit files its writes into -- as before the
    /// thread. Against `supdb` it prices the thread: the lag sweep's pass
    /// after a burst, which the thread files ahead of, and the mixes'
    /// commits and reads, which share the machine with it.
    pub fn create_inline(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                upkeep: Some(supdb::Upkeep::Inline),
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose commits hold for the upkeep thread while reads are
    /// around (`Upkeep::Background(2)`), the default before commits
    /// stopped waiting for it: against `supdb` it prices the hold, the
    /// lag sweep's bursts against the passes after them.
    pub fn create_hold(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                upkeep: Some(supdb::Upkeep::Background(2)),
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose commits hold for the upkeep, as `supdb-hold`.
    pub fn create_ingest_hold(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                upkeep: Some(supdb::Upkeep::Background(2)),
                ..Policy::default()
            },
        )
    }

    /// `supdb` dropping a partition's copies at a merge that rewrote it
    /// over the same keys. Against `supdb` it prices the rebase: the
    /// fully-unmerged lag point's pass after the burst's partition merge.
    pub fn create_norebase(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                norebase: true,
                ..Policy::default()
            },
        )
    }

    pub fn create_nocarry(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                carry: false,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the builder started at the first commit after a
    /// publish and its forms installed at the commits they arrive at.
    /// Against `supdb` it prices what a table filled before the first
    /// read is worth against the filing the posting commits do.
    pub fn create_aheadpub(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                aheadpub: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` publishing the forms at every maintained commit whether
    /// or not a handle is live to take them, as it did before. Against
    /// `supdb` it prices the copy every publish forces on the next patch
    /// of the block, over the mixes and the sweep, where no handle is.
    pub fn create_pubalways(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                pubalways: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` settling by its backlog only within `RECENT` percent of
    /// the store's keys written since the last scan (ten unless set), at
    /// `BACKLOG` percent of the store's keys or the engine's own bound.
    /// Against `supdb` it prices the recency window: what a burst's
    /// filing costs the mixes when nothing reads it before the next seal,
    /// and what leaving it costs the first read after the burst.
    pub fn create_recency(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                recent: Some(
                    std::env::var("RECENT")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(10),
                ),
                eager: std::env::var("BACKLOG").ok().and_then(|v| v.parse().ok()),
                ..Policy::default()
            },
        )
    }

    /// `supdb` settling past a pinned backlog, `BACKLOG` percent of the
    /// store's keys (one unless set). Against `supdb` it prices moving a
    /// write burst's filing from the first read after it onto the
    /// commits themselves, at a bound other than the engine's own.
    pub fn create_eager(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                eager: Some(
                    std::env::var("BACKLOG")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1),
                ),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the forms waiting for a handle the caller made to
    /// have scanned, which is what the engine did before the seal cap made
    /// the fill cheap enough to stop waiting.
    pub fn create_lazyforms(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                lazyforms: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the builder starting on any store. Below
    /// `scan_cache_ahead_min_blocks` the builder declines and the writer
    /// fills the forms inline at the commit instead; the threshold was
    /// measured as the builder against no builder at all, never against
    /// that inline fill, and at ten and thirty thousand keys the inline
    /// fill is what ycsb-E pays for.
    pub fn create_ahead(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                aheadmin: Some(0),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the seal capped only by `seal_bytes`, which is the
    /// shape every arm had before the cap: on a store smaller than the
    /// 32 MiB floor the memtable can hold the whole of it and never seal.
    /// Against `supdb` it prices the cap, and it prices the lag axis and
    /// the load axis together, which is the only way either means
    /// anything. `SEALPCT` pins a different share for a sweep.
    pub fn create_noseal(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                sealcap: Some(
                    std::env::var("SEALPCT")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                ),
                ..Policy::default()
            },
        )
    }

    /// EXPERIMENT: `supdb` with the merge trigger pinned by `L0TRIG`
    /// (the engine's own when unset): the pieces a range holds before
    /// they are merged into its partition, swept against `supdb`.
    pub fn create_l0(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                l0: Some(
                    std::env::var("L0TRIG")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(supdb::Options::default().l0_trigger),
                ),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the writer's operations pinning nothing, as before
    /// `Options::writer_pins`, and so with the segment work inline, since
    /// an unpinned writer must be the one publisher: against
    /// `supdb-inlinemaint` it prices the pins alone.
    pub fn create_nopin(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                nopin: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the forms holding the pieces' fold alone and every
    /// walk overlaying the scan snapshot (`Options::forms_pieces_only`):
    /// against `supdb` it prices the settle that files nothing.
    pub fn create_piecesonly(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                piecesonly: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the forms holding the pieces' fold alone, as
    /// `supdb-piecesonly`.
    pub fn create_ingest_piecesonly(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                piecesonly: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the segment writer leaving every piece dirty and
    /// unsynced until the closing fsync (`SegmentOptions::early_writeback`
    /// off and `sync_ahead` zero): against `supdb` it prices the writeback
    /// the writer starts early and the syncs its thread makes behind it.
    pub fn create_latewb(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                latewb: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose scans and settles search a copy for their key, as
    /// before `Options::copy_start_at_rank`: against `supdb` it prices
    /// finding the entry by the rank the seek or the resolve found.
    pub fn create_copysearch(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                copysearch: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the copies searched, as `supdb-copysearch`.
    pub fn create_ingest_copysearch(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                copysearch: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` converting a dense sparse form by a build from the sources,
    /// as before `Options::convert_from_form`: against `supdb` it prices
    /// the conversion from the form's own deltas.
    pub fn create_convertsources(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                convertsources: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` converting from the sources, as `supdb-convertsources`.
    pub fn create_ingest_convertsources(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                convertsources: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` sizing a dense form's copy with room, as before
    /// `Options::copy_exact`: against `supdb` it prices the exact size,
    /// which fits the chunks the forms before it freed.
    pub fn create_copyslack(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                copyslack: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the copies sized with room, as `supdb-copyslack`.
    pub fn create_ingest_copyslack(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                copyslack: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose drain lands its lone piece and promotes it
    /// under a second manifest, as before `Options::drain_names_partition`:
    /// against `supdb-ingest` it prices naming the partition at the close.
    pub fn create_ingest_drainpromote(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                drainpromote: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose manifest publish frees the old manifest on its path,
    /// as before `Options::manifest_spare`: against `supdb` it prices the
    /// spare link and the deferred unlink.
    pub fn create_manifestfree(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                manifestfree: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` freeing the old manifest on the path, as
    /// `supdb-manifestfree`.
    pub fn create_ingest_manifestfree(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                manifestfree: true,
                ..Policy::default()
            },
        )
    }

    /// The ordered index fsynced where it is written, as before
    /// `Options::ord_durable`: against `supdb` it prices the fsync of a
    /// file the open can rebuild, on every seal's and merge's landing.
    pub fn create_oidxsync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                oidxsync: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` fsyncing the ordered index, as `supdb-oidxsync`.
    pub fn create_ingest_oidxsync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                oidxsync: true,
                ..Policy::default()
            },
        )
    }

    /// A direct run's table holding references into its segment in place
    /// of copies of its values (`Options::direct_table_refs`): against
    /// `supdb` it prices the copy and the fresh pages it touches on the
    /// writer's path.
    pub fn create_refvals(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                refvals: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the references, as `supdb-refvals`.
    pub fn create_ingest_refvals(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                refvals: true,
                ..Policy::default()
            },
        )
    }

    /// The checksum row hashed at the close from the file rather than on
    /// the writer as the bytes are buffered (`SegmentOptions::row_on_write`):
    /// against `supdb` it prices the read-back on the drain's path.
    pub fn create_rowatclose(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                rowatclose: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` hashing the row at the close, as `supdb-rowatclose`.
    pub fn create_ingest_rowatclose(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                rowatclose: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with every commit lending its batch to the upkeep thread
    /// (`Options::upkeep_inline_bursts` off). Its commits sync, so they
    /// lend either way: against `supdb` it is a null pairing.
    pub fn create_lendbursts(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                lendbursts: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with every commit lending its batch to the thread,
    /// as before a buffered commit near reads filed a burst's batch
    /// itself: against `supdb-ingest` it prices paying the filing in the
    /// burst against the first scan after it paying it.
    pub fn create_ingest_lendbursts(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                lendbursts: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a commit near reads filing any batch itself, however
    /// small (`Options::upkeep_inline_min` at one). Its commits sync, so
    /// they lend either way: against `supdb` it is a null pairing.
    pub fn create_inlineall(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                inlineall: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with a commit near reads filing any batch itself,
    /// however small: against `supdb-ingest` it prices paying the filing
    /// of a batch under the threshold in the burst against the first
    /// scan after it paying it, and the mixes' batches of a hundred with
    /// it.
    pub fn create_ingest_inlineall(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                inlineall: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` leaving a small batch's settle to the next scan
    /// (`Options::upkeep_settle_after_scans` off). Its commits sync, so
    /// they lend either way: against `supdb` it is a null pairing.
    pub fn create_lazysettle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                lazysettle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` leaving a small batch's settle to the next scan, as
    /// before a commit with a scan behind it settled the batch itself:
    /// against `supdb-ingest` it prices the settle in the burst against
    /// the settle in the first scan.
    pub fn create_ingest_lazysettle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                lazysettle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose flush fsyncs the WAL before its seal, as before. Its
    /// commits sync, so the log holds nothing to sync at a flush: against
    /// `supdb` it is a null pairing.
    pub fn create_walsync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                walsync: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose flush fsyncs the WAL before its seal, as
    /// before a buffered drain was answered behind its slot instead:
    /// against `supdb-ingest` it prices that fdatasync on the drain.
    pub fn create_ingest_walsync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                walsync: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose drain's seal builds no snapshot copy and walks the
    /// table for its records: against `supdb` it prices the copy on the
    /// durable drain, which read as the cheaper walk.
    pub fn create_nodrainsnap(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                nodrainsnap: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose drain's seal builds no snapshot copy and
    /// walks the table for its records: against `supdb-ingest` it prices
    /// the copy on the buffered drain, which read as the cheaper walk.
    pub fn create_ingest_nodrainsnap(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                nodrainsnap: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose seals write every key through the writer's general
    /// path, as before: against `supdb` it prices the one-value path on
    /// the durable seals.
    pub fn create_sealrun(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                sealrun: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose seals write every key through the writer's
    /// general path, as before: against `supdb-ingest` it prices the
    /// one-value path on the buffered seals.
    pub fn create_ingest_sealrun(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                sealrun: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose flush composes the trailer on the writer's thread
    /// while a thread syncs the records: against `supdb` it prices the
    /// finish on the writer, which read slower, since the trailer's write
    /// waits for the records' sync on either thread.
    pub fn create_closeinline(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                closeinline: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose flush composes the trailer on the writer's
    /// thread while a thread syncs the records: against `supdb-ingest` it
    /// prices the finish on the writer, which read slower.
    pub fn create_ingest_closeinline(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                closeinline: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` timing every scan's phases, as before the clock reads were
    /// gated: against `supdb` it prices the instrument.
    pub fn create_timers(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                timers: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` timing every scan's phases, as `supdb-timers`.
    pub fn create_ingest_timers(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                timers: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose lends wake the upkeep thread when it is parked past a
    /// short batch's wait (`Options::upkeep_wake_parked`).
    pub fn create_wake(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                wake: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-wake`.
    pub fn create_ingest_wake(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                wake: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose block walks issue no prefetches
    /// (`Options::scan_prefetch` off).
    pub fn create_noprefetch(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                noprefetch: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-noprefetch`.
    pub fn create_ingest_noprefetch(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                noprefetch: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose scans seek without the stride window's directory
    /// hint and whose record hint reads its span's two directory words
    /// (`Options::seek_prefetch_dir` off): against `supdb` it prices
    /// the fill taken off a scan's start.
    pub fn create_dirplain(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                dirplain: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-dirplain`.
    pub fn create_ingest_dirplain(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                dirplain: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose commits file their backlog into the forms far from
    /// any scan (`Options::forms_settle_far`), the shape before: against
    /// `supdb` it prices the filing a stretch nobody reads paid for, in
    /// the load's rate and the process's memory.
    pub fn create_settlefar(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                settlefar: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose block cache costs what its forms' buffers allocate
    /// (`Options::forms_to_fit`): against `supdb` it prices the forms
    /// built and grown to fit, and the process's memory they leave.
    pub fn create_tofit(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                tofit: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-tofit`.
    pub fn create_ingest_tofit(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                tofit: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache256` whose cache is counted and kept to fit
    /// (`Options::forms_to_fit`): against `supdb-cache256` it prices the
    /// budget as a bound on the process, which the arm beside it is not.
    pub fn create_cache256_tofit(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 256 << 20,
                tofit: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache256tofit` with the cache's bound at 64 MB, the base of
    /// the combined arm below: the Zipfian head's hot blocks are a small
    /// set, and 256 MB of cache beside the compact form did not fit a
    /// 640 MiB bound at five million keys (the cgroup killed the harness
    /// at 650 MB of anonymous memory in the shuffled load).
    pub fn create_cache64_tofit(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofit` with the second form the controller's to
    /// choose: the block cache's merged forms over the hot blocks, kept to
    /// fit 64 MB, beside the compact form for the rest of the store. The
    /// pair against `supdb-nocacheautozpages` prices the cache under a
    /// bound: what E, A, B and F gain from the merged forms of the Zipfian
    /// head against what the point reads lose to the page cache the cache
    /// displaces; against `supdb-cache64tofit`, what the compact form adds
    /// to the cache arm out of core.
    pub fn create_cache64_tofit_auto_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the block cache's budget at 128 MB
    /// (`Options::scan_cache_bytes`), `supdb-cache128tofitautozpages`: the
    /// pair against the default prices the budget on the E mix under the
    /// bound, whose scans build 24,000 forms a pass at 64 MB while the
    /// Zipfian head they start in is about 9 MB of copies.
    pub fn create_cache128_tofit_auto_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 128 << 20,
                tofit: true,
                zpages_auto: true,
                ..Policy::default()
            },
        )
    }

    /// The same with the budget at 256 MB: `supdb-cache256tofitautozpages`.
    pub fn create_cache256_tofit_auto_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 256 << 20,
                tofit: true,
                zpages_auto: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the block cache's reads of a
    /// partition through the door instead of the form file
    /// (`Options::zpages_block_cache` off): the shape the combined arm had
    /// when it was first priced, whose walks dragged the raw records
    /// through the page cache and evicted the form the point reads needed.
    /// The pair against `supdb-cache64tofitautozpages` prices the cache's
    /// reads through the form.
    pub fn create_cache64_tofit_auto_zpages_raw(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zcacheraw: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the coding of the form files the
    /// selector's to choose (`Options::zpages_coding_auto`): the raw-tail
    /// record coding where the model predicts that form fits the budget, the
    /// block coding otherwise, `supdb-cache64tofitautozpagescodingauto`. The
    /// pair against the combined arm prices the gear: at three million keys
    /// under 640 MiB it picks the raw tails, at five million the block
    /// coding the default builds.
    pub fn create_cache64_tofit_auto_zpages_codingauto(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zcodingauto: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a block that reads have walked
    /// a block's worth of entries (sixty-four) held as a merged copy in
    /// its slot under the form, a clean block included
    /// (`Options::promote_entries`, `zpages_promote_copies`): the E mix's
    /// hot head walked as copies, which open no group, where the combined
    /// arm walks it as sparse and clean forms through the form file at a
    /// group decoded a scan. The pair against `supdb-cache64tofitautozpages`
    /// prices the copies against the decodes they replace and the budget
    /// they take.
    pub fn create_cache64_tofit_auto_zpages_promote(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zpromote: 64,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the form's page as one group
    /// (`Options::zpages_group_bytes` zero), the shape before the page
    /// became a container of groups: a point read decodes the whole page
    /// for its record. The pair against `supdb-cache64tofitautozpages`
    /// prices the cut, which costs the form six percent of its bytes at
    /// the suite's shape for a point read's decode a quarter of the
    /// page's.
    pub fn create_cache64_tofit_auto_zpages_onegroup(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zgroup: Some(0),
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the page cut at a kibibyte of
    /// records (`Options::zpages_group_bytes` 1024) where the default cuts
    /// at two: twice the groups, twelve percent of the form's bytes against
    /// six for a point read's decode a seventh of the page's against a
    /// quarter. The pair against `supdb-cache64tofitautozpages` prices the
    /// target.
    pub fn create_cache64_tofit_auto_zpages_group1k(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zgroup: Some(1024),
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the form files built ahead of
    /// the controller's decision once the store outgrows its budget
    /// (`Options::zpages_prebuild`): written to disk and left dormant
    /// while the raw form serves, so that the controller's build is the
    /// attach of files already there. The pair against
    /// `supdb-cache64tofitautozpages` prices the head start -- the build's
    /// wall under the readers' faults taken off the shift -- against what
    /// the build costs the passes it runs beside; a store that fits builds
    /// nothing, so the in-memory rungs read the same arm twice.
    pub fn create_cache64_tofit_auto_zpages_prebuild(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zprebuild: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a block-cache scan's block taken
    /// from the form file's fence and the page's directory, and its rank
    /// sought only for a form walked by rank (`Options::zpages_seek_span`):
    /// the pair against `supdb-cache64tofitautozpages` prices the seek a
    /// copy walked from its start key does not need, and read level.
    pub fn create_cache64_tofit_auto_zpages_span(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zspan: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the memo's slot the group's
    /// number plus a multiple of the identity (`Options::zpages_memo_hash`
    /// off), the shape before the hash: the pair against
    /// `supdb-cache64tofitautozpages` prices the slots a form file of the
    /// shipped cut could not reach.
    /// `supdb-cache64tofitautozpages` with a settle resolving every
    /// written key's cut through a seek (`Options::zpages_settle_span`
    /// off), the shape before: the pair against
    /// `supdb-cache64tofitautozpages` prices the cuts left to the builds.
    pub fn create_cache64_tofit_auto_zpages_exactcut(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zexactcut: true,
                ..Policy::default()
            },
        )
    }

    pub fn create_cache64_tofit_auto_zpages_memolinear(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zmemolinear: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with every view of a form file's page
    /// checked and the memo asked after it (`Options::zpages_trusted_views`
    /// off), the shape before: the pair against
    /// `supdb-cache64tofitautozpages` prices the directory's cross-checks
    /// re-made at every group open.
    pub fn create_cache64_tofit_auto_zpages_checkedviews(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zcheckedviews: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the form file's groups holding
    /// their records one by one (`Options::zpages_record_coding` on): the
    /// pair against `supdb-cache64tofitautozpages` prices a record decoded
    /// against a group decoded, on every read through the form -- the
    /// point-read mixes a quarter to a third faster and E's scans a fifth
    /// slower at five million keys under 640 MiB.
    pub fn create_cache64_tofit_auto_zpages_record(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zrecord: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a verified page's group asked of
    /// the memo before any view of the page (`Options::zpages_memo_first`
    /// on): the pair against `supdb-cache64tofitautozpages` prices the
    /// order of the memo's look and the page's first touch, which the
    /// instruction count cannot.
    pub fn create_cache64_tofit_auto_zpages_memofirst(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zmemofirst: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the form file's fence searched
    /// through a tree over the entries' words (`Options::zpages_fence_tree`
    /// on): the pair against `supdb-cache64tofitautozpages` prices the tree
    /// on every read, seek and settle through the form.
    pub fn create_cache64_tofit_auto_zpages_fencetree(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zfencetree: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a decode asking nothing ahead for
    /// the memo entry its put displaces (`Options::zpages_memo_ahead` off),
    /// the shape before: the pair against `supdb-cache64tofitautozpages`
    /// prices the hint on every decode through the memo.
    pub fn create_cache64_tofit_auto_zpages_memonoahead(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zmemonoahead: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a scan's start key kept with its
    /// block (`Options::scan_start_cache` at 4,096 entries), a scan starting
    /// where a scan started skipping the partition's seek: the pair against
    /// `supdb-cache64tofitautozpages` prices the cache on those starts.
    pub fn create_cache64_tofit_auto_zpages_startcache(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zstartcache: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a block read through a cursor a
    /// run, a seek a cut and a read by rank a replaced record
    /// (`Options::zpages_block_cursor` off), the shape before a block was
    /// read through one cursor: the pair against
    /// `supdb-cache64tofitautozpages` prices the cursor held across the
    /// block, by the groups a mix decodes and asks the memo for first.
    pub fn create_cache64_tofit_auto_zpages_perrun(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zperrun: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with the memo of decoded groups capped
    /// at four, sixteen or thirty-two mebibytes (`Options::zpages_memo_bytes`)
    /// instead of the engine's eight: the pair against
    /// `supdb-cache64tofitautozpages` counts the decodes a size saves or
    /// costs and prices what a memo hit costs against a decode.
    pub fn create_cache64_tofit_auto_zpages_memo(path: &Path, mib: usize) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zmemo_mib: mib,
                ..Policy::default()
            },
        )
    }

    pub fn create_cache64_tofit_auto_zpages_memo4(path: &Path) -> Res<Supdb> {
        Supdb::create_cache64_tofit_auto_zpages_memo(path, 4)
    }

    pub fn create_cache64_tofit_auto_zpages_memo16(path: &Path) -> Res<Supdb> {
        Supdb::create_cache64_tofit_auto_zpages_memo(path, 16)
    }

    pub fn create_cache64_tofit_auto_zpages_memo32(path: &Path) -> Res<Supdb> {
        Supdb::create_cache64_tofit_auto_zpages_memo(path, 32)
    }

    /// `supdb-cache256tofit` without the trim (`Options::forms_trim_bytes`
    /// zero): against it, the part of the saving that is the allocator's
    /// free pages given back; against `supdb-cache256`, the part that is
    /// the forms' buffers.
    pub fn create_cache256_tofit_notrim(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 256 << 20,
                tofit: true,
                notrim: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache256tofit` without the budget kept after a patch
    /// (`Options::forms_fit_shed` off): against it, the part of the
    /// saving that is the cache held to its budget through a burst.
    pub fn create_cache256_tofit_noshed(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 256 << 20,
                tofit: true,
                noshed: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache256tofit` without the sweep at an operation's end
    /// (`Options::forms_fit_sweep` off): against it, the part of the
    /// saving that is the retired snapshots and forms freed when the
    /// operation that retired them ends and not at the next commit.
    pub fn create_cache256_tofit_nosweep(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 256 << 20,
                tofit: true,
                nosweep: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-settlefar`.
    pub fn create_ingest_settlefar(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                settlefar: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` whose walks prefetch a kilobyte of records rather than
    /// four: against `supdb` it prices the span.
    pub fn create_prefetch1k(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                prefetch1k: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` whose walks prefetch a kilobyte of records rather
    /// than four: against `supdb-ingest` it prices the span.
    pub fn create_ingest_prefetch1k(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                prefetch1k: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` publishing its manifest by a temp file, a rename and a
    /// directory fsync, as before the two slots.
    pub fn create_manifestrename(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                manifestrename: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-manifestrename`.
    pub fn create_ingest_manifestrename(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                manifestrename: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a seal's manifest slot written at its readable landing,
    /// its segments named pending (`Options::manifest_pending`).
    pub fn create_manifestpending(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                manifestpending: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-manifestpending`.
    pub fn create_ingest_manifestpending(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                manifestpending: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with its buffers' pages populated ahead of the fill
    /// (`Options::prefault`).
    pub fn create_populate(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                populate: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-populate`.
    pub fn create_ingest_populate(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                populate: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` writing its segments in 256 KB pieces
    /// (`SegmentOptions::write_piece`).
    pub fn create_piece256(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                piece256: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-piece256`.
    pub fn create_ingest_piece256(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                piece256: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a direct run's records written in three calls each,
    /// as before `Options::direct_put_one`.
    pub fn create_recordsteps(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                recordsteps: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-recordsteps`.
    pub fn create_ingest_recordsteps(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                recordsteps: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a direct run's table copying each value and the commit
    /// re-encoding it, the shape before `Options::direct_stream`.
    pub fn create_copyvals(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                copyvals: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-copyvals`.
    pub fn create_ingest_copyvals(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                copyvals: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a direct run's segment writer keeping its own copy
    /// of every key, the shape before `Options::direct_derive_keys`.
    pub fn create_keycopies(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                keycopies: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-keycopies`.
    pub fn create_ingest_keycopies(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                keycopies: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the finish's slot and Bloom fills placing each key as
    /// it is asked for, the shape before `SegmentOptions::lookahead`.
    pub fn create_fillplain(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                fillplain: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-fillplain`.
    pub fn create_ingest_fillplain(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                fillplain: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a flush's close making the one fsync at the end, the
    /// shape before `Options::close_presync`.
    pub fn create_latesync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                latesync: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-latesync`.
    pub fn create_ingest_latesync(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                latesync: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a direct run's record stream kept whole in its arena,
    /// the shape before `Options::direct_recycle`.
    pub fn create_norecycle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                norecycle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-norecycle`.
    pub fn create_ingest_norecycle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                norecycle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a streamed table keeping a copy of every key, the
    /// shape before `Options::direct_keys_in_stream`.
    pub fn create_keysarena(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                keysarena: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-keysarena`.
    pub fn create_ingest_keysarena(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                keysarena: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the upkeep thread's back-off as it was, the shape
    /// before `Options::upkeep_alert`.
    pub fn create_noalert(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                noalert: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-noalert`.
    pub fn create_ingest_noalert(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                noalert: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with an ordered table keeping a write log, the shape
    /// before `Options::direct_log_range`.
    pub fn create_ordlog(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                ordlog: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-ordlog`.
    pub fn create_ingest_ordlog(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                ordlog: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with an ordered table's entries twenty-four bytes each, the
    /// shape before `Options::direct_slim_entries`.
    pub fn create_fatentries(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                fatentries: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-fatentries`.
    pub fn create_ingest_fatentries(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                fatentries: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a flush waiting for its manifest slot's fdatasync, the
    /// shape before `Options::flush_waits_manifest` was off.
    pub fn create_waitmanifest(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                waitmanifest: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` as `supdb-waitmanifest`.
    pub fn create_ingest_waitmanifest(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                waitmanifest: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` ranking a piece's keys over the partition's records, as
    /// before `Options::piece_ranks_by_heads`: against `supdb` it prices
    /// the ranking over the index's heads, on the segment work's thread.
    pub fn create_rankrecords(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                rankrecords: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` ranking over the records, as `supdb-rankrecords`.
    pub fn create_ingest_rankrecords(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                rankrecords: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-ingest` with the closing fsync writing everything, as
    /// `supdb-latewb`.
    pub fn create_ingest_latewb(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                partition: false,
                durable: false,
                latewb: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with a point read taking a segment's extents by value from
    /// `lookup_full`, as before `Blob::with_lookup` lent them on the
    /// index's frame (`Options::exts_by_value`): against `supdb` it prices
    /// the by-value shape on the point read.
    pub fn create_extsval(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                extsval: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the sweep fencing for the pins through `membarrier`
    /// and the pin a plain store (`Options::asym_pins`): against `supdb`
    /// it prices the fence on every read.
    pub fn create_asympin(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                asympin: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the segment work driven inline by the writer, as
    /// before `Options::publish_in_background`: against `supdb` it prices
    /// the thread that lands the seals and merges.
    pub fn create_inlinemaint(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                inlinemaint: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` maintaining the forms whoever is reading, as `supdb-forms`
    /// does, but stopping where too much of the store is unsealed. The pair
    /// against `supdb` says whether the regime's 3.19x on the lag sweep can
    /// be had without its loss at the deepest lag.
    pub fn create_regime(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                forms: true,
                lagcap: std::env::var("LAGCAP")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(25),
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the writer reading the canonical forms it maintains.
    /// Against `supdb` it prices the half of the mechanism the shipping
    /// arm cannot use: a form is taken only by a handle with a slot, and
    /// the writer's has none.
    pub fn create_wforms(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                wforms: true,
                ..Policy::default()
            },
        )
    }

    pub fn create_nosnap(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                snap: false,
                ..Policy::default()
            },
        )
    }

    /// `sync` fsyncs and seals nothing; reads then answer from the
    /// memtable, the unrouted tail and the partitions together.
    pub fn create_nodrain(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                drain: false,
                ..Policy::default()
            },
        )
    }

    /// `supdb` in every respect but the read advice, which is pinned to the
    /// kernel's plain readahead. The pair differs by one option and needs no
    /// matching.
    pub fn create_noadvice(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                advice: Some(supdb::ReadAdvice::Normal),
                ..Policy::default()
            },
        )
    }

    /// `supdb` in every respect but the block cache, off: the merge on
    /// every scan, the shape before the cache, kept as the comparison arm.
    /// The pair differs by one option and needs no matching.
    pub fn create_nocache(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocache` with a point read of a partition made through its
    /// ordered index rather than its hash (`Options::point_via_ord`):
    /// the pair against `supdb-nocache` differs by that one option and
    /// prices what the hash costs the records' share of the cache out of
    /// core. Chosen by hand, as `ab --arms supdb-nocache,supdb-nocacheordread`.
    pub fn create_nocache_ordread(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                ordread: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocache` with every partition's records also kept in the
    /// second form and a point read or a count of a partition answered
    /// from it (`Options::zpages` forced on): the pair against
    /// `supdb-nocache` differs by that one option and prices what the form
    /// does for a store that does not fit. The form files are built at
    /// `sync`, inside the load window, so this arm's load carries their
    /// build. Chosen by hand, as
    /// `ab --arms supdb-nocache,supdb-nocachezpages`.
    ///
    /// The pair differs by one thing the option does not name: the build
    /// needs the partitions to be final, so this arm's `sync` waits for the
    /// segment work (`Db::zpages_settle` settles before it builds and after),
    /// and `supdb-nocache`'s returns when its flush is answered, with the
    /// manifest slot's fdatasync and the WAL's retirement still to run on the
    /// segment-work thread beside the first reads of its pass. Milliseconds
    /// beside a pass of seconds, and not made equal by making the base arm
    /// wait, which would change an arm the rows are banked on: a pairing is
    /// read with the load's phases beside it (`SUPDB_LOAD_PHASES`), where the
    /// sync is a column of its own.
    pub fn create_nocache_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` whose build leaves the raw records it read in
    /// the page cache instead of dropping them behind itself
    /// (`Options::zpages_drop_behind` off): the pair against
    /// `supdb-nocachezpages` differs by that one option and prices the drop,
    /// which a build pays in `madvise` and `fadvise` calls a window and which
    /// leaves the raw form cold for the scans that follow it. Chosen by
    /// hand, as `ab --arms supdb-nocachezpages,supdb-nocachezpagesnodrop`,
    /// read by the load's phases, where the build is the closing sync.
    pub fn create_nocache_zpages_nodrop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_nodrop: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with the warm of a form file after its
    /// attach off (`Options::zpages_warm` off): the pair against
    /// `supdb-nocachezpages` differs by that one option and prices the
    /// warm, a read of each form file once per process that a store
    /// opened on cold form files pays at its settle. Chosen by hand, as
    /// `ab --arms supdb-nocachezpages,supdb-nocachezpagesnowarm`, with a
    /// read pass after the load and the bound set, where the warm is what
    /// stands between the first reads and a cold file.
    pub fn create_nocache_zpages_nowarm(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_nowarm: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with the memo of decoded groups off
    /// (`Options::zpages_memo` off): the pair against `supdb-nocachezpages`
    /// differs by that one option and prices the memo, the decode a read
    /// of a compressed page skips when the thread decoded the page before.
    /// Chosen by hand, as `ab --arms supdb-nocachezpages,supdb-nocachezpagesnomemo`,
    /// on the point-read passes and the C mix, in core and under a bound.
    pub fn create_nocache_zpages_nomemo(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_nomemo: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with every view of a form file's page checked
    /// and the memo asked after it (`Options::zpages_trusted_views` off),
    /// the shape before: the pair against `supdb-nocachezpages` prices the
    /// trusted view on a point read through the form from the pass's first
    /// read, in core as well as under a bound.
    pub fn create_nocache_zpages_checkedviews(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zcheckedviews: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with the form file's groups holding their
    /// records one by one (`Options::zpages_record_coding` on): the pair
    /// against `supdb-nocachezpages` prices the coding on the forced form,
    /// in core as well as under a bound.
    pub fn create_nocache_zpages_record(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zrecord: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with every group of the form stored verbatim
    /// (`Options::zpages_store_all`), `supdb-nocachezpagesstored`: the pair
    /// prices the LZ4 decode on every read through the form against the
    /// form's size, which the stored one has at the records' own.
    pub fn create_nocache_zpages_stored(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zstored: true,
                ..Policy::default()
            },
        )
    }

    /// The forced form with the block cache and its 64 MB budget kept to fit,
    /// `supdb-cache64tofitzpages`: the combined arm's shape with the form
    /// installed from the start whatever the controller would decide, so
    /// that a coding the controller's size model would refuse can be
    /// priced beside it.
    pub fn create_cache64_tofit_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpages` with every group stored verbatim,
    /// `supdb-cache64tofitzpagesstored`: the E mix's walks over the form
    /// with no decode, the hot head's copies beside them.
    pub fn create_cache64_tofit_zpages_stored(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zstored: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` under the record coding with every tail stored
    /// raw (`Options::zpages_raw_tails`), `supdb-nocachezpagesrecordraw`: the
    /// form that fits the bound where the stored-whole one does not, a
    /// decode a copy a record.
    pub fn create_nocache_zpages_record_raw(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zrawtails: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpages` under the record coding with every tail
    /// stored raw, `supdb-cache64tofitzpagesrecordraw`.
    pub fn create_cache64_tofit_zpages_record_raw(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zrawtails: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesrecordraw` with a scan's groups decoded
    /// whole instead of read in place (`Options::zpages_inplace_walk` off),
    /// `supdb-cache64tofitzpagesrecordrawdecode`: the shape the raw-tail
    /// form had when the scan pass read 0.70x of the block coding's. The
    /// pair against `supdb-cache64tofitzpagesrecordraw` prices the in-place
    /// walk.
    pub fn create_cache64_tofit_zpages_record_raw_decode(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zrawtails: true,
                zdecodewalk: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesrecordraw` with the memo of decoded groups
    /// off and the allocator trimmed once after the form files' install
    /// (`Options::zpages_trim_install`), `supdb-cache64tofitzpagesrecordrawlean`:
    /// the fit check of the raw-tail form at five million keys under
    /// 640 MiB, where the form's 556 MB beside the block cache left the
    /// process's anonymous bytes the margin.
    pub fn create_cache64_tofit_zpages_record_raw_lean(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zrawtails: true,
                zpages_nomemo: true,
                ztrim: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesrecordrawlean` with the trim thread asked to
    /// look at the allocator at every upkeep pass end
    /// (`Options::upkeep_trim_look`), `supdb-cache64tofitzpagesrecordrawleantrim`:
    /// the second fit check of the raw-tail form, the trim where the mixes
    /// are and not before them.
    pub fn create_cache64_tofit_zpages_record_raw_lean_trim(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zrawtails: true,
                zpages_nomemo: true,
                ztrim: true,
                ztrimpass: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesrecordrawleantrim` with the trim thread
    /// counting its stride by the anonymous bytes resident beyond the
    /// allocator's use (`Options::trim_by_resident`),
    /// `supdb-cache64tofitzpagesrecordrawleanresident`: the third fit check
    /// of the raw-tail form, a trim that fires on the pages a dead arena's
    /// re-use brings back, which the free count never sees.
    pub fn create_cache64_tofit_zpages_record_raw_lean_resident(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zrawtails: true,
                zpages_nomemo: true,
                ztrim: true,
                ztrimpass: true,
                ztrimresident: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpages` with its first four partitions by key order
    /// built in the record coding with raw tails and the rest in the block
    /// coding (`Options::zpages_raw_tails_first`),
    /// `supdb-cache64tofitzpageshot4`: the upper bound of a coding chosen per
    /// partition by its heat, where the suite's unscrambled Zipfian puts
    /// three quarters of the E mix's decodes at five million keys.
    pub fn create_cache64_tofit_zpages_hot4(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zhotfirst: 4,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpages` with each partition's raw file and ordered
    /// index dropped from the page cache once its form file serves it
    /// (`Options::zpages_drop_raw`), `supdb-cache64tofitzpagesdrop`: the
    /// room the raw key section and the ordered index held, a fifth of the
    /// files' page cache at five million keys under 640 MiB, given to the
    /// form.
    pub fn create_cache64_tofit_zpages_drop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zdropraw: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpageshot4` with the raw pages dropped behind the
    /// form (`Options::zpages_drop_raw`), `supdb-cache64tofitzpageshot4drop`:
    /// whether the room makes the four raw-tailed partitions fit the
    /// uniform read pass the hot4 arm faulted in.
    pub fn create_cache64_tofit_zpages_hot4_drop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zhotfirst: 4,
                zdropraw: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpageshot4drop` with the first eight partitions
    /// raw-tailed, `supdb-cache64tofitzpageshot8drop`: where the room the
    /// drop gives ends for the raw-tail coding by key order.
    pub fn create_cache64_tofit_zpages_hot8_drop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zhotfirst: 8,
                zdropraw: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpageshot8drop` with every commit near reads
    /// settling its batch into the forms (`Options::forms_settle_every`),
    /// `supdb-cache64tofitzpageshot8dropsettleevery`: the update mixes'
    /// writes filed and their blocks' forms built by their own commits,
    /// on the upkeep thread through the fsync, rather than by ycsb-E's
    /// first scans.
    pub fn create_cache64_tofit_zpages_hot8_drop_settle_every(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zhotfirst: 8,
                zdropraw: true,
                settleevery: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpageshot8dropsettleevery` with the commit's fill
    /// building a form for every block its settle filed into that held
    /// none (`Options::forms_settle_builds`),
    /// `supdb-cache64tofitzpageshot8dropsettlebuilds`: the update mixes'
    /// blocks formed by the commits that dirty them, where the settle
    /// alone patches only the blocks a scan has formed.
    pub fn create_cache64_tofit_zpages_hot8_drop_settle_builds(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zhotfirst: 8,
                zdropraw: true,
                settleevery: true,
                settlebuilds: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesdrop` with its partitions re-coded while
    /// served by what their decodes cost (`Options::zpages_recode`),
    /// `supdb-cache64tofitzpagesrecodedrop`: the raw tails where the
    /// decodes are, to the budget, and given back at the group's refaults --
    /// the policy the key-order arms are the upper bound of.
    pub fn create_cache64_tofit_zpages_recode_drop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zdropraw: true,
                zrecode: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesrecodedrop` with the form file a re-code
    /// replaced kept beside the served one (`Options::zpages_recode_keep`),
    /// `supdb-cache64tofitzpagesrecodekeepdrop`: a partition's re-codes
    /// after its first are the two files exchanged, no build.
    pub fn create_cache64_tofit_zpages_recode_keep_drop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zdropraw: true,
                zrecode: true,
                zrecodekeep: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpagesrecordraw` with the raw pages dropped behind
    /// the form (`Options::zpages_drop_raw`),
    /// `supdb-cache64tofitzpagesrecordrawdrop`: whether the whole raw-tail
    /// form fits the bound the raw key section and the ordered index no
    /// longer share.
    pub fn create_cache64_tofit_zpages_record_raw_drop(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zrawtails: true,
                zdropraw: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitzpageshot4` with the first partition alone
    /// raw-tailed, `supdb-cache64tofitzpageshot1`: half of the E mix's
    /// decodes at five million keys for a quarter of hot4's extra bytes.
    pub fn create_cache64_tofit_zpages_hot1(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages: true,
                zhotfirst: 1,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with a scan's seek of a covered partition by
    /// the ordered index's heads (`Options::zpages_seek_ord` on): the pair
    /// against `supdb-nocachezpages` differs by that one option and prices
    /// the seek by the form file's fence and one group, which touches no raw
    /// page, against the seek by the heads and a key read of the raw
    /// records at each probe. Chosen by hand, as
    /// `ab --arms supdb-nocachezpages,supdb-nocachezpagesheads`, on the
    /// scan passes and the E mix, in core and under a bound.
    pub fn create_nocache_zpages_heads(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_heads: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with a scan's prefetch of the pages it will
    /// cross (`Options::zpages_scan_prefetch` on): the pair against
    /// `supdb-nocachezpages` differs by that one option and prices the
    /// advice, which out of core brings the walk's pages in behind its
    /// first and in core is a syscall for pages that are there. Chosen by
    /// hand, as `ab --arms supdb-nocachezpages,supdb-nocachezpagesprefetch`,
    /// on the scan passes and the E mix under a bound.
    pub fn create_nocache_zpages_prefetch(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_prefetch: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with the overlay walk of a covered partition
    /// cut by rank, as it was before the cursor (`Options::zpages_walk_cursor`
    /// off): the pair against `supdb-nocachezpages` differs by that one
    /// option and prices the cursor, which opens a group once a group
    /// crossed, against the seek, the fresh walk and the read that each
    /// opened the group again at every unsealed key inside a window. Chosen
    /// by hand, as `ab --arms supdb-nocachezpages,supdb-nocachezpagesrankwalk`,
    /// on the E mix and the scan passes, in core and under a bound.
    pub fn create_nocache_zpages_rankwalk(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_rankwalk: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with the page as one group
    /// (`Options::zpages_group_bytes` zero), the shape before the cut: the
    /// pair against `supdb-nocachezpages` differs by that one option and
    /// prices the cut on the point read through the form from the first
    /// read of the pass, which the controller's pair
    /// (`supdb-cache64tofitautozpagesonegroup`) cannot, since under the
    /// controller the form lands after the read passes. Chosen by hand, as
    /// `ab --arms supdb-nocachezpages,supdb-nocachezpagesonegroup`.
    pub fn create_nocache_zpages_onegroup(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zgroup: Some(0),
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocachezpages` with every build left to the settle of the
    /// form rather than started as its partition lands
    /// (`Options::zpages_eager` off): the shape the forced arm's first
    /// sittings were taken with, where the builds are the load's closing
    /// sync. The pair against `supdb-nocachezpages` differs by that one
    /// option and priced the start-at-landing in two sittings (the ordered
    /// load 1.2-1.3x, the shuffled load level), which made it the default.
    /// Chosen by hand, as
    /// `ab --arms supdb-nocachezpages,supdb-nocachezpagessettle`, read by
    /// the load's phases.
    pub fn create_nocache_zpages_settle(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages: true,
                zpages_settle: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the second form the controller's to choose
    /// (`Options::zpages` at `ZPages::Auto`), and nothing else moved:
    /// `supdb-autozpages`. The pair against `supdb` prices the controller
    /// in memory, where it must build nothing and the pair must read
    /// level, and out of core, where it must reach the form and the cells
    /// after the entry read against the forced arm's. Chosen by hand, as
    /// `ab --arms supdb,supdb-autozpages`.
    pub fn create_auto_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                zpages_auto: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocache` with the second form the controller's to choose:
    /// `supdb-nocacheautozpages`, the pair against `supdb-nocache` and
    /// against `supdb-nocachezpages`, which forces what this arm decides.
    pub fn create_nocache_auto_zpages(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages_auto: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocacheautozpages` with the form's threads at the readers'
    /// I/O class (`Options::zpages_io_idle` off): the pair against
    /// `supdb-nocacheautozpages` differs by that one option and prices,
    /// under a bound, a build that shares the device with the readers'
    /// faults against one the device serves after them. Chosen by hand,
    /// as `ab --arms supdb-nocacheautozpages,supdb-nocacheautozpagesfairio
    /// --bound ...`.
    pub fn create_nocache_auto_zpages_fairio(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages_auto: true,
                zpages_fairio: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocacheautozpages` with its builds asking nothing of the raw
    /// records ahead of their walks (`Options::zpages_build_ahead` at zero,
    /// the shape the window was priced against): the pair against
    /// `supdb-nocacheautozpages` differs by that one option and prices,
    /// under a bound, a build whose waits fetch the device's readahead
    /// against one whose waits fetch a window each. Chosen by hand, as
    /// `ab --arms supdb-nocacheautozpages,supdb-nocacheautozpagesnoahead
    /// --bound ...`.
    pub fn create_nocache_auto_zpages_noahead(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages_auto: true,
                zpages_noahead: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-nocacheautozpages` with the eager entry
    /// (`Options::zpages_build_eager`): the controller buys the form at
    /// SUSPECT on the model's prediction once the sampler has priced it
    /// and the partitions have stood for the settle, instead of after the
    /// rent. The pair against `supdb-nocacheautozpages` differs by that one
    /// option and prices, under a bound, the read passes the rent's wait
    /// leaves raw against what an eager build costs the load. Chosen by
    /// hand, as `ab --arms supdb-nocacheautozpages,supdb-nocacheautozpageseager
    /// --bound ...`.
    pub fn create_nocache_auto_zpages_eager(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                block_cache: false,
                zpages_auto: true,
                zpages_buildeager: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb` with the block cache bounded at 256 MB, the budget
    /// `rocksdb-tuned` runs its block cache on, so that comparison is on
    /// the same memory. Below the bound the two arms are one.
    pub fn create_cache256(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 256 << 20,
                ..Policy::default()
            },
        )
    }

    fn with_policy(path: &Path, policy: Policy) -> Res<Supdb> {
        let Policy {
            partition,
            drain,
            advice,
            block_cache,
            durable,
            budget,
            forms,
            snap,
            settle,
            wforms,
            lagcap,
            sealcap,
            l0,
            nopin,
            asympin,
            extsval,
            piecesonly,
            latewb,
            inlinemaint,
            copysearch,
            convertsources,
            copyslack,
            drainpromote,
            manifestfree,
            oidxsync,
            refvals,
            rowatclose,
            lendbursts,
            inlineall,
            lazysettle,
            walsync,
            nodrainsnap,
            sealrun,
            closeinline,
            timers,
            wake,
            noprefetch,
            dirplain,
            settlefar,
            tofit,
            notrim,
            noshed,
            nosweep,
            prefetch1k,
            manifestrename,
            manifestpending,
            waitmanifest,
            populate,
            piece256,
            recordsteps,
            copyvals,
            keycopies,
            fillplain,
            latesync,
            norecycle,
            keysarena,
            noalert,
            ordlog,
            fatentries,
            rankrecords,
            shape,
            aheadmin,
            lazyforms,
            eager,
            recent,
            carry,
            rebuild,
            snapcarry,
            lazysnap,
            fullrec,
            norebase,
            sealfile,
            settleall,
            upkeep,
            tier,
            runs,
            keeper,
            aheadpub,
            pubalways,
            leave,
            adaptcap,
            adapttrig,
            rotate,
            defer,
            firstfloor,
            nofrozen,
            idle,
            sortover,
            pmd,
            settlefreeze,
            convertcap,
            shortpass,
            noyield,
            postedyield,
            behindyield,
            bg,
            bglag,
            ordread,
            zpages,
            zpages_nodrop,
            zpages_nowarm,
            zpages_nomemo,
            zpages_heads,
            zpages_prefetch,
            zpages_rankwalk,
            zpages_settle,
            zpages_auto,
            zpages_fairio,
            zpages_noahead,
            zpages_buildeager,
            zcacheraw,
            zcodingauto,
            zpromote,
            zgroup,
            zprebuild,
            zspan,
            zmemolinear,
            zexactcut,
            zcheckedviews,
            zrecord,
            zstored,
            zrawtails,
            ztrim,
            zdecodewalk,
            ztrimpass,
            ztrimresident,
            zhotfirst,
            zdropraw,
            zrecode,
            zrecodekeep,
            settleevery,
            settlebuilds,
            zmemofirst,
            zfencetree,
            zmemonoahead,
            zstartcache,
            zperrun,
            zmemo_mib,
        } = policy;
        // What the engine ships, so an arm that pins nothing inherits it
        // rather than restating it and drifting from it.
        let base_seal_max_pct = supdb::Options::default().seal_max_pct;
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        // Checksums off in the segments, because LMDB has none and the axis
        // is equalizable -- the same call `supdb-durable` makes, and the
        // fairness gate refused to rank this arm until it was made here too.
        let opts = supdb::Options {
            segment: supdb::SegmentOptions {
                checksums: false,
                compact_records: !fullrec,
                early_writeback: !latewb,
                sync_ahead: if latewb {
                    0
                } else {
                    supdb::SegmentOptions::default().sync_ahead
                },
                write_piece: if pmd {
                    2 << 20
                } else if piece256 {
                    256 << 10
                } else {
                    supdb::SegmentOptions::default().write_piece
                },
                row_on_write: !rowatclose,
                lookahead: !fillplain,
                ..Default::default()
            },
            // The engine's own defaults: 32 MB seals over 64 MB partitions,
            // the seal growing with the store past that floor as the
            // engine's default has it, since a fixed seal at thirty million
            // keys left level-0 at 24 pieces a range.
            // Few partitions is not an accident of the benchmark, it is the
            // operating point the design is FOR: measured, the same data read
            // at 1.19x of LMDB in one segment and 0.77x spread over eight,
            // and the seal-size sweep found seal size and partition size had to
            // be set apart -- 32 MB seals ingest 1.129x over 64 MB at the
            // same device bytes once the partitions stay at 64 MB, where
            // coupled they multiplied and cost every read. An 8 MB seal was
            // tried here once to make the level machinery work harder, and
            // all it measured was the engine at a shape it should not be
            // run in.
            seal_bytes: 32 << 20,
            partition_bytes: Some(64 << 20),
            // SUPDB_NO_FLUSH_PARTITION trades the read lead for ingest:
            // the flush stops partitioning what it sealed and leaves that
            // to background compaction. Both arms are measured rather than
            // argued about.
            partition_on_flush: partition,
            // The option and nothing beside it: the budget stays at the
            // engine's default, so the arm's memory over the ladder is the
            // option's own and a row's figure describes what a user gets.
            scan_block_cache: block_cache,
            scan_cache_bytes: budget,
            // Always on now; what varies is when the writer acts on it,
            // which `forms_from_readers` below says.
            commit_forms: true,
            // The sorted unsealed keys published in the state and carried
            // forward, or each handle's own sorted from nothing: the
            // engine's arm, priced here. `snapshot_adopt_behind` stays at
            // the engine's default, since a run short of current is
            // carried forward rather than taken as it stands.
            share_snapshot: snap,
            // Zero maintains the forms at every commit whoever is
            // reading; the engine's default waits for a caller's handle.
            // `supdb` inherits what the engine ships; `supdb-lazyforms`
            // pins the one that waited for a caller's handle.
            forms_from_reader_scans: match (forms, lazyforms) {
                (_, true) => 1,
                (true, _) => 0,
                _ => supdb::Options::default().forms_from_reader_scans,
            },
            commit_forms_build: !settle,
            // The writer's own reads take the forms it maintains. Off in
            // the shipping arm until the pair is measured: the suite's
            // ycsb-E reads through the writer, so it pays for the
            // maintenance and today cannot read it.
            forms_to_writer: wforms,
            // Settle every commit, so a write burst never leaves its
            // filing to the first read after it.
            forms_settle_backlog_pct: eager
                .unwrap_or(supdb::Options::default().forms_settle_backlog_pct),
            // The window within which the bound settles at all: since the
            // last scan, as a share of the store's keys written.
            forms_settle_recent_pct: recent
                .unwrap_or(supdb::Options::default().forms_settle_recent_pct),
            // The forms carried across a seal, or the table started
            // afresh at every publish as it was.
            forms_carry: carry,
            freeze_settles: settlefreeze,
            copy_start_at_rank: !copysearch,
            convert_from_form: !convertsources,
            copy_exact: !copyslack,
            drain_names_partition: !drainpromote,
            manifest_spare: !manifestfree,
            ord_durable: oidxsync,
            direct_table_refs: refvals,
            piece_ranks_by_heads: !rankrecords,
            forms_convert_unread: !(bg || bglag),
            // The capped conversions and the builder's, kept to price again
            // once the thread keeps up with the writer.
            forms_convert_cap: if convertcap {
                CONVERT_CAP
            } else {
                supdb::Options::default().forms_convert_cap
            },
            ahead_converts: convertcap,
            forms_convert_yield: !noyield,
            forms_convert_behind: if postedyield {
                0
            } else if behindyield {
                1000
            } else {
                supdb::Options::default().forms_convert_behind
            },
            // The bounded pass, at the share the square-root rule of the
            // cost model gives at these rungs.
            upkeep_pass_pct: if shortpass {
                SHORT_PASS_PCT
            } else {
                supdb::Options::default().upkeep_pass_pct
            },
            // The thread trails a burst by at most a sixth of the store's
            // keys with reads around, and begins a pass for no fewer than
            // a twelfth: the bound and the batch of the cost model, as
            // the shares the sweep at a hundred thousand keys settled on.
            upkeep_lag_pct: if bglag { 16 } else { 0 },
            upkeep_batch_pct: if bglag { 8 } else { 0 },
            forms_rebase: !norebase,
            // The seal sized by the file, as it was before the store's
            // payload was recorded.
            seal_on_file: sealfile,
            // The settle bound on every segment's keys, as it was.
            forms_settle_keys_all: settleall,
            // The upkeep where the arm puts it, or where the engine does.
            upkeep: upkeep.unwrap_or(supdb::Options::default().upkeep),
            upkeep_inline_bursts: !lendbursts,
            upkeep_inline_min: if inlineall {
                1
            } else {
                supdb::Options::default().upkeep_inline_min
            },
            upkeep_settle_after_scans: !lazysettle,
            drain_syncs_wal: walsync,
            drain_snapshot: !nodrainsnap,
            seal_put_one: !sealrun,
            close_finish_inline: closeinline,
            upkeep_wake_parked: wake,
            scan_prefetch: !noprefetch,
            seek_prefetch_dir: !dirplain,
            forms_settle_far: settlefar,
            forms_to_fit: tofit,
            // The resident count's arm takes a stride of eight megabytes:
            // at the default's thirty-two the count read 15-25 MB over its
            // post-install floor through every mix at five million keys
            // under 640 MiB and fired once over the store's life.
            forms_trim_bytes: if notrim {
                0
            } else if ztrimresident {
                8 << 20
            } else {
                supdb::Options::default().forms_trim_bytes
            },
            forms_fit_shed: !noshed,
            forms_fit_sweep: !nosweep,
            scan_prefetch_bytes: if prefetch1k { 1024 } else { 4096 },
            manifest_slots: !manifestrename,
            manifest_pending: manifestpending,
            flush_waits_manifest: waitmanifest,
            close_presync: !latesync,
            prefault: populate,
            direct_put_one: !recordsteps,
            direct_stream: !copyvals && !refvals,
            direct_derive_keys: !keycopies,
            direct_recycle: !norecycle,
            direct_keys_in_stream: !keysarena,
            upkeep_alert: !noalert,
            direct_log_range: !ordlog,
            direct_slim_entries: !fatentries,
            // The scan phases timed where the arm asks, or where the
            // counters lines will read them: ten clock reads a scan.
            scan_timers: timers
                || std::env::var_os("SUPDB_LAG_COUNTERS").is_some()
                || std::env::var_os("SUPDB_MIX_COUNTERS").is_some(),
            forms_settle_rebuild_from: rebuild,
            snapshot_carry: snapcarry,
            scan_lazy_snapshot: lazysnap,
            // The pieces over a range merged into one piece beside the
            // partition merge, or left for it as the engine has it.
            tier_pieces: tier.unwrap_or(supdb::Options::default().tier_pieces),
            snapshot_runs: runs,
            snapshot_keeper: keeper,
            // The builder started by a publish and installed by a commit,
            // or started by a scan as it is.
            build_ahead_on_publish: aheadpub,
            // The forms published for a handle, or at every maintained
            // commit as they were.
            forms_publish_lazily: !pubalways,
            flush_schedules: !leave,
            // The unsealed share past which the maintenance stops, so an
            // arm can maintain whoever is reading without paying for it
            // on a store that is nearly all unmerged.
            forms_max_unsealed_pct: lagcap,
            // The seal capped by a share of the store, so the memtable
            // cannot hold the whole of a store smaller than the floor.
            seal_max_pct: sealcap.unwrap_or(base_seal_max_pct),
            l0_trigger: l0.unwrap_or(supdb::Options::default().l0_trigger),
            writer_pins: !nopin,
            asym_pins: asympin,
            exts_by_value: extsval,
            point_via_ord: ordread,
            // The second form forced on for the arm that prices it, and
            // whatever the engine ships for every other.
            zpages: if zpages {
                supdb::ZPages::Always
            } else if zpages_auto {
                supdb::ZPages::Auto
            } else {
                supdb::Options::default().zpages
            },
            zpages_drop_behind: if zpages_nodrop {
                false
            } else {
                supdb::Options::default().zpages_drop_behind
            },
            zpages_warm: if zpages_nowarm {
                false
            } else {
                supdb::Options::default().zpages_warm
            },
            zpages_memo: if zpages_nomemo {
                false
            } else {
                supdb::Options::default().zpages_memo
            },
            zpages_io_idle: if zpages_fairio {
                false
            } else {
                supdb::Options::default().zpages_io_idle
            },
            zpages_build_ahead: if zpages_noahead {
                0
            } else {
                supdb::Options::default().zpages_build_ahead
            },
            zpages_build_eager: zpages_buildeager,
            zpages_block_cache: !zcacheraw,
            zpages_coding_auto: zcodingauto,
            promote_entries: zpromote,
            zpages_promote_copies: zpromote > 0,
            zpages_group_bytes: zgroup.unwrap_or(supdb::Options::default().zpages_group_bytes),
            zpages_prebuild: zprebuild,
            zpages_seek_span: zspan,
            zpages_memo_hash: !zmemolinear,
            zpages_settle_span: !zexactcut,
            zpages_trusted_views: !zcheckedviews,
            zpages_store_all: zstored,
            zpages_raw_tails: zrawtails,
            zpages_trim_install: ztrim,
            zpages_inplace_walk: !zdecodewalk,
            upkeep_trim_look: ztrimpass,
            trim_by_resident: ztrimresident,
            zpages_raw_tails_first: zhotfirst,
            zpages_drop_raw: zdropraw,
            zpages_recode: zrecode,
            zpages_recode_keep: zrecodekeep,
            forms_settle_every: settleevery,
            forms_settle_builds: settlebuilds,
            zpages_record_coding: zrecord || zrawtails,
            zpages_memo_first: zmemofirst,
            zpages_fence_tree: zfencetree,
            zpages_memo_ahead: !zmemonoahead,
            scan_start_cache: if zstartcache { 4096 } else { 0 },
            zpages_block_cursor: !zperrun,
            zpages_memo_bytes: if zmemo_mib > 0 {
                zmemo_mib << 20
            } else {
                supdb::Options::default().zpages_memo_bytes
            },
            zpages_seek_ord: if zpages_heads {
                true
            } else {
                supdb::Options::default().zpages_seek_ord
            },
            zpages_scan_prefetch: if zpages_prefetch {
                true
            } else {
                supdb::Options::default().zpages_scan_prefetch
            },
            zpages_walk_cursor: if zpages_rankwalk {
                false
            } else {
                supdb::Options::default().zpages_walk_cursor
            },
            zpages_eager: if zpages_settle {
                false
            } else {
                supdb::Options::default().zpages_eager
            },
            forms_pieces_only: piecesonly,
            // Unpinned, the writer must be the one publisher, as it was
            // when the pins were priced: the store refuses the pair.
            publish_in_background: !(inlinemaint || nopin),
            adaptive_shape: shape,
            // The cap and the trigger following the reads, or the cap
            // as it stands whoever reads; the trigger arm takes both.
            adaptive_cap: adaptcap.is_some() || adapttrig,
            cap_reading_pct: adaptcap.unwrap_or(supdb::Options::default().cap_reading_pct),
            adaptive_trigger: adapttrig,
            seal_rotates_wal: rotate,
            frozen_snaps: !nofrozen,
            overlay_merge: !sortover,
            seal_defers: defer,
            seal_idle: idle,
            seal_first_floor: firstfloor,
            // Below this the builder declines and the writer fills the
            // forms inline on the commit path instead, which is the
            // comparison the threshold was never measured against.
            scan_cache_ahead_min_blocks: aheadmin
                .unwrap_or(supdb::Options::default().scan_cache_ahead_min_blocks),
            // The guarantee the arm's row names, not the engine's default:
            // durable per batch, or frames written per commit and fsynced
            // at `sync`, which is how `lmdb-nosync` and `rocksdb-nosync`
            // buffer.
            sync: if durable {
                supdb::SyncPolicy::Always
            } else {
                supdb::SyncPolicy::EveryN(u32::MAX)
            },
            ..Default::default()
        };
        let opts = match advice {
            Some(a) => supdb::Options {
                read_advice: a,
                ..opts
            },
            None => opts,
        };
        let db = supdb::Db::create(path, opts).map_err(|e| e.to_string())?;
        Ok(Supdb {
            db: Some(db),
            path: path.to_path_buf(),
            partition,
            drain,
            advice,
            block_cache,
            durable,
            budget,
            forms,
            snap,
            settle,
            wforms,
            lagcap,
            sealcap,
            l0,
            nopin,
            asympin,
            extsval,
            piecesonly,
            latewb,
            inlinemaint,
            copysearch,
            convertsources,
            copyslack,
            drainpromote,
            manifestfree,
            oidxsync,
            refvals,
            rowatclose,
            lendbursts,
            inlineall,
            lazysettle,
            walsync,
            nodrainsnap,
            sealrun,
            closeinline,
            timers,
            wake,
            noprefetch,
            dirplain,
            settlefar,
            tofit,
            notrim,
            noshed,
            nosweep,
            prefetch1k,
            manifestrename,
            manifestpending,
            waitmanifest,
            populate,
            piece256,
            recordsteps,
            copyvals,
            keycopies,
            fillplain,
            latesync,
            norecycle,
            keysarena,
            noalert,
            ordlog,
            fatentries,
            rankrecords,
            shape,
            aheadmin,
            lazyforms,
            eager,
            recent,
            carry,
            rebuild,
            snapcarry,
            lazysnap,
            fullrec,
            norebase,
            sealfile,
            settleall,
            upkeep,
            tier,
            runs,
            keeper,
            aheadpub,
            pubalways,
            leave,
            adaptcap,
            adapttrig,
            rotate,
            defer,
            firstfloor,
            nofrozen,
            idle,
            sortover,
            pmd,
            settlefreeze,
            convertcap,
            shortpass,
            noyield,
            postedyield,
            behindyield,
            bg,
            bglag,
            ordread,
            zpages,
            zpages_nodrop,
            zpages_nowarm,
            zpages_nomemo,
            zpages_heads,
            zpages_prefetch,
            zpages_rankwalk,
            zpages_settle,
            zpages_auto,
            zpages_fairio,
            zpages_noahead,
            zpages_buildeager,
            zcacheraw,
            zcodingauto,
            zpromote,
            zgroup,
            zprebuild,
            zspan,
            zmemolinear,
            zexactcut,
            zcheckedviews,
            zrecord,
            zstored,
            zrawtails,
            ztrim,
            zdecodewalk,
            ztrimpass,
            ztrimresident,
            zhotfirst,
            zdropraw,
            zrecode,
            zrecodekeep,
            settleevery,
            settlebuilds,
            zmemofirst,
            zfencetree,
            zmemonoahead,
            zstartcache,
            zperrun,
            zmemo_mib,
            ord_threads: Default::default(),
        })
    }
}

thread_local! {
    /// Where a read puts the value it consumes: one per thread, reused
    /// across calls, so a threaded pass copies into buffers no other
    /// thread writes and no call allocates once the first has grown it.
    static SINK: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Consume a value as a caller does: every byte copied out, into this
/// thread's buffer, where `black_box` keeps the copy from being dropped.
/// Every read and scan of every arm goes through it.
///
/// The reads once summed the lengths of the values they were lent and
/// never read a byte of one, which priced finding a value and not using
/// it, and no caller gets a value to leave it unread. It also priced the
/// layouts unevenly: an LMDB leaf keeps a key beside its value, so the
/// first touch of the value lands on lines the lookup already loaded,
/// while supdb keeps its values in blocks apart from the index, where
/// that touch is a miss of its own -- measured outside the suite, a
/// point read that consumed the value at a hundred bytes read 1.6x of
/// LMDB where the row's read said 2x. The copy is the same bytes on every
/// arm, so what it adds to one arm and not another is where they lay.
#[inline]
fn consume(v: &[u8]) -> usize {
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        s.clear();
        s.extend_from_slice(v);
        std::hint::black_box(s.as_slice());
    });
    v.len()
}

/// One read, whichever handle answers it: the writer's own in `Engine`,
/// under its read-your-writes isolation, or a thread's from `Db::reader`.
fn supdb_get(r: &supdb::Reader, key: &[u8]) -> Res<usize> {
    let mut n = 0usize;
    r.read_all(key, |v| n += consume(v))
        .map_err(|e| e.to_string())?;
    Ok(n)
}

fn supdb_range(r: &supdb::Reader, from: &[u8], n: usize) -> Res<usize> {
    let mut bytes = 0usize;
    r.scan(from, n, |_k, v| bytes += consume(v))
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

impl ThreadReader for supdb::Reader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        supdb_get(self, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        supdb_range(self, from, n)
    }
}

impl ThreadReader for TallyReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        supdb_get(&self.r, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        supdb_range(&self.r, from, n)
    }
}

impl Engine for Supdb {
    fn name(&self) -> &'static str {
        // Before the budget branch, which would name the policy the cache
        // arm whatever else it carries.
        if self.budget > 0 && self.tofit && self.zpages_auto {
            if self.budget == 128 << 20 {
                return "supdb-cache128tofitautozpages";
            }
            if self.budget == 256 << 20 {
                return "supdb-cache256tofitautozpages";
            }
            return if self.zcacheraw {
                "supdb-cache64tofitautozpagesraw"
            } else if self.zpromote > 0 {
                "supdb-cache64tofitautozpagespromote"
            } else if self.zprebuild {
                "supdb-cache64tofitautozpagesprebuild"
            } else if self.zspan {
                "supdb-cache64tofitautozpagesspan"
            } else if self.zmemolinear {
                "supdb-cache64tofitautozpagesmemolinear"
            } else if self.zexactcut {
                "supdb-cache64tofitautozpagesexactcut"
            } else if self.zcheckedviews {
                "supdb-cache64tofitautozpagescheckedviews"
            } else if self.zrecord {
                "supdb-cache64tofitautozpagesrecord"
            } else if self.zcodingauto {
                "supdb-cache64tofitautozpagescodingauto"
            } else if self.zmemofirst {
                "supdb-cache64tofitautozpagesmemofirst"
            } else if self.zfencetree {
                "supdb-cache64tofitautozpagesfencetree"
            } else if self.zmemonoahead {
                "supdb-cache64tofitautozpagesmemonoahead"
            } else if self.zstartcache {
                "supdb-cache64tofitautozpagesstartcache"
            } else if self.zperrun {
                "supdb-cache64tofitautozpagesperrun"
            } else if self.zmemo_mib == 4 {
                "supdb-cache64tofitautozpagesmemo4"
            } else if self.zmemo_mib == 16 {
                "supdb-cache64tofitautozpagesmemo16"
            } else if self.zmemo_mib == 32 {
                "supdb-cache64tofitautozpagesmemo32"
            } else {
                match self.zgroup {
                    Some(0) => "supdb-cache64tofitautozpagesonegroup",
                    Some(1024) => "supdb-cache64tofitautozpagesgroup1k",
                    _ => "supdb-cache64tofitautozpages",
                }
            };
        }
        if self.budget == 64 << 20 && self.tofit && self.zpages {
            return if self.zstored {
                "supdb-cache64tofitzpagesstored"
            } else if self.zrecodekeep && self.zrecode && self.zdropraw {
                "supdb-cache64tofitzpagesrecodekeepdrop"
            } else if self.zrecode && self.zdropraw {
                "supdb-cache64tofitzpagesrecodedrop"
            } else if self.zhotfirst == 8 && self.zdropraw && self.settlebuilds {
                "supdb-cache64tofitzpageshot8dropsettlebuilds"
            } else if self.zhotfirst == 8 && self.zdropraw && self.settleevery {
                "supdb-cache64tofitzpageshot8dropsettleevery"
            } else if self.zhotfirst == 8 && self.zdropraw {
                "supdb-cache64tofitzpageshot8drop"
            } else if self.zrawtails && self.zdropraw {
                "supdb-cache64tofitzpagesrecordrawdrop"
            } else if self.zhotfirst == 4 && self.zdropraw {
                "supdb-cache64tofitzpageshot4drop"
            } else if self.zhotfirst == 4 {
                "supdb-cache64tofitzpageshot4"
            } else if self.zdropraw {
                "supdb-cache64tofitzpagesdrop"
            } else if self.zhotfirst == 1 {
                "supdb-cache64tofitzpageshot1"
            } else if self.zrawtails
                && self.zpages_nomemo
                && self.ztrim
                && self.ztrimpass
                && self.ztrimresident
            {
                "supdb-cache64tofitzpagesrecordrawleanresident"
            } else if self.zrawtails && self.zpages_nomemo && self.ztrim && self.ztrimpass {
                "supdb-cache64tofitzpagesrecordrawleantrim"
            } else if self.zrawtails && self.zpages_nomemo && self.ztrim {
                "supdb-cache64tofitzpagesrecordrawlean"
            } else if self.zrawtails && self.zdecodewalk {
                "supdb-cache64tofitzpagesrecordrawdecode"
            } else if self.zrawtails {
                "supdb-cache64tofitzpagesrecordraw"
            } else {
                "supdb-cache64tofitzpages"
            };
        }
        if self.budget == 64 << 20 && self.tofit {
            return "supdb-cache64tofit";
        }
        if self.budget > 0 {
            return if self.tofit && self.notrim {
                "supdb-cache256tofitnotrim"
            } else if self.tofit && self.noshed {
                "supdb-cache256tofitnoshed"
            } else if self.tofit && self.nosweep {
                "supdb-cache256tofitnosweep"
            } else if self.tofit {
                "supdb-cache256tofit"
            } else {
                "supdb-cache256"
            };
        }
        if self.firstfloor {
            return "supdb-shapefloor";
        }
        if self.defer {
            return "supdb-shapedefer";
        }
        if self.idle {
            return if self.shape {
                "supdb-shapeidle"
            } else {
                "supdb-idle"
            };
        }
        if self.pmd {
            return if self.partition {
                "supdb-pmd"
            } else {
                "supdb-ingestpmd"
            };
        }
        if self.settlefreeze {
            return if self.partition {
                "supdb-settlefreeze"
            } else {
                "supdb-ingestsettlefreeze"
            };
        }
        if self.convertcap {
            return if self.partition {
                "supdb-convertcap"
            } else {
                "supdb-ingestconvertcap"
            };
        }
        if self.shortpass {
            return if self.partition {
                "supdb-shortpass"
            } else {
                "supdb-ingestshortpass"
            };
        }
        if self.noyield {
            return if self.partition {
                "supdb-noyield"
            } else {
                "supdb-ingestnoyield"
            };
        }
        if self.postedyield {
            return if self.partition {
                "supdb-postedyield"
            } else {
                "supdb-ingestpostedyield"
            };
        }
        if self.behindyield {
            return if self.partition {
                "supdb-behindyield"
            } else {
                "supdb-ingestbehindyield"
            };
        }
        if self.bg {
            return if self.partition {
                "supdb-bg"
            } else {
                "supdb-ingestbg"
            };
        }
        if self.bglag {
            return if self.partition {
                "supdb-bglag"
            } else {
                "supdb-ingestbglag"
            };
        }
        if self.sortover {
            return if self.shape {
                "supdb-shapesortover"
            } else {
                "supdb-sortover"
            };
        }
        if self.nofrozen {
            return if self.shape {
                "supdb-shapenofrozen"
            } else {
                "supdb-nofrozen"
            };
        }
        if self.rotate {
            return if self.shape {
                "supdb-shaperotate"
            } else {
                "supdb-rotate"
            };
        }
        if self.settle {
            return "supdb-settle";
        }
        if !self.carry {
            return "supdb-nocarry";
        }
        if self.rebuild > 0 {
            return if self.partition {
                "supdb-rebuild"
            } else {
                "supdb-ingestrebuild"
            };
        }
        if self.snapcarry {
            return "supdb-snapcarry";
        }
        if self.lazysnap {
            return "supdb-lazysnap";
        }
        if self.fullrec {
            return "supdb-fullrec";
        }
        if self.norebase {
            return "supdb-norebase";
        }
        if self.sealfile {
            return "supdb-sealfile";
        }
        if self.settleall {
            return "supdb-settleall";
        }
        match self.upkeep {
            Some(supdb::Upkeep::Background(2)) => {
                return if self.partition {
                    "supdb-hold"
                } else {
                    "supdb-ingesthold"
                };
            }
            Some(_) => return "supdb-inline",
            None => {}
        }
        if self.tier.is_some_and(|t| t > 0) {
            return "supdb-tier";
        }
        if self.keeper {
            return "supdb-keeper";
        }
        if self.runs {
            return "supdb-runs";
        }
        if self.aheadpub {
            return "supdb-aheadpub";
        }
        if self.leave {
            return "supdb-ingestleave";
        }
        if self.pubalways {
            return "supdb-pubalways";
        }
        if self.recent.is_some() {
            return "supdb-recency";
        }
        match self.eager {
            Some(0) => return "supdb-nosettle",
            Some(_) => return "supdb-eager",
            None => {}
        }
        if self.lazyforms {
            return "supdb-lazyforms";
        }
        if self.aheadmin.is_some() {
            return "supdb-ahead";
        }
        if self.l0.is_some() {
            return "supdb-l0";
        }
        if self.nopin {
            return "supdb-nopin";
        }
        if self.asympin {
            return "supdb-asympin";
        }
        if self.extsval {
            return "supdb-extsval";
        }
        if self.piecesonly {
            return if self.partition {
                "supdb-piecesonly"
            } else {
                "supdb-ingestpiecesonly"
            };
        }
        if self.latewb {
            return if self.partition {
                "supdb-latewb"
            } else {
                "supdb-ingestlatewb"
            };
        }
        if self.copysearch {
            return if self.partition {
                "supdb-copysearch"
            } else {
                "supdb-ingestcopysearch"
            };
        }
        if self.convertsources {
            return if self.partition {
                "supdb-convertsources"
            } else {
                "supdb-ingestconvertsources"
            };
        }
        if self.copyslack {
            return if self.partition {
                "supdb-copyslack"
            } else {
                "supdb-ingestcopyslack"
            };
        }
        if self.drainpromote {
            return "supdb-ingestdrainpromote";
        }
        if self.manifestfree {
            return if self.partition {
                "supdb-manifestfree"
            } else {
                "supdb-ingestmanifestfree"
            };
        }
        if self.oidxsync {
            return if self.partition {
                "supdb-oidxsync"
            } else {
                "supdb-ingestoidxsync"
            };
        }
        if self.refvals {
            return if self.partition {
                "supdb-refvals"
            } else {
                "supdb-ingestrefvals"
            };
        }
        if self.rowatclose {
            return if self.partition {
                "supdb-rowatclose"
            } else {
                "supdb-ingestrowatclose"
            };
        }
        if self.lendbursts {
            return if self.partition {
                "supdb-lendbursts"
            } else {
                "supdb-ingestlendbursts"
            };
        }
        if self.inlineall {
            return if self.partition {
                "supdb-inlineall"
            } else {
                "supdb-ingestinlineall"
            };
        }
        if self.lazysettle {
            return if self.partition {
                "supdb-lazysettle"
            } else {
                "supdb-ingestlazysettle"
            };
        }
        if self.walsync {
            return if self.partition {
                "supdb-walsync"
            } else {
                "supdb-ingestwalsync"
            };
        }
        if self.nodrainsnap {
            return if self.partition {
                "supdb-nodrainsnap"
            } else {
                "supdb-ingestnodrainsnap"
            };
        }
        if self.sealrun {
            return if self.partition {
                "supdb-sealrun"
            } else {
                "supdb-ingestsealrun"
            };
        }
        if self.closeinline {
            return if self.partition {
                "supdb-closeinline"
            } else {
                "supdb-ingestcloseinline"
            };
        }
        if self.timers {
            return if self.partition {
                "supdb-timers"
            } else {
                "supdb-ingesttimers"
            };
        }
        if self.wake {
            return if self.partition {
                "supdb-wake"
            } else {
                "supdb-ingestwake"
            };
        }
        if self.noprefetch {
            return if self.partition {
                "supdb-noprefetch"
            } else {
                "supdb-ingestnoprefetch"
            };
        }
        if self.dirplain {
            return if self.partition {
                "supdb-dirplain"
            } else {
                "supdb-ingestdirplain"
            };
        }
        if self.settlefar {
            return if self.partition {
                "supdb-settlefar"
            } else {
                "supdb-ingestsettlefar"
            };
        }
        if self.tofit {
            return if self.partition {
                "supdb-tofit"
            } else {
                "supdb-ingesttofit"
            };
        }
        if self.prefetch1k {
            return if self.partition {
                "supdb-prefetch1k"
            } else {
                "supdb-ingestprefetch1k"
            };
        }
        if self.manifestrename {
            return if self.partition {
                "supdb-manifestrename"
            } else {
                "supdb-ingestmanifestrename"
            };
        }
        if self.manifestpending {
            return if self.partition {
                "supdb-manifestpending"
            } else {
                "supdb-ingestmanifestpending"
            };
        }
        if self.populate {
            return if self.partition {
                "supdb-populate"
            } else {
                "supdb-ingestpopulate"
            };
        }
        if self.piece256 {
            return if self.partition {
                "supdb-piece256"
            } else {
                "supdb-ingestpiece256"
            };
        }
        if self.recordsteps {
            return if self.partition {
                "supdb-recordsteps"
            } else {
                "supdb-ingestrecordsteps"
            };
        }
        if self.copyvals {
            return if self.partition {
                "supdb-copyvals"
            } else {
                "supdb-ingestcopyvals"
            };
        }
        if self.keycopies {
            return if self.partition {
                "supdb-keycopies"
            } else {
                "supdb-ingestkeycopies"
            };
        }
        if self.fillplain {
            return if self.partition {
                "supdb-fillplain"
            } else {
                "supdb-ingestfillplain"
            };
        }
        if self.latesync {
            return if self.partition {
                "supdb-latesync"
            } else {
                "supdb-ingestlatesync"
            };
        }
        if self.norecycle {
            return if self.partition {
                "supdb-norecycle"
            } else {
                "supdb-ingestnorecycle"
            };
        }
        if self.keysarena {
            return if self.partition {
                "supdb-keysarena"
            } else {
                "supdb-ingestkeysarena"
            };
        }
        if self.noalert {
            return if self.partition {
                "supdb-noalert"
            } else {
                "supdb-ingestnoalert"
            };
        }
        if self.ordlog {
            return if self.partition {
                "supdb-ordlog"
            } else {
                "supdb-ingestordlog"
            };
        }
        if self.fatentries {
            return if self.partition {
                "supdb-fatentries"
            } else {
                "supdb-ingestfatentries"
            };
        }
        if self.waitmanifest {
            return if self.partition {
                "supdb-waitmanifest"
            } else {
                "supdb-ingestwaitmanifest"
            };
        }
        if self.rankrecords {
            return if self.partition {
                "supdb-rankrecords"
            } else {
                "supdb-ingestrankrecords"
            };
        }
        if self.shape {
            return "supdb-ingestshape";
        }
        if self.adapttrig {
            return if self.partition {
                "supdb-adapttrig"
            } else {
                "supdb-ingestadapttrig"
            };
        }
        if let Some(reading) = self.adaptcap {
            let loose = reading > supdb::Options::default().seal_max_pct;
            return match (self.partition, loose) {
                (true, false) => "supdb-adapt",
                (true, true) => "supdb-adaptloose",
                (false, false) => "supdb-ingestadapt",
                (false, true) => "supdb-ingestadaptloose",
            };
        }
        if self.inlinemaint {
            return if self.partition {
                "supdb-inlinemaint"
            } else {
                "supdb-ingestinline"
            };
        }
        if self.sealcap.is_some() && !self.partition {
            return "supdb-ingestnoseal";
        }
        if self.sealcap.is_some() {
            return "supdb-noseal";
        }
        if self.lagcap > 0 {
            return "supdb-regime";
        }
        if self.wforms {
            return "supdb-wforms";
        }
        if self.forms {
            return "supdb-forms";
        }
        if !self.snap {
            return "supdb-nosnap";
        }
        if !self.partition && !self.drain {
            return "supdb-ingestsync";
        }
        // Before the match below, which would name the nocache policy
        // `supdb-nocache` whatever else it carries.
        if self.ordread {
            return "supdb-nocacheordread";
        }
        if self.zpages_auto {
            return if self.block_cache {
                "supdb-autozpages"
            } else if self.zpages_fairio {
                "supdb-nocacheautozpagesfairio"
            } else if self.zpages_noahead {
                "supdb-nocacheautozpagesnoahead"
            } else if self.zpages_buildeager {
                "supdb-nocacheautozpageseager"
            } else {
                "supdb-nocacheautozpages"
            };
        }
        if self.zpages {
            return if self.zpages_nodrop {
                "supdb-nocachezpagesnodrop"
            } else if self.zpages_nowarm {
                "supdb-nocachezpagesnowarm"
            } else if self.zpages_nomemo {
                "supdb-nocachezpagesnomemo"
            } else if self.zpages_heads {
                "supdb-nocachezpagesheads"
            } else if self.zpages_prefetch {
                "supdb-nocachezpagesprefetch"
            } else if self.zpages_rankwalk {
                "supdb-nocachezpagesrankwalk"
            } else if self.zpages_settle {
                "supdb-nocachezpagessettle"
            } else if self.zcheckedviews {
                "supdb-nocachezpagescheckedviews"
            } else if self.zrecord {
                "supdb-nocachezpagesrecord"
            } else if self.zstored {
                "supdb-nocachezpagesstored"
            } else if self.zrawtails {
                "supdb-nocachezpagesrecordraw"
            } else if self.zgroup == Some(0) {
                "supdb-nocachezpagesonegroup"
            } else {
                "supdb-nocachezpages"
            };
        }
        match (
            self.partition,
            self.drain,
            self.advice.is_some(),
            self.block_cache,
        ) {
            (true, true, false, false) => "supdb-nocache",
            (true, true, true, _) => "supdb-noadvice",
            (true, true, false, true) => "supdb",
            (false, _, _, _) => "supdb-ingest",
            (true, false, _, _) => "supdb-nodrain",
        }
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: self.durable,
            // A batch is the WAL frames behind one commit frame and replay
            // applies it whole or not at all; `Txn` stages, commits as one
            // batch, and aborts by dropping; and the engine is single-writer
            // with reads that borrow it, so nothing observes a batch
            // half-applied. That is the axis LMDB held over every Supdb arm
            // and the residual every matched comparison carried.
            transactions: true,
            // Equalized off, matching lmdb -- see create().
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        let db = self.db.as_mut().ok_or("db closed")?;
        db.append_batch(items);
        db.commit().map_err(|e| e.to_string())
    }
    fn update_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        let db = self.db.as_mut().ok_or("db closed")?;
        for &(k, v) in items {
            db.put(k, v);
        }
        db.commit().map_err(|e| e.to_string())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        let db = self.db.as_ref().ok_or("db closed")?;
        supdb_get(db, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let db = self.db.as_ref().ok_or("db closed")?;
        supdb_range(db, from, n)
    }
    fn counters(&self) -> Vec<(&'static str, f64)> {
        let Some(db) = self.db.as_ref() else {
            return Vec::new();
        };
        // `canonical_forms` answers for the state standing now, which a
        // publish resets: its held count and bytes are what the store
        // carries at the end of the pass, which is the question for
        // memory, but its takes would say what happened since the last
        // seal and be read as the pass's. The pass's takes come from the
        // store's own life instead.
        let (forms, form_bytes, _, _) = db.canonical_forms();
        let ev = db.forms_events();
        let sd = db.settle_density();
        let bk = db.build_kinds();
        let wk = db.forms_walks();
        let cs = db.copy_starts();
        let ph = db.scan_phases();
        let uk = db.upkeep_counts();
        let ord = db.ord_point_reads();
        let z = db.zpage_counts();
        vec![
            ("snapshot_builds", db.snapshot_builds() as f64),
            ("snapshot_extends", db.snapshot_extends() as f64),
            ("snapshot_refreshes", db.snapshot_refreshes() as f64),
            ("form_takes", db.form_takes() as f64),
            ("blk_by_reader", db.blocks_built().0 as f64),
            ("blk_by_engine", db.blocks_built().1 as f64),
            ("rd_scans", db.reader_scans().0 as f64),
            ("rd_blockpath", db.reader_scans().1 as f64),
            ("canon_tried", db.canonical_tries().0 as f64),
            ("canon_hit", db.canonical_tries().1 as f64),
            ("forms_held_at_end", forms as f64),
            ("form_bytes_at_end", form_bytes as f64),
            // What the writer's own block tables count themselves holding
            // (contents, not capacity), beside the published forms' bytes
            // above: the pair a memory decomposition holds against the
            // process's resident bytes (`lagmem`).
            ("cache_bytes_at_end", db.block_cache_bytes() as f64),
            ("lazy_scans_done", db.lazy_scans().0 as f64),
            ("lazy_scans_resumed", db.lazy_scans().1 as f64),
            ("seals", db.seal_waits().joins as f64),
            ("publishes", db.seal_waits().publishes as f64),
            ("seals_deferred", db.seal_waits().deferred as f64),
            ("seals_idle", db.seal_waits().idle_seals as f64),
            ("wal_rotations", db.seal_waits().wal_rotations as f64),
            ("freeze_ms", db.seal_waits().freeze_ns as f64 / 1e6),
            ("join_wait_ms", db.seal_waits().join_wait_ns as f64 / 1e6),
            // The manifest publishes' time on the segment work's thread:
            // the slot's write and fdatasync, a device round trip each.
            ("publish_ms", db.seal_waits().publish_ns as f64 / 1e6),
            // Who sorted the frozen tables: the seals that found no order
            // on their table, and readers that sorted one alone.
            ("seal_sorts", db.seal_waits().seal_sorts as f64),
            ("seal_sort_ms", db.seal_waits().seal_sort_ns as f64 / 1e6),
            ("seal_snap_ms", db.seal_waits().seal_snap_ns as f64 / 1e6),
            ("frozen_sorts", db.frozen_sorts() as f64),
            // What the upkeep thread spent, and the passes it spent it in.
            ("upkeep_ms", db.upkeep_counts()[6] as f64 / 1e3),
            ("upkeep_passes", db.upkeep_counts()[0] as f64),
            ("upkeep_skipped", db.upkeep_counts()[7] as f64),
            ("snap_switches", db.snapshot_switches() as f64),
            // The store's shape as it stands: what a read has to consult.
            ("segments", db.segments() as f64),
            ("partitions", db.levels().0 as f64),
            ("pieces", db.levels().1 as f64),
            ("unsealed_keys", db.unsealed_keys() as f64),
            ("pieces_aligned", db.pieces_aligned() as u8 as f64),
            // What the forms' upkeep did: see `Db::forms_events`.
            ("blk_by_upkeep", ev[0] as f64),
            ("table_drops", ev[1] as f64),
            ("ahead_starts", ev[2] as f64),
            ("ahead_stops", ev[3] as f64),
            ("ahead_installed", ev[4] as f64),
            ("settle_notable", ev[5] as f64),
            ("fill_dense", ev[6] as f64),
            ("fill_fresh", ev[7] as f64),
            ("ahead_skip_held", ev[8] as f64),
            ("ahead_skip_overlay", ev[9] as f64),
            ("ahead_built", ev[10] as f64),
            ("install_gen_drop", ev[11] as f64),
            ("install_slot_skip", ev[12] as f64),
            ("fill_from_form", ev[13] as f64),
            // The forms the writer's walks met: see `Db::forms_walks`.
            ("walk_copy", wk[0] as f64),
            ("start_at_rank", cs[0] as f64),
            ("start_search", cs[1] as f64),
            // The writer's builds by kind, the covered blocks copied on
            // their second touch and the budget's sheds: see
            // `Db::build_kinds`.
            ("builds_copy", bk.0[0] as f64),
            ("builds_sparse", bk.0[1] as f64),
            ("builds_wide", bk.0[2] as f64),
            ("builds_clean", bk.0[3] as f64),
            ("recopies", bk.1 as f64),
            ("sheds", bk.2 as f64),
            ("span_starts", db.span_starts() as f64),
            // The scans' start blocks the start cache named, and the
            // starts it kept: see `Db::start_cache_counts`.
            ("start_cache_hits", db.start_cache_counts().0 as f64),
            ("start_cache_puts", db.start_cache_counts().1 as f64),
            ("lazy_cuts_filed", db.lazy_cuts()[0] as f64),
            ("lazy_cuts_resolved", db.lazy_cuts()[1] as f64),
            // The writer's settles: see `Db::settle_density`.
            ("settle_runs", sd[0] as f64),
            ("settle_keys", sd[1] as f64),
            ("settle_blocks", sd[2] as f64),
            ("settle_rebuilds", sd[3] as f64),
            ("settle_built", db.settle_built() as f64),
            ("walk_sparse", wk[1] as f64),
            ("walk_other", wk[2] as f64),
            // Seeks that hinted their stride window's directory: see
            // `Db::dir_prefetches`.
            ("dir_prefetches", db.dir_prefetches() as f64),
            // Where the writer's scans spent their time before walking,
            // in microseconds: see `Db::scan_phases`.
            ("scan_take_us", ph[0] as f64 / 1e3),
            ("scan_sync_us", ph[1] as f64 / 1e3),
            ("scan_settle_us", ph[2] as f64 / 1e3),
            ("scan_snap_us", ph[3] as f64 / 1e3),
            ("scan_ahead_us", ph[4] as f64 / 1e3),
            ("scan_install_us", ph[5] as f64 / 1e3),
            ("upkeep_wait_us", uk[3] as f64),
            ("upkeep_partial", uk[8] as f64),
            ("upkeep_cpu_us", uk[9] as f64),
            ("maint_cpu_us", db.maint_cpu_us() as f64),
            ("convert_build_us", db.convert_us().0 as f64),
            ("convert_total_us", db.convert_us().1 as f64),
            ("forms_cloned", db.forms_cloned() as f64),
            ("forms_trims", db.forms_trims() as f64),
            ("forms_trim_looks", db.forms_trim_looks() as f64),
            ("forms_trim_us", db.forms_trim_us() as f64),
            ("forms_rebased", db.forms_rebased() as f64),
            ("pin_barriers", uk[10] as f64),
            // Point reads of a partition answered through its ordered
            // index, and the ones that left it for the hash: the writer's
            // own handle's (`Db::ord_point_reads`), so the arm that reads
            // through the index shows the path was taken and the arm that
            // does not shows none. The reader handles of the threaded
            // passes count apart, summed as each is dropped.
            ("ord_point_reads", ord.0 as f64),
            ("ord_point_declined", ord.1 as f64),
            (
                "ord_point_reads_threads",
                self.ord_threads
                    .served
                    .load(std::sync::atomic::Ordering::Relaxed) as f64,
            ),
            (
                "ord_point_declined_threads",
                self.ord_threads
                    .declined
                    .load(std::sync::atomic::Ordering::Relaxed) as f64,
            ),
            // The second form (`Options::zpages`): what answered the reads
            // and counts of a partition, what was built and what was
            // declined or refused, and what the store holds at the end.
            // Read from the reader table's slots and the live segments, so
            // the handles of a threaded pass are in the sum after they are
            // dropped. Zero throughout on every arm that does not force it.
            (
                "zpage_forced",
                (db.options().zpages == supdb::ZPages::Always) as u8 as f64,
            ),
            (
                "zpage_auto",
                (db.options().zpages == supdb::ZPages::Auto) as u8 as f64,
            ),
            ("zpage_reads", z.reads as f64),
            ("zpage_counts", z.counts as f64),
            ("zpage_decodes", z.decodes as f64),
            ("zpage_inplace_groups", z.inplace_groups as f64),
            ("zpage_rawserves", z.rawserves as f64),
            ("zpage_fallbacks", z.fallbacks as f64),
            ("zpage_nopage_reads", z.nopage_reads as f64),
            ("zpage_scans", z.scans as f64),
            ("zpage_scan_groups", z.scan_groups as f64),
            ("zpage_nopage_scans", z.nopage_scans as f64),
            ("zpage_raw_scans", z.raw_scans as f64),
            ("zpage_builds", z.builds as f64),
            ("zpage_build_pages", z.build_pages as f64),
            ("zpage_build_bytes", z.build_bytes as f64),
            ("zpage_build_stored", z.build_stored as f64),
            ("zpage_build_failed", z.build_failed as f64),
            ("zpage_build_orphaned", z.build_orphaned as f64),
            ("zpage_build_retries", z.build_retries as f64),
            ("zpage_rebuilds", z.rebuilds as f64),
            ("zpage_declined_damage", z.declined_damage as f64),
            ("zpage_build_starts", z.build_starts as f64),
            ("zpage_build_cancelled", z.build_cancelled as f64),
            ("zpage_prebuilt", z.prebuilt as f64),
            ("zpage_build_io_class", z.build_io_class as f64),
            ("zpage_build_ahead_bytes", z.build_ahead_bytes as f64),
            ("zpage_build_nopage", z.build_nopage as f64),
            ("zpage_warms", z.warms as f64),
            ("zpage_warm_bytes", z.warm_bytes as f64),
            ("zpage_warm_skipped", z.warm_skipped as f64),
            ("zpage_warm_us", z.warm_us as f64),
            ("zpage_memo_hits", z.memo_hits as f64),
            ("zpage_memo_misses", z.memo_misses as f64),
            ("zpage_prefetches", z.prefetches as f64),
            ("zpage_declined_small", z.declined_small as f64),
            ("zpage_declined_small_bytes", z.declined_small_bytes as f64),
            (
                "zpage_declined_block_backed",
                z.declined_block_backed as f64,
            ),
            ("zpage_declined_oversize", z.declined_oversize as f64),
            (
                "zpage_declined_incompressible",
                z.declined_incompressible as f64,
            ),
            ("zpage_declined_unplaceable", z.declined_unplaceable as f64),
            ("zpage_attached", z.attached as f64),
            ("zpage_rejected_damage", z.rejected_damage as f64),
            ("zpage_rejected_identity", z.rejected_identity as f64),
            ("zpage_swept", z.swept as f64),
            ("zpage_gate_at_end", z.gate as f64),
            ("zpage_phase_at_end", z.phase as f64),
            ("zpage_auto_inert", z.auto_inert as f64),
            ("zpage_sel_steps", z.sel_steps as f64),
            ("zpage_sel_s1", z.sel_s1 as f64),
            ("zpage_sel_switches", z.sel_switches as f64),
            ("zpage_sel_trials", z.sel_trials as f64),
            ("zpage_sel_trials_back", z.sel_trials_back as f64),
            ("zpage_sel_threads", z.sel_threads as f64),
            ("zpage_sel_mixed", z.sel_mixed as f64),
            ("zpage_sel_t_f_ns", z.sel_t_f_ns as f64),
            ("zpage_sel_t_hit_ns", z.sel_t_hit_ns as f64),
            ("zpage_sel_t_hit_scan_ns", z.sel_t_hit_scan_ns as f64),
            ("zpage_sel_t_z_ns", z.sel_t_z_ns as f64),
            ("zpage_sel_t_z_scan_ns", z.sel_t_z_scan_ns as f64),
            ("zpage_census_windows", z.census_windows as f64),
            (
                "zpage_census_commit_windows",
                z.census_commit_windows as f64,
            ),
            ("zpage_census_mixed", z.census_mixed as f64),
            ("zpage_census_moved", z.census_moved as f64),
            ("zpage_census_reads", z.census_reads as f64),
            ("zpage_census_read_faults", z.census_read_faults as f64),
            ("zpage_census_scans", z.census_scans as f64),
            ("zpage_census_scan_faults", z.census_scan_faults as f64),
            ("zpage_census_t_f_ns", z.census_t_f_ns as f64),
            ("zpage_census_t_hit_ns", z.census_t_hit_ns as f64),
            ("zpage_census_t_hit_scan_ns", z.census_t_hit_scan_ns as f64),
            ("zpage_census_t_z_ns", z.census_t_z_ns as f64),
            ("zpage_census_t_z_scan_ns", z.census_t_z_scan_ns as f64),
            ("zpage_samples", z.samples as f64),
            ("zpage_sampled", z.sampled as f64),
            ("zpage_sample_declined", z.sample_declined as f64),
            ("zpage_sample_us", z.sample_us as f64),
            ("zpage_sample_w_z_bytes", z.sample_w_z_bytes as f64),
            ("zpage_sample_w_z_raw_bytes", z.sample_w_z_raw_bytes as f64),
            ("zpage_coding_picks_block", z.coding_picks_block as f64),
            ("zpage_coding_picks_raw", z.coding_picks_raw as f64),
            ("zpage_builds_raw_tails", z.builds_raw_tails as f64),
            ("zpage_raw_dropped", z.raw_dropped as f64),
            ("zpage_recodes", z.recodes as f64),
            ("zpage_recodes_back", z.recodes_back as f64),
            ("zpage_recode_failed", z.recode_failed as f64),
            ("zpage_recode_flips", z.recode_flips as f64),
            ("zpage_spare_files", z.spare_files as f64),
            ("zpage_sel_samples", z.sel_samples as f64),
            ("zpage_sel_builds", z.sel_builds as f64),
            ("zpage_sel_installs", z.sel_installs as f64),
            ("zpage_sel_cancelled", z.sel_cancelled as f64),
            ("zpage_sel_suspended", z.sel_suspended as f64),
            ("zpage_sel_exit_e1", z.sel_exit_e1 as f64),
            ("zpage_sel_exit_e2", z.sel_exit_e2 as f64),
            ("zpage_sel_exit_e3", z.sel_exit_e3 as f64),
            ("zpage_sel_retired", z.sel_retired as f64),
            ("zpage_sel_excess_ns", z.sel_excess_ns as f64),
            ("zpage_sel_c_build_ns", z.sel_c_build_ns as f64),
            ("zpage_footprint_pages", z.footprint_pages as f64),
            ("zpage_footprint_bytes", z.footprint_bytes as f64),
            ("zpage_retired", z.retired as f64),
            ("zpage_partitions_at_end", z.partitions as f64),
            ("zpage_form files_at_end", z.form_files as f64),
            ("zpage_small_partitions_at_end", z.small_partitions as f64),
            ("zpage_damaged_at_end", z.damaged as f64),
            ("zpage_pages_at_end", z.pages as f64),
            ("zpage_verified_pages_at_end", z.verified_pages as f64),
            ("zpage_bad_pages_at_end", z.bad_pages as f64),
            ("zpage_resident_bytes_at_end", z.resident_bytes as f64),
            ("zpage_file_bytes_at_end", z.file_bytes as f64),
            ("zpage_group_bytes_at_end", z.group_bytes as f64),
        ]
    }
    fn zpage_heat(&self) -> String {
        let Some(db) = self.db.as_ref() else {
            return String::new();
        };
        db.zpage_heat()
            .iter()
            .map(|h| {
                let lo = String::from_utf8_lossy(&h.lo[..h.lo.len().min(16)]).into_owned();
                format!("{lo}:{}:{}MB:{}", h.keys, h.file_bytes >> 20, h.decodes)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn memory_census(&self) -> Vec<(String, u64)> {
        self.db
            .as_ref()
            .map(|db| db.memory_census())
            .unwrap_or_default()
    }
    fn zpages_state(&self) -> Option<ZState> {
        let db = self.db.as_ref()?;
        let c = db.zpage_counts();
        let (mut files, mut stray) = (0, 0);
        for e in std::fs::read_dir(&self.path)
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("zpg-") {
                if name.ends_with(".zpg") {
                    files += 1;
                } else {
                    stray += 1;
                }
            }
        }
        Some(ZState {
            forced: db.options().zpages == supdb::ZPages::Always,
            auto: db.options().zpages == supdb::ZPages::Auto,
            reads: c.reads,
            raw: c.rawserves + c.fallbacks,
            partitions: c.partitions,
            form_files: c.form_files,
            small: c.small_partitions,
            damaged: c.damaged,
            files,
            stray,
        })
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        let db = self.db.as_ref().ok_or("db closed")?;
        // Claimed now, on this thread: the handle's slot in the reader
        // table. `Latest`, the handle's default, sees every commit before
        // a read, and the load's last is behind `sync`.
        let r = db.reader().map_err(|e| e.to_string())?;
        if self.ordread {
            let tally = self.ord_threads.clone();
            return Ok(Box::new(move || {
                Ok(Box::new(TallyReader { r, tally }) as Box<dyn ThreadReader>)
            }));
        }
        Ok(Box::new(move || Ok(Box::new(r) as Box<dyn ThreadReader>)))
    }
    fn sync(&mut self) -> Res<()> {
        if let Some(db) = self.db.as_mut() {
            // The default drains: commit AND seal AND partition, so the seal
            // cost lands in the load window and the read phase answers from
            // routed segments. That was chosen so a supdb arm would not read
            // half its keys out of a resident memtable while LMDB read a
            // tree -- but RocksDB's sync is an fsync of its WAL and its
            // reads go through its memtable and level 0, so against it the
            // drain is a residual the engine pays alone. `supdb-nodrain`
            // is the arm matched to that; both are measured.
            if self.drain {
                db.flush().map_err(|e| e.to_string())?;
            } else {
                db.sync().map_err(|e| e.to_string())?;
            }
            // The second form, forced: a flush is answered before the
            // segment work's later steps, and the arm would read the raw
            // form of every partition without the form files this waits for.
            // Their build is in the load window, and so is the arm's.
            if self.zpages {
                db.zpages_settle().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        dir_size(&self.path)
    }
}

// ------------------------------------------------------------------- redb --

/// redb: the closest architectural sibling in the field.
///
/// Single writer, many readers, MVCC, a copy-on-write B-tree -- and
/// deliberately *not* mmap-based. It is therefore the comparison that isolates
/// the mmap decision rather than confounding it with the storage model.
pub struct Rocks {
    /// Shared with the reader threads: the handle is `Sync`, and a thread
    /// reads through this one with read options of its own.
    db: std::sync::Arc<rocksdb::DB>,
    path: PathBuf,
    sync: bool,
    tuned: bool,
    /// Whether `sync` flushes the memtable and compacts everything, so the
    /// load window carries the same drain the engine's default does
    /// and the reads run against a fully compacted tree.
    drain: bool,
    read: rocksdb::ReadOptions,
}

impl Rocks {
    pub fn create(path: &Path, sync: bool) -> Res<Rocks> {
        Rocks::with(path, sync, false, false)
    }

    /// The deployed shape rather than the shipped one, stated in full so the
    /// claim can name it: a 256 MB LRU block cache (the canonical data is
    /// 110 MB, so every block a read wants is in memory after the first
    /// touch, as it is for the mapped engines), a 10-bit Bloom filter per
    /// SST with index and filter blocks cached, four background threads,
    /// and the write side set as RocksDB's tuning guide sets it for a bulk
    /// load (see `with`). Not a tuning contest: one stated configuration a
    /// reader can recognise as a deployment.
    pub fn create_tuned(path: &Path) -> Res<Rocks> {
        Rocks::with(path, true, true, false)
    }

    /// Tuned, and drained at `sync`: memtable flushed, every level
    /// compacted into one, inside the window -- the engine's default
    /// shape, charged to RocksDB.
    pub fn create_tuned_drain(path: &Path) -> Res<Rocks> {
        Rocks::with(path, true, true, true)
    }

    fn with(path: &Path, sync: bool, tuned: bool, drain: bool) -> Res<Rocks> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let mut o = rocksdb::Options::default();
        o.create_if_missing(true);
        o.set_compression_type(rocksdb::DBCompressionType::None);
        if tuned {
            let mut bbo = rocksdb::BlockBasedOptions::default();
            let cache = rocksdb::Cache::new_lru_cache(256 << 20);
            bbo.set_block_cache(&cache);
            bbo.set_bloom_filter(10.0, false);
            bbo.set_cache_index_and_filter_blocks(true);
            o.set_block_based_table_factory(&bbo);
            o.increase_parallelism(4);
            o.set_max_background_jobs(4);
            // The write side, tuned for the load the suite runs, so the
            // load comparison is against RocksDB tuned for the load too:
            // 128 MB write buffers, four of them, merged two
            // at a time, and level 0 allowed eight files before a
            // compaction -- the shape RocksDB's own tuning guide gives a
            // bulk load, against its 64 MB / two / four defaults.
            o.set_write_buffer_size(128 << 20);
            o.set_max_write_buffer_number(4);
            o.set_min_write_buffer_number_to_merge(2);
            o.set_level_zero_file_num_compaction_trigger(8);
        }
        let db = rocksdb::DB::open(&o, path).map_err(|e| e.to_string())?;
        let mut read = rocksdb::ReadOptions::default();
        read.set_verify_checksums(false);
        Ok(Rocks {
            db: std::sync::Arc::new(db),
            path: path.to_path_buf(),
            sync,
            tuned,
            drain,
            read,
        })
    }
}

/// One read through whichever handle: the arm's own or a thread's.
fn rocks_get(db: &rocksdb::DB, read: &rocksdb::ReadOptions, key: &[u8]) -> Res<usize> {
    // Pinned: the value is lent from the block cache rather than copied
    // into an allocation of RocksDB's making, the same lend the other
    // engines give; the caller's copy is `consume`, as theirs is.
    Ok(db
        .get_pinned_opt(key, read)
        .map_err(|e| e.to_string())?
        .map(|v| consume(&v))
        .unwrap_or(0))
}

fn rocks_range(db: &rocksdb::DB, from: &[u8], n: usize) -> Res<usize> {
    // By value, and `ReadOptions` does not clone: one per scan, which is
    // one small allocation against a walk of `n` entries.
    let mut ro = rocksdb::ReadOptions::default();
    ro.set_verify_checksums(false);
    let mut it = db.raw_iterator_opt(ro);
    it.seek(from);
    let mut bytes = 0usize;
    let mut seen = 0usize;
    while it.valid() && seen < n {
        bytes += it.value().map(consume).unwrap_or(0);
        seen += 1;
        it.next();
    }
    it.status().map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// A reader thread's RocksDB: the shared handle, and read options of the
/// thread's own.
struct RocksReader {
    db: std::sync::Arc<rocksdb::DB>,
    read: rocksdb::ReadOptions,
}

impl ThreadReader for RocksReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        rocks_get(&self.db, &self.read, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        rocks_range(&self.db, from, n)
    }
}

impl Engine for Rocks {
    fn name(&self) -> &'static str {
        match (self.sync, self.tuned, self.drain) {
            (_, true, true) => "rocksdb-tuned-drain",
            (_, true, false) => "rocksdb-tuned",
            (true, false, _) => "rocksdb",
            (false, false, _) => "rocksdb-nosync",
        }
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: self.sync,
            // A WriteBatch is applied whole or not at all and is readable by
            // this handle the moment `write` returns: the same atomic-batch,
            // read-your-writes contract the engine's `Txn` and LMDB's
            // write transaction give the suite. Reader isolation beyond that
            // is RocksDB's snapshot, which no workload here needs.
            transactions: true,
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        let mut b = rocksdb::WriteBatch::default();
        for &(k, v) in items {
            b.put(k, v);
        }
        let mut wo = rocksdb::WriteOptions::default();
        wo.set_sync(self.sync);
        self.db.write_opt(b, &wo).map_err(|e| e.to_string())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        rocks_get(&self.db, &self.read, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        rocks_range(&self.db, from, n)
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        // The handle is shared; what a thread gets of its own is read
        // options, made where they are used.
        let db = self.db.clone();
        Ok(Box::new(move || {
            let mut read = rocksdb::ReadOptions::default();
            read.set_verify_checksums(false);
            Ok(Box::new(RocksReader { db, read }) as Box<dyn ThreadReader>)
        }))
    }
    fn sync(&mut self) -> Res<()> {
        // Everything written reaches the device: the WAL is fsynced, which
        // is what the nosync arm has been deferring. The memtable stays a
        // memtable; RocksDB reads it, so nothing more is needed for
        // "readable", and flushing it would charge this arm a compaction
        // the others do not pay at this point.
        if self.drain {
            self.db.flush().map_err(|e| e.to_string())?;
            self.db.compact_range::<&[u8], &[u8]>(None, None);
            return Ok(());
        }
        self.db.flush_wal(true).map_err(|e| e.to_string())
    }
    fn size_bytes(&self) -> u64 {
        dir_size(&self.path)
    }
}

// ------------------------------------------------------------------- lmdb --

/// LMDB through heed.
///
/// The engine the design document names as the one Supdb had to beat for the
/// design to mean anything: mmap, single writer, many readers, no daemon. The
/// same architecture, with the two mechanisms Supdb did not adopt -- a
/// never-shrink-under-readers invariant and process-lock reader liveness.
pub struct Lmdb {
    env: heed::Env,
    db: heed::Database<heed::types::Bytes, heed::types::Bytes>,
    path: PathBuf,
    /// A read transaction held across operations, dropped when a write makes
    /// it stale.
    ///
    /// This adapter opened one per `get` and one per `range`. The comment
    /// where it did called that "conservative against LMDB", which is exactly
    /// backwards -- it is a cost LMDB pays and Supdb does not, since Supdb's
    /// adapter caches its `Reader` across calls and rebuilds it only when
    /// dirty. Worse, a transaction per lookup is the specific handicap the
    /// architecture review criticised the design document's own LMDB adapter
    /// for, and removing it there was worth 2.3x. Reproducing it here put the
    /// same thumb on the same scale.
    txn: Option<heed::RoTxn<'static>>,
    /// MDB_NOSYNC: commit stops reaching the device.
    ///
    /// The other half of matching guarantees. `supdb-durable` brings Supdb up
    /// to LMDB's boundary; this brings LMDB down to Supdb's default, so the
    /// pair can be compared at both levels of promise rather than at neither.
    nosync: bool,
    /// MDB_NORDAHEAD: the map opened with the kernel's readahead off, so a
    /// point read past the page cache faults one page and not the
    /// readahead window around it. Out of core, where every miss is a
    /// device read, the window is what a random read pays for a
    /// sequential one's benefit; this is the LMDB a user would configure
    /// for that regime, beside the default a user gets. Durable as the
    /// default is: the flag touches the read path alone.
    nordahead: bool,
}

impl Lmdb {
    pub fn create(path: &Path, map_gb: usize) -> Res<Lmdb> {
        Lmdb::with_flags(path, map_gb, true, false)
    }

    /// Matched to Supdb's default: a commit that does not reach the device.
    pub fn create_nosync(path: &Path, map_gb: usize) -> Res<Lmdb> {
        Lmdb::with_flags(path, map_gb, false, false)
    }

    /// The durable arm with readahead off (`nordahead`).
    pub fn create_nordahead(path: &Path, map_gb: usize) -> Res<Lmdb> {
        Lmdb::with_flags(path, map_gb, true, true)
    }

    fn with_flags(path: &Path, map_gb: usize, sync: bool, nordahead: bool) -> Res<Lmdb> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let env = unsafe {
            let mut o = heed::EnvOpenOptions::new();
            o.map_size(map_gb * 1024 * 1024 * 1024).max_dbs(4);
            let mut flags = heed::EnvFlags::empty();
            if !sync {
                flags |= heed::EnvFlags::NO_SYNC;
            }
            if nordahead {
                flags |= heed::EnvFlags::NO_READ_AHEAD;
            }
            if !flags.is_empty() {
                o.flags(flags);
            }
            o.open(path)
        }
        .map_err(|e| e.to_string())?;
        let mut w = env.write_txn().map_err(|e| e.to_string())?;
        let db = env
            .create_database(&mut w, None)
            .map_err(|e| e.to_string())?;
        w.commit().map_err(|e| e.to_string())?;
        Ok(Lmdb {
            env,
            db,
            path: path.to_path_buf(),
            txn: None,
            nosync: !sync,
            nordahead,
        })
    }

    /// The flags the environment was opened with, as LMDB reports them;
    /// the test that holds the readahead arm to one flag reads them.
    #[cfg(test)]
    fn env_flags(&self) -> heed::EnvFlags {
        self.env
            .flags()
            .ok()
            .flatten()
            .unwrap_or(heed::EnvFlags::empty())
    }

    /// The held read transaction, opened if there is not one.
    fn snapshot(&mut self) -> Res<&heed::RoTxn<'static>> {
        if self.txn.is_none() {
            self.txn = Some(
                self.env
                    .clone()
                    .static_read_txn()
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(self.txn.as_ref().expect("just filled"))
    }
}

/// heed keeps every environment it opens in a registry of its own, and an
/// environment closes only once `prepare_for_closing` has taken the
/// registry's handle and every other handle is gone. Dropping the arm alone
/// closed nothing: every store a run opened stayed mapped until the process
/// exited, holding its disk after its directory was removed, and the second
/// store a pass opens at the first one's path was handed the first one back.
impl Drop for Lmdb {
    fn drop(&mut self) {
        // The held transaction owns a handle; the last one, in `env`, drops
        // after this and closes the environment.
        self.txn = None;
        let _ = self.env.clone().prepare_for_closing();
    }
}

type LmdbDb = heed::Database<heed::types::Bytes, heed::types::Bytes>;

/// One read under whichever transaction: the arm's held one or a thread's.
fn lmdb_get(db: LmdbDb, r: &heed::RoTxn<'_>, key: &[u8]) -> Res<usize> {
    // Lent from the mapping; the caller's copy is `consume`.
    Ok(db
        .get(r, key)
        .map_err(|e| e.to_string())?
        .map(consume)
        .unwrap_or(0))
}

fn lmdb_range(db: LmdbDb, r: &heed::RoTxn<'_>, from: &[u8], n: usize) -> Res<usize> {
    let mut bytes = 0usize;
    let range = (std::ops::Bound::Included(from), std::ops::Bound::Unbounded);
    for row in db.range(r, &range).map_err(|e| e.to_string())?.take(n) {
        let (_, v) = row.map_err(|e| e.to_string())?;
        bytes += consume(v);
    }
    Ok(bytes)
}

/// A reader thread's LMDB: a read transaction begun on the thread, over
/// the shared environment it holds a handle to.
struct LmdbReader {
    db: LmdbDb,
    txn: heed::RoTxn<'static>,
}

impl ThreadReader for LmdbReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        lmdb_get(self.db, &self.txn, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        lmdb_range(self.db, &self.txn, from, n)
    }
}

impl Engine for Lmdb {
    fn name(&self) -> &'static str {
        match (self.nosync, self.nordahead) {
            (true, _) => "lmdb-nosync",
            (false, true) => "lmdb-nordahead",
            (false, false) => "lmdb",
        }
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: !self.nosync,
            transactions: true,
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        // A held read transaction pins the version it was opened at, so it has
        // to go before a write, exactly as Supdb's adapter drops its Reader.
        self.txn = None;
        let mut w = self.env.write_txn().map_err(|e| e.to_string())?;
        for &(k, v) in items {
            self.db.put(&mut w, k, v).map_err(|e| e.to_string())?;
        }
        w.commit().map_err(|e| e.to_string())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        let db = self.db;
        let r = self.snapshot()?;
        lmdb_get(db, r, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let db = self.db;
        let r = self.snapshot()?;
        lmdb_range(db, r, from, n)
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        // The environment is shared and the transaction is the thread's:
        // begun on it, since without `MDB_NOTLS` a read transaction lives
        // in the slot of the thread that began it, and dropped on it with
        // the reader. Begun after the load's last commit, so it reads the
        // store as loaded, as the arm's own held transaction does.
        let env = self.env.clone();
        let db = self.db;
        Ok(Box::new(move || {
            let txn = env.static_read_txn().map_err(|e| e.to_string())?;
            Ok(Box::new(LmdbReader { db, txn }) as Box<dyn ThreadReader>)
        }))
    }
    fn sync(&mut self) -> Res<()> {
        self.txn = None;
        self.env.force_sync().map_err(|e| e.to_string())
    }
    fn size_bytes(&self) -> u64 {
        dir_size(&self.path)
    }
}

// ------------------------------------------------------------------- sled --

/// sled: a log-structured B-tree, and the other well-known Rust embedded store.
// ---------------------------------------------------------------------------
// The arms.
use crate::row::Guarantee;

/// Every arm a run measures, in the order they are interleaved. Each is a
/// shipping supdb configuration or the comparator a user would otherwise
/// pick. Comparisons are made within a guarantee, never across one.
pub const ARMS: [&str; 12] = [
    "supdb",
    "supdb-forms",
    "supdb-settle",
    "supdb-nosnap",
    "supdb-noadvice",
    "supdb-nocache",
    "supdb-cache256",
    "lmdb",
    "rocksdb-tuned",
    "supdb-ingest",
    "lmdb-nosync",
    "rocksdb-nosync",
];

/// The arms a bounded run (`Scale::Ooc`) measures by default: the default
/// and the three shipping options whose difference is what a read past
/// the page cache costs -- no block cache, the cache bounded at RocksDB's
/// 256 MB, and no read advice -- against the durable comparators, LMDB
/// twice: as a user gets it and with its readahead off, which is the LMDB
/// a user configures for the regime. The buffered group is not here: a
/// bounded row is about reads from the device, and a buffered load reads
/// nothing a durable one does not. `--arms` overrides it as it does
/// `ARMS`.
pub const OOC_ARMS: [&str; 7] = [
    "supdb",
    "supdb-nocache",
    "supdb-cache256",
    "supdb-noadvice",
    "lmdb",
    "lmdb-nordahead",
    "rocksdb-tuned",
];

pub fn guarantee(arm: &str) -> Option<Guarantee> {
    Some(match arm {
        "supdb"
        | "supdb-forms"
        | "supdb-settle"
        | "supdb-wforms"
        | "supdb-regime"
        | "supdb-noseal"
        | "supdb-ahead"
        | "supdb-lazyforms"
        | "supdb-eager"
        | "supdb-nosettle"
        | "supdb-recency"
        | "supdb-nocarry"
        | "supdb-rebuild"
        | "supdb-snapcarry"
        | "supdb-lazysnap"
        | "supdb-fullrec"
        | "supdb-norebase"
        | "supdb-sealfile"
        | "supdb-settleall"
        | "supdb-inline"
        | "supdb-tier"
        | "supdb-runs"
        | "supdb-keeper"
        | "supdb-aheadpub"
        | "supdb-pubalways"
        | "supdb-nosnap"
        | "supdb-noadvice"
        | "supdb-nocache"
        | "supdb-nocacheordread"
        | "supdb-nocachezpages"
        | "supdb-nocachezpagesnodrop"
        | "supdb-nocachezpagesnowarm"
        | "supdb-nocachezpagesnomemo"
        | "supdb-nocachezpagescheckedviews"
        | "supdb-nocachezpagesrecord"
        | "supdb-nocachezpagesstored"
        | "supdb-nocachezpagesrecordraw"
        | "supdb-cache64tofitzpages"
        | "supdb-cache64tofitzpagesstored"
        | "supdb-cache64tofitzpagesrecordraw"
        | "supdb-cache64tofitzpagesrecordrawlean"
        | "supdb-cache64tofitzpagesrecordrawleantrim"
        | "supdb-cache64tofitzpagesrecordrawleanresident"
        | "supdb-cache64tofitzpageshot4"
        | "supdb-cache64tofitzpageshot1"
        | "supdb-cache64tofitzpagesdrop"
        | "supdb-cache64tofitzpageshot4drop"
        | "supdb-cache64tofitzpageshot8drop"
        | "supdb-cache64tofitzpageshot8dropsettleevery"
        | "supdb-cache64tofitzpageshot8dropsettlebuilds"
        | "supdb-cache64tofitzpagesrecodedrop"
        | "supdb-cache64tofitzpagesrecodekeepdrop"
        | "supdb-cache64tofitzpagesrecordrawdrop"
        | "supdb-cache64tofitzpagesrecordrawdecode"
        | "supdb-nocachezpagesheads"
        | "supdb-nocachezpagesprefetch"
        | "supdb-nocachezpagesrankwalk"
        | "supdb-nocachezpagesonegroup"
        | "supdb-nocachezpagessettle"
        | "supdb-autozpages"
        | "supdb-nocacheautozpages"
        | "supdb-cache64tofit"
        | "supdb-cache64tofitautozpages"
        | "supdb-cache128tofitautozpages"
        | "supdb-cache256tofitautozpages"
        | "supdb-cache64tofitautozpagesraw"
        | "supdb-cache64tofitautozpagescodingauto"
        | "supdb-cache64tofitautozpagespromote"
        | "supdb-cache64tofitautozpagesonegroup"
        | "supdb-cache64tofitautozpagesgroup1k"
        | "supdb-cache64tofitautozpagesprebuild"
        | "supdb-cache64tofitautozpagesspan"
        | "supdb-cache64tofitautozpagesmemolinear"
        | "supdb-cache64tofitautozpagesexactcut"
        | "supdb-cache64tofitautozpagescheckedviews"
        | "supdb-cache64tofitautozpagesrecord"
        | "supdb-cache64tofitautozpagesmemofirst"
        | "supdb-cache64tofitautozpagesfencetree"
        | "supdb-cache64tofitautozpagesmemonoahead"
        | "supdb-cache64tofitautozpagesstartcache"
        | "supdb-cache64tofitautozpagesperrun"
        | "supdb-cache64tofitautozpagesmemo4"
        | "supdb-cache64tofitautozpagesmemo16"
        | "supdb-cache64tofitautozpagesmemo32"
        | "supdb-nocacheautozpagesfairio"
        | "supdb-nocacheautozpagesnoahead"
        | "supdb-nocacheautozpageseager"
        | "supdb-cache256"
        | "supdb-l0"
        | "supdb-nopin"
        | "supdb-asympin"
        | "supdb-extsval"
        | "supdb-piecesonly"
        | "supdb-latewb"
        | "supdb-copysearch"
        | "supdb-convertsources"
        | "supdb-copyslack"
        | "supdb-manifestfree"
        | "supdb-oidxsync"
        | "supdb-refvals"
        | "supdb-rowatclose"
        | "supdb-lendbursts"
        | "supdb-inlineall"
        | "supdb-lazysettle"
        | "supdb-walsync"
        | "supdb-nodrainsnap"
        | "supdb-sealrun"
        | "supdb-closeinline"
        | "supdb-timers"
        | "supdb-wake"
        | "supdb-noprefetch"
        | "supdb-dirplain"
        | "supdb-settlefar"
        | "supdb-tofit"
        | "supdb-cache256tofit"
        | "supdb-cache256tofitnotrim"
        | "supdb-cache256tofitnoshed"
        | "supdb-cache256tofitnosweep"
        | "supdb-prefetch1k"
        | "supdb-manifestrename"
        | "supdb-manifestpending"
        | "supdb-populate"
        | "supdb-piece256"
        | "supdb-recordsteps"
        | "supdb-copyvals"
        | "supdb-keycopies"
        | "supdb-fillplain"
        | "supdb-latesync"
        | "supdb-norecycle"
        | "supdb-keysarena"
        | "supdb-noalert"
        | "supdb-ordlog"
        | "supdb-fatentries"
        | "supdb-waitmanifest"
        | "supdb-inlinemaint"
        | "supdb-adapt"
        | "supdb-adapttrig"
        | "supdb-adaptloose"
        | "supdb-rotate"
        | "supdb-nofrozen"
        | "supdb-idle"
        | "supdb-sortover"
        | "supdb-pmd"
        | "supdb-hold"
        | "supdb-settlefreeze"
        | "supdb-convertcap"
        | "supdb-shortpass"
        | "supdb-noyield"
        | "supdb-postedyield"
        | "supdb-behindyield"
        | "supdb-bg"
        | "supdb-bglag"
        | "lmdb"
        | "lmdb-nordahead"
        | "rocksdb-tuned" => Guarantee::Durable,
        "supdb-ingest"
        | "supdb-ingestleave"
        | "supdb-ingestnoseal"
        | "supdb-ingestsync"
        | "supdb-ingestinline"
        | "supdb-ingestshape"
        | "supdb-ingestadapt"
        | "supdb-ingestadapttrig"
        | "supdb-ingestadaptloose"
        | "supdb-shaperotate"
        | "supdb-shapedefer"
        | "supdb-shapefloor"
        | "supdb-shapenofrozen"
        | "supdb-shapeidle"
        | "supdb-shapesortover"
        | "supdb-ingestpmd"
        | "supdb-ingestsettlefreeze"
        | "supdb-ingestconvertcap"
        | "supdb-ingestshortpass"
        | "supdb-ingestnoyield"
        | "supdb-ingestpostedyield"
        | "supdb-ingestbehindyield"
        | "supdb-ingestpiecesonly"
        | "supdb-ingestlatewb"
        | "supdb-ingestcopysearch"
        | "supdb-ingestconvertsources"
        | "supdb-ingestcopyslack"
        | "supdb-ingestdrainpromote"
        | "supdb-ingestmanifestfree"
        | "supdb-ingestoidxsync"
        | "supdb-ingestrefvals"
        | "supdb-ingestrowatclose"
        | "supdb-ingestlendbursts"
        | "supdb-ingestinlineall"
        | "supdb-ingestlazysettle"
        | "supdb-ingestwalsync"
        | "supdb-ingestnodrainsnap"
        | "supdb-ingestsealrun"
        | "supdb-ingestcloseinline"
        | "supdb-ingesttimers"
        | "supdb-ingestwake"
        | "supdb-ingestnoprefetch"
        | "supdb-ingestdirplain"
        | "supdb-ingestsettlefar"
        | "supdb-ingesttofit"
        | "supdb-ingestprefetch1k"
        | "supdb-ingestmanifestrename"
        | "supdb-ingestmanifestpending"
        | "supdb-ingestpopulate"
        | "supdb-ingestpiece256"
        | "supdb-ingestrecordsteps"
        | "supdb-ingestcopyvals"
        | "supdb-ingestkeycopies"
        | "supdb-ingestfillplain"
        | "supdb-ingestlatesync"
        | "supdb-ingestnorecycle"
        | "supdb-ingestkeysarena"
        | "supdb-ingestnoalert"
        | "supdb-ingestordlog"
        | "supdb-ingestfatentries"
        | "supdb-ingestwaitmanifest"
        | "supdb-ingestrebuild"
        | "supdb-rankrecords"
        | "supdb-ingestrankrecords"
        | "supdb-ingestbg"
        | "supdb-ingestbglag"
        | "supdb-ingesthold"
        | "lmdb-nosync"
        | "rocksdb-nosync" => Guarantee::Buffered,
        _ => return None,
    })
}

/// Open an arm on a fresh directory. `map_gb` sizes LMDB's map; the other
/// engines grow on their own.
pub fn open(arm: &str, dir: &Path, map_gb: usize) -> Res<Box<dyn Engine>> {
    Ok(match arm {
        "supdb" => Box::new(Supdb::create(dir)?),
        "supdb-forms" => Box::new(Supdb::create_forms(dir)?),
        "supdb-settle" => Box::new(Supdb::create_settle(dir)?),
        "supdb-nosettle" => Box::new(Supdb::create_nosettle(dir)?),
        "supdb-eager" => Box::new(Supdb::create_eager(dir)?),
        "supdb-recency" => Box::new(Supdb::create_recency(dir)?),
        "supdb-nocarry" => Box::new(Supdb::create_nocarry(dir)?),
        "supdb-rebuild" => Box::new(Supdb::create_rebuild(dir)?),
        "supdb-ingestrebuild" => Box::new(Supdb::create_ingest_rebuild(dir)?),
        "supdb-snapcarry" => Box::new(Supdb::create_snapcarry(dir)?),
        "supdb-lazysnap" => Box::new(Supdb::create_lazysnap(dir)?),
        "supdb-fullrec" => Box::new(Supdb::create_fullrec(dir)?),
        "supdb-norebase" => Box::new(Supdb::create_norebase(dir)?),
        "supdb-sealfile" => Box::new(Supdb::create_sealfile(dir)?),
        "supdb-settleall" => Box::new(Supdb::create_settleall(dir)?),
        "supdb-inline" => Box::new(Supdb::create_inline(dir)?),
        "supdb-hold" => Box::new(Supdb::create_hold(dir)?),
        "supdb-ingesthold" => Box::new(Supdb::create_ingest_hold(dir)?),
        "supdb-tier" => Box::new(Supdb::create_tier(dir)?),
        "supdb-runs" => Box::new(Supdb::create_runs(dir)?),
        "supdb-keeper" => Box::new(Supdb::create_keeper(dir)?),
        "supdb-aheadpub" => Box::new(Supdb::create_aheadpub(dir)?),
        "supdb-pubalways" => Box::new(Supdb::create_pubalways(dir)?),
        "supdb-lazyforms" => Box::new(Supdb::create_lazyforms(dir)?),
        "supdb-ahead" => Box::new(Supdb::create_ahead(dir)?),
        "supdb-noseal" => Box::new(Supdb::create_noseal(dir)?),
        "supdb-regime" => Box::new(Supdb::create_regime(dir)?),
        "supdb-wforms" => Box::new(Supdb::create_wforms(dir)?),
        "supdb-nosnap" => Box::new(Supdb::create_nosnap(dir)?),
        "supdb-noadvice" => Box::new(Supdb::create_noadvice(dir)?),
        "supdb-nocache" => Box::new(Supdb::create_nocache(dir)?),
        "supdb-nocacheordread" => Box::new(Supdb::create_nocache_ordread(dir)?),
        "supdb-nocachezpages" => Box::new(Supdb::create_nocache_zpages(dir)?),
        "supdb-nocachezpagesnodrop" => Box::new(Supdb::create_nocache_zpages_nodrop(dir)?),
        "supdb-nocachezpagesnowarm" => Box::new(Supdb::create_nocache_zpages_nowarm(dir)?),
        "supdb-nocachezpagesnomemo" => Box::new(Supdb::create_nocache_zpages_nomemo(dir)?),
        "supdb-nocachezpagescheckedviews" => {
            Box::new(Supdb::create_nocache_zpages_checkedviews(dir)?)
        }
        "supdb-nocachezpagesrecord" => Box::new(Supdb::create_nocache_zpages_record(dir)?),
        "supdb-nocachezpagesstored" => Box::new(Supdb::create_nocache_zpages_stored(dir)?),
        "supdb-nocachezpagesrecordraw" => Box::new(Supdb::create_nocache_zpages_record_raw(dir)?),
        "supdb-cache64tofitzpagesrecordraw" => {
            Box::new(Supdb::create_cache64_tofit_zpages_record_raw(dir)?)
        }
        "supdb-cache64tofitzpagesrecordrawlean" => {
            Box::new(Supdb::create_cache64_tofit_zpages_record_raw_lean(dir)?)
        }
        "supdb-cache64tofitzpagesrecordrawleantrim" => Box::new(
            Supdb::create_cache64_tofit_zpages_record_raw_lean_trim(dir)?,
        ),
        "supdb-cache64tofitzpagesrecordrawleanresident" => Box::new(
            Supdb::create_cache64_tofit_zpages_record_raw_lean_resident(dir)?,
        ),
        "supdb-cache64tofitzpageshot4" => Box::new(Supdb::create_cache64_tofit_zpages_hot4(dir)?),
        "supdb-cache64tofitzpageshot1" => Box::new(Supdb::create_cache64_tofit_zpages_hot1(dir)?),
        "supdb-cache64tofitzpagesdrop" => Box::new(Supdb::create_cache64_tofit_zpages_drop(dir)?),
        "supdb-cache64tofitzpageshot4drop" => {
            Box::new(Supdb::create_cache64_tofit_zpages_hot4_drop(dir)?)
        }
        "supdb-cache64tofitzpageshot8drop" => {
            Box::new(Supdb::create_cache64_tofit_zpages_hot8_drop(dir)?)
        }
        "supdb-cache64tofitzpageshot8dropsettleevery" => Box::new(
            Supdb::create_cache64_tofit_zpages_hot8_drop_settle_every(dir)?,
        ),
        "supdb-cache64tofitzpageshot8dropsettlebuilds" => Box::new(
            Supdb::create_cache64_tofit_zpages_hot8_drop_settle_builds(dir)?,
        ),
        "supdb-cache64tofitzpagesrecodedrop" => {
            Box::new(Supdb::create_cache64_tofit_zpages_recode_drop(dir)?)
        }
        "supdb-cache64tofitzpagesrecodekeepdrop" => {
            Box::new(Supdb::create_cache64_tofit_zpages_recode_keep_drop(dir)?)
        }
        "supdb-cache64tofitzpagesrecordrawdrop" => {
            Box::new(Supdb::create_cache64_tofit_zpages_record_raw_drop(dir)?)
        }
        "supdb-cache64tofitzpagesrecordrawdecode" => {
            Box::new(Supdb::create_cache64_tofit_zpages_record_raw_decode(dir)?)
        }
        "supdb-cache64tofitzpages" => Box::new(Supdb::create_cache64_tofit_zpages(dir)?),
        "supdb-cache64tofitzpagesstored" => {
            Box::new(Supdb::create_cache64_tofit_zpages_stored(dir)?)
        }
        "supdb-nocachezpagesheads" => Box::new(Supdb::create_nocache_zpages_heads(dir)?),
        "supdb-nocachezpagesprefetch" => Box::new(Supdb::create_nocache_zpages_prefetch(dir)?),
        "supdb-nocachezpagesrankwalk" => Box::new(Supdb::create_nocache_zpages_rankwalk(dir)?),
        "supdb-nocachezpagesonegroup" => Box::new(Supdb::create_nocache_zpages_onegroup(dir)?),
        "supdb-nocachezpagessettle" => Box::new(Supdb::create_nocache_zpages_settle(dir)?),
        "supdb-autozpages" => Box::new(Supdb::create_auto_zpages(dir)?),
        "supdb-cache64tofit" => Box::new(Supdb::create_cache64_tofit(dir)?),
        "supdb-cache64tofitautozpages" => Box::new(Supdb::create_cache64_tofit_auto_zpages(dir)?),
        "supdb-cache128tofitautozpages" => Box::new(Supdb::create_cache128_tofit_auto_zpages(dir)?),
        "supdb-cache256tofitautozpages" => Box::new(Supdb::create_cache256_tofit_auto_zpages(dir)?),
        "supdb-cache64tofitautozpagesraw" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_raw(dir)?)
        }
        "supdb-cache64tofitautozpagescodingauto" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_codingauto(dir)?)
        }
        "supdb-cache64tofitautozpagespromote" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_promote(dir)?)
        }
        "supdb-cache64tofitautozpagesonegroup" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_onegroup(dir)?)
        }
        "supdb-cache64tofitautozpagesgroup1k" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_group1k(dir)?)
        }
        "supdb-cache64tofitautozpagesprebuild" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_prebuild(dir)?)
        }
        "supdb-cache64tofitautozpagesspan" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_span(dir)?)
        }
        "supdb-cache64tofitautozpagesmemolinear" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memolinear(dir)?)
        }
        "supdb-cache64tofitautozpagesexactcut" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_exactcut(dir)?)
        }
        "supdb-cache64tofitautozpagescheckedviews" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_checkedviews(dir)?)
        }
        "supdb-cache64tofitautozpagesrecord" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_record(dir)?)
        }
        "supdb-cache64tofitautozpagesmemofirst" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memofirst(dir)?)
        }
        "supdb-cache64tofitautozpagesfencetree" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_fencetree(dir)?)
        }
        "supdb-cache64tofitautozpagesmemonoahead" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memonoahead(dir)?)
        }
        "supdb-cache64tofitautozpagesstartcache" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_startcache(dir)?)
        }
        "supdb-cache64tofitautozpagesperrun" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_perrun(dir)?)
        }
        "supdb-cache64tofitautozpagesmemo4" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memo4(dir)?)
        }
        "supdb-cache64tofitautozpagesmemo16" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memo16(dir)?)
        }
        "supdb-cache64tofitautozpagesmemo32" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memo32(dir)?)
        }
        "supdb-nocacheautozpages" => Box::new(Supdb::create_nocache_auto_zpages(dir)?),
        "supdb-nocacheautozpagesfairio" => Box::new(Supdb::create_nocache_auto_zpages_fairio(dir)?),
        "supdb-nocacheautozpagesnoahead" => {
            Box::new(Supdb::create_nocache_auto_zpages_noahead(dir)?)
        }
        "supdb-nocacheautozpageseager" => Box::new(Supdb::create_nocache_auto_zpages_eager(dir)?),
        "supdb-cache256" => Box::new(Supdb::create_cache256(dir)?),
        "supdb-ingest" => Box::new(Supdb::create_ingest(dir)?),
        "supdb-ingestleave" => Box::new(Supdb::create_ingest_leave(dir)?),
        "supdb-l0" => Box::new(Supdb::create_l0(dir)?),
        "supdb-nopin" => Box::new(Supdb::create_nopin(dir)?),
        "supdb-asympin" => Box::new(Supdb::create_asympin(dir)?),
        "supdb-extsval" => Box::new(Supdb::create_extsval(dir)?),
        "supdb-piecesonly" => Box::new(Supdb::create_piecesonly(dir)?),
        "supdb-ingestpiecesonly" => Box::new(Supdb::create_ingest_piecesonly(dir)?),
        "supdb-latewb" => Box::new(Supdb::create_latewb(dir)?),
        "supdb-copysearch" => Box::new(Supdb::create_copysearch(dir)?),
        "supdb-convertsources" => Box::new(Supdb::create_convertsources(dir)?),
        "supdb-copyslack" => Box::new(Supdb::create_copyslack(dir)?),
        "supdb-manifestfree" => Box::new(Supdb::create_manifestfree(dir)?),
        "supdb-oidxsync" => Box::new(Supdb::create_oidxsync(dir)?),
        "supdb-refvals" => Box::new(Supdb::create_refvals(dir)?),
        "supdb-rowatclose" => Box::new(Supdb::create_rowatclose(dir)?),
        "supdb-ingestcopysearch" => Box::new(Supdb::create_ingest_copysearch(dir)?),
        "supdb-ingestconvertsources" => Box::new(Supdb::create_ingest_convertsources(dir)?),
        "supdb-ingestcopyslack" => Box::new(Supdb::create_ingest_copyslack(dir)?),
        "supdb-ingestdrainpromote" => Box::new(Supdb::create_ingest_drainpromote(dir)?),
        "supdb-ingestmanifestfree" => Box::new(Supdb::create_ingest_manifestfree(dir)?),
        "supdb-ingestoidxsync" => Box::new(Supdb::create_ingest_oidxsync(dir)?),
        "supdb-ingestrefvals" => Box::new(Supdb::create_ingest_refvals(dir)?),
        "supdb-ingestrowatclose" => Box::new(Supdb::create_ingest_rowatclose(dir)?),
        "supdb-rankrecords" => Box::new(Supdb::create_rankrecords(dir)?),
        "supdb-ingestrankrecords" => Box::new(Supdb::create_ingest_rankrecords(dir)?),
        "supdb-ingestlatewb" => Box::new(Supdb::create_ingest_latewb(dir)?),
        "supdb-inlinemaint" => Box::new(Supdb::create_inlinemaint(dir)?),
        "supdb-ingestinline" => Box::new(Supdb::create_ingest_inline(dir)?),
        "supdb-ingestsync" => Box::new(Supdb::create_ingest_sync(dir)?),
        "supdb-ingestshape" => Box::new(Supdb::create_ingest_shape(dir)?),
        "supdb-adapt" => Box::new(Supdb::create_adapt(dir)?),
        "supdb-adapttrig" => Box::new(Supdb::create_adapttrig(dir)?),
        "supdb-ingestadapt" => Box::new(Supdb::create_ingest_adapt(dir)?),
        "supdb-ingestadapttrig" => Box::new(Supdb::create_ingest_adapttrig(dir)?),
        "supdb-adaptloose" => Box::new(Supdb::create_adaptloose(dir)?),
        "supdb-rotate" => Box::new(Supdb::create_rotate(dir)?),
        "supdb-nofrozen" => Box::new(Supdb::create_nofrozen(dir)?),
        "supdb-shaperotate" => Box::new(Supdb::create_shape_rotate(dir)?),
        "supdb-shapedefer" => Box::new(Supdb::create_shape_defer(dir)?),
        "supdb-shapefloor" => Box::new(Supdb::create_shape_floor(dir)?),
        "supdb-shapenofrozen" => Box::new(Supdb::create_shape_nofrozen(dir)?),
        "supdb-sortover" => Box::new(Supdb::create_sortover(dir)?),
        "supdb-shapesortover" => Box::new(Supdb::create_shape_sortover(dir)?),
        "supdb-pmd" => Box::new(Supdb::create_pmd(dir)?),
        "supdb-ingestpmd" => Box::new(Supdb::create_ingest_pmd(dir)?),
        "supdb-settlefreeze" => Box::new(Supdb::create_settlefreeze(dir)?),
        "supdb-convertcap" => Box::new(Supdb::create_convertcap(dir)?),
        "supdb-shortpass" => Box::new(Supdb::create_shortpass(dir)?),
        "supdb-noyield" => Box::new(Supdb::create_noyield(dir)?),
        "supdb-postedyield" => Box::new(Supdb::create_postedyield(dir)?),
        "supdb-behindyield" => Box::new(Supdb::create_behindyield(dir)?),
        "supdb-bg" => Box::new(Supdb::create_bg(dir)?),
        "supdb-ingestbg" => Box::new(Supdb::create_ingest_bg(dir)?),
        "supdb-bglag" => Box::new(Supdb::create_bglag(dir)?),
        "supdb-ingestbglag" => Box::new(Supdb::create_ingest_bglag(dir)?),
        "supdb-lendbursts" => Box::new(Supdb::create_lendbursts(dir)?),
        "supdb-ingestlendbursts" => Box::new(Supdb::create_ingest_lendbursts(dir)?),
        "supdb-inlineall" => Box::new(Supdb::create_inlineall(dir)?),
        "supdb-ingestinlineall" => Box::new(Supdb::create_ingest_inlineall(dir)?),
        "supdb-lazysettle" => Box::new(Supdb::create_lazysettle(dir)?),
        "supdb-ingestlazysettle" => Box::new(Supdb::create_ingest_lazysettle(dir)?),
        "supdb-walsync" => Box::new(Supdb::create_walsync(dir)?),
        "supdb-ingestwalsync" => Box::new(Supdb::create_ingest_walsync(dir)?),
        "supdb-nodrainsnap" => Box::new(Supdb::create_nodrainsnap(dir)?),
        "supdb-ingestnodrainsnap" => Box::new(Supdb::create_ingest_nodrainsnap(dir)?),
        "supdb-sealrun" => Box::new(Supdb::create_sealrun(dir)?),
        "supdb-ingestsealrun" => Box::new(Supdb::create_ingest_sealrun(dir)?),
        "supdb-closeinline" => Box::new(Supdb::create_closeinline(dir)?),
        "supdb-ingestcloseinline" => Box::new(Supdb::create_ingest_closeinline(dir)?),
        "supdb-timers" => Box::new(Supdb::create_timers(dir)?),
        "supdb-ingesttimers" => Box::new(Supdb::create_ingest_timers(dir)?),
        "supdb-wake" => Box::new(Supdb::create_wake(dir)?),
        "supdb-ingestwake" => Box::new(Supdb::create_ingest_wake(dir)?),
        "supdb-noprefetch" => Box::new(Supdb::create_noprefetch(dir)?),
        "supdb-ingestnoprefetch" => Box::new(Supdb::create_ingest_noprefetch(dir)?),
        "supdb-dirplain" => Box::new(Supdb::create_dirplain(dir)?),
        "supdb-ingestdirplain" => Box::new(Supdb::create_ingest_dirplain(dir)?),
        "supdb-settlefar" => Box::new(Supdb::create_settlefar(dir)?),
        "supdb-ingestsettlefar" => Box::new(Supdb::create_ingest_settlefar(dir)?),
        "supdb-tofit" => Box::new(Supdb::create_tofit(dir)?),
        "supdb-ingesttofit" => Box::new(Supdb::create_ingest_tofit(dir)?),
        "supdb-cache256tofit" => Box::new(Supdb::create_cache256_tofit(dir)?),
        "supdb-cache256tofitnotrim" => Box::new(Supdb::create_cache256_tofit_notrim(dir)?),
        "supdb-cache256tofitnoshed" => Box::new(Supdb::create_cache256_tofit_noshed(dir)?),
        "supdb-cache256tofitnosweep" => Box::new(Supdb::create_cache256_tofit_nosweep(dir)?),
        "supdb-prefetch1k" => Box::new(Supdb::create_prefetch1k(dir)?),
        "supdb-ingestprefetch1k" => Box::new(Supdb::create_ingest_prefetch1k(dir)?),
        "supdb-manifestrename" => Box::new(Supdb::create_manifestrename(dir)?),
        "supdb-manifestpending" => Box::new(Supdb::create_manifestpending(dir)?),
        "supdb-ingestmanifestrename" => Box::new(Supdb::create_ingest_manifestrename(dir)?),
        "supdb-ingestmanifestpending" => Box::new(Supdb::create_ingest_manifestpending(dir)?),
        "supdb-populate" => Box::new(Supdb::create_populate(dir)?),
        "supdb-ingestpopulate" => Box::new(Supdb::create_ingest_populate(dir)?),
        "supdb-piece256" => Box::new(Supdb::create_piece256(dir)?),
        "supdb-ingestpiece256" => Box::new(Supdb::create_ingest_piece256(dir)?),
        "supdb-recordsteps" => Box::new(Supdb::create_recordsteps(dir)?),
        "supdb-ingestrecordsteps" => Box::new(Supdb::create_ingest_recordsteps(dir)?),
        "supdb-copyvals" => Box::new(Supdb::create_copyvals(dir)?),
        "supdb-ingestcopyvals" => Box::new(Supdb::create_ingest_copyvals(dir)?),
        "supdb-keycopies" => Box::new(Supdb::create_keycopies(dir)?),
        "supdb-ingestkeycopies" => Box::new(Supdb::create_ingest_keycopies(dir)?),
        "supdb-fillplain" => Box::new(Supdb::create_fillplain(dir)?),
        "supdb-ingestfillplain" => Box::new(Supdb::create_ingest_fillplain(dir)?),
        "supdb-latesync" => Box::new(Supdb::create_latesync(dir)?),
        "supdb-ingestlatesync" => Box::new(Supdb::create_ingest_latesync(dir)?),
        "supdb-norecycle" => Box::new(Supdb::create_norecycle(dir)?),
        "supdb-ingestnorecycle" => Box::new(Supdb::create_ingest_norecycle(dir)?),
        "supdb-keysarena" => Box::new(Supdb::create_keysarena(dir)?),
        "supdb-ingestkeysarena" => Box::new(Supdb::create_ingest_keysarena(dir)?),
        "supdb-noalert" => Box::new(Supdb::create_noalert(dir)?),
        "supdb-ingestnoalert" => Box::new(Supdb::create_ingest_noalert(dir)?),
        "supdb-ordlog" => Box::new(Supdb::create_ordlog(dir)?),
        "supdb-ingestordlog" => Box::new(Supdb::create_ingest_ordlog(dir)?),
        "supdb-fatentries" => Box::new(Supdb::create_fatentries(dir)?),
        "supdb-ingestfatentries" => Box::new(Supdb::create_ingest_fatentries(dir)?),
        "supdb-waitmanifest" => Box::new(Supdb::create_waitmanifest(dir)?),
        "supdb-ingestwaitmanifest" => Box::new(Supdb::create_ingest_waitmanifest(dir)?),
        "supdb-ingestsettlefreeze" => Box::new(Supdb::create_ingest_settlefreeze(dir)?),
        "supdb-ingestconvertcap" => Box::new(Supdb::create_ingest_convertcap(dir)?),
        "supdb-ingestshortpass" => Box::new(Supdb::create_ingest_shortpass(dir)?),
        "supdb-ingestnoyield" => Box::new(Supdb::create_ingest_noyield(dir)?),
        "supdb-ingestpostedyield" => Box::new(Supdb::create_ingest_postedyield(dir)?),
        "supdb-ingestbehindyield" => Box::new(Supdb::create_ingest_behindyield(dir)?),
        "supdb-idle" => Box::new(Supdb::create_idle(dir)?),
        "supdb-shapeidle" => Box::new(Supdb::create_shape_idle(dir)?),
        "supdb-ingestadaptloose" => Box::new(Supdb::create_ingest_adaptloose(dir)?),
        "supdb-ingestnoseal" => Box::new(Supdb::create_ingest_noseal(dir)?),
        "lmdb" => Box::new(Lmdb::create(dir, map_gb)?),
        "lmdb-nordahead" => Box::new(Lmdb::create_nordahead(dir, map_gb)?),
        "lmdb-nosync" => Box::new(Lmdb::create_nosync(dir, map_gb)?),
        "rocksdb-tuned" => Box::new(Rocks::create_tuned(dir)?),
        "rocksdb-nosync" => Box::new(Rocks::create(dir, false)?),
        other => return Err(format!("no such arm: {other}")),
    })
}

/// Every arm in a guarantee must promise the same things, or the comparison
/// is not one. Checked once at the start of a run and fatal if it fails:
/// a mismatched pair is a bug in this file, not a measurement.
pub fn check_matched(arms: &[String], dir: &Path) -> Res<()> {
    let mut by_g: std::collections::HashMap<Guarantee, Vec<(String, Features)>> =
        Default::default();
    for a in arms {
        let g = guarantee(a).ok_or_else(|| format!("no such arm: {a}"))?;
        let d = dir.join(format!("probe-{a}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let f = open(a, &d, 1)?.features();
        let _ = std::fs::remove_dir_all(&d);
        by_g.entry(g).or_default().push((a.clone(), f));
    }
    for (g, list) in by_g {
        for w in list.windows(2) {
            let gap = w[0].1.unmatched(&w[1].1);
            if !gap.is_empty() {
                return Err(format!(
                    "{} and {} are both {:?} but differ on {}",
                    w[0].0,
                    w[1].0,
                    g,
                    gap.join(", ")
                ));
            }
        }
    }
    Ok(())
}

/// Every arm either roster runs, each once, `ARMS` first and in its
/// order: what a test over "every arm" walks, so an arm only the bounded
/// roster names is not an arm nothing tests, and the order the figures
/// draw curves in, so an arm only that roster names is not a curve
/// nothing draws.
pub fn every_arm() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = ARMS.to_vec();
    for a in OOC_ARMS {
        if !v.contains(&a) {
            v.push(a);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The readahead arm is the durable LMDB arm with one flag more, and
    /// nothing else: named, durable, in the bounded roster, and opened
    /// with exactly `NO_READ_AHEAD` over the flags `lmdb` is opened with.
    /// Copied from the matching test's shape: a difference that is not
    /// the one the arm is for is a second arm nobody declared.
    #[test]
    fn the_readahead_arm_differs_from_lmdb_in_one_flag() {
        assert_eq!(guarantee("lmdb-nordahead"), Some(Guarantee::Durable));
        assert!(OOC_ARMS.contains(&"lmdb-nordahead"));
        assert!(OOC_ARMS.contains(&"lmdb"));
        let root =
            std::env::temp_dir().join(format!("supdb-bench-nordahead-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plain = Lmdb::create(&root.join("lmdb"), 1).unwrap();
        let off = Lmdb::create_nordahead(&root.join("lmdb-nordahead"), 1).unwrap();
        assert_eq!(plain.name(), "lmdb");
        assert_eq!(off.name(), "lmdb-nordahead");
        assert_eq!(plain.features(), off.features());
        assert!(plain.features().durable_commit);
        assert_eq!(
            off.env_flags() ^ plain.env_flags(),
            heed::EnvFlags::NO_READ_AHEAD
        );
        assert!(!plain.env_flags().contains(heed::EnvFlags::NO_READ_AHEAD));
        drop(plain);
        drop(off);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The buffered group is defined by what it gives up: an arm that
    /// commits durably does not belong in it, whatever its row says.
    #[test]
    fn durability_is_an_axis_the_matching_checks() {
        let buffered = Features {
            durable_commit: false,
            transactions: false,
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        };
        let durable = Features {
            durable_commit: true,
            ..buffered
        };
        assert_eq!(durable.unmatched(&buffered), vec!["durable_commit"]);
        assert!(buffered.unmatched(&buffered).is_empty());
    }

    /// A pass drops its first store, removes the directory and opens a
    /// second at the same path, so every arm's drop must close what it
    /// opened. heed keeps each environment in a registry of its own until
    /// asked to close it, and the LMDB arm never asked: its second open got
    /// the first store back, deleted from the directory and still mapped, so
    /// LMDB's shuffled load and lag sweep ran over its ordered load's tree,
    /// and every store it opened held its disk until the run exited.
    #[test]
    fn a_store_reopened_where_its_predecessor_was_is_a_fresh_store() {
        let root = std::env::temp_dir().join(format!("supdb-bench-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (k, v) = (b"k0000000000000001".as_slice(), [7u8; 100]);
        for arm in every_arm() {
            let dir = root.join(arm);
            std::fs::create_dir_all(&dir).unwrap();
            let mut e = open(arm, &dir, 1).unwrap();
            e.write_batch(&[(k, &v)]).unwrap();
            e.sync().unwrap();
            assert_eq!(e.get(k).unwrap(), v.len(), "{arm}: the first store");
            drop(e);
            std::fs::remove_dir_all(&dir).unwrap();
            #[cfg(target_os = "linux")]
            {
                let held: Vec<_> = std::fs::read_dir("/proc/self/fd")
                    .unwrap()
                    .filter_map(|f| std::fs::read_link(f.ok()?.path()).ok())
                    .filter(|p| p.starts_with(&dir))
                    .collect();
                assert!(held.is_empty(), "{arm}: dropped and still open: {held:?}");
            }
            std::fs::create_dir_all(&dir).unwrap();
            let mut e = open(arm, &dir, 1).unwrap();
            assert_eq!(
                e.get(k).unwrap(),
                0,
                "{arm}: the second store read the first's key"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-dirplain` and `supdb-ingestdirplain` are the policy with
    /// `dirplain` alone set over their row's default, open under that
    /// name, and the options they open the engine with differ from the
    /// default's in `seek_prefetch_dir` alone on the scan's prefetch
    /// path -- an arm sound only beside the default it is priced with
    /// keeps that default's other settings (`CLAUDE.md`).
    #[test]
    fn the_dirplain_arms_differ_from_their_defaults_in_the_directory_hint_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-dirplain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, buffered) in [("supdb-dirplain", false), ("supdb-ingestdirplain", true)] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            let arm = if buffered {
                Supdb::create_ingest_dirplain(&d)
            } else {
                Supdb::create_dirplain(&d)
            }
            .unwrap();
            assert_eq!(arm.name(), name);
            assert!(arm.dirplain && !arm.noprefetch && !arm.prefetch1k, "{name}");
            let base_d = root.join(format!("{name}-base"));
            std::fs::create_dir_all(&base_d).unwrap();
            let base = if buffered {
                Supdb::create_ingest(&base_d)
            } else {
                Supdb::create(&base_d)
            }
            .unwrap();
            assert_eq!(arm.partition, base.partition, "{name}: the row's shape");
            assert_eq!(arm.durable, base.durable, "{name}: the row's guarantee");
            assert_eq!(arm.features(), base.features(), "{name}");
            let (a, b) = (
                arm.db.as_ref().unwrap().options(),
                base.db.as_ref().unwrap().options(),
            );
            assert!(
                !a.seek_prefetch_dir && b.seek_prefetch_dir,
                "{name}: the one option"
            );
            assert_eq!(a.scan_prefetch, b.scan_prefetch, "{name}");
            assert_eq!(a.scan_prefetch_bytes, b.scan_prefetch_bytes, "{name}");
            drop(arm);
            drop(base);
            // The dispatch opens the arm by its name.
            let opened_d = root.join(format!("{name}-open"));
            std::fs::create_dir_all(&opened_d).unwrap();
            let e = open(name, &opened_d, 1).unwrap();
            assert_eq!(e.name(), name, "the dispatch");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-nocacheordread` is the nocache policy with `ordread` alone
    /// set over `supdb-nocache`: named, durable like it, opened under its
    /// name by the dispatch, and opened on options that differ from
    /// `supdb-nocache`'s in `point_via_ord` alone among the read path's and
    /// the policy's (`CLAUDE.md`: an arm that keeps an old shape keeps
    /// that shape's other settings with it). And the path is taken: the
    /// same keys written and read through each, the arm's reads are
    /// answered through the partition's ordered index and the base's are
    /// not, and the counters a pairing prints say so, for the writer's
    /// handle and for the handles a threaded pass hands its threads.
    #[test]
    fn the_ordread_arm_differs_from_nocache_in_the_partition_read_path_alone() {
        let root = std::env::temp_dir().join(format!("supdb-bench-ordread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, arm: bool| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            if arm {
                Supdb::create_nocache_ordread(&d)
            } else {
                Supdb::create_nocache(&d)
            }
            .unwrap()
        };
        let (mut arm, mut base) = (new("arm", true), new("base", false));
        assert_eq!(arm.name(), "supdb-nocacheordread");
        assert_eq!(base.name(), "supdb-nocache");
        assert_eq!(guarantee("supdb-nocacheordread"), Some(Guarantee::Durable));
        assert_eq!(
            guarantee("supdb-nocacheordread"),
            guarantee("supdb-nocache")
        );
        assert!(arm.ordread && !base.ordread);
        // The policy: every flag the arm could have set beside its own.
        assert_eq!(arm.partition, base.partition, "the row's shape");
        assert_eq!(arm.drain, base.drain);
        assert_eq!(arm.durable, base.durable, "the row's guarantee");
        assert_eq!(arm.block_cache, base.block_cache);
        assert!(!arm.block_cache, "the nocache policy");
        assert_eq!(arm.budget, base.budget);
        assert_eq!(arm.advice, base.advice);
        assert_eq!(arm.features(), base.features());
        assert!(!arm.extsval && !arm.dirplain && !arm.forms && !arm.lazyforms);
        let (a, b) = (
            arm.db.as_ref().unwrap().options(),
            base.db.as_ref().unwrap().options(),
        );
        assert!(a.point_via_ord && !b.point_via_ord, "the one option");
        assert_eq!(a.exts_by_value, b.exts_by_value);
        assert_eq!(a.scan_block_cache, b.scan_block_cache);
        assert_eq!(a.scan_cache_bytes, b.scan_cache_bytes);
        assert_eq!(a.read_advice, b.read_advice);
        assert_eq!(a.seek_prefetch_dir, b.seek_prefetch_dir);
        assert_eq!(a.scan_prefetch, b.scan_prefetch);
        assert_eq!(a.inline_bytes, b.inline_bytes);
        assert_eq!(a.partition_bytes, b.partition_bytes);
        assert_eq!(a.seal_bytes, b.seal_bytes);
        assert_eq!(a.compact, b.compact);
        assert_eq!(a.promote, b.promote);
        assert_eq!(a.sync, b.sync);
        assert_eq!(a.upkeep, b.upkeep);
        assert_eq!(a.writer_pins, b.writer_pins);
        assert_eq!(a.publish_in_background, b.publish_in_background);
        // The default is untouched: the shipping arm reads as it always did.
        assert!(!supdb::Options::default().point_via_ord);

        // The path is taken by the arm and by nothing else. Keys of the
        // suite's shape, in key order, drained into a partition.
        let value = [7u8; 100];
        let keys: Vec<[u8; 16]> = (0..20_000u64)
            .map(|i| {
                let mut k = [0u8; 16];
                crate::workload::db_key_into(i, &mut k);
                k
            })
            .collect();
        for e in [&mut arm, &mut base] {
            for chunk in keys.chunks(1_000) {
                let items: Vec<(&[u8], &[u8])> =
                    chunk.iter().map(|k| (&k[..], &value[..])).collect();
                e.write_batch(&items).unwrap();
            }
            e.sync().unwrap();
            assert!(e.db.as_ref().unwrap().levels().0 >= 1, "a partition");
        }
        let (mut reads, absent) = (0f64, [0xffu8; 16]);
        for k in keys.iter().step_by(7) {
            assert_eq!(arm.get(k).unwrap(), value.len());
            assert_eq!(base.get(k).unwrap(), value.len());
            reads += 1.0;
        }
        assert_eq!(arm.get(&absent).unwrap(), 0);
        assert_eq!(base.get(&absent).unwrap(), 0);
        reads += 1.0;
        let count = |e: &Supdb, name: &str| {
            e.counters()
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("no counter {name}"))
                .1
        };
        assert_eq!(count(&arm, "ord_point_reads"), reads, "every read");
        assert_eq!(count(&arm, "ord_point_declined"), 0.0);
        assert_eq!(count(&base, "ord_point_reads"), 0.0, "the default's path");
        assert_eq!(count(&base, "ord_point_declined"), 0.0);
        assert_eq!(count(&arm, "ord_point_reads_threads"), 0.0, "no handle yet");

        // The handles a threaded pass hands its threads count apart, and
        // what each answered reaches the store's counters when it is
        // dropped, so the threaded cells of a pairing print whether they
        // took the path. A handle on each store reads the same keys, and
        // the writer's own words stay where they were.
        let handle_reads = (keys.iter().step_by(11).count() + 1) as f64;
        for e in [&arm, &base] {
            let mut r = (e.thread_reader().unwrap())().unwrap();
            for k in keys.iter().step_by(11) {
                assert_eq!(r.get(k).unwrap(), value.len());
            }
            assert_eq!(r.get(&absent).unwrap(), 0);
            drop(r);
        }
        assert_eq!(count(&arm, "ord_point_reads_threads"), handle_reads);
        assert_eq!(count(&arm, "ord_point_declined_threads"), 0.0);
        assert_eq!(count(&base, "ord_point_reads_threads"), 0.0);
        assert_eq!(count(&base, "ord_point_declined_threads"), 0.0);
        assert_eq!(count(&arm, "ord_point_reads"), reads, "the writer's own");
        drop(arm);
        drop(base);
        // The dispatch opens the arm by its name.
        let opened = root.join("open");
        std::fs::create_dir_all(&opened).unwrap();
        let e = open("supdb-nocacheordread", &opened, 1).unwrap();
        assert_eq!(e.name(), "supdb-nocacheordread", "the dispatch");
        drop(e);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-nocachezpages` is the nocache policy with `zpages` alone set
    /// over `supdb-nocache`: named, durable like it, opened under its name
    /// by the dispatch, and opened on options that differ from
    /// `supdb-nocache`'s in `Options::zpages` alone, the floor under which
    /// a partition is left raw included (`CLAUDE.md`: an arm that keeps an
    /// old shape keeps that shape's other settings with it). And the form
    /// is used: the same keys written and read through each, the arm's
    /// partition is given its form file by its `sync` and its reads are
    /// answered from it, the base's are not, for the writer's own handle
    /// and for the handles a threaded pass hands its threads, whose counts
    /// are in the store's after they are dropped.
    #[test]
    fn the_zpages_arm_differs_from_nocache_in_the_option_alone() {
        let root = std::env::temp_dir().join(format!("supdb-bench-zpages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, arm: bool| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            if arm {
                Supdb::create_nocache_zpages(&d)
            } else {
                Supdb::create_nocache(&d)
            }
            .unwrap()
        };
        let (mut arm, mut base) = (new("arm", true), new("base", false));
        assert_eq!(arm.name(), "supdb-nocachezpages");
        assert_eq!(base.name(), "supdb-nocache");
        assert_eq!(guarantee("supdb-nocachezpages"), Some(Guarantee::Durable));
        assert_eq!(guarantee("supdb-nocachezpages"), guarantee("supdb-nocache"));
        assert!(arm.zpages && !base.zpages);
        // The policy: every flag the arm could have set beside its own.
        assert_eq!(arm.partition, base.partition, "the row's shape");
        assert_eq!(arm.drain, base.drain);
        assert_eq!(arm.durable, base.durable, "the row's guarantee");
        assert_eq!(arm.block_cache, base.block_cache);
        assert!(!arm.block_cache, "the nocache policy");
        assert_eq!(arm.budget, base.budget);
        assert_eq!(arm.advice, base.advice);
        assert_eq!(arm.features(), base.features());
        assert!(!arm.ordread && !arm.extsval && !arm.dirplain && !arm.forms && !arm.lazyforms);
        let (a, b) = (
            arm.db.as_ref().unwrap().options(),
            base.db.as_ref().unwrap().options(),
        );
        assert!(
            a.zpages == supdb::ZPages::Always && b.zpages == supdb::ZPages::Off,
            "the one option"
        );
        assert_eq!(a.zpages_min_bytes, b.zpages_min_bytes, "the engine's floor");
        assert_eq!(
            a.zpages_drop_behind, b.zpages_drop_behind,
            "the drop behind the build"
        );
        assert!(a.zpages_drop_behind, "the engine's own, on");
        assert_eq!(a.point_via_ord, b.point_via_ord);
        assert_eq!(a.exts_by_value, b.exts_by_value);
        assert_eq!(a.scan_block_cache, b.scan_block_cache);
        assert_eq!(a.scan_cache_bytes, b.scan_cache_bytes);
        assert_eq!(a.read_advice, b.read_advice);
        assert_eq!(a.seek_prefetch_dir, b.seek_prefetch_dir);
        assert_eq!(a.scan_prefetch, b.scan_prefetch);
        assert_eq!(a.inline_bytes, b.inline_bytes);
        assert_eq!(a.partition_bytes, b.partition_bytes);
        assert_eq!(a.seal_bytes, b.seal_bytes);
        assert_eq!(a.compact, b.compact);
        assert_eq!(a.promote, b.promote);
        assert_eq!(a.sync, b.sync);
        assert_eq!(a.upkeep, b.upkeep);
        assert_eq!(a.writer_pins, b.writer_pins);
        assert!(
            a.writer_pins,
            "the form is refused beside an unpinned writer"
        );
        assert_eq!(a.publish_in_background, b.publish_in_background);
        // The default is untouched: the shipping arm reads as it always did.
        assert_eq!(supdb::Options::default().zpages, supdb::ZPages::Off);
        let shipping = {
            let d = root.join("shipping");
            std::fs::create_dir_all(&d).unwrap();
            Supdb::create(&d).unwrap()
        };
        assert!(!shipping.zpages);
        assert_eq!(
            shipping.db.as_ref().unwrap().options().zpages,
            supdb::ZPages::Off
        );
        drop(shipping);

        // The form is used by the arm and by nothing else. Keys of the
        // suite's shape in key order, drained into a partition, and enough
        // of them to stand over the floor a partition is left raw under.
        let value = [7u8; 100];
        let keys: Vec<[u8; 16]> = (0..30_000u64)
            .map(|i| {
                let mut k = [0u8; 16];
                crate::workload::db_key_into(i, &mut k);
                k
            })
            .collect();
        for e in [&mut arm, &mut base] {
            for chunk in keys.chunks(1_000) {
                let items: Vec<(&[u8], &[u8])> =
                    chunk.iter().map(|k| (&k[..], &value[..])).collect();
                e.write_batch(&items).unwrap();
            }
            e.sync().unwrap();
            assert!(e.db.as_ref().unwrap().levels().0 >= 1, "a partition");
        }
        let (az, bz) = (arm.zpages_state().unwrap(), base.zpages_state().unwrap());
        assert!(az.forced && !bz.forced, "{az:?} {bz:?}");
        assert!(
            az.partitions >= 1 && az.form_files == az.partitions && az.small == 0,
            "every partition has its form file when the sync returns: {az:?}"
        );
        assert_eq!((bz.form_files, bz.small), (0, 0), "{bz:?}");
        // The directory holds the form files and no other file of the
        // form, which is what a pass asks of it.
        assert_eq!((az.files, az.stray), (az.form_files, 0), "{az:?}");
        assert_eq!((bz.files, bz.stray), (0, 0), "{bz:?}");
        // And the count sees a file that is not a live form file's, of either
        // kind: a whole one nobody claims, and a build's temp name.
        let (whole, temp) = (
            arm.path.join("zpg-00000999-0000000000000999.zpg"),
            arm.path.join("zpg-00000999-0000000000000999.zpg.tmp"),
        );
        std::fs::write(&whole, b"nobody's").unwrap();
        std::fs::write(&temp, b"half a build").unwrap();
        let planted = arm.zpages_state().unwrap();
        assert_eq!(
            (planted.files, planted.stray),
            (az.files + 1, 1),
            "{planted:?}"
        );
        std::fs::remove_file(&whole).unwrap();
        std::fs::remove_file(&temp).unwrap();
        let (mut reads, absent) = (0f64, [0xffu8; 16]);
        for k in keys.iter().step_by(7) {
            assert_eq!(arm.get(k).unwrap(), value.len());
            assert_eq!(base.get(k).unwrap(), value.len());
            reads += 1.0;
        }
        assert_eq!(arm.get(&absent).unwrap(), 0);
        assert_eq!(base.get(&absent).unwrap(), 0);
        let count = |e: &Supdb, name: &str| {
            e.counters()
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("no counter {name}"))
                .1
        };
        // A key the segment's Bloom rules out never reaches the partition's
        // read, so the absent key is in the count when the filter lets it
        // through and not when it does not.
        let served = count(&arm, "zpage_reads");
        assert!(
            served == reads || served == reads + 1.0,
            "{served} of {reads}"
        );
        assert_eq!(count(&arm, "zpage_rawserves"), 0.0, "none left raw");
        assert_eq!(count(&arm, "zpage_fallbacks"), 0.0, "none refused");
        assert_eq!(count(&arm, "zpage_forced"), 1.0);
        assert_eq!(count(&arm, "zpage_form files_at_end"), az.form_files as f64);
        assert_eq!(count(&base, "zpage_forced"), 0.0);
        for name in [
            "zpage_reads",
            "zpage_rawserves",
            "zpage_builds",
            "zpage_form files_at_end",
        ] {
            assert_eq!(count(&base, name), 0.0, "the default's path: {name}");
        }
        assert_eq!(count(&arm, "zpage_builds"), az.form_files as f64);

        // The handles a threaded pass hands its threads count on their
        // slots, which a handle's drop leaves where they are: the counters
        // read after a pass carry the whole of it.
        let handle_reads = keys.iter().step_by(11).count() as f64;
        for e in [&arm, &base] {
            let mut r = (e.thread_reader().unwrap())().unwrap();
            for k in keys.iter().step_by(11) {
                assert_eq!(r.get(k).unwrap(), value.len());
            }
            drop(r);
        }
        assert_eq!(count(&arm, "zpage_reads"), served + handle_reads);
        assert_eq!(count(&base, "zpage_reads"), 0.0);
        // A scan's walk of the partition is served by its form file, and
        // counted as such; the base's is nobody's to count.
        assert_eq!(arm.range(&keys[0], 10).unwrap(), 10 * value.len());
        assert_eq!(count(&arm, "zpage_scans"), 1.0);
        assert_eq!(count(&arm, "zpage_raw_scans"), 0.0);
        assert_eq!(count(&base, "zpage_scans"), 0.0);
        assert_eq!(count(&base, "zpage_raw_scans"), 0.0);
        drop(arm);
        drop(base);
        // The dispatch opens the arm by its name.
        let opened = root.join("open");
        std::fs::create_dir_all(&opened).unwrap();
        let e = open("supdb-nocachezpages", &opened, 1).unwrap();
        assert_eq!(e.name(), "supdb-nocachezpages", "the dispatch");
        drop(e);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the arm's state says of a store whose form file a check refused: the
    /// refusal that reads it is tested on states it is handed, and this is
    /// the state a real engine hands it. A page of a form file is damaged
    /// before anything reads it; the read that meets it falls back to the raw
    /// form and answers right, and the state counts that read as raw and the
    /// form file as out of service, then the reads after it as raw too.
    #[test]
    fn the_arms_state_says_what_a_damaged_form_file_did() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-zdamaged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut arm = Supdb::create_nocache_zpages(&root).unwrap();
        let value = [7u8; 100];
        let keys: Vec<[u8; 16]> = (0..30_000u64)
            .map(|i| {
                let mut k = [0u8; 16];
                crate::workload::db_key_into(i, &mut k);
                k
            })
            .collect();
        for chunk in keys.chunks(1_000) {
            let items: Vec<(&[u8], &[u8])> = chunk.iter().map(|k| (&k[..], &value[..])).collect();
            arm.write_batch(&items).unwrap();
        }
        arm.sync().unwrap();
        let whole = arm.zpages_state().unwrap();
        assert!(whole.form_files >= 1 && whole.form_files == whole.partitions);
        assert_eq!((whole.damaged, whole.raw), (0, 0), "{whole:?}");
        // The first page of every form file, one byte changed under the file
        // the store holds mapped: the page's CRC no longer holds, and no
        // read has verified it yet.
        let mut changed = 0;
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "zpg") {
                use std::os::unix::fs::FileExt;
                let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                f.write_all_at(&[0xA5], 200).unwrap();
                changed += 1;
            }
        }
        assert_eq!(changed, whole.form_files);
        // The read is answered, from the raw form, and counted as that.
        assert_eq!(arm.get(&keys[0]).unwrap(), value.len());
        let hurt = arm.zpages_state().unwrap();
        assert_eq!((hurt.damaged, hurt.raw), (1, 1), "{hurt:?}");
        assert_eq!(hurt.reads, whole.reads, "no read was served by the form");
        // The form file is out of service from then on: the next read of the
        // partition is raw too, without a check failing again.
        assert_eq!(arm.get(&keys[1]).unwrap(), value.len());
        let after = arm.zpages_state().unwrap();
        assert_eq!((after.damaged, after.raw), (1, 2), "{after:?}");
        drop(arm);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-autozpages` is `supdb` with the controller and nothing else,
    /// and `supdb-nocacheautozpages` is `supdb-nocache` the same way:
    /// named, durable like their bases, opened under their names by the
    /// dispatch, the form the controller's by the engine's own options and
    /// neither forced nor off, and every other option the base's.
    #[test]
    fn the_fair_io_arm_differs_from_the_controllers_in_the_io_class_alone() {
        let root = std::env::temp_dir().join(format!("supdb-bench-fairio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, make: fn(&Path) -> Res<Supdb>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            make(&d).unwrap()
        };
        // The engine's own default: the form's threads at the idle class,
        // so an arm that does not pin it has the class the engine ships.
        assert!(supdb::Options::default().zpages_io_idle);
        let base = new("base", Supdb::create_nocache_auto_zpages);
        let arm = new("fairio", Supdb::create_nocache_auto_zpages_fairio);
        assert_eq!(base.name(), "supdb-nocacheautozpages");
        assert_eq!(arm.name(), "supdb-nocacheautozpagesfairio");
        assert_eq!(guarantee(arm.name()), guarantee(base.name()));
        assert!(base.zpages_auto && arm.zpages_auto, "both the controller's");
        assert!(!base.zpages_fairio && arm.zpages_fairio);
        assert_eq!(arm.block_cache, base.block_cache);
        assert_eq!(arm.features(), base.features());
        let b = base.db.as_ref().unwrap().options();
        let a = arm.db.as_ref().unwrap().options();
        assert!(b.zpages_io_idle && !a.zpages_io_idle, "the one option");
        assert_eq!(a.zpages_build_ahead, b.zpages_build_ahead);
        assert_eq!(a.zpages, b.zpages);
        // And the arm that asks nothing ahead: the same base, its one
        // option the window, the engine's own default four mebibytes.
        assert_eq!(supdb::Options::default().zpages_build_ahead, 4 << 20);
        let ahead = new("noahead", Supdb::create_nocache_auto_zpages_noahead);
        assert_eq!(ahead.name(), "supdb-nocacheautozpagesnoahead");
        assert_eq!(guarantee(ahead.name()), guarantee(base.name()));
        assert!(ahead.zpages_auto && ahead.zpages_noahead && !ahead.zpages_fairio);
        assert_eq!(ahead.block_cache, base.block_cache);
        assert_eq!(ahead.features(), base.features());
        let h = ahead.db.as_ref().unwrap().options();
        assert_eq!(
            (h.zpages_build_ahead, b.zpages_build_ahead),
            (0, 4 << 20),
            "the one option"
        );
        assert_eq!(h.zpages_io_idle, b.zpages_io_idle);
        assert_eq!(h.zpages, b.zpages);
        drop(ahead);
        // And the eager arm: the same base, its one option the entry.
        assert!(!supdb::Options::default().zpages_build_eager);
        let eager = new("eager", Supdb::create_nocache_auto_zpages_eager);
        assert_eq!(eager.name(), "supdb-nocacheautozpageseager");
        assert_eq!(guarantee(eager.name()), guarantee(base.name()));
        assert!(eager.zpages_auto && eager.zpages_buildeager);
        assert!(!eager.zpages_noahead && !eager.zpages_fairio);
        assert_eq!(eager.block_cache, base.block_cache);
        assert_eq!(eager.features(), base.features());
        let e = eager.db.as_ref().unwrap().options();
        assert!(
            e.zpages_build_eager && !b.zpages_build_eager,
            "the one option"
        );
        assert_eq!(
            (e.zpages_build_ahead, e.zpages_io_idle, e.zpages),
            (b.zpages_build_ahead, b.zpages_io_idle, b.zpages)
        );
        drop(eager);
        assert_eq!(
            (
                a.zpages_warm,
                a.zpages_eager,
                a.zpages_memo,
                a.zpages_walk_cursor
            ),
            (
                b.zpages_warm,
                b.zpages_eager,
                b.zpages_memo,
                b.zpages_walk_cursor
            )
        );
        drop(arm);
        drop(base);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_auto_arm_differs_from_its_base_in_the_controller_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-autoarms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, make: fn(&Path) -> Res<Supdb>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            make(&d).unwrap()
        };
        for (name, make, base_name, base_make) in [
            (
                "supdb-autozpages",
                Supdb::create_auto_zpages as fn(&Path) -> Res<Supdb>,
                "supdb",
                Supdb::create as fn(&Path) -> Res<Supdb>,
            ),
            (
                "supdb-nocacheautozpages",
                Supdb::create_nocache_auto_zpages,
                "supdb-nocache",
                Supdb::create_nocache,
            ),
            (
                "supdb-cache64tofitautozpages",
                Supdb::create_cache64_tofit_auto_zpages,
                "supdb-cache64tofit",
                Supdb::create_cache64_tofit,
            ),
        ] {
            let arm = new(name, make);
            let base = new(base_name, base_make);
            assert_eq!(arm.name(), name);
            assert_eq!(base.name(), base_name);
            assert_eq!(guarantee(name), Some(Guarantee::Durable));
            assert_eq!(guarantee(name), guarantee(base_name));
            let (a, b) = (
                arm.db.as_ref().unwrap().options(),
                base.db.as_ref().unwrap().options(),
            );
            assert_eq!(a.zpages, supdb::ZPages::Auto);
            assert_eq!(b.zpages, supdb::ZPages::Off);
            assert!(arm.zpages_auto && !base.zpages_auto && !arm.zpages);
            assert_eq!(arm.block_cache, base.block_cache);
            assert_eq!(a.scan_block_cache, b.scan_block_cache);
            assert_eq!(arm.partition, base.partition);
            assert_eq!(arm.drain, base.drain);
            assert_eq!(arm.durable, base.durable);
            assert_eq!(arm.budget, base.budget);
            assert_eq!(arm.advice, base.advice);
            assert_eq!(arm.features(), base.features());
            let state = arm.zpages_state().unwrap();
            assert!(state.auto && !state.forced, "{state:?}");
            let state = base.zpages_state().unwrap();
            assert!(!state.auto && !state.forced, "{state:?}");
            let d = root.join(format!("{name}-dispatch"));
            std::fs::create_dir_all(&d).unwrap();
            let opened = open(name, &d, 1).unwrap();
            assert_eq!(opened.name(), name);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-cache64tofitautozpagescodingauto` is `supdb-cache64tofitautozpages`
    /// with the coding the selector's to choose (`Options::zpages_coding_auto`)
    /// and nothing else: named, durable like it, opened under its name by
    /// the dispatch, the codings the options name off in both, and the
    /// engine's default off.
    #[test]
    fn the_coding_gear_arm_differs_from_the_combined_arm_in_the_choice_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-codingauto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, make: fn(&Path) -> Res<Supdb>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            make(&d).unwrap()
        };
        assert!(!supdb::Options::default().zpages_coding_auto);
        let base = new("base", Supdb::create_cache64_tofit_auto_zpages);
        let arm = new("gear", Supdb::create_cache64_tofit_auto_zpages_codingauto);
        assert_eq!(base.name(), "supdb-cache64tofitautozpages");
        assert_eq!(arm.name(), "supdb-cache64tofitautozpagescodingauto");
        assert_eq!(guarantee(arm.name()), Some(Guarantee::Durable));
        assert_eq!(guarantee(arm.name()), guarantee(base.name()));
        assert!(arm.zcodingauto && !base.zcodingauto);
        assert!(arm.zpages_auto && base.zpages_auto && arm.tofit && base.tofit);
        assert_eq!(arm.features(), base.features());
        let (a, b) = (
            arm.db.as_ref().unwrap().options(),
            base.db.as_ref().unwrap().options(),
        );
        assert!(
            a.zpages_coding_auto && !b.zpages_coding_auto,
            "the one option"
        );
        assert_eq!(
            (
                a.zpages,
                a.zpages_record_coding,
                a.zpages_raw_tails,
                a.zpages_store_all,
                a.scan_cache_bytes,
                a.forms_to_fit,
                a.zpages_block_cache
            ),
            (
                b.zpages,
                false,
                false,
                false,
                b.scan_cache_bytes,
                b.forms_to_fit,
                b.zpages_block_cache
            ),
            "the rest as the combined arm's, the options' codings off"
        );
        let d = root.join("dispatch");
        std::fs::create_dir_all(&d).unwrap();
        let opened = open("supdb-cache64tofitautozpagescodingauto", &d, 1).unwrap();
        assert_eq!(opened.name(), "supdb-cache64tofitautozpagescodingauto");
        drop(opened);
        drop(arm);
        drop(base);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-cache64tofitautozpagesraw` is `supdb-cache64tofitautozpages`
    /// with the block cache's reads of a partition sent through the door
    /// (`Options::zpages_block_cache` off) and nothing else: named, durable
    /// like it, opened under its name by the dispatch, the cache and the
    /// controller as the base has them, and the engine's default on.
    #[test]
    fn the_raw_cache_arm_differs_from_the_combined_arm_in_the_cache_reads_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-cacherawarm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, make: fn(&Path) -> Res<Supdb>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            make(&d).unwrap()
        };
        assert!(supdb::Options::default().zpages_block_cache);
        let base = new("base", Supdb::create_cache64_tofit_auto_zpages);
        let arm = new("raw", Supdb::create_cache64_tofit_auto_zpages_raw);
        assert_eq!(base.name(), "supdb-cache64tofitautozpages");
        assert_eq!(arm.name(), "supdb-cache64tofitautozpagesraw");
        assert_eq!(guarantee(arm.name()), Some(Guarantee::Durable));
        assert_eq!(guarantee(arm.name()), guarantee(base.name()));
        assert!(arm.zcacheraw && !base.zcacheraw);
        assert!(arm.zpages_auto && base.zpages_auto && arm.tofit && base.tofit);
        assert_eq!(
            (arm.budget, arm.block_cache),
            (base.budget, base.block_cache)
        );
        assert_eq!(arm.features(), base.features());
        let (a, b) = (
            arm.db.as_ref().unwrap().options(),
            base.db.as_ref().unwrap().options(),
        );
        assert!(
            !a.zpages_block_cache && b.zpages_block_cache,
            "the one option"
        );
        assert_eq!(
            (
                a.zpages,
                a.scan_block_cache,
                a.scan_cache_bytes,
                a.forms_to_fit
            ),
            (
                b.zpages,
                b.scan_block_cache,
                b.scan_cache_bytes,
                b.forms_to_fit
            )
        );
        assert_eq!((a.promote_entries, a.zpages_promote_copies), (0, false));
        let d = root.join("dispatch");
        std::fs::create_dir_all(&d).unwrap();
        let opened = open("supdb-cache64tofitautozpagesraw", &d, 1).unwrap();
        assert_eq!(opened.name(), "supdb-cache64tofitautozpagesraw");
        drop(opened);
        // The promotion arm: the combined arm with the copies in the slots
        // and nothing else moved.
        let promote = new("promote", Supdb::create_cache64_tofit_auto_zpages_promote);
        assert_eq!(promote.name(), "supdb-cache64tofitautozpagespromote");
        assert_eq!(guarantee(promote.name()), guarantee(base.name()));
        assert!(promote.zpromote == 64 && base.zpromote == 0 && !promote.zcacheraw);
        assert_eq!(promote.features(), base.features());
        let p = promote.db.as_ref().unwrap().options();
        assert_eq!((p.promote_entries, p.zpages_promote_copies), (64, true));
        assert_eq!(
            (
                p.zpages,
                p.scan_block_cache,
                p.scan_cache_bytes,
                p.forms_to_fit,
                p.zpages_block_cache
            ),
            (
                b.zpages,
                b.scan_block_cache,
                b.scan_cache_bytes,
                b.forms_to_fit,
                b.zpages_block_cache
            )
        );
        let d = root.join("dispatch-promote");
        std::fs::create_dir_all(&d).unwrap();
        let opened = open("supdb-cache64tofitautozpagespromote", &d, 1).unwrap();
        assert_eq!(opened.name(), "supdb-cache64tofitautozpagespromote");
        drop(opened);
        // The group arms: the combined arm with the form's page cut at
        // another target, and nothing else moved; the default target is
        // the engine's, not restated.
        assert_eq!(
            b.zpages_group_bytes,
            supdb::Options::default().zpages_group_bytes
        );
        assert_eq!(b.zpages_group_bytes, supdb::db::zpages::ZGROUP_BYTES);
        for (name, make, target) in [
            (
                "supdb-cache64tofitautozpagesonegroup",
                Supdb::create_cache64_tofit_auto_zpages_onegroup as fn(&Path) -> Res<Supdb>,
                0usize,
            ),
            (
                "supdb-cache64tofitautozpagesgroup1k",
                Supdb::create_cache64_tofit_auto_zpages_group1k,
                1024,
            ),
        ] {
            let arm = new(name, make);
            assert_eq!(arm.name(), name);
            assert_eq!(guarantee(arm.name()), guarantee(base.name()));
            assert_eq!(arm.zgroup, Some(target));
            assert!(base.zgroup.is_none() && !arm.zcacheraw && arm.zpromote == 0);
            assert_eq!(arm.features(), base.features());
            let g = arm.db.as_ref().unwrap().options();
            assert_eq!(g.zpages_group_bytes, target, "{name}: the one option");
            assert_eq!(
                (
                    g.zpages,
                    g.scan_block_cache,
                    g.scan_cache_bytes,
                    g.forms_to_fit,
                    g.zpages_block_cache,
                    g.promote_entries
                ),
                (
                    b.zpages,
                    b.scan_block_cache,
                    b.scan_cache_bytes,
                    b.forms_to_fit,
                    b.zpages_block_cache,
                    b.promote_entries
                )
            );
            let d = root.join(format!("dispatch-{name}"));
            std::fs::create_dir_all(&d).unwrap();
            let opened = open(name, &d, 1).unwrap();
            assert_eq!(opened.name(), name);
            drop(opened);
            drop(arm);
        }
        // The prebuild arm: the combined arm with the builds ahead on and
        // nothing else moved; off is the engine's default.
        assert!(!supdb::Options::default().zpages_prebuild && !b.zpages_prebuild);
        assert!(!supdb::Options::default().zpages_seek_span && !b.zpages_seek_span);
        let span = new(
            "supdb-cache64tofitautozpagesspan",
            Supdb::create_cache64_tofit_auto_zpages_span,
        );
        assert_eq!(span.name(), "supdb-cache64tofitautozpagesspan");
        assert_eq!(guarantee(span.name()), guarantee(base.name()));
        assert_eq!(span.features(), base.features());
        let g = span.db.as_ref().unwrap().options();
        assert!(g.zpages_seek_span, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.scan_cache_bytes,
                g.forms_to_fit,
                g.zpages_prebuild
            ),
            (
                b.zpages,
                b.scan_cache_bytes,
                b.forms_to_fit,
                b.zpages_prebuild
            )
        );
        drop(span);
        assert!(supdb::Options::default().zpages_memo_hash && b.zpages_memo_hash);
        let linear = new(
            "supdb-cache64tofitautozpagesmemolinear",
            Supdb::create_cache64_tofit_auto_zpages_memolinear,
        );
        assert_eq!(linear.name(), "supdb-cache64tofitautozpagesmemolinear");
        assert_eq!(guarantee(linear.name()), guarantee(base.name()));
        assert_eq!(linear.features(), base.features());
        let g = linear.db.as_ref().unwrap().options();
        assert!(!g.zpages_memo_hash, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_memo,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_memo,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(linear);
        assert!(supdb::Options::default().zpages_settle_span && b.zpages_settle_span);
        let exact = new(
            "supdb-cache64tofitautozpagesexactcut",
            Supdb::create_cache64_tofit_auto_zpages_exactcut,
        );
        assert_eq!(exact.name(), "supdb-cache64tofitautozpagesexactcut");
        assert_eq!(guarantee(exact.name()), guarantee(base.name()));
        assert_eq!(exact.features(), base.features());
        let g = exact.db.as_ref().unwrap().options();
        assert!(!g.zpages_settle_span, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_memo_hash,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_memo_hash,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(exact);
        assert!(supdb::Options::default().zpages_trusted_views && b.zpages_trusted_views);
        let checked = new(
            "supdb-cache64tofitautozpagescheckedviews",
            Supdb::create_cache64_tofit_auto_zpages_checkedviews,
        );
        assert_eq!(checked.name(), "supdb-cache64tofitautozpagescheckedviews");
        assert_eq!(guarantee(checked.name()), guarantee(base.name()));
        assert_eq!(checked.features(), base.features());
        let g = checked.db.as_ref().unwrap().options();
        assert!(!g.zpages_trusted_views, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_memo_hash,
                g.zpages_settle_span,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_memo_hash,
                b.zpages_settle_span,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(checked);
        assert!(!supdb::Options::default().zpages_memo_first && !b.zpages_memo_first);
        let memofirst = new(
            "supdb-cache64tofitautozpagesmemofirst",
            Supdb::create_cache64_tofit_auto_zpages_memofirst,
        );
        assert_eq!(memofirst.name(), "supdb-cache64tofitautozpagesmemofirst");
        assert_eq!(guarantee(memofirst.name()), guarantee(base.name()));
        assert_eq!(memofirst.features(), base.features());
        let g = memofirst.db.as_ref().unwrap().options();
        assert!(g.zpages_memo_first, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_trusted_views,
                g.zpages_settle_span,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_trusted_views,
                b.zpages_settle_span,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(memofirst);
        assert!(!supdb::Options::default().zpages_fence_tree && !b.zpages_fence_tree);
        let fencetree = new(
            "supdb-cache64tofitautozpagesfencetree",
            Supdb::create_cache64_tofit_auto_zpages_fencetree,
        );
        assert_eq!(fencetree.name(), "supdb-cache64tofitautozpagesfencetree");
        assert_eq!(guarantee(fencetree.name()), guarantee(base.name()));
        assert_eq!(fencetree.features(), base.features());
        let g = fencetree.db.as_ref().unwrap().options();
        assert!(g.zpages_fence_tree, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_memo_ahead,
                g.zpages_trusted_views,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_memo_ahead,
                b.zpages_trusted_views,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(fencetree);
        assert!(supdb::Options::default().zpages_memo_ahead && b.zpages_memo_ahead);
        let noahead = new(
            "supdb-cache64tofitautozpagesmemonoahead",
            Supdb::create_cache64_tofit_auto_zpages_memonoahead,
        );
        assert_eq!(noahead.name(), "supdb-cache64tofitautozpagesmemonoahead");
        assert_eq!(guarantee(noahead.name()), guarantee(base.name()));
        assert_eq!(noahead.features(), base.features());
        let g = noahead.db.as_ref().unwrap().options();
        assert!(!g.zpages_memo_ahead, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_fence_tree,
                g.zpages_trusted_views,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_fence_tree,
                b.zpages_trusted_views,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(noahead);
        assert!(supdb::Options::default().scan_start_cache == 0 && b.scan_start_cache == 0);
        let startc = new(
            "supdb-cache64tofitautozpagesstartcache",
            Supdb::create_cache64_tofit_auto_zpages_startcache,
        );
        assert_eq!(startc.name(), "supdb-cache64tofitautozpagesstartcache");
        assert_eq!(guarantee(startc.name()), guarantee(base.name()));
        assert_eq!(startc.features(), base.features());
        let g = startc.db.as_ref().unwrap().options();
        assert_eq!(g.scan_start_cache, 4096, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_memo_ahead,
                g.zpages_trusted_views,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_memo_ahead,
                b.zpages_trusted_views,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(startc);
        assert_eq!(b.scan_cache_bytes, 64 << 20);
        for (name, make, budget) in [
            (
                "supdb-cache128tofitautozpages",
                Supdb::create_cache128_tofit_auto_zpages as fn(&Path) -> Res<Supdb>,
                128usize << 20,
            ),
            (
                "supdb-cache256tofitautozpages",
                Supdb::create_cache256_tofit_auto_zpages as fn(&Path) -> Res<Supdb>,
                256usize << 20,
            ),
        ] {
            let arm = new(name, make);
            assert_eq!(arm.name(), name);
            assert_eq!(guarantee(arm.name()), guarantee(base.name()));
            assert_eq!(arm.features(), base.features());
            let g = arm.db.as_ref().unwrap().options();
            assert_eq!(
                g.scan_cache_bytes, budget,
                "{name}: the one option, the budget"
            );
            assert_eq!(
                (
                    g.zpages,
                    g.forms_to_fit,
                    g.zpages_memo_ahead,
                    g.zpages_block_cursor,
                    g.scan_start_cache
                ),
                (
                    b.zpages,
                    b.forms_to_fit,
                    b.zpages_memo_ahead,
                    b.zpages_block_cursor,
                    b.scan_start_cache
                ),
                "{name}: the rest as the default's"
            );
            drop(arm);
        }
        for (name, make, stored, raw, lean, trimpass, decodewalk, resident) in [
            (
                "supdb-cache64tofitzpages",
                Supdb::create_cache64_tofit_zpages as fn(&Path) -> Res<Supdb>,
                false,
                false,
                false,
                false,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesstored",
                Supdb::create_cache64_tofit_zpages_stored as fn(&Path) -> Res<Supdb>,
                true,
                false,
                false,
                false,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesrecordraw",
                Supdb::create_cache64_tofit_zpages_record_raw as fn(&Path) -> Res<Supdb>,
                false,
                true,
                false,
                false,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesrecordrawlean",
                Supdb::create_cache64_tofit_zpages_record_raw_lean as fn(&Path) -> Res<Supdb>,
                false,
                true,
                true,
                false,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesrecordrawleantrim",
                Supdb::create_cache64_tofit_zpages_record_raw_lean_trim as fn(&Path) -> Res<Supdb>,
                false,
                true,
                true,
                true,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesrecordrawleanresident",
                Supdb::create_cache64_tofit_zpages_record_raw_lean_resident
                    as fn(&Path) -> Res<Supdb>,
                false,
                true,
                true,
                true,
                false,
                true,
            ),
            (
                "supdb-cache64tofitzpagesrecordrawdecode",
                Supdb::create_cache64_tofit_zpages_record_raw_decode as fn(&Path) -> Res<Supdb>,
                false,
                true,
                false,
                false,
                true,
                false,
            ),
        ] {
            let arm = new(name, make);
            assert_eq!(arm.name(), name);
            assert_eq!(guarantee(arm.name()), guarantee(base.name()));
            assert_eq!(arm.features(), base.features());
            let g = arm.db.as_ref().unwrap().options();
            assert_eq!(g.zpages, supdb::ZPages::Always, "{name}: the form forced");
            assert_eq!(g.zpages_store_all, stored, "{name}: the coding");
            assert_eq!(g.zpages_raw_tails, raw, "{name}: the tails");
            assert_eq!(g.zpages_record_coding, raw, "{name}: the record coding");
            assert_eq!(g.zpages_memo, !lean, "{name}: the memo");
            assert_eq!(g.zpages_trim_install, lean, "{name}: the trim");
            assert_eq!(
                g.upkeep_trim_look, trimpass,
                "{name}: the look at the pass end"
            );
            assert!(!supdb::Options::default().upkeep_trim_look);
            assert_eq!(g.trim_by_resident, resident, "{name}: the trim's count");
            assert!(!supdb::Options::default().trim_by_resident);
            assert_eq!(
                g.forms_trim_bytes,
                if resident {
                    8 << 20
                } else {
                    b.forms_trim_bytes
                },
                "{name}: the trim's stride"
            );
            assert_eq!(
                g.zpages_inplace_walk, !decodewalk,
                "{name}: the in-place walk"
            );
            assert!(supdb::Options::default().zpages_inplace_walk);
            assert!(
                supdb::Options::default().zpages_memo
                    && !supdb::Options::default().zpages_trim_install
            );
            assert_eq!(
                (
                    g.scan_cache_bytes,
                    g.forms_to_fit,
                    g.scan_block_cache,
                    g.zpages_memo_ahead
                ),
                (
                    b.scan_cache_bytes,
                    b.forms_to_fit,
                    b.scan_block_cache,
                    b.zpages_memo_ahead
                ),
                "{name}: the rest as the combined arm's"
            );
            drop(arm);
        }
        assert!(supdb::Options::default().zpages_block_cursor && b.zpages_block_cursor);
        // The first four partitions raw-tailed, the rest block-coded: the
        // forced block form's arm but for that one option.
        for (name, make, first, drop_raw, recode, keep) in [
            (
                "supdb-cache64tofitzpageshot4",
                Supdb::create_cache64_tofit_zpages_hot4 as fn(&Path) -> Res<Supdb>,
                4,
                false,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpageshot1",
                Supdb::create_cache64_tofit_zpages_hot1 as fn(&Path) -> Res<Supdb>,
                1,
                false,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesdrop",
                Supdb::create_cache64_tofit_zpages_drop as fn(&Path) -> Res<Supdb>,
                0,
                true,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpageshot4drop",
                Supdb::create_cache64_tofit_zpages_hot4_drop as fn(&Path) -> Res<Supdb>,
                4,
                true,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpageshot8drop",
                Supdb::create_cache64_tofit_zpages_hot8_drop as fn(&Path) -> Res<Supdb>,
                8,
                true,
                false,
                false,
            ),
            (
                "supdb-cache64tofitzpagesrecodedrop",
                Supdb::create_cache64_tofit_zpages_recode_drop as fn(&Path) -> Res<Supdb>,
                0,
                true,
                true,
                false,
            ),
            (
                "supdb-cache64tofitzpagesrecodekeepdrop",
                Supdb::create_cache64_tofit_zpages_recode_keep_drop as fn(&Path) -> Res<Supdb>,
                0,
                true,
                true,
                true,
            ),
        ] {
            let hot = new(name, make);
            assert_eq!(hot.name(), name);
            assert_eq!(guarantee(hot.name()), guarantee(base.name()));
            assert_eq!(hot.features(), base.features());
            let g = hot.db.as_ref().unwrap().options();
            assert_eq!(g.zpages, supdb::ZPages::Always);
            assert_eq!(
                g.zpages_raw_tails_first, first,
                "{name}: the partitions raw-tailed"
            );
            assert_eq!(g.zpages_drop_raw, drop_raw, "{name}: the raw pages dropped");
            assert_eq!(g.zpages_recode, recode, "{name}: the re-code policy");
            assert_eq!(
                g.zpages_recode_keep, keep,
                "{name}: the replaced form file kept"
            );
            assert!(!g.forms_settle_every, "{name}: the default settle");
            assert!(!supdb::Options::default().zpages_recode_keep);
            assert!(!supdb::Options::default().zpages_recode);
            assert!(!supdb::Options::default().zpages_drop_raw);
            assert_eq!(supdb::Options::default().zpages_raw_tails_first, 0);
            assert!(!g.zpages_raw_tails && !g.zpages_record_coding && g.zpages_memo);
            assert_eq!(
                (
                    g.scan_cache_bytes,
                    g.forms_to_fit,
                    g.scan_block_cache,
                    g.zpages_memo_ahead,
                    g.zpages_inplace_walk
                ),
                (
                    b.scan_cache_bytes,
                    b.forms_to_fit,
                    b.scan_block_cache,
                    b.zpages_memo_ahead,
                    b.zpages_inplace_walk
                ),
                "{name}: the rest as the combined arm's"
            );
            drop(hot);
        }
        // Hot8 with the drop and every commit near reads settling: hot8's
        // options but for that one.
        let every = new(
            "supdb-cache64tofitzpageshot8dropsettleevery",
            Supdb::create_cache64_tofit_zpages_hot8_drop_settle_every,
        );
        assert_eq!(every.name(), "supdb-cache64tofitzpageshot8dropsettleevery");
        assert_eq!(guarantee(every.name()), guarantee(base.name()));
        assert_eq!(every.features(), base.features());
        {
            let e = every.db.as_ref().unwrap().options();
            assert!(e.forms_settle_every && !supdb::Options::default().forms_settle_every);
            assert_eq!(
                (e.zpages_raw_tails_first, e.zpages_drop_raw, e.zpages_recode),
                (8, true, false)
            );
            assert_eq!(e.forms_settle_backlog_pct, b.forms_settle_backlog_pct);
            assert!(!e.forms_settle_builds && !supdb::Options::default().forms_settle_builds);
        }
        drop(every);
        let builds = new(
            "supdb-cache64tofitzpageshot8dropsettlebuilds",
            Supdb::create_cache64_tofit_zpages_hot8_drop_settle_builds,
        );
        assert_eq!(
            builds.name(),
            "supdb-cache64tofitzpageshot8dropsettlebuilds"
        );
        assert_eq!(guarantee(builds.name()), guarantee(base.name()));
        {
            let e = builds.db.as_ref().unwrap().options();
            assert!(e.forms_settle_every && e.forms_settle_builds);
            assert_eq!((e.zpages_raw_tails_first, e.zpages_drop_raw), (8, true));
        }
        drop(builds);
        // The whole raw-tail form with the raw pages dropped: the raw-tail
        // arm's options but for that one.
        let raw = new(
            "supdb-cache64tofitzpagesrecordraw",
            Supdb::create_cache64_tofit_zpages_record_raw,
        );
        let rawdrop = new(
            "supdb-cache64tofitzpagesrecordrawdrop",
            Supdb::create_cache64_tofit_zpages_record_raw_drop,
        );
        assert_eq!(rawdrop.name(), "supdb-cache64tofitzpagesrecordrawdrop");
        assert_eq!(guarantee(rawdrop.name()), guarantee(base.name()));
        assert_eq!(rawdrop.features(), base.features());
        let (r, d) = (
            raw.db.as_ref().unwrap().options(),
            rawdrop.db.as_ref().unwrap().options(),
        );
        assert!(d.zpages_drop_raw && !r.zpages_drop_raw, "the one option");
        assert_eq!(
            (
                d.zpages,
                d.zpages_raw_tails,
                d.zpages_record_coding,
                d.zpages_memo,
                d.scan_cache_bytes,
                d.forms_to_fit,
                d.zpages_inplace_walk
            ),
            (
                r.zpages,
                r.zpages_raw_tails,
                r.zpages_record_coding,
                r.zpages_memo,
                r.scan_cache_bytes,
                r.forms_to_fit,
                r.zpages_inplace_walk
            ),
            "the rest as the raw-tail arm's"
        );
        drop((raw, rawdrop));
        let perrun = new(
            "supdb-cache64tofitautozpagesperrun",
            Supdb::create_cache64_tofit_auto_zpages_perrun,
        );
        assert_eq!(perrun.name(), "supdb-cache64tofitautozpagesperrun");
        assert_eq!(guarantee(perrun.name()), guarantee(base.name()));
        assert_eq!(perrun.features(), base.features());
        let g = perrun.db.as_ref().unwrap().options();
        assert!(!g.zpages_block_cursor, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_trusted_views,
                g.zpages_memo_first,
                g.zpages_settle_span,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_trusted_views,
                b.zpages_memo_first,
                b.zpages_settle_span,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(perrun);
        assert_eq!(supdb::Options::default().zpages_memo_bytes, 8 << 20);
        assert_eq!(b.zpages_memo_bytes, 8 << 20);
        for (mib, name, make) in [
            (
                4usize,
                "supdb-cache64tofitautozpagesmemo4",
                Supdb::create_cache64_tofit_auto_zpages_memo4 as fn(&Path) -> Res<Supdb>,
            ),
            (
                16,
                "supdb-cache64tofitautozpagesmemo16",
                Supdb::create_cache64_tofit_auto_zpages_memo16,
            ),
            (
                32,
                "supdb-cache64tofitautozpagesmemo32",
                Supdb::create_cache64_tofit_auto_zpages_memo32,
            ),
        ] {
            let memo = new(name, make);
            assert_eq!(memo.name(), name);
            assert_eq!(guarantee(memo.name()), guarantee(base.name()));
            assert_eq!(memo.features(), base.features());
            let g = memo.db.as_ref().unwrap().options();
            assert_eq!(g.zpages_memo_bytes, mib << 20, "the one option");
            assert_eq!(
                (
                    g.zpages,
                    g.zpages_memo,
                    g.zpages_trusted_views,
                    g.zpages_block_cursor,
                    g.zpages_seek_span,
                    g.scan_cache_bytes
                ),
                (
                    b.zpages,
                    b.zpages_memo,
                    b.zpages_trusted_views,
                    b.zpages_block_cursor,
                    b.zpages_seek_span,
                    b.scan_cache_bytes
                )
            );
            drop(memo);
        }
        let pre = new(
            "supdb-cache64tofitautozpagesprebuild",
            Supdb::create_cache64_tofit_auto_zpages_prebuild,
        );
        assert_eq!(pre.name(), "supdb-cache64tofitautozpagesprebuild");
        assert_eq!(guarantee(pre.name()), guarantee(base.name()));
        assert!(pre.zprebuild && !base.zprebuild);
        assert!(pre.zgroup.is_none() && !pre.zcacheraw && pre.zpromote == 0);
        assert_eq!(pre.features(), base.features());
        let g = pre.db.as_ref().unwrap().options();
        assert!(g.zpages_prebuild, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.scan_block_cache,
                g.scan_cache_bytes,
                g.forms_to_fit,
                g.zpages_block_cache,
                g.promote_entries,
                g.zpages_group_bytes
            ),
            (
                b.zpages,
                b.scan_block_cache,
                b.scan_cache_bytes,
                b.forms_to_fit,
                b.zpages_block_cache,
                b.promote_entries,
                b.zpages_group_bytes
            )
        );
        let d = root.join("dispatch-prebuild");
        std::fs::create_dir_all(&d).unwrap();
        let opened = open("supdb-cache64tofitautozpagesprebuild", &d, 1).unwrap();
        assert_eq!(opened.name(), "supdb-cache64tofitautozpagesprebuild");
        drop(opened);
        drop(pre);
        drop(promote);
        drop(arm);
        drop(base);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-nocachezpagesnowarm`, `supdb-nocachezpagessettle`,
    /// `supdb-nocachezpagesnomemo`, `supdb-nocachezpagesheads`,
    /// `supdb-nocachezpagesprefetch` and `supdb-nocachezpagesrankwalk` are
    /// each `supdb-nocachezpages` with one option moved -- the warm of a
    /// form file after its attach off, every build left to the settle,
    /// the memo of decoded groups off, the seeks by the ordered index's
    /// heads, a scan's prefetch of its pages on, the overlay walk cut by
    /// rank -- and nothing else: named, durable like it, opened
    /// under their names by the dispatch, on options that differ from the
    /// arm's in that option alone, and forced like it.
    #[test]
    fn each_single_option_arm_differs_from_the_zpages_arm_in_that_option_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-zpagesarms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, make: fn(&Path) -> Res<Supdb>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            make(&d).unwrap()
        };
        let base = new("base", Supdb::create_nocache_zpages);
        assert_eq!(base.name(), "supdb-nocachezpages");
        let b = base.db.as_ref().unwrap().options();
        // The engine's own defaults: the warm on, the build at the landing,
        // so an arm that pins neither has both as the engine ships them.
        assert!(supdb::Options::default().zpages_warm);
        assert!(supdb::Options::default().zpages_eager);
        assert!(supdb::Options::default().zpages_memo);
        assert!(!supdb::Options::default().zpages_seek_ord);
        assert!(!supdb::Options::default().zpages_scan_prefetch);
        assert!(supdb::Options::default().zpages_walk_cursor);
        assert!(supdb::Options::default().zpages_trusted_views && b.zpages_trusted_views);
        assert!(b.zpages_warm && b.zpages_eager && b.zpages_memo && b.zpages_walk_cursor);
        assert!(!b.zpages_seek_ord && !b.zpages_scan_prefetch);
        for (name, make, arm_dir) in [
            (
                "supdb-nocachezpagesnowarm",
                Supdb::create_nocache_zpages_nowarm as fn(&Path) -> Res<Supdb>,
                "nowarm",
            ),
            (
                "supdb-nocachezpagessettle",
                Supdb::create_nocache_zpages_settle,
                "settle",
            ),
            (
                "supdb-nocachezpagesnomemo",
                Supdb::create_nocache_zpages_nomemo,
                "nomemo",
            ),
            (
                "supdb-nocachezpagesheads",
                Supdb::create_nocache_zpages_heads,
                "heads",
            ),
            (
                "supdb-nocachezpagesprefetch",
                Supdb::create_nocache_zpages_prefetch,
                "prefetch",
            ),
            (
                "supdb-nocachezpagesrankwalk",
                Supdb::create_nocache_zpages_rankwalk,
                "rankwalk",
            ),
            (
                "supdb-nocachezpagesonegroup",
                Supdb::create_nocache_zpages_onegroup,
                "onegroup",
            ),
            (
                "supdb-nocachezpagescheckedviews",
                Supdb::create_nocache_zpages_checkedviews,
                "checkedviews",
            ),
            (
                "supdb-nocachezpagesstored",
                Supdb::create_nocache_zpages_stored,
                "stored",
            ),
            (
                "supdb-nocachezpagesrecordraw",
                Supdb::create_nocache_zpages_record_raw,
                "recordraw",
            ),
        ] {
            let arm = new(arm_dir, make);
            assert_eq!(arm.name(), name);
            assert_eq!(guarantee(name), Some(Guarantee::Durable));
            assert_eq!(guarantee(name), guarantee("supdb-nocachezpages"));
            assert!(arm.zpages && base.zpages, "both force the form");
            assert!(!arm.zpages_nodrop);
            assert_eq!(arm.zgroup, (arm_dir == "onegroup").then_some(0));
            assert!(base.zgroup.is_none());
            assert_eq!(arm.zpages_nowarm, arm_dir == "nowarm");
            assert_eq!(arm.zpages_settle, arm_dir == "settle");
            assert_eq!(arm.zpages_nomemo, arm_dir == "nomemo");
            assert_eq!(arm.zpages_heads, arm_dir == "heads");
            assert_eq!(arm.zpages_prefetch, arm_dir == "prefetch");
            assert_eq!(arm.zpages_rankwalk, arm_dir == "rankwalk");
            assert_eq!(arm.zcheckedviews, arm_dir == "checkedviews");
            assert_eq!(arm.zrecord, arm_dir == "record");
            assert_eq!(arm.zstored, arm_dir == "stored");
            assert_eq!(arm.zrawtails, arm_dir == "recordraw");
            assert_eq!(
                arm.db.as_ref().unwrap().options().zpages_raw_tails,
                arm_dir == "recordraw",
                "{name}: the raw tails"
            );
            assert_eq!(
                arm.db.as_ref().unwrap().options().zpages_record_coding,
                arm_dir == "record" || arm_dir == "recordraw",
                "{name}: the record coding"
            );
            assert_eq!(
                arm.db.as_ref().unwrap().options().zpages_store_all,
                arm_dir == "stored",
                "{name}: the stored option"
            );
            assert_eq!(arm.partition, base.partition, "the row's shape");
            assert_eq!(arm.drain, base.drain);
            assert_eq!(arm.durable, base.durable);
            assert_eq!(arm.block_cache, base.block_cache);
            assert_eq!(arm.budget, base.budget);
            assert_eq!(arm.advice, base.advice);
            assert_eq!(arm.features(), base.features());
            let a = arm.db.as_ref().unwrap().options();
            assert_eq!(a.zpages_warm, arm_dir != "nowarm", "{name}: the one option");
            assert_eq!(
                a.zpages_eager,
                arm_dir != "settle",
                "{name}: the one option"
            );
            assert_eq!(a.zpages_memo, arm_dir != "nomemo", "{name}: the one option");
            assert_eq!(
                a.zpages_trusted_views,
                arm_dir != "checkedviews",
                "{name}: the one option"
            );
            assert_eq!(
                a.zpages_seek_ord,
                arm_dir == "heads",
                "{name}: the one option"
            );
            assert_eq!(
                a.zpages_scan_prefetch,
                arm_dir == "prefetch",
                "{name}: the one option"
            );
            assert_eq!(
                a.zpages_walk_cursor,
                arm_dir != "rankwalk",
                "{name}: the one option"
            );
            assert_eq!(
                a.zpages_group_bytes,
                if arm_dir == "onegroup" {
                    0
                } else {
                    b.zpages_group_bytes
                },
                "{name}: the one option"
            );
            assert_eq!(b.zpages_group_bytes, supdb::db::zpages::ZGROUP_BYTES);
            assert!(a.zpages == supdb::ZPages::Always && b.zpages == supdb::ZPages::Always);
            assert_eq!(a.zpages_drop_behind, b.zpages_drop_behind);
            assert_eq!(a.zpages_min_bytes, b.zpages_min_bytes);
            assert_eq!(a.zpages_retry_ms, b.zpages_retry_ms);
            assert_eq!(a.point_via_ord, b.point_via_ord);
            assert_eq!(a.scan_block_cache, b.scan_block_cache);
            assert_eq!(a.scan_cache_bytes, b.scan_cache_bytes);
            assert_eq!(a.read_advice, b.read_advice);
            assert_eq!(a.inline_bytes, b.inline_bytes);
            assert_eq!(a.partition_bytes, b.partition_bytes);
            assert_eq!(a.seal_bytes, b.seal_bytes);
            assert_eq!(a.compact, b.compact);
            assert_eq!(a.promote, b.promote);
            assert_eq!(a.sync, b.sync);
            assert_eq!(a.upkeep, b.upkeep);
            assert_eq!(a.writer_pins, b.writer_pins);
            assert_eq!(a.publish_in_background, b.publish_in_background);
            drop(arm);
            let opened = root.join(format!("open-{arm_dir}"));
            std::fs::create_dir_all(&opened).unwrap();
            let e = open(name, &opened, 1).unwrap();
            assert_eq!(e.name(), name, "the dispatch");
            assert_eq!(e.zpages_state().map(|z| z.forced), Some(true));
        }
        drop(base);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-nocachezpagesnodrop` is `supdb-nocachezpages` with the build's
    /// drop of the raw records behind its walk off, and nothing else: named,
    /// durable like it, opened under its name by the dispatch, on options that
    /// differ from the arm's in `Options::zpages_drop_behind` alone, and
    /// forced like it. The drop is what the pair prices, so the arm is held
    /// to leaving the build's other settings where the arm has them.
    #[test]
    fn the_nodrop_arm_differs_from_the_zpages_arm_in_the_drop_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-zpagesnodrop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let new = |name: &str, nodrop: bool| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            if nodrop {
                Supdb::create_nocache_zpages_nodrop(&d)
            } else {
                Supdb::create_nocache_zpages(&d)
            }
            .unwrap()
        };
        let (arm, base) = (new("arm", true), new("base", false));
        assert_eq!(arm.name(), "supdb-nocachezpagesnodrop");
        assert_eq!(base.name(), "supdb-nocachezpages");
        assert_eq!(
            guarantee("supdb-nocachezpagesnodrop"),
            Some(Guarantee::Durable)
        );
        assert_eq!(
            guarantee("supdb-nocachezpagesnodrop"),
            guarantee("supdb-nocachezpages")
        );
        assert!(arm.zpages && base.zpages, "both force the form");
        assert!(arm.zpages_nodrop && !base.zpages_nodrop);
        assert_eq!(arm.partition, base.partition, "the row's shape");
        assert_eq!(arm.drain, base.drain);
        assert_eq!(arm.durable, base.durable);
        assert_eq!(arm.block_cache, base.block_cache);
        assert_eq!(arm.budget, base.budget);
        assert_eq!(arm.advice, base.advice);
        assert_eq!(arm.features(), base.features());
        let (a, b) = (
            arm.db.as_ref().unwrap().options(),
            base.db.as_ref().unwrap().options(),
        );
        assert!(
            !a.zpages_drop_behind && b.zpages_drop_behind,
            "the one option"
        );
        assert!(a.zpages == supdb::ZPages::Always && b.zpages == supdb::ZPages::Always);
        assert_eq!(a.zpages_min_bytes, b.zpages_min_bytes);
        assert_eq!(a.point_via_ord, b.point_via_ord);
        assert_eq!(a.scan_block_cache, b.scan_block_cache);
        assert_eq!(a.scan_cache_bytes, b.scan_cache_bytes);
        assert_eq!(a.read_advice, b.read_advice);
        assert_eq!(a.inline_bytes, b.inline_bytes);
        assert_eq!(a.partition_bytes, b.partition_bytes);
        assert_eq!(a.seal_bytes, b.seal_bytes);
        assert_eq!(a.compact, b.compact);
        assert_eq!(a.promote, b.promote);
        assert_eq!(a.sync, b.sync);
        assert_eq!(a.upkeep, b.upkeep);
        assert_eq!(a.writer_pins, b.writer_pins);
        assert_eq!(a.publish_in_background, b.publish_in_background);
        // The engine's own default is the drop on, so an arm that pins
        // nothing has it.
        assert!(supdb::Options::default().zpages_drop_behind);
        drop((arm, base));
        let opened = root.join("open");
        std::fs::create_dir_all(&opened).unwrap();
        let e = open("supdb-nocachezpagesnodrop", &opened, 1).unwrap();
        assert_eq!(e.name(), "supdb-nocachezpagesnodrop", "the dispatch");
        assert_eq!(e.zpages_state().map(|z| z.forced), Some(true));
        drop(e);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-settlefar` and `supdb-ingestsettlefar` are the policy with
    /// `settlefar` alone set over their row's default, open under that
    /// name, and the options they open the engine with differ from the
    /// default's in `forms_settle_far` alone among the forms' settings.
    #[test]
    fn the_settlefar_arms_differ_from_their_defaults_in_the_far_filing_alone() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-settlefar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, buffered) in [("supdb-settlefar", false), ("supdb-ingestsettlefar", true)] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            let arm = if buffered {
                Supdb::create_ingest_settlefar(&d)
            } else {
                Supdb::create_settlefar(&d)
            }
            .unwrap();
            assert_eq!(arm.name(), name);
            assert!(arm.settlefar && !arm.dirplain && !arm.lazyforms, "{name}");
            let base_d = root.join(format!("{name}-base"));
            std::fs::create_dir_all(&base_d).unwrap();
            let base = if buffered {
                Supdb::create_ingest(&base_d)
            } else {
                Supdb::create(&base_d)
            }
            .unwrap();
            assert_eq!(arm.partition, base.partition, "{name}: the row's shape");
            assert_eq!(arm.durable, base.durable, "{name}: the row's guarantee");
            assert_eq!(arm.features(), base.features(), "{name}");
            let (a, b) = (
                arm.db.as_ref().unwrap().options(),
                base.db.as_ref().unwrap().options(),
            );
            assert!(
                a.forms_settle_far && !b.forms_settle_far,
                "{name}: the one option"
            );
            assert_eq!(a.commit_forms, b.commit_forms, "{name}");
            assert_eq!(a.scan_block_cache, b.scan_block_cache, "{name}");
            assert_eq!(
                a.forms_from_reader_scans, b.forms_from_reader_scans,
                "{name}"
            );
            assert_eq!(
                a.forms_settle_backlog_pct, b.forms_settle_backlog_pct,
                "{name}"
            );
            assert_eq!(
                a.forms_settle_recent_pct, b.forms_settle_recent_pct,
                "{name}"
            );
            assert_eq!(a.seal_idle, b.seal_idle, "{name}");
            drop(arm);
            drop(base);
            // The dispatch opens the arm by its name.
            let opened_d = root.join(format!("{name}-open"));
            std::fs::create_dir_all(&opened_d).unwrap();
            let e = open(name, &opened_d, 1).unwrap();
            assert_eq!(e.name(), name, "the dispatch");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `supdb-tofit`, `supdb-ingesttofit` and `supdb-cache256tofit` are
    /// `supdb`, `supdb-ingest` and `supdb-cache256` with `forms_to_fit`
    /// alone set: each opens under its name, in its default's row and
    /// under its default's guarantee, with the budget the default has and
    /// every option the forms and the cache read as the default has them.
    #[test]
    fn the_tofit_arms_differ_from_their_defaults_in_the_one_option() {
        type Make = fn(&Path) -> Res<Supdb>;
        let root = std::env::temp_dir().join(format!("supdb-bench-tofit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pairs: [(&str, Make, &str, Make); 3] = [
            ("supdb-tofit", Supdb::create_tofit, "supdb", Supdb::create),
            (
                "supdb-ingesttofit",
                Supdb::create_ingest_tofit,
                "supdb-ingest",
                Supdb::create_ingest,
            ),
            (
                "supdb-cache256tofit",
                Supdb::create_cache256_tofit,
                "supdb-cache256",
                Supdb::create_cache256,
            ),
        ];
        for (name, make, base_name, make_base) in pairs {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            let arm = make(&d).unwrap();
            assert_eq!(arm.name(), name);
            let base_d = root.join(base_name);
            std::fs::create_dir_all(&base_d).unwrap();
            let base = make_base(&base_d).unwrap();
            assert_eq!(base.name(), base_name);
            assert_eq!(arm.partition, base.partition, "{name}: the row's shape");
            assert_eq!(arm.durable, base.durable, "{name}: the row's guarantee");
            assert_eq!(arm.features(), base.features(), "{name}");
            assert_eq!(guarantee(name), guarantee(base_name), "{name}");
            let (a, b) = (
                arm.db.as_ref().unwrap().options(),
                base.db.as_ref().unwrap().options(),
            );
            assert!(a.forms_to_fit && !b.forms_to_fit, "{name}: the one option");
            assert_eq!(a.scan_cache_bytes, b.scan_cache_bytes, "{name}: the budget");
            assert_eq!(
                a.scan_cache_bytes,
                if name.contains("cache256") {
                    256 << 20
                } else {
                    0
                },
                "{name}"
            );
            assert_eq!(a.scan_block_cache, b.scan_block_cache, "{name}");
            assert_eq!(a.commit_forms, b.commit_forms, "{name}");
            assert_eq!(a.copy_exact, b.copy_exact, "{name}");
            assert_eq!(a.forms_publish_lazily, b.forms_publish_lazily, "{name}");
            assert_eq!(a.scan_cache_ahead, b.scan_cache_ahead, "{name}");
            assert_eq!(a.upkeep, b.upkeep, "{name}");
            assert_eq!(a.forms_settle_far, b.forms_settle_far, "{name}");
            // The three parts of the flag are the default's, which is on:
            // the arm is the whole of it.
            assert_eq!(a.forms_fit_shed, b.forms_fit_shed, "{name}");
            assert_eq!(a.forms_fit_sweep, b.forms_fit_sweep, "{name}");
            assert_eq!(a.forms_trim_bytes, b.forms_trim_bytes, "{name}");
            drop(arm);
            drop(base);
            // The dispatch opens the arm by its name.
            let opened_d = root.join(format!("{name}-open"));
            std::fs::create_dir_all(&opened_d).unwrap();
            let e = open(name, &opened_d, 1).unwrap();
            assert_eq!(e.name(), name, "the dispatch");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The three arms that take a part of `supdb-cache256tofit` away
    /// (`supdb-cache256tofitnotrim`, `...noshed`, `...nosweep`) are
    /// `supdb-cache256tofit` with that part's option alone changed: each
    /// opens under its name, in its default's row and under its default's
    /// guarantee, with the one option off and the other two as the
    /// default has them, so a pairing prices the part and nothing else.
    #[test]
    fn the_removal_arms_differ_from_their_default_in_the_one_option() {
        type Make = fn(&Path) -> Res<Supdb>;
        let root = std::env::temp_dir().join(format!("supdb-bench-removal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base_d = root.join("tofit");
        std::fs::create_dir_all(&base_d).unwrap();
        let base = Supdb::create_cache256_tofit(&base_d).unwrap();
        assert_eq!(base.name(), "supdb-cache256tofit");
        let b = base.db.as_ref().unwrap().options().clone();
        let default = supdb::Options::default();
        assert!(b.forms_to_fit, "the default arm keeps its forms to fit");
        assert_eq!(b.forms_trim_bytes, default.forms_trim_bytes);
        assert!(b.forms_trim_bytes > 0 && b.forms_fit_shed && b.forms_fit_sweep);
        let arms: [(&str, Make); 3] = [
            (
                "supdb-cache256tofitnotrim",
                Supdb::create_cache256_tofit_notrim,
            ),
            (
                "supdb-cache256tofitnoshed",
                Supdb::create_cache256_tofit_noshed,
            ),
            (
                "supdb-cache256tofitnosweep",
                Supdb::create_cache256_tofit_nosweep,
            ),
        ];
        for (name, make) in arms {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            let arm = make(&d).unwrap();
            assert_eq!(arm.name(), name);
            assert_eq!(arm.partition, base.partition, "{name}: the row's shape");
            assert_eq!(arm.durable, base.durable, "{name}: the row's guarantee");
            assert_eq!(arm.features(), base.features(), "{name}");
            assert_eq!(guarantee(name), guarantee(base.name()), "{name}");
            let a = arm.db.as_ref().unwrap().options();
            // Exactly the one part is off, and it is the one the name says.
            let off = [
                ("notrim", a.forms_trim_bytes == 0),
                ("noshed", !a.forms_fit_shed),
                ("nosweep", !a.forms_fit_sweep),
            ];
            let named: Vec<&str> = off.iter().filter(|(_, o)| *o).map(|(n, _)| *n).collect();
            assert_eq!(
                named.len(),
                1,
                "{name}: one part off and no other: {named:?}"
            );
            assert!(
                name.ends_with(named[0]),
                "{name}: the part off is {named:?}"
            );
            if a.forms_trim_bytes != 0 {
                assert_eq!(a.forms_trim_bytes, b.forms_trim_bytes, "{name}");
            }
            assert!(a.forms_to_fit && b.forms_to_fit, "{name}: kept to fit");
            assert_eq!(a.scan_cache_bytes, b.scan_cache_bytes, "{name}: the budget");
            assert_eq!(a.scan_block_cache, b.scan_block_cache, "{name}");
            assert_eq!(a.commit_forms, b.commit_forms, "{name}");
            assert_eq!(a.copy_exact, b.copy_exact, "{name}");
            assert_eq!(a.scan_cache_ahead, b.scan_cache_ahead, "{name}");
            assert_eq!(a.upkeep, b.upkeep, "{name}");
            assert_eq!(a.forms_settle_far, b.forms_settle_far, "{name}");
            drop(arm);
            // The dispatch opens the arm by its name.
            let opened_d = root.join(format!("{name}-open"));
            std::fs::create_dir_all(&opened_d).unwrap();
            let e = open(name, &opened_d, 1).unwrap();
            assert_eq!(e.name(), name, "the dispatch");
        }
        drop(base);
        let _ = std::fs::remove_dir_all(&root);
    }
}
