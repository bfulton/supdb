use crate::block::{self, BlockBuilder, BlockCache, BlockLoc};
use crate::freelist::{capacity_for, FreeList};
use crate::readers;
use crate::index::{get_uvarint, put_uvarint, Ext, Extents};
use crate::keytable::KeyTable;

use memmap2::{Mmap, MmapMut};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Result, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

const MAGIC: u64 = 0x5355_5044_4200_0001;

/// Two superblock slots live in the first sector-pair of the file, and a
/// checkpoint alternates between them.
///
/// The store previously wrote its index only at close, so a crash lost not
/// just recent appends but the entire file: without a footer nothing could be
/// read back at all. Alternating slots make a checkpoint atomic in the way
/// that matters -- a torn write can damage at most the slot being written,
/// and the other still describes a complete, older state. Recovery picks the
/// valid slot with the higher generation.
const SUPER: u64 = 4096;
const SLOT: u64 = 512;

#[derive(Clone, Copy, Default, Debug)]
struct Super {
    generation: u64,
    /// The oldest generation whose history is still intact.
    ///
    /// Reclaiming a superseded extent hands its space back for reuse, which
    /// silently invalidates every earlier index that still points at it.
    /// Recording where that happened lets a read of an older state fail
    /// loudly instead of returning whatever now occupies those bytes -- the
    /// one outcome worse than refusing.
    history_from: u64,
    /// Wall-clock milliseconds when this checkpoint was taken.
    ///
    /// The generation is the authoritative identity of a state -- exact,
    /// monotonic, independent of any clock. The timestamp is how people
    /// actually want to ask the question, and it is advisory: a clock can be
    /// stepped backwards, so it is recorded as max(previous, now) to keep the
    /// chain searchable in time order as well as in generation order.
    timestamp: u64,
    key_off: u64,
    key_stored: u64,
    key_uncompressed: u64,
    blk_off: u64,
    blk_stored: u64,
    blk_uncompressed: u64,
    reuse_off: u64,
    reuse_stored: u64,
    reuse_uncompressed: u64,
    high_water: u64,
}

impl Super {
    fn fields(&self) -> [u64; 13] {
        [self.generation, self.history_from, self.timestamp, self.key_off,
         self.key_stored, self.key_uncompressed, self.blk_off, self.blk_stored,
         self.blk_uncompressed, self.reuse_off, self.reuse_stored,
         self.reuse_uncompressed, self.high_water]
    }

    /// FNV-1a over the fields and the magic. Enough to reject a torn or never
    /// written slot; this guards against truncation, not tampering.
    fn checksum(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in self.fields().iter().chain(std::iter::once(&MAGIC)) {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        }
        h
    }

    fn encode(&self) -> [u8; 120] {
        let mut out = [0u8; 120];
        for (i, v) in self.fields().iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        out[104..112].copy_from_slice(&MAGIC.to_le_bytes());
        out[112..120].copy_from_slice(&self.checksum().to_le_bytes());
        out
    }

    fn decode(buf: &[u8]) -> Option<Super> {
        if buf.len() < 120 {
            return None;
        }
        let f: Vec<u64> = (0..13)
            .map(|i| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap()))
            .collect();
        if u64::from_le_bytes(buf[104..112].try_into().unwrap()) != MAGIC {
            return None;
        }
        let s = Super {
            generation: f[0], history_from: f[1], timestamp: f[2], key_off: f[3],
            key_stored: f[4], key_uncompressed: f[5], blk_off: f[6],
            blk_stored: f[7], blk_uncompressed: f[8], reuse_off: f[9],
            reuse_stored: f[10], reuse_uncompressed: f[11], high_water: f[12],
        };
        if u64::from_le_bytes(buf[112..120].try_into().unwrap()) != s.checksum() {
            return None;
        }
        Some(s)
    }
}

/// When space belonging to a superseded value may be handed out again.
///
/// Releasing a block only makes its space available; reuse is what actually
/// destroys the older state that still points there. These are the points on
/// that spectrum, from cheapest to safest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reclaim {
    /// Reuse immediately. Cheapest and smallest, and correct only when nothing
    /// reads the store while it is written -- a reader that has opened but not
    /// finished can have its data written over. Measured here, this failed 41%
    /// of reads under a concurrent reader.
    Now,
    /// Reuse once no reader that could see it remains. Readers publish the
    /// generation they hold in a shared table and the writer stays behind the
    /// oldest of them, so this costs only the space live readers are actually
    /// pinning. The default: it is exact rather than estimated.
    AfterReads,
    /// Reuse once this many checkpoints have passed. An approximation of
    /// AfterReads for readers that cannot register -- another process on a
    /// filesystem without shared memory semantics, say. A reader slower than
    /// the delay still fails loudly rather than reading the wrong bytes.
    AfterDelay(u64),
    /// Accumulate freed space and reuse none of it until the store is closed,
    /// where defragmentation and truncation reclaim it in one pass. For a
    /// writer that wants no reuse hazard at all while it runs and does not
    /// need the space back until it stops.
    OnClose,
    /// Never reuse, and never release. Earlier states stay readable, so this
    /// is what read-as-of requires; the file keeps every version ever written.
    Never,
}

