//! Blocks: the unit of compression and of caching.
//!
//! A block holds the sealed extents of many keys, concatenated. Compressing at
//! this granularity rather than per key is the difference between a window of
//! roughly a kilobyte and one of tens of kilobytes -- the measured difference
//! between no compression at all and 2.7-3.6x on realistic event data.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Where a block lives and how big it is in each form.
#[derive(Clone, Copy, Debug)]
pub struct BlockLoc {
    pub off: u64,
    pub stored: u32,
    pub uncompressed: u32,
    /// Space reserved for this block; >= stored, rounded to a size class so
    /// the slot can be handed to a later block of similar size.
    pub cap: u32,
    /// Payload is compressed in fixed-size chunks with a directory, so a
    /// point read decompresses one chunk instead of the whole block.
    pub chunked: bool,
    /// This block holds a single key's extent.
    ///
    /// A solo block can never produce a cache hit for another key, so caching
    /// one is pure loss: it costs an allocation and evicts a shared block that
    /// would have been reused. Solo blocks decompress into a per-thread
    /// scratch buffer and are never retained.
    pub solo: bool,
}

impl BlockLoc {
    /// Stored verbatim because compressing it did not pay.
    pub fn is_plain(&self) -> bool {
        self.stored == self.uncompressed
    }
}

/// Accumulates sealed extents until the block is full.
pub struct BlockBuilder {
    buf: Vec<u8>,
    cap: usize,
}

impl BlockBuilder {
    pub fn new(cap: usize) -> Self {
        BlockBuilder { buf: Vec::with_capacity(cap), cap }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// True when adding `n` more bytes would overflow the target size. An
    /// extent larger than a whole block is allowed to make its own oversized
    /// block rather than being split, which keeps a key's run contiguous.
    pub fn would_overflow(&self, n: usize) -> bool {
        !self.buf.is_empty() && self.buf.len() + n > self.cap
    }

    pub fn push(&mut self, extent: &[u8]) -> u32 {
        let off = self.buf.len() as u32;
        self.buf.extend_from_slice(extent);
        off
    }

    pub fn take(&mut self) -> Vec<u8> {
        std::mem::replace(&mut self.buf, Vec::with_capacity(self.cap))
    }
}

/// A bounded cache of decompressed blocks.
///
/// Without this, a compressed store pays decompression on every read and warm
/// reads collapse -- the measured tradeoff that forced Uppend to choose
/// between 407 MB at 16,988 reads/s and 1,065 MB at 53,507 reads/s.
pub struct BlockCache {
    shards: Vec<Mutex<Shard>>,
    mask: usize,
}

struct Shard {
    map: HashMap<u32, Arc<Vec<u8>>>,
    order: VecDeque<u32>,
    cap: usize,
}

impl BlockCache {
    pub fn new(total_blocks: usize) -> Self {
        let shard_count = 16;
        let per = (total_blocks / shard_count).max(4);
        BlockCache {
            shards: (0..shard_count)
                .map(|_| {
                    Mutex::new(Shard {
                        map: HashMap::with_capacity(per),
                        order: VecDeque::with_capacity(per),
                        cap: per,
                    })
                })
                .collect(),
            mask: shard_count - 1,
        }
    }

    fn shard(&self, id: u32) -> &Mutex<Shard> {
        &self.shards[(id as usize) & self.mask]
    }

    pub fn get(&self, id: u32) -> Option<Arc<Vec<u8>>> {
        let s = self.shard(id).lock().unwrap();
        s.map.get(&id).map(Arc::clone)
    }

