//! The threaded mixes' three operations on one thread writer, with the
//! harness out of the loop: an arm's ordered load and its sync, every key
//! put once through the writer, so that a store whose live table every
//! handle writes holds them all there (`Options::shared_writers`), and then
//! a phase of each operation over keys and values drawn before its clock
//! starts. A phase's time and its instruction count (callgrind, toggled on
//! the phase's function) are the engine's and the loop's alone.
//!
//!   shared_ops ARMS [SIZE] [ROUNDS] [DIST]
//!
//! `ARMS` is a comma-separated list, each arm a store of its own in the one
//! process, the arms run by turns phase by phase, the order turned a place
//! each round, as `bench ab` alternates its arms, since on this host a
//! process's memory reads at rates a factor apart from the next one's.
//! The phases, each over a sixth of the keys (the mixes' count) and run
//! `ROUNDS` times (3 where not given; 1 for an instruction count, which a
//! second round only repeats):
//!
//!   gets     `ThreadWriter::get`
//!   puts     `ThreadWriter::update_batch`, a hundred a batch
//!   rmws     `ThreadWriter::rmw`, or a get and a put where the arm has none
//!
//! `DIST` is `uniform` (the default) or `zipf` (YCSB's scrambled Zipfian).
//! Each phase prints its median nanoseconds an operation over the rounds.
//! `SHARED_OPS_ONLY=gets` (or `puts`, `rmws`, `half`) runs that phase alone,
//! for a profile of it. `SHARED_OPS_THREADS=N` runs `half` -- a get or a
//! put by turns, the threaded mixes' A -- and `rmws` on N threads at once,
//! each through a writer of its own over keys of its own, timed from the
//! first release to the last finish, as the threaded mixes are.
//! `SHARED_OPS_FRESH=1` skips the put of every key, so the phases meet
//! the store as its load and sync left it, every key's first touch among
//! them; with one round, the threaded phases are a key's first touches as
//! the threaded mixes' first cells meet them.
use std::time::Instant;
use supdb_bench::engines::{self, Engine, ThreadWriter};
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

// One function a phase, so that callgrind's inclusive rows name them apart;
// each holds a constant of its own, or the linker folds them into one.
#[inline(never)]
fn phase_gets(w: &mut dyn ThreadWriter, ks: &[[u8; 16]]) -> usize {
    let mut n = 0x6E75;
    for k in ks {
        n += w.get(k).unwrap();
    }
    n
}

#[inline(never)]
fn phase_puts(w: &mut dyn ThreadWriter, ks: &[[u8; 16]], vals: &[Vec<u8>]) -> usize {
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(BATCH);
    for (k, v) in ks.iter().zip(vals) {
        pairs.push((k.as_slice(), v.as_slice()));
        if pairs.len() == BATCH {
            w.update_batch(&pairs).unwrap();
            pairs.clear();
        }
    }
    if !pairs.is_empty() {
        w.update_batch(&pairs).unwrap();
    }
    0x7075
}

#[inline(never)]
fn phase_rmws(w: &mut dyn ThreadWriter, ks: &[[u8; 16]], vals: &[Vec<u8>]) -> usize {
    let mut n = 0x726D;
    for (k, v) in ks.iter().zip(vals) {
        match w.rmw(k, v) {
            Some(read) => n += read.unwrap(),
            None => {
                n += w.get(k).unwrap();
                w.update_batch(&[(k.as_slice(), v.as_slice())]).unwrap();
            }
        }
    }
    n
}

/// Half gets and half puts by turns, the puts a hundred a batch: the
/// threaded mixes' A.
#[inline(never)]
fn phase_half(w: &mut dyn ThreadWriter, ks: &[[u8; 16]], vals: &[Vec<u8>]) -> usize {
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(BATCH);
    let mut n = 0x6861;
    for (i, (k, v)) in ks.iter().zip(vals).enumerate() {
        if i % 2 == 0 {
            n += w.get(k).unwrap();
        } else {
            pairs.push((k.as_slice(), v.as_slice()));
            if pairs.len() == BATCH {
                w.update_batch(&pairs).unwrap();
                pairs.clear();
            }
        }
    }
    if !pairs.is_empty() {
        w.update_batch(&pairs).unwrap();
    }
    n
}

