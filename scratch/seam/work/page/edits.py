#!/usr/bin/env python3
# SCRATCH(page): the edits of E9's patch, applied to a tree at ROOT (3df85d22).
# Every replacement must match exactly once. usage: edits.py ROOT
import sys, os
ROOT = sys.argv[1]


def sub(path, old, new, count=1):
    p = os.path.join(ROOT, path)
    s = open(p).read()
    n = s.count(old)
    if n != count:
        sys.exit(f"{path}: expected {count} of {old[:80]!r}, found {n}")
    s = s.replace(old, new)
    open(p, "w").write(s)


# ---------------------------------------------------------------- crypt.rs
C = "src/crypt.rs"
sub(C, "const VERSION: u32 = 1;\n",
    "const VERSION: u32 = 1;\n/// SCRATCH(page): a store's envelope with its tags apart.\nconst VERSION_APART: u32 = 2;\n")
sub(C, """pub struct Header {
    chunk_shift: u32,
    plain_len: u64,
    id: [u8; 16],
    key_id: u64,
}""", """pub struct Header {
    chunk_shift: u32,
    plain_len: u64,
    id: [u8; 16],
    key_id: u64,
    /// SCRATCH(page): version 2, a chunk's ciphertext a whole stored chunk
    /// and the tags in a region after the last.
    apart: bool,
}""")
sub(C, """    pub fn new(plain_len: u64, chunk_shift: u32, id: [u8; 16], key_id: u64) -> Result<Header> {
        if !(MIN_CHUNK_SHIFT""", """    pub fn new(plain_len: u64, chunk_shift: u32, id: [u8; 16], key_id: u64) -> Result<Header> {
        Header::new_as(plain_len, chunk_shift, id, key_id, false)
    }

    /// SCRATCH(page): `new` for version 2, the tags apart.
    pub(crate) fn new_apart(
        plain_len: u64,
        chunk_shift: u32,
        id: [u8; 16],
        key_id: u64,
    ) -> Result<Header> {
        Header::new_as(plain_len, chunk_shift, id, key_id, true)
    }

    fn new_as(
        plain_len: u64,
        chunk_shift: u32,
        id: [u8; 16],
        key_id: u64,
        apart: bool,
    ) -> Result<Header> {
        if !(MIN_CHUNK_SHIFT""")
sub(C, """        let h = Header {
            chunk_shift,
            plain_len,
            id,
            key_id,
        };""", """        let h = Header {
            chunk_shift,
            plain_len,
            id,
            key_id,
            apart,
        };""")
sub(C, """        let version = u32_at(8);
        if version != VERSION {
            return Err(refused(&format!(
                "an encrypted object of version {version}; this reader reads version {VERSION}"
            )));
        }
        Header::new(
            u64_at(16),
            u32_at(12),
            b[24..40].try_into().unwrap(),
            u64_at(40),
        )""", """        let version = u32_at(8);
        if version != VERSION && version != VERSION_APART {
            return Err(refused(&format!(
                "an encrypted object of version {version}; this reader reads version {VERSION}"
            )));
        }
        Header::new_as(
            u64_at(16),
            u32_at(12),
            b[24..40].try_into().unwrap(),
            u64_at(40),
            version == VERSION_APART,
        )""")
sub(C, """        b[8..12].copy_from_slice(&VERSION.to_le_bytes());""",
    """        let v = if self.apart { VERSION_APART } else { VERSION };
        b[8..12].copy_from_slice(&v.to_le_bytes());""")
sub(C, """    pub fn chunk_len(&self) -> usize {
        (1 << self.chunk_shift) - TAG
    }""", """    pub fn chunk_len(&self) -> usize {
        if self.apart {
            1 << self.chunk_shift
        } else {
            (1 << self.chunk_shift) - TAG
        }
    }

    /// SCRATCH(page): whether the tags are apart (version 2).
    pub(crate) fn apart(&self) -> bool {
        self.apart
    }

    /// SCRATCH(page): where chunk `c`'s tag is: in the region after the
    /// last ciphertext byte under version 2, after its ciphertext under 1.
    pub(crate) fn tag_off(&self, c: u64) -> u64 {
        if self.apart {
            DATA_START + self.plain_len + c * TAG as u64
        } else {
            self.stored_off(c) + self.chunk_plain_len(c) as u64
        }
    }

    /// SCRATCH(page): the end of chunk `c`'s bytes in the data region.
    pub(crate) fn stored_end(&self, c: u64) -> u64 {
        self.stored_off(c) + self.chunk_plain_len(c) as u64 + if self.apart { 0 } else { TAG as u64 }
    }""")
