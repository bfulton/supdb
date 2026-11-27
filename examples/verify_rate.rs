//! The open's verification of a segment's key section against its checksum
//! row, timed beside a read of the same bytes.
//!
//! ```sh
//! cargo run --release --example verify_rate -- [KEYS] [ROUNDS]
//! ```
//!
//! A segment of `KEYS` keys (three million by default) of the suite's shape
//! -- sixteen decimal digits a key and a hundred-byte value held inline, so
//! that the key section is nearly the whole file -- is written through
//! `SegmentWriter` and read into memory, and opened from there
//! (`VecBytes`), so that the open reads no page from the device and takes
//! no fault on a mapping. Each round opens it with the key section's
//! verification and without it (`BlobOptions::verify_index`), and sums the
//! section's bytes a word at a time, which is what one core reads of
//! memory; the best round of each is printed, and the verification's rate
//! is the section's bytes over the difference of the two opens.

use std::time::{Duration, Instant};
use supdb::bytes::VecBytes;
use supdb::{Blob, BlobOptions, SegmentOptions, SegmentWriter};

fn main() {
    let mut args = std::env::args().skip(1);
    let keys: usize = args.next().map_or(3_000_000, |a| a.parse().expect("KEYS"));
    let rounds: usize = args.next().map_or(9, |a| a.parse().expect("ROUNDS"));
    let dir = std::env::temp_dir().join(format!("supdb-verify-rate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("segment.sup");
    let mut w = SegmentWriter::create(&path, &SegmentOptions::default()).unwrap();
    w.set_inline_max(256);
    let value = [b'v'; 100];
    for i in 0..keys {
        w.begin(format!("{i:016}").as_bytes()).unwrap();
        w.value(&value);
        w.end().unwrap();
    }
    w.finish(1).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    // The copy into the source is outside the clock: each open is handed a
    // fresh copy, the clone made before the clock starts.
    let open_timed = |verify: bool| {
        let src = VecBytes(bytes.clone());
        let t = Instant::now();
        let b = Blob::open_with(
            src,
            BlobOptions {
                verify_index: verify,
                ..BlobOptions::default()
            },
        )
        .unwrap();
        let took = t.elapsed();
        std::hint::black_box(&b);
        took
    };
    let section = Blob::open(VecBytes(bytes.clone())).unwrap().index_bytes();
    let words = bytes.as_chunks::<8>().0;
    let mut best = [Duration::MAX; 3];
    for _ in 0..rounds {
        best[0] = best[0].min(open_timed(true));
        best[1] = best[1].min(open_timed(false));
        let t = Instant::now();
        let mut sum = 0u64;
        for w in words {
            sum = sum.wrapping_add(u64::from_le_bytes(*w));
        }
        std::hint::black_box(sum);
        best[2] = best[2].min(t.elapsed());
    }
    let gbs = |n: usize, d: Duration| n as f64 / d.as_secs_f64() / 1e9;
    let verify = best[0].saturating_sub(best[1]);
    println!(
        "{keys} keys, a key section of {:.1} MB in a file of {:.1} MB, best of {rounds}",
        section as f64 / 1e6,
        bytes.len() as f64 / 1e6
    );
    println!(
        "open verified {:.1} ms, unverified {:.1} ms: the verification {:.1} ms, {:.2} GB/s",
        best[0].as_secs_f64() * 1e3,
        best[1].as_secs_f64() * 1e3,
        verify.as_secs_f64() * 1e3,
        gbs(section, verify)
    );
    println!(
        "the file summed a word at a time: {:.1} ms, {:.2} GB/s",
        best[2].as_secs_f64() * 1e3,
        gbs(bytes.len(), best[2])
    );
}
