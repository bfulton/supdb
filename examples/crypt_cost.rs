//! What a read through an envelope costs, beside the same read without one.
//!
//! ```sh
//! cargo run --release --example crypt_cost -- [KEYS] [REPS]
//! ```
//!
//! One segment, written once, read through every arm in one process with
//! the arms taken in turn inside each repetition, since two runs of one arm
//! differ here by more than two arms in one run do. Each line is an arm and
//! an operation: the first pass over a reader just opened, where an envelope
//! pays for its decryption, and the median of the passes after it.
//!
//! The arms:
//!
//! - `plain-mmap`: the segment through a mapping, which lends. The native
//!   reader with no envelope.
//! - `plain-copy`: the segment through a source that cannot lend: what a copy
//!   costs with no cipher in it.
//! - `enc-copy@N`: the envelope in chunks of `1 << N`, through a reader that
//!   keeps nothing (`ReadOptions::keep_bytes` of zero) and decrypts at each
//!   read.
//! - `enc-kept@N`: the same envelope through a reader that keeps each chunk
//!   it decrypts, the default, which lends.
//! - `kept+populate`, `kept+huge`: the default chunk again, the reader
//!   asking for a long range's pages in one call, or for huge pages
//!   (`ReadOptions::populate`, `huge_pages`): what the fresh pages a reader
//!   keeps its plaintext in cost a first touch.
//!
//! The operations: the open; point reads of keys whose values are inline in
//! the index record, of keys with a short run in a block, and of keys with a
//! long one; and a scan of every key with its values. The cipher's own rate
//! is printed first, since an envelope's first pass is that rate and little
//! else.
//!
//! Last, the same short-run reads with nothing in the page cache and the
//! kernel told not to read ahead, which is the shape of a read out of core:
//! the major faults a read takes, where the device's latency is the cost
//! and the cipher's is not.

use ring::aead::{Aad, LessSafeKey, Nonce, Tag, UnboundKey, AES_256_GCM};
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::crypt::{self, Aead, EncryptOptions, Encrypted, ReadOptions, NONCE, TAG};
use supdb::{Blob, Bytes, MmapBytes, SegmentOptions, SegmentWriter};

struct Gcm(LessSafeKey);

impl Aead for Gcm {
    fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG] {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        let tag = self
            .0
            .seal_in_place_separate_tag(nonce, Aad::from(aad), buf)
            .expect("a chunk is far under the cipher's limit");
        tag.as_ref().try_into().expect("a sixteen-byte tag")
    }
    fn decrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8], tag: &[u8; TAG]) -> bool {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        self.0
            .open_in_place_separate_tag(nonce, Aad::from(aad), Tag::from(*tag), buf, 0..)
            .is_ok()
    }
}

fn gcm() -> Gcm {
    Gcm(LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &[7u8; 32]).expect("a 32-byte key"),
    ))
}

/// A source that answers reads from a mapping and lends nothing.
struct Copying(MmapBytes);

impl Bytes for Copying {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        self.0.read_at(off, dst)
    }
}

/// The values a key holds, by its number: inline, a short run in a block,
/// a long one.
fn run_len(k: usize) -> usize {
    [1, 2, 17, 200, 1500, 3, 40][k % 7]
}

fn key_of(k: usize) -> Vec<u8> {
    format!("term={k:08}").into_bytes()
}

fn build(path: &Path, keys: usize) {
    let mut w = SegmentWriter::create(path, &SegmentOptions::default()).expect("create");
    // Short runs in the index record, as the engine's own segments have them.
    w.set_inline_max(256);
    for k in 0..keys {
        w.begin(&key_of(k)).expect("begin");
        for i in 0..run_len(k) {
            w.value(format!("{k}:{i}").as_bytes());
        }
        w.end().expect("end");
    }
    w.finish(1).expect("finish");
}

/// The keys of one kind, in an order that is not the file's.
fn probes(keys: usize, kinds: &[usize], n: usize) -> Vec<Vec<u8>> {
    let pool: Vec<usize> = (0..keys).filter(|k| kinds.contains(&(k % 7))).collect();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    (0..n)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            key_of(pool[(seed >> 33) as usize % pool.len()])
        })
        .collect()
}

/// One pass of an operation over a reader: nanoseconds an item, and a sum
/// of what was read so that nothing is optimised away.
fn timed(items: usize, f: impl FnOnce() -> u64) -> (f64, u64) {
    let t = Instant::now();
    let sum = f();
    (t.elapsed().as_nanos() as f64 / items as f64, sum)
}

fn point<B: Bytes>(blob: &Blob<B>, keys: &[Vec<u8>]) -> u64 {
    let mut sum = 0u64;
    let mut f = |v: &[u8]| sum += v.len() as u64 + u64::from(v[0]);
    let f: &mut dyn FnMut(&[u8]) = &mut f;
    for k in keys {
        blob.read_all(k, &mut *f).expect("read");
    }
    sum
}

