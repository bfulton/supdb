//! Scratch probe for G1-4 round 1 (not committed): a plain store of one
//! partition and three pieces with the gate set away from the raw form by
//! hand (`Db::zgate_set`, no form file: `ZPages::Off`), so the block cache's
//! builds and the settles ask ahead (`blocks_willneed`, `pieces_willneed`,
//! `pieces_dir_willneed`) through `Blob::willneed_ranks` and `willneed_dir`,
//! for a callgrind count by function at the base and the tree.
//! usage: g14_gate DIR
use std::path::Path;
use supdb::{Db, Options};

fn key(i: u64) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = Path::new(&a[1]);
    let _ = std::fs::remove_dir_all(dir);
    let o = Options {
        partition_bytes: Some(1 << 30),
        seal_bytes: 1 << 30,
        l0_trigger: 1000,
        zpages: supdb::ZPages::Off,
        ..Options::default()
    };
    let keys = 60_000u64;
    let mut db = Db::create(dir, o).unwrap();
    let val = vec![b'v'; 100];
    for i in 0..keys {
        db.append(&key(i), &val);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    for p in 0..3u64 {
        let val = vec![b'w'; 100];
        for i in (p..keys).step_by(3) {
            db.append(&key(i), &val);
        }
        db.commit().unwrap();
        db.seal().unwrap();
    }
    db.settle().unwrap();
    db.zgate_set(1);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut n = 0usize;
    for round in 0..20u64 {
        for _ in 0..50 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = key(x % keys);
            n += db.scan(&k, 50, |_, v| n_add(v)).unwrap();
        }
        for j in 0..100u64 {
            db.append(&key((round * 977 + j * 613) % keys), b"z");
        }
        db.commit().unwrap();
    }
    db.settle().unwrap();
    println!("scanned {n} levels {:?}", db.levels());
}

#[inline(never)]
fn n_add(v: &[u8]) {
    std::hint::black_box(v);
}
