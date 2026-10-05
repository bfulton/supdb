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
        let t = Instant::now();
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        let loaded = t.elapsed();
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
