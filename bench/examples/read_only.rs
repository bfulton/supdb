//! The suite's ordered load and its sync on one arm, then the point-read
//! pass alone, timed in ten windows so a pass that starts slow and ends
//! fast reads apart from one that is slow throughout. The store's counters
//! are read after the sync and after each pass, never inside one, and the
//! pass is run a second time after a second's pause so work the sync left
//! in flight has landed before it.
use std::time::{Duration, Instant};
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

fn get(c: &[(&'static str, f64)], n: &str) -> f64 {
    c.iter().find(|(m, _)| *m == n).map_or(0.0, |x| x.1)
}

fn shape(c: &[(&'static str, f64)]) -> String {
    format!(
        "segs {:.0} parts {:.0} pieces {:.0} unsealed {:.0} aligned {:.0} blk_r {:.0} blk_e {:.0}",
        get(c, "segments"),
        get(c, "partitions"),
        get(c, "pieces"),
        get(c, "unsealed_keys"),
        get(c, "pieces_aligned"),
        get(c, "blk_by_reader"),
        get(c, "blk_by_engine"),
    )
}

/// This thread's minor and major faults, user and system ticks, and its
/// voluntary and involuntary context switches, from procfs.
fn usage() -> [u64; 6] {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap_or_default();
    let after = stat.rsplit_once(')').map(|x| x.1).unwrap_or("");
    let f: Vec<u64> = after
        .split_whitespace()
        .map(|x| x.parse().unwrap_or(0))
        .collect();
    // After the comm: state is f[0], so minflt is f[7], majflt f[9], utime f[11], stime f[12].
    let status = std::fs::read_to_string("/proc/thread-self/status").unwrap_or_default();
    let sw = |k: &str| -> u64 {
        status
            .lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|x| x.parse().ok())
            .unwrap_or(0)
    };
    [
        f.get(7).copied().unwrap_or(0),
        f.get(9).copied().unwrap_or(0),
        f.get(11).copied().unwrap_or(0),
        f.get(12).copied().unwrap_or(0),
        sw("voluntary_ctxt_switches"),
        sw("nonvoluntary_ctxt_switches"),
    ]
}

fn pass(e: &mut dyn engines::Engine, size: u64, kb: &mut [u8; 16]) -> (f64, Vec<f64>, [u64; 6]) {
    let u0 = usage();
    let mut g = KeyGen::new(KeyDist::Uniform, size, 7);
    let windows = 10u64;
    let per = size / windows;
    let mut rates = Vec::new();
    let t = Instant::now();
    for _ in 0..windows {
        let tw = Instant::now();
        for _ in 0..per {
            db_key_into(g.next(), kb);
            e.get(kb).unwrap();
        }
        rates.push(per as f64 / tw.elapsed().as_secs_f64() / 1e6);
    }
    let secs = t.elapsed().as_secs_f64();
    let u1 = usage();
    let mut d = [0u64; 6];
    for i in 0..6 {
        d[i] = u1[i].saturating_sub(u0[i]);
    }
    ((per * windows) as f64 / secs / 1e6, rates, d)
}

fn fmt_usage(u: &[u64; 6]) -> String {
    format!(
        "minflt {} majflt {} utick {} stick {} vcsw {} nvcsw {}",
        u[0], u[1], u[2], u[3], u[4], u[5]
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).cloned().unwrap_or("supdb".into());
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300_000);
    let reps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    // What comes between the passes: `c` reads the counters (which takes
    // the writer's upkeep home) and `n` does not; then `sleep` a second,
    // `spin` a second on this thread, or `none`.
    let mode = args.get(4).cloned().unwrap_or("c-sleep".into());
    let (counters_between, wait) = mode.split_once('-').unwrap_or(("c", "sleep"));
    let payload = Payload::new(100, 0.5, 0xE1);
    let root = std::env::temp_dir().join(format!("supdb-readonly-{}", std::process::id()));
    let mut kb = [0u8; 16];
    for rep in 0..reps {
        let dir = root.join(format!("r{rep}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = engines::open(&arm, &dir, 1).unwrap();
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        let tl = Instant::now();
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        e.sync().unwrap();
        let load_ms = tl.elapsed().as_secs_f64() * 1e3;
        let c0 = e.counters();
        let (r1, w1, u1) = pass(e.as_mut(), size, &mut kb);
        if counters_between == "c" {
            let _ = e.counters();
        }
        match wait {
            "sleep" => std::thread::sleep(Duration::from_secs(1)),
            "spin" => {
                let t = Instant::now();
                let mut x = 0u64;
                while t.elapsed() < Duration::from_secs(1) {
                    x = x
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                }
                std::hint::black_box(x);
            }
            _ => {}
        }
        let (r2, w2, u2) = pass(e.as_mut(), size, &mut kb);
        let c3 = e.counters();
        let f = |w: &[f64]| {
            w.iter()
                .map(|x| format!("{x:.2}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!(
            "rep {rep} {arm} {size} {mode}: load {load_ms:.0}ms  after sync: {}",
            shape(&c0)
        );
        println!("  pass 1 {r1:.2}M/s [{}]  {}", f(&w1), fmt_usage(&u1));
        println!("  pass 2 {r2:.2}M/s [{}]  {}", f(&w2), fmt_usage(&u2));
        println!("  after: {}", shape(&c3));
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
