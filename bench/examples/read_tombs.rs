//! Point reads over a store a write mix has reached: the suite's ordered
//! load and its sync, then rounds of `put`s at random keys, each a delete and
//! an append, synced so that they seal into level-0 pieces carrying
//! tombstones, then one more round left in the memtable, then a pass of
//! uniform point reads. For an instruction count of the read path over the
//! store shape the write mixes leave:
//!
//!     read_tombs <arm> <size> <rounds> <puts a round> <reads>
//!
//! under `valgrind --tool=callgrind --collect-atstart=no
//! --toggle-collect='<supdb_bench::engines::Supdb as
//! supdb_bench::engines::Engine>::get'`, the count divided by `reads`.
use std::time::Instant;
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, Payload, Rng};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).cloned().unwrap_or("supdb-ingest".into());
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(300_000);
    let rounds: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let per: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let reads: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let payload = Payload::new(100, 0.5, 0xE1);
    let root = std::env::temp_dir().join(format!("supdb-readtombs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let map_gb = supdb_bench::run::lmdb_map_gb(size, payload.value_size());
    let mut e = engines::open(&arm, &root, map_gb).unwrap();
    let mut vrng = Rng::new(0xE1);
    let mut kb = [0u8; 16];
    let mut buf = Batch::with_capacity(1000, payload.value_size());
    for i in 0..size {
        db_key_into(i, &mut kb);
        buf.push(&kb, payload.get(&mut vrng));
        if buf.len() == 1000 {
            buf.flush(e.as_mut()).unwrap();
        }
    }
    if !buf.is_empty() {
        buf.flush(e.as_mut()).unwrap();
    }
    e.sync().unwrap();
    // The updates: a round of `per` puts at uniform keys, in commits of a
    // hundred as a mix's, synced after every round but the last.
    let mut krng = Rng::new(0x5EED);
    let mut items: Vec<([u8; 16], Vec<u8>)> = Vec::with_capacity(100);
    for round in 0..=rounds {
        for j in 0..per {
            let mut k = [0u8; 16];
            db_key_into(krng.next() % size, &mut k);
            items.push((k, payload.get(&mut vrng).to_vec()));
            if items.len() == 100 || j + 1 == per {
                let pairs: Vec<(&[u8], &[u8])> =
                    items.iter().map(|(k, v)| (&k[..], &v[..])).collect();
                e.update_batch(&pairs).unwrap();
                items.clear();
            }
        }
        if round < rounds {
            e.sync().unwrap();
        }
    }
    let c = e.counters();
    let get = |n: &str| c.iter().find(|(m, _)| *m == n).map_or(0.0, |x| x.1);
    println!(
        "{arm}: segs {:.0} parts {:.0} pieces {:.0} unsealed {:.0}",
        get("segments"),
        get("partitions"),
        get("pieces"),
        get("unsealed_keys")
    );
    let mut rrng = Rng::new(0xBEEF);
    let mut bytes = 0usize;
    let t = Instant::now();
    for _ in 0..reads {
        db_key_into(rrng.next() % size, &mut kb);
        bytes += e.get(&kb).unwrap();
    }
    let el = t.elapsed();
    println!(
        "{arm}: {reads} reads, {:.1} ns a read, {} bytes",
        el.as_secs_f64() * 1e9 / reads as f64,
        bytes
    );
    drop(e);
    let _ = std::fs::remove_dir_all(&root);
}
