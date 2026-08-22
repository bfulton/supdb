//! Blocks: the unit of compression and of caching.
//!
//! A block holds the sealed extents of many keys, concatenated. Compressing at
//! this granularity rather than per key is the difference between a window of
//! roughly a kilobyte and one of tens of kilobytes -- the measured difference
//! between no compression at all and 2.7-3.6x on realistic event data.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// CRC-32C (Castagnoli) over stored bytes.
///
/// Nothing outside the 120-byte superblock used to be checksummed, so a bit
/// flip, a torn write, or a reused slot produced silently wrong data. LZ4 will
/// decode many corrupted inputs into plausible bytes, so a successful
/// decompression is not evidence of integrity.
///
/// Castagnoli rather than IEEE because x86-64 implements it in hardware. That
/// matters here: a byte-at-a-time IEEE table cost 13-35% of write throughput
/// when first measured (`results/` keeps that datapoint), which is not a
/// reasonable price on the one axis this design is built to win. The hardware
/// path is used when the CPU advertises SSE4.2; everything else takes a
/// portable slice-by-8 table. A test asserts the two agree, because two
/// implementations of one function is exactly the kind of thing that silently
/// diverges.
const CASTAGNOLI: u32 = 0x82F6_3B78;

/// Eight tables, for consuming eight bytes per iteration.
static CRC_TABLES: [[u32; 256]; 8] = build_crc_tables();

const fn build_crc_tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                CASTAGNOLI ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut j = 1usize;
    while j < 8 {
        let mut i = 0usize;
        while i < 256 {
            let prev = t[j - 1][i];
            t[j][i] = (prev >> 8) ^ t[0][(prev & 0xff) as usize];
            i += 1;
        }
        j += 1;
    }
    t
}

