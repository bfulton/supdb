//! The scrambled E mix on the store the suite's mixes leave, inside a
//! function of its own, for callgrind to count by toggling on it.
//!
//! The suite runs the first family's mixes, a scan pass and then the
//! scrambled family's, the inserts of D and E going above every loaded key;
//! the store E meets has a tenth of its keys unsealed and spread through it.
//! This builds that store and runs the scrambled E inside `e_phase`, so
//!
//! ```text
//! valgrind --tool=callgrind --collect-atstart=no \
//!   --toggle-collect='e_scrambled::e_phase' \
//!   target/release/examples/e_scrambled 30000 supdb-ingest
//! ```
//!
//! counts the writer's E alone, its commits under `Db::commit` beside its
//! scans. The mixes' loop is the suite's in the parts that shape the store
//! (`bench/src/run.rs`): the order, the batches of a hundred, the scrambled
//! family's key space and its inserts above the store.
use supdb_bench::engines::{self, Batch, Engine};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

const YCSB_SCAN: usize = 50;
const YCSB_BATCH: usize = 100;
type Mix = (char, u32, u32, u32, u32, u32, KeyDist);
const YCSB: [Mix; 6] = [
    ('A', 50, 50, 0, 0, 0, KeyDist::Zipfian),
    ('B', 95, 5, 0, 0, 0, KeyDist::Zipfian),
    ('C', 100, 0, 0, 0, 0, KeyDist::Zipfian),
    ('D', 95, 0, 5, 0, 0, KeyDist::Uniform),
    ('E', 0, 0, 5, 95, 0, KeyDist::Zipfian),
    ('F', 50, 0, 0, 0, 50, KeyDist::Zipfian),
];
const YCSBS: [Mix; 6] = [
    ('A', 50, 50, 0, 0, 0, KeyDist::Scrambled),
    ('B', 95, 5, 0, 0, 0, KeyDist::Scrambled),
    ('C', 100, 0, 0, 0, 0, KeyDist::Scrambled),
    ('D', 95, 0, 5, 0, 0, KeyDist::Latest),
    ('E', 0, 0, 5, 95, 0, KeyDist::Scrambled),
    ('F', 50, 0, 0, 0, 50, KeyDist::Scrambled),
];
const ORDER: [char; 6] = ['C', 'B', 'A', 'F', 'D', 'E'];

