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
    use std::os::unix::fs::MetadataExt;
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            total += if m.is_dir() {
                dir_size(&e.path())
            } else {
                m.blocks() * 512
            };
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
    /// sealed and leaves that to background compaction. Both arms exist so
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
    /// Whether the forms are carried across a seal. `supdb-carry`.
    carry: bool,
    /// Aligned pieces over a range at which they are merged into one
    /// piece, or none for the engine's own, which leaves them for the
    /// partition merge. `supdb-tier`.
    tier: Option<usize>,
    /// Whether a publish starts the builder and a commit installs its
    /// forms. `supdb-aheadpub`.
    aheadpub: bool,
    /// Whether the forms are published at every maintained commit, as
    /// they were before they waited for a handle. `supdb-pubalways`.
    pubalways: bool,
    /// The partitions' blocks below which no builder starts, or none for
    /// the engine's own. `supdb-ahead`.
    aheadmin: Option<usize>,
    /// The seal cap this arm pins, or none for the engine's own;
    /// `supdb-noseal` pins zero. `Option` and not a number, because the
    /// shipping arm must inherit a default rather than restate it.
    sealcap: Option<usize>,
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
    /// Whether the forms are carried across a seal. `supdb-carry`.
    carry: bool,
    /// Aligned pieces over a range at which they are merged into one
    /// piece, or none for the engine's own, which leaves them for the
    /// partition merge. `supdb-tier`.
    tier: Option<usize>,
    /// Whether a publish starts the builder and a commit installs its
    /// forms. `supdb-aheadpub`.
    aheadpub: bool,
    /// Whether the forms are published at every maintained commit, as
    /// they were before they waited for a handle. `supdb-pubalways`.
    pubalways: bool,
    /// The builder's minimum blocks, or none for the engine's own.
    /// `supdb-ahead`.
    aheadmin: Option<usize>,
    /// The seal cap this arm pins, or none for the engine's own.
    /// `supdb-noseal`.
    sealcap: Option<usize>,
}

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
            aheadmin: None,
            lazyforms: false,
            eager: None,
            recent: None,
            carry: false,
            tier: None,
            aheadpub: false,
            pubalways: false,
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
                block_cache: false,
                durable: false,
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

    /// `supdb` with the canonical forms carried across a seal. Against
    /// `supdb` it prices the carry: what filing the backlog at the freeze
    /// costs the mixes, and what a table that survives the seal is worth
    /// to the reads after it.
    /// `supdb` with the piece merge on at three pieces, so a range's
    /// pieces fold into one beside the partition merge: what prices it.
    pub fn create_tier(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                tier: Some(3),
                ..Policy::default()
            },
        )
    }

    pub fn create_carry(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                carry: true,
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
            aheadmin,
            lazyforms,
            eager,
            recent,
            carry,
            tier,
            aheadpub,
            pubalways,
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
            // The pieces over a range merged into one piece beside the
            // partition merge, or left for it as the engine has it.
            tier_pieces: tier.unwrap_or(supdb::Options::default().tier_pieces),
            // The builder started by a publish and installed by a commit,
            // or started by a scan as it is.
            build_ahead_on_publish: aheadpub,
            // The forms published for a handle, or at every maintained
            // commit as they were.
            forms_publish_lazily: !pubalways,
            // The unsealed share past which the maintenance stops, so an
            // arm can maintain whoever is reading without paying for it
            // on a store that is nearly all unmerged.
            forms_max_unsealed_pct: lagcap,
            // The seal capped by a share of the store, so the memtable
            // cannot hold the whole of a store smaller than the floor.
            seal_max_pct: sealcap.unwrap_or(base_seal_max_pct),
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
            aheadmin,
            lazyforms,
            eager,
            recent,
            carry,
            tier,
            aheadpub,
            pubalways,
        })
    }
}

/// One read, whichever handle answers it: the writer's own in `Engine`,
/// under its read-your-writes isolation, or a thread's from `Db::reader`.
fn supdb_get(r: &supdb::Reader, key: &[u8]) -> Res<usize> {
    let mut n = 0usize;
    r.read_all(key, |v| n += v.len())
        .map_err(|e| e.to_string())?;
    Ok(n)
}

