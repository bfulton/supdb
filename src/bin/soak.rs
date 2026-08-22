//! Sustained mixed load, to find what only shows up over time.
//!
//! Everything measured so far ran for seconds. The failure modes that matter
//! for a storage engine -- the free list fragmenting until nothing can be
//! reused, the file growing without bound against steady live data, merge
//! work compounding, throughput sagging as structures fill -- none of them
//! are visible in a thirty second run.
//!
//! The workload is deliberately churny: appends that fragment keys and force
//! merges, replacements that supersede whole values, and deletes that leave
//! tombstones, all against a working set that stays the same size. Live data
//! is therefore roughly constant, so any sustained growth in the file is
//! drift rather than data.

use std::time::{Duration, Instant};
use supdb::{Options, Reader, Reclaim, Store};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn key(k: u64) -> Vec<u8> {
    format!("k{:09}", k).into_bytes()
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |n: &str, d: usize| -> usize {
        args.iter().position(|a| a == n).map(|i| args[i + 1].parse().unwrap()).unwrap_or(d)
    };
    let secs = get("--seconds", 180) as u64;
    let keys = get("--keys", 200_000) as u64;
    let value_size = get("--value-size", 96);
    let snapshots = args.iter().any(|a| a == "--snapshots");

    let dir = std::env::temp_dir().join("supdb-soak");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("s.dat");

    let store = Store::create(
        &path,
        Options {
            buffer_bytes: 64 << 20,
            reclaim: if snapshots { Reclaim::Never } else { Reclaim::AfterReads },
            ..Default::default()
        },
    )?;

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut val = vec![b'v'; value_size];
    let deadline = Instant::now() + Duration::from_secs(secs);
    let t0 = Instant::now();
    let mut ops = 0u64;
    let mut last_report = Instant::now();
    let mut last_ops = 0u64;

    println!("{:>6} {:>12} {:>10} {:>10} {:>10} {:>9} {:>9} {:>8}",
             "sec", "ops", "ops/s", "file MB", "live MB", "free MB", "reused", "merges");
    while Instant::now() < deadline {
        for _ in 0..200_000 {
            let k = rng.next() % keys;
            val[..8].copy_from_slice(&rng.next().to_be_bytes());
            match rng.next() % 100 {
                0..=79 => store.append(&key(k), &val)?,   // fragments keys, drives merges
                80..=94 => store.put(&key(k), &val)?,     // supersedes
                _ => store.delete(&key(k))?,              // tombstones
            }
            ops += 1;
        }
        store.checkpoint()?;
        if last_report.elapsed() >= Duration::from_secs(15) {
            let file = std::fs::metadata(&path)?.len() as f64 / 1048576.0;
            let r = Reader::open(&path)?;
            let mut live = 0u64;
            for k in (0..keys).step_by(211) {
                r.read_all(&key(k), |v| live += v.len() as u64)?;
            }
            let live_mb = live as f64 * (keys as f64 / (keys as f64 / 211.0)) / 1048576.0 / 211.0;
            let dt = last_report.elapsed().as_secs_f64();
            println!("{:>6.0} {:>12} {:>10.0} {:>10.1} {:>10.1} {:>9} {:>9} {:>8}",
                     t0.elapsed().as_secs_f64(), ops, (ops - last_ops) as f64 / dt,
                     file, live_mb, "-", "-", "-");
            last_report = Instant::now();
            last_ops = ops;
        }
    }

    let stats = store.close()?;
    let file = std::fs::metadata(&path)?.len() as f64 / 1048576.0;
    println!("\nfinal: {ops} ops in {:.0}s, file {file:.1} MB", t0.elapsed().as_secs_f64());
    println!("  merges {} | slots reused {} ({:.1} MB) | free left {:.1} MB",
             stats.merges, stats.reused, stats.reused_bytes as f64 / 1048576.0,
             stats.free_bytes as f64 / 1048576.0);

    // everything still readable and self-consistent?
    let r = Reader::open(&path)?;
    let (mut present, mut errs) = (0u64, 0u64);
    for k in 0..keys {
        let mut n = 0;
        match r.read_all(&key(k), |_| n += 1) {
            Ok(_) => { if n > 0 { present += 1 } }
            Err(_) => errs += 1,
        }
    }
    println!("  keys indexed {} | with values {} | read errors {}", r.keys(), present, errs);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