sub(C, """    pub fn open_probe(&self) -> u64 {
        self.stored_off(0)""", """    pub fn open_probe(&self) -> u64 {
        debug_assert!(!self.apart, "SCRATCH(page): v1 only");
        self.stored_off(0)""")
sub(C, """    pub fn stored_range(&self, off: u64, len: u64) -> (u64, u64) {
""", """    pub fn stored_range(&self, off: u64, len: u64) -> (u64, u64) {
        debug_assert!(!self.apart, "SCRATCH(page): v1 only");
""")
sub(C, """    fn chunk_plain_len(&self, chunk: u64) -> usize {""",
    """    pub(crate) fn chunk_plain_len(&self, chunk: u64) -> usize {""")
# the writer, after encrypt_with_id
sub(C, """/// The header of the object `src` holds, after everything an open checks
/// before a tag:""", """/// SCRATCH(page): `encrypt` in version 2: each chunk's ciphertext a whole
/// stored chunk at its page, and the tags, in chunk order, in a region
/// right after the last ciphertext byte. Written by the vault alone and read
/// by the pager alone.
#[cfg(unix)]
pub(crate) fn encrypt_tags_apart<S, W, A>(
    src: &S,
    out: &mut W,
    aead: &A,
    opts: &EncryptOptions,
) -> Result<Header>
where
    S: Bytes + ?Sized,
    W: Write + ?Sized,
    A: Aead + ?Sized,
{
    use std::io::Read;
    let mut id = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut id)?;
    encrypt_tags_apart_with_id(src, out, aead, opts, id)
}

/// SCRATCH(page): `encrypt_tags_apart` with the id given, for tests.
pub(crate) fn encrypt_tags_apart_with_id<S, W, A>(
    src: &S,
    out: &mut W,
    aead: &A,
    opts: &EncryptOptions,
    id: [u8; 16],
) -> Result<Header>
where
    S: Bytes + ?Sized,
    W: Write + ?Sized,
    A: Aead + ?Sized,
{
    let hdr = Header::new_apart(src.len(), opts.chunk_shift, id, opts.key_id)?;
    let aad = hdr.encode();
    let mut buf = Vec::with_capacity(WRITE_PIECE + hdr.stored_chunk());
    let mut tags = Vec::with_capacity(hdr.chunks() as usize * TAG);
    buf.extend_from_slice(&aad);
    buf.resize(DATA_START as usize, 0);
    for c in 0..hdr.chunks() {
        let (at, n) = (buf.len(), hdr.chunk_plain_len(c));
        buf.resize(at + n, 0);
        src.read_at(hdr.plain_off(c), &mut buf[at..])?;
        let tag = aead.encrypt(&hdr.nonce(c), &aad, &mut buf[at..]);
        tags.extend_from_slice(&tag);
        let whole = buf.len() / WRITE_PIECE * WRITE_PIECE;
        for piece in buf[..whole].chunks(WRITE_PIECE) {
            out.write_all(piece)?;
        }
        buf.drain(..whole);
    }
    buf.extend_from_slice(&tags);
    for piece in buf.chunks(WRITE_PIECE) {
        out.write_all(piece)?;
    }
    Ok(hdr)
}

/// The header of the object `src` holds, after everything an open checks
/// before a tag:""")
APART_REFUSE = """    if hdr.apart() {
        return Err(refused(
            "an envelope with its tags apart is a store's file, read through its cache",
        ));
    }
"""
sub(C, """    let hdr = checked_header(src)?;
    let aad = hdr.encode();
    let mut buf = Vec::with_capacity(WRITE_PIECE + hdr.stored_chunk());
    let mut stored = Vec::new();""", """    let hdr = checked_header(src)?;
""" + APART_REFUSE + """    let aad = hdr.encode();
    let mut buf = Vec::with_capacity(WRITE_PIECE + hdr.stored_chunk());
    let mut stored = Vec::new();""")
sub(C, """        let hdr = checked_header(&src)?;
        #[cfg(target_family = "wasm")]""", """        let hdr = checked_header(&src)?;
        if hdr.apart() {
            return Err(refused(
                "an envelope with its tags apart is a store's file, read through its cache",
            ));
        }
        #[cfg(target_family = "wasm")]""")

