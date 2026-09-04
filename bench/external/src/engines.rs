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
//!     `lmdb-nosync` gives up durability the way Supdb's default does. The
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

use lmdb_master_sys as mdb;
use std::path::{Path, PathBuf};
use supdb::bench::J;
use supdb::jobj;

/// What an engine actually guarantees. Reported beside every number.
#[derive(Clone, Copy, Debug)]
pub struct Features {
    pub durable_commit: bool,
    pub transactions: bool,
    pub checksums: bool,
    pub reopen_for_write: bool,
    pub read_your_writes: bool,
    pub ordered_scan: bool,
}

impl Features {
    #[allow(clippy::wrong_self_convention)]
    pub fn to_json(&self) -> J {
        jobj! {
            "durable_commit" => J::Bool(self.durable_commit),
            "transactions" => J::Bool(self.transactions),
            "checksums" => J::Bool(self.checksums),
            "reopen_for_write" => J::Bool(self.reopen_for_write),
            "read_your_writes" => J::Bool(self.read_your_writes),
            "ordered_scan" => J::Bool(self.ordered_scan),
        }
    }
    /// Axes where two engines promise different things and could have been
    /// made to promise the same, restricted to those that bear on this metric.
    ///
    /// A non-empty answer means the pair is not comparable on that metric --
    /// not that the number is noisy, that it is not an ordering. `durable`
    /// says whether the metric touches the write path; a read or a scan does
    /// not care when a commit reaches the device, and does care about
    /// verification.
    pub fn unmatched(&self, other: &Features, durable: bool) -> Vec<&'static str> {
        let mut v = Vec::new();
        if durable && self.durable_commit != other.durable_commit {
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
/// and that therefore sat inside every load ratio in `results/` (f58).
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
    /// and f58 found the two allocations, two copies and two frees that
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
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            total += if m.is_dir() {
                dir_size(&e.path())
            } else {
                m.len()
            };
        }
    } else if let Ok(m) = std::fs::metadata(p) {
        total = m.len();
    }
    total
}

// ------------------------------------------------------------------- next --

/// The next engine (`supdb::next`): a WAL-only commit with sealed segments in
/// today's store format. Always durable -- a commit is a WAL append plus one
/// fdatasync, which is LMDB's own boundary, so this arm is guarantee-matched
/// against `lmdb` the way `supdb-durable` is. Scans pay the unrouted fan
/// (every segment contributes candidates) until range-partitioned compaction
/// lands; that cost is the arm's to show, not to hide.
pub struct Next {
    db: Option<supdb::Db>,
    path: PathBuf,
    /// False for the ingest-first arm: a flush stops partitioning what it
    /// sealed and leaves that to background compaction. Both arms exist so
    /// the trade is measured in ONE interleaved run rather than compared
    /// across two, which is the whole reason this suite interleaves.
    partition: bool,
    /// Whether `sync` drains -- seals the last memtable and partitions what
    /// it sealed inside the load window -- or only makes the WAL durable and
    /// leaves the tail in memory, as RocksDB's `sync` does. f60 found the
    /// drain to be 11% of the canonical window and the whole of the seal
    /// phase, so both shapes are arms (drain-plan.md).
    drain: bool,
}

impl Next {
    pub fn create(path: &Path) -> Res<Next> {
        Next::with_policy(path, true, true)
    }

    pub fn create_ingest(path: &Path) -> Res<Next> {
        Next::with_policy(path, false, true)
    }

    /// `sync` fsyncs and seals nothing; reads then answer from the
    /// memtable, the unrouted tail and the partitions together.
    pub fn create_nodrain(path: &Path) -> Res<Next> {
        Next::with_policy(path, true, false)
    }

