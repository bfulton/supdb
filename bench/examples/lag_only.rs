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
        // LAG_PCTS=0,1,10 runs a prefix of the sweep, for a profile of one point.
        let pcts: Vec<u64> = std::env::var("LAG_PCTS")
            .ok()
            .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![0, 1, 10, 100]);
        for pct in pcts {
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
            let mut first = 0.0f64;
            for i in 0..scans {
                db_key_into(g3.next(), &mut kb);
                e.range(&kb, 100).unwrap();
                if i == 0 {
                    first = t.elapsed().as_secs_f64() * 1e3;
                }
            }
            let pass = t.elapsed().as_secs_f64() * 1e3;
            let c1 = e.counters();
            let up = get(&c1, "upkeep_ms") - get(&c0, "upkeep_ms");
            let cpu = (get(&c1, "upkeep_cpu_us") - get(&c0, "upkeep_cpu_us")) / 1e3;
            let barriers = get(&c1, "pin_barriers") - get(&c0, "pin_barriers");
            let passes = get(&c1, "upkeep_passes") - get(&c0, "upkeep_passes");
            let built = get(&c1, "blk_by_engine") - get(&c0, "blk_by_engine");
            let skipped = get(&c1, "upkeep_skipped") - get(&c0, "upkeep_skipped");
            let scans_n = get(&c1, "rd_scans") - get(&c0, "rd_scans");
            let blockpath = get(&c1, "rd_blockpath") - get(&c0, "rd_blockpath");
            let seals = get(&c1, "seals") - get(&c0, "seals");
            let pubs = get(&c1, "publishes") - get(&c0, "publishes");
            let snaps = get(&c1, "snapshot_builds") - get(&c0, "snapshot_builds");
            let refr = get(&c1, "snapshot_refreshes") - get(&c0, "snapshot_refreshes");
            let swit = get(&c1, "snap_switches") - get(&c0, "snap_switches");
            let ext = get(&c1, "snapshot_extends") - get(&c0, "snapshot_extends");
            let d = |n: &str| get(&c1, n) - get(&c0, n);
            let upkeep_built = format!(
                "first {first:.2}ms up_built {:.0} drops {:.0} ahead {:.0}/{:.0}/{:.0} notable {:.0} fill {:.0}/{:.0} bld skip {:.0}/{:.0} built {:.0} inst drop {:.0} skip {:.0} walks {:.0}/{:.0}/{:.0}",
                d("blk_by_upkeep"),
                d("table_drops"),
                d("ahead_starts"),
                d("ahead_stops"),
                d("ahead_installed"),
                d("settle_notable"),
                d("fill_dense"),
                d("fill_fresh"),
                d("ahead_skip_held"),
                d("ahead_skip_overlay"),
                d("ahead_built"),
                d("install_gen_drop"),
                d("install_slot_skip"),
                d("walk_copy"),
                d("walk_sparse"),
                d("walk_other")
            );
            let phases = format!(
                "ph take {:.2} sync {:.2} settle {:.2} snap {:.2} ahead {:.2} install {:.2} ms, wait {:.2}",
                d("scan_take_us") / 1e3,
                d("scan_sync_us") / 1e3,
                d("scan_settle_us") / 1e3,
                d("scan_snap_us") / 1e3,
                d("scan_ahead_us") / 1e3,
                d("scan_install_us") / 1e3,
                d("upkeep_wait_us") / 1e3
            );
            let phases = format!("{phases} partial {:.0}", d("upkeep_partial"));
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
                "\n  lag{pct} {burst:.1}+{pass:.1}ms up {up:.1}ms/{passes:.0}p cpu {cpu:.1}ms mb {barriers:.0} c_t {ct:.2} c_w {cw:.2} built {built:.0} skipped {skipped:.0} blockpath {blockpath:.0}/{scans_n:.0} seals {seals:.0} pubs {pubs:.0} snaps {snaps:.0} refr {refr:.0} swit {swit:.0} ext {ext:.0} | {layout} | {upkeep_built} | {phases}"
            ));
            c0 = c1;
        }
        println!("{line}");
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