fn scan<B: Bytes>(blob: &Blob<B>) -> u64 {
    let mut sum = 0u64;
    let mut f = |k: &[u8], v: &[u8]| sum += (k.len() + v.len()) as u64;
    let f: &mut dyn FnMut(&[u8], &[u8]) = &mut f;
    blob.scan(b"", usize::MAX, f).expect("scan");
    sum
}

const OPS: [&str; 5] = ["open", "read-inline", "read-short", "read-long", "scan"];

/// Every operation over one arm: the open, then `passes` passes of each
/// read over the reader it made. `[op][pass]`, and the sums to hold the arms
/// to each other.
fn arm<B: Bytes>(
    open: &dyn Fn() -> Blob<B>,
    sets: &[Vec<Vec<u8>>; 3],
    nkeys: usize,
    passes: usize,
) -> (Vec<Vec<f64>>, Vec<u64>) {
    let t = Instant::now();
    let blob = open();
    let mut ns = vec![vec![t.elapsed().as_nanos() as f64]];
    let mut sums = Vec::new();
    for set in sets {
        let mut row = Vec::new();
        for _ in 0..passes {
            let (n, s) = timed(set.len(), || point(&blob, set));
            row.push(n);
            sums.push(s);
        }
        ns.push(row);
    }
    let mut row = Vec::new();
    for _ in 0..passes {
        let (n, s) = timed(nkeys, || scan(&blob));
        row.push(n);
        sums.push(s);
    }
    ns.push(row);
    (ns, sums)
}

/// The file's pages out of the page cache: nothing maps it when this is
/// called, and it was synced when it was written.
fn evict(path: &Path) {
    use std::os::fd::AsRawFd;
    let f = std::fs::File::open(path).expect("open");
    // SAFETY: advice about a descriptor this function owns.
    let rc = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    assert_eq!(rc, 0, "posix_fadvise");
}

/// This thread's major faults so far.
fn major_faults() -> i64 {
    // SAFETY: a query that fills the struct handed to it.
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_THREAD, &mut u);
        u.ru_majflt
    }
}