    fn with_policy(path: &Path, partition: bool, drain: bool) -> Res<Next> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        // Checksums off in the segments, because LMDB has none and the axis
        // is equalizable -- the same call `supdb-durable` makes, and the
        // fairness gate refused to rank this arm until it was made here too.
        let opts = supdb::Options {
            segment: supdb::SegmentOptions {
                checksums: false,
                ..Default::default()
            },
            // The engine's own defaults: 32 MB seals over 64 MB partitions.
            // Few partitions is not an accident of the benchmark, it is the
            // operating point the design is FOR: f44 measured the same data
            // reading 1.19x of LMDB in one segment and 0.77x spread over
            // eight, and f52 found that seal size and partition size had to
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
            ..Default::default()
        };
        let db = supdb::Db::create(path, opts).map_err(|e| e.to_string())?;
        Ok(Next {
            db: Some(db),
            path: path.to_path_buf(),
            partition,
            drain,
        })
    }
}

impl Engine for Next {
    fn name(&self) -> &'static str {
        match (self.partition, self.drain) {
            (true, true) => "next",
            (false, _) => "next-ingest",
            (true, false) => "next-nodrain",
        }
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: true,
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
        let mut n = 0usize;
        db.read_all(key, |v| n += v.len())
            .map_err(|e| e.to_string())?;
        Ok(n)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let db = self.db.as_ref().ok_or("db closed")?;
        let mut bytes = 0usize;
        db.scan(from, n, |_k, v| bytes += v.len())
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
    fn sync(&mut self) -> Res<()> {
        if let Some(db) = self.db.as_mut() {
            // The default drains: commit AND seal AND partition, so the seal
            // cost lands in the load window and the read phase answers from
            // routed segments. That was chosen so a next arm would not read
            // half its keys out of a resident memtable while LMDB read a
            // tree -- but RocksDB's sync is an fsync of its WAL and its
            // reads go through its memtable and level 0, so against it the
            // drain is a residual the next engine pays alone. `next-nodrain`
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
pub struct Redb {
    db: redb::Database,
    path: PathBuf,
    /// Held across operations for the same reason as LMDB's.
    txn: Option<redb::ReadTransaction>,
}

const T: redb::TableDefinition<&[u8], &[u8]> = redb::TableDefinition::new("kv");

impl Redb {
    pub fn create(path: &Path) -> Res<Redb> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let p = path.join("redb.db");
        let db = redb::Database::create(&p).map_err(|e| e.to_string())?;
        {
            let w = db.begin_write().map_err(|e| e.to_string())?;
            w.open_table(T).map_err(|e| e.to_string())?;
            w.commit().map_err(|e| e.to_string())?;
        }
        Ok(Redb {
            db,
            path: p,
            txn: None,
        })
    }

    /// The held read transaction, opened if there is not one.
    fn snapshot(&mut self) -> Res<&redb::ReadTransaction> {
        if self.txn.is_none() {
            self.txn = Some(self.db.begin_read().map_err(|e| e.to_string())?);
        }
        Ok(self.txn.as_ref().expect("just filled"))
    }
}

impl Engine for Redb {
    fn name(&self) -> &'static str {
        "redb"
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: true,
            transactions: true,
            checksums: true,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        self.txn = None;
        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = w.open_table(T).map_err(|e| e.to_string())?;
            for &(k, v) in items {
                t.insert(k, v).map_err(|e| e.to_string())?;
            }
        }
        w.commit().map_err(|e| e.to_string())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        let r = self.snapshot()?;
        let t = r.open_table(T).map_err(|e| e.to_string())?;
        Ok(t.get(key)
            .map_err(|e| e.to_string())?
            .map(|v| v.value().len())
            .unwrap_or(0))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let r = self.snapshot()?;
        let t = r.open_table(T).map_err(|e| e.to_string())?;
        let mut bytes = 0usize;
        for row in t.range(from..).map_err(|e| e.to_string())?.take(n) {
            let (_, v) = row.map_err(|e| e.to_string())?;
            bytes += v.value().len();
        }
        Ok(bytes)
    }
    fn sync(&mut self) -> Res<()> {
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------- rocksdb --

/// RocksDB through rust-rocksdb.
///
/// The engine the next engine is shaped like -- a write-ahead log, a
/// memtable, sorted immutable files and compaction -- and so the comparator
/// that separates "the next engine is fast" from "an LSM is fast". Two
/// arms, as for LMDB: `rocksdb` syncs the WAL on every batch, matching the
/// next engine's `Sync::Always` and LMDB's default; `rocksdb-nosync` writes
/// the WAL and lets the OS get to it, matching `lmdb-nosync` and
/// `supdb-buffered`.
///
/// Its options are RocksDB's defaults except that compression is off, because
/// every other engine here stores values as written (block compression is
/// off in supdb since f12 priced it) and a comparison of compressed bytes
/// against plain ones would be a comparison of codecs, and that reads do
/// not verify block checksums, because the matched arms of every other
/// engine here verify none and the fairness gate refuses to rank a pair
/// that differs on that axis. RocksDB still *computes* a CRC-32C per block
/// when it writes one; that residual leans against RocksDB on the load,
/// by one hardware CRC per 4 KB block, and is named here rather than
/// equalized because the table format does not offer a switch this
/// binding exposes.
#[cfg(feature = "rocksdb")]
pub struct Rocks {
    db: rocksdb::DB,
    path: PathBuf,
    sync: bool,
    tuned: bool,
    /// Whether `sync` flushes the memtable and compacts everything, so the
    /// load window carries the same drain the next engine's default does
    /// and the reads run against a fully compacted tree.
    drain: bool,
    read: rocksdb::ReadOptions,
}

#[cfg(feature = "rocksdb")]
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
    /// compacted into one, inside the window -- the next engine's default
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
            // The write side, tuned for the load the suite runs and stated
            // here so EXT.32 and EXT.36 read as "against RocksDB tuned for
            // the load" too: 128 MB write buffers, four of them, merged two
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
            db,
            path: path.to_path_buf(),
            sync,
            tuned,
            drain,
            read,
        })
    }
}

