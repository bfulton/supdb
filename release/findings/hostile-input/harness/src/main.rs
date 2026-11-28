//! Hostile input for every supdb reader: bytes an attacker made, fed to the
//! segment readers (`Blob` over a lending source, `Blob` over a copying
//! source, `SparseBlob`), to `Db::open` over a mutated store directory (its
//! segments, both manifest slots, its WAL), to a store in one file, to a
//! store under a cipher, and to the `crypt` envelope reader.
//!
//! A reader must return an error, never panic, hang, allocate without
//! bound, read out of bounds or answer something that was never written.
//! Each case runs on a thread of its own under a clock and `catch_unwind`;
//! a single allocation over `HOSTILE_ALLOC_MAX` (default 1 GiB) aborts the
//! process with the case named, and so does a case that does not return.
//!
//!     cargo run --release -- TARGET FROM TO
//!
//! TARGET: segment | segforge | store | wal | onefile | cipher | envelope
//! Every case prints a line only when it fails: `PANIC`, `HANG`, `ALLOC`,
//! `MISREAD` (a value never written for that key, from a store that opened
//! and answered without an error). The last line counts the outcomes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use supdb::bytes::{Bytes, SliceBytes};
use supdb::crypt::{self, Aead, EncryptOptions, Encrypted};
use supdb::MmapBytes;
use supdb::{
    flatindex, Blob, Db, Encryption, Options, SegmentOptions, SegmentWriter, SparseBlob,
    StoreCipher,
};

// ---------------------------------------------------------------- allocator

struct Capped;
static ALLOC_MAX: AtomicUsize = AtomicUsize::new(256 << 20);
static CASE: Mutex<String> = Mutex::new(String::new());

static BIG: AtomicUsize = AtomicUsize::new(0);
static BT_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const HARD_MAX: usize = 16 << 30;

unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        over(l.size());
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        over(l.size());
        System.alloc_zeroed(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        over(n);
        System.realloc(p, l, n)
    }
}

/// An allocation over the cap is recorded against the case (the largest
/// one), and its first backtrace printed; one over 16 GiB aborts.
fn over(n: usize) {
    if n <= ALLOC_MAX.load(Ordering::Relaxed) {
        return;
    }
    BIG.fetch_max(n, Ordering::Relaxed);
    if n > HARD_MAX {
        let _ = writeln!(std::io::stderr(), "ALLOC {n} bytes: aborting");
        std::process::abort();
    }
    if !BT_DONE.swap(true, Ordering::Relaxed) && std::env::var_os("HOSTILE_BT").is_some() {
        let old = ALLOC_MAX.swap(usize::MAX, Ordering::Relaxed);
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let mut e = std::io::stderr();
        let _ = writeln!(e, "ALLOC {n} bytes, first such backtrace:");
        for l in bt
            .lines()
            .filter(|l| l.contains("supdb") && !l.contains("hostile"))
            .take(30)
        {
            let _ = writeln!(e, "  {l}");
        }
        ALLOC_MAX.store(old, Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Capped = Capped;

// ---------------------------------------------------------------- utilities

fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

struct Copying(Vec<u8>);
impl Bytes for Copying {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        SliceBytes(&self.0).read_at(off, dst)
    }
}

#[derive(Default)]
struct Tally {
    cases: u64,
    opened: u64,
    refused: u64,
    fails: Vec<String>,
}

static LAST_PANIC: Mutex<String> = Mutex::new(String::new());

/// Run `f` on a thread of its own with a clock on it. A panic is recorded
/// with its message and place; a hang ends the process, since the thread
/// cannot be stopped.
fn case<F: FnOnce() -> Outcome + Send + 'static>(t: &mut Tally, name: String, f: F) {
    *CASE.lock().unwrap() = name.clone();
    if std::env::var_os("HOSTILE_TRACE").is_some() {
        eprintln!("CASE {name}");
    }
    t.cases += 1;
    BIG.store(0, Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel();
    let th = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = tx.send(r);
        })
        .unwrap();
    let secs = std::env::var("HOSTILE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    match rx.recv_timeout(Duration::from_secs(secs)) {
        Ok(Ok(o)) => {
            let _ = th.join();
            let big = BIG.swap(0, Ordering::Relaxed);
            if big > 0 {
                let line = format!("ALLOC {name}: one allocation of {big} bytes");
                println!("{line}");
                t.fails.push(line);
            }
            match o {
                Outcome::Opened => t.opened += 1,
                Outcome::Refused => t.refused += 1,
                Outcome::Misread(m) => {
                    t.opened += 1;
                    let line = format!("MISREAD {name}: {m}");
                    println!("{line}");
                    std::io::stdout().flush().unwrap();
                    t.fails.push(line);
                }
            }
        }
        Ok(Err(_)) => {
            let msg = LAST_PANIC.lock().unwrap().clone();
            let line = format!("PANIC {name}: {msg}");
            println!("{line}");
            t.fails.push(line);
        }
        Err(_) => {
            println!("HANG {name}: no answer in {secs} s");
            std::io::stdout().flush().unwrap();
            std::process::exit(3);
        }
    }
}

enum Outcome {
    Opened,
    Refused,
    Misread(String),
}