impl Reclaim {
    /// Whether superseded extents are handed back at all.
    fn releases(&self) -> bool {
        !matches!(self, Reclaim::Never)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Target size of a compression block. Bigger compresses better and costs
    /// more to decompress on a point read; this is the size/read dial.
    pub block_size: usize,
    /// Ceiling on bytes held in unsealed per-key buffers, across all shards.
    /// A shallow workload has more keys in flight than fit, so this is what
    /// decides how large extents get before they are forced out.
    pub buffer_bytes: usize,
    pub compress: bool,
    /// Independent write shards.
    ///
    /// More of them means each key table is smaller and its working set is
    /// likelier to stay in cache -- the write path's cost grows with the
    /// number of distinct keys, so this is the cheapest lever on that slope.
    /// It also spreads lock contention across writers.
    pub shards: usize,
    pub cache_blocks: usize,
    /// Merge a key's extents once it has this many, inline on the append that
    /// crosses the line.
    ///
    /// There is no background thread and no stop-the-world pass: a batch
    /// compaction measured 18.7 seconds, which is a stall whether or not a
    /// separate thread runs it. Doing the merge on the writer that caused the
    /// fragmentation keeps the cost proportional to the damage and spreads it
    /// across the run. A key that never fragments is never rewritten, and the
    /// data is rewritten about log_threshold(N) times over its life rather
    /// than on a levelled schedule.
    ///
    /// Contiguity on ingest is bought with buffer memory, and the exchange
    /// rate is steep: at a 64 MB buffer a deep key fragments into ~21 extents
    /// and warm reads fall to 1,168/s, against 38,509/s at 1 GB. Since no
    /// buffer can cover a dataset larger than memory, fragmentation has to be
    /// repaired rather than prevented.
    ///
    /// Doing it per key and only past a threshold is what keeps it cheap: the
    /// work scales with how badly a key fragmented, not with total data
    /// volume, so a key that never fragmented is never rewritten. That is the
    /// distinction from LSM levelled compaction, which rewrites everything on
    /// a schedule regardless of need.
    pub merge_threshold: usize,
    pub reclaim: Reclaim,
    /// An extent at least this large gets a block to itself.
    ///
    /// Packing exists to give the compressor a window when a key holds only a
    /// kilobyte. A key that already holds tens of kilobytes needs no help, and
    /// packing it costs dearly on reads: sharing 64 KB blocks meant
    /// decompressing 128 KB to extract 47 KB, which measured as 9,377 warm
    /// reads/s against Uppend's 57,247. Above this size a key's run stands
    /// alone and a read touches nothing but that key's bytes.
    pub solo_threshold: usize,
    pub chunk_size: usize,
    /// Chunk size for solo blocks. A deep key's whole run is decompressed on a
    /// full read regardless of chunking, so warm reads there are flat across
    /// this value while compression is not: the larger window is close to free
    /// on space. Only read_last pays for it.
    pub solo_chunk_size: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            block_size: 64 * 1024,
            buffer_bytes: 512 * 1024 * 1024,
            compress: true,
            shards: 64,
            cache_blocks: 4096,
            solo_threshold: 16 * 1024,
            chunk_size: 1024,
            solo_chunk_size: block::CHUNK,
            merge_threshold: 4,
            reclaim: Reclaim::AfterReads,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub blocks: u64,
    pub bytes_written: u64,
    pub index_bytes: u64,
    pub keys: u64,
    pub merges: u64,
    pub free_bytes: u64,
    pub reused: u64,
    pub reused_bytes: u64,
}

// ---------------------------------------------------------------- writing --

struct Appender {
    file: File,
    off: u64,
    blocks: Vec<BlockLoc>,
    /// Extents still referencing each block. A block nobody references is
    /// garbage and its space can be handed back.
    live: Vec<u32>,
    free: FreeList,
    /// The reader table, in the reserved first page.
    table: MmapMut,
    generation: u64,
    timestamp: u64,
    reuse_log: Vec<(u64, u32, u64)>,
    last_index: Option<BlockLoc>,
}

impl Appender {
    fn retain(&mut self, id: u32) {
        self.live[id as usize] += 1;
    }

    /// Drop one reference; punch the block out when the last one goes.
    fn release(&mut self, id: u32) -> Result<()> {
        let n = &mut self.live[id as usize];
        if *n > 0 {
            *n -= 1;
        }
        if self.live[id as usize] == 0 {
            let loc = self.blocks[id as usize];
            self.free.release(loc.off, loc.cap, self.generation);
        }
        Ok(())
    }

    /// Read one extent back out, decompressing its block if needed.
    fn read_extent(&self, e: Ext) -> Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let loc = self.blocks[e.block as usize];
        let mut raw = vec![0u8; loc.stored as usize];
        self.file.read_exact_at(&mut raw, loc.off)?;
        // a chunked block is a chunk directory plus per-chunk streams, not a
        // single lz4 stream, so it has to be decoded as such
        let full = if loc.is_plain() {
            raw
        } else if loc.chunked {
            let mut out = vec![0u8; loc.uncompressed as usize];
            block::read_chunked_range(&raw, loc.uncompressed as usize, 0, loc.uncompressed as usize, &mut out)?;
            out
        } else {
            block::decompress(&raw, loc.uncompressed as usize)?
        };
        Ok(full[e.off as usize..(e.off + e.len) as usize].to_vec())
    }
}

impl Appender {
    /// Write one block and return its id. Compression is attempted and kept
    /// only if it actually shrinks the block; a block that does not compress
    /// is stored verbatim so that reads of it can be served straight from the
    /// mapping with no copy at all.
    /// The generation below which freed space may be reused.
    ///
    /// A live reader's generation is authoritative: nothing it can still see
    /// is handed out, however long it takes. The grace window only applies
    /// when no reader is registered, covering the moment between a reader
    /// deciding to open and getting its slot.
    fn reuse_floor(&self, policy: Reclaim) -> u64 {
        let live = || {
            let t = unsafe { readers::slots(self.table.as_ptr()) };
            readers::oldest(t)
        };
        match policy {
            Reclaim::Now => u64::MAX,
            // Never hand out anything a live reader can still see. With none
            // registered the floor is still the current generation, not past
            // it: merging frees blocks the newest checkpoint points at, and a
            // reader may have opened on that checkpoint a moment ago and not
            // yet claimed its slot.
            Reclaim::AfterReads => live().unwrap_or(self.generation),
            Reclaim::AfterDelay(n) => {
                let by_delay = self.generation.saturating_sub(n);
                live().map_or(by_delay, |o| o.min(by_delay))
            }
            // nothing is reusable while the store runs
            Reclaim::OnClose | Reclaim::Never => 0,
        }
    }

    fn write_block(&mut self, payload: &[u8], compress: bool, solo: bool, chunk: usize, policy: Reclaim) -> Result<u32> {
        // Every compressed block is chunked, packed ones included. A packed
        // block holds many keys' short runs, so reading one key out of a
        // 64 KiB block compressed as a unit meant decompressing 64 KiB to
        // reach 960 bytes -- 68x read amplification, and the reason the wide
        // shape read at 29,706/s against RocksDB's 123,153. Chunking bounds
        // that to the one chunk the extent lands in.
        let chunked = compress && payload.len() > chunk;
        let stored: Option<Vec<u8>> = if chunked {
            let c = block::write_chunked_sz(payload, chunk);
            if c.len() < payload.len() { Some(c) } else { None }
        } else if compress {
            block::compress(payload)
        } else {
            None
        };
        let chunked = chunked && stored.is_some();
        let bytes: &[u8] = stored.as_deref().unwrap_or(payload);
        use std::os::unix::fs::FileExt;
        let len = bytes.len() as u32;
        let floor = self.reuse_floor(policy);
        let (off, cap) = match self.free.take_below(len, floor) {
            // A slot freed earlier, now reused. This is the moment history is
            // actually lost -- releasing a block only made its space
            // available, and until something is written over it every earlier
            // index that pointed there is still correct. Recording the range
            // and the generation lets a later read of an older state fail only
            // if it actually touches these bytes.
            Some(slot) => {
                self.reuse_log.push((slot.0, slot.1, self.generation));
                slot
            }
            None => {
                let cap = capacity_for(len);
                let off = self.off;
                self.off += cap as u64;
                (off, cap)
            }
        };
        let loc = BlockLoc {
            off,
            stored: len,
            uncompressed: payload.len() as u32,
            cap,
            chunked,
            solo,
        };
        self.file.write_all_at(bytes, off)?;
        self.blocks.push(loc);
        self.live.push(0);
        Ok((self.blocks.len() - 1) as u32)
    }
}

/// A key's extent while it is still being built.
#[derive(Default)]
struct Pending {
    buf: Vec<u8>,
    /// Offset of the most recently appended record within `buf`.
    last: u32,
    /// Extents this pending value replaces, to be released when it is sealed
    /// and only if the store is reclaiming.
    supersedes: Vec<Ext>,
    /// True when this came from put(): the sealed extent replaces the key's
    /// extents rather than being added to them.
    replaces: bool,
}

