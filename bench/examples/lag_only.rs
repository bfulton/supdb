//! The suite's lag sweep alone on one arm: the shuffled load and its sync,
//! then each lag point's updates and scans, nothing read between a burst
//! and its scans. After each pass it reads the store's counters, which
//! take the writer's upkeep home -- harmless there, since the pass is
//! timed and over -- and prints what the upkeep thread spent over the
//! point against the writes it had to file.
use std::time::Instant;
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, KeyDist, KeyGen, Payload, Permutation, Rng};

fn get(c: &[(&'static str, f64)], n: &str) -> f64 {
    c.iter().find(|(m, _)| *m == n).map_or(0.0, |x| x.1)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).cloned().unwrap_or("supdb-ingestbglag".into());
    let size: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let reps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let payload = Payload::new(100, 0.5, 0xE1);
    let root = std::env::temp_dir().join(format!("supdb-lagonly-{}", std::process::id()));
    let mut kb = [0u8; 16];
    for rep in 0..reps {
        let dir = root.join(format!("r{rep}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = engines::open(&arm, &dir, 1).unwrap();
        let perm = Permutation::new(size, 0x5EED);
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        for i in 0..size {
            db_key_into(perm.at(i), &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        e.sync().unwrap();
        let scans = (size / 100).max(1);
        let scan_keys = size.saturating_sub(100).max(1);
        let mut vrng = Rng::new(0x1A5);
        let mut updated = 0u64;
        let mut ug = KeyGen::new(KeyDist::Uniform, size, 0x1A6);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        let mut line = format!("rep {rep}:");
        let mut c0 = e.counters();
        for pct in [0u64, 1, 10, 100] {
            let want = size * pct / 100;
            let wrote = want.saturating_sub(updated);
            let tw = Instant::now();
            while updated < want {
                db_key_into(ug.next(), &mut kb);
                buf.push(&kb, payload.get(&mut vrng));
                updated += 1;
                if buf.len() == 1000 {
                    buf.flush_updates(e.as_mut()).unwrap();
                }
            }
            if !buf.is_empty() {
                buf.flush_updates(e.as_mut()).unwrap();
            }
            if pct == 0 {
                e.sync().unwrap();
            }
            let burst = tw.elapsed().as_secs_f64() * 1e3;
            let mut g3 = KeyGen::new(KeyDist::Uniform, scan_keys, 0x1A7);
            let t = Instant::now();
            for _ in 0..scans {
                db_key_into(g3.next(), &mut kb);
                e.range(&kb, 100).unwrap();
            }
            let pass = t.elapsed().as_secs_f64() * 1e3;
            let c1 = e.counters();
            let up = get(&c1, "upkeep_ms") - get(&c0, "upkeep_ms");
            let passes = get(&c1, "upkeep_passes") - get(&c0, "upkeep_passes");
            let built = get(&c1, "blk_by_engine") - get(&c0, "blk_by_engine");
            let skipped = get(&c1, "upkeep_skipped") - get(&c0, "upkeep_skipped");
            let scans_n = get(&c1, "rd_scans") - get(&c0, "rd_scans");
            let blockpath = get(&c1, "rd_blockpath") - get(&c0, "rd_blockpath");
            let seals = get(&c1, "seals") - get(&c0, "seals");
            let pubs = get(&c1, "publishes") - get(&c0, "publishes");
            let snaps = get(&c1, "snapshot_builds") - get(&c0, "snapshot_builds");
            let layout = format!(
                "parts {:.0} pieces {:.0} unsealed {:.0} forms {:.0}",
                get(&c1, "partitions"),
                get(&c1, "pieces"),
                get(&c1, "unsealed_keys"),
                get(&c1, "forms_held_at_end")
            );
            let ct = if wrote > 0 {
                up * 1e3 / wrote as f64
            } else {
                0.0
            };
            let cw = if wrote > 0 {
                burst * 1e3 / wrote as f64
            } else {
                0.0
            };
            line.push_str(&format!(
                "\n  lag{pct} {burst:.1}+{pass:.1}ms up {up:.1}ms/{passes:.0}p c_t {ct:.2} c_w {cw:.2} built {built:.0} skipped {skipped:.0} blockpath {blockpath:.0}/{scans_n:.0} seals {seals:.0} pubs {pubs:.0} snaps {snaps:.0} | {layout}"
            ));
            c0 = c1;
        }
        println!("{line}");
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
