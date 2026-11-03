//! ycsb-E's first pass against its second, at one rung, arms interleaved.
//!
//! The suite records E once, on the store the mixes before it wrote, so every
//! E number a row carries is a cold-cache pass. This runs the same mix twice
//! on the same store and prints both, so the build's share of the first pass
//! is visible rather than inferred.
use std::time::Instant;
use supdb_bench::engines::{self, Batch, Engine};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

const YCSB_SCAN: usize = 50;
const YCSB_BATCH: usize = 100;
const YCSB: [(char, u32, u32, u32, u32, u32, KeyDist); 6] = [
    ('A', 50, 50, 0, 0, 0, KeyDist::Zipfian),
    ('B', 95, 5, 0, 0, 0, KeyDist::Zipfian),
    ('C', 100, 0, 0, 0, 0, KeyDist::Zipfian),
    ('D', 95, 0, 5, 0, 0, KeyDist::Uniform),
    ('E', 0, 0, 5, 95, 0, KeyDist::Zipfian),
    ('F', 50, 0, 0, 0, 50, KeyDist::Zipfian),
];
fn ycsb_ops(size: u64) -> u64 {
    (size / 6).max(1_000)
}

fn mix(
    e: &mut dyn Engine,
    w: &(char, u32, u32, u32, u32, u32, KeyDist),
    size: u64,
    inserted: &mut u64,
    payload: &Payload,
    salt: u64,
) -> f64 {
    let (letter, pread, pupd, pins, pscan, prmw, dist) = *w;
    let ops = ycsb_ops(size);
    let mut keys = KeyGen::new(dist, size, 0x9C5B ^ letter as u64 ^ salt);
    let mut pick = Rng::new(0x5EED ^ letter as u64 ^ salt);
    let mut vrng = Rng::new(0xE2);
    let mut kb = [0u8; 16];
    let mut wbuf = Batch::with_capacity(YCSB_BATCH, payload.value_size());
    let mut is_insert = false;
    let t = Instant::now();
    for _ in 0..ops {
        let roll = pick.below(100) as u32;
        if roll < pread {
            db_key_into(keys.next(), &mut kb);
            e.get(&kb).unwrap();
        } else if roll < pread + pupd {
            db_key_into(keys.next(), &mut kb);
            wbuf.push(&kb, payload.get(&mut vrng));
            is_insert = false;
        } else if roll < pread + pupd + pins {
            db_key_into(size + *inserted, &mut kb);
            *inserted += 1;
            wbuf.push(&kb, payload.get(&mut vrng));
            is_insert = true;
        } else if roll < pread + pupd + pins + pscan {
            db_key_into(keys.next(), &mut kb);
            e.range(&kb, YCSB_SCAN).unwrap();
        } else if prmw > 0 {
            db_key_into(keys.next(), &mut kb);
            e.get(&kb).unwrap();
            wbuf.push(&kb, payload.get(&mut vrng));
            is_insert = false;
        }
        if wbuf.len() >= YCSB_BATCH {
            if is_insert {
                wbuf.flush(e).unwrap()
            } else {
                wbuf.flush_updates(e).unwrap()
            }
        }
    }
    if is_insert {
        wbuf.flush(e).unwrap()
    } else {
        wbuf.flush_updates(e).unwrap()
    }
    t.elapsed().as_secs_f64()
}

/// This thread's minor page faults so far, from `/proc/thread-self/stat`.
fn minflt_self() -> u64 {
    let Ok(stat) = std::fs::read_to_string("/proc/thread-self/stat") else {
        return 0;
    };
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(7).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

fn main() {
    let mut a = std::env::args().skip(1);
    let size: u64 = a.next().unwrap().parse().unwrap();
    let rounds: usize = a.next().unwrap().parse().unwrap();
    let arms: Vec<String> = a.next().unwrap().split(',').map(str::to_string).collect();
    let dir = std::env::temp_dir().join(format!("supdb-epass-{}", std::process::id()));
    let payload = Payload::new(100, 0.5, 0xE1);
    println!("{size} keys, {rounds} rounds, arms interleaved within a round");
    for round in 1..=rounds {
        for arm in &arms {
            let d = dir.join(arm);
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let mut e = engines::open(arm, &d, 3).unwrap();
            // The load, then the mixes in the suite's order, then E twice.
            let mut b = Batch::with_capacity(1000, 100);
            let mut vr = Rng::new(0xE2);
            let mut kb = [0u8; 16];
            for i in 0..size {
                db_key_into(i, &mut kb);
                b.push(&kb, payload.get(&mut vr));
                if b.len() >= 1000 {
                    b.flush(e.as_mut()).unwrap();
                }
            }
            b.flush(e.as_mut()).unwrap();
            e.sync().unwrap();
            let mut inserted = 0u64;
            for l in ['C', 'B', 'A', 'F', 'D'] {
                let w = YCSB.iter().find(|w| w.0 == l).unwrap();
                mix(e.as_mut(), w, size, &mut inserted, &payload, 0);
            }
            let w = YCSB.iter().find(|w| w.0 == 'E').unwrap();
            // The store's counters around each pass: the scans' phases
            // before their walk, the thread's time and the blocks built,
            // which say what a pass's time was made of. Read between the
            // passes only, which is where the suite reads nothing either.
            let c0 = e.counters();
            let f0 = minflt_self();
            supdb_bench::run::lag_mark(arm, 1, "start");
            let first = mix(e.as_mut(), w, size, &mut inserted, &payload, 0);
            supdb_bench::run::lag_mark(arm, 1, "end");
            let f1 = minflt_self();
            let c1 = e.counters();
            let f1b = minflt_self();
            supdb_bench::run::lag_mark(arm, 2, "start");
            let second = mix(e.as_mut(), w, size, &mut inserted, &payload, 1);
            supdb_bench::run::lag_mark(arm, 2, "end");
            let f2 = minflt_self();
            let c2 = e.counters();
            let ops = ycsb_ops(size) as f64;
            println!("  round {round}  {arm:<16} E first {:>9.0} ops/s   second {:>9.0} ops/s   second/first {:.2}x",
                     ops / first, ops / second, first / second);
            let get = |c: &[(&str, f64)], n: &str| {
                c.iter().find(|(k, _)| *k == n).map_or(0.0, |(_, v)| *v)
            };
            for (name, a, b, flt) in [("first", &c0, &c1, f1 - f0), ("second", &c1, &c2, f2 - f1b)]
            {
                let d = |n: &str| get(b, n) - get(a, n);
                println!(
                    "    {name}: pass {:.1} ms flt {flt} | ph take {:.2} sync {:.2} settle {:.2} snap {:.2} ahead {:.2} install {:.2} ms | thread {:.1} ms/{:.0}p | built up {:.0} rd {:.0} eng {:.0} | snaps {:.0} refr {:.0} | walks {:.0}/{:.0}/{:.0} starts {:.0}/{:.0}",
                    (if name == "first" { first } else { second }) * 1e3,
                    d("scan_take_us") / 1e3, d("scan_sync_us") / 1e3, d("scan_settle_us") / 1e3,
                    d("scan_snap_us") / 1e3, d("scan_ahead_us") / 1e3, d("scan_install_us") / 1e3,
                    d("upkeep_cpu_us") / 1e3, d("upkeep_passes"),
                    d("blk_by_upkeep"), d("blk_by_reader"), d("blk_by_engine"),
                    d("snapshot_builds"), d("snapshot_refreshes"),
                    d("walk_copy"), d("walk_sparse"), d("walk_other"),
                    d("start_at_rank"), d("start_search")
                );
            }
            drop(e);
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