struct Shard {
    merges: u64,
    /// One table holding both a key's sealed extents and the value still
    /// buffered for it, so a put probes once rather than twice.
    keys: KeyTable<Pending>,
    pending_bytes: usize,
    builder: BlockBuilder,
    /// Extents already placed in the current block, awaiting its id.
    members: Vec<(u32, u32, u32, u32, bool)>,
}

pub struct Store {
    shards: Vec<Mutex<Shard>>,
    appender: Mutex<Appender>,
    opts: Options,
    path: PathBuf,
}

impl Store {
    pub fn create(path: &Path, opts: Options) -> Result<Store> {
        // opened for reading too: compaction reads written extents back
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let shards = (0..opts.shards)
            .map(|_| {
                Mutex::new(Shard {
                    merges: 0,
                    keys: KeyTable::new(),
                    pending_bytes: 0,
                    builder: BlockBuilder::new(opts.block_size),
                    members: Vec::new(),
                })
            })
            .collect();
        // the first page must exist before it can be mapped
        file.set_len(SUPER)?;
        let table = unsafe { MmapMut::map_mut(&file)? };
        Ok(Store {
            shards,
            appender: Mutex::new(Appender {
                table,
                file,
                // the first page is reserved for the two superblock slots
                off: SUPER,
                blocks: Vec::new(),
                live: Vec::new(),
                free: FreeList::new(),
                generation: 0,
                timestamp: 0,
                reuse_log: Vec::new(),
                last_index: None,
            }),
            opts,
            path: path.to_path_buf(),
        })
    }

    fn shard_of(&self, key: &[u8]) -> usize {
        // FxHash-style multiply-xor; keys here are short and highly similar,
        // so a cheap mixer that still moves the low bits is what is wanted.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in key {
            h ^= b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        ((h >> 32) as usize) % self.shards.len()
    }

    /// Append one value to a key. This is the path that has to stay fast --
    /// it is the axis the whole design is built to win.
    pub fn append(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let si = self.shard_of(key);
        let mut sh = self.shards[si].lock().unwrap();
        let grew = {
            let e = sh.keys.get_or_insert(key);
            let p = e.pending.get_or_insert_with(Pending::default);
            let before = p.buf.len();
            p.last = before as u32;
            put_uvarint(&mut p.buf, value.len() as u64);
            p.buf.extend_from_slice(value);
            p.buf.len() - before
        };
        sh.pending_bytes += grew;

        if sh.pending_bytes >= self.opts.buffer_bytes / self.shards.len() {
            self.seal_shard(&mut sh)?;
        }
        Ok(())
    }

    /// Seal every buffered extent in this shard.
    ///
    /// The batch is sorted by key on the way out. It is already being copied,
    /// so ordering it costs almost nothing here and yields sorted runs --
    /// which is what an ordered scan would need, and what cannot be recovered
    /// later without rewriting the data.
    fn seal_shard(&self, sh: &mut Shard) -> Result<()> {
        if sh.pending_bytes == 0 {
            return Ok(());
        }
        let mut batch: Vec<(u32, Pending)> = sh.keys.take_pending();
        sh.pending_bytes = 0;
        // sorted by the keys the indices point at, with no key copied
        sh.keys.sort_by_key(&mut batch);

        for (idx, p) in batch {
            // Whatever this extent replaces stops being referenced now. For a
            // replacement the superseded extents are whatever the index still
            // holds for the key, resolved here rather than on the hot path.
            if self.opts.reclaim.releases() {
                let superseded: Vec<Ext> = if p.replaces {
                    sh.keys.entry_at(idx).extents.as_slice().to_vec()
                } else {
                    p.supersedes.clone()
                };
                if !superseded.is_empty() {
                    let mut ap = self.appender.lock().unwrap();
                    for e in &superseded {
                        ap.release(e.block)?;
                    }
                }
            }
            if p.buf.len() >= self.opts.solo_threshold {
                // big enough to compress on its own; giving it a private block
                // means a read of this key decompresses only this key
                let id = {
                    let mut ap = self.appender.lock().unwrap();
                    ap.write_block(&p.buf, self.opts.compress, true, self.opts.solo_chunk_size, self.opts.reclaim)?
                };
                let len = p.buf.len() as u32;
                self.appender.lock().unwrap().retain(id);
                let ext = Ext { block: id, off: 0, len, last: p.last };
                {
                    let entry = sh.keys.entry_at(idx);
                    if p.replaces {
                        entry.extents = Extents::One(ext);
                    } else {
                        entry.extents.push(ext);
                    }
                }
                self.merge_key(sh, idx)?;
                continue;
            }
            if sh.builder.would_overflow(p.buf.len()) {
                self.flush_builder(sh)?;
            }
            let off = sh.builder.push(&p.buf);
            sh.members.push((idx, off, p.buf.len() as u32, p.last, p.replaces));
        }
        Ok(())
    }

    /// Write the current block and only then record where each extent landed:
    /// a block's id is not known until it is placed in the file.
    fn flush_builder(&self, sh: &mut Shard) -> Result<()> {
        if sh.builder.is_empty() {
            return Ok(());
        }
        let payload = sh.builder.take();
        let id = {
            let mut ap = self.appender.lock().unwrap();
            ap.write_block(&payload, self.opts.compress, false, self.opts.chunk_size, self.opts.reclaim)?
        };
        {
            let mut ap = self.appender.lock().unwrap();
            for _ in 0..sh.members.len() {
                ap.retain(id);
            }
        }
        let touched: Vec<u32> = sh
            .members
            .drain(..)
            .map(|(idx, off, len, last, replaces)| {
                let ext = Ext { block: id, off, len, last };
                let entry = sh.keys.entry_at(idx);
                if replaces {
                    entry.extents = Extents::One(ext);
                } else {
                    entry.extents.push(ext);
                }
                idx
            })
            .collect();
        for idx in touched {
            self.merge_key(sh, idx)?;
        }
        Ok(())
    }

