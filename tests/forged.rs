//! The read-only readers held to a file written by someone who can
//! compute a checksum. A segment's key section carries a row of CRC32C
//! words over its pieces, which stops an accident and not an author: the
//! damage tests flip a byte and the row refuses the file before any deeper
//! check runs, so what the parsers do with bytes that are wrong and
//! checksummed right is something no test had asked.
//!
//! Here a segment's key section is changed and its row computed again, so
//! the open's verification passes, and every reader is run over the result:
//! each must answer with an error or with some answer, and never panic,
//! and never run without end. What it answers is not held to anything: a
//! forged index is an index of something else.

use supdb::bytes::{Bytes, SliceBytes};
use supdb::flatindex;
use supdb::{Blob, SegmentOptions, SegmentWriter, SparseBlob};

mod common;
use common::{kv, vf};

fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A source that copies, so the readers that own their sections are run as
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

/// A segment of a few hundred keys in every shape a record takes: values
/// inline and in blocks, runs of one and of many, fixed width and not, an
/// empty value, a key of every length up to a few dozen bytes.
fn segment(seed: u64) -> (Vec<u8>, Vec<Vec<u8>>) {
    let dir = std::env::temp_dir().join(format!("supdb-next-forged-{seed}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("seg.sup");
    let mut r = seed;
    let mut keys: Vec<Vec<u8>> = (0..400)
        .map(|i| {
            let pad = (splitmix(&mut r) % 24) as usize;
            format!("k{i:05}{}", "x".repeat(pad)).into_bytes()
        })
        .collect();
    keys.sort();
    keys.dedup();
    let opts = SegmentOptions {
        block_size: 4096,
        checksums: true,
        ..SegmentOptions::default()
    };
    let mut w = SegmentWriter::create(&path, &opts).unwrap();
    w.set_inline_max(64);
    for k in &keys {
        w.begin(k).unwrap();
        let n = match splitmix(&mut r) % 10 {
            0 => 0,
            1 => 40,
            _ => 1 + (splitmix(&mut r) % 3) as usize,
        };
        for _ in 0..n {
            let len = match splitmix(&mut r) % 20 {
                0 => 3000,
                1 => 0,
                _ => 8 + (splitmix(&mut r) % 120) as usize,
            };
            let v: Vec<u8> = (0..len).map(|_| splitmix(&mut r) as u8).collect();
            w.value(&v);
        }
        w.end().unwrap();
    }
    w.finish(1).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (bytes, keys)
}

/// Every reader over `bytes`, to its end: the open, every key it was
/// written with and some it was not, the walks by rank, the scans and the
/// counts, through a source that lends and one that copies, and the sparse
/// reader's plans. Nothing is asked of the answers.
fn read_everything(bytes: &[u8], keys: &[Vec<u8>], seed: u64) {
    let mut r = seed;
    let probes: Vec<Vec<u8>> = keys
        .iter()
        .cloned()
        .chain((0..40).map(|_| {
            let n = (splitmix(&mut r) % 20) as usize;
            (0..n).map(|_| splitmix(&mut r) as u8).collect()
        }))
        .chain([Vec::new(), vec![0xff; 40]])
        .collect();
    fn run<B: Bytes>(blob: &Blob<B>, probes: &[Vec<u8>]) {
        let mut sink = 0u64;
        let n = blob.keys().min(100_000);
        for k in probes {
            let _ = blob.read_all(k, vf(&mut |v| sink += v.len() as u64));
            let _ = blob.count(k);
            let rank = blob.seek(k);
            let _ = blob.key_at(rank);
            let _ = blob.scan_at(rank, 8, kv(&mut |k, v| sink += (k.len() + v.len()) as u64));
        }
        for rank in (0..n).step_by(7) {
            let _ = blob.key_at(rank);
            if let Some((_, exts)) = blob.exts_at(rank) {
                sink += exts.len() as u64;
            }
            let _ = blob.values_at(rank, vf(&mut |v| sink += v.len() as u64));
        }
        let _ = blob.scan_at(0, n, kv(&mut |k, v| sink += (k.len() + v.len()) as u64));
        let mut seen = 0usize;
        let _ = blob.scan_counts(b"", 100_000, |_k: &[u8], c: u64| {
            sink += c;
            seen += 1;
            seen < 100_000
        });
        std::hint::black_box(sink);
    }
    if let Ok(blob) = Blob::open(SliceBytes(bytes)) {
        run(&blob, &probes);
    }
    if let Ok(blob) = Blob::open(Copying(bytes.to_vec())) {
        run(&blob, &probes);
    }
    if let Ok(sparse) = SparseBlob::open(SliceBytes(bytes)) {
        let mut sink = 0u64;
        let _ = sparse.keys();
        for k in probes.iter().take(60) {
            sink += sparse.dictionary_plan(k, None).len() as u64;
            sink += sparse
                .dictionary_plan_records(k, None)
                .map_or(0, |p| p.len() as u64);
        }
        let mut seen = 0usize;
        let _ = sparse.dictionary_counts(b"", None, |_k: &[u8], c: u64| {
            sink += c;
            seen += 1;
            seen < 100_000
        });
        std::hint::black_box(sink);
    }
}

/// `seed`'s forgery of `pristine`: a few bytes of the key section changed,
/// a byte or a whole word, and the section's row computed again over what
/// it holds now.
fn forge(pristine: &[u8], seed: u64) -> Option<Vec<u8>> {
    let blob = Blob::open(SliceBytes(pristine)).ok()?;
    let facts = blob.z_source_facts()?;
    let (base, len) = (facts.key_off as usize, facts.crc_off as usize);
    let mut bytes = pristine.to_vec();
    let mut r = seed ^ 0x5eed;
    for _ in 0..1 + splitmix(&mut r) % 3 {
        // Half the changes in the header's words, where a field read
        // wrong sends every reader somewhere else; half anywhere.
        let at = if splitmix(&mut r).is_multiple_of(2) {
            (splitmix(&mut r) % 192) as usize
        } else {
            (splitmix(&mut r) % len as u64) as usize
        };
        match splitmix(&mut r) % 4 {
            0 => bytes[base + at] ^= 1 << (splitmix(&mut r) % 8),
            1 => bytes[base + at] = splitmix(&mut r) as u8,
            2 => bytes[base + at] = 0xff,
            _ => {
                let word = splitmix(&mut r).to_le_bytes();
                let end = (at + 8).min(len);
                bytes[base + at..base + end].copy_from_slice(&word[..end - at]);
            }
        }
    }
    let row = flatindex::checksum_row(
        &bytes[base..base + len],
        flatindex::PIECE_SHIFT,
        base as u64,
    );
    bytes[base + len..base + len + row.len()].copy_from_slice(&row);
    Some(bytes)
}

/// Every forgery of a range of seeds read to its end by every reader, each
/// on a thread of its own with a clock on it: a panic and a read that does
/// not return are the two failures, named by the seed.
#[test]
fn a_key_section_forged_under_a_valid_row_panics_and_stalls_no_reader() {
    let from: u64 = std::env::var("FORGED_FROM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let to: u64 = std::env::var("FORGED_TO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(from + 400);
    let (pristine, keys) = segment(7);
    read_everything(&pristine, &keys, 0);
    let (mut opened, mut bad) = (0u64, Vec::new());
    for seed in from..to {
        let Some(bytes) = forge(&pristine, seed) else {
            panic!("the pristine segment has no row to forge");
        };
        if Blob::open(SliceBytes(&bytes)).is_ok() {
            opened += 1;
        }
        let keys = keys.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::Builder::new()
            .name(format!("forged-{seed}"))
            .spawn(move || {
                let r = std::panic::catch_unwind(|| read_everything(&bytes, &keys, seed));
                let _ = tx.send(r.is_ok());
            })
            .unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(true) => {
                let _ = t.join();
            }
            Ok(false) => bad.push(format!("seed {seed}: a reader panicked")),
            Err(_) => {
                bad.push(format!("seed {seed}: a reader did not return"));
                break;
            }
        }
    }
    eprintln!(
        "forged {from}..{to}: {opened} opened under their rows, {} failures",
        bad.len()
    );
    for b in &bad {
        eprintln!("FAIL: {b}");
    }
    // A forgery that never opens is a test of the open alone.
    assert!(opened > (to - from) / 4, "{opened} forgeries opened");
    assert!(bad.is_empty(), "{} forgeries broke a reader", bad.len());
}