#[cfg(feature = "rocksdb")]
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
            // read-your-writes contract the next engine's `Txn` and LMDB's
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
        // Pinned: the value is borrowed from the block cache, not copied out,
        // which is the cheapest read RocksDB offers and the fair one against
        // engines that hand back a borrow.
        Ok(self
            .db
            .get_pinned_opt(key, &self.read)
            .map_err(|e| e.to_string())?
            .map(|v| v.len())
            .unwrap_or(0))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        // By value, and `ReadOptions` does not clone: one per scan, which is
        // one small allocation against a walk of `n` entries.
        let mut ro = rocksdb::ReadOptions::default();
        ro.set_verify_checksums(false);
        let mut it = self.db.raw_iterator_opt(ro);
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
        // Values are borrowed from the mapping, never copied.
        Ok(db
            .get(r, key)
            .map_err(|e| e.to_string())?
            .map(|v| v.len())
            .unwrap_or(0))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let db = self.db;
        let r = self.snapshot()?;
        let mut bytes = 0usize;
        let range = (std::ops::Bound::Included(from), std::ops::Bound::Unbounded);
        for row in db.range(r, &range).map_err(|e| e.to_string())?.take(n) {
            let (_, v) = row.map_err(|e| e.to_string())?;
            bytes += v.len();
        }
        Ok(bytes)
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
pub struct Sled {
    db: sled::Db,
    path: PathBuf,
}

impl Sled {
    pub fn create(path: &Path) -> Res<Sled> {
        let db = sled::Config::new()
            .path(path)
            .open()
            .map_err(|e| e.to_string())?;
        Ok(Sled {
            db,
            path: path.to_path_buf(),
        })
    }
}

impl Engine for Sled {
    fn name(&self) -> &'static str {
        "sled"
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: true,
            transactions: true,
            checksums: true,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        let mut b = sled::Batch::default();
        for &(k, v) in items {
            b.insert(k, v);
        }
        self.db.apply_batch(b).map_err(|e| e.to_string())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        Ok(self
            .db
            .get(key)
            .map_err(|e| e.to_string())?
            .map(|v| v.len())
            .unwrap_or(0))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let mut bytes = 0usize;
        for row in self.db.range(from..).take(n) {
            let (_, v) = row.map_err(|e| e.to_string())?;
            bytes += v.len();
        }
        Ok(bytes)
    }
    fn sync(&mut self) -> Res<()> {
        self.db.flush().map_err(|e| e.to_string()).map(|_| ())
    }
    fn size_bytes(&self) -> u64 {
        dir_size(&self.path)
    }
}