/// A mutation of `b`, drawn from `r`: a bit flip, a byte set, a 0xff run, a
/// zero run, a u32 or u64 word at an extreme, or a truncation. Returns a
/// short description.
fn mutate(b: &mut Vec<u8>, r: &mut u64, region: (usize, usize)) -> String {
    let (lo, hi) = region;
    let hi = hi.min(b.len());
    if hi <= lo {
        return "nothing".into();
    }
    let at = lo + (splitmix(r) % (hi - lo) as u64) as usize;
    let words: [u64; 10] = [
        0,
        1,
        0xff,
        0xffff,
        0x7fff_ffff,
        0xffff_ffff,
        0x1_0000_0000,
        u64::MAX / 2,
        u64::MAX - 7,
        u64::MAX,
    ];
    match splitmix(r) % 9 {
        0 => {
            let bit = splitmix(r) % 8;
            b[at] ^= 1 << bit;
            format!("flip {at}.{bit}")
        }
        1 => {
            let v = splitmix(r) as u8;
            b[at] = v;
            format!("set {at}={v:#x}")
        }
        2 => {
            let n = (1 + splitmix(r) % 64) as usize;
            let end = (at + n).min(b.len());
            b[at..end].fill(0xff);
            format!("ff {at}..{end}")
        }
        3 => {
            let n = (1 + splitmix(r) % 64) as usize;
            let end = (at + n).min(b.len());
            b[at..end].fill(0);
            format!("zero {at}..{end}")
        }
        4 | 5 => {
            let w = words[(splitmix(r) % words.len() as u64) as usize];
            let at = at & !7;
            let end = (at + 8).min(b.len());
            b[at..end].copy_from_slice(&w.to_le_bytes()[..end - at]);
            format!("u64 {at}={w:#x}")
        }
        6 => {
            let w = words[(splitmix(r) % words.len() as u64) as usize] as u32;
            let at = at & !3;
            let end = (at + 4).min(b.len());
            b[at..end].copy_from_slice(&w.to_le_bytes()[..end - at]);
            format!("u32 {at}={w:#x}")
        }
        7 => {
            b.truncate(at);
            format!("truncate {at}")
        }
        _ => {
            let n = (1 + splitmix(r) % 4096) as usize;
            b.extend(std::iter::repeat_n(0xffu8, n));
            format!("extend {n}x0xff")
        }
    }
}

// ----------------------------------------------------------------- dumping

/// With `HOSTILE_DUMP=DIR`, a segment case is written to DIR as
/// `<n>.sup` with its name and shape in `cases.tsv`, instead of being run,
/// for the browser reader's driver (`../wasm/drive.mjs`).
fn dump(name: &str, b: &[u8], shape: usize) -> bool {
    let Some(d) = std::env::var_os("HOSTILE_DUMP") else {
        return false;
    };
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = PathBuf::from(d);
    std::fs::create_dir_all(&d).unwrap();
    if N.load(Ordering::Relaxed) == 0 {
        let have = std::fs::read_to_string(d.join("cases.tsv")).map_or(0, |t| t.lines().count());
        N.store(have + 1_000_000, Ordering::Relaxed);
    }
    let n = N.fetch_add(1, Ordering::Relaxed) - 1_000_000;
    std::fs::write(d.join(format!("{n}.sup")), b).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.join("cases.tsv"))
        .unwrap();
    writeln!(f, "{n}\t{shape}\t{name}").unwrap();
    true
}

fn dump_models(segs: &[(String, Vec<u8>, Model)]) {
    let Some(d) = std::env::var_os("HOSTILE_DUMP") else {
        return;
    };
    let d = PathBuf::from(d);
    std::fs::create_dir_all(&d).unwrap();
    for (i, (_, b, m)) in segs.iter().enumerate() {
        let rows: Vec<String> = m
            .iter()
            .map(|(k, v)| {
                format!(
                    "[\"{}\",{}]",
                    k.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    v.len()
                )
            })
            .collect();
        std::fs::write(
            d.join(format!("shape{i}.json")),
            format!("[{}]", rows.join(",")),
        )
        .unwrap();
        std::fs::write(d.join(format!("shape{i}.sup")), b).unwrap();
    }
}

// ----------------------------------------------------------------- segments

type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

/// Segments in several shapes, written by the public `SegmentWriter`.
fn segments(dir: &Path) -> Vec<(String, Vec<u8>, Model)> {
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
        if shape == 2 {
            w.set_compress(true);
        }
        let mut r = 0x5eed ^ shape;
        let mut m: Model = BTreeMap::new();
        let mut keys: Vec<Vec<u8>> = (0..300)
            .map(|i| {
                let pad = (splitmix(&mut r) % 24) as usize;
                format!("k{i:05}{}", "x".repeat(pad)).into_bytes()
            })
            .collect();
        keys.push(Vec::new());
        keys.push(vec![0xff; 33]);
        keys.sort();
        keys.dedup();
        for k in &keys {
            w.begin(k).unwrap();
            let n = match splitmix(&mut r) % 10 {
                0 => 0,
                1 => 30,
                _ => 1 + (splitmix(&mut r) % 3) as usize,
            };
            let fixed = splitmix(&mut r).is_multiple_of(4);
            let mut vs = Vec::new();
            for _ in 0..n {
                let len = if fixed {
                    8
                } else {
                    match splitmix(&mut r) % 20 {
                        0 => 3000,
                        1 => 0,
                        _ => 8 + (splitmix(&mut r) % 120) as usize,
                    }
                };
                let v: Vec<u8> = (0..len).map(|_| splitmix(&mut r) as u8).collect();
                w.value(&v);
                vs.push(v);
            }
            w.end().unwrap();
            m.insert(k.clone(), vs);
        }
        w.finish(1).unwrap();
        out.push((format!("shape{shape}"), std::fs::read(&path).unwrap(), m));
    }
    out
}

