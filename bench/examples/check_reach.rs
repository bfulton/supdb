//! What a read's checksum switch can reach in the suite's shape.
//!
//! The supdb arms write their blocks with no CRC and RocksDB's read with
//! `verify_checksums` off (`bench/DESIGN.md`, what each engine verifies).
//! A block CRC is read only where a read reaches a value block, and a
//! RocksDB block checksum only where a read fetches a block from a table
//! file, so what each switch reaches is a count: every block the supdb
//! store's segments hold, and every table file the RocksDB store holds,
//! after the suite's ordered load, its shuffled load into a fresh store,
//! and a full rewrite of either, each followed by the arm's `sync` and
//! `settle`. An arm of any other engine prints its files' count alone.
//!
//! `check_reach SIZE ARM[,ARM...] [VALUE]`: `VALUE`, the value's bytes, is
//! the suite's 100 unless given, and is there for the census's control, a
//! value past `Options::inline_bytes` that a block has to hold.
use std::path::Path;
use supdb::{Blob, MmapBytes};
use supdb_bench::engines::{self, Batch, Engine};
use supdb_bench::workload::{db_key_into, Payload, Permutation, Rng};

/// Segments, their keys and their blocks, and table files, under `dir`.
fn census(dir: &Path) -> (usize, usize, usize, usize) {
    let (mut segs, mut keys, mut blocks, mut tables) = (0, 0, 0, 0);
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        tables += usize::from(name.ends_with(".sst"));
        if name.ends_with(".sup") {
            let b = Blob::open(MmapBytes::open(&e.path()).unwrap()).unwrap();
            segs += 1;
            keys += b.keys();
            blocks += b.blocks();
        }
    }
    (segs, keys, blocks, tables)
}

/// The suite's batches of a thousand, in the order `key` gives.
fn load(e: &mut dyn Engine, size: u64, value: usize, key: impl Fn(u64) -> u64, seed: u64) {
    let payload = Payload::new(value, 0.5, seed);
    let mut vr = Rng::new(seed ^ 1);
    let mut b = Batch::with_capacity(1000, value);
    let mut kb = [0u8; 16];
    for i in 0..size {
        db_key_into(key(i), &mut kb);
        b.push(&kb, payload.get(&mut vr));
        if b.len() >= 1000 {
            b.flush(e).unwrap();
        }
    }
    b.flush(e).unwrap();
    e.sync().unwrap();
    e.settle().unwrap();
}

fn main() {
    let mut a = std::env::args().skip(1);
    let size: u64 = a.next().unwrap().parse().unwrap();
    let arms: Vec<String> = a.next().unwrap().split(',').map(str::to_string).collect();
    let value: usize = a.next().map_or(100, |v| v.parse().unwrap());
    let base = std::env::temp_dir().join(format!("supdb-check-reach-{}", std::process::id()));
    let perm = Permutation::new(size, 0xC4);
    let map_gb = ((size * (16 + value as u64) * 3) / (1 << 30) + 1) as usize;
    println!("{size} keys of {value}-byte values: segments, their keys, their blocks, table files");
    for arm in &arms {
        for shuffled in [false, true] {
            let d = base.join(arm);
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            let mut e = engines::open(arm, &d, map_gb).unwrap();
            let what = if shuffled { "load-shuffled" } else { "load" };
            load(
                e.as_mut(),
                size,
                value,
                |i| if shuffled { perm.at(i) } else { i },
                0xE1,
            );
            println!("{arm} {what}: {:?}", census(&d));
            load(e.as_mut(), size, value, |i| perm.at(i), 0xE3);
            println!("{arm} {what} and a full rewrite: {:?}", census(&d));
            drop(e);
            let _ = std::fs::remove_dir_all(&d);
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}
