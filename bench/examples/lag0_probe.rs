//! The lag sweep's first point in its own shape, scan by scan: a fresh
//! store per round for each arm, the shuffled load of `size` keys through
//! `Batch` flushes of a thousand, its `sync`, the sweep's `sync` at the 0%
//! point, then `size / 100` scans of a hundred entries from uniform keys,
//! each timed alone. The arms alternate round by round, the order swapped
//! every other round, and each round prints the first three scans and the
//! median beside the pass's sum, for a pass whose difference sits in its
//! first operations.
//!
//!     lag0_probe <armA,armB> <size> <rounds>
use std::time::Instant;
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Permutation, Rng};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arms: Vec<String> = args
        .get(1)
        .cloned()
        .unwrap_or("supdb,supdb-sealcopyfar".into())
        .split(',')
        .map(String::from)
        .collect();
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30_000);
    let rounds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let payload = Payload::new(100, 0.5, 0xE1);
    let scans = (size / 100).max(100);
    for round in 0..rounds {
        let order: Vec<&String> = if round % 2 == 0 {
            arms.iter().collect()
        } else {
            arms.iter().rev().collect()
        };
        for arm in order {
            let dir = std::env::temp_dir().join(format!(
                "supdb-lag0probe-{}-{round}-{arm}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut e = engines::open(arm, &dir, 1).unwrap();
            let perm = Permutation::new(size, 0x5EED);
            let mut vrng = Rng::new(0xE1);
            let mut buf = Batch::with_capacity(1000, payload.value_size());
            let mut kb = [0u8; 16];
            for i in 0..size {
                db_key_into(perm.at(i), &mut kb);
                buf.push(&kb, payload.get(&mut vrng));
                if buf.len() == 1000 {
                    buf.flush(e.as_mut()).unwrap();
                }
            }
            buf.flush(e.as_mut()).unwrap();
            e.sync().unwrap();
            e.sync().unwrap();
            let mut g = KeyGen::new(KeyDist::Uniform, size, 0x1A7);
            let mut ns: Vec<u64> = Vec::with_capacity(scans as usize);
            for _ in 0..scans {
                db_key_into(g.next(), &mut kb);
                let t = Instant::now();
                e.range(&kb, 100).unwrap();
                ns.push(t.elapsed().as_nanos() as u64);
            }
            let sum: u64 = ns.iter().sum();
            let mut sorted = ns.clone();
            sorted.sort_unstable();
            println!(
                "round {round} {arm} first {} {} {} median {} sum_us {}",
                ns[0],
                ns[1],
                ns[2],
                sorted[sorted.len() / 2],
                sum / 1000
            );
            drop(e);
            if std::env::var_os("LAG0_KEEP").is_none() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}
