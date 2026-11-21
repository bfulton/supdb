//! Scratch probe for G1-4 (not committed): one merge with
//! `merge_drop_behind` on, for strace of its advice calls.
//! usage: g14_drop build DIR | g14_drop merge FROM TRIAL
use std::path::Path;
use supdb::{Db, Options};

fn key(i: u64) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

fn opts(trigger: usize, drop: bool) -> Options {
    let ahead = std::env::var("G14_AHEAD").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    Options {
        merge_read_ahead: ahead,
        partition_bytes: Some(1 << 30),
        seal_bytes: 1 << 30,
        l0_trigger: trigger,
        merge_drop_behind: drop,
        merge_drop_chunk: 64 << 10,
        zpages: supdb::ZPages::Off,
        ..Options::default()
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let keys = 60_000u64;
    match a[1].as_str() {
        "build" => {
            let dir = Path::new(&a[2]);
            let _ = std::fs::remove_dir_all(dir);
            let mut db = Db::create(dir, opts(1000, false)).unwrap();
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
            eprintln!("levels {:?}", db.levels());
            drop(db);
        }
        _ => {
            let (from, trial) = (Path::new(&a[2]), Path::new(&a[3]));
            let _ = std::fs::remove_dir_all(trial);
            std::fs::create_dir_all(trial).unwrap();
            for e in std::fs::read_dir(from).unwrap().flatten() {
                if e.file_type().unwrap().is_file() {
                    std::fs::copy(e.path(), trial.join(e.file_name())).unwrap();
                }
            }
            let mut db = Db::open(trial, opts(4, true)).unwrap();
            db.append(&key(keys), b"x");
            db.commit().unwrap();
            db.seal().unwrap();
            db.settle().unwrap();
            eprintln!("levels {:?}", db.levels());
            drop(db);
        }
    }
}
