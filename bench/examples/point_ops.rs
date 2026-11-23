//! Point gets and puts on one arm with the harness out of the loop: the
//! suite's ordered load and its sync, then the phases a mix is made of,
//! each over keys and values drawn before its clock starts, so a phase's
//! time and its instruction count (callgrind, toggled on the phase's
//! function) are the engine's and the loop's alone.
//!
//!   point_ops ARM [SIZE] [REPS] [DIST] [ROUNDS]
//!
//! The phases, each `OPS` operations (a sixth of the keys, the mixes'
//! count, and at least a thousand), in the order a pass runs them:
//!
//!   read-pass     as many uniform gets as keys, the suite's read pass,
//!                 which is also what faults the store in for the rest
//!   reads-clean   gets over the store as loaded
//!   updates       puts of keys that exist, a hundred a commit
//!   reads-dirty   gets over the store the updates left
//!   half          a get or a put by turns, the puts a hundred a commit
//!
//! The two read phases over a mix's keys go over them `ROUNDS` times (6
//! where not given), since a sixth of a small store's keys is read in a
//! millisecond or two and a rate over that is the host's; 1 for an
//! instruction count, which a second round only repeats.
//!
//! `DIST` is `uniform` (the default), `zipf` (YCSB's scrambled Zipfian) or
//! `zipf-plain` (the Zipfian unscrambled, the first mix family's). A get's
//! key never depends on the get before it, as the suite's do not.
use std::time::Instant;
use supdb_bench::engines::{self, Engine};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

const BATCH: usize = 100;

fn keys(dist: KeyDist, size: u64, seed: u64, n: usize) -> Vec<[u8; 16]> {
    let mut g = KeyGen::new(dist, size, seed);
    (0..n)
        .map(|_| {
            let mut kb = [0u8; 16];
            db_key_into(g.next(), &mut kb);
            kb
        })
        .collect()
}

#[inline(always)]
fn reads(e: &mut dyn Engine, ks: &[[u8; 16]], rounds: usize) -> usize {
    let mut n = 0;
    for _ in 0..rounds {
        for k in ks {
            n += e.get(k).unwrap();
        }
    }
    n
}

// One function a phase, so that callgrind's inclusive rows name them apart;
// each holds a constant of its own, or the linker folds the three into one.
#[inline(never)]
fn phase_read_pass(e: &mut dyn Engine, ks: &[[u8; 16]]) -> usize {
    reads(e, ks, 1) + std::hint::black_box(1) - 1
}
#[inline(never)]
fn phase_reads_clean(e: &mut dyn Engine, ks: &[[u8; 16]], rounds: usize) -> usize {
    reads(e, ks, rounds) + std::hint::black_box(2) - 2
}
#[inline(never)]
fn phase_reads_dirty(e: &mut dyn Engine, ks: &[[u8; 16]], rounds: usize) -> usize {
    reads(e, ks, rounds) + std::hint::black_box(3) - 3
}

#[inline(never)]
fn phase_updates(e: &mut dyn Engine, ks: &[[u8; 16]], vals: &[Vec<u8>]) {
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(BATCH);
    for (chunk, vs) in ks.chunks(BATCH).zip(vals.chunks(BATCH)) {
        pairs.clear();
        for (k, v) in chunk.iter().zip(vs) {
            pairs.push((k.as_slice(), v.as_slice()));
        }
        e.update_batch(&pairs).unwrap();
    }
}

#[inline(never)]
fn phase_half(e: &mut dyn Engine, ks: &[[u8; 16]], vals: &[Vec<u8>]) -> usize {
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(BATCH);
    let mut n = 0;
    for (i, k) in ks.iter().enumerate() {
        if i % 2 == 0 {
            n += e.get(k).unwrap();
        } else {
            pairs.push((k.as_slice(), vals[i / 2].as_slice()));
            if pairs.len() == BATCH {
                e.update_batch(&pairs).unwrap();
                pairs.clear();
            }
        }
    }
    if !pairs.is_empty() {
        e.update_batch(&pairs).unwrap();
    }
    n
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).cloned().unwrap_or("supdb-ingest".into());
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let reps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let dist = match args.get(4).map(String::as_str) {
        Some("zipf") => KeyDist::Scrambled,
        Some("zipf-plain") => KeyDist::Zipfian,
        _ => KeyDist::Uniform,
    };
    let rounds: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(6);
    let ops = ((size / 6).max(1_000)) as usize;
    let payload = Payload::new(100, 0.5, 0xE1);
    let root = std::env::temp_dir().join(format!("supdb-pointops-{}", std::process::id()));
    for rep in 0..reps {
        let dir = root.join(format!("r{rep}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        engines::set_store_keys(size);
        let mut e = engines::open(&arm, &dir, 1).unwrap();
        let mut vrng = Rng::new(0xE1);
        let mut kb = [0u8; 16];
        let mut buf = engines::Batch::with_capacity(1000, payload.value_size());
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        e.sync().unwrap();
        e.settle().unwrap();

        let k0 = keys(KeyDist::Uniform, size, 7, size as usize);
        let k1 = keys(dist, size, 0x11, ops);
        let k2 = keys(dist, size, 0x22, ops);
        let k3 = keys(dist, size, 0x33, ops);
        let k4 = keys(dist, size, 0x44, ops);
        let vals: Vec<Vec<u8>> = (0..ops).map(|_| payload.get(&mut vrng).to_vec()).collect();

        let ns = |t: Instant, n: usize| t.elapsed().as_nanos() as f64 / n as f64;
        let t = Instant::now();
        let b0 = phase_read_pass(e.as_mut(), &k0);
        let r0 = ns(t, k0.len());
        let t = Instant::now();
        let b1 = phase_reads_clean(e.as_mut(), &k1, rounds);
        let r1 = ns(t, ops * rounds);
        let t = Instant::now();
        phase_updates(e.as_mut(), &k2, &vals);
        let w = ns(t, ops);
        let shape = |e: &dyn Engine| {
            let c = e.counters();
            let g = |n: &str| c.iter().find(|(m, _)| *m == n).map_or(0.0, |x| x.1);
            format!(
                "parts {:.0} pieces {:.0} unsealed {:.0}",
                g("partitions"),
                g("pieces"),
                g("unsealed_keys")
            )
        };
        // The store's shape between the phases, on request: a counter read
        // takes the writer's upkeep home, so a run that prints it is not
        // one whose rates are read.
        let shapes = std::env::var_os("POINT_OPS_SHAPE").is_some();
        if shapes {
            eprintln!("shape after updates: {}", shape(e.as_ref()));
        }
        let t = Instant::now();
        let b2 = phase_reads_dirty(e.as_mut(), &k3, rounds);
        let r2 = ns(t, ops * rounds);
        let t = Instant::now();
        let b3 = phase_half(e.as_mut(), &k4, &vals);
        let h = ns(t, ops);
        if shapes {
            eprintln!("shape after half: {}", shape(e.as_ref()));
        }
        println!(
            "rep {rep} {arm} {size} ops {ops}: read-pass {r0:.0} ns  reads-clean {r1:.0} ns  updates {w:.0} ns  reads-dirty {r2:.0} ns  half {h:.0} ns  (bytes {b0} {b1} {b2} {b3})"
        );
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
    let _ = std::fs::remove_dir_all(&root);
}
