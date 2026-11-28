//! The readers held to bytes made to break them: the five ways the
//! pre-tag hostile-input audit found to make a reader answer wrong, panic
//! or allocate what the object cannot back, each built as the audit's
//! harness built it and each refused now.
//!
//! Every case runs every segment reader against the others -- `Blob` over
//! a lent slice, over a copying source and over a file mapping, and
//! `SparseBlob` -- and each is held to what the writer was given: a reader
//! that opens answers every key as written, or errors. The browser reader
//! is held to the same cases through `web/hostile.test.mjs`, which reads
//! what `SUPDB_HOSTILE_DUMP` asks the dump test here to write.
//!
//! The binary is its own, because it counts a process-wide effect: the
//! largest single allocation any reader asks for.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use supdb::bytes::{Bytes, MmapBytes, SliceBytes};
use supdb::flatindex;
use supdb::{Blob, SegmentOptions, SegmentWriter, SparseBlob};

mod common;
use common::{kv, vf};

/// The largest single allocation since the last `take_peak`.
struct Peak;
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        PEAK.fetch_max(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        PEAK.fetch_max(l.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        PEAK.fetch_max(n, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOC: Peak = Peak;

/// What one reader may ask for in one allocation over these segments,
/// which are a few hundred kilobytes: anything near it is a length taken
/// from the file and believed.
const ALLOC_BOUND: usize = 64 << 20;

fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn scratch(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "supdb-next-hostile-{name}{n}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A source that copies, so the readers that own their sections run as
/// well as the ones that borrow them.
struct Copying(Vec<u8>);

impl Bytes for Copying {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        SliceBytes(&self.0).read_at(off, dst)
    }
}

type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

/// A segment in each of the audit's six shapes: the default; every value in
/// blocks; compressed; 512-byte blocks; checksums off; compact records off.
/// Each holds the empty key and a run of `0xff`, runs of none, a few and
/// thirty values, fixed-width runs, and empty and 3,000-byte values.
fn shapes() -> Vec<(Vec<u8>, Model)> {
    let dir = scratch("shapes");
    let mut out = Vec::new();
    for shape in 0..6u64 {
        let path = dir.join(format!("seg-{shape}.sup"));
        let opts = SegmentOptions {
            block_size: if shape == 3 { 512 } else { 4096 },
            checksums: shape != 4,
            compact_records: shape != 5,
            ..SegmentOptions::default()
        };
        let mut w = SegmentWriter::create(&path, &opts).unwrap();
        w.set_inline_max(if shape == 1 { 0 } else { 64 });
        w.set_compress(shape == 2);
        let mut r = 0x5eed ^ shape;
        let mut keys: Vec<Vec<u8>> = (0..300)
            .map(|i| {
                let pad = (splitmix(&mut r) % 24) as usize;
                format!("k{i:05}{}", "x".repeat(pad)).into_bytes()
            })
            .chain([Vec::new(), vec![0xff; 33]])
            .collect();
        keys.sort();
        keys.dedup();
        let mut m = Model::new();
        for k in keys {
            w.begin(&k).unwrap();
            let n = match splitmix(&mut r) % 10 {
                0 => 0,
                1 => 30,
                _ => 1 + (splitmix(&mut r) % 3) as usize,
            };
            let fixed = splitmix(&mut r).is_multiple_of(4);
            let mut vs = Vec::new();
            for _ in 0..n {
                let len = match (fixed, splitmix(&mut r) % 20) {
                    (true, _) => 8,
                    (false, 0) => 3000,
                    (false, 1) => 0,
                    _ => 8 + (splitmix(&mut r) % 120) as usize,
                };
                let v: Vec<u8> = (0..len).map(|_| splitmix(&mut r) as u8).collect();
                w.value(&v);
                vs.push(v);
            }
            w.end().unwrap();
            m.insert(k, vs);
        }
        w.finish(1).unwrap();
        out.push((std::fs::read(&path).unwrap(), m));
    }
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// The segment's key section: its offset (superblock field 3) and the
/// offset of its checksum row within it.
fn key_section(b: &[u8]) -> (usize, usize) {
    let ko = u64::from_le_bytes(b[24..32].try_into().unwrap()) as usize;
    let co = u64::from_le_bytes(b[ko + 160..ko + 168].try_into().unwrap()) as usize;
    (ko, co)
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in bytes {
        h ^= x as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Both superblock slots' FNV computed again over their fields and magic,
/// as a forger does.
fn forge_super(b: &mut [u8]) {
    for slot in [0usize, 512] {
        let h = fnv(&b[slot..slot + 136]);
        b[slot + 136..slot + 144].copy_from_slice(&h.to_le_bytes());
    }
}

/// The superblock extension's copy of the key header (at 1024, after its
/// twenty words) with `f` applied, and its FNV computed again.
fn forge_ext_header(b: &mut [u8], f: impl FnOnce(&mut [u8])) {
    let (at, n) = (1024, supdb::blob::SX_BYTES);
    f(&mut b[at + 160..at + 160 + flatindex::HEADER_BYTES]);
    let h = fnv(&b[at..at + n - 8]);
    b[at + n - 8..at + n].copy_from_slice(&h.to_le_bytes());
}

/// The key section's row computed again over the section as it stands.
fn forge_row(b: &mut [u8]) {
    let (ko, co) = key_section(b);
    let row = flatindex::checksum_row(&b[ko..ko + co], flatindex::PIECE_SHIFT, ko as u64);
    b[ko + co..ko + co + row.len()].copy_from_slice(&row);
}

/// What one reader did with one case.
#[derive(Debug, PartialEq)]
enum Verdict {
    Refused,
    Answered,
    /// Opened and answered something that was never written.
    Misread(String),
}

fn blob_verdict<B: Bytes>(open: std::io::Result<Blob<B>>, m: &Model) -> Verdict {
    let Ok(blob) = open else {
        return Verdict::Refused;
    };
    for (k, want) in m {
        let mut got: Vec<Vec<u8>> = Vec::new();
        if let Ok(n) = blob.read_all(k, vf(&mut |v| got.push(v.to_vec()))) {
            if got != *want || n as usize != want.len() {
                return Verdict::Misread(format!("read_all({k:02x?}) gave {} values", got.len()));
            }
        }
        if let Ok(c) = blob.count(k) {
            if c as usize != want.len() {
                return Verdict::Misread(format!("count({k:02x?}) = {c}"));
            }
        }
    }
    let mut seen = Vec::new();
    if blob
        .scan(
            b"",
            usize::MAX,
            kv(&mut |k, v| seen.push((k.to_vec(), v.to_vec()))),
        )
        .is_ok()
    {
        let want: Vec<(Vec<u8>, Vec<u8>)> = m
            .iter()
            .flat_map(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
            .collect();
        if seen != want {
            return Verdict::Misread(format!("scan gave {} pairs of {}", seen.len(), want.len()));
        }
    }
    Verdict::Answered
}

fn sparse_verdict<B: Bytes>(open: std::io::Result<SparseBlob<B>>, m: &Model) -> Verdict {
    let Ok(sp) = open else {
        return Verdict::Refused;
    };
    let mut walked = Vec::new();
    let r = sp.dictionary_counts(b"", None, |k: &[u8], n: u64| {
        walked.push((k.to_vec(), n));
        true
    });
    let want: Vec<(Vec<u8>, u64)> = m.iter().map(|(k, v)| (k.clone(), v.len() as u64)).collect();
    if r.is_ok() && walked != want {
        return Verdict::Misread(format!("dictionary_counts gave {} keys", walked.len()));
    }
    Verdict::Answered
}

/// Every reader over `b`, by name, each with the largest allocation it
/// asked for.
fn every_reader(b: &[u8], m: &Model) -> Vec<(&'static str, Verdict, usize)> {
    let path = std::env::temp_dir().join(format!(
        "supdb-next-hostile-map-{}-{:?}.sup",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, b).unwrap();
    let mapped = MmapBytes::open(&path);
    let _ = std::fs::remove_file(&path);
    let mut out = Vec::new();
    let mut run = |name: &'static str, f: &mut dyn FnMut() -> Verdict| {
        PEAK.store(0, Ordering::Relaxed);
        let v = f();
        out.push((name, v, PEAK.load(Ordering::Relaxed)));
    };
    run("Blob(lent)", &mut || {
        blob_verdict(Blob::open(SliceBytes(b)), m)
    });
    run("Blob(copying)", &mut || {
        blob_verdict(Blob::open(Copying(b.to_vec())), m)
    });
    let mut mapped = Some(mapped);
    run("Blob(mapped)", &mut || match mapped.take().unwrap() {
        Ok(src) => blob_verdict(Blob::open(src), m),
        Err(_) => Verdict::Refused,
    });
    run("SparseBlob(lent)", &mut || {
        sparse_verdict(SparseBlob::open(SliceBytes(b)), m)
    });
    run("SparseBlob(copying)", &mut || {
        sparse_verdict(SparseBlob::open(Copying(b.to_vec())), m)
    });
    out
}

/// One case: no reader misreads or allocates past the bound, and where
/// `refuse` says so every reader refuses.
fn hold(name: &str, shape: usize, b: &[u8], m: &Model, refuse: bool) {
    for (reader, v, peak) in every_reader(b, m) {
        assert!(
            peak <= ALLOC_BOUND,
            "{name}: {reader} asked for {peak} bytes in one allocation"
        );
        if refuse {
            assert_eq!(v, Verdict::Refused, "{name}: {reader} opened");
        } else {
            assert!(!matches!(v, Verdict::Misread(_)), "{name}: {reader}: {v:?}");
        }
    }
    dump(name, shape, b, refuse);
}

/// The fixture's premise: every pristine shape opens in every reader and
/// answers what was written, and names its row.
#[test]
fn every_reader_answers_every_pristine_shape() {
    for (i, (b, m)) in shapes().iter().enumerate() {
        assert!(key_section(b).1 > 0, "shape {i} names a row");
        dump_model(i, m);
        dump(&format!("pristine shape{i}"), i, b, false);
        for (reader, v, _) in every_reader(b, m) {
            assert_eq!(v, Verdict::Answered, "shape {i}: {reader}");
        }
    }
}

/// HI-1. A key section's `crc_off` word zeroed, in the section and in the
/// extension's copy of its header, and one bit flipped behind the header:
/// a row named as absent turned verification off and the flip was answered
/// as fact. Every segment names its row, so every reader refuses.
#[test]
fn a_key_section_that_names_no_row_is_refused() {
    for (i, (pristine, m)) in shapes().iter().enumerate() {
        let (ko, co) = key_section(pristine);
        for seed in 0..20u64 {
            let mut b = pristine.clone();
            b[ko + 160..ko + 168].fill(0);
            let mut r = seed ^ 0xc0ff;
            if seed >= 1 {
                let at = ko + 192 + (splitmix(&mut r) % (co - 192) as u64) as usize;
                b[at] ^= 1 << (splitmix(&mut r) % 8);
            }
            // The section's word alone: the sparse reader takes the header
            // from the extension's copy, which still names the row, and so
            // verifies what it reads; nobody may answer the flip.
            hold(
                &format!("crcoff shape{i} seed {seed} section"),
                i,
                &b,
                m,
                false,
            );
            forge_ext_header(&mut b, |h| h[160..168].fill(0));
            hold(&format!("crcoff shape{i} seed {seed}"), i, &b, m, true);
        }
    }
}

/// Superblock field `i` of both slots set to `v`.
fn set_super(b: &mut [u8], i: usize, v: u64) {
    for slot in [0usize, 512] {
        b[slot + i * 8..slot + i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
}

/// HI-2. Lengths in the file that sized an allocation before anything
/// held them to the object: a block table entry's stored length
/// (`bytes::take`'s scratch) and its uncompressed length (`Blob`'s decode
/// buffer), which no checksum covers, and the superblock's section lengths
/// under a forged FNV (`Sec::read`), which panicked on a capacity overflow
/// past `isize::MAX` and asked for the forged size below it. No reader asks
/// for more than the bound; a section past the high water is refused.
#[test]
fn a_length_from_the_file_sizes_nothing_the_object_cannot_hold() {
    for (i, (pristine, m)) in shapes().iter().enumerate() {
        let blk_off = u64::from_le_bytes(pristine[48..56].try_into().unwrap()) as usize;
        let n = u64::from_le_bytes(pristine[blk_off + 8..blk_off + 16].try_into().unwrap());
        assert!(n > 0, "shape {i} has blocks");
        for e in 0..n.min(6) as usize {
            let at = blk_off + 16 + e * 32;
            for (field, v) in [
                (8, 0xffff_ffffu32),
                (8, 0x7fff_ffff),
                (12, 0xffff_fff0),
                (12, 0x7fff_ffff),
            ] {
                let mut b = pristine.clone();
                b[at + field..at + field + 4].copy_from_slice(&v.to_le_bytes());
                let what = if field == 8 { "stored" } else { "uncompressed" };
                hold(
                    &format!("block shape{i} entry {e} {what}={v:#x}"),
                    i,
                    &b,
                    m,
                    false,
                );
            }
        }
        // The key section (fields 4 and 5) and the block table (7 and 8),
        // each length set in both its words so the open does not refuse it
        // as compressed first.
        for (stored, unc) in [(4, 5), (7, 8)] {
            for v in [3u64 << 30, 1 << 62, u64::MAX - 4095, u64::MAX] {
                let mut b = pristine.clone();
                set_super(&mut b, stored, v);
                set_super(&mut b, unc, v);
                forge_super(&mut b);
                hold(
                    &format!("superblock shape{i} f{stored}={v:#x}"),
                    i,
                    &b,
                    m,
                    true,
                );
            }
        }
    }
}

/// The superblock extension's word `i` (at 1024) set to `v`, its FNV
/// computed again.
fn forge_ext_word(b: &mut [u8], i: usize, v: u64) {
    let (at, n) = (1024, supdb::blob::SX_BYTES);
    b[at + i * 8..at + i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    let h = fnv(&b[at..at + n - 8]);
    b[at + n - 8..at + n].copy_from_slice(&h.to_le_bytes());
}

/// The sparse open's two plans over `b`, as a host that fetches by range
/// asks for them: each refuses or names ranges inside the object.
fn plans_hold(name: &str, b: &[u8]) {
    let len = b.len() as u64;
    let inside = |v: &[(u64, u64)]| {
        v.iter()
            .all(|&(o, l)| o.checked_add(l).is_some_and(|e| e <= len))
    };
    for directory in [false, true] {
        if let Ok(v) = supdb::blob::open_sparse_ranges_opts(&b[..4096], len, directory) {
            assert!(inside(&v), "{name}: the first plan names {v:?}");
        }
        if let Ok(v) = supdb::blob::sparse_open_ranges_via_opts(&SliceBytes(b), directory) {
            assert!(inside(&v), "{name}: the first plan names {v:?}");
        }
        let ko = u64::from_le_bytes(b[24..32].try_into().unwrap()) as usize;
        if let Some(h) = b.get(ko..ko.saturating_add(flatindex::HEADER_BYTES)) {
            if let Ok(v) = supdb::blob::open_sparse_fence_ranges_opts(&b[..4096], len, h, directory)
            {
                assert!(inside(&v), "{name}: the fence plan names {v:?}");
            }
        }
    }
}

/// HI-3. The sparse open summed offsets out of the file unchecked: the
/// superblock's `key_off` at the top of the u64 range under a forged FNV
/// overflowed the sum of it and the fence's offset (a panic in a checked
/// build, an index out of the verified-piece bitmap in release), and the
/// open plans summed the extension's ranges and the key header's offsets
/// the same way. Every sum is checked now, and every planned range is held
/// to the object.
#[test]
fn an_offset_at_the_top_of_the_range_panics_no_sparse_open() {
    for (i, (pristine, m)) in shapes().iter().enumerate() {
        for v in [u64::MAX - 4095, u64::MAX] {
            for both in [false, true] {
                let mut b = pristine.clone();
                b[24..32].copy_from_slice(&v.to_le_bytes());
                if both {
                    b[512 + 24..512 + 32].copy_from_slice(&v.to_le_bytes());
                }
                forge_super(&mut b);
                let name = format!("key_off shape{i} {v:#x} both {both}");
                plans_hold(&name, &b);
                hold(&name, i, &b, m, true);
            }
        }
        // The extension's fence, directory, hash and row ranges (words 2
        // to 9) and its copies' offsets (10, 12, 15, 16).
        for w in [2usize, 3, 4, 5, 6, 7, 8, 9, 10, 12, 15, 16] {
            for v in [u64::MAX - 4095, u64::MAX, 1 << 62] {
                let mut b = pristine.clone();
                forge_ext_word(&mut b, w, v);
                let name = format!("extension shape{i} w{w}={v:#x}");
                plans_hold(&name, &b);
                hold(&name, i, &b, m, false);
            }
        }
        // With the extension gone, the fence plan reads the key header:
        // its directory and row offsets at the top of the range, the row
        // computed again.
        let (ko, _) = key_section(pristine);
        for w in [10usize, 15, 20] {
            for v in [u64::MAX - 4095, u64::MAX] {
                let mut b = pristine.clone();
                b[1024..1032].fill(0);
                b[ko + w * 8..ko + w * 8 + 8].copy_from_slice(&v.to_le_bytes());
                if w != 20 {
                    forge_row(&mut b);
                }
                let name = format!("key header shape{i} w{w}={v:#x}");
                plans_hold(&name, &b);
                hold(&name, i, &b, m, false);
            }
        }
    }
}

/// HI-4. A u64 header word narrowed to `usize`: on wasm32 `2^32` read as
/// 0, and the key header's `recs_cap` (word 13) at `2^32` or `2^62` made
/// the parse `clamp` with its minimum past its maximum, which panicked,
/// and under the browser module's `panic = "abort"` killed the instance.
/// Every word of the key header from the key count on is set past 32 bits
/// with the row computed again, and the superblock's lengths under a
/// forged FNV: a 64-bit reader refuses or answers as written, and the
/// browser reader (`web/hostile.test.mjs`) the same, never trapping.
#[test]
fn a_header_word_past_32_bits_is_refused_and_never_narrowed() {
    for (i, (pristine, m)) in shapes().iter().enumerate() {
        let (ko, _) = key_section(pristine);
        for w in 7..20usize {
            for v in [1u64 << 32, (1 << 32) + 1, 1 << 62, u64::MAX] {
                let mut b = pristine.clone();
                b[ko + w * 8..ko + w * 8 + 8].copy_from_slice(&v.to_le_bytes());
                forge_row(&mut b);
                // The extension's copy, which the sparse open reads first.
                forge_ext_header(&mut b, |h| {
                    h[w * 8..w * 8 + 8].copy_from_slice(&v.to_le_bytes())
                });
                hold(&format!("keyheader shape{i} w{w}={v:#x}"), i, &b, m, false);
            }
        }
        for f in [4usize, 7] {
            for v in [1u64 << 32, (1 << 32) + 4096] {
                let mut b = pristine.clone();
                set_super(&mut b, f, v);
                set_super(&mut b, f + 1, v);
                forge_super(&mut b);
                hold(&format!("superblock shape{i} f{f}={v:#x}"), i, &b, m, true);
            }
        }
    }
}

/// The cases, for the browser reader (`web/hostile.test.mjs`): each case's
/// bytes and whether every reader must refuse it, under the directory
/// `SUPDB_HOSTILE_DUMP` names. Every other test here writes its cases there
/// when it is set; this one writes the pristine shapes and their models.
fn dump(name: &str, shape: usize, b: &[u8], refuse: bool) {
    let Some(d) = std::env::var_os("SUPDB_HOSTILE_DUMP") else {
        return;
    };
    use std::io::Write;
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = PathBuf::from(d);
    std::fs::create_dir_all(&d).unwrap();
    let n = N.fetch_add(1, Ordering::Relaxed);
    let file = format!("{}-{n}.sup", std::process::id());
    std::fs::write(d.join(&file), b).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.join("cases.tsv"))
        .unwrap();
    let verdict = if refuse { "refuse" } else { "hold" };
    // One write a line, under a lock: the tests run on threads of their
    // own, and a formatted write is several.
    static LINE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LINE.lock().unwrap();
    f.write_all(format!("{file}\t{shape}\t{verdict}\t{name}\n").as_bytes())
        .unwrap();
}

/// Shape `i`'s model for the browser reader: each key, in hex, and its
/// count of values.
fn dump_model(i: usize, m: &Model) {
    let Some(d) = std::env::var_os("SUPDB_HOSTILE_DUMP") else {
        return;
    };
    let d = PathBuf::from(d);
    std::fs::create_dir_all(&d).unwrap();
    let rows: Vec<String> = m
        .iter()
        .map(|(k, v)| {
            let hex: String = k.iter().map(|b| format!("{b:02x}")).collect();
            format!("[\"{hex}\",{}]", v.len())
        })
        .collect();
    std::fs::write(
        d.join(format!("shape{i}.json")),
        format!("[{}]", rows.join(",")),
    )
    .unwrap();
}

// -------------------------------------------------------------- the store

/// Every value of `key` the store answers.
fn values(db: &supdb::Db, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    db.read_all(key, vf(&mut |v| out.push(v.to_vec()))).unwrap();
    out
}

/// Every key the store's scan answers, with its value's length.
fn scanned(db: &supdb::Db) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| out.push((k.len(), v.len()))),
    )
    .unwrap();
    out
}

fn refused(r: std::io::Result<()>, what: &str) {
    let e = r.expect_err(what);
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{what}: {e}");
    assert!(e.to_string().contains("longer than"), "{what}: {e}");
}

/// A handle's writes (`Options::shared_writers`) of `key` and `value`, one
/// of them past its limit: each refused with nothing written.
fn handle_refuses(key: &[u8], value: &[u8]) {
    let dir = scratch("handle");
    let mut db = supdb::Db::open(
        &dir,
        supdb::Options {
            sync: supdb::SyncPolicy::Explicit,
            held_frames: false,
            scan_block_cache: false,
            upkeep: supdb::Upkeep::Inline,
            direct_ingest: false,
            shared_writers: true,
            ..common::options()
        },
    )
    .unwrap();
    let h = db.writer().unwrap();
    let mut answers = vec![h.put(key, value), h.append(key, value)];
    if value.len() <= supdb::MAX_VALUE_LEN {
        answers.push(h.delete(key));
    }
    for r in answers {
        let e = r.expect_err("a handle's write");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{e}");
    }
    drop(h);
    db.commit().unwrap();
    assert!(values(&db, key).is_empty());
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// HI-5. A key over 65,535 bytes was stored in release as another key: its
/// length went into a 16-bit word, so 65,536 bytes landed on the empty key
/// and 70,000 on its first 4,464 bytes, and a checked build panicked. Every
/// write entry point refuses it now -- `put`, `append`, `delete`, both
/// batches, a transaction, a handle's write, an ordered load's next key,
/// and the public segment writer -- with nothing of the write taken, and
/// 65,535 bytes round-trips through the live table, a flush and a reopen.
/// In a store in the clear and in a store in one file.
#[test]
fn a_key_past_65535_bytes_is_refused_at_every_write() {
    for one_file in [false, true] {
        let dir = scratch(if one_file { "bigkey-one" } else { "bigkey" });
        let opts = || supdb::Options {
            one_file,
            ..common::options()
        };
        let mut db = supdb::Db::open(&dir, opts()).unwrap();
        // An ordered load first, so the last refusal below is an ordered
        // load's next key.
        for i in 0..100u32 {
            db.append(format!("a{i:04}").as_bytes(), b"v");
        }
        db.commit().unwrap();
        // The load is a direct run, and its next key past the limit is
        // refused with the run left open.
        assert!(
            db.run_synced_bytes().is_some(),
            "the load is an ordered run"
        );
        let mut last = vec![b'z'; supdb::MAX_KEY_LEN + 1];
        last[0] = b'~';
        db.append(&last, b"4");
        refused(db.commit(), "an ordered load's next key");
        assert!(db.run_synced_bytes().is_some(), "the refusal left the run");
        assert!(values(&db, &last).is_empty());
        db.append(b"", b"the empty key's");
        db.commit().unwrap();
        let edge = vec![b'k'; supdb::MAX_KEY_LEN];
        db.put(&edge, b"edge");
        db.commit().unwrap();
        for n in [supdb::MAX_KEY_LEN + 1, 70_000] {
            let long = vec![b'k'; n];
            db.append(&long, b"long");
            refused(db.commit(), "append");
            db.put(&long, b"long");
            refused(db.commit(), "put");
            db.delete(&long);
            refused(db.commit(), "delete");
            // A batch is refused whole: its key that fits is not written.
            db.append_batch(&[(b"b-fits", b"1"), (&long, b"2")]);
            refused(db.commit(), "append_batch");
            db.put_batch(&[(b"c-fits", b"1"), (&long, b"2")]);
            refused(db.commit(), "put_batch");
            let mut t = db.begin();
            t.append(b"d-fits", b"1");
            t.append(&long, b"2");
            refused(t.commit(), "a transaction");
            // Nothing of any of them landed, on the key or on another.
            db.commit().unwrap();
            assert!(values(&db, &long).is_empty(), "{n}: the long key");
            assert!(values(&db, &long[..n % 65_536]).is_empty() || n % 65_536 == 0);
            assert_eq!(values(&db, b""), vec![b"the empty key's".to_vec()], "{n}");
            for k in [&b"b-fits"[..], b"c-fits", b"d-fits"] {
                assert!(values(&db, k).is_empty(), "{n}: {k:?}");
            }
        }
        let want = |db: &supdb::Db| {
            assert_eq!(values(db, &edge), vec![b"edge".to_vec()]);
            assert_eq!(values(db, b""), vec![b"the empty key's".to_vec()]);
            let s = scanned(db);
            assert_eq!(s.len(), 102, "{s:?}");
            assert!(s.iter().all(|&(k, _)| k <= supdb::MAX_KEY_LEN));
        };
        want(&db);
        db.flush().unwrap();
        want(&db);
        db.close().unwrap();
        let db = supdb::Db::open(&dir, opts()).unwrap();
        want(&db);
        db.close().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
    handle_refuses(&vec![b'k'; supdb::MAX_KEY_LEN + 1], b"v");
    // The public segment writer.
    let dir = scratch("bigkey-seg");
    let long = vec![b'k'; supdb::MAX_KEY_LEN + 1];
    let opts = SegmentOptions::default();
    let mut w = SegmentWriter::create(&dir.join("a.sup"), &opts).unwrap();
    assert!(w.begin(&long).is_err(), "begin");
    let mut w = SegmentWriter::create(&dir.join("b.sup"), &opts).unwrap();
    assert!(w.put_one(&long, b"v").is_err(), "put_one");
    let _ = std::fs::remove_dir_all(&dir);
}

/// HI-5's values: a value's length is a 32-bit word in the live table, so a
/// value past 4 GiB would be stored as a shorter one. Refused at the store's
/// writes as a long key is, its bytes never touched: the value is a mapping
/// of a sparse file, so no memory backs it.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_value_past_4_gib_is_refused_at_every_write() {
    let dir = scratch("bigvalue");
    let path = dir.join("value");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(supdb::MAX_VALUE_LEN as u64 + 1).unwrap();
    drop(f);
    let map = MmapBytes::open(&path).unwrap();
    let big = map.slice_at(0, supdb::MAX_VALUE_LEN + 1).unwrap();
    let store = dir.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let mut db = supdb::Db::open(&store, common::options()).unwrap();
    db.append(b"k", big);
    refused(db.commit(), "append");
    db.put(b"k", big);
    refused(db.commit(), "put");
    db.append_batch(&[(b"j", b"1"), (b"k", big)]);
    refused(db.commit(), "append_batch");
    db.put_batch(&[(b"j", b"1"), (b"k", big)]);
    refused(db.commit(), "put_batch");
    db.commit().unwrap();
    assert!(values(&db, b"k").is_empty() && values(&db, b"j").is_empty());
    db.close().unwrap();
    handle_refuses(b"k", big);
    drop(map);
    let _ = std::fs::remove_dir_all(&dir);
}
