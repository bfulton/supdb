//! Apparent bytes against allocated blocks, per arm, after the suite's load.
//!
//! The evidence behind two choices `bytes_on_disk_per_byte` had to make,
//! both of which had a plausible wrong answer.
//!
//! What to count. A sparse file would make apparent length overstate the
//! disk a store holds, and LMDB is opened with a map sized at three times
//! the records -- but it grows the file rather than truncating to the map,
//! and on these arms apparent and allocated agree within a thousandth. So
//! blocks is robustness against a file none of these arms leaves, not a
//! correction to one they do; it is kept because it is what `du` reports
//! and what a store that did reserve a map would deserve to be charged.
//!
//! Where to read it. The suite reads it where the load's guarantee is met,
//! not once the engine has settled. `rocksdb-tuned` holds its memtable
//! there, so its bytes are a WAL; `rocksdb-tuned-drain`, which is not an
//! arm and is constructed here only to answer this, flushes and compacts.
//! At 300 000 keys that is 1.03 against 1.00 bytes per byte stored.
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use supdb_bench::engines::{self, Batch, Engine, Rocks};
use supdb_bench::workload::{db_key_into, Payload, Rng};

fn walk(p: &Path, apparent: &mut u64, allocated: &mut u64, files: &mut Vec<(String, u64, u64)>) {
    let Ok(rd) = std::fs::read_dir(p) else { return };
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() {
            walk(&e.path(), apparent, allocated, files);
        } else {
            *apparent += m.len();
            *allocated += m.blocks() * 512;
            files.push((
                e.file_name().to_string_lossy().into_owned(),
                m.len(),
                m.blocks() * 512,
            ));
        }
    }
}

fn main() {
    let mut a = std::env::args().skip(1);
    let size: u64 = a.next().unwrap().parse().unwrap();
    let arms: Vec<String> = a.next().unwrap().split(',').map(str::to_string).collect();
    let base = std::path::PathBuf::from(
        "/tmp/claude-0/-home-user-supdb/ee7b8ded-0bc9-50fc-95d2-e84de73c4b53/scratchpad/ssz",
    );
    let payload = Payload::new(100, 0.5, 0xE1);
    let stored = size * (16 + 100);
    // The map LMDB is opened with, as run.rs sizes it.
    println!("{size} keys, {stored} B stored");
    for arm in &arms {
        let d = base.join(arm);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let map_gb = ((stored * 3) / (1 << 30) + 1) as usize;
        // `rocksdb-tuned-drain` is not in ARMS, so it is constructed
        // directly: it is here only to say what RocksDB's store weighs once
        // its memtable has been flushed and compacted, which its shipping
        // arm never does inside a load.
        let mut e: Box<dyn Engine> = if arm == "rocksdb-tuned-drain" {
            Box::new(Rocks::create_tuned_drain(&d).unwrap())
        } else {
            engines::open(arm, &d, map_gb).unwrap()
        };
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
        let (mut ap, mut al, mut files) = (0u64, 0u64, Vec::new());
        walk(&d, &mut ap, &mut al, &mut files);
        // What the same store weighs once it has settled: a second sync on
        // an engine whose first one left work undone.
        e.sync().unwrap();
        let (mut ap2, mut al2, mut f2) = (0u64, 0u64, Vec::new());
        walk(&d, &mut ap2, &mut al2, &mut f2);
        println!(
            "  {arm:<16} after a second sync: apparent {ap2} ({:.2} B/B) allocated {al2}",
            ap2 as f64 / stored as f64
        );
        println!(
            "  {arm:<16} apparent {:>12} ({:.2} B/B)   allocated {:>12} ({:.2} B/B)   size_bytes() {:>12}",
            ap,
            ap as f64 / stored as f64,
            al,
            al as f64 / stored as f64,
            e.size_bytes()
        );
        files.sort_by_key(|f| std::cmp::Reverse(f.1));
        for (n, l, bl) in files.iter().take(3) {
            println!("      {n:<52} len {l:>12}  blocks*512 {bl:>12}");
        }
        drop(e);
        let _ = std::fs::remove_dir_all(&d);
    }
}
