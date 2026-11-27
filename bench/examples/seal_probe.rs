//! A segment written through the public writer, apart from the engine:
//! `SegmentWriter::create_with`, every key's `begin`, `value` and `end`,
//! and the `finish` that composes the trailer and syncs, inside one
//! function (`seal`) for a callgrind toggle to collect.
//!
//! The keys are the suite's shape (sixteen digits, values of a hundred
//! bytes), sorted, written with the engine's inline limit, so the segment
//! is the one a seal of the same keys writes. `scripts/cost.sh`'s `seal`
//! probe counts it by function against a base, four segments of a quarter
//! of the keys each, so that a cost a segment is four times its count over
//! the run and a few instructions a segment are past the toggle's slack.
//!
//!     seal_probe [KEYS] [ROUNDS]
use std::path::Path;
use supdb::{Options, SegmentOptions, SegmentWrite, SegmentWriter};
use supdb_bench::workload::{db_key_into, Payload, Rng};

/// One segment of `keys` keys at `path`: what the toggle collects.
#[inline(never)]
fn seal(path: &Path, keys: &[[u8; 16]], vals: &[Vec<u8>], write: &SegmentWrite) -> u64 {
    let opts = SegmentOptions::default();
    let mut w = SegmentWriter::create_with(path, &opts, write, 0).expect("the writer opens");
    for (k, v) in keys.iter().zip(vals) {
        w.begin(k).expect("a key in order");
        w.value(v);
        w.end().expect("the record ends");
    }
    w.finish(1).expect("the segment finishes");
    keys.len() as u64
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let rounds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let payload = Payload::new(100, 0.5, 0xE1);
    let mut vrng = Rng::new(0xE1);
    let mut keys = vec![[0u8; 16]; n as usize];
    for (i, k) in keys.iter_mut().enumerate() {
        db_key_into(i as u64, k);
    }
    let vals: Vec<Vec<u8>> = (0..n).map(|_| payload.get(&mut vrng).to_vec()).collect();
    let write = SegmentWrite {
        inline_max: Options::default().inline_bytes,
        ..SegmentWrite::default()
    };
    let dir = std::env::temp_dir().join(format!("supdb-sealprobe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for round in 0..rounds {
        let path = dir.join(format!("seg-{round}.sup"));
        let t = std::time::Instant::now();
        let written = seal(&path, &keys, &vals, &write);
        println!(
            "round {round} keys {written} bytes {} us {}",
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            t.elapsed().as_micros()
        );
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