// --------------------------------------------------------------- lmdb-dup --

/// LMDB in its genuinely best shape for a day index: `MDB_DUPSORT |
/// MDB_DUPFIXED`, postings stored as fixed-width duplicate values under their
/// term key.
///
/// Exists for `ext-analytics`. The flashiest numbers in this repository --
/// `count_fixed` at 27x and `scan_counts_fixed` at 283x (W2.2, W2.4) -- were
/// measured against Supdb's own varint walk, never against a competitor's
/// best effort. This adapter is that best effort: DUPFIXED packs same-width
/// dups end to end with no per-value header, `mdb_cursor_count` answers a
/// per-key count from a count the B-tree already stores rather than by
/// walking anything -- the exact format change W2.3 priced for Supdb and
/// declined -- and `MDB_GET_MULTIPLE`/`MDB_NEXT_MULTIPLE` hand back a page of
/// postings per call. Feeding postings through the plain `Lmdb` adapter above
/// -- values concatenated by hand, or one key per (term, line) pair -- would
/// be the design document's Java-harness mistake again: a comparison against
/// a configuration nobody would deploy.
///
/// It deliberately does **not** implement `Engine`. On a DUPSORT database
/// `put` inserts another value under the key where every `Engine` here
/// overwrites, so entering it into the kv/ycsb shapes would time a different
/// operation and print it in the same column. It has exactly the operations
/// the analytics suite measures, plus the build path.
///
/// Raw `lmdb-master-sys` rather than heed, and that needs saying: heed 0.20
/// exposes neither cursors nor its sys crate, and every query here is a
/// cursor operation (`mdb_cursor_count`, `MDB_GET_MULTIPLE`,
/// `MDB_NEXT_NODUP`). The sys crate is the same build of the same LMDB
/// (0.9.70) that the `Lmdb` adapter above links through heed -- one crate
/// instance in the lockfile, so the C code under measurement is
/// byte-identical -- and what this adapter skips is heed's typed wrapper,
/// which if it is anything is a bias in LMDB's favour.
///
/// The fairness rules from the top of this file, applied: one read
/// transaction held for the life of the adapter (the store is immutable once
/// built), cursors opened once and repositioned with `MDB_SET` rather than
/// reopened, and no allocation per value anywhere -- pages and single values
/// are handed out as borrows from the map.
pub struct LmdbDup {
    env: *mut mdb::MDB_env,
    dbi: mdb::MDB_dbi,
    /// The build transaction, alive between `begin_load` and `end_load`.
    wtxn: *mut mdb::MDB_txn,
    /// The held read transaction, opened by `end_load`, and the two cursors
    /// bound to it. Two, because an intersection walks two dup lists at once.
    rtxn: *mut mdb::MDB_txn,
    cur: *mut mdb::MDB_cursor,
    cur2: *mut mdb::MDB_cursor,
    path: PathBuf,
}

fn mdb_err(rc: std::os::raw::c_int, what: &str) -> String {
    // mdb_strerror hands back a static string for every code LMDB defines.
    let msg = unsafe { std::ffi::CStr::from_ptr(mdb::mdb_strerror(rc)) };
    format!("{what}: {}", msg.to_string_lossy())
}

fn ck(rc: std::os::raw::c_int, what: &str) -> Res<()> {
    if rc == mdb::MDB_SUCCESS {
        Ok(())
    } else {
        Err(mdb_err(rc, what))
    }
}

/// An input `MDB_val`. LMDB never writes through the pointer on the get and
/// put paths used here, so the cast from `*const` is sound.
fn mval(b: &[u8]) -> mdb::MDB_val {
    mdb::MDB_val {
        mv_size: b.len(),
        mv_data: b.as_ptr() as *mut _,
    }
}

fn mval_out() -> mdb::MDB_val {
    mdb::MDB_val {
        mv_size: 0,
        mv_data: std::ptr::null_mut(),
    }
}

/// # Safety
/// `v` must have been filled in by a successful `mdb_cursor_get` on a
/// transaction that is still live; the slice borrows the map for `'a`, which
/// the caller must keep inside that transaction's lifetime.
unsafe fn mslice<'a>(v: &mdb::MDB_val) -> &'a [u8] {
    if v.mv_size == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(v.mv_data as *const u8, v.mv_size)
    }
}

