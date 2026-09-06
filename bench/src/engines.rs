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

/// Supdb (`supdb::Db`): a WAL-only commit with sealed segments in
/// today's store format. Always durable -- a commit is a WAL append plus one
/// fdatasync, which is LMDB's own boundary, so this arm is guarantee-matched
/// against `lmdb` the way `supdb-durable` is. Scans pay the unrouted fan
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
    /// leaves the tail in memory, as RocksDB's `sync` does. f60 found the
    /// drain to be 11% of the canonical window and the whole of the seal
    /// phase, so both shapes are arms (drain-plan.md).
    drain: bool,
    /// A read advice pinned against the engine's own default, or `None` to
    /// take whatever the default is.
    ///
    /// `None` for the canonical arm, deliberately: the numbers this project
    /// quotes should describe what a user gets, so the arm follows the
    /// default rather than pinning a setting beside it. The contrast arm
    /// pins the kernel's plain readahead, and `EXT.46` and `EXT.47` are the
    /// two run interleaved -- which is the only way to price this, since the
    /// three unchanged comparators in this suite once moved +20% to +43%
    /// between consecutive runs.
    advice: Option<supdb::ReadAdvice>,
}

impl Supdb {
    pub fn create(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(path, true, true, None)
    }

    pub fn create_ingest(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(path, false, true, None)
    }

    /// `sync` fsyncs and seals nothing; reads then answer from the
    /// memtable, the unrouted tail and the partitions together.
    pub fn create_nodrain(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(path, true, false, None)
    }

    /// `supdb` in every respect but the read advice, which is pinned to the
    /// kernel's plain readahead. The pair differs by one option and needs no
    /// matching.
    pub fn create_noadvice(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(path, true, true, Some(supdb::ReadAdvice::Normal))
    }

    fn with_policy(
        path: &Path,
        partition: bool,
        drain: bool,
        advice: Option<supdb::ReadAdvice>,
    ) -> Res<Supdb> {
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
        })
    }
}

impl Engine for Supdb {
    fn name(&self) -> &'static str {
        match (self.partition, self.drain, self.advice.is_some()) {
            (true, true, true) => "supdb-noadvice",
            (true, true, false) => "supdb",
            (false, _, _) => "supdb-ingest",
            (true, false, _) => "supdb-nodrain",
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
    db: rocksdb::DB,
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
// ---------------------------------------------------------------------------
// The arms.
use crate::row::Guarantee;

/// Every arm a run measures, in the order they are interleaved. Each is a
/// shipping supdb configuration or the comparator a user would otherwise
/// pick. Comparisons are made within a guarantee, never across one.
pub const ARMS: [&str; 7] = [
    "supdb",
    "supdb-noadvice",
    "lmdb",
    "rocksdb-tuned",
    "supdb-ingest",
    "lmdb-nosync",
    "rocksdb-nosync",
];

pub fn guarantee(arm: &str) -> Option<Guarantee> {
    Some(match arm {
        "supdb" | "supdb-noadvice" | "lmdb" | "rocksdb-tuned" => Guarantee::Durable,
        "supdb-ingest" | "lmdb-nosync" | "rocksdb-nosync" => Guarantee::Buffered,
        _ => return None,
    })
}

/// Open an arm on a fresh directory. `map_gb` sizes LMDB's map; the other
/// engines grow on their own.
pub fn open(arm: &str, dir: &Path, map_gb: usize) -> Res<Box<dyn Engine>> {
    Ok(match arm {
        "supdb" => Box::new(Supdb::create(dir)?),
        "supdb-noadvice" => Box::new(Supdb::create_noadvice(dir)?),
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
        let durable = g == Guarantee::Durable;
        for w in list.windows(2) {
            let gap = w[0].1.unmatched(&w[1].1, durable);
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