# ---------------------------------------------------------------- pager.rs
P = "src/db/pager.rs"
sub(P, """pub(crate) const CHUNK: usize = (1 << CHUNK_SHIFT) - TAG;""",
    """/// SCRATCH(page): true, a chunk is a whole page of plaintext with its tag
/// apart (envelope version 2); false, today's 4080.
pub(crate) const PAGE_LAYOUT: bool = true;
pub(crate) const CHUNK: usize = if PAGE_LAYOUT {
    1 << CHUNK_SHIFT
} else {
    (1 << CHUNK_SHIFT) - TAG
};""")
sub(P, """/// Chunks in a unit: the least count whose plaintext is whole pages. A""",
    """/// SCRATCH(page): under PAGE_LAYOUT a unit is 256 whole pages, a megabyte,
/// and no longer the least count whose plaintext is whole pages (that is 1).
/// Chunks in a unit: the least count whose plaintext is whole pages. A""")
sub(P, """    breaths: AtomicU64,
    adopted_bytes: AtomicU64,
}""", """    breaths: AtomicU64,
    adopted_bytes: AtomicU64,
    // SCRATCH(page): the dynamic budget's range.
    limit_lo: AtomicU64,
    limit_hi: AtomicU64,
}""")
sub(P, """    /// budget, each twenty milliseconds at most (`StoreCipher::cache_bytes`).
    pub breaths: u64,
}""", """    /// budget, each twenty milliseconds at most (`StoreCipher::cache_bytes`).
    pub breaths: u64,
    /// SCRATCH(page): the dynamic budget's lowest and highest.
    pub limit_lo: u64,
    pub limit_hi: u64,
}""")
sub(P, """    /// Where the cache is `crowded`: its budget and the slack past it,
    /// worked out once, since every read of the store asks.
    hard: u64,
    /// Where it is `swamped`.
    swamp: u64,
    limit: AtomicU64,""", """    // SCRATCH(page): `hard` and `swamp` derived from the current limit; a
    // unit's bytes, kept so the derivation asks no page size.
    unit_bytes: u64,
    /// SCRATCH(page): the budget recomputed as this percentage of
    /// `zfile::read_budget().budget`, every 100 ms; 0 fixed.
    dyn_pct: u8,
    trace: bool,
    limit: AtomicU64,""")
sub(P, """    pub(crate) fn new(limit: u64) -> Arc<Pager> {
        let unit = (unit_chunks() * CHUNK) as u64;
        let p = Arc::new(Pager {
            hard: limit.saturating_add((limit / 16).max(4 * unit)),
            swamp: limit.saturating_add((limit / 2).max(6 * unit)),
            told""", """    #[allow(dead_code)] // SCRATCH(page)
    pub(crate) fn new(limit: u64) -> Arc<Pager> {
        Pager::new_with(limit, 0)
    }

    /// SCRATCH(page): `new` with the budget's mode.
    pub(crate) fn new_with(limit: u64, dyn_pct: u8) -> Arc<Pager> {
        let unit = (unit_chunks() * CHUNK) as u64;
        let trace = std::env::var_os("SUPDB_PAGER_TRACE").is_some_and(|v| v == "1");
        if trace {
            static SAID: AtomicBool = AtomicBool::new(false);
            if !SAID.swap(true, Ordering::Relaxed) {
                eprintln!("pagerlayout chunk {CHUNK} apart {PAGE_LAYOUT} unit {unit}");
            }
        }
        let p = Arc::new(Pager {
            unit_bytes: unit,
            dyn_pct,
            trace,
            told""")
sub(P, """            counts: Counts::default(),
        });
        let weak = Arc::downgrade(&p);""", """            counts: Counts::default(),
        });
        p.counts.limit_lo.store(limit, Ordering::Relaxed);
        p.counts.limit_hi.store(limit, Ordering::Relaxed);
        let weak = Arc::downgrade(&p);""")
sub(P, """            breaths: r(&c.breaths),
        }""", """            breaths: r(&c.breaths),
            limit_lo: r(&c.limit_lo),
            limit_hi: r(&c.limit_hi),
        }""")
sub(P, """        self.could_give() > self.hard""", """        let l = self.budget();
        self.could_give() > l.saturating_add((l / 16).max(4 * self.unit_bytes))""")
sub(P, """        self.could_give() > self.swamp""", """        let l = self.budget();
        self.could_give() > l.saturating_add((l / 2).max(6 * self.unit_bytes))""")
