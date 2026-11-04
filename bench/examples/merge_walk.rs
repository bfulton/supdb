//! One merge of a partition and the pieces over it, priced by its own
//! account (`SUPDB_MERGE_TRACE`'s `mergejob` line: the job's wall, its
//! thread's CPU and its major faults by pass) under each shape of the
//! merge's walk (`Options::merge_walk_cached`), in core and alternated.
//!
//! The store is built once: `KEYS` keys of the suite's shape into one
//! partition, then `PIECES` seals, each rewriting a shuffled `1/PIECES` of
//! the keys -- the full rewrite's shape, every key in the partition and in
//! one piece. Each trial copies it, opens the copy with the merge trigger
//! at the pieces' count and the walk under trial, scans once so the
//! segment work's loop plans the merge, and settles.
//!
//! usage: merge_walk [KEYS] [PIECES] [ROUNDS]
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::{Db, Options};

fn key(i: u64) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

fn opts(pieces: usize, cached: bool) -> Options {
    Options {
        // One partition of the whole store: a merge of it and its pieces.
        partition_bytes: Some(1 << 30),
        seal_bytes: 1 << 30,
        l0_trigger: pieces,
        merge_walk_cached: cached,
        // In core, as a store of this size is here: no form file.
        zpages: supdb::ZPages::Off,
        ..Options::default()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let keys: u64 = args
        .get(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let pieces: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(14);
    let rounds: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(3);
    let tmp = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let base = tmp.join(format!("merge-walk-base-{}", std::process::id()));
    let trial = tmp.join(format!("merge-walk-trial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    {
        // No merge while the pieces are made.
        let mut o = opts(1000, true);
        o.l0_trigger = 1000;
        let mut db = Db::create(&base, o).unwrap();
        let val = vec![b'v'; 100];
        for i in 0..keys {
            db.append(&key(i), &val);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        // A permutation of the keys, cut into one slice a piece.
        let mut order: Vec<u64> = (0..keys).collect();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for i in (1..order.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            order.swap(i, (x % (i as u64 + 1)) as usize);
        }
        let per = (keys as usize).div_ceil(pieces);
        let val = vec![b'w'; 100];
        for chunk in order.chunks(per) {
            let mut c = chunk.to_vec();
            c.sort_unstable();
            for &i in &c {
                db.append(&key(i), &val);
            }
            db.commit().unwrap();
            db.seal().unwrap();
        }
        db.settle().unwrap();
        eprintln!("base levels {:?}", db.levels());
        // Dropped, not closed: a close's flush would merge the pieces.
        drop(db);
    }
    for round in 0..rounds {
        let order = if round % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        };
        for cached in order {
            copy_dir(&base, &trial);
            let mut db = Db::open(&trial, opts(pieces, cached)).unwrap();
            let before = db.levels();
            let t = Instant::now();
            // One more piece: its landing finds the trigger's count and
            // plans the merge, which the settle joins.
            db.append(&key(keys), b"x");
            db.commit().unwrap();
            db.seal().unwrap();
            db.settle().unwrap();
            println!(
                "round {round} cached {cached} levels {before:?} -> {:?} scan+settle_ms {:.1}",
                db.levels(),
                t.elapsed().as_secs_f64() * 1e3
            );
            drop(db);
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&trial);
}