    /// Replace a key's values with a single new value.
    ///
    /// The new version is appended and the index is repointed at it; the old
    /// extents are released only under Retain::Reclaim. Nothing is ever
    /// overwritten in place, so an earlier checkpoint that still points at the
    /// old extents keeps reading the old value.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let si = self.shard_of(key);
        let mut sh = self.shards[si].lock().unwrap();
        // Buffer it like an append rather than writing a block of its own.
        // Giving each replaced value a private block cost a whole size class
        // per value: a hundred bytes reserved four kilobytes, and writing a
        // million of them produced a 4.6 GB file for 100 MB of data. Small
        // values have to share a block, whether they arrive by append or by
        // replacement.
        // What this replaces is not looked up here. Finding it costs a hash
        // and a probe on the hot path, and the index is already in hand at
        // seal time, where the new extent has to be recorded anyway. Putting a
        // value therefore touches one map, not two.
let (before, after) = {
            let e = sh.keys.get_or_insert(key);
            let p = e.pending.get_or_insert_with(Pending::default);
            let before = p.buf.len();
            p.buf.clear();
            put_uvarint(&mut p.buf, value.len() as u64);
            p.buf.extend_from_slice(value);
            p.last = 0;
            p.replaces = true;
            (before, p.buf.len())
        };
        sh.pending_bytes = sh.pending_bytes + after - before;
        Ok(())
    }

    /// Delete a key by leaving a tombstone.
    ///
    /// The entry stays in the index with no extents, which is what
    /// distinguishes a deleted key from one that was never written -- and what
    /// lets an older snapshot still find the value while the current
    /// generation reports it gone.
    pub fn delete(&self, key: &[u8]) -> Result<()> {
        let si = self.shard_of(key);
        let mut sh = self.shards[si].lock().unwrap();
        let (freed, old) = {
            let e = sh.keys.get_or_insert(key);
            let freed = e.pending.take().map(|p| p.buf.len()).unwrap_or(0);
            let old = e.extents.as_slice().to_vec();
            e.extents = Extents::None;
            (freed, old)
        };
        sh.pending_bytes -= freed;
        if self.opts.reclaim.releases() {
            let mut ap = self.appender.lock().unwrap();
            for x in &old {
                ap.release(x.block)?;
            }
        }
        Ok(())
    }

    /// Move live blocks down into freed space, then truncate.
    ///
    /// A block is named by its id, and the only place a file offset appears is
    /// the block index, so relocating one is a copy plus a single entry
    /// update -- no key ever has to be touched, however many of them point at
    /// it. That is what makes reclaiming interior space practical without any
    /// filesystem cooperation, and why hole punching is not needed: it worked
    /// only on some platforms and left the file sparse and scattered.
    ///
    /// Bounded by `max_moves` so it can be run in slices. Stopping early is
    /// always safe: every state it passes through is one the block index
    /// already describes.
    pub fn defragment(&self, max_moves: usize) -> Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut ap = self.appender.lock().unwrap();
        let mut moved = 0usize;
        while moved < max_moves {
            let holes = ap.free.coalesced();
            let Some(&(hole_off, hole_len)) = holes.first() else { break };
            // the highest-offset live block that fits in this hole
            let mut best: Option<(usize, u64)> = None;
            for (i, b) in ap.blocks.iter().enumerate() {
                if ap.live[i] == 0 || b.off <= hole_off {
                    continue;
                }
                if (b.cap as u64) <= hole_len {
                    match best {
                        Some((_, off)) if off >= b.off => {}
                        _ => best = Some((i, b.off)),
                    }
                }
            }
            let Some((id, _)) = best else { break };
            let loc = ap.blocks[id];
            let mut buf = vec![0u8; loc.stored as usize];
            ap.file.read_exact_at(&mut buf, loc.off)?;
            ap.file.write_all_at(&buf, hole_off)?;
            // one entry changes; every extent pointing here is untouched
            ap.blocks[id].off = hole_off;
            let gen = ap.generation;
            ap.free.release(loc.off, loc.cap, gen);
            ap.free.take_at(hole_off, loc.cap);
            moved += 1;
        }
        // give back whatever is now trailing
        let holes = ap.free.coalesced();
        let mut end = ap.off;
        for &(off, len) in holes.iter().rev() {
            if off + len == end {
                end = off;
            } else {
                break;
            }
        }
        if end < ap.off {
            ap.off = end;
            let _ = ap.file.set_len(end);
        }
        Ok(moved)
    }

    /// Merge one key's extents into a single contiguous run, in place.
    ///
    /// Called from the seal path, so the writer that fragmented the key pays
    /// for it. The old extents are released, which punches their blocks out
    /// once nothing references them.
    fn merge_key(&self, sh: &mut Shard, idx: u32) -> Result<()> {
        let exts = {
            let e = &sh.keys.entry_at(idx).extents;
            if e.as_slice().len() < self.opts.merge_threshold {
                return Ok(());
            }
            e.as_slice().to_vec()
        };
        let mut buf = Vec::new();
        let mut last = 0u32;
        let mut ap = self.appender.lock().unwrap();
        for e in &exts {
            let bytes = ap.read_extent(*e)?;
            last = (buf.len() as u32) + e.last;
            buf.extend_from_slice(&bytes);
        }
        let len = buf.len() as u32;
        let id = ap.write_block(&buf, self.opts.compress, true, self.opts.solo_chunk_size, self.opts.reclaim)?;
        ap.retain(id);
        // Only when reclaiming. Merging relocates a key's values; the originals
        // are unreachable from the current index either way, but an older
        // checkpoint still points at them and a reader may be holding it.
        // Releasing here unconditionally is what let Retain::Snapshots hand
        // out space it had promised to keep.
        if self.opts.reclaim.releases() {
            for e in &exts {
                ap.release(e.block)?;
            }
        }
        drop(ap);
        sh.keys.entry_at(idx).extents = Extents::One(Ext { block: id, off: 0, len, last });
        sh.merges += 1;
        Ok(())
    }

    /// Make everything written so far recoverable.
    ///
    /// Data first, then the slot that points at it, with a barrier between:
    /// a checkpoint that reached the slot but not the data would describe
    /// blocks that do not exist. Blocks written after the last checkpoint are
    /// simply unreachable after a crash -- leaked space, not corruption.
    pub fn checkpoint(&self) -> Result<u64> {
        use std::os::unix::fs::FileExt;
        self.flush()?;
        let mut all: Vec<(Vec<u8>, Extents)> = Vec::new();
        for sh in &self.shards {
            let sh = sh.lock().unwrap();
            all.extend(sh.keys.iter().map(|(k, e)| (k.to_vec(), e.extents.clone())));
        }
        all.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let key_idx = {
            let ap = self.appender.lock().unwrap();
            let prev = ap.last_index.map(|loc| (loc, ap.generation, ap.timestamp));
            encode_key_index(&all, ap.generation + 1, prev)
        };
        let mut ap = self.appender.lock().unwrap();
        let blk_idx = encode_block_index(&ap.blocks);
        let key_loc = write_section(&mut ap, &key_idx)?;
        let blk_loc = write_section(&mut ap, &blk_idx)?;
        // Drop entries older than any reader could still be relying on. A
        // reader outside the grace window is already unsafe and is told so, so
        // keeping the record forever only made every checkpoint bigger and
        // slower than the last.
        // keep only what a reader could still be checking against; below the
        // reuse floor the record can never change an answer
        let horizon = ap.reuse_floor(self.opts.reclaim).min(ap.generation).saturating_sub(1);
        ap.reuse_log.retain(|(_, _, gen)| *gen >= horizon);
        let reuse = encode_reuse_log(&ap.reuse_log);
        let reuse_loc = write_section(&mut ap, &reuse)?;
        ap.file.sync_data()?;

        let gen = ap.generation + 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let sb = Super {
            generation: gen,
            history_from: 0,
            timestamp: now.max(ap.timestamp),
            key_off: key_loc.off, key_stored: key_loc.stored as u64,
            key_uncompressed: key_loc.uncompressed as u64,
            blk_off: blk_loc.off, blk_stored: blk_loc.stored as u64,
            blk_uncompressed: blk_loc.uncompressed as u64,
            reuse_off: reuse_loc.off, reuse_stored: reuse_loc.stored as u64,
            reuse_uncompressed: reuse_loc.uncompressed as u64,
            high_water: ap.off,
        };
        let at = if gen % 2 == 0 { 0 } else { SLOT };
        ap.file.write_all_at(&sb.encode(), at)?;
        ap.file.sync_data()?;
        ap.generation = gen;
        ap.timestamp = sb.timestamp;
        ap.last_index = Some(key_loc);
        Ok(gen)
    }

    pub fn flush(&self) -> Result<()> {
        for s in &self.shards {
            let mut sh = s.lock().unwrap();
            self.seal_shard(&mut sh)?;
            self.flush_builder(&mut sh)?;
        }
        Ok(())
    }

    /// Serialize both indexes and the footer. Keys are written in sorted order
    /// so the index itself compresses (adjacent keys share prefixes) and so a
    /// future ordered scan can binary-search it directly.
    pub fn close(self) -> Result<Stats> {
        let keys: u64 = {
            let mut n = 0u64;
            for sh in &self.shards {
                n += sh.lock().unwrap().keys.len() as u64;
            }
            n
        };
        // checkpoint first so the store is recoverable even if trimming fails
        self.checkpoint()?;
        {
            let mut ap = self.appender.lock().unwrap();
            let free = ap.free.coalesced();
            let mut end = ap.off;
            for &(off, len) in free.iter().rev() {
                if off + len == end {
                    end = off;
                } else {
                    break;
                }
            }
            if end < ap.off {
                ap.off = end;
                let _ = ap.file.set_len(end);
            }
        }
        // trimming moved the high-water mark, so record the final state
        self.checkpoint()?;
        let ap = self.appender.lock().unwrap();
        let merges: u64 = self.shards.iter().map(|s| s.lock().unwrap().merges).sum();
        Ok(Stats {
            blocks: ap.blocks.len() as u64,
            bytes_written: ap.off,
            index_bytes: 0,
            keys,
            merges,
            free_bytes: ap.free.free_bytes(),
            reused: ap.free.reused().0,
            reused_bytes: ap.free.reused().1,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn encode_key_index(
    all: &[(Vec<u8>, Extents)],
    generation: u64,
    prev: Option<(BlockLoc, u64, u64)>,
) -> Vec<u8> {
    let mut out = Vec::new();
    // chain header: this generation always, then where the previous index
    // lives. The first checkpoint has no predecessor but still has an
    // identity, so the generation is written unconditionally.
    put_uvarint(&mut out, generation);
    match prev {
        Some((loc, pgen, pts)) => {
            put_uvarint(&mut out, pgen);
            put_uvarint(&mut out, pts);
            put_uvarint(&mut out, loc.off);
            put_uvarint(&mut out, loc.stored as u64);
            put_uvarint(&mut out, loc.uncompressed as u64);
        }
        None => {
            for _ in 0..5 {
                put_uvarint(&mut out, 0);
            }
        }
    }
    put_uvarint(&mut out, all.len() as u64);
    for (k, exts) in all {
        put_uvarint(&mut out, k.len() as u64);
        out.extend_from_slice(k);
        let slice = exts.as_slice();
        put_uvarint(&mut out, slice.len() as u64);
        for e in slice {
            put_uvarint(&mut out, e.block as u64);
            put_uvarint(&mut out, e.off as u64);
            put_uvarint(&mut out, e.len as u64);
            put_uvarint(&mut out, e.last as u64);
        }
    }
    out
}

fn encode_reuse_log(log: &[(u64, u32, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    put_uvarint(&mut out, log.len() as u64);
    for (off, cap, gen) in log {
        put_uvarint(&mut out, *off);
        put_uvarint(&mut out, *cap as u64);
        put_uvarint(&mut out, *gen);
    }
    out
}

fn decode_reuse_log(buf: &[u8]) -> Vec<(u64, u32, u64)> {
    let mut p = 0usize;
    let n = get_uvarint(buf, &mut p) as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let off = get_uvarint(buf, &mut p);
        let cap = get_uvarint(buf, &mut p) as u32;
        let gen = get_uvarint(buf, &mut p);
        out.push((off, cap, gen));
    }
    out
}

fn encode_block_index(blocks: &[BlockLoc]) -> Vec<u8> {
    let mut out = Vec::new();
    put_uvarint(&mut out, blocks.len() as u64);
    for b in blocks {
        put_uvarint(&mut out, b.off);
        put_uvarint(&mut out, b.stored as u64);
        put_uvarint(&mut out, b.uncompressed as u64);
        put_uvarint(&mut out, b.cap as u64);
        out.push((b.solo as u8) | ((b.chunked as u8) << 1));
    }
    out
}

fn write_section(ap: &mut Appender, payload: &[u8]) -> Result<BlockLoc> {
    let stored = block::compress(payload);
    let bytes: &[u8] = stored.as_deref().unwrap_or(payload);
    use std::os::unix::fs::FileExt;
    let loc = BlockLoc {
        off: ap.off,
        stored: bytes.len() as u32,
        uncompressed: payload.len() as u32,
        cap: bytes.len() as u32,
        chunked: false,
        solo: false,
    };
    ap.file.write_all_at(bytes, ap.off)?;
    ap.off += bytes.len() as u64;
    Ok(loc)
}

// ---------------------------------------------------------------- reading --

enum BlockRef<'a> {
    Mapped(&'a [u8]),
    Owned(Arc<Vec<u8>>),
}

impl BlockRef<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            BlockRef::Mapped(s) => s,
            BlockRef::Owned(v) => v.as_slice(),
        }
    }
}

thread_local! {
    static SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

/// Hand each record in an extent to the visitor; returns bytes emitted.
fn emit<F: FnMut(&[u8])>(extent: &[u8], f: &mut F) -> u64 {
    let mut p = 0usize;
    let mut total = 0u64;
    while p < extent.len() {
        let n = get_uvarint(extent, &mut p) as usize;
        f(&extent[p..p + n]);
        total += n as u64;
        p += n;
    }
    total
}

pub struct Reader {
    mmap: Mmap,
    /// Keys in sorted order, which is how they were written.
    ///
    /// A seal batch is sorted before it is packed, so the index comes off disk
    /// already ordered and an ordered scan is a walk rather than a sort. Point
    /// lookups binary-search this instead of consulting a separate hash table,
    /// which halves what an open has to build and keeps one structure
    /// authoritative.
    entries: Vec<(Vec<u8>, Extents)>,
    /// Open-addressed hash of key -> position in `entries`, plus a byte of the
    /// hash to reject most misses without touching the key.
    ///
    /// Binary search over a million sorted keys costs twenty comparisons, each
    /// chasing a pointer into a separate allocation; it measured 2.2x slower
    /// on shallow-key point reads than a hash. Keeping both costs eight bytes
    /// per key and keeps ordered scans and O(1) lookups in the same reader.
    hash: Vec<(u8, u32)>,
    hash_mask: usize,
    blocks: Vec<BlockLoc>,
    cache: BlockCache,
    /// Slot held in the reader table, released when this reader is dropped.
    slot: Option<usize>,
    /// The reserved page, mapped writable so the slot can be published.
    table: Option<MmapMut>,
    generation: u64,
    timestamp: u64,
    history_from: u64,
    /// Byte ranges overwritten after this reader's generation, sorted by
    /// offset. Empty for a reader of the current state, which is why a normal
    /// read pays nothing for this.
    overwritten: Vec<(u64, u64)>,
    /// (generation, timestamp, offset, stored, uncompressed) of the previous
    /// checkpoint's index.
    prev: Option<(u64, u64, u64, u64, u64)>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let (Some(slot), Some(table)) = (self.slot, self.table.as_ref()) {
            let t = unsafe { readers::slots(table.as_ptr()) };
            readers::release(t, slot);
        }
    }
}

fn key_hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

impl Reader {
    /// Open at the last complete checkpoint.
    ///
    /// Both slots are read and the valid one with the higher generation wins,
    /// so a crash during a checkpoint falls back to the previous state rather
    /// than failing to open. Anything written after that checkpoint is
    /// unreachable: leaked space, never corruption.
    /// Open the store as it stood at a given checkpoint generation.
    ///
    /// Walks the chain of index sections backwards. Only meaningful under
    /// Retain::Snapshots -- with Reclaim the older index is still there but the
    /// blocks it points at may have been handed to the free list and rewritten.
    pub fn open_as_of(path: &Path, generation: u64) -> Result<Reader> {
        let mut r = Reader::open(path)?;
        if generation < r.history_from {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "cannot read as of generation {generation}: history is intact only from \
                     generation {} onwards, because a mutation under Retain::Reclaim released \
                     space that earlier states referred to. Use Retain::Snapshots to keep it.",
                    r.history_from
                ),
            ));
        }
        while r.generation > generation {
            let Some((_pgen, pts, off, stored, uncompressed)) = r.prev else {
                break;
            };
            r = Reader::open_at(path, off, stored as usize, uncompressed as usize, pts)?;
        }
        r.load_overwritten(path)?;
        Ok(r)
    }

    /// Open the store as it stood at a wall-clock time (milliseconds since the
    /// epoch): the newest checkpoint taken at or before it.
    ///
    /// The generation is the exact handle; this is the convenient one. If the
    /// requested time predates every checkpoint, the oldest reachable one is
    /// returned rather than an error, since that is the earliest state that
    /// exists.
    pub fn open_as_of_time(path: &Path, millis: u64) -> Result<Reader> {
        let mut r = Reader::open(path)?;
        let floor = r.history_from;
        while r.timestamp > millis {
            let Some((pgen, pts, off, stored, uncompressed)) = r.prev else {
                break;
            };
            if pgen < floor {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "cannot read as of time {millis}: it falls before generation {floor}, \
                         the oldest state whose history survived a mutation under \
                         Retain::Reclaim. Use Retain::Snapshots to keep it."
                    ),
                ));
            }
            r = Reader::open_at(path, off, stored as usize, uncompressed as usize, pts)?;
        }
        r.load_overwritten(path)?;
        Ok(r)
    }

    /// Collect the byte ranges rewritten after this reader's generation.
    /// How many byte ranges are known to have been overwritten since this
    /// reader's generation. Zero means every value it can see is intact.
    pub fn overwritten_ranges(&self) -> usize {
        self.overwritten.len()
    }

    fn load_overwritten(&mut self, path: &Path) -> Result<()> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let a = Super::decode(&mmap[0..120]);
        let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + 120]);
        let sb = match (a, b) {
            (Some(x), Some(y)) => if x.generation >= y.generation { x } else { y },
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => return Ok(()),
        };
        if sb.reuse_off == 0 {
            return Ok(());
        }
        let raw = read_section(&mmap, sb.reuse_off, sb.reuse_stored as usize, sb.reuse_uncompressed as usize)?;
        let mut v: Vec<(u64, u64)> = decode_reuse_log(&raw)
            .into_iter()
            // a slot reused during generation N was overwritten after
            // generation N was checkpointed, so it is unsafe for readers at N
            // and earlier
            .filter(|(_, _, gen)| *gen >= self.generation)
            .map(|(off, cap, _)| (off, cap as u64))
            .collect();
        v.sort_unstable();
        self.overwritten = v;
        Ok(())
    }

    /// This reader's checkpoint identity: (generation, milliseconds).
    pub fn version(&self) -> (u64, u64) {
        (self.generation, self.timestamp)
    }

    /// True if this byte range was written over after this reader's generation.
    fn is_overwritten(&self, off: u64, len: u64) -> bool {
        if self.overwritten.is_empty() {
            return false;
        }
        let i = self.overwritten.partition_point(|(o, _)| *o < off + len);
        self.overwritten[..i].iter().any(|(o, l)| o + l > off)
    }

    fn check_extent(&self, e: Ext) -> Result<()> {
        if self.overwritten.is_empty() {
            return Ok(());
        }
        let loc = self.blocks[e.block as usize];
        if self.is_overwritten(loc.off, loc.stored as u64) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "this value was stored at bytes {}..{} of the file, which have since been \
                     written over by a later block: the store is running under Retain::Reclaim, \
                     which reuses the space a superseded value occupied. Other keys in this \
                     snapshot are still readable. Use Retain::Snapshots to keep history intact.",
                    loc.off,
                    loc.off + loc.stored as u64
                ),
            ));
        }
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Reader> {
        // The superblock lives in the first page and so is always visible, but
        // it is written last and points at data that may lie beyond the length
        // this mapping captured -- a writer checkpointing between the open and
        // the map is enough. Remap until the mapping covers what the
        // superblock describes; each attempt sees a state at least as new, so
        // this converges rather than spinning.
        for attempt in 0.. {
            let file = File::open(path)?;
            let mmap = unsafe { Mmap::map(&file)? };
            if mmap.len() < SUPER as usize {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "file too short to hold a superblock",
                ));
            }
            let a = Super::decode(&mmap[0..120]);
            let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + 120]);
            let newest = match (a, b) {
                (Some(x), Some(y)) => Some(if x.generation >= y.generation { x } else { y }),
                (Some(x), None) => Some(x),
                (None, Some(y)) => Some(y),
                (None, None) => None,
            };
            match newest {
                Some(sb) if sb.high_water as usize <= mmap.len() => {
                    // Claim the slot before parsing anything. Registration has
                    // to precede the slow part, or the writer can reuse the
                    // space this reader is about to walk while it is still
                    // decoding the index.
                    let claim = Self::claim(path, sb.generation);
                    let mut r = Self::open_mapped(mmap, sb)?;
                    if let Some((slot, table)) = claim {
                        r.slot = Some(slot);
                        r.table = Some(table);
                    }
                    return Ok(r);
                }
                // an older slot may still be fully covered by this mapping
                Some(_) => {
                    if let Some(older) = match (a, b) {
                        (Some(x), Some(y)) => Some(if x.generation < y.generation { x } else { y }),
                        _ => None,
                    } {
                        if older.high_water as usize <= mmap.len() {
                            return Self::open_mapped(mmap, older);
                        }
                    }
                }
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "no valid supdb checkpoint",
                    ))
                }
            }
            if attempt >= 16 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "the writer kept moving ahead of the mapping",
                ));
            }
            std::thread::yield_now();
        }
        unreachable!()
    }

    fn open_mapped(mmap: Mmap, sb: Super) -> Result<Reader> {
        let key_idx = read_section(&mmap, sb.key_off, sb.key_stored as usize, sb.key_uncompressed as usize)?;
        let blk_idx = read_section(&mmap, sb.blk_off, sb.blk_stored as usize, sb.blk_uncompressed as usize)?;
        Self::build(mmap, key_idx, blk_idx, sb.timestamp, sb.history_from)
    }

    /// Publish the generation being read so the writer will not hand out the
    /// space behind it. Best effort: if the table is full or unwritable the
    /// reader still works, and falls back on the grace window for safety.
    fn claim(path: &Path, generation: u64) -> Option<(usize, MmapMut)> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path).ok()?;
        let table = unsafe { MmapMut::map_mut(&file) }.ok()?;
        if table.len() < readers::TABLE_OFF + readers::TABLE_BYTES {
            return None;
        }
        let slot = {
            let t = unsafe { readers::slots(table.as_ptr()) };
            readers::acquire(t, generation.max(1))?
        };
        Some((slot, table))
    }

    /// Load a specific index section, for reading as of an older generation.
    fn open_at(path: &Path, off: u64, stored: usize, uncompressed: usize, ts: u64) -> Result<Reader> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let a = Super::decode(&mmap[0..120]);
        let b = Super::decode(&mmap[SLOT as usize..SLOT as usize + 120]);
        let sb = match (a, b) {
            (Some(x), Some(y)) => if x.generation >= y.generation { x } else { y },
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no checkpoint")),
        };
        let key_idx = read_section(&mmap, off, stored, uncompressed)?;
        let blk_idx = read_section(&mmap, sb.blk_off, sb.blk_stored as usize, sb.blk_uncompressed as usize)?;
        Self::build(mmap, key_idx, blk_idx, ts, sb.history_from)
    }

    fn build(mmap: Mmap, key_idx: Vec<u8>, blk_idx: Vec<u8>, ts: u64, history_from: u64) -> Result<Reader> {

        let mut p = 0usize;
        let nblocks = get_uvarint(&blk_idx, &mut p) as usize;
        let mut blocks = Vec::with_capacity(nblocks);
        for _ in 0..nblocks {
            let off = get_uvarint(&blk_idx, &mut p);
            let stored = get_uvarint(&blk_idx, &mut p) as u32;
            let uncompressed = get_uvarint(&blk_idx, &mut p) as u32;
            let cap = get_uvarint(&blk_idx, &mut p) as u32;
            let flags = blk_idx[p];
            p += 1;
            blocks.push(BlockLoc {
                off,
                stored,
                uncompressed,
                cap,
                solo: flags & 1 != 0,
                chunked: flags & 2 != 0,
            });
        }

        let mut p = 0usize;
        let _gen_read = get_uvarint(&key_idx, &mut p);
        let prev_gen = get_uvarint(&key_idx, &mut p);
        let prev_ts = get_uvarint(&key_idx, &mut p);
        let prev_off = get_uvarint(&key_idx, &mut p);
        let prev_stored = get_uvarint(&key_idx, &mut p);
        let prev_uncompressed = get_uvarint(&key_idx, &mut p);
        let prev = if prev_off > 0 {
            Some((prev_gen, prev_ts, prev_off, prev_stored, prev_uncompressed))
        } else {
            None
        };
        let nkeys = get_uvarint(&key_idx, &mut p) as usize;
        let mut entries: Vec<(Vec<u8>, Extents)> = Vec::with_capacity(nkeys);
        for _ in 0..nkeys {
            let klen = get_uvarint(&key_idx, &mut p) as usize;
            let key = key_idx[p..p + klen].to_vec();
            p += klen;
            let n = get_uvarint(&key_idx, &mut p) as usize;
            let mut exts = Extents::None;
            for _ in 0..n {
                let block = get_uvarint(&key_idx, &mut p) as u32;
                let o = get_uvarint(&key_idx, &mut p) as u32;
                let l = get_uvarint(&key_idx, &mut p) as u32;
                let last = get_uvarint(&key_idx, &mut p) as u32;
                exts.push(Ext { block, off: o, len: l, last });
            }
            entries.push((key, exts));
        }
        // written sorted, but a checkpoint taken mid-run can interleave, so
        // make the ordering an invariant rather than an assumption
        if !entries.windows(2).all(|w| w[0].0 <= w[1].0) {
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        }

        let mut cap = 1usize;
        while cap < entries.len() * 2 {
            cap <<= 1;
        }
        cap = cap.max(16);
        let mask = cap - 1;
        let mut hash = vec![(0u8, u32::MAX); cap];
        for (i, (k, _)) in entries.iter().enumerate() {
            let h = key_hash(k);
            let mut slot = (h as usize) & mask;
            while hash[slot].1 != u32::MAX {
                slot = (slot + 1) & mask;
            }
            // 0 marks an empty tag, so fold it away
            hash[slot] = (((h >> 56) as u8) | 1, i as u32);
        }

        let cache = BlockCache::new(4096);
        Ok(Reader { mmap, entries, hash, hash_mask: mask, blocks, cache, slot: None, table: None, generation: _gen_read, timestamp: ts, history_from, overwritten: Vec::new(), prev })
    }

    fn block(&self, id: u32) -> Result<BlockRef<'_>> {
        let loc = self.blocks[id as usize];
        let raw = &self.mmap[loc.off as usize..loc.off as usize + loc.stored as usize];
        if loc.is_plain() {
            // never compressed, so hand out the mapping itself
            return Ok(BlockRef::Mapped(raw));
        }
        if let Some(hit) = self.cache.get(id) {
            return Ok(BlockRef::Owned(hit));
        }
        let out = Arc::new(block::decompress(raw, loc.uncompressed as usize)?);
        self.cache.put(id, Arc::clone(&out));
        Ok(BlockRef::Owned(out))
    }

    /// Visit every value of a key in append order. Values are handed out as
    /// slices of the block, so a read allocates nothing per value.
    pub fn read_all<F: FnMut(&[u8])>(&self, key: &[u8], mut f: F) -> Result<u64> {
        let Some(exts) = self.lookup(key) else { return Ok(0) };
        let mut total = 0u64;
        for e in exts.as_slice() {
            self.check_extent(*e)?;
            let loc = self.blocks[e.block as usize];
            let end = loc.off as usize + loc.stored as usize;
            if end > self.mmap.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "block {} spans {}..{} but the mapping is {} bytes: the file grew after \
                         it was mapped",
                        e.block, loc.off, end, self.mmap.len()
                    ),
                ));
            }
            let raw = &self.mmap[loc.off as usize..end];
            if loc.is_plain() {
                total += emit(&raw[e.off as usize..(e.off + e.len) as usize], &mut f);
            } else if loc.chunked || loc.solo {
                if std::env::var_os("SUPDB_DEBUG").is_some() && loc.stored as usize > raw.len() {
                    eprintln!("# block {} off={} stored={} chunked={} solo={} mmap={}",
                        e.block, loc.off, loc.stored, loc.chunked, loc.solo, self.mmap.len());
                }
                // one key's run: decompress into scratch, retain nothing
                total += SCRATCH.with(|s| -> Result<u64> {
                    let mut buf = s.borrow_mut();
                    let un = loc.uncompressed as usize;
                    if buf.len() < un {
                        buf.resize(un, 0);
                    }
                    let (a, b) = (e.off as usize, (e.off + e.len) as usize);
                    let r = if loc.chunked {
                        block::read_chunked_range(raw, un, a, b, &mut buf[..un])
                    } else {
                        block::decompress_into(raw, &mut buf, un)
                    };
                    if let Err(err) = r {
                        if std::env::var_os("SUPDB_DEBUG").is_some() {
                            let head: Vec<u8> = raw.iter().take(12).copied().collect();
                            eprintln!(
                                "# gen={} blk={} off={} stored={} uncomp={} cap={} chunked={} solo={} \
                                 ext=({}..{}) mmap={} nblocks={} head={:?}",
                                self.generation, e.block, loc.off, loc.stored, loc.uncompressed, loc.cap,
                                loc.chunked, loc.solo, a, b, self.mmap.len(),
                                self.blocks.len(), head
                            );
                        }
                        return Err(err);
                    }
                    Ok(emit(&buf[a..b], &mut f))
                })?;
            } else {
                let b = self.block(e.block)?;
                total += emit(&b.as_slice()[e.off as usize..(e.off + e.len) as usize], &mut f);
            }
        }
        Ok(total)
    }

    /// Length of the record starting at `at` within the extent.
    ///
    /// A point read of a solo block still has to decompress the run to reach
    /// one record; scratch keeps it from allocating as well.
    fn record_len_at(&self, e: Ext, at: u32) -> Result<i32> {
        self.check_extent(e)?;
        let loc = self.blocks[e.block as usize];
        let raw = &self.mmap[loc.off as usize..loc.off as usize + loc.stored as usize];
        if loc.is_plain() {
            let mut p = (e.off + at) as usize;
            return Ok(get_uvarint(raw, &mut p) as i32);
        }
        if loc.chunked || loc.solo {
            return SCRATCH.with(|s| -> Result<i32> {
                let mut buf = s.borrow_mut();
                let un = loc.uncompressed as usize;
                if buf.len() < un {
                    buf.resize(un, 0);
                }
                let at = (e.off + at) as usize;
                if loc.chunked {
                    // only the chunks holding this one record
                    block::read_chunked_range(raw, un, at, (at + 16).min(un), &mut buf[..un])?;
                } else {
                    block::decompress_into(raw, &mut buf, un)?;
                }
                let mut p = at;
                Ok(get_uvarint(&buf, &mut p) as i32)
            });
        }
        let b = self.block(e.block)?;
        let mut p = (e.off + at) as usize;
        Ok(get_uvarint(b.as_slice(), &mut p) as i32)
    }

    pub fn read_first(&self, key: &[u8]) -> Result<i32> {
        let Some(exts) = self.lookup(key) else { return Ok(-1) };
        let Some(e) = exts.first() else { return Ok(-1) };
        self.record_len_at(e, 0)
    }

    pub fn read_last(&self, key: &[u8]) -> Result<i32> {
        let Some(exts) = self.lookup(key) else { return Ok(-1) };
        let Some(e) = exts.last() else { return Ok(-1) };
        self.record_len_at(e, e.last)
    }

    pub fn keys(&self) -> usize {
        self.entries.len()
    }

    fn lookup(&self, key: &[u8]) -> Option<&Extents> {
        let h = key_hash(key);
        let tag = ((h >> 56) as u8) | 1;
        let mut slot = (h as usize) & self.hash_mask;
        loop {
            let (t, i) = self.hash[slot];
            if i == u32::MAX {
                return None;
            }
            if t == tag {
                let e = &self.entries[i as usize];
                if e.0.as_slice() == key {
                    return Some(&e.1);
                }
            }
            slot = (slot + 1) & self.hash_mask;
        }
    }

    /// Position of the first key at or after `key`.
    pub fn seek(&self, key: &[u8]) -> usize {
        match self.entries.binary_search_by(|(k, _)| k.as_slice().cmp(key)) {
            Ok(i) | Err(i) => i,
        }
    }

    /// Visit keys in order from `from`, handing each key's values to the
    /// visitor. Returns the number of values emitted.
    ///
    /// A seal batch is sorted by key before it is packed, so a block holds a
    /// contiguous run of keys and a scan in key order returns to the same
    /// block hundreds of times before moving on -- around eight hundred keys
    /// per block at the sizes measured here. Holding the last block decoded
    /// turns that into no work at all; decompressing per key threw it away.
    ///
    /// A scan also needs no lookup: the entry is already in hand, so neither
    /// the hash nor the key copy that read_all performs is required.
    pub fn scan<F: FnMut(&[u8], &[u8])>(&self, from: Option<&[u8]>, limit: usize, mut f: F) -> Result<u64> {
        let start = from.map(|k| self.seek(k)).unwrap_or(0);
        let end = (start + limit).min(self.entries.len());
        let mut n = 0u64;
        let mut cached: u32 = u32::MAX;
        let mut buf: Vec<u8> = Vec::new();
        // which chunks of the cached block have been decoded
        let mut have: Vec<u64> = Vec::new();

        for i in start..end {
            let (key, exts) = &self.entries[i];
            for e in exts.as_slice() {
                let loc = self.blocks[e.block as usize];
                let raw = &self.mmap[loc.off as usize..loc.off as usize + loc.stored as usize];
                let extent: &[u8] = if loc.is_plain() {
                    &raw[e.off as usize..(e.off + e.len) as usize]
                } else {
                    let un = loc.uncompressed as usize;
                    if cached != e.block {
                        if buf.len() < un {
                            buf.resize(un, 0);
                        }
                        have.clear();
                        have.resize(un / 64 + 2, 0);
                        if !loc.chunked {
                            block::decompress_into(raw, &mut buf, un)?;
                        }
                        cached = e.block;
                    }
                    if loc.chunked {
                        // only the chunks this key needs and does not have
                        block::read_chunks_into(
                            raw,
                            un,
                            e.off as usize,
                            (e.off + e.len) as usize,
                            &mut buf[..un],
                            &mut have,
                        )?;
                    }
                    &buf[e.off as usize..(e.off + e.len) as usize]
                };
                let mut p = 0usize;
                while p < extent.len() {
                    let len = get_uvarint(extent, &mut p) as usize;
                    f(key, &extent[p..p + len]);
                    n += 1;
                    p += len;
                }
            }
        }
        Ok(n)
    }

}

fn read_section(mmap: &Mmap, off: u64, stored: usize, uncompressed: usize) -> Result<Vec<u8>> {
    // A writer may have extended the file since this mapping was made, so a
    // section can legitimately lie beyond its end. Report it rather than
    // indexing past the slice.
    let end = off as usize + stored;
    if end > mmap.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("section at {off}..{end} lies past the {} byte mapping", mmap.len()),
        ));
    }
    let raw = &mmap[off as usize..end];
    if stored == uncompressed {
        Ok(raw.to_vec())
    } else {
        block::decompress(raw, uncompressed)
    }
}

impl Store {
    pub fn reopen(path: &Path) -> Result<Reader> {
        Reader::open(path)
    }
}

#[allow(dead_code)]
fn unused_seek(f: &mut File) -> Result<u64> {
    f.seek(SeekFrom::Current(0))
}
