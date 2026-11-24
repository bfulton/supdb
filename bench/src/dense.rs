//! A probe form for one cell, not an engine: YCSB-C at ten million keys
//! under 1.25 GiB, the cell LMDB wins because its B+tree fits the bound
//! and supdb's raw form does not.
//!
//! The suite's keys are the sixteen decimal digits of their number and its
//! values a fixed hundred bytes. A key's number is then its slot, so the
//! form holds nothing but the values at their stride (`values.bin`): a
//! point read parses the key and reads one slot, a line or two and one
//! translation where LMDB descends a tree, and the whole form is the
//! values' bytes, a gigabyte at ten million keys where LMDB's file is 1.33.
//! A key's first value is written into its slot wherever the slot lies, so
//! the shuffled load fills the file out of order and a word of presence a
//! slot says which are filled (`present.bin`, written at a sync); the
//! slots below the first empty one need no look at it. Whatever has a slot
//! filled already or no slot at all -- the mixes' updates, a key that is
//! not sixteen digits, a value of another length -- goes to an ordered
//! overlay in memory, logged and synced as a batch commits
//! (`overlay.log`), and a read asks it first once it holds anything.
//!
//! It answers what this cell's physics allows; how a store would carry it
//! is a question for after (a partition whose keys a model places exactly
//! and whose values are one size). Presence written only at a sync is the
//! probe's: a crash between a shuffled batch and the sync after it reopens
//! with the slots past the first empty one unread.

use super::{consume, dir_size, Engine, Features, ReaderOpener, Res, ThreadReader};
use crate::workload::db_key_into;
use memmap2::{Mmap, MmapOptions};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const META: &str = "dense.meta";
const VALS: &str = "values.bin";
const PRESENT: &str = "present.bin";
const LOG: &str = "overlay.log";
/// Slots the mapping covers past the furthest slot written, so a run of
/// appends is read without a remap at every batch.
const MAP_SPARE: u64 = 1 << 26;
/// The most slots the file takes: a key whose number lies past it goes to
/// the overlay rather than to a sparse file of its number's size.
const MAX_SLOTS: u64 = 1 << 30;

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

/// What a read needs: the mapping, the slots' shape and the overlay, shared
/// with the reader threads as they were when the reader was opened.
#[derive(Clone)]
struct View {
    map: Option<Arc<Mmap>>,
    mapped: u64,
    stride: usize,
    /// One past the furthest slot filled.
    count: u64,
    /// Every slot below it is filled.
    full: u64,
    /// A bit a slot, read only at and past `full`.
    present: Arc<Vec<u64>>,
    over: Arc<Overlay>,
}

impl View {
    #[inline]
    fn has(&self, i: u64) -> bool {
        i < self.full
            || (i < self.count
                && self
                    .present
                    .get((i / 64) as usize)
                    .is_some_and(|w| w >> (i % 64) & 1 == 1))
    }

    #[inline]
    fn slot(&self, i: u64) -> Option<&[u8]> {
        if !self.has(i) || i >= self.mapped {
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
        match parse16(key) {
            Some(n) => self.slot(n).map_or(0, consume),
            None => 0,
        }
    }

    /// `n` entries from `from` in key order: the filled slots and the
    /// overlay merged, an overlay entry over a slot of the same key.
    fn range(&self, from: &[u8], n: usize) -> usize {
        let mut i = match parse16(from) {
            Some(k) => k,
            None if from.is_empty() => 0,
            None => self.count,
        };
        let mut over = self.over.range(from.to_vec()..).peekable();
        let mut kb = [0u8; 16];
        let (mut bytes, mut seen) = (0usize, 0usize);
        while seen < n {
            while i < self.count && !self.has(i) {
                i += 1;
            }
            let slot = (i < self.count).then(|| {
                db_key_into(i, &mut kb);
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
    /// The batch's slot writes, coalesced into runs at the commit.
    pending: Vec<(u64, Vec<u8>)>,
    pend_log: Vec<u8>,
    /// Whether presence has moved since it was last written.
    present_dirty: bool,
}

impl Dense {
    /// The store at `dir`, made where there is none: the stride from the
    /// meta file, the furthest slot from the values' length, presence from
    /// its file or, where there is none, every slot below the furthest,
    /// and the overlay read back from its log.
    pub fn open(dir: &Path) -> Res<Dense> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let vals = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(VALS))
            .map_err(|e| e.to_string())?;
        let mut log = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(dir.join(LOG))
            .map_err(|e| e.to_string())?;
        let mut stride = 0usize;
        if let Ok(b) = std::fs::read(dir.join(META)) {
            if b.len() == 8 {
                stride = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize;
            }
        }
        let len = vals.metadata().map_err(|e| e.to_string())?.len();
        let count = if stride == 0 { 0 } else { len / stride as u64 };
        let (full, present) = match std::fs::read(dir.join(PRESENT)) {
            Ok(b) => {
                let words: Vec<u64> = b
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| u64::from_le_bytes(*c))
                    .collect();
                let mut full = 0u64;
                while full < count
                    && words
                        .get((full / 64) as usize)
                        .is_some_and(|w| w >> (full % 64) & 1 == 1)
                {
                    full += 1;
                }
                (full, words)
            }
            Err(_) => (count, Vec::new()),
        };
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
                stride,
                count,
                full,
                present: Arc::new(present),
                over: Arc::new(over),
            },
            pending: Vec::new(),
            pend_log: Vec::new(),
            present_dirty: false,
        };
        d.remap()?;
        Ok(d)
    }

