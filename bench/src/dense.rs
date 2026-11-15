//! A probe form for one cell, not an engine: YCSB-C at ten million keys
//! under 1.25 GiB, the cell LMDB wins because its B+tree fits the bound
//! and supdb's raw form does not.
//!
//! The suite's keys are the sixteen decimal digits of their number, loaded
//! in order, and its values are a fixed hundred bytes. A key's number is
//! then its position, so the form holds nothing but the values, back to
//! back at their stride (`values.bin`): a point read parses the key and
//! reads one slot, a line or two and one translation where LMDB descends a
//! tree, and the whole form is the values' bytes, a gigabyte at ten
//! million keys where LMDB's file is 1.33. Whatever is not the next key in
//! the run or not a value of the stride -- the mixes' updates, their
//! inserts out of order -- goes to an ordered overlay in memory, logged
//! and synced as a batch commits (`overlay.log`), and a read asks it first
//! once it holds anything.
//!
//! It answers what this cell's physics allows; how a store would carry it
//! is a question for after (a partition whose keys a model places exactly
//! and whose values are one size).

use super::{consume, dir_size, Engine, Features, ReaderOpener, Res, ThreadReader};
use crate::workload::db_key_into;
use memmap2::{Mmap, MmapOptions};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const META: &str = "dense.meta";
const VALS: &str = "values.bin";
const LOG: &str = "overlay.log";
/// Slots the mapping covers past the file's end, so the run's appends are
/// read without a remap at every batch.
const MAP_SPARE: u64 = 1 << 26;

/// The key's number where it is sixteen decimal digits.
#[inline]
fn parse16(k: &[u8]) -> Option<u64> {
    if k.len() != 16 {
        return None;
    }
    let mut n = 0u64;
    for &b in k {
        let d = b.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        n = n * 10 + u64::from(d);
    }
    Some(n)
}

type Overlay = BTreeMap<Vec<u8>, Vec<u8>>;

/// What a read needs: the mapping, the run's shape and the overlay, shared
/// with the reader threads as they were when the reader was opened.
#[derive(Clone)]
struct View {
    map: Option<Arc<Mmap>>,
    mapped: u64,
    base: u64,
    stride: usize,
    count: u64,
    over: Arc<Overlay>,
}

impl View {
    #[inline]
    fn slot(&self, i: u64) -> Option<&[u8]> {
        if i >= self.count || i >= self.mapped {
            return None;
        }
        let off = i as usize * self.stride;
        self.map.as_ref().map(|m| &m[off..off + self.stride])
    }

    #[inline]
    fn get(&self, key: &[u8]) -> usize {
        if !self.over.is_empty() {
            if let Some(v) = self.over.get(key) {
                return consume(v);
            }
        }
        let Some(n) = parse16(key) else {
            return 0;
        };
        if n < self.base {
            return 0;
        }
        self.slot(n - self.base).map_or(0, consume)
    }

    /// `n` entries from `from` in key order: the run's slots and the
    /// overlay merged, an overlay entry over a slot of the same key.
    fn range(&self, from: &[u8], n: usize) -> usize {
        let mut i = match parse16(from) {
            Some(k) => k.saturating_sub(self.base),
            None if from.is_empty() => 0,
            None => self.count,
        };
        let mut over = self.over.range(from.to_vec()..).peekable();
        let mut kb = [0u8; 16];
        let (mut bytes, mut seen) = (0usize, 0usize);
        while seen < n {
            let slot = (i < self.count).then(|| {
                db_key_into(self.base + i, &mut kb);
                kb
            });
            match (slot, over.peek()) {
                (None, None) => break,
                (Some(_), None) => {
                    bytes += self.slot(i).map_or(0, consume);
                    i += 1;
                }
                (None, Some(_)) => {
                    let (_, v) = over.next().unwrap();
                    bytes += consume(v);
                }
                (Some(k), Some((ok, _))) => match ok.as_slice().cmp(&k[..]) {
                    std::cmp::Ordering::Less => {
                        let (_, v) = over.next().unwrap();
                        bytes += consume(v);
                    }
                    std::cmp::Ordering::Equal => {
                        let (_, v) = over.next().unwrap();
                        bytes += consume(v);
                        i += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        bytes += self.slot(i).map_or(0, consume);
                        i += 1;
                    }
                },
            }
            seen += 1;
        }
        bytes
    }
}