sub(P, """    fn run(me: Weak<Pager>) {
        loop {
            let Some(p) = me.upgrade() else {
                return;
            };
            if p.stop.load(Ordering::SeqCst) {
                return;
            }
""", """    fn run(me: Weak<Pager>) {
        let mut last_dyn = std::time::Instant::now() - Duration::from_secs(1);
        let mut last_trace = std::time::Instant::now();
        loop {
            let Some(p) = me.upgrade() else {
                return;
            };
            if p.stop.load(Ordering::SeqCst) {
                return;
            }
            // SCRATCH(page): the budget followed, and the trace.
            #[cfg(target_os = "linux")]
            if p.dyn_pct > 0 && last_dyn.elapsed() >= Duration::from_millis(100) {
                last_dyn = std::time::Instant::now();
                if let Some(b) = super::zfile::read_budget() {
                    let l = (b.budget / 100 * u64::from(p.dyn_pct)).max(64 << 20);
                    p.limit.store(l, Ordering::Relaxed);
                    p.counts.limit_lo.fetch_min(l, Ordering::Relaxed);
                    p.counts.limit_hi.fetch_max(l, Ordering::Relaxed);
                }
            }
            if p.trace && last_trace.elapsed() >= Duration::from_secs(1) {
                last_trace = std::time::Instant::now();
                let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                // SAFETY: a clock read into a local.
                unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
                let c = p.counts();
                eprintln!(
                    "pagertrace {}.{:09} held {} limit {} unbacked {} fills {} evicted {}",
                    ts.tv_sec, ts.tv_nsec, c.held, c.budget,
                    p.unbacked.load(Ordering::Relaxed), c.fills, c.evicted_units,
                );
            }
""")
sub(P, """        if hdr.chunk_shift() != CHUNK_SHIFT {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "an envelope of another chunk size than a store's files are written in",
            ));
        }""", """        if hdr.chunk_shift() != CHUNK_SHIFT || hdr.apart() != PAGE_LAYOUT {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "an envelope of another chunk size or layout than a store's files are written in",
            ));
        }
        debug_assert_eq!(hdr.chunk_len(), CHUNK);""")
sub(P, """        let n = self.chunk_len(c);
        let at = back.hdr.stored_off(c as u64) as usize;
        let stored = back.map.get(at..at + n + TAG).ok_or_else(|| {""", """        let n = self.chunk_len(c);
        let at = back.hdr.stored_off(c as u64) as usize;
        // SCRATCH(page): the ciphertext and the tag read apart, through the
        // mapping; under v1 the tag is right after the ciphertext.
        let t = back.hdr.tag_off(c as u64) as usize;
        let tg = back.map.get(t..t + TAG);
        let stored = back.map.get(at..at + n).filter(|_| tg.is_some()).ok_or_else(|| {""")
sub(P, """        dst.copy_from_slice(&stored[..n]);
        let tag: [u8; TAG] = stored[n..].try_into().expect("a tag's length");""",
    """        dst.copy_from_slice(stored);
        let tag: [u8; TAG] = tg.expect("checked").try_into().expect("a tag's length");""")
sub(P, """            let to = (back.hdr.stored_off(b as u64) as usize + self.chunk_len(b) + TAG)
                .min(back.map.len());""", """            let to = (back.hdr.stored_end(b as u64) as usize).min(back.map.len());""")
sub(P, """                let end = at + self.chunk_len(c) + TAG;""",
    """                let end = back.hdr.stored_end(c as u64) as usize;
                let _ = at;""")
sub(P, """        let hi =
            (back.hdr.stored_off(b as u64) as usize + self.chunk_len(b) + TAG).min(back.map.len());""",
    """        let hi = (back.hdr.stored_end(b as u64) as usize).min(back.map.len());""")
# tests
sub(P, """    fn envelope(path: &std::path::Path, bytes: &[u8], key: u64) {
        let mut out = File::create(path).unwrap();
        crypt::encrypt(
            &crate::bytes::SliceBytes(bytes),
            &mut out,
            &Toy(key),
            &EncryptOptions::default(),
        )
        .unwrap();""", """    fn envelope(path: &std::path::Path, bytes: &[u8], key: u64) {
        let mut out = File::create(path).unwrap();
        let src = crate::bytes::SliceBytes(bytes);
        if PAGE_LAYOUT {
            crypt::encrypt_tags_apart(&src, &mut out, &Toy(key), &EncryptOptions::default())
        } else {
            crypt::encrypt(&src, &mut out, &Toy(key), &EncryptOptions::default())
        }
        .unwrap();""")
