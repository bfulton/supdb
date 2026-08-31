//! The next engine, milestone 1: a WAL is the only mutable thing.
//!
//! `docs/next-engine.md` is the design brief and every load-bearing decision
//! here cites a measurement. A durable commit is one framed append and one
//! fdatasync and nothing else, because f39 measured that shape at 1,191,125
//! ops/s with all engine work removed (F39.1) and today's engine 5.85x below
//! it on work this design deletes (F39.3). Sealed segments are byte-for-byte
//! today's store format, written by the existing `Store` writer and read by
//! `Blob` (`Options { redo_log: false, shards: 1 }` -- the logshed
//! configuration), so everything measured about that read path carries over,
//! browser reader included. There is no checkpoint: sealing is off the
//! commit path, and a store killed before its first seal opens from the WAL
//! alone, which is the brief's P-E and the flip of C3.4.
//!
//! Milestone 1 deliberately leaves out: deletes, scans over segments,
//! per-segment Blooms and range-partitioned compaction (the routing story
//! F40/F41 settled -- the read path here queries every source, which is the
//! unfiltered fan and is priced at 90ns per segment by F38.1), sharded
//! writers (P-D), and group commit. Each arrives with its own experiment.
//!
//! Crash discipline, in order, so every window is survivable:
//! commit = WAL append + fdatasync (the batch is durable or its tail frame
//! fails its CRC and replay stops before it); seal = write the segment to a
//! temp name, fsync it, rename into place, fsync the directory, then reset
//! the WAL -- a crash between any two of those leaves either a WAL that
//! replays the whole memtable or a complete renamed segment plus a WAL
//! whose sealed prefix is skipped by sequence number.

use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Write};
use std::path::{Path, PathBuf};

use crate::block::crc32;
use crate::bytes::MmapBytes;
use crate::{Blob, Options, Store};

fn err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn get_uvarint(buf: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*p)?;
        *p += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

#[derive(Clone)]
pub struct NextOptions {
    /// Memtable bytes that trigger a seal at the next commit. Sealing is off
    /// the commit path in cost accounting but runs on the committing thread
    /// in milestone 1; the brief's "Segment size" question owns this number.
    pub seal_bytes: usize,
    /// Options for the segment writer. Fixed to `redo_log: false, shards: 1`
    /// regardless of what is passed, because a sealed segment is written
    /// once and never reopened for writing -- the logshed finding that a
    /// 4 MiB redo arena in a write-once file is pure waste.
    pub segment: Options,
}

impl Default for NextOptions {
    fn default() -> NextOptions {
        NextOptions { seal_bytes: 64 << 20, segment: Options::default() }
    }
}

/// One WAL frame: `len u32 | crc u32 | seq u64 | klen uvarint | key | value`.
/// `len` covers everything after `crc`; `crc` covers the same bytes. The
/// value's length is `len` minus what precedes it, so values cost no second
/// length field.
const FRAME_HEADER: usize = 8;

struct Wal {
    file: File,
    path: PathBuf,
    /// Sequence of the next record to be written.
    seq: u64,
    /// Buffered frames since the last commit.
    pending: Vec<u8>,
}

impl Wal {
    fn create(path: &Path) -> Result<Wal> {
        let file = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
        Ok(Wal { file, path: path.to_path_buf(), seq: 0, pending: Vec::new() })
    }

    fn append(&mut self, key: &[u8], value: &[u8]) {
        let body_at = self.pending.len() + FRAME_HEADER;
        self.pending.extend_from_slice(&[0u8; FRAME_HEADER]);
        self.pending.extend_from_slice(&self.seq.to_le_bytes());
        put_uvarint(&mut self.pending, key.len() as u64);
        self.pending.extend_from_slice(key);
        self.pending.extend_from_slice(value);
        let body_len = (self.pending.len() - body_at) as u32;
        let crc = crc32(&self.pending[body_at..]);
        self.pending[body_at - 8..body_at - 4].copy_from_slice(&body_len.to_le_bytes());
        self.pending[body_at - 4..body_at].copy_from_slice(&crc.to_le_bytes());
        self.seq += 1;
    }

