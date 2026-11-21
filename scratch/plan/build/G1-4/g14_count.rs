//! Scratch probe for G1-4 round 1 (not committed): the slice arms of
//! `Blob::record_offset`, `willneed_ranks` and `willneed_dir` called once a
//! rank over a mapped partition, each loop a function of its own, for a
//! callgrind count by function at the base and the tree.
//! usage: g14_count DIR
use std::hint::black_box;
use std::path::Path;
use supdb::bytes::MmapBytes;
use supdb::{Blob, Db, Options};

fn key(i: u64) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

#[inline(never)]
fn offsets(b: &Blob<MmapBytes>) -> u64 {
    let mut s = 0u64;
    for r in 0..b.keys() {
        s = s.wrapping_add(black_box(b).record_offset(black_box(r)).unwrap_or(0));
    }
    s
}

#[inline(never)]
fn ranks(b: &Blob<MmapBytes>) -> usize {
    let mut s = 0;
    for r in (0..b.keys()).step_by(64) {
        s += black_box(b).willneed_ranks(black_box(r), 64, usize::MAX);
    }
    s
}

#[inline(never)]
fn dirs(b: &Blob<MmapBytes>) -> usize {
    let mut s = 0;
    for r in (0..b.keys()).step_by(64) {
        s += black_box(b).willneed_dir(black_box(r), 64);
    }
    s
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = Path::new(&a[1]);
    let _ = std::fs::remove_dir_all(dir);
    let o = Options {
        partition_bytes: Some(1 << 30),
        seal_bytes: 1 << 30,
        zpages: supdb::ZPages::Off,
        ..Options::default()
    };
    let mut db = Db::create(dir, o).unwrap();
    let val = vec![b'v'; 100];
    for i in 0..20_000u64 {
        db.append(&key(i), &val);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    drop(db);
    let mut seg = None;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("par-") && n.ends_with(".sup") {
            seg = Some(e.path());
        }
    }
    let seg = seg.expect("a partition");
    let b = Blob::open(MmapBytes::open(&seg).unwrap()).unwrap();
    println!("keys {} offsets {} ranks {} dirs {}", b.keys(), offsets(&b), ranks(&b), dirs(&b));
}
