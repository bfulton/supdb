//! The segment work on the writer's thread, for a callgrind toggle to
//! collect: a store whose landings, merges, promotions and manifest
//! publishes the writer drives itself (`Options::publish_in_background`
//! off, `Upkeep::Inline`), so that `Maint`'s functions and the sweep of
//! the retired states at an operation's end run inside one function
//! (`rounds`) on one thread, with no background thread's pin or poll to
//! make their counts follow the clock. The probes with no toggle run the
//! same functions on the store's own threads, where a count moves between
//! two runs of one build; this one holds them to the instruction.
//!
//! The store is `KEYS` keys of the suite's shape in one partition. Each
//! round rewrites a shuffled `1/PIECES` of them and seals the batch into a
//! piece, then scans, so that the next landing is near reads and takes the
//! piece's ranks and bounds before its publish; every `PIECES` rounds the
//! pieces reach the merge trigger and the next landing merges them into
//! the partition. The last round settles. `scripts/cost.sh`'s `land` probe
//! counts it by function against a base, an operation a key rewritten.
//!
//!     land_probe [KEYS] [ROUNDS] [PIECES]
use std::path::PathBuf;
use std::sync::Arc;
use supdb::{Db, Options, Upkeep, ZInject, ZPages};

fn key(i: u64) -> [u8; 16] {
    let mut k = [0u8; 16];
    supdb_bench::workload::db_key_into(i, &mut k);
    k
}

fn opts(pieces: usize) -> Options {
    // The platform's readings through the store's seam, the budget and
    // the group's refaults: read from the cgroup's files on a period,
    // they are counted by where the period falls.
    let inject = Arc::new(ZInject::new());
    inject.set_budget(1 << 40);
    inject.set_refault_rate(0.0);
    Options {
        // One partition of the whole store, and seals only where a round
        // asks for one.
        partition_bytes: Some(1 << 30),
        seal_bytes: 1 << 30,
        l0_trigger: pieces,
        publish_in_background: false,
        upkeep: Upkeep::Inline,
        // In core: no form file and no controller on a clock.
        zpages: ZPages::Off,
        zpages_inject: Some(inject),
        ..Options::default()
    }
}

/// `rounds` rounds of rewrites, seals and scans over `db`: what the
/// toggle collects. The keys rewritten, the operations.
#[inline(never)]
fn rounds(db: &mut Db, order: &[u64], per: usize, val: &[u8]) -> u64 {
    let mut n = 0u64;
    let mut seen = 0usize;
    for chunk in order.chunks(per) {
        let mut c = chunk.to_vec();
        c.sort_unstable();
        for &i in &c {
            db.put(&key(i), val);
        }
        db.commit().expect("the round commits");
        db.seal().expect("the round seals");
        // The seal landed, and any merge it was the trigger for, before
        // the scan: inline, a landing falls at whichever of the writer's
        // operations finds the seal's thread done, and the scan reads
        // what has landed.
        db.settle().expect("the round settles");
        seen += db
            .scan(&key(chunk[0]), 100, |_, v| {
                std::hint::black_box(v);
            })
            .expect("the scan reads");
        n += c.len() as u64;
    }
    db.settle().expect("the store settles");
    std::hint::black_box(seen);
    n
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let keys: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(50_000);
    let rounds_n: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(8);
    let pieces: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(4);
    let tmp = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
    // Named without the process id, so its path is as long in every run:
    // the cost count (scripts/cost.sh) compares two runs' file calls by
    // their instructions. Two runs at once want a TMPDIR each.
    let dir = tmp.join("supdb-landprobe");
    let _ = std::fs::remove_dir_all(&dir);
    let mut db = Db::create(&dir, opts(pieces)).expect("the store is made");
    let val = vec![b'v'; 100];
    for i in 0..keys {
        db.append(&key(i), &val);
    }
    db.commit().expect("the load commits");
    db.flush().expect("the load flushes");
    db.settle().expect("the load settles");
    // A permutation of the keys, cut into one slice a round.
    let mut order: Vec<u64> = (0..keys).collect();
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for i in (1..order.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        order.swap(i, (x % (i as u64 + 1)) as usize);
    }
    let per = (keys as usize).div_ceil(pieces);
    let take = (per * rounds_n).min(order.len() * rounds_n);
    let order: Vec<u64> = order.iter().cycle().take(take).copied().collect();
    let val = vec![b'w'; 100];
    let n = rounds(&mut db, &order, per, &val);
    println!("rounds {rounds_n} keys {n} levels {:?}", db.levels());
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