    /// The durable point: one write, one fdatasync. F39.1 is the budget this
    /// function is held to.
    fn commit(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.pending)?;
        self.file.sync_data()?;
        self.pending.clear();
        Ok(())
    }

    /// Replay a WAL into `apply`, stopping cleanly at a torn tail: a frame
    /// whose length runs past the buffer or whose CRC fails is the crash
    /// point, not corruption to report -- everything durable precedes it.
    /// Records with seq below `from` were sealed and are skipped.
    fn replay(path: &Path, from: u64, mut apply: impl FnMut(&[u8], &[u8])) -> Result<u64> {
        let mut buf = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut buf)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(from),
            Err(e) => return Err(e),
        }
        let mut p = 0usize;
        let mut next_seq = from;
        while buf.len() - p >= FRAME_HEADER {
            let len = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[p + 4..p + 8].try_into().unwrap());
            let body_at = p + FRAME_HEADER;
            let Some(end) = body_at.checked_add(len) else { break };
            if end > buf.len() || len < 8 {
                break;
            }
            let body = &buf[body_at..end];
            if crc32(body) != crc {
                break;
            }
            let seq = u64::from_le_bytes(body[..8].try_into().unwrap());
            let mut q = 8usize;
            let Some(klen) = get_uvarint(body, &mut q) else {
                return Err(err("wal frame key length is malformed"));
            };
            let kend = q
                .checked_add(klen as usize)
                .filter(|&e| e <= body.len())
                .ok_or_else(|| err("wal frame key runs past its frame"))?;
            if seq >= from {
                if seq != next_seq {
                    return Err(err("wal sequence gap: a durable record is missing"));
                }
                apply(&body[q..kend], &body[kend..]);
                next_seq = seq + 1;
            }
            p = end;
        }
        Ok(next_seq)
    }
}

/// The memtable, built so that an append allocates nothing per key or per
/// value: f42's decomposition priced the HashMap<Box<[u8]>, Vec> version at
/// 456k ops/s of the gap to the floor, more than the seal itself (F42.3).
/// Keys live in one bump arena; values live in another as per-key backward
/// chains (each chunk records the previous chunk's offset, and a read or
/// seal walks the chain and reverses it); the table is open-addressed with
/// linear probing at load <= 0.5, resized by rehash of the fixed-size
/// entries only -- key and value bytes never move.
struct MemTable {
    entries: Vec<MemEntry>,
    mask: usize,
    len: usize,
    keys: Vec<u8>,
    vals: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct MemEntry {
    hash: u64,
    key_off: u32,
    key_len: u32,
    /// Offset+1 of this key's newest chunk in `vals`; 0 = vacant slot.
    head: u64,
    count: u64,
}

const NO_CHUNK: u64 = u64::MAX;

fn mem_hash(key: &[u8]) -> u64 {
    // FNV-1a, then a splitmix finish; the std SipHash was part of what f42
    // priced.
    let mut h = 0xcbf29ce484222325u64;
    for &b in key {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h | 1 // 0 marks a vacant slot
}

impl MemTable {
    fn new() -> MemTable {
        MemTable {
            entries: vec![MemEntry::default(); 1024],
            mask: 1023,
            len: 0,
            keys: Vec::new(),
            vals: Vec::new(),
        }
    }

    fn key_of<'a>(keys: &'a [u8], e: &MemEntry) -> &'a [u8] {
        &keys[e.key_off as usize..(e.key_off + e.key_len) as usize]
    }

    fn grow(&mut self) {
        let cap = self.entries.len() * 2;
        let mut entries = vec![MemEntry::default(); cap];
        let mask = cap - 1;
        for e in self.entries.iter().filter(|e| e.hash != 0) {
            let mut i = (e.hash as usize) & mask;
            while entries[i].hash != 0 {
                i = (i + 1) & mask;
            }
            entries[i] = *e;
        }
        self.entries = entries;
        self.mask = mask;
    }