fn supdb_range(r: &supdb::Reader, from: &[u8], n: usize) -> Res<usize> {
    let mut bytes = 0usize;
    r.scan(from, n, |_k, v| bytes += v.len())
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

impl Engine for Supdb {
    fn name(&self) -> &'static str {
        if self.budget > 0 {
            return "supdb-cache256";
        }
        if self.settle {
            return "supdb-settle";
        }
        if self.carry {
            return "supdb-carry";
        }
        if self.tier.is_some_and(|t| t > 0) {
            return "supdb-tier";
        }
        if self.aheadpub {
            return "supdb-aheadpub";
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
        for &(k, v) in items {
            db.append(k, v);
        }
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
        vec![
            ("snapshot_builds", db.snapshot_builds() as f64),
            ("snapshot_extends", db.snapshot_extends() as f64),
            ("form_takes", db.form_takes() as f64),
            ("blk_by_reader", db.blocks_built().0 as f64),
            ("blk_by_engine", db.blocks_built().1 as f64),
            ("rd_scans", db.reader_scans().0 as f64),
            ("rd_blockpath", db.reader_scans().1 as f64),
            ("canon_tried", db.canonical_tries().0 as f64),
            ("canon_hit", db.canonical_tries().1 as f64),
            ("forms_held_at_end", forms as f64),
            ("form_bytes_at_end", form_bytes as f64),
        ]
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        let db = self.db.as_ref().ok_or("db closed")?;
        // Claimed now, on this thread: the handle's slot in the reader
        // table. `Latest`, the handle's default, sees every commit before
        // a read, and the load's last is behind `sync`.
        let r = db.reader().map_err(|e| e.to_string())?;
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
    // Pinned: the value is borrowed from the block cache, not copied out,
    // which is the cheapest read RocksDB offers and the fair one against
    // engines that hand back a borrow.
    Ok(db
        .get_pinned_opt(key, read)
        .map_err(|e| e.to_string())?
        .map(|v| v.len())
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
        bytes += it.value().map(|v| v.len()).unwrap_or(0);
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
}

impl Lmdb {
    pub fn create(path: &Path, map_gb: usize) -> Res<Lmdb> {
        Lmdb::with_sync(path, map_gb, true)
    }

    /// Matched to Supdb's default: a commit that does not reach the device.
    pub fn create_nosync(path: &Path, map_gb: usize) -> Res<Lmdb> {
        Lmdb::with_sync(path, map_gb, false)
    }

    fn with_sync(path: &Path, map_gb: usize, sync: bool) -> Res<Lmdb> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let env = unsafe {
            let mut o = heed::EnvOpenOptions::new();
            o.map_size(map_gb * 1024 * 1024 * 1024).max_dbs(4);
            if !sync {
                o.flags(heed::EnvFlags::NO_SYNC);
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
        })
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

type LmdbDb = heed::Database<heed::types::Bytes, heed::types::Bytes>;

/// One read under whichever transaction: the arm's held one or a thread's.
fn lmdb_get(db: LmdbDb, r: &heed::RoTxn<'_>, key: &[u8]) -> Res<usize> {
    // Values are borrowed from the mapping, never copied.
    Ok(db
        .get(r, key)
        .map_err(|e| e.to_string())?
        .map(|v| v.len())
        .unwrap_or(0))
}

fn lmdb_range(db: LmdbDb, r: &heed::RoTxn<'_>, from: &[u8], n: usize) -> Res<usize> {
    let mut bytes = 0usize;
    let range = (std::ops::Bound::Included(from), std::ops::Bound::Unbounded);
    for row in db.range(r, &range).map_err(|e| e.to_string())?.take(n) {
        let (_, v) = row.map_err(|e| e.to_string())?;
        bytes += v.len();
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
        if self.nosync {
            "lmdb-nosync"
        } else {
            "lmdb"
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

pub fn guarantee(arm: &str) -> Option<Guarantee> {
    Some(match arm {
        "supdb" | "supdb-forms" | "supdb-settle" | "supdb-wforms" | "supdb-regime"
        | "supdb-noseal" | "supdb-ahead" | "supdb-lazyforms" | "supdb-eager" | "supdb-nosettle"
        | "supdb-recency" | "supdb-carry" | "supdb-tier" | "supdb-aheadpub" | "supdb-pubalways"
        | "supdb-nosnap" | "supdb-noadvice" | "supdb-nocache" | "supdb-cache256" | "lmdb"
        | "rocksdb-tuned" => Guarantee::Durable,
        "supdb-ingest" | "lmdb-nosync" | "rocksdb-nosync" => Guarantee::Buffered,
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
        "supdb-carry" => Box::new(Supdb::create_carry(dir)?),
        "supdb-tier" => Box::new(Supdb::create_tier(dir)?),
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
        "supdb-cache256" => Box::new(Supdb::create_cache256(dir)?),
        "supdb-ingest" => Box::new(Supdb::create_ingest(dir)?),
        "lmdb" => Box::new(Lmdb::create(dir, map_gb)?),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
