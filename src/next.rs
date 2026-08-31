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

use std::collections::HashMap;
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

    /// After a seal has been renamed into place and the directory synced,
    /// nothing in the WAL is needed: every record is in the segment.
    fn reset(&mut self) -> Result<()> {
        self.file.set_len(0)?;
        self.file.sync_data()?;
        self.file = OpenOptions::new().write(true).open(&self.path)?;
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

/// Values for one key, varint-length-framed end to end -- the same framing
/// the store's extents use, so a seal is a straight walk.
#[derive(Default)]
struct Run {
    bytes: Vec<u8>,
    count: u64,
}

pub struct Db {
    dir: PathBuf,
    opts: NextOptions,
    wal: Wal,
    mem: HashMap<Box<[u8]>, Run>,
    mem_bytes: usize,
    /// Sequence number of the first record NOT covered by a sealed segment;
    /// persisted implicitly by the WAL reset (a reset WAL means everything
    /// sealed) and in memory between.
    sealed_seq: u64,
    /// Sealed segments, oldest first; read_all visits them in order so a
    /// key's values come back in append order across seals.
    segs: Vec<Blob<MmapBytes>>,
    next_seg: u64,
}

impl Db {
    fn wal_path(dir: &Path) -> PathBuf {
        dir.join("wal")
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
        let wal = Wal::create(&Db::wal_path(dir))?;
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal,
            mem: HashMap::new(),
            mem_bytes: 0,
            sealed_seq: 0,
            segs: Vec::new(),
            next_seg: 0,
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
        let mut mem: HashMap<Box<[u8]>, Run> = HashMap::new();
        let mut mem_bytes = 0usize;
        let wal_path = Db::wal_path(dir);
        let next_seq = Wal::replay(&wal_path, sealed, |k, v| {
            let run = mem.entry(k.into()).or_default();
            put_uvarint(&mut run.bytes, v.len() as u64);
            run.bytes.extend_from_slice(v);
            run.count += 1;
            mem_bytes += k.len() + v.len();
        })?;
        let file = OpenOptions::new().create(true).append(true).open(&wal_path)?;
        let next_seg = seg_ids.last().map_or(0, |&(n, _)| n + 1);
        Ok(Db {
            dir: dir.to_path_buf(),
            opts,
            wal: Wal { file, path: wal_path, seq: next_seq, pending: Vec::new() },
            mem,
            mem_bytes,
            sealed_seq: sealed,
            segs,
            next_seg,
        })
    }

    /// Buffered until `commit`; visible to this handle's reads immediately,
    /// which is the read-your-writes contract `Store::read_all` set.
    pub fn append(&mut self, key: &[u8], value: &[u8]) {
        self.wal.append(key, value);
        let run = self.mem.entry(key.into()).or_default();
        put_uvarint(&mut run.bytes, value.len() as u64);
        run.bytes.extend_from_slice(value);
        run.count += 1;
        self.mem_bytes += key.len() + value.len();
    }

    /// The durability point: WAL append + fdatasync. If the memtable has
    /// crossed the seal threshold, seal after the commit -- after, so the
    /// batch's durability never waits on a segment write.
    pub fn commit(&mut self) -> Result<()> {
        self.wal.commit()?;
        if self.mem_bytes >= self.opts.seal_bytes {
            self.seal()?;
        }
        Ok(())
    }

    /// Write the memtable as one immutable segment in today's store format,
    /// fsync, rename into place, fsync the directory, then reset the WAL.
    pub fn seal(&mut self) -> Result<()> {
        self.wal.commit()?;
        if self.mem.is_empty() {
            return Ok(());
        }
        let tmp = self.dir.join(format!("seal-{:08}.tmp", self.next_seg));
        let _ = std::fs::remove_file(&tmp);
        {
            let store = Store::create(&tmp, Db::segment_opts(&self.opts))
                .map_err(|e| err(&format!("seal create: {e}")))?;
            for (key, run) in self.mem.iter() {
                let mut p = 0usize;
                while p < run.bytes.len() {
                    let len = get_uvarint(&run.bytes, &mut p)
                        .ok_or_else(|| err("memtable framing is malformed"))?
                        as usize;
                    store
                        .append(key, &run.bytes[p..p + len])
                        .map_err(|e| err(&format!("seal append: {e}")))?;
                    p += len;
                }
            }
            store.checkpoint().map_err(|e| err(&format!("seal checkpoint: {e}")))?;
            store.close().map_err(|e| err(&format!("seal close: {e}")))?;
        }
        let path = Db::seg_path(&self.dir, self.next_seg, self.wal.seq);
        std::fs::rename(&tmp, &path)?;
        File::open(&self.dir)?.sync_all()?;
        self.segs.push(Blob::open(MmapBytes::open(&path)?).map_err(|e| err(&format!("{e}")))?);
        self.next_seg += 1;
        self.sealed_seq = self.wal.seq;
        self.mem.clear();
        self.mem_bytes = 0;
        self.wal.reset()?;
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
        if let Some(run) = self.mem.get(key) {
            let mut p = 0usize;
            while p < run.bytes.len() {
                let len = get_uvarint(&run.bytes, &mut p)
                    .ok_or_else(|| err("memtable framing is malformed"))? as usize;
                f(&run.bytes[p..p + len]);
                p += len;
            }
            n += run.count;
        }
        Ok(n)
    }

    pub fn segments(&self) -> usize {
        self.segs.len()
    }

    /// Commit what is pending, seal the rest. Close is a convenience, not a
    /// durability point -- the WAL already made everything durable.
    pub fn close(mut self) -> Result<()> {
        self.wal.commit()?;
        self.seal()
    }
}
