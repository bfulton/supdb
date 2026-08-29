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
//!     does not commit durably and LMDB does; EXT.1 compared their load
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

pub trait Engine {
    fn name(&self) -> &'static str;
    fn features(&self) -> Features;
    /// Write a batch and make it visible to this engine's own read path.
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()>;
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

// ------------------------------------------------------------------ supdb --

/// Supdb.
///
/// Two structural constraints show up here that no benchmark in the design
/// document can reach, because none of them mixes reads with writes:
///
///   * `Store` has no read method at all. Reading requires `Reader::open`,
///     which is a snapshot of the last checkpoint -- so serving a read
///     requires a checkpoint and a fresh reader, and both are O(key count).
///   * a reopened store declares history before the reopen broken, since the
///     reuse log is not carried across.
pub struct Supdb {
    store: Option<supdb::Store>,
    path: PathBuf,
    /// Take a durability point at the end of every batch, the way LMDB's
    /// `commit` does.
    ///
    /// Without this Supdb buffers the whole load and reaches the device once,
    /// at `sync`. LMDB opens a write transaction per batch and commits it, and
    /// heed sets no MDB_NOSYNC, so every one of those commits is an fsync.
    /// `Options::sync` is already `Sync::Always` here; what this changes is
    /// when a checkpoint happens, not whether it reaches the device.
    durable: bool,
    /// Off in the matched arms, because LMDB has no checksums to turn on.
    ///
    /// This axis was unequalized in Supdb's *dis*favour for as long as
    /// durability was unequalized in its favour: f8 prices verification at
    /// +8.5% on writes, and every EXT.1 and EXT.4 figure has been paying it
    /// against a comparator that does not.
    checksums: bool,
}

impl Supdb {
    /// The shipping defaults: buffered writes, checksums on.
    pub fn create(path: &Path, buffer_mb: usize) -> Res<Supdb> {
        Supdb::with_guarantees(path, buffer_mb, false, true)
    }

    /// Matched to `lmdb`: commits on the same boundary, checksums neither.
    pub fn create_durable(path: &Path, buffer_mb: usize) -> Res<Supdb> {
        Supdb::with_guarantees(path, buffer_mb, true, false)
    }

    /// Matched to `lmdb-nosync`: neither reaches the device per batch,
    /// checksums neither.
    pub fn create_buffered(path: &Path, buffer_mb: usize) -> Res<Supdb> {
        Supdb::with_guarantees(path, buffer_mb, false, false)
    }

    fn with_guarantees(
        path: &Path,
        buffer_mb: usize,
        durable: bool,
        checksums: bool,
    ) -> Res<Supdb> {
        let file = path.join("supdb.dat");
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: buffer_mb << 20,
                checksums,
                // `AfterReads` cannot function in a load-only phase -- there
                // are no reads -- and the durable arm appends an index section
                // per batch, so it is the arm that most needs sections back.
                // Measured both ways at `dev`: `Now` gives 41,305 ops/s and a
                // 479MB file, `AfterReads` 36,715 and 610MB. The verdict is
                // the same either way (0.051x against 0.054x of LMDB), so this
                // is not the thumb on the scale it would be if it were -- it
                // is here so nobody has to wonder whether the durable arm lost
                // because of a policy whose precondition it never meets.
                reclaim: if durable {
                    supdb::Reclaim::Now
                } else {
                    supdb::Reclaim::AfterReads
                },
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(Supdb {
            store: Some(store),
            path: file,
            durable,
            checksums,
        })
    }
}

impl Engine for Supdb {
    fn name(&self) -> &'static str {
        match (self.durable, self.checksums) {
            (true, _) => "supdb-durable",
            (false, false) => "supdb-buffered",
            (false, true) => "supdb",
        }
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: self.durable,
            transactions: false,
            // True since the CRC-32C work: `Options::checksums` defaults on,
            // this adapter takes that default, and f8-checksums measures what
            // it costs (+8.5% write, no measurable read cost, +0.166% on
            // disk). The table said false long after that stopped being so,
            // which understated the engine by a feature point in the one
            // comparison that is meant to price its missing features.
            checksums: self.checksums,
            // True since `Store::open`. `Store::create` truncates, so until it
            // existed a store could be written once and thereafter only read
            // -- the review lists that first among critical defects and it had
            // no test, because a limitation preventing the second session also
            // prevents the test of the second session.
            reopen_for_write: true,
            // True since `Store::read_all`: the writer reads its own buffered,
            // staged and sealed state directly, with no checkpoint and no
            // reader rebuild. It was false when that cost a full publish.
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()> {
        let s = self.store.as_ref().ok_or("store closed")?;
        for (k, v) in items {
            s.put(k, v).map_err(|e| e.to_string())?;
        }
        if self.durable {
            // `Options::sync` is Sync::Always, so this reaches the device.
            s.checkpoint().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        // Read the writer's own state. This used to call `refresh`, which
        // checkpoints and reopens a Reader -- the read-your-writes path that
        // put the mixed YCSB workloads two orders of magnitude behind LMDB,
        // which needs neither.
        let s = self.store.as_ref().ok_or("store closed")?;
        let mut n = 0usize;
        s.read_all(key, |v| n += v.len())
            .map_err(|e| e.to_string())?;
        Ok(n)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        // Ordered reads from the writer's own state. This used to call
        // `refresh`, which published and then opened a `Reader` -- a second
        // mapping of the same file with an empty verified bitset, so a scan
        // re-verified a store this process had just written and checksummed.
        // `Store::scan` publishes only when something is outstanding and asks
        // the store itself, which is warm from the read path.
        let s = self.store.as_ref().ok_or("store closed")?;
        let mut bytes = 0usize;
        s.scan(Some(from), n, |_k, v| bytes += v.len())
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
    fn sync(&mut self) -> Res<()> {
        if let Some(s) = &self.store {
            s.flush().map_err(|e| e.to_string())?;
            s.checkpoint().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
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
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()> {
        self.txn = None;
        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = w.open_table(T).map_err(|e| e.to_string())?;
            for (k, v) in items {
                t.insert(k.as_slice(), v.as_slice())
                    .map_err(|e| e.to_string())?;
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
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()> {
        // A held read transaction pins the version it was opened at, so it has
        // to go before a write, exactly as Supdb's adapter drops its Reader.
        self.txn = None;
        let mut w = self.env.write_txn().map_err(|e| e.to_string())?;
        for (k, v) in items {
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
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()> {
        let mut b = sled::Batch::default();
        for (k, v) in items {
            b.insert(k.as_slice(), v.as_slice());
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