/// Reads from cold: the reader opened over a file with no page in the
/// cache, random advice, and each key read once. Major faults at the open,
/// major faults a read, and microseconds a read.
fn cold<B: Bytes>(path: &Path, open: &dyn Fn() -> Blob<B>, keys: &[Vec<u8>]) -> (i64, f64, f64) {
    evict(path);
    let f0 = major_faults();
    let blob = open();
    blob.advise_random();
    let f1 = major_faults();
    let (ns, _) = timed(keys.len(), || point(&blob, keys));
    let f2 = major_faults();
    (f1 - f0, (f2 - f1) as f64 / keys.len() as f64, ns / 1e3)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keys: usize = args.first().map_or(12_000, |s| s.parse().expect("KEYS"));
    let reps: usize = args.get(1).map_or(5, |s| s.parse().expect("REPS"));
    let passes = 4;
    let shifts = [12u32, 14, 16, 18];

    let dir: PathBuf =
        std::env::temp_dir().join(format!("supdb-crypt-cost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let plain = dir.join("segment.sup");
    build(&plain, keys);
    let plain_len = std::fs::metadata(&plain).unwrap().len();
    let enc = |shift: u32| dir.join(format!("segment.{shift}.enc"));

    // The cipher alone, and the writer.
    println!("segment: {keys} keys, {:.1} MB", plain_len as f64 / 1e6);
    for n in [4usize << 10, 64 << 10] {
        let (aead, mut buf) = (gcm(), vec![3u8; n]);
        let rounds = (256 << 20) / n;
        let t = Instant::now();
        let mut ok = true;
        for i in 0..rounds as u64 {
            let mut nonce = [0u8; NONCE];
            nonce[..8].copy_from_slice(&i.to_le_bytes());
            let tag = aead.encrypt(&nonce, &[0; 48], &mut buf);
            ok &= aead.decrypt(&nonce, &[0; 48], &mut buf, &tag);
        }
        assert!(ok);
        let mb_s = 2.0 * (rounds * n) as f64 / 1e6 / t.elapsed().as_secs_f64();
        println!("cipher: AES-256-GCM in {n}-byte pieces, {mb_s:.0} MB/s");
    }
    for &shift in &shifts {
        let src = MmapBytes::open(&plain).unwrap();
        let mut out = std::fs::File::create(enc(shift)).unwrap();
        let opts = EncryptOptions {
            chunk_shift: shift,
            key_id: 0,
        };
        let t = Instant::now();
        crypt::encrypt(&src, &mut out, &gcm(), &opts).unwrap();
        let mb_s = plain_len as f64 / 1e6 / t.elapsed().as_secs_f64();
        out.sync_all().unwrap();
        println!("encrypt@{shift}: {mb_s:.0} MB/s");
    }
    std::fs::File::open(&plain).unwrap().sync_all().unwrap();

    let sets = [
        probes(keys, &[0, 1, 2, 5], 20_000),
        probes(keys, &[3, 6], 20_000),
        probes(keys, &[4], 4_000),
    ];
    // [arm][op] -> first passes and later passes, over the repetitions.
    let mut names: Vec<String> = vec!["plain-mmap".into(), "plain-copy".into()];
    for s in shifts {
        names.push(format!("enc-copy@{s}"));
        names.push(format!("enc-kept@{s}"));
    }
    names.push("kept+populate".into());
    names.push("kept+huge".into());
    let mut first = vec![vec![Vec::new(); OPS.len()]; names.len()];
    let mut later = vec![vec![Vec::new(); OPS.len()]; names.len()];
    let mut truth: Option<Vec<u64>> = None;
    for _ in 0..reps {
        let mut results = vec![
            arm(
                &|| Blob::open(MmapBytes::open(&plain).unwrap()).unwrap(),
                &sets,
                keys,
                passes,
            ),
            arm(
                &|| Blob::open(Copying(MmapBytes::open(&plain).unwrap())).unwrap(),
                &sets,
                keys,
                passes,
            ),
        ];
        for s in shifts {
            let stored = || MmapBytes::open(&enc(s)).unwrap();
            let nothing = ReadOptions {
                keep_bytes: 0,
                ..ReadOptions::default()
            };
            results.push(arm(
                &|| Blob::open(Encrypted::open_with(stored(), gcm(), &nothing).unwrap()).unwrap(),
                &sets,
                keys,
                passes,
            ));
            results.push(arm(
                &|| Blob::open(Encrypted::open(stored(), gcm()).unwrap()).unwrap(),
                &sets,
                keys,
                passes,
            ));
        }
        for opts in [
            ReadOptions {
                populate: true,
                ..ReadOptions::default()
            },
            ReadOptions {
                huge_pages: true,
                ..ReadOptions::default()
            },
        ] {
            let stored = || MmapBytes::open(&enc(crypt::DEFAULT_CHUNK_SHIFT)).unwrap();
            results.push(arm(
                &|| Blob::open(Encrypted::open_with(stored(), gcm(), &opts).unwrap()).unwrap(),
                &sets,
                keys,
                passes,
            ));
        }
        for (a, (ns, sums)) in results.into_iter().enumerate() {
            let want = truth.get_or_insert_with(|| sums.clone());
            assert_eq!(&sums, want, "{} read something else", names[a]);
            for (op, row) in ns.into_iter().enumerate() {
                first[a][op].push(row[0]);
                later[a][op].extend_from_slice(&row[1.min(row.len() - 1)..]);
            }
        }
    }

    println!("\nns an item (the open: microseconds); first pass / median of the passes after");
    print!("{:<14}", "");
    for op in OPS {
        print!("{op:>22}");
    }
    println!();
    for (a, name) in names.iter().enumerate() {
        print!("{name:<14}");
        for op in 0..OPS.len() {
            let scale = if op == 0 { 1e-3 } else { 1.0 };
            let f = median(&mut first[a][op]) * scale;
            if op == 0 {
                print!("{f:>22.0}");
            } else {
                let l = median(&mut later[a][op]) * scale;
                print!("{:>22}", format!("{f:.0} / {l:.0}"));
            }
        }
        println!();
    }

    // Out of the page cache: distinct keys, each read once.
    let mut once = probes(keys, &[3, 6], 3_000);
    once.sort();
    once.dedup();
    println!(
        "\nfrom cold, random advice: major faults at the open, major faults a read, us a read"
    );
    let nothing = ReadOptions {
        keep_bytes: 0,
        ..ReadOptions::default()
    };
    let row = |name: &str, (at_open, a_read, us): (i64, f64, f64)| {
        println!("{name:<14}{at_open:>8}{a_read:>10.2}{us:>10.1}");
    };
    row(
        "plain-mmap",
        cold(
            &plain,
            &|| Blob::open(MmapBytes::open(&plain).unwrap()).unwrap(),
            &once,
        ),
    );
    for s in shifts {
        let (path, stored) = (enc(s), || MmapBytes::open(&enc(s)).unwrap());
        row(
            &format!("enc-copy@{s}"),
            cold(
                &path,
                &|| Blob::open(Encrypted::open_with(stored(), gcm(), &nothing).unwrap()).unwrap(),
                &once,
            ),
        );
        row(
            &format!("enc-kept@{s}"),
            cold(
                &path,
                &|| Blob::open(Encrypted::open(stored(), gcm()).unwrap()).unwrap(),
                &once,
            ),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
