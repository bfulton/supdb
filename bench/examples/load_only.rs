//! The suite's ordered load alone, for a profile: `size` keys in key order
//! through `Batch` flushes of a thousand, ending in the `sync` the suite's
//! load ends in, on one arm; `reps` stores in turn. Prints ops/s a store.
//!
//!     load_only <arm> <size> <reps>
use std::time::Instant;
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, Payload, Rng};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).cloned().unwrap_or("supdb-ingest".into());
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300_000);
    let reps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let payload = Payload::new(100, 0.5, 0xE1);
    let root = std::env::temp_dir().join(format!("supdb-loadonly-{}", std::process::id()));
    let mut kb = [0u8; 16];
    for rep in 0..reps {
        let dir = root.join(format!("r{rep}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = engines::open(&arm, &dir, 1).unwrap();
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        // Each batch's flush timed on its own, for the shape of the
        // commits: the first few, the median and the slowest.
        let mut batches: Vec<f64> = Vec::new();
        let t = Instant::now();
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                let tb = Instant::now();
                buf.flush(e.as_mut()).unwrap();
                batches.push(tb.elapsed().as_secs_f64() * 1e6);
            }
        }
        buf.flush(e.as_mut()).unwrap();
        let loaded = t.elapsed();
        let mut sorted = batches.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = sorted[sorted.len() / 2];
        let slow = batches.iter().filter(|&&b| b > 2.0 * med).count();
        let slow_sum: f64 = batches.iter().filter(|&&b| b > 2.0 * med).sum();
        let slow_at: Vec<String> = batches
            .iter()
            .enumerate()
            .filter(|(_, &b)| b > 2.0 * med)
            .take(16)
            .map(|(i, b)| format!("{i}:{b:.0}"))
            .collect();
        println!(
            "  batches us: first {:.0} {:.0} {:.0} median {:.0} p90 {:.0} max {:.0}; {} over 2x median summing {:.1} ms",
            batches.first().copied().unwrap_or(0.0),
            batches.get(1).copied().unwrap_or(0.0),
            batches.get(2).copied().unwrap_or(0.0),
            med,
            sorted[sorted.len() * 9 / 10],
            sorted[sorted.len() - 1],
            slow,
            slow_sum / 1e3
        );
        println!("  slow batches (index:us): {}", slow_at.join(" "));
        e.sync().unwrap();
        let all = t.elapsed();
        println!(
            "{arm} rep {rep}: {:.2} M ops/s over the commits, {:.2} M ops/s with the sync ({:.1} ms + {:.1} ms)",
            size as f64 / loaded.as_secs_f64() / 1e6,
            size as f64 / all.as_secs_f64() / 1e6,
            loaded.as_secs_f64() * 1e3,
            (all - loaded).as_secs_f64() * 1e3
        );
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
