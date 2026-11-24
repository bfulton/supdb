//! Point gets and puts on one arm with the harness out of the loop: the
//! suite's ordered load and its sync, then the phases a mix is made of,
//! each over keys and values drawn before its clock starts, so a phase's
//! time and its instruction count (callgrind, toggled on the phase's
//! function) are the engine's and the loop's alone.
//!
//!   point_ops ARM [SIZE] [REPS] [DIST] [ROUNDS]
//!
//! `ARM` is any of the suite's, or `proto`: the probe's own table of records
//! written in place (`supdb_bench::proto`), driven as the arms are.
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
//!
//! The updates cross the engine's seal cap, a tenth of the store, so the
//! reads after them are of a store with a seal in flight and then a piece:
//! natively the seal lands a few milliseconds into `reads-dirty`, and under
//! valgrind, whose seal thread runs fifty times slow, it never does, so a
//! count taken there is of a frozen table where the native reads are over
//! a piece. From the environment:
//!
//!   POINT_OPS_LAND=1     the seal joined before `reads-dirty`: the store
//!                        with its piece, for a count or a profile of it
//!   POINT_OPS_SHAPE=1    the store's shape and the reads by the path that
//!                        answered them, after each phase, on stderr; a
//!                        counter read takes the writer's upkeep home, so
//!                        the run's rates are not read
//!   POINT_OPS_ROUNDS=1   `reads-dirty` timed a round at a time, for a
//!                        cost its first round pays once
//!   POINT_OPS_CLASSES=1  after `reads-dirty`, gets of the keys the live
//!                        table answers, of the keys the piece answers
//!                        (with `POINT_OPS_LAND`) and of the keys no update
//!                        wrote, each class read over and over: what a get
//!                        of each costs warm and with every branch learned
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
        // `proto` is the probe's own table (`supdb_bench::proto`), driven as
        // the suite's arms are.
        let mut e: Box<dyn Engine> = if arm == "proto" {
            Box::new(supdb_bench::proto::Hot::create(size))
        } else {
            engines::open(&arm, &dir, 1).unwrap()
        };
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
        // The writer's minor faults by phase, on request
        // (`POINT_OPS_FAULTS=1`): the writer thread's own, read from
        // `/proc/thread-self/stat` around each phase.
        let faults = std::env::var_os("POINT_OPS_FAULTS").is_some();
        let f0 = supdb_bench::run::minflt_self();
        let t = Instant::now();
        phase_updates(e.as_mut(), &k2, &vals);
        let w = ns(t, ops);
        if faults {
            eprintln!(
                "faults: updates {} on the writer, {:.3} a write",
                supdb_bench::run::minflt_self() - f0,
                (supdb_bench::run::minflt_self() - f0) as f64 / ops as f64
            );
        }
        let shape = |e: &dyn Engine| {
            let c = e.counters();
            let g = |n: &str| c.iter().find(|(m, _)| *m == n).map_or(0.0, |x| x.1);
            format!(
                "parts {:.0} pieces {:.0} unsealed {:.0} seals {:.0} idle {:.0} reads lone {:.0} layers {:.0} placed {:.0} pool takes {:.0} gives {:.0}",
                g("partitions"),
                g("pieces"),
                g("unsealed_keys"),
                g("seals"),
                g("seals_idle"),
                g("reads_lone"),
                g("reads_layers"),
                g("reads_placed"),
                g("pool_takes"),
                g("pool_gives")
            )
        };
        // The store's shape between the phases, on request: a counter read
        // takes the writer's upkeep home, so a run that prints it is not
        // one whose rates are read.
        let shapes = std::env::var_os("POINT_OPS_SHAPE").is_some();
        // A seal the updates began landed before the reads, on request
        // (`POINT_OPS_LAND=1`): natively it lands a few milliseconds into
        // them, and under valgrind, whose seal thread is fifty times slow,
        // never, so a count of the reads over the piece asks for it.
        if std::env::var_os("POINT_OPS_LAND").is_some() {
            e.settle().unwrap();
        }
        if shapes {
            eprintln!("shape after updates: {}", shape(e.as_ref()));
        }
        // The dirty reads a round at a time, on request
        // (`POINT_OPS_ROUNDS=1`): a phase slower than its gets is read for
        // a cost its first round pays once before its rate is the gets'.
        let by_round = std::env::var_os("POINT_OPS_ROUNDS").is_some();
        let t = Instant::now();
        let b2 = if by_round {
            let mut b = 0;
            let mut line = String::from("dirty rounds:");
            for _ in 0..rounds {
                let t = Instant::now();
                b += phase_reads_dirty(e.as_mut(), &k3, 1);
                line.push_str(&format!(" {:.0}", ns(t, ops)));
            }
            eprintln!("{line} ns a get");
            b
        } else {
            phase_reads_dirty(e.as_mut(), &k3, rounds)
        };
        let r2 = ns(t, ops * rounds);
        if shapes {
            eprintln!("shape after reads-dirty: {}", shape(e.as_ref()));
        }
        // The dirty reads by where their key is answered, on request
        // (`POINT_OPS_CLASSES=1`): keys the last updates wrote, which the
        // live table holds; keys only the first updates wrote, which the
        // piece holds where a seal has landed (`POINT_OPS_LAND`); and keys
        // no update wrote, the partition's. Each class read over and over,
        // its time a get and how many keys it is.
        if std::env::var_os("POINT_OPS_CLASSES").is_some() {
            let tail = k2.len() / 8;
            let late: std::collections::HashSet<[u8; 16]> =
                k2[k2.len() - tail..].iter().copied().collect();
            let all: std::collections::HashSet<[u8; 16]> = k2.iter().copied().collect();
            let early: std::collections::HashSet<[u8; 16]> = k2[..k2.len() / 4]
                .iter()
                .copied()
                .filter(|k| !k2[k2.len() / 4..].contains(k))
                .collect();
            let untouched: Vec<[u8; 16]> = k0
                .iter()
                .copied()
                .filter(|k| !all.contains(k))
                .take(4096)
                .collect();
            let mut classes: Vec<(&str, Vec<[u8; 16]>)> = vec![
                ("live", late.into_iter().collect()),
                ("piece", early.into_iter().collect()),
                ("partition", untouched),
            ];
            for (_, ks) in classes.iter_mut() {
                ks.sort_unstable();
                // A fixed shuffle, so the reads are in no key order.
                let mut r = Rng::new(0xC1A5);
                for i in (1..ks.len()).rev() {
                    ks.swap(i, (r.next() % (i as u64 + 1)) as usize);
                }
            }
            for round in 0..3 {
                for (name, ks) in &classes {
                    let reps = (200_000 / ks.len().max(1)).max(1);
                    let t = Instant::now();
                    let b = reads(e.as_mut(), ks, reps);
                    eprintln!(
                        "class {name} round {round}: {} keys, {:.0} ns a get ({b} bytes)",
                        ks.len(),
                        ns(t, ks.len() * reps)
                    );
                }
            }
            if shapes {
                eprintln!("shape after the classes: {}", shape(e.as_ref()));
            }
        }
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
