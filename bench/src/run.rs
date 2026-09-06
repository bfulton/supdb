//! One run: the floors, then the ladder, times the arms, times the reps,
//! into a row.
//!
//! Every arm in one process, interleaved one round at a time, so a machine
//! that drifts drifts across all of them rather than into one. A rep is one
//! complete pass of every workload for one arm; rep zero is a warmup and is
//! discarded, because the first touch of a fresh file pays for allocation
//! and first-fault costs that no steady state repeats.

use crate::engines::{self, Batch, Engine};
use crate::env::IoCounters;
use crate::hist::Hist;
use crate::row::{Guarantee, MachineInfo, Measurement, Row};
use crate::workload::{db_key_into, KeyDist, KeyGen, Payload, Permutation, Rng};
use crate::{ladder, Scale};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

/// Every record the suite writes is this key and this value, so a figure
/// can turn a byte rate into an entry rate and a key count into bytes.
pub const KEY_SIZE: usize = 16;
pub const VALUE_SIZE: usize = 100;

/// The arm name the floors are recorded under: no engine ran.
pub const FLOOR_ARM: &str = "floor";

pub struct Plan {
    pub scale: Scale,
    pub arms: Vec<String>,
    /// The ladder's top rung, in keys. The ladder rounds up to a rung.
    pub top: u64,
    pub reps: usize,
    pub value_size: usize,
    pub batch: usize,
    pub scan_len: usize,
}

impl Plan {
    pub fn new(scale: Scale, arms: Vec<String>, top: u64) -> Plan {
        Plan {
            scale,
            arms,
            top,
            reps: scale.reps(),
            value_size: VALUE_SIZE,
            batch: 1_000,
            scan_len: 100,
        }
    }
}

/// YCSB's core workloads: (letter, read %, update %, insert %, scan %,
/// read-modify-write %, key distribution). Theta 0.99 for the Zipfian, as
/// in the original. D's reads are uniform over the loaded keys rather than
/// skewed to the latest inserts: the latest distribution needs a Zipfian
/// over a count that grows with every insert, and the cost of tracking
/// that is not a cost the engines should be charged for.
const YCSB: [(char, u32, u32, u32, u32, u32, KeyDist); 6] = [
    ('A', 50, 50, 0, 0, 0, KeyDist::Zipfian),
    ('B', 95, 5, 0, 0, 0, KeyDist::Zipfian),
    ('C', 100, 0, 0, 0, 0, KeyDist::Zipfian),
    ('D', 95, 0, 5, 0, 0, KeyDist::Uniform),
    ('E', 0, 0, 5, 95, 0, KeyDist::Zipfian),
    ('F', 50, 0, 0, 0, 50, KeyDist::Zipfian),
];
/// The order the six run in on one store: the read-only mix first, on the
/// store as loaded, and the two that insert last, so no earlier workload
/// reads a store another has grown.
const YCSB_ORDER: [char; 6] = ['C', 'B', 'A', 'F', 'D', 'E'];
const YCSB_BATCH: usize = 100;
const YCSB_SCAN: usize = 50;

/// A workload's operation count at a rung: a sixth of the keys, so the six
/// mixes together cost about one read pass. At a third, `quick` took twice
/// the two minutes it is meant to take.
fn ycsb_ops(size: u64) -> u64 {
    (size / 6).max(1_000)
}

/// The scan floor's file: the top rung's bytes, capped, so `full` on a
/// large machine does not spend its disk on a file that is not a store.
const SCAN_FLOOR_CAP: u64 = 4 << 30;
const WAL_FLOOR_BATCHES: u64 = 100;

type Key = (String, Option<u64>, String, String); // workload, size, arm, quantity