sub(P, """        assert_eq!(CHUNK, 4080);""", """        assert_eq!(CHUNK, if PAGE_LAYOUT { 4096 } else { 4080 });
        if PAGE_LAYOUT {
            assert_eq!(bytes, 1 << 20);
        }""")
sub(P, """        assert_eq!(pager.held(), 5 * 4080 + (bytes.len() % 4080) as u64);""",
    """        assert_eq!(pager.held(), (5 * CHUNK + bytes.len() % CHUNK) as u64);""")
sub(P, """        assert_eq!(read(&p, 0, 4080).unwrap(), &bytes[..4080]);""",
    """        assert_eq!(read(&p, 0, CHUNK).unwrap(), &bytes[..CHUNK]);""")
sub(P, """            read(&p, 2 * 4080 + 5, 10).is_none(),""", """            read(&p, 2 * CHUNK as u64 + 5, 10).is_none(),""")
sub(P, """p.base.add(2 * 4080), 4080)""", """p.base.add(2 * CHUNK), CHUNK)""")
sub(P, """            read(&p, 4080, 10).is_some(),""", """            read(&p, CHUNK as u64, 10).is_some(),""")

# ---------------------------------------------------------------- vault.rs
V = "src/db/vault.rs"
sub(V, """    key_id: u64,
    cache: Option<u64>,
}

impl StoreCipher {""", """    key_id: u64,
    cache: Option<u64>,
    // SCRATCH(page): `Pager::new_with`'s budget mode.
    dyn_pct: u8,
}

impl StoreCipher {""")
sub(V, """            key_id: 0,
            cache: None,
        }
    }
""", """            key_id: 0,
            cache: None,
            dyn_pct: 0,
        }
    }

    /// SCRATCH(page): the budget recomputed as `pct` of the read budget.
    pub fn scratch_dynamic(mut self, pct: u8) -> StoreCipher {
        self.dyn_pct = pct;
        self
    }
""")
sub(V, """            pager: Pager::new(cipher.cache.unwrap_or_else(default_cache)),""",
    """            pager: Pager::new_with(cipher.cache.unwrap_or_else(default_cache), cipher.dyn_pct),""")
sub(V, """        if len == 0 {
            crypt::encrypt(&crate::bytes::SliceBytes(&[]), &mut out, &*aead, &opts)?;
        } else {
            // SAFETY: a mapping of a memory file the store owns, whose
            // writer has finished with it.
            let src = MmapBytes::from_map(unsafe { memmap2::Mmap::map(mem)? });
            crypt::encrypt(&src, &mut out, &*aead, &opts)?;
        }""", """        // SCRATCH(page): the tags apart under PAGE_LAYOUT.
        if len == 0 {
            let src = crate::bytes::SliceBytes(&[]);
            if pager::PAGE_LAYOUT {
                crypt::encrypt_tags_apart(&src, &mut out, &*aead, &opts)?;
            } else {
                crypt::encrypt(&src, &mut out, &*aead, &opts)?;
            }
        } else {
            // SAFETY: a mapping of a memory file the store owns, whose
            // writer has finished with it.
            let src = MmapBytes::from_map(unsafe { memmap2::Mmap::map(mem)? });
            if pager::PAGE_LAYOUT {
                crypt::encrypt_tags_apart(&src, &mut out, &*aead, &opts)?;
            } else {
                crypt::encrypt(&src, &mut out, &*aead, &opts)?;
            }
        }""")

# ---------------------------------------------------------------- engines.rs
E = "bench/src/engines.rs"
sub(E, """    /// `Policy::cipher_cache`.
    cipher_cache: u64,
""", """    /// `Policy::cipher_cache`.
    cipher_cache: u64,
    /// SCRATCH(page)
    cipher_dyn: u8,
""")
sub(E, """    cipher_cache: u64,
    /// The store under a cipher with ordered ingest off""", """    cipher_cache: u64,
    /// SCRATCH(page): `StoreCipher::scratch_dynamic`.
    cipher_dyn: u8,
    /// The store under a cipher with ordered ingest off""")
sub(E, """            cipher_cache: 0,
            cipher_log: false,""", """            cipher_cache: 0,
            cipher_dyn: 0,
            cipher_log: false,""")