    fn append(&mut self, key: &[u8], value: &[u8]) {
        if (self.len + 1) * 2 > self.entries.len() {
            self.grow();
        }
        let hash = mem_hash(key);
        let mut i = (hash as usize) & self.mask;
        loop {
            let e = self.entries[i];
            if e.hash == 0 {
                let key_off = self.keys.len() as u32;
                self.keys.extend_from_slice(key);
                let head = self.push_chunk(NO_CHUNK, value);
                self.entries[i] = MemEntry {
                    hash,
                    key_off,
                    key_len: key.len() as u32,
                    head: head + 1,
                    count: 1,
                };
                self.len += 1;
                return;
            }
            if e.hash == hash && MemTable::key_of(&self.keys, &e) == key {
                let head = self.push_chunk(e.head - 1, value);
                self.entries[i].head = head + 1;
                self.entries[i].count += 1;
                return;
            }
            i = (i + 1) & self.mask;
        }
    }

    fn push_chunk(&mut self, prev: u64, value: &[u8]) -> u64 {
        let off = self.vals.len() as u64;
        self.vals.extend_from_slice(&prev.to_le_bytes());
        put_uvarint(&mut self.vals, value.len() as u64);
        self.vals.extend_from_slice(value);
        off
    }

    /// Chunk offsets for one entry, oldest first.
    fn chain(&self, e: &MemEntry) -> Vec<usize> {
        let mut offs = Vec::with_capacity(e.count as usize);
        let mut at = e.head - 1;
        while at != NO_CHUNK {
            offs.push(at as usize);
            at = u64::from_le_bytes(self.vals[at as usize..at as usize + 8].try_into().unwrap());
        }
        offs.reverse();
        offs
    }

    fn value_at(&self, off: usize) -> &[u8] {
        let mut p = off + 8;
        let len = get_uvarint(&self.vals, &mut p).expect("memtable framing") as usize;
        &self.vals[p..p + len]
    }

    fn get(&self, key: &[u8]) -> Option<&MemEntry> {
        let hash = mem_hash(key);
        let mut i = (hash as usize) & self.mask;
        loop {
            let e = &self.entries[i];
            if e.hash == 0 {
                return None;
            }
            if e.hash == hash && MemTable::key_of(&self.keys, e) == key {
                return Some(e);
            }
            i = (i + 1) & self.mask;
        }
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

pub struct Db {
    dir: PathBuf,
    opts: NextOptions,
    wal: Wal,
    wal_id: u64,
    mem: MemTable,
    mem_bytes: usize,
    /// Sealed segments, oldest first; read_all visits them in order so a
    /// key's values come back in append order across seals.
    segs: Vec<Blob<MmapBytes>>,
    next_seg: u64,
    /// A seal in flight: the frozen memtable stays readable (it is newer
    /// than every segment and older than `mem`) while a thread writes it
    /// out; `join_seal` collects the finished segment.
    frozen: Option<std::sync::Arc<MemTable>>,
    sealing: Option<std::thread::JoinHandle<Result<PathBuf>>>,
    /// Sorted keys of the unsealed sources (memtable + frozen), built lazily
    /// by `scan` and reused until a write or a seal changes what is
    /// unsealed. Without this, every scan walked the whole memtable: the
    /// ext-kv scan phase spent 15 minutes a rep in that walk, twice -- once
    /// through the live table and once through the frozen one.
    scan_keys: std::cell::RefCell<Option<(u64, Vec<Vec<u8>>)>>,
}

impl Db {
    /// WAL files are numbered and rotate at each seal: the sealing thread
    /// owns the old file and deletes it once its segment is renamed into
    /// place, while commits continue into the next file. Replay walks them
    /// in id order; sequence numbers are continuous across the boundary.
    fn wal_path(dir: &Path, id: u64) -> PathBuf {
        dir.join(format!("wal-{id:08}"))
    }

    /// The end-of-covered-sequence rides the file name so the rename that
    /// publishes a segment also publishes, atomically, which WAL records it
    /// covers. A crash between the rename and the WAL reset then leaves a
    /// WAL whose covered prefix is skipped by sequence on replay instead of
    /// replayed into duplicates.
    fn seg_path(dir: &Path, n: u64, end_seq: u64) -> PathBuf {
        dir.join(format!("seg-{n:08}-{end_seq:016}.sup"))
    }

    fn segment_opts(opts: &NextOptions) -> Options {
        Options { redo_log: false, shards: 1, ..opts.segment.clone() }
    }

    pub fn create(dir: &Path, opts: NextOptions) -> Result<Db> {
        std::fs::create_dir_all(dir)?;
        let wal = Wal::create(&Db::wal_path(dir, 0))?;
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal,
            wal_id: 0,
            mem: MemTable::new(),
            mem_bytes: 0,
            segs: Vec::new(),
            next_seg: 0,
            frozen: None,
            sealing: None,
            scan_keys: std::cell::RefCell::new(None),
        })
    }