fn mix(e: &mut dyn Engine, w: &Mix, size: u64, salt: u64, inserted: &mut u64, payload: &Payload) {
    let (letter, pread, pupd, pins, pscan, prmw, dist) = *w;
    let ops = (size / 6).max(1_000);
    let keyspace = match dist {
        KeyDist::Latest => size + *inserted,
        KeyDist::Scrambled => size + *inserted + 2 * (ops * pins as u64 / 100),
        _ => size,
    };
    let mut keys = KeyGen::new(dist, keyspace, 0x9C5B ^ letter as u64 ^ salt);
    let mut pick = Rng::new(0x5EED ^ letter as u64 ^ salt);
    let mut vrng = Rng::new(0xE2);
    let mut kb = [0u8; 16];
    let mut wbuf = Batch::with_capacity(YCSB_BATCH, payload.value_size());
    let mut is_insert = false;
    let committed = size + *inserted;
    let draw = |keys: &mut KeyGen| loop {
        let k = keys.next();
        if k < committed {
            return k;
        }
    };
    for _ in 0..ops {
        let roll = pick.below(100) as u32;
        if roll < pread {
            db_key_into(draw(&mut keys), &mut kb);
            e.get(&kb).unwrap();
        } else if roll < pread + pupd {
            db_key_into(draw(&mut keys), &mut kb);
            wbuf.push(&kb, payload.get(&mut vrng));
            is_insert = false;
        } else if roll < pread + pupd + pins {
            db_key_into(size + *inserted, &mut kb);
            *inserted += 1;
            wbuf.push(&kb, payload.get(&mut vrng));
            is_insert = true;
        } else if roll < pread + pupd + pins + pscan {
            db_key_into(draw(&mut keys), &mut kb);
            e.range(&kb, YCSB_SCAN).unwrap();
        } else if prmw > 0 {
            db_key_into(draw(&mut keys), &mut kb);
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
}

/// The scrambled E, alone in a function callgrind can toggle on. The
/// constant keeps the linker from folding it with anything.
#[inline(never)]
fn e_phase(e: &mut dyn Engine, size: u64, inserted: &mut u64, payload: &Payload) -> u64 {
    let w = YCSBS.iter().find(|w| w.0 == 'E').unwrap();
    mix(e, w, size, 0x5300, inserted, payload);
    std::hint::black_box(0xE5C2_u64)
}

/// One scan of one entry, alone in a function a profile can name.
#[inline(never)]
fn scan_one(e: &mut dyn Engine, kb: &[u8; 16]) -> u64 {
    e.range(kb, 1).unwrap();
    std::hint::black_box(0x5C01_u64)
}

fn main() {
    let mut a = std::env::args().skip(1);
    let size: u64 = a.next().unwrap().parse().unwrap();
    let arm = a.next().unwrap();
    let dir = std::env::temp_dir().join(format!("supdb-escr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let payload = Payload::new(100, 0.5, 0xE1);
    let mut e = engines::open(&arm, &dir, 3).unwrap();
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
    for l in ORDER {
        mix(
            e.as_mut(),
            YCSB.iter().find(|w| w.0 == l).unwrap(),
            size,
            0,
            &mut inserted,
            &payload,
        );
    }
    // The scan pass over the store the mixes left, as `scan-mixed` makes it.
    let mut r = Rng::new(0x5CA2);
    for _ in 0..(size / 100).max(100) {
        db_key_into(r.below(size), &mut kb);
        e.range(&kb, 100).unwrap();
    }
    for l in ORDER.iter().filter(|&&l| l != 'E') {
        let w = YCSBS.iter().find(|w| w.0 == *l).unwrap();
        mix(e.as_mut(), w, size, 0x5300, &mut inserted, &payload);
    }
    // `scans`: the store E meets, read by scans of one, ten and fifty
    // from the scrambled distribution's keys, interleaved by round, for
    // a scan's fixed start against its cost an entry; nothing written.
    let mode = a.next().unwrap_or_default();
    if mode == "scans1" {
        // Scans of one entry alone, for a profile of a scan's start.
        let keyspace = size + inserted;
        let mut keys = KeyGen::new(KeyDist::Scrambled, keyspace, 0x5CA9);
        let t = std::time::Instant::now();
        let n: usize = a.next().map_or(400_000, |v| v.parse().unwrap());
        let c0 = e.counters();
        for _ in 0..n {
            let k = loop {
                let k = keys.next();
                if k < keyspace {
                    break k;
                }
            };
            db_key_into(k, &mut kb);
            std::hint::black_box(scan_one(e.as_mut(), &kb));
        }
        println!(
            "{arm} {size}: {n} scans of 1 at {:.3} us",
            t.elapsed().as_secs_f64() * 1e6 / n as f64
        );
        let c1 = e.counters();
        let get =
            |c: &[(&str, f64)], k: &str| c.iter().find(|(n, _)| *n == k).map_or(0.0, |(_, v)| *v);
        let d = |k: &str| get(&c1, k) - get(&c0, k);
        println!(
            "  walks copy {} sparse {} other {} starts at rank {} search {}",
            d("walk_copy"),
            d("walk_sparse"),
            d("walk_other"),
            d("start_at_rank"),
            d("start_search")
        );
        return;
    }
    if mode == "scans" {
        let keyspace = size + inserted;
        let mut keys = KeyGen::new(KeyDist::Scrambled, keyspace, 0x5CA9);
        let n = 4000usize;
        let mut best = [f64::MAX; 3];
        let mut all: [Vec<f64>; 3] = Default::default();
        for _round in 0..7 {
            for (j, len) in [1usize, 10, 50].into_iter().enumerate() {
                let t = std::time::Instant::now();
                for _ in 0..n {
                    let k = loop {
                        let k = keys.next();
                        if k < keyspace {
                            break k;
                        }
                    };
                    db_key_into(k, &mut kb);
                    e.range(&kb, len).unwrap();
                }
                let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
                best[j] = best[j].min(us);
                all[j].push(us);
            }
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let m: Vec<f64> = all.iter_mut().map(med).collect();
        println!(
            "{arm} {size}: scan of 1 {:.3} us, of 10 {:.3}, of 50 {:.3} (median of 7 rounds); per entry past the first {:.1} ns",
            m[0], m[1], m[2], (m[2] - m[0]) * 1e3 / 49.0
        );
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    // The phase repeated where a third argument asks, for a profile's
    // samples: each round inserts its share past the store's end again.
    let rounds: usize = mode.parse().unwrap_or(1);
    eprintln!("e_phase start");
    let t = std::time::Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(e_phase(e.as_mut(), size, &mut inserted, &payload));
    }
    println!(
        "{arm} {size}: scrambled E x{rounds} in {:.3} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    drop(e);
    let _ = std::fs::remove_dir_all(&dir);
}
