//! `Blob::scan_at` through the public API, apart from the engine's scan: a
//! segment of the suite's shape written once with `SegmentWriter`, opened
//! as a `Blob<MmapBytes>`, and `SCANS` scans of `LEN` entries from uniform
//! keys, each a `seek` and a `scan_at` with a callback that reads every
//! value, inside one function (`scans`) for a callgrind toggle to collect.
//!
//! The engine reaches `scan_at` through its own walk; a caller reaches it
//! here, with its own callback as the walk's only taker, which is the
//! instantiation a change to the reader's dispatch would cost.
//! `scripts/cost.sh`'s `scan_at` probe counts it by function against a base.
//!
//!     scan_at_probe [KEYS] [SCANS] [LEN]
use supdb::{Blob, MmapBytes, Options, SegmentOptions, SegmentWrite, SegmentWriter};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

/// `starts.len()` scans of `len` entries: what the toggle collects.
#[inline(never)]
fn scans(blob: &Blob<MmapBytes>, starts: &[[u8; 16]], len: usize) -> (usize, u64) {
    let mut seen = 0usize;
    let mut sum = 0u64;
    for k in starts {
        seen += blob
            .scan_at(blob.seek(k), len, |key, v| {
                // Each value read at both ends, so its bytes are touched
                // and the callback's own work stays a few instructions.
                let ends = v.first().map_or(0, |b| *b as u64) + v.last().map_or(0, |b| *b as u64);
                sum = sum.wrapping_add(key.len() as u64 + v.len() as u64 + ends);
            })
            .expect("the scan reads");
    }
    (seen, std::hint::black_box(sum))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let len: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(100);
    let payload = Payload::new(100, 0.5, 0xE1);
    let mut vrng = Rng::new(0xE1);
    let dir = std::env::temp_dir().join(format!("supdb-scanatprobe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("seg.sup");
    let write = SegmentWrite {
        inline_max: Options::default().inline_bytes,
        ..SegmentWrite::default()
    };
    let mut w = SegmentWriter::create_with(&path, &SegmentOptions::default(), &write, 0)
        .expect("the writer opens");
    let mut kb = [0u8; 16];
    for i in 0..n {
        db_key_into(i, &mut kb);
        w.begin(&kb).expect("a key in order");
        w.value(payload.get(&mut vrng));
        w.end().expect("the record ends");
    }
    w.finish(1).expect("the segment finishes");
    let blob = Blob::open(MmapBytes::open(&path).expect("the segment maps")).expect("it opens");
    let mut g = KeyGen::new(KeyDist::Uniform, n, 0x1A7);
    let starts: Vec<[u8; 16]> = (0..count)
        .map(|_| {
            let mut k = [0u8; 16];
            db_key_into(g.next(), &mut k);
            k
        })
        .collect();
    let t = std::time::Instant::now();
    let (seen, sum) = scans(&blob, &starts, len);
    println!(
        "scans {count} of {len} entries {seen} sum {sum} us {}",
        t.elapsed().as_micros()
    );
    drop(blob);
    let _ = std::fs::remove_dir_all(&dir);
}