    /// Open from the directory alone. Segments are complete by construction
    /// (they were renamed into place after their fsync); the WAL replays
    /// whatever outlived the last seal, torn tail tolerated. A directory
    /// with no segments and only a WAL is a store killed before its first
    /// seal, and it opens -- the brief's P-E.
    pub fn open(dir: &Path, opts: NextOptions) -> Result<Db> {
        let mut seg_ids: Vec<(u64, u64)> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix("seg-").and_then(|s| s.strip_suffix(".sup")) {
                let (id, end) =
                    rest.split_once('-').ok_or_else(|| err("segment file name is malformed"))?;
                seg_ids.push((
                    id.parse().map_err(|_| err("segment file name is malformed"))?,
                    end.parse().map_err(|_| err("segment file name is malformed"))?,
                ));
            }
        }
        seg_ids.sort_unstable();
        let mut segs = Vec::with_capacity(seg_ids.len());
        for &(id, end) in &seg_ids {
            let blob = Blob::open(MmapBytes::open(&Db::seg_path(dir, id, end))?)
                .map_err(|e| err(&format!("segment {id}: {e}")))?;
            segs.push(blob);
        }
        // Everything at or past the newest segment's end-sequence outlived
        // the last seal; everything before it is covered and skipped, which
        // is what makes the rename-then-reset crash window safe.
        let sealed = seg_ids.last().map_or(0, |&(_, end)| end);
        let mut wal_ids: Vec<u64> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_prefix("wal-") {
                wal_ids.push(id.parse().map_err(|_| err("wal file name is malformed"))?);
            }
        }
        wal_ids.sort_unstable();
        let mut mem = MemTable::new();
        let mut mem_bytes = 0usize;
        let mut from = sealed;
        for &id in &wal_ids {
            from = Wal::replay(&Db::wal_path(dir, id), from, |k, v| {
                mem.append(k, v);
                mem_bytes += k.len() + v.len();
            })?;
        }
        // A crash can leave a fully-covered WAL the sealing thread never
        // deleted; every record in it was skipped above, and it is garbage
        // now. Keep only the newest file and continue appending to it.
        for &id in wal_ids.iter().rev().skip(1) {
            let _ = std::fs::remove_file(Db::wal_path(dir, id));
        }
        let wal_id = wal_ids.last().copied().unwrap_or(0);
        let wal_path = Db::wal_path(dir, wal_id);
        let file = OpenOptions::new().create(true).append(true).open(&wal_path)?;
        let next_seg = seg_ids.last().map_or(0, |&(n, _)| n + 1);
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal: Wal { file, path: wal_path, seq: from, pending: Vec::new() },
            wal_id,
            mem,
            mem_bytes,
            segs,
            next_seg,
            frozen: None,
            sealing: None,
            scan_keys: std::cell::RefCell::new(None),
        })
    }

    /// Buffered until `commit`; visible to this handle's reads immediately,
    /// which is the read-your-writes contract `Store::read_all` set.
    pub fn append(&mut self, key: &[u8], value: &[u8]) {
        self.wal.append(key, value);
        self.mem.append(key, value);
        self.mem_bytes += key.len() + value.len();
    }

    /// The durability point: WAL append + fdatasync. If the memtable has
    /// crossed the seal threshold, seal after the commit -- after, so the
    /// batch's durability never waits on a segment write.
    pub fn commit(&mut self) -> Result<()> {
        self.wal.commit()?;
        if self.sealing.as_ref().is_some_and(|h| h.is_finished()) {
            self.join_seal()?;
        }
        if self.mem_bytes >= self.opts.seal_bytes {
            self.seal()?;
        }
        Ok(())
    }

    /// Freeze the memtable, rotate the WAL, and hand the frozen table to a
    /// thread that writes it as one immutable segment in today's store
    /// format -- fsync, rename into place (the name carrying the covered
    /// end-sequence), fsync the directory, delete the rotated-out WAL.
    /// Commits continue into the new WAL while it runs; at most one seal is
    /// in flight, so a second trigger joins the first (backpressure).
    pub fn seal(&mut self) -> Result<()> {
        self.wal.commit()?;
        if self.mem.is_empty() {
            return Ok(());
        }
        self.join_seal()?;
        let frozen = std::sync::Arc::new(std::mem::replace(&mut self.mem, MemTable::new()));
        self.mem_bytes = 0;
        let old_wal = std::mem::replace(
            &mut self.wal,
            Wal::create(&Db::wal_path(&self.dir, self.wal_id + 1))?,
        );
        self.wal_id += 1;
        self.wal.seq = old_wal.seq;
        let seg_id = self.next_seg;
        self.next_seg += 1;
        let dir = self.dir.clone();
        let opts = Db::segment_opts(&self.opts);
        let end_seq = old_wal.seq;
        let old_wal_path = old_wal.path.clone();
        drop(old_wal);
        let mem = frozen.clone();
        self.frozen = Some(frozen);
        self.sealing = Some(std::thread::spawn(move || {
            let tmp = dir.join(format!("seal-{seg_id:08}.tmp"));
            let _ = std::fs::remove_file(&tmp);
            {
                let store = Store::create(&tmp, opts)
                    .map_err(|e| err(&format!("seal create: {e}")))?;
                for e in mem.entries.iter().filter(|e| e.hash != 0) {
                    let key = MemTable::key_of(&mem.keys, e);
                    for off in mem.chain(e) {
                        store
                            .append(key, mem.value_at(off))
                            .map_err(|e| err(&format!("seal append: {e}")))?;
                    }
                }
                store.checkpoint().map_err(|e| err(&format!("seal checkpoint: {e}")))?;
                store.close().map_err(|e| err(&format!("seal close: {e}")))?;
            }
            let path = Db::seg_path(&dir, seg_id, end_seq);
            std::fs::rename(&tmp, &path)?;
            File::open(&dir)?.sync_all()?;
            let _ = std::fs::remove_file(&old_wal_path);
            Ok(path)
        }));
        Ok(())
    }

    /// Commit, seal, and wait for the segment: the full drain, for a caller
    /// entering a read-heavy phase. `seal` alone leaves the frozen memtable
    /// readable until an eventual join, which is right for a writer that
    /// keeps committing and wrong for one that stops: the ext-kv adapter
    /// sealed without draining and every scan walked the 550k-key frozen
    /// table for the rest of the phase -- the same artifact the seal was
    /// supposed to remove, back through the side door.
    pub fn flush(&mut self) -> Result<()> {
        self.wal.commit()?;
        self.seal()?;
        self.join_seal()
    }

    /// Collect a finished (or in-flight) seal: join the thread, open its
    /// segment, retire the frozen memtable.
    fn join_seal(&mut self) -> Result<()> {
        let Some(handle) = self.sealing.take() else {
            return Ok(());
        };
        let path = handle.join().map_err(|_| err("seal thread panicked"))??;
        self.segs
            .push(Blob::open(MmapBytes::open(&path)?).map_err(|e| err(&format!("{e}")))?);
        self.frozen = None;
        Ok(())
    }

    /// Every value for `key`, in append order across seals: segments oldest
    /// first, memtable last. Milestone 1 queries every source -- the
    /// unfiltered fan F38.1 prices at 90ns per segment; routing lands with
    /// the compaction milestone.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let mut n = 0u64;
        for seg in &self.segs {
            n += seg
                .read_all(key, &mut f)
                .map_err(|e| err(&format!("segment read: {e}")))?;
        }
        if let Some(fr) = &self.frozen {
            if let Some(e) = fr.get(key) {
                for off in fr.chain(e) {
                    f(fr.value_at(off));
                }
                n += e.count;
            }
        }
        if let Some(e) = self.mem.get(key) {
            for off in self.mem.chain(e) {
                f(self.mem.value_at(off));
            }
            n += e.count;
        }
        Ok(n)
    }

    /// Ordered scan from `from`, at most `limit` distinct keys, values in
    /// append order within each key. Milestone 3's merge is the unrouted fan
    /// on the scan axis: every segment contributes up to `limit` candidate
    /// keys via its own index walk, the memtables contribute theirs, and the
    /// union is sorted and re-read through `read_all`. Range-partitioned
    /// compaction is what will make this cost one segment instead of all of
    /// them; until then this is priced as what it is.
    pub fn scan<F: FnMut(&[u8], &[u8])>(
        &self,
        from: &[u8],
        limit: usize,
        mut f: F,
    ) -> Result<usize> {
        let gen = self.wal.seq ^ (self.next_seg << 48) ^ ((self.frozen.is_some() as u64) << 63);
        {
            let mut cache = self.scan_keys.borrow_mut();
            let stale = cache.as_ref().is_none_or(|(g, _)| *g != gen);
            if stale {
                let mut all: Vec<Vec<u8>> = Vec::with_capacity(self.mem.len);
                let mut take = |mem: &MemTable| {
                    for e in mem.entries.iter().filter(|e| e.hash != 0) {
                        all.push(MemTable::key_of(&mem.keys, e).to_vec());
                    }
                };
                if let Some(fr) = &self.frozen {
                    take(fr);
                }
                take(&self.mem);
                all.sort_unstable();
                all.dedup();
                *cache = Some((gen, all));
            }
        }
        let cache = self.scan_keys.borrow();
        let unsealed = &cache.as_ref().expect("scan snapshot").1;
        let start = unsealed.partition_point(|k| k.as_slice() < from);
        let mut keys: Vec<Vec<u8>> =
            unsealed[start..start + limit.min(unsealed.len() - start)].to_vec();
        for seg in &self.segs {
            seg.scan_counts(from, limit, |k, _| {
                keys.push(k.to_vec());
                true
            })
            .map_err(|e| err(&format!("segment scan: {e}")))?;
        }
        keys.sort_unstable();
        keys.dedup();
        keys.truncate(limit);
        for key in &keys {
            self.read_all(key, |v| f(key, v))?;
        }
        Ok(keys.len())
    }

    pub fn segments(&self) -> usize {
        self.segs.len() + usize::from(self.sealing.is_some())
    }

    /// Commit what is pending, seal the rest. Close is a convenience, not a
    /// durability point -- the WAL already made everything durable.
    pub fn close(mut self) -> Result<()> {
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<()> {
        self.flush()
    }
}

/// Dropping a `Db` without `close` emulates a crash in tests, but the seal
/// thread is not a crash casualty in-process: it would keep mutating the
/// directory under whoever reopens it. Join it; the WAL it rotated out
/// still covers everything either way, so crash semantics are unchanged.
impl Drop for Db {
    fn drop(&mut self) {
        if let Some(h) = self.sealing.take() {
            let _ = h.join();
        }
    }
}