pub struct Dense {
    dir: PathBuf,
    vals: File,
    log: File,
    view: View,
    pending: Vec<u8>,
    pend_log: Vec<u8>,
}

impl Dense {
    /// The store at `dir`, made where there is none: the run's base and
    /// stride from the meta file, its count from the values' length, and
    /// the overlay read back from its log.
    pub fn open(dir: &Path) -> Res<Dense> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let opts = || {
            let mut o = OpenOptions::new();
            o.read(true).append(true).create(true);
            o
        };
        let vals = opts().open(dir.join(VALS)).map_err(|e| e.to_string())?;
        let mut log = opts().open(dir.join(LOG)).map_err(|e| e.to_string())?;
        let (mut base, mut stride) = (0u64, 0usize);
        if let Ok(b) = std::fs::read(dir.join(META)) {
            if b.len() == 16 {
                base = u64::from_le_bytes(b[..8].try_into().unwrap());
                stride = u64::from_le_bytes(b[8..].try_into().unwrap()) as usize;
            }
        }
        let len = vals.metadata().map_err(|e| e.to_string())?.len();
        let count = if stride == 0 { 0 } else { len / stride as u64 };
        let mut over = Overlay::new();
        let mut buf = Vec::new();
        log.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let mut at = 0usize;
        while at + 6 <= buf.len() {
            let kl = u16::from_le_bytes([buf[at], buf[at + 1]]) as usize;
            let vl = u32::from_le_bytes(buf[at + 2..at + 6].try_into().unwrap()) as usize;
            let end = at + 6 + kl + vl;
            if end > buf.len() {
                break;
            }
            over.insert(
                buf[at + 6..at + 6 + kl].to_vec(),
                buf[at + 6 + kl..end].to_vec(),
            );
            at = end;
        }
        let mut d = Dense {
            dir: dir.to_path_buf(),
            vals,
            log,
            view: View {
                map: None,
                mapped: 0,
                base,
                stride,
                count,
                over: Arc::new(over),
            },
            pending: Vec::new(),
            pend_log: Vec::new(),
        };
        d.remap()?;
        Ok(d)
    }

    /// The values mapped with room past their end, advised random as
    /// `lmdb-nordahead` advises its file.
    fn remap(&mut self) -> Res<()> {
        let v = &mut self.view;
        if v.stride == 0 || v.count == 0 {
            return Ok(());
        }
        let slots = v.count + MAP_SPARE / v.stride as u64;
        let len = slots as usize * v.stride;
        // SAFETY: the file is this store's alone and only appended to; a
        // slot is read only once the file holds it.
        let m =
            unsafe { MmapOptions::new().len(len).map(&self.vals) }.map_err(|e| e.to_string())?;
        m.advise(memmap2::Advice::Random)
            .map_err(|e| e.to_string())?;
        v.map = Some(Arc::new(m));
        v.mapped = slots;
        Ok(())
    }

    fn flush(&mut self) -> Res<()> {
        if !self.pending.is_empty() {
            self.vals
                .write_all(&self.pending)
                .map_err(|e| e.to_string())?;
            self.vals.sync_data().map_err(|e| e.to_string())?;
            self.pending.clear();
        }
        if !self.pend_log.is_empty() {
            self.log
                .write_all(&self.pend_log)
                .map_err(|e| e.to_string())?;
            self.log.sync_data().map_err(|e| e.to_string())?;
            self.pend_log.clear();
        }
        if self.view.count > self.view.mapped || (self.view.map.is_none() && self.view.count > 0) {
            self.remap()?;
        }
        Ok(())
    }
}

struct DenseReader(View);

impl ThreadReader for DenseReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        Ok(self.0.get(key))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        Ok(self.0.range(from, n))
    }
}

