//! A load on one arm with what the process holds printed as it goes: the
//! suite's shuffled order (`Permutation`, the shuffled load's seed) or key
//! order, `size` keys through `Batch` flushes of a thousand, and every
//! `every` batches a line of the process's anonymous bytes beside the
//! engine's census by owner (`Engine::memory_census`), then the `sync` the
//! suite's load ends in and a last line. For the owner that grows under a
//! load a bound kills: the census takes the writer's upkeep home at each
//! line, so the lines are for who holds what and not for the load's time.
//! A thread samples the anonymous bytes every ten milliseconds and each
//! store ends in a `peak` line, the load's and the sync's apart, since a
//! sync's merges hold what no census line between batches is there to
//! see. `reps` loads run in one process, each into a fresh store dropped
//! and trimmed after as the runner's passes are (`release_freed_memory`),
//! since what one store leaves in the process is a variable of the next.
//!
//!     load_census <arm> <size> <every> [shuffled|ordered] [reps]
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;
use supdb_bench::engines::{self, Batch, Engine};
use supdb_bench::workload::{db_key_into, Payload, Permutation, Rng};

/// The process's anonymous resident bytes, from the kernel.
fn rss_anon() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("RssAnon:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb << 10)
}

fn line(e: &dyn Engine, at: u64, t: &Instant) {
    let census = e.memory_census();
    let mut parts: Vec<String> = census
        .iter()
        .filter(|(_, b)| *b >= 1 << 20)
        .map(|(k, b)| format!("{k} {}", b >> 20))
        .collect();
    parts.sort();
    // SAFETY: mallinfo2 reads the allocator's own totals.
    let mi = unsafe { libc::mallinfo2() };
    println!(
        "at {at} s {:.1} anon_mb {} inuse_mb {} mmap_mb {} | {}",
        t.elapsed().as_secs_f64(),
        rss_anon() >> 20,
        (mi.uordblks + mi.hblkhd) >> 20,
        mi.hblkhd >> 20,
        parts.join(" ")
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).cloned().unwrap_or("supdb".into());
    let size: u64 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000_000);
    let every: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(250);
    let shuffled = args.get(4).is_none_or(|s| s != "ordered");
    let reps: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(1);
    // The sampler: the phase the main thread is in, and the peak of each.
    let phase = Arc::new(AtomicU8::new(0));
    let peaks: Arc<[AtomicU64; 3]> = Arc::new(Default::default());
    {
        let (phase, peaks) = (phase.clone(), peaks.clone());
        std::thread::spawn(move || loop {
            let p = phase.load(Ordering::Relaxed) as usize;
            peaks[p.min(2)].fetch_max(rss_anon(), Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(10));
        });
    }
    let payload = Payload::new(100, 0.5, 0xE1);
    for rep in 0..reps {
        let dir =
            std::env::temp_dir().join(format!("supdb-loadcensus-{}-{rep}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for p in peaks.iter() {
            p.store(0, Ordering::Relaxed);
        }
        let mut e = engines::open(&arm, &dir, 1).unwrap();
        let perm = Permutation::new(size, 0x5EED);
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut batches = 0u64;
        phase.store(1, Ordering::Relaxed);
        for i in 0..size {
            let k = if shuffled { perm.at(i) } else { i };
            db_key_into(k, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
                batches += 1;
                if batches.is_multiple_of(every) {
                    line(e.as_ref(), i + 1, &t);
                }
            }
        }
        buf.flush(e.as_mut()).unwrap();
        line(e.as_ref(), size, &t);
        let t_sync = Instant::now();
        phase.store(2, Ordering::Relaxed);
        e.sync().unwrap();
        let sync_s = t_sync.elapsed().as_secs_f64();
        std::thread::sleep(std::time::Duration::from_millis(20));
        phase.store(0, Ordering::Relaxed);
        line(e.as_ref(), size, &t);
        println!(
            "peak rep {rep} load_mb {} sync_mb {} sync_s {sync_s:.1}",
            peaks[1].load(Ordering::Relaxed) >> 20,
            peaks[2].load(Ordering::Relaxed) >> 20,
        );
        let keep = [
            "seals",
            "publishes",
            "freeze_ms",
            "join_wait_ms",
            "publish_ms",
            "seal_sort_ms",
            "seal_snap_ms",
            "upkeep_ms",
            "upkeep_passes",
            "partitions",
            "pieces",
            "merge_ms",
            "merges",
            "zpage_builds",
        ];
        let c: Vec<String> = e
            .counters()
            .iter()
            .filter(|(k, _)| keep.contains(k))
            .map(|(k, v)| format!("{k} {v:.0}"))
            .collect();
        println!("counters rep {rep} {}", c.join(" "));
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: `malloc_trim` takes no pointer and only returns free
        // memory to the system, as the runner does between passes.
        unsafe {
            libc::malloc_trim(0);
        }
        println!("trimmed rep {rep} anon_mb {}", rss_anon() >> 20);
    }
}