    /// The values mapped with room past the furthest slot, advised random
    /// as `lmdb-nordahead` advises its file.
    fn remap(&mut self) -> Res<()> {
        let v = &mut self.view;
        if v.stride == 0 || v.count == 0 {
            return Ok(());
        }
        let slots = v.count + MAP_SPARE / v.stride as u64;
        let len = slots as usize * v.stride;
        // SAFETY: the file is this store's alone, a slot is written once,
        // and a slot is read only once the file holds it.
        let m =
            unsafe { MmapOptions::new().len(len).map(&self.vals) }.map_err(|e| e.to_string())?;
        m.advise(memmap2::Advice::Random)
            .map_err(|e| e.to_string())?;
        v.map = Some(Arc::new(m));
        v.mapped = slots;
        Ok(())
    }

    fn mark(&mut self, i: u64) {
        let v = &mut self.view;
        if i == v.full && v.present.is_empty() {
            // The common case, a run: presence is the prefix itself.
            v.full += 1;
            v.count = v.count.max(i + 1);
            return;
        }
        let words = Arc::make_mut(&mut v.present);
        if words.is_empty() {
            // The first write off the run: the prefix in bits.
            words.resize(v.full.div_ceil(64) as usize, 0);
            for j in 0..v.full {
                words[(j / 64) as usize] |= 1 << (j % 64);
            }
        }
        let w = (i / 64) as usize;
        if words.len() <= w {
            words.resize(w + 1, 0);
        }
        words[w] |= 1 << (i % 64);
        v.count = v.count.max(i + 1);
        while v.full < v.count && words[(v.full / 64) as usize] >> (v.full % 64) & 1 == 1 {
            v.full += 1;
        }
        self.present_dirty = true;
    }