/// A phase of operations through a writer over keys and values drawn ahead.
type Phase = fn(&mut dyn ThreadWriter, &[[u8; 16]], &[Vec<u8>]) -> usize;

/// `phase` on `threads` threads at once, each through a writer of its own
/// opened on its thread and over keys of its own, released together: the
/// span from the first release to the last finish, in nanoseconds an
/// operation over every thread's.
fn threaded(
    e: &mut dyn Engine,
    threads: usize,
    ks: &[Vec<[u8; 16]>],
    vals: &[Vec<u8>],
    phase: Phase,
) -> f64 {
    let openers = e.thread_writers(threads).unwrap();
    let go = std::sync::Barrier::new(threads);
    mark("start");
    let spans: Vec<(Instant, Instant)> = std::thread::scope(|s| {
        let hs: Vec<_> = openers
            .into_iter()
            .zip(ks)
            .map(|(open, ks)| {
                let go = &go;
                s.spawn(move || {
                    let mut w = open().unwrap();
                    go.wait();
                    let t0 = Instant::now();
                    std::hint::black_box(phase(w.as_mut(), ks, vals));
                    (t0, Instant::now())
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    mark("end");
    let first = spans.iter().map(|s| s.0).min().unwrap();
    let last = spans.iter().map(|s| s.1).max().unwrap();
    let n: usize = ks.iter().map(Vec::len).sum();
    last.duration_since(first).as_nanos() as f64 / n as f64
}

/// The monotonic clock at a threaded phase's edge, on stderr where
/// `SHARED_OPS_MARK` is set, for a profile taken under `perf record -k
/// CLOCK_MONOTONIC` to be cut to the phases.
fn mark(edge: &str) {
    if std::env::var_os("SHARED_OPS_MARK").is_none() {
        return;
    }
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a clock read into a timespec on the stack.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    eprintln!("sharedmark {edge} {}.{:09}", ts.tv_sec, ts.tv_nsec);
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arms: Vec<String> = args
        .get(1)
        .map_or("supdb-heldsharedbig,faster", |s| s.as_str())
        .split(',')
        .map(str::to_string)
        .collect();
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300_000);
    let rounds: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let dist = match args.get(4).map(String::as_str) {
        Some("zipf") => KeyDist::Scrambled,
        _ => KeyDist::Uniform,
    };
    let ops = ((size / 6).max(1_000)) as usize;
    let payload = Payload::new(100, 0.5, 0xE1);
    let root = std::env::temp_dir().join(format!("supdb-sharedops-{}", std::process::id()));
    engines::set_store_keys(size);
    engines::set_store_bound(None);
    let mut es: Vec<Box<dyn Engine>> = Vec::new();
    for arm in &arms {
        let dir = root.join(arm);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut e: Box<dyn Engine> = engines::open(arm, &dir, 1).unwrap();
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
        es.push(e);
    }
    let ks: Vec<Vec<[u8; 16]>> = (0..3).map(|p| keys(dist, size, 0x51 + p, ops)).collect();
    let mut vrng = Rng::new(0xE2);
    let vals: Vec<Vec<u8>> = (0..ops).map(|_| payload.get(&mut vrng).to_vec()).collect();
    // One phase alone, on request (`SHARED_OPS_ONLY=gets|puts|rmws`), for a
    // profile of it.
    let only = std::env::var("SHARED_OPS_ONLY").ok();
    let on = |p: &str| only.as_deref().is_none_or(|o| o == p);
    let mut ws: Vec<Box<dyn ThreadWriter + '_>> = es
        .iter_mut()
        .map(|e| {
            let open = e.thread_writers(1).unwrap().pop().unwrap();
            let mut w = open().unwrap();
            // Every key put once through the writer, its value of the
            // length the phases write; left as the load and its sync left
            // them where `SHARED_OPS_FRESH` is set, for what a key's first
            // touch costs.
            if std::env::var_os("SHARED_OPS_FRESH").is_none() {
                let all = keys(KeyDist::Sequential, size, 0, size as usize);
                let v = vec![0x5Au8; payload.value_size()];
                for chunk in all.chunks(BATCH) {
                    let pairs: Vec<(&[u8], &[u8])> =
                        chunk.iter().map(|k| (k.as_slice(), v.as_slice())).collect();
                    w.update_batch(&pairs).unwrap();
                }
            }
            w
        })
        .collect();
    // On several threads at once, on request (`SHARED_OPS_THREADS=N`):
    // the A and RMW phases, each thread through a writer of its own over
    // keys of its own, the arms by turns as below.
    if let Some(n) = std::env::var("SHARED_OPS_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        drop(ws);
        let tks: Vec<Vec<[u8; 16]>> = (0..n)
            .map(|i| keys(dist, size, 0x91 + i as u64, ops))
            .collect();
        let mut t: Vec<[Vec<f64>; 4]> = arms.iter().map(|_| Default::default()).collect();
        let gets = |w: &mut dyn ThreadWriter, ks: &[[u8; 16]], _: &[Vec<u8>]| phase_gets(w, ks);
        for round in 0..rounds {
            for j in 0..arms.len() {
                let a = (round + j) % arms.len();
                if on("half") {
                    t[a][0].push(threaded(es[a].as_mut(), n, &tks, &vals, phase_half));
                }
                if on("rmws") {
                    t[a][1].push(threaded(es[a].as_mut(), n, &tks, &vals, phase_rmws));
                }
                if only.as_deref() == Some("gets") {
                    t[a][2].push(threaded(es[a].as_mut(), n, &tks, &vals, gets));
                }
                if only.as_deref() == Some("puts") {
                    t[a][3].push(threaded(es[a].as_mut(), n, &tks, &vals, phase_puts));
                }
            }
        }
        for (arm, [h, r, g, p]) in arms.iter().zip(t) {
            let m = |v: Vec<f64>| if v.is_empty() { 0.0 } else { median(v) };
            println!(
                "shared_ops {arm} size {size} ops {ops} threads {n} rounds {rounds} half {:.1} rmws {:.1} gets {:.1} puts {:.1} ns",
                m(h),
                m(r),
                m(g),
                m(p)
            );
        }
        drop(es);
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    // The arms by turns, the order turned a place each round, so that what
    // the host does to a moment is every arm's.
    let mut t: Vec<[Vec<f64>; 3]> = arms.iter().map(|_| Default::default()).collect();
    let mut sink = 0usize;
    for round in 0..rounds {
        for j in 0..arms.len() {
            let a = (round + j) % arms.len();
            let w = ws[a].as_mut();
            if on("gets") {
                let s = Instant::now();
                sink += phase_gets(w, &ks[0]);
                t[a][0].push(s.elapsed().as_nanos() as f64 / ops as f64);
            }
            if on("puts") {
                let s = Instant::now();
                sink += phase_puts(w, &ks[1], &vals);
                t[a][1].push(s.elapsed().as_nanos() as f64 / ops as f64);
            }
            if on("rmws") {
                let s = Instant::now();
                sink += phase_rmws(w, &ks[2], &vals);
                t[a][2].push(s.elapsed().as_nanos() as f64 / ops as f64);
            }
        }
    }
    std::hint::black_box(sink);
    drop(ws);
    for (arm, [g, p, r]) in arms.iter().zip(t) {
        let m = |v: Vec<f64>| if v.is_empty() { 0.0 } else { median(v) };
        println!(
            "shared_ops {arm} size {size} ops {ops} rounds {rounds} gets {:.1} puts {:.1} rmws {:.1} ns",
            m(g),
            m(p),
            m(r)
        );
    }
    drop(es);
    let _ = std::fs::remove_dir_all(&root);
}