ARMS = [("x432", 432, 0), ("x480", 480, 0), ("x520", 520, 0), ("x550", 550, 0),
        ("xdyn94", 0, 94), ("xdyn85", 0, 85)]
creates = "".join(f"""    /// SCRATCH(page): `supdb-cipher{n}`.
    pub fn create_cipher_{n}(path: &Path) -> Res<Supdb> {{
        Supdb::with_policy(
            path,
            Policy {{
                cipher: true,
                cipher_cache: {mb} << 20,
                cipher_dyn: {d},
                ..Policy::default()
            }},
        )
    }}

""" for n, mb, d in ARMS)
sub(E, """    /// `supdb-cipher` with a cache of sixty-four mebibytes
    /// (`Policy::cipher_cache`).
    pub fn create_cipher64""", creates + """    /// `supdb-cipher` with a cache of sixty-four mebibytes
    /// (`Policy::cipher_cache`).
    pub fn create_cipher64""")
sub(E, """            cipher_cache,
            cipher_log,
            partition,
            drain,""", """            cipher_cache,
            cipher_dyn,
            cipher_log,
            partition,
            drain,""", count=2)
sub(E, """                let c = supdb::StoreCipher::new(Gcm::new(), Gcm::of);""",
    """                let c = supdb::StoreCipher::new(Gcm::new(), Gcm::of).scratch_dynamic(cipher_dyn);""")
names = "".join(f"""                true if self.cipher_dyn == {d} && self.cipher_cache == {mb} << 20 => "supdb-cipher{n}",
""" for n, mb, d in ARMS)
sub(E, """                true if self.cipher_cache != 0 => "supdb-cipher64",""",
    names + """                true if self.cipher_cache != 0 => "supdb-cipher64",""")
sub(E, """            ("cipher_evictor_passes", pc.passes as f64),""", """            ("cipher_evictor_passes", pc.passes as f64),
            ("cipher_limit_lo", pc.limit_lo as f64),
            ("cipher_limit_hi", pc.limit_hi as f64),""")
sub(E, """        | "supdb-cipher64"
""", """        | "supdb-cipher64"
""" + "".join(f'        | "supdb-cipher{n}"\n' for n, _, _ in ARMS))
sub(E, """        "supdb-cipher64" => supdb_arm(Supdb::create_cipher64, dir)?,
""", """        "supdb-cipher64" => supdb_arm(Supdb::create_cipher64, dir)?,
""" + "".join(f'        "supdb-cipher{n}" => supdb_arm(Supdb::create_cipher_{n}, dir)?,\n' for n, _, _ in ARMS))

# ---------------------------------------------------------------- run.rs
R = "bench/src/run.rs"
sub(R, """    for letter in YCSB_ORDER {
        if !passes.mixes {
            break;
        }
        let w = YCSB.iter().find(|w| w.0 == letter).unwrap();""", """    // SCRATCH(page): `SUPDB_MIX_ONLY`, the mixes' letters in order; a
    // letter repeated is the mix again on the same store, `ycsb-C#2`.
    let order: Vec<char> = match std::env::var("SUPDB_MIX_ONLY") {
        Ok(s) if !s.is_empty() => s.chars().collect(),
        _ => YCSB_ORDER.to_vec(),
    };
    let mut seen: Vec<char> = Vec::new();
    for letter in order {
        if !passes.mixes {
            break;
        }
        let again = seen.iter().filter(|&&l| l == letter).count();
        seen.push(letter);
        let w = YCSB.iter().find(|w| w.0 == letter).unwrap();""")
sub(R, """        let mix = format!("ycsb-{letter}");
        let cb = counting.begin();""", """        let mix = if again == 0 {
            format!("ycsb-{letter}")
        } else {
            format!("ycsb-{letter}#{}", again + 1)
        };
        let cb = counting.begin();""")
sub(R, """        ycsb_ops_s.push((letter, mix_ops as f64 / secs));""", """        if again == 0 {
            ycsb_ops_s.push((letter, mix_ops as f64 / secs));
        } else {
            eprintln!("mixagain {arm} {mix} ops_per_s {:.1}", mix_ops as f64 / secs);
        }""")
print("ok")