    pub fn put(&self, id: u32, bytes: Arc<Vec<u8>>) {
        let mut s = self.shard(id).lock().unwrap();
        if s.map.contains_key(&id) {
            return;
        }
        if s.map.len() >= s.cap {
            if let Some(old) = s.order.pop_front() {
                s.map.remove(&old);
            }
        }
        s.order.push_back(id);
        s.map.insert(id, bytes);
    }
}

pub fn compress(src: &[u8]) -> Option<Vec<u8>> {
    let out = lz4_flex::compress(src);
    if out.len() < src.len() { Some(out) } else { None }
}

pub fn decompress(src: &[u8], uncompressed: usize) -> std::io::Result<Vec<u8>> {
    lz4_flex::decompress(src, uncompressed)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Decompress into a caller-owned buffer, allocating nothing.
pub fn decompress_into(src: &[u8], dst: &mut Vec<u8>, uncompressed: usize) -> std::io::Result<()> {
    if dst.len() < uncompressed {
        dst.resize(uncompressed, 0);
    }
    lz4_flex::block::decompress_into(src, &mut dst[..uncompressed])
        .map(|_| ())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// A block compressed in independently-decodable chunks.
///
/// Compressing a 47 KB run as one unit means reading a single record out of it
/// costs a full decompression -- which is what turned a 308k/s point lookup
/// into 46k/s after merging. Chunking keeps the compression window large
/// enough to be worth having while making any byte range reachable by
/// decompressing only the chunks that cover it.
///
/// Layout: [u32 chunk_size][u32 n][u32 start_0 .. start_n] then the chunks.
/// `start_i` is relative to the start of the payload; `start_n` is the end.
pub const CHUNK: usize = 4096;

/// The chunk size is the read-amplification dial. A wide key holds under a
/// kilobyte, so a 4 KiB chunk inflates 4 KiB to hand back 960 bytes. Smaller
/// chunks decompress less per read and compress slightly worse; the reader
/// takes the size from the block header, so this is a write-time choice and
/// old blocks stay readable.
pub fn write_chunked_sz(src: &[u8], chunk: usize) -> Vec<u8> {
    let n = (src.len() + chunk - 1) / chunk;
    let header = 8 + 4 * (n + 1);
    let mut out = Vec::with_capacity(header + src.len());
    out.extend_from_slice(&(chunk as u32).to_le_bytes());
    out.extend_from_slice(&(n as u32).to_le_bytes());
    out.resize(header, 0);
    let mut starts = Vec::with_capacity(n + 1);
    for i in 0..n {
        starts.push((out.len() - header) as u32);
        let lo = i * chunk;
        let hi = ((i + 1) * chunk).min(src.len());
        let raw = &src[lo..hi];
        match compress(raw) {
            Some(c) => out.extend_from_slice(&c),
            // a chunk that will not compress is stored verbatim; the stored
            // length tells the reader which it is
            None => out.extend_from_slice(raw),
        }
    }
    starts.push((out.len() - header) as u32);
    for (i, st) in starts.iter().enumerate() {
        out[8 + 4 * i..12 + 4 * i].copy_from_slice(&st.to_le_bytes());
    }
    out
}

pub fn write_chunked(src: &[u8]) -> Vec<u8> {
    write_chunked_sz(src, CHUNK)
}

/// Populate `dst[a..b]` from a chunked block, touching only the chunks that
/// cover that range. `dst` must already be sized to the uncompressed length.
/// Chunk size and count of a chunked block, without decoding anything.
pub fn chunk_geometry(blk: &[u8]) -> Option<(usize, usize)> {
    if blk.len() < 8 {
        return None;
    }
    let cs = u32::from_le_bytes(blk[0..4].try_into().unwrap()) as usize;
    let n = u32::from_le_bytes(blk[4..8].try_into().unwrap()) as usize;
    if cs == 0 || n == 0 || n > (1 << 20) {
        return None;
    }
    Some((cs, n))
}

/// Decode only the chunks covering `a..b` that `have` does not already mark as
/// decoded, setting their bits.
///
/// A scan revisits a block many times as it walks keys in order, but a key may
/// need only one chunk of it. Decoding the whole block on first touch suits a
/// block holding hundreds of single-value keys and wastes sixty-fold on one
/// holding a few keys with many values each. Tracking which chunks are already
/// decoded serves both.
pub fn read_chunks_into(
    blk: &[u8],
    uncompressed: usize,
    a: usize,
    b: usize,
    dst: &mut [u8],
    have: &mut [u64],
) -> std::io::Result<()> {
    let Some((cs, n)) = chunk_geometry(blk) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a valid chunked block; its space may have been reused",
        ));
    };
    let header = 8 + 4 * (n + 1);
    let start = |i: usize| -> usize {
        u32::from_le_bytes(blk[8 + 4 * i..12 + 4 * i].try_into().unwrap()) as usize
    };
    for i in (a / cs)..=((b.saturating_sub(1)) / cs).min(n - 1) {
        let (w, bit) = (i / 64, 1u64 << (i % 64));
        if have[w] & bit != 0 {
            continue;
        }
        let (s0, s1) = (start(i), start(i + 1));
        if s1 < s0 || header + s1 > blk.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "chunk offsets out of range",
            ));
        }
        let raw = &blk[header + s0..header + s1];
        let (lo, hi) = (i * cs, ((i + 1) * cs).min(uncompressed));
        if raw.len() == hi - lo {
            dst[lo..hi].copy_from_slice(raw);
        } else {
            lz4_flex::block::decompress_into(raw, &mut dst[lo..hi])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        }
        have[w] |= bit;
    }
    Ok(())
}

pub fn read_chunked_range(blk: &[u8], uncompressed: usize, a: usize, b: usize, dst: &mut [u8]) -> std::io::Result<()> {
    // These bytes may not be a chunk directory at all -- under Retain::Reclaim
    // the space a superseded value occupied can have been written over. Decode
    // defensively and report it, rather than trusting a length read out of
    // whatever now lives here.
    let bad = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("not a valid chunked block ({what}); its space may have been reused"),
        )
    };
    if blk.len() < 8 {
        return Err(bad("truncated header"));
    }
    let cs = u32::from_le_bytes(blk[0..4].try_into().unwrap()) as usize;
    let n = u32::from_le_bytes(blk[4..8].try_into().unwrap()) as usize;
    if cs == 0 || n == 0 || n > (1 << 20) {
        return Err(bad("implausible chunk directory"));
    }
    let header = 8 + 4 * (n + 1);
    if blk.len() < header {
        return Err(bad("directory extends past the block"));
    }
    let start = |i: usize| -> usize {
        u32::from_le_bytes(blk[8 + 4 * i..12 + 4 * i].try_into().unwrap()) as usize
    };
    let first = a / cs;
    let last = (b.saturating_sub(1)) / cs;
    for i in first..=last.min(n - 1) {
        let (s0, s1) = (start(i), start(i + 1));
        if s1 < s0 || header + s1 > blk.len() {
            return Err(bad("chunk offsets out of range"));
        }
        let raw = &blk[header + s0..header + s1];
        let lo = i * cs;
        let hi = ((i + 1) * cs).min(uncompressed);
        if raw.len() == hi - lo {
            dst[lo..hi].copy_from_slice(raw);
        } else {
            lz4_flex::block::decompress_into(raw, &mut dst[lo..hi])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        }
    }
    Ok(())
}