/// A dup list walked page-at-a-time: `MDB_GET_MULTIPLE` for the first page,
/// `MDB_NEXT_MULTIPLE` for the rest. The one wrinkle is a key with a single
/// value: LMDB stores it inline with no dup sub-structure, `GET_MULTIPLE`
/// then returns success *without touching the output val* (mdb.c breaks out
/// before `fetchm`), and the value has to come from `MDB_GET_CURRENT`
/// instead. The null `mv_data` this struct initialises is how that case is
/// detected.
struct DupPages {
    cur: *mut mdb::MDB_cursor,
    page: mdb::MDB_val,
    pos: usize,
}

impl DupPages {
    /// Position `cur` on `key` and fetch the first page. `None` when the key
    /// is absent.
    fn start(cur: *mut mdb::MDB_cursor, key: &[u8]) -> Res<Option<DupPages>> {
        unsafe {
            let mut k = mval(key);
            let mut d = mval_out();
            let rc = mdb::mdb_cursor_get(cur, &mut k, &mut d, mdb::MDB_SET);
            if rc == mdb::MDB_NOTFOUND {
                return Ok(None);
            }
            ck(rc, "mdb_cursor_get(MDB_SET)")?;
            let mut page = mval_out();
            let rc = mdb::mdb_cursor_get(cur, &mut k, &mut page, mdb::MDB_GET_MULTIPLE);
            if rc == mdb::MDB_NOTFOUND {
                return Ok(None);
            }
            ck(rc, "mdb_cursor_get(MDB_GET_MULTIPLE)")?;
            if page.mv_data.is_null() {
                // Single inline value, no sub-page: the "page" is the value
                // itself, via GET_CURRENT.
                let mut d = mval_out();
                ck(
                    mdb::mdb_cursor_get(cur, &mut k, &mut d, mdb::MDB_GET_CURRENT),
                    "mdb_cursor_get(MDB_GET_CURRENT)",
                )?;
                page = d;
            }
            Ok(Some(DupPages { cur, page, pos: 0 }))
        }
    }

    /// The current page's remaining bytes.
    fn rest(&self) -> &[u8] {
        // Safety: `page` was filled by a successful cursor_get and the read
        // transaction outlives this struct's use.
        unsafe { &mslice(&self.page)[self.pos..] }
    }

    /// Step `width` bytes forward, crossing to the next page when this one is
    /// exhausted. `false` when the list ends.
    fn advance(&mut self, width: usize) -> Res<bool> {
        self.pos += width;
        if self.pos < self.page.mv_size {
            return Ok(true);
        }
        unsafe {
            let mut k = mval_out();
            let mut page = mval_out();
            let rc = mdb::mdb_cursor_get(self.cur, &mut k, &mut page, mdb::MDB_NEXT_MULTIPLE);
            if rc == mdb::MDB_NOTFOUND {
                return Ok(false);
            }
            ck(rc, "mdb_cursor_get(MDB_NEXT_MULTIPLE)")?;
            self.page = page;
            self.pos = 0;
            Ok(true)
        }
    }
}

