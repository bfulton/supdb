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
//!   * **What each engine promises is recorded**, not assumed. Supdb has no
//!     write-ahead log, no transactions, no checksums, and cannot reopen a
//!     store for writing. Comparing its throughput against engines that
//!     provide all four without saying so is the comparison the review
//!     objected to.

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
///   * `Store::create` truncates, so a store cannot be reopened for writing.
pub struct Supdb {
    store: Option<supdb::Store>,
    reader: Option<supdb::Reader>,
    path: PathBuf,
    /// Reader refreshes forced by a read after a write. Counted because it is
    /// the cost of Supdb's snapshot read model under a mixed workload.
    pub refreshes: u64,
    dirty: bool,
}

impl Supdb {
    pub fn create(path: &Path, buffer_mb: usize) -> Res<Supdb> {
        let file = path.join("supdb.dat");
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: buffer_mb << 20,
                reclaim: supdb::Reclaim::AfterReads,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(Supdb {
            store: Some(store),
            reader: None,
            path: file,
            refreshes: 0,
            dirty: false,
        })
    }

    /// Publish buffered writes and rebuild the snapshot a read can see.
    fn refresh(&mut self) -> Res<()> {
        if !self.dirty && self.reader.is_some() {
            return Ok(());
        }
        if let Some(s) = &self.store {
            s.checkpoint().map_err(|e| e.to_string())?;
        }
        self.reader = Some(supdb::Reader::open(&self.path).map_err(|e| e.to_string())?);
        self.refreshes += 1;
        self.dirty = false;
        Ok(())
    }
}

impl Engine for Supdb {
    fn name(&self) -> &'static str {
        "supdb"
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: false,
            transactions: false,
            checksums: false,
            reopen_for_write: false,
            // Only after a checkpoint and a reader rebuild, both O(keys).
            read_your_writes: false,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()> {
        let s = self.store.as_ref().ok_or("store closed")?;
        for (k, v) in items {
            s.put(k, v).map_err(|e| e.to_string())?;
        }
        self.dirty = true;
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        self.refresh()?;
        let r = self.reader.as_ref().ok_or("no reader")?;
        let mut n = 0usize;
        r.read_all(key, |v| n += v.len())
            .map_err(|e| e.to_string())?;
        Ok(n)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        self.refresh()?;
        let r = self.reader.as_ref().ok_or("no reader")?;
        let mut bytes = 0usize;
        r.scan(Some(from), n, |_k, v| bytes += v.len())
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
    fn sync(&mut self) -> Res<()> {
        if let Some(s) = &self.store {
            s.flush().map_err(|e| e.to_string())?;
            s.checkpoint().map_err(|e| e.to_string())?;
        }
        self.dirty = true;
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
        Ok(Redb { db, path: p })
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
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let t = r.open_table(T).map_err(|e| e.to_string())?;
        Ok(t.get(key)
            .map_err(|e| e.to_string())?
            .map(|v| v.value().len())
            .unwrap_or(0))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
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
}

impl Lmdb {
    pub fn create(path: &Path, map_gb: usize) -> Res<Lmdb> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let env = unsafe {
            heed::EnvOpenOptions::new()
                .map_size(map_gb * 1024 * 1024 * 1024)
                .max_dbs(4)
                .open(path)
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
        })
    }
}

impl Engine for Lmdb {
    fn name(&self) -> &'static str {
        "lmdb"
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: true,
            transactions: true,
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(Vec<u8>, Vec<u8>)]) -> Res<()> {
        let mut w = self.env.write_txn().map_err(|e| e.to_string())?;
        for (k, v) in items {
            self.db.put(&mut w, k, v).map_err(|e| e.to_string())?;
        }
        w.commit().map_err(|e| e.to_string())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        // One transaction per lookup was the handicap the design document
        // found in its own LMDB adapter and removed for 2.3x; a read txn here
        // is cheap but a shared one would be cheaper still, so this is
        // conservative against LMDB rather than for it.
        let r = self.env.read_txn().map_err(|e| e.to_string())?;
        // Values are borrowed from the mapping, never copied.
        Ok(self
            .db
            .get(&r, key)
            .map_err(|e| e.to_string())?
            .map(|v| v.len())
            .unwrap_or(0))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let r = self.env.read_txn().map_err(|e| e.to_string())?;
        let mut bytes = 0usize;
        let range = (std::ops::Bound::Included(from), std::ops::Bound::Unbounded);
        for row in self
            .db
            .range(&r, &range)
            .map_err(|e| e.to_string())?
            .take(n)
        {
            let (_, v) = row.map_err(|e| e.to_string())?;
            bytes += v.len();
        }
        Ok(bytes)
    }
    fn sync(&mut self) -> Res<()> {
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