    fn flush(&mut self) -> Res<()> {
        if !self.pending.is_empty() {
            self.pending.sort_unstable_by_key(|p| p.0);
            let stride = self.view.stride;
            let mut run: Vec<u8> = Vec::new();
            let mut start = self.pending[0].0;
            for (k, (i, v)) in self.pending.iter().enumerate() {
                if k > 0 && *i != start + (run.len() / stride) as u64 {
                    self.vals
                        .write_all_at(&run, start * stride as u64)
                        .map_err(|e| e.to_string())?;
                    run.clear();
                    start = *i;
                }
                run.extend_from_slice(v);
            }
            self.vals
                .write_all_at(&run, start * stride as u64)
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
        if self.view.count > self.view.mapped {
            self.remap()?;
        }
        Ok(())
    }

    /// Presence, written where it has moved and the slots are not one run.
    fn write_present(&mut self) -> Res<()> {
        if !self.present_dirty {
            return Ok(());
        }
        let path = self.dir.join(PRESENT);
        if self.view.full == self.view.count {
            let _ = std::fs::remove_file(&path);
        } else {
            let mut b = Vec::with_capacity(self.view.present.len() * 8);
            for w in self.view.present.iter() {
                b.extend_from_slice(&w.to_le_bytes());
            }
            let mut f = File::create(&path).map_err(|e| e.to_string())?;
            f.write_all(&b).map_err(|e| e.to_string())?;
            f.sync_data().map_err(|e| e.to_string())?;
        }
        self.present_dirty = false;
        Ok(())
    }
}

impl Drop for Dense {
    fn drop(&mut self) {
        let _ = self.flush();
        let _ = self.write_present();
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
            if self.view.stride == 0 && !v.is_empty() && parse16(k).is_some() {
                self.view.stride = v.len();
                let mut f = File::create(self.dir.join(META)).map_err(|e| e.to_string())?;
                f.write_all(&(v.len() as u64).to_le_bytes())
                    .map_err(|e| e.to_string())?;
                f.sync_data().map_err(|e| e.to_string())?;
            }
            if let Some(n) = parse16(k) {
                if v.len() == self.view.stride
                    && n < MAX_SLOTS
                    && !self.view.has(n)
                    && !self.view.over.contains_key(k)
                {
                    self.pending.push((n, v.to_vec()));
                    self.mark(n);
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
        self.flush()?;
        self.write_present()
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

    fn put(d: &mut Dense, ns: &[u64], tag: u8) {
        let ks: Vec<Vec<u8>> = ns.iter().map(|&n| key(n)).collect();
        let vs: Vec<Vec<u8>> = ns.iter().map(|&n| vec![tag ^ (n as u8); 100]).collect();
        let items: Vec<(&[u8], &[u8])> = ks
            .iter()
            .zip(&vs)
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        d.write_batch(&items).unwrap();
    }

    #[test]
    fn the_dense_probe_reads_back_what_it_was_given_across_a_reopen() {
        let dir = std::env::temp_dir().join(format!("dense-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut d = Dense::open(&dir).unwrap();
            for b in 0..10u64 {
                put(&mut d, &(b * 100..b * 100 + 100).collect::<Vec<_>>(), 1);
            }
            // An update, a key that is not sixteen digits and a value of
            // another length: the overlay.
            let (u, o) = (vec![9u8; 100], vec![3u8; 40]);
            d.write_batch(&[(&key(7), &u), (b"odd", &o), (&key(2000), &o)])
                .unwrap();
            assert_eq!((d.view.count, d.view.full), (1000, 1000));
            assert!(d.view.present.is_empty(), "a run keeps no bits");
            assert_eq!(d.view.over.len(), 3);
        }
        let mut d = Dense::open(&dir).unwrap();
        assert_eq!((d.view.count, d.view.full), (1000, 1000));
        assert_eq!(d.get(&key(0)).unwrap(), 100);
        assert_eq!(d.get(&key(999)).unwrap(), 100);
        assert_eq!(d.get(&key(1000)).unwrap(), 0);
        assert_eq!(d.get(&key(2000)).unwrap(), 40);
        assert_eq!(d.get(b"odd").unwrap(), 40);
        assert_eq!(d.get(b"nope").unwrap(), 0);
        assert_eq!(d.view.slot(7).unwrap(), &[1 ^ 7u8; 100][..]);
        assert_eq!(d.view.over.get(&key(7)).unwrap(), &vec![9u8; 100]);
        // A scan merges the overlay: key 7 once, from the overlay.
        assert_eq!(d.range(&key(5), 5).unwrap(), 500);
        assert_eq!(d.range(&key(998), 5).unwrap(), 100 + 100 + 40 + 40);
        drop(d);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shuffled_load_fills_its_slots_out_of_order() {
        let dir = std::env::temp_dir().join(format!("dense-shuf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let order: Vec<u64> = (0..1000u64).map(|i| (i * 617) % 1000).collect();
        {
            let mut d = Dense::open(&dir).unwrap();
            for c in order[..500].chunks(37) {
                put(&mut d, c, 2);
            }
            assert_eq!(d.view.count, order[..500].iter().max().unwrap() + 1);
            assert!(d.view.full < 500);
            let absent = (0..1000).find(|n| !order[..500].contains(n)).unwrap();
            assert_eq!(d.get(&key(absent)).unwrap(), 0, "a hole reads absent");
            assert_eq!(d.get(&key(order[3])).unwrap(), 100);
            // Half the slots: a scan from zero of a thousand reads 500.
            assert_eq!(d.range(&key(0), 1000).unwrap(), 500 * 100);
            d.sync().unwrap();
        }
        {
            let mut d = Dense::open(&dir).unwrap();
            assert!(d.view.full < 1000, "presence read back");
            for c in order[500..].chunks(41) {
                put(&mut d, c, 2);
            }
            assert_eq!((d.view.count, d.view.full), (1000, 1000));
            assert!(d.view.over.is_empty());
        }
        let d = Dense::open(&dir).unwrap();
        assert_eq!((d.view.count, d.view.full), (1000, 1000));
        for n in [0u64, 1, 499, 500, 999] {
            assert_eq!(d.view.slot(n).unwrap(), &[2 ^ n as u8; 100][..]);
        }
        assert!(!dir.join(PRESENT).exists(), "one run writes no presence");
        drop(d);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