/// Portable slice-by-8. Consumes eight bytes per round rather than one.
fn crc32c_scalar(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    let mut chunks = data.chunks_exact(8);
    for w in &mut chunks {
        let lo = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) ^ c;
        let hi = u32::from_le_bytes([w[4], w[5], w[6], w[7]]);
        c = CRC_TABLES[7][(lo & 0xff) as usize]
            ^ CRC_TABLES[6][((lo >> 8) & 0xff) as usize]
            ^ CRC_TABLES[5][((lo >> 16) & 0xff) as usize]
            ^ CRC_TABLES[4][((lo >> 24) & 0xff) as usize]
            ^ CRC_TABLES[3][(hi & 0xff) as usize]
            ^ CRC_TABLES[2][((hi >> 8) & 0xff) as usize]
            ^ CRC_TABLES[1][((hi >> 16) & 0xff) as usize]
            ^ CRC_TABLES[0][((hi >> 24) & 0xff) as usize];
    }
    for b in chunks.remainder() {
        c = CRC_TABLES[0][((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn crc32c_hw(data: &[u8]) -> u32 {
    use core::arch::x86_64::{_mm_crc32_u64, _mm_crc32_u8};
    let mut c = 0xFFFF_FFFFu64;
    let mut chunks = data.chunks_exact(8);
    for w in &mut chunks {
        c = _mm_crc32_u64(c, u64::from_le_bytes(w.try_into().unwrap()));
    }
    let mut c = c as u32;
    for b in chunks.remainder() {
        c = _mm_crc32_u8(c, *b);
    }
    c ^ 0xFFFF_FFFF
}

/// ARMv8 CRC extension. Graviton and Apple Silicon both have it; it is
/// optional in the base ISA, so it is still feature-detected.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
unsafe fn crc32c_hw(data: &[u8]) -> u32 {
    use core::arch::aarch64::{__crc32cb, __crc32cd};
    let mut c = 0xFFFF_FFFFu32;
    let mut chunks = data.chunks_exact(8);
    for w in &mut chunks {
        c = __crc32cd(c, u64::from_le_bytes(w.try_into().unwrap()));
    }
    for b in chunks.remainder() {
        c = __crc32cb(c, *b);
    }
    c ^ 0xFFFF_FFFF
}

/// Resolved once, not per call: 0 unknown, 1 hardware, 2 scalar.
static CRC_IMPL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn crc32(data: &[u8]) -> u32 {
    #[allow(unused_imports)]
    use std::sync::atomic::Ordering;
    #[cfg(target_arch = "aarch64")]
    {
        let mut which = CRC_IMPL.load(Ordering::Relaxed);
        if which == 0 {
            which = if std::arch::is_aarch64_feature_detected!("crc") {
                1
            } else {
                2
            };
            CRC_IMPL.store(which, Ordering::Relaxed);
        }
        if which == 1 {
            // SAFETY: only reached once the CPU has advertised the CRC extension.
            return unsafe { crc32c_hw(data) };
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut which = CRC_IMPL.load(Ordering::Relaxed);
        if which == 0 {
            which = if std::arch::is_x86_feature_detected!("sse4.2") {
                1
            } else {
                2
            };
            CRC_IMPL.store(which, Ordering::Relaxed);
        }
        if which == 1 {
            // SAFETY: only reached once the CPU has advertised SSE4.2.
            return unsafe { crc32c_hw(data) };
        }
    }
    crc32c_scalar(data)
}

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
    /// CRC-32 over the block's stored bytes.
    ///
    /// Chunked blocks carry per-chunk checksums in their directory, so a point
    /// read verifies only what it decodes. This covers everything else: a
    /// block stored verbatim because compression did not pay, and a block
    /// small enough to be compressed as a single stream. Without it those two
    /// paths were the residue that survived chunk checksums -- 7.5% of damage
    /// to live payload still read back silently.
    pub crc: u32,
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
        BlockBuilder {
            buf: Vec::with_capacity(cap),
            cap,
        }
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
    if out.len() < src.len() {
        Some(out)
    } else {
        None
    }
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
/// Layout: [u32 chunk_size][u32 n][u32 start_0 .. start_n][u32 crc_0 .. crc_{n-1}]
/// then the chunks. `start_i` is relative to the start of the payload;
/// `start_n` is the end. `crc_i` covers the *stored* bytes of chunk `i`, so a
/// point read verifies only the chunk it decodes rather than the whole block --
/// which is the whole reason the chunking exists.
pub const CHUNK: usize = 4096;

/// The chunk size is the read-amplification dial. A wide key holds under a
/// kilobyte, so a 4 KiB chunk inflates 4 KiB to hand back 960 bytes. Smaller
/// chunks decompress less per read and compress slightly worse; the reader
/// takes the size from the block header, so this is a write-time choice and
/// old blocks stay readable.
/// When false, chunk checksums are written as zero and not verified. Set once
/// at store creation; this is a measurement knob, not a per-call switch.
pub static CHECKSUMS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

#[inline]
pub fn checksums_on() -> bool {
    CHECKSUMS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn write_chunked_sz(src: &[u8], chunk: usize) -> Vec<u8> {
    let n = src.len().div_ceil(chunk);
    let header = chunk_header_len(n);
    let mut out = Vec::with_capacity(header + src.len());
    out.extend_from_slice(&(chunk as u32).to_le_bytes());
    out.extend_from_slice(&(n as u32).to_le_bytes());
    out.resize(header, 0);
    let mut starts = Vec::with_capacity(n + 1);
    let mut crcs = Vec::with_capacity(n);
    for i in 0..n {
        starts.push((out.len() - header) as u32);
        let lo = i * chunk;
        let hi = ((i + 1) * chunk).min(src.len());
        let raw = &src[lo..hi];
        let before = out.len();
        match compress(raw) {
            Some(c) => out.extend_from_slice(&c),
            // a chunk that will not compress is stored verbatim; the stored
            // length tells the reader which it is
            None => out.extend_from_slice(raw),
        }
        crcs.push(if checksums_on() {
            crc32(&out[before..])
        } else {
            0
        });
    }
    starts.push((out.len() - header) as u32);
    for (i, st) in starts.iter().enumerate() {
        out[8 + 4 * i..12 + 4 * i].copy_from_slice(&st.to_le_bytes());
    }
    let crc_base = 8 + 4 * (n + 1);
    for (i, c) in crcs.iter().enumerate() {
        out[crc_base + 4 * i..crc_base + 4 * i + 4].copy_from_slice(&c.to_le_bytes());
    }
    out
}

/// Directory size for `n` chunks: sizes, n+1 starts, and n checksums.
#[inline]
pub fn chunk_header_len(n: usize) -> usize {
    8 + 4 * (n + 1) + 4 * n
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
    let header = chunk_header_len(n);
    if blk.len() < header {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "chunk directory extends past the block",
        ));
    }
    let start = |i: usize| -> usize {
        u32::from_le_bytes(blk[8 + 4 * i..12 + 4 * i].try_into().unwrap()) as usize
    };
    let crc_base = 8 + 4 * (n + 1);
    let stored_crc = |i: usize| {
        u32::from_le_bytes(
            blk[crc_base + 4 * i..crc_base + 4 * i + 4]
                .try_into()
                .unwrap(),
        )
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
        if checksums_on() && crc32(raw) != stored_crc(i) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "chunk checksum mismatch: these bytes are not what was written",
            ));
        }
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

pub fn read_chunked_range(
    blk: &[u8],
    uncompressed: usize,
    a: usize,
    b: usize,
    dst: &mut [u8],
) -> std::io::Result<()> {
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
    let header = chunk_header_len(n);
    if blk.len() < header {
        return Err(bad("directory extends past the block"));
    }
    let start = |i: usize| -> usize {
        u32::from_le_bytes(blk[8 + 4 * i..12 + 4 * i].try_into().unwrap()) as usize
    };
    let crc_base = 8 + 4 * (n + 1);
    let stored_crc = |i: usize| {
        u32::from_le_bytes(
            blk[crc_base + 4 * i..crc_base + 4 * i + 4]
                .try_into()
                .unwrap(),
        )
    };
    let first = a / cs;
    let last = (b.saturating_sub(1)) / cs;
    for i in first..=last.min(n - 1) {
        let (s0, s1) = (start(i), start(i + 1));
        if s1 < s0 || header + s1 > blk.len() {
            return Err(bad("chunk offsets out of range"));
        }
        let raw = &blk[header + s0..header + s1];
        // Verify before decoding. LZ4 decodes many corrupted inputs into
        // plausible bytes, so a successful decompression is not evidence that
        // the bytes are the ones that were written.
        if checksums_on() && crc32(raw) != stored_crc(i) {
            return Err(bad("chunk checksum mismatch"));
        }
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

#[cfg(test)]
mod checksum_tests {
    use super::*;

    fn payload() -> Vec<u8> {
        // Compressible enough that chunks actually shrink, varied enough that
        // they are not all identical.
        (0..8192u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn round_trip_is_lossless() {
        let src = payload();
        let blk = write_chunked_sz(&src, 1024);
        let mut out = vec![0u8; src.len()];
        read_chunked_range(&blk, src.len(), 0, src.len(), &mut out).unwrap();
        assert_eq!(out, src);
    }

    /// The point of the checksum: a flipped bit in a stored chunk is caught
    /// rather than decoded into plausible bytes.
    #[test]
    fn a_flipped_bit_in_a_chunk_is_detected() {
        let src = payload();
        let blk = write_chunked_sz(&src, 1024);
        let header = {
            let (_, n) = chunk_geometry(&blk).unwrap();
            chunk_header_len(n)
        };
        let mut caught = 0;
        let mut missed = 0;
        for off in header..blk.len() {
            let mut damaged = blk.clone();
            damaged[off] ^= 0x01;
            let mut out = vec![0u8; src.len()];
            match read_chunked_range(&damaged, src.len(), 0, src.len(), &mut out) {
                Err(_) => caught += 1,
                Ok(()) => {
                    if out == src {
                        caught += 1; // byte was padding; output still correct
                    } else {
                        missed += 1;
                    }
                }
            }
        }
        assert_eq!(
            missed, 0,
            "{missed} corruptions produced wrong data silently"
        );
        assert!(caught > 0);
    }

    #[test]
    fn a_damaged_chunk_directory_does_not_panic() {
        let src = payload();
        let blk = write_chunked_sz(&src, 1024);
        for off in 0..64.min(blk.len()) {
            let mut damaged = blk.clone();
            damaged[off] ^= 0xff;
            let mut out = vec![0u8; src.len()];
            // Either an error or a correct decode; never a panic.
            let _ = read_chunked_range(&damaged, src.len(), 0, src.len(), &mut out);
        }
    }

    /// Two implementations of one function silently diverging is exactly the
    /// hazard this arrangement creates, so it is tested directly.
    #[test]
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn hardware_and_scalar_crc_agree() {
        #[cfg(target_arch = "x86_64")]
        if !std::arch::is_x86_feature_detected!("sse4.2") {
            return;
        }
        #[cfg(target_arch = "aarch64")]
        if !std::arch::is_aarch64_feature_detected!("crc") {
            return;
        }
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        for len in [0usize, 1, 3, 7, 8, 9, 15, 16, 31, 64, 255, 4096, 9001] {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed & 0xff) as u8
                })
                .collect();
            let hw = unsafe { crc32c_hw(&data) };
            assert_eq!(hw, crc32c_scalar(&data), "diverged at len {len}");
            assert_eq!(
                hw,
                crc32(&data),
                "dispatch picked the wrong one at len {len}"
            );
        }
    }

    #[test]
    fn crc_detects_single_bit_changes() {
        let a = b"the quick brown fox";
        let mut b = *a;
        b[3] ^= 1;
        assert_ne!(crc32(a), crc32(&b));
        assert_eq!(crc32(a), crc32(a));
    }
}
