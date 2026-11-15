//! One writer's whole round through a plain store, for an instruction
//! count by function: a shuffled load, a drain, bursts of appends with
//! scans between, point reads, counts, deletes, and a settle after each
//! stage.
//!
//! ```sh
//! cargo build --release --example work_probe
//! valgrind --tool=callgrind --callgrind-out-file=work.out \
//!     target/release/examples/work_probe 100000 [whole]
//! callgrind_annotate --threshold=100 work.out
//! ```
//!
//! The store's own threads run under the count too, and how often they
//! look and park is the clock's, so the figure to read is a function's
//! own instructions against the same function's in another build of the
//! same source of this file, never the total.

use supdb::{Db, Options};

fn key(i: u64) -> [u8; 16] {
    let mut k = [b'0'; 16];
    let mut v = i;
    for b in k.iter_mut().rev() {
        *b = b'0' + (v % 10) as u8;
        v /= 10;
    }
    k
}

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100_000);
    let dir = std::env::temp_dir().join(format!("supdb-work-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Seals, a partitioning and merges at this size: a megabyte a seal
    // and a partition, two pieces a merge. With `whole` as the second
    // argument the store keeps the engine's sizes, and the round is the
    // writer's and the upkeep thread's alone, which is the same work in
    // every run.
    let opts = if std::env::args().nth(2).as_deref() == Some("whole") {
        Options::default()
    } else {
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1 << 20),
            l0_trigger: 2,
            ..Options::default()
        }
    };
    let mut db = Db::create(&dir, opts).expect("create");
    let value = [b'v'; 100];
    // A shuffled load: a multiplier coprime to the count walks every key
    // once, far from the one before.
    let step = 7_919u64;
    let mut at = 0u64;
    for i in 0..n {
        at = (at + step) % n;
        db.put(&key(at), &value);
        if i % 1000 == 999 {
            db.commit().expect("commit");
        }
    }
    db.commit().expect("commit");
    db.sync().expect("sync");
    db.settle().expect("settle");
    let mut sink = 0u64;
    // Bursts of appends over every tenth key, a scan pass after each.
    for round in 0..4u64 {
        for i in 0..n / 10 {
            db.append(&key((i * 10 + round) % n), &value[..40]);
            if i % 1000 == 999 {
                db.commit().expect("commit");
            }
        }
        db.commit().expect("commit");
        for s in 0..n / 200 {
            db.scan(&key((s * 197) % n), 100, |k, v| {
                sink += u64::from(k[15]) + v.len() as u64;
            })
            .expect("scan");
        }
    }
    db.sync().expect("sync");
    db.settle().expect("settle");
    for i in 0..n {
        db.read_all(&key((i * 7_907) % n), |v| sink += v.len() as u64)
            .expect("read");
    }
    for i in 0..n / 2 {
        sink += db.count(&key((i * 6_151) % n)).expect("count");
    }
    for i in 0..n / 20 {
        db.delete(&key(i * 20));
        if i % 1000 == 999 {
            db.commit().expect("commit");
        }
    }
    db.commit().expect("commit");
    db.sync().expect("sync");
    db.settle().expect("settle");
    for s in 0..n / 200 {
        db.scan(&key((s * 197) % n), 100, |k, v| {
            sink += u64::from(k[15]) + v.len() as u64;
        })
        .expect("scan");
    }
    println!("{sink}");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