/// Every reader over `bytes` to its end. Returns whether any opened, and
/// for a raw (unforged) mutation, the first answer that is not what the
/// model holds for its key when the read said it succeeded.
fn read_segment(bytes: &[u8], m: &Model, check: bool) -> Outcome {
    let mut probes: Vec<Vec<u8>> = m.keys().cloned().collect();
    probes.push(b"absent".to_vec());
    probes.push(vec![0xff; 200]);
    let mut wrong: Option<String> = None;
    let mut opened = false;
    fn run<B: Bytes>(
        blob: &Blob<B>,
        probes: &[Vec<u8>],
        m: &Model,
        check: bool,
        wrong: &mut Option<String>,
        who: &str,
    ) {
        let mut sink = 0u64;
        for k in probes {
            let mut got: Vec<Vec<u8>> = Vec::new();
            let r = blob.read_all(k, |v: &[u8]| got.push(v.to_vec()));
            if check && wrong.is_none() {
                if let Ok(n) = r {
                    let want = m.get(k).cloned().unwrap_or_default();
                    if got != want || n as usize != want.len() {
                        *wrong = Some(format!(
                            "{who} read_all({}) gave {} values, want {}",
                            hex(k),
                            got.len(),
                            want.len()
                        ));
                    }
                }
            }
            let c = blob.count(k);
            if check && wrong.is_none() {
                if let Ok(c) = c {
                    let want = m.get(k).map_or(0, |v| v.len()) as u64;
                    if c != want {
                        *wrong = Some(format!("{who} count({}) = {c}, want {want}", hex(k)));
                    }
                }
            }
            let rank = blob.seek(k);
            let _ = blob.key_at(rank);
            let _ = blob.scan_at(rank, 4, |k: &[u8], v: &[u8]| {
                sink += (k.len() + v.len()) as u64
            });
            let _ = blob.stored_bytes(k);
            let _ = blob.count_fixed(k, 8);
            let _ = blob.ranges_for(k);
        }
        let n = blob.keys().min(100_000);
        for rank in 0..n {
            let _ = blob.key_at(rank);
            let _ = blob.exts_at_full(rank);
            let _ = blob.values_at(rank, |v: &[u8]| sink += v.len() as u64);
            let _ = blob.record_offset(rank);
        }
        let mut seen: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let r = blob.scan(b"", 1_000_000, |k: &[u8], v: &[u8]| {
            if seen.len() < 200_000 {
                seen.push((k.to_vec(), v.to_vec()))
            }
        });
        if check && wrong.is_none() {
            if let Ok(_) = r {
                let want: Vec<(Vec<u8>, Vec<u8>)> = m
                    .iter()
                    .flat_map(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
                    .collect();
                if seen != want {
                    *wrong = Some(format!(
                        "{who} scan gave {} pairs, want {} (first differs at {})",
                        seen.len(),
                        want.len(),
                        seen.iter()
                            .zip(&want)
                            .position(|(a, b)| a != b)
                            .unwrap_or(seen.len().min(want.len()))
                    ));
                }
            }
        }
        let mut c = 0usize;
        let _ = blob.scan_counts(b"", 1_000_000, |_k: &[u8], n: u64| {
            sink += n;
            c += 1;
            c < 1_000_000
        });
        let _ = blob.scan_counts_fixed(b"", 1_000_000, 8, |_k: &[u8], n: Option<u64>| {
            sink += n.unwrap_or(0);
            true
        });
        let mut w = blob.run_walk();
        let _ = &mut w;
        let _ = blob.block_extents();
        let _ = blob.intersect_fixed(b"k00001", b"k00002", 8);
        std::hint::black_box(sink);
    }
    if let Ok(blob) = Blob::open(SliceBytes(bytes)) {
        opened = true;
        run(&blob, &probes, m, check, &mut wrong, "Blob(lent)");
    }
    if let Ok(blob) = Blob::open(Copying(bytes.to_vec())) {
        opened = true;
        run(&blob, &probes, m, check, &mut wrong, "Blob(copying)");
    }
    // The mapped reader over a real file mapping, as a store opens one.
    if !bytes.is_empty() {
        let p = std::env::temp_dir().join(format!(
            "supdb-hostile-map-{}-{:?}.sup",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&p, bytes).unwrap();
        let mapped = MmapBytes::open(&p);
        let _ = std::fs::remove_file(&p);
        if let Ok(src) = mapped {
            if let Ok(blob) = Blob::open(src) {
                opened = true;
                run(&blob, &probes, m, check, &mut wrong, "Blob(mapped)");
            }
        }
    }
    if let Ok(sp) = SparseBlob::open(SliceBytes(bytes)) {
        opened = true;
        let mut sink = 0u64;
        for k in probes.iter().take(80) {
            sink += sp.dictionary_plan(k, None).len() as u64;
            sink += sp
                .dictionary_plan_records(k, None)
                .map_or(0, |p| p.len() as u64);
        }
        let mut c = 0usize;
        let mut walked: Vec<(Vec<u8>, u64)> = Vec::new();
        let r = sp.dictionary_counts(b"", None, |k: &[u8], n: u64| {
            if walked.len() < 200_000 {
                walked.push((k.to_vec(), n));
            }
            c += 1;
            c < 1_000_000
        });
        if check && wrong.is_none() {
            if r.is_ok() {
                let want: Vec<(Vec<u8>, u64)> =
                    m.iter().map(|(k, v)| (k.clone(), v.len() as u64)).collect();
                if walked != want {
                    wrong = Some(format!(
                        "SparseBlob dictionary_counts gave {} keys, want {}",
                        walked.len(),
                        want.len()
                    ));
                }
            }
        }
        let _ = sp.dictionary_walk(b"", None, |_k: &[u8], exts, tail: &[u8]| {
            let _ = sp.read_exts(exts, tail, |v: &[u8]| sink += v.len() as u64);
            let _ = sp.ranges_for_exts(exts);
            true
        });
        std::hint::black_box(sink);
    }
    match (wrong, opened) {
        (Some(w), _) => Outcome::Misread(w),
        (None, true) => Outcome::Opened,
        (None, false) => Outcome::Refused,
    }
}

fn hex(k: &[u8]) -> String {
    if k.len() > 12 {
        format!(
            "{}..({} bytes)",
            k[..12]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            k.len()
        )
    } else {
        k.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// The superblock's FNV, recomputed over its 16 fields and the magic, as
/// `Super::decode` checks it; both slots.
fn refix_super(b: &mut [u8]) {
    for slot in [0usize, 512] {
        if b.len() < slot + 144 {
            continue;
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for i in 0..17 {
            for x in &b[slot + i * 8..slot + i * 8 + 8] {
                h ^= *x as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
        }
        b[slot + 136..slot + 144].copy_from_slice(&h.to_le_bytes());
    }
}

/// The key section's row of CRCs recomputed over the section as it stands.
fn refix_row(b: &mut [u8], key_off: usize, crc_off: usize) {
    if crc_off > b.len() || key_off > crc_off {
        return;
    }
    let row = flatindex::checksum_row(&b[key_off..crc_off], flatindex::PIECE_SHIFT, key_off as u64);
    let end = (crc_off + row.len()).min(b.len());
    b[crc_off..end].copy_from_slice(&row[..end - crc_off]);
}

/// Raw mutations: no checksum recomputed. Every answer that a read reports
/// as a success is held to the model.
fn t_segment(from: u64, to: u64, forge: bool) -> Tally {
    let dir = scratch("seg");
    let segs = Arc::new(segments(&dir));
    dump_models(&segs);
    for (name, b, m) in segs.iter() {
        if let Outcome::Misread(w) = read_segment(b, m, true) {
            panic!("pristine {name} misreads: {w}");
        }
    }
    let mut t = Tally::default();
    for seed in from..to {
        let si = (seed % segs.len() as u64) as usize;
        let (shape, pristine, _) = &segs[si];
        let mut r = seed.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let mut b = pristine.clone();
        let facts = Blob::open(SliceBytes(pristine)).unwrap().z_source_facts();
        // Where the mutation lands: the superblock page, the key section,
        // the block table and blocks past it, or anywhere.
        let (ko, co) = facts.as_ref().map_or((4096, b.len()), |f| {
            (f.key_off as usize, (f.key_off + f.crc_off) as usize)
        });
        let regions = [
            (0usize, 4096usize),
            (ko, ko + 192),
            (ko, co),
            (co, b.len()),
            (4096, ko),
            (0, b.len()),
        ];
        let reg = regions[(splitmix(&mut r) % regions.len() as u64) as usize];
        let n = 1 + splitmix(&mut r) % 3;
        let mut what = Vec::new();
        for _ in 0..n {
            what.push(mutate(&mut b, &mut r, reg));
        }
        if std::env::var_os("HOSTILE_RESTORE_CRCOFF").is_some() && b.len() >= ko + 168 {
            b[ko + 160..ko + 168].copy_from_slice(&pristine[ko + 160..ko + 168]);
        }
        if forge {
            if reg.0 == ko || reg == (0, b.len()) || reg.0 == 4096 {
                refix_row(&mut b, ko, co);
            }
            refix_super(&mut b);
        }
        let name = format!(
            "{} seed {seed} {shape} region {}..{}: {}",
            if forge { "segforge" } else { "segment" },
            reg.0,
            reg.1,
            what.join(", ")
        );
        if dump(&name, &b, si) {
            continue;
        }
        let segs = segs.clone();
        case(&mut t, name, move || read_segment(&b, &segs[si].2, !forge));
    }
    let _ = std::fs::remove_dir_all(&dir);
    t
}

/// Superblock fields set to extremes under a valid FNV: offsets and
/// lengths of the key section and the block table, the high water, the
/// log arena, every one of the sixteen words.
fn t_superfields() -> Tally {
    let dir = scratch("sf");
    let segs = Arc::new(segments(&dir));
    dump_models(&segs);
    let mut t = Tally::default();
    let vals: [u64; 12] = [
        0,
        1,
        7,
        4095,
        4096,
        4097,
        0x7fff_ffff,
        0xffff_ffff,
        0x1_0000_0000,
        1 << 62,
        u64::MAX - 4095,
        u64::MAX,
    ];
    for (si, (shape, pristine, _)) in segs.iter().enumerate() {
        let len = pristine.len() as u64;
        for field in 0..16usize {
            for v in vals.iter().copied().chain([len, len - 1, len + 1]) {
                for slots in [1u8, 2] {
                    let mut b = pristine.clone();
                    b[field * 8..field * 8 + 8].copy_from_slice(&v.to_le_bytes());
                    if slots == 2 {
                        b[512 + field * 8..512 + field * 8 + 8].copy_from_slice(&v.to_le_bytes());
                    }
                    refix_super(&mut b);
                    let name = format!("superfield {shape} f{field}={v:#x} slots {slots}");
                    if dump(&name, &b, si) {
                        continue;
                    }
                    let segs = segs.clone();
                    case(&mut t, name, move || read_segment(&b, &segs[si].2, false));
                }
            }
        }
        // The key section's 24 header words, each at an extreme, the row
        // recomputed.
        let f = Blob::open(SliceBytes(pristine))
            .unwrap()
            .z_source_facts()
            .unwrap();
        let (ko, co) = (f.key_off as usize, (f.key_off + f.crc_off) as usize);
        for word in 0..24usize {
            for v in vals {
                let mut b = pristine.clone();
                b[ko + word * 8..ko + word * 8 + 8].copy_from_slice(&v.to_le_bytes());
                refix_row(&mut b, ko, co);
                let name = format!("keyheader {shape} w{word}={v:#x}");
                if !dump(&name, &b, si) {
                    let s2 = segs.clone();
                    case(&mut t, name, move || read_segment(&b, &s2[si].2, false));
                }
                for half in 0..2 {
                    let mut b = pristine.clone();
                    let at = ko + word * 8 + half * 4;
                    b[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
                    refix_row(&mut b, ko, co);
                    let name = format!("keyheader {shape} w{word}.{half}={:#x}", v as u32);
                    if dump(&name, &b, si) {
                        continue;
                    }
                    let s2 = segs.clone();
                    case(&mut t, name, move || read_segment(&b, &s2[si].2, false));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    t
}

/// The key section's row named absent: the header's `crc_off` word (byte
/// 160 of the section) zeroed, nothing else recomputed, then one byte of
/// the section past its header changed. A reader that opens and answers
/// differently from the model has read damage it had a checksum for.
fn t_crcoff(from: u64, to: u64) -> Tally {
    let dir = scratch("crcoff");
    let segs = Arc::new(segments(&dir));
    dump_models(&segs);
    let mut t = Tally::default();
    for seed in from..to {
        let si = (seed % segs.len() as u64) as usize;
        let (shape, pristine, _) = &segs[si];
        let f = Blob::open(SliceBytes(pristine))
            .unwrap()
            .z_source_facts()
            .unwrap();
        let (ko, co) = (f.key_off as usize, (f.key_off + f.crc_off) as usize);
        let mut b = pristine.clone();
        b[ko + 160..ko + 168].fill(0);
        let mut r = seed ^ 0xc0ff;
        let at = ko + 192 + (splitmix(&mut r) % (co - ko - 192) as u64) as usize;
        let bit = splitmix(&mut r) % 8;
        if seed >= 1 << 32 {
            // seed's top half set: the zeroed word alone.
        } else {
            b[at] ^= 1 << bit;
        }
        let name = format!(
            "crcoff seed {seed} {shape}: zero key section +160..+168 at {}, flip {at}.{bit}",
            ko + 160
        );
        if dump(&name, &b, si) {
            continue;
        }
        let s2 = segs.clone();
        case(&mut t, name, move || read_segment(&b, &s2[si].2, true));
    }
    let _ = std::fs::remove_dir_all(&dir);
    t
}

// -------------------------------------------------------------------- stores

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-hostile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Every value ever written for each key, by phase. A read from a mutated
/// store may answer any phase's state (a fallback slot, a log cut), never a
/// value no phase wrote.
type History = BTreeMap<Vec<u8>, Vec<Vec<Vec<u8>>>>;

#[derive(Clone, Copy)]
enum Kind {
    Plain,
    OneFile,
    Cipher,
}

fn opts(kind: Kind) -> Options {
    let mut o = Options {
        seal_bytes: 32 << 10,
        partition_bytes: Some(32 << 10),
        l0_trigger: 2,
        ..Options::default()
    };
    match kind {
        Kind::Plain => {}
        Kind::OneFile => o.one_file = true,
        Kind::Cipher => o.encryption = Encryption::Cipher(store_cipher()),
    }
    o
}

/// A store in three phases: a load and a flush, overwrites and a flush,
/// then commits left in the WAL and a crash. `History` holds every state.
fn build_store(dir: &Path, kind: Kind) -> History {
    let mut h: History = BTreeMap::new();
    let mut db = Db::create(dir, opts(kind)).unwrap();
    let mut state: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let snap = |h: &mut History, state: &BTreeMap<Vec<u8>, Vec<Vec<u8>>>, keys: &[Vec<u8>]| {
        for k in keys {
            h.entry(k.clone())
                .or_default()
                .push(state.get(k).cloned().unwrap_or_default());
        }
    };
    let keys: Vec<Vec<u8>> = (0..1500u32)
        .map(|i| format!("key-{:06}", i.wrapping_mul(7919) % 1500).into_bytes())
        .chain([Vec::new(), vec![0xff; 40]])
        .collect();
    snap(&mut h, &state, &keys);
    for (i, k) in keys.iter().enumerate() {
        let v = format!("a-{i}-{}", "v".repeat(i % 90)).into_bytes();
        db.append(k, &v);
        state.entry(k.clone()).or_default().push(v);
        if i % 3 == 0 {
            let v2 = vec![i as u8; i % 300];
            db.append(k, &v2);
            state.entry(k.clone()).or_default().push(v2);
        }
        if i % 100 == 99 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    snap(&mut h, &state, &keys);
    for (i, k) in keys.iter().enumerate().step_by(3) {
        let v = format!("b-{i}").into_bytes();
        db.put(k, &v);
        state.insert(k.clone(), vec![v]);
        if i % 60 == 0 {
            db.commit().unwrap();
            snap(&mut h, &state, &keys);
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    snap(&mut h, &state, &keys);
    for (i, k) in keys.iter().enumerate().step_by(7) {
        let v = format!("c-{i}").into_bytes();
        db.append(k, &v);
        state.entry(k.clone()).or_default().push(v);
        if i % 70 == 0 {
            db.commit().unwrap();
            snap(&mut h, &state, &keys);
        }
    }
    db.commit().unwrap();
    db.sync().unwrap();
    snap(&mut h, &state, &keys);
    db.crash();
    h
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// Open the mutated store and read every key, a scan and the counts. An
/// answer is held to the key's history: it must equal some phase's values.
fn read_store(dir: &Path, kind: Kind, h: &History) -> Outcome {
    let db = match Db::open(dir, opts(kind)) {
        Ok(db) => db,
        Err(e) => {
            if std::env::var_os("HOSTILE_WHY").is_some() {
                eprintln!("refused: {e}");
            }
            return Outcome::Refused;
        }
    };
    let mut wrong = None;
    let rd = match db.reader() {
        Ok(r) => r,
        Err(_) => {
            let _ = db.close();
            return Outcome::Refused;
        }
    };
    let mut any_err = false;
    let mut earlier = false;
    for (k, states) in h {
        let mut got = Vec::new();
        match rd.read_all(k, |v: &[u8]| got.push(v.to_vec())) {
            Ok(_) => {
                if states.last() != Some(&got) {
                    earlier = true;
                }
                if wrong.is_none() && !states.contains(&got) {
                    wrong = Some(format!(
                        "read_all({}) gave {} values in no phase's state",
                        hex(k),
                        got.len()
                    ));
                }
            }
            Err(_) => any_err = true,
        }
        let _ = rd.count(k);
    }
    let mut n = 0usize;
    let _ = rd.scan(b"", 10_000_000, |k: &[u8], _v: &[u8]| {
        n += 1;
        if wrong.is_none() && !h.contains_key(k) {
            wrong = Some(format!("scan gave a key never written: {}", hex(k)));
        }
    });
    drop(rd);
    let _ = db.close();
    match wrong {
        Some(w) => Outcome::Misread(w),
        None if any_err => Outcome::Refused,
        None => {
            if earlier {
                EARLIER.fetch_add(1, Ordering::Relaxed);
                if std::env::var_os("HOSTILE_WHY").is_some() {
                    eprintln!("EARLIER {}", CASE.lock().unwrap());
                }
            }
            Outcome::Opened
        }
    }
}
static EARLIER: AtomicUsize = AtomicUsize::new(0);

fn t_store(kind: Kind, from: u64, to: u64, only: Option<&str>) -> Tally {
    let label = match kind {
        Kind::Plain => "store",
        Kind::OneFile => "onefile",
        Kind::Cipher => "cipher",
    };
    let base = scratch(label);
    let pristine = base.join("pristine");
    std::fs::create_dir_all(&pristine).unwrap();
    let h = Arc::new(build_store(&pristine, kind));
    let mut files: Vec<String> = std::fs::read_dir(&pristine)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "LOCK")
        .filter(|n| only.is_none_or(|p| n.starts_with(p)))
        .collect();
    files.sort();
    eprintln!("{label}: files {files:?}");
    // The pristine copy after its crash must read as its last state.
    let work = base.join("work");
    copy_dir(&pristine, &work);
    match read_store(&work, kind, &h) {
        Outcome::Opened => {}
        Outcome::Refused => panic!("{label}: the pristine store refused or erred"),
        Outcome::Misread(w) => panic!("{label}: the pristine store misreads: {w}"),
    }
    let mut t = Tally::default();
    for seed in from..to {
        let mut r = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xabcd;
        let f = &files[(splitmix(&mut r) % files.len() as u64) as usize];
        // A directory of its own per case: a store under a cipher is
        // registered in the process by its directory.
        let _ = std::fs::remove_dir_all(&work);
        let work = base.join(format!("work-{seed}"));
        copy_dir(&pristine, &work);
        let p = work.join(f);
        let mut b = std::fs::read(&p).unwrap();
        let len = b.len();
        // Half the mutations in the file's first 4 KiB, where its header
        // and lengths are; half anywhere.
        let reg = if splitmix(&mut r).is_multiple_of(2) {
            (0, len.min(4096))
        } else {
            (0, len)
        };
        let n = 1 + splitmix(&mut r) % 2;
        let what: Vec<String> = (0..n).map(|_| mutate(&mut b, &mut r, reg)).collect();
        std::fs::write(&p, &b).unwrap();
        let name = format!("{label} seed {seed} {f} ({len} bytes): {}", what.join(", "));
        let (w, h) = (work.clone(), h.clone());
        case(&mut t, name, move || read_store(&w, kind, &h));
        let _ = std::fs::remove_dir_all(&work);
    }
    let _ = std::fs::remove_dir_all(&base);
    t
}

// ------------------------------------------------------------------- ciphers

struct Gcm(ring::aead::LessSafeKey);
impl Gcm {
    fn new(k: &[u8; 32]) -> Gcm {
        Gcm(ring::aead::LessSafeKey::new(
            ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, k).unwrap(),
        ))
    }
}
impl Aead for Gcm {
    fn encrypt(&self, n: &[u8; crypt::NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; crypt::TAG] {
        let t = self
            .0
            .seal_in_place_separate_tag(
                ring::aead::Nonce::assume_unique_for_key(*n),
                ring::aead::Aad::from(aad),
                buf,
            )
            .unwrap();
        t.as_ref().try_into().unwrap()
    }
    fn decrypt(
        &self,
        n: &[u8; crypt::NONCE],
        aad: &[u8],
        buf: &mut [u8],
        tag: &[u8; crypt::TAG],
    ) -> bool {
        self.0
            .open_in_place_separate_tag(
                ring::aead::Nonce::assume_unique_for_key(*n),
                ring::aead::Aad::from(aad),
                ring::aead::Tag::from(*tag),
                buf,
                0..,
            )
            .is_ok()
    }
}

fn store_cipher() -> StoreCipher {
    StoreCipher::new(Gcm::new(&[0x42; 32]), Gcm::new).key_id(0x42)
}

/// The `crypt` envelope: a segment encrypted whole, its header and chunks
/// mutated, read through `Encrypted` by every segment reader and by
/// `crypt::decrypt`. Header fields are set to extremes; the AAD covers the
/// header, so a changed header must fail every chunk, never misread.
fn t_envelope(from: u64, to: u64) -> Tally {
    let dir = scratch("env");
    let segs = segments(&dir);
    let (_, seg, m) = segs.into_iter().next().unwrap();
    let m = Arc::new(m);
    let mut sealed = Vec::new();
    for shift in [9u32, 12, 16] {
        let mut out = Vec::new();
        crypt::encrypt_with_id(
            &SliceBytes(&seg),
            &mut out,
            &Gcm::new(&[7; 32]),
            &EncryptOptions {
                chunk_shift: shift,
                ..EncryptOptions::default()
            },
            [shift as u8; 16],
        )
        .unwrap();
        sealed.push(out);
    }
    let sealed = Arc::new(sealed);
    // The pristine envelopes read whole.
    for e in sealed.iter() {
        let enc = Encrypted::open(Copying(e.clone()), Gcm::new(&[7; 32])).unwrap();
        let mut v = vec![0u8; enc.len() as usize];
        enc.read_at(0, &mut v).unwrap();
        assert_eq!(v, seg);
    }
    let mut t = Tally::default();
    for seed in from..to {
        let mut r = seed ^ 0xe11e;
        let ei = (seed % 3) as usize;
        let mut b = sealed[ei].clone();
        let reg = match splitmix(&mut r) % 3 {
            0 => (0, 48),
            1 => (0, 4096),
            _ => (0, b.len()),
        };
        let what = mutate(&mut b, &mut r, reg);
        if b == sealed[ei] {
            continue;
        }
        let name = format!("envelope seed {seed} shift#{ei}: {what}");
        let m = m.clone();
        let seg_len = seg.len();
        case(&mut t, name, move || {
            let mut out = Vec::new();
            let d = crypt::decrypt(&SliceBytes(&b), &mut out, &Gcm::new(&[7; 32]));
            let mut o = Outcome::Refused;
            if d.is_ok() {
                o = Outcome::Misread(format!(
                    "decrypt accepted a changed envelope ({} bytes out)",
                    out.len()
                ));
            }
            if let Ok(enc) = Encrypted::open(Copying(b.clone()), Gcm::new(&[7; 32])) {
                if enc.len() != seg_len as u64 {
                    return Outcome::Misread(format!(
                        "an opened envelope says {} bytes, wrote {seg_len}",
                        enc.len()
                    ));
                }
                let mut probe = vec![0u8; 4096.min(seg_len)];
                let _ = enc.read_at(0, &mut probe);
                let _ = enc.read_at(enc.len().saturating_sub(10), &mut [0u8; 10]);
                let _ = enc.read_at(u64::MAX - 5, &mut [0u8; 10]);
                let _ = enc.slice_at(u64::MAX - 5, 10);
                let _ = enc.slice_at(0, usize::MAX);
                if let Outcome::Misread(w) = read_segment_through(enc, &m) {
                    return Outcome::Misread(w);
                }
                if !matches!(o, Outcome::Misread(_)) {
                    o = Outcome::Opened;
                }
            }
            o
        });
    }
    // Header fields at extremes (the AAD then fails), and an envelope
    // truncated at every chunk boundary.
    let fields: [(usize, usize); 5] = [(8, 4), (12, 4), (16, 8), (24, 16), (40, 8)];
    for (ei, e) in sealed.iter().enumerate() {
        for (at, w) in fields {
            for v in [
                0u64,
                1,
                8,
                9,
                24,
                25,
                31,
                32,
                63,
                64,
                0xffff_ffff,
                1 << 40,
                u64::MAX,
            ] {
                let mut b = e.clone();
                b[at..at + w.min(8)].copy_from_slice(&v.to_le_bytes()[..w.min(8)]);
                if &b == e {
                    continue;
                }
                let name = format!("envelope header shift#{ei} @{at}={v:#x}");
                let m = m.clone();
                case(&mut t, name, move || {
                    let mut out = Vec::new();
                    if crypt::decrypt(&SliceBytes(&b), &mut out, &Gcm::new(&[7; 32])).is_ok() {
                        return Outcome::Misread("decrypt accepted a changed header".into());
                    }
                    match Encrypted::open(Copying(b), Gcm::new(&[7; 32])) {
                        Ok(enc) => read_segment_through(enc, &m),
                        Err(_) => Outcome::Refused,
                    }
                });
            }
        }
        let h = crypt::Header::parse(&e[..48]).unwrap();
        let stored = h.stored_chunk() as usize;
        let mut cut = 4096usize;
        while cut < e.len() {
            for d in [0usize, 1, stored - 1] {
                let mut b = e.clone();
                b.truncate((cut + d).min(e.len() - 1));
                let name = format!("envelope truncated shift#{ei} at {}", b.len());
                let m = m.clone();
                case(&mut t, name, move || {
                    let mut out = Vec::new();
                    if crypt::decrypt(&SliceBytes(&b), &mut out, &Gcm::new(&[7; 32])).is_ok() {
                        return Outcome::Misread("decrypt accepted a truncated envelope".into());
                    }
                    match Encrypted::open(Copying(b), Gcm::new(&[7; 32])) {
                        Ok(enc) => read_segment_through(enc, &m),
                        Err(_) => Outcome::Refused,
                    }
                });
            }
            cut += stored * 7;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    t
}

/// A segment read through an envelope: `Blob` and `SparseBlob` over the
/// `Encrypted` source, answers held to the model (the AEAD makes every
/// answer either right or an error).
fn read_segment_through(enc: Encrypted<Copying, Gcm>, m: &Model) -> Outcome {
    let mut wrong = None;
    let raw = enc.source().0.clone();
    if let Ok(blob) = Blob::open(enc) {
        for (k, want) in m.iter() {
            let mut got = Vec::new();
            if blob.read_all(k, |v: &[u8]| got.push(v.to_vec())).is_ok()
                && &got != want
                && wrong.is_none()
            {
                wrong = Some(format!("Blob(Encrypted) read_all({}) differs", hex(k)));
            }
        }
        let _ = blob.scan(b"", 1 << 20, |_: &[u8], _: &[u8]| {});
    }
    if let Ok(sp) = Encrypted::open(Copying(raw), Gcm::new(&[7; 32]))
        .map_err(|_| ())
        .and_then(|e| SparseBlob::open(e).map_err(|_| ()))
    {
        let _ = sp.dictionary_counts(b"", None, |_: &[u8], _| true);
    }
    match wrong {
        Some(w) => Outcome::Misread(w),
        None => Outcome::Opened,
    }
}

/// Mappings and resident bytes across repeated opens of one store: each
/// open ended by `crash`, by `close`, or refused (its newest manifest slot
/// and the other both damaged).
fn t_leak(mode: &str, n: u64) -> Tally {
    let base = scratch("leak");
    let pristine = base.join("pristine");
    std::fs::create_dir_all(&pristine).unwrap();
    let _ = build_store(&pristine, Kind::Plain);
    let maps = || {
        std::fs::read_to_string("/proc/self/maps")
            .unwrap()
            .lines()
            .count()
    };
    let rss = || {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("VmRSS"))
            .unwrap_or("")
            .to_string()
    };
    let refused_ok = mode.starts_with("refused");
    for i in 0..n {
        let w = base.join(format!("w{i}"));
        copy_dir(&pristine, &w);
        if refused_ok {
            for m in ["manifest", "manifest.alt"] {
                let p = w.join(m);
                let mut b = std::fs::read(&p).unwrap();
                let k = b.len() / 2;
                b[k] ^= 0x55;
                std::fs::write(&p, b).unwrap();
            }
            if mode == "refused-par" {
                // A segment's key section damaged instead, manifests whole.
                copy_dir(&pristine, &w);
                for e in std::fs::read_dir(&w).unwrap() {
                    let p = e.unwrap().path();
                    if p.to_string_lossy().ends_with(".sup") {
                        let mut b = std::fs::read(&p).unwrap();
                        let k = b.len() - 100;
                        b[k] ^= 0x55;
                        std::fs::write(&p, b).unwrap();
                    }
                }
            }
        }
        match Db::open(&w, opts(Kind::Plain)) {
            Ok(db) => {
                if mode == "close" {
                    let _ = db.close();
                } else {
                    db.crash();
                }
            }
            Err(e) => {
                if i == 0 {
                    eprintln!("refused: {e}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&w);
        if i % 100 == 0 || i + 1 == n {
            println!("{mode} open {i}: {} mappings, {}", maps(), rss());
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    Tally::default()
}

/// Every byte of the newest WAL's first `upto` bytes flipped in turn
/// (bit 0), one case each: which flips refuse, which open at the last
/// state, which open at an earlier one (with `HOSTILE_WHY`, named).
fn t_walhead(kind: Kind, upto: usize) -> Tally {
    let base = scratch("walhead");
    let pristine = base.join("pristine");
    std::fs::create_dir_all(&pristine).unwrap();
    let h = Arc::new(build_store(&pristine, kind));
    let mut wals: Vec<String> = std::fs::read_dir(&pristine)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wal-"))
        .collect();
    wals.sort();
    let f = wals.last().unwrap().clone();
    let mut t = Tally::default();
    for at in 0..upto {
        let work = base.join(format!("w{at}"));
        copy_dir(&pristine, &work);
        let p = work.join(&f);
        let mut b = std::fs::read(&p).unwrap();
        if at >= b.len() {
            break;
        }
        b[at] ^= 1;
        std::fs::write(&p, &b).unwrap();
        let name = format!("walhead {f} flip {at}.0");
        let (w, h2) = (work.clone(), h.clone());
        case(&mut t, name, move || read_store(&w, kind, &h2));
        let _ = std::fs::remove_dir_all(&work);
    }
    let _ = std::fs::remove_dir_all(&base);
    t
}

/// format.md F1: a key over 65,535 bytes put, read, flushed and read
/// again after a reopen, through the public API.
fn t_bigkey() -> Tally {
    let mut t = Tally::default();
    for len in [65_535usize, 65_536, 70_000] {
        let d = scratch(&format!("bigkey{len}"));
        let name = format!("bigkey {len}");
        case(&mut t, name, move || {
            let key: Vec<u8> = (0..len).map(|i| (i * 7 + 1) as u8).collect();
            let mut db = Db::create(&d, Options::default()).unwrap();
            db.put(&key, b"value");
            if let Err(e) = db.commit() {
                return if len > 65_535 {
                    Outcome::Refused
                } else {
                    Outcome::Misread(format!("commit refused: {e}"))
                };
            }
            let read = |db: &Db, when: &str| -> Option<String> {
                let rd = db.reader().ok()?;
                let mut got = Vec::new();
                let _ = rd.read_all(&key, |v: &[u8]| got.push(v.to_vec()));
                let mut short = 0usize;
                let _ = rd.scan(b"", 100, |k: &[u8], _v: &[u8]| {
                    if k.len() != len {
                        short = k.len()
                    }
                });
                if got != vec![b"value".to_vec()] || short != 0 {
                    Some(format!(
                        "{when}: read_all gave {} values; a scan's key is {short} bytes",
                        got.len()
                    ))
                } else {
                    None
                }
            };
            if let Some(w) = read(&db, "before the flush") {
                return Outcome::Misread(w);
            }
            db.flush().unwrap();
            if let Some(w) = read(&db, "after the flush") {
                return Outcome::Misread(w);
            }
            db.close().unwrap();
            let db = Db::open(&d, Options::default()).unwrap();
            let r = read(&db, "after a reopen");
            let _ = db.close();
            let _ = std::fs::remove_dir_all(&d);
            match r {
                Some(w) => Outcome::Misread(w),
                None => Outcome::Opened,
            }
        });
    }
    t
}

// ---------------------------------------------------------------------- main

fn main() {
    if let Ok(v) = std::env::var("HOSTILE_ALLOC_MAX") {
        ALLOC_MAX.store(v.parse().unwrap(), Ordering::Relaxed);
    }
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let mut s = format!("{} at {at}", msg.chars().take(300).collect::<String>());
        if std::env::var_os("HOSTILE_BT").is_some() {
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let mut lines = bt.lines();
            while let Some(l) = lines.next() {
                if l.contains("supdb::") && !l.contains("hostile::") {
                    let next = lines.next().unwrap_or("").trim().to_string();
                    s.push_str(&format!(" | {} {}", l.trim(), next));
                }
            }
        }
        if std::thread::current().name() == Some("main") {
            eprintln!("harness panic: {s}");
        }
        if let Ok(mut p) = LAST_PANIC.lock() {
            *p = s;
        }
    }));
    let a: Vec<String> = std::env::args().collect();
    let target = a.get(1).map(String::as_str).unwrap_or("segment");
    let from: u64 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let to: u64 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(from + 1000);
    let only = a.get(4).map(String::as_str);
    let t = match target {
        "segment" => t_segment(from, to, false),
        "segforge" => t_segment(from, to, true),
        "superfields" => t_superfields(),
        "crcoff" => t_crcoff(from, to),
        "walhead" => t_walhead(Kind::Plain, to as usize),
        "walhead-cipher" => t_walhead(Kind::Cipher, to as usize),
        "walhead-onefile" => t_walhead(Kind::OneFile, to as usize),
        "bigkey" => t_bigkey(),
        "leak" => t_leak(only.unwrap_or("crash"), to),
        "store" => t_store(Kind::Plain, from, to, only),
        "onefile" => t_store(Kind::OneFile, from, to, only),
        "cipher" => t_store(Kind::Cipher, from, to, only),
        "envelope" => t_envelope(from, to),
        other => panic!("unknown target {other}"),
    };
    println!(
        "{target} {from}..{to}: {} cases, {} opened ({} of them at an earlier state), {} refused, {} failures",
        t.cases,
        t.opened,
        EARLIER.load(Ordering::Relaxed),
        t.refused,
        t.fails.len()
    );
    std::process::exit(if t.fails.is_empty() { 0 } else { 1 });
}