impl LmdbDup {
    pub fn create(path: &Path, map_gb: usize) -> Res<LmdbDup> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let cpath = std::ffi::CString::new(path.to_str().ok_or("non-utf8 path")?)
            .map_err(|e| e.to_string())?;
        unsafe {
            let mut env: *mut mdb::MDB_env = std::ptr::null_mut();
            ck(mdb::mdb_env_create(&mut env), "mdb_env_create")?;
            ck(
                mdb::mdb_env_set_mapsize(env, map_gb << 30),
                "mdb_env_set_mapsize",
            )?;
            // Flags 0 and mode 0644, exactly as heed opens the `Lmdb` engine
            // above: full sync on commit, readahead on.
            let rc = mdb::mdb_env_open(env, cpath.as_ptr(), 0, 0o644);
            if rc != mdb::MDB_SUCCESS {
                mdb::mdb_env_close(env);
                return Err(mdb_err(rc, "mdb_env_open"));
            }
            // The unnamed database, with the dup flags made persistent by a
            // committed write transaction.
            let mut txn: *mut mdb::MDB_txn = std::ptr::null_mut();
            ck(
                mdb::mdb_txn_begin(env, std::ptr::null_mut(), 0, &mut txn),
                "mdb_txn_begin",
            )?;
            let mut dbi: mdb::MDB_dbi = 0;
            ck(
                mdb::mdb_dbi_open(
                    txn,
                    std::ptr::null(),
                    mdb::MDB_DUPSORT | mdb::MDB_DUPFIXED,
                    &mut dbi,
                ),
                "mdb_dbi_open",
            )?;
            ck(mdb::mdb_txn_commit(txn), "mdb_txn_commit")?;
            Ok(LmdbDup {
                env,
                dbi,
                wtxn: std::ptr::null_mut(),
                rtxn: std::ptr::null_mut(),
                cur: std::ptr::null_mut(),
                cur2: std::ptr::null_mut(),
                path: path.to_path_buf(),
            })
        }
    }

    /// What this engine promises, honestly. It is the `lmdb` row: the build
    /// commits with a full sync, reads are transactional snapshots, and there
    /// are no checksums to turn on. DUPFIXED changes the layout, not the
    /// guarantees.
    pub fn features(&self) -> Features {
        Features {
            durable_commit: true,
            transactions: true,
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }

    // ---- build ----

    pub fn begin_load(&mut self) -> Res<()> {
        unsafe {
            let mut txn: *mut mdb::MDB_txn = std::ptr::null_mut();
            ck(
                mdb::mdb_txn_begin(self.env, std::ptr::null_mut(), 0, &mut txn),
                "mdb_txn_begin(load)",
            )?;
            self.wtxn = txn;
        }
        Ok(())
    }

    /// One posting. The analytics suite feeds these grouped by term and
    /// ascending within a term, which is a sorted insert for this database --
    /// the shape LMDB likes best. The build is not timed either way.
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Res<()> {
        if self.wtxn.is_null() {
            return Err("put outside begin_load/end_load".into());
        }
        unsafe {
            let mut k = mval(key);
            let mut v = mval(value);
            ck(
                mdb::mdb_put(self.wtxn, self.dbi, &mut k, &mut v, 0),
                "mdb_put",
            )
        }
    }

    /// Commit the build, then open the held read transaction and both
    /// cursors. After this the adapter is read-only.
    pub fn end_load(&mut self) -> Res<()> {
        unsafe {
            ck(mdb::mdb_txn_commit(self.wtxn), "mdb_txn_commit(load)")?;
            self.wtxn = std::ptr::null_mut();
            let mut txn: *mut mdb::MDB_txn = std::ptr::null_mut();
            ck(
                mdb::mdb_txn_begin(self.env, std::ptr::null_mut(), mdb::MDB_RDONLY, &mut txn),
                "mdb_txn_begin(read)",
            )?;
            self.rtxn = txn;
            ck(
                mdb::mdb_cursor_open(self.rtxn, self.dbi, &mut self.cur),
                "mdb_cursor_open",
            )?;
            ck(
                mdb::mdb_cursor_open(self.rtxn, self.dbi, &mut self.cur2),
                "mdb_cursor_open",
            )?;
        }
        Ok(())
    }

    // ---- the four queries ----

    /// q2: the count of one key's postings. `MDB_SET` positions, and
    /// `mdb_cursor_count` reads `md_entries` out of the dup tree's header --
    /// a stored count, not a walk. Zero for a key that is not there.
    pub fn count(&mut self, key: &[u8]) -> Res<u64> {
        unsafe {
            let mut k = mval(key);
            let mut d = mval_out();
            let rc = mdb::mdb_cursor_get(self.cur, &mut k, &mut d, mdb::MDB_SET);
            if rc == mdb::MDB_NOTFOUND {
                return Ok(0);
            }
            ck(rc, "mdb_cursor_get(MDB_SET)")?;
            let mut n: mdb::mdb_size_t = 0;
            ck(mdb::mdb_cursor_count(self.cur, &mut n), "mdb_cursor_count")?;
            Ok(n as u64)
        }
    }

    /// q1: one pass over the whole dictionary, handing `f` every key and its
    /// stored count. `MDB_NEXT_NODUP` steps over each dup list without
    /// entering it. Returns the number of keys visited.
    pub fn rank_pass<F: FnMut(&[u8], u64)>(&mut self, mut f: F) -> Res<u64> {
        unsafe {
            let mut k = mval_out();
            let mut d = mval_out();
            let mut rc = mdb::mdb_cursor_get(self.cur, &mut k, &mut d, mdb::MDB_FIRST);
            let mut keys = 0u64;
            while rc == mdb::MDB_SUCCESS {
                let mut n: mdb::mdb_size_t = 0;
                ck(mdb::mdb_cursor_count(self.cur, &mut n), "mdb_cursor_count")?;
                f(mslice(&k), n as u64);
                keys += 1;
                rc = mdb::mdb_cursor_get(self.cur, &mut k, &mut d, mdb::MDB_NEXT_NODUP);
            }
            if rc != mdb::MDB_NOTFOUND {
                return Err(mdb_err(rc, "mdb_cursor_get(MDB_NEXT_NODUP)"));
            }
            Ok(keys)
        }
    }

    /// q3: every posting under one key, a `MDB_GET_MULTIPLE` page at a time.
    /// `f` is handed each page (fixed-width values packed end to end) as a
    /// borrow from the map; nothing is copied. Returns total bytes visited.
    pub fn read_postings<F: FnMut(&[u8])>(&mut self, key: &[u8], mut f: F) -> Res<u64> {
        let Some(mut pages) = DupPages::start(self.cur, key)? else {
            return Ok(0);
        };
        let mut bytes = 0u64;
        loop {
            let rest = pages.rest();
            bytes += rest.len() as u64;
            f(rest);
            // Jump to the end of the page; `advance` then fetches the next
            // one or reports the end of the list.
            pages.pos = pages.page.mv_size;
            if !pages.advance(0)? {
                return Ok(bytes);
            }
        }
    }

    /// q4: how many values two keys' dup lists share. A two-pointer merge
    /// over both lists, page-at-a-time on each side, comparing fixed-width
    /// values as byte strings -- which is dup order on this database, and
    /// numeric order for the suite's big-endian postings. A seek-based
    /// leapfrog (`MDB_GET_BOTH_RANGE`) exists and is not exercised here; for
    /// the day-index shape the lists are dense enough that stepping is the
    /// honest default.
    pub fn intersect_fixed(&mut self, ka: &[u8], kb: &[u8], width: usize) -> Res<u64> {
        let a = DupPages::start(self.cur, ka)?;
        let b = DupPages::start(self.cur2, kb)?;
        let (Some(mut a), Some(mut b)) = (a, b) else {
            return Ok(0);
        };
        let mut matches = 0u64;
        loop {
            let av = &a.rest()[..width];
            let bv = &b.rest()[..width];
            match av.cmp(bv) {
                std::cmp::Ordering::Equal => {
                    matches += 1;
                    if !a.advance(width)? || !b.advance(width)? {
                        break;
                    }
                }
                std::cmp::Ordering::Less => {
                    if !a.advance(width)? {
                        break;
                    }
                }
                std::cmp::Ordering::Greater => {
                    if !b.advance(width)? {
                        break;
                    }
                }
            }
        }
        Ok(matches)
    }

    pub fn size_bytes(&self) -> u64 {
        dir_size(&self.path)
    }
}

impl Drop for LmdbDup {
    fn drop(&mut self) {
        unsafe {
            if !self.cur.is_null() {
                mdb::mdb_cursor_close(self.cur);
            }
            if !self.cur2.is_null() {
                mdb::mdb_cursor_close(self.cur2);
            }
            if !self.rtxn.is_null() {
                mdb::mdb_txn_abort(self.rtxn);
            }
            if !self.wtxn.is_null() {
                mdb::mdb_txn_abort(self.wtxn);
            }
            if !self.env.is_null() {
                mdb::mdb_env_close(self.env);
            }
        }
    }
}