struct Samples {
    map: BTreeMap<Key, (Guarantee, &'static str, Vec<f64>)>,
}

impl Samples {
    fn push(
        &mut self,
        workload: &str,
        size: Option<u64>,
        arm: &str,
        guarantee: Guarantee,
        (quantity, unit): (&str, &'static str),
        v: f64,
    ) {
        self.map
            .entry((
                workload.to_string(),
                size,
                arm.to_string(),
                quantity.to_string(),
            ))
            .or_insert_with(|| (guarantee, unit, Vec::new()))
            .2
            .push(v);
    }
}

pub fn run(plan: &Plan, machine: MachineInfo, log: &mut dyn FnMut(&str)) -> Result<Row, String> {
    let utc = crate::row::utc_now();
    let root = std::env::temp_dir().join(format!("supdb-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    engines::check_matched(&plan.arms, &root)?;

    let rungs = ladder(plan.top);
    let payload = Payload::new(plan.value_size, 0.5, 0xE1);
    let mut s = Samples {
        map: BTreeMap::new(),
    };
    let started = Instant::now();

    // The floors first: what the device does with no engine in the way.
    let floor_bytes =
        (*rungs.last().unwrap_or(&0) * (KEY_SIZE + plan.value_size) as u64).min(SCAN_FLOOR_CAP);
    for rep in 0..=plan.reps {
        let wal = wal_floor(&root, plan, &payload)?;
        let scan = scan_floor(&root, floor_bytes)?;
        if rep > 0 {
            s.push(
                "wal-floor",
                None,
                FLOOR_ARM,
                Guarantee::Durable,
                ("ops_per_s", "ops/s"),
                wal,
            );
            // No guarantee applies to a read of a file; the field is
            // required, and buffered is the one that claims nothing.
            s.push(
                "scan-floor",
                None,
                FLOOR_ARM,
                Guarantee::Buffered,
                ("bytes_per_s", "B/s"),
                scan,
            );
        }
        log(&format!(
            "{:>7}s  floors rep {rep}{}  wal {wal:>10.0} ops/s  mmap {:>8.2} GB/s",
            started.elapsed().as_secs(),
            if rep == 0 { " (warmup)" } else { "" },
            scan / 1e9,
        ));
    }

    for &size in &rungs {
        let map_gb = lmdb_map_gb(size, plan.value_size);
        for rep in 0..=plan.reps {
            for arm in &plan.arms {
                let g = engines::guarantee(arm).expect("checked at start");
                let dir = root.join(format!("{arm}-{size}-{rep}"));
                let one = one_pass(arm, &dir, size, map_gb, plan, &payload)?;
                let _ = std::fs::remove_dir_all(&dir);
                if rep > 0 {
                    let sz = Some(size);
                    s.push("load", sz, arm, g, ("ops_per_s", "ops/s"), one.load_ops_s);
                    s.push(
                        "load",
                        sz,
                        arm,
                        g,
                        ("device_bytes_per_byte", "B/B"),
                        one.load_bpb,
                    );
                    s.push(
                        "load-shuffled",
                        sz,
                        arm,
                        g,
                        ("ops_per_s", "ops/s"),
                        one.shuffled_ops_s,
                    );
                    s.push("read", sz, arm, g, ("reads_per_s", "reads/s"), one.reads_s);
                    s.push("read", sz, arm, g, ("p99_us", "µs"), one.p99_us);
                    s.push(
                        "scan",
                        sz,
                        arm,
                        g,
                        ("entries_per_s", "entries/s"),
                        one.scan_entries_s,
                    );
                    for (letter, ops_s) in &one.ycsb_ops_s {
                        s.push(
                            &format!("ycsb-{letter}"),
                            sz,
                            arm,
                            g,
                            ("ops_per_s", "ops/s"),
                            *ops_s,
                        );
                    }
                }
                log(&format!(
                    "{:>7}s  size {size:>9}  rep {rep}{}  {arm:<15} load {:>10.0} ops/s  read {:>10.0}/s  scan {:>11.0}/s  ycsb-A {:>9.0}/s",
                    started.elapsed().as_secs(),
                    if rep == 0 { " (warmup)" } else { "" },
                    one.load_ops_s,
                    one.reads_s,
                    one.scan_entries_s,
                    one.ycsb_ops_s.iter().find(|(l, _)| *l == 'A').map(|(_, v)| *v).unwrap_or(0.0),
                ));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);

    let measurements = s
        .map
        .into_iter()
        .map(
            |((workload, size, arm, quantity), (guarantee, unit, samples))| Measurement {
                workload,
                arm,
                guarantee,
                size,
                quantity,
                unit: unit.to_string(),
                samples,
            },
        )
        .collect();

    Ok(Row {
        utc,
        sha: option_env!("SUPDB_SHA").unwrap_or("unknown").to_string(),
        rustc: option_env!("SUPDB_RUSTC").unwrap_or("unknown").to_string(),
        scale: plan.scale,
        machine,
        measurements,
    })
}

struct OnePass {
    load_ops_s: f64,
    load_bpb: f64,
    shuffled_ops_s: f64,
    reads_s: f64,
    p99_us: f64,
    scan_entries_s: f64,
    ycsb_ops_s: Vec<(char, f64)>,
}

fn one_pass(
    arm: &str,
    dir: &Path,
    size: u64,
    map_gb: usize,
    plan: &Plan,
    payload: &Payload,
) -> Result<OnePass, String> {
    // Ordered load, then reads, scans and the YCSB mixes over it.
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut e = engines::open(arm, dir, map_gb)?;
    let (load_s, wrote) = load(e.as_mut(), size, plan, payload, |i| i)?;
    let stored = size as f64 * (KEY_SIZE + plan.value_size) as f64;

    let mut kb = [0u8; KEY_SIZE];
    let mut g = KeyGen::new(KeyDist::Uniform, size, 7);
    let mut h = Hist::new();
    let t = Instant::now();
    for _ in 0..size {
        db_key_into(g.next(), &mut kb);
        let t1 = Instant::now();
        e.get(&kb)?;
        h.record(t1.elapsed().as_nanos() as u64);
    }
    let read_s = t.elapsed().as_secs_f64();

    let scans = (size / plan.scan_len as u64).max(1);
    let mut g2 = KeyGen::new(
        KeyDist::Uniform,
        size.saturating_sub(plan.scan_len as u64).max(1),
        11,
    );
    let t = Instant::now();
    for _ in 0..scans {
        db_key_into(g2.next(), &mut kb);
        e.range(&kb, plan.scan_len)?;
    }
    let scan_s = t.elapsed().as_secs_f64();

    let mut ycsb_ops_s = Vec::with_capacity(YCSB.len());
    let mut inserted = 0u64;
    for letter in YCSB_ORDER {
        let w = YCSB.iter().find(|w| w.0 == letter).unwrap();
        let secs = ycsb(e.as_mut(), w, size, &mut inserted, payload)?;
        ycsb_ops_s.push((letter, ycsb_ops(size) as f64 / secs));
    }
    drop(e);
    let _ = std::fs::remove_dir_all(dir);

    // The same keys in a shuffled order, on a fresh store.
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut e = engines::open(arm, dir, map_gb)?;
    let perm = Permutation::new(size, 0x5EED);
    let (shuf_s, _) = load(e.as_mut(), size, plan, payload, |i| perm.at(i))?;
    drop(e);

    Ok(OnePass {
        load_ops_s: size as f64 / load_s,
        load_bpb: wrote as f64 / stored,
        shuffled_ops_s: size as f64 / shuf_s,
        reads_s: size as f64 / read_s,
        p99_us: h.percentile(99.0) as f64 / 1000.0,
        scan_entries_s: (scans * plan.scan_len as u64) as f64 / scan_s,
        ycsb_ops_s,
    })
}

/// One YCSB workload over a loaded store: `ycsb_ops(size)` operations,
/// writes batched by `YCSB_BATCH` and the tail flushed inside the timed
/// region, inserts taking fresh keys past the loaded range. Returns seconds.
fn ycsb(
    e: &mut dyn Engine,
    w: &(char, u32, u32, u32, u32, u32, KeyDist),
    size: u64,
    inserted: &mut u64,
    payload: &Payload,
) -> Result<f64, String> {
    let (letter, pread, pupd, pins, pscan, prmw, dist) = *w;
    let ops = ycsb_ops(size);
    let mut keys = KeyGen::new(dist, size, 0x9C5B ^ letter as u64);
    let mut pick = Rng::new(0x5EED ^ letter as u64);
    let mut vrng = Rng::new(0xE2);
    let mut kb = [0u8; KEY_SIZE];
    let mut wbuf = Batch::with_capacity(YCSB_BATCH, payload.value_size());
    let mut wbuf_is_insert = false;
    let t = Instant::now();
    for _ in 0..ops {
        let roll = pick.below(100) as u32;
        if roll < pread {
            db_key_into(keys.next(), &mut kb);
            e.get(&kb)?;
        } else if roll < pread + pupd {
            db_key_into(keys.next(), &mut kb);
            wbuf.push(&kb, payload.get(&mut vrng));
            wbuf_is_insert = false;
        } else if roll < pread + pupd + pins {
            db_key_into(size + *inserted, &mut kb);
            *inserted += 1;
            wbuf.push(&kb, payload.get(&mut vrng));
            wbuf_is_insert = true;
        } else if roll < pread + pupd + pins + pscan {
            db_key_into(keys.next(), &mut kb);
            e.range(&kb, YCSB_SCAN)?;
        } else if prmw > 0 {
            db_key_into(keys.next(), &mut kb);
            e.get(&kb)?;
            wbuf.push(&kb, payload.get(&mut vrng));
            wbuf_is_insert = false;
        }
        if wbuf.len() >= YCSB_BATCH {
            flush_ycsb(&mut wbuf, e, wbuf_is_insert)?;
        }
    }
    flush_ycsb(&mut wbuf, e, wbuf_is_insert)?;
    Ok(t.elapsed().as_secs_f64())
}

fn flush_ycsb(b: &mut Batch, e: &mut dyn Engine, insert: bool) -> Result<(), String> {
    if insert {
        b.flush(e)
    } else {
        b.flush_updates(e)
    }
}

/// Load `size` keys through `order`, batched, ending with one `sync` so a
/// buffered arm's number includes getting its tail to the device once. Returns
/// (seconds, device bytes written).
fn load(
    e: &mut dyn Engine,
    size: u64,
    plan: &Plan,
    payload: &Payload,
    order: impl Fn(u64) -> u64,
) -> Result<(f64, u64), String> {
    let mut vrng = Rng::new(0xE1);
    let mut buf = Batch::with_capacity(plan.batch, payload.value_size());
    let mut kb = [0u8; KEY_SIZE];
    let io0 = IoCounters::read_now();
    let t = Instant::now();
    for i in 0..size {
        db_key_into(order(i), &mut kb);
        buf.push(&kb, payload.get(&mut vrng));
        if buf.len() == plan.batch {
            buf.flush(e)?;
        }
    }
    buf.flush(e)?;
    e.sync()?;
    let secs = t.elapsed().as_secs_f64();
    let wrote = IoCounters::read_now().since(&io0).write_bytes;
    Ok((secs, wrote))
}

/// The durable-write floor: the load's records, framed as length-prefixed
/// key and value, appended to one file in batches of `plan.batch` with one
/// `fdatasync` closing each -- what a WAL does with nothing around it.
/// Records per second. A store's durable load cannot beat this on this
/// device; how far below it lands is the engine's own cost.
fn wal_floor(root: &Path, plan: &Plan, payload: &Payload) -> Result<f64, String> {
    let path = root.join("wal-floor.dat");
    let _ = std::fs::remove_file(&path);
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut vrng = Rng::new(0xF10);
    let mut kb = [0u8; KEY_SIZE];
    let mut buf: Vec<u8> = Vec::with_capacity(plan.batch * (KEY_SIZE + plan.value_size + 8));
    let records = WAL_FLOOR_BATCHES * plan.batch as u64;
    let t = Instant::now();
    for i in 0..records {
        db_key_into(i, &mut kb);
        let v = payload.get(&mut vrng);
        buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
        buf.extend_from_slice(&kb);
        buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
        buf.extend_from_slice(v);
        if (i + 1) % plan.batch as u64 == 0 {
            f.write_all(&buf).map_err(|e| e.to_string())?;
            f.sync_data().map_err(|e| e.to_string())?;
            buf.clear();
        }
    }
    let secs = t.elapsed().as_secs_f64();
    drop(f);
    let _ = std::fs::remove_file(&path);
    Ok(records as f64 / secs)
}

/// The sequential-read floor: a file of `bytes`, mapped and walked once
/// front to back, every word touched. Bytes per second. On the second and
/// later reps a file that fits in memory is served from the page cache,
/// which is also what a store that fits in memory sees; at `full` the file
/// does not fit and neither does the store.
fn scan_floor(root: &Path, bytes: u64) -> Result<f64, String> {
    let path = root.join("scan-floor.dat");
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) != bytes {
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut rng = Rng::new(0xF20);
        let mut chunk = vec![0u8; 1 << 20];
        let mut left = bytes;
        while left > 0 {
            for w in chunk.as_chunks_mut::<8>().0 {
                *w = rng.next().to_le_bytes();
            }
            let n = (left as usize).min(chunk.len());
            f.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
            left -= n as u64;
        }
        f.sync_all().map_err(|e| e.to_string())?;
    }
    let f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    // SAFETY: the file is private to this process and not written while mapped.
    let map = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| e.to_string())?;
    let t = Instant::now();
    let mut acc = 0u64;
    for w in map.as_chunks::<8>().0 {
        acc = acc.wrapping_add(u64::from_le_bytes(*w));
    }
    let secs = t.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    Ok(bytes as f64 / secs)
}

/// LMDB needs its map sized up front. Three times the raw payload, at least
/// 8 GB: sparse until used, so generosity costs nothing.
fn lmdb_map_gb(size: u64, value_size: usize) -> usize {
    let raw = size as f64 * (KEY_SIZE + value_size) as f64;
    ((raw * 3.0 / 1073741824.0).ceil() as usize).max(8)
}

/// The top rung for `full` on this machine: the store at least 1.5x memory.
pub fn full_top(mem_total_kb: u64, value_size: usize) -> u64 {
    let need = mem_total_kb as f64 * 1024.0 * 1.5;
    (need / (KEY_SIZE + value_size) as f64).ceil() as u64
}