# ---------------------------------------------------------------- new tests
sub(C, """    fn envelope(p: &[u8], shift: u32, id: u8, key: u64) -> Vec<u8> {""", """    /// SCRATCH(page): a version-2 header survives its encoding, says it is
    /// apart, holds a page a chunk, and its last tag ends the object.
    #[test]
    fn scratch_a_header_with_its_tags_apart_round_trips() {
        for len in [0u64, 1, 4095, 4096, 4097, 3 * (1 << 20) + 123] {
            let h = Header::new_apart(len, 12, [3; 16], 5).unwrap();
            let back = Header::parse(&h.encode()).unwrap();
            assert_eq!(back, h);
            assert!(back.apart());
            assert_eq!(back.chunk_len(), 4096);
            assert_eq!(back.tag_off(back.chunks() - 1) + TAG as u64, back.stored_len(), "{len}");
            let v1 = Header::new(len, 12, [3; 16], 5).unwrap();
            assert!(!Header::parse(&v1.encode()).unwrap().apart());
            assert_eq!(v1.tag_off(0), v1.stored_off(0) + v1.chunk_plain_len(0) as u64);
        }
    }

    /// SCRATCH(page): a reader of the public envelope refuses version 2 by
    /// name, and the writer's bytes decrypt chunk by chunk with the tags read
    /// from the region.
    #[test]
    fn scratch_an_envelope_with_its_tags_apart_is_refused_by_the_public_reader() {
        let p = plain(3 * 4096 + 77);
        let mut out = Vec::new();
        let hdr = encrypt_tags_apart_with_id(
            &SliceBytes(&p),
            &mut out,
            &Toy::new(4),
            &EncryptOptions::default(),
            [8; 16],
        )
        .unwrap();
        assert_eq!(out.len() as u64, hdr.stored_len());
        let e = Encrypted::open(SliceBytes(&out), Toy::new(4)).err().unwrap().to_string();
        assert!(e.contains("tags apart"), "{e}");
        let e = decrypt(&SliceBytes(&out), &mut Vec::new(), &Toy::new(4)).unwrap_err().to_string();
        assert!(e.contains("tags apart"), "{e}");
        let aad = hdr.encode();
        for c in 0..hdr.chunks() {
            let (a, n, t) = (hdr.stored_off(c) as usize, hdr.chunk_plain_len(c), hdr.tag_off(c) as usize);
            let mut buf = out[a..a + n].to_vec();
            let tag: [u8; TAG] = out[t..t + TAG].try_into().unwrap();
            assert!(Toy::new(4).decrypt(&hdr.nonce(c), &aad, &mut buf, &tag), "{c}");
            assert_eq!(buf, &p[hdr.plain_off(c) as usize..][..n]);
        }
    }

    fn envelope(p: &[u8], shift: u32, id: u8, key: u64) -> Vec<u8> {""")
sub(P, """    /// A chunk changed on the device fails its read and is remembered: the""", """    /// SCRATCH(page): a tag changed in the tag region fails its chunk alone,
    /// and the last byte of the file is the last chunk's tag.
    #[test]
    fn a_changed_tag_in_the_tag_region_fails_its_chunk() {
        if !PAGE_LAYOUT {
            return;
        }
        for (which, last) in [(3u64, false), (0, true)] {
            let dir = scratch(if last { "tagend" } else { "tag3" });
            let bytes = plain(8 * 4096 + 321, 9);
            envelope(&dir.join("e"), &bytes, 23);
            let mut raw = std::fs::read(dir.join("e")).unwrap();
            let hdr = Header::parse(&raw).unwrap();
            assert!(hdr.apart());
            let c = if last { hdr.chunks() - 1 } else { which };
            let at = if last { raw.len() - 1 } else { hdr.tag_off(c) as usize };
            raw[at] ^= 1;
            std::fs::write(dir.join("e"), &raw).unwrap();
            let pager = Pager::new(u64::MAX);
            let p = sparse(&pager, &dir.join("e"), 23);
            let _h = pager.hold();
            for k in 0..3u64 {
                assert_eq!(
                    read(&p, k * CHUNK as u64, 10).unwrap(),
                    &bytes[k as usize * CHUNK..][..10]
                );
            }
            assert!(pager.check().is_ok());
            assert!(read(&p, c * CHUNK as u64 + 1, 5).is_none(), "the chunk read");
            let e = pager.check().unwrap_err().to_string();
            assert!(e.contains(&format!("chunk {c} failed its check")), "{e}");
            drop(p);
            pager.stop();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A chunk changed on the device fails its read and is remembered: the""")
print("tests ok")