impl Engine for Dense {
    fn name(&self) -> &'static str {
        "dense"
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
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        for &(k, v) in items {
            let n = parse16(k);
            if let Some(n) = n {
                if self.view.stride == 0 && self.view.count == 0 && !v.is_empty() {
                    self.view.base = n;
                    self.view.stride = v.len();
                    let mut m = Vec::with_capacity(16);
                    m.extend_from_slice(&n.to_le_bytes());
                    m.extend_from_slice(&(v.len() as u64).to_le_bytes());
                    let mut f = File::create(self.dir.join(META)).map_err(|e| e.to_string())?;
                    f.write_all(&m).map_err(|e| e.to_string())?;
                    f.sync_data().map_err(|e| e.to_string())?;
                }
                if v.len() == self.view.stride
                    && n == self.view.base + self.view.count
                    && !self.view.over.contains_key(k)
                {
                    self.pending.extend_from_slice(v);
                    self.view.count += 1;
                    continue;
                }
            }
            self.pend_log
                .extend_from_slice(&(k.len() as u16).to_le_bytes());
            self.pend_log
                .extend_from_slice(&(v.len() as u32).to_le_bytes());
            self.pend_log.extend_from_slice(k);
            self.pend_log.extend_from_slice(v);
            Arc::make_mut(&mut self.view.over).insert(k.to_vec(), v.to_vec());
        }
        self.flush()
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        Ok(self.view.get(key))
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        Ok(self.view.range(from, n))
    }
    fn sync(&mut self) -> Res<()> {
        self.flush()
    }
    fn size_bytes(&self) -> u64 {
        dir_size(&self.dir)
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        let v = self.view.clone();
        Ok(Box::new(move || {
            Ok(Box::new(DenseReader(v)) as Box<dyn ThreadReader>)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u64) -> Vec<u8> {
        let mut b = [0u8; 16];
        db_key_into(n, &mut b);
        b.to_vec()
    }

    #[test]
    fn the_dense_probe_reads_back_what_it_was_given_across_a_reopen() {
        let dir = std::env::temp_dir().join(format!("dense-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let val = |n: u64, tag: u8| vec![tag ^ (n as u8); 100];
        {
            let mut d = Dense::open(&dir).unwrap();
            for b in 0..10u64 {
                let ks: Vec<Vec<u8>> = (b * 100..b * 100 + 100).map(key).collect();
                let vs: Vec<Vec<u8>> = (b * 100..b * 100 + 100).map(|n| val(n, 1)).collect();
                let items: Vec<(&[u8], &[u8])> = ks
                    .iter()
                    .zip(&vs)
                    .map(|(k, v)| (k.as_slice(), v.as_slice()))
                    .collect();
                d.write_batch(&items).unwrap();
            }
            // An update, an insert out of order and an odd key: the overlay.
            let (u, o) = (val(7, 9), vec![3u8; 40]);
            d.write_batch(&[(&key(7), &u), (b"odd", &o), (&key(5000), &u)])
                .unwrap();
            assert_eq!(d.view.count, 1000);
            assert_eq!(d.view.over.len(), 3);
        }
        let mut d = Dense::open(&dir).unwrap();
        assert_eq!(d.view.count, 1000);
        assert_eq!(d.get(&key(0)).unwrap(), 100);
        assert_eq!(d.get(&key(999)).unwrap(), 100);
        assert_eq!(d.get(&key(1000)).unwrap(), 0);
        assert_eq!(d.get(&key(5000)).unwrap(), 100);
        assert_eq!(d.get(b"odd").unwrap(), 40);
        assert_eq!(d.get(b"nope").unwrap(), 0);
        let s = d.view.slot(7).unwrap().to_vec();
        assert_eq!(s, val(7, 1), "the slot keeps the loaded value");
        assert_eq!(d.view.over.get(&key(7)).unwrap(), &val(7, 9));
        // A scan merges the overlay: key 7 once, from the overlay.
        assert_eq!(d.range(&key(5), 5).unwrap(), 500);
        assert_eq!(d.range(&key(998), 5).unwrap(), 100 + 100 + 100 + 40);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
